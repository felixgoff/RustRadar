//! Item B9's acceptance through the app's own scenes (doc 01 §9.5, plan 2.2):
//!
//! - the map error card drawn as the window draws a frame (the map's passes, then egui's over
//!   them): a real wgpu error stops the map at its last picture, the card goes over its middle,
//!   "Copy details" copies the details of this device, "Retry" clears the error and the map draws
//!   again with no card;
//! - the window's side (`app::Engine`): the card from the engine's own UI pass, and a lost device
//!   rebuilt at once through `Engine::rebuild`, the same frame drawn on the new device;
//! - the device lost under the app's whole scene (the scene builders' scene with the synthetic
//!   snapshot: the vector map with true-width runways, buildings with their shadows, aircraft
//!   models with gear and shadows, icons, labels; and the satellite map's imagery), the renderer
//!   rebuilt on a new device from what it keeps, and the same picture drawn again, pixel for pixel.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. Shots go to `target/shots/b9-*.png`.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::NOW_MS;
use common::fixtures::{self, ARLANDA, STOCKHOLM, WORLD};
use common::shots::{NamedView, Rect, V3, V4, bounds, close, components, count, gpu, pixel, save};
use egui::{Event, PointerButton, Pos2, RawInput, Vec2};
use rustradar_app::app::Engine;
use rustradar_app::scene_builder::SceneBuilder;
use rustradar_app::state::{AppState, Startup};
use rustradar_app::theme;
use rustradar_app::ui::map_error;
use rustradar_core::geo::Basemap;
use rustradar_map::headless::{FORMAT, Image, Offscreen, render_with};
use rustradar_map::io::{DirTiles, TileFetcher};
use rustradar_map::renderer::{DEVICE_LOST_MESSAGE, Retry};
use rustradar_map::{Gpu, MapRenderer, SceneInput};

/// The canvas, CSS px, at scale 1.
const SIZE: [u32; 2] = [1100, 720];

/// A device of the test's own, which it may break or destroy, opened while the shared one is held
/// so that no other test of this binary uses SwiftShader meanwhile.
fn own_device(_shared: &Gpu) -> Gpu {
    Gpu::headless().expect("a second device on the adapter the shared one is on")
}

/// A `--fixture` folder holding the synthetic snapshot as `live_flights.json` and no tiles (the
/// map's come from the committed fixtures).
fn snapshot_folder() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("b9-synthetic");
    fixtures::write_fixture(&dir, &fixtures::synthetic_snapshot(NOW_MS)).expect("the fixture folder");
    dir
}

/// The scene the app makes of the synthetic snapshot at `view` on `basemap`, as `--screenshot`
/// does, with the aircraft labels on, and the place labels (which the scene builders do not turn
/// on yet).
fn app_scene(view: NamedView, basemap: Basemap) -> SceneInput {
    let startup = Startup { basemap: Some(basemap), fixture: Some(snapshot_folder()), demo: false };
    let mut state = AppState::new(startup, None, None, None, view.view(), SIZE.map(f64::from));
    state.persisted.settings.aircraft_labels = true;
    state.poll(NOW_MS);
    assert_eq!(state.aircraft.flights().len(), WORLD + STOCKHOLM + ARLANDA, "the snapshot is in");
    let clocks = state.clocks(NOW_MS);
    let mut scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
    scene.labels.shown = true;
    scene
}

/// A renderer on `gpu` with the committed tiles, catching its errors (as the window's does).
fn renderer(gpu: &Gpu, tiles: &Arc<DirTiles>, wake: Option<rustradar_map::jobs::Wake>) -> MapRenderer {
    let mut renderer = MapRenderer::new(gpu, FORMAT);
    renderer.set_tiles(tiles.clone(), None);
    renderer.catch_errors(gpu, wake);
    renderer
}

fn differing_pixels(a: &Image, b: &Image) -> usize {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.rgba.chunks_exact(4).zip(b.rgba.chunks_exact(4)).filter(|(a, b)| a != b).count()
}

/// The device lost under the app's whole scene at `view` on `basemap`, the renderer rebuilt on a
/// new device: the same picture before and after.
fn rebuilt_draws_the_same(shared: &Gpu, view: NamedView, basemap: Basemap, name: &str) -> (SceneInput, Image) {
    let scene = app_scene(view, basemap);
    let tiles = Arc::new(DirTiles::fixtures());
    let first = own_device(shared);
    let mut map = renderer(&first, &tiles, None);
    let before = render_with(&first, &mut map, &scene, 1.0).expect("rendered");
    assert!(!map.pending(), "still loading: {:?}", map.pending_layers());
    assert!(tiles.misses().is_empty(), "the fixtures lack {:?}", tiles.misses());
    assert_eq!(map.error(), None, "{name}: drawn without an error");
    save(&before, &format!("b9-{name}-before-the-loss.png"));

    first.device.destroy();
    let _ = first.device.poll(wgpu::PollType::wait_indefinitely());
    let lost = map.error().expect("the loss is caught");
    assert!(lost.lost && lost.message == DEVICE_LOST_MESSAGE, "{lost:?}");
    assert_eq!(map.retry(), Retry::Rebuild);

    let second = own_device(shared);
    map.rebuild(&second, FORMAT);
    assert!(Arc::ptr_eq(&map.tiles().fetcher(), &(tiles.clone() as Arc<dyn TileFetcher>)), "the fetcher is kept");
    let after = render_with(&second, &mut map, &scene, 1.0).expect("rendered after the rebuild");
    assert!(!map.pending(), "still loading after the rebuild: {:?}", map.pending_layers());
    assert_eq!(map.error(), None, "{name}: nothing went wrong on the new device");
    save(&after, &format!("b9-{name}-after-the-rebuild.png"));
    let differing = differing_pixels(&before, &after);
    assert_eq!(differing, 0, "{name}: the rebuilt map draws the same shot ({differing} pixels differ)");
    (scene, after)
}

/// V4 on the dark map, pitched over Arlanda at noon: the vector map with runways at their true
/// width, buildings with their shadows, the six airliners on the ground as lit models with gear
/// and shadows; and V3 on satellite: Esri's imagery under the aircraft icons. Each the same
/// after the device is lost and the renderer rebuilt.
#[test]
fn the_apps_scene_is_drawn_the_same_after_a_rebuild() {
    let Some(shared) = gpu() else { return };

    let (v4, image) = rebuilt_draws_the_same(&shared, V4, Basemap::Dark, "dark-v4");
    assert!(v4.vector_map.shown && v4.buildings.shown && v4.labels.shown, "the dark map's layers are on");
    let aircraft = &v4.aircraft.scene;
    assert!(aircraft.mesh_count() >= ARLANDA && aircraft.gear_count() > 0, "models with gear: {} {}", aircraft.mesh_count(), aircraft.gear_count());
    // the runways' asphalt and the buildings' lit roofs and walls are in the picture
    let colours: std::collections::HashSet<[u8; 4]> = image.rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect();
    assert!(colours.len() > 200, "a picture of the map, not a flat one: {} colours", colours.len());

    let (v3, image) = rebuilt_draws_the_same(&shared, V3, Basemap::Satellite, "satellite-v3");
    assert!(!v3.aircraft.scene.icons.is_empty(), "icons over the imagery");
    // Esri's sea and land, tinted, not the bare earth sphere's blue
    let middle = Rect::new(300, 150, 500, 400);
    let earth = count(&image, middle, |p| close(p, common::shots::SATELLITE_EARTH, 3));
    assert!(earth < middle.len() / 10, "the imagery covers the earth: {earth} earth pixels");
}

/// A window's frames without a window: an egui context with the app's theme, egui's renderer and
/// a target of the canvas's size.
struct Window {
    ctx: egui::Context,
    egui: egui_wgpu::Renderer,
    target: Offscreen,
}

impl Window {
    fn new(gpu: &Gpu) -> Self {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let egui = egui_wgpu::Renderer::new(&gpu.device, FORMAT, egui_wgpu::RendererOptions::default());
        Window { ctx, egui, target: Offscreen::new(gpu, SIZE) }
    }

    /// A frame as the window draws one (`app::Engine::render`): the map's passes onto the target,
    /// then egui's over them (`LoadOp::Load`), with the card of [`map_error::ui`] as the whole UI;
    /// egui's clock at `egui_s`, its `events`. Returns the card's answer and egui's output.
    fn frame(&mut self, gpu: &Gpu, map: &mut MapRenderer, local_ms: f64, egui_s: f64, events: Vec<Event>) -> (Option<Retry>, egui::PlatformOutput) {
        let raw = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(SIZE[0] as f32, SIZE[1] as f32))),
            time: Some(egui_s),
            events,
            ..Default::default()
        };
        let info = gpu.adapter.get_info();
        let mut answer = None;
        let output = self.ctx.run_ui(raw, |ui| answer = map_error::ui(ui.ctx(), map, Some(&info)));
        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: self.target.size(), pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui.update_texture(device, queue, id, delta);
            }
        }
        map.prepare(gpu, local_ms);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let mut commands = self.egui.update_buffers(device, queue, &mut encoder, &primitives, &screen);
        map.draw(gpu, &mut encoder, self.target.view(), self.target.size());
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: self.target.view(),
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
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
        (answer, output.platform_output)
    }

    fn read(&self, gpu: &Gpu) -> Image {
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        self.target.read(gpu).expect("read back")
    }
}

fn click(at: [f64; 2]) -> Vec<Event> {
    let at = Pos2::new(at[0] as f32, at[1] as f32);
    vec![
        Event::PointerMoved(at),
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() },
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() },
    ]
}

fn rgba(c: egui::Color32) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

/// Doc 01 §9.5 on the GPU: a validation error under the dark map at V3 stops it at its last
/// picture; the card (`--surface`, a `--line` border, 420 + 2 × 17 px wide, centred) says "The map
/// couldn't draw" and the message over two `.button`s; "Copy details" copies this device's
/// details; "Retry" clears the error and the next frame is the map again, with no card.
#[test]
fn the_card_goes_over_a_stopped_map_until_retry() {
    let Some(shared) = gpu() else { return };
    let gpu = own_device(&shared);
    let scene = app_scene(V3, Basemap::Dark);
    let tiles = Arc::new(DirTiles::fixtures());
    let wakes = Arc::new(AtomicUsize::new(0));
    let counted = wakes.clone();
    let mut map = renderer(
        &gpu,
        &tiles,
        Some(Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        })),
    );
    let picture = render_with(&gpu, &mut map, &scene, 1.0).expect("rendered");
    assert!(!map.pending() && tiles.misses().is_empty());
    let local_ms = scene.local_now_ms;
    let mut window = Window::new(&gpu);

    // drawing: no card, the map as it is
    let (answer, _) = window.frame(&gpu, &mut map, local_ms, 0.0, Vec::new());
    assert_eq!(answer, None);
    assert_eq!(differing_pixels(&window.read(&gpu), &picture), 0, "no card while the map draws");

    // a validation error (a buffer both mapped for reading and used as vertices)
    let _ = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("broken"),
        size: 16,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::VERTEX,
        mapped_at_creation: false,
    });
    let error = map.error().expect("caught");
    assert!(!error.lost && error.message.contains("'broken'"), "{error:?}");
    assert_eq!(wakes.load(Ordering::SeqCst), 1, "the loop is woken to show the card");

    // the card: laid out on its first frame, shown from the second
    for egui_s in [1.0, 2.0] {
        let (answer, _) = window.frame(&gpu, &mut map, local_ms, egui_s, Vec::new());
        assert_eq!(answer, None);
    }
    let shown = window.read(&gpu);
    save(&shown, "b9-map-error-card.png");
    let whole = Rect::new(0, 0, shown.width, shown.height);
    // the card is what changed the picture: its border, text and buttons (its fill is the dark
    // land's grey, so only where it covers the sea or a road)
    let changed = Image {
        width: shown.width,
        height: shown.height,
        rgba: shown.rgba.chunks_exact(4).zip(picture.rgba.chunks_exact(4)).flat_map(|(a, b)| if a == b { [0; 4] } else { [255; 4] }).collect(),
    };
    let card = bounds(&changed, whole, |p| p[0] == 255).expect("the card changed the picture");
    eprintln!("card {card:?}");
    // 420 + 2 × (16 + 1) wide, centred on the 1100 × 720 map
    assert!((i64::from(card.width()) - 454).abs() <= 1, "{card:?}");
    let [cx, cy] = card.centre();
    assert!((cx - 550.0).abs() <= 1.0 && (cy - 360.0).abs() <= 1.0, "{card:?}");
    // the map outside it is the last picture, untouched
    let outside = whole.pixels().filter(|&(x, y)| !card.contains(x, y) && pixel(&shown, x, y) != pixel(&picture, x, y)).count();
    assert_eq!(outside, 0, "the stopped map keeps its last picture round the card");
    // the --line border along the straight parts of its sides
    let line = rgba(theme::LINE);
    let sides: Vec<(u32, u32)> = (card.x0 + 6..card.x1 - 6)
        .flat_map(|x| [(x, card.y0), (x, card.y1 - 1)])
        .chain((card.y0 + 6..card.y1 - 6).flat_map(|y| [(card.x0, y), (card.x1 - 1, y)]))
        .collect();
    let bordered = sides.iter().filter(|&&(x, y)| close(pixel(&shown, x, y), line, 2)).count();
    assert!(bordered * 100 >= sides.len() * 95, "{bordered} of {} border pixels are --line", sides.len());
    // the --surface fill in the padding under the top border
    let padding = Rect::new(card.x0 + 4, card.y0 + 3, card.width() - 8, 12);
    let filled = count(&shown, padding, |p| p == rgba(theme::SURFACE));
    assert_eq!(filled, padding.len(), "the padding is --surface");
    // the title in --text-1 and the message in --text-2, light on the card
    let title = Rect::new(card.x0 + 17, card.y0 + 17, 420, 20);
    let message = Rect::new(card.x0 + 17, card.y0 + 17 + 20 + 8, 420, 16);
    let lit = |r: Rect, c: egui::Color32| count(&shown, r, |p| close(p, rgba(c), 24));
    assert!(lit(title, theme::TEXT_1) > 60, "the title: {}", lit(title, theme::TEXT_1));
    assert!(lit(message, theme::TEXT_2) > 60, "the message: {}", lit(message, theme::TEXT_2));
    // two .buttons, 32 px high, outlined in --line-control, side by side along the bottom
    let row = Rect::new(card.x0 + 1, card.y1 - 17 - 32 - 2, card.width() - 2, 36);
    // (a grey from 75 up to --line-control's 110 joins each button's sides through its antialiased
    // rounded corners)
    let mut buttons = components(&shown, row, |p| p[0] == p[1] && p[1] == p[2] && (75..=114).contains(&p[0]));
    buttons.retain(|c| c.pixels > 100);
    buttons.sort_by_key(|c| c.bounds.x0);
    assert_eq!(buttons.len(), 2, "{buttons:?}");
    for b in &buttons {
        assert!((i64::from(b.bounds.height()) - 32).abs() <= 1, "{b:?}");
        assert!((i64::from(b.bounds.y1) - i64::from(card.y1 - 17)).abs() <= 1, "{b:?}");
    }
    assert!((i64::from(buttons[0].bounds.x0) - i64::from(card.x0 + 17)).abs() <= 1, "{:?}", buttons[0]);
    let gap = i64::from(buttons[1].bounds.x0) - i64::from(buttons[0].bounds.x1);
    assert!((gap - 8).abs() <= 1, "8 px between the buttons: {gap}");

    // a stopped map waits for nothing and draws nothing new, whatever the scene
    assert!(!map.pending());
    map.update(app_scene(V3, Basemap::Satellite));
    window.frame(&gpu, &mut map, local_ms, 2.5, Vec::new());
    assert_eq!(differing_pixels(&window.read(&gpu), &shown), 0, "still the last picture under the card");
    map.update(scene.clone());

    // Copy details: the message, this adapter, the system, wgpu's report
    let (answer, output) = window.frame(&gpu, &mut map, local_ms, 3.0, click(buttons[0].bounds.centre()));
    assert_eq!(answer, None);
    let copied: Vec<&String> =
        output.commands.iter().filter_map(|c| if let egui::OutputCommand::CopyText(t) = c { Some(t) } else { None }).collect();
    assert_eq!(copied, [&map_error::details(&error, Some(&gpu.adapter.get_info()))]);
    assert!(copied[0].starts_with(&format!("RustRadar map error: {}\nRenderer: {}", error.message, gpu.adapter.get_info().name)), "{}", copied[0]);

    // Retry: the error is cleared and the next frame is the map, with no card
    let (answer, _) = window.frame(&gpu, &mut map, local_ms, 4.0, click(buttons[1].bounds.centre()));
    assert_eq!(answer, Some(Retry::Redraw));
    assert_eq!(map.error(), None);
    window.frame(&gpu, &mut map, local_ms, 5.0, Vec::new());
    let again = window.read(&gpu);
    save(&again, "b9-map-error-retried.png");
    assert_eq!(differing_pixels(&again, &picture), 0, "the map drawn again, no card");
}

/// The engine's frames on `target` at `NOW_MS` until the map waits for nothing (tiles in), then
/// one more for egui; egui's clock moves a second each frame. Returns the last frame.
fn settle(engine: &mut Engine, gpu: &Gpu, target: &Offscreen, egui_s: &mut f64) -> Image {
    for _ in 0..1000 {
        engine_frame(engine, gpu, target, egui_s, Vec::new());
        if !engine.map.pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!engine.map.pending(), "still loading: {:?}", engine.map.pending_layers());
    engine_frame(engine, gpu, target, egui_s, Vec::new())
}

/// One frame of `engine` onto `target`, as the window's `View::redraw` draws one.
fn engine_frame(engine: &mut Engine, gpu: &Gpu, target: &Offscreen, egui_s: &mut f64, events: Vec<Event>) -> Image {
    *egui_s += 1.0;
    let raw = RawInput {
        screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(SIZE[0] as f32, SIZE[1] as f32))),
        time: Some(*egui_s),
        events,
        ..Default::default()
    };
    engine.frame(gpu, target.view(), SIZE, 1.0, raw, NOW_MS);
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    target.read(gpu).expect("read back")
}

/// The window's side, as `app.rs` wires it (`Engine` under `View`): a validation error shows the
/// card through the engine's own UI pass and its Retry brings the map back; a lost device asks the
/// window for a new one at once, once; `Engine::rebuild` on the new device (the map, egui's
/// renderer and a fresh egui context) draws the same frame again, pixel for pixel.
#[test]
fn the_engine_shows_the_card_and_rebuilds_after_a_lost_device() {
    let Some(shared) = gpu() else { return };
    let first = own_device(&shared);
    let startup = Startup { basemap: Some(Basemap::Dark), fixture: None, demo: false };
    let state = AppState::new(startup, None, None, None, V3.view(), SIZE.map(f64::from));
    let mut engine = Engine::new(&first, FORMAT, state);
    let tiles = Arc::new(DirTiles::fixtures());
    engine.map.set_tiles(tiles.clone(), None);
    engine.map.catch_errors(&first, None);
    let mut egui_s = 0.0;
    let target = Offscreen::new(&first, SIZE);
    let before = settle(&mut engine, &first, &target, &mut egui_s);
    assert!(tiles.misses().is_empty(), "the fixtures lack {:?}", tiles.misses());
    assert_eq!(engine.map.error(), None);
    assert!(!engine.take_rebuild(), "no rebuild while the map draws");

    // a validation error: the card over the stopped map, from the engine's UI pass
    let _ = first.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("broken"),
        size: 16,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::VERTEX,
        mapped_at_creation: false,
    });
    assert!(engine.map.error().is_some_and(|e| !e.lost));
    assert!(!engine.take_rebuild(), "an error that is not a loss needs no new device");
    engine_frame(&mut engine, &first, &target, &mut egui_s, Vec::new());
    let shown = engine_frame(&mut engine, &first, &target, &mut egui_s, Vec::new());
    let changed = Image {
        width: shown.width,
        height: shown.height,
        rgba: shown.rgba.chunks_exact(4).zip(before.rgba.chunks_exact(4)).flat_map(|(a, b)| if a == b { [0; 4] } else { [255; 4] }).collect(),
    };
    let card = bounds(&changed, Rect::new(0, 0, shown.width, shown.height), |p| p[0] == 255).expect("the card over the map");
    assert!((i64::from(card.width()) - 454).abs() <= 1, "{card:?}");
    let row = Rect::new(card.x0 + 1, card.y1 - 17 - 32 - 2, card.width() - 2, 36);
    let mut buttons = components(&shown, row, |p| p[0] == p[1] && p[1] == p[2] && (75..=114).contains(&p[0]));
    buttons.retain(|c| c.pixels > 100);
    buttons.sort_by_key(|c| c.bounds.x0);
    assert_eq!(buttons.len(), 2, "{buttons:?}");
    engine_frame(&mut engine, &first, &target, &mut egui_s, click(buttons[1].bounds.centre()));
    assert_eq!(engine.map.error(), None, "Retry cleared the error");
    let again = engine_frame(&mut engine, &first, &target, &mut egui_s, Vec::new());
    assert_eq!(differing_pixels(&again, &before), 0, "the map again, no card");

    // the device lost: a new one at once, asked once
    first.device.destroy();
    let _ = first.device.poll(wgpu::PollType::wait_indefinitely());
    assert!(engine.map.error().is_some_and(|e| e.lost));
    assert!(engine.take_rebuild(), "a new device at once");
    assert!(!engine.take_rebuild(), "asked once for this loss");
    let second = own_device(&shared);
    engine.rebuild(&second, FORMAT);
    let target = Offscreen::new(&second, SIZE);
    let after = settle(&mut engine, &second, &target, &mut egui_s);
    save(&after, "b9-engine-after-the-rebuild.png");
    assert_eq!(engine.map.error(), None);
    assert!(!engine.take_rebuild());
    let differing = differing_pixels(&after, &before);
    assert_eq!(differing, 0, "the rebuilt engine draws the same frame ({differing} pixels differ)");
}
