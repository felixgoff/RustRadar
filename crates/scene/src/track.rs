//! The realistic map's railway track in 3D, close in (item R13): what `rustradar_map`'s ground
//! rails layer draws over the flat track (`crate::ground_rails`) where the camera is near enough
//! to see it. Made on the worker pool with the tile's ground rails, from the same lines.
//!
//! Each track is a raised **ballast bed** (a flat top [`BED_TOP_HALF_M`] either side of the
//! middle, [`BED_M`] up, sloping down to the ground [`BED_TOE_HALF_M`] out), **sleepers** across it
//! every [`SLEEPER_SPACING_M`] (instances of one box, [`SLEEPER`]), and **two steel rails** on them
//! at the track's gauge, their tops [`RAIL_TOP_M`] up. A bridge's track stands on its deck: each
//! vertex carries the height it stands on (its *base*: the deck as `crate::bridges::bridge_parts`
//! raises it, else 0), which the map replaces by the ground under it on the 3D terrain (where no
//! deck is drawn).
//!
//! Lines are clipped to the tile's square here (the flat track is clipped by the stencil; a solid
//! drawn twice by two tiles would fight itself), and both the mesh's triangles and the sleepers
//! are sorted into [`TrackCell`]s about [`CELL_M`] across, so that only those near the camera are
//! drawn. Positions are in the tile's units (`u` east, `v` south) and metres up; normals east,
//! north, up; paints the ground's (`crate::ground::paint`).

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use glam::DVec2;

use crate::ground::{Rgba, paint};

/// The ballast bed's height over what it stands on, metres.
pub const BED_M: f64 = 0.3;
/// Half the bed's flat top, metres.
pub const BED_TOP_HALF_M: f64 = 1.65;
/// Half the bed's width at its toe, on the ground, metres (as wide as the flat track's ballast).
pub const BED_TOE_HALF_M: f64 = 2.3;
/// A sleeper's top, metres up; its underside sits in the ballast.
pub const SLEEPER_TOP_M: f64 = 0.37;
const SLEEPER_BOTTOM_M: f64 = 0.2;
/// A sleeper's length across the track and its width along it, metres.
const SLEEPER_LONG_M: f64 = 2.6;
const SLEEPER_WIDE_M: f64 = 0.26;
/// From one sleeper to the next, metres.
pub const SLEEPER_SPACING_M: f64 = 0.65;
/// The rails' tops, metres up: where a train's wheels run.
pub const RAIL_TOP_M: f64 = 0.52;
/// A rail's head, metres across (a little wider than a real one's 7 cm: it is seen from afar).
const RAIL_HEAD_M: f64 = 0.08;
/// Standard gauge, metres between the rails' inner edges; a track's own (`gaugeint0`, mm) where
/// it says so.
pub const STANDARD_GAUGE_M: f64 = 1.435;
/// The cells the mesh and sleepers are sorted into, metres across.
pub const CELL_M: f64 = 256.0;
/// A segment longer than this is cut, so each lies near the cell it is sorted into, metres.
const MAX_SEGMENT_M: f64 = 32.0;

/// The ballast: a gravel bed, its texture fixed to the world.
pub const BALLAST: Rgba = paint(18, [170, 160, 148]);
/// The rails' heads, bright where the wheels polish them, and their sides, rust brown.
pub const RAIL_HEAD: Rgba = paint(19, [255, 255, 255]);
pub const RAIL_SIDE: Rgba = paint(19, [150, 98, 70]);
/// Concrete sleepers.
pub const SLEEPER_PAINT: Rgba = paint(20, [255, 255, 255]);

/// One vertex of the track's mesh: `(u, v, h)` with `h` metres over its base, its normal (east,
/// north, up), its paint, and its base (metres: a bridge's deck, else 0). 32 bytes, `#[repr(C)]`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct TrackVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub paint: Rgba,
    pub base: f32,
}

/// One sleeper: its middle `(u, v)` on its base, the track's way there (tile units per metre along
/// it, so `(u, v) + dir · x` is `x` metres on), and its base. 20 bytes, `#[repr(C)]`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Sleeper {
    pub at: [f32; 2],
    pub dir: [f32; 2],
    pub base: f32,
}

/// One vertex of the sleeper box: metres along the track, across it (left) and up, its normal in
/// the same axes, and its paint. 28 bytes, `#[repr(C)]`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct BoxVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub paint: Rgba,
}

/// The sleeper every [`Sleeper`] places: a box without its underside (5 faces), and its indices.
pub fn sleeper_box() -> (Vec<BoxVertex>, Vec<u32>) {
    let (x, y) = ((SLEEPER_WIDE_M / 2.0) as f32, (SLEEPER_LONG_M / 2.0) as f32);
    let (z0, z1) = (SLEEPER_BOTTOM_M as f32, SLEEPER_TOP_M as f32);
    let faces: [([f32; 3], [[f32; 3]; 4]); 5] = [
        ([0.0, 0.0, 1.0], [[-x, -y, z1], [x, -y, z1], [x, y, z1], [-x, y, z1]]),
        ([1.0, 0.0, 0.0], [[x, -y, z0], [x, y, z0], [x, y, z1], [x, -y, z1]]),
        ([-1.0, 0.0, 0.0], [[-x, y, z0], [-x, -y, z0], [-x, -y, z1], [-x, y, z1]]),
        ([0.0, 1.0, 0.0], [[x, y, z0], [-x, y, z0], [-x, y, z1], [x, y, z1]]),
        ([0.0, -1.0, 0.0], [[-x, -y, z0], [x, -y, z0], [x, -y, z1], [-x, -y, z1]]),
    ];
    let (mut vertices, mut indices) = (Vec::new(), Vec::new());
    for (normal, corners) in faces {
        let base = vertices.len() as u32;
        vertices.extend(corners.map(|position| BoxVertex { position, normal, paint: SLEEPER_PAINT }));
        indices.extend([0, 1, 2, 0, 2, 3].map(|k| base + k));
    }
    (vertices, indices)
}

/// Some of a tile's track lying together: what of it to draw when the camera is near.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackCell {
    /// Its middle, tile units.
    pub centre: [f32; 2],
    /// How far from the middle anything of it lies, metres (level).
    pub radius_m: f32,
    /// Its triangles in [`TrackMesh::indices`] and its sleepers in [`TrackMesh::sleepers`].
    pub indices: Range<u32>,
    pub sleepers: Range<u32>,
}

/// One tile's track in 3D.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackMesh {
    pub vertices: Vec<TrackVertex>,
    pub indices: Vec<u32>,
    pub sleepers: Vec<Sleeper>,
    pub cells: Vec<TrackCell>,
    /// Each track's bed for its shadow: per vertex of its line its left and right edges at the
    /// bed's top, `(u, v, top)`, with the base under them.
    pub outlines: Vec<Vec<([[f32; 3]; 2], f32)>>,
    /// Metres per tile unit.
    pub metres_per_unit: f64,
}

impl TrackMesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty() && self.sleepers.is_empty()
    }

    /// The bytes it holds.
    pub fn byte_size(&self) -> usize {
        size_of_val(self.vertices.as_slice())
            + size_of_val(self.indices.as_slice())
            + size_of_val(self.sleepers.as_slice())
            + self.cells.len() * size_of::<TrackCell>()
            + self.outlines.iter().map(|o| size_of_val(o.as_slice())).sum::<usize>()
    }
}

/// A tile's track as it is being made: the triangles and sleepers by cell.
pub struct TrackBuilder {
    metres: f64,
    cell: f64,
    vertices: Vec<TrackVertex>,
    cells: std::collections::BTreeMap<(i32, i32), (Vec<u32>, Vec<Sleeper>)>,
    outlines: Vec<Vec<([[f32; 3]; 2], f32)>>,
}

/// The cross-section's faces: each from `(across, up)` to `(across, up)` (metres; across left of
/// the track's way), in its paint. Its outward normal follows from the order.
fn faces(gauge: f64) -> Vec<([f64; 2], [f64; 2], Rgba)> {
    let (top, toe, bed) = (BED_TOP_HALF_M, BED_TOE_HALF_M, BED_M);
    let mut out = vec![([toe, 0.0], [top, bed], BALLAST), ([top, bed], [-top, bed], BALLAST), ([-top, bed], [-toe, 0.0], BALLAST)];
    let half = gauge / 2.0 + RAIL_HEAD_M / 2.0;
    for c in [half, -half] {
        let (l, r) = (c + RAIL_HEAD_M / 2.0, c - RAIL_HEAD_M / 2.0);
        out.push(([l, SLEEPER_TOP_M], [l, RAIL_TOP_M], RAIL_SIDE));
        out.push(([l, RAIL_TOP_M], [r, RAIL_TOP_M], RAIL_HEAD));
        out.push(([r, RAIL_TOP_M], [r, SLEEPER_TOP_M], RAIL_SIDE));
    }
    out
}

/// A track's gauge, metres: its `gaugeint0` (mm) where it is a sensible one, else standard.
pub fn gauge_of(p: &crate::bridges::Properties) -> f64 {
    let mm = p.get("gaugeint0").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
    if (600.0..=1700.0).contains(&mm) { mm / 1000.0 } else { STANDARD_GAUGE_M }
}

/// `path` (tile units, metres up) cut to the tile's square: the pieces inside it.
pub fn clip_to_square(path: &[[f64; 3]]) -> Vec<Vec<[f64; 3]>> {
    let mut out: Vec<Vec<[f64; 3]>> = Vec::new();
    let mut current: Vec<[f64; 3]> = Vec::new();
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        // Liang–Barsky against 0 ≤ u, v ≤ 1
        let (du, dv) = (b[0] - a[0], b[1] - a[1]);
        let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
        let mut inside = true;
        for (p, q) in [(-du, a[0]), (du, 1.0 - a[0]), (-dv, a[1]), (dv, 1.0 - a[1])] {
            if p == 0.0 {
                if q < 0.0 {
                    inside = false;
                    break;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    t0 = t0.max(r);
                } else {
                    t1 = t1.min(r);
                }
            }
        }
        if !inside || t0 > t1 {
            if current.len() > 1 {
                out.push(std::mem::take(&mut current));
            }
            current.clear();
            continue;
        }
        let at = |t: f64| [a[0] + du * t, a[1] + dv * t, a[2] + (b[2] - a[2]) * t];
        let (p0, p1) = (at(t0), at(t1));
        if current.last() != Some(&p0) {
            if current.len() > 1 {
                out.push(std::mem::take(&mut current));
            }
            current.clear();
            current.push(p0);
        }
        if p1 != p0 {
            current.push(p1);
        }
        if t1 < 1.0 {
            if current.len() > 1 {
                out.push(std::mem::take(&mut current));
            }
            current.clear();
        }
    }
    if current.len() > 1 {
        out.push(current);
    }
    out
}

impl TrackBuilder {
    /// For a tile of `metres_per_unit`.
    pub fn new(metres_per_unit: f64) -> Self {
        TrackBuilder {
            metres: metres_per_unit,
            cell: CELL_M / metres_per_unit,
            vertices: Vec::new(),
            cells: std::collections::BTreeMap::new(),
            outlines: Vec::new(),
        }
    }

    fn cell_of(&self, [u, v]: [f64; 2]) -> (i32, i32) {
        ((u / self.cell).floor() as i32, (v / self.cell).floor() as i32)
    }

    /// One track along `path` (tile units, the third number its base in metres), `gauge` metres.
    pub fn push(&mut self, path: &[[f64; 3]], gauge: f64) {
        for piece in clip_to_square(path) {
            self.push_inside(&piece, gauge);
        }
    }

    fn push_inside(&mut self, path: &[[f64; 3]], gauge: f64) {
        let m = self.metres;
        // in metres east and north (from the path's first point), cut to short segments, repeated
        // points dropped
        let origin = DVec2::new(path[0][0], path[0][1]);
        let to_m = |p: [f64; 3]| DVec2::new((p[0] - origin.x) * m, -(p[1] - origin.y) * m);
        let mut pts: Vec<(DVec2, f64)> = Vec::with_capacity(path.len());
        for &p in path {
            let q = (to_m(p), p[2]);
            if let Some(&(last, lh)) = pts.last() {
                let d = q.0.distance(last);
                if d < 0.05 {
                    continue;
                }
                let n = (d / MAX_SEGMENT_M).ceil().max(1.0) as usize;
                for k in 1..n {
                    let t = k as f64 / n as f64;
                    pts.push((last.lerp(q.0, t), lh + (q.1 - lh) * t));
                }
            }
            pts.push(q);
        }
        if pts.len() < 2 {
            return;
        }
        let back = |e: DVec2| [origin.x + e.x / m, origin.y - e.y / m];
        // each point's way, and its left offset (a miter, no longer than twice the half widths)
        let n = pts.len();
        let dirs: Vec<DVec2> = (0..n - 1).map(|i| (pts[i + 1].0 - pts[i].0).normalize_or_zero()).collect();
        let lefts: Vec<DVec2> = (0..n)
            .map(|i| {
                let (a, b) = (dirs[i.saturating_sub(1)], dirs[(i).min(n - 2)]);
                let t = (a + b).normalize_or(b);
                let left = DVec2::new(-t.y, t.x);
                let seg_left = DVec2::new(-b.y, b.x);
                left / left.dot(seg_left).max(0.5)
            })
            .collect();
        // the mesh: per point each face's two vertices
        let faces = faces(gauge);
        let first = self.vertices.len() as u32;
        let per = (faces.len() * 2) as u32;
        let mut outline = Vec::with_capacity(n);
        for i in 0..n {
            let (p, base) = pts[i];
            let left = lefts[i];
            let way = dirs[i.min(n - 2)];
            let side_left = DVec2::new(-way.y, way.x);
            for &(a, b, paint) in &faces {
                // the face's outward normal: its direction (across, up) turned a quarter right
                let (da, dz) = (b[0] - a[0], b[1] - a[1]);
                let len = da.hypot(dz);
                let (na, nz) = (dz / len, -da / len);
                let normal = side_left * na;
                for c in [a, b] {
                    let at = back(p + left * c[0]);
                    self.vertices.push(TrackVertex {
                        position: [at[0] as f32, at[1] as f32, c[1] as f32],
                        normal: [normal.x as f32, normal.y as f32, nz as f32],
                        paint,
                        base: base as f32,
                    });
                }
            }
            let (l, r) = (back(p + left * BED_TOP_HALF_M), back(p - left * BED_TOP_HALF_M));
            let top = (base + BED_M) as f32;
            outline.push(([[l[0] as f32, l[1] as f32, top], [r[0] as f32, r[1] as f32, top]], base as f32));
        }
        self.outlines.push(outline);
        // a bed that ends here (not at the tile's edge, where the next tile's goes on) rounds off
        let on_edge = |p: DVec2| {
            let [u, v] = back(p);
            u <= 1e-6 || v <= 1e-6 || u >= 1.0 - 1e-6 || v >= 1.0 - 1e-6
        };
        for (end, out) in [(0, -dirs[0]), (n - 1, dirs[n - 2])] {
            let (p, base) = pts[end];
            if !on_edge(p) {
                self.bed_end(p, base, out, &back);
            }
        }
        // the triangles of each segment, in the cell of its middle
        for i in 0..n - 1 {
            let mid = back((pts[i].0 + pts[i + 1].0) / 2.0);
            let cell = self.cell_of(mid);
            let (a, b) = (first + i as u32 * per, first + (i as u32 + 1) * per);
            let tris = &mut self.cells.entry(cell).or_default().0;
            for f in 0..faces.len() as u32 {
                let (a0, a1, b0, b1) = (a + 2 * f, a + 2 * f + 1, b + 2 * f, b + 2 * f + 1);
                tris.extend([a0, b0, b1, a0, b1, a1]);
            }
        }
        // the sleepers, evenly along it
        let mut next = SLEEPER_SPACING_M / 2.0;
        let mut walked = 0.0;
        for i in 0..n - 1 {
            let ((p0, h0), (p1, h1)) = (pts[i], pts[i + 1]);
            let len = p0.distance(p1);
            let way = dirs[i];
            let dir = [(way.x / m) as f32, (-way.y / m) as f32];
            while next <= walked + len {
                let t = (next - walked) / len;
                let at = back(p0.lerp(p1, t));
                let cell = self.cell_of(at);
                let base = (h0 + (h1 - h0) * t) as f32;
                self.cells.entry(cell).or_default().1.push(Sleeper { at: [at[0] as f32, at[1] as f32], dir, base });
                next += SLEEPER_SPACING_M;
            }
            walked += len;
        }
    }

    /// The rounded end of a bed at `p` (metres, on `base`) facing `out`: a half cone from its top
    /// down to its toe, and the top's half disc.
    fn bed_end(&mut self, p: DVec2, base: f64, out: DVec2, back: &dyn Fn(DVec2) -> [f64; 2]) {
        const STEPS: usize = 8;
        let left = DVec2::new(-out.y, out.x);
        let (rise, run) = (BED_M, BED_TOE_HALF_M - BED_TOP_HALF_M);
        let len = rise.hypot(run);
        let first = self.vertices.len() as u32;
        let vertex = |q: DVec2, h: f64, normal: [f64; 3]| {
            let at = back(q);
            TrackVertex {
                position: [at[0] as f32, at[1] as f32, h as f32],
                normal: normal.map(|c| c as f32),
                paint: BALLAST,
                base: base as f32,
            }
        };
        self.vertices.push(vertex(p, BED_M, [0.0, 0.0, 1.0]));
        for k in 0..=STEPS {
            let a = std::f64::consts::PI * (k as f64 / STEPS as f64 - 0.5);
            let radial = out * a.cos() + left * a.sin();
            let slope = [radial.x * rise / len, radial.y * rise / len, run / len];
            self.vertices.push(vertex(p + radial * BED_TOP_HALF_M, BED_M, [0.0, 0.0, 1.0]));
            self.vertices.push(vertex(p + radial * BED_TOP_HALF_M, BED_M, slope));
            self.vertices.push(vertex(p + radial * BED_TOE_HALF_M, 0.0, slope));
        }
        let cell = self.cell_of(back(p));
        let tris = &mut self.cells.entry(cell).or_default().0;
        for k in 0..STEPS as u32 {
            let (a, b) = (first + 1 + 3 * k, first + 1 + 3 * (k + 1));
            tris.extend([first, a, b]);
            tris.extend([a + 1, b + 1, b + 2, a + 1, b + 2, a + 2]);
        }
    }

    pub fn finish(self) -> TrackMesh {
        let mut mesh = TrackMesh { vertices: self.vertices, metres_per_unit: self.metres, outlines: self.outlines, ..TrackMesh::default() };
        // a segment reaches at most half its length and the bed's half width out of its cell
        let reach = (MAX_SEGMENT_M / 2.0 + BED_TOE_HALF_M) as f32;
        let half = (self.cell / 2.0) as f32;
        let radius_m = (CELL_M as f32 / 2.0) * std::f32::consts::SQRT_2 + reach;
        for ((x, y), (tris, sleepers)) in self.cells {
            let (i0, s0) = (mesh.indices.len() as u32, mesh.sleepers.len() as u32);
            mesh.indices.extend(tris);
            mesh.sleepers.extend(sleepers);
            mesh.cells.push(TrackCell {
                centre: [x as f32 * 2.0 * half + half, y as f32 * 2.0 * half + half],
                radius_m,
                indices: i0..mesh.indices.len() as u32,
                sleepers: s0..mesh.sleepers.len() as u32,
            });
        }
        mesh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: f64 = 1000.0;

    #[test]
    fn a_line_is_cut_to_the_square_where_it_leaves_and_comes_back() {
        let pieces = clip_to_square(&[[-0.5, 0.5, 0.0], [0.5, 0.5, 2.0], [1.5, 0.5, 4.0]]);
        assert_eq!(pieces, vec![vec![[0.0, 0.5, 1.0], [0.5, 0.5, 2.0], [1.0, 0.5, 3.0]]]);
        let out_and_in = clip_to_square(&[[0.5, 0.5, 0.0], [0.5, -0.5, 0.0], [0.6, 0.5, 0.0]]);
        assert_eq!(out_and_in.len(), 2);
        assert!(clip_to_square(&[[2.0, 2.0, 0.0], [3.0, 2.0, 0.0]]).is_empty());
    }

    #[test]
    fn a_straight_track_is_a_bed_two_rails_and_sleepers_every_65_cm() {
        let mut b = TrackBuilder::new(M);
        // 100 m east along the middle of a tile 1 km across
        b.push(&[[0.45, 0.5, 0.0], [0.55, 0.5, 0.0]], STANDARD_GAUGE_M);
        let mesh = b.finish();
        assert_eq!(mesh.sleepers.len(), (100.0 / SLEEPER_SPACING_M).round() as usize);
        // every vertex lies within the bed's toe of the line, on the ground up to the rails' tops
        for v in &mesh.vertices {
            let across = (f64::from(v.position[1]) - 0.5).abs() * M;
            assert!(across <= BED_TOE_HALF_M + 1e-3, "{across}");
            assert!((0.0..=RAIL_TOP_M as f32 + 1e-6).contains(&v.position[2]));
            let n = v.normal;
            assert!(((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() - 1.0).abs() < 1e-5);
        }
        // the rails' heads stand at the rails' top, the gauge apart
        let heads: Vec<f64> = mesh.vertices.iter().filter(|v| v.paint == RAIL_HEAD).map(|v| (f64::from(v.position[1]) - 0.5) * M).collect();
        let inner = heads.iter().filter(|&&a| a > 0.0).fold(f64::INFINITY, |a, &b| a.min(b))
            + heads.iter().filter(|&&a| a < 0.0).fold(f64::INFINITY, |a, &b| a.min(-b));
        assert!((inner - STANDARD_GAUGE_M).abs() < 1e-3, "{inner}");
        // the bed's slopes face out and up, its top up; a sleeper lies across the way (east)
        assert!(mesh.vertices.iter().any(|v| v.paint == BALLAST && v.normal[1] > 0.3 && v.normal[2] > 0.8));
        assert!(mesh.vertices.iter().any(|v| v.paint == BALLAST && v.normal[2] == 1.0));
        assert!((mesh.sleepers[0].dir[0] as f64 * M - 1.0).abs() < 1e-6 && mesh.sleepers[0].dir[1] == 0.0);
        // all in one cell, every triangle and sleeper drawn once
        assert_eq!(mesh.cells.iter().map(|c| c.indices.len()).sum::<usize>(), mesh.indices.len());
        assert_eq!(mesh.cells.iter().map(|c| c.sleepers.len()).sum::<usize>(), mesh.sleepers.len());
    }

    #[test]
    fn a_bridge_s_track_stands_on_its_deck_and_a_long_line_spreads_over_cells() {
        let mut b = TrackBuilder::new(M);
        b.push(&[[0.1, 0.5, 0.0], [0.5, 0.5, 6.0], [0.9, 0.5, 0.0]], STANDARD_GAUGE_M);
        let mesh = b.finish();
        let top = mesh.vertices.iter().map(|v| v.base).fold(0.0, f32::max);
        assert!((top - 6.0).abs() < 1e-4);
        assert!(mesh.sleepers.iter().any(|s| s.base > 5.0));
        assert!(mesh.cells.len() >= 3);
    }

    #[test]
    fn stockholm_c_s_tile_stands_its_track_in_3d_within_a_few_megabytes() {
        let path = format!("{}/../map/tests/data/tiles/openrailwaymap/12/2253/1204.pbf", env!("CARGO_MANIFEST_DIR"));
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let tile = crate::tiles::TileIndex::new(2253, 1204, 12);
        let lines = crate::ground_rails::rails_tile(tile, &bytes, true, &|| false).unwrap();
        let t = &lines.track;
        // the station's tracks: hundreds of them, a sleeper every 65 cm
        assert!(t.outlines.len() > 300 && t.sleepers.len() > 100_000, "{} {}", t.outlines.len(), t.sleepers.len());
        assert!(t.byte_size() < 24 << 20, "{} bytes", t.byte_size());
        // every line cut to the tile's square (a vertex lies at most the bed's toe beyond it), every
        // bridge's at most a deck up
        let margin = (BED_TOE_HALF_M * 2.5 / t.metres_per_unit) as f32;
        let inside = |c: f32| (-margin..=1.0 + margin).contains(&c);
        assert!(t.vertices.iter().all(|v| inside(v.position[0]) && inside(v.position[1])));
        assert!(t.sleepers.iter().all(|s| (0.0..=1.0).contains(&s.at[0]) && (0.0..=1.0).contains(&s.at[1])));
        assert!(t.vertices.iter().any(|v| v.base > 1.0) && t.vertices.iter().all(|v| v.base <= crate::bridges::RAIL_DECK_M as f32 + 1e-3));
    }

    #[test]
    fn a_narrow_gauge_track_says_so_else_standard() {
        let p = |v: serde_json::Value| v.as_object().unwrap().clone();
        assert_eq!(gauge_of(&p(serde_json::json!({ "gaugeint0": 891 }))), 0.891);
        assert_eq!(gauge_of(&p(serde_json::json!({}))), STANDARD_GAUGE_M);
        assert_eq!(gauge_of(&p(serde_json::json!({ "gaugeint0": 5 }))), STANDARD_GAUGE_M);
    }
}
