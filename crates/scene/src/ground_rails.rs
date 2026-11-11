//! Ground rails and platforms on the realistic map, the CPU side (item T9; doc 02 §8.6;
//! `railGroundLayers`, `realistic-ground.ts:865-924`, drawn by `GroundTile`, `:646-778`, styled
//! by `lineOf` and `areaPaint`, `:338-402` and `:160-260`, here `crate::ground`): what the
//! realistic map's ground layer (stage 8, item R9) uploads and draws, made from a backend rail
//! tile's bytes on the worker pool.
//!
//! What today's app does, and so this module:
//! - **Only in the app, only on the realistic map** (`OWN_RAILS = isTauri()`: always so in the
//!   native app), pushed right after the ground's tiles (G:2663). While they are on, the ground's
//!   own railways are left out but for trams' rails in the street (`crate::ground::OWN_RAILS`).
//! - **`ground-rails-globe` / `ground-rails-flat`**: the backend's rail tiles (OpenRailwayMap's
//!   `railway_line_high`, [`RAIL_LINES`]) at tile zooms 7 to 12 ([`RAIL_TILES`], the speed map's,
//!   over-zoomed past 12), one MVTLayer (and cache) per projection. Widths in **metres**, at
//!   least [`RAILS_MIN_PIXELS`] px, round joints and caps, not pickable; drawn with the ground's
//!   surfaces (`GROUNDED`) in `SURFACE_NO_CULL` (no depth write: over the rest of the ground, in
//!   order). A track ([`crate::ground::line_of`]): not drawn in a tunnel, nor a tram's (the
//!   maps' own, in the street); light rail, subways and monorails as `ROADS.transit`, 2.2 m of
//!   track (paint 35) on 0.6 m of ballast each side (paint 38); every other railway in use as
//!   `ROADS.rail`, 2.6 m on 1 m; abandoned, razed or other railways not at all.
//! - **`ground-platforms`** (flat map only): the same backend's `standard_railway_platforms`
//!   ([`PLATFORMS`]) at tile zoom 15 alone ([`PLATFORM_TILES`]), tiles within 4 km of the camera
//!   ([`platform_lod`]). Each platform's polygon stands [`PLATFORM_M`] (0.6 m) tall: its top and
//!   its sides, in `AREAS.platform` (paint 9, the pavement), unlit by deck (`material: false`:
//!   the ground's own shading lights the sides by their facing), outlines counter-clockwise in
//!   the tile's numbers (`_windingOrder: "CCW"`, sides facing out), drawn in `NO_CULL` (depth
//!   written: it hides what is behind it). A platform mapped as a line lies flat as
//!   `PATHS.pedestrian` (5 m, paint 36), at least 0 px ([`PLATFORMS_MIN_PIXELS`]).
//!
//! **The passes** (`GroundTile.renderLayers`): each tile is drawn as deck's GeoJsonLayers
//! `-areas` (its polygons), `-edges` (each line's edging, [`crate::ground::Line::edge_width`]
//! wide in its `edge_paint`) and `-lines` (each line in its paint): so no track's ballast covers
//! another's track where they meet. On the flat map (loaders.gl's binary tiles) a tile's bridges
//! are raised to their decks ([`crate::bridges::bridge_parts`]: OpenRailwayMap's tracks to
//! [`crate::bridges::RAIL_DECK_M`] with `ormDeck`, anything else with `omtDeck`), left out of the
//! ground's passes and drawn after them: `-bridge-shade` (the bridges flat, in
//! [`BRIDGE_SHADE`], unlit, `max(width, edge width) + 2` m wide: every bridge of the tile, drawn
//! or not), `-bridge-edges`, `-bridge-lines`, and `-bridge-decks` (in [`NONE`] with depth
//! written, 0 m wide on a railway, else as wide as its edging: so what comes after passes under a
//! road's bridge; a railway's carries what is drawn on it after). On the globe (GeoJSON) nothing
//! is raised. [`GroundLines`] holds the line passes in that order; [`PlatformsTile`] adds the
//! platforms' mesh for `-areas`.
//!
//! **Paints**: a segment's colour ([`PathSegment::color`]) and a vertex's [`PlatformVertex::paint`]
//! are the ground's paints as deck carries them (`crate::ground`): RGB a tint (sRGB), alpha 255
//! less the surface's id; the ground shader reads the surface from the alpha. The shade is a
//! plain colour (drawn without the ground's surfaces) and the decks are depth only.
//!
//! **Geometry**, as `crate::rail` tessellates the speed map's tiles: in the tile's units
//! ([`LocalMercator::tile`]: `u` east, `v` south, metres up). On the flat map a tile's lines as
//! loaders.gl's binary tiles hold them: open paths (`_pathType: 'open'`), each part of a
//! multi-line its own row. On the globe as GeoJSON: each line cut on the 5° grid (math.gl's
//! `cutPolylineByGrid`, through longitude and latitude), a line whose last point is its first a
//! loop. The flat map also cuts each tile at its square (MVTLayer's `ClipExtension`): that is the
//! layer's. So a z12 tile is tessellated one way for each projection, as deck keeps one layer per
//! projection ([`rails_tile`]'s `flat`).
//!
//! Deviations, none visible:
//! - what deck draws transparent and without depth (a line or edging in [`NONE`], an area of no
//!   paint) is left out; the decks are kept, though transparent;
//! - the rail tiles' polygons are left out: deck would fill them in its default black (the
//!   ground's terrain, tinted black), but OpenRailwayMap's `railway_line_high` holds lines alone;
//! - the tags are read as GeoJSON features have them; loaders.gl's binary tiles read a numeric
//!   tag a feature lacks as 0 where other features have it, which no tag read here notices
//!   (`feature`, `state`, `service`, `tunnel` and `bridge` are text or booleans; `layer` is
//!   clamped to at least 1, so 0 and absent agree);
//! - as in `crate::tess::buildings`, a platform's side of no length is left out, and its top is
//!   `crate::geom::earcut`'s triangles of its rings (earcut 2.2.4, loaders.gl's).

use bytemuck::{Pod, Zeroable};
use glam::DVec2;
use rustradar_core::mvt::{self, GeomType};
use serde_json::Value;

use crate::bridges::{self, Lines, Properties, bridge_parts, omt_deck, orm_deck};
use crate::camera::lng_lat_to_world;
use crate::geom::densify::GLOBE_RESOLUTION_DEG;
use crate::geom::earcut::earcut;
use crate::geom::grid_cut::{PolylineGridOptions, cut_polyline_by_grid};
use crate::geom::js_max;
use crate::geom::path::{LocalMercator, Paint, PathBuilder, PathSegment, PathType};
pub use crate::ground::{BRIDGE_SHADE, NONE, PLATFORM_M, PLATFORM_MAX_DISTANCE_M, PLATFORM_ZOOM, PLATFORMS, RAIL_LINES, Rgba};
use crate::ground::{area_paint, line_of, on_rails};
use crate::lod::Lod;
use crate::tess::buildings::binary_polygons;
pub use crate::tess::vector::Cancelled;
use crate::tiles::{TileIndex, TilesetOptions};

/// The ground rails' tiles: the backend's rail tiles at zooms 7 to 12, 512 px, over-zoomed past 12
/// (`SPEED_TILE_ZOOMS`, the speed map's).
pub const RAIL_TILES: TilesetOptions = crate::rail::SPEED_TILES;

/// The platforms' tiles: zoom 15 alone (`minZoom: PLATFORM_ZOOM, maxZoom: PLATFORM_ZOOM`).
pub const PLATFORM_TILES: TilesetOptions = TilesetOptions::new(512.0, PLATFORM_ZOOM as f64, PLATFORM_ZOOM as f64);

/// Far out, a track stays a hairline this wide, px (`lineWidthMinPixels`, as the maps' railways
/// were).
pub const RAILS_MIN_PIXELS: f64 = crate::ground::LINE_MIN_PIXELS;

/// The platforms' layer sets no minimum width (deck's default, 0 px).
pub const PLATFORMS_MIN_PIXELS: f64 = 0.0;

/// `PLATFORM_LOD`: the platforms' tiles by their distance from the camera, zoom 15 within 4 km.
pub fn platform_lod() -> Lod {
    Lod { levels: vec![PLATFORM_ZOOM], bias: 0.0, max_distance: PLATFORM_MAX_DISTANCE_M }
}

/// How high a line's bridge is (`deckOf`): OpenRailwayMap's tracks' as the rail map and the
/// trains have them (`ormDeck`), anything else's as OpenMapTiles has them (`omtDeck`).
pub fn deck_of(p: &Properties, numeric: &dyn Fn(&str) -> Option<f64>) -> f64 {
    if p.get("layerName").and_then(Value::as_str) == Some(RAIL_LINES) { orm_deck(p, numeric) } else { omt_deck(p, numeric) }
}

/// One tile's lines, in the passes deck draws them in (see the module notes), each in the tile's
/// units with widths in metres.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroundLines {
    /// `-edges`: the edging (ballast) of each line on the ground.
    pub edges: Vec<PathSegment>,
    /// `-lines`: each line on the ground.
    pub lines: Vec<PathSegment>,
    /// `-bridge-shade` (flat map): the bridges flat on the ground, in [`BRIDGE_SHADE`], unlit.
    pub bridge_shade: Vec<PathSegment>,
    /// `-bridge-edges` (flat map): the bridges' edging, raised.
    pub bridge_edges: Vec<PathSegment>,
    /// `-bridge-lines` (flat map): the bridges, raised.
    pub bridge_lines: Vec<PathSegment>,
    /// `-bridge-decks` (flat map): the bridges raised in [`NONE`], for the depth buffer alone
    /// (depth written; a railway's 0 m wide, so at the layer's minimum width).
    pub bridge_decks: Vec<PathSegment>,
}

impl GroundLines {
    /// The passes in draw order, with deck's sub-layer id suffixes.
    pub fn passes(&self) -> [(&'static str, &[PathSegment]); 6] {
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
        self.passes().iter().all(|(_, s)| s.is_empty())
    }

    /// The bytes it holds (for the tile cache's budget).
    pub fn byte_size(&self) -> usize {
        self.passes().iter().map(|(_, s)| size_of_val(*s)).sum()
    }
}

/// One vertex of the platforms' mesh: `(u, v, h)` in the tile's frame (tile units, `v` south,
/// metres up), its normal east, north, up (unit length: up on a top, level and facing out on a
/// side, which the ground's shading reads as the side's facing, `rg_wall`), and its paint. 28
/// bytes, `#[repr(C)]`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct PlatformVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub paint: Rgba,
}

/// One tile of platforms: the platforms standing (`-areas`, extruded) and the lines (platforms
/// mapped as lines, flat).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlatformsTile {
    pub vertices: Vec<PlatformVertex>,
    /// Three indices into [`Self::vertices`] per triangle, platform after platform, each one's
    /// sides then its top.
    pub indices: Vec<u32>,
    pub lines: GroundLines,
}

impl PlatformsTile {
    /// Whether it draws nothing.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty() && self.lines.is_empty()
    }

    /// The bytes it holds (for the tile cache's budget).
    pub fn byte_size(&self) -> usize {
        size_of_val(self.vertices.as_slice()) + size_of_val(self.indices.as_slice()) + self.lines.byte_size()
    }
}

/// The ground rails of one backend rail tile (`bytes`, as served) at `tile`, as the layer of the
/// flat map (`flat`: binary tiles, bridges raised) or of the globe draws it. `cancelled` is asked
/// between features.
pub fn rails_tile(tile: TileIndex, bytes: &[u8], flat: bool, cancelled: &dyn Fn() -> bool) -> Result<GroundLines, Cancelled> {
    let mut out = Passes::new(tile);
    for layer in mvt::layers_named(bytes, &[RAIL_LINES]) {
        out.layer_lines(tile, &layer, flat, cancelled)?;
    }
    Ok(out.finish())
}

/// The platforms of one backend platform tile (zoom [`PLATFORM_ZOOM`], flat map only) at `tile`:
/// each polygon standing [`PLATFORM_M`] tall, each line flat. `cancelled` is asked between
/// features.
pub fn platforms_tile(tile: TileIndex, bytes: &[u8], cancelled: &dyn Fn() -> bool) -> Result<PlatformsTile, Cancelled> {
    let mut out = Passes::new(tile);
    let mut mesh = PlatformsTile::default();
    for layer in mvt::layers_named(bytes, &[PLATFORMS]) {
        for feature in layer.features() {
            if cancelled() {
                return Err(Cancelled);
            }
            if feature.kind != GeomType::Polygon {
                continue;
            }
            let paint = area_paint(&properties_of(&feature, layer.name));
            if paint[3] == 0 {
                continue;
            }
            // deck's `elevations` attribute: f32
            for polygon in binary_polygons(&feature, layer.extent) {
                extrude(&mut mesh, &polygon, PLATFORM_M as f32, paint);
            }
        }
        out.layer_lines(tile, &layer, true, cancelled)?;
    }
    mesh.lines = out.finish();
    Ok(mesh)
}

/// One platform: its rings (closed, tile units) standing `height` metres tall in `paint`. Each
/// edge of each ring a side from the ground to the height, its normal `(dv, du)` east and north for
/// the edge `(du, dv)` (deck's side normal with `_windingOrder: 'CCW'` through the tile's
/// flipped `v`: out of an outline, into a hole); then the top, the rings' earcut triangles.
fn extrude(out: &mut PlatformsTile, rings: &[Vec<[f32; 2]>], height: f32, paint: Rgba) {
    for ring in rings {
        for edge in ring.windows(2) {
            let ([u0, v0], [u1, v1]) = (edge[0], edge[1]);
            let (du, dv) = (f64::from(u1) - f64::from(u0), f64::from(v1) - f64::from(v0));
            let length = du.hypot(dv);
            if length == 0.0 {
                continue;
            }
            let normal = [(dv / length) as f32, (du / length) as f32, 0.0];
            let base = out.vertices.len() as u32;
            for [u, v, h] in [[u0, v0, 0.0], [u1, v1, 0.0], [u1, v1, height], [u0, v0, height]] {
                out.vertices.push(PlatformVertex { position: [u, v, h], normal, paint });
            }
            out.indices.extend([0, 1, 2, 0, 2, 3].map(|k| base + k));
        }
    }
    let mut positions = Vec::new();
    let mut holes = Vec::new();
    for (i, ring) in rings.iter().enumerate() {
        if i > 0 {
            holes.push(positions.len() / 2);
        }
        positions.extend(ring.iter().flat_map(|&[u, v]| [f64::from(u), f64::from(v)]));
    }
    let triangles = earcut(&positions, &holes, 2);
    if triangles.is_empty() {
        return;
    }
    let base = out.vertices.len() as u32;
    for &[u, v] in rings.iter().flatten() {
        out.vertices.push(PlatformVertex { position: [u, v, height], normal: [0.0, 0.0, 1.0], paint });
    }
    out.indices.extend(triangles.into_iter().map(|i| base + i as u32));
}

/// A feature's properties as loaders.gl hands them to the accessors: its tags, and `layerName`
/// (the MVT loader's `layerProperty`).
fn properties_of(feature: &mvt::Feature<'_>, layer: &str) -> Properties {
    let mut p = feature.json_properties();
    p.insert("layerName".into(), Value::from(layer));
    p
}

/// The six line passes being made.
struct Passes {
    edges: PathBuilder,
    lines: PathBuilder,
    shade: PathBuilder,
    bridge_edges: PathBuilder,
    bridge_lines: PathBuilder,
    decks: PathBuilder,
}

impl Passes {
    fn new(tile: TileIndex) -> Self {
        let b = || PathBuilder::new(LocalMercator::tile(tile));
        Passes { edges: b(), lines: b(), shade: b(), bridge_edges: b(), bridge_lines: b(), decks: b() }
    }

    fn finish(self) -> GroundLines {
        GroundLines {
            edges: self.edges.finish(),
            lines: self.lines.finish(),
            bridge_shade: self.shade.finish(),
            bridge_edges: self.bridge_edges.finish(),
            bridge_lines: self.bridge_lines.finish(),
            bridge_decks: self.decks.finish(),
        }
    }

    /// The line features of one layer of `tile`.
    fn layer_lines(&mut self, tile: TileIndex, layer: &mvt::Layer<'_>, flat: bool, cancelled: &dyn Fn() -> bool) -> Result<(), Cancelled> {
        let geo = TileGeo::new(tile, layer.extent);
        if !flat {
            // GeoJSON: each part of each line its own row, in longitude and latitude, on the ground
            for feature in layer.features() {
                if cancelled() {
                    return Err(Cancelled);
                }
                if feature.kind != GeomType::LineString {
                    continue;
                }
                let p = properties_of(&feature, layer.name);
                for line in feature.lines() {
                    self.ground(&p, &geo.globe_path(&line), PathType::Auto);
                }
            }
            return Ok(());
        }
        // loaders.gl's binary lines, split by their decks
        let mut lines = Lines { kind: "LineString".into(), size: 2, path_indices: vec![0], ..Lines::default() };
        for feature in layer.features() {
            if cancelled() {
                return Err(Cancelled);
            }
            if feature.kind != GeomType::LineString {
                continue;
            }
            let id = lines.properties.len() as u32;
            lines.properties.push(properties_of(&feature, layer.name));
            for line in feature.lines() {
                for &q in &line {
                    lines.positions.extend(geo.unit(q));
                    lines.feature_ids.push(id);
                    lines.global_feature_ids.push(id);
                }
                lines.path_indices.push((lines.positions.len() / 2) as u32);
            }
        }
        let index = bridges::TileIndex { x: tile.x, y: tile.y, z: tile.z };
        match bridge_parts(&lines, index, &deck_of) {
            None => each_path(&lines, |p, path| self.ground(p, &path, PathType::Open)),
            Some(parts) => {
                each_path(&parts.ground, |p, path| self.ground(p, &path, PathType::Open));
                each_path(&parts.under, |p, path| {
                    let line = line_of(p);
                    self.shade.push(&path, &paint(BRIDGE_SHADE, js_max(line.width, line.edge_width()) + 2.0));
                });
                each_path(&parts.raised, |p, path| {
                    let line = line_of(p);
                    push_painted(&mut self.bridge_edges, &path, line.edge_paint, line.edge_width(), PathType::Open);
                    push_painted(&mut self.bridge_lines, &path, line.paint, line.width, PathType::Open);
                    let deck = if on_rails(p) { 0.0 } else { js_max(line.width, line.edge_width()) };
                    self.decks.push(&path, &paint(NONE, deck));
                });
            }
        }
        Ok(())
    }

    /// A line on the ground: its edging, then (in the next pass) itself.
    fn ground(&mut self, p: &Properties, path: &[[f64; 3]], path_type: PathType) {
        let line = line_of(p);
        push_painted(&mut self.edges, path, line.edge_paint, line.edge_width(), path_type);
        push_painted(&mut self.lines, path, line.paint, line.width, path_type);
    }
}

/// An open path's paint (the flat map's binary lines).
fn paint(color: Rgba, width: f64) -> Paint<'static> {
    Paint { path_type: PathType::Open, ..Paint::solid(color, width) }
}

/// `path` in `color` and `width` unless the colour is transparent (drawn as nothing, no depth).
fn push_painted(out: &mut PathBuilder, path: &[[f64; 3]], color: Rgba, width: f64, path_type: PathType) {
    if color[3] != 0 {
        out.push(path, &Paint { path_type, ..Paint::solid(color, width) });
    }
}

/// Each path of `lines` with its feature's properties, as `[u, v, metres]`.
fn each_path(lines: &Lines, mut f: impl FnMut(&Properties, Vec<[f64; 3]>)) {
    let empty = Properties::new();
    let size = lines.size;
    for w in lines.path_indices.windows(2) {
        let (start, end) = (w[0] as usize, w[1] as usize);
        let p = lines.feature_ids.get(start).and_then(|&i| lines.properties.get(i as usize)).unwrap_or(&empty);
        let path = (start..end)
            .map(|i| {
                let at = |k: usize| lines.positions.get(i * size + k).copied().unwrap_or(f64::NAN);
                [at(0), at(1), if size == 3 { at(2) } else { 0.0 }]
            })
            .collect();
        f(p, path);
    }
}

/// A tile's units ↔ longitude and latitude, as loaders.gl turns them (`projectToLngLat` for the
/// globe's GeoJSON, `x / extent` for the flat map's local tiles): the twin of `crate::rail`'s
/// and `crate::tess::vector`'s.
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
        [lon, (360.0 / std::f64::consts::PI) * ((y2 * std::f64::consts::PI) / 180.0).exp().atan() - 90.0]
    }

    /// A point in the tile's units → `(u, v)` (loaders.gl's `convertToLocalCoordinates`).
    fn unit(&self, (x, y): (f64, f64)) -> [f64; 2] {
        [x / self.extent, y / self.extent]
    }

    /// One line as the globe's PathLayer reads it: in longitude and latitude, cut on the 5° grid
    /// (which also drops repeated points), back in the tile's `(u, v)` on the ground.
    fn globe_path(&self, line: &[(f64, f64)]) -> Vec<[f64; 3]> {
        let lng_lat: Vec<[f64; 2]> = line.iter().map(|&q| self.lng_lat(q)).collect();
        let cell = |[lon, lat]: [f64; 2]| ((lon / GLOBE_RESOLUTION_DEG).floor(), (lat / GLOBE_RESOLUTION_DEG).floor());
        let in_frame = |&[u, v]: &[f64; 2]| [u, v, 0.0];
        if lng_lat.iter().all(|&q| cell(q) == cell(lng_lat[0])) {
            // no grid line crossed: the cut only drops repeated points
            let mut kept: Vec<(f64, f64)> = Vec::with_capacity(line.len());
            for &q in line {
                if kept.last() != Some(&q) {
                    kept.push(q);
                }
            }
            return kept.into_iter().map(|q| in_frame(&self.unit(q))).collect();
        }
        let flat: Vec<f64> = lng_lat.iter().flatten().copied().collect();
        let options = PolylineGridOptions { size: 2, broken: false, grid_resolution: GLOBE_RESOLUTION_DEG, grid_offset: [0.0, 0.0] };
        cut_polyline_by_grid(&flat, &options)
            .iter()
            .flat_map(|part| part.chunks_exact(2).map(|q| in_frame(&self.local.from_world(DVec2::from(lng_lat_to_world([q[0], q[1]]))))).collect::<Vec<_>>())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::EARTH_CIRCUMFERENCE_M;

    use super::*;
    use crate::geom::path::{END_CAP, START_CAP};
    use crate::ground::{area, paint as ground_paint};

    /// A tag value of a test tile.
    #[derive(Clone, Copy)]
    enum V {
        S(&'static str),
        U(u64),
        B(bool),
    }

    /// A feature of a test tile: geometry type (2 line, 3 polygon), tags and parts (absolute
    /// points in tile units of 4096; a polygon's rings without their closing point).
    struct F {
        kind: u64,
        tags: Vec<(&'static str, V)>,
        parts: Vec<Vec<(i64, i64)>>,
    }

    fn line_f(tags: &[(&'static str, V)], parts: &[&[(i64, i64)]]) -> F {
        F { kind: 2, tags: tags.to_vec(), parts: parts.iter().map(|p| p.to_vec()).collect() }
    }

    fn polygon_f(tags: &[(&'static str, V)], rings: &[&[(i64, i64)]]) -> F {
        F { kind: 3, tags: tags.to_vec(), parts: rings.iter().map(|p| p.to_vec()).collect() }
    }

    fn varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push((v as u8) | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    fn field_bytes(out: &mut Vec<u8>, field: u64, b: &[u8]) {
        varint(out, (field << 3) | 2);
        varint(out, b.len() as u64);
        out.extend_from_slice(b);
    }

    fn field_uint(out: &mut Vec<u8>, field: u64, v: u64) {
        varint(out, field << 3);
        varint(out, v);
    }

    fn field_packed(out: &mut Vec<u8>, field: u64, values: &[u64]) {
        let mut body = Vec::new();
        for &v in values {
            varint(&mut body, v);
        }
        field_bytes(out, field, &body);
    }

    /// A vector tile of `layers` (name, features), extent 4096: the fixture rail tiles.
    fn encode(layers: &[(&str, Vec<F>)]) -> Vec<u8> {
        let zigzag = |v: i64| ((v << 1) ^ (v >> 63)) as u64;
        let mut tile = Vec::new();
        for (name, features) in layers {
            let (mut keys, mut values): (Vec<&str>, Vec<Vec<u8>>) = (Vec::new(), Vec::new());
            let mut layer = Vec::new();
            field_uint(&mut layer, 15, 2);
            field_bytes(&mut layer, 1, name.as_bytes());
            for f in features {
                let mut tags = Vec::new();
                for &(k, v) in &f.tags {
                    let ki = keys.iter().position(|&x| x == k).unwrap_or_else(|| {
                        keys.push(k);
                        keys.len() - 1
                    });
                    let mut value = Vec::new();
                    match v {
                        V::S(s) => field_bytes(&mut value, 1, s.as_bytes()),
                        V::U(n) => field_uint(&mut value, 5, n),
                        V::B(b) => field_uint(&mut value, 7, u64::from(b)),
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
                field_packed(&mut feature, 2, &tags);
                field_uint(&mut feature, 3, f.kind);
                field_packed(&mut feature, 4, &geometry);
                field_bytes(&mut layer, 2, &feature);
            }
            for k in &keys {
                field_bytes(&mut layer, 3, k.as_bytes());
            }
            for v in &values {
                field_bytes(&mut layer, 4, v);
            }
            field_uint(&mut layer, 5, 4096);
            field_bytes(&mut tile, 3, &layer);
        }
        tile
    }

    fn never() -> bool {
        false
    }

    /// Stockholm's rail tile at zoom 12 (59.33° N, 18.07° E), where the flat map's ground rails
    /// come from.
    const STOCKHOLM: TileIndex = TileIndex::new(2253, 1208, 12);

    /// Metres across [`STOCKHOLM`] at its middle (`bridges.ts`' `tileMetres`).
    fn tile_metres(tile: TileIndex) -> f64 {
        let n = 2f64.powi(tile.z as i32);
        let lat = (std::f64::consts::PI * (1.0 - 2.0 * (f64::from(tile.y) + 0.5) / n)).sinh().atan();
        EARTH_CIRCUMFERENCE_M * lat.cos() / n
    }

    /// The colours and widths of segments, each run once, in order.
    fn paints(segments: &[PathSegment]) -> Vec<(Rgba, f32)> {
        let mut out: Vec<(Rgba, f32)> = Vec::new();
        for s in segments {
            if out.last() != Some(&(s.color, s.width)) {
                out.push((s.color, s.width));
            }
        }
        out
    }

    const TRACK: Rgba = ground_paint(35, [255, 255, 255]);
    const BALLAST: Rgba = ground_paint(38, [255, 255, 255]);
    const PEDESTRIAN: Rgba = ground_paint(36, [255, 255, 255]);

    /// Every kind of OpenRailwayMap track, each a straight line across the tile at its own height,
    /// with tags as the backend's tiles carry them.
    fn every_kind() -> Vec<u8> {
        let (s, u, b) = (V::S, V::U, V::B);
        let row = |k: i64| -> Vec<(i64, i64)> { vec![(400, 300 + 400 * k), (3600, 300 + 400 * k)] };
        let rows: Vec<Vec<(i64, i64)>> = (0..9).map(row).collect();
        let present = ("state", s("present"));
        encode(&[(
            RAIL_LINES,
            vec![
                // 0: a main line
                line_f(&[("feature", s("rail")), present, ("usage", s("main")), ("maxspeed", u(160))], &[&rows[0]]),
                // 1: a siding (a railway's service track: on ballast as a railway)
                line_f(&[("feature", s("rail")), present, ("service", s("siding"))], &[&rows[1]]),
                // 2, 3: light rail and a subway above ground: transit track
                line_f(&[("feature", s("light_rail")), present], &[&rows[2]]),
                line_f(&[("feature", s("subway")), present, ("maxspeed", u(70))], &[&rows[3]]),
                // 4: a tram: its rails are the street's (the base map's)
                line_f(&[("feature", s("tram")), present], &[&rows[4]]),
                // 5: a main line in a tunnel: nothing on the ground
                line_f(&[("feature", s("rail")), present, ("tunnel", b(true))], &[&rows[5]]),
                // 6: razed, not drawn; 7: not a railway the map draws
                line_f(&[("feature", s("rail")), ("state", s("razed"))], &[&rows[6]]),
                line_f(&[("feature", s("miniature")), present], &[&rows[7]]),
                // 8: narrow gauge with no state: a railway
                line_f(&[("feature", s("narrow_gauge"))], &[&rows[8]]),
            ],
        )])
    }

    #[test]
    fn ordinary_track_is_2_6_m_on_1_m_of_ballast_transit_2_2_m_on_0_6_m_tunnels_and_trams_nothing() {
        let bytes = every_kind();
        for flat in [true, false] {
            let t = rails_tile(STOCKHOLM, &bytes, flat, &never).unwrap();
            // the track: 2.6 m of rails on sleepers (the main line, the siding, the narrow gauge),
            // 2.2 m for light rail and the subway; nothing for the tram, the tunnel, the razed line
            // and the miniature railway
            assert_eq!(paints(&t.lines), [(TRACK, 2.6), (TRACK, 2.2), (TRACK, 2.6)], "flat {flat}");
            assert_eq!(t.lines.len(), 5);
            // its ballast either side: 1 m (4.6 m in all), 0.6 m for transit (3.4 m)
            assert_eq!(paints(&t.edges), [(BALLAST, 4.6), (BALLAST, 3.4), (BALLAST, 4.6)]);
            assert_eq!(t.edges.len(), 5);
            // in the order of the tile's features, edging and track on the same lines
            let rows: Vec<f32> = t.lines.iter().map(|s| s.start[1]).collect();
            let rows_px: Vec<i64> = rows.iter().map(|v| (f64::from(*v) * 4096.0).round() as i64).collect();
            assert_eq!(rows_px, [300, 700, 1100, 1500, 3500]);
            assert_eq!(t.edges.iter().map(|s| s.start).collect::<Vec<_>>(), t.lines.iter().map(|s| s.start).collect::<Vec<_>>());
            for s in &t.lines {
                assert_eq!((s.start[0], s.end[0]), (400.0 / 4096.0, 3600.0 / 4096.0));
                assert_eq!((s.start[2], s.end[2]), (0.0, 0.0), "on the ground");
                assert_eq!(s.dash_array, [0.0, 0.0]);
                assert_eq!(s.types, START_CAP | END_CAP, "open, capped at both ends");
            }
            // no bridges in this tile
            assert!(t.bridge_shade.is_empty() && t.bridge_edges.is_empty() && t.bridge_lines.is_empty() && t.bridge_decks.is_empty());
            assert!(!t.is_empty() && t.byte_size() == 10 * size_of::<PathSegment>());
        }
        // the surfaces the ground's shader reads from the alpha: the railway's track and gravel
        assert_eq!((255 - TRACK[3], 255 - BALLAST[3]), (35, 38));
    }

    #[test]
    fn on_the_flat_map_a_bridge_rises_to_its_deck_over_its_shade_and_on_the_globe_lies_on_the_ground() {
        let (s, b) = (V::S, V::B);
        let present = ("state", s("present"));
        let bridge: &[(i64, i64)] = &[(1200, 2048), (2800, 2048)];
        // a bridge whose west end the tile cuts: no ramp there, the rest of it is the next tile's
        let cut: &[(i64, i64)] = &[(0, 3000), (1600, 3000)];
        let ground: &[(i64, i64)] = &[(0, 1000), (4096, 1000)];
        let bytes = encode(&[(
            RAIL_LINES,
            vec![
                line_f(&[("feature", s("rail")), present], &[ground]),
                line_f(&[("feature", s("rail")), present, ("bridge", b(true))], &[bridge]),
                line_f(&[("feature", s("light_rail")), present, ("bridge", b(true))], &[cut]),
                // a tram's bridge: no track, but its shade and its deck
                line_f(&[("feature", s("tram")), present, ("bridge", b(true))], &[&[(500, 3500), (900, 3500)]]),
            ],
        )]);
        let t = rails_tile(STOCKHOLM, &bytes, true, &never).unwrap();
        // on the ground only the line that is not a bridge
        assert_eq!(paints(&t.lines), [(TRACK, 2.6)]);
        assert_eq!(t.lines.len(), 1);
        assert_eq!(t.edges.len(), 1);
        // the bridges' shade: flat, every bridge, as wide as its edging and 2 m more
        assert_eq!(paints(&t.bridge_shade), [(BRIDGE_SHADE, 6.6), (BRIDGE_SHADE, 5.4), (BRIDGE_SHADE, 2.0)]);
        assert_eq!(t.bridge_shade.len(), 3, "the shade keeps the bridges' own points");
        assert!(t.bridge_shade.iter().all(|s| s.start[2] == 0.0 && s.end[2] == 0.0));
        // raised: the main line's 6 m deck, ramped up over 40 m from each end, in pieces of at
        // most 6 m; the light rail's ramp only at its east end
        let metres = tile_metres(STOCKHOLM);
        let long = 1600.0 / 4096.0 * metres;
        let pieces = (long / 6.0).ceil() as usize;
        let main: Vec<&PathSegment> = t.bridge_lines.iter().filter(|s| s.width == 2.6).collect();
        assert_eq!(main.len(), pieces);
        assert_eq!(main[0].start[2], 0.0);
        assert_eq!(main[pieces - 1].end[2], 0.0);
        let top = main.iter().map(|s| s.end[2]).fold(0.0, f32::max);
        assert_eq!(top, 6.0);
        assert!(main.iter().all(|s| (0.0..=6.0).contains(&s.start[2])));
        // up the ramp as a smoothstep of the way along its 40 m
        for s in main.iter().take_while(|s| (f64::from(s.start[0]) - 1200.0 / 4096.0) * metres < 40.0) {
            let k = (f64::from(s.start[0]) - 1200.0 / 4096.0) * metres / 40.0;
            assert!((f64::from(s.start[2]) - 6.0 * k * k * (3.0 - 2.0 * k)).abs() < 1e-3, "{k}: {}", s.start[2]);
        }
        let light: Vec<&PathSegment> = t.bridge_lines.iter().filter(|s| s.width == 2.2).collect();
        assert_eq!(light[0].start[2], 6.0, "the tile's edge: up already");
        assert_eq!(light[light.len() - 1].end[2], 0.0);
        // the raised edging under the raised track, the same pieces
        assert_eq!(paints(&t.bridge_edges), [(BALLAST, 4.6), (BALLAST, 3.4)]);
        assert_eq!(t.bridge_edges.len(), t.bridge_lines.len());
        // the decks: every bridge raised, transparent, and on a railway 0 m wide (the layer's
        // minimum width)
        assert_eq!(paints(&t.bridge_decks), [(NONE, 0.0)]);
        assert_eq!(t.bridge_decks.len(), t.bridge_lines.len() + (400.0 / 4096.0 * metres / 6.0).ceil() as usize);
        assert_eq!(t.passes().map(|(name, _)| name), ["edges", "lines", "bridge-shade", "bridge-edges", "bridge-lines", "bridge-decks"]);

        // the globe raises nothing: every line on the ground, a bridge as well
        let g = rails_tile(STOCKHOLM, &bytes, false, &never).unwrap();
        assert_eq!(paints(&g.lines), [(TRACK, 2.6), (TRACK, 2.2)]);
        assert_eq!(g.lines.len(), 3);
        assert!(g.lines.iter().all(|s| s.start[2] == 0.0));
        assert!(g.bridge_shade.is_empty() && g.bridge_lines.is_empty() && g.bridge_decks.is_empty());
    }

    #[test]
    fn platforms_stand_0_6_m_tall_in_the_pavement_and_a_platform_mapped_as_a_line_lies_flat() {
        let s = V::S;
        let tile = TileIndex::new(36056, 19247, PLATFORM_ZOOM);
        // an outline as MVT has it (clockwise on the screen, y down)
        let square: &[(i64, i64)] = &[(1000, 1000), (3000, 1000), (3000, 1400), (1000, 1400)];
        let bytes = encode(&[
            (PLATFORMS, vec![polygon_f(&[("railway", s("platform")), ("ref", s("2"))], &[square]), line_f(&[("railway", s("platform"))], &[&[(200, 3000), (3800, 3000)]])]),
            // the tile's other layer is not read
            ("standard_railway_platform_edges", vec![line_f(&[("railway", s("platform_edge"))], &[&[(0, 0), (4096, 4096)]])]),
            (RAIL_LINES, vec![line_f(&[("feature", s("rail"))], &[&[(0, 2000), (4096, 2000)]])]),
        ]);
        let t = platforms_tile(tile, &bytes, &never).unwrap();
        let h = PLATFORM_M as f32;
        assert_eq!(h, 0.6);
        // four sides of four vertices, then the top: the ring's five points (its first repeated)
        assert_eq!(t.vertices.len(), 4 * 4 + 5);
        assert_eq!(t.indices.len(), 4 * 6 + 2 * 3);
        assert!(t.vertices.iter().all(|v| v.paint == area::PLATFORM));
        assert_eq!(255 - area::PLATFORM[3], 9, "the pavement");
        let (sides, top) = t.vertices.split_at(16);
        let centre = [2000.0f32 / 4096.0, 1200.0 / 4096.0];
        for side in sides.chunks(4) {
            let heights: Vec<f32> = side.iter().map(|v| v.position[2]).collect();
            assert_eq!(heights, [0.0, 0.0, h, h], "from the ground to 0.6 m");
            let n = side[0].normal;
            assert!(side.iter().all(|v| v.normal == n));
            assert_eq!(n[2], 0.0);
            assert!((n[0].hypot(n[1]) - 1.0).abs() < 1e-6, "unit length");
            // out of the platform: east and north against the middle of the side (v runs south)
            let mid = [(side[0].position[0] + side[1].position[0]) / 2.0, (side[0].position[1] + side[1].position[1]) / 2.0];
            let out = [mid[0] - centre[0], -(mid[1] - centre[1])];
            assert!(n[0] * out[0] + n[1] * out[1] > 0.0, "{n:?} at {mid:?}");
        }
        assert!(top.iter().all(|v| v.position[2] == h && v.normal == [0.0, 0.0, 1.0]));
        // the top covers the platform: two triangles of the outline's corners
        let area_of = |tri: &[u32]| {
            let p = |i: u32| t.vertices[i as usize].position;
            let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
            ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0
        };
        let covered: f32 = t.indices[24..].chunks(3).map(area_of).sum();
        assert!((covered - (2000.0 * 400.0) / (4096.0 * 4096.0)).abs() < 1e-7, "{covered}");
        // the line: flat, 5 m of pavement, no edging; the rail tile's layer is not this tile's
        assert_eq!(paints(&t.lines.lines), [(PEDESTRIAN, 5.0)]);
        assert_eq!(t.lines.lines.len(), 1);
        assert!(t.lines.edges.is_empty() && t.lines.bridge_lines.is_empty());
        assert!(!t.is_empty());
        assert_eq!(t.byte_size(), t.vertices.len() * 28 + t.indices.len() * 4 + size_of::<PathSegment>());
    }

    #[test]
    fn a_hole_s_sides_face_into_it() {
        let tile = TileIndex::new(36056, 19247, PLATFORM_ZOOM);
        let outer: &[(i64, i64)] = &[(0, 0), (4000, 0), (4000, 4000), (0, 4000)];
        // a hole turns the other way
        let hole: &[(i64, i64)] = &[(1000, 1000), (1000, 3000), (3000, 3000), (3000, 1000)];
        let bytes = encode(&[(PLATFORMS, vec![polygon_f(&[], &[outer, hole])])]);
        let t = platforms_tile(tile, &bytes, &never).unwrap();
        assert_eq!(t.vertices.len(), 8 * 4 + 10);
        let centre = [2000.0 / 4096.0, 2000.0 / 4096.0];
        for (k, side) in t.vertices[..32].chunks(4).enumerate() {
            let mid = [(side[0].position[0] + side[1].position[0]) / 2.0, (side[0].position[1] + side[1].position[1]) / 2.0];
            let away = [mid[0] - centre[0], -(mid[1] - centre[1])];
            let n = side[0].normal;
            // the outline's sides face away from the middle, the hole's toward it (into the hole)
            let facing = n[0] * away[0] + n[1] * away[1];
            if k < 4 { assert!(facing > 0.0, "outline {k}") } else { assert!(facing < 0.0, "hole {k}") }
        }
        // the top leaves the hole open
        let covered: f32 = t.indices[48..]
            .chunks(3)
            .map(|tri| {
                let p = |i: u32| t.vertices[i as usize].position;
                let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
                ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0
            })
            .sum();
        assert!((covered - (16e6 - 4e6) / (4096.0 * 4096.0)).abs() < 1e-6, "{covered}");
    }

    #[test]
    fn the_globe_cuts_a_line_at_5_degrees_and_closes_a_loop_the_flat_map_keeps_both_open() {
        let (s, present) = (V::S, ("state", V::S("present")));
        // zoom 7 tile 71/37: 19.6875° to 22.5° E, across the 20° meridian
        let tile = TileIndex::new(71, 37, 7);
        let across: &[(i64, i64)] = &[(200, 2000), (3800, 2000)];
        let a_loop: &[(i64, i64)] = &[(3000, 3000), (3500, 3000), (3500, 3500), (3000, 3000)];
        let bytes = encode(&[(
            RAIL_LINES,
            vec![line_f(&[("feature", s("rail")), present], &[across]), line_f(&[("feature", s("rail")), present], &[a_loop])],
        )]);
        let globe = rails_tile(tile, &bytes, false, &never).unwrap();
        // the line in two at the meridian, the loop joined round (no caps)
        let (line, ring): (Vec<&PathSegment>, Vec<&PathSegment>) = globe.lines.iter().partition(|s| s.start[1] < 0.6);
        assert_eq!(line.len(), 2);
        let meridian = LocalMercator::tile(tile).from_world(DVec2::from(lng_lat_to_world([20.0, 0.0])))[0];
        assert!((f64::from(line[0].end[0]) - meridian).abs() < 1e-6);
        assert_eq!((line[0].types, line[1].types), (START_CAP, END_CAP));
        assert!(ring.len() >= 3 && ring.iter().all(|s| s.types == 0), "{:?}", ring.iter().map(|s| s.types).collect::<Vec<_>>());
        // the flat map's binary lines: as they are, open
        let flat = rails_tile(tile, &bytes, true, &never).unwrap();
        assert_eq!(flat.lines.len(), 1 + 3);
        assert_eq!(flat.lines[0].types, START_CAP | END_CAP);
        assert_eq!((flat.lines[1].types, flat.lines[3].types), (START_CAP, END_CAP));
    }

    #[test]
    fn a_cancelled_job_stops_and_an_empty_tile_draws_nothing() {
        let bytes = every_kind();
        assert_eq!(rails_tile(STOCKHOLM, &bytes, true, &|| true), Err(Cancelled));
        assert_eq!(platforms_tile(STOCKHOLM, &encode(&[(PLATFORMS, vec![polygon_f(&[], &[&[(0, 0), (9, 0), (9, 9)]])])]), &|| true), Err(Cancelled));
        assert!(rails_tile(STOCKHOLM, &[], true, &never).unwrap().is_empty());
        assert!(platforms_tile(STOCKHOLM, &[], &never).unwrap().is_empty());
    }

    #[test]
    fn the_tiles_and_their_limits_are_today_s() {
        assert_eq!((RAIL_TILES.min_zoom, RAIL_TILES.max_zoom, RAIL_TILES.tile_size), (Some(7.0), Some(12.0), 512.0));
        assert_eq!((PLATFORM_TILES.min_zoom, PLATFORM_TILES.max_zoom), (Some(15.0), Some(15.0)));
        assert_eq!(platform_lod(), Lod { levels: vec![15], bias: 0.0, max_distance: 4000.0 });
        assert_eq!((RAILS_MIN_PIXELS, PLATFORMS_MIN_PIXELS), (0.6, 0.0));
        assert_eq!(BRIDGE_SHADE, [16, 20, 14, 70]);
        // OpenRailwayMap's bridges to the rail map's deck, the rest as OpenMapTiles has them
        let p = |v: Value| v.as_object().unwrap().clone();
        let none = |_: &str| None;
        assert_eq!(deck_of(&p(serde_json::json!({ "layerName": RAIL_LINES, "bridge": true })), &none), 6.0);
        assert_eq!(deck_of(&p(serde_json::json!({ "layerName": RAIL_LINES, "brunnel": "bridge" })), &none), 0.0);
        assert_eq!(deck_of(&p(serde_json::json!({ "layerName": PLATFORMS, "brunnel": "bridge", "layer": 2 })), &none), 12.0);
    }
}
