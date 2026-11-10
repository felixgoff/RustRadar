//! Map tiles from the network, kept on disk: what the webview's HTTP cache did for the basemap,
//! imagery, terrain and radar tiles (plan 1.4.8, doc 07 §11.5). `app` serves `map::io::TileFetcher`
//! with it.
//!
//! - One cached GET, [`Tiles::get`] ([`Tiles::tile`] for a [`Source`]'s `z/x/y`): a file under
//!   `<cache>/tiles` answers while it is fresh, without asking the server. Otherwise the server is
//!   asked, and what it sends is kept for its `Cache-Control: max-age` (less its `Age`), else for the
//!   source's default, [`DEFAULT_FRESH`] (7 days). `no-cache` is kept but stale at once; `no-store`
//!   is not kept.
//! - A stale file is used when the download fails: no connection, a timeout, any status but 2xx and
//!   404. A file kept for another URL is stale too: OpenFreeMap's tile URLs change with each planet
//!   build (its tiles say `max-age` ten years, its TileJSON an hour), and a new build is fetched
//!   again, as the browser did, but the old tile beats none.
//! - 404 (and 204) is `Ok(None)`: nothing there. It is kept like a tile, so it is not asked again
//!   while fresh.
//! - At most [`PER_HOST`] requests to one host at once ([`Tiles::set_host_limit`] changes it). A
//!   host answering 429, or 503 with a `Retry-After`, is left alone for as long as it asks
//!   ([`BACKOFF`] without a number, at most [`MAX_BACKOFF`]); meanwhile its requests fail at once and
//!   take the stale file where there is one.
//! - All the files together are kept under a cap ([`CAP_BYTES`]: 512 MB on desktop, 128 MB on
//!   phones). Past it, the least recently used go first, down to nine tenths of the cap. A file's
//!   modification time is when it was last used.
//! - OpenFreeMap's tile URL is read from its TileJSON ([`Tiles::openfreemap`]), never written here;
//!   the TileJSON is kept like a tile, for a day unless it says otherwise.
//!
//! On disk, `<cache>/tiles/<source>/<z>/<x>/<y>`: a 24-byte header (magic `RRT1`, the kind, the
//! expiry in Unix ms, a hash of the URL) and then the body as received (reqwest has already undone
//! any `Content-Encoding`; a body that is itself gzip is kept as it is).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::header::{AGE, CACHE_CONTROL, HeaderMap, RETRY_AFTER};
use tokio::sync::Semaphore;

/// OpenFreeMap's TileJSON: its `tiles[0]` is the vector tiles' URL, which changes with each planet
/// build (doc 01 §5.6).
pub const OPENFREEMAP_TILEJSON: &str = "https://tiles.openfreemap.org/planet";
/// The folder OpenFreeMap's tiles are kept in.
pub const OPENFREEMAP: &str = "openfreemap";
/// How long a response without `max-age` is fresh.
pub const DEFAULT_FRESH: Duration = Duration::from_secs(7 * 24 * 3600);
/// How long a TileJSON without `max-age` is fresh: the planet build changes, a day is soon enough
/// to notice (as `building_lod` keeps it).
pub const TILEJSON_FRESH: Duration = Duration::from_secs(24 * 3600);
/// The most the files on disk may take together.
pub const CAP_BYTES: u64 = if cfg!(any(target_os = "android", target_os = "ios")) { 128 << 20 } else { 512 << 20 };
/// Requests to one host at once. As many as the busiest tile layer asked for (the imagery's
/// `maxRequests: 12`), so no layer is slower than it was, while the layers sharing a host (the
/// vector map, labels and buildings on OpenFreeMap; the hillshade and terrain fallback on the
/// terrarium bucket) together ask no more than that.
pub const PER_HOST: usize = 12;
/// How long a host that asked to slow down without saying for how long is left alone.
pub const BACKOFF: Duration = Duration::from_secs(60);
/// The longest a host is left alone, whatever it asks.
pub const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TIMEOUT: Duration = Duration::from_secs(20);

const MAGIC: &[u8; 4] = b"RRT1";
const HEADER: usize = 24;
const FOUND: u8 = 1;
const NOT_FOUND: u8 = 2;

/// Where a kind of tile comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// The folder under `<cache>/tiles` its tiles are kept in: letters, digits, `-`, `_` and `.`.
    pub name: String,
    /// The tile URL with `{z}`, `{x}` and `{y}` in it (in any order: Esri's is `{z}/{y}/{x}`).
    pub template: String,
    /// How long a tile is fresh when the server does not say.
    pub fresh: Duration,
}

impl Source {
    pub fn new(name: impl Into<String>, template: impl Into<String>) -> Self {
        Self { name: name.into(), template: template.into(), fresh: DEFAULT_FRESH }
    }

    /// The URL of tile `z/x/y`.
    pub fn url(&self, z: u32, x: u32, y: u32) -> String {
        self.template.replace("{z}", &z.to_string()).replace("{x}", &x.to_string()).replace("{y}", &y.to_string())
    }
}

/// What a TileJSON says that the app reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TileJson {
    /// Tile URL templates; the first is the one used.
    pub tiles: Vec<String>,
    pub minzoom: Option<u32>,
    pub maxzoom: Option<u32>,
}

impl TileJson {
    pub fn parse(body: &[u8]) -> Result<Self, String> {
        let json: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        let zoom = |key: &str| json[key].as_f64().filter(|z| *z >= 0.0).map(|z| z as u32);
        let tiles = json["tiles"].as_array().map(|t| t.iter().filter_map(|t| Some(t.as_str()?.to_owned())).collect());
        Ok(Self { tiles: tiles.unwrap_or_default(), minzoom: zoom("minzoom"), maxzoom: zoom("maxzoom") })
    }
}

/// How a [`Tiles`] is set up; [`Config::new`] has the app's settings.
pub struct Config {
    /// The folder the files are kept in: `<cache>/tiles`.
    pub root: PathBuf,
    pub cap_bytes: u64,
    pub per_host: usize,
    /// The HTTP client; None builds the app's (through `fr24::client_builder()`, doc 07 §2.3).
    pub client: Option<reqwest::Client>,
}

impl Config {
    pub fn new(cache_dir: &Path) -> Self {
        Self { root: cache_dir.join("tiles"), cap_bytes: CAP_BYTES, per_host: PER_HOST, client: None }
    }
}

/// The tile cache. Cheap to clone; every clone is the same cache.
#[derive(Clone)]
pub struct Tiles {
    inner: Arc<Inner>,
}

struct Inner {
    root: PathBuf,
    cap: u64,
    per_host: usize,
    http: reqwest::Client,
    hosts: Mutex<HashMap<String, Arc<Host>>>,
    /// Bytes on disk, as last counted and kept up since; None until the first count.
    total: Mutex<Option<u64>>,
    /// One trim at a time; and whether one is already on its way.
    trimming: tokio::sync::Mutex<()>,
    trim_queued: AtomicBool,
}

/// One host's turns and its pause.
struct Host {
    turns: Semaphore,
    paused_until: Mutex<Option<Instant>>,
}

impl Host {
    fn new(max: usize, paused_until: Option<Instant>) -> Self {
        Self { turns: Semaphore::new(max.max(1)), paused_until: Mutex::new(paused_until) }
    }

    /// An error while the host has asked to be left alone.
    fn check(&self, name: &str) -> Result<(), String> {
        match *self.paused_until.lock().unwrap() {
            Some(until) if Instant::now() < until => Err(format!("{name} asked to wait")),
            _ => Ok(()),
        }
    }

    fn pause(&self, wait: Duration) {
        let until = Instant::now() + wait.clamp(Duration::from_secs(1), MAX_BACKOFF);
        let mut paused = self.paused_until.lock().unwrap();
        *paused = Some(paused.map_or(until, |p| p.max(until)));
    }
}

/// What came back from the server: the body (None: not found) and how long to keep it (None:
/// not at all).
struct Downloaded {
    body: Option<Vec<u8>>,
    keep: Option<Duration>,
}

/// A file read back: the body (None: not found), until when it is fresh, and its URL's hash.
#[derive(Debug, PartialEq)]
struct Entry {
    body: Option<Vec<u8>>,
    expires_ms: i64,
    url_hash: u64,
}

impl Tiles {
    /// The app's tile cache in `<cache_dir>/tiles`.
    pub fn new(cache_dir: &Path) -> Result<Self, String> {
        Self::with_config(Config::new(cache_dir))
    }

    pub fn with_config(config: Config) -> Result<Self, String> {
        let http = match config.client {
            Some(client) => client,
            None => fr24::client_builder()
                .user_agent(concat!("RustRadar/", env!("CARGO_PKG_VERSION"), " (+https://github.com/felixgoff/RustRadar)"))
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(TIMEOUT)
                .build()
                .map_err(|e| e.to_string())?,
        };
        Ok(Self {
            inner: Arc::new(Inner {
                root: config.root,
                cap: config.cap_bytes,
                per_host: config.per_host,
                http,
                hosts: Mutex::new(HashMap::new()),
                total: Mutex::new(None),
                trimming: tokio::sync::Mutex::new(()),
                trim_queued: AtomicBool::new(false),
            }),
        })
    }

    /// The folder the files are kept in.
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Lets `max` requests to `host` (a host name, as in its URLs) run at once instead of
    /// [`PER_HOST`]. For requests started after the call.
    pub fn set_host_limit(&self, host: &str, max: usize) {
        let mut hosts = self.inner.hosts.lock().unwrap();
        let paused = hosts.get(host).and_then(|h| *h.paused_until.lock().unwrap());
        hosts.insert(host.to_owned(), Arc::new(Host::new(max, paused)));
    }

    /// Tile `z/x/y` of `source`: the bytes, or None where the server has none (404).
    pub async fn tile(&self, source: &Source, z: u32, x: u32, y: u32) -> Result<Option<Vec<u8>>, String> {
        self.get(&format!("{}/{z}/{x}/{y}", source.name), &source.url(z, x, y), source.fresh).await
    }

    /// The cached GET: `url`, kept at `path` under the root (`/`-separated parts of letters,
    /// digits, `-`, `_` and `.`, none starting with a dot). `fresh` is how long the answer is fresh
    /// when the server does not say. None where the server answers 404 (or 204).
    pub async fn get(&self, path: &str, url: &str, fresh: Duration) -> Result<Option<Vec<u8>>, String> {
        let file = self.file(path)?;
        let hash = fnv1a(url.as_bytes());
        let stale = match fs::read(&file).ok().and_then(decode) {
            Some(entry) if entry.url_hash == hash && crate::now_ms() < entry.expires_ms => {
                touch(&file);
                return Ok(entry.body);
            }
            Some(entry) => Some(entry.body),
            None => None,
        };
        match self.download(url, fresh).await {
            Ok(Downloaded { body, keep }) => {
                if let Some(keep) = keep {
                    let keep_ms = i64::try_from(keep.as_millis()).unwrap_or(i64::MAX);
                    let bytes = encode(body.as_deref(), crate::now_ms().saturating_add(keep_ms), hash);
                    if self.keep(&file, &bytes) && !self.inner.trim_queued.swap(true, Ordering::AcqRel) {
                        let tiles = self.clone();
                        tokio::spawn(async move {
                            tiles.inner.trim_queued.store(false, Ordering::Release);
                            tiles.trim().await;
                        });
                    }
                }
                Ok(body)
            }
            // an old tile beats none
            Err(e) => match stale {
                Some(body) => {
                    touch(&file);
                    Ok(body)
                }
                None => Err(e),
            },
        }
    }

    /// The TileJSON at `url`, kept in `<name>/tilejson.json`.
    pub async fn tilejson(&self, name: &str, url: &str) -> Result<TileJson, String> {
        let body = self.get(&format!("{name}/tilejson.json"), url, TILEJSON_FRESH).await?;
        TileJson::parse(&body.ok_or_else(|| format!("{url}: no TileJSON there"))?).map_err(|e| format!("{url}: {e}"))
    }

    /// The source a TileJSON describes: its first tile URL, kept under `name`.
    pub async fn source_from_tilejson(&self, name: &str, url: &str) -> Result<Source, String> {
        let template = self.tilejson(name, url).await?.tiles.into_iter().next();
        Ok(Source::new(name, template.ok_or_else(|| format!("{url}: the TileJSON names no tiles"))?))
    }

    /// OpenFreeMap's vector tiles (doc 01 §5.6), with the URL of the current planet build. Asked once
    /// at start-up; when it fails, the map has no vector layer, labels or buildings.
    pub async fn openfreemap(&self) -> Result<Source, String> {
        self.source_from_tilejson(OPENFREEMAP, OPENFREEMAP_TILEJSON).await
    }

    /// Counts the files on disk and, past the cap, removes the least recently used down to nine
    /// tenths of it. Runs by itself after a write takes the cache past the cap (and after the first
    /// write, to count). The bytes left on disk.
    pub async fn trim(&self) -> u64 {
        let _one = self.inner.trimming.lock().await;
        let (root, cap) = (self.inner.root.clone(), self.inner.cap);
        let Ok(total) = tokio::task::spawn_blocking(move || trim_dir(&root, cap)).await else {
            return self.bytes_on_disk().unwrap_or_default();
        };
        *self.inner.total.lock().unwrap() = Some(total);
        total
    }

    /// The bytes on disk as last counted and kept up since; None before the first count.
    pub fn bytes_on_disk(&self) -> Option<u64> {
        *self.inner.total.lock().unwrap()
    }

    fn file(&self, path: &str) -> Result<PathBuf, String> {
        let part = |p: &str| {
            !p.is_empty() && !p.starts_with('.') && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        };
        if !path.split('/').all(part) {
            return Err(format!("not a tile cache path: {path:?}"));
        }
        Ok(path.split('/').fold(self.inner.root.clone(), |file, p| file.join(p)))
    }

    fn host(&self, name: &str) -> Arc<Host> {
        let mut hosts = self.inner.hosts.lock().unwrap();
        hosts.entry(name.to_owned()).or_insert_with(|| Arc::new(Host::new(self.inner.per_host, None))).clone()
    }

    /// Asks the server, in one of its host's turns.
    async fn download(&self, url: &str, fresh: Duration) -> Result<Downloaded, String> {
        let parsed = reqwest::Url::parse(url).map_err(|e| format!("{url}: {e}"))?;
        let name = parsed.host_str().unwrap_or_default().to_owned();
        let host = self.host(&name);
        host.check(&name)?;
        let _turn = host.turns.acquire().await.map_err(|e| e.to_string())?;
        host.check(&name)?;
        let response = self.inner.http.get(parsed).send().await.map_err(|e| format!("{name}: {e}"))?;
        let status = response.status().as_u16();
        let headers = response.headers();
        let keep = keep_for(&joined(headers, CACHE_CONTROL), headers.get(AGE).and_then(|a| a.to_str().ok()), fresh);
        match status {
            204 | 404 => Ok(Downloaded { body: None, keep }),
            200..=299 => {
                let body = response.bytes().await.map_err(|e| format!("{name}: {e}"))?;
                Ok(Downloaded { body: Some(body.to_vec()), keep })
            }
            429 | 503 => {
                let wait = headers.get(RETRY_AFTER).and_then(|w| w.to_str().ok()).and_then(retry_after);
                if status == 429 || wait.is_some() {
                    host.pause(wait.unwrap_or(BACKOFF));
                }
                Err(format!("{name} answered {status}"))
            }
            _ => Err(format!("{name} answered {status}")),
        }
    }

    /// Writes a file through a temporary one beside it, so a reader never sees half of one. Whether
    /// the cache should be trimmed now: it grew past the cap, or has not been counted yet.
    fn keep(&self, file: &Path, bytes: &[u8]) -> bool {
        let Ok(grew) = write_file(file, bytes) else { return false };
        match self.inner.total.lock().unwrap().as_mut() {
            Some(total) => {
                *total = total.saturating_add_signed(grew);
                *total > self.inner.cap
            }
            None => true,
        }
    }
}

/// How long a response is kept: `max-age` less `Age`; 0 with `no-cache`; not at all with
/// `no-store`; `fresh` when it says nothing.
fn keep_for(cache_control: &str, age: Option<&str>, fresh: Duration) -> Option<Duration> {
    let (mut max_age, mut no_cache) = (None, false);
    for directive in cache_control.split(',') {
        let (name, value) = directive.split_once('=').unwrap_or((directive, ""));
        match name.trim().to_ascii_lowercase().as_str() {
            "no-store" => return None,
            "no-cache" => no_cache = true,
            "max-age" => max_age = value.trim().trim_matches('"').parse::<u64>().ok().or(max_age),
            _ => {}
        }
    }
    if no_cache {
        return Some(Duration::ZERO);
    }
    let age = age.and_then(|a| a.trim().parse::<u64>().ok()).unwrap_or(0);
    Some(max_age.map_or(fresh, |s| Duration::from_secs(s.saturating_sub(age))))
}

/// `Retry-After` in seconds (its date form is not read).
fn retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// A header's values joined with commas, as a list header is read.
fn joined(headers: &HeaderMap, name: reqwest::header::HeaderName) -> String {
    headers.get_all(name).iter().filter_map(|v| v.to_str().ok()).collect::<Vec<_>>().join(",")
}

/// 64-bit FNV-1a: tells the URL a file was kept for.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3))
}

fn encode(body: Option<&[u8]>, expires_ms: i64, url_hash: u64) -> Vec<u8> {
    let body = body.map_or((NOT_FOUND, &[][..]), |b| (FOUND, b));
    let mut out = Vec::with_capacity(HEADER + body.1.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&[body.0, 0, 0, 0]);
    out.extend_from_slice(&expires_ms.to_le_bytes());
    out.extend_from_slice(&url_hash.to_le_bytes());
    out.extend_from_slice(body.1);
    out
}

/// A file's contents back; None for anything this module did not write.
fn decode(mut bytes: Vec<u8>) -> Option<Entry> {
    if bytes.len() < HEADER || bytes[..4] != MAGIC[..] {
        return None;
    }
    let expires_ms = i64::from_le_bytes(bytes[8..16].try_into().ok()?);
    let url_hash = u64::from_le_bytes(bytes[16..24].try_into().ok()?);
    let body = match bytes[4] {
        FOUND => {
            bytes.drain(..HEADER);
            Some(bytes)
        }
        NOT_FOUND => None,
        _ => return None,
    };
    Some(Entry { body, expires_ms, url_hash })
}

/// Writes `bytes` to `file` through a temporary file in its folder. The bytes the folder grew by.
fn write_file(file: &Path, bytes: &[u8]) -> std::io::Result<i64> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let (Some(dir), Some(name)) = (file.parent(), file.file_name()) else {
        return Err(std::io::Error::other("no folder"));
    };
    fs::create_dir_all(dir)?;
    let old = fs::metadata(file).map_or(0, |m| m.len());
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{}.{}.{n}.tmp", name.to_string_lossy(), std::process::id()));
    fs::write(&tmp, bytes)?;
    if let Err(e) = fs::rename(&tmp, file) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(bytes.len() as i64 - old as i64)
}

/// Marks a file used now.
fn touch(file: &Path) {
    let _ = fs::File::options().append(true).open(file).and_then(|f| f.set_modified(SystemTime::now()));
}

/// Counts the files under `root`; past `cap`, removes the least recently used down to nine tenths
/// of it. The bytes left.
fn trim_dir(root: &Path, cap: u64) -> u64 {
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                dirs.push(entry.path());
            } else if meta.is_file() {
                files.push((meta.modified().unwrap_or(UNIX_EPOCH), meta.len(), entry.path()));
            }
        }
    }
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= cap {
        return total;
    }
    files.sort_by_key(|f| f.0);
    let low = cap / 10 * 9;
    for (_, len, file) in files {
        if total <= low {
            break;
        }
        if fs::remove_file(&file).is_ok() {
            total -= len;
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU16, AtomicUsize};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// What the test server answers.
    #[derive(Clone)]
    struct Reply {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
        delay: Duration,
    }

    impl Reply {
        fn ok(body: &[u8]) -> Self {
            Self { status: 200, headers: Vec::new(), body: body.to_vec(), delay: Duration::ZERO }
        }

        fn status(status: u16) -> Self {
            Self { status, ..Self::ok(b"") }
        }

        fn header(mut self, name: &'static str, value: &str) -> Self {
            self.headers.push((name, value.to_owned()));
            self
        }

        fn after(mut self, delay: Duration) -> Self {
            self.delay = delay;
            self
        }
    }

    /// An HTTP/1.1 server on 127.0.0.1: one connection per request, answered by `handler` from the
    /// path. Counts the requests and the most it was answering at once.
    struct Server {
        base: String,
        paths: Arc<Mutex<Vec<String>>>,
        most: Arc<AtomicUsize>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Server {
        async fn start(handler: impl Fn(&str) -> Reply + Send + Sync + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let handler = Arc::new(handler);
            let paths = Arc::new(Mutex::new(Vec::new()));
            let (busy, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
            let (p, m) = (paths.clone(), most.clone());
            let task = tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    let (handler, paths, busy, most) = (handler.clone(), p.clone(), busy.clone(), m.clone());
                    tokio::spawn(async move {
                        let (mut head, mut buf) = (Vec::new(), [0u8; 1024]);
                        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                            match socket.read(&mut buf).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => head.extend_from_slice(&buf[..n]),
                            }
                        }
                        let path = String::from_utf8_lossy(&head).split_whitespace().nth(1).unwrap_or("/").to_owned();
                        paths.lock().unwrap().push(path.clone());
                        most.fetch_max(busy.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                        let reply = handler(&path);
                        tokio::time::sleep(reply.delay).await;
                        busy.fetch_sub(1, Ordering::SeqCst);
                        let mut out = format!(
                            "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nConnection: close\r\n",
                            reply.status,
                            reply.body.len()
                        );
                        for (name, value) in &reply.headers {
                            out += &format!("{name}: {value}\r\n");
                        }
                        out += "\r\n";
                        let _ = socket.write_all(out.as_bytes()).await;
                        let _ = socket.write_all(&reply.body).await;
                        let _ = socket.shutdown().await;
                    });
                }
            });
            Self { base, paths, most, task }
        }

        fn url(&self, path: &str) -> String {
            format!("{}{path}", self.base)
        }

        fn hits(&self) -> usize {
            self.paths.lock().unwrap().len()
        }

        /// Closes the port: from now on connections are refused.
        async fn stop(self) {
            self.task.abort();
            let _ = self.task.await;
        }
    }

    /// A cache folder of its own for one test, removed when it ends.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("rustradar-tiles-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            Self(root)
        }

        fn tiles(&self) -> Tiles {
            self.tiles_capped(CAP_BYTES)
        }

        fn tiles_capped(&self, cap_bytes: u64) -> Tiles {
            let client = fr24::client_builder().no_proxy().timeout(Duration::from_secs(10)).build().unwrap();
            Tiles::with_config(Config { root: self.0.clone(), cap_bytes, per_host: PER_HOST, client: Some(client) }).unwrap()
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn source(server: &Server) -> Source {
        Source::new("test", server.url("/{z}/{x}/{y}.pbf"))
    }

    fn entry(file: &Path) -> Entry {
        decode(fs::read(file).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn the_second_request_is_served_from_disk() {
        let root = TempRoot::new("from-disk");
        let server = Server::start(|path| Reply::ok(path.as_bytes()).header("Cache-Control", "public, max-age=3600")).await;
        let first = root.tiles().tile(&source(&server), 3, 4, 5).await.unwrap();
        assert_eq!(first.as_deref(), Some(&b"/3/4/5.pbf"[..]));
        assert!(root.0.join("test/3/4/5").is_file());
        // a new cache on the same folder: nothing is held in memory
        let second = root.tiles().tile(&source(&server), 3, 4, 5).await.unwrap();
        assert_eq!(second, first);
        assert_eq!(server.hits(), 1);
        // another tile is asked for
        root.tiles().tile(&source(&server), 3, 4, 6).await.unwrap();
        assert_eq!(server.hits(), 2);
        server.stop().await;
    }

    #[tokio::test]
    async fn a_stale_file_is_used_when_the_server_fails() {
        let root = TempRoot::new("stale");
        let failing = Arc::new(AtomicBool::new(false));
        let f = failing.clone();
        let server = Server::start(move |_| {
            if f.load(Ordering::SeqCst) { Reply::status(500) } else { Reply::ok(b"old").header("Cache-Control", "max-age=0") }
        })
        .await;
        let (tiles, src) = (root.tiles(), source(&server));
        assert_eq!(tiles.tile(&src, 1, 0, 0).await.unwrap().as_deref(), Some(&b"old"[..]));
        // stale at once, so the server is asked again; it fails, the old tile is used
        failing.store(true, Ordering::SeqCst);
        assert_eq!(tiles.tile(&src, 1, 0, 0).await.unwrap().as_deref(), Some(&b"old"[..]));
        assert_eq!(server.hits(), 2);
        // without one, the failure is an error
        assert_eq!(tiles.tile(&src, 1, 1, 0).await, Err("127.0.0.1 answered 500".to_owned()));
        // no server at all: the same
        let url = src.url(1, 0, 0);
        server.stop().await;
        assert_eq!(tiles.tile(&src, 1, 0, 0).await.unwrap().as_deref(), Some(&b"old"[..]));
        assert!(tiles.get("test/9/9/9", &url, DEFAULT_FRESH).await.is_err());
    }

    #[tokio::test]
    async fn a_stale_file_is_replaced_when_the_server_answers() {
        let root = TempRoot::new("replaced");
        let version = Arc::new(AtomicU16::new(1));
        let v = version.clone();
        let server = Server::start(move |_| {
            let v = v.load(Ordering::SeqCst);
            Reply::ok(format!("v{v}").as_bytes()).header("Cache-Control", if v == 1 { "no-cache" } else { "max-age=600" })
        })
        .await;
        let (tiles, src) = (root.tiles(), source(&server));
        assert_eq!(tiles.tile(&src, 2, 1, 1).await.unwrap().as_deref(), Some(&b"v1"[..]));
        version.store(2, Ordering::SeqCst);
        assert_eq!(tiles.tile(&src, 2, 1, 1).await.unwrap().as_deref(), Some(&b"v2"[..]));
        assert_eq!(tiles.tile(&src, 2, 1, 1).await.unwrap().as_deref(), Some(&b"v2"[..]));
        assert_eq!(server.hits(), 2);
        server.stop().await;
    }

    #[tokio::test]
    async fn not_found_is_none_and_is_remembered() {
        let root = TempRoot::new("not-found");
        let server = Server::start(|_| Reply::status(404)).await;
        let (tiles, src) = (root.tiles(), source(&server));
        assert_eq!(tiles.tile(&src, 14, 1, 2).await, Ok(None));
        assert_eq!(tiles.tile(&src, 14, 1, 2).await, Ok(None));
        assert_eq!(server.hits(), 1);
        assert_eq!(entry(&root.0.join("test/14/1/2")).body, None);
        server.stop().await;
    }

    #[tokio::test]
    async fn the_expiry_on_disk_follows_the_response() {
        let root = TempRoot::new("expiry");
        let server = Server::start(|path| match path {
            "/a" => Reply::ok(b"a").header("Cache-Control", "max-age=3600"),
            // as OpenFreeMap sends its TileJSON: two headers, and the age of the CDN's copy
            "/b" => Reply::ok(b"b").header("Cache-Control", "max-age=3600").header("Cache-Control", "public").header("Age", "600"),
            "/c" => Reply::ok(b"c").header("Cache-Control", "no-store"),
            _ => Reply::ok(b"d"),
        })
        .await;
        let tiles = root.tiles();
        let before = crate::now_ms();
        for name in ["a", "b", "c", "d"] {
            tiles.get(&format!("misc/{name}"), &server.url(&format!("/{name}")), DEFAULT_FRESH).await.unwrap();
        }
        let after = crate::now_ms();
        let expires = |name: &str| entry(&root.0.join("misc").join(name)).expires_ms;
        let within = |at: i64, s: i64| (before + s * 1000..=after + s * 1000).contains(&at);
        assert!(within(expires("a"), 3600));
        assert!(within(expires("b"), 3000));
        assert!(!root.0.join("misc/c").exists());
        assert!(within(expires("d"), 7 * 24 * 3600));
        // what was not kept is asked again
        tiles.get("misc/c", &server.url("/c"), DEFAULT_FRESH).await.unwrap();
        assert_eq!(server.hits(), 5);
        server.stop().await;
    }

    #[test]
    fn freshness_is_max_age_less_age_else_the_default() {
        let week = DEFAULT_FRESH;
        assert_eq!(keep_for("", None, week), Some(week));
        assert_eq!(keep_for("public", None, week), Some(week));
        assert_eq!(keep_for("public, max-age=3600, immutable", None, week), Some(Duration::from_secs(3600)));
        assert_eq!(keep_for("Max-Age=\"60\"", None, week), Some(Duration::from_secs(60)));
        assert_eq!(keep_for("max-age=60", Some("20"), week), Some(Duration::from_secs(40)));
        assert_eq!(keep_for("max-age=60", Some("90"), week), Some(Duration::ZERO));
        assert_eq!(keep_for("max-age=0", None, week), Some(Duration::ZERO));
        assert_eq!(keep_for("max-age=60, no-cache", None, week), Some(Duration::ZERO));
        assert_eq!(keep_for("public, no-store", None, week), None);
        assert_eq!(keep_for("max-age=soon", None, week), Some(week));
        // s-maxage is for shared caches
        assert_eq!(keep_for("s-maxage=60", None, week), Some(week));
        assert_eq!(retry_after(" 120 "), Some(Duration::from_secs(120)));
        assert_eq!(retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
    }

    #[tokio::test]
    async fn a_changed_url_counts_as_stale() {
        let root = TempRoot::new("changed-url");
        let server = Server::start(|path| {
            if path.starts_with("/b2/") { Reply::status(502) } else { Reply::ok(path.as_bytes()).header("Cache-Control", "max-age=86400") }
        })
        .await;
        let tiles = root.tiles();
        let build1 = Source::new("planet", server.url("/b1/{z}/{x}/{y}"));
        let build3 = Source::new("planet", server.url("/b3/{z}/{x}/{y}"));
        let build2 = Source::new("planet", server.url("/b2/{z}/{x}/{y}"));
        assert_eq!(tiles.tile(&build1, 5, 6, 7).await.unwrap().as_deref(), Some(&b"/b1/5/6/7"[..]));
        // a new build is fetched again, though the old one is still fresh
        assert_eq!(tiles.tile(&build3, 5, 6, 7).await.unwrap().as_deref(), Some(&b"/b3/5/6/7"[..]));
        assert_eq!(tiles.tile(&build3, 5, 6, 7).await.unwrap().as_deref(), Some(&b"/b3/5/6/7"[..]));
        assert_eq!(server.hits(), 2);
        // one that fails gets the tile of the build before
        assert_eq!(tiles.tile(&build2, 5, 6, 7).await.unwrap().as_deref(), Some(&b"/b3/5/6/7"[..]));
        assert_eq!(server.hits(), 3);
        server.stop().await;
    }

    #[tokio::test]
    async fn requests_to_one_host_take_turns() {
        let root = TempRoot::new("per-host");
        let server = Server::start(|path| Reply::ok(path.as_bytes()).after(Duration::from_millis(100))).await;
        let tiles = root.tiles();
        tiles.set_host_limit("127.0.0.1", 2);
        let src = source(&server);
        let all = (0..6).map(|x| {
            let (tiles, src) = (tiles.clone(), src.clone());
            tokio::spawn(async move { tiles.tile(&src, 4, x, 0).await })
        });
        for (x, task) in all.collect::<Vec<_>>().into_iter().enumerate() {
            assert_eq!(task.await.unwrap().unwrap(), Some(format!("/4/{x}/0.pbf").into_bytes()));
        }
        assert_eq!(server.hits(), 6);
        assert_eq!(server.most.load(Ordering::SeqCst), 2);
        server.stop().await;
    }

    #[tokio::test]
    async fn a_host_that_asks_to_wait_is_left_alone() {
        let root = TempRoot::new("backoff");
        let busy = Arc::new(AtomicBool::new(false));
        let b = busy.clone();
        let server = Server::start(move |path| {
            if b.load(Ordering::SeqCst) {
                Reply::status(429).header("Retry-After", "60")
            } else {
                Reply::ok(path.as_bytes()).header("Cache-Control", "max-age=0")
            }
        })
        .await;
        let (tiles, src) = (root.tiles(), source(&server));
        tiles.tile(&src, 6, 1, 1).await.unwrap();
        busy.store(true, Ordering::SeqCst);
        assert_eq!(tiles.tile(&src, 6, 2, 2).await, Err("127.0.0.1 answered 429".to_owned()));
        assert_eq!(server.hits(), 2);
        // not asked again while it waits: an error, or the stale tile where there is one
        assert_eq!(tiles.tile(&src, 6, 3, 3).await, Err("127.0.0.1 asked to wait".to_owned()));
        assert_eq!(tiles.tile(&src, 6, 1, 1).await.unwrap().as_deref(), Some(&b"/6/1/1.pbf"[..]));
        assert_eq!(server.hits(), 2);
        server.stop().await;
    }

    #[tokio::test]
    async fn a_503_without_a_wait_is_only_a_failure() {
        let root = TempRoot::new("503");
        let server = Server::start(|path| if path == "/a" { Reply::status(503) } else { Reply::ok(b"b") }).await;
        let tiles = root.tiles();
        assert!(tiles.get("x/a", &server.url("/a"), DEFAULT_FRESH).await.is_err());
        assert_eq!(tiles.get("x/b", &server.url("/b"), DEFAULT_FRESH).await, Ok(Some(b"b".to_vec())));
        assert_eq!(server.hits(), 2);
        server.stop().await;
    }

    #[tokio::test]
    async fn the_tilejson_names_the_tile_url() {
        let root = TempRoot::new("tilejson");
        let down = Arc::new(AtomicBool::new(false));
        let d = down.clone();
        let server = Server::start(move |path| match path {
            _ if d.load(Ordering::SeqCst) => Reply::status(500),
            "/planet" => Reply::ok(
                br#"{"tilejson":"3.0.0","tiles":["https://tiles.openfreemap.org/planet/20261104_001001_pt/{z}/{x}/{y}.pbf"],"minzoom":0,"maxzoom":14}"#,
            ),
            "/stale" => Reply::ok(br#"{"tiles":["https://example.invalid/{z}/{x}/{y}"]}"#).header("Cache-Control", "no-cache"),
            "/empty" => Reply::ok(br#"{"tiles":[]}"#),
            _ => Reply::status(404),
        })
        .await;
        let tiles = root.tiles();
        let json = tiles.tilejson(OPENFREEMAP, &server.url("/planet")).await.unwrap();
        assert_eq!((json.minzoom, json.maxzoom), (Some(0), Some(14)));
        let planet = tiles.source_from_tilejson(OPENFREEMAP, &server.url("/planet")).await.unwrap();
        assert_eq!(planet, Source::new("openfreemap", "https://tiles.openfreemap.org/planet/20261104_001001_pt/{z}/{x}/{y}.pbf"));
        assert_eq!(planet.url(14, 8800, 4800), "https://tiles.openfreemap.org/planet/20261104_001001_pt/14/8800/4800.pbf");
        assert!(root.0.join("openfreemap/tilejson.json").is_file());
        assert_eq!(server.hits(), 1);
        assert_eq!(
            tiles.source_from_tilejson("empty", &server.url("/empty")).await,
            Err(format!("{}: the TileJSON names no tiles", server.url("/empty")))
        );
        assert!(tiles.tilejson("missing", &server.url("/missing")).await.is_err());
        tiles.tilejson("stale", &server.url("/stale")).await.unwrap();
        // the server gone: a fresh one is read from disk, a stale one is used all the same
        down.store(true, Ordering::SeqCst);
        assert_eq!(tiles.source_from_tilejson(OPENFREEMAP, &server.url("/planet")).await, Ok(planet));
        let stale = tiles.source_from_tilejson("stale", &server.url("/stale")).await.unwrap();
        assert_eq!(stale.template, "https://example.invalid/{z}/{x}/{y}");
        server.stop().await;
    }

    #[test]
    fn a_tilejson_is_read_leniently() {
        let json = TileJson::parse(br#"{"tiles":["a",3,"b"],"minzoom":2.0,"maxzoom":-1}"#).unwrap();
        assert_eq!(json, TileJson { tiles: vec!["a".into(), "b".into()], minzoom: Some(2), maxzoom: None });
        assert_eq!(TileJson::parse(b"{}"), Ok(TileJson::default()));
        assert!(TileJson::parse(b"<html>").is_err());
    }

    #[tokio::test]
    async fn the_cache_is_trimmed_least_recently_used_first() {
        let root = TempRoot::new("trim");
        let server = Server::start(|_| Reply::ok(&[7; 1000])).await;
        let (tiles, src) = (root.tiles(), source(&server));
        for x in 0..5 {
            tiles.tile(&src, 8, x, 0).await.unwrap();
        }
        let file = |x: u32| root.0.join(format!("test/8/{x}/0"));
        let size = fs::metadata(file(0)).unwrap().len();
        assert_eq!(size, 1000 + HEADER as u64);
        assert_eq!(tiles.trim().await, 5 * size);
        // tile 0 used longest ago, tile 4 last
        let now = SystemTime::now();
        for x in 0..5 {
            let used = now - Duration::from_secs(3600 * u64::from(5 - x));
            fs::File::options().append(true).open(file(x)).unwrap().set_modified(used).unwrap();
        }
        // reading tile 0 makes it the most recently used
        tiles.tile(&src, 8, 0, 0).await.unwrap();
        assert_eq!(server.hits(), 5);
        // room for three and a half: three stay, under nine tenths
        let small = root.tiles_capped(size * 7 / 2);
        assert_eq!(small.trim().await, 3 * size);
        assert_eq!(small.bytes_on_disk(), Some(3 * size));
        let kept: Vec<bool> = (0..5).map(|x| file(x).exists()).collect();
        assert_eq!(kept, [true, false, false, true, true]);
        server.stop().await;
    }

    #[tokio::test]
    async fn writes_past_the_cap_trim_the_cache() {
        let root = TempRoot::new("cap");
        let server = Server::start(|_| Reply::ok(&[1; 4000])).await;
        let cap = 10_000;
        let (tiles, src) = (root.tiles_capped(cap), source(&server));
        for x in 0..8 {
            assert!(tiles.tile(&src, 9, x, 3).await.unwrap().is_some());
            // a trim started by the write has finished once this one has
            assert!(tiles.trim().await <= cap);
        }
        let left = (0..8).filter(|x| root.0.join(format!("test/9/{x}/3")).exists()).count();
        assert_eq!(left, 2);
        assert_eq!(tiles.bytes_on_disk(), Some(2 * 4024));
        server.stop().await;
    }

    #[tokio::test]
    async fn a_file_this_did_not_write_is_fetched_again() {
        let root = TempRoot::new("foreign");
        let server = Server::start(|_| Reply::ok(b"tile")).await;
        let (tiles, src) = (root.tiles(), source(&server));
        let file = root.0.join("test/0/0/0");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"not one of ours, long enough to hold a header").unwrap();
        assert_eq!(tiles.tile(&src, 0, 0, 0).await, Ok(Some(b"tile".to_vec())));
        assert_eq!(server.hits(), 1);
        assert_eq!(entry(&file).body.as_deref(), Some(&b"tile"[..]));
        server.stop().await;
    }

    #[tokio::test]
    async fn paths_outside_the_cache_are_refused() {
        let root = TempRoot::new("paths");
        let server = Server::start(|_| Reply::ok(b"x")).await;
        let tiles = root.tiles();
        for path in ["../x", "a/../../x", "/abs", "a//b", "a/.hidden", "", "a b", "a\\b"] {
            assert!(tiles.get(path, &server.url("/x"), DEFAULT_FRESH).await.is_err(), "{path}");
        }
        let bad = Source::new("..", server.url("/{z}/{x}/{y}"));
        assert!(tiles.tile(&bad, 0, 0, 0).await.is_err());
        assert_eq!(server.hits(), 0);
        server.stop().await;
    }

    #[test]
    fn the_url_fills_the_template() {
        let esri = Source::new("esri", "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}");
        assert_eq!(esri.url(3, 4, 5), "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/3/5/4");
        assert_eq!(esri.fresh, DEFAULT_FRESH);
    }

    #[test]
    fn a_file_reads_back_as_written() {
        let found = encode(Some(b"body"), 1_700_000_000_000, 42);
        assert_eq!(found.len(), HEADER + 4);
        assert_eq!(decode(found), Some(Entry { body: Some(b"body".to_vec()), expires_ms: 1_700_000_000_000, url_hash: 42 }));
        assert_eq!(decode(encode(None, -1, 7)), Some(Entry { body: None, expires_ms: -1, url_hash: 7 }));
        assert_eq!(decode(encode(Some(b""), 0, 0)).unwrap().body, Some(Vec::new()));
        assert_eq!(decode(b"RRT1".to_vec()), None);
        // FNV-1a's published values
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    /// The real TileJSON, for a person to run: `cargo test -p rustradar-backend tiles::live -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_openfreemap() {
        let root = TempRoot::new("live");
        let tiles = Tiles::new(&root.0).unwrap();
        let planet = tiles.openfreemap().await.unwrap();
        assert!(planet.template.starts_with("https://tiles.openfreemap.org/planet/"), "{}", planet.template);
        let world = tiles.tile(&planet, 0, 0, 0).await.unwrap().unwrap();
        assert!(world.len() > 1000);
    }
}
