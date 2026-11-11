//! Item U11's acceptance (plan stage 7; doc 06 §7.2–7.4): the flight panel's Route, Aircraft and
//! History tabs as the shell draws them, from fixture state, one shot per tab and state.
//!
//! - `route-estimated` (`target/shots/u11-route-estimated.png`): the app's estimate with no
//!   runways, not signed in: `Estimated`, AIRAC 2606, the explanation to the airport, `Sign in to
//!   Flightradar24`, the distances, the map key's gradient and the two dashed lines;
//! - `route-runways` (`u11-route-runways.png`): runways known and OpenSky's start of the track,
//!   signed in: the explanation to a runway, the OpenSky line under the gradient, the runways in
//!   the key; `route-runways-scrolled` the same scrolled 120 px, where the runways' credit shows
//!   above the footer as today;
//! - `route-filed` (`u11-route-filed.png`): Flightradar24's flight plan: `Filed flight plan`, the
//!   route string on its `--ink` box cut by the panel's edge, waypoints, alternates, the note, the
//!   solid line;
//! - `aircraft` (`u11-aircraft.png`): the eight facts;
//! - `history` (`u11-history.png`): four flights, the one shown on `--amber-soft`, statuses in
//!   their tones; `history-loading` and `history-failed` the tab's other states.
//!
//! Each was compared, part by part, with Chromium's rendering of today's `FlightPanel.svelte`
//! given the same props (server-rendered with Svelte, laid out with app.css at 1440 × 900; the
//! props are written to `target/shots/u11-props.json`). The tests check the places and colours
//! the module documentation states; the gradient's pixels against CSS's stops.
//!
//! The GPU tests need a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without
//! one they print `skipped: …` and pass.

mod common;

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use common::NOW_MS;
use common::shots::{Rect, bounds, gpu, pixel, save};
use egui::epaint::{ClippedShape, Shape};
use egui::{Color32, Event, PointerButton, Pos2, RawInput, Vec2};
use rustradar_app::state::aircraft::{Aircraft, Answer, Feeds, LoadedReference};
use rustradar_app::state::flight::{AdsbQuery, Flight, FlightFeeds, FollowState, FollowStream, SelectOptions};
use rustradar_app::state::shortcuts::Open;
use rustradar_app::state::{Action, AppState, Startup};
use rustradar_app::ui::flight_panel::{self, Tab, View, history, route};
use rustradar_app::ui::shell::{self, Placed, Slots};
use rustradar_app::ui::{anim, theme};
use rustradar_core::geo::Basemap;
use rustradar_core::model::{
    AdsbSnapshot, AircraftInfo, Airline, Airport, BoundingBox, FlightDetails, FlightInfo, FlightPlan, FlightProgress, FollowEvent, HistoryRow,
    LiveFlight, LiveSnapshot, OpenSkyPoint, ReferenceData, Runway, RunwayEnd, ScheduleInfo, SessionInfo, TopFlight, TrailPoint,
};
use rustradar_core::routes::RouteKind;
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_scene::camera::ViewState;

/// The desktop reference pictures' canvas, CSS px, at scale 1.
const SIZE: [u32; 2] = [1440, 900];
/// The flight (SAS1417, Stockholm to Copenhagen) and its aircraft's address.
const ID: u32 = 0x3a8f_21c4;
const HEX: u32 = 0x4a_ca8f;

/// The pixels `x0..x1` × `y0..y1`.
const fn area(x0: u32, y0: u32, x1: u32, y1: u32) -> Rect {
    Rect { x0, y0, x1, y1 }
}

// ------------------------------------------------------------------------------------------------
// fixture state

/// An airport: id, name, codes (IATA, ICAO), city, zone, place (lat, lon).
fn airport(id: u32, name: &str, (iata, icao): (&str, &str), city: &str, timezone: &str, (lat, lon): (f64, f64)) -> Airport {
    Airport { id, name: name.into(), iata: iata.into(), icao: icao.into(), city: city.into(), timezone: timezone.into(), lat, lon, size: 100, ..Default::default() }
}

fn reference() -> ReferenceData {
    ReferenceData {
        airlines: vec![Airline { name: "SAS".into(), iata: "SK".into(), icao: "SAS".into() }],
        airports: vec![
            airport(1, "Stockholm Arlanda Airport", ("ARN", "ESSA"), "Stockholm", "Europe/Stockholm", (59.6519, 17.9186)),
            airport(2, "Copenhagen Kastrup Airport", ("CPH", "EKCH"), "Copenhagen", "Europe/Copenhagen", (55.6179, 12.656)),
        ],
        ..Default::default()
    }
}

/// The feed's row, last reported at `seen_ms`.
fn row(seen_ms: f64) -> LiveFlight {
    LiveFlight {
        id: ID,
        lat: 57.62,
        lon: 15.41,
        track: 214,
        alt: 36_000,
        speed: 448,
        timestamp_ms: seen_ms as u64,
        callsign: "SAS1417".into(),
        flight: "SK1417".into(),
        reg: "SE-ROR".into(),
        typecode: "A20N".into(),
        origin: "ARN".into(),
        destination: "CPH".into(),
        icon: "A320".into(),
        ..Default::default()
    }
}

/// The stream's details (no pictures, no logo), its report at `seen_ms`.
fn details(seen_ms: f64, plan: bool) -> FlightDetails {
    let trail = (0..60)
        .map(|i| TrailPoint { timestamp: 1_782_034_980 + i * 52, latitude: 59.45 - i as f32 * 0.03, longitude: 17.70 - i as f32 * 0.037, altitude: (i as i32 * 1400).min(36_000), ground_speed: 448, track: 214, ..Default::default() })
        .collect();
    FlightDetails {
        aircraft: Some(AircraftInfo {
            icao_address: HEX,
            reg: "SE-ROR".into(),
            typecode: "A20N".into(),
            full_description: "Airbus A320-251N".into(),
            registered_owners: "SAS Scandinavian Airlines".into(),
            msn: Some("7894".into()),
            birth_date: Some("2017-11-21".into()),
            age: Some(8),
            ..Default::default()
        }),
        schedule: Some(ScheduleInfo {
            flight_number: "SK1417".into(),
            origin_id: 1,
            destination_id: 2,
            scheduled_departure: 1_782_033_900,
            scheduled_arrival: 1_782_038_100,
            actual_departure: 1_782_034_980,
            ..Default::default()
        }),
        progress: Some(FlightProgress {
            traversed_distance: 312_400,
            remaining_distance: 210_600,
            elapsed_time: 1_020,
            remaining_time: 3_120,
            eta: 1_782_039_120,
            great_circle_distance: 523_000,
            mean_flight_time: 4_200,
            flight_stage: "AIRBORNE".into(),
            progress_pct: 60,
            ..Default::default()
        }),
        flight: Some(FlightInfo {
            timestamp_ms: seen_ms as u64,
            flightid: ID,
            latitude: 57.62,
            longitude: 15.41,
            track: 214,
            altitude: 36_000,
            ground_speed: 448,
            callsign: "SAS1417".into(),
            source: "ADSB".into(),
            ..Default::default()
        }),
        // waypoints in 1e5 degrees from Arlanda towards Kastrup
        flight_plan: plan.then(|| FlightPlan {
            departure: "ESSA".into(),
            destination: "EKCH".into(),
            route: "N0450F360 HAPZI2C HAPZI DCT ARS DCT NEGIL M852 KOGAX DCT NOSLI NOSLI1E".into(),
            length: 520.0,
            waypoints: vec![[5_965_190, 1_791_860], [5_890_000, 1_700_000], [5_760_000, 1_530_000], [5_650_000, 1_330_000], [5_561_790, 1_265_600]],
            alternates: vec!["ESMS".into(), "EKBI".into()],
        }),
        trail,
        ..Default::default()
    }
}

/// The aircraft feed: the one flight, the reference data, signed in or not.
struct Canned {
    flight: LiveFlight,
    authenticated: bool,
}

impl Feeds for Canned {
    fn live_flights(&mut self, _area: Option<BoundingBox>, _categories: Option<Vec<u8>>) -> Answer<Result<LiveSnapshot, String>> {
        Answer::ready(Ok(LiveSnapshot { flights: vec![self.flight.clone()], server_time_ms: NOW_MS as u64, requests: 1, ..Default::default() }))
    }

    fn top_flights(&mut self) -> Answer<Result<Vec<TopFlight>, String>> {
        Answer::ready(Ok(Vec::new()))
    }

    fn reference_data(&mut self) -> Answer<Result<LoadedReference, String>> {
        Answer::ready(Ok(LoadedReference::new(reference())))
    }

    fn session_info(&mut self) -> Option<SessionInfo> {
        self.authenticated.then_some(SessionInfo { authenticated: true, expires: None })
    }
}

fn runway(a: (&str, f64, f64), b: (&str, f64, f64)) -> Runway {
    let end = |(ident, lat, lon): (&str, f64, f64)| RunwayEnd { ident: ident.into(), lat, lon, elevation_ft: Some(20.0), displaced_ft: 0.0 };
    Runway { length_ft: Some(11_000.0), width_ft: Some(150.0), surface: "ASP".into(), ends: [end(a), end(b)] }
}

/// The follow stream (its sender kept); runways and OpenSky's track when asked for.
struct Stream {
    sender: Arc<Mutex<Option<Sender<FollowEvent>>>>,
    runways: bool,
}

impl FlightFeeds for Stream {
    fn follow_flight(&mut self, _id: u32) -> Result<FollowStream, String> {
        let (tx, rx) = mpsc::channel();
        *self.sender.lock().unwrap() = Some(tx);
        Ok(FollowStream::Channel(rx))
    }

    fn unfollow_flight(&mut self) {}

    fn opensky_track(&mut self, _icao24: &str) -> Answer<Result<Vec<OpenSkyPoint>, String>> {
        if !self.runways {
            return Answer::ready(Err("no OpenSky track here".into()));
        }
        // the climb-out, before Flightradar24's first point (still 18 km out of Arlanda)
        let points = (0..5).map(|i| OpenSkyPoint { timestamp: 1_782_034_700 + i * 50, latitude: 59.50 - i as f32 * 0.01, longitude: 17.78 - i as f32 * 0.016, altitude: 5_000 + i as i32 * 900, track: 214, on_ground: false }).collect();
        Answer::ready(Ok(points))
    }

    fn adsb(&mut self, _query: &AdsbQuery) -> Answer<Result<AdsbSnapshot, String>> {
        Answer::ready(Err("no adsb.lol here".into()))
    }

    fn airport_runways(&mut self, codes: Vec<String>) -> Answer<Result<HashMap<String, Vec<Runway>>, String>> {
        let mut all = HashMap::new();
        if self.runways {
            all.insert("ESSA".to_owned(), vec![runway(("01L", 59.6366, 17.9285), ("19R", 59.6703, 17.9453)), runway(("08", 59.6496, 17.9029), ("26", 59.6556, 17.9600))]);
            all.insert("EKCH".to_owned(), vec![runway(("04L", 55.6046, 12.6261), ("22R", 55.6327, 12.6672)), runway(("12", 55.6264, 12.6201), ("30", 55.6085, 12.6699))]);
        }
        Answer::ready(Ok(codes.into_iter().filter_map(|c| all.remove(&c).map(|r| (c, r))).collect()))
    }
}

/// What a fixture is made of.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Setup {
    authenticated: bool,
    plan: bool,
    runways: bool,
}

/// The state at [`NOW_MS`]: the flight selected 10 s before, its details streamed, polled and
/// synced as the window does.
fn fixture(setup: Setup) -> AppState {
    let seen = NOW_MS - 2_000.0;
    let mut s = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, ViewState::INITIAL, SIZE.map(f64::from));
    s.aircraft = Aircraft::with_feeds(Some(Box::new(Canned { flight: row(seen), authenticated: setup.authenticated })));
    let sender = Arc::new(Mutex::new(None));
    s.flight = Flight::with_feeds(Some(Box::new(Stream { sender: sender.clone(), runways: setup.runways })));
    let t0 = NOW_MS - 10_000.0;
    for t in [t0 - 2_000.0, t0 - 1_000.0] {
        s.poll(t);
    }
    s.select_flight(Some(ID), SelectOptions::default(), t0);
    sender.lock().unwrap().as_ref().expect("the stream started").send(FollowEvent::Details(Box::new(details(seen, setup.plan)))).unwrap();
    let mut t = t0;
    while t <= NOW_MS {
        s.poll(t);
        s.sync_flight(t);
        t += 1_000.0;
    }
    assert_eq!(s.flight.follow_state(), FollowState::Live);
    s
}

/// The aircraft's other flights: this one (live, on its way), the one before, yesterday's last
/// (late), and a scheduled one to an airport the reference data lacks.
fn flights() -> Vec<HistoryRow> {
    let row = |id: Option<u32>, number: Option<&str>, legs: (Option<&str>, &str), status: &str, color: &str, live: bool, times: [Option<i64>; 6]| HistoryRow {
        flight_id: id,
        number: number.map(Into::into),
        callsign: number.map(|n| n.replace("SK", "SAS")),
        origin_iata: legs.0.map(Into::into),
        destination_iata: Some(legs.1.into()),
        status: Some(status.into()),
        status_color: Some(color.into()),
        live,
        stod: times[0],
        etod: times[1],
        atod: times[2],
        stoa: times[3],
        etoa: times[4],
        atoa: times[5],
        ..Default::default()
    };
    vec![
        row(Some(ID), Some("SK1417"), (Some("ARN"), "CPH"), "Estimated 12:12", "yellow", true, [Some(1_782_033_900), None, Some(1_782_034_980), Some(1_782_038_100), Some(1_782_039_120), None]),
        row(Some(0x3a8e_0001), Some("SK1416"), (Some("CPH"), "ARN"), "Landed 07:51", "green", true, [Some(1_782_022_800), None, Some(1_782_023_400), Some(1_782_027_300), None, Some(1_782_027_060)]),
        row(Some(0x3a8e_0002), Some("SK1411"), (Some("ARN"), "CPH"), "Landed 22:30", "red", false, [Some(1_781_980_000), None, Some(1_781_982_000), Some(1_781_985_000), None, Some(1_781_989_800)]),
        row(None, None, (None, "OSL"), "Scheduled", "gray", false, [Some(1_782_050_000), None, None, Some(1_782_054_000), None, None]),
    ]
}

/// What the History tab is answered.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Answered {
    Rows,
    Never,
    Failed,
}

/// The tab's feed: the canned answer, the registrations asked for.
struct HistoryFeed {
    answered: Answered,
    asked: Arc<Mutex<Vec<String>>>,
    /// Kept so a `Never` answer stays pending.
    pending: Vec<Sender<Result<Vec<HistoryRow>, String>>>,
}

impl history::HistoryFeeds for HistoryFeed {
    fn aircraft_history(&mut self, registration: &str) -> Answer<Result<Vec<HistoryRow>, String>> {
        self.asked.lock().unwrap().push(registration.to_owned());
        match self.answered {
            Answered::Rows => Answer::ready(Ok(flights())),
            Answered::Failed => Answer::ready(Err("Flightradar24 answered 503".into())),
            Answered::Never => {
                let (tx, rx) = mpsc::channel();
                self.pending.push(tx);
                rx.into()
            }
        }
    }
}

/// A scenario: its name, the state, the tab, the history's answer, how far it is scrolled, the
/// window's height.
struct Scenario {
    name: &'static str,
    setup: Setup,
    tab: Tab,
    answered: Answered,
    scroll: f32,
    height: u32,
}

const fn scenario(name: &'static str, setup: Setup, tab: Tab) -> Scenario {
    Scenario { name, setup, tab, answered: Answered::Rows, scroll: 0.0, height: SIZE[1] }
}

fn scenarios() -> Vec<Scenario> {
    let runways = Setup { authenticated: true, runways: true, plan: false };
    vec![
        scenario("route-estimated", Setup::default(), Tab::Route),
        scenario("route-runways", runways, Tab::Route),
        // a window 700 px high: the panel scrolls, the credit rises above the footer
        Scenario { scroll: 120.0, height: 700, ..scenario("route-runways-scrolled", runways, Tab::Route) },
        scenario("route-filed", Setup { authenticated: true, plan: true, runways: false }, Tab::Route),
        scenario("aircraft", Setup::default(), Tab::Aircraft),
        scenario("history", Setup::default(), Tab::History),
        Scenario { answered: Answered::Never, ..scenario("history-loading", Setup::default(), Tab::History) },
        Scenario { answered: Answered::Failed, ..scenario("history-failed", Setup::default(), Tab::History) },
    ]
}

/// The tab's name as the page's `tab` state writes it.
fn tab_name(tab: Tab) -> &'static str {
    match tab {
        Tab::Overview => "overview",
        Tab::Route => "route",
        Tab::Aircraft => "aircraft",
        Tab::History => "history",
    }
}

/// The props Chromium's page is rendered with, from the panel's own view of each scenario.
fn props() -> serde_json::Value {
    let scenarios: Vec<serde_json::Value> = scenarios()
        .iter()
        .map(|sc| {
            let state = fixture(sc.setup);
            let view = View::of(&state, NOW_MS).expect("a flight is selected");
            let history = match sc.answered {
                Answered::Rows => serde_json::json!({ "reg": "SE-ROR", "rows": flights(), "failed": false }),
                Answered::Never => serde_json::json!({ "reg": "SE-ROR", "rows": null, "failed": false }),
                Answered::Failed => serde_json::json!({ "reg": "SE-ROR", "rows": null, "failed": true }),
            };
            serde_json::json!({
                "name": sc.name,
                "tab": tab_name(sc.tab),
                "flight": view.flight,
                "details": view.details,
                "route": view.route,
                "status": "live",
                "authenticated": view.authenticated,
                "openskyAdded": view.opensky_added,
                "history": history,
                "scroll": sc.scroll,
                "height": sc.height,
            })
        })
        .collect();
    serde_json::json!({ "now": NOW_MS, "reference": reference(), "scenarios": scenarios })
}

// ------------------------------------------------------------------------------------------------
// the shell with the panel

fn slots() -> Slots {
    Slots { flight_panel: Some(flight_panel::show), ..Slots::default() }
}

fn context(answered: Answered, asked: Arc<Mutex<Vec<String>>>) -> egui::Context {
    let ctx = egui::Context::default();
    theme::apply(&ctx);
    // the skeleton stands at its start, as in Chromium's capture
    anim::set_reduced_motion(&ctx, true);
    history::install_feeds(&ctx, Box::new(HistoryFeed { answered, asked, pending: Vec::new() }));
    ctx
}

/// One pass of the shell over `state` with `events`: where it put things, the actions, the output.
fn run(ctx: &egui::Context, size: [u32; 2], time: f64, state: &AppState, events: Vec<Event>) -> (Placed, Vec<Action>, egui::FullOutput) {
    let raw = RawInput {
        screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(size[0] as f32, size[1] as f32))),
        time: Some(time),
        events,
        ..Default::default()
    };
    let mut placed = Placed::default();
    let mut actions = Vec::new();
    let output = ctx.run_ui(raw, |ui| placed = shell::show(ui.ctx(), state, NOW_MS, &mut actions, &slots()));
    (placed, actions, output)
}

/// Where text shapes reading `text` were painted, in paint order.
fn text_rects(shapes: &[ClippedShape], text: &str) -> Vec<egui::Rect> {
    fn walk(shape: &Shape, text: &str, out: &mut Vec<egui::Rect>) {
        match shape {
            Shape::Text(t) if t.galley.text() == text => out.push(t.galley.rect.translate(t.pos.to_vec2())),
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, text, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    shapes.iter().for_each(|c| walk(&c.shape, text, &mut out));
    out
}

/// Where the last text shape reading `text` was painted (the tab panel's, after the strip's).
fn text_rect(shapes: &[ClippedShape], text: &str) -> Option<egui::Rect> {
    text_rects(shapes, text).pop()
}

/// The shapes of a pass nobody draws (its textures dropped).
fn undrawn(mut output: egui::FullOutput) -> Vec<ClippedShape> {
    output.textures_delta.clear();
    output.shapes
}

fn click(at: Pos2) -> Vec<Event> {
    vec![
        Event::PointerMoved(at),
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() },
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() },
    ]
}

/// The pointer far from the panel.
fn away() -> Vec<Event> {
    vec![Event::PointerMoved(Pos2::new(20.0, 880.0))]
}

/// Drives the shell to `scenario`'s tab (a click on its label) and scroll: the passes' outputs, in
/// order, each with what it placed and asked.
struct Driver {
    ctx: egui::Context,
    time: f64,
    size: [u32; 2],
}

impl Driver {
    fn new(answered: Answered, asked: Arc<Mutex<Vec<String>>>) -> Self {
        Driver { ctx: context(answered, asked), time: 0.0, size: SIZE }
    }

    fn pass(&mut self, state: &AppState, events: Vec<Event>) -> (Placed, Vec<Action>, egui::FullOutput) {
        let out = run(&self.ctx, self.size, self.time, state, events);
        self.time += 1.0;
        out
    }

    /// A pass nobody draws: what it placed and asked, its shapes.
    fn step(&mut self, state: &AppState, events: Vec<Event>) -> (Placed, Vec<Action>, Vec<ClippedShape>) {
        let (placed, actions, mut output) = self.pass(state, events);
        output.textures_delta.clear();
        (placed, actions, output.shapes)
    }

    /// Passes until the panel stands, then the tab's label clicked, then the scroll.
    fn open(&mut self, state: &AppState, tab: Tab, scroll: f32, mut each: impl FnMut(&egui::Context, egui::FullOutput)) -> Placed {
        let mut placed = Placed::default();
        let mut label = None;
        for _ in 0..2 {
            let (p, _, output) = self.pass(state, away());
            placed = p;
            label = text_rect(&output.shapes, flight_panel::TABS.iter().find(|t| t.value == tab).map_or("", |t| t.label));
            each(&self.ctx, output);
        }
        if tab != Tab::Overview {
            let at = label.expect("the tab's label").center();
            let (_, _, output) = self.pass(state, click(at));
            each(&self.ctx, output);
        }
        if scroll > 0.0 {
            let inside = placed.panel.expect("the panel").1.center();
            let wheel = Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, -scroll), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Default::default() };
            let (_, _, output) = self.pass(state, vec![Event::PointerMoved(inside), wheel]);
            each(&self.ctx, output);
        }
        for _ in 0..3 {
            let (p, _, output) = self.pass(state, away());
            placed = p;
            each(&self.ctx, output);
        }
        assert_eq!(flight_panel::tab(&self.ctx), tab);
        placed
    }
}

/// egui's output drawn onto a target cleared to `--ink`, as the window's UI pass draws it.
struct Canvas {
    egui: egui_wgpu::Renderer,
    target: Offscreen,
}

impl Canvas {
    fn new(gpu: &Gpu, size: [u32; 2]) -> Self {
        Canvas { egui: egui_wgpu::Renderer::new(&gpu.device, FORMAT, egui_wgpu::RendererOptions::default()), target: Offscreen::new(gpu, size) }
    }

    fn draw(&mut self, gpu: &Gpu, ctx: &egui::Context, output: egui::FullOutput) {
        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: self.target.size(), pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui.update_texture(device, queue, id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("flight panel tabs") });
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
    }

    fn read(&self, gpu: &Gpu) -> Image {
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        self.target.read(gpu).expect("read back")
    }
}

/// `scenario` drawn: what the shell placed and the picture.
fn shot(gpu: &Gpu, sc: &Scenario) -> (Placed, Image) {
    let state = fixture(sc.setup);
    let size = [SIZE[0], sc.height];
    let mut driver = Driver { size, ..Driver::new(sc.answered, Arc::default()) };
    let mut canvas = Canvas::new(gpu, size);
    let placed = driver.open(&state, sc.tab, sc.scroll, |ctx, output| canvas.draw(gpu, ctx, output));
    let image = canvas.read(gpu);
    save(&image, &format!("u11-{}.png", sc.name));
    (placed, image)
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
    bounds(image, area(1000, 0, 1440, image.height), |p| close(p, theme::LINE, 2)).expect("the panel's border")
}

/// Pixels of `c` (within 12 levels) in `area`.
fn count(image: &Image, area: Rect, c: Color32) -> usize {
    common::shots::count(image, area, |p| close(p, c, 12))
}

#[test]
fn the_props_for_chromium() {
    let props = props();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("u11-props.json"), serde_json::to_vec(&props).unwrap()).unwrap();
    // the routes the scenarios are about
    let kinds: HashMap<&str, (RouteKind, bool, bool, usize)> = scenarios()
        .iter()
        .map(|sc| {
            let s = fixture(sc.setup);
            let r = s.flight.route().expect("a route").clone();
            (sc.name, (r.kind, r.arrival.is_some(), r.departure.is_some(), s.flight.opensky_added()))
        })
        .collect();
    assert_eq!(kinds["route-estimated"], (RouteKind::Estimated, false, false, 0));
    assert_eq!(kinds["route-filed"].0, RouteKind::Filed);
    let (kind, arrival, departure, added) = kinds["route-runways"];
    assert_eq!(kind, RouteKind::Estimated);
    assert!(arrival && departure && added > 0, "runways both ways and OpenSky's start: {:?}", kinds["route-runways"]);
}

#[test]
fn the_route_tab() {
    let Some(gpu) = gpu() else { return };
    for sc in scenarios().iter().filter(|s| s.tab == Tab::Route) {
        let (placed, image) = shot(&gpu, sc);
        let b = panel_box(&image);
        assert_eq!((b.x0, b.x1, b.y1), (1062, 1424, sc.height - 28), "{}: {b:?}, {:?}", sc.name, placed.panel);
        // the gradient swatch's pixels: CSS's stops at each column's centre, within the
        // antialiased ends (a 3 px bar, 28 wide, 16 px into the panel)
        let swatch = (b.y0..b.y1).find(|&y| (1080..1106).all(|x| {
            let p = pixel(&image, x, y);
            let want = route::gradient_at((f64::from(x - 1079) + 0.5) / 28.0);
            (0..3).all(|k| (f64::from(p[k]) - want[k]).abs() <= 8.0)
        }));
        if sc.name != "route-runways-scrolled" {
            let y = swatch.unwrap_or_else(|| panic!("{}: the altitude gradient", sc.name));
            assert!(close(pixel(&image, 1079 + 30, y), theme::SURFACE, 2), "{}: the swatch ends at 28 px", sc.name);
        } else {
            // scrolled, the runways' credit (11 px --text-3, right 8) rises above the footer
            let footer = b.y1 - 1 - 57;
            assert!(count(&image, area(1330, footer - 34, 1416, footer), theme::TEXT_3) > 20, "the runways' credit above the footer");
        }
    }
}

#[test]
fn the_aircraft_and_history_tabs() {
    let Some(gpu) = gpu() else { return };
    for sc in scenarios().iter().filter(|s| s.tab != Tab::Route) {
        let (_, image) = shot(&gpu, sc);
        let b = panel_box(&image);
        assert_eq!((b.x0, b.x1, b.y1), (1062, 1424, 872), "{}: {b:?}", sc.name);
        let inside = area(1063, b.y0 + 1, 1423, 871 - 57);
        match sc.name {
            // the flight shown on --amber-soft, its status and the yesterday's late one in their
            // tones, the landed one in --live
            "history" => {
                let soft = Color32::from_rgb(51, 67, 84);
                assert!(count(&image, inside, soft) > 328 * 40, "the row of the flight shown");
                assert!(count(&image, inside, theme::LIVE) > 20, "Landed in --live");
                assert!(count(&image, inside, theme::DANGER) > 20, "late in --danger");
                assert!(count(&image, inside, theme::WARN) > 20, "This flight in its status's --warn");
            }
            "history-loading" => {
                // five skeleton lines 30 px apart over the footer (the tab panel: 5 × 30 and its padding)
                let lines: Vec<u32> = (871 - 57 - 170..871 - 57).filter(|&y| !close(pixel(&image, 1100, y), theme::SURFACE, 3) && close(pixel(&image, 1100, y - 1), theme::SURFACE, 3)).collect();
                assert_eq!(lines.len(), 5, "{lines:?}");
                assert!(lines.windows(2).all(|w| w[1] - w[0] == 30), "{lines:?}");
            }
            "history-failed" => assert!(count(&image, inside, theme::LINE_CONTROL) > 100, "Try again's border"),
            _ => {}
        }
    }
}

// ------------------------------------------------------------------------------------------------
// interactions (egui only)

#[test]
fn a_live_flight_in_the_history_opens_and_the_tab_asks_once() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let state = fixture(Setup::default());
    let mut driver = Driver::new(Answered::Rows, asked.clone());
    let mut shapes = Vec::new();
    driver.open(&state, Tab::History, 0.0, |_, output| shapes = undrawn(output));
    // the row before this flight (SK1416, live) asks to select it; this flight's and the ones not
    // live ask nothing
    let at = text_rect(&shapes, "SK1416").expect("the row").center();
    let (_, actions, _) = driver.step(&state, click(at));
    assert_eq!(actions, [Action::PickFlight(0x3a8e_0001)]);
    for number in ["SK1417", "SK1411"] {
        let at = text_rect(&shapes, number).expect("the row").center();
        let (_, actions, _) = driver.step(&state, click(at));
        assert!(actions.is_empty(), "{number}: {actions:?}");
    }
    assert!(text_rect(&shapes, history::LIVE_NOTE).is_some(), "the note under the list");
    assert_eq!(*asked.lock().unwrap(), ["SE-ROR"], "asked once while open");
    // to the Route tab and back: nothing asked again
    driver.open(&state, Tab::Route, 0.0, |_, output| drop(undrawn(output)));
    driver.open(&state, Tab::History, 0.0, |_, output| drop(undrawn(output)));
    assert_eq!(asked.lock().unwrap().len(), 1);
    // closed for a pass and opened again (on Overview, then History): asked afresh
    let none = AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, SIZE.map(f64::from));
    driver.step(&none, away());
    driver.open(&state, Tab::History, 0.0, |_, output| drop(undrawn(output)));
    assert_eq!(asked.lock().unwrap().len(), 2);
}

#[test]
fn try_again_and_sign_in() {
    // failed: Try again asks again
    let asked = Arc::new(Mutex::new(Vec::new()));
    let state = fixture(Setup::default());
    let mut driver = Driver::new(Answered::Failed, asked.clone());
    let mut shapes = Vec::new();
    driver.open(&state, Tab::History, 0.0, |_, output| shapes = undrawn(output));
    assert!(text_rect(&shapes, "Couldn't load the flights of SE-ROR.").is_some());
    let at = text_rect(&shapes, history::TRY_AGAIN).expect("Try again").center();
    driver.step(&state, click(at));
    assert_eq!(asked.lock().unwrap().len(), 2, "asked again");

    // not signed in: the Route tab's button opens the keys dialog with the login
    let mut driver = Driver::new(Answered::Rows, Arc::default());
    driver.open(&state, Tab::Route, 0.0, |_, output| shapes = undrawn(output));
    let at = text_rect(&shapes, route::SIGN_IN).expect("the button").center();
    let (_, actions, _) = driver.step(&state, click(at));
    assert_eq!(actions, [Action::Open(Open::Keys, true)]);
    // signed in: no button
    let signed_in = fixture(Setup { authenticated: true, ..Setup::default() });
    let mut driver = Driver::new(Answered::Rows, Arc::default());
    driver.open(&signed_in, Tab::Route, 0.0, |_, output| shapes = undrawn(output));
    assert!(text_rect(&shapes, route::SIGN_IN).is_none());
}
