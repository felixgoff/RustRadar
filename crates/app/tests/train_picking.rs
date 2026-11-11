//! Item T8's checks through the app as the window runs it (winit's events into
//! `app::Engine::window_event`, frames through `Engine::frame` onto an offscreen target) with the
//! demo trains (`--demo`), and through the state's pick step with a hand-made block set:
//!
//! - the pointer over a train's icon shows its tooltip with its top left corner at the pointer:
//!   `--surface-raised` inside a 1 px `--line` border, the train's name, kind and operator, where
//!   to and how late, and the position's age; the cursor is the pointing hand; a drag (the camera
//!   moving) hides it until the pointer moves again; a click selects the train, one on empty map
//!   lets it go;
//! - close in, the car under the pointer is highlighted `[55,142,240,110]` in the next frame and
//!   the tooltip names its train; off the car the highlight goes;
//! - an airport's code as the labels layer draws it: its name and `Open departures and arrivals`;
//! - a signal: its name and aspect, who is in the block it leads into, and the note; a click on it
//!   does nothing.
//!
//! Needs a wgpu adapter for the first three (SwiftShader or lavapipe through `VK_ICD_FILENAMES`);
//! without one they print `skipped: …` and pass. Shots go to `target/shots/t8-*.png`.

mod common;

use std::sync::Arc;

use common::NOW_MS;
use common::shots::{gpu, save};
use rustradar_app::app::Engine;
use rustradar_app::input::Click;
use rustradar_app::state::pick::{Target, pick_at};
use rustradar_app::state::{AppState, Startup};
use rustradar_app::ui::theme;
use rustradar_app::ui::tooltip::{AIRPORT_NOTE, SIGNAL_NOTE, Tip};
use rustradar_core::geo::Basemap;
use rustradar_core::model::TrainBlock;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_map::io::DirTiles;
use rustradar_map::{Gpu, SceneInput};
use rustradar_scene::blocks::{BlockBuilder, DrawnTrain, SetRef};
use rustradar_scene::camera::{ViewState, viewport_for};
use rustradar_scene::trains::CAR_HIGHLIGHT;
use winit::dpi::PhysicalPosition;
use winit::event::{DeviceId, ElementState, MouseButton, WindowEvent};

/// The shots' canvas, at pixel ratio 1.
const SIZE: [u32; 2] = [1100, 720];
/// East of Stockholm at zoom 12, the last zoom of the trains' icons: the demo trains (made round
/// the view's centre) 4 to 14 px apart, away from the city's name.
const ICONS: ViewState = ViewState { latitude: 59.36, longitude: 18.22, zoom: 12.0, pitch: 0.0, bearing: 0.0, position: None };
/// The same place close in: the trains as their cars.
const CARS: ViewState = ViewState { zoom: 16.5, ..ICONS };
/// Stockholm at zoom 9 (plan section 8's V3), for the airports' codes.
const V3: ViewState = ViewState { latitude: 59.33, longitude: 18.07, zoom: 9.0, pitch: 0.0, bearing: 0.0, position: None };

/// The window's frames, drawn offscreen.
struct Window {
    engine: Engine,
    target: Offscreen,
    /// egui's clock, seconds.
    egui_s: f64,
}

impl Window {
    /// The app on the dark map at `view`, with the demo trains and nothing else.
    fn demo(gpu: &Gpu, view: ViewState) -> Window {
        let startup = Startup { basemap: Some(Basemap::Dark), fixture: None, demo: true };
        let mut state = AppState::new(startup, None, None, None, view, SIZE.map(f64::from));
        state.poll(NOW_MS);
        assert_eq!(state.trains.on_map().len(), 6, "the demo trains are on the map");
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

    fn button(&mut self, state: ElementState, now: f64) {
        self.event(WindowEvent::MouseInput { device_id: DeviceId::dummy(), state, button: MouseButton::Left }, now);
    }

    /// A left click at `at` at `now`: the click fires 300 ms after the release, in the frame then.
    fn click(&mut self, gpu: &Gpu, at: [f64; 2], now: f64) -> Image {
        self.move_to(at, now);
        self.button(ElementState::Pressed, now);
        self.button(ElementState::Released, now + 80.0);
        self.frame(gpu, now + 400.0).1
    }

    /// What deck picks at `at` in the scene the map draws.
    fn pick(&self, at: [f64; 2]) -> Option<Target> {
        let state = &self.engine.state;
        pick_at(self.engine.map.scene(), &state.controller.view(), state.controller.size(), at)
    }

    /// Where `[lon, lat, metres]` is on the screen with the camera now.
    fn screen(&self, position: [f64; 3]) -> [f64; 2] {
        let state = &self.engine.state;
        let [x, y, _] = viewport_for(&state.controller.view(), state.controller.size()).project(position);
        [x, y]
    }
}

fn rgb(c: egui::Color32) -> [u8; 3] {
    [c.r(), c.g(), c.b()]
}

fn near(p: [u8; 4], c: [u8; 3], tolerance: u8) -> bool {
    (0..3).all(|i| p[i].abs_diff(c[i]) <= tolerance)
}

/// The tooltip's box as drawn: the run of `--surface-raised` inside a `--line` border from its top
/// left corner `at` (the pointer, rounded), right and down. Returns `[width, height]`.
fn tooltip_box(image: &Image, [x, y]: [u32; 2]) -> [u32; 2] {
    let raised = rgb(theme::SURFACE_RAISED);
    let line = rgb(theme::LINE);
    // the top border runs right from past the rounded corner, the left border down
    let mut w = 4;
    while x + w < image.width && near(image.pixel(x + w, y), line, 2) {
        w += 1;
    }
    let mut h = 4;
    while y + h < image.height && near(image.pixel(x, y + h), line, 2) {
        h += 1;
    }
    assert!(near(image.pixel(x + 3, y + h - 3), raised, 1), "--surface-raised inside, by the bottom left: {:?}", image.pixel(x + 3, y + h - 3));
    assert!(near(image.pixel(x + w - 2, y + 3), raised, 1), "--surface-raised inside, by the top right: {:?}", image.pixel(x + w - 2, y + 3));
    [w + 4, h + 4]
}

/// The pixels of `image` in `[x0, y0, x1, y1)` close to `colour`.
fn count(image: &Image, [x0, y0, x1, y1]: [u32; 4], colour: [u8; 3], tolerance: u8) -> usize {
    (y0..y1.min(image.height)).flat_map(|y| (x0..x1.min(image.width)).map(move |x| (x, y))).filter(|&(x, y)| near(image.pixel(x, y), colour, tolerance)).count()
}

#[test]
fn the_pointer_over_a_train_shows_its_tooltip_a_drag_hides_it_and_a_click_selects_it() {
    let Some(shared) = gpu() else { return };
    let gpu = &*shared;
    let mut window = Window::demo(gpu, ICONS);
    window.settle(gpu, NOW_MS);
    // the picture without the pointer at the time of the hover (two of the trains move)
    let (_, before) = window.frame(gpu, NOW_MS + 100.0);
    save(&before, "t8-icons-before.png");
    let scene = window.engine.map.scene();
    assert!(!scene.trains.scene.cars && scene.trains.scene.icons.len() == 6, "icons at zoom 12");
    // a train whose icon's anchor picks it (the later drawn of two that overlap wins)
    let anchors: Vec<[f64; 2]> = scene.trains.scene.icons.iter().map(|i| window.screen(i.position).map(f64::round)).collect();
    let (k, at) = anchors
        .iter()
        .enumerate()
        .rev()
        .find_map(|(k, &at)| matches!(window.pick(at), Some(Target::Train(ref p)) if p.train == k).then_some((k, at)))
        .expect("a train picked over its icon");
    let train = window.engine.state.trains.on_map().get(k).cloned().expect("its row");
    eprintln!("{} (train {k}) at {at:?}", train.name);

    window.move_to(at, NOW_MS + 100.0);
    let (output, shot) = window.frame(gpu, NOW_MS + 100.0);
    save(&shot, "t8-train-tooltip.png");
    assert_eq!(output.cursor_icon, egui::CursorIcon::PointingHand, "a pointing hand over the train");
    let shown = window.engine.state.pick.tooltip().cloned().expect("a tooltip");
    assert_eq!(shown.at, at, "at the pointer");
    let expected = Tip::train(&train, (NOW_MS + 100.0) as i64);
    assert_eq!(shown.tip, expected);
    // a demo train: its kind and operator, on time, reported as it was made
    let kind = rustradar_core::trains::train_label(train.kind);
    assert_eq!(expected.text(), format!("{}\n{kind} · {}\non time\nPosition 0 s ago", train.name, train.operator.as_deref().unwrap_or_default()));
    eprintln!("tooltip:\n{}", shown.tip.text());
    // the box from the pointer, its rows inside: the head in --text-1, the rows below in --text-2
    let [x, y] = at.map(|v| v as u32);
    let [w, h] = tooltip_box(&shot, [x, y]);
    let rows = shown.tip.text().lines().count() as f32;
    let tall = 1.0 + 8.0 + 20.3 + (rows - 1.0) * 17.4 + 8.0 + 1.0;
    assert!((h as f32 - tall).abs() <= 1.0, "{h} px tall, CSS {tall}");
    assert!(w > 100 && w < 400, "{w} px wide");
    let head = count(&shot, [x + 13, y + 9, x + w - 13, y + 29], rgb(theme::TEXT_1), 24);
    let subs = count(&shot, [x + 13, y + 30, x + w - 13, y + h - 9], rgb(theme::TEXT_2), 24);
    assert!(head > 40 && subs > 60, "head ink {head}, sub ink {subs}");
    assert_eq!(count(&shot, [x + 13, y + 30, x + w - 13, y + h - 9], rgb(theme::TEXT_1), 6), 0, "no --text-1 in the sub rows");
    // nothing of the map changed but the box (the icon highlight is the sidebar's, not the pointer's)
    let outside = (0..shot.height).flat_map(|py| (0..shot.width).map(move |px| (px, py))).filter(|&(px, py)| !(px >= x && px < x + w && py >= y && py < y + h)).filter(|&(px, py)| shot.pixel(px, py) != before.pixel(px, py)).count();
    assert_eq!(outside, 0, "the map is as it was");

    // a drag moves the camera: the tooltip goes, and stays gone once the drag ends where it is
    let view = window.engine.state.controller.view();
    window.button(ElementState::Pressed, NOW_MS + 200.0);
    for i in 1..=5 {
        window.move_to([at[0] + 12.0 * f64::from(i), at[1] + 6.0 * f64::from(i)], NOW_MS + 200.0 + 16.0 * f64::from(i));
    }
    let (_, dragged) = window.frame(gpu, NOW_MS + 290.0);
    assert_ne!(window.engine.state.controller.view(), view, "the camera moved");
    assert!(window.engine.state.pick.tooltip().is_none(), "hidden while the camera moves");
    let boxed = count(&shot, [x, y, x + w, y + h], rgb(theme::SURFACE_RAISED), 0);
    let left = count(&dragged, [x, y, x + w, y + h], rgb(theme::SURFACE_RAISED), 0);
    assert!(boxed as u32 > w * h / 2 && left < boxed / 10, "no box drawn: {left} of its {boxed} --surface-raised pixels left");
    window.button(ElementState::Released, NOW_MS + 300.0);
    window.frame(gpu, NOW_MS + 350.0);
    window.frame(gpu, NOW_MS + 700.0);
    assert!(window.engine.state.pick.tooltip().is_none(), "back only when the pointer moves");
    // the train is under the pointer again: moving there shows the tooltip
    let moved = window.screen(window.engine.map.scene().trains.scene.icons[k].position).map(f64::round);
    window.move_to(moved, NOW_MS + 800.0);
    window.frame(gpu, NOW_MS + 800.0);
    assert_eq!(window.engine.state.pick.tooltip().map(|s| s.at), Some(moved));

    // a click selects it; one on empty map lets it go
    window.click(gpu, moved, NOW_MS + 1000.0);
    assert_eq!(window.engine.state.trains.selected_id(), Some(train.id.as_str()));
    let empty = [moved[0] + 200.0, moved[1] + 150.0];
    assert_eq!(window.pick(empty), None);
    window.click(gpu, empty, NOW_MS + 2000.0);
    assert_eq!(window.engine.state.trains.selected_id(), None, "a click on empty map lets it go");
    assert!(window.engine.state.pick.tooltip().is_none(), "nothing there to say");
}

#[test]
fn close_in_the_car_under_the_pointer_is_highlighted_and_named() {
    let Some(shared) = gpu() else { return };
    let gpu = &*shared;
    let mut window = Window::demo(gpu, CARS);
    let before = window.settle(gpu, NOW_MS);
    let scene = window.engine.map.scene();
    assert!(scene.trains.scene.cars, "cars close in");
    // a standing train's car in the middle of its train (they do not move between frames)
    let layers = &scene.trains.scene.car_layers;
    let ids = scene.trains.scene.ids.clone();
    let car = layers.cars.iter().find(|c| ids[c.train].ends_with(":2") && !c.level.tunnel).cloned().expect("a car of the third demo train");
    let train = window.engine.state.trains.on_map().get(car.train).cloned().expect("its train");
    assert_eq!(train.id, ids[car.train]);
    // its middle, on its roof
    let at = window.screen([car.at[0], car.at[1], layers.height]).map(f64::round);
    let Some(Target::Train(pick)) = window.pick(at) else { panic!("a train under {at:?}: {:?}", window.pick(at)) };
    assert_eq!((pick.train, pick.id.as_str()), (car.train, train.id.as_str()));
    let picked = pick.car.clone().expect("a car");
    eprintln!("{} car {picked:?} at {at:?}", train.name);

    window.move_to(at, NOW_MS + 100.0);
    let (_, lit) = window.frame(gpu, NOW_MS + 100.0);
    save(&lit, "t8-car-highlight.png");
    assert_eq!(window.engine.state.pick.hovered_car(), Some(&picked));
    let highlighted: Vec<usize> = window.engine.map.scene().trains.scene.car_layers.cars.iter().enumerate().filter(|(_, c)| c.highlighted).map(|(i, _)| i).collect();
    assert_eq!(highlighted.len(), 1, "one car");
    assert_eq!(window.engine.state.pick.tooltip().map(|s| s.tip.clone()), Some(Tip::train(&train, (NOW_MS + 100.0) as i64)));
    // the car's roof turned towards [55,142,240] at 110/255 next to the tooltip's corner, the
    // other trains' cars as they were
    let probe = [at[0] as u32 - 3, at[1] as u32];
    let (was, now) = (before.pixel(probe[0], probe[1]), lit.pixel(probe[0], probe[1]));
    // (the blend itself, lit, is the car layer's: `layers::train_cars`' own shot checks it)
    assert!(now[0] + 15 < was[0] && now[2] > was[2] + 15, "towards {CAR_HIGHLIGHT:?}: {was:?} → {now:?}");

    // off every car: the highlight goes at once
    let off = [at[0] + 300.0, at[1] + 250.0];
    assert_eq!(window.pick(off), None);
    window.move_to(off, NOW_MS + 200.0);
    let (_, unlit) = window.frame(gpu, NOW_MS + 200.0);
    assert_eq!(window.engine.state.pick.hovered_car(), None);
    assert!(window.engine.map.scene().trains.scene.car_layers.cars.iter().all(|c| !c.highlighted));
    assert_eq!(unlit.pixel(probe[0], probe[1]), was, "drawn as before");
}

#[test]
fn an_airports_code_says_its_name() {
    let Some(shared) = gpu() else { return };
    let gpu = &*shared;
    let mut window = Window::demo(gpu, V3);
    window.settle(gpu, NOW_MS);
    let drawn = window.engine.map.scene().labels.drawn.labels();
    let airport = drawn
        .iter()
        .find(|l| l.fade > 0.5 && matches!(window.pick(l.anchor.map(f64::round)), Some(Target::Label(Some(ref c))) if Some(c) == l.iata.as_ref()))
        .cloned()
        .expect("an airport's code picked at its anchor");
    let at = airport.anchor.map(f64::round);
    eprintln!("{:?} ({}) at {at:?}", airport.iata, airport.name);
    assert!(!airport.name.is_empty(), "the feature's name");
    window.move_to(at, NOW_MS + 100.0);
    let (_, shot) = window.frame(gpu, NOW_MS + 100.0);
    save(&shot, "t8-airport-tooltip.png");
    let shown = window.engine.state.pick.tooltip().cloned().expect("a tooltip");
    assert_eq!(shown.tip.text(), format!("{}\n{AIRPORT_NOTE}", airport.name));
    let [w, h] = tooltip_box(&shot, at.map(|v| v as u32));
    assert!((h as f32 - (20.3 + 17.4 + 18.0)).abs() <= 1.0 && w > 100, "{w} × {h}");
    // a place's name has none
    let place = drawn.iter().find(|l| l.iata.is_none() && l.fade > 0.5 && window.pick(l.anchor.map(f64::round)) == Some(Target::Label(None))).cloned().expect("a place's name");
    window.move_to(place.anchor.map(f64::round), NOW_MS + 200.0);
    window.frame(gpu, NOW_MS + 200.0);
    assert!(window.engine.state.pick.tooltip().is_none(), "{} has no tooltip", place.text);
}

/// A signal's tooltip through the state's pick step (no GPU): two blocks one after the other on a
/// straight track, a train in the second, so the first one's signal shows caution.
#[test]
fn a_signal_says_its_aspect_and_who_is_ahead_and_a_click_does_nothing() {
    let view = ViewState { latitude: 59.6730, longitude: 17.9118, zoom: 15.5, pitch: 0.0, bearing: 0.0, position: None };
    let size = SIZE.map(f64::from);
    let (lat, lon) = (view.latitude, view.longitude);
    let step = 0.002;
    let blocks = vec![
        TrainBlock { id: "a".into(), signal: Some("27".into()), lines: vec![vec![[lon - 2.0 * step, lat], [lon - step, lat]]] },
        TrainBlock { id: "b".into(), signal: None, lines: vec![vec![[lon - step, lat], [lon, lat]]] },
    ];
    let trains = [DrawnTrain { id: "t", name: "Pendeltåg 2245", at: [lon - step / 2.0, lat] }];
    let mut scene = SceneInput::new(view, size, 1.0, Basemap::Dark);
    scene.blocks.scene = BlockBuilder::new().build(Some(SetRef { generation: 1, list: &blocks, structures: &[] }), &trains, &view, size, NOW_MS);
    let startup = Startup { basemap: Some(Basemap::Dark), fixture: None, demo: false };
    let mut state = AppState::new(startup, None, None, None, view, size);
    state.poll(NOW_MS);

    let [x, y, _] = viewport_for(&view, size).project([lon - 2.0 * step, lat, 0.0]);
    let signal = [x.round(), y.round()];
    assert_eq!(pick_at(&scene, &view, size, signal), Some(Target::Signal(0)));
    state.pick_frame(&scene, Some(signal), false, &[], NOW_MS);
    let shown = state.pick.tooltip().cloned().expect("a tooltip");
    assert_eq!(shown.tip.text(), format!("Signal 27 · Caution\nPendeltåg 2245 in the next block\n{SIGNAL_NOTE}"));
    assert!(state.pick.hovering(), "the pointing hand");
    // the second block's signal (unnamed) stands at its start, behind the train: stop
    let [x, y, _] = viewport_for(&view, size).project([lon - step, lat, 0.0]);
    state.pick_frame(&scene, Some([x.round(), y.round()]), false, &[], NOW_MS);
    assert_eq!(state.pick.tooltip().map(|s| s.tip.text()), Some(format!("Main signal · Stop\nPendeltåg 2245 in the block after it\n{SIGNAL_NOTE}")));
    // a click on it changes nothing
    let click = Click { at: signal, tap_count: 1, modifiers: Default::default() };
    state.pick_press(&scene, signal);
    let wants = state.pick_frame(&scene, Some(signal), false, &[click], NOW_MS);
    assert!(!wants.scene);
    assert_eq!((state.flight.selected_id(), state.trains.selected_id()), (None, None));
    // off the track: no tooltip
    state.pick_frame(&scene, Some([signal[0], signal[1] + 100.0]), false, &[], NOW_MS);
    assert!(state.pick.tooltip().is_none());
}
