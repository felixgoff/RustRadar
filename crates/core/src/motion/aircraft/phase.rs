//! Taking in a report and the flight phase (`motion.ts` `update`, `onRunway`, `touchedDown`,
//! `measuredRates`, `decidePhase`, `:1399-1592`, `:1692-1721`, `:3371-3409`; doc 05 §10.5).
//!
//! A report updates the samples, the estimates (acceleration, turn and vertical rates), the
//! QNH and the ground/air decision (Flightradar24's flag lags: an aircraft rolling on the runway
//! is on the ground whatever it says, and its runway reading tells the field's QNH), then the
//! phase, with hysteresis from the previous one.
//!
//! `measuredRates` is here (rather than with the rest of the measured data, S1.20) because
//! `update` calls it for every report: fresh measured rates replace the estimates before the
//! phase is decided.

use super::altitude::{add_altitude, corrected, qnh_from, valid_qnh};
use super::consts::*;
use super::delay::note_arrival;
use super::evaluate::evaluate;
use super::samples::{buffer_acceleration, buffer_turn_rate, merge_report};
use super::track::Track;
use super::{Phase, World, clamp, field_fade, meas_fresh, same_airport, wrap_lon, wrap180, wrap360};
use crate::model::{LiveFlight, f32_as_js};
use crate::trains::js_max;

/// Flight phase for the latest report, with hysteresis from the previous one (`None` on a first
/// sighting).
pub fn decide_phase(tr: &Track, prev: Option<Phase>) -> Phase {
    let speed = tr.speed_kt;
    if tr.on_ground {
        if speed < PARKED_MAX_KT {
            return Phase::Parked;
        }
        if speed < TAXI_MAX_KT || tr.cls == AircraftClass::Heli {
            return Phase::Taxi;
        }
        // the speed change between reports, else the estimate (on a first sighting, the buffer's:
        // the speed change over the history it holds)
        let accel = if tr.report_accel.is_finite() { tr.report_accel } else { tr.accel };
        // a roll goes on as it began: a landing becomes a take-off (touch and go) only on clear
        // acceleration, a take-off a landing (rejected) on clear braking
        if let Some(prev) = prev
            && prev.from_landing()
        {
            return if accel > 2.0 * ROLL_ACCEL_KTS { Phase::TakeoffRoll } else { Phase::LandingRoll };
        }
        if let Some(prev) = prev
            && prev.from_taxi()
        {
            return if accel < -2.0 * ROLL_ACCEL_KTS { Phase::LandingRoll } else { Phase::TakeoffRoll };
        }
        if accel > ROLL_ACCEL_KTS {
            return Phase::TakeoffRoll;
        }
        if accel < -ROLL_ACCEL_KTS {
            return Phase::LandingRoll;
        }
        // first sighting at speed on a runway: at the destination it is landing
        return if tr.field.is_some() && same_airport(tr.field.as_ref(), tr.dest_ap.as_ref()) {
            Phase::LandingRoll
        } else {
            Phase::TakeoffRoll
        };
    }
    let vr = tr.vrate;
    if tr.field.is_some() {
        let agl = tr.alt - tr.field_elev;
        let near = tr.field_km < APPROACH_RANGE_KM;
        if agl < APPROACH_MAX_AGL_FT {
            let was_approach = matches!(prev, Some(Phase::Approach | Phase::Flare));
            if near && (vr < APPROACH_MAX_VRATE_FPM || (was_approach && vr < -APPROACH_MAX_VRATE_FPM)) {
                return if agl < FLARE_MAX_AGL_FT { Phase::Flare } else { Phase::Approach };
            }
            if (near && vr > INITIAL_CLIMB_MIN_VRATE_FPM)
                || prev == Some(Phase::TakeoffRoll)
                || (prev == Some(Phase::InitialClimb) && vr > APPROACH_MAX_VRATE_FPM)
            {
                return Phase::InitialClimb;
            }
        }
    }
    if vr > CLIMB_VRATE_FPM || (matches!(prev, Some(Phase::Climb | Phase::InitialClimb)) && vr > CLIMB_HOLD_VRATE_FPM) {
        return Phase::Climb;
    }
    if vr < -CLIMB_VRATE_FPM || (matches!(prev, Some(Phase::Descent | Phase::Approach)) && vr < -CLIMB_HOLD_VRATE_FPM) {
        return Phase::Descent;
    }
    Phase::Cruise
}

/// Measured turn and vertical rates, airspeed, crab and selected heading replace the estimates
/// while fresh at `t` (`measuredRates`). Nothing without measured data.
pub fn measured_rates(tr: &mut Track, t: f64) {
    let Some(m) = &tr.meas else {
        return;
    };
    let has = |x: Option<f64>| x.filter(|v| v.is_finite());
    let track_rate = has(m.track_rate);
    let roll = has(m.roll);
    let vr = has(m.baro_rate).or(m.geom_rate);
    let nav_heading = has(m.nav_heading);
    let true_heading = has(m.true_heading);
    let mag_heading = has(m.mag_heading);
    let track = has(m.track).unwrap_or(tr.track_deg);
    let tas = has(m.tas);
    // the selected heading steers only when the autopilot isn't following a route (LNAV)
    let lnav = m.nav_modes.as_ref().is_some_and(|modes| modes.iter().any(|mode| mode.to_lowercase() == "lnav"));
    tr.crab = match true_heading {
        Some(heading) if !tr.on_ground => clamp(wrap180(heading - track), -MAX_CRAB, MAX_CRAB),
        _ => 0.0,
    };
    tr.tas_ms = match tas {
        Some(tas) if tas > 0.0 => tas * KNOTS_TO_MS,
        _ => 0.0,
    };
    tr.nav_track = f64::NAN;
    if !meas_fresh(tr, t) || tr.on_ground {
        return;
    }
    let mut turn = f64::NAN;
    if let Some(rate) = track_rate {
        turn = rate;
    } else if let Some(roll) = roll
        && tr.speed_kt > 60.0
    {
        // a coordinated turn: rate = g tan(bank) / speed
        turn = (GRAVITY_MS2 * (clamp(roll, -60.0, 60.0) * DEG).tan()) / (tr.speed_kt * KNOTS_TO_MS) / DEG;
    }
    if turn.is_finite() {
        tr.turn_rate = clamp(turn, -MAX_TURN_DEG_S, MAX_TURN_DEG_S);
    }
    if let Some(vr) = has(vr) {
        tr.vrate = clamp(vr, -MAX_VRATE_FPM, MAX_VRATE_FPM);
        tr.vr_reference(t);
    }
    // it is magnetic, so it needs the variation the two reported headings give
    if let (Some(nav), Some(true_heading), Some(mag)) = (nav_heading, true_heading, mag_heading)
        && !lnav
    {
        let variation = wrap180(true_heading - mag);
        tr.nav_track = wrap360(nav + variation - tr.crab);
    }
}

impl World {
    /// Updates samples, estimates, phase and predictions from a report at `t` (`first`: the
    /// track's first).
    pub fn update(&mut self, tr: &mut Track, f: &LiveFlight, t: f64, first: bool, now_ms: f64) {
        let dt_report = if first { f64::NAN } else { (t - tr.report_ms) / 1000.0 };
        let prev_speed = tr.speed_kt;
        let prev_track = tr.track_deg;
        let prev_phase = if first { None } else { Some(tr.phase) };

        if f.icon != tr.icon || f.typecode != tr.typecode {
            tr.icon.clone_from(&f.icon);
            tr.typecode.clone_from(&f.typecode);
            tr.cls = aircraft_class(&f.icon, &f.typecode);
        }
        tr.fr24_ms = t;
        let f_lat = f32_as_js(f.lat);
        let f_lon = f32_as_js(f.lon);
        if !first && t < tr.report_ms {
            // older than measured state already taken in: only its samples and route are news
            self.select_field(tr, Some(f));
            let lat0 = if f_lat.is_finite() { clamp(f_lat, -90.0, 90.0) } else { 0.0 };
            let lon0 = if f_lon.is_finite() { wrap_lon(f_lon) } else { 0.0 };
            if merge_report(tr, &mut self.scratch.samples, &f.positions, t, lat0, lon0, true).is_some() {
                self.refresh(tr);
                note_arrival(tr, now_ms);
            }
            return;
        }
        // Flightradar24 reports no altitude as 0, and a negative pressure altitude (a field near sea
        // level on a high-pressure day) as 0 too: an airborne 0 is no altitude, carried on from the
        // model instead (before this report changes it)
        let raw_alt = f.alt as f64;
        let alt_known = raw_alt > 0.0;
        let mut est_alt = f64::NAN;
        let mut est_vrate = f64::NAN;
        let mut est_ground = false;
        if !f.on_ground && !alt_known && !first {
            let est = evaluate(tr, t, None);
            est_ground = est.ground;
            est_alt = est.alt_raw;
            est_vrate = est.vs_raw;
        }
        let prev_on_ground = tr.on_ground;
        let prev_raw = tr.raw_alt;
        let level_dt = (t - tr.raw_alt_ms) / 1000.0;

        tr.report_ms = t;
        tr.speed_kt = js_max(0.0, f.speed as f64);
        tr.track_deg = wrap360(f.track as f64);
        tr.lat0 = if f_lat.is_finite() { clamp(f_lat, -90.0, 90.0) } else { 0.0 };
        tr.lon0 = if f_lon.is_finite() { wrap_lon(f_lon) } else { 0.0 };

        self.select_field(tr, Some(f));
        // (a newer report is never dropped)
        let (lat0, lon0) = (tr.lat0, tr.lon0);
        let reported_only = merge_report(tr, &mut self.scratch.samples, &f.positions, t, lat0, lon0, false) == Some(true);
        // this report's own samples: its buffer (the ~10 s before it) up to the report itself
        let own_from = self.scratch.samples.report_from_ms;

        // --- acceleration (kt/s): across reports and along the buffer
        let report_accel = if (MIN_ACCEL_DT_S..=MAX_ACCEL_DT_S).contains(&dt_report) {
            (tr.speed_kt - prev_speed) / dt_report
        } else {
            f64::NAN
        };
        let buffer_accel = buffer_acceleration(tr, own_from);
        let mut accel = if report_accel.is_finite() && buffer_accel.is_finite() {
            (report_accel + buffer_accel) / 2.0
        } else if report_accel.is_finite() {
            report_accel
        } else {
            buffer_accel
        };
        if !accel.is_finite() {
            accel = tr.accel * SMOOTHING; // no news: let it relax
        }
        tr.accel = clamp(if first { accel } else { tr.accel + SMOOTHING * (accel - tr.accel) }, -MAX_ACCEL_KTS, MAX_ACCEL_KTS);
        tr.report_accel = if (2.0..=MAX_ACCEL_DT_S).contains(&dt_report) {
            (tr.speed_kt - prev_speed) / dt_report
        } else {
            f64::NAN
        };

        // --- on the ground or in the air: Flightradar24's flag lags, so an aircraft rolling on the
        // runway, which reads the runway's pressure altitude, is on the ground (and tells its QNH)
        let old_qnh = tr.qnh;
        if !same_airport(tr.field.as_ref(), tr.qnh_own_field.as_ref()) {
            tr.qnh_own = f64::NAN;
        }
        tr.qnh = self.track_qnh(tr, t);
        tr.raw_w = if tr.field.is_some() { field_fade(tr.field_km) } else { 0.0 };
        let mut on_ground = f.on_ground;
        if !on_ground && !first {
            on_ground = self.on_runway(tr, raw_alt, prev_on_ground, prev_phase, prev_raw, level_dt, est_ground);
        } else if on_ground && !prev_on_ground && !first {
            self.touched_down(tr, prev_raw, level_dt, prev_phase);
        }
        tr.on_ground = on_ground;
        self.set_qnh(tr, t, old_qnh);
        tr.raw_alt = if on_ground || alt_known { raw_alt } else { 0.0 };
        tr.raw_alt_ms = t;
        let mut alt_estimated = false;
        if on_ground {
            tr.alt = 0.0;
            tr.ground_ms = t;
        } else if alt_known {
            tr.alt = corrected(tr, raw_alt, tr.raw_w);
            add_altitude(tr, t, raw_alt, tr.raw_w);
        } else {
            // no altitude: carried on from the model (or the field, for a first sighting)
            alt_estimated = true;
            tr.alt = if est_alt.is_finite() {
                est_alt
            } else if tr.field.is_some() {
                tr.field_elev
            } else {
                0.0
            };
        }

        // --- turn rate (deg/s): buffer chord headings when fast enough, else reported tracks
        let report_turn = if (MIN_TURN_DT_S..=MAX_TURN_DT_S).contains(&dt_report) {
            wrap180(tr.track_deg - prev_track) / dt_report
        } else {
            f64::NAN
        };
        let buffer_turn = if tr.on_ground { f64::NAN } else { buffer_turn_rate(tr, own_from) };
        let mut turn = if buffer_turn.is_finite() {
            buffer_turn
        } else if report_turn.is_finite() {
            report_turn
        } else {
            0.0
        };
        if tr.on_ground && tr.speed_kt < PARKED_MAX_KT {
            turn = 0.0;
        }
        tr.turn_rate =
            clamp(if first { turn } else { tr.turn_rate + SMOOTHING * (turn - tr.turn_rate) }, -MAX_TURN_DEG_S, MAX_TURN_DEG_S);

        // --- vertical rate (fpm): reported when signed in, else altitude differences between reports
        if let Some(vspeed) = f.vspeed
            && !(alt_estimated && vspeed == 0)
        {
            tr.vrate = clamp(if on_ground { 0.0 } else { vspeed as f64 }, -MAX_VRATE_FPM, MAX_VRATE_FPM);
            tr.vr_reference(t);
        } else if tr.on_ground {
            tr.vrate = 0.0;
            tr.vr_reference(t);
        } else if alt_estimated {
            // no altitude, no news: the model's own rate carries on (the reference stays the last real altitude)
            if est_vrate.is_finite() {
                tr.vrate = clamp(est_vrate, -MAX_VRATE_FPM, MAX_VRATE_FPM);
            }
        } else {
            let dt = (t - tr.vr_ref_ms) / 1000.0;
            if !(dt >= MIN_VRATE_DT_S) {
                if !tr.vr_ref_ms.is_finite() {
                    tr.vr_reference(t);
                }
            } else {
                // FR24 reports 0 ft on the ground: a lift-off climbs from the field (both re-corrected
                // with the QNH in use now, so a QNH learned in between is no vertical rate)
                let ref_alt = if tr.vr_ref_ground {
                    if tr.field.is_some() { tr.field_elev } else { f64::NAN }
                } else {
                    corrected(tr, tr.vr_ref_raw, tr.vr_ref_w)
                };
                if dt <= MAX_VRATE_DT_S && ref_alt.is_finite() {
                    let vr = ((tr.alt - ref_alt) / dt) * 60.0;
                    let fresh = prev_phase.is_none() || tr.vr_ref_ground;
                    tr.vrate =
                        clamp(if fresh { vr } else { tr.vrate + SMOOTHING * (vr - tr.vrate) }, -MAX_VRATE_FPM, MAX_VRATE_FPM);
                } else if dt > MAX_VRATE_DT_S {
                    tr.vrate = 0.0;
                }
                tr.vr_reference(t);
            }
        }

        measured_rates(tr, t);
        tr.phase = decide_phase(tr, prev_phase);
        self.refit(tr, reported_only);
        note_arrival(tr, now_ms);
    }

    /// Whether an airborne report is really of an aircraft on the ground. Flightradar24's
    /// air/ground flag lags: it flags take-off rolls airborne from ~80-110 kt, and landings on the
    /// ground 10-15 s after touchdown, and meanwhile reports the runway's pressure altitude,
    /// level. So: below flying speed; rolling on after a ground report (below rotation speed, or
    /// level); level and braking after an approach; or, with no altitude, where the model has it
    /// on the ground already. Each runway read this way gives the field's QNH (a landing's only if
    /// it brings the aircraft down: a touchdown never lifts it back up).
    #[allow(clippy::too_many_arguments)]
    pub fn on_runway(
        &mut self,
        tr: &mut Track,
        raw: f64,
        prev_on_ground: bool,
        prev_phase: Option<Phase>,
        prev_raw: f64,
        level_dt: f64,
        est_ground: bool,
    ) -> bool {
        let Some(field) = tr.field.clone() else {
            return false;
        };
        if tr.cls == AircraftClass::Heli || tr.field_km > RUNWAY_FIELD_KM {
            return false;
        }
        if !(raw > 0.0) {
            return est_ground;
        }
        let agl = corrected(tr, raw, tr.raw_w) - tr.field_elev;
        if !(agl.abs() < if valid_qnh(tr.qnh) { RUNWAY_TOL_KNOWN_FT } else { RUNWAY_TOL_UNKNOWN_FT }) {
            return false;
        }
        let speed = tr.speed_kt;
        let level = prev_raw > 0.0 && (raw - prev_raw).abs() <= LEVEL_FT && level_dt <= LEVEL_MAX_DT_S;
        let runway = if speed < min_flying_kt(tr.cls) {
            true
        } else if prev_on_ground {
            speed < rotation_kt(tr.cls) || level
        } else {
            level
                && prev_phase.is_some_and(Phase::from_landing)
                && (tr.report_accel <= -RUNWAY_DECEL_KTS || tr.accel <= -RUNWAY_DECEL_KTS)
        };
        if runway {
            let q = qnh_from(raw, tr.field_elev);
            if valid_qnh(q) {
                self.learn_qnh(&field, q, tr.report_ms);
                if prev_on_ground || agl >= 0.0 {
                    tr.qnh_own = q;
                    tr.qnh_own_field = Some(field);
                }
            }
        }
        runway
    }

    /// A ground report after an approach: its last altitude, moments ago, was the runway's (if it
    /// was near the field).
    pub fn touched_down(&mut self, tr: &Track, prev_raw: f64, level_dt: f64, prev_phase: Option<Phase>) {
        let Some(field) = &tr.field else {
            return;
        };
        if tr.cls == AircraftClass::Heli || tr.field_km > RUNWAY_FIELD_KM || !(prev_raw > 0.0) || level_dt > LEVEL_MAX_DT_S {
            return;
        }
        if prev_phase != Some(Phase::Approach) && prev_phase != Some(Phase::Flare) {
            return;
        }
        let agl = corrected(tr, prev_raw, tr.raw_w) - tr.field_elev;
        if agl.abs() < if valid_qnh(tr.qnh) { RUNWAY_TOL_KNOWN_FT } else { RUNWAY_TOL_UNKNOWN_FT } {
            self.learn_qnh(field, qnh_from(prev_raw, tr.field_elev), tr.report_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::aircraft::Motion;
    use crate::motion::aircraft::blend::testing::*;
    use crate::motion::aircraft::{Measured, Pose};

    /// Reports one after another, each taken in as it happens and looked at once the drawn
    /// time reaches it (`report` of the two phase tests).
    struct Run {
        m: Motion,
        t: f64,
        north: f64,
        origin: &'static str,
        destination: &'static str,
        track: i32,
    }

    impl Run {
        fn report(&mut self, f: LiveFlight) -> Pose {
            let f = LiveFlight {
                origin: self.origin.into(),
                destination: self.destination.into(),
                track: self.track,
                timestamp_ms: self.t as u64,
                ..f
            };
            pose(&mut self.m, &f, self.t);
            pose_at(&mut self.m, &f, self.t)
        }

        /// `report(over, dt)` of "phases through a takeoff": `dt` later, as far north as the
        /// speed took it.
        fn climb_out(&mut self, on_ground: bool, speed: i32, alt: i32, vspeed: Option<i32>, dt: f64) -> Pose {
            self.t += dt;
            self.north += (speed as f64 * KT * dt) / 1000.0;
            let (lat, lon) = offset(45.0, 10.0, 0.0, self.north);
            self.report(LiveFlight { on_ground, speed, alt, vspeed, ..flight_at(lat, lon) })
        }

        /// `report(over, km)` of "phases through a landing": 10 s later, `km` north of 45N 10E.
        fn come_in(&mut self, on_ground: bool, speed: i32, alt: i32, vspeed: i32, km: f64) -> Pose {
            self.t += 10_000.0;
            let (lat, lon) = offset(45.0, 10.0, 0.0, km * 1000.0);
            self.report(LiveFlight { on_ground, speed, alt, vspeed: Some(vspeed), ..flight_at(lat, lon) })
        }
    }

    #[test]
    fn phases_through_a_takeoff() {
        let mut m = Motion::new();
        m.set_airports(&[airport_at("AAA", 45.0, 10.0, 100), airport_at("BBB", 50.0, 20.0, 0)]);
        let mut run = Run { m, t: T0, north: 0.0, origin: "AAA", destination: "BBB", track: 0 };
        assert_eq!(run.climb_out(true, 0, 0, None, 10_000.0).phase, Phase::Parked);
        assert_eq!(run.climb_out(true, 15, 0, None, 10_000.0).phase, Phase::Taxi);
        assert_eq!(run.climb_out(true, 60, 0, None, 10_000.0).phase, Phase::TakeoffRoll);
        assert_eq!(run.climb_out(true, 120, 0, None, 10_000.0).phase, Phase::TakeoffRoll);
        let climbing = run.climb_out(false, 150, 500, None, 10_000.0);
        assert_eq!(climbing.phase, Phase::InitialClimb);
        // drawn above the field, not MSL
        let f = LiveFlight { origin: "AAA".into(), destination: "BBB".into(), timestamp_ms: run.t as u64, ..flight() };
        let later = pose_at(&mut run.m, &f, run.t + 5_000.0).alt_ft;
        assert!(later > 400.0 && later < 800.0, "{later}");
        assert_eq!(run.climb_out(false, 250, 5_100, None, 120_000.0).phase, Phase::Climb);
        assert_eq!(run.climb_out(false, 450, 35_000, Some(0), 600_000.0).phase, Phase::Cruise);
    }

    #[test]
    fn phases_through_a_landing() {
        let mut m = Motion::new();
        m.set_airports(&[airport_at("AAA", 50.0, 20.0, 0), airport_at("BBB", 45.0, 10.0, 300)]);
        let mut run = Run { m, t: T0, north: 0.0, origin: "AAA", destination: "BBB", track: 180 };
        assert_eq!(run.come_in(false, 450, 36_000, 0, 300.0).phase, Phase::Cruise);
        assert_eq!(run.come_in(false, 300, 20_000, -1_500, 120.0).phase, Phase::Descent);
        let approach = run.come_in(false, 160, 2_300, -800, 15.0);
        assert_eq!(approach.phase, Phase::Approach);
        crate::assert_close!(approach.alt_ft, 2_000.0, 0);
        assert_eq!(run.come_in(false, 140, 340, -600, 1.0).phase, Phase::Flare);
        assert_eq!(run.come_in(true, 120, 0, 0, 0.0).phase, Phase::LandingRoll);
        assert_eq!(run.come_in(true, 20, 0, 0, -1.0).phase, Phase::Taxi);
        assert_eq!(run.come_in(true, 0, 0, 0, -1.5).phase, Phase::Parked);
    }

    fn on_ground(speed_kt: f64, report_accel: f64) -> Track {
        let mut tr = Track::new(1);
        tr.on_ground = true;
        tr.speed_kt = speed_kt;
        tr.report_accel = report_accel;
        tr
    }

    #[test]
    fn ground_phases_go_on_as_they_began() {
        assert_eq!(decide_phase(&on_ground(2.0, f64::NAN), None), Phase::Parked);
        assert_eq!(decide_phase(&on_ground(30.0, f64::NAN), None), Phase::Taxi);
        let mut heli = on_ground(80.0, 3.0);
        heli.cls = AircraftClass::Heli;
        assert_eq!(decide_phase(&heli, None), Phase::Taxi);
        // a landing turns into a take-off only on clear acceleration, a take-off into a landing on
        // clear braking
        assert_eq!(decide_phase(&on_ground(80.0, 0.9), Some(Phase::Flare)), Phase::LandingRoll);
        assert_eq!(decide_phase(&on_ground(80.0, 1.1), Some(Phase::Descent)), Phase::TakeoffRoll);
        assert_eq!(decide_phase(&on_ground(80.0, -0.9), Some(Phase::Taxi)), Phase::TakeoffRoll);
        assert_eq!(decide_phase(&on_ground(80.0, -1.1), Some(Phase::TakeoffRoll)), Phase::LandingRoll);
        // from the air otherwise (a climb), half a knot a second either way decides
        assert_eq!(decide_phase(&on_ground(80.0, 0.6), Some(Phase::Climb)), Phase::TakeoffRoll);
        assert_eq!(decide_phase(&on_ground(80.0, -0.6), None), Phase::LandingRoll);
        // without a rate across reports, the estimate
        let mut tr = on_ground(80.0, f64::NAN);
        tr.accel = -0.6;
        assert_eq!(decide_phase(&tr, None), Phase::LandingRoll);
        // first sighted at speed, neither: landing at the destination, else taking off
        let dest = airport_at("DST", 45.0, 10.0, 0);
        let mut tr = on_ground(80.0, 0.0);
        assert_eq!(decide_phase(&tr, None), Phase::TakeoffRoll);
        tr.dest_ap = Some(dest.clone());
        assert_eq!(decide_phase(&tr, None), Phase::TakeoffRoll);
        tr.field = Some(dest);
        assert_eq!(decide_phase(&tr, None), Phase::LandingRoll);
    }

    #[test]
    fn air_phases_with_hysteresis() {
        let mut tr = Track::new(1);
        let air = |tr: &mut Track, vrate: f64, prev: Option<Phase>| {
            tr.vrate = vrate;
            decide_phase(tr, prev)
        };
        assert_eq!(air(&mut tr, 600.0, None), Phase::Climb);
        assert_eq!(air(&mut tr, 300.0, None), Phase::Cruise);
        assert_eq!(air(&mut tr, 300.0, Some(Phase::Climb)), Phase::Climb);
        assert_eq!(air(&mut tr, 300.0, Some(Phase::InitialClimb)), Phase::Climb);
        assert_eq!(air(&mut tr, -600.0, None), Phase::Descent);
        assert_eq!(air(&mut tr, -300.0, Some(Phase::Approach)), Phase::Descent);
        assert_eq!(air(&mut tr, -300.0, Some(Phase::Cruise)), Phase::Cruise);
        // near a field, low: approach, flare, initial climb
        tr.field = Some(airport_at("FLD", 45.0, 10.0, 1_000));
        tr.field_elev = 1_000.0;
        tr.field_km = 10.0;
        tr.alt = 2_000.0;
        assert_eq!(air(&mut tr, -300.0, None), Phase::Approach);
        assert_eq!(air(&mut tr, -100.0, None), Phase::Cruise);
        // (held on approach while not climbing more than 200 fpm)
        assert_eq!(air(&mut tr, 100.0, Some(Phase::Approach)), Phase::Approach);
        tr.alt = 1_050.0;
        assert_eq!(air(&mut tr, -300.0, None), Phase::Flare);
        tr.alt = 2_000.0;
        assert_eq!(air(&mut tr, 300.0, None), Phase::InitialClimb);
        assert_eq!(air(&mut tr, 0.0, Some(Phase::TakeoffRoll)), Phase::InitialClimb);
        assert_eq!(air(&mut tr, -100.0, Some(Phase::InitialClimb)), Phase::InitialClimb);
        assert_eq!(air(&mut tr, -300.0, Some(Phase::InitialClimb)), Phase::Approach);
        // far out (but low), no approach; high, none either
        tr.field_km = 30.0;
        assert_eq!(air(&mut tr, -300.0, None), Phase::Cruise);
        tr.field_km = 10.0;
        tr.alt = 4_500.0;
        assert_eq!(air(&mut tr, -300.0, None), Phase::Cruise);
    }

    #[test]
    fn fresh_measured_rates_replace_the_estimates() {
        let mut tr = Track::new(1);
        // nothing measured: nothing changes
        tr.turn_rate = 1.0;
        tr.crab = 3.0;
        measured_rates(&mut tr, 0.0);
        assert_eq!((tr.turn_rate, tr.crab), (1.0, 3.0));
        tr.track_deg = 90.0;
        tr.speed_kt = 250.0;
        tr.raw_alt = 10_000.0;
        tr.meas = Some(Measured {
            true_heading: Some(98.0),
            mag_heading: Some(96.0),
            tas: Some(270.0),
            roll: Some(25.0),
            geom_rate: Some(1_200.0),
            nav_heading: Some(118.0),
            ..Measured::default()
        });
        tr.meas_ms = 0.0;
        measured_rates(&mut tr, 1_000.0);
        assert_eq!(tr.crab, 8.0);
        assert_eq!(tr.tas_ms, 270.0 * KNOTS_TO_MS);
        // the turn from the roll, the vertical rate from the geometric one, the track the
        // autopilot heads for (true, less the crab)
        let turn = (GRAVITY_MS2 * (25.0 * DEG).tan()) / (250.0 * KNOTS_TO_MS) / DEG;
        assert_eq!(tr.turn_rate, clamp(turn, -MAX_TURN_DEG_S, MAX_TURN_DEG_S));
        assert_eq!((tr.vrate, tr.vr_ref_ms), (1_200.0, 1_000.0));
        assert_eq!(tr.nav_track, 112.0);
        // on LNAV the selected heading doesn't steer; a track rate wins over the roll
        let m = tr.meas.as_mut().unwrap();
        m.nav_modes = Some(vec!["LNAV".into()]);
        m.track_rate = Some(-1.5);
        m.baro_rate = Some(-500.0);
        measured_rates(&mut tr, 2_000.0);
        assert!(tr.nav_track.is_nan());
        assert_eq!((tr.turn_rate, tr.vrate), (-1.5, -500.0));
        // stale: the crab and airspeed are kept, the rates are not touched
        tr.turn_rate = 0.5;
        measured_rates(&mut tr, 16_000.0);
        assert_eq!((tr.turn_rate, tr.crab), (0.5, 8.0));
        // on the ground: no crab
        tr.on_ground = true;
        measured_rates(&mut tr, 2_000.0);
        assert_eq!(tr.crab, 0.0);
    }
}
