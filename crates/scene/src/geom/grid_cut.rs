//! Port of `@math.gl/polygon` 4.1 `cutPolygonByGrid` and `cutPolylineByGrid` (`cut-by-grid.js`,
//! with `bitCode` and `intersect` from `lineclip.js`), holes kept (S1.24; doc 02 §14.3.1).
//!
//! The radar clips its contours to the tile with a grid of 1/8 and cuts wide polygons on a grid
//! of 4 degrees; deck.gl's globe cuts polygons (with `edge_types`) and paths on a grid of
//! `resolution` = 5 degrees ([`super::densify`]). A polygon is bisected at the first grid line
//! its bounding box crosses, again and again, until each part fits in one cell; each hole is
//! bisected at the same line and goes with the halves it has points in. Cut points are snapped
//! onto the grid line, so the parts of neighbouring cells (and tiles) meet exactly.
//!
//! The layout is math.gl's: flat positions, `size` numbers a vertex (x and y are cut, a third
//! number is interpolated), and hole indices that count **values, not vertices** (vertex × size),
//! both in the input and in each [`GridPolygon`] that comes out. earcut ([`super::earcut`]) takes
//! vertex indices: divide by `size`, as `radar-mesh.ts` does.
//!
//! Transliterated as written, including math.gl's reading of the edge type before a ring's first
//! vertex at `edge_types[numPoints - 1]` without the ring's offset (so a hole takes it from the
//! outline). Deviations, all where math.gl would throw or never return: a bisection that leaves a
//! half without a point strictly on its side (math.gl reads `null.pos`; only possible when the
//! grid spacing is not a binary fraction and rounding puts a bounding box on a grid line) keeps
//! the polygon whole; an infinite coordinate, or a grid spacing that is not a positive finite
//! number, cuts nothing (math.gl would loop forever on the first and on a negative spacing);
//! hole indices past the end are read as the end. A path's segment to an infinite point is not
//! cut either.

use std::collections::VecDeque;

use rustradar_core::num::sign0;

/// An edge of the original outline or of a hole ([`GridPolygon::edge_types`]).
pub const TYPE_BORDER: u8 = 1;
/// An edge made by a cut, inside the original polygon.
pub const TYPE_INSIDE: u8 = 0;

/// `cutPolygonByGrid`'s options (math.gl's defaults: size 2, a grid of 10, no offset, no types).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridOptions {
    /// Numbers per vertex: x, y and, from 3, values interpolated along a cut.
    pub size: usize,
    /// The grid's spacing, in x and in y.
    pub grid_resolution: f64,
    /// Where a grid line passes, in x and in y.
    pub grid_offset: [f64; 2],
    /// Whether to tell the original edges from the cut ones ([`TYPE_BORDER`], [`TYPE_INSIDE`]).
    pub edge_types: bool,
}

impl Default for GridOptions {
    fn default() -> Self {
        Self { size: 2, grid_resolution: 10.0, grid_offset: [0.0, 0.0], edge_types: false }
    }
}

impl GridOptions {
    /// Two numbers a vertex, a grid of `grid_resolution` through the origin, no edge types.
    pub fn grid(grid_resolution: f64) -> Self {
        Self { grid_resolution, ..Self::default() }
    }
}

/// One part of a cut polygon: its outline, then its holes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GridPolygon {
    /// `size` numbers a vertex.
    pub positions: Vec<f64>,
    /// Where each hole starts, as an index into `positions` (vertex × size); empty without holes
    /// (math.gl then leaves `holeIndices` out).
    pub hole_indices: Vec<usize>,
    /// With [`GridOptions::edge_types`]: per vertex, the type of the edge from it to the next.
    pub edge_types: Option<Vec<u8>>,
}

/// A polygon waiting to be cut (math.gl's queue entries).
struct Pending {
    pos: Vec<f64>,
    types: Option<Vec<u8>>,
    holes: Vec<usize>,
}

/// One side of a bisected ring.
struct Half {
    pos: Vec<f64>,
    types: Vec<u8>,
}

/// `index || length`: a missing or zero index is the end.
fn or_end(index: Option<usize>, len: usize) -> usize {
    match index {
        Some(i) if i != 0 => i.min(len),
        _ => len,
    }
}

/// `cutPolygonByGrid(positions, holeIndices, options)`: the polygon cut into parts that each fit
/// in one grid cell, holes kept, in math.gl's order. Empty for empty positions (or a `size`
/// below 2).
pub fn cut_polygon_by_grid(positions: &[f64], hole_indices: &[usize], options: &GridOptions) -> Vec<GridPolygon> {
    let GridOptions { size, grid_resolution, grid_offset, edge_types } = *options;
    if positions.is_empty() || size < 2 {
        return Vec::new();
    }
    let types = edge_types.then(|| vec![TYPE_BORDER; positions.len() / size]);
    let cuttable = grid_resolution > 0.0
        && grid_resolution.is_finite()
        && positions.chunks_exact(size).all(|p| !p[0].is_infinite() && !p[1].is_infinite());
    if !cuttable {
        return vec![GridPolygon { positions: positions.to_vec(), hole_indices: hole_indices.to_vec(), edge_types: types }];
    }
    let mut result = Vec::new();
    let mut queue = VecDeque::from([Pending { pos: positions.to_vec(), types, holes: hole_indices.to_vec() }]);
    // recursively bisect the polygon until every part fits in a single grid cell
    while let Some(Pending { pos, types, holes }) = queue.pop_front() {
        let outer_end = or_end(holes.first().copied(), pos.len());
        let [min, max] = bounding_box(&pos, size, outer_end);
        let cell = grid_cell(min, grid_resolution, grid_offset);
        let code = bit_code(&max, &cell);
        let halves = if code != 0 { bisect_polygon(&pos, types.as_deref(), size, 0, outer_end, &cell, code) } else { (None, None) };
        let (Some(low), Some(high)) = halves else {
            // it fits in a single cell (or, where math.gl would throw, cannot be bisected)
            result.push(GridPolygon { positions: pos, hole_indices: holes, edge_types: types });
            continue;
        };
        let tracked = types.is_some();
        let mut low = Pending { pos: low.pos, types: tracked.then_some(low.types), holes: Vec::new() };
        let mut high = Pending { pos: high.pos, types: tracked.then_some(high.types), holes: Vec::new() };
        // split each hole at the same line
        for i in 0..holes.len() {
            let end = or_end(holes.get(i + 1).copied(), pos.len());
            let (l, h) = bisect_polygon(&pos, types.as_deref(), size, holes[i].min(pos.len()), end, &cell, code);
            for (part, half) in [(&mut low, l), (&mut high, h)] {
                if let Some(half) = half {
                    part.holes.push(part.pos.len());
                    part.pos.extend(half.pos);
                    if let Some(t) = &mut part.types {
                        t.extend(half.types);
                    }
                }
            }
        }
        queue.push_back(low);
        queue.push_back(high);
    }
    result
}

/// `bisectPolygon`: the ring from `start` to `end` (indices into `positions`) split at the line
/// of `bbox` that `edge` names (its top when bit 8 is set, else its right), each half with the
/// points on its side and the crossings; a half with no point strictly on its side is `None`.
fn bisect_polygon(
    positions: &[f64],
    edge_types: Option<&[u8]>,
    size: usize,
    start: usize,
    end: usize,
    bbox: &[f64; 4],
    edge: u8,
) -> (Option<Half>, Option<Half>) {
    let num_points = end.saturating_sub(start) / size;
    if num_points == 0 {
        return (None, None);
    }
    let point = |i: usize| &positions[start + i * size..start + (i + 1) * size];
    let side_of = |p: &[f64]| sign0(if edge & 8 != 0 { p[1] - bbox[3] } else { p[0] - bbox[2] });
    let type_at = |i: usize| edge_types.and_then(|t| t.get(i).copied()).unwrap_or(TYPE_INSIDE);
    let tracked = edge_types.is_some();
    let (mut low, mut high) = (Vec::new(), Vec::new());
    let (mut types_low, mut types_high) = (Vec::new(), Vec::new());
    let mut scratch = vec![0.0; size];
    let mut prev = point(num_points - 1);
    let mut prev_side = side_of(prev);
    // (math.gl's index, without the ring's offset)
    let mut prev_type = type_at(num_points - 1);
    let (mut low_count, mut high_count) = (0.0_f64, 0.0_f64);
    let truthy = |s: f64| s != 0.0 && !s.is_nan();
    for i in 0..num_points {
        let p = point(i);
        let side = side_of(p);
        let ty = type_at(start / size + i);
        // if the segment goes through the boundary, add an intersection
        if truthy(side) && truthy(prev_side) && prev_side != side {
            intersect(prev, p, edge, bbox, &mut scratch);
            if push(&mut low, &scratch) && tracked {
                types_low.push(prev_type);
            }
            if push(&mut high, &scratch) && tracked {
                types_high.push(prev_type);
            }
        }
        if side <= 0.0 {
            if push(&mut low, p) && tracked {
                types_low.push(ty);
            }
            low_count -= side;
        } else if let Some(last) = types_low.last_mut() {
            *last = TYPE_INSIDE;
        }
        if side >= 0.0 {
            if push(&mut high, p) && tracked {
                types_high.push(ty);
            }
            high_count += side;
        } else if let Some(last) = types_high.last_mut() {
            *last = TYPE_INSIDE;
        }
        prev = p;
        prev_side = side;
        prev_type = ty;
    }
    (
        (low_count != 0.0).then_some(Half { pos: low, types: types_low }),
        (high_count != 0.0).then_some(Half { pos: high, types: types_high }),
    )
}

/// `getGridCell`: `[left, bottom, right, top]` of the cell `p` is in.
fn grid_cell(p: [f64; 2], grid_resolution: f64, grid_offset: [f64; 2]) -> [f64; 4] {
    let left = ((p[0] - grid_offset[0]) / grid_resolution).floor() * grid_resolution + grid_offset[0];
    let bottom = ((p[1] - grid_offset[1]) / grid_resolution).floor() * grid_resolution + grid_offset[1];
    [left, bottom, left + grid_resolution, bottom + grid_resolution]
}

/// `moveToNeighborCell`: the cell beyond the edge `edge` names.
fn move_to_neighbor_cell(cell: &mut [f64; 4], grid_resolution: f64, edge: u8) {
    if edge & 8 != 0 {
        cell[1] += grid_resolution;
        cell[3] += grid_resolution;
    } else if edge & 4 != 0 {
        cell[1] -= grid_resolution;
        cell[3] -= grid_resolution;
    } else if edge & 2 != 0 {
        cell[0] += grid_resolution;
        cell[2] += grid_resolution;
    } else if edge & 1 != 0 {
        cell[0] -= grid_resolution;
        cell[2] -= grid_resolution;
    }
}

/// `getBoundingBox`: the least and greatest x and y of the vertices before `end` (a NaN is
/// passed over; none gives +∞ and −∞).
fn bounding_box(positions: &[f64], size: usize, end: usize) -> [[f64; 2]; 2] {
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    let mut i = 0;
    while i < end {
        let x = positions.get(i).copied().unwrap_or(f64::NAN);
        let y = positions.get(i + 1).copied().unwrap_or(f64::NAN);
        min_x = if x < min_x { x } else { min_x };
        max_x = if x > max_x { x } else { max_x };
        min_y = if y < min_y { y } else { min_y };
        max_y = if y > max_y { y } else { max_y };
        i += size;
    }
    [[min_x, min_y], [max_x, max_y]]
}

/// lineclip's `bitCode`: where `p` is from `bbox` (`[left, bottom, right, top]`): 1 left, 2 right,
/// 4 below, 8 above.
fn bit_code(p: &[f64], bbox: &[f64; 4]) -> u8 {
    let mut code = 0;
    if p[0] < bbox[0] {
        code |= 1;
    } else if p[0] > bbox[2] {
        code |= 2;
    }
    if p[1] < bbox[1] {
        code |= 4;
    } else if p[1] > bbox[3] {
        code |= 8;
    }
    code
}

/// lineclip's `intersect`: where the segment from `a` to `b` crosses the line of `bbox` that
/// `edge` names (top, bottom, right, left, in that order of precedence), with that coordinate
/// set to the line exactly; the other numbers interpolated. Nothing is written for no edge.
fn intersect(a: &[f64], b: &[f64], edge: u8, bbox: &[f64; 4], out: &mut [f64]) {
    let (t, snap) = if edge & 8 != 0 {
        ((bbox[3] - a[1]) / (b[1] - a[1]), 3)
    } else if edge & 4 != 0 {
        ((bbox[1] - a[1]) / (b[1] - a[1]), 1)
    } else if edge & 2 != 0 {
        ((bbox[2] - a[0]) / (b[0] - a[0]), 2)
    } else if edge & 1 != 0 {
        ((bbox[0] - a[0]) / (b[0] - a[0]), 0)
    } else {
        return;
    };
    for i in 0..a.len() {
        out[i] = if (snap & 1) == i { bbox[snap] } else { t * (b[i] - a[i]) + a[i] };
    }
}

/// math.gl's `push`: appends `source` unless it repeats the last vertex; whether it did.
fn push(target: &mut Vec<f64>, source: &[f64]) -> bool {
    let n = target.len();
    if n > 0 && target[n - source.len()..] == *source {
        return false;
    }
    target.extend_from_slice(source);
    true
}

/// `cutPolylineByGrid`'s options (math.gl's defaults: size 2, not broken, a grid of 10, no offset).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolylineGridOptions {
    /// Numbers per vertex, as for [`GridOptions::size`].
    pub size: usize,
    /// Whether to start a new part at each grid line (else one part with the crossings added).
    pub broken: bool,
    pub grid_resolution: f64,
    pub grid_offset: [f64; 2],
}

impl Default for PolylineGridOptions {
    fn default() -> Self {
        Self { size: 2, broken: false, grid_resolution: 10.0, grid_offset: [0.0, 0.0] }
    }
}

impl PolylineGridOptions {
    /// Two numbers a vertex, one part, a grid of `grid_resolution` through the origin.
    pub fn grid(grid_resolution: f64) -> Self {
        Self { grid_resolution, ..Self::default() }
    }
}

/// `cutPolylineByGrid(positions, options)`: the path with a vertex added wherever it crosses a
/// grid line, as one part, or (`broken`) as one part per cell it passes through. math.gl returns
/// the part itself when not broken; here it is the only element. No parts for a path without a
/// vertex (math.gl returns a part with an undefined point).
pub fn cut_polyline_by_grid(positions: &[f64], options: &PolylineGridOptions) -> Vec<Vec<f64>> {
    let PolylineGridOptions { size, broken, grid_resolution, grid_offset } = *options;
    if size < 2 || positions.len() < size {
        return Vec::new();
    }
    let cuttable = grid_resolution > 0.0 && grid_resolution.is_finite();
    let num_points = positions.len() / size;
    let point = |i: usize| &positions[i * size..(i + 1) * size];
    let mut a = point(0).to_vec();
    let mut cell = grid_cell([a[0], a[1]], grid_resolution, grid_offset);
    let mut scratch = vec![0.0; size];
    let mut crossing = vec![0.0; size];
    let mut result = Vec::new();
    let mut part = Vec::new();
    push(&mut part, &a);
    for i in 1..num_points {
        let b = point(i);
        let cut = cuttable && !b[0].is_infinite() && !b[1].is_infinite();
        let mut code_b = if cut { bit_code(b, &cell) } else { 0 };
        while code_b != 0 {
            // find the intersection with the current cell
            intersect(&a, b, code_b, &cell, &mut scratch);
            let code_alt = bit_code(&scratch, &cell);
            if code_alt != 0 {
                crossing.copy_from_slice(&scratch);
                intersect(&a, &crossing, code_alt, &cell, &mut scratch);
                code_b = code_alt;
            }
            push(&mut part, &scratch);
            // move to the next cell
            a.copy_from_slice(&scratch);
            move_to_neighbor_cell(&mut cell, grid_resolution, code_b);
            if broken && part.len() > size {
                result.push(std::mem::take(&mut part));
                push(&mut part, &a);
            }
            code_b = bit_code(b, &cell);
        }
        push(&mut part, b);
        a.copy_from_slice(b);
    }
    result.push(part);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(positions: &[f64], hole_indices: &[usize], edge_types: Option<&[u8]>) -> GridPolygon {
        GridPolygon { positions: positions.to_vec(), hole_indices: hole_indices.to_vec(), edge_types: edge_types.map(<[u8]>::to_vec) }
    }

    /// The shoelace area of the ring from `start` to `end` (value indices), positive.
    fn ring_area(positions: &[f64], size: usize, start: usize, end: usize) -> f64 {
        let n = (end - start) / size;
        let p = |i: usize| [positions[start + (i % n) * size], positions[start + (i % n) * size + 1]];
        ((0..n).map(|i| p(i)[0] * p(i + 1)[1] - p(i + 1)[0] * p(i)[1]).sum::<f64>() / 2.0).abs()
    }

    /// The outline's area less its holes'.
    fn area(positions: &[f64], hole_indices: &[usize], size: usize) -> f64 {
        let mut ends: Vec<usize> = hole_indices.to_vec();
        ends.push(positions.len());
        let mut total = ring_area(positions, size, 0, ends[0]);
        for w in ends.windows(2) {
            total -= ring_area(positions, size, w[0], w[1]);
        }
        total
    }

    /// Every vertex of every part inside the part's cell, and every vertex that is not one of the
    /// original's exactly on one of the cell's lines; the areas add up to the original's.
    fn check(positions: &[f64], hole_indices: &[usize], options: &GridOptions) -> Vec<GridPolygon> {
        let parts = cut_polygon_by_grid(positions, hole_indices, options);
        let size = options.size;
        let original: Vec<&[f64]> = positions.chunks(size).collect();
        let mut total = 0.0;
        for part in &parts {
            let [min, _] = bounding_box(&part.positions, size, part.positions.len());
            let [left, bottom, right, top] = grid_cell(min, options.grid_resolution, options.grid_offset);
            for p in part.positions.chunks(size) {
                assert!(p[0] >= left && p[0] <= right && p[1] >= bottom && p[1] <= top, "{p:?} outside [{left}, {bottom}, {right}, {top}]");
                if !original.iter().any(|q| q[..2] == p[..2]) {
                    assert!(p[0] == left || p[0] == right || p[1] == bottom || p[1] == top, "{p:?} not on a grid line");
                }
            }
            if let Some(types) = &part.edge_types {
                assert_eq!(types.len(), part.positions.len() / size);
            }
            total += area(&part.positions, &part.hole_indices, size);
        }
        let whole = area(positions, hole_indices, size);
        assert!((total - whole).abs() <= 1e-12 * whole.max(1.0), "parts {total}, whole {whole}");
        parts
    }

    /// A star of `n` points round `centre`, anticlockwise, flat.
    fn star(centre: [f64; 2], outer: f64, inner: f64, n: usize, turn: f64) -> Vec<f64> {
        (0..2 * n)
            .flat_map(|i| {
                let a = turn + std::f64::consts::PI * i as f64 / n as f64;
                let r = if i % 2 == 0 { outer } else { inner };
                [centre[0] + r * a.cos(), centre[1] + r * a.sin()]
            })
            .collect()
    }

    #[test]
    fn a_rectangle_over_six_cells_as_math_gl_cuts_it() {
        let parts = cut_polygon_by_grid(&[1.0, 1.0, 9.0, 1.0, 9.0, 7.0, 1.0, 7.0], &[], &GridOptions::grid(4.0));
        assert_eq!(
            parts,
            [
                piece(&[4.0, 4.0, 1.0, 4.0, 1.0, 1.0, 4.0, 1.0], &[], None),
                piece(&[1.0, 4.0, 4.0, 4.0, 4.0, 7.0, 1.0, 7.0], &[], None),
                piece(&[8.0, 4.0, 4.0, 4.0, 4.0, 1.0, 8.0, 1.0], &[], None),
                piece(&[8.0, 4.0, 8.0, 1.0, 9.0, 1.0, 9.0, 4.0], &[], None),
                piece(&[4.0, 4.0, 8.0, 4.0, 8.0, 7.0, 4.0, 7.0], &[], None),
                piece(&[8.0, 4.0, 9.0, 4.0, 9.0, 7.0, 8.0, 7.0], &[], None),
            ]
        );
    }

    #[test]
    fn a_u_with_a_hole_on_the_radars_grid_as_math_gl_cuts_it() {
        let mut positions = vec![0.05, 0.05, 0.3, 0.05, 0.3, 0.3, 0.2, 0.3, 0.2, 0.15, 0.15, 0.15, 0.15, 0.3, 0.05, 0.3];
        positions.extend([0.07, 0.07, 0.07, 0.2, 0.1, 0.2, 0.1, 0.07]);
        let options = GridOptions { grid_resolution: 1.0 / 8.0, edge_types: true, ..GridOptions::default() };
        let parts = check(&positions, &[16], &options);
        assert_eq!(
            parts,
            [
                piece(&[0.125, 0.125, 0.05, 0.125, 0.05, 0.05, 0.125, 0.05, 0.07, 0.07, 0.07, 0.125, 0.1, 0.125, 0.1, 0.07], &[8], Some(&[0, 1, 1, 0, 1, 0, 1, 1])),
                piece(&[0.25, 0.125, 0.125, 0.125, 0.125, 0.05, 0.25, 0.05], &[], Some(&[0, 0, 1, 0])),
                piece(&[0.25, 0.125, 0.25, 0.05, 0.3, 0.05, 0.3, 0.125], &[], Some(&[0, 1, 1, 0])),
                piece(&[0.125, 0.25, 0.05, 0.25, 0.05, 0.125, 0.125, 0.125, 0.07, 0.125, 0.07, 0.2, 0.1, 0.2, 0.1, 0.125], &[8], Some(&[0, 1, 0, 0, 1, 1, 1, 0])),
                piece(&[0.05, 0.25, 0.125, 0.25, 0.125, 0.3, 0.05, 0.3], &[], Some(&[0, 0, 1, 1])),
                piece(&[0.125, 0.25, 0.125, 0.125, 0.25, 0.125, 0.25, 0.25, 0.2, 0.25, 0.2, 0.15, 0.15, 0.15, 0.15, 0.25], &[], Some(&[0, 0, 0, 0, 1, 1, 1, 0])),
                piece(&[0.25, 0.125, 0.3, 0.125, 0.3, 0.25, 0.25, 0.25], &[], Some(&[0, 1, 0, 0])),
                piece(&[0.125, 0.25, 0.25, 0.25, 0.25, 0.3, 0.2, 0.3, 0.2, 0.25, 0.15, 0.25, 0.15, 0.3, 0.125, 0.3], &[], Some(&[0, 0, 1, 1, 0, 1, 1, 0])),
                piece(&[0.25, 0.25, 0.3, 0.25, 0.3, 0.3, 0.25, 0.3], &[], Some(&[0, 1, 1, 0])),
            ]
        );
    }

    #[test]
    fn heights_are_interpolated_along_the_cuts_as_math_gl_does() {
        let options = GridOptions { size: 3, grid_resolution: 5.0, grid_offset: [2.0, 1.0], edge_types: true };
        let parts = check(&[0.0, 0.0, 10.0, 12.0, 0.0, 20.0, 6.0, 9.0, 30.0], &[], &options);
        let t = 13.333333333333332;
        assert_eq!(
            parts,
            [
                piece(&[2.0, 1.0, t, 0.666666666666667, 1.0, 12.222222222222221, 0.0, 0.0, 10.0, 2.0, 0.0, 11.666666666666666], &[], Some(&[0, 1, 1, 0])),
                piece(&[7.0, 1.0, 17.5, 2.0, 1.0, t, 2.0, 0.0, 11.666666666666666, 7.0, 0.0, 15.833333333333332], &[], Some(&[0, 0, 1, 0])),
                piece(&[7.0, 1.0, 17.5, 7.0, 0.0, 15.833333333333332, 12.0, 0.0, 20.0, 11.333333333333334, 1.0, 21.11111111111111], &[], Some(&[0, 1, 1, 0])),
                piece(&[2.0, 2.9999999999999996, 16.666666666666664, 0.666666666666667, 1.0, 12.222222222222221, 2.0, 1.0, t], &[], Some(&[1, 0, 0])),
                piece(&[4.0, 6.0, 23.333333333333332, 7.0, 6.0, 25.833333333333332, 7.0, 7.5, 28.333333333333332, 6.0, 9.0, 30.0], &[], Some(&[0, 0, 1, 1])),
                piece(&[7.0, 6.0, 25.833333333333332, 8.0, 6.0, 26.666666666666664, 7.0, 7.5, 28.333333333333332], &[], Some(&[0, 1, 0])),
                piece(&[7.0, 6.0, 25.833333333333332, 4.0, 6.0, 23.333333333333332, 2.0, 2.9999999999999996, 16.666666666666664, 2.0, 1.0, t, 7.0, 1.0, 17.5], &[], Some(&[0, 1, 0, 0, 0])),
                piece(&[7.0, 6.0, 25.833333333333332, 7.0, 1.0, 17.5, 11.333333333333334, 1.0, 21.11111111111111, 8.0, 6.0, 26.666666666666664], &[], Some(&[0, 0, 1, 0])),
            ]
        );
    }

    #[test]
    fn grid_cut_keeps_the_total_area_and_clips_exactly_at_grid_lines() {
        // a star with two star-shaped holes, on the radar's 4-degree grid and on the globe's 5
        let mut positions = star([20.3, 10.1], 9.0, 4.0, 7, 0.1);
        let first = positions.len();
        let mut hole = star([20.3, 10.1], 2.5, 1.0, 5, 0.3);
        // holes go the other way round
        hole.reverse_pairs();
        positions.extend(&hole);
        let second = positions.len();
        let mut tip = star([28.0, 11.5], 0.6, 0.3, 3, 0.0);
        tip.reverse_pairs();
        positions.extend(&tip);
        for options in [GridOptions::grid(4.0), GridOptions { grid_resolution: 5.0, grid_offset: [0.5, -0.25], edge_types: true, ..GridOptions::default() }] {
            let parts = check(&positions, &[first, second], &options);
            assert!(parts.len() > 10);
            assert!(parts.iter().any(|p| !p.hole_indices.is_empty()));
        }
        // and in tile units on the radar's grid of 1/8, with a height
        let tile: Vec<f64> = star([0.5, 0.5], 0.45, 0.2, 9, 0.2).chunks(2).flat_map(|p| [p[0], p[1], p[0] + p[1]]).collect();
        let parts = check(&tile, &[], &GridOptions { size: 3, grid_resolution: 1.0 / 8.0, ..GridOptions::default() });
        // the third number stays where the cut lies: x + y along straight edges
        for p in parts.iter().flat_map(|p| p.positions.chunks(3)) {
            assert!((p[2] - (p[0] + p[1])).abs() < 1e-12);
        }
    }

    #[test]
    fn what_fits_in_a_cell_comes_back_as_it_is() {
        let positions = [1.0, 1.0, 3.0, 1.0, 3.0, 3.0, 1.0, 3.0, 1.5, 1.5, 1.5, 2.0, 2.0, 2.0];
        let parts = cut_polygon_by_grid(&positions, &[8], &GridOptions { edge_types: true, ..GridOptions::grid(4.0) });
        assert_eq!(parts, [piece(&positions, &[8], Some(&[1; 7]))]);
        assert!(cut_polygon_by_grid(&[], &[], &GridOptions::default()).is_empty());
    }

    #[test]
    fn nothing_is_cut_on_a_grid_that_is_not_one_or_at_an_infinite_point() {
        let positions = [1.0, 1.0, 30.0, 1.0, 30.0, 30.0];
        for resolution in [0.0, -4.0, f64::NAN, f64::INFINITY] {
            assert_eq!(cut_polygon_by_grid(&positions, &[], &GridOptions::grid(resolution)), [piece(&positions, &[], None)]);
            assert_eq!(cut_polyline_by_grid(&positions, &PolylineGridOptions::grid(resolution)), [positions.to_vec()]);
        }
        let far = [1.0, 1.0, f64::INFINITY, 1.0, 30.0, 30.0];
        assert_eq!(cut_polygon_by_grid(&far, &[], &GridOptions::grid(4.0)), [piece(&far, &[], None)]);
        assert_eq!(cut_polyline_by_grid(&[1.0, 1.0, f64::INFINITY, 1.0], &PolylineGridOptions::grid(4.0)), [vec![1.0, 1.0, f64::INFINITY, 1.0]]);
    }

    #[test]
    fn a_path_gets_a_vertex_at_each_grid_line_as_math_gl_adds_them() {
        let path = [-12.0, -3.0, 7.0, 9.0, 13.0, -4.0];
        let cut = cut_polyline_by_grid(&path, &PolylineGridOptions::grid(5.0));
        let expected = [
            -12.0, -3.0, -10.0, -1.736842105263158, -7.25, 0.0, -5.0, 1.4210526315789473, 0.0, 4.578947368421053,
            0.666666666666666, 5.0, 5.0, 7.7368421052631575, 7.0, 9.0, 8.846153846153847, 5.0, 10.0, 2.5,
            11.153846153846153, 0.0, 13.0, -4.0,
        ];
        assert_eq!(cut, [expected.to_vec()]);
        let broken = cut_polyline_by_grid(&path, &PolylineGridOptions { broken: true, ..PolylineGridOptions::grid(5.0) });
        let parts: Vec<Vec<f64>> = vec![
            vec![-12.0, -3.0, -10.0, -1.736842105263158],
            vec![-10.0, -1.736842105263158, -7.25, 0.0],
            vec![-7.25, 0.0, -5.0, 1.4210526315789473],
            vec![-5.0, 1.4210526315789473, 0.0, 4.578947368421053],
            vec![0.0, 4.578947368421053, 0.666666666666666, 5.0],
            vec![0.666666666666666, 5.0, 5.0, 7.7368421052631575],
            vec![5.0, 7.7368421052631575, 7.0, 9.0, 8.846153846153847, 5.0],
            vec![8.846153846153847, 5.0, 10.0, 2.5],
            vec![10.0, 2.5, 11.153846153846153, 0.0],
            vec![11.153846153846153, 0.0, 13.0, -4.0],
        ];
        assert_eq!(broken, parts);
        let high = cut_polyline_by_grid(&[0.0, 0.0, 0.0, 11.0, 0.0, 110.0, 11.0, 11.0, 0.0], &PolylineGridOptions { size: 3, ..PolylineGridOptions::grid(5.0) });
        assert_eq!(high, [vec![0.0, 0.0, 0.0, 5.0, 0.0, 50.0, 10.0, 0.0, 100.0, 11.0, 0.0, 110.0, 11.0, 5.0, 60.0, 11.0, 10.0, 10.0, 11.0, 11.0, 0.0]]);
        assert!(cut_polyline_by_grid(&[], &PolylineGridOptions::default()).is_empty());
    }

    #[test]
    fn a_cut_path_has_no_segment_wider_or_taller_than_a_cell() {
        let path = [-179.0, -84.0, 178.0, 83.5, 10.0, -60.0, 10.5, 70.0];
        let cut = cut_polyline_by_grid(&path, &PolylineGridOptions::grid(5.0));
        let points: Vec<&[f64]> = cut[0].chunks(2).collect();
        for w in points.windows(2) {
            let [left, bottom, right, top] = grid_cell([w[0][0].min(w[1][0]), w[0][1].min(w[1][1])], 5.0, [0.0, 0.0]);
            for p in w {
                assert!(p[0] >= left && p[0] <= right && p[1] >= bottom && p[1] <= top, "{w:?}");
            }
        }
    }

    trait ReversePairs {
        fn reverse_pairs(&mut self);
    }

    impl ReversePairs for Vec<f64> {
        /// Reverses the order of the (x, y) vertices.
        fn reverse_pairs(&mut self) {
            let pairs: Vec<[f64; 2]> = self.chunks(2).rev().map(|p| [p[0], p[1]]).collect();
            *self = pairs.concat();
        }
    }
}
