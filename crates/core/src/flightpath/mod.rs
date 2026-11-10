//! Port of `flightpath.ts` (S1.16; doc 05 §8.1, doc 03 §15): the shape of an estimated route.
//! Great circles en route, and near the airports what aircraft actually fly there: arrivals turn
//! onto the extended runway centreline and fly a straight final down a 3° glide path to the
//! touchdown zone; departures roll and climb out straight ahead before turning on course. Turns
//! are circular arcs of a radius the speed allows (Dubins paths), so the drawn path never kinks.
//!
//! Everything here is an estimate made without the runway in use, the procedures or ATC's
//! vectors; `routes` labels it as such.
//!
//! - this module: the constants, the local flat frame, turn radii, and joining two poses on the
//!   globe (`plan`, `sample`, `connect`);
//! - [`dubins`]: Dubins paths and one-turn-then-straight paths in a local frame;
//! - [`runways`]: runway ends, centreline offsets, the runway use other traffic shows, and the
//!   runway choice;
//! - [`estimate`]: arrival and departure paths with their heights, trimming a path to the
//!   aircraft, and [`estimate_route`].
//!
//! The great-circle helpers (`flightpath.ts:76-133`) are [`crate::geo::sphere`]'s, re-exported
//! here.
//!
//! Transliterated as written: degrees become radians with `* DEG` and come back with `/ DEG`,
//! `Math.min`/`Math.max` keep NaN as in JavaScript (the private `js_min`, `js_max`), the `%` wraps are
//! JavaScript's (the sign of the dividend, as Rust's), and `Array.reduce` without a seed is a
//! loop in which ties keep the earlier element. TypeScript defaults become constants
//! ([`DEFAULT_SEARCH_M`], [`DEFAULT_NEAR_M`], [`DEFAULT_STEP_KM`]).

pub mod dubins;
pub mod estimate;
pub mod runways;
#[cfg(test)]
mod tests;

use std::f64::consts::PI;

pub use crate::geo::sphere::{
    DEFAULT_STEP_KM, LonLat, PathPoint, bearing, destination, distance_km, distance_m, great_circle,
};
use crate::geo::sphere::intermediate;
use crate::num::wrap180;
use dubins::{Plan, dubins_plan, sample_plan, turn_then_straight_plan};
pub use estimate::{
    DEFAULT_NEAR_M, DEFAULT_SEARCH_M, Estimate, EstimateInput, RouteKind, RunwayGuess, arrival_path,
    arrival_to_point, departure_from_point, departure_path, estimate_route, trim_to_position,
};
pub use runways::{
    Aircraft, CentrelineOffset, RunwayChoice, RunwayEndInfo, RunwayReason, RunwayUse, Traffic,
    centreline_offset, choose_arrival_runway, choose_departure_runway, runway_ends, runway_use,
};

const DEG: f64 = PI / 180.0;
const TAU: f64 = 2.0 * PI;
const NM_M: f64 = 1852.0;
const FT_M: f64 = 0.3048;
const KT_MS: f64 = 0.514444;
const GRAVITY: f64 = 9.80665;

/// The standard glide path, degrees.
pub const GLIDE_DEG: f64 = 3.0;
/// `Math.tan(GLIDE_DEG * DEG)` as JavaScript computes it (a test checks it against `f64::tan`).
const GLIDE_TAN: f64 = 0.052_407_779_283_041_21;
/// The height the glide path crosses the threshold at; only [`AIM_M`] is made from it (a test
/// checks the literal against it).
#[cfg(test)]
const THRESHOLD_CROSSING_FT: f64 = 50.0;
/// The glide path's aiming point lies this far past the threshold (about 290 m):
/// `(THRESHOLD_CROSSING_FT * FT_M) / GLIDE_TAN`, as JavaScript computes it.
pub const AIM_M: f64 = 290.796_523_120_977_9;
/// Where arrivals usually join the final approach, NM before the threshold.
pub const FINAL_NM: f64 = 10.0;
/// The limits on the usual final.
const MIN_FINAL_NM: f64 = 5.0;
const MAX_FINAL_NM: f64 = 25.0;
/// The shortest final considered at all, for an aircraft already turning in close.
const MIN_FINAL_NM_CLOSE: f64 = 2.0;
/// Candidate joining points are this far apart along the centreline.
const FINAL_STEP_NM: f64 = 0.25;
/// Cost per NM of joining the final short of, or beyond, the usual point (in NM of extra flying).
const SHORT_FINAL_PENALTY: f64 = 3.0;
const LONG_FINAL_PENALTY: f64 = 0.2;
/// Cost per NM of descent the path is short of (it would have to be steeper than planned).
const TOO_HIGH_PENALTY: f64 = 4.0;
/// Airliners turn at up to 25-30° of bank; 25° gives the usual radii.
const BANK_DEG: f64 = 25.0;
/// Terminal-area speeds for the turn radius, kt (arrivals are slowed to ~180-230 kt).
const TERMINAL_KT: [f64; 2] = [140.0, 230.0];
const ENROUTE_KT: [f64; 2] = [140.0, 480.0];
/// Steepest descent drawn before the final (idle and speedbrakes), ft per NM.
const STEEP_FT_PER_NM: f64 = 500.0;
/// Steepest descent a path is planned for: about 3.3°, the 3° rule of thumb with a little in hand.
const PLANNED_FT_PER_NM: f64 = 350.0;
/// Average climb gradient after take-off, ft per NM.
const CLIMB_FT_PER_NM: f64 = 600.0;
/// Closer than this, the whole path is shaped in one local frame.
const LOCAL_M: f64 = 150_000.0;
/// Further out, the great circle hands over to the terminal shape this far from the fix.
const ENTRY_M: f64 = 60_000.0;
/// ...and the aircraft's current heading joins the great circle within this distance.
const JOIN_M: f64 = 60_000.0;
/// Straight climb after lift-off before a departure turns on course.
const CLIMB_OUT_M: f64 = 6_000.0;
/// Sampling: arcs every few degrees, straight legs every few kilometres.
const ARC_STEP: f64 = 3.0 * DEG;
const LINE_STEP_M: f64 = 2_000.0;
/// Great circles are sampled about this often.
const GREAT_CIRCLE_STEP_KM: f64 = 25.0;

/// `Math.min(a, b)`: NaN when either is NaN, and -0 below +0 (`f64::min` drops a NaN instead).
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b || (a == b && a.is_sign_negative()) {
        a
    } else {
        b
    }
}

/// `Math.max(a, b)`: NaN when either is NaN, and +0 above -0 (`f64::max` drops a NaN instead).
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b || (a == b && a.is_sign_positive()) {
        a
    } else {
        b
    }
}

/// `Math.min(hi, Math.max(lo, v))`.
fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    js_min(hi, js_max(lo, v))
}

/// An angle brought into [0, 2π).
fn mod2pi(a: f64) -> f64 {
    a - TAU * (a / TAU).floor()
}

/// Compass heading (degrees) to mathematical angle (radians, counter-clockwise from east).
fn to_math(heading: f64) -> f64 {
    (90.0 - heading) * DEG
}

/// A local flat frame: azimuthal equidistant around `centre`, x east and y north in metres.
/// Distances and courses from the centre are exact, and within the ~200 km it is used for,
/// headings elsewhere are off by far less than a degree.
fn to_local(centre: LonLat, p: LonLat) -> [f64; 2] {
    let d = distance_m(centre, p);
    let c = bearing(centre, p) * DEG;
    [d * c.sin(), d * c.cos()]
}

/// The point at (`x`, `y`) of [`to_local`]'s frame around `centre`.
fn from_local(centre: LonLat, x: f64, y: f64) -> LonLat {
    destination(centre, x.atan2(y) / DEG, x.hypot(y))
}

/// Radius of a turn at `speed_kt` and the usual bank (25°), metres.
pub fn turn_radius(speed_kt: f64) -> f64 {
    let v = speed_kt * KT_MS;
    (v * v) / (GRAVITY * (BANK_DEG * DEG).tan())
}

// --- Joining two poses on the globe -----------------------------------------------------------

/// Where an aircraft is and which way it is going.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub at: LonLat,
    /// Degrees clockwise from true north.
    pub heading: f64,
}

/// Radii for the two ends of a connection, metres.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Radii {
    start: f64,
    end: f64,
}

/// A local plan placed on the globe: the frame's centre and the plan.
#[derive(Debug, Clone, PartialEq)]
struct Placed {
    centre: LonLat,
    plan: Plan,
}

/// One leg of a [`Connection`]: a local turn, or a great circle.
#[derive(Debug, Clone, PartialEq)]
enum Leg {
    Placed(Placed),
    Great { from: LonLat, to: LonLat },
}

/// A connection: local turns and great circle legs, measured but not yet sampled.
#[derive(Debug, Clone, PartialEq)]
struct Connection {
    legs: Vec<Leg>,
    length: f64,
}

/// One local plan around `centre`, from `from` heading `h0`: a Dubins path onto heading `h1`, or
/// with no end heading one turn then straight to the centre.
fn local(centre: LonLat, from: LonLat, h0: f64, h1: Option<f64>, r: f64) -> Placed {
    let [x0, y0] = to_local(centre, from);
    let plan = match h1 {
        None => turn_then_straight_plan(x0, y0, h0, 0.0, 0.0, r),
        Some(h1) => dubins_plan(x0, y0, h0, 0.0, 0.0, h1, r),
    };
    Placed { centre, plan }
}

/// A smooth path from `a` to `b`: a great circle en route, with only the ends shaped (joining
/// from `a`'s heading, and turning onto `b`'s), or, when `b` has no heading, arriving however the
/// great circle does.
fn plan(a: Pose, b: LonLat, end_heading: Option<f64>, radii: Radii) -> Connection {
    let total = distance_m(a.at, b);
    if total < 1.0 {
        return Connection { legs: vec![Leg::Great { from: a.at, to: b }], length: total };
    }
    if total <= LOCAL_M {
        let r = if end_heading.is_none() { radii.start } else { radii.end };
        let placed = local(b, a.at, a.heading, end_heading, r);
        let length = placed.plan.length;
        return Connection { legs: vec![Leg::Placed(placed)], length };
    }
    // en route: the great circle, entered from the aircraft's heading
    let entry = match end_heading {
        None => b,
        Some(_) => intermediate(a.at, b, (total - ENTRY_M) / total),
    };
    let course = bearing(a.at, entry);
    let mut legs = Vec::new();
    let mut length = 0.0;
    let mut from = a.at;
    if wrap180(a.heading - course).abs() > 1.0 {
        let join_m = js_min(JOIN_M, (distance_m(a.at, entry) - 1.0) / 2.0);
        let join = intermediate(a.at, entry, join_m / distance_m(a.at, entry));
        let placed = local(join, a.at, a.heading, Some(bearing(join, entry)), radii.start);
        length += placed.plan.length;
        legs.push(Leg::Placed(placed));
        from = join;
    }
    legs.push(Leg::Great { from, to: entry });
    length += distance_m(from, entry);
    if let Some(end_heading) = end_heading {
        // the great circle's own course where it hands over, turning onto the end pose
        let placed = local(b, entry, bearing(entry, b), Some(end_heading), radii.end);
        length += placed.plan.length;
        legs.push(Leg::Placed(placed));
    }
    Connection { legs, length }
}

/// The points of a connection, from `a` to `b` exactly.
fn sample(connection: &Connection, a: LonLat, b: LonLat) -> Vec<LonLat> {
    let mut out: Vec<LonLat> = Vec::new();
    for leg in &connection.legs {
        let points: Vec<LonLat> = match leg {
            Leg::Placed(placed) => {
                sample_plan(&placed.plan).into_iter().map(|[x, y]| from_local(placed.centre, x, y)).collect()
            }
            Leg::Great { from, to } => great_circle(*from, *to, GREAT_CIRCLE_STEP_KM),
        };
        let skip = usize::from(!out.is_empty());
        out.extend(points.into_iter().skip(skip));
    }
    // `out[0] = a` on an empty array makes it `[a]`, which the next line then overwrites
    if out.is_empty() {
        out.push(a);
    }
    out[0] = a;
    let last = out.len() - 1;
    out[last] = b;
    out
}

/// [`plan`], sampled.
fn connect(a: Pose, b: LonLat, end_heading: Option<f64>, radii: Radii) -> Vec<LonLat> {
    sample(&plan(a, b, end_heading, radii), a.at, b)
}
