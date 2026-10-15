//! Where trains (and ships) are between reports, as `frames` the map moves
//! them along so they glide instead of jumping.
//!
//! A train is worked out twice over, on the same stretch of track:
//!
//! - The *predicted* train is where the real one most likely is: from its
//!   last report, on along the track for the time since, at the speeds the
//!   track allows its kind of train, braking into the next station it calls
//!   at and standing there. It never leaves a station, or a place it was
//!   reported standing, on a guess (only a report moving it does), and never
//!   goes back.
//! - The *drawn* train is the one on the map. It follows the predicted one,
//!   gently: bounded acceleration and braking, never backwards, never past
//!   where the predicted one stands. A new report moves only the predicted
//!   train; the drawn one carries on from where it is drawn and closes the
//!   gap, so nothing jumps. It calls briefly at the stations the real train
//!   has called at since it was last drawn.
//!
//! Trains run only along the track: where it isn't known (not loaded yet)
//! a train waits where it is. Ships go straight on (`frames`).

/// A place at a time: `(lat, lon, unix_ms)`.
pub type Frame = (f64, f64, i64);

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

/// How far ahead the frames reach, past now.
pub const HORIZON_MS: i64 = 120_000;
/// A report older than this isn't carried on: better a still train than a guess.
const MAX_AGE_MS: i64 = 10 * 60_000;
/// The report before the latest says how it moved, when this recent.
const MAX_LAG_MS: i64 = 180_000;
const STEP_MS: i64 = 1000;
const DT: f64 = STEP_MS as f64 / 1000.0;
/// A frame every this many steps while moving (and at every stop and start).
const FRAME_EVERY: i64 = 3;

/// The predicted train, as a passenger train runs: m/s².
const ACCEL: f64 = 0.4;
const BRAKE: f64 = 0.5;
/// The drawn train, following it: a little livelier, so it keeps up, m/s².
const DRAWN_ACCEL: f64 = 0.5;
const DRAWN_BRAKE: f64 = 0.8;
/// The hardest the drawn train brakes, only when it must to stop in time, m/s².
const HARD_BRAKE: f64 = 1.2;
/// The drawn train runs up to this far behind the predicted one (where it
/// was this long before), so a new report moving the predicted train a
/// little changes little, ms.
const LAG_MS: i64 = 5_000;
/// Within that, it drifts back by its distance from there over this long, seconds.
const DRIFT_S: f64 = 60.0;
/// It closes the gap to there over about this long, seconds (far off,
/// no faster than it can ease out of again at `EASE`).
const FOLLOW_S: f64 = 5.0;
const EASE: f64 = 0.4;
/// Behind, the drawn train may run this much faster than the track allows to catch up,
const CATCH_UP: f64 = 1.4;
/// and accelerate this much harder (m/s²), all of it 300 m behind.
const CATCH_UP_ACCEL: f64 = 0.5;
/// Ahead, it slows to no less than this share of the predicted train's speed.
const AHEAD_FLOOR: f64 = 0.6;
/// Further behind than this it is put where the predicted train is, metres.
const RESET_M: f64 = 3_000.0;

/// Close enough to the station to have arrived, metres.
const ARRIVED_M: f64 = 3.0;
/// Standing this near a stop: it is at it, metres.
const AT_STOP_M: f64 = 400.0;
/// The slowest it rolls up to a station, m/s, so it gets there.
const CREEP: f64 = 1.5;
/// Slower than this, m/s, a train is standing.
const STILL: f64 = 1.0;
/// Moved this far between two reports, the way it went is seen, metres.
const SEEN_M: f64 = 20.0;
/// Pace limits when working it out from the timetable, m/s (30 to 200 km/h).
const PACE: (f64, f64) = (8.3, 55.0);
/// The fastest a train is taken to run: 200 km/h.
const TOP: f64 = 55.6;
/// A train seen running steadily below the limit is taken to keep on at
/// the same share of the limits ahead, at least this much of them.
const MIN_SHARE: f64 = 0.4;
/// The top speed taken for a train of unknown kind, unless seen faster: 90 km/h.
const UNKNOWN_TOP: f64 = 25.0;
/// The longest stretch of track worked out, metres.
const MAX_REACH_M: f64 = 25_000.0;
/// A place this near the way is on it, metres (the track beside it, or a report a little off).
const ON_PATH_M: f64 = 40.0;
/// A way to a place longer than this many times the distance (plus metres) goes round, not to it.
const DETOUR: (f64, f64) = (1.6, 600.0);
/// A stop the way on can't reach this near is behind it, metres.
const BEHIND_M: f64 = 2_000.0;
/// Arriving, past a stop's point by up to this it is still at its platform, metres.
const PLATFORM_M: f64 = 250.0;
/// Slower than this near its stop, a train is arriving there (not running through), m/s.
const ARRIVING_V: f64 = 10.0;
/// A call at a station the real train has already left, as drawn, ms: its
/// timetabled stop (default otherwise), kept short as the real train is ahead.
const DWELL_MS: (i64, i64, i64) = (15_000, 20_000, 25_000);

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

/// The point on the segment `a`-`b` nearest `p`, and how far along it is (0..1).
fn project(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> ((f64, f64), f64) {
    let k = p.0.to_radians().cos();
    let (dx, dy) = ((b.1 - a.1) * k, b.0 - a.0);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { ((((p.1 - a.1) * k) * dx + (p.0 - a.0) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    ((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t), t)
}

/// How far `p` is ahead of `from` going `heading` (degrees), metres; behind, less than 0.
fn ahead_of(p: (f64, f64), from: (f64, f64), heading: f64) -> f64 {
    let east = (p.1 - from.1).to_radians() * EARTH_M * from.0.to_radians().cos();
    let north = (p.0 - from.0).to_radians() * EARTH_M;
    let h = heading.to_radians();
    east * h.sin() + north * h.cos()
}

/// Adds a frame; one at the same moment as the last replaces it.
fn push(out: &mut Vec<Frame>, pos: (f64, f64), t: i64) {
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
/// reached (those reached, in order), and each segment's speed limit (m/s)
/// where known. See `rail::route`.
pub type Routed = (Vec<(f64, f64)>, Vec<f64>, Vec<Option<f64>>);

/// The railway, as far as the motion needs it (`rail::Rail`).
pub trait Track {
    /// The way from a point (heading roughly so) through these targets in
    /// order and on for so many metres; with `onwards_only`, only the way the
    /// heading points (else whichever way reaches the targets). None while
    /// the track there isn't known.
    fn route(
        &self,
        start: (f64, f64),
        heading: Option<f64>,
        targets: &[(f64, f64)],
        onwards_m: f64,
        onwards_only: bool,
    ) -> Option<Routed>;
    /// Where a train reported at `at`, standing at `platform`, is on the platform's track.
    fn hold(&self, at: (f64, f64), platform: (f64, f64)) -> Option<(f64, f64)>;
    /// The train's top speed, m/s, when known.
    fn top(&self) -> Option<f64> {
        None
    }
}

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
        for (i, from) in at.iter().enumerate().take(points.len().saturating_sub(1)) {
            let speed = speeds.get(i).copied().flatten().filter(|v| v.is_finite() && *v > 0.0);
            if limits.last().is_none_or(|l| l.1 != speed) {
                limits.push((*from, speed));
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

    /// How far along the way `p` is, if it is on it (within `within`
    /// metres), running within 90° of `heading` when given: where the way
    /// first comes that near past `after` metres (a way that passes twice
    /// takes the first).
    fn locate(&self, p: (f64, f64), within: f64, heading: Option<f64>, after: f64) -> Option<f64> {
        let mut best: Option<(f64, f64)> = None;
        let last = self.points.len().saturating_sub(1);
        for i in 1..self.points.len() {
            if self.at[i] < after {
                continue;
            }
            let (a, b) = (self.points[i - 1], self.points[i]);
            let (q, k) = project(p, a, b);
            let d = distance(p, q);
            let along = heading.is_none_or(|h| distance(a, b) < 0.5 || turn(h, course(a, b)) < 90.0);
            // beyond either end isn't on it
            let beyond = d > ARRIVED_M && ((i == 1 && k <= 0.0) || (i == last && k >= 1.0));
            if d <= within && along && !beyond {
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, self.at[i - 1] + (self.at[i] - self.at[i - 1]) * k));
                }
            } else if best.is_some() && d > within {
                break;
            }
        }
        best.map(|(_, s)| s)
    }
}

/// The fastest it may go `s` metres along by the track: the speed `target`
/// gives for the limit there and for every lower one ahead no closer than
/// it can slow for.
fn line_cap(path: &Path, s: f64, v: f64, target: impl Fn(Option<f64>) -> f64) -> f64 {
    let (section, limit) = path.limit(s);
    let mut cap = target(limit);
    let look = v.max(cap).powi(2) / (2.0 * BRAKE) + v * DT + 50.0;
    for l in path.limits.iter().skip(section + 1).take_while(|l| l.0 <= s + look) {
        let d = (l.0 - s).max(0.0);
        cap = cap.min((target(l.1).powi(2) + 2.0 * BRAKE * d).sqrt());
    }
    cap
}

/// The fastest it may go `room` metres short of where it stops.
fn stop_cap(room: f64) -> f64 {
    (2.0 * BRAKE * (room - ARRIVED_M).max(0.0)).sqrt().max(CREEP)
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
/// A stop is behind it when it is past it on the way to one of the next few
/// (nearer that one than the stop is), or, moving, when it lies behind it
/// close by: by where it is, not by the clock, so a late train still stops
/// at the station it hasn't reached.
pub fn plan(pos: (f64, f64), speed: Option<f64>, heading: Option<f64>, stops: &[Stop], t0_ms: i64) -> Plan {
    let speed = speed.filter(|s| s.is_finite() && *s >= 0.0);
    let moving = speed.is_none_or(|s| s > 3.0);
    let mut next = 0;
    while next + 1 < stops.len() {
        let here = (stops[next].lat, stops[next].lon);
        let near = distance(pos, here);
        // nearer one of the next few stops than it is (past more than one, the very next is behind too)
        let past = near > PASSED_M
            && stops[next + 1..(next + 4).min(stops.len())].iter().any(|s| {
                let then = (s.lat, s.lon);
                distance(pos, then) + PASSED_M < distance(here, then)
            });
        // seen moving away from it: it has left (a reported speed says so sooner)
        let left_m = if speed.is_some() { LEFT_M } else { PASSED_M };
        let behind = moving
            && near > left_m
            && near < 5_000.0
            && heading.filter(|h| h.is_finite()).is_some_and(|h| turn(course(pos, here), h) > 120.0);
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

/// Where a vehicle drawn along `frames` is at `t`: its place, speed (m/s)
/// and heading. None before the first frame or without two.
pub fn sample(frames: &[Frame], t: i64) -> Option<((f64, f64), f64, Option<f64>)> {
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

/// How fast a vehicle drawn along `frames` goes at `t`, m/s: each leg's
/// speed is its speed halfway along it, and between those it changes
/// evenly (what it was worked out with, not the legs' steps).
fn speed_at(frames: &[Frame], t: i64) -> Option<f64> {
    let leg = |j: usize| -> Option<(f64, f64)> {
        let (a, b) = (frames.get(j.checked_sub(1)?)?, frames.get(j)?);
        let span = (b.2 - a.2) as f64;
        (span > 0.0).then(|| ((a.2 + b.2) as f64 / 2.0, distance((a.0, a.1), (b.0, b.1)) / (span / 1000.0)))
    };
    let i = frames.partition_point(|f| f.2 <= t).clamp(1, frames.len().checked_sub(1)?);
    let (mid, v) = leg(i)?;
    let t = t as f64;
    let Some((other_mid, other_v)) = (if t < mid { leg(i - 1) } else { leg(i + 1) }) else { return Some(v) };
    // across a stand, the leg's own speed
    if other_v < 0.01 || (other_mid - mid).abs() < 1.0 {
        return Some(v);
    }
    let k = ((t - mid) / (other_mid - mid)).clamp(0.0, 1.0);
    Some(v + (other_v - v) * k)
}

/// A train as it is drawn at some moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drawn {
    pub at: (f64, f64),
    /// m/s.
    pub v: f64,
    /// The way it last moved, degrees; none if it never has.
    pub heading: Option<f64>,
    /// Standing there since (Unix ms); none while moving.
    pub still_since: Option<i64>,
    /// Standing: the last place it was drawn at before, coming in (it keeps
    /// the way it came by).
    pub came: Option<Frame>,
}

/// Where the train drawn along `frames` is at `now_ms`.
pub fn drawn_at(frames: &[Frame], now_ms: i64) -> Option<Drawn> {
    let first = *frames.first()?;
    let last = *frames.last()?;
    if !first.0.is_finite() || !first.1.is_finite() {
        return None;
    }
    let (at, mut v, mut heading) = sample(frames, now_ms).unwrap_or(((first.0, first.1), 0.0, None));
    if now_ms >= last.2 {
        v = 0.0;
    } else if v > 0.0 {
        v = speed_at(frames, now_ms).unwrap_or(v);
    }
    let upto = frames.partition_point(|f| f.2 <= now_ms);
    if heading.is_none() {
        let seen = &frames[..(upto + 1).min(frames.len())];
        heading = seen
            .windows(2)
            .rev()
            .find(|w| distance((w[0].0, w[0].1), (w[1].0, w[1].1)) > 0.5)
            .map(|w| course((w[0].0, w[0].1), (w[1].0, w[1].1)));
    }
    let here = |f: &&Frame| distance((f.0, f.1), at) < 0.5;
    let still_since = (v < 0.05).then(|| frames[..upto].iter().rev().take_while(here).last().map_or(now_ms, |f| f.2));
    let came = still_since.and_then(|_| frames[..upto].iter().rev().find(|f| !here(f)).copied());
    Some(Drawn { at, v, heading, still_since, came })
}

/// Frames for a ship: straight on along its heading at its speed, from a
/// report at `t0_ms` up to `now_ms` plus the horizon. Empty when it isn't
/// moving, or the report is too old.
pub fn frames(lat: f64, lon: f64, t0_ms: i64, speed: Option<f64>, heading: Option<f64>, now_ms: i64) -> Vec<Frame> {
    let (Some(v), Some(h)) = (speed.filter(|v| v.is_finite() && *v > 0.5), heading.filter(|h| h.is_finite())) else {
        return Vec::new();
    };
    if now_ms - t0_ms > MAX_AGE_MS || !lat.is_finite() || !lon.is_finite() {
        return Vec::new();
    }
    let until = now_ms.max(t0_ms) + HORIZON_MS;
    let mut out = Vec::new();
    let mut t = t0_ms;
    loop {
        let p = ahead((lat, lon), h, v * (t - t0_ms) as f64 / 1000.0);
        out.push((p.0, p.1, t));
        if t >= until {
            return out;
        }
        t = (t + 10 * STEP_MS).min(until);
    }
}

/// A train's frames, and (for the developer's map) its predicted ones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sim {
    /// The drawn train, from now (or from when it stopped, standing) to the horizon.
    pub frames: Vec<Frame>,
    /// Where the real train most likely is, from now to the horizon.
    pub predicted: Vec<Frame>,
}

impl Sim {
    fn still(at: (f64, f64), from: i64, until: i64) -> Sim {
        Sim { frames: vec![(at.0, at.1, from), (at.0, at.1, until)], predicted: Vec::new() }
    }
}

/// The real train, as its reports tell it.
struct Real {
    at: (f64, f64),
    t_ms: i64,
    /// m/s: reported, else from how far it went since the report before.
    speed: Option<f64>,
    heading: Option<f64>,
    /// The heading is seen (it moved between reports), not only reported.
    seen: bool,
    standing: bool,
    /// The first stop it hasn't left: the one it stands at, or the next on its way.
    next: usize,
    /// It stands at that stop's platform.
    at_stop: bool,
}

fn read(latest: &Fix, prev: Option<&Fix>, stops: &[Stop]) -> Real {
    let at = (latest.lat, latest.lon);
    let valid = |v: &f64| v.is_finite() && *v >= 0.0;
    let prev =
        prev.filter(|p| p.lat.is_finite() && p.lon.is_finite() && (1..=MAX_LAG_MS).contains(&(latest.t_ms - p.t_ms)));
    let moved = prev.map(|p| (distance((p.lat, p.lon), at), (latest.t_ms - p.t_ms) as f64 / 1000.0));
    let seen = prev.zip(moved).filter(|(_, m)| m.0 >= SEEN_M).map(|(p, _)| course((p.lat, p.lon), at));
    let heading = seen.or(latest.heading.filter(|h| h.is_finite()));
    let speed = latest.speed.filter(valid).or(moved.map(|(d, s)| d / s));
    let (mut next, at_stop) = match plan(at, speed, heading, stops, latest.t_ms) {
        Plan::Standing { at } => (at, true),
        Plan::Moving { next } => (next, false),
    };
    // stood at a station at the report before and now moving: it has left it
    if !at_stop
        && speed.is_some_and(|v| v >= STILL)
        && let Some(p) = prev
        && p.speed.is_some_and(|v| v < STILL)
        && let Some(i) = (next..stops.len())
            .take(2)
            .find(|&i| !stops[i].pass && distance((p.lat, p.lon), (stops[i].lat, stops[i].lon)) < AT_STOP_M)
    {
        next = i + 1;
    }
    // nothing says it moves: no speed, and no timetable to go by
    let standing = at_stop || speed.map_or(stops.is_empty(), |v| v < STILL);
    Real { at, t_ms: latest.t_ms, speed, heading, seen: seen.is_some(), standing, next, at_stop }
}

/// A place a way is found through: where, which stop (none: the report).
#[derive(Clone, Copy, Debug)]
struct Target {
    at: (f64, f64),
    stop: Option<usize>,
}

/// A stop on the way: how far along, which.
#[derive(Clone, Copy, Debug)]
struct Mark {
    at: f64,
    stop: usize,
}

/// The way found: the path, the targets reached on it (how far along), and
/// the leading targets left out as behind the start.
struct Found {
    path: Path,
    reached: Vec<(f64, Target)>,
    behind: Vec<Target>,
}

/// The way from `start` along the track through the targets in order, then
/// on for `onwards` metres. Leading targets it can't reach going on (a stop
/// it is just past) are left out as behind it; a target reached only the
/// long way round isn't counted reached.
fn find(
    track: &dyn Track,
    start: (f64, f64),
    heading: Option<f64>,
    targets: &[Target],
    onwards: f64,
    only: bool,
) -> Option<Found> {
    let mut skip = 0;
    loop {
        let rest = &targets[skip..];
        let points: Vec<(f64, f64)> = rest.iter().map(|t| t.at).collect();
        let (pts, reached, speeds) = track.route(start, heading, &points, onwards, only)?;
        if pts.len() < 2 {
            return None;
        }
        let first_ok =
            rest.is_empty() || reached.first().is_some_and(|s| *s <= distance(start, rest[0].at) * DETOUR.0 + DETOUR.1);
        if first_ok {
            let reached = reached.into_iter().zip(rest.iter().copied()).collect();
            return Some(beside(Path::new(pts, &speeds), reached, targets, skip));
        }
        if skip >= 3 || distance(start, rest[0].at) > BEHIND_M {
            // its stops can't be reached from here: just the way on
            let (pts, _, speeds) = track.route(start, heading, &[], onwards, only)?;
            return (pts.len() >= 2).then(|| beside(Path::new(pts, &speeds), Vec::new(), targets, targets.len()));
        }
        skip += 1;
    }
}

/// The way found, with the targets it didn't go through (the first `skipped`
/// left out as behind it, and those after the ones `reached`) placed where it
/// passes them: a stop whose platform is on the track beside the way (the one
/// for trains the other way, say) is still stopped at. Those it doesn't pass
/// near, of the skipped, are behind it.
fn beside(path: Path, routed: Vec<(f64, Target)>, targets: &[Target], skipped: usize) -> Found {
    let (first, rest) = targets.split_at(skipped.min(targets.len()));
    let mut reached = Vec::new();
    let mut behind = Vec::new();
    for t in first {
        match path.locate(t.at, ON_PATH_M, None, 0.0).filter(|s| *s > ARRIVED_M) {
            Some(s) => reached.push((s, *t)),
            None => behind.push(*t),
        }
    }
    let mut after = routed.last().map_or(0.0, |r| r.0);
    for t in &rest[routed.len().min(rest.len())..] {
        if let Some(s) = path.locate(t.at, ON_PATH_M, None, after) {
            reached.push((s, *t));
            after = s;
        }
    }
    reached.extend(routed);
    reached.sort_by(|a, b| a.0.total_cmp(&b.0));
    Found { path, reached, behind }
}

/// The stretch of track both trains are worked out on.
struct Layout {
    path: Path,
    /// Where the drawn train is, metres along; none: put it where the predicted one is now.
    d0: Option<f64>,
    /// Where the real train was reported, metres along (short of the start:
    /// on a way of its own beside this one, that far behind).
    p0: f64,
    /// The stops on the way, in order.
    marks: Vec<Mark>,
    /// Stops the way left out as behind its start, and how far each is from the report.
    behind: Vec<(f64, usize)>,
    /// The drawn train stands at this stop.
    standing_at: Option<usize>,
}

impl Layout {
    fn new(found: Found, d0: Option<f64>, p0: f64, report: (f64, f64), standing_at: Option<usize>) -> Layout {
        let marks = found.reached.iter().filter_map(|(at, t)| t.stop.map(|stop| Mark { at: *at, stop })).collect();
        let behind = found.behind.iter().filter_map(|t| t.stop.map(|s| (distance(report, t.at), s))).collect();
        Layout { path: found.path, d0, p0, marks, behind, standing_at }
    }
}

/// The stops from `from` a way is found through: up to the first it calls
/// at and one more, as far as a train could get, and a few at most.
fn ahead_targets(stops: &[Stop], from: usize, at: (f64, f64), reach: f64) -> Vec<Target> {
    let from = from.min(stops.len());
    let calls = stops[from..].iter().position(|s| !s.pass).map_or(stops.len(), |i| from + i + 2);
    (from..calls.min(from + 6).min(stops.len()))
        .take_while(|&i| distance(at, (stops[i].lat, stops[i].lon)) <= reach + 3_500.0)
        .map(|i| Target { at: (stops[i].lat, stops[i].lon), stop: Some(i) })
        .collect()
}

/// The stretch of track the trains are worked out on: from the drawn train
/// through the report when it is behind it, else from the report (through
/// the drawn train, ahead of it); else from the report alone, the drawn
/// train put anew.
fn layout(
    track: &dyn Track,
    drawn: Option<&Drawn>,
    real: &Real,
    stops: &[Stop],
    now_ms: i64,
    reach: f64,
) -> Option<Layout> {
    // where it is: on its platform's track when it stands at one
    let r_at = match (real.at_stop, stops.get(real.next)) {
        (true, Some(s)) => track.hold(real.at, (s.lat, s.lon)).unwrap_or(real.at),
        _ => real.at,
    };
    let ahead = ahead_targets(stops, real.next + usize::from(real.at_stop), r_at, reach);
    let from_report = || find(track, r_at, real.heading, &ahead, reach, real.seen);
    let Some(d) = drawn else {
        return from_report().map(|f| Layout::new(f, None, 0.0, r_at, None));
    };

    // the drawn train's next stop, or the one it stands at
    let (d_first, standing_at) = match plan(d.at, Some(d.v), d.heading, stops, now_ms) {
        // there already: on from it, unless the real train is still to call there
        Plan::Standing { at } if at < real.next => (at + 1, Some(at)),
        Plan::Standing { at } => (at, Some(at)),
        Plan::Moving { next } => (next, None),
    };
    // turning back: standing, and the real train moves off the other way
    let d_heading = match (d.heading, real.heading) {
        (Some(h), Some(r))
            if d.v < 0.5
                && !real.standing
                && real.seen
                && turn(h, r) > 120.0
                && distance(d.at, r_at) > 10.0
                && turn(course(d.at, r_at), r) < 90.0 =>
        {
            Some(r)
        }
        (h, _) => h,
    };
    let only = d_heading.is_some();
    // the stops between them, the real train's own when it stands at one, then those ahead
    let mut targets: Vec<Target> = (d_first..real.next.min(stops.len()))
        .take(8)
        .map(|i| Target { at: (stops[i].lat, stops[i].lon), stop: Some(i) })
        .collect();
    let lead = targets.len();
    if real.at_stop && d_first <= real.next {
        targets.push(Target { at: r_at, stop: Some(real.next) });
    }
    targets.extend(ahead.iter().filter(|t| t.stop.is_some_and(|i| i >= d_first)));

    // from the drawn train through the report: it is behind
    let on_drawn_way = |found: Found| -> Result<Layout, Found> {
        let heading = real.seen.then_some(real.heading).flatten();
        match found.path.locate(r_at, ON_PATH_M, heading, f64::NEG_INFINITY) {
            Some(p0) => Ok(Layout::new(found, Some(0.0), p0, r_at, standing_at)),
            None => Err(found),
        }
    };
    // the report as a place to go through
    let through_report = || -> Option<Layout> {
        let mut with = targets.clone();
        with.insert(lead, Target { at: r_at, stop: None });
        let found = find(track, d.at, d_heading, &with, reach, only)?;
        let p0 = found.reached.iter().find(|(_, t)| t.stop.is_none()).map(|r| r.0)?;
        Some(Layout::new(found, Some(0.0), p0, r_at, standing_at))
    };
    // from the report through the drawn train: it is ahead
    let on_report_way = |found: Found| -> Result<Layout, Found> {
        match found.path.locate(d.at, ON_PATH_M, d.heading, f64::NEG_INFINITY) {
            Some(d0) => Ok(Layout::new(found, Some(d0), 0.0, r_at, standing_at)),
            None => Err(found),
        }
    };
    // each on a way of its own (another branch to the same platform): the
    // drawn train on along its own, the real one as far from their next
    // station as it is along its way
    let side_by_side = |drawn_way: Found, report_way: &Found| -> Result<Layout, Found> {
        let calls = |t: &Target| t.stop.filter(|i| *i >= real.next && !stops[*i].pass);
        let common = drawn_way.reached.iter().find_map(|(at, t)| {
            let i = calls(t)?;
            let theirs = report_way.reached.iter().find(|(_, u)| u.stop == Some(i))?.0;
            Some(at - theirs)
        });
        match common {
            // (ahead of it, the real train starts short of the way's start)
            Some(p0) if p0 > -BEHIND_M && distance(d.at, r_at) < BEHIND_M => {
                Ok(Layout::new(drawn_way, Some(0.0), p0, r_at, standing_at))
            }
            _ => Err(drawn_way),
        }
    };
    let anew = |found: Found| Layout::new(found, None, 0.0, r_at, None);
    let ahead_of_it = real.heading.is_some_and(|h| distance(r_at, d.at) > 5.0 && turn(course(r_at, d.at), h) < 90.0);

    let mut report_way = None;
    if ahead_of_it {
        match from_report().map(on_report_way) {
            Some(Ok(l)) => return Some(l),
            Some(Err(found)) => report_way = Some(found),
            None => {}
        }
    }
    let mut drawn_way = None;
    match find(track, d.at, d_heading, &targets, reach, only).map(on_drawn_way) {
        Some(Ok(l)) => return Some(l),
        Some(Err(found)) => drawn_way = Some(found),
        None => {}
    }
    if let Some(l) = through_report() {
        return Some(l);
    }
    if !ahead_of_it {
        match from_report().map(on_report_way) {
            Some(Ok(l)) => return Some(l),
            Some(Err(found)) => report_way = Some(found),
            None => {}
        }
    }
    let report_way = report_way?;
    if let Some(drawn_way) = drawn_way
        && let Ok(l) = side_by_side(drawn_way, &report_way)
    {
        return Some(l);
    }
    // no way between them: put anew where the predicted train is
    Some(anew(report_way))
}

/// The predicted train along the layout, step by step from the report.
struct Predicted {
    t0: i64,
    s: Vec<f64>,
    v: Vec<f64>,
    /// Where it stands for good, and from when.
    hold: Option<f64>,
    stood_ms: Option<i64>,
}

impl Predicted {
    /// Where it is at `t`: metres along, m/s (before the report, on back at its speed then).
    fn at(&self, t: i64) -> (f64, f64) {
        if t < self.t0 {
            return (self.s[0] - self.v[0] * (self.t0 - t) as f64 / 1000.0, self.v[0]);
        }
        let k = (t - self.t0) as f64 / STEP_MS as f64;
        let i = (k.floor() as usize).min(self.s.len() - 1);
        let j = (i + 1).min(self.s.len() - 1);
        let f = (k - i as f64).clamp(0.0, 1.0);
        (self.s[i] + (self.s[j] - self.s[i]) * f, self.v[i] + (self.v[j] - self.v[i]) * f)
    }

    /// Whether it stands where it stays by `t`.
    fn stood(&self, t: i64) -> bool {
        self.stood_ms.is_some_and(|at| at <= t)
    }
}

/// Where the real train stops for good: where it stands; else the first
/// station on its way it calls at (one just behind the report as it pulls
/// in along the platform: a little on, as soon as it can stop).
fn hold_of(layout: &Layout, real: &Real, stops: &[Stop], v0: f64) -> Option<f64> {
    let p0 = layout.p0;
    if real.standing {
        return Some(p0);
    }
    let arriving = v0 < ARRIVING_V;
    let pulling_in = p0 + v0 * v0 / (2.0 * BRAKE);
    let calls = |i: usize| i >= real.next && stops.get(i).is_some_and(|s| !s.pass);
    if arriving && layout.behind.iter().any(|(d, i)| calls(*i) && *d < PLATFORM_M) {
        return Some(pulling_in);
    }
    for m in layout.marks.iter().filter(|m| calls(m.stop)) {
        if m.at > p0 + ARRIVED_M {
            return Some(m.at);
        }
        if arriving && p0 - m.at < PLATFORM_M {
            return Some(pulling_in);
        }
    }
    None
}

/// `share`: of each limit it runs at (as it was seen running).
fn predict(layout: &Layout, real: &Real, stops: &[Stop], until: i64, top: f64, cruise: f64, share: f64) -> Predicted {
    let path = &layout.path;
    let target = |limit: Option<f64>| limit.map_or(cruise, |l| l * share).min(top);
    let end = path.len();
    let mut s = layout.p0.min(end);
    let first_hold = hold_of(layout, real, stops, real.speed.unwrap_or(0.0)).map(|h| h.clamp(s, end));
    // not saying how fast: as fast as the track lets it go here
    let mut v = if real.standing {
        0.0
    } else {
        real.speed.map(|v| v.min(top * 1.1)).unwrap_or_else(|| {
            line_cap(path, s, cruise, target).min(first_hold.map_or(f64::INFINITY, |h| stop_cap(h - s)))
        })
    };
    let hold = if real.speed.is_none() { hold_of(layout, real, stops, v).map(|h| h.clamp(s, end)) } else { first_hold };
    let mut out = Predicted { t0: real.t_ms, s: vec![s], v: vec![v], hold, stood_ms: None };
    let stop_at = hold.unwrap_or(end);
    if stop_at - s <= ARRIVED_M {
        out.s[0] = stop_at;
        out.v[0] = 0.0;
        out.stood_ms = Some(real.t_ms);
        out.hold = Some(out.s[0]);
        return out;
    }
    let mut t = real.t_ms;
    while t < until {
        t += STEP_MS;
        if out.stood_ms.is_none() {
            let cap = line_cap(path, s, v, target).min(stop_cap(stop_at - s));
            v = if v > cap { cap.max(v - 2.0 * BRAKE * DT) } else { (v + ACCEL * DT).min(cap) };
            s += v * DT;
            if stop_at - s <= ARRIVED_M {
                s = stop_at;
                v = 0.0;
                out.stood_ms = Some(t);
                out.hold = Some(stop_at);
            }
        }
        out.s.push(s);
        out.v.push(v);
    }
    out
}

/// How long the drawn train calls at a station the real train has left.
fn dwell_of(stop: &Stop) -> i64 {
    match (stop.arrive_ms, stop.depart_ms) {
        (Some(a), Some(d)) => (d - a).clamp(DWELL_MS.0, DWELL_MS.2),
        _ => DWELL_MS.1,
    }
}

/// The drawn train along the layout from now, following the predicted one.
#[allow(clippy::too_many_arguments)]
fn follow(
    layout: &Layout,
    pred: &Predicted,
    drawn: Option<&Drawn>,
    real: &Real,
    stops: &[Stop],
    now_ms: i64,
    until: i64,
    top: f64,
    cruise: f64,
) -> Vec<Frame> {
    let path = &layout.path;
    // put anew: where the predicted train was a little before, never before the report
    let lag = if layout.d0.is_some() { LAG_MS } else { LAG_MS.min(now_ms - pred.t0).max(0) };
    let (mut s, mut v, since) = match (layout.d0, drawn) {
        (Some(d0), Some(d)) => (d0, d.v, d.still_since),
        _ => {
            let (s, v) = pred.at(now_ms - lag);
            (s, v, None)
        }
    };
    // calls at stations the real train has left since, on the way to it
    let calls = |i: usize| i < real.next && stops.get(i).is_some_and(|s| !s.pass);
    let mut dwells: Vec<(f64, i64)> = layout
        .marks
        .iter()
        .filter(|m| calls(m.stop) && m.at > s + ARRIVED_M && m.at < layout.p0)
        .map(|m| (m.at, dwell_of(&stops[m.stop])))
        .collect();
    // standing at one already: what is left of its call
    let mut wait_until = match (layout.standing_at, since) {
        (Some(i), Some(since)) if calls(i) && layout.d0.is_some() => Some(since + dwell_of(&stops[i])),
        _ => None,
    };
    // carried on from where it is drawn: exactly there, until the way gets past
    // it (its place on the way can be a little behind), so never a step back
    let (drawn_at_start, past) = match (layout.d0, drawn) {
        (Some(d0), Some(d)) => {
            let on = path.point(d0);
            let lead = ahead_of(d.at, on, course(on, path.point(d0 + 1.0)));
            (Some(d.at), d0 + lead.max(0.0))
        }
        _ => (None, f64::NEG_INFINITY),
    };
    let pos = |x: f64| match drawn_at_start {
        Some(at) if x <= past => at,
        _ => path.point(x),
    };
    let still_at_start = v < 0.05;
    let first_t = if still_at_start { since.unwrap_or(now_ms).min(now_ms) } else { now_ms };
    let mut out: Vec<Frame> =
        drawn.filter(|_| still_at_start && layout.d0.is_some()).and_then(|d| d.came).into_iter().collect();
    out.push({
        let p = pos(s);
        (p.0, p.1, first_t)
    });
    let mut was_still = still_at_start;
    // how far along the last frame is
    let mut pushed = s;
    let mut t = now_ms;
    let mut steps = 0;
    while t < until {
        t += STEP_MS;
        steps += 1;
        let before = s;
        if wait_until.is_some_and(|w| t < w) {
            v = 0.0;
        } else {
            wait_until = None;
            // where it should be, as this step starts: between where the
            // predicted train was a little before (`back`) and where it is
            let (back, v_back) = pred.at(t - STEP_MS - lag);
            let (front, v_front) = pred.at(t - STEP_MS);
            let stop_at = dwells.first().map(|d| d.0).into_iter().chain(pred.hold).fold(path.len(), f64::min);
            let room = stop_at - s;
            let gap = back - s;
            let slack = if gap > 0.0 { CATCH_UP } else { 1.0 };
            let line = line_cap(path, s, v, |l| l.unwrap_or(cruise).min(top) * slack);
            let stop = stop_cap(room);
            let cap = line.min(stop);
            let want = if room <= ARRIVED_M {
                0.0
            } else if pred.stood(t) {
                // the predicted train stands ahead: in at a train's pace
                cap
            } else if gap > 0.0 {
                // behind: catching up, easing off as it gets there
                v_back + (gap / FOLLOW_S).min((2.0 * EASE * gap).sqrt())
            } else if s <= front {
                // between: at the predicted train's pace, drifting back a little
                let k = if front > back { (s - back) / (front - back) } else { 1.0 };
                v_back + (v_front - v_back) * k - (s - back) / DRIFT_S
            } else if v_front > STILL {
                // ahead of it: slower, for it to catch up
                let ahead = s - front;
                (v_front - (ahead / FOLLOW_S).min((2.0 * EASE * ahead).sqrt())).max(v_front * AHEAD_FLOOR)
            } else {
                0.0
            };
            let want = want.clamp(0.0, cap);
            v = if want > v {
                // livelier the further behind it is
                let accel = DRAWN_ACCEL + CATCH_UP_ACCEL * (gap / 300.0).clamp(0.0, 1.0);
                (v + accel * DT).min(want)
            } else {
                // harder only when it must, to stop in time
                let need = if room > ARRIVED_M { v * v / (2.0 * (room - ARRIVED_M).max(1.0)) } else { HARD_BRAKE };
                (v - DRAWN_BRAKE.max(need.min(HARD_BRAKE)) * DT).max(want)
            };
            s += v * DT;
            if before < stop_at - ARRIVED_M + 1e-9 && s >= stop_at - ARRIVED_M {
                // in: at a station the real train has left, a while; where the predicted train stands, for good
                s = stop_at.max(before);
                v = 0.0;
                if dwells.first().is_some_and(|d| d.0 <= stop_at) {
                    let (_, dwell) = dwells.remove(0);
                    wait_until = Some(t + dwell);
                }
            }
            s = s.min(path.len());
        }
        let still = v < 0.01 && (s - before).abs() < 1e-6;
        if still && !was_still {
            push(&mut out, pos(s), t);
            pushed = s;
        }
        if !still && was_still {
            push(&mut out, pos(before), t - STEP_MS);
            pushed = before;
        }
        // through a bend, a frame before the line between frames leaves the track
        if !still && cuts_bend(path, pushed, s) {
            push(&mut out, pos(before), t - STEP_MS);
            pushed = before;
        }
        if !still && (steps % FRAME_EVERY == 0 || cuts_bend(path, pushed, s)) {
            push(&mut out, pos(s), t);
            pushed = s;
        }
        was_still = still;
    }
    push(&mut out, pos(s), until);
    out
}

/// The line between frames this far off the track at most, metres.
const BEND_M: f64 = 1.0;

/// Whether a line from `a` to `b` metres along cuts the bend between them by more than `BEND_M`.
fn cuts_bend(path: &Path, a: f64, b: f64) -> bool {
    let (pa, pb) = (path.point(a), path.point(b));
    [0.25, 0.5, 0.75].iter().any(|k| {
        let on = path.point(a + (b - a) * k);
        let chord = (pa.0 + (pb.0 - pa.0) * k, pa.1 + (pb.1 - pa.1) * k);
        distance(on, chord) > BEND_M
    })
}

/// The predicted train's frames from now, for the developer's map.
fn predicted_frames(path: &Path, pred: &Predicted, now_ms: i64, until: i64) -> Vec<Frame> {
    let mut out: Vec<Frame> = Vec::new();
    let mut t = now_ms;
    loop {
        let p = path.point(pred.at(t).0);
        // standing: only where it stood from and the end
        let same = out.len() >= 2 && {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            (a.0, a.1) == (b.0, b.1) && (b.0, b.1) == p
        };
        if same {
            out.pop();
        }
        out.push((p.0, p.1, t));
        if t >= until {
            return out;
        }
        t = (t + FRAME_EVERY * STEP_MS).min(until);
    }
}

/// A train's frames: where it is drawn from now to the horizon, along the
/// track, following where the real train most likely is (see the module).
/// `drawn` is how it was drawn so far, which it carries on from; `latest`
/// and `prev` its newest report and the one before (at another place).
pub fn simulate(
    drawn: Option<&[Frame]>,
    latest: &Fix,
    prev: Option<&Fix>,
    stops: &[Stop],
    now_ms: i64,
    track: Option<&dyn Track>,
) -> Sim {
    let until = now_ms + HORIZON_MS;
    let d = drawn.and_then(|f| drawn_at(f, now_ms));
    // where it waits when it can't be worked out
    let wait = |d: Option<&Drawn>, at: Option<(f64, f64)>| match d {
        Some(d) => Sim::still(d.at, d.still_since.unwrap_or(now_ms).min(now_ms), until),
        None => at.map_or_else(Sim::default, |at| Sim::still(at, now_ms, until)),
    };
    if !latest.lat.is_finite() || !latest.lon.is_finite() {
        return wait(d.as_ref(), None);
    }
    let report = (latest.lat, latest.lon);
    if now_ms - latest.t_ms > MAX_AGE_MS {
        return wait(d.as_ref(), None);
    }
    let Some(track) = track else { return wait(d.as_ref(), Some(report)) };
    let real = read(latest, prev, stops);
    let seen = real.speed.unwrap_or(0.0);
    let top = track.top().unwrap_or((seen * 1.1).max(UNKNOWN_TOP)).min(TOP);
    let reach = (top * (until - real.t_ms.min(now_ms)) as f64 / 1000.0 + 1_000.0).min(MAX_REACH_M);
    let Some(mut layout) = layout(track, d.as_ref(), &real, stops, now_ms, reach) else {
        // the track here isn't known (yet): on as it was drawn while it lasts, else it waits
        return match (drawn, d.as_ref()) {
            (Some(f), Some(d)) if d.v > 0.5 && f.last().is_some_and(|l| l.2 > now_ms) => {
                let from = f.partition_point(|x| x.2 <= now_ms).saturating_sub(1);
                Sim { frames: f[from..].to_vec(), predicted: Vec::new() }
            }
            _ => wait(d.as_ref(), Some(report)),
        };
    };
    // where the track doesn't say, it goes as fast as it was seen going, else at the timetable's pace
    let next_call = layout.marks.iter().find(|m| m.stop >= real.next && !stops[m.stop].pass);
    let cruise = real
        .speed
        .filter(|v| *v > 3.0)
        .unwrap_or_else(|| {
            next_call.map_or(22.0, |m| pace((m.at - layout.p0).max(0.0), stops[m.stop].arrive_ms, real.t_ms))
        })
        .min(top);
    // running steadily below the limit: on at the same share of the limits ahead
    let limit_here = layout.path.limit(layout.p0).1;
    let share = match (real.speed, prev.and_then(|p| p.speed), limit_here) {
        (Some(v), Some(before), Some(limit)) if v > 3.0 && v <= before + 1.0 && limit > 0.0 => {
            (v / limit.min(top)).clamp(MIN_SHARE, 1.0)
        }
        _ => 1.0,
    };
    let pred = predict(&layout, &real, stops, until, top, cruise, share);
    // far behind: put where the predicted train is
    if layout.d0.is_some_and(|d0| pred.at(now_ms - LAG_MS).0 - d0 > RESET_M) {
        layout.d0 = None;
    }
    let frames = follow(&layout, &pred, d.as_ref(), &real, stops, now_ms, until, top, cruise);
    let predicted = predicted_frames(&layout.path, &pred, now_ms, until);
    Sim { frames, predicted }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    const ORIGIN: (f64, f64) = (59.0, 18.0);

    /// A straight railway running north from `ORIGIN`, with speed limits
    /// by stretch (metres from the origin; m/s), the rest unknown.
    struct Rails {
        limits: Vec<(f64, f64, f64)>,
        loaded: bool,
    }

    fn rails(limit_kmh: f64) -> Rails {
        Rails { limits: vec![(-1e9, 1e9, limit_kmh / 3.6)], loaded: true }
    }

    /// The point `x` metres north of the origin along the railway, and back.
    fn at(x: f64) -> (f64, f64) {
        ahead(ORIGIN, 0.0, x)
    }
    fn x_of(p: (f64, f64)) -> f64 {
        (p.0 - ORIGIN.0).to_radians() * EARTH_M
    }

    impl Track for Rails {
        fn route(
            &self,
            start: (f64, f64),
            heading: Option<f64>,
            targets: &[(f64, f64)],
            onwards_m: f64,
            onwards_only: bool,
        ) -> Option<Routed> {
            if !self.loaded {
                return None;
            }
            let x0 = x_of(start);
            let by_heading = heading.map(|h| if turn(h, 0.0) < 90.0 { 1.0 } else { -1.0 });
            let to_first = targets.first().map(|t| if x_of(*t) >= x0 { 1.0 } else { -1.0 });
            let dir: f64 = if onwards_only { by_heading.or(to_first) } else { to_first.or(by_heading) }.unwrap_or(1.0);
            let mut reached = Vec::new();
            let mut end = x0;
            for t in targets {
                let xt = x_of(*t);
                // behind, or on another track beside this one: not reached
                if (xt - end) * dir < -0.5 || distance(*t, at(xt)) > 3.0 {
                    break;
                }
                end = xt;
                reached.push((xt - x0).abs());
            }
            end += dir * onwards_m.max(1.0);
            let n = ((end - x0).abs() / 50.0).ceil().max(1.0) as usize;
            let xs: Vec<f64> = (0..=n).map(|i| x0 + (end - x0) * i as f64 / n as f64).collect();
            let speeds = xs
                .windows(2)
                .map(|w| {
                    let mid = (w[0] + w[1]) / 2.0;
                    self.limits.iter().find(|l| l.0 <= mid && mid < l.1).map(|l| l.2)
                })
                .collect();
            Some((xs.into_iter().map(at).collect(), reached, speeds))
        }

        fn hold(&self, at: (f64, f64), _: (f64, f64)) -> Option<(f64, f64)> {
            Some(at)
        }

        fn top(&self) -> Option<f64> {
            Some(200.0 / 3.6)
        }
    }

    fn fix(x: f64, t_ms: i64, speed: Option<f64>) -> Fix {
        let p = at(x);
        Fix { lat: p.0, lon: p.1, t_ms, speed, heading: Some(0.0) }
    }

    fn call(x: f64, arrive_ms: i64) -> Stop {
        let p = at(x);
        Stop { lat: p.0, lon: p.1, arrive_ms: Some(arrive_ms), depart_ms: Some(arrive_ms + 60_000), pass: false }
    }

    /// Far ahead: a stop that doesn't come into it.
    fn far() -> Stop {
        call(80_000.0, NOW + 3_600_000)
    }

    fn sim(
        drawn: Option<&[Frame]>,
        latest: &Fix,
        prev: Option<&Fix>,
        stops: &[Stop],
        now: i64,
        track: &Rails,
    ) -> Vec<Frame> {
        simulate(drawn, latest, prev, stops, now, Some(track as &dyn Track)).frames
    }

    /// Where along the railway the frames put it every second from `from` to their end.
    fn trace(f: &[Frame], from: i64) -> Vec<(i64, f64)> {
        let end = f.last().unwrap().2;
        (0..)
            .map(|k| from + k * 1000)
            .take_while(|t| *t <= end)
            .map(|t| {
                let (p, _, _) = sample(f, t).unwrap_or(((f[0].0, f[0].1), 0.0, None));
                (t, x_of(p))
            })
            .collect()
    }

    /// Speeds over each frame-to-frame leg.
    fn legs(f: &[Frame]) -> Vec<f64> {
        f.windows(2)
            .filter(|w| w[1].2 > w[0].2)
            .map(|w| distance((w[0].0, w[0].1), (w[1].0, w[1].1)) / ((w[1].2 - w[0].2) as f64 / 1000.0))
            .collect()
    }

    fn never_back(f: &[Frame]) {
        let xs = trace(f, f[0].2);
        assert!(xs.windows(2).all(|w| w[1].1 >= w[0].1 - 0.01), "never backwards: {xs:?}");
        assert!(f.windows(2).all(|w| w[0].2 < w[1].2), "time runs forward: {f:?}");
    }

    /// The largest change of speed from one leg to the next, per second;
    /// coming to a stand from a crawl doesn't count.
    fn jerkiest(f: &[Frame]) -> f64 {
        let times: Vec<f64> =
            f.windows(2).filter(|w| w[1].2 > w[0].2).map(|w| (w[1].2 - w[0].2) as f64 / 1000.0).collect();
        legs(f)
            .windows(2)
            .zip(times.windows(2))
            .filter(|(v, _)| !(v[0].min(v[1]) < 0.01 && v[0].max(v[1]) <= 3.0))
            .map(|(v, t)| (v[1] - v[0]).abs() / ((t[0] + t[1]) / 2.0))
            .fold(0.0, f64::max)
    }

    #[test]
    fn ships_carry_on_at_their_speed() {
        let f = frames(59.0, 18.0, NOW - 10_000, Some(20.0), Some(90.0), NOW);
        let (first, last) = (f[0], *f.last().unwrap());
        assert_eq!(last.2, NOW + HORIZON_MS);
        let v = distance((first.0, first.1), (last.0, last.1)) / ((last.2 - first.2) as f64 / 1000.0);
        assert!((v - 20.0).abs() < 0.1, "{v}");
        assert!(last.1 > 18.0 && (last.0 - 59.0).abs() < 0.01, "east");
        assert!(frames(59.0, 18.0, NOW, Some(0.0), Some(90.0), NOW).is_empty(), "standing still");
        assert!(frames(59.0, 18.0, NOW - 20 * 60_000, Some(20.0), Some(90.0), NOW).is_empty(), "too old");
    }

    #[test]
    fn it_is_drawn_where_the_real_train_is_by_now() {
        // seen 30 s ago at 0 doing 30 m/s on a 108 km/h line: by now 900 m on, drawn where it was 5 s ago
        let track = rails(108.0);
        let f = sim(None, &fix(0.0, NOW - 30_000, Some(30.0)), None, &[far()], NOW, &track);
        assert_eq!(f[0].2, NOW);
        let x = x_of((f[0].0, f[0].1));
        assert!((x - 750.0).abs() < 5.0, "{x}");
        let v = legs(&f)[0];
        assert!((v - 30.0).abs() < 0.5, "{v}");
        never_back(&f);
    }

    #[test]
    fn it_runs_up_to_the_limit_and_slows_before_a_slower_stretch() {
        // 160 km/h for 4 km, then 80
        let track = Rails { limits: vec![(-1e9, 4_000.0, 160.0 / 3.6), (4_000.0, 1e9, 80.0 / 3.6)], loaded: true };
        let f = sim(None, &fix(0.0, NOW, Some(30.0)), None, &[far()], NOW, &track);
        let speeds: Vec<(f64, f64)> = f.windows(2).map(|w| (x_of((w[1].0, w[1].1)), legs(w)[0])).collect();
        let top = speeds.iter().map(|s| s.1).fold(0.0, f64::max);
        assert!(top > 160.0 / 3.6 - 3.0 && top < 160.0 / 3.6 + 0.5, "up to the limit: {top:.1}");
        assert!(speeds.iter().all(|(x, v)| *x < 4_050.0 || *v < 80.0 / 3.6 + 0.5), "slow past the change: {speeds:?}");
        assert!(jerkiest(&f) <= DRAWN_BRAKE + 0.1, "gently: {}", jerkiest(&f));
    }

    #[test]
    fn without_a_speed_it_goes_as_fast_as_the_track_allows_slower_ahead_of_a_slow_stretch() {
        let track = rails(160.0);
        let f = sim(None, &fix(0.0, NOW, None), None, &[far()], NOW, &track);
        assert!((legs(&f)[0] - 160.0 / 3.6).abs() < 1.0, "{:?}", legs(&f));
        let track = Rails { limits: vec![(-1e9, 300.0, 160.0 / 3.6), (300.0, 1e9, 40.0 / 3.6)], loaded: true };
        let f = sim(None, &fix(0.0, NOW, None), None, &[far()], NOW, &track);
        let most = ((40.0f64 / 3.6).powi(2) + 2.0 * BRAKE * 300.0).sqrt();
        assert!(legs(&f)[0] <= most + 0.5, "{:?}", legs(&f));
    }

    #[test]
    fn it_stops_at_the_next_station_and_never_leaves_on_a_guess() {
        // 1.5 km from the station at 25 m/s; due to leave it long before the horizon
        let track = rails(120.0);
        let stops = [call(1_500.0, NOW + 60_000), far()];
        let f = sim(None, &fix(0.0, NOW, Some(25.0)), None, &stops, NOW, &track);
        let xs = trace(&f, NOW);
        let arrived = xs.iter().position(|(_, x)| (x - 1_500.0).abs() < 0.5).expect("gets there");
        assert!(xs[arrived..].iter().all(|(_, x)| (x - 1_500.0).abs() < 0.5), "stays");
        assert!(jerkiest(&f) <= DRAWN_BRAKE + 0.1, "brakes gently: {}", jerkiest(&f));
        never_back(&f);
        // and stood there past its departure
        let f = sim(None, &fix(1_500.0, NOW, Some(0.0)), None, &stops, NOW + 300_000, &track);
        assert!(trace(&f, NOW + 300_000).iter().all(|(_, x)| (x - 1_500.0).abs() < 0.5));
    }

    #[test]
    fn it_stops_at_a_station_whose_platform_is_on_the_track_beside() {
        // the station's platform is on the other track, 5 m to the side: never reached, still stopped at
        let track = rails(120.0);
        let beside = ahead(at(1_500.0), 90.0, 5.0);
        let station = Stop { lat: beside.0, lon: beside.1, ..call(1_500.0, NOW + 60_000) };
        let f = sim(None, &fix(0.0, NOW, Some(25.0)), None, &[station, far()], NOW, &track);
        let xs = trace(&f, NOW);
        let end = xs.last().unwrap().1;
        assert!((end - 1_500.0).abs() < 5.0, "stops there: {end}");
        assert!(xs.iter().all(|(_, x)| *x < 1_505.0), "not on past it");
    }

    #[test]
    fn reported_standing_it_stays_put_anywhere() {
        let track = rails(120.0);
        let f = sim(None, &fix(700.0, NOW - 60_000, Some(0.0)), None, &[far()], NOW, &track);
        assert!(f.iter().all(|p| (x_of((p.0, p.1)) - 700.0).abs() < 0.5), "{f:?}");
    }

    #[test]
    fn a_new_report_moves_it_on_from_where_it_is_drawn_without_a_jump() {
        let track = rails(120.0);
        let stops = [far()];
        let first = sim(None, &fix(0.0, NOW, Some(30.0)), None, &stops, NOW, &track);
        // 20 s on, the real train turns out to be 150 m behind where it was drawn
        let now = NOW + 20_000;
        let drawn_x = x_of(sample(&first, now).unwrap().0);
        let latest = fix(drawn_x - 150.0, now - 2_000, Some(28.0));
        let next = sim(Some(&first[..]), &latest, Some(&fix(0.0, NOW, Some(30.0))), &stops, now, &track);
        assert_eq!(next[0].2, now);
        assert!((x_of((next[0].0, next[0].1)) - drawn_x).abs() < 0.5, "from where it was drawn");
        never_back(&next);
        assert!(jerkiest(&next) <= HARD_BRAKE + 0.1, "{}", jerkiest(&next));
        // it lets the real train catch up, and runs with it, a little behind
        let before = fix(0.0, NOW, Some(30.0));
        let predicted =
            simulate(Some(&first[..]), &latest, Some(&before), &stops, now, Some(&track as &dyn Track)).predicted;
        let end = x_of((next.last().unwrap().0, next.last().unwrap().1));
        let lagged = x_of(sample(&predicted, now + HORIZON_MS - LAG_MS).unwrap().0);
        let front = x_of((predicted.last().unwrap().0, predicted.last().unwrap().1));
        assert!(end > lagged - 30.0 && end < front + 1.0, "{end} between {lagged} and {front}");
    }

    #[test]
    fn behind_the_real_train_it_catches_up_smoothly() {
        let track = rails(120.0);
        let stops = [far()];
        let first = sim(None, &fix(0.0, NOW, Some(30.0)), None, &stops, NOW, &track);
        let now = NOW + 20_000;
        let drawn_x = x_of(sample(&first, now).unwrap().0);
        // it is 800 m further on than drawn
        let latest = fix(drawn_x + 800.0, now, Some(33.0));
        let s = simulate(Some(&first[..]), &latest, None, &stops, now, Some(&track as &dyn Track));
        let f = &s.frames;
        assert!((x_of((f[0].0, f[0].1)) - drawn_x).abs() < 0.5, "no jump");
        never_back(f);
        assert!(legs(f).iter().all(|v| *v <= 120.0 / 3.6 * CATCH_UP + 0.5), "{:?}", legs(f));
        assert!(jerkiest(f) <= DRAWN_ACCEL + CATCH_UP_ACCEL + 0.1, "{}", jerkiest(f));
        let gap = |t: i64| x_of(sample(&s.predicted, t).unwrap().0) - x_of(sample(f, t).unwrap().0);
        // caught up: no further behind than where it was 5 s before
        assert!(
            gap(now) > 790.0 && gap(now + HORIZON_MS - 1) < 120.0 / 3.6 * LAG_MS as f64 / 1000.0 + 30.0,
            "{} then {}",
            gap(now),
            gap(now + HORIZON_MS - 1)
        );
    }

    #[test]
    fn drawn_past_where_it_now_stands_it_stops_and_never_goes_back() {
        let track = rails(120.0);
        let stops = [far()];
        let first = sim(None, &fix(0.0, NOW, Some(30.0)), None, &stops, NOW, &track);
        let now = NOW + 20_000;
        let drawn_x = x_of(sample(&first, now).unwrap().0);
        // it stopped 100 m short of where it is drawn
        let latest = fix(drawn_x - 100.0, now, Some(0.0));
        let f = sim(Some(&first[..]), &latest, Some(&fix(0.0, NOW, Some(30.0))), &stops, now, &track);
        never_back(&f);
        let end = x_of((f.last().unwrap().0, f.last().unwrap().1));
        assert!(end > drawn_x && end < drawn_x + 600.0, "stopped soon: {end} from {drawn_x}");
        assert!(jerkiest(&f) <= HARD_BRAKE + 0.1, "{}", jerkiest(&f));
    }

    #[test]
    fn it_calls_at_a_station_the_real_train_has_passed_since() {
        // drawn at 0 heading for the station at 1 km; the real train is reported 600 m past it
        let track = rails(100.0);
        let stops = [call(1_000.0, NOW + 30_000), far()];
        let first = sim(None, &fix(0.0, NOW, Some(25.0)), None, &stops, NOW, &track);
        let now = NOW + 10_000;
        let latest = fix(1_600.0, now, Some(20.0));
        let f = sim(Some(&first[..]), &latest, Some(&fix(1_100.0, now - 20_000, Some(5.0))), &stops, now, &track);
        never_back(&f);
        let xs = trace(&f, now);
        let at_station: Vec<i64> = xs.iter().filter(|(_, x)| (x - 1_000.0).abs() < 0.5).map(|(t, _)| *t).collect();
        assert!(!at_station.is_empty(), "calls there");
        let stood = at_station.last().unwrap() - at_station[0];
        assert!(stood >= DWELL_MS.0 - 1_000 && stood <= DWELL_MS.2 + 1_000, "briefly: {stood} ms");
        assert!(xs.last().unwrap().1 > 1_300.0, "and on after the real train: {xs:?}");
        assert!(jerkiest(&f) <= DRAWN_ACCEL + CATCH_UP_ACCEL + 0.1, "{}", jerkiest(&f));
    }

    #[test]
    fn a_train_moving_off_from_a_station_isnt_stopped_there_again() {
        // stood at the station; now moving off, still short of its point
        let track = rails(100.0);
        let stops = [call(1_000.0, NOW - 120_000), far()];
        let prev = fix(950.0, NOW - 30_000, Some(0.0));
        let f = sim(None, &fix(980.0, NOW, Some(5.0)), Some(&prev), &stops, NOW, &track);
        assert!(x_of((f.last().unwrap().0, f.last().unwrap().1)) > 1_500.0, "goes on");
    }

    #[test]
    fn arriving_along_the_platform_past_its_point_it_stops_soon() {
        let track = rails(100.0);
        let stops = [call(1_000.0, NOW), far()];
        let prev = fix(800.0, NOW - 20_000, Some(12.0));
        let f = sim(None, &fix(1_040.0, NOW, Some(4.0)), Some(&prev), &stops, NOW, &track);
        let end = x_of((f.last().unwrap().0, f.last().unwrap().1));
        assert!(end > 1_040.0 && end < 1_100.0, "stops soon: {end}");
    }

    #[test]
    fn it_heads_for_its_next_station_whatever_the_reported_heading() {
        // the report says south, the next station is north and nothing has been seen moving
        let track = rails(100.0);
        let mut latest = fix(0.0, NOW, Some(20.0));
        latest.heading = Some(180.0);
        let f = sim(None, &latest, None, &[call(5_000.0, NOW + 300_000), far()], NOW, &track);
        assert!(x_of((f.last().unwrap().0, f.last().unwrap().1)) > 1_000.0);
    }

    #[test]
    fn without_the_track_it_waits_or_carries_on_as_drawn() {
        let track = rails(100.0);
        let first = sim(None, &fix(0.0, NOW, Some(20.0)), None, &[far()], NOW, &track);
        let none = Rails { loaded: false, ..rails(100.0) };
        let f = sim(None, &fix(0.0, NOW, Some(20.0)), None, &[far()], NOW, &none);
        assert!(f.iter().all(|p| x_of((p.0, p.1)).abs() < 0.5), "waits where seen");
        let now = NOW + 10_000;
        let f = sim(Some(&first[..]), &fix(150.0, now, Some(20.0)), None, &[far()], now, &none);
        let drawn = sample(&first, now).unwrap().0;
        assert!(distance(sample(&f, now).unwrap().0, drawn) < 0.5, "on as drawn");
        assert!(x_of((f.last().unwrap().0, f.last().unwrap().1)) > x_of(drawn) + 1_000.0);
    }

    #[test]
    fn far_behind_it_is_put_where_the_real_train_is() {
        let track = rails(120.0);
        let first = sim(None, &fix(0.0, NOW, Some(30.0)), None, &[far()], NOW, &track);
        let now = NOW + 10_000;
        let f = sim(Some(&first[..]), &fix(5_000.0, now, Some(30.0)), None, &[far()], now, &track);
        assert!((x_of((f[0].0, f[0].1)) - 5_000.0).abs() < 5.0);
    }

    #[test]
    fn standing_it_remembers_since_when() {
        let track = rails(120.0);
        let stops = [call(500.0, NOW + 30_000), far()];
        let first = sim(None, &fix(0.0, NOW, Some(15.0)), None, &stops, NOW, &track);
        let xs = trace(&first, NOW);
        let arrived = xs.iter().find(|(_, x)| (x - 500.0).abs() < 0.5).unwrap().0;
        let later = arrived + 30_000;
        let d = drawn_at(&first, later).unwrap();
        assert_eq!((d.v, d.still_since), (0.0, Some(arrived)));
        assert!(d.heading.is_some_and(|h| turn(h, 0.0) < 1.0));
        // carried on through a new simulation
        let again = sim(Some(&first[..]), &fix(500.0, later, Some(0.0)), None, &stops, later, &track);
        assert_eq!(drawn_at(&again, later + 10_000).and_then(|d| d.still_since), Some(arrived));
        assert!(
            drawn_at(&again, later).and_then(|d| d.heading).is_some_and(|h| turn(h, 0.0) < 1.0),
            "and the way it came"
        );
    }

    /// The real train: stands at 0, runs at 30 m/s (accelerating at 0.4, braking at 0.5) to
    /// a station at 6 km, stands there a minute, then on to the next at 14 km. Where it
    /// is and how fast, every 100 ms from 0, and when it arrives at and leaves 6 km.
    fn real_run() -> (Vec<(f64, f64)>, f64, f64) {
        let mut out = Vec::new();
        let (mut x, mut v, mut t) = (0.0f64, 0.0f64, 0.0f64);
        let (mut arrived, mut left) = (f64::NAN, f64::NAN);
        let mut target = 6_000.0;
        let mut wait_until = f64::NAN;
        while t < 900.0 {
            out.push((x, v));
            let dt = 0.1;
            if wait_until.is_finite() {
                if t >= wait_until {
                    wait_until = f64::NAN;
                    left = t;
                    target = 14_000.0;
                }
            } else if x < target {
                let room = target - x;
                let brake = (2.0 * 0.5 * room).sqrt();
                v = if v > brake { (v - 0.5 * dt).max(0.0) } else { (v + 0.4 * dt).min(30.0).min(brake.max(0.3)) };
                x = (x + v * dt).min(target);
                if target - x < 0.01 {
                    x = target;
                    v = 0.0;
                    if target == 6_000.0 {
                        arrived = t;
                        wait_until = t + 60.0;
                    }
                }
            }
            t += dt;
        }
        (out, arrived, left)
    }

    #[test]
    fn a_run_between_stations_is_smooth_close_and_waits_for_the_real_train() {
        run_between_stations(true, (400.0, 120.0));
    }

    #[test]
    fn a_run_reported_without_speeds_too() {
        run_between_stations(false, (450.0, 150.0));
    }

    /// The real run, reported every 5 to 30 s (with its speed, or not), drawn
    /// as the map does: anew on every report and every 10 s. Off by at most
    /// `off.0` metres, `off.1` on average.
    fn run_between_stations(with_speed: bool, off: (f64, f64)) {
        let track = rails(120.0);
        let (real, arrived, left) = real_run();
        let real_at = |t: i64| real[((t - NOW) / 100) as usize];
        // it starts from a station, as every run does
        let start = Stop { depart_ms: Some(NOW), ..call(0.0, NOW - 60_000) };
        let stops = [start, call(6_000.0, NOW + 260_000), call(14_000.0, NOW + 650_000)];
        // reports every 5 to 30 s, each heard 2 s after it was made
        let gaps = [12, 25, 8, 30, 15, 20, 5, 28, 18, 10];
        let mut times = vec![NOW];
        while *times.last().unwrap() < NOW + 880_000 {
            times.push(times.last().unwrap() + gaps[times.len() % gaps.len()] * 1000);
        }
        let report = |t: i64| {
            let (x, v) = real_at(t);
            let mut f = fix(x, t, with_speed.then_some(v));
            f.heading = None;
            f
        };
        let (mut frames, mut latest, mut prev): (Option<Vec<Frame>>, Option<Fix>, Option<Fix>) = (None, None, None);
        let (mut heard, mut last_sim) = (0, i64::MIN);
        let mut drawn: Vec<(i64, f64)> = Vec::new();
        let mut moving_report_after_stop: Option<i64> = None;
        for now in (NOW..NOW + 880_000).step_by(1000) {
            let mut fresh = false;
            while heard < times.len() && times[heard] + 2_000 <= now {
                let r = report(times[heard]);
                heard += 1;
                match latest {
                    // its place unchanged: still the same report, unless it says it stopped
                    Some(l) if distance((l.lat, l.lon), (r.lat, r.lon)) < 2.0 => {
                        // (without a speed, the time it has stood there says it stopped)
                        let r = Fix { speed: r.speed.or((r.t_ms - l.t_ms >= 5_000).then_some(0.0)), ..r };
                        if r.speed.is_some_and(|v| v < 1.0) {
                            latest = Some(r);
                            fresh = true;
                        }
                    }
                    l => {
                        prev = l;
                        latest = Some(r);
                        fresh = true;
                    }
                }
                let t_real = (r.t_ms - NOW) as f64 / 1000.0;
                if t_real > left && moving_report_after_stop.is_none() {
                    moving_report_after_stop = Some(now);
                }
            }
            let Some(l) = latest else { continue };
            if fresh || now - last_sim >= 10_000 {
                let next = sim(frames.as_deref(), &l, prev.as_ref(), &stops, now, &track);
                if let Some(old) = &frames {
                    let (a, b) =
                        (sample(old, now).unwrap().0, sample(&next, now).map_or((next[0].0, next[0].1), |s| s.0));
                    assert!(
                        distance(a, b) < 1.0,
                        "no jump at {}: {} m\nold {:?}\nnew {:?}",
                        now - NOW,
                        distance(a, b),
                        old.iter().take(4).map(|f| (x_of((f.0, f.1)), f.2 - NOW)).collect::<Vec<_>>(),
                        next.iter().take(4).map(|f| (x_of((f.0, f.1)), f.2 - NOW)).collect::<Vec<_>>()
                    );
                }
                frames = Some(next);
                last_sim = now;
            }
            let f = frames.as_ref().unwrap();
            let at = sample(f, now).map_or((f[0].0, f[0].1), |s| s.0);
            drawn.push((now, x_of(at)));
        }
        // never back, gently
        assert!(drawn.windows(2).all(|w| w[1].1 >= w[0].1 - 0.01), "never backwards");
        // speeds over two frames' time (6 s), and how they change from one to the next
        let v: Vec<f64> = drawn.windows(7).map(|w| (w[6].1 - w[0].1) / 6.0).collect();
        let worst = v.windows(7).map(|w| (w[6] - w[0]).abs() / 6.0).fold(0.0, f64::max);
        // (a new simulation starting part way through a frame's time shows a little more)
        assert!(worst <= DRAWN_ACCEL + CATCH_UP_ACCEL + 0.15, "speed changes at most {worst} m/s²");
        assert!(v.iter().all(|v| *v <= 120.0 / 3.6 * CATCH_UP + 0.5), "never much over the limit");
        // close to the real train
        let errors: Vec<f64> = drawn.iter().map(|(t, x)| (x - real_at(*t).0).abs()).collect();
        let (worst, mean) =
            (errors.iter().fold(0.0f64, |a, b| a.max(*b)), errors.iter().sum::<f64>() / errors.len() as f64);
        eprintln!("with speed {with_speed}: off by {worst:.0} m at worst, {mean:.0} m on average");
        assert!(worst < off.0 && mean < off.1, "off by {worst:.0} m at worst, {mean:.0} m on average");
        // it stops at the station, and leaves only once a report shows the real train moving off
        // (without speeds it pulls in a little further along the platform)
        let at_station: Vec<i64> = drawn
            .windows(2)
            .filter(|w| (w[0].1 - 6_000.0).abs() < 15.0 && (w[1].1 - w[0].1).abs() < 0.01)
            .map(|w| w[0].0)
            .collect();
        assert!(at_station.len() > 30, "stood there: {} s", at_station.len());
        let set_off = *at_station.last().unwrap();
        let first_moving = moving_report_after_stop.unwrap();
        assert!(
            set_off + 1_000 >= first_moving,
            "left at {} before a report said so at {}",
            set_off - NOW,
            first_moving - NOW
        );
        assert!(((at_station[0] - NOW) as f64 / 1000.0 - arrived).abs() < 40.0, "arrived about when it did");
    }

    #[test]
    fn frames_are_sampled_with_their_speed() {
        let a = (59.0, 18.0);
        let b = ahead(a, 90.0, 100.0);
        let f = [(a.0, a.1, NOW), (b.0, b.1, NOW + 10_000)];
        let (at, v, h) = sample(&f, NOW + 5_000).unwrap();
        assert!((distance(at, a) - 50.0).abs() < 0.5);
        assert!((v - 10.0).abs() < 0.01);
        assert!(h.is_some_and(|h| (h - 90.0).abs() < 1.0));
        assert!(sample(&f, NOW - 1).is_none());
    }

    #[test]
    fn a_late_train_still_calls_at_the_station_ahead() {
        // due at the station 20 minutes ago, it is still 2 km short
        let here = (59.0, 18.0);
        let station = ahead(here, 0.0, 2_000.0);
        let then = ahead(here, 0.0, 12_000.0);
        let stops = [
            Stop {
                lat: station.0,
                lon: station.1,
                arrive_ms: Some(NOW - 1_200_000),
                depart_ms: Some(NOW - 1_140_000),
                pass: false,
            },
            Stop { lat: then.0, lon: then.1, arrive_ms: Some(NOW - 600_000), depart_ms: None, pass: false },
        ];
        assert_eq!(plan(here, Some(20.0), Some(0.0), &stops, NOW), Plan::Moving { next: 0 });
        assert_eq!(plan(station, Some(0.0), Some(0.0), &stops, NOW), Plan::Standing { at: 0 });
        let beyond = ahead(here, 0.0, 2_500.0);
        assert_eq!(plan(beyond, Some(20.0), Some(0.0), &stops, NOW), Plan::Moving { next: 1 });
    }

    #[test]
    fn past_two_stations_both_are_behind_it() {
        // from 0, through 6 km, to 14 km: at 12.4 km it is further from 6 km than 0 is from it
        let s = |x: f64| Stop { lat: at(x).0, lon: at(x).1, arrive_ms: None, depart_ms: None, pass: false };
        let stops = [s(0.0), s(6_000.0), s(14_000.0)];
        assert_eq!(plan(at(12_400.0), None, None, &stops, NOW), Plan::Moving { next: 2 });
        assert_eq!(plan(at(3_000.0), None, None, &stops, NOW), Plan::Moving { next: 1 });
    }
}
