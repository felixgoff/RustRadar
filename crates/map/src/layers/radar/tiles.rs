//! RainViewer's radar tiles (item W1; doc 02 §3.2, §3.4; doc 01 §5.11): the tile source the
//! ground radar (W2) and the lifted radar (W3) draw from, as today's `radarLayer` (G:1340-1354),
//! `loadRadarTile` (`radar-pool.ts`) and the radar workers (`radar-worker.ts`) load it.
//!
//! - **Which frame.** The scene builder hands the frame's tile URL template ([`Input::url`],
//!   `backend::rainviewer::Frame::url`, item W5); `None` draws no RainViewer (the weather off, no
//!   frame yet, or the last ask for one failed: `if (radarUrl) layers.push(radarLayer(radarUrl))`,
//!   G:2683). The tiles are asked of the source [`io::RAINVIEWER`], which the app maps to that
//!   template (`tile_fetcher::TileSources::set_template`): **set the template before the scene
//!   carries the new URL**, so no request of the new frame goes out under the old one's.
//! - **Tiles.** 256 px, zooms 0 to 7 (`RADAR_TILES`: deck's rule, `round(zoom + 1)` clamped to
//!   0–7, the z7 tiles stretched past it), deck's TileLayer defaults otherwise: 6 requests at once
//!   ([`MAX_REQUESTS`]), `best-available` refinement, a cache of five times the selection.
//! - **Decoded on the worker pool** ([`decode`], as a radar worker's `tile` job): the PNG to RGBA,
//!   then `scene::radar::process_tile`: the Universal Blue colours back to dBZ (blanked where a
//!   measured volume's coverage is), blurred, contoured into 5 dBZ bands, triangulated, with the
//!   wall lines and the lifted levels. A tile with no echo (most of the world) decodes to a
//!   [`Decoded`] without a mesh: it counts as loaded, so a dry tile hides the coarser one that
//!   stood in for it, as deck's `null` content does. The meshes' vertices are also put into the
//!   tile's own frame ([`tile_vertices`], `u` east and `v` south in tile units, for
//!   [`crate::layers::PrepareCx::tile_frame`]), so the upload is a copy.
//! - **The pool's result cache** (`radar-pool.ts`): at most [`CACHE_SIZE`] finished tiles, dry
//!   ones included, least recently used out first, keyed by the tile and the keys of the measured
//!   coverages over it ([`cache_key`]); emptied when the frame's URL changes. A decode that was
//!   cancelled is not kept. A tile set reloaded for a change of coverage elsewhere costs nothing
//!   but the fetch.
//! - **Coverage.** [`Input::coverages`] are where MRMS and DWD measure (item W4's volumes, in the
//!   backend's order): RainViewer is blanked there. The pool takes a new list only when its keys
//!   differ (`setRadarCoverage`); the tiles are made again when the layer's key does
//!   ([`coverage_key`], `radarCoverageKey()`: in today's app part of the TileLayer's id).
//! - **In view** ([`RadarTiles::in_view`], `onViewportLoad`, G:1218-1225): once every selected
//!   tile has loaded (and again whenever that set or its loading state changes), the selected
//!   tiles that have a mesh, in selection order; [`RadarTiles::version`] moves on only when that
//!   set changes, compared by identity (`radarTiles`, `radarVersion`). The lifted layers are built
//!   from it (W3). It keeps the last viewport's tiles while a new frame or coverage loads, and while
//!   no RainViewer is drawn, as today's `radarTiles` does.
//!
//! What W2 and W3 get: [`RadarTiles<G>`] with `G` what their upload makes of a tile with echo
//! ([`RadarTiles::update`]'s `upload`, run within the frame's upload budget); [`RadarTiles::visible`]
//! for the ground (every visible tile with echo, coarse to fine: `renderSubLayers` per tile),
//! [`RadarTiles::in_view`] and [`RadarTiles::version`] for the stack.
//!
//! Deviations:
//! - A tile whose answer is in the result cache is still fetched (from the disk cache in the app)
//!   before the cache is asked: the tile runtime decodes what it fetched. Its contouring is saved.
//! - A change of coverage reloads the selected tiles in place, their old meshes drawn until the
//!   new ones are in (`TileSet::reload_all`); today a new TileLayer starts empty. A new frame
//!   starts empty, as today.
//! - The coverages a tile is decoded with are taken when its decode starts (after its fetch), not
//!   when its job was sent; the key it is cached under comes from the same list, and the tiles are
//!   reloaded for any change, so no tile keeps a picture of an older coverage.
//! - The PNG's pixels come from the `image` crate exactly as stored; the browser's canvas stores
//!   them premultiplied by alpha, so its `getImageData` may differ by a level in the colour of
//!   partly transparent pixels (whose reflectivity reads only their alpha).
//! - A tile the source has nothing for (404) or that fails does not stand in for its ancestors
//!   (the tile runtime's rule; deck counts it loaded with nothing in it).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use rustradar_scene::camera::{Viewport, lng_lat_to_world};
use rustradar_scene::radar::{self, Coverage, RadarMesh, TileMesh, covered_by, coverages_over, process_tile, tile_bounds};
use rustradar_scene::tiles::RADAR_TILES;

use crate::frame::tile_extent;
use crate::gpu::Gpu;
use crate::io;
use crate::jobs::Cancel;
use crate::layers::{ByteSize, DeckSelect, Init, PrepareCx, Source, TileIndex, TileRuntime, TileSet, TileSetOptions, UploadBudget};
use crate::pipelines::fill;

/// The radar TileLayer's `maxRequests`: deck's default (the layer sets none, G:1340-1354).
pub const MAX_REQUESTS: usize = 6;

/// Finished tiles the result cache keeps (`CACHE_SIZE`, `radar-pool.ts`).
pub const CACHE_SIZE: usize = 96;

/// What the scene builder hands the radar tiles (`radar::Input::tiles`).
#[derive(Clone, Default)]
pub struct Input {
    /// The RainViewer frame's tile URL template (`backend::rainviewer::Frame::url`); `None`: no
    /// RainViewer on the map.
    pub url: Option<String>,
    /// Where the measured volumes have data, in the order the backend gave the volumes
    /// (`measured.map(v => v.coverage)`): RainViewer is blanked there.
    pub coverages: Arc<Vec<Coverage>>,
}

impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // the masks are hundreds of kilobytes: their keys say which they are
        let keys: Vec<&str> = self.coverages.iter().map(|c| c.key.as_str()).collect();
        f.debug_struct("Input").field("url", &self.url).field("coverages", &keys).finish()
    }
}

/// A tile with echo, decoded: the scene's meshes and their vertices in the tile's frame.
#[derive(Debug, Clone, PartialEq)]
pub struct RadarTile {
    pub index: TileIndex,
    /// `process_tile`'s meshes, in degrees (`TileMesh`): `slices` (every band, flat; from a level's
    /// band up, lifted) and `walls` (z from 0 at the foot to 1 at the top), with the levels.
    pub mesh: TileMesh,
    /// `mesh.slices`' vertices in the tile's frame ([`tile_vertices`]): `u` east, `v` south in tile
    /// units, `h` 0, the band's colour. Drawn with `mesh.slices.indices`.
    pub slice_vertices: Vec<fill::Vertex>,
    /// `mesh.walls`' vertices likewise, `h` the wall's unit height (0 at the foot, 1 at the top: the
    /// draw stretches it between two heights); empty without walls. Drawn with
    /// `mesh.walls`' indices.
    pub wall_vertices: Vec<fill::Vertex>,
}

impl RadarTile {
    /// `mesh` of `index`, with its vertices put into the tile's frame.
    pub fn new(index: TileIndex, mesh: TileMesh) -> Self {
        let slice_vertices = tile_vertices(&mesh.slices, index);
        let wall_vertices = mesh.walls.as_ref().map(|w| tile_vertices(w, index)).unwrap_or_default();
        RadarTile { index, mesh, slice_vertices, wall_vertices }
    }

    fn byte_size(&self) -> usize {
        let mesh = |m: &RadarMesh| m.positions.len() * 4 + m.colors.len() + m.indices.len() * 4;
        mesh(&self.mesh.slices)
            + self.mesh.walls.as_ref().map_or(0, mesh)
            + (self.slice_vertices.len() + self.wall_vertices.len()) * size_of::<fill::Vertex>()
    }
}

/// A RainViewer tile as the worker pool hands it over: its meshes, or `None` when it has no echo.
#[derive(Debug, Clone, Default)]
pub struct Decoded {
    pub tile: Option<Arc<RadarTile>>,
}

impl ByteSize for Decoded {
    fn byte_size(&self) -> usize {
        self.tile.as_ref().map_or(0, |t| t.byte_size())
    }
}

/// What a tile set holds for a loaded tile: its meshes and what the upload made of them, or nothing
/// for a dry tile.
pub struct Content<G>(Option<(Arc<RadarTile>, G)>);

impl<G> Content<G> {
    /// The tile's meshes and its upload, if it has echo.
    pub fn get(&self) -> Option<(&Arc<RadarTile>, &G)> {
        self.0.as_ref().map(|(t, g)| (t, g))
    }
}

/// A mesh's vertices in its tile's frame ([`crate::frame::ViewFrame::tile`]): `u` east and `v`
/// south in tile units from the tile's north-west corner, from the mesh's longitude and latitude
/// (in `f64` through deck's Web Mercator); `h` is the mesh's third coordinate when it has one (the
/// walls' unit height), else 0; the colours as they are.
pub fn tile_vertices(mesh: &RadarMesh, tile: TileIndex) -> Vec<fill::Vertex> {
    let extent = tile_extent(tile.z);
    let (west, north) = (f64::from(tile.x) * extent, rustradar_scene::camera::WORLD_SIZE - f64::from(tile.y) * extent);
    let size = mesh.size.max(2);
    mesh.positions
        .chunks_exact(size)
        .zip(mesh.colors.chunks_exact(4))
        .map(|(p, c)| {
            let [x, y] = lng_lat_to_world([f64::from(p[0]), f64::from(p[1])]);
            let h = if size > 2 { p[2] } else { 0.0 };
            fill::Vertex { position: [((x - west) / extent) as f32, ((north - y) / extent) as f32, h], color: [c[0], c[1], c[2], c[3]] }
        })
        .collect()
}

/// The scene's index of a map tile.
fn scene_index(tile: TileIndex) -> radar::TileIndex {
    radar::TileIndex { x: tile.x, y: tile.y, z: tile.z }
}

/// The result cache's key of a tile: `z/x/y`, then `|` and the keys of the coverages over it,
/// joined by `|`, in the list's order (`loadRadarTile`).
pub fn cache_key(tile: TileIndex, coverages: &[Coverage]) -> String {
    let over = coverages_over(&tile_bounds(scene_index(tile)), coverages);
    let keys: Vec<&str> = over.iter().map(|c| c.key.as_str()).collect();
    format!("{}/{}/{}|{}", tile.z, tile.x, tile.y, keys.join("|"))
}

/// The key of everything the tiles depend on besides their address (`radarCoverageKey`): the
/// coverages' keys sorted and joined by `-`, or `"full"` when there are none.
pub fn coverage_key(coverages: &[Coverage]) -> String {
    let mut keys: Vec<&str> = coverages.iter().map(|c| c.key.as_str()).collect();
    if keys.is_empty() {
        return "full".into();
    }
    keys.sort_unstable();
    keys.join("-")
}

/// Whether two coverage lists are the same to the pool (`setRadarCoverage`: the keys in order).
fn same_coverages(a: &[Coverage], b: &[Coverage]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.key == b.key)
}

/// The decode step, on the worker pool (a radar worker's `tile` job): a tile's PNG through
/// `process_tile`, blanked under `coverages`. An error for bytes that are no image, or once
/// cancelled.
pub fn decode(tile: TileIndex, bytes: &[u8], coverages: &[Coverage], cancel: &Cancel) -> Result<Decoded, String> {
    if cancel.is_cancelled() {
        return Err("cancelled".into());
    }
    let image = image::load_from_memory(bytes).map_err(|e| format!("radar tile: {e}"))?.into_rgba8();
    if cancel.is_cancelled() {
        // a tile the view has left is not worth contouring
        return Err("cancelled".into());
    }
    let (width, height) = (image.width() as usize, image.height() as usize);
    let mesh = process_tile(image.as_raw(), width, height, scene_index(tile), covered_by(coverages));
    Ok(Decoded { tile: mesh.map(|mesh| Arc::new(RadarTile::new(tile, mesh))) })
}

/// The pool's result cache: least recently used out first.
#[derive(Default)]
struct Results {
    entries: HashMap<String, (u64, Option<Arc<RadarTile>>)>,
    clock: u64,
    hits: u64,
    misses: u64,
}

impl Results {
    fn get(&mut self, key: &str) -> Option<Option<Arc<RadarTile>>> {
        self.clock += 1;
        let Some(entry) = self.entries.get_mut(key) else {
            self.misses += 1;
            return None;
        };
        // most recently used goes last
        entry.0 = self.clock;
        self.hits += 1;
        Some(entry.1.clone())
    }

    fn insert(&mut self, key: String, tile: Option<Arc<RadarTile>>) {
        self.clock += 1;
        self.entries.insert(key, (self.clock, tile));
        if self.entries.len() > CACHE_SIZE {
            let oldest = self.entries.iter().min_by_key(|(_, (used, _))| *used).map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

/// What the main thread and the decode jobs share: the pool's frame, coverages and cache.
#[derive(Default)]
struct Shared {
    /// The frame the cache holds tiles of (`cachedUrl`).
    url: Option<String>,
    /// Moves on whenever the cache is emptied: a decode that started before is not kept.
    generation: u64,
    coverages: Arc<Vec<Coverage>>,
    results: Results,
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How the result cache has been used: answers it had and decodes it did not save.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    /// Tiles it holds now.
    pub held: usize,
}

/// RainViewer's tiles for the map: the tile set, the result cache, the tiles in view. `G` is what
/// the layer's upload makes of a tile with echo (GPU buffers; `()` to keep only the CPU meshes).
pub struct RadarTiles<G = ()> {
    tiles: TileSet<Decoded, Content<G>>,
    shared: Arc<Mutex<Shared>>,
    runtime: TileRuntime,
    /// The runtime's fetcher generation the cache was filled with.
    generation: u64,
    /// The frame drawn now.
    url: Option<String>,
    /// [`coverage_key`] of the coverages the tiles were made for.
    layer_key: String,
    /// The tile layer's `isLoaded` and `frameNumber` at the last update.
    was_loaded: bool,
    frame_number: u64,
    in_view: Vec<(TileIndex, Arc<RadarTile>)>,
    version: u64,
}

impl<G> std::fmt::Debug for RadarTiles<G> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RadarTiles")
            .field("url", &self.url)
            .field("layer_key", &self.layer_key)
            .field("tiles", &self.tiles)
            .field("in_view", &self.in_view.len())
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl<G> RadarTiles<G> {
    /// The radar tiles on the renderer's tile runtime.
    pub fn new(init: &Init<'_>) -> Self {
        Self::on(init.tiles)
    }

    /// The radar tiles on `runtime` (tests without a GPU).
    pub fn on(runtime: &TileRuntime) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let jobs = Arc::clone(&shared);
        let options = TileSetOptions { max_requests: MAX_REQUESTS, ..Default::default() };
        let tiles = TileSet::new(runtime, vec![Source::new(io::RAINVIEWER)], DeckSelect(RADAR_TILES), options, move |tile, bytes, cancel| {
            // the coverages as the job starts, and the key they make: a newer list may come
            // while it runs
            let (generation, coverages) = {
                let shared = lock(&jobs);
                (shared.generation, Arc::clone(&shared.coverages))
            };
            let key = cache_key(tile, &coverages);
            if let Some(tile) = lock(&jobs).results.get(&key) {
                return Ok(Some(Decoded { tile }));
            }
            let decoded = decode(tile, &bytes, &coverages, cancel)?;
            if cancel.is_cancelled() {
                // an aborted load is not cached
                return Err("cancelled".into());
            }
            let mut shared = lock(&jobs);
            if shared.generation == generation {
                shared.results.insert(key, decoded.tile.clone());
            }
            Ok(Some(decoded))
        });
        RadarTiles {
            tiles,
            shared,
            runtime: runtime.clone(),
            generation: runtime.generation(),
            url: None,
            layer_key: coverage_key(&[]),
            was_loaded: false,
            frame_number: 0,
            in_view: Vec::new(),
            version: 0,
        }
    }

    /// One frame's work for this frame's camera ([`Self::update_with`]); `upload` makes what the
    /// layer draws of a tile with echo, on the main thread within the frame's upload budget.
    /// Whether the tiles drawn changed.
    pub fn update(&mut self, cx: &mut PrepareCx<'_>, input: &Input, mut upload: impl FnMut(&Gpu, TileIndex, &Arc<RadarTile>) -> G) -> bool {
        let (gpu, view) = (cx.gpu, cx.view);
        self.update_with(&view.viewport, cx.budget, input, |tile, radar| upload(gpu, tile, radar))
    }

    /// One frame's work for `viewport`: the frame and the coverages from `input`, then the tile
    /// set's update within `budget`, then the tiles in view. Whether the tiles drawn changed.
    pub fn update_with(&mut self, viewport: &Viewport, budget: &mut UploadBudget, input: &Input, mut upload: impl FnMut(TileIndex, &Arc<RadarTile>) -> G) -> bool {
        let generation = self.runtime.generation();
        let mut changed = false;
        {
            let mut shared = lock(&self.shared);
            if generation != self.generation {
                // another fetcher: nothing the old one gave is kept (the tile set starts over by
                // itself)
                self.generation = generation;
                shared.results.clear();
                shared.generation += 1;
            }
            if input.url != self.url {
                // a new frame: nothing of the old one is wanted again
                if let Some(url) = &input.url
                    && shared.url.as_ref() != Some(url)
                {
                    shared.results.clear();
                    shared.url = Some(url.clone());
                    shared.generation += 1;
                }
                // the old frame's tile layer goes, and a new one starts empty
                changed = !self.tiles.is_empty();
                self.tiles.clear();
                self.url = input.url.clone();
                self.was_loaded = false;
            }
            if !same_coverages(&shared.coverages, &input.coverages) {
                shared.coverages = Arc::clone(&input.coverages);
            }
        }
        // the tiles have the measured area punched out of them: made again when its key changes
        let key = coverage_key(&input.coverages);
        if key != self.layer_key {
            self.layer_key = key;
            self.tiles.reload_all();
            self.was_loaded = false;
        }
        if self.url.is_none() {
            return changed;
        }
        changed |= self.tiles.update(viewport, budget, |tile, decoded| {
            Content(decoded.tile.map(|radar| {
                let gpu = upload(tile, &radar);
                (radar, gpu)
            }))
        });
        // onViewportLoad: when every selected tile is in, and again when that set changes
        let loaded = self.tiles.is_loaded();
        let frame_number = self.tiles.frame_number();
        if loaded && (loaded != self.was_loaded || frame_number != self.frame_number) {
            self.viewport_loaded();
        }
        self.was_loaded = loaded;
        self.frame_number = frame_number;
        changed
    }

    /// `onRadarTiles`: the selected tiles with echo; the version moves on only when they change.
    fn viewport_loaded(&mut self) {
        let next: Vec<(TileIndex, Arc<RadarTile>)> = self
            .tiles
            .selected()
            .iter()
            .filter_map(|&index| self.tiles.get(index)?.get().map(|(radar, _)| (index, Arc::clone(radar))))
            .collect();
        // fired again whenever the view settles; the same tiles need nothing rebuilt
        let known: HashSet<*const RadarTile> = self.in_view.iter().map(|(_, t)| Arc::as_ptr(t)).collect();
        if next.len() == self.in_view.len() && next.iter().all(|(_, t)| known.contains(&Arc::as_ptr(t))) {
            return;
        }
        self.in_view = next;
        self.version += 1;
    }

    /// The tiles with echo to draw on the ground, coarse to fine, with their uploads.
    pub fn visible(&self) -> impl Iterator<Item = (TileIndex, &Arc<RadarTile>, &G)> {
        self.tiles.visible().filter_map(|(index, content)| content.get().map(|(radar, gpu)| (index, radar, gpu)))
    }

    /// The selected tiles with echo as of the last time they had all loaded, in selection order
    /// (today's `radarTiles`), for the lifted layers.
    pub fn in_view(&self) -> &[(TileIndex, Arc<RadarTile>)] {
        &self.in_view
    }

    /// Moves on whenever [`Self::in_view`] changes (today's `radarVersion`).
    pub fn version(&self) -> u64 {
        self.version
    }

    /// A cached tile's meshes and upload, if it has loaded with echo.
    pub fn get(&self, index: TileIndex) -> Option<(&Arc<RadarTile>, &G)> {
        self.tiles.get(index)?.get()
    }

    /// The frame drawn now.
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// The tile set, for its states and selection.
    pub fn tile_set(&self) -> &TileSet<Decoded, Content<G>> {
        &self.tiles
    }

    /// How the result cache has been used.
    pub fn cache_stats(&self) -> CacheStats {
        let shared = lock(&self.shared);
        CacheStats { hits: shared.results.hits, misses: shared.results.misses, held: shared.results.entries.len() }
    }

    /// Whether tiles of the frame drawn are still on their way.
    pub fn pending(&self) -> bool {
        self.url.is_some() && self.tiles.loading()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use rustradar_scene::camera::{ViewState, viewport_for};

    use super::*;
    use crate::io::{DirTiles, FetchRequest, FetchResult, FnFetcher, TileFetcher};
    use crate::jobs::Pool;

    fn fixture(tile: TileIndex) -> Vec<u8> {
        let path: PathBuf = io::fixture_tiles_dir().join(format!("rainviewer/{}/{}/{}.png", tile.z, tile.x, tile.y));
        fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn decoded(tile: TileIndex) -> Arc<RadarTile> {
        decode(tile, &fixture(tile), &[], &Cancel::new()).unwrap().tile.expect("a tile with echo")
    }

    /// `sum += a[i] * (k % 97 + 1)` over every value, as the TypeScript checksums were taken.
    fn checksum<T: Copy + Into<f64>>(values: &[T]) -> f64 {
        values.iter().enumerate().map(|(k, &v)| v.into() * ((k % 97) + 1) as f64).sum()
    }

    /// What bun's `processTile` gave for a fixture tile's pixels: the levels, then for the slices
    /// and the walls the lengths of the positions and indices, their checksums and the band starts.
    struct Golden {
        levels: &'static [(f64, usize)],
        slices: (usize, usize, f64, f64, [u32; 15], [u32; 15]),
        walls: (usize, usize, f64, f64, [u32; 15]),
    }

    fn check(tile: TileIndex, golden: &Golden) {
        let radar = decoded(tile);
        let mesh = &radar.mesh;
        assert_eq!(mesh.id, format!("{}/{}/{}", tile.z, tile.x, tile.y));
        let levels: Vec<(f64, usize)> = mesh.levels.iter().map(|l| (l.km, l.from_band)).collect();
        assert_eq!(levels, golden.levels, "{}", mesh.id);
        let s = &mesh.slices;
        let (positions, indices, sum, isum, starts, vertex_starts) = golden.slices;
        assert_eq!((s.positions.len(), s.indices.len()), (positions, indices), "{} slices", mesh.id);
        assert_eq!(s.band_starts, starts, "{} slices", mesh.id);
        assert_eq!(s.band_vertex_starts, vertex_starts, "{} slices", mesh.id);
        assert_eq!(checksum(&s.positions), sum, "{} slice positions", mesh.id);
        assert_eq!(checksum(&s.indices), isum, "{} slice triangles", mesh.id);
        let w = mesh.walls.as_ref().expect("walls");
        let (positions, indices, sum, isum, starts) = golden.walls;
        assert_eq!((w.positions.len(), w.indices.len()), (positions, indices), "{} walls", mesh.id);
        assert_eq!(w.band_starts, starts, "{} walls", mesh.id);
        assert_eq!(checksum(&w.positions), sum, "{} wall positions", mesh.id);
        assert_eq!(checksum(&w.indices), isum, "{} wall triangles", mesh.id);
        // the vertices in the tile's frame: one per mesh vertex, inside the tile
        assert_eq!(radar.slice_vertices.len(), s.vertex_count());
        assert_eq!(radar.wall_vertices.len(), w.vertex_count());
        for v in radar.slice_vertices.iter().chain(&radar.wall_vertices) {
            let [u, v, h] = v.position;
            assert!((-1e-5..=1.0 + 1e-5).contains(&u) && (-1e-5..=1.0 + 1e-5).contains(&v), "{u} {v}");
            assert!(h == 0.0 || h == 1.0);
        }
    }

    #[test]
    fn fixture_tiles_mesh_as_the_typescript_does() {
        // the committed RainViewer frame's tiles over Germany and Poland, run through `processTile`
        // in bun on the same pixels (the PNG has no colour chunks: the browser reads them as
        // stored): the same bands, vertices, triangles and walls, value for value
        check(
            TileIndex::new(35, 22, 6),
            &Golden {
                levels: &[(1.5, 1), (3.0, 2), (4.5, 4), (6.0, 5), (8.0, 7)],
                slices: (
                    51906,
                    71808,
                    84487670.22814941,
                    45733156101.0,
                    [0, 10134, 22281, 33966, 45960, 56985, 65475, 70428, 71790, 71808, 71808, 71808, 71808, 71808, 71808],
                    [0, 3693, 8062, 12391, 16726, 20632, 23622, 25404, 25944, 25953, 25953, 25953, 25953, 25953, 25953],
                ),
                walls: (
                    38706,
                    37200,
                    42415646.42710686,
                    11681134050.0,
                    [0, 3516, 8994, 14430, 19908, 25848, 30984, 35256, 37158, 37200, 37200, 37200, 37200, 37200, 37200],
                ),
            },
        );
        check(
            TileIndex::new(36, 22, 6),
            &Golden {
                levels: &[(1.5, 1), (3.0, 2), (4.5, 4), (6.0, 5), (8.0, 7), (10.0, 9), (12.0, 11)],
                slices: (
                    72302,
                    99153,
                    126240639.88841438,
                    87896764143.0,
                    [0, 10704, 24528, 39858, 55182, 69375, 81420, 89130, 93354, 95634, 97515, 98709, 99153, 99153, 99153],
                    [0, 3913, 9024, 14618, 20151, 25378, 29669, 32435, 33951, 34793, 35461, 35964, 36151, 36151, 36151],
                ),
                walls: (
                    50394,
                    48732,
                    59012374.083192825,
                    19942544772.0,
                    [0, 3030, 8670, 15300, 22194, 29298, 36396, 41748, 44880, 46320, 47280, 48240, 48732, 48732, 48732],
                ),
            },
        );
        check(
            TileIndex::new(17, 7, 5),
            &Golden {
                levels: &[(1.5, 1), (3.0, 2), (4.5, 4), (6.0, 5), (8.0, 7), (10.0, 9), (12.0, 11)],
                slices: (
                    104462,
                    144525,
                    216101143.9623537,
                    181537230423.0,
                    [0, 23373, 49389, 75558, 97905, 115524, 127227, 134466, 139143, 141975, 143772, 144468, 144525, 144525, 144525],
                    [0, 8281, 17438, 26734, 34696, 41205, 45615, 48328, 50067, 51151, 51821, 52153, 52231, 52231, 52231],
                ),
                walls: (
                    99762,
                    93258,
                    138192499.23242378,
                    74775382426.0,
                    [0, 12648, 25434, 41934, 56154, 69660, 79044, 84756, 88332, 90780, 92172, 93084, 93258, 93258, 93258],
                ),
            },
        );
    }

    /// Whether a point in world Mercator lies in one of a mesh's triangles (edges included).
    fn covers(world: &[[f64; 2]], indices: &[u32], p: [f64; 2]) -> bool {
        let cross = |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        indices.chunks_exact(3).any(|t| {
            let [a, b, c] = [world[t[0] as usize], world[t[1] as usize], world[t[2] as usize]];
            let (d1, d2, d3) = (cross(a, b), cross(b, c), cross(c, a));
            let eps = 1e-12;
            (d1 >= -eps && d2 >= -eps && d3 >= -eps) || (d1 <= eps && d2 <= eps && d3 <= eps)
        })
    }

    /// The ground mesh of a tile (every band) in world Mercator, with only the triangles that come
    /// within `margin` world units of the line `x = at` (`vertical`) or `y = at`.
    fn near(radar: &RadarTile, vertical: bool, at: f64, margin: f64) -> (Vec<[f64; 2]>, Vec<u32>) {
        let s = &radar.mesh.slices;
        let world: Vec<[f64; 2]> = s.positions.chunks_exact(2).map(|p| lng_lat_to_world([f64::from(p[0]), f64::from(p[1])])).collect();
        let axis = usize::from(!vertical);
        let indices = s
            .indices
            .chunks_exact(3)
            .filter(|t| {
                let c: Vec<f64> = t.iter().map(|&i| world[i as usize][axis]).collect();
                c.iter().cloned().fold(f64::INFINITY, f64::min) <= at + margin && c.iter().cloned().fold(f64::NEG_INFINITY, f64::max) >= at - margin
            })
            .flatten()
            .copied()
            .collect();
        (world, indices)
    }

    /// The blurred reflectivity of a fixture tile's pixels (what `process_tile` contours).
    fn blurred(tile: TileIndex) -> Vec<f32> {
        let image = image::load_from_memory(&fixture(tile)).unwrap().into_rgba8();
        let dbz: Vec<f32> = image.as_raw().chunks_exact(4).map(|p| radar::reflectivity(p[0], p[1], p[2], p[3]).max(0.0) as f32).collect();
        radar::mesh::blur(&dbz, 256, 256)
    }

    /// Across the seam between `a` and its neighbour `b` (east of it, or south when `!vertical`):
    /// wherever both tiles' edge pixels hold the first band with room to spare, the two meshes
    /// cover every point from 2 px inside `a` to 2 px inside `b`. Returns how many pixel rows (or
    /// columns) along the seam were checked.
    fn meets_without_a_gap(a: TileIndex, b: TileIndex, vertical: bool) -> usize {
        let (ta, tb) = (decoded(a), decoded(b));
        let extent = tile_extent(a.z);
        let px = extent / 256.0;
        // the seam in world units: `a`'s east edge, or its south edge (y grows north)
        let at = if vertical { f64::from(b.x) * extent } else { rustradar_scene::camera::WORLD_SIZE - f64::from(b.y) * extent };
        let ((wa, ia), (wb, ib)) = (near(&ta, vertical, at, 3.0 * px), near(&tb, vertical, at, 3.0 * px));
        let (da, db) = (blurred(a), blurred(b));
        let mut checked = 0;
        for k in 0..256_usize {
            // the three pixels next to the seam on each side, and their neighbours along it
            let edge = |d: &[f32], first: bool| {
                let depth = if first { 253..=255 } else { 0..=2 };
                (k.saturating_sub(1)..=(k + 1).min(255)).all(|j| {
                    depth.clone().all(|i| {
                        let (c, r) = if vertical { (i, j) } else { (j, i) };
                        d[r * 256 + c] >= 7.0
                    })
                })
            };
            if !(edge(&da, true) && edge(&db, false)) {
                continue;
            }
            checked += 1;
            // the pixel's middle along the seam
            let along = if vertical {
                rustradar_scene::camera::WORLD_SIZE - f64::from(a.y) * extent - (k as f64 + 0.5) * px
            } else {
                f64::from(a.x) * extent + (k as f64 + 0.5) * px
            };
            for step in [-2.0, -1.5, -1.0, -0.5, -0.1, -0.001, 0.0, 0.001, 0.1, 0.5, 1.0, 1.5, 2.0] {
                // into `a` is west (x less) or north (y more)
                let across = if vertical { at + step * px } else { at - step * px };
                let p = if vertical { [across, along] } else { [along, across] };
                let inside_a = step <= 0.0 && covers(&wa, &ia, p);
                let inside_b = step >= 0.0 && covers(&wb, &ib, p);
                assert!(inside_a || inside_b, "a gap at {step} px across the seam of {} and {}, {k} px along", ta.mesh.id, tb.mesh.id);
            }
        }
        checked
    }

    #[test]
    fn two_adjacent_fixture_tiles_meet_without_a_gap() {
        // 6/35/22 and its eastern neighbour, then its southern one: rain across both seams
        let a = TileIndex::new(35, 22, 6);
        let east = meets_without_a_gap(a, TileIndex::new(36, 22, 6), true);
        let south = meets_without_a_gap(a, TileIndex::new(35, 23, 6), false);
        assert!(east >= 100 && south >= 100, "{east} and {south} px of the seams with rain on both sides");
        // and the polygons run onto the seam itself: vertices of both tiles lie on it
        let (ta, tb) = (decoded(a), decoded(TileIndex::new(36, 22, 6)));
        let seam = ta.mesh.bounds[2] as f32;
        assert_eq!(seam, tb.mesh.bounds[0] as f32);
        let on = |t: &RadarTile| t.mesh.slices.positions.chunks_exact(2).filter(|p| p[0] == seam).count();
        assert!(on(&ta) > 100 && on(&tb) > 100, "{} and {} vertices on the seam", on(&ta), on(&tb));
    }

    #[test]
    fn a_dry_tile_decodes_to_no_mesh_and_a_broken_one_fails() {
        let mut png = Vec::new();
        image::RgbaImage::new(256, 256).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let dry = decode(TileIndex::new(1, 1, 3), &png, &[], &Cancel::new()).unwrap();
        assert!(dry.tile.is_none());
        assert_eq!(dry.byte_size(), 0);
        assert!(decode(TileIndex::new(1, 1, 3), b"not a picture", &[], &Cancel::new()).is_err());
        let cancel = Cancel::new();
        cancel.cancel();
        assert!(decode(TileIndex::new(35, 22, 6), &fixture(TileIndex::new(35, 22, 6)), &[], &cancel).is_err());
    }

    /// A coverage of `bounds` with every cell covered.
    fn coverage(key: &str, bounds: [f64; 4]) -> Coverage {
        Coverage { key: key.into(), bounds, width: 4, height: 4, mask: vec![1; 16] }
    }

    #[test]
    fn the_cache_key_names_the_coverages_over_the_tile() {
        let tile = TileIndex::new(35, 22, 6);
        let [w, s, e, n] = tile_bounds(scene_index(tile));
        assert_eq!(cache_key(tile, &[]), "6/35/22|");
        let over = coverage("dwd:a", [w - 1.0, s - 1.0, w + 0.5, n + 1.0]);
        let elsewhere = coverage("mrms:b", [-130.0, 20.0, -60.0, 55.0]);
        let touching = coverage("dwd:c", [e, s, e + 1.0, n]);
        assert_eq!(cache_key(tile, &[elsewhere.clone(), over.clone(), touching.clone()]), "6/35/22|dwd:a|dwd:c");
        // the layer's key: sorted, joined by '-'
        assert_eq!(coverage_key(&[]), "full");
        assert_eq!(coverage_key(&[over, elsewhere]), "dwd:a-mrms:b");
    }

    #[test]
    fn a_covered_tile_is_blanked_where_the_coverage_is() {
        let tile = TileIndex::new(35, 22, 6);
        let [w, s, e, n] = tile_bounds(scene_index(tile));
        // the whole tile measured: nothing of RainViewer's left
        let all = decode(tile, &fixture(tile), &[coverage("x", [w - 1.0, s - 1.0, e + 1.0, n + 1.0])], &Cancel::new()).unwrap();
        assert!(all.tile.is_none());
        // its western half measured: the echo left all lies east of the middle (within a blur)
        let half = coverage("y", [w - 1.0, s - 1.0, (w + e) / 2.0, n + 1.0]);
        let left = decode(tile, &fixture(tile), &[half], &Cancel::new()).unwrap().tile.unwrap();
        let px = (e - w) / 256.0;
        assert!(left.mesh.slices.positions.chunks_exact(2).all(|p| f64::from(p[0]) >= (w + e) / 2.0 - 3.0 * px));
        assert!(left.mesh.slices.indices.len() < decoded(tile).mesh.slices.indices.len());
    }

    #[test]
    fn tile_vertices_are_in_the_tile_frame() {
        let tile = TileIndex::new(35, 22, 6);
        let radar = decoded(tile);
        let s = &radar.mesh.slices;
        let [w, south, e, north] = tile_bounds(scene_index(tile));
        for (p, v) in s.positions.chunks_exact(2).zip(&radar.slice_vertices).step_by(97) {
            let [u, vv, _] = v.position;
            // u is linear in longitude; v follows Mercator, 0 at the north edge and 1 at the south
            assert!((f64::from(u) - (f64::from(p[0]) - w) / (e - w)).abs() < 1e-5);
            let lat = rustradar_scene::radar::tile_latitude(f64::from(tile.y) + f64::from(vv), tile.z);
            assert!((lat - f64::from(p[1])).abs() < 1e-4, "{lat} {}", p[1]);
            assert!(lat >= south - 1e-4 && lat <= north + 1e-4);
        }
        assert_eq!(radar.slice_vertices[0].color, band_color_of(s, 0));
    }

    fn band_color_of(mesh: &RadarMesh, vertex: usize) -> [u8; 4] {
        let c = &mesh.colors[vertex * 4..vertex * 4 + 4];
        [c[0], c[1], c[2], c[3]]
    }

    fn v2() -> ViewState {
        ViewState { latitude: 52.0, longitude: 10.0, zoom: 4.5, pitch: 0.0, bearing: 0.0, position: None }
    }

    /// Updates until nothing is on its way, waiting for the pool between updates.
    fn settle(tiles: &mut RadarTiles, runtime: &TileRuntime, viewport: &Viewport, input: &Input) {
        for _ in 0..500 {
            tiles.update_with(viewport, &mut UploadBudget::unlimited(), input, |_, _| ());
            if !tiles.pending() {
                return;
            }
            runtime.pool.wait_idle();
        }
        panic!("the radar tiles never settled: {tiles:?}");
    }

    /// A fetcher with every request noted.
    struct Noted<F> {
        tiles: F,
        asked: Mutex<Vec<(String, TileIndex)>>,
    }

    impl<F: TileFetcher> TileFetcher for Noted<F> {
        fn fetch(&self, request: FetchRequest) -> std::sync::mpsc::Receiver<FetchResult> {
            self.asked.lock().unwrap().push((request.source.to_string(), request.tile));
            self.tiles.fetch(request)
        }
    }

    const FRAME: &str = "https://tilecache.rainviewer.com/v2/radar/47622fe604ca/256/{z}/{x}/{y}/2/1_1.png";

    #[test]
    fn the_v2_frame_loads_from_the_fixtures_and_reloads_only_what_a_coverage_changes() {
        let fetcher = Arc::new(Noted { tiles: DirTiles::fixtures(), asked: Mutex::new(Vec::new()) });
        let runtime = TileRuntime::new(fetcher.clone(), Arc::new(Pool::new(2)));
        let mut tiles: RadarTiles = RadarTiles::on(&runtime);
        let viewport = viewport_for(&v2(), [1100.0, 720.0]);
        // no frame: nothing asked
        let mut input = Input::default();
        settle(&mut tiles, &runtime, &viewport, &input);
        assert!(fetcher.asked.lock().unwrap().is_empty());
        assert!(!tiles.pending() && tiles.in_view().is_empty() && tiles.version() == 0);

        input.url = Some(FRAME.into());
        settle(&mut tiles, &runtime, &viewport, &input);
        let asked = fetcher.asked.lock().unwrap().clone();
        // only RainViewer's tiles, at round(4.5 + 1) = 6 (on the globe deck's traversal takes a far
        // tile a zoom coarser), and the fixtures hold each of them
        assert!(asked.iter().all(|(source, tile)| source == io::RAINVIEWER && (tile.z == 6 || tile.z == 5)), "{asked:?}");
        assert!(asked.iter().filter(|(_, tile)| tile.z == 6).count() > 40, "{asked:?}");
        let selected = tiles.tile_set().selected().to_vec();
        assert_eq!(asked.len(), selected.len());
        let held = DirTiles::fixtures();
        assert!(selected.iter().all(|&t| held.read(io::RAINVIEWER, t).is_ok_and(|b| b.is_some())), "a V2 tile is missing");
        // the rain in view: the selected tiles with echo, in their order, once
        let wet: Vec<TileIndex> = tiles.in_view().iter().map(|(t, _)| *t).collect();
        assert!(wet.len() >= 20 && wet.len() < selected.len(), "{} of {}", wet.len(), selected.len());
        assert!(wet.iter().all(|t| selected.contains(t)));
        assert!(wet.contains(&TileIndex::new(35, 22, 6)));
        assert_eq!(tiles.version(), 1);
        assert_eq!(tiles.visible().count(), wet.len());
        let stats = tiles.cache_stats();
        assert_eq!((stats.hits, stats.misses as usize, stats.held), (0, selected.len(), selected.len()));
        // settled again for the same view: nothing new
        settle(&mut tiles, &runtime, &viewport, &input);
        assert_eq!(tiles.version(), 1);

        // a measured volume over the western half of 6/35/22: its tiles are made again, the
        // others come from the cache
        let tile = TileIndex::new(35, 22, 6);
        let [w, s, e, n] = tile_bounds(scene_index(tile));
        let before = tiles.get(tile).map(|(t, _)| Arc::clone(t)).unwrap();
        input.coverages = Arc::new(vec![coverage("dwd:test", [w + 0.1, s + 0.1, (w + e) / 2.0, n - 0.1])]);
        settle(&mut tiles, &runtime, &viewport, &input);
        let stats = tiles.cache_stats();
        assert_eq!(stats.misses as usize, selected.len() + 1, "only the covered tile is decoded again");
        assert_eq!(stats.hits as usize, selected.len() - 1);
        let after = tiles.get(tile).map(|(t, _)| Arc::clone(t)).unwrap();
        assert!(after.mesh.slices.indices.len() < before.mesh.slices.indices.len());
        assert_eq!(tiles.version(), 2, "the tile in view changed");
        assert!(tiles.in_view().iter().any(|(t, radar)| *t == tile && Arc::ptr_eq(radar, &after)));

        // a new frame: the cache is emptied and every tile asked again
        input.url = Some(FRAME.replace("47622fe604ca", "new"));
        let asked_before = fetcher.asked.lock().unwrap().len();
        settle(&mut tiles, &runtime, &viewport, &input);
        assert_eq!(fetcher.asked.lock().unwrap().len(), asked_before + selected.len());
        assert_eq!(tiles.cache_stats().held, selected.len());

        // the weather off: nothing drawn, nothing pending, the last tiles in view kept
        input.url = None;
        settle(&mut tiles, &runtime, &viewport, &input);
        assert_eq!(tiles.visible().count(), 0);
        assert!(!tiles.in_view().is_empty());
    }

    /// A 256 px PNG of one colour.
    fn plain(rgba: [u8; 4]) -> Vec<u8> {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(256, 256, image::Rgba(rgba)).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        png
    }

    #[test]
    fn a_dry_tile_hides_the_wet_one_that_stood_in_for_it() {
        // hail everywhere at zoom 5, dry at zoom 6: zoomed in, the z5 picture must go
        let (hail, dry) = (plain([255, 255, 255, 255]), plain([0, 0, 0, 0]));
        let fetcher = FnFetcher(move |source: &str, tile: TileIndex| -> FetchResult {
            assert_eq!(source, io::RAINVIEWER);
            Ok(Some(if tile.z == 5 { hail.clone() } else { dry.clone() }))
        });
        let runtime = TileRuntime::new(Arc::new(fetcher), Arc::new(Pool::new(2)));
        let mut tiles: RadarTiles = RadarTiles::on(&runtime);
        let input = Input { url: Some(FRAME.into()), ..Default::default() };
        let mut view = v2();
        view.zoom = 3.9;
        let far = viewport_for(&view, [800.0, 600.0]);
        settle(&mut tiles, &runtime, &far, &input);
        assert!(tiles.visible().count() > 0 && tiles.visible().all(|(t, _, _)| t.z == 5));
        view.zoom = 4.6;
        let near = viewport_for(&view, [800.0, 600.0]);
        settle(&mut tiles, &runtime, &near, &input);
        assert!(tiles.tile_set().selected().iter().all(|t| t.z == 6));
        assert_eq!(tiles.visible().count(), 0, "a dry tile counts as loaded");
        assert!(tiles.in_view().is_empty());
        assert_eq!(tiles.version(), 2);
    }
}
