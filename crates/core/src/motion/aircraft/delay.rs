//! The report states and the drawing delay of `motion.ts` (`snapshot`, `stateAt`, `delayAt`,
//! `delaySlope`, `drawTime`, `retarget`, `noteArrival`, `:3193-3305`; doc 05 §10.8), and
//! [`Motion::drawn_time`] (`:1123-1132`).
//!
//! Each aircraft is drawn a few seconds in the past (2-12 s), so that it is almost always between
//! known positions. The delay is chosen per aircraft from its data's cadence and how old its
//! newest sample is on arrival, and eases between values (a cubic Hermite ease, C1) at no more
//! than ~10% of real time, so the drawn time never jumps or runs backwards. Each report's state
//! takes over when the drawn time reaches the report, not on arrival.

use super::consts::*;
use super::track::{State, Track};
use super::{Motion, clamp};
use crate::trains::js_max;

/// Keeps the latest report's state (replacing an earlier snapshot of the same report), at most
/// MAX_STATES of them.
pub fn snapshot(tr: &mut Track) {
    let s = &mut tr.states;
    while s.last().is_some_and(|last| last.report_ms >= tr.report_ms) {
        s.pop();
    }
    s.push(State {
        report_ms: tr.report_ms,
        phase: tr.phase,
        track_deg: tr.track_deg,
        alt: tr.alt,
        vrate: tr.vrate,
        field: tr.field.clone(),
        field_elev: tr.field_elev,
        liftoff_ms: tr.liftoff_ms,
        touchdown_ms: tr.touchdown_ms,
        touchdown_pitch: tr.touchdown_pitch,
        cap_alt: tr.cap_alt,
    });
    if s.len() > MAX_STATES {
        s.remove(0);
    }
}

/// Index in `states` of the report state in effect at `t`: the newest one not after it (the
/// oldest kept, before them all).
///
/// # Panics
///
/// Without any state: a track has one from its first report on (the TypeScript would read
/// `undefined` and throw as soon as it used it).
pub fn state_index(tr: &Track, t: f64) -> usize {
    let s = &tr.states;
    let mut i = s.len().checked_sub(1).expect("a track that has taken a report has a state");
    while i > 0 && s[i].report_ms > t {
        i -= 1;
    }
    i
}

/// The report state in effect at `t` (`stateAt`), see [`state_index`].
pub fn state_at(tr: &Track, t: f64) -> &State {
    &tr.states[state_index(tr, t)]
}

/// The drawing delay at `now_ms`, ms: a C1 ease (cubic Hermite) from `d_from` with slope
/// `d_slope` to `d_to`.
pub fn delay_at(tr: &Track, now_ms: f64) -> f64 {
    if !(tr.d_dur > 0.0) {
        return tr.d_to;
    }
    let u = (now_ms - tr.d_ms) / tr.d_dur;
    if u >= 1.0 {
        return tr.d_to;
    }
    if u <= 0.0 {
        return tr.d_from;
    }
    let u2 = u * u;
    let u3 = u2 * u;
    (2.0 * u3 - 3.0 * u2 + 1.0) * tr.d_from + (u3 - 2.0 * u2 + u) * tr.d_dur * tr.d_slope + (3.0 * u2 - 2.0 * u3) * tr.d_to
}

/// The delay's rate of change at `now_ms` (ms per ms): the drawn time runs at 1 minus this.
pub fn delay_slope(tr: &Track, now_ms: f64) -> f64 {
    if !(tr.d_dur > 0.0) {
        return 0.0;
    }
    let u = (now_ms - tr.d_ms) / tr.d_dur;
    if u >= 1.0 {
        return 0.0;
    }
    if u <= 0.0 {
        return tr.d_slope;
    }
    ((6.0 * u * u - 6.0 * u) * (tr.d_from - tr.d_to)) / tr.d_dur + (3.0 * u * u - 4.0 * u + 1.0) * tr.d_slope
}

/// The time a track is drawn at (`drawTime`).
pub fn draw_time(tr: &Track, now_ms: f64) -> f64 {
    now_ms - delay_at(tr, now_ms)
}

/// Eases the delay towards `target` from where it is now, at most ~DELAY_RATE; `first` sets it
/// outright.
pub fn retarget(tr: &mut Track, now_ms: f64, target: f64, first: bool) {
    if first {
        tr.d_from = target;
        tr.d_to = target;
        tr.d_slope = 0.0;
        tr.d_dur = 0.0;
        tr.d_ms = now_ms;
        return;
    }
    if (target - tr.d_to).abs() < DELAY_DEADBAND_MS {
        return;
    }
    let from = delay_at(tr, now_ms);
    tr.d_slope = delay_slope(tr, now_ms);
    tr.d_from = from;
    tr.d_to = target;
    tr.d_ms = now_ms;
    // long enough that the rate stays under DELAY_RATE (a cubic ease peaks at 1.5x its
    // average) and changes gently: |D''| ≤ (6|Δ|/L + 4|s|)/L ≤ DELAY_ACCEL
    let delta = (target - from).abs();
    let s = tr.d_slope.abs();
    let j = DELAY_ACCEL / 1000.0; // per ms
    tr.d_dur = js_max(
        js_max(1_000.0, (1.5 * delta) / DELAY_RATE),
        (4.0 * s + (16.0 * s * s + 24.0 * j * delta).sqrt()) / (2.0 * j),
    );
}

/// Newer data arrived: adapt the delay. The newest sample is the latest report (a buffer only
/// holds the past), already `age` old on arrival. The drawn time should still be a margin (one
/// sample spacing) short of it when the next data is due, one cadence from now, so the delay is
///
/// ```text
/// cadence + margin + age        (age = now - newest sample)
/// ```
///
/// within 2-12 s: right after an arrival the drawn time is a cadence and a margin behind the
/// newest data, and it closes up to the margin as the next comes due. Flightradar24 alone, every
/// ~8 s (the area refresh) with its buffer 2 s apart and reports 0-5 s old, gives 10-15 s: 10-12 s,
/// mostly the cap; every 15 s (the world view), the cap. With adsb.lol for the selected aircraft
/// (every 3 s, ~0.5-1 s old, samples 1-3 s apart): ~5-6 s. The target is smoothed over reports and
/// the delay eases towards it.
pub fn note_arrival(tr: &mut Track, now_ms: f64) {
    let n = tr.n;
    if n == 0 {
        return;
    }
    let newest = tr.ts[n - 1];
    if !(newest > tr.newest_ms) {
        return;
    }
    let first = tr.newest_ms == f64::NEG_INFINITY;
    tr.newest_ms = newest;
    let data_ms = tr.report_ms;
    if tr.arrival_data_ms.is_finite() && data_ms > tr.arrival_data_ms {
        let gap = clamp(data_ms - tr.arrival_data_ms, MIN_CADENCE_MS, MAX_CADENCE_MS);
        tr.cadence_ms += DELAY_SMOOTHING * (gap - tr.cadence_ms);
    }
    tr.arrival_data_ms = js_max(tr.arrival_data_ms, data_ms);
    let k = (n - 1).min(3);
    let margin = clamp(
        if k > 0 { (newest - tr.ts[n - 1 - k]) / k as f64 } else { MAX_MARGIN_MS },
        MIN_MARGIN_MS,
        MAX_MARGIN_MS,
    );
    let target = clamp(tr.cadence_ms + margin - (newest - now_ms), MIN_DELAY_MS, MAX_DELAY_MS);
    tr.delay_target = if first || !tr.delay_target.is_finite() {
        target
    } else {
        tr.delay_target + DELAY_SMOOTHING * (target - tr.delay_target)
    };
    retarget(tr, now_ms, tr.delay_target, first);
}

impl Motion {
    /// The (server) time an aircraft is drawn at: `now_ms` less its delay, which is 2-12 s and
    /// changes smoothly. Trails and anything else that shows where the aircraft has been should
    /// stop at this time, or they run ahead of it. `now_ms` itself for an aircraft not seen yet.
    pub fn drawn_time(&self, id: u32, now_ms: f64) -> f64 {
        match self.tracks.get(&id) {
            Some(tr) if tr.n > 0 => draw_time(tr, now_ms),
            _ => now_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;
    use crate::motion::aircraft::Phase;

    fn state(report_ms: f64) -> State {
        State {
            report_ms,
            phase: Phase::Cruise,
            track_deg: 0.0,
            alt: 0.0,
            vrate: 0.0,
            field: None,
            field_elev: 0.0,
            liftoff_ms: f64::NAN,
            touchdown_ms: f64::NAN,
            touchdown_pitch: 0.0,
            cap_alt: f64::NAN,
        }
    }

    #[test]
    fn snapshots_replace_their_report_and_keep_sixteen() {
        let mut tr = Track::new(1);
        tr.report_ms = 1_000.0;
        tr.phase = Phase::Climb;
        snapshot(&mut tr);
        tr.phase = Phase::Descent;
        snapshot(&mut tr);
        assert_eq!(tr.states.len(), 1);
        assert_eq!(tr.states[0].phase, Phase::Descent);
        // an older report (measured state came first) replaces the newer ones too
        tr.report_ms = 2_000.0;
        snapshot(&mut tr);
        tr.report_ms = 1_500.0;
        snapshot(&mut tr);
        assert_eq!(tr.states.iter().map(|s| s.report_ms).collect::<Vec<_>>(), [1_000.0, 1_500.0]);
        for k in 0..20 {
            tr.report_ms = 3_000.0 + k as f64;
            snapshot(&mut tr);
        }
        assert_eq!(tr.states.len(), 16);
        assert_eq!(tr.states[0].report_ms, 3_004.0);
    }

    #[test]
    fn the_state_at_a_time_is_the_newest_not_after_it() {
        let mut tr = Track::new(1);
        tr.states = vec![state(1_000.0), state(2_000.0), state(3_000.0)];
        assert_eq!(state_index(&tr, 500.0), 0);
        assert_eq!(state_index(&tr, 1_999.0), 0);
        assert_eq!(state_index(&tr, 2_000.0), 1);
        assert_eq!(state_at(&tr, 9_000.0).report_ms, 3_000.0);
        assert_eq!(state_index(&tr, f64::NAN), 2);
    }

    #[test]
    fn the_delay_eases_c1_and_slowly() {
        let mut tr = Track::new(1);
        retarget(&mut tr, 0.0, 5_000.0, true);
        assert_eq!((delay_at(&tr, 0.0), delay_slope(&tr, 0.0), draw_time(&tr, 10_000.0)), (5_000.0, 0.0, 5_000.0));
        // under the dead band: nothing
        retarget(&mut tr, 1_000.0, 5_400.0, false);
        assert_eq!(tr.d_to, 5_000.0);
        // 5 s more: 1.5 * 5000 / 0.1 = 75 s, and (sqrt(24 j Δ)) / 2j = 31.6 s
        retarget(&mut tr, 1_000.0, 10_000.0, false);
        assert_eq!(tr.d_dur, 75_000.0);
        assert_eq!(delay_at(&tr, 1_000.0), 5_000.0);
        assert_eq!(delay_at(&tr, 1_000.0 + 37_500.0), 7_500.0);
        assert_eq!(delay_at(&tr, 80_000.0), 10_000.0);
        // the slope peaks at 1.5 times the mean rate, and is the derivative of the delay
        assert_close!(delay_slope(&tr, 1_000.0 + 37_500.0), 0.1, 12);
        let d = (delay_at(&tr, 20_001.0) - delay_at(&tr, 19_999.0)) / 2.0;
        assert_close!(delay_slope(&tr, 20_000.0), d, 9);
        // retargeting part way carries the slope on
        let s = delay_slope(&tr, 20_000.0);
        let at = delay_at(&tr, 20_000.0);
        retarget(&mut tr, 20_000.0, 3_000.0, false);
        assert_eq!((tr.d_from, tr.d_slope), (at, s));
        assert_close!(delay_slope(&tr, 20_000.0), s, 12);
        // bun: retarget as above (5000 first at 0, 10000 at 1000, 3000 at 20000): dDur, dFrom, dSlope
        assert_close!(tr.d_dur, 58_825.01420913069, 6);
        assert_close!(tr.d_from, 5_800.082962962963, 9);
        assert_close!(tr.d_slope, 0.07566222222222223, 12);
    }

    #[test]
    fn arrivals_set_the_delay_from_cadence_margin_and_age() {
        let mut tr = Track::new(1);
        note_arrival(&mut tr, 0.0);
        assert_eq!(tr.newest_ms, f64::NEG_INFINITY);
        // first data: 8 s cadence + 2 s spacing + 1 s age, set outright
        tr.ts = vec![0.0, 2_000.0, 4_000.0, 6_000.0, 8_000.0];
        tr.n = 5;
        tr.report_ms = 8_000.0;
        note_arrival(&mut tr, 9_000.0);
        assert_eq!((tr.delay_target, tr.d_to, tr.d_dur), (11_000.0, 11_000.0, 0.0));
        assert_eq!(tr.arrival_data_ms, 8_000.0);
        // nothing newer: nothing changes
        note_arrival(&mut tr, 10_000.0);
        assert_eq!(tr.d_ms, 9_000.0);
        // 4 s later, 3 s old: cadence 8 + 0.3 (4 - 8) = 6.8 s, margin 8/3 s, target past the cap,
        // smoothed: 11 + 0.3 (12 - 11) s
        tr.ts.push(12_000.0);
        tr.n = 6;
        tr.report_ms = 12_000.0;
        note_arrival(&mut tr, 15_000.0);
        assert_close!(tr.cadence_ms, 6_800.0, 9);
        assert_close!(tr.delay_target, 11_300.0, 9);
        // under the dead band, the ease stays where it was
        assert_eq!(tr.d_to, 11_000.0);
        // a lone sample: the margin is its maximum; the delay is clamped to 2-12 s
        let mut lone = Track::new(2);
        lone.ts = vec![100_000.0];
        lone.n = 1;
        lone.report_ms = 100_000.0;
        note_arrival(&mut lone, 100_000.0);
        assert_eq!(lone.delay_target, 11_000.0);
        let mut old = Track::new(3);
        old.ts = vec![0.0];
        old.n = 1;
        note_arrival(&mut old, 300_000.0);
        assert_eq!(old.delay_target, 12_000.0);
    }

    #[test]
    fn the_delay_follows_the_datas_cadence_within_2_12_s_and_eases() {
        use crate::model::LiveFlight;
        use crate::motion::aircraft::blend::testing::*;
        let mut m = Motion::new();
        let path = east(250.0);
        // first: a report every 4 s holding the 8 s before it (2 s apart), arriving 1 s late: the drawn
        // time is due one sample spacing behind the newest report when the next one arrives, so
        // 4 + 2 + 1 = 7 s behind now
        let dense = |k: i64| {
            let f = path_flight(|s| path(s + (k * 4) as f64), T0 + (k * 4_000) as f64, &BUFFER);
            LiveFlight { speed: 250, track: 90, ..f }
        };
        // then: a bare report every 20 s, arriving 2 s late (20 + 3 + 2: the 12 s cap)
        let sparse = |t: f64| {
            let (e, n) = path((t - T0) / 1000.0);
            let (lat, lon) = offset(45.0, 10.0, e, n);
            LiveFlight { speed: 250, track: 90, timestamp_ms: t as u64, ..flight_at(lat, lon) }
        };
        let mut f = dense(0);
        let mut next = T0 + 1_000.0;
        let mut k = 0;
        let mut prev_t = f64::NEG_INFINITY;
        let mut prev_delay = f64::NAN;
        let mut worst_rate = 0.0;
        let mut dense_delay = f64::NAN;
        let mut t = T0 + 1_000.0;
        while t <= T0 + 240_000.0 {
            if t >= next {
                if t < T0 + 100_000.0 {
                    f = dense(k);
                    k += 1;
                    next = T0 + (k * 4_000) as f64 + 1_000.0;
                } else {
                    let at = ((t - T0) / 20_000.0).floor() * 20_000.0 + T0;
                    f = sparse(at);
                    next = at + 22_000.0;
                }
            }
            pose(&mut m, &f, t);
            let drawn = m.drawn_time(1, t);
            let delay = t - drawn;
            assert!(delay >= 2_000.0 - 1e-6, "{delay}");
            assert!(delay <= 12_000.0 + 1e-6, "{delay}");
            assert!(drawn > prev_t); // never runs backwards
            if !prev_delay.is_nan() {
                worst_rate = crate::trains::js_max(worst_rate, (delay - prev_delay).abs() / 16.0);
            }
            prev_delay = delay;
            prev_t = drawn;
            if t <= T0 + 100_000.0 {
                dense_delay = delay;
            }
            t += 16.0;
        }
        assert!(dense_delay > 6_500.0 && dense_delay < 7_500.0, "{dense_delay}");
        assert!(prev_delay > 11_000.0, "{prev_delay}");
        // eased: never faster than ~0.15 s per second (the drawn time runs at 85-115% at most)
        assert!(worst_rate < 0.16, "{worst_rate}");
    }

    #[test]
    fn an_unknown_aircraft_is_drawn_now() {
        let m = Motion::new();
        assert_eq!(m.drawn_time(42, 1_234.5), 1_234.5);
    }
}
