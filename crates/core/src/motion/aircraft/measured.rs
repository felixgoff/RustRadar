//! Measured data for the selected aircraft (`motion.ts` `measure`, `applyMeasured`,
//! `measuredQnh`, `:1141-1168`, `:1625-1690`; doc 05 §10.12).
//!
//! A second source (adsb.lol's readsb fields) measures the selected aircraft every few seconds.
//! Its position joins the track's samples in time order (a newer one becomes the latest
//! report: position, ground flag, altitude, speed, track, and the acceleration when 4-120 s
//! since the last); its roll and crab are kept as half-minute series, so that they are shown at
//! the drawn time rather than on arrival; while fresh it replaces the estimated turn and
//! vertical rates, the airspeed and (off LNAV) the turn's end ([`measured_rates`]); and its
//! altimeter setting below 3000 ft above the field teaches the field's QNH. Every change is
//! blended in like a new report, so nothing jumps.
//!
//! `measuredRates` and `measuredAttitude`, which every report and refit also call, are in
//! [`phase`](super::phase) and [`evaluate`](super::evaluate).

use super::altitude::{add_altitude, corrected, valid_qnh};
use super::consts::*;
use super::delay::note_arrival;
use super::phase::{decide_phase, measured_rates};
use super::samples::merge_measured;
use super::track::Track;
use super::{Measured, Motion, World, clamp, field_fade, push_series, same_airport, wrap_lon, wrap180, wrap360};
use crate::trains::js_max;

/// Whether an optional measured value is there and finite (`Number.isFinite(m.x)`).
fn finite(x: Option<f64>) -> Option<f64> {
    x.filter(|v| v.is_finite())
}

impl Motion {
    /// Measured state for an aircraft (the selected one), from a second source; `None` drops
    /// it. Call on each fetch, not per frame. Its position joins the track's samples in time
    /// order; its heading, bank, turn and vertical rates, airspeed and autopilot targets replace
    /// the estimates while fresh, and every change is blended in like a new report, so nothing
    /// jumps. Nothing happens before the aircraft's first Flightradar24 report (the next fetch
    /// will do), or when `m` holds nothing newer than what was taken in.
    pub fn measure(&mut self, id: u32, m: Option<&Measured>, now_ms: f64) {
        let Motion { tracks, world } = self;
        let Some(tr) = tracks.get_mut(&id) else {
            return;
        };
        if tr.n == 0 {
            return;
        }
        let Some(m) = m else {
            if tr.meas.is_none() && tr.meas_ms == f64::NEG_INFINITY {
                return;
            }
            world.blend_around(tr, now_ms, |w, tr| {
                tr.meas = None;
                tr.meas_ms = f64::NEG_INFINITY;
                tr.crab = 0.0;
                tr.tas_ms = 0.0;
                tr.nav_track = f64::NAN;
                tr.roll_ts.clear();
                tr.rolls.clear();
                tr.roll_biases.clear();
                tr.crab_ts.clear();
                tr.crabs.clear();
                w.refresh(tr);
            });
            return;
        };
        let positioned = finite(m.position_ms).is_some() && finite(m.lat).is_some() && finite(m.lon).is_some();
        let new_position = positioned && m.position_ms.is_some_and(|pos_ms| pos_ms > tr.meas_pos_ms);
        if !new_position && !(m.seen_ms > tr.meas_ms) {
            return; // nothing new
        }
        tr.touched = now_ms;
        world.blend_around(tr, now_ms, |w, tr| w.apply_measured(tr, m, new_position, now_ms));
    }
}

impl World {
    /// Takes in a measured state; a newer position (`new_position`: `m` has a finite position
    /// newer than the last one taken in) also becomes the track's latest report.
    pub fn apply_measured(&mut self, tr: &mut Track, m: &Measured, new_position: bool, now_ms: f64) {
        tr.meas = Some(m.clone());
        tr.meas_ms = js_max(tr.meas_ms, m.seen_ms);
        let prev_phase = tr.phase;
        let old_qnh = tr.qnh;
        self.measured_qnh(tr, m);
        // the attitude's history, so that it is shown at the drawn time
        if !m.on_ground {
            if let Some(roll) = finite(m.roll) {
                push_series(&mut tr.roll_ts, &mut tr.rolls, m.seen_ms, clamp(roll, -MAX_BANK, MAX_BANK));
            }
            let track = finite(m.track).unwrap_or(tr.track_deg);
            if let Some(heading) = finite(m.true_heading) {
                push_series(&mut tr.crab_ts, &mut tr.crabs, m.seen_ms, clamp(wrap180(heading - track), -MAX_CRAB, MAX_CRAB));
            }
        }
        if new_position
            && let (Some(t), Some(lat), Some(lon)) = (m.position_ms, m.lat, m.lon)
        {
            tr.meas_pos_ms = t;
            let inserted = merge_measured(tr, t, lat, lon, if m.mlat { SRC_MLAT } else { SRC_ADSB });
            if inserted && t > tr.report_ms {
                let dt_report = (t - tr.report_ms) / 1000.0;
                let prev_speed = tr.speed_kt;
                tr.report_ms = t;
                tr.lat0 = clamp(lat, -90.0, 90.0);
                tr.lon0 = wrap_lon(lon);
                self.select_field(tr, None);
                if !same_airport(tr.field.as_ref(), tr.qnh_own_field.as_ref()) {
                    tr.qnh_own = f64::NAN;
                }
                tr.qnh = self.track_qnh(tr, t);
                tr.on_ground = m.on_ground;
                tr.raw_w = if tr.field.is_some() { field_fade(tr.field_km) } else { 0.0 };
                tr.raw_alt_ms = t;
                // Flightradar24 reports 0 ft on the ground too
                if tr.on_ground {
                    tr.alt = 0.0;
                    tr.raw_alt = 0.0;
                    tr.ground_ms = t;
                } else if let Some(alt_baro) = finite(m.alt_baro) {
                    tr.raw_alt = alt_baro;
                    tr.alt = corrected(tr, tr.raw_alt, tr.raw_w);
                    let (raw_alt, raw_w) = (tr.raw_alt, tr.raw_w);
                    add_altitude(tr, t, raw_alt, raw_w);
                }
                if let Some(gs) = finite(m.gs) {
                    tr.speed_kt = js_max(0.0, gs);
                }
                if let Some(track) = finite(m.track) {
                    tr.track_deg = wrap360(track);
                }
                if (MIN_ACCEL_DT_S..=MAX_ACCEL_DT_S).contains(&dt_report) {
                    let accel = (tr.speed_kt - prev_speed) / dt_report;
                    tr.report_accel = accel;
                    tr.accel = clamp(tr.accel + SMOOTHING * (accel - tr.accel), -MAX_ACCEL_KTS, MAX_ACCEL_KTS);
                }
            }
        }
        let report_ms = tr.report_ms;
        self.set_qnh(tr, report_ms, old_qnh);
        measured_rates(tr, report_ms);
        tr.phase = decide_phase(tr, Some(prev_phase));
        self.refresh(tr);
        note_arrival(tr, now_ms);
    }

    /// The on-board altimeter setting near the field is its QNH: learned for it. Returns whether
    /// it was.
    pub fn measured_qnh(&mut self, tr: &Track, m: &Measured) -> bool {
        let Some(field) = &tr.field else {
            return false;
        };
        let nav_qnh = m.nav_qnh.unwrap_or(f64::NAN);
        if !valid_qnh(nav_qnh) || m.on_ground || !(tr.alt - tr.field_elev < MEAS_QNH_MAX_AGL_FT) {
            return false;
        }
        self.learn_qnh(field, nav_qnh, m.seen_ms);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::gear::testing::*;
    use super::super::Pose;
    use super::*;
    use crate::assert_close;
    use crate::model::{LiveFlight, f32_as_js};
    use crate::trains::js_min;

    /// `measuredAt(path, t)`: a measured state at `t` for a flight following `path`, east/north
    /// metres from 45N 10E (position and values at `t`, airborne).
    fn measured_at(path: impl Fn(f64) -> (f64, f64), t: f64) -> Measured {
        let (e, n) = path((t - T0) / 1000.0);
        let (lat, lon) = offset(45.0, 10.0, e, n);
        Measured { position_ms: Some(t), seen_ms: t, lat: Some(lat), lon: Some(lon), on_ground: false, ..Measured::default() }
    }

    /// `maxStep`: the largest per-frame change of `get` between `from` and `to`, at 16 ms
    /// frames, measuring each frame (as an angle difference).
    fn max_step(m: &mut Motion, f: &LiveFlight, from: f64, to: f64, get: impl Fn(&Pose) -> f64) -> f64 {
        let mut prev = get(&m.pose(f, from));
        let mut worst = 0.0;
        let mut t = from + 16.0;
        while t <= to {
            let v = get(&m.pose(f, t));
            worst = js_max(worst, signed(v - prev).abs());
            prev = v;
            t += 16.0;
        }
        worst
    }

    fn north(speed_kt: f64) -> impl Fn(f64) -> (f64, f64) + Copy {
        move |s| (0.0, speed_kt * KT * s)
    }

    /// `noise(k) = ((k * 7919) % 21) - 10`: a few metres, the same for each k.
    fn noise(k: i64) -> f64 {
        (((k * 7919) % 21) - 10) as f64
    }

    #[test]
    fn measured_roll_drives_the_bank_smoothly() {
        let mut m = Motion::new();
        let path = north(250.0);
        let f = LiveFlight { speed: 250, alt: 10_000, ..path_flight(path, T0, &BUFFER) };
        m.pose(&f, T0 + 1_000.0);
        let t = T0 + 3_000.0;
        assert!(m.pose(&f, t).bank.abs() < 0.5);
        let state = Measured {
            gs: Some(250.0),
            track: Some(0.0),
            true_heading: Some(0.0),
            roll: Some(25.0),
            track_rate: Some(2.0),
            ..measured_at(path, t)
        };
        m.measure(1, Some(&state), t);
        // no jump on arrival, and only a gentle change per frame while it blends in
        // (a 25° step eases in at up to ~20°/s, 0.33° a frame)
        let bank = max_step(&mut m, &f, t, t + 6_000.0, |p| p.bank);
        assert!(bank < 0.4, "{bank}");
        let heading = max_step(&mut m, &f, t, t + 6_000.0, |p| p.heading);
        assert!(heading < 0.3, "{heading}");
        // banked as measured, though the Flightradar24 path (its history up to T0) runs straight
        for at in [t - 2_000.0, t] {
            let p = pose_at(&mut m, &f, at);
            assert!(p.bank > 22.0 && p.bank < 28.0, "{}", p.bank);
        }
        // past the data the path turns right, as the measured track rate says
        assert!(pose_at(&mut m, &f, T0 + 15_000.0).track > 5.0);
    }

    #[test]
    fn a_measured_true_heading_off_the_track_shows_the_crab() {
        let mut m = Motion::new();
        let path = east(300.0);
        let f = LiveFlight { speed: 300, alt: 20_000, track: 90, ..path_flight(path, T0, &BUFFER) };
        let t = T0 + 9_000.0;
        m.pose(&f, t);
        let state = Measured {
            gs: Some(300.0),
            track: Some(90.0),
            true_heading: Some(100.0),
            roll: Some(0.0),
            track_rate: Some(0.0),
            ..measured_at(path, t)
        };
        m.measure(1, Some(&state), t);
        let p = m.pose(&f, t + 8_000.0);
        assert_close!(p.heading, 100.0, 0); // the nose points into the wind
        assert_close!(p.track, 90.0, 0); // the path still follows the track
        assert_close!(p.bank, 0.0, 1);
        let truth = truth_at(&m, 1, t + 8_000.0, path, T0);
        assert!(metres(&p, &truth) < 10.0, "{}", metres(&p, &truth));
    }

    #[test]
    fn a_climb_levels_off_at_the_selected_altitude() {
        let path = north(280.0);
        let run = |nav_altitude_mcp: Option<f64>| {
            let mut m = Motion::new();
            let f = LiveFlight { speed: 280, alt: 30_000, ..path_flight(path, T0, &BUFFER) };
            let t = T0 + 8_000.0;
            m.pose(&f, t);
            let state = Measured {
                gs: Some(280.0),
                track: Some(0.0),
                alt_baro: Some(30_000.0),
                baro_rate: Some(2_000.0),
                nav_altitude_mcp,
                ..measured_at(path, t)
            };
            m.measure(1, Some(&state), t);
            (m, f, t)
        };
        let (mut free, f, t) = run(None);
        assert!(pose_at(&mut free, &f, t + 90_000.0).alt_ft > 32_300.0);

        let (mut m, f, t) = run(Some(32_000.0));
        let mut prev = pose_at(&mut m, &f, t).alt_ft;
        let mut prev_rate = 0.0;
        let mut s = t + 1_000.0;
        while s <= t + 120_000.0 {
            let alt = pose_at(&mut m, &f, s).alt_ft;
            assert!(alt <= 32_000.5, "{}: {alt}", s - t);
            assert!(alt >= prev - 0.5, "{}: {alt} after {prev}", s - t); // never sinks back
            let rate = alt - prev;
            if s > t + 8_000.0 {
                // the rate eases off, no kink
                assert!((rate - prev_rate).abs() < 6.0, "{}: {rate} after {prev_rate}", s - t);
            }
            prev = alt;
            prev_rate = rate;
            s += 1_000.0;
        }
        assert_close!(pose_at(&mut m, &f, t + 90_000.0).alt_ft, 32_000.0, 0);
        // at the selected level the attitude is level flight's, not the climb's
        assert!(pose_at(&mut m, &f, t + 90_000.0).pitch < 4.0);
    }

    #[test]
    fn a_turn_on_the_autopilot_stops_at_the_selected_heading() {
        let mut m = Motion::new();
        let path = circle(160.0, 3.0);
        let f = LiveFlight { speed: 160, alt: 10_000, ..path_flight(path, T0, &BUFFER) };
        let t = T0 + 8_000.0;
        m.pose(&f, t);
        let bank_now = (160.0 * KT * 3.0 * DEG).atan2(9.80665) / DEG;
        // selected 088 magnetic with 2° east variation: 090 true
        let state = Measured {
            gs: Some(160.0),
            track: Some(24.0),
            true_heading: Some(24.0),
            mag_heading: Some(22.0),
            roll: Some(bank_now),
            track_rate: Some(3.0),
            nav_heading: Some(88.0),
            ..measured_at(path, t)
        };
        m.measure(1, Some(&state), t);
        let bank = max_step(&mut m, &f, t, t + 60_000.0, |p| p.bank);
        assert!(bank < 0.3, "{bank}");
        let end = pose_at(&mut m, &f, t + 40_000.0);
        assert_close!(end.track, 90.0, 0);
        assert_close!(end.heading, 90.0, 0);
        assert!(end.bank.abs() < 0.5, "{}", end.bank);
        assert_close!(pose_at(&mut m, &f, t + 60_000.0).track, 90.0, 0);

        // with LNAV engaged the selected heading doesn't steer: the turn carries on
        let mut lnav = Motion::new();
        lnav.pose(&f, t);
        let state = Measured {
            gs: Some(160.0),
            track: Some(24.0),
            true_heading: Some(24.0),
            mag_heading: Some(22.0),
            track_rate: Some(3.0),
            nav_heading: Some(88.0),
            nav_modes: Some(vec!["lnav".into()]),
            ..measured_at(path, t)
        };
        lnav.measure(1, Some(&state), t);
        let track = pose_at(&mut lnav, &f, t + 40_000.0).track;
        assert!(track > 130.0, "{track}");
    }

    #[test]
    fn a_stale_measured_source_falls_back_to_the_estimates_smoothly() {
        let mut m = Motion::new();
        let path = east(300.0);
        let f = LiveFlight { speed: 300, alt: 20_000, track: 90, ..path_flight(path, T0, &BUFFER) };
        let t = T0 + 9_000.0;
        m.pose(&f, t);
        let state = Measured {
            gs: Some(300.0),
            track: Some(90.0),
            true_heading: Some(98.0),
            roll: Some(12.0),
            track_rate: Some(0.0),
            ..measured_at(path, t)
        };
        m.measure(1, Some(&state), t);
        assert_close!(pose_at(&mut m, &f, t + 10_000.0).heading, 98.0, 0);
        assert_close!(pose_at(&mut m, &f, t + 10_000.0).bank, 12.0, 0);
        // nothing more arrives: between 15 and 20 s (drawn time) the measured values ease out
        let heading = max_step(&mut m, &f, t + 10_000.0, t + 40_000.0, |p| p.heading);
        assert!(heading < 0.06, "{heading}");
        let bank = max_step(&mut m, &f, t + 10_000.0, t + 40_000.0, |p| p.bank);
        assert!(bank < 0.1, "{bank}");
        let late = pose_at(&mut m, &f, t + 25_000.0);
        assert_close!(late.heading, late.track, 3);
        assert_close!(late.bank, 0.0, 3);

        // dropping the source (deselected) eases out too
        let mut n = Motion::new();
        n.pose(&f, t);
        n.measure(1, Some(&state), t);
        n.pose(&f, t + 5_000.0);
        n.measure(1, None, t + 5_000.0);
        let heading = max_step(&mut n, &f, t + 5_000.0, t + 12_000.0, |p| p.heading);
        assert!(heading < 0.3, "{heading}");
        assert_close!(n.pose(&f, t + 12_000.0).heading, 90.0, 1);
    }

    #[test]
    fn measured_attitude_is_shown_at_the_drawn_time_not_on_arrival() {
        let mut m = Motion::new();
        let path = north(250.0);
        // no buffer: drawn ~10 s behind at first
        let f = LiveFlight { speed: 250, track: 0, alt: 10_000, ..flight() };
        m.pose(&f, T0);
        // adsb.lol every 3 s (T0, T0 + 3 s, ...), half a second old: wings level and nose on
        // track, then from T0 + 12 s a 20° bank (a slip: no turn) with the nose 5° right
        let state = |t: f64| {
            let slipping = t >= T0 + 10_000.0;
            Measured {
                gs: Some(250.0),
                track: Some(0.0),
                track_rate: Some(0.0),
                roll: Some(if slipping { 20.0 } else { 0.0 }),
                true_heading: Some(if slipping { 5.0 } else { 0.0 }),
                ..measured_at(path, t)
            }
        };
        let mut a = 0.0;
        let mut early = 0;
        let mut late_checks = 0;
        let mut t = T0;
        while t <= T0 + 40_000.0 {
            if t >= T0 + 500.0 + a * 3_000.0 {
                m.measure(1, Some(&state(T0 + a * 3_000.0)), t);
                a += 1.0;
            }
            let p = m.pose(&f, t);
            let drawn = m.drawn_time(1, t);
            let crab = signed(p.heading - p.track);
            if drawn <= T0 + 9_000.0 && t > T0 + 1_000.0 {
                // already in hand (from T0 + 12.5 s) but not drawn yet: between the T0 + 9 s and
                // T0 + 12 s samples it eases over, never before
                if t > T0 + 12_500.0 {
                    early += 1;
                }
                assert!(p.bank.abs() < 0.5, "{}: {}", t - T0, p.bank);
                assert!(crab.abs() < 0.1, "{}: {crab}", t - T0);
            }
            if drawn >= T0 + 12_000.0 {
                late_checks += 1;
                assert!((p.bank - 20.0).abs() < 1.0, "{}: {}", t - T0, p.bank);
                assert!((crab - 5.0).abs() < 0.3, "{}: {crab}", t - T0);
            }
            t += 16.0;
        }
        assert!(early > 100, "{early}");
        assert!(late_checks > 100, "{late_checks}");
    }

    #[test]
    fn interleaved_flightradar24_and_measured_samples_stay_continuous() {
        let mut m = Motion::new();
        let v = 450.0 * KT;
        let path = east(450.0);
        // Flightradar24: a report every 8 s with the 8 s before it in its buffer, arriving 12 s after
        // its time (older than the measured positions by then: only its samples are news)
        let fr24_at = |k: i64| {
            let f = path_flight(move |s| path(s + (k * 8) as f64), T0 + (k * 8_000) as f64, &BUFFER);
            LiveFlight { speed: 450, alt: 36_000, track: 90, ..f }
        };
        // adsb.lol: every 3 s, 0.5 s old on arrival, a few metres of noise and a little clock offset
        let mut report = fr24_at(0);
        let mut k = 0;
        let mut a = 0;
        let start = T0 + 12_000.0;
        let mut prev = m.pose(&report, start);
        let mut worst_step = 0.0;
        let mut worst_error = 0.0;
        let mut t = start + 16.0;
        while t <= start + 90_000.0 {
            if t >= T0 + ((k + 1) * 8_000) as f64 + 12_000.0 {
                k += 1;
                report = fr24_at(k);
            }
            if t >= start + (a * 3_000) as f64 {
                let at = start + (a * 3_000) as f64 - 500.0 + 150.0;
                let (e, n) = path((at - 150.0 - T0) / 1000.0);
                let (lat, lon) = offset(45.0, 10.0, e + noise(a), n + noise(a + 3));
                let state = Measured {
                    position_ms: Some(at),
                    seen_ms: at,
                    lat: Some(lat),
                    lon: Some(lon),
                    on_ground: false,
                    gs: Some(450.0),
                    track: Some(90.0),
                    true_heading: Some(90.0),
                    roll: Some(0.0),
                    track_rate: Some(0.0),
                    alt_baro: Some(36_000.0),
                    baro_rate: Some(0.0),
                    ..Measured::default()
                };
                m.measure(1, Some(&state), t);
                a += 1;
            }
            let p = m.pose(&report, t);
            let step = metres(&prev, &p);
            // moving forward each frame, at about the right speed (the drawn time runs at 90-110%
            // while the delay eases)
            assert!(p.lon > prev.lon, "{}", t - T0);
            worst_step = js_max(worst_step, (step - v * 0.016).abs());
            worst_error = js_max(worst_error, metres(&p, &truth_at(&m, 1, t, path, T0)));
            prev = p;
            t += 16.0;
        }
        assert!(worst_step < 1.5, "{worst_step}");
        assert!(worst_error < 80.0, "{worst_error}");
    }

    #[test]
    fn a_new_sample_causes_no_step_in_position_or_velocity() {
        // Flightradar24 and adsb.lol together, the sources a few metres and a little clock apart
        let mut m = Motion::new();
        let v = 300.0 * KT;
        let path = east(300.0);
        let fr24_at = |k: i64| {
            let f = path_flight(move |s| path(s + (k * 8) as f64), T0 + (k * 8_000) as f64, &BUFFER);
            LiveFlight { speed: 300, alt: 30_000, track: 90, ..f }
        };
        let mut report = fr24_at(0);
        let mut k = 0;
        let mut a = 0;
        let start = T0 + 6_000.0;
        let to_e = |p: &Pose| (p.lon - 10.0) * M_PER_DEG * (45.0 * DEG).cos();
        let to_n = |p: &Pose| (p.lat - 45.0) * M_PER_DEG;
        let mut prev = m.pose(&report, start);
        let mut prev_ve = f64::NAN;
        let mut prev_vn = f64::NAN;
        let mut worst_position = 0.0;
        let mut worst_velocity = 0.0;
        let mut arrivals = 0;
        let mut t = start + 16.0;
        while t <= start + 60_000.0 {
            let mut arrived = false;
            if t >= T0 + ((k + 1) * 8_000) as f64 + 6_000.0 {
                k += 1;
                report = fr24_at(k);
                arrived = true;
            }
            if t >= start + (a * 3_000) as f64 {
                let at = start + (a * 3_000) as f64 - 800.0 + 100.0;
                let (e, n) = path((at - 100.0 - T0) / 1000.0);
                let (lat, lon) = offset(45.0, 10.0, e + noise(a), n + noise(a + 5));
                let state = Measured {
                    position_ms: Some(at),
                    seen_ms: at,
                    lat: Some(lat),
                    lon: Some(lon),
                    on_ground: false,
                    gs: Some(300.0),
                    track: Some(90.0),
                    alt_baro: Some(30_000.0),
                    ..Measured::default()
                };
                m.measure(1, Some(&state), t);
                a += 1;
                arrived = true;
            }
            let p = m.pose(&report, t);
            let ve = (to_e(&p) - to_e(&prev)) / 0.016;
            let vn = (to_n(&p) - to_n(&prev)) / 0.016;
            // the step from the frame before: the speed's worth, give or take the playback rate
            worst_position = js_max(worst_position, (ve.hypot(vn) * 0.016 - v * 0.016).abs());
            if !prev_ve.is_nan() {
                let dv = (ve - prev_ve).hypot(vn - prev_vn);
                worst_velocity = js_max(worst_velocity, dv);
                if arrived {
                    arrivals += 1;
                }
            }
            prev_ve = ve;
            prev_vn = vn;
            prev = p;
            t += 16.0;
        }
        assert!(arrivals > 20, "{arrivals}");
        // within 10% of the speed per frame (the drawn time runs at 90-110% while the delay eases)
        assert!(worst_position < v * 0.016 * 0.12, "{worst_position}");
        // velocity changes by under 0.1 m/s a frame (6 m/s²), arrivals included; drawn through
        // each sample at "now", the same data gave 16 m/s in a frame (~1000 m/s²)
        assert!(worst_velocity < 0.1, "{worst_velocity}");
    }

    #[test]
    fn the_drawn_time_stays_about_one_sample_spacing_or_more_behind_the_newest_data() {
        // Flightradar24 alone, a report every 8 s holding the 10 s before it, arriving 3 s late:
        // 8 + 2 + 3 = 13 s, so the 12 s cap; and with adsb.lol's positions every 3 s, half a
        // second old, as for the selected aircraft: ~3 + 2 + 0.5 = 5-6 s
        for measured in [false, true] {
            let mut m = Motion::new();
            let path = east(250.0);
            let report = |k: i64| {
                let f = path_flight(move |s| path(s + (k * 8) as f64), T0 + (k * 8_000) as f64, &[2.0, 4.0, 6.0, 8.0, 10.0]);
                LiveFlight { speed: 250, track: 90, alt: 30_000, ..f }
            };
            let mut f = report(0);
            let mut k = 0;
            let mut a = 0;
            let mut newest = f.timestamp_ms as f64;
            let mut min_behind = f64::INFINITY;
            let mut max_behind = 0.0;
            let mut delay = f64::NAN;
            let mut t = T0 + 3_000.0;
            while t <= T0 + 180_000.0 {
                if t >= T0 + ((k + 1) * 8_000) as f64 + 3_000.0 {
                    k += 1;
                    f = report(k);
                    newest = js_max(newest, f.timestamp_ms as f64);
                }
                if measured && t >= T0 + 3_000.0 + (a * 3_000) as f64 {
                    let at = T0 + 2_500.0 + (a * 3_000) as f64;
                    a += 1;
                    let (e, n) = path((at - T0) / 1000.0);
                    let (lat, lon) = offset(45.0, 10.0, e, n);
                    let state = Measured {
                        position_ms: Some(at),
                        seen_ms: at,
                        lat: Some(lat),
                        lon: Some(lon),
                        on_ground: false,
                        gs: Some(250.0),
                        track: Some(90.0),
                        alt_baro: Some(30_000.0),
                        ..Measured::default()
                    };
                    m.measure(1, Some(&state), t);
                    newest = js_max(newest, at);
                }
                m.pose(&f, t);
                let drawn = m.drawn_time(1, t);
                if t > T0 + 120_000.0 {
                    min_behind = js_min(min_behind, newest - drawn);
                    max_behind = js_max(max_behind, newest - drawn);
                    delay = t - drawn;
                }
                t += 16.0;
            }
            // within the data, never past it, and no further behind than one cadence and a spacing
            assert!(min_behind > if measured { 500.0 } else { 0.0 }, "{measured}: {min_behind}");
            if measured {
                assert!(delay > 4_500.0 && delay < 6_500.0, "{delay}");
                assert!(max_behind < 3_000.0 + 3_000.0, "{max_behind}");
            } else {
                // toBeCloseTo(12_000, -2): within 50 ms
                assert!((delay - 12_000.0).abs() < 50.0, "{delay}");
                assert!(max_behind < 8_000.0 + 2_000.0, "{max_behind}");
            }
        }
    }

    #[test]
    fn the_selected_aircrafts_altimeter_setting_adsb_lol_nav_qnh_corrects_it_and_its_field() {
        let mut m = Motion::new();
        m.set_airports(&[fld(), xxx()]);
        // 1000 hPa: pressure altitude reads ~365 ft high
        let reports = arrival(365.0, 5.0, 12.0, 1, f64::INFINITY, FIELD_FT);
        let f = &reports[10]; // 50 s in, ~1300 ft above the field
        let t9 = reports[9].timestamp_ms as f64;
        let t = f.timestamp_ms as f64;
        m.pose(&reports[9], t9 + 3_000.0);
        m.pose(f, t + 3_000.0);
        let state = Measured {
            position_ms: Some(t),
            seen_ms: t,
            lat: Some(f32_as_js(f.lat)),
            lon: Some(f32_as_js(f.lon)),
            on_ground: false,
            alt_baro: Some(f.alt as f64),
            nav_qnh: Some(1000.0),
            ..Measured::default()
        };
        m.measure(1, Some(&state), t + 3_000.0);
        assert_close!(m.field_qnh(&fld(), t), 1000.0, 3);
        // eased over to the true height
        let truth = ((120.0 - 50.0) * 140.0 * KT * (3.0 * DEG).tan()) / 0.3048;
        let p = pose_at(&mut m, f, t + 1_000.0);
        assert!((p.alt_ft - truth).abs() < 150.0, "{} against {truth}", p.alt_ft);
        let q = pose_at(&mut m, f, t + 8_000.0);
        let later = truth - 8.0 * 140.0 * KT * (3.0 * DEG).tan() / 0.3048;
        assert!((q.alt_ft - later).abs() < 60.0, "{} against {later}", q.alt_ft);
    }

    #[test]
    fn a_measured_crab_is_kicked_out_in_the_flare_and_gone_on_the_runway() {
        let l = landings();
        let f = fixture(&l, "KLM76F");
        let mut m = Motion::new();
        m.set_airports(&fixture_airports(&l));
        let reports = fixture_reports(f);
        let mut k: i64 = -1;
        let mut crabs: Vec<(f64, f64)> = Vec::new();
        let end = reports[reports.len() - 1].timestamp_ms as f64 + 13_000.0;
        let mut now = reports[0].timestamp_ms as f64 + 3_000.0;
        while now <= end {
            while ((k + 1) as usize) < reports.len() && reports[(k + 1) as usize].timestamp_ms as f64 + 3_000.0 <= now {
                k += 1;
                let r = &reports[k as usize];
                // a crosswind from the right all the way down, as adsb.lol reports it
                let state = Measured {
                    position_ms: Some(r.timestamp_ms as f64),
                    seen_ms: r.timestamp_ms as f64,
                    lat: Some(f32_as_js(r.lat)),
                    lon: Some(f32_as_js(r.lon)),
                    on_ground: r.on_ground,
                    gs: Some(r.speed as f64),
                    track: Some(r.track as f64),
                    true_heading: Some(r.track as f64 + 6.0),
                    alt_baro: Some(r.alt as f64),
                    ..Measured::default()
                };
                m.measure(f.id, Some(&state), now);
            }
            let p = m.pose(&reports[k.max(0) as usize], now);
            // (the first measurement comes before the track exists and is dropped)
            if k >= 2 {
                crabs.push((p.alt_ft, signed(p.heading - p.track)));
            }
            now += 16.0;
        }
        let high: Vec<_> = crabs.iter().filter(|c| c.0 > 300.0).collect();
        assert!(!high.is_empty());
        for (alt, crab) in high {
            assert!((crab - 6.0).abs() < 1.0, "{alt} ft: {crab}");
        }
        let down: Vec<_> = crabs.iter().filter(|c| c.0 < 1.0).collect();
        assert!(down.len() > 100, "{}", down.len());
        for (alt, crab) in down {
            assert!(crab.abs() < 0.01, "{alt} ft: {crab}");
        }
    }

    #[test]
    fn measure_takes_only_news_and_a_drop_only_when_there_is_something_to_drop() {
        let mut m = Motion::new();
        let path = east(300.0);
        let f = LiveFlight { speed: 300, alt: 20_000, track: 90, ..path_flight(path, T0, &BUFFER) };
        let state = Measured { gs: Some(300.0), roll: Some(10.0), ..measured_at(path, T0 + 1_000.0) };
        // before the first report: nothing
        m.measure(1, Some(&state), T0 + 2_000.0);
        assert!(m.tracks.is_empty());
        m.pose(&f, T0 + 2_000.0);
        m.measure(1, Some(&state), T0 + 2_000.0);
        let tr = &m.tracks[&1];
        assert_eq!((tr.meas_ms, tr.meas_pos_ms, tr.report_ms), (T0 + 1_000.0, T0 + 1_000.0, T0 + 1_000.0));
        assert_eq!((tr.roll_ts.clone(), tr.rolls.clone()), (vec![T0 + 1_000.0], vec![10.0]));
        assert_eq!(tr.touched, T0 + 2_000.0);
        // the same again is no news (not even touched); a newer seen time without a position is
        m.measure(1, Some(&state), T0 + 3_000.0);
        assert_eq!(m.tracks[&1].touched, T0 + 2_000.0);
        let later = Measured { position_ms: None, lat: None, lon: None, seen_ms: T0 + 1_500.0, roll: Some(40.0), ..state.clone() };
        m.measure(1, Some(&later), T0 + 3_000.0);
        let tr = &m.tracks[&1];
        assert_eq!((tr.meas_ms, tr.meas_pos_ms, tr.report_ms), (T0 + 1_500.0, T0 + 1_000.0, T0 + 1_000.0));
        // (the roll clamped to 30°)
        assert_eq!(tr.rolls, vec![10.0, 30.0]);
        // dropped: every measured value goes
        m.measure(1, None, T0 + 4_000.0);
        let tr = &m.tracks[&1];
        assert!(tr.meas.is_none() && tr.meas_ms == f64::NEG_INFINITY && tr.roll_ts.is_empty() && tr.rolls.is_empty());
        assert!(tr.roll_biases.is_empty() && tr.crab_ts.is_empty() && tr.crabs.is_empty());
        assert!(tr.nav_track.is_nan() && tr.crab == 0.0 && tr.tas_ms == 0.0);
        // and dropping again does nothing (no blend restarted)
        let off_ms = tr.off_ms;
        m.measure(1, None, T0 + 5_000.0);
        assert_eq!(m.tracks[&1].off_ms, off_ms);
    }

    #[test]
    fn the_altimeter_setting_teaches_the_field_only_near_it_and_in_the_air() {
        let field = airport_at("FLD", 45.0, 10.0, 1_000);
        let mut w = World::default();
        let mut tr = Track::new(1);
        let state = Measured { nav_qnh: Some(1000.0), seen_ms: T0, ..Measured::default() };
        // no field
        assert!(!w.measured_qnh(&tr, &state));
        tr.field = Some(field.clone());
        tr.field_elev = 1_000.0;
        tr.alt = 3_999.0;
        assert!(w.measured_qnh(&tr, &state));
        assert_eq!(w.field_qnh(&field, T0), 1000.0);
        // 3000 ft or more above the field, on the ground, or an implausible setting: not learned
        tr.alt = 4_000.0;
        assert!(!w.measured_qnh(&tr, &state));
        tr.alt = 1_000.0;
        assert!(!w.measured_qnh(&tr, &Measured { on_ground: true, ..state.clone() }));
        assert!(!w.measured_qnh(&tr, &Measured { nav_qnh: Some(900.0), ..state.clone() }));
        assert!(!w.measured_qnh(&tr, &Measured { nav_qnh: None, ..state.clone() }));
        assert_eq!(w.qnh_version, 1);
    }
}
