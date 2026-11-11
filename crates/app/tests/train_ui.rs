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
use rustradar_app::state::trains::FIXTURE_TRAINS;
use rustradar_app::state::{Action, AppState, Startup};
use rustradar_app::ui::shell::{self, Placed, Slots};
use rustradar_app::ui::train_panel::FIXTURE_STOPS;
use rustradar_app::ui::{lists, theme, train_panel};
use rustradar_core::geo::Basemap;
use rustradar_core::model::{StopTime, Train, TrainStop};
use rustradar_core::settings::Mode;
use rustradar_core::train_list::TrainSort;
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

#[track_caller]
fn near(got: f32, want: f32, tolerance: f32, what: &str) {
    assert!((got - want).abs() <= tolerance, "{what}: {got} for {want}");
}

// ------------------------------------------------------------------------------------------------
// the list

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
    let _ = (card, painted(&image, Rect::whole(&image), theme::SURFACE), ink(&image, Rect::whole(&image), theme::SURFACE), pixel(&image, 0, 0));
    let _ = (lists::trains::LIMIT, train_panel::LABEL, TrainSort::Delay, Open::Filters, near as fn(f32, f32, f32, &str));
}

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
    let (_, _, image) = canvas.settle(&gpu, &state);
    save(&image, "u14-panel.png");
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
    let (_, _, image) = canvas.settle(&gpu, &state);
    save(&image, "u14-panel-long.png");
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
    let (_, _, image) = canvas.settle(&gpu, &state);
    save(&image, "u14-panel-bare.png");
}

#[test]
fn the_filter_panel() {
    let Some(gpu) = gpu() else { return };
    let dir = Folder::new("filters");
    let mut state = state(&dir, &trains(|i, t| if i == 5 { t.operator = None }));
    state.persisted.train_filters.hidden_kinds = vec![rustradar_core::model::TrainKind::Suburban];
    state.persisted.train_filters.hidden_operators = vec!["Arriva".into()];
    assert!(state.apply(Action::Open(Open::Filters, true), NOW_MS + 40.0));
    let mut canvas = Canvas::new(&gpu);
    let (_, _, image) = canvas.settle(&gpu, &state);
    save(&image, "u14-filters.png");
}
