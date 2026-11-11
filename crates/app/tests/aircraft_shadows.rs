//! Item A6's acceptance shots: an aircraft 300 m up, read from a `--fixture` folder's
//! `live_flights.json`, drawn by the app's `--screenshot` path (`headless::render_on`: the state,
//! the motion model, the scene builders, the map) at local noon and at local midnight (`--now`).
//!
//! The folder holds no tiles, so the map is plain; the same shot with no flight in the folder is
//! the background, and the pixels darker than it are the shadow. Where the shadow should be comes
//! from the app's own scene at the same clock (the pose the motion model gives, the sun at the feed
//! clock) through `layers::aircraft::shadows::cast`.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. Shots go to `target/shots/a6-app-*.png`.

mod common;

use std::path::PathBuf;

use common::app_shot;
use common::shots::{gpu, save};
use rustradar_app::Options;
use rustradar_app::scene_builder::SceneBuilder;
use rustradar_app::state::{AppState, Startup};
use rustradar_core::geo::Basemap;
use rustradar_core::model::{LiveFlight, LiveSnapshot};
use rustradar_core::sun::sun_direction;
use rustradar_map::SceneInput;
use rustradar_map::headless::Image;
use rustradar_map::layers::aircraft::shadows::{cast, shadow_fade};
use rustradar_scene::camera::{ViewState, viewport_for};

/// 2026-06-21 10:44 UTC: midsummer's local (solar) noon at 19.5° E, the sun due south 55° up.
const NOON_MS: f64 = 1_782_038_640_000.0;
/// 22:44 UTC the same day: local midnight there, the sun 8° below the horizon.
const NIGHT_MS: f64 = 1_782_081_840_000.0;
const SIZE: [u32; 2] = [900, 600];
/// Over the Baltic between Gotland and Latvia, far from any airport (so the motion model draws the
/// reported altitude as it is), on the flat map, straight down.
const VIEW: ViewState = ViewState { longitude: 19.5, latitude: 58.5, zoom: 13.5, bearing: 0.0, pitch: 0.0, position: None };
/// 300 m in feet, as the feed reports it.
const ALT_FT: i32 = 984;

/// An A320 at the view's centre, 300 m up, heading north-east at 120 kt, reported two seconds
/// before `now_ms`; or no flight at all.
fn snapshot(now_ms: f64, with_flight: bool) -> LiveSnapshot {
    let flight = LiveFlight {
        id: 6006,
        lat: VIEW.latitude as f32,
        lon: VIEW.longitude as f32,
        track: 40,
        alt: ALT_FT,
        speed: 120,
        on_ground: false,
        timestamp_ms: (now_ms - 2000.0) as u64,
        callsign: "TST6".into(),
        flight: "TS6".into(),
        reg: "SE-TSS".into(),
        typecode: "A20N".into(),
        icon: "A320".into(),
        ..Default::default()
    };
    LiveSnapshot { flights: if with_flight { vec![flight] } else { Vec::new() }, server_time_ms: now_ms as u64, requests: 1, ..Default::default() }
}

/// A `--fixture` folder with the snapshot of [`snapshot`] (its server time sets the feed clock)
/// and no tiles.
fn folder(now_ms: f64, with_flight: bool) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("a6-{now_ms}-{with_flight}"));
    common::fixtures::write_fixture(&dir, &snapshot(now_ms, with_flight)).expect("the fixture folder");
    dir
}

fn options(now_ms: f64, with_flight: bool) -> Options {
    Options {
        view: Some(VIEW),
        basemap: Some(Basemap::Dark),
        size: Some(SIZE),
        scale: Some(1.0),
        now_ms: Some(now_ms),
        fixture: Some(folder(now_ms, with_flight)),
        ..Default::default()
    }
}

/// The scene the app builds from the folder at `now_ms`, as `--screenshot` does.
fn app_scene(now_ms: f64) -> SceneInput {
    let startup = Startup { basemap: Some(Basemap::Dark), fixture: Some(folder(now_ms, true)), demo: false };
    let mut state = AppState::new(startup, None, None, None, VIEW, SIZE.map(f64::from));
    state.poll(now_ms);
    assert_eq!(state.aircraft.flights().len(), 1, "the flight is in");
    let clocks = state.clocks(now_ms);
    SceneBuilder::default().build(&mut state, clocks, 1.0)
}

/// How much darker `p` is than `bg` (the mean of the three channels).
fn darker(bg: [u8; 4], p: [u8; 4]) -> f64 {
    (0..3).map(|c| f64::from(bg[c]) - f64::from(p[c])).sum::<f64>() / 3.0
}

/// The pixels of `image` more than 8 levels darker than `plain`.
fn dark(image: &Image, plain: &Image) -> Vec<[u32; 2]> {
    (0..image.height).flat_map(|y| (0..image.width).map(move |x| [x, y])).filter(|&[x, y]| darker(plain.pixel(x, y), image.pixel(x, y)) > 8.0).collect()
}

/// An aircraft 300 m up at local noon has a darker blob under it, where the sun casts it: north of
/// the point below by 300 m / tan(55°); at local midnight there is none.
#[test]
fn an_aircraft_300_m_up_has_a_shadow_at_noon_and_none_at_night() {
    let Some(gpu) = gpu() else { return };
    let mpp = viewport_for(&VIEW, SIZE.map(f64::from)).meters_per_pixel();
    for (now_ms, name) in [(NOON_MS, "noon"), (NIGHT_MS, "night")] {
        let scene = app_scene(now_ms);
        assert_eq!(scene.feed_now_ms, now_ms, "the feed clock is the snapshot's server time");
        let aircraft = &scene.aircraft.scene;
        assert_eq!((aircraft.groups.len(), aircraft.mesh_count()), (1, 1), "{name}: drawn as a mesh");
        let a320 = &aircraft.groups[0].instances[0];
        assert!((a320.pose.alt_ft * 0.3048 - 300.0).abs() < 1.0, "{name}: {} ft", a320.pose.alt_ft);
        let shadow = cast(0, a320, sun_direction(scene.feed_now_ms), shadow_fade(VIEW.zoom));

        let image = app_shot(&gpu, &options(now_ms, true));
        let plain = app_shot(&gpu, &options(now_ms, false));
        save(&image, &format!("a6-app-{name}.png"));
        let dark = dark(&image, &plain);
        let n = dark.len() as f64;
        let Some(shadow) = shadow else {
            assert_eq!(name, "night");
            assert_eq!(dark, Vec::<[u32; 2]>::new(), "no shadow at night");
            continue;
        };
        assert_eq!(name, "noon");
        assert_eq!(shadow.color, [4, 8, 16, 128], "the darkest a shadow gets, 300 m is crisp");
        // where the shadow's origin lands: the point below the aircraft moved away from the sun
        let below = viewport_for(&VIEW, SIZE.map(f64::from)).project([shadow.position[0], shadow.position[1], 0.0]);
        let [east, north] = [shadow.transform[12], shadow.transform[13]];
        let origin = [below[0] + east / mpp, below[1] - north / mpp];
        let reach = a320.position[2] / shadow.to_sun[2].asin().tan();
        let centroid = [0, 1].map(|k| dark.iter().map(|p| f64::from(p[k]) + 0.5).sum::<f64>() / n);
        eprintln!(
            "noon: {n} pixels darker, around {centroid:?}; the shadow's origin at {origin:?}, {:.1} m north of the point below ({reach:.1} m expected), {:.1} m east",
            north,
            east
        );
        // an A320 at the floor of 24 px: about a hundred pixels
        assert!(n > 80.0, "{n} pixels");
        let altitude = shadow.to_sun[2].asin().to_degrees();
        assert!((54.0..56.0).contains(&altitude), "the sun {altitude}° up");
        assert!((east.hypot(north) - reach).abs() < 1e-6 && east.abs() < 0.05 * reach && (205.0..235.0).contains(&reach), "{east} m east, {north} m north");
        // the blob's middle within 3 px of the origin's (the airframe's area is not centred on it)
        assert!((centroid[0] - origin[0]).hypot(centroid[1] - origin[1]) < 3.0, "{centroid:?} for {origin:?}");
        // the map half way to the navy ink there
        let [x, y] = centroid.map(|c| c as u32);
        let (p, b) = (image.pixel(x, y), plain.pixel(x, y));
        eprintln!("noon: at the middle {p:?} over {b:?}");
        assert!(darker(b, p) > (f64::from(b[0]) - 16.0) * 0.5 - 2.0, "{p:?} over {b:?}");
    }
}
