//! The tile runtime (plan 1.4.8, doc 04 §13.3): [`TileSet`] turns a pure tile selection into
//! content on the GPU. It is deck.gl 9.4.0's `Tileset2D` (`@deck.gl/geo-layers/dist/tileset-2d/
//! tileset-2d.js`) with loaders.gl's `RequestScheduler` and the app's `LodTileset`
//! (`src/lib/lod-tileset.ts`), ported in their order of operations; the pure parts (selection,
//! refinement, priorities) are `scene::tiles` and `scene::lod`.
//!
//! Each [`TileSet::update`] (a layer's `prepare`, once a frame):
//! 1. collects what has come back: a fetch's bytes go to the worker pool to be decoded (the
//!    layer's decode function: MVT → tessellation, an image → pixels…); a decoded tile waits for
//!    its upload;
//! 2. when the viewport has changed, selects the tiles for it with the [`Select`] strategy
//!    ([`DeckSelect`]: deck's frustum selection by the view's zoom; [`LodSelect`]: the distance
//!    LOD of doc 04 §13.2 with the look-ahead of §13.3), and creates and requests every tile it
//!    lists that is new or was cancelled (`_getTile`);
//! 3. uploads decoded tiles, nearest the view's centre first, within an [`UploadBudget`] that the
//!    frame shares between its tile sets (plan 1.4.4: 4 tiles or 8 MB a frame), through the
//!    layer's upload function;
//! 4. refines (`updateTileStates`): which cached tiles are drawn, with the [`Refinement`] strategy
//!    (deck's `best-available`, the app's `keepCoarserUntilIn`, or none). A tile is ready to stand
//!    in only once it has content: a tile that failed, or has nothing there, is not, so a loaded
//!    parent stays drawn in its place;
//! 5. prunes requests (`_pruneRequests`): while more tiles are loading than `max_requests`, those
//!    neither selected nor visible are cancelled, oldest first;
//! 6. trims the cache (`_resizeCache`) to `max_cache_size` tiles (by default five times the
//!    selection) and `max_cache_bytes`, dropping the least recently used tiles that are neither
//!    selected nor visible;
//! 7. issues requests (the scheduler's pass): the waiting requests are re-prioritised, those of
//!    tiles neither selected, visible nor ahead are dropped (cancelled), and the lowest
//!    priorities go out while fewer than `max_requests` are in flight. A request holds its slot
//!    while its bytes are fetched and decoded, as deck's `getTileData` does through parsing.
//!
//! Priorities are deck's ([`rustradar_scene::tiles::request_priority`]: selected tiles from 0,
//! visible stand-ins from 1e8, each by its outline's distance on screen from the centre) and, for
//! tiles loading ahead, `LodTileset`'s 2e8 plus that distance: lower goes first. The same numbers
//! order the decode jobs on the [`Pool`], which every tile set shares.
//!
//! Differences from deck.gl:
//! - the cache is a true LRU: past its size, the tile neither selected nor visible that was last
//!   selected, visible or ahead the longest ago goes first (deck drops in creation order);
//! - dropping a tile from the cache cancels its request (deck lets it run on, orphaned);
//! - a tile that failed or came back empty does not stand in for its ancestors (deck 9.4 counts
//!   it loaded, so it hides them and leaves a hole) — `scene::tiles::best_available`'s `ready`;
//! - decoded content waits for an upload step with a per-frame budget (deck uploads as it draws);
//! - the scheduler's pass runs once per update, not on timers; there is no `debounceTime`, and no
//!   `onTileLoad`, `onTileError` or `onViewportLoad` callbacks: a layer reads
//!   [`TileSet::frame_number`], [`TileSet::state`] and [`TileSet::is_loaded`];
//! - a tile no source serves settles empty at once, without taking a request slot (deck's
//!   `getTileData` resolves `null` from inside one);
//! - tiles loading ahead are exempt from the scheduler's drop, not from the pruning, exactly as
//!   `LodTileset` (it overrides only `_getRequestPriority`): they use spare request slots only.

use std::collections::HashMap;
use std::sync::Arc;

use rustradar_scene::camera::Viewport;
use rustradar_scene::lod::{self, AHEAD_PRIORITY, Lod, LodTileset, Selection, keep_coarser_until_in};
pub use rustradar_scene::tiles::{RefineTile, TileIndex, TilesetOptions};
use rustradar_scene::tiles::{best_available, link_tree, request_priority, tile_distance_priority, tile_indices};

use crate::io::{Fetch, FetchError, TileFetcher};
use crate::jobs::{Cancel, Job, JobPoll, Pool};

/// Requests in flight per tile set unless its spec says otherwise (plan 1.4.8).
pub const MAX_REQUESTS: usize = 8;
/// Without a size or byte limit, the cache holds this many times the selected tiles
/// (`DEFAULT_CACHE_SCALE`).
pub const DEFAULT_CACHE_SCALE: usize = 5;
/// Tiles uploaded per frame, over all tile sets (plan 1.4.4).
pub const UPLOAD_TILES_PER_FRAME: usize = 4;
/// Bytes uploaded per frame, over all tile sets (plan 1.4.4).
pub const UPLOAD_BYTES_PER_FRAME: usize = 8 << 20;

/// What every tile set fetches and decodes with: the app's [`TileFetcher`] and the worker pool.
#[derive(Clone)]
pub struct TileRuntime {
    pub fetcher: Arc<dyn TileFetcher>,
    pub pool: Arc<Pool>,
}

impl TileRuntime {
    pub fn new(fetcher: Arc<dyn TileFetcher>, pool: Arc<Pool>) -> Self {
        TileRuntime { fetcher, pool }
    }

    /// A runtime whose sources have nothing anywhere (every tile settles empty), with a pool of
    /// `threads` workers: for renderers without tiles.
    pub fn offline(threads: usize) -> Self {
        Self::new(Arc::new(crate::io::FnFetcher(|_: &str, _: TileIndex| Ok(None))), Arc::new(Pool::new(threads)))
    }
}

impl std::fmt::Debug for TileRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileRuntime").field("pool", &self.pool).finish_non_exhaustive()
    }
}

/// How much memory decoded tile content takes, for the upload budget and the cache's byte limit
/// (deck's `tile.byteLength`).
pub trait ByteSize {
    fn byte_size(&self) -> usize;
}

impl<T> ByteSize for Vec<T> {
    fn byte_size(&self) -> usize {
        self.len() * size_of::<T>()
    }
}

impl ByteSize for () {
    fn byte_size(&self) -> usize {
        0
    }
}

/// A layer's decode step, run on the worker pool: a tile's bytes to its content (`None`: nothing
/// to draw), or why it cannot be read. It may stop early once `cancel` is set.
pub type DecodeFn<D> = dyn Fn(TileIndex, Vec<u8>, &Cancel) -> Result<Option<D>, String> + Send + Sync;

// --- strategies -----------------------------------------------------------------------------------

/// How a tile set picks its tiles for a view.
pub trait Select: Send {
    /// The tiles for a view: those to load ahead (created and requested first) and those in view.
    /// `cached` says whether the tile set holds a tile already.
    fn select(&mut self, viewport: &Viewport, cached: &dyn Fn(TileIndex) -> bool) -> Selection;
    /// Whether a tile is being loaded ahead (its request is kept, after every tile in view).
    fn is_ahead(&self, _tile: TileIndex) -> bool {
        false
    }
    /// The lowest zoom a tile's ancestors are looked for at (deck's `ceil(minZoom)`, else 0).
    fn min_zoom(&self) -> u32 {
        0
    }
}

/// `ceil(min_zoom)` when it is finite, else 0 (`Tileset2D.setOptions`, `_getNearestAncestor`).
fn tree_min_zoom(min_zoom: Option<f64>) -> u32 {
    min_zoom.filter(|z| z.is_finite()).map_or(0, |z| z.ceil().max(0.0) as u32)
}

/// deck's own selection, by the view's zoom (`Tileset2D.getTileIndices`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeckSelect(pub TilesetOptions);

impl Select for DeckSelect {
    fn select(&mut self, viewport: &Viewport, _cached: &dyn Fn(TileIndex) -> bool) -> Selection {
        Selection { ahead: Vec::new(), tiles: tile_indices(viewport, &self.0) }
    }

    fn min_zoom(&self) -> u32 {
        tree_min_zoom(self.0.min_zoom)
    }
}

/// The distance LOD selection (doc 04 §13.2) with the look-ahead while zooming (§13.3):
/// `LodTileset.getTileIndices` for a layer with a `lod`.
#[derive(Debug, Clone, PartialEq)]
pub struct LodSelect {
    pub lod: Lod,
    /// The layer's tile size, pixels.
    pub tile_size: f64,
    pub zoom_offset: f64,
    /// The layer's `minZoom`: nothing is selected while the view is below it.
    pub min_zoom: Option<f64>,
    /// The lowest and highest ground in metres, for the tiles' bounding volumes.
    pub z_range: Option<[f64; 2]>,
    tileset: LodTileset,
}

impl LodSelect {
    /// `lod` for `tile_size` px tiles, with no zoom offset, minimum zoom or elevation range.
    pub fn new(lod: Lod, tile_size: f64) -> Self {
        LodSelect { lod, tile_size, zoom_offset: 0.0, min_zoom: None, z_range: None, tileset: LodTileset::new() }
    }
}

impl Select for LodSelect {
    fn select(&mut self, viewport: &Viewport, cached: &dyn Fn(TileIndex) -> bool) -> Selection {
        let opts = lod::TilesetOptions {
            lod: Some(&self.lod),
            tile_size: self.tile_size,
            zoom_offset: self.zoom_offset,
            min_zoom: self.min_zoom,
            extent: false,
        };
        self.tileset.tile_indices(viewport, &opts, self.z_range, cached).unwrap_or_else(|| {
            // (only a viewport that is not geospatial gets here: deck's own way)
            let deck = TilesetOptions {
                tile_size: self.tile_size,
                min_zoom: self.min_zoom,
                z_range: self.z_range,
                zoom_offset: self.zoom_offset,
                ..TilesetOptions::DEFAULT
            };
            Selection { ahead: Vec::new(), tiles: tile_indices(viewport, &deck) }
        })
    }

    fn is_ahead(&self, tile: TileIndex) -> bool {
        self.tileset.is_ahead(tile)
    }

    fn min_zoom(&self) -> u32 {
        tree_min_zoom(self.min_zoom)
    }
}

/// Which cached tiles are drawn (deck's `refinementStrategy`).
#[derive(Debug, Clone, Copy)]
pub enum Refinement {
    /// deck's default: while a selected tile loads, its nearest loaded ancestor, or else its
    /// loaded descendants, stand in (`scene::tiles::best_available`). Finer tiles draw over
    /// coarser ones.
    BestAvailable,
    /// The app's no-overlap rule for the realistic map: a coarser tile is drawn in place of finer
    /// ones until they are all in, never with them (`scene::lod::keep_coarser_until_in`).
    KeepCoarserUntilIn,
    /// deck's `never`: only the selected tiles, drawn once loaded.
    Never,
    /// A rule of the layer's own.
    Custom(fn(&mut [RefineTile])),
}

impl Refinement {
    fn apply(self, tiles: &mut [RefineTile]) {
        match self {
            Refinement::BestAvailable => best_available(tiles),
            Refinement::KeepCoarserUntilIn => keep_coarser_until_in(tiles),
            Refinement::Never => {}
            Refinement::Custom(refine) => refine(tiles),
        }
    }
}

/// Where a tile set's tiles come from: a source name the [`TileFetcher`] knows, for some zooms or
/// all (doc 04 §13.5: a layer's tiles can come from a different source per zoom).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub name: Arc<str>,
    /// The zooms it serves; `None`: all of them.
    pub zooms: Option<Vec<u32>>,
}

impl Source {
    /// A source for every zoom.
    pub fn new(name: &str) -> Self {
        Source { name: name.into(), zooms: None }
    }

    /// A source for these zooms only.
    pub fn at(name: &str, zooms: impl Into<Vec<u32>>) -> Self {
        Source { name: name.into(), zooms: Some(zooms.into()) }
    }

    fn serves(&self, z: u32) -> bool {
        self.zooms.as_ref().is_none_or(|zooms| zooms.contains(&z))
    }
}

/// A tile set's limits and refinement.
#[derive(Debug, Clone, Copy)]
pub struct TileSetOptions {
    /// Requests in flight at once (fetching or decoding); 0: no limit (deck's `maxRequests`).
    pub max_requests: usize,
    /// Tiles kept; `None`: [`DEFAULT_CACHE_SCALE`] times the selection, or no limit when
    /// `max_cache_bytes` is set (deck's `maxCacheSize`).
    pub max_cache_size: Option<usize>,
    /// Bytes of content kept ([`ByteSize`]); `None`: no limit (deck's `maxCacheByteSize`).
    pub max_cache_bytes: Option<usize>,
    pub refinement: Refinement,
}

impl Default for TileSetOptions {
    fn default() -> Self {
        TileSetOptions {
            max_requests: MAX_REQUESTS,
            max_cache_size: None,
            max_cache_bytes: None,
            refinement: Refinement::BestAvailable,
        }
    }
}

/// What may still be uploaded this frame: a number of tiles and of bytes. A tile larger than the
/// bytes left goes only as the frame's first upload (so no tile waits for ever).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadBudget {
    pub tiles: usize,
    pub bytes: usize,
    used: usize,
}

impl UploadBudget {
    pub const fn new(tiles: usize, bytes: usize) -> Self {
        UploadBudget { tiles, bytes, used: 0 }
    }

    /// A frame's budget: [`UPLOAD_TILES_PER_FRAME`] tiles and [`UPLOAD_BYTES_PER_FRAME`] bytes.
    pub const fn per_frame() -> Self {
        Self::new(UPLOAD_TILES_PER_FRAME, UPLOAD_BYTES_PER_FRAME)
    }

    /// No limit (the headless path).
    pub const fn unlimited() -> Self {
        Self::new(usize::MAX, usize::MAX)
    }

    /// Takes one tile of `bytes` from the budget, if it fits.
    pub fn take(&mut self, bytes: usize) -> bool {
        if self.tiles == 0 || (bytes > self.bytes && self.used > 0) {
            return false;
        }
        self.tiles -= 1;
        self.bytes = self.bytes.saturating_sub(bytes);
        self.used += 1;
        true
    }

    /// Tiles uploaded with this budget.
    pub fn used(&self) -> usize {
        self.used
    }
}

impl Default for UploadBudget {
    fn default() -> Self {
        Self::per_frame()
    }
}

// --- the tile set -----------------------------------------------------------------------------------

/// Where a tile is on its way to the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileState {
    /// Waiting for a request slot.
    Queued,
    /// Its bytes are being fetched.
    Fetching,
    /// Its bytes are being decoded on the worker pool.
    Decoding,
    /// Decoded, waiting for its upload.
    Uploading,
    /// Loaded, with content.
    Loaded,
    /// Loaded, with nothing to draw (the source has no tile there, or nothing in it).
    Empty,
    /// The request or the decoding failed ([`TileSet::error`] says why). It is not asked again
    /// while it stays cached.
    Failed,
    /// Its request was cancelled; it is requested again when it is selected again.
    Cancelled,
}

enum Load<D> {
    /// Not loading: settled, cancelled, or (only for a moment) created.
    Idle,
    Queued,
    Fetching(Fetch),
    Decoding(Job<Result<Option<D>, String>>),
    Decoded(D),
}

impl<D> Load<D> {
    /// deck's `isLoading`: waiting for a slot or holding one.
    fn is_loading(&self) -> bool {
        matches!(self, Load::Queued | Load::Fetching(_) | Load::Decoding(_))
    }

    /// Holding a request slot.
    fn is_active(&self) -> bool {
        matches!(self, Load::Fetching(_) | Load::Decoding(_))
    }
}

struct Tile<D, T> {
    index: TileIndex,
    /// Creation order (deck's cache order).
    seq: u64,
    load: Load<D>,
    content: Option<T>,
    /// [`ByteSize`] of the content.
    bytes: usize,
    /// deck's `isLoaded`: its last request has finished (with content or not).
    settled: bool,
    /// deck's `needsReload`: its request was cancelled.
    needs_reload: bool,
    error: Option<String>,
    is_selected: bool,
    is_visible: bool,
    /// The update it was last selected, visible or ahead in.
    last_used: u64,
}

/// What came back for a tile.
enum Outcome<D> {
    Bytes(Vec<u8>),
    Decoded(D),
    Settled(Option<String>),
}

/// One tile source's tiles for a layer: selection, requests, decoding, cache, refinement and
/// uploads. `D` is the decoded content (made on the worker pool), `T` what the layer's upload
/// makes of it on the main thread (GPU buffers; `D` itself for a layer that draws from the CPU).
pub struct TileSet<D, T = D> {
    runtime: TileRuntime,
    sources: Vec<Source>,
    select: Box<dyn Select>,
    decode: Arc<DecodeFn<D>>,
    options: TileSetOptions,
    /// The cache, in creation order.
    tiles: Vec<Tile<D, T>>,
    at: HashMap<TileIndex, usize>,
    /// Requests waiting for a slot, in the scheduler's order.
    queue: Vec<TileIndex>,
    selected: Vec<TileIndex>,
    viewport: Option<Viewport>,
    seq: u64,
    clock: u64,
    cache_bytes: usize,
    /// Tiles were created since the cache was last trimmed (deck's `_dirty`).
    dirty: bool,
    /// Something the refinement reads has changed.
    states_dirty: bool,
    frame_number: u64,
}

impl<D, T> std::fmt::Debug for TileSet<D, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileSet")
            .field("sources", &self.sources)
            .field("tiles", &self.tiles.len())
            .field("selected", &self.selected.len())
            .field("queued", &self.queue.len())
            .field("cache_bytes", &self.cache_bytes)
            .finish_non_exhaustive()
    }
}

impl<D: ByteSize + Send + 'static, T> TileSet<D, T> {
    /// A tile set fetching from `sources` (a tile's zoom picks the first that serves it; none: the
    /// tile settles empty) through `runtime`, selecting with `select` and decoding with `decode`
    /// on the worker pool.
    pub fn new(
        runtime: &TileRuntime,
        sources: Vec<Source>,
        select: impl Select + 'static,
        options: TileSetOptions,
        decode: impl Fn(TileIndex, Vec<u8>, &Cancel) -> Result<Option<D>, String> + Send + Sync + 'static,
    ) -> Self {
        TileSet {
            runtime: runtime.clone(),
            sources,
            select: Box::new(select),
            decode: Arc::new(decode),
            options,
            tiles: Vec::new(),
            at: HashMap::new(),
            queue: Vec::new(),
            selected: Vec::new(),
            viewport: None,
            seq: 0,
            clock: 0,
            cache_bytes: 0,
            dirty: false,
            states_dirty: false,
            frame_number: 0,
        }
    }

    /// One frame's work (the module's steps 1–7): collect, select if the viewport changed,
    /// upload within `budget` with `upload`, refine, prune, trim, request. Whether any tile's
    /// visibility changed (deck's `frameNumber` moves on then).
    pub fn update(&mut self, viewport: &Viewport, budget: &mut UploadBudget, mut upload: impl FnMut(TileIndex, D) -> T) -> bool {
        self.clock += 1;
        self.collect();
        if self.viewport.as_ref() != Some(viewport) {
            self.viewport = Some(viewport.clone());
            let at = &self.at;
            let Selection { ahead, tiles } = self.select.select(viewport, &|tile| at.contains_key(&tile));
            for index in ahead {
                self.get_tile(index);
            }
            for &index in &tiles {
                self.get_tile(index);
            }
            self.selected = tiles;
            self.mark_selected();
        } else if self.selected.iter().any(|t| self.at.get(t).is_some_and(|&i| self.tiles[i].needs_reload)) {
            // a selected tile was cancelled: ask again (deck's `needsReload` branch)
            for index in self.selected.clone() {
                self.get_tile(index);
            }
        }
        self.upload(budget, &mut upload);
        let changed = self.states_dirty && self.update_tile_states();
        self.prune_requests();
        if self.dirty || self.options.max_cache_bytes.is_some_and(|max| self.cache_bytes > max) {
            self.resize_cache();
        }
        self.issue_requests();
        // (a fetcher that answers at once: decode now, not a frame later)
        self.collect();
        if changed {
            self.frame_number += 1;
        }
        changed
    }

    /// The tiles to draw, coarse to fine (finer tiles draw over the coarser ones standing in
    /// around them), with their content.
    pub fn visible(&self) -> impl Iterator<Item = (TileIndex, &T)> {
        let mut order: Vec<&Tile<D, T>> = self.tiles.iter().filter(|t| t.is_visible && t.content.is_some()).collect();
        order.sort_by_key(|t| t.index.z);
        order.into_iter().filter_map(|t| Some((t.index, t.content.as_ref()?)))
    }

    /// A cached tile's content.
    pub fn get(&self, index: TileIndex) -> Option<&T> {
        self.tile(index)?.content.as_ref()
    }

    /// Where a cached tile is; `None` when it is not cached.
    pub fn state(&self, index: TileIndex) -> Option<TileState> {
        let tile = self.tile(index)?;
        Some(match tile.load {
            Load::Queued => TileState::Queued,
            Load::Fetching(_) => TileState::Fetching,
            Load::Decoding(_) => TileState::Decoding,
            Load::Decoded(_) => TileState::Uploading,
            Load::Idle if tile.needs_reload || !tile.settled => TileState::Cancelled,
            Load::Idle if tile.error.is_some() => TileState::Failed,
            Load::Idle if tile.content.is_some() => TileState::Loaded,
            Load::Idle => TileState::Empty,
        })
    }

    /// Why a cached tile failed.
    pub fn error(&self, index: TileIndex) -> Option<&str> {
        self.tile(index)?.error.as_deref()
    }

    /// The tiles selected for the last viewport.
    pub fn selected(&self) -> &[TileIndex] {
        &self.selected
    }

    pub fn is_selected(&self, index: TileIndex) -> bool {
        self.tile(index).is_some_and(|t| t.is_selected)
    }

    /// Whether the refinement draws a tile (it may still lack content with [`Refinement::Never`]).
    pub fn is_visible(&self, index: TileIndex) -> bool {
        self.tile(index).is_some_and(|t| t.is_visible)
    }

    /// Whether the selection loads a tile ahead.
    pub fn is_ahead(&self, index: TileIndex) -> bool {
        self.select.is_ahead(index)
    }

    /// A cached tile's request priority now (lower goes first); `None` when nothing wants it, so a
    /// request of its that waits is dropped.
    pub fn priority(&self, index: TileIndex) -> Option<f64> {
        self.at.get(&index).and_then(|&i| self.priority_of(i))
    }

    /// deck's `isLoaded`: there is a selection and every selected tile has settled (uploaded, or
    /// with nothing to draw).
    pub fn is_loaded(&self) -> bool {
        self.viewport.is_some() && self.selected.iter().all(|t| self.tile(*t).is_some_and(|t| t.settled && !t.needs_reload))
    }

    /// Whether any tile is still on its way (queued, fetching, decoding or waiting for its upload):
    /// the headless path renders until no layer has one.
    pub fn pending(&self) -> bool {
        self.tiles.iter().any(|t| !matches!(t.load, Load::Idle))
    }

    /// Requests holding a slot (fetching or decoding).
    pub fn in_flight(&self) -> usize {
        self.tiles.iter().filter(|t| t.load.is_active()).count()
    }

    /// Requests waiting for a slot.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Tiles cached, in whatever state.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// The cached tiles, oldest first.
    pub fn cached(&self) -> impl Iterator<Item = TileIndex> + '_ {
        self.tiles.iter().map(|t| t.index)
    }

    /// [`ByteSize`] of all the content cached.
    pub fn cache_bytes(&self) -> usize {
        self.cache_bytes
    }

    /// Moves on whenever an update changes which tiles are visible.
    pub fn frame_number(&self) -> u64 {
        self.frame_number
    }

    pub fn options(&self) -> &TileSetOptions {
        &self.options
    }

    /// New limits or refinement; the next update selects again (`setOptions`).
    pub fn set_options(&mut self, options: TileSetOptions) {
        self.options = options;
        self.reselect();
    }

    /// The selection strategy, to change its options (call [`Self::reselect`] after).
    pub fn select_mut(&mut self) -> &mut dyn Select {
        &mut *self.select
    }

    /// The next update selects again, even for the same viewport.
    pub fn reselect(&mut self) {
        self.viewport = None;
    }

    /// deck's `reloadAll`: drops every tile not selected and requests the selected ones again
    /// (their old content is drawn until the new is in).
    pub fn reload_all(&mut self) {
        let keep: Vec<bool> = self.tiles.iter().map(|t| t.is_selected).collect();
        self.remove(&keep);
        for tile in &mut self.tiles {
            // setNeedsReload: a load under way is abandoned
            tile.load = Load::Idle;
            tile.needs_reload = true;
        }
        self.queue.clear();
        self.states_dirty = true;
    }

    /// Drops everything, cancelling every request (`finalize`).
    pub fn clear(&mut self) {
        self.tiles.clear();
        self.at.clear();
        self.queue.clear();
        self.selected.clear();
        self.viewport = None;
        self.cache_bytes = 0;
        self.dirty = false;
        self.states_dirty = false;
    }

    fn tile(&self, index: TileIndex) -> Option<&Tile<D, T>> {
        self.at.get(&index).map(|&i| &self.tiles[i])
    }

    /// `_getTile(index, true)`: the tile, created and requested if it is new, requested again if
    /// it was cancelled.
    fn get_tile(&mut self, index: TileIndex) {
        let i = match self.at.get(&index) {
            Some(&i) => {
                if !self.tiles[i].needs_reload {
                    return;
                }
                i
            }
            None => {
                let i = self.tiles.len();
                self.tiles.push(Tile {
                    index,
                    seq: self.seq,
                    load: Load::Idle,
                    content: None,
                    bytes: 0,
                    settled: false,
                    needs_reload: true,
                    error: None,
                    is_selected: false,
                    is_visible: false,
                    last_used: self.clock,
                });
                self.seq += 1;
                self.at.insert(index, i);
                self.dirty = true;
                self.states_dirty = true;
                i
            }
        };
        self.request(i);
    }

    /// `loadData`: queues the tile's request (a tile no source serves settles empty at once).
    fn request(&mut self, i: usize) {
        let tile = &mut self.tiles[i];
        if !matches!(tile.load, Load::Idle) {
            return;
        }
        tile.needs_reload = false;
        tile.settled = false;
        tile.error = None;
        if self.sources.iter().any(|s| s.serves(tile.index.z)) {
            tile.load = Load::Queued;
            self.queue.push(tile.index);
        } else {
            self.settle(i, None, 0, None);
        }
    }

    /// The tile's request has finished: `content` (or none), with `error` if it failed.
    fn settle(&mut self, i: usize, content: Option<T>, bytes: usize, error: Option<String>) {
        let tile = &mut self.tiles[i];
        self.cache_bytes = self.cache_bytes - tile.bytes + bytes;
        tile.content = content;
        tile.bytes = bytes;
        tile.error = error;
        tile.settled = true;
        tile.load = Load::Idle;
        self.states_dirty = true;
    }

    /// The request was cancelled (pruned, or dropped by the scheduler): asked again when selected.
    fn cancelled(&mut self, i: usize) {
        let tile = &mut self.tiles[i];
        tile.load = Load::Idle;
        tile.needs_reload = true;
        tile.settled = false;
    }

    fn mark_selected(&mut self) {
        for tile in &mut self.tiles {
            tile.is_selected = false;
        }
        for index in &self.selected {
            if let Some(&i) = self.at.get(index) {
                self.tiles[i].is_selected = true;
            }
        }
        self.states_dirty = true;
    }

    /// `_getRequestPriority`, with `LodTileset`'s for tiles loading ahead.
    fn priority_of(&self, i: usize) -> Option<f64> {
        let tile = &self.tiles[i];
        let viewport = self.viewport.as_ref()?;
        if !tile.is_selected && !tile.is_visible && self.select.is_ahead(tile.index) {
            return Some(AHEAD_PRIORITY + tile_distance_priority(viewport, tile.index));
        }
        request_priority(viewport, tile.index, tile.is_selected, tile.is_visible)
    }

    /// Step 1: answers to fetches go to be decoded; decoded tiles wait for their upload.
    fn collect(&mut self) {
        for i in 0..self.tiles.len() {
            let outcome = match &mut self.tiles[i].load {
                Load::Fetching(fetch) => match fetch.poll() {
                    None => continue,
                    Some(Ok(Some(bytes))) => Outcome::Bytes(bytes),
                    Some(Ok(None)) => Outcome::Settled(None),
                    Some(Err(FetchError::Failed(why))) => Outcome::Settled(Some(why)),
                    // (a request this set cancels is dropped, never polled: the fetcher gave up on
                    // this one, which deck counts as an error)
                    Some(Err(FetchError::Cancelled)) => Outcome::Settled(Some("the request was dropped".into())),
                },
                Load::Decoding(job) => match job.poll() {
                    JobPoll::Pending => continue,
                    JobPoll::Done(Ok(Some(decoded))) => Outcome::Decoded(decoded),
                    JobPoll::Done(Ok(None)) => Outcome::Settled(None),
                    JobPoll::Done(Err(why)) => Outcome::Settled(Some(why)),
                    JobPoll::Cancelled | JobPoll::Lost => Outcome::Settled(Some("decoding the tile panicked".into())),
                },
                _ => continue,
            };
            match outcome {
                Outcome::Bytes(bytes) => {
                    let (index, decode) = (self.tiles[i].index, Arc::clone(&self.decode));
                    let priority = self.priority_of(i).unwrap_or(f64::MAX);
                    let job = self.runtime.pool.spawn(priority, move |cancel| decode(index, bytes, cancel));
                    self.tiles[i].load = Load::Decoding(job);
                }
                Outcome::Decoded(decoded) => self.tiles[i].load = Load::Decoded(decoded),
                Outcome::Settled(error) => self.settle(i, None, 0, error),
            }
        }
    }

    /// Step 3: decoded tiles to content, by priority, while the budget lasts.
    fn upload(&mut self, budget: &mut UploadBudget, upload: &mut impl FnMut(TileIndex, D) -> T) {
        let mut waiting: Vec<(f64, u64, usize)> = (0..self.tiles.len())
            .filter(|&i| matches!(self.tiles[i].load, Load::Decoded(_)))
            .map(|i| (self.priority_of(i).unwrap_or(f64::INFINITY), self.tiles[i].seq, i))
            .collect();
        waiting.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (_, _, i) in waiting {
            let Load::Decoded(decoded) = &self.tiles[i].load else { continue };
            let bytes = decoded.byte_size();
            if !budget.take(bytes) {
                break;
            }
            let Load::Decoded(decoded) = std::mem::replace(&mut self.tiles[i].load, Load::Idle) else { continue };
            let content = upload(self.tiles[i].index, decoded);
            self.settle(i, Some(content), bytes, None);
        }
    }

    /// Step 4, `updateTileStates`: selected tiles start visible, then the refinement decides.
    /// Whether any tile's visibility changed.
    fn update_tile_states(&mut self) -> bool {
        self.states_dirty = false;
        let before: Vec<bool> = self.tiles.iter().map(|t| t.is_visible).collect();
        for tile in &mut self.tiles {
            tile.is_selected = false;
            tile.is_visible = false;
        }
        for index in &self.selected {
            if let Some(&i) = self.at.get(index) {
                self.tiles[i].is_selected = true;
                self.tiles[i].is_visible = true;
            }
        }
        let mut refine: Vec<RefineTile> = self
            .tiles
            .iter()
            .map(|t| RefineTile {
                is_selected: t.is_selected,
                is_visible: t.is_visible,
                zoom: t.index.z,
                ready: t.content.is_some(),
                ..RefineTile::default()
            })
            .collect();
        let indices: Vec<TileIndex> = self.tiles.iter().map(|t| t.index).collect();
        link_tree(&mut refine, &indices, self.select.min_zoom());
        self.options.refinement.apply(&mut refine);
        let mut changed = false;
        for (i, r) in refine.iter().enumerate() {
            let tile = &mut self.tiles[i];
            tile.is_visible = r.is_visible;
            changed |= tile.is_visible != before[i];
            if tile.is_selected || tile.is_visible || self.select.is_ahead(tile.index) {
                tile.last_used = self.clock;
            }
        }
        changed
    }

    /// Step 5, `_pruneRequests`: while more tiles load than `max_requests`, cancel those neither
    /// selected nor visible, oldest first.
    fn prune_requests(&mut self) {
        let max = self.options.max_requests;
        if max == 0 {
            return;
        }
        let mut ongoing = self.tiles.iter().filter(|t| t.load.is_loading()).count();
        let candidates: Vec<usize> =
            (0..self.tiles.len()).filter(|&i| self.tiles[i].load.is_loading() && !self.tiles[i].is_selected && !self.tiles[i].is_visible).collect();
        for i in candidates {
            if ongoing <= max {
                break;
            }
            // abort(): a fetch or a decode under way is cancelled as its handle drops
            let index = self.tiles[i].index;
            self.queue.retain(|&q| q != index);
            self.cancelled(i);
            ongoing -= 1;
        }
    }

    /// Step 6, `_resizeCache`: past the size or byte limit, the least recently used tiles that are
    /// neither selected nor visible go.
    fn resize_cache(&mut self) {
        self.dirty = false;
        let max_size = self.options.max_cache_size.unwrap_or(if self.options.max_cache_bytes.is_some() {
            usize::MAX
        } else {
            DEFAULT_CACHE_SCALE * self.selected.len()
        });
        let max_bytes = self.options.max_cache_bytes.unwrap_or(usize::MAX);
        if self.tiles.len() <= max_size && self.cache_bytes <= max_bytes {
            return;
        }
        let mut order: Vec<usize> = (0..self.tiles.len()).collect();
        order.sort_by_key(|&i| (self.tiles[i].last_used, self.tiles[i].seq));
        let mut keep = vec![true; self.tiles.len()];
        let (mut size, mut bytes) = (self.tiles.len(), self.cache_bytes);
        for i in order {
            let tile = &self.tiles[i];
            if !tile.is_visible && !tile.is_selected {
                keep[i] = false;
                size -= 1;
                bytes -= tile.bytes;
            }
            if size <= max_size && bytes <= max_bytes {
                break;
            }
        }
        self.remove(&keep);
    }

    /// Drops the tiles whose `keep` is false (their requests are cancelled as they drop).
    fn remove(&mut self, keep: &[bool]) {
        let mut i = 0;
        self.tiles.retain(|_| {
            i += 1;
            keep[i - 1]
        });
        self.at = self.tiles.iter().enumerate().map(|(i, t)| (t.index, i)).collect();
        self.queue.retain(|q| self.at.contains_key(q));
        self.cache_bytes = self.tiles.iter().map(|t| t.bytes).sum();
        self.states_dirty = true;
    }

    /// Step 7, the scheduler's pass: with a slot free, the waiting requests get their priorities
    /// again (none: dropped), and the lowest go out.
    fn issue_requests(&mut self) {
        let max = self.options.max_requests;
        let active = self.tiles.iter().filter(|t| t.load.is_active()).count();
        let free = if max == 0 { usize::MAX } else { max.saturating_sub(active) };
        if free == 0 || self.queue.is_empty() {
            return;
        }
        let mut waiting: Vec<(f64, usize)> = Vec::with_capacity(self.queue.len());
        for index in std::mem::take(&mut self.queue) {
            let Some(&i) = self.at.get(&index) else { continue };
            if !matches!(self.tiles[i].load, Load::Queued) {
                continue;
            }
            match self.priority_of(i) {
                Some(priority) => waiting.push((priority, i)),
                // a priority below 0 cancels the request
                None => self.cancelled(i),
            }
        }
        // (stable: equal priorities keep their order)
        waiting.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (n, &(_, i)) in waiting.iter().enumerate() {
            let index = self.tiles[i].index;
            if n >= free {
                self.queue.push(index);
                continue;
            }
            let Some(source) = self.sources.iter().find(|s| s.serves(index.z)) else {
                self.settle(i, None, 0, None);
                continue;
            };
            let fetch = Fetch::start(&*self.runtime.fetcher, Arc::clone(&source.name), index);
            self.tiles[i].load = Load::Fetching(fetch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{FetchRequest, FetchResult, FnFetcher};
    use rustradar_scene::camera::{ViewState, viewport_for};
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::mpsc::{self, Receiver, Sender};

    // --- the fakes ------------------------------------------------------------------------------------

    struct Asked {
        source: String,
        tile: TileIndex,
        cancel: Cancel,
        answer: Option<Sender<FetchResult>>,
    }

    /// A fetcher that holds every request until the test answers it.
    #[derive(Default)]
    struct Fake(Mutex<Vec<Asked>>);

    impl TileFetcher for Fake {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            let (tx, rx) = mpsc::channel();
            let FetchRequest { source, tile, cancel } = request;
            self.0.lock().unwrap().push(Asked { source: source.to_string(), tile, cancel, answer: Some(tx) });
            rx
        }
    }

    impl Fake {
        /// Every tile asked for, in order (a tile asked twice is listed twice).
        fn asked(&self) -> Vec<TileIndex> {
            self.0.lock().unwrap().iter().map(|a| a.tile).collect()
        }

        fn sources(&self) -> Vec<(String, TileIndex)> {
            self.0.lock().unwrap().iter().map(|a| (a.source.clone(), a.tile)).collect()
        }

        /// Requests not answered and not cancelled.
        fn outstanding(&self) -> Vec<TileIndex> {
            self.0.lock().unwrap().iter().filter(|a| a.answer.is_some() && !a.cancel.is_cancelled()).map(|a| a.tile).collect()
        }

        /// Whether the last request for `tile` was cancelled.
        fn cancelled(&self, tile: TileIndex) -> bool {
            self.0.lock().unwrap().iter().rev().find(|a| a.tile == tile).is_some_and(|a| a.cancel.is_cancelled())
        }

        /// Answers the last request for `tile`.
        fn answer(&self, tile: TileIndex, result: FetchResult) {
            let mut asked = self.0.lock().unwrap();
            let request = asked.iter_mut().rev().find(|a| a.tile == tile && a.answer.is_some()).expect("an open request");
            // (a cancelled request's receiver may be gone)
            let _ = request.answer.take().unwrap().send(result);
        }

        fn answer_text(&self, tile: TileIndex, text: &str) {
            self.answer(tile, Ok(Some(text.as_bytes().to_vec())));
        }

        /// Gives up on the last request for `tile` without answering.
        fn abandon(&self, tile: TileIndex) {
            let mut asked = self.0.lock().unwrap();
            asked.iter_mut().rev().find(|a| a.tile == tile && a.answer.is_some()).expect("an open request").answer = None;
        }
    }

    /// Decoded test content: its text, and how big it claims to be (`size:N`, else its length).
    #[derive(Debug, Clone, PartialEq)]
    struct Payload {
        text: String,
        size: usize,
    }

    impl ByteSize for Payload {
        fn byte_size(&self) -> usize {
            self.size
        }
    }

    fn decode(_tile: TileIndex, bytes: Vec<u8>, _cancel: &Cancel) -> Result<Option<Payload>, String> {
        let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        match text.as_str() {
            "broken" => Err("not a tile".into()),
            "nothing" => Ok(None),
            _ => {
                let size = text.strip_prefix("size:").map_or(text.len(), |n| n.parse().unwrap());
                Ok(Some(Payload { text, size }))
            }
        }
    }

    /// A selection the test sets by hand: the tiles in view and those ahead.
    #[derive(Clone, Default)]
    struct Fixed(Arc<Mutex<(Vec<TileIndex>, Vec<TileIndex>)>>);

    impl Select for Fixed {
        fn select(&mut self, _viewport: &Viewport, _cached: &dyn Fn(TileIndex) -> bool) -> Selection {
            let (tiles, ahead) = self.0.lock().unwrap().clone();
            Selection { ahead, tiles }
        }

        fn is_ahead(&self, tile: TileIndex) -> bool {
            self.0.lock().unwrap().1.contains(&tile)
        }
    }

    fn view(longitude: f64, latitude: f64, zoom: f64) -> ViewState {
        ViewState { longitude, latitude, zoom, bearing: 0.0, pitch: 0.0, position: None }
    }

    struct Harness {
        fake: Arc<Fake>,
        pool: Arc<Pool>,
        set: TileSet<Payload, String>,
        chosen: Fixed,
        viewport: Viewport,
        uploads: Vec<TileIndex>,
    }

    fn harness(options: TileSetOptions) -> Harness {
        let chosen = Fixed::default();
        let mut h = harness_with(chosen.clone(), options);
        h.chosen = chosen;
        h
    }

    fn harness_with(select: impl Select + 'static, options: TileSetOptions) -> Harness {
        let fake = Arc::new(Fake::default());
        let pool = Arc::new(Pool::new(2));
        let runtime = TileRuntime { fetcher: fake.clone(), pool: Arc::clone(&pool) };
        let set = TileSet::new(&runtime, vec![Source::new("test")], select, options, decode);
        // Europe at zoom 4.5: the tiles the tests pick lie around the centre
        let viewport = viewport_for(&view(10.0, 52.0, 4.5), [800.0, 600.0]);
        Harness { fake, pool, set, chosen: Fixed::default(), viewport, uploads: Vec::new() }
    }

    impl Harness {
        /// The tiles in view from the next update on.
        fn choose(&mut self, tiles: &[TileIndex]) {
            self.choose_with_ahead(tiles, &[]);
        }

        fn choose_with_ahead(&mut self, tiles: &[TileIndex], ahead: &[TileIndex]) {
            *self.chosen.0.lock().unwrap() = (tiles.to_vec(), ahead.to_vec());
            self.set.reselect();
        }

        fn update_with(&mut self, budget: &mut UploadBudget) -> bool {
            let uploads = &mut self.uploads;
            self.set.update(&self.viewport, budget, |tile, payload| {
                uploads.push(tile);
                payload.text
            })
        }

        fn update(&mut self) -> bool {
            self.update_with(&mut UploadBudget::unlimited())
        }

        /// Updates until what has come back is decoded and uploaded.
        fn settle(&mut self) {
            for _ in 0..3 {
                self.update();
                self.pool.wait_idle();
            }
            self.update();
        }

        /// Updates, then answers every open request with `ok` until nothing is pending.
        fn load_all(&mut self) {
            self.update();
            for _ in 0..100 {
                if !self.set.pending() {
                    return;
                }
                for tile in self.fake.outstanding() {
                    self.fake.answer_text(tile, "ok");
                }
                self.settle();
            }
            panic!("still pending");
        }

        fn visible(&self) -> Vec<TileIndex> {
            self.set.visible().map(|(t, _)| t).collect()
        }
    }

    /// The tile at `z` holding `[lon, lat]`.
    fn tile_at(lon: f64, lat: f64, z: u32) -> TileIndex {
        let n = f64::from(1u32 << z);
        let x = ((lon + 180.0) / 360.0 * n).floor() as u32;
        let phi = lat.to_radians();
        let y = ((1.0 - (phi.tan() + 1.0 / phi.cos()).ln() / std::f64::consts::PI) / 2.0 * n).floor() as u32;
        TileIndex::new(x, y, z)
    }

    /// `n` tiles at zoom 6 in a row through the view's centre.
    fn row(n: u32) -> Vec<TileIndex> {
        let centre = tile_at(10.0, 52.0, 6);
        (0..n).map(|i| TileIndex::new(centre.x - n / 2 + i, centre.y, 6)).collect()
    }

    // --- requests -----------------------------------------------------------------------------------

    #[test]
    fn at_most_max_requests_are_in_flight() {
        let mut h = harness(TileSetOptions { max_requests: 3, ..Default::default() });
        let tiles = row(10);
        h.choose(&tiles);
        h.update();
        assert_eq!(h.fake.asked().len(), 3);
        assert_eq!((h.set.in_flight(), h.set.queued()), (3, 7));
        // nothing more goes out while the three are open
        h.update();
        assert_eq!(h.fake.asked().len(), 3);
        let mut rounds = 0;
        while h.set.pending() {
            for tile in h.fake.outstanding() {
                h.fake.answer_text(tile, "ok");
                // a slot is held through the decoding too
                h.update();
                assert!(h.set.in_flight() <= 3, "{} in flight", h.set.in_flight());
                assert!(h.fake.outstanding().len() <= 3);
            }
            h.pool.wait_idle();
            h.update();
            assert!(h.set.in_flight() <= 3, "{} in flight", h.set.in_flight());
            assert!(h.fake.outstanding().len() <= 3);
            rounds += 1;
            assert!(rounds < 50);
        }
        let mut asked = h.fake.asked();
        asked.sort_by_key(|t| t.x);
        assert_eq!(asked, tiles, "each tile asked once");
        assert!(tiles.iter().all(|&t| h.set.state(t) == Some(TileState::Loaded)));
        assert!(h.set.is_loaded());
    }

    #[test]
    fn requests_go_out_nearest_the_view_centre_first() {
        let options = TileSetOptions { max_requests: 1, ..Default::default() };
        let mut h = harness_with(DeckSelect(rustradar_scene::tiles::VECTOR_TILES), options);
        h.update();
        let selected = h.set.selected().to_vec();
        assert!(selected.len() > 4);
        h.load_all();
        let asked = h.fake.asked();
        assert_eq!(asked.len(), selected.len());
        // the tile under the centre first, then outwards
        assert_eq!(asked[0], tile_at(10.0, 52.0, 5));
        let distances: Vec<f64> = asked.iter().map(|&t| tile_distance_priority(&h.viewport, t)).collect();
        assert!(distances.windows(2).all(|w| w[0] <= w[1]), "{distances:?}");
    }

    #[test]
    fn no_request_limit_sends_every_request_at_once() {
        let mut h = harness(TileSetOptions { max_requests: 0, ..Default::default() });
        h.choose(&row(12));
        h.update();
        assert_eq!(h.fake.asked().len(), 12);
        assert_eq!(h.set.queued(), 0);
    }

    #[test]
    fn a_tile_that_leaves_the_selection_is_cancelled() {
        let mut h = harness(TileSetOptions { max_requests: 2, ..Default::default() });
        let tiles = row(8);
        let (before, after) = tiles.split_at(4);
        h.choose(before);
        h.update();
        let in_flight = h.fake.asked();
        assert_eq!(in_flight.len(), 2);
        h.choose(after);
        h.update();
        for &tile in before {
            assert_eq!(h.set.state(tile), Some(TileState::Cancelled), "{tile:?}");
            if in_flight.contains(&tile) {
                // the fetcher is told nobody waits for it
                assert!(h.fake.cancelled(tile), "{tile:?}");
            }
        }
        // the two that waited were never asked; the slots went to the new view's tiles
        let asked = h.fake.asked();
        assert_eq!(asked.len(), 4);
        assert!(asked[2..].iter().all(|t| after.contains(t)));
        assert_eq!(h.set.in_flight(), 2);
        // an answer that comes anyway changes nothing
        h.fake.answer_text(in_flight[0], "late");
        h.settle();
        assert_eq!(h.set.state(in_flight[0]), Some(TileState::Cancelled));
    }

    #[test]
    fn a_cancelled_tile_is_asked_again_when_it_is_selected_again() {
        let mut h = harness(TileSetOptions { max_requests: 1, ..Default::default() });
        let tiles = row(3);
        h.choose(&tiles[..1]);
        h.update();
        h.choose(&tiles[1..2]);
        h.update();
        assert_eq!(h.set.state(tiles[0]), Some(TileState::Cancelled));
        h.choose(&tiles[..1]);
        h.update();
        assert_eq!(h.fake.asked(), [tiles[0], tiles[1], tiles[0]]);
        assert!(h.fake.cancelled(tiles[1]));
        h.load_all();
        assert_eq!(h.set.state(tiles[0]), Some(TileState::Loaded));
        assert_eq!(h.visible(), [tiles[0]]);
    }

    #[test]
    fn an_unselected_tile_in_flight_finishes_when_nothing_waits_for_its_slot() {
        let mut h = harness(TileSetOptions { max_requests: 4, ..Default::default() });
        let tiles = row(3);
        h.choose(&tiles[..2]);
        h.update();
        h.choose(&tiles[2..]);
        h.update();
        // three loading, four slots: nothing is pruned
        assert!(!h.fake.cancelled(tiles[0]) && !h.fake.cancelled(tiles[1]));
        h.fake.answer_text(tiles[0], "ok");
        h.settle();
        assert_eq!(h.set.state(tiles[0]), Some(TileState::Loaded));
        assert!(!h.set.is_visible(tiles[0]));
        assert_eq!(h.set.state(tiles[1]), Some(TileState::Fetching));
    }

    // --- refinement --------------------------------------------------------------------------------

    #[test]
    fn a_failed_tile_leaves_its_loaded_parent_visible() {
        for refinement in [Refinement::BestAvailable, Refinement::KeepCoarserUntilIn] {
            let mut h = harness(TileSetOptions { refinement, ..Default::default() });
            let parent = tile_at(10.0, 52.0, 5);
            h.choose(&[parent]);
            h.load_all();
            assert_eq!(h.visible(), [parent]);
            let children = parent.children();
            h.choose(&children);
            h.update();
            // while the children load, the parent stands in
            assert_eq!(h.visible(), [parent], "{refinement:?}");
            assert!(h.set.is_visible(parent));
            h.fake.answer_text(children[0], "ok");
            h.fake.answer_text(children[1], "ok");
            h.fake.answer(children[2], Err(FetchError::Failed("HTTP 500".into())));
            h.fake.answer_text(children[3], "ok");
            h.settle();
            assert_eq!(h.set.state(children[2]), Some(TileState::Failed));
            assert_eq!(h.set.error(children[2]), Some("HTTP 500"));
            assert!(h.set.is_loaded());
            match refinement {
                // the parent under the hole, the three children over it
                Refinement::BestAvailable => assert_eq!(h.visible(), [parent, children[0], children[1], children[3]]),
                // never both: the parent stays in place of all four
                _ => assert_eq!(h.visible(), [parent]),
            }
            // it is not asked again, and the parent is kept
            h.settle();
            assert_eq!(h.fake.asked().iter().filter(|&&t| t == children[2]).count(), 1);
            assert!(h.set.is_visible(parent));
        }
    }

    #[test]
    fn an_empty_or_unreadable_tile_leaves_its_parent_visible_too() {
        let mut h = harness(TileSetOptions::default());
        let parent = tile_at(10.0, 52.0, 5);
        h.choose(&[parent]);
        h.load_all();
        let children = parent.children();
        h.choose(&children);
        h.update();
        h.fake.answer(children[0], Ok(None)); // 404
        h.fake.answer_text(children[1], "nothing"); // decodes to nothing
        h.fake.answer_text(children[2], "broken"); // does not decode
        h.fake.answer_text(children[3], "ok");
        h.settle();
        assert_eq!(h.set.state(children[0]), Some(TileState::Empty));
        assert_eq!(h.set.state(children[1]), Some(TileState::Empty));
        assert_eq!(h.set.state(children[2]), Some(TileState::Failed));
        assert_eq!(h.set.error(children[2]), Some("not a tile"));
        assert_eq!(h.visible(), [parent, children[3]]);
    }

    #[test]
    fn loaded_children_stand_in_for_a_parent_that_loads() {
        let mut h = harness(TileSetOptions::default());
        let parent = tile_at(10.0, 52.0, 5);
        let children = parent.children();
        h.choose(&children);
        h.load_all();
        h.choose(&[parent]);
        h.update();
        assert_eq!(h.visible(), children);
        h.load_all();
        assert_eq!(h.visible(), [parent]);
    }

    #[test]
    fn never_draws_only_the_selected_tiles() {
        let mut h = harness(TileSetOptions { refinement: Refinement::Never, ..Default::default() });
        let parent = tile_at(10.0, 52.0, 5);
        h.choose(&[parent]);
        h.load_all();
        let children = parent.children();
        h.choose(&children);
        h.update();
        assert!(h.visible().is_empty());
        assert!(children.iter().all(|&c| h.set.is_visible(c)));
        h.fake.answer_text(children[1], "ok");
        h.settle();
        assert_eq!(h.visible(), [children[1]]);
    }

    #[test]
    fn a_tile_no_source_serves_settles_empty_without_a_request() {
        let fake = Arc::new(Fake::default());
        let pool = Arc::new(Pool::new(1));
        let runtime = TileRuntime { fetcher: fake.clone(), pool: Arc::clone(&pool) };
        let chosen = Fixed::default();
        let sources = [Source::at("coarse", [3, 4]), Source::at("fine", [5]), Source::new("never reached")];
        let mut set: TileSet<Payload> = TileSet::new(&runtime, sources[..2].to_vec(), chosen.clone(), TileSetOptions::default(), decode);
        let tiles = [tile_at(10.0, 52.0, 4), tile_at(10.0, 52.0, 5), tile_at(10.0, 52.0, 6)];
        chosen.0.lock().unwrap().0 = tiles.to_vec();
        let viewport = viewport_for(&view(10.0, 52.0, 4.5), [800.0, 600.0]);
        set.update(&viewport, &mut UploadBudget::unlimited(), |_, d| d);
        let mut asked = fake.sources();
        asked.sort_by_key(|(_, t)| t.z);
        assert_eq!(asked, [("coarse".to_string(), tiles[0]), ("fine".to_string(), tiles[1])]);
        assert_eq!(set.state(tiles[2]), Some(TileState::Empty));
        assert!(Source::new("never reached").serves(6) && !sources[0].serves(5));
    }

    #[test]
    fn a_request_the_fetcher_gives_up_on_fails_and_is_not_asked_again() {
        let mut h = harness(TileSetOptions::default());
        let tile = tile_at(10.0, 52.0, 5);
        h.choose(&[tile]);
        h.update();
        h.fake.abandon(tile);
        h.settle();
        assert_eq!(h.set.state(tile), Some(TileState::Failed));
        assert_eq!(h.set.error(tile), Some("the request was dropped"));
        assert_eq!(h.fake.asked(), [tile]);
        assert!(h.set.is_loaded() && !h.set.pending());
    }

    #[test]
    fn an_offline_runtime_settles_every_tile_empty() {
        let runtime = TileRuntime::offline(1);
        let mut set: TileSet<Vec<u8>> =
            TileSet::new(&runtime, vec![Source::new("any")], DeckSelect(rustradar_scene::tiles::VECTOR_TILES), TileSetOptions::default(), |_, b, _| Ok(Some(b)));
        let viewport = viewport_for(&ViewState::INITIAL, [1100.0, 720.0]);
        for _ in 0..2 {
            set.update(&viewport, &mut UploadBudget::unlimited(), |_, d| d);
        }
        assert!(!set.selected().is_empty());
        assert!(set.selected().iter().all(|&t| set.state(t) == Some(TileState::Empty)));
        assert!(set.is_loaded() && !set.pending());
        assert_eq!(set.visible().count(), 0);
    }

    // --- the cache ---------------------------------------------------------------------------------

    #[test]
    fn the_lru_evicts_the_oldest_unselected_tile() {
        let mut h = harness(TileSetOptions { max_cache_size: Some(3), ..Default::default() });
        let [a, b, c, d, e]: [TileIndex; 5] = row(5).try_into().unwrap();
        for tile in [a, b, c] {
            h.choose(&[tile]);
            h.load_all();
        }
        assert_eq!(h.set.cached().collect::<Vec<_>>(), [a, b, c]);
        // a fourth: the one used longest ago goes
        h.choose(&[d]);
        h.update();
        assert_eq!(h.set.cached().collect::<Vec<_>>(), [b, c, d]);
        h.load_all();
        // b is used again, so c is now the oldest (deck would drop b, the oldest created)
        h.choose(&[b]);
        h.update();
        h.choose(&[e]);
        h.update();
        assert_eq!(h.set.cached().collect::<Vec<_>>(), [b, d, e]);
        assert!(h.set.get(b).is_some());
    }

    #[test]
    fn selected_and_visible_tiles_are_never_evicted() {
        let mut h = harness(TileSetOptions { max_cache_size: Some(1), ..Default::default() });
        let parent = tile_at(10.0, 52.0, 5);
        h.choose(&[parent]);
        h.load_all();
        let children = parent.children();
        h.choose(&children);
        h.update();
        // four selected and the parent standing in for them: all kept, over the limit
        assert_eq!(h.set.len(), 5);
        h.load_all();
        // once the children are in, the parent is neither selected nor visible: it goes at the
        // next trim (when a tile is created)
        h.choose(&[children[0], children[1], children[2], children[3], tile_at(30.0, 52.0, 6)]);
        h.update();
        assert_eq!(h.set.state(parent), None);
        assert_eq!(h.set.len(), 5);
    }

    #[test]
    fn the_cache_holds_five_times_the_selection_by_default() {
        let mut h = harness(TileSetOptions::default());
        let tiles = row(8);
        for &tile in &tiles {
            h.choose(&[tile]);
            h.load_all();
            assert!(h.set.len() <= 5);
        }
        assert_eq!(h.set.cached().collect::<Vec<_>>(), tiles[3..]);
    }

    #[test]
    fn a_byte_limit_evicts_by_size() {
        let mut h = harness(TileSetOptions { max_cache_bytes: Some(250), ..Default::default() });
        let tiles = row(4);
        for &tile in &tiles[..3] {
            h.choose(&[tile]);
            h.update();
            h.fake.answer_text(tile, "size:100");
            h.settle();
        }
        // a third 100-byte tile is over 250: the oldest unused goes
        assert_eq!(h.set.cached().collect::<Vec<_>>(), tiles[1..3]);
        assert_eq!(h.set.cache_bytes(), 200);
        // with only a byte limit, the count is free
        h.choose(&tiles[..1]);
        h.update();
        h.fake.answer_text(tiles[0], "size:10");
        h.settle();
        assert_eq!(h.set.len(), 3);
        assert_eq!(h.set.cache_bytes(), 210);
    }

    #[test]
    fn reload_all_keeps_only_the_selection_and_asks_for_it_again() {
        let mut h = harness(TileSetOptions::default());
        let tiles = row(3);
        h.choose(&tiles[..1]);
        h.load_all();
        h.choose(&tiles[1..]);
        h.load_all();
        assert_eq!(h.set.len(), 3);
        h.set.reload_all();
        assert_eq!(h.set.cached().collect::<Vec<_>>(), tiles[1..]);
        // the old content is drawn until the new is in
        assert_eq!(h.visible(), tiles[1..]);
        h.update();
        assert_eq!(h.fake.asked().len(), 5);
        h.load_all();
        assert_eq!(h.visible(), tiles[1..]);
    }

    // --- uploads -----------------------------------------------------------------------------------

    #[test]
    fn uploads_keep_to_the_frame_budget_nearest_first() {
        let mut h = harness(TileSetOptions { max_requests: 0, ..Default::default() });
        let tiles = row(6);
        h.choose(&tiles);
        h.update();
        for &tile in &tiles {
            h.fake.answer_text(tile, "ok");
        }
        h.update();
        h.pool.wait_idle();
        h.update_with(&mut UploadBudget::per_frame());
        assert_eq!(h.uploads.len(), 4);
        assert_eq!(tiles.iter().filter(|&&t| h.set.state(t) == Some(TileState::Uploading)).count(), 2);
        assert!(h.set.pending() && !h.set.is_loaded());
        h.update_with(&mut UploadBudget::per_frame());
        assert_eq!(h.uploads.len(), 6);
        assert!(!h.set.pending() && h.set.is_loaded());
        let distances: Vec<f64> = h.uploads.iter().map(|&t| tile_distance_priority(&h.viewport, t)).collect();
        assert!(distances.windows(2).all(|w| w[0] <= w[1]), "{distances:?}");
    }

    #[test]
    fn a_frame_uploads_eight_megabytes_and_a_bigger_tile_alone() {
        let mut h = harness(TileSetOptions { max_requests: 0, ..Default::default() });
        let tiles = row(4);
        h.choose(&tiles);
        h.update();
        for (i, &tile) in tiles.iter().enumerate() {
            h.fake.answer_text(tile, if i == 3 { "size:20000000" } else { "size:5000000" });
        }
        h.update();
        h.pool.wait_idle();
        let mut frames = Vec::new();
        while h.uploads.len() < 4 {
            let mut budget = UploadBudget::per_frame();
            h.update_with(&mut budget);
            frames.push(budget.used());
            assert!(frames.len() < 10);
        }
        // 5 MB each, one a frame (a second would pass 8 MB); the 20 MB one in a frame of its own
        assert_eq!(frames, [1, 1, 1, 1]);
        // a budget shared by two tile sets runs out for both
        let mut budget = UploadBudget::new(4, 8 << 20);
        assert!(budget.take(3 << 20) && budget.take(3 << 20));
        assert!(!budget.take(3 << 20));
        assert!(budget.take(2 << 20));
        assert_eq!(budget.used(), 3);
        // nothing left but a tile: one more of nothing fits, then none
        assert!(budget.take(0));
        assert!(!budget.take(0));
        assert_eq!(budget.used(), 4);
        let mut empty = UploadBudget::new(0, 100);
        assert!(!empty.take(1));
    }

    #[test]
    fn decoding_runs_on_the_worker_pool() {
        let fake = Arc::new(Fake::default());
        let pool = Arc::new(Pool::new(2));
        let runtime = TileRuntime { fetcher: fake.clone(), pool: Arc::clone(&pool) };
        let chosen = Fixed::default();
        let threads = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&threads);
        let mut set: TileSet<Vec<u8>> = TileSet::new(&runtime, vec![Source::new("t")], chosen.clone(), TileSetOptions::default(), move |_, bytes, _| {
            seen.lock().unwrap().push(std::thread::current().id());
            Ok(Some(bytes))
        });
        let tile = tile_at(10.0, 52.0, 5);
        chosen.0.lock().unwrap().0 = vec![tile];
        let viewport = viewport_for(&view(10.0, 52.0, 4.5), [800.0, 600.0]);
        let step = |set: &mut TileSet<Vec<u8>>| set.update(&viewport, &mut UploadBudget::unlimited(), |_, d| d);
        step(&mut set);
        fake.answer(tile, Ok(Some(vec![1, 2, 3])));
        step(&mut set);
        // (sent to the pool; it may be done already)
        assert!(matches!(set.state(tile), Some(TileState::Decoding | TileState::Uploading)));
        pool.wait_idle();
        assert!(step(&mut set), "the tile became visible");
        assert_eq!(set.get(tile), Some(&vec![1, 2, 3]));
        assert_eq!(set.cache_bytes(), 3);
        assert_eq!(set.frame_number(), 1);
        let threads = threads.lock().unwrap();
        assert_eq!(threads.len(), 1);
        assert_ne!(threads[0], std::thread::current().id());
    }

    #[test]
    fn a_fetcher_that_answers_at_once_loads_in_two_updates() {
        let pool = Arc::new(Pool::new(1));
        let fetcher = FnFetcher(|source: &str, tile: TileIndex| Ok(Some(format!("{source}/{}", tile.id()).into_bytes())));
        let runtime = TileRuntime { fetcher: Arc::new(fetcher), pool: Arc::clone(&pool) };
        let mut set: TileSet<Vec<u8>, String> =
            TileSet::new(&runtime, vec![Source::new("files")], DeckSelect(rustradar_scene::tiles::VECTOR_TILES), TileSetOptions::default(), |_, b, _| Ok(Some(b)));
        let viewport = viewport_for(&ViewState::INITIAL, [1100.0, 720.0]);
        let upload = |_, bytes: Vec<u8>| String::from_utf8(bytes).unwrap();
        set.update(&viewport, &mut UploadBudget::unlimited(), upload);
        assert!(set.pending());
        let mut updates = 1;
        while set.pending() {
            pool.wait_idle();
            set.update(&viewport, &mut UploadBudget::unlimited(), upload);
            updates += 1;
            assert!(updates < 20);
        }
        assert_eq!(set.selected(), tile_indices(&viewport, &rustradar_scene::tiles::VECTOR_TILES));
        assert!(set.is_loaded());
        let drawn: Vec<&str> = set.visible().map(|(_, s)| s.as_str()).collect();
        assert_eq!(drawn.len(), set.selected().len());
        assert!(drawn.iter().all(|s| s.starts_with("files/")));
    }

    // --- loading ahead -------------------------------------------------------------------------------

    /// lod-tileset.test.ts's views: Malmö, flat map, tilted.
    fn lod_view(zoom: f64) -> Viewport {
        let view = ViewState { longitude: 13.19, latitude: 55.705, zoom, bearing: 20.0, pitch: 60.0, position: None };
        viewport_for(&view, [1100.0, 720.0])
    }

    fn lod() -> Lod {
        Lod { levels: vec![13, 14, 16], bias: 0.5, max_distance: 25_000.0 }
    }

    fn in_view(zoom: f64) -> HashSet<TileIndex> {
        lod::distance_tile_indices(&lod_view(zoom), &lod(), None, 0.0).into_iter().collect()
    }

    fn lod_harness(max_requests: usize) -> Harness {
        let mut h = harness_with(LodSelect::new(lod(), 512.0), TileSetOptions { max_requests, refinement: Refinement::KeepCoarserUntilIn, ..Default::default() });
        h.viewport = lod_view(13.8);
        h
    }

    #[test]
    fn zooming_in_asks_for_the_tiles_of_a_level_further_in_besides_the_view_s() {
        let mut h = lod_harness(0);
        h.update();
        h.viewport = lod_view(14.0);
        h.update();
        let (now, before) = (in_view(14.0), in_view(13.8));
        let extra: Vec<TileIndex> = h.fake.asked().into_iter().filter(|t| !now.contains(t) && !before.contains(t)).collect();
        assert!(!extra.is_empty());
        let further = in_view(15.0);
        assert!(extra.iter().all(|t| further.contains(t)));
        assert!(extra.iter().all(|&t| h.set.is_ahead(t)));
    }

    #[test]
    fn standing_still_asks_for_nothing_more() {
        let mut h = lod_harness(0);
        h.viewport = lod_view(14.0);
        h.update();
        h.update();
        let asked: HashSet<TileIndex> = h.fake.asked().into_iter().collect();
        assert_eq!(asked, in_view(14.0));
    }

    #[test]
    fn tiles_ahead_load_after_the_view_s_and_are_there_on_arrival() {
        let mut h = lod_harness(64);
        h.load_all();
        let first = h.fake.asked().len();
        h.viewport = lod_view(14.0);
        h.update();
        let asked = h.fake.asked()[first..].to_vec();
        let ahead: Vec<bool> = asked.iter().map(|&t| h.set.is_ahead(t) && !h.set.is_selected(t)).collect();
        assert!(ahead.iter().any(|&a| a) && ahead.iter().any(|&a| !a), "{ahead:?}");
        // every tile in view goes out before any tile ahead
        assert!(ahead.windows(2).all(|w| w[0] <= w[1]), "{ahead:?}");
        let aheads: Vec<TileIndex> = asked.iter().zip(&ahead).filter(|(_, a)| **a).map(|(t, _)| *t).collect();
        assert!(aheads.iter().all(|&t| h.set.priority(t).is_some_and(|p| p >= AHEAD_PRIORITY)));
        // neither selected nor visible, still kept while the view stands still
        h.update();
        h.update();
        assert!(aheads.iter().all(|&t| !h.fake.cancelled(t) && h.set.state(t) == Some(TileState::Fetching)));
        h.load_all();
        assert!(aheads.iter().all(|&t| h.set.state(t) == Some(TileState::Loaded)));
        // arriving where it was heading: what is there is drawn at once
        let there = asked.len() + first;
        h.viewport = lod_view(15.0);
        h.update();
        let arrived = h.set.selected().iter().filter(|t| aheads.contains(t)).count();
        assert!(arrived > 0);
        assert!(h.set.selected().iter().filter(|t| aheads.contains(t)).all(|&t| h.set.state(t) == Some(TileState::Loaded)));
        assert!(h.fake.asked()[there..].iter().all(|t| !aheads.contains(t)), "not asked twice");
    }

    #[test]
    fn tiles_ahead_use_only_spare_request_slots() {
        let tiles = row(4);
        let (view_tiles, ahead) = tiles.split_at(3);
        // three in view, one ahead, four slots: all go, the one ahead last
        let mut h = harness(TileSetOptions { max_requests: 4, ..Default::default() });
        h.choose_with_ahead(view_tiles, ahead);
        h.update();
        assert_eq!(h.fake.asked().len(), 4);
        assert_eq!(h.fake.asked()[3], ahead[0]);
        // with two slots the view's tiles need them all: the one ahead is pruned, as LodTileset's
        let mut h = harness(TileSetOptions { max_requests: 2, ..Default::default() });
        h.choose_with_ahead(view_tiles, ahead);
        h.update();
        assert_eq!(h.set.state(ahead[0]), Some(TileState::Cancelled));
        assert!(!h.fake.asked().contains(&ahead[0]));
    }

    #[test]
    fn tiles_ahead_lose_their_place_when_the_zoom_turns() {
        let tiles = row(5);
        let mut h = harness(TileSetOptions { max_requests: 3, ..Default::default() });
        h.choose_with_ahead(&tiles[..1], &tiles[1..3]);
        h.update();
        // the view's tile first, then those ahead (the nearer the centre the sooner)
        assert_eq!(h.fake.asked(), [tiles[0], tiles[2], tiles[1]]);
        assert!(h.set.priority(tiles[1]).is_some_and(|p| p >= AHEAD_PRIORITY));
        // the camera turned: nothing ahead now, and the view wants two more tiles
        h.choose_with_ahead(&[tiles[0], tiles[3], tiles[4]], &[]);
        h.update();
        assert_eq!(h.set.priority(tiles[1]), None);
        assert!(h.fake.cancelled(tiles[1]) && h.fake.cancelled(tiles[2]));
        assert_eq!(h.set.state(tiles[1]), Some(TileState::Cancelled));
        assert_eq!(h.fake.asked()[3..], tiles[3..]);
    }

    #[test]
    fn deck_selection_is_scene_tile_indices() {
        for view_state in [ViewState::INITIAL, view(18.07, 59.33, 9.0), view(17.9186, 59.6519, 13.5)] {
            let viewport = viewport_for(&view_state, [1100.0, 720.0]);
            for options in [rustradar_scene::tiles::VECTOR_TILES, rustradar_scene::tiles::IMAGERY_TILES] {
                let mut select = DeckSelect(options);
                let selection = select.select(&viewport, &|_| false);
                assert_eq!(selection.tiles, tile_indices(&viewport, &options));
                assert!(selection.ahead.is_empty());
            }
        }
        assert_eq!(DeckSelect(TilesetOptions { min_zoom: Some(2.5), ..TilesetOptions::DEFAULT }).min_zoom(), 3);
        assert_eq!(DeckSelect(TilesetOptions::DEFAULT).min_zoom(), 0);
    }
}
