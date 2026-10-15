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

/// The angle between two courses, degrees (0 to 180).
fn turn(a: f64, b: f64) -> f64 {
    let d = (b - a).rem_euclid(360.0);
    d.min(360.0 - d)
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
    /// The train's top speed, m/s, when known.
    fn top(&self) -> Option<f64> {
        None
    }
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
    /// Stops this long, then goes on; none: stands there until a report moves it.
    dwell_ms: Option<i64>,
}

/// What a train does next, by its report and its stops ahead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// At the platform of `stops[at]`, until a report shows it moving.
    Standing { at: usize },
    /// On its way to `stops[next]` (the ones before it are behind it already).
    Moving { next: usize },
}

/// A station further than this from a train it has passed, metres.
const PASSED_M: f64 = 300.0;
/// A train reported moving this far beyond a station, away from it, has left it, metres.
const LEFT_M: f64 = 50.0;

/// Whether a train is standing at a stop or on its way, and to which stop.
/// A stop is behind it when it is past it on the way to the one after
/// (nearer that one than the stop is), or, moving, when it lies behind it
/// close by: by where it is, not by the clock, so a late train still stops
/// at the station it hasn't reached.
pub fn plan(pos: (f64, f64), speed: Option<f64>, heading: Option<f64>, stops: &[Stop], t0_ms: i64) -> Plan {
    let speed = speed.filter(|s| s.is_finite() && *s >= 0.0);
    let moving = speed.is_none_or(|s| s > 3.0);
    let mut next = 0;
    while next + 1 < stops.len() {
        let (here, then) = ((stops[next].lat, stops[next].lon), (stops[next + 1].lat, stops[next + 1].lon));
        let near = distance(pos, here);
        let past = near > PASSED_M && distance(pos, then) + PASSED_M < distance(here, then);
        // seen moving away from it: it has left (a reported speed says so sooner)
        let left_m = if speed.is_some() { LEFT_M } else { PASSED_M };
        let behind = moving
            && near > left_m
            && near < 5_000.0
            && heading.filter(|h| h.is_finite()).is_some_and(|h| turn(bearing(pos, here), h) > 120.0);
        if past || behind {
            next += 1;
        } else {
            break;
        }
    }
    let Some(first) = stops.get(next) else { return Plan::Moving { next } };
    // still, or (not saying) due there already
    let standing = match speed {
        Some(s) => s < STILL,
        None => first.arrive_ms.is_none_or(|a| a <= t0_ms),
    };
    if !first.pass && standing && distance(pos, (first.lat, first.lon)) < AT_STOP_M {
        return Plan::Standing { at: next };
    }
    Plan::Moving { next }
}

/// How many of the stops ahead the way along the track is found through:
/// up to the next one it stops at, and one more.
pub fn route_targets(ahead: &[Stop]) -> usize {
    ahead.iter().position(|s| !s.pass).map_or(ahead.len(), |i| i + 2).min(6).min(ahead.len())
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

/// A position report: where and when (Unix ms), and as far as it says how
/// fast (m/s) and which way (degrees).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fix {
    pub lat: f64,
    pub lon: f64,
    pub t_ms: i64,
    pub speed: Option<f64>,
    pub heading: Option<f64>,
}

/// Reports further apart than this aren't drawn between: the older is too stale.
const MAX_LAG_MS: i64 = 180_000;
/// A train that moved less than this between two reports is drawn from the latest, metres.
const MOVED_M: f64 = 5.0;

/// A place on a train's way, and what it does there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub at: (f64, f64),
    /// It only passes it: no slowing.
    pub pass: bool,
    /// When it is due there (sets the pace where the track's limit is unknown).
    pub arrive_ms: Option<i64>,
    /// It stops this long, then goes on (a station the real train has already
    /// left); none: it stands there until a report shows it moving.
    pub dwell_ms: Option<i64>,
    /// Which of the train's stops it is; none for a report.
    pub stop: Option<usize>,
}

/// A stop at a station the real train has already called at, as drawn, ms:
/// its timetabled stop, within reason.
const DWELL_MS: (i64, i64, i64) = (15_000, 20_000, 45_000);

fn target_of(stops: &[Stop], i: usize, left_already: bool) -> Target {
    let s = &stops[i];
    let dwell = match (s.arrive_ms, s.depart_ms) {
        (Some(a), Some(d)) => (d - a).clamp(DWELL_MS.0, DWELL_MS.2),
        _ => DWELL_MS.1,
    };
    Target {
        at: (s.lat, s.lon),
        pass: s.pass,
        arrive_ms: if left_already { None } else { s.arrive_ms },
        dwell_ms: (left_already && !s.pass).then_some(dwell),
        stop: Some(i),
    }
}

/// The newest report as a place on the way: passed, never stopped at.
fn report_target(at: (f64, f64)) -> Target {
    Target { at, pass: true, arrive_ms: None, dwell_ms: None, stop: None }
}

/// The stops from `next` the way along the track is found through.
fn stop_targets(stops: &[Stop], next: usize) -> Vec<Target> {
    let next = next.min(stops.len());
    (next..next + route_targets(&stops[next..])).map(|i| target_of(stops, i, false)).collect()
}

/// The stops between a train drawn at `from` and where it was last seen (on
/// its way to `stops[next]`): ones the real train has passed already, or
/// called at and left. The drawn train still calls at them, briefly.
fn between(from: (f64, f64), heading: Option<f64>, speed: Option<f64>, stops: &[Stop], next: usize, t: i64) -> Vec<Target> {
    let first = match plan(from, speed, heading, stops, t) {
        Plan::Standing { at } => at,
        Plan::Moving { next } => next,
    };
    (first..next.min(stops.len())).map(|i| target_of(stops, i, true)).collect()
}

/// Where a train's way starts and what it goes through: what its frames and
/// its drawn route are both made from.
#[derive(Clone, Debug, PartialEq)]
pub struct Way {
    /// The start: the report before the latest, when there is one to come
    /// from, so the train is drawn a report behind and never ahead of where
    /// it was last seen; else the latest.
    pub from: (f64, f64),
    pub heading: Option<f64>,
    /// The places it goes through, in order: stations the real train has
    /// already called at (it calls at them too), the newest report, its next stops.
    pub targets: Vec<Target>,
    /// Which target is the newest report, when it comes from behind it.
    pub lead: Option<usize>,
    /// The first of the stops the real train is on its way to.
    pub next: usize,
    /// Its speed at the start, if known: m/s.
    pub speed: Option<f64>,
}

impl Way {
    pub fn points(&self) -> Vec<(f64, f64)> {
        self.targets.iter().map(|t| t.at).collect()
    }
}

/// The way of a train by its latest report (and the one before), or none
/// while it stands at a platform.
pub fn way(latest: &Fix, prev: Option<&Fix>, stops: &[Stop]) -> Option<Way> {
    let pos = (latest.lat, latest.lon);
    let Plan::Moving { next } = plan(pos, latest.speed, latest.heading, stops, latest.t_ms) else { return None };
    let ahead = stop_targets(stops, next);
    let valid = |v: &f64| v.is_finite() && *v >= 0.0;
    let from_prev = prev.filter(|p| {
        let lag = latest.t_ms - p.t_ms;
        p.lat.is_finite() && p.lon.is_finite() && (1..=MAX_LAG_MS).contains(&lag) && distance((p.lat, p.lon), pos) >= MOVED_M
    });
    Some(match from_prev {
        Some(p) => {
            let from = (p.lat, p.lon);
            let lag_s = (latest.t_ms - p.t_ms) as f64 / 1000.0;
            // the way it went
            let heading = Some(course(from, pos));
            let speed = p.speed.filter(valid).or(Some(distance(from, pos) / lag_s));
            let mut targets = between(from, heading, speed, stops, next, p.t_ms);
            let lead = targets.len();
            targets.push(report_target(pos));
            targets.extend(ahead);
            Way { from, heading, targets, lead: Some(lead), next, speed }
        }
        None => Way {
            from: pos,
            heading: latest.heading.filter(|h| h.is_finite()),
            targets: ahead,
            lead: None,
            next,
            speed: latest.speed.filter(valid),
        },
    })
}

/// The fastest it may go `s` metres along: the limit there, every lower
/// limit ahead no closer than it can slow for, and the stop at `stop_at`
/// (metres along) it brakes for.
fn allowed(path: &Path, s: f64, v: f64, dt: f64, cruise: f64, stop_at: Option<f64>, top: f64) -> f64 {
    let target = |limit: Option<f64>| limit.unwrap_or(cruise).min(top);
    let (section, limit) = path.limit(s);
    let mut allowed = target(limit);
    let look = v.max(allowed).powi(2) / (2.0 * BRAKE) + v * dt + 50.0;
    for l in path.limits.iter().skip(section + 1).take_while(|l| l.0 <= s + look) {
        let d = (l.0 - s).max(0.0);
        allowed = allowed.min((target(l.1).powi(2) + 2.0 * BRAKE * d).sqrt());
    }
    if let Some(end) = stop_at {
        let d = (end - s - ARRIVED_M).max(0.0);
        allowed = allowed.min((2.0 * BRAKE * d).sqrt().max(CREEP));
    }
    allowed
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
    let latest = Fix { lat, lon, t_ms: t0_ms, speed, heading };
    frames_between(&latest, None, stops, now_ms, track)
}

/// As `frames_with`, drawn a report behind: from the report before the
/// latest (at the latest's time) along the track through the latest and on,
/// so it never runs ahead of where the train was last seen. Without a speed
/// in the report it runs as fast as the track allows there, slower ahead of
/// a slow section or its stop.
pub fn frames_between(
    latest: &Fix,
    prev: Option<&Fix>,
    stops: &[Stop],
    now_ms: i64,
    track: Option<&dyn Track>,
) -> Vec<(f64, f64, i64)> {
    let t0_ms = latest.t_ms;
    if now_ms - t0_ms > MAX_AGE_MS || !latest.lat.is_finite() || !latest.lon.is_finite() {
        return Vec::new();
    }
    let until = now_ms.max(t0_ms) + HORIZON_MS;
    let Some(way) = way(latest, prev, stops) else {
        // at the platform: there until a report shows it on its way
        let pos = (latest.lat, latest.lon);
        let at = match plan(pos, latest.speed, latest.heading, stops, t0_ms) {
            Plan::Standing { at } => at,
            Plan::Moving { next } => next,
        };
        let platform = stops.get(at).map_or(pos, |s| (s.lat, s.lon));
        let at = track.and_then(|tr| tr.hold(pos, platform)).unwrap_or(pos);
        return vec![(at.0, at.1, t0_ms), (at.0, at.1, until)];
    };
    // reported standing, away from a platform (at a signal, say): there until it moves
    if way.lead.is_none() && !stops.is_empty() && latest.speed.is_some_and(|v| v < STILL) {
        let pos = (latest.lat, latest.lon);
        let at = track.and_then(|tr| tr.route(pos, way.heading, &[], 0.0)).and_then(|r| r.0.first().copied()).unwrap_or(pos);
        return vec![(at.0, at.1, t0_ms), (at.0, at.1, until)];
    }
    // with nothing to come from, it starts where it was seen, now: never
    // ahead of it by however late the report came
    let start = if way.lead.is_some() { t0_ms } else { now_ms.max(t0_ms) };
    run(&way, start, now_ms, track, None)
}

/// How a train drawn apart from its newest report is brought back to it.
#[derive(Clone, Copy, Debug)]
enum Chase {
    /// Behind it: at the pace that reaches it by then (Unix ms).
    Reach(i64),
    /// Ahead of it: no faster than this (m/s) until then, so the train catches up.
    Ease(f64, i64),
}

/// A train drawn this far past where it is reported standing goes back there, metres.
const STOOD_PAST_M: f64 = 30.0;
/// Running ahead of its newest report, a train slows to no less than this
/// share of the reported speed while the real one catches up.
const EASE_FLOOR: f64 = 0.4;

/// How much faster than the track allows a train drawn behind may go to
/// catch up with where it was last seen.
const CATCH_UP: f64 = 1.35;
/// Braking into a station and pulling away again takes about this much
/// longer than running through it, seconds.
const STATION_TIME_S: f64 = 20.0;

/// Where a vehicle drawn along `frames` is at `t`: its place, speed (m/s)
/// and heading. None before the first frame or without two.
pub fn sample(frames: &[(f64, f64, i64)], t: i64) -> Option<((f64, f64), f64, Option<f64>)> {
    let first = frames.first()?;
    if frames.len() < 2 || t < first.2 {
        return None;
    }
    let i = frames.partition_point(|f| f.2 <= t).clamp(1, frames.len() - 1);
    let (a, b) = (frames[i - 1], frames[i]);
    let span = (b.2 - a.2) as f64;
    let k = if span > 0.0 { ((t - a.2) as f64 / span).clamp(0.0, 1.0) } else { 1.0 };
    let at = (a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k);
    let metres = distance((a.0, a.1), (b.0, b.1));
    let speed = if span > 0.0 { metres / (span / 1000.0) } else { 0.0 };
    let heading = (metres > 0.5).then(|| course((a.0, a.1), (b.0, b.1)));
    Some((at, speed, heading))
}

/// Frames for a train already drawn moving, on a new report: on from where
/// it is drawn (`from`, at `v0` m/s, heading so) along the track through the
/// newest position, paced to get there about `arrive_ms`, then on as usual.
/// No jump, and no new start a report behind.
#[allow(clippy::too_many_arguments)]
pub fn frames_chasing(
    from: (f64, f64),
    v0: f64,
    heading: Option<f64>,
    latest: &Fix,
    arrive_ms: i64,
    stops: &[Stop],
    now_ms: i64,
    track: Option<&dyn Track>,
) -> Vec<(f64, f64, i64)> {
    if now_ms - latest.t_ms > MAX_AGE_MS || !from.0.is_finite() || !from.1.is_finite() {
        return Vec::new();
    }
    let pos = (latest.lat, latest.lon);
    let next = match plan(pos, latest.speed, latest.heading, stops, latest.t_ms) {
        // it has stopped at the platform: in to it from where it is drawn (calling
        // at any station on the way the real train has already called at)
        Plan::Standing { at } => {
            let mut targets = between(from, heading, Some(v0), stops, at, now_ms);
            targets.push(target_of(stops, at, false));
            let way = Way { from, heading, targets, lead: None, next: at, speed: Some(v0) };
            return run(&way, now_ms, now_ms, track, None);
        }
        Plan::Moving { next } => next,
    };
    // reported standing (at a signal, say): in to where it stands, and there until it moves
    if latest.speed.is_some_and(|v| v < STILL) {
        // drawn past it already: back to where it stands (the map eases it there)
        let travel = latest.heading.filter(|h| h.is_finite()).or(heading);
        if distance(pos, from) > STOOD_PAST_M && travel.is_none_or(|h| turn(course(pos, from), h) < 90.0) {
            let until = now_ms + HORIZON_MS;
            let at = track.and_then(|tr| tr.route(pos, travel, &[], 0.0)).and_then(|r| r.0.first().copied()).unwrap_or(pos);
            return vec![(at.0, at.1, now_ms), (at.0, at.1, until)];
        }
        let here = Target { at: pos, pass: false, arrive_ms: None, dwell_ms: None, stop: None };
        let mut targets = between(from, heading, Some(v0), stops, next, now_ms);
        targets.push(here);
        let way = Way { from, heading, targets, lead: None, next, speed: Some(v0) };
        return run(&way, now_ms, now_ms, track, None);
    }
    // drawn ahead of where it was seen: on more slowly, for the real train to catch up
    let gap = distance(pos, from);
    let travel = latest.heading.filter(|h| h.is_finite()).or(heading);
    if let Some(h) = travel
        && (20.0..5_000.0).contains(&gap)
        && turn(course(pos, from), h) < 90.0
    {
        let reported = latest.speed.filter(|v| v.is_finite()).unwrap_or(v0).max(0.0);
        let secs = ((arrive_ms - now_ms) as f64 / 1000.0).max(1.0);
        let pace = ((reported * secs - gap) / secs).clamp(reported * EASE_FLOOR, reported.max(0.1));
        let way = Way { from, heading, targets: stop_targets(stops, next), lead: None, next, speed: Some(v0) };
        return run(&way, now_ms, now_ms, track, Some(Chase::Ease(pace, arrive_ms)));
    }
    // behind it: on through the stations the real train has called at since, to it, and on
    let mut targets = between(from, heading, Some(v0), stops, next, now_ms);
    let lead = targets.len();
    targets.push(report_target(pos));
    targets.extend(stop_targets(stops, next));
    let way = Way { from, heading, targets, lead: Some(lead), next, speed: Some(v0) };
    run(&way, now_ms, now_ms, track, Some(Chase::Reach(arrive_ms)))
}

/// The frames along a way from `t0_ms`; with `chase`, paced to reach the
/// way's lead (the newest report) by then.
fn run(way: &Way, t0_ms: i64, now_ms: i64, track: Option<&dyn Track>, chase: Option<Chase>) -> Vec<(f64, f64, i64)> {
    let until = now_ms.max(t0_ms) + HORIZON_MS;
    if way.targets.is_empty() && (way.heading.is_none() || way.speed.unwrap_or(0.0) <= 0.5) {
        return Vec::new();
    }
    let pos = way.from;
    // as fast as the train goes, at most
    let top = track.and_then(|tr| tr.top()).unwrap_or(TOP).min(TOP);
    let mut t = t0_ms;
    // as far as it could go by the horizon, and a little more
    let reach = way.speed.unwrap_or(0.0).max(TOP) * (until - t) as f64 / 1000.0 + 500.0;

    // the way: along the track through the places ahead, else straight to them
    let targets = way.points();
    let routed = track.and_then(|tr| tr.route(pos, way.heading, &targets, reach)).filter(|(p, _, _)| p.len() >= 2);
    let (points, reached, speeds) = match routed {
        Some(r) => r,
        None if !targets.is_empty() => {
            let mut points = vec![pos];
            points.extend(&targets);
            let at = Path::new(points.clone(), &[]).at;
            (points, at[1..].to_vec(), Vec::new())
        }
        None => (vec![pos, ahead(pos, way.heading.unwrap_or(0.0), reach)], Vec::new(), Vec::new()),
    };
    let path = Path::new(points, &speeds);
    let marks: Vec<Mark> = reached
        .iter()
        .zip(&way.targets)
        .map(|(at, t)| Mark { at: *at, arrive_ms: t.arrive_ms, pass: t.pass, dwell_ms: t.dwell_ms })
        .collect();
    // where it next stops: places it only passes don't slow it
    let stop_after = |from: usize| (from..marks.len()).find(|i| !marks[*i].pass);

    // where the track doesn't say, the timetable sets the pace
    let first_stop = stop_after(0).map(|i| &marks[i]);
    let paced = first_stop.map(|m| pace(m.at, m.arrive_ms, t));
    // not saying how fast: as fast as the track lets it go here
    let mut v = way.speed.unwrap_or_else(|| {
        let cruise = paced.unwrap_or(22.0).min(top);
        let at = first_stop.map(|m| m.at);
        allowed(&path, 0.0, path.limit(0.0).1.unwrap_or(cruise), STEP_MS as f64 / 1000.0, cruise, at, top)
    });
    let mut cruise = match paced {
        Some(p) if v > 3.0 => v.max(p.min(v * 1.2)),
        Some(p) => p,
        None => v,
    }
    .min(top);

    // catching up: the pace that gets it to the newest report by then
    let reach = match chase {
        Some(Chase::Reach(arrive)) => Some(arrive),
        _ => None,
    };
    let chase_pace = reach.zip(way.lead.and_then(|i| marks.get(i)).map(|m| m.at)).map(|(arrive, lead_at)| {
        // less the time it stands at stations on the way, and braking and pulling away there
        let calls: f64 = marks
            .iter()
            .filter(|m| m.at < lead_at)
            .filter_map(|m| m.dwell_ms)
            .map(|d| d as f64 / 1000.0 + STATION_TIME_S)
            .sum();
        let secs = ((arrive - t0_ms) as f64 / 1000.0 - calls).max(5.0);
        (lead_at, (lead_at / secs).min(top * CATCH_UP))
    });
    // ahead of the newest report: held back a while
    let eased = match chase {
        Some(Chase::Ease(pace, until)) => Some((pace, until)),
        _ => None,
    };

    let mut s = 0.0;
    let mut mark = 0;
    let mut out = vec![{
        let p = path.point(0.0);
        (p.0, p.1, t0_ms)
    }];
    let mut steps = 0;
    // standing at a station until then, before going on
    let mut dwell_until: Option<i64> = None;
    while t < until {
        t += STEP_MS;
        steps += 1;
        let dt = STEP_MS as f64 / 1000.0;
        if let Some(leave) = dwell_until {
            if t < leave {
                if steps % FRAME_EVERY == 0 {
                    push(&mut out, path.point(s), t);
                }
                continue;
            }
            dwell_until = None;
            push(&mut out, path.point(s), t);
        }
        // the places passed by now
        while marks.get(mark).is_some_and(|m| m.pass && m.at <= s) {
            mark += 1;
        }
        let stop = stop_after(mark).map(|i| (i, &marks[i]));
        if let Some((i, m)) = stop
            && m.at - s <= ARRIVED_M
        {
            s = m.at;
            v = 0.0;
            push(&mut out, path.point(s), t);
            match m.dwell_ms {
                // a station the real train has left: a while there, then on
                Some(d) => {
                    dwell_until = Some(t + d);
                    mark = i + 1;
                    continue;
                }
                // in: it stands at the platform from here on, until a report says otherwise
                None => break,
            }
        }
        let end = stop.map_or(path.len(), |(_, m)| m.at);
        if end - s <= 0.5 {
            break;
        }
        let limit = match chase_pace {
            // catching up with the newest report: its pace, braking only for a stop
            Some((lead_at, pace)) if s < lead_at => match stop {
                Some((_, m)) => pace.min((2.0 * BRAKE * (m.at - s - ARRIVED_M).max(0.0)).sqrt().max(CREEP)),
                None => pace,
            },
            _ => {
                let limit = allowed(&path, s, v, dt, cruise, stop.map(|(_, m)| m.at), top);
                match eased {
                    Some((pace, until)) if t < until => limit.min(pace),
                    _ => limit,
                }
            }
        };
        v = if v > limit { limit.max(v - 2.0 * BRAKE * dt) } else { (v + ACCEL * dt).min(limit) };
        s += (v * dt).min(end - s);
        // past the stop it was timed for, the next one sets the pace
        if let Some((i, m)) = stop
            && path.limit(s).1.is_none()
            && m.at < s + 1.0
            && let Some(n) = stop_after(i + 1).map(|j| &marks[j])
        {
            cruise = pace(n.at - s, n.arrive_ms, t).min(top);
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

    fn far_stop(from: (f64, f64)) -> Stop {
        let far = ahead(from, 0.0, 60_000.0);
        Stop { lat: far.0, lon: far.1, arrive_ms: Some(NOW + 3_600_000), depart_ms: None, pass: false }
    }

    #[test]
    fn without_a_speed_it_runs_as_fast_as_the_track_allows() {
        let start = (59.0, 18.0);
        let track = Fixed((vec![start, ahead(start, 0.0, 20_000.0)], Vec::new(), vec![Some(160.0 / 3.6)]));
        let f = frames_with(start.0, start.1, NOW, None, Some(0.0), &[far_stop(start)], NOW, Some(&track));
        let first = speed_between(f[0], f[1]);
        assert!((first - 160.0 / 3.6).abs() < 2.0, "at the limit from the start: {first:.1} m/s");
    }

    #[test]
    fn without_a_speed_it_is_slower_ahead_of_a_slow_section() {
        let start = (59.0, 18.0);
        let slow = ahead(start, 0.0, 300.0);
        let track = Fixed((
            vec![start, slow, ahead(start, 0.0, 20_000.0)],
            Vec::new(),
            vec![Some(160.0 / 3.6), Some(40.0 / 3.6)],
        ));
        let f = frames_with(start.0, start.1, NOW, None, Some(0.0), &[far_stop(start)], NOW, Some(&track));
        let first = speed_between(f[0], f[1]);
        let most = ((40.0f64 / 3.6).powi(2) + 2.0 * BRAKE * 300.0).sqrt();
        assert!(first > 5.0 && first <= most + 0.5, "slower for what is coming: {first:.1} m/s");
    }

    #[test]
    fn it_is_drawn_a_report_behind_and_goes_on_through_the_latest() {
        // seen at A, then 20 s later at B 500 m on: drawn from A, reaching B about 20 s later, not slowing there
        let a = (59.0, 18.0);
        let b = ahead(a, 0.0, 500.0);
        let prev = Fix { lat: a.0, lon: a.1, t_ms: NOW - 20_000, speed: None, heading: Some(0.0) };
        let latest = Fix { lat: b.0, lon: b.1, t_ms: NOW, speed: None, heading: Some(0.0) };
        let track = Fixed((vec![a, b, ahead(a, 0.0, 20_000.0)], vec![500.0], vec![Some(160.0 / 3.6), Some(160.0 / 3.6)]));
        let f = frames_between(&latest, Some(&prev), &[far_stop(a)], NOW, Some(&track));
        assert!(distance((f[0].0, f[0].1), a) < 1.0 && f[0].2 == NOW, "starts where it was before, now");
        let at_b = f.iter().find(|p| distance((p.0, p.1), a) >= 500.0).expect("goes past B");
        assert!((at_b.2 - (NOW + 20_000)).abs() <= 6_000, "at B about a report's time later: {} ms", at_b.2 - NOW);
        let speeds: Vec<f64> = f.windows(2).map(|w| speed_between(w[0], w[1])).collect();
        assert!(speeds.windows(2).all(|w| w[1] >= w[0] - 0.5), "never slows for the report: {speeds:?}");
        // the way the route is drawn starts there too
        let w = way(&latest, Some(&prev), &[far_stop(a)]).unwrap();
        assert_eq!((w.from, w.lead, w.targets[0].at), (a, Some(0), b));
    }

    #[test]
    fn a_new_report_carries_it_on_from_where_it_is_drawn() {
        // drawn at A going north at 50 m/s; the newest report puts it 900 m on, at B
        let a = (59.0, 18.0);
        let b = ahead(a, 0.0, 900.0);
        let latest = Fix { lat: b.0, lon: b.1, t_ms: NOW - 2_000, speed: Some(55.0), heading: Some(0.0) };
        let track = Fixed((vec![a, b, ahead(a, 0.0, 20_000.0)], vec![900.0], vec![Some(200.0 / 3.6), Some(200.0 / 3.6)]));
        let f = frames_chasing(a, 50.0, Some(0.0), &latest, NOW + 15_000, &[far_stop(a)], NOW, Some(&track));
        assert!(distance((f[0].0, f[0].1), a) < 1.0 && f[0].2 == NOW, "on from where it is, now");
        let first = speed_between(f[0], f[1]);
        assert!((first - 50.0).abs() < 8.0, "about the speed it had: {first:.1} m/s");
        let at_b = f.iter().find(|p| distance((p.0, p.1), a) >= 900.0).expect("reaches B");
        assert!((at_b.2 - (NOW + 15_000)).abs() <= 5_000, "about when asked: {} ms", at_b.2 - NOW);
        let jumps = f.windows(2).map(|w| speed_between(w[0], w[1])).fold(0.0, f64::max);
        assert!(jumps < 200.0 / 3.6 * CATCH_UP + 1.0, "never faster than it may catch up: {jumps:.1}");
    }

    #[test]
    fn frames_are_sampled_with_their_speed() {
        let a = (59.0, 18.0);
        let b = ahead(a, 0.0, 100.0);
        let f = [(a.0, a.1, NOW), (b.0, b.1, NOW + 4_000)];
        let (at, v, heading) = sample(&f, NOW + 2_000).unwrap();
        assert!((distance(at, a) - 50.0).abs() < 0.5);
        assert!((v - 25.0).abs() < 0.1);
        assert!(heading.unwrap() < 1.0 || heading.unwrap() > 359.0);
        assert!(sample(&f, NOW - 1).is_none());
    }

    #[test]
    fn a_train_reported_standing_stays_put() {
        // seen standing twice between stations (at a signal): it doesn't set off
        let here = (59.0, 18.0);
        let latest = Fix { lat: here.0, lon: here.1, t_ms: NOW - 5_000, speed: Some(0.0), heading: Some(0.0) };
        let f = frames_between(&latest, None, &[far_stop(here)], NOW, None);
        assert!(f.iter().all(|p| distance((p.0, p.1), here) < 1.0), "stays: {f:?}");
        // drawn moving towards it: it comes in to where it stands and stops there
        let behind = ahead(here, 180.0, 300.0);
        let track = Fixed((vec![behind, here, ahead(here, 0.0, 5_000.0)], vec![300.0], Vec::new()));
        let f = frames_chasing(behind, 15.0, Some(0.0), &latest, NOW + 15_000, &[far_stop(here)], NOW, Some(&track));
        let end = f.last().unwrap();
        assert!(distance((end.0, end.1), here) < 5.0, "stopped where it stands: {:.0} m off", distance((end.0, end.1), here));
        // drawn already past it: back to where it stands, not on up the line
        let past = ahead(here, 0.0, 800.0);
        let f = frames_chasing(past, 30.0, Some(0.0), &latest, NOW + 15_000, &[far_stop(here)], NOW, None);
        assert!(f.iter().all(|p| distance((p.0, p.1), here) < 1.0), "back where it stands");
    }

    #[test]
    fn drawn_ahead_it_eases_off_for_the_train_to_catch_up() {
        // seen at B doing 40 m/s, but drawn 400 m further on: it slows, without stopping
        let b = (59.0, 18.0);
        let drawn = ahead(b, 0.0, 400.0);
        let latest = Fix { lat: b.0, lon: b.1, t_ms: NOW - 1_000, speed: Some(40.0), heading: Some(0.0) };
        let f = frames_chasing(drawn, 40.0, Some(0.0), &latest, NOW + 20_000, &[far_stop(b)], NOW, None);
        let at_20s = f.iter().find(|p| p.2 >= NOW + 20_000).unwrap();
        let went = distance(drawn, (at_20s.0, at_20s.1));
        // the real train covers 800 m in 20 s; the drawn one about 400, so they meet
        assert!((250.0..600.0).contains(&went), "eased: {went:.0} m in 20 s");
        assert!(f.windows(2).all(|w| w[1].0 >= w[0].0 - 1e-9), "never backwards");
        assert!(f.windows(2).skip(1).all(|w| speed_between(w[0], w[1]) > 40.0 * EASE_FLOOR - 1.0), "never stops");
    }

    fn station(at: (f64, f64), arrive_ms: i64) -> Stop {
        Stop { lat: at.0, lon: at.1, arrive_ms: Some(arrive_ms), depart_ms: Some(arrive_ms + 60_000), pass: false }
    }

    #[test]
    fn a_late_train_still_calls_at_the_station_ahead() {
        // due at the station 3 minutes ago, but still 2 km short of it: it stops there
        let here = (59.0, 18.0);
        let st = ahead(here, 0.0, 2_000.0);
        let after = ahead(here, 0.0, 12_000.0);
        let stops = [station(st, NOW - 180_000), station(after, NOW + 600_000)];
        assert_eq!(plan(here, Some(30.0), Some(0.0), &stops, NOW), Plan::Moving { next: 0 });
        let f = frames(here.0, here.1, NOW, Some(30.0), Some(0.0), &stops, NOW);
        assert!(f.iter().any(|p| distance((p.0, p.1), st) < 1.0), "stops at the station");
        // and past it (300 m on, nearer the next), it is behind
        let past = ahead(st, 0.0, 400.0);
        assert_eq!(plan(past, None, None, &stops, NOW), Plan::Moving { next: 1 });
    }

    #[test]
    fn drawn_behind_a_station_the_train_has_left_it_still_calls_there() {
        // drawn 1 km before a station; the newest report is 1 km beyond it
        let drawn = (59.0, 18.0);
        let st = ahead(drawn, 0.0, 1_000.0);
        let seen = ahead(drawn, 0.0, 2_000.0);
        let after = ahead(drawn, 0.0, 20_000.0);
        let stops = [station(st, NOW - 60_000), station(after, NOW + 900_000)];
        let latest = Fix { lat: seen.0, lon: seen.1, t_ms: NOW - 2_000, speed: Some(25.0), heading: Some(0.0) };
        let f = frames_chasing(drawn, 25.0, Some(0.0), &latest, NOW + 90_000, &stops, NOW, None);
        let at_station: Vec<_> = f.iter().filter(|p| distance((p.0, p.1), st) < 1.0).collect();
        assert!(at_station.len() >= 2, "stands at the station a while");
        let stood = at_station.last().unwrap().2 - at_station[0].2;
        assert!((DWELL_MS.0..=DWELL_MS.2 + 5_000).contains(&stood), "{stood} ms");
        let end = f.last().unwrap();
        assert!(distance((end.0, end.1), drawn) > 1_020.0, "then on, away from the station");
    }
}
