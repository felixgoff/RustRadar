//! Path segment records (plan 1.4.6, item E6): what deck.gl's PathLayer tessellator
//! (`@deck.gl/layers/dist/path-layer/path-tesselator.js`) and PathStyleExtension's whole-path dash
//! metrics (`@deck.gl/extensions/dist/path-style/path-style-extension.js`, `calculateDashMetrics`)
//! work out on the CPU, as the instances of `rustradar_map::pipelines::path`.
//!
//! deck draws a path as one instance per vertex: instance `i` reads vertices `i − 1`, `i`, `i + 1`
//! and `i + 2` (left, start, end, right) and its vertex shader extrudes the segment from start to
//! end with the joints at both ends. A [`PathSegment`] is one such instance with its four positions
//! spelled out, in the local Mercator of the mesh ([`LocalMercator`], the frame `rustradar_map`
//! draws it in), so one mesh serves the globe and the flat map.
//!
//! deck's rules, kept here ([`PathBuilder::push_parts`]):
//! - A path of fewer than two points draws nothing.
//! - With deck's default normalisation ([`PathType::Auto`]) a path whose last point equals its first
//!   is closed: it takes two more vertex slots (its second and third points again), and every
//!   segment is joined at both ends, with no caps. A closed path of fewer than three points draws
//!   nothing. [`PathType::Open`] (`_pathType: 'open'`) never closes.
//! - An open path's first segment has a start cap and its last an end cap ([`START_CAP`],
//!   [`END_CAP`]). deck's padding slots (marked invalid, drawn as nothing) are left out.
//! - A row (one object of deck's `data`) may hold several parts (a cut path); its vertex slots and
//!   its dash metrics run on across them.
//! - Coordinates read as deck flattens nested paths: a missing or NaN number is 0 (`|| 0`).
//! - A per-vertex colour list fills the row's vertex slots in order, as deck writes an attribute
//!   whose accessor returns one value per vertex: a closed path's extra slots take the next colours
//!   of the list, and a slot past its end gets transparent black.
//!
//! Dash metrics (only with [`PathBuilder::measuring`], for whole-path dashes): each segment gets
//! the distance from its row's start to its own start and the row's length, as deck's
//! `instanceDashOffsets`. They are measured in the common space the shader measures dashes in,
//! once for each projection: the globe's (a sphere of 256 units, heights included) and the flat
//! map's (world Mercator, heights in units at the point's latitude, as `lib/project.wgsl` lifts
//! them). A closed first part starts measuring at its first point, as deck does.

use bytemuck::{Pod, Zeroable};
use glam::{DVec2, DVec3};

use crate::camera::{GLOBE_EARTH_RADIUS_M, GLOBE_RADIUS, WORLD_SIZE, mercator_units_per_meter, world_to_lng_lat};
use crate::tiles::TileIndex;

/// [`PathSegment::types`]: the segment starts the path (a start cap).
pub const START_CAP: u32 = 1;
/// [`PathSegment::types`]: the segment ends the path (an end cap).
pub const END_CAP: u32 = 2;
/// deck's padding slot, never made into a segment.
const INVALID: u32 = 4;

/// One instance of the `path` pipeline: a segment from `start` to `end` with its neighbours, in
/// local Mercator `(u east, v south, metres up)`. 84 bytes, `#[repr(C)]`, the pipeline's instance
/// buffer layout.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct PathSegment {
    /// The vertex before `start` (`start` itself for a start cap).
    pub left: [f32; 3],
    pub start: [f32; 3],
    pub end: [f32; 3],
    /// The vertex after `end` (`end` itself for an end cap).
    pub right: [f32; 3],
    /// deck's `getWidth`, in the draw's width units.
    pub width: f32,
    /// deck's `getColor`, RGBA 0–255, gamma space.
    pub color: [u8; 4],
    /// deck's `getDashArray`: solid and gap, in half widths of the line; `[0, 0]` is solid.
    pub dash_array: [f32; 2],
    /// Globe: the row's length up to this segment's start, and the whole row's length, in common
    /// units (only from [`PathBuilder::measuring`]).
    pub dash_globe: [f32; 2],
    /// The same on the flat map, in world Mercator units.
    pub dash_flat: [f32; 2],
    /// [`START_CAP`] and [`END_CAP`] bits.
    pub types: u32,
}

/// Where a mesh's local Mercator sits: vertex `(u, v)` is world Mercator `anchor + (u, −v) · scale`
/// (512 units round the world, y north), `v` growing south. `rustradar_map::frame::ViewFrame::local`
/// takes the same anchor and scale (a power of two, `u` best within `[0, 1]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalMercator {
    /// The world Mercator point of `(0, 0)`.
    pub anchor: DVec2,
    /// World units per local unit.
    pub scale: f64,
}

impl LocalMercator {
    pub fn new(anchor: DVec2, scale: f64) -> Self {
        LocalMercator { anchor, scale }
    }

    /// A tile's units: `(0, 0)` its north-west corner, `(1, 1)` its south-east corner
    /// (`ViewFrame::tile`).
    pub fn tile(tile: TileIndex) -> Self {
        let extent = WORLD_SIZE / 2f64.powi(tile.z as i32);
        LocalMercator { anchor: DVec2::new(f64::from(tile.x) * extent, WORLD_SIZE - f64::from(tile.y) * extent), scale: extent }
    }

    /// The world Mercator point of local `(u, v)`.
    pub fn to_world(&self, [u, v]: [f64; 2]) -> DVec2 {
        self.anchor + DVec2::new(u, -v) * self.scale
    }

    /// The local `(u, v)` of a world Mercator point.
    pub fn from_world(&self, world: DVec2) -> [f64; 2] {
        let d = (world - self.anchor) / self.scale;
        [d.x, -d.y]
    }
}

/// How a row's ends are read (deck's `_pathType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PathType {
    /// deck's default normalisation: a part whose last point equals its first is a closed loop.
    #[default]
    Auto,
    /// `_pathType: 'open'`: always capped at both ends.
    Open,
}

/// A row's colour (deck's `getColor`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Colors<'a> {
    /// One colour for the whole row.
    Same([u8; 4]),
    /// One per vertex slot, in order (see the module notes).
    PerVertex(&'a [[u8; 4]]),
}

/// What a row is drawn with: deck's per-object accessors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paint<'a> {
    pub color: Colors<'a>,
    /// `getWidth` (default 1), in the draw's width units.
    pub width: f64,
    /// `getDashArray` (default `[0, 0]`, solid): solid and gap in half widths.
    pub dash_array: [f64; 2],
    pub path_type: PathType,
}

impl Paint<'static> {
    /// A solid line of one colour and width (deck's other defaults).
    pub fn solid(color: [u8; 4], width: f64) -> Self {
        Paint { color: Colors::Same(color), width, dash_array: [0.0, 0.0], path_type: PathType::Auto }
    }
}

impl<'a> Paint<'a> {
    /// The same paint dashed with `dash_array` (solid, gap in half widths).
    pub fn dashed(self, dash_array: [f64; 2]) -> Self {
        Paint { dash_array, ..self }
    }
}

/// Makes the [`PathSegment`]s of a mesh, row by row.
#[derive(Debug, Clone)]
pub struct PathBuilder {
    local: LocalMercator,
    measure: bool,
    segments: Vec<PathSegment>,
}

impl PathBuilder {
    /// Segments in `local`, without dash metrics (solid lines and per-segment dashes).
    pub fn new(local: LocalMercator) -> Self {
        PathBuilder { local, measure: false, segments: Vec::new() }
    }

    /// Segments in `local` with the whole-path dash metrics (the pipeline's `Dash::Path`, deck's
    /// `highPrecisionDash`).
    pub fn measuring(local: LocalMercator) -> Self {
        PathBuilder { local, measure: true, segments: Vec::new() }
    }

    /// The frame the segments are in.
    pub fn local(&self) -> LocalMercator {
        self.local
    }

    /// One row of one part: `path` in local Mercator `[u, v, metres]`.
    pub fn push(&mut self, path: &[[f64; 3]], paint: &Paint<'_>) {
        self.push_parts(&[path], paint);
    }

    /// One row of several parts (a cut path), in local Mercator `[u, v, metres]`.
    pub fn push_parts(&mut self, parts: &[&[[f64; 3]]], paint: &Paint<'_>) {
        // the row's vertex slots (deck's normalised positions) and their types
        let mut slots: Vec<[f64; 3]> = Vec::new();
        let mut types: Vec<u32> = Vec::new();
        for part in parts {
            let points: Vec<[f64; 3]> = part.iter().map(|p| p.map(or0)).collect();
            let n = points.len();
            let closed = paint.path_type == PathType::Auto && n >= 2 && points[0] == points[n - 1];
            let size = match (n, closed) {
                (0 | 1, _) => 0,
                (_, true) if n < 3 => 0,
                (_, true) => n + 2,
                (_, false) => n,
            };
            if size == 0 {
                continue;
            }
            let first = slots.len();
            // getPointOnPath: past the end it goes round again from the second point
            slots.extend((0..size).map(|k| points[if k < n { k } else { k + 1 - n }]));
            types.resize(first + size, 0);
            if closed {
                types[first] = INVALID;
                types[first + size - 2] = INVALID;
            } else {
                types[first] += START_CAP;
                types[first + size - 2] += END_CAP;
            }
            types[first + size - 1] = INVALID;
        }
        let row_first = self.segments.len();
        // getPathSegmentIndices: every valid slot but the row's last, in order
        let mut order = Vec::new();
        for (k, &kind) in types.iter().enumerate().take(slots.len().saturating_sub(1)) {
            if kind & INVALID != 0 {
                continue;
            }
            let at = |i: usize| slots[i].map(|c| c as f32);
            let cap_start = kind & START_CAP != 0;
            let cap_end = kind & END_CAP != 0;
            let left = if k == 0 || cap_start { at(k) } else { at(k - 1) };
            let right = if k + 2 >= slots.len() || cap_end { at(k + 1) } else { at(k + 2) };
            let color = match paint.color {
                Colors::Same(c) => c,
                Colors::PerVertex(list) => list.get(k).copied().unwrap_or([0; 4]),
            };
            order.push((k, self.segments.len()));
            self.segments.push(PathSegment {
                left,
                start: at(k),
                end: at(k + 1),
                right,
                width: paint.width as f32,
                color,
                dash_array: paint.dash_array.map(|d| d as f32),
                dash_globe: [0.0; 2],
                dash_flat: [0.0; 2],
                types: kind & (START_CAP | END_CAP),
            });
        }
        if !self.measure || order.is_empty() {
            return;
        }
        // a closed first part is measured from its first point: visit A→B first
        if types[0] & INVALID != 0 {
            order.rotate_right(1);
        }
        let projected: Vec<(DVec3, DVec3)> = slots.iter().map(|&p| self.common(p)).collect();
        let (mut globe, mut flat) = (0.0, 0.0);
        for &(k, s) in &order {
            self.segments[s].dash_globe[0] = globe as f32;
            self.segments[s].dash_flat[0] = flat as f32;
            globe += projected[k].0.distance(projected[k + 1].0);
            flat += projected[k].1.distance(projected[k + 1].1);
        }
        for segment in &mut self.segments[row_first..] {
            segment.dash_globe[1] = globe as f32;
            segment.dash_flat[1] = flat as f32;
        }
    }

    /// The segments so far.
    pub fn segments(&self) -> &[PathSegment] {
        &self.segments
    }

    /// The segments.
    pub fn finish(self) -> Vec<PathSegment> {
        self.segments
    }

    /// A local point in the globe's common space and in the flat map's (world Mercator, height in
    /// units at its latitude).
    fn common(&self, [u, v, h]: [f64; 3]) -> (DVec3, DVec3) {
        let world = self.local.to_world([u, v]);
        let [lon, lat] = world_to_lng_lat(world.to_array());
        (globe_common([lon, lat, h]), DVec3::new(world.x, world.y, h * mercator_units_per_meter(lat)))
    }
}

/// deck's `GlobeViewport.projectPosition`: `[lon, lat, metres]` on the sphere of 256 units.
fn globe_common([lon, lat, h]: [f64; 3]) -> DVec3 {
    let (lambda, phi) = (lon.to_radians(), lat.to_radians());
    let d = (h / GLOBE_EARTH_RADIUS_M + 1.0) * GLOBE_RADIUS;
    DVec3::new(lambda.sin() * phi.cos() * d, -lambda.cos() * phi.cos() * d, phi.sin() * d)
}

/// `x || 0`.
fn or0(x: f64) -> f64 {
    if x == 0.0 || x.is_nan() { 0.0 } else { x }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::lng_lat_to_world;

    const RED: [u8; 4] = [255, 0, 0, 255];

    fn unit() -> LocalMercator {
        LocalMercator::new(DVec2::new(256.0, 256.0), 1.0)
    }

    fn ends(s: &PathSegment) -> ([f32; 2], [f32; 2], [f32; 2], [f32; 2]) {
        let xy = |p: [f32; 3]| [p[0], p[1]];
        (xy(s.left), xy(s.start), xy(s.end), xy(s.right))
    }

    #[test]
    fn an_open_path_is_capped_at_both_ends_and_joined_between() {
        let mut b = PathBuilder::new(unit());
        b.push(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [2.0, 1.0, 5.0]], &Paint::solid(RED, 3.0));
        let s = b.finish();
        assert_eq!(s.len(), 3);
        assert_eq!(ends(&s[0]), ([0.0, 0.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]));
        assert_eq!(ends(&s[1]), ([0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [2.0, 1.0]));
        assert_eq!(ends(&s[2]), ([1.0, 0.0], [1.0, 1.0], [2.0, 1.0], [2.0, 1.0]));
        assert_eq!(s.iter().map(|s| s.types).collect::<Vec<_>>(), [START_CAP, 0, END_CAP]);
        assert_eq!(s[2].end, [2.0, 1.0, 5.0]);
        assert!(s.iter().all(|s| s.width == 3.0 && s.color == RED && s.dash_array == [0.0, 0.0]));
        // two points: one segment with both caps
        let mut b = PathBuilder::new(unit());
        b.push(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], &Paint::solid(RED, 1.0));
        assert_eq!(b.segments()[0].types, START_CAP | END_CAP);
    }

    #[test]
    fn a_path_whose_last_point_repeats_its_first_is_a_joined_loop() {
        let square = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]];
        let mut b = PathBuilder::new(unit());
        b.push(&square, &Paint::solid(RED, 1.0));
        let s = b.finish();
        // slots A B C D A B C: segments B→C, C→D, D→A, A→B, no caps
        assert_eq!(s.len(), 4);
        assert!(s.iter().all(|s| s.types == 0));
        assert_eq!(ends(&s[0]), ([0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]));
        assert_eq!(ends(&s[3]), ([0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]));
        // open: the same points capped
        let mut b = PathBuilder::new(unit());
        b.push(&square, &Paint { path_type: PathType::Open, ..Paint::solid(RED, 1.0) });
        let s = b.finish();
        assert_eq!(s.len(), 4);
        assert_eq!((s[0].types, s[3].types), (START_CAP, END_CAP));
    }

    #[test]
    fn too_short_paths_draw_nothing() {
        let mut b = PathBuilder::measuring(unit());
        b.push(&[], &Paint::solid(RED, 1.0));
        b.push(&[[0.0, 0.0, 0.0]], &Paint::solid(RED, 1.0));
        // closed with two points
        b.push(&[[0.5, 0.5, 0.0], [0.5, 0.5, 0.0]], &Paint::solid(RED, 1.0));
        assert!(b.segments().is_empty());
        // a degenerate open segment is still one segment (deck caps it on both sides)
        b.push(&[[0.5, 0.5, 0.0], [0.5, 0.5, 1.0]], &Paint::solid(RED, 1.0));
        assert_eq!(b.segments().len(), 1);
    }

    #[test]
    fn coordinates_read_nan_as_zero_and_closing_compares_them_cleaned() {
        let mut b = PathBuilder::new(unit());
        b.push(&[[0.0, 0.0, f64::NAN], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 0.0, 0.0]], &Paint::solid(RED, 1.0));
        let s = b.finish();
        assert_eq!(s.len(), 3);
        assert!(s.iter().all(|s| s.types == 0), "closed: NaN height reads as 0");
        // B→C, C→A, A→B
        assert_eq!(s[1].end, [0.0, 0.0, 0.0]);
        assert_eq!(s[2].start, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn per_vertex_colours_fill_the_vertex_slots_in_order() {
        let colors = [[1, 0, 0, 255], [2, 0, 0, 255], [3, 0, 0, 255], [4, 0, 0, 255]];
        let paint = Paint { color: Colors::PerVertex(&colors), ..Paint::solid(RED, 1.0) };
        let mut b = PathBuilder::new(unit());
        b.push(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]], &paint);
        assert_eq!(b.segments().iter().map(|s| s.color[0]).collect::<Vec<_>>(), [1, 2]);
        // closed A B C A: slots A B C A B C, segments at slots 1, 2, 3; slot 4 and on are past the list
        let mut b = PathBuilder::new(unit());
        b.push(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 0.0, 0.0]], &paint);
        assert_eq!(b.segments().iter().map(|s| s.color[0]).collect::<Vec<_>>(), [2, 3, 4]);
        let mut b = PathBuilder::new(unit());
        b.push(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [2.0, 1.0, 0.0], [3.0, 1.0, 0.0], [4.0, 1.0, 0.0]], &paint);
        assert_eq!(b.segments()[4].color, [0; 4]);
        // parts run on: the second part's first segment is at slot 2
        let mut b = PathBuilder::new(unit());
        b.push_parts(&[&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], &[[0.0, 1.0, 0.0], [1.0, 1.0, 0.0]]], &paint);
        let s = b.finish();
        assert_eq!(s.iter().map(|s| s.color[0]).collect::<Vec<_>>(), [1, 3]);
        assert!(s.iter().all(|s| s.types == START_CAP | END_CAP));
    }

    #[test]
    fn whole_path_dash_metrics_run_along_the_row_in_both_projections() {
        // three points along the equator, 1 and 2 degrees apart, at sea level
        let local = LocalMercator::new(DVec2::from_array(lng_lat_to_world([0.0, 0.0])), 1.0);
        let at = |lon: f64| {
            let [u, v] = local.from_world(DVec2::from_array(lng_lat_to_world([lon, 0.0])));
            [u, v, 0.0]
        };
        let path = [at(0.0), at(1.0), at(3.0)];
        let mut b = PathBuilder::measuring(local);
        b.push(&path, &Paint::solid(RED, 2.0).dashed([4.0, 3.0]));
        let s = b.finish();
        let chord = |deg: f64| 2.0 * GLOBE_RADIUS * (deg.to_radians() / 2.0).sin();
        let world = |deg: f64| WORLD_SIZE * deg / 360.0;
        let close = |a: f32, b: f64| ((f64::from(a) - b) / b.max(1e-9)).abs() < 1e-6;
        assert_eq!(s[0].dash_globe[0], 0.0);
        assert!(close(s[1].dash_globe[0], chord(1.0)), "{:?}", s[1].dash_globe);
        assert!(close(s[0].dash_globe[1], chord(1.0) + chord(2.0)));
        assert_eq!(s[0].dash_globe[1], s[1].dash_globe[1]);
        assert!(close(s[1].dash_flat[0], world(1.0)));
        assert!(close(s[1].dash_flat[1], world(3.0)));
        assert_eq!(s[0].dash_array, [4.0, 3.0]);
        // heights count: on the globe the sphere grows, on the flat map units at the latitude
        let mut b = PathBuilder::measuring(local);
        b.push(&[at(0.0), [at(0.0)[0], at(0.0)[1], 1000.0]], &Paint::solid(RED, 1.0));
        let s = b.finish();
        assert!(close(s[0].dash_globe[1], 1000.0 * GLOBE_RADIUS / GLOBE_EARTH_RADIUS_M));
        assert!(close(s[0].dash_flat[1], 1000.0 * mercator_units_per_meter(0.0)));
        // without measuring, nothing is measured
        let mut b = PathBuilder::new(local);
        b.push(&path, &Paint::solid(RED, 2.0));
        assert!(b.segments().iter().all(|s| s.dash_globe == [0.0; 2] && s.dash_flat == [0.0; 2]));
    }

    #[test]
    fn a_closed_path_is_measured_from_its_first_point() {
        let local = LocalMercator::new(DVec2::new(0.0, 512.0), 1.0);
        let mut b = PathBuilder::measuring(local);
        // A(0,0) B(3,0) C(3,4) A: |AB| = 3, |BC| = 4, |CA| = 5 world units on the flat map
        b.push(&[[0.0, 0.0, 0.0], [3.0, 0.0, 0.0], [3.0, 4.0, 0.0], [0.0, 0.0, 0.0]], &Paint::solid(RED, 1.0));
        let s = b.finish();
        // segments in slot order B→C, C→A, A→B
        assert_eq!(s.iter().map(|s| s.dash_flat[0]).collect::<Vec<_>>(), [3.0, 7.0, 0.0]);
        assert!(s.iter().all(|s| s.dash_flat[1] == 12.0));
        // rows are measured each on their own
        let mut b = PathBuilder::measuring(local);
        b.push(&[[0.0, 0.0, 0.0], [3.0, 0.0, 0.0]], &Paint::solid(RED, 1.0));
        b.push(&[[0.0, 1.0, 0.0], [0.0, 2.0, 0.0], [0.0, 4.0, 0.0]], &Paint::solid(RED, 1.0));
        let s = b.finish();
        assert_eq!(s.iter().map(|s| s.dash_flat).collect::<Vec<_>>(), [[0.0, 3.0], [0.0, 3.0], [1.0, 3.0]]);
    }

    #[test]
    fn tile_frames_put_the_north_west_corner_at_the_origin() {
        let local = LocalMercator::tile(TileIndex { x: 3, y: 1, z: 2 });
        assert_eq!(local.scale, 128.0);
        assert_eq!(local.to_world([0.0, 0.0]), DVec2::new(384.0, 384.0));
        assert_eq!(local.to_world([1.0, 1.0]), DVec2::new(512.0, 256.0));
        assert_eq!(local.from_world(DVec2::new(448.0, 320.0)), [0.5, 0.5]);
        assert_eq!(size_of::<PathSegment>(), 84);
    }
}
