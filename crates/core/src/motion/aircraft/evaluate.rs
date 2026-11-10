//! The model at a drawn time, without blending (`motion.ts` `evaluate`, `airAltitude`,
//! `measuredAttitude`, `:1838-2032`, `:2266-2333`, `:1723-1732`; doc 05 §10.10).
//!
//! Horizontally: the smoothed path inside the samples (a Hermite piece), straight back at the
//! first slope before them, and past the newest along the predicted arc and speed profile.
//! Vertically, by the report state in effect: on the field for the ground phases (a takeoff roll
//! rotating and lifting off at its predicted time), a predicted landing roll past a predicted
//! touchdown, else the smoothed altitude curve and the estimated rate past it, levelling off at a
//! selected altitude, shaped into a flare near a known field. Pitch follows the flight path,
//! bank the turn rate (read a moment ahead inside the data), heading the track plus the measured
//! crab.

use super::altitude::{altitude_at, field_removal, flare, flare_slope, vertical_integral, vertical_weight};
use super::consts::*;
use super::delay::state_at;
use super::fit::hermite;
use super::track::{Raw, State, Track, profile};
use super::{Phase, air_pitch, clamp, meas_weight, series_at, smoothstep, wrap_lon, wrap360};
use crate::trains::{js_max, js_min};

/// The model at drawn time `t` (server ms), without blending: the smoothed path inside the
/// samples, the extrapolation past them, and the vertical profile, phase and attitude of `state`
/// (`None`: the report state in effect at `t`). Needs a sample and a state (a track that has taken
/// a report).
pub fn evaluate(tr: &Track, t: f64, state: Option<&State>) -> Raw {
    let s = match state {
        Some(s) => s,
        None => state_at(tr, t),
    };
    let mut out = Raw::default();
    // --- horizontal
    let n = tr.n;
    let last = n - 1;
    let t_last = tr.ts[last];
    let mut x;
    let mut y;
    let vx;
    let vy;
    let omega; // turn rate, rad/s
    let mut bank_fade = 1.0;
    let mut path_heading = tr.h0; // heading to keep when the aircraft stops
    if t >= t_last {
        let mut dt = (t - t_last) / 1000.0;
        let held = dt >= MAX_EXTRAPOLATION_S;
        if held {
            dt = MAX_EXTRAPOLATION_S;
        }
        let (prof_s, prof_v) = profile(tr, dt);
        let v = if held { 0.0 } else { js_max(0.0, prof_v) };
        let k = tr.kappa;
        let h0 = tr.h0;
        let s_turn = js_min(prof_s, tr.s_turn_max);
        let h = h0 + k * s_turn;
        // from the end of the smoothed path, along its direction
        x = tr.fx[last];
        y = tr.fy[last];
        if (k * s_turn).abs() < 1e-9 {
            x += s_turn * h0.sin();
            y += s_turn * h0.cos();
        } else {
            // constant-curvature arc: d(east)/ds = sin h, d(north)/ds = cos h, h = h0 + k s
            x += (h0.cos() - h.cos()) / k;
            y += (h.sin() - h0.sin()) / k;
        }
        x += (prof_s - s_turn) * h.sin();
        y += (prof_s - s_turn) * h.cos();
        vx = v * h.sin();
        vy = v * h.cos();
        path_heading = h;
        let turn_rad;
        if tr.turn_end_s < TURN_HORIZON_S {
            // rolling out onto the selected heading
            turn_rad = if dt < tr.turn_end_s { k * v } else { 0.0 };
            bank_fade = smoothstep((tr.turn_end_s - dt) / ROLL_OUT_S);
        } else {
            turn_rad = if dt < TURN_HORIZON_S { k * v } else { 0.0 };
            bank_fade = if dt <= BANK_FADE_START_S {
                1.0
            } else {
                js_max(0.0, (TURN_HORIZON_S - dt) / (TURN_HORIZON_S - BANK_FADE_START_S))
            };
        }
        // the smoothed path's own turn rate hands over to the prediction's
        omega = if n > 1 { tr.om[last] + (turn_rad - tr.om[last]) * smoothstep(dt / TURN_HANDOVER_S) } else { turn_rad };
    } else {
        if t <= tr.ts[0] {
            let dt = js_max((t - tr.ts[0]) / 1000.0, -BACKWARD_LIMIT_S);
            x = tr.fx[0] + tr.mx[0] * dt;
            y = tr.fy[0] + tr.my[0] * dt;
            vx = tr.mx[0];
            vy = tr.my[0];
        } else {
            // the smoothed path: a cubic Hermite piece between samples i and i+1
            let h = hermite(tr, t);
            x = h.x;
            y = h.y;
            vx = h.vx;
            vy = h.vy;
        }
        // the turn rate for the bank: what the path does a moment later, which the data already
        // holds (the aircraft is drawn seconds in the past), so the wings roll in before the
        // heading swings and level before it settles; as far as the data goes
        let lead = js_min(t + BANK_LEAD_MS, t_last);
        omega = if lead > tr.ts[0] {
            let h = hermite(tr, lead);
            tr.om[h.i] + (tr.om[h.i + 1] - tr.om[h.i]) * h.u
        } else {
            tr.om[0]
        };
    }
    let lat = clamp(tr.ref_lat + y / M_PER_DEG, -90.0, 90.0);
    let cos_mid = js_max((((tr.ref_lat + lat) / 2.0) * DEG).cos(), 1e-3);
    out.lat = lat;
    out.lon = wrap_lon(tr.ref_lon + x / (M_PER_DEG * cos_mid));
    out.v_e = vx;
    out.v_n = vy;
    let speed = vx.hypot(vy);
    out.speed_ms = speed;
    out.track = if s.phase == Phase::Parked {
        s.track_deg
    } else {
        wrap360((if speed > HEADING_MIN_SPEED_MS { vx.atan2(vy) } else { path_heading }) / DEG)
    };
    // measured values count while fresh (the nose's crab, the bank, the airspeed), then ease out
    let mw = meas_weight(tr, t);

    // --- vertical, phase and attitude
    let mut phase = s.phase;
    let field = s.field.is_some();
    let field_elev = s.field_elev;
    let floor = if field { field_elev } else { js_min(0.0, s.alt) };
    out.has_field = field;
    out.bank = 0.0;
    out.model_bank = 0.0;
    out.vs_fpm = 0.0;
    out.agl = f64::NAN;
    if phase.is_ground() {
        out.ground = true;
        out.alt_msl = if field { field_elev } else { 0.0 };
        out.alt_raw = out.alt_msl;
        out.vs_raw = 0.0;
        out.pitch = 0.0;
        if phase == Phase::LandingRoll && speed < TAXI_MAX_KT * KNOTS_TO_MS {
            phase = Phase::Taxi;
        }
        let lo = s.liftoff_ms;
        if phase == Phase::TakeoffRoll && lo.is_finite() {
            if t < lo {
                out.pitch = ROTATION_PITCH * smoothstep(1.0 - (lo - t) / (ROTATION_S * 1000.0));
            } else {
                // predicted lift-off and initial climb, leaving the ground through the flare's curve
                let u = (t - lo) / 1000.0;
                let raw = FLARE_FLOOR_FT + (TAKEOFF_CLIMB_FPM * vertical_integral(u)) / 60.0;
                let vs = TAKEOFF_CLIMB_FPM * vertical_weight(u);
                out.ground = false;
                out.agl = flare(raw);
                out.alt_raw = out.alt_msl + raw;
                out.vs_raw = vs;
                out.alt_msl = js_min(out.alt_msl + out.agl, MAX_ALT_FT);
                out.vs_fpm = vs * flare_slope(raw);
                phase = if out.agl < APPROACH_MAX_AGL_FT { Phase::InitialClimb } else { Phase::Climb };
                let settled = air_pitch(vs, speed, phase, out.agl);
                out.pitch = ROTATION_PITCH + (settled - ROTATION_PITCH) * smoothstep(u / ROTATION_S);
            }
        }
    } else {
        let td = s.touchdown_ms;
        if td.is_finite() && t >= td {
            // predicted landing roll
            out.ground = true;
            out.alt_msl = field_elev;
            out.alt_raw = field_elev + FLARE_FLOOR_FT;
            out.vs_raw = 0.0;
            out.agl = 0.0;
            phase = if speed < TAXI_MAX_KT * KNOTS_TO_MS { Phase::Taxi } else { Phase::LandingRoll };
            out.pitch = s.touchdown_pitch * (1.0 - smoothstep((t - td) / (TOUCHDOWN_EASE_S * 1000.0)));
        } else {
            let (mut alt, mut vs) = air_altitude(tr, s, t);
            if field && td.is_finite() && t > td - TOUCHDOWN_FIT_MS {
                // the last moments before the predicted touchdown: height still left above the
                // flare's floor then (the data moved on since it was predicted: a QNH re-read, say)
                // eases out, so the wheels meet the runway when the landing roll begins
                let (at_td, _) = air_altitude(tr, s, td);
                let excess = at_td - (field_elev + FLARE_FLOOR_FT);
                if excess > 0.0 {
                    let u = 1.0 - (td - t) / TOUCHDOWN_FIT_MS;
                    alt -= excess * smoothstep(u);
                    vs -= ((excess * 6.0 * u * (1.0 - u)) / TOUCHDOWN_FIT_MS) * 60_000.0;
                }
            }
            out.ground = false;
            out.alt_raw = alt;
            out.vs_raw = vs;
            if field {
                // near the ground, the flare: into the runway at a believable sink rate, never through it
                let raw = js_min(alt, MAX_ALT_FT) - field_elev;
                out.agl = flare(raw);
                out.alt_msl = field_elev + out.agl;
                out.vs_fpm = vs * flare_slope(raw);
                if phase == Phase::Approach && out.agl < FLARE_MAX_AGL_FT {
                    phase = Phase::Flare;
                }
            } else {
                out.alt_msl = clamp(alt, floor, MAX_ALT_FT);
                out.vs_fpm = if alt == out.alt_msl { vs } else { 0.0 };
            }
            // the flight-path angle is through the air: true airspeed when measured
            let air_speed = if mw > 0.0 && tr.tas_ms > 0.0 { speed + (tr.tas_ms - speed) * mw } else { speed };
            out.pitch = air_pitch(out.vs_fpm, air_speed, phase, out.agl);
            let bank = (speed * omega).atan2(GRAVITY_MS2) / DEG;
            out.model_bank = clamp(bank * bank_fade, -MAX_BANK, MAX_BANK);
            let bias = if mw > 0.0 { series_at(&tr.roll_ts, &tr.roll_biases, t, 0.0) * mw } else { 0.0 };
            out.bank = clamp(out.model_bank + bias, -MAX_BANK, MAX_BANK);
        }
    }
    // the nose's crab into a crosswind, as it was at T: kicked out in the flare so the wheels
    // touch down along the runway, and none on the ground, where the aircraft points where it
    // rolls. (The last crab measured in the air would otherwise hold all the way down the runway.)
    let decrab = if out.ground {
        0.0
    } else if out.agl.is_finite() {
        smoothstep(out.agl / DECRAB_AGL_FT)
    } else {
        1.0
    };
    let crab = if mw > 0.0 && decrab > 0.0 { series_at(&tr.crab_ts, &tr.crabs, t, tr.crab) * mw * decrab } else { 0.0 };
    out.heading = wrap360(out.track + crab);
    out.phase = phase;
    out.draw_alt = if out.ground {
        0.0
    } else if s.liftoff_ms.is_finite() && !field {
        out.agl // predicted climb off an unknown field
    } else {
        js_max(0.0, out.alt_msl - field_removal(tr, out.lat, out.lon))
    };
    out
}

/// The model's airborne altitude (ft, corrected) and vertical rate (fpm) at `t` for a state,
/// before the flare shapes them near the ground (the TypeScript's `amAlt`, `amVs`): the smoothed
/// altitude curve inside its samples, and past them on from it at the state's estimated rate
/// (levelling off at a selected altitude).
pub fn air_altitude(tr: &Track, s: &State, t: f64) -> (f64, f64) {
    // the altitude samples count for a state that knew of them: one from before the first
    // (its altitude estimated, say) carries on its own way, so that the state taking over
    // when the drawn time reaches that first sample's report blends the difference out
    // (no samples reads `undefined` in the TypeScript: a false comparison)
    let na = if tr.ats.first().is_some_and(|&first| s.report_ms >= first) { tr.ats.len() } else { 0 };
    let alt;
    let vs;
    if na > 1 && t >= tr.ats[0] && t <= tr.ats[na - 1] {
        // inside the altitude samples: the smoothed curve and its slope
        let (value, slope) = altitude_at(tr, t);
        alt = value;
        vs = slope * 60.0;
    } else if na > 0 && t < tr.ats[0] {
        // before them (a new track is drawn from its buffer's history): back along the
        // smoothed curve's start, as the horizontal path is, so it meets the curve
        vs = clamp(tr.afd[0] * 60.0, -MAX_VRATE_FPM, MAX_VRATE_FPM);
        alt = tr.afv[0] + (vs / 60.0) * js_max((t - tr.ats[0]) / 1000.0, -BACKWARD_LIMIT_S);
    } else {
        // past them: on from the smoothed newest altitude (or the report) at the
        // estimated rate, the curve's own rate easing into it (C1)
        let mut base_ms = s.report_ms;
        let mut base = s.alt;
        let mut base_rate = s.vrate * vertical_weight(0.0);
        if na > 0 && t >= tr.ats[0] {
            base_ms = tr.ats[na - 1];
            base = tr.afv[na - 1];
            base_rate = clamp(tr.afd[na - 1] * 60.0, -MAX_VRATE_FPM, MAX_VRATE_FPM);
        }
        let dt_r = (t - s.report_ms) / 1000.0;
        let dt_b = (base_ms - s.report_ms) / 1000.0;
        let extra = base_rate - s.vrate * vertical_weight(dt_b);
        let ease = (-js_max(0.0, (t - base_ms) / 1000.0) / VRATE_HANDOVER_S).exp();
        let mut climb =
            (s.vrate * (vertical_integral(dt_r) - vertical_integral(dt_b))) / 60.0 + (extra / 60.0) * VRATE_HANDOVER_S * (1.0 - ease);
        let mut vs_scale = 1.0;
        let vs_weight = vertical_weight(dt_r);
        let cap = s.cap_alt;
        if !cap.is_nan() {
            // altitude capture: ease onto the selected altitude (C1: the rate goes smoothly to 0)
            let d = cap - base;
            let band = clamp(s.vrate.abs() * CAPTURE_FT_PER_FPM, MIN_CAPTURE_FT, MAX_CAPTURE_FT);
            let kk = js_min(1.0, band / d.abs());
            let xx = climb / d;
            if xx >= 1.0 + kk {
                climb = d;
                vs_scale = 0.0;
            } else if xx > 1.0 - kk {
                let u = xx - (1.0 - kk);
                climb = d * (xx - (u * u) / (4.0 * kk));
                vs_scale = 1.0 - u / (2.0 * kk);
            }
        }
        alt = base + climb;
        vs = (s.vrate * vs_weight + extra * ease) * vs_scale;
    }
    (alt, vs)
}

/// Each measured bank as an offset to the model's own at the time it was reported (interpolated
/// at the drawn time): `roll_biases` (`measuredAttitude`).
pub fn measured_attitude(tr: &mut Track) {
    let n = tr.roll_ts.len();
    tr.roll_biases.clear();
    tr.roll_biases.resize(n, 0.0); // the model's own bank (`model_bank`) is what is compared
    for k in 0..n {
        let raw = evaluate(tr, tr.roll_ts[k], None);
        tr.roll_biases[k] = if raw.ground { 0.0 } else { tr.rolls[k] - raw.model_bank };
    }
}

#[cfg(test)]
mod tests {
    use crate::assert_close;
    use crate::model::LiveFlight;
    use crate::motion::aircraft::blend::testing::*;
    use crate::motion::aircraft::consts::{DEG, M_PER_DEG};
    use crate::motion::aircraft::{Motion, Phase};
    use crate::num::js_round;
    use crate::trains::js_max;

    #[test]
    fn straight_flight_matches_dead_reckoning_a_few_seconds_in_the_past() {
        let mut m = Motion::new();
        let f = LiveFlight { track: 90, speed: 250, ..flight() };
        pose(&mut m, &f, T0); // taken in as it happens
        // drawn behind real time, by 2-12 s
        let lag = T0 + 30_000.0 - m.drawn_time(1, T0 + 30_000.0);
        assert!(lag >= 2_000.0, "{lag}");
        assert!(lag <= 12_000.0, "{lag}");
        let p = pose_at(&mut m, &f, T0 + 30_000.0);
        let truth = offset(45.0, 10.0, 250.0 * KT * 30.0, 0.0);
        assert!(metres(&p, &truth) < 5.0, "{}", metres(&p, &truth));
        assert_close!(p.heading, 90.0, 1);
        assert_close!(p.bank, 0.0, 3);

        // and with a buffer along the same line
        let line = |s: f64| (0.0, 250.0 * KT * s);
        let g = LiveFlight { id: 2, track: 0, speed: 250, ..path_flight(line, T0, &BUFFER) };
        pose(&mut m, &g, T0);
        let q = pose_at(&mut m, &g, T0 + 38_000.0);
        let (e, n) = line(38.0);
        assert!(metres(&q, &offset(45.0, 10.0, e, n)) < 5.0);
    }

    #[test]
    fn the_smoothed_path_stays_on_exact_samples() {
        let mut m = Motion::new();
        let path = circle(220.0, 2.0);
        let f = LiveFlight { speed: 220, alt: 12_000, ..path_flight(path, T0, &BUFFER) };
        pose(&mut m, &f, T0);
        // smoothing takes out jitter, not a steady turn: exact (1 m quantised) samples, the report and
        // the history before it, are kept to within a metre or so
        let (lat, lon) = (seen(f.lat as f64), seen(f.lon as f64));
        let mut points = vec![[0, 0, 0]];
        points.extend(f.positions.iter().copied());
        for [d_lat, d_lon, d_ms] in points {
            let p = pose_at(&mut m, &f, T0 - d_ms as f64);
            let sample = (lat - d_lat as f64 * 1e-5, lon - d_lon as f64 * 1e-5);
            assert!(metres(&p, &sample) < 1.5, "{d_ms}: {}", metres(&p, &sample));
        }
        // and runs smoothly between them, close to the true path
        let p = pose_at(&mut m, &f, T0 - 5_000.0);
        let (e, n) = path(-5.0);
        assert!(metres(&p, &offset(45.0, 10.0, e, n)) < 3.0);
    }

    #[test]
    fn bank_follows_the_turn_pitch_follows_the_climb() {
        let mut m = Motion::new();
        let r = LiveFlight { id: 1, speed: 250, alt: 20_000, ..path_flight(circle(250.0, 2.0), T0, &BUFFER) };
        let l = LiveFlight { id: 2, speed: 250, alt: 20_000, ..path_flight(circle(250.0, -2.0), T0, &BUFFER) };
        pose(&mut m, &r, T0);
        pose(&mut m, &l, T0);
        let right = pose_at(&mut m, &r, T0 + 12_000.0);
        let left = pose_at(&mut m, &l, T0 + 12_000.0);
        // rate-two turn at 250 kt: atan(v ω / g) ≈ 24°
        assert!(right.bank > 20.0 && right.bank < 28.0, "{}", right.bank);
        assert!(left.bank < -20.0 && left.bank > -28.0, "{}", left.bank);
        // inside the samples too, from the smoothed path's own turn
        assert!(pose_at(&mut m, &r, T0 + 4_000.0).bank > 20.0);
        // bank fades out as the predicted turn ends
        assert_eq!(pose_at(&mut m, &r, T0 + 75_000.0).bank, 0.0);

        let mut pitch = |vspeed: i32, id: u32| {
            let f = LiveFlight { id, alt: 20_000, speed: 300, vspeed: Some(vspeed), ..flight() };
            pose(&mut m, &f, T0);
            pose_at(&mut m, &f, T0 + 5_000.0)
        };
        let climb = pitch(2_000, 10);
        let cruise = pitch(0, 11);
        let descent = pitch(-2_000, 12);
        assert_eq!(climb.phase, Phase::Climb);
        assert_eq!(cruise.phase, Phase::Cruise);
        assert_eq!(descent.phase, Phase::Descent);
        assert!(climb.pitch > cruise.pitch);
        assert!(cruise.pitch > descent.pitch);
    }

    #[test]
    fn trail_heights_match_the_models() {
        let mut m = Motion::new();
        m.set_airports(&[airport_at("DEN", 45.0, 10.0, 5_000)]);
        let f = LiveFlight { destination: "DEN".into(), alt: 7_000, speed: 160, vspeed: Some(-800), track: 0, ..flight() };
        pose(&mut m, &f, T0);
        let p = pose_at(&mut m, &f, T0);
        let (lat, lon) = (seen(f.lat as f64), seen(f.lon as f64));
        assert_close!(m.draw_altitude(1, 7_000.0, lat, lon), p.alt_ft, 3);
        assert_eq!(m.draw_altitude(1, 0.0, lat, lon), 0.0);
        // far away, plain MSL
        let (lat, lon) = offset(45.0, 10.0, 0.0, 80_000.0);
        assert_eq!(m.draw_altitude(1, 7_000.0, lat, lon), 7_000.0);
        assert_eq!(m.draw_altitude(999, 7_000.0, lat, lon), 7_000.0);
    }

    // --- Delayed, smoothed playback

    /// A small deterministic generator, uniform in [-1, 1].
    fn noise_source(seed: f64) -> impl FnMut() -> f64 {
        let mut s = seed;
        move || {
            s = (s * 16807.0) % 2147483647.0;
            (s / 2147483647.0) * 2.0 - 1.0
        }
    }

    /// `jitter`: each moment's own noise, the same in every report that holds it.
    fn jitter(noise: f64, s: f64, axis: f64, k: f64) -> f64 {
        let x = ((s * k + axis) * 12.9898).sin() * 43758.5453;
        noise * ((x - x.floor()) * 2.0 - 1.0)
    }

    #[derive(Debug, Default)]
    struct Flown {
        /// RMS and largest deviation of the drawn acceleration's size from the truth's, m/s².
        rms_accel: f64,
        max_accel: f64,
        /// Largest change of the drawn velocity from one 16 ms frame to the next, beyond the truth's, m/s.
        max_velocity_step: f64,
        /// Largest turn of the drawn heading, deg/s.
        max_heading_rate: f64,
        /// RMS and largest distance from the noise-free truth at the drawn time, m.
        rms_error: f64,
        max_error: f64,
        /// Largest height error at the drawn time, ft, and largest change of vertical speed per frame, ft/s.
        max_alt_error: f64,
        max_climb_step: f64,
    }

    /// `fly`: Flightradar24-like reports through 16 ms frames for two minutes, the drawn path
    /// measured: a report every 8 s with the 8 s before it in its buffer (2 s apart), each
    /// arriving 4 s after its time, at 250 kt turning right at `turn_deg` deg/s and climbing
    /// 600 fpm; positions off by up to ±`noise` m and altitudes by ±`alt_noise` ft, in 25 ft steps.
    fn fly(turn_deg: f64, noise: f64, alt_noise: f64) -> Flown {
        let mut m = Motion::new();
        let mut rnd = noise_source(7.0);
        let v = 250.0 * KT;
        let w = turn_deg * DEG;
        let truth = |s: f64| if w == 0.0 { (v * s, 0.0) } else { ((v / w) * (w * s).sin(), -(v / w) * (1.0 - (w * s).cos())) };
        let accel_of = |s: f64| (-v * w * (w * s).sin(), -v * w * (w * s).cos());
        let alt_at = |s: f64| 20_000.0 + 10.0 * s;
        let measured = |s: f64| {
            let (e, n) = truth(s);
            offset(45.0, 10.0, e + jitter(noise, s, 0.0, 2.0), n + jitter(noise, s, 1.0, 2.0))
        };
        let mut report = |k: i64| {
            let s0 = (k * 8) as f64;
            let (lat, lon) = measured(s0);
            let alt = js_round((alt_at(s0) + alt_noise * rnd()) / 25.0) * 25.0;
            LiveFlight {
                track: (90.0 + turn_deg * s0) as i32,
                alt: alt as i32,
                speed: 250,
                timestamp_ms: (T0 + s0 * 1000.0) as u64,
                ..flight_with_history(lat, lon, |d| measured(s0 - d), &BUFFER)
            }
        };
        let dt = 0.016;
        let mut k = 0;
        let mut f = report(0);
        let start = T0 + 24_000.0;
        let mut t = T0 + 4_000.0;
        while t < start {
            while T0 + ((k + 1) * 8_000) as f64 + 4_000.0 <= t {
                k += 1;
                f = report(k);
            }
            pose(&mut m, &f, t);
            t += 16.0;
        }
        let to_en = |lat: f64, lon: f64| ((lon - 10.0) * M_PER_DEG * (45.0 * DEG).cos(), (lat - 45.0) * M_PER_DEG);
        let mut prev: Option<(f64, f64)> = None;
        let mut prev_v: Option<(f64, f64)> = None;
        let mut prev_heading = f64::NAN;
        let mut prev_alt = f64::NAN;
        let mut prev_climb = f64::NAN;
        let mut out = Flown::default();
        let mut n = 0;
        let mut sum_a = 0.0;
        let mut sum_e = 0.0;
        let mut t = start;
        while t < start + 120_000.0 {
            while T0 + ((k + 1) * 8_000) as f64 + 4_000.0 <= t {
                k += 1;
                f = report(k);
            }
            let p = pose(&mut m, &f, t);
            let s = (m.drawn_time(1, t) - T0) / 1000.0;
            let pos = to_en(p.lat, p.lon);
            let (te, tn) = truth(s);
            let err = (pos.0 - te).hypot(pos.1 - tn);
            out.max_error = js_max(out.max_error, err);
            sum_e += err * err;
            out.max_alt_error = js_max(out.max_alt_error, (p.alt_ft - alt_at(s)).abs());
            if let Some(prev) = prev {
                let vel = ((pos.0 - prev.0) / dt, (pos.1 - prev.1) / dt);
                if let Some(prev_v) = prev_v {
                    let (ax, ay) = accel_of(s);
                    let dvx = vel.0 - prev_v.0 - ax * dt;
                    let dvy = vel.1 - prev_v.1 - ay * dt;
                    out.max_velocity_step = js_max(out.max_velocity_step, dvx.hypot(dvy));
                    let dev = ((vel.0 - prev_v.0).hypot(vel.1 - prev_v.1) / dt - v * w.abs()).abs();
                    out.max_accel = js_max(out.max_accel, dev);
                    sum_a += dev * dev;
                    n += 1;
                }
                prev_v = Some(vel);
            }
            prev = Some(pos);
            if !prev_heading.is_nan() {
                out.max_heading_rate = js_max(out.max_heading_rate, signed(p.heading - prev_heading).abs() / dt);
            }
            prev_heading = p.heading;
            if !prev_alt.is_nan() {
                let climb = (p.alt_ft - prev_alt) / dt;
                if !prev_climb.is_nan() {
                    out.max_climb_step = js_max(out.max_climb_step, (climb - prev_climb).abs());
                }
                prev_climb = climb;
            }
            prev_alt = p.alt_ft;
            t += 16.0;
        }
        out.rms_accel = (sum_a / n as f64).sqrt();
        out.rms_error = (sum_e / n as f64).sqrt();
        out
    }

    #[test]
    fn noisy_positions_are_smoothed_no_sample_rate_wiggle_close_to_the_truth() {
        // Drawn at "now" through each sample, the same data gave RMS accelerations of
        // ~58 m/s² (peaks ~1600), heading rates of 23-30°/s and errors up to 110 m.
        for turn in [0.0, 3.0] {
            let r = fly(turn, 30.0, 25.0);
            assert!(r.rms_accel < 5.0, "{turn}: {r:?}");
            assert!(r.max_accel < 30.0, "{turn}: {r:?}");
            assert!(r.max_heading_rate < turn + 4.0, "{turn}: {r:?}");
            // (4 s late every 8 s, the delay is at its 12 s cap: the drawn time reaches the newest
            // report, the noisiest end of the smoothed path, just before the next one arrives)
            assert!(r.rms_error < 16.0, "{turn}: {r:?}");
            assert!(r.max_error < 35.0, "{turn}: {r:?}");
            assert!(r.max_alt_error < 40.0, "{turn}: {r:?}");
            assert!(r.max_climb_step < 1.0, "{turn}: {r:?}");
        }
        // and exact data is followed exactly
        for turn in [0.0, 3.0] {
            let r = fly(turn, 0.0, 0.0);
            assert!(r.max_error < 3.0, "{turn}: {r:?}");
            assert!(r.rms_accel < 0.5, "{turn}: {r:?}");
            assert!(r.max_accel < 3.0, "{turn}: {r:?}");
            assert!(r.max_heading_rate < turn + 0.5, "{turn}: {r:?}");
        }
    }

    // --- The bank

    #[derive(Debug, Clone, Copy)]
    struct BankFrame {
        /// Drawn time, s from T0.
        t: f64,
        bank: f64,
        track: f64,
    }

    /// `flyRates`: Flightradar24-like reports through 16 ms frames (a report every 8 s holding the
    /// 8 s before it, 2 s apart, each arriving `latency` s after its time) at 250 kt, turning at
    /// `rate(s)` deg/s (clockwise) `s` seconds from T0 and heading north at s = 0; the reports are
    /// at s = `phase` + 8k. Positions are off by up to ±`noise` m, each moment's the same in every
    /// report that holds it. Returns every frame from drawn time `from` s to `to` s. (Tracks are
    /// whole degrees here.)
    fn fly_rates(rate: impl Fn(f64) -> f64, phase: f64, noise: f64, from: f64, to: f64, latency: f64) -> Vec<BankFrame> {
        // the path, integrated in 10 ms steps from 40 s before `from`, then moved to the origin at s = 0
        let v = 250.0 * KT;
        let dt = 0.01;
        let s0 = from - 40.0;
        let steps = ((to + 20.0 - s0) / dt).ceil() as usize;
        let mut es = vec![0.0; steps + 1];
        let mut ns = vec![0.0; steps + 1];
        let mut hs = vec![0.0; steps + 1];
        for k in 0..steps {
            let r = rate(s0 + (k as f64 + 0.5) * dt) * DEG;
            let mid = hs[k] + (r * dt) / 2.0;
            es[k + 1] = es[k] + v * mid.sin() * dt;
            ns[k + 1] = ns[k] + v * mid.cos() * dt;
            hs[k + 1] = hs[k] + r * dt;
        }
        let at = |arr: &[f64], s: f64| {
            let x = (s - s0) / dt;
            let k = ((steps - 1) as f64).min(js_max(0.0, x.floor())) as usize;
            arr[k] + (arr[k + 1] - arr[k]) * (x - k as f64)
        };
        let h0 = at(&hs, 0.0);
        let e0 = at(&es, 0.0);
        let n0 = at(&ns, 0.0);
        // rotated so that the heading at s = 0 is north
        let measured = |s: f64| {
            let e = at(&es, s) - e0;
            let n = at(&ns, s) - n0;
            let c = h0.cos();
            let si = h0.sin();
            offset(45.0, 10.0, e * c - n * si + jitter(noise, s, 0.0, 2.0), e * si + n * c + jitter(noise, s, 1.0, 2.0))
        };
        let report = |k: i64| {
            let s = phase + (8 * k) as f64;
            let (lat, lon) = measured(s);
            let track = ((((at(&hs, s) - h0) / DEG) % 360.0) + 360.0) % 360.0;
            LiveFlight {
                track: js_round(track) as i32,
                alt: 20_000,
                speed: 250,
                timestamp_ms: (T0 + s * 1000.0) as u64,
                ..flight_with_history(lat, lon, |d| measured(s - d), &BUFFER)
            }
        };
        let mut m = Motion::new();
        let mut k = ((from - 40.0 - phase) / 8.0).floor() as i64;
        let mut f = report(k);
        let mut frames = Vec::new();
        let mut t = T0 + (phase + (8 * k) as f64 + latency) * 1000.0;
        loop {
            while T0 + (phase + (8 * (k + 1)) as f64 + latency) * 1000.0 <= t {
                k += 1;
                f = report(k);
            }
            let p = pose(&mut m, &f, t);
            let drawn = (m.drawn_time(1, t) - T0) / 1000.0;
            if drawn > to {
                break;
            }
            if drawn >= from {
                frames.push(BankFrame { t: drawn, bank: p.bank, track: p.track });
            }
            t += 16.0;
        }
        frames
    }

    /// The frame drawn nearest `t` (the first of equals).
    fn frame_at(frames: &[BankFrame], t: f64) -> BankFrame {
        frames.iter().copied().reduce(|best, f| if (f.t - t).abs() < (best.t - t).abs() { f } else { best }).unwrap()
    }

    /// Largest change of the bank from one frame to the next.
    fn bank_step(frames: &[BankFrame]) -> f64 {
        frames.windows(2).fold(0.0, |w, p| js_max(w, (p[1].bank - p[0].bank).abs()))
    }

    /// A coordinated turn's bank at 250 kt and `rate` deg/s: atan(v ω / g).
    fn physical_bank(rate: f64) -> f64 {
        ((250.0 * KT * rate * DEG) / 9.80665).atan() / DEG
    }

    #[test]
    fn the_bank_rolls_into_a_turn_from_the_data_ahead_before_the_heading_swings() {
        // straight north, then from s = 0 a right turn at 2 deg/s (a coordinated bank of ~24.6 deg);
        // the drawn aircraft is ~12 s behind, so the reports already hold what it does next
        let bank = physical_bank(2.0);
        let frames = fly_rates(|s| if s < 0.0 { 0.0 } else { 2.0 }, 4.0, 0.0, -20.0, 30.0, 4.0);
        let at = |t: f64| frame_at(&frames, t);
        // wings level well before the turn...
        assert!(at(-5.0).bank.abs() < 2.0, "{:?}", at(-5.0));
        // ...already rolling in a second before the heading starts to swing, which it hasn't yet...
        assert!(at(-1.0).bank > 8.0, "{:?}", at(-1.0));
        assert!(signed(at(-1.0).track).abs() < 1.0, "{:?}", at(-1.0));
        // ...more than half way as it does...
        assert!(at(0.0).bank > 0.5 * bank, "{:?}", at(0.0));
        // ...and banked to the turn once in it
        assert!(at(5.0).bank > 18.0 && at(5.0).bank < 28.0, "{:?}", at(5.0));
    }

    #[test]
    fn a_turns_bank_whenever_the_reports_come_level_before_it_never_the_wrong_way_at_the_physical_bank_in_it() {
        // reports at every phase of their 8 s cycle against the turn, arriving 1 s or 4 s late (the drawn
        // time then has ~3-11 s or ~0-8 s of data ahead of it)
        for rate in [2.0, -2.0] {
            let bank = physical_bank(rate);
            for latency in [1.0, 4.0] {
                for phase in 0..8 {
                    let frames = fly_rates(|s| if s < 0.0 { 0.0 } else { rate }, phase as f64, 0.0, -20.0, 30.0, latency);
                    for f in &frames {
                        if f.t <= -6.0 {
                            assert!(f.bank.abs() < 2.0, "{rate} {latency} {phase}: {f:?}");
                        }
                        assert!(f.bank * rate.signum() > -1.0, "{rate} {latency} {phase}: {f:?}");
                        if f.t >= 6.0 {
                            assert!((f.bank - bank).abs() < 0.15 * bank.abs(), "{rate} {latency} {phase}: {f:?}");
                        }
                    }
                    // rolling smoothly, report arrivals included (under ~19 deg/s)
                    assert!(bank_step(&frames) < 0.3, "{rate} {latency} {phase}: {}", bank_step(&frames));
                }
            }
        }
    }

    #[test]
    fn the_wings_level_as_the_turn_ends_as_they_rolled_in_as_it_began() {
        for rate in [2.0, -2.0] {
            let bank = physical_bank(rate);
            for latency in [1.0, 4.0] {
                for phase in 0..8 {
                    // a turn that ends at s = 0, heading north
                    let frames = fly_rates(|s| if s < 0.0 { rate } else { 0.0 }, phase as f64, 0.0, -20.0, 30.0, latency);
                    for f in &frames {
                        if f.t <= -6.0 {
                            assert!((f.bank - bank).abs() < 0.15 * bank.abs(), "{rate} {latency} {phase}: {f:?}");
                        }
                        // level a few seconds on: by 4 s with the straight in the data, by 6 s whenever the
                        // next report comes (the end of the data can't tell a straight from a gentle turn)
                        if f.t >= if latency == 1.0 { 4.0 } else { 6.0 } {
                            assert!(f.bank.abs() < 2.0, "{rate} {latency} {phase}: {f:?}");
                        }
                        assert!(f.bank * rate.signum() > -1.5, "{rate} {latency} {phase}: {f:?}");
                    }
                    assert!(bank_step(&frames) < 0.3, "{rate} {latency} {phase}: {}", bank_step(&frames));
                }
            }
            // the roll-out mirrors the roll-in: rolled out by as much as the same turn starting then had rolled in
            let into = fly_rates(|s| if s < 0.0 { 0.0 } else { rate }, 4.0, 0.0, -20.0, 30.0, 4.0);
            let out = fly_rates(|s| if s < 0.0 { rate } else { 0.0 }, 4.0, 0.0, -20.0, 30.0, 4.0);
            let mut t = -3.0;
            while t <= 4.0 {
                let sum = frame_at(&into, t).bank + frame_at(&out, t).bank;
                assert!((sum - bank).abs() < 2.0, "{rate} at {t}: {sum}");
                t += 0.5;
            }
            // rolling out before the turn ends
            assert!(frame_at(&out, 0.0).bank.abs() < 0.5 * bank.abs());
        }
    }

    #[test]
    fn the_bank_doesnt_wobble_with_noisy_positions() {
        // positions off by up to ±10 m (±30 m: as in `fly`, more than Flightradar24's ADS-B data has), straight
        // and in a turn: the bank stays near the truth's and changes no faster than an aircraft rolls
        for (noise, rms_max) in [(10.0, 2.0), (30.0, 5.0)] {
            for rate in [0.0, 2.0] {
                let frames = fly_rates(|_| rate, 4.0, noise, 0.0, 120.0, 4.0);
                let bank = physical_bank(rate);
                let rms = (frames.iter().map(|f| (f.bank - bank).powi(2)).sum::<f64>() / frames.len() as f64).sqrt();
                assert!(rms < rms_max, "{noise} {rate}: {rms}");
                let step = bank_step(&frames);
                assert!(step < if noise == 10.0 { 0.15 } else { 0.3 }, "{noise} {rate}: {step}");
            }
        }
    }
}
