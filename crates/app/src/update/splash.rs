//! The launch's update with its progress window (item P3; doc 07 §5.2, §7.1; `RS:updater.rs`'s
//! `run`, `update`, `open_splash` and `show`, and `static/updater.html`).
//!
//! **The launch** ([`Launch`]): the main window is made hidden (`visible: false`, as today) and
//! stays hidden while a release build without `--updated` asks the endpoint, at most
//! [`CHECK_TIMEOUT`] (8 s). No update, the timeout or any error: the main window opens (shown and
//! focused). An update: the splash opens showing `Updating RustRadar…` with the sliver sweeping,
//! the update is downloaded (`Downloading update…` with the fraction, at most once per 1 %
//! ([`Meter`]), and only once the size is known), `Installing…` at 1.0 once the download has come,
//! then it is verified and installed ([`install`]) and the app relaunches as the new version
//! (`--updated`) and closes; on Windows the installer has taken over and starts it. The user never
//! sees the old version's interface. **An update is never a reason not to start**: a failed
//! check, download, signature, install or relaunch, or a splash that cannot open, logs `update
//! skipped: {e}`, closes the splash and opens the installed version.
//!
//! **The splash** ([`Splash`], [`paint`]): `RustRadar`, 380 × 150, not resizable, no decorations,
//! neither maximisable nor minimisable, always on top, centred on the primary monitor, first drawn
//! hidden and then shown. The page: `#04060b`, a 1 px `#1b2230` border, 32 px side padding, a
//! column centred on the height with 14 px gaps: `RustRadar` (15 px at 650, letter spacing
//! 0.02 em, `#e9edf3`), the status (14 px, `#9aa5b8`), the 3 px bar (`#141a26`, radius 2,
//! clipping its `#fcb442` fill). With a fraction the fill is `round(fraction × 100) %` wide,
//! eased there over 0.15 s (`ease-out`); without one a 30 % sliver sweeps from −100 % to 340 % of
//! its width every 1.2 s (`ease-in-out`), or under reduced motion the whole bar at 0.4.
//!
//! Wiring (`app.rs`): [`Launch::start`] before the event loop runs; the window's first frame is
//! drawn hidden and shown only when there is no launch; [`Launch::poll`] on every wake and before
//! the loop waits ([`Step::Open`] shows the main window, [`Step::Exit`] closes the app);
//! [`Launch::window_event`] takes the splash's events; [`Launch::about_to_wait`] keeps the splash
//! moving, at most every [`FRAME`] (60 a second, also without vsync), and the main window idle
//! meanwhile.
//!
//! **Checked** against Chromium's rendering of `updater.html` (with Overpass) at 380 × 150 for
//! `Updating…` (the sliver frozen at the same phase), `Downloading…` at 50 % and `Installing…`: the
//! text's ink and the bar on the same pixel rows and columns, a mean difference under 0.2 of 255
//! (`target/shots/pk3-splash-*.png`); and in a real window under Xvfb (centred, about 55 frames a
//! second while the download came, closed after the relaunch; `the_splash_in_a_window`, ignored by
//! default).
//!
//! Deviations:
//! - The text is Overpass (the app's font) rather than the system's `system-ui`.
//! - A download that stalls (nothing for [`STALL`], 60 s) is given up and the installed version
//!   opens; today's had no time limit and could hold the splash for ever.
//! - The download starts while the splash opens, rather than after; a splash that cannot open
//!   stops it.
//! - The main window idles while hidden (today's page ran hidden behind the check).
//! - `prefers-reduced-motion` is read from the egui context's [`anim::reduced_motion`], which no
//!   platform glue sets yet.

use std::error::Error;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, Vec2};
use rustradar_backend::Wake;
use rustradar_map::Gpu;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::{Window, WindowButtons, WindowId, WindowLevel};

use super::install::{self, Handover, Target};
use super::{CHECK_TIMEOUT, Updater};
use crate::ui::anim::{self, CSS_EASE_OUT, CubicBezier};
use crate::ui::text::{self, Style};
use crate::ui::theme;

/// The window's title.
pub const TITLE: &str = "RustRadar";
/// Its inner size, logical px.
pub const SIZE: [f64; 2] = [380.0, 150.0];

/// The status lines (`updater.rs`).
pub const UPDATING: &str = "Updating RustRadar…";
pub const DOWNLOADING: &str = "Downloading update…";
pub const INSTALLING: &str = "Installing…";

/// `updater.html`'s colours: the body and the window's background, the text, the status, the
/// border, the bar and its fill.
pub const BACKGROUND: Color32 = Color32::from_rgb(0x04, 0x06, 0x0b);
pub const TEXT: Color32 = Color32::from_rgb(0xe9, 0xed, 0xf3);
pub const STATUS: Color32 = Color32::from_rgb(0x9a, 0xa5, 0xb8);
pub const BORDER: Color32 = Color32::from_rgb(0x1b, 0x22, 0x30);
pub const TRACK: Color32 = Color32::from_rgb(0x14, 0x1a, 0x26);
pub const FILL: Color32 = Color32::from_rgb(0xfc, 0xb4, 0x42);

/// `main { padding: 0 32px; gap: 14px; border: 1px }`.
pub const PAD_X: f32 = 32.0;
pub const GAP: f32 = 14.0;
pub const BORDER_WIDTH: f32 = 1.0;
/// `.bar { height: 3px; border-radius: 2px }`.
pub const BAR_HEIGHT: f32 = 3.0;
pub const BAR_RADIUS: u8 = 2;
/// `#fill { transition: width 0.15s ease-out }`.
pub const FILL_TRANSITION: f64 = 0.15;
/// `.indeterminate #fill { width: 30%; animation: sweep 1.2s ease-in-out infinite }`, from
/// `translateX(-100%)` to `translateX(340%)`.
pub const SLIVER: f32 = 0.3;
pub const SWEEP_PERIOD: f64 = 1.2;
pub const SWEEP_FROM: f32 = -1.0;
pub const SWEEP_TO: f32 = 3.4;
pub const EASE_IN_OUT: CubicBezier = CubicBezier::new(0.42, 0.0, 0.58, 1.0);
/// The reduced-motion bar's opacity.
pub const REDUCED_OPACITY: f32 = 0.4;

/// How long a download may go without a byte before it is given up.
pub const STALL: Duration = Duration::from_secs(60);

/// The splash's frames while it moves: at most 60 a second, also where nothing paces the
/// presentation (no vsync).
pub const FRAME: Duration = Duration::from_nanos(16_666_667);

/// The launch's time limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// For the check: [`CHECK_TIMEOUT`].
    pub check: Duration,
    /// For a download without a byte: [`STALL`].
    pub stall: Duration,
}

impl Limits {
    /// Today's 8 s for the check, and this port's 60 s stall.
    pub const LAUNCH: Limits = Limits { check: CHECK_TIMEOUT, stall: STALL };
}

/// `h1`: 15 px at 650, `letter-spacing: 0.02em`, at the body's line height 1.4.
pub const fn title_style() -> Style {
    Style::ui(15.0, 650, TEXT).letter_spacing(0.02)
}

/// `#status`: the body's 14 px at 400 in `#9aa5b8`.
pub const fn status_style() -> Style {
    Style::ui(theme::TEXT_BASE, theme::WEIGHT_REGULAR, STATUS)
}

/// What the splash shows (today's `Progress`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shown {
    pub status: &'static str,
    /// 0 to 1, or `None` while the size is not known (the sweeping sliver).
    pub fraction: Option<f64>,
}

impl Shown {
    /// The splash as it opens.
    pub const UPDATING: Shown = Shown { status: UPDATING, fraction: None };
    /// Once the download has come.
    pub const INSTALLING: Shown = Shown { status: INSTALLING, fraction: Some(1.0) };
}

/// Today's progress rule over the download's chunks: the fraction once the size is known, shown
/// when it has grown by 1 % or reached the end.
#[derive(Debug, Default, Clone)]
pub struct Meter {
    downloaded: u64,
    shown: f64,
}

impl Meter {
    /// One chunk of `len` bytes of `total`: what to show now, if anything.
    pub fn chunk(&mut self, len: usize, total: Option<u64>) -> Option<Shown> {
        self.downloaded += len as u64;
        let total = total.filter(|t| *t > 0)?;
        let fraction = (self.downloaded as f64 / total as f64).min(1.0);
        // a download is thousands of chunks; a frame per percent is plenty
        if fraction - self.shown >= 0.01 || fraction >= 1.0 {
            self.shown = fraction;
            return Some(Shown { status: DOWNLOADING, fraction: Some(fraction) });
        }
        None
    }
}

// ------------------------------------------------------------------------------------------------
// the page

/// Where the page's parts are in a window of `size` (CSS px): the column's line boxes, each top
/// on the pixel grid as Chromium paints them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub left: f32,
    pub width: f32,
    pub title_top: f32,
    pub status_top: f32,
    pub bar: Rect,
}

/// The page's layout: `justify-content: center` in the border box.
pub fn layout(size: Vec2) -> Layout {
    let (title, status) = (title_style().line_px(), status_style().line_px());
    let column = title + GAP + status + GAP + BAR_HEIGHT;
    let top = BORDER_WIDTH + (size.y - 2.0 * BORDER_WIDTH - column) / 2.0;
    let left = BORDER_WIDTH + PAD_X;
    let width = (size.x - 2.0 * (BORDER_WIDTH + PAD_X)).max(0.0);
    let bar_top = (top + title + GAP + status + GAP).round();
    Layout {
        left,
        width,
        title_top: top.round(),
        status_top: (top + title + GAP).round(),
        bar: Rect::from_min_size(Pos2::new(left, bar_top), Vec2::new(width, BAR_HEIGHT)),
    }
}

/// The bar's fill: indeterminate (the sliver), or a width easing to its target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fill {
    determinate: bool,
    from: f32,
    to: f32,
    start: f64,
}

impl Default for Fill {
    /// The page as it loads: `.bar.indeterminate`.
    fn default() -> Fill {
        Fill { determinate: false, from: SLIVER, to: SLIVER, start: 0.0 }
    }
}

impl Fill {
    /// `window.__update`'s `fraction` at `t` seconds: the class toggled, the width set to
    /// `Math.round(fraction * 100)%`, its transition starting from the width drawn now.
    pub fn set(&mut self, fraction: Option<f64>, t: f64) {
        match fraction {
            None => *self = Fill { start: t, ..Fill::default() },
            Some(f) => {
                let target = ((f * 100.0).round() / 100.0) as f32;
                if self.determinate && target == self.to {
                    return;
                }
                let from = self.width(t);
                *self = Fill { determinate: true, from, to: target, start: t };
            }
        }
    }

    /// Whether the size is unknown (the sweeping sliver).
    pub fn indeterminate(&self) -> bool {
        !self.determinate
    }

    /// The fill's width at `t`, as a share of the bar.
    pub fn width(&self, t: f64) -> f32 {
        if !self.determinate {
            return SLIVER;
        }
        let progress = ((t - self.start) / FILL_TRANSITION).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * CSS_EASE_OUT.at(progress) as f32
    }

    /// Whether it still moves at `t`.
    pub fn moving(&self, t: f64) -> bool {
        !self.determinate || t - self.start < FILL_TRANSITION
    }

    /// The fill's span at `t` within `bar` (before the bar clips it) and its opacity.
    pub fn span(&self, bar: Rect, t: f64, reduced_motion: bool) -> (f32, f32, f32) {
        if self.determinate {
            return (bar.min.x, bar.min.x + bar.width() * self.width(t), 1.0);
        }
        if reduced_motion {
            return (bar.min.x, bar.max.x, REDUCED_OPACITY);
        }
        let w = bar.width() * SLIVER;
        let phase = (t / SWEEP_PERIOD).rem_euclid(1.0);
        let shift = SWEEP_FROM + (SWEEP_TO - SWEEP_FROM) * EASE_IN_OUT.at(phase) as f32;
        let x = bar.min.x + shift * w;
        (x, x + w, 1.0)
    }
}

/// Paints the page into `ui` (a window of `size` CSS px) as `shown` and `fill` say at `t`.
pub fn paint(ui: &egui::Ui, size: Vec2, shown: Shown, fill: &Fill, t: f64) {
    let painter = ui.painter();
    let page = Rect::from_min_size(Pos2::ZERO, size);
    painter.rect(page, CornerRadius::ZERO, BACKGROUND, Stroke::new(BORDER_WIDTH, BORDER), StrokeKind::Inside);
    let l = layout(size);
    let title = painter.layout_job(text::job(TITLE, &title_style()));
    painter.galley(Pos2::new(l.left, l.title_top), title, Color32::PLACEHOLDER);
    let status = painter.layout_job(text::wrapped(shown.status, &status_style(), l.width));
    painter.galley(Pos2::new(l.left, l.status_top), status, Color32::PLACEHOLDER);

    painter.rect_filled(l.bar, CornerRadius::same(BAR_RADIUS), TRACK);
    let (x0, x1, opacity) = fill.span(l.bar, t, anim::reduced_motion(ui.ctx()));
    let (x0, x1) = (x0.max(l.bar.min.x), x1.min(l.bar.max.x));
    if x1 > x0 {
        // `overflow: hidden` on the rounded bar: the fill's ends round where they meet the bar's
        let r = f32::from(BAR_RADIUS);
        let left = if x0 <= l.bar.min.x + r { BAR_RADIUS } else { 0 };
        let right = if x1 >= l.bar.max.x - r { BAR_RADIUS } else { 0 };
        let corners = CornerRadius { nw: left, sw: left, ne: right, se: right };
        let rect = Rect::from_x_y_ranges(x0..=x1, l.bar.y_range());
        painter.rect_filled(rect, corners, FILL.gamma_multiply(opacity));
    }
}

/// The page's egui pass and its drawing onto a texture (the splash window's surface; a test's
/// offscreen target).
pub struct Page {
    ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    shown: Shown,
    fill: Fill,
}

impl Page {
    /// A page drawing onto textures of `format` (a non-sRGB one: the colours are CSS values).
    pub fn new(gpu: &Gpu, format: wgpu::TextureFormat, shown: Shown) -> Page {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let renderer = egui_wgpu::Renderer::new(&gpu.device, format, egui_wgpu::RendererOptions::default());
        let mut fill = Fill::default();
        fill.set(shown.fraction, 0.0);
        Page { ctx, renderer, shown, fill }
    }

    /// The page's egui context (tests set reduced motion on it).
    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    /// `window.__update(shown)` at `t` seconds. Returns whether it changed.
    pub fn show(&mut self, shown: Shown, t: f64) -> bool {
        if shown == self.shown {
            return false;
        }
        self.fill.set(shown.fraction, t);
        self.shown = shown;
        true
    }

    /// Draws the page at `t` seconds onto `target` (`size` device px at `pixels_per_point`).
    /// Returns whether it still moves.
    pub fn render(&mut self, gpu: &Gpu, target: &wgpu::TextureView, size: [u32; 2], pixels_per_point: f32, t: f64) -> bool {
        let points = Vec2::new(size[0] as f32, size[1] as f32) / pixels_per_point;
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, points)),
            time: Some(t),
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(pixels_per_point);
        let (shown, fill) = (self.shown, self.fill);
        let output = self.ctx.run_ui(raw, |ui| paint(ui, points, shown, &fill, t));

        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.renderer.update_texture(device, queue, id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("splash") });
        let mut commands = self.renderer.update_buffers(device, queue, &mut encoder, &primitives, &screen);
        {
            let [r, g, b, _] = BACKGROUND.to_array().map(|c| f64::from(c) / 255.0);
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("splash"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }), store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.renderer.render(&mut pass, &primitives, &screen);
        }
        commands.push(encoder.finish());
        queue.submit(commands);
        for id in textures.free.drain() {
            self.renderer.free_texture(&id);
        }
        fill.moving(t) && !anim::reduced_motion(&self.ctx)
    }
}

// ------------------------------------------------------------------------------------------------
// the window

/// The splash window: [`Page`] on a surface of its own, on the main window's device.
pub struct Splash {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    page: Page,
    opened: Instant,
    moving: bool,
    /// When the last frame was drawn (the pace of [`FRAME`]).
    drawn: Instant,
}

/// The position that centres a window of `size` logical px on the primary monitor (Tauri's
/// `center()`), if the system names one.
fn centred(event_loop: &ActiveEventLoop, size: [f64; 2]) -> Option<PhysicalPosition<i32>> {
    let monitor = event_loop.primary_monitor().or_else(|| event_loop.available_monitors().next())?;
    let (at, area, scale) = (monitor.position(), monitor.size(), monitor.scale_factor());
    let x = f64::from(at.x) + (f64::from(area.width) - size[0] * scale) / 2.0;
    let y = f64::from(at.y) + (f64::from(area.height) - size[1] * scale) / 2.0;
    Some(PhysicalPosition::new(x.round() as i32, y.round() as i32))
}

impl Splash {
    /// Opens the splash showing `shown`, on `gpu` (the main window's device): drawn once hidden,
    /// then shown.
    pub fn open(event_loop: &ActiveEventLoop, gpu: &Gpu, shown: Shown) -> Result<Splash, Box<dyn Error>> {
        let mut attributes = Window::default_attributes()
            .with_title(TITLE)
            .with_inner_size(LogicalSize::new(SIZE[0], SIZE[1]))
            .with_resizable(false)
            .with_decorations(false)
            .with_enabled_buttons(WindowButtons::CLOSE)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_visible(false);
        if let Some(at) = centred(event_loop, SIZE) {
            attributes = attributes.with_position(at);
        }
        let window = Arc::new(event_loop.create_window(attributes)?);
        let surface = gpu.instance.create_surface(window.clone())?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .ok_or("the graphics adapter cannot present to the update window")?;
        let formats = surface.get_capabilities(&gpu.adapter).formats;
        config.format = formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(config.format);
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&gpu.device, &config);
        let page = Page::new(gpu, config.format, shown);
        let now = Instant::now();
        let mut splash = Splash { window, surface, config, page, opened: now, moving: true, drawn: now };
        splash.draw(gpu);
        splash.window.set_visible(true);
        Ok(splash)
    }

    /// The splash window's id.
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// Seconds since it opened (the page's clock).
    fn time(&self) -> f64 {
        self.opened.elapsed().as_secs_f64()
    }

    /// `window.__update(shown)`: drawn with the next frame ([`Splash::about_to_wait`]).
    pub fn show(&mut self, shown: Shown) {
        let t = self.time();
        if self.page.show(shown, t) {
            self.moving = true;
        }
    }

    /// Draws a frame and presents it.
    pub fn draw(&mut self, gpu: &Gpu) {
        let size = self.window.inner_size();
        if size.width > 0 && size.height > 0 && (size.width, size.height) != (self.config.width, self.config.height) {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&gpu.device, &self.config);
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&gpu.device, &self.config);
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Validation => return,
        };
        let view = frame.texture.create_view(&Default::default());
        let t = self.time();
        let size = [self.config.width, self.config.height];
        self.moving = self.page.render(gpu, &view, size, self.window.scale_factor() as f32, t);
        self.drawn = Instant::now();
        self.window.pre_present_notify();
        gpu.queue.present(frame);
    }

    /// Handles one of the splash window's events. Closing it is ignored: the update goes on.
    pub fn window_event(&mut self, gpu: &Gpu, event: &WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => self.draw(gpu),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.window.request_redraw(),
            _ => {}
        }
    }

    /// Asks for the next frame while the page moves, a [`FRAME`] after the last; otherwise the
    /// loop waits for the next wake.
    pub fn about_to_wait(&self, event_loop: &ActiveEventLoop) {
        let due = self.drawn + FRAME;
        if !self.moving {
            event_loop.set_control_flow(ControlFlow::Wait);
        } else if Instant::now() >= due {
            self.window.request_redraw();
            event_loop.set_control_flow(ControlFlow::Wait);
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(due));
        }
    }
}

// ------------------------------------------------------------------------------------------------
// the launch

/// What the launch's task answers: what the install left to do, or nothing to install.
pub type Answer = Result<Option<Handover>, String>;

/// Today's `update`: the check within `limits.check`; an update shown (`show`), downloaded with
/// [`Meter`]'s progress and given up after `limits.stall` without a byte, `Installing…`, then
/// `install(version, bytes)` on the blocking pool.
pub async fn update<S, I>(updater: Updater, show: S, install: I, limits: Limits) -> Answer
where
    S: Fn(Shown) + Send + Sync,
    I: FnOnce(String, Vec<u8>) -> Result<Handover, install::Error> + Send + 'static,
{
    let update = match tokio::time::timeout(limits.check, updater.check()).await {
        Err(elapsed) => return Err(elapsed.to_string()),
        Ok(Err(e)) => return Err(e.to_string()),
        Ok(Ok(None)) => return Ok(None),
        Ok(Ok(Some(update))) => update,
    };
    show(Shown::UPDATING);
    let mut meter = Meter::default();
    let last = Mutex::new(Instant::now());
    let download = update.download(
        |chunk, total| {
            *last.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
            if let Some(shown) = meter.chunk(chunk, total) {
                show(shown);
            }
        },
        || show(Shown::INSTALLING),
    );
    let bytes = tokio::select! {
        bytes = download => bytes.map_err(|e| e.to_string())?,
        () = stalled(&last, limits.stall) => return Err(format!("the download stalled: nothing came for {} s", limits.stall.as_secs())),
    };
    let version = update.version.clone();
    let handover = tokio::task::spawn_blocking(move || install(version, bytes)).await.map_err(|e| e.to_string())?;
    handover.map(Some).map_err(|e| e.to_string())
}

/// Ends once `last` (the last byte's time) is `stall` old.
async fn stalled(last: &Mutex<Instant>, stall: Duration) {
    loop {
        let since = last.lock().unwrap_or_else(PoisonError::into_inner).elapsed();
        if since >= stall {
            return;
        }
        tokio::time::sleep(stall - since).await;
    }
}

/// What the window does after [`Launch::poll`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The check or the update is still under way: the main window stays hidden.
    Wait,
    /// Open the installed version: show the main window (the splash is closed).
    Open,
    /// The new version has been started (or its installer has): close the app.
    Exit,
}

/// The launch's update: its task, what it shows, the splash once there is something to show.
pub struct Launch {
    shown: Arc<Mutex<Option<Shown>>>,
    answer: Receiver<Answer>,
    task: JoinHandle<()>,
    splash: Option<Splash>,
    relaunch: fn() -> std::io::Result<()>,
    /// The step taken once the answer came: asked again, it is the same.
    done: Option<Step>,
}

impl Launch {
    /// Starts the launch's check on `rt` (today's `run` before `show`): `None`, and the main window
    /// opens at once, in debug builds, with `--updated`, or when this build updates nothing.
    pub fn start(updated: bool, rt: &Handle, wake: Wake) -> Option<Launch> {
        if !install::CHECKS || updated {
            return None;
        }
        install::remember_binary();
        let target = match Target::current() {
            Ok(target) => target,
            Err(why) => {
                log::info!("no update check: {why}");
                return None;
            }
        };
        let updater = match Updater::new() {
            Ok(updater) => updater,
            Err(e) => {
                log::warn!("update skipped: {e}");
                return None;
            }
        };
        Some(Launch::spawn(rt, wake, updater, move |version, bytes| target.install(&version, &bytes), Limits::LAUNCH, install::relaunch))
    }

    /// Starts [`update`] with `install` and `relaunch` (tests give their own).
    pub fn spawn(
        rt: &Handle,
        wake: Wake,
        updater: Updater,
        install: impl FnOnce(String, Vec<u8>) -> Result<Handover, install::Error> + Send + 'static,
        limits: Limits,
        relaunch: fn() -> std::io::Result<()>,
    ) -> Launch {
        let shown: Arc<Mutex<Option<Shown>>> = Arc::default();
        let (tx, answer) = mpsc::channel();
        let into = shown.clone();
        let woken = wake.clone();
        let show = move |s: Shown| {
            *into.lock().unwrap_or_else(PoisonError::into_inner) = Some(s);
            woken();
        };
        let task = rt.spawn(async move {
            let _ = tx.send(update(updater, show, install, limits).await);
            wake();
        });
        Launch { shown, answer, task, splash: None, relaunch, done: None }
    }

    /// What the splash shows (or would show), if the launch found an update.
    pub fn shown(&self) -> Option<Shown> {
        *self.shown.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the splash window is open.
    pub fn splash_open(&self) -> bool {
        self.splash.is_some()
    }

    /// Opens or updates the splash (once `gpu`, the main window's device, exists) and looks at the
    /// task's answer.
    pub fn poll(&mut self, event_loop: &ActiveEventLoop, gpu: Option<&Gpu>) -> Step {
        if let Some(step) = self.done {
            return step;
        }
        if let (Some(shown), Some(gpu)) = (self.shown(), gpu) {
            match &mut self.splash {
                Some(splash) => splash.show(shown),
                None => match Splash::open(event_loop, gpu, shown) {
                    Ok(splash) => self.splash = Some(splash),
                    Err(e) => {
                        log::warn!("update skipped: {e}");
                        self.task.abort();
                        self.done = Some(Step::Open);
                        return Step::Open;
                    }
                },
            }
        }
        self.step()
    }

    /// The task's answer, if it came: open the installed version, or close after the relaunch.
    pub fn step(&mut self) -> Step {
        if let Some(step) = self.done {
            return step;
        }
        let answer = match self.answer.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return Step::Wait,
            Err(TryRecvError::Disconnected) => Err("the update stopped".into()),
        };
        let step = match answer {
            Ok(None) => Step::Open,
            // Windows: the installer took over and starts the new version itself
            Ok(Some(Handover::Installer)) => Step::Exit,
            Ok(Some(Handover::Relaunch)) => match (self.relaunch)() {
                Ok(()) => Step::Exit,
                Err(e) => {
                    log::warn!("update skipped: {e}");
                    Step::Open
                }
            },
            Err(e) => {
                log::warn!("update skipped: {e}");
                Step::Open
            }
        };
        if step == Step::Open {
            // close the splash if any
            self.splash = None;
        }
        self.done = Some(step);
        step
    }

    /// Takes the splash window's events; false for another window's.
    pub fn window_event(&mut self, gpu: &Gpu, id: WindowId, event: &WindowEvent) -> bool {
        match &mut self.splash {
            Some(splash) if splash.id() == id => {
                splash.window_event(gpu, event);
                true
            }
            _ => false,
        }
    }

    /// Before the loop waits: the splash's next frame while it moves; otherwise the next wake.
    pub fn about_to_wait(&self, event_loop: &ActiveEventLoop) {
        match &self.splash {
            Some(splash) => splash.about_to_wait(event_loop),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

impl Drop for Launch {
    fn drop(&mut self) {
        // an answer nobody reads any more (the window shown, the app closing)
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use reqwest::Url;

    use super::super::install::tests::{Server, payload};
    use super::*;

    #[test]
    fn the_progress_rule_is_todays() {
        let mut meter = Meter::default();
        // the size unknown: nothing (the sliver goes on sweeping)
        assert_eq!(meter.chunk(1000, None), None);
        assert_eq!(meter.chunk(1000, Some(0)), None);
        let mut meter = Meter::default();
        let total = Some(100_000);
        assert_eq!(meter.chunk(999, total), None, "under 1 %");
        assert_eq!(meter.chunk(1, total), Some(Shown { status: DOWNLOADING, fraction: Some(0.01) }));
        assert_eq!(meter.chunk(500, total), None);
        let shown = meter.chunk(600, total).unwrap();
        assert!((shown.fraction.unwrap() - 0.021).abs() < 1e-9);
        // the last chunk always shows, at most 1
        assert_eq!(meter.chunk(97_900, total), Some(Shown { status: DOWNLOADING, fraction: Some(1.0) }));
        assert_eq!(meter.chunk(10, total), Some(Shown { status: DOWNLOADING, fraction: Some(1.0) }));
        // a 30 MB download in 16 kB chunks: about a hundred updates, not two thousand
        let mut meter = Meter::default();
        let total = 30_000_000u64;
        let shown = (0..total.div_ceil(16_384)).filter_map(|_| meter.chunk(16_384, Some(total))).count();
        assert!((95..=101).contains(&shown), "{shown}");
        assert_eq!(Shown::UPDATING, Shown { status: "Updating RustRadar…", fraction: None });
        assert_eq!(Shown::INSTALLING, Shown { status: "Installing…", fraction: Some(1.0) });
        assert_eq!(DOWNLOADING, "Downloading update…");
    }

    #[test]
    fn the_page_is_laid_out_as_updater_html() {
        let l = layout(Vec2::new(380.0, 150.0));
        // a column of 21 + 14 + 19.6 + 14 + 3 = 71.6 px centred in 148
        assert_eq!((l.left, l.width), (33.0, 314.0));
        assert_eq!((l.title_top, l.status_top), (39.0, 74.0));
        assert_eq!(l.bar, Rect::from_min_size(Pos2::new(33.0, 108.0), Vec2::new(314.0, 3.0)));
        assert_eq!(title_style().line_px(), 21.0);
        assert!((status_style().line_px() - 19.6).abs() < 1e-5);
    }

    #[test]
    fn the_fill_eases_to_its_width_and_the_sliver_sweeps() {
        let bar = Rect::from_min_size(Pos2::new(33.0, 108.0), Vec2::new(314.0, 3.0));
        let mut fill = Fill::default();
        assert!(fill.indeterminate());
        // the sliver: 30 % wide, from -100 % of its width to 340 %, eased in and out
        let w = 314.0 * 0.3;
        let (x0, x1, a) = fill.span(bar, 0.0, false);
        assert!(((x0 - (33.0 - w)).abs() < 1e-3) && ((x1 - 33.0).abs() < 1e-3) && a == 1.0);
        let (x0, _, _) = fill.span(bar, 0.6, false);
        assert!((x0 - (33.0 + 1.2 * w)).abs() < 1e-3, "halfway through at half the period: {x0}");
        assert!(fill.span(bar, 1.2, false).0 < 33.0 - w + 1e-3, "and again");
        assert_eq!(fill.span(bar, 0.3, true), (33.0, 347.0, REDUCED_OPACITY));
        assert!(fill.moving(100.0));

        // a fraction: from the sliver's 30 % to round(f * 100) % over 0.15 s
        fill.set(Some(0.014), 10.0);
        assert!(!fill.indeterminate());
        assert_eq!(fill.width(10.0), 0.3);
        assert!((fill.width(10.15) - 0.01).abs() < 1e-6);
        assert!(fill.width(10.075) < 0.3 && fill.width(10.075) > 0.01);
        assert!(fill.moving(10.1) && !fill.moving(10.2));
        // the same percentage again does not restart it
        fill.set(Some(0.0149), 10.2);
        assert!(!fill.moving(10.2));
        // a new one starts from where the fill is
        fill.set(Some(0.5), 11.0);
        assert!((fill.width(11.0) - 0.01).abs() < 1e-6);
        let (x0, x1, _) = fill.span(bar, 12.0, false);
        assert_eq!((x0, x1), (33.0, 33.0 + 157.0));
    }

    #[test]
    fn the_splash_is_drawn_as_todays_page() {
        use rustradar_map::headless::{FORMAT, Offscreen};
        let gpu = match Gpu::headless() {
            Ok(gpu) => gpu,
            Err(e) => {
                println!("skipped: {e}");
                return;
            }
        };
        let size = [380u32, 150u32];
        let target = Offscreen::new(&gpu, size);
        let mut page = Page::new(&gpu, FORMAT, Shown::UPDATING);
        let shot = |page: &mut Page, t: f64, name: &str| {
            // a few frames for the fonts to load
            for _ in 0..3 {
                page.render(&gpu, target.view(), size, 1.0, t);
            }
            let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
            let image = target.read(&gpu).expect("read back");
            let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
            std::fs::create_dir_all(&dir).unwrap();
            image.save_png(&dir.join(name)).unwrap();
            image
        };
        let rgb = |image: &rustradar_map::headless::Image, x: u32, y: u32| {
            let [r, g, b, _] = image.pixel(x, y);
            [r, g, b]
        };

        // opening: the sliver a quarter of the way through its sweep
        let image = shot(&mut page, 0.45, "pk3-splash-updating.png");
        assert_eq!(rgb(&image, 190, 2), [0x04, 0x06, 0x0b], "the page");
        assert_eq!(rgb(&image, 0, 75), [0x1b, 0x22, 0x30], "the border");
        assert_eq!(rgb(&image, 379, 149), [0x1b, 0x22, 0x30]);
        let ink = |image: &rustradar_map::headless::Image, rows: std::ops::Range<u32>, near: [u8; 3]| {
            let mut n = 0;
            for y in rows {
                for x in 33..347 {
                    let p = rgb(image, x, y);
                    if (0..3).all(|i| p[i].abs_diff(near[i]) < 40) {
                        n += 1;
                    }
                }
            }
            n
        };
        assert!(ink(&image, 39..60, TEXT.to_array()[..3].try_into().unwrap()) > 60, "RustRadar in #e9edf3");
        assert!(ink(&image, 74..94, STATUS.to_array()[..3].try_into().unwrap()) > 60, "the status in #9aa5b8");
        let track: [u8; 3] = TRACK.to_array()[..3].try_into().unwrap();
        let fill: [u8; 3] = FILL.to_array()[..3].try_into().unwrap();
        let bar_row: Vec<[u8; 3]> = (33..347).map(|x| rgb(&image, x, 109)).collect();
        let lit = bar_row.iter().filter(|p| **p == fill).count();
        assert!((80..=96).contains(&lit), "the 94 px sliver inside the bar: {lit}");
        assert_eq!(rgb(&image, 344, 109), track, "the bar's far end");
        assert_eq!(rgb(&image, 190, 107), [0x04, 0x06, 0x0b], "the bar is 3 px");
        assert_eq!(rgb(&image, 190, 111), [0x04, 0x06, 0x0b]);

        // downloading, halfway: the fill from the bar's start to its middle
        page.show(Shown { status: DOWNLOADING, fraction: Some(0.5) }, 1.0);
        let image = shot(&mut page, 2.0, "pk3-splash-downloading.png");
        assert_eq!(rgb(&image, 40, 109), fill);
        assert_eq!(rgb(&image, 188, 109), fill);
        assert_eq!(rgb(&image, 192, 109), track);
        // installing: the whole bar
        page.show(Shown::INSTALLING, 2.0);
        let image = shot(&mut page, 3.0, "pk3-splash-installing.png");
        assert_eq!(rgb(&image, 40, 109), fill);
        assert_eq!(rgb(&image, 340, 109), fill);
        assert!(!page.render(&gpu, target.view(), size, 1.0, 3.0), "at rest");
        // reduced motion: the whole bar at 0.4, still
        let mut page = Page::new(&gpu, FORMAT, Shown::UPDATING);
        anim::set_reduced_motion(page.ctx(), true);
        let image = shot(&mut page, 0.45, "pk3-splash-reduced.png");
        let p = rgb(&image, 190, 109);
        let expected = |i: usize| (f32::from(fill[i]) * 0.4 + f32::from(track[i]) * 0.6) as u8;
        assert!((0..3).all(|i| p[i].abs_diff(expected(i)) <= 3), "{p:?}");
    }

    fn counting_wake() -> (Wake, Arc<AtomicUsize>) {
        let wakes = Arc::new(AtomicUsize::new(0));
        let count = wakes.clone();
        (Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        }), wakes)
    }

    /// Polls `launch` (without a window) until it answers.
    fn settle(launch: &mut Launch) -> Step {
        let started = Instant::now();
        loop {
            let step = launch.step();
            if step != Step::Wait || started.elapsed() > Duration::from_secs(20) {
                return step;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn relaunched() -> std::io::Result<()> {
        Ok(())
    }

    fn not_relaunched() -> std::io::Result<()> {
        Err(std::io::Error::other("the binary is gone"))
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
    }

    #[test]
    fn an_update_is_shown_downloaded_installed_and_relaunched() {
        let rt = runtime();
        let server = Server::start();
        server.route("/artifact", 200, payload());
        server.release("9.0.0");
        let (wake, wakes) = counting_wake();
        let installed = Arc::new(Mutex::new(None));
        let into = installed.clone();
        let install = move |version: String, bytes: Vec<u8>| {
            *into.lock().unwrap() = Some((version, bytes.len()));
            Ok(Handover::Relaunch)
        };
        let mut launch = Launch::spawn(rt.handle(), wake, server.updater(), install, Limits::LAUNCH, relaunched);
        assert_eq!(settle(&mut launch), Step::Exit, "relaunched, then closed");
        assert_eq!(*installed.lock().unwrap(), Some(("9.0.0".to_owned(), 200_000)));
        assert_eq!(launch.shown(), Some(Shown::INSTALLING), "the last thing shown");
        assert!(wakes.load(Ordering::SeqCst) >= 3, "each change wakes the window");
        assert!(!launch.splash_open(), "no window in a test");
    }

    #[test]
    fn every_failure_opens_the_installed_version() {
        let rt = runtime();
        let no_install = |_: String, _: Vec<u8>| -> Result<Handover, install::Error> { panic!("nothing to install") };

        // nothing newer
        let server = Server::start();
        server.route("/artifact", 200, payload());
        server.release("8.1.0");
        let (wake, _) = counting_wake();
        let mut launch = Launch::spawn(rt.handle(), wake.clone(), server.updater(), no_install, Limits::LAUNCH, relaunched);
        assert_eq!(settle(&mut launch), Step::Open);
        assert_eq!(launch.shown(), None, "no splash");

        // no release at the endpoint (404)
        let empty = Server::start();
        let mut launch = Launch::spawn(rt.handle(), wake.clone(), empty.updater(), no_install, Limits::LAUNCH, relaunched);
        assert_eq!(settle(&mut launch), Step::Open);

        // offline: nobody listens
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = Url::parse(&format!("http://{}/latest.json", closed.local_addr().unwrap())).unwrap();
        drop(closed);
        let client = super::super::client_builder().no_proxy().build().unwrap();
        let offline = Updater::with_client(client).endpoints(vec![endpoint]);
        let mut launch = Launch::spawn(rt.handle(), wake.clone(), offline, no_install, Limits::LAUNCH, relaunched);
        assert_eq!(settle(&mut launch), Step::Open);

        // a bad signature
        let server = Server::start();
        let mut tampered = payload();
        tampered[5] ^= 1;
        server.route("/artifact", 200, tampered);
        server.release("9.0.0");
        let mut launch = Launch::spawn(rt.handle(), wake.clone(), server.updater(), no_install, Limits::LAUNCH, relaunched);
        assert_eq!(settle(&mut launch), Step::Open);
        assert_eq!(launch.shown(), Some(Shown::INSTALLING), "the splash had opened");

        // a download that stalls
        let server = Server::start();
        server.route("/artifact", 0, payload());
        server.release("9.0.0");
        let mut launch = Launch::spawn(rt.handle(), wake.clone(), server.updater(), no_install, Limits { check: CHECK_TIMEOUT, stall: Duration::from_millis(300) }, relaunched);
        assert_eq!(settle(&mut launch), Step::Open);
        assert_eq!(launch.shown(), Some(Shown::UPDATING));

        // a failed install
        let server = Server::start();
        server.route("/artifact", 200, payload());
        server.release("9.0.0");
        let failing = |_: String, _: Vec<u8>| Err(install::Error::TempDirNotOnSameMountPoint);
        let mut launch = Launch::spawn(rt.handle(), wake.clone(), server.updater(), failing, Limits::LAUNCH, relaunched);
        assert_eq!(settle(&mut launch), Step::Open);

        // installed, but the relaunch failed: the running version opens
        let ok = |_: String, _: Vec<u8>| Ok(Handover::Relaunch);
        let mut launch = Launch::spawn(rt.handle(), wake.clone(), server.updater(), ok, Limits::LAUNCH, not_relaunched);
        assert_eq!(settle(&mut launch), Step::Open);

        // Windows: the installer took over, so the app closes without a relaunch
        let installer = |_: String, _: Vec<u8>| Ok(Handover::Installer);
        let mut launch = Launch::spawn(rt.handle(), wake, server.updater(), installer, Limits::LAUNCH, not_relaunched);
        assert_eq!(settle(&mut launch), Step::Exit);
    }

    #[test]
    fn a_check_slower_than_its_time_limit_opens_the_app() {
        let rt = runtime();
        let server = Server::start();
        // the manifest's headers, then nothing
        server.route("/latest.json", 0, "{}");
        let no_install = |_: String, _: Vec<u8>| -> Result<Handover, install::Error> { panic!("nothing to install") };
        let limits = Limits { check: Duration::from_millis(300), stall: STALL };
        let started = Instant::now();
        let answer = rt.block_on(update(server.updater(), |_| {}, no_install, limits));
        assert_eq!(answer, Err("deadline has elapsed".to_owned()));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(Limits::LAUNCH, Limits { check: Duration::from_secs(8), stall: Duration::from_secs(60) });
    }

    /// The splash in a real window (X11): opened on the main window's device once the update is
    /// found, centred, moving at most 60 frames a second while the download comes, then the app
    /// closes after the relaunch. Run with `xvfb-run -a cargo test -p rustradar-app --lib
    /// update::splash -- --ignored` (winit needs `libxkbcommon-x11`).
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "opens windows: needs an X display (xvfb-run) and libxkbcommon-x11"]
    fn the_splash_in_a_window() {
        use winit::application::ApplicationHandler;
        use winit::event_loop::{EventLoop, EventLoopProxy};
        use winit::platform::x11::EventLoopBuilderExtX11;

        struct Harness {
            rt: tokio::runtime::Runtime,
            server: Server,
            proxy: EventLoopProxy<()>,
            main: Option<(Arc<Window>, Gpu)>,
            launch: Option<Launch>,
            started: Instant,
            opened_at: Option<Instant>,
            frames: usize,
            steps: Vec<Step>,
        }

        impl Harness {
            fn step(&mut self, event_loop: &ActiveEventLoop) {
                let Some(launch) = &mut self.launch else { return };
                let step = launch.poll(event_loop, self.main.as_ref().map(|m| &m.1));
                if launch.splash_open() && self.opened_at.is_none() {
                    self.opened_at = Some(Instant::now());
                    let splash = launch.splash.as_ref().unwrap();
                    let (at, size) = (splash.window.outer_position().unwrap(), splash.window.inner_size());
                    let monitor = event_loop.primary_monitor().unwrap().size();
                    assert_eq!((size.width, size.height), (380, 150));
                    assert_eq!((at.x, at.y), ((monitor.width as i32 - 380) / 2, (monitor.height as i32 - 150) / 2), "centred");
                }
                if step != Step::Wait {
                    self.steps.push(step);
                    event_loop.exit();
                }
            }
        }

        impl ApplicationHandler<()> for Harness {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                if self.main.is_some() {
                    return;
                }
                // the main window as the app makes it: hidden while the launch checks
                let window = Arc::new(event_loop.create_window(Window::default_attributes().with_visible(false)).unwrap());
                let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(
                    event_loop.owned_display_handle(),
                )));
                let surface = instance.create_surface(window.clone()).unwrap();
                let gpu = Gpu::for_surface(instance, &surface).unwrap();
                let proxy = self.proxy.clone();
                let wake: Wake = Arc::new(move || {
                    let _ = proxy.send_event(());
                });
                let install = |version: String, bytes: Vec<u8>| {
                    assert_eq!((version.as_str(), bytes.len()), ("9.0.0", 200_000));
                    Ok(Handover::Relaunch)
                };
                self.launch = Some(Launch::spawn(self.rt.handle(), wake, self.server.updater(), install, Limits::LAUNCH, relaunched));
                self.main = Some((window, gpu));
            }

            fn user_event(&mut self, event_loop: &ActiveEventLoop, (): ()) {
                self.step(event_loop);
            }

            fn window_event(&mut self, _: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
                if let (Some(launch), Some((main, gpu))) = (&mut self.launch, &self.main) {
                    if matches!(event, WindowEvent::RedrawRequested) && main.id() != id {
                        self.frames += 1;
                    }
                    assert!(launch.window_event(gpu, id, &event) || main.id() == id);
                }
            }

            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                self.step(event_loop);
                if let Some(launch) = &self.launch {
                    launch.about_to_wait(event_loop);
                }
                if self.started.elapsed() > Duration::from_secs(60) {
                    event_loop.exit();
                }
            }
        }

        let server = Server::start();
        server.route("/artifact", 200, payload());
        server.release("9.0.0");
        // eight pieces 250 ms apart: the splash shows the download for about two seconds
        server.pace(250);
        let event_loop = EventLoop::<()>::with_user_event().with_any_thread(true).build().unwrap();
        let mut harness = Harness {
            rt: runtime(),
            server,
            proxy: event_loop.create_proxy(),
            main: None,
            launch: None,
            started: Instant::now(),
            opened_at: None,
            frames: 0,
            steps: Vec::new(),
        };
        event_loop.run_app(&mut harness).unwrap();
        assert_eq!(harness.steps.first(), Some(&Step::Exit), "relaunched, then closed");
        let opened = harness.opened_at.expect("the splash opened");
        let shown_for = opened.elapsed().as_secs_f64();
        assert!(shown_for > 1.0, "{shown_for} s");
        assert!(harness.frames as f64 > shown_for * 20.0, "it moved: {} frames in {shown_for} s", harness.frames);
        assert!((harness.frames as f64) < shown_for * 64.0 + 2.0, "at most 60 a second: {} in {shown_for} s", harness.frames);
        assert_eq!(harness.launch.as_ref().unwrap().shown(), Some(Shown::INSTALLING));
    }

    #[test]
    fn debug_builds_and_updated_launches_check_nothing() {
        let rt = runtime();
        let (wake, _) = counting_wake();
        assert!(Launch::start(true, rt.handle(), wake.clone()).is_none(), "--updated");
        if !install::CHECKS {
            assert!(Launch::start(false, rt.handle(), wake).is_none(), "a debug build");
        }
    }
}
