//! Item U16's acceptance (plan stage 7; doc 06 §19): the map controls, the layers menu and the
//! settings menu as the UI pass draws them, on SwiftShader.
//!
//! - `u16-closed.png`: the column at V1 against `docs/port/reference/dark-v1-1440x900.png`: the
//!   three stacks and their eight icons on the same pixels;
//! - `u16-turned.png`: zoomed in to 18, tilted and turned, signed in: zoom in off, the tilt
//!   pressed, the compass needle in the accent turned by the bearing, the key in `--live`;
//! - `u16-inset.png`: an airport board open, the column 372 px further left beside the panel;
//! - `u16-layers.png`, `u16-layers-full.png`, `u16-layers-inset.png`: the layers menu (the
//!   defaults; the realistic map with the rail map and terrain on), each part where Chromium lays
//!   out today's markup;
//! - `u16-settings.png`, `u16-settings-off.png`: the settings menu (place names on; off, with the
//!   exaggeration at 40);
//! - `u16-window-v1.png`: through the window's engine over the globe: the layers button opens the
//!   menu, its Terrain row and Satellite reach the settings, Escape closes it.
//!
//! The Chromium layouts were made from `MapControls.svelte` (with `LayersMenu.svelte` and
//! `SettingsMenu.svelte`) server-rendered by Svelte with the same props, laid out with app.css and
//! the Overpass fonts (Playwright's Chromium, scale 1, 1440 × 900); their numbers are written into
//! the checks.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::shots::{Rect, bounds, gpu, pixel, save};
use common::NOW_MS;
use egui::{Color32, Event, Key, PointerButton, Pos2, RawInput, Vec2};
use rustradar_app::app::Engine;
use rustradar_app::state::shortcuts::Menu;
use rustradar_app::state::{Action, AppState, Startup};
use rustradar_app::ui::map_controls::{self, Placed};
use rustradar_app::ui::shell::{self, Slots};
use rustradar_app::ui::theme;
use rustradar_core::geo::Basemap;
use rustradar_core::model::SessionInfo;
use rustradar_core::settings::RailShows;
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_map::io::DirTiles;
use rustradar_scene::camera::ViewState;

/// The desktop reference pictures' canvas, CSS px, at scale 1.
const SIZE: [u32; 2] = [1440, 900];
/// V1, the initial view.
const V1: ViewState = ViewState { longitude: 10.0, latitude: 35.0, zoom: 1.9, bearing: 0.0, pitch: 0.0, position: None };

/// egui alone onto a target cleared to `--ink`, as the window's UI pass draws it.
struct Canvas {
    ctx: egui::Context,
    egui: egui_wgpu::Renderer,
    target: Offscreen,
}

impl Canvas {
    fn new(gpu: &Gpu) -> Self {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let egui = egui_wgpu::Renderer::new(&gpu.device, FORMAT, egui_wgpu::RendererOptions::default());
        Canvas { ctx, egui, target: Offscreen::new(gpu, SIZE) }
    }

    /// A frame at egui time `time`: the shell (without its slots unless `shell`) and the controls
    /// as the UI pass draws them.
    fn frame(&mut self, gpu: &Gpu, time: f64, state: &AppState, shell: bool) -> Placed {
        let raw = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(SIZE[0] as f32, SIZE[1] as f32))),
            time: Some(time),
            ..Default::default()
        };
        let mut placed = Placed::default();
        let mut actions: Vec<Action> = Vec::new();
        let output = self.ctx.run_ui(raw, |ui| {
            map_controls::escape(ui.ctx(), state, &mut actions);
            if shell {
                shell::show(ui.ctx(), state, NOW_MS, &mut actions, &Slots::APP);
            }
            placed = map_controls::ui(ui.ctx(), state, &mut actions);
        });
        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: self.target.size(), pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui.update_texture(device, queue, id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("u16") });
        let mut commands = self.egui.update_buffers(device, queue, &mut encoder, &primitives, &screen);
        {
            let [r, g, b, _] = theme::INK.to_array().map(|c| f64::from(c) / 255.0);
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: self.target.view(),
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }), store: wgpu::StoreOp::Store },
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
        placed
    }

    /// Frames at egui times 0–3 s (an anchored area is sized in its first frame, the popover's
    /// whole-pixel height takes the second, the entry and the inset ease out), then the picture.
    fn settle(&mut self, gpu: &Gpu, state: &AppState, shell: bool) -> (Placed, Image) {
        let mut placed = Placed::default();
        for time in [0.0, 1.0, 2.0, 3.0] {
            placed = self.frame(gpu, time, state, shell);
        }
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        (placed, self.target.read(gpu).expect("read back"))
    }
}

/// A picture of `docs/port/reference` as RGBA.
fn reference(name: &str) -> Image {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference").join(name);
    let file = std::io::BufReader::new(std::fs::File::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())));
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().expect("a PNG");
    let mut buf = vec![0; reader.output_buffer_size().expect("a size")];
    let info = reader.next_frame(&mut buf).expect("a frame");
    let channels = info.color_type.samples();
    let rgba = buf[..info.buffer_size()].chunks_exact(channels).flat_map(|p| [p[0], p[1], p[2], if channels == 4 { p[3] } else { 255 }]).collect();
    Image { width: info.width, height: info.height, rgba }
}

fn state_at(view: ViewState) -> AppState {
    AppState::new(Startup::default(), None, None, None, view, SIZE.map(f64::from))
}

fn rgb(c: Color32) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

fn rect(r: egui::Rect) -> Rect {
    Rect { x0: r.left().round() as u32, y0: r.top().round() as u32, x1: r.right().round() as u32, y1: r.bottom().round() as u32 }
}

/// The pixels in `area` of exactly `c`, bounded.
fn painted(image: &Image, area: Rect, c: Color32) -> Option<Rect> {
    bounds(image, area, |p| p == rgb(c))
}

/// Ink in `area` that is not `under` (by more than 12 on a channel).
fn ink(image: &Image, area: Rect, under: Color32) -> Option<Rect> {
    let u = rgb(under);
    bounds(image, area, |p| (0..3).any(|i| p[i].abs_diff(u[i]) > 12))
}

/// The ink in `area` that is not `under`, summed over the channels' distances, as pixels' worth.
fn ink_amount(image: &Image, area: Rect, under: [u8; 4]) -> f64 {
    area.pixels().map(|(x, y)| (0..3).map(|i| f64::from(pixel(image, x, y)[i].abs_diff(under[i]))).sum::<f64>() / (3.0 * 255.0)).sum()
}

fn near(got: f32, want: f32, tolerance: f32, what: &str) {
    assert!((got - want).abs() <= tolerance, "{what}: {got}, Chromium {want} (± {tolerance})");
}

/// The popover's top edge, a whole pixel: where Chromium paints the edge of its box whose top is
/// at `chromium` (rounded), or a pixel higher (the box here is a whole number of pixels tall, its
/// height rounded up).
fn top_edge(got: f32, chromium: f32) {
    let painted = chromium.round();
    assert!(got.fract() == 0.0 && (got == painted || got == painted - 1.0), "the popover's top: {got}, Chromium {chromium} (painted at {painted})");
}

/// A colour over `--surface` at its own alpha (egui blends in gamma space, as Chromium does).
fn over_surface(c: Color32) -> Color32 {
    let a = f32::from(c.a()) / 255.0;
    let unmul = |v: u8| f32::from(v) / a;
    let mix = |fg: f32, bg: u8| (fg * a + f32::from(bg) * (1.0 - a)).round() as u8;
    let s = theme::SURFACE;
    Color32::from_rgb(mix(unmul(c.r()), s.r()), mix(unmul(c.g()), s.g()), mix(unmul(c.b()), s.b()))
}

/// The rows of `x` from `top` to `bottom` painted `--line` (the menus' rules).
fn rules(image: &Image, x: u32, top: u32, bottom: u32) -> Vec<u32> {
    (top..bottom).filter(|&y| pixel(image, x, y) == rgb(theme::LINE)).collect()
}

/// The column's stacks and icons against the reference picture: the closed controls at V1.
#[test]
fn the_controls_closed_as_today() {
    let Some(gpu) = gpu() else { return };
    let mut canvas = Canvas::new(&gpu);
    let state = state_at(V1);
    let (placed, image) = canvas.settle(&gpu, &state, false);
    save(&image, "u16-closed.png");
    // Chromium (and the reference): the column 1386–1424 × 582–872, stacks 134, 70 and 70 tall
    assert_eq!(rect(placed.column), Rect { x0: 1386, y0: 582, x1: 1424, y1: 872 });
    let stacks = placed.stacks.map(rect);
    assert_eq!(stacks.map(|r| (r.y0, r.y1)), [(582, 716), (724, 794), (802, 872)]);
    // each stack's border (--line) round --surface
    for s in stacks {
        assert_eq!(pixel(&image, s.x0, (s.y0 + s.y1) / 2), rgb(theme::LINE), "left border of {s:?}");
        assert_eq!(pixel(&image, s.x1 - 1, (s.y0 + s.y1) / 2), rgb(theme::LINE), "right border of {s:?}");
        assert_eq!(pixel(&image, s.x0 + 19, s.y0), rgb(theme::LINE), "top border of {s:?}");
        assert_eq!(pixel(&image, s.x0 + 2, s.y0 + 2), rgb(theme::SURFACE), "fill of {s:?}");
    }
    let reference = reference("dark-v1-1440x900.png");
    // the same pixels as today's, the icons' ink included: each button's 32 × 32 within a few
    // levels on average, and its icon's ink within 10 % of today's (15 % for the keyboard: the
    // reference's GPU raster covers about a third of each of Lucide's dots, U2, and the keyboard
    // has nine; Chromium's own raster of the same markup gives 53.9 against this 50.1)
    let buttons = [585, 617, 649, 681, 727, 759, 805, 837];
    for (n, y) in buttons.into_iter().enumerate() {
        let tolerance = if n == 3 { 0.15 } else { 0.1 };
        let b = Rect::new(1389, y, 32, 32);
        let diff: f64 = b.pixels().map(|(x, y)| (0..3).map(|i| f64::from(pixel(&image, x, y)[i].abs_diff(pixel(&reference, x, y)[i]))).sum::<f64>()).sum::<f64>() / (3.0 * b.len() as f64);
        assert!(diff < 4.0, "button {n}: {diff:.2} levels from today's on average");
        let surface = rgb(theme::SURFACE);
        let (mine, today) = (ink_amount(&image, b, surface), ink_amount(&reference, b, surface));
        assert!((mine - today).abs() <= tolerance * today, "button {n}'s icon: ink {mine:.1}, today {today:.1}");
        let (a, t) = (ink(&image, b, theme::SURFACE).expect("an icon"), ink(&reference, b, theme::SURFACE).expect("today's icon"));
        assert!(a.x0.abs_diff(t.x0) <= 1 && a.y0.abs_diff(t.y0) <= 1 && a.x1.abs_diff(t.x1) <= 1 && a.y1.abs_diff(t.y1) <= 1, "button {n}: icon at {a:?}, today {t:?}");
    }
}

/// Zoomed in to 18, tilted 45° and turned 30°, signed in to Flightradar24.
#[test]
fn the_controls_turned_and_signed_in() {
    let Some(gpu) = gpu() else { return };
    let mut canvas = Canvas::new(&gpu);
    let mut state = state_at(ViewState { zoom: 18.0, pitch: 45.0, bearing: 30.0, ..V1 });
    state.aircraft.set_session(SessionInfo { authenticated: true, expires: None });
    let (_, image) = canvas.settle(&gpu, &state, false);
    save(&image, "u16-turned.png");
    // the key in --live
    let key = Rect::new(1389, 649, 32, 32);
    assert!(painted(&image, key, theme::LIVE).is_some(), "the key in --live");
    // zoom in at half opacity in --text-3 (Chromium: 104 at its brightest), zoom out on (159)
    let brightest = |area: Rect| area.pixels().map(|(x, y)| pixel(&image, x, y)[0]).max().unwrap();
    let plus = brightest(Rect::new(1389, 727, 32, 32));
    assert!(plus.abs_diff(104) <= 6, "zoom in off: {plus}");
    let minus = brightest(Rect::new(1389, 759, 32, 32));
    assert!(minus.abs_diff(159) <= 10, "zoom out on: {minus}");
    // the tilt pressed: --amber-soft over --surface, the icon in --amber
    let tilt = Rect::new(1389, 805, 32, 32);
    assert_eq!(pixel(&image, 1391, 807), rgb(over_surface(theme::AMBER_SOFT)), "the tilt's fill");
    assert!(painted(&image, tilt, theme::AMBER).is_some(), "the tilt's icon in --amber");
    // the compass on: the needle in --amber, turned 30° anticlockwise (Chromium's ink at x
    // 1400–1414, y 845–863, its tip's first --amber pixel at (1401, 846), up and left of the
    // middle)
    let compass = Rect::new(1389, 837, 32, 32);
    let needle = ink(&image, compass, theme::SURFACE).expect("the needle");
    assert!(needle.x0.abs_diff(1400) <= 1 && needle.x1.abs_diff(1414) <= 1 && needle.y0.abs_diff(845) <= 1 && needle.y1.abs_diff(863) <= 1, "{needle:?}");
    let amber = |p: [u8; 4]| p[2] > 200 && p[0] < 80;
    let tip = compass.pixels().find(|&(x, y)| amber(pixel(&image, x, y))).expect("--amber ink");
    assert!(tip.0.abs_diff(1401) <= 1 && tip.1.abs_diff(846) <= 1, "the tip: {tip:?}");
}

/// An airport board open: the column 372 px left of its place, the layers menu with it.
#[test]
fn the_controls_beside_a_panel() {
    let Some(gpu) = gpu() else { return };
    let mut canvas = Canvas::new(&gpu);
    let mut state = state_at(V1);
    state.airport.open("ARN", NOW_MS);
    let (placed, image) = canvas.settle(&gpu, &state, true);
    save(&image, "u16-inset.png");
    assert_eq!(rect(placed.column), Rect { x0: 1014, y0: 582, x1: 1052, y1: 872 });
    state.set_menu(Some(Menu::Layers));
    let (placed, image) = canvas.settle(&gpu, &state, true);
    save(&image, "u16-layers-inset.png");
    // Chromium: 672–1006 × 397.52–872
    let popover = rect(placed.popover.expect("the popover"));
    assert_eq!((popover.x0, popover.x1, popover.y1), (672, 1006, 872));
    assert_eq!(pixel(&image, 672, 600), rgb(theme::LINE), "its left border");
    assert_eq!(pixel(&image, 1005, 600), rgb(theme::LINE), "its right border");
}

/// The layers menu with the defaults, then on the realistic map with the rail map and terrain on.
#[test]
fn the_layers_menu() {
    let Some(gpu) = gpu() else { return };
    let mut canvas = Canvas::new(&gpu);
    let mut state = state_at(V1);
    state.set_menu(Some(Menu::Layers));
    let (placed, image) = canvas.settle(&gpu, &state, false);
    save(&image, "u16-layers.png");
    let popover = placed.popover.expect("the popover");
    // Chromium: 1044–1378 × 397.52–872 (its top edge painted at 398)
    assert_eq!((popover.left(), popover.right(), popover.bottom()), (1044.0, 1378.0, 872.0));
    top_edge(popover.top(), 397.52);
    assert_eq!(pixel(&image, 1044, 600), rgb(theme::LINE));
    assert_eq!(pixel(&image, 1377, 600), rgb(theme::LINE));
    // the layers button pressed
    assert_eq!(pixel(&image, 1391, 587), rgb(over_surface(theme::AMBER_SOFT)), "the layers button pressed");
    // the sections' rules (Chromium: 652.45 and 739.23), 300 wide from 1061
    assert_eq!(rules(&image, 1200, 420, 870), [652, 739]);
    assert_eq!(painted(&image, Rect::new(1050, 652, 320, 1), theme::LINE).map(|r| (r.x0, r.x1)), Some((1061, 1361)));
    // the basemap's box (Chromium: 1061–1361 × 441.31–475.5) with Dark chosen (1064 × 444.31, 96.66 wide)
    let basemap = painted(&image, Rect::new(1055, 435, 312, 46), theme::LINE_CONTROL).expect("the basemap's box");
    assert_eq!((basemap.x0, basemap.x1), (1061, 1361));
    near(basemap.y0 as f32, 441.31, 1.0, "the basemap's top");
    near(basemap.y1 as f32, 475.5, 1.0, "the basemap's bottom");
    let dark = painted(&image, Rect::new(1061, 441, 300, 35), theme::AMBER).expect("Dark chosen");
    near(dark.x0 as f32, 1064.0, 1.0, "Dark's left");
    near(dark.x1 as f32, 1160.66, 1.0, "Dark's right");
    // the switches (Chromium: x 1327–1361, tops 492.98, 533.97, 573.47, 612.95, 699.73, 786.52, 827.5):
    // terrain off, buildings and place names on, rail map off; aircraft labels off; weather off,
    // day and night on
    let tops = [492.98, 533.97, 573.47, 612.95, 699.73, 786.52, 827.5];
    let on = [false, true, true, false, false, false, true];
    for (top, on) in tops.into_iter().zip(on) {
        let area = Rect::new(1320, top as u32 - 4, 48, 28);
        let fill = if on { theme::AMBER } else { theme::LINE_CONTROL };
        let track = painted(&image, area, fill).unwrap_or_else(|| panic!("a switch at {top}"));
        near(track.x0 as f32, 1327.0, 1.0, "a switch's left");
        near(track.x1 as f32, 1361.0, 1.0, "a switch's right");
        near((track.y0 + track.y1) as f32 / 2.0, top + 10.0, 1.0, "a switch's middle");
    }
    // the key caps (Chromium: 1285–1315 × 23 from 491.48, 532.47, 571.97, 611.45, 698.23, 785.02, 826)
    for top in [491.48, 532.47, 571.97, 611.45, 698.23, 785.02, 826.0] {
        let cap = painted(&image, Rect::new(1280, top as u32 - 3, 40, 29), theme::LINE).unwrap_or_else(|| panic!("a key cap at {top}"));
        near(cap.x0 as f32, 1285.0, 1.0, "a key cap's left");
        near(cap.x1 as f32, 1315.0, 1.0, "a key cap's right");
        near(cap.y0 as f32, top, 1.0, "a key cap's top");
    }
    // the headings' ink (Chromium's line boxes: 414.52, 665.45, 752.23; 16.8 tall)
    for top in [414.52, 665.45, 752.23] {
        let heading = ink(&image, Rect::new(1061, top as u32 - 2, 80, 21), theme::SURFACE).unwrap_or_else(|| panic!("a heading at {top}"));
        assert!(heading.y0 as f32 >= top && heading.y1 as f32 <= top + 17.8, "heading at {top}: {heading:?}");
    }

    let mut state = state_at(V1);
    state.set_menu(Some(Menu::Layers));
    state.persisted.settings.basemap = Basemap::Realistic;
    state.persisted.settings.rail_map = true;
    state.persisted.settings.rail_shows = RailShows::Blocks;
    state.persisted.settings.terrain = true;
    let mut canvas = Canvas::new(&gpu);
    let (placed, image) = canvas.settle(&gpu, &state, false);
    save(&image, "u16-layers-full.png");
    let popover = placed.popover.expect("the popover");
    // Chromium: 188.98–872
    top_edge(popover.top(), 188.98);
    // the weather's box (Chromium: 276.97 + 18.19 + 8 = 303.16, 64.38 tall), Live chosen
    let weather = painted(&image, Rect::new(1055, 298, 312, 74), theme::LINE_CONTROL).expect("the weather's box");
    near(weather.y0 as f32, 303.16, 1.0, "the weather's top");
    near(weather.y1 as f32, 367.54, 1.0, "the weather's bottom");
    let weather_live = painted(&image, Rect::new(1061, 300, 120, 40), theme::AMBER).expect("Live chosen");
    near(weather_live.y0 as f32, 306.16, 1.0, "Live's top");
    // Signal blocks chosen (Chromium: 1212 × 584.47, 146 × 28.19)
    let blocks = painted(&image, Rect::new(1061, 578, 300, 40), theme::AMBER).expect("Signal blocks chosen");
    near(blocks.x0 as f32, 1212.0, 1.0, "Signal blocks' left");
    near(blocks.y0 as f32, 584.47, 1.0, "Signal blocks' top");
    // its note (Chromium: 619.66) and the rules (652.45, 739.23 as before)
    let note = ink(&image, Rect::new(1061, 617, 300, 21), theme::SURFACE).expect("the note");
    assert!(note.y0 as f32 >= 619.66 - 1.0 && note.y1 as f32 <= 619.66 + 17.8, "{note:?}");
    assert_eq!(rules(&image, 1200, 640, 870), [652, 739]);
    // Tilt-shift's switch off, Terrain's on (Chromium: 385.02, 426)
    for (top, fill) in [(385.02, theme::LINE_CONTROL), (426.0, theme::AMBER)] {
        let track = painted(&image, Rect::new(1320, top as u32 - 4, 48, 28), fill).unwrap_or_else(|| panic!("a switch at {top}"));
        near((track.y0 + track.y1) as f32 / 2.0, top + 10.0, 1.0, "a switch's middle");
    }
}

/// The settings menu with place names on, then off with the exaggeration at 40.
#[test]
fn the_settings_menu() {
    let Some(gpu) = gpu() else { return };
    let mut canvas = Canvas::new(&gpu);
    let mut state = state_at(V1);
    state.set_menu(Some(Menu::Settings));
    let (placed, image) = canvas.settle(&gpu, &state, false);
    save(&image, "u16-settings.png");
    let popover = placed.popover.expect("the popover");
    // Chromium: 1044–1378 × 562.88–872
    assert_eq!((popover.left(), popover.right(), popover.bottom()), (1044.0, 1378.0, 872.0));
    top_edge(popover.top(), 562.88);
    assert_eq!(pixel(&image, 1391, 619), rgb(over_surface(theme::AMBER_SOFT)), "the settings button pressed");
    // the rule (Chromium: 782.22)
    assert_eq!(rules(&image, 1200, 570, 870), [782]);
    // Medium chosen (Chromium: 1212 × 633.86, 146 × 28.19)
    let medium = painted(&image, Rect::new(1061, 625, 300, 70), theme::AMBER).expect("Medium chosen");
    near(medium.x0 as f32, 1212.0, 1.0, "Medium's left");
    near(medium.x1 as f32, 1358.0, 1.0, "Medium's right");
    near(medium.y0 as f32, 633.86, 1.0, "Medium's top");
    // the slider's track (Chromium: 1061–1361 × 733.42–749.42, the 8 px track in the middle) and
    // its thumb at 10 of 1–40: centre 1061 + 8 + 284 × 9 / 39
    let track = bounds(&image, Rect::new(1055, 728, 312, 26), |p| p[0] > 100 && p[0] == p[2]).expect("the track's border");
    assert_eq!((track.x0, track.x1), (1061, 1361));
    near((track.y0 + track.y1) as f32 / 2.0, 741.42, 1.0, "the track's middle");
    let thumb = painted(&image, Rect::new(1061, 728, 300, 26), theme::AMBER).expect("the thumb");
    near(thumb.y0 as f32, 733.42, 1.0, "the thumb's top");
    near(thumb.x1 as f32, 1069.0 + 284.0 * 9.0 / 39.0 + 8.0, 1.0, "the thumb's right");
    // ×10 at the right in --amber (Chromium: 1337.36–1361 × 707.23)
    let value = bounds(&image, Rect::new(1300, 703, 70, 24), |p| p[2] > 160 && p[0] < 120).expect("the value");
    near(value.x1 as f32, 1361.0, 1.5, "the value's right");
    // the Data row's switch on (Chromium: 1327 × 827.5)
    let switch = painted(&image, Rect::new(1320, 823, 48, 28), theme::AMBER).expect("the switch");
    near((switch.y0 + switch.y1) as f32 / 2.0, 837.5, 1.0, "the switch's middle");

    let mut state = state_at(V1);
    state.set_menu(Some(Menu::Settings));
    state.persisted.settings.labels = false;
    state.persisted.settings.exaggeration = 40.0;
    state.persisted.settings.label_size = 1.5;
    let mut canvas = Canvas::new(&gpu);
    let (placed, image) = canvas.settle(&gpu, &state, false);
    save(&image, "u16-settings-off.png");
    // Chromium: 542.08–872
    top_edge(placed.popover.expect("the popover").top(), 542.08);
    // Extra large chosen, faded to 0.4 (Chromium: 1212 × 643.25)
    let faded = Color32::from_rgb(
        (f32::from(theme::AMBER.r()) * 0.4 + f32::from(theme::SURFACE.r()) * 0.6).round() as u8,
        (f32::from(theme::AMBER.g()) * 0.4 + f32::from(theme::SURFACE.g()) * 0.6).round() as u8,
        (f32::from(theme::AMBER.b()) * 0.4 + f32::from(theme::SURFACE.b()) * 0.6).round() as u8,
    );
    let large = bounds(&image, Rect::new(1061, 610, 300, 70), |p| (0..3).all(|i| p[i].abs_diff(rgb(faded)[i]) <= 2)).expect("Extra large chosen, faded");
    near(large.x0 as f32, 1212.0, 1.0, "Extra large's left");
    near(large.y0 as f32, 643.25, 1.0, "Extra large's top");
    // the note under it (Chromium: 678.44)
    let note = ink(&image, Rect::new(1061, 676, 300, 21), theme::SURFACE).expect("the note");
    assert!(note.y0 as f32 >= 678.44 - 1.0 && note.y1 as f32 <= 678.44 + 17.8, "{note:?}");
    // the thumb at the right end
    let thumb = painted(&image, Rect::new(1061, 728, 300, 26), theme::AMBER).expect("the thumb");
    near(thumb.x1 as f32, 1361.0, 1.0, "the thumb at 40");
}

// ------------------------------------------------------------------------------------------------
// through the window

/// The window's engine with the shell on, drawing into an offscreen target.
struct Window {
    engine: Engine,
    target: Offscreen,
    egui_s: f64,
}

impl Window {
    fn frame(&mut self, gpu: &Gpu, events: Vec<Event>, now: f64) {
        let mut input = RawInput {
            time: Some(self.egui_s),
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(SIZE[0] as f32, SIZE[1] as f32))),
            events,
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(1.0);
        self.egui_s += 0.25;
        self.engine.frame(gpu, self.target.view(), SIZE, 1.0, input, now);
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// A click at `at`: the pointer moves there, presses and lets go, each in a frame.
    fn click(&mut self, gpu: &Gpu, at: Pos2, now: f64) {
        let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        self.frame(gpu, vec![Event::PointerMoved(at)], now);
        self.frame(gpu, vec![button(true)], now);
        self.frame(gpu, vec![button(false)], now);
    }
}

/// V1 through the window's engine: the layers button opens the menu, Terrain and Satellite reach
/// the settings, Escape closes the menu.
#[test]
fn the_menus_through_the_window() {
    let Some(gpu) = gpu() else { return };
    let state = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, V1, SIZE.map(f64::from));
    let mut engine = Engine::new(&gpu, FORMAT, state);
    engine.map.set_tiles(Arc::new(DirTiles::fixtures()), None);
    engine.shell = true;
    engine.draw = false;
    let mut w = Window { engine, target: Offscreen::new(&gpu, SIZE), egui_s: 0.0 };
    let started = std::time::Instant::now();
    loop {
        w.frame(&gpu, Vec::new(), NOW_MS);
        if !w.engine.map.pending() || started.elapsed() > rustradar_map::headless::MAX_WAIT {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for _ in 0..3 {
        w.frame(&gpu, Vec::new(), NOW_MS);
    }
    // the layers button
    w.click(&gpu, Pos2::new(1405.0, 601.0), NOW_MS);
    assert_eq!(w.engine.state.shortcuts.menu(), Some(Menu::Layers));
    for _ in 0..4 {
        w.frame(&gpu, Vec::new(), NOW_MS);
    }
    // Terrain's row (Chromium: 485.5, 34.98 tall)
    w.click(&gpu, Pos2::new(1120.0, 503.0), NOW_MS);
    assert!(w.engine.state.persisted.settings.terrain, "Terrain on");
    w.frame(&gpu, vec![Event::PointerMoved(Pos2::new(1200.0, 300.0))], NOW_MS);
    w.engine.draw = true;
    w.frame(&gpu, Vec::new(), NOW_MS);
    let image = w.target.read(&gpu).expect("read back");
    save(&image, "u16-window-v1.png");
    // the menu over the globe, the globe's land left of it
    assert_eq!(pixel(&image, 1044, 700), rgb(theme::LINE), "the popover's left border");
    w.engine.draw = false;
    // Satellite (Chromium: 1162.66 × 441.31)
    w.click(&gpu, Pos2::new(1210.0, 458.0), NOW_MS);
    assert_eq!(w.engine.state.persisted.settings.basemap, Basemap::Satellite);
    // Escape closes the menu, and does nothing else
    let escape = |pressed| Event::Key { key: Key::Escape, physical_key: Some(Key::Escape), pressed, repeat: false, modifiers: Default::default() };
    w.frame(&gpu, vec![escape(true)], NOW_MS);
    w.frame(&gpu, vec![escape(false)], NOW_MS);
    assert_eq!(w.engine.state.shortcuts.menu(), None);
    // a press on the map closes it too
    w.click(&gpu, Pos2::new(1405.0, 601.0), NOW_MS);
    assert_eq!(w.engine.state.shortcuts.menu(), Some(Menu::Layers));
    w.click(&gpu, Pos2::new(600.0, 450.0), NOW_MS);
    assert_eq!(w.engine.state.shortcuts.menu(), None);
}
