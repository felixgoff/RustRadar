//! `MapRenderer`: owns the layers in draw order and records the map's passes (plan 1.4.4, 1.4.7).
//!
//! Per frame, after the app has set the camera:
//! 1. [`MapRenderer::prepare`]: this frame's [`ViewFrame`] and globals, the map target at
//!    `size × map_scale`, every layer's `prepare` (uploads, tile requests, frames), the frame upload.
//! 2. [`MapRenderer::draw`]: the map pass (MSAA ×4, cleared to `--ink`, layers in order) and the
//!    composite pass onto the surface. The app's egui pass follows with `LoadOp::Load`.
//!
//! [`MapRenderer::update`] hands over a new [`SceneInput`] at each scene refresh (plan 1.4.9);
//! [`MapRenderer::set_camera`] moves the camera at display rate in between.
//!
//! The 3D terrain (stage 9, plan 1.4.4 step 1, `layers::terrain::cover`): while the scene shows it,
//! the [`Drape::Cover`] layers are left out of the map pass and drawn into the terrain tiles' covers
//! instead ([`Drape::Part`] layers in both, their flat parts into the covers). `prepare` notes the
//! tiles each of them draws differently this frame
//! ([`layers::drawn_changes`]), hands the terrain's covers those and what they draw everywhere
//! ([`cover::cover_look`]), and records and submits the passes of the covers due (at most
//! [`cover::COVERS_PER_FRAME`]) in a command buffer of their own, so a shot, which only prepares
//! while it waits, has its covers too.
//!
//! Tiles (plan 1.4.8, item T1): the renderer owns the tile runtime, a worker pool of
//! [`jobs::default_threads`] and a [`TileFetcher`] (none at first: [`io::offline`]), which every
//! layer gets in [`Init::tiles`] and [`PrepareCx::tiles`]; the app hands over its fetcher and the
//! event loop's wake with [`MapRenderer::set_tiles`]. Each frame the layers share one
//! [`UploadBudget`] ([`MapRenderer::set_upload_budget`]).
//!
//! Errors and the rebuild path (item B9; doc 01 §9.5, plan 2.2). Once the app has called
//! [`MapRenderer::catch_errors`], the first thing that goes wrong on the device (a wgpu
//! validation, internal or out-of-memory error, or the device lost) is kept as a [`MapError`]
//! for the map error card (`app::ui::map_error`), as `reportMapError` keeps the first error
//! today (G:1252-1259), and the map stops drawing: no layer is prepared or drawn and the last
//! picture stays on the surface under the card, until Retry ([`MapRenderer::retry`]). A lost
//! device (today's `webglcontextlost`, G:3489-3492) cannot draw again: the app opens a new device
//! and calls [`MapRenderer::rebuild`], which makes everything the renderer holds on the GPU again
//! from what it keeps on the CPU (the scene, the camera, the clock, the tile runtime with its
//! fetcher, whose disk cache serves the tiles again, and the worker pool); today's Retry reloads
//! the page instead. Errors after the first are logged at debug level, as today's are dropped.
//!
//! Deviations: today's context-loss message names WebGL ("The graphics context was lost (WebGL
//! context lost)"); here it names the GPU device ([`DEVICE_LOST_MESSAGE`]). Without
//! [`MapRenderer::catch_errors`] (shots, tests) a wgpu error panics, as wgpu's default handler
//! does, so no test passes over a broken frame.

use std::sync::{Arc, Mutex, PoisonError};

use rustradar_core::geo::Basemap;
use rustradar_core::num::clamp01;
use rustradar_core::sun::{local_sun, sun_direction, sunlight};
use rustradar_scene::camera::{FLAT_ABOVE_ZOOM, ViewState};

use crate::frame::{Bindings, FrameArena, GLOBALS_GROUP, ViewFrame};
use crate::gpu::Gpu;
use crate::io::{self, TileFetcher};
use crate::jobs::{self, Pool, Wake};
use crate::layers::terrain::cover;
use crate::layers::{self, DrawCx, Drape, Init, Layer, Pass, PrepareCx, TileIndex};
use crate::pipelines::mesh::LightRig;
use crate::post::Post;
use crate::state::SHADOW_STENCIL_REFERENCE;
use crate::target::{CLEAR, Composite, MAP_FORMAT, MapTarget};
use crate::textures::RealisticTextures;
use crate::tiles::{TileRuntime, UploadBudget};

/// Everything the map draws from, as plain data (plan 1.4.7): the view, what is common to all
/// layers, and one `Input` per layer group, filled by the app's scene builders at each scene
/// refresh. `map` knows nothing about flights, trains or the UI.
#[derive(Debug, Clone)]
pub struct SceneInput {
    /// The camera the instances were placed with (the scene refresh's).
    pub view: ViewState,
    /// The canvas in CSS pixels (deck's viewport size).
    pub size: [f64; 2],
    /// Map pixels per CSS pixel ([`map_scale`]).
    pub map_scale: f64,
    pub basemap: Basemap,
    /// The local clock (ships, trains, camera, sky), Unix ms.
    pub local_now_ms: f64,
    /// The feed clock (aircraft, their shadows, night), Unix ms.
    pub feed_now_ms: f64,
    /// The scene's lights (deck's `LightingEffect`, G:417-445), which every lit layer reads:
    /// unaimed from [`SceneInput::new`]; [`SceneInput::aim_lights`] aims the key light at the sun
    /// as each render of today's app does (G:2723).
    pub lights: LightRig,
    pub sky: layers::sky::Input,
    pub earth: layers::earth::Input,
    pub terrain: layers::terrain::Input,
    pub imagery: layers::imagery::Input,
    pub ground: layers::ground::Input,
    pub hillshade: layers::hillshade::Input,
    pub clouds: layers::clouds::Input,
    pub vector_map: layers::vector_map::Input,
    pub rail: layers::rail::Input,
    pub radar: layers::radar::Input,
    pub night: layers::night::Input,
    pub buildings: layers::buildings::Input,
    pub trees: layers::trees::Input,
    pub ships: layers::ships::Input,
    pub blocks: layers::blocks::Input,
    pub trains: layers::trains::Input,
    pub labels: layers::labels::Input,
    pub route: layers::route::Input,
    pub markers: layers::markers::Input,
    pub aircraft: layers::aircraft::Input,
    /// What is drawn over the map after its pass: the tilt-shift and the rain overlay (R11).
    pub post: crate::post::Input,
}

impl SceneInput {
    /// A scene of `view` on a `size` CSS-pixel canvas at `map_scale`, with nothing on it.
    pub fn new(view: ViewState, size: [f64; 2], map_scale: f64, basemap: Basemap) -> Self {
        SceneInput {
            view,
            size,
            map_scale,
            basemap,
            local_now_ms: 0.0,
            feed_now_ms: 0.0,
            lights: scene_lights(&view, 0.0, false),
            sky: Default::default(),
            earth: Default::default(),
            terrain: Default::default(),
            imagery: Default::default(),
            ground: Default::default(),
            hillshade: Default::default(),
            clouds: Default::default(),
            vector_map: Default::default(),
            rail: Default::default(),
            radar: Default::default(),
            night: Default::default(),
            buildings: Default::default(),
            trees: Default::default(),
            ships: Default::default(),
            blocks: Default::default(),
            trains: Default::default(),
            labels: Default::default(),
            route: Default::default(),
            markers: Default::default(),
            aircraft: Default::default(),
            post: Default::default(),
        }
    }

    /// Aims the key light for this scene's view and feed clock, with buildings drawn or not
    /// ([`scene_lights`]). The scene builder calls it once the clocks are set, as each render
    /// does today (G:2723-2724).
    pub fn aim_lights(&mut self, buildings: bool) {
        self.lights = scene_lights(&self.view, self.feed_now_ms, buildings);
    }

    /// The map's size in map pixels: `size × map_scale`, rounded, at least 1.
    pub fn map_size(&self) -> [u32; 2] {
        [(self.size[0] * self.map_scale).round().max(1.0) as u32, (self.size[1] * self.map_scale).round().max(1.0) as u32]
    }
}

impl Default for SceneInput {
    /// The initial view (doc 01 §1.1) on a 1440 × 900 canvas, dark, at scale 1.
    fn default() -> Self {
        SceneInput::new(ViewState::INITIAL, [1440.0, 900.0], 1.0, Basemap::Dark)
    }
}

/// The scene's lights for `view` at the feed clock `feed_now_ms` (G:2721-2724): close in over
/// buildings the key light comes from the real sun at the view's centre, fading back to its usual
/// direction as the sun sets and as the camera leaves the flat map:
/// `aimKeyLight(toSunHere, buildings && flat ? sunlight(toSunHere[2]) · clamp01(zoom − 12) : 0)`
/// with `toSunHere = localSun(sunDirection(now), lat, lon)` (aimed even at weight 0, as today).
pub fn scene_lights(view: &ViewState, feed_now_ms: f64, buildings: bool) -> LightRig {
    let to_sun = local_sun(sun_direction(feed_now_ms), view.latitude, view.longitude);
    let flat = view.zoom > FLAT_ABOVE_ZOOM;
    let weight = if buildings && flat { sunlight(to_sun[2]) * clamp01(view.zoom - FLAT_ABOVE_ZOOM) } else { 0.0 };
    LightRig::aimed(to_sun, weight)
}

/// Map pixels per CSS pixel for a display's `scale_factor` (doc 01 §2.5): the full scale on dark
/// and satellite, at most 1.5 on realistic.
pub fn map_scale(basemap: Basemap, scale_factor: f64) -> f64 {
    match basemap {
        Basemap::Realistic => scale_factor.min(1.5),
        Basemap::Dark | Basemap::Satellite => scale_factor,
    }
}

/// The card's message when the device is lost (today's "The graphics context was lost (WebGL
/// context lost)", G:3491, with the GPU device in WebGL's place).
pub const DEVICE_LOST_MESSAGE: &str = "The graphics context was lost (GPU device lost)";

/// The first thing that stopped the map drawing (`mapError`, G:1252-1259), for the map error card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapError {
    /// What went wrong, on one line: the card's message (today's `err.message`).
    pub message: String,
    /// Everything that was said about it (today's `err.stack`), for "Copy details".
    pub details: String,
    /// The device is gone: nothing more can be drawn with it, and Retry means a new device and
    /// [`MapRenderer::rebuild`] (today's `contextLost`, after which Retry reloads the page).
    pub lost: bool,
}

impl MapError {
    /// An error of the app's own (a surface that cannot be drawn on, say).
    pub fn new(message: impl Into<String>, details: impl Into<String>) -> Self {
        MapError { message: one_line(&message.into()), details: details.into(), lost: false }
    }

    /// The device was lost (`webglcontextlost`, G:3489-3492), for `reason` and the driver's
    /// `message`.
    pub fn device_lost(reason: wgpu::DeviceLostReason, message: &str) -> Self {
        let message = message.trim();
        let details = if message.is_empty() { format!("Device lost ({reason:?})") } else { format!("Device lost ({reason:?}): {message}") };
        MapError { message: DEVICE_LOST_MESSAGE.into(), details, lost: true }
    }

    /// A wgpu error: the message is its chain of causes on one line ("In Device::create_buffer:
    /// Buffer usages … are invalid"), the details wgpu's whole report.
    pub fn from_wgpu(error: &wgpu::Error) -> Self {
        let mut causes = Vec::new();
        let mut source = std::error::Error::source(error);
        while let Some(cause) = source {
            let line = one_line(&cause.to_string());
            if !line.is_empty() {
                causes.push(line);
            }
            source = cause.source();
        }
        let text = error.to_string();
        let first_line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
        let chain = causes.join(": ");
        let (message, details) = match error {
            wgpu::Error::OutOfMemory { .. } => {
                let tree: String = causes.iter().enumerate().map(|(i, c)| format!("{}{c}\n", "  ".repeat(i + 1))).collect();
                (if chain.is_empty() { first_line } else { format!("Out of memory: {chain}") }, format!("{text}\n\nCaused by:\n{tree}"))
            }
            wgpu::Error::Validation { .. } | wgpu::Error::Internal { .. } => (if chain.is_empty() { first_line } else { chain }, text),
        };
        MapError { message, details, lost: false }
    }
}

/// `text` with every run of white space (new lines too) made one space, trimmed.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What Retry takes (`retryMap`, G:1272-1277).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    /// The error is cleared: the next frame draws again (`mapError = null; render()`).
    Redraw,
    /// The device is lost: open a new one and [`MapRenderer::rebuild`] on it (today the page
    /// reloads). The error stays until then.
    Rebuild,
}

/// The renderer's catch of the device's errors (shared with the callbacks wgpu holds).
#[derive(Clone, Default)]
struct ErrorWatch(Arc<Mutex<Watched>>);

#[derive(Default)]
struct Watched {
    /// The first error since the last retry or rebuild.
    first: Option<MapError>,
    /// Which installation counts: callbacks of an older device (the one before a rebuild) are
    /// ignored.
    device: u64,
    /// Errors are caught (`catch_errors`), so a rebuild catches them on its new device too.
    catching: bool,
    /// Called when an error is caught, so the loop draws the card.
    wake: Option<Wake>,
}

impl ErrorWatch {
    fn lock(&self) -> std::sync::MutexGuard<'_, Watched> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Catches `device`'s errors from now on; an earlier device's no longer count.
    fn install(&self, device: &wgpu::Device) {
        let id = {
            let mut watched = self.lock();
            watched.device += 1;
            watched.catching = true;
            watched.device
        };
        let watch = self.clone();
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| watch.report_from(id, MapError::from_wgpu(&error))));
        let watch = self.clone();
        device.set_device_lost_callback(move |reason, message| watch.report_from(id, MapError::device_lost(reason, &message)));
    }

    fn report_from(&self, device: u64, error: MapError) {
        if self.lock().device != device {
            log::debug!("map error on a device since replaced: {}", error.message);
            return;
        }
        self.report(error);
    }

    /// Keeps `error` unless one is kept already (`if (mapError) return`); wakes the loop.
    fn report(&self, error: MapError) {
        let wake = {
            let mut watched = self.lock();
            if watched.first.is_some() {
                log::debug!("map error after the first: {}", error.message);
                return;
            }
            log::error!("map error: {}\n{}", error.message, error.details);
            watched.first = Some(error);
            watched.wake.clone()
        };
        if let Some(wake) = wake {
            wake();
        }
    }

    fn error(&self) -> Option<MapError> {
        self.lock().first.clone()
    }

    fn stopped(&self) -> bool {
        self.lock().first.is_some()
    }
}

/// What the renderer holds on the GPU: made by [`MapRenderer::new`], made again by
/// [`MapRenderer::rebuild`].
struct GpuParts {
    bindings: Bindings,
    frames: FrameArena,
    target: MapTarget,
    composite: Composite,
    layers: Vec<Box<dyn Layer>>,
    /// The realistic map's textures (stand-ins until a layer starts them, `PrepareCx::textures`).
    textures: RealisticTextures,
}

impl GpuParts {
    fn new(gpu: &Gpu, surface_format: wgpu::TextureFormat, tiles: &TileRuntime) -> Self {
        let device = &gpu.device;
        let samples = gpu.msaa_samples(MAP_FORMAT);
        let bindings = Bindings::new(device);
        let frames = FrameArena::new(device, &bindings.frame_layout);
        let layers = layers::ordered(&Init { gpu, bindings: &bindings, color_format: MAP_FORMAT, samples, tiles });
        GpuParts {
            frames,
            // sized by the first `prepare`
            target: MapTarget::new(device, [1, 1], samples),
            composite: Composite::new(device, surface_format),
            layers,
            bindings,
            textures: RealisticTextures::new(device, &gpu.queue),
        }
    }
}

/// The map: its layers, its target and its passes.
pub struct MapRenderer {
    bindings: Bindings,
    frames: FrameArena,
    target: MapTarget,
    composite: Composite,
    layers: Vec<Box<dyn Layer>>,
    /// The realistic map's textures, which its layers share.
    textures: RealisticTextures,
    scene: SceneInput,
    /// The scene refresh's frame (what common offsets are measured from).
    scene_view: ViewFrame,
    /// The camera now, and the canvas size it is drawn at.
    camera: ViewState,
    camera_size: [f64; 2],
    /// This frame's view, made by `prepare`.
    view: ViewFrame,
    /// The local clock of the first prepared frame: `globals.viewport.w` counts seconds from it.
    epoch_ms: Option<f64>,
    /// The fetcher and the worker pool every layer's tiles (and other jobs) use.
    tiles: TileRuntime,
    /// What a frame may upload, over all layers.
    upload_budget: UploadBudget,
    /// What stopped the map drawing ([`Self::catch_errors`]).
    errors: ErrorWatch,
    /// The tilt-shift and the rain over the map (R11).
    post: Post,
}

impl MapRenderer {
    /// A renderer that composites onto surfaces of `surface_format` (prefer a non-sRGB one: the
    /// map's colours are gamma-space values).
    pub fn new(gpu: &Gpu, surface_format: wgpu::TextureFormat) -> Self {
        let scene = SceneInput::default();
        let tiles = TileRuntime::new(io::offline(), Arc::new(Pool::new(jobs::default_threads())));
        let GpuParts { bindings, frames, target, composite, layers, textures } = GpuParts::new(gpu, surface_format, &tiles);
        let scene_view = ViewFrame::new(&scene.view, scene.size, scene.map_scale, 0.0);
        MapRenderer {
            frames,
            target,
            composite,
            layers,
            textures,
            camera: scene.view,
            camera_size: scene.size,
            view: scene_view.clone(),
            scene_view,
            scene,
            bindings,
            epoch_ms: None,
            tiles,
            upload_budget: UploadBudget::per_frame(),
            errors: ErrorWatch::default(),
            post: Post::new(&gpu.device, surface_format),
        }
    }

    /// From now on the first error on `gpu`'s device (validation, internal, out of memory, the
    /// device lost) is kept for the map error card instead of panicking, and the map stops
    /// drawing until [`Self::retry`] or [`Self::rebuild`]; `wake` (the event loop's) is called
    /// when it is caught. The window calls it once; shots and tests do not, so an error fails them.
    pub fn catch_errors(&mut self, gpu: &Gpu, wake: Option<Wake>) {
        self.errors.lock().wake = wake;
        self.errors.install(&gpu.device);
    }

    /// Stops the map with an error found outside the device's own reports (the window's surface,
    /// say), unless one is kept already.
    pub fn report_error(&self, error: MapError) {
        self.errors.report(error);
    }

    /// The first error since the last retry or rebuild: while there is one the map is stopped
    /// and the card shows it.
    pub fn error(&self) -> Option<MapError> {
        self.errors.error()
    }

    /// The card's Retry (`retryMap`, G:1272-1277): an error is cleared and the next frame draws
    /// again ([`Retry::Redraw`]), unless the device is lost: then nothing changes until the app has
    /// a new device for [`Self::rebuild`] ([`Retry::Rebuild`]).
    pub fn retry(&mut self) -> Retry {
        let mut watched = self.errors.lock();
        if watched.first.as_ref().is_some_and(|e| e.lost) {
            return Retry::Rebuild;
        }
        watched.first = None;
        Retry::Redraw
    }

    /// Makes everything this renderer holds on the GPU again on `gpu` (plan 2.2: after the device
    /// was lost, a new one; or the same), compositing onto surfaces of `surface_format`.
    ///
    /// Kept, from the CPU: the scene and the camera, the clock's epoch, the upload budget, and the
    /// tile runtime (the app's fetcher, whose disk cache serves the tiles again without the
    /// network, the worker pool and its wake). Made again: the bindings, the frame arena, the map
    /// target, the composite pass and every layer of [`layers::ordered`], which load their tiles
    /// and embedded textures as on a first start. Layers added with [`Self::add_layer`] are
    /// dropped (they belong to the old device). The error is cleared, and if errors were caught
    /// they are caught on `gpu` from now on (the old device's no longer count).
    pub fn rebuild(&mut self, gpu: &Gpu, surface_format: wgpu::TextureFormat) {
        // the old device's resources go before the new ones are made
        self.layers.clear();
        let catching = {
            let mut watched = self.errors.lock();
            watched.first = None;
            watched.catching
        };
        // caught from the start: an error while the layers are made is the card's (plan 2.2: the
        // rebuild failed)
        if catching {
            self.errors.install(&gpu.device);
        }
        let GpuParts { bindings, frames, target, composite, layers, textures } = GpuParts::new(gpu, surface_format, &self.tiles);
        self.textures = textures;
        self.bindings = bindings;
        self.frames = frames;
        self.target = target;
        self.composite = composite;
        self.layers = layers;
        self.post = Post::new(&gpu.device, surface_format);
        log::info!("map rebuilt on {}", gpu.describe());
    }

    /// Where tiles come from from now on: the app's `fetcher` (on `backend::tiles`, or a folder of
    /// fixtures), and the `wake` the worker pool calls after each job (the event loop's; `None`
    /// for a loop that polls, as the headless path's). Call it before the first frame; a later
    /// call makes every layer's tiles start over.
    pub fn set_tiles(&mut self, fetcher: Arc<dyn TileFetcher>, wake: Option<Wake>) {
        self.tiles.set_fetcher(fetcher);
        self.tiles.pool.set_wake(wake);
    }

    /// The tile runtime: the fetcher and the worker pool the layers use.
    pub fn tiles(&self) -> &TileRuntime {
        &self.tiles
    }

    /// What each frame may upload over all layers: [`UploadBudget::per_frame`] unless set
    /// ([`UploadBudget::unlimited`] where nobody watches the frames, as a shot).
    pub fn set_upload_budget(&mut self, budget: UploadBudget) {
        self.upload_budget = budget;
    }

    /// A scene refresh: the input every layer draws from until the next one. The camera jumps to
    /// the scene's view.
    pub fn update(&mut self, scene: SceneInput) {
        self.scene_view = ViewFrame::new(&scene.view, scene.size, scene.map_scale, 0.0);
        self.camera = scene.view;
        self.camera_size = scene.size;
        self.scene = scene;
    }

    /// The camera for the next frames (display rate, between scene refreshes), on a canvas of
    /// `size` CSS pixels.
    pub fn set_camera(&mut self, view: ViewState, size: [f64; 2]) {
        self.camera = view;
        self.camera_size = size;
    }

    /// Whether the camera has left the scene's projection (zoom 12 crossed) or canvas size since
    /// the last [`Self::update`]: the app should refresh the scene before the next frame, as the
    /// instances' common offsets do not carry across.
    pub fn needs_refresh(&self) -> bool {
        self.camera.is_flat() != self.scene.view.is_flat() || self.camera_size != self.scene.size
    }

    /// The scene of the last [`Self::update`].
    pub fn scene(&self) -> &SceneInput {
        &self.scene
    }

    /// This frame's view (after [`Self::prepare`]).
    pub fn view(&self) -> &ViewFrame {
        &self.view
    }

    /// The map target (map pixels, MSAA, the resolved picture).
    pub fn target(&self) -> &MapTarget {
        &self.target
    }

    /// MSAA samples of the map pass.
    pub fn samples(&self) -> u32 {
        self.target.samples()
    }

    /// What a layer is made with, for a layer made outside [`layers::ordered`].
    pub fn init<'a>(&'a self, gpu: &'a Gpu) -> Init<'a> {
        Init { gpu, bindings: &self.bindings, color_format: MAP_FORMAT, samples: self.target.samples(), tiles: &self.tiles }
    }

    /// The 3D terrain's covers while it is drawn (stage 9, `layers::terrain::cover`): their
    /// bookkeeping and memory, for tests and logs.
    pub fn terrain_covers(&mut self) -> Option<&mut cover::Covers> {
        self.layers.iter_mut().find_map(|l| l.covers())
    }

    /// Adds a layer after all the others (tests, debug overlays).
    pub fn add_layer(&mut self, layer: Box<dyn Layer>) {
        self.layers.push(layer);
    }

    /// The layers' names in draw order.
    pub fn layer_names(&self) -> Vec<&'static str> {
        self.layers.iter().map(|l| l.name()).collect()
    }

    /// Whether a layer still waits for work that will change the picture (never while the map is
    /// stopped by an error).
    pub fn pending(&self) -> bool {
        !self.errors.stopped() && self.layers.iter().any(|l| l.pending())
    }

    /// The layers that still wait, in draw order (for a log).
    pub fn pending_layers(&self) -> Vec<&'static str> {
        if self.errors.stopped() {
            return Vec::new();
        }
        self.layers.iter().filter(|l| l.pending()).map(|l| l.name()).collect()
    }

    /// Main thread, before encoding: the frame's view and globals, the layers' uploads and
    /// frames. `local_now_ms` is the local clock. Nothing while the map is stopped by an error.
    pub fn prepare(&mut self, gpu: &Gpu, local_now_ms: f64) {
        if self.errors.stopped() {
            return;
        }
        let epoch = *self.epoch_ms.get_or_insert(local_now_ms);
        let time_s = ((local_now_ms - epoch) / 1000.0) as f32;
        let scale = self.scene.map_scale;
        let size = self.camera_size;
        self.view = ViewFrame::following(&self.camera, size, scale, time_s, &self.scene_view);
        let map_size = [(size[0] * scale).round().max(1.0) as u32, (size[1] * scale).round().max(1.0) as u32];
        self.target.ensure_size(&gpu.device, map_size);
        self.frames.clear();
        // (a fresh copy each frame: nothing is taken from the stored one)
        let mut budget = self.upload_budget;
        let mut cx = PrepareCx {
            gpu,
            scene: &self.scene,
            view: &self.view,
            frames: &mut self.frames,
            local_now_ms,
            tiles: &self.tiles,
            budget: &mut budget,
            textures: &mut self.textures,
        };
        // while the 3D terrain is drawn, what the cover layers draw differently puts its covers out
        // of date
        let draping = self.scene.terrain.shown;
        let mut changed = Vec::new();
        for layer in &mut self.layers {
            let cover = draping && layer.drape().into_covers();
            if cover {
                layers::listen_for_drawn_changes();
            }
            layer.prepare(&mut cx);
            if cover {
                changed.extend(layers::drawn_changes());
            }
        }
        self.frames.upload(&gpu.device, &gpu.queue, &self.bindings.frame_layout);
        self.bindings.write_globals(&gpu.queue, &self.view.globals);
        self.post.prepare(&self.scene, &self.view, local_now_ms);
        self.draw_covers(gpu, &changed);
    }

    /// The terrain's covers due this frame (stage 9): out of date where `changed` tiles of the cover
    /// layers lie and everywhere when what they draw everywhere changed, then at most
    /// [`cover::COVERS_PER_FRAME`] drawn (the cover layers in order, each pass with the cover's
    /// globals and the map pass's stencil reference), submitted now.
    fn draw_covers(&mut self, gpu: &Gpu, changed: &[TileIndex]) {
        let look = cover::cover_look(&self.scene, &self.view, !self.textures.ready);
        let Some(covers) = self.layers.iter_mut().find_map(|l| l.covers()) else { return };
        covers.set_look(look);
        covers.invalidate(changed);
        let jobs = covers.jobs(gpu, &self.view, cover::COVERS_PER_FRAME);
        if jobs.is_empty() {
            return;
        }
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("terrain covers") });
        let (scene, view, frames, layers) = (&self.scene, &self.view, &self.frames, &self.layers);
        for job in &jobs {
            job.encode(&mut encoder, &mut |pass, globals| {
                pass.set_stencil_reference(SHADOW_STENCIL_REFERENCE);
                pass.set_bind_group(GLOBALS_GROUP, globals, &[]);
                frames.bind(pass, FrameArena::DEFAULT);
                let cx = DrawCx { scene, view, frames, globals, pass: Pass::Cover };
                for layer in layers.iter().filter(|l| l.drape().into_covers()) {
                    layer.draw(pass, &cx);
                }
            });
        }
        gpu.queue.submit([encoder.finish()]);
    }

    /// Whether the picture moves by itself (the rain falls, R11): the window should draw every
    /// frame while it does.
    pub fn animating(&self) -> bool {
        !self.errors.stopped() && self.post.animating()
    }

    /// Records the map pass and the composite pass onto `surface` (`surface_size` pixels). While
    /// the map is stopped by an error only the composite of the last picture is recorded (the card
    /// goes over it), and nothing once the device is lost.
    pub fn draw(&mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, surface: &wgpu::TextureView, surface_size: [u32; 2]) {
        match self.errors.error() {
            Some(error) if error.lost => return,
            Some(_) => {
                self.composite.draw(&gpu.device, &gpu.queue, encoder, &self.target, surface, surface_size);
                return;
            }
            None => {}
        }
        {
            let mut pass = self.target.begin_pass(encoder, CLEAR);
            pass.set_stencil_reference(SHADOW_STENCIL_REFERENCE);
            pass.set_bind_group(GLOBALS_GROUP, self.bindings.globals_group(), &[]);
            self.frames.bind(&mut pass, FrameArena::DEFAULT);
            let cx = DrawCx {
                scene: &self.scene,
                view: &self.view,
                frames: &self.frames,
                globals: self.bindings.globals_group(),
                pass: Pass::Map,
            };
            // (while the 3D terrain is drawn the cover layers are drawn into its covers instead)
            let draping = self.scene.terrain.shown;
            for layer in self.layers.iter().filter(|l| !(draping && l.drape() == Drape::Cover)) {
                layer.draw(&mut pass, &cx);
            }
        }
        // the composite, through the tilt-shift when it is on, and the rain over it (R11)
        self.post.draw(gpu, encoder, &mut self.composite, &self.target, surface, surface_size);
    }

    /// [`Self::prepare`] and [`Self::draw`] in a command buffer of their own, submitted.
    pub fn render(&mut self, gpu: &Gpu, surface: &wgpu::TextureView, surface_size: [u32; 2], local_now_ms: f64) {
        self.prepare(gpu, local_now_ms);
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("map") });
        self.draw(gpu, &mut encoder, surface, surface_size);
        gpu.queue.submit([encoder.finish()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::testing::gpu;
    use crate::pipelines::mesh::key_light_unit;

    /// 2026-06-21 10:00 UTC: noon in Stockholm at midsummer (the reference pictures' clock).
    const MIDSUMMER_NOON: f64 = 1_782_036_000_000.0;
    /// 2026-12-21 22:00 UTC: night in Stockholm.
    const MIDWINTER_NIGHT: f64 = 1_797_890_400_000.0;

    fn stockholm(zoom: f64) -> ViewState {
        ViewState { longitude: 18.0686, latitude: 59.3293, zoom, bearing: 20.0, pitch: 60.0, position: None }
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-12)
    }

    /// G:2723-2724: the key light follows the sun only over buildings on the flat map, by
    /// `sunlight(up) · clamp01(zoom − 12)`, else it shines from its usual direction.
    #[test]
    fn the_scene_aims_its_key_light_at_the_sun_only_over_buildings_close_in() {
        let to_sun = local_sun(sun_direction(MIDSUMMER_NOON), 59.3293, 18.0686);
        assert!(sunlight(to_sun[2]) == 1.0, "the sun is high at noon: {to_sun:?}");
        let full = scene_lights(&stockholm(15.5), MIDSUMMER_NOON, true);
        assert_eq!(full, LightRig::aimed(to_sun, 1.0));
        // from the sun's bearing: the light shines away from it (north-ish at noon), not straight down
        assert!(full.key_direction[1] > 0.5 && full.key_direction[2] < 0.0, "{:?}", full.key_direction);
        assert!(!close(full.key_direction, key_light_unit()));
        // half way in over zoom 12-13
        assert_eq!(scene_lights(&stockholm(12.5), MIDSUMMER_NOON, true), LightRig::aimed(to_sun, 0.5));
        // the usual direction: buildings off, the globe, or night
        for (view, now, buildings) in [
            (stockholm(15.5), MIDSUMMER_NOON, false),
            (stockholm(12.0), MIDSUMMER_NOON, true),
            (stockholm(15.5), MIDWINTER_NIGHT, true),
        ] {
            let rig = scene_lights(&view, now, buildings);
            assert!(close(rig.key_direction, key_light_unit()), "{view:?} {now} {buildings}: {:?}", rig.key_direction);
            assert_eq!((rig.ambient, rig.camera, rig.key), (LightRig::new().ambient, LightRig::new().camera, LightRig::new().key));
        }
        // a new scene starts unaimed; aim_lights uses its own view and feed clock
        let mut scene = SceneInput::new(stockholm(15.5), [1100.0, 720.0], 1.0, Basemap::Dark);
        assert!(close(scene.lights.key_direction, key_light_unit()));
        scene.feed_now_ms = MIDSUMMER_NOON;
        scene.aim_lights(true);
        assert_eq!(scene.lights, full);
        scene.aim_lights(false);
        assert!(close(scene.lights.key_direction, key_light_unit()));
    }

    /// A device of the test's own, which it may break or destroy, opened while the shared one is
    /// held so that no other test uses SwiftShader meanwhile.
    fn own_device(_shared: &Gpu) -> Gpu {
        Gpu::headless().expect("a second device on the adapter the shared one is on")
    }

    /// A validation error on `gpu`'s device (a buffer that would be mapped for reading and used as
    /// vertices), reported to its uncaptured-error handler.
    fn provoke_validation_error(gpu: &Gpu, label: &str) {
        let _ = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
    }

    fn shots_dir() -> std::path::PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    fn differing_pixels(a: &crate::headless::Image, b: &crate::headless::Image) -> usize {
        assert_eq!((a.width, a.height), (b.width, b.height));
        a.rgba.chunks_exact(4).zip(b.rgba.chunks_exact(4)).filter(|(a, b)| a != b).count()
    }

    /// B9's acceptance (plan 2.2): the device is lost under a map drawn from tiles (the dark map at
    /// V3 with its labels and the night, from the fixtures), the renderer is rebuilt on a new
    /// device from what it keeps on the CPU (the scene, the camera, the tile runtime and its
    /// fetcher), and draws the same picture as before, pixel for pixel.
    #[test]
    fn a_renderer_rebuilt_after_its_device_is_lost_draws_the_same_shot() {
        use crate::headless::{FORMAT, render_with};
        use crate::io::DirTiles;
        use crate::layers::{earth, labels, night, vector_map};

        let Some(shared) = gpu() else { return };
        let v3 = ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, bearing: 0.0, pitch: 0.0, position: None };
        let mut scene = SceneInput::new(v3, [1100.0, 720.0], 1.0, Basemap::Dark);
        scene.local_now_ms = MIDWINTER_NIGHT;
        scene.feed_now_ms = MIDWINTER_NIGHT;
        scene.earth = earth::Input::SHOWN;
        scene.vector_map = vector_map::Input::SHOWN;
        scene.labels = labels::Input::shown();
        scene.night = night::Input::SHOWN;
        scene.aim_lights(false);

        let first = own_device(&shared);
        let tiles = Arc::new(DirTiles::fixtures());
        let mut renderer = MapRenderer::new(&first, FORMAT);
        renderer.set_tiles(tiles.clone(), None);
        renderer.catch_errors(&first, None);
        let before = render_with(&first, &mut renderer, &scene, 1.0).expect("rendered");
        assert!(!renderer.pending(), "still loading: {:?}", renderer.pending_layers());
        assert!(tiles.misses().is_empty(), "the fixtures lack {:?}", tiles.misses());
        assert_eq!(renderer.error(), None);
        before.save_png(&shots_dir().join("b9-before-the-loss.png")).expect("png");
        // tiles, labels and the night are on it: the sea, the land, the night over both, text
        let colours: std::collections::HashSet<[u8; 4]> = before.rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect();
        assert!(colours.len() > 50, "a picture of the map, not a flat one: {} colours", colours.len());

        // the device goes (a driver reset, the GPU removed: here destroyed), and wgpu says so at
        // its next poll
        first.device.destroy();
        let _ = first.device.poll(wgpu::PollType::wait_indefinitely());
        let lost = renderer.error().expect("the loss is caught");
        assert!(lost.lost);
        assert_eq!(lost.message, DEVICE_LOST_MESSAGE);
        assert_eq!(lost.details, "Device lost (Destroyed)");
        assert!(!renderer.pending(), "a stopped map waits for nothing");
        assert_eq!(renderer.retry(), Retry::Rebuild, "Retry cannot draw on a lost device");
        assert_eq!(renderer.error(), Some(lost));

        // a new device, and everything made again on it
        let second = own_device(&shared);
        renderer.rebuild(&second, FORMAT);
        assert_eq!(renderer.error(), None);
        assert_eq!(renderer.scene().view, v3, "the scene is kept");
        assert!(Arc::ptr_eq(&renderer.tiles().fetcher(), &(tiles.clone() as Arc<dyn TileFetcher>)), "the fetcher is kept");
        let after = render_with(&second, &mut renderer, &scene, 1.0).expect("rendered after the rebuild");
        after.save_png(&shots_dir().join("b9-after-the-rebuild.png")).expect("png");
        assert!(!renderer.pending());
        assert_eq!(renderer.error(), None, "nothing went wrong on the new device");
        assert_eq!(differing_pixels(&before, &after), 0, "the rebuilt map draws the same shot");
    }

    /// `reportMapError` and `retryMap` (G:1252-1277): the first error is kept, a later one does not
    /// replace it, the map stops (the last picture stays and nothing new is drawn) until Retry;
    /// errors are caught on the new device after a rebuild, and the old device's no longer count.
    #[test]
    fn the_first_error_stops_the_map_until_retry() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use crate::headless::{FORMAT, Offscreen, render_with};
        use crate::layers::earth;

        let Some(shared) = gpu() else { return };
        let gpu = own_device(&shared);
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        let wakes = Arc::new(AtomicUsize::new(0));
        let counted = wakes.clone();
        renderer.catch_errors(
            &gpu,
            Some(Arc::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            })),
        );
        let scene = |basemap: Basemap| {
            let mut scene = SceneInput::new(ViewState::INITIAL, [320.0, 200.0], 1.0, basemap);
            scene.earth = earth::Input::SHOWN;
            scene
        };
        let v1 = scene(Basemap::Dark);
        // the satellite earth's blue where the dark map's land is grey
        let v1_satellite = scene(Basemap::Satellite);
        let first = render_with(&gpu, &mut renderer, &v1, 1.0).expect("rendered");
        assert_eq!(renderer.error(), None);
        assert_eq!(wakes.load(Ordering::SeqCst), 0);

        provoke_validation_error(&gpu, "first");
        let error = renderer.error().expect("caught");
        assert!(!error.lost);
        assert!(error.message.starts_with("In Device::create_buffer, label = 'first': "), "{}", error.message);
        assert!(!error.message.contains('\n'));
        assert!(error.details.starts_with("Validation Error"), "{}", error.details);
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "the loop is woken to show the card");
        provoke_validation_error(&gpu, "second");
        assert_eq!(renderer.error(), Some(error.clone()), "the first error is kept");
        assert_eq!(wakes.load(Ordering::SeqCst), 1);

        // stopped: another scene draws nothing new; the last picture is composited under the card
        assert!(!renderer.pending());
        let stopped = render_with(&gpu, &mut renderer, &v1_satellite, 1.0).expect("rendered");
        assert_eq!(differing_pixels(&first, &stopped), 0, "the map is stopped at its last picture");

        // Retry: drawing again
        assert_eq!(renderer.retry(), Retry::Redraw);
        assert_eq!(renderer.error(), None);
        let satellite = render_with(&gpu, &mut renderer, &v1_satellite, 1.0).expect("rendered");
        assert!(differing_pixels(&first, &satellite) > 10_000, "the satellite earth drawn after Retry");

        // the app's own report stops it too
        renderer.report_error(MapError::new("The window's surface\ncannot be drawn on", "Surface lost"));
        assert_eq!(renderer.error().map(|e| e.message), Some("The window's surface cannot be drawn on".to_string()));
        assert_eq!(renderer.retry(), Retry::Redraw);

        // rebuilt on another device: its errors are caught, the old device's are not
        let other = own_device(&shared);
        renderer.rebuild(&other, FORMAT);
        provoke_validation_error(&gpu, "old device");
        assert_eq!(renderer.error(), None, "the old device's errors no longer count");
        gpu.device.destroy();
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        assert_eq!(renderer.error(), None, "nor its loss");
        let rebuilt = render_with(&other, &mut renderer, &v1, 1.0).expect("rendered");
        assert_eq!(differing_pixels(&first, &rebuilt), 0);
        provoke_validation_error(&other, "new device");
        assert!(renderer.error().expect("caught on the new device").message.contains("'new device'"));
        let off = Offscreen::new(&other, [320, 200]);
        renderer.render(&other, off.view(), off.size(), 0.0);
        assert_eq!(differing_pixels(&first, &off.read(&other).expect("read")), 0, "stopped again");
    }

    /// The card's text for each kind of error (doc 01 §9.5): one line for the message, wgpu's whole
    /// report for the details.
    #[test]
    fn map_errors_read_as_the_card_shows_them() {
        #[derive(Debug)]
        struct Cause(&'static str, Option<Box<Cause>>);
        impl std::fmt::Display for Cause {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.0)
            }
        }
        impl std::error::Error for Cause {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.1.as_deref().map(|c| c as &(dyn std::error::Error + 'static))
            }
        }
        let chain = || Box::new(Cause("In Device::create_render_pipeline, label = 'path'", Some(Box::new(Cause("Error matching\n  the vertex shader", None)))));
        let description = "Validation Error\n\nCaused by:\n  In Device::create_render_pipeline, label = 'path'\n    Error matching\n  the vertex shader\n";
        let validation = MapError::from_wgpu(&wgpu::Error::Validation { source: chain(), description: description.into() });
        assert_eq!(validation.message, "In Device::create_render_pipeline, label = 'path': Error matching the vertex shader");
        assert_eq!(validation.details, description);
        assert!(!validation.lost);
        let memory = MapError::from_wgpu(&wgpu::Error::OutOfMemory { source: Box::new(Cause("In Device::create_buffer", Some(Box::new(Cause("Not enough memory left.", None))))) });
        assert_eq!(memory.message, "Out of memory: In Device::create_buffer: Not enough memory left.");
        assert_eq!(memory.details, "Out of Memory\n\nCaused by:\n  In Device::create_buffer\n    Not enough memory left.\n");
        let internal = MapError::from_wgpu(&wgpu::Error::Internal { source: Box::new(Cause("", None)), description: "\n  Internal error\nmore".into() });
        assert_eq!(internal.message, "Internal error", "no cause: the report's first line");
        let lost = MapError::device_lost(wgpu::DeviceLostReason::Unknown, " VK_ERROR_DEVICE_LOST \n");
        assert_eq!(lost, MapError { message: DEVICE_LOST_MESSAGE.into(), details: "Device lost (Unknown): VK_ERROR_DEVICE_LOST".into(), lost: true });
        assert_eq!(DEVICE_LOST_MESSAGE, "The graphics context was lost (GPU device lost)");
        assert_eq!(MapError::new(" a\n b ", "d"), MapError { message: "a b".into(), details: "d".into(), lost: false });
    }

    #[test]
    fn map_scale_caps_realistic_at_one_and_a_half() {
        assert_eq!(map_scale(Basemap::Dark, 2.0), 2.0);
        assert_eq!(map_scale(Basemap::Satellite, 3.0), 3.0);
        assert_eq!(map_scale(Basemap::Realistic, 2.0), 1.5);
        assert_eq!(map_scale(Basemap::Realistic, 1.25), 1.25);
        let scene = SceneInput::new(ViewState::INITIAL, [1100.0, 720.0], 1.5, Basemap::Realistic);
        assert_eq!(scene.map_size(), [1650, 1080]);
        assert_eq!(SceneInput::new(ViewState::INITIAL, [0.0, 0.0], 1.0, Basemap::Dark).map_size(), [1, 1]);
    }
}
