//! Item U18's acceptance (plan stage 7; doc 06 §18.1, doc 03 §10.4): the hover card from fixture
//! cards, on SwiftShader, beside Chromium's layout of today's `HoverCard.svelte` (server-rendered
//! by Svelte with the same props, laid out with app.css at 1440 × 900, transitions off, the photo
//! and logo served from the same pictures; the numbers in the tests are Chromium's boxes).
//!
//! - `the_card_beside_chromium`: `target/shots/u18-card-{loading,ready,plain,bare}.png`: the photo
//!   slot (open and flat while loading; the picture with its credit once in; shut without one),
//!   the body's rows, the type chip, the registration, the logo tile, the track and its fill, the
//!   numbers, where Chromium puts them and in its colours.
//! - `the_card_through_the_window`: through the window's engine with the shell on (the synthetic
//!   snapshot as the `--fixture` folder, its `flight_details/{id}.json` written for the aircraft
//!   hovered): the pointer over an aircraft shows the card 16 px right of and below it at once,
//!   the photo slot open while the details load; after the 300 ms dwell the details' photo, logo
//!   and progress are in; off the aircraft the card goes. `u18-window-{loading,ready}.png`.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes.

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::NOW_MS;
use common::fixtures;
use common::shots::{gpu, save};
use egui::load::{ImageLoadResult, ImageLoader, ImagePoll, LoadError};
use egui::{ColorImage, Pos2, RawInput, Rect, SizeHint, Vec2};
use rustradar_app::app::Engine;
use rustradar_app::state::pick::{HoverCard, Progress};
use rustradar_app::state::{AppState, Startup};
use rustradar_app::ui::hover_card::{self, Painted};
use rustradar_app::ui::theme;
use rustradar_core::geo::Basemap;
use rustradar_core::hover::{HoverInfo, HoverStatus};
use rustradar_core::model::{AircraftImage, AircraftInfo, FlightDetails, FlightProgress, LiveFlight, ScheduleInfo, airline_logo_url};
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_map::io::DirTiles;
use rustradar_scene::camera::{ViewState, viewport_for};
use winit::dpi::PhysicalPosition;
use winit::event::{DeviceId, WindowEvent};

/// The desktop canvas, CSS px, at scale 1.
const SIZE: [u32; 2] = [1440, 900];
/// The photo's URI (a JetPhotos thumbnail's form) and the logo's airline.
const PHOTO: &str = "https://cdn.jetphotos.com/200/5/1_tb.jpg";
const LOGO_ID: u32 = 81;

/// The fixture photo, 200 × 133 (a JetPhotos thumbnail's size): red across, green down, blue 128.
/// Chromium's page loads the same picture from a PNG.
fn photo() -> ColorImage {
    let (w, h) = (200usize, 133usize);
    let rgba: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| [(x * 255 / (w - 1)) as u8, (y * 255 / (h - 1)) as u8, 128, 255])).collect();
    ColorImage::from_rgba_unmultiplied([w, h], &rgba)
}

/// The fixture logo, 140 × 50: a red block from (10, 10) to (130, 40) on transparency.
fn logo() -> ColorImage {
    let (w, h) = (140usize, 50usize);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 10..40 {
        for x in 10..130 {
            rgba[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[200, 30, 40, 255]);
        }
    }
    ColorImage::from_rgba_unmultiplied([w, h], &rgba)
}

/// Serves the fixture's pictures to egui's image loading for their remote URIs, standing in for
/// `app::images` (whose own tests fetch through a local HTTP server).
struct Pictures(HashMap<String, Arc<ColorImage>>);

impl Pictures {
    fn new() -> Self {
        Pictures(HashMap::from([(PHOTO.to_owned(), Arc::new(photo())), (airline_logo_url(LOGO_ID), Arc::new(logo()))]))
    }
}

impl ImageLoader for Pictures {
    fn id(&self) -> &str {
        "rustradar::tests::hover_card::pictures"
    }

    fn load(&self, _ctx: &egui::Context, uri: &str, _size_hint: SizeHint) -> ImageLoadResult {
        self.0.get(uri).map(|image| ImagePoll::Ready { image: image.clone() }).ok_or(LoadError::NotSupported)
    }

    fn forget(&self, _uri: &str) {}

    fn forget_all(&self) {}

    fn byte_size(&self) -> usize {
        0
    }
}

/// egui alone onto a target cleared to `--ink`, as the window's UI pass draws over the map.
struct Canvas {
    ctx: egui::Context,
    egui: egui_wgpu::Renderer,
    target: Offscreen,
}

impl Canvas {
    fn new(gpu: &Gpu) -> Self {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        ctx.add_image_loader(Arc::new(Pictures::new()));
        let egui = egui_wgpu::Renderer::new(&gpu.device, FORMAT, egui_wgpu::RendererOptions::default());
        Canvas { ctx, egui, target: Offscreen::new(gpu, SIZE) }
    }

    /// A frame of `card` at egui time `time`: what the card drew.
    fn frame(&mut self, gpu: &Gpu, time: f64, card: Option<&HoverCard>) -> Option<Painted> {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(SIZE[0] as f32, SIZE[1] as f32))),
            time: Some(time),
            ..Default::default()
        };
        let mut painted = None;
        let output = self.ctx.run_ui(raw, |ui| painted = hover_card::ui(ui.ctx(), card));
        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: self.target.size(), pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui.update_texture(device, queue, id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("hover card") });
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
        painted
    }

    /// Frames at egui times 0–2 s (the photo fades in over 160 ms), then the picture.
    fn settle(&mut self, gpu: &Gpu, card: &HoverCard) -> (Painted, Image) {
        let mut painted = None;
        for time in [0.0, 1.0, 2.0] {
            painted = self.frame(gpu, time, Some(card));
        }
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        (painted.expect("a card"), self.target.read(gpu).expect("read back"))
    }
}

fn flight() -> LiveFlight {
    LiveFlight {
        id: 1,
        lat: 59.6,
        lon: 17.9,
        track: 200,
        alt: 34_000,
        speed: 451,
        callsign: "SAS1417".into(),
        flight: "SK1417".into(),
        reg: "SE-ROM".into(),
        typecode: "A20N".into(),
        origin: "ARN".into(),
        destination: "CPH".into(),
        ..Default::default()
    }
}

/// The four cards of the Chromium pages, the pointer at (400, 300).
fn cards() -> Vec<(&'static str, HoverCard)> {
    let base = HoverCard {
        flight: flight(),
        from: "Stockholm".into(),
        to: "Copenhagen".into(),
        progress: Some(Progress { value: 0.42, measured: false }),
        info: None,
        status: HoverStatus::Loading,
        pointer: [400.0, 300.0],
    };
    let image = AircraftImage { copyright: "Jonas Lindqvist".into(), thumbnail: PHOTO.into(), ..Default::default() };
    let ready = HoverCard {
        progress: Some(Progress { value: 0.42, measured: true }),
        info: Some(HoverInfo { images: vec![image], logo_id: LOGO_ID, progress_pct: Some(42) }),
        status: HoverStatus::Ready,
        ..base.clone()
    };
    let plain = HoverCard {
        flight: LiveFlight { callsign: "AVERYLONGCALLSIGN12345".into(), flight: String::new(), alt: 0, speed: 12, on_ground: true, origin: String::new(), ..flight() },
        from: String::new(),
        progress: None,
        info: Some(HoverInfo { images: Vec::new(), logo_id: LOGO_ID, progress_pct: None }),
        status: HoverStatus::Ready,
        ..base.clone()
    };
    let bare = HoverCard {
        flight: LiveFlight { callsign: String::new(), flight: String::new(), reg: String::new(), typecode: String::new(), alt: 975, speed: 140, origin: String::new(), destination: String::new(), ..flight() },
        from: String::new(),
        to: String::new(),
        progress: None,
        info: None,
        status: HoverStatus::Unavailable,
        ..base.clone()
    };
    vec![("loading", base), ("ready", ready), ("plain", plain), ("bare", bare)]
}

fn rgb(image: &Image, x: u32, y: u32) -> [u8; 3] {
    let [r, g, b, _] = image.pixel(x, y);
    [r, g, b]
}

#[track_caller]
fn near(image: &Image, x: u32, y: u32, expected: [u8; 3], tolerance: u8) {
    let got = rgb(image, x, y);
    assert!(got.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= tolerance), "({x}, {y}): {got:?}, Chromium {expected:?}");
}

#[track_caller]
fn rect_near(got: Rect, [x, y, w, h]: [f32; 4], tolerance: f32, what: &str) {
    let ok = (got.min.x - x).abs() <= tolerance && (got.min.y - y).abs() <= tolerance && (got.width() - w).abs() <= tolerance && (got.height() - h).abs() <= tolerance;
    assert!(ok, "{what}: {got:?}, Chromium x {x} y {y} w {w} h {h}");
}

/// The rows where the colour at column `x` changes between `y0` and `y1`.
fn edges(image: &Image, x: u32, y0: u32, y1: u32) -> Vec<u32> {
    (y0 + 1..y1).filter(|&y| rgb(image, x, y) != rgb(image, x, y - 1)).collect()
}

/// The darkest pixel's rows in a box (where the ink of a text is), as a range.
fn ink_rows(image: &Image, [x0, y0, x1, y1]: [u32; 4], background: [u8; 3]) -> Option<(u32, u32)> {
    let rows: Vec<u32> = (y0..y1).filter(|&y| (x0..x1).any(|x| rgb(image, x, y).iter().zip(background).any(|(a, b)| a.abs_diff(b) > 40))).collect();
    Some((*rows.first()?, *rows.last()?))
}

#[test]
fn the_card_beside_chromium() {
    let Some(shared) = gpu() else { return };
    let gpu = &*shared;
    let ink = theme::INK.to_array();
    let ink = [ink[0], ink[1], ink[2]];
    for (name, card) in cards() {
        // a fresh context for each, as each page mounts its own component
        let mut canvas = Canvas::new(gpu);
        let (p, shot) = canvas.settle(gpu, &card);
        save(&shot, &format!("u18-card-{name}.png"));
        // the corner at the pointer + 16, the width 272 (Chromium: x 416, y 316)
        assert_eq!(p.rect.min, Pos2::new(416.0, 316.0), "{name}");
        assert_eq!(p.rect.width(), 272.0);
        // the outside of the card is the page's ink
        near(&shot, 414, 400, ink, 0);
        near(&shot, 690, 400, ink, 0);
        match name {
            "loading" | "ready" => {
                // Chromium: .photo-slot h 159, .photo 272 × 153, .body y 475 h 102.984
                rect_near(p.slot, [416.0, 316.0, 272.0, 159.0], 0.01, "slot");
                rect_near(p.body, [416.0, 475.0, 272.0, 102.984], 0.02, "body");
                rect_near(p.callsign, [429.0, 487.203, 59.391, 19.594], 0.5, "callsign");
                rect_near(p.chip.unwrap(), [496.391, 488.0, 38.672, 18.0], 0.5, "type chip");
                rect_near(p.reg.unwrap(), [543.063, 489.297, 40.0, 15.391], 0.5, "registration");
                rect_near(p.route.unwrap(), [429.0, 512.0, p.route.unwrap().width(), 18.188], 0.02, "route");
                rect_near(p.track.unwrap(), [429.0, 540.188, 246.0, 2.0], 0.02, "track");
                assert!((p.fill.unwrap().width() - 103.32).abs() < 0.7, "fill {:?}", p.fill);
                assert!((p.numbers.top() - 548.188).abs() < 0.02, "numbers {:?}", p.numbers);
                // the body: --surface inside a --line border; the gap under the photo is the page
                near(&shot, 600, 476, theme::SURFACE.to_array()[..3].try_into().unwrap(), 0);
                near(&shot, 600, 475, theme::LINE.to_array()[..3].try_into().unwrap(), 0);
                near(&shot, 600, 471, ink, 0);
                // the track: --line, its fill --amber (measured) or --text-3 (estimate)
                near(&shot, 600, 540, theme::LINE.to_array()[..3].try_into().unwrap(), 0);
                let fill = if name == "ready" { theme::AMBER } else { theme::TEXT_3 };
                near(&shot, 450, 540, fill.to_array()[..3].try_into().unwrap(), 0);
                near(&shot, 450, 541, fill.to_array()[..3].try_into().unwrap(), 0);
                assert_eq!(edges(&shot, 600, 535, 546), [540, 542], "the 2 px track on rows 540–541");
            }
            _ => {
                assert_eq!(p.slot.height(), 0.0, "{name}: the slot shut");
                assert_eq!(p.body.top(), 316.0);
            }
        }
        match name {
            "loading" => {
                // flat --surface-raised while loading (`.photo` outranks `.skeleton`), corners round
                assert_eq!(p.picture, None);
                for (x, y) in [(500, 400), (420, 320), (683, 465)] {
                    near(&shot, x, y, [62, 62, 62], 0);
                }
                near(&shot, 416, 316, ink, 12);
                assert_eq!(p.logo, None);
            }
            "ready" => {
                assert_eq!(p.picture, Some(1.0));
                // the photo covers the block: 200 × 133 scaled 1.36 (181 tall), 14 px cut off top
                // and bottom; Chromium's pixels there
                near(&shot, 552, 392, [127, 127, 128], 3);
                near(&shot, 450, 340, [31, 53, 128], 3);
                near(&shot, 660, 440, [229, 195, 128], 3);
                // its rows: Chromium's from 316 to 468, then the 6 px gap of ink
                assert_eq!(rgb(&shot, 600, 468)[2], 128);
                near(&shot, 600, 469, ink, 0);
                assert_eq!(p.credit.as_deref(), Some("© Jonas Lindqvist"));
                // Chromium: .logo x 614.609 w 60.391 h 26, its picture 50.391 × 18 at 619.609, 488
                rect_near(p.logo.unwrap(), [614.609, 484.0, 60.391, 26.0], 0.5, "logo tile");
                rect_near(p.logo_picture.unwrap(), [619.609, 488.0, 50.391, 18.0], 0.5, "logo picture");
                near(&shot, 617, 486, theme::TEXT_1.to_array()[..3].try_into().unwrap(), 0);
                near(&shot, 645, 497, [200, 30, 40], 2);
            }
            "plain" => {
                rect_near(p.body, [416.0, 316.0, 272.0, 86.984], 0.02, "body");
                // the callsign gives way to the others: 82.938 px, cut with …
                rect_near(p.callsign, [429.0, 328.203, 82.938, 19.594], 0.5, "callsign");
                assert!(p.callsign_text.ends_with('…'), "{}", p.callsign_text);
                rect_near(p.chip.unwrap(), [519.938, 329.0, 38.672, 18.0], 0.5, "type chip");
                rect_near(p.reg.unwrap(), [566.609, 330.297, 40.0, 15.391], 0.5, "registration");
                rect_near(p.logo.unwrap(), [614.609, 325.0, 60.391, 26.0], 0.5, "logo tile");
                assert_eq!(p.route_text, "To Copenhagen");
                assert!((p.route.unwrap().top() - 353.0).abs() < 0.02);
                assert_eq!(p.track, None);
                assert_eq!(p.numbers_text, "On ground · 12 kt");
                assert!((p.numbers.top() - 373.188).abs() < 0.02);
            }
            "bare" => {
                rect_near(p.body, [416.0, 316.0, 272.0, 66.797], 0.02, "body");
                assert_eq!(p.callsign_text, "No callsign");
                rect_near(p.callsign, [429.0, 328.203, 93.328, 19.594], 0.5, "callsign");
                assert_eq!((p.chip, p.reg, p.logo, p.route), (None, None, None, None));
                assert_eq!(p.numbers_text, "975 ft · 140 kt");
                assert!((p.numbers.top() - 353.0).abs() < 0.02);
                // the body's bottom border on row 382 (316 + 66.797, snapped)
                assert_eq!(edges(&shot, 600, 375, 390), [382, 383]);
            }
            _ => unreachable!(),
        }
        // the callsign's ink inside its line box, as Chromium's sits
        let (top, bottom) = ink_rows(&shot, [429, p.callsign.top() as u32, 470, p.callsign.bottom() as u32 + 1], theme::SURFACE.to_array()[..3].try_into().unwrap()).expect("the callsign's ink");
        assert!(bottom - top >= 8, "{name}: callsign ink rows {top}..{bottom}");
    }
}

// ------------------------------------------------------------------------------------------------
// through the window

/// The window's canvas, CSS px.
const WINDOW: [u32; 2] = [1100, 720];
/// Stockholm at zoom 9 (plan section 8's V3).
const V3: ViewState = ViewState { latitude: 59.33, longitude: 18.07, zoom: 9.0, pitch: 0.0, bearing: 0.0, position: None };

/// The window's frames, drawn offscreen, with the shell.
struct Window {
    engine: Engine,
    target: Offscreen,
    egui_s: f64,
}

impl Window {
    fn new(gpu: &Gpu, state: AppState) -> Window {
        let mut engine = Engine::new(gpu, FORMAT, state);
        engine.shell = true;
        engine.map.set_tiles(Arc::new(DirTiles::fixtures()), None);
        engine.egui.add_image_loader(Arc::new(Pictures::new()));
        Window { engine, target: Offscreen::new(gpu, WINDOW), egui_s: 0.0 }
    }

    fn frame(&mut self, gpu: &Gpu, now: f64) -> Image {
        let mut input = egui::RawInput {
            time: Some(self.egui_s),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(WINDOW[0] as f32, WINDOW[1] as f32))),
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(1.0);
        self.egui_s += 0.25;
        self.engine.frame(gpu, self.target.view(), WINDOW, 1.0, input, now);
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("the frame");
        self.target.read(gpu).expect("read back")
    }

    fn settle(&mut self, gpu: &Gpu, now: f64) -> Image {
        let started = std::time::Instant::now();
        loop {
            self.frame(gpu, now);
            if !self.engine.map.pending() || started.elapsed() > rustradar_map::headless::MAX_WAIT {
                return self.frame(gpu, now);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn move_to(&mut self, [x, y]: [f64; 2], now: f64) {
        let event = WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(x, y) };
        self.engine.window_event(&event, 1.0, now);
    }
}

/// A `--fixture` folder holding the synthetic snapshot as `live_flights.json`.
fn snapshot_folder() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("u18-synthetic");
    fixtures::write_fixture(&dir, &fixtures::synthetic_snapshot(NOW_MS)).expect("the fixture folder");
    dir
}

/// Writes `flight_details/{id}.json` into `folder`: a photo, an airline and a measured progress.
fn write_details(folder: &Path, id: u32) {
    let details = FlightDetails {
        aircraft: Some(AircraftInfo { images: vec![AircraftImage { copyright: "Jonas Lindqvist".into(), thumbnail: PHOTO.into(), ..Default::default() }], ..Default::default() }),
        schedule: Some(ScheduleInfo { painted_as_id: LOGO_ID, ..Default::default() }),
        progress: Some(FlightProgress { traversed_distance: 300_000, remaining_distance: 400_000, progress_pct: 43, ..Default::default() }),
        ..Default::default()
    };
    let dir = folder.join("flight_details");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{id}.json")), serde_json::to_vec(&details).unwrap()).unwrap();
}

/// The icon farthest from every other aircraft in the part of the map the shell leaves free, with
/// room for the card right of and below it.
fn lone_icon(engine: &Engine) -> (u32, [f64; 2]) {
    let state = &engine.state;
    let viewport = viewport_for(&state.controller.view(), state.controller.size());
    let aircraft = &engine.map.scene().aircraft.scene;
    let spots: Vec<(u32, [f64; 2])> = aircraft
        .icons
        .iter()
        .map(|i| (i.id, i.position))
        .chain(aircraft.groups.iter().flat_map(|g| g.instances.iter().map(|m| (m.id, m.position))))
        .map(|(id, p)| {
            let [x, y, _] = viewport.project(p);
            (id, [x, y])
        })
        .collect();
    let free = |[x, y]: [f64; 2]| (360.0..700.0).contains(&x) && (90.0..380.0).contains(&y);
    let (id, at, clear) = spots
        .iter()
        .filter(|(_, at)| free(*at))
        .map(|&(id, at)| {
            let clear = spots.iter().filter(|(other, _)| *other != id).map(|(_, p)| (p[0] - at[0]).hypot(p[1] - at[1])).fold(f64::INFINITY, f64::min);
            (id, at, clear)
        })
        .max_by(|a, b| a.2.total_cmp(&b.2))
        .expect("an aircraft in the free part of the map");
    assert!(clear > 40.0, "no aircraft within 40 px of {id}: {clear}");
    (id, at.map(f64::round))
}

#[test]
fn the_card_through_the_window() {
    let Some(shared) = gpu() else { return };
    let gpu = &*shared;
    let folder = snapshot_folder();
    let startup = Startup { basemap: Some(Basemap::Dark), fixture: Some(folder.clone()), demo: false };
    let mut state = AppState::new(startup, None, None, None, V3, WINDOW.map(f64::from));
    state.poll(NOW_MS);
    let mut window = Window::new(gpu, state);
    window.settle(gpu, NOW_MS);
    let (id, at) = lone_icon(&window.engine);
    write_details(&folder, id);

    // over the aircraft: the card at once, loading (the slot open, flat), at the pointer + 16
    window.move_to(at, NOW_MS);
    window.frame(gpu, NOW_MS);
    let loading = window.frame(gpu, NOW_MS + 10.0);
    save(&loading, "u18-window-loading.png");
    let card = window.engine.state.hover_card(NOW_MS + 10.0).expect("a card for the hovered aircraft");
    assert_eq!(card.flight.id, id);
    assert_eq!(card.status, HoverStatus::Loading);
    let corner = [at[0] as u32 + 16, at[1] as u32 + 16];
    near(&loading, corner[0] + 100, corner[1] + 60, [62, 62, 62], 0);
    near(&loading, corner[0] + 100, corner[1] + 160, theme::SURFACE.to_array()[..3].try_into().unwrap(), 0);

    // the dwell is up: the details are asked for and answered from the fixture folder
    for step in 1..=8 {
        window.frame(gpu, NOW_MS + 100.0 * f64::from(step));
    }
    let card = window.engine.state.hover_card(NOW_MS + 800.0).expect("still a card");
    assert_eq!(card.status, HoverStatus::Ready);
    assert_eq!(card.progress, Some(Progress { value: 0.43, measured: true }));
    assert_eq!(card.info.as_ref().map(|i| i.logo_id), Some(LOGO_ID));
    let ready = window.frame(gpu, NOW_MS + 900.0);
    save(&ready, "u18-window-ready.png");
    // the photo (cover-fitted: 552, 392 of the 1440 page is (136, 76) into the card) and the logo
    let [r, g, b] = rgb(&ready, corner[0] + 136, corner[1] + 76);
    assert!(r.abs_diff(127) <= 3 && g.abs_diff(127) <= 3 && b.abs_diff(128) <= 3, "the photo's middle: {r} {g} {b}");
    let red = (corner[0] + 190..corner[0] + 260).flat_map(|x| (corner[1] + 165..corner[1] + 190).map(move |y| (x, y))).filter(|&(x, y)| rgb(&ready, x, y) == [200, 30, 40]).count();
    assert!(red > 200, "the logo's red block: {red} px");

    // off the aircraft onto empty map, twice: the card goes
    window.move_to([at[0], at[1] + 300.0], NOW_MS + 1000.0);
    window.frame(gpu, NOW_MS + 1000.0);
    window.move_to([at[0] + 1.0, at[1] + 300.0], NOW_MS + 1000.0);
    let off = window.frame(gpu, NOW_MS + 1000.0);
    assert!(window.engine.state.hover_card(NOW_MS + 1000.0).is_none());
    let [r, g, b] = rgb(&off, corner[0] + 136, corner[1] + 76);
    assert!(!(r.abs_diff(127) <= 3 && g.abs_diff(127) <= 3 && b.abs_diff(128) <= 3), "the card is gone");
}
