//! Where a vehicle will be over the next couple of minutes, as `frames` the
//! map moves it along between reports, so it glides instead of jumping.
//!
//! With its stops ahead (a train with a timetable) it runs to each, braking
//! gently into the station, waits until it is due to leave, and pulls away
//! again. Without them it carries on along its heading at its speed.

/// A stop ahead: where it is and when the vehicle is expected, Unix ms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    pub lat: f64,
    pub lon: f64,
    /// When it is due to arrive (sets the pace when pulling away); none: unknown.
    pub arrive_ms: Option<i64>,
    /// When it leaves; none: it ends there.
    pub depart_ms: Option<i64>,
}

/// How far ahead the frames reach, past now.
pub const HORIZON_MS: i64 = 120_000;
/// A report older than this isn't carried on: better a still train than a guess.
const MAX_AGE_MS: i64 = 10 * 60_000;
const STEP_MS: i64 = 1000;
/// A frame every this many steps (and at every stop and start).
const FRAME_EVERY: i64 = 5;
/// Gentle, as on a passenger train: m/s².
const ACCEL: f64 = 0.4;
const BRAKE: f64 = 0.5;
/// Close enough to the station to have arrived, metres.
const ARRIVED_M: f64 = 3.0;
/// Standing this near the first stop: it is at it, metres.
const AT_STOP_M: f64 = 400.0;
/// The slowest it rolls up to a station, m/s, so it gets there.
const CREEP: f64 = 1.5;
/// Pace limits when working it out from the timetable, m/s (30 to 200 km/h).
const PACE: (f64, f64) = (8.3, 55.0);

const EARTH_M: f64 = 6_371_000.0;

pub fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, la2) = (a.0.to_radians(), b.0.to_radians());
    let dlat = la2 - la1;
    let dlon = (b.1 - a.1).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_M * h.sqrt().min(1.0).asin()
}

/// The initial course from `a` to `b`, degrees clockwise from north.
pub fn course(a: (f64, f64), b: (f64, f64)) -> f64 {
    bearing(a, b)
}

fn bearing(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, la2) = (a.0.to_radians(), b.0.to_radians());
    let dlon = (b.1 - a.1).to_radians();
    let y = dlon.sin() * la2.cos();
    let x = la1.cos() * la2.sin() - la1.sin() * la2.cos() * dlon.cos();
    y.atan2(x).to_degrees().rem_euclid(360.0)
}

/// `metres` from `from` on a bearing in degrees.
pub fn ahead(from: (f64, f64), bearing: f64, metres: f64) -> (f64, f64) {
    let d = metres / EARTH_M;
    let (lat1, lon1, b) = (from.0.to_radians(), from.1.to_radians(), bearing.to_radians());
    let lat2 = (lat1.sin() * d.cos() + lat1.cos() * d.sin() * b.cos()).asin();
    let lon2 = lon1 + (b.sin() * d.sin() * lat1.cos()).atan2(d.cos() - lat1.sin() * lat2.sin());
    (lat2.to_degrees(), (lon2.to_degrees() + 540.0).rem_euclid(360.0) - 180.0)
}

/// Adds a frame; one at the same moment as the last replaces it.
fn push(out: &mut Vec<(f64, f64, i64)>, pos: (f64, f64), t: i64) {
    if out.last().is_some_and(|f| f.2 >= t) {
        out.pop();
    }
    out.push((pos.0, pos.1, t));
}

/// The pace to make `to` by its arrival from `from` at `t`, m/s; a default without one.
fn pace(from: (f64, f64), to: &Stop, t: i64) -> f64 {
    let d = distance(from, (to.lat, to.lon));
    match to.arrive_ms {
        Some(arrive) => (d / ((arrive - t).max(60_000) as f64 / 1000.0)).clamp(PACE.0, PACE.1),
        None => 22.0,
    }
}

/// Frames `(lat, lon, unix_ms)` from a report at `t0_ms` up to `now_ms` plus
/// the horizon. `speed` in m/s; `heading` in degrees. Empty when there is
/// nothing to move: no speed and no stops, or a report too old.
pub fn frames(
    lat: f64,
    lon: f64,
    t0_ms: i64,
    speed: Option<f64>,
    heading: Option<f64>,
    stops: &[Stop],
    now_ms: i64,
) -> Vec<(f64, f64, i64)> {
    if now_ms - t0_ms > MAX_AGE_MS || !lat.is_finite() || !lon.is_finite() {
        return Vec::new();
    }
    let until = now_ms.max(t0_ms) + HORIZON_MS;
    let mut v = speed.filter(|s| s.is_finite() && *s >= 0.0).unwrap_or(0.0);
    let start = (lat, lon);

    if stops.is_empty() {
        // no timetable: straight on at its speed
        return match heading.filter(|h| h.is_finite()) {
            Some(h) if v > 0.5 => {
                vec![(lat, lon, t0_ms), {
                    let (la, lo) = ahead(start, h, v * (until - t0_ms) as f64 / 1000.0);
                    (la, lo, until)
                }]
            }
            _ => Vec::new(),
        };
    }

    let mut pos = start;
    let mut t = t0_ms;
    let mut next = 0;
    let mut out = vec![(lat, lon, t0_ms)];
    // waiting at a stop until then
    let mut dwell: Option<i64> = None;

    // standing at the first stop: it is waiting there
    let first = &stops[0];
    if v < 1.5 && distance(pos, (first.lat, first.lon)) < AT_STOP_M {
        pos = (first.lat, first.lon);
        dwell = Some(first.depart_ms.unwrap_or(i64::MAX));
        next = 1;
    } else if let Some(h) = heading.filter(|_| v > 3.0) {
        // a stop behind it was passed already: the timetable runs a little late or early
        while next + 1 < stops.len() {
            let s = &stops[next];
            let off = (bearing(pos, (s.lat, s.lon)) - h).rem_euclid(360.0);
            if (120.0..=240.0).contains(&off) && distance(pos, (s.lat, s.lon)) < 5_000.0 {
                next += 1;
            } else {
                break;
            }
        }
    }
    let mut cruise = match stops.get(next) {
        Some(s) if v > 3.0 => v.max(pace(pos, s, t).min(v * 1.2)),
        Some(s) => pace(pos, s, t),
        None => v,
    };

    let mut steps = 0;
    while t < until {
        t += STEP_MS;
        steps += 1;
        let dt = STEP_MS as f64 / 1000.0;
        if let Some(leave) = dwell {
            if t < leave {
                if steps % FRAME_EVERY == 0 {
                    push(&mut out, pos, t);
                }
                continue;
            }
            dwell = None;
            if let Some(s) = stops.get(next) {
                cruise = pace(pos, s, t);
            }
            push(&mut out, pos, t);
        }
        let Some(target) = stops.get(next) else {
            // past its last stop
            push(&mut out, pos, until);
            break;
        };
        let goal = (target.lat, target.lon);
        let d = distance(pos, goal);
        if d <= ARRIVED_M {
            pos = goal;
            v = 0.0;
            dwell = Some(target.depart_ms.unwrap_or(i64::MAX));
            next += 1;
            push(&mut out, pos, t);
            continue;
        }
        // brake when the stopping distance reaches the station, else pull up to pace
        if v * v / (2.0 * BRAKE) >= d - v * dt {
            v = (v - BRAKE * dt).max(CREEP);
        } else {
            v = (v + ACCEL * dt).min(cruise.max(CREEP));
        }
        let step = (v * dt).min(d);
        pos = ahead(pos, bearing(pos, goal), step);
        if steps % FRAME_EVERY == 0 {
            push(&mut out, pos, t);
        }
    }
    if out.last().is_some_and(|f| f.2 < until) {
        push(&mut out, pos, until);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn speed_between(a: (f64, f64, i64), b: (f64, f64, i64)) -> f64 {
        distance((a.0, a.1), (b.0, b.1)) / ((b.2 - a.2) as f64 / 1000.0)
    }

    #[test]
    fn without_stops_it_carries_on_at_its_speed() {
        let f = frames(59.0, 18.0, NOW - 10_000, Some(20.0), Some(90.0), &[], NOW);
        assert_eq!(f.len(), 2);
        assert_eq!(f[1].2, NOW + HORIZON_MS);
        assert!((speed_between(f[0], f[1]) - 20.0).abs() < 0.1);
        assert!(f[1].1 > 18.0 && (f[1].0 - 59.0).abs() < 0.01, "east");
        assert!(frames(59.0, 18.0, NOW, Some(0.0), Some(90.0), &[], NOW).is_empty(), "standing still");
        assert!(frames(59.0, 18.0, NOW - 20 * 60_000, Some(20.0), Some(90.0), &[], NOW).is_empty(), "too old");
    }

    #[test]
    fn it_slows_into_a_station_and_waits_there() {
        // 1.5 km to the station at 25 m/s, leaving it a minute after now
        let station = ahead((59.0, 18.0), 0.0, 1500.0);
        let stop =
            Stop { lat: station.0, lon: station.1, arrive_ms: Some(NOW + 60_000), depart_ms: Some(NOW + 90_000) };
        let beyond = ahead((59.0, 18.0), 0.0, 20_000.0);
        let after = Stop { lat: beyond.0, lon: beyond.1, arrive_ms: Some(NOW + 900_000), depart_ms: None };
        let f = frames(59.0, 18.0, NOW, Some(25.0), Some(0.0), &[stop, after], NOW);

        let arrived = f.iter().position(|p| distance((p.0, p.1), station) < 1.0).expect("reaches the station");
        let first = speed_between(f[0], f[1]);
        let last = speed_between(f[arrived - 1], f[arrived]);
        assert!(first > 20.0 && last < 10.0, "slows down: {first:.1} then {last:.1} m/s");
        let leaves = f.iter().position(|p| p.2 > NOW + 90_000 && distance((p.0, p.1), station) > 1.0).expect("leaves");
        assert!(f[leaves - 1].2 >= NOW + 90_000 - 5_000, "waits until it is due to leave");
        assert!(f.windows(2).all(|w| w[0].2 < w[1].2), "time runs forward");
        assert_eq!(f.last().unwrap().2, NOW + HORIZON_MS);
    }

    #[test]
    fn standing_at_a_stop_it_waits_then_pulls_away() {
        let here = (59.0, 18.0);
        let next = ahead(here, 90.0, 10_000.0);
        let stops = [
            Stop { lat: here.0, lon: here.1, arrive_ms: None, depart_ms: Some(NOW + 30_000) },
            Stop { lat: next.0, lon: next.1, arrive_ms: Some(NOW + 630_000), depart_ms: None },
        ];
        let f = frames(here.0, here.1, NOW, Some(0.0), None, &stops, NOW);
        assert!(f.iter().filter(|p| p.2 <= NOW + 30_000).all(|p| distance((p.0, p.1), here) < 1.0));
        let end = f.last().unwrap();
        assert!(distance((end.0, end.1), here) > 500.0, "on its way after leaving");
    }

    #[test]
    fn a_stop_already_behind_it_is_skipped() {
        let here = (59.0, 18.0);
        let behind = ahead(here, 180.0, 800.0);
        let front = ahead(here, 0.0, 30_000.0);
        let stops = [
            Stop { lat: behind.0, lon: behind.1, arrive_ms: None, depart_ms: Some(NOW) },
            Stop { lat: front.0, lon: front.1, arrive_ms: Some(NOW + 1_200_000), depart_ms: None },
        ];
        let f = frames(here.0, here.1, NOW, Some(25.0), Some(0.0), &stops, NOW);
        assert!(f.last().unwrap().0 > here.0, "still heading north");
    }
}
