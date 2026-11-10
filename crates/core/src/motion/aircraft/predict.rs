//! Prediction past the newest sample (`motion.ts` `predict`, `captureAltitude`, `:1754-1836`,
//! `:2343-2358`; doc 05 §10.9), and the refit that runs after every change to the samples or the
//! estimates (`refresh`, `refit`, `:1614-1627`).
//!
//! From the end of the smoothed path the aircraft carries on along a constant-curvature arc (the
//! estimated turn rate, for up to a minute), with a speed profile by phase: a taxi settles at
//! taxi speed, a takeoff roll accelerates to rotation and lifts off, a landing roll brakes, and in
//! the air the estimated acceleration applies for a while; a descent on approach reaches the
//! field at a predicted touchdown, after which it rolls out. Fresh measured autopilot targets end
//! a turn at the selected heading and level a climb off at the selected altitude.

use super::altitude::{fit_altitude, vertical_integral_inverse, vertical_weight};
use super::consts::*;
use super::delay::snapshot;
use super::evaluate::measured_attitude;
use super::fit::{fit_track, turn_nodes};
use super::track::{Track, profile, ramp};
use super::{Phase, World, air_pitch, clamp, meas_fresh, sign0, wrap180};
use crate::trains::{js_max, js_min};

/// Extrapolation start (`v0`, `h0`, `kappa`), speed profile and touchdown / lift-off predictions,
/// the turn's end and the level-off altitude, from the smoothed path's end and the estimates.
pub fn predict(tr: &mut Track) {
    let last = tr.n - 1;
    let parked = tr.phase == Phase::Parked;
    let vx = tr.mx[last];
    let vy = tr.my[last];
    let speed = js_min(vx.hypot(vy), MAX_SPEED_MS);
    tr.h0 = if speed > HEADING_MIN_SPEED_MS && !parked { vx.atan2(vy) } else { tr.track_deg * DEG };
    tr.v0 = if parked { 0.0 } else { speed };
    tr.kappa = if tr.v0 > HEADING_MIN_SPEED_MS {
        clamp((tr.turn_rate * DEG) / tr.v0, -MAX_CURVATURE, MAX_CURVATURE)
    } else {
        0.0
    };

    tr.seg_a.clear();
    tr.seg_dur.clear();
    tr.prof_end_v = tr.v0;
    tr.prof_end_t = 0.0;
    tr.touchdown_ms = f64::NAN;
    tr.liftoff_ms = f64::NAN;
    let t_last = tr.ts[last];
    let accel = tr.accel * KNOTS_TO_MS;
    let v0 = tr.v0;

    match tr.phase {
        Phase::Parked => {}
        Phase::Taxi => {
            let a = clamp(accel, -MAX_TAXI_ACCEL_KTS * KNOTS_TO_MS, MAX_TAXI_ACCEL_KTS * KNOTS_TO_MS);
            ramp(tr, a, if a > 0.0 { js_max(v0, MAX_TAXI_SPEED_KT * KNOTS_TO_MS) } else { 0.0 }, ACCEL_HORIZON_S);
        }
        Phase::TakeoffRoll => {
            if tr.cls != AircraftClass::Heli {
                let rotation = rotation_kt(tr.cls);
                ramp(tr, TAKEOFF_ACCEL_KTS * KNOTS_TO_MS, (rotation + TAKEOFF_OVERSPEED_KT) * KNOTS_TO_MS, f64::INFINITY);
                tr.liftoff_ms = tr.report_ms + (js_max(0.0, rotation - tr.speed_kt) / TAKEOFF_ACCEL_KTS) * 1000.0;
            }
        }
        Phase::LandingRoll => {
            ramp(tr, js_min(accel, -LANDING_DECEL_KTS * KNOTS_TO_MS), ROLLOUT_KT * KNOTS_TO_MS, f64::INFINITY);
        }
        _ => {
            // airborne: does the descent reach the field within the vertical horizon?
            if matches!(tr.phase, Phase::Approach | Phase::Flare) && tr.field.is_some() && tr.vrate < 0.0 {
                // from the smoothed altitude, as drawn, where the newest altitude is this report's
                let na = tr.ats.len();
                let alt0 = if na > 0 && tr.ats[na - 1] >= tr.report_ms - COINCIDENT_MS {
                    tr.afv[na - 1]
                        + ((clamp(tr.afd[na - 1] * 60.0, -MAX_VRATE_FPM, MAX_VRATE_FPM) - tr.vrate) / 60.0) * VRATE_HANDOVER_S
                } else {
                    tr.alt
                };
                let needed = js_max(0.0, (alt0 - tr.field_elev - FLARE_FLOOR_FT) / (-tr.vrate / 60.0));
                let t = vertical_integral_inverse(needed);
                if t.is_finite() {
                    tr.touchdown_ms = tr.report_ms + t * 1000.0;
                }
            }
            let change = MAX_AIR_SPEED_CHANGE_KT * KNOTS_TO_MS;
            let until_touchdown =
                if tr.touchdown_ms.is_finite() { js_max(0.0, (tr.touchdown_ms - t_last) / 1000.0) } else { f64::INFINITY };
            ramp(
                tr,
                accel,
                if accel > 0.0 { v0 + change } else { js_max(0.0, v0 - change) },
                js_min(ACCEL_HORIZON_S, until_touchdown),
            );
            if until_touchdown.is_finite() {
                let end_t = tr.prof_end_t;
                ramp(tr, 0.0, 0.0, until_touchdown - end_t);
                if tr.prof_end_v > ROLLOUT_KT * KNOTS_TO_MS {
                    ramp(tr, -LANDING_DECEL_KTS * KNOTS_TO_MS, ROLLOUT_KT * KNOTS_TO_MS, f64::INFINITY);
                }
                // the attitude at touchdown, which the rollout eases down from
                let (_, prof_v) = profile(tr, until_touchdown);
                let vs = TOUCHDOWN_SINK * tr.vrate * vertical_weight((tr.touchdown_ms - tr.report_ms) / 1000.0);
                tr.touchdown_pitch = js_max(0.0, air_pitch(vs, prof_v, Phase::Flare, 0.0));
            }
        }
    }
    let (prof_s, _) = profile(tr, TURN_HORIZON_S);
    tr.s_turn_max = prof_s;

    // measured autopilot targets: a turn onto the selected heading stops there,
    // and a climb or descent levels off at the selected altitude
    tr.turn_end_s = TURN_HORIZON_S;
    if tr.nav_track.is_finite() && tr.kappa != 0.0 && tr.v0 > HEADING_MIN_SPEED_MS && !tr.phase.is_ground() {
        let togo = wrap180(tr.nav_track - tr.h0 / DEG);
        if sign0(togo) == sign0(tr.kappa) || togo.abs() < 1.0 {
            let s = ((togo * DEG) / tr.kappa).abs();
            if s < tr.s_turn_max {
                tr.s_turn_max = s;
                tr.turn_end_s = s / tr.v0;
            }
        }
    }
    tr.cap_alt = capture_altitude(tr);
}

/// The selected altitude a climb or descent is heading for, from fresh measured data; NaN if
/// none.
pub fn capture_altitude(tr: &Track) -> f64 {
    let Some(m) = &tr.meas else {
        return f64::NAN;
    };
    if !meas_fresh(tr, tr.report_ms) || tr.phase.is_ground() || tr.phase == Phase::Approach || tr.phase == Phase::Flare {
        return f64::NAN;
    }
    if tr.vrate.abs() < 100.0 {
        return f64::NAN;
    }
    let mut best = f64::NAN;
    for c in [m.nav_altitude_mcp, m.nav_altitude_fms] {
        let Some(c) = c else {
            continue;
        };
        if !c.is_finite() || c <= 0.0 {
            continue;
        }
        let d = c - tr.alt;
        // only a target ahead of the climb or descent; one behind it is no level-off
        if d * tr.vrate < 0.0 || d.abs() < 1.0 {
            continue;
        }
        if !(d.abs() >= (best - tr.alt).abs()) {
            best = c;
        }
    }
    best
}

impl World {
    /// Re-derives the extrapolation after samples or estimates changed without a newer report.
    pub fn refresh(&mut self, tr: &mut Track) {
        let reported_only = tr.ts.last() == Some(&tr.report_ms);
        self.refit(tr, reported_only);
    }

    /// Smoothed curves, extrapolation and predictions, the latest state's snapshot, and the
    /// measured roll's bias.
    pub fn refit(&mut self, tr: &mut Track, reported_only: bool) {
        fit_track(tr, &mut self.scratch.fit, reported_only);
        fit_altitude(tr, &mut self.scratch.fit.spline);
        predict(tr);
        turn_nodes(tr);
        snapshot(tr);
        measured_attitude(tr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LiveFlight;
    use crate::motion::aircraft::Measured;
    use crate::motion::aircraft::Motion;
    use crate::motion::aircraft::blend::testing::*;

    #[test]
    fn a_constant_rate_turn_continues_along_its_circle() {
        let mut m = Motion::new();
        let path = circle(160.0, 3.0);
        // the circle from T0 on: a report at T0 + 8 s, its buffer back to T0
        let f = LiveFlight { speed: 160, alt: 10_000, track: 24, ..path_flight(|s| path(s + 8.0), T0 + 8_000.0, &BUFFER) };
        pose(&mut m, &f, T0 + 8_000.0);
        for s in [5.0, 10.0, 20.0, 30.0, 50.0] {
            let p = pose_at(&mut m, &f, T0 + s * 1000.0);
            let (e, n) = path(s);
            let d = metres(&p, &offset(45.0, 10.0, e, n));
            assert!(d < 25.0, "{s}: {d}");
        }
        // heading follows the circle: 3 deg/s from north
        crate::assert_close!(pose_at(&mut m, &f, T0 + 20_000.0).heading, 60.0, 0);
    }

    #[test]
    fn an_old_report_still_moves() {
        let mut m = Motion::new();
        let now = T0 + 300_000.0;
        let f = LiveFlight { track: 45, speed: 400, alt: 30_000, ..flight() };
        let p = pose(&mut m, &f, now);
        let q = pose(&mut m, &f, now + 1_000.0);
        assert!(metres(&p, &q) > 400.0 * KT * 0.9);
        assert!(metres(&p, &q) < 400.0 * KT * 1.1);
        // and holds after ten minutes without data
        let r = pose_at(&mut m, &f, T0 + 700_000.0);
        let s = pose_at(&mut m, &f, T0 + 701_000.0);
        assert!(metres(&r, &s) < 0.01);
    }

    #[test]
    fn the_buffer_is_history_a_braking_roll_is_drawn_slowing_down_an_accelerating_one_speeding_up() {
        // first sighted on the ground at 100 kt, the buffer holding the 10 s before: 2.5 kt/s either way
        for accel in [-2.5, 2.5] {
            let mut m = Motion::new();
            let north = |s: f64| 100.0 * KT * s + (accel * KT * s * s) / 2.0;
            let (lat, lon) = offset(45.0, 10.0, 0.0, north(0.0));
            let f = LiveFlight {
                on_ground: true,
                alt: 0,
                speed: 100,
                track: 0,
                ..flight_with_history(lat, lon, |d| offset(45.0, 10.0, 0.0, north(-d)), &[2.0, 4.0, 6.0, 8.0, 10.0])
            };
            pose(&mut m, &f, T0 + 2_000.0);
            let mut at = |t: f64| pose_at(&mut m, &f, t);
            assert_eq!(at(T0).phase, if accel < 0.0 { Phase::LandingRoll } else { Phase::TakeoffRoll });
            // metres covered in a second, in the buffer's history and past the report
            let (a, b) = (at(T0 - 7_000.0), at(T0 - 6_000.0));
            let d1 = metres(&a, &b);
            let (a, b) = (at(T0 - 2_000.0), at(T0 - 1_000.0));
            let d2 = metres(&a, &b);
            let (a, b) = (at(T0 + 2_000.0), at(T0 + 3_000.0));
            let d3 = metres(&a, &b);
            let step = accel.abs() * KT * 4.0; // 4 s of the speed change
            if accel < 0.0 {
                assert!(d1 > d2 + 0.8 * step, "{d1} {d2}");
                assert!(d2 > d3 + 0.5 * step, "{d2} {d3}");
            } else {
                assert!(d2 > d1 + 0.8 * step, "{d1} {d2}");
                assert!(d3 > d2 + 0.5 * step, "{d2} {d3}");
            }
        }
    }

    #[test]
    fn the_buffer_is_history_it_tells_which_way_a_turn_goes() {
        // first sighted heading north, the buffer holding the 8 s of turn before it
        for rate in [2.0, -2.0] {
            let mut m = Motion::new();
            let f = LiveFlight { speed: 250, alt: 20_000, track: 0, ..path_flight(circle(250.0, rate), T0, &BUFFER) };
            pose(&mut m, &f, T0 + 2_000.0);
            // in the history, the heading led up to north...
            let track = pose_at(&mut m, &f, T0 - 4_000.0).track;
            assert!((signed(track) + 4.0 * rate).abs() < 1.0, "{rate}: {track}");
            // ...and past the report it carries on round, banked into the turn
            let later = pose_at(&mut m, &f, T0 + 10_000.0);
            assert!((signed(later.track) - 10.0 * rate).abs() < 1.5, "{rate}: {later:?}");
            assert_eq!(sign0(later.bank), sign0(rate));
            assert!(later.bank.abs() > 15.0, "{rate}: {later:?}");
        }
    }

    #[test]
    fn sparse_reports_on_short_final_the_descent_carries_on_into_a_flare_not_a_drop_at_the_ground_report() {
        let mut m = Motion::new();
        m.set_airports(&[fld(), xxx()]);
        // a report every 15 s, the last airborne one 30 s (~370 ft) out
        let frames = play(&mut m, &arrival(0.0, 15.0, 12.0, 1, 90.0, FIELD_FT), 3_000.0, 10_000.0);
        assert!(worst_step(&frames) < MAX_FRAME_FT, "{}", worst_step(&frames));
        assert!(frames.iter().all(|f| f.alt_ft >= 0.0));
        let down = first_down(&frames, T0 + 60_000.0);
        assert!((down - (T0 + 120_000.0)).abs() < 6_000.0, "{}", down - T0);
    }

    fn track_with_one_sample() -> Track {
        let mut tr = Track::new(1);
        tr.n = 1;
        tr.ts = vec![T0];
        tr.report_ms = T0;
        tr.mx = vec![0.0];
        tr.my = vec![100.0];
        tr.speed_kt = 100.0 / KNOTS_TO_MS;
        tr
    }

    #[test]
    fn predictions_by_phase() {
        // a takeoff roll at 120 kt: 2.5 kt/s to 165 kt, lifting off at rotation (25 kt to go: 10 s)
        let mut tr = track_with_one_sample();
        tr.my = vec![120.0 * KNOTS_TO_MS];
        tr.speed_kt = 120.0;
        tr.phase = Phase::TakeoffRoll;
        predict(&mut tr);
        assert_eq!(tr.liftoff_ms, T0 + 10_000.0);
        assert_eq!(tr.seg_a, [TAKEOFF_ACCEL_KTS * KNOTS_TO_MS]);
        assert!((tr.prof_end_v - 165.0 * KNOTS_TO_MS).abs() < 1e-9);
        assert!(tr.touchdown_ms.is_nan());
        // a helicopter does not roll
        tr.cls = AircraftClass::Heli;
        predict(&mut tr);
        assert!(tr.liftoff_ms.is_nan() && tr.seg_a.is_empty());
        // parked: nothing moves, the reported track is the heading
        let mut tr = track_with_one_sample();
        tr.phase = Phase::Parked;
        tr.track_deg = 123.0;
        predict(&mut tr);
        assert_eq!((tr.v0, tr.kappa, tr.h0), (0.0, 0.0, 123.0 * DEG));
        assert_eq!(tr.s_turn_max, 0.0);
        // a landing roll brakes at 2 kt/s at least, to 20 kt
        let mut tr = track_with_one_sample();
        tr.phase = Phase::LandingRoll;
        tr.accel = -1.0;
        predict(&mut tr);
        assert_eq!(tr.seg_a, [-LANDING_DECEL_KTS * KNOTS_TO_MS]);
        assert!((tr.prof_end_v - ROLLOUT_KT * KNOTS_TO_MS).abs() < 1e-9);
        // a turn's curvature, capped at 1/30 per metre
        let mut tr = track_with_one_sample();
        tr.turn_rate = 3.0;
        predict(&mut tr);
        assert_eq!(tr.kappa, (3.0 * DEG) / 100.0);
        assert_eq!(tr.h0, 0.0);
        assert_eq!(tr.turn_end_s, TURN_HORIZON_S);
        assert_eq!(tr.s_turn_max, 6_000.0);
    }

    #[test]
    fn a_descent_on_approach_predicts_its_touchdown() {
        let mut tr = track_with_one_sample();
        tr.phase = Phase::Approach;
        tr.field = Some(fld());
        tr.field_elev = 1_000.0;
        tr.alt = 1_500.0;
        tr.vrate = -600.0;
        predict(&mut tr);
        // (500 + 33.3) ft at 10 ft/s: 53.3 s, then held to touchdown and braking to 20 kt
        let t = (500.0 - FLARE_FLOOR_FT) / 10.0;
        assert!((tr.touchdown_ms - (T0 + t * 1000.0)).abs() < 1e-6, "{}", tr.touchdown_ms - T0);
        assert_eq!(tr.seg_a.len(), 3);
        assert_eq!(tr.seg_a[2], -LANDING_DECEL_KTS * KNOTS_TO_MS);
        assert!((tr.seg_dur[0] + tr.seg_dur[1] - t).abs() < 1e-6);
        assert!(tr.touchdown_pitch > 0.0);
        // climbing: no touchdown
        tr.vrate = 100.0;
        predict(&mut tr);
        assert!(tr.touchdown_ms.is_nan());
    }

    #[test]
    fn the_selected_heading_ends_the_turn_and_the_selected_altitude_levels_off() {
        let mut tr = track_with_one_sample();
        tr.turn_rate = 3.0;
        tr.nav_track = 30.0;
        predict(&mut tr);
        // 30 degrees at 3 deg/s: 10 s, 1 km
        assert!((tr.turn_end_s - 10.0).abs() < 1e-9 && (tr.s_turn_max - 1_000.0).abs() < 1e-6);
        // the other way round: the turn carries on
        tr.nav_track = 330.0;
        predict(&mut tr);
        assert_eq!(tr.turn_end_s, TURN_HORIZON_S);
        // a level-off ahead of a climb, from fresh measured data
        tr.alt = 10_000.0;
        tr.vrate = 1_500.0;
        tr.phase = Phase::Climb;
        tr.meas =
            Some(Measured { nav_altitude_mcp: Some(12_000.0), nav_altitude_fms: Some(14_000.0), ..Measured::default() });
        tr.meas_ms = T0;
        assert_eq!(capture_altitude(&tr), 12_000.0);
        // one behind the climb is no level-off; nor on approach, nor once stale
        tr.meas.as_mut().unwrap().nav_altitude_mcp = Some(8_000.0);
        assert_eq!(capture_altitude(&tr), 14_000.0);
        tr.phase = Phase::Approach;
        assert!(capture_altitude(&tr).is_nan());
        tr.phase = Phase::Climb;
        tr.meas_ms = T0 - 16_000.0;
        assert!(capture_altitude(&tr).is_nan());
        tr.meas_ms = T0;
        tr.vrate = 50.0;
        assert!(capture_altitude(&tr).is_nan());
    }
}
