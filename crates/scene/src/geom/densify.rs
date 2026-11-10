//! Subdivision for the globe (S1.24; doc 01 §2): deck.gl's `GlobeView({ resolution: 5 })` cuts
//! what it draws so that no edge spans more than 5 degrees, and so must we, or straight edges
//! would cut through the sphere instead of following it.
//!
//! - lines and outlines: [`densify_line`] and [`densify_ring`] cut every edge into equal pieces no
//!   longer than the step (planar degrees: longitudes are not wrapped, so unwrap first);
//! - textured quads (imagery, hillshade, clouds): [`quad_mesh`], a port of deck.gl 9's BitmapLayer
//!   `createMesh` (`bitmap-layer/create-mesh.js`), a grid of cells no wider and no taller than the
//!   resolution;
//! - polygon fills: [`super::grid_cut::cut_polygon_by_grid`] with `grid_resolution` 5 and
//!   `edge_types`, as deck's SolidPolygonLayer does, then [`super::earcut::earcut`] per part;
//! - paths: [`super::grid_cut::cut_polyline_by_grid`] with `grid_resolution` 5, as deck's
//!   PathLayer does.

use super::{js_hypot2, js_max};

/// deck.gl's globe tessellation step, degrees (`GlobeView({ resolution: 5 })`, G:3687).
pub const GLOBE_RESOLUTION_DEG: f64 = 5.0;

/// How many equal pieces the edge from `a` to `b` is cut into so that none is longer than
/// `max_step` (x and y only): at least one; one when `max_step` is not positive or the edge's
/// length is not finite.
pub fn pieces(a: &[f64], b: &[f64], max_step: f64) -> usize {
    let n = (js_hypot2(b[0] - a[0], b[1] - a[1]) / max_step).ceil();
    if max_step > 0.0 && n.is_finite() && n > 1.0 { n as usize } else { 1 }
}

fn between<const N: usize>(a: [f64; N], b: [f64; N], t: f64) -> [f64; N] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// The line with points added along each edge, evenly, so that no edge is longer than
/// `max_step` (measured in x and y; any further numbers, such as a height, are interpolated). The
/// line's own points are all kept, unchanged.
pub fn densify_line<const N: usize>(line: &[[f64; N]], max_step: f64) -> Vec<[f64; N]> {
    const { assert!(N >= 2, "a point needs an x and a y") };
    let mut out = Vec::with_capacity(line.len());
    for (i, &b) in line.iter().enumerate() {
        if i > 0 {
            let a = line[i - 1];
            let n = pieces(&a, &b, max_step);
            out.extend((1..n).map(|k| between(a, b, k as f64 / n as f64)));
        }
        out.push(b);
    }
    out
}

/// [`densify_line`] for a ring, its closing edge (from the last point back to the first) included;
/// the result repeats its first point at the end only if the ring did.
pub fn densify_ring<const N: usize>(ring: &[[f64; N]], max_step: f64) -> Vec<[f64; N]> {
    let Some(&first) = ring.first() else { return Vec::new() };
    let mut closed = ring.to_vec();
    closed.push(first);
    let mut out = densify_line(&closed, max_step);
    out.pop();
    out
}

/// A textured quad as triangles ([`quad_mesh`]): positions, texture coordinates (v down, as
/// deck's `1 - vt`) and three indices per triangle.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QuadMesh {
    pub positions: Vec<[f64; 3]>,
    pub tex_coords: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// math.gl's `lerp` on points: `t·b + (1 − t)·a`.
fn lerp(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    std::array::from_fn(|i| t * b[i] + (1.0 - t) * a[i])
}

/// `z || 0`.
fn or_zero(z: f64) -> f64 {
    if z == 0.0 || z.is_nan() { 0.0 } else { z }
}

/// deck.gl's `createMesh(bounds, resolution)`: the quad with corners `bounds` = south-west,
/// north-west, north-east, south-east (`[[minX, minY], [minX, maxY], [maxX, maxY], [maxX, minY]]`,
/// each with a height; give 0 for none) as a grid of `ceil(span / resolution)` cells each way,
/// vertices column by column from the west. A `resolution` of 0 (or NaN) gives the plain quad,
/// two triangles. Empty where deck would build typed arrays of a negative or infinite length.
pub fn quad_mesh(bounds: &[[f64; 3]; 4], resolution: f64) -> QuadMesh {
    if resolution == 0.0 || resolution.is_nan() {
        return QuadMesh {
            positions: bounds.iter().map(|p| [p[0], p[1], or_zero(p[2])]).collect(),
            tex_coords: vec![[0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            indices: vec![0, 2, 1, 0, 3, 2],
        };
    }
    let max_x_span = js_max((bounds[0][0] - bounds[3][0]).abs(), (bounds[1][0] - bounds[2][0]).abs());
    let max_y_span = js_max((bounds[1][1] - bounds[0][1]).abs(), (bounds[2][1] - bounds[3][1]).abs());
    let u_count = (max_x_span / resolution).ceil() + 1.0;
    let v_count = (max_y_span / resolution).ceil() + 1.0;
    if !(u_count >= 1.0 && v_count >= 1.0 && u_count.is_finite() && v_count.is_finite()) {
        return QuadMesh::default();
    }
    let (u_count, v_count) = (u_count as usize, v_count as usize);
    let mut mesh = QuadMesh {
        positions: Vec::with_capacity(u_count * v_count),
        tex_coords: Vec::with_capacity(u_count * v_count),
        indices: Vec::with_capacity((u_count - 1) * (v_count - 1) * 6),
    };
    let mut vertex = 0_u32;
    let vc = v_count as u32;
    for u in 0..u_count {
        // (0/0 for a single column, as in deck)
        let ut = u as f64 / (u_count - 1) as f64;
        for v in 0..v_count {
            let vt = v as f64 / (v_count - 1) as f64;
            let p = lerp(lerp(bounds[0], bounds[1], vt), lerp(bounds[3], bounds[2], vt), ut);
            mesh.positions.push([p[0], p[1], or_zero(p[2])]);
            mesh.tex_coords.push([ut as f32, (1.0 - vt) as f32]);
            if u > 0 && v > 0 {
                mesh.indices.extend([vertex - vc, vertex - vc - 1, vertex - 1, vertex - vc, vertex - 1, vertex]);
            }
            vertex += 1;
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::earcut::earcut;
    use crate::geom::grid_cut::{GridOptions, cut_polygon_by_grid};

    /// deck's `land` rectangle (doc 01 §5.5).
    const LAND: [[f64; 2]; 4] = [[-180.0, -85.06], [180.0, -85.06], [180.0, 85.06], [-180.0, 85.06]];

    fn length(a: &[f64], b: &[f64]) -> f64 {
        (b[0] - a[0]).hypot(b[1] - a[1])
    }

    #[test]
    fn densify_leaves_no_edge_over_5_degrees() {
        let ring = densify_ring(&LAND, GLOBE_RESOLUTION_DEG);
        // 72 pieces along each parallel, 35 along each meridian
        assert_eq!(ring.len(), 72 + 35 + 72 + 35);
        for i in 0..ring.len() {
            assert!(length(&ring[i], &ring[(i + 1) % ring.len()]) <= 5.0 + 1e-9);
        }
        // the ring's own corners are kept, in order
        let corners: Vec<[f64; 2]> = ring.iter().copied().filter(|p| LAND.contains(p)).collect();
        assert_eq!(corners, LAND);

        let line = densify_line(&[[-170.0, -80.0, 0.0], [170.0, 80.0, 1000.0], [171.0, 80.0, 0.0]], GLOBE_RESOLUTION_DEG);
        // 375.8 degrees: 76 pieces; then one degree, one piece
        assert_eq!(line.len(), 1 + 75 + 1 + 1);
        for w in line.windows(2) {
            assert!(length(&w[0], &w[1]) <= 5.0 + 1e-9);
        }
        // a height goes along evenly
        assert!((line[38][2] - 500.0).abs() < 1e-9);
        assert_eq!(line[76], [170.0, 80.0, 1000.0]);

        // the globe's polygon fill: no triangle edge wider or taller than a cell
        let flat: Vec<f64> = LAND.concat();
        let parts = cut_polygon_by_grid(&flat, &[], &GridOptions { edge_types: true, ..GridOptions::grid(GLOBE_RESOLUTION_DEG) });
        assert_eq!(parts.len(), 72 * 36);
        let mut area = 0.0;
        for part in &parts {
            let triangles = earcut(&part.positions, &[], 2);
            let p = |k: usize| &part.positions[triangles[k] * 2..triangles[k] * 2 + 2];
            for t in (0..triangles.len()).step_by(3) {
                for (a, b) in [(t, t + 1), (t + 1, t + 2), (t + 2, t)] {
                    assert!((p(a)[0] - p(b)[0]).abs() <= 5.0 && (p(a)[1] - p(b)[1]).abs() <= 5.0);
                }
                let (a, b, c) = (p(t), p(t + 1), p(t + 2));
                area += ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0;
            }
        }
        assert!((area - 360.0 * 170.12).abs() < 1e-6);
    }

    #[test]
    fn short_edges_and_no_step_are_left_alone() {
        let line = [[0.0, 0.0], [3.0, 4.0], [3.0, 4.0], [3.0, 4.5]];
        assert_eq!(densify_line(&line, 5.0), line);
        for step in [0.0, -1.0, f64::NAN] {
            assert_eq!(densify_line(&[[0.0, 0.0], [100.0, 0.0]], step), [[0.0, 0.0], [100.0, 0.0]]);
        }
        assert_eq!(densify_line(&[[0.0, 0.0], [f64::INFINITY, 0.0]], 5.0).len(), 2);
        assert!(densify_line::<2>(&[], 5.0).is_empty());
        assert!(densify_ring::<2>(&[], 5.0).is_empty());
        // a ring closed by a repeat keeps it, and gains nothing on its zero-length last edge
        let closed = densify_ring(&[[0.0, 0.0], [10.0, 0.0], [0.0, 0.0]], 5.0);
        assert_eq!(closed, [[0.0, 0.0], [5.0, 0.0], [10.0, 0.0], [5.0, 0.0], [0.0, 0.0]]);
        assert_eq!(pieces(&[0.0, 0.0], &[10.0, 0.0], 5.0), 2);
        assert_eq!(pieces(&[0.0, 0.0], &[10.0, 0.1], 5.0), 3);
    }

    #[test]
    fn a_quad_mesh_as_deck_builds_it() {
        // deck.gl's createMesh on the same bounds (bun)
        let mesh = quad_mesh(&[[-3.0, 40.1, 0.0], [-3.0, 47.3, 0.0], [9.2, 47.3, 0.0], [9.2, 40.1, 0.0]], 5.0);
        let positions = [
            [-3.0, 40.1, 0.0], [-3.0, 43.7, 0.0], [-3.0, 47.3, 0.0],
            [1.0666666666666664, 40.10000000000001, 0.0], [1.0666666666666664, 43.7, 0.0], [1.0666666666666664, 47.3, 0.0],
            [5.133333333333333, 40.1, 0.0], [5.133333333333333, 43.7, 0.0], [5.133333333333333, 47.3, 0.0],
            [9.2, 40.1, 0.0], [9.2, 43.7, 0.0], [9.2, 47.3, 0.0],
        ];
        assert_eq!(mesh.positions, positions);
        let indices = [1, 0, 3, 1, 3, 4, 2, 1, 4, 2, 4, 5, 4, 3, 6, 4, 6, 7, 5, 4, 7, 5, 7, 8, 7, 6, 9, 7, 9, 10, 8, 7, 10, 8, 10, 11];
        assert_eq!(mesh.indices, indices);
        let third = 0.3333333432674408_f64 as f32;
        let two = 0.6666666865348816_f64 as f32;
        let tex = [[0.0, 1.0], [0.0, 0.5], [0.0, 0.0], [third, 1.0], [third, 0.5], [third, 0.0], [two, 1.0], [two, 0.5], [two, 0.0], [1.0, 1.0], [1.0, 0.5], [1.0, 0.0]];
        assert_eq!(mesh.tex_coords, tex);

        let tilted = quad_mesh(&[[0.0, 0.0, 1.0], [1.0, 6.0, 2.0], [7.0, 7.0, 3.0], [6.0, 1.0, 4.0]], 5.0);
        let positions = [
            [0.0, 0.0, 1.0], [0.5, 3.0, 1.5], [1.0, 6.0, 2.0], [3.0, 0.5, 2.5], [3.5, 3.5, 2.5], [4.0, 6.5, 2.5],
            [6.0, 1.0, 4.0], [6.5, 4.0, 3.5], [7.0, 7.0, 3.0],
        ];
        assert_eq!(tilted.positions, positions);
        assert_eq!(tilted.indices, [1, 0, 3, 1, 3, 4, 2, 1, 4, 2, 4, 5, 4, 3, 6, 4, 6, 7, 5, 4, 7, 5, 7, 8]);

        let plain = quad_mesh(&[[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, 0.0, 0.0]], 0.0);
        assert_eq!(plain.positions, [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, 0.0, 0.0]]);
        assert_eq!(plain.indices, [0, 2, 1, 0, 3, 2]);
        assert_eq!(plain.tex_coords, [[0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]);
        // a negative step: deck would make typed arrays of a negative length
        let square = [[0.0, 0.0, 0.0], [0.0, 10.0, 0.0], [10.0, 10.0, 0.0], [10.0, 0.0, 0.0]];
        assert_eq!(quad_mesh(&square, -5.0), QuadMesh::default());
    }

    #[test]
    fn the_land_quad_mesh_has_no_cell_over_5_degrees() {
        let [sw, se, ne, nw] = LAND.map(|[x, y]| [x, y, 0.0]);
        let mesh = quad_mesh(&[sw, nw, ne, se], GLOBE_RESOLUTION_DEG);
        assert_eq!(mesh.positions.len(), 73 * 36);
        let mut area = 0.0;
        for t in mesh.indices.chunks(3) {
            let p = |k: usize| mesh.positions[t[k] as usize];
            for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                // (5 degrees as `t·b + (1 − t)·a` makes them, give or take the last bits)
                assert!((p(a)[0] - p(b)[0]).abs() <= 5.0 + 1e-9 && (p(a)[1] - p(b)[1]).abs() <= 5.0 + 1e-9);
            }
            let (a, b, c) = (p(0), p(1), p(2));
            area += ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0;
        }
        assert!((area - 360.0 * 170.12).abs() < 1e-6);
    }
}
