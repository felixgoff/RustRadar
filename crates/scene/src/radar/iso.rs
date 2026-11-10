//! Marching squares and the even-odd rule (S1.27): `IsoRings`, `smoothLinear`, `evenOdd`,
//! `ringArea` and `insideRing` of `radar-mesh.ts:591-881`.
//!
//! [`IsoRings`] is d3-contour's `isorings` with `smoothLinear` (ISC licence, Mike Bostock), as
//! `radar-mesh.ts` ported it to flat arrays: the rings come out exactly as d3's do, point for
//! point and in the same order. d3's matching of holes to polygons is left out; [`even_odd`] does
//! it instead. Values are read as `f64` whatever their storage (`f32` for the radar's
//! `Float32Array`s, `f64` for plain arrays), as JavaScript reads both.
//!
//! Coordinates: the value at column `i`, row `j` sits at `(i + 0.5, j + 0.5)`; ring points lie
//! on the edges between values, moved along them to where the values cross the threshold.
//!
//! Deviations, both for input with NaN in it, where the TypeScript would sort in an engine's own
//! order or throw: [`even_odd`] sorts by area with `total_cmp` (a NaN area first), and files a
//! ring whose box is NaN in the grid's first cell.

use std::collections::VecDeque;

/// Marching squares segments per case, `[x0, y0, x1, y1, …]` within the cell, in half cells
/// (d3-contour's table, `radar-mesh.ts:711-728`). The case is
/// `at(x, y + 1) | at(x + 1, y + 1) << 1 | at(x + 1, y) << 2 | at(x, y) << 3`.
pub const CASES: [&[f64]; 16] = [
    &[],
    &[1.0, 1.5, 0.5, 1.0],
    &[1.5, 1.0, 1.0, 1.5],
    &[1.5, 1.0, 0.5, 1.0],
    &[1.0, 0.5, 1.5, 1.0],
    &[1.0, 1.5, 0.5, 1.0, 1.0, 0.5, 1.5, 1.0],
    &[1.0, 0.5, 1.0, 1.5],
    &[1.0, 0.5, 0.5, 1.0],
    &[0.5, 1.0, 1.0, 0.5],
    &[1.0, 1.5, 1.0, 0.5],
    &[0.5, 1.0, 1.0, 0.5, 1.5, 1.0, 1.0, 1.5],
    &[1.5, 1.0, 1.0, 0.5],
    &[0.5, 1.0, 1.5, 1.0],
    &[1.0, 1.5, 1.5, 1.0],
    &[0.5, 1.0, 1.0, 1.5],
    &[],
];

/// An open stretch of a ring while it is being stitched: its end points' keys and its points.
struct Fragment {
    start: usize,
    end: usize,
    ring: VecDeque<f64>,
}

/// Marching squares with the isolines stitched into closed rings and placed by linear
/// interpolation (`class IsoRings`). One tracer serves every threshold of a grid of `dx` × `dy`
/// values; its buffers are reused between calls to [`IsoRings::trace`].
pub struct IsoRings {
    dx: usize,
    dy: usize,
    /// Slots per row of half-cell points, from -0.5 to `dx + 0.5`.
    span: usize,
    /// Fragment number + 1 by start and by end point; 0 when none.
    by_start: Vec<usize>,
    by_end: Vec<usize>,
    /// Per row, the first and last column with a value at or above the threshold.
    first: Vec<isize>,
    last: Vec<isize>,
    fragments: Vec<Fragment>,
}

impl IsoRings {
    /// A tracer for grids of `dx` columns and `dy` rows.
    pub fn new(dx: usize, dy: usize) -> Self {
        // points sit on half cells from -0.5 to d + 0.5: one slot each
        let span = 2 * dx + 3;
        Self {
            dx,
            dy,
            span,
            by_start: vec![0; span * (2 * dy + 3)],
            by_end: vec![0; span * (2 * dy + 3)],
            first: vec![0; dy],
            last: vec![0; dy],
            fragments: Vec::new(),
        }
    }

    /// Calls `emit` with each closed ring (flat `[x0, y0, x1, y1, …]`, the last point repeating
    /// the first) of the cells with `values >= value`, row 0 at the top. `values` holds `dx`
    /// × `dy` values, row by row.
    pub fn trace<T: Copy + Into<f64>>(&mut self, values: &[T], value: f64, mut emit: impl FnMut(Vec<f64>)) {
        let (dx, dy) = (self.dx, self.dy);
        self.by_start.fill(0);
        self.by_end.fill(0);
        self.fragments.clear();
        let mut done = |mut ring: Vec<f64>| {
            smooth_linear(&mut ring, values, dx, dy, value);
            emit(ring);
        };
        // Only the cells around values at or above the threshold can hold a line: each row's
        // span of those is found first, and the rest of the grid (most of a tile, usually) is
        // skipped. The cells visited are the same, in the same order, as d3's full sweep that
        // skips nothing.
        for y in 0..dy {
            let row = y * dx;
            let mut lo = dx as isize;
            let mut hi = -1;
            for x in 0..dx {
                if values[row + x].into() >= value {
                    if lo == dx as isize {
                        lo = x as isize;
                    }
                    hi = x as isize;
                }
            }
            self.first[y] = lo;
            self.last[y] = hi;
        }
        let at = |x: isize, y: isize| -> usize {
            let inside = x >= 0 && (x as usize) < dx && y >= 0 && (y as usize) < dy;
            usize::from(inside && values[y as usize * dx + x as usize].into() >= value)
        };
        // cell (x, y) has the values (x, y) to (x + 1, y + 1) at its corners; the grid is
        // ringed by cells half outside it, from -1 to d - 1
        for y in -1..dy as isize {
            let mut lo = dx as isize;
            let mut hi = -1;
            if y >= 0 {
                (lo, hi) = (self.first[y as usize], self.last[y as usize]);
            }
            if y + 1 < dy as isize {
                (lo, hi) = (lo.min(self.first[(y + 1) as usize]), hi.max(self.last[(y + 1) as usize]));
            }
            for x in lo - 1..=hi {
                let c = at(x, y + 1) | (at(x + 1, y + 1) << 1) | (at(x + 1, y) << 2) | (at(x, y) << 3);
                if c != 0 && c != 15 {
                    self.stitch(c, x as f64, y as f64, &mut done);
                }
            }
        }
    }

    fn key(&self, x: f64, y: f64) -> usize {
        (x * 2.0 + 1.0 + (y * 2.0 + 1.0) * self.span as f64) as usize
    }

    fn stitch(&mut self, c: usize, x: f64, y: f64, done: &mut impl FnMut(Vec<f64>)) {
        let segments = CASES[c];
        for k in (0..segments.len()).step_by(4) {
            let sx = segments[k] + x;
            let sy = segments[k + 1] + y;
            let ex = segments[k + 2] + x;
            let ey = segments[k + 3] + y;
            let si = self.key(sx, sy);
            let ei = self.key(ex, ey);
            let fi = self.by_end[si];
            if fi != 0 {
                let gi = self.by_start[ei];
                if gi != 0 {
                    let (f_end, f_start) = (self.fragments[fi - 1].end, self.fragments[fi - 1].start);
                    let (g_start, g_end) = (self.fragments[gi - 1].start, self.fragments[gi - 1].end);
                    self.by_end[f_end] = 0;
                    self.by_start[g_start] = 0;
                    if fi == gi {
                        let f = &mut self.fragments[fi - 1];
                        f.ring.push_back(ex);
                        f.ring.push_back(ey);
                        done(Vec::from(std::mem::take(&mut f.ring)));
                    } else {
                        let ring: VecDeque<f64> =
                            self.fragments[fi - 1].ring.iter().chain(&self.fragments[gi - 1].ring).copied().collect();
                        self.fragments.push(Fragment { start: f_start, end: g_end, ring });
                        let n = self.fragments.len();
                        self.by_start[f_start] = n;
                        self.by_end[g_end] = n;
                    }
                } else {
                    let f = &mut self.fragments[fi - 1];
                    self.by_end[f.end] = 0;
                    f.ring.push_back(ex);
                    f.ring.push_back(ey);
                    f.end = ei;
                    self.by_end[ei] = fi;
                }
            } else if self.by_start[ei] != 0 {
                // nothing ends where this segment starts (that was the case above)
                let fi = self.by_start[ei];
                let f = &mut self.fragments[fi - 1];
                self.by_start[f.start] = 0;
                f.ring.push_front(sy);
                f.ring.push_front(sx);
                f.start = si;
                self.by_start[si] = fi;
            } else {
                self.fragments.push(Fragment { start: si, end: ei, ring: VecDeque::from([sx, sy, ex, ey]) });
                let n = self.fragments.len();
                self.by_start[si] = n;
                self.by_end[ei] = n;
            }
        }
    }
}

/// Moves each point along its cell edge to where the values cross `value` (`smoothLinear`).
fn smooth_linear<T: Copy + Into<f64>>(ring: &mut [f64], values: &[T], dx: usize, dy: usize, value: f64) {
    let at = |i: isize| -> f64 {
        if i >= 0 && (i as usize) < values.len() { values[i as usize].into() } else { f64::NEG_INFINITY }
    };
    let (dxf, dyf, dxi) = (dx as f64, dy as f64, dx as isize);
    for i in (0..ring.len()).step_by(2) {
        let x = ring[i];
        let y = ring[i + 1];
        // `x | 0`: truncated towards zero; the points lie within a few cells of the grid
        let xt = x as isize;
        let yt = y as isize;
        let v1 = at(yt * dxi + xt);
        if x > 0.0 && x < dxf && xt as f64 == x {
            ring[i] = smooth1(x, at(yt * dxi + xt - 1), v1, value);
        }
        if y > 0.0 && y < dyf && yt as f64 == y {
            ring[i + 1] = smooth1(y, at((yt - 1) * dxi + xt), v1, value);
        }
    }
}

/// Where between the values `v0` (at `x - 0.5`) and `v1` (at `x + 0.5`) the line crosses `value`.
fn smooth1(x: f64, v0: f64, v1: f64, value: f64) -> f64 {
    let a = value - v0;
    let b = v1 - v0;
    let d = if a.is_finite() || b.is_finite() { a / b } else { js_sign(a) / js_sign(b) };
    if d.is_nan() { x } else { x + d - 0.5 }
}

/// `Math.sign`: -1, 0 (keeping its sign), 1, or NaN.
fn js_sign(v: f64) -> f64 {
    if v.is_nan() || v == 0.0 { v } else { v.signum() }
}

/// Shoelace area of a flat ring (`ringArea`); positive for a ring that runs counter-clockwise
/// with y up.
pub fn ring_area(ring: &[f64]) -> f64 {
    let n = ring.len() / 2 * 2;
    let mut sum = 0.0;
    if n == 0 {
        return 0.0;
    }
    let mut j = n - 2;
    let mut i = 0;
    while i < n {
        sum += (ring[j] - ring[i]) * (ring[j + 1] + ring[i + 1]);
        j = i;
        i += 2;
    }
    sum / 2.0
}

/// Whether `(x, y)` is inside the flat ring by the even-odd crossing test (`insideRing`).
pub fn inside_ring(ring: &[f64], x: f64, y: f64) -> bool {
    let n = ring.len() / 2 * 2;
    let mut inside = false;
    if n == 0 {
        return false;
    }
    let mut j = n - 2;
    let mut i = 0;
    while i < n {
        let (xi, yi, xj, yj) = (ring[i], ring[i + 1], ring[j], ring[j + 1]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
        i += 2;
    }
    inside
}

/// A polygon from [`even_odd`]: flat positions, the outer ring first, then its holes, which start
/// at the vertex indices in `hole_indices` (earcut's convention).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RingPolygon {
    pub positions: Vec<f64>,
    pub hole_indices: Vec<usize>,
}

/// A ring and what [`even_odd`] learns of it.
struct RingInfo {
    ring: usize,
    area: f64,
    bbox: [f64; 4],
    depth: usize,
    holes: Vec<usize>,
}

/// `[min x, min y, max x, max y]` of a flat ring, with `Math.min`/`Math.max` semantics.
pub(crate) fn ring_box(ring: &[f64]) -> [f64; 4] {
    use crate::geom::{js_max, js_min};
    let [mut x0, mut y0, mut x1, mut y1] = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in ring.chunks_exact(2) {
        x0 = js_min(x0, p[0]);
        x1 = js_max(x1, p[0]);
        y0 = js_min(y0, p[1]);
        y1 = js_max(y1, p[1]);
    }
    [x0, y0, x1, y1]
}

/// Polygons (outer ring plus holes) filling a set of non-crossing flat rings by the even-odd
/// rule (`evenOdd`): rings nested an even number deep are outer rings, and the rings directly
/// inside them are their holes. A ring's parent is the smallest larger ring whose box holds its
/// box and which holds its first point. Polygons come in order of falling outer-ring area, holes
/// in order of falling area.
pub fn even_odd(rings: &[Vec<f64>]) -> Vec<RingPolygon> {
    let mut info: Vec<RingInfo> = rings
        .iter()
        .enumerate()
        .map(|(i, ring)| RingInfo { ring: i, area: ring_area(ring).abs(), bbox: ring_box(ring), depth: 0, holes: Vec::new() })
        .collect();
    // stable, largest first, as `sort((a, b) => b.area - a.area)`
    info.sort_by(|a, b| b.area.total_cmp(&a.area));
    // rings are filed in a coarse grid by their boxes, so a speckled field with thousands of
    // rings doesn't compare every pair
    const N: usize = 32;
    use crate::geom::{js_max, js_min};
    let [mut gx0, mut gy0, mut gx1, mut gy1] = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for r in &info {
        gx0 = js_min(gx0, r.bbox[0]);
        gy0 = js_min(gy0, r.bbox[1]);
        gx1 = js_max(gx1, r.bbox[2]);
        gy1 = js_max(gy1, r.bbox[3]);
    }
    // `(gx1 - gx0 || 1)`: an empty or NaN extent counts as 1
    let or_one = |d: f64| if d == 0.0 || d.is_nan() { 1.0 } else { d };
    let (wx, wy) = (or_one(gx1 - gx0), or_one(gy1 - gy0));
    // clamped to the grid; `as` maps NaN to 0 where the TypeScript would index with NaN
    let cell = |v: f64, origin: f64, extent: f64| (((v - origin) / extent * N as f64).floor() as i64).clamp(0, N as i64 - 1) as usize;
    let cell_x = |x: f64| cell(x, gx0, wx);
    let cell_y = |y: f64| cell(y, gy0, wy);
    let mut cells: Vec<Vec<usize>> = vec![Vec::new(); N * N];
    for i in 0..info.len() {
        let ring = &rings[info[i].ring];
        let (x, y) = (ring.first().copied().unwrap_or(f64::NAN), ring.get(1).copied().unwrap_or(f64::NAN));
        // the smallest ring around this one is its parent: filed in order of falling area, the
        // last match in the cell is the one
        let candidates = &cells[cell_y(y) * N + cell_x(x)];
        let b = info[i].bbox;
        for &k in candidates.iter().rev() {
            let p = &info[k];
            if b[0] < p.bbox[0] || b[1] < p.bbox[1] || b[2] > p.bbox[2] || b[3] > p.bbox[3] {
                continue;
            }
            if !inside_ring(&rings[p.ring], x, y) {
                continue;
            }
            let depth = p.depth + 1;
            info[i].depth = depth;
            if depth % 2 == 1 {
                let ring = info[i].ring;
                info[k].holes.push(ring);
            }
            break;
        }
        for cy in cell_y(b[1])..=cell_y(b[3]) {
            for cx in cell_x(b[0])..=cell_x(b[2]) {
                cells[cy * N + cx].push(i);
            }
        }
    }
    info.iter()
        .filter(|r| r.depth % 2 == 0)
        .map(|r| {
            let ring = &rings[r.ring];
            let mut positions = Vec::with_capacity(ring.len() + r.holes.iter().map(|&h| rings[h].len()).sum::<usize>());
            positions.extend_from_slice(ring);
            let mut hole_indices = Vec::with_capacity(r.holes.len());
            for &h in &r.holes {
                hole_indices.push(positions.len() / 2);
                positions.extend_from_slice(&rings[h]);
            }
            RingPolygon { positions, hole_indices }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rings_of(values: &[f64], dx: usize, dy: usize, threshold: f64) -> Vec<Vec<f64>> {
        let mut out = Vec::new();
        IsoRings::new(dx, dy).trace(values, threshold, |r| out.push(r));
        out
    }

    #[test]
    fn a_single_value_is_a_diamond_round_its_centre() {
        let rings = rings_of(&[1.0], 1, 1, 0.5);
        assert_eq!(rings.len(), 1);
        let ring = &rings[0];
        // closed: the last point repeats the first
        assert_eq!(ring[..2], ring[ring.len() - 2..]);
        assert_eq!(ring.len(), 10);
        // the four points sit on the grid's border round (0.5, 0.5), where smoothLinear leaves
        // them
        let mut points: Vec<(f64, f64)> = ring[..8].chunks(2).map(|p| (p[0], p[1])).collect();
        points.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(points, [(0.0, 0.5), (0.5, 0.0), (0.5, 1.0), (1.0, 0.5)]);
    }

    #[test]
    fn rings_are_placed_by_linear_interpolation() {
        // a ramp across: 0 | 10, threshold 2.5 crosses a quarter of the way
        let rings = rings_of(&[0.0, 10.0], 2, 1, 2.5);
        assert_eq!(rings.len(), 1);
        let xs: Vec<f64> = rings[0].chunks(2).map(|p| p[0]).collect();
        // between the values at 0.5 and 1.5: 0.5 + 0.25
        assert!(xs.contains(&0.75), "{xs:?}");
        assert!(xs.iter().all(|&x| (0.75..=2.0).contains(&x)), "{xs:?}");
    }

    #[test]
    fn a_ring_with_a_hole_is_two_rings_and_even_odd_nests_them() {
        // a 5 x 5 square of 10 with a 0 in the middle
        let mut values = vec![10.0; 25];
        values[12] = 0.0;
        let rings = rings_of(&values, 5, 5, 5.0);
        assert_eq!(rings.len(), 2);
        let polygons = even_odd(&rings);
        assert_eq!(polygons.len(), 1);
        let p = &polygons[0];
        assert_eq!(p.hole_indices.len(), 1);
        // outer area minus hole area: 25 cells less the corners cut, minus the diamond hole
        let outer = ring_area(&p.positions[..p.hole_indices[0] * 2]).abs();
        let hole = ring_area(&p.positions[p.hole_indices[0] * 2..]).abs();
        assert!(outer > hole && hole > 0.0);
        assert_eq!(hole, 0.5);
    }

    #[test]
    fn even_odd_makes_islands_in_holes_outer_rings() {
        let square = |x0: f64, y0: f64, s: f64| vec![x0, y0, x0 + s, y0, x0 + s, y0 + s, x0, y0 + s];
        let rings = vec![square(3.0, 3.0, 2.0), square(0.0, 0.0, 8.0), square(2.0, 2.0, 4.0), square(20.0, 0.0, 1.0)];
        let polygons = even_odd(&rings);
        // the 8-square with the 4-square as hole; the 2-square inside the hole; the far square
        assert_eq!(polygons.len(), 3);
        assert_eq!(polygons[0].positions[..8], rings[1][..]);
        assert_eq!(polygons[0].hole_indices, [4]);
        assert_eq!(polygons[0].positions[8..], rings[2][..]);
        assert_eq!(polygons[1].positions, rings[0]);
        assert_eq!(polygons[2].positions, rings[3]);
        assert!(polygons[1].hole_indices.is_empty() && polygons[2].hole_indices.is_empty());
    }

    /// d3's full sweep, every cell of every row: what the span-limited sweep must equal.
    fn full_sweep(values: &[f32], dx: usize, dy: usize, value: f64) -> Vec<Vec<f64>> {
        let mut tracer = IsoRings::new(dx, dy);
        let mut out = Vec::new();
        let mut done = |mut ring: Vec<f64>| {
            smooth_linear(&mut ring, values, dx, dy, value);
            out.push(ring);
        };
        let at = |x: isize, y: isize| -> usize {
            let inside = x >= 0 && (x as usize) < dx && y >= 0 && (y as usize) < dy;
            usize::from(inside && f64::from(values[y as usize * dx + x as usize]) >= value)
        };
        for y in -1..dy as isize {
            for x in -1..dx as isize {
                let c = at(x, y + 1) | (at(x + 1, y + 1) << 1) | (at(x + 1, y) << 2) | (at(x, y) << 3);
                if c != 0 && c != 15 {
                    tracer.stitch(c, x as f64, y as f64, &mut done);
                }
            }
        }
        out
    }

    #[test]
    fn the_row_spans_do_not_change_the_rings() {
        // a scattered field with blobs on the edges, in a corner and in the middle, and a hole
        let (dx, dy) = (12, 9);
        let mut values = vec![0.0_f32; dx * dy];
        let mut seed = 7_u32;
        for v in values.iter_mut() {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            if (seed >> 16).is_multiple_of(3) {
                *v = ((seed >> 8) % 20) as f32 + 0.25;
            }
        }
        for (x, y, v) in [(0, 0, 9.0), (1, 0, 7.0), (0, 1, 6.0), (6, 4, 8.0), (7, 4, 9.0), (6, 5, 9.0), (11, 8, 5.0)] {
            values[y * dx + x] = v;
        }
        for threshold in [5.0, 10.0, 15.0] {
            let mut rings = Vec::new();
            IsoRings::new(dx, dy).trace(&values, threshold, |r| rings.push(r));
            assert!(!rings.is_empty());
            assert_eq!(rings, full_sweep(&values, dx, dy, threshold));
            for ring in &rings {
                assert_eq!(ring[..2], ring[ring.len() - 2..]);
                assert!(ring_area(ring).abs() > 0.0);
            }
        }
    }

    #[test]
    fn smooth1_handles_infinities_as_d3_does() {
        // outside the grid counts as -inf: both differences infinite, so their signs divide
        assert_eq!(smooth1(1.0, f64::NEG_INFINITY, 10.0, 5.0), 1.0 + 1.0 - 0.5);
        assert_eq!(smooth1(1.0, 0.0, 10.0, 5.0), 1.0);
        assert_eq!(smooth1(2.0, 4.0, 4.0, 4.0), 2.0);
    }
}
