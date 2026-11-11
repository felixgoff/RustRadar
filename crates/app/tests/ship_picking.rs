//! Item H4's checks through the app as the window runs it (winit's events into
//! `app::Engine::window_event`, frames through `Engine::frame` onto an offscreen target) with ships
//! from a `--fixture` folder at V6 (Gothenburg harbour, zoom 13) on the satellite map:
//!
//! - the pointer over a ship's icon shows its tooltip at the pointer (its name, its kind and
//!   length, its MMSI) with the pointing hand; a click selects the ship, which then has the halo
//!   under it and the ring over it round its report, sized to clear its icon by 5 px; a click on
//!   empty map lets it go and both go;
//! - close in, the model under the pointer is highlighted `[55,142,240,110]` in the next frame and
//!   the tooltip names its ship; off the model the highlight goes.
//!
//! No reference picture has a selected ship (today's needs the backend). Needs a wgpu adapter
//! (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one the tests print `skipped: …`
//! and pass. Shots go to `target/shots/h4-*.png`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::NOW_MS;
use common::shots::{V6, gpu, save};
use rustradar_app::app::Engine;
use rustradar_app::state::pick::{Target, pick_at};
use rustradar_app::state::{AppState, Startup};
use rustradar_app::ui::theme;
use rustradar_app::ui::tooltip::Tip;
use rustradar_core::geo::Basemap;
use rustradar_core::model::Ship;
use rustradar_core::settings::Mode;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_map::io::DirTiles;
use rustradar_map::layers::ships::{HALO_COLOR, RING_COLOR};
use rustradar_map::{Gpu, SceneInput};
use rustradar_scene::camera::viewport_for;
use rustradar_scene::ships::MESH_HIGHLIGHT;
use winit::dpi::PhysicalPosition;
use winit::event::{DeviceId, ElementState, MouseButton, WindowEvent};

/// The reference pictures' canvas, at pixel ratio 1.
const SIZE: [u32; 2] = [1440, 900];

/// Ships on open water of the Göta älv at V6 (places from item H2's shot): `[x, y]` on the canvas,
/// type, heading, length (AIS bow + stern, the reference point amidships).
const SPOTS: [([f64; 2], u8, Option<f32>, u16); 4] = [
    // a 60 m cargo ship heading north-east: an icon at zoom 13
    ([700.0, 415.0], 4, Some(60.0), 60),
    // a moored pleasure boat: a dot
    ([540.0, 500.0], 8, None, 12),
    // a tug heading north-west
    ([900.0, 377.0], 2, Some(300.0), 28),
    // a 190 m tanker: a model from zoom 12.5
    ([740.0, 416.0], 6, Some(70.0), 190),
];

/// The ships of [`SPOTS`], placed on V6's canvas, all large (22 px icons).
fn ships() -> Vec<Ship> {
    let viewport = viewport_for(&V6.view(), SIZE.map(f64::from));
    SPOTS
        .iter()
        .enumerate()
        .map(|(i, &(xy, kind, heading, length))| {
            let [lon, lat] = viewport.unproject(xy, 0.0);
            let (half, side) = (length / 2, (length / 10).max(1));
            Ship {
                mmsi: 265_100_000 + i as u32,
                lat,
                lon,
                name: format!("VESSEL {i}"),
                kind,
                size: 2,
                heading,
                old: false,
                dims: Some([half, length - half, side, side]),
                frames: Vec::new(),
            }
        })
        .collect()
}

/// A `--fixture` folder with the ships and the committed tiles.
fn folder() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("h4-ships-v6");
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

/// The window's frames, drawn offscreen.
struct Window {
    engine: Engine,
    target: Offscreen,
    /// egui's clock, seconds.
    egui_s: f64,
}

impl Window {
    /// The app on the satellite map at V6 with the fixture's ships on the map.
    fn harbour(gpu: &Gpu) -> Window {
        let startup = Startup { basemap: Some(Basemap::Satellite), fixture: Some(folder()), demo: false };
        let mut state = AppState::new(startup, None, None, None, V6.view(), SIZE.map(f64::from));
        state.persisted.settings.mode = Mode::Ships;
        state.poll(NOW_MS);
        assert_eq!(state.ships.on_map().len(), SPOTS.len(), "the fixture's ships are on the map");
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

    fn scene(&self) -> &SceneInput {
        self.engine.map.scene()
    }

    /// What deck picks at `at` in the scene the map draws.
    fn pick(&self, at: [f64; 2]) -> Option<Target> {
        let state = &self.engine.state;
        pick_at(self.scene(), &state.controller.view(), state.controller.size(), at)
    }

    /// Where `[lon, lat]` is on the screen with the camera now.
    fn screen(&self, [lon, lat]: [f64; 2]) -> [f64; 2] {
        let state = &self.engine.state;
        let [x, y, _] = viewport_for(&state.controller.view(), state.controller.size()).project([lon, lat, 0.0]);
        [x, y]
    }
}

fn rgb(c: egui::Color32) -> [u8; 3] {
    [c.r(), c.g(), c.b()]
}

fn near(p: [u8; 4], c: [u8; 3], tolerance: u8) -> bool {
    (0..3).all(|i| p[i].abs_diff(c[i]) <= tolerance)
}

/// `top` at `alpha` over the pixel `under`.
fn over(top: [u8; 4], under: [u8; 4]) -> [u8; 3] {
    let a = f64::from(top[3]) / 255.0;
    [0, 1, 2].map(|c| (f64::from(top[c]) * a + f64::from(under[c]) * (1.0 - a)).round() as u8)
}

/// The pixels of `image` whose centres are `from` to `to` px from `centre`.
fn annulus(image: &Image, centre: [f64; 2], from: f64, to: f64) -> Vec<(u32, u32)> {
    let r = to.ceil() as i64 + 1;
    let (cx, cy) = (centre[0].floor() as i64, centre[1].floor() as i64);
    let mut out = Vec::new();
    for y in (cy - r).max(0)..(cy + r).min(i64::from(image.height)) {
        for x in (cx - r).max(0)..(cx + r).min(i64::from(image.width)) {
            let d = (x as f64 + 0.5 - centre[0]).hypot(y as f64 + 0.5 - centre[1]);
            if d >= from && d <= to {
                out.push((x as u32, y as u32));
            }
        }
    }
    out
}

/// The tooltip's box as drawn from its top left corner `at`: `[width, height]`.
fn tooltip_box(image: &Image, [x, y]: [u32; 2]) -> [u32; 2] {
    let line = rgb(theme::LINE);
    let mut w = 4;
    while x + w < image.width && near(image.pixel(x + w, y), line, 2) {
        w += 1;
    }
    let mut h = 4;
    while y + h < image.height && near(image.pixel(x, y + h), line, 2) {
        h += 1;
    }
    let raised = rgb(theme::SURFACE_RAISED);
    assert!(near(image.pixel(x + 3, y + h - 3), raised, 1), "--surface-raised inside: {:?}", image.pixel(x + 3, y + h - 3));
    [w + 4, h + 4]
}

#[test]
fn the_pointer_over_a_ship_says_what_it_is_and_a_click_rings_it() {
    let Some(shared) = gpu() else { return };
    let gpu = &*shared;
    let mut window = Window::harbour(gpu);
    let before = window.settle(gpu, NOW_MS);
    save(&before, "h4-v6-before.png");
    let ship = window.engine.state.ships.on_map()[0].clone();
    let at = window.screen([ship.lon, ship.lat]).map(f64::round);
    let Some(Target::Ship(pick)) = window.pick(at) else { panic!("a ship under {at:?}: {:?}", window.pick(at)) };
    assert_eq!((pick.ship, pick.mmsi, pick.model), (0, ship.mmsi, None), "its icon");

    // the tooltip at the pointer, the pointing hand
    window.move_to(at, NOW_MS + 100.0);
    let (output, shot) = window.frame(gpu, NOW_MS + 100.0);
    save(&shot, "h4-ship-tooltip.png");
    assert_eq!(output.cursor_icon, egui::CursorIcon::PointingHand, "a pointing hand over the ship");
    let shown = window.engine.state.pick.tooltip().cloned().expect("a tooltip");
    assert_eq!(shown.at, at, "at the pointer");
    assert_eq!(shown.tip, Tip::ship(&ship));
    assert_eq!(shown.tip.text(), format!("VESSEL 0\nCargo · 60 m\nMMSI {}", ship.mmsi));
    let [w, h] = tooltip_box(&shot, at.map(|v| v as u32));
    let tall = 1.0 + 8.0 + 20.3 + 2.0 * 17.4 + 8.0 + 1.0;
    assert!((h as f32 - tall).abs() <= 1.0 && w > 80 && w < 300, "{w} x {h}, CSS {tall} tall");

    // a click selects it; with the pointer gone off the ships the tooltip goes, the halo and the
    // ring stay round its report
    window.click(gpu, at, NOW_MS + 200.0);
    assert_eq!(window.engine.state.ships.selected_mmsi(), Some(ship.mmsi));
    let empty = [at[0] - 260.0, at[1] - 300.0];
    assert_eq!(window.pick(empty), None);
    window.move_to(empty, NOW_MS + 700.0);
    let (_, ringed) = window.frame(gpu, NOW_MS + 700.0);
    save(&ringed, "h4-ship-selected.png");
    assert!(window.engine.state.pick.tooltip().is_none());
    let ring = window.scene().ships.ring.expect("the ring");
    assert_eq!(ring.at, [ship.lon, ship.lat]);
    // the 22 px icon clears the hull (30 m from its reference point is 3 px at zoom 13)
    assert_eq!(ring.radius_px, 22.0 / 2.0 + 5.0);
    let centre = window.screen(ring.at);
    // the ring's middle in its colour over the water
    let middle = annulus(&ringed, centre, ring.radius_px - 0.4, ring.radius_px + 0.4);
    let blue = middle.iter().filter(|&&(x, y)| near(ringed.pixel(x, y), over(RING_COLOR, before.pixel(x, y)), 12)).count();
    assert!(blue * 10 >= middle.len() * 9, "{blue} of {} round the ring in its colour", middle.len());
    // the halo between the icon and the ring, over the water
    let inside = annulus(&ringed, centre, 12.0, 14.0);
    let tinted = inside.iter().filter(|&&(x, y)| near(ringed.pixel(x, y), over(HALO_COLOR, before.pixel(x, y)), 4)).count();
    assert!(tinted * 10 >= inside.len() * 9, "{tinted} of {} inside the ring tinted", inside.len());
    // nothing else of the map changed
    let changed: Vec<(u32, u32)> = (0..ringed.height)
        .flat_map(|y| (0..ringed.width).map(move |x| (x, y)))
        .filter(|&(x, y)| ringed.pixel(x, y) != before.pixel(x, y))
        .filter(|&(x, y)| (f64::from(x) + 0.5 - centre[0]).hypot(f64::from(y) + 0.5 - centre[1]) > ring.radius_px + 2.0)
        .collect();
    assert!(changed.is_empty(), "{} pixels changed off the ring, the first at {:?}", changed.len(), changed.first());

    // a click on empty map lets it go: no halo, no ring
    window.click(gpu, empty, NOW_MS + 1000.0);
    assert_eq!(window.engine.state.ships.selected_mmsi(), None, "a click on empty map lets it go");
    let (_, after) = window.frame(gpu, NOW_MS + 1500.0);
    assert!(window.scene().ships.ring.is_none());
    let left = middle.iter().filter(|&&(x, y)| after.pixel(x, y) != before.pixel(x, y)).count();
    assert_eq!(left, 0, "the ring's pixels as before");
}

#[test]
fn close_in_the_model_under_the_pointer_is_highlighted_and_named() {
    let Some(shared) = gpu() else { return };
    let gpu = &*shared;
    let mut window = Window::harbour(gpu);
    let before = window.settle(gpu, NOW_MS);
    let tanker = window.engine.state.ships.on_map()[3].clone();
    let models = &window.scene().ships.scene.models;
    assert_eq!(models.len(), 1, "the tanker is a model at zoom 13");
    // its hull's middle: the reference point, amidships
    let at = window.screen([tanker.lon, tanker.lat]).map(f64::round);
    let Some(Target::Ship(pick)) = window.pick(at) else { panic!("a ship under {at:?}: {:?}", window.pick(at)) };
    let model = pick.model.expect("its model");
    assert_eq!((pick.ship, pick.mmsi, model.index), (3, tanker.mmsi, 0));

    window.move_to(at, NOW_MS + 100.0);
    let (_, lit) = window.frame(gpu, NOW_MS + 100.0);
    save(&lit, "h4-model-highlight.png");
    assert_eq!(window.engine.state.pick.hovered_ship_model(), Some(&model));
    let highlighted: Vec<usize> = window.scene().ships.scene.models.iter().flat_map(|g| &g.models).filter(|m| m.highlighted).map(|m| m.index).collect();
    assert_eq!(highlighted, [3], "the tanker");
    assert_eq!(window.engine.state.pick.tooltip().map(|s| s.tip.clone()), Some(Tip::ship(&tanker)));
    // its deck turned towards the selection blue, beside the tooltip's corner (the hull runs
    // east-north-east from there)
    let probe = [at[0] as u32 + 12, at[1] as u32 - 3];
    let (was, now) = (before.pixel(probe[0], probe[1]), lit.pixel(probe[0], probe[1]));
    assert!(now[2] > was[2] + 15 && now[0] + 10 < was[0], "towards {MESH_HIGHLIGHT:?}: {was:?} → {now:?}");

    // off it: the highlight goes at once
    let off = [at[0] - 300.0, at[1] - 250.0];
    assert_eq!(window.pick(off), None);
    window.move_to(off, NOW_MS + 200.0);
    let (_, unlit) = window.frame(gpu, NOW_MS + 200.0);
    assert_eq!(window.engine.state.pick.hovered_ship_model(), None);
    assert!(window.scene().ships.scene.models.iter().flat_map(|g| &g.models).all(|m| !m.highlighted));
    assert_eq!(unlit.pixel(probe[0], probe[1]), was, "drawn as before");
}
