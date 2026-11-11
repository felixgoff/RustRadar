//! Item U12 in the flight panel (doc 06 §7.1, §9, §10): the app's [`Parts::APP`] draw the altitude
//! chart and its export menu in the Overview, between the facts and the footer, from the flight's
//! streamed trail and its departure airport's zone; the menu opens over the pinned footer, cut by
//! the panel's edge; Escape closes the menu before the page's Escape would close the panel.
//!
//! Shots (`target/shots/u12-panel-live.png`, `u12-panel-menu.png`): the flight panel at 1440 × 900
//! with the live flight of item U10's fixture (SAS1417 Arlanda to Copenhagen, 60 positions
//! climbing to 36,000 ft).
//!
//! The GPU test needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without
//! one it prints `skipped: …` and passes.

mod common;

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use common::NOW_MS;
use common::shots::{Rect as Px, bounds, gpu, pixel, save};
use egui::{Color32, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use rustradar_app::state::aircraft::{Aircraft, Answer, Feeds, LoadedReference};
use rustradar_app::state::flight::{AdsbQuery, Flight, FlightFeeds, FollowStream, SelectOptions};
use rustradar_app::state::{Action, AppState, Startup};
use rustradar_app::ui::shell::{self, Placed, Slots};
use rustradar_app::ui::{anim, export_menu, flight_panel, theme};
use rustradar_core::geo::Basemap;
use rustradar_core::model::{
    AdsbSnapshot, Airline, Airport, BoundingBox, FlightDetails, FlightInfo, FollowEvent, LiveFlight, LiveSnapshot, OpenSkyPoint, ReferenceData, Runway,
    ScheduleInfo, SessionInfo, TopFlight, TrailPoint,
};
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_scene::camera::ViewState;

const SIZE: [u32; 2] = [1440, 900];
const ID: u32 = 0x3a8f_21c4;

fn airport(id: u32, name: &str, iata: &str, icao: &str, city: &str, timezone: &str) -> Airport {
    Airport { id, name: name.into(), iata: iata.into(), icao: icao.into(), city: city.into(), timezone: timezone.into(), size: 100, ..Default::default() }
}

fn reference() -> ReferenceData {
    ReferenceData {
        airlines: vec![Airline { name: "SAS".into(), iata: "SK".into(), icao: "SAS".into() }],
        airports: vec![
            Airport { lat: 59.6519, lon: 17.9186, ..airport(1, "Stockholm Arlanda Airport", "ARN", "ESSA", "Stockholm", "Europe/Stockholm") },
            Airport { lat: 55.6179, lon: 12.656, ..airport(2, "Copenhagen Kastrup Airport", "CPH", "EKCH", "Copenhagen", "Europe/Copenhagen") },
        ],
        ..Default::default()
    }
}

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

/// The stream's details: no pictures, the schedule, and the trail of `tests/altitude_chart.rs`'s
/// `climb` (U10's live trail).
fn details(seen_ms: f64) -> FlightDetails {
    let trail = (0..60)
        .map(|i| TrailPoint {
            timestamp: 1_782_034_980 + i * 52,
            latitude: 59.65 - i as f32 * 0.034,
            longitude: 17.92 - i as f32 * 0.042,
            altitude: (i as i32 * 1400).min(36_000),
            ground_speed: (180 + 9 * i as u32).min(448),
            track: 214,
            ..Default::default()
        })
        .collect();
    FlightDetails {
        schedule: Some(ScheduleInfo { flight_number: "SK1417".into(), origin_id: 1, destination_id: 2, scheduled_departure: 1_782_033_900, scheduled_arrival: 1_782_038_100, actual_departure: 1_782_034_980, ..Default::default() }),
        flight: Some(FlightInfo { timestamp_ms: seen_ms as u64, flightid: ID, latitude: 57.62, longitude: 15.41, track: 214, altitude: 36_000, ground_speed: 448, callsign: "SAS1417".into(), source: "ADSB".into(), ..Default::default() }),
        trail,
        ..Default::default()
    }
}

struct Canned {
    flight: LiveFlight,
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
        None
    }
}

struct Stream {
    sender: Arc<Mutex<Option<Sender<FollowEvent>>>>,
}

impl FlightFeeds for Stream {
    fn follow_flight(&mut self, _id: u32) -> Result<FollowStream, String> {
        let (tx, rx) = mpsc::channel();
        *self.sender.lock().unwrap() = Some(tx);
        Ok(FollowStream::Channel(rx))
    }

    fn unfollow_flight(&mut self) {}

    fn opensky_track(&mut self, _icao24: &str) -> Answer<Result<Vec<OpenSkyPoint>, String>> {
        Answer::ready(Err("no OpenSky track here".into()))
    }

    fn adsb(&mut self, _query: &AdsbQuery) -> Answer<Result<AdsbSnapshot, String>> {
        Answer::ready(Err("no adsb.lol here".into()))
    }

    fn airport_runways(&mut self, _codes: Vec<String>) -> Answer<Result<HashMap<String, Vec<Runway>>, String>> {
        Answer::ready(Ok(HashMap::new()))
    }
}

/// The live flight selected 10 s before [`NOW_MS`], its details streamed.
fn live() -> AppState {
    let seen = NOW_MS - 2_000.0;
    let mut s = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, ViewState::INITIAL, SIZE.map(f64::from));
    s.aircraft = Aircraft::with_feeds(Some(Box::new(Canned { flight: row(seen) })));
    let sender = Arc::new(Mutex::new(None));
    s.flight = Flight::with_feeds(Some(Box::new(Stream { sender: sender.clone() })));
    let t0 = NOW_MS - 10_000.0;
    for t in [t0 - 2_000.0, t0 - 1_000.0] {
        s.poll(t);
    }
    s.select_flight(Some(ID), SelectOptions::default(), t0);
    sender.lock().unwrap().as_ref().expect("the stream started").send(FollowEvent::Details(Box::new(details(seen)))).unwrap();
    let mut t = t0;
    while t <= NOW_MS {
        s.poll(t);
        s.sync_flight(t);
        t += 1_000.0;
    }
    s
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    theme::apply(&ctx);
    anim::set_reduced_motion(&ctx, true);
    ctx
}

fn slots() -> Slots {
    Slots { flight_panel: Some(flight_panel::show), ..Slots::default() }
}

/// One pass as the window's UI pass runs it (the export menu's Escape, then the shell): where the
/// shell put things and the actions.
fn run(ctx: &egui::Context, time: f64, state: &AppState, events: Vec<Event>) -> (Placed, Vec<Action>, egui::FullOutput) {
    let raw = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(SIZE[0] as f32, SIZE[1] as f32))),
        time: Some(time),
        events,
        ..Default::default()
    };
    let mut placed = Placed::default();
    let mut actions = Vec::new();
    let output = ctx.run_ui(raw, |ui| {
        export_menu::escape(ui.ctx());
        placed = shell::show(ui.ctx(), state, NOW_MS, &mut actions, &slots());
    });
    (placed, actions, output)
}

fn step(ctx: &egui::Context, time: f64, state: &AppState, events: Vec<Event>) -> (Placed, Vec<Action>) {
    let (placed, actions, mut output) = run(ctx, time, state, events);
    output.textures_delta.clear();
    (placed, actions)
}

fn click(at: Pos2) -> Vec<Event> {
    vec![
        Event::PointerMoved(at),
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() },
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() },
    ]
}

fn escape() -> Vec<Event> {
    vec![Event::Key { key: Key::Escape, physical_key: Some(Key::Escape), pressed: true, repeat: false, modifiers: Modifiers::NONE }]
}

/// Where the export trigger was at the last pass.
fn trigger(ctx: &egui::Context) -> Rect {
    ctx.read_response(export_menu::trigger_id()).expect("the export trigger").rect
}

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

    fn frame(&mut self, gpu: &Gpu, time: f64, state: &AppState, events: Vec<Event>) -> Placed {
        let (placed, _, output) = run(&self.ctx, time, state, events);
        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: self.target.size(), pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui.update_texture(device, queue, id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("flight panel with chart") });
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
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        placed
    }

    fn read(&self, gpu: &Gpu) -> Image {
        self.target.read(gpu).expect("read back")
    }
}

fn near(p: [u8; 4], c: Color32, tolerance: u8) -> bool {
    p[0].abs_diff(c.r()) <= tolerance && p[1].abs_diff(c.g()) <= tolerance && p[2].abs_diff(c.b()) <= tolerance
}

#[test]
fn the_chart_and_its_menu_in_the_flight_panel() {
    let Some(gpu) = gpu() else { return };
    let s = live();
    let mut canvas = Canvas::new(&gpu);
    let mut placed = Placed::default();
    for t in [0.0, 1.0, 2.0, 3.0] {
        placed = canvas.frame(&gpu, t, &s, Vec::new());
    }
    let image = canvas.read(&gpu);
    save(&image, "u12-panel-live.png");
    let panel = placed.panel.expect("the flight panel").1;
    // the trigger in the panel's header row of the chart, 16 px in from the panel's right
    let t = trigger(&canvas.ctx);
    assert!(panel.contains_rect(t), "{t:?} in {panel:?}");
    // the plot's grid, the line and the end dot inside the panel
    let px = |r: Rect| Px { x0: r.left() as u32, y0: r.top() as u32, x1: r.right() as u32, y1: r.bottom() as u32 };
    let series = bounds(&image, px(panel), |p| near(p, theme::SERIES_1, 6)).expect("the line");
    assert!(series.x1 as f32 <= panel.right() - 1.0 - 16.0 + 6.0 + 1.0, "the end dot inside the padding: {series:?}");
    // the menu, open over the panel's footer and cut by its edge
    canvas.frame(&gpu, 4.0, &s, click(t.center()));
    canvas.frame(&gpu, 4.1, &s, Vec::new());
    canvas.frame(&gpu, 4.2, &s, vec![Event::PointerMoved(Pos2::new(700.0, 400.0))]);
    let image = canvas.read(&gpu);
    save(&image, "u12-panel-menu.png");
    // (the panel grew upwards to hold it: it was shorter than the window allows)
    let panel = canvas.frame(&gpu, 4.3, &s, Vec::new()).panel.expect("the flight panel").1;
    let t = trigger(&canvas.ctx);
    // its border: the right edge on the trigger's, 250 wide, the top 8 px under the trigger's
    // 19.6 px block (whose top is 1.9 over the trigger's line box)
    let right = t.right().round() as u32;
    let top = (t.top() - 1.9 + 19.6 + 8.0).round() as u32;
    let mid = top + 100;
    assert!(near(pixel(&image, right - 1, mid), theme::LINE, 1) && near(pixel(&image, right - 2, mid), theme::SURFACE_RAISED, 1), "right edge at {right}");
    assert!(near(pixel(&image, right - 250, mid), theme::LINE, 1) && near(pixel(&image, right - 249, mid), theme::SURFACE_RAISED, 1), "left edge");
    assert!(near(pixel(&image, right - 100, top), theme::LINE, 1) && near(pixel(&image, right - 100, top + 1), theme::SURFACE_RAISED, 1), "top edge at {top}");
    assert!(bounds(&image, Px { x0: right - 249, y0: top + 1, x1: right - 2, y1: top + 200 }, |p| near(p, theme::SERIES_1, 8)).is_none(), "the line hidden under it");
    // all of it in the panel, which stands where it stood (right 16, bottom 28)
    assert!(top + 289 <= panel.bottom() as u32 && (panel.bottom() - (900.0 - 28.0)).abs() <= 0.5, "{panel:?}");
}

#[test]
fn escape_closes_the_menu_before_the_panel() {
    let s = live();
    let ctx = context();
    for t in 0..3 {
        step(&ctx, f64::from(t), &s, Vec::new());
    }
    let t = trigger(&ctx);
    step(&ctx, 3.0, &s, click(t.center()));
    step(&ctx, 3.1, &s, Vec::new());
    assert!(export_menu::menu(&ctx).lock().unwrap().open);
    // the first Escape is the menu's: the panel stays
    let (_, actions) = step(&ctx, 3.2, &s, escape());
    assert!(actions.is_empty(), "{actions:?}");
    assert!(!export_menu::menu(&ctx).lock().unwrap().open);
    // the next is the page's: the top thing closes
    let (_, actions) = step(&ctx, 3.3, &s, escape());
    assert_eq!(actions, [Action::CloseTop]);
}
