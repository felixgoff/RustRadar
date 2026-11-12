//! Where tile bytes come from (plan 1.1, 1.4.8): `map` never depends on `backend`. A tile set asks
//! a [`TileFetcher`] for a tile's bytes; `app` implements it on its tokio runtime with
//! `backend::tiles` (disk cache, freshness, per-host limits), and headless tests with files on
//! disk.
//!
//! The contract:
//! - [`TileFetcher::fetch`] returns at once; the answer arrives later on the receiver it hands
//!   back, from any thread, and whoever sends it also wakes the main thread (plan 2.1) so the tile
//!   runtime collects it in its next frame.
//! - The answer is `Ok(Some(bytes))` for a tile, `Ok(None)` when the source has nothing there (an
//!   HTTP 404 or 204: an expected hole, settled and not asked again), or `Err` when the request
//!   failed.
//! - The request carries a [`Cancel`] flag: once it is set, nobody waits for the answer, and the
//!   fetcher may stop early and drop the sender without answering.
//!
//! [`Fetch`] is a request in flight as the tile runtime holds it; [`FnFetcher`] answers from a
//! function on the calling thread; [`offline`] has nothing anywhere; [`DirTiles`] answers from a
//! folder of tiles (the committed fixtures, [`fixture_tiles_dir`]: what headless shots and tests
//! draw from).
//!
//! Source names: a layer names its source in a [`FetchRequest`] with one of the constants below
//! ([`OPENFREEMAP`], [`ESRI`], …). They are also the fixture set's folder names and the folders
//! `backend::tiles` keeps each source's files in; the app maps each to its URL.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use rustradar_scene::tiles::TileIndex;

use crate::jobs::Cancel;

/// OpenFreeMap's vector tiles (OpenMapTiles schema; the URL from its TileJSON, doc 01 §5.6): the
/// vector map, labels, plain buildings, the rail map.
pub const OPENFREEMAP: &str = "openfreemap";
/// Esri World Imagery (doc 01 §5.2).
pub const ESRI: &str = "esri";
/// Mapzen's terrarium elevation tiles on AWS (hillshade; the terrain's fallback, doc 04 §14).
pub const TERRARIUM: &str = "terrarium";
/// Mapterhorn's terrarium WebP tiles (the 3D terrain, doc 04 §14).
pub const MAPTERHORN: &str = "mapterhorn";
/// RainViewer's radar frames (doc 02 §2): a frame's URL changes, so the app registers it.
pub const RAINVIEWER: &str = "rainviewer";
/// Streets GL's vector tiles (the realistic map close in, doc 04).
pub const STREETS_GL: &str = "streets-gl";

/// Why a fetch gave no bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The request failed (no connection, a server error, a broken file); the text says how.
    Failed(String),
    /// The request was cancelled, or the fetcher dropped it without an answer.
    Cancelled,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Failed(why) => write!(f, "tile request failed: {why}"),
            FetchError::Cancelled => f.write_str("tile request cancelled"),
        }
    }
}

impl std::error::Error for FetchError {}

/// A fetch's answer: the bytes, `None` for nothing there (404), or why there are none.
pub type FetchResult = Result<Option<Vec<u8>>, FetchError>;

/// One tile asked of a source.
#[derive(Debug, Clone)]
pub struct FetchRequest {
    /// The source's name as the layer knows it (`"openfreemap"`, `"esri"`, …): the fetcher maps it
    /// to a URL template and a cache folder.
    pub source: Arc<str>,
    pub tile: TileIndex,
    /// Set once nobody waits for the answer.
    pub cancel: Cancel,
}

/// The tile runtime's way to the network or the disk.
pub trait TileFetcher: Send + Sync {
    /// Starts fetching `request.tile` from `request.source` and returns at once; the answer comes
    /// on the receiver (see the module's contract).
    fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult>;
}

/// A receiver that already holds `result` (for fetchers that answer at once).
pub fn answered(result: FetchResult) -> Receiver<FetchResult> {
    let (tx, rx) = mpsc::channel();
    // (the receiver is right here: the send cannot fail)
    let _ = tx.send(result);
    rx
}

/// A fetcher that answers at once, on the calling thread, with what a function returns for the
/// source's name and the tile (files on disk, tests).
pub struct FnFetcher<F>(pub F);

impl<F> TileFetcher for FnFetcher<F>
where
    F: Fn(&str, TileIndex) -> FetchResult + Send + Sync,
{
    fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
        answered((self.0)(&request.source, request.tile))
    }
}

/// A fetcher with nothing anywhere: every tile settles empty (a renderer before the app hands it
/// its fetcher, and tests without tiles).
pub fn offline() -> Arc<dyn TileFetcher> {
    Arc::new(FnFetcher(|_: &str, _: TileIndex| Ok(None)))
}

/// The tile fixtures committed with this crate (`crates/map/tests/data/tiles`, plan E16), where
/// this crate was built. Only there in a source checkout: check that it is a folder.
pub fn fixture_tiles_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/tiles")
}

/// What a [`DirTiles`] folder holds for a tile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Held {
    /// The tile's file.
    Tile(PathBuf),
    /// The server had nothing there.
    Nothing,
    /// The folder has no answer for it.
    Missing,
}

/// A [`TileFetcher`] over a folder of tiles laid out as the fixture set is (plan E16): one
/// subfolder per source, named as the layers name their sources, each tile at
/// `<source>/<z>/<x>/<y>.<ext>` (the bytes the server sent, HTTP compression undone, as
/// `backend::tiles` hands them over), and an empty `<y>.none` for a tile the server had nothing
/// for. `cargo xtask fixtures` writes the committed set.
///
/// Answers at once, on the calling thread:
/// - a tile the folder holds: `Ok(Some(bytes))`;
/// - a tile the server had nothing for: `Ok(None)`;
/// - anything else (another tile, another source): `Ok(None)` as well, so a shot simply does not
///   draw it (plan section 8), and the request is noted in [`DirTiles::misses`] so a test or a
///   shot's log can say the folder lacks it;
/// - a file that cannot be read: `Err(FetchError::Failed)`.
#[derive(Debug)]
pub struct DirTiles {
    root: PathBuf,
    misses: Mutex<Vec<(String, TileIndex)>>,
}

impl DirTiles {
    /// The tiles in `root`.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        DirTiles { root: root.into(), misses: Mutex::new(Vec::new()) }
    }

    /// The committed fixtures ([`fixture_tiles_dir`]).
    pub fn fixtures() -> Self {
        Self::at(fixture_tiles_dir())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The sources the folder holds tiles of, sorted.
    pub fn sources(&self) -> Vec<String> {
        let mut sources: Vec<String> = fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        sources.sort();
        sources
    }

    /// What the folder holds for `tile` of `source`.
    pub fn held(&self, source: &str, tile: TileIndex) -> Held {
        // (a source name is a folder name: nothing that climbs out of the folder)
        if source.is_empty() || source.starts_with('.') || source.contains(['/', '\\']) {
            return Held::Missing;
        }
        let dir = self.root.join(source).join(tile.z.to_string()).join(tile.x.to_string());
        let stem = tile.y.to_string();
        let Ok(entries) = fs::read_dir(&dir) else { return Held::Missing };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_stem().and_then(|s| s.to_str()) != Some(stem.as_str()) {
                continue;
            }
            return match path.extension().and_then(|e| e.to_str()) {
                Some("none") => Held::Nothing,
                _ => Held::Tile(path),
            };
        }
        Held::Missing
    }

    /// The answer for `tile` of `source` (see the type's list).
    pub fn read(&self, source: &str, tile: TileIndex) -> FetchResult {
        // the land cover's classes (R8): the folder's NMD and WorldCover pictures put together
        // as `backend::landcover` puts the servers' together
        if source == crate::layers::ground::landcover::LAND_COVER {
            return crate::layers::ground::landcover::fixture_land_cover(self, tile);
        }
        // Overture's buildings (R6): the fixture folder's PMTiles archive, not a folder of tiles
        // (a tile the archive lacks is nothing there)
        if source == crate::layers::roofs::OVERTURE {
            let Some(archive) = crate::layers::roofs::fixture_archive(&self.root) else {
                self.misses.lock().unwrap_or_else(|e| e.into_inner()).push((source.to_owned(), tile));
                return Ok(None);
            };
            return crate::layers::roofs::archive_tile(&archive, tile);
        }
        // the trees' canopy heights (R7): the fixture folder's recorded ranges of the canopy
        // files, read as `backend::canopy` reads the network (a tile they do not hold is a miss)
        if source == crate::layers::trees::CANOPY {
            return crate::layers::trees::fixture_canopy(&self.root, tile).unwrap_or_else(|| {
                self.misses.lock().unwrap_or_else(|e| e.into_inner()).push((source.to_owned(), tile));
                Ok(None)
            });
        }
        match self.held(source, tile) {
            Held::Tile(path) => fs::read(&path).map(Some).map_err(|e| FetchError::Failed(format!("{}: {e}", path.display()))),
            // the 3D surface's stand-in (Z1, `terrainFetch` as `backend::terrain` has it): where
            // Mapterhorn had nothing, Mapzen's terrarium tile of the same z/x/y, which a fixture
            // set keeps beside it to Mapzen's deepest zoom
            Held::Nothing if source == MAPTERHORN && tile.z <= rustradar_scene::hillshade::TERRARIUM_MAX_ZOOM => self.read(TERRARIUM, tile),
            Held::Nothing => Ok(None),
            Held::Missing => {
                self.misses.lock().unwrap_or_else(|e| e.into_inner()).push((source.to_owned(), tile));
                Ok(None)
            }
        }
    }

    /// The requests the folder had no answer for, in the order they came.
    pub fn misses(&self) -> Vec<(String, TileIndex)> {
        self.misses.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Every tile the folder holds of `source` (files and "nothing there" markers), sorted by zoom,
    /// column and row.
    pub fn tiles(&self, source: &str) -> Vec<TileIndex> {
        let numbered = |dir: &Path| -> Vec<(u32, PathBuf)> {
            fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| Some((e.file_name().to_str()?.parse().ok()?, e.path()))).collect()
        };
        let mut tiles = Vec::new();
        for (z, zdir) in numbered(&self.root.join(source)) {
            for (x, xdir) in numbered(&zdir) {
                for entry in fs::read_dir(&xdir).into_iter().flatten().flatten() {
                    let path = entry.path();
                    if let Some(y) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse().ok()) {
                        tiles.push(TileIndex::new(x, y, z));
                    }
                }
            }
        }
        tiles.sort_by_key(|t| (t.z, t.x, t.y));
        tiles
    }
}

impl TileFetcher for DirTiles {
    fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
        answered(self.read(&request.source, request.tile))
    }
}

/// A request in flight. Dropping it cancels the request.
#[derive(Debug)]
pub struct Fetch {
    answer: Receiver<FetchResult>,
    cancel: Cancel,
}

impl Fetch {
    /// Asks `fetcher` for `tile` from `source`.
    pub fn start(fetcher: &dyn TileFetcher, source: Arc<str>, tile: TileIndex) -> Self {
        let cancel = Cancel::new();
        let answer = fetcher.fetch(FetchRequest { source, tile, cancel: cancel.clone() });
        Fetch { answer, cancel }
    }

    /// The answer if it has come, without blocking. A fetcher that dropped the request without an
    /// answer reads as [`FetchError::Cancelled`] once.
    pub fn poll(&mut self) -> Option<FetchResult> {
        match self.answer.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(FetchError::Cancelled)),
        }
    }

    /// Tells the fetcher nobody waits for the answer.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

impl Drop for Fetch {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Sender;

    #[test]
    fn a_fn_fetcher_answers_at_once_with_the_source_and_tile() {
        let fetcher = FnFetcher(|source: &str, tile: TileIndex| match tile.z {
            0 => Ok(Some(format!("{source} {}", tile.id()).into_bytes())),
            1 => Ok(None),
            _ => Err(FetchError::Failed("no such file".into())),
        });
        let mut fetch = Fetch::start(&fetcher, "dark".into(), TileIndex::new(0, 0, 0));
        assert_eq!(fetch.poll(), Some(Ok(Some(b"dark 0-0-0".to_vec()))));
        let mut fetch = Fetch::start(&fetcher, "dark".into(), TileIndex::new(1, 0, 1));
        assert_eq!(fetch.poll(), Some(Ok(None)));
        let mut fetch = Fetch::start(&fetcher, "dark".into(), TileIndex::new(1, 0, 2));
        let answer = fetch.poll().unwrap();
        assert_eq!(answer, Err(FetchError::Failed("no such file".into())));
        assert_eq!(answer.unwrap_err().to_string(), "tile request failed: no such file");
    }

    /// Holds every request so a test answers (or drops) them itself.
    #[derive(Default)]
    struct Held(Mutex<Vec<(FetchRequest, Sender<FetchResult>)>>);

    impl TileFetcher for Held {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().push((request, tx));
            rx
        }
    }

    #[test]
    fn an_answer_arrives_later_from_another_thread() {
        let fetcher = Held::default();
        let mut fetch = Fetch::start(&fetcher, "esri".into(), TileIndex::new(3, 4, 5));
        assert_eq!(fetch.poll(), None);
        let (request, tx) = fetcher.0.lock().unwrap().pop().unwrap();
        assert_eq!((&*request.source, request.tile), ("esri", TileIndex::new(3, 4, 5)));
        std::thread::spawn(move || tx.send(Ok(Some(vec![1, 2, 3]))).unwrap()).join().unwrap();
        assert_eq!(fetch.poll(), Some(Ok(Some(vec![1, 2, 3]))));
    }

    #[test]
    fn cancelling_reaches_the_fetcher_and_a_dropped_request_reads_cancelled() {
        let fetcher = Held::default();
        let cancelled = Fetch::start(&fetcher, "esri".into(), TileIndex::new(0, 0, 1));
        let dropped = Fetch::start(&fetcher, "esri".into(), TileIndex::new(1, 0, 1));
        let mut abandoned = Fetch::start(&fetcher, "esri".into(), TileIndex::new(0, 1, 1));
        let mut requests = std::mem::take(&mut *fetcher.0.lock().unwrap());
        assert!(requests.iter().all(|(r, _)| !r.cancel.is_cancelled()));
        cancelled.cancel();
        assert!(cancelled.is_cancelled());
        assert!(requests[0].0.cancel.is_cancelled());
        // dropping the handle cancels too
        drop(dropped);
        assert!(requests[1].0.cancel.is_cancelled());
        // a fetcher that gives up on a request without answering
        assert_eq!(abandoned.poll(), None);
        requests.pop();
        assert_eq!(abandoned.poll(), Some(Err(FetchError::Cancelled)));
        assert!(!abandoned.is_cancelled());
    }

    #[test]
    fn offline_has_nothing_anywhere() {
        let mut fetch = Fetch::start(&*offline(), OPENFREEMAP.into(), TileIndex::ROOT);
        assert_eq!(fetch.poll(), Some(Ok(None)));
    }

    #[test]
    fn a_folder_of_tiles_serves_its_files_and_notes_what_it_lacks() {
        let root = std::env::temp_dir().join(format!("rustradar-dir-tiles-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("esri/3/4")).unwrap();
        fs::write(root.join("esri/3/4/5.jpg"), b"jpeg").unwrap();
        fs::write(root.join("esri/3/4/6.none"), b"").unwrap();
        let tiles = DirTiles::at(&root);
        assert_eq!(tiles.sources(), ["esri"]);
        assert_eq!(tiles.tiles(ESRI), [TileIndex::new(4, 5, 3), TileIndex::new(4, 6, 3)]);
        let mut fetch = Fetch::start(&tiles, ESRI.into(), TileIndex::new(4, 5, 3));
        assert_eq!(fetch.poll(), Some(Ok(Some(b"jpeg".to_vec()))));
        assert_eq!(tiles.read(ESRI, TileIndex::new(4, 6, 3)), Ok(None));
        assert!(tiles.misses().is_empty(), "a tile with nothing there is no miss");
        assert_eq!(tiles.read(ESRI, TileIndex::new(4, 7, 3)), Ok(None));
        assert_eq!(tiles.read("../esri", TileIndex::new(4, 5, 3)), Ok(None));
        assert_eq!(tiles.misses(), [(ESRI.to_owned(), TileIndex::new(4, 7, 3)), ("../esri".to_owned(), TileIndex::new(4, 5, 3))]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_committed_fixtures_are_found_from_this_crate() {
        let tiles = DirTiles::fixtures();
        assert!(tiles.root().is_dir(), "{}", tiles.root().display());
        for source in [OPENFREEMAP, ESRI, TERRARIUM, RAINVIEWER, STREETS_GL] {
            assert!(tiles.sources().iter().any(|s| s == source), "the fixtures lack {source}");
        }
        assert!(matches!(tiles.held(OPENFREEMAP, TileIndex::ROOT), super::Held::Tile(_)));
    }
}
