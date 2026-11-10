//! Runways (`flightpath.ts:429-662`): the ends airliners would use, where a point lies against an
//! end's extended centreline, which ends other traffic is seen using, and the choice of the
//! landing and take-off runway.
//!
//! The TypeScript keyed runway use by the end object (`Map<RunwayEndInfo, …>`); here
//! [`runway_use`] returns one [`RunwayUse`] per end, in the order of the ends, and a
//! [`RunwayChoice`] borrows the chosen end from the slice it was chosen from.

use serde::{Deserialize, Serialize};

use super::estimate::{arrival_plan, departure_plan};
use super::{DEG, FT_M, LonLat, NM_M, bearing, destination, distance_m, js_max, js_min};
use crate::model::{LiveFlight, Runway, f32_as_js};
use crate::num::wrap180;

/// One landing (or take-off) direction of a runway.
#[derive(Debug, Clone, PartialEq)]
pub struct RunwayEndInfo {
    pub ident: String,
    /// Degrees true: the direction of landing and take-off on this end.
    pub heading: f64,
    /// Where the runway physically begins (take-off rolls start here).
    pub start: LonLat,
    /// The landing threshold: the start, moved in by any displacement.
    pub threshold: LonLat,
    /// Feet above mean sea level at the threshold.
    pub elevation_ft: f64,
    /// Metres from the start to the far end.
    pub length_m: f64,
}

/// `/ASP|CON|PEM|BIT|TAR|PAV|SEAL|MAC|CEM/i`: without the `u` flag only ASCII letters match
/// case-insensitively, so an ASCII upper-casing and a substring search are the same test.
fn hard_surface(surface: &str) -> bool {
    const WORDS: [&str; 9] = ["ASP", "CON", "PEM", "BIT", "TAR", "PAV", "SEAL", "MAC", "CEM"];
    let upper = surface.to_ascii_uppercase();
    WORDS.iter().any(|w| upper.contains(w))
}

/// The ends airliners would use: on hard surfaces where the airport has any, and not much
/// shorter than its longest runway (at least `min(0.7 · longest, 1800 m)`). An end without an
/// elevation takes `field_elevation_ft` (0 in the TypeScript when left out).
pub fn runway_ends(runways: &[Runway], field_elevation_ft: f64) -> Vec<RunwayEndInfo> {
    let hard: Vec<&Runway> = runways.iter().filter(|r| hard_surface(&r.surface)).collect();
    let usable: Vec<&Runway> = if hard.is_empty() { runways.iter().collect() } else { hard };
    let ends: Vec<RunwayEndInfo> = usable
        .iter()
        .flat_map(|r| {
            [0, 1].map(|i| {
                let near = &r.ends[i];
                let far = &r.ends[1 - i];
                let start: LonLat = [near.lon, near.lat];
                let heading = bearing(start, [far.lon, far.lat]);
                RunwayEndInfo {
                    ident: near.ident.clone(),
                    heading,
                    start,
                    threshold: if near.displaced_ft > 0.0 {
                        destination(start, heading, near.displaced_ft * FT_M)
                    } else {
                        start
                    },
                    elevation_ft: near.elevation_ft.unwrap_or(field_elevation_ft),
                    length_m: distance_m(start, [far.lon, far.lat]),
                }
            })
        })
        .collect();
    let longest = ends.iter().fold(0.0, |m, e| js_max(m, e.length_m));
    let shortest = js_min(0.7 * longest, 1_800.0);
    ends.into_iter().filter(|e| e.length_m >= shortest).collect()
}

/// Where a point lies against an end's extended centreline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CentrelineOffset {
    /// Metres before the threshold (negative past it).
    pub along: f64,
    /// Metres to the right of an aircraft flying the approach.
    pub cross: f64,
}

/// Position relative to an end's extended centreline.
pub fn centreline_offset(end: &RunwayEndInfo, p: LonLat) -> CentrelineOffset {
    let d = distance_m(end.threshold, p);
    let angle = (bearing(end.threshold, p) - (end.heading + 180.0)) * DEG;
    // to the right of an aircraft flying the approach is to the left seen from the threshold
    CentrelineOffset { along: d * angle.cos(), cross: -d * angle.sin() }
}

/// Another flight, as runway use reads it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Traffic {
    pub id: u32,
    pub lat: f64,
    pub lon: f64,
    /// Degrees true.
    pub track: f64,
    /// Feet.
    pub alt: f64,
    /// Knots.
    pub speed: f64,
    pub on_ground: bool,
}

impl From<&LiveFlight> for Traffic {
    /// The TypeScript passed live flights as traffic as they were; their `f32` positions are read
    /// as the JSON's shortest decimals ([`f32_as_js`]).
    fn from(f: &LiveFlight) -> Self {
        Traffic {
            id: f.id,
            lat: f32_as_js(f.lat),
            lon: f32_as_js(f.lon),
            track: f.track as f64,
            alt: f.alt as f64,
            speed: f.speed as f64,
            on_ground: f.on_ground,
        }
    }
}

/// Arrivals and departures seen flying along one end's direction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunwayUse {
    pub arrivals: u32,
    pub departures: u32,
}

/// Arrivals and departures seen flying along each end's direction, one entry per end in the
/// order of `ends`. Each flight (but `ignore_id`) within 15° of an end's heading counts for the
/// end it is best aligned with (smallest |cross|): on the ground at 40 kt or more on the runway
/// as a departure, airborne within 3000 ft of the threshold on final as an arrival or climbing
/// out straight ahead as a departure.
pub fn runway_use(ends: &[RunwayEndInfo], traffic: &[Traffic], ignore_id: Option<u32>) -> Vec<RunwayUse> {
    let mut usage = vec![RunwayUse::default(); ends.len()];
    for f in traffic {
        if Some(f.id) == ignore_id {
            continue;
        }
        // (end index, arrival, cross)
        let mut best: Option<(usize, bool, f64)> = None;
        for (i, end) in ends.iter().enumerate() {
            if wrap180(f.track - end.heading).abs() > 15.0 {
                continue;
            }
            let CentrelineOffset { along, cross } = centreline_offset(end, [f.lon, f.lat]);
            let agl = f.alt - end.elevation_ft;
            let arrival;
            if f.on_ground {
                // on the runway at speed: a landing or a take-off roll
                if f.speed < 40.0 || along > 100.0 || along < -end.length_m || cross.abs() > 60.0 {
                    continue;
                }
                arrival = false;
            } else if agl > 3_000.0 {
                continue;
            } else if along >= 0.0 {
                // on final: inside a few degrees of the centreline, within 20 km
                if along > 20_000.0 || cross.abs() > 150.0 + along * 0.06 {
                    continue;
                }
                arrival = true;
            } else {
                // climbing out straight ahead
                let past = -along - end.length_m;
                if past > 12_000.0 || cross.abs() > 300.0 + js_max(0.0, past) * 0.08 {
                    continue;
                }
                arrival = false;
            }
            if best.is_none_or(|(_, _, best_cross)| cross.abs() < best_cross.abs()) {
                best = Some((i, arrival, cross));
            }
        }
        if let Some((i, arrival, _)) = best {
            if arrival {
                usage[i].arrivals += 1;
            } else {
                usage[i].departures += 1;
            }
        }
    }
    usage
}

/// Why a runway was picked: the aircraft is lined up with it, other traffic is using it, or
/// neither (a guess).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunwayReason {
    Aligned,
    Traffic,
    Guessed,
}

/// A chosen runway end, borrowed from the ends it was chosen from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunwayChoice<'a> {
    pub end: &'a RunwayEndInfo,
    pub reason: RunwayReason,
}

/// The direction most traffic is using, if any is seen: the indexes of its ends (parallel
/// runways, within 15°, share one), scored as two per arrival and one per departure.
fn flow_in_use(ends: &[RunwayEndInfo], usage: &[RunwayUse]) -> Vec<usize> {
    let mut best: Vec<usize> = Vec::new();
    let mut best_count = 0;
    for end in ends {
        let group: Vec<usize> =
            (0..ends.len()).filter(|&i| wrap180(ends[i].heading - end.heading).abs() <= 15.0).collect();
        let count: u32 = group.iter().map(|&i| usage[i].arrivals * 2 + usage[i].departures).sum();
        if count > best_count {
            best = group;
            best_count = count;
        }
    }
    best
}

/// An aircraft, as the path estimates read it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Aircraft {
    pub lon: f64,
    pub lat: f64,
    /// Degrees true.
    pub track: f64,
    /// Feet above mean sea level.
    pub alt_ft: f64,
    pub speed_kt: f64,
}

/// Lined up with this end's final: close to the centreline, heading the same way, low enough.
pub(super) fn aligned_with(end: &RunwayEndInfo, ac: &Aircraft) -> bool {
    let CentrelineOffset { along, cross } = centreline_offset(end, [ac.lon, ac.lat]);
    along > 300.0
        && along < 30.0 * NM_M
        && cross.abs() < 300.0 + along * 0.08
        && wrap180(ac.track - end.heading).abs() < 30.0
        && ac.alt_ft - end.elevation_ft < 6_000.0
}

/// Landing roll after touchdown, metres: from the aiming point to a taxi speed off the runway.
pub(super) const ROLLOUT_M: f64 = 1_400.0;

/// Over the runway and along it, landing: the last moments of the flare and the landing roll,
/// which the ground flag reports only 10-30 s late.
pub(super) fn landing_on(end: &RunwayEndInfo, ac: &Aircraft) -> bool {
    let CentrelineOffset { along, cross } = centreline_offset(end, [ac.lon, ac.lat]);
    let displaced = distance_m(end.start, end.threshold);
    along <= 300.0
        && along > displaced - end.length_m
        && cross.abs() < 150.0
        && wrap180(ac.track - end.heading).abs() < 30.0
        && ac.alt_ft - end.elevation_ft < 1_000.0
}

/// The index of the element of `indexes` with the smallest `key`: the first seeds, and ties keep
/// the earlier (`Array.reduce` without a seed). `indexes` must not be empty.
fn reduce_smallest(indexes: &[usize], key: impl Fn(usize) -> f64) -> usize {
    let mut best = indexes[0];
    for &i in &indexes[1..] {
        if key(i) < key(best) {
            best = i;
        }
    }
    best
}

/// The landing runway: the one the aircraft is landing on or lined up with (`Aligned`); else the
/// direction other traffic is landing or departing, preferring the parallel arrivals use, then
/// the nearer one (`Traffic`); else the one the shortest arrival reaches (`Guessed`). `None`
/// only without ends. `traffic` was `[]` in the TypeScript when left out.
pub fn choose_arrival_runway<'a>(
    ends: &'a [RunwayEndInfo],
    ac: &Aircraft,
    traffic: &[Traffic],
    ignore_id: Option<u32>,
) -> Option<RunwayChoice<'a>> {
    if ends.is_empty() {
        return None;
    }
    let here: LonLat = [ac.lon, ac.lat];
    let cross_of = |i: usize| centreline_offset(&ends[i], here).cross.abs();
    let landing: Vec<usize> = (0..ends.len()).filter(|&i| landing_on(&ends[i], ac)).collect();
    if !landing.is_empty() {
        return Some(RunwayChoice { end: &ends[reduce_smallest(&landing, cross_of)], reason: RunwayReason::Aligned });
    }
    let aligned: Vec<usize> = (0..ends.len()).filter(|&i| aligned_with(&ends[i], ac)).collect();
    if !aligned.is_empty() {
        return Some(RunwayChoice { end: &ends[reduce_smallest(&aligned, cross_of)], reason: RunwayReason::Aligned });
    }

    let usage = runway_use(ends, traffic, ignore_id);
    let flow = flow_in_use(ends, &usage);
    if !flow.is_empty() {
        let mut end = flow[0];
        for &b in &flow[1..] {
            let d = usage[b].arrivals as i64 - usage[end].arrivals as i64;
            if d > 0 || (d == 0 && cross_of(b) < cross_of(end)) {
                end = b;
            }
        }
        return Some(RunwayChoice { end: &ends[end], reason: RunwayReason::Traffic });
    }
    let mut best = 0;
    let mut best_length = f64::INFINITY;
    for (i, end) in ends.iter().enumerate() {
        let length = arrival_plan(ac, end).length;
        if length < best_length {
            best = i;
            best_length = length;
        }
    }
    Some(RunwayChoice { end: &ends[best], reason: RunwayReason::Guessed })
}

/// The take-off runway, a guess: among the direction traffic is using now if any is seen (the
/// flight left recently, or the wind holds; `Traffic`), else among all ends (`Guessed`), the end
/// whose climb-out turns onto the first tracked position `to` most directly. `None` only without
/// ends. `traffic` was `[]` in the TypeScript when left out.
pub fn choose_departure_runway<'a>(
    ends: &'a [RunwayEndInfo],
    to: &Aircraft,
    traffic: &[Traffic],
) -> Option<RunwayChoice<'a>> {
    if ends.is_empty() {
        return None;
    }
    let flow = flow_in_use(ends, &runway_use(ends, traffic, None));
    let candidates: Vec<usize> = if flow.is_empty() { (0..ends.len()).collect() } else { flow.clone() };
    let mut best = candidates[0];
    let mut best_length = f64::INFINITY;
    for &i in &candidates {
        let length = departure_plan(&ends[i], to).length;
        if length < best_length {
            best = i;
            best_length = length;
        }
    }
    let reason = if flow.is_empty() { RunwayReason::Guessed } else { RunwayReason::Traffic };
    Some(RunwayChoice { end: &ends[best], reason })
}
