//! Remote pictures for the UI, kept on disk and decoded off the UI thread (plan 1.5, 2.3; item
//! U18): the aircraft photos and airline logos of the hover card and the flight panel, the ships'
//! and the trains' photos and the operators' logos. Today's webview loaded each `<img>` itself
//! and kept it in its HTTP cache; `app::images` asks this module instead.
//!
//! - **The cached GET** ([`Images::bytes`]) is [`crate::tiles::Tiles`]' (`Tiles::get`) on a cache
//!   of its own in `<cache>/images`, capped at [`CAP_BYTES`] (64 MB), least recently used first
//!   past it: a file answers while it is fresh (`Cache-Control: max-age`, else
//!   [`crate::tiles::DEFAULT_FRESH`]); a stale one answers when the download fails; a host that
//!   answers 429 is left alone for as long as it asks. At most [`PER_HOST`] requests to one host
//!   at once (a browser's six). Only `http:` and `https:` URLs are asked; redirects are followed
//!   (Wikimedia's `Special:FilePath` answers with one). A 404 or 204 is a failure here (`<img>`'s
//!   `onerror`), kept like a tile so it is not asked again while fresh.
//! - **The failure memo**: a URL whose last attempt failed (no connection, an error status, not a
//!   picture) fails again at once, without asking, for [`FAILURE_TTL`] (two minutes, the hover
//!   loader's `failureTtlMs`): a card shown again and again does not hammer a host that refused it.
//! - **Decoding** ([`Images::picture`], [`decode`]) runs on tokio's blocking pool: PNG, JPEG or
//!   WebP (the `image` crate's features in the workspace) to 8-bit RGBA, not premultiplied. A
//!   picture larger than [`MAX_SIDE`] on a side is scaled down to it, its aspect kept (egui's
//!   textures have a size limit; the UI draws none larger than 360 × 180 px). Bodies that are not
//!   a picture (an HTML error page answered with 200, an SVG) fail, and their file is removed so a
//!   later attempt downloads again.
//!
//! **Hosts and headers** (plan risk K16, tried from this machine on 2026-11-11, each with a
//! descriptive User-Agent with a contact URL, a browser's, and none):
//!
//! | Host | Serves | Answer |
//! |---|---|---|
//! | `cdn.jetphotos.com` | aircraft photos (`images[].thumbnail`, `medium`, `large`) | 200 to any User-Agent, no Referer needed; `max-age` 16 days |
//! | `cdn.flightradar24.com` | airline logos (`airline_logo_url`) | 200 to any User-Agent, no Referer; `max-age` 31 days; an unknown airline 403 |
//! | `static.vesselfinder.net` | ship photos (`ship_photo_url`) | 200 to any User-Agent, no Referer; no `Cache-Control` (7 days kept) |
//! | `upload.wikimedia.org`, `commons.wikimedia.org` | train photos and operators' logos (`train_media`) | 200 to the descriptive User-Agent; **403 with none, 429 to a browser's**; a thumbnail width off Wikimedia's steps 400 |
//!
//! So no host needs a Referer or a browser-like User-Agent, and Wikimedia refuses the latter: every
//! request carries the app's own User-Agent, `RustRadar/{version} (+{repository})`, the tile
//! cache's (`Tiles::with_config`), which Wikimedia's robot policy asks for.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::tiles::{self, Tiles};

/// The most the pictures on disk may take together (plan 1.5).
pub const CAP_BYTES: u64 = 64 << 20;
/// Requests to one host at once: a browser's six connections a host.
pub const PER_HOST: usize = 6;
/// How long a URL that failed fails again without being asked.
pub const FAILURE_TTL: Duration = Duration::from_secs(120);
/// The longest side of a decoded picture; a larger one is scaled down to it.
pub const MAX_SIDE: u32 = 2048;
/// The largest picture decoded at all, a side (anything larger is refused before it is decoded).
pub const MAX_DECODED_SIDE: u32 = 16_384;
/// The most memory a decoder may take for one picture.
pub const MAX_DECODE_BYTES: u64 = 256 << 20;
/// How long a picture is kept when its server does not say: the tile cache's default.
pub const DEFAULT_FRESH: Duration = tiles::DEFAULT_FRESH;

/// How an [`Images`] is set up; [`Config::new`] has the app's settings.
pub struct Config {
    /// The folder the files are kept in: `<cache>/images`.
    pub root: PathBuf,
    pub cap_bytes: u64,
    pub per_host: usize,
    pub failure_ttl: Duration,
    /// The HTTP client; None builds the tile cache's (through `fr24::client_builder()`).
    pub client: Option<reqwest::Client>,
}

impl Config {
    pub fn new(cache_dir: &Path) -> Self {
        Self { root: cache_dir.join("images"), cap_bytes: CAP_BYTES, per_host: PER_HOST, failure_ttl: FAILURE_TTL, client: None }
    }
}

/// A decoded picture: 8-bit RGBA, not premultiplied, row by row from the top left.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({} × {})", self.width, self.height)
    }
}

/// The picture cache. Cheap to clone; every clone is the same cache.
#[derive(Clone)]
pub struct Images {
    tiles: Tiles,
    /// The URLs whose last attempt failed: when, and why.
    failed: Arc<Mutex<HashMap<String, (Instant, String)>>>,
    failure_ttl: Duration,
}

impl Images {
    /// The app's picture cache in `<cache_dir>/images`.
    pub fn new(cache_dir: &Path) -> Result<Self, String> {
        Self::with_config(Config::new(cache_dir))
    }

    pub fn with_config(config: Config) -> Result<Self, String> {
        let tiles = Tiles::with_config(tiles::Config { root: config.root, cap_bytes: config.cap_bytes, per_host: config.per_host, client: config.client })?;
        Ok(Self { tiles, failed: Arc::new(Mutex::new(HashMap::new())), failure_ttl: config.failure_ttl })
    }

    /// The folder the files are kept in.
    pub fn root(&self) -> &Path {
        self.tiles.root()
    }

    /// Why `url` failed, while its failure is remembered (see the module documentation).
    pub fn failure(&self, url: &str) -> Option<String> {
        let failed = self.failed.lock().unwrap();
        failed.get(url).filter(|(at, _)| at.elapsed() < self.failure_ttl).map(|(_, why)| why.clone())
    }

    /// The picture's bytes as the server sent them, from disk while fresh (see the module
    /// documentation). An error for a URL that is not `http:` or `https:`, one that failed less than
    /// [`FAILURE_TTL`] ago, a 404, and any failure of the download with no file to fall back on.
    pub async fn bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        if let Some(why) = self.failure(url) {
            return Err(why);
        }
        let result = match cache_path(url) {
            Ok(path) => match self.tiles.get(&path, url, DEFAULT_FRESH).await {
                Ok(Some(bytes)) => Ok(bytes),
                Ok(None) => Err(format!("{}: no picture there", host_of(url))),
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        };
        if let Err(why) = &result {
            self.remember(url, why);
        }
        result
    }

    /// The picture at `url`, decoded on the blocking pool ([`decode`]). Fails as [`Self::bytes`]
    /// does, and when the bytes are not a picture (their file is then removed and the failure
    /// remembered).
    pub async fn picture(&self, url: &str) -> Result<Picture, String> {
        let bytes = self.bytes(url).await?;
        let decoded = tokio::task::spawn_blocking(move || decode(&bytes)).await.map_err(|e| e.to_string()).and_then(|r| r);
        decoded.map_err(|e| {
            let why = format!("{}: {e}", host_of(url));
            if let Ok(path) = cache_path(url) {
                let _ = fs::remove_file(path.split('/').fold(self.root().to_path_buf(), |file, p| file.join(p)));
            }
            self.remember(url, &why);
            why
        })
    }

    /// Counts the files on disk and, past the cap, removes the least recently used (the tile
    /// cache's trim; it also runs by itself after the writes that take the cache past its cap).
    /// The bytes left on disk.
    pub async fn trim(&self) -> u64 {
        self.tiles.trim().await
    }

    fn remember(&self, url: &str, why: &str) {
        let mut failed = self.failed.lock().unwrap();
        let ttl = self.failure_ttl;
        failed.retain(|_, (at, _)| at.elapsed() < ttl);
        failed.insert(url.to_owned(), (Instant::now(), why.to_owned()));
    }
}

/// `url`'s host, for messages.
fn host_of(url: &str) -> String {
    reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_else(|| url.to_owned())
}

/// Where `url` is kept under the cache's folder: `<host>/<hash>`, the host's characters other than
/// letters, digits, `-` and `.` as `_`, the hash 16 hex digits of the whole URL (64-bit FNV-1a; the
/// file also holds the URL's hash, so two URLs of one name never answer for each other). An error
/// for a URL that is not `http:` or `https:`.
pub fn cache_path(url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("{url}: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("{url}: not an http or https picture"));
    }
    let host: String = parsed.host_str().unwrap_or_default().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
    let host = match host.trim_start_matches('.') {
        "" => "_".to_owned(),
        h => h.to_owned(),
    };
    let hash = url.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3));
    Ok(format!("{host}/{hash:016x}"))
}

/// Decodes a PNG, JPEG or WebP to 8-bit RGBA, scaled down to [`MAX_SIDE`] on its longer side when
/// it is larger. Refuses anything else, and pictures over [`MAX_DECODED_SIDE`] on a side or
/// needing over [`MAX_DECODE_BYTES`] to decode.
pub fn decode(bytes: &[u8]) -> Result<Picture, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    if reader.format().is_none() {
        return Err("not a PNG, JPEG or WebP picture".to_owned());
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODED_SIDE);
    limits.max_image_height = Some(MAX_DECODED_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let mut picture = reader.decode().map_err(|e| e.to_string())?;
    if picture.width() > MAX_SIDE || picture.height() > MAX_SIDE {
        picture = picture.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle);
    }
    let rgba = picture.into_rgba8();
    Ok(Picture { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A `width` × `height` picture of one colour, encoded as `format`.
    fn encoded(width: u32, height: u32, rgba: [u8; 4], format: image::ImageFormat) -> Vec<u8> {
        let picture = image::RgbaImage::from_pixel(width, height, image::Rgba(rgba));
        let mut out = std::io::Cursor::new(Vec::new());
        match format {
            // JPEG has no alpha
            image::ImageFormat::Jpeg => image::DynamicImage::ImageRgba8(picture).into_rgb8().write_to(&mut out, format).unwrap(),
            _ => picture.write_to(&mut out, format).unwrap(),
        }
        out.into_inner()
    }

    /// An HTTP/1.1 server on 127.0.0.1 answering every path with the status, content type and body
    /// `reply` gives for it, counting the requests.
    struct Server {
        base: String,
        hits: Arc<AtomicUsize>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Server {
        async fn start(reply: impl Fn(&str) -> (u16, &'static str, Vec<u8>) + Send + Sync + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (reply, hits) = (Arc::new(reply), Arc::new(AtomicUsize::new(0)));
            let h = hits.clone();
            let task = tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    let (reply, hits) = (reply.clone(), h.clone());
                    tokio::spawn(async move {
                        let (mut head, mut buf) = (Vec::new(), [0u8; 1024]);
                        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                            match socket.read(&mut buf).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => head.extend_from_slice(&buf[..n]),
                            }
                        }
                        hits.fetch_add(1, Ordering::SeqCst);
                        let path = String::from_utf8_lossy(&head).split_whitespace().nth(1).unwrap_or("/").to_owned();
                        let (status, kind, body) = reply(&path);
                        let out = format!("HTTP/1.1 {status} Test\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        let _ = socket.write_all(out.as_bytes()).await;
                        let _ = socket.write_all(&body).await;
                        let _ = socket.shutdown().await;
                    });
                }
            });
            Self { base, hits, task }
        }

        fn url(&self, path: &str) -> String {
            format!("{}{path}", self.base)
        }

        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    /// A cache folder of its own for one test, removed when it ends.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("rustradar-images-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            Self(root)
        }

        fn images(&self) -> Images {
            self.images_with(CAP_BYTES, FAILURE_TTL)
        }

        fn images_with(&self, cap_bytes: u64, failure_ttl: Duration) -> Images {
            let client = fr24::client_builder().no_proxy().timeout(Duration::from_secs(10)).build().unwrap();
            Images::with_config(Config { root: self.0.clone(), cap_bytes, per_host: PER_HOST, failure_ttl, client: Some(client) }).unwrap()
        }

        fn files(&self) -> usize {
            let mut count = 0;
            let mut dirs = vec![self.0.clone()];
            while let Some(dir) = dirs.pop() {
                for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
                    if entry.path().is_dir() {
                        dirs.push(entry.path());
                    } else {
                        count += 1;
                    }
                }
            }
            count
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn a_picture_is_decoded_and_kept_on_disk() {
        let root = TempRoot::new("kept");
        let png = encoded(40, 30, [200, 30, 40, 255], image::ImageFormat::Png);
        let server = Server::start(move |_| (200, "image/png", png.clone())).await;
        let url = server.url("/photo.png");
        let picture = root.images().picture(&url).await.unwrap();
        assert_eq!((picture.width, picture.height), (40, 30));
        assert_eq!(&picture.rgba[..4], &[200, 30, 40, 255]);
        assert!(root.0.join(cache_path(&url).unwrap()).is_file(), "kept under <host>/<hash>");
        // a new cache on the same folder: answered from disk
        let again = root.images().picture(&url).await.unwrap();
        assert_eq!(again, picture);
        assert_eq!(server.hits(), 1);
    }

    #[tokio::test]
    async fn jpeg_and_webp_decode_too() {
        let root = TempRoot::new("formats");
        let jpeg = encoded(16, 9, [30, 120, 220, 255], image::ImageFormat::Jpeg);
        let webp = encoded(9, 16, [10, 200, 90, 255], image::ImageFormat::WebP);
        let server = Server::start(move |path| if path.ends_with(".jpg") { (200, "image/jpeg", jpeg.clone()) } else { (200, "image/webp", webp.clone()) }).await;
        let images = root.images();
        let jpeg = images.picture(&server.url("/a.jpg")).await.unwrap();
        assert_eq!((jpeg.width, jpeg.height), (16, 9));
        // lossy, but near the colour
        let [r, g, b, a] = [jpeg.rgba[0], jpeg.rgba[1], jpeg.rgba[2], jpeg.rgba[3]];
        assert!(r.abs_diff(30) < 8 && g.abs_diff(120) < 8 && b.abs_diff(220) < 8 && a == 255, "{r} {g} {b} {a}");
        let webp = images.picture(&server.url("/b.webp")).await.unwrap();
        assert_eq!((webp.width, webp.height), (9, 16));
        assert_eq!(&webp.rgba[..4], &[10, 200, 90, 255]);
    }

    #[tokio::test]
    async fn a_failure_is_remembered_and_asked_again_after_its_time() {
        let root = TempRoot::new("memo");
        let status = Arc::new(AtomicU16::new(500));
        let png = encoded(2, 2, [1, 2, 3, 255], image::ImageFormat::Png);
        let s = status.clone();
        let server = Server::start(move |_| match s.load(Ordering::SeqCst) {
            200 => (200, "image/png", png.clone()),
            code => (code, "text/plain", b"no".to_vec()),
        })
        .await;
        let images = root.images_with(CAP_BYTES, Duration::from_millis(300));
        let url = server.url("/logo.png");
        let first = images.picture(&url).await.unwrap_err();
        assert_eq!(first, "127.0.0.1 answered 500");
        assert_eq!(images.failure(&url).as_deref(), Some(first.as_str()));
        // the server is fine now, but the failure stands for its time without asking
        status.store(200, Ordering::SeqCst);
        assert_eq!(images.picture(&url).await.unwrap_err(), first);
        assert_eq!(server.hits(), 1);
        // other URLs are asked as usual
        assert!(images.picture(&server.url("/other.png")).await.is_ok());
        assert_eq!(server.hits(), 2);
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert_eq!(images.failure(&url), None);
        assert!(images.picture(&url).await.is_ok());
        assert_eq!(server.hits(), 3);
    }

    #[tokio::test]
    async fn not_found_and_refused_are_failures() {
        let root = TempRoot::new("refused");
        let server = Server::start(|path| if path == "/gone.png" { (404, "text/html", b"<p>gone</p>".to_vec()) } else { (403, "application/xml", b"<Error/>".to_vec()) }).await;
        let images = root.images();
        assert_eq!(images.picture(&server.url("/gone.png")).await.unwrap_err(), "127.0.0.1: no picture there");
        assert_eq!(images.picture(&server.url("/999999.png")).await.unwrap_err(), "127.0.0.1 answered 403");
        assert_eq!(server.hits(), 2);
    }

    #[tokio::test]
    async fn a_body_that_is_not_a_picture_fails_and_is_not_kept() {
        let root = TempRoot::new("html");
        let server = Server::start(|_| (200, "text/html", b"<!doctype html><title>Please wait</title>".to_vec())).await;
        let images = root.images_with(CAP_BYTES, Duration::ZERO);
        let url = server.url("/photo.jpg");
        let why = images.picture(&url).await.unwrap_err();
        assert!(why.starts_with("127.0.0.1: "), "{why}");
        assert!(!root.0.join(cache_path(&url).unwrap()).exists(), "the file is removed");
        // with nothing kept, the next attempt downloads again
        images.picture(&url).await.unwrap_err();
        assert_eq!(server.hits(), 2);
    }

    #[tokio::test]
    async fn only_http_and_https_are_asked() {
        let root = TempRoot::new("schemes");
        let images = root.images();
        for url in ["file:///etc/passwd", "bytes://logo.png", "data:image/png;base64,AAAA", "not a url"] {
            assert!(images.picture(url).await.is_err(), "{url}");
        }
        assert_eq!(root.files(), 0);
    }

    #[tokio::test]
    async fn the_pictures_on_disk_stay_under_the_cap() {
        let root = TempRoot::new("cap");
        // noise, so the PNGs do not compress below the cap
        let server = Server::start(|path| {
            let seed = path.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
            let noise = image::RgbaImage::from_fn(24, 24, |x, y| {
                let v = (x * 7919 + y * 104_729 + seed).wrapping_mul(2_654_435_761);
                image::Rgba([(v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, 255])
            });
            let mut out = std::io::Cursor::new(Vec::new());
            noise.write_to(&mut out, image::ImageFormat::Png).unwrap();
            (200, "image/png", out.into_inner())
        })
        .await;
        let cap = 6_000;
        let images = root.images_with(cap, FAILURE_TTL);
        for n in 0..8 {
            images.picture(&server.url(&format!("/{n}.png"))).await.unwrap();
        }
        let left = images.trim().await;
        assert!(left <= cap, "{left} bytes on disk");
        assert!(root.files() < 8, "the oldest went");
        // the newest is still there
        assert!(root.0.join(cache_path(&server.url("/7.png")).unwrap()).is_file());
    }

    #[test]
    fn where_a_url_is_kept() {
        let a = cache_path("https://cdn.jetphotos.com/200/5/459674_1738674373_tb.jpg").unwrap();
        assert!(a.starts_with("cdn.jetphotos.com/"), "{a}");
        assert_eq!(a.len(), "cdn.jetphotos.com/".len() + 16);
        assert_eq!(a, cache_path("https://cdn.jetphotos.com/200/5/459674_1738674373_tb.jpg").unwrap());
        assert_ne!(a, cache_path("https://cdn.jetphotos.com/400/5/459674_1738674373.jpg").unwrap());
        assert!(cache_path("http://[::1]:8080/x.png").unwrap().starts_with("___1_/"));
        assert!(cache_path("ftp://example.org/x.png").is_err());
    }

    #[test]
    fn a_large_picture_is_scaled_down_with_its_aspect() {
        let png = encoded(4096, 1024, [9, 9, 9, 255], image::ImageFormat::Png);
        let picture = decode(&png).unwrap();
        assert_eq!((picture.width, picture.height), (MAX_SIDE, 512));
        assert_eq!(picture.rgba.len(), (MAX_SIDE * 512 * 4) as usize);
        let small = decode(&encoded(3, 5, [1, 1, 1, 128], image::ImageFormat::Png)).unwrap();
        assert_eq!((small.width, small.height), (3, 5));
        assert_eq!(&small.rgba[..4], &[1, 1, 1, 128], "not premultiplied");
    }

    #[test]
    fn what_is_not_a_picture_is_refused() {
        assert!(decode(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>").is_err());
        assert!(decode(b"").is_err());
        // a PNG cut short
        let png = encoded(8, 8, [1, 2, 3, 255], image::ImageFormat::Png);
        assert!(decode(&png[..png.len() / 2]).is_err());
    }
}
