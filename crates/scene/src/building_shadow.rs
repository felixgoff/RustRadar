//! Port of `building-shadow.ts` (doc 04 §7.6, doc 05 §7.3): building shadows on the ground, cast
//! by the real sun.
//!
//! A building is its footprint extruded straight up. Its shadow on the flat ground is the
//! footprint swept along the sun's ground direction by `height / tan(sun altitude)`: the Minkowski
//! sum of the footprint with that segment. That is the footprint itself plus, for every wall that
//! faces away from the sun, the parallelogram the wall's foot sweeps out. The footprint is under
//! the building and never seen, so only the parallelograms are drawn: two triangles per wall, no
//! tessellation, exact for any footprint, concave or holed. They overlap each other (and other
//! buildings' shadows) where walls meet; the layer that draws them lets only one through per pixel
//! (the stencil rule of doc 04 §7.6).
//!
//! Transliterated as written: `Math.sign` is [`sign0`], `Math.min` the private `js_min` (NaN stays
//! NaN), and the output is `f32` positions and `u32` indices as the TypeScript's typed arrays are.
//! A read past the end of an input reads NaN, where JavaScript reads `undefined`; writes past the
//! end of the output are dropped, as a typed array drops them.

use rustradar_core::num::sign0;

/// At a low sun a shadow is at most this many times the building's height.
pub const SHADOW_LENGTH_CAP: f64 = 10.0;
/// Building shadows in full sun: a little lighter than an aircraft's on the ground.
pub const BUILDING_SHADOW_ALPHA: f64 = 0.35;

/// `Math.min(a, b)`: NaN when either is NaN, and -0 below +0.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b || (a == b && a.is_sign_negative()) {
        a
    } else {
        b
    }
}

/// Where the shadow of a point one metre up lands, as `[east, north]` metres from the point below
/// it, with `to_sun` the direction toward the sun as `[east, north, up]` ([`rustradar_core::sun::local_sun`]).
/// `None` with the sun on or below the horizon. Long shadows are capped at [`SHADOW_LENGTH_CAP`].
pub fn shadow_step(to_sun: [f64; 3]) -> Option<[f64; 2]> {
    shadow_step_capped(to_sun, SHADOW_LENGTH_CAP)
}

/// [`shadow_step`] with the cap given (`shadowStep(toSun, cap)`).
pub fn shadow_step_capped([east, north, up]: [f64; 3], cap: f64) -> Option<[f64; 2]> {
    // `!(up > 0)`: NaN too
    if up.is_nan() || up <= 0.0 {
        return None;
    }
    let across = east.hypot(north);
    if across == 0.0 {
        return Some([0.0, 0.0]);
    }
    // cot(altitude), the length of the shadow per metre of height
    let length = js_min(across / up, cap);
    Some([(-east / across) * length, (-north / across) * length])
}

/// Footprints in the layout of loaders.gl's binary polygons: flat positions (`size` numbers per
/// vertex), and the vertex at which each polygon and each ring starts, each list ending with the
/// vertex count. A polygon's first ring is its outline, the rest are holes; rings may or may not
/// repeat their first vertex at the end.
#[derive(Debug, Clone, Copy)]
pub struct Footprints<'a, P> {
    pub positions: &'a [P],
    pub size: usize,
    pub polygon_starts: &'a [u32],
    pub ring_starts: &'a [u32],
}

/// Triangles on the ground: flat `[x, y, …]` positions and their indices.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShadowMesh {
    pub positions: Vec<f32>,
    pub indices: Vec<u32>,
}

/// The shadows of extruded footprints, in the footprints' own coordinates. `heights[p]` is polygon
/// `p`'s height in metres (0 or less casts nothing) and `step` is where one metre of height lands,
/// in the same units as the positions (from [`shadow_step`], scaled to them). `None` when nothing
/// casts a shadow.
///
/// A `size` of 0 casts nothing (the TypeScript throws a `RangeError` there).
pub fn building_shadows<P, H>(footprints: &Footprints<'_, P>, heights: &[H], step: [f64; 2]) -> Option<ShadowMesh>
where
    P: Copy + Into<f64>,
    H: Copy + Into<f64>,
{
    let Footprints { positions, size, polygon_starts, ring_starts } = *footprints;
    let [sx, sy] = step;
    if sx == 0.0 && sy == 0.0 {
        return None;
    }
    if size == 0 {
        return None;
    }
    let at = |i: i64| -> f64 { usize::try_from(i).ok().and_then(|i| positions.get(i)).map_or(f64::NAN, |&p| p.into()) };
    let size = size as i64;
    let vertex_count = positions.len() / size as usize;
    // at most one wall per vertex, four corners and six indices each
    let mut out: Vec<f32> = Vec::with_capacity(vertex_count * 8);
    let mut indices: Vec<u32> = Vec::with_capacity(vertex_count * 6);
    let mut v: usize = 0;
    let mut k: usize = 0;
    let mut polygon: i64 = -1;
    let polygon_count = polygon_starts.len() as i64 - 1;
    let rings = ring_starts.len() as i64 - 1;
    for r in 0..rings.max(0) as usize {
        let start = i64::from(ring_starts[r]);
        let end = i64::from(ring_starts[r + 1]);
        // which polygon this ring belongs to, and whether it is the outline
        while polygon + 1 < polygon_count && i64::from(polygon_starts[(polygon + 1) as usize]) <= start {
            polygon += 1;
        }
        let outline = polygon >= 0 && i64::from(polygon_starts[polygon as usize]) == start;
        if end - start < 3 {
            continue;
        }
        let height = if polygon >= 0 { heights.get(polygon as usize).map_or(f64::NAN, |&h| h.into()) } else { 0.0 };
        // twice the signed area: positive when the ring runs counter-clockwise (x right, y up)
        let mut area = 0.0;
        let mut j = end - 1;
        for i in start..end {
            area += at(j * size) * at(i * size + 1) - at(i * size) * at(j * size + 1);
            j = i;
        }
        // `!(height > 0)`: NaN (a height missing) too
        if height.is_nan() || height <= 0.0 || area == 0.0 {
            continue;
        }
        // A wall's outward normal points away from the building's material: to
        // the right of a counter-clockwise outline, into a hole.
        let sign = if outline { sign0(area) } else { -sign0(area) };
        let dx = sx * height;
        let dy = sy * height;
        for i in start..end {
            let next = if i + 1 < end { i + 1 } else { start };
            let ax = at(i * size);
            let ay = at(i * size + 1);
            let bx = at(next * size);
            let by = at(next * size + 1);
            let ex = bx - ax;
            let ey = by - ay;
            // the outward normal is sign * (ey, -ex); only walls facing away from the sun cast
            if sign * (ey * dx - ex * dy) <= 0.0 {
                continue;
            }
            let base = (v / 2) as u32;
            for p in [ax, ay, bx, by, bx + dx, by + dy, ax + dx, ay + dy] {
                out.push(p as f32);
            }
            v += 8;
            for o in [0, 1, 2, 0, 2, 3] {
                indices.push(base.wrapping_add(o));
            }
            k += 6;
        }
    }
    if k == 0 {
        return None;
    }
    // what a typed array of the sizes above keeps
    out.truncate(vertex_count * 8);
    indices.truncate(vertex_count * 6);
    Some(ShadowMesh { positions: out, indices })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;
    use std::f64::consts::{FRAC_1_SQRT_2, PI, SQRT_2};

    const DEG: f64 = PI / 180.0;
    type P2 = [f64; 2];

    /// Toward a sun `altitude` degrees up, from the compass `azimuth` (degrees clockwise from north).
    fn sun_at(altitude: f64, azimuth: f64) -> [f64; 3] {
        [
            (altitude * DEG).cos() * (azimuth * DEG).sin(),
            (altitude * DEG).cos() * (azimuth * DEG).cos(),
            (altitude * DEG).sin(),
        ]
    }

    /// Footprints owning their lists, for [`Footprints`] to borrow.
    struct Owned {
        positions: Vec<f64>,
        polygon_starts: Vec<u32>,
        ring_starts: Vec<u32>,
    }

    impl Owned {
        fn borrow(&self) -> Footprints<'_, f64> {
            Footprints { positions: &self.positions, size: 2, polygon_starts: &self.polygon_starts, ring_starts: &self.ring_starts }
        }
    }

    /// Polygons (each a list of rings, outline first) in the binary layout.
    fn footprints(polygons: &[Vec<Vec<P2>>], closed: bool) -> Owned {
        let mut positions = Vec::new();
        let mut polygon_starts = Vec::new();
        let mut ring_starts = Vec::new();
        for rings in polygons {
            polygon_starts.push((positions.len() / 2) as u32);
            for ring in rings {
                ring_starts.push((positions.len() / 2) as u32);
                let mut ring = ring.clone();
                if closed {
                    ring.push(ring[0]);
                }
                for [x, y] in ring {
                    positions.push(x);
                    positions.push(y);
                }
            }
        }
        polygon_starts.push((positions.len() / 2) as u32);
        ring_starts.push((positions.len() / 2) as u32);
        Owned { positions, polygon_starts, ring_starts }
    }

    fn cross(o: P2, a: P2, b: P2) -> f64 {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    }

    /// Even-odd: holes count as outside.
    fn inside(p: P2, rings: &[Vec<P2>]) -> bool {
        let mut odd = false;
        for ring in rings {
            let mut j = ring.len() - 1;
            for i in 0..ring.len() {
                let [xi, yi] = ring[i];
                let [xj, yj] = ring[j];
                if (yi > p[1]) != (yj > p[1]) && p[0] < ((xj - xi) * (p[1] - yi)) / (yj - yi) + xi {
                    odd = !odd;
                }
                j = i;
            }
        }
        odd
    }

    /// `Math.sign(x) !== Math.sign(y)` (NaN differs from everything).
    fn signs_differ(x: f64, y: f64) -> bool {
        sign0(x) != sign0(y)
    }

    fn segments_cross(a: P2, b: P2, c: P2, d: P2) -> bool {
        signs_differ(cross(a, b, c), cross(a, b, d)) && signs_differ(cross(c, d, a), cross(c, d, b))
    }

    /// Ground point `p` is in the shadow when the ray from it toward the sun meets the building below its roof.
    fn shadowed_exactly(p: P2, rings: &[Vec<P2>], d: P2) -> bool {
        let q = [p[0] - d[0], p[1] - d[1]];
        if inside(p, rings) || inside(q, rings) {
            return true;
        }
        rings.iter().any(|ring| (0..ring.len()).any(|i| segments_cross(p, q, ring[i], ring[(i + 1) % ring.len()])))
    }

    fn in_mesh(p: P2, mesh: &ShadowMesh) -> bool {
        let at = |i: u32| -> P2 { [f64::from(mesh.positions[2 * i as usize]), f64::from(mesh.positions[2 * i as usize + 1])] };
        for t in mesh.indices.chunks(3) {
            let (a, b, c) = (at(t[0]), at(t[1]), at(t[2]));
            let s = [cross(a, b, p), cross(b, c, p), cross(c, a, p)];
            if s.iter().all(|&v| v >= 0.0) || s.iter().all(|&v| v <= 0.0) {
                return true;
            }
        }
        false
    }

    /// The drawn shadow plus the (hidden) footprint covers exactly the true shadow.
    fn expect_exact(rings: &[Vec<P2>], height: f64, to_sun: [f64; 3], closed: bool) {
        let step = shadow_step(to_sun).unwrap();
        let owned = footprints(&[rings.to_vec()], closed);
        let mesh = building_shadows(&owned.borrow(), &[height], step).expect("a shadow");
        let d = [step[0] * height, step[1] * height];
        let mut shadowed = 0;
        // a grid offset by irrational amounts, so no sample sits on an edge
        let mut x = -120.0 + SQRT_2 / 7.0;
        while x < 120.0 {
            let mut y = -120.0 + PI / 11.0;
            while y < 120.0 {
                let p = [x, y];
                let expected = shadowed_exactly(p, rings, d);
                assert_eq!(in_mesh(p, &mesh) || inside(p, rings), expected, "at {p:?}, sun {to_sun:?}");
                if expected && !inside(p, rings) {
                    shadowed += 1;
                }
                y += 1.7;
            }
            x += 1.7;
        }
        assert!(shadowed > 50);
    }

    fn reversed(ring: &[P2]) -> Vec<P2> {
        ring.iter().rev().copied().collect()
    }

    fn boxed() -> Vec<P2> {
        vec![[0.0, 0.0], [20.0, 0.0], [20.0, 10.0], [0.0, 10.0]]
    }

    fn l_shape() -> Vec<P2> {
        vec![[0.0, 0.0], [40.0, 0.0], [40.0, 10.0], [10.0, 10.0], [10.0, 30.0], [0.0, 30.0]]
    }

    // describe("shadowStep")

    #[test]
    fn is_height_tan_altitude_away_from_the_sun() {
        let [x, y] = shadow_step(sun_at(45.0, 90.0)).unwrap(); // sun in the east, 45 degrees up
        assert_close!(x, -1, 12);
        assert_close!(y, 0, 12);
        let [x2, y2] = shadow_step(sun_at(30.0, 180.0)).unwrap(); // in the south: the shadow points north
        assert_close!(x2, 0, 12);
        assert_close!(y2, 1.0 / (30.0 * DEG).tan(), 12);
    }

    #[test]
    fn none_at_night_or_with_the_sun_on_the_horizon() {
        assert_eq!(shadow_step(sun_at(-5.0, 120.0)), None);
        assert_eq!(shadow_step([1.0, 0.0, 0.0]), None);
    }

    #[test]
    fn caps_the_length_at_a_low_sun() {
        let [x, y] = shadow_step(sun_at(2.0, 225.0)).unwrap();
        assert_close!(x.hypot(y), SHADOW_LENGTH_CAP, 9);
        // still away from the sun (south-west), so toward the north-east
        assert!(x > 0.0);
        assert_close!(y, x, 9);
        // a higher sun is under the cap and unchanged
        let [x, y] = shadow_step(sun_at(10.0, 225.0)).unwrap();
        assert_close!(x.hypot(y), 1.0 / (10.0 * DEG).tan(), 9);
    }

    #[test]
    fn straight_down_with_the_sun_overhead() {
        assert_eq!(shadow_step([0.0, 0.0, 1.0]), Some([0.0, 0.0]));
    }

    // describe("buildingShadows")

    #[test]
    fn a_boxs_shadow_either_winding_open_or_closed_rings() {
        for sun in [sun_at(40.0, 135.0), sun_at(25.0, 300.0), sun_at(60.0, 10.0), sun_at(15.0, 90.0)] {
            expect_exact(&[boxed()], 30.0, sun, false);
            expect_exact(&[reversed(&boxed())], 30.0, sun, false);
            expect_exact(&[boxed()], 30.0, sun, true);
        }
    }

    #[test]
    fn a_box_under_a_sun_from_the_south_east_walls_facing_away_only() {
        // sun 45 degrees up in the south-east: the west and north walls cast
        let owned = footprints(&[vec![boxed()]], false);
        let mesh = building_shadows(&owned.borrow(), &[10.0], shadow_step(sun_at(45.0, 135.0)).unwrap()).unwrap();
        assert_eq!(mesh.indices.len(), 2 * 6);
        // each parallelogram has the wall's length times the shadow's reach across it
        let d = 10.0 * FRAC_1_SQRT_2;
        let mut area = 0.0;
        for t in mesh.indices.chunks(3) {
            let at = |i: usize| -> P2 {
                [f64::from(mesh.positions[2 * t[i] as usize]), f64::from(mesh.positions[2 * t[i] as usize + 1])]
            };
            area += cross(at(0), at(1), at(2)).abs() / 2.0;
        }
        assert_close!(area, 20.0 * d + 10.0 * d, 3);
    }

    #[test]
    fn an_l_shapes_shadow_is_exact_including_the_inner_corner() {
        for sun in [sun_at(30.0, 45.0), sun_at(30.0, 225.0), sun_at(50.0, 160.0), sun_at(20.0, 280.0)] {
            expect_exact(&[l_shape()], 25.0, sun, false);
            expect_exact(&[reversed(&l_shape())], 25.0, sun, false);
        }
    }

    #[test]
    fn a_courtyards_walls_shade_the_yard() {
        let outer = vec![[0.0, 0.0], [60.0, 0.0], [60.0, 60.0], [0.0, 60.0]];
        let yard = vec![[20.0, 20.0], [20.0, 40.0], [40.0, 40.0], [40.0, 20.0]]; // the opposite winding, as tiles have it
        expect_exact(&[outer.clone(), yard.clone()], 15.0, sun_at(35.0, 200.0), false);
        expect_exact(&[outer, reversed(&yard)], 15.0, sun_at(35.0, 200.0), false);
    }

    #[test]
    fn buildings_of_different_heights_in_one_call() {
        let tall = vec![[50.0, 50.0], [60.0, 50.0], [60.0, 60.0], [50.0, 60.0]];
        let step = shadow_step(sun_at(30.0, 180.0)).unwrap();
        let owned = footprints(&[vec![boxed()], vec![tall]], false);
        let mesh = building_shadows(&owned.borrow(), &[10.0, 40.0], step).unwrap();
        let far = mesh.positions.iter().skip(1).step_by(2).fold(f64::NEG_INFINITY, |m, &y| m.max(f64::from(y)));
        assert_close!(far, 60.0 + 40.0 * step[1], 3);
    }

    #[test]
    fn nothing_with_no_height_or_with_the_sun_overhead() {
        let owned = footprints(&[vec![boxed()]], false);
        assert_eq!(building_shadows(&owned.borrow(), &[0.0], shadow_step(sun_at(30.0, 90.0)).unwrap()), None);
        assert_eq!(building_shadows(&owned.borrow(), &[20.0], shadow_step([0.0, 0.0, 1.0]).unwrap()), None);
    }

    // new tests

    #[test]
    fn the_quads_are_the_walls_swept_along_the_step() {
        // the box's west wall (0,10) -> (0,0) and its north wall (20,10) -> (0,10) under a sun in the south-east
        let owned = footprints(&[vec![boxed()]], false);
        let mesh = building_shadows(&owned.borrow(), &[2.0_f32], [-0.5, 0.25]).unwrap();
        assert_eq!(
            mesh.positions,
            vec![20.0, 10.0, 0.0, 10.0, -1.0, 10.5, 19.0, 10.5, 0.0, 10.0, 0.0, 0.0, -1.0, 0.5, -1.0, 10.5]
        );
        assert_eq!(mesh.indices, vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
    }

    #[test]
    fn short_rings_missing_heights_and_unfinished_lists_cast_nothing() {
        // a two-vertex ring, then a box whose polygon has no height
        let positions = [0.0_f32, 0.0, 1.0, 1.0, 0.0, 0.0, 20.0, 0.0, 20.0, 10.0, 0.0, 10.0];
        let fp = Footprints { positions: &positions, size: 2, polygon_starts: &[0, 2, 6], ring_starts: &[0, 2, 6] };
        assert_eq!(building_shadows(&fp, &[10.0_f32], [1.0, 1.0]), None);
        // with the height there, the box casts
        assert!(building_shadows(&fp, &[10.0_f32, 10.0], [1.0, 1.0]).is_some());
        // no rings at all, or a size of 0
        let empty = Footprints { positions: &positions, size: 2, polygon_starts: &[], ring_starts: &[] };
        assert_eq!(building_shadows(&empty, &[10.0_f32], [1.0, 1.0]), None);
        let zero = Footprints { size: 0, ..fp };
        assert_eq!(building_shadows(&zero, &[10.0_f32, 10.0], [1.0, 1.0]), None);
    }

    #[test]
    fn three_dimensional_positions_are_read_by_their_size() {
        let flat = footprints(&[vec![l_shape()]], false);
        let positions: Vec<f64> = flat.positions.chunks(2).flat_map(|p| [p[0], p[1], 7.0]).collect();
        let deep = Footprints { positions: &positions, size: 3, polygon_starts: &flat.polygon_starts, ring_starts: &flat.ring_starts };
        let step = shadow_step(sun_at(30.0, 45.0)).unwrap();
        assert_eq!(building_shadows(&deep, &[25.0], step), building_shadows(&flat.borrow(), &[25.0], step));
    }
}
