//! Item U7's acceptance (plan stage 7; doc 06 §2.6, §2.7, §6.2): the mode bar, the attribution
//! strip and the dev readout as the UI pass draws them.
//!
//! - `u7-modebar-desktop.png` (1440 × 900): today's dark-v1 shell, the aircraft the focus and no
//!   trains key: the bar's border, its chosen option's accent and the ink of both options, the
//!   strip's box and text, and the readout's box where `docs/port/reference/dark-v1-1440x900.png`
//!   has them;
//! - `u7-modebar-counts.png` (1100 × 720): all three kinds offered with counts, the trains also on
//!   the map, the pointer on the bar (the eyes unfolded): the bar centred between x 352 and
//!   width − 352, the shown kind's eye in the accent, a train credit underlined first in the strip;
//! - `u7-modebar-v1.png`: through the window's engine over the V1 globe.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::NOW_MS;
use common::shots::{Rect, bounds, gpu, pixel, save};
use egui::{Color32, Event, Pos2, RawInput, Vec2};
use rustradar_app::app::Engine;
use rustradar_app::state::{Action, AppState, Startup};
use rustradar_app::ui::attribution::{self, Attribution, Conditions, Place};
use rustradar_app::ui::modebar::{self, Bar};
use rustradar_app::ui::theme;
use rustradar_core::geo::Basemap;
use rustradar_core::settings::{Mode, ShownKinds};
use rustradar_core::trains::{Credit, MODEL_CREDIT};
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_map::io::{self, DirTiles};
use rustradar_scene::camera::ViewState;

/// The desktop reference pictures' canvas, CSS px, at scale 1.
const SIZE: [u32; 2] = [1440, 900];

/// egui alone onto a target cleared to `--ink` (the map's space colour), as the window's UI pass
/// draws it.
struct Canvas {
    ctx: egui::Context,
    egui: egui_wgpu::Renderer,
    target: Offscreen,
    size: [u32; 2],
}

impl Canvas {
    fn new(gpu: &Gpu, size: [u32; 2]) -> Self {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let egui = egui_wgpu::Renderer::new(&gpu.device, FORMAT, egui_wgpu::RendererOptions::default());
        Canvas { ctx, egui, target: Offscreen::new(gpu, size), size }
    }

    /// A frame of `draw` at egui time `time` with `events`; answers the actions it asked for.
    fn frame(&mut self, gpu: &Gpu, time: f64, events: Vec<Event>, draw: &dyn Fn(&egui::Context, &mut Vec<Action>)) -> Vec<Action> {
        let raw = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(self.size[0] as f32, self.size[1] as f32))),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut actions = Vec::new();
        let output = self.ctx.run_ui(raw, |ui| draw(ui.ctx(), &mut actions));
        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: self.target.size(), pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui.update_texture(device, queue, id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("modebar") });
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
        actions
    }

    fn read(&self, gpu: &Gpu) -> Image {
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        self.target.read(gpu).expect("read back")
    }
}

fn rgb(c: Color32) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

fn lum(p: [u8; 4]) -> f64 {
    (f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])) / 3.0
}

/// The rectangle of pixels whose colour is within `tolerance` levels of `c` in `area`.
fn painted(image: &Image, area: Rect, c: Color32, tolerance: u8) -> Option<Rect> {
    bounds(image, area, |p| (0..3).all(|i| p[i].abs_diff(rgb(c)[i]) <= tolerance))
}

/// A reference picture (8-bit RGB, or a palette) as RGBA.
fn reference(name: &str) -> Image {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference").join(name);
    let file = std::io::BufReader::new(std::fs::File::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())));
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().expect("a PNG");
    let mut buf = vec![0; reader.output_buffer_size().expect("a size")];
    let info = reader.next_frame(&mut buf).expect("a frame");
    let channels = info.color_type.samples();
    let rgba = buf[..info.buffer_size()].chunks_exact(channels).flat_map(|p| [p[0], p[1], p[2], if channels == 4 { p[3] } else { 255 }]).collect();
    Image { width: info.width, height: info.height, rgba }
}

/// `ours` and `theirs` within `dx` across and `dy` down on every side.
#[track_caller]
fn same_box(what: &str, ours: Rect, theirs: Rect, dx: u32, dy: u32) {
    let ok = ours.x0.abs_diff(theirs.x0) <= dx && ours.x1.abs_diff(theirs.x1) <= dx && ours.y0.abs_diff(theirs.y0) <= dy && ours.y1.abs_diff(theirs.y1) <= dy;
    assert!(ok, "{what}: {ours:?}, today {theirs:?}");
}

/// The bar of dark-v1: the aircraft the focus, no trains key, nothing loaded.
fn today() -> Bar {
    Bar {
        mode: Mode::Aircraft,
        shown: ShownKinds { aircraft: true, trains: false, ships: false },
        offered: ShownKinds { aircraft: true, trains: false, ships: true },
        counts: [None; 3],
    }
}

/// dark-v1's readout: V1, the dark map, SwiftShader's 0 fps and nothing loaded.
fn today_readout() -> [String; 4] {
    attribution::readout_lines(&ViewState { latitude: 35.0, longitude: 10.0, zoom: 1.9, ..ViewState::INITIAL }, Basemap::Dark, 0.0, [0, 0, 0])
}

#[test]
fn the_bar_the_strip_and_the_readout_where_today_s_are() {
    let Some(gpu) = gpu() else { return };
    let mut canvas = Canvas::new(&gpu, SIZE);
    let line = Attribution { trains: vec![], credits: attribution::credits(&Conditions { aircraft_on: true, ..Default::default() }) };
    let readout = today_readout();
    let mut placed = (egui::Rect::NOTHING, egui::Rect::NOTHING, egui::Rect::NOTHING);
    let placed_at = std::cell::Cell::new(placed);
    let draw = |ctx: &egui::Context, actions: &mut Vec<Action>| {
        let bar = modebar::show(ctx, &today(), actions);
        let strip = attribution::show(ctx, &line, Place::Desktop);
        let box_ = attribution::readout(ctx, &readout);
        placed_at.set((bar, strip, box_));
    };
    // areas are sized on their first frame
    for time in [0.0, 1.0, 2.0] {
        canvas.frame(&gpu, time, Vec::new(), &draw);
    }
    placed = placed_at.get();
    let image = canvas.read(&gpu);
    save(&image, "u7-modebar-desktop.png");
    let today = reference("dark-v1-1440x900.png");

    // the bar: --line border round --surface, centred on 720 between 352 and 1088
    let (bar, _, _) = placed;
    assert!((bar.center().x - 720.0).abs() < 0.01 && bar.top() == 16.0, "{bar:?}");
    let area = Rect::new(560, 0, 320, 80);
    let border = |image: &Image| painted(image, area, theme::LINE, 3).expect("the bar's border");
    same_box("the bar's border", border(&image), border(&today), 1, 0);
    assert_eq!(pixel(&image, 720, 18), rgb(theme::SURFACE), "its fill");
    // the chosen option's accent, 3 px inside the border
    let accent = |image: &Image| painted(image, area, theme::AMBER, 2).expect("the accent");
    same_box("the chosen option", accent(&image), accent(&today), 1, 0);
    // the plane and "Aircraft" in white on it, the ship and "Ships" in --text-2 beside it
    let (ours_accent, theirs_accent) = (accent(&image), accent(&today));
    let white = |image: &Image, a: Rect| bounds(image, a, |p| p[0] > 120 && p[2] > 200).expect("white ink");
    same_box("Aircraft's ink", white(&image, ours_accent), white(&today, theirs_accent), 1, 1);
    let grey = |image: &Image, a: Rect| {
        let right = Rect { x0: a.x1 + 1, y0: a.y0, x1: 806, y1: a.y1 };
        bounds(image, right, |p| lum(p) > 80.0).expect("grey ink")
    };
    same_box("Ships's ink", grey(&image, ours_accent), grey(&today, theirs_accent), 2, 1);

    // the strip: oklch(0.15 0 0 / 0.6) over the map along the bottom right, its text --text-3
    let (_, strip, _) = placed;
    assert_eq!((strip.right(), strip.bottom()), (1440.0, 900.0));
    let corner = Rect::new(800, 886, 640, 14);
    let bg = |image: &Image| bounds(image, corner, |p| lum(p) < 22.0).expect("the strip");
    same_box("the strip", bg(&image), bg(&today), 3, 1);
    assert!((lum(pixel(&image, 1438, 898)) - 19.0).abs() <= 1.0, "the strip's fill: {:?}", pixel(&image, 1438, 898));
    let words = |image: &Image| bounds(image, corner, |p| lum(p) > 60.0).expect("the strip's text");
    same_box("the strip's text", words(&image), words(&today), 4, 1);

    // the readout: top 8, right 8, oklch(0.15 0 0 / 0.75), four lines of --text-2
    let (_, _, readout_box) = placed;
    assert_eq!((readout_box.right(), readout_box.top()), (1432.0, 8.0));
    let top_right = Rect::new(1100, 0, 340, 100);
    let dark = |image: &Image| bounds(image, top_right, |p| lum(p) < 20.0).expect("the readout");
    // Overpass Mono's 0.616 em against the system monospace's 0.602: a little wider to the left
    let (ours, theirs) = (dark(&image), dark(&today));
    same_box("the readout's right, top and bottom", Rect { x0: theirs.x0, ..ours }, theirs, 1, 1);
    assert!(ours.x0 <= theirs.x0 && theirs.x0 - ours.x0 <= 6, "the readout's left: {ours:?}, today {theirs:?}");
    let rows = |image: &Image, b: Rect| (b.y0..b.y1).filter(|&y| (b.x0..b.x1).any(|x| lum(pixel(image, x, y)) > 60.0)).collect::<Vec<u32>>();
    let (ours_rows, theirs_rows) = (rows(&image, ours), rows(&today, theirs));
    // four lines of text 13.5 apart: the same rows within a pixel
    let runs = |r: &[u32]| r.windows(2).filter(|w| w[1] != w[0] + 1).count() + 1;
    assert_eq!((runs(&ours_rows), runs(&theirs_rows)), (4, 4), "{ours_rows:?} today {theirs_rows:?}");
    assert!(ours_rows[0].abs_diff(theirs_rows[0]) <= 1 && ours_rows.last().unwrap().abs_diff(*theirs_rows.last().unwrap()) <= 1, "{ours_rows:?} today {theirs_rows:?}");
}

#[test]
fn the_bar_with_counts_and_eyes_centred_between_352_and_w_minus_352() {
    let Some(gpu) = gpu() else { return };
    let size = [1100, 720];
    let mut canvas = Canvas::new(&gpu, size);
    let bar = Bar {
        mode: Mode::Aircraft,
        shown: ShownKinds { aircraft: true, trains: true, ships: false },
        offered: ShownKinds { aircraft: true, trains: true, ships: true },
        counts: [Some(1443), Some(12_345), None],
    };
    let trafikverket = Credit { text: "Trains in Sweden: Trafikverket, CC0", url: "https://data.trafikverket.se/" };
    let line = Attribution {
        trains: vec![trafikverket, MODEL_CREDIT],
        credits: attribution::credits(&Conditions { aircraft_on: true, ..Default::default() }),
    };
    let placed = std::cell::Cell::new((egui::Rect::NOTHING, egui::Rect::NOTHING));
    let draw = |ctx: &egui::Context, actions: &mut Vec<Action>| {
        let b = modebar::show(ctx, &bar, actions);
        let s = attribution::show(ctx, &line, Place::Desktop);
        placed.set((b, s));
    };
    for time in [0.0, 0.5] {
        canvas.frame(&gpu, time, Vec::new(), &draw);
    }
    let folded = placed.get().0;
    // the pointer on the bar: both eyes unfold over 150 ms
    let on_bar = Pos2::new(folded.left() + 10.0, folded.center().y);
    canvas.frame(&gpu, 1.0, vec![Event::PointerMoved(on_bar)], &draw);
    for time in [1.5, 2.0] {
        canvas.frame(&gpu, time, Vec::new(), &draw);
    }
    let (rect, strip) = placed.get();
    let image = canvas.read(&gpu);
    save(&image, "u7-modebar-counts.png");

    // centred between 352 and 1100 − 352, 24 px wider than with the pointer away (the ships' eye
    // unfolded; the trains' is on, so open already)
    assert!((rect.center().x - 550.0).abs() < 0.01, "{rect:?}");
    assert!((rect.width() - folded.width() - 24.0).abs() < 0.01, "{folded:?} → {rect:?}");
    let border = painted(&image, Rect::new(0, 0, 1100, 58), theme::LINE, 3).expect("the bar's border");
    let middle = f64::from(border.x0 + border.x1) / 2.0;
    assert!((middle - 550.0).abs() <= 1.0, "the bar's pixels: {border:?}");
    // 398.6 px in a wrapper of 396: it overflows both sides alike (`justify-content: center`)
    assert!((i64::from(border.x0) - 352).abs_diff(i64::from(1100 - 352) - i64::from(border.x1)) <= 1, "{border:?}");
    // the trains' eye is on: the accent's Eye at the trains option's right, on --surface
    let accent = painted(&image, Rect::new(border.x0, 16, border.width(), 40), theme::AMBER, 2).expect("the chosen option");
    let eyes = bounds(&image, Rect::new(accent.x1 + 1, 18, border.x1 - accent.x1 - 1, 34), |p| p[2] > 180 && p[0] < 110 && p[1] > 100).expect("the trains' eye");
    assert!(eyes.width() >= 10 && eyes.width() <= 15, "one eye in the accent: {eyes:?}");
    // the trains' option reads --text-1 (shown with the focus), its count --text-3
    let trains_ink = bounds(&image, Rect::new(accent.x1 + 1, 20, eyes.x0 - accent.x1 - 8, 30), |p| lum(p) > 200.0).expect("the trains' name");
    assert!(trains_ink.width() > 30, "{trains_ink:?}");

    // the strip: the train credits first, underlined in white at 25 % over its fill (19 → 78)
    let (left, top) = (strip.left() as u32, strip.top() as u32);
    let underline = (top..720).find(|&y| (left + 4..left + 150).filter(|&x| (lum(pixel(&image, x, y)) - 78.0).abs() <= 4.0).count() > 120);
    let y = underline.expect("the first credit's underline");
    let run = (left..left + 200).filter(|&x| (lum(pixel(&image, x, y)) - 78.0).abs() <= 4.0).min();
    assert_eq!(run, Some(left + 4), "from the first credit's first letter");
    let text = bounds(&image, Rect::new(left, top, 150, 720 - top), |p| lum(p) > 120.0).expect("the first credit");
    // 2 px under the baseline: the line just below the letters' feet
    assert!(y >= text.y1 && y <= text.y1 + 3, "underline at {y}, letters {text:?}");
}

/// The bar and the strip drawn by the window's engine (the shell on) over the V1 globe, and a
/// click on Ships through its frames makes them the focus.
#[test]
fn through_the_window() {
    let Some(gpu) = gpu() else { return };
    let state = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, ViewState::INITIAL, SIZE.map(f64::from));
    let mut engine = Engine::new(&gpu, FORMAT, state);
    engine.map.set_tiles(Arc::new(DirTiles::fixtures()), None);
    engine.shell = true;
    engine.draw = false;
    let target = Offscreen::new(&gpu, SIZE);
    let mut egui_s = 0.0;
    let mut frame = |engine: &mut Engine, events: Vec<Event>, now: f64| {
        let mut input = RawInput {
            time: Some(egui_s),
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(SIZE[0] as f32, SIZE[1] as f32))),
            events,
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(1.0);
        egui_s += 0.25;
        engine.frame(&gpu, target.view(), SIZE, 1.0, input, now);
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    };
    let started = std::time::Instant::now();
    loop {
        frame(&mut engine, Vec::new(), NOW_MS);
        if !engine.map.pending() || started.elapsed() > rustradar_map::headless::MAX_WAIT {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    frame(&mut engine, Vec::new(), NOW_MS);
    engine.draw = true;
    frame(&mut engine, Vec::new(), NOW_MS);
    engine.draw = false;
    let image = target.read(&gpu).expect("read back");
    save(&image, "u7-modebar-v1.png");
    let today = reference("dark-v1-1440x900.png");
    let area = Rect::new(560, 0, 320, 80);
    let border = |image: &Image| painted(image, area, theme::LINE, 3).expect("the bar's border");
    same_box("the bar's border", border(&image), border(&today), 1, 0);
    // the strip over the map: the map's credit, nothing else of today's (aircraft are on)
    let corner = Rect::new(800, 880, 640, 20);
    let words = bounds(&image, corner, |p| lum(p) > 60.0).expect("the strip's text");
    assert!(words.x1 >= 1434 && words.x0 < 1000, "{words:?}");
    if cfg!(debug_assertions) {
        assert!(lum(pixel(&image, 1430, 12)) < 22.0, "the readout's box: {:?}", pixel(&image, 1430, 12));
    }

    // a click on Ships: the focus moves
    let ships = Pos2::new(790.0, 35.0);
    let button = |pressed| Event::PointerButton { pos: ships, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
    engine.map.set_tiles(io::offline(), None);
    frame(&mut engine, vec![Event::PointerMoved(ships)], NOW_MS + 16.0);
    frame(&mut engine, vec![Event::PointerMoved(ships)], NOW_MS + 32.0);
    assert!(engine.egui.is_pointer_over_egui(), "over the bar");
    frame(&mut engine, vec![button(true)], NOW_MS + 48.0);
    frame(&mut engine, vec![button(false)], NOW_MS + 64.0);
    assert_eq!(engine.state.persisted.settings.mode, Mode::Ships);
    // beside the bar the pointer is the map's
    frame(&mut engine, vec![Event::PointerMoved(Pos2::new(900.0, 35.0))], NOW_MS + 80.0);
    frame(&mut engine, Vec::new(), NOW_MS + 96.0);
    assert!(!engine.egui.is_pointer_over_egui(), "over the map");
}
