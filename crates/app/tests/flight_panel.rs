//! Item U10's acceptance (plan stage 7; doc 06 §4, §7, §7.1, §7.5): the flight panel as the
//! shell draws it, from fixture state, in its three states.
//!
//! - `loading` (`target/shots/u10-flight-loading.png`): selected, the stream connecting, no
//!   details: the strip from the feed's row, the photo's skeleton, the airline from the callsign,
//!   the codes with `Departure —`/`Arrival —`, the `--line` rule and the line's skeleton, the
//!   facts with the sign-in note, the spinner and `Connecting…`, Follow as its icon alone;
//! - `live` (`u10-flight-live.png`): the details streamed (pictures, schedule, progress) and
//!   adsb.lol's measurements, the camera following: the photo with its credit and count, the logo
//!   tile, departed and expected with their delays, the progress track at 60 %, the measured
//!   values tagged `adsb.lol`, the airspace note, "Reported by the aircraft", the pulsing dot and
//!   `Live`, Follow pressed; taller than the window, so it scrolls under its pinned footer;
//! - `ended` (`u10-flight-ended.png`): the stream ended 95 s after its last report: the altitude
//!   and speed dimmed, the estimate's badge neutral, the track in `--text-3`, the three sign-in
//!   values named in one note, the hollow dot and `Last seen 11:58 CEST`.
//!
//! Each was compared by eye, part by part, with Chromium's rendering of today's
//! `FlightPanel.svelte` given the same props (server-rendered with Svelte, laid out with app.css
//! at 1440 × 900); the altitude chart (item U12) is a stand-in of the height Chromium gives it.
//! The tests check the places and colours the module documentation states.
//!
//! The interaction tests drive the panel through egui and the state: the close button, Follow
//! (the camera locks on the aircraft, as with the key C), an airport code (its board opens), the
//! sign-in note, the photo's buttons, the tab kept while the panel stays open.
//!
//! The GPU tests need a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without
//! one they print `skipped: …` and pass.

mod common;

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use common::NOW_MS;
use common::shots::{Rect, bounds, gpu, pixel, save};

/// The pixels `x0..x1` × `y0..y1`.
const fn area(x0: u32, y0: u32, x1: u32, y1: u32) -> Rect {
    Rect { x0, y0, x1, y1 }
}
use egui::load::{ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint};
use egui::{Color32, ColorImage, Event, PointerButton, Pos2, RawInput, Sense, Ui, Vec2};
use rustradar_app::state::aircraft::{Aircraft, Answer, Feeds, LoadedReference};
use rustradar_app::state::flight::{AdsbQuery, Flight, FlightFeeds, FollowState, FollowStream, SelectOptions};
use rustradar_app::state::shortcuts::Open;
use rustradar_app::state::{Action, AppState, Startup};
use rustradar_app::ui::flight_panel::{self, Parts, Tab, View};
use rustradar_app::ui::shell::{self, Pass, Placed, Slots};
use rustradar_app::ui::{anim, theme};
use rustradar_core::geo::Basemap;
use rustradar_core::model::{
    AdsbAircraft, AdsbSnapshot, AircraftImage, AircraftInfo, Airline, Airport, BoundingBox, FlightDetails, FlightInfo, FlightProgress,
    FollowEvent, LiveFlight, LiveSnapshot, OpenSkyPoint, ReferenceData, Runway, ScheduleInfo, SessionInfo, TopFlight, TrailPoint,
    airline_logo_url,
};
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_scene::camera::ViewState;

/// The desktop reference pictures' canvas, CSS px, at scale 1.
const SIZE: [u32; 2] = [1440, 900];
/// The flight (SAS1417, Stockholm to Copenhagen) and its aircraft's address.
const ID: u32 = 0x3a8f_21c4;
const HEX: u32 = 0x4a_ca8f;
/// The pictures' and the logo's URIs.
const PHOTOS: [&str; 3] = ["test://photo1.png", "test://photo2.png", "test://photo3.png"];
const LOGO_ID: u32 = 9;
/// How tall Chromium lays out today's altitude chart for these props (its header, and the empty
/// note while loading; its plot is sized by script, 0 when rendered on the server).
const CHART_LOADING: f32 = 58.39;
const CHART: f32 = 24.8;

// ------------------------------------------------------------------------------------------------
// fixture state

fn airport(id: u32, name: &str, iata: &str, icao: &str, city: &str, country: &str, timezone: &str) -> Airport {
    Airport { id, name: name.into(), iata: iata.into(), icao: icao.into(), city: city.into(), country: country.into(), timezone: timezone.into(), size: 100, ..Default::default() }
}

fn reference() -> ReferenceData {
    ReferenceData {
        airlines: vec![Airline { name: "SAS".into(), iata: "SK".into(), icao: "SAS".into() }],
        airports: vec![
            Airport { lat: 59.6519, lon: 17.9186, ..airport(1, "Stockholm Arlanda Airport", "ARN", "ESSA", "Stockholm", "Sweden", "Europe/Stockholm") },
            Airport { lat: 55.6179, lon: 12.656, ..airport(2, "Copenhagen Kastrup Airport", "CPH", "EKCH", "Copenhagen", "Denmark", "Europe/Copenhagen") },
            Airport { lat: 57.6628, lon: 12.2798, ..airport(3, "Göteborg Landvetter Airport", "GOT", "ESGG", "Gothenburg", "Sweden", "Europe/Stockholm") },
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

/// The stream's details, its report at `seen_ms`.
fn details(seen_ms: f64) -> FlightDetails {
    let image = |n: usize| AircraftImage { url: format!("https://www.jetphotos.com/photo/{n}"), copyright: "Jonas Lindqvist".into(), medium: PHOTOS[n - 1].into(), ..Default::default() };
    let trail = (0..60)
        .map(|i| TrailPoint { timestamp: 1_782_034_980 + i * 52, latitude: 59.65 - i as f32 * 0.034, longitude: 17.92 - i as f32 * 0.042, altitude: (i as i32 * 1400).min(36_000), ground_speed: 448, track: 214, ..Default::default() })
        .collect();
    FlightDetails {
        aircraft: Some(AircraftInfo {
            icao_address: HEX,
            reg: "SE-ROR".into(),
            typecode: "A20N".into(),
            full_description: "Airbus A320-251N".into(),
            registered_owners: "SAS Scandinavian Airlines".into(),
            images: (1..=3).map(image).collect(),
            ..Default::default()
        }),
        schedule: Some(ScheduleInfo {
            flight_number: "SK1417".into(),
            operated_by_id: LOGO_ID,
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
        trail,
        ..Default::default()
    }
}

/// adsb.lol's answer: the aircraft's own values.
fn adsb() -> AdsbSnapshot {
    let seen = (NOW_MS - 1_500.0) as u64;
    AdsbSnapshot {
        now_ms: NOW_MS as u64,
        aircraft: vec![AdsbAircraft {
            hex: format!("{HEX:06x}"),
            callsign: "SAS1417".into(),
            reg: "SE-ROR".into(),
            typecode: "A20N".into(),
            source: "adsb_icao".into(),
            lat: Some(57.62),
            lon: Some(15.41),
            position_ms: Some(seen),
            seen_ms: seen,
            alt_baro: Some(36_000),
            alt_geom: Some(36_575),
            gs: Some(448.2),
            track: Some(214.1),
            true_heading: Some(211.4),
            mag_heading: Some(206.9),
            roll: Some(0.2),
            track_rate: Some(0.0),
            baro_rate: Some(-64),
            geom_rate: Some(-32),
            ias: Some(262),
            tas: Some(452),
            mach: Some(0.784),
            wind_dir: Some(255),
            wind_speed: Some(38),
            oat: Some(-52),
            tat: Some(-24),
            nav_altitude_mcp: Some(36_000),
            nav_heading: Some(212.0),
            nav_qnh: Some(1013.2),
            nav_modes: ["autopilot", "vnav", "lnav", "tcas"].map(String::from).to_vec(),
            squawk: "2312".into(),
            ..Default::default()
        }],
    }
}

/// The aircraft feed: the one flight, the reference data.
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

/// The follow stream (its sender kept), adsb.lol's answer when there is one.
struct Stream {
    sender: Arc<Mutex<Option<Sender<FollowEvent>>>>,
    adsb: Option<AdsbSnapshot>,
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
        Answer::ready(self.adsb.clone().ok_or_else(|| "no adsb.lol here".to_owned()))
    }

    fn airport_runways(&mut self, _codes: Vec<String>) -> Answer<Result<HashMap<String, Vec<Runway>>, String>> {
        Answer::ready(Ok(HashMap::new()))
    }
}

/// The panel's states.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Fixture {
    Loading,
    Live,
    Ended,
    /// Live, diverted to Gothenburg, the camera free, no adsb.lol.
    Diverted,
}

/// The state of `fixture` at [`NOW_MS`]: the flight selected 10 s before, the stream's events
/// sent, polled and synced as the window does.
fn fixture(fixture: Fixture) -> AppState {
    let seen = if fixture == Fixture::Ended { NOW_MS - 95_000.0 } else { NOW_MS - 2_000.0 };
    let mut s = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, ViewState::INITIAL, SIZE.map(f64::from));
    s.aircraft = Aircraft::with_feeds(Some(Box::new(Canned { flight: row(seen) })));
    let sender = Arc::new(Mutex::new(None));
    let adsb = (fixture == Fixture::Live).then(adsb);
    s.flight = Flight::with_feeds(Some(Box::new(Stream { sender: sender.clone(), adsb })));
    let t0 = NOW_MS - 10_000.0;
    for t in [t0 - 2_000.0, t0 - 1_000.0] {
        s.poll(t);
    }
    s.select_flight(Some(ID), SelectOptions::default(), t0);
    let send = |event| sender.lock().unwrap().as_ref().expect("the stream started").send(event).unwrap();
    match fixture {
        Fixture::Loading => {}
        Fixture::Live => {
            send(FollowEvent::Details(Box::new(details(seen))));
            s.toggle_follow(t0);
        }
        Fixture::Ended => {
            send(FollowEvent::Details(Box::new(details(seen))));
            send(FollowEvent::Ended("not live".into()));
        }
        Fixture::Diverted => {
            let mut d = details(seen);
            d.schedule.as_mut().unwrap().diverted_id = 3;
            send(FollowEvent::Details(Box::new(d)));
        }
    }
    let mut t = t0;
    while t <= NOW_MS {
        s.poll(t);
        s.sync_flight(t);
        t += 1_000.0;
    }
    let want = match fixture {
        Fixture::Loading => FollowState::Connecting,
        Fixture::Live | Fixture::Diverted => FollowState::Live,
        Fixture::Ended => FollowState::Ended,
    };
    assert_eq!(s.flight.follow_state(), want);
    s
}

// ------------------------------------------------------------------------------------------------
// pictures

/// A 3:2 photo of an aircraft against the sky: a gradient, the ground, a pale fuselage.
fn photo(n: usize) -> ColorImage {
    let (w, h) = (640usize, 427usize);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let ground = y >= 330;
            let body = (250..270).contains(&y) && (120..520 + n * 10).contains(&x) && y as f32 >= 250.0 - (x as f32 - 120.0) * 0.05;
            let c = if body {
                [235, 235, 240]
            } else if ground {
                [70, 80, 70]
            } else {
                [(90 + y / 6) as u8, (130 + y / 8) as u8, (200 - y / 10) as u8]
            };
            rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    ColorImage::from_rgba_unmultiplied([w, h], &rgba)
}

/// A 200 × 60 airline logo: a dark blue bar on transparency.
fn logo() -> ColorImage {
    let (w, h) = (200usize, 60usize);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 10..=50 {
        for x in 10..=190 {
            rgba[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[0, 48, 135, 255]);
        }
    }
    ColorImage::from_rgba_unmultiplied([w, h], &rgba)
}

/// Serves the fixture's pictures to egui's image loading, as item U18's loader will serve the
/// network's.
struct Pictures(HashMap<String, Arc<ColorImage>>);

impl Pictures {
    fn new() -> Self {
        let mut map: HashMap<String, Arc<ColorImage>> = PHOTOS.iter().enumerate().map(|(n, uri)| ((*uri).to_owned(), Arc::new(photo(n)))).collect();
        map.insert(airline_logo_url(LOGO_ID), Arc::new(logo()));
        Pictures(map)
    }
}

impl ImageLoader for Pictures {
    fn id(&self) -> &str {
        "rustradar::tests::flight_panel::pictures"
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

/// The ColorImage as a shot image, for the Chromium page's `<img>`s.
fn as_image(c: &ColorImage) -> Image {
    Image { width: c.size[0] as u32, height: c.size[1] as u32, rgba: c.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect() }
}

// ------------------------------------------------------------------------------------------------
// the shell with the panel

/// The altitude chart's stand-in: Chromium's height for these props.
fn chart(ui: &mut Ui, view: &View<'_>, _pass: &mut Pass<'_>) {
    let height = if view.loading() { CHART_LOADING } else { CHART };
    ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
}

fn panel(ui: &mut Ui, pass: &mut Pass<'_>) {
    flight_panel::show_with(ui, pass, &Parts { chart: Some(chart), ..Parts::APP });
}

fn slots() -> Slots {
    Slots { flight_panel: Some(panel), ..Slots::default() }
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    theme::apply(&ctx);
    // the skeleton, the spinner and the live dot stand at their start, as in Chromium's capture
    anim::set_reduced_motion(&ctx, true);
    ctx.add_image_loader(Arc::new(Pictures::new()));
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
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("flight panel") });
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

/// Pixels of `c` (within 12 levels) in `area`.
fn count(image: &Image, area: Rect, c: Color32) -> usize {
    common::shots::count(image, area, |p| close(p, c, 12))
}

#[test]
fn loading_live_and_ended() {
    let Some(gpu) = gpu() else { return };
    // the fixture's pictures, for the page Chromium lays out
    save(&as_image(&photo(0)), "u10-photo.png");
    save(&as_image(&logo()), "u10-logo.png");
    let mut boxes = Vec::new();
    for (which, name) in [(Fixture::Loading, "u10-flight-loading.png"), (Fixture::Live, "u10-flight-live.png"), (Fixture::Ended, "u10-flight-ended.png")] {
        let state = fixture(which);
        let mut canvas = Canvas::new(&gpu);
        let (placed, image) = canvas.settle(&gpu, &state);
        save(&image, name);
        let (kind, rect) = placed.panel.expect("the flight panel");
        assert_eq!(kind, shell::Panel::Flight);
        let b = panel_box(&image);
        // 360 wide inside its border at right 16, its bottom 28 above the window's
        assert_eq!((b.x0, b.x1, b.y1), (1062, 1424, 872), "{which:?}: {b:?}, area {rect:?}");
        boxes.push((which, b, image));
    }
    let Ok([(_, loading_box, loading), (_, live_box, live), (_, ended_box, ended)]) = <[_; 3]>::try_from(boxes) else { panic!("three shots") };

    // the panels' heights as Chromium lays them out (681.31, 858 at the window's limit, 686.83)
    assert!((loading_box.y0 as i32 - 191).abs() <= 1, "{loading_box:?}");
    assert_eq!(live_box.y0, 14, "taller than the window: 856 px of content and its border");
    assert!((ended_box.y0 as i32 - 185).abs() <= 1, "{ended_box:?}");

    // the strip: 68 px under the panel's top border, its rules at Chromium's columns (x 1245 and
    // 1314: 1062 + 183.72 and + 252.36)
    for (image, b) in [(&loading, loading_box), (&live, live_box), (&ended, ended_box)] {
        let rule = b.y0 + 1 + 67;
        assert!(close(pixel(image, 1100, rule), theme::LINE, 2), "the strip's rule at {rule}: {:?}", pixel(image, 1100, rule));
        assert!(close(pixel(image, 1245, b.y0 + 30), theme::LINE, 2) || close(pixel(image, 1246, b.y0 + 30), theme::LINE, 2), "the first column rule");
        assert!(close(pixel(image, 1314, b.y0 + 30), theme::LINE, 2) || close(pixel(image, 1315, b.y0 + 30), theme::LINE, 2), "the second column rule");
    }
    // the altitude in the accent while live, in --text-3 once ended
    let level = |b: Rect| area(1327, b.y0 + 14, 1372, b.y0 + 35);
    assert!(count(&live, level(live_box), theme::AMBER) > 20, "FL360 in --amber");
    assert_eq!(count(&ended, level(ended_box), theme::AMBER), 0, "no accent once ended");

    // loading: the photo's skeleton under the strip (--surface-raised to --surface-hover)
    let photo_mid = loading_box.y0 + 69 + 90;
    let p = pixel(&loading, 1100, photo_mid);
    assert!((60..=76).contains(&p[0]) && p[0] == p[1] && p[1] == p[2], "a skeleton grey: {p:?}");
    // live: the photo itself (its sky), 180 px high under the strip
    let p = pixel(&live, 1100, live_box.y0 + 69 + 40);
    assert!(p[2] > p[0] + 60, "the sky of the picture: {p:?}");
    assert!(close(pixel(&live, 1100, live_box.y0 + 69 + 175), Color32::from_rgb(70, 80, 70), 6), "its ground");

    // the footer: pinned at the bottom, its rule 57 px above the panel's bottom border
    for (image, b) in [(&loading, loading_box), (&live, live_box), (&ended, ended_box)] {
        let rule = b.y1 - 1 - 57;
        assert!(close(pixel(image, 1100, rule), theme::LINE, 2), "the footer's rule at {rule}");
    }
    // live: Follow pressed (its --amber border) beside Flightradar24; the Follow button 85.6 wide
    let follow = bounds(&live, area(1150, 872 - 50, 1300, 872 - 10), |p| close(p, theme::AMBER, 30)).expect("Follow's border");
    assert!((follow.width() as i32 - 86).abs() <= 1 && follow.height() == 32, "{follow:?}");
    // the live dot at x 1079–1087
    assert!(count(&live, area(1078, 872 - 33, 1089, 872 - 22), theme::LIVE) >= 30, "the live dot");
    // loading and ended: Follow compact (no --amber anywhere in the footer)
    for (image, what) in [(&loading, "loading"), (&ended, "ended")] {
        assert_eq!(count(image, area(1063, 872 - 57, 1423, 871), theme::AMBER), 0, "{what}: Follow not pressed");
    }
}

// ------------------------------------------------------------------------------------------------
// interactions (egui only)

/// A pass with no renderer: what was placed, the actions, the links opened.
fn step(ctx: &egui::Context, time: f64, state: &AppState, events: Vec<Event>) -> (Placed, Vec<Action>, Vec<String>) {
    let (placed, actions, mut output) = run(ctx, time, state, events);
    output.textures_delta.clear();
    let opened = output.platform_output.commands.iter().filter_map(|c| if let egui::OutputCommand::OpenUrl(u) = c { Some(u.url.clone()) } else { None }).collect();
    (placed, actions, opened)
}

fn click(at: Pos2) -> Vec<Event> {
    vec![
        Event::PointerMoved(at),
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() },
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() },
    ]
}

/// Frames until the panel is placed, then a click at `at` (relative to the panel's top left):
/// the actions it gave.
fn click_panel(ctx: &egui::Context, state: &AppState, at: impl Fn(egui::Rect) -> Pos2) -> Vec<Action> {
    // egui's clock runs on from where the context's last pass left it
    let start = ctx.input(|i| i.time) + 1.0;
    let mut rect = egui::Rect::NOTHING;
    for t in 0..3 {
        let (placed, _, _) = step(ctx, start + f64::from(t), state, Vec::new());
        rect = placed.panel.expect("the panel").1;
    }
    let p = at(rect);
    step(ctx, start + 3.0, state, vec![Event::PointerMoved(p)]);
    let (_, actions, _) = step(ctx, start + 3.1, state, click(p));
    step(ctx, start + 3.2, state, Vec::new());
    actions
}

#[test]
fn close_and_follow() {
    let ctx = context();
    let mut s = fixture(Fixture::Live);
    // the close button: the strip's last column, centred
    let actions = click_panel(&ctx, &s, |r| Pos2::new(r.right() - 1.0 - 20.0, r.top() + 1.0 + 34.0));
    assert_eq!(actions, [Action::CloseFlight]);

    // Follow: the footer's first button; following already (the fixture locked it), it lets go
    let follow = |r: egui::Rect| Pos2::new(r.right() - 1.0 - 16.0 - 129.6 - 8.0 - 40.0, r.bottom() - 1.0 - 28.0);
    let actions = click_panel(&ctx, &s, follow);
    assert_eq!(actions, [Action::ToggleFollow]);
    assert!(s.flight.following());
    s.apply(Action::ToggleFollow, NOW_MS);
    assert!(!s.flight.following(), "Follow lets go");
    // and takes the camera again, as the key C does: the lock reaches the controller
    s.apply(Action::ToggleFollow, NOW_MS);
    assert!(s.flight.following());
    for t in 0..5 {
        s.advance_camera(NOW_MS + 100.0 * f64::from(t));
    }
    assert!(s.controller.locked(), "the camera is locked on the aircraft");

    assert!(s.apply(Action::CloseFlight, NOW_MS + 500.0));
    assert_eq!(s.flight.selected_id(), None, "closed");
    for t in 0..5 {
        s.advance_camera(NOW_MS + 600.0 + 100.0 * f64::from(t));
    }
    assert!(!s.controller.locked(), "and the camera let go");
    assert!(!s.apply(Action::CloseFlight, NOW_MS + 1_200.0), "nothing more to close");
}

#[test]
fn an_airport_code_opens_its_board() {
    let ctx = context();
    let mut s = fixture(Fixture::Live);
    // CPH: the destination's code at the right of the route section (strip 68, photo 180,
    // airline row 42, the section's padding 16)
    let actions = click_panel(&ctx, &s, |r| Pos2::new(r.right() - 1.0 - 16.0 - 15.0, r.top() + 1.0 + 68.0 + 180.0 + 42.0 + 16.0 + 10.0));
    assert_eq!(actions, [Action::OpenAirportOf { destination: true }]);
    assert!(s.apply(Action::OpenAirportOf { destination: true }, NOW_MS));
    assert_eq!(s.airport.code(), Some("CPH"));
    assert_eq!(s.flight.selected_id(), None, "the flight gave way to the board");
    assert_eq!(shell::panels(&s), [shell::Panel::Airport]);
}

#[test]
fn the_sign_in_note_opens_the_keys_dialog() {
    let ctx = context();
    let s = fixture(Fixture::Ended);
    // ended: the facts (one row) and the note under them: `Sign in` at x 240–278 of the panel
    let mut rect = egui::Rect::NOTHING;
    for t in 0..3 {
        rect = step(&ctx, f64::from(t), &s, Vec::new()).0.panel.unwrap().1;
    }
    let tabs_bottom = 68.0 + 180.0 + 42.0 + 160.08 + 39.19;
    let at = Pos2::new(rect.left() + 1.0 + 258.0, rect.top() + 1.0 + tabs_bottom + 16.0 + 36.98 + 8.0 + 10.0);
    step(&ctx, 3.0, &s, vec![Event::PointerMoved(at)]);
    let (_, actions, _) = step(&ctx, 3.1, &s, click(at));
    assert_eq!(actions, [Action::Open(Open::Keys, true)]);
}

#[test]
fn the_photo_steps_and_wraps_and_the_tab_stays_while_open() {
    let ctx = context();
    let s = fixture(Fixture::Live);
    let mut rect = egui::Rect::NOTHING;
    for t in 0..3 {
        rect = step(&ctx, f64::from(t), &s, Vec::new()).0.panel.unwrap().1;
    }
    let previous = Pos2::new(rect.left() + 1.0 + 8.0 + 14.0, rect.top() + 1.0 + 68.0 + 90.0);
    // previous from the first wraps to the third: the count reads 3/3 (checked through the
    // panel's kept index by stepping back round to the start)
    let mut t = 3.0;
    for _ in 0..3 {
        step(&ctx, t, &s, vec![Event::PointerMoved(previous)]);
        let (_, actions, opened) = step(&ctx, t + 0.1, &s, click(previous));
        assert!(actions.is_empty(), "the photo's buttons ask nothing of the state");
        assert!(opened.is_empty(), "nor open a link");
        t += 1.0;
    }
    // the picture itself opens its page
    let picture = Pos2::new(rect.center().x, rect.top() + 1.0 + 68.0 + 40.0);
    step(&ctx, t, &s, vec![Event::PointerMoved(picture)]);
    let (_, _, opened) = step(&ctx, t + 0.1, &s, click(picture));
    assert_eq!(opened, ["https://www.jetphotos.com/photo/1"], "three steps back is the first picture again");

    // the Aircraft tab, kept from frame to frame while the panel is drawn
    let aircraft_tab = Pos2::new(rect.left() + 1.0 + 143.91 + 30.0, rect.top() + 1.0 + 68.0 + 180.0 + 42.0 + 160.08 + 20.0);
    t += 1.0;
    step(&ctx, t, &s, vec![Event::PointerMoved(aircraft_tab)]);
    step(&ctx, t + 0.1, &s, click(aircraft_tab));
    step(&ctx, t + 0.2, &s, Vec::new());
    assert_eq!(flight_panel::tab(&ctx), Tab::Aircraft);
    step(&ctx, t + 0.3, &s, Vec::new());
    assert_eq!(flight_panel::tab(&ctx), Tab::Aircraft, "kept while open");
    // closed for a pass, then opened again: Overview
    let none = AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, SIZE.map(f64::from));
    step(&ctx, t + 0.4, &none, Vec::new());
    step(&ctx, t + 0.5, &s, Vec::new());
    assert_eq!(flight_panel::tab(&ctx), Tab::Overview, "a panel opened again starts on Overview");
}

#[test]
fn more_from_the_aircraft_opens_for_the_session_and_scrolls_clear_of_the_footer() {
    let ctx = context();
    let s = fixture(Fixture::Live);
    let mut rect = egui::Rect::NOTHING;
    for t in 0..3 {
        rect = step(&ctx, f64::from(t), &s, Vec::new()).0.panel.unwrap().1;
    }
    assert!(!flight_panel::overview::more_open(&ctx));
    // the live panel holds 856 px; the summary sits at 821.6 px of content (Chromium), under the
    // pinned footer (its top at 800): scroll it up into view with the wheel first
    let inside = Pos2::new(rect.center().x, rect.top() + 400.0);
    step(&ctx, 3.0, &s, vec![Event::PointerMoved(inside), Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, -60.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Default::default() }]);
    for t in 0..3 {
        step(&ctx, 3.1 + 0.1 * f64::from(t), &s, Vec::new());
    }
    // the summary now 60 px higher: 821.6 − 60 + 12 into its row
    let summary = Pos2::new(rect.left() + 1.0 + 16.0 + 60.0, rect.top() + 1.0 + 821.6 - 60.0 + 12.0);
    step(&ctx, 4.0, &s, vec![Event::PointerMoved(summary)]);
    let (_, actions, _) = step(&ctx, 4.1, &s, click(summary));
    assert!(actions.is_empty());
    assert!(flight_panel::overview::more_open(&ctx), "opened");
    // the next passes scroll the opened block clear of the footer: its seven readings in three
    // rows and the note, 143 px (Chromium), and 12 px more
    let before = flight_panel::scroll_offset(&ctx);
    for t in 0..4 {
        step(&ctx, 4.2 + 0.2 * f64::from(t), &s, Vec::new());
    }
    let after = flight_panel::scroll_offset(&ctx);
    assert!((before - 60.0).abs() < 1.0, "the wheel's 60 px: {before}");
    assert!(after > before + 100.0, "scrolled on to show the opened block: {before} → {after}");
    // closed and opened again, it is still open: the session's choice
    let none = AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, SIZE.map(f64::from));
    step(&ctx, 6.0, &none, Vec::new());
    step(&ctx, 6.1, &s, Vec::new());
    assert!(flight_panel::overview::more_open(&ctx), "kept from flight to flight");
}

#[test]
fn a_diverted_flight() {
    let Some(gpu) = gpu() else { return };
    let state = fixture(Fixture::Diverted);
    let mut canvas = Canvas::new(&gpu);
    let (_, image) = canvas.settle(&gpu, &state);
    save(&image, "u10-flight-diverted.png");
    let b = panel_box(&image);
    assert_eq!((b.x0, b.x1, b.y1), (1062, 1424, 872), "{b:?}");
    // the photo 5:2, 144 px under the strip: the airline row's light logo tile right under it
    let photo_top = b.y0 + 1 + 68;
    assert!(close(pixel(&image, 1100, photo_top + 140), Color32::from_rgb(70, 80, 70), 6), "the picture's ground at its foot");
    let tile = bounds(&image, area(1063, photo_top + 144, 1200, photo_top + 144 + 50), |p| close(p, theme::TEXT_1, 3)).expect("the logo tile");
    assert_eq!(tile.y0, photo_top + 144 + 12, "12 px under the short photo: {tile:?}");
    // GOT on the right, and the bad badge `Diverted to …` at the route section's foot
    let route_top = photo_top + 144 + 42;
    let badge = bounds(&image, area(1063, route_top, 1423, route_top + 200), |p| close(p, theme::DANGER, 40)).expect("the badge's text");
    assert!(badge.x0 >= 1079 && badge.x0 <= 1086, "16 px in, its 6 px padding: {badge:?}");
}
