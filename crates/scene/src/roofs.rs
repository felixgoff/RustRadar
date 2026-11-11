//! Port of `roofs.ts` (S1.24; doc 04 §7.5, doc 05 §13.1): pitched roofs for Streets GL's
//! buildings, built in metres. The walls stop at the eaves, and over each footprint its roof.
//!
//! A roof is the lowest of a few planes over the footprint's minimum-area rectangle: two slopes
//! meeting at a ridge along its longer side (gabled), with the ends sloped in too (hipped), all
//! four sides (pyramidal), or one slope (skillion). Each plane covers the part of the footprint
//! where it is the lowest (the footprint clipped to it), and the walls are carried up under the
//! roof wherever its edge stands above the eaves (the gables). Shapes Streets GL has and these
//! don't come out as the nearest of them.
//!
//! Transliterated as written: `Math.min`/`Math.max`/`Math.hypot` are [`js_min`], [`js_max`] and
//! [`js_hypot`]; the hull's sort orders as the TypeScript's comparator does for finite numbers
//! (a NaN cannot make it panic). Triangles come from [`earcut`] (math.gl's earcut, the same
//! code as earcut 2.2.4), so they are the TypeScript's own. `pitched` answers
//! only for the twelve names; the TypeScript's `shape in SHAPES` is also true for the names on
//! `Object.prototype` ("toString", "constructor", …), for which it then builds nothing.

use std::f64::consts::PI;

use crate::geom::earcut::earcut;
use crate::geom::{js_hypot, js_hypot2, js_max, js_min};

/// A point, metres east and north.
pub type P = [f64; 2];

/// One building's roof to build.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoofInput<'a> {
    /// The footprint, metres east and north of an origin (a closing repeat of the first point is
    /// ignored), and its courtyards.
    pub ring: &'a [P],
    pub holes: &'a [Vec<P>],
    /// The eaves, metres up, and the roof's height above them.
    pub base: f64,
    pub height: f64,
    /// Streets GL's roof type.
    pub shape: &'a str,
    /// `"across"`: the ridge across the longer side.
    pub orientation: Option<&'a str>,
    /// Degrees from north the roof faces (a skillion's slope runs down that way).
    pub direction: Option<f64>,
}

/// Triangles: their corners (east, north, up), each corner's normal and texture position (in
/// metres), and which corners are walls (gables). Three numbers a corner in `positions` and
/// `normals`, two in `uvs`, one flag in `walls`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RoofParts {
    pub positions: Vec<f64>,
    pub normals: Vec<f64>,
    pub uvs: Vec<f64>,
    pub walls: Vec<bool>,
}

/// A plane over the ground: height in units of the roof's, `c + gx·x + gy·y`.
type Plane = [f64; 3];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Gabled,
    Hipped,
    Pyramidal,
    Skillion,
}

/// `SHAPES`: Streets GL's roof types and the shape each is built as.
fn shape_of(shape: &str) -> Option<Shape> {
    match shape {
        "gabled" | "gambrel" | "saltbox" | "round" => Some(Shape::Gabled),
        "hipped" | "mansard" | "quadrupleSaltbox" | "quadruple_saltbox" => Some(Shape::Hipped),
        "pyramidal" | "dome" | "onion" => Some(Shape::Pyramidal),
        "skillion" => Some(Shape::Skillion),
        _ => None,
    }
}

/// Whether a roof of this type is built (flat ones aren't).
pub fn pitched(shape: &str) -> bool {
    shape_of(shape).is_some()
}

fn cross(o: P, a: P, b: P) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// The convex hull, anticlockwise (Andrew's monotone chain).
fn hull(points: &[P]) -> Vec<P> {
    let mut s = points.to_vec();
    // `a[0] - b[0] || a[1] - b[1]`: for finite numbers the same order (+ 0.0 makes -0 equal +0)
    s.sort_by(|a, b| (a[0] + 0.0).total_cmp(&(b[0] + 0.0)).then((a[1] + 0.0).total_cmp(&(b[1] + 0.0))));
    let half = |list: &mut dyn Iterator<Item = &P>| {
        let mut out: Vec<P> = Vec::new();
        for &p in list {
            while out.len() >= 2 && cross(out[out.len() - 2], out[out.len() - 1], p) <= 0.0 {
                out.pop();
            }
            out.push(p);
        }
        out.pop();
        out
    };
    let mut out = half(&mut s.iter());
    out.extend(half(&mut s.iter().rev()));
    out
}

/// The rectangle of least area round some points ([`bounding_rectangle`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingRectangle {
    /// Its middle.
    pub centre: P,
    /// Its axes (unit vectors).
    pub axes: [P; 2],
    /// Its half-sizes along each axis.
    pub half: [f64; 2],
}

/// The rectangle of least area round the points: over each edge of their convex hull, the
/// smallest box with a side along it. Fewer than two distinct points give a zero-sized rectangle
/// at the origin along x and y.
pub fn bounding_rectangle(points: &[P]) -> BoundingRectangle {
    let h = hull(points);
    let mut best_area = f64::INFINITY;
    let mut best = BoundingRectangle { centre: [0.0, 0.0], axes: [[1.0, 0.0], [0.0, 1.0]], half: [0.0, 0.0] };
    for i in 0..h.len() {
        let (a, b) = (h[i], h[(i + 1) % h.len()]);
        let len = js_hypot2(b[0] - a[0], b[1] - a[1]);
        if len < 1e-9 {
            continue;
        }
        let u: P = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        let v: P = [-u[1], u[0]];
        let (mut u0, mut u1, mut v0, mut v1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
        for p in &h {
            let (pu, pv) = (p[0] * u[0] + p[1] * u[1], p[0] * v[0] + p[1] * v[1]);
            (u0, u1, v0, v1) = (js_min(u0, pu), js_max(u1, pu), js_min(v0, pv), js_max(v1, pv));
        }
        let area = (u1 - u0) * (v1 - v0);
        if area < best_area {
            let (cu, cv) = ((u0 + u1) / 2.0, (v0 + v1) / 2.0);
            best_area = area;
            best = BoundingRectangle {
                centre: [u[0] * cu + v[0] * cv, u[1] * cu + v[1] * cv],
                axes: [u, v],
                half: [(u1 - u0) / 2.0, (v1 - v0) / 2.0],
            };
        }
    }
    best
}

/// The planes a roof is the lowest of, in the ground's own x and y.
fn planes_of(input: &RoofInput, ring: &[P]) -> Vec<Plane> {
    let shape = shape_of(input.shape);
    let r = bounding_rectangle(ring);
    // the ridge along the longer side, unless mapped across it
    let [mut along, mut across] = r.axes;
    let [mut a, mut b] = r.half;
    if (a < b) != (input.orientation == Some("across")) {
        (along, across) = (across, along);
        (a, b) = (b, a);
    }
    if b < 0.05 {
        return Vec::new();
    }
    let [cx, cy] = r.centre;
    // `k + ku·u + kv·v`, with u and v measured along and across from the middle, in x and y
    let plane = |k: f64, ku: f64, kv: f64| -> Plane {
        let (gx, gy) = (ku * along[0] + kv * across[0], ku * along[1] + kv * across[1]);
        [k - gx * cx - gy * cy, gx, gy]
    };
    match shape {
        Some(Shape::Gabled) => vec![plane(1.0, 0.0, -1.0 / b), plane(1.0, 0.0, 1.0 / b)],
        // the ends as steep as the sides
        Some(Shape::Hipped) => vec![
            plane(1.0, 0.0, -1.0 / b),
            plane(1.0, 0.0, 1.0 / b),
            plane(a / b, -1.0 / b, 0.0),
            plane(a / b, 1.0 / b, 0.0),
        ],
        Some(Shape::Pyramidal) => vec![
            plane(1.0, 0.0, -1.0 / b),
            plane(1.0, 0.0, 1.0 / b),
            plane(1.0, -1.0 / a, 0.0),
            plane(1.0, 1.0 / a, 0.0),
        ],
        Some(Shape::Skillion) => {
            // down the way the roof faces (else across), from the top at the far edge
            let down: P = match input.direction {
                Some(deg) if deg.is_finite() => [((deg * PI) / 180.0).sin(), ((deg * PI) / 180.0).cos()],
                _ => across,
            };
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for p in ring {
                let d = p[0] * down[0] + p[1] * down[1];
                (lo, hi) = (js_min(lo, d), js_max(hi, d));
            }
            if hi - lo < 0.05 {
                return Vec::new();
            }
            vec![[hi / (hi - lo), -down[0] / (hi - lo), -down[1] / (hi - lo)]]
        }
        None => Vec::new(),
    }
}

/// One triangle corner of the output.
fn vertex(out: &mut RoofParts, x: f64, y: f64, z: f64, n: [f64; 3], uv: P, wall: bool) {
    out.positions.extend([x, y, z]);
    out.normals.extend(n);
    out.uvs.extend(uv);
    out.walls.push(wall);
}

fn at(p: &Plane, q: P) -> f64 {
    p[0] + p[1] * q[0] + p[2] * q[1]
}

/// The ring clipped to where `f` is at most 0 (Sutherland–Hodgman: one convex constraint at a
/// time).
fn clip(ring: &[P], f: impl Fn(P) -> f64) -> Vec<P> {
    let values: Vec<f64> = ring.iter().map(|&p| f(p)).collect();
    let mut out = Vec::new();
    for i in 0..ring.len() {
        let j = (i + 1) % ring.len();
        let (p, q) = (ring[i], ring[j]);
        let (fp, fq) = (values[i], values[j]);
        if fp <= 0.0 {
            out.push(p);
        }
        if (fp <= 0.0) != (fq <= 0.0) {
            let t = fp / (fp - fq);
            out.push([p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]);
        }
    }
    out
}

/// The ring without a closing repeat of its first point.
fn open(r: &[P]) -> &[P] {
    match r {
        [first, .., last] if first[0] == last[0] && first[1] == last[1] => &r[..r.len() - 1],
        _ => r,
    }
}

/// The ring's signed area, positive anticlockwise.
fn area_of(r: &[P]) -> f64 {
    r.iter().enumerate().fold(0.0, |a, (i, &p)| a + cross([0.0, 0.0], p, r[(i + 1) % r.len()])) / 2.0
}

/// A roof's triangles; none when it has no shape to build (flat, too small, a ring that isn't
/// one).
pub fn roof_parts(input: &RoofInput) -> RoofParts {
    let mut out = RoofParts::default();
    let mut ring = open(input.ring).to_vec();
    // (`!(height > 0.05)` would also stop a NaN height; the TypeScript lets it through)
    if ring.len() < 3 || input.height <= 0.05 || !pitched(input.shape) {
        return out;
    }
    // the outside anticlockwise and the courtyards clockwise, so a wall's outside is to the right
    // of its way along
    let area = area_of(&ring);
    if area.abs() < 1.0 {
        return out;
    }
    if area < 0.0 {
        ring.reverse();
    }
    let holes: Vec<Vec<P>> = input
        .holes
        .iter()
        .map(|h| open(h))
        .filter(|h| h.len() >= 3)
        .map(|h| {
            let mut h = h.to_vec();
            if area_of(&h) > 0.0 {
                h.reverse();
            }
            h
        })
        .collect();
    let planes = planes_of(input, &ring);
    if planes.is_empty() {
        return out;
    }
    let (base, height) = (input.base, input.height);
    let lowest = |q: P| js_max(0.0, planes.iter().fold(f64::INFINITY, |m, p| js_min(m, at(p, q))));

    // the roof's faces: each plane where it is the lowest
    for (i, plane) in planes.iter().enumerate() {
        let lowest_here = |r: &[P]| {
            let mut r = r.to_vec();
            for (j, other) in planes.iter().enumerate() {
                if r.len() < 3 {
                    break;
                }
                if j != i {
                    r = clip(&r, |q| at(plane, q) - at(other, q));
                }
            }
            r
        };
        let outer = lowest_here(&ring);
        if outer.len() < 3 {
            continue;
        }
        let inner: Vec<Vec<P>> = holes.iter().map(|h| lowest_here(h)).filter(|h| h.len() >= 3).collect();
        let mut part = outer;
        let mut hole_starts = Vec::new();
        for h in &inner {
            hole_starts.push(part.len());
            part.extend(h);
        }
        // its normal; across it level, and up it, for its texture (in metres along its face)
        let (gx, gy) = (plane[1] * height, plane[2] * height);
        let n = [-gx, -gy, 1.0];
        let len = js_hypot(&n);
        let normal = n.map(|c| c / len);
        let level: P = if js_hypot2(gx, gy) > 1e-9 {
            let across = js_hypot2(normal[0], normal[1]);
            [-normal[1] / across, normal[0] / across]
        } else {
            [1.0, 0.0]
        };
        // up the face: normal × level
        let up = [-normal[2] * level[1], normal[2] * level[0], normal[0] * level[1] - normal[1] * level[0]];
        let flat: Vec<f64> = part.concat();
        for k in earcut(&flat, &hole_starts, 2) {
            let q = part[k];
            let z = base + height * js_max(0.0, at(plane, q));
            let uv: P = [q[0] * level[0] + q[1] * level[1], q[0] * up[0] + q[1] * up[1] + (z - base) * up[2]];
            vertex(&mut out, q[0], q[1], z, normal, uv, false);
        }
    }

    // the walls up under it: along each side of the footprint, the roof's edge over the eaves
    // (where it is one of the planes' creases, a corner)
    for r in std::iter::once(&ring).chain(&holes) {
        for i in 0..r.len() {
            let (p, q) = (r[i], r[(i + 1) % r.len()]);
            let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
            let len = js_hypot2(dx, dy);
            if len < 1e-6 {
                continue;
            }
            let mut ts = vec![0.0, 1.0];
            for a in 0..planes.len() {
                for b in a + 1..planes.len() {
                    let (fa, fb) = (at(&planes[a], p) - at(&planes[b], p), at(&planes[a], q) - at(&planes[b], q));
                    if fa * fb < 0.0 {
                        ts.push(fa / (fa - fb));
                    }
                }
            }
            ts.sort_by(f64::total_cmp);
            let normal = [dy / len, -dx / len, 0.0];
            let point = |t: f64| -> P { [p[0] + dx * t, p[1] + dy * t] };
            for k in 1..ts.len() {
                let (a, b) = (point(ts[k - 1]), point(ts[k]));
                let (ha, hb) = (height * lowest(a), height * lowest(b));
                if ha < 0.05 && hb < 0.05 {
                    continue;
                }
                let (ua, ub) = (ts[k - 1] * len, ts[k] * len);
                // (in metres along the wall and up it)
                vertex(&mut out, a[0], a[1], base, normal, [ua, 0.0], true);
                vertex(&mut out, b[0], b[1], base, normal, [ub, 0.0], true);
                vertex(&mut out, b[0], b[1], base + hb, normal, [ub, hb], true);
                vertex(&mut out, a[0], a[1], base, normal, [ua, 0.0], true);
                vertex(&mut out, b[0], b[1], base + hb, normal, [ub, hb], true);
                vertex(&mut out, a[0], a[1], base + ha, normal, [ua, ha], true);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;

    /// A house 12 m east–west, 8 m north–south.
    const HOUSE: [P; 4] = [[0.0, 0.0], [12.0, 0.0], [12.0, 8.0], [0.0, 8.0]];

    fn input<'a>(ring: &'a [P], base: f64, height: f64, shape: &'a str) -> RoofInput<'a> {
        RoofInput { ring, holes: &[], base, height, shape, orientation: None, direction: None }
    }

    struct Measured {
        roof: f64,
        wall: f64,
        top: f64,
    }

    /// The roof's triangles' horizontal area and the highest point, and the gable walls' area.
    fn measure(parts: &RoofParts) -> Measured {
        let (mut roof, mut wall, mut top) = (0.0, 0.0, f64::NEG_INFINITY);
        for t in (0..parts.walls.len()).step_by(3) {
            let v = |k: usize| &parts.positions[(t + k) * 3..(t + k) * 3 + 3];
            let (a, b, c) = (v(0), v(1), v(2));
            top = js_max(top, js_max(a[2], js_max(b[2], c[2])));
            let (ux, uy, uz) = (b[0] - a[0], b[1] - a[1], b[2] - a[2]);
            let (wx, wy, wz) = (c[0] - a[0], c[1] - a[1], c[2] - a[2]);
            let n = [uy * wz - uz * wy, uz * wx - ux * wz, ux * wy - uy * wx];
            if parts.walls[t] {
                wall += js_hypot(&n) / 2.0;
            } else {
                roof += n[2].abs() / 2.0;
            }
        }
        Measured { roof, wall, top }
    }

    #[test]
    fn a_gabled_roof_the_ridge_along_the_longer_side_gables_at_the_ends() {
        let parts = roof_parts(&input(&HOUSE, 6.0, 3.0, "gabled"));
        let m = measure(&parts);
        assert_close!(m.roof, 96.0, 3);
        assert_close!(m.top, 9.0, 5);
        // two triangles 8 m wide, 3 m high
        assert_close!(m.wall, 24.0, 3);
        // the ridge runs east–west: every top corner is on the north–south middle
        for p in parts.positions.chunks(3) {
            if (p[2] - 9.0).abs() < 1e-6 {
                assert_close!(p[1], 4.0, 5);
            }
        }
    }

    #[test]
    fn mapped_across_the_ridge_across_the_longer_side() {
        let parts = roof_parts(&RoofInput { orientation: Some("across"), ..input(&HOUSE, 6.0, 3.0, "gabled") });
        for p in parts.positions.chunks(3) {
            if (p[2] - 9.0).abs() < 1e-6 {
                assert_close!(p[0], 6.0, 5);
            }
        }
    }

    #[test]
    fn hipped_and_pyramidal_no_gables_the_whole_footprint_covered() {
        for shape in ["hipped", "pyramidal"] {
            let m = measure(&roof_parts(&input(&HOUSE, 6.0, 3.0, shape)));
            assert_close!(m.roof, 96.0, 3);
            assert_close!(m.wall, 0.0, 5);
            assert_close!(m.top, 9.0, 5);
        }
    }

    #[test]
    fn a_skillion_down_the_way_it_faces_a_wall_under_its_high_edge() {
        let parts = roof_parts(&RoofInput { direction: Some(180.0), ..input(&HOUSE, 3.0, 2.0, "skillion") });
        let m = measure(&parts);
        assert_close!(m.roof, 96.0, 3);
        // high along the north side (it faces south)
        for (i, p) in parts.positions.chunks(3).enumerate() {
            if !parts.walls[i] && p[1] > 7.99 {
                assert_close!(p[2], 5.0, 5);
            }
        }
        assert!(m.wall > 0.0);
    }

    #[test]
    fn an_l_shaped_house_covered_whole_gables_where_its_roof_stands_over_the_eaves() {
        let l: [P; 6] = [[0.0, 0.0], [10.0, 0.0], [10.0, 4.0], [4.0, 4.0], [4.0, 10.0], [0.0, 10.0]];
        let m = measure(&roof_parts(&input(&l, 3.0, 3.0, "gabled")));
        assert_close!(m.roof, 64.0, 3);
        assert!(m.wall > 0.0);
    }

    #[test]
    fn round_a_courtyard_the_yard_left_open_walls_under_the_roof_facing_into_it() {
        let yard = vec![[4.0, 3.0], [8.0, 3.0], [8.0, 5.0], [4.0, 5.0]];
        let holes = [yard];
        let m = measure(&roof_parts(&RoofInput { holes: &holes, ..input(&HOUSE, 6.0, 3.0, "hipped") }));
        assert_close!(m.roof, 96.0 - 8.0, 3);
        assert!(m.wall > 0.0);
    }

    #[test]
    fn flat_roofs_and_degenerate_rings_nothing() {
        assert!(roof_parts(&input(&HOUSE, 6.0, 3.0, "flat")).positions.is_empty());
        assert!(roof_parts(&input(&[[0.0, 0.0], [1.0, 0.0]], 6.0, 3.0, "gabled")).positions.is_empty());
    }

    #[test]
    fn the_least_rectangle_of_a_turned_box() {
        let a = PI / 6.0;
        let turn = |[x, y]: P| -> P { [x * a.cos() - y * a.sin(), x * a.sin() + y * a.cos()] };
        let r = bounding_rectangle(&HOUSE.map(turn));
        assert_close!(r.half[0] * r.half[1] * 4.0, 96.0, 5);
    }
    #[test]
    fn the_same_parts_as_roofs_ts_for_a_turned_skillion() {
        // roofParts on the same input, run with bun: every number the same
        let ring: [P; 5] = [
            [2.160428175621629, -1.2579944745427944],
            [12.009348999383901, 5.620323532591062],
            [7.255903090525279, 13.653364799231936],
            [-3.3811680643163213, 6.713248283867313],
            [2.160428175621629, -1.2579944745427944],
        ];
        let parts = roof_parts(&RoofInput { direction: Some(37.0), ..input(&ring, 10.0, 4.5, "skillion") });
        let (a, b, c, d) = (ring[0], ring[1], ring[2], ring[3]);
        let (zb, zd) = (11.068193839019202, 13.589168110371235);
        // a wall's far end is `p + (q − p)·1`, which can miss `q` in the last bit
        let a2 = [2.1604281756216293, -1.2579944745427944];
        let d2 = [-3.381168064316321, 6.713248283867313];
        #[rustfmt::skip]
        let positions = [
            c[0], c[1], 10.0, d[0], d[1], zd, a[0], a[1], 14.5, a[0], a[1], 14.5, b[0], b[1], zb, c[0], c[1], 10.0,
            a[0], a[1], 10.0, b[0], b[1], 10.0, b[0], b[1], zb, a[0], a[1], 10.0, b[0], b[1], zb, a[0], a[1], 14.5,
            b[0], b[1], 10.0, c[0], c[1], 10.0, c[0], c[1], 10.0, b[0], b[1], 10.0, c[0], c[1], 10.0, b[0], b[1], zb,
            c[0], c[1], 10.0, d2[0], d2[1], 10.0, d2[0], d2[1], zd, c[0], c[1], 10.0, d2[0], d2[1], zd, c[0], c[1], 10.0,
            d[0], d[1], 10.0, a2[0], a2[1], 10.0, a2[0], a2[1], 14.5, d[0], d[1], 10.0, a2[0], a2[1], 14.5, d[0], d[1], zd,
        ];
        assert_eq!(parts.positions, positions);
        let face = [0.1731921700094502, 0.22983377235624255, 0.9576956245759488];
        let sides = [
            [0.572572223337939, -0.8198542852616245, 0.0],
            [0.8606143879367419, 0.5092571798966287, 0.0],
            [-0.5464277552685035, 0.8375062437213375, 0.0],
            [-0.8210797776125675, -0.5708134535868061, 0.0],
        ];
        let normals: Vec<f64> = std::iter::repeat_n(face, 6).chain(sides.iter().flat_map(|&n| std::iter::repeat_n(n, 6))).flatten().collect();
        assert_eq!(parts.normals, normals);
        #[rustfmt::skip]
        let uvs = [
            2.421978187197743, -14.624752913804596, 6.740454552981939, -2.15296917599591, -2.4824746317802426, 1.0120256082062262,
            -2.4824746317802426, 1.0120256082062262, -6.208697426570616, -10.912950585066882, 2.421978187197743, -14.624752913804596,
            0.0, 0.0, 12.013013776733963, 0.0, 12.013013776733963, 1.0681938390192007, 0.0, 0.0, 12.013013776733963, 1.0681938390192007, 0.0, 4.500000000000001,
            0.0, 0.0, 9.334077351297234, 0.0, 9.334077351297234, 4.996003610813204e-16, 0.0, 0.0, 9.334077351297234, 4.996003610813204e-16, 0.0, 1.0681938390192007,
            0.0, 0.0, 12.700885795880538, 0.0, 12.700885795880538, 3.589168110371235, 0.0, 0.0, 12.700885795880538, 3.589168110371235, 0.0, 4.996003610813204e-16,
            0.0, 0.0, 9.7082439194738, 0.0, 9.7082439194738, 4.500000000000001, 0.0, 0.0, 9.7082439194738, 4.500000000000001, 0.0, 3.589168110371235,
        ];
        assert_eq!(parts.uvs, uvs);
        let walls: Vec<bool> = (0..30).map(|i| i >= 6).collect();
        assert_eq!(parts.walls, walls);
    }

    #[test]
    fn every_name_streets_gl_uses_is_built_as_its_shape() {
        for shape in ["gabled", "gambrel", "saltbox", "round", "hipped", "mansard", "quadrupleSaltbox", "quadruple_saltbox", "pyramidal", "dome", "onion", "skillion"] {
            assert!(pitched(shape), "{shape}");
            assert!(!roof_parts(&input(&HOUSE, 6.0, 3.0, shape)).positions.is_empty(), "{shape}");
        }
        for shape in ["flat", "", "Gabled", "toString", "constructor"] {
            assert!(!pitched(shape), "{shape}");
        }
        // the gambrel is built as a gabled roof, the dome as a pyramid
        assert_eq!(roof_parts(&input(&HOUSE, 6.0, 3.0, "gambrel")), roof_parts(&input(&HOUSE, 6.0, 3.0, "gabled")));
        assert_eq!(roof_parts(&input(&HOUSE, 6.0, 3.0, "dome")), roof_parts(&input(&HOUSE, 6.0, 3.0, "pyramidal")));
    }

    #[test]
    fn too_low_too_small_or_too_narrow_nothing() {
        assert!(roof_parts(&input(&HOUSE, 6.0, 0.05, "gabled")).positions.is_empty());
        let tiny: [P; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        assert!(roof_parts(&input(&tiny, 6.0, 3.0, "gabled")).positions.is_empty());
        // a sliver 100 m long and 3 cm wide: the rectangle's half-width is under 5 cm
        let sliver: [P; 4] = [[0.0, 0.0], [100.0, 0.0], [100.0, 0.03], [0.0, 0.03]];
        assert!(roof_parts(&input(&sliver, 6.0, 3.0, "hipped")).positions.is_empty());
        // either winding, the same roof
        let mut clockwise = HOUSE;
        clockwise.reverse();
        let m = measure(&roof_parts(&input(&clockwise, 6.0, 3.0, "gabled")));
        assert_close!(m.roof, 96.0, 3);
        assert_close!(m.wall, 24.0, 3);
    }
}
