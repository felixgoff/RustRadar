//! Port of `train-route.ts` (S1.14, doc 05 §12.7): the selected train's way ahead along its
//! route, from exactly where it is drawn, and the way to where it next stands.
//!
//! The TypeScript knows a cursor's route by object identity (`cursor.route === route`); here the
//! caller numbers its routes (`generation`: a new number for each route it receives) and the
//! cursor keeps that number. The route on from the train (`rest`) is shared through an `Arc`, so
//! "the same lines" is `Arc::ptr_eq`: a renderer can keep the mesh it built from them.

use std::f64::consts::PI;
use std::sync::Arc;

pub use crate::geo::sphere::LonLat;
use crate::model::TrainRoute;
use crate::trains::{Frame, js_max, js_min};

/// Within this of its route (metres), a drawn train is on the track it runs along.
pub const ON_TRACK_M: f64 = 40.0;
/// Further than this from every part of its route (metres), a drawn train isn't on it: the route
/// is out of date.
pub const OFF_ROUTE_M: f64 = 300.0;
/// It is looked for this far along its route on from where it was last (metres): it can't have
/// gone further meanwhile.
const SEARCH_M: f64 = 20_000.0;

/// Where along its route the train was last found: a stretch, and a leg of it (it only ever
/// moves on), and the route on from the end of that leg.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteCursor {
    /// The caller's number for the route this cursor is on.
    pub route: u64,
    pub seg: usize,
    pub leg: usize,
    pub rest: Arc<Vec<Vec<LonLat>>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RouteAhead {
    /// What to draw: from exactly where the train is drawn onto its route (changing as it goes),
    /// then the route on from there (the same lines until it is on a new leg of it). Both empty
    /// when there is none to draw.
    pub head: Vec<LonLat>,
    pub rest: Arc<Vec<Vec<LonLat>>>,
    pub cursor: RouteCursor,
    /// False when the train isn't on its route: the route is out of date, ask again.
    pub on: bool,
}

const M_PER_DEG: f64 = 111_320.0;

/// The point on a leg nearest a point, how far along the leg (0..1), and how far from the point,
/// metres.
#[derive(Debug, Clone, Copy)]
struct Nearest {
    q: LonLat,
    t: f64,
    d: f64,
}

/// The point on the leg `a`-`b` nearest `p` (flat: fine over a few km).
fn nearest_on(p: LonLat, a: LonLat, b: LonLat) -> Nearest {
    let k = (p[1] * PI / 180.0).cos();
    let (ax, ay) = ((a[0] - p[0]) * k, a[1] - p[1]);
    let (dx, dy) = ((b[0] - a[0]) * k, b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { js_max(0.0, js_min(1.0, -(ax * dx + ay * dy) / len2)) } else { 0.0 };
    let (x, y) = (ax + dx * t, ay + dy * t);
    Nearest { q: [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t], t, d: x.hypot(y) * M_PER_DEG }
}

/// Metres between two points (flat).
fn metres(a: LonLat, b: LonLat) -> f64 {
    let k = (a[1] * PI / 180.0).cos();
    ((b[0] - a[0]) * k).hypot(b[1] - a[1]) * M_PER_DEG
}

#[derive(Debug, Clone, Copy)]
struct Found {
    seg: usize,
    leg: usize,
    n: Nearest,
}

/// The first place along the route from `(seg, leg)` where `p` is on it: the nearest point of the
/// first run of legs within [`ON_TRACK_M`] of it (a line that later comes back past it is later
/// on); else, failing that, the nearest point at all (within `SEARCH_M` on).
fn locate(route: &TrainRoute, p: LonLat, from_seg: usize, from_leg: usize) -> Option<Found> {
    let mut first: Option<Found> = None;
    let mut nearest: Option<Found> = None;
    let mut searched = 0.0;
    let mut s = from_seg;
    while s < route.segments.len() && searched < SEARCH_M {
        let line = &route.segments[s];
        let mut j = if s == from_seg { from_leg } else { 0 };
        // `j < line.length - 1`
        while j + 1 < line.len() && searched < SEARCH_M {
            searched += metres(line[j], line[j + 1]);
            let n = nearest_on(p, line[j], line[j + 1]);
            let here = Found { seg: s, leg: j, n };
            if nearest.is_none_or(|best| n.d < best.n.d) {
                nearest = Some(here);
            }
            if n.d <= ON_TRACK_M {
                if first.is_none_or(|best| n.d < best.n.d) {
                    first = Some(here);
                }
            } else if first.is_some() {
                return first;
            }
            j += 1;
        }
        s += 1;
    }
    first.or(nearest)
}

/// The selected train's way ahead from exactly where it is drawn (`drawn`): from the train to
/// where it is on its route, then on along the route; the part behind it trimmed off. A train
/// short of the route's start (drawn a little behind where the route was worked out from) goes
/// there along its own way `ahead` (where it is drawn next, on the track). A train beside the
/// route (on another track) is joined to it, but the route is out of date; one off it, or past
/// its end, has none drawn.
///
/// `generation` numbers `route` (a new number for a new route); `cursor` is the last answer's.
pub fn route_ahead(
    route: &TrainRoute,
    generation: u64,
    drawn: LonLat,
    cursor: Option<&RouteCursor>,
    ahead: &[LonLat],
) -> RouteAhead {
    let from = match cursor {
        Some(c) if c.route == generation => c.clone(),
        _ => RouteCursor { route: generation, seg: 0, leg: 0, rest: Arc::new(Vec::new()) },
    };
    let off = |from: RouteCursor| RouteAhead { head: Vec::new(), rest: Arc::new(Vec::new()), cursor: from, on: false };
    let Some(found) = locate(route, drawn, from.seg, from.leg) else {
        return off(from);
    };
    let line = &route.segments[found.seg];
    // the route on from the end of the leg it is on (kept while it is on that leg)
    let on_from = |leg: usize| -> RouteCursor {
        if from.seg == found.seg && from.leg == leg && !from.rest.is_empty() {
            return from.clone();
        }
        let rest: Vec<Vec<LonLat>> = std::iter::once(line[leg + 1..].to_vec())
            .chain(route.segments[found.seg + 1..].iter().cloned())
            .filter(|l| l.len() >= 2)
            .collect();
        RouteCursor { route: generation, seg: found.seg, leg, rest: Arc::new(rest) }
    };
    let on = found.n.d <= ON_TRACK_M;
    if !on && found.seg == 0 && found.leg == 0 && found.n.t == 0.0 {
        // short of its start: along its own way there
        let at = on_from(0);
        let mut head: Vec<LonLat> = vec![drawn];
        for &p in ahead {
            if metres(p, line[0]) <= ON_TRACK_M {
                head.extend([line[0], line[1]]);
                return RouteAhead { head, rest: Arc::clone(&at.rest), cursor: at, on: true };
            }
            let onto = nearest_on(p, line[0], line[1]);
            if onto.t > 0.0 && onto.d <= ON_TRACK_M {
                head.extend([onto.q, line[1]]);
                return RouteAhead { head, rest: Arc::clone(&at.rest), cursor: at, on: true };
            }
            head.push(p);
        }
        return off(from);
    }
    if found.n.d > OFF_ROUTE_M || (!on && found.n.t == 1.0 && found.leg == line.len() - 2) {
        return off(from);
    }
    let at = on_from(found.leg);
    RouteAhead { head: vec![drawn, found.n.q, line[found.leg + 1]], rest: Arc::clone(&at.rest), cursor: at, on }
}

/// The way from where the train is drawn (`drawn`) to the next place its frames
/// (`(lat, lon, unix ms)`) have it stand still, from `now_ms`: empty when it doesn't stop within
/// them, or stands there already.
pub fn way_to_stop(drawn: LonLat, frames: &[Frame], now_ms: i64) -> Vec<LonLat> {
    let mut way: Vec<LonLat> = vec![drawn];
    for pair in frames.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if b.2 <= now_ms {
            continue;
        }
        // a frame where it was a moment before: it stands there
        if a.0 == b.0 && a.1 == b.1 {
            return if a.2 <= now_ms { Vec::new() } else { way };
        }
        way.push([b.1, b.0]);
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// About 11 m of longitude at 60°N: a straight line east along 60°N, one point every `step`
    /// of it.
    fn east(from: i32, to: i32) -> Vec<LonLat> {
        east_by(from, to, 100)
    }

    fn east_by(from: i32, to: i32, step: i32) -> Vec<LonLat> {
        let mut out = Vec::new();
        let mut i = from;
        while i <= to {
            out.push([18.0 + f64::from(i) * 0.000018, 60.0]);
            i += step;
        }
        out
    }

    /// `m` metres east along the line.
    fn at(m: f64) -> LonLat {
        at_north(m, 0.0)
    }

    fn at_north(m: f64, north: f64) -> LonLat {
        [18.0 + m * 0.000018, 60.0 + north / 111_320.0]
    }

    fn route(segments: Vec<Vec<LonLat>>) -> TrainRoute {
        TrainRoute { segments, stops: Vec::new(), complete: true }
    }

    fn dist(a: LonLat, b: LonLat) -> f64 {
        ((b[0] - a[0]) * 0.5).hypot(b[1] - a[1]) * 111_320.0
    }

    /// As drawn: the head on into the rest, then the stretches after.
    struct Drawn {
        a: RouteAhead,
        paths: Vec<Vec<LonLat>>,
    }

    fn drawn(route: &TrainRoute, generation: u64, p: LonLat, cursor: Option<&RouteCursor>, ahead: &[LonLat]) -> Drawn {
        let a = route_ahead(route, generation, p, cursor, ahead);
        let mut paths = Vec::new();
        if !a.head.is_empty() {
            let mut first = a.head.clone();
            if let Some(next) = a.rest.first() {
                first.extend_from_slice(&next[1..]);
            }
            paths.push(first);
            paths.extend(a.rest.iter().skip(1).cloned());
        }
        Drawn { a, paths }
    }

    fn last(path: &[LonLat]) -> LonLat {
        path[path.len() - 1]
    }

    #[test]
    fn starts_exactly_at_the_train_and_goes_on_from_where_it_is_along_the_route() {
        let r = route(vec![east(0, 2000)]);
        let d = drawn(&r, 1, at(450.0), None, &[]);
        assert!(d.a.on);
        assert_eq!(d.paths[0][0], at(450.0));
        // the next point is where it is on the route, then the route on from there
        assert!(dist(d.paths[0][1], at(450.0)) < 0.5);
        assert!(dist(d.paths[0][2], at(500.0)) < 0.5);
        assert!(dist(last(&d.paths[0]), at(2000.0)) < 0.5);
    }

    #[test]
    fn the_part_behind_the_train_stays_trimmed_off_as_it_goes_on() {
        let r = route(vec![east(0, 2000)]);
        let mut cursor: Option<RouteCursor> = None;
        for m in [100.0, 400.0, 900.0, 1500.0] {
            let d = drawn(&r, 1, at(m), cursor.as_ref(), &[]);
            cursor = Some(d.a.cursor.clone());
            assert_eq!(d.paths[0][0], at(m));
            assert!(d.paths.iter().flatten().all(|p| p[0] >= at(m)[0] - 1e-9));
        }
    }

    #[test]
    fn a_route_that_comes_back_past_the_train_it_is_on_the_first_pass() {
        // out east 1 km and back along a track 10 m north of it
        let back: Vec<LonLat> = east(0, 1000).into_iter().rev().map(|[lon, _]| [lon, 60.0 + 10.0 / 111_320.0]).collect();
        let r = route(vec![[east(0, 1000), back].concat()]);
        let d = drawn(&r, 1, at(300.0), None, &[]);
        assert!(d.a.cursor.leg < 10);
        assert!(d.paths[0].len() > 15);
    }

    #[test]
    fn a_train_on_into_the_next_stretch_past_a_gap_starts_from_there() {
        let r = route(vec![east(0, 1000), east(1500, 3000)]);
        let d = drawn(&r, 1, at(2000.0), None, &[]);
        assert!(d.a.on);
        assert_eq!(d.paths.len(), 1);
        assert_eq!(d.paths[0][0], at(2000.0));
    }

    #[test]
    fn short_of_the_routes_start_it_goes_there_along_its_own_way_ahead() {
        let r = route(vec![east(500, 2000)]);
        // where the train is drawn going next: along the track, past the start
        let ahead = [at(200.0), at(300.0), at(400.0), at(495.0), at(600.0)];
        let d = drawn(&r, 1, at(100.0), None, &ahead);
        assert!(d.a.on);
        assert_eq!(d.paths[0][..4], [at(100.0), at(200.0), at(300.0), at(400.0)]);
        assert!(dist(last(&d.paths[0]), at(2000.0)) < 0.5);
    }

    #[test]
    fn short_of_the_start_with_no_way_of_its_own_there_none_drawn_ask_again() {
        let r = route(vec![east(500, 2000)]);
        let d = drawn(&r, 1, at(100.0), None, &[at(150.0)]);
        assert!(!d.a.on);
        assert!(d.paths.is_empty());
    }

    #[test]
    fn off_the_route_or_past_its_end_none_drawn_ask_again() {
        let r = route(vec![east(0, 1000)]);
        for p in [at_north(500.0, 800.0), at(1200.0)] {
            let d = drawn(&r, 1, p, None, &[]);
            assert!(!d.a.on, "{p:?}");
            assert!(d.paths.is_empty(), "{p:?}");
            assert!(d.a.rest.is_empty(), "{p:?}");
        }
    }

    #[test]
    fn beside_the_route_on_another_track_joined_to_it_but_asked_again() {
        let r = route(vec![east(0, 1000)]);
        let d = drawn(&r, 1, at_north(500.0, 80.0), None, &[]);
        assert!(!d.a.on);
        assert_eq!(d.paths[0][0], at_north(500.0, 80.0));
    }

    #[test]
    fn the_route_on_from_the_train_stays_the_same_lines_while_it_is_on_the_same_leg() {
        let r = route(vec![east(0, 2000)]);
        let a = route_ahead(&r, 1, at(410.0), None, &[]);
        let b = route_ahead(&r, 1, at(450.0), Some(&a.cursor), &[]);
        assert!(Arc::ptr_eq(&b.rest, &a.rest));
        assert_eq!(b.head[0], at(450.0));
        let c = route_ahead(&r, 1, at(520.0), Some(&b.cursor), &[]);
        assert!(!Arc::ptr_eq(&c.rest, &b.rest));
    }

    #[test]
    fn a_new_route_starts_the_search_again() {
        let r1 = route(vec![east(0, 2000)]);
        let a = drawn(&r1, 1, at(1500.0), None, &[]);
        let r2 = route(vec![east(1400, 4000)]);
        let b = drawn(&r2, 2, at(1500.0), Some(&a.a.cursor), &[]);
        assert_eq!(b.a.cursor.route, 2);
        assert!(b.a.on);
        assert_eq!(b.paths[0][0], at(1500.0));
    }

    // --- wayToStop: frames east along 60°N, standing from 30 s

    fn f(m: f64, t: i64) -> Frame {
        (60.0, 18.0 + m * 0.000018, t)
    }

    fn stop_frames() -> Vec<Frame> {
        vec![f(0.0, 0), f(100.0, 10_000), f(200.0, 20_000), f(250.0, 30_000), f(250.0, 60_000), f(400.0, 70_000)]
    }

    #[test]
    fn from_the_train_to_where_it_next_stands() {
        let w = way_to_stop(at(120.0), &stop_frames(), 12_000);
        assert_eq!(w[0], at(120.0));
        assert_eq!(w[1..], [at(200.0), at(250.0)]);
    }

    #[test]
    fn none_while_it_stands_or_with_no_stop_ahead_in_its_frames() {
        assert!(way_to_stop(at(250.0), &stop_frames(), 40_000).is_empty());
        assert!(way_to_stop(at(300.0), &stop_frames(), 62_000).is_empty());
    }

    #[test]
    fn a_stretch_too_short_to_draw_is_left_out_of_the_rest() {
        // a one-point stretch after the leg it is on is no line: the rest skips it
        let r = route(vec![east(0, 1000), vec![at(1200.0)], east_by(1300, 2000, 350)]);
        let a = route_ahead(&r, 1, at(950.0), None, &[]);
        assert!(a.on);
        assert_eq!(a.rest.len(), 1);
        assert_eq!(a.rest[0], east_by(1300, 2000, 350));
    }
}
