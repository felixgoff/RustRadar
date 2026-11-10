//! Vector map tessellation (item B1; doc 01 §5.6): one OpenFreeMap tile (OpenMapTiles schema)
//! becomes the fills and path segments the dark basemap draws, in today's palette and widths, as
//! `Globe.svelte` (G:492-541, 587-608) has deck.gl 9.4's MVTLayer draw them, ready for
//! `rustradar_map`'s `fill` and `path` pipelines in the tile's frame (`ViewFrame::tile`, `(u, v)`
//! with `(0, 0)` the north-west corner and `(1, 1)` the south-east one; the buffer round the tile
//! lies a little outside).
//!
//! What today's app does, and so this module ([`tessellate`]):
//! - Only the source layers `water`, `boundary`, `transportation` and `aeroway` are decoded
//!   ([`SOURCE_LAYERS`]), **in that order**: loaders.gl 4.5's `parseMVT` walks its `layers`
//!   option in the order given, whatever order the tile stores its layers in (doc 01 §5.6 says
//!   lines follow the tile's layer order; loaders.gl's code says otherwise). For each name the
//!   last non-empty layer of that name counts (`rustradar_core::mvt::layers_named`).
//! - Polygons are filled ([`vector_fill`]) and never outlined (`stroked: false`); lines are drawn
//!   ([`vector_line`], [`vector_line_width`]: screen pixels, deck's PathLayer defaults of butt
//!   caps, miter joints, no dashes); points are not drawn (`getPointRadius: 0`).
//! - deck's GeoJsonLayer draws all of a tile's fills, then all of its lines, each in feature
//!   order. [`VectorTile::fill_indices`] and [`VectorTile::lines`] keep that order: draw a
//!   tile's fills, then its lines, with `SURFACE_NO_CULL` (doc 01 §5.6).
//! - Polygon rings are grouped as loaders.gl's `classifyRings` groups them ([`classify_rings`]).
//!   On the globe each polygon is then what deck's `Polygon.normalize` makes of it (each ring
//!   closed, the outline clockwise and the holes counter-clockwise with north up), cut on the
//!   grid (below) and each part cut into triangles by earcut 2.2.4, deck's own, ported in this
//!   file (the `earcut` module says why not `crate::geom::earcut`): the globe gets deck's very
//!   triangles. On the flat map each ring is closed as loaders.gl's binary tiles hold it and the
//!   polygon triangulated in tile units.
//! - The tile's numbers are tile units over the layer's `extent`, so a tile above zoom 14 (the
//!   source's last, [`MAX_ZOOM`]) is drawn from its z14 ancestor: the tile selection
//!   (`crate::tiles::VECTOR_TILES`) asks for no deeper tile, and this module's output does not
//!   depend on the view's zoom.
//!
//! **Both projections from one mesh.** deck decodes each tile once per projection (layer ids
//! `map-globe` and `map-flat`): on the globe as GeoJSON in longitude and latitude, every polygon
//! and path cut on the 5° grid of `GlobeView({ resolution: 5 })` (math.gl's `cutPolygonByGrid`
//! with edge types, `cutPolylineByGrid`), a path whose last point repeats its first drawn as a
//! joined loop and repeated points dropped by the cut; on the flat map as binary tiles, nothing
//! cut, every path open (`_pathType: 'open'`) and kept as it is. The port keeps one mesh per tile
//! for both (plan 1.4.2), so a tile is tessellated as deck draws it in the projection that asks
//! for it: tiles up to z12 ([`GLOBE_MAX_Z`], every tile the globe selects) the globe's way,
//! tiles z13 and z14 (only the flat map selects them) the flat map's way. A z12 tile also serves
//! the flat map from zoom 12 to 12.5 with the globe's cuts: where a grid line crosses it, the
//! cut's points lie on the line between two of the tile's points in longitude and latitude, at
//! most a metre or so off the straight Mercator line (under a tenth of a pixel there).
//!
//! Deviations, all invisible:
//! - The plan's row says "globe subdivision for tiles z ≤ 6"; deck cuts every tile it draws on
//!   the globe, and so does this (above z6 a cut is rare, as a tile spans less than the grid).
//! - A feature whose colour is transparent ([`NONE`]: maritime borders, rail, paths, other
//!   aeroways' lines, `boundary` and `transportation` areas) is left out; deck draws it, and with
//!   deck's blending a colour of alpha 0 changes no pixel.
//! - A property is read as on the globe's GeoJSON tiles (an absent `admin_level` is `NaN`, so a
//!   state border's colour and width); deck's binary tiles on the flat map read an absent number
//!   as 0 (a country border). OpenMapTiles gives every boundary its `admin_level`.
//! - A feature of unknown geometry type is skipped (loaders.gl throws and loses the tile).
//! - Triangles keep earcut's turn, which is not deck's culling order: draw them unculled, as deck
//!   does (`SURFACE_NO_CULL`).
//! - Not here, for the layer that draws these (item B2): on the flat map deck clips each tile's
//!   features to the tile's square (MVTLayer adds a `ClipExtension`, default bounds `[0, 0, 1,
//!   1]` in tile units); on the globe it does not clip.
//!
//! Runways: today the centre line of a runway is 4 px ([`RUNWAY_PX`]) until a runway 45 m wide
//! is wider (doc 01 §5.7, item B3), when the map draws it 0 px wide and another layer draws it at
//! 45 m. [`VectorTile::runway_lines`] names the runway segments of [`VectorTile::lines`], so the
//! layer can leave them out of a draw without tessellating again.

use std::f64::consts::PI;
use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use glam::DVec2;
use rustradar_core::mvt::{self, Feature, GeomType, Line, Value};

use crate::camera::lng_lat_to_world;
use crate::geom::densify::GLOBE_RESOLUTION_DEG;
use crate::geom::grid_cut::{GridOptions, PolylineGridOptions, cut_polygon_by_grid, cut_polyline_by_grid};
use crate::geom::path::{Colors, LocalMercator, Paint, PathBuilder, PathSegment, PathType};
use crate::tiles::TileIndex;

/// The source layers the vector map decodes, in the order their features are drawn (G:539-541).
pub const SOURCE_LAYERS: [&str; 4] = ["water", "boundary", "transportation", "aeroway"];

/// The source's last zoom (`maxZoom: 14`): deeper views draw z14 tiles, over-zoomed.
pub const MAX_ZOOM: u32 = 14;

/// The deepest tile the globe draws (deck's projection swap at zoom 12 and its tile rule
/// `round(zoom)` for 512-px tiles): tiles up to here are tessellated the globe's way.
pub const GLOBE_MAX_Z: u32 = 12;

/// An RGBA colour, 0–255, gamma space (deck's `getFillColor` / `getLineColor`).
pub type Rgba = [u8; 4];

/// The palette (G:497-506, doc 01 §5.6).
pub const NONE: Rgba = [0, 0, 0, 0];
pub const LAND: Rgba = [50, 50, 50, 255];
pub const WATER: Rgba = [30, 30, 30, 255];
/// Country borders.
pub const BORDER: Rgba = [118, 118, 118, 255];
pub const BORDER_STATE: Rgba = [84, 84, 84, 255];
pub const ROAD_MAJOR: Rgba = [88, 88, 88, 255];
pub const ROAD_MINOR: Rgba = [66, 66, 66, 255];
pub const RUNWAY: Rgba = [163, 119, 52, 255];
pub const TAXIWAY: Rgba = [105, 80, 46, 255];
pub const APRON: Rgba = [60, 60, 60, 255];

/// `MAJOR_ROADS` (G:507).
pub const MAJOR_ROADS: [&str; 3] = ["motorway", "trunk", "primary"];
/// `MINOR_ROADS` (G:508).
pub const MINOR_ROADS: [&str; 4] = ["secondary", "tertiary", "minor", "service"];

/// A runway's centre line in screen pixels while the true-width runways are off (G:547).
pub const RUNWAY_PX: f64 = 4.0;

/// One vertex of a fill: the layout of `rustradar_map::pipelines::fill::Vertex` (16 bytes,
/// `#[repr(C)]`), so a mesh goes to the GPU with `bytemuck::cast_slice`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct FillVertex {
    /// `u` east and `v` south in tile units, and the height in metres (always 0).
    pub position: [f32; 3],
    /// RGBA, 0–255, gamma space.
    pub color: [u8; 4],
}

/// One tile of the vector map, tessellated.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VectorTile {
    /// The fills' vertices, in tile units.
    pub fill_vertices: Vec<FillVertex>,
    /// Three indices into [`Self::fill_vertices`] per triangle, polygon after polygon in feature
    /// order.
    pub fill_indices: Vec<u32>,
    /// The lines, as instances of the `path` pipeline in tile units (`LocalMercator::tile`),
    /// widths in screen pixels, in feature order.
    pub lines: Vec<PathSegment>,
    /// The ranges of [`Self::lines`] that are runway centre lines (each range of whole lines, in
    /// order, adjacent ranges merged).
    pub runway_lines: Vec<Range<u32>>,
}

impl VectorTile {
    /// Whether it draws nothing.
    pub fn is_empty(&self) -> bool {
        self.fill_indices.is_empty() && self.lines.is_empty()
    }

    /// The bytes it holds (for the tile cache's budget).
    pub fn byte_size(&self) -> usize {
        size_of_val(self.fill_vertices.as_slice())
            + size_of_val(self.fill_indices.as_slice())
            + size_of_val(self.lines.as_slice())
            + size_of_val(self.runway_lines.as_slice())
    }

    /// Appends one polygon's vertices and triangles (indices into `vertices`).
    fn push_fill(&mut self, vertices: Vec<FillVertex>, indices: Vec<u32>) {
        let base = self.fill_vertices.len() as u32;
        self.fill_vertices.extend(vertices);
        self.fill_indices.extend(indices.into_iter().map(|i| base + i));
    }

    /// [`Self::lines`] without the runway centre lines, as ranges of instances (what the vector
    /// map draws while the true-width runways are on).
    pub fn lines_without_runways(&self) -> Vec<Range<u32>> {
        let mut ranges = Vec::new();
        let mut start = 0;
        for runway in &self.runway_lines {
            if runway.start > start {
                ranges.push(start..runway.start);
            }
            start = runway.end;
        }
        let end = self.lines.len() as u32;
        if end > start {
            ranges.push(start..end);
        }
        ranges
    }
}

/// [`tessellate`] stopped because its job was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

/// JavaScript's truthiness of a property (`undefined` when absent).
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None => false,
        Some(Value::Str(s)) => !s.is_empty(),
        Some(&Value::Num(n)) => n != 0.0 && !n.is_nan(),
        Some(&Value::Bool(b)) => b,
    }
}

/// JavaScript's `Number(value)` of a property: `NaN` when absent, a boolean as 1 or 0, a string
/// as `StringToNumber` reads it ([`string_to_number`]).
fn number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Str(s)) => string_to_number(s),
        Some(&Value::Num(n)) => n,
        Some(&Value::Bool(b)) => f64::from(u8::from(b)),
    }
}

/// ECMAScript's `StringToNumber`: white space and line terminators trimmed; empty is 0;
/// `0x`/`0o`/`0b` integers (no sign); a decimal literal with an optional sign, digits with an
/// optional point, an optional exponent, or `Infinity`; anything else `NaN`.
fn string_to_number(s: &str) -> f64 {
    let t = s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if t.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)] {
        if let Some(digits) = t.strip_prefix(prefix) {
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            return digits.chars().fold(0.0, |n, c| n * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0)));
        }
    }
    let (sign, body) = match t.as_bytes()[0] {
        b'+' => (1.0, &t[1..]),
        b'-' => (-1.0, &t[1..]),
        _ => (1.0, t),
    };
    if body == "Infinity" {
        return sign * f64::INFINITY;
    }
    // digits, an optional point, digits (at least one digit in all), then an optional exponent
    let bytes = body.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < bytes.len() && bytes[*i].is_ascii_digit() {
            *i += 1;
        }
        *i - start
    };
    let mut mantissa = digits(&mut i);
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        mantissa += digits(&mut i);
    }
    if mantissa == 0 {
        return f64::NAN;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return f64::NAN;
        }
    }
    if i != bytes.len() {
        return f64::NAN;
    }
    body.parse::<f64>().map_or(f64::NAN, |n| sign * n)
}

/// `p.class === name`.
fn class_is(f: &Feature<'_>, name: &str) -> bool {
    f.text("class") == Some(name)
}

/// `set.has(String(p.class))`: only a string can equal one of the names (a number, a boolean or
/// `undefined` turn into digits, `true`, `false` or `undefined`).
fn class_in(f: &Feature<'_>, set: &[&str]) -> bool {
    f.text("class").is_some_and(|c| set.contains(&c))
}

/// `vectorFill` (G:510-514): a polygon's fill colour by source layer.
pub fn vector_fill(layer: &str, f: &Feature<'_>) -> Rgba {
    if layer == "water" {
        return WATER;
    }
    if layer == "aeroway" {
        return if class_is(f, "runway") {
            RUNWAY
        } else if class_is(f, "taxiway") {
            TAXIWAY
        } else {
            APRON
        };
    }
    NONE
}

/// `vectorLine` (G:515-526): a line's colour by source layer.
pub fn vector_line(layer: &str, f: &Feature<'_>) -> Rgba {
    match layer {
        "boundary" => {
            if truthy(f.get("maritime")) {
                NONE
            } else if number(f.get("admin_level")) <= 2.0 {
                BORDER
            } else {
                BORDER_STATE
            }
        }
        "transportation" => {
            if class_in(f, &MAJOR_ROADS) {
                ROAD_MAJOR
            } else if class_in(f, &MINOR_ROADS) {
                ROAD_MINOR
            } else {
                NONE
            }
        }
        "aeroway" => {
            if class_is(f, "runway") {
                RUNWAY
            } else if class_is(f, "taxiway") {
                TAXIWAY
            } else {
                NONE
            }
        }
        _ => NONE,
    }
}

/// `vectorLineWidth` (G:527-538): a line's width in screen pixels by source layer (a runway's
/// [`RUNWAY_PX`]; the layer draws it 0 px wide while the true-width runways are on).
pub fn vector_line_width(layer: &str, f: &Feature<'_>) -> f64 {
    match layer {
        "boundary" => {
            if number(f.get("admin_level")) <= 2.0 {
                1.0
            } else {
                0.6
            }
        }
        "transportation" => {
            if class_is(f, "motorway") || class_is(f, "trunk") {
                1.4
            } else if class_is(f, "primary") {
                1.0
            } else {
                0.6
            }
        }
        "aeroway" => {
            if class_is(f, "runway") {
                RUNWAY_PX
            } else {
                1.5
            }
        }
        _ => 0.0,
    }
}

/// Whether a line is a runway centre line (the vector map's runway width switch, G:601-603).
pub fn is_runway_line(layer: &str, f: &Feature<'_>) -> bool {
    layer == "aeroway" && class_is(f, "runway")
}

/// loaders.gl's `signedArea` (`mvt/lib/utils/geometry-utils.js`), on an open ring: twice the
/// area, its sign the ring's turn (positive for a ring clockwise with y down, an outline in the
/// spec's terms).
fn signed_area(ring: &[(f64, f64)]) -> f64 {
    let n = ring.len();
    let mut sum = 0.0;
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (p1, p2) = (ring[i], ring[j]);
        sum += (p2.0 - p1.0) * (p1.1 + p2.1);
        j = i;
    }
    sum
}

/// loaders.gl's `classifyRings`: a feature's rings grouped into polygons, each an outline and
/// its holes. The first ring with an area sets which turn is an outline; every later ring of
/// that turn starts a polygon, every other one is a hole of the polygon before it. Rings without
/// an area are dropped, as are holes before any outline; a single ring is a polygon whatever its
/// area. No rings, no polygon.
pub fn classify_rings(rings: &[Line]) -> Vec<Vec<&Line>> {
    if rings.is_empty() {
        return Vec::new();
    }
    if rings.len() == 1 {
        return vec![vec![&rings[0]]];
    }
    let mut polygons = Vec::new();
    let mut polygon: Option<Vec<&Line>> = None;
    let mut ccw = None;
    for ring in rings {
        let area = signed_area(ring);
        if area == 0.0 {
            continue;
        }
        let ccw = *ccw.get_or_insert(area < 0.0);
        if ccw == (area < 0.0) {
            if let Some(done) = polygon.replace(vec![ring]) {
                polygons.push(done);
            }
        } else if let Some(open) = &mut polygon {
            open.push(ring);
        }
    }
    polygons.extend(polygon);
    polygons
}

/// math.gl's `getPolygonSignedArea` of the ring `points[start..end]` (two numbers a vertex):
/// positive for a ring clockwise with y up.
fn math_gl_area(points: &[f64], start: usize, end: usize) -> f64 {
    let mut area = 0.0;
    let mut j = end - 2;
    let mut i = start;
    while i < end {
        area += (points[i] - points[j]) * (points[i + 1] + points[j + 1]);
        j = i;
        i += 2;
    }
    area / 2.0
}

/// math.gl's `modifyPolygonWindingDirection`: reverses the ring `points[start..end]` unless the
/// sign of its area is `direction` (1 clockwise, −1 counter-clockwise, y up; an area of 0 is
/// neither, so such a ring is reversed).
fn set_winding(points: &mut [f64], start: usize, end: usize, direction: f64) {
    let area = math_gl_area(points, start, end);
    let sign = if area > 0.0 {
        1.0
    } else if area < 0.0 {
        -1.0
    } else {
        0.0
    };
    if sign != direction {
        let ring = &mut points[start..end];
        let n = ring.len() / 2;
        for k in 0..n / 2 {
            ring.swap(2 * k, 2 * (n - 1 - k));
            ring.swap(2 * k + 1, 2 * (n - 1 - k) + 1);
        }
    }
}

/// deck's `Polygon.normalize` for a nested polygon in longitude and latitude: the rings one after
/// another (two numbers a vertex), each closed (its first point repeated at its end unless it
/// already is), the outline clockwise and the holes counter-clockwise; and where each hole
/// starts, in values.
fn normalize(rings: &[Vec<[f64; 2]>]) -> (Vec<f64>, Vec<usize>) {
    let mut positions = Vec::new();
    let mut holes = Vec::new();
    for (i, ring) in rings.iter().enumerate() {
        let start = positions.len();
        if i > 0 {
            holes.push(start);
        }
        positions.extend(ring.iter().flatten());
        if let (Some(first), Some(last)) = (ring.first(), ring.last())
            && first != last
        {
            positions.extend(first);
        }
        let end = positions.len();
        if end > start {
            set_winding(&mut positions, start, end, if i == 0 { 1.0 } else { -1.0 });
        }
    }
    (positions, holes)
}

/// The globe's 5° grid cell `[left, bottom, right, top]` of a point (math.gl's `getGridCell`).
fn grid_cell([lon, lat]: [f64; 2]) -> [f64; 4] {
    let left = (lon / GLOBE_RESOLUTION_DEG).floor() * GLOBE_RESOLUTION_DEG;
    let bottom = (lat / GLOBE_RESOLUTION_DEG).floor() * GLOBE_RESOLUTION_DEG;
    [left, bottom, left + GLOBE_RESOLUTION_DEG, bottom + GLOBE_RESOLUTION_DEG]
}

/// A tile's units ↔ longitude and latitude, as loaders.gl's `projectToLngLat` turns them and
/// back to the tile's `(u, v)` through the world Mercator.
struct TileGeo {
    size: f64,
    x0: f64,
    y0: f64,
    extent: f64,
    local: LocalMercator,
}

impl TileGeo {
    fn new(tile: TileIndex, extent: f64) -> Self {
        let size = extent * 2f64.powi(tile.z as i32);
        TileGeo { size, x0: extent * f64::from(tile.x), y0: extent * f64::from(tile.y), extent, local: LocalMercator::tile(tile) }
    }

    /// `projectToLngLat`: a point in the tile's units → `[lon, lat]`.
    fn lng_lat(&self, (x, y): (f64, f64)) -> [f64; 2] {
        let lon = ((x + self.x0) * 360.0) / self.size - 180.0;
        let y2 = 180.0 - ((y + self.y0) * 360.0) / self.size;
        [lon, (360.0 / PI) * ((y2 * PI) / 180.0).exp().atan() - 90.0]
    }

    /// `[lon, lat]` → the tile's `(u, v)`.
    fn local(&self, lng_lat: [f64; 2]) -> [f64; 2] {
        self.local.from_world(DVec2::from_array(lng_lat_to_world(lng_lat)))
    }

    /// A point in the tile's units → `(u, v)` (loaders.gl's `convertToLocalCoordinates`).
    fn unit(&self, (x, y): (f64, f64)) -> [f64; 2] {
        [x / self.extent, y / self.extent]
    }

    /// The 5° cell the longitudes and latitudes of points between `min` and `max` (tile units,
    /// y down) all fit in, if they all fit in the one of `first`, the cell math.gl starts from.
    fn fits_one_cell(&self, first: (f64, f64), min: (f64, f64), max: (f64, f64)) -> bool {
        let [left, bottom, right, top] = grid_cell(self.lng_lat(first));
        // longitude grows with x, latitude falls as y grows
        let [west, north] = self.lng_lat(min);
        let [east, south] = self.lng_lat(max);
        west >= left && east <= right && south >= bottom && north <= top
    }
}

/// The least and the greatest x and y of `points`.
fn bounds(points: &[(f64, f64)]) -> ((f64, f64), (f64, f64)) {
    points.iter().fold(((f64::INFINITY, f64::INFINITY), (f64::NEG_INFINITY, f64::NEG_INFINITY)), |(lo, hi), &(x, y)| {
        ((lo.0.min(x), lo.1.min(y)), (hi.0.max(x), hi.1.max(y)))
    })
}

/// earcut's triangles of `positions` (two numbers a vertex, holes from the vertex indices
/// `holes`) as fill vertices in tile units (`to_local`) and indices into them.
fn triangulate(positions: &[f64], holes: &[usize], color: Rgba, to_local: impl Fn([f64; 2]) -> [f64; 2]) -> (Vec<FillVertex>, Vec<u32>) {
    let indices = earcut::earcut(positions, holes, 2);
    let vertices = positions
        .chunks_exact(2)
        .map(|p| {
            let [u, v] = to_local([p[0], p[1]]);
            FillVertex { position: [u as f32, v as f32, 0.0], color }
        })
        .collect();
    (vertices, indices.into_iter().map(|i| i as u32).collect())
}

/// Builds one tile.
struct Builder {
    geo: TileGeo,
    globe: bool,
    tile: VectorTile,
    paths: PathBuilder,
}

impl Builder {
    /// One polygon (an outline and its holes, tile units) in `color`.
    fn polygon(&mut self, rings: &[&Line], color: Rgba) {
        if !self.globe {
            // the flat map's binary polygon: each ring as loaders.gl reads it (closed by its
            // ClosePath, in the tile's turn), cut into triangles in tile units
            let mut positions = Vec::new();
            let mut holes = Vec::new();
            for (i, ring) in rings.iter().enumerate() {
                if i > 0 {
                    holes.push(positions.len() / 2);
                }
                positions.extend(ring.iter().chain(ring.first()).flat_map(|&p| self.geo.unit(p)));
            }
            let (vertices, indices) = triangulate(&positions, &holes, color, |p| p);
            self.tile.push_fill(vertices, indices);
            return;
        }
        // the globe's GeoJSON polygon: longitude and latitude, normalised, cut on the 5° grid,
        // each part cut into triangles
        let lng_lat: Vec<Vec<[f64; 2]>> = rings.iter().map(|r| r.iter().map(|&p| self.geo.lng_lat(p)).collect()).collect();
        let (positions, holes) = normalize(&lng_lat);
        let options = GridOptions { size: 2, grid_resolution: GLOBE_RESOLUTION_DEG, grid_offset: [0.0, 0.0], edge_types: true };
        for part in cut_polygon_by_grid(&positions, &holes, &options) {
            let holes: Vec<usize> = part.hole_indices.iter().map(|h| h / 2).collect();
            let (vertices, indices) = triangulate(&part.positions, &holes, color, |p| self.geo.local(p));
            self.tile.push_fill(vertices, indices);
        }
    }

    /// One line (tile units) in `color`, `width` px wide; a runway's segments are recorded.
    fn line(&mut self, line: &Line, color: Rgba, width: f64, runway: bool) {
        let points: Vec<[f64; 3]> = if !self.globe {
            // the flat map's binary paths: as they are
            line.iter().map(|&p| self.geo.unit(p)).map(|[u, v]| [u, v, 0.0]).collect()
        } else {
            let (min, max) = bounds(line);
            if line.is_empty() || self.geo.fits_one_cell(line[0], min, max) {
                // no grid line crossed: the cut only drops repeated points
                let mut kept: Vec<(f64, f64)> = Vec::with_capacity(line.len());
                for &p in line {
                    if kept.last() != Some(&p) {
                        kept.push(p);
                    }
                }
                kept.into_iter().map(|p| self.geo.unit(p)).map(|[u, v]| [u, v, 0.0]).collect()
            } else {
                let lng_lat: Vec<f64> = line.iter().flat_map(|&p| self.geo.lng_lat(p)).collect();
                let options = PolylineGridOptions { size: 2, broken: false, grid_resolution: GLOBE_RESOLUTION_DEG, grid_offset: [0.0, 0.0] };
                let parts = cut_polyline_by_grid(&lng_lat, &options);
                parts.iter().flat_map(|part| part.chunks_exact(2)).map(|p| self.geo.local([p[0], p[1]])).map(|[u, v]| [u, v, 0.0]).collect()
            }
        };
        let path_type = if self.globe { PathType::Auto } else { PathType::Open };
        let paint = Paint { color: Colors::Same(color), width, dash_array: [0.0, 0.0], path_type };
        let start = self.paths.segments().len() as u32;
        self.paths.push(&points, &paint);
        let end = self.paths.segments().len() as u32;
        if runway && end > start {
            match self.tile.runway_lines.last_mut() {
                Some(last) if last.end == start => last.end = end,
                _ => self.tile.runway_lines.push(start..end),
            }
        }
    }
}

/// The vector map of one OpenFreeMap tile (`bytes`, the tile as served, decompressed) at
/// `tile`: its fills and lines in today's palette, tessellated for the projection that draws a
/// tile of that zoom (see the module notes). `cancelled` is asked between features; once it says
/// so the work stops.
pub fn tessellate(tile: TileIndex, bytes: &[u8], cancelled: &dyn Fn() -> bool) -> Result<VectorTile, Cancelled> {
    let mut b = Builder {
        geo: TileGeo::new(tile, mvt::EXTENT_DEFAULT),
        globe: tile.z <= GLOBE_MAX_Z,
        tile: VectorTile::default(),
        paths: PathBuilder::new(LocalMercator::tile(tile)),
    };
    for layer in mvt::layers_named(bytes, &SOURCE_LAYERS) {
        // each layer has its own extent
        b.geo = TileGeo::new(tile, layer.extent);
        for feature in layer.features() {
            if cancelled() {
                return Err(Cancelled);
            }
            match feature.kind {
                GeomType::Polygon => {
                    let color = vector_fill(layer.name, &feature);
                    if color[3] == 0 {
                        continue;
                    }
                    let rings = feature.rings();
                    for polygon in classify_rings(&rings) {
                        b.polygon(&polygon, color);
                    }
                }
                GeomType::LineString => {
                    let color = vector_line(layer.name, &feature);
                    if color[3] == 0 {
                        continue;
                    }
                    let width = vector_line_width(layer.name, &feature);
                    let runway = is_runway_line(layer.name, &feature);
                    for line in feature.lines() {
                        b.line(&line, color, width, runway);
                    }
                }
                GeomType::Point | GeomType::Unknown => {}
            }
        }
    }
    let Builder { tile: mut out, paths, .. } = b;
    out.lines = paths.finish();
    Ok(out)
}

/// earcut 2.2.4 (`node_modules/earcut/src/earcut.js`), the triangulation deck.gl 9.4's
/// SolidPolygonLayer runs (`solid-polygon-layer/polygon.js`, `getSurfaceIndices`), transliterated
/// with its formulas and branch order, so the globe gets deck's very triangles.
///
/// Why not [`crate::geom::earcut`]: the `earcutr` 0.5 crate behind it follows an older earcut
/// whose hole bridging goes wrong where a hole touches the outline. Every part of a polygon cut
/// on the grid has such holes (the halves of an island cut by a grid line), and on the fixture
/// tiles `earcutr` covered too much (water over land, up to three times a part's area) in 34 of
/// 11 814 cut parts and 2 of 2 924 whole polygons, where earcut 2.2.4 is right on all of them.
/// This port belongs in `geom::earcut` (S1.24's file); it lives here until that module takes it.
///
/// Deviation, where earcut.js throws: an empty hole is skipped.
mod earcut {
    use rustradar_core::num::to_int32;

    use crate::geom::{js_max, js_min};

    const NIL: usize = usize::MAX;

    /// earcut's `Node`.
    struct Node {
        /// The vertex's index into the data (vertex × dim).
        i: usize,
        x: f64,
        y: f64,
        prev: usize,
        next: usize,
        z: i32,
        prev_z: usize,
        next_z: usize,
        steiner: bool,
    }

    /// JavaScript's truthiness of a number.
    fn truthy(x: f64) -> bool {
        x != 0.0 && !x.is_nan()
    }

    struct Ear<'a> {
        data: &'a [f64],
        dim: usize,
        nodes: Vec<Node>,
        triangles: Vec<usize>,
    }

    /// `earcut(data, holeIndices, dim)`: three vertex indices per triangle.
    pub(super) fn earcut(data: &[f64], hole_indices: &[usize], dim: usize) -> Vec<usize> {
        let dim = if dim == 0 { 2 } else { dim };
        let has_holes = !hole_indices.is_empty();
        let outer_len = if has_holes { (hole_indices[0] * dim).min(data.len()) } else { data.len() };
        let mut e = Ear { data, dim, nodes: Vec::with_capacity(data.len() / dim * 2), triangles: Vec::new() };
        let mut outer_node = e.linked_list(0, outer_len, true);
        if outer_node == NIL || e.nodes[outer_node].next == e.nodes[outer_node].prev {
            return e.triangles;
        }
        let (mut min_x, mut min_y, mut inv_size) = (0.0, 0.0, 0.0);
        if has_holes {
            outer_node = e.eliminate_holes(hole_indices, outer_node);
        }
        // if the shape is not too simple, a z-order curve hash is used later: the polygon's bbox
        if data.len() > 80 * dim {
            min_x = data[0];
            let mut max_x = data[0];
            min_y = data[1];
            let mut max_y = data[1];
            let mut i = dim;
            while i < outer_len {
                let (x, y) = (data[i], data[i + 1]);
                if x < min_x {
                    min_x = x;
                }
                if y < min_y {
                    min_y = y;
                }
                if x > max_x {
                    max_x = x;
                }
                if y > max_y {
                    max_y = y;
                }
                i += dim;
            }
            // min_x, min_y and inv_size turn coordinates into integers for the z-order
            inv_size = js_max(max_x - min_x, max_y - min_y);
            inv_size = if inv_size != 0.0 { 32767.0 / inv_size } else { 0.0 };
        }
        e.earcut_linked(outer_node, min_x, min_y, inv_size, 0);
        e.triangles
    }

    /// `signedArea(data, start, end, dim)`.
    fn signed_area(data: &[f64], start: usize, end: usize, dim: usize) -> f64 {
        let mut sum = 0.0;
        let mut j = end - dim;
        let mut i = start;
        while i < end {
            sum += (data[j] - data[i]) * (data[i + 1] + data[j + 1]);
            j = i;
            i += dim;
        }
        sum
    }

    /// `pointInTriangle`: whether the point lies within the convex triangle.
    #[allow(clippy::too_many_arguments)]
    fn point_in_triangle(ax: f64, ay: f64, bx: f64, by: f64, cx: f64, cy: f64, px: f64, py: f64) -> bool {
        (cx - px) * (ay - py) >= (ax - px) * (cy - py)
            && (ax - px) * (by - py) >= (bx - px) * (ay - py)
            && (bx - px) * (cy - py) >= (cx - px) * (by - py)
    }

    /// `sign(num)`.
    fn sign(num: f64) -> i8 {
        if num > 0.0 {
            1
        } else if num < 0.0 {
            -1
        } else {
            0
        }
    }

    /// `zOrder`: the z-order of a point from its coordinates and the inverse of the longer side
    /// of the data's bbox.
    fn z_order(x: f64, y: f64, min_x: f64, min_y: f64, inv_size: f64) -> i32 {
        // coordinates are taken into the non-negative 15-bit range
        let mut x = to_int32((x - min_x) * inv_size);
        let mut y = to_int32((y - min_y) * inv_size);
        x = (x | x.wrapping_shl(8)) & 0x00FF_00FF;
        x = (x | x.wrapping_shl(4)) & 0x0F0F_0F0F;
        x = (x | x.wrapping_shl(2)) & 0x3333_3333;
        x = (x | x.wrapping_shl(1)) & 0x5555_5555;
        y = (y | y.wrapping_shl(8)) & 0x00FF_00FF;
        y = (y | y.wrapping_shl(4)) & 0x0F0F_0F0F;
        y = (y | y.wrapping_shl(2)) & 0x3333_3333;
        y = (y | y.wrapping_shl(1)) & 0x5555_5555;
        x | y.wrapping_shl(1)
    }

    impl Ear<'_> {
        fn x(&self, p: usize) -> f64 {
            self.nodes[p].x
        }

        fn y(&self, p: usize) -> f64 {
            self.nodes[p].y
        }

        fn next(&self, p: usize) -> usize {
            self.nodes[p].next
        }

        fn prev(&self, p: usize) -> usize {
            self.nodes[p].prev
        }

        /// `area(p, q, r)`: the signed area of a triangle.
        fn area(&self, p: usize, q: usize, r: usize) -> f64 {
            let (p, q, r) = (&self.nodes[p], &self.nodes[q], &self.nodes[r]);
            (q.y - p.y) * (r.x - q.x) - (q.x - p.x) * (r.y - q.y)
        }

        /// `equals(p1, p2)`.
        fn equals(&self, p1: usize, p2: usize) -> bool {
            self.nodes[p1].x == self.nodes[p2].x && self.nodes[p1].y == self.nodes[p2].y
        }

        /// `insertNode`: a node, linked after `last` (a circular doubly linked list).
        fn insert_node(&mut self, i: usize, x: f64, y: f64, last: usize) -> usize {
            let p = self.new_node(i, x, y);
            if last == NIL {
                self.nodes[p].prev = p;
                self.nodes[p].next = p;
            } else {
                let last_next = self.nodes[last].next;
                self.nodes[p].next = last_next;
                self.nodes[p].prev = last;
                self.nodes[last_next].prev = p;
                self.nodes[last].next = p;
            }
            p
        }

        /// `new Node(i, x, y)`.
        fn new_node(&mut self, i: usize, x: f64, y: f64) -> usize {
            self.nodes.push(Node { i, x, y, prev: NIL, next: NIL, z: 0, prev_z: NIL, next_z: NIL, steiner: false });
            self.nodes.len() - 1
        }

        /// `removeNode`.
        fn remove_node(&mut self, p: usize) {
            let Node { prev, next, prev_z, next_z, .. } = self.nodes[p];
            self.nodes[next].prev = prev;
            self.nodes[prev].next = next;
            if prev_z != NIL {
                self.nodes[prev_z].next_z = next_z;
            }
            if next_z != NIL {
                self.nodes[next_z].prev_z = prev_z;
            }
        }

        /// `linkedList`: a circular doubly linked list of the points from `start` to `end`, in
        /// the winding asked for.
        fn linked_list(&mut self, start: usize, end: usize, clockwise: bool) -> usize {
            let (data, dim) = (self.data, self.dim);
            let mut last = NIL;
            if end <= start {
                return NIL;
            }
            if clockwise == (signed_area(data, start, end, dim) > 0.0) {
                let mut i = start;
                while i < end {
                    last = self.insert_node(i, data[i], data[i + 1], last);
                    i += dim;
                }
            } else {
                let mut i = end - dim;
                loop {
                    last = self.insert_node(i, data[i], data[i + 1], last);
                    if i < start + dim {
                        break;
                    }
                    i -= dim;
                }
            }
            if last != NIL && self.equals(last, self.next(last)) {
                self.remove_node(last);
                last = self.next(last);
            }
            last
        }

        /// `filterPoints`: removes collinear or duplicate points.
        fn filter_points(&mut self, start: usize, end: usize) -> usize {
            if start == NIL {
                return start;
            }
            let mut end = if end == NIL { start } else { end };
            let mut p = start;
            loop {
                let mut again = false;
                if !self.nodes[p].steiner && (self.equals(p, self.next(p)) || self.area(self.prev(p), p, self.next(p)) == 0.0) {
                    self.remove_node(p);
                    p = self.prev(p);
                    end = p;
                    if p == self.next(p) {
                        break;
                    }
                    again = true;
                } else {
                    p = self.next(p);
                }
                if !(again || p != end) {
                    break;
                }
            }
            end
        }

        /// `earcutLinked`: the main ear slicing loop.
        fn earcut_linked(&mut self, ear: usize, min_x: f64, min_y: f64, inv_size: f64, pass: u8) {
            if ear == NIL {
                return;
            }
            // interlink polygon nodes in z-order
            if pass == 0 && truthy(inv_size) {
                self.index_curve(ear, min_x, min_y, inv_size);
            }
            let mut ear = ear;
            let mut stop = ear;
            // iterate through ears, slicing them one by one
            while self.prev(ear) != self.next(ear) {
                let prev = self.prev(ear);
                let next = self.next(ear);
                let is_ear = if truthy(inv_size) { self.is_ear_hashed(ear, min_x, min_y, inv_size) } else { self.is_ear(ear) };
                if is_ear {
                    // cut off the triangle
                    let dim = self.dim;
                    self.triangles.push(self.nodes[prev].i / dim);
                    self.triangles.push(self.nodes[ear].i / dim);
                    self.triangles.push(self.nodes[next].i / dim);
                    self.remove_node(ear);
                    // skipping the next vertex leads to fewer sliver triangles
                    ear = self.next(next);
                    stop = self.next(next);
                    continue;
                }
                ear = next;
                // the whole remaining polygon looped through and no more ears found
                if ear == stop {
                    if pass == 0 {
                        // try filtering points and slicing again
                        let filtered = self.filter_points(ear, NIL);
                        self.earcut_linked(filtered, min_x, min_y, inv_size, 1);
                    } else if pass == 1 {
                        // if this didn't work, try curing all small self-intersections locally
                        let filtered = self.filter_points(ear, NIL);
                        let ear = self.cure_local_intersections(filtered);
                        self.earcut_linked(ear, min_x, min_y, inv_size, 2);
                    } else if pass == 2 {
                        // as a last resort, try splitting the remaining polygon into two
                        self.split_earcut(ear, min_x, min_y, inv_size);
                    }
                    break;
                }
            }
        }

        /// `isEar`: whether a node makes a valid ear with its neighbours.
        fn is_ear(&self, ear: usize) -> bool {
            let (a, b, c) = (self.prev(ear), ear, self.next(ear));
            if self.area(a, b, c) >= 0.0 {
                return false; // reflex, can't be an ear
            }
            // no other point may lie inside the ear
            let (ax, bx, cx, ay, by, cy) = (self.x(a), self.x(b), self.x(c), self.y(a), self.y(b), self.y(c));
            let (x0, y0, x1, y1) = bbox(ax, bx, cx, ay, by, cy);
            let mut p = self.next(c);
            while p != a {
                let (px, py) = (self.x(p), self.y(p));
                if px >= x0
                    && px <= x1
                    && py >= y0
                    && py <= y1
                    && point_in_triangle(ax, ay, bx, by, cx, cy, px, py)
                    && self.area(self.prev(p), p, self.next(p)) >= 0.0
                {
                    return false;
                }
                p = self.next(p);
            }
            true
        }

        /// Whether `p` (not `a` or `c`) lies in the ear's triangle and is not reflex (the test of
        /// `isEarHashed`'s loops).
        #[allow(clippy::too_many_arguments)]
        fn blocks(&self, p: usize, a: usize, c: usize, [x0, y0, x1, y1]: [f64; 4], [ax, ay, bx, by, cx, cy]: [f64; 6]) -> bool {
            let (px, py) = (self.x(p), self.y(p));
            px >= x0
                && px <= x1
                && py >= y0
                && py <= y1
                && p != a
                && p != c
                && point_in_triangle(ax, ay, bx, by, cx, cy, px, py)
                && self.area(self.prev(p), p, self.next(p)) >= 0.0
        }

        /// `isEarHashed`.
        fn is_ear_hashed(&self, ear: usize, min_x: f64, min_y: f64, inv_size: f64) -> bool {
            let (a, b, c) = (self.prev(ear), ear, self.next(ear));
            if self.area(a, b, c) >= 0.0 {
                return false; // reflex, can't be an ear
            }
            let (ax, bx, cx, ay, by, cy) = (self.x(a), self.x(b), self.x(c), self.y(a), self.y(b), self.y(c));
            let (x0, y0, x1, y1) = bbox(ax, bx, cx, ay, by, cy);
            let rect = [x0, y0, x1, y1];
            let tri = [ax, ay, bx, by, cx, cy];
            // z-order range for the current triangle bbox
            let min_z = z_order(x0, y0, min_x, min_y, inv_size);
            let max_z = z_order(x1, y1, min_x, min_y, inv_size);
            let mut p = self.nodes[ear].prev_z;
            let mut n = self.nodes[ear].next_z;
            // look for points inside the triangle in both directions
            while p != NIL && self.nodes[p].z >= min_z && n != NIL && self.nodes[n].z <= max_z {
                if self.blocks(p, a, c, rect, tri) {
                    return false;
                }
                p = self.nodes[p].prev_z;
                if self.blocks(n, a, c, rect, tri) {
                    return false;
                }
                n = self.nodes[n].next_z;
            }
            // look for remaining points in decreasing z-order
            while p != NIL && self.nodes[p].z >= min_z {
                if self.blocks(p, a, c, rect, tri) {
                    return false;
                }
                p = self.nodes[p].prev_z;
            }
            // look for remaining points in increasing z-order
            while n != NIL && self.nodes[n].z <= max_z {
                if self.blocks(n, a, c, rect, tri) {
                    return false;
                }
                n = self.nodes[n].next_z;
            }
            true
        }

        /// `cureLocalIntersections`: cures small local self-intersections.
        fn cure_local_intersections(&mut self, start: usize) -> usize {
            let mut start = start;
            let mut p = start;
            loop {
                let a = self.prev(p);
                let b = self.next(self.next(p));
                if !self.equals(a, b) && self.intersects(a, p, self.next(p), b) && self.locally_inside(a, b) && self.locally_inside(b, a) {
                    let dim = self.dim;
                    self.triangles.push(self.nodes[a].i / dim);
                    self.triangles.push(self.nodes[p].i / dim);
                    self.triangles.push(self.nodes[b].i / dim);
                    // remove the two nodes involved
                    self.remove_node(p);
                    self.remove_node(self.next(p));
                    p = b;
                    start = b;
                }
                p = self.next(p);
                if p == start {
                    break;
                }
            }
            self.filter_points(p, NIL)
        }

        /// `splitEarcut`: splits the polygon in two along a valid diagonal and triangulates
        /// both halves.
        fn split_earcut(&mut self, start: usize, min_x: f64, min_y: f64, inv_size: f64) {
            let mut a = start;
            loop {
                let mut b = self.next(self.next(a));
                while b != self.prev(a) {
                    if self.nodes[a].i != self.nodes[b].i && self.is_valid_diagonal(a, b) {
                        // split the polygon in two by the diagonal
                        let mut c = self.split_polygon(a, b);
                        // filter collinear points around the cuts
                        a = self.filter_points(a, self.next(a));
                        c = self.filter_points(c, self.next(c));
                        // run earcut on each half
                        self.earcut_linked(a, min_x, min_y, inv_size, 0);
                        self.earcut_linked(c, min_x, min_y, inv_size, 0);
                        return;
                    }
                    b = self.next(b);
                }
                a = self.next(a);
                if a == start {
                    break;
                }
            }
        }

        /// `eliminateHoles`: links every hole into the outer ring, leaving one ring.
        fn eliminate_holes(&mut self, hole_indices: &[usize], outer_node: usize) -> usize {
            let (data, dim) = (self.data, self.dim);
            let mut queue = Vec::with_capacity(hole_indices.len());
            for (k, &h) in hole_indices.iter().enumerate() {
                let start = (h * dim).min(data.len());
                let end = if k < hole_indices.len() - 1 { (hole_indices[k + 1] * dim).min(data.len()) } else { data.len() };
                let list = self.linked_list(start, end, false);
                if list == NIL {
                    continue; // earcut.js throws
                }
                if list == self.next(list) {
                    self.nodes[list].steiner = true;
                }
                queue.push(self.get_leftmost(list));
            }
            // `queue.sort(compareX)`: a stable sort by x
            queue.sort_by(|&a, &b| self.x(a).partial_cmp(&self.x(b)).unwrap_or(std::cmp::Ordering::Equal));
            // process holes from left to right
            let mut outer_node = outer_node;
            for hole in queue {
                outer_node = self.eliminate_hole(hole, outer_node);
            }
            outer_node
        }

        /// `eliminateHole`: bridges a hole to the outer ring and links it in.
        fn eliminate_hole(&mut self, hole: usize, outer_node: usize) -> usize {
            let bridge = self.find_hole_bridge(hole, outer_node);
            if bridge == NIL {
                return outer_node;
            }
            let bridge_reverse = self.split_polygon(bridge, hole);
            // filter collinear points around the cuts
            self.filter_points(bridge_reverse, self.next(bridge_reverse));
            self.filter_points(bridge, self.next(bridge))
        }

        /// `findHoleBridge`: David Eberly's algorithm for a bridge between a hole and the outer
        /// ring.
        fn find_hole_bridge(&self, hole: usize, outer_node: usize) -> usize {
            let mut p = outer_node;
            let (hx, hy) = (self.x(hole), self.y(hole));
            let mut qx = f64::NEG_INFINITY;
            let mut m = NIL;
            // a segment crossed by a ray from the hole's leftmost point to the left; the
            // segment's end with the lesser x is a possible connection
            loop {
                let next = self.next(p);
                if hy <= self.y(p) && hy >= self.y(next) && self.y(next) != self.y(p) {
                    let x = self.x(p) + (hy - self.y(p)) * (self.x(next) - self.x(p)) / (self.y(next) - self.y(p));
                    if x <= hx && x > qx {
                        qx = x;
                        m = if self.x(p) < self.x(next) { p } else { next };
                        if x == hx {
                            return m; // the hole touches the outer segment; pick its leftmost end
                        }
                    }
                }
                p = next;
                if p == outer_node {
                    break;
                }
            }
            if m == NIL {
                return NIL;
            }
            // look for points inside the triangle of the hole's point, the crossing and the
            // end; with none, the connection is valid; else take the point of the least angle
            // with the ray
            let stop = m;
            let (mx, my) = (self.x(m), self.y(m));
            let mut tan_min = f64::INFINITY;
            p = m;
            loop {
                let (px, py) = (self.x(p), self.y(p));
                if hx >= px
                    && px >= mx
                    && hx != px
                    && point_in_triangle(if hy < my { hx } else { qx }, hy, mx, my, if hy < my { qx } else { hx }, hy, px, py)
                {
                    let tan = (hy - py).abs() / (hx - px); // tangential
                    if self.locally_inside(p, hole)
                        && (tan < tan_min
                            || (tan == tan_min && (px > self.x(m) || (px == self.x(m) && self.sector_contains_sector(m, p)))))
                    {
                        m = p;
                        tan_min = tan;
                    }
                }
                p = self.next(p);
                if p == stop {
                    break;
                }
            }
            m
        }

        /// `sectorContainsSector`: whether the sector at `m` contains the sector at `p`.
        fn sector_contains_sector(&self, m: usize, p: usize) -> bool {
            self.area(self.prev(m), m, self.prev(p)) < 0.0 && self.area(self.next(p), m, self.next(m)) < 0.0
        }

        /// `indexCurve`: interlinks the nodes in z-order.
        fn index_curve(&mut self, start: usize, min_x: f64, min_y: f64, inv_size: f64) {
            let mut p = start;
            loop {
                if self.nodes[p].z == 0 {
                    self.nodes[p].z = z_order(self.x(p), self.y(p), min_x, min_y, inv_size);
                }
                self.nodes[p].prev_z = self.prev(p);
                self.nodes[p].next_z = self.next(p);
                p = self.next(p);
                if p == start {
                    break;
                }
            }
            let last = self.nodes[p].prev_z;
            self.nodes[last].next_z = NIL;
            self.nodes[p].prev_z = NIL;
            self.sort_linked(p);
        }

        /// `sortLinked`: Simon Tatham's linked-list merge sort, on z.
        fn sort_linked(&mut self, list: usize) -> usize {
            let mut list = list;
            let mut in_size = 1;
            loop {
                let mut p = list;
                list = NIL;
                let mut tail = NIL;
                let mut num_merges = 0;
                while p != NIL {
                    num_merges += 1;
                    let mut q = p;
                    let mut p_size = 0;
                    for _ in 0..in_size {
                        p_size += 1;
                        q = self.nodes[q].next_z;
                        if q == NIL {
                            break;
                        }
                    }
                    let mut q_size = in_size;
                    while p_size > 0 || (q_size > 0 && q != NIL) {
                        let e;
                        if p_size != 0 && (q_size == 0 || q == NIL || self.nodes[p].z <= self.nodes[q].z) {
                            e = p;
                            p = self.nodes[p].next_z;
                            p_size -= 1;
                        } else {
                            e = q;
                            q = self.nodes[q].next_z;
                            q_size -= 1;
                        }
                        if tail != NIL {
                            self.nodes[tail].next_z = e;
                        } else {
                            list = e;
                        }
                        self.nodes[e].prev_z = tail;
                        tail = e;
                    }
                    p = q;
                }
                self.nodes[tail].next_z = NIL;
                in_size *= 2;
                if num_merges <= 1 {
                    break;
                }
            }
            list
        }

        /// `getLeftmost`: a ring's leftmost node (the lowest of those).
        fn get_leftmost(&self, start: usize) -> usize {
            let mut p = start;
            let mut leftmost = start;
            loop {
                if self.x(p) < self.x(leftmost) || (self.x(p) == self.x(leftmost) && self.y(p) < self.y(leftmost)) {
                    leftmost = p;
                }
                p = self.next(p);
                if p == start {
                    break;
                }
            }
            leftmost
        }

        /// `isValidDiagonal`: whether the diagonal between two nodes lies inside the polygon.
        fn is_valid_diagonal(&self, a: usize, b: usize) -> bool {
            let bi = self.nodes[b].i;
            self.nodes[self.next(a)].i != bi
                && self.nodes[self.prev(a)].i != bi
                && !self.intersects_polygon(a, b) // crosses no other edge
                && ((self.locally_inside(a, b) && self.locally_inside(b, a) && self.middle_inside(a, b) // locally visible
                    && (truthy(self.area(self.prev(a), a, self.prev(b))) || truthy(self.area(a, self.prev(b), b)))) // no opposite-facing sectors
                    || (self.equals(a, b) && self.area(self.prev(a), a, self.next(a)) > 0.0 && self.area(self.prev(b), b, self.next(b)) > 0.0)) // the zero-length case
        }

        /// `intersects(p1, q1, p2, q2)`: whether two segments cross.
        fn intersects(&self, p1: usize, q1: usize, p2: usize, q2: usize) -> bool {
            let o1 = sign(self.area(p1, q1, p2));
            let o2 = sign(self.area(p1, q1, q2));
            let o3 = sign(self.area(p2, q2, p1));
            let o4 = sign(self.area(p2, q2, q1));
            if o1 != o2 && o3 != o4 {
                return true; // the general case
            }
            // collinear, and an end on the other segment
            (o1 == 0 && self.on_segment(p1, p2, q1))
                || (o2 == 0 && self.on_segment(p1, q2, q1))
                || (o3 == 0 && self.on_segment(p2, p1, q2))
                || (o4 == 0 && self.on_segment(p2, q1, q2))
        }

        /// `onSegment(p, q, r)`: for collinear points, whether `q` lies on the segment `pr`.
        fn on_segment(&self, p: usize, q: usize, r: usize) -> bool {
            let (p, q, r) = (&self.nodes[p], &self.nodes[q], &self.nodes[r]);
            q.x <= js_max(p.x, r.x) && q.x >= js_min(p.x, r.x) && q.y <= js_max(p.y, r.y) && q.y >= js_min(p.y, r.y)
        }

        /// `intersectsPolygon`: whether a diagonal crosses any of the polygon's edges.
        fn intersects_polygon(&self, a: usize, b: usize) -> bool {
            let (ai, bi) = (self.nodes[a].i, self.nodes[b].i);
            let mut p = a;
            loop {
                let next = self.next(p);
                let (pi, ni) = (self.nodes[p].i, self.nodes[next].i);
                if pi != ai && ni != ai && pi != bi && ni != bi && self.intersects(p, next, a, b) {
                    return true;
                }
                p = next;
                if p == a {
                    break;
                }
            }
            false
        }

        /// `locallyInside`: whether a diagonal is locally inside the polygon.
        fn locally_inside(&self, a: usize, b: usize) -> bool {
            if self.area(self.prev(a), a, self.next(a)) < 0.0 {
                self.area(a, b, self.next(a)) >= 0.0 && self.area(a, self.prev(a), b) >= 0.0
            } else {
                self.area(a, b, self.prev(a)) < 0.0 || self.area(a, self.next(a), b) < 0.0
            }
        }

        /// `middleInside`: whether the middle of a diagonal is inside the polygon.
        fn middle_inside(&self, a: usize, b: usize) -> bool {
            let mut p = a;
            let mut inside = false;
            let px = (self.x(a) + self.x(b)) / 2.0;
            let py = (self.y(a) + self.y(b)) / 2.0;
            loop {
                let next = self.next(p);
                let (x, y, nx, ny) = (self.x(p), self.y(p), self.x(next), self.y(next));
                if (y > py) != (ny > py) && ny != y && px < (nx - x) * (py - y) / (ny - y) + x {
                    inside = !inside;
                }
                p = next;
                if p == a {
                    break;
                }
            }
            inside
        }

        /// `splitPolygon`: links two vertices by a bridge; for two of one ring it splits the
        /// polygon in two, for an outer one and a hole's it merges them into one ring.
        fn split_polygon(&mut self, a: usize, b: usize) -> usize {
            let (ai, ax, ay) = (self.nodes[a].i, self.x(a), self.y(a));
            let (bi, bx, by) = (self.nodes[b].i, self.x(b), self.y(b));
            let a2 = self.new_node(ai, ax, ay);
            let b2 = self.new_node(bi, bx, by);
            let an = self.next(a);
            let bp = self.prev(b);
            self.nodes[a].next = b;
            self.nodes[b].prev = a;
            self.nodes[a2].next = an;
            self.nodes[an].prev = a2;
            self.nodes[b2].next = a2;
            self.nodes[a2].prev = b2;
            self.nodes[bp].next = b2;
            self.nodes[b2].prev = bp;
            b2
        }
    }

    /// A triangle's bbox, `(x0, y0, x1, y1)`, computed as earcut does.
    fn bbox(ax: f64, bx: f64, cx: f64, ay: f64, by: f64, cy: f64) -> (f64, f64, f64, f64) {
        let x0 = if ax < bx { if ax < cx { ax } else { cx } } else if bx < cx { bx } else { cx };
        let y0 = if ay < by { if ay < cy { ay } else { cy } } else if by < cy { by } else { cy };
        let x1 = if ax > bx { if ax > cx { ax } else { cx } } else if bx > cx { bx } else { cx };
        let y1 = if ay > by { if ay > cy { ay } else { cy } } else if by > cy { by } else { cy };
        (x0, y0, x1, y1)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Two parts of the 5° cut of the z0 tile's water whose holes (land) touch the part's
        /// outline along a grid line: earcut.js 2.2.4's triangles, exactly (`earcutr` 0.5 covers
        /// too much of both).
        #[test]
        fn holes_touching_the_outline_triangulate_as_earcut_js_does() {
            let cell = [
                -15.0, 75.0, -20.0, 75.0, -20.0, 80.0, -15.0, 80.0, -20.0, 80.0, -20.0, 75.21898243165758, -19.599609375,
                75.25305660483545, -19.86328125, 76.10079606754579, -20.0, 76.1391741748791, -20.0, 76.94146766973962,
                -18.45703125, 76.98014914976216, -19.6875, 77.63654163009747, -19.6875, 78.75065903068668, -17.940747155567326, 80.0,
            ];
            assert_eq!(
                earcut(&cell, &[4], 2),
                [0, 3, 13, 13, 12, 11, 10, 9, 8, 6, 5, 1, 13, 11, 10, 10, 8, 7, 6, 1, 0, 0, 13, 10, 10, 7, 6, 6, 0, 10]
            );
            let cell = [
                -25.0, 80.0, -30.0, 80.0, -30.0, 85.0, -25.0, 85.0, -27.0703125, 83.52016238353204, -30.0, 83.56692972324694, -30.0,
                80.0, -25.0, 80.0, -25.0, 81.8008335117073, -27.861328125, 82.13042660886293, -30.0, 82.06408871700589, -30.0,
                82.23156156448887, -25.0, 82.3083878077587, -25.0, 83.25870547036925, -27.0703125, 83.52016238353204,
            ];
            assert_eq!(earcut(&cell, &[4], 2), [3, 2, 5, 12, 11, 10, 3, 5, 14, 12, 10, 9, 3, 14, 13, 12, 9, 8]);
        }

        #[test]
        fn simple_shapes_and_degenerate_input() {
            assert_eq!(earcut(&[0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0], &[], 2), [2, 3, 0, 0, 1, 2]);
            // a hole in a square
            let t = earcut(&[0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0, 2.0, 2.0, 2.0, 8.0, 8.0, 8.0, 8.0, 2.0], &[4], 2);
            assert_eq!(t.len(), 8 * 3);
            // too few points, and an empty hole
            assert!(earcut(&[0.0, 0.0, 1.0, 1.0], &[], 2).is_empty());
            assert!(earcut(&[], &[], 2).is_empty());
            assert_eq!(earcut(&[0.0, 0.0, 10.0, 0.0, 10.0, 10.0], &[3], 2).len(), 3);
        }
    }

}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;
    use crate::camera::world_to_lng_lat;
    use crate::tiles::VECTOR_TILES;

    // ---------------------------------------------------------- a small tile encoder

    fn varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push((v as u8) | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    fn bytes_field(out: &mut Vec<u8>, field: u64, b: &[u8]) {
        varint(out, (field << 3) | 2);
        varint(out, b.len() as u64);
        out.extend_from_slice(b);
    }

    fn uint_field(out: &mut Vec<u8>, field: u64, v: u64) {
        varint(out, field << 3);
        varint(out, v);
    }

    fn packed(out: &mut Vec<u8>, field: u64, values: &[u64]) {
        let mut body = Vec::new();
        for &v in values {
            varint(&mut body, v);
        }
        bytes_field(out, field, &body);
    }

    const fn zigzag(v: i64) -> u64 {
        ((v << 1) ^ (v >> 63)) as u64
    }

    /// A tag value.
    #[derive(Clone, Copy)]
    enum V {
        S(&'static str),
        U(u64),
        B(bool),
    }

    /// A feature: its geometry type, tags and parts (absolute points, closed for polygons).
    struct F {
        kind: u64,
        tags: Vec<(&'static str, V)>,
        parts: Vec<Vec<(i64, i64)>>,
    }

    fn polygon(tags: &[(&'static str, V)], rings: &[&[(i64, i64)]]) -> F {
        F { kind: 3, tags: tags.to_vec(), parts: rings.iter().map(|r| r.to_vec()).collect() }
    }

    fn line(tags: &[(&'static str, V)], parts: &[&[(i64, i64)]]) -> F {
        F { kind: 2, tags: tags.to_vec(), parts: parts.iter().map(|r| r.to_vec()).collect() }
    }

    /// A tile of `layers` (name, features), extent 4096.
    fn encode(layers: &[(&str, Vec<F>)]) -> Vec<u8> {
        let mut tile = Vec::new();
        for (name, features) in layers {
            let mut keys: Vec<&str> = Vec::new();
            let mut values: Vec<Vec<u8>> = Vec::new();
            let mut layer = Vec::new();
            uint_field(&mut layer, 15, 2);
            bytes_field(&mut layer, 1, name.as_bytes());
            for f in features {
                let mut tags = Vec::new();
                for &(k, v) in &f.tags {
                    let ki = keys.iter().position(|&x| x == k).unwrap_or_else(|| {
                        keys.push(k);
                        keys.len() - 1
                    });
                    let mut value = Vec::new();
                    match v {
                        V::S(s) => bytes_field(&mut value, 1, s.as_bytes()),
                        V::U(n) => uint_field(&mut value, 5, n),
                        V::B(b) => uint_field(&mut value, 7, u64::from(b)),
                    }
                    values.push(value);
                    tags.extend([ki as u64, values.len() as u64 - 1]);
                }
                let (mut cx, mut cy) = (0, 0);
                let mut geometry = Vec::new();
                for part in &f.parts {
                    for (k, &(x, y)) in part.iter().enumerate() {
                        if k == 0 {
                            geometry.push((1 << 3) | 1);
                        } else if k == 1 {
                            geometry.push(((part.len() as u64 - 1) << 3) | 2);
                        }
                        geometry.extend([zigzag(x - cx), zigzag(y - cy)]);
                        (cx, cy) = (x, y);
                    }
                    if f.kind == 3 {
                        geometry.push((1 << 3) | 7);
                    }
                }
                let mut feature = Vec::new();
                packed(&mut feature, 2, &tags);
                uint_field(&mut feature, 3, f.kind);
                packed(&mut feature, 4, &geometry);
                bytes_field(&mut layer, 2, &feature);
            }
            for k in &keys {
                bytes_field(&mut layer, 3, k.as_bytes());
            }
            for v in &values {
                bytes_field(&mut layer, 4, v);
            }
            uint_field(&mut layer, 5, 4096);
            bytes_field(&mut tile, 3, &layer);
        }
        tile
    }

    fn never() -> bool {
        false
    }

    fn run(tile: TileIndex, bytes: &[u8]) -> VectorTile {
        tessellate(tile, bytes, &never).expect("not cancelled")
    }

    /// A square ring, clockwise with y down (an outline), `(x, y)` its north-west corner.
    fn square(x: i64, y: i64, side: i64) -> Vec<(i64, i64)> {
        vec![(x, y), (x + side, y), (x + side, y + side), (x, y + side)]
    }

    /// A ring the other way round (a hole).
    fn hole(x: i64, y: i64, side: i64) -> Vec<(i64, i64)> {
        let mut r = square(x, y, side);
        r.reverse();
        r
    }

    /// The tile's triangles' area, each counted positive, in tile units squared.
    fn fill_area(t: &VectorTile, color: Rgba) -> f64 {
        t.fill_indices
            .chunks_exact(3)
            .filter(|tri| t.fill_vertices[tri[0] as usize].color == color)
            .map(|tri| {
                let p = |k: usize| t.fill_vertices[tri[k] as usize].position.map(f64::from);
                let (a, b, c) = (p(0), p(1), p(2));
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0
            })
            .sum()
    }

    // ---------------------------------------------------------- the tables

    fn one(layer: &'static str, f: F) -> (Rgba, Rgba, f64) {
        let bytes = encode(&[(layer, vec![f])]);
        let l = mvt::layer(&bytes, layer).expect("layer");
        let feature = l.feature(0).expect("feature");
        (vector_fill(layer, &feature), vector_line(layer, &feature), vector_line_width(layer, &feature))
    }

    #[test]
    fn colours_and_widths_follow_todays_tables() {
        let ring: &[(i64, i64)] = &[(0, 0), (10, 0), (10, 10)];
        let path: &[(i64, i64)] = &[(0, 0), (10, 10)];
        let class = |c: &'static str| vec![("class", V::S(c))];
        assert_eq!(one("water", polygon(&class("lake"), &[ring])).0, WATER);
        assert_eq!(one("water", polygon(&[], &[ring])).0, WATER);
        assert_eq!(one("aeroway", polygon(&class("runway"), &[ring])).0, RUNWAY);
        assert_eq!(one("aeroway", polygon(&class("taxiway"), &[ring])).0, TAXIWAY);
        for c in ["apron", "aerodrome", "helipad", "gate"] {
            assert_eq!(one("aeroway", polygon(&class(c), &[ring])).0, APRON, "{c}");
        }
        assert_eq!(one("transportation", polygon(&class("pier"), &[ring])).0, NONE);
        assert_eq!(one("boundary", polygon(&[], &[ring])).0, NONE);
        // lines
        let boundary = |tags: Vec<(&'static str, V)>| {
            let (_, color, width) = one("boundary", line(&tags, &[path]));
            (color, width)
        };
        assert_eq!(boundary(vec![("admin_level", V::U(2)), ("maritime", V::U(0))]), (BORDER, 1.0));
        assert_eq!(boundary(vec![("admin_level", V::U(1))]), (BORDER, 1.0));
        assert_eq!(boundary(vec![("admin_level", V::U(4)), ("maritime", V::U(0))]), (BORDER_STATE, 0.6));
        assert_eq!(boundary(vec![("admin_level", V::U(2)), ("maritime", V::U(1))]), (NONE, 1.0));
        assert_eq!(boundary(vec![("admin_level", V::U(4)), ("maritime", V::U(1))]), (NONE, 0.6));
        for (c, color, width) in [
            ("motorway", ROAD_MAJOR, 1.4),
            ("trunk", ROAD_MAJOR, 1.4),
            ("primary", ROAD_MAJOR, 1.0),
            ("secondary", ROAD_MINOR, 0.6),
            ("tertiary", ROAD_MINOR, 0.6),
            ("minor", ROAD_MINOR, 0.6),
            ("service", ROAD_MINOR, 0.6),
            ("rail", NONE, 0.6),
            ("path", NONE, 0.6),
            ("ferry", NONE, 0.6),
            ("motorway_construction", NONE, 0.6),
        ] {
            let (_, got_color, got_width) = one("transportation", line(&class(c), &[path]));
            assert_eq!((got_color, got_width), (color, width), "{c}");
        }
        assert_eq!(one("transportation", line(&[], &[path])).1, NONE);
        let (_, color, width) = one("aeroway", line(&class("runway"), &[path]));
        assert_eq!((color, width), (RUNWAY, RUNWAY_PX));
        let (_, color, width) = one("aeroway", line(&class("taxiway"), &[path]));
        assert_eq!((color, width), (TAXIWAY, 1.5));
        let (_, color, width) = one("aeroway", line(&class("gate"), &[path]));
        assert_eq!((color, width), (NONE, 1.5));
        let (_, color, width) = one("water", line(&class("river"), &[path]));
        assert_eq!((color, width), (NONE, 0.0));
    }

    #[test]
    fn properties_read_as_javascript_reads_them() {
        let path: &[(i64, i64)] = &[(0, 0), (10, 10)];
        let boundary = |tags: Vec<(&'static str, V)>| {
            let (_, color, width) = one("boundary", line(&tags, &[path]));
            (color, width)
        };
        // Number(undefined) is NaN, and NaN <= 2 is false
        assert_eq!(boundary(vec![]), (BORDER_STATE, 0.6));
        // Number("2") is 2, Number(" 2 ") too; Number("") is 0; Number("two") is NaN
        assert_eq!(boundary(vec![("admin_level", V::S("2"))]), (BORDER, 1.0));
        assert_eq!(boundary(vec![("admin_level", V::S(" 2 "))]), (BORDER, 1.0));
        assert_eq!(boundary(vec![("admin_level", V::S(""))]), (BORDER, 1.0));
        assert_eq!(boundary(vec![("admin_level", V::S("two"))]), (BORDER_STATE, 0.6));
        assert_eq!(boundary(vec![("admin_level", V::B(true))]), (BORDER, 1.0));
        // a maritime of "0" is a non-empty string, so truthy; false and 0 are not
        assert_eq!(boundary(vec![("admin_level", V::U(2)), ("maritime", V::S("0"))]).0, NONE);
        assert_eq!(boundary(vec![("admin_level", V::U(2)), ("maritime", V::S(""))]).0, BORDER);
        assert_eq!(boundary(vec![("admin_level", V::U(2)), ("maritime", V::B(false))]).0, BORDER);
        assert_eq!(boundary(vec![("admin_level", V::U(2)), ("maritime", V::B(true))]).0, NONE);
        // a class that is not a string matches no name
        let (_, color, _) = one("transportation", line(&[("class", V::U(1))], &[path]));
        assert_eq!(color, NONE);
    }

    #[test]
    fn string_to_number_follows_ecmascript() {
        for (s, n) in [
            ("2", 2.0),
            ("  -3.5e1\n", -35.0),
            ("+.5", 0.5),
            ("5.", 5.0),
            ("0x1F", 31.0),
            ("0b101", 5.0),
            ("0o17", 15.0),
            ("", 0.0),
            ("\t ", 0.0),
            ("Infinity", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
            ("1e3", 1000.0),
        ] {
            assert_eq!(string_to_number(s), n, "{s:?}");
        }
        for s in ["two", ".", "1e", "inf", "nan", "NaN", "infinity", "-0x10", "0x", "1_000", "1,5", "e5", "2 3"] {
            assert!(string_to_number(s).is_nan(), "{s:?}");
        }
    }

    // ---------------------------------------------------------- rings and order

    #[test]
    fn rings_are_grouped_as_loaders_gl_groups_them() {
        let to_line = |r: Vec<(i64, i64)>| -> Line { r.into_iter().map(|(x, y)| (x as f64, y as f64)).collect() };
        let a = to_line(square(0, 0, 100));
        let a_hole = to_line(hole(10, 10, 10));
        let b = to_line(square(200, 0, 100));
        let flat = to_line(vec![(0, 0), (5, 0), (10, 0)]);
        let rings = vec![a.clone(), a_hole.clone(), flat.clone(), b.clone()];
        let polygons = classify_rings(&rings);
        assert_eq!(polygons, vec![vec![&a, &a_hole], vec![&b]]);
        // the first ring with an area decides which turn is an outline: here the "holes"
        let rings = vec![flat.clone(), a_hole.clone(), a.clone(), b.clone()];
        assert_eq!(classify_rings(&rings), vec![vec![&a_hole, &a, &b]]);
        // one ring is a polygon whatever its area; no rings, no polygon
        assert_eq!(classify_rings(std::slice::from_ref(&flat)), vec![vec![&flat]]);
        assert!(classify_rings(&[]).is_empty());
    }

    #[test]
    fn a_polygon_with_a_hole_fills_around_it() {
        let bytes = encode(&[("water", vec![polygon(&[], &[&square(0, 0, 2048), &hole(512, 512, 1024)])])]);
        // z13: the flat map's tile, nothing cut
        let t = run(TileIndex::new(4000, 2000, 13), &bytes);
        assert!((fill_area(&t, WATER) - (0.25 - 0.0625)).abs() < 1e-9, "{}", fill_area(&t, WATER));
        assert!(t.fill_vertices.iter().all(|v| v.color == WATER && v.position[2] == 0.0));
        assert_eq!(t.fill_vertices[0].position, [0.0, 0.0, 0.0]);
        // the same on the globe's tile at z9 (one cell), and on a z2 tile, cut on the grid
        for tile in [TileIndex::new(281, 149, 9), TileIndex::new(2, 1, 2)] {
            let t = run(tile, &bytes);
            assert!((fill_area(&t, WATER) - 0.1875).abs() < 1e-6, "{tile:?}: {}", fill_area(&t, WATER));
        }
    }

    #[test]
    fn fills_and_lines_keep_feature_order_in_the_order_of_the_source_layers() {
        let path: &[(i64, i64)] = &[(0, 0), (4096, 4096)];
        // the tile stores aeroway, transportation, boundary, water (and a layer not read)
        let bytes = encode(&[
            ("aeroway", vec![polygon(&[("class", V::S("apron"))], &[&square(0, 0, 100)]), line(&[("class", V::S("taxiway"))], &[path])]),
            ("transportation", vec![line(&[("class", V::S("motorway"))], &[path]), line(&[("class", V::S("service"))], &[path])]),
            ("landcover", vec![polygon(&[("class", V::S("wood"))], &[&square(0, 0, 4096)])]),
            ("boundary", vec![line(&[("admin_level", V::U(2))], &[path])]),
            ("water", vec![polygon(&[], &[&square(0, 0, 4096)])]),
        ]);
        let t = run(TileIndex::new(4000, 2000, 13), &bytes);
        let fills: Vec<Rgba> = t.fill_indices.chunks_exact(3).map(|tri| t.fill_vertices[tri[0] as usize].color).collect();
        assert_eq!(fills, [WATER, WATER, APRON, APRON]);
        let lines: Vec<(Rgba, f32)> = t.lines.iter().map(|s| (s.color, s.width)).collect();
        assert_eq!(lines, [(BORDER, 1.0), (ROAD_MAJOR, 1.4), (ROAD_MINOR, 0.6), (TAXIWAY, 1.5)]);
        // a path in tile units, capped at both ends
        assert_eq!(t.lines[0].start, [0.0, 0.0, 0.0]);
        assert_eq!(t.lines[0].end, [1.0, 1.0, 0.0]);
        assert_eq!(t.lines[0].types, crate::geom::path::START_CAP | crate::geom::path::END_CAP);
    }

    #[test]
    fn points_and_transparent_features_add_nothing() {
        let path: &[(i64, i64)] = &[(0, 0), (100, 100)];
        let bytes = encode(&[
            ("boundary", vec![line(&[("admin_level", V::U(2)), ("maritime", V::U(1))], &[path])]),
            ("transportation", vec![line(&[("class", V::S("rail"))], &[path]), polygon(&[("class", V::S("bridge"))], &[&square(0, 0, 50)])]),
            ("aeroway", vec![F { kind: 1, tags: vec![("class", V::S("gate"))], parts: vec![vec![(5, 5)]] }]),
        ]);
        let t = run(TileIndex::new(1, 1, 2), &bytes);
        assert!(t.is_empty(), "{t:?}");
        assert_eq!(t.byte_size(), 0);
    }

    #[test]
    fn globe_paths_close_loops_and_drop_repeats_flat_ones_stay_open() {
        let loop_: &[(i64, i64)] = &[(0, 0), (100, 0), (100, 0), (100, 100), (0, 100), (0, 0)];
        let bytes = encode(&[("transportation", vec![line(&[("class", V::S("minor"))], &[loop_])])]);
        // globe (z9): the repeat is dropped, the loop joined all round (no caps)
        let t = run(TileIndex::new(281, 149, 9), &bytes);
        assert_eq!(t.lines.len(), 4);
        assert!(t.lines.iter().all(|s| s.types == 0));
        // flat (z14): every point kept, capped at both ends
        let t = run(TileIndex::new(9007, 4789, 14), &bytes);
        assert_eq!(t.lines.len(), 5);
        assert_eq!(t.lines[0].types, crate::geom::path::START_CAP);
        assert_eq!(t.lines[4].types, crate::geom::path::END_CAP);
    }

    #[test]
    fn runway_centre_lines_are_named_for_the_true_width_switch() {
        let path: &[(i64, i64)] = &[(0, 0), (100, 0), (200, 50)];
        let runway = || line(&[("class", V::S("runway"))], &[path]);
        let taxiway = || line(&[("class", V::S("taxiway"))], &[path]);
        let bytes = encode(&[
            ("transportation", vec![line(&[("class", V::S("primary"))], &[path])]),
            ("aeroway", vec![runway(), runway(), taxiway(), runway()]),
        ]);
        let t = run(TileIndex::new(9007, 4789, 14), &bytes);
        assert_eq!(t.lines.len(), 10);
        assert_eq!(t.runway_lines, [2..6, 8..10]);
        assert!(t.runway_lines.iter().flat_map(|r| r.clone()).all(|i| t.lines[i as usize].color == RUNWAY));
        assert_eq!(t.lines_without_runways(), [0..2, 6..8]);
        let none = VectorTile { lines: t.lines.clone(), ..VectorTile::default() };
        assert_eq!(none.lines_without_runways(), std::iter::once(0..10).collect::<Vec<_>>());
        let all = VectorTile { lines: t.lines[2..6].to_vec(), runway_lines: std::iter::once(0..4).collect(), ..VectorTile::default() };
        assert!(all.lines_without_runways().is_empty());
    }

    #[test]
    fn a_cancelled_job_stops() {
        let bytes = encode(&[("water", vec![polygon(&[], &[&square(0, 0, 4096)])])]);
        assert_eq!(tessellate(TileIndex::new(0, 0, 0), &bytes, &|| true), Err(Cancelled));
        assert!(tessellate(TileIndex::new(0, 0, 0), &[], &never).expect("empty tile").is_empty());
    }

    #[test]
    fn the_source_stops_at_zoom_14_and_fill_vertices_match_the_fill_pipeline() {
        assert_eq!(VECTOR_TILES.max_zoom, Some(f64::from(MAX_ZOOM)));
        assert_eq!(VECTOR_TILES.tile_size, 512.0);
        assert_eq!(size_of::<FillVertex>(), 16);
        assert_eq!(std::mem::offset_of!(FillVertex, color), 12);
    }

    // ---------------------------------------------------------- the fixture tiles

    fn fixture(z: u32, x: u32, y: u32) -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../map/tests/data/tiles/openfreemap/{z}/{x}/{y}.pbf"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// Per line colour and width: segments, total length in world Mercator units.
    fn line_stats(tile: TileIndex, t: &VectorTile) -> BTreeMap<(Rgba, String), (usize, f64)> {
        let local = LocalMercator::tile(tile);
        let mut stats: BTreeMap<(Rgba, String), (usize, f64)> = BTreeMap::new();
        for s in &t.lines {
            let world = |p: [f32; 3]| local.to_world([f64::from(p[0]), f64::from(p[1])]);
            let e = stats.entry((s.color, format!("{}", s.width))).or_default();
            e.0 += 1;
            e.1 += world(s.start).distance(world(s.end));
        }
        stats
    }

    /// Per fill colour: triangles, and the area they cover in world Mercator units squared.
    fn fill_stats(tile: TileIndex, t: &VectorTile) -> BTreeMap<Rgba, (usize, f64)> {
        let scale = LocalMercator::tile(tile).scale;
        let mut stats: BTreeMap<Rgba, (usize, f64)> = BTreeMap::new();
        for tri in t.fill_indices.chunks_exact(3) {
            let color = t.fill_vertices[tri[0] as usize].color;
            stats.entry(color).or_default().0 += 1;
        }
        for (color, e) in &mut stats {
            e.1 = fill_area(t, *color) * scale * scale;
        }
        stats
    }

    fn close(got: f64, want: f64, rel: f64) -> bool {
        (got - want).abs() <= rel * want.abs().max(1e-12)
    }

    /// What deck.gl 9.4 makes of these tiles as today's app draws them. Computed once (bun, in a
    /// scratch folder) from the fixture tiles with loaders.gl 4.5.2's `parseMVT` as MVTLayer asks
    /// on the globe (`shape: 'geojson'`, `coordinates: 'wgs84'`, the four layers,
    /// `layerProperty: 'layerName'`), deck's `separateGeojsonFeatures`, `Globe.svelte`'s
    /// `vectorFill`, `vectorLine` and `vectorLineWidth` copied verbatim, the features of colour
    /// alpha 0 left out, then deck's own `PathTesselator` (`resolution: 5` up to z12; above, the
    /// flat map's binary paths, unnormalised and open) and `PolygonTesselator` (`resolution: 5`
    /// up to z12): per line colour and width the valid segments of `getPathSegmentIndices` and
    /// their total length in world Mercator units (`lngLatToWorld`), per fill colour the
    /// triangles of `indices` and the sum of their areas in world units squared.
    #[allow(clippy::type_complexity)]
    #[rustfmt::skip]
    const DECK: &[((u32, u32, u32), &[(Rgba, &str, usize, f64)], &[(Rgba, usize, f64)])] = &[
        ((9, 281, 149), &[(BORDER_STATE, "0.6", 214, 1.91688776973616), (ROAD_MAJOR, "1.4", 398, 4.111239979147457), (ROAD_MINOR, "0.6", 1196, 7.336197100988485), (ROAD_MAJOR, "1", 161, 1.2702203450997689)], &[(WATER, 3198, 0.056958973407752356)]),
        ((5, 17, 9), &[(BORDER_STATE, "0.6", 2098, 110.96712771876345), (BORDER, "1", 176, 14.121281461737805), (ROAD_MAJOR, "1.4", 5189, 196.48390211710716)], &[(WATER, 4705, 127.42331724154967)]),
        ((4, 10, 7), &[(BORDER, "1", 41, 40.5650657452685), (BORDER_STATE, "0.6", 68, 34.59819083344161)], &[(WATER, 265, 814.02280842225)]),
        ((2, 2, 1), &[(BORDER, "1", 3484, 1695.0307883799906), (BORDER_STATE, "0.6", 14289, 4578.167842327032)], &[(WATER, 5217, 4088.1108545905026)]),
        ((0, 0, 0), &[(BORDER, "1", 2332, 3067.1153924566424)], &[(WATER, 8955, 173797.02392170933)]),
        ((12, 2251, 1197), &[(ROAD_MINOR, "0.6", 1526, 2.031524471650427), (ROAD_MAJOR, "1.4", 125, 0.4182815282658192), (RUNWAY, "4", 1, 0.08338854929900164), (TAXIWAY, "1.5", 414, 0.4276706355035555)], &[(WATER, 171, 5.3839758038545966e-05), (APRON, 191, 0.0027051512151951285), (RUNWAY, 3, 9.56109724930829e-05)]),
        ((14, 9007, 4789), &[(ROAD_MINOR, "0.6", 278, 0.1686746563640602), (RUNWAY, "4", 1, 0.03276037492391663), (TAXIWAY, "1.5", 472, 0.23234802048599576)], &[(WATER, 30, 2.9067596188038633e-05), (APRON, 74, 0.0010317832639006375), (RUNWAY, 2, 3.737211227430176e-05)]),
    ];

    #[test]
    fn fixture_tiles_tessellate_as_deck_tessellates_them() {
        for &((z, x, y), lines, fills) in DECK {
            let tile = TileIndex::new(x, y, z);
            let t = run(tile, &fixture(z, x, y));
            let got = line_stats(tile, &t);
            let want: BTreeMap<(Rgba, String), (usize, f64)> =
                lines.iter().map(|&(c, w, n, len)| ((c, w.to_string()), (n, len))).collect();
            assert_eq!(got.keys().collect::<Vec<_>>(), want.keys().collect::<Vec<_>>(), "{tile:?} line classes");
            for (key, &(n, len)) in &want {
                let (got_n, got_len) = got[key];
                assert_eq!(got_n, n, "{tile:?} {key:?}: segments");
                assert!(close(got_len, len, 1e-6), "{tile:?} {key:?}: length {got_len} against {len}");
            }
            let got = fill_stats(tile, &t);
            let colors: std::collections::BTreeSet<Rgba> = fills.iter().map(|f| f.0).collect();
            assert_eq!(got.keys().copied().collect::<std::collections::BTreeSet<_>>(), colors, "{tile:?} fill colours");
            for &(color, triangles, area) in fills {
                let (got_n, got_area) = got[&color];
                assert!(close(got_area, area, 1e-6), "{tile:?} {color:?}: area {got_area} against {area}");
                // the globe's tiles get deck's own triangles (same cut, same earcut); the flat
                // map's are cut from the tile's units, where deck's binary tiles use math.gl's
                // earcut on loaders.gl's rings
                if z <= GLOBE_MAX_Z {
                    assert_eq!(got_n, triangles, "{tile:?} {color:?}: triangles");
                }
            }
        }
    }

    /// On the globe's tiles the fills are deck's own triangles, index for index: the same rows,
    /// the same parts of the same cut, earcut 2.2.4's triangles of each. The index list's
    /// length and hash (`h = h·31 + index`, 32 bits) of deck's PolygonTesselator over the
    /// visible polygons, computed with the tiles of [`DECK`].
    #[test]
    fn globe_fills_are_decks_triangles_index_for_index() {
        for ((z, x, y), count, hash) in [
            ((9, 281, 149), 9594, 2913703585u32),
            ((5, 17, 9), 14115, 249200192),
            ((4, 10, 7), 795, 1732178495),
            ((2, 2, 1), 15651, 1717439459),
            ((0, 0, 0), 26865, 2677402096),
            ((12, 2251, 1197), 1095, 1778834326),
        ] {
            let t = run(TileIndex::new(x, y, z), &fixture(z, x, y));
            assert_eq!(t.fill_indices.len(), count, "{z}/{x}/{y}");
            let got = t.fill_indices.iter().fold(0u32, |h, &i| h.wrapping_mul(31).wrapping_add(i));
            assert_eq!(got, hash, "{z}/{x}/{y}");
        }
    }

    /// Item B1's acceptance: a `water` polygon becomes triangles of `[30,30,30,255]`; a motorway
    /// becomes 1.4 px `[88,88,88,255]` segments; maritime borders draw nothing.
    #[test]
    fn water_motorways_and_maritime_borders_at_stockholm() {
        // z9 at V3: lakes and the Baltic's edge, the E4 and E18 motorways
        let tile = TileIndex::new(281, 149, 9);
        let bytes = fixture(9, 281, 149);
        let t = run(tile, &bytes);
        let water = mvt::layer(&bytes, "water").expect("water layer");
        assert!(water.features().any(|f| f.kind == GeomType::Polygon));
        assert!(!t.fill_indices.is_empty());
        assert!(t.fill_vertices.iter().all(|v| v.color == WATER), "only water fills at z9");
        let motorways: Vec<&PathSegment> = t.lines.iter().filter(|s| s.width == 1.4).collect();
        assert!(!motorways.is_empty());
        assert!(motorways.iter().all(|s| s.color == ROAD_MAJOR));
        // every motorway and trunk of the tile is drawn, in 1.4 px road grey
        let transportation = mvt::layer(&bytes, "transportation").expect("transportation layer");
        let major: usize = transportation
            .features()
            .filter(|f| matches!(f.text("class"), Some("motorway" | "trunk")))
            .flat_map(|f| f.lines())
            .map(|l| l.len() - 1)
            .sum();
        assert_eq!(motorways.len(), major);
        // the tile's one boundary south-east of Stockholm is maritime: nothing at all
        for (z, x, y) in [(9, 282, 151), (9, 283, 149), (6, 35, 9)] {
            let bytes = fixture(z, x, y);
            let boundary = mvt::layer(&bytes, "boundary").expect("boundary layer");
            assert!(boundary.features().all(|f| f.get("maritime") == Some(&Value::Num(1.0))));
            let t = run(TileIndex::new(x, y, z), &bytes);
            assert!(t.lines.iter().all(|s| s.color != BORDER && s.color != BORDER_STATE), "{z}/{x}/{y}");
        }
        // nothing transparent is ever emitted
        for &((z, x, y), _, _) in DECK {
            let t = run(TileIndex::new(x, y, z), &fixture(z, x, y));
            assert!(t.lines.iter().all(|s| s.color[3] == 255));
            assert!(t.fill_vertices.iter().all(|v| v.color[3] == 255));
        }
    }

    /// On the globe's tiles every triangle and every segment lies in one 5° cell (math.gl's cut),
    /// the cut points on the grid lines; and nothing reaches outside the tile's buffer.
    #[test]
    fn globe_tiles_are_cut_on_the_five_degree_grid() {
        for (z, x, y) in [(0, 0, 0), (2, 2, 1), (5, 17, 9), (4, 10, 7)] {
            let tile = TileIndex::new(x, y, z);
            let t = run(tile, &fixture(z, x, y));
            let local = LocalMercator::tile(tile);
            let lng_lat = |p: [f32; 3]| world_to_lng_lat(local.to_world([f64::from(p[0]), f64::from(p[1])]).to_array());
            // f32 tile units over a z0 tile: about 3e-5 degrees
            let tol = 1e-4 * 2f64.powi(-(z as i32)) * 360.0;
            let in_one_cell = |points: &[[f64; 2]]| {
                let mid = points.iter().fold([0.0, 0.0], |m, p| [m[0] + p[0] / points.len() as f64, m[1] + p[1] / points.len() as f64]);
                let [left, bottom, right, top] = grid_cell(mid);
                points.iter().all(|p| p[0] >= left - tol && p[0] <= right + tol && p[1] >= bottom - tol && p[1] <= top + tol)
            };
            for tri in t.fill_indices.chunks_exact(3) {
                let points: Vec<[f64; 2]> = tri.iter().map(|&i| lng_lat(t.fill_vertices[i as usize].position)).collect();
                assert!(in_one_cell(&points), "{tile:?}: triangle {points:?}");
            }
            for s in &t.lines {
                let points = [lng_lat(s.start), lng_lat(s.end)];
                assert!(in_one_cell(&points), "{tile:?}: segment {points:?}");
            }
            assert!(t.fill_vertices.iter().all(|v| v.position[0] > -0.1 && v.position[0] < 1.1 && v.position[1] > -0.1 && v.position[1] < 1.1));
        }
    }
}
