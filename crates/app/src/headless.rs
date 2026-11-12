//! `--screenshot` (plan section 8): the app's frames without a window, read back as a PNG.
//!
//! A shot runs the whole app as the window does (the backend started, the state, the scene
//! builders, the map, the egui pass) on a stand-in surface, with its clock standing still at
//! `--now` (or the time it started). It touches no folder of the user's: the backend gets fresh
//! cache and config folders, removed afterwards, so no saved keys and no cached tiles take part.
//! Its map tiles never come from the network: from the `--fixture` folder, else the committed
//! fixtures ([`tile_fetcher::shot_tiles`]); a tile the folder lacks is not drawn (and logged).
//! It runs frames until no layer waits for work, its tiles above all (at most `--wait` seconds,
//! else [`max_wait`]: `RUSTRADAR_SHOT_WAIT`, else [`MAX_WAIT`]), then two more with egui's clock a
//! second apart (an egui area is invisible on its first frame, spike pitfall 9). Only the last
//! frame is drawn ([`Engine::draw`]): the others poll, build the scene, run egui and prepare the
//! map as a drawn frame does, but nobody sees their picture, and on SwiftShader a map pass can
//! take seconds (P1: V2's dark map, about five). With `--ui` the shot draws the window's UI over
//! the map ([`Engine::shell`]), its last frames a quarter of a second of egui's clock apart for
//! the UI to settle; `--select` opens it with a flight selected, `--mode` with that focus. Its
//! remote pictures (photos, logos) come from the `--fixture` folder's `images/`, else from nowhere
//! ([`images::connect_offline`], item F2): one without a file there reads as failed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use rustradar_backend::Dirs;
use rustradar_map::Gpu;
pub use rustradar_map::headless::{FORMAT, Image, MAX_WAIT, Offscreen, max_wait};
use rustradar_map::io::{self, TileFetcher};
use rustradar_scene::camera::ViewState;

use crate::app::{Engine, start_backend};
use crate::requests::{self, Requests, no_wake};
use crate::state::aircraft::Aircraft;
use crate::state::airport::Airport;
use crate::state::blocks::Blocks;
use crate::state::flight::{Flight, SelectOptions};
use crate::state::ships::Ships;
use crate::state::liveries::Liveries;
use crate::state::taxiways::Taxiways;
use crate::state::{AppState, Clock, system_now_ms};
use crate::{Options, SHOT_SIZE};
use crate::{images, tile_fetcher};

type Error = Box<dyn std::error::Error>;

/// A shot: the picture and what drew it.
#[derive(Debug, Clone)]
pub struct Shot {
    pub image: Image,
    /// Adapter, backend and the map's MSAA samples, for the log.
    pub adapter: String,
    /// Whether the backend started.
    pub backend: bool,
    /// The tiles the map asked for that the shot's tile folder lacks (source, `z/x/y`), drawn as
    /// nothing.
    pub missing_tiles: Vec<(String, String)>,
}

/// Cache and config folders of the shot's own, removed when it ends.
struct ShotFolders(PathBuf);

impl ShotFolders {
    fn new() -> std::io::Result<Self> {
        static SHOTS: AtomicUsize = AtomicUsize::new(0);
        let n = SHOTS.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("rustradar-shot-{}-{n}", std::process::id()));
        std::fs::create_dir_all(root.join("cache"))?;
        std::fs::create_dir_all(root.join("config"))?;
        Ok(ShotFolders(root))
    }

    fn dirs(&self) -> Dirs {
        Dirs { cache: self.0.join("cache"), config: self.0.join("config"), downloads: None }
    }
}

impl Drop for ShotFolders {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The shot's canvas in CSS pixels and its scale factor.
pub fn canvas(options: &Options) -> ([u32; 2], f64) {
    (options.size.unwrap_or(SHOT_SIZE), options.scale.unwrap_or(1.0))
}

/// The surface of a canvas of `size` CSS pixels at `scale`: device pixels, rounded, at least 1.
pub fn surface_size(size: [u32; 2], scale: f64) -> [u32; 2] {
    size.map(|v| (f64::from(v) * scale).round().max(1.0) as u32)
}

/// Draws the shot `options` describe, on a new device.
pub fn render(options: &Options) -> Result<Shot, Error> {
    let gpu = Gpu::headless()?;
    render_on(&gpu, options)
}

/// [`render`] on `gpu`.
pub fn render_on(gpu: &Gpu, options: &Options) -> Result<Shot, Error> {
    render_on_with(gpu, options, |_| {})
}

/// [`render_on`], with `setup` given the app's state before the first frame: settings a test wants
/// other than today's defaults (the shot's own folders hold no saved ones), such as the night off
/// under a check of the land.
pub fn render_on_with(gpu: &Gpu, options: &Options, setup: impl FnOnce(&mut AppState)) -> Result<Shot, Error> {
    let rt = requests::runtime()?;
    let folders = ShotFolders::new()?;
    // the frames are drawn in a loop here: nothing needs waking
    let (events, events_rx) = mpsc::channel();
    let backend = start_backend(Some(folders.dirs()), rt.handle(), events, no_wake());
    let started_backend = backend.is_some();
    let (size, scale) = canvas(options);
    let view = options.view.unwrap_or(ViewState::INITIAL);
    let requests = Requests::new(rt.handle().clone(), no_wake());
    let mut state = AppState::new(options.startup(), backend, Some(requests), Some(events_rx), view, size.map(f64::from));
    // a shot never touches the network (plan section 8): without a --fixture folder the feeds
    // that would ask the backend have no source (A1, A2, A11)
    if options.fixture.is_none() {
        state.aircraft = Aircraft::with_feeds(None);
        state.liveries = Liveries::with_source(None);
        state.taxiways = Taxiways::with_source(None);
        // nor do the ships (VesselFinder), the signal blocks, a selected flight's stream and the
        // airport board (H1, T5, A8, U13): each shows what it shows with no source
        state.ships = Ships::with_feeds(None);
        state.blocks = Blocks::with_feed(None);
        state.flight = Flight::with_feeds(None);
        state.airport = Airport::with_feeds(None);
        // nor RainViewer's frame list and the measured volumes (W5)
        state.radar = crate::state::radar::Radar::with_feeds(None);
        // nor EUMETSAT's cloud picture (R10)
        state.clouds = rustradar_backend::clouds::Feed::none();
    }
    // --mode: the focus as a saved settings file would give it
    if let Some(mode) = options.mode {
        state.persisted.settings.mode = mode;
    }
    // --select: the flight selected at start, as a click selects it (no fly-to)
    if let Some(id) = options.select {
        state.select_flight(Some(id), SelectOptions::default(), options.now_ms.unwrap_or_else(system_now_ms));
    }
    setup(&mut state);

    let mut engine = Engine::new(gpu, FORMAT, state);
    // --ui: the shell, the panels and the controls over the map, as the window draws them
    engine.shell = options.ui;
    // the remote pictures from the --fixture folder, else none: never the network (F2)
    images::connect_offline(&engine.egui, &engine.state);
    // the shot's tiles, never the network's; the frames are drawn in a loop: nothing to wake
    let tiles = tile_fetcher::shot_tiles(options);
    match &tiles {
        Some(tiles) => log::info!("map tiles from {}", tiles.root().display()),
        None => log::warn!("no tile folder for the shot: the map's tiles are not drawn"),
    }
    let fetcher: Arc<dyn TileFetcher> = match &tiles {
        Some(tiles) => tiles.clone(),
        None => io::offline(),
    };
    engine.map.set_tiles(fetcher, None);
    let surface = surface_size(size, scale);
    let target = Offscreen::new(gpu, surface);
    let clock = Clock::fixed(options.now_ms.unwrap_or_else(system_now_ms));
    let input = |egui_time: f64| {
        let mut input = egui::RawInput {
            time: Some(egui_time),
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0] as f32, size[1] as f32))),
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(scale as f32);
        input
    };
    let poll = |gpu: &Gpu| gpu.device.poll(wgpu::PollType::wait_indefinitely()).map(|_| ()).map_err(|e| e.to_string());

    let started = Instant::now();
    let wait = options.wait.unwrap_or_else(max_wait);
    // the frames before the last are not drawn: nobody sees them
    engine.draw = false;
    let mut frames = 0;
    loop {
        engine.frame(gpu, target.view(), surface, scale, input(0.0), clock.now_ms());
        poll(gpu)?;
        frames += 1;
        if !engine.map.pending() {
            log::info!("the map settled after {} ms ({frames} frames)", started.elapsed().as_millis());
            break;
        }
        if started.elapsed() > wait {
            log::warn!(
                "still loading after {:.1} s ({}); the shot shows what had arrived",
                wait.as_secs_f64(),
                engine.map.pending_layers().join(", ")
            );
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // with the UI, a quarter of a second of egui's clock a frame for its parts to settle (the
    // sidebar's heights, the sheet's summary and easing, the controls following it: U6, U19)
    let egui_times: &[f64] = if options.ui { &[1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0] } else { &[1.0, 2.0] };
    for (i, &egui_time) in egui_times.iter().enumerate() {
        engine.draw = i + 1 == egui_times.len();
        engine.frame(gpu, target.view(), surface, scale, input(egui_time), clock.now_ms());
        poll(gpu)?;
    }
    let image = target.read(gpu)?;
    let adapter = format!("{}, map MSAA x{}", gpu.describe(), engine.map.samples());
    let missing_tiles: Vec<(String, String)> =
        tiles.map(|t| t.misses().into_iter().map(|(source, tile)| (source, format!("{}/{}/{}", tile.z, tile.x, tile.y))).collect()).unwrap_or_default();
    if !missing_tiles.is_empty() {
        let shown: Vec<String> = missing_tiles.iter().take(8).map(|(source, tile)| format!("{source} {tile}")).collect();
        log::warn!("the tile folder lacks {} tiles the map asked for, drawn as nothing: {}", missing_tiles.len(), shown.join(", "));
    }
    Ok(Shot { image, adapter, backend: started_backend, missing_tiles })
}

/// `--screenshot FILE`: draws the shot and writes it as a PNG.
pub fn save(options: &Options, path: &Path) -> Result<Shot, Error> {
    let shot = render(options)?;
    shot.image.save_png(path)?;
    Ok(shot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_canvas_and_its_surface() {
        assert_eq!(canvas(&Options::default()), ([1100, 720], 1.0));
        let o = Options { size: Some([1440, 900]), scale: Some(2.0), ..Default::default() };
        assert_eq!(canvas(&o), ([1440, 900], 2.0));
        assert_eq!(surface_size([1440, 900], 2.0), [2880, 1800]);
        assert_eq!(surface_size([1100, 720], 1.25), [1375, 900]);
        assert_eq!(surface_size([1, 1], 0.1), [1, 1]);
    }

    #[test]
    fn shot_folders_are_fresh_and_go_away() {
        let folders = ShotFolders::new().expect("folders");
        let dirs = folders.dirs();
        assert!(dirs.cache.is_dir() && dirs.config.is_dir());
        assert!(!dirs.config_file("keys.json").exists(), "no saved keys take part");
        assert_eq!(dirs.downloads, None);
        let root = folders.0.clone();
        let other = ShotFolders::new().expect("folders");
        assert_ne!(other.0, root);
        drop(folders);
        assert!(!root.exists());
    }
}
