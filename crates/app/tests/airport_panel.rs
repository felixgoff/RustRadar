//! Item U13's acceptance (plan stage 7; doc 06 §3.6, §8.3): the airport board as the shell draws
//! it, from a `--fixture` folder (`airport_board/ARN-{mode}-1.json` and the reference data).
//!
//! - `departures` (`target/shots/u13-board-departures.png`): Stockholm Arlanda at 12:00 CEST,
//!   page 1 of 3: departed, boarding, estimated, scheduled and cancelled flights in their tones,
//!   live ones with the dot, third lines where there is something, `Tomorrow` before tomorrow's
//!   rows, Show more departures, `Updated 12:00 · times at ARN`;
//! - `arrivals` (`u13-board-arrivals.png`): the other tab, `Yesterday` before the first row and
//!   `Today` after it, a callsign for a missing number, `—` and `From ?` for a row with nothing,
//!   the selected flight on `--amber-soft`, no Show more on the last page;
//! - `loading` and `failed` (`u13-board-loading.png`, `u13-board-failed.png`): the seven
//!   placeholder rows; the message and Try again.
//!
//! Each was compared by eye, part by part, with Chromium's rendering of today's
//! `AirportPanel.svelte` given the same board (server-rendered with Svelte, its state set to the
//! fixture's, laid out with app.css at 1440 × 900); the checks hold the places Chromium gives.
//! The interaction tests drive the panel through egui: a live row selects its flight from the
//! board, a row that is not live does nothing, the tabs (a click, and Left and Right on the focused
//! tab), Show more, Show on map and the close button ask what the module documentation says.
//!
//! The GPU tests need a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without
//! one they print `skipped: …` and pass.

mod common;

use std::path::{Path, PathBuf};

use common::NOW_MS;
use common::shots::{Rect, bounds, gpu, pixel, save};
use egui::{Color32, Event, PointerButton, Pos2, RawInput, Vec2};
use rustradar_app::state::airport::{Ask, FIXTURE_BOARD};
use rustradar_app::state::flight::SelectOptions;
use rustradar_app::state::{Action, AppState, Startup, aircraft};
use rustradar_app::ui::airport_panel::{self, Body};
use rustradar_app::ui::shell::{self, Placed, Slots};
use rustradar_app::ui::{anim, theme};
use rustradar_core::geo::Basemap;
use rustradar_core::model::{Airport, Board, BoardMode, BoardRow, ReferenceData};
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_scene::camera::ViewState;

/// The desktop reference pictures' canvas, CSS px, at scale 1.
const SIZE: [u32; 2] = [1440, 900];
/// The shots' clock in seconds: 2026-06-21 10:00 UTC, 12:00 in Stockholm.
const NOW: i64 = NOW_MS as i64 / 1000;
/// The live departure (SK1415 to Copenhagen) and the selected arrival (LH808 from Munich).
const DEPARTED: u32 = 0x03a1_b2c3;
const SELECTED: u32 = 0x03a1_c002;

/// The pixels `x0..x1` × `y0..y1`.
const fn area(x0: u32, y0: u32, x1: u32, y1: u32) -> Rect {
    Rect { x0, y0, x1, y1 }
}

// ------------------------------------------------------------------------------------------------
// the fixture

fn some(s: &str) -> Option<String> {
    Some(s.to_owned())
}

#[allow(clippy::too_many_arguments)]
fn flight(id: Option<u32>, number: &str, airline: &str, to: (&str, &str, &str), at: i64, status: (&str, &str), live: bool, extra: [&str; 4]) -> BoardRow {
    let opt = |s: &str| (!s.is_empty()).then(|| s.to_owned());
    BoardRow {
        flight_id: id,
        number: opt(number),
        airline: opt(airline),
        airport_city: opt(to.0),
        airport_name: opt(to.1),
        airport_iata: opt(to.2),
        scheduled: Some(at),
        status: opt(status.0),
        status_color: opt(status.1),
        live,
        aircraft: opt(extra[0]),
        registration: opt(extra[1]),
        terminal: opt(extra[2]),
        gate: opt(extra[3]),
        ..Default::default()
    }
}

/// Arlanda's departures, page 1 of 3.
fn departures() -> Board {
    let tomorrow = NOW + 86_400;
    let rows = vec![
        flight(Some(DEPARTED), "SK1415", "Scandinavian Airlines", ("Copenhagen", "Copenhagen Kastrup Airport", "CPH"), NOW - 1200, ("Departed 11:52", "green"), true, ["A20N", "SE-ROR", "5", "F62"]),
        flight(Some(0x03a1_b2d0), "DY4321", "Norwegian Air Shuttle", ("Oslo", "Oslo Gardermoen Airport", "OSL"), NOW + 300, ("Boarding", "yellow"), false, ["B38M", "", "5", "F58"]),
        flight(None, "LH809", "Lufthansa", ("Frankfurt", "Frankfurt Airport", "FRA"), NOW + 1200, ("Estimated dep 12:45", "yellow"), false, ["A321", "", "5", ""]),
        flight(None, "BA779", "British Airways", ("London", "London Heathrow Airport", "LHR"), NOW + 1800, ("Scheduled", "gray"), false, ["", "", "", ""]),
        flight(Some(0x03a1_b2e1), "AY648", "Finnair", ("Helsinki", "Helsinki Vantaa Airport", "HEL"), NOW + 4200, ("Canceled", "red"), false, ["AT76", "", "", ""]),
        flight(Some(0x03a1_b2f2), "KL1110", "KLM Royal Dutch Airlines", ("Amsterdam", "Amsterdam Schiphol Airport", "AMS"), NOW + 5700, ("Scheduled", "gray"), false, ["E190", "PH-EZA", "", "F34"]),
        flight(None, "SK903", "Scandinavian Airlines", ("New York", "Newark Liberty International Airport", "EWR"), tomorrow - 8100, ("Scheduled", "gray"), false, ["A339", "", "5", ""]),
        flight(None, "TK1796", "Turkish Airlines", ("", "Istanbul Airport", "IST"), tomorrow - 5400, ("Scheduled", "gray"), false, ["", "", "5", ""]),
    ];
    Board { total: Some(64), rows, page: 1, pages: Some(3) }
}

/// Arlanda's arrivals, its one page.
fn arrivals() -> Board {
    let rows = vec![
        flight(Some(0x03a1_bf00), "SK1426", "Scandinavian Airlines", ("Copenhagen", "", "CPH"), NOW - 43_800, ("Landed 00:12", "green"), false, ["A20N", "", "", ""]),
        flight(Some(0x03a1_c000), "SK1416", "Scandinavian Airlines", ("Copenhagen", "", "CPH"), NOW - 2100, ("Landed 11:21", "green"), false, ["A20N", "SE-ROA", "5", "F60"]),
        BoardRow { callsign: some("NAX12K"), ..flight(Some(0x03a1_c001), "", "", ("Bergen", "", "BGO"), NOW - 300, ("Estimated 12:04", "yellow"), true, ["B738", "", "", ""]) },
        flight(Some(SELECTED), "LH808", "Lufthansa", ("Munich", "Munich Airport", "MUC"), NOW + 900, ("Delayed 12:50", "red"), true, ["A320", "D-AIZB", "", ""]),
        flight(None, "", "", ("", "", ""), NOW + 2400, ("", ""), false, ["", "", "", ""]),
    ];
    Board { total: Some(5), rows, page: 1, pages: Some(1) }
}

fn arlanda() -> Airport {
    Airport {
        id: 1,
        name: "Stockholm Arlanda Airport".into(),
        iata: "ARN".into(),
        icao: "ESSA".into(),
        city: "Stockholm".into(),
        country: "Sweden".into(),
        lat: 59.6519,
        lon: 17.9186,
        alt: Some(137),
        size: 9,
        timezone: "Europe/Stockholm".into(),
    }
}

/// The fixture folder `name` under the tests' temporary folder, with the reference data and
/// `pages` (by `{mode}-{page}`). Each call has a folder of its own: the tests run in parallel, and
/// one that cleared a shared folder while another was writing or reading it failed with NotFound.
fn folder(name: &str, pages: &[(&str, Board)]) -> PathBuf {
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}-{call}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(FIXTURE_BOARD)).expect("the fixture folder");
    let reference = ReferenceData { fetched_at: NOW, airlines: Vec::new(), airports: vec![arlanda()] };
    std::fs::write(dir.join(aircraft::FIXTURE_REFERENCE), serde_json::to_vec(&reference).unwrap()).unwrap();
    for (key, board) in pages {
        std::fs::write(dir.join(FIXTURE_BOARD).join(format!("ARN-{key}.json")), serde_json::to_vec(board).unwrap()).unwrap();
    }
    dir
}

/// The state reading `dir`, its reference data in, Arlanda's board open at [`NOW_MS`].
fn opened(dir: &Path) -> AppState {
    let startup = Startup { basemap: Some(Basemap::Dark), fixture: Some(dir.to_path_buf()), demo: false };
    let mut s = AppState::new(startup, None, None, None, ViewState::INITIAL, SIZE.map(f64::from));
    for t in [NOW_MS - 48.0, NOW_MS - 32.0, NOW_MS - 16.0] {
        s.poll(t);
    }
    assert!(s.aircraft.reference().is_some_and(|r| r.airport(Some("ARN")).is_some()), "the reference data");
    s.open_airport("ARN", NOW_MS);
    s
}

/// The board of the full fixture (`departures` page 1, `arrivals` page 1), loaded.
fn board(mode: BoardMode) -> AppState {
    let dir = folder("u13-board", &[("departures-1", departures()), ("arrivals-1", arrivals())]);
    let mut s = opened(&dir);
    s.poll(NOW_MS);
    if mode == BoardMode::Arrivals {
        assert!(s.apply(Action::Board(Ask::Mode(BoardMode::Arrivals)), NOW_MS));
        s.poll(NOW_MS);
        // picked from the board before the feed has it: the board stays, its row marked
        s.select_flight(Some(SELECTED), SelectOptions::default(), NOW_MS);
        assert!(s.flight.selected().is_none());
    }
    assert_eq!(airport_panel::body(&s), Body::Rows);
    s
}

// ------------------------------------------------------------------------------------------------
// the shell with the panel

fn slots() -> Slots {
    Slots { airport_panel: Some(airport_panel::show), ..Slots::default() }
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    theme::apply(&ctx);
    // the skeletons stand at their start, as in Chromium's capture
    anim::set_reduced_motion(&ctx, true);
    ctx
}

/// One pass of the shell over `state` with `events`: where it put things and the actions.
fn run(ctx: &egui::Context, time: f64, state: &AppState, events: Vec<Event>) -> (Placed, Vec<Action>, egui::FullOutput) {
    let raw = RawInput {
        screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(SIZE[0] as f32, SIZE[1] as f32))),
        time: Some(time),
        events,
        ..Default::default()
    };
    let mut placed = Placed::default();
    let mut actions = Vec::new();
    let output = ctx.run_ui(raw, |ui| placed = shell::show(ui.ctx(), state, NOW_MS, &mut actions, &slots()));
    (placed, actions, output)
}

/// egui alone onto a target cleared to `--ink`, as the window's UI pass draws it.
struct Canvas {
    ctx: egui::Context,
    egui: egui_wgpu::Renderer,
    target: Offscreen,
}

impl Canvas {
    fn new(gpu: &Gpu) -> Self {
        let egui = egui_wgpu::Renderer::new(&gpu.device, FORMAT, egui_wgpu::RendererOptions::default());
        Canvas { ctx: context(), egui, target: Offscreen::new(gpu, SIZE) }
    }

    fn frame(&mut self, gpu: &Gpu, time: f64, state: &AppState) -> Placed {
        let (placed, _, output) = run(&self.ctx, time, state, Vec::new());
        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: self.target.size(), pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui.update_texture(device, queue, id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("airport panel") });
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
        placed
    }

    /// Frames at egui times 0–3 s (an area is sized on its first frame), then the picture.
    fn settle(&mut self, gpu: &Gpu, state: &AppState) -> (Placed, Image) {
        let mut placed = Placed::default();
        for time in [0.0, 1.0, 2.0, 3.0] {
            placed = self.frame(gpu, time, state);
        }
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        (placed, self.target.read(gpu).expect("read back"))
    }
}

// ------------------------------------------------------------------------------------------------
// checks

fn rgb(c: Color32) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

fn close(p: [u8; 4], c: Color32, tolerance: u8) -> bool {
    (0..3).all(|i| p[i].abs_diff(rgb(c)[i]) <= tolerance)
}

/// The panel's border box on the picture: the `--line` border's outer edges.
fn panel_box(image: &Image) -> Rect {
    bounds(image, area(1000, 0, 1440, 900), |p| close(p, theme::LINE, 2)).expect("the panel's border")
}

/// Pixels of `c` (within `tolerance` levels) in `area`.
fn count(image: &Image, area: Rect, c: Color32, tolerance: u8) -> usize {
    common::shots::count(image, area, |p| close(p, c, tolerance))
}

/// `c` over `--surface` at its alpha, as the picture shows it.
fn over_surface(c: Color32) -> Color32 {
    let a = f32::from(c.a()) / 255.0;
    let s = theme::SURFACE;
    let mix = |x: u8, y: u8| (f32::from(x) * a + f32::from(y) * (1.0 - a)).round() as u8;
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgb(mix(r, s.r()), mix(g, s.g()), mix(b, s.b()))
}

#[test]
fn departures_and_arrivals() {
    let Some(gpu) = gpu() else { return };
    let mut shots = Vec::new();
    for (mode, name) in [(BoardMode::Departures, "u13-board-departures.png"), (BoardMode::Arrivals, "u13-board-arrivals.png")] {
        let state = board(mode);
        let mut canvas = Canvas::new(&gpu);
        let (placed, image) = canvas.settle(&gpu, &state);
        save(&image, name);
        let (kind, _) = placed.panel.expect("the board's panel");
        assert_eq!(kind, shell::Panel::Airport);
        let b = panel_box(&image);
        // 360 wide inside its border at right 16, its bottom 28 above the window's, as tall as
        // Chromium lays out today's panel with this board (794.58 and 574.25 px)
        assert_eq!((b.x0, b.x1, b.y1), (1062, 1424, 872), "{mode:?}: {b:?}");
        let top = if mode == BoardMode::Departures { 77 } else { 297 };
        assert!(b.y0.abs_diff(top) <= 1, "{mode:?}: {b:?}");
        shots.push((b, image));
    }
    let Ok([(dep_box, dep), (arr_box, arr)]) = <[_; 2]>::try_from(shots) else { panic!("two shots") };

    // the strip: 68 px with its rule, and the tabs' 39.19 under it with the accent's underline
    for (image, b) in [(&dep, dep_box), (&arr, arr_box)] {
        let rule = b.y0 + 1 + 67;
        assert!(close(pixel(image, 1200, rule), theme::LINE, 2), "the strip's rule at {rule}: {:?}", pixel(image, 1200, rule));
        let tabs_rule = rule + 39;
        assert!(close(pixel(image, 1400, tabs_rule), theme::LINE, 2), "the tabs' rule at {tabs_rule}: {:?}", pixel(image, 1400, tabs_rule));
        let underline = bounds(image, area(1063, rule, 1423, tabs_rule + 1), |p| close(p, theme::AMBER, 8)).expect("the chosen tab's underline");
        assert_eq!(underline.y1, tabs_rule + 1, "over the rule: {underline:?}");
    }
    // Departures' underline under the first tab, Arrivals' under the second
    let first = bounds(&dep, area(1063, dep_box.y0 + 69, 1423, dep_box.y0 + 109), |p| close(p, theme::AMBER, 8)).expect("departures");
    let second = bounds(&arr, area(1063, arr_box.y0 + 69, 1423, arr_box.y0 + 109), |p| close(p, theme::AMBER, 8)).expect("arrivals");
    assert!(first.x0 < 1110 && second.x0 > 1150, "{first:?} {second:?}");

    // the live dot and the green of Departed, the danger of Canceled
    let rows = area(1063, dep_box.y0 + 110, 1423, 800);
    assert!(count(&dep, rows, theme::LIVE, 12) > 40, "the live dot and Departed in --live");
    assert!(count(&dep, rows, theme::DANGER, 16) > 15, "Canceled in --danger");
    assert!(count(&dep, rows, theme::WARN, 16) > 15, "Boarding in --warn");
    // the selected arrival's row on --amber-soft
    let soft = over_surface(theme::AMBER_SOFT);
    assert!(count(&arr, area(1063, arr_box.y0 + 110, 1423, 860), soft, 2) > 5000, "the selected row");
    assert_eq!(count(&dep, area(1063, dep_box.y0 + 110, 1423, 860), soft, 2), 0, "nothing selected on the departures");
}

#[test]
fn loading_and_failed() {
    let Some(gpu) = gpu() else { return };
    // loading: open, nothing asked yet
    let dir = folder("u13-board-failed", &[]);
    let mut s = opened(&dir);
    assert_eq!(airport_panel::body(&s), Body::Loading);
    let mut canvas = Canvas::new(&gpu);
    let (_, image) = canvas.settle(&gpu, &s);
    save(&image, "u13-board-loading.png");
    let b = panel_box(&image);
    assert_eq!((b.x0, b.x1, b.y1), (1062, 1424, 872), "{b:?}");
    // Chromium's panel: 434.17 px high, its top at 437.83
    assert!(b.y0.abs_diff(437) <= 1, "{b:?}");
    // seven rows of three skeletons 12 px high, from --surface-raised to --surface-hover
    let skeletons = area(1063, b.y0 + 110, 1423, 813);
    let shimmer = common::shots::count(&image, skeletons, |p| (60..=76).contains(&p[0]) && p[0] == p[1] && p[1] == p[2]);
    let least: usize = (0..airport_panel::SKELETON_ROWS).map(|i| airport_panel::skeleton_widths(i).iter().sum::<f32>() as usize * 10).sum();
    assert!(shimmer > least, "the placeholders: {shimmer} of at least {least}");
    // failed: the message and Try again
    s.poll(NOW_MS);
    assert_eq!(airport_panel::body(&s), Body::Failed);
    let mut canvas = Canvas::new(&gpu);
    let (_, image) = canvas.settle(&gpu, &s);
    save(&image, "u13-board-failed.png");
    let b = panel_box(&image);
    assert_eq!((b.x0, b.x1, b.y1), (1062, 1424, 872), "{b:?}");
    // Chromium's panel: 268.36 px high (the message 86.19 in the board's padding)
    assert!(b.y0.abs_diff(603) <= 1, "{b:?}");
}

// ------------------------------------------------------------------------------------------------
// interaction

/// Passes of the shell at egui times 0–3 s: the panel laid out.
fn settle(ctx: &egui::Context, state: &AppState) -> Placed {
    let mut placed = Placed::default();
    for time in [0.0, 1.0, 2.0, 3.0] {
        let (p, _, mut output) = run(ctx, time, state, Vec::new());
        output.textures_delta.clear();
        placed = p;
    }
    placed
}

/// One pass over `events`: the actions.
fn actions(ctx: &egui::Context, time: f64, state: &AppState, events: Vec<Event>) -> Vec<Action> {
    let (_, actions, mut output) = run(ctx, time, state, events);
    output.textures_delta.clear();
    actions
}

/// A click at `at`: the pointer there, pressed, then released; the actions.
fn click(ctx: &egui::Context, state: &AppState, at: Pos2, time: f64) -> Vec<Action> {
    let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
    let mut all = actions(ctx, time, state, vec![Event::PointerMoved(at)]);
    all.extend(actions(ctx, time + 0.05, state, vec![button(true)]));
    all.extend(actions(ctx, time + 0.1, state, vec![button(false)]));
    all
}

/// The row `i`'s rectangle, as the panel answered for it.
fn row_rect(ctx: &egui::Context, i: usize) -> egui::Rect {
    ctx.read_response(egui::Id::unique("rustradar::airport_panel").with(("row", i))).expect("the row").rect
}

#[test]
fn a_live_row_selects_its_flight_and_the_buttons_ask() {
    let ctx = context();
    let mut s = board(BoardMode::Departures);
    let placed = settle(&ctx, &s);
    let (_, panel) = placed.panel.expect("the panel");
    // the live departure selects its flight from the board; one that is not live does nothing
    let live = row_rect(&ctx, 0);
    assert!((live.width() - 344.0).abs() < 0.01, "{live:?}");
    assert_eq!(click(&ctx, &s, live.center(), 4.0), [Action::Board(Ask::Pick(DEPARTED))]);
    let boarding = row_rect(&ctx, 1);
    assert!(click(&ctx, &s, boarding.center(), 5.0).is_empty(), "not live: disabled");
    // the tabs: Arrivals
    let arrivals_tab = Pos2::new(panel.left() + 1.0 + 12.0 + 120.0 + 30.0, panel.top() + 1.0 + 68.0 + 20.0);
    assert_eq!(click(&ctx, &s, arrivals_tab, 6.0), [Action::Board(Ask::Mode(BoardMode::Arrivals))]);
    // the close button, in the strip's last 40 px
    let close_button = Pos2::new(panel.right() - 1.0 - 20.0, panel.top() + 1.0 + 34.0);
    assert_eq!(click(&ctx, &s, close_button, 7.0), [Action::Board(Ask::Close)]);
    // Show on map, the footer's button: to Arlanda at zoom 11
    let locate = Pos2::new(panel.right() - 1.0 - 16.0 - 40.0, panel.bottom() - 1.0 - 28.0);
    assert_eq!(click(&ctx, &s, locate, 8.0), [Action::FlyTo { longitude: 17.9186, latitude: 59.6519, zoom: Some(11.0) }]);
    // Show more departures, under the last row
    let last = row_rect(&ctx, 7);
    let more = Pos2::new(last.left() + 8.0 + 40.0, last.bottom() + 8.0 + 16.0);
    assert_eq!(click(&ctx, &s, more, 9.0), [Action::Board(Ask::More)]);

    // picked: the flight from the board, the board kept under it
    assert!(s.apply(Action::Board(Ask::Pick(DEPARTED)), NOW_MS + 1000.0));
    assert_eq!(s.flight.selected_id(), Some(DEPARTED));
    assert!(s.flight.from_board());
    assert_eq!(s.airport.code(), Some("ARN"));
}

/// The tabs' keys through the shell: the chosen tab focused, Right asks for the other tab and Left
/// back (`tabKeys`), and the shell's own shortcuts do not take the arrows from the focused tab.
#[test]
fn left_and_right_toggle_the_tabs() {
    let ctx = context();
    let departures = board(BoardMode::Departures);
    let placed = settle(&ctx, &departures);
    let (_, panel) = placed.panel.expect("the panel");
    let key = |key: egui::Key| Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
    // a click on the chosen tab asks nothing; Tab brings the focus to it (the close button first)
    let departures_tab = Pos2::new(panel.left() + 1.0 + 12.0 + 50.0, panel.top() + 1.0 + 68.0 + 20.0);
    assert!(click(&ctx, &departures, departures_tab, 4.0).is_empty(), "already shown");
    let on_tab = |ctx: &egui::Context| ctx.memory(|m| m.focused()).and_then(|id| ctx.read_response(id)).is_some_and(|r| r.rect.contains(departures_tab));
    let mut time = 4.1;
    for _ in 0..8 {
        if on_tab(&ctx) {
            break;
        }
        time += 0.02;
        assert!(actions(&ctx, time, &departures, vec![key(egui::Key::Tab)]).is_empty());
    }
    assert!(on_tab(&ctx), "the chosen tab in the tab order");
    assert_eq!(actions(&ctx, time + 0.02, &departures, vec![key(egui::Key::ArrowRight)]), [Action::Board(Ask::Mode(BoardMode::Arrivals))]);
    // the arrivals shown, the focus moved with them: Left asks for the departures
    let mut arrivals = board(BoardMode::Departures);
    assert!(arrivals.apply(Action::Board(Ask::Mode(BoardMode::Arrivals)), NOW_MS));
    actions(&ctx, time + 0.04, &arrivals, Vec::new());
    assert_eq!(actions(&ctx, time + 0.06, &arrivals, vec![key(egui::Key::ArrowLeft)]), [Action::Board(Ask::Mode(BoardMode::Departures))]);
    // Up and Down are not the tabs' keys
    assert!(actions(&ctx, time + 0.08, &arrivals, vec![key(egui::Key::ArrowDown)]).is_empty());
}

/// A board taller than the window: the panel stops 16 px from the top (`max-height: calc(100% -
/// 16px - 28px)`), the strip, the tabs and the footer keep their places and the board scrolls
/// between them.
#[test]
fn a_long_board_scrolls_between_the_tabs_and_the_footer() {
    let Some(gpu) = gpu() else { return };
    let mut long = departures();
    let first = long.rows[0].clone();
    long.rows = (0..30).map(|i| BoardRow { flight_id: Some(0x3a20_0000 + i), number: Some(format!("SK{}", 100 + i)), scheduled: Some(NOW + 300 * i64::from(i)), ..first.clone() }).collect();
    let dir = folder("u13-board-long", &[("departures-1", long)]);
    let mut s = opened(&dir);
    s.poll(NOW_MS);
    assert_eq!(s.airport.rows().map(<[BoardRow]>::len), Some(30));
    let mut canvas = Canvas::new(&gpu);
    let (_, image) = canvas.settle(&gpu, &s);
    save(&image, "u13-board-long.png");
    let b = panel_box(&image);
    assert_eq!((b.x0, b.y0, b.x1, b.y1), (1062, 14, 1424, 872), "856 px of content and the border: {b:?}");
    // the strip's rule, the tabs' rule, and the footer's 57 px at the bottom (Chromium: 814)
    assert!(close(pixel(&image, 1200, 14 + 1 + 67), theme::LINE, 2), "the strip's rule");
    assert!(close(pixel(&image, 1400, 14 + 1 + 67 + 39), theme::LINE, 2), "the tabs' rule");
    assert!(close(pixel(&image, 1200, 814), theme::LINE, 2), "the footer's rule: {:?}", pixel(&image, 1200, 814));
    // the footer's button where it stands on a short board
    assert!(count(&image, area(1300, 820, 1420, 866), theme::LINE_CONTROL, 3) > 100, "Show on map's border");
}
