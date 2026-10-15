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

/// The pace to cover `metres` by `arrive` from `t`, m/s; a default without a time.
fn pace(metres: f64, arrive: Option<i64>, t: i64) -> f64 {
    match arrive {
        Some(arrive) => (metres / ((arrive - t).max(60_000) as f64 / 1000.0)).clamp(PACE.0, PACE.1),
        None => 22.0,
    }
}

/// A route along the track: its points and how far along it each target is
/// reached. See `rail::route`.
pub type Routed = (Vec<(f64, f64)>, Vec<f64>);

/// Finds the way from a point (heading roughly so) through these targets in
/// order and on for so many metres; none to go straight instead.
pub type Router<'a> = &'a dyn Fn((f64, f64), Option<f64>, &[(f64, f64)], f64) -> Option<Routed>;

/// A line to move along, measured.
struct Path {
    points: Vec<(f64, f64)>,
    /// Distance from the start to each point, metres.
    at: Vec<f64>,
}

impl Path {
    fn new(points: Vec<(f64, f64)>) -> Path {
        let mut at = Vec::with_capacity(points.len());
        let mut total = 0.0;
        for (i, p) in points.iter().enumerate() {
            if i > 0 {
                total += distance(points[i - 1], *p);
            }
            at.push(total);
        }
        Path { points, at }
    }

    fn len(&self) -> f64 {
        self.at.last().copied().unwrap_or(0.0)
    }

    /// The point `s` metres along.
    fn point(&self, s: f64) -> (f64, f64) {
        let i = self.at.partition_point(|d| *d <= s).clamp(1, self.points.len().max(2) - 1);
        let (a, b) = (self.points[i - 1], self.points[i]);
        let span = self.at[i] - self.at[i - 1];
        let k = if span > 0.0 { ((s - self.at[i - 1]) / span).clamp(0.0, 1.0) } else { 0.0 };
        (a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k)
    }
}

/// A stop on the path: how far along, and when it is due.
struct Mark {
    at: f64,
    arrive_ms: Option<i64>,
    depart_ms: Option<i64>,
}

/// Frames `(lat, lon, unix_ms)` from a report at `t0_ms` up to `now_ms` plus
/// the horizon, in straight lines. `speed` in m/s; `heading` in degrees.
/// Empty when there is nothing to move: no speed and no stops, or a report
/// too old.
pub fn frames(
    lat: f64,
    lon: f64,
    t0_ms: i64,
    speed: Option<f64>,
    heading: Option<f64>,
    stops: &[Stop],
    now_ms: i64,
) -> Vec<(f64, f64, i64)> {
    frames_with(lat, lon, t0_ms, speed, heading, stops, now_ms, None)
}

/// As `frames`, along the way `router` finds (the track) where it finds one.
#[allow(clippy::too_many_arguments)]
pub fn frames_with(
    lat: f64,
    lon: f64,
    t0_ms: i64,
    speed: Option<f64>,
    heading: Option<f64>,
    stops: &[Stop],
    now_ms: i64,
    router: Option<Router>,
) -> Vec<(f64, f64, i64)> {
    if now_ms - t0_ms > MAX_AGE_MS || !lat.is_finite() || !lon.is_finite() {
        return Vec::new();
    }
    let until = now_ms.max(t0_ms) + HORIZON_MS;
    let mut v = speed.filter(|s| s.is_finite() && *s >= 0.0).unwrap_or(0.0);
    let heading = heading.filter(|h| h.is_finite());
    let mut pos = (lat, lon);
    let mut t = t0_ms;
    let mut next = 0;
    // waiting at a stop until then
    let mut dwell: Option<i64> = None;

    if stops.is_empty() && (heading.is_none() || v <= 0.5) {
        return Vec::new();
    }
    if let Some(first) = stops.first() {
        if v < 1.5 && distance(pos, (first.lat, first.lon)) < AT_STOP_M {
            // standing at the first stop: it is waiting there
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
    }
    let ahead_stops = &stops[next.min(stops.len())..];
    // as far as it could go by the horizon, and a little more
    let reach = v.max(PACE.1 / 2.0) * (until - t) as f64 / 1000.0 + 500.0;

    // the way: along the track through the next stops, else straight to them
    let targets: Vec<(f64, f64)> = ahead_stops.iter().take(2).map(|s| (s.lat, s.lon)).collect();
    let routed = router.and_then(|r| r(pos, heading, &targets, reach)).filter(|(p, _)| p.len() >= 2);
    let (points, reached) = match routed {
        // standing at a station, the route starts where it snapped to the track
        Some((points, reached)) => (points, reached),
        None if !ahead_stops.is_empty() => {
            let mut points = vec![pos];
            points.extend(ahead_stops.iter().map(|s| (s.lat, s.lon)));
            let at = Path::new(points.clone()).at;
            (points, at[1..].to_vec())
        }
        None => (vec![pos, ahead(pos, heading.unwrap_or(0.0), reach)], Vec::new()),
    };
    if dwell.is_some() {
        pos = points[0];
    }
    let path = Path::new(points);
    let marks: Vec<Mark> = reached
        .iter()
        .zip(ahead_stops)
        .map(|(at, s)| Mark { at: *at, arrive_ms: s.arrive_ms, depart_ms: s.depart_ms })
        .collect();

    let mut s = 0.0;
    let mut mark = 0;
    let unreached = |t: i64| match ahead_stops.first() {
        Some(st) => pace(distance(pos, (st.lat, st.lon)), st.arrive_ms, t),
        None => v,
    };
    let mut cruise = match marks.first() {
        Some(m) if v > 3.0 => v.max(pace(m.at, m.arrive_ms, t).min(v * 1.2)),
        Some(m) => pace(m.at, m.arrive_ms, t),
        None => v.max(unreached(t)),
    };
    let mut out = vec![(pos.0, pos.1, t0_ms)];
    let mut steps = 0;
    while t < until {
        t += STEP_MS;
        steps += 1;
        let dt = STEP_MS as f64 / 1000.0;
        if let Some(leave) = dwell {
            if t < leave {
                if steps % FRAME_EVERY == 0 {
                    push(&mut out, path.point(s), t);
                }
                continue;
            }
            dwell = None;
            cruise = match marks.get(mark) {
                Some(m) => pace(m.at - s, m.arrive_ms, t),
                None => pace(path.len() - s, None, t),
            };
            push(&mut out, path.point(s), t);
        }
        match marks.get(mark) {
            Some(m) => {
                let d = m.at - s;
                if d <= ARRIVED_M {
                    s = m.at;
                    v = 0.0;
                    dwell = Some(m.depart_ms.unwrap_or(i64::MAX));
                    mark += 1;
                    push(&mut out, path.point(s), t);
                    continue;
                }
                // brake when the stopping distance reaches the station, else pull up to pace
                if v * v / (2.0 * BRAKE) >= d - v * dt {
                    v = (v - BRAKE * dt).max(CREEP);
                } else {
                    v = (v + ACCEL * dt).min(cruise.max(CREEP));
                }
                s += (v * dt).min(d);
            }
            None => {
                // no stop ahead on the way it knows: on at its pace, to the end of it
                let d = path.len() - s;
                if d <= 0.5 {
                    break;
                }
                v = (v + ACCEL * dt).min(cruise.max(CREEP));
                s += (v * dt).min(d);
            }
        }
        if steps % FRAME_EVERY == 0 {
            push(&mut out, path.point(s), t);
        }
    }
    push(&mut out, path.point(s), until);
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
        let (first, last) = (f[0], *f.last().unwrap());
        assert_eq!(last.2, NOW + HORIZON_MS);
        assert!((speed_between(first, last) - 20.0).abs() < 0.1);
        assert!(last.1 > 18.0 && (last.0 - 59.0).abs() < 0.01, "east");
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

    #[test]
    fn with_a_route_it_follows_the_track() {
        // the track bends: 2 km east, then north to the station 2 km on
        let start = (59.0, 18.0);
        let corner = ahead(start, 90.0, 2_000.0);
        let station = ahead(corner, 0.0, 2_000.0);
        let router = |_: (f64, f64), _: Option<f64>, _: &[(f64, f64)], _: f64| -> Option<Routed> {
            Some((vec![start, corner, station], vec![4_000.0]))
        };
        let stop = Stop { lat: station.0, lon: station.1, arrive_ms: Some(NOW + 200_000), depart_ms: None };
        let f = frames_with(start.0, start.1, NOW, Some(30.0), Some(90.0), &[stop], NOW, Some(&router));
        // halfway through the horizon it has gone round the corner, not cut across
        let mid = f.iter().find(|p| p.2 >= NOW + 100_000).unwrap();
        let off_track = distance((mid.0, mid.1), corner).min(
            // on the second leg: due north of the corner
            if mid.0 >= corner.0 { distance((mid.0, mid.1), (mid.0, corner.1)) } else { f64::MAX },
        );
        assert!(off_track < 20.0, "{off_track} m off the track at {mid:?}");
        let end = f.last().unwrap();
        assert!(end.0 > corner.0 && end.0 <= station.0 + 1e-9, "on the northern leg, short of the station: {end:?}");
        assert!((end.1 - corner.1).abs() < 1e-6, "on the track");
    }
}
