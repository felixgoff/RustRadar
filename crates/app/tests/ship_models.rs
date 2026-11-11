//! Item H3's acceptance shot: ships at V6 (Gothenburg harbour) at zoom 15, from a fixture
//! (`--fixture`'s `ships_in_view.json`), drawn by the whole app (`--screenshot`: the ship state,
//! the scene builders, the map) beside the same shot with the ships off the map.
//!
//! Three ships under way are long enough on the screen at zoom 15 to be models: a 100 m cargo ship
//! heading east, an 80 m passenger ship heading north and a 30 m tug heading west, each placed so
//! its hull's middle (not its AIS reference point, which the dimensions put off the middle) is on
//! a device pixel's centre, the shot drawn at 2 device pixels to the CSS pixel (so a pointed bow's
//! last sliver and an end pixel of another colour than the one inside it cost less). Each is as
//! long on the screen as the ship, `length / mpp` CSS px, and as wide as its beam, within a pixel,
//! and its decks are lit as the scene's rig and `SHIP_MESH_MATERIAL` light them. An 8 m pleasure boat (a model only from zoom 16.2) and a moored ship (no heading,
//! never a model) stay icons.
//!
//! The committed tiles have none for V6 at zoom 15 (the satellite map asks for z16 imagery, the
//! dark map for z14 vector tiles), so the ships stand on the bare map; there is no reference
//! picture with ships (today's needs the backend). All ships are between the sidebar's and the
//! detail panel's places (stage 7 draws its cards over the map in the same shot).
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. Shots go to `target/shots/h3-*.png`.

mod common;

use std::path::{Path, PathBuf};

use common::shots::{Rect, V6, difference, gpu, save};
use common::{NOW_MS, app_shot_with, options};
use rustradar_app::Options;
use rustradar_app::state::AppState;
use rustradar_core::geo::{Basemap, meters_per_pixel};
use rustradar_core::model::Ship;
use rustradar_core::settings::Mode;
use rustradar_map::frame::ViewFrame;
use rustradar_map::headless::Image;
use rustradar_map::layers::ship_models::MATERIAL;
use rustradar_map::renderer::scene_lights;
use rustradar_scene::camera::{ViewState, viewport_for};
use rustradar_scene::ship_models::{ShipClass, ship_class_of, ship_placement};

/// The reference pictures' canvas.
const SIZE: [u32; 2] = [1440, 900];
/// V6 at zoom 15: 1.28 m a pixel.
const VIEW: ViewState = ViewState { zoom: 15.0, ..V6.view() };
/// Device pixels to the CSS pixel.
const SCALE: f64 = 2.0;
/// Metres per degree of latitude, the ships' flat metres.
const M_PER_DEG: f64 = 111_320.0;

/// One ship of the fixture: where its hull's middle stands on the canvas, its type, heading and
/// AIS dimensions (bow, stern, port, starboard from its reference point).
struct Spot {
    xy: [f64; 2],
    kind: u8,
    heading: Option<f32>,
    dims: [u16; 4],
}

const SPOTS: [Spot; 5] = [
    // a cargo ship, its reference point 10 m aft and 1 m to port of the hull's middle; the
    // middles on device pixels' centres
    Spot { xy: [500.25, 300.25], kind: 4, heading: Some(90.0), dims: [60, 40, 7, 9] },
    Spot { xy: [720.25, 450.25], kind: 3, heading: Some(0.0), dims: [40, 40, 7, 7] },
    Spot { xy: [940.25, 600.25], kind: 2, heading: Some(270.0), dims: [12, 18, 5, 5] },
    // icons: a pleasure boat too short for a model yet, a moored ship
    Spot { xy: [600.0, 700.0], kind: 8, heading: Some(45.0), dims: [4, 4, 1, 1] },
    Spot { xy: [850.0, 250.0], kind: 6, heading: None, dims: [90, 90, 15, 15] },
];

/// `(lat, lon)` `m` metres from `at` going `heading`.
fn ahead((lat, lon): (f64, f64), heading: f64, m: f64) -> (f64, f64) {
    let h = heading.to_radians();
    (lat + h.cos() * m / M_PER_DEG, lon + h.sin() * m / (M_PER_DEG * lat.to_radians().cos()))
}

/// The ships of [`SPOTS`]: each reported back from its hull's middle by the hull's offset.
fn ships() -> Vec<Ship> {
    let viewport = viewport_for(&VIEW, SIZE.map(f64::from));
    SPOTS
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let [lon, lat] = viewport.unproject(s.xy, 0.0);
            let mut ship = Ship {
                mmsi: 265_100_000 + i as u32,
                lat,
                lon,
                name: format!("SHIP {i}"),
                kind: s.kind,
                size: 1,
                heading: s.heading,
                old: false,
                dims: Some(s.dims),
                frames: Vec::new(),
            };
            if let Some(heading) = s.heading.map(f64::from) {
                let place = ship_placement(&ship);
                let (lat, lon) = ahead((lat, lon), heading, -place.ahead);
                (ship.lat, ship.lon) = ahead((lat, lon), heading + 90.0, -place.right);
            }
            ship
        })
        .collect()
}

/// A `--fixture` folder with the ships and the committed tiles.
fn folder() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("h3-ships-v6-z15");
    std::fs::create_dir_all(&dir).expect("the fixture folder");
    std::fs::write(dir.join("ships_in_view.json"), serde_json::to_vec(&ships()).unwrap()).unwrap();
    link_tiles(&dir);
    dir
}

#[cfg(unix)]
fn link_tiles(dir: &Path) {
    let link = dir.join("tiles");
    if std::fs::symlink_metadata(&link).is_err() {
        std::os::unix::fs::symlink(rustradar_map::io::fixture_tiles_dir(), &link).expect("the tiles linked in");
    }
}

#[cfg(not(unix))]
fn link_tiles(_dir: &Path) {}

/// What `with` adds to `without` at pixel `(x, y)`, per colour channel.
fn off(with: &Image, without: &Image, x: i64, y: i64) -> [f64; 3] {
    let (a, b) = (with.pixel(x as u32, y as u32), without.pixel(x as u32, y as u32));
    [0, 1, 2].map(|c| f64::from(a[c]) - f64::from(b[c]))
}

fn size(v: [f64; 3]) -> f64 {
    v.iter().map(|c| c * c).sum::<f64>().sqrt()
}

/// How far what `with` draws over `without` reaches along the line through pixel `[x0, y0]` going
/// `[dx, dy]` (a unit step along a row or a column), px: the run of pixels it changes, each end
/// pixel counted by how much of the drawn colour it shows (against whichever of the three pixels
/// inside it is most like it in colour), as its share of the pixel covered.
fn extent(with: &Image, without: &Image, [x0, y0]: [i64; 2], [dx, dy]: [i64; 2]) -> f64 {
    let at = |k: i64| off(with, without, x0 + k * dx, y0 + k * dy);
    let on = |k: i64| size(at(k)) > 8.0;
    let (mut a, mut b) = (0, 0);
    while on(a - 1) {
        a -= 1;
    }
    while on(b + 1) {
        b += 1;
    }
    let share = |end: i64, inner: [i64; 3]| {
        let p = at(end);
        let like = |q: &[f64; 3]| (p[0] * q[0] + p[1] * q[1] + p[2] * q[2]) / (size(p) * size(*q)).max(1e-9);
        let q = inner.map(at).into_iter().max_by(|u, v| like(u).total_cmp(&like(v))).unwrap();
        ((p[0] * q[0] + p[1] * q[1] + p[2] * q[2]) / (size(q) * size(q))).clamp(0.0, 1.0)
    };
    (b - a - 1) as f64 + share(a, [a + 1, a + 2, a + 3]) + share(b, [b - 1, b - 2, b - 3])
}

/// V6 at zoom 15 with the ships on the map, and the same with them off it.
fn shots() -> Option<(Image, Image)> {
    let gpu = gpu()?;
    let options = Options { fixture: Some(folder()), ..options(VIEW, Basemap::Satellite, SIZE, SCALE) };
    let ships_on = |state: &mut AppState| state.persisted.settings.mode = Mode::Ships;
    let with = app_shot_with(&gpu, &options, ships_on);
    let without = app_shot_with(&gpu, &options, |_| {});
    save(&with, "h3-v6-z15-ships-1440x900.png");
    save(&without, "h3-v6-z15-1440x900.png");
    Some((with, without))
}

/// V6 at zoom 15: each model as long as its ship and as wide as its beam within a pixel, its decks
/// lit, the short and the moored ship icons; nothing else on the map changed.
#[test]
fn v6_zoom_15_hulls_are_their_ships_lengths_and_lit() {
    let Some((with, without)) = shots() else { return };
    let ships = ships();
    let classes: Vec<ShipClass> = ships.iter().map(ship_class_of).collect();
    assert_eq!(classes[..3], [ShipClass::Cargo, ShipClass::Passenger, ShipClass::Tug]);
    let mpp = meters_per_pixel(VIEW.zoom, VIEW.latitude);
    for (i, (spot, ship)) in SPOTS.iter().zip(&ships).enumerate().take(3) {
        let place = ship_placement(ship);
        let middle = spot.xy.map(|v| (v * SCALE) as i64);
        let (along, across) = if spot.heading == Some(0.0) { ([0, 1], [1, 0]) } else { ([1, 0], [0, 1]) };
        let long = extent(&with, &without, middle, along) / SCALE;
        let wide = extent(&with, &without, middle, across) / SCALE;
        let (want_long, want_wide) = (place.length / mpp, place.beam / mpp);
        eprintln!("ship {i} ({:?}): {long:.2} by {wide:.2} px, the ship {want_long:.2} by {want_wide:.2}", classes[i]);
        assert!((long - want_long).abs() <= 1.0, "ship {i}: {long:.2} px long against {want_long:.2}");
        assert!((wide - want_wide).abs() <= 1.0, "ship {i}: {wide:.2} px wide against {want_wide:.2}");
    }
    // the cargo ship's second hatch cover, its middle 3.75 m aft of the hull's middle on the centre
    // line, 0.067 of its 100 m up: its blue-grey top lit by the scene's rig (the key light aimed at
    // the sun over the flat map, buildings on) as the CPU twin of the mesh shader lights it, not
    // the flat paint (the passenger ship's white decks are lit past white)
    let cargo = &ships[0];
    let up = ship_placement(cargo).scale[2];
    let canvas = SIZE.map(f64::from);
    let viewport = viewport_for(&VIEW, canvas);
    let [lon, lat] = viewport.unproject(SPOTS[0].xy, 0.0);
    let (lat, lon) = ahead((lat, lon), 90.0, -3.75);
    let (at, h) = ([lon, lat], (0.055 + 0.012) * up);
    let [x, y, _] = viewport.project([lon, lat, h]);
    let rig = scene_lights(&VIEW, NOW_MS, true);
    let frame = ViewFrame::new(&VIEW, canvas, SCALE, 0.0);
    let camera = frame.globals.camera_common.map(f64::from);
    let p = frame.common_offset(at, h).to_array().map(f64::from);
    let to_camera = [camera[0] - p[0], camera[1] - p[1], camera[2] - p[2]];
    let paint = [84.0, 116.0, 148.0];
    let want = rig.shade(&MATERIAL, paint.map(|c: f64| c / 255.0), [0.0, 0.0, 1.0].into(), to_camera.into(), rig.key_direction.into()).map(|c| c.min(1.0) * 255.0);
    let got = with.pixel((x * SCALE) as u32, (y * SCALE) as u32);
    eprintln!("the hatch cover at {x:.1},{y:.1}: {got:?}, the twin {want:.1?}");
    for c in 0..3 {
        assert!((f64::from(got[c]) - want[c]).abs() <= 4.0, "{got:?} against {want:.1?}");
    }
    assert!((0..3).any(|c| (f64::from(got[c]) - paint[c]).abs() > 8.0), "lit, not the flat paint: {got:?}");
    // the icons: the pleasure boat's hull and the moored ship's dot, in their type colours
    for spot in &SPOTS[3..] {
        let around = Rect::around(&with, spot.xy.map(|v| v * SCALE), 10.0 * SCALE).clip(&with);
        let n = around.pixels().filter(|&(x, y)| difference(with.pixel(x, y), without.pixel(x, y)) > 12).count() as f64 / (SCALE * SCALE);
        assert!((20.0..300.0).contains(&n), "an icon at {:?}: {n} CSS pixels", spot.xy);
    }
    // nothing else changed on the map between the sidebar's and the detail panel's places
    let k = SCALE as u32;
    let map = Rect::new(352 * k, 70 * k, (1080 - 352) * k, (860 - 70) * k);
    for (x, y) in map.pixels().filter(|&(x, y)| difference(with.pixel(x, y), without.pixel(x, y)) > 12) {
        let [x, y] = [x, y].map(|v| (f64::from(v) + 0.5) / SCALE);
        let d = SPOTS.iter().map(|s| (x - s.xy[0]).hypot(y - s.xy[1])).fold(f64::INFINITY, f64::min);
        assert!(d < 45.0, "a pixel changed at {x},{y}, {d:.1} px from the nearest ship");
    }
}
