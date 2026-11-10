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
//! Tiles (plan 1.4.8, item T1): the renderer owns the tile runtime, a worker pool of
//! [`jobs::default_threads`] and a [`TileFetcher`] (none at first: [`io::offline`]), which every
//! layer gets in [`Init::tiles`] and [`PrepareCx::tiles`]; the app hands over its fetcher and the
//! event loop's wake with [`MapRenderer::set_tiles`]. Each frame the layers share one
//! [`UploadBudget`] ([`MapRenderer::set_upload_budget`]).

use std::sync::Arc;

use rustradar_core::geo::Basemap;
use rustradar_core::num::clamp01;
use rustradar_core::sun::{local_sun, sun_direction, sunlight};
use rustradar_scene::camera::{FLAT_ABOVE_ZOOM, ViewState};

use crate::frame::{Bindings, FrameArena, GLOBALS_GROUP, ViewFrame};
use crate::gpu::Gpu;
use crate::io::{self, TileFetcher};
use crate::jobs::{self, Pool, Wake};
use crate::layers::{self, DrawCx, Init, Layer, Pass, PrepareCx};
use crate::pipelines::mesh::LightRig;
use crate::state::SHADOW_STENCIL_REFERENCE;
use crate::target::{CLEAR, Composite, MAP_FORMAT, MapTarget};
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

/// The map: its layers, its target and its passes.
pub struct MapRenderer {
    bindings: Bindings,
    frames: FrameArena,
    target: MapTarget,
    composite: Composite,
    layers: Vec<Box<dyn Layer>>,
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
}

impl MapRenderer {
    /// A renderer that composites onto surfaces of `surface_format` (prefer a non-sRGB one: the
    /// map's colours are gamma-space values).
    pub fn new(gpu: &Gpu, surface_format: wgpu::TextureFormat) -> Self {
        let device = &gpu.device;
        let samples = gpu.msaa_samples(MAP_FORMAT);
        let bindings = Bindings::new(device);
        let frames = FrameArena::new(device, &bindings.frame_layout);
        let scene = SceneInput::default();
        let tiles = TileRuntime::new(io::offline(), Arc::new(Pool::new(jobs::default_threads())));
        let layers = layers::ordered(&Init { gpu, bindings: &bindings, color_format: MAP_FORMAT, samples, tiles: &tiles });
        let scene_view = ViewFrame::new(&scene.view, scene.size, scene.map_scale, 0.0);
        MapRenderer {
            frames,
            // sized by the first `prepare`
            target: MapTarget::new(device, [1, 1], samples),
            composite: Composite::new(device, surface_format),
            layers,
            camera: scene.view,
            camera_size: scene.size,
            view: scene_view.clone(),
            scene_view,
            scene,
            bindings,
            epoch_ms: None,
            tiles,
            upload_budget: UploadBudget::per_frame(),
        }
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

    /// Adds a layer after all the others (tests, debug overlays).
    pub fn add_layer(&mut self, layer: Box<dyn Layer>) {
        self.layers.push(layer);
    }

    /// The layers' names in draw order.
    pub fn layer_names(&self) -> Vec<&'static str> {
        self.layers.iter().map(|l| l.name()).collect()
    }

    /// Whether a layer still waits for work that will change the picture.
    pub fn pending(&self) -> bool {
        self.layers.iter().any(|l| l.pending())
    }

    /// The layers that still wait, in draw order (for a log).
    pub fn pending_layers(&self) -> Vec<&'static str> {
        self.layers.iter().filter(|l| l.pending()).map(|l| l.name()).collect()
    }

    /// Main thread, before encoding: the frame's view and globals, the layers' uploads and
    /// frames. `local_now_ms` is the local clock.
    pub fn prepare(&mut self, gpu: &Gpu, local_now_ms: f64) {
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
        };
        for layer in &mut self.layers {
            layer.prepare(&mut cx);
        }
        self.frames.upload(&gpu.device, &gpu.queue, &self.bindings.frame_layout);
        self.bindings.write_globals(&gpu.queue, &self.view.globals);
    }

    /// Records the map pass and the composite pass onto `surface` (`surface_size` pixels).
    pub fn draw(&mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, surface: &wgpu::TextureView, surface_size: [u32; 2]) {
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
            for layer in &self.layers {
                layer.draw(&mut pass, &cx);
            }
        }
        self.composite.draw(&gpu.device, &gpu.queue, encoder, &self.target, surface, surface_size);
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
