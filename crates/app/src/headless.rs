//! `--screenshot` (plan section 8): the app's frames without a window, read back as a PNG.
//!
//! A shot runs the whole app as the window does (the backend started, the state, the scene
//! builders, the map, the egui pass) on a stand-in surface, with its clock standing still at
//! `--now` (or the time it started). It touches no folder of the user's: the backend gets fresh
//! cache and config folders, removed afterwards, so no saved keys and no cached tiles take part.
//! It draws frames until no layer waits for work (at most [`MAX_WAIT`]), then two more with egui's
//! clock a second apart (an egui area is invisible on its first frame, spike pitfall 9).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rustradar_backend::Dirs;
use rustradar_map::Gpu;
pub use rustradar_map::headless::{FORMAT, Image, MAX_WAIT, Offscreen};
use rustradar_scene::camera::ViewState;

use crate::app::{Engine, start_backend};
use crate::requests::{self, Requests, no_wake};
use crate::state::{AppState, Clock, system_now_ms};
use crate::{Options, SHOT_SIZE};

type Error = Box<dyn std::error::Error>;

/// A shot: the picture and what drew it.
#[derive(Debug, Clone)]
pub struct Shot {
    pub image: Image,
    /// Adapter, backend and the map's MSAA samples, for the log.
    pub adapter: String,
    /// Whether the backend started.
    pub backend: bool,
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
    let rt = requests::runtime()?;
    let folders = ShotFolders::new()?;
    // the frames are drawn in a loop here: nothing needs waking
    let (events, events_rx) = mpsc::channel();
    let backend = start_backend(Some(folders.dirs()), rt.handle(), events, no_wake());
    let started_backend = backend.is_some();
    let (size, scale) = canvas(options);
    let view = options.view.unwrap_or(ViewState::INITIAL);
    let requests = Requests::new(rt.handle().clone(), no_wake());
    let state = AppState::new(options.startup(), backend, Some(requests), Some(events_rx), view, size.map(f64::from));

    let mut engine = Engine::new(gpu, FORMAT, state);
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
    loop {
        engine.frame(gpu, target.view(), surface, scale, input(0.0), clock.now_ms());
        poll(gpu)?;
        if !engine.map.pending() {
            break;
        }
        if started.elapsed() > MAX_WAIT {
            log::warn!("a layer was still loading after {} s; the shot shows what had arrived", MAX_WAIT.as_secs());
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    for egui_time in [1.0, 2.0] {
        engine.frame(gpu, target.view(), surface, scale, input(egui_time), clock.now_ms());
        poll(gpu)?;
    }
    let image = target.read(gpu)?;
    let adapter = format!("{}, map MSAA x{}", gpu.describe(), engine.map.samples());
    Ok(Shot { image, adapter, backend: started_backend })
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
