//! The landing gear (`motion.ts` `gear`, `gearAt`, `:2219-2250`, `:2336-2341`; doc 05 §10.13), and
//! [`Motion::pose`] (`:1079-1121`), which ends with it.
//!
//! The gear is down on the ground; with a field, on approach or in the flare below 2000 ft above
//! it, on the initial climb below 400 ft, or (whatever the phase says) below 400 ft and slower
//! than 180 kt; without a field, below 1500 ft, slower than 180 kt and sinking more than 200 fpm.
//! A new track starts with its gear where it should be; a change eases over 8 s.

use super::consts::*;
use super::track::{Raw, Track};
use super::{Motion, Phase, Pose, clamp, smoothstep};
use crate::model::LiveFlight;

/// Gear fraction (0 up .. 1 down) at `now_ms`, given the last target change.
pub fn gear_at(tr: &Track, now_ms: f64) -> f64 {
    let t = (now_ms - tr.gear_ms) / GEAR_TRANSIT_MS;
    if t >= 1.0 {
        return tr.gear_target;
    }
    if t <= 0.0 {
        return tr.gear_from;
    }
    tr.gear_from + (tr.gear_target - tr.gear_from) * smoothstep(t)
}

/// Gear fraction for this pose, animating towards its target: what is shown at `now_ms`, a
/// change of target starting a new 8 s transit from there.
pub fn gear(tr: &mut Track, raw: &Raw, now_ms: f64) -> f64 {
    let phase = raw.phase;
    let down = if raw.ground || phase.is_ground() {
        true
    } else if raw.has_field {
        (matches!(phase, Phase::Approach | Phase::Flare) && raw.agl < GEAR_APPROACH_AGL_FT)
            || (phase == Phase::InitialClimb && raw.agl < GEAR_CLIMB_AGL_FT)
            // whatever the phase (no altitude to tell one): just above the field and slow
            || (raw.agl < GEAR_CLIMB_AGL_FT && raw.speed_ms < GEAR_FALLBACK_KT * KNOTS_TO_MS)
    } else {
        raw.alt_msl < GEAR_FALLBACK_ALT_FT
            && raw.speed_ms < GEAR_FALLBACK_KT * KNOTS_TO_MS
            && raw.vs_fpm < APPROACH_MAX_VRATE_FPM
    };
    let target = if down { 1.0 } else { 0.0 };
    if tr.gear_target < 0.0 {
        // a new track starts with its gear where it should be
        tr.gear_target = target;
        tr.gear_from = target;
        tr.gear_ms = f64::NEG_INFINITY;
        return target;
    }
    let current = gear_at(tr, now_ms);
    if target != tr.gear_target {
        tr.gear_from = current;
        tr.gear_target = target;
        tr.gear_ms = now_ms;
    }
    current
}

impl Motion {
    /// The pose to draw this aircraft at, at `now_ms` (server-synced, as [`Motion::now`] gives).
    /// Lazily ingests `f` as a new report when it is newer than what the track holds
    /// (`timestamp_ms` larger), so callers just pass whatever `LiveFlight` they have. Cheap: it
    /// is called per visible aircraft per frame, and allocates nothing once a track has grown.
    ///
    /// [`Motion::pose_raw`] draws the model at the drawn time; then a slow aircraft on the
    /// ground is drawn on its airport's taxiway centrelines (when any network is loaded, or the
    /// snap is still fading out), pitch and bank are clamped, and the gear animated.
    pub fn pose(&mut self, f: &LiveFlight, now_ms: f64) -> Pose {
        let (mut raw, t) = self.pose_raw(f, now_ms);
        let Motion { tracks, world } = self;
        let tr = tracks.get_mut(&f.id).expect("pose_raw keeps the track it drew");
        if !world.taxiways.is_empty() || tr.snap_gate > 0.0 {
            world.taxi_snap(tr, &mut raw, now_ms, t);
        }
        Pose {
            lon: raw.lon,
            lat: raw.lat,
            alt_ft: raw.draw_alt,
            heading: raw.heading,
            track: raw.track,
            pitch: clamp(raw.pitch, MIN_PITCH, MAX_PITCH),
            bank: clamp(raw.bank, -MAX_BANK, MAX_BANK),
            gear: gear(tr, &raw, now_ms),
            phase: raw.phase,
        }
    }
}

/// What the ported `motion.test.ts` cases of S1.20 share: the helpers of
/// [`blend::testing`](super::blend::testing) (re-exported here, but for its gear-less `pose`,
/// `pose_at` and `play`), and the TypeScript's `poseAt` and `play` with the full
/// [`Motion::pose`] (gear and taxi snap included), its `Frame` with the gear, `lastUp` and
/// `departure`.
#[cfg(test)]
pub(crate) mod testing {
    pub use super::super::blend::testing::{
        BUFFER, FIELD_FT, KT, MAX_FRAME_FT, T0, airport_at, arrival, circle, east, fixture, fixture_airports,
        fixture_reports, fld, flight, flight_at, flight_with_history, landings, metres, offset, path_flight, seen, signed,
        truth_at, xxx,
    };
    use super::super::{Motion, Pose};
    use crate::model::LiveFlight;
    use crate::num::js_round;
    use crate::trains::js_max;

    /// `poseAt`: the pose `f` is drawn with at the moment its drawn time is `t` (a few
    /// fixed-point steps; the delay changes slowly). `f` must have been taken in already.
    pub fn pose_at(m: &mut Motion, f: &LiveFlight, t: f64) -> Pose {
        let mut now = t;
        for _ in 0..8 {
            now += t - m.drawn_time(f.id, now);
        }
        m.pose(f, now)
    }

    /// What the drawn aircraft did, frame by frame (16 ms), while reports arrived `latency` ms
    /// after their time (the TypeScript's also held the phase, which no case reads).
    #[derive(Debug, Clone, Copy)]
    pub struct Frame {
        pub drawn: f64,
        pub alt_ft: f64,
        pub gear: f64,
    }

    /// `play(m, reports, latency = 3000, after = 10000)`.
    pub fn play(m: &mut Motion, reports: &[LiveFlight], latency: f64, after: f64) -> Vec<Frame> {
        let mut frames = Vec::new();
        let mut k = 0;
        let end = reports[reports.len() - 1].timestamp_ms as f64 + latency + after;
        let mut now = reports[0].timestamp_ms as f64 + latency;
        while now <= end {
            while k + 1 < reports.len() && reports[k + 1].timestamp_ms as f64 + latency <= now {
                k += 1;
            }
            let p = m.pose(&reports[k], now);
            frames.push(Frame { drawn: m.drawn_time(reports[k].id, now), alt_ft: p.alt_ft, gear: p.gear });
            now += 16.0;
        }
        frames
    }

    /// Largest change of the drawn height between frames, ft.
    pub fn worst_step(frames: &[Frame]) -> f64 {
        frames.windows(2).fold(0.0, |w, p| js_max(w, (p[1].alt_ft - p[0].alt_ft).abs()))
    }

    /// Drawn time at which the aircraft first comes within a foot of the ground after `from`
    /// (landing), NaN if never.
    pub fn first_down(frames: &[Frame], from: f64) -> f64 {
        frames.iter().find(|f| f.drawn > from && f.alt_ft < 1.0).map_or(f64::NAN, |f| f.drawn)
    }

    /// Drawn time at which the aircraft last leaves the ground (take-off), NaN if never.
    pub fn last_up(frames: &[Frame]) -> f64 {
        for i in (1..frames.len()).rev() {
            if frames[i - 1].alt_ft < 1.0 && frames[i].alt_ft >= 1.0 {
                return frames[i].drawn;
            }
        }
        f64::NAN
    }

    /// `departure(biasFt, id = 2)`: a departure from the synthetic field (`FIELD_FT` up) at 45N
    /// 10E: rolls north from T0 at 2.5 kt/s, airborne-flagged from 90 kt, lifts off at 150 kt
    /// (60 s), climbs at 2000 fpm; `alt` reads `bias_ft` high, in 25 ft steps. A report every
    /// 5 s with the 8 s before it in its buffer.
    ///
    /// Speeds are whole knots in `LiveFlight`, and 2.5 kt/s gives a half every other report:
    /// those are truncated. The take-off case is within 0.2 s of its bound here (the TypeScript
    /// lifts off at 58.16 s with its exact speeds, and the case asks for nothing before 58 s);
    /// fed the same whole knots the TypeScript gives the same poses as this port, lifting off at
    /// 58.35 s with truncated speeds, and at 57.96 s (0.1 ft up at 57.82 s) with rounded ones.
    /// (Checked with bun.)
    pub fn departure(bias_ft: f64, id: u32) -> Vec<LiveFlight> {
        let lo_s = 60.0;
        let north = move |s: f64| {
            if s <= 0.0 {
                0.0
            } else if s <= lo_s {
                (2.5 * KT * s * s) / 2.0
            } else {
                (2.5 * KT * lo_s * lo_s) / 2.0 + 150.0 * KT * (s - lo_s)
            }
        };
        let mut reports = Vec::new();
        let mut s = 0.0;
        while s <= 110.0 {
            let speed = if s <= lo_s { 2.5 * s } else { 150.0 };
            let (lat, lon) = offset(45.0, 10.0, 0.0, north(s));
            let f = flight_with_history(lat, lon, |d| offset(45.0, 10.0, 0.0, north(s - d)), &BUFFER);
            let climb = js_max(0.0, ((s - lo_s) * 2000.0) / 60.0);
            let on_ground = speed < 90.0;
            let alt = if on_ground { 0.0 } else { js_max(0.0, js_round((FIELD_FT + climb + bias_ft) / 25.0) * 25.0) };
            reports.push(LiveFlight {
                id,
                alt: alt as i32,
                speed: speed.floor() as i32,
                track: 0,
                on_ground,
                timestamp_ms: (T0 + s * 1000.0) as u64,
                origin: "FLD".into(),
                destination: "XXX".into(),
                ..f
            });
            s += 5.0;
        }
        reports
    }
}

#[cfg(test)]
mod tests {
    use super::super::track::Raw;
    use super::testing::*;
    use super::*;
    use crate::assert_close;
    use crate::trains::js_min;

    fn raw(phase: Phase, ground: bool, has_field: bool, agl: f64, alt_msl: f64, speed_kt: f64, vs_fpm: f64) -> Raw {
        Raw { phase, ground, has_field, agl, alt_msl, speed_ms: speed_kt * KNOTS_TO_MS, vs_fpm, ..Raw::default() }
    }

    #[test]
    fn the_gear_is_down_on_the_ground_near_the_field_and_low_and_slow() {
        let target = |r: Raw| {
            let mut tr = Track::new(1);
            gear(&mut tr, &r, 0.0)
        };
        // the ground, by flag or phase
        assert_eq!(target(raw(Phase::Cruise, true, false, f64::NAN, 30_000.0, 450.0, 0.0)), 1.0);
        assert_eq!(target(raw(Phase::TakeoffRoll, false, false, f64::NAN, 30_000.0, 450.0, 0.0)), 1.0);
        // with a field: approach or flare below 2000 ft, initial climb below 400 ft
        assert_eq!(target(raw(Phase::Approach, false, true, 1_999.0, 0.0, 250.0, 0.0)), 1.0);
        assert_eq!(target(raw(Phase::Approach, false, true, 2_000.0, 0.0, 150.0, 0.0)), 0.0);
        assert_eq!(target(raw(Phase::Flare, false, true, 30.0, 0.0, 250.0, 0.0)), 1.0);
        assert_eq!(target(raw(Phase::InitialClimb, false, true, 399.0, 0.0, 250.0, 0.0)), 1.0);
        assert_eq!(target(raw(Phase::InitialClimb, false, true, 401.0, 0.0, 150.0, 0.0)), 0.0);
        // whatever the phase, below 400 ft and slower than 180 kt; an unknown height is not low
        assert_eq!(target(raw(Phase::Cruise, false, true, 399.0, 0.0, 179.0, 0.0)), 1.0);
        assert_eq!(target(raw(Phase::Cruise, false, true, 399.0, 0.0, 181.0, 0.0)), 0.0);
        assert_eq!(target(raw(Phase::Approach, false, true, f64::NAN, 0.0, 120.0, 0.0)), 0.0);
        // without a field: below 1500 ft, slower than 180 kt and sinking more than 200 fpm
        assert_eq!(target(raw(Phase::Descent, false, false, f64::NAN, 1_400.0, 170.0, -300.0)), 1.0);
        assert_eq!(target(raw(Phase::Descent, false, false, f64::NAN, 1_400.0, 170.0, -100.0)), 0.0);
        assert_eq!(target(raw(Phase::Descent, false, false, f64::NAN, 1_600.0, 170.0, -300.0)), 0.0);
        assert_eq!(target(raw(Phase::Descent, false, false, f64::NAN, 1_400.0, 190.0, -300.0)), 0.0);
    }

    #[test]
    fn a_new_track_starts_at_its_target_and_a_change_eases_over_eight_seconds() {
        let up = raw(Phase::Cruise, false, false, f64::NAN, 30_000.0, 450.0, 0.0);
        let down = raw(Phase::Taxi, true, true, 0.0, 0.0, 10.0, 0.0);
        let mut tr = Track::new(1);
        assert_eq!(gear(&mut tr, &up, 1_000.0), 0.0);
        assert_eq!((tr.gear_target, tr.gear_from, tr.gear_ms), (0.0, 0.0, f64::NEG_INFINITY));
        assert_eq!(gear(&mut tr, &up, 2_000.0), 0.0);
        // the change shows from the next frame on
        assert_eq!(gear(&mut tr, &down, 10_000.0), 0.0);
        assert_eq!((tr.gear_target, tr.gear_from, tr.gear_ms), (1.0, 0.0, 10_000.0));
        assert_eq!(gear(&mut tr, &down, 14_000.0), 0.5);
        assert_eq!(gear_at(&tr, 12_000.0), 0.15625);
        assert_eq!(gear(&mut tr, &down, 18_000.0), 1.0);
        // reversed half-way: from where it was
        let mut tr = Track::new(1);
        gear(&mut tr, &up, 0.0);
        gear(&mut tr, &down, 0.0);
        assert_eq!(gear(&mut tr, &up, 4_000.0), 0.5);
        assert_eq!((tr.gear_target, tr.gear_from, tr.gear_ms), (0.0, 0.5, 4_000.0));
        assert_eq!(gear_at(&tr, 8_000.0), 0.25);
        assert_eq!(gear_at(&tr, 3_000.0), 0.5);
        assert_eq!(gear_at(&tr, 12_000.0), 0.0);
    }

    #[test]
    fn taxiing_aircraft_move_parked_ones_dont() {
        let mut m = Motion::new();
        let taxi = LiveFlight { on_ground: true, speed: 15, alt: 0, track: 270, ..flight() };
        let p = m.pose(&taxi, T0 + 10_000.0);
        let q = m.pose(&taxi, T0 + 11_000.0);
        assert_eq!(p.phase, Phase::Taxi);
        assert!(metres(&p, &q) > 15.0 * KT * 0.9, "{}", metres(&p, &q));
        assert_eq!(p.alt_ft, 0.0);
        assert_close!(p.heading, 270.0, 1);

        let parked = LiveFlight { id: 2, on_ground: true, speed: 2, alt: 0, track: 123, ..flight() };
        let r = m.pose(&parked, T0 + 10_000.0);
        let s = m.pose(&parked, T0 + 60_000.0);
        assert_eq!(r.phase, Phase::Parked);
        assert!(metres(&r, &s) < 0.01, "{}", metres(&r, &s));
        assert_close!(r.heading, 123.0, 3);
        assert_eq!(r.gear, 1.0);
    }

    #[test]
    fn an_approach_to_a_high_field_touches_down_and_rolls_out() {
        let mut m = Motion::new();
        let den = airport_at("DEN", 39.8617, -104.6731, 5_434);
        m.set_airports(&[den.clone(), airport_at("ORD", 41.97, -87.9, 672)]);
        let (lat, lon) = (den.lat - 14.0 / 111.2, den.lon);
        let f = LiveFlight {
            track: 0,
            speed: 140,
            alt: 5_434 + 800,
            vspeed: Some(-700),
            origin: "ORD".into(),
            destination: "DEN".into(),
            ..flight_at(lat, lon)
        };
        m.pose(&f, T0);
        let mut at = |t: f64| pose_at(&mut m, &f, t);
        let first = at(T0);
        assert_eq!(first.phase, Phase::Approach);
        assert_eq!(first.gear, 1.0);
        assert_close!(first.alt_ft, 800.0, 0);
        let mut landed = f64::NAN;
        let mut last = first;
        let mut t = T0;
        while t <= T0 + 120_000.0 {
            let p = at(t);
            assert!(p.alt_ft >= 0.0, "{t}: {}", p.alt_ft);
            assert!(p.alt_ft <= last.alt_ft + 1e-9, "{t}: {} after {}", p.alt_ft, last.alt_ft); // never climbs back up
            if p.alt_ft < 200.0 {
                assert_eq!(p.gear, 1.0, "{t}");
            }
            if p.alt_ft == 0.0 && landed.is_nan() {
                landed = t;
            }
            last = p;
            t += 250.0;
        }
        // touchdown predicted ~70 s out (the descent levels off past 60 s)
        assert!(landed > T0 + 60_000.0 && landed < T0 + 80_000.0, "{}", landed - T0);
        let roll = at(landed + 10_000.0);
        assert_eq!(roll.phase, Phase::LandingRoll);
        assert_eq!(roll.bank, 0.0);
        assert_close!(at(landed + 6_000.0).pitch, 0.0, 3);
        // decelerating: less ground covered each second
        let d1 = metres(&at(landed + 2_000.0), &at(landed + 3_000.0));
        let d2 = metres(&at(landed + 20_000.0), &at(landed + 21_000.0));
        assert!(d2 < d1 - 10.0, "{d1} {d2}");
        // a report from the runway confirms it without lifting the aircraft off the ground
        let on_runway = LiveFlight {
            alt: 0,
            speed: 110,
            on_ground: true,
            vspeed: Some(0),
            timestamp_ms: (landed + 5_000.0) as u64,
            ..LiveFlight { lat: (den.lat - 4.0 / 111.2) as f32, ..f.clone() }
        };
        m.pose(&on_runway, landed + 6_000.0);
        let p = pose_at(&mut m, &on_runway, landed + 7_000.0);
        assert_eq!(p.phase, Phase::LandingRoll);
        assert_eq!(p.alt_ft, 0.0);
    }

    #[test]
    fn phases_follow_the_drawn_time_not_the_newest_report() {
        let mut m = Motion::new();
        m.set_airports(&[airport_at("AAA", 45.0, 10.0, 100)]);
        let roll = LiveFlight { origin: "AAA".into(), on_ground: true, speed: 60, alt: 0, track: 0, ..flight() };
        m.pose(&roll, T0);
        // airborne at T0 + 10 s, taken in at once: the drawn aircraft, seconds behind, is still rolling
        let (lat, lon) = offset(45.0, 10.0, 0.0, 1_000.0);
        let air = LiveFlight {
            origin: "AAA".into(),
            on_ground: false,
            speed: 150,
            alt: 400,
            track: 0,
            timestamp_ms: (T0 + 10_000.0) as u64,
            ..flight_at(lat, lon)
        };
        m.pose(&air, T0 + 10_000.0);
        assert!(m.drawn_time(1, T0 + 10_000.0) < T0 + 10_000.0);
        let before = pose_at(&mut m, &air, T0 + 9_000.0);
        assert_eq!(before.phase, Phase::TakeoffRoll);
        assert_eq!(before.alt_ft, 0.0);
        assert_eq!(before.gear, 1.0);
        let after = pose_at(&mut m, &air, T0 + 11_000.0);
        assert_eq!(after.phase, Phase::InitialClimb);
        assert!(after.alt_ft > 250.0, "{}", after.alt_ft);
    }

    #[test]
    fn a_takeoff_roll_lifts_off_by_itself() {
        let mut m = Motion::new();
        m.set_airports(&[airport_at("AAA", 45.0, 10.0, 100)]);
        let f = LiveFlight { origin: "AAA".into(), on_ground: true, speed: 120, alt: 0, ..flight() };
        m.pose(&f, T0);
        let mut at = |t: f64| pose_at(&mut m, &f, t);
        assert_eq!(at(T0).phase, Phase::TakeoffRoll);
        // 25 kt to go at 2.5 kt/s: rotation starts 3 s before lift-off at 10 s
        assert_eq!(at(T0 + 6_000.0).pitch, 0.0);
        let rotating = at(T0 + 9_000.0).pitch;
        assert!(rotating > 0.0, "{rotating}");
        assert_eq!(at(T0 + 9_900.0).alt_ft, 0.0);
        let airborne = at(T0 + 20_000.0);
        assert_eq!(airborne.phase, Phase::InitialClimb);
        // 10 s at 1800 fpm, less the ~33 ft the lift-off's curve (the flare's, mirrored) takes
        assert_close!(airborne.alt_ft, 300.0 - 100.0 / 3.0, 0);
        assert!(airborne.pitch > rotating, "{} {rotating}", airborne.pitch);
        assert_eq!(airborne.gear, 1.0); // still below 400 ft
    }

    #[test]
    fn gear_animates_when_the_target_changes() {
        let mut m = Motion::new();
        m.set_airports(&[airport_at("BBB", 45.0, 10.0, 0)]);
        let (lat, lon) = offset(45.0, 10.0, 0.0, -20_000.0);
        // 2500 ft on approach at 1000 fpm: gear goes down passing 2000 ft, 30 s on (drawn time)
        let f = LiveFlight { destination: "BBB".into(), speed: 150, alt: 2_500, vspeed: Some(-1_000), ..flight_at(lat, lon) };
        m.pose(&f, T0);
        assert_eq!(pose_at(&mut m, &f, T0).gear, 0.0);
        let mut t = T0;
        while t <= T0 + 31_000.0 {
            pose_at(&mut m, &f, t);
            t += 1_000.0;
        }
        let moving = pose_at(&mut m, &f, T0 + 32_000.0).gear;
        assert!(moving > 0.0 && moving < 1.0, "{moving}");
        assert_eq!(pose_at(&mut m, &f, T0 + 40_000.0).gear, 1.0);
    }

    #[test]
    fn real_touchdowns_and_lift_offs_are_smooth_no_jump_at_the_ground_flag_gear_down_never_below_ground() {
        let l = landings();
        let airports = fixture_airports(&l);
        let events: serde_json::Value = serde_json::from_str(include_str!("../../../tests/data/landings.json")).unwrap();
        for (i, f) in l.flights.iter().enumerate() {
            let event = events["flights"][i]["event"].as_str().unwrap();
            let mut m = Motion::new();
            m.set_airports(&airports);
            let frames = play(&mut m, &fixture_reports(f), 3_000.0, 10_000.0);
            // before this change: up to 5.7 ft a frame (21,000 fpm) at Schiphol, 13 ft at Denver
            assert!(worst_step(&frames) < MAX_FRAME_FT, "{}: {}", f.callsign, worst_step(&frames));
            for fr in &frames {
                assert!(fr.alt_ft >= 0.0, "{}", f.callsign);
                if fr.alt_ft < 30.0 && (fr.drawn - f.event_ms).abs() < 60_000.0 {
                    assert_eq!(fr.gear, 1.0, "{} at {}", f.callsign, fr.drawn - f.event_ms);
                }
            }
            if event == "land" {
                // down by the ground flag at the latest (it lags), and not long before the runway
                let down = first_down(&frames, f.event_ms - 120_000.0);
                assert!(down > f.event_ms - 45_000.0, "{}: {}", f.callsign, down - f.event_ms);
                assert!(down < f.event_ms + 8_000.0, "{}: {}", f.callsign, down - f.event_ms);
                assert!(frames.iter().filter(|fr| fr.drawn > down + 3_000.0).all(|fr| fr.alt_ft < 1.0), "{}", f.callsign);
            } else {
                // the airborne flag comes during the roll: off the ground once it climbs, not before
                let up = last_up(&frames);
                assert!(up > f.event_ms, "{}: {}", f.callsign, up - f.event_ms);
                assert!(up < f.event_ms + 30_000.0, "{}: {}", f.callsign, up - f.event_ms);
            }
        }
    }

    #[test]
    fn a_pressure_altitude_400_ft_off_either_way_down_on_time_once_the_fields_qnh_is_known() {
        for bias in [400.0, -400.0] {
            let mut m = Motion::new();
            m.set_airports(&[fld(), xxx()]);
            // an earlier departure's take-off roll tells the field's QNH
            let earlier: Vec<LiveFlight> = departure(bias, 2)
                .into_iter()
                .map(|f| LiveFlight { timestamp_ms: f.timestamp_ms - 600_000, ..f })
                .collect();
            play(&mut m, &earlier, 3_000.0, 10_000.0);
            let qnh = m.field_qnh(&fld(), T0);
            assert!((qnh - (1013.25 - bias / 27.7)).abs() < 1.0, "{bias}: {qnh}");
            let frames = play(&mut m, &arrival(bias, 5.0, 12.0, 1, f64::INFINITY, FIELD_FT), 3_000.0, 10_000.0);
            assert!(worst_step(&frames) < MAX_FRAME_FT, "{bias}: {}", worst_step(&frames));
            assert!(frames.iter().fold(f64::INFINITY, |a, f| js_min(a, f.alt_ft)) >= 0.0, "{bias}");
            // at the true height on the approach (to within a 25 ft step and the smoothing's lag)
            let mid = frames.iter().find(|f| f.drawn >= T0 + 60_000.0).unwrap();
            let truth = (60.0 * 140.0 * KT * (3.0 * DEG).tan()) / 0.3048;
            assert!((mid.alt_ft - truth).abs() < 60.0, "{bias}: {} against {truth}", mid.alt_ft);
            // down within a few seconds of the true touchdown (the flare stretches it a little)
            let down = first_down(&frames, T0 + 60_000.0);
            assert!((down - (T0 + 120_000.0)).abs() < 5_000.0, "{bias}: {}", down - T0);
            assert_eq!(frames.iter().find(|f| f.drawn >= down).unwrap().gear, 1.0, "{bias}");
        }
    }

    #[test]
    fn a_take_off_on_the_ground_through_the_airborne_flagged_roll_then_off_it_smoothly_from_0() {
        for bias in [300.0, -300.0] {
            let mut m = Motion::new();
            m.set_airports(&[fld(), xxx()]);
            let frames = play(&mut m, &departure(bias, 2), 3_000.0, 10_000.0);
            assert!(worst_step(&frames) < MAX_FRAME_FT, "{bias}: {}", worst_step(&frames));
            // flagged airborne at 90 kt (36 s) at the biased runway altitude: still rolling
            for f in &frames {
                if f.drawn < T0 + 58_000.0 {
                    assert_eq!(f.alt_ft, 0.0, "{bias} at {}", f.drawn - T0);
                }
            }
            let up = last_up(&frames);
            assert!((up - (T0 + 60_000.0)).abs() < 5_000.0, "{bias}: {}", up - T0);
            // climbing at its true height above the field, not the biased one
            let later = frames.iter().find(|f| f.drawn >= T0 + 90_000.0).unwrap();
            assert!((later.alt_ft - 1_000.0).abs() < 120.0, "{bias}: {}", later.alt_ft);
            assert_eq!(frames.iter().find(|f| f.drawn >= up).unwrap().gear, 1.0, "{bias}");
        }
    }
}
