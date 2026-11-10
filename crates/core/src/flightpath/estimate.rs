//! Arrivals, departures and a whole estimate (`flightpath.ts:664-948`): the ground track of an
//! arrival (onto the extended centreline, down the final to the aiming point) and its heights
//! down the 3° glide path, the take-off roll and climb-out of a departure, the paths used when no
//! runways are known, trimming a path to the aircraft as it moves, and [`estimate_route`].
//!
//! `arrival_plan` and `departure_plan` return a length and a plan to build the ground track
//! from (the TypeScript's lazy `build` closure): the runway choice plans every end and samples
//! only the winner.

use serde::{Deserialize, Serialize};

use super::runways::{
    Aircraft, CentrelineOffset, ROLLOUT_M, RunwayEndInfo, RunwayReason, Traffic, aligned_with,
    centreline_offset, choose_arrival_runway, choose_departure_runway, landing_on, runway_ends,
};
use super::{
    AIM_M, CLIMB_FT_PER_NM, CLIMB_OUT_M, Connection, DEG, ENROUTE_KT, FINAL_NM, FINAL_STEP_NM, FT_M,
    GLIDE_TAN, LINE_STEP_M, LONG_FINAL_PENALTY, LonLat, MAX_FINAL_NM, MIN_FINAL_NM, MIN_FINAL_NM_CLOSE,
    NM_M, PLANNED_FT_PER_NM, PathPoint, Pose, Radii, SHORT_FINAL_PENALTY, STEEP_FT_PER_NM, TERMINAL_KT,
    TOO_HIGH_PENALTY, bearing, clamp, connect, destination, distance_km, distance_m, js_max, js_min,
    plan, sample, to_local, turn_radius,
};
use crate::geo::sphere::unwrap;
use crate::model::{Airport, LiveFlight, Runway, TrailPoint, f32_as_js};
use crate::num::wrap180;

/// Glide path height above the touchdown zone `s` metres before the aiming point, feet.
fn glide_ft(s: f64) -> f64 {
    (js_max(0.0, s) * GLIDE_TAN) / FT_M
}

/// How an arrival's ground track is built.
#[derive(Debug, Clone, PartialEq)]
enum ArrivalBuild {
    /// Already known: the landing roll, or a straight line to the threshold and the aiming point.
    Points(Vec<LonLat>),
    /// The connection to the final approach fix `fix_m` before the threshold, then the straight
    /// final to the threshold and on to the aiming point.
    Finish { to_fix: Connection, fix_m: f64, here: LonLat, end_threshold: LonLat, end_heading: f64, aim: LonLat },
}

/// An arrival's length (metres), and its ground track when wanted: sampling costs far more than
/// planning.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ArrivalPlan {
    pub(super) length: f64,
    build: ArrivalBuild,
}

impl ArrivalPlan {
    /// The ground track.
    fn build(&self) -> Vec<LonLat> {
        match &self.build {
            ArrivalBuild::Points(points) => points.clone(),
            ArrivalBuild::Finish { to_fix, fix_m, here, end_threshold, end_heading, aim } => {
                let fix = destination(*end_threshold, end_heading + 180.0, *fix_m);
                let mut points = sample(to_fix, *here, fix);
                let last = points[points.len() - 1];
                points.extend(straight(last, *end_threshold, *fix_m).into_iter().skip(1));
                points.push(*aim);
                points
            }
        }
    }
}

/// The ground track of an arrival: from the aircraft, a smooth turn onto the extended
/// centreline, then the straight final to the threshold and on to the aiming point.
fn arrival_ground(ac: &Aircraft, end: &RunwayEndInfo) -> Vec<LonLat> {
    arrival_plan(ac, end).build()
}

/// An arrival's length and how to build its ground track.
pub(super) fn arrival_plan(ac: &Aircraft, end: &RunwayEndInfo) -> ArrivalPlan {
    let here: LonLat = [ac.lon, ac.lat];
    let aim = destination(end.threshold, end.heading, AIM_M);
    let radii = Radii {
        start: turn_radius(clamp(ac.speed_kt, ENROUTE_KT[0], ENROUTE_KT[1])),
        end: turn_radius(clamp(ac.speed_kt, TERMINAL_KT[0], TERMINAL_KT[1])),
    };
    let agl = js_max(0.0, ac.alt_ft - end.elevation_ft);
    let fix = |m: f64| destination(end.threshold, end.heading + 180.0, m);
    let finish = |to_fix: Connection, fix_m: f64| ArrivalPlan {
        length: to_fix.length + fix_m + AIM_M,
        build: ArrivalBuild::Finish {
            to_fix,
            fix_m,
            here,
            end_threshold: end.threshold,
            end_heading: end.heading,
            aim,
        },
    };

    if landing_on(end, ac) {
        // over the runway already: what is left is the landing roll, straight on
        // (an approach planned from here would go round and land again)
        let CentrelineOffset { along, .. } = centreline_offset(end, here);
        let remaining = end.length_m - distance_m(end.start, end.threshold) - 100.0;
        let stop_m = js_min(remaining, AIM_M + ROLLOUT_M);
        let to_go = stop_m + along;
        if to_go < 1.0 {
            return ArrivalPlan { length: 0.0, build: ArrivalBuild::Points(vec![here]) };
        }
        let stop = destination(end.threshold, end.heading, stop_m);
        return ArrivalPlan { length: to_go, build: ArrivalBuild::Points(vec![here, stop]) };
    }

    if aligned_with(end, ac) {
        // already on (or nearly on) the final: merge onto the centreline a little
        // ahead, far enough on for a gentle S-turn of this radius
        let CentrelineOffset { along, cross } = centreline_offset(end, here);
        let off_heading = wrap180(ac.track - end.heading).abs() * DEG;
        let needed = 1.2 * (2.0 * (radii.end * cross.abs()).sqrt() + radii.end * off_heading);
        let lead = js_min(along * 0.9, js_max(1_500.0, needed));
        if along - lead < 200.0 {
            return ArrivalPlan {
                length: along + AIM_M,
                build: ArrivalBuild::Points(vec![here, end.threshold, aim]),
            };
        }
        let to_fix = plan(Pose { at: here, heading: ac.track }, fix(along - lead), Some(end.heading), radii);
        return finish(to_fix, along - lead);
    }

    // Join the final where the glide path meets the usual intercept height
    // (about 3,000 ft, 10 NM out), but closer in when that saves a long way
    // round (an aircraft already on base), and further out when the aircraft
    // is too high to get down from where it is.
    let usual = clamp(agl / (GLIDE_TAN * (NM_M / FT_M)), MIN_FINAL_NM, FINAL_NM);
    let mut best: Option<(Connection, f64)> = None;
    let mut best_cost = f64::INFINITY;
    let mut nm = MIN_FINAL_NM_CLOSE;
    while nm <= MAX_FINAL_NM {
        let fix_m = nm * NM_M;
        let to_fix = plan(Pose { at: here, heading: ac.track }, fix(fix_m), Some(end.heading), radii);
        let length = to_fix.length;
        // height to lose before the fix beyond what the path allows for, as the distance it would take
        let excess = agl - glide_ft(fix_m + AIM_M);
        let short = js_max(0.0, (excess / PLANNED_FT_PER_NM) * NM_M - length);
        let penalty = if nm < usual {
            SHORT_FINAL_PENALTY * (usual - nm) * NM_M
        } else {
            LONG_FINAL_PENALTY * (nm - usual) * NM_M
        };
        let cost = length + fix_m + penalty + TOO_HIGH_PENALTY * short;
        if cost < best_cost {
            best_cost = cost;
            best = Some((to_fix, fix_m));
        }
        nm += FINAL_STEP_NM;
    }
    match best {
        Some((to_fix, fix_m)) => finish(to_fix, fix_m),
        // every cost NaN (a NaN position or altitude), where the TypeScript threw on `best!`
        None => ArrivalPlan { length: f64::NAN, build: ArrivalBuild::Points(vec![here]) },
    }
}

/// A straight leg from `a` to `b` sampled every few kilometres, both ends included: `metres`
/// (the TypeScript's default is the distance from `a` to `b`) sets the number of points.
fn straight(a: LonLat, b: LonLat, metres: f64) -> Vec<LonLat> {
    let n = js_max(1.0, (metres / LINE_STEP_M).ceil());
    if n.is_nan() {
        // `Array.from({ length: NaN })` is empty
        return Vec::new();
    }
    let n = n as usize;
    let course = bearing(a, b);
    let d = distance_m(a, b);
    (0..=n)
        .map(|i| {
            if i == n {
                b
            } else if i == 0 {
                a
            } else {
                destination(a, course, (d * i as f64) / n as f64)
            }
        })
        .collect()
}

/// [`straight`] with its default `metres`: the distance from `a` to `b`.
fn straight_between(a: LonLat, b: LonLat) -> Vec<LonLat> {
    straight(a, b, distance_m(a, b))
}

/// Heights for an arrival's ground track: the current altitude until the descent, then down the
/// glide path to the aiming point (an aircraft below it flies level until it meets it, as when
/// intercepting a glideslope from below); an aircraft above it descends more steeply until it is
/// on it.
fn descend(ground: &[LonLat], alt_ft: f64, elevation_ft: f64) -> Vec<PathPoint> {
    let mut cumulative = vec![0.0];
    for i in 1..ground.len() {
        cumulative.push(cumulative[i - 1] + distance_m(ground[i - 1], ground[i]));
    }
    let total = cumulative[cumulative.len() - 1];
    let start = js_max(0.0, alt_ft - elevation_ft);
    let points: Vec<PathPoint> = ground
        .iter()
        .enumerate()
        .map(|(i, &[lon, lat])| {
            let to_go = total - cumulative[i];
            let steep = start - (STEEP_FT_PER_NM * cumulative[i]) / NM_M;
            [lon, lat, elevation_ft + js_min(start, js_max(glide_ft(to_go), steep))]
        })
        .collect();
    unwrap(&points)
}

/// The estimated path from the aircraft to touchdown on `end`, with heights (ft MSL).
pub fn arrival_path(ac: &Aircraft, end: &RunwayEndInfo) -> Vec<PathPoint> {
    descend(&arrival_ground(ac, end), ac.alt_ft, end.elevation_ft)
}

/// Without runways: turn toward the airport and fly straight there, descending at 3° to
/// `elevation_ft` (0 in the TypeScript when left out).
pub fn arrival_to_point(ac: &Aircraft, airport: LonLat, elevation_ft: f64) -> Vec<PathPoint> {
    let ground = connect(
        Pose { at: [ac.lon, ac.lat], heading: ac.track },
        airport,
        None,
        Radii { start: turn_radius(clamp(ac.speed_kt, ENROUTE_KT[0], ENROUTE_KT[1])), end: 0.0 },
    );
    descend(&ground, ac.alt_ft, elevation_ft)
}

/// A departure's length (metres) and what its ground track is built from.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct DeparturePlan {
    pub(super) length: f64,
    start: LonLat,
    liftoff: LonLat,
    climb_out: LonLat,
    target: LonLat,
    rest: Connection,
}

impl DeparturePlan {
    /// The ground track, and the index of lift-off in it.
    fn build(&self) -> (Vec<LonLat>, usize) {
        let mut ground = vec![self.start];
        let mut climb = straight_between(self.liftoff, self.climb_out);
        climb.pop();
        ground.extend(climb);
        ground.extend(sample(&self.rest, self.climb_out, self.target));
        (ground, 1)
    }
}

/// A departure from `end` to the first tracked position `to`.
pub(super) fn departure_plan(end: &RunwayEndInfo, to: &Aircraft) -> DeparturePlan {
    // lift-off well before the far end, then straight ahead before turning on course
    let roll_m = js_min(0.6 * end.length_m, 2_200.0);
    let liftoff = destination(end.start, end.heading, roll_m);
    let climb_out = destination(liftoff, end.heading, CLIMB_OUT_M);
    let target: LonLat = [to.lon, to.lat];
    let rest = plan(
        Pose { at: climb_out, heading: end.heading },
        target,
        Some(to.track),
        Radii {
            start: turn_radius(clamp(to.speed_kt * 0.6, TERMINAL_KT[0], TERMINAL_KT[1])),
            end: turn_radius(clamp(to.speed_kt, ENROUTE_KT[0], ENROUTE_KT[1])),
        },
    );
    DeparturePlan { length: roll_m + CLIMB_OUT_M + rest.length, start: end.start, liftoff, climb_out, target, rest }
}

/// Heights for a climb from the ground at `elevation_ft` at `ground[from]` up to `alt_ft` at its
/// end: a steady climb, levelling off at the end altitude if it is reached early.
fn climb(ground: &[LonLat], from: usize, elevation_ft: f64, alt_ft: f64) -> Vec<PathPoint> {
    let mut cumulative = vec![0.0; ground.len()];
    for i in from + 1..ground.len() {
        cumulative[i] = cumulative[i - 1] + distance_m(ground[i - 1], ground[i]);
    }
    // `|| 1`: no points, a zero or a NaN length all climb over one metre
    let total = cumulative.last().copied().filter(|t| *t != 0.0 && !t.is_nan()).unwrap_or(1.0);
    let top = js_max(0.0, alt_ft - elevation_ft);
    let gradient = js_max(CLIMB_FT_PER_NM / NM_M, top / total);
    let points: Vec<PathPoint> = ground
        .iter()
        .zip(&cumulative)
        .map(|(&[lon, lat], &c)| [lon, lat, elevation_ft + js_min(top, gradient * c)])
        .collect();
    unwrap(&points)
}

/// The estimated path from the take-off roll on `end` to the first tracked position `to`, with
/// heights (ft MSL).
pub fn departure_path(end: &RunwayEndInfo, to: &Aircraft) -> Vec<PathPoint> {
    let (ground, liftoff) = departure_plan(end, to).build();
    let mut path = climb(&ground, liftoff, end.elevation_ft, to.alt_ft);
    if let Some(last) = path.last_mut() {
        last[2] = to.alt_ft;
    }
    path
}

/// Without runways: from the airport, the great circle (turning onto the track at its end),
/// climbing steadily.
pub fn departure_from_point(airport: LonLat, elevation_ft: f64, to: &Aircraft) -> Vec<PathPoint> {
    // flown backwards from the first tracked position, it is an arrival at the airport
    let mut back = connect(
        Pose { at: [to.lon, to.lat], heading: (to.track + 180.0) % 360.0 },
        airport,
        None,
        Radii { start: turn_radius(clamp(to.speed_kt, ENROUTE_KT[0], ENROUTE_KT[1])), end: 0.0 },
    );
    back.reverse();
    let mut path = climb(&back, 0, elevation_ft, to.alt_ft);
    if let Some(last) = path.last_mut() {
        last[2] = to.alt_ft;
    }
    path
}

/// [`trim_to_position`]'s default `search_m`.
pub const DEFAULT_SEARCH_M: f64 = 20_000.0;
/// [`trim_to_position`]'s default `near_m`.
pub const DEFAULT_NEAR_M: f64 = 3_000.0;

/// The rest of `path` from the point nearest `here`, starting exactly at `here`: the aircraft
/// moves between route updates, and the drawn path should start at the aircraft without doubling
/// back. Only the first `search_m` of the path is searched; if nothing there is within `near_m`,
/// `here` just replaces the first point. The TypeScript's defaults are [`DEFAULT_SEARCH_M`] and
/// [`DEFAULT_NEAR_M`].
pub fn trim_to_position(path: &[PathPoint], here: LonLat, search_m: f64, near_m: f64) -> Vec<PathPoint> {
    if path.len() < 2 {
        return path.to_vec();
    }
    // (i, f, d)
    let mut best = (0, 0.0, f64::INFINITY);
    let mut walked = 0.0;
    let mut i = 0;
    while i < path.len() - 1 && walked <= search_m {
        let a: LonLat = [path[i][0], path[i][1]];
        let b: LonLat = [path[i + 1][0], path[i + 1][1]];
        // project in a local frame at the segment start
        let [bx, by] = to_local(a, b);
        let [px, py] = to_local(a, here);
        let len2 = bx * bx + by * by;
        let f = if len2 > 0.0 { clamp((px * bx + py * by) / len2, 0.0, 1.0) } else { 0.0 };
        let d = (px - f * bx).hypot(py - f * by);
        if d < best.2 {
            best = (i, f, d);
        }
        walked += len2.sqrt();
        i += 1;
    }
    let (best_i, best_f, best_d) = best;
    if best_d > near_m {
        let mut out = vec![[here[0], here[1], path[0][2]]];
        out.extend_from_slice(&path[1..]);
        return out;
    }
    let a = path[best_i];
    let b = path[best_i + 1];
    let alt = a[2] + (b[2] - a[2]) * best_f;
    let mut out = vec![[here[0], here[1], alt]];
    out.extend_from_slice(&path[best_i + 1..]);
    unwrap(&out)
}

// --- A whole estimate --------------------------------------------------------------------------

/// What a route is: the subscriber flight plan (`routes` only), the app's own estimate, or
/// nothing (no airport known). Written `"filed"`, `"estimated"`, `"none"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RouteKind {
    Filed,
    Estimated,
    None,
}

/// A runway an estimate departs from or lands on, and why it was picked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunwayGuess {
    /// e.g. `22L`
    pub runway: String,
    pub reason: RunwayReason,
}

/// How far around an airport other traffic says which runway is in use.
const TRAFFIC_KM: f64 = 30.0;

/// What [`estimate_route`] reads.
#[derive(Debug, Clone, Copy)]
pub struct EstimateInput<'a> {
    /// The flight; only `id`, `lat`, `lon`, `track`, `alt`, `speed` and `on_ground` are read.
    pub flight: &'a LiveFlight,
    /// The first point of the recorded track.
    pub trail_start: Option<&'a TrailPoint>,
    pub origin: Option<&'a Airport>,
    pub destination: Option<&'a Airport>,
    pub origin_runways: &'a [Runway],
    pub destination_runways: &'a [Runway],
    pub traffic: &'a [Traffic],
}

/// The estimated parts of a route.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Estimate {
    /// [`RouteKind::Estimated`] with an origin or a destination, else [`RouteKind::None`].
    pub kind: RouteKind,
    /// From the take-off roll to where the recorded track begins, with heights (ft MSL).
    pub before: Vec<PathPoint>,
    /// From the aircraft to touchdown, with heights (ft MSL).
    pub ahead: Vec<PathPoint>,
    /// The runway `before` departs from, when runways are known.
    pub departure: Option<RunwayGuess>,
    /// The runway `ahead` lands on, when runways are known.
    pub arrival: Option<RunwayGuess>,
}

/// The estimated parts of a route; pure, given the airports and their runways. `ahead` is filled
/// only for an airborne flight with a destination, `before` only when the origin is more than
/// 15 km from the first recorded trail point (the recorded track usually begins at the gate).
pub fn estimate_route(input: &EstimateInput) -> Estimate {
    let flight = input.flight;
    let kind = if input.origin.is_some() || input.destination.is_some() {
        RouteKind::Estimated
    } else {
        RouteKind::None
    };
    let mut ahead = Vec::new();
    let mut arrival = None;
    if let Some(destination) = input.destination
        && !flight.on_ground
    {
        let elevation = destination.alt.map_or(0.0, |a| a as f64);
        let ends = runway_ends(input.destination_runways, elevation);
        let ac = Aircraft {
            lon: f32_as_js(flight.lon),
            lat: f32_as_js(flight.lat),
            track: flight.track as f64,
            alt_ft: flight.alt as f64,
            speed_kt: flight.speed as f64,
        };
        let traffic = near(input.traffic, destination);
        match choose_arrival_runway(&ends, &ac, &traffic, Some(flight.id)) {
            Some(choice) => {
                ahead = arrival_path(&ac, choice.end);
                arrival = Some(RunwayGuess { runway: choice.end.ident.clone(), reason: choice.reason });
            }
            None => ahead = arrival_to_point(&ac, [destination.lon, destination.lat], elevation),
        }
    }

    let mut before = Vec::new();
    let mut departure = None;
    // the recorded track usually begins at the gate; only fill a real gap
    if let (Some(origin), Some(start)) = (input.origin, input.trail_start) {
        let start_at: LonLat = [f32_as_js(start.longitude), f32_as_js(start.latitude)];
        if distance_km([origin.lon, origin.lat], start_at) > 15.0 {
            let elevation = origin.alt.map_or(0.0, |a| a as f64);
            let ends = runway_ends(input.origin_runways, elevation);
            let to = Aircraft {
                lon: start_at[0],
                lat: start_at[1],
                track: start.track as f64,
                alt_ft: start.altitude as f64,
                speed_kt: if start.ground_speed != 0 { start.ground_speed as f64 } else { 250.0 },
            };
            let traffic = near(input.traffic, origin);
            match choose_departure_runway(&ends, &to, &traffic) {
                Some(choice) => {
                    before = departure_path(choice.end, &to);
                    departure = Some(RunwayGuess { runway: choice.end.ident.clone(), reason: choice.reason });
                }
                None => before = departure_from_point([origin.lon, origin.lat], elevation, &to),
            }
        }
    }
    Estimate { kind, before, ahead, departure, arrival }
}

/// Flights within [`TRAFFIC_KM`] of an airport.
pub(super) fn near(traffic: &[Traffic], airport: &Airport) -> Vec<Traffic> {
    let d_lat = TRAFFIC_KM / 111.0;
    let d_lon = d_lat / js_max(0.05, ((airport.lat * std::f64::consts::PI) / 180.0).cos());
    traffic
        .iter()
        .filter(|f| {
            (f.lat - airport.lat).abs() < d_lat
                && (f.lon - airport.lon).abs() < d_lon
                && distance_km([f.lon, f.lat], [airport.lon, airport.lat]) < TRAFFIC_KM
        })
        .copied()
        .collect()
}
