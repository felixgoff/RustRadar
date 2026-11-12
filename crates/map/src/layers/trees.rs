//! The realistic map's trees (item R7; doc 04 §8; row 10 of doc 04 §1; row 8 of doc 03 §3):
//! `treeLayers` (`tree-layer.ts`, G:2695-2720) with `TreeExtension` and `TreeShadowExtension`
//! (`realistic.ts`, R:765-890): 3D trees wherever the tiles have woods, parks, gardens,
//! cemeteries, orchards, scrub or the gardens of residential streets, where the canopy height map
//! says trees stand, and the towns' registered trees, each one of Streets GL's pictures on two
//! crossed cards, lit by the realistic map's light, coloured by the season, with its shadow on
//! the ground. The planting is `rustradar_scene::trees` (`plantTile`); the shaders are
//! `shaders/tree.wgsl` and `shaders/tree_shadow.wgsl`.
//!
//! What today's app does, and so this layer:
//! - **When.** On the realistic basemap, on the flat map from zoom 14 (`TREES_ZOOM`), always (no
//!   setting); the scene builder shows it ([`Input::SHOWN`]) and the layer checks the rest.
//! - **Which tiles** ([`tiles_for`]): those within `min(sgl ? 2400 : 4800, max(900, 1.6·d))`
//!   metres of the view's centre, nearest first (`tilesAround`), `d` how far the camera is from the
//!   point it looks at (`CAMERA_HEIGHTS · height · metresPerPixel`): from zoom 15.5
//!   (`SGL_FROM_ZOOM`) Streets GL's zoom-16 tiles (layers `SGL_TREE_LAYERS`), else OpenFreeMap's
//!   zoom-14 tiles (`TREE_LAYERS`).
//! - **The tile cache** ([`TreeTiles`], the module's `cache`): a tile is planted once, from its
//!   features and what else is known of its ground, asked for at once: its land cover (NMD's
//!   picture in Sweden, WorldCover's where NMD is clear somewhere or not to be had: `landCover`),
//!   its canopy heights ([`CANOPY`], `backend::canopy`) and the towns' registered trees in it
//!   (`static/street-trees`, embedded); each of those may be missing, and the tile is planted
//!   without it; a tile that can't be read is failed and asked again only once it is forgotten. At
//!   most [`MAX_LOADING`] tiles load at once (a tile left out waits for a later frame); past
//!   [`MAX_CACHED`] tiles the ones used longest ago are forgotten. The planting runs on the worker
//!   pool.
//! - **Drawing**, tile by tile in that order: with the sun out (`sunStrength > 0.05` and the
//!   sun more than 0.02 up) the tile's trees' shadows (the cards laid on the ground away from the
//!   sun, cut to the pictures' shapes, `[6, 12, 28, round(min(0.6, 0.55·sunStrength)·255)]`
//!   fading into the air, with the shadows' stencil rule, `SHADOW_PARAMETERS`: a pixel shaded once,
//!   so never under a building's shadow too), then its trees (depth tested and written, both faces,
//!   cut to their pictures' shapes). The shadows' sun is held until it has moved half a degree
//!   (`treeShadows` cast again for each `az,alt` key).
//! - **Textures** (doc 04 §5.3): the realistic map's set; until it is in, the trees' mask is the
//!   stand-in of none, so no tree shows, and the layer is pending.
//!
//! How it is checked (the tests below): a fixture tile planted by this layer holds as many trees
//! as `plant_tile` plants from the same tile and surroundings, and as many as `tree-layer.ts`'s
//! `plantTile` plants under bun; the land cover and canopy read from the fixtures; at Humlegården
//! (zoom 16, the fixtures' park) at noon the trees stand in the park, each card cut to its
//! picture's shape, and their shadows lie on the ground on the side away from the sun, bluish and
//! once darkened. Only the realistic flat map from zoom 14 asks for tiles, Streets GL's from 15.5.
//!
//! Deviations, invisible: the shadows are the trees' own instances sheared by the vertex shader
//! for the held sun (`treeShadows` shears each tile's on the CPU, so a tile cast later takes the
//! sun of that moment, at most half a degree on); a tile forgotten while it loads stops loading
//! (today's promise runs on and still counts as loading until it ends); the GPU buffers of tiles
//! not drawn this frame are let go of, as deck finalizes the layers that leave the list; the cards
//! are sized at the view centre's metres, as deck's `project_size` does at these zooms; each tile's
//! land cover is asked for by this layer alone (today's `landCover` cache is shared with the land
//! cover ground's).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use bytemuck::{Pod, Zeroable};
use rustradar_core::geo::Basemap;
use rustradar_core::mvt::{self, GeomType};
use rustradar_core::num::js_round;
use rustradar_scene::building_shadow::shadow_step;
use rustradar_scene::camera::ViewState;
use rustradar_scene::canopy::{self as canopy_rules, CanopyGrid};
use rustradar_scene::landcover::LandGrid;
use rustradar_scene::street_trees::{RegisteredTree, Town, parse_town, towns_for_tile, trees_in_tile};
use rustradar_scene::streets_gl::{SGL_FROM_ZOOM, SGL_ZOOM};
use rustradar_scene::tess::vector::classify_rings;
use rustradar_scene::trees::{
    Feature, Geometry, SGL_TREE_LAYERS, Surroundings, TREE_LAYERS, TREE_TILE_ZOOM, TileAround, TileTrees, card_arrays, plant_tile, shadow_key,
    tiles_around,
};
pub use rustradar_scene::trees::{MAX_CACHED, MAX_LOADING};
use wgpu::util::DeviceExt;

use super::building_shadows::{SHADOW_COLOR, sun_strength};
use super::{DrawCx, Init, Layer, Pass, PrepareCx, TileIndex};
use crate::frame::FrameSlot;
use crate::io::{self, Fetch, FetchError, FetchResult, TileFetcher};
use crate::jobs::{Job, JobPoll, Pool};
use crate::layers::ground::load_textures;
use crate::look::{LightUniform, Look, camera_metres};
use crate::pipelines::{Desc, build};
use crate::shaders;
use crate::state::DrawState;
use crate::textures::Sampling;

/// `shaders/tree.wgsl`.
pub const SHADER: &str = include_str!("../shaders/tree.wgsl");
/// `shaders/tree_shadow.wgsl`.
pub const SHADOW_SHADER: &str = include_str!("../shaders/tree_shadow.wgsl");

/// The bind group of the trees' textures and their uniform (after the light's, group 2).
pub const TREE_GROUP: u32 = 3;

/// The canopy heights' source (`backend::canopy::NAME`): a tile's answer is its grid's heights,
/// row by row, `size²` bytes.
pub const CANOPY: &str = "canopy";
/// A `--fixture` folder's recording of the canopy files (`backend::canopy::FIXTURE`), in the
/// folder or beside its `tiles` folder.
pub const CANOPY_FIXTURE: &str = "canopy";
/// A tile's land cover (`landCover`: NMD's classes in Sweden, WorldCover's where NMD is clear or
/// not to be had), the source the land cover ground asks too (`layers::ground::landcover::
/// LAND_COVER`, `backend::landcover::NAME`): a tile's answer is its classes, `size²` bytes.
pub const LAND_COVER: &str = "landcover";

/// Trees from this zoom (`TREES_ZOOM`, G:1003).
pub const TREES_ZOOM: f64 = 14.0;
/// The radius planted round the centre: `min(sgl ? 2400 : 4800, max(900, 1.6·distance))` metres.
pub const SGL_RADIUS_M: f64 = 2400.0;
pub const RADIUS_M: f64 = 4800.0;
pub const MIN_RADIUS_M: f64 = 900.0;
pub const RADIUS_PER_DISTANCE: f64 = 1.6;
/// Shadows only with this much sun (`sunStrength() > 0.05`) and the sun this high (`sun[2] >
/// 0.02`).
pub const SHADOW_SUN_STRENGTH: f64 = 0.05;
pub const SHADOW_SUN_UP: f64 = 0.02;

/// What the scene builder hands this layer (`SceneInput::trees`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Input {
    /// Today's trees are always on the realistic map; the layer checks the basemap and the zoom.
    pub shown: bool,
}

impl Input {
    pub const SHOWN: Input = Input { shown: true };
}

/// Whether the trees are drawn for `view` on `basemap` (`realistic && tileUrl && flat && zoom >=
/// TREES_ZOOM`).
pub fn drawn_at(view: &ViewState, basemap: Basemap) -> bool {
    basemap == Basemap::Realistic && view.is_flat() && view.zoom >= TREES_ZOOM
}

/// Whether the trees are Streets GL's at `view` (`sgl = realistic && flat && zoom >=
/// SGL_FROM_ZOOM`).
pub fn sgl_at(view: &ViewState) -> bool {
    view.zoom >= SGL_FROM_ZOOM
}

/// The radius planted round the centre of `view` on a canvas `height` CSS pixels high.
pub fn radius(view: &ViewState, height: f64) -> f64 {
    let distance = camera_metres(view, height);
    let cap = if sgl_at(view) { SGL_RADIUS_M } else { RADIUS_M };
    cap.min(MIN_RADIUS_M.max(RADIUS_PER_DISTANCE * distance))
}

/// The tiles planted for `view` on a canvas `height` CSS pixels high, nearest first.
pub fn tiles_for(view: &ViewState, height: f64) -> Vec<TileAround> {
    let zoom = if sgl_at(view) { SGL_ZOOM } else { TREE_TILE_ZOOM };
    tiles_around(view.longitude, view.latitude, radius(view, height), zoom)
}

/// The shadows' colour with `look`'s sun, none without enough of it (G:2709-2712).
pub fn shadow_color(look: &Look) -> Option<[u8; 4]> {
    let strength = sun_strength(look);
    // (a NaN casts none)
    if strength.partial_cmp(&SHADOW_SUN_STRENGTH) != Some(std::cmp::Ordering::Greater) || look.sun_dir[2].partial_cmp(&SHADOW_SUN_UP) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    let a = js_round((0.55 * strength).min(0.6) * 255.0);
    Some([SHADOW_COLOR[0], SHADOW_COLOR[1], SHADOW_COLOR[2], a as u8])
}

// --- the tile's data ---------------------------------------------------------------------------------

/// A tile's features as loaders.gl's `MVTLoader` hands them to `plantTile` (`shape: "geojson"`,
/// `coordinates: "local"`, `layers`): each feature of the layers named, in the order named, its
/// tags with `layerName`; points and lines as they are, polygons' rings grouped by
/// `classifyRings`; each coordinate divided by the layer's extent (0 to 1 across, y down).
pub fn features(bytes: &[u8], layers: &[&str]) -> Vec<Feature> {
    let mut out = Vec::new();
    for layer in mvt::layers_named(bytes, layers) {
        let extent = layer.extent;
        let local = |(x, y): (f64, f64)| [x / extent, y / extent];
        for feature in layer.features() {
            let geometry = match feature.kind {
                GeomType::Point => {
                    let points: Vec<[f64; 2]> = feature.points().map(local).collect();
                    match points.as_slice() {
                        [p] => Geometry::Point(*p),
                        _ => Geometry::MultiPoint(points),
                    }
                }
                GeomType::LineString => {
                    let mut lines: Vec<Vec<[f64; 2]>> = feature.lines().into_iter().map(|l| l.into_iter().map(local).collect()).collect();
                    if lines.len() == 1 { Geometry::LineString(lines.remove(0)) } else { Geometry::MultiLineString(lines) }
                }
                GeomType::Polygon => {
                    let rings = feature.rings();
                    let mut polygons: Vec<Vec<Vec<[f64; 2]>>> =
                        classify_rings(&rings).into_iter().map(|p| p.into_iter().map(|r| r.iter().copied().map(local).collect()).collect()).collect();
                    if polygons.len() == 1 { Geometry::Polygon(polygons.remove(0)) } else { Geometry::MultiPolygon(polygons) }
                }
                GeomType::Unknown => continue,
            };
            let mut properties = feature.json_properties();
            properties.insert("layerName".into(), serde_json::Value::from(layer.name));
            out.push(Feature { geometry, properties });
        }
    }
    out
}

/// A land cover answer's grid: its classes, `size²` bytes; none for anything else.
pub fn land_grid(bytes: &[u8]) -> Option<LandGrid> {
    let size = (bytes.len() as f64).sqrt() as usize;
    (size > 0 && size * size == bytes.len()).then(|| LandGrid { size, classes: bytes.to_vec() })
}

/// A canopy answer's grid: its heights, `size²` bytes; none for anything else.
pub fn canopy_grid(bytes: &[u8]) -> Option<CanopyGrid> {
    let size = (bytes.len() as f64).sqrt() as usize;
    (size > 0 && size * size == bytes.len()).then(|| CanopyGrid { size, heights: bytes.to_vec() })
}

static MALMO: LazyLock<Option<Town>> = LazyLock::new(|| parse_town(include_bytes!("../../../../static/street-trees/malmo.bin")));
static UMEA: LazyLock<Option<Town>> = LazyLock::new(|| parse_town(include_bytes!("../../../../static/street-trees/umea.bin")));

/// The towns' registered trees in `tile` (`streetTrees`): each town's whose register the tile
/// meets, in the towns' order, read once from the embedded files.
pub fn registered(tile: TileIndex) -> Vec<RegisteredTree> {
    towns_for_tile(tile.x, tile.y, tile.z)
        .filter_map(|town| match town.file {
            "malmo" => MALMO.as_ref(),
            "umea" => UMEA.as_ref(),
            _ => None,
        })
        .flat_map(|town| trees_in_tile(town, tile.x, tile.y, tile.z))
        .collect()
}

/// One tree as the GPU draws it: where it stands (its tile's units, v south), its card's 3×3
/// matrix's columns (metres east, north, up: `instanceModelMatrix`), its colour with the code in
/// the alpha (`200 + layer·10 + 5 if retinted`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct TreeInstance {
    pub place: [f32; 2],
    pub c0: [f32; 3],
    pub c1: [f32; 3],
    pub c2: [f32; 3],
    pub colour: [u8; 4],
}

/// [`TreeInstance`]'s buffer: locations 3–7, one a tree.
pub const INSTANCE_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: size_of::<TreeInstance>() as u64,
    step_mode: wgpu::VertexStepMode::Instance,
    attributes: &wgpu::vertex_attr_array![3 => Float32x2, 4 => Float32x3, 5 => Float32x3, 6 => Float32x3, 7 => Unorm8x4],
};

/// The card's buffer (`card_arrays`): position, normal, UV, locations 0–2.
pub const CARD_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 32,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
};

/// A planted tile: how many conifers and broadleaves, and their instances (the conifers first,
/// as the two layers are drawn).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Planted {
    pub conifers: usize,
    pub broadleaves: usize,
    pub instances: Vec<TreeInstance>,
}

impl Planted {
    /// `trees` of `tile` as instances.
    pub fn of(tile: TileIndex, trees: &TileTrees) -> Self {
        let n = 2f64.powi(tile.z as i32);
        let mut instances = Vec::with_capacity(trees.conifers.len() + trees.broadleaves.len());
        for kind in [&trees.conifers, &trees.broadleaves] {
            for k in 0..kind.len() {
                let (lon, lat) = (kind.positions[3 * k], kind.positions[3 * k + 1]);
                let r = lat.to_radians();
                let u = (lon + 180.0) / 360.0 * n - f64::from(tile.x);
                let v = (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0 * n - f64::from(tile.y);
                let m = &kind.matrices[12 * k..12 * k + 12];
                let c = &kind.colors[4 * k..4 * k + 4];
                instances.push(TreeInstance {
                    place: [u as f32, v as f32],
                    c0: [m[0], m[1], m[2]],
                    c1: [m[3], m[4], m[5]],
                    c2: [m[6], m[7], m[8]],
                    colour: [c[0], c[1], c[2], c[3]],
                });
            }
        }
        Planted { conifers: trees.conifers.len(), broadleaves: trees.broadleaves.len(), instances }
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }
}

/// What else a tile's trees are planted from, as it came: its canopy's answer and its land
/// cover's (either may be missing).
#[derive(Debug, Clone, Default)]
pub struct Ground {
    pub canopy: Option<Vec<u8>>,
    pub land: Option<Vec<u8>>,
}

/// `plantTile` of `tile` (Streets GL's when `sgl`) from its bytes and ground, on the day of
/// `date_ms` (`plant`, on the worker pool).
pub fn plant(tile: TileIndex, sgl: bool, bytes: &[u8], ground: &Ground, date_ms: f64) -> (TileTrees, Planted) {
    let layers: &[&str] = if sgl { &SGL_TREE_LAYERS } else { &TREE_LAYERS };
    let features = features(bytes, layers);
    let land = ground.land.as_deref().and_then(land_grid);
    let canopy = ground.canopy.as_deref().and_then(canopy_grid);
    let registered = registered(tile);
    let trees = plant_tile(&features, tile, date_ms, &Surroundings { land: land.as_ref(), canopy: canopy.as_ref(), registered: &registered });
    let planted = Planted::of(tile, &trees);
    (trees, planted)
}

// --- the tile cache ----------------------------------------------------------------------------------

/// An ask of the fetcher: waiting, or its answer.
enum Ask {
    Waiting(Fetch),
    Done(FetchResult),
}

impl Ask {
    fn start(fetcher: &dyn TileFetcher, source: &str, tile: TileIndex) -> Self {
        Ask::Waiting(Fetch::start(fetcher, source.into(), tile))
    }

    fn poll(&mut self) -> Option<&FetchResult> {
        if let Ask::Waiting(fetch) = self
            && let Some(answer) = fetch.poll()
        {
            *self = Ask::Done(answer);
        }
        match self {
            Ask::Done(answer) => Some(answer),
            Ask::Waiting(_) => None,
        }
    }

    /// The answer's bytes (none for nothing there or a failure: `.catch(() => null)`).
    fn bytes(&self) -> Option<Vec<u8>> {
        match self {
            Ask::Done(Ok(Some(bytes))) => Some(bytes.clone()),
            _ => None,
        }
    }
}

/// A tile on its way: what it is planted from, being fetched, then its planting on the pool.
struct Loading {
    tile: TileIndex,
    sgl: bool,
    date_ms: f64,
    priority: f64,
    tile_bytes: Ask,
    canopy: Ask,
    land: Ask,
    plant: Option<Job<Planted>>,
}

impl Loading {
    fn start(fetcher: &dyn TileFetcher, tile: TileIndex, sgl: bool, date_ms: f64, priority: f64) -> Self {
        let source = if sgl { io::STREETS_GL } else { io::OPENFREEMAP };
        Loading {
            tile,
            sgl,
            date_ms,
            priority,
            tile_bytes: Ask::start(fetcher, source, tile),
            land: Ask::start(fetcher, LAND_COVER, tile),
            canopy: Ask::start(fetcher, CANOPY, tile),
            plant: None,
        }
    }

    /// Moves it on; its planting once it is done (`Err` for a tile that can't be read).
    fn poll(&mut self, pool: &Pool) -> Option<Result<Planted, String>> {
        match self.tile_bytes.poll() {
            // (`load` of a tile that isn't there throws: the tile fails, whatever its ground)
            Some(Err(e)) => return Some(Err(e.to_string())),
            Some(Ok(None)) => return Some(Err("no tile there".into())),
            _ => {}
        }
        if self.plant.is_none() {
            // (`Promise.all`: the planting waits for the land cover and the canopy too, each of
            // which may come back with nothing)
            let (Some(_), Some(_), Some(_)) = (self.tile_bytes.poll(), self.land.poll(), self.canopy.poll()) else { return None };
            let bytes = self.tile_bytes.bytes().unwrap_or_default();
            let ground = Ground { canopy: self.canopy.bytes(), land: self.land.bytes() };
            let (tile, sgl, date_ms) = (self.tile, self.sgl, self.date_ms);
            self.plant = Some(pool.spawn(self.priority, move |_| plant(tile, sgl, &bytes, &ground, date_ms).1));
        }
        let job = self.plant.as_mut()?;
        match job.poll() {
            JobPoll::Pending => None,
            JobPoll::Done(planted) => Some(Ok(planted)),
            JobPoll::Cancelled | JobPoll::Lost => Some(Err("the planting was lost".into())),
        }
    }
}

/// Where a cached tile is.
enum TileState {
    Loading(Box<Loading>),
    Planted(Arc<Planted>),
    Failed,
}

struct Entry {
    /// The frame it was last wanted in, and when it came into the cache (`Map` order).
    used: u64,
    seq: u64,
    state: TileState,
}

/// What a frame's tiles are: the planted ones, nearest first, and whether any is still to come.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Around {
    pub planted: Vec<(TileIndex, Arc<Planted>)>,
    pub waiting: bool,
}

/// The planted tiles (`tree-layer.ts`'s `cache` and `loading`): see the module docs. No GPU.
#[derive(Default)]
pub struct TreeTiles {
    cache: HashMap<TileIndex, Entry>,
    loading: usize,
    frame: u64,
    seq: u64,
}

impl TreeTiles {
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves every loading tile on (whether drawn or not, as today's promises go on); whether one
    /// was planted.
    pub fn poll(&mut self, pool: &Pool) -> bool {
        let mut done = false;
        for (tile, entry) in &mut self.cache {
            let TileState::Loading(loading) = &mut entry.state else { continue };
            let Some(result) = loading.poll(pool) else { continue };
            entry.state = match result {
                Ok(planted) => TileState::Planted(Arc::new(planted)),
                Err(e) => {
                    log::debug!("trees {tile:?}: {e}");
                    TileState::Failed
                }
            };
            self.loading -= 1;
            done = true;
        }
        done
    }

    /// A frame's `tiles` (nearest first; Streets GL's when `sgl`), planted on the day of
    /// `date_ms`: each not yet cached is started while fewer than [`MAX_LOADING`] load; every one
    /// is marked used; then the cache is cut to [`MAX_CACHED`], the tiles used longest ago going
    /// first.
    pub fn around(&mut self, fetcher: &dyn TileFetcher, pool: &Pool, tiles: &[TileAround], sgl: bool, date_ms: f64) -> Around {
        self.frame += 1;
        let mut around = Around::default();
        for t in tiles {
            if !self.cache.contains_key(&t.tile) {
                if self.loading >= MAX_LOADING {
                    around.waiting = true;
                    continue;
                }
                self.loading += 1;
                self.seq += 1;
                let mut loading = Box::new(Loading::start(fetcher, t.tile, sgl, date_ms, t.distance));
                // (a fetcher that answers at once: the tile moves on in this frame)
                let state = match loading.poll(pool) {
                    None => TileState::Loading(loading),
                    Some(Ok(planted)) => {
                        self.loading -= 1;
                        TileState::Planted(Arc::new(planted))
                    }
                    Some(Err(_)) => {
                        self.loading -= 1;
                        TileState::Failed
                    }
                };
                self.cache.insert(t.tile, Entry { used: 0, seq: self.seq, state });
            }
            let Some(entry) = self.cache.get_mut(&t.tile) else { continue };
            entry.used = self.frame;
            match &entry.state {
                TileState::Loading(_) => around.waiting = true,
                TileState::Planted(planted) => around.planted.push((t.tile, Arc::clone(planted))),
                TileState::Failed => {}
            }
        }
        // forget the tiles used longest ago
        if self.cache.len() > MAX_CACHED {
            let mut old: Vec<(u64, u64, TileIndex)> = self.cache.iter().map(|(&tile, e)| (e.used, e.seq, tile)).collect();
            // (`seq` is unique: the tile never decides)
            old.sort_unstable_by_key(|&(used, seq, _)| (used, seq));
            for &(_, _, tile) in &old[..self.cache.len() - MAX_CACHED] {
                if let Some(Entry { state: TileState::Loading(_), .. }) = self.cache.remove(&tile) {
                    self.loading -= 1;
                }
            }
        }
        around
    }

    /// A tile's planting, once planted.
    pub fn planted(&self, tile: TileIndex) -> Option<&Planted> {
        match &self.cache.get(&tile)?.state {
            TileState::Planted(planted) => Some(planted.as_ref()),
            _ => None,
        }
    }

    /// Whether a tile failed (asked again only once it is forgotten).
    pub fn failed(&self, tile: TileIndex) -> bool {
        matches!(self.cache.get(&tile).map(|e| &e.state), Some(TileState::Failed))
    }

    /// The tiles kept (loading, planted or failed).
    pub fn cached(&self) -> impl Iterator<Item = TileIndex> + '_ {
        self.cache.keys().copied()
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Tiles loading now.
    pub fn loading(&self) -> usize {
        self.loading
    }

    /// Forgets everything (a new fetcher: what the old one answered may have been nothing).
    pub fn clear(&mut self) {
        self.cache.clear();
        self.loading = 0;
    }
}

// --- the fixtures' canopy ----------------------------------------------------------------------------

/// A `--fixture` folder's canopy recording (`backend::canopy::FIXTURE`), in `root` or beside it.
pub fn fixture_canopy_dir(root: &Path) -> Option<PathBuf> {
    [root.join(CANOPY_FIXTURE), root.join("..").join(CANOPY_FIXTURE)].into_iter().find(|p| p.is_dir())
}

/// The bytes `[start, end)` of the canopy file `key` as recorded in `dir` (the range holding
/// `start`, cut at its end); none where it was not recorded.
fn recorded(dir: &Path, key: &str, start: u64, end: u64) -> Result<Option<Vec<u8>>, String> {
    let manifest = dir.join(format!("{key}.json"));
    let Ok(text) = std::fs::read(&manifest) else { return Ok(None) };
    let json: serde_json::Value = serde_json::from_slice(&text).map_err(|e| format!("{}: {e}", manifest.display()))?;
    for range in json["ranges"].as_array().into_iter().flatten() {
        let (Some(from), Some(to)) = (range[0].as_u64(), range[1].as_u64()) else { continue };
        if from <= start && start < to {
            let path = dir.join(key).join(format!("{from}-{to}.bin"));
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            return Ok(bytes.get((start - from) as usize..(end.min(to) - from) as usize).map(<[u8]>::to_vec));
        }
    }
    Ok(None)
}

/// One image of a COG, what reading its first samples takes.
#[derive(Debug, Clone, PartialEq)]
struct CogImage {
    new_subfile_type: Option<u32>,
    width: u32,
    height: u32,
    tile_width: u32,
    tile_height: u32,
    bits: u32,
    samples: u32,
    compression: u32,
    predictor: u32,
    offsets: Vec<u64>,
    byte_counts: Vec<u64>,
}

/// The images of a TIFF from its first bytes (all its IFDs there), as `backend::canopy`'s
/// `parse_tiff` reads them; `Err(Some(n))` where `n` bytes are needed, `Err(None)` for no TIFF.
fn cog_images(bytes: &[u8]) -> Result<Vec<CogImage>, Option<u64>> {
    let need = |end: u64| if end > bytes.len() as u64 { Err(Some(end)) } else { Ok(()) };
    need(8)?;
    let little = match &bytes[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Err(None),
    };
    let uint = |at: u64, size: u64| -> Result<u64, Option<u64>> {
        need(at + size)?;
        let b = &bytes[at as usize..(at + size) as usize];
        Ok(b.iter().enumerate().fold(0u64, |v, (i, &byte)| v | (u64::from(byte) << if little { 8 * i } else { 8 * (size as usize - 1 - i) })))
    };
    let big = match uint(2, 2)? {
        42 => false,
        43 => true,
        _ => return Err(None),
    };
    let (count_size, entry_size, offset_size) = if big { (8, 20, 8) } else { (2, 12, 4) };
    let mut next = if big { uint(8, 8)? } else { uint(4, 4)? };
    let mut images = Vec::new();
    while next != 0 && images.len() <= 64 {
        let count = uint(next, count_size)?;
        let entries = next + count_size;
        let ifd_end = entries + count * entry_size + offset_size;
        need(ifd_end)?;
        let mut tags: Vec<(u16, Vec<u64>)> = Vec::new();
        for i in 0..count {
            let at = entries + i * entry_size;
            let tag = uint(at, 2)? as u16;
            let kind = uint(at + 2, 2)?;
            let n = uint(at + 4, if big { 8 } else { 4 })?;
            let item = match kind {
                1 => 1,
                3 => 2,
                4 => 4,
                16 => 8,
                _ => continue,
            };
            if ![254, 256, 257, 258, 259, 277, 317, 322, 323, 324, 325].contains(&tag) {
                continue;
            }
            let value_at = at + if big { 12 } else { 8 };
            let start = if n * item <= offset_size { value_at } else { uint(value_at, offset_size)? };
            tags.push((tag, (0..n).map(|k| uint(start + k * item, item)).collect::<Result<_, _>>()?));
        }
        next = uint(ifd_end - offset_size, offset_size)?;
        let all = |tag: u16| tags.iter().find(|(t, _)| *t == tag).map(|(_, v)| v.clone()).unwrap_or_default();
        let one = |tag: u16, default: u32| all(tag).first().map_or(default, |&v| v as u32);
        images.push(CogImage {
            new_subfile_type: all(254).first().map(|&v| v as u32),
            width: one(256, 0),
            height: one(257, 0),
            tile_width: one(322, 0),
            tile_height: one(323, 0),
            bits: one(258, 1),
            samples: one(277, 1),
            compression: one(259, 1),
            predictor: one(317, 1),
            offsets: all(324),
            byte_counts: all(325),
        });
    }
    Ok(images)
}

/// An internal tile's stored bytes to its first samples (inflated, the differencing undone).
fn cog_tile(image: &CogImage, stored: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    if image.bits != 8 {
        return Err(format!("{}-bit samples", image.bits));
    }
    let (w, h, s) = (image.tile_width as usize, image.tile_height as usize, image.samples.max(1) as usize);
    let mut raw = match image.compression {
        1 => stored.to_vec(),
        8 | 32946 => {
            let mut out = Vec::with_capacity(w * h * s);
            flate2::read::ZlibDecoder::new(stored).read_to_end(&mut out).map_err(|e| e.to_string())?;
            out
        }
        c => return Err(format!("compression {c}")),
    };
    if raw.len() < w * h * s {
        return Err("a short tile".into());
    }
    if image.predictor == 2 {
        for row in raw[..w * h * s].chunks_exact_mut(w * s) {
            for i in s..row.len() {
                row[i] = row[i].wrapping_add(row[i - s]);
            }
        }
    }
    Ok(raw[..w * h * s].chunks_exact(s).map(|px| px[0]).collect())
}

/// The canopy answer of `tile` from a `--fixture` folder at `root` (`io::DirTiles`' reader of
/// [`CANOPY`]): as `backend::canopy` reads the network, from the recorded ranges. `None` where
/// the folder holds no recording of what the tile needs (a miss); a tile under zoom 10, or a file
/// without images, is nothing there.
pub fn fixture_canopy(root: &Path, tile: TileIndex) -> Option<FetchResult> {
    let dir = fixture_canopy_dir(root)?;
    if tile.z < canopy_rules::FILE_ZOOM {
        return Some(Ok(None));
    }
    let failed = |why: String| Some(Err(FetchError::Failed(format!("canopy {tile:?}: {why}"))));
    let place = canopy_rules::place_in_file(tile.x, tile.y, tile.z);
    let mut head = Vec::new();
    let images = loop {
        let have = head.len() as u64;
        if have > 4 << 20 {
            return failed("IFDs past 4 MiB".into());
        }
        match recorded(&dir, &place.key, have, have + 65_536) {
            Ok(Some(more)) if !more.is_empty() => head.extend(more),
            Ok(_) => return None,
            Err(e) => return failed(e),
        }
        match cog_images(&head) {
            Ok(images) => break images,
            Err(Some(_)) => continue,
            Err(None) => return failed("not a TIFF".into()),
        }
    };
    let mut readable: Vec<CogImage> = images.into_iter().filter(|im| !canopy_rules::is_mask(im.new_subfile_type)).collect();
    readable.sort_by_key(|im| std::cmp::Reverse(im.width));
    let widths: Vec<u32> = readable.iter().map(|im| im.width).collect();
    let Some(i) = canopy_rules::pick_image(&widths, place.span) else { return Some(Ok(None)) };
    let image = &readable[i];
    if image.tile_width == 0 || image.tile_height == 0 {
        return failed("an image without tiles".into());
    }
    let win = canopy_rules::window(image.width, &place);
    let (tw, th) = (image.tile_width as usize, image.tile_height as usize);
    let across = (image.width as usize).div_ceil(tw);
    let (x1, y1) = ((win.x0 + win.size).min(image.width as usize), (win.y0 + win.size).min(image.height as usize));
    let mut heights = vec![0u8; win.size * win.size];
    for row in win.y0 / th..y1.div_ceil(th) {
        for col in win.x0 / tw..x1.div_ceil(tw) {
            let k = row * across + col;
            let (Some(&offset), Some(&count)) = (image.offsets.get(k), image.byte_counts.get(k)) else { continue };
            if count == 0 {
                continue;
            }
            let stored = match recorded(&dir, &place.key, offset, offset + count) {
                Ok(Some(bytes)) if bytes.len() as u64 == count => bytes,
                Ok(_) => return None,
                Err(e) => return failed(e),
            };
            let pixels = match cog_tile(image, &stored) {
                Ok(pixels) => pixels,
                Err(e) => return failed(e),
            };
            let (tx0, ty0) = (col * tw, row * th);
            for y in ty0.max(win.y0)..(ty0 + th).min(y1) {
                let (from, to) = (tx0.max(win.x0), (tx0 + tw).min(x1));
                if from < to {
                    let src = (y - ty0) * tw + (from - tx0);
                    let dst = (y - win.y0) * win.size + (from - win.x0);
                    heights[dst..dst + (to - from)].copy_from_slice(&pixels[src..src + (to - from)]);
                }
            }
        }
    }
    Some(Ok(Some(heights)))
}

// --- the layer -----------------------------------------------------------------------------------------

/// `shaders/tree_shadow.wgsl`'s `TreeUniform`: the shadows' colour (0 to 1) and their sun's step.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct TreeUniform {
    pub shadow: [f32; 4],
    pub step: [f32; 4],
}

/// A planted tile's instances on the GPU.
struct TileBuffer {
    planted: Arc<Planted>,
    buffer: wgpu::Buffer,
}

/// Row 10 of doc 04 §1: `trees-z/x/y-shadows`, `-conifers`, `-broadleaves`.
pub struct Trees {
    tiles: TreeTiles,
    trees: wgpu::RenderPipeline,
    shadows: wgpu::RenderPipeline,
    light: LightUniform,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    volume_sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    /// Group 3, made from the textures as they were (`bound`: ready or not).
    group: Option<wgpu::BindGroup>,
    bound: Option<bool>,
    card: wgpu::Buffer,
    buffers: HashMap<TileIndex, TileBuffer>,
    /// The fetcher's generation the cache holds the answers of.
    generation: u64,
    shown: bool,
    waiting: bool,
    textures_loading: bool,
    /// The sun the shadows are cast with, and its half-degree key, held until the key changes.
    held_sun: Option<((i64, i64), [f64; 3])>,
    /// This frame's shadows' colour and step, none without them.
    shadow: Option<([u8; 4], [f64; 2])>,
    /// This frame's tiles in draw order, each with its frame.
    drawn: Vec<(FrameSlot, TileIndex)>,
}

impl Trees {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let texture = |binding, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension, multisampled: false },
            count: None,
        };
        let sampler = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("trees"),
            entries: &[
                texture(0, wgpu::TextureViewDimension::D2Array),
                texture(1, wgpu::TextureViewDimension::D2Array),
                texture(2, wgpu::TextureViewDimension::D2Array),
                texture(3, wgpu::TextureViewDimension::D2),
                sampler(4),
                sampler(5),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let light = LightUniform::new(device);
        let pipeline = |label: &str, source: &str, vertex_entry: &str, fragment_entry: &str, state: DrawState| {
            let module = shaders::module(device, label, source);
            build(
                init,
                &Desc {
                    label,
                    module: &module,
                    vertex_entry,
                    fragment_entry,
                    buffers: &[Some(CARD_LAYOUT), Some(INSTANCE_LAYOUT)],
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    state,
                    groups: &[light.layout(), &layout],
                },
            )
        };
        // the trees: deck's own parameters (depth tested and written, no culling)
        let trees = pipeline("trees", SHADER, "vs_tree", "fs_tree", DrawState::NO_CULL);
        let shadows = pipeline("tree shadows", SHADOW_SHADER, "vs_tree_shadow", "fs_tree_shadow", DrawState::SHADOW);
        let cards = card_arrays();
        let vertices: Vec<f32> = (0..cards.positions.len() / 3)
            .flat_map(|i| {
                let p = &cards.positions[3 * i..3 * i + 3];
                let n = &cards.normals[3 * i..3 * i + 3];
                let t = &cards.tex_coords[2 * i..2 * i + 2];
                [p[0], p[1], p[2], n[0], n[1], n[2], t[0], t[1]]
            })
            .collect();
        let card = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("tree card"), contents: bytemuck::cast_slice(&vertices), usage: wgpu::BufferUsages::VERTEX });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("trees"),
            size: size_of::<TreeUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Trees {
            tiles: TreeTiles::new(),
            trees,
            shadows,
            light,
            layout,
            sampler: Sampling::Repeat.sampler(device),
            volume_sampler: Sampling::Clamp.sampler(device),
            uniform,
            group: None,
            bound: None,
            card,
            buffers: HashMap::new(),
            generation: init.tiles.generation(),
            shown: false,
            waiting: false,
            textures_loading: false,
            held_sun: None,
            shadow: None,
            drawn: Vec::new(),
        }
    }

    /// The planted tiles (tests).
    pub fn tiles(&self) -> &TreeTiles {
        &self.tiles
    }

    /// This frame's tiles in draw order, nearest first.
    pub fn drawn(&self) -> impl Iterator<Item = TileIndex> + '_ {
        self.drawn.iter().map(|&(_, tile)| tile)
    }

    /// This frame's shadows' colour and the step of their sun (none without them).
    pub fn shadow(&self) -> Option<([u8; 4], [f64; 2])> {
        self.shadow
    }

    /// The shadows' sun (towards it: east, north, up), held until it has moved half a degree.
    fn shadow_step(&mut self, sun: [f64; 3]) -> Option<[f64; 2]> {
        let key = shadow_key(sun)?;
        if self.held_sun.is_none_or(|(held, _)| held != key) {
            self.held_sun = Some((key, sun));
        }
        shadow_step(self.held_sun?.1)
    }

    fn bind_textures(&mut self, cx: &PrepareCx<'_>) {
        let ready = cx.textures.ready;
        if self.bound == Some(ready) {
            return;
        }
        let t = &cx.textures.trees;
        let view = |binding, view| wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) };
        self.group = Some(cx.gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("trees"),
            layout: &self.layout,
            entries: &[
                view(0, &t.color.view),
                view(1, &t.normal.view),
                view(2, &t.mask.view),
                view(3, &t.volume.view),
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&self.volume_sampler) },
                wgpu::BindGroupEntry { binding: 6, resource: self.uniform.as_entire_binding() },
            ],
        }));
        self.bound = Some(ready);
    }
}

impl Layer for Trees {
    fn name(&self) -> &'static str {
        "trees"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.drawn.clear();
        self.waiting = false;
        let fetcher = cx.tiles.fetcher();
        // a new fetcher: what the old one answered may have been nothing
        if cx.tiles.generation() != self.generation {
            self.generation = cx.tiles.generation();
            self.tiles.clear();
            self.buffers.clear();
        }
        self.tiles.poll(&cx.tiles.pool);
        let view = cx.view.view;
        self.shown = cx.scene.trees.shown && !cx.view.is_globe() && drawn_at(&view, cx.scene.basemap);
        if !self.shown {
            // (deck finalizes the layers that leave the list: their buffers go)
            self.buffers.clear();
            self.textures_loading = false;
            self.shadow = None;
            return;
        }
        load_textures(cx);
        self.textures_loading = cx.textures.pending();
        self.bind_textures(cx);
        let look = self.light.prepare(cx);
        self.shadow = shadow_color(&look).and_then(|colour| Some((colour, self.shadow_step(look.sun_dir)?)));
        if let Some((colour, step)) = self.shadow {
            let uniform = TreeUniform { shadow: colour.map(|c| f32::from(c) / 255.0), step: [step[0] as f32, step[1] as f32, 0.0, 0.0] };
            cx.gpu.queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        }
        // the tiles round the view's centre, nearest first, planted on the feed clock's day
        let tiles = tiles_for(&view, cx.view.size[1]);
        let around = self.tiles.around(&*fetcher, &cx.tiles.pool, &tiles, sgl_at(&view), cx.scene.feed_now_ms);
        self.waiting = around.waiting;
        let mut buffers = HashMap::with_capacity(around.planted.len());
        for (tile, planted) in around.planted {
            if planted.is_empty() {
                continue;
            }
            let buffer = match self.buffers.remove(&tile) {
                Some(held) if Arc::ptr_eq(&held.planted, &planted) => held,
                _ => {
                    let bytes = planted.instances.len() * size_of::<TreeInstance>();
                    if !cx.budget.take(bytes) {
                        self.waiting = true;
                        continue;
                    }
                    let buffer = cx.gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("trees"),
                        contents: bytemuck::cast_slice(&planted.instances),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    TileBuffer { planted, buffer }
                }
            };
            self.drawn.push((cx.tile_frame(tile, 0.0), tile));
            buffers.insert(tile, buffer);
        }
        // (the tiles not drawn this frame let their buffers go)
        self.buffers = buffers;
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if !self.shown || cx.pass != Pass::Map {
            return;
        }
        let Some(group) = &self.group else { return };
        for &(slot, tile) in &self.drawn {
            let Some(held) = self.buffers.get(&tile) else { continue };
            let count = held.planted.instances.len() as u32;
            let mut draw = |pipeline: &wgpu::RenderPipeline| {
                pass.set_pipeline(pipeline);
                self.light.bind(pass);
                pass.set_bind_group(TREE_GROUP, group, &[]);
                cx.bind_frame(pass, slot);
                pass.set_vertex_buffer(0, self.card.slice(..));
                pass.set_vertex_buffer(1, held.buffer.slice(..));
                pass.draw(0..12, 0..count);
            };
            // in sunshine the tile's shadows first, then its trees
            if self.shadow.is_some() {
                draw(&self.shadows);
            }
            draw(&self.trees);
        }
    }

    fn drape(&self) -> super::Drape {
        super::Drape::Offset
    }

    fn pending(&self) -> bool {
        self.shown && (self.waiting || self.textures_loading)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::mpsc::{Receiver, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use rustradar_core::weather::PRESETS;
    use rustradar_scene::landcover::{self, lc};

    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{DirTiles, FetchRequest, FnFetcher};
    use crate::layers::{buildings, earth, ground, sky};
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 10:00 UTC: noon in Stockholm (the reference pictures' clock).
    const MIDSUMMER: f64 = 1_782_036_000_000.0;
    /// The same day at 16:00 UTC: the sun in the west, 27° up, the trees' shadows long to the east.
    const EVENING: f64 = 1_782_057_600_000.0;
    /// Humlegården (the fixtures' park, `CLOSE_UP_VIEWS`) at zoom 16, and its tile.
    const HUMLEGARDEN: ViewState = ViewState { latitude: 59.3398, longitude: 18.0727, zoom: 16.0, pitch: 45.0, bearing: 20.0, position: None };
    const SIZE: [f64; 2] = [1100.0, 720.0];
    const PARK: TileIndex = TileIndex { x: 36058, y: 19269, z: 16 };

    fn shots_dir() -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    /// A land cover picture's classes (`landcover.ts`'s `pixels`: as stored, drawn to 256 × 256
    /// without smoothing; NMD's colours or WorldCover's codes).
    fn picture_classes(png: &[u8], nmd: bool) -> Option<Vec<landcover::LandClass>> {
        let image = image::load_from_memory(png).ok()?.to_rgba8();
        assert_eq!(image.dimensions(), (256, 256), "the fixtures' pictures are 256 px");
        let rgba = image.into_raw();
        Some(if nmd { landcover::nmd_classes(&rgba) } else { landcover::world_cover_classes(&rgba) })
    }

    /// `landCover(tile)` of the fixtures' pictures (NMD's in Sweden, WorldCover's where NMD is
    /// clear or not to be had), as the land cover source answers it.
    fn land_of(tiles: &DirTiles, tile: TileIndex) -> Option<Vec<u8>> {
        let read = |source: &str| tiles.read(source, tile).expect("read");
        let nmd = if landcover::in_sweden(tile.x, tile.y, tile.z) { read("nmd").and_then(|png| picture_classes(&png, true)) } else { None };
        let world = if landcover::wants_world_cover(nmd.as_deref()) { read("worldcover").and_then(|png| picture_classes(&png, false)) } else { None };
        landcover::combine(nmd, world).map(|grid| grid.classes)
    }

    /// The fixtures' ground of `tile`, read as the layer reads it.
    fn ground_of(tiles: &DirTiles, tile: TileIndex) -> Ground {
        Ground { canopy: tiles.read(CANOPY, tile).expect("read"), land: land_of(tiles, tile) }
    }

    /// The fixture tiles, the land cover answered from their pictures as `backend::landcover`
    /// answers it (whatever `DirTiles` itself does with the land cover's source).
    struct Fixtures(Arc<DirTiles>);

    impl TileFetcher for Fixtures {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            if &*request.source == LAND_COVER {
                return io::answered(Ok(land_of(&self.0, request.tile)));
            }
            self.0.fetch(request)
        }
    }

    /// `TreeTiles` over `fetcher` for `tiles` (Streets GL's when `sgl`) until none loads (10 s).
    fn settle(fetcher: &dyn TileFetcher, pool: &Pool, set: &mut TreeTiles, tiles: &[TileAround], sgl: bool, date: f64) -> Around {
        let started = Instant::now();
        loop {
            set.poll(pool);
            let around = set.around(fetcher, pool, tiles, sgl, date);
            if !around.waiting {
                return around;
            }
            assert!(started.elapsed() < Duration::from_secs(30), "still loading");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn at(tile: TileIndex) -> TileAround {
        TileAround { tile, distance: 0.0 }
    }

    #[test]
    fn the_shaders_validate_and_the_buffers_are_theirs() {
        shaders::tests::validate("tree.wgsl", &shaders::expand(SHADER).expect("libraries known"));
        shaders::tests::validate("tree_shadow.wgsl", &shaders::expand(SHADOW_SHADER).expect("libraries known"));
        assert_eq!(INSTANCE_LAYOUT.array_stride, 48);
        let offsets: Vec<u64> = INSTANCE_LAYOUT.attributes.iter().map(|a| a.offset).collect();
        let want = [
            std::mem::offset_of!(TreeInstance, place),
            std::mem::offset_of!(TreeInstance, c0),
            std::mem::offset_of!(TreeInstance, c1),
            std::mem::offset_of!(TreeInstance, c2),
            std::mem::offset_of!(TreeInstance, colour),
        ];
        assert_eq!(offsets, want.map(|o| o as u64));
        let cards = card_arrays();
        assert_eq!((CARD_LAYOUT.array_stride, cards.positions.len()), (32, 36));
        assert_eq!(size_of::<TreeUniform>(), 32);
    }

    /// The tiles, the radius and the shadows' rule, as G:2695-2716 have them.
    #[test]
    fn where_and_when_as_today() {
        let v5 = ViewState { latitude: 59.3293, longitude: 18.0686, zoom: 15.5, pitch: 60.0, bearing: 20.0, position: None };
        // V5 at 1440 × 900: the camera 1.16 km out, Streets GL's tiles out to 1.6 times that
        let d = camera_metres(&v5, 900.0);
        assert!((d - 1163.1).abs() < 0.1, "{d}");
        assert!(sgl_at(&v5) && (radius(&v5, 900.0) - 1.6 * d).abs() < 1e-9);
        assert!(tiles_for(&v5, 900.0).iter().all(|t| t.tile.z == SGL_ZOOM && t.distance <= 1.6 * d));
        // at most 2.4 km of them, 4.8 km of OpenFreeMap's
        assert_eq!(radius(&v5, 3000.0), SGL_RADIUS_M);
        assert_eq!(radius(&ViewState { zoom: 14.5, ..v5 }, 3000.0), RADIUS_M);
        let z15 = ViewState { zoom: 15.0, ..v5 };
        let d = camera_metres(&z15, 900.0);
        assert!(!sgl_at(&z15) && (radius(&z15, 900.0) - 1.6 * d).abs() < 1e-9);
        assert!(tiles_for(&z15, 900.0).iter().all(|t| t.tile.z == TREE_TILE_ZOOM));
        // a small canvas: at least 900 m
        assert_eq!(radius(&ViewState { zoom: 14.0, ..v5 }, 100.0), MIN_RADIUS_M);
        // nearest first, the centre's own tile first of all
        let tiles = tiles_for(&v5, 900.0);
        assert_eq!(tiles[0].distance, 0.0);
        assert!(tiles.windows(2).all(|w| w[0].distance <= w[1].distance));
        assert!(drawn_at(&v5, Basemap::Realistic) && !drawn_at(&v5, Basemap::Dark) && !drawn_at(&ViewState { zoom: 13.9, ..v5 }, Basemap::Realistic));
        // shadows: in sunshine only, as dark as the sun is strong, at most 0.6
        let mut look = crate::look::frame_look(&sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true }, &v5, [1440.0, 900.0], MIDSUMMER);
        let strength = sun_strength(&look);
        let alpha = js_round((0.55 * strength).min(0.6) * 255.0) as u8;
        assert_eq!(shadow_color(&look), Some([6, 12, 28, alpha]));
        look.sun_color = [0.15; 3];
        assert_eq!(shadow_color(&look), None, "a sun under 0.05 casts none");
        look.sun_color = [0.17; 3];
        assert_eq!(shadow_color(&look), Some([6, 12, 28, 7]), "just over 0.05");
        look.sun_color = [3.2; 3];
        assert_eq!(shadow_color(&look), Some([6, 12, 28, 140]), "the whole sun: 0.55");
        look.sun_color = [4.0; 3];
        assert_eq!(shadow_color(&look), Some([6, 12, 28, 153]), "0.6 at most");
        look.sun_dir[2] = 0.02;
        assert_eq!(shadow_color(&look), None, "a sun 0.02 up casts none");
    }

    /// Humlegården's tile from the fixtures: its features as loaders.gl hands them (local
    /// coordinates, `layerName`), its land cover (NMD's, the park's broadleaves and conifers)
    /// and its canopy (512 px, the park's trees 10 to 30 m); a Malmö tile has registered trees.
    #[test]
    fn the_fixtures_ground_reads_as_today() {
        let tiles = DirTiles::fixtures();
        let bytes = tiles.read(io::STREETS_GL, PARK).expect("read").expect("the park's tile");
        let features = features(&bytes, &SGL_TREE_LAYERS);
        assert!(!features.is_empty());
        let names: Vec<&str> = features.iter().map(|f| f.properties["layerName"].as_str().unwrap()).collect();
        let mut order: Vec<&str> = names.clone();
        order.dedup();
        assert!(order.iter().all(|n| SGL_TREE_LAYERS.contains(n)), "{order:?}");
        // in the order named
        let rank = |n: &str| SGL_TREE_LAYERS.iter().position(|l| *l == n).unwrap();
        assert!(order.windows(2).all(|w| rank(w[0]) < rank(w[1])), "{order:?}");
        let mut inside = 0;
        for f in &features {
            if let Geometry::Point([x, y]) = f.geometry {
                inside += usize::from((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y));
            }
        }
        assert!(inside > 10, "{inside} mapped points in the park's tile");
        let ground = ground_of(&tiles, PARK);
        let land = land_grid(ground.land.as_deref().expect("NMD's land cover")).expect("a grid");
        assert_eq!((land.size, land.classes.len()), (256, 256 * 256));
        let wooded = land.classes.iter().filter(|&&c| landcover::wooded(c)).count();
        assert!(wooded > 1000, "{wooded} px of woods");
        let canopy = canopy_grid(ground.canopy.as_deref().expect("the recorded canopy")).expect("a grid");
        assert_eq!(canopy.size, 512);
        let tall = canopy.heights.iter().filter(|&&h| h >= 10).count();
        assert!(tall > 10_000, "{tall} px of trees 10 m tall");
        assert!(canopy.heights.iter().all(|&h| h < 60));
        // nothing for a tile the recording lacks, nor under zoom 10
        assert_eq!(fixture_canopy(tiles.root(), TileIndex::new(0, 0, 9)), Some(Ok(None)));
        assert_eq!(fixture_canopy(tiles.root(), TileIndex::new(36000, 19000, 16)), None);
        // Malmö's register: Stortorget's tile
        assert!(registered(PARK).is_empty());
        let malmo = TileIndex::new(35135, 20534, 16);
        let trees = registered(malmo);
        assert!(trees.len() > 50, "{} registered trees", trees.len());
        assert!(trees.iter().all(|t| (0.0..1.0).contains(&t.x) && (0.0..1.0).contains(&t.y) && t.kind <= 4));
        let _ = lc::NONE;
    }

    /// What `tree-layer.ts`'s `plantTile` planted under bun from four of the fixtures' Streets GL
    /// tiles round Humlegården (loaders.gl's `MVTLoader` with `coordinates: "local"`, the canopy
    /// through `canopy.ts` and geotiff.js over the recorded ranges, the land cover as this layer
    /// reads it, 2026-06-21 10:00 UTC): the features, and per kind the trees, their positions',
    /// matrices' and colours' sums.
    /// A kind of tree's count and its positions', matrices' and colours' sums.
    type Digest = (usize, f64, f64, u64);

    const TYPESCRIPT: [((u32, u32), usize, Digest, Digest); 4] = [
        ((36058, 19269), 198, (100, 7741.518317178579, 1736.9532703254372, 87273), (565, 43739.130589719396, 13095.608081802726, 499567)),
        ((36057, 19268), 229, (79, 6115.612480863468, 952.3267860412598, 69132), (102, 7896.064120856241, 950.8492837995291, 89530)),
        ((36059, 19270), 146, (57, 4412.885019657731, 332.28512689471245, 49746), (60, 4645.10405250517, 390.9471175391227, 53610)),
        ((36058, 19268), 151, (119, 9212.722915039498, 1545.8384345173836, 103146), (194, 15019.226324238756, 2955.6219134926796, 170770)),
    ];

    /// The layer's own tile cache, over the fixtures through a fetcher as the map has it, plants
    /// each of the four tiles with as many trees as `plant_tile` from the same features and ground,
    /// and as today's TypeScript, to the last digit of every sum; the canopy the fixtures' reader
    /// gives is geotiff.js's (the bun run checked it byte for byte).
    #[test]
    fn fixture_tiles_plant_as_plant_tile_and_today_s_typescript() {
        let tiles = Arc::new(DirTiles::fixtures());
        let pool = Pool::new(2);
        let mut set = TreeTiles::new();
        let wanted: Vec<TileAround> = TYPESCRIPT.iter().map(|&((x, y), ..)| at(TileIndex::new(x, y, 16))).collect();
        let around = settle(&Fixtures(tiles.clone()), &pool, &mut set, &wanted, true, MIDSUMMER);
        assert_eq!(around.planted.len(), 4);
        let sum = |v: &[f64]| v.iter().sum::<f64>();
        let msum = |v: &[f32]| v.iter().map(|&x| f64::from(x)).sum::<f64>();
        let csum = |v: &[u8]| v.iter().map(|&x| u64::from(x)).sum::<u64>();
        for ((x, y), features_count, conifers, broadleaves) in TYPESCRIPT {
            let tile = TileIndex::new(x, y, 16);
            let bytes = tiles.read(io::STREETS_GL, tile).unwrap().unwrap();
            assert_eq!(features(&bytes, &SGL_TREE_LAYERS).len(), features_count, "{x}/{y}");
            let (trees, planted) = plant(tile, true, &bytes, &ground_of(&tiles, tile), MIDSUMMER);
            let digest = |k: &rustradar_scene::trees::Instances| (k.len(), sum(&k.positions), msum(&k.matrices), csum(&k.colors));
            assert_eq!(digest(&trees.conifers), conifers, "{x}/{y} conifers");
            assert_eq!(digest(&trees.broadleaves), broadleaves, "{x}/{y} broadleaves");
            let layer = set.planted(tile).expect("planted");
            assert_eq!((layer.conifers, layer.broadleaves, layer.len()), (conifers.0, broadleaves.0, conifers.0 + broadleaves.0), "{x}/{y}");
            assert_eq!(*layer, planted);
        }
        assert!(tiles.misses().is_empty(), "the fixtures lack {:?}", tiles.misses());
    }

    /// A fetcher that holds every ask to answer later, noting them.
    #[derive(Default)]
    struct Held {
        asks: Mutex<Vec<(String, TileIndex, Sender<FetchResult>)>>,
    }

    impl TileFetcher for Held {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            let (tx, rx) = std::sync::mpsc::channel();
            self.asks.lock().unwrap().push((request.source.to_string(), request.tile, tx));
            rx
        }
    }

    impl Held {
        /// The asks not yet answered, by source and tile.
        fn waiting(&self) -> Vec<(String, TileIndex)> {
            self.asks.lock().unwrap().iter().map(|(s, t, _)| (s.clone(), *t)).collect()
        }

        /// Answers every ask with what `answer` gives for its source and tile.
        fn answer(&self, answer: impl Fn(&str, TileIndex) -> FetchResult) {
            for (source, tile, tx) in self.asks.lock().unwrap().drain(..) {
                let _ = tx.send(answer(&source, tile));
            }
        }
    }

    /// `treeLayers`' tile runtime: at most four tiles load at once (the rest wait for a later
    /// frame), each asking for its tile (Streets GL's at 16, OpenFreeMap's at 14), its land cover
    /// and its canopy, planted once all three have answered (the land cover and the canopy may
    /// have nothing); a tile that can't be had fails and is not asked again while it is kept.
    #[test]
    fn four_tiles_load_at_once_each_asking_for_its_ground() {
        let pool = Pool::new(2);
        let held = Held::default();
        let mut set = TreeTiles::new();
        let tiles: Vec<TileAround> = (0..10).map(|i| at(TileIndex::new(36050 + i, 19270, 16))).collect();
        let around = set.around(&held, &pool, &tiles, true, MIDSUMMER);
        assert!(around.waiting && around.planted.is_empty());
        assert_eq!((set.len(), set.loading()), (4, 4));
        let asked = held.waiting();
        for t in &tiles[..4] {
            for source in [io::STREETS_GL, CANOPY, LAND_COVER] {
                assert!(asked.contains(&(source.to_owned(), t.tile)), "{source} {:?}", t.tile);
            }
        }
        assert_eq!(asked.len(), 12);
        // the first tile's own is not there: it fails; the others' ground is not there either
        let first = tiles[0].tile;
        held.answer(|source, tile| Ok((source == io::STREETS_GL && tile != first).then(Vec::new)));
        let around = settle(&held, &pool, &mut set, &tiles[..4], true, MIDSUMMER);
        assert!(set.failed(first));
        assert_eq!(around.planted.len(), 3);
        assert!(around.planted.iter().all(|(_, p)| p.is_empty()), "an empty tile plants nothing");
        assert_eq!(set.loading(), 0);
        // the next four start; the failed one is not asked again
        set.around(&held, &pool, &tiles, true, MIDSUMMER);
        let asked = held.waiting();
        assert!(asked.iter().all(|(_, t)| tiles[4..8].iter().any(|a| a.tile == *t)), "{asked:?}");
        assert_eq!(set.loading(), 4);
        // OpenFreeMap's zoom-14 tiles below Streets GL's zoom
        let mut set = TreeTiles::new();
        let held = Held::default();
        let paris = [at(TileIndex::new(8299, 5636, 14))];
        set.around(&held, &pool, &paris, false, MIDSUMMER);
        let mut asked: Vec<String> = held.waiting().into_iter().map(|(source, tile)| { assert_eq!(tile, paris[0].tile); source }).collect();
        asked.sort();
        assert_eq!(asked, [CANOPY, LAND_COVER, io::OPENFREEMAP]);
    }

    /// Past 256 tiles the ones used longest ago are forgotten (a forgotten failure is asked again).
    #[test]
    fn two_hundred_and_fifty_six_tiles_are_kept() {
        let pool = Pool::new(2);
        let empty = FnFetcher(|source: &str, _| Ok((source == io::STREETS_GL).then(Vec::new)));
        let mut set = TreeTiles::new();
        let tiles: Vec<TileAround> = (0..300).map(|i| at(TileIndex::new(36000 + i, 19270, 16))).collect();
        for batch in tiles.chunks(4) {
            settle(&empty, &pool, &mut set, batch, true, MIDSUMMER);
        }
        assert_eq!(set.len(), MAX_CACHED);
        let kept: Vec<TileIndex> = set.cached().collect();
        assert!(tiles[..44].iter().all(|t| !kept.contains(&t.tile)));
        assert!(tiles[44..].iter().all(|t| kept.contains(&t.tile)));
        // a tile asked again is kept over the rest of its frame's
        settle(&empty, &pool, &mut set, &tiles[44..45], true, MIDSUMMER);
        settle(&empty, &pool, &mut set, &tiles[..1], true, MIDSUMMER);
        let kept: Vec<TileIndex> = set.cached().collect();
        assert!(kept.contains(&tiles[44].tile) && kept.contains(&tiles[0].tile) && !kept.contains(&tiles[45].tile));
    }

    /// The realistic map at `view` on a `size` canvas at `now`, a fine day: the sky, the earth, the
    /// ground, the buildings, and the trees when `trees`.
    fn realistic(view: ViewState, size: [f64; 2], now: f64, trees: bool) -> SceneInput {
        let mut scene = SceneInput::new(view, size, 1.0, Basemap::Realistic);
        scene.local_now_ms = now;
        scene.feed_now_ms = now;
        scene.earth = earth::Input::SHOWN;
        scene.sky = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
        scene.ground = ground::Input::SHOWN;
        scene.buildings = buildings::Input { shown: true };
        scene.trees = Input { shown: trees };
        scene
    }

    /// The built-in trees layer is shown by the scene; this one, added after it, draws with the
    /// scene's trees shown whatever the scene says, so a test can read its tiles.
    struct Watch(Rc<RefCell<Trees>>, Rc<Cell<bool>>);

    impl Layer for Watch {
        fn name(&self) -> &'static str {
            "trees watch"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            let mut scene = cx.scene.clone();
            scene.trees = Input { shown: self.1.get() };
            let mut inner = PrepareCx {
                gpu: cx.gpu,
                scene: &scene,
                view: cx.view,
                frames: &mut *cx.frames,
                local_now_ms: cx.local_now_ms,
                tiles: cx.tiles,
                budget: &mut *cx.budget,
                textures: &mut *cx.textures,
            };
            self.0.borrow_mut().prepare(&mut inner);
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            self.0.borrow().draw(pass, cx);
        }

        fn pending(&self) -> bool {
            self.0.borrow().pending()
        }
    }

    /// A renderer over the fixtures with a watched trees layer, drawing it when `trees` (the
    /// scene's own trees stay hidden), and its shot.
    struct Park {
        renderer: MapRenderer,
        watch: Rc<RefCell<Trees>>,
        shown: Rc<Cell<bool>>,
        fixtures: Arc<DirTiles>,
    }

    impl Park {
        fn new(gpu: &crate::gpu::Gpu) -> Self {
            let fixtures = Arc::new(DirTiles::fixtures());
            let mut renderer = MapRenderer::new(gpu, FORMAT);
            renderer.set_tiles(Arc::new(Fixtures(fixtures.clone())), None);
            let watch = Rc::new(RefCell::new(Trees::new(&renderer.init(gpu))));
            let shown = Rc::new(Cell::new(true));
            renderer.add_layer(Box::new(Watch(Rc::clone(&watch), Rc::clone(&shown))));
            Park { renderer, watch, shown, fixtures }
        }

        /// Humlegården at `now` in `weather`, with buildings when `buildings` and trees when
        /// `trees`.
        fn shot(&mut self, gpu: &crate::gpu::Gpu, now: f64, weather: rustradar_core::weather::Conditions, buildings: bool, trees: bool) -> Image {
            let mut scene = realistic(HUMLEGARDEN, SIZE, now, false);
            scene.sky.weather = Some(weather);
            scene.buildings = buildings::Input { shown: buildings };
            self.shown.set(trees);
            let image = render_with(gpu, &mut self.renderer, &scene, 1.0).expect("rendered");
            assert!(!self.renderer.pending(), "{:?}", self.renderer.pending_layers());
            image
        }
    }

    /// Where a planted tree's two cards are on the screen: each card's corners (CSS px), its
    /// ground at the card's foot first, metres through the instance's matrix at the view centre's
    /// scale as the shader has them; laid on the ground along `step` for its shadow.
    fn card_quads(view: &ViewFrame, tile: TileIndex, tree: &TreeInstance, step: Option<[f64; 2]>) -> [[[f64; 2]; 4]; 2] {
        let n = 2f64.powi(tile.z as i32);
        let units_per_metre = n / (40_075_016.686 * view.view.latitude.to_radians().cos());
        let column = |c: [f32; 3]| {
            let c = c.map(f64::from);
            match step {
                Some([e, nn]) => [c[0] + c[2] * e, c[1] + c[2] * nn, 0.0],
                None => c,
            }
        };
        let (c0, c1, c2) = (column(tree.c0), column(tree.c1), column(tree.c2));
        let lift = if step.is_some() { 0.05 } else { 0.0 };
        let corner = |card: usize, u: f64, z: f64| {
            let p = if card == 0 { [u - 0.5, 0.0, z] } else { [0.0, u - 0.5, z] };
            let m = |i: usize| c0[i] * p[0] + c1[i] * p[1] + c2[i] * p[2];
            let (tu, tv) = (f64::from(tile.x) + f64::from(tree.place[0]) + m(0) * units_per_metre, f64::from(tile.y) + f64::from(tree.place[1]) - m(1) * units_per_metre);
            let [lon, lat] = rustradar_scene::tiles::osm_tile_to_lng_lat(tu, tv, tile.z);
            let [x, y, _] = view.viewport.project([lon, lat, m(2) + lift]);
            [x, y]
        };
        [0, 1].map(|card| [corner(card, 0.0, 0.0), corner(card, 1.0, 0.0), corner(card, 1.0, 1.0), corner(card, 0.0, 1.0)])
    }

    /// Whether `p` is inside the convex quadrilateral `q`.
    fn inside(q: &[[f64; 2]; 4], p: [f64; 2]) -> bool {
        let side = |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        let s: Vec<f64> = (0..4).map(|i| side(q[i], q[(i + 1) % 4])).collect();
        s.iter().all(|&v| v >= 0.0) || s.iter().all(|&v| v <= 0.0)
    }

    /// How many quads cover each pixel of a `w` × `h` screen (their pixel centres strictly
    /// inside), and which tree's they are where only one does.
    fn coverage(quads: &[(usize, [[f64; 2]; 4])], w: u32, h: u32) -> (Vec<u16>, Vec<usize>) {
        let (mut count, mut owner) = (vec![0u16; (w * h) as usize], vec![usize::MAX; (w * h) as usize]);
        for (tree, q) in quads {
            let (x0, x1) = (q.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min), q.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max));
            let (y0, y1) = (q.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min), q.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max));
            // (a card behind the camera projects anywhere: no tree in view is 400 px across)
            if !(x1 - x0 < 400.0 && y1 - y0 < 400.0) {
                continue;
            }
            for y in (y0.floor().max(0.0) as u32)..(y1.ceil().min(f64::from(h)) as u32) {
                for x in (x0.floor().max(0.0) as u32)..(x1.ceil().min(f64::from(w)) as u32) {
                    if inside(q, [f64::from(x) + 0.5, f64::from(y) + 0.5]) {
                        let i = (y * w + x) as usize;
                        // (one tree's two cards are one tree)
                        if owner[i] != *tree {
                            count[i] += 1;
                        }
                        owner[i] = *tree;
                    }
                }
            }
        }
        (count, owner)
    }

    /// The shadow's alpha where `with` is `bare` darkened toward the shadow colour, alike in the
    /// three channels (within 0.06); none for anything else.
    fn shade(bare: [u8; 4], with: [u8; 4]) -> Option<f64> {
        let alphas: Vec<f64> = (0..3)
            .filter(|&c| f64::from(bare[c]) - f64::from(SHADOW_COLOR[c]) > 25.0)
            .map(|c| (f64::from(bare[c]) - f64::from(with[c])) / (f64::from(bare[c]) - f64::from(SHADOW_COLOR[c])))
            .collect();
        if alphas.len() < 2 || bare == with {
            return None;
        }
        let (lo, hi) = alphas.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &a| (lo.min(a), hi.max(a)));
        (hi - lo <= 0.06 && lo > 0.04 && hi < 0.7).then_some((lo + hi) / 2.0)
    }

    /// Humlegården (zoom 16, pitched 45°, the fixtures' park, its ground and canopy held): the
    /// park's tile holds as many trees as `plant_tile` (and today's TypeScript) plants there;
    /// - on a grey day (no shadows) each tree's cards are cut to its picture: of the pixels only
    ///   one tree's cards cover, a share shows the ground as it is without the trees, the rest the
    ///   tree (a card drawn whole would leave none, a mask of nothing all);
    /// - in the evening sun each tree's shadow lies on the ground where its cards laid along the
    ///   sun fall (the pixels darkened toward the shadow colour, alike in every channel, lie within
    ///   them, far fewer within the cards laid the other way), and where two trees' shadows meet
    ///   the ground is darkened once.
    #[test]
    fn humlegarden_s_trees_stand_cut_to_their_shapes_with_their_shadows_away_from_the_sun() {
        let Some(gpu) = gpu() else { return };
        let mut park = Park::new(&gpu);
        let view = ViewFrame::new(&HUMLEGARDEN, SIZE, 1.0, 0.0);
        let (w, h) = (SIZE[0] as u32, SIZE[1] as u32);

        // the planting: the park's tile as plant_tile and the TypeScript have it
        let grey = park.shot(&gpu, MIDSUMMER, PRESETS.cloudy, false, true);
        grey.save_png(&shots_dir().join("r7-humlegarden-grey.png")).expect("png");
        let watch = park.watch.borrow();
        let planted = watch.tiles().planted(PARK).expect("the park's tile planted").clone();
        let (trees, direct) = plant(PARK, true, &park.fixtures.read(io::STREETS_GL, PARK).unwrap().unwrap(), &ground_of(&park.fixtures, PARK), MIDSUMMER);
        assert_eq!(planted, direct);
        assert_eq!((planted.conifers, planted.broadleaves, planted.len()), (trees.conifers.len(), trees.broadleaves.len(), 665));
        assert!(watch.drawn().any(|t| t == PARK));
        assert_eq!(watch.shadow(), None, "no shadows under cloud");

        // every drawn tree's cards
        let mut quads = Vec::new();
        let mut trees_drawn = Vec::new();
        for tile in watch.drawn() {
            for tree in &watch.tiles().planted(tile).expect("planted").instances {
                let id = trees_drawn.len();
                trees_drawn.push((tile, *tree));
                quads.extend(card_quads(&view, tile, tree, None).map(|q| (id, q)));
            }
        }
        drop(watch);
        let bare_grey = park.shot(&gpu, MIDSUMMER, PRESETS.cloudy, false, false);
        let (count, _) = coverage(&quads, w, h);
        let (mut alone, mut ground) = (0usize, 0usize);
        for y in 0..h {
            for x in 0..w {
                // (a pixel only one tree covers, away from the cards' edges)
                let i = (y * w + x) as usize;
                let lone = |dx: i32, dy: i32| {
                    let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                    xx >= 0 && yy >= 0 && (xx as u32) < w && (yy as u32) < h && count[(yy as u32 * w + xx as u32) as usize] == 1
                };
                if count[i] != 1 || !(lone(-1, 0) && lone(1, 0) && lone(0, -1) && lone(0, 1)) {
                    continue;
                }
                alone += 1;
                ground += usize::from(grey.pixel(x, y) == bare_grey.pixel(x, y));
            }
        }
        let cut = ground as f64 / alone as f64;
        eprintln!("{} trees drawn; {alone} px under one tree's cards alone, {:.1} % of them the bare ground", trees_drawn.len(), 100.0 * cut);
        assert!(alone > 5000, "{alone}");
        assert!((0.2..0.8).contains(&cut), "the cards are cut to their pictures: {cut}");

        // the evening sun: the shadows
        let lit = park.shot(&gpu, EVENING, PRESETS.sunny, false, true);
        lit.save_png(&shots_dir().join("r7-humlegarden-shadows.png")).expect("png");
        let (colour, step) = park.watch.borrow().shadow().expect("shadows in the evening sun");
        let bare = park.shot(&gpu, EVENING, PRESETS.sunny, false, false);
        assert_eq!(&colour[..3], &SHADOW_COLOR[..]);
        let laid = |step: [f64; 2]| {
            let quads: Vec<(usize, [[f64; 2]; 4])> =
                trees_drawn.iter().enumerate().flat_map(|(id, (tile, tree))| card_quads(&view, *tile, tree, Some(step)).map(|q| (id, q))).collect();
            coverage(&quads, w, h).0
        };
        let (along, against) = (laid(step), laid([-step[0], -step[1]]));
        let (mut shaded, mut within, mut opposite) = (0usize, 0usize, 0usize);
        let (mut along_only, mut against_only) = (0usize, 0usize);
        let (mut once, mut twice) = (Vec::new(), Vec::new());
        for y in 0..h {
            for x in 0..w {
                let Some(alpha) = shade(bare.pixel(x, y), lit.pixel(x, y)) else { continue };
                let i = (y * w + x) as usize;
                shaded += 1;
                within += usize::from(along[i] > 0);
                opposite += usize::from(against[i] > 0);
                along_only += usize::from(along[i] > 0 && against[i] == 0);
                against_only += usize::from(against[i] > 0 && along[i] == 0);
                match along[i] {
                    1 => once.push(alpha),
                    2.. => twice.push(alpha),
                    0 => {}
                }
            }
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        eprintln!(
            "the sun's step {step:?}: {shaded} px of shade, {:.1} % within the cards laid along it, {:.1} % within those laid against it ({along_only} px within the first only, {against_only} within the second only); alpha {:.3} under one tree's shadow ({} px), {:.3} under two or more ({} px), the tile's {:.3}",
            100.0 * within as f64 / shaded as f64,
            100.0 * opposite as f64 / shaded as f64,
            mean(&once),
            once.len(),
            mean(&twice),
            twice.len(),
            f64::from(colour[3]) / 255.0
        );
        assert!(shaded > 3000, "{shaded}");
        assert!(within as f64 > 0.9 * shaded as f64, "{within} of {shaded}");
        // (in the park's thick of trees both lay over much the same ground: where only one does)
        assert!(along_only > 10 * against_only, "{along_only} along the sun, {against_only} against it");
        assert!(!once.is_empty() && twice.len() > 100);
        assert!((mean(&twice) - mean(&once)).abs() < 0.03, "darkened once: {} against {}", mean(&twice), mean(&once));
        assert!(mean(&once) <= f64::from(colour[3]) / 255.0 + 0.02);

        // the whole map at noon and in the evening, with and without the trees, for review
        for (now, name) in [(MIDSUMMER, "noon"), (EVENING, "evening")] {
            for trees in [true, false] {
                let image = park.shot(&gpu, now, PRESETS.sunny, true, trees);
                image.save_png(&shots_dir().join(format!("r7-humlegarden-{name}{}.png", if trees { "" } else { "-bare" }))).expect("png");
            }
        }
        assert!(park.fixtures.misses().iter().all(|(source, _)| [CANOPY, "nmd", "worldcover"].contains(&source.as_str())), "{:?}", park.fixtures.misses());
    }

    /// V5 from the fixtures at noon with the trees and without, for review beside
    /// `docs/port/reference/realistic-v5-1440x900.png`: Humlegården's crowns at the top, the
    /// trees along the quays and on the islands.
    #[test]
    fn v5_with_its_trees() {
        let Some(gpu) = gpu() else { return };
        let mut park = Park::new(&gpu);
        let v5 = ViewState { latitude: 59.3293, longitude: 18.0686, zoom: 15.5, pitch: 60.0, bearing: 20.0, position: None };
        let mut trees_on = 0;
        for trees in [true, false] {
            let scene = realistic(v5, [1440.0, 900.0], MIDSUMMER, false);
            park.shown.set(trees);
            let image = render_with(&gpu, &mut park.renderer, &scene, 1.0).expect("rendered");
            assert!(!park.renderer.pending(), "{:?}", park.renderer.pending_layers());
            image.save_png(&shots_dir().join(format!("r7-v5{}.png", if trees { "" } else { "-bare" }))).expect("png");
            if trees {
                trees_on = park.watch.borrow().drawn().count();
            }
        }
        eprintln!("{trees_on} tiles of trees drawn at V5");
        assert!(trees_on > 20);
    }
}
