//! Item F2 (the stages 5-7 review's gap 7): a shot's remote pictures. `--screenshot` installs the
//! app's picture loader (`rustradar_app::images::connect_offline`), which serves the `--fixture`
//! folder's `images/<host>/<path>` files and nothing else, so a shot shows the photos and logos
//! its fixture has and never asks the network for the others.
//!
//! - `f2-flight-photo.png`: `--screenshot --ui --fixture DIR --select ID` at 1440 × 900 on the
//!   dark map, the flight panel's photo (`cdn.jetphotos.com`) and the airline's logo
//!   (`cdn.flightradar24.com`) read from the folder: the picture's sky and ground in the photo
//!   block under the strip, its credit, the logo's dark blue bar in its light tile;
//! - `f2-flight-no-photo.png`: the same folder without `images/`: the photo block is the flat
//!   `--surface-raised` block and the logo is left out, as today when they do not load.
//!
//! Compared by eye with `u10-flight-live.png` (item U10's shot of the same panel, its pictures
//! from a test's loader): the same photo block, credit, count and logo tile on the same pixels.
//!
//! The GPU tests need a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without
//! one they print `skipped: …` and pass.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::NOW_MS;
use common::shots::{Rect, bounds, count, gpu, pixel, save};
use egui::Color32;
use rustradar_app::images::FIXTURE_IMAGES;
use rustradar_app::state::aircraft::{FIXTURE_LIVE, FIXTURE_REFERENCE, FIXTURE_TOP};
use rustradar_app::state::flight::FIXTURE_DETAILS;
use rustradar_app::ui::theme;
use rustradar_app::Options;
use rustradar_core::geo::Basemap;
use rustradar_core::model::{
    AircraftImage, AircraftInfo, Airline, Airport, FlightDetails, FlightInfo, FlightProgress, LiveFlight, LiveSnapshot, ReferenceData,
    ScheduleInfo, TrailPoint, airline_logo_url,
};
use rustradar_map::Gpu;
use rustradar_map::headless::Image;
use rustradar_scene::camera::ViewState;

/// The desktop reference pictures' canvas, CSS px, at scale 1.
const SIZE: [u32; 2] = [1440, 900];
/// The flight (SAS1417, Stockholm to Copenhagen).
const ID: u32 = 0x3a8f_21c4;
const LOGO_ID: u32 = 9;
/// The pictures' URLs, as Flightradar24's details name them.
fn photo_url(n: usize) -> String {
    format!("https://cdn.jetphotos.com/400/6/f2-photo-{n}.png")
}

// ------------------------------------------------------------------------------------------------
// the fixture folder

/// An airport from `[name, IATA, ICAO, city, country, time zone]` at `[lat, lon]`.
fn airport(id: u32, [name, iata, icao, city, country, timezone]: [&str; 6], [lat, lon]: [f64; 2]) -> Airport {
    Airport { id, name: name.into(), iata: iata.into(), icao: icao.into(), city: city.into(), country: country.into(), timezone: timezone.into(), size: 100, lat, lon, ..Default::default() }
}

fn reference() -> ReferenceData {
    ReferenceData {
        airlines: vec![Airline { name: "SAS".into(), iata: "SK".into(), icao: "SAS".into() }],
        airports: vec![
            airport(1, ["Stockholm Arlanda Airport", "ARN", "ESSA", "Stockholm", "Sweden", "Europe/Stockholm"], [59.6519, 17.9186]),
            airport(2, ["Copenhagen Kastrup Airport", "CPH", "EKCH", "Copenhagen", "Denmark", "Europe/Copenhagen"], [55.6179, 12.656]),
        ],
        ..Default::default()
    }
}

/// The feed's row, last reported 2 s before the shot.
fn row() -> LiveFlight {
    LiveFlight {
        id: ID,
        lat: 57.62,
        lon: 15.41,
        track: 214,
        alt: 36_000,
        speed: 448,
        timestamp_ms: (NOW_MS - 2_000.0) as u64,
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

/// The stream's details: three photos, the schedule with its operator's logo, the progress.
fn details() -> FlightDetails {
    let image = |n: usize| AircraftImage { url: format!("https://www.jetphotos.com/photo/{n}"), copyright: "Jonas Lindqvist".into(), medium: photo_url(n), ..Default::default() };
    let trail = (0..60)
        .map(|i| TrailPoint { timestamp: 1_782_034_980 + i * 52, latitude: 59.65 - i as f32 * 0.034, longitude: 17.92 - i as f32 * 0.042, altitude: (i as i32 * 1400).min(36_000), ground_speed: 448, track: 214, ..Default::default() })
        .collect();
    FlightDetails {
        aircraft: Some(AircraftInfo {
            icao_address: 0x4a_ca8f,
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
            timestamp_ms: (NOW_MS - 2_000.0) as u64,
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

/// `width` × `height` RGBA pixels as a PNG.
fn png(width: usize, height: usize, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(rgba).unwrap();
    writer.finish().unwrap();
    out
}

/// A 3:2 photo of an aircraft against the sky (item U10's test picture): a gradient, the ground,
/// a pale fuselage.
fn photo(n: usize) -> Vec<u8> {
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
    png(w, h, &rgba)
}

/// The logo's dark blue.
const LOGO_BLUE: Color32 = Color32::from_rgb(0, 48, 135);

/// A 200 × 60 airline logo: a dark blue bar on transparency.
fn logo() -> Vec<u8> {
    let (w, h) = (200usize, 60usize);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 10..=50 {
        for x in 10..=190 {
            rgba[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[LOGO_BLUE.r(), LOGO_BLUE.g(), LOGO_BLUE.b(), 255]);
        }
    }
    png(w, h, &rgba)
}

/// The fixture's picture for `url` at the path the loader reads it from (`images/<host>/<path>`).
fn put_picture(dir: &Path, url: &str, bytes: &[u8]) {
    let file = rustradar_app::images::fixture_file(dir, url).expect("a fixture path");
    assert!(file.starts_with(dir.join(FIXTURE_IMAGES)));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, bytes).unwrap();
}

/// A `--fixture` folder named `name` with the flight, its details and, if `pictures`, its photos
/// and its airline's logo.
fn folder(name: &str, pictures: bool) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(FIXTURE_DETAILS)).unwrap();
    let snapshot = LiveSnapshot { flights: vec![row()], server_time_ms: NOW_MS as u64, requests: 1, ..Default::default() };
    std::fs::write(dir.join(FIXTURE_LIVE), serde_json::to_vec(&snapshot).unwrap()).unwrap();
    std::fs::write(dir.join(FIXTURE_TOP), b"[]").unwrap();
    std::fs::write(dir.join(FIXTURE_REFERENCE), serde_json::to_vec(&reference()).unwrap()).unwrap();
    std::fs::write(dir.join(FIXTURE_DETAILS).join(format!("{ID}.json")), serde_json::to_vec(&details()).unwrap()).unwrap();
    if pictures {
        for n in 1..=3 {
            put_picture(&dir, &photo_url(n), &photo(n - 1));
        }
        put_picture(&dir, &airline_logo_url(LOGO_ID), &logo());
    }
    dir
}

/// `--screenshot --ui --fixture dir --select ID` at 1440 × 900 on the dark map at V1.
fn shot(gpu: &Gpu, dir: PathBuf) -> Image {
    let options = Options {
        view: Some(ViewState::INITIAL),
        basemap: Some(Basemap::Dark),
        size: Some(SIZE),
        scale: Some(1.0),
        now_ms: Some(NOW_MS),
        fixture: Some(dir),
        ui: true,
        select: Some(ID),
        wait: Some(Duration::from_secs(30)),
        ..Default::default()
    };
    common::app_shot(gpu, &options)
}

// ------------------------------------------------------------------------------------------------
// checks

fn rgb(c: Color32) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

fn close(p: [u8; 4], c: Color32, tolerance: u8) -> bool {
    (0..3).all(|i| p[i].abs_diff(rgb(c)[i]) <= tolerance)
}

/// The flight panel's top border: the first `--line` row down its left part (x 1100 and 1150; the
/// dev readout of a debug build covers its top right), from the top; the panel is 360 wide inside
/// its border at right 16 (x 1062–1424, item U10).
fn panel_top(image: &Image) -> u32 {
    (0..400).find(|&y| close(pixel(image, 1100, y), theme::LINE, 2) && close(pixel(image, 1150, y), theme::LINE, 2)).expect("the panel's top border")
}

#[test]
fn the_flight_panel_s_photo_and_logo_come_from_the_fixture_folder() {
    let Some(gpu) = gpu() else { return };
    let image = shot(&gpu, folder("f2-photos", true));
    save(&image, "f2-flight-photo.png");
    let top = panel_top(&image);
    // the panel's border at U10's columns
    assert!(close(pixel(&image, 1062, top + 300), theme::LINE, 2) && close(pixel(&image, 1423, top + 300), theme::LINE, 2), "the panel's sides");
    // the strip's rule 68 under the border, the photo's 180 px under it: its sky, then its ground
    let photo_top = top + 1 + 68;
    assert!(close(pixel(&image, 1100, photo_top - 1), theme::LINE, 2), "the strip's rule");
    let sky = pixel(&image, 1100, photo_top + 40);
    assert!(sky[2] > sky[0] + 60, "the sky of the fixture's picture: {sky:?}");
    assert!(close(pixel(&image, 1100, photo_top + 175), Color32::from_rgb(70, 80, 70), 6), "its ground");
    // the credit at its bottom right in --text-1 over the picture
    let credit = Rect { x0: 1300, y0: photo_top + 150, x1: 1415, y1: photo_top + 178 };
    assert!(count(&image, credit, |p| close(p, theme::TEXT_1, 40)) > 20, "the credit");
    // the airline row under the photo: the logo's dark blue bar in its light tile
    let airline = Rect { x0: 1063, y0: photo_top + 180, x1: 1423, y1: photo_top + 180 + 60 };
    let tile = bounds(&image, airline, |p| close(p, theme::TEXT_1, 3)).expect("the logo tile");
    assert!(count(&image, tile, |p| close(p, LOGO_BLUE, 12)) > 200, "the logo in its tile: {tile:?}");
}

#[test]
fn without_the_pictures_the_photo_is_its_block_and_the_logo_is_left_out() {
    let Some(gpu) = gpu() else { return };
    let image = shot(&gpu, folder("f2-no-photos", false));
    save(&image, "f2-flight-no-photo.png");
    let top = panel_top(&image);
    let photo_top = top + 1 + 68;
    for y in [photo_top + 40, photo_top + 175] {
        assert_eq!(pixel(&image, 1100, y), rgb(theme::SURFACE_RAISED), "the flat block at {y}");
    }
    let airline = Rect { x0: 1063, y0: photo_top + 180, x1: 1423, y1: photo_top + 180 + 60 };
    assert_eq!(count(&image, airline, |p| close(p, LOGO_BLUE, 12)), 0, "no logo");
}
