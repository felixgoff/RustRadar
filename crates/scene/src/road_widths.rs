//! Port of `road-widths.ts` (S1.23; doc 04 §6.7): numbered Swedish roads as wide as they really
//! are, and gravel where they are gravel. Trafikverket's road database (NVDB), which the backend
//! asks for by square of the map with the Trafikverket key (`backend::roads`). Each of a ground
//! tile's roads is matched to the nearest of its stretches that runs the same way, and carries
//! its width (`nvdbWidth`, metres) and surface (`nvdbGravel`) as properties for the ground to
//! draw it by.
//!
//! The TypeScript keeps the stretches in module state; here they are a [`RoadWidths`] the caller
//! owns. Asking is the caller's too: [`RoadWidths::want`] names the squares to ask the backend
//! for, and [`RoadWidths::arrived`] / [`RoadWidths::failed`] take its answers.

use std::collections::{HashMap, HashSet};
use std::f64::consts::PI;
use std::sync::Arc;

use rustradar_core::model::RoadPiece;
use rustradar_core::num::clamp01;

use crate::landcover::{js_max, js_min};

/// The backend's squares, metres (`backend::roads`' `CELL_M`), and the Earth's radius as it has
/// it.
const CELL_M: f64 = 2000.0;
const EARTH_RADIUS: f64 = 6_371_008.8;
/// A road takes a stretch's width this near to it, metres, running within this many degrees of
/// its way.
const MATCH_M: f64 = 10.0;
const MATCH_DEG: f64 = 30.0;
/// The index's cells, degrees.
const INDEX_DEG: f64 = 0.002;

/// One of the backend's squares: its key (`"{i},{j}"` in the TypeScript) and its middle
/// (longitude, latitude), where the backend is asked for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub key: (i64, i64),
    pub centre: [f64; 2],
}

/// The square a point is in, as `backend::roads` has it, and its middle.
pub fn cell_of(lon: f64, lat: f64) -> Cell {
    let step_lat = (CELL_M / EARTH_RADIUS) * (180.0 / PI);
    let j = (lat / step_lat).floor();
    let mid_lat = (j + 0.5) * step_lat;
    let step_lon = step_lat / js_max(0.05, ((mid_lat * PI) / 180.0).cos());
    let i = (lon / step_lon).floor();
    Cell { key: (i as i64, j as i64), centre: [(i + 0.5) * step_lon, mid_lat] }
}

/// A stretch's segment, `a` to `b` (longitude, latitude), and the stretch (by its place in
/// [`RoadWidths::pieces`]).
#[derive(Debug, Clone, Copy)]
struct Segment {
    a: [f64; 2],
    b: [f64; 2],
    piece: usize,
}

/// The stretches known, indexed by cells of `INDEX_DEG` (0.002°), the squares asked for, and a version
/// bumped as stretches come in (the ground's tiles drawn again with them).
#[derive(Debug, Clone, Default)]
pub struct RoadWidths {
    pieces: Vec<RoadPiece>,
    index: HashMap<(i64, i64), Vec<Segment>>,
    asked: HashSet<(i64, i64)>,
    version: u64,
}

/// A binary tile's lines (loaders.gl's binary GeoJSON), as far as they are read here: in the
/// tile's own coordinates (0 to 1), y south.
#[derive(Debug, Clone, Copy)]
pub struct Lines<'a, P> {
    /// The vertices' coordinates, [`size`](Self::size) numbers a vertex.
    pub positions: &'a [f64],
    pub size: usize,
    /// Where each line's vertices start, and after the last where it ends.
    pub path_indices: &'a [u32],
    /// Each vertex's feature.
    pub feature_ids: &'a [u32],
    /// Each feature's properties.
    pub properties: &'a [P],
}

/// The two columns a tile's lines are given, a value a vertex: `nvdbWidth` (metres, 0 where no
/// stretch was found) and `nvdbGravel` (1 where the stretch is unpaved).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoadColumns {
    pub nvdb_width: Vec<f32>,
    pub nvdb_gravel: Vec<f32>,
}

/// What a tile's lines were last given, and at which version: kept beside the lines (the
/// TypeScript's `WeakMap` entry for them), so that they are given the same columns while no
/// more stretches have come.
pub type Surveyed = Option<(u64, Arc<RoadColumns>)>;

/// Metres from a point to a segment (near enough over a few metres), as east and north metres
/// at `cos`.
fn distance(p: [f64; 2], a: [f64; 2], b: [f64; 2], cos: f64) -> f64 {
    let k = (EARTH_RADIUS * PI) / 180.0;
    let (px, py) = ((p[0] - a[0]) * k * cos, (p[1] - a[1]) * k);
    let (bx, by) = ((b[0] - a[0]) * k * cos, (b[1] - a[1]) * k);
    let len2 = bx * bx + by * by;
    // `Math.max(0, Math.min(1, t))`
    let t = if len2 > 0.0 { clamp01((px * bx + py * by) / len2) } else { 0.0 };
    (px - bx * t).hypot(py - by * t)
}

/// The bearing from `a` to `b`, degrees.
fn bearing(a: [f64; 2], b: [f64; 2], cos: f64) -> f64 {
    (((b[0] - a[0]) * cos).atan2(b[1] - a[1]) * 180.0) / PI
}

/// A point of a tile (`x, y` its own coordinates) as longitude and latitude.
fn lon_lat(tile: (u32, u32), n: f64, x: f64, y: f64) -> [f64; 2] {
    [
        ((f64::from(tile.0) + x) / n) * 360.0 - 180.0,
        ((PI * (1.0 - (2.0 * (f64::from(tile.1) + y)) / n)).sinh().atan() * 180.0) / PI,
    ]
}

impl RoadWidths {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bumped as stretches come in: the ground's tiles drawn again with them (0 while none
    /// have).
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The stretches known.
    pub fn pieces(&self) -> &[RoadPiece] {
        &self.pieces
    }

    fn add(&mut self, piece: RoadPiece) {
        let at = self.pieces.len();
        for k in 1..piece.line.len() {
            let s = Segment { a: piece.line[k - 1], b: piece.line[k], piece: at };
            let (x0, x1) = (js_min(s.a[0], s.b[0]), js_max(s.a[0], s.b[0]));
            let (y0, y1) = (js_min(s.a[1], s.b[1]), js_max(s.a[1], s.b[1]));
            // (counted in doubles as the TypeScript does: a NaN end indexes nothing)
            let mut j = (y0 / INDEX_DEG).floor();
            while j <= (y1 / INDEX_DEG).floor() {
                let mut i = (x0 / INDEX_DEG).floor();
                while i <= (x1 / INDEX_DEG).floor() {
                    self.index.entry((i as i64, j as i64)).or_default().push(s);
                    i += 1.0;
                }
                j += 1.0;
            }
        }
        self.pieces.push(piece);
    }

    /// The squares to ask the backend for (once a square) round the view's centre, in Sweden:
    /// the square here and those within a kilometre or so. Each is taken as asked; tell
    /// [`arrived`](Self::arrived) or [`failed`](Self::failed) how it went.
    pub fn want(&mut self, lon: f64, lat: f64) -> Vec<Cell> {
        // (the TypeScript asks only in the app, which has the backend; this app always has it.
        // Written as it is there: a NaN centre is not out of Sweden)
        #[allow(clippy::manual_range_contains)]
        if lon < 10.5 || lon > 24.5 || lat < 55.0 || lat > 69.2 {
            return Vec::new();
        }
        let here = cell_of(lon, lat);
        let step_lat = (CELL_M / EARTH_RADIUS) * (180.0 / PI);
        let step_lon = step_lat / js_max(0.05, ((lat * PI) / 180.0).cos());
        let mut out = Vec::new();
        for (di, dj) in [(0.0, 0.0), (-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
            let cell = cell_of(lon + di * step_lon * 0.6, lat + dj * step_lat * 0.6);
            if self.asked.contains(&cell.key)
                || (cell.key != here.key
                    && ((cell.centre[0] - lon) / step_lon).hypot((cell.centre[1] - lat) / step_lat) > 1.1)
            {
                continue;
            }
            self.asked.insert(cell.key);
            out.push(cell);
        }
        out
    }

    /// A square's stretches, come from the backend; true when there were any, and the version
    /// was bumped (`onLoad`: the ground's tiles to be drawn again). A square without any is not
    /// asked for again.
    pub fn arrived(&mut self, pieces: Vec<RoadPiece>) -> bool {
        if pieces.is_empty() {
            return false;
        }
        for piece in pieces {
            self.add(piece);
        }
        self.version += 1;
        true
    }

    /// A square the backend could not answer for: asked again when the view comes back.
    pub fn failed(&mut self, key: (i64, i64)) {
        self.asked.remove(&key);
    }

    /// The stretch a road at `p` running `heading` (degrees, either way) is on, or none.
    pub fn stretch_at(&self, p: [f64; 2], heading: f64) -> Option<&RoadPiece> {
        let cos = ((p[1] * PI) / 180.0).cos();
        let mut best: Option<(f64, usize)> = None;
        let (ci, cj) = ((p[0] / INDEX_DEG).floor(), (p[1] / INDEX_DEG).floor());
        for dj in -1..=1 {
            for di in -1..=1 {
                let key = ((ci + f64::from(di)) as i64, (cj + f64::from(dj)) as i64);
                for s in self.index.get(&key).map_or(&[][..], Vec::as_slice) {
                    let d = distance(p, s.a, s.b, cos);
                    if d > MATCH_M || best.is_some_and(|(best, _)| d >= best) {
                        continue;
                    }
                    let turn = ((((bearing(s.a, s.b, cos) - heading) % 180.0) + 180.0).abs()) % 180.0;
                    if js_min(turn, 180.0 - turn) > MATCH_DEG {
                        continue;
                    }
                    best = Some((d, s.piece));
                }
            }
        }
        best.map(|(_, piece)| &self.pieces[piece])
    }

    /// A tile's lines (`x, y, z`; binary, in the tile's own coordinates, y south) given each
    /// road's stretch's width and surface as [`RoadColumns`], where `is_road` says it is one
    /// (a feature without properties is asked about `P::default()`) and a stretch is found at
    /// the middle of its middle segment. None (the lines as they were) while no stretches have
    /// come or there are no lines; the same columns again (`surveyed`, kept beside the lines)
    /// while no more have come.
    pub fn with_road_widths<P: Default>(
        &self,
        lines: &Lines<'_, P>,
        (x, y, z): (u32, u32, u32),
        is_road: impl Fn(&P) -> bool,
        surveyed: &mut Surveyed,
    ) -> Option<Arc<RoadColumns>> {
        if self.version == 0 || lines.path_indices.is_empty() {
            return None;
        }
        if let Some((version, out)) = surveyed
            && *version == self.version
        {
            return Some(Arc::clone(out));
        }
        let n = 2f64.powi(z as i32);
        let size = lines.size;
        let pos = |i: usize| lines.positions.get(i).copied().unwrap_or(f64::NAN);
        let count = lines.positions.len().checked_div(size).unwrap_or(0);
        let mut width = vec![0f32; count];
        let mut gravel = vec![0f32; count];
        let at = |i: usize| lon_lat((x, y), n, pos(i * size), pos(i * size + 1));
        let none = P::default();
        let paths = lines.path_indices;
        for p in 0..paths.len().saturating_sub(1) {
            let (start, end) = (paths[p] as usize, paths[p + 1] as usize);
            let properties =
                lines.feature_ids.get(start).and_then(|&id| lines.properties.get(id as usize)).unwrap_or(&none);
            if end < start + 2 || !is_road(properties) {
                continue;
            }
            // at its middle, the way it runs there
            let mid = start + (end - start - 1) / 2;
            let (a, b) = (at(mid), at(mid + 1));
            let cos = ((a[1] * PI) / 180.0).cos();
            let Some(piece) = self.stretch_at([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0], bearing(a, b, cos)) else {
                continue;
            };
            // `Float32Array.fill(value, start, end)`: the range cut to the column
            let range = start.min(count)..end.min(count);
            width[range.clone()].fill(piece.width);
            gravel[range].fill(if piece.paved { 0.0 } else { 1.0 });
        }
        let out = Arc::new(RoadColumns { nvdb_width: width, nvdb_gravel: gravel });
        *surveyed = Some((self.version, Arc::clone(&out)));
        Some(out)
    }

    /// For tests: the stretches known, as if they had come (`addRoadPiecesForTest`).
    #[cfg(test)]
    fn add_road_pieces_for_test(&mut self, pieces: Vec<RoadPiece>) {
        for piece in pieces {
            self.add(piece);
        }
        self.version += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;
    use serde_json::{Value, json};

    /// A road running east along 55.7°N near Lund, 9 m wide, and a gravel one north–south
    /// across it (the test file's module-level stretches).
    fn lund() -> RoadWidths {
        let mut widths = RoadWidths::new();
        widths.add_road_pieces_for_test(vec![
            RoadPiece { line: vec![[13.18, 55.7], [13.2, 55.7]], width: 9.0, paved: true },
            RoadPiece { line: vec![[13.19, 55.69], [13.19, 55.71]], width: 4.5, paved: false },
        ]);
        widths
    }

    #[test]
    fn a_road_takes_the_stretch_it_is_on_and_runs_along() {
        let widths = lund();
        assert_eq!(widths.stretch_at([13.185, 55.70003], 90.0).map(|p| p.width), Some(9.0));
        // either way along it
        assert_eq!(widths.stretch_at([13.185, 55.70003], 270.0).map(|p| p.width), Some(9.0));
        // across it, the crossing road's
        assert_eq!(widths.stretch_at([13.19, 55.6999], 0.0).map(|p| p.width), Some(4.5));
        // too far off, or the wrong way
        assert_eq!(widths.stretch_at([13.185, 55.7003], 90.0), None);
        assert_eq!(widths.stretch_at([13.185, 55.70003], 45.0), None);
    }

    #[test]
    fn a_tiles_roads_carry_their_stretches_widths_and_surfaces_its_paths_dont() {
        let widths = lund();
        // the zoom-16 tile round 13.185, 55.7, and a line along 55.7° through it, and one of a path
        let z = 16;
        let n = 2f64.powi(z);
        let tx = ((13.185 + 180.0) / 360.0) * n;
        let r = (55.7 * PI) / 180.0;
        let ty = ((1.0 - (r.tan() + 1.0 / r.cos()).ln() / PI) / 2.0) * n;
        let tile = (tx.floor() as u32, ty.floor() as u32, z as u32);
        let (x, y) = (tx - tx.floor(), ty - ty.floor());
        let positions = [x - 0.2, y, x + 0.2, y, x - 0.2, y, x + 0.2, y];
        let properties = [json!({"road": true}), json!({"road": false})];
        let lines = Lines { positions: &positions, size: 2, path_indices: &[0, 2, 4], feature_ids: &[0, 0, 1, 1], properties: &properties };
        let is_road = |p: &Value| p.get("road") == Some(&Value::Bool(true));
        let mut surveyed = None;
        let out = widths.with_road_widths(&lines, tile, is_road, &mut surveyed).unwrap();
        assert_eq!(out.nvdb_width, [9.0, 9.0, 0.0, 0.0]);
        assert_eq!(out.nvdb_gravel, [0.0, 0.0, 0.0, 0.0]);
        // the same object again, while no more have come
        assert!(Arc::ptr_eq(&widths.with_road_widths(&lines, tile, is_road, &mut surveyed).unwrap(), &out));
    }

    #[test]
    fn the_backends_squares_the_same_as_roadsrss() {
        let Cell { key, centre } = cell_of(13.19, 55.705);
        assert_eq!(cell_of(centre[0], centre[1]).key, key);
    }

    // New tests (no TypeScript twin).

    #[test]
    fn squares_as_the_typescript_has_them() {
        // the TypeScript's keys and middles for these (a middle to 1e-12°: `cos` may differ in
        // its last bit between V8 and libm)
        let lund = cell_of(13.19, 55.705);
        assert_eq!(lund.key, (413, 3097));
        assert_close!(lund.centre[0], 13.202290287220219, 12);
        assert_close!(lund.centre[1], 55.71289653273513, 12);
        let santiago = cell_of(-70.5, -33.4);
        assert_eq!(santiago.key, (-3273, -1857));
        assert_close!(santiago.centre[0], -70.49780364699986, 12);
        // near a pole a square is at most 20 times as wide as it is high
        let pole = cell_of(10.0, 89.99);
        assert_eq!(pole.key, (27, 5003));
        assert_close!(pole.centre[0], 9.892524000969917, 12);
    }

    #[test]
    fn the_squares_round_the_view_asked_for_once() {
        let mut widths = RoadWidths::new();
        assert!(widths.want(10.0, 59.0).is_empty());
        assert!(widths.want(13.0, 54.9).is_empty());
        let first = widths.want(13.19, 55.705);
        assert!(!first.is_empty() && first.len() <= 5);
        assert_eq!(first[0], cell_of(13.19, 55.705));
        let keys: HashSet<_> = first.iter().map(|c| c.key).collect();
        assert_eq!(keys.len(), first.len());
        // asked already
        assert!(widths.want(13.19, 55.705).is_empty());
        // a failure is asked again; an empty answer is not, and bumps nothing
        widths.failed(first[0].key);
        assert_eq!(widths.want(13.19, 55.705), [first[0]]);
        assert!(!widths.arrived(Vec::new()));
        assert_eq!(widths.version(), 0);
        assert!(widths.arrived(vec![RoadPiece { line: vec![[13.18, 55.7], [13.2, 55.7]], width: 7.0, paved: false }]));
        assert_eq!(widths.version(), 1);
        assert_eq!(widths.pieces().len(), 1);
    }

    #[test]
    fn lines_as_they_were_while_no_stretches_have_come_and_new_columns_when_more_do() {
        let mut widths = RoadWidths::new();
        let positions = [0.4, 0.5, 0.6, 0.5];
        let properties: [Value; 0] = [];
        let lines = Lines { positions: &positions, size: 2, path_indices: &[0, 2], feature_ids: &[0, 0], properties: &properties };
        let mut surveyed = None;
        let tile = (35_196, 20_509, 16);
        assert_eq!(widths.with_road_widths(&lines, tile, |_| true, &mut surveyed), None);
        widths.arrived(vec![RoadPiece { line: vec![[0.0, 0.0], [0.001, 0.0]], width: 7.0, paved: false }]);
        let none = Lines { path_indices: &[], ..lines };
        assert_eq!(widths.with_road_widths(&none, tile, |_| true, &mut surveyed), None);
        // a feature without properties is asked about the default (here JSON's null)
        let first = widths.with_road_widths(&lines, tile, |p| p.is_null(), &mut surveyed).unwrap();
        assert_eq!(*first, RoadColumns { nvdb_width: vec![0.0, 0.0], nvdb_gravel: vec![0.0, 0.0] });
        widths.arrived(vec![RoadPiece { line: vec![[1.0, 0.0], [1.001, 0.0]], width: 7.0, paved: false }]);
        let second = widths.with_road_widths(&lines, tile, |p| p.is_null(), &mut surveyed).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(surveyed.as_ref().map(|(v, _)| *v), Some(2));
    }

    #[test]
    fn a_gravel_road_and_the_nearer_of_two_stretches() {
        let mut widths = lund();
        widths.add_road_pieces_for_test(vec![RoadPiece { line: vec![[13.18, 55.70005], [13.2, 55.70005]], width: 12.0, paved: true }]);
        // 3.3 m from the first, 2.2 m from the third
        assert_eq!(widths.stretch_at([13.185, 55.70003], 90.0).map(|p| p.width), Some(12.0));
        assert_eq!(widths.stretch_at([13.185, 55.700015], 90.0).map(|p| p.width), Some(9.0));
        // the gravel road, a line north-south along it in its tile
        let z = 16u32;
        let n = 65536.0;
        let tx = ((13.19 + 180.0) / 360.0) * n;
        let r = (55.705f64 * PI) / 180.0;
        let ty = ((1.0 - (r.tan() + 1.0 / r.cos()).ln() / PI) / 2.0) * n;
        let (x, y) = (tx - tx.floor(), ty - ty.floor());
        let positions = [x, y - 0.1, x, y, x, y + 0.1];
        let properties = [true];
        let lines = Lines { positions: &positions, size: 2, path_indices: &[0, 3], feature_ids: &[0, 0, 0], properties: &properties };
        let out = widths.with_road_widths(&lines, (tx.floor() as u32, ty.floor() as u32, z), |&road| road, &mut None).unwrap();
        assert_eq!((out.nvdb_width.as_slice(), out.nvdb_gravel.as_slice()), (&[4.5; 3][..], &[1.0; 3][..]));
    }
}
