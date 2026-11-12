//! Overture Maps' buildings, for how tall Streets GL's buildings are where its tiles don't say
//! (item R6; doc 04 §7.7; `overture.ts`): Overture's own PMTiles archive of its buildings (©
//! OpenStreetMap, © Overture Maps Foundation, ODbL), which adds machine-learned heights
//! (Microsoft's) to OpenStreetMap's, read a zoom-[`ZOOM`] tile at a time with HTTP range requests.
//!
//! What today's `buildings()` and `tileBuildings` do, and so this module:
//! - **The release** is the one the catalogue at [`CATALOG`] names as `latest`, asked once; where
//!   the catalogue can't be read (no answer, a status that is not ok, no JSON) it is
//!   [`KNOWN_RELEASE`] ([`release_of`]). The archive is [`archive_url`] of it.
//! - **A tile** ([`Archive::tile`]): the archive's header and root directory read once (the first
//!   16 KiB), a leaf directory where the root points to one, then the tile's bytes, gunzipped (the
//!   archive's tiles are gzip'd MVT with the layers `building` and `building_part`). `None` where
//!   the archive has no tile there, or for a zoom it doesn't hold. Directories are kept as
//!   pmtiles.js keeps them: the [`DIRECTORIES_KEPT`] last used.
//! - The tiles themselves are decoded and kept by the map (`map::layers::roofs`: their buildings
//!   with a height or floors, `scene::overture::TILES_CACHED` tiles, as `tileBuildings` keeps its
//!   promises), which asks for them through the app's tile fetcher as the source [`NAME`]; the
//!   app's network fetcher hands those asks to [`tile`] ([`global`]). A `--fixture` folder's
//!   [`FIXTURE`] is read by the map's own fixture reader (`map::io::DirTiles`, through
//!   `map::layers::roofs::archive_tile`); the app's tests check that it hands out the very bytes
//!   [`Archive::file`] reads.
//!
//! `scene::overture` has the same constants and the same rule for the release, ported with
//! `overture.ts` in stage 1; this module cannot see it (the backend depends on `core` only), so
//! the two are kept equal by hand (the app's tests compare them).
//!
//! Deviations: an archive whose header could not be read is tried again at the next ask
//! (pmtiles.js keeps the failed header's promise, so today one failure at the first ask leaves the
//! buildings without Overture's heights until the page is reloaded); every ask gives up after
//! [`TIMEOUT`] (a `fetch` has no time limit); the release is asked for at the first tile, not at
//! the first building tile's load (the same moment in practice).

use std::collections::HashMap;
use std::fs::File;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use pmtiles::{AsyncPmTilesReader, DirEntry, Directory, DirectoryCache, HttpBackend, PmTilesReader, PmtResult, TileCoord, TileId};
use serde_json::Value;

use crate::CmdResult;

/// The source's name: the map's `layers::roofs::OVERTURE`, as the app's tile fetcher is asked it.
pub const NAME: &str = "overture";
/// The catalogue naming the latest release (`CATALOG`).
pub const CATALOG: &str = "https://stac.overturemaps.org/catalog.json";
/// The release asked for when the catalogue can't be read (`KNOWN_RELEASE`).
pub const KNOWN_RELEASE: &str = "2026-09-23.1";
/// A release's archive of buildings; `{release}` is the release.
pub const ARCHIVE: &str = "https://tiles.overturemaps.org/{release}/buildings.pmtiles";
/// The zoom of the tiles read (`ZOOM`).
pub const ZOOM: u32 = 14;
/// Their credit (`OVERTURE_ATTRIBUTION`).
pub const ATTRIBUTION: &str = "Building heights: © Overture Maps Foundation, © OpenStreetMap (ODbL)";
/// A `--fixture` folder's archive, as `cargo xtask fixtures` keeps it (with the catalogue it was
/// picked by, `overture/catalog.json`).
pub const FIXTURE: &str = "overture/buildings.pmtiles";
/// Directories kept (pmtiles.js's `SharedPromiseCache` default, its header counted among them).
pub const DIRECTORIES_KEPT: usize = 100;
/// How long an ask (the catalogue, or one range of the archive) may take.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// The release the catalogue names (`c?.latest ?? KNOWN_RELEASE`, `latest` as JavaScript's
/// `String` has it; none where it can't be read).
pub fn release_of(catalog: Option<&Value>) -> String {
    match catalog.and_then(|c| c.as_object()).and_then(|c| c.get("latest")) {
        None | Some(Value::Null) => KNOWN_RELEASE.to_owned(),
        Some(latest) => js_string(latest),
    }
}

/// JavaScript's `String(v)` of a JSON value.
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

/// The archive of `release`, from the template `archive` ([`ARCHIVE`]).
pub fn archive_url(archive: &str, release: &str) -> String {
    archive.replace("{release}", release)
}

/// The release the catalogue at `url` names, asked with `http`: [`KNOWN_RELEASE`] where it
/// can't be read.
pub async fn release(http: &reqwest::Client, url: &str) -> String {
    let json = async {
        let response = http.get(url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.json::<Value>().await.ok()
    };
    release_of(json.await.as_ref())
}

/// The directories an archive keeps (pmtiles.js's `SharedPromiseCache`): by offset, the
/// [`DIRECTORIES_KEPT`] last used.
#[derive(Default)]
pub struct Directories {
    kept: Mutex<Kept>,
}

/// Each kept directory by its offset, with when it was last used; and the count of uses.
type Kept = (HashMap<u64, (u64, Arc<Directory>)>, u64);

impl Directories {
    /// How many are kept now.
    pub fn len(&self) -> usize {
        self.kept.lock().unwrap_or_else(PoisonError::into_inner).0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl DirectoryCache for Directories {
    async fn get_dir_entry_or_insert(
        &self,
        offset: u64,
        tile_id: TileId,
        fetcher: impl Future<Output = PmtResult<Directory>> + Send,
    ) -> PmtResult<Option<DirEntry>> {
        {
            let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
            kept.1 += 1;
            let used = kept.1;
            if let Some((last, directory)) = kept.0.get_mut(&offset) {
                *last = used;
                return Ok(directory.find_tile_id(tile_id).copied());
            }
        }
        let directory = Arc::new(fetcher.await?);
        let entry = directory.find_tile_id(tile_id).copied();
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.1 += 1;
        let used = kept.1;
        kept.0.insert(offset, (used, directory));
        // (the header and root directory are held apart, but counted as pmtiles.js counts them)
        while kept.0.len() + 1 > DIRECTORIES_KEPT {
            let Some(oldest) = kept.0.iter().min_by_key(|(_, (last, _))| *last).map(|(&k, _)| k) else { break };
            kept.0.remove(&oldest);
        }
        Ok(entry)
    }
}

/// The archive read over HTTP.
type HttpReader = AsyncPmTilesReader<HttpBackend, Directories>;
/// A local archive.
type FileReader = PmTilesReader<File, Directories>;

enum Source {
    /// The network: the catalogue at `catalog` (its release asked once), the archive from the
    /// template `archive`.
    Network { http: reqwest::Client, catalog: String, archive: String, release: tokio::sync::OnceCell<String>, reader: tokio::sync::OnceCell<Box<HttpReader>> },
    /// A file (the fixtures).
    File { path: PathBuf, reader: Mutex<Option<Arc<FileReader>>> },
}

/// One PMTiles archive of Overture's buildings (see the module docs).
pub struct Archive {
    source: Source,
}

impl std::fmt::Debug for Archive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.source {
            Source::Network { catalog, archive, .. } => f.debug_struct("Archive").field("catalog", catalog).field("archive", archive).finish_non_exhaustive(),
            Source::File { path, .. } => f.debug_struct("Archive").field("path", path).finish_non_exhaustive(),
        }
    }
}

impl Archive {
    /// The latest release's archive ([`CATALOG`], [`ARCHIVE`]) asked with `http`.
    pub fn latest(http: reqwest::Client) -> Self {
        Self::network(http, CATALOG, ARCHIVE)
    }

    /// The release the catalogue at `catalog` names, its archive from the template `archive`
    /// (`{release}` in it), asked with `http`.
    pub fn network(http: reqwest::Client, catalog: &str, archive: &str) -> Self {
        Archive {
            source: Source::Network {
                http,
                catalog: catalog.to_owned(),
                archive: archive.to_owned(),
                release: tokio::sync::OnceCell::new(),
                reader: tokio::sync::OnceCell::new(),
            },
        }
    }

    /// The archive in the file at `path`.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Archive { source: Source::File { path: path.into(), reader: Mutex::new(None) } }
    }

    /// A `--fixture` folder's archive ([`FIXTURE`] in `dir`, or beside `dir` when `dir` is the
    /// folder's `tiles`), where it has one.
    pub fn fixture(dir: &Path) -> Option<Self> {
        [dir.join(FIXTURE), dir.join("..").join(FIXTURE)].into_iter().find(|p| p.is_file()).map(Self::file)
    }

    /// The release read (the network's, once asked; a file has none of its own).
    pub fn release(&self) -> Option<&str> {
        match &self.source {
            Source::Network { release, .. } => release.get().map(String::as_str),
            Source::File { .. } => None,
        }
    }

    /// The zoom-[`ZOOM`] tile `x`/`y`: its MVT, gunzipped; `None` where the archive has none
    /// (`getZxy`'s `undefined`), or why it can't be had.
    pub async fn tile(&self, x: u32, y: u32) -> CmdResult<Option<Vec<u8>>> {
        let coord = TileCoord::new(ZOOM as u8, x, y).map_err(|e| e.to_string())?;
        match &self.source {
            Source::Network { http, catalog, archive, release, reader } => {
                let release = release
                    .get_or_init(|| async { tokio::time::timeout(TIMEOUT, self::release(http, catalog)).await.unwrap_or_else(|_| KNOWN_RELEASE.to_owned()) })
                    .await;
                let url = archive_url(archive, release);
                // (a failure leaves the cell empty: the next ask reads the header again)
                let reader = reader
                    .get_or_try_init(|| async {
                        let read = HttpReader::new_with_url_and_cache(http.clone(), url.as_str(), Directories::default());
                        let reader = tokio::time::timeout(TIMEOUT, read).await.map_err(|_| format!("{url}: no answer"))?.map_err(|e| format!("{url}: {e}"))?;
                        Ok::<_, String>(Box::new(reader))
                    })
                    .await?;
                // (`getZxy` answers nothing for a zoom outside the archive's)
                if !holds_zoom(reader.get_header()) {
                    return Ok(None);
                }
                let tile = tokio::time::timeout(TIMEOUT, reader.get_tile_decompressed(coord)).await.map_err(|_| format!("{url}: no answer"))?;
                tile.map(|t| t.map(|b| b.to_vec())).map_err(|e| format!("{url}: {e}"))
            }
            Source::File { .. } => self.tile_now(x, y),
        }
    }

    /// [`Self::tile`] on the calling thread: a file's (a network archive answers with an error).
    pub fn tile_now(&self, x: u32, y: u32) -> CmdResult<Option<Vec<u8>>> {
        let Source::File { path, reader } = &self.source else {
            return Err("a network archive is read on the runtime".to_owned());
        };
        let coord = TileCoord::new(ZOOM as u8, x, y).map_err(|e| e.to_string())?;
        let reader = {
            let mut held = reader.lock().unwrap_or_else(PoisonError::into_inner);
            match &*held {
                Some(reader) => Arc::clone(reader),
                None => {
                    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
                    let opened = Arc::new(FileReader::try_from_source_and_cache(file, Directories::default()).map_err(|e| format!("{}: {e}", path.display()))?);
                    *held = Some(Arc::clone(&opened));
                    opened
                }
            }
        };
        if !holds_zoom(reader.get_header()) {
            return Ok(None);
        }
        reader.get_tile_decompressed(coord).map(|t| t.map(|b| b.to_vec())).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Whether an archive has tiles at [`ZOOM`].
fn holds_zoom(header: &pmtiles::Header) -> bool {
    (header.min_zoom..=header.max_zoom).contains(&(ZOOM as u8))
}

/// The app's archive: the latest release's, through the app's HTTP client
/// (`fr24::client_builder()`, doc 07 §2.3), made at the first ask.
pub fn global() -> Option<&'static Archive> {
    static ARCHIVE: OnceLock<Option<Archive>> = OnceLock::new();
    ARCHIVE
        .get_or_init(|| {
            let http = fr24::client_builder()
                .user_agent(concat!("RustRadar/", env!("CARGO_PKG_VERSION"), " (+https://github.com/felixgoff/RustRadar)"))
                .timeout(TIMEOUT)
                .build()
                .ok()?;
            Some(Archive::latest(http))
        })
        .as_ref()
}

/// Tile `z/x/y` of the app's archive ([`global`]): `Ok(None)` at once for a zoom other than
/// [`ZOOM`] (nothing is asked).
pub async fn tile(z: u32, x: u32, y: u32) -> CmdResult<Option<Vec<u8>>> {
    if z != ZOOM {
        return Ok(None);
    }
    let archive = global().ok_or("no HTTP client for Overture")?;
    archive.tile(x, y).await
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    /// The committed fixture: 4 zoom-14 tiles of release 2026-10-21.0 (plan, "Fixtures for stages
    /// 8-10").
    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../map/tests/data").join(FIXTURE)
    }

    /// The fixture's tiles.
    const HELD: [(u32, u32); 4] = [(9013, 4817), (9013, 4818), (9014, 4817), (9014, 4818)];

    /// The names of an MVT's layers, in order (a tile's `Layer` messages, field 3, their name
    /// field 1).
    fn layer_names(mvt: &[u8]) -> Vec<String> {
        rustradar_core::mvt::layers(mvt).map(|l| l.name.to_owned()).collect()
    }

    #[test]
    fn the_latest_release_else_the_known_one() {
        assert_eq!(release_of(Some(&json!({"latest": "2027-01-01.0"}))), "2027-01-01.0");
        assert_eq!(release_of(Some(&json!({"latest": null}))), KNOWN_RELEASE);
        assert_eq!(release_of(Some(&json!({}))), KNOWN_RELEASE);
        assert_eq!(release_of(Some(&json!(["x"]))), KNOWN_RELEASE);
        assert_eq!(release_of(None), KNOWN_RELEASE);
        // `${release}` of what is not a string
        assert_eq!(release_of(Some(&json!({"latest": 2026.5}))), "2026.5");
        assert_eq!(release_of(Some(&json!({"latest": true}))), "true");
        assert_eq!(release_of(Some(&json!({"latest": [1, null, "a"]}))), "1,,a");
        assert_eq!(archive_url(ARCHIVE, KNOWN_RELEASE), "https://tiles.overturemaps.org/2026-09-23.1/buildings.pmtiles");
        // the catalogue the fixtures were picked by names the fixtures' release
        let catalog: Value = serde_json::from_slice(&std::fs::read(fixture().with_file_name("catalog.json")).unwrap()).unwrap();
        assert_eq!(release_of(Some(&catalog)), "2026-10-21.0");
    }

    /// The fixture file's four tiles are MVT with Overture's two layers; every other tile is
    /// none, other zooms too; the file is opened once.
    #[test]
    fn a_local_archive_s_tiles() {
        let archive = Archive::file(fixture());
        for (x, y) in HELD {
            let mvt = archive.tile_now(x, y).expect("read").expect("held");
            assert_eq!(layer_names(&mvt), ["building", "building_part"], "{x}/{y}");
        }
        assert_eq!(archive.tile_now(9015, 4817), Ok(None));
        assert_eq!(archive.tile_now(0, 0), Ok(None));
        assert!(Archive::file("/nonexistent/buildings.pmtiles").tile_now(9013, 4817).is_err());
        // a fixture folder's, or its tiles folder's neighbour
        let data = fixture().parent().unwrap().parent().unwrap().to_owned();
        assert!(Archive::fixture(&data).is_some() && Archive::fixture(&data.join("tiles")).is_some());
        assert!(Archive::fixture(Path::new("/nonexistent")).is_none());
    }

    /// An HTTP server on 127.0.0.1 serving `file` with range requests (206) at
    /// `/<release>/buildings.pmtiles` and the catalogue `catalog` at `/catalog.json` (500 when
    /// none), noting each request's path and range.
    async fn server(file: Vec<u8>, catalog: Option<Value>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (noted, file) = (Arc::clone(&asked), Arc::new(file));
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (noted, file, catalog) = (Arc::clone(&noted), Arc::clone(&file), catalog.clone());
                tokio::spawn(async move {
                    let (mut head, mut buf) = (Vec::new(), [0u8; 1024]);
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    let head = String::from_utf8_lossy(&head).to_string();
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    let range = head
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_owned))
                        .and_then(|r| r.split_once('-').map(|(a, b)| (a.trim().parse::<usize>().unwrap(), b.trim().parse::<usize>().unwrap())));
                    noted.lock().unwrap().push(format!("{path} {range:?}"));
                    let (status, body) = match (path.as_str(), range) {
                        ("/catalog.json", _) => match &catalog {
                            Some(c) => ("200 OK", serde_json::to_vec(c).unwrap()),
                            None => ("500 Broken", Vec::new()),
                        },
                        (p, Some((from, to))) if p.ends_with("/buildings.pmtiles") && p.starts_with("/2026-10-21.0/") => {
                            ("206 Partial Content", file[from.min(file.len())..(to + 1).min(file.len())].to_vec())
                        }
                        _ => ("404 Not Found", Vec::new()),
                    };
                    let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        (base, asked)
    }

    fn client() -> reqwest::Client {
        fr24::client_builder().no_proxy().timeout(Duration::from_secs(10)).build().unwrap()
    }

    /// Over HTTP with range requests, as today's `PMTiles(url).getZxy(14, x, y)`: the catalogue's
    /// release asked once, the header and root directory in one read of the first 16 KiB, then
    /// each tile's own range, the same bytes as the file's; a tile the archive lacks is none; a
    /// zoom other than 14 asks nothing.
    #[tokio::test]
    async fn an_archive_over_http_reads_ranges() {
        let bytes = std::fs::read(fixture()).unwrap();
        let (base, asked) = server(bytes.clone(), Some(json!({"latest": "2026-10-21.0"}))).await;
        let archive = Archive::network(client(), &format!("{base}/catalog.json"), &format!("{base}/{{release}}/buildings.pmtiles"));
        let local = Archive::file(fixture());
        for (x, y) in HELD {
            assert_eq!(archive.tile(x, y).await, local.tile_now(x, y), "{x}/{y}");
        }
        assert_eq!(archive.tile(9015, 4817).await, Ok(None));
        assert_eq!(archive.release(), Some("2026-10-21.0"));
        let asked = asked.lock().unwrap().clone();
        assert_eq!(asked.iter().filter(|a| a.starts_with("/catalog.json")).count(), 1, "{asked:?}");
        assert_eq!(asked[1], "/2026-10-21.0/buildings.pmtiles Some((0, 16383))", "{asked:?}");
        // one directory (the root) and a range per tile held
        assert_eq!(asked.len(), 2 + HELD.len(), "{asked:?}");
        assert_eq!(tile(13, 4506, 2408).await, Ok(None));
    }

    /// Where the catalogue can't be read the known release is asked for (and, the server not
    /// having it, every tile fails); a failed header is read again at the next ask.
    #[tokio::test]
    async fn without_a_catalogue_the_known_release() {
        let bytes = std::fs::read(fixture()).unwrap();
        let (base, asked) = server(bytes, None).await;
        let archive = Archive::network(client(), &format!("{base}/catalog.json"), &format!("{base}/{{release}}/buildings.pmtiles"));
        assert!(archive.tile(9013, 4817).await.is_err());
        assert!(archive.tile(9013, 4817).await.is_err());
        assert_eq!(archive.release(), Some(KNOWN_RELEASE));
        let asked = asked.lock().unwrap().clone();
        let headers = asked.iter().filter(|a| a.starts_with(&format!("/{KNOWN_RELEASE}/"))).count();
        assert_eq!((asked.iter().filter(|a| a.starts_with("/catalog.json")).count(), headers), (1, 2), "{asked:?}");
    }

    /// The directories kept: the last used, at most [`DIRECTORIES_KEPT`] with the header.
    #[tokio::test]
    async fn directories_kept_are_the_last_used() {
        let dirs = Directories::default();
        let fetched = AtomicUsize::new(0);
        // (counted when awaited: a directory kept is not fetched)
        let fetch = || async {
            fetched.fetch_add(1, Ordering::SeqCst);
            Ok(Directory::default())
        };
        for offset in 0..DIRECTORIES_KEPT as u64 + 20 {
            assert_eq!(dirs.get_dir_entry_or_insert(offset, TileId::new(0).unwrap(), fetch()).await.unwrap(), None);
            // the first stays in use
            dirs.get_dir_entry_or_insert(0, TileId::new(0).unwrap(), fetch()).await.unwrap();
        }
        assert_eq!(dirs.len(), DIRECTORIES_KEPT - 1);
        let before = fetched.load(Ordering::SeqCst);
        dirs.get_dir_entry_or_insert(0, TileId::new(0).unwrap(), fetch()).await.unwrap();
        dirs.get_dir_entry_or_insert(DIRECTORIES_KEPT as u64 + 19, TileId::new(0).unwrap(), fetch()).await.unwrap();
        assert_eq!(fetched.load(Ordering::SeqCst), before);
        dirs.get_dir_entry_or_insert(1, TileId::new(0).unwrap(), fetch()).await.unwrap();
        assert_eq!(fetched.load(Ordering::SeqCst), before + 1);
    }
}
