//! Item A9's checks through the app as the window runs it: winit's events into
//! `app::Engine::window_event`, frames through `Engine::frame` (the state, the pick step, the scene
//! builders, the map, the egui pass) onto an offscreen target, with the stage-4 synthetic snapshot
//! (`common::fixtures`) read as the `--fixture` folder's `live_flights.json`.
//!
//! - the pointer over an aircraft highlights it in the very next frame, `[55,142,240,150]` over
//!   white, and nothing else changes; the cursor is the pointing hand over it and the open hand
//!   off it;
//! - a click (press and release, the click fired 300 ms later) on the aircraft selects it, drawn in
//!   selection blue; a click on empty map lets it go;
//! - the place and airport labels as the labels layer draws them: a click on an airport's code
//!   opens its board (the focused airport) and lets the selected flight go, one on a place's name
//!   changes nothing.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. Shots go to `target/shots/a9-*.png`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::NOW_MS;
use common::fixtures::{self, ARLANDA, STOCKHOLM, WORLD};
use common::shots::{gpu, save};
use rustradar_app::app::Engine;
use rustradar_app::scene_builder::SceneBuilder;
use rustradar_app::state::pick::{Target, pick_at};
use rustradar_app::state::{AppState, Startup};
use rustradar_core::geo::Basemap;
use rustradar_map::headless::{FORMAT, Image, Offscreen, render_with};
use rustradar_map::io::DirTiles;
use rustradar_map::{Gpu, MapRenderer, SceneInput};
use rustradar_scene::aircraft::{HIGHLIGHT_COLOR, icon_pick_radius};
use rustradar_scene::camera::{ViewState, viewport_for};
use winit::dpi::PhysicalPosition;
use winit::event::{DeviceId, ElementState, MouseButton, WindowEvent};

/// The shots' canvas, at pixel ratio 1.
const SIZE: [u32; 2] = [1100, 720];
/// Stockholm at zoom 9 (plan section 8's V3): icons for the published models.
const V3: ViewState = ViewState { latitude: 59.33, longitude: 18.07, zoom: 9.0, pitch: 0.0, bearing: 0.0, position: None };

/// A `--fixture` folder holding the synthetic snapshot as `live_flights.json`.
fn snapshot_folder() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("a9-synthetic");
    fixtures::write_fixture(&dir, &fixtures::synthetic_snapshot(NOW_MS)).expect("the fixture folder");
    dir
}

/// The app's state at `view` on the dark map with the snapshot of `folder` read.
fn state_at(folder: &Path, view: ViewState) -> AppState {
    let startup = Startup { basemap: Some(Basemap::Dark), fixture: Some(folder.to_path_buf()), demo: false };
    let mut state = AppState::new(startup, None, None, None, view, SIZE.map(f64::from));
    state.poll(NOW_MS);
    assert_eq!(state.aircraft.flights().len(), WORLD + STOCKHOLM + ARLANDA, "the snapshot is in");
    state
}

/// The window's frames, drawn offscreen.
struct Window {
    engine: Engine,
    target: Offscreen,
    /// egui's clock, seconds.
    egui_s: f64,
}

impl Window {
    fn new(gpu: &Gpu, state: AppState) -> Window {
        let mut engine = Engine::new(gpu, FORMAT, state);
        engine.map.set_tiles(Arc::new(DirTiles::fixtures()), None);
        Window { engine, target: Offscreen::new(gpu, SIZE), egui_s: 0.0 }
    }

    /// One frame at local time `now`, returning egui's output (with the cursor) and the picture.
    fn frame(&mut self, gpu: &Gpu, now: f64) -> (egui::PlatformOutput, Image) {
        let mut input = egui::RawInput {
            time: Some(self.egui_s),
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(SIZE[0] as f32, SIZE[1] as f32))),
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(1.0);
        self.egui_s += 0.25;
        let (output, _) = self.engine.frame(gpu, self.target.view(), SIZE, 1.0, input, now);
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("the frame");
        (output, self.target.read(gpu).expect("read back"))
    }

    /// Frames at `now` until nothing is loading.
    fn settle(&mut self, gpu: &Gpu, now: f64) -> Image {
        let started = std::time::Instant::now();
        loop {
            self.frame(gpu, now);
            if !self.engine.map.pending() || started.elapsed() > rustradar_map::headless::MAX_WAIT {
                return self.frame(gpu, now).1;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn event(&mut self, event: WindowEvent, now: f64) {
        self.engine.window_event(&event, 1.0, now);
    }

    fn move_to(&mut self, [x, y]: [f64; 2], now: f64) {
        self.event(WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(x, y) }, now);
    }

    /// A left click at `at` at `now`: the click fires 300 ms after the release, in the frame then.
    /// Returns what the press picked, and the picture after the click.
    fn click(&mut self, gpu: &Gpu, at: [f64; 2], now: f64) -> (Option<Target>, Image) {
        self.move_to(at, now);
        let button = |state| WindowEvent::MouseInput { device_id: DeviceId::dummy(), state, button: MouseButton::Left };
        self.event(button(ElementState::Pressed), now);
        let pressed = self.engine.state.pick.pressed().cloned().expect("the press was picked");
        self.event(button(ElementState::Released), now + 80.0);
        (pressed, self.frame(gpu, now + 400.0).1)
    }
}

/// The icon farthest from every other aircraft on the screen (`scene`'s, in the camera of
/// `state`), where it is (rounded to whole pixels, as the pointer is) and its pick radius.
fn lone_icon(state: &AppState, scene: &SceneInput) -> (u32, [f64; 2], f64) {
    let viewport = viewport_for(&state.controller.view(), state.controller.size());
    let aircraft = &scene.aircraft.scene;
    let spots: Vec<(u32, [f64; 2])> = aircraft
        .icons
        .iter()
        .map(|i| (i.id, i.position))
        .chain(aircraft.groups.iter().flat_map(|g| g.instances.iter().map(|m| (m.id, m.position))))
        .map(|(id, p)| {
            let [x, y, _] = viewport.project(p);
            (id, [x, y])
        })
        .collect();
    let inside = |[x, y]: [f64; 2]| (120.0..980.0).contains(&x) && (120.0..600.0).contains(&y);
    let (icon, at, clear) = aircraft
        .icons
        .iter()
        .filter_map(|icon| {
            let &(_, at) = spots.iter().find(|(id, _)| *id == icon.id)?;
            let clear = spots.iter().filter(|(id, _)| *id != icon.id).map(|(_, p)| (p[0] - at[0]).hypot(p[1] - at[1])).fold(f64::INFINITY, f64::min);
            inside(at).then_some((icon, at, clear))
        })
        .max_by(|a, b| a.2.total_cmp(&b.2))
        .expect("an icon on the screen");
    assert!(clear > 80.0, "no aircraft within 80 px of {}: {clear}", icon.id);
    (icon.id, at.map(f64::round), icon_pick_radius(icon))
}

/// The pixels that differ between `a` and `b`, and their bounds `[x0, y0, x1, y1]`.
fn changed(a: &Image, b: &Image) -> (usize, [u32; 4]) {
    let mut n = 0;
    let mut bounds = [u32::MAX, u32::MAX, 0, 0];
    for y in 0..a.height {
        for x in 0..a.width {
            if a.pixel(x, y) != b.pixel(x, y) {
                n += 1;
                bounds = [bounds[0].min(x), bounds[1].min(y), bounds[2].max(x), bounds[3].max(y)];
            }
        }
    }
    (n, bounds)
}

/// The pixels of `image` within `r` of `at` that `keep` keeps.
fn count_near(image: &Image, [x, y]: [f64; 2], r: f64, keep: impl Fn([u8; 4]) -> bool) -> usize {
    let mut n = 0;
    for py in (y - r).max(0.0) as u32..((y + r) as u32).min(image.height) {
        for px in (x - r).max(0.0) as u32..((x + r) as u32).min(image.width) {
            if (f64::from(px) + 0.5 - x).hypot(f64::from(py) + 0.5 - y) <= r && keep(image.pixel(px, py)) {
                n += 1;
            }
        }
    }
    n
}

/// Within `tolerance` of `rgb` on every channel.
fn near(pixel: [u8; 4], rgb: [u8; 3], tolerance: i32) -> bool {
    (0..3).all(|k| (i32::from(pixel[k]) - i32::from(rgb[k])).abs() <= tolerance)
}

/// A bright grey: the white silhouette, its core and most of its blended edge.
fn whitish(p: [u8; 4]) -> bool {
    let (lo, hi) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
    lo >= 160 && hi - lo <= 12
}

/// White mixed toward the highlight `[55,142,240]`: blue well over red, and bright.
fn highlightish(p: [u8; 4]) -> bool {
    let [r, g, b, _] = p.map(i32::from);
    b >= 160 && b - r >= 60 && g > r && g < b
}

/// Selection blue `[55,142,240]` and its blended edge.
fn selectionish(p: [u8; 4]) -> bool {
    let [r, g, b, _] = p.map(i32::from);
    b >= 120 && b - r >= 90 && g > r && g < b
}

/// White mixed toward the highlight as deck's picking module does for an opaque aircraft:
/// `mix(rgb, highlight.rgb, a / (a + 1 − a))` with `a = 150/255`.
fn highlighted_white() -> [u8; 3] {
    let a = f64::from(HIGHLIGHT_COLOR[3]) / 255.0;
    [0, 1, 2].map(|k| (255.0 + (f64::from(HIGHLIGHT_COLOR[k]) - 255.0) * a).round() as u8)
}

#[test]
fn hover_highlights_and_a_click_selects_through_the_window() {
    let Some(shared) = gpu() else { return };
    let gpu = &*shared;
    let folder = snapshot_folder();
    let mut window = Window::new(gpu, state_at(&folder, V3));
    let before = window.settle(gpu, NOW_MS);
    save(&before, "a9-before-v3.png");
    let (id, at, r) = lone_icon(&window.engine.state, window.engine.map.scene());
    // the silhouette's own pixels (its edges are blended into the map)
    let silhouette = |image: &Image, keep: fn([u8; 4]) -> bool| count_near(image, at, r, keep);
    let white = silhouette(&before, whitish);
    assert!(white > 10, "the icon at {at:?} is drawn white: {white} pixels");
    assert_eq!(silhouette(&before, highlightish), 0);

    // the pointer over it: highlighted in the next frame, nothing else changed, a pointing hand
    window.move_to(at, NOW_MS);
    let (output, hovered) = window.frame(gpu, NOW_MS);
    save(&hovered, "a9-hover-v3.png");
    assert_eq!(window.engine.state.pick.hovered_id(), Some(id));
    assert_eq!(output.cursor_icon, egui::CursorIcon::PointingHand);
    let blue = silhouette(&hovered, highlightish);
    assert!(blue * 10 >= white * 9, "the silhouette in the highlight: {blue} of {white}");
    assert_eq!(silhouette(&hovered, whitish), 0, "no white left in it");
    let core = count_near(&hovered, at, r, |p| near(p, highlighted_white(), 3));
    assert!(core > 0, "its core exactly white mixed toward [55,142,240] at 150/255");
    let (n, [x0, y0, x1, y1]) = changed(&before, &hovered);
    let reach = r * 1.25 + 2.0;
    assert!(n > 0);
    assert!(
        f64::from(x0) >= at[0] - reach && f64::from(x1) <= at[0] + reach && f64::from(y0) >= at[1] - reach && f64::from(y1) <= at[1] + reach,
        "only the icon changed: {n} pixels in {x0},{y0}..{x1},{y1} round {at:?}"
    );

    // off it, twice (deck's leave event keeps the hand for one move): the open hand, no highlight
    let empty = [at[0] + 60.0, at[1]];
    window.move_to(empty, NOW_MS);
    window.frame(gpu, NOW_MS);
    window.move_to([empty[0] + 1.0, empty[1]], NOW_MS);
    let (output, off) = window.frame(gpu, NOW_MS);
    assert_eq!(window.engine.state.pick.hovered_id(), None);
    assert_eq!(output.cursor_icon, egui::CursorIcon::Grab);
    assert_eq!(silhouette(&off, highlightish), 0);
    assert_eq!(silhouette(&off, whitish), white, "white again");

    // a click on it selects it: selection blue
    let (pressed, selected) = window.click(gpu, at, NOW_MS + 1000.0);
    assert_eq!(pressed, Some(Target::Aircraft(id)), "the press picked it");
    save(&selected, "a9-click-selects-v3.png");
    assert_eq!(window.engine.state.flight.selected_id(), Some(id));
    let selection = silhouette(&selected, selectionish);
    assert!(selection * 10 >= white * 9, "the selected icon in selection blue: {selection} of {white}");
    assert!(count_near(&selected, at, r, |p| near(p, [55, 142, 240], 3)) > 0, "its core exactly [55,142,240]");

    // a click on empty map lets it go
    let (pressed, released) = window.click(gpu, [empty[0] + 1.0, empty[1]], NOW_MS + 2000.0);
    assert_eq!(pressed, None, "nothing under the press");
    assert_eq!(window.engine.state.flight.selected_id(), None);
    assert_eq!(silhouette(&released, selectionish), 0);
    // two seconds on by the feed's clock the aircraft has moved a few pixels
    let again = silhouette(&released, whitish);
    assert!(again * 10 >= white * 7, "drawn white again: {again} of {white}");
}

/// The labels as the labels layer draws them (the app's scene builder does not show them yet, so
/// this scene has them on): an airport's code opens its board, a place's name does nothing.
#[test]
fn a_click_on_an_airport_code_opens_its_board_and_on_a_place_does_nothing() {
    let Some(shared) = gpu() else { return };
    let gpu = &*shared;
    let folder = snapshot_folder();
    let mut state = state_at(&folder, V3);
    let clocks = state.clocks(NOW_MS);
    let mut scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
    scene.labels.shown = true;
    let mut renderer = MapRenderer::new(gpu, FORMAT);
    renderer.set_tiles(Arc::new(DirTiles::fixtures()), None);
    let image = render_with(gpu, &mut renderer, &scene, 1.0).expect("rendered");
    save(&image, "a9-labels-v3.png");
    let drawn = renderer.scene().labels.drawn.labels();
    assert!(!drawn.is_empty(), "labels drawn");

    // labels where no aircraft is within reach
    let viewport = viewport_for(&state.controller.view(), state.controller.size());
    let aircraft = renderer.scene().aircraft.scene.picks(&viewport);
    let clear = |[x, y]: [f64; 2]| aircraft.pick(&viewport, [x.round(), y.round()], 1.0).is_none();
    let airport = drawn.iter().find(|l| l.iata.is_some() && l.fade > 0.5 && clear(l.anchor)).expect("an airport's code on its own");
    let place = drawn.iter().find(|l| l.iata.is_none() && l.fade > 0.5 && clear(l.anchor)).expect("a place's name on its own");
    let code = airport.iata.clone().unwrap();
    eprintln!("airport {code} at {:?}, place {} at {:?}", airport.anchor, place.text, place.anchor);
    let pointer = |l: &rustradar_map::layers::labels::Shown| [l.anchor[0].round(), l.anchor[1].round()];
    assert_eq!(pick_at(renderer.scene(), &state.controller.view(), state.controller.size(), pointer(airport)), Some(Target::Label(Some(code.clone()))));
    assert_eq!(pick_at(renderer.scene(), &state.controller.view(), state.controller.size(), pointer(place)), Some(Target::Label(None)));

    // a flight selected, then a click on the place: nothing changes
    let flight = fixtures::FIRST_ID + 3;
    state.click_map(Some(Target::Aircraft(flight)), NOW_MS);
    assert_eq!(state.flight.selected_id(), Some(flight));
    let click = |at| rustradar_app::input::Click { at, tap_count: 1, modifiers: Default::default() };
    state.pick_press(renderer.scene(), pointer(place));
    let wants = state.pick_frame(renderer.scene(), None, false, &[click(pointer(place))], NOW_MS);
    assert!(!wants.scene);
    assert_eq!(state.flight.selected_id(), Some(flight), "a place's name does not deselect");
    assert_eq!(state.flight.focus_airport(), None);
    // a click on the airport's code: its board, the flight let go
    state.pick_press(renderer.scene(), pointer(airport));
    let wants = state.pick_frame(renderer.scene(), None, false, &[click(pointer(airport))], NOW_MS);
    assert!(wants.scene);
    assert_eq!(state.flight.focus_airport(), Some(code.as_str()));
    assert_eq!(state.flight.selected_id(), None);
}

/// Arlanda close in, pitched (plan section 8's V4): the aircraft are lit models, picked by the boxes
/// their airframes fill. The pointer over one highlights it (its pixels turn bluer) and a click
/// selects it.
#[test]
fn a_model_close_in_is_hovered_and_selected() {
    let Some(shared) = gpu() else { return };
    let gpu = &*shared;
    let v4 = ViewState { latitude: 59.6519, longitude: 17.9186, zoom: 13.5, pitch: 45.0, bearing: 30.0, position: None };
    let folder = snapshot_folder();
    let mut window = Window::new(gpu, state_at(&folder, v4));
    let before = window.settle(gpu, NOW_MS);
    save(&before, "a9-before-v4.png");
    let state = &window.engine.state;
    let viewport = viewport_for(&state.controller.view(), state.controller.size());
    let scene = &window.engine.map.scene().aircraft.scene;
    assert!(scene.icons.is_empty() && scene.mesh_count() >= ARLANDA);
    // the model farthest from the others on the screen, by its origin's pixel
    let spots: Vec<(u32, [f64; 2])> = scene
        .groups
        .iter()
        .flat_map(|g| g.instances.iter())
        .map(|m| {
            let [x, y, _] = viewport.project(m.position);
            (m.id, [x, y])
        })
        .filter(|(_, [x, y])| (100.0..1000.0).contains(x) && (100.0..620.0).contains(y))
        .collect();
    let (id, at) = spots
        .iter()
        .map(|&(id, at)| {
            let clear = spots.iter().filter(|(other, _)| *other != id).map(|(_, p)| (p[0] - at[0]).hypot(p[1] - at[1])).fold(f64::INFINITY, f64::min);
            (id, at, clear)
        })
        .max_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(id, at, _)| (id, at.map(f64::round)))
        .expect("a model on the screen");

    window.move_to(at, NOW_MS);
    let (output, hovered) = window.frame(gpu, NOW_MS);
    save(&hovered, "a9-hover-v4.png");
    assert_eq!(window.engine.state.pick.hovered_id(), Some(id));
    assert_eq!(output.cursor_icon, egui::CursorIcon::PointingHand);
    // its pixels bluer, the rest of the picture as it was
    let (n, [x0, y0, x1, y1]) = changed(&before, &hovered);
    assert!(n > 20, "the model changed: {n} pixels");
    let bluer = (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y))).filter(|&(x, y)| {
        let (a, b) = (before.pixel(x, y), hovered.pixel(x, y));
        i32::from(b[2]) - i32::from(b[0]) > i32::from(a[2]) - i32::from(a[0]) + 20
    });
    assert!(bluer.count() * 2 >= n, "mostly bluer");
    let reach = 80.0;
    assert!(
        f64::from(x0) >= at[0] - reach && f64::from(x1) <= at[0] + reach && f64::from(y0) >= at[1] - reach && f64::from(y1) <= at[1] + reach,
        "only the model changed: {n} pixels in {x0},{y0}..{x1},{y1} round {at:?}"
    );

    let (pressed, _) = window.click(gpu, at, NOW_MS + 500.0);
    assert_eq!(pressed, Some(Target::Aircraft(id)));
    assert_eq!(window.engine.state.flight.selected_id(), Some(id));
}
