//! Item U9's acceptance (plan stage 7; doc 06 §11, §14): the aircraft list (TopFlights) and the
//! aircraft filter panel as the shell draws them with the app's slots.
//!
//! - `u9-list-loading.png`: no feed (the reference pictures' plain browser): the list card's six
//!   placeholder rows where `docs/port/reference/dark-v1-1440x900.png` has them;
//! - `u9-list-rows.png`, `u9-list-failed.png`: rows from a fixture (a selected one, one with no
//!   route, one with no callsign, a long type cut short) and the failure with Try again, sized as
//!   Chromium lays out today's markup (52.98 px rows, a 29.48 px header, the message 78.19 px);
//! - `u9-filters.png`: the panel open in today's window, giving way to the list's least height and
//!   scrolling its body;
//! - `u9-filters-whole.png`, `u9-filters-set.png`: on a canvas tall enough for all of it, with
//!   today's defaults and with filters set (chips in the three token inputs, categories off, a
//!   range, one box off): the natural heights (722.75 and 714.75) and each part where Chromium
//!   puts it (the header's rule at 49, the text field at 86.8, the first chip at 167.59, the
//!   thumbs, the boxes, the token fields), within a pixel;
//! - `u9-filters-suggestions.png`: the airlines' suggestions after a click in the field and an
//!   `a` typed, 4 px under the field, rows 30.19 high.
//!
//! The Chromium layouts were made from `TopFlights.svelte`, `FilterPanel.svelte` and
//! `TokenInput.svelte`'s markup and CSS with app.css and the Overpass fonts (Playwright's
//! Chromium, scale 1); their numbers are written into the checks.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes.

mod common;

use std::path::Path;

use common::NOW_MS;
use common::shots::{Rect, bounds, gpu, pixel, save};
use egui::{Color32, Id, Pos2, RawInput, Vec2};
use rustradar_app::state::aircraft::{FIXTURE_LIVE, FIXTURE_REFERENCE, FIXTURE_TOP};
use rustradar_app::state::shortcuts::Open;
use rustradar_app::state::{Action, AppState, Startup};
use rustradar_app::ui::lists::flights;
use rustradar_app::ui::shell::{self, Placed, Slots};
use rustradar_app::ui::theme;
use rustradar_core::filters::Filters;
use rustradar_core::geo::Basemap;
use rustradar_core::model::{Airline, Airport, LiveFlight, LiveSnapshot, ReferenceData, TopFlight};
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_scene::camera::ViewState;

/// The desktop reference pictures' canvas, CSS px, at scale 1.
const SIZE: [u32; 2] = [1440, 900];
/// A canvas tall enough for the whole filter panel over the list's least height.
const TALL: [u32; 2] = [1440, 1200];

/// egui alone onto a target cleared to `--ink`, as the window's UI pass draws it.
struct Canvas {
    ctx: egui::Context,
    egui: egui_wgpu::Renderer,
    target: Offscreen,
    size: [u32; 2],
}

impl Canvas {
    fn new(gpu: &Gpu) -> Self {
        Canvas::sized(gpu, SIZE)
    }

    fn sized(gpu: &Gpu, size: [u32; 2]) -> Self {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let egui = egui_wgpu::Renderer::new(&gpu.device, FORMAT, egui_wgpu::RendererOptions::default());
        Canvas { ctx, egui, target: Offscreen::new(gpu, size), size }
    }

    /// A frame of the shell with the app's slots for `state` at egui time `time`.
    fn frame(&mut self, gpu: &Gpu, time: f64, state: &AppState) -> (Placed, Vec<Action>) {
        self.frame_with(gpu, time, state, Vec::new())
    }

    /// A frame with `events` as its input.
    fn frame_with(&mut self, gpu: &Gpu, time: f64, state: &AppState, events: Vec<egui::Event>) -> (Placed, Vec<Action>) {
        let raw = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(self.size[0] as f32, self.size[1] as f32))),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut placed = Placed::default();
        let mut actions = Vec::new();
        let output = self.ctx.run_ui(raw, |ui| placed = shell::show(ui.ctx(), state, NOW_MS, &mut actions, &Slots::APP));
        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: self.target.size(), pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui.update_texture(device, queue, id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("u9") });
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
        (placed, actions)
    }

    /// Frames at egui times 0–3 s (areas are sized on their first frame, the sidebar solves its
    /// heights from the last frame's measures), then the picture.
    fn settle(&mut self, gpu: &Gpu, state: &AppState) -> (Placed, Image) {
        let mut placed = Placed::default();
        for time in [0.0, 1.0, 2.0, 3.0] {
            let (p, actions) = self.frame(gpu, time, state);
            assert!(actions.is_empty(), "drawing alone asks nothing: {actions:?}");
            placed = p;
        }
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        (placed, self.target.read(gpu).expect("read back"))
    }
}

// ------------------------------------------------------------------------------------------------
// fixtures

/// A folder of the test's own.
struct Folder(std::path::PathBuf);

impl Folder {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rustradar-u9-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a folder");
        Folder(dir)
    }

    fn write(&self, name: &str, json: Vec<u8>) {
        std::fs::write(self.0.join(name), json).expect("a fixture file");
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn top(id: u32, callsign: &str, flight_number: &str, clicks: u32, from: &str, to: &str, description: &str) -> TopFlight {
    TopFlight {
        flight_id: id,
        live_clicks: clicks,
        callsign: callsign.into(),
        flight_number: flight_number.into(),
        from_iata: from.into(),
        to_iata: to.into(),
        full_description: description.into(),
        ..Default::default()
    }
}

/// The rows of the Chromium layout.
fn top_flights() -> Vec<TopFlight> {
    vec![
        top(1, "SAS1415", "SK1415", 12_345, "ARN", "CPH", "Airbus A320neo"),
        top(2, "BAW283", "BA283", 1443, "LHR", "LAX", "Boeing 777-336(ER)"),
        top(3, "", "DL1", 987, "", "", "Boeing 767-332(ER)"),
        top(4, "", "", 45, "JFK", "", ""),
        top(5, "AFR1", "AF1", 100_000, "CDG", "JFK", "Airbus A350-941 with a very long description that is cut"),
        top(6, "N12345", "", 7, "", "HNL", "Cessna 172"),
    ]
}

fn reference_data() -> ReferenceData {
    let airline = |icao: &str, iata: &str, name: &str| Airline { icao: icao.into(), iata: iata.into(), name: name.into() };
    let airport = |iata: &str, icao: &str, city: &str, name: &str| Airport { iata: iata.into(), icao: icao.into(), city: city.into(), name: name.into(), size: 50, ..Default::default() };
    ReferenceData {
        fetched_at: 0,
        airlines: vec![airline("SAS", "SK", "Scandinavian Airlines"), airline("DLH", "LH", "Lufthansa"), airline("AFR", "AF", "Air France")],
        airports: vec![airport("ARN", "ESSA", "Stockholm", "Arlanda"), airport("LHR", "EGLL", "London", "Heathrow")],
    }
}

/// A state on `dir` with the feed's answers in it, polled until they are in.
fn fixture_state(dir: &Folder) -> AppState {
    let startup = Startup { basemap: Some(Basemap::Dark), fixture: Some(dir.0.clone()), ..Default::default() };
    let mut s = AppState::new(startup, None, None, None, ViewState::INITIAL, SIZE.map(f64::from));
    for t in [NOW_MS, NOW_MS + 16.0, NOW_MS + 32.0] {
        s.poll(t);
    }
    s
}

// ------------------------------------------------------------------------------------------------
// checks

fn rgb(c: Color32) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

fn lum(p: [u8; 4]) -> f64 {
    (f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])) / 3.0
}

/// The bounds of the pixels in `area` brighter than `under` by more than 30 levels: text and icons.
fn ink(image: &Image, area: Rect, under: Color32) -> Option<Rect> {
    bounds(image, area, |p| lum(p) - lum(rgb(under)) > 30.0)
}

/// The rectangle of pixels within 2 levels of `c` in `area`.
fn painted(image: &Image, area: Rect, c: Color32) -> Option<Rect> {
    bounds(image, area, |p| (0..3).all(|i| p[i].abs_diff(rgb(c)[i]) <= 2))
}

#[track_caller]
fn near(got: f32, want: f32, tolerance: f32, what: &str) {
    assert!((got - want).abs() <= tolerance, "{what}: {got} for {want}");
}

fn rect(r: egui::Rect) -> Rect {
    Rect::new(r.left().round() as u32, r.top().round() as u32, r.width().round() as u32, r.height().round() as u32)
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

/// The tops of the skeleton bars in the list card's left column (x 40–47, clear of their rounded
/// corners) from `from` down to the card's bottom, each bar once.
fn bar_tops(image: &Image, card: Rect, from: u32) -> Vec<u32> {
    let mut tops = Vec::new();
    let mut inside = false;
    for y in from..card.y1 {
        let bar = (40..47).all(|x| lum(pixel(image, x, y)) > lum(rgb(theme::SURFACE)) + 6.0);
        if bar && !inside {
            tops.push(y);
        }
        inside = bar;
    }
    tops
}

// ------------------------------------------------------------------------------------------------
// the list

/// No feed, as the reference pictures' plain browser: the placeholders in today's places.
#[test]
fn the_loading_list_is_today_s() {
    let Some(gpu) = gpu() else { return };
    let state = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, ViewState::INITIAL, SIZE.map(f64::from));
    assert_eq!(flights::body(&state), flights::Body::Loading);
    let mut canvas = Canvas::new(&gpu);
    let (placed, image) = canvas.settle(&gpu, &state);
    save(&image, "u9-list-loading.png");
    let card = placed.sidebar.list.expect("the list card");
    near(card.width(), 320.0, 0.01, "the card's width");
    // header 29.48 and six rows of 40, in a card padded 12 with a border
    near(card.height(), 2.0 * 13.0 + flights::header_height() + 240.0, 0.5, "the card's height");
    // the title's ink in the header, the eye and its word on the right
    let title = ink(&image, Rect::new(card.left() as u32 + 9, card.top() as u32 + 13, 120, 24), theme::SURFACE).expect("the title");
    let hint = ink(&image, Rect::new(card.right() as u32 - 90, card.top() as u32 + 13, 80, 20), theme::SURFACE).expect("the hint");
    // and the same against today's picture, measured from each card's top edge
    let today = reference("dark-v1-1440x900.png");
    let ref_top = (250..320).find(|&y| (0..3).all(|i| pixel(&today, 100, y)[i].abs_diff(68) <= 3) && (lum(pixel(&today, 100, y + 1)) - 50.0).abs() < 2.0).expect("today's list card");
    let ours: Vec<i64> = bar_tops(&image, rect(card), (card.top() + 40.0) as u32).iter().map(|&y| i64::from(y) - card.top() as i64).collect();
    let theirs: Vec<i64> = bar_tops(&today, Rect::new(16, ref_top, 320, 300), ref_top + 40).iter().map(|&y| i64::from(y) - i64::from(ref_top)).collect();
    // twelve bars and the card's bottom edge
    assert_eq!(ours.len(), 13, "{ours:?}");
    assert_eq!(theirs.len(), 13, "{theirs:?}");
    for (a, b) in ours.iter().zip(&theirs) {
        assert!((a - b).abs() <= 1, "every bar within a pixel of today's: ours {ours:?}, today's {theirs:?}");
    }
    // the rows' bars every 40 px, the second line 14 under the first
    for pair in ours[..12].chunks(2) {
        assert_eq!(pair[1] - pair[0], 14, "{ours:?}");
    }
    for i in (2..12).step_by(2) {
        assert_eq!(ours[i] - ours[i - 2], 40, "{ours:?}");
    }
    let first = card.top() as u32 + ours[0] as u32;
    // the bars' extent: the first row's left and right bars 8 in from the row's edges
    let left = bounds(&image, Rect::new(card.left() as u32 + 2, first + 3, 316, 1), |p| lum(p) > lum(rgb(theme::SURFACE)) + 6.0).expect("the first row");
    near(left.x0 as f32, card.left() + 1.0 + 8.0 + 8.0, 0.5, "the first bar's left");
    near(left.x1 as f32, card.right() - 1.0 - 8.0 - 8.0, 0.5, "the right bar's right");
    assert!(title.x0 as f32 >= card.left() + 16.0 && title.x0 as f32 <= card.left() + 19.0, "{title:?}");
    near(hint.x1 as f32, card.right() - 17.0, 1.5, "the hint's right edge");
}

/// Rows from a fixture: the selected flight, the second line's forms, the sizes of today's.
#[test]
fn the_rows_and_the_failure() {
    let Some(gpu) = gpu() else { return };
    let dir = Folder::new("rows");
    dir.write(FIXTURE_TOP, serde_json::to_vec(&top_flights()).unwrap());
    let mut state = fixture_state(&dir);
    assert_eq!(flights::body(&state), flights::Body::Rows);
    // the second row selected, as in Chromium's layout (its panel is U10's: not drawn here)
    assert!(state.apply(Action::PickFlight(2), NOW_MS + 40.0));
    let mut canvas = Canvas::new(&gpu);
    let (placed, image) = canvas.settle(&gpu, &state);
    save(&image, "u9-list-rows.png");
    let card = placed.sidebar.list.expect("the list card");
    // Chromium: the card 373.39 high (29.48 + 6 × 52.98 + 26)
    near(card.height(), 373.39, 0.6, "the card's height");
    let row_top = |i: usize| card.top() + 13.0 + flights::header_height() + i as f32 * flights::row_height();
    // the selected row on --amber-soft over --surface (Chromium: (51,67,84)), edge to edge of the
    // card's content
    let soft = Color32::from_rgb(51, 67, 84);
    let selected = painted(&image, Rect::new(card.left() as u32 + 2, row_top(1) as u32 + 1, 316, 50), soft).expect("the selected row's fill");
    near(selected.x0 as f32, card.left() + 9.0, 1.0, "the selection's left");
    near(selected.x1 as f32, card.right() - 9.0, 1.0, "the selection's right");
    // its callsign in --amber
    let amber = bounds(&image, Rect::new(card.left() as u32 + 9, row_top(1) as u32, 150, 26), |p| p[2] > 180 && p[0] < 120).expect("the amber callsign");
    near(amber.x0 as f32, card.left() + 17.0, 1.5, "the callsign's left");
    // the watchers right-aligned 8 in from the row's edge
    let watchers = ink(&image, Rect::new(card.right() as u32 - 80, row_top(0) as u32 + 8, 70, 18), theme::SURFACE).expect("the watchers");
    near(watchers.x1 as f32, card.right() - 17.0, 1.5, "the watchers' right edge");
    // the long type cut at the row's content edge
    let long = ink(&image, Rect::new(card.left() as u32 + 9, row_top(4) as u32 + 28, 302, 18), theme::SURFACE).expect("the fifth row's second line");
    assert!(long.x1 as f32 <= card.right() - 16.0, "cut within the row: {long:?}");
    assert!(long.x1 as f32 >= card.right() - 30.0, "an ellipsis near the end: {long:?}");

    // the failure: nothing loaded and the ask failed
    let failing = Folder::new("failed");
    let state = fixture_state(&failing);
    assert_eq!(flights::body(&state), flights::Body::Failed);
    let mut canvas = Canvas::new(&gpu);
    let (placed, image) = canvas.settle(&gpu, &state);
    save(&image, "u9-list-failed.png");
    let card = placed.sidebar.list.expect("the list card");
    // Chromium: 29.48 + 78.19 + 26
    near(card.height(), 133.67, 0.6, "the card's height");
    // the button's border: 32 high, its top 12 under the message's line
    let top = card.top() + 13.0 + flights::header_height() + 8.0 + 18.2 + 12.0;
    let button = painted(&image, Rect::new(card.left() as u32 + 10, top as u32 - 3, 200, 40), theme::LINE_CONTROL).expect("the button's border");
    near(button.y0 as f32, top, 1.0, "the button's top");
    near(button.height() as f32, 32.0, 1.0, "the button's height");
    near(button.x0 as f32, card.left() + 17.0, 1.0, "the button's left");
}

// ------------------------------------------------------------------------------------------------
// the filter panel

/// The first row from `y0` to `y1` at column `x` whose pixel is within `tolerance` of `c`, as an
/// offset from `top`.
fn first_row(image: &Image, x: u32, top: f32, (y0, y1): (f32, f32), c: Color32, tolerance: u8) -> Option<f32> {
    ((top + y0) as u32..(top + y1) as u32).find(|&y| (0..3).all(|i| pixel(image, x, y)[i].abs_diff(rgb(c)[i]) <= tolerance)).map(|y| y as f32 - top)
}

/// Chromium's layout of today's markup, offsets from the panel's top edge: the text field's
/// border, the first chip's, the low thumb's ring, a box, and the three token fields' borders.
struct Tops {
    field: f32,
    chip: f32,
    thumb: f32,
    /// The low thumb's centre across, from the panel's left edge.
    thumb_x: f32,
    checkbox: f32,
    tokens: [f32; 3],
}

const DEFAULT_TOPS: Tops = Tops { field: 86.8, chip: 167.59, thumb: 386.39, thumb_x: 25.0, checkbox: 414.98, tokens: [467.38, 546.17, 644.36] };
const SET_TOPS: Tops = Tops { field: 86.8, chip: 167.59, thumb: 356.39, thumb_x: 25.0 + 270.0 * 0.2, checkbox: 384.98, tokens: [437.38, 538.17, 636.36] };

/// Checks the parts of the panel at `panel` in `image` against `want`, within a pixel.
#[track_caller]
fn check_parts(image: &Image, panel: egui::Rect, want: &Tops, what: &str) {
    let top = panel.top();
    let x = |dx: f32| (panel.left() + dx) as u32;
    let control = theme::LINE_CONTROL;
    let got = |name: &str, v: Option<f32>, w: f32| {
        let v = v.unwrap_or_else(|| panic!("{what}: {name} not found"));
        assert!((v - w.floor()).abs() <= 1.0, "{what}: {name} at {v} for Chromium's {w}");
    };
    got("the rule", first_row(image, x(5.0), top, (40.0, 60.0), theme::LINE, 3), 49.0);
    got("the text field", first_row(image, x(100.0), top, (60.0, 120.0), control, 3), want.field);
    got("the first chip", first_row(image, x(40.0), top, (want.chip - 8.0, want.chip + 8.0), control, 3), want.chip);
    got("the low thumb", first_row(image, x(want.thumb_x), top, (want.thumb - 8.0, want.thumb + 8.0), theme::AMBER, 10), want.thumb);
    got("a box", first_row(image, x(23.5), top, (want.checkbox - 6.0, want.checkbox + 6.0), theme::AMBER, 10), want.checkbox);
    for (i, t) in want.tokens.iter().enumerate() {
        got(&format!("token field {i}"), first_row(image, x(290.0), top, (t - 8.0, t + 8.0), control, 3), *t);
    }
}

#[test]
fn the_filter_panel_is_today_s() {
    let Some(gpu) = gpu() else { return };
    let dir = Folder::new("filters");
    let flights: Vec<LiveFlight> = (0..5).map(|i| LiveFlight { id: i + 1, callsign: format!("SAS{i}"), alt: 30_000, lat: 59.0, lon: 18.0, ..Default::default() }).collect();
    dir.write(FIXTURE_LIVE, serde_json::to_vec(&LiveSnapshot { flights, server_time_ms: NOW_MS as u64, ..Default::default() }).unwrap());
    dir.write(FIXTURE_REFERENCE, serde_json::to_vec(&reference_data()).unwrap());
    let mut state = fixture_state(&dir);
    assert!(state.apply(Action::Open(Open::Filters, true), NOW_MS + 40.0));

    // in today's window: the panel gives way to the list's least height and scrolls its body
    let mut canvas = Canvas::new(&gpu);
    let (placed, image) = canvas.settle(&gpu, &state);
    save(&image, "u9-filters.png");
    let panel = placed.sidebar.filters.expect("the filter panel");
    let list = placed.sidebar.list.expect("the list card");
    near(panel.width(), 320.0, 0.01, "the panel's width");
    near(list.height(), 96.0 + 26.0, 0.5, "the list held at 96");
    near(list.bottom(), 900.0 - 40.0, 0.5, "the column's bottom");
    let top = panel.top();
    assert!(first_row(&image, (panel.left() + 5.0) as u32, top, (40.0, 60.0), theme::LINE, 3) == Some(49.0), "the header's rule");

    // tall enough for all of it: every part where Chromium puts it
    let mut canvas = Canvas::sized(&gpu, TALL);
    let (placed, image) = canvas.settle(&gpu, &state);
    save(&image, "u9-filters-whole.png");
    let panel = placed.sidebar.filters.expect("the filter panel");
    // Chromium: 722.75 (49 + 671.75 + the borders)
    near(panel.height(), 722.75, 1.0, "the panel's natural height");
    check_parts(&image, panel, &DEFAULT_TOPS, "default");
    let id = |s: &str| Id::unique(s);
    let rect_of = |id: Id| canvas.ctx.read_response(id).unwrap_or_else(|| panic!("{id:?}")).rect;
    let reset = rect_of(id("rustradar::filters::aircraft::reset"));
    // Chromium: Reset at x 230.78 of the panel's content, 13.59 down, 39.22 × 20.8
    near(reset.left() - panel.left() - 1.0, 230.78, 1.0, "Reset's left");
    near(reset.top() - panel.top() - 1.0, 13.59, 0.5, "Reset's top");
    let air = rect_of(id("rustradar::filters::aircraft::airborne"));
    let ground = rect_of(id("rustradar::filters::aircraft::on_ground"));
    // Chromium: the labels 72.47 and 104.77 wide, 16 apart
    near(air.width(), 72.47, 2.0, "In the air's width");
    near(ground.width(), 104.77, 2.0, "On the ground's width");
    near(ground.left() - air.right(), 16.0, 0.01, "between the boxes");
    // Reset is off: no --amber in it
    assert!(bounds(&image, rect(reset), |p| p[2] > 180 && p[0] < 120).is_none(), "Reset disabled");

    // the airlines' suggestions: a click in the field, an "a" typed (Chromium's layout of the
    // list: 4 under the field, rows 30.19 high, 4 + 1 round them)
    let field_top = panel.top() + DEFAULT_TOPS.tokens[0];
    let at = Pos2::new(panel.left() + 150.0, field_top + 21.0);
    let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
    let mut time = 4.0;
    for events in [vec![egui::Event::PointerMoved(at)], vec![button(true)], vec![button(false)], vec![egui::Event::Text("a".into())], vec![], vec![]] {
        let (_, actions) = canvas.frame_with(&gpu, time, &state, events);
        assert!(actions.is_empty(), "typing a query changes no filter: {actions:?}");
        time += 0.5;
    }
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    let image = canvas.target.read(&gpu).expect("read back");
    save(&image, "u9-filters-suggestions.png");
    // the list on --surface-raised, its top border 4 under the field's 42
    let list_x = (panel.left() + 200.0) as u32;
    let list = first_row(&image, list_x, field_top, (40.0, 60.0), theme::LINE, 3).expect("the suggestions' border");
    near(list, 46.0, 1.0, "the list's top under the field's");
    // three suggestions: Scandinavian Airlines (5 live), then Air France and Lufthansa by name
    let rows = (field_top + list + 5.0) as u32;
    let first = bounds(&image, Rect::new(list_x, rows, 2, 30), |p| (0..3).all(|i| p[i].abs_diff(rgb(theme::SURFACE_HOVER)[i]) <= 2)).expect("the first row, active");
    near(first.height() as f32, 30.19, 1.0, "a row's height");
    let below = rows as f32 + 3.0 * 30.19 + 5.0;
    assert_eq!(first_row(&image, list_x, 0.0, (below - 2.0, below + 2.0), theme::LINE, 3).map(|y| y.round()), Some(below.floor()), "the list's bottom border after three rows");

    // set filters: chips in the token inputs, categories off, a range, one box off
    state.persisted.filters = Filters {
        categories: vec![0, 1, 3, 4],
        altitude: (10_000, 35_000),
        airborne: true,
        on_ground: false,
        airlines: vec!["SAS".into(), "DLH".into()],
        types: vec!["B77".into()],
        airports: vec!["ARN".into()],
        text: "SAS".into(),
    };
    state.poll(NOW_MS + 100.0);
    let mut canvas = Canvas::sized(&gpu, TALL);
    let (placed, image) = canvas.settle(&gpu, &state);
    save(&image, "u9-filters-set.png");
    let panel = placed.sidebar.filters.expect("the filter panel");
    // Chromium: 714.75
    near(panel.height(), 714.75, 1.0, "the set panel's natural height");
    check_parts(&image, panel, &SET_TOPS, "set");
    let reset = canvas.ctx.read_response(id("rustradar::filters::aircraft::reset")).expect("Reset").rect;
    assert!(bounds(&image, rect(reset), |p| p[2] > 180 && p[0] < 120).is_some(), "Reset in --amber");
    // the span between the thumbs in --amber, from 10,000 to 35,000 ft: thumb centres at
    // 8 + 270 × v of the body's content (16 in)
    let slider_y = (panel.top() + SET_TOPS.thumb + 8.0) as u32;
    let span = bounds(&image, Rect::new(panel.left() as u32 + 2, slider_y, 316, 1), |p| (0..3).all(|i| p[i].abs_diff(rgb(theme::AMBER)[i]) <= 2)).expect("the span");
    near(span.x0 as f32 - panel.left() - 1.0, 16.0 + 8.0 + 270.0 * 0.2 - 8.0, 2.0, "the low thumb's left");
    near(span.x1 as f32 - panel.left() - 1.0, 16.0 + 8.0 + 270.0 * 0.7 + 8.0, 2.0, "the high thumb's right");
    // the box that is off: (59,59,59) inside a (133,133,133) border
    let off = canvas.ctx.read_response(id("rustradar::filters::aircraft::on_ground")).expect("On the ground").rect;
    let inside = pixel(&image, (off.left() + 6.5) as u32, (off.center().y) as u32);
    assert!((0..3).all(|i| inside[i].abs_diff(59) <= 2), "the empty box's fill: {inside:?}");
}
