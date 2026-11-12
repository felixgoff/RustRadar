//! Where the map's tiles come from (plan 1.1, 1.4.8, section 8; item T1): `map::io::TileFetcher`
//! for the app.
//!
//! - [`BackendTiles`]: the network through `backend::tiles` (disk cache under `<cache>/tiles`,
//!   freshness, stale fallback, per-host turns), on the backend's tokio runtime. A request returns
//!   at once; its task sends the answer and wakes the event loop, and stops early (sending
//!   nothing) once the tile runtime cancels it.
//! - [`TileSources`]: what each source name a layer asks for (`map::io::OPENFREEMAP`, `ESRI`, …)
//!   means: a URL template, or a TileJSON read once (OpenFreeMap's, whose tile URL changes with each
//!   planet build and is never written here, doc 01 §5.6). A source whose URL changes while the app
//!   runs (a RainViewer frame) is set again with [`TileSources::set_template`].
//! - `--fixture DIR`, and every shot: a folder of tiles ([`DirTiles`], `DIR/tiles` when the
//!   fixture folder has one, else `DIR` itself). A shot never touches the network: without
//!   `--fixture` it draws from the committed fixtures (`crates/map/tests/data/tiles`) where they
//!   are, else from nothing ([`shot_tiles`]).
//!
//! [`connect`] hands the map the fetcher the state calls for and the event loop's wake.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use rustradar_backend::Wake;
use rustradar_backend::tiles::{self, Tiles};
use rustradar_map::MapRenderer;
use rustradar_map::io::{self, DirTiles, FetchError, FetchRequest, FetchResult, TileFetcher};
use rustradar_map::jobs::Cancel;
use tokio::runtime::Handle;
use tokio::sync::OnceCell;

use crate::Options;
use crate::state::AppState;

/// Esri World Imagery (G:309, G:483).
pub const ESRI_TEMPLATE: &str = "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}";
/// Mapzen's terrarium tiles on AWS (`terrain-layer.ts` `TERRARIUM_URL`).
pub const TERRARIUM_TEMPLATE: &str = "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png";
/// Mapterhorn's terrarium WebP tiles (`terrain-layer.ts` `MAPTERHORN_URL`).
pub const MAPTERHORN_TEMPLATE: &str = "https://tiles.mapterhorn.com/{z}/{x}/{y}.webp";
/// Streets GL's vector tiles (`streets-gl.ts` `SGL_TILES`).
pub const STREETS_GL_TEMPLATE: &str = "https://tiles.streets.gl/vector/{z}/{x}/{y}";

/// How often a request in flight looks whether it was cancelled.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// What a source name stands for.
#[derive(Clone)]
enum Entry {
    /// A URL template.
    Template(tiles::Source),
    /// The first tile URL of the TileJSON at `url`, read once (until it is read, every request
    /// asks for it again).
    TileJson { url: String, source: Arc<OnceCell<tiles::Source>> },
}

/// The tile sources by the names the map's layers use. Cheap to clone; every clone is the same
/// table.
#[derive(Clone, Default)]
pub struct TileSources(Arc<Mutex<HashMap<String, Entry>>>);

impl std::fmt::Debug for TileSources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names: Vec<String> = self.lock().keys().cloned().collect();
        names.sort();
        f.debug_tuple("TileSources").field(&names).finish()
    }
}

impl TileSources {
    /// The app's sources: OpenFreeMap from its TileJSON, Esri imagery, terrarium, Mapterhorn and
    /// Streets GL. RainViewer's frames are set as the radar state learns them.
    pub fn app() -> Self {
        let sources = Self::default();
        sources.set_tilejson(io::OPENFREEMAP, tiles::OPENFREEMAP_TILEJSON);
        sources.set_template(io::ESRI, ESRI_TEMPLATE);
        sources.set_template(io::TERRARIUM, TERRARIUM_TEMPLATE);
        sources.set_template(io::MAPTERHORN, MAPTERHORN_TEMPLATE);
        sources.set_template(io::STREETS_GL, STREETS_GL_TEMPLATE);
        sources
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `name`'s tiles from `template` (`{z}`, `{x}`, `{y}` in any order), kept in the cache folder
    /// `name`. A new template for a name makes its kept tiles stale.
    pub fn set_template(&self, name: &str, template: &str) {
        self.lock().insert(name.to_owned(), Entry::Template(tiles::Source::new(name, template)));
    }

    /// `name`'s tiles from the first tile URL of the TileJSON at `url` (kept in the cache folder
    /// `name`, the TileJSON as `name/tilejson.json`).
    pub fn set_tilejson(&self, name: &str, url: &str) {
        self.lock().insert(name.to_owned(), Entry::TileJson { url: url.to_owned(), source: Arc::new(OnceCell::new()) });
    }

    /// Whether `name` is a source.
    pub fn contains(&self, name: &str) -> bool {
        self.lock().contains_key(name)
    }

    /// `name`'s template now, its TileJSON read with `tiles` if need be.
    pub async fn resolve(&self, tiles: &Tiles, name: &str) -> Result<tiles::Source, String> {
        let entry = self.lock().get(name).cloned().ok_or_else(|| format!("no tile source named {name:?}"))?;
        match entry {
            Entry::Template(source) => Ok(source),
            Entry::TileJson { url, source } => source.get_or_try_init(|| tiles.source_from_tilejson(name, &url)).await.cloned(),
        }
    }
}

/// The network's tiles: `backend::tiles` on the backend's runtime.
#[derive(Clone)]
pub struct BackendTiles {
    tiles: Tiles,
    sources: TileSources,
    rt: Handle,
    wake: Wake,
}

impl std::fmt::Debug for BackendTiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackendTiles").field("root", &self.tiles.root()).field("sources", &self.sources).finish_non_exhaustive()
    }
}

impl BackendTiles {
    /// Tiles through `tiles` from `sources`, fetched on `rt`; `wake` after each answer.
    pub fn new(tiles: Tiles, sources: TileSources, rt: Handle, wake: Wake) -> Self {
        BackendTiles { tiles, sources, rt, wake }
    }

    /// The app's sources ([`TileSources::app`]) through the cache in `<cache_dir>/tiles`.
    pub fn in_cache(cache_dir: &Path, rt: Handle, wake: Wake) -> Result<Self, String> {
        Ok(Self::new(Tiles::new(cache_dir)?, TileSources::app(), rt, wake))
    }

    /// The source table (to set a source whose URL changes, as a radar frame's).
    pub fn sources(&self) -> &TileSources {
        &self.sources
    }
}

impl TileFetcher for BackendTiles {
    fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
        let (tx, rx) = mpsc::channel();
        let (tiles, sources, wake) = (self.tiles.clone(), self.sources.clone(), self.wake.clone());
        // (a spawn from the main thread: through the runtime's handle, doc 07 §11.4)
        self.rt.spawn(async move {
            let FetchRequest { source, tile, cancel } = request;
            let get = async {
                // the rail map's tracks (T4): the backend's own rail tiles, kept for routing
                // (`railtiles:` today), not a URL of the table
                if &*source == rustradar_map::layers::rail::RAIL_TILES {
                    return rustradar_backend::rail::tile(tile.z, tile.x, tile.y).await.map(|t| t.map(|b| b.to_vec()));
                }
                // Overture's buildings (R6): its PMTiles archive, read with range requests by
                // `backend::overture` (`--fixture` folders: `DirTiles` reads theirs)
                if &*source == rustradar_map::layers::roofs::OVERTURE {
                    return rustradar_backend::overture::tile(tile.z, tile.x, tile.y).await;
                }
                // the trees' canopy heights (R7): `backend::canopy` reads the canopy height map's
                // files with range requests (`--fixture` folders: `DirTiles` reads their recording)
                if &*source == rustradar_map::layers::trees::CANOPY {
                    return rustradar_backend::canopy::tile(tile.z, tile.x, tile.y).await;
                }
                // the land cover (R8): a tile's classes, NMD's and WorldCover's pictures put
                // together by `backend::landcover` (`--fixture` folders: `DirTiles` puts theirs)
                if &*source == rustradar_map::layers::ground::landcover::LAND_COVER {
                    return rustradar_backend::landcover::tile(&tiles, tile.z, tile.x, tile.y).await;
                }
                let source = sources.resolve(&tiles, &source).await?;
                // Streets GL (R3): only its zoom 16 goes to its server (`sglFetch`)
                if source.name == rustradar_backend::streets_gl::NAME {
                    return rustradar_backend::streets_gl::tile(&tiles, &source, tile.z, tile.x, tile.y).await;
                }
                // the 3D surface (Z1): Mapzen's terrarium where Mapterhorn has nothing (`terrainFetch`)
                if source.name == rustradar_backend::terrain::MAPTERHORN {
                    let mapzen = sources.resolve(&tiles, io::TERRARIUM).await?;
                    return rustradar_backend::terrain::tile(&tiles, &source, &mapzen, tile.z, tile.x, tile.y).await;
                }
                tiles.tile(&source, tile.z, tile.x, tile.y).await
            };
            let result = tokio::select! {
                result = get => result,
                // nobody waits: stop the download, answer nothing
                () = cancelled(&cancel) => return,
            };
            if tx.send(result.map_err(FetchError::Failed)).is_ok() {
                wake();
            }
        });
        rx
    }
}

/// Resolves once `cancel` is set.
async fn cancelled(cancel: &Cancel) {
    while !cancel.is_cancelled() {
        tokio::time::sleep(CANCEL_POLL).await;
    }
}

/// The tile folder of a `--fixture` folder: its `tiles` subfolder if it has one, else itself
/// (`--fixture crates/map/tests/data/tiles` and `--fixture crates/map/tests/data` read the same).
pub fn fixture_tiles(fixture: &Path) -> PathBuf {
    let tiles = fixture.join("tiles");
    if tiles.is_dir() { tiles } else { fixture.to_owned() }
}

/// A shot's tiles (plan section 8: never the network): the `--fixture` folder's, else the
/// committed fixtures where this build finds them, else none.
pub fn shot_tiles(options: &Options) -> Option<Arc<DirTiles>> {
    let root = match &options.fixture {
        Some(fixture) => fixture_tiles(fixture),
        None => io::fixture_tiles_dir(),
    };
    root.is_dir().then(|| Arc::new(DirTiles::at(root)))
}

/// The fetcher `state` calls for: the `--fixture` folder's tiles, else the network through the
/// backend's tile cache, else (no backend) nothing.
pub fn for_state(state: &AppState) -> Arc<dyn TileFetcher> {
    if let Some(fixture) = &state.startup.fixture {
        return Arc::new(DirTiles::at(fixture_tiles(fixture)));
    }
    let Some(backend) = &state.backend else { return io::offline() };
    // the radar state's table: it sets RainViewer's template in it as each frame comes (W5)
    match Tiles::new(&backend.dirs().cache) {
        Ok(tiles) => Arc::new(BackendTiles::new(tiles, state.radar.tile_sources().clone(), backend.rt().clone(), backend.waker())),
        Err(e) => {
            log::error!("no map tiles: the tile cache did not open: {e}");
            io::offline()
        }
    }
}

/// Hands `map` the fetcher `state` calls for ([`for_state`]) and the event loop's wake (the
/// requests' wake: the worker pool calls it after each decoded tile). The window calls it once,
/// before the first frame.
pub fn connect(map: &mut MapRenderer, state: &AppState) {
    map.set_tiles(for_state(state), state.requests.as_ref().map(|r| r.wake()));
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use rustradar_map::io::Fetch;
    use rustradar_scene::tiles::TileIndex;

    use super::*;
    use crate::requests;

    /// An HTTP/1.1 server on 127.0.0.1 on threads of its own: each connection answered by
    /// `handler` from the path with a status, a body and a delay. Notes every path asked for.
    struct Server {
        base: String,
        paths: Arc<Mutex<Vec<String>>>,
    }

    impl Server {
        fn start(handler: impl Fn(&str) -> (u16, Vec<u8>, Duration) + Send + Sync + 'static) -> Server {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let paths = Arc::new(Mutex::new(Vec::new()));
            let (handler, noted) = (Arc::new(handler), Arc::clone(&paths));
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let (handler, noted) = (Arc::clone(&handler), Arc::clone(&noted));
                    std::thread::spawn(move || {
                        let (mut head, mut buf) = (Vec::new(), [0u8; 1024]);
                        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                            match stream.read(&mut buf) {
                                Ok(0) | Err(_) => return,
                                Ok(n) => head.extend_from_slice(&buf[..n]),
                            }
                        }
                        let path = String::from_utf8_lossy(&head).split_whitespace().nth(1).unwrap_or("/").to_owned();
                        noted.lock().unwrap().push(path.clone());
                        let (status, body, delay) = handler(&path);
                        std::thread::sleep(delay);
                        let head = format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        let _ = stream.write_all(head.as_bytes());
                        let _ = stream.write_all(&body);
                    });
                }
            });
            Server { base, paths }
        }

        fn url(&self, path: &str) -> String {
            format!("{}{path}", self.base)
        }

        fn paths(&self) -> Vec<String> {
            self.paths.lock().unwrap().clone()
        }
    }

    /// A tile cache folder of its own for one test, removed when it ends.
    struct TempCache(PathBuf);

    impl TempCache {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("rustradar-app-tiles-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            TempCache(root)
        }
    }

    impl Drop for TempCache {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A fetcher over `sources` in `cache`, counting its wakes.
    fn fetcher(cache: &TempCache, sources: TileSources) -> (BackendTiles, Arc<AtomicUsize>) {
        let woken = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&woken);
        let rt = requests::runtime().expect("runtime");
        let wake: Wake = Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        (BackendTiles::new(Tiles::new(&cache.0).unwrap(), sources, rt.handle().clone(), wake), woken)
    }

    /// Polls `fetch` until it answers (at most 10 s).
    fn answer(fetch: &mut Fetch) -> FetchResult {
        let started = Instant::now();
        loop {
            if let Some(result) = fetch.poll() {
                return result;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "no answer");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn wait_for(count: &AtomicUsize, n: usize) {
        let started = Instant::now();
        while count.load(Ordering::SeqCst) < n && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_tile_comes_through_the_backend_cache_and_wakes_the_loop() {
        let server = Server::start(|path| match path {
            "/t/3/4/5.pbf" => (200, b"tile 3/4/5".to_vec(), Duration::ZERO),
            _ => (404, Vec::new(), Duration::ZERO),
        });
        let cache = TempCache::new("through");
        let sources = TileSources::default();
        sources.set_template("test", &server.url("/t/{z}/{x}/{y}.pbf"));
        let (tiles, woken) = fetcher(&cache, sources);
        let mut fetch = Fetch::start(&tiles, "test".into(), TileIndex::new(4, 5, 3));
        assert_eq!(answer(&mut fetch), Ok(Some(b"tile 3/4/5".to_vec())));
        wait_for(&woken, 1);
        assert_eq!(woken.load(Ordering::SeqCst), 1);
        // kept on disk where backend::tiles keeps it: the second answer does not ask the server
        assert!(cache.0.join("tiles/test/3/4/5").is_file());
        let mut again = Fetch::start(&tiles, "test".into(), TileIndex::new(4, 5, 3));
        assert_eq!(answer(&mut again), Ok(Some(b"tile 3/4/5".to_vec())));
        // nothing there, and a source nobody named
        let mut hole = Fetch::start(&tiles, "test".into(), TileIndex::new(0, 0, 3));
        assert_eq!(answer(&mut hole), Ok(None));
        let mut unknown = Fetch::start(&tiles, "nowhere".into(), TileIndex::ROOT);
        assert_eq!(answer(&mut unknown), Err(FetchError::Failed("no tile source named \"nowhere\"".into())));
        assert_eq!(server.paths(), ["/t/3/4/5.pbf", "/t/3/0/0.pbf"]);
    }

    /// Streets GL (R3, `sglFetch`): only its zoom 16 goes to its server; another zoom is nothing.
    #[test]
    fn streets_gl_is_asked_for_its_zoom_only() {
        let server = Server::start(|path| (200, path.as_bytes().to_vec(), Duration::ZERO));
        let cache = TempCache::new("streets-gl");
        let sources = TileSources::default();
        sources.set_template(io::STREETS_GL, &server.url("/vector/{z}/{x}/{y}"));
        let (tiles, _) = fetcher(&cache, sources);
        let mut near = Fetch::start(&tiles, io::STREETS_GL.into(), TileIndex::new(36057, 19273, 16));
        assert_eq!(answer(&mut near), Ok(Some(b"/vector/16/36057/19273".to_vec())));
        let mut coarser = Fetch::start(&tiles, io::STREETS_GL.into(), TileIndex::new(9014, 4818, 14));
        assert_eq!(answer(&mut coarser), Ok(None));
        assert_eq!(server.paths(), ["/vector/16/36057/19273"]);
    }

    /// The 3D surface (Z1, `terrainFetch`): a Mapterhorn tile it has nothing for is Mapzen's
    /// terrarium tile of the same `z/x/y`.
    #[test]
    fn mapterhorn_falls_back_to_mapzen() {
        let server = Server::start(|path| match path {
            "/mt/16/1/2.webp" => (200, b"webp".to_vec(), Duration::ZERO),
            "/mz/16/3/4.png" => (200, b"png".to_vec(), Duration::ZERO),
            _ => (404, Vec::new(), Duration::ZERO),
        });
        let cache = TempCache::new("terrain");
        let sources = TileSources::default();
        sources.set_template(io::MAPTERHORN, &server.url("/mt/{z}/{x}/{y}.webp"));
        sources.set_template(io::TERRARIUM, &server.url("/mz/{z}/{x}/{y}.png"));
        let (tiles, _) = fetcher(&cache, sources);
        let mut there = Fetch::start(&tiles, io::MAPTERHORN.into(), TileIndex::new(1, 2, 16));
        assert_eq!(answer(&mut there), Ok(Some(b"webp".to_vec())));
        let mut stand_in = Fetch::start(&tiles, io::MAPTERHORN.into(), TileIndex::new(3, 4, 16));
        assert_eq!(answer(&mut stand_in), Ok(Some(b"png".to_vec())));
        assert_eq!(server.paths(), ["/mt/16/1/2.webp", "/mt/16/3/4.webp", "/mz/16/3/4.png"]);
    }

    #[test]
    fn a_tilejson_source_is_read_once_for_all_its_tiles() {
        let base = Arc::new(Mutex::new(String::new()));
        let b = Arc::clone(&base);
        let server = Server::start(move |path| match path {
            "/planet" => (200, format!("{{\"tiles\":[\"{}/build-1/{{z}}/{{x}}/{{y}}.pbf\"]}}", b.lock().unwrap()).into_bytes(), Duration::ZERO),
            p if p.starts_with("/build-1/") => (200, p.as_bytes().to_vec(), Duration::ZERO),
            _ => (404, Vec::new(), Duration::ZERO),
        });
        *base.lock().unwrap() = server.base.clone();
        let cache = TempCache::new("tilejson");
        let sources = TileSources::default();
        sources.set_tilejson(io::OPENFREEMAP, &server.url("/planet"));
        let (tiles, _) = fetcher(&cache, sources);
        let mut fetches: Vec<Fetch> = (0..3).map(|x| Fetch::start(&tiles, io::OPENFREEMAP.into(), TileIndex::new(x, 1, 2))).collect();
        for (x, fetch) in fetches.iter_mut().enumerate() {
            assert_eq!(answer(fetch), Ok(Some(format!("/build-1/2/{x}/1.pbf").into_bytes())));
        }
        let paths = server.paths();
        assert_eq!(paths.iter().filter(|p| *p == "/planet").count(), 1, "{paths:?}");
        assert!(cache.0.join("tiles/openfreemap/tilejson.json").is_file());
    }

    #[test]
    fn a_cancelled_request_stops_and_answers_nothing() {
        let server = Server::start(|_| (200, b"late".to_vec(), Duration::from_secs(3)));
        let cache = TempCache::new("cancel");
        let sources = TileSources::default();
        sources.set_template("slow", &server.url("/{z}/{x}/{y}"));
        let (tiles, woken) = fetcher(&cache, sources);
        let mut fetch = Fetch::start(&tiles, "slow".into(), TileIndex::new(1, 1, 1));
        let started = Instant::now();
        while server.paths().is_empty() {
            assert!(started.elapsed() < Duration::from_secs(5), "never asked");
            std::thread::sleep(Duration::from_millis(2));
        }
        fetch.cancel();
        // the task drops its sender within a poll of the flag, long before the server answers
        assert_eq!(answer(&mut fetch), Err(FetchError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        assert_eq!(woken.load(Ordering::SeqCst), 0, "nothing to collect: no wake");
        // a request cancelled before it starts never reaches the server
        let request = FetchRequest { source: "slow".into(), tile: TileIndex::new(0, 1, 1), cancel: Cancel::new() };
        request.cancel.cancel();
        let rx = tiles.fetch(request);
        assert!(rx.recv_timeout(Duration::from_secs(2)).is_err());
        assert_eq!(server.paths(), ["/1/1/1"]);
    }

    #[test]
    fn the_app_s_sources_are_today_s() {
        let sources = TileSources::app();
        for name in [io::OPENFREEMAP, io::ESRI, io::TERRARIUM, io::MAPTERHORN, io::STREETS_GL] {
            assert!(sources.contains(name), "{name}");
        }
        let entries = sources.lock();
        let template = |name: &str| match entries.get(name) {
            Some(Entry::Template(source)) => source.url(3, 4, 5),
            _ => panic!("{name} is no template"),
        };
        assert_eq!(template(io::ESRI), "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/3/5/4");
        assert_eq!(template(io::TERRARIUM), "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/3/4/5.png");
        assert_eq!(template(io::MAPTERHORN), "https://tiles.mapterhorn.com/3/4/5.webp");
        assert_eq!(template(io::STREETS_GL), "https://tiles.streets.gl/vector/3/4/5");
        // OpenFreeMap's URL is read from its TileJSON, never written here (doc 01 §5.6)
        assert!(matches!(entries.get(io::OPENFREEMAP), Some(Entry::TileJson { url, .. }) if url == "https://tiles.openfreemap.org/planet"));
    }

    #[test]
    fn a_fixture_folder_names_its_tiles_folder_or_is_one() {
        let committed = io::fixture_tiles_dir();
        assert_eq!(fixture_tiles(&committed), committed);
        assert_eq!(fixture_tiles(committed.parent().unwrap()), committed);
        // a shot reads the committed set without --fixture, the fixture's with it
        let shot = shot_tiles(&Options::default()).expect("the committed fixtures");
        assert_eq!(shot.root(), committed);
        assert_eq!(shot.read(io::OPENFREEMAP, TileIndex::ROOT), Ok(Some(std::fs::read(committed.join("openfreemap/0/0/0.pbf")).unwrap())));
        let elsewhere = Options { fixture: Some(committed.parent().unwrap().to_owned()), ..Default::default() };
        assert_eq!(shot_tiles(&elsewhere).unwrap().root(), committed);
        let missing = Options { fixture: Some(PathBuf::from("/no/such/fixture")), ..Default::default() };
        assert!(shot_tiles(&missing).is_none());
    }

    #[test]
    fn the_state_picks_the_fixture_s_tiles_or_none_without_a_backend() {
        use rustradar_scene::camera::ViewState;
        let committed = io::fixture_tiles_dir();
        let state = |fixture: Option<PathBuf>| {
            let startup = crate::state::Startup { basemap: None, fixture, demo: false };
            AppState::new(startup, None, None, None, ViewState::INITIAL, [1100.0, 720.0])
        };
        let fixture = for_state(&state(Some(committed.clone())));
        let mut world = Fetch::start(&*fixture, io::OPENFREEMAP.into(), TileIndex::ROOT);
        assert_eq!(answer(&mut world), Ok(Some(std::fs::read(committed.join("openfreemap/0/0/0.pbf")).unwrap())));
        let offline = for_state(&state(None));
        let mut nothing = Fetch::start(&*offline, io::OPENFREEMAP.into(), TileIndex::ROOT);
        assert_eq!(answer(&mut nothing), Ok(None));
    }

    /// Overture's buildings (R6): a `--fixture` folder's archive answers through the state's
    /// fetcher with the very bytes `backend::overture` reads from it (the map's own PMTiles reader
    /// against the pmtiles crate's), a tile it lacks as nothing; the map's, the scene's and the
    /// backend's names and constants are one.
    #[test]
    fn a_fixture_folder_s_overture_tiles_are_the_backend_s() {
        use rustradar_backend::overture as backend;
        use rustradar_map::layers::roofs;
        use rustradar_scene::camera::ViewState;
        use rustradar_scene::overture as scene;
        assert_eq!(roofs::OVERTURE, backend::NAME);
        assert_eq!(roofs::OVERTURE_FIXTURE, backend::FIXTURE);
        assert_eq!((scene::CATALOG, scene::KNOWN_RELEASE, scene::ZOOM, scene::OVERTURE_ATTRIBUTION), (backend::CATALOG, backend::KNOWN_RELEASE, backend::ZOOM, backend::ATTRIBUTION));
        assert_eq!(scene::archive_url(scene::KNOWN_RELEASE), backend::archive_url(backend::ARCHIVE, backend::KNOWN_RELEASE));
        let data = io::fixture_tiles_dir().parent().unwrap().to_owned();
        let archive = backend::Archive::fixture(&data).expect("the committed archive");
        let startup = crate::state::Startup { basemap: None, fixture: Some(data), demo: false };
        let fetcher = for_state(&AppState::new(startup, None, None, None, ViewState::INITIAL, [1100.0, 720.0]));
        for (x, y) in [(9013, 4817), (9013, 4818), (9014, 4817), (9014, 4818), (9015, 4818)] {
            let mut fetch = Fetch::start(&*fetcher, roofs::OVERTURE.into(), TileIndex::new(x, y, backend::ZOOM));
            assert_eq!(answer(&mut fetch), archive.tile_now(x, y).map_err(FetchError::Failed), "{x}/{y}");
        }
    }

    /// The land cover (R8): the backend's tables and rules are the scene's (`backend::landcover`
    /// cannot see `scene::landcover`, so they are kept equal by hand), its decode of every fixture
    /// picture the map's, and a `--fixture` folder's land cover through the state's fetcher the
    /// backend's put together from the same pictures.
    #[test]
    fn a_fixture_folder_s_land_cover_is_the_backend_s() {
        use rustradar_backend::landcover as backend;
        use rustradar_map::layers::ground::landcover as map;
        use rustradar_scene::camera::ViewState;
        use rustradar_scene::landcover as scene;
        assert_eq!((map::LAND_COVER, map::NMD, map::WORLDCOVER), (backend::NAME, backend::NMD, backend::WORLDCOVER));
        assert_eq!((scene::SIZE, scene::MAX_CACHED, scene::WORLD_COVER_MIN_ZOOM), (backend::SIZE, backend::MAX_CACHED, backend::WORLD_COVER_MIN_ZOOM));
        assert_eq!((scene::LANDCOVER_ATTRIBUTION, scene::WORLD_COVER_SEARCH), (backend::ATTRIBUTION, backend::WORLD_COVER_SEARCH));
        assert_eq!(scene::WORLD_COVER_REGISTER, backend::register_url(backend::PC));
        for z in [6, 10, 14] {
            let n = 1u32 << z;
            for (x, y) in (0..40).map(|i| ((i * 7919 + 13) % n, (i * 104_729 + 7) % n)).chain([(n * 35 / 64, n * 18 / 64), (n / 2, n / 2)]) {
                assert_eq!(scene::in_sweden(x, y, z), backend::in_sweden(x, y, z), "{z}/{x}/{y}");
                assert_eq!(scene::bbox(x, y, z), backend::bbox(x, y, z));
                assert_eq!(scene::nmd_url(x, y, z), backend::nmd_url(x, y, z));
                assert_eq!(scene::world_cover_url("pc1.id", x, y, z), backend::world_cover_url("pc1.id", x, y, z));
            }
        }
        // the colours: every legend colour and its neighbours, and a spread of all the others
        let colours = (0..200_000u32).map(|i| i.wrapping_mul(2_654_435_761) >> 8).chain([0x6E8C05, 0x6E8C04, 0x6E8C06, 0xC500FF, 0x8ACCFA, 0xFFFFBE]);
        for colour in colours {
            let [_, r, g, b] = colour.to_be_bytes();
            assert_eq!(scene::nmd_classes(&[r, g, b, 255]), [backend::nmd_class(colour)], "#{colour:06X}");
        }
        for code in 0..=255u8 {
            assert_eq!(scene::world_cover_classes(&[code, code, code, 255]), backend::world_cover_classes(&[code, code, code, 255]), "{code}");
        }
        assert_eq!(scene::combine(Some(vec![0, 1, 0]), Some(vec![13, 4, 7])).map(|g| g.classes), backend::combine(Some(vec![0, 1, 0]), Some(vec![13, 4, 7])));
        // every fixture picture decoded alike, and the fixture folder's land cover the backend's
        let data = io::fixture_tiles_dir().parent().unwrap().to_owned();
        let dir = DirTiles::at(fixture_tiles(&data));
        let startup = crate::state::Startup { basemap: None, fixture: Some(data), demo: false };
        let fetcher = for_state(&AppState::new(startup, None, None, None, ViewState::INITIAL, [1100.0, 720.0]));
        for tile in dir.tiles(map::NMD) {
            let read = |source: &str| std::fs::read(dir.root().join(format!("{source}/{}/{}/{}.png", tile.z, tile.x, tile.y))).ok();
            let nmd = read(map::NMD).expect("NMD's picture");
            let theirs = backend::pixels(&nmd).map(|px| backend::nmd_classes(&px));
            assert_eq!(map::land_classes(&nmd, true), theirs, "{tile:?}");
            let world = read(map::WORLDCOVER).map(|png| (map::land_classes(&png, false), backend::pixels(&png).map(|px| backend::world_cover_classes(&px))));
            if let Some((ours, theirs)) = &world {
                assert_eq!(ours, theirs, "{tile:?}");
            }
            let want = if backend::wants_world_cover(theirs.as_deref()) { backend::combine(theirs, world.and_then(|(_, w)| w)) } else { theirs };
            let mut fetch = Fetch::start(&*fetcher, map::LAND_COVER.into(), tile);
            assert_eq!(answer(&mut fetch), Ok(want), "{tile:?}");
        }
    }
}
