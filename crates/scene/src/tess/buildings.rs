//! Plain buildings, the CPU side (item B7; doc 01 §9.3; doc 04 §7.1, §7.6, §13): OpenFreeMap's
//! `building` layer as today's app draws it on the dark and satellite maps (`cityBuildingLayer`,
//! G:1039-1092, through `BuildingLayer`, `building-layer.ts`, an `LodMVTLayer`, `lod-tileset.ts`).
//! The GPU side is `rustradar_map::layers::buildings`. The realistic map's buildings (item R4:
//! each building's look, walls with their bays and storeys, Streets GL's tiles) are the second
//! half of this file, from [`tessellate_realistic`]; their GPU side is
//! `rustradar_map::layers::buildings_realistic`.
//!
//! What today's app does, and so this module:
//! - **Which tiles.** Only on the flat map (zoom > 12) and with the *buildings* setting on
//!   (G:2694). The tiles are picked by their distance from the camera, [`city_lod`] (`CITY_LOD`,
//!   levels 13, 14 and 16, bias 0.5, nothing past 25 km), each zoom from its own source: on the
//!   dark and satellite maps OpenFreeMap's `building` layer at zooms 13 and 14 ([`SOURCE_ZOOMS`],
//!   G:1047-1053); zoom 16 is Streets GL's, a source only the realistic map has, so on these maps
//!   a tile of 16 comes back empty (`LodMVTLayer.getTileData` resolves `null`) and its coarser
//!   ancestor, if one is cached, stands in (`keepCoarserUntilIn`). Requests: 8 at once
//!   ([`MAX_REQUESTS`]).
//! - **Far tiles flat, near tiles standing.** A tile under [`CITY_CLOSE_ZOOM`] (14), the far
//!   zoom-13 ones, is drawn flat (`extruded: false`): each footprint on the ground in the
//!   building colour, unlit, no shadows. A tile of 14 is extruded: walls and a flat roof at the
//!   building's height, lit, and its shadows cast on the ground ([`tile_shadows`]).
//! - **Heights** ([`building_height`], `buildingHeight`, `BL:36-43`): `hide_3d ? 0 :
//!   Number(render_height ?? 6)` metres, read as deck reads loaders.gl's binary tiles (the flat
//!   map's): a numeric property a feature lacks reads 0 when the tile's other features have it as
//!   a number, so `?? 6` applies only when no feature of the tile has a numeric `render_height`
//!   ([`Heights`]). Buildings stand on the ground (`render_min_height` is not used). The heights
//!   go to the GPU as deck's `elevations` attribute does, as `f32`.
//! - **Rings** as loaders.gl's binary MVT reads them (`loadFlatGeometry`, `classifyRingsFlat`): in
//!   tile units (the layer's numbers over its `extent`, `f32` as its typed arrays hold them), each
//!   ring closed by its ClosePath (the first point repeated), rings of no area dropped, grouped
//!   into polygons by the turn of the first ring with an area (the same turn starts a polygon, the
//!   other is a hole of the polygon before it). Each polygon is one building of deck's
//!   SolidPolygonLayer, with its feature's properties.
//! - **Extrusion** as deck's SolidPolygonLayer draws it with `_normalize: false` (GeoJsonLayer's
//!   binary polygons) and `_windingOrder: 'CCW'` (`BUILDING_SUBLAYER_PROPS`, G:979): the roof is
//!   the polygon's earcut triangles at the building's height, normal up; each edge of each ring
//!   is a wall from the ground to the height, its normal `(dv, du)` east and north for the edge
//!   `(du, dv)` in tile units (`v` south): outward for an outline that turns counter-clockwise in
//!   the tile's numbers, as MVT's outlines do, and into a hole for a hole. Lighting is per vertex
//!   (Gouraud: SolidPolygonLayer's `gouraudMaterial`), so a wall is four vertices of its own and a
//!   roof shares its ring's.
//! - **Shadows** ([`tile_shadows`], `tileShadows`, `BL:95-129`): per extruded tile and per sun,
//!   `crate::building_shadow::building_shadows` of its footprints, the sun taken at the tile's
//!   middle, in the tile's units; alpha `0.35 · sunlight(up)`, none under
//!   `rustradar_core::sun::SHADOW_MIN_ALPHA` or with the sun down; colour [`SHADOW_COLOR`]
//!   ([`shadow_color`]). The sun is held until it has moved a quarter of a degree
//!   ([`ShadowSun`], `shadowSun`, G:981-994).
//!
//! Deviations, invisible: a wall of no length or no height is left out (deck draws it, with no
//! area); a ring whose ClosePath is missing, or of fewer than three points, is left out
//! (`rustradar_core::mvt::Feature::rings`; loaders.gl keeps one of three points or more without
//! its ClosePath, unclosed); the roof's triangles are `crate::geom::earcut`'s, the earcut 2.2.4
//! deck runs (doc 05 §13.1). On the flat map
//! deck also cuts each tile's buildings at the tile's square (MVTLayer's `ClipExtension`); that is
//! the layer's (the map side's), as is the stencil rule of the shadows.

use std::f64::consts::PI;

use bytemuck::{Pod, Zeroable};
use rustradar_core::mvt::{self, Feature, GeomType, Value};
use rustradar_core::num::{js_round, to_int32};
use rustradar_core::sun::{SHADOW_MIN_ALPHA, local_sun, sun_direction, sunlight};

use crate::building_shadow::{self, BUILDING_SHADOW_ALPHA, shadow_step};
use crate::geom::earcut::earcut;
use crate::lod::Lod;
use crate::roofs::pitched;
use crate::streets_gl::{Facade, Properties, RoofMaterial, SglBuilding, sgl_building};
use crate::tiles::{TileIndex, osm_tile_to_lng_lat};

/// The MVT layer the buildings are read from (`BUILDING_LOAD_OPTIONS`, G:971).
pub const SOURCE_LAYER: &str = "building";

/// The zooms OpenFreeMap serves the buildings at (G:1049): the others of [`city_lod`] have no
/// source on the dark and satellite maps.
pub const SOURCE_ZOOMS: [u32; 2] = [13, 14];

/// From this tile zoom on the buildings stand up and cast shadows; the coarser (far) tiles are
/// drawn flat (`CITY_CLOSE_ZOOM`, G:1042).
pub const CITY_CLOSE_ZOOM: u32 = 14;

/// Requests at once (`maxRequests: 8`, G:1066).
pub const MAX_REQUESTS: usize = 8;

/// The buildings' colour on the dark and satellite maps (`BUILDING_COLOR`, G:978).
pub const BUILDING_COLOR: [u8; 3] = [58, 68, 84];

/// The building material (`BUILDING_MATERIAL`, G:972-977): ambient, diffuse, shininess and the
/// specular colour (0–255).
pub const BUILDING_AMBIENT: f64 = 0.42;
pub const BUILDING_DIFFUSE: f64 = 0.6;
pub const BUILDING_SHININESS: f64 = 16.0;
pub const BUILDING_SPECULAR: [f64; 3] = [60.0, 66.0, 80.0];

/// The shadows' colour on the dark and satellite maps (`SHADOW_COLOR`, G:358: the app's navy ink).
pub const SHADOW_COLOR: [u8; 3] = [4, 8, 16];

/// How dark the shadows are against their own (the dark and satellite maps': 1, `shadowStrength`'s
/// default).
pub const SHADOW_STRENGTH: f64 = 1.0;

/// A building's height when its tile gives none, metres (`buildingHeight`).
pub const DEFAULT_HEIGHT_M: f64 = 6.0;

/// Main-thread time a frame may spend recasting shadows once the sun has moved; at least one tile
/// is recast each frame (`RECAST_BUDGET_MS`, `BL:313`).
pub const RECAST_BUDGET_MS: f64 = 4.0;

/// Web Mercator's equator, metres (`EARTH_CIRCUMFERENCE`, `BL:24`).
const EARTH_CIRCUMFERENCE: f64 = 2.0 * PI * 6_378_137.0;

/// `CITY_LOD` (G:1039): the buildings' tiles by their distance from the camera.
pub fn city_lod() -> Lod {
    Lod { levels: vec![13, 14, 16], bias: 0.5, max_distance: 25_000.0 }
}

/// `SHADOW_SUN_STEP` (G:985): the cosine of a quarter of a degree; the shadows are recast once the
/// sun has moved that far.
pub fn shadow_sun_step() -> f64 {
    ((0.25 * PI) / 180.0).cos()
}

/// One vertex of a building mesh: `(u, v, h)` in the tile's frame (tile units, `v` south, metres
/// up) and its normal east, north, up (not unit length). 24 bytes, `#[repr(C)]`, for the map's
/// building pipeline (`bytemuck::cast_slice`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct BuildingVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

/// A tile's footprints in the layout of loaders.gl's binary polygons
/// ([`crate::building_shadow::Footprints`]): `x, y` tile units per vertex, each ring closed; where
/// each polygon and each ring starts, each list ending with the vertex count; and each polygon's
/// height (what its shadow is cast from, `castHeight`: OpenFreeMap's is the drawn height).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Footprints {
    pub positions: Vec<f32>,
    pub polygon_starts: Vec<u32>,
    pub ring_starts: Vec<u32>,
    pub heights: Vec<f32>,
}

impl Footprints {
    /// How many polygons (buildings) it holds.
    pub fn len(&self) -> usize {
        self.heights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }

    /// As [`building_shadow::building_shadows`] takes them.
    pub fn as_shadow_input(&self) -> building_shadow::Footprints<'_, f32> {
        building_shadow::Footprints { positions: &self.positions, size: 2, polygon_starts: &self.polygon_starts, ring_starts: &self.ring_starts }
    }

    /// The bytes it holds.
    pub fn byte_size(&self) -> usize {
        size_of_val(self.positions.as_slice())
            + size_of_val(self.polygon_starts.as_slice())
            + size_of_val(self.ring_starts.as_slice())
            + size_of_val(self.heights.as_slice())
    }
}

/// One tile of buildings, tessellated.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BuildingTile {
    /// Whether its buildings stand (zoom ≥ [`CITY_CLOSE_ZOOM`]): walls and lit roofs, shadows; else
    /// footprints flat on the ground, unlit.
    pub extruded: bool,
    pub vertices: Vec<BuildingVertex>,
    /// Three indices into [`Self::vertices`] per triangle, building after building, each one's
    /// walls then its roof.
    pub indices: Vec<u32>,
    /// What its shadows are cast from (extruded tiles only; empty for a flat one).
    pub footprints: Footprints,
}

impl BuildingTile {
    /// Whether it draws nothing.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// The bytes it holds (for the tile cache's budget).
    pub fn byte_size(&self) -> usize {
        size_of_val(self.vertices.as_slice()) + size_of_val(self.indices.as_slice()) + self.footprints.byte_size()
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

/// JavaScript's `Number(value)` of a property that is there: a boolean as 1 or 0, a string as
/// `StringToNumber` reads it ([`string_to_number`]).
fn number(v: &Value) -> f64 {
    match *v {
        Value::Str(s) => string_to_number(s),
        Value::Num(n) => n,
        Value::Bool(b) => f64::from(u8::from(b)),
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

/// How a tile's features read the two properties a height needs, as loaders.gl's binary tiles hold
/// them (`extractNumericPropTypes`, `fillNumericProperties`): a property every feature that has it
/// gives as a finite number is numeric, and a feature without it reads 0; any other property is
/// kept as it is, and a feature without it reads `undefined`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Heights {
    /// `render_height` is numeric in the tile.
    pub numeric_height: bool,
}

impl Heights {
    /// What the features of a `building` layer make of `render_height`.
    pub fn of(layer: &mvt::Layer<'_>) -> Self {
        let mut seen = false;
        let mut numeric = true;
        for feature in layer.features() {
            if let Some(v) = feature.get("render_height") {
                seen = true;
                numeric &= matches!(*v, Value::Num(n) if n.is_finite());
            }
        }
        Heights { numeric_height: seen && numeric }
    }
}

/// A building's height as drawn, metres (`buildingHeight`, `BL:36-43`, for OpenFreeMap's
/// buildings): `hide_3d ? 0 : Number(render_height ?? 6)`, with the tile's properties read as
/// `heights` says.
pub fn building_height(feature: &Feature<'_>, heights: Heights) -> f64 {
    if truthy(feature.get("hide_3d")) {
        return 0.0;
    }
    match feature.get("render_height") {
        Some(v) => number(v),
        None if heights.numeric_height => 0.0,
        None => DEFAULT_HEIGHT_M,
    }
}

/// math.gl's `getPolygonSignedArea` of a closed ring of `x, y` pairs (its sign is a turn; only its
/// sign and whether it is 0 matter here).
fn ring_area(ring: &[[f64; 2]]) -> f64 {
    let n = ring.len();
    if n < 2 {
        return 0.0;
    }
    let mut area = 0.0;
    let mut j = n - 1;
    for i in 0..n {
        area += (ring[i][0] - ring[j][0]) * (ring[i][1] + ring[j][1]);
        j = i;
    }
    area / 2.0
}

/// A feature's polygons as loaders.gl's binary tiles hold them (`loadFlatGeometry`,
/// `classifyRingsFlat`): its rings closed, in tile units (`f32`), those of no area dropped, grouped
/// by the turn of the first ring with an area. A single ring is a polygon whatever its area.
pub fn binary_polygons(feature: &Feature<'_>, extent: f64) -> Vec<Vec<Vec<[f32; 2]>>> {
    let rings: Vec<Vec<[f64; 2]>> = feature
        .rings()
        .into_iter()
        .map(|ring| {
            let first = ring[0];
            ring.iter().chain(std::iter::once(&first)).map(|&(x, y)| [x, y]).collect()
        })
        .collect();
    let units = |ring: &[[f64; 2]]| -> Vec<[f32; 2]> { ring.iter().map(|&[x, y]| [(x / extent) as f32, (y / extent) as f32]).collect() };
    if rings.len() <= 1 {
        return rings.iter().map(|r| vec![units(r)]).collect();
    }
    let mut polygons: Vec<Vec<Vec<[f32; 2]>>> = Vec::new();
    let mut polygon: Vec<Vec<[f32; 2]>> = Vec::new();
    let mut ccw = None;
    for ring in &rings {
        let area = ring_area(ring);
        if area == 0.0 {
            continue;
        }
        let ccw = *ccw.get_or_insert(area < 0.0);
        if ccw == (area < 0.0) {
            if !polygon.is_empty() {
                polygons.push(std::mem::take(&mut polygon));
            }
            polygon.push(units(ring));
        } else {
            polygon.push(units(ring));
        }
    }
    if !polygon.is_empty() {
        polygons.push(polygon);
    }
    polygons
}

/// Builds one tile.
struct Builder {
    extruded: bool,
    tile: BuildingTile,
}

impl Builder {
    /// One building: its rings (closed, tile units), `height` metres tall.
    fn building(&mut self, rings: &[Vec<[f32; 2]>], height: f32) {
        let out = &mut self.tile;
        if self.extruded {
            let f = &mut out.footprints;
            f.polygon_starts.push((f.positions.len() / 2) as u32);
            for ring in rings {
                f.ring_starts.push((f.positions.len() / 2) as u32);
                f.positions.extend(ring.iter().flatten());
            }
            f.heights.push(height);
            // the walls: each edge from the ground to the height (deck's side model; the ring's
            // last vertex, its first repeated, starts no wall)
            if height > 0.0 {
                for ring in rings {
                    for edge in ring.windows(2) {
                        let ([u0, v0], [u1, v1]) = (edge[0], edge[1]);
                        let (du, dv) = (u1 - u0, v1 - v0);
                        if du == 0.0 && dv == 0.0 {
                            continue;
                        }
                        let normal = [dv, du, 0.0];
                        let base = out.vertices.len() as u32;
                        for [u, v, h] in [[u0, v0, 0.0], [u1, v1, 0.0], [u1, v1, height], [u0, v0, height]] {
                            out.vertices.push(BuildingVertex { position: [u, v, h], normal });
                        }
                        out.indices.extend([0, 1, 2, 0, 2, 3].map(|k| base + k));
                    }
                }
            }
        }
        // the roof (flat tiles: the footprint on the ground), earcut's triangles of the polygon
        let top = if self.extruded { height } else { 0.0 };
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
        for ring in rings {
            for &[u, v] in ring {
                out.vertices.push(BuildingVertex { position: [u, v, top], normal: [0.0, 0.0, 1.0] });
            }
        }
        out.indices.extend(triangles.into_iter().map(|i| base + i as u32));
    }
}

/// The buildings of one OpenFreeMap tile (`bytes`, the tile as served, decompressed) at `tile`:
/// extruded when its zoom is at least `close_from_zoom` ([`CITY_CLOSE_ZOOM`]), flat otherwise.
/// `cancelled` is asked between features; once it says so the work stops.
pub fn tessellate(tile: TileIndex, bytes: &[u8], close_from_zoom: u32, cancelled: &dyn Fn() -> bool) -> Result<BuildingTile, Cancelled> {
    let extruded = tile.z >= close_from_zoom;
    let mut b = Builder { extruded, tile: BuildingTile { extruded, ..Default::default() } };
    for layer in mvt::layers_named(bytes, &[SOURCE_LAYER]) {
        let heights = Heights::of(&layer);
        for feature in layer.features() {
            if cancelled() {
                return Err(Cancelled);
            }
            if feature.kind != GeomType::Polygon {
                continue;
            }
            // deck's `elevations` attribute: f32
            let height = building_height(&feature, heights) as f32;
            for polygon in binary_polygons(&feature, layer.extent) {
                b.building(&polygon, height);
            }
        }
    }
    let f = &mut b.tile.footprints;
    if b.tile.extruded {
        f.polygon_starts.push((f.positions.len() / 2) as u32);
        f.ring_starts.push((f.positions.len() / 2) as u32);
    }
    Ok(b.tile)
}

/// A tile's shadows for one sun: triangles on the ground in its units, and their alpha.
#[derive(Debug, Clone, PartialEq)]
pub struct TileShadows {
    /// `x, y` tile units per vertex.
    pub positions: Vec<f32>,
    pub indices: Vec<u32>,
    /// `BUILDING_SHADOW_ALPHA · sunlight(up)` at the tile's middle.
    pub alpha: f64,
}

/// The middle of a tile, `[lon, lat]`: deck's tile `bbox` (`tileToBoundingBox`) halved.
pub fn tile_middle(tile: TileIndex) -> [f64; 2] {
    let [west, north] = osm_tile_to_lng_lat(f64::from(tile.x), f64::from(tile.y), tile.z);
    let [east, south] = osm_tile_to_lng_lat(f64::from(tile.x) + 1.0, f64::from(tile.y) + 1.0, tile.z);
    [(west + east) / 2.0, (north + south) / 2.0]
}

/// The shadows of a tile's buildings under `sun` (the sun's direction from the Earth's centre,
/// `rustradar_core::sun::sun_direction`), in the tile's units (`tileShadows`, `BL:95-129`): `None`
/// when the sun is too low (alpha under `SHADOW_MIN_ALPHA`) or down, or nothing casts one.
pub fn tile_shadows(tile: TileIndex, footprints: &Footprints, sun: [f64; 3]) -> Option<TileShadows> {
    let [lon, lat] = tile_middle(tile);
    let to_sun = local_sun(sun, lat, lon);
    let alpha = BUILDING_SHADOW_ALPHA * sunlight(to_sun[2]);
    let step = shadow_step(to_sun);
    let step = match step {
        Some(step) if alpha >= SHADOW_MIN_ALPHA => step,
        _ => return None,
    };
    if footprints.polygon_starts.len() < 2 {
        return None;
    }
    // a tile's width in metres on the ground, at its middle
    let tile_metres = (EARTH_CIRCUMFERENCE * (lat * PI / 180.0).cos()) / 2f64.powi(tile.z as i32);
    let mesh = building_shadow::building_shadows(&footprints.as_shadow_input(), &footprints.heights, [step[0] / tile_metres, -step[1] / tile_metres])?;
    Some(TileShadows { positions: mesh.positions, indices: mesh.indices, alpha })
}

/// The shadow layer's colour (`[...shadowColor, round(min(1, alpha · strength) · 255)]`).
pub fn shadow_color(alpha: f64, strength: f64) -> [u8; 4] {
    let a = js_round(js_min(1.0, alpha * strength) * 255.0);
    [SHADOW_COLOR[0], SHADOW_COLOR[1], SHADOW_COLOR[2], a.clamp(0.0, 255.0) as u8]
}

/// `Math.min(a, b)` (NaN stays NaN).
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}

/// The sun the buildings' shadows are cast with (`shadowSun`, G:981-994): the real sun, held until
/// it has moved a quarter of a degree (about once a minute), so the shadows are not recast every
/// frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ShadowSun {
    held: Option<[f64; 3]>,
}

impl ShadowSun {
    /// The sun to cast with at `now_ms` (the feed clock): the one held, or the sun now once it has
    /// moved [`shadow_sun_step`] from it.
    pub fn at(&mut self, now_ms: f64) -> [f64; 3] {
        let sun = sun_direction(now_ms);
        match self.held {
            Some(held) if sun[0] * held[0] + sun[1] * held[1] + sun[2] * held[2] >= shadow_sun_step() => held,
            _ => {
                self.held = Some(sun);
                sun
            }
        }
    }

    /// The sun held, if any.
    pub fn held(&self) -> Option<[f64; 3]> {
        self.held
    }
}

// ------------------------------------------------------------------ the realistic map's buildings
//
// Item R4 (doc 04 §7.1–7.3; `building-layer.ts`'s `wallColoursOf`, `styleAt`, `buildingHeight`,
// `castHeight`, `lookOf`; `realistic.ts`'s `buildingStyle`, `roofColour`, `hash`, `pickWeighted`,
// and the vertex half of its `rbuildInject`): `buildings-city-realistic` (G:1056-1092), drawn by
// `rustradar_map`'s `layers::buildings_realistic` through `shaders/building.wgsl`.
//
// What today's app does, and so this part ([`tessellate_realistic`]):
// - **Sources.** OpenFreeMap's `building` layer at zooms 13 and 14, Streets GL's `buildings` at its
//   zoom 16 ([`REALISTIC_SOURCE_ZOOMS`], `cityBuildingSources`, G:1047-1053), by `CITY_LOD`
//   ([`city_lod`]). A feature's properties are read as loaders.gl's binary tiles hand them to the
//   accessors, `layerName` included.
// - **Heights** ([`realistic_height`], `buildingHeight`): OpenFreeMap's as the plain buildings';
//   Streets GL's from `scene::streets_gl::sgl_building`: a roof on posts (`buildingType` "roof")
//   none, else up to its eaves where its roof is pitched (`roofs::pitched`, the roof mesh is item
//   R6's) and half way up the roof otherwise. Every building stands on the ground. What its shadow
//   is cast from ([`cast_height`], `castHeight`, item R5's): OpenFreeMap's the same, Streets GL's
//   half way up its roof (none for a roof on posts).
// - **Each building's look** ([`building_style`], `buildingStyle`): picked by a seed from the
//   middle of its outline in the world ([`style_seed`], `styleAt`: the mean of the polygon's
//   vertices, every ring's closing point included, at zoom-23 precision), so it is the same in a
//   zoom-14 and a zoom-16 tile and each time it loads; its façade and roof as Streets GL maps them
//   (`lookOf`, [`sgl_look`]) or picked by the seed and the height, its wall colour the one mapped
//   (`colour` in OpenMapTiles, Streets GL's `color`) toned down, else one of its façade's
//   [`WALL_COLOURS`] with a little jitter. deck carries the façade and roof in the colour's alpha
//   (`255 − (façade + 5·roof)`); here a vertex carries the RGBA as deck's `fillColors` has it
//   ([`RealisticVertex::tint`]) and the façade and roof explicitly ([`RealisticVertex::style`],
//   doc 04 §16.1).
// - **Walls as explicit quads** (doc 04 §16.3; deck's instanced sides): each ring edge a quad from
//   the ground to the height, its normal out of the outline as the plain buildings' (deck's side
//   normal), and its texture coordinates worked out here as `rbuildInject`'s vertex part does on
//   the GPU: whole 4 m bays to the wall (`round(length / 4)`, the wall's own length in metres;
//   none: the plain wall, `length / 4` across), whole 3.3 m storeys to the building (at least
//   one), `uv = (along · bays, up · storeys)`. Roofs flat at the height, earcut's triangles as the
//   plain buildings'; their texture lies fixed to the world (the shader's `rl_t`).
// - **Far tiles** (zoom 13, under [`CITY_CLOSE_ZOOM`]): the footprints flat on the ground in the
//   building's colour, alpha `255 − code` as deck's `fillColors` has it, no walls, unlit (the
//   building shader needs `extruded`): what the plan's section 7 decided ("as today").
//
// Not here: Overture's heights for Streets GL's buildings that have none (`sglBuildingFetch`,
// item R6: it writes `height` or `levels` into a building's properties before this reads them),
// the pitched roofs' meshes (R6), the shadows and foot shade (R5: [`RealisticTile::footprints`]).
//
// Deviation, invisible: a wall's length is its own, at the tile's middle latitude, where deck's
// shader divides the common-space length by the view centre's units per metre (both math.gl's
// 40.03e6 m round the Earth); the bays of a wall change only where its length lies within a hair
// of a half bay.

/// The MVT layer of Streets GL's buildings (`cityBuildingSources`: `layers: ["buildings"]`).
pub const SGL_SOURCE_LAYER: &str = "buildings";

/// The zooms the realistic map's buildings come from (`cityBuildingSources`, G:1047-1053):
/// OpenFreeMap's at 13 and 14 ([`SOURCE_ZOOMS`]), Streets GL's at its own zoom.
pub const REALISTIC_SOURCE_ZOOMS: [u32; 3] = [13, 14, crate::streets_gl::SGL_ZOOM];

/// A window bay and a storey, metres (`BAY_M`, `STOREY_M`, `R:582-583`).
pub const BAY_M: f64 = 4.0;
pub const STOREY_M: f64 = 3.3;

/// Façades (`FACADES`, `R:1006-1007`), in the shader's order.
pub const FACADES: u8 = 5;
pub const PLASTER: u8 = 0;
pub const BRICK: u8 = 1;
pub const BLOCK: u8 = 2;
pub const WOOD: u8 = 3;
pub const GLASS: u8 = 4;
/// Roofs (`R:1009`), in the shader's order: tar, concrete, metal, tiles, eternit, and two of
/// Streets GL's flat roofs with their plant.
pub const TAR: u8 = 0;
pub const CONCRETE: u8 = 1;
pub const METAL: u8 = 2;
pub const TILES: u8 = 3;
pub const ETERNIT: u8 = 4;
pub const FLAT1: u8 = 5;
pub const FLAT3: u8 = 6;

/// Each façade's colours, sRGB (`WALL_COLOURS`, `R:1012-1047`): its texture is near white where
/// tinted, so these are the walls'.
pub const WALL_COLOURS: [&[[u8; 3]]; 5] = [
    &[[238, 234, 222], [232, 220, 196], [226, 208, 172], [214, 214, 208], [236, 214, 196], [206, 220, 214], [240, 228, 180]],
    &[[176, 92, 70], [150, 78, 60], [190, 130, 96], [128, 72, 58], [206, 176, 136]],
    &[[212, 210, 204], [196, 194, 188], [222, 216, 204]],
    // Falu red, ochre, white and grey: wooden houses as the north has them
    &[[150, 44, 32], [150, 44, 32], [214, 172, 92], [236, 234, 226], [168, 170, 166]],
    &[[200, 214, 222], [190, 206, 200], [214, 206, 190]],
];

/// The colours OpenStreetMap's `building:colour` most often names, as `#rrggbb` (`NAMED`,
/// `R:1053-1075`).
pub fn named_colour(name: &str) -> Option<&'static str> {
    Some(match name {
        "white" => "#f2f0ea",
        "black" => "#3a3a3a",
        "grey" | "gray" => "#9a9a9a",
        "lightgrey" | "lightgray" => "#c8c8c8",
        "darkgrey" | "darkgray" => "#6a6a6a",
        "silver" => "#c0c0c0",
        "red" => "#b04a3a",
        "maroon" => "#7a3028",
        "brown" => "#8a5a40",
        "tan" => "#d2b48c",
        "beige" => "#e6dcc0",
        "cream" => "#f0e6c8",
        "yellow" => "#e8d27a",
        "gold" => "#d8b860",
        "orange" => "#d88a50",
        "pink" => "#e0b0b0",
        "green" => "#8aa080",
        "blue" => "#8098b0",
        _ => return None,
    })
}

/// `Math.imul`.
fn imul(a: i32, b: i32) -> i32 {
    a.wrapping_mul(b)
}

/// A hash of `seed`, 0 to 1, a different one for each `salt` (`hash`, `R:1078-1082`).
pub fn style_hash(seed: i32, salt: i32) -> f64 {
    let mut h = imul(seed ^ imul(salt.wrapping_add(1), 0x9e37_79b9_u32 as i32), 0x85eb_ca6b_u32 as i32) as u32;
    h = imul((h ^ (h >> 13)) as i32, 0xc2b2_ae35_u32 as i32) as u32;
    f64::from(h ^ (h >> 16)) / 4_294_967_296.0
}

/// `pickWeighted`: walk `weights` taking each one's weight off `r · total`.
fn pick_weighted(r: f64, weights: &[(u8, f64)]) -> u8 {
    let total = weights.iter().fold(0.0, |s, &(_, w)| s + w);
    let mut left = r * total;
    for &(value, w) in weights {
        if left < w {
            return value;
        }
        left -= w;
    }
    weights[weights.len() - 1].0
}

/// A building's roof as its seed and height pick it (both `buildingStyle` and `roofColour`).
fn picked_roof(seed: i32, height: f64) -> u8 {
    if height < 10.0 {
        pick_weighted(style_hash(seed, 3), &[(TILES, 0.55), (METAL, 0.2), (ETERNIT, 0.15), (TAR, 0.1)])
    } else {
        pick_weighted(style_hash(seed, 3), &[(TAR, 0.35), (CONCRETE, 0.25), (FLAT1, 0.2), (FLAT3, 0.15), (METAL, 0.05)])
    }
}

/// A building's façade (0–4) and roof (the shader's order) where its map data says
/// (`BuildingLook`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BuildingLook {
    pub facade: Option<u8>,
    pub roof: Option<u8>,
}

/// A building's look (`buildingStyle`, `R:1128-1169`), picked by `seed` (the same each time it is
/// loaded) and its `height`: its wall colour (the one OpenStreetMap maps for it, toned down, where
/// it has one) and in the alpha its façade and roof (`255 − (façade + 5 · roof)`), which the
/// shader reads.
pub fn building_style(seed: i32, height: f64, colour: Option<&str>, look: BuildingLook) -> [u8; 4] {
    let facade = look.facade.unwrap_or_else(|| {
        if height > 30.0 && style_hash(seed, 1) < 0.55 {
            GLASS
        } else {
            let block = if height > 12.0 { 0.25 } else { 0.1 };
            let wood = if height < 9.0 { 0.22 } else { 0.02 };
            pick_weighted(style_hash(seed, 2), &[(PLASTER, 0.42), (BRICK, 0.28), (BLOCK, block), (WOOD, wood)])
        }
    });
    let roof = look.roof.unwrap_or_else(|| picked_roof(seed, height));
    // (`NAMED[colour.toLowerCase()] ?? colour`, then `/^#[0-9a-f]{6}$/i`)
    let hex = colour.map(|c| named_colour(&c.to_lowercase()).map_or_else(|| c.to_owned(), str::to_owned));
    let parsed = hex.as_deref().and_then(|h| {
        let digits = h.strip_prefix('#')?;
        (digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit())).then(|| u32::from_str_radix(digits, 16).ok()).flatten()
    });
    let rgb = match parsed {
        Some(v) => {
            let soften = |c: u32| js_round(f64::from(c) * 0.75 + 200.0 * 0.25) as u8;
            [soften(v >> 16), soften((v >> 8) & 255), soften(v & 255)]
        }
        None => {
            let list = WALL_COLOURS[usize::from(facade)];
            let base = list[(style_hash(seed, 4) * list.len() as f64).floor() as usize];
            // a little variety within each
            let j = js_round((style_hash(seed, 5) - 0.5) * 14.0);
            base.map(|c| (f64::from(c) + j).clamp(0.0, 255.0) as u8)
        }
    };
    [rgb[0], rgb[1], rgb[2], 255 - (facade + FACADES * roof)]
}

/// A building's roof as seen from far out, sRGB (`roofColour`, `R:1100-1120`): its roof (as
/// [`building_style`] picks it by `seed` and height) in its texture's mean colour under the
/// building shader's tint for it. Today's app has it but switches it on nowhere (doc 04 §17.1);
/// kept for the post-parity far tiles.
pub fn roof_colour(seed: i32, height: f64) -> [u8; 3] {
    let roof = picked_roof(seed, height);
    let which = style_hash(seed, 6);
    match roof {
        TILES if which < 0.75 => [190, 117, 93],
        TILES => [131, 131, 141],
        METAL if which < 0.8 => [169, 181, 190],
        METAL => [140, 174, 163],
        TAR => [64, 64, 64],
        CONCRETE => [139, 139, 139],
        ETERNIT => [88, 86, 84],
        FLAT1 => [100, 99, 100],
        _ => [142, 144, 144],
    }
}

/// The seed of a building's look (`styleAt`, `BL:219-231`): the mean of its polygon's vertices
/// (`rings` closed, in its tile's units), in the world in the tile grid's units at zoom 23,
/// `imul(gx, 73 856 093) ^ imul(gy, 19 349 663)`: the same in a zoom-14 and a zoom-16 tile.
pub fn style_seed(tile: TileIndex, rings: &[Vec<[f32; 2]>]) -> i32 {
    let (mut sx, mut sy, mut n) = (0.0, 0.0, 0usize);
    for ring in rings {
        for &[x, y] in ring {
            sx += f64::from(x);
            sy += f64::from(y);
            n += 1;
        }
    }
    let n = n.max(1) as f64;
    let scale = 2f64.powi(23 - tile.z as i32);
    let gx = js_round((f64::from(tile.x) + sx / n) * scale);
    let gy = js_round((f64::from(tile.y) + sy / n) * scale);
    imul(to_int32(gx), 73_856_093) ^ imul(to_int32(gy), 19_349_663)
}

/// Streets GL's mapped façades and roofs in the shader's order (`SGL_FACADES`, `SGL_ROOFS`,
/// `BL:52-53`; `lookOf`).
pub fn sgl_look(b: &SglBuilding) -> BuildingLook {
    let facade = b.facade.map(|f| match f {
        Facade::Plaster => PLASTER,
        Facade::Brick => BRICK,
        Facade::CementBlock => BLOCK,
        Facade::Wood => WOOD,
        Facade::Glass => GLASS,
    });
    let roof = b.roof.map(|r| match r {
        RoofMaterial::Tar => TAR,
        RoofMaterial::Concrete => CONCRETE,
        RoofMaterial::Metal | RoofMaterial::Glass => METAL,
        RoofMaterial::Tiles | RoofMaterial::Thatch => TILES,
        RoofMaterial::Eternit => ETERNIT,
        RoofMaterial::Grass => FLAT3,
    });
    BuildingLook { facade, roof }
}

/// Whether a Streets GL building is a roof on posts (`buildingType` "roof").
fn is_roof(p: &Properties) -> bool {
    p.get("buildingType").and_then(serde_json::Value::as_str) == Some("roof")
}

/// A Streets GL building's height as drawn, metres (`buildingHeight` for `layerName` "buildings"):
/// a roof on posts none; else to its eaves where its roof is pitched (the roof mesh stands on
/// them), half way up its roof otherwise.
pub fn realistic_height(p: &Properties, b: &SglBuilding) -> f64 {
    if is_roof(p) {
        return 0.0;
    }
    b.height - if pitched(b.roof_type) { b.roof_height } else { b.roof_height / 2.0 }
}

/// What a Streets GL building's shadow is cast from (`castHeight`): half way up its roof, none for
/// a roof on posts.
pub fn cast_height(p: &Properties, b: &SglBuilding) -> f64 {
    if is_roof(p) { 0.0 } else { b.height - b.roof_height / 2.0 }
}

/// [`RealisticVertex::style`]'s fourth byte: a wall.
pub const STYLE_WALL: u8 = 1;
/// ... a wall with a window to each bay (`rb_bays > 0.5`).
pub const STYLE_BAYS: u8 = 2;

/// [`RealisticVertex::style`]'s third byte for a roof (`rb_info.z`): its texture fixed to the
/// world (a flat roof), along its own face (a pitched roof's face, R6), along it in its own mapped
/// colour (R6).
pub const ROOF_WORLD: u8 = 0;
pub const ROOF_FACE: u8 = 1;
pub const ROOF_OWN_COLOUR: u8 = 2;

/// One vertex of a realistic building: what deck's solid-polygon shaders and `rbuildInject` give
/// the building shader, made explicit (doc 04 §16.1). 40 bytes, `#[repr(C)]`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct RealisticVertex {
    /// `(u, v, h)` in the tile's frame: tile units (`v` south) and metres up.
    pub position: [f32; 3],
    /// East, north, up (not unit length): a wall's out of its outline, a roof's up.
    pub normal: [f32; 3],
    /// `rb_uv`: a wall's bays along it and storeys up it; 0 for a flat roof (its texture lies on
    /// `rl_t`).
    pub uv: [f32; 2],
    /// The building's RGBA as [`building_style`] gives it (deck's `fillColors`): sRGB, alpha
    /// `255 − (façade + 5 · roof)` (a far tile's flat footprint is drawn in it as it is).
    pub tint: [u8; 4],
    /// Façade, roof, [`ROOF_WORLD`] (or R6's kinds), and [`STYLE_WALL`] | [`STYLE_BAYS`].
    pub style: [u8; 4],
}

/// One tile of the realistic map's buildings, tessellated.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RealisticTile {
    /// Whether its buildings stand (zoom ≥ [`CITY_CLOSE_ZOOM`]): walls and roofs in the building
    /// shader; else footprints flat on the ground, unlit, in each building's colour.
    pub extruded: bool,
    /// Streets GL's (zoom 16) rather than OpenFreeMap's.
    pub sgl: bool,
    pub vertices: Vec<RealisticVertex>,
    /// Three indices into [`Self::vertices`] per triangle, building after building, each one's
    /// walls then its roof.
    pub indices: Vec<u32>,
    /// What its shadows and foot shade are cast from (extruded tiles only): each polygon's
    /// [`cast_height`] (OpenFreeMap's: its drawn height).
    pub footprints: Footprints,
    /// Its pitched roofs and their gables (Streets GL's tiles; item R6, `rustradar_map`'s
    /// `layers::roofs`): triangles, three vertices each, drawn after the buildings through
    /// `vs_roof`.
    pub roofs: Vec<RealisticVertex>,
}

impl RealisticTile {
    /// Whether it draws nothing.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty() && self.roofs.is_empty()
    }

    /// The bytes it holds (for the tile cache's budget).
    pub fn byte_size(&self) -> usize {
        size_of_val(self.vertices.as_slice()) + size_of_val(self.indices.as_slice()) + self.footprints.byte_size() + size_of_val(self.roofs.as_slice())
    }
}

/// One building of a realistic tile: what its look and shape are made from.
pub struct RealisticBuilding<'a> {
    /// Its polygon: rings closed, tile units (the outline first).
    pub rings: &'a [Vec<[f32; 2]>],
    /// Drawn height (deck's `elevations`, `f32`) and what its look is picked with (`f64`).
    pub height: f32,
    pub style_height: f64,
    /// What its shadow is cast from.
    pub cast_height: f32,
    pub colour: Option<&'a str>,
    pub look: BuildingLook,
}

/// Builds one realistic tile.
struct RealisticBuilder {
    tile: TileIndex,
    /// Metres per tile unit at the tile's middle (math.gl's circumference, as deck's shader).
    metres_per_unit: f64,
    out: RealisticTile,
}

impl RealisticBuilder {
    fn new(tile: TileIndex, extruded: bool, sgl: bool) -> Self {
        let [_, lat] = tile_middle(tile);
        let metres_per_unit = crate::camera::MERCATOR_EARTH_CIRCUMFERENCE_M * (lat * PI / 180.0).cos() / 2f64.powi(tile.z as i32);
        RealisticBuilder { tile, metres_per_unit, out: RealisticTile { extruded, sgl, ..Default::default() } }
    }

    fn building(&mut self, b: &RealisticBuilding<'_>) {
        let tint = building_style(style_seed(self.tile, b.rings), b.style_height, b.colour, b.look);
        let code = 255 - tint[3];
        let (facade, roof) = (code % FACADES, code / FACADES);
        let out = &mut self.out;
        let height = b.height;
        if out.extruded {
            let f = &mut out.footprints;
            f.polygon_starts.push((f.positions.len() / 2) as u32);
            for ring in b.rings {
                f.ring_starts.push((f.positions.len() / 2) as u32);
                f.positions.extend(ring.iter().flatten());
            }
            f.heights.push(b.cast_height);
            if height > 0.0 {
                // whole storeys to the building (Streets GL's way): f32, as the shader worked it out
                let levels = (height / STOREY_M as f32 + 0.5).floor().max(1.0);
                for ring in b.rings {
                    for edge in ring.windows(2) {
                        let ([u0, v0], [u1, v1]) = (edge[0], edge[1]);
                        let (du, dv) = (u1 - u0, v1 - v0);
                        if du == 0.0 && dv == 0.0 {
                            continue;
                        }
                        // the wall's length, metres, and whole bays of windows to it
                        let length = (f64::from(du).hypot(f64::from(dv)) * self.metres_per_unit) as f32;
                        let bays = (length / BAY_M as f32 + 0.5).floor();
                        let across = if bays > 0.5 { bays } else { length / BAY_M as f32 };
                        let normal = [dv, du, 0.0];
                        let style = [facade, roof, ROOF_WORLD, STYLE_WALL | if bays > 0.5 { STYLE_BAYS } else { 0 }];
                        let base = out.vertices.len() as u32;
                        for ([u, v, h], uv) in [
                            ([u0, v0, 0.0], [0.0, 0.0]),
                            ([u1, v1, 0.0], [across, 0.0]),
                            ([u1, v1, height], [across, levels]),
                            ([u0, v0, height], [0.0, levels]),
                        ] {
                            out.vertices.push(RealisticVertex { position: [u, v, h], normal, uv, tint, style });
                        }
                        out.indices.extend([0, 1, 2, 0, 2, 3].map(|k| base + k));
                    }
                }
            }
        }
        // the roof (flat tiles: the footprint on the ground), earcut's triangles of the polygon
        let top = if out.extruded { height } else { 0.0 };
        let mut positions = Vec::new();
        let mut holes = Vec::new();
        for (i, ring) in b.rings.iter().enumerate() {
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
        let style = [facade, roof, ROOF_WORLD, 0];
        for ring in b.rings {
            for &[u, v] in ring {
                out.vertices.push(RealisticVertex { position: [u, v, top], normal: [0.0, 0.0, 1.0], uv: [0.0, 0.0], tint, style });
            }
        }
        out.indices.extend(triangles.into_iter().map(|i| base + i as u32));
    }
}

/// The buildings of one tile of the realistic map (`bytes`, the tile as served, decompressed) at
/// `tile`: Streets GL's `buildings` at its zoom, else OpenFreeMap's `building`; extruded when its
/// zoom is at least `close_from_zoom` ([`CITY_CLOSE_ZOOM`]), flat otherwise. `cancelled` is asked
/// between features; once it says so the work stops.
pub fn tessellate_realistic(tile: TileIndex, bytes: &[u8], close_from_zoom: u32, cancelled: &dyn Fn() -> bool) -> Result<RealisticTile, Cancelled> {
    tessellate_realistic_with(tile, bytes, close_from_zoom, cancelled, &|_, _| {})
}

/// [`tessellate_realistic`], with each Streets GL building's properties handed to `adjust` before
/// they are read, with the building's place among its layer's features (item R6: Overture's
/// heights, `sglBuildingFetch`).
pub fn tessellate_realistic_with(
    tile: TileIndex,
    bytes: &[u8],
    close_from_zoom: u32,
    cancelled: &dyn Fn() -> bool,
    adjust: &dyn Fn(usize, &mut Properties),
) -> Result<RealisticTile, Cancelled> {
    let sgl = tile.z >= crate::streets_gl::SGL_ZOOM;
    let mut b = RealisticBuilder::new(tile, tile.z >= close_from_zoom, sgl);
    let name = if sgl { SGL_SOURCE_LAYER } else { SOURCE_LAYER };
    for layer in mvt::layers_named(bytes, &[name]) {
        let heights = Heights::of(&layer);
        for (index, feature) in layer.features().enumerate() {
            if cancelled() {
                return Err(Cancelled);
            }
            if feature.kind != GeomType::Polygon {
                continue;
            }
            let polygons = binary_polygons(&feature, layer.extent);
            if sgl {
                let mut p = feature.json_properties();
                p.insert("layerName".into(), serde_json::Value::from(SGL_SOURCE_LAYER));
                adjust(index, &mut p);
                let sgl = sgl_building(&p);
                let height = realistic_height(&p, &sgl);
                for rings in &polygons {
                    b.building(&RealisticBuilding {
                        rings,
                        height: height as f32,
                        style_height: height,
                        cast_height: cast_height(&p, &sgl) as f32,
                        colour: sgl.colour.as_deref(),
                        look: sgl_look(&sgl),
                    });
                }
            } else {
                let height = building_height(&feature, heights);
                let colour = match feature.get("colour") {
                    Some(Value::Str(s)) => Some(*s),
                    _ => None,
                };
                for rings in &polygons {
                    b.building(&RealisticBuilding {
                        rings,
                        height: height as f32,
                        style_height: height,
                        cast_height: height as f32,
                        colour,
                        look: BuildingLook::default(),
                    });
                }
            }
        }
    }
    let f = &mut b.out.footprints;
    if b.out.extruded {
        f.polygon_starts.push((f.positions.len() / 2) as u32);
        f.ring_starts.push((f.positions.len() / 2) as u32);
    }
    Ok(b.out)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixture(z: u32, x: u32, y: u32) -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../map/tests/data/tiles/openfreemap/{z}/{x}/{y}.pbf"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// A tile of one layer named `name`, features of `(properties, rings)` with integer
    /// coordinates (extent 4096): a minimal MVT encoder for the tests.
    /// A test feature: its properties and its rings.
    type TestFeature<'a> = (&'a [(&'a str, Value<'a>)], &'a [&'a [(i64, i64)]]);

    fn mvt_tile(name: &str, features: &[TestFeature<'_>]) -> Vec<u8> {
        fn varint(out: &mut Vec<u8>, mut v: u64) {
            loop {
                let byte = (v & 0x7f) as u8;
                v >>= 7;
                if v == 0 {
                    out.push(byte);
                    return;
                }
                out.push(byte | 0x80);
            }
        }
        fn field(out: &mut Vec<u8>, number: u64, bytes: &[u8]) {
            varint(out, (number << 3) | 2);
            varint(out, bytes.len() as u64);
            out.extend_from_slice(bytes);
        }
        let zigzag = |v: i64| ((v << 1) ^ (v >> 63)) as u64;
        let mut keys: Vec<&str> = Vec::new();
        let mut values: Vec<Vec<u8>> = Vec::new();
        let mut layer = Vec::new();
        field(&mut layer, 1, name.as_bytes());
        for (properties, rings) in features {
            let mut tags = Vec::new();
            for (key, value) in properties.iter() {
                let k = keys.iter().position(|k| k == key).unwrap_or_else(|| {
                    keys.push(key);
                    keys.len() - 1
                });
                let mut encoded = Vec::new();
                match *value {
                    Value::Str(s) => field(&mut encoded, 1, s.as_bytes()),
                    Value::Num(n) => {
                        varint(&mut encoded, (3 << 3) | 1);
                        encoded.extend_from_slice(&n.to_le_bytes());
                    }
                    Value::Bool(b) => {
                        varint(&mut encoded, 7 << 3);
                        varint(&mut encoded, u64::from(b));
                    }
                }
                values.push(encoded);
                varint(&mut tags, k as u64);
                varint(&mut tags, (values.len() - 1) as u64);
            }
            let mut geometry = Vec::new();
            let (mut x, mut y) = (0, 0);
            for ring in rings.iter() {
                varint(&mut geometry, (1 << 3) | 1);
                varint(&mut geometry, zigzag(ring[0].0 - x));
                varint(&mut geometry, zigzag(ring[0].1 - y));
                (x, y) = ring[0];
                varint(&mut geometry, ((ring.len() as u64 - 1) << 3) | 2);
                for &(px, py) in &ring[1..] {
                    varint(&mut geometry, zigzag(px - x));
                    varint(&mut geometry, zigzag(py - y));
                    (x, y) = (px, py);
                }
                varint(&mut geometry, (1 << 3) | 7);
            }
            let mut feature = Vec::new();
            field(&mut feature, 2, &tags);
            varint(&mut feature, 3 << 3);
            varint(&mut feature, 3);
            field(&mut feature, 4, &geometry);
            field(&mut layer, 2, &feature);
        }
        for key in keys {
            field(&mut layer, 3, key.as_bytes());
        }
        for value in values {
            field(&mut layer, 4, &value);
        }
        varint(&mut layer, 15 << 3);
        varint(&mut layer, 2);
        let mut tile = Vec::new();
        field(&mut tile, 3, &layer);
        tile
    }

    /// A square building 1024 units (a quarter of the tile) across, from `(1024, 1024)`, its outline
    /// counter-clockwise in the tile's numbers (an MVT outline: a positive area by the surveyor's
    /// formula with y down).
    const SQUARE: [(i64, i64); 4] = [(1024, 1024), (2048, 1024), (2048, 2048), (1024, 2048)];

    const Z14: TileIndex = TileIndex { x: 9014, y: 4818, z: 14 };

    #[test]
    fn heights_are_read_as_decks_binary_tiles_read_them() {
        let square: &[&[(i64, i64)]] = &[&SQUARE];
        let tile = mvt_tile(
            "building",
            &[
                (&[("render_height", Value::Num(21.5))], square),
                (&[("render_height", Value::Num(30.0)), ("hide_3d", Value::Bool(true))], square),
                // no render_height in a tile whose others have it as a number: 0, not 6
                (&[("colour", Value::Str("#aabbcc"))], square),
            ],
        );
        let layer = mvt::layer(&tile, "building").expect("the layer");
        let heights = Heights::of(&layer);
        assert!(heights.numeric_height);
        let got: Vec<f64> = layer.features().map(|f| building_height(&f, heights)).collect();
        assert_eq!(got, [21.5, 0.0, 0.0]);
        // with no numeric render_height anywhere, `?? 6`; a string is read as Number reads it
        let tile = mvt_tile("building", &[(&[], square), (&[("render_height", Value::Str(" 12.5 "))], square)]);
        let layer = mvt::layer(&tile, "building").expect("the layer");
        let heights = Heights::of(&layer);
        assert!(!heights.numeric_height);
        let got: Vec<f64> = layer.features().map(|f| building_height(&f, heights)).collect();
        assert_eq!(got, [DEFAULT_HEIGHT_M, 12.5]);
    }

    #[test]
    fn an_extruded_square_has_four_walls_facing_out_and_a_roof_at_its_height() {
        let square: &[&[(i64, i64)]] = &[&SQUARE];
        let tile = mvt_tile("building", &[(&[("render_height", Value::Num(20.0))], square)]);
        let out = tessellate(Z14, &tile, CITY_CLOSE_ZOOM, &|| false).expect("not cancelled");
        assert!(out.extruded);
        // four walls of four vertices, the roof's five (the ring closed)
        assert_eq!(out.vertices.len(), 4 * 4 + 5);
        assert_eq!(out.indices.len(), 4 * 6 + 2 * 3);
        let walls = &out.vertices[..16];
        // each wall's normal points away from the square's middle (0.375, 0.375), east and north
        for wall in walls.chunks(4) {
            let mid = [(wall[0].position[0] + wall[1].position[0]) / 2.0, (wall[0].position[1] + wall[1].position[1]) / 2.0];
            let out_east = mid[0] - 0.375;
            let out_north = -(mid[1] - 0.375);
            let n = wall[0].normal;
            assert!(n[0] * out_east + n[1] * out_north > 0.0, "{wall:?}");
            assert_eq!(n[2], 0.0);
            assert!(wall.iter().all(|v| v.normal == n));
            assert_eq!(wall.iter().map(|v| v.position[2]).collect::<Vec<_>>(), [0.0, 0.0, 20.0, 20.0]);
        }
        let roof = &out.vertices[16..];
        assert!(roof.iter().all(|v| v.position[2] == 20.0 && v.normal == [0.0, 0.0, 1.0]));
        // the roof's two triangles cover the square
        let area: f32 = out.indices[24..]
            .chunks(3)
            .map(|t| {
                let p = |k: usize| out.vertices[t[k] as usize].position;
                let (a, b, c) = (p(0), p(1), p(2));
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0
            })
            .sum();
        assert!((area - 0.0625).abs() < 1e-6, "{area}");
        // its footprint, for the shadows: one polygon of one closed ring, 20 m
        assert_eq!(out.footprints.heights, [20.0]);
        assert_eq!(out.footprints.polygon_starts, [0, 5]);
        assert_eq!(out.footprints.ring_starts, [0, 5]);
        assert_eq!(out.footprints.positions.len(), 10);
        assert_eq!(out.footprints.positions[..2], out.footprints.positions[8..]);
    }

    #[test]
    fn a_far_tile_is_flat_and_casts_nothing() {
        let square: &[&[(i64, i64)]] = &[&SQUARE];
        let tile = mvt_tile("building", &[(&[("render_height", Value::Num(20.0))], square)]);
        let far = TileIndex { x: 4507, y: 2409, z: 13 };
        let out = tessellate(far, &tile, CITY_CLOSE_ZOOM, &|| false).expect("not cancelled");
        assert!(!out.extruded);
        assert_eq!(out.vertices.len(), 5);
        assert_eq!(out.indices.len(), 6);
        assert!(out.vertices.iter().all(|v| v.position[2] == 0.0));
        assert!(out.footprints.is_empty() && out.footprints.polygon_starts.is_empty());
        assert_eq!(tile_shadows(far, &out.footprints, [1.0, 0.0, 0.0]), None);
    }

    #[test]
    fn a_hole_s_walls_face_into_it_and_rings_of_no_area_are_dropped() {
        // an outline, a hole in it, and a ring of no area
        let hole: &[(i64, i64)] = &[(1280, 1280), (1280, 1792), (1792, 1792), (1792, 1280)];
        let flat: &[(i64, i64)] = &[(100, 100), (200, 100), (300, 100)];
        let rings: &[&[(i64, i64)]] = &[&SQUARE, hole, flat];
        let tile = mvt_tile("building", &[(&[("render_height", Value::Num(10.0))], rings)]);
        let layer = mvt::layer(&tile, "building").expect("the layer");
        let feature = layer.features().next().expect("a feature");
        let polygons = binary_polygons(&feature, layer.extent);
        assert_eq!(polygons.len(), 1);
        assert_eq!(polygons[0].len(), 2, "the outline and its hole");
        let out = tessellate(Z14, &tile, CITY_CLOSE_ZOOM, &|| false).expect("not cancelled");
        // the hole's walls: normals toward the hole's middle (0.375, 0.375)
        let hole_walls = &out.vertices[16..32];
        for wall in hole_walls.chunks(4) {
            let mid = [(wall[0].position[0] + wall[1].position[0]) / 2.0, (wall[0].position[1] + wall[1].position[1]) / 2.0];
            let (to_east, to_north) = (0.375 - mid[0], -(0.375 - mid[1]));
            let n = wall[0].normal;
            assert!(n[0] * to_east + n[1] * to_north > 0.0, "{wall:?}");
        }
        // the roof covers the ring between them: 0.0625 − 0.015625
        let roof_indices = &out.indices[8 * 6..];
        let area: f32 = roof_indices
            .chunks(3)
            .map(|t| {
                let p = |k: usize| out.vertices[t[k] as usize].position;
                let (a, b, c) = (p(0), p(1), p(2));
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0
            })
            .sum();
        assert!((area - 0.046875).abs() < 1e-6, "{area}");
        assert_eq!(out.footprints.ring_starts, [0, 5, 10]);
    }

    #[test]
    fn shadows_fall_away_from_the_sun_and_fade_with_it() {
        let square: &[&[(i64, i64)]] = &[&SQUARE];
        let tile = mvt_tile("building", &[(&[("render_height", Value::Num(20.0))], square)]);
        let out = tessellate(Z14, &tile, CITY_CLOSE_ZOOM, &|| false).expect("not cancelled");
        let [lon, lat] = tile_middle(Z14);
        // the sun in the south-east at 30°: from the Earth's centre, by turning back `local_sun`
        let to_sun_at = |east: f64, north: f64, up: f64| {
            let (sl, cl) = (lat.to_radians().sin(), lat.to_radians().cos());
            let (so, co) = (lon.to_radians().sin(), lon.to_radians().cos());
            [-so * east - sl * co * north + cl * co * up, co * east - sl * so * north + cl * so * up, cl * north + sl * up]
        };
        let alt = 30f64.to_radians();
        let az = 135f64.to_radians();
        let sun = to_sun_at(alt.cos() * az.sin(), alt.cos() * az.cos(), alt.sin());
        let shadows = tile_shadows(Z14, &out.footprints, sun).expect("shadows");
        assert!((shadows.alpha - BUILDING_SHADOW_ALPHA).abs() < 1e-12, "full sun: {}", shadows.alpha);
        // two walls (north and west) cast, toward the north-west: x smaller, y smaller (v south)
        assert_eq!(shadows.indices.len(), 12);
        let tile_metres = (EARTH_CIRCUMFERENCE * lat.to_radians().cos()) / 2f64.powi(14);
        let reach = 20.0 / alt.tan() / tile_metres;
        let min_x = shadows.positions.chunks(2).map(|p| f64::from(p[0])).fold(f64::INFINITY, f64::min);
        let min_y = shadows.positions.chunks(2).map(|p| f64::from(p[1])).fold(f64::INFINITY, f64::min);
        let along = reach * 45f64.to_radians().sin();
        assert!((min_x - (0.25 - along)).abs() < 1e-5, "{min_x} against {}", 0.25 - along);
        assert!((min_y - (0.25 - along)).abs() < 1e-5, "{min_y}");
        assert_eq!(shadow_color(shadows.alpha, SHADOW_STRENGTH), [4, 8, 16, 89]);
        // at 3° the alpha is halved; under the horizon, none
        let low = 3f64.to_radians();
        let sun = to_sun_at(low.cos() * az.sin(), low.cos() * az.cos(), low.sin());
        let half = tile_shadows(Z14, &out.footprints, sun).expect("a pale shadow");
        assert!((half.alpha - BUILDING_SHADOW_ALPHA / 2.0).abs() < 1e-9, "{}", half.alpha);
        let below = to_sun_at(az.sin(), az.cos(), -0.1);
        assert_eq!(tile_shadows(Z14, &out.footprints, below), None);
        // just over the horizon the alpha is under SHADOW_MIN_ALPHA: none
        let grazing = 0.5f64.to_radians();
        let sun = to_sun_at(grazing.cos() * az.sin(), grazing.cos() * az.cos(), grazing.sin());
        assert_eq!(tile_shadows(Z14, &out.footprints, sun), None);
    }

    #[test]
    fn the_shadow_sun_is_held_for_a_quarter_of_a_degree() {
        let mut sun = ShadowSun::default();
        let t0 = 1_782_036_000_000.0;
        let first = sun.at(t0);
        assert_eq!(first, sun_direction(t0));
        // the sun turns 0.25° of longitude a minute, at midsummer 0.25° · cos 23.4° = 0.23° on
        // the sky: held a minute, moved 75 s on (0.29°)
        assert_eq!(sun.at(t0 + 61_000.0), first);
        let moved = sun.at(t0 + 75_000.0);
        assert_ne!(moved, first);
        assert_eq!(moved, sun_direction(t0 + 75_000.0));
        // and held again from there
        assert_eq!(sun.at(t0 + 90_000.0), moved);
        assert_eq!(sun.held(), Some(moved));
    }

    /// The fixtures' tiles of V5 (Stockholm city) and V4 (Arlanda): extruded z14 tiles have walls,
    /// roofs and footprints; z13 tiles are flat; a tile with no buildings gives nothing; a
    /// cancelled job stops.
    #[test]
    fn fixture_tiles_tessellate() {
        let close = TileIndex { x: 9014, y: 4818, z: 14 };
        let out = tessellate(close, &fixture(14, 9014, 4818), CITY_CLOSE_ZOOM, &|| false).expect("not cancelled");
        let buildings = out.footprints.len();
        let walls = out.vertices.iter().filter(|v| v.normal[2] == 0.0).count() / 4;
        eprintln!("14/9014/4818: {buildings} buildings, {walls} walls, {} vertices, {} indices, {} bytes", out.vertices.len(), out.indices.len(), out.byte_size());
        assert!(buildings > 500, "{buildings}");
        assert!(walls > 4 * buildings);
        let tallest = out.footprints.heights.iter().copied().fold(0.0, f32::max);
        assert!(tallest > 20.0, "{tallest}");
        // every roof vertex is at its building's height, every wall vertex on the ground or there
        assert!(out.vertices.iter().all(|v| v.position[2] >= 0.0 && v.position[2] <= tallest));
        let far = TileIndex { x: 4507, y: 2409, z: 13 };
        let flat = tessellate(far, &fixture(13, 4507, 2409), CITY_CLOSE_ZOOM, &|| false).expect("not cancelled");
        assert!(!flat.extruded && !flat.is_empty());
        assert!(flat.vertices.iter().all(|v| v.position[2] == 0.0));
        assert!(flat.footprints.is_empty());
        assert!(tessellate(close, &[], CITY_CLOSE_ZOOM, &|| false).expect("not cancelled").is_empty());
        assert_eq!(tessellate(close, &fixture(14, 9014, 4818), CITY_CLOSE_ZOOM, &|| true), Err(Cancelled));
    }

    #[test]
    fn the_lod_and_the_constants_are_today_s() {
        assert_eq!(city_lod(), Lod { levels: vec![13, 14, 16], bias: 0.5, max_distance: 25_000.0 });
        assert!((1.0 - shadow_sun_step() - 9.519e-6).abs() < 1e-9, "{}", shadow_sun_step());
        assert_eq!(size_of::<BuildingVertex>(), 24);
        assert_eq!(string_to_number("0x10"), 16.0);
        assert!(string_to_number("12 m").is_nan());
    }

    // ---- the realistic map's buildings (R4) ----

    /// `buildingStyle` and `roofColour` against today's own (`realistic.ts`, run under bun):
    /// seeds over the whole 32-bit range, heights each side of the thresholds, colours mapped as
    /// `#rrggbb` (either case) or by name (either case), unknown ones, mapped façades and roofs.
    #[test]
    fn building_style_is_today_s() {
        let look = |facade: Option<u8>, roof: Option<u8>| BuildingLook { facade, roof };
        /// Seed, height, colour, mapped look, and today's RGBA.
        type Case<'a> = (i32, f64, Option<&'a str>, BuildingLook, [u8; 4]);
        let cases: [Case<'_>; 10] = [
            (0, 0.0, None, look(None, None), [242, 240, 232, 237]),
            (123_456_789, 5.0, None, look(None, None), [220, 178, 98, 237]),
            (-987_654_321, 35.0, None, look(None, None), [203, 217, 225, 251]),
            (42, 12.5, Some("#aabbcc"), look(None, None), [178, 190, 203, 228]),
            (7, 8.0, Some("Red"), look(None, None), [182, 106, 94, 245]),
            (99, 40.0, Some("white"), look(Some(1), None), [232, 230, 226, 229]),
            (1000, 20.0, Some("nonsense"), look(None, Some(6)), [226, 220, 208, 223]),
            (-1, 3.0, None, look(Some(0), Some(3)), [209, 209, 203, 240]),
            (i32::MAX, 31.0, None, look(None, None), [205, 219, 227, 226]),
            (i32::MIN, 9.0, Some("#ABCDEF"), look(None, None), [178, 204, 229, 239]),
        ];
        for (seed, height, colour, look, today) in cases {
            assert_eq!(building_style(seed, height, colour, look), today, "{seed} {height} {colour:?} {look:?}");
        }
        for (seed, height, today) in [(0, 0.0, [190, 117, 93]), (5, 5.0, [131, 131, 141]), (77, 20.0, [139, 139, 139]), (-12_345, 9.9, [64, 64, 64]), (31_337, 50.0, [64, 64, 64])] {
            assert_eq!(roof_colour(seed, height), today, "{seed} {height}");
        }
        // the shader's decoding of the alpha gives back the façade and the roof
        let [_, _, _, a] = building_style(0, 0.0, None, look(Some(GLASS), Some(FLAT3)));
        assert_eq!(((255 - a) % FACADES, (255 - a) / FACADES), (GLASS, FLAT3));
    }

    /// Each building of four fixture tiles (two of Streets GL's at V5, OpenFreeMap's zoom-14 tile
    /// under V5's centre and a zoom-13 one) as today's app styles it: its vertex count, seed,
    /// height (`f32`, in mm) and RGBA, as `building-layer.ts`'s `styleAt` and `buildingHeight` give
    /// them over loaders.gl's binary tiles under bun, as an FNV-1a digest of the lot (with the
    /// first rows spelled out).
    #[test]
    fn fixture_buildings_look_as_today_s() {
        fn sgl_fixture(z: u32, x: u32, y: u32) -> Vec<u8> {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../map/tests/data/tiles/streets-gl/{z}/{x}/{y}.pbf"));
            std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        }
        /// `[vertices, seed, height mm, r, g, b, a]` of each building, as the TypeScript lists them.
        fn rows(tile: TileIndex, bytes: &[u8]) -> Vec<[i64; 7]> {
            let sgl = tile.z >= crate::streets_gl::SGL_ZOOM;
            let mut out = Vec::new();
            for layer in mvt::layers_named(bytes, &[if sgl { SGL_SOURCE_LAYER } else { SOURCE_LAYER }]) {
                let heights = Heights::of(&layer);
                for feature in layer.features().filter(|f| f.kind == GeomType::Polygon) {
                    let (height, colour, look) = if sgl {
                        let mut p = feature.json_properties();
                        p.insert("layerName".into(), serde_json::Value::from(SGL_SOURCE_LAYER));
                        let b = sgl_building(&p);
                        (realistic_height(&p, &b), b.colour.clone(), sgl_look(&b))
                    } else {
                        let colour = match feature.get("colour") {
                            Some(Value::Str(s)) => Some((*s).to_owned()),
                            _ => None,
                        };
                        (building_height(&feature, heights), colour, BuildingLook::default())
                    };
                    for rings in binary_polygons(&feature, layer.extent) {
                        let seed = style_seed(tile, &rings);
                        let [r, g, b, a] = building_style(seed, height, colour.as_deref(), look);
                        let n = rings.iter().map(Vec::len).sum::<usize>();
                        let mm = js_round(f64::from(height as f32) * 1000.0) as i64;
                        out.push([n as i64, i64::from(seed), mm, i64::from(r), i64::from(g), i64::from(b), i64::from(a)]);
                    }
                }
            }
            out
        }
        fn fnv(rows: &[[i64; 7]]) -> u32 {
            let mut h: u32 = 0x811c_9dc5;
            for &v in rows.iter().flatten() {
                for byte in (v as u32).to_le_bytes() {
                    h ^= u32::from(byte);
                    h = h.wrapping_mul(0x0100_0193);
                }
            }
            h
        }
        let today: [(TileIndex, usize, u32, [[i64; 7]; 3]); 4] = [
            (
                TileIndex { x: 36057, y: 19267, z: 16 },
                38,
                0x6d42_0c2a,
                [[14, -226_391_345, 12000, 169, 85, 63, 229], [28, -1_847_315_158, 73000, 184, 76, 76, 239], [28, 1_106_534_311, 16000, 205, 203, 197, 253]],
            ),
            (
                TileIndex { x: 36056, y: 19269, z: 16 },
                23,
                0xe773_ad27,
                [[53, -1_076_731_213, 4000, 232, 210, 192, 240], [82, -893_807_872, 4000, 222, 204, 168, 240], [23, 1_892_200_301, 4000, 217, 211, 199, 233]],
            ),
            (
                TileIndex { x: 9014, y: 4818, z: 14 },
                1106,
                0x24a7_6fc4,
                [[5, 1_969_432_688, 0, 201, 171, 131, 239], [5, -183_671_296, 2000, 147, 75, 57, 239], [5, -1_072_268_540, 3000, 229, 211, 175, 240]],
            ),
            (
                TileIndex { x: 4507, y: 2409, z: 13 },
                489,
                0xe6c8_72c0,
                [[6, -418_412_049, 6000, 192, 132, 98, 239], [6, 1_371_270_478, 6000, 238, 234, 222, 240], [10, -1_885_692_243, 6000, 209, 207, 201, 233]],
            ),
        ];
        for (tile, count, digest, first) in today {
            let bytes = if tile.z == 16 { sgl_fixture(tile.z, tile.x, tile.y) } else { fixture(tile.z, tile.x, tile.y) };
            let got = rows(tile, &bytes);
            assert_eq!(got.len(), count, "{tile:?}");
            assert_eq!(got[..3], first, "{tile:?}");
            assert_eq!(fnv(&got), digest, "{tile:?}: {got:?}");
            // the tile's vertices carry those colours (the far tile's as deck's flat polygons)
            let out = tessellate_realistic(tile, &bytes, CITY_CLOSE_ZOOM, &|| false).expect("not cancelled");
            assert_eq!(out.sgl, tile.z == 16);
            assert_eq!(out.extruded, tile.z >= CITY_CLOSE_ZOOM);
            let tints: std::collections::HashSet<[u8; 4]> = got.iter().map(|r| [r[3], r[4], r[5], r[6]].map(|c| c as u8)).collect();
            assert!(out.vertices.iter().all(|v| tints.contains(&v.tint)), "{tile:?}");
            assert_eq!(out.footprints.len(), if out.extruded { count } else { 0 });
        }
    }

    /// A wall is a quad from the ground to the height with whole 4 m bays along it and whole
    /// 3.3 m storeys up it (`rbuildInject`'s vertex part), its normal out of the outline; a wall
    /// under 2 m has no bay, its plain texture `length / 4` across; the roof lies flat at the
    /// height, its texture on the world's (`rl_t`). Each vertex carries the building's RGBA and its
    /// façade and roof.
    #[test]
    fn a_realistic_wall_has_whole_bays_and_storeys() {
        // the square of the plain tests, and a sliver 1.5 m wide in the same tile
        let sliver: &[(i64, i64)] = &[(3000, 3000), (3005, 3000), (3005, 3400), (3000, 3400)];
        let square: &[&[(i64, i64)]] = &[&SQUARE];
        let thin: &[&[(i64, i64)]] = &[sliver];
        let tile = mvt_tile("building", &[(&[("render_height", Value::Num(20.0))], square), (&[("render_height", Value::Num(2.0))], thin)]);
        let out = tessellate_realistic(Z14, &tile, CITY_CLOSE_ZOOM, &|| false).expect("not cancelled");
        assert!(out.extruded && !out.sgl);
        let [_, lat] = tile_middle(Z14);
        let metres_per_unit = crate::camera::MERCATOR_EARTH_CIRCUMFERENCE_M * lat.to_radians().cos() / 2f64.powi(14);
        let side = 0.25 * metres_per_unit;
        let bays = (side / BAY_M).round() as f32;
        let levels = (20.0f64 / STOREY_M).round() as f32;
        eprintln!("a side of {side:.1} m: {bays} bays, {levels} storeys");
        let walls = &out.vertices[..16];
        let tint = walls[0].tint;
        for wall in walls.chunks(4) {
            assert_eq!(wall.iter().map(|v| v.uv).collect::<Vec<_>>(), [[0.0, 0.0], [bays, 0.0], [bays, levels], [0.0, levels]]);
            assert_eq!(wall.iter().map(|v| v.position[2]).collect::<Vec<_>>(), [0.0, 0.0, 20.0, 20.0]);
            let code = 255 - tint[3];
            assert!(wall.iter().all(|v| v.tint == tint && v.style == [code % FACADES, code / FACADES, ROOF_WORLD, STYLE_WALL | STYLE_BAYS]));
            // out of the square (its middle at 0.375, 0.375; v south)
            let mid = [(wall[0].position[0] + wall[1].position[0]) / 2.0, (wall[0].position[1] + wall[1].position[1]) / 2.0];
            let n = wall[0].normal;
            assert!(n[0] * (mid[0] - 0.375) - n[1] * (mid[1] - 0.375) > 0.0);
        }
        let roof = &out.vertices[16..21];
        assert!(roof.iter().all(|v| v.position[2] == 20.0 && v.normal == [0.0, 0.0, 1.0] && v.uv == [0.0, 0.0] && v.style[3] == 0 && v.tint == tint));
        // the sliver's short walls: no bay, the plain wall a fraction across
        let sliver_walls: Vec<&RealisticVertex> = out.vertices[21..].iter().filter(|v| v.style[3] & STYLE_WALL != 0).collect();
        let short = sliver_walls.chunks(4).find(|w| w[1].uv[0] < 1.0).expect("a short wall");
        let width = 5.0 / 4096.0 * metres_per_unit;
        assert!((f64::from(short[1].uv[0]) - width / BAY_M).abs() < 1e-4, "{:?} for {width} m", short[1].uv);
        assert!(short.iter().all(|v| v.style[3] == STYLE_WALL));
        assert_eq!(short[2].uv[1], 1.0, "at least one storey");
        // footprints for the shadows: the drawn heights
        assert_eq!(out.footprints.heights, [20.0, 2.0]);
    }

    /// Streets GL's buildings (`buildingHeight` for `layerName` "buildings"): a pitched roof's
    /// walls stop at its eaves, a flat roof's half way up it, a roof on posts lies flat; their
    /// shadows are cast from half way up the roof (none for the roof on posts); a mapped material
    /// and colour give the façade, the roof and the wall colour.
    #[test]
    fn streets_gl_buildings_stand_to_their_eaves_in_their_mapped_looks() {
        let square: &[&[(i64, i64)]] = &[&SQUARE];
        let tile = mvt_tile(
            "buildings",
            &[
                (&[("height", Value::Num(20.0)), ("roofHeight", Value::Num(6.0)), ("roofType", Value::Str("gabled")), ("material", Value::Str("brick")), ("roofMaterial", Value::Str("tiles"))], square),
                (&[("height", Value::Num(20.0)), ("roofHeight", Value::Num(6.0)), ("color", Value::Num(f64::from(0x00ff_0000u32)))], square),
                (&[("height", Value::Num(5.0)), ("buildingType", Value::Str("roof"))], square),
            ],
        );
        let z16 = TileIndex { x: 36057, y: 19267, z: 16 };
        let out = tessellate_realistic(z16, &tile, CITY_CLOSE_ZOOM, &|| false).expect("not cancelled");
        assert!(out.sgl && out.extruded);
        assert_eq!(out.footprints.heights, [17.0, 17.0, 0.0]);
        let tops: Vec<f32> = out.vertices.iter().filter(|v| v.normal == [0.0, 0.0, 1.0]).map(|v| v.position[2]).collect();
        assert_eq!(tops, [14.0, 14.0, 14.0, 14.0, 14.0, 17.0, 17.0, 17.0, 17.0, 17.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        // the roof on posts has no walls: 2 × 4 walls of 4 vertices and three roofs of 5
        assert_eq!(out.vertices.len(), 2 * 16 + 3 * 5);
        // brick with tiles; red (Streets GL's 0xff0000, the map's "red" toned down)
        assert_eq!(out.vertices[0].style[..2], [BRICK, TILES]);
        let red = out.vertices.iter().find(|v| v.position[2] == 17.0).expect("the red one").tint;
        assert_eq!(red[..3], [182, 106, 94]);
    }

    /// A far tile (zoom 13) of the realistic map: the footprints flat on the ground in each
    /// building's colour with deck's alpha, no walls, no footprints for shadows; a cancelled job
    /// stops; a tile without buildings gives nothing.
    #[test]
    fn a_far_realistic_tile_lies_flat_in_each_building_s_colour() {
        let far = TileIndex { x: 4507, y: 2409, z: 13 };
        let out = tessellate_realistic(far, &fixture(13, 4507, 2409), CITY_CLOSE_ZOOM, &|| false).expect("not cancelled");
        assert!(!out.extruded && !out.is_empty());
        assert!(out.vertices.iter().all(|v| v.position[2] == 0.0 && v.style[3] == 0));
        assert!(out.footprints.is_empty());
        let colours: std::collections::HashSet<[u8; 4]> = out.vertices.iter().map(|v| v.tint).collect();
        assert!(colours.len() > 50, "{}", colours.len());
        assert!(out.vertices.iter().all(|v| v.tint[3] >= 255 - (GLASS + FACADES * FLAT3)));
        assert!(tessellate_realistic(far, &[], CITY_CLOSE_ZOOM, &|| false).expect("not cancelled").is_empty());
        assert_eq!(tessellate_realistic(far, &fixture(13, 4507, 2409), CITY_CLOSE_ZOOM, &|| true), Err(Cancelled));
        assert_eq!(size_of::<RealisticVertex>(), 40);
        assert_eq!(REALISTIC_SOURCE_ZOOMS, [13, 14, 16]);
    }
}
