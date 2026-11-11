//! Item U12's acceptance (plan stage 7; doc 06 §9, §10): the altitude chart and its export menu,
//! drawn from fixture trails in a 360 px panel (the flight panel's tab panel: 16 px padding, 328 px
//! of chart) at left 20, top 20 of a 400 × 440 canvas, and the four export formats written to a
//! folder.
//!
//! Shots (`target/shots/u12-chart-*.png`), each compared part by part with Chromium's layout of
//! today's `AltitudeChart.svelte` and `ExportMenu.svelte` mounted by Svelte in the same panel with
//! app.css (a 400 px high `aside.panel.surface`, Overpass from @fontsource, the viewer in UTC):
//! - `climb`: 60 points over 51 minutes climbing to 36,000 ft and level, Arlanda's zone: the grid at
//!   0, 15,000, 30,000, 45,000, the times 12:00, 12:15, 12:30, the end label over the dot;
//! - `hover`, `hover-flip`: the pointer 120 and 260 px into the plot: the line down the plot, the
//!   dot, the box right of the line, then left of it;
//! - `descend` (the end label under the dot), `low` (none), `long` (five hours: 04:00 and 06:00);
//! - `ended`: without the departure zone (`Your time, UTC`), the dot and its label in `--text-3`;
//! - `table`, `table-long`: the table's 31 rows, the first seven in view;
//! - `menu`: the export menu open; `menu-keys`: opened with Enter, Down on GPX (its ring);
//! - `saved`: `Saved` after a KML was written; `empty`: no trail, the message;
//! - `focus-plot`: the plot holding the keyboard, Left twice from the last point.
//!
//! The GPU tests need a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without
//! one they print `skipped: …` and pass. The rest drive the chart through egui alone.

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use common::shots::{Rect as Px, bounds, gpu, pixel, save};
use egui::{Area, Color32, Event, Frame, Id, Key, Modifiers, Order, PointerButton, Pos2, RawInput, Rect, Vec2};
use rustradar_app::state::aircraft::Answer;
use rustradar_app::ui::chart::{self, Chart, Placed};
use rustradar_app::ui::export_menu::{self, ExportFeeds};
use rustradar_app::ui::{anim, theme};
use rustradar_core::export::{EXPORT_FORMATS, ExportFormat, ExportMeta, export_path};
use rustradar_core::model::TrailPoint;
use rustradar_core::time::Clocks;
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};

/// The canvas, CSS px at scale 1.
const SIZE: [u32; 2] = [400, 440];
/// The panel's border box: its left top, its content (360 × 400) and the tab panel's padding.
const PANEL: Pos2 = Pos2::new(20.0, 20.0);
const CONTENT: Vec2 = Vec2::new(360.0, 400.0);
const PAD: f32 = 16.0;
const STOCKHOLM: &str = "Europe/Stockholm";

// ------------------------------------------------------------------------------------------------
// fixture trails (the Chromium page's, by the same integer formulas)

fn pt(i: u64, timestamp: u64, altitude: i32, ground_speed: u32) -> TrailPoint {
    TrailPoint {
        timestamp,
        latitude: 59.65 - i as f32 * 0.034,
        longitude: 17.92 - i as f32 * 0.042,
        altitude,
        ground_speed,
        track: 214,
        ..Default::default()
    }
}

fn climb() -> Vec<TrailPoint> {
    (0..60).map(|i| pt(i, 1_782_034_980 + 52 * i, (1_400 * i as i32).min(36_000), (180 + 9 * i as u32).min(448))).collect()
}

fn descend() -> Vec<TrailPoint> {
    (0..45).map(|i| pt(i, 1_782_040_000 + 40 * i, 36_000 - 350 * i as i32, 450 - 5 * i as u32)).collect()
}

fn low() -> Vec<TrailPoint> {
    (0..45).map(|i| pt(i, 1_782_040_000 + 40 * i, 36_000 - 800 * i as i32, 450 - 5 * i as u32)).collect()
}

fn long() -> Vec<TrailPoint> {
    (0..300)
        .map(|i| {
            let alt = if i < 20 {
                1_900 * i as i32
            } else if i < 270 {
                38_000
            } else {
                38_000 - 1_250 * (i as i32 - 270)
            };
            pt(i, 1_782_000_000 + 60 * i, alt, 470)
        })
        .collect()
}

fn meta() -> ExportMeta {
    ExportMeta {
        callsign: "SAS1417".into(),
        flight_number: "SK1417".into(),
        registration: "SE-ROR".into(),
        aircraft_type: "A20N".into(),
        origin: "ARN".into(),
        destination: "CPH".into(),
    }
}

/// The props of a scenario.
#[derive(Clone)]
struct Props {
    trail: Vec<TrailPoint>,
    zone: Option<&'static str>,
    place: Option<&'static str>,
    ended: bool,
    meta: ExportMeta,
}

impl Props {
    fn new(trail: Vec<TrailPoint>) -> Props {
        Props { trail, zone: Some(STOCKHOLM), place: Some("ARN"), ended: false, meta: meta() }
    }
}

// ------------------------------------------------------------------------------------------------
// where the files go

/// Writes each file into `dir` as the backend does (`backend::export::save`), noting the names.
struct Downloads {
    dir: Option<PathBuf>,
    asked: Arc<Mutex<Vec<String>>>,
}

impl ExportFeeds for Downloads {
    fn save(&mut self, file_name: &str, contents: String) -> Answer<Result<PathBuf, String>> {
        self.asked.lock().unwrap().push(file_name.to_owned());
        Answer::ready(rustradar_backend::export::save(self.dir.as_deref(), file_name, &contents))
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rustradar-u12-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ------------------------------------------------------------------------------------------------
// drawing

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    theme::apply(&ctx);
    anim::set_reduced_motion(&ctx, true);
    ctx
}

/// One pass at egui time `time` with `events`: the export menu's Escape step first (as the
/// window's UI pass), then the panel with the chart. Where the chart put things, and what of the
/// input reached past it.
fn pass(ctx: &egui::Context, time: f64, props: &Props, events: Vec<Event>) -> (Placed, Vec<Event>, egui::FullOutput) {
    let raw = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(SIZE[0] as f32, SIZE[1] as f32))),
        time: Some(time),
        events,
        ..Default::default()
    };
    let mut placed = None;
    let mut left = Vec::new();
    let output = ctx.run_ui(raw, |ui| {
        let ctx = ui.ctx().clone();
        export_menu::escape(&ctx);
        Area::new(Id::unique("panel")).order(Order::Middle).fixed_pos(PANEL).constrain(false).fade_in(false).show(&ctx, |ui| {
            theme::surface().show(ui, |ui| {
                ui.set_width(CONTENT.x);
                ui.set_min_height(CONTENT.y);
                // the panel's `overflow` cuts what hangs past its padding box
                ui.set_clip_rect(Rect::from_min_size(PANEL + Vec2::splat(1.0), CONTENT));
                Frame::NONE.inner_margin(PAD).show(ui, |ui| {
                    ui.set_width(CONTENT.x - 2.0 * PAD);
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    let chart = Chart { trail: &props.trail, time_zone: props.zone, place: props.place, ended: props.ended, export: Some(&props.meta) };
                    placed = Some(chart::chart(ui, Id::unique("chart"), &chart, &Clocks::default()));
                });
            });
        });
        left = ctx.input(|i| i.events.clone());
    });
    (placed.expect("the chart drawn"), left, output)
}

fn move_to(at: Pos2) -> Vec<Event> {
    vec![Event::PointerMoved(at)]
}

fn click(at: Pos2) -> Vec<Event> {
    vec![
        Event::PointerMoved(at),
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() },
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() },
    ]
}

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }
}

fn press(k: Key) -> Vec<Event> {
    vec![key(k, Modifiers::NONE)]
}

/// egui alone onto a target cleared to `--ink`.
struct Canvas {
    ctx: egui::Context,
    egui: egui_wgpu::Renderer,
    target: Offscreen,
}

impl Canvas {
    fn new(gpu: &Gpu) -> Self {
        let egui = egui_wgpu::Renderer::new(&gpu.device, FORMAT, egui_wgpu::RendererOptions::default());
        Canvas { ctx: context(), egui, target: Offscreen::new(gpu, SIZE) }
    }

    fn frame(&mut self, gpu: &Gpu, time: f64, props: &Props, events: Vec<Event>) -> Placed {
        let (placed, _, output) = pass(&self.ctx, time, props, events);
        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: self.target.size(), pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui.update_texture(device, queue, id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("altitude chart") });
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
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        placed
    }

    fn read(&self, gpu: &Gpu) -> Image {
        self.target.read(gpu).expect("read back")
    }
}

/// What a scenario does after its first frames: events at successive passes.
type Steps = Vec<Vec<Event>>;

/// Draws `props` for a second of egui time, then each of `steps` in a pass of its own, then one
/// more; the picture and where the chart put things.
fn shot(gpu: &Gpu, props: &Props, steps: impl FnOnce(&Placed) -> Steps, feeds: Option<Box<dyn ExportFeeds>>) -> (Image, Placed) {
    let mut canvas = Canvas::new(gpu);
    if let Some(feeds) = feeds {
        export_menu::install_feeds(&canvas.ctx, feeds);
    }
    canvas.frame(gpu, 0.0, props, Vec::new());
    let first = canvas.frame(gpu, 0.5, props, Vec::new());
    let mut t = 1.0;
    for events in steps(&first) {
        canvas.frame(gpu, t, props, events);
        t += 0.1;
    }
    let placed = canvas.frame(gpu, t, props, Vec::new());
    (canvas.read(gpu), placed)
}

// ------------------------------------------------------------------------------------------------
// checks

fn rgb(c: Color32) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

fn near(p: [u8; 4], c: Color32, tolerance: u8) -> bool {
    (0..3).all(|i| p[i].abs_diff(rgb(c)[i]) <= tolerance)
}

/// The pixels `x0..x1` × `y0..y1`.
const fn area(x0: u32, y0: u32, x1: u32, y1: u32) -> Px {
    Px { x0, y0, x1, y1 }
}

/// The rows in column `x` from `y0` to `y1` whose pixel is `c` (within 2).
fn rows_of(image: &Image, x: u32, y0: u32, y1: u32, c: Color32) -> Vec<u32> {
    (y0..y1).filter(|&y| near(pixel(image, x, y), c, 2)).collect()
}

/// The ink of text in `c` (within 24) inside `area`: its bounding box.
fn ink(image: &Image, area: Px, c: Color32) -> Option<Px> {
    bounds(image, area, |p| near(p, c, 24))
}

/// The area under the line: `--series-1` at 10 % over `--surface`.
fn area_fill() -> Color32 {
    let a = 0.1;
    let mix = |s: u8, d: u8| (f32::from(s) * a + f32::from(d) * (1.0 - a)).round() as u8;
    Color32::from_rgb(mix(theme::SERIES_1.r(), theme::SURFACE.r()), mix(theme::SERIES_1.g(), theme::SURFACE.g()), mix(theme::SERIES_1.b(), theme::SURFACE.b()))
}

#[test]
fn the_chart_climbing_descending_ended_and_long() {
    let Some(gpu) = gpu() else { return };

    // climbing, then level: Chromium's grid rows, labels, end dot and label
    let (image, placed) = shot(&gpu, &Props::new(climb()), |_| Vec::new(), None);
    save(&image, "u12-chart-climb.png");
    // the header 19.6 high (the export's strut), the plot 8 under it, 132 high
    assert_eq!(placed.header.min, Pos2::new(37.0, 37.0));
    assert!((placed.header.height() - 19.6).abs() < 0.04, "{:?}", placed.header);
    let plot = placed.plot.expect("the plot");
    assert!((plot.top() - 64.6).abs() < 1e-3 && plot.height() == 132.0 && plot.width() == 328.0, "{plot:?}");
    // the grid lines on Chromium's rows (the svg's origin on pixel 65), from x 83 to 357
    assert_eq!(rows_of(&image, 100, 70, 190, theme::LINE), [83, 115, 147, 179]);
    assert!(near(pixel(&image, 83, 147), theme::LINE, 2) && near(pixel(&image, 356, 179), theme::LINE, 2));
    assert!(near(pixel(&image, 82, 147), theme::SURFACE, 2) && near(pixel(&image, 357, 179), theme::SURFACE, 2));
    // the area's fill under the level line, the grid line through it
    assert!(near(pixel(&image, 300, 130), area_fill(), 2), "{:?}", pixel(&image, 300, 130));
    assert!(near(pixel(&image, 300, 108), area_fill(), 2));
    // the line 2 px over rows 101–102 where level (Chromium: 101 part, 102 full)
    assert!(near(pixel(&image, 300, 102), theme::SERIES_1, 12), "{:?}", pixel(&image, 300, 102));
    // the end dot at (357, 102): the series colour inside its surface ring
    assert!(near(pixel(&image, 357, 102), theme::SERIES_1, 4));
    assert!(near(pixel(&image, 357, 107), theme::SURFACE, 12), "the ring under the dot: {:?}", pixel(&image, 357, 107));
    // the tick labels: `45,000` ending at x 77, the times centred on 174, 254, 335
    let label = ink(&image, area(30, 76, 80, 92), theme::TEXT_3).expect("45,000");
    assert!((76..=78).contains(&label.x1), "{label:?}");
    let noon = ink(&image, area(150, 180, 200, 196), theme::TEXT_3).expect("12:00");
    assert!(((noon.x0 + noon.x1) as i32 - 2 * 174).abs() <= 2, "{noon:?}");
    // the end label `36,000 ft` in --text-2, ending near x 348, over the dot
    // (Chromium's ink: 289..345 × 87..94)
    let end = ink(&image, area(280, 80, 352, 96), theme::TEXT_2).expect("the end label");
    assert!((288..=290).contains(&end.x0) && (344..=346).contains(&end.x1) && (86..=88).contains(&end.y0), "{end:?}");
    // the header: `Altitude` from x 37, `Table` ending at 365, `Export` and `ARN time` before it
    let heading = ink(&image, area(36, 38, 110, 56), theme::TEXT_2).expect("the heading");
    assert!(heading.x0 <= 38, "{heading:?}");
    let toggle = placed.toggle.unwrap();
    assert!((toggle.right() - 365.0).abs() < 0.01 && (toggle.top() - 39.0).abs() < 1.0, "{toggle:?}");
    let export = placed.export.unwrap();
    assert!((export.right() - 324.13).abs() < 0.5 && (export.width() - 36.47).abs() < 0.5, "{export:?}");
    let zone = placed.zone.unwrap();
    assert!((zone.left() - 228.58).abs() < 0.6 && (zone.top() - 40.0).abs() < 1.0, "{zone:?}");

    // descending with room under the dot: the label under it
    let (image, _) = shot(&gpu, &Props::new(descend()), |_| Vec::new(), None);
    save(&image, "u12-chart-descend.png");
    let end = ink(&image, area(250, 120, 352, 150), theme::TEXT_2).expect("the end label under the dot");
    assert!(end.y0 > 130, "{end:?}");
    // near the ground: none
    let (image, _) = shot(&gpu, &Props::new(low()), |_| Vec::new(), None);
    save(&image, "u12-chart-low.png");
    assert!(ink(&image, area(250, 100, 352, 178), theme::TEXT_2).is_none(), "no end label");
    // five hours: two times, 04:00 and 06:00
    let (image, _) = shot(&gpu, &Props::new(long()), |_| Vec::new(), None);
    save(&image, "u12-chart-long.png");
    let four = ink(&image, area(170, 180, 216, 196), theme::TEXT_3).expect("04:00");
    assert!(((four.x0 + four.x1) as i32 - 2 * 193).abs() <= 2, "{four:?}");
    assert!(ink(&image, area(84, 180, 160, 196), theme::TEXT_3).is_none(), "02:00 would hang off the start");

    // ended, without the departure airport's zone: `Your time, UTC`, the dot and label dimmed
    let props = Props { zone: None, place: None, ended: true, ..Props::new(climb()) };
    let (image, placed) = shot(&gpu, &props, |_| Vec::new(), None);
    save(&image, "u12-chart-ended.png");
    assert!((placed.zone.unwrap().left() - 202.55).abs() < 0.6);
    assert!(near(pixel(&image, 357, 102), theme::TEXT_3, 4), "{:?}", pixel(&image, 357, 102));
    assert!(ink(&image, area(280, 80, 352, 96), theme::TEXT_2).is_none() && ink(&image, area(280, 80, 352, 96), theme::TEXT_3).is_some());

    // no trail: the message under the header
    let (image, placed) = shot(&gpu, &Props::new(Vec::new()), |_| Vec::new(), None);
    save(&image, "u12-chart-empty.png");
    assert!((placed.header.height() - 16.8).abs() < 0.04 && placed.zone.is_none() && placed.export.is_none());
    // (egui keeps a rectangle's edges on 1/32 px)
    assert!((placed.rect.height() - (16.8 + 8.0 + 33.6)).abs() < 0.04, "two lines: {:?}", placed.rect);
    assert!(ink(&image, area(36, 62, 365, 96), theme::TEXT_3).is_some());
}

#[test]
fn reading_a_point_with_the_pointer_and_the_keyboard() {
    let Some(gpu) = gpu() else { return };
    let props = Props::new(climb());
    // 120 px into the plot: 11:56 at 22,400 ft, the box 10 px right of the line
    let (image, placed) = shot(&gpu, &props, |p| vec![move_to(p.plot.unwrap().min + Vec2::new(120.0, 60.0))], None);
    save(&image, "u12-chart-hover.png");
    let tip = placed.tip.expect("the box");
    assert!((tip.left() - 167.3).abs() < 0.05 && (tip.top() - 64.6).abs() < 1e-3, "{tip:?}");
    assert!((tip.width() - 128.45).abs() < 1.5 && (tip.height() - 44.6).abs() < 0.05, "{tip:?}");
    // its border on Chromium's pixels (167, 65), its fill --surface-raised
    assert!(near(pixel(&image, 167, 80), theme::LINE, 2) && near(pixel(&image, 200, 65), theme::LINE, 2));
    assert!(near(pixel(&image, 170, 100), theme::SURFACE_RAISED, 2));
    // the key, 10 × 2 at (176, 78)
    assert!(near(pixel(&image, 180, 78), theme::SERIES_1, 4) && near(pixel(&image, 180, 79), theme::SERIES_1, 4));
    // the line down the plot at x 157.3, the dot at (157, 131), no end label
    assert!(pixel(&image, 157, 150)[0] > theme::SURFACE.r() + 20, "the crosshair: {:?}", pixel(&image, 157, 150));
    assert!(near(pixel(&image, 157, 131), theme::SERIES_1, 8));
    assert!(ink(&image, area(300, 80, 352, 96), theme::TEXT_2).is_none());
    // past the middle the box flips to the left of the line
    let (image, placed) = shot(&gpu, &props, |p| vec![move_to(p.plot.unwrap().min + Vec2::new(260.0, 60.0))], None);
    save(&image, "u12-chart-hover-flip.png");
    let tip = placed.tip.unwrap();
    assert!((tip.right() - (37.0 + 259.627 - 10.0)).abs() < 0.05, "{tip:?}");
    // the plot with the keyboard (Tab), Left twice: the third point from the end, flipped
    let (image, placed) = shot(&gpu, &props, |_| vec![press(Key::Tab), press(Key::Tab), press(Key::Tab), press(Key::ArrowLeft), press(Key::ArrowLeft)], None);
    save(&image, "u12-chart-focus-plot.png");
    let tip = placed.tip.expect("the box");
    assert!((tip.right() - (37.0 + 310.712 - 10.0)).abs() < 0.05, "{tip:?}");
    // the ring 2 px outside the plot (35..367 × 62.6..198.6), 2 px wide
    assert!(near(pixel(&image, 34, 130), theme::AMBER, 8), "{:?}", pixel(&image, 34, 130));
}

#[test]
fn the_table() {
    let Some(gpu) = gpu() else { return };
    let (image, placed) = shot(&gpu, &Props::new(climb()), |p| vec![click(p.toggle.unwrap().center())], None);
    save(&image, "u12-chart-table.png");
    let table = placed.table.expect("the table");
    assert!((table.top() - 64.6).abs() < 1e-3 && table.height() == 180.0, "{table:?}");
    // the rules on Chromium's rows
    assert_eq!(rows_of(&image, 110, 60, 245, theme::LINE), [85, 107, 129, 151, 173, 194, 216, 238]);
    // `Chart` now ends the header; the columns: times from x 37, altitudes ending at 266.6,
    // speeds at 365
    assert!((placed.toggle.unwrap().width() - 30.44).abs() < 0.5);
    // (Chromium's ink: `11:43` 38..72 × 91..99, `0 ft` 242..263, `180 kt` 323..362, `Altitude`
    // 223..266 × 68..76)
    // (the aligned edge and the top: the other edge depends on how heavy the glyphs' edges are)
    let start = |r: Px, x0: u32, y0: u32| r.x0.abs_diff(x0) <= 1 && r.y0.abs_diff(y0) <= 1;
    let end = |r: Px, x1: u32, y0: u32| r.x1.abs_diff(x1) <= 1 && r.y0.abs_diff(y0) <= 1;
    let time = ink(&image, area(36, 88, 100, 105), theme::TEXT_1).expect("11:43");
    assert!(start(time, 38, 91), "{time:?}");
    let alt = ink(&image, area(120, 88, 280, 105), theme::TEXT_1).expect("0 ft");
    assert!(end(alt, 263, 91), "{alt:?}");
    let speed = ink(&image, area(280, 88, 370, 105), theme::TEXT_1).expect("180 kt");
    assert!(end(speed, 362, 91), "{speed:?}");
    let header = ink(&image, area(120, 64, 280, 82), theme::TEXT_3).expect("Altitude");
    assert!(end(header, 266, 68), "{header:?}");
    // five hours: the same 31 rows
    let (image, _) = shot(&gpu, &Props::new(long()), |p| vec![click(p.toggle.unwrap().center())], None);
    save(&image, "u12-chart-table-long.png");
    assert_eq!(rows_of(&image, 110, 60, 245, theme::LINE), [85, 107, 129, 151, 173, 194, 216, 238]);
}

#[test]
fn the_export_menu_open_by_pointer_and_by_keys_and_saved() {
    let Some(gpu) = gpu() else { return };
    let props = Props::new(climb());
    let (image, placed) = shot(&gpu, &props, |p| vec![click(p.export.unwrap().center()), Vec::new()], None);
    save(&image, "u12-chart-menu.png");
    // the box: 250 × 289.3 at (74.13, 64.6): its border on Chromium's pixels
    let menu = placed.menu.expect("the menu");
    assert!((menu.left() - 74.13).abs() < 0.5 && (menu.top() - 64.6).abs() < 0.01 && menu.width() == 250.0, "{menu:?}");
    assert!((menu.height() - 289.33).abs() < 0.1, "{menu:?}");
    // its border on Chromium's columns 74 and 323, rows 65 and 353, `--surface-raised` inside
    for (x, y) in [(74, 200), (323, 200), (200, 65), (200, 353)] {
        assert!(near(pixel(&image, x, y), theme::LINE, 1), "the border at ({x}, {y}): {:?}", pixel(&image, x, y));
    }
    for (x, y) in [(73, 200), (324, 200), (200, 64), (200, 354)] {
        assert!(near(pixel(&image, x, y), theme::SURFACE, 1), "outside at ({x}, {y})");
    }
    assert!(near(pixel(&image, 75, 200), theme::SURFACE_RAISED, 1) && near(pixel(&image, 200, 352), theme::SURFACE_RAISED, 1));
    // the trigger `--text-1` while open; KML has the keyboard, without a ring after a click
    assert!(ink(&image, area(286, 38, 326, 56), theme::TEXT_1).is_some());
    assert!(!near(pixel(&image, 100, 100), theme::SURFACE_HOVER, 2), "no highlight");
    assert!(bounds(&image, area(76, 67, 322, 351), |p| near(p, theme::AMBER, 8)).is_none(), "no ring");
    // the labels and the note's rule
    assert!(ink(&image, area(86, 78, 120, 96), theme::TEXT_1).is_some(), "KML");
    assert_eq!(rows_of(&image, 150, 300, 352, theme::LINE), [319]);

    // opened with Enter on the trigger (Tab to it), Down to GPX: its highlight and ring
    let (image, _) = shot(&gpu, &props, |_| vec![press(Key::Tab), press(Key::Enter), Vec::new(), press(Key::ArrowDown), Vec::new()], None);
    save(&image, "u12-chart-menu-keys.png");
    assert!(near(pixel(&image, 100, 170), theme::SURFACE_HOVER, 2), "GPX highlighted: {:?}", pixel(&image, 100, 170));
    assert!(near(pixel(&image, 100, 100), theme::SURFACE_RAISED, 2), "KML not");
    assert!(near(pixel(&image, 200, 136), theme::AMBER, 8) && near(pixel(&image, 75, 160), theme::AMBER, 8), "the ring over GPX: {:?}", pixel(&image, 200, 136));

    // KML chosen: written, the menu closed, `Saved` on the trigger
    let dir = temp_dir("shot");
    let feeds = Downloads { dir: Some(dir.clone()), asked: Arc::default() };
    let (image, placed) = shot(
        &gpu,
        &props,
        |p| {
            let kml = Pos2::new(150.0, 100.0);
            vec![click(p.export.unwrap().center()), Vec::new(), click(kml), Vec::new(), move_to(Pos2::new(5.0, 430.0))]
        },
        Some(Box::new(feeds)),
    );
    save(&image, "u12-chart-saved.png");
    assert!(dir.join("SAS1417 ARN-CPH 2026-06-21.kml").exists());
    let export = placed.export.unwrap();
    assert!((export.width() - 32.98).abs() < 0.5 && (export.right() - 324.13).abs() < 0.5, "`Saved`: {export:?}");
    assert!(near(pixel(&image, 100, 100), theme::SURFACE, 2), "the menu closed");
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------------------------------------
// behaviour, through egui alone

/// [`pass`] without a picture: the textures egui made are let go.
fn step(ctx: &egui::Context, time: f64, props: &Props, events: Vec<Event>) -> (Placed, Vec<Event>) {
    let (placed, left, mut output) = pass(ctx, time, props, events);
    output.textures_delta.clear();
    (placed, left)
}

/// A context with the chart drawn twice (an area is sized on its first pass), and `feeds`.
fn ready(props: &Props, feeds: Option<Box<dyn ExportFeeds>>) -> (egui::Context, Placed) {
    let ctx = context();
    if let Some(feeds) = feeds {
        export_menu::install_feeds(&ctx, feeds);
    }
    step(&ctx, 0.0, props, Vec::new());
    let (placed, _) = step(&ctx, 0.5, props, Vec::new());
    (ctx, placed)
}

fn open(ctx: &egui::Context) -> bool {
    export_menu::menu(ctx).lock().unwrap().open
}

fn focused(ctx: &egui::Context) -> Option<Id> {
    ctx.memory(|m| m.focused())
}

#[test]
fn the_pointer_reads_the_nearest_point_and_leaving_clears_it() {
    let props = Props::new(climb());
    let (ctx, placed) = ready(&props, None);
    let plot = placed.plot.unwrap();
    let (p, _) = step(&ctx, 1.0, &props, move_to(plot.min + Vec2::new(120.0, 60.0)));
    let tip = p.tip.expect("a point read");
    assert!((tip.left() - (plot.left() + 120.305 + 10.0)).abs() < 0.01, "the 16th point: {tip:?}");
    // resting, it stays
    let (p, _) = step(&ctx, 1.1, &props, Vec::new());
    assert!(p.tip.is_some());
    // leaving the plot clears it
    let (p, _) = step(&ctx, 1.2, &props, move_to(Pos2::new(200.0, 300.0)));
    assert!(p.tip.is_none());
    // the pointer's time is held to the span: left of the plot's margin reads the first point
    let (p, _) = step(&ctx, 1.3, &props, move_to(plot.min + Vec2::new(10.0, 60.0)));
    assert!((p.tip.unwrap().left() - (plot.left() + 46.0 + 10.0)).abs() < 0.01);
}

#[test]
fn the_plot_s_keys_step_through_the_points() {
    let props = Props::new(climb());
    let (ctx, placed) = ready(&props, None);
    let plot = placed.plot.unwrap();
    let x_of = |i: usize| plot.left() + 46.0 + i as f32 * 274.0 / 59.0;
    // Tab: the zone note is no stop; the toggle and the export, then the plot
    let mut t = 1.0;
    let mut reached = None;
    for _ in 0..4 {
        let (p, _) = step(&ctx, t, &props, press(Key::Tab));
        t += 0.1;
        if p.tip.is_some() {
            reached = Some(p);
            break;
        }
    }
    let p = reached.expect("the plot takes the keyboard");
    // focus reads the last point (flipped: past the middle)
    assert!((p.tip.unwrap().right() - (x_of(59) - 10.0)).abs() < 0.05);
    let (p, _) = step(&ctx, t, &props, press(Key::ArrowLeft));
    assert!((p.tip.unwrap().right() - (x_of(58) - 10.0)).abs() < 0.05);
    // Shift: a tenth of the points
    let (p, _) = step(&ctx, t + 0.1, &props, vec![key(Key::ArrowLeft, Modifiers::SHIFT)]);
    assert!((p.tip.unwrap().right() - (x_of(52) - 10.0)).abs() < 0.05);
    let (p, left) = step(&ctx, t + 0.2, &props, press(Key::Home));
    assert!((p.tip.unwrap().left() - (x_of(0) + 10.0)).abs() < 0.05);
    assert!(left.is_empty(), "the keys go no further: {left:?}");
    let (p, _) = step(&ctx, t + 0.3, &props, press(Key::End));
    assert!((p.tip.unwrap().right() - (x_of(59) - 10.0)).abs() < 0.05);
    // the keyboard leaving clears it
    let (p, _) = step(&ctx, t + 0.4, &props, press(Key::Tab));
    assert!(p.tip.is_none());
}

#[test]
fn the_toggle_shows_the_table_and_the_chart_starts_afresh_when_drawn_again() {
    let props = Props::new(climb());
    let (ctx, placed) = ready(&props, None);
    let (p, _) = step(&ctx, 1.0, &props, click(placed.toggle.unwrap().center()));
    assert!(p.table.is_some(), "the click's pass draws the table");
    let (p, _) = step(&ctx, 1.1, &props, Vec::new());
    assert!(p.table.is_some() && p.plot.is_none());
    assert!((p.rect.height() - (19.6 + 8.0 + 180.0)).abs() < 0.04, "{:?}", p.rect);
    // a pass without the chart (another tab, the panel closed): the table is forgotten
    let raw = RawInput { time: Some(1.2), ..Default::default() };
    let _ = ctx.run_ui(raw, |_| {});
    let (p, _) = step(&ctx, 1.3, &props, Vec::new());
    assert!(p.table.is_none() && p.plot.is_some());
}

#[test]
fn the_menu_opens_cycles_and_closes() {
    let props = Props::new(climb());
    let (ctx, placed) = ready(&props, None);
    let trigger = placed.export.unwrap().center();
    step(&ctx, 1.0, &props, click(trigger));
    assert!(open(&ctx));
    assert_eq!(focused(&ctx), Some(export_menu::trigger_id()), "the clicked button");
    // the first item takes the keyboard at the next pass
    step(&ctx, 1.1, &props, Vec::new());
    assert_eq!(focused(&ctx), Some(export_menu::item_id(0)));
    // Down, Down, Up: the second; Up twice wraps to the last
    for k in [Key::ArrowDown, Key::ArrowDown, Key::ArrowUp] {
        step(&ctx, 1.2, &props, press(k));
    }
    assert_eq!(focused(&ctx), Some(export_menu::item_id(1)));
    step(&ctx, 1.3, &props, press(Key::ArrowUp));
    let (_, left) = step(&ctx, 1.4, &props, press(Key::ArrowUp));
    assert_eq!(focused(&ctx), Some(export_menu::item_id(3)));
    assert!(left.is_empty(), "the arrows are the menu's");
    // Escape closes it, goes no further, and the trigger takes the keyboard
    let (_, left) = step(&ctx, 1.5, &props, press(Key::Escape));
    assert!(!open(&ctx));
    assert!(left.is_empty(), "the page's Escape does not see it: {left:?}");
    assert_eq!(focused(&ctx), Some(export_menu::trigger_id()));
    // with the menu closed, Escape goes on to the page
    let (_, left) = step(&ctx, 1.6, &props, press(Key::Escape));
    assert!(matches!(left[..], [Event::Key { key: Key::Escape, .. }]), "{left:?}");
    // a click outside closes it; the trigger toggles it
    step(&ctx, 2.0, &props, click(trigger));
    assert!(open(&ctx));
    step(&ctx, 2.1, &props, click(Pos2::new(60.0, 300.0)));
    assert!(!open(&ctx));
    step(&ctx, 2.2, &props, click(trigger));
    step(&ctx, 2.3, &props, click(trigger));
    assert!(!open(&ctx), "the trigger again closes it");
    // the keyboard leaving it (Tab past the last item) closes it
    step(&ctx, 2.4, &props, click(trigger));
    step(&ctx, 2.5, &props, Vec::new());
    for _ in 0..3 {
        step(&ctx, 2.6, &props, press(Key::ArrowDown));
    }
    assert_eq!(focused(&ctx), Some(export_menu::item_id(3)));
    step(&ctx, 2.7, &props, press(Key::Tab));
    step(&ctx, 2.8, &props, Vec::new());
    assert!(!open(&ctx), "focus left the menu");
}

#[test]
fn nothing_to_export_and_without_a_backend() {
    // one position: drawable needs two, so neither the tools nor the trigger show
    let props = Props::new(climb().into_iter().take(1).collect());
    let (_, placed) = ready(&props, None);
    assert!(placed.export.is_none() && placed.plot.is_none());
    // two positions without a place: the trigger enabled; without a backend the save fails
    let mut props = Props::new(climb());
    for p in &mut props.trail[2..] {
        p.latitude = f32::NAN;
    }
    let ctx = context();
    step(&ctx, 0.0, &props, Vec::new());
    let (placed, _) = step(&ctx, 0.5, &props, Vec::new());
    step(&ctx, 1.0, &props, click(placed.export.unwrap().center()));
    let (p, _) = step(&ctx, 1.1, &props, Vec::new());
    assert!(open(&ctx));
    step(&ctx, 1.2, &props, press(Key::Enter));
    assert!(export_menu::menu(&ctx).lock().unwrap().busy, "Saving…");
    step(&ctx, 1.3, &props, Vec::new());
    let menu = export_menu::menu(&ctx);
    let m = menu.lock().unwrap();
    assert!(!m.busy && !m.open);
    assert_eq!(m.label(), export_menu::NOT_SAVED);
    assert_eq!(m.status.as_ref().map(|s| s.text.as_str()), Some("Couldn't save the path: Not available: the app's backend didn't start."));
    assert_eq!(m.title(2), "Couldn't save the path: Not available: the app's backend didn't start.");
    let _ = p;
}

#[test]
fn export_writes_the_four_formats_to_downloads() {
    let dir = temp_dir("formats");
    let asked = Arc::new(Mutex::new(Vec::new()));
    let props = Props::new(climb());
    let (ctx, placed) = ready(&props, Some(Box::new(Downloads { dir: Some(dir.clone()), asked: asked.clone() })));
    let trigger = placed.export.unwrap().center();
    let mut t = 1.0;
    for (n, f) in EXPORT_FORMATS.iter().enumerate() {
        // open, the item's place (the menu under the trigger's block), chosen
        step(&ctx, t, &props, click(trigger));
        step(&ctx, t + 0.01, &props, Vec::new());
        for _ in 0..n {
            step(&ctx, t + 0.02, &props, press(Key::ArrowDown));
        }
        step(&ctx, t + 0.03, &props, press(Key::Enter));
        let (p, _) = step(&ctx, t + 0.04, &props, Vec::new());
        let file = dir.join(format!("SAS1417 ARN-CPH 2026-06-21.{}", f.format.as_str()));
        let written = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        assert_eq!(written, export_path(&props.trail, &props.meta, f.format), "{:?}", f.format);
        let m = export_menu::menu(&ctx);
        let m = m.lock().unwrap();
        assert_eq!(m.status.as_ref().map(|s| s.text.clone()), Some(format!("Saved SAS1417 ARN-CPH 2026-06-21.{}", f.format.as_str())));
        assert!(!m.open && !m.busy && m.label() == export_menu::SAVED, "{m:?}");
        drop(m);
        assert_eq!(focused(&ctx), Some(export_menu::trigger_id()), "the trigger takes the keyboard back");
        let _ = p;
        t += 1.0;
    }
    assert_eq!(asked.lock().unwrap().len(), 4);
    // a name taken is numbered, as the backend does
    step(&ctx, t, &props, click(trigger));
    step(&ctx, t + 0.01, &props, Vec::new());
    step(&ctx, t + 0.02, &props, press(Key::Enter));
    step(&ctx, t + 0.03, &props, Vec::new());
    assert!(dir.join("SAS1417 ARN-CPH 2026-06-21 (2).kml").exists());
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names.len(), 5, "{names:?}");
    // `Saved` for 5 s, then `Export` again
    let label = |ctx: &egui::Context| export_menu::menu(ctx).lock().unwrap().label();
    step(&ctx, t + 0.03 + 4.9, &props, Vec::new());
    assert_eq!(label(&ctx), export_menu::SAVED);
    step(&ctx, t + 0.03 + 5.01, &props, Vec::new());
    assert_eq!(label(&ctx), export_menu::EXPORT);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = ExportFormat::Kml;
}
