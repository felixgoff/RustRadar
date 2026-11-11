//! Item T6's acceptance shots: the selected train's halo and ring, and its route, drawn by the
//! whole app (`--screenshot`: the train state, the trains' scene builder, the map) over the
//! committed tiles, each beside the same shot with no train selected.
//!
//! - `--demo` with a demo train selected: the halo under the train and the ring round it, where it
//!   is drawn. The demo trains have no route (as today: `#demo` asks no backend), so nothing else.
//! - The route needs a `--fixture` folder: the demo trains as `trains_in_view.json`,
//!   Skånetrafiken's set to stand at its next stop 400 m on, and its route in `train_routes.json`
//!   (the backend's `TrainRoute`): its track from 200 m behind it to 1.5 km ahead, stopping at
//!   400 m and 1150 m, passing through at 800 m. Looking along the train at zoom 15 (bearing 340°,
//!   so its heading of 70° runs left to right along the canvas's middle row): black dashes ahead
//!   of the train (`TRAIN_ROUTE_DASH`, 3 px, 2.2 on and 1.6 off in half widths), nothing behind it,
//!   the way to where it stands next 4 px in `TRAIN_TO_STOP` over them, the spot there white in a
//!   blue line under the station's light disc in a black line, the stop it passes through not
//!   drawn, the ring round the train.
//!
//! The plan's row asks for the route "in `[55,142,240,210]`": that is the selected flight's route
//! ahead (doc 03); today's train route is black dashes (G:2078-2082, doc 02 §7), which this
//! follows. There is no reference picture with trains (today's needs a Trafikverket key).
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. Shots go to `target/shots/t6-*.png`.

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use common::shots::{close, gpu, pixel_at, save};
use common::{NOW_MS, app_shot_with, options};
use rustradar_app::Options;
use rustradar_app::dev::{ahead, demo_trains};
use rustradar_app::state::AppState;
use rustradar_core::geo::Basemap;
use rustradar_core::model::{RouteStop, Train, TrainRoute};
use rustradar_core::settings::Mode;
use rustradar_map::headless::Image;
use rustradar_map::layers::train_route::{HALO_COLOR, RING_COLOR, STOP_SPOT_FILL, TRAIN_ROUTE_CASING, TRAIN_ROUTE_DASH, TRAIN_TO_STOP};
use rustradar_scene::camera::{ViewState, viewport_for};

/// The canvas, CSS px (at a scale of 1).
const SIZE: [u32; 2] = [1200, 500];
/// Where the demo trains are made: V3's centre.
const CENTRE: (f64, f64) = (59.33, 18.07);
/// Skånetrafiken's demo train (the first): heading 70°, 14 m/s, three regional cars.
const HEADING: f64 = 70.0;
/// Where it stands next, the stop it passes through and its last stop, metres on from where it is.
const STANDS_M: f64 = 400.0;
const PASSES_M: f64 = 800.0;
const LAST_M: f64 = 1150.0;

/// The demo trains as the train state makes them round [`CENTRE`] at the shots' clock.
fn demo() -> Vec<Train> {
    demo_trains(CENTRE.0, CENTRE.1, NOW_MS as i64)
}

/// `[lon, lat]` `m` metres on from Skånetrafiken's train along its heading (back for negative).
fn on_track(m: f64) -> [f64; 2] {
    let first = &demo()[0];
    let (lat, lon) = ahead((first.lat, first.lon), HEADING, m);
    [lon, lat]
}

/// Looking along Skånetrafiken's train at zoom 15, centred `m` metres ahead of it.
fn along(m: f64) -> ViewState {
    let [longitude, latitude] = on_track(m);
    ViewState { longitude, latitude, zoom: 15.0, bearing: 340.0, pitch: 0.0, position: None }
}

/// Where `[lon, lat]` is on the canvas of `view`.
fn screen(view: &ViewState, [lon, lat]: [f64; 2]) -> [f64; 2] {
    let p = viewport_for(view, SIZE.map(f64::from)).project([lon, lat, 0.0]);
    [p[0], p[1]]
}

/// The demo trains with Skånetrafiken's running on at 14 m/s until it stands at [`STANDS_M`] (two
/// frames there: it stands), and its route.
fn fixture() -> (Vec<Train>, TrainRoute) {
    let mut trains = demo();
    let first = &mut trains[0];
    let start = (first.lat, first.lon);
    first.frames = (0..=60)
        .step_by(2)
        .map(|s| {
            let (lat, lon) = ahead(start, HEADING, (14.0 * f64::from(s)).min(STANDS_M));
            (lat, lon, NOW_MS as i64 + i64::from(s) * 1000)
        })
        .collect();
    let stop = |name: &str, m: f64, stops: bool| {
        let [lon, lat] = on_track(m);
        RouteStop { name: name.into(), lat, lon, track: None, stops }
    };
    let route = TrainRoute {
        segments: vec![(-4..=30).map(|k| on_track(f64::from(k) * 50.0)).collect()],
        stops: vec![stop("Nästa", STANDS_M, true), stop("Förbi", PASSES_M, false), stop("Sista", LAST_M, true)],
        complete: true,
    };
    (trains, route)
}

/// A `--fixture` folder `name` with the trains and their routes, the committed tiles linked in.
fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).expect("the fixture folder");
    let (trains, route) = fixture();
    let routes: HashMap<String, TrainRoute> = HashMap::from([(trains[0].id.clone(), route)]);
    std::fs::write(dir.join("trains_in_view.json"), serde_json::to_vec(&trains).unwrap()).unwrap();
    std::fs::write(dir.join("train_routes.json"), serde_json::to_vec(&routes).unwrap()).unwrap();
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

/// Elsewhere the shots draw on the bare land: the comparisons hold all the same.
#[cfg(not(unix))]
fn link_tiles(_dir: &Path) {}

/// The shot of `options` with trains on the map, the camera moved to `view` once the list is in
/// (the demo trains are made round the view centre the state first asks for them at), and, when
/// `select`, the first train selected then (as a click on it does).
fn shot(gpu: &rustradar_map::Gpu, options: &Options, view: ViewState, select: bool) -> Image {
    app_shot_with(gpu, options, |state: &mut AppState| {
        state.persisted.settings.mode = Mode::Trains;
        state.poll(NOW_MS);
        state.poll(NOW_MS);
        assert_eq!(state.trains.on_map().len(), 6, "the trains are in");
        state.controller.set_view(view);
        if select {
            let first = state.trains.on_map().get(0).cloned().expect("Skånetrafiken's train");
            state.select_train(Some(first), NOW_MS);
        }
    })
}

/// `top` at its alpha over `under`.
fn over(top: [u8; 4], under: [u8; 4]) -> [u8; 4] {
    let a = f64::from(top[3]) / 255.0;
    let c = |i: usize| (f64::from(top[i]) * a + f64::from(under[i]) * (1.0 - a)).round() as u8;
    [c(0), c(1), c(2), 255]
}

/// The pixel holding `xy`.
fn at(image: &Image, xy: [f64; 2]) -> [u8; 4] {
    pixel_at(image, xy).unwrap_or_else(|| panic!("{xy:?} is off the canvas"))
}

/// The halo and the ring round the train drawn at `head` (canvas px): the ring's 2 px of blue at
/// `radius` in every direction, over the halo or the train; the halo's blue over the map inside
/// it where nothing else is; nothing changed beyond the ring but where `drawn` says something else
/// is.
fn assert_ringed(with: &Image, without: &Image, head: [f64; 2], radius: f64, drawn: impl Fn([f64; 2]) -> bool) {
    for k in 0..24 {
        let a = f64::from(k) * std::f64::consts::TAU / 24.0;
        let (dx, dy) = (a.cos(), a.sin());
        // the pixel across the ring most covered by it
        let ringed = |r: f64| {
            let p = [head[0] + dx * r, head[1] + dy * r];
            let (got, under) = (at(with, p), at(without, p));
            let want = [over(RING_COLOR, over(HALO_COLOR, under)), over(RING_COLOR, under)];
            (want.iter().map(|&w| common::shots::difference(got, w)).min().unwrap(), got, p)
        };
        let (off, got, p) = (-6..=6).map(|i| ringed(radius + f64::from(i) * 0.25)).min_by_key(|r| r.0).unwrap();
        assert!(off <= 12, "the ring at {k}/24 ({p:?}): {got:?}, {off} off the ring's blue");
        // beyond it, the map as it was
        let out = [head[0] + dx * (radius + 4.0), head[1] + dy * (radius + 4.0)];
        assert!(drawn(out) || close(at(with, out), at(without, out), 2), "beyond the ring at {k}/24 ({out:?})");
    }
    // the halo within: the map tinted where the train is not (the train drawn over the halo the
    // same in both)
    let mut tinted = 0;
    for dy in -10..=10 {
        for dx in -10..=10 {
            let p = [head[0] + f64::from(dx), head[1] + f64::from(dy)];
            let (got, under) = (at(with, p), at(without, p));
            if close(got, over(HALO_COLOR, under), 3) && got != under {
                tinted += 1;
            }
        }
    }
    assert!(tinted > 100, "the halo over the map round the train: {tinted} px");
}

/// `--demo`, a demo train selected: its halo under it and its ring round it where it is drawn (its
/// report: the shot's clock stands at the start of its frames), sized for a regional train's icon
/// (`round(21 / 2 + 5)` = 16 px) though it is drawn as its cars close in; no route.
#[test]
fn demo_train_selected_ringed_where_it_is_drawn() {
    let Some(gpu) = gpu() else { return };
    let view = along(0.0);
    let made = ViewState { longitude: CENTRE.1, latitude: CENTRE.0, ..view };
    let options = Options { demo: true, ..options(made, Basemap::Dark, SIZE, 1.0) };
    let with = shot(&gpu, &options, view, true);
    let without = shot(&gpu, &options, view, false);
    save(&with, "t6-demo-selected-1200x500.png");
    save(&without, "t6-demo-1200x500.png");
    let head = screen(&view, on_track(0.0));
    assert!((head[0] - 600.0).abs() < 0.01 && (head[1] - 250.0).abs() < 0.01, "{head:?}");
    assert_ringed(&with, &without, head, 16.0, |_| false);
    // nothing else: no route ahead of it
    for m in [100.0, 200.0, 300.0] {
        let p = screen(&view, on_track(m));
        assert_eq!(at(&with, p), at(&without, p), "nothing {m} m ahead");
    }
}

/// The route from a fixture, the train selected: dashes ahead, the way to where it stands, the
/// spot and the stations, nothing behind it, the ring round it.
#[test]
fn selected_train_route_dashed_ahead_to_its_stop_and_on() {
    let Some(gpu) = gpu() else { return };
    let view = along(550.0);
    let base = options(view, Basemap::Dark, SIZE, 1.0);
    let options = Options { fixture: Some(folder("t6-route")), ..base };
    let with = shot(&gpu, &options, view, true);
    let without = shot(&gpu, &options, view, false);
    save(&with, "t6-route-1200x500.png");
    save(&without, "t6-route-unselected-1200x500.png");
    let p = |m: f64| screen(&view, on_track(m));
    // the track runs along the middle row, left to right
    for m in [-200.0, 0.0, STANDS_M, LAST_M] {
        assert!((p(m)[1] - 250.0).abs() < 0.5, "{m} m on: {:?}", p(m));
    }
    let (x0, x_stand, x_pass, x_last) = (p(0.0)[0], p(STANDS_M)[0], p(PASSES_M)[0], p(LAST_M)[0]);
    assert!(x0 > 100.0 && x_last < 1150.0, "{x0} {x_last}");

    // the ring round the train, the way to the stop on from it
    let head = p(0.0);
    assert_ringed(&with, &without, head, 16.0, |q| q[0] > head[0] && (q[1] - head[1]).abs() < 4.0);

    // behind the train (past its last car, 77 m long), none of the route
    for m in [-180.0, -140.0, -100.0] {
        let q = p(m);
        for dy in -3..=3 {
            let r = [q[0], q[1] + f64::from(dy)];
            assert_eq!(at(&with, r), at(&without, r), "{m} m behind, {dy} px off the track");
        }
    }

    // to where it stands: 4 px of TRAIN_TO_STOP across the track (from 30 m on, clear of the ring)
    let blue = |c: [u8; 4]| close(c, TRAIN_TO_STOP, 10);
    for m in [40.0, 120.0, 200.0, 280.0, 360.0] {
        let q = p(m);
        let across = (-6..=6).filter(|&dy| blue(at(&with, [q[0], q[1] + f64::from(dy)]))).count();
        assert_eq!(across, 4, "{m} m on: {across} px of the way to the stop");
    }

    // where it stands: the station's light disc in its black line over the white spot, the
    // spot's blue line just round it
    let s = p(STANDS_M);
    let centre = at(&with, s);
    assert!(close(centre, over(TRAIN_ROUTE_CASING, STOP_SPOT_FILL), 8), "the stop's middle: {centre:?}");
    let black = |c: [u8; 4]| c[0] < 30 && c[1] < 30 && c[2] < 30;
    for (dx, dy) in [(0.0, -4.5), (0.0, 4.5), (-4.5, 0.0)] {
        let q = [s[0] + dx, s[1] + dy];
        assert!(black(at(&with, q)), "the station's line at {dx}, {dy}: {:?}", at(&with, q));
    }
    // (the spot's line from 3.75 to 6.25 px out, the station's over it to 5.5 px: a thin rim)
    let mut rim = 0;
    for dy in -8..=8 {
        for dx in -8..=8 {
            let (dx, dy) = (f64::from(dx), f64::from(dy));
            let q = [s[0] + dx, s[1] + dy];
            let r = (dx * dx + dy * dy).sqrt();
            if (5.0..=7.0).contains(&r) && dx > -4.0 && close(at(&with, q), TRAIN_TO_STOP, 40) {
                rim += 1;
            }
        }
    }
    assert!(rim >= 8, "the spot's blue line round the station: {rim} px");

    // on beyond it: dashes along the track, 3 px across, 3.3 px on and 2.4 off; the stop it
    // passes through not drawn
    let row: Vec<bool> = ((x_stand + 12.0) as u32..(x_last - 12.0) as u32).map(|x| black(at(&with, [f64::from(x), 250.0]))).collect();
    let on = row.iter().filter(|&&b| b).count() as f64 / row.len() as f64;
    assert!((on - 3.3 / 5.7).abs() < 0.08, "{on} of the row black");
    let starts = row.windows(2).filter(|w| !w[0] && w[1]).count() as f64;
    let periods = row.len() as f64 / 5.7;
    assert!((starts - periods).abs() <= periods * 0.1, "{starts} dashes in {periods} periods");
    // across: how much of each pixel of the column the black covers, summed (the map under it
    // from the shot without)
    let mut bodies: Vec<f64> = ((x_stand + 12.0) as u32..(x_last - 12.0) as u32)
        .map(|x| {
            (-6..=6)
                .map(|dy| {
                    let q = [f64::from(x), 250.0 + f64::from(dy)];
                    let (p, under) = (at(&with, q), at(&without, q));
                    (1.0 - f64::from(p[1]) / f64::from(under[1].max(1))).clamp(0.0, 1.0)
                })
                .sum::<f64>()
        })
        .filter(|&across| across > 2.0)
        .collect();
    bodies.sort_by(f64::total_cmp);
    let median = bodies[bodies.len() / 2];
    assert!((median - 3.0).abs() < 0.1, "3 px dashes: {median:.2} px across");
    for dy in [-6.0, -5.0, 5.0, 6.0] {
        let q = [x_pass, 250.0 + dy];
        assert_eq!(at(&with, q), at(&without, q), "no station where it passes through ({dy} px off)");
    }

    // its last stop: the light disc in its black line over the dashes (its middle the casing over
    // a dash, a gap or between)
    let l = p(LAST_M);
    let fill = at(&with, l);
    let (on_dash, on_gap) = (over(TRAIN_ROUTE_CASING, TRAIN_ROUTE_DASH), over(TRAIN_ROUTE_CASING, at(&without, l)));
    let between = (0..3).all(|c| fill[c] + 8 >= on_dash[c].min(on_gap[c]) && fill[c] <= on_dash[c].max(on_gap[c]) + 8);
    assert!(between, "the last stop's fill: {fill:?}, between {on_dash:?} and {on_gap:?}");
    for (dx, dy) in [(0.0, -4.5), (0.0, 4.5)] {
        let q = [l[0] + dx, l[1] + dy];
        assert!(black(at(&with, q)), "the last stop's line at {dx}, {dy}: {:?}", at(&with, q));
    }
}
