//! The window and the frame loop (plan 2.1–2.2; doc 07 §11.4, §11.7).
//!
//! [`run`] makes the tokio runtime, the event loop and the wake, starts the backend, and runs the
//! winit loop. [`Engine`] draws one frame, the same in the window and in the headless path:
//!
//! ```text
//! 1 clocks   local now; the feed clock from the motion model
//! 2 poll     the backend's events, every part's answers and due requests (state::AppState::poll)
//! 3 camera   the running transition, the follow lock (state::AppState::advance_camera)
//! 4 scene    when due (scene_builder::Pacer) or forced: scene_builder → SceneInput → MapRenderer
//! 5 ui       the egui pass, its actions applied
//! 6 render   the map's passes and the composite onto the surface, egui on top; submit
//! 7 schedule draw again at once while a transition, a gesture or an egui animation runs, else
//!            by the next scene refresh, egui's repaint delay or a state deadline
//! ```

use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use rustradar_backend::{Backend, BackendEvent, Dirs, Wake};
use rustradar_map::renderer::Retry;
use rustradar_map::{Gpu, MapRenderer};
use rustradar_scene::camera::ViewState;
use tokio::runtime::Handle;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopBuilder};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::input::{self, KeyGate, MapInput};
use crate::requests::{self, Requests, WakeLatch};
use crate::scene_builder::{Pacer, SceneBuilder};
use crate::state::{Action, AppState, Clock};
use crate::ui::{attribution, dev_panel, export_menu, hover_card, keys_dialog, map_controls, map_error, modebar, phone, shell, shortcuts, speed_legend, theme, toast, tooltip};
use crate::{Options, images, platform, tile_fetcher};

type Error = Box<dyn std::error::Error>;

/// What reaches the event loop from other threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserEvent {
    /// A request finished, a stream delivered, the backend has an event: draw a frame and poll.
    Wake,
}

/// The window's size before it is maximised (doc 07 §5.2, §11.7), CSS pixels.
pub const WINDOW_SIZE: [f64; 2] = [1440.0, 900.0];
/// The smallest the window may be made.
pub const WINDOW_MIN_SIZE: [f64; 2] = [900.0, 600.0];

/// The main window as the Tauri build opened it (doc 07 §11.7): titled `RustRadar`, 1440 × 900,
/// at least 900 × 600, maximised, and hidden until its first frame is drawn (no flash of an empty
/// window). `--size` sets the inner size instead of maximising.
pub fn window_attributes(options: &Options) -> WindowAttributes {
    let size = options.size.map_or(WINDOW_SIZE, |[w, h]| [f64::from(w), f64::from(h)]);
    Window::default_attributes()
        .with_title("RustRadar")
        .with_inner_size(LogicalSize::new(size[0], size[1]))
        .with_min_inner_size(LogicalSize::new(WINDOW_MIN_SIZE[0], WINDOW_MIN_SIZE[1]))
        .with_maximized(options.size.is_none())
        .with_visible(false)
}

/// Starts the backend (`Backend::start`, doc 07 §10.1) in `dirs`, or explains in the log why the
/// app goes on without it (no folders, no HTTP client): the map still runs, without feeds.
pub fn start_backend(dirs: Option<Dirs>, rt: &Handle, events: Sender<BackendEvent>, wake: Wake) -> Option<Backend> {
    let Some(dirs) = dirs else {
        log::error!("this system names no cache or config folder: running without the backend");
        return None;
    };
    match Backend::start(dirs, rt.clone(), events, wake) {
        Ok(backend) => Some(backend),
        Err(e) => {
            log::error!("the backend did not start, running without it: {e}");
            None
        }
    }
}

/// What a frame leaves for the scheduler (plan 2.2 step 7).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Next {
    /// Draw again at once: a camera transition, a gesture or an egui animation runs.
    pub now: bool,
    /// Else draw again by this local time (Unix ms; infinite when nothing is due).
    pub at_ms: f64,
}

/// Everything that draws a frame: the state, the scene builders, the map, egui.
pub struct Engine {
    pub state: AppState,
    pub scene: SceneBuilder,
    pub pacer: Pacer,
    pub map: MapRenderer,
    pub input: MapInput,
    pub egui: egui::Context,
    egui_renderer: egui_wgpu::Renderer,
    /// The scale factor and basemap the last scene was built for.
    built_for: Option<(f64, rustradar_core::geo::Basemap)>,
    /// The error card's Retry asked for a new device after a loss ([`Self::take_rebuild`], B9).
    rebuild: bool,
    /// A rebuild was already asked for the loss the map has now: a second loss in a row waits
    /// for Retry with the card up (plan 2.2: the card shows only if the rebuilt map fails too).
    rebuilt_for_loss: bool,
    /// Whether [`Self::frame`] draws and submits its picture (the default). The headless path
    /// turns it off for the frames whose picture nobody reads while the map's work comes in (P1):
    /// such a frame still polls, builds the scene, runs egui (its textures kept) and prepares the
    /// map, so the layers take their tiles in as a drawn frame does, without the map pass that
    /// costs seconds on a software rasteriser.
    pub draw: bool,
    /// Whether the UI pass draws the shell and takes the page's keys (`ui::shell`, U6). The window
    /// turns it on; it is off by default, so the headless shots and the tests of the map draw the
    /// map alone, as they were written to (the error card and the map's tooltip are drawn either
    /// way).
    pub shell: bool,
}

impl Engine {
    /// An engine drawing onto surfaces of `surface_format` (prefer a non-sRGB one: the map's and
    /// egui's colours are gamma-space values).
    pub fn new(gpu: &Gpu, surface_format: wgpu::TextureFormat, state: AppState) -> Self {
        let egui = egui::Context::default();
        theme::apply(&egui);
        Engine {
            state,
            scene: SceneBuilder::default(),
            pacer: Pacer::default(),
            map: MapRenderer::new(gpu, surface_format),
            input: MapInput::default(),
            // egui feathers its own edges; it draws after the map's MSAA resolve
            egui_renderer: egui_wgpu::Renderer::new(&gpu.device, surface_format, egui_wgpu::RendererOptions::default()),
            egui,
            built_for: None,
            rebuild: false,
            rebuilt_for_loss: false,
            draw: true,
            shell: false,
        }
    }

    /// Whether the window must open a new device and call [`Self::rebuild`] before it draws
    /// (B9), and takes the ask: at once when the map's device is lost (plan 2.2), once for that
    /// loss; again when the error card's Retry asks for one. The window asks before each frame.
    pub fn take_rebuild(&mut self) -> bool {
        match self.map.error() {
            Some(error) if error.lost && !self.rebuilt_for_loss => {
                self.rebuilt_for_loss = true;
                self.rebuild = true;
            }
            Some(_) => {}
            None => self.rebuilt_for_loss = false,
        }
        std::mem::take(&mut self.rebuild)
    }

    /// Remakes everything of the engine that lived on the old device on `gpu` (B9): the map
    /// ([`MapRenderer::rebuild`], which keeps its scene, camera and tiles) and egui's renderer, with
    /// a fresh egui context, since a context sends its font atlas whole only once. The window
    /// makes its `egui_winit::State` again on [`Self::egui`].
    pub fn rebuild(&mut self, gpu: &Gpu, surface_format: wgpu::TextureFormat) {
        self.map.rebuild(gpu, surface_format);
        self.egui_renderer = egui_wgpu::Renderer::new(&gpu.device, surface_format, egui_wgpu::RendererOptions::default());
        self.egui = egui::Context::default();
        theme::apply(&self.egui);
        self.pacer.force();
    }

    /// The map's share of a window event (after egui has seen it): the controller's gestures unless
    /// egui wants the pointer. Returns whether to draw a frame.
    pub fn window_event(&mut self, event: &WindowEvent, scale_factor: f64, local_ms: f64) -> bool {
        let Some(event) = input::map_event(event, scale_factor) else { return false };
        let egui_wants_pointer = self.egui.egui_wants_pointer_input();
        let held = self.input.dragging();
        let draw = self.input.handle(event, egui_wants_pointer, &mut self.state.controller, local_ms);
        // a press the map took: what it is on, for the click that may follow (A9, `_onPointerDown`)
        if !held && self.input.dragging() {
            let at = match event {
                input::MapEvent::Touch { at, .. } => Some(at),
                _ => self.input.pointer(),
            };
            if let Some(at) = at {
                self.state.pick_press(self.map.scene(), at);
            }
        }
        draw
    }

    /// One frame onto `surface` (`surface_size` device pixels, `scale_factor` of them per CSS
    /// pixel) at the local time `local_ms`, egui's events in `raw_input`. Returns egui's platform
    /// output (cursor, clipboard, links) and when to draw next.
    pub fn frame(
        &mut self,
        gpu: &Gpu,
        surface: &wgpu::TextureView,
        surface_size: [u32; 2],
        scale_factor: f64,
        raw_input: egui::RawInput,
        local_ms: f64,
    ) -> (egui::PlatformOutput, Next) {
        // 2 poll
        let mut wants = self.state.poll(local_ms);
        // 3 camera: on a canvas of the surface's size in CSS pixels
        let size = [f64::from(surface_size[0].max(1)) / scale_factor, f64::from(surface_size[1].max(1)) / scale_factor];
        if self.state.controller.size() != size {
            self.state.controller.set_size(size);
        }
        // the click and double click whose 300 ms are up, before the camera moves (B8)
        self.input.update(&mut self.state.controller, local_ms);
        // following a train or ship: flown to, then kept in the middle where it was last drawn
        let vehicle = self.scene.vehicle_at(&self.state);
        self.state.follow_vehicle(vehicle, local_ms);
        let (_, camera_wants) = self.state.advance_camera(local_ms);
        wants.merge(camera_wants);
        // hover and clicks against the scene the map draws, before the scene refresh (A9): a new
        // highlight or selection refreshes it now
        let pointer = self.input.pointer().filter(|_| !self.egui.is_pointer_over_egui());
        let clicks = self.input.take_clicks();
        let picked = self.state.pick_frame(self.map.scene(), pointer, self.input.dragging(), &clicks, local_ms);
        wants.merge(picked);

        // 4 scene: on its interval, or now when something changed
        let view = self.state.controller.view();
        let locked = self.state.locked_steady(local_ms);
        let built_for = (scale_factor, self.state.basemap());
        if wants.scene || self.built_for != Some(built_for) {
            self.pacer.force();
        }
        self.map.set_camera(view, size);
        if self.pacer.due(local_ms, view.zoom, locked) || self.map.needs_refresh() {
            let clocks = self.state.clocks(local_ms);
            let scene = self.scene.build(&mut self.state, clocks, scale_factor);
            self.map.update(scene);
            self.built_for = Some(built_for);
        }

        // 5 ui
        let mut actions = Vec::new();
        let adapter = gpu.adapter.get_info();
        let map = &mut self.map;
        let mut retry = None;
        let draw_shell = self.shell;
        // the hovered aircraft's card with the shell (U18), read before the pass: reading the
        // hover loader's cache refreshes its recency
        let card = if draw_shell { self.state.hover_card(local_ms) } else { None };
        let output = self.egui.run_ui(raw_input, |ui| {
            if draw_shell {
                ui_pass(ui, &self.state, local_ms, &mut actions);
            }
            // deck's tooltip at the pointer, over the map and under the panels (T8)
            tooltip::ui(ui.ctx(), self.state.pick.tooltip());
            // the hover card by the pointer over an aircraft, over the map and under the panels
            // (with the shell only: the headless shots and the map's tests draw the map alone)
            hover_card::ui(ui.ctx(), card.as_ref());
            // over everything else: the card while an error stops the map (B9)
            retry = map_error::ui(ui.ctx(), map, Some(&adapter));
        });
        let mut applied = false;
        for action in actions {
            applied |= self.state.apply(action, local_ms);
        }
        // what an action changed is drawn by the next frame, at once (U6)
        if applied {
            self.pacer.force();
        }
        // after a lost device the card's Retry asks the window for a new one (B9)
        if retry == Some(Retry::Rebuild) {
            self.rebuild = true;
        }
        // what may take the keys as of this pass (B8; the keys dialog and the help turn the map's
        // shortcuts off: U6), and deck's cursor over the map (a pointing hand over anything
        // pickable: A9)
        let typing = self.egui.text_edit_focused();
        let shortcuts_off = self.state.shortcuts.shortcuts_off();
        self.input.set_key_gate(KeyGate { typing, ui_focused: self.egui.egui_wants_keyboard_input(), shortcuts_off });
        let mut platform_output = output.platform_output;
        self.input.set_cursor(&self.egui, &mut platform_output, &self.state.controller, self.state.pick.hovering());

        // 6 render
        let repaint_delay = output.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay);
        self.render(gpu, surface, surface_size, output.shapes, output.textures_delta, output.pixels_per_point, local_ms);

        // 7 schedule
        let view = self.state.controller.view();
        let locked = self.state.locked_steady(local_ms);
        let mut at_ms = self.pacer.next_due_ms(view.zoom, locked);
        if let Some(t) = wants.wake_at {
            at_ms = at_ms.min(t);
        }
        // a click or double click waiting out its 300 ms (B8)
        at_ms = at_ms.min(self.input.next_due_ms());
        if repaint_delay != Duration::MAX {
            at_ms = at_ms.min(local_ms + repaint_delay.as_secs_f64() * 1000.0);
        }
        if self.map.pending() {
            // tiles and textures on their way: their answers wake the loop too, but look again soon
            at_ms = at_ms.min(local_ms + 50.0);
        }
        // Retry cleared the error: the map draws again in the next frame
        let retried = retry == Some(Retry::Redraw);
        let now = self.state.controller.in_transition() || self.state.controller.is_dragging() || repaint_delay.is_zero() || retried || self.rebuild;
        (platform_output, Next { now, at_ms })
    }

    /// Step 6: the map's passes and egui's onto `surface`, submitted.
    #[allow(clippy::too_many_arguments)]
    fn render(
        &mut self,
        gpu: &Gpu,
        surface: &wgpu::TextureView,
        surface_size: [u32; 2],
        shapes: Vec<egui::epaint::ClippedShape>,
        mut textures: egui::TexturesDelta,
        pixels_per_point: f32,
        local_ms: f64,
    ) {
        let (device, queue) = (&gpu.device, &gpu.queue);
        if !self.draw {
            // an undrawn frame (headless, P1): egui's textures and the map's uploads only, the
            // uploads sent at once rather than piling up for the next drawn frame
            for (id, deltas) in textures.set.drain() {
                for delta in &deltas {
                    self.egui_renderer.update_texture(device, queue, id, delta);
                }
            }
            self.map.prepare(gpu, local_ms);
            queue.submit(std::iter::empty::<wgpu::CommandBuffer>());
            for id in textures.free.drain() {
                self.egui_renderer.free_texture(&id);
            }
            return;
        }
        let primitives = self.egui.tessellate(shapes, pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: surface_size, pixels_per_point };
        // TexturesDelta asserts on drop that both lists were drained
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui_renderer.update_texture(device, queue, id, delta);
            }
        }
        self.map.prepare(gpu, local_ms);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        // egui's own command buffers go first (spike pitfall 5)
        let mut commands = self.egui_renderer.update_buffers(device, queue, &mut encoder, &primitives, &screen);
        self.map.draw(gpu, &mut encoder, surface, surface_size);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: surface,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.egui_renderer.render(&mut pass, &primitives, &screen);
        }
        commands.push(encoder.finish());
        queue.submit(commands);
        for id in textures.free.drain() {
            self.egui_renderer.free_texture(&id);
        }
    }
}

/// The UI pass: what egui draws over the map (the shell and the page's keys, U6) at local time
/// `local_ms`, and the actions it asks for.
fn ui_pass(ui: &mut egui::Ui, state: &AppState, local_ms: f64, actions: &mut Vec<Action>) {
    // a phone-sized window: the top bar, the bottom sheet and the rail instead (U19)
    let phone = phone::Layout::of(ui.ctx()).is_some();
    // the mode bar, before the sidebar in the page (and in the tab order: U7)
    if !phone {
        modebar::ui(ui.ctx(), state, actions);
    }
    // Escape closes the map controls' menu before the page's keys see it (U16)
    map_controls::escape(ui.ctx(), state, actions);
    // and the altitude chart's export menu, before the page's Escape closes the panel (U12)
    export_menu::escape(ui.ctx());
    // `--sheet-h`: the phone's sheet's height, 0 on the desktop
    let mut sheet_height = 0.0;
    if phone {
        // the page's keys, the top bar, the sheet with the list or the details, the rail, the
        // credits on the sheet (U19)
        let placed = phone::ui(ui.ctx(), state, local_ms, actions);
        sheet_height = placed.and_then(|p| p.sheet).map_or(0.0, |s| s.height);
    } else {
        shell::ui(ui.ctx(), state, local_ms, actions);
        // the map controls and their menu, after the panels (U16)
        map_controls::ui(ui.ctx(), state, actions);
        // the credits along the bottom, and the view's readout in debug builds (U7)
        attribution::ui(ui.ctx(), state, local_ms);
    }
    // the speed map's legend along the bottom of the map, under the panels (T4's data)
    speed_legend::ui(ui.ctx(), state, sheet_height);
    // the update offer at the bottom left, over the panels (U20)
    toast::ui(ui.ctx(), state);
    // the modal dialogs over everything: the shortcuts' help and the keys (U17)
    shortcuts::ui(ui.ctx(), state, actions);
    keys_dialog::ui(ui.ctx(), state, actions);
    // the API calls and the selected train, right, over the rest in debug builds (U20)
    dev_panel::ui(ui.ctx(), state, local_ms, actions);
}

/// Opens the window and runs until it is closed.
pub fn run(options: Options) -> Result<(), Error> {
    run_with(EventLoop::<UserEvent>::with_user_event(), options)
}

/// Like [`run`], for platforms that set the event loop up first (Android).
pub fn run_with(mut builder: EventLoopBuilder<UserEvent>, options: Options) -> Result<(), Error> {
    // the runtime before the event loop, kept for the process's lifetime (plan 2.1)
    let rt = requests::runtime()?;
    let event_loop = builder.build()?;
    let latch = WakeLatch::default();
    let proxy = event_loop.create_proxy();
    let wake = latch.wake_with(move || proxy.send_event(UserEvent::Wake).is_ok());
    // the activity for the phone glue (logcat, folders, insets, keyboard, back), before the
    // folders are read
    #[cfg(target_os = "android")]
    platform::android::init(winit::platform::android::EventLoopExtAndroid::android_app(&event_loop), wake.clone());
    let (events, events_rx) = mpsc::channel();
    let backend = start_backend(platform::dirs(), rt.handle(), events, wake.clone());
    let requests = Requests::new(rt.handle().clone(), wake);
    let view = options.view.unwrap_or(ViewState::INITIAL);
    let mut state = AppState::new(options.startup(), backend, Some(requests), Some(events_rx), view, WINDOW_SIZE);
    // times without a zone in the system's (today: the browser's); shots keep UTC (item F1)
    state.viewer_clocks = rustradar_core::time::Clocks::new(&platform::time_zone());
    let clock = options.now_ms.map_or_else(Clock::system, Clock::starting_at);
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App { options, clock, latch, state: Some(state), view: None, next: None, error: None };
    event_loop.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

struct App {
    options: Options,
    clock: Clock,
    latch: WakeLatch,
    /// The state until the window (and its engine) exists.
    state: Option<AppState>,
    view: Option<View>,
    /// What the last frame asked for.
    next: Option<Next>,
    /// Why start-up failed, reported once the loop has exited.
    error: Option<Error>,
}

/// Everything tied to the window.
struct View {
    window: Arc<Window>,
    gpu: Gpu,
    /// `None` while suspended: Android takes the native window away then.
    surface: Option<wgpu::Surface<'static>>,
    config: wgpu::SurfaceConfiguration,
    engine: Engine,
    egui_input: egui_winit::State,
}

impl View {
    fn new(event_loop: &ActiveEventLoop, options: &Options, state: AppState) -> Result<Self, Error> {
        let window = Arc::new(event_loop.create_window(window_attributes(options))?);
        // the GL backend needs the display connection up front (Wayland above all)
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(
            event_loop.owned_display_handle(),
        )));
        let surface = instance.create_surface(window.clone())?;
        let gpu = Gpu::for_surface(instance, &surface)?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .ok_or("the graphics adapter cannot present to this window")?;
        // a gamma-space format keeps the CSS colours and egui's blending exact (plan 1.4.1)
        let formats = surface.get_capabilities(&gpu.adapter).formats;
        config.format = formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(config.format);
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&gpu.device, &config);
        let mut engine = Engine::new(&gpu, config.format, state);
        // the window draws the UI over the map (U6)
        engine.shell = true;
        // the map's tiles: the --fixture folder's, else the network through the backend's cache (T1)
        tile_fetcher::connect(&mut engine.map, &engine.state);
        // remote pictures (photos, logos) through the backend's picture cache (U18)
        images::connect(&engine.egui, &engine.state);
        // wgpu's errors and a lost device stop the map with the error card, not the app (B9)
        engine.map.catch_errors(&gpu, engine.state.requests.as_ref().map(|r| r.wake()));
        let egui_input = egui_winit::State::new(
            engine.egui.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        log::info!("{}, {:?}, map MSAA x{}", gpu.describe(), config.format, engine.map.samples());
        Ok(View { window, gpu, surface: Some(surface), config, engine, egui_input })
    }

    fn resize(&mut self, width: u32, height: u32) {
        // a zero-sized surface (a minimised window) is invalid: keep the old one until it grows back
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        if let Some(surface) = &self.surface {
            surface.configure(&self.gpu.device, &self.config);
        }
    }

    /// Draws a frame and presents it. `None` when there was nothing to draw onto (suspended,
    /// occluded, a surface being remade).
    fn redraw(&mut self, local_ms: f64) -> Option<Next> {
        // the map's device lost: a new one before drawing (B9)
        if self.engine.take_rebuild() {
            self.rebuild();
        }
        let surface = self.surface.as_ref()?;
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                surface.configure(&self.gpu.device, &self.config);
                return None;
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return None,
            wgpu::CurrentSurfaceTexture::Validation => {
                log::error!("surface texture validation error");
                return None;
            }
        };
        let target = frame.texture.create_view(&Default::default());
        #[cfg_attr(not(target_os = "android"), allow(unused_mut))]
        let mut input = self.egui_input.take_egui_input(&self.window);
        // Android: the safe area and the soft keyboard's edits; whether back was pressed
        #[cfg(target_os = "android")]
        let back = crate::platform::android::before_frame(&self.engine.egui, &mut input, self.window.scale_factor() as f32);
        let size = [self.config.width, self.config.height];
        let (platform, next) = self.engine.frame(&self.gpu, &target, size, self.window.scale_factor(), input, local_ms);
        #[cfg(target_os = "android")]
        {
            // the focused text field mirrored to the keyboard; back closes the top card, else leaves
            crate::platform::android::after_frame(&self.engine.egui, &platform);
            if back && !self.engine.state.apply(Action::CloseTop, local_ms) {
                crate::platform::android::leave();
            }
        }
        self.egui_input.handle_platform_output(&self.window, platform);
        self.window.pre_present_notify();
        self.gpu.queue.present(frame);
        Some(next)
    }

    /// A new device for the map after the old one was lost (B9; plan 2.2): the adapter and device
    /// opened again on the window's surface, the surface configured on them, the engine's GPU parts
    /// and egui's window state made again. If no device opens, the map stays stopped with the card
    /// (its Retry tries again).
    fn rebuild(&mut self) {
        let Some(surface) = &self.surface else { return };
        match Gpu::for_surface(self.gpu.instance.clone(), surface) {
            Ok(gpu) => {
                surface.configure(&gpu.device, &self.config);
                self.engine.rebuild(&gpu, self.config.format);
                // the new egui context needs the picture loader again (U18)
                images::connect(&self.engine.egui, &self.engine.state);
                self.egui_input = egui_winit::State::new(
                    self.engine.egui.clone(),
                    egui::ViewportId::ROOT,
                    &self.window,
                    Some(self.window.scale_factor() as f32),
                    self.window.theme(),
                    Some(gpu.device.limits().max_texture_dimension_2d as usize),
                );
                log::info!("the map has a new device: {}", gpu.describe());
                self.gpu = gpu;
            }
            // the lost device's error stays: the card's Retry asks again
            Err(e) => log::error!("no new graphics device for the map: {e}"),
        }
        self.window.request_redraw();
    }
}

impl App {
    fn redraw(&mut self) {
        let now = self.clock.now_ms();
        let Some(view) = &mut self.view else { return };
        self.next = view.redraw(now);
    }

    /// Writes a settings change still waiting for its 500 ms, and waits for the disk.
    fn flush_settings(&mut self) {
        let state = match (&mut self.view, &mut self.state) {
            (Some(view), _) => &mut view.engine.state,
            (None, Some(state)) => state,
            (None, None) => return,
        };
        state.settings.flush(&state.persisted);
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if let (StartCause::ResumeTimeReached { .. }, Some(view)) = (cause, &self.view) {
            view.window.request_redraw();
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        match &mut self.view {
            None => {
                let Some(state) = self.state.take() else { return };
                match View::new(event_loop, &self.options, state) {
                    Ok(view) => {
                        self.view = Some(view);
                        // the first frame now, then show the window: a hidden window gets no redraw
                        // request on every platform
                        self.redraw();
                        if let Some(view) = &self.view {
                            view.window.set_visible(true);
                            view.window.request_redraw();
                        }
                    }
                    Err(e) => {
                        self.error = Some(e);
                        event_loop.exit();
                    }
                }
            }
            // back from the background: the window has a new native surface
            Some(view) if view.surface.is_none() => match view.gpu.instance.create_surface(view.window.clone()) {
                Ok(surface) => {
                    surface.configure(&view.gpu.device, &view.config);
                    view.surface = Some(surface);
                    view.window.request_redraw();
                }
                Err(e) => log::error!("cannot remake the window surface: {e}"),
            },
            Some(_) => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Wake => {
                // taken before the frame polls, so a wake raised meanwhile is sent again
                self.latch.reset();
                if let Some(view) = &self.view {
                    view.window.request_redraw();
                }
            }
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // a phone may end the app in the background: the settings not yet written go now (U5)
        self.flush_settings();
        // Android destroys the native window here and the surface must go before it does; the
        // desktop never suspends and iOS keeps its view
        if cfg!(target_os = "android")
            && let Some(view) = &mut self.view
        {
            view.surface = None;
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // the window closed or the system ends the app: the settings not yet written go now (U5)
        self.flush_settings();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let now = self.clock.now_ms();
        let Some(view) = &mut self.view else { return };
        // egui sees every event first
        let response = view.egui_input.on_window_event(&view.window, &event);
        if response.repaint {
            view.window.request_redraw();
        }
        match &event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
                return;
            }
            WindowEvent::Resized(size) => {
                view.resize(size.width, size.height);
                view.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
                return;
            }
            _ => {}
        }
        if view.engine.window_event(&event, view.window.scale_factor(), now) {
            view.window.request_redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(view) = &self.view else { return };
        let now = self.clock.now_ms();
        match self.next {
            Some(next) if next.now || next.at_ms <= now => {
                // continuous: each frame waits for the display (vsync), so this is paced
                event_loop.set_control_flow(ControlFlow::Wait);
                view.window.request_redraw();
            }
            Some(next) if next.at_ms.is_finite() => {
                let wait = Duration::from_secs_f64(((next.at_ms - now) / 1000.0).max(0.0));
                event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + wait));
            }
            Some(_) => event_loop.set_control_flow(ControlFlow::Wait),
            // nothing was drawn (occluded, suspended): look again in a while
            None => event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(250))),
        }
    }
}

#[cfg(test)]
mod tests {
    use winit::dpi::Size;

    use super::*;

    #[test]
    fn the_window_opens_as_the_tauri_one_did() {
        let a = window_attributes(&Options::default());
        assert_eq!(a.title, "RustRadar");
        assert_eq!(a.inner_size, Some(Size::Logical(LogicalSize::new(1440.0, 900.0))));
        assert_eq!(a.min_inner_size, Some(Size::Logical(LogicalSize::new(900.0, 600.0))));
        assert!(a.maximized);
        assert!(!a.visible, "hidden until the first frame");
        let sized = window_attributes(&Options { size: Some([1100, 720]), ..Default::default() });
        assert_eq!(sized.inner_size, Some(Size::Logical(LogicalSize::new(1100.0, 720.0))));
        assert!(!sized.maximized);
        assert!(!sized.visible);
    }

    #[test]
    fn without_folders_the_app_goes_on_without_the_backend() {
        let rt = requests::runtime().expect("runtime");
        let (tx, _rx) = mpsc::channel();
        assert!(start_backend(None, rt.handle(), tx, requests::no_wake()).is_none());
    }
}
