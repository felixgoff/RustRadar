//! The realistic map's ground tiles, the CPU side (item R2; doc 04 §6.1–6.5, §6.9;
//! `realistic-ground.ts`'s `GroundTile`, `RG:646-778`, painted by `areaPaint` and `lineOf`, here
//! [`crate::ground`]): one vector tile's areas, its lines' edging and its lines, as the ground
//! shader (`rustradar_map`'s `shaders/ground.wgsl`) draws them, in the tile's frame
//! ([`LocalMercator::tile`]: `u` east, `v` south, metres up), made on the worker pool.
//!
//! What today's app does, and so this module ([`tessellate`]):
//! - **The layers read**: OpenMapTiles' [`GROUND_LAYERS`] (`landuse`, `landcover`, `aeroway`,
//!   `water`, `waterway`, `transportation`), **in that order** (loaders.gl's `parseMVT` walks its
//!   `layers` option in the order given; for each name the last non-empty layer of that name,
//!   `rustradar_core::mvt::layers_named`), or Streets GL's [`SGL_GROUND_LAYERS`] (the city
//!   ground's z16 tiles, item R3). Each feature's properties are its tags with `layerName`, as
//!   the MVT loader hands them to the accessors.
//! - **Three passes** (`GroundTile.renderLayers`): the tile's polygons filled (`-areas`, each in
//!   its [`area_paint`]), then each line's edging (`-edges`: [`Line::edge_width`] metres wide in
//!   its `edge_paint`), then each line (`-lines`: [`Line::width`] metres in its `paint`), so no
//!   road's pavement covers another's asphalt where they meet. [`GroundTile`] holds them in that
//!   order, each in feature order. Widths are metres; the layer draws them at least
//!   [`crate::ground::LINE_MIN_PIXELS`] wide (0 for Streets GL's), with round joints and caps.
//! - **Paints as explicit attributes** (doc 04 §6.1, §16.1): deck carries a feature's surface in
//!   its colour's alpha (255 less the id) and reads it back as `floor((1 − a)·255 + 0.5)`. Here a
//!   vertex and a line segment carry the tint (sRGB, `[r, g, b]`, or a mapped field's crop, rows
//!   and shade, `crate::ground`'s `fieldPaint`) and the surface id as a whole number in its fourth
//!   byte ([`explicit`]); the shader reads them as integers (`Uint8x4`). What deck draws
//!   transparent ([`NONE`]: an area of no paint, a line or edging of none; the ground shader gives
//!   an id of 42 or more alpha 0) is left out.
//! - **The globe's tiles** (`flat` false: deck's GeoJSON, `ground-globe`): each polygon as deck's
//!   `Polygon.normalize` makes it (rings closed, the outline clockwise, holes counter-clockwise
//!   with north up), cut on the 5° grid of `GlobeView({ resolution: 5 })` and each part cut into
//!   triangles by earcut 2.2.4 (`crate::geom::earcut`); each line in longitude and latitude cut on
//!   the grid (or, within one cell, only its repeated points dropped), a line whose last point is
//!   its first a joined loop. As `crate::tess::vector` cuts the dark map's tiles.
//! - **The flat map's tiles** (`flat` true: loaders.gl's binary tiles, `ground-city`'s, item R3):
//!   polygons as the binary tiles hold them (`crate::tess::buildings::binary_polygons`),
//!   triangulated in tile units; lines as they are, open (`_pathType: 'open'`), each part its own
//!   path; and the tile's bridges split off (`bridgeParts` with `deckOf`,
//!   `crate::ground_rails::deck_of`: OpenMapTiles' `brunnel` decks, OpenRailwayMap's tracks at
//!   their rail decks) into four more passes after the three: [`GroundTile::bridge_shade`]
//!   (`-bridge-shade`: every bridge flat in [`BRIDGE_SHADE`], `max(width, edge width) + 2` m wide,
//!   unlit, a plain RGBA colour, not a paint), [`GroundTile::bridge_edges`] and
//!   [`GroundTile::bridge_lines`] (raised to their decks, metres up), and
//!   [`GroundTile::bridge_decks`] (`-bridge-decks`: the raised bridges in [`NONE`] with depth
//!   written, a railway's 0 m wide, else as wide as its edging; plain RGBA). On the globe nothing
//!   is raised and these stay empty. MVTLayer's tile clip on the flat map is the layer's.
//! - Lines are drawn with deck's PathLayer; its `geometry.uv` (across the line −1 to 1, along
//!   each segment from its start in half widths), which the railway's and the pier's textures are
//!   laid by, is worked out by the shader from the segment: nothing more is stored here.
//!
//! The survey's road widths (`withSurvey`, Trafikverket's NVDB on the flat map, item R9) come in
//! through [`tessellate_surveyed`]: the flat map's lines get `nvdbWidth` and `nvdbGravel` columns
//! before [`crate::ground::line_of`] reads them (`rustradar_map`'s `layers::ground::roads` makes
//! them); [`tessellate`] reads the tile's own.
//!
//! Deviations, none visible:
//! - a feature of unknown geometry type is skipped (loaders.gl throws and loses the tile), points
//!   are not drawn (`getPointRadius: 0`);
//! - the properties are read as GeoJSON features have them on both projections (loaders.gl's
//!   binary tiles read a numeric tag a feature lacks as 0; no rule here tells the two apart:
//!   `crate::ground`'s rules read text, and `layer`, which a bridge's deck reads, is clamped to at
//!   least 1);
//! - triangles keep earcut's turn: draw them unculled, as deck does (`SURFACE_NO_CULL`).

use std::f64::consts::PI;

use bytemuck::{Pod, Zeroable};
use glam::DVec2;
use rustradar_core::mvt::{self, GeomType, Line as Points};
use serde_json::Value;

use crate::bridges::{self, Lines, Properties, bridge_parts};
use crate::camera::lng_lat_to_world;
use crate::geom::densify::GLOBE_RESOLUTION_DEG;
use crate::geom::earcut::earcut;
use crate::geom::grid_cut::{GridOptions, PolylineGridOptions, cut_polygon_by_grid, cut_polyline_by_grid};
use crate::geom::js_max;
use crate::geom::path::{LocalMercator, Paint, PathBuilder, PathSegment, PathType};
pub use crate::ground::{BRIDGE_SHADE, GROUND_LAYERS, NONE, Rgba, SGL_GROUND_LAYERS, SURFACE_COUNT};
use crate::ground::{Line, area_paint, line_of, on_rails};
use crate::ground_rails::deck_of;
use crate::tess::buildings::binary_polygons;
use crate::tess::vector::classify_rings;
pub use crate::tess::vector::Cancelled;
use crate::tiles::TileIndex;

/// One vertex of a ground area: `(u, v, h)` in the tile's frame, then the tint (sRGB) and the
/// surface id ([`explicit`]). 16 bytes, `#[repr(C)]`: the layout of `rustradar_map`'s ground
/// vertex buffer (position `Float32x3`, tint and surface `Uint8x4`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GroundVertex {
    pub position: [f32; 3],
    /// RGB of the paint (a mapped field: its rows' lie, its crop and its shade).
    pub tint: [u8; 3],
    /// The surface's id (`crate::ground`'s table; [`SURFACE_COUNT`] or more draws nothing).
    pub surface: u8,
}

/// A paint (RGB tint, alpha 255 less the surface id, as `crate::ground` and deck carry it) as the
/// ground shader's explicit attributes: the tint and the surface id, `[r, g, b, id]`. A line
/// segment's [`PathSegment::color`] holds this in the ground's passes.
pub const fn explicit(paint: Rgba) -> [u8; 4] {
    [paint[0], paint[1], paint[2], 255 - paint[3]]
}

/// One tile of ground, in its passes' order (see the module notes). The segments of `edges`,
/// `lines`, `bridge_edges` and `bridge_lines` carry [`explicit`] paints; `bridge_shade` and
/// `bridge_decks` plain RGBA colours.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroundTile {
    /// `-areas`: the polygons' vertices, in tile units.
    pub vertices: Vec<GroundVertex>,
    /// Three indices into [`Self::vertices`] per triangle, polygon after polygon in feature order.
    pub indices: Vec<u32>,
    /// `-edges`: each line's edging (pavements, shoulders, ballast), widths in metres.
    pub edges: Vec<PathSegment>,
    /// `-lines`: each line, widths in metres.
    pub lines: Vec<PathSegment>,
    /// `-bridge-shade` (flat map): the bridges flat on the ground in [`BRIDGE_SHADE`], unlit.
    pub bridge_shade: Vec<PathSegment>,
    /// `-bridge-edges` (flat map): the bridges' edging, raised.
    pub bridge_edges: Vec<PathSegment>,
    /// `-bridge-lines` (flat map): the bridges, raised.
    pub bridge_lines: Vec<PathSegment>,
    /// `-bridge-decks` (flat map): the raised bridges in [`NONE`], for the depth buffer alone.
    pub bridge_decks: Vec<PathSegment>,
}

impl GroundTile {
    /// The line passes in draw order, with deck's sub-layer id suffixes.
    pub fn line_passes(&self) -> [(&'static str, &[PathSegment]); 6] {
        [
            ("edges", &self.edges),
            ("lines", &self.lines),
            ("bridge-shade", &self.bridge_shade),
            ("bridge-edges", &self.bridge_edges),
            ("bridge-lines", &self.bridge_lines),
            ("bridge-decks", &self.bridge_decks),
        ]
    }

    /// Whether it draws nothing.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty() && self.line_passes().iter().all(|(_, s)| s.is_empty())
    }

    /// The bytes it holds (for the tile cache's budget).
    pub fn byte_size(&self) -> usize {
        size_of_val(self.vertices.as_slice())
            + size_of_val(self.indices.as_slice())
            + self.line_passes().iter().map(|(_, s)| size_of_val(*s)).sum::<usize>()
    }
}

/// The ground of one vector tile (`bytes`, as served, decompressed) at `tile`: the source layers
/// `layers` ([`GROUND_LAYERS`] for OpenMapTiles, [`SGL_GROUND_LAYERS`] for Streets GL), as the
/// globe's layer draws them (`flat` false) or the flat map's (`flat` true, bridges raised). See
/// the module notes. `cancelled` is asked between features; once it says so the work stops.
pub fn tessellate(tile: TileIndex, bytes: &[u8], layers: &[&str], flat: bool, cancelled: &dyn Fn() -> bool) -> Result<GroundTile, Cancelled> {
    tessellate_surveyed(tile, bytes, layers, flat, &|_, _| {}, cancelled)
}

/// [`tessellate`], the flat map's lines first handed to `survey` (`withSurvey`, item R9: the
/// survey's road widths as the numeric columns `nvdbWidth` and `nvdbGravel`, a value a vertex,
/// which each line reads at its first vertex with its feature's tags, as loaders.gl's binary
/// accessors do), before their bridges are split off. The globe's lines are not handed over.
pub fn tessellate_surveyed(
    tile: TileIndex,
    bytes: &[u8],
    layers: &[&str],
    flat: bool,
    survey: &dyn Fn(TileIndex, &mut Lines),
    cancelled: &dyn Fn() -> bool,
) -> Result<GroundTile, Cancelled> {
    let mut b = Builder::new(tile, flat);
    // the flat map's lines, gathered as loaders.gl's binary tile holds them, for the bridges
    let mut binary = Lines { kind: "LineString".into(), size: 2, path_indices: vec![0], ..Lines::default() };
    for layer in mvt::layers_named(bytes, layers) {
        let geo = TileGeo::new(tile, layer.extent);
        for feature in layer.features() {
            if cancelled() {
                return Err(Cancelled);
            }
            match feature.kind {
                GeomType::Polygon => {
                    let paint = area_paint(&properties_of(&feature, layer.name));
                    if paint[3] == 0 {
                        continue;
                    }
                    let surface = explicit(paint);
                    if flat {
                        for polygon in binary_polygons(&feature, layer.extent) {
                            b.flat_polygon(&polygon, surface);
                        }
                    } else {
                        let rings = feature.rings();
                        for polygon in classify_rings(&rings) {
                            b.globe_polygon(&geo, &polygon, surface);
                        }
                    }
                }
                GeomType::LineString => {
                    let p = properties_of(&feature, layer.name);
                    if flat {
                        let id = binary.properties.len() as u32;
                        binary.properties.push(p);
                        for line in feature.lines() {
                            for &q in &line {
                                binary.positions.extend(geo.unit(q));
                                binary.feature_ids.push(id);
                                binary.global_feature_ids.push(id);
                            }
                            binary.path_indices.push((binary.positions.len() / 2) as u32);
                        }
                    } else {
                        for line in feature.lines() {
                            let path = geo.globe_path(&line);
                            b.ground_line(&p, &path, PathType::Auto);
                        }
                    }
                }
                GeomType::Point | GeomType::Unknown => {}
            }
        }
    }
    if flat {
        survey(tile, &mut binary);
        b.flat_lines(tile, &binary);
    }
    Ok(b.finish())
}

/// A feature's properties as loaders.gl hands them to the accessors: its tags, and `layerName`
/// (the MVT loader's `layerProperty`).
fn properties_of(feature: &mvt::Feature<'_>, layer: &str) -> Properties {
    let mut p = feature.json_properties();
    p.insert("layerName".into(), Value::from(layer));
    p
}

/// A tile's units ↔ longitude and latitude, as loaders.gl turns them (`projectToLngLat` for the
/// globe's GeoJSON, `x / extent` for the flat map's binary tiles): the twin of
/// `crate::tess::vector`'s and `crate::ground_rails`'.
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

    /// A point in the tile's units → `(u, v)`.
    fn unit(&self, (x, y): (f64, f64)) -> [f64; 2] {
        [x / self.extent, y / self.extent]
    }

    /// Whether the longitudes and latitudes of points between `min` and `max` (tile units, y
    /// down) all lie in the 5° cell of `first`, the cell math.gl starts from.
    fn fits_one_cell(&self, first: (f64, f64), min: (f64, f64), max: (f64, f64)) -> bool {
        let [lon, lat] = self.lng_lat(first);
        let left = (lon / GLOBE_RESOLUTION_DEG).floor() * GLOBE_RESOLUTION_DEG;
        let bottom = (lat / GLOBE_RESOLUTION_DEG).floor() * GLOBE_RESOLUTION_DEG;
        let [west, north] = self.lng_lat(min);
        let [east, south] = self.lng_lat(max);
        west >= left && east <= left + GLOBE_RESOLUTION_DEG && south >= bottom && north <= bottom + GLOBE_RESOLUTION_DEG
    }

    /// A GeoJSON line on the globe as deck draws it (`[u, v, 0]`): cut on the 5° grid in
    /// longitude and latitude, or, within one cell, with its repeated points dropped (all the cut
    /// does there).
    fn globe_path(&self, line: &Points) -> Vec<[f64; 3]> {
        let (min, max) = line.iter().fold(((f64::INFINITY, f64::INFINITY), (f64::NEG_INFINITY, f64::NEG_INFINITY)), |(lo, hi), &(x, y)| {
            ((lo.0.min(x), lo.1.min(y)), (hi.0.max(x), hi.1.max(y)))
        });
        if line.is_empty() || self.fits_one_cell(line[0], min, max) {
            let mut kept: Vec<(f64, f64)> = Vec::with_capacity(line.len());
            for &p in line {
                if kept.last() != Some(&p) {
                    kept.push(p);
                }
            }
            return kept.into_iter().map(|p| self.unit(p)).map(|[u, v]| [u, v, 0.0]).collect();
        }
        let lng_lat: Vec<f64> = line.iter().flat_map(|&p| self.lng_lat(p)).collect();
        let options = PolylineGridOptions { size: 2, broken: false, grid_resolution: GLOBE_RESOLUTION_DEG, grid_offset: [0.0, 0.0] };
        cut_polyline_by_grid(&lng_lat, &options)
            .iter()
            .flat_map(|part| part.chunks_exact(2).map(|p| self.local([p[0], p[1]])).collect::<Vec<_>>())
            .map(|[u, v]| [u, v, 0.0])
            .collect()
    }
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
/// another (two numbers a vertex), each closed, the outline clockwise and the holes
/// counter-clockwise; and where each hole starts, in values.
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

/// Builds one tile.
struct Builder {
    tile: GroundTile,
    edges: PathBuilder,
    lines: PathBuilder,
    shade: PathBuilder,
    bridge_edges: PathBuilder,
    bridge_lines: PathBuilder,
    decks: PathBuilder,
    flat: bool,
}

impl Builder {
    fn new(tile: TileIndex, flat: bool) -> Self {
        let local = LocalMercator::tile(tile);
        let b = || PathBuilder::new(local);
        Builder {
            tile: GroundTile::default(),
            edges: b(),
            lines: b(),
            shade: b(),
            bridge_edges: b(),
            bridge_lines: b(),
            decks: b(),
            flat,
        }
    }

    fn finish(self) -> GroundTile {
        GroundTile {
            edges: self.edges.finish(),
            lines: self.lines.finish(),
            bridge_shade: self.shade.finish(),
            bridge_edges: self.bridge_edges.finish(),
            bridge_lines: self.bridge_lines.finish(),
            bridge_decks: self.decks.finish(),
            ..self.tile
        }
    }

    /// earcut's triangles of `positions` (two numbers a vertex, holes from the vertex indices
    /// `holes`), the vertices in tile units (`to_local`), appended.
    fn triangulate(&mut self, positions: &[f64], holes: &[usize], surface: [u8; 4], to_local: impl Fn([f64; 2]) -> [f64; 2]) {
        let indices = earcut(positions, holes, 2);
        let base = self.tile.vertices.len() as u32;
        self.tile.vertices.extend(positions.chunks_exact(2).map(|p| {
            let [u, v] = to_local([p[0], p[1]]);
            GroundVertex { position: [u as f32, v as f32, 0.0], tint: [surface[0], surface[1], surface[2]], surface: surface[3] }
        }));
        self.tile.indices.extend(indices.into_iter().map(|i| base + i as u32));
    }

    /// One polygon of the globe's GeoJSON (an outline and its holes, tile units): longitude and
    /// latitude, normalised, cut on the 5° grid, each part cut into triangles.
    fn globe_polygon(&mut self, geo: &TileGeo, rings: &[&Points], surface: [u8; 4]) {
        let lng_lat: Vec<Vec<[f64; 2]>> = rings.iter().map(|r| r.iter().map(|&p| geo.lng_lat(p)).collect()).collect();
        let (positions, holes) = normalize(&lng_lat);
        let options = GridOptions { size: 2, grid_resolution: GLOBE_RESOLUTION_DEG, grid_offset: [0.0, 0.0], edge_types: true };
        for part in cut_polygon_by_grid(&positions, &holes, &options) {
            let holes: Vec<usize> = part.hole_indices.iter().map(|h| h / 2).collect();
            self.triangulate(&part.positions, &holes, surface, |p| geo.local(p));
        }
    }

    /// One polygon of the flat map's binary tile (its rings closed, tile units), cut into
    /// triangles in tile units.
    fn flat_polygon(&mut self, rings: &[Vec<[f32; 2]>], surface: [u8; 4]) {
        let mut positions = Vec::new();
        let mut holes = Vec::new();
        for (i, ring) in rings.iter().enumerate() {
            if i > 0 {
                holes.push(positions.len() / 2);
            }
            positions.extend(ring.iter().flat_map(|&[u, v]| [f64::from(u), f64::from(v)]));
        }
        self.triangulate(&positions, &holes, surface, |p| p);
    }

    /// A line on the ground: its edging (`-edges`), and itself (`-lines`).
    fn ground_line(&mut self, p: &Properties, path: &[[f64; 3]], path_type: PathType) {
        let line = line_of(p);
        push_painted(&mut self.edges, path, line.edge_paint, line.edge_width(), path_type);
        push_painted(&mut self.lines, path, line.paint, line.width, path_type);
    }

    /// The flat map's lines (loaders.gl's binary lines, `lines`), their bridges split off and
    /// raised (`bridgeParts`).
    fn flat_lines(&mut self, tile: TileIndex, lines: &Lines) {
        debug_assert!(self.flat);
        let index = bridges::TileIndex { x: tile.x, y: tile.y, z: tile.z };
        let Some(parts) = bridge_parts(lines, index, &deck_of) else {
            each_path(lines, |p, path| self.ground_line(p, &path, PathType::Open));
            return;
        };
        each_path(&parts.ground, |p, path| self.ground_line(p, &path, PathType::Open));
        // (the shade plain: not one of the ground's surfaces)
        each_path(&parts.under, |p, path| {
            let line = line_of(p);
            self.shade.push(&path, &open(BRIDGE_SHADE, js_max(line.width, line.edge_width()) + 2.0));
        });
        each_path(&parts.raised, |p, path| {
            let line: Line = line_of(p);
            push_painted(&mut self.bridge_edges, &path, line.edge_paint, line.edge_width(), PathType::Open);
            push_painted(&mut self.bridge_lines, &path, line.paint, line.width, PathType::Open);
            // the roads' bridges only: the railways' carry the rail map, signal blocks and trains,
            // drawn after on the same deck
            let deck = if on_rails(p) { 0.0 } else { js_max(line.width, line.edge_width()) };
            self.decks.push(&path, &open(NONE, deck));
        });
    }
}

/// An open path's plain paint (the flat map's binary lines).
fn open(color: Rgba, width: f64) -> Paint<'static> {
    Paint { path_type: PathType::Open, ..Paint::solid(color, width) }
}

/// `path` in the ground's `paint` ([`explicit`]) and `width` metres, unless the paint is
/// transparent (drawn as nothing).
fn push_painted(out: &mut PathBuilder, path: &[[f64; 3]], paint: Rgba, width: f64, path_type: PathType) {
    if paint[3] != 0 {
        out.push(path, &Paint { path_type, ..Paint::solid(explicit(paint), width) });
    }
}

/// Each path of `lines` with its feature's properties, as `[u, v, metres]` (and the numeric
/// columns' values at its first vertex, as loaders.gl's binary accessors read them: the survey's).
fn each_path(lines: &Lines, mut f: impl FnMut(&Properties, Vec<[f64; 3]>)) {
    let empty = Properties::new();
    let size = lines.size;
    for w in lines.path_indices.windows(2) {
        let (start, end) = (w[0] as usize, w[1] as usize);
        let own = lines.feature_ids.get(start).and_then(|&i| lines.properties.get(i as usize)).unwrap_or(&empty);
        let with_columns;
        let p = if lines.numeric_props.is_empty() {
            own
        } else {
            let mut p = own.clone();
            for (name, column) in &lines.numeric_props {
                if let Some(value) = column.get(start).copied().and_then(serde_json::Number::from_f64) {
                    p.insert(name.clone(), Value::Number(value));
                }
            }
            with_columns = p;
            &with_columns
        };
        let path = (start..end)
            .map(|i| {
                let at = |k: usize| lines.positions.get(i * size + k).copied().unwrap_or(f64::NAN);
                [at(0), at(1), if size == 3 { at(2) } else { 0.0 }]
            })
            .collect();
        f(p, path);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::camera::world_to_lng_lat;
    use crate::ground::{FARMLAND, ICE, WATER_ID, surface};

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
    }

    /// A feature: its geometry type, tags and parts (absolute points, polygons' rings open).
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

    /// A square ring, clockwise with y down (an outline).
    fn square(x: i64, y: i64, side: i64) -> Vec<(i64, i64)> {
        vec![(x, y), (x + side, y), (x + side, y + side), (x, y + side)]
    }

    /// Stockholm's z9 tile (V3's centre), and a z14 tile in it.
    const Z9: TileIndex = TileIndex { x: 281, y: 150, z: 9 };
    const Z14: TileIndex = TileIndex { x: 9006, y: 4818, z: 14 };

    /// The surfaces of the areas' triangles, each polygon once, in order.
    fn area_surfaces(t: &GroundTile) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        for tri in t.indices.chunks_exact(3) {
            let s = t.vertices[tri[0] as usize].surface;
            if out.last() != Some(&s) {
                out.push(s);
            }
        }
        out
    }

    /// The segments' (tint, surface) colours and widths, each run of one colour once.
    fn runs(segments: &[PathSegment]) -> Vec<([u8; 4], f32)> {
        let mut out: Vec<([u8; 4], f32)> = Vec::new();
        for s in segments {
            if out.last() != Some(&(s.color, s.width)) {
                out.push((s.color, s.width));
            }
        }
        out
    }

    /// The test tile: a lake and a wood, a residential area and a quarter's name (no ground), a
    /// motorway, a footway, a railway (OpenRailwayMap's in the app), a tram, a road in a tunnel,
    /// a river; a building layer the ground does not read.
    fn tile_bytes() -> Vec<u8> {
        encode(&[
            ("building", vec![polygon(&[], &[&square(100, 100, 100)])]),
            (
                "transportation",
                vec![
                    line(&[("class", V::S("motorway"))], &[&[(0, 2048), (4096, 2048)]]),
                    line(&[("class", V::S("path")), ("subclass", V::S("footway"))], &[&[(2048, 0), (2048, 4096)]]),
                    line(&[("class", V::S("rail"))], &[&[(0, 100), (4096, 100)]]),
                    line(&[("class", V::S("transit")), ("subclass", V::S("tram"))], &[&[(0, 300), (4096, 300)]]),
                    line(&[("class", V::S("primary")), ("brunnel", V::S("tunnel"))], &[&[(0, 500), (4096, 500)]]),
                ],
            ),
            ("waterway", vec![line(&[("class", V::S("river"))], &[&[(0, 3000), (4096, 3100)]])]),
            ("water", vec![polygon(&[("class", V::S("lake"))], &[&square(500, 500, 1000)])]),
            (
                "landuse",
                vec![
                    polygon(&[("class", V::S("residential"))], &[&square(2500, 2500, 1000)]),
                    polygon(&[("class", V::S("suburb"))], &[&square(0, 0, 4096)]),
                ],
            ),
            ("landcover", vec![polygon(&[("class", V::S("wood")), ("subclass", V::S("forest"))], &[&square(1500, 500, 500)])]),
        ])
    }

    #[test]
    fn a_paint_becomes_its_tint_and_surface_id() {
        assert_eq!(explicit(crate::ground::LAND), [58, 81, 29, 0]);
        assert_eq!(explicit(crate::ground::area::OCEAN), [200, 214, 222, WATER_ID]);
        // nothing: an id the shader draws as nothing
        assert_eq!(explicit(NONE)[3], 255);
        assert!(usize::from(explicit(NONE)[3]) >= SURFACE_COUNT);
        assert_eq!(size_of::<GroundVertex>(), 16);
    }

    /// The three passes: areas in the layers' order (landuse, landcover, aeroway, water,
    /// waterway, transportation, whatever the tile's own), names of places left out; then each
    /// line's edging, then the lines, the app's own railways and tunnels left out, metres wide.
    #[test]
    fn a_tile_is_its_areas_then_its_edging_then_its_lines() {
        let bytes = tile_bytes();
        for flat in [false, true] {
            let t = tessellate(Z9, &bytes, &GROUND_LAYERS, flat, &never).expect("not cancelled");
            // residential (5), wood (2), lake (11)
            assert_eq!(area_surfaces(&t), [5, 2, WATER_ID], "flat {flat}");
            assert!(t.vertices.iter().all(|v| v.position[2] == 0.0));
            let lake = t.vertices.iter().find(|v| v.surface == WATER_ID).expect("a lake");
            assert_eq!(lake.tint, [255, 255, 255]);
            // the river (no edging), then the motorway's shoulders, the footway (none), the tram (none)
            assert_eq!(runs(&t.edges), [([210, 210, 210, 37], 14.0)], "flat {flat}");
            assert_eq!(
                runs(&t.lines),
                [([255, 255, 255, WATER_ID], 18.0), ([255, 255, 255, 32], 11.0), ([255, 255, 255, 36], 2.0), ([255, 255, 255, 39], 2.2)],
                "flat {flat}"
            );
            assert!(t.bridge_shade.is_empty() && t.bridge_lines.is_empty() && t.bridge_decks.is_empty());
        }
    }

    /// A road with edging of none and an edge paint (a service road's pavement, `road(4, false)`)
    /// is edged at width 0 (the layer's minimum, under the road); a line of no edge paint is not.
    #[test]
    fn edging_follows_line_of() {
        let bytes = encode(&[(
            "transportation",
            vec![
                line(&[("class", V::S("service"))], &[&[(0, 2048), (4096, 2048)]]),
                line(&[("class", V::S("primary"))], &[&[(0, 1000), (4096, 1000)]]),
                line(&[("class", V::S("track"))], &[&[(0, 3000), (4096, 3000)]]),
                line(&[("class", V::S("ferry"))], &[&[(0, 3500), (4096, 3500)]]),
                line(&[("class", V::S("minor_construction"))], &[&[(0, 3600), (4096, 3600)]]),
            ],
        )]);
        let t = tessellate(Z9, &bytes, &GROUND_LAYERS, false, &never).expect("not cancelled");
        assert_eq!(runs(&t.edges), [([255, 255, 255, 36], 0.0), ([255, 255, 255, 36], 13.0)]);
        assert_eq!(runs(&t.lines), [([255, 255, 255, 33], 4.0), ([255, 255, 255, 32], 9.0), ([255, 255, 255, 34], 3.0)]);
    }

    /// On the globe each polygon and line is cut on the 5° grid in longitude and latitude, as
    /// deck's GeoJSON is; within one cell nothing changes but the vertices' place. A z5 tile
    /// (11.25° E to 22.5° E, 55.7° N to 61.7° N) is cut at 15° and 20° E and at 60° N.
    #[test]
    fn the_globe_cuts_on_the_5_degree_grid() {
        let tile = TileIndex { x: 17, y: 9, z: 5 };
        let bytes = encode(&[
            ("landcover", vec![polygon(&[("class", V::S("grass"))], &[&square(0, 0, 4096)])]),
            ("transportation", vec![line(&[("class", V::S("motorway"))], &[&[(0, 2048), (4096, 2048)]])]),
        ]);
        let globe = tessellate(tile, &bytes, &GROUND_LAYERS, false, &never).expect("not cancelled");
        let flat = tessellate(tile, &bytes, &GROUND_LAYERS, true, &never).expect("not cancelled");
        assert_eq!(flat.indices.len(), 6, "the flat map's square: two triangles");
        assert!(globe.indices.len() > 6, "{} indices", globe.indices.len());
        // every vertex of the cut lies inside the tile, on its square or on a grid line
        let local = LocalMercator::tile(tile);
        for v in &globe.vertices {
            let [u, w] = [f64::from(v.position[0]), f64::from(v.position[1])];
            assert!((-1e-6..=1.0 + 1e-6).contains(&u) && (-1e-6..=1.0 + 1e-6).contains(&w), "{u} {w}");
            let [lon, lat] = world_to_lng_lat(local.to_world([u, w]).to_array());
            let on_edge = u.abs() < 1e-6 || (u - 1.0).abs() < 1e-6 || w.abs() < 1e-6 || (w - 1.0).abs() < 1e-6;
            let on_grid = (lon / 5.0 - (lon / 5.0).round()).abs() < 1e-6 || (lat / 5.0 - (lat / 5.0).round()).abs() < 1e-6;
            assert!(on_edge || on_grid, "{lon} {lat}");
        }
        // the cut areas cover the tile: the triangles' area is the square's
        let area: f64 = globe
            .indices
            .chunks_exact(3)
            .map(|tri| {
                let p = |k: usize| globe.vertices[tri[k] as usize].position.map(f64::from);
                let (a, b, c) = (p(0), p(1), p(2));
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0
            })
            .sum();
        assert!((area - 1.0).abs() < 1e-3, "{area}");
        // the motorway crosses 15° E and 20° E: two more vertices on the globe
        assert_eq!(flat.lines.len(), 1);
        assert_eq!(globe.lines.len(), 3);
    }

    /// The flat map's bridges stand on their decks over their shade (`bridgeParts`): a road's a
    /// deck a `layer` (6 m each), a tram's a railway's (6 m, its deck 0 wide in the depth buffer,
    /// as it carries the rail map); on the globe they lie on the ground.
    #[test]
    fn the_flat_maps_bridges_are_raised() {
        let bytes = encode(&[(
            "transportation",
            vec![
                line(&[("class", V::S("primary")), ("brunnel", V::S("bridge"))], &[&[(1000, 2048), (3000, 2048)]]),
                line(&[("class", V::S("secondary")), ("brunnel", V::S("bridge")), ("layer", V::U(2))], &[&[(1000, 1000), (3000, 1000)]]),
                line(&[("class", V::S("transit")), ("subclass", V::S("tram")), ("brunnel", V::S("bridge")), ("layer", V::U(3))], &[&[(2048, 1000), (2048, 3000)]]),
            ],
        )]);
        let flat = tessellate(Z14, &bytes, &GROUND_LAYERS, true, &never).expect("not cancelled");
        assert!(flat.edges.is_empty() && flat.lines.is_empty(), "every line is a bridge");
        // `max(width, edge width) + 2`
        assert_eq!(runs(&flat.bridge_shade), [(BRIDGE_SHADE, 15.0), (BRIDGE_SHADE, 14.0), (BRIDGE_SHADE, 4.2)]);
        assert_eq!(runs(&flat.bridge_edges), [([255, 255, 255, 36], 13.0), ([255, 255, 255, 36], 12.0)]);
        assert_eq!(runs(&flat.bridge_lines), [([255, 255, 255, 32], 9.0), ([255, 255, 255, 32], 8.0), ([255, 255, 255, 39], 2.2)]);
        assert_eq!(runs(&flat.bridge_decks), [(NONE, 13.0), (NONE, 12.0), (NONE, 0.0)]);
        // up to their decks in the middle
        let top = |segments: &[PathSegment], width: f32| segments.iter().filter(|s| s.width == width).map(|s| s.start[2]).fold(0.0, f32::max);
        assert_eq!(top(&flat.bridge_lines, 9.0), 6.0);
        assert_eq!(top(&flat.bridge_lines, 8.0), 12.0);
        assert_eq!(top(&flat.bridge_lines, 2.2), 6.0);
        assert!(flat.bridge_shade.iter().all(|s| s.start[2] == 0.0));
        // on the globe, on the ground
        let globe = tessellate(Z14, &bytes, &GROUND_LAYERS, false, &never).expect("not cancelled");
        assert_eq!(runs(&globe.lines), [([255, 255, 255, 32], 9.0), ([255, 255, 255, 32], 8.0), ([255, 255, 255, 39], 2.2)]);
        assert!(globe.bridge_lines.is_empty() && globe.lines.iter().all(|s| s.start[2] == 0.0));
    }

    /// Streets GL's layers: fields with their rows, crop and shade in their colour, gardens as
    /// residential, highways as wide as their lanes.
    #[test]
    fn streets_gl_tiles_read_their_own_layers() {
        let bytes = encode(&[
            ("common", vec![polygon(&[("type", V::S("garden"))], &[&square(0, 0, 1000)])]),
            ("natural", vec![polygon(&[("type", V::S("farmland")), ("osmId", V::U(42))], &[&square(1000, 1000, 1000)])]),
            ("highways", vec![line(&[("type", V::S("path")), ("pathType", V::S("residential"))], &[&[(0, 3000), (4096, 3000)]])]),
            ("transportation", vec![line(&[("class", V::S("motorway"))], &[&[(0, 2048), (4096, 2048)]])]),
        ]);
        let t = tessellate(TileIndex { x: 36027, y: 19272, z: 16 }, &bytes, &SGL_GROUND_LAYERS, true, &never).expect("not cancelled");
        assert_eq!(area_surfaces(&t), [5, 17]);
        assert_eq!(runs(&t.lines), [([255, 255, 255, 33], 6.0)]);
    }

    #[test]
    fn a_cancelled_job_stops() {
        let bytes = tile_bytes();
        assert_eq!(tessellate(Z9, &bytes, &GROUND_LAYERS, false, &|| true), Err(Cancelled));
    }

    fn fixture(z: u32, x: u32, y: u32) -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../map/tests/data/tiles/openfreemap/{z}/{x}/{y}.pbf"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// V3's tiles (Stockholm at z9) hold the ground the realistic map shows there: water, woods,
    /// fields, gardens and towns, roads; every surface one of the table's, every width positive.
    #[test]
    fn stockholms_tiles_hold_water_woods_fields_towns_and_roads() {
        let mut surfaces = std::collections::BTreeSet::new();
        let mut line_surfaces = std::collections::BTreeSet::new();
        for (x, y) in [(280, 150), (281, 150), (281, 149), (282, 150)] {
            let t = tessellate(TileIndex { x, y, z: 9 }, &fixture(9, x, y), &GROUND_LAYERS, false, &never).expect("not cancelled");
            assert!(!t.is_empty() && t.indices.len().is_multiple_of(3));
            assert!(t.indices.iter().all(|&i| (i as usize) < t.vertices.len()));
            for v in &t.vertices {
                assert!(surface(usize::from(v.surface)).is_some(), "surface {}", v.surface);
                surfaces.insert(v.surface);
            }
            for s in t.edges.iter().chain(&t.lines) {
                assert!(surface(usize::from(s.color[3])).is_some(), "line surface {}", s.color[3]);
                assert!(s.width >= 0.0);
                line_surfaces.insert(s.color[3]);
            }
            assert!(t.lines.iter().all(|s| s.width > 0.0));
            assert!(t.byte_size() > 0);
        }
        eprintln!("areas {surfaces:?}, lines {line_surfaces:?}");
        for id in [WATER_ID, 2, FARMLAND, 5] {
            assert!(surfaces.contains(&id), "no surface {id} in {surfaces:?}");
        }
        assert!(!surfaces.contains(&ICE));
        // marked roads and their pavements (z9 holds no minor roads)
        for id in [32, 36] {
            assert!(line_surfaces.contains(&id), "no line surface {id} in {line_surfaces:?}");
        }
    }
}
