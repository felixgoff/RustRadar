//! Item U14's acceptance (plan stage 7; doc 06 §8.1, §12, §15, §24.4): the trains list, the train
//! filter panel and the train panel as the shell draws them with the app's slots, from the demo
//! trains (`rustradar_app::dev::demo_trains` through V3's centre at the shots' clock, some given a
//! destination, a delay or no speed) as a `--fixture` folder's `trains_in_view.json`, with the
//! stops of `train_stops.json`.
//!
//! - `u14-list.png`: the list sorted by delay with a selected train (and its panel), rows with
//!   and without a destination, badges of every tone and none, a speed not known;
//! - `u14-list-distance.png`, `u14-list-states.png`: sorted by distance; loading, the feeds
//!   failed, none matching with Show all;
//! - `u14-filters.png`: the filter panel with a kind and an operator left out (one no longer
//!   running), an unknown operator;
//! - `u14-panel.png`, `u14-panel-long.png`, `u14-panel-bare.png`: the panel with its stops
//!   (passed, the next with a note, cancelled and early, the last), a long name and destination
//!   with a stale report and the stops loading, a standing train with no timetable.
//!
//! The Chromium layouts were made from `TrainList.svelte`, `TrainFilterPanel.svelte` and
//! `TrainPanel.svelte` server-rendered by Svelte with the same props, laid out with app.css and the
//! Overpass fonts (Playwright's Chromium, scale 1); their numbers are written into the checks.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes.

mod common;

use common::NOW_MS;
use common::shots::{Rect, bounds, gpu, pixel, save};
use egui::{Color32, Pos2, RawInput, Vec2};
use rustradar_app::dev::demo_trains;
use rustradar_app::state::shortcuts::Open;
use rustradar_app::state::aircraft::Answer;
use rustradar_app::state::trains::{FIXTURE_TRAINS, TrainFeeds, TrainStream, Trains};
use rustradar_app::state::{Action, AppState, Startup};
use rustradar_app::ui::shell::{self, Placed, Slots};
use rustradar_app::ui::train_panel::FIXTURE_STOPS;
use rustradar_app::ui::{lists, theme};
use rustradar_core::geo::{Area, Basemap};
use rustradar_core::model::{StopTime, TrafikverketKeyStatus as KeyStatus, Train, TrainKind, TrainRoute, TrainStop};
use rustradar_core::settings::Mode;
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_scene::camera::ViewState;

/// The desktop reference pictures' canvas, CSS px, at scale 1.
const SIZE: [u32; 2] = [1440, 900];
/// V3: the demo trains run through its centre.
const V3: ViewState = ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, bearing: 0.0, pitch: 0.0, position: None };
/// The shots' clock in Unix seconds.
const S: i64 = (NOW_MS / 1000.0) as i64;

/// egui alone onto a target cleared to `--ink`, as the window's UI pass draws it.
struct Canvas {
    ctx: egui::Context,
    egui: egui_wgpu::Renderer,
    target: Offscreen,
    size: [u32; 2],
}

impl Canvas {
    fn new(gpu: &Gpu) -> Self {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let egui = egui_wgpu::Renderer::new(&gpu.device, FORMAT, egui_wgpu::RendererOptions::default());
        Canvas { ctx, egui, target: Offscreen::new(gpu, SIZE), size: SIZE }
    }

    /// A frame of the shell with the app's slots for `state` at egui time `time` with `events`.
    fn frame(&mut self, gpu: &Gpu, time: f64, state: &AppState, events: Vec<egui::Event>) -> (Placed, Vec<Action>) {
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
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("u14") });
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
    /// heights from the last frame's measures, the panel enters over 200 ms), then the picture and
    /// the last frame's actions.
    fn settle(&mut self, gpu: &Gpu, state: &AppState) -> (Placed, Vec<Action>, Image) {
        let mut placed = Placed::default();
        let mut actions = Vec::new();
        for time in [0.0, 1.0, 2.0, 3.0] {
            (placed, actions) = self.frame(gpu, time, state, Vec::new());
        }
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        (placed, actions, self.target.read(gpu).expect("read back"))
    }
}

// ------------------------------------------------------------------------------------------------
// fixtures

/// A folder of the test's own.
struct Folder(std::path::PathBuf);

impl Folder {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rustradar-u14-{name}-{}", std::process::id()));
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

/// The demo trains through V3's centre at the shots' clock, `edit` given each.
fn trains(edit: impl Fn(usize, &mut Train)) -> Vec<Train> {
    let mut trains = demo_trains(V3.latitude, V3.longitude, NOW_MS as i64);
    for (i, t) in trains.iter_mut().enumerate() {
        edit(i, t);
    }
    trains
}

/// The list's trains of Chromium's layout: destinations, delays of each tone, no speed.
fn list_trains() -> Vec<Train> {
    trains(|i, t| match i {
        0 => {
            t.destination = Some("Malmö C".into());
            t.delay_minutes = Some(7);
        }
        1 => {
            t.destination = Some("Göteborg Central via Skövde and Herrljunga".into());
            t.delay_minutes = Some(34);
        }
        3 => t.delay_minutes = Some(-2),
        4 => {
            t.delay_minutes = None;
            t.speed_kmh = None;
        }
        _ => {}
    })
}

fn time(planned: Option<i64>, estimated: Option<i64>, actual: Option<i64>) -> Option<StopTime> {
    Some(StopTime { planned, estimated, actual })
}

/// The stops of Chromium's layout: two passed (late), the next late with a note, one cancelled and
/// early, the last.
fn stops() -> Vec<TrainStop> {
    let stop = |name: &str, arrival, departure, track: Option<&str>, canceled, notes: &[&str]| TrainStop {
        name: name.into(),
        arrival,
        departure,
        track: track.map(Into::into),
        canceled,
        notes: notes.iter().map(|n| (*n).to_owned()).collect(),
        ..Default::default()
    };
    vec![
        stop("Stockholm Central", None, time(Some(S - 1800), None, Some(S - 1740)), Some("14"), false, &[]),
        stop("Södertälje Syd", time(Some(S - 600), None, Some(S - 540)), time(Some(S - 540), None, Some(S - 480)), Some("3"), false, &[]),
        stop("Katrineholm Central", time(Some(S + 1140), Some(S + 1380), None), time(Some(S + 1200), Some(S + 1440), None), Some("2"), false, &["Signal failure near Flen"]),
        stop("Norrköping Central", time(Some(S + 3000), None, None), time(Some(S + 3060), Some(S + 2940), None), None, true, &[]),
        stop("Linköping Central", time(Some(S + 4500), None, None), None, Some("1a"), false, &[]),
    ]
}

/// A state on `dir` with `trains` answered, trains the focus, polled until they are in.
fn state(dir: &Folder, trains: &[Train]) -> AppState {
    dir.write(FIXTURE_TRAINS, serde_json::to_vec(trains).unwrap());
    let startup = Startup { basemap: Some(Basemap::Dark), fixture: Some(dir.0.clone()), ..Default::default() };
    let mut s = AppState::new(startup, None, None, None, V3, SIZE.map(f64::from));
    s.persisted.settings.mode = Mode::Trains;
    for t in [NOW_MS, NOW_MS + 16.0, NOW_MS + 32.0] {
        s.poll(t);
    }
    assert!(s.trains.loaded(), "the fixture's trains are in");
    s
}

/// The index of the train with `id` in the loaded trains.
fn index(state: &AppState, id: &str) -> usize {
    state.trains.trains().iter().position(|t| t.id == id).expect("a loaded train")
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

fn rect(r: egui::Rect) -> Rect {
    Rect::new(r.left().round() as u32, r.top().round() as u32, r.width().round() as u32, r.height().round() as u32)
}

#[track_caller]
fn near(got: f32, want: f32, tolerance: f32, what: &str) {
    assert!((got - want).abs() <= tolerance, "{what}: {got} for {want}");
}

// ------------------------------------------------------------------------------------------------
// the list

/// The colour of `c` at `alpha` over `--surface`, as the browser blends it.
fn over_surface(c: Color32) -> Color32 {
    let a = f32::from(c.a()) / 255.0;
    let (r, g, b) = c.to_tuple_unmultiplied_rgb();
    let mix = |v: u8, s: u8| (f32::from(s) + (f32::from(v) - f32::from(s)) * a).round() as u8;
    Color32::from_rgb(mix(r, 50), mix(g, 50), mix(b, 50))
}

trait Unmultiplied {
    fn to_tuple_unmultiplied_rgb(&self) -> (u8, u8, u8);
}

impl Unmultiplied for Color32 {
    fn to_tuple_unmultiplied_rgb(&self) -> (u8, u8, u8) {
        let [r, g, b, _] = self.to_srgba_unmultiplied();
        (r, g, b)
    }
}

fn row(ctx: &egui::Context, id: &str) -> egui::Rect {
    ctx.read_response(egui::Id::unique(("rustradar::lists::trains::row", id))).expect("the row").rect
}

#[test]
fn the_list_sorted_by_delay_with_a_train_picked() {
    let Some(gpu) = gpu() else { return };
    let dir = Folder::new("list");
    let mut state = state(&dir, &list_trains());
    let picked = index(&state, "trafikverket:demo:2");
    assert!(state.apply(Action::PickTrain(picked), NOW_MS + 40.0));
    let mut canvas = Canvas::new(&gpu);
    let (placed, actions, image) = canvas.settle(&gpu, &state);
    save(&image, "u14-list.png");
    assert!(actions.is_empty(), "drawing alone asks nothing: {actions:?}");
    let card = placed.sidebar.list.expect("the list card");
    // Chromium: the card 442.64 high (12 + 27.59 + 42.19 + five rows of 53.3, one of 52.99, the
    // coverage note 27.39, 12, the borders)
    near(card.width(), 320.0, 0.01, "the card's width");
    near(card.height(), 442.64, 0.6, "the card's height");
    // the rows in Chromium's order and places, from the card's top
    let order = ["trafikverket:demo:1", "trafikverket:demo:0", "trafikverket:demo:5", "trafikverket:demo:2", "trafikverket:demo:3", "trafikverket:demo:4"];
    let tops = [82.78, 136.08, 189.38, 242.67, 295.97, 349.27];
    for (id, top) in order.iter().zip(tops) {
        let r = row(&canvas.ctx, id);
        near(r.top() - card.top(), top, 0.3, id);
        near(r.left() - card.left(), 9.0, 0.01, id);
        near(r.width(), 302.0, 0.01, id);
    }
    near(row(&canvas.ctx, "trafikverket:demo:4").height(), 52.99, 0.02, "a row without a badge");
    // the selected row on --amber-soft (Chromium: (51,67,84)), its name in --amber
    let selected = rect(row(&canvas.ctx, "trafikverket:demo:2"));
    let soft = Color32::from_rgb(51, 67, 84);
    let fill = painted(&image, selected, soft).expect("the selected row's fill");
    assert!(fill.width() >= 300 && fill.height() >= 52, "{fill:?}");
    assert!(painted(&image, Rect::new(selected.x0 + 24, selected.y0 + 8, 150, 18), theme::AMBER).is_some(), "the name in the accent");
    // the badges in their tones, flush right in the card's content (Chromium: x 249.45–319)
    for (id, tone) in [("trafikverket:demo:1", theme::DANGER_SOFT), ("trafikverket:demo:0", theme::WARN_SOFT), ("trafikverket:demo:3", theme::LIVE_SOFT)] {
        let r = rect(row(&canvas.ctx, id));
        let badge = painted(&image, Rect::new(r.x0 + 200, r.y0 + 6, 102, 22), over_surface(tone)).unwrap_or_else(|| panic!("{id}'s badge"));
        near(badge.x1 as f32, card.left() + 1.0 + 8.0 + 302.0 - 8.0, 1.0, "the badge's right edge");
        near((badge.y1 - badge.y0) as f32, 18.5, 1.0, "the badge's height");
    }
    let wide = rect(row(&canvas.ctx, "trafikverket:demo:1"));
    let badge = painted(&image, Rect::new(wide.x0 + 150, wide.y0 + 6, 152, 22), over_surface(theme::DANGER_SOFT)).expect("34 min late");
    near(badge.width() as f32, 69.55, 1.5, "34 min late's width");
    // the swatches: 8 × 8 in the trains' colours, 8 in from the row
    for (id, colour) in [("trafikverket:demo:1", Color32::from_rgb(232, 232, 236)), ("trafikverket:demo:0", Color32::from_rgb(196, 92, 222)), ("trafikverket:demo:4", Color32::from_rgb(120, 214, 92))] {
        let r = rect(row(&canvas.ctx, id));
        let swatch = painted(&image, Rect::new(r.x0, r.y0, 24, 24), colour).unwrap_or_else(|| panic!("{id}'s swatch"));
        assert_eq!((swatch.x0, swatch.width(), swatch.height()), (r.x0 + 8, 8, 8), "{id}");
        near(swatch.y0 as f32 - r.y0 as f32, 13.25, 1.0, "centred on the first line");
    }
    // the header's title and count, the sort with Delay chosen
    let title = ink(&image, Rect::new(card.left() as u32 + 9, card.top() as u32 + 13, 120, 22), theme::SURFACE).expect("the title");
    near(title.x0 as f32, card.left() + 17.0, 1.5, "the title's left");
    let count = ink(&image, Rect::new(card.right() as u32 - 40, card.top() as u32 + 13, 30, 22), theme::SURFACE).expect("the count");
    near(count.x1 as f32, card.right() - 17.0, 1.5, "the count's right");
    let delay = painted(&image, Rect::new(card.left() as u32 + 9, card.top() as u32 + 40, 302, 42), theme::AMBER).expect("Delay chosen");
    near(delay.width() as f32, 68.5, 1.0, "a quarter of the sort");
}

/// No trains in view, none of them answered, and none matching: the list's messages.
#[test]
fn the_list_s_states() {
    let Some(gpu) = gpu() else { return };
    let mut canvas = Canvas::new(&gpu);
    // loading: the feeds never answer
    let mut loading = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, V3, SIZE.map(f64::from));
    loading.persisted.settings.mode = Mode::Trains;
    loading.trains = Trains::with_feeds(Some(Box::new(Silent { fail: false })));
    loading.poll(NOW_MS);
    assert_eq!(lists::trains::body(&loading, 0), lists::trains::Body::Loading);
    let (placed, _, image) = canvas.settle(&gpu, &loading);
    save(&image, "u14-list-loading.png");
    let card = placed.sidebar.list.expect("the list card");
    // Chromium: 335.78 (12 + 27.59 + 42.19 + six rows of 40 + 12 and the borders)
    near(card.height(), 335.78, 0.6, "the loading card");
    let bars = bounds(&image, Rect::new(card.left() as u32 + 2, card.top() as u32 + 83, 316, 245), |p| lum(p) > lum(rgb(theme::SURFACE)) + 6.0).expect("the placeholders");
    near(bars.x0 as f32, card.left() + 17.0, 1.0, "the bars' left");
    near(bars.x1 as f32, card.right() - 17.0, 1.0, "the right bars' right");

    // failed
    let mut failed = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, V3, SIZE.map(f64::from));
    failed.persisted.settings.mode = Mode::Trains;
    failed.trains = Trains::with_feeds(Some(Box::new(Silent { fail: true })));
    // asked once the camera has stood 600 ms
    for t in [NOW_MS, NOW_MS + 16.0, NOW_MS + 700.0, NOW_MS + 716.0] {
        failed.poll(t);
    }
    assert_eq!(lists::trains::body(&failed, 0), lists::trains::Body::NoneInView { failed: true });
    let mut canvas = Canvas::new(&gpu);
    let (placed, _, image) = canvas.settle(&gpu, &failed);
    save(&image, "u14-list-failed.png");
    // Chromium: 176.95 (the message on two lines, 12 apart from the coverage line)
    near(placed.sidebar.list.expect("the list card").height(), 176.95, 0.6, "the failed card");

    // none matching: the search
    let dir = Folder::new("nomatch");
    let mut none = state(&dir, &list_trains());
    none.search.hand_over("Uppsala".into());
    assert!(none.apply(Action::Search(rustradar_app::state::search::Ask::Typed), NOW_MS + 40.0));
    let mut canvas = Canvas::new(&gpu);
    let (placed, _, image) = canvas.settle(&gpu, &none);
    save(&image, "u14-list-nomatch.png");
    let card = placed.sidebar.list.expect("the list card");
    // Chromium: 173.97 (the message, 12, the 32 px button, padded 8)
    near(card.height(), 173.97, 0.6, "the no-match card");
    let button = bounds(&image, Rect::new(card.left() as u32 + 2, card.top() as u32 + 115, 200, 40), |p| (0..3).all(|i| p[i].abs_diff(110) <= 3)).expect("Show all trains' border");
    near(button.x0 as f32, card.left() + 17.0, 1.0, "the button's left");
    near(button.width() as f32, 112.55, 1.5, "the button's width");
}

/// Train feeds that never answer, or fail at once.
struct Silent {
    fail: bool,
}

impl TrainFeeds for Silent {
    fn key_status(&mut self) -> Answer<Result<KeyStatus, String>> {
        Answer::ready(Ok(KeyStatus::Ok))
    }

    fn trains_in_view(&mut self, _area: Area, _zoom: f64) -> Answer<Result<Vec<Train>, String>> {
        if self.fail {
            return Answer::ready(Err("unreachable".into()));
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::mem::forget(tx);
        Answer::Channel(rx)
    }

    fn watch_trains(&mut self, _ids: Vec<String>) {}

    fn hide_train_operators(&mut self, _names: Vec<String>) {}

    fn follow_train(&mut self, _id: &str) -> Result<TrainStream, String> {
        Err("no stream".into())
    }

    fn unfollow_train(&mut self) {}

    fn train_route(&mut self, _id: &str) -> Answer<Result<TrainRoute, String>> {
        Answer::ready(Err("no route".into()))
    }
}

// ------------------------------------------------------------------------------------------------
// the panel

/// The panel trains: the selected one by its id, edited by `edit`, its stops `stops`.
fn panel_state(dir: &Folder, id: &str, edit: impl Fn(&mut Train), stops: Option<Vec<TrainStop>>) -> AppState {
    let all = trains(|_, t| {
        if t.id == id {
            edit(t);
        }
    });
    if let Some(stops) = stops {
        dir.write(FIXTURE_STOPS, serde_json::to_vec(&std::collections::HashMap::from([(id.to_owned(), stops)])).unwrap());
    }
    let mut state = state(dir, &all);
    let picked = index(&state, id);
    assert!(state.apply(Action::PickTrain(picked), NOW_MS + 40.0));
    state
}

/// The rows down `x` painted `--line` (the panel's rules), from `top` to `bottom`.
fn rules(image: &Image, x: u32, top: u32, bottom: u32) -> Vec<u32> {
    (top..bottom).filter(|&y| (0..3).all(|i| pixel(image, x, y)[i].abs_diff(68) <= 2) && (0..3).all(|i| pixel(image, x + 3, y)[i].abs_diff(68) <= 2)).collect()
}

#[test]
fn the_panel_with_its_stops() {
    let Some(gpu) = gpu() else { return };
    let dir = Folder::new("panel");
    let mut state = panel_state(
        &dir,
        "trafikverket:demo:1",
        |t| {
            t.destination = Some("Linköping Central".into());
            t.delay_minutes = Some(7);
        },
        Some(stops()),
    );
    state.trains.set_following(true);
    let mut canvas = Canvas::new(&gpu);
    let (placed, _, image) = canvas.settle(&gpu, &state);
    save(&image, "u14-panel.png");
    let (kind, panel) = placed.panel.expect("the panel");
    assert_eq!(kind, shell::Panel::Train);
    // Chromium: 362 × 545.38 at right 16, bottom 28
    near(panel.width(), 362.0, 0.01, "the panel's width");
    near(panel.height(), 545.38, 1.0, "the panel's height");
    near(panel.right(), 1424.0, 0.01, "right 16");
    // (the shell puts the area's top on a whole pixel)
    near(panel.bottom(), 872.0, 0.6, "bottom 28");
    // its rules: the strip (67.98), the journey (74.8), the footer's top (57 above the bottom)
    let top = panel.top();
    let found: Vec<i64> = rules(&image, panel.left() as u32 + 6, top as u32 + 2, panel.bottom() as u32 - 2).iter().map(|&y| i64::from(y)).collect();
    let want = [top + 67.98, top + 67.98 + 74.8, panel.bottom() - 1.0 - 57.0].map(|y| y.floor() as i64);
    assert_eq!(found.len(), 3, "{found:?}");
    for (f, w) in found.iter().zip(want) {
        assert!((f - w).abs() <= 1, "rules {found:?} for {want:?}");
    }
    // the moving cell's rule: 84.39 from the strip's close column
    let cell = bounds(&image, Rect::new(panel.left() as u32 + 200, top as u32 + 10, 120, 50), |p| (0..3).all(|i| p[i].abs_diff(68) <= 2)).expect("the cell's rule");
    near(cell.x0 as f32, panel.right() - 1.0 - 40.0 - 84.39, 1.0, "the moving cell's left");
    // the speed in the accent
    assert!(painted(&image, Rect::new(cell.x0 + 5, top as u32 + 12, 70, 22), theme::AMBER).is_some(), "79 km/h");
    // the dots (Chromium's centres, 26 in): passed, passed, the next, cancelled, the last
    let centres = [196.06, 224.25, 261.82, 299.57, 327.92];
    let x = panel.left() as u32 + 26;
    let at = |dy: f64| pixel(&image, x, (f64::from(top) + dy).round() as u32);
    for (dy, want) in centres.iter().zip([theme::TEXT_3, theme::TEXT_3, theme::AMBER, theme::SURFACE, theme::SURFACE]) {
        let got = at(*dy);
        assert!((0..3).all(|i| got[i].abs_diff(rgb(want)[i]) <= 3), "the dot at {dy}: {got:?} for {want:?}");
    }
    // the rings of the stops ahead: --line-control round --surface
    let ring = pixel(&image, x - 6, (f64::from(top) + centres[3]).round() as u32);
    assert!((0..3).all(|i| ring[i].abs_diff(110) <= 12), "{ring:?}");
    // the line between the passed stops in --text-3, past the next one --line-control (23–25 in)
    let line = |dy: f64| pixel(&image, panel.left() as u32 + 24, (f64::from(top) + dy).round() as u32);
    assert!((0..3).all(|i| line(210.0)[i].abs_diff(168) <= 3), "{:?}", line(210.0));
    assert!((0..3).all(|i| line(282.0)[i].abs_diff(110) <= 3), "{:?}", line(282.0));
    // the times: late in --warn, early in --live
    let when = Rect::new(panel.right() as u32 - 140, top as u32 + 180, 125, 160);
    assert!(painted(&image, when, theme::WARN).is_some() && painted(&image, when, theme::LIVE).is_some());
    // Follow pressed: its accent border
    let footer = Rect::new(panel.left() as u32 + 100, panel.bottom() as u32 - 50, 260, 40);
    let follow = painted(&image, footer, theme::AMBER).expect("Follow pressed");
    near(follow.width() as f32, 85.58, 1.5, "Follow's width");
    near(follow.x0 as f32, panel.right() - 1.0 - 16.0 - 119.67 - 8.0 - 85.58, 1.5, "Follow's left");
}

#[test]
fn the_panel_long_and_bare() {
    let Some(gpu) = gpu() else { return };
    let dir = Folder::new("panel-long");
    let state = panel_state(
        &dir,
        "trafikverket:demo:0",
        |t| {
            t.name = "Arlanda Express Stockholm 7041".into();
            t.speed_kmh = None;
            t.heading = None;
            t.frames.clear();
            t.destination = Some("Arlanda Terminal 2".into());
            t.delay_minutes = Some(34);
            t.position_unix = S - 600;
        },
        None,
    );
    let mut canvas = Canvas::new(&gpu);
    let (placed, _, image) = canvas.settle(&gpu, &state);
    save(&image, "u14-panel-long.png");
    let (_, panel) = placed.panel.expect("the panel");
    // Chromium: 399.19 high (the strip 65.05 with the name at 17 px, the stops' message)
    near(panel.height(), 399.19, 1.0, "the long panel's height");
    let found = rules(&image, panel.left() as u32 + 6, panel.top() as u32 + 2, panel.bottom() as u32 - 2);
    assert!((i64::from(found[0]) - (panel.top() + 65.05).floor() as i64).abs() <= 1, "{found:?}");
    // no moving cell: no rule down the strip (a column of --line the strip's height)
    let column = |x: u32| (panel.top() as u32 + 4..panel.top() as u32 + 60).all(|y| (0..3).all(|i| pixel(&image, x, y)[i].abs_diff(68) <= 2));
    assert!(!(panel.left() as u32 + 30..panel.right() as u32 - 2).any(column), "a bare strip");
    // the stale report's warn badge (12 px: 20 high) in the footer
    let badge = painted(&image, Rect::new(panel.left() as u32 + 10, panel.bottom() as u32 - 50, 140, 40), over_surface(theme::WARN_SOFT)).expect("the stale badge");
    near(badge.height() as f32, 20.0, 1.0, "the badge's height");
    near(badge.x0 as f32, panel.left() + 17.0, 1.0, "the badge's left");

    let dir = Folder::new("panel-bare");
    let state = panel_state(
        &dir,
        "trafikverket:demo:4",
        |t| {
            t.speed_kmh = None;
            t.heading = None;
            t.frames.clear();
            t.delay_minutes = None;
            t.operator = None;
        },
        Some(Vec::new()),
    );
    let mut canvas = Canvas::new(&gpu);
    let (placed, _, image) = canvas.settle(&gpu, &state);
    save(&image, "u14-panel-bare.png");
    let (_, panel) = placed.panel.expect("the panel");
    // Chromium: 324.39 high (no journey, no timetable)
    near(panel.height(), 324.39, 1.0, "the bare panel's height");
}

// ------------------------------------------------------------------------------------------------
// the filter panel

#[test]
fn the_filter_panel() {
    let Some(gpu) = gpu() else { return };
    let dir = Folder::new("filters");
    let mut state = state(&dir, &trains(|i, t| if i == 5 { t.operator = None }));
    state.persisted.train_filters.hidden_kinds = vec![TrainKind::Suburban];
    state.persisted.train_filters.hidden_operators = vec!["Arriva".into()];
    assert!(state.apply(Action::Open(Open::Filters, true), NOW_MS + 40.0));
    // in today's window: the panel gives way to the list's least height and scrolls its body
    let mut canvas = Canvas::new(&gpu);
    let (placed, _, image) = canvas.settle(&gpu, &state);
    save(&image, "u14-filters.png");
    let filters = placed.sidebar.filters.expect("the filter panel");
    let list = placed.sidebar.list.expect("the list card");
    assert!(list.top() - filters.bottom() >= 10.9, "12 apart: {filters:?} {list:?}");
    // egui's debug builds mark an edge off the pixel grid in orange: none
    assert!(bounds(&image, Rect::whole(&image), |p| p[0] > 230 && (140..180).contains(&p[1]) && p[2] < 40).is_none(), "no unaligned edge");

    // on a canvas tall enough for all of it: Chromium's natural height and places
    let mut tall = Canvas::new(&gpu);
    tall.size = [1440, 1200];
    tall.target = Offscreen::new(&gpu, tall.size);
    let mut state = state;
    state.controller.set_size([1440.0, 1200.0]);
    let (placed, _, image) = tall.settle(&gpu, &state);
    save(&image, "u14-filters-whole.png");
    let card = placed.sidebar.filters.expect("the filter panel");
    near(card.height(), 391.17, 1.0, "the panel's natural height");
    let top = card.top();
    // the header's rule 49 down, the chips' rows (Chromium: 94.8, 124.8; 195.59, 225.59, 255.59)
    let found = rules(&image, card.left() as u32 + 6, top as u32 + 2, top as u32 + 60);
    assert_eq!(found.iter().map(|&y| i64::from(y) - top as i64).collect::<Vec<_>>(), [49], "the header's rule");
    // a chip's top border: --line-control with the chip's inside under it and its bottom border 25 down
    let control = |y: u32| (0..3).all(|i| pixel(&image, card.left() as u32 + 30, y)[i].abs_diff(110) <= 6);
    let chip_rows: Vec<u32> = (top as u32 + 60..top as u32 + 300).filter(|&y| control(y) && !control(y - 1) && control(y + 25)).collect();
    let want = [94.8, 124.8, 195.59, 225.59, 255.59];
    assert_eq!(chip_rows.len(), want.len(), "the chips' top borders: {chip_rows:?}");
    for (got, want) in chip_rows.iter().zip(want) {
        near(*got as f32 - top, want, 1.0, "a row of chips");
    }
    // the switch (off: --line-control) at Chromium's place: 269 in, 346.67 down, 34 × 20 (its
    // middle, clear of the rounded ends' blending)
    let switch = painted(&image, Rect::new(card.left() as u32 + 250, top as u32 + 330, 66, 40), theme::LINE_CONTROL).expect("the switch");
    near(switch.x0 as f32 - card.left(), 269.0, 1.0, "the switch's left");
    // (Chromium's own pixels of it: rows 347–366 under the card's top, the middle at 357)
    near((switch.y0 + switch.y1) as f32 / 2.0 - top, 357.0, 1.0, "the switch's middle");
}
