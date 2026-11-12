//! Remote pictures as egui textures (plan 1.5, 2.3; item U18): the aircraft photos and airline
//! logos of the hover card and the flight panel, the ships' and trains' photos and the operators'
//! logos, which today's `<img>` elements loaded from the network.
//!
//! [`Loader`] is an egui texture loader for `http:` and `https:` URIs, so the UI asks for a picture
//! as for any other image: `Context::try_load_texture` (`ui::panel::remote`), which answers
//! pending, a texture with its size in texels, or a failure (`<img>`'s `onerror`). Behind it:
//!
//! - the first ask for a URI starts its download on the backend's runtime through
//!   `rustradar_backend::images` (the disk cache in `<cache>/images`, 64 MB; its two-minute
//!   failure memo; decoding to RGBA on the blocking pool), then turns the picture into egui's
//!   premultiplied colours there too, and wakes the event loop (the backend's wake and
//!   `request_repaint`);
//! - the next ask after that, in the UI pass, uploads the texture and answers ready (the decoded
//!   pixels are dropped once uploaded: the texture is the only copy);
//! - a failure is remembered while the URI is asked for (an immediate-mode UI asks every pass), so
//!   a picture that did not load stays out without flickering; one not asked for
//!   [`FORGET_FAILED`] is forgotten, so a component that shows it again later (a card or a panel
//!   mounted again) tries again, as a new `<img>` does: the backend's memo answers at once for its
//!   two minutes;
//! - the textures together are kept under [`TEXTURE_CAP`]: past it, the least recently asked go
//!   first, never one asked in this pass or the one before (it is still on screen). One asked
//!   again after that loads again, from the disk cache.
//!
//! Each texture is keyed by its URI and its options, as egui's own texture loader keys them.
//! Other URIs (`bytes://`, `file://`, a test's own scheme) are not this loader's: egui asks the
//! next one.
//!
//! **Sources** ([`Source`]): the backend's picture cache, as above; or a `--fixture` folder's
//! pictures ([`fixture_file`]: `images/<host>/<path>`), read and decoded with the backend's
//! [`decode`] in the pass that first asks, so a shot has them in its first frame; or none, every
//! remote picture failing at once without asking anyone.
//!
//! **Wiring**: the window installs one ([`connect`]) on its egui context, and again on the new
//! context after a lost device (B9): the `--fixture` folder's pictures when it has one (as its
//! tiles and feeds come from there), else the backend's when the app has one. The headless shots
//! install theirs ([`connect_offline`], item F2): the `--fixture` folder's pictures, else none,
//! for a shot never touches the network; a remote picture a shot has no file for reads as failed
//! (left out, or the `--surface-raised` block), as today when one does not load. The tests of
//! other items install their own loaders or none.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::load::{LoadError, SizedTexture, TextureLoadResult, TextureLoader, TexturePoll};
use egui::{ColorImage, Context, SizeHint, TextureHandle, TextureOptions, Vec2};
use rustradar_backend::Wake;
use rustradar_backend::images::{Images, Picture, decode};
use tokio::runtime::Handle;

use crate::state::AppState;

/// The loader's id (egui's `generate_loader_id!` form).
pub const LOADER_ID: &str = concat!(module_path!(), "::Loader");
/// The most the uploaded textures may take together, bytes.
pub const TEXTURE_CAP: usize = if cfg!(any(target_os = "android", target_os = "ios")) { 16 << 20 } else { 32 << 20 };
/// How long a failed URI that nobody asks for is remembered here.
pub const FORGET_FAILED: Duration = Duration::from_secs(5);

/// The `--fixture` folder's pictures: `images/<host>/<path>` ([`fixture_file`]).
pub const FIXTURE_IMAGES: &str = "images";
/// Why a remote picture fails where the loader has no source (a shot without a fixture folder).
pub const NO_SOURCE: &str = "no remote pictures in this run";

/// Whether `uri` is this loader's: an `http:` or `https:` URL.
pub fn is_remote(uri: &str) -> bool {
    let lower = |n: usize| uri.get(..n).map(str::to_ascii_lowercase);
    lower(7).as_deref() == Some("http://") || lower(8).as_deref() == Some("https://")
}

/// Where the `--fixture` folder `dir` keeps the picture at `url`: `images/<host>/<path>`, the
/// URL's host (lower case, without a user name) and path, so
/// `https://cdn.jetphotos.com/400/6/1.jpg` is `images/cdn.jetphotos.com/400/6/1.jpg`. A query is
/// joined to the file's name after a `_`; a fragment is left out. In every part the characters
/// other than ASCII letters, digits, `-`, `.` and `_` are `_`, and empty, `.` and `..` parts are
/// left out, so the file is always inside `dir/images`. `None` for a URL that is not `http:` or
/// `https:`, or that names no file.
pub fn fixture_file(dir: &Path, url: &str) -> Option<PathBuf> {
    if !is_remote(url) {
        return None;
    }
    let rest = &url[url.find("://")? + 3..];
    let rest = rest.split('#').next().unwrap_or_default();
    let (rest, query) = match rest.split_once('?') {
        Some((rest, query)) => (rest, Some(query)),
        None => (rest, None),
    };
    let clean = |part: &str| -> String { part.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') { c } else { '_' }).collect() };
    let kept = |part: &String| !part.is_empty() && part != "." && part != "..";
    let mut parts = rest.split('/');
    let host = parts.next()?;
    let host = clean(&host.rsplit('@').next().unwrap_or(host).to_ascii_lowercase());
    let mut path: Vec<String> = parts.map(clean).filter(kept).collect();
    if !kept(&host) || path.is_empty() {
        return None;
    }
    if let (Some(query), Some(name)) = (query.filter(|q| !q.is_empty()), path.last_mut()) {
        name.push('_');
        name.push_str(&clean(query));
    }
    Some(path.iter().fold(dir.join(FIXTURE_IMAGES).join(host), |file, part| file.join(part)))
}

/// Where a loader's pictures come from (see the module documentation).
pub enum Source {
    /// The backend's picture cache (`images`), downloading on `rt`, calling `wake` when an
    /// answer is in.
    Backend { images: Images, rt: Handle, wake: Option<Wake> },
    /// A `--fixture` folder's pictures ([`fixture_file`]), read in the pass that asks.
    Folder(PathBuf),
    /// None: every remote picture fails at once ([`NO_SOURCE`]).
    None,
}

/// A download's answer, as the background task leaves it for the UI pass.
type Answer = Arc<Mutex<Option<Result<ColorImage, String>>>>;

enum Slot {
    /// On its way; the answer once it is in.
    Loading(Answer),
    /// Uploaded: the texture and its size in texels.
    Ready(TextureHandle, Vec2),
    Failed(String),
}

struct Entry {
    slot: Slot,
    /// The pass and the moment it was last asked for.
    pass: u64,
    at: Instant,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    uri: String,
    options: TextureOptions,
}

#[derive(Default)]
struct State {
    pass: u64,
    entries: HashMap<Key, Entry>,
}

/// The texture loader for remote pictures (see the module documentation).
pub struct Loader {
    source: Source,
    texture_cap: usize,
    forget_failed: Duration,
    state: Mutex<State>,
}

impl Loader {
    /// A loader fetching through `images` on `rt`, calling `wake` when an answer is in.
    pub fn new(images: Images, rt: Handle, wake: Option<Wake>) -> Self {
        Loader::with_limits(images, rt, wake, TEXTURE_CAP, FORGET_FAILED)
    }

    /// [`Self::new`] with its own texture cap and failure time (tests).
    pub fn with_limits(images: Images, rt: Handle, wake: Option<Wake>, texture_cap: usize, forget_failed: Duration) -> Self {
        Loader::with_source(Source::Backend { images, rt, wake }, texture_cap, forget_failed)
    }

    /// A loader on the `--fixture` folder `dir`'s pictures, or on none without one: nothing it
    /// does reaches the network.
    pub fn offline(dir: Option<PathBuf>) -> Self {
        Loader::with_source(dir.map_or(Source::None, Source::Folder), TEXTURE_CAP, FORGET_FAILED)
    }

    /// A loader on `source` with its own texture cap and failure time.
    pub fn with_source(source: Source, texture_cap: usize, forget_failed: Duration) -> Self {
        Loader { source, texture_cap, forget_failed, state: Mutex::new(State::default()) }
    }

    /// Installs `self` on `ctx`, unless a loader of this kind is there already.
    pub fn install(self: Arc<Self>, ctx: &Context) {
        if !ctx.is_loader_installed(LOADER_ID) {
            ctx.add_texture_loader(self);
        }
    }

    /// How many URIs are on their way, loaded and failed.
    pub fn counts(&self) -> (usize, usize, usize) {
        let state = self.state.lock().unwrap();
        state.entries.values().fold((0, 0, 0), |(l, r, f), e| match e.slot {
            Slot::Loading(_) => (l + 1, r, f),
            Slot::Ready(..) => (l, r + 1, f),
            Slot::Failed(_) => (l, r, f + 1),
        })
    }

    /// Starts the download of `uri`, the answer left in the slot it returns (already there for a
    /// fixture folder or no source: taken in the same pass).
    fn start(&self, ctx: &Context, uri: &str) -> Slot {
        let (images, rt, wake) = match &self.source {
            Source::Backend { images, rt, wake } => (images, rt, wake),
            Source::Folder(dir) => {
                let result = read_fixture(dir, uri);
                if let Err(e) = &result {
                    log::debug!("picture {uri}: {e}");
                }
                return Slot::Loading(Arc::new(Mutex::new(Some(result))));
            }
            Source::None => return Slot::Loading(Arc::new(Mutex::new(Some(Err(NO_SOURCE.to_owned()))))),
        };
        let answer: Answer = Arc::new(Mutex::new(None));
        let (images, url, ctx, wake, out) = (images.clone(), uri.to_owned(), ctx.clone(), wake.clone(), Arc::clone(&answer));
        // a spawn from the main thread: through the runtime's handle (doc 07 §11.4)
        rt.spawn(async move {
            let result = match images.picture(&url).await {
                Ok(picture) => tokio::task::spawn_blocking(move || color_image(&picture)).await.map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            if let Err(e) = &result {
                log::debug!("picture {url}: {e}");
            }
            *out.lock().unwrap() = Some(result);
            ctx.request_repaint();
            if let Some(wake) = wake {
                wake();
            }
        });
        Slot::Loading(answer)
    }
}

/// `picture` as egui's colours (premultiplied).
pub fn color_image(picture: &Picture) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([picture.width as usize, picture.height as usize], &picture.rgba)
}

/// The `--fixture` folder `dir`'s picture for `url` ([`fixture_file`]), decoded as the backend's
/// pictures are ([`decode`]).
fn read_fixture(dir: &Path, url: &str) -> Result<ColorImage, String> {
    let file = fixture_file(dir, url).ok_or_else(|| format!("{url}: no fixture file for it"))?;
    let bytes = std::fs::read(&file).map_err(|e| format!("fixture {}: {e}", file.display()))?;
    decode(&bytes).map(|picture| color_image(&picture)).map_err(|e| format!("fixture {}: {e}", file.display()))
}

impl TextureLoader for Loader {
    fn id(&self) -> &str {
        LOADER_ID
    }

    fn load(&self, ctx: &Context, uri: &str, texture_options: TextureOptions, _size_hint: SizeHint) -> TextureLoadResult {
        if !is_remote(uri) {
            return Err(LoadError::NotSupported);
        }
        let mut state = self.state.lock().unwrap();
        let pass = state.pass;
        let key = Key { uri: uri.to_owned(), options: texture_options };
        if !state.entries.contains_key(&key) {
            let slot = self.start(ctx, uri);
            state.entries.insert(key.clone(), Entry { slot, pass, at: Instant::now() });
        }
        let Some(entry) = state.entries.get_mut(&key) else { return Err(LoadError::NotSupported) };
        entry.pass = pass;
        entry.at = Instant::now();
        // the answer in: uploaded (or failed) in this pass
        if let Slot::Loading(answer) = &entry.slot {
            let taken = answer.lock().unwrap().take();
            match taken {
                Some(Ok(image)) => {
                    let size = Vec2::new(image.size[0] as f32, image.size[1] as f32);
                    entry.slot = Slot::Ready(ctx.load_texture(uri, image, texture_options), size);
                }
                Some(Err(e)) => entry.slot = Slot::Failed(e),
                None => {}
            }
        }
        match &entry.slot {
            Slot::Loading(_) => Ok(TexturePoll::Pending { size: None }),
            Slot::Ready(handle, size) => Ok(TexturePoll::Ready { texture: SizedTexture::new(handle.id(), *size) }),
            Slot::Failed(e) => Err(LoadError::Loading(e.clone())),
        }
    }

    fn forget(&self, uri: &str) {
        self.state.lock().unwrap().entries.retain(|key, _| key.uri != uri);
    }

    fn forget_all(&self) {
        self.state.lock().unwrap().entries.clear();
    }

    fn end_pass(&self, pass_index: u64) {
        let mut state = self.state.lock().unwrap();
        state.pass = pass_index;
        let forget_failed = self.forget_failed;
        state.entries.retain(|_, e| !matches!(e.slot, Slot::Failed(_)) || e.at.elapsed() < forget_failed);
        // past the cap, the least recently asked go first; none still on screen
        let bytes = |e: &Entry| match &e.slot {
            Slot::Ready(handle, _) => handle.byte_size(),
            _ => 0,
        };
        let mut total: usize = state.entries.values().map(bytes).sum();
        if total <= self.texture_cap {
            return;
        }
        let mut old: Vec<(u64, Key, usize)> = state
            .entries
            .iter()
            .filter(|(_, e)| matches!(e.slot, Slot::Ready(..)) && e.pass + 1 < pass_index)
            .map(|(k, e)| (e.pass, k.clone(), bytes(e)))
            .collect();
        old.sort_by_key(|(pass, _, _)| *pass);
        for (_, key, size) in old {
            if total <= self.texture_cap {
                break;
            }
            state.entries.remove(&key);
            total -= size;
        }
    }

    fn byte_size(&self) -> usize {
        let state = self.state.lock().unwrap();
        state
            .entries
            .values()
            .map(|e| match &e.slot {
                Slot::Ready(handle, _) => handle.byte_size(),
                _ => 0,
            })
            .sum()
    }
}

/// The window's loader for `state` installed on `ctx` (see the module documentation): the
/// `--fixture` folder's pictures when it has one; else through the backend's picture cache in its
/// cache folder, on its runtime, waking its loop. Nothing without either, or when the cache folder
/// does not open (remote pictures then read as failed).
pub fn connect(ctx: &Context, state: &AppState) -> Option<Arc<Loader>> {
    if state.startup.fixture.is_some() {
        return Some(connect_offline(ctx, state));
    }
    let backend = state.backend.as_ref()?;
    let images = match Images::new(&backend.dirs().cache) {
        Ok(images) => images,
        Err(e) => {
            log::error!("no remote pictures: the picture cache did not open: {e}");
            return None;
        }
    };
    let loader = Arc::new(Loader::new(images, backend.rt().clone(), Some(backend.waker())));
    Arc::clone(&loader).install(ctx);
    Some(loader)
}

/// A shot's loader for `state` installed on `ctx` (`--screenshot`, item F2): the `--fixture`
/// folder's pictures, else none, never the network (see the module documentation).
pub fn connect_offline(ctx: &Context, state: &AppState) -> Arc<Loader> {
    let loader = Arc::new(Loader::offline(state.startup.fixture.clone()));
    Arc::clone(&loader).install(ctx);
    loader
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};

    use super::*;
    use crate::requests;
    use crate::ui::panel::{self, Remote};

    /// An HTTP/1.1 server on 127.0.0.1 on threads of its own, answering each path with the status
    /// and body `handler` gives. Counts the requests.
    struct Server {
        base: String,
        hits: Arc<AtomicUsize>,
    }

    impl Server {
        fn start(handler: impl Fn(&str) -> (u16, Vec<u8>) + Send + Sync + 'static) -> Server {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let hits = Arc::new(AtomicUsize::new(0));
            let (handler, counted) = (Arc::new(handler), Arc::clone(&hits));
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let (handler, counted) = (Arc::clone(&handler), Arc::clone(&counted));
                    std::thread::spawn(move || {
                        let (mut head, mut buf) = (Vec::new(), [0u8; 1024]);
                        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                            match stream.read(&mut buf) {
                                Ok(0) | Err(_) => return,
                                Ok(n) => head.extend_from_slice(&buf[..n]),
                            }
                        }
                        counted.fetch_add(1, Ordering::SeqCst);
                        let path = String::from_utf8_lossy(&head).split_whitespace().nth(1).unwrap_or("/").to_owned();
                        let (status, body) = handler(&path);
                        let head = format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        let _ = stream.write_all(head.as_bytes());
                        let _ = stream.write_all(&body);
                    });
                }
            });
            Server { base, hits }
        }

        fn url(&self, path: &str) -> String {
            format!("{}{path}", self.base)
        }

        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    /// A picture cache folder of its own for one test, removed when it ends.
    struct TempCache(PathBuf);

    impl TempCache {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("rustradar-app-images-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            TempCache(root)
        }
    }

    impl Drop for TempCache {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A `width` × `height` PNG of one colour.
    fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&rgba.repeat((width * height) as usize)).unwrap();
        writer.finish().unwrap();
        out
    }

    /// An egui context with `loader` installed and a counter of its wakes.
    struct Ui {
        ctx: Context,
        loader: Arc<Loader>,
        woken: Arc<AtomicUsize>,
    }

    impl Ui {
        fn new(cache: &TempCache, texture_cap: usize, forget_failed: Duration) -> Ui {
            let rt = requests::runtime().expect("runtime");
            let woken = Arc::new(AtomicUsize::new(0));
            let count = Arc::clone(&woken);
            let wake: Wake = Arc::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            });
            let images = Images::new(&cache.0).unwrap();
            let loader = Arc::new(Loader::with_limits(images, rt.handle().clone(), Some(wake), texture_cap, forget_failed));
            let ctx = Context::default();
            Arc::clone(&loader).install(&ctx);
            Ui { ctx, loader, woken }
        }

        /// One egui pass asking for each of `uris`; what each read as.
        fn pass(&self, uris: &[&str]) -> Vec<Remote> {
            let mut seen = Vec::new();
            let mut output = self.ctx.run_ui(egui::RawInput::default(), |ui| {
                seen = uris.iter().map(|uri| panel::remote(ui.ctx(), uri)).collect();
            });
            output.textures_delta.clear();
            seen
        }

        /// Passes until `uri` is no longer loading (at most 10 s).
        fn settled(&self, uri: &str) -> Remote {
            let started = Instant::now();
            loop {
                match self.pass(&[uri])[0] {
                    Remote::Loading => {}
                    other => return other,
                }
                assert!(started.elapsed() < Duration::from_secs(10), "{uri} still loading");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    #[test]
    fn a_picture_loads_as_a_texture_and_wakes_the_loop() {
        let cache = TempCache::new("loads");
        let body = png(64, 36, [10, 20, 30, 255]);
        let server = Server::start(move |_| (200, body.clone()));
        let ui = Ui::new(&cache, TEXTURE_CAP, FORGET_FAILED);
        let uri = server.url("/photo.png");
        assert_eq!(ui.pass(&[&uri]), [Remote::Loading], "pending at first");
        let Remote::Ready(texture) = ui.settled(&uri) else { panic!("not loaded") };
        assert_eq!(texture.size, Vec2::new(64.0, 36.0), "its size in texels");
        assert!(ui.woken.load(Ordering::SeqCst) >= 1, "the loop was woken");
        assert_eq!(ui.loader.counts(), (0, 1, 0));
        assert_eq!(ui.loader.byte_size(), 64 * 36 * 4);
        // asked again: the same texture, nothing downloaded again
        assert_eq!(ui.pass(&[&uri]), [Remote::Ready(texture)]);
        assert_eq!(server.hits(), 1);
        assert!(cache.0.join("images").is_dir(), "kept in <cache>/images");
    }

    #[test]
    fn other_schemes_are_left_to_other_loaders() {
        let cache = TempCache::new("schemes");
        let ui = Ui::new(&cache, TEXTURE_CAP, FORGET_FAILED);
        assert!(is_remote("https://cdn.jetphotos.com/a.jpg") && is_remote("HTTP://x/y.png"));
        assert!(!is_remote("bytes://logo.png") && !is_remote("file:///tmp/a.png") && !is_remote("test://photo1.png") && !is_remote(""));
        // nothing else loads them in this context: no loader of egui's answers either
        assert_eq!(ui.pass(&["test://photo1.png", "bytes://x.png"]), [Remote::Failed, Remote::Failed]);
        assert_eq!(ui.loader.counts(), (0, 0, 0));
    }

    #[test]
    fn a_failure_stays_while_asked_and_is_tried_again_later() {
        let cache = TempCache::new("failed");
        let status = Arc::new(AtomicU16::new(404));
        let body = png(4, 4, [1, 2, 3, 255]);
        let s = Arc::clone(&status);
        let server = Server::start(move |_| match s.load(Ordering::SeqCst) {
            200 => (200, body.clone()),
            code => (code, b"no".to_vec()),
        });
        let ui = Ui::new(&cache, TEXTURE_CAP, Duration::from_millis(200));
        let uri = server.url("/logos/999999.png");
        assert_eq!(ui.settled(&uri), Remote::Failed);
        // the server has it now, but while the UI keeps asking the failure stands
        status.store(200, Ordering::SeqCst);
        for _ in 0..5 {
            assert_eq!(ui.pass(&[&uri]), [Remote::Failed]);
            std::thread::sleep(Duration::from_millis(60));
        }
        assert_eq!(ui.loader.counts(), (0, 0, 1));
        // not asked for a while: forgotten here; the backend still remembers it for two minutes
        std::thread::sleep(Duration::from_millis(250));
        ui.pass(&[]);
        assert_eq!(ui.loader.counts(), (0, 0, 0));
        assert_eq!(ui.settled(&uri), Remote::Failed, "the backend's memo answered");
        assert_eq!(server.hits(), 1, "without asking the server");
    }

    #[test]
    fn past_the_cap_the_least_recently_asked_textures_go() {
        let cache = TempCache::new("cap");
        let server = Server::start(|path| (200, png(32, 32, [path.len() as u8, 0, 0, 255])));
        // room for two 32 × 32 textures (4 KB each)
        let ui = Ui::new(&cache, 2 * 32 * 32 * 4, FORGET_FAILED);
        let uris: Vec<String> = (0..3).map(|n| server.url(&format!("/{n}.png"))).collect();
        for uri in &uris[..2] {
            assert!(matches!(ui.settled(uri), Remote::Ready(_)));
        }
        // a few passes asking only for the second; then the third comes in
        for _ in 0..3 {
            ui.pass(&[&uris[1]]);
        }
        assert!(matches!(ui.settled(&uris[2]), Remote::Ready(_)));
        ui.pass(&[&uris[1], &uris[2]]);
        assert_eq!(ui.loader.counts(), (0, 2, 0), "the first went");
        assert!(ui.loader.byte_size() <= 2 * 32 * 32 * 4);
        // asked again, it loads again: from the disk cache, not the server
        assert!(matches!(ui.settled(&uris[0]), Remote::Ready(_)));
        assert_eq!(server.hits(), 3);
    }

    #[test]
    fn two_options_are_two_textures_and_forget_drops_both() {
        let cache = TempCache::new("options");
        let body = png(8, 8, [9, 9, 9, 255]);
        let server = Server::start(move |_| (200, body.clone()));
        let ui = Ui::new(&cache, TEXTURE_CAP, FORGET_FAILED);
        let uri = server.url("/a.png");
        assert!(matches!(ui.settled(&uri), Remote::Ready(_)));
        let nearest = || ui.ctx.try_load_texture(&uri, TextureOptions::NEAREST, SizeHint::default());
        let started = Instant::now();
        while !matches!(nearest(), Ok(TexturePoll::Ready { .. })) {
            assert!(started.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(ui.loader.counts(), (0, 2, 0));
        ui.ctx.forget_image(&uri);
        assert_eq!(ui.loader.counts(), (0, 0, 0));
    }

    #[test]
    fn colours_are_premultiplied() {
        let picture = Picture { width: 1, height: 1, rgba: vec![200, 100, 50, 128] };
        let image = color_image(&picture);
        assert_eq!(image.size, [1, 1]);
        assert_eq!(image.pixels[0], egui::Color32::from_rgba_unmultiplied(200, 100, 50, 128));
    }

    /// An egui context with an offline loader on `dir` (or none) installed.
    fn offline(dir: Option<PathBuf>) -> (Context, Arc<Loader>) {
        let ctx = Context::default();
        let loader = Arc::new(Loader::offline(dir));
        Arc::clone(&loader).install(&ctx);
        (ctx, loader)
    }

    fn pass(ctx: &Context, uris: &[&str]) -> Vec<Remote> {
        let mut seen = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            seen = uris.iter().map(|uri| panel::remote(ui.ctx(), uri)).collect();
        });
        output.textures_delta.clear();
        seen
    }

    #[test]
    fn a_fixture_folder_keeps_a_picture_under_its_host_and_path() {
        let dir = Path::new("fx");
        let file = |url: &str| fixture_file(dir, url);
        let at = |parts: &[&str]| Some(parts.iter().fold(dir.join(FIXTURE_IMAGES), |f, p| f.join(p)));
        assert_eq!(file("https://cdn.jetphotos.com/400/6/1_17.jpg"), at(&["cdn.jetphotos.com", "400", "6", "1_17.jpg"]));
        assert_eq!(file("https://cdn.flightradar24.com/assets/airlines/logotypes/9.png"), at(&["cdn.flightradar24.com", "assets", "airlines", "logotypes", "9.png"]));
        // the host in lower case without a user or a port's colon; the query joined, the fragment left out
        assert_eq!(file("HTTP://User@Static.VesselFinder.net:8080/ship-photo/9234567.jpg#top"), at(&["static.vesselfinder.net_8080", "ship-photo", "9234567.jpg"]));
        assert_eq!(file("https://commons.wikimedia.org/wiki/Special:FilePath/X 2.jpg?width=400"), at(&["commons.wikimedia.org", "wiki", "Special_FilePath", "X_2.jpg_width_400"]));
        // never out of the folder
        assert_eq!(file("https://h/../../etc/passwd"), at(&["h", "etc", "passwd"]));
        assert_eq!(file("https://../a.png"), None);
        assert_eq!(file("https://h/./"), None);
        assert_eq!(file("https://h"), None);
        assert_eq!(file("file:///tmp/a.png"), None);
        assert_eq!(file("test://photo1.png"), None);
    }

    #[test]
    fn a_fixture_folder_serves_its_pictures_in_the_pass_that_asks() {
        let cache = TempCache::new("fixture");
        let picture = cache.0.join(FIXTURE_IMAGES).join("cdn.jetphotos.com").join("400");
        std::fs::create_dir_all(&picture).unwrap();
        std::fs::write(picture.join("1.png"), png(30, 20, [200, 100, 50, 255])).unwrap();
        std::fs::write(picture.join("2.png"), b"<html>not a picture</html>").unwrap();
        let (ctx, loader) = offline(Some(cache.0.clone()));
        let [ready, not_a_picture, missing] = ["https://cdn.jetphotos.com/400/1.png", "https://cdn.jetphotos.com/400/2.png", "https://cdn.jetphotos.com/400/3.png"];
        let seen = pass(&ctx, &[ready, not_a_picture, missing, "test://photo1.png"]);
        let Remote::Ready(texture) = seen[0] else { panic!("not ready in the first pass: {:?}", seen[0]) };
        assert_eq!(texture.size, Vec2::new(30.0, 20.0));
        assert_eq!(seen[1..], [Remote::Failed, Remote::Failed, Remote::Failed]);
        assert_eq!(loader.counts(), (0, 1, 2), "the other scheme is not the loader's");
        assert_eq!(loader.byte_size(), 30 * 20 * 4);
    }

    #[test]
    fn without_a_fixture_folder_nothing_is_asked() {
        let body = png(4, 4, [1, 2, 3, 255]);
        let server = Server::start(move |_| (200, body.clone()));
        let (ctx, loader) = offline(None);
        let uri = server.url("/photo.png");
        assert_eq!(pass(&ctx, &[&uri]), [Remote::Failed], "failed at once");
        assert_eq!(pass(&ctx, &[&uri]), [Remote::Failed]);
        assert_eq!(loader.counts(), (0, 0, 1));
        assert_eq!(server.hits(), 0, "the network was not asked");
    }

    #[test]
    fn a_shot_s_loader_reads_the_fixture_folder_or_nothing() {
        use crate::state::Startup;
        use rustradar_scene::camera::ViewState;
        let state = |fixture: Option<PathBuf>| AppState::new(Startup { fixture, ..Default::default() }, None, None, None, ViewState::INITIAL, [1440.0, 900.0]);
        let cache = TempCache::new("connect");
        let folder = cache.0.join(FIXTURE_IMAGES).join("h");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("a.png"), png(2, 2, [9, 9, 9, 255])).unwrap();
        for (fixture, want) in [(Some(cache.0.clone()), (0, 1, 0)), (None, (0, 0, 1))] {
            let ctx = Context::default();
            let loader = connect_offline(&ctx, &state(fixture.clone()));
            assert!(ctx.is_loader_installed(LOADER_ID));
            pass(&ctx, &["https://h/a.png"]);
            assert_eq!(loader.counts(), want, "{fixture:?}");
            // the window's: the fixture folder's too, and nothing without a fixture or a backend
            let ctx = Context::default();
            let window = connect(&ctx, &state(fixture.clone()));
            assert_eq!(window.is_some(), fixture.is_some());
        }
    }
}
