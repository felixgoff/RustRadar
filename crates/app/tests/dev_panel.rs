//! Item U20's acceptance for the dev panel (plan stage 7; doc 06 §25): the panel as the window's UI
//! pass draws it, on SwiftShader, beside Chromium's layout of today's component (server-rendered by
//! Svelte, open, its rows, an opened call and the train tab built from its own markup, laid out
//! with app.css; the numbers in the tests are Chromium's).
//!
//! - `the_calls`: `target/shots/u20-dev-empty.png` (no calls yet), `u20-dev-calls.png` (four calls,
//!   the MET Norway one opened: its request and answer laid out), `u20-dev-calls-error.png` (the
//!   failed call opened), `u20-dev-hover.png` (a row under the pointer): the panel 562 wide at the
//!   right edge, the header's and the tools' rules, the rows and their rules where Chromium has
//!   them; the select, Pause, Clear, Copy and the close button answer.
//! - `the_selected_train`: `u20-dev-train.png`: the tab, Copy and the hint, the dump filling the
//!   rest; with a demo train its id and dump.
//! - `on_a_phone`: `u20-dev-phone.png` at 390 × 844: as wide as the window and its border, its left
//!   border out of the window, as today's.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes.

mod common;

use std::sync::{Arc, Mutex};

use common::shots::{gpu, pixel, save};
use egui::{Event, Pos2, RawInput, Rect, Vec2};
use rustradar_app::ui::dev_panel::{self, DevLog, Shown, Tab};
use rustradar_app::ui::theme;
use rustradar_backend::devlog::Entry;
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};

const DESKTOP: [u32; 2] = [1440, 900];
const PHONE: [u32; 2] = [390, 844];
/// 2026-06-21 10:00:00 UTC.
const NOW_MS: i64 = 1_782_036_000_000;

const INK: [u8; 3] = [30, 30, 30];
const SURFACE: [u8; 3] = [50, 50, 50];
const RAISED: [u8; 3] = [62, 62, 62];
const LINE: [u8; 3] = [68, 68, 68];
const CODE: [u8; 3] = [6, 6, 6];

/// The four calls of Chromium's pictures, oldest first.
fn calls() -> Vec<Entry> {
    let e = |id: u64, dt: i64, source: &str, method: &str, url: &str, status: Option<u16>, ms: u64, size: usize, body: &str, error: Option<&str>| Entry {
        id,
        at_ms: NOW_MS + dt,
        source: source.into(),
        method: method.into(),
        url: url.into(),
        request: None,
        status,
        ms,
        size,
        body: body.into(),
        error: error.map(Into::into),
    };
    let mut met = e(
        1,
        -2000,
        "MET Norway",
        "GET",
        "https://api.met.no/weatherapi/locationforecast/2.0/compact?lat=59.33&lon=18.07",
        Some(200),
        1204,
        1_363_149,
        r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[18.07,59.33,28]},"properties":{"meta":{"updated_at":"2026-06-21T09:30:12Z"}}}"#,
        None,
    );
    met.request = Some(
        r#"<REQUEST><LOGIN authenticationkey="•••"/><QUERY objecttype="TrainAnnouncement" schemaversion="1.9"><FILTER><EQ name="Advertised" value="true"/></FILTER></QUERY></REQUEST>"#.into(),
    );
    vec![
        met,
        e(2, 1000, "OpenRailwayMap", "GET", "https://tiles.openrailwaymap.org/standard/13/4410/2408.png", Some(404), 87, 9, "Not Found", None),
        e(
            3,
            5000,
            "VesselFinder",
            "GET",
            "https://www.vesselfinder.com/api/pub/vesselsonmap?bbox=11.6,57.5,12.3,57.9&zoom=13&mmsi=0&show_names=1",
            None,
            10_012,
            0,
            "",
            Some("error sending request: operation timed out"),
        ),
        e(4, 7000, "Trafikverket", "POST", "https://api.trafikinfo.trafikverket.se/v2/data.json", Some(200), 312, 49_357, "{}", None),
    ]
}

/// A log handing out its calls, counting the clears.
#[derive(Clone, Default)]
struct Fake(Arc<Mutex<(Vec<Entry>, usize)>>);

impl DevLog for Fake {
    fn since(&mut self, since: u64) -> Vec<Entry> {
        self.0.lock().unwrap().0.iter().filter(|e| e.id > since).cloned().collect()
    }

    fn clear(&mut self) {
        let mut s = self.0.lock().unwrap();
        s.0.clear();
        s.1 += 1;
    }
}

/// egui alone onto a target cleared to `--ink`.
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

    /// A frame 0.25 s after the last: what the panel answered and the clipboard's new text.
    fn frame(&mut self, gpu: &Gpu, events: Vec<Event>, shown: Shown<'_>) -> (dev_panel::Answer, Option<String>) {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(self.size[0] as f32, self.size[1] as f32))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        self.time += 0.25;
        let mut answer = dev_panel::Answer::default();
        let output = self.ctx.run_ui(raw, |ui| answer = dev_panel::show(ui.ctx(), shown));
        let copied = output.platform_output.commands.iter().find_map(|c| if let egui::OutputCommand::CopyText(t) = c { Some(t.clone()) } else { None });
        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: self.target.size(), pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui.update_texture(device, queue, id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("dev panel") });
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
        // SwiftShader's workers must be done before the process ends (exiting while they still
        // run the last frame crashes in them)
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        for id in textures.free.drain() {
            self.egui.free_texture(&id);
        }
        (answer, copied)
    }

    fn settle(&mut self, gpu: &Gpu, shown: Shown<'_>) -> Image {
        for _ in 0..3 {
            self.frame(gpu, Vec::new(), shown);
        }
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        self.target.read(gpu).expect("read back")
    }

    /// A click at `at`: what the panel answered and the clipboard's new text.
    fn click(&mut self, gpu: &Gpu, at: Pos2, shown: Shown<'_>) -> (dev_panel::Answer, Option<String>) {
        let press = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
        self.frame(gpu, vec![Event::PointerMoved(at), press(true)], shown);
        let out = self.frame(gpu, vec![press(false)], shown);
        self.frame(gpu, vec![Event::PointerGone], shown);
        out
    }

    /// `f` of the panel's state.
    fn with<R>(&self, f: impl FnOnce(&mut dev_panel::Panel) -> R) -> R {
        let p = dev_panel::panel(&self.ctx);
        let mut p = p.lock().unwrap();
        f(&mut p)
    }
}

fn open(now_ms: f64) -> Shown<'static> {
    Shown { open: true, dialog_open: false, now_ms, train: None, stops: None, route: None }
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

/// The rows of column `x` between `y0` and `y1` painted exactly `colour`.
fn rows_of(image: &Image, x: u32, y0: u32, y1: u32, colour: [u8; 3]) -> Vec<u32> {
    (y0..y1).filter(|&y| rgb(image, x, y) == colour).collect()
}

/// Each of `got` within a pixel of Chromium's row (the list's content starts on a whole pixel,
/// 74, where Chromium's starts at 73.797).
#[track_caller]
fn within_a_pixel(got: &[u32], chromium: &[u32], what: &str) {
    assert!(got.len() == chromium.len() && got.iter().zip(chromium).all(|(a, b)| a.abs_diff(*b) <= 1), "{what}: {got:?}, Chromium {chromium:?}");
}

/// The calls: empty, listed, opened, hovered; the controls answer.
#[test]
fn the_calls() {
    let Some(gpu) = gpu() else { return };
    let mut c = Canvas::new(&gpu, DESKTOP);
    let fake = Fake::default();
    dev_panel::install_log(&c.ctx, Box::new(fake.clone()));
    let image = c.settle(&gpu, open(0.0));
    save(&image, "u20-dev-empty.png");
    // Chromium: the box 878–1440 × 0–900, its border on the outer columns and rows
    near(&image, 877, 450, INK, 0);
    near(&image, 878, 450, LINE, 0);
    near(&image, 879, 450, SURFACE, 0);
    near(&image, 1439, 450, LINE, 0);
    // the header's rule on row 36, the tools' on row 73 (Chromium)
    assert_eq!(rows_of(&image, 1312, 30, 80, LINE), vec![36, 73], "the rules under the header and the tools");
    // the select (887–1007 for All sources), the field and the buttons on --surface-raised
    near(&image, 890, 55, RAISED, 0);
    near(&image, 1320, 55, RAISED, 0);
    assert!(c.with(|p| p.entries.is_empty()));

    // four calls: the next poll a second later brings them
    fake.0.lock().unwrap().0 = calls();
    let image = c.settle(&gpu, open(1500.0));
    assert_eq!(c.with(|p| p.entries.len()), 4);
    // the select as wide as OpenRailwayMap (Chromium 887–1029): its border at 1028
    near(&image, 1028, 50, LINE, 0);
    near(&image, 1030, 50, SURFACE, 0);
    // a rule under each call: Chromium 117.391, 161.984, 206.578, 251.172
    within_a_pixel(&rows_of(&image, 1430, 74, 260, LINE), &[117, 162, 207, 251], "the calls' rules");

    // the oldest (MET Norway, fourth) opened by a click on its row: request and answer laid out
    c.click(&gpu, Pos2::new(1200.0, 230.0), open(1600.0));
    assert_eq!(c.with(|p| p.expanded), Some(1));
    let image = c.settle(&gpu, open(1700.0));
    save(&image, "u20-dev-calls.png");
    // Chromium: the request block 281.969–428.344, the answer's 459.141–739.891, x 887–1431
    let code = rows_of(&image, 1100, 252, 760, CODE);
    let (first, last) = (code[0], code[code.len() - 1]);
    assert!(first.abs_diff(282) <= 1 && last.abs_diff(739) <= 1, "the blocks from {first} to {last}, Chromium 282–739");
    near(&image, 888, 300, CODE, 0);
    near(&image, 1430, 300, CODE, 0);
    // Copy puts the raw request on the clipboard (Chromium's first Copy at 944.75–992.328 × 257.172)
    let (_, copied) = c.click(&gpu, Pos2::new(968.0, 268.0), open(1800.0));
    assert_eq!(copied.as_deref(), calls()[0].request.as_deref());

    // the failed call opened instead: its error, an empty answer
    c.click(&gpu, Pos2::new(1200.0, 140.0), open(1900.0));
    assert_eq!(c.with(|p| p.expanded), Some(3));
    let image = c.settle(&gpu, open(2000.0));
    save(&image, "u20-dev-calls-error.png");
    // Chromium: the empty answer's block 209.578–221.578, the call's rule at 229.578
    let code = rows_of(&image, 1100, 160, 240, CODE);
    assert!(code[0].abs_diff(210) <= 1 && code.len().abs_diff(12) <= 1, "the empty block at {code:?}, Chromium 210–221");

    // a row under the pointer: --surface-raised
    c.click(&gpu, Pos2::new(1200.0, 140.0), open(2100.0));
    c.frame(&gpu, vec![Event::PointerMoved(Pos2::new(1200.0, 185.0))], open(2200.0));
    let image = c.settle(&gpu, open(2300.0));
    save(&image, "u20-dev-hover.png");
    near(&image, 1430, 185, RAISED, 0);
    near(&image, 1430, 140, SURFACE, 0);

    // the select's list: Trafikverket only
    c.click(&gpu, Pos2::new(950.0, 55.0), open(2400.0));
    let list_top = 55.0 + 12.0 + 2.0;
    let row = 16.8 + 4.0;
    // All sources, MET Norway, OpenRailwayMap, Trafikverket, VesselFinder
    c.click(&gpu, Pos2::new(950.0, list_top + row * 3.5), open(2500.0));
    assert_eq!(c.with(|p| p.source.clone()).as_deref(), Some("Trafikverket"));
    assert_eq!(c.with(|p| p.shown().len()), 1);
    // Pause, then Clear (Chromium 1315.063 and 1376.031)
    c.click(&gpu, Pos2::new(1340.0, 55.0), open(2600.0));
    assert!(c.with(|p| p.paused));
    c.click(&gpu, Pos2::new(1400.0, 55.0), open(2700.0));
    assert!(c.with(|p| p.entries.is_empty()));
    assert_eq!(fake.0.lock().unwrap().1, 1, "the backend's record cleared");
    // × closes (Chromium 1403.141–1431 × 7.391–29.391)
    let (answer, _) = c.click(&gpu, Pos2::new(1417.0, 18.0), open(2800.0));
    assert!(answer.close);
}

/// The selected train's tab.
#[test]
fn the_selected_train() {
    let Some(gpu) = gpu() else { return };
    let mut c = Canvas::new(&gpu, DESKTOP);
    dev_panel::install_log(&c.ctx, Box::new(Fake::default()));
    c.settle(&gpu, open(0.0));
    // Chromium's second tab at 1009.719–1131.219
    c.click(&gpu, Pos2::new(1070.0, 18.0), open(100.0));
    assert_eq!(c.with(|p| p.tab), Tab::Train);
    let image = c.settle(&gpu, open(200.0));
    save(&image, "u20-dev-train.png");
    // the open tab: --text-1 with --surface text
    near(&image, 1012, 10, [227, 227, 227], 0);
    // Chromium: the tools' rule at 81.594 (row 82), the dump 90.594–891 at x 887–1431
    assert_eq!(rows_of(&image, 1200, 30, 89, LINE), vec![36, 82], "the rules under the header and the tools");
    let code = rows_of(&image, 1200, 85, 900, CODE);
    assert!(code[0].abs_diff(91) <= 1 && code[code.len() - 1].abs_diff(890) <= 1, "the dump from {} to {}", code[0], code[code.len() - 1]);
    // Copy copies the dump
    let (_, copied) = c.click(&gpu, Pos2::new(910.0, 59.0), open(300.0));
    assert_eq!(copied.as_deref(), Some("{\n  \"train\": null,\n  \"stops\": null,\n  \"route\": null\n}"));
    // a demo train: its id in the hint, its dump
    let train = rustradar_app::dev::demo_trains(59.33, 18.07, NOW_MS).remove(0);
    let shown = Shown { train: Some(&train), ..open(400.0) };
    let (_, copied) = c.click(&gpu, Pos2::new(910.0, 59.0), shown);
    let copied = copied.expect("copied");
    assert!(copied.contains(&format!("\"id\": \"{}\"", train.id)), "{copied}");
    assert!(copied.contains(&format!("\"frames\": \"{} frames\"", train.frames.len())));
}

/// On a phone the panel is as wide as the window, its left border just out of it.
#[test]
fn on_a_phone() {
    let Some(gpu) = gpu() else { return };
    let mut c = Canvas::new(&gpu, PHONE);
    let fake = Fake::default();
    fake.0.lock().unwrap().0 = calls();
    dev_panel::install_log(&c.ctx, Box::new(fake));
    let image = c.settle(&gpu, open(0.0));
    save(&image, "u20-dev-phone.png");
    near(&image, 0, 450, SURFACE, 0);
    near(&image, 389, 450, LINE, 0);
    assert_eq!(rows_of(&image, 262, 30, 80, LINE), vec![36, 73]);
    within_a_pixel(&rows_of(&image, 380, 74, 260, LINE), &[117, 162, 207, 251], "the calls' rules");
}
