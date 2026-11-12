//! The clouds as they are now, the world over, for the realistic globe (item R10; doc 04 §9;
//! `clouds.ts`): EUMETSAT's world cloud map (EUMETView), its geostationary satellites' infrared at
//! 10.8 µm stitched round the Earth every three hours, one JPEG of 4096 × 1934 over
//! [`BOUNDS`] (the map's clouds layer turns its grey into cloud).
//!
//! [`Feed`] is `cloudPicture(onLoad)`: called while the clouds show (the app's state, each frame
//! on the realistic basemap below zoom 6.5, as today's `buildLayers` calls it at each render),
//! it asks for the picture when it is due ([`is_due`]: not while an ask is out; an hour after the
//! last ask once there is a picture, five minutes while there is none; the time counts from the
//! ask's start, success or not) and hands over the latest [`Picture`] once one has come. A failed
//! ask (no answer, a status that is not ok, a body that is no picture) keeps the picture there
//! was, as today's `.catch(() => {})` does.
//!
//! Sources: the network ([`Feed::network`]: [`URL`] through the app's HTTP client on the
//! runtime, waking the main thread when the answer is in), a `--fixture` folder ([`Feed::fixture`]:
//! [`FIXTURE`] in it, read at the ask itself, so the first frame that asks has it), or none
//! ([`Feed::none`]: shots without a fixture folder and tests never ask).
//!
//! The picture is handed over as the JPEG's bytes ([`Picture::jpeg`]), checked to be a picture
//! (its header read, as `createImageBitmap` would refuse what is not one); the map decodes it on
//! its worker pool. `scene::clouds` has the same constants and the same rule (`picture_due`),
//! ported with `clouds.ts` in stage 1; this module cannot see it (the backend depends on `core`
//! only), so the two are kept equal by hand.
//!
//! Deviations: the ask gives up after [`TIMEOUT`] (a `fetch` has no time limit, and an ask that
//! never ends would stop the asking for good); the picture is not cached on disk (today's browser
//! cache may keep it, for at most what EUMETView's headers allow).

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::runtime::Handle;

use crate::{CmdResult, Wake};

/// Who to credit (`CLOUDS_ATTRIBUTION`).
pub const ATTRIBUTION: &str = "Clouds: © EUMETSAT";
/// The picture's edges, `[west, south, east, north]` (`CLOUD_BOUNDS`): as far north and south as
/// the flat map goes.
pub const BOUNDS: [f64; 4] = [-180.0, -85.0, 180.0, 85.0];
/// The picture: WMS 1.3.0 in EPSG:4326, so its bounding box is latitude first (`clouds.ts`'s
/// `URL`).
pub const URL: &str = "https://view.eumetsat.int/geoserver/wms?service=WMS&version=1.3.0&request=GetMap&layers=mumi:worldcloudmap_ir108&styles=&crs=EPSG:4326&bbox=-85,-180,85,180&width=4096&height=1934&format=image/jpeg";
/// A newer picture is asked for this long after the last ask, ms (`REFRESH_MS`).
pub const REFRESH_MS: f64 = 60.0 * 60_000.0;
/// While there is no picture (after a failure), asked for again this soon, ms (`RETRY_MS`).
pub const RETRY_MS: f64 = 5.0 * 60_000.0;
/// How long an ask may take.
pub const TIMEOUT: Duration = Duration::from_secs(120);
/// A `--fixture` folder's picture, as `cargo xtask fixtures` keeps it (`clouds/`, with a `.json`
/// saying when it was fetched).
pub const FIXTURE: &str = "clouds/worldcloudmap_ir108.jpg";

/// Whether to ask for the picture at `now_ms` (`cloudPicture`'s test): not while an ask is out
/// (`loading`), else [`REFRESH_MS`] after the last ask (`asked_at_ms`, `None` before the first:
/// the TypeScript's `-Infinity`) once there is a picture, [`RETRY_MS`] while there is none.
pub fn is_due(now_ms: f64, asked_at_ms: Option<f64>, have_picture: bool, loading: bool) -> bool {
    let wait = if have_picture { REFRESH_MS } else { RETRY_MS };
    !loading && asked_at_ms.is_none_or(|asked| now_ms - asked > wait)
}

/// A cloud picture as it came: the JPEG's bytes (shared, so the map's scene takes it without a
/// copy) and its size from its header.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub jpeg: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({} × {}, {} bytes)", self.width, self.height, self.jpeg.len())
    }
}

impl Picture {
    /// `bytes` as a picture: an error when its header is not one an image decoder reads (an
    /// error page, a WMS exception sent with status 200).
    pub fn new(bytes: Vec<u8>) -> CmdResult<Picture> {
        let (width, height) = image::ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())?
            .into_dimensions()
            .map_err(|e| format!("the cloud picture is no picture: {e}"))?;
        Ok(Picture { jpeg: bytes.into(), width, height })
    }
}

/// The client the picture is asked with: the app's (`fr24::client_builder()`, doc 07 §2.3).
fn client() -> Option<&'static reqwest::Client> {
    static C: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    C.get_or_init(|| {
        fr24::client_builder()
            .user_agent(concat!("RustRadar/", env!("CARGO_PKG_VERSION"), " (+https://github.com/felixgoff/RustRadar)"))
            .timeout(TIMEOUT)
            .build()
            .ok()
    })
    .as_ref()
}

/// The picture at `url`, asked with `http` (`fetch(URL).then(r => r.ok ? r.blob() : reject)`,
/// then `createImageBitmap`): an error when it cannot be had, its status is not ok, or it is no
/// picture.
pub async fn fetch(http: &reqwest::Client, url: &str) -> CmdResult<Picture> {
    let response = http.get(url).send().await.map_err(|e| format!("can't reach EUMETSAT: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("EUMETSAT answered {}", status.as_u16()));
    }
    let body = response.bytes().await.map_err(|e| e.to_string())?;
    Picture::new(body.to_vec())
}

/// The picture in the `--fixture` folder `dir` ([`FIXTURE`]).
pub fn read(dir: &Path) -> CmdResult<Picture> {
    let path = dir.join(FIXTURE);
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Picture::new(bytes)
}

/// Where a [`Feed`]'s pictures come from.
enum Source {
    /// Nowhere: never asked.
    None,
    /// A `--fixture` folder, read at the ask.
    Fixture(PathBuf),
    /// The network: `url` through `http` on `rt`, `wake` once the answer is in.
    Network { rt: Handle, wake: Wake, http: Option<reqwest::Client>, url: String },
}

/// `cloudPicture`: the latest cloud picture, and the asks for a newer one (see the module docs).
pub struct Feed {
    source: Source,
    /// When the last ask started (local Unix ms; `askedAt`).
    asked_at_ms: Option<f64>,
    /// The ask out (`loading`): its answer, once.
    loading: Option<Receiver<CmdResult<Picture>>>,
    /// The latest picture (`image`).
    picture: Option<Picture>,
}

impl std::fmt::Debug for Feed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let source = match &self.source {
            Source::None => "none".to_owned(),
            Source::Fixture(dir) => dir.display().to_string(),
            Source::Network { url, .. } => url.clone(),
        };
        f.debug_struct("Feed")
            .field("source", &source)
            .field("asked_at_ms", &self.asked_at_ms)
            .field("loading", &self.loading.is_some())
            .field("picture", &self.picture)
            .finish()
    }
}

impl Default for Feed {
    fn default() -> Self {
        Feed::none()
    }
}

impl Feed {
    fn with(source: Source) -> Self {
        Feed { source, asked_at_ms: None, loading: None, picture: None }
    }

    /// A feed that never asks.
    pub fn none() -> Self {
        Feed::with(Source::None)
    }

    /// The picture of the `--fixture` folder `dir`.
    pub fn fixture(dir: impl Into<PathBuf>) -> Self {
        Feed::with(Source::Fixture(dir.into()))
    }

    /// EUMETSAT's picture ([`URL`]) through the app's client, asked on `rt`; `wake` brings the
    /// main thread round once the answer is in.
    pub fn network(rt: Handle, wake: Wake) -> Self {
        Feed::with(Source::Network { rt, wake, http: None, url: URL.to_owned() })
    }

    /// As [`Feed::network`], from `url` with `http` (tests).
    pub fn network_from(rt: Handle, wake: Wake, http: reqwest::Client, url: impl Into<String>) -> Self {
        Feed::with(Source::Network { rt, wake, http: Some(http), url: url.into() })
    }

    /// The feed for the app: the `--fixture` folder's picture when there is one, else the
    /// network's on `network`'s runtime when there is one, else none.
    pub fn new(fixture: Option<&Path>, network: Option<(Handle, Wake)>) -> Self {
        match (fixture, network) {
            (Some(dir), _) => Feed::fixture(dir),
            (None, Some((rt, wake))) => Feed::network(rt, wake),
            (None, None) => Feed::none(),
        }
    }

    /// One frame of `cloudPicture`: while the clouds are `wanted` (they show), asks for the
    /// picture when it is due at `now_ms` (local Unix ms); then takes the answer if it is in.
    /// Whether a new picture came (the scene should be built again: today's `onLoad`).
    pub fn poll(&mut self, now_ms: f64, wanted: bool) -> bool {
        if wanted && !matches!(self.source, Source::None) && is_due(now_ms, self.asked_at_ms, self.picture.is_some(), self.loading.is_some()) {
            self.asked_at_ms = Some(now_ms);
            self.loading = Some(self.ask());
        }
        let Some(rx) = &self.loading else { return false };
        let answer = match rx.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err("the ask for the cloud picture went away".to_owned()),
        };
        self.loading = None;
        match answer {
            Ok(picture) => {
                self.picture = Some(picture);
                true
            }
            Err(e) => {
                log::warn!("clouds: {e}");
                false
            }
        }
    }

    /// Starts an ask; its answer comes on the receiver (at once from a fixture folder).
    fn ask(&self) -> Receiver<CmdResult<Picture>> {
        let (tx, rx) = mpsc::channel();
        match &self.source {
            Source::None => {}
            Source::Fixture(dir) => {
                let _ = tx.send(read(dir));
            }
            Source::Network { rt, wake, http, url } => {
                let (wake, http, url) = (wake.clone(), http.clone(), url.clone());
                rt.spawn(async move {
                    let answer = match http.or_else(|| client().cloned()) {
                        Some(http) => fetch(&http, &url).await,
                        None => Err("no HTTP client".to_owned()),
                    };
                    // (a feed that is gone wants nothing)
                    let _ = tx.send(answer);
                    wake();
                });
            }
        }
        rx
    }

    /// The latest picture (`image`; `None` until one has come).
    pub fn picture(&self) -> Option<&Picture> {
        self.picture.as_ref()
    }

    /// Whether an ask is out (`loading`).
    pub fn loading(&self) -> bool {
        self.loading.is_some()
    }

    /// When the last ask started (`askedAt`; `None` before the first).
    pub fn asked_at_ms(&self) -> Option<f64> {
        self.asked_at_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const T0: f64 = 1_794_446_629_000.0;

    /// The committed fixture folder (`crates/map/tests/data`).
    fn fixture_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../map/tests/data")
    }

    /// A small grey JPEG.
    fn jpeg() -> Vec<u8> {
        let picture = image::GrayImage::from_fn(16, 8, |x, y| image::Luma([(x * 16 + y) as u8]));
        let mut out = Vec::new();
        image::DynamicImage::ImageLuma8(picture).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Jpeg).expect("a JPEG");
        out
    }

    #[test]
    fn the_url_asks_for_the_bounds_latitude_first() {
        let [west, south, east, north] = BOUNDS;
        assert!(URL.contains(&format!("&bbox={south},{west},{north},{east}&")));
        assert!(URL.ends_with("&width=4096&height=1934&format=image/jpeg"));
        assert!(URL.starts_with("https://view.eumetsat.int/geoserver/wms?service=WMS&version=1.3.0&request=GetMap&layers=mumi:worldcloudmap_ir108&styles="));
    }

    /// `clouds.ts`'s rule, as `scene::clouds::picture_due` has it.
    #[test]
    fn asked_for_hourly_and_again_soon_after_a_failure() {
        assert!(is_due(0.0, None, false, false));
        assert!(!is_due(0.0, None, false, true));
        assert!(!is_due(1000.0 + RETRY_MS, Some(1000.0), false, false));
        assert!(is_due(1001.0 + RETRY_MS, Some(1000.0), false, false));
        assert!(!is_due(1001.0 + RETRY_MS, Some(1000.0), true, false));
        assert!(is_due(1001.0 + REFRESH_MS, Some(1000.0), true, false));
        assert!(!is_due(1001.0 + REFRESH_MS, Some(1000.0), true, true));
        assert_eq!((REFRESH_MS, RETRY_MS), (3_600_000.0, 300_000.0));
    }

    #[test]
    fn the_fixture_picture_is_eumetsats_4096_by_1934() {
        let picture = read(&fixture_dir()).expect("the committed picture");
        assert_eq!((picture.width, picture.height), (4096, 1934));
        assert_eq!(&picture.jpeg[..3], &[0xff, 0xd8, 0xff]);
        let json = std::fs::read_to_string(fixture_dir().join("clouds/worldcloudmap_ir108.json")).unwrap();
        assert!(json.contains(&format!("\"url\": \"{URL}\"")), "{json}");
        assert!(read(Path::new("/nonexistent")).is_err());
    }

    #[test]
    fn what_is_no_picture_is_refused() {
        assert!(Picture::new(b"<?xml version=\"1.0\"?><ServiceExceptionReport/>".to_vec()).is_err());
        assert!(Picture::new(Vec::new()).is_err());
        let p = Picture::new(jpeg()).unwrap();
        assert_eq!((p.width, p.height), (16, 8));
        assert_eq!(format!("{p:?}"), format!("Picture(16 × 8, {} bytes)", p.jpeg.len()));
    }

    /// A fixture folder's picture is there at the first poll that wants it, not before; asked
    /// again hourly while wanted.
    #[test]
    fn a_fixture_folder_answers_at_once() {
        let mut feed = Feed::fixture(fixture_dir());
        assert!(!feed.poll(T0, false), "not wanted: not asked");
        assert_eq!((feed.picture(), feed.asked_at_ms()), (None, None));
        assert!(feed.poll(T0, true));
        assert_eq!(feed.picture().map(|p| (p.width, p.height)), Some((4096, 1934)));
        assert_eq!((feed.asked_at_ms(), feed.loading()), (Some(T0), false));
        assert!(!feed.poll(T0 + REFRESH_MS, true), "an hour is not more than an hour");
        assert!(feed.poll(T0 + REFRESH_MS + 1.0, true), "asked again");
        assert_eq!(feed.asked_at_ms(), Some(T0 + REFRESH_MS + 1.0));
        // a folder without the picture: none, asked again after five minutes
        let mut missing = Feed::fixture("/nonexistent");
        assert!(!missing.poll(T0, true));
        assert_eq!((missing.picture(), missing.asked_at_ms(), missing.loading()), (None, Some(T0), false));
        assert!(!missing.poll(T0 + RETRY_MS, true));
        assert_eq!(missing.asked_at_ms(), Some(T0));
        missing.poll(T0 + RETRY_MS + 1.0, true);
        assert_eq!(missing.asked_at_ms(), Some(T0 + RETRY_MS + 1.0));
    }

    #[test]
    fn a_feed_without_a_source_never_asks() {
        let mut feed = Feed::none();
        assert!(!feed.poll(T0, true));
        assert_eq!((feed.picture(), feed.asked_at_ms(), feed.loading()), (None, None, false));
        assert!(matches!(Feed::new(None, None).source, Source::None));
        assert!(matches!(Feed::new(Some(&fixture_dir()), None).source, Source::Fixture(_)));
    }

    /// An HTTP/1.1 server on 127.0.0.1 answering every request with `status` and `body`, counting
    /// the requests.
    async fn serve(status: u16, body: Vec<u8>) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let body: Arc<[u8]> = body.into();
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (h, body) = (h.clone(), body.clone());
                tokio::spawn(async move {
                    let (mut head, mut buf) = (Vec::new(), [0u8; 1024]);
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    h.fetch_add(1, Ordering::SeqCst);
                    let out = format!("HTTP/1.1 {status} Test\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = socket.write_all(out.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (base, hits, task)
    }

    fn local_client() -> reqwest::Client {
        fr24::client_builder().no_proxy().timeout(Duration::from_secs(10)).build().unwrap()
    }

    #[tokio::test]
    async fn a_picture_is_read_and_a_failure_is_an_error() {
        let (base, hits, task) = serve(200, jpeg()).await;
        let p = fetch(&local_client(), &base).await.unwrap();
        assert_eq!((p.width, p.height, hits.load(Ordering::SeqCst)), (16, 8, 1));
        task.abort();
        let (base, _, task) = serve(503, jpeg()).await;
        assert_eq!(fetch(&local_client(), &base).await, Err("EUMETSAT answered 503".to_owned()));
        task.abort();
        // a WMS exception comes with status 200
        let (base, _, task) = serve(200, b"<ServiceExceptionReport/>".to_vec()).await;
        assert!(fetch(&local_client(), &base).await.is_err());
        task.abort();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let gone = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        assert!(fetch(&local_client(), &gone).await.unwrap_err().starts_with("can't reach EUMETSAT"));
    }

    /// Polls `feed` until its ask is answered (or 5 s pass).
    fn settle(feed: &mut Feed, now_ms: f64, wanted: bool) -> bool {
        let start = std::time::Instant::now();
        loop {
            let changed = feed.poll(now_ms, wanted);
            if changed || !feed.loading() || start.elapsed() > Duration::from_secs(5) {
                return changed;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Over the network: one ask out at a time, the answer taken with a wake; a failure keeps
    /// the picture and waits an hour, none before the first picture five minutes.
    #[test]
    fn the_network_is_asked_while_wanted_and_one_ask_at_a_time() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (base, hits, task) = rt.block_on(serve(200, jpeg()));
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let wake: Wake = Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        });
        let mut feed = Feed::network_from(rt.handle().clone(), wake, local_client(), base.clone());
        assert!(!feed.poll(T0, false));
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        feed.poll(T0, true);
        assert!(feed.loading());
        // while it is out no second ask, though five minutes have passed
        assert!(settle(&mut feed, T0 + RETRY_MS + 1.0, true));
        assert_eq!((hits.load(Ordering::SeqCst), wakes.load(Ordering::SeqCst)), (1, 1));
        assert_eq!(feed.picture().map(|p| p.width), Some(16));
        assert_eq!(feed.asked_at_ms(), Some(T0));
        task.abort();
        // the server gone: the ask fails, the picture stays, the next ask an hour after this one
        let at = T0 + REFRESH_MS + 1.0;
        feed.poll(at, true);
        assert!(!settle(&mut feed, at, true));
        assert_eq!(feed.picture().map(|p| p.width), Some(16));
        assert!(!feed.poll(at + RETRY_MS + 1.0, true) && !feed.loading(), "with a picture, not after five minutes");
        // with none: five minutes
        let mut fresh = Feed::network_from(rt.handle().clone(), Arc::new(|| {}), local_client(), base);
        fresh.poll(T0, true);
        assert!(!settle(&mut fresh, T0, true));
        assert_eq!(fresh.picture(), None);
        fresh.poll(T0 + RETRY_MS + 1.0, true);
        assert_eq!(fresh.asked_at_ms(), Some(T0 + RETRY_MS + 1.0));
    }
}
