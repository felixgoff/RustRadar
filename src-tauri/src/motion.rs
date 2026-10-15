//! Where a vehicle will be over the next couple of minutes, as `frames` the
//! map moves it along between reports, so it glides instead of jumping.
//!
//! With its stops ahead (a train with a timetable) it runs along the track to
//! the next, at each section's speed limit, braking gently into the station
//! and standing there: it leaves when a report shows it moving, never on a
//! guess. Without stops it carries on along its heading at its speed.

/// A stop ahead: where it is and when the vehicle is expected, Unix ms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    pub lat: f64,
    pub lon: f64,
    /// When it is due to arrive (sets the pace where the track's limit is unknown); none: unknown.
    pub arrive_ms: Option<i64>,
    /// When it leaves; none: it ends there.
    pub depart_ms: Option<i64>,
    /// A place it only passes: on its way there, but it doesn't stop.
    pub pass: bool,
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
/// Slower than this, m/s, a train is standing.
const STILL: f64 = 1.0;
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

/// A route along the track: its points, how far along it each target is
/// reached, and each segment's speed limit (m/s) where known. See `rail::route`.
pub type Routed = (Vec<(f64, f64)>, Vec<f64>, Vec<Option<f64>>);

/// The railway, as far as the motion needs it (`rail::Rail`).
pub trait Track {
    /// The way from a point (heading roughly so) through these targets in
    /// order and on for so many metres; none to go straight instead.
    fn route(&self, start: (f64, f64), heading: Option<f64>, targets: &[(f64, f64)], onwards_m: f64)
    -> Option<Routed>;
    /// Where a train reported at `at`, standing at `platform`, is on the platform's track.
    fn hold(&self, at: (f64, f64), platform: (f64, f64)) -> Option<(f64, f64)>;
}

/// The fastest a train is taken to run where the track's limit is higher
/// (or unknown and the timetable asks for more), m/s: 200 km/h.
const TOP: f64 = 55.6;

/// A line to move along, measured, with its speed limits.
struct Path {
    points: Vec<(f64, f64)>,
    /// Distance from the start to each point, metres.
    at: Vec<f64>,
    /// Where the limit changes: from how far along, and to what (m/s; none: unknown).
    limits: Vec<(f64, Option<f64>)>,
}

impl Path {
    fn new(points: Vec<(f64, f64)>, speeds: &[Option<f64>]) -> Path {
        let mut at = Vec::with_capacity(points.len());
        let mut total = 0.0;
        for (i, p) in points.iter().enumerate() {
            if i > 0 {
                total += distance(points[i - 1], *p);
            }
            at.push(total);
        }
        let mut limits: Vec<(f64, Option<f64>)> = Vec::new();
        for (i, speed) in speeds.iter().enumerate().take(points.len().saturating_sub(1)) {
            let speed = speed.filter(|v| v.is_finite() && *v > 0.0);
            if limits.last().is_none_or(|l| l.1 != speed) {
                limits.push((at[i], speed));
            }
        }
        Path { points, at, limits }
    }

    fn len(&self) -> f64 {
        self.at.last().copied().unwrap_or(0.0)
    }

    /// The point `s` metres along.
    fn point(&self, s: f64) -> (f64, f64) {
        if self.points.len() < 2 {
            return self.points.first().copied().unwrap_or_default();
        }
        let i = self.at.partition_point(|d| *d <= s).clamp(1, self.points.len() - 1);
        let (a, b) = (self.points[i - 1], self.points[i]);
        let span = self.at[i] - self.at[i - 1];
        let k = if span > 0.0 { ((s - self.at[i - 1]) / span).clamp(0.0, 1.0) } else { 0.0 };
        (a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k)
    }

    /// The limit `s` metres along, and the index of its section.
    fn limit(&self, s: f64) -> (usize, Option<f64>) {
        let i = self.limits.partition_point(|l| l.0 <= s).saturating_sub(1);
        (i, self.limits.get(i).and_then(|l| l.1))
    }
}

/// A stop on the path: how far along, and when it is due.
struct Mark {
    at: f64,
    arrive_ms: Option<i64>,
    pass: bool,
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

/// As `frames`, along the track where `track` knows it: keeping to each
/// section's speed limit, slowing before a slower one, braking only into the
/// stations it stops at, and standing at a platform until a report shows it
/// moving again (it is never sent off on a guess).
#[allow(clippy::too_many_arguments)]
pub fn frames_with(
    lat: f64,
    lon: f64,
    t0_ms: i64,
    speed: Option<f64>,
    heading: Option<f64>,
    stops: &[Stop],
    now_ms: i64,
    track: Option<&dyn Track>,
) -> Vec<(f64, f64, i64)> {
    if now_ms - t0_ms > MAX_AGE_MS || !lat.is_finite() || !lon.is_finite() {
        return Vec::new();
    }
    let until = now_ms.max(t0_ms) + HORIZON_MS;
    let speed = speed.filter(|s| s.is_finite() && *s >= 0.0);
    let mut v = speed.unwrap_or(0.0);
    let heading = heading.filter(|h| h.is_finite());
    let pos = (lat, lon);
    let mut t = t0_ms;
    let mut next = 0;

    if stops.is_empty() && (heading.is_none() || v <= 0.5) {
        return Vec::new();
    }
    if let Some(first) = stops.first() {
        let platform = (first.lat, first.lon);
        // still, or (not saying) due there already
        let standing = match speed {
            Some(s) => s < STILL,
            None => first.arrive_ms.is_none_or(|a| a <= t0_ms),
        };
        if !first.pass && standing && distance(pos, platform) < AT_STOP_M {
            // at the platform: there until a report shows it on its way
            let at = track.and_then(|tr| tr.hold(pos, platform)).unwrap_or(pos);
            return vec![(at.0, at.1, t0_ms), (at.0, at.1, until)];
        }
        if let Some(h) = heading.filter(|_| v > 3.0) {
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
    let reach = v.max(TOP) * (until - t) as f64 / 1000.0 + 500.0;

    // the way: along the track through the next places (to the next stop and
    // one more), else straight to them
    let upto = ahead_stops.iter().position(|s| !s.pass).map_or(ahead_stops.len(), |i| i + 2);
    let targets: Vec<(f64, f64)> = ahead_stops.iter().take(upto.min(6)).map(|s| (s.lat, s.lon)).collect();
    let routed = track.and_then(|tr| tr.route(pos, heading, &targets, reach)).filter(|(p, _, _)| p.len() >= 2);
    let (points, reached, speeds) = match routed {
        Some(r) => r,
        None if !ahead_stops.is_empty() => {
            let mut points = vec![pos];
            points.extend(ahead_stops.iter().map(|s| (s.lat, s.lon)));
            let at = Path::new(points.clone(), &[]).at;
            (points, at[1..].to_vec(), Vec::new())
        }
        None => (vec![pos, ahead(pos, heading.unwrap_or(0.0), reach)], Vec::new(), Vec::new()),
    };
    let path = Path::new(points, &speeds);
    let marks: Vec<Mark> = reached
        .iter()
        .zip(ahead_stops)
        .map(|(at, s)| Mark { at: *at, arrive_ms: s.arrive_ms, pass: s.pass })
        .collect();
    // where it next stops: places it only passes don't slow it
    let stop_after = |from: usize| (from..marks.len()).find(|i| !marks[*i].pass);

    // where the track doesn't say, the timetable sets the pace
    let mut cruise = match stop_after(0).map(|i| &marks[i]) {
        Some(m) if v > 3.0 => v.max(pace(m.at, m.arrive_ms, t).min(v * 1.2)),
        Some(m) => pace(m.at, m.arrive_ms, t),
        None => v,
    }
    .min(TOP);
    let target = |limit: Option<f64>, cruise: f64| limit.unwrap_or(cruise).min(TOP);

    let mut s = 0.0;
    let mut mark = 0;
    let mut out = vec![{
        let p = path.point(0.0);
        (p.0, p.1, t0_ms)
    }];
    let mut steps = 0;
    while t < until {
        t += STEP_MS;
        steps += 1;
        let dt = STEP_MS as f64 / 1000.0;
        // the places passed by now
        while marks.get(mark).is_some_and(|m| m.pass && m.at <= s) {
            mark += 1;
        }
        let stop = stop_after(mark).map(|i| (i, &marks[i]));
        if let Some((_, m)) = stop
            && m.at - s <= ARRIVED_M
        {
            // in: it stands at the platform from here on, until a report says otherwise
            s = m.at;
            push(&mut out, path.point(s), t);
            break;
        }
        let end = stop.map_or(path.len(), |(_, m)| m.at);
        if end - s <= 0.5 {
            break;
        }

        // the fastest it may go here: the limit, and every lower limit ahead
        // and the stop it is braking for no closer than it can slow for
        let (section, limit) = path.limit(s);
        let mut allowed = target(limit, cruise);
        let look = v * v / (2.0 * BRAKE) + v * dt + 50.0;
        for l in path.limits.iter().skip(section + 1).take_while(|l| l.0 <= s + look) {
            let d = (l.0 - s).max(0.0);
            allowed = allowed.min((target(l.1, cruise).powi(2) + 2.0 * BRAKE * d).sqrt());
        }
        if stop.is_some() {
            let d = (end - s - ARRIVED_M).max(0.0);
            allowed = allowed.min((2.0 * BRAKE * d).sqrt().max(CREEP));
        }
        v = if v > allowed { allowed.max(v - 2.0 * BRAKE * dt) } else { (v + ACCEL * dt).min(allowed) };
        s += (v * dt).min(end - s);
        // past the stop it was timed for, the next one sets the pace
        if let Some((i, m)) = stop
            && limit.is_none()
            && m.at < s + 1.0
            && let Some(n) = stop_after(i + 1).map(|j| &marks[j])
        {
            cruise = pace(n.at - s, n.arrive_ms, t).min(TOP);
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
    fn it_slows_into_a_station_and_stays_until_a_report_moves_it() {
        // 1.5 km to the station at 25 m/s, due to leave it soon after
        let station = ahead((59.0, 18.0), 0.0, 1500.0);
        let stop = Stop {
            lat: station.0,
            lon: station.1,
            arrive_ms: Some(NOW + 60_000),
            depart_ms: Some(NOW + 90_000),
            pass: false,
        };
        let beyond = ahead((59.0, 18.0), 0.0, 20_000.0);
        let after = Stop { lat: beyond.0, lon: beyond.1, arrive_ms: Some(NOW + 900_000), depart_ms: None, pass: false };
        let f = frames(59.0, 18.0, NOW, Some(25.0), Some(0.0), &[stop, after], NOW);

        let arrived = f.iter().position(|p| distance((p.0, p.1), station) < 1.0).expect("reaches the station");
        let first = speed_between(f[0], f[1]);
        let last = speed_between(f[arrived - 1], f[arrived]);
        assert!(first > 20.0 && last < 10.0, "slows down: {first:.1} then {last:.1} m/s");
        assert!(f[arrived..].iter().all(|p| distance((p.0, p.1), station) < 1.0), "never leaves on a guess");
        assert!(f.windows(2).all(|w| w[0].2 < w[1].2), "time runs forward");
        assert_eq!(f.last().unwrap().2, NOW + HORIZON_MS);
    }

    #[test]
    fn standing_at_a_stop_it_stays_even_past_its_departure() {
        let here = (59.0, 18.0);
        let next = ahead(here, 90.0, 10_000.0);
        let stops = [
            Stop { lat: here.0, lon: here.1, arrive_ms: None, depart_ms: Some(NOW - 60_000), pass: false },
            Stop { lat: next.0, lon: next.1, arrive_ms: Some(NOW + 630_000), depart_ms: None, pass: false },
        ];
        let f = frames(here.0, here.1, NOW, Some(0.0), None, &stops, NOW);
        assert!(f.iter().all(|p| distance((p.0, p.1), here) < 1.0));
        assert_eq!(f.last().unwrap().2, NOW + HORIZON_MS);
        // no speed in the report, due there already: standing too
        let near = ahead(here, 90.0, 80.0);
        let f = frames(near.0, near.1, NOW, None, Some(90.0), &stops, NOW);
        assert!(f.iter().all(|p| distance((p.0, p.1), near) < 1.0));
        // a report showing it moving sets it off
        let f = frames(near.0, near.1, NOW, Some(4.0), Some(90.0), &stops, NOW);
        assert!(distance((f.last().unwrap().0, f.last().unwrap().1), near) > 500.0);
    }

    #[test]
    fn it_passes_places_it_doesnt_stop_at_without_slowing() {
        let start = (59.0, 18.0);
        let passed = ahead(start, 0.0, 1_000.0);
        let far = ahead(start, 0.0, 40_000.0);
        let stops = [
            Stop { lat: passed.0, lon: passed.1, arrive_ms: Some(NOW + 40_000), depart_ms: None, pass: true },
            Stop { lat: far.0, lon: far.1, arrive_ms: Some(NOW + 1_000_000), depart_ms: None, pass: false },
        ];
        let f = frames(start.0, start.1, NOW, Some(30.0), Some(0.0), &stops, NOW);
        let slowest = f.windows(2).map(|w| speed_between(w[0], w[1])).fold(f64::MAX, f64::min);
        assert!(slowest > 25.0, "kept going: {slowest:.1} m/s");
    }

    #[test]
    fn a_stop_already_behind_it_is_skipped() {
        let here = (59.0, 18.0);
        let behind = ahead(here, 180.0, 800.0);
        let front = ahead(here, 0.0, 30_000.0);
        let stops = [
            Stop { lat: behind.0, lon: behind.1, arrive_ms: None, depart_ms: Some(NOW), pass: false },
            Stop { lat: front.0, lon: front.1, arrive_ms: Some(NOW + 1_200_000), depart_ms: None, pass: false },
        ];
        let f = frames(here.0, here.1, NOW, Some(25.0), Some(0.0), &stops, NOW);
        assert!(f.last().unwrap().0 > here.0, "still heading north");
    }

    struct Fixed(Routed);
    impl Track for Fixed {
        fn route(&self, _: (f64, f64), _: Option<f64>, _: &[(f64, f64)], _: f64) -> Option<Routed> {
            Some(self.0.clone())
        }
        fn hold(&self, at: (f64, f64), _: (f64, f64)) -> Option<(f64, f64)> {
            Some(at)
        }
    }

    #[test]
    fn with_a_route_it_follows_the_track() {
        // the track bends: 2 km east, then north to the station 2 km on
        let start = (59.0, 18.0);
        let corner = ahead(start, 90.0, 2_000.0);
        let station = ahead(corner, 0.0, 2_000.0);
        let track = Fixed((vec![start, corner, station], vec![4_000.0], Vec::new()));
        let stop =
            Stop { lat: station.0, lon: station.1, arrive_ms: Some(NOW + 200_000), depart_ms: None, pass: false };
        let f = frames_with(start.0, start.1, NOW, Some(30.0), Some(90.0), &[stop], NOW, Some(&track));
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

    #[test]
    fn it_keeps_to_the_limits_and_slows_before_a_slower_section() {
        // 4 km at 160 km/h, then 80 km/h; no stops for a long way
        let start = (59.0, 18.0);
        let change = ahead(start, 0.0, 4_000.0);
        let end = ahead(start, 0.0, 20_000.0);
        let track = Fixed((vec![start, change, end], Vec::new(), vec![Some(160.0 / 3.6), Some(80.0 / 3.6)]));
        let far = ahead(start, 0.0, 60_000.0);
        let stop = Stop { lat: far.0, lon: far.1, arrive_ms: Some(NOW + 3_600_000), depart_ms: None, pass: false };
        let f = frames_with(start.0, start.1, NOW, Some(40.0), Some(0.0), &[stop], NOW, Some(&track));
        let speeds: Vec<(f64, f64)> =
            f.windows(2).map(|w| (distance(start, (w[1].0, w[1].1)), speed_between(w[0], w[1]))).collect();
        let top = speeds.iter().map(|s| s.1).fold(0.0, f64::max);
        assert!((top - 160.0 / 3.6).abs() < 1.5, "up to the limit: {top:.1} m/s");
        assert!(speeds.iter().all(|(at, v)| *at < 4_000.0 || *v < 80.0 / 3.6 + 1.0), "slow by the change: {speeds:?}");
        let before = speeds.iter().filter(|(at, _)| (3_400.0..3_950.0).contains(at)).map(|s| s.1).fold(f64::MAX, f64::min);
        assert!(before < 160.0 / 3.6 - 3.0, "braking before it: {before:.1} m/s");
    }
}
