//! Pitched roofs and Overture's heights (item R6; doc 04 §7.5, §7.7; row 9.4 of doc 04 §1):
//! Streets GL's buildings on the realistic map with the roofs their tiles map (`tileRoofs`,
//! `building-layer.ts`; `roofs.ts`, ported as `rustradar_scene::roofs`) and, where a building has
//! neither a height nor levels mapped, the height Overture Maps gives it (`sglBuildingFetch`,
//! `withOvertureHeights`, `overture.ts`; its pure part is `rustradar_scene::overture`). The
//! buildings themselves are R4's [`super::buildings_realistic`], whose tiles are decoded here
//! ([`decode`]) and whose draw loop draws each tile's roofs after its buildings ([`Roofs`]).
//!
//! What today's app does, and so this module:
//! - **Overture's heights** (`sglBuildingFetch`): a Streets GL tile is not handed to the layer
//!   until Overture has answered for it. Its buildings with neither `height` nor `levels` (as
//!   loaders.gl's binary tiles read them: a numeric property a building lacks is 0, and 0 is
//!   none) are looked up by the middle of their vertices (every ring of every polygon of the
//!   feature, each ring's closing point included) in the zoom-14 tiles of Overture's buildings
//!   archive ([`OVERTURE`], `backend::overture` through the app's tile fetcher): the tallest
//!   Overture building or part whose outline holds the point gives its height, else its floors
//!   as `levels` ([`Overture::heights_at`], `heightsAt`). Overture's tiles are kept as
//!   `tileBuildings` keeps them: the last [`overture::TILES_CACHED`] asked for, first in first
//!   out, a tile that could not be had as one with no buildings. Once any building has been given
//!   a height, every building's `height` and `levels` read as the `Float32Array`s
//!   `withOvertureHeights` writes them into (`f32`; 0 where the column was not numeric).
//! - **Pitched roofs** (`tileRoofs`, only in Streets GL's tiles, which stand: zoom 16): for each
//!   building with a pitched roof type (`pitched`), a roof height above 0 and not a roof on posts,
//!   each of its polygons in metres east and north of the tile's north-west corner (the tile's
//!   width `2π · 6 378 137 · cos(middle latitude) / 2^z`) is roofed by `roof_parts` from its eaves
//!   (`height − roofHeight`), with its gables; each corner carries the building's look
//!   (`styleAt`'s seed, `buildingStyle`): a gable a wall of the façade without windows, its
//!   texture in bays and storeys (`u / 4, v / 3.3`); a roof face along its own slope in metres,
//!   in the shader's colour for its roofing, or in its own mapped `roofColor` toned down as a wall
//!   colour is (`hexRgb`). deck draws the mesh in `METER_OFFSETS` from the tile's corner
//!   (SimpleMeshLayer, `composeModelMatrix`), which reads metres with math.gl's 40 030 km round
//!   the Earth at the corner's latitude (and its first-order change northwards): so today's roofs
//!   are 0.11 % larger about the tile's corner than the walls they stand on (up to 0.35 m at the
//!   far corner of a zoom-16 tile), and the port puts each corner where deck does
//!   ([`deck_meter_offset`]).
//! - **Shaded as today's are**, which is not as `tileRoofs` means them: deck 9.4's SimpleMeshLayer
//!   keeps only a mesh's `positions`, `colors`, `normals` and `texCoords`
//!   (`normalizeGeometryAttributes`), so the `rr_info` attribute never reaches the shader, which
//!   reads WebGL's value for an attribute with no buffer, `(0, 0, 0, 1)`: every face and gable is a
//!   wall of code 0 without bays, plain plaster (texture layer 0) in its corners' colour (a face's
//!   mapped roof colour, else the building's wall colour), its texture repeating once a unit of
//!   `texCoords` (a face's metres, a gable's bays and storeys), no snow, no lit windows ([`AS_DRAWN`];
//!   `TileRoofs::info` keeps what `tileRoofs` computes). Checked against today's picture of V5: a
//!   hipped roof in its mapped dark grey shows light grey, a terracotta one pink, within a few levels.
//! - **Drawn** after the tile's buildings with the building shader's roof entry points
//!   (`vs_roof`, `fs_roof`: `RoofExtension`'s `roofInject`, the tangent across the slope, no shade
//!   at the foot) and `NO_CULL`, not clipped to the tile (the roofs' SimpleMeshLayer has no
//!   `ClipExtension`): a building over a tile's edge has its whole roof from each tile it is in.
//!
//! Where the work is done: [`decode`] runs on the worker pool as the tile's decode step, so a
//! Streets GL tile stays loading (its coarser stand-in drawn) until Overture's tiles it needs are
//! in, as today; the job waits for them (a fetch through the tile runtime's fetcher, shared by the
//! jobs that need the same tile), and gives up when the tile is cancelled. Headless, the fixture
//! folder's archive answers at once ([`archive_tile`], read by `io::DirTiles`).
//!
//! Deviations, invisible: a roof's heights are in the tile frame's metres, as its walls' (deck's
//! `METER_OFFSETS` reads them at the tile's corner: within 0.1 % of the walls' metres); an
//! Overture tile whose request the fetcher dropped is asked again by the next tile that needs it
//! (today's failed promise stays cached as no buildings); a job waiting for an Overture tile
//! holds a worker of the pool (today's tile fetch waits off the main thread without one), so on
//! the network the first tiles of an area decode a little later than today.

use std::collections::{HashMap, VecDeque};
use std::f64::consts::PI;
use std::io::Read;
use std::path::Path;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rustradar_core::mvt::{self, GeomType};
use rustradar_scene::overture::{self, Building, Feature, Geometry, Height, Polygons};
use rustradar_scene::roofs::{RoofInput, pitched, roof_parts};
use rustradar_scene::streets_gl::{Properties, sgl_building};
use rustradar_scene::tess::buildings::{
    self as tess, BuildingLook, CITY_CLOSE_ZOOM, RealisticTile, RealisticVertex, ROOF_FACE, ROOF_OWN_COLOUR, SGL_SOURCE_LAYER, STYLE_WALL,
};
use rustradar_scene::tiles::{TileIndex, osm_tile_to_lng_lat};
use serde_json::Value;

use super::buildings_realistic::BuildingShader;
use super::{DrawCx, Init};
use crate::frame::FrameSlot;
use crate::io::{FetchError, FetchRequest, FetchResult};
use crate::jobs::Cancel;
use crate::pipelines::fill;
use crate::state::DrawState;
use crate::tiles::TileRuntime;

/// Overture's buildings archive, as the app's tile fetcher is asked for it (`backend::overture::
/// NAME`): zoom-14 tiles of MVT, gunzipped, layers `building` and `building_part`.
pub const OVERTURE: &str = "overture";

/// A fixture folder's archive of Overture's buildings (`backend::overture::FIXTURE`), in the
/// folder or beside its `tiles` folder.
pub const OVERTURE_FIXTURE: &str = "overture/buildings.pmtiles";

/// A window bay and a storey (`BAY_M`, `STOREY_M`): a gable's texture, in its units.
const BAY_M: f64 = tess::BAY_M;
const STOREY_M: f64 = tess::STOREY_M;

/// Web Mercator's equator (`EARTH_CIRCUMFERENCE` of `building-layer.ts`), metres.
const EARTH_CIRCUMFERENCE: f64 = 2.0 * PI * 6_378_137.0;
/// Degrees to radians (`DEG`, as `building-layer.ts` multiplies by it: a roof's corners are the
/// TypeScript's to the last bit).
const DEG: f64 = PI / 180.0;

/// How often a job waiting for an Overture tile looks whether it was cancelled.
const WAIT_STEP: Duration = Duration::from_millis(50);

// --- Overture's heights ----------------------------------------------------------------------------

/// A zoom-14 Overture tile as the map keeps it: its buildings with a height or floors.
enum Kept {
    /// A job is fetching it.
    Loading,
    Ready(Arc<Vec<Building>>),
}

/// What [`Overture`] keeps: the tiles by `(x, y)`, in the order first asked for.
struct Store {
    /// The tile runtime's generation the tiles came through (a new fetcher drops them, as it does
    /// every tile set's tiles).
    generation: u64,
    tiles: HashMap<(u32, u32), Kept>,
    order: VecDeque<(u32, u32)>,
}

impl Store {
    /// `tiles.set(key, …)`, then `while (tiles.size > 48) tiles.delete(oldest)`.
    fn insert_loading(&mut self, key: (u32, u32)) {
        self.tiles.insert(key, Kept::Loading);
        self.order.push_back(key);
        while self.tiles.len() > overture::TILES_CACHED {
            let Some(oldest) = self.order.pop_front() else { break };
            self.tiles.remove(&oldest);
        }
    }

    fn remove(&mut self, key: (u32, u32)) {
        self.tiles.remove(&key);
        self.order.retain(|&k| k != key);
    }
}

/// Overture's tiles for the map's Streets GL buildings (`tileBuildings`'s cache, `heightsAt`),
/// fetched through the tile runtime's fetcher as the source [`OVERTURE`]. Cheap to clone; every
/// clone shares the tiles. Used from the worker pool: [`Self::heights_at`] waits for the tiles it
/// needs.
#[derive(Clone)]
pub struct Overture(Arc<(Mutex<Store>, Condvar, TileRuntime)>);

impl std::fmt::Debug for Overture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Overture").field("tiles", &self.lock().tiles.len()).finish_non_exhaustive()
    }
}

/// The work stopped: its tile was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl Overture {
    /// Overture's tiles through `runtime`'s fetcher.
    pub fn new(runtime: &TileRuntime) -> Self {
        let store = Store { generation: runtime.generation(), tiles: HashMap::new(), order: VecDeque::new() };
        Overture(Arc::new((Mutex::new(store), Condvar::new(), runtime.clone())))
    }

    fn lock(&self) -> MutexGuard<'_, Store> {
        let mut store = self.0.0.lock().unwrap_or_else(PoisonError::into_inner);
        let generation = self.0.2.generation();
        if store.generation != generation {
            *store = Store { generation, tiles: HashMap::new(), order: VecDeque::new() };
        }
        store
    }

    /// How many tiles it keeps (loaded or loading).
    pub fn len(&self) -> usize {
        self.lock().tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The zoom-14 tile `x`/`y`'s buildings with a height or floors (`tileBuildings`): kept, being
    /// fetched by another job (waited for), or fetched now. None where it can't be had.
    pub fn tile(&self, x: u32, y: u32, cancel: &Cancel) -> Result<Arc<Vec<Building>>, Cancelled> {
        let key = (x, y);
        let mut store = self.lock();
        loop {
            match store.tiles.get(&key) {
                Some(Kept::Ready(buildings)) => return Ok(Arc::clone(buildings)),
                Some(Kept::Loading) => {
                    if cancel.is_cancelled() {
                        return Err(Cancelled);
                    }
                    store = self.0.1.wait_timeout(store, WAIT_STEP).unwrap_or_else(PoisonError::into_inner).0;
                    // (a new fetcher meanwhile drops what was kept)
                    drop(store);
                    store = self.lock();
                }
                None => break,
            }
        }
        store.insert_loading(key);
        let generation = store.generation;
        drop(store);
        let answer = self.fetch(x, y, cancel);
        let mut store = self.lock();
        let still_ours = store.generation == generation && matches!(store.tiles.get(&key), Some(Kept::Loading));
        let result = match answer {
            None => {
                // nobody waits for this one: the next to need it asks again
                if still_ours {
                    store.remove(key);
                }
                Err(Cancelled)
            }
            Some(Err(FetchError::Cancelled)) => {
                if still_ours {
                    store.remove(key);
                }
                Ok(Arc::new(Vec::new()))
            }
            Some(answer) => {
                // `.catch(() => [])`: a tile that can't be had is kept as one without buildings
                let buildings = Arc::new(match answer {
                    Ok(Some(bytes)) => tall_buildings_of(&bytes, x, y),
                    _ => Vec::new(),
                });
                if still_ours {
                    store.tiles.insert(key, Kept::Ready(Arc::clone(&buildings)));
                }
                Ok(buildings)
            }
        };
        drop(store);
        self.0.1.notify_all();
        result
    }

    /// Asks the fetcher for tile `x`/`y` and waits for the answer; `None` once `cancel` is set.
    fn fetch(&self, x: u32, y: u32, cancel: &Cancel) -> Option<FetchResult> {
        let asked = Cancel::new();
        let request = FetchRequest { source: Arc::from(OVERTURE), tile: TileIndex::new(x, y, overture::ZOOM), cancel: asked.clone() };
        let answer = self.0.2.fetcher().fetch(request);
        loop {
            match answer.recv_timeout(WAIT_STEP) {
                Ok(result) => return Some(result),
                Err(RecvTimeoutError::Disconnected) => return Some(Err(FetchError::Cancelled)),
                Err(RecvTimeoutError::Timeout) if cancel.is_cancelled() => {
                    asked.cancel();
                    return None;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// The tallest Overture building (or part) at each point (`heightsAt`): the points' zoom-14
    /// tiles each loaded once ([`Self::tile`]), then `scene::overture::heights_at`.
    pub fn heights_at(&self, points: &[[f64; 2]], cancel: &Cancel) -> Result<Vec<Option<Height>>, Cancelled> {
        let n = 1i64 << overture::ZOOM;
        let mut loaded: HashMap<(i64, i64), Vec<Building>> = HashMap::new();
        for (x, y) in overture::tiles_of(points) {
            // (a tile off the world has none: `getZxy` of it fails, and the failure is no buildings)
            let buildings = if (0..n).contains(&x) && (0..n).contains(&y) { self.tile(x as u32, y as u32, cancel)?.as_ref().clone() } else { Vec::new() };
            loaded.insert((x, y), buildings);
        }
        Ok(overture::heights_at(points, &loaded))
    }
}

/// An Overture tile's buildings with a height or floors (`tileBuildings`' parse and
/// `tallBuildings`): its `building` then `building_part` layers decoded as loaders.gl's MVT loader
/// hands them over as GeoJSON in longitude and latitude (`shape: "geojson"`, `coordinates:
/// "wgs84"`): each ring closed, rings of no area dropped (but a lone ring), grouped into polygons
/// by the turn of the first (`classifyRings`), projected as `projectToLngLat` does.
pub fn tall_buildings_of(mvt: &[u8], x: u32, y: u32) -> Vec<Building> {
    let mut features = Vec::new();
    for name in overture::LAYERS {
        let Some(layer) = mvt::layer(mvt, name) else { continue };
        let extent = layer.extent;
        let size = extent * 2f64.powi(overture::ZOOM as i32);
        let (x0, y0) = (extent * f64::from(x), extent * f64::from(y));
        let lng_lat = |(px, py): (f64, f64)| -> [f64; 2] {
            let lon = ((px + x0) * 360.0) / size - 180.0;
            let y2 = 180.0 - ((py + y0) * 360.0) / size;
            [lon, (360.0 / PI) * ((y2 * PI) / 180.0).exp().atan() - 90.0]
        };
        for feature in layer.features() {
            let geometry = match feature.kind {
                GeomType::Polygon => {
                    let rings: Vec<Vec<(f64, f64)>> = feature
                        .rings()
                        .into_iter()
                        .map(|mut ring| {
                            ring.push(ring[0]);
                            ring
                        })
                        .collect();
                    let polygons: Vec<Vec<Vec<[f64; 2]>>> =
                        classify_rings(rings).into_iter().map(|polygon| polygon.into_iter().map(|ring| ring.into_iter().map(lng_lat).collect()).collect()).collect();
                    match <[_; 1]>::try_from(polygons) {
                        Ok([polygon]) => Geometry::Polygon(polygon),
                        Err(polygons) => Geometry::MultiPolygon(polygons),
                    }
                }
                _ => Geometry::Other,
            };
            features.push(Feature { geometry, properties: feature.json_properties() });
        }
    }
    overture::tall_buildings(features)
}

/// loaders.gl's `classifyRings`: rings of no area left out (`signedArea`), the first one's turn
/// starting each polygon and the other turn a hole of the one before; a lone ring is a polygon
/// whatever its area.
fn classify_rings(rings: Vec<Vec<(f64, f64)>>) -> Vec<Vec<Vec<(f64, f64)>>> {
    if rings.len() <= 1 {
        return vec![rings];
    }
    let signed_area = |ring: &[(f64, f64)]| {
        let mut sum = 0.0;
        let mut j = ring.len().wrapping_sub(1);
        for i in 0..ring.len() {
            let (p1, p2) = (ring[i], ring[j]);
            sum += (p2.0 - p1.0) * (p1.1 + p2.1);
            j = i;
        }
        sum
    };
    let mut polygons = Vec::new();
    let mut polygon: Option<Vec<Vec<(f64, f64)>>> = None;
    let mut ccw = None;
    for ring in rings {
        let area = signed_area(&ring);
        if area == 0.0 {
            continue;
        }
        let ccw = *ccw.get_or_insert(area < 0.0);
        if ccw == (area < 0.0) {
            if let Some(done) = polygon.take() {
                polygons.push(done);
            }
            polygon = Some(vec![ring]);
        } else if let Some(polygon) = &mut polygon {
            polygon.push(ring);
        }
    }
    polygons.extend(polygon);
    polygons
}

/// Whether a layer's `key` is a numeric column of loaders.gl's binary tile (`deduceArrayType`):
/// every feature that has it gives a finite number.
fn numeric_column(layer: &mvt::Layer<'_>, key: &str) -> bool {
    layer.features().all(|f| match f.get(key) {
        None => true,
        Some(&mvt::Value::Num(n)) => n.is_finite(),
        Some(_) => false,
    })
}

/// What a Streets GL tile's buildings are given by Overture (`withOvertureHeights`), applied to
/// each building's properties by [`Self::apply`] before they are read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Given {
    /// Whether anything was found at all (else nothing changes).
    found: bool,
    /// Whether `height` and `levels` are numeric columns of the tile.
    numeric: [bool; 2],
    /// By the feature's place in its layer: what Overture has for it.
    heights: HashMap<usize, Height>,
}

impl Given {
    /// How many buildings were given a height or levels.
    pub fn len(&self) -> usize {
        self.heights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }

    /// The `index`th feature's properties as the layer reads them once Overture's heights are in:
    /// `height` and `levels` from the `Float32Array` columns `withOvertureHeights` makes (each
    /// value rounded to `f32`, 0 where the column was not numeric), Overture's own height (else
    /// its floors as levels) where it was given one.
    pub fn apply(&self, index: usize, p: &mut Properties) {
        if !self.found {
            return;
        }
        let column = |p: &Properties, name: &str, numeric: bool| if numeric { p.get(name).and_then(Value::as_f64).unwrap_or(0.0) as f32 } else { 0.0 };
        let (mut height, mut levels) = (column(p, "height", self.numeric[0]), column(p, "levels", self.numeric[1]));
        if let Some(f) = self.heights.get(&index) {
            if f.height > 0.0 {
                height = f.height as f32;
            } else {
                levels = f.floors as f32;
            }
        }
        p.insert("height".into(), Value::from(f64::from(height)));
        p.insert("levels".into(), Value::from(f64::from(levels)));
    }
}

/// Overture's heights for the buildings of Streets GL's tile `tile` (`bytes`, as served):
/// `withOvertureHeights` with `heights_at` answering for the buildings' middles. Nothing is asked
/// where every building has a height or levels.
pub fn overture_heights(
    tile: TileIndex,
    bytes: &[u8],
    heights_at: impl FnOnce(&[[f64; 2]]) -> Result<Vec<Option<Height>>, Cancelled>,
) -> Result<Given, Cancelled> {
    let Some(layer) = mvt::layers_named(bytes, &[SGL_SOURCE_LAYER]).into_iter().next() else { return Ok(Given::default()) };
    let numeric = [numeric_column(&layer, "height"), numeric_column(&layer, "levels")];
    // the tile's polygons as loaders.gl's binary tile holds them: each vertex its feature
    let mut polygons = Polygons { size: 2, ..Polygons::default() };
    let mut columns: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
    let extent = layer.extent;
    for (index, feature) in layer.features().enumerate() {
        if feature.kind != GeomType::Polygon {
            continue;
        }
        let values = ["height", "levels"].map(|name| match feature.get(name) {
            Some(&mvt::Value::Num(n)) => n as f32,
            _ => 0.0,
        });
        for ring in tess::binary_polygons(&feature, extent).iter().flatten() {
            for &[u, v] in ring {
                polygons.positions.extend([f64::from(u), f64::from(v)]);
                polygons.feature_ids.push(index as u32);
                for (column, value) in columns.iter_mut().zip(values) {
                    column.push(value);
                }
            }
        }
    }
    for ((name, column), numeric) in ["height", "levels"].into_iter().zip(columns).zip(numeric) {
        if numeric {
            polygons.numeric_props.insert(name.to_owned(), column);
        }
    }
    let spans = overture::unheighted(&polygons, tile.x, tile.y, tile.z);
    if spans.is_empty() {
        return Ok(Given::default());
    }
    let points: Vec<[f64; 2]> = spans.iter().map(|s| s.at).collect();
    let found = heights_at(&points)?;
    let mut given = Given { found: found.iter().any(Option::is_some), numeric, heights: HashMap::new() };
    for (span, height) in spans.iter().zip(found) {
        if let Some(height) = height {
            given.heights.insert(polygons.feature_ids[span.start] as usize, height);
        }
    }
    Ok(given)
}

// --- the roofs ---------------------------------------------------------------------------------------

/// deck's `METER_OFFSETS` from the tile's north-west corner (`[west, north, 0]`), as its
/// `project_offset_` reads `x` metres east and `y` north there: math.gl's units per metre at the
/// corner's latitude (`getDistanceScales` with `highPrecision`: 512 units round 40 030 km), the
/// east one changing with `y` (`unitsPerMeter2`). In the tile's units (`v` south).
pub fn deck_meter_offset(tile: TileIndex, x: f64, y: f64) -> [f64; 2] {
    const WORLD_SIZE: f64 = 512.0;
    const DEGREES_TO_RADIANS: f64 = PI / 180.0;
    let [_, north] = osm_tile_to_lng_lat(f64::from(tile.x), f64::from(tile.y), tile.z);
    let lat_cosine = (north * DEGREES_TO_RADIANS).cos();
    let units_per_degree_y = (WORLD_SIZE / 360.0) / lat_cosine;
    let units_per_metre = WORLD_SIZE / rustradar_scene::camera::MERCATOR_EARTH_CIRCUMFERENCE_M / lat_cosine;
    let lat_cosine2 = (DEGREES_TO_RADIANS * (north * DEGREES_TO_RADIANS).tan()) / lat_cosine;
    let units_per_degree2 = (WORLD_SIZE / rustradar_scene::camera::MERCATOR_EARTH_CIRCUMFERENCE_M) * lat_cosine2;
    let units_per_metre2 = (units_per_degree2 / units_per_degree_y) * units_per_metre;
    let units_per_tile = WORLD_SIZE / 2f64.powi(tile.z as i32);
    [x * (units_per_metre + units_per_metre2 * y) / units_per_tile, -(y * units_per_metre) / units_per_tile]
}

/// JavaScript's `String(v)` of a property's value.
fn js_string(v: &Value) -> String {
    match v {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => rustradar_core::num::js_string(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => s.clone(),
        // (an array's `String` joins its items, null as nothing)
        Value::Array(items) => items.iter().map(|v| if v.is_null() { String::new() } else { js_string(v) }).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// JavaScript's `Number(v)` of a property (`undefined` is NaN).
fn js_number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(v) if v.is_null() => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => {
            let s = s.trim();
            match s {
                "" => 0.0,
                "Infinity" | "+Infinity" => f64::INFINITY,
                "-Infinity" => f64::NEG_INFINITY,
                _ if s.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E')) => s.parse().unwrap_or(f64::NAN),
                _ => f64::NAN,
            }
        }
        Some(_) => f64::NAN,
    }
}

/// How the building shader reads every corner of today's roof meshes: deck 9.4's SimpleMeshLayer
/// keeps only a mesh's positions, colours, normals and texture coordinates
/// (`normalizeGeometryAttributes`), so `rr_info` never reaches the shader, which reads WebGL's
/// default for an attribute with no buffer, `(0, 0, 0, 1)`: code 0 (plaster, tar), no bays, a wall.
pub const AS_DRAWN: [u8; 4] = [tess::PLASTER, tess::TAR, 0, STYLE_WALL];

/// A tile's pitched roofs as `tileRoofs` hands them to deck: each corner of each triangle (three a
/// triangle) with the mesh's attributes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TileRoofs {
    /// East, north and up, metres from the tile's north-west corner (`METER_OFFSETS`).
    pub positions: Vec<[f64; 3]>,
    pub normals: Vec<[f64; 3]>,
    /// `texCoords`: a gable's in bays and storeys, a roof face's in metres along it.
    pub tex_coords: Vec<[f64; 2]>,
    /// `colors` as bytes (the mesh has them over 255): the building's wall colour, or a roof
    /// face's own mapped colour.
    pub colors: Vec<[u8; 3]>,
    /// `rr_info`: the building's code (`255 − alpha` of its look), 0, a roof face's kind (0 for a
    /// gable, [`ROOF_FACE`], [`ROOF_OWN_COLOUR`]), 1 for a gable.
    pub info: Vec<[u8; 4]>,
}

impl TileRoofs {
    pub fn len(&self) -> usize {
        self.info.len()
    }

    pub fn is_empty(&self) -> bool {
        self.info.is_empty()
    }

    /// As the building shader's vertices in `tile`'s frame: each corner where deck draws it
    /// ([`deck_meter_offset`]), its colour with its building's alpha, its code as façade and roof.
    pub fn vertices(&self, tile: TileIndex) -> Vec<RealisticVertex> {
        (0..self.len())
            .map(|k| {
                let [x, y, z] = self.positions[k];
                let [u, v] = deck_meter_offset(tile, x, y);
                let [r, g, b] = self.colors[k];
                RealisticVertex {
                    position: [u as f32, v as f32, z as f32],
                    normal: self.normals[k].map(|n| n as f32),
                    uv: self.tex_coords[k].map(|t| t as f32),
                    tint: [r, g, b, 255],
                    style: AS_DRAWN,
                }
            })
            .collect()
    }
}

/// The pitched roofs of Streets GL's tile `tile` (`bytes`, as served; `tileRoofs`), each
/// building's properties first handed to `adjust` with its place in its layer (Overture's
/// heights, [`Given::apply`]). `cancelled` is asked between buildings.
pub fn tile_roofs(tile: TileIndex, bytes: &[u8], adjust: &dyn Fn(usize, &mut Properties), cancelled: &dyn Fn() -> bool) -> Result<TileRoofs, Cancelled> {
    let mut out = TileRoofs::default();
    let [_, lat] = tess::tile_middle(tile);
    // a tile's width in metres on the ground, at its middle
    let tile_metres = (EARTH_CIRCUMFERENCE * (lat * DEG).cos()) / 2f64.powi(tile.z as i32);
    for layer in mvt::layers_named(bytes, &[SGL_SOURCE_LAYER]) {
        for (index, feature) in layer.features().enumerate() {
            if cancelled() {
                return Err(Cancelled);
            }
            if feature.kind != GeomType::Polygon {
                continue;
            }
            let polygons = tess::binary_polygons(&feature, layer.extent);
            if polygons.is_empty() {
                continue;
            }
            let mut p = feature.json_properties();
            p.insert("layerName".into(), Value::from(SGL_SOURCE_LAYER));
            adjust(index, &mut p);
            let b = sgl_building(&p);
            if !pitched(b.roof_type) || b.roof_height <= 0.0 || p.get("buildingType").and_then(Value::as_str) == Some("roof") {
                continue;
            }
            // `String(properties.roofOrientation ?? "")`, `Number(properties.roofDirection) || undefined`
            let orientation = match p.get("roofOrientation") {
                None | Some(Value::Null) => String::new(),
                Some(v) => js_string(v),
            };
            let direction = Some(js_number(p.get("roofDirection"))).filter(|d| *d != 0.0 && !d.is_nan());
            // a roof colour mapped, its own (toned down as a wall's: `hexRgb`); else the shader's
            // for its kind
            let roof_colour = b.roof_colour.as_deref().map(|c| tess::building_style(0, 0.0, Some(c), BuildingLook::default()));
            for rings in &polygons {
                // its rings, in metres (the tile's y runs south)
                let own: Vec<Vec<[f64; 2]>> =
                    rings.iter().map(|ring| ring.iter().map(|&[u, v]| [f64::from(u) * tile_metres, -f64::from(v) * tile_metres]).collect()).collect();
                let parts = roof_parts(&RoofInput {
                    ring: &own[0],
                    holes: &own[1..],
                    base: b.height - b.roof_height,
                    height: b.roof_height,
                    shape: b.roof_type,
                    orientation: Some(&orientation),
                    direction,
                });
                if parts.walls.is_empty() {
                    continue;
                }
                let style = tess::building_style(tess::style_seed(tile, rings), tess::realistic_height(&p, &b), b.colour.as_deref(), tess::sgl_look(&b));
                let code = 255 - style[3];
                for (k, &wall) in parts.walls.iter().enumerate() {
                    out.positions.push([parts.positions[k * 3], parts.positions[k * 3 + 1], parts.positions[k * 3 + 2]]);
                    out.normals.push([parts.normals[k * 3], parts.normals[k * 3 + 1], parts.normals[k * 3 + 2]]);
                    let [u, v] = [parts.uvs[k * 2], parts.uvs[k * 2 + 1]];
                    out.tex_coords.push(if wall { [u / BAY_M, v / STOREY_M] } else { [u, v] });
                    let colour = match roof_colour {
                        Some(c) if !wall => c,
                        _ => style,
                    };
                    out.colors.push([colour[0], colour[1], colour[2]]);
                    let kind = match (wall, roof_colour) {
                        (true, _) => 0,
                        (false, Some(_)) => ROOF_OWN_COLOUR,
                        (false, None) => ROOF_FACE,
                    };
                    out.info.push([code, 0, kind, u8::from(wall)]);
                }
            }
        }
    }
    Ok(out)
}

/// The pitched roofs of Streets GL's tile `tile` as triangles in the tile's frame, three vertices
/// each ([`tile_roofs`], [`TileRoofs::vertices`]).
pub fn roof_vertices(
    tile: TileIndex,
    bytes: &[u8],
    adjust: &dyn Fn(usize, &mut Properties),
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<RealisticVertex>, Cancelled> {
    Ok(tile_roofs(tile, bytes, adjust, cancelled)?.vertices(tile))
}

/// A realistic tile's decode step (the buildings layer's, on the worker pool): Streets GL's tiles
/// with Overture's heights (when `overture` is given; none without) and their pitched roofs, then
/// `tess::tessellate_realistic_with`; OpenFreeMap's as R4 decodes them. `None` for a tile with no
/// buildings.
pub fn decode(tile: TileIndex, bytes: &[u8], cancel: &Cancel, overture: Option<&Overture>) -> Result<Option<RealisticTile>, String> {
    let cancelled = || cancel.is_cancelled();
    let sgl = tile.z >= rustradar_scene::streets_gl::SGL_ZOOM;
    let given = match overture {
        Some(overture) if sgl => overture_heights(tile, bytes, |points| overture.heights_at(points, cancel)).map_err(|Cancelled| "cancelled".to_owned())?,
        _ => Given::default(),
    };
    let adjust = |index: usize, p: &mut Properties| given.apply(index, p);
    let mut decoded = match tess::tessellate_realistic_with(tile, bytes, CITY_CLOSE_ZOOM, &cancelled, &adjust) {
        Ok(decoded) => decoded,
        Err(tess::Cancelled) => return Err("cancelled".into()),
    };
    // (only Streets GL's tiles say how their roofs are shaped; far tiles have none)
    if decoded.sgl && decoded.extruded {
        decoded.roofs = roof_vertices(tile, bytes, &adjust, &cancelled).map_err(|Cancelled| "cancelled".to_owned())?;
    }
    Ok((!decoded.is_empty()).then_some(decoded))
}

/// A tile as the layer decodes it from the committed fixtures (with the fixture archive's Overture
/// heights): the CPU twin of what the map's tests draw.
#[cfg(test)]
pub(crate) fn decode_from_fixtures(tile: TileIndex, bytes: &[u8], cancel: &Cancel) -> Result<Option<RealisticTile>, String> {
    static FIXTURES: std::sync::OnceLock<Overture> = std::sync::OnceLock::new();
    let overture = FIXTURES.get_or_init(|| Overture::new(&TileRuntime::new(Arc::new(crate::io::DirTiles::fixtures()), Arc::new(crate::jobs::Pool::new(1)))));
    decode(tile, bytes, cancel, Some(overture))
}

/// Which pixels of `view` the pitched roofs of `tiles` cover (each triangle projected and filled,
/// a pixel round it too, for its antialiased edge), whatever stands in front: for the map's tests
/// that read the ground between the buildings (a grey roof face can pass for shaded ground).
#[cfg(test)]
pub(crate) fn roof_cover(view: &crate::frame::ViewFrame, tiles: &[(TileIndex, RealisticTile)]) -> impl Fn(u32, u32) -> bool + use<> {
    let (w, h) = (view.globals.viewport[0] as usize, view.globals.viewport[1] as usize);
    let mut cover = vec![false; w * h];
    for (tile, decoded) in tiles {
        let screen = |p: [f32; 3]| {
            let [lon, lat] = osm_tile_to_lng_lat(f64::from(tile.x) + f64::from(p[0]), f64::from(tile.y) + f64::from(p[1]), tile.z);
            let [x, y, _] = view.viewport.project([lon, lat, f64::from(p[2])]);
            [x, y]
        };
        for t in decoded.roofs.chunks_exact(3) {
            let [a, b, c] = [screen(t[0].position), screen(t[1].position), screen(t[2].position)];
            let cross = |o: [f64; 2], p: [f64; 2], q: [f64; 2]| (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0]);
            let area = cross(a, b, c);
            if area == 0.0 || !area.is_finite() {
                continue;
            }
            let lo = |i: usize| (a[i].min(b[i]).min(c[i]).floor() - 1.0).max(0.0) as usize;
            let hi = |i: usize, n: usize| ((a[i].max(b[i]).max(c[i]).ceil() + 1.0).max(0.0) as usize).min(n);
            for y in lo(1)..hi(1, h) {
                for x in lo(0)..hi(0, w) {
                    // within a pixel of the triangle: its middle or a corner of the pixel round it in it
                    let inside = |p: [f64; 2]| {
                        let (u, v, s) = (cross(b, c, p) / area, cross(c, a, p) / area, cross(a, b, p) / area);
                        u >= 0.0 && v >= 0.0 && s >= 0.0
                    };
                    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                    if [[0.0, 0.0], [-1.5, -1.5], [1.5, -1.5], [-1.5, 1.5], [1.5, 1.5]].iter().any(|d| inside([px + d[0], py + d[1]])) {
                        cover[y * w + x] = true;
                    }
                }
            }
        }
    }
    move |x: u32, y: u32| cover.get(y as usize * w + x as usize).copied().unwrap_or(false)
}

// --- on the GPU ----------------------------------------------------------------------------------------

/// A tile's roofs on the GPU (`None`: it has none).
pub fn upload(device: &wgpu::Device, roofs: &[RealisticVertex]) -> Option<fill::Mesh<RealisticVertex>> {
    if roofs.is_empty() {
        return None;
    }
    let indices: Vec<u32> = (0..roofs.len() as u32).collect();
    Some(fill::Mesh::new(device, "realistic roofs", roofs, &indices))
}

/// Row 9.4 of doc 04 §1, `…-roofs`: the pipeline that draws a tile's roofs with the buildings'
/// shader, after its buildings.
pub struct Roofs {
    pipeline: wgpu::RenderPipeline,
}

impl Roofs {
    pub fn new(init: &Init<'_>, shader: &BuildingShader) -> Self {
        Roofs { pipeline: shader.pipeline(init, "realistic roofs", "vs_roof", "fs_roof", DrawState::NO_CULL) }
    }

    /// Draws `mesh` in the tile frame `slot`, with `shader`'s light and textures (prepared this
    /// frame by the buildings), standing on the ground `standing` says is under its vertices (Z3).
    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        cx: &DrawCx<'_>,
        shader: &BuildingShader,
        slot: FrameSlot,
        mesh: &fill::Mesh<RealisticVertex>,
        standing: &crate::layers::terrain::ground::Standing,
    ) {
        shader.begin(pass, &self.pipeline);
        cx.bind_frame(pass, slot);
        standing.bind(pass, 1);
        mesh.draw(pass);
    }
}

// --- a fixture folder's archive ------------------------------------------------------------------------

/// The fixture folder of `root` (a folder of tiles, or the folder holding them): its archive of
/// Overture's buildings ([`OVERTURE_FIXTURE`], in `root` or beside it), as
/// `backend::overture::Archive::fixture` finds it.
pub fn fixture_archive(root: &Path) -> Option<std::path::PathBuf> {
    [root.join(OVERTURE_FIXTURE), root.join("..").join(OVERTURE_FIXTURE)].into_iter().find(|p| p.is_file())
}

/// PMTiles v3's tile id of `z`/`x`/`y` (the tiles of the zooms below, then the Hilbert curve's
/// place in the zoom; `zxyToTileId`).
fn tile_id(z: u32, x: u64, y: u64) -> u64 {
    let base = ((1u64 << (2 * z)) - 1) / 3;
    let (mut x, mut y) = (x, y);
    let mut d = 0;
    let mut s = (1u64 << z) / 2;
    while s > 0 {
        let rx = u64::from(x & s > 0);
        let ry = u64::from(y & s > 0);
        d += s * s * ((3 * rx) ^ ry);
        if ry == 0 {
            if rx == 1 {
                x = s - 1 - (x & (s - 1));
                y = s - 1 - (y & (s - 1));
            }
            std::mem::swap(&mut x, &mut y);
        }
        s /= 2;
    }
    base + d
}

/// One entry of a PMTiles directory: tiles `tile_id..tile_id + run_length` at `offset`, or (run
/// length 0) a leaf directory there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    tile_id: u64,
    offset: u64,
    length: u64,
    run_length: u64,
}

/// A PMTiles directory: varints of the count, the tile ids (as steps), the run lengths, the
/// lengths and the offsets (0: right after the entry before).
fn directory(bytes: &[u8]) -> Option<Vec<Entry>> {
    let mut at = 0;
    let mut varint = || -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *bytes.get(at)?;
            at += 1;
            v |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return Some(v);
            }
        }
        None
    };
    let count = usize::try_from(varint()?).ok()?;
    if count > bytes.len() {
        return None;
    }
    let mut entries = vec![Entry { tile_id: 0, offset: 0, length: 0, run_length: 0 }; count];
    let mut last = 0u64;
    for e in &mut entries {
        last = last.checked_add(varint()?)?;
        e.tile_id = last;
    }
    for e in &mut entries {
        e.run_length = varint()?;
    }
    for e in &mut entries {
        e.length = varint()?;
    }
    for i in 0..count {
        let v = varint()?;
        entries[i].offset = if v == 0 && i > 0 { entries[i - 1].offset + entries[i - 1].length } else { v.checked_sub(1)? };
    }
    Some(entries)
}

/// `bytes` as stored with `compression` (PMTiles: 1 none, 2 gzip).
fn decompressed(bytes: &[u8], compression: u8) -> Result<Vec<u8>, String> {
    match compression {
        0 | 1 => Ok(bytes.to_vec()),
        2 => {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(bytes).read_to_end(&mut out).map_err(|e| e.to_string())?;
            Ok(out)
        }
        other => Err(format!("compression {other} is not read here")),
    }
}

/// Tile `tile` of the PMTiles v3 archive in the file at `path`, as `backend::overture::Archive::
/// tile_now` reads it (pmtiles' `get_tile_decompressed`): `Ok(None)` where the archive holds none
/// (or none at that zoom). For fixture folders (`io::DirTiles`); the app's network archive is the
/// backend's.
pub fn archive_tile(path: &Path, tile: TileIndex) -> FetchResult {
    let failed = |why: String| FetchError::Failed(format!("{}: {why}", path.display()));
    let file = std::fs::read(path).map_err(|e| failed(e.to_string()))?;
    let header = file.get(..127).ok_or_else(|| failed("no PMTiles header".into()))?;
    if &header[..7] != b"PMTiles" || header[7] != 3 {
        return Err(failed("not a PMTiles v3 archive".into()));
    }
    let u64_at = |at: usize| u64::from_le_bytes(header[at..at + 8].try_into().expect("8 bytes"));
    let (root, leaves, data) = ((u64_at(8), u64_at(16)), u64_at(40), u64_at(56));
    let (internal, compression, min_zoom, max_zoom) = (header[97], header[98], header[100], header[101]);
    if tile.z > 31 || !(u32::from(min_zoom)..=u32::from(max_zoom)).contains(&tile.z) {
        return Ok(None);
    }
    let slice = |offset: u64, length: u64| -> Result<&[u8], FetchError> {
        let start = usize::try_from(offset).map_err(|e| failed(e.to_string()))?;
        let end = start.checked_add(usize::try_from(length).map_err(|e| failed(e.to_string()))?).ok_or_else(|| failed("a range past the end".into()))?;
        file.get(start..end).ok_or_else(|| failed("a range past the end".into()))
    };
    let id = tile_id(tile.z, u64::from(tile.x), u64::from(tile.y));
    let (mut offset, mut length) = root;
    // (the root, then at most three levels of leaves, as pmtiles reads them)
    for depth in 0..4 {
        let entries = directory(&decompressed(slice(offset, length)?, internal).map_err(failed)?).ok_or_else(|| failed("a broken directory".into()))?;
        let found = match entries.binary_search_by_key(&id, |e| e.tile_id) {
            Ok(i) => Some(entries[i]),
            Err(0) => None,
            Err(i) => Some(entries[i - 1]),
        };
        let Some(entry) = found else { return Ok(None) };
        if entry.run_length == 0 {
            if depth == 3 {
                break;
            }
            (offset, length) = (leaves + entry.offset, entry.length);
            continue;
        }
        if id >= entry.tile_id + entry.run_length {
            return Ok(None);
        }
        return decompressed(slice(data + entry.offset, entry.length)?, compression).map(Some).map_err(failed);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::Receiver;

    use rustradar_core::geo::Basemap;
    use rustradar_core::weather::PRESETS;
    use rustradar_scene::camera::ViewState;
    use rustradar_scene::lod::distance_tile_indices;
    use rustradar_scene::streets_gl::SGL_ZOOM;

    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{self, DirTiles, TileFetcher};
    use crate::layers::{buildings, earth, ground, sky};
    use crate::renderer::{MapRenderer, SceneInput};

    /// The committed archive: 4 zoom-14 tiles round the close-ups (plan, "Fixtures for stages
    /// 8-10").
    fn archive() -> PathBuf {
        fixture_archive(&io::fixture_tiles_dir()).expect("the committed archive")
    }

    /// The archive's tiles.
    const HELD: [(u32, u32); 4] = [(9013, 4817), (9013, 4818), (9014, 4817), (9014, 4818)];

    fn sgl_fixture(tile: TileIndex) -> Vec<u8> {
        DirTiles::fixtures().read(io::STREETS_GL, tile).expect("read").expect("a fixture tile")
    }

    fn shots_dir() -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    /// The fixture tiles, every request counted by source.
    struct Counted {
        tiles: DirTiles,
        overture: AtomicUsize,
    }

    impl TileFetcher for Counted {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            if &*request.source == OVERTURE {
                self.overture.fetch_add(1, Ordering::SeqCst);
            }
            self.tiles.fetch(request)
        }
    }

    #[test]
    fn pmtiles_ids_run_along_the_hilbert_curve() {
        // (the specification's examples)
        assert_eq!(tile_id(0, 0, 0), 0);
        assert_eq!([tile_id(1, 0, 0), tile_id(1, 0, 1), tile_id(1, 1, 1), tile_id(1, 1, 0)], [1, 2, 3, 4]);
        assert_eq!(tile_id(2, 0, 0), 5);
        assert_eq!(tile_id(3, 0, 0), 21);
        assert_eq!(tile_id(20, 0, 0), 366_503_875_925);
        // every tile of a zoom once, in 4^z places after the zooms below
        let mut ids: Vec<u64> = (0..8).flat_map(|x| (0..8).map(move |y| tile_id(3, x, y))).collect();
        ids.sort_unstable();
        assert_eq!(ids, (21..21 + 64).collect::<Vec<_>>());
    }

    /// The fixture folder's archive answers its four tiles (MVT with Overture's two layers), none
    /// for the tiles it lacks or another zoom; the folder of tiles and the folder beside it both
    /// find it, and `io::DirTiles` hands it out as the source [`OVERTURE`].
    #[test]
    fn a_fixture_archive_s_tiles() {
        let path = archive();
        for (x, y) in HELD {
            let mvt = archive_tile(&path, TileIndex::new(x, y, 14)).expect("read").expect("held");
            let names: Vec<&str> = mvt::layers(&mvt).map(|l| l.name).collect();
            assert_eq!(names, ["building", "building_part"], "{x}/{y}");
            assert_eq!(DirTiles::fixtures().read(OVERTURE, TileIndex::new(x, y, 14)), Ok(Some(mvt)));
        }
        assert_eq!(archive_tile(&path, TileIndex::new(9015, 4817, 14)), Ok(None));
        assert_eq!(archive_tile(&path, TileIndex::new(4506, 2408, 13)), Ok(None));
        assert!(archive_tile(Path::new("/nonexistent/buildings.pmtiles"), TileIndex::new(9013, 4817, 14)).is_err());
        assert_eq!(fixture_archive(&io::fixture_tiles_dir().join("..")), Some(path));
        assert!(fixture_archive(Path::new("/nonexistent")).is_none());
    }

    /// An Overture tile's buildings: thousands with a height or floors, each outline within the
    /// tile (with MVT's buffer) in longitude and latitude, the tallest Stockholm's towers.
    #[test]
    fn an_overture_tile_s_tall_buildings() {
        let (x, y) = (9014, 4818);
        let mvt = archive_tile(&archive(), TileIndex::new(x, y, 14)).expect("read").expect("held");
        let buildings = tall_buildings_of(&mvt, x, y);
        let [west, north] = osm_tile_to_lng_lat(f64::from(x), f64::from(y), 14);
        let [east, south] = osm_tile_to_lng_lat(f64::from(x + 1), f64::from(y + 1), 14);
        let (dx, dy) = ((east - west) / 4.0, (north - south) / 4.0);
        let tallest = buildings.iter().map(|b| b.height).fold(0.0, f64::max);
        let with_height = buildings.iter().filter(|b| b.height > 0.0).count();
        eprintln!("{} buildings with a height or floors ({with_height} with a height), the tallest {tallest} m", buildings.len());
        assert!(buildings.len() > 500, "{}", buildings.len());
        assert!((40.0..200.0).contains(&tallest), "{tallest}");
        for b in &buildings {
            assert!(b.height > 0.0 || b.floors > 0.0);
            assert!(b.ring.len() >= 4 && b.ring.first() == b.ring.last());
            let [w, s, e, n] = b.bbox;
            assert!(w >= west - dx && e <= east + dx && s >= south - dy && n <= north + dy, "{:?}", b.bbox);
        }
    }

    #[test]
    fn rings_classify_as_loaders_gl_s() {
        let square = |x: f64, s: f64| vec![(x, 0.0), (x, s), (x + s, s), (x + s, 0.0), (x, 0.0)];
        let mut hole = square(1.0, 1.0);
        hole.reverse();
        let flat = vec![(0.0, 0.0), (5.0, 0.0), (0.0, 0.0)];
        let polygons = classify_rings(vec![square(0.0, 4.0), flat.clone(), hole.clone(), square(10.0, 2.0)]);
        assert_eq!(polygons, vec![vec![square(0.0, 4.0), hole], vec![square(10.0, 2.0)]]);
        // a lone ring is a polygon whatever its area
        assert_eq!(classify_rings(vec![flat.clone()]), vec![vec![flat]]);
    }

    /// deck's metre offsets from a tile's corner: east 0.11 % past the tile's own metres (math.gl's
    /// 40 030 km against Web Mercator's radius, at the corner's latitude, not the middle's), north
    /// the same, the east one growing southwards with the latitude's cosine.
    #[test]
    fn meter_offsets_land_where_deck_puts_them() {
        let tile = TileIndex::new(36057, 19273, 16);
        let [_, lat] = tess::tile_middle(tile);
        let tile_metres = (EARTH_CIRCUMFERENCE * (lat * DEG).cos()) / 2f64.powi(16);
        let [u, v] = deck_meter_offset(tile, tile_metres, -tile_metres);
        // the far corner, in the tile's own metres, as deck draws it
        assert!((u - 1.00116).abs() < 1e-4 && (v - 1.00110).abs() < 1e-4, "{u} {v}");
        assert_eq!(deck_meter_offset(tile, 0.0, 0.0), [0.0, -0.0]);
        // north-west of the corner by a metre, and the east scale changing south
        let [a, _] = deck_meter_offset(tile, 1.0, 0.0);
        let [b, _] = deck_meter_offset(tile, 1.0, -300.0);
        assert!(b < a && (a - b) / a < 1e-4, "{a} {b}");
    }

    /// A building of Streets GL's fixture tile at V5 without a height or levels takes Overture's
    /// where the archive has one: about every such building whose middle lies in a building of the
    /// archive; the others keep theirs (as `f32`s); with no archive, nothing changes.
    #[test]
    fn streets_gl_s_unheighted_buildings_take_overture_s() {
        let tile = TileIndex::new(36057, 19273, 16);
        let bytes = sgl_fixture(tile);
        let runtime = TileRuntime::new(Arc::new(DirTiles::fixtures()), Arc::new(crate::jobs::Pool::new(1)));
        let overture = Overture::new(&runtime);
        let given = overture_heights(tile, &bytes, |points| overture.heights_at(points, &Cancel::new())).expect("not cancelled");
        let layer = mvt::layers_named(&bytes, &[SGL_SOURCE_LAYER]).into_iter().next().expect("the buildings");
        let unheighted =
            layer.features().filter(|f| f.kind == GeomType::Polygon && f.num_or_zero("height").is_none() && f.num_or_zero("levels").is_none()).count();
        eprintln!("{} of {} buildings without a height or levels took Overture's", given.len(), unheighted);
        assert!(given.found && given.len() * 2 > unheighted && given.len() <= unheighted, "{} of {unheighted}", given.len());
        assert_eq!(overture.len(), 1);
        // given: a height (else levels) where there was none; the others the same, but as f32
        let (mut taller, mut kept) = (0, 0);
        for (index, feature) in layer.features().enumerate() {
            let mut p = feature.json_properties();
            let before = (p.get("height").and_then(Value::as_f64), p.get("levels").and_then(Value::as_f64));
            given.apply(index, &mut p);
            let after = (p["height"].as_f64().expect("a number"), p["levels"].as_f64().expect("a number"));
            match given.heights.get(&index) {
                Some(h) if h.height > 0.0 => {
                    assert_eq!(after.0, f64::from(h.height as f32));
                    taller += 1;
                }
                Some(h) => assert_eq!(after.1, f64::from(h.floors as f32)),
                None => {
                    assert_eq!(after.0, f64::from(before.0.unwrap_or(0.0) as f32));
                    assert_eq!(after.1, f64::from(before.1.unwrap_or(0.0) as f32));
                    kept += 1;
                }
            }
        }
        assert!(taller > 0 && kept > 0);
        // the tile's decode takes them: a building's walls stand to Overture's height
        let plain = decode(tile, &bytes, &Cancel::new(), None).expect("decoded").expect("buildings");
        let heighted = decode(tile, &bytes, &Cancel::new(), Some(&overture)).expect("decoded").expect("buildings");
        let top = |t: &RealisticTile| t.vertices.iter().map(|v| v.position[2]).fold(0.0f32, f32::max);
        eprintln!("the tallest wall {} m, with Overture's heights {} m", top(&plain), top(&heighted));
        assert_ne!(plain.footprints.heights, heighted.footprints.heights);
        // an archive that has nothing asks once and changes nothing
        let empty = TileRuntime::offline(1);
        let none = Overture::new(&empty);
        let nothing = overture_heights(tile, &bytes, |points| none.heights_at(points, &Cancel::new())).expect("not cancelled");
        assert!(!nothing.found && nothing.is_empty());
        let mut p = layer.feature(0).expect("a building").json_properties();
        let before = p.clone();
        nothing.apply(0, &mut p);
        assert_eq!(p, before);
    }

    /// The store keeps each tile once for every tile that asks (one fetch), at most
    /// `TILES_CACHED` of them, the first asked for going first; a new fetcher drops them; a job
    /// cancelled while waiting stops.
    #[test]
    fn overture_s_tiles_are_kept_and_asked_once() {
        let counted = Arc::new(Counted { tiles: DirTiles::fixtures(), overture: AtomicUsize::new(0) });
        let runtime = TileRuntime::new(counted.clone(), Arc::new(crate::jobs::Pool::new(1)));
        let overture = Overture::new(&runtime);
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let overture = overture.clone();
                std::thread::spawn(move || overture.tile(9014, 4818, &Cancel::new()).expect("not cancelled").len())
            })
            .collect();
        let counts: Vec<usize> = threads.into_iter().map(|t| t.join().expect("joined")).collect();
        assert!(counts[0] > 500 && counts.iter().all(|&c| c == counts[0]), "{counts:?}");
        assert_eq!(counted.overture.load(Ordering::SeqCst), 1);
        for i in 0..overture::TILES_CACHED as u32 + 2 {
            overture.tile(i, 0, &Cancel::new()).expect("not cancelled");
        }
        assert_eq!(overture.len(), overture::TILES_CACHED);
        // the first went first
        overture.tile(9014, 4818, &Cancel::new()).expect("not cancelled");
        assert_eq!(counted.overture.load(Ordering::SeqCst), 2 + overture::TILES_CACHED + 2);
        runtime.set_fetcher(counted.clone());
        assert!(overture.is_empty());
        // a fetcher that never answers: the waiting job stops once cancelled
        struct Silent(Mutex<Vec<std::sync::mpsc::Sender<FetchResult>>>);
        impl TileFetcher for Silent {
            fn fetch(&self, _: FetchRequest) -> Receiver<FetchResult> {
                let (tx, rx) = std::sync::mpsc::channel();
                self.0.lock().unwrap().push(tx);
                rx
            }
        }
        runtime.set_fetcher(Arc::new(Silent(Mutex::new(Vec::new()))));
        let cancel = Cancel::new();
        let waiting = {
            let (overture, cancel) = (overture.clone(), cancel.clone());
            std::thread::spawn(move || overture.tile(1, 1, &cancel))
        };
        std::thread::sleep(Duration::from_millis(120));
        cancel.cancel();
        assert_eq!(waiting.join().expect("joined").map(|b| b.len()), Err(Cancelled));
        assert!(overture.is_empty());
    }

    /// The pitched roofs of Streets GL's fixture tiles in Gamla stan: triangles of roof faces
    /// (normals up the slope, the kinds `tileRoofs` gives them) and gables (walls, normals level);
    /// each roof's lowest corners at its building's eaves, where the walls stop; every building
    /// with a pitched roof roofed; on the GPU every corner as today's shader reads it.
    #[test]
    fn streets_gl_s_pitched_roofs() {
        let (mut faces, mut gables, mut own, mut buildings, mut roofed) = (0, 0, 0, 0, 0);
        // (Gamla stan's: the old town's roofs)
        for tile in [TileIndex::new(36057, 19275, 16), TileIndex::new(36057, 19274, 16), TileIndex::new(36056, 19275, 16), TileIndex::new(36058, 19275, 16)] {
            let bytes = sgl_fixture(tile);
            let roofs = tile_roofs(tile, &bytes, &|_, _| {}, &|| false).expect("not cancelled");
            assert_eq!(roofs.len() % 3, 0);
            for k in 0..roofs.len() {
                let n = roofs.normals[k];
                if roofs.info[k][3] == 1 {
                    gables += 1;
                    assert_eq!(roofs.info[k][2], 0);
                    assert!(n[2].abs() < 1e-9 && (n[0].hypot(n[1]) - 1.0).abs() < 1e-9, "{n:?}");
                } else {
                    faces += 1;
                    assert!(n[2] > 0.0 && n[2] < 1.0, "{n:?}");
                    assert!([ROOF_FACE, ROOF_OWN_COLOUR].contains(&roofs.info[k][2]));
                    own += usize::from(roofs.info[k][2] == ROOF_OWN_COLOUR);
                }
            }
            // on the GPU: where deck draws them, within the tile give or take the buildings over its
            // edge, each in its colour and as today's shader reads it
            let decoded = decode(tile, &bytes, &Cancel::new(), None).expect("decoded").expect("buildings");
            assert_eq!(decoded.roofs, roofs.vertices(tile));
            for (v, colour) in decoded.roofs.iter().zip(&roofs.colors) {
                assert_eq!((v.style, &v.tint[..3]), (AS_DRAWN, &colour[..]));
                assert!((-0.3..1.3).contains(&v.position[0]) && (-0.3..1.3).contains(&v.position[1]), "{:?}", v.position);
            }
            let layer = mvt::layers_named(&bytes, &[SGL_SOURCE_LAYER]).into_iter().next().expect("the buildings");
            for feature in layer.features().filter(|f| f.kind == GeomType::Polygon) {
                let mut p = feature.json_properties();
                p.insert("layerName".into(), Value::from(SGL_SOURCE_LAYER));
                let b = sgl_building(&p);
                if pitched(b.roof_type) && b.roof_height > 0.0 && p.get("buildingType").and_then(Value::as_str) != Some("roof") {
                    buildings += 1;
                    // its walls stop at the eaves
                    assert_eq!(tess::realistic_height(&p, &b), b.height - b.roof_height);
                    // (a footprint under a square metre, or too thin, has none)
                    let eaves = b.height - b.roof_height;
                    roofed += usize::from(roofs.positions.iter().any(|p| (p[2] - eaves).abs() < 1e-9));
                }
            }
        }
        eprintln!("{buildings} buildings with pitched roofs ({roofed} roofed from their eaves): {} roof triangles ({own} corners in their own colour), {} gable triangles", faces / 3, gables / 3);
        assert!(buildings >= 20 && faces > 0 && gables > 0, "{buildings} {faces} {gables}");
        assert!(roofed * 10 >= buildings * 9, "{roofed} of {buildings}");
    }

    /// Today's `tileRoofs` and `withOvertureHeights`, run under bun on Streets GL's fixture tiles
    /// in Gamla stan and at V5 (`building-layer.ts`'s `tileRoofs` as written, through the app's own
    /// `roofs.ts`, `streets-gl.ts`, `realistic.ts` and `overture.ts`, loaders.gl's binary tiles, and
    /// `fetch` answering from the committed archive): each tile's roof corners (positions to the
    /// millimetre, normals to 1e-5, texture coordinates to 1e-3, colours, `rr_info`) as an FNV-1a
    /// digest of their numbers as `i32`s, the first corner spelled out, before and after Overture's
    /// heights; and every building's `height` and `levels` after them (its place among the tile's
    /// polygons, millimetres).
    #[test]
    fn roofs_and_overture_heights_are_today_s() {
        type Today = (TileIndex, usize, u32, u32, u32, usize, [i64; 15]);
        let today: [Today; 5] = [
            (TileIndex::new(36057, 19275, 16), 2859, 0xe18a1264, 0xe18a1264, 0xbabed857, 186, [335960, -144453, 20000, -33851, -26510, 90285, 320869, 160114, 219, 213, 201, 2, 0, 1, 0]),
            (TileIndex::new(36057, 19274, 16), 2541, 0x75caa355, 0xc3a351c5, 0x76efaa6a, 152, [158434, -203959, 6518, -5384, 19214, 97989, -97523, 234837, 223, 215, 194, 11, 0, 1, 0]),
            (TileIndex::new(36056, 19275, 16), 471, 0xed90eee5, 0xed90eee5, 0x1da85ba1, 18, [197943, -71072, 28000, 15706, 2009, 98739, -95608, -184332, 224, 208, 142, 10, 0, 1, 0]),
            (TileIndex::new(36058, 19275, 16), 3576, 0xa8b5f1cc, 0xa8b5f1cc, 0x630c2df7, 129, [59258, -214049, 16101, -11837, -26156, 95790, 142241, -163365, 234, 217, 184, 10, 0, 1, 0]),
            (TileIndex::new(36057, 19273, 16), 339, 0xbe863425, 0xbe863425, 0xeba871df, 18, [55375, -66453, 20000, -15770, 29565, 94219, -17583, 82479, 202, 200, 194, 7, 0, 1, 0]),
        ];
        let fnv = |values: &[i64]| {
            let mut h: u32 = 0x811c_9dc5;
            for &v in values {
                for byte in (v as i32 as u32).to_le_bytes() {
                    h ^= u32::from(byte);
                    h = h.wrapping_mul(0x0100_0193);
                }
            }
            h
        };
        let round = |x: f64, by: f64| rustradar_core::num::js_round(x * by) as i64;
        let rows = |t: &TileRoofs| -> Vec<i64> {
            let mut v = Vec::new();
            for k in 0..t.len() {
                v.extend(t.positions[k].map(|n| round(n, 1000.0)));
                v.extend(t.normals[k].map(|n| round(n, 1e5)));
                v.extend(t.tex_coords[k].map(|n| round(n, 1000.0)));
                v.extend(t.colors[k].map(i64::from));
                v.extend(t.info[k].map(i64::from));
            }
            v
        };
        let runtime = TileRuntime::new(Arc::new(DirTiles::fixtures()), Arc::new(crate::jobs::Pool::new(1)));
        let overture = Overture::new(&runtime);
        for (tile, corners, plain_digest, overture_digest, heights_digest, features, first) in today {
            let bytes = sgl_fixture(tile);
            let plain = tile_roofs(tile, &bytes, &|_, _| {}, &|| false).expect("not cancelled");
            let got = rows(&plain);
            assert_eq!(plain.len(), corners, "{tile:?}");
            assert_eq!(got[..15], first, "{tile:?}");
            assert_eq!(fnv(&got), plain_digest, "{tile:?}");
            let given = overture_heights(tile, &bytes, |points| overture.heights_at(points, &Cancel::new())).expect("not cancelled");
            let with = tile_roofs(tile, &bytes, &|i, p| given.apply(i, p), &|| false).expect("not cancelled");
            assert_eq!(fnv(&rows(&with)), overture_digest, "{tile:?}");
            // every building's height and levels as the layer reads them, by its place among the
            // tile's polygons
            let layer = mvt::layers_named(&bytes, &[SGL_SOURCE_LAYER]).into_iter().next().expect("the buildings");
            let (mut heights, mut polygon) = (Vec::new(), 0);
            for (index, feature) in layer.features().enumerate() {
                if feature.kind != GeomType::Polygon {
                    continue;
                }
                polygon += 1;
                if tess::binary_polygons(&feature, layer.extent).is_empty() {
                    continue;
                }
                let mut p = feature.json_properties();
                given.apply(index, &mut p);
                let read = |key: &str| p.get(key).and_then(Value::as_f64).unwrap_or(0.0);
                heights.extend([polygon - 1, round(read("height"), 1000.0), round(read("levels"), 1000.0)]);
            }
            assert_eq!(heights.len() / 3, features, "{tile:?}");
            assert_eq!(fnv(&heights), heights_digest, "{tile:?}: {:?}", &heights[..12]);
        }
        // (the one zoom-14 tile all five lie in, asked once)
        assert_eq!(overture.len(), 1);
    }

    /// 2026-06-21 06:00 UTC: a summer morning in Stockholm, the sun low in the east (the faces
    /// towards it lit, the others in its shade).
    const MORNING: f64 = 1_782_021_600_000.0;
    /// Gamla stan close in, from the south: the old town's pitched roofs.
    const OLD_TOWN: ViewState = ViewState { latitude: 59.3245, longitude: 18.0710, zoom: 17.5, pitch: 55.0, bearing: 0.0, position: None };
    const SIZE: [f64; 2] = [1100.0, 720.0];

    fn realistic(view: ViewState, now: f64) -> SceneInput {
        let mut scene = SceneInput::new(view, SIZE, 1.0, Basemap::Realistic);
        scene.local_now_ms = now;
        scene.feed_now_ms = now;
        scene.earth = earth::Input::SHOWN;
        scene.sky = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
        scene.ground = ground::Input::SHOWN;
        scene.buildings = buildings::Input::SHOWN;
        scene
    }

    /// The fixture tiles, Streets GL's only at `keep` (none elsewhere).
    struct OnlyOne {
        tiles: DirTiles,
        keep: TileIndex,
    }

    impl TileFetcher for OnlyOne {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            if &*request.source == io::STREETS_GL && request.tile != self.keep {
                return io::answered(Ok(None));
            }
            self.tiles.fetch(request)
        }
    }

    /// `tile`'s roofs as the layer makes them (with the fixtures' Overture heights): today's mesh,
    /// and its vertices in the tile's frame.
    fn fixture_roofs(tile: TileIndex) -> Option<(TileRoofs, Vec<RealisticVertex>)> {
        let bytes = DirTiles::fixtures().read(io::STREETS_GL, tile).ok()??;
        let decoded = decode_from_fixtures(tile, &bytes, &Cancel::new()).ok()??;
        let runtime = TileRuntime::new(Arc::new(DirTiles::fixtures()), Arc::new(crate::jobs::Pool::new(1)));
        let overture = Overture::new(&runtime);
        let given = overture_heights(tile, &bytes, |points| overture.heights_at(points, &Cancel::new())).ok()?;
        let roofs = tile_roofs(tile, &bytes, &|i, p| given.apply(i, p), &|| false).ok()?;
        assert_eq!(decoded.roofs, roofs.vertices(tile));
        Some((roofs, decoded.roofs))
    }

    /// A point of a tile (its units, metres up) on the screen of `view`.
    fn screen(view: &ViewFrame, tile: TileIndex, p: [f32; 3]) -> glam::DVec2 {
        let [lon, lat] = osm_tile_to_lng_lat(f64::from(tile.x) + f64::from(p[0]), f64::from(tile.y) + f64::from(p[1]), tile.z);
        let [x, y, _] = view.viewport.project([lon, lat, f64::from(p[2])]);
        glam::DVec2::new(x, y)
    }

    fn luma(p: [u8; 4]) -> f64 {
        0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
    }

    /// The median of the 3 × 3 pixels round `at` (by luma).
    fn median_pixel(image: &Image, at: glam::DVec2) -> [u8; 4] {
        let mut patch: Vec<[u8; 4]> = (-1..=1).flat_map(|dy| (-1..=1).map(move |dx| (dx, dy))).map(|(dx, dy)| image.pixel((at.x as i32 + dx) as u32, (at.y as i32 + dy) as u32)).collect();
        patch.sort_by(|p, q| luma(*p).total_cmp(&luma(*q)));
        patch[4]
    }

    /// The roof shot: Gamla stan from the fixtures on a summer morning. Each large roof face
    /// turned to the camera is drawn where it stands, in its corners' colour as today's (a warm one
    /// warm, a grey one grey: its mapped roof colour, else its walls'); the faces turned most to the
    /// low sun lighter than those turned least.
    #[test]
    fn old_town_roofs_stand_in_their_colours() {
        let Some(gpu) = gpu() else { return };
        let fixtures = Arc::new(DirTiles::fixtures());
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(fixtures.clone(), None);
        let image = render_with(&gpu, &mut renderer, &realistic(OLD_TOWN, MORNING), 1.0).expect("rendered");
        assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
        image.save_png(&shots_dir().join("r6-old-town-roofs.png")).expect("png");
        let view = ViewFrame::new(&OLD_TOWN, SIZE, 1.0, 0.0);
        let camera = glam::Vec3::from(view.globals.camera_render).as_dvec3();
        let to_sun = rustradar_core::sun::local_sun(rustradar_core::sun::sun_direction(MORNING), OLD_TOWN.latitude, OLD_TOWN.longitude);
        let sun = glam::DVec3::from_array(to_sun).normalize();
        // (faces in a warm colour of their own, and faces in a grey roofing's: how many of each,
        // and how many drawn in that hue)
        let (mut warm, mut warm_drawn, mut grey, mut grey_drawn) = (0, 0, 0, 0);
        let (mut lit_by, mut seen) = (Vec::new(), 0);
        for tile in distance_tile_indices(&view.viewport, &tess::city_lod(), None, 0.0) {
            if tile.z != SGL_ZOOM {
                continue;
            }
            let Some((roofs, vertices)) = fixture_roofs(tile) else { continue };
            for (k, t) in vertices.chunks_exact(3).enumerate() {
                if roofs.info[3 * k][3] == 1 {
                    continue;
                }
                // the face's middle on the screen, and how big and how turned to the camera it is
                let middle: [f32; 3] = std::array::from_fn(|i| (t[0].position[i] + t[1].position[i] + t[2].position[i]) / 3.0);
                let [a, b, c] = [screen(&view, tile, t[0].position), screen(&view, tile, t[1].position), screen(&view, tile, t[2].position)];
                let area = ((b - a).perp_dot(c - a) / 2.0).abs();
                let m = screen(&view, tile, middle);
                if area < 400.0 || m.x < 4.0 || m.y < 4.0 || m.x > SIZE[0] - 4.0 || m.y > SIZE[1] - 4.0 {
                    continue;
                }
                let n = glam::DVec3::from_array(t[0].normal.map(f64::from)).normalize();
                let [lon, lat] = osm_tile_to_lng_lat(f64::from(tile.x) + f64::from(middle[0]), f64::from(tile.y) + f64::from(middle[1]), tile.z);
                let at = view.common_offset([lon, lat], f64::from(middle[2])).as_dvec3();
                if n.dot((camera - at).normalize()) < 0.3 {
                    continue;
                }
                // (a face hidden behind a taller building is not this face's colour: the median of
                // a small patch at its middle)
                let p = median_pixel(&image, m);
                seen += 1;
                // (today's faces in their corners' colour, as plaster walls: `AS_DRAWN`)
                let [r, g, b] = [t[0].tint[0], t[0].tint[1], t[0].tint[2]].map(i32::from);
                let [pr, pg, pb] = [p[0], p[1], p[2]].map(i32::from);
                if r > b + 40 && r >= g {
                    warm += 1;
                    warm_drawn += usize::from(pr > pb + 10);
                } else if (r - b).abs() <= 8 && (r - g).abs() <= 8 {
                    grey += 1;
                    grey_drawn += usize::from((pr - pb).abs() <= 24 && (pr - pg).abs() <= 24);
                }
                lit_by.push((n.dot(sun), luma(p)));
            }
        }
        // the quarter of the faces turned most to the sun against the quarter turned least
        lit_by.sort_by(|a, b| a.0.total_cmp(&b.0));
        let quarter = lit_by.len() / 4;
        let median = |v: &[(f64, f64)]| {
            let mut l: Vec<f64> = v.iter().map(|f| f.1).collect();
            l.sort_by(f64::total_cmp);
            (v[v.len() / 2].0, l[l.len() / 2])
        };
        let ((least, dark), (most, lit)) = (median(&lit_by[..quarter]), median(&lit_by[lit_by.len() - quarter..]));
        eprintln!(
            "{seen} roof faces seen; {warm_drawn} of {warm} in a warm colour drawn warm, {grey_drawn} of {grey} in a grey drawn grey; turned to the sun by {most:.2}, luma {lit:.1}, by {least:.2}, luma {dark:.1}"
        );
        assert!(seen >= 30, "{seen}");
        assert!(warm >= 3 && warm_drawn * 10 >= warm * 8, "{warm_drawn} of {warm}");
        assert!(grey >= 5 && grey_drawn * 10 >= grey * 8, "{grey_drawn} of {grey}");
        assert!(most > least + 0.2 && lit > dark + 15.0, "{lit} against {dark}");
    }

    /// A roof is drawn whole from its tile, not cut at the tile's edge (the roofs' SimpleMeshLayer
    /// has no `ClipExtension`): with only one Streets GL tile served, a roof face of a building
    /// over that tile's edge shows past the edge in the same colour as inside it, where without
    /// the tile the ground is.
    #[test]
    fn a_roof_over_its_tile_s_edge_is_drawn_whole() {
        let Some(gpu) = gpu() else { return };
        let view = ViewFrame::new(&OLD_TOWN, SIZE, 1.0, 0.0);
        // a roof face of a tile in view with a corner well past the tile's edge (0.6 m and more)
        // and one inside: the point a fifth of the way from its middle to each, on the screen
        let mut picked = None;
        'tiles: for tile in distance_tile_indices(&view.viewport, &tess::city_lod(), None, 0.0) {
            if tile.z != SGL_ZOOM {
                continue;
            }
            let Some((roofs, vertices)) = fixture_roofs(tile) else { continue };
            for (k, t) in vertices.chunks_exact(3).enumerate() {
                if roofs.info[3 * k][3] == 1 {
                    continue;
                }
                let past = |p: [f32; 3]| p[0] < -0.002 || p[0] > 1.002 || p[1] < -0.002 || p[1] > 1.002;
                let inside = |p: [f32; 3]| (0.002..0.998).contains(&p[0]) && (0.002..0.998).contains(&p[1]);
                let (Some(out), Some(within)) = (t.iter().find(|v| past(v.position)), t.iter().find(|v| inside(v.position))) else { continue };
                let middle: [f32; 3] = std::array::from_fn(|i| (t[0].position[i] + t[1].position[i] + t[2].position[i]) / 3.0);
                let toward = |p: [f32; 3], k: f32| -> [f32; 3] { std::array::from_fn(|i| p[i] + (middle[i] - p[i]) * k) };
                let (o, w) = (toward(out.position, 0.2), toward(within.position, 0.2));
                if !past(o) || !inside(w) {
                    continue;
                }
                let (so, sw) = (screen(&view, tile, o), screen(&view, tile, w));
                let on = |s: glam::DVec2| s.x > 8.0 && s.y > 8.0 && s.x < SIZE[0] - 8.0 && s.y < SIZE[1] - 8.0;
                let [a, b, c] = [screen(&view, tile, t[0].position), screen(&view, tile, t[1].position), screen(&view, tile, t[2].position)];
                if on(so) && on(sw) && ((b - a).perp_dot(c - a) / 2.0).abs() > 300.0 {
                    picked = Some((tile, so, sw));
                    break 'tiles;
                }
            }
        }
        let (tile, past, within) = picked.expect("a roof over a tile's edge in view");
        let shot = |keep: TileIndex| {
            let mut renderer = MapRenderer::new(&gpu, FORMAT);
            renderer.set_tiles(Arc::new(OnlyOne { tiles: DirTiles::fixtures(), keep }), None);
            let image = render_with(&gpu, &mut renderer, &realistic(OLD_TOWN, MORNING), 1.0).expect("rendered");
            assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
            image
        };
        let alone = shot(tile);
        alone.save_png(&shots_dir().join("r6-one-tile-roofs.png")).expect("png");
        // (no Streets GL tile at all: the zoom-16 tiles there are empty)
        let none = shot(TileIndex::new(0, 0, SGL_ZOOM));
        let (a, b, c) = (median_pixel(&alone, past), median_pixel(&alone, within), median_pixel(&none, past));
        eprintln!("tile {tile:?}: the roof past its edge {a:?}, inside it {b:?}; without the tile there {c:?}");
        let close = |p: [u8; 4], q: [u8; 4], d: u8| (0..3).all(|i| p[i].abs_diff(q[i]) <= d);
        assert!(close(a, b, 24), "{a:?} against {b:?}");
        assert!(!close(a, c, 12), "{a:?} against the ground's {c:?}");
    }
}
