//! Item T3's acceptance shots: the demo trains (`--demo`, `dev::demo_trains`) close in, drawn as
//! their cars by the whole app (`--screenshot`: the train state, the scene builders, the map) over
//! the committed tiles, each beside the same shot without `--demo` (the map under them).
//!
//! The demo trains are made round the view centre when the train state first asks for them, at the
//! start of their frames (the shot's clock stands still), their track known 400 m back. For the
//! measured shots the camera is then moved and turned (as a pan would) to look along
//! Skånetrafiken's train, three regional cars in its purple heading 70°: at bearing 340° it runs
//! left to right along the canvas's middle row, its head on the right. Today's `#demo` draws no
//! train (no key in a plain browser), so there is no reference picture with trains; the map under
//! them is the committed tiles' dark map.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. Shots go to `target/shots/t3-*.png`.

mod common;

use common::shots::{Rect, V3, difference, gpu, save};
use common::{NOW_MS, app_shot_with, options};
use rustradar_app::Options;
use rustradar_app::dev::demo_trains;
use rustradar_core::geo::{Basemap, meters_per_pixel};
use rustradar_core::model::Train;
use rustradar_core::trains::train_color_of;
use rustradar_map::Gpu;
use rustradar_map::frame::ViewFrame;
use rustradar_map::headless::Image;
use rustradar_map::layers::train_cars::{MATERIAL, models};
use rustradar_map::renderer::scene_lights;
use rustradar_scene::camera::{ViewState, viewport_for};
use rustradar_scene::trains::{DrawnCar, TrainBuilder};

/// The canvas of the measured shots, CSS px.
const SIZE: [u32; 2] = [1000, 500];
/// Where the demo trains are made: V3's centre, at zoom 16.
const AT: ViewState = ViewState { longitude: 18.07, latitude: 59.33, zoom: 16.0, bearing: 0.0, pitch: 0.0, position: None };
/// Metres per degree of latitude, the trains' flat metres.
const M_PER_DEG: f64 = 111_320.0;

/// The demo trains as the train state makes them for `AT`, and the cars of the first
/// (Skånetrafiken's) as the trains' scene builder lays them out at the shot's clock.
fn layout() -> (Vec<Train>, Vec<DrawnCar>) {
    let trains = demo_trains(AT.latitude, AT.longitude, NOW_MS as i64);
    let list: Vec<&Train> = trains.iter().collect();
    let mut builder = TrainBuilder::new();
    assert!(builder.build(&list, true, &AT, None, NOW_MS as i64).cars, "zoom 16: cars");
    let layers = builder.car_layers(&list, &AT, SIZE.map(f64::from), models(), NOW_MS as i64);
    let cars = layers.cars.into_iter().filter(|c| c.train == 0).collect();
    (trains, cars)
}

/// The shot of `view` on a canvas of `size` CSS px at `scale` through the app, with the demo trains
/// made round `AT` and the camera moved to `view` once they are there, or the same without them.
fn shot(gpu: &Gpu, view: ViewState, size: [u32; 2], scale: f64, demo: bool) -> Image {
    let options = Options { demo, ..options(AT, Basemap::Dark, size, scale) };
    app_shot_with(gpu, &options, |state| {
        state.poll(NOW_MS);
        state.controller.set_view(view);
    })
}

/// Looking along Skånetrafiken's train (heading 70°): centred on `[lon, lat]`, turned to bearing
/// 340° so the train runs left to right, at `pitch`.
fn along([lon, lat]: [f64; 2], pitch: f64) -> ViewState {
    ViewState { longitude: lon, latitude: lat, zoom: 16.0, bearing: 340.0, pitch, position: None }
}

/// `(lat, lon)` `m` metres from `at` going `heading`.
fn ahead((lat, lon): (f64, f64), heading: f64, m: f64) -> (f64, f64) {
    let h = heading.to_radians();
    (lat + h.cos() * m / M_PER_DEG, lon + h.sin() * m / (M_PER_DEG * lat.to_radians().cos()))
}

/// `car`'s front and back, `[lon, lat]`: half its length ahead of and behind its middle.
fn ends(car: &DrawnCar) -> ([f64; 2], [f64; 2]) {
    let at = (car.at[1], car.at[0]);
    let ((f_lat, f_lon), (b_lat, b_lon)) = (ahead(at, car.heading, car.length / 2.0), ahead(at, car.heading, -car.length / 2.0));
    ([f_lon, f_lat], [b_lon, b_lat])
}

/// What `with` adds to `without` at pixel `(x, y)`, per colour channel.
fn off(with: &Image, without: &Image, x: u32, y: u32) -> [f64; 3] {
    let (a, b) = (with.pixel(x, y), without.pixel(x, y));
    [0, 1, 2].map(|c| f64::from(a[c]) - f64::from(b[c]))
}

fn size(v: [f64; 3]) -> f64 {
    v.iter().map(|c| c * c).sum::<f64>().sqrt()
}

/// How much of pixel `(x, y)` the drawing covers, 0–1: what it adds there against what it adds
/// at the strongest of the pixels `inside` (fully covered, of the same colour).
fn share(with: &Image, without: &Image, (x, y): (u32, u32), inside: &[u32]) -> f64 {
    let q = inside.iter().map(|&i| off(with, without, i, y)).max_by(|u, v| size(*u).total_cmp(&size(*v))).unwrap();
    let p = off(with, without, x, y);
    ((p[0] * q[0] + p[1] * q[1] + p[2] * q[2]) / (size(q) * size(q))).clamp(0.0, 1.0)
}

/// The ends of what is drawn along row `y` of `with` (over `without`) within `reach` px of the
/// canvas's middle: the first and last pixel it changes, each counted by its share. `(left, right)`.
fn train_ends(with: &Image, without: &Image, y: u32, reach: u32) -> (f64, f64) {
    let mid = with.width / 2;
    let on: Vec<u32> = (mid - reach..mid + reach).filter(|&x| size(off(with, without, x, y)) > 8.0).collect();
    let (a, b) = (on[0], on[on.len() - 1]);
    let right = f64::from(b) + share(with, without, (b, y), &[b - 1, b - 2, b - 3]);
    let left = f64::from(a) + 1.0 - share(with, without, (a, y), &[a + 1, a + 2, a + 3]);
    (left, right)
}

/// The canvas of the shots centred on one car, CSS px, and their scale: the 0.6 m gaps 3 device
/// pixels wide.
const CAR_SIZE: [u32; 2] = [400, 400];
const CAR_SCALE: f64 = 3.0;

/// Straight down at zoom 16 along Skånetrafiken's train. Centred on the train: its three cars run
/// back from where the train stands (its front car's front there, the rest on the side its heading
/// points away from: the tail runs backward from the head), the whole train 3 × 26 − 0.6 m long.
/// Then centred on each car in turn at 3 device pixels to the CSS pixel (its ends face away from
/// the camera, and the 0.6 m gaps show the ground between the cars): each car is its own 25.4 m
/// (`length / mpp`) within a pixel, and the gaps are where the layout puts them.
#[test]
fn zoom_16_demo_train_runs_back_from_its_head_each_car_its_length() {
    let Some(gpu) = gpu() else { return };
    let (trains, cars) = layout();
    let keys: Vec<String> = cars.iter().map(|c| c.mesh.key()).collect();
    assert_eq!(keys, ["regional-cab", "regional-coach", "regional-coach"]);
    assert!(cars.iter().all(|c| (c.length - 25.4).abs() < 1e-3 && c.color == [196, 92, 222, 240]), "{cars:?}");
    // centred on the train's middle, looking along it
    let (front, _) = ends(&cars[0]);
    let (_, back) = ends(&cars[2]);
    let view = along([(front[0] + back[0]) / 2.0, (front[1] + back[1]) / 2.0], 0.0);
    let with = shot(&gpu, view, SIZE, 1.0, true);
    let without = shot(&gpu, view, SIZE, 1.0, false);
    save(&with, "t3-z16-along-demo.png");
    save(&without, "t3-z16-along.png");
    let mpp = meters_per_pixel(16.0, view.latitude);
    let (left, right) = train_ends(&with, &without, SIZE[1] / 2, 100);
    // the front car's front where the train stands; the cars behind it, none ahead
    let head = viewport_for(&view, SIZE.map(f64::from)).project([trains[0].lon, trains[0].lat, 0.0])[0];
    assert!((right - head).abs() <= 1.0, "the front at {right:.2}, the train at {head:.2}");
    assert!(left < head - 100.0, "the cars run back (left) from the head: {left:.2}");
    let whole = 3.0 * 26.0 - 0.6;
    assert!((right - left - whole / mpp).abs() <= 1.0, "{:.2} px against {:.2} ({whole} m)", right - left, whole / mpp);
    // each car, centred: from the middle out to the ground on either side
    let canvas = CAR_SIZE.map(f64::from);
    for (k, car) in cars.iter().enumerate() {
        let view = along(car.at, 0.0);
        let with = shot(&gpu, view, CAR_SIZE, CAR_SCALE, true);
        let without = shot(&gpu, view, CAR_SIZE, CAR_SCALE, false);
        if k == 1 {
            save(&with, "t3-z16-car-x3-demo.png");
        }
        let (y, mid) = (with.height / 2, with.width / 2);
        let on = |x: u32| size(off(&with, &without, x, y)) > 8.0;
        let (mut a, mut b) = (mid, mid);
        while on(b + 1) {
            b += 1;
        }
        while on(a - 1) {
            a -= 1;
        }
        let right = f64::from(b) + share(&with, &without, (b, y), &[b - 1, b - 2, b - 3]);
        let left = f64::from(a) + 1.0 - share(&with, &without, (a, y), &[a + 1, a + 2, a + 3]);
        let got = (right - left) / CAR_SCALE;
        let want = car.length / mpp;
        eprintln!("car {k}: {got:.2} px, {want:.2} px long");
        assert!((got - want).abs() <= 1.0, "car {k}: {got:.2} px against {want:.2}");
        // the ground between it and the cars beside it: where the layout has the gaps
        let viewport = viewport_for(&view, canvas);
        let x_of = |[lon, lat]: [f64; 2]| viewport.project([lon, lat, 0.0])[0] * CAR_SCALE;
        let beside = [(k.checked_sub(1), 1.0), ((k + 1 < cars.len()).then_some(k + 1), -1.0)];
        for (other, side) in beside {
            let Some(other) = other else { continue };
            let (ahead_end, behind_end) = if side > 0.0 { (ends(&cars[other]).1, ends(car).0) } else { (ends(car).1, ends(&cars[other]).0) };
            let want = (x_of(ahead_end) + x_of(behind_end)) / 2.0;
            // the gap starts at this car's end; half of 0.6 m on is its middle
            let edge = if side > 0.0 { right } else { left };
            let got = edge + side * 0.3 / mpp * CAR_SCALE;
            eprintln!("car {k}: the gap {} at {got:.2}, laid out at {want:.2} (device px)", if side > 0.0 { "ahead" } else { "behind" });
            assert!((got - want).abs() <= CAR_SCALE, "car {k}: the gap at {got:.2}, laid out at {want:.2}");
            assert!(!on(want as u32), "car {k}: the ground shows in the gap ({want:.1})");
        }
    }
}

/// Tilted 60° and looking along the same train, its middle car's roof (grey shade 0.75, facing
/// up) and its near side (shade 1, facing 160°, toward the camera) are each lit as the CPU twin of
/// the mesh shader lights them with the app's rig (the key light aimed at the sun over the flat
/// map with buildings on) and `CAR_MESH_MATERIAL`, at the trains' alpha 240 over the face drawn
/// before it (the body's top under the roof, the underframe's top behind the side): their own
/// normals, not flat colours.
#[test]
fn zoom_16_demo_car_roof_and_side_are_lit_as_deck_lights_them() {
    let Some(gpu) = gpu() else { return };
    let (_, cars) = layout();
    let middle = &cars[1];
    let view = along(middle.at, 60.0);
    let with = shot(&gpu, view, SIZE, 1.0, true);
    let without = shot(&gpu, view, SIZE, 1.0, false);
    save(&with, "t3-z16-tilted-demo.png");
    let canvas = SIZE.map(f64::from);
    let viewport = viewport_for(&view, canvas);
    let screen = |[lon, lat]: [f64; 2], h: f64| {
        let [x, y, _] = viewport.project([lon, lat, h]);
        (x as u32, y as u32)
    };
    // the twin: the scene's rig, deck's camera, the cars' material, the train's purple
    let rig = scene_lights(&view, NOW_MS, true);
    let frame = ViewFrame::new(&view, canvas, 1.0, 0.0);
    let camera = frame.globals.camera_common.map(f64::from);
    let lit = |shade: f64, normal: [f64; 3], at: [f64; 2], h: f64| {
        let p = frame.common_offset(at, h).to_array().map(f64::from);
        let to_camera = [camera[0] - p[0], camera[1] - p[1], camera[2] - p[2]];
        let surface = [196.0, 92.0, 222.0].map(|c: f64| c / 255.0 * shade);
        rig.shade(&MATERIAL, surface, normal.into(), to_camera.into(), rig.key_direction.into()).map(|c| c.min(1.0) * 255.0)
    };
    let a = 240.0 / 255.0;
    let over = |top: [f64; 3], under: [f64; 3]| [0, 1, 2].map(|c| top[c] * a + under[c] * (1.0 - a));
    let ground = |p: (u32, u32)| {
        let g = without.pixel(p.0, p.1);
        [0, 1, 2].map(|c| f64::from(g[c]))
    };
    let up = [0.0, 0.0, 1.0];
    // the roof's middle, 4.1 m up, over the body's top (3.6 m), over the window band's
    let roof_at = screen(middle.at, 4.1);
    let roof = over(lit(0.75, up, middle.at, 4.1), over(lit(0.9, up, middle.at, 0.88 * 4.1), ground(roof_at)));
    // the lower body band's middle on the near side, 1.55 m toward 160° and 1.43 m up, over the
    // underframe's top (0.8 m up, half a metre in)
    let (slat, slon) = ahead((middle.at[1], middle.at[0]), middle.heading + 90.0, 1.55);
    let side_at = screen([slon, slat], 0.35 * 4.1);
    let h = (middle.heading + 90.0).to_radians();
    let (ulat, ulon) = ahead((middle.at[1], middle.at[0]), middle.heading + 90.0, 0.5);
    let side = over(lit(1.0, [h.sin(), h.cos(), 0.0], [slon, slat], 0.35 * 4.1), over(lit(0.22, up, [ulon, ulat], 0.2 * 4.1), ground(side_at)));
    for (what, at, want) in [("roof", roof_at, roof), ("side", side_at, side)] {
        let got = with.pixel(at.0, at.1);
        eprintln!("{what} at {at:?}: {got:?}, the twin {want:.1?}");
        for c in 0..3 {
            assert!((f64::from(got[c]) - want[c]).abs() <= 4.0, "{what}: {got:?} against {want:.1?}");
        }
    }
    assert!(difference(with.pixel(roof_at.0, roof_at.1), with.pixel(side_at.0, side_at.1)) > 15);
}

/// At V3's centre, zoom 13 (just past the switch from icons, about zoom 12.2 here) and zoom 16, the
/// whole app: every demo train is drawn as its cars along its track back from where it stands (the
/// icons are gone), at zoom 13 never thinner than 3 px.
#[test]
fn every_demo_train_is_drawn_as_its_cars_close_in() {
    let Some(gpu) = gpu() else { return };
    let size = [1440, 900];
    for zoom in [13.0, 16.0] {
        let view = ViewState { zoom, ..V3.view() };
        let options = Options { demo: true, ..options(view, Basemap::Dark, size, 1.0) };
        let with = app_shot_with(&gpu, &options, |_| {});
        let without = app_shot_with(&gpu, &Options { demo: false, ..options.clone() }, |_| {});
        save(&with, &format!("t3-z{zoom}-demo-1440x900.png"));
        let viewport = viewport_for(&view, size.map(f64::from));
        let trains = demo_trains(view.latitude, view.longitude, NOW_MS as i64);
        for t in &trains {
            // the middle of its second car, or of its only one
            let back = if t.cars == Some(1) { 13.0 } else { 39.0 };
            let (lat, lon) = ahead((t.lat, t.lon), f64::from(t.heading.unwrap()), -back);
            let [x, y, _] = viewport.project([lon, lat, 0.0]);
            let near = Rect::around(&with, [x, y], 3.0);
            let n = near.pixels().filter(|&(px, py)| difference(with.pixel(px, py), without.pixel(px, py)) > 12).count();
            assert!(n >= 6, "zoom {zoom}: {} ({:?}) at ({x:.0}, {y:.0}): {n} pixels", t.name, train_color_of(t));
        }
        if zoom == 13.0 {
            // never thinner than 3 px: across the long intercity train (Öresundståg, heading 160°)
            // at its sixth car, where no other train runs (it lies 20° off the vertical on the
            // screen, so a row crosses it nearly square)
            let t = &trains[2];
            let (lat, lon) = ahead((t.lat, t.lon), 160.0, -150.0);
            let [x, y, _] = viewport.project([lon, lat, 0.0]);
            let row: Vec<u32> = (x as u32 - 10..x as u32 + 10).filter(|&px| difference(with.pixel(px, y as u32), without.pixel(px, y as u32)) > 12).collect();
            assert!((3..=6).contains(&row.len()), "{row:?}");
        }
    }
}
