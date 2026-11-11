//! Item A8's acceptance shot: a flight of `landings.json` selected (KLM76F, Munich to Amsterdam,
//! landing on 27 at Schiphol on 2026-10-09), drawn as the app draws it.
//!
//! The app's state runs as the window runs it: the aircraft feed answers each round with the
//! flight's latest report (a canned feed: the recorded reports, one snapshot per request), the scene
//! is rebuilt every 100 ms (the motion model takes the reports in as the aircraft is posed), the
//! flight is selected ten seconds before the shot, and its follow stream sends the details with the
//! trail recorded so far (canned too). The reference data has Munich and Schiphol. The shot is
//! drawn over the dark map's land with no tiles (the committed tiles do not cover Amsterdam), 15 s
//! after the touchdown report, on the globe at zoom 11, pitched 40°.
//!
//! What it checks (plan, A8's row):
//! - the trail: 3 px wide, each segment in its start point's `altitude_color` (from `(255,151,0)`
//!   at 2150 ft to orange `(255,98,0)` at the ground end);
//! - its shadow on the ground under the raised part: a 1.5 px line of `[241,238,231,60]` over the
//!   land;
//! - the selection ring `[55,142,240,230]`, 1.5 px, round the aircraft's drawn position;
//! - the route's airport dots `[55,142,240]` with their codes above (Schiphol's on screen), and the
//!   two codes left out of the map's labels.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. The shot goes to `target/shots/a8-selected-flight.png`.

mod common;

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use common::shots::{gpu, map_shot, save};
use rustradar_app::scene_builder::SceneBuilder;
use rustradar_app::state::aircraft::{Aircraft, Answer, Feeds, LoadedReference};
use rustradar_app::state::flight::{AdsbQuery, Flight, FlightFeeds, FollowState, FollowStream, SelectOptions};
use rustradar_app::state::{AppState, Startup};
use rustradar_core::geo::{Basemap, altitude_color};
use rustradar_core::model::{
    AdsbSnapshot, AircraftInfo, Airport, BoundingBox, FlightDetails, FlightInfo, FollowEvent, LiveFlight, LiveSnapshot, OpenSkyPoint,
    ReferenceData, Runway, SessionInfo, TopFlight, TrailPoint,
};
use rustradar_map::SceneInput;
use rustradar_map::headless::Image;
use rustradar_map::layers::markers::{MARKER_FILL, RING_COLOR};
use rustradar_map::layers::route::TRAIL_SHADOW_COLOR;
use rustradar_scene::camera::{ViewState, viewport_for};
use serde_json::Value;

/// The canvas, CSS px (scale 1).
const SIZE: [f64; 2] = [1100.0, 720.0];
/// The land under everything (no tiles).
const LAND: [f64; 3] = [50.0, 50.0, 50.0];

/// One recorded report: `[timestampMs, lat, lon, alt ft, ground speed kt, track, onGround]`.
#[derive(Debug, Clone, Copy)]
struct Report {
    ms: f64,
    lat: f64,
    lon: f64,
    alt: i32,
    speed: i32,
    track: i32,
    on_ground: bool,
}

/// KLM76F's reports, oldest first, and its id.
fn klm76f() -> (u32, Vec<Report>) {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../core/tests/data/landings.json")).expect("landings.json");
    let data: Value = serde_json::from_str(&text).expect("landings.json is JSON");
    let flight = data["flights"].as_array().unwrap().iter().find(|f| f["callsign"] == "KLM76F").expect("KLM76F");
    let reports = flight["reports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| Report {
            ms: r[0].as_f64().unwrap(),
            lat: r[1].as_f64().unwrap(),
            lon: r[2].as_f64().unwrap(),
            alt: r[3].as_i64().unwrap() as i32,
            speed: r[4].as_i64().unwrap() as i32,
            track: r[5].as_i64().unwrap() as i32,
            on_ground: r[6].as_i64().unwrap() == 1,
        })
        .collect();
    (flight["id"].as_u64().unwrap() as u32, reports)
}

/// The feed's row for report `r` of flight `id`.
fn row(id: u32, r: &Report) -> LiveFlight {
    LiveFlight {
        id,
        lat: r.lat as f32,
        lon: r.lon as f32,
        track: r.track,
        alt: r.alt,
        speed: r.speed,
        on_ground: r.on_ground,
        timestamp_ms: r.ms as u64,
        callsign: "KLM76F".into(),
        flight: "KL1776".into(),
        reg: "PH-NXA".into(),
        typecode: "E295".into(),
        origin: "MUC".into(),
        destination: "AMS".into(),
        icon: "E190".into(),
        ..Default::default()
    }
}

/// The latest report at local time `t`.
fn latest(reports: &[Report], t: f64) -> Option<&Report> {
    reports.iter().rev().find(|r| r.ms <= t)
}

/// The aircraft feed: each request answered with the flight's latest report at the shared clock.
struct Recorded {
    id: u32,
    reports: Vec<Report>,
    now: Arc<Mutex<f64>>,
    reference: ReferenceData,
}

impl Feeds for Recorded {
    fn live_flights(&mut self, _area: Option<BoundingBox>, _categories: Option<Vec<u8>>) -> Answer<Result<LiveSnapshot, String>> {
        let now = *self.now.lock().unwrap();
        let flights = latest(&self.reports, now).map(|r| row(self.id, r)).into_iter().collect();
        Answer::ready(Ok(LiveSnapshot { flights, server_time_ms: now as u64, requests: 1, ..Default::default() }))
    }

    fn top_flights(&mut self) -> Answer<Result<Vec<TopFlight>, String>> {
        Answer::ready(Ok(Vec::new()))
    }

    fn reference_data(&mut self) -> Answer<Result<LoadedReference, String>> {
        Answer::ready(Ok(LoadedReference::new(self.reference.clone())))
    }

    fn session_info(&mut self) -> Option<SessionInfo> {
        None
    }
}

/// The follow stream (its sender kept for the test); no OpenSky track, adsb.lol or runways known.
struct Stream(Arc<Mutex<Option<Sender<FollowEvent>>>>);

impl FlightFeeds for Stream {
    fn follow_flight(&mut self, _id: u32) -> Result<FollowStream, String> {
        let (tx, rx) = mpsc::channel();
        *self.0.lock().unwrap() = Some(tx);
        Ok(FollowStream::Channel(rx))
    }

    fn unfollow_flight(&mut self) {}

    fn opensky_track(&mut self, _icao24: &str) -> Answer<Result<Vec<OpenSkyPoint>, String>> {
        Answer::ready(Err("no OpenSky track in this test".into()))
    }

    fn adsb(&mut self, _query: &AdsbQuery) -> Answer<Result<AdsbSnapshot, String>> {
        Answer::ready(Err("no adsb.lol in this test".into()))
    }

    fn airport_runways(&mut self, _codes: Vec<String>) -> Answer<Result<HashMap<String, Vec<Runway>>, String>> {
        Answer::ready(Ok(HashMap::new()))
    }
}

/// The stream's details at local time `t`: the latest report, and the trail recorded so far.
fn details(reports: &[Report], t: f64) -> FlightDetails {
    let r = latest(reports, t).expect("a report");
    FlightDetails {
        aircraft: Some(AircraftInfo { icao_address: 0x484c55, reg: "PH-NXA".into(), typecode: "E295".into(), ..Default::default() }),
        flight: Some(FlightInfo {
            timestamp_ms: r.ms as u64,
            latitude: r.lat as f32,
            longitude: r.lon as f32,
            track: r.track,
            altitude: r.alt,
            ground_speed: r.speed,
            on_ground: r.on_ground,
            callsign: "KLM76F".into(),
            ..Default::default()
        }),
        trail: reports
            .iter()
            .filter(|p| p.ms <= t)
            .map(|p| TrailPoint {
                timestamp: (p.ms / 1000.0).floor() as u64,
                latitude: p.lat as f32,
                longitude: p.lon as f32,
                altitude: p.alt,
                ground_speed: p.speed as u32,
                track: p.track as u32,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

fn airport(id: u32, iata: &str, icao: &str, lat: f64, lon: f64, alt: i64) -> Airport {
    Airport { id, iata: iata.into(), icao: icao.into(), name: iata.into(), lat, lon, alt: Some(alt), ..Default::default() }
}

/// The app's scene 15 s after KLM76F's touchdown report, the flight selected 10 s before.
fn scene() -> SceneInput {
    let (id, reports) = klm76f();
    let touchdown = reports.last().unwrap().ms;
    let shot = touchdown + 15_000.0;
    let view = ViewState { longitude: 4.86, latitude: 52.315, zoom: 11.0, pitch: 0.0, bearing: 0.0, position: None };
    let mut state = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, view, SIZE);
    // the trail is measured against the bare land: the night (B5), over Schiphol at the recorded
    // landing's hour, is off
    state.persisted.settings.daylight = false;
    let now = Arc::new(Mutex::new(reports[0].ms + 500.0));
    let reference = ReferenceData {
        airports: vec![airport(1, "AMS", "EHAM", 52.3086, 4.7639, -11), airport(2, "MUC", "EDDM", 48.3538, 11.7861, 1487)],
        ..Default::default()
    };
    state.aircraft = Aircraft::with_feeds(Some(Box::new(Recorded { id, reports: reports.clone(), now: now.clone(), reference })));
    let stream = Arc::new(Mutex::new(None));
    state.flight = Flight::with_feeds(Some(Box::new(Stream(stream.clone()))));
    let mut builder = SceneBuilder::default();
    let mut t = reports[0].ms + 500.0;
    let select_at = shot - 10_000.0;
    let mut selected = false;
    let mut last = None;
    while t <= shot {
        *now.lock().unwrap() = t;
        if !selected && t >= select_at {
            state.select_flight(Some(id), SelectOptions::default(), t);
            selected = true;
        }
        if selected {
            // the stream sends the details every few seconds
            let sender = stream.lock().unwrap().clone().expect("the stream started");
            if ((t - select_at) % 5_000.0) < 100.0 {
                sender.send(FollowEvent::Details(Box::new(details(&reports, t)))).unwrap();
            }
        }
        state.poll(t);
        state.advance_camera(t);
        let clocks = state.clocks(t);
        last = Some(builder.build(&mut state, clocks, 1.0));
        t += 100.0;
    }
    assert_eq!(state.flight.follow_state(), FollowState::Live);
    assert_eq!(state.flight.selected().map(|f| f.id), Some(id));
    last.expect("frames")
}

/// Where `[lon, lat, h]` is on the canvas, CSS px (= device px at scale 1).
fn screen(scene: &SceneInput, [lon, lat, h]: [f64; 3]) -> [f64; 2] {
    let p = viewport_for(&scene.view, scene.size).project([lon, lat, h]);
    [p[0], p[1]]
}

fn pixel(image: &Image, [x, y]: [f64; 2]) -> Option<[u8; 4]> {
    (x >= 0.0 && y >= 0.0 && x < f64::from(image.width) && y < f64::from(image.height)).then(|| image.pixel(x as u32, y as u32))
}

fn distance(p: [u8; 4], c: [f64; 3]) -> f64 {
    (0..3).map(|i| (f64::from(p[i]) - c[i]).abs()).fold(0.0, f64::max)
}

/// `color` over the land at its alpha.
fn over_land(color: [u8; 4]) -> [f64; 3] {
    let a = f64::from(color[3]) / 255.0;
    [0, 1, 2].map(|i| LAND[i] + (f64::from(color[i]) - LAND[i]) * a)
}

#[test]
fn a_selected_landing_shows_its_trail_shadow_ring_and_airports() {
    let Some(gpu) = gpu() else { return };
    let scene = scene();
    let (image, _) = map_shot(&gpu, &scene, 1.0, |_| Vec::new());
    save(&image, "a8-selected-flight.png");
    // the same scene pitched: the raised trail stands clear of its shadow
    let mut pitched = scene.clone();
    pitched.view.pitch = 40.0;
    let (tilted, _) = map_shot(&gpu, &pitched, 1.0, |_| Vec::new());
    save(&tilted, "a8-selected-flight-pitched.png");

    // the trail as the scene builder made it: the recorded points up to the drawn time, then the
    // aircraft; the colours by altitude
    let trail = scene.route.trail.as_ref().expect("a trail");
    let n = trail.path.len();
    assert!(n >= 30, "most of the recorded trail is drawn: {n} points");
    let [r, g, b] = altitude_color(2150.0);
    assert_eq!(trail.colors[0], [r, g, b, 255]);
    assert_eq!(trail.colors[0], [255, 151, 0, 255], "2150 ft");
    assert_eq!(trail.colors[n - 2], [255, 98, 0, 255], "the touchdown report, at the ground end");
    assert_eq!(trail.colors[n - 1], [255, 98, 0, 255], "the aircraft on the runway");

    // each segment long enough on the screen: its start colour at its middle, 3 px across
    let mut checked = 0;
    let mut widths = Vec::new();
    for i in 0..n - 1 {
        let (a, b) = (screen(&scene, trail.path[i]), screen(&scene, trail.path[i + 1]));
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = dx.hypot(dy);
        if len < 8.0 {
            continue;
        }
        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let c = trail.colors[i].map(f64::from);
        let want = [c[0], c[1], c[2]];
        let normal = [-dy / len, dx / len];
        let across: Vec<[u8; 4]> = (-6..=6).filter_map(|k| pixel(&image, [mid[0] + normal[0] * f64::from(k), mid[1] + normal[1] * f64::from(k)])).collect();
        let best = across.iter().map(|&p| distance(p, want)).fold(f64::INFINITY, f64::min);
        assert!(best <= 3.0, "segment {i}: its colour {want:?} at its middle, best {best} in {across:?}");
        // the line's coverage down the pixel column through its middle, from the blue channel (0
        // on the trail, the land's 50 off it; the shadow, the ring and the markers are bluer than
        // the land and count nothing), times the cosine of its slope: its width
        let column: f64 = (-8..=8)
            .filter_map(|k| pixel(&image, [mid[0], mid[1] + f64::from(k)]))
            .map(|p| ((50.0 - f64::from(p[2])) / 50.0).max(0.0))
            .sum();
        widths.push(column * dx.abs() / len);
        checked += 1;
    }
    assert!(checked >= 15, "{checked} segments checked");
    let mean = widths.iter().sum::<f64>() / widths.len() as f64;
    assert!((2.7..=3.3).contains(&mean), "3 px wide: {mean} on average, {widths:?}");
    assert!(widths.iter().all(|w| (2.4..=3.6).contains(w)), "{widths:?}");

    // the shadow under the raised start of the trail: on the ground, clear of the trail
    let start = trail.path[0];
    assert!(start[2] > 500.0, "2150 ft up, drawn {} m above Schiphol", start[2]);
    let lifted = screen(&pitched, start);
    let ground = screen(&pitched, [start[0], start[1], 0.0]);
    assert!((ground[1] - lifted[1]).abs() > 8.0, "pitched 40°: the trail stands clear of its shadow ({lifted:?}, {ground:?})");
    let shadow = over_land(TRAIL_SHADOW_COLOR);
    // deck turns the 1.5 px into ground metres at the view centre's scale (straight down there)
    // and the line lies on the ground: pitched, it is foreshortened across the view
    let centre = [pitched.view.longitude, pitched.view.latitude];
    let px_per_m = (screen(&scene, [centre[0], centre[1] + 100.0 / 111_320.0, 0.0])[1] - screen(&scene, [centre[0], centre[1], 0.0])[1]).abs() / 100.0;
    let width_m = 1.5 / px_per_m;
    for k in 0..6 {
        // the middles of the first segments' shadows: the brightest pixel down the column there,
        // and the line's coverage (one shadow's colour over the land is full coverage)
        let [a, b] = [trail.path[k], trail.path[k + 1]];
        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let (at, up) = (screen(&pitched, [mid[0], mid[1], 0.0]), screen(&pitched, [mid[0], mid[1], (a[2] + b[2]) / 2.0]));
        assert!((at[1] - up[1]).abs() > 8.0, "segment {k} clear of its shadow");
        let column: Vec<f64> = (-4..=4).filter_map(|d| pixel(&tilted, [at[0], at[1] + f64::from(d)])).map(|p| f64::from(p[0])).collect();
        let brightest = column.iter().copied().fold(0.0, f64::max);
        let coverage: f64 = column.iter().map(|r| (r - LAND[0]) / (shadow[0] - LAND[0])).sum();
        let expected = (screen(&pitched, [mid[0], mid[1] + width_m / 111_320.0, 0.0])[1] - at[1]).abs();
        assert!(brightest > LAND[0] + 10.0 && brightest <= shadow[0] + 2.0, "shadow {k}: brightest {brightest}, one shadow is {shadow:?} ({column:?})");
        assert!((coverage - expected).abs() <= 0.25, "shadow {k}: covers {coverage} px down the column, {expected} expected ({column:?})");
    }

    // the ring round the aircraft's drawn position
    let ring = scene.markers.ring.expect("a ring");
    let centre = screen(&scene, ring.position);
    let on_ring = (0..72)
        .filter(|k| {
            let a = f64::from(*k) * 5f64.to_radians();
            [-0.5, 0.0, 0.5].iter().any(|dr| {
                let r = ring.radius_px + dr;
                pixel(&image, [centre[0] + r * a.cos(), centre[1] + r * a.sin()]).is_some_and(|p| u16::from(p[2]) > u16::from(p[0]) + 100)
            })
        })
        .count();
    assert!(on_ring >= 60, "the ring all round: {on_ring} of 72 directions");
    let ring_color = over_land(RING_COLOR);
    let mut best = f64::INFINITY;
    for k in 0..72 {
        let a = f64::from(k) * 5f64.to_radians();
        for dr in [-0.5, 0.0, 0.5] {
            let r = ring.radius_px + dr;
            if let Some(p) = pixel(&image, [centre[0] + r * a.cos(), centre[1] + r * a.sin()]) {
                best = best.min(distance(p, ring_color));
            }
        }
    }
    assert!(best <= 4.0, "the ring's colour {ring_color:?} over the land: off by {best}");
    let inside = (0..36)
        .filter(|k| {
            let a = f64::from(*k) * 10f64.to_radians();
            pixel(&image, [centre[0] + 0.5 * ring.radius_px * a.cos(), centre[1] + 0.5 * ring.radius_px * a.sin()])
                .is_some_and(|p| u16::from(p[2]) > u16::from(p[0]) + 100)
        })
        .count();
    assert!(inside <= 4, "not filled: {inside}");

    // the route's airports: both marked, Schiphol on screen with its code above its dot
    let codes: Vec<&str> = scene.markers.route_airports.iter().map(|a| a.code.as_str()).collect();
    assert_eq!(codes, ["MUC", "AMS"]);
    assert_eq!(scene.labels.marked, ["MUC", "AMS"], "their labels left out of the map's");
    let ams = &scene.markers.route_airports[1];
    let dot = screen(&scene, [ams.lon, ams.lat, 0.0]);
    let fill = [0, 1, 2].map(|i| f64::from(MARKER_FILL[i]));
    let mut blue = 0;
    let mut light_above = 0;
    for dy in -30i32..=8 {
        for dx in -20i32..=20 {
            let Some(p) = pixel(&image, [dot[0] + f64::from(dx), dot[1] + f64::from(dy)]) else { continue };
            if dx.abs() <= 6 && dy.abs() <= 6 && distance(p, fill) <= 3.0 {
                blue += 1;
            }
            if (-24..=-10).contains(&dy) && p[0] > 200 && p[1] > 200 && p[2] > 200 {
                light_above += 1;
            }
        }
    }
    assert!(blue >= 30, "Schiphol's dot: {blue} px of {MARKER_FILL:?}");
    assert!(light_above >= 20, "its code above it: {light_above} light px");
}
