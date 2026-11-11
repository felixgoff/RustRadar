//! Item U20's acceptance for the update toast (plan stage 7; doc 06 §22): the toast as the window's
//! UI pass draws it, on SwiftShader, beside Chromium's layout of today's markup (UpdateToast's
//! template and scoped styles with app.css and the fonts; the numbers in the tests are
//! Chromium's).
//!
//! - `the_toast`: `target/shots/u20-toast.png` (1440 × 900): the card at left 16, bottom 40, as wide
//!   as its one-row text, the amber icon, the primary button and the close button; under the
//!   pointer the button lightens (`u20-toast-hover.png`).
//! - `restart_and_its_failure`: Restart disables itself as `Restarting…` at 0.7
//!   (`u20-toast-restarting.png`); the updater's failure comes back as two rows in the 440 px cap,
//!   the close button shrunk as Chromium shrinks it (`u20-toast-error.png`); Later hides it.
//! - `android_on_a_phone`: `Download` and its note at 390 × 844, the card reaching the window's
//!   right edge as today's does (`u20-toast-android.png`).
//! - `the_toast_through_the_window`: through the window's engine with the shell: the toast over the
//!   sidebar, kept on top after a click on the sidebar (`u20-toast-window.png`).
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes.

mod common;

use std::sync::mpsc;

use common::shots::{gpu, pixel, save};
use egui::{Event, Pos2, RawInput, Rect, Vec2};
use rustradar_app::app::Engine;
use rustradar_app::state::aircraft::Answer;
use rustradar_app::state::shortcuts::Open;
use rustradar_app::state::{Action, AppState, Startup};
use rustradar_app::ui::theme;
use rustradar_app::ui::toast::{self, Shown, Updater};
use rustradar_core::geo::Basemap;
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_map::io;
use rustradar_scene::camera::ViewState;

const DESKTOP: [u32; 2] = [1440, 900];
const PHONE: [u32; 2] = [390, 844];
const VERSION: &str = "0.4.0";
const ERROR: &str = "Error: Couldn't install the update: the download's signature does not match.";

const INK: [u8; 3] = [30, 30, 30];
const SURFACE: [u8; 3] = [50, 50, 50];
const LINE: [u8; 3] = [68, 68, 68];
const AMBER: [u8; 3] = [55, 142, 240];

/// egui alone onto a target cleared to `--ink`, as the window's UI pass draws it.
struct Canvas {
    ctx: egui::Context,
    egui: egui_wgpu::Renderer,
    target: Offscreen,
    size: [u32; 2],
    time: f64,
}

impl Canvas {
    fn new(gpu: &Gpu, size: [u32; 2]) -> Self {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let egui = egui_wgpu::Renderer::new(&gpu.device, FORMAT, egui_wgpu::RendererOptions::default());
        Canvas { ctx, egui, target: Offscreen::new(gpu, size), size, time: 0.0 }
    }

    /// A frame of the toast as `shown` says, 0.25 s after the last: the card's rectangle.
    fn frame(&mut self, gpu: &Gpu, events: Vec<Event>, shown: Shown<'_>) -> Option<Rect> {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(self.size[0] as f32, self.size[1] as f32))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        self.time += 0.25;
        let mut card = None;
        let output = self.ctx.run_ui(raw, |ui| card = toast::show(ui.ctx(), shown));
        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: self.target.size(), pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui.update_texture(device, queue, id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("toast") });
        let mut commands = self.egui.update_buffers(device, queue, &mut encoder, &primitives, &screen);
        {
            let [r, g, b, _] = theme::INK.to_array().map(|c| f64::from(c) / 255.0);
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: self.target.view(),
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }), store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.egui.render(&mut pass, &primitives, &screen);
        }
        commands.push(encoder.finish());
        queue.submit(commands);
        for id in textures.free.drain() {
            self.egui.free_texture(&id);
        }
        card
    }

    /// Four quiet frames (the hover's 150 ms over), then the picture.
    fn settle(&mut self, gpu: &Gpu, shown: Shown<'_>) -> Image {
        for _ in 0..4 {
            self.frame(gpu, Vec::new(), shown);
        }
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        self.target.read(gpu).expect("read back")
    }

    /// A click at `at` (moved there first), over two frames.
    fn click(&mut self, gpu: &Gpu, at: Pos2, shown: Shown<'_>) {
        let press = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
        self.frame(gpu, vec![Event::PointerMoved(at), press(true)], shown);
        self.frame(gpu, vec![press(false)], shown);
    }
}

fn desktop(version: Option<&str>) -> Shown<'_> {
    Shown { version, android: false, dialog_open: false, dev_open: false }
}

fn rgb(image: &Image, x: u32, y: u32) -> [u8; 3] {
    let [r, g, b, _] = pixel(image, x, y);
    [r, g, b]
}

#[track_caller]
fn near(image: &Image, x: u32, y: u32, expected: [u8; 3], tolerance: u8) {
    let got = rgb(image, x, y);
    assert!(got.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= tolerance), "({x}, {y}): {got:?}, Chromium {expected:?}");
}

/// The rows where the colour at column `x` changes between `y0` and `y1`.
fn edges(image: &Image, x: u32, y0: u32, y1: u32) -> Vec<(u32, [u8; 3])> {
    let mut out = Vec::new();
    let mut last = None;
    for y in y0..y1 {
        let p = rgb(image, x, y);
        if last != Some(p) {
            out.push((y, p));
            last = Some(p);
        }
    }
    out
}

/// The card's right border column along row `y`: the last `--line` pixel before the ink.
fn right_border(image: &Image, y: u32) -> u32 {
    (0..image.width).rev().find(|&x| rgb(image, x, y) == LINE).expect("a border")
}

/// An updater that answers what it is handed, or never.
struct Canned(Option<Result<(), String>>);

impl Updater for Canned {
    fn restart_to_update(&mut self) -> Answer<Result<(), String>> {
        let (tx, rx) = mpsc::channel();
        if let Some(answer) = self.0.take() {
            tx.send(answer).unwrap();
        } else {
            // never answers while the test runs
            std::mem::forget(tx);
        }
        rx.into()
    }
}

/// The plain offer at 1440 × 900 and its button under the pointer.
#[test]
fn the_toast() {
    let Some(gpu) = gpu() else { return };
    let mut c = Canvas::new(&gpu, DESKTOP);
    let shown = desktop(Some(VERSION));
    let image = c.settle(&gpu, shown);
    save(&image, "u20-toast.png");
    // Chromium: the card 799.016–860 snapped to rows 799–859, its border on the outer rows
    assert_eq!(edges(&image, 24, 700, 900), vec![(700, INK), (799, LINE), (800, SURFACE), (859, LINE), (860, INK)]);
    near(&image, 15, 830, INK, 0);
    near(&image, 16, 830, LINE, 0);
    near(&image, 17, 830, SURFACE, 0);
    // Chromium: 433.688 wide (its right border at x 449), the sub line on one row
    let right = right_border(&image, 830);
    assert!(right.abs_diff(449) <= 1, "right border at {right}, Chromium 449");
    // the primary button (Chromium 322.719–392.688 × 813.5–845.5) filled with the accent
    near(&image, 325, 816, AMBER, 0);
    near(&image, 390, 843, AMBER, 0);
    near(&image, 321, 829, SURFACE, 0);
    // the icon's stroke in the accent somewhere in its 16 px at x 33, y 821.5
    let icon = (33..49).flat_map(|x| (822..838).map(move |y| (x, y))).filter(|&(x, y)| {
        let [r, g, b] = rgb(&image, x, y);
        b > 150 && b > r + 60 && g > r
    });
    assert!(icon.count() > 20, "the RefreshCw icon in --amber");
    // the close button's X in --text-2 inside 404.688–436.688
    let ink = (412..429).flat_map(|x| (822..838).map(move |y| (x, y))).filter(|&(x, y)| rgb(&image, x, y)[0] > 120).count();
    assert!(ink > 15, "the X drawn: {ink} px");

    // under the pointer: oklch(from --amber calc(l + 0.05) c h), clipped as Chromium draws it
    c.frame(&gpu, vec![Event::PointerMoved(Pos2::new(357.0, 829.0))], shown);
    let image = c.settle(&gpu, shown);
    save(&image, "u20-toast-hover.png");
    near(&image, 325, 816, [72, 158, 255], 1);
}

/// Restart, its failure and Later.
#[test]
fn restart_and_its_failure() {
    let Some(gpu) = gpu() else { return };
    let mut c = Canvas::new(&gpu, DESKTOP);
    let shown = desktop(Some(VERSION));
    toast::install_updater(&c.ctx, Box::new(Canned(None)));
    c.settle(&gpu, shown);
    c.click(&gpu, Pos2::new(357.0, 829.0), shown);
    c.frame(&gpu, vec![Event::PointerGone], shown);
    let image = c.settle(&gpu, shown);
    save(&image, "u20-toast-restarting.png");
    {
        let t = toast::toast(&c.ctx);
        let t = t.lock().unwrap();
        assert!(t.restarting);
        assert_eq!(t.button_label(false), toast::RESTARTING);
    }
    // disabled at 0.7 over --surface: Chromium (53, 114, 183)
    near(&image, 325, 816, [53, 114, 183], 2);
    // Chromium: 464.938 wide with the longer label
    let right = right_border(&image, 830);
    assert!(right.abs_diff(480) <= 1, "right border at {right}, Chromium 480");
    // a click on the disabled button asks nothing more
    c.click(&gpu, Pos2::new(357.0, 829.0), shown);

    // the failure: a fresh toast whose updater answers at once
    drop(c);
    let mut c = Canvas::new(&gpu, DESKTOP);
    toast::install_updater(&c.ctx, Box::new(Canned(Some(Err(ERROR.into())))));
    c.settle(&gpu, shown);
    c.click(&gpu, Pos2::new(357.0, 829.0), shown);
    c.frame(&gpu, vec![Event::PointerGone], shown);
    let image = c.settle(&gpu, shown);
    save(&image, "u20-toast-error.png");
    {
        let t = toast::toast(&c.ctx);
        let t = t.lock().unwrap();
        assert!(!t.restarting, "enabled again");
        assert_eq!(t.sub(false), "Couldn't install the update: the download's signature does not match.");
    }
    // Chromium: two rows, the card 782.219–860 (rows 782–859), 470 wide (440 of content)
    assert_eq!(edges(&image, 24, 700, 900), vec![(700, INK), (782, LINE), (783, SURFACE), (859, LINE), (860, INK)]);
    assert_eq!(right_border(&image, 820), 485, "the content capped at 440");
    // the close button shrunk to 25.391 (447.609–473): its X centred at 460.3
    let xs: Vec<u32> = (440..480).filter(|&x| (814..830).any(|y| rgb(&image, x, y)[0] > 120)).collect();
    let centre = (xs[0] + xs[xs.len() - 1]) as f32 / 2.0;
    assert!((centre - 460.3).abs() <= 1.0, "the X centred at {centre}, Chromium 460.3");
    // the button where Chromium has it, still enabled
    near(&image, 368, 808, AMBER, 0);
    near(&image, 363, 808, SURFACE, 0);

    // Later hides it for this version, and a newer one shows again
    let card = c.frame(&gpu, Vec::new(), shown).expect("shown");
    c.click(&gpu, Pos2::new(460.0, card.center().y), shown);
    assert_eq!(c.frame(&gpu, Vec::new(), shown), None, "dismissed");
    assert!(c.frame(&gpu, Vec::new(), desktop(Some("0.4.1"))).is_some(), "a newer version shows it again");
}

/// Android's wording on a 390 × 844 phone.
#[test]
fn android_on_a_phone() {
    let Some(gpu) = gpu() else { return };
    let mut c = Canvas::new(&gpu, PHONE);
    let shown = Shown { android: true, ..desktop(Some(VERSION)) };
    let image = c.settle(&gpu, shown);
    save(&image, "u20-toast-android.png");
    // Chromium: two rows, the card 726.219–804 (rows 726–803), 16 to the window's right edge
    assert_eq!(edges(&image, 24, 650, 844), vec![(650, INK), (726, LINE), (727, SURFACE), (803, LINE), (804, INK)]);
    assert_eq!(right_border(&image, 760), 389);
    // Download at 253.453–338.141
    near(&image, 256, 752, AMBER, 0);
    near(&image, 336, 778, AMBER, 0);
    let t = toast::toast(&c.ctx);
    let mut t = t.lock().unwrap();
    assert_eq!((t.button_label(true), t.sub(true)), (toast::DOWNLOAD, toast::SUB_ANDROID));
    t.restart(true);
    assert!(!t.restarting, "Android's button stays enabled");
}

/// Through the window's engine: the toast over the bottom of the sidebar's column (the filter panel
/// open), still on top after the sidebar is clicked; under the keys dialog's backdrop while it is
/// open.
#[test]
fn the_toast_through_the_window() {
    let Some(gpu) = gpu() else { return };
    let now = common::NOW_MS;
    let mut state = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, ViewState::INITIAL, DESKTOP.map(f64::from));
    state.update_ready = Some(VERSION.into());
    // the filter panel open: the sidebar's column reaches 40 above the bottom, under the toast
    state.apply(Action::Open(Open::Filters, true), now);
    let mut engine = Engine::new(&gpu, FORMAT, state);
    engine.map.set_tiles(io::offline(), None);
    engine.shell = true;
    let target = Offscreen::new(&gpu, DESKTOP);
    let mut egui_s = 0.0;
    let mut frame = |engine: &mut Engine, events: Vec<Event>, t: f64| {
        let mut input = RawInput {
            time: Some(egui_s),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(DESKTOP[0] as f32, DESKTOP[1] as f32))),
            events,
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(1.0);
        egui_s += 0.25;
        engine.frame(&gpu, target.view(), DESKTOP, 1.0, input, now + t);
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    };
    for t in 0..4 {
        frame(&mut engine, Vec::new(), f64::from(t) * 100.0);
    }
    let image = target.read(&gpu).expect("read back");
    assert_ne!(rgb(&image, 24, 790), INK, "the sidebar's column behind the toast's top edge");
    // a click on the sidebar's card (which comes to the top of its layer)
    let at = Pos2::new(200.0, 60.0);
    let press = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
    frame(&mut engine, vec![Event::PointerMoved(at), press(true)], 500.0);
    frame(&mut engine, vec![press(false)], 600.0);
    for t in 0..3 {
        frame(&mut engine, Vec::new(), 700.0 + f64::from(t) * 100.0);
    }
    let image = target.read(&gpu).expect("read back");
    save(&image, "u20-toast-window.png");
    near(&image, 325, 816, AMBER, 0);
    assert_eq!(edges(&image, 24, 790, 805), vec![(790, rgb(&image, 24, 790)), (799, LINE), (800, SURFACE)], "the toast's top edge over the sidebar");

    // the keys dialog: the toast under its backdrop, dimmed
    engine.state.apply(Action::Open(Open::Keys, true), now + 1000.0);
    for t in 0..4 {
        frame(&mut engine, Vec::new(), 1100.0 + f64::from(t) * 100.0);
    }
    let image = target.read(&gpu).expect("read back");
    let [r, g, b] = rgb(&image, 325, 816);
    assert!(b < 150 && b > r, "the button under the backdrop: ({r}, {g}, {b})");
}
