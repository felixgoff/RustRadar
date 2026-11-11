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
//! **Wiring**: the window installs one ([`connect`]) on its egui context, and again on the new
//! context after a lost device (B9), when the app has a backend. The headless shots and the tests
//! of other items install none: a shot never touches the network, so every remote picture there
//! reads as failed (left out, or the `--surface-raised` block), as today when one does not load.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::load::{LoadError, SizedTexture, TextureLoadResult, TextureLoader, TexturePoll};
use egui::{ColorImage, Context, SizeHint, TextureHandle, TextureOptions, Vec2};
use rustradar_backend::Wake;
use rustradar_backend::images::{Images, Picture};
use tokio::runtime::Handle;

use crate::state::AppState;

/// The loader's id (egui's `generate_loader_id!` form).
pub const LOADER_ID: &str = concat!(module_path!(), "::Loader");
/// The most the uploaded textures may take together, bytes.
pub const TEXTURE_CAP: usize = if cfg!(any(target_os = "android", target_os = "ios")) { 16 << 20 } else { 32 << 20 };
/// How long a failed URI that nobody asks for is remembered here.
pub const FORGET_FAILED: Duration = Duration::from_secs(5);

/// Whether `uri` is this loader's: an `http:` or `https:` URL.
pub fn is_remote(uri: &str) -> bool {
    let lower = |n: usize| uri.get(..n).map(str::to_ascii_lowercase);
    lower(7).as_deref() == Some("http://") || lower(8).as_deref() == Some("https://")
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
    images: Images,
    rt: Handle,
    wake: Option<Wake>,
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
        Loader { images, rt, wake, texture_cap, forget_failed, state: Mutex::new(State::default()) }
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

    /// Starts the download of `uri`, the answer left in the slot it returns.
    fn start(&self, ctx: &Context, uri: &str) -> Slot {
        let answer: Answer = Arc::new(Mutex::new(None));
        let (images, url, ctx, wake, out) = (self.images.clone(), uri.to_owned(), ctx.clone(), self.wake.clone(), Arc::clone(&answer));
        // a spawn from the main thread: through the runtime's handle (doc 07 §11.4)
        self.rt.spawn(async move {
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

/// The window's loader for `state` installed on `ctx` (see the module documentation): through the
/// backend's picture cache in its cache folder, on its runtime, waking its loop. Nothing without a
/// backend, or when the cache folder does not open (remote pictures then read as failed).
pub fn connect(ctx: &Context, state: &AppState) -> Option<Arc<Loader>> {
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
}
