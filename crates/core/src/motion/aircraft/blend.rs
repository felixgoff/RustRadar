//! Blending every change in (`motion.ts` `drawn`, `setOffsets`, `shownState`, `takeOver`,
//! `blendAround`, `ingest`, `:1261-1397`; doc 05 §10.11), and the part of `pose` (`:1079-1121`)
//! up to the taxi snap and the gear.
//!
//! Every change (a report, measured data, a state taking over) is wrapped: what is on screen is
//! evaluated before, the change applied, the model evaluated after at the same drawn time, and
//! the difference blended out matching position and velocity (C1). Large position and height
//! differences go to a fast blend and small ones (the smoothed curve moving as samples come in)
//! to a gentle one, whose acceleration stays small; a new altitude reference (a QNH learned)
//! moves the whole vertical profile and is eased in more slowly. More than 5 km, or a previous
//! report older than two minutes, snaps instead. The angles blend out from their offset at the
//! full rate.

use super::consts::*;
use super::delay::{delay_slope, draw_time, state_index};
use super::evaluate::evaluate;
use super::track::{Raw, Track, blend};
use super::{Motion, World, smoothstep, wrap_lon, wrap180, wrap360};
use crate::model::LiveFlight;
use crate::trains::{js_max, js_min};

/// The model at drawn time `t` as it moves in real time at `now_ms`: its velocities scaled by the
/// playback rate (the delay may be easing), and with the correction being blended out if
/// `with_offsets`. `state` indexes `tr.states`.
pub fn drawn(tr: &mut Track, now_ms: f64, t: f64, state: usize, with_offsets: bool) -> Raw {
    let mut out = evaluate(tr, t, Some(&tr.states[state]));
    let rate = 1.0 - delay_slope(tr, now_ms);
    out.v_e *= rate;
    out.v_n *= rate;
    out.vs_fpm *= rate;
    if !with_offsets {
        return out;
    }
    // position and height: the corrections being blended out (C1)
    let Track { fast, gentle, alt_fast, alt_gentle, .. } = tr;
    fast.at(now_ms);
    gentle.at(now_ms);
    let d_e = fast.px + gentle.px;
    let d_n = fast.py + gentle.py;
    if d_e != 0.0 || d_n != 0.0 {
        out.lat += d_n / M_PER_DEG;
        out.lon = wrap_lon(out.lon + d_e / (M_PER_DEG * js_max((out.lat * DEG).cos(), 1e-3)));
        out.v_e += fast.pvx + gentle.pvx;
        out.v_n += fast.pvy + gentle.pvy;
    }
    alt_fast.at(now_ms);
    alt_gentle.at(now_ms);
    let d_a = alt_fast.px + alt_gentle.px;
    if d_a != 0.0 {
        out.draw_alt = js_max(0.0, out.draw_alt + d_a);
        out.vs_fpm += (alt_fast.pvx + alt_gentle.pvx) * 60.0;
    }
    let age = (now_ms - tr.off_ms) / 1000.0;
    // the angles: from the offset with zero rate, at the full rate as before
    let w = blend(age, BLEND_RATE);
    if w == 0.0 {
        return out;
    }
    out.heading = wrap360(out.heading + tr.off_hdg * w);
    out.track = wrap360(out.track + tr.off_trk * w);
    out.pitch += tr.off_pitch * w;
    out.bank += tr.off_bank * w;
    out
}

/// Starts blending out the difference between what was drawn (`before`) and the model now
/// (`after`); snaps instead when they are far apart or `before` came from data older than
/// SNAP_AGE_MS (`snap_before_ms`).
pub fn set_offsets(tr: &mut Track, now_ms: f64, before: &Raw, after: &Raw, snap_before_ms: f64) {
    let cos_lat = js_max((after.lat * DEG).cos(), 1e-3);
    let off_e = wrap180(before.lon - after.lon) * M_PER_DEG * cos_lat;
    let off_n = (before.lat - after.lat) * M_PER_DEG;
    let Track { fast, gentle, alt_fast, alt_gentle, .. } = tr;
    if off_e.hypot(off_n) > SNAP_DISTANCE_M || now_ms - snap_before_ms > SNAP_AGE_MS {
        fast.clear();
        gentle.clear();
        alt_fast.clear();
        alt_gentle.clear();
        tr.off_ms = f64::NEG_INFINITY;
        tr.alt_blend_rate = f64::NAN;
        return;
    }
    // what changed now, beyond the corrections already under way (which carry on as they were),
    // goes to the fast blend if large and the gentle one if small
    fast.at(now_ms);
    gentle.at(now_ms);
    let d_e = off_e - fast.px - gentle.px;
    let d_n = off_n - fast.py - gentle.py;
    let d_ve = before.v_e - after.v_e - fast.pvx - gentle.pvx;
    let d_vn = before.v_n - after.v_n - fast.pvy - gentle.pvy;
    let w = smoothstep((d_e.hypot(d_n) - GENTLE_MAX_M) / GENTLE_MAX_M);
    fast.add(now_ms, w * d_e, w * d_n, w * d_ve, w * d_vn, Some(BLEND_RATE));
    gentle.add(now_ms, (1.0 - w) * d_e, (1.0 - w) * d_n, (1.0 - w) * d_ve, (1.0 - w) * d_vn, None);
    alt_fast.at(now_ms);
    alt_gentle.at(now_ms);
    let d_a = before.draw_alt - after.draw_alt - alt_fast.px - alt_gentle.px;
    let d_va = (before.vs_fpm - after.vs_fpm) / 60.0 - alt_fast.pvx - alt_gentle.pvx;
    let wa = smoothstep((d_a.abs() * FEET_TO_M - GENTLE_MAX_M) / GENTLE_MAX_M);
    // a new altitude reference (a QNH learned) moves the whole vertical profile: eased in, not snapped
    // (and one still under way carries on at its own pace, sped up no more than that allows;
    // a critically damped blend of x from rest moves at most k |x| / e)
    let capped = (REFERENCE_MAX_FT_S * std::f64::consts::E) / js_max(1.0, (alt_fast.px + wa * d_a).abs());
    let alt_rate = if tr.alt_blend_rate.is_finite() {
        js_min(tr.alt_blend_rate, capped)
    } else if alt_fast.px != 0.0 && alt_fast.k < BLEND_RATE {
        js_min(BLEND_RATE, js_max(alt_fast.k, capped))
    } else {
        BLEND_RATE
    };
    tr.alt_blend_rate = f64::NAN;
    alt_fast.add(now_ms, wa * d_a, 0.0, wa * d_va, 0.0, Some(alt_rate));
    alt_gentle.add(now_ms, (1.0 - wa) * d_a, 0.0, (1.0 - wa) * d_va, 0.0, None);
    // the angles, from the offset with zero rate as before
    tr.off_ms = now_ms;
    tr.off_hdg = wrap180(before.heading - after.heading);
    tr.off_trk = wrap180(before.track - after.track);
    tr.off_pitch = before.pitch - after.pitch;
    tr.off_bank = before.bank - after.bank;
}

/// The state drawn last frame, if the previous pose was a moment ago; else the one in effect at
/// `t` (an index into `tr.states`).
pub fn shown_state(tr: &Track, now_ms: f64, t: f64) -> usize {
    if (now_ms - tr.last_pose_ms).abs() <= CONTINUITY_GAP_MS
        && let Some(i) = tr.states.iter().position(|s| s.report_ms == tr.active_ms)
    {
        return i;
    }
    state_index(tr, t)
}

/// The drawn time has reached a newer (or, going back, older) report's state (`state`, an index
/// into `tr.states`): blend over to it.
pub fn take_over(tr: &mut Track, now_ms: f64, t: f64, state: usize) {
    if (now_ms - tr.last_pose_ms).abs() <= CONTINUITY_GAP_MS
        && let Some(prev) = tr.states.iter().position(|s| s.report_ms == tr.active_ms)
    {
        let before = drawn(tr, now_ms, t, prev, true);
        let after = drawn(tr, now_ms, t, state, false);
        set_offsets(tr, now_ms, &before, &after, now_ms);
    }
    tr.active_ms = tr.states[state].report_ms;
}

impl World {
    /// Applies `change` to an existing track, blending from the pose drawn before it to the one
    /// after.
    pub fn blend_around<F: FnOnce(&mut World, &mut Track)>(&mut self, tr: &mut Track, now_ms: f64, change: F) {
        let previous_ms = tr.report_ms;
        // what is on screen right now, correction included
        let t = draw_time(tr, now_ms);
        let shown = shown_state(tr, now_ms, t);
        let before = drawn(tr, now_ms, t, shown, true);

        change(self, tr);

        // the delay eases from where it was, so the drawn time is still t
        let state = state_index(tr, t);
        let after = drawn(tr, now_ms, t, state, false);
        tr.active_ms = tr.states[state].report_ms;
        set_offsets(tr, now_ms, &before, &after, previous_ms);
    }

    /// Takes a newer report into the track and blends from the old drawn pose to the new one.
    pub fn ingest(&mut self, tr: &mut Track, f: &LiveFlight, now_ms: f64) {
        let first = tr.n == 0;
        let t = f.timestamp_ms as f64;
        if first {
            self.update(tr, f, t, true, now_ms);
        } else {
            self.blend_around(tr, now_ms, |w, tr| w.update(tr, f, t, false, now_ms));
        }
    }
}

impl Motion {
    /// Everything `Motion::pose` does before the taxi snap and the gear (S1.20 adds those): the
    /// aircraft's track is created or found and touched, `f` ingested when the track is empty or
    /// `f` is newer than its last Flightradar24 report (at most INGEST_BUDGET reports a frame, one
    /// frame being one `now_ms`), stale tracks pruned once a minute, and the model drawn at the
    /// drawn time, blending over to the report state in effect there. Returns the drawn,
    /// un-clamped result and the drawn time.
    pub fn pose_raw(&mut self, f: &LiveFlight, now_ms: f64) -> (Raw, f64) {
        let Motion { tracks, world } = self;
        let tr = tracks.entry(f.id).or_insert_with(|| Track::new(f.id));
        tr.touched = now_ms;
        if tr.n == 0 {
            world.ingest(tr, f, now_ms);
        } else if f.timestamp_ms as f64 > tr.fr24_ms {
            // a snapshot brings every aircraft's report at once: take in at most so many a frame
            // (one frame being one `now_ms`); the rest are drawn as they were for a frame or two,
            // which changes nothing on screen, since the drawn time is seconds behind anyway
            if now_ms != world.budget_ms {
                world.budget_ms = now_ms;
                world.budget = INGEST_BUDGET;
            }
            if world.budget > 0 {
                world.budget -= 1;
                world.ingest(tr, f, now_ms);
            }
        }
        if now_ms - self.world.last_prune > PRUNE_INTERVAL_MS {
            self.prune(now_ms);
        }
        // (just touched: pruning keeps it)
        let tr = self.tracks.get_mut(&f.id).expect("the track posed is kept");

        // drawn a few seconds in the past, by design (see the top of `mod.rs`)
        let t = draw_time(tr, now_ms);
        let state = state_index(tr, t);
        if tr.states[state].report_ms != tr.active_ms {
            take_over(tr, now_ms, t, state);
        }
        tr.last_pose_ms = now_ms;
        (drawn(tr, now_ms, t, state, true), t)
    }
}

/// What the ported `motion.test.ts` cases share (its helpers, `T0`, `flight`, `pathFlight`,
/// `poseAt`, `play`, the recorded flights), and [`testing::pose`]: the TypeScript's `pose` without
/// the gear (S1.20), for cases that neither read the gear nor load taxiways.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Arc;

    use super::super::consts::*;
    use super::super::{Motion, Pose, clamp};
    use crate::model::{Airport, LiveFlight, f32_as_js};
    use crate::num::js_round;

    pub const T0: f64 = 1_760_000_000_000.0;
    pub const KT: f64 = 0.514444;

    /// `m.pose(f, now)` with the gear left NaN: S1.20 brings the gear and the taxi snap, which
    /// these cases do not use (no taxiway network is loaded).
    pub fn pose(m: &mut Motion, f: &LiveFlight, now_ms: f64) -> Pose {
        assert!(m.world.taxiways.is_empty(), "the taxi snap is S1.20's");
        let (raw, _) = m.pose_raw(f, now_ms);
        Pose {
            lon: raw.lon,
            lat: raw.lat,
            alt_ft: raw.draw_alt,
            heading: raw.heading,
            track: raw.track,
            pitch: clamp(raw.pitch, MIN_PITCH, MAX_PITCH),
            bank: clamp(raw.bank, -MAX_BANK, MAX_BANK),
            gear: f64::NAN,
            phase: raw.phase,
        }
    }

    /// A point `east`/`north` metres from lat/lon (flat earth, as the model uses).
    pub fn offset(lat: f64, lon: f64, east: f64, north: f64) -> (f64, f64) {
        (lat + north / M_PER_DEG, lon + east / (M_PER_DEG * (lat * DEG).cos()))
    }

    /// Something with a position.
    pub trait LatLon {
        fn lat_lon(&self) -> (f64, f64);
    }

    impl LatLon for Pose {
        fn lat_lon(&self) -> (f64, f64) {
            (self.lat, self.lon)
        }
    }

    impl LatLon for (f64, f64) {
        fn lat_lon(&self) -> (f64, f64) {
            *self
        }
    }

    /// Metres between two poses or points.
    pub fn metres(a: &impl LatLon, b: &impl LatLon) -> f64 {
        let (alat, alon) = a.lat_lon();
        let (blat, blon) = b.lat_lon();
        let x = (blon - alon) * M_PER_DEG * (((alat + blat) / 2.0) * DEG).cos();
        x.hypot((blat - alat) * M_PER_DEG)
    }

    /// `flight({})`: a report at 45N 10E at T0, the TypeScript's defaults. Latitude and longitude
    /// are `f32` in `LiveFlight` (the TypeScript tests' doubles are rounded to them), track,
    /// altitude and speed whole numbers.
    pub fn flight() -> LiveFlight {
        LiveFlight {
            id: 1,
            lat: 45.0,
            lon: 10.0,
            track: 0,
            alt: 35_000,
            speed: 450,
            on_ground: false,
            timestamp_ms: T0 as u64,
            callsign: "TEST1".into(),
            flight: "TE1".into(),
            reg: "N1".into(),
            typecode: "A320".into(),
            origin: String::new(),
            destination: String::new(),
            icon: "A320".into(),
            positions: Vec::new(),
            vspeed: None,
        }
    }

    /// `flight` at a position (degrees, rounded to `f32`).
    pub fn flight_at(lat: f64, lon: f64) -> LiveFlight {
        LiveFlight { lat: lat as f32, lon: lon as f32, ..flight() }
    }

    /// A latitude or longitude as the model reads it from a `LiveFlight` (its `f32`).
    pub fn seen(deg: f64) -> f64 {
        f32_as_js(deg as f32)
    }

    /// A report at a position (`flight_at`) whose buffer holds where it was `ds` seconds before
    /// (`past(d)`), relative to the position the model reads, so the buffer's points are those
    /// of the TypeScript's tests, to Flightradar24's 1e-5 degrees.
    pub fn flight_with_history(lat: f64, lon: f64, past: impl Fn(f64) -> (f64, f64), ds: &[f64]) -> LiveFlight {
        let positions = history(seen(lat), seen(lon), past, ds);
        LiveFlight { positions, ..flight_at(lat, lon) }
    }

    /// Flightradar24's position buffer for a report at `lat`/`lon`: the aircraft's recent
    /// history, each point `d` seconds before the report (newest first) at
    /// `lat/lon - [dLat, dLon]·1e-5`. `past(d)` is where it was then.
    pub fn history(lat: f64, lon: f64, past: impl Fn(f64) -> (f64, f64), ds: &[f64]) -> Vec<[i32; 3]> {
        ds.iter()
            .map(|&d| {
                let (la, lo) = past(d);
                [js_round((lat - la) / 1e-5) as i32, js_round((lon - lo) / 1e-5) as i32, (d * 1000.0) as i32]
            })
            .collect()
    }

    pub const BUFFER: [f64; 4] = [2.0, 4.0, 6.0, 8.0];

    /// A report at time `t0` on `path(seconds from t0) -> (east, north)` metres from 45N 10E, with
    /// the buffer of where it was `buffer_s` seconds before (`pathFlight`).
    pub fn path_flight(path: impl Fn(f64) -> (f64, f64), t0: f64, buffer_s: &[f64]) -> LiveFlight {
        let (e, n) = path(0.0);
        let (lat, lon) = offset(45.0, 10.0, e, n);
        let past = |d: f64| {
            let (e, n) = path(-d);
            offset(45.0, 10.0, e, n)
        };
        LiveFlight { timestamp_ms: t0 as u64, ..flight_with_history(lat, lon, past, buffer_s) }
    }

    /// Right turn at `rate_deg` deg/s (negative: left) from heading north at `speed_kt`.
    pub fn circle(speed_kt: f64, rate_deg: f64) -> impl Fn(f64) -> (f64, f64) + Copy {
        let v = speed_kt * KT;
        let w = rate_deg * DEG;
        let r = v / w;
        move |s| (r * (1.0 - (w * s).cos()), r * (w * s).sin())
    }

    pub fn east(speed_kt: f64) -> impl Fn(f64) -> (f64, f64) + Copy {
        move |s| (speed_kt * KT * s, 0.0)
    }

    /// The pose `f` is drawn with at the moment its drawn time is `t`: the delay changes slowly,
    /// so a few fixed-point steps find that moment. `f` must have been taken in already.
    pub fn pose_at(m: &mut Motion, f: &LiveFlight, t: f64) -> Pose {
        let mut now = t;
        for _ in 0..8 {
            now += t - m.drawn_time(f.id, now);
        }
        pose(m, f, now)
    }

    /// Truth `path(seconds)` at the drawn time of `id` at `now`, as a point.
    pub fn truth_at(m: &Motion, id: u32, now: f64, path: impl Fn(f64) -> (f64, f64), t0: f64) -> (f64, f64) {
        let (e, n) = path((m.drawn_time(id, now) - t0) / 1000.0);
        offset(45.0, 10.0, e, n)
    }

    /// `airport({})`: "TST" at 45N 10E at sea level.
    pub fn airport() -> Airport {
        Airport {
            id: 1,
            name: "Test".into(),
            iata: "TST".into(),
            icao: "XTST".into(),
            city: String::new(),
            country: String::new(),
            lat: 45.0,
            lon: 10.0,
            alt: Some(0),
            size: 0,
            timezone: "UTC".into(),
        }
    }

    pub fn airport_at(iata: &str, lat: f64, lon: f64, alt: i64) -> Arc<Airport> {
        Arc::new(Airport { iata: iata.into(), lat, lon, alt: Some(alt), ..airport() })
    }

    /// The signed difference of two angles, degrees (`((a - b + 540) % 360) - 180`).
    pub fn signed(deg: f64) -> f64 {
        ((deg + 540.0) % 360.0) - 180.0
    }

    /// What the drawn aircraft did, frame by frame (16 ms), while reports arrived `latency` ms
    /// after their time (`play`); the gear is S1.20's and the phase no case here reads, so
    /// neither is recorded.
    #[derive(Debug, Clone, Copy)]
    pub struct Frame {
        pub drawn: f64,
        pub alt_ft: f64,
    }

    pub fn play(m: &mut Motion, reports: &[LiveFlight], latency: f64, after: f64) -> Vec<Frame> {
        let mut frames = Vec::new();
        let mut k = 0;
        let end = reports[reports.len() - 1].timestamp_ms as f64 + latency + after;
        let mut now = reports[0].timestamp_ms as f64 + latency;
        while now <= end {
            while k + 1 < reports.len() && reports[k + 1].timestamp_ms as f64 + latency <= now {
                k += 1;
            }
            let p = pose(m, &reports[k], now);
            frames.push(Frame { drawn: m.drawn_time(reports[k].id, now), alt_ft: p.alt_ft });
            now += 16.0;
        }
        frames
    }

    /// Largest change of the drawn height between frames, ft.
    pub fn worst_step(frames: &[Frame]) -> f64 {
        frames.windows(2).fold(0.0, |w, p| crate::trains::js_max(w, (p[1].alt_ft - p[0].alt_ft).abs()))
    }

    /// Drawn time at which the aircraft first comes within a foot of the ground after `from`
    /// (landing), NaN if never.
    pub fn first_down(frames: &[Frame], from: f64) -> f64 {
        frames.iter().find(|f| f.drawn > from && f.alt_ft < 1.0).map_or(f64::NAN, |f| f.drawn)
    }

    /// 4000 fpm at 60 fps: the most the drawn height may move in a frame.
    pub const MAX_FRAME_FT: f64 = (4_000.0 / 60.0) * 0.016;

    /// The synthetic field's elevation, ft (so that its runway reads above 0 even 400 ft low).
    pub const FIELD_FT: f64 = 1_000.0;

    /// `fld()`: a new "FLD" at 45N 10E, `FIELD_FT` up (each call a new airport, as in the
    /// TypeScript: another object than the one the model was given).
    pub fn fld() -> Arc<Airport> {
        airport_at("FLD", 45.0, 10.0, FIELD_FT as i64)
    }

    /// `airport({ id: 2, iata: "XXX", lat: 50, lon: 20 })`: the far end of the synthetic flights.
    pub fn xxx() -> Arc<Airport> {
        Arc::new(Airport { id: 2, ..(*airport_at("XXX", 50.0, 20.0, 0)).clone() })
    }

    /// A synthetic arrival at a field `elev` ft up (`arrival`): 3° down at 140 kt, touching down at
    /// T0 + 120 s, then braking at 2.5 kt/s. `alt` reads `bias_ft` above the true height
    /// (pressure altitude, in 25 ft steps), the ground flag comes `late_s` after touchdown and
    /// meanwhile the runway reads level. Reports every `every_s` s, with the 8 s before each in
    /// its buffer; none on short final after `last_air_s`. (Speeds are whole knots here.)
    pub fn arrival(bias_ft: f64, every_s: f64, late_s: f64, id: u32, last_air_s: f64, elev: f64) -> Vec<LiveFlight> {
        let td_s = 120.0;
        let v0 = 140.0 * KT;
        let north = |s: f64| {
            if s <= td_s { v0 * (s - td_s) } else { v0 * (s - td_s) - (2.5 * KT * (s - td_s) * (s - td_s)) / 2.0 }
        };
        let speed = |s: f64| if s <= td_s { 140.0 } else { crate::trains::js_max(20.0, 140.0 - 2.5 * (s - td_s)) };
        let agl = |s: f64| crate::trains::js_max(0.0, ((td_s - s) * v0 * (3.0 * DEG).tan()) / 0.3048);
        let mut reports = Vec::new();
        let mut s = 0.0;
        while s <= td_s + 40.0 {
            if !(s < td_s && s > last_air_s) {
                let (lat, lon) = offset(45.0, 10.0, 0.0, north(s));
                let on_ground = s >= td_s + late_s;
                let alt = if on_ground {
                    0.0
                } else {
                    crate::trains::js_max(0.0, js_round((elev + agl(s) + bias_ft) / 25.0) * 25.0)
                };
                let f = flight_with_history(lat, lon, |d| offset(45.0, 10.0, 0.0, north(s - d)), &BUFFER);
                reports.push(LiveFlight {
                    id,
                    alt: alt as i32,
                    speed: js_round(speed(s)) as i32,
                    track: 0,
                    on_ground,
                    timestamp_ms: (T0 + s * 1000.0) as u64,
                    destination: "FLD".into(),
                    origin: "XXX".into(),
                    ..f
                });
            }
            s += every_s;
        }
        reports
    }

    /// `landings.json`: real reports (Flightradar24, 2026-10-09).
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Landings {
        pub airports: Vec<FixtureAirport>,
        pub flights: Vec<FixtureFlight>,
    }

    #[derive(serde::Deserialize)]
    pub struct FixtureAirport {
        pub iata: String,
        pub icao: String,
        pub lat: f64,
        pub lon: f64,
        pub alt: i64,
    }

    /// `[timestampMs, lat, lon, alt, speed, track, onGround, buffer]`.
    pub type FixtureReport = (f64, f64, f64, f64, f64, f64, u8, Vec<[i32; 3]>);

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct FixtureFlight {
        pub id: u32,
        pub callsign: String,
        pub typecode: String,
        pub origin: String,
        pub destination: String,
        pub airport: String,
        pub event_ms: f64,
        pub reports: Vec<FixtureReport>,
    }

    pub fn landings() -> Landings {
        serde_json::from_str(include_str!("../../../tests/data/landings.json")).expect("landings.json")
    }

    /// `fixtureAirports`: the file's airports with ids from 1.
    pub fn fixture_airports(l: &Landings) -> Vec<Arc<Airport>> {
        l.airports
            .iter()
            .enumerate()
            .map(|(i, a)| {
                Arc::new(Airport {
                    id: i as u32 + 1,
                    iata: a.iata.clone(),
                    icao: a.icao.clone(),
                    lat: a.lat,
                    lon: a.lon,
                    alt: Some(a.alt),
                    ..airport()
                })
            })
            .collect()
    }

    /// `fixture(callsign)`: a recorded flight.
    pub fn fixture<'a>(l: &'a Landings, callsign: &str) -> &'a FixtureFlight {
        l.flights.iter().find(|f| f.callsign == callsign).expect("a recorded flight")
    }

    /// `fixtureReports`: a recorded flight's reports as `LiveFlight`s.
    pub fn fixture_reports(f: &FixtureFlight) -> Vec<LiveFlight> {
        f.reports
            .iter()
            .map(|(t, lat, lon, alt, speed, track, ground, positions)| LiveFlight {
                id: f.id,
                lat: *lat as f32,
                lon: *lon as f32,
                alt: *alt as i32,
                speed: *speed as i32,
                track: *track as i32,
                on_ground: *ground == 1,
                timestamp_ms: *t as u64,
                positions: positions.clone(),
                callsign: f.callsign.clone(),
                typecode: f.typecode.clone(),
                icon: f.typecode.clone(),
                origin: f.origin.clone(),
                destination: f.destination.clone(),
                ..flight()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::testing::*;
    use super::*;
    use crate::model::Airport;

    #[test]
    fn a_new_report_blends_in_without_a_jump_and_converges() {
        let mut m = Motion::new();
        let speed = 250;
        let v = speed as f64 * KT;
        let line = move |s: f64| (v * s, 0.0);
        let a = LiveFlight { track: 90, speed, ..path_flight(line, T0, &BUFFER) };
        let frame = 16.0;
        let mut prev = pose(&mut m, &a, T0 + 2_000.0);
        let mut t = T0 + 2_000.0;
        while t <= T0 + 12_000.0 {
            let p = pose(&mut m, &a, t);
            assert!(metres(&prev, &p) < v * 0.016 + 0.5, "{}", metres(&prev, &p));
            prev = p;
            t += frame;
        }
        // a report from 10 s, 300 m north of the line
        let off = move |s: f64| (v * (s + 10.0), 300.0);
        let b = LiveFlight { track: 90, speed, ..path_flight(off, T0 + 10_000.0, &BUFFER) };
        let now = t;
        while t <= now + 5_000.0 {
            let p = pose(&mut m, &b, t);
            // the blend adds at most ~0.8·k·300 m/s (k = 2.2/s) on top of the motion itself
            assert!(metres(&prev, &p) < (v + 250.0) * 0.016, "{}", metres(&prev, &p));
            assert!(signed(p.heading - prev.heading).abs() < 5.0);
            prev = p;
            t += frame;
        }
        // a few seconds later it is on the new data (at the drawn time)
        while t <= now + 8_000.0 {
            pose(&mut m, &b, t);
            t += frame;
        }
        let p = pose(&mut m, &b, t);
        let truth = truth_at(&m, 1, t, |s| off(s - 10.0), T0);
        assert!(metres(&p, &truth) < 2.0, "{}", metres(&p, &truth));
    }

    #[test]
    fn prune_drops_stale_tracks() {
        let mut m = Motion::new();
        let f = flight();
        pose(&mut m, &f, T0);
        m.prune(T0 + 16.0 * 60_000.0);
        // a re-created track starts from scratch: no blending from the old one
        let g = LiveFlight { lat: 46.0, timestamp_ms: (T0 + 16.0 * 60_000.0) as u64, ..flight() };
        pose(&mut m, &g, T0 + 16.0 * 60_000.0);
        crate::assert_close!(pose_at(&mut m, &g, T0 + 16.0 * 60_000.0).lat, 46.0, 6);
    }

    #[test]
    fn stale_data_never_freezes_and_fresh_data_is_caught_up_smoothly() {
        let mut m = Motion::new();
        let v = 250.0 * KT;
        // east at 250 kt, and from 30 s on drifting north at 2 m/s, which the extrapolation can't know
        let truth = move |s: f64| (v * s, if s > 30.0 { 2.0 * (s - 30.0) } else { 0.0 });
        let at = |k: i64| {
            let f = path_flight(|s| truth(s + (k * 8) as f64), T0 + (k * 8_000) as f64, &BUFFER);
            LiveFlight { speed: 250, track: 90, ..f }
        };
        let mut f = at(0);
        let mut k = 0;
        let mut prev = pose(&mut m, &f, T0 + 4_000.0);
        let mut prev_step = f64::NAN;
        let mut worst_gap = f64::INFINITY;
        let mut worst_gap_step = 0.0;
        let mut worst_resume = 0.0;
        let mut worst_step_change = 0.0;
        let mut t = T0 + 4_016.0;
        while t <= T0 + 150_000.0 {
            // reports up to 30 s, then nothing until 90 s
            let silent = t > T0 + 30_000.0 && t < T0 + 90_000.0;
            if t >= T0 + ((k + 1) * 8_000) as f64 + 4_000.0 && !silent {
                k += 1;
                f = at(k);
            }
            if silent {
                k = ((t - T0 - 4_000.0) / 8_000.0).floor() as i64;
            }
            let p = pose(&mut m, &f, t);
            let step = metres(&prev, &p);
            if t > T0 + 40_000.0 && t < T0 + 90_000.0 {
                worst_gap = js_min(worst_gap, step);
                worst_gap_step = js_max(worst_gap_step, step);
            }
            if (T0 + 90_000.0..T0 + 100_000.0).contains(&t) {
                worst_resume = js_max(worst_resume, step);
            }
            if !prev_step.is_nan() {
                worst_step_change = js_max(worst_step_change, (step - prev_step).abs());
            }
            prev_step = step;
            prev = p;
            t += 16.0;
        }
        // moving all through the gap, at its speed
        assert!(worst_gap > v * 0.016 * 0.85, "{worst_gap}");
        assert!(worst_gap_step < v * 0.016 * 1.15, "{worst_gap_step}");
        // ~120 m of drift to catch up: a smooth slide, no jump
        assert!(worst_resume < v * 0.016 + 3.0, "{worst_resume}");
        assert!(worst_step_change < 0.1, "{worst_step_change}");
        // and back on the data
        let truth = truth_at(&m, 1, T0 + 150_000.0, truth, T0);
        assert!(metres(&prev, &truth) < 2.0, "{}", metres(&prev, &truth));
    }

    /// The golden poses of S1.17 (`crates/core/tests/data/golden`, written by
    /// `scripts/port/golden.ts` from the TypeScript) for the cases that have no measured data and
    /// no taxiway network, replayed as the script describes: every column but the gear (S1.20's)
    /// matches, positions to 1e-9 degrees and the rest to 1e-6.
    #[test]
    fn the_golden_poses_match_the_typescript_but_for_the_gear() {
        let cases = [
            include_str!("../../../tests/data/golden/motion-aee604.json"),
            include_str!("../../../tests/data/golden/motion-baw9173.json"),
            include_str!("../../../tests/data/golden/motion-dal1424.json"),
            include_str!("../../../tests/data/golden/motion-dal2049.json"),
            include_str!("../../../tests/data/golden/motion-klc20d.json"),
            include_str!("../../../tests/data/golden/motion-klm76f.json"),
            include_str!("../../../tests/data/golden/motion-skw446w.json"),
            include_str!("../../../tests/data/golden/motion-swa1571.json"),
            include_str!("../../../tests/data/golden/motion-swa2713.json"),
            include_str!("../../../tests/data/golden/motion-synthetic-arrival-departure.json"),
        ];
        for text in cases {
            let golden: serde_json::Value = serde_json::from_str(text).unwrap();
            let about = golden["about"].as_str().unwrap();
            let airports: Vec<Arc<Airport>> =
                serde_json::from_value::<Vec<Airport>>(golden["airports"].clone()).unwrap().into_iter().map(Arc::new).collect();
            let ids: Vec<u32> = serde_json::from_value(golden["ids"].clone()).unwrap();
            let phases: Vec<String> = serde_json::from_value(golden["phases"].clone()).unwrap();
            let events = golden["events"].as_array().unwrap();
            let poses = golden["poses"].as_array().unwrap();
            let step = golden["stepMs"].as_f64().unwrap();
            let start = golden["startMs"].as_f64().unwrap();
            let frames = golden["frames"].as_u64().unwrap();
            let mut m = Motion::new();
            m.set_airports(&airports);
            let mut current: std::collections::HashMap<u32, LiveFlight> = Default::default();
            let mut next = 0;
            let mut row = 0;
            for frame in 0..frames {
                let now = start + frame as f64 * step;
                while next < events.len() && events[next]["atMs"].as_f64().unwrap() <= now {
                    let e = &events[next];
                    assert_eq!(e["type"], "report", "{about}");
                    let f: LiveFlight = serde_json::from_value(e["flight"].clone()).unwrap();
                    current.insert(f.id, f);
                    next += 1;
                }
                for id in &ids {
                    let Some(f) = current.get(id) else {
                        continue;
                    };
                    let p = pose(&mut m, f, now);
                    let drawn = m.drawn_time(*id, now);
                    let want: Vec<f64> = poses[row].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
                    row += 1;
                    let at = format!("{about} frame {frame}");
                    assert_eq!(want[0], frame as f64, "{at}");
                    assert_eq!(want[1], *id as f64, "{at}");
                    assert!((p.lon - want[2]).abs() <= 1e-9, "{at}: lon {} against {}", p.lon, want[2]);
                    assert!((p.lat - want[3]).abs() <= 1e-9, "{at}: lat {} against {}", p.lat, want[3]);
                    assert!((p.alt_ft - want[4]).abs() <= 1e-6, "{at}: altFt {} against {}", p.alt_ft, want[4]);
                    assert!(signed(p.heading - want[5]).abs() <= 1e-6, "{at}: heading {} against {}", p.heading, want[5]);
                    assert!(signed(p.track - want[6]).abs() <= 1e-6, "{at}: track {} against {}", p.track, want[6]);
                    assert!((p.pitch - want[7]).abs() <= 1e-6, "{at}: pitch {} against {}", p.pitch, want[7]);
                    assert!((p.bank - want[8]).abs() <= 1e-6, "{at}: bank {} against {}", p.bank, want[8]);
                    assert_eq!(p.phase.as_str(), phases[want[10] as usize], "{at}");
                    // (drawn times are ~1.8e12 ms, whose doubles are 2.4e-4 ms apart)
                    assert!((drawn - want[11]).abs() <= 1e-3, "{at}: drawn {drawn} against {}", want[11]);
                }
            }
            assert_eq!(row, poses.len(), "{about}");
        }
    }
}
