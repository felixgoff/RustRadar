//! Item U6's acceptance (plan stage 7; doc 06 §2.3, §2.8–2.10, §3.11, §3.13): the desktop shell
//! as the UI pass draws it.
//!
//! - `shell-desktop` at 1440 × 900 (`target/shots/u6-shell-desktop.png`): the sidebar at x 16–336
//!   from top 16, its cards 12 apart, the primary card's brand, the search slot and the Filters
//!   button where `docs/port/reference/dark-v1-1440x900.png` has them; the airport board's panel
//!   362 px wide (360 of content) at right 16, bottom 28; the map's ink in the gaps;
//! - with a list too long for the window and the filter panel open, the column ends 40 px above
//!   the bottom, the heights the solver gives (the list three times as fast as the panel);
//! - a panel enters over 200 ms from 8 px to the right and transparent;
//! - through the app over the map (`target/shots/u6-shell-v1.png`), and the keys through the
//!   window's frames: `c` locks the camera on the selected flight, `?` opens the help and turns the
//!   map's keys off, Escape closes it.
//!
//! The components inside the shell are later items': the canvas tests put stand-ins of today's
//! sizes in the slots (the search box 38 px high, TopFlights' loading list 269 px of content).
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::NOW_MS;
use common::shots::{Rect, bounds, gpu, pixel, save};
use egui::{Color32, Event, Key, Pos2, RawInput, Sense, Ui, Vec2};
use rustradar_app::app::Engine;
use rustradar_app::input::{KeyName, MapEvent};
use rustradar_app::state::flight::SelectOptions;
use rustradar_app::state::shortcuts::Open;
use rustradar_app::state::{AppState, Startup};
use rustradar_app::ui::shell::{self, Panel, Pass, Placed, Slots};
use rustradar_app::ui::sidebar::{self, Item};
use rustradar_app::ui::widgets::skeleton::Skeleton;
use rustradar_app::ui::{anim, theme};
use rustradar_core::geo::Basemap;
use rustradar_core::model::LiveFlight;
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_map::io::{self, DirTiles};
use rustradar_scene::camera::ViewState;

/// The desktop reference pictures' canvas, CSS px, at scale 1.
const SIZE: [u32; 2] = [1440, 900];

/// egui alone onto a target cleared to `--ink` (the map's space colour), as the window's UI pass
/// draws it.
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

    /// A frame of the shell for `state` with `slots` at egui time `time`.
    fn frame(&mut self, gpu: &Gpu, time: f64, state: &AppState, slots: &Slots) -> Placed {
        let raw = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(SIZE[0] as f32, SIZE[1] as f32))),
            time: Some(time),
            ..Default::default()
        };
        let mut placed = Placed::default();
        let mut actions = Vec::new();
        let output = self.ctx.run_ui(raw, |ui| placed = shell::show(ui.ctx(), state, NOW_MS, &mut actions, slots));
        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: self.target.size(), pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui.update_texture(device, queue, id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shell") });
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

    fn read(&self, gpu: &Gpu) -> Image {
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        self.target.read(gpu).expect("read back")
    }

    /// Frames at egui times 0–3 s (areas are sized on their first frame, the sidebar solves its
    /// heights from the last frame's measures), then the picture.
    fn settle(&mut self, gpu: &Gpu, state: &AppState, slots: &Slots) -> (Placed, Image) {
        let mut placed = Placed::default();
        for time in [0.0, 1.0, 2.0, 3.0] {
            placed = self.frame(gpu, time, state, slots);
        }
        (placed, self.read(gpu))
    }
}

// ------------------------------------------------------------------------------------------------
// stand-ins for the later items' components, at today's sizes

/// The search box's height on the desktop (`.search` 38 px: U4's `TextField` search height).
const SEARCH_HEIGHT: f32 = 38.0;
/// TopFlights' loading list in `dark-v1`: the list card 295 px high, 269 of content.
const LIST_HEIGHT: f32 = 269.0;
/// A list far taller than the window.
const TALL_LIST: f32 = 1200.0;
/// A filter panel's natural height: header and body.
const FILTERS_HEIGHT: f32 = 520.0;
/// A detail panel's content.
const PANEL_HEIGHT: f32 = 480.0;

/// The search field: `--surface-raised`, a `--line-control` border, radius 4, the card's width.
fn search(ui: &mut Ui, _: &mut Pass<'_>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), SEARCH_HEIGHT), Sense::hover());
    ui.painter().rect(rect, theme::RADIUS, theme::SURFACE_RAISED, egui::Stroke::new(1.0, theme::LINE_CONTROL), egui::StrokeKind::Inside);
}

/// Rows of skeletons `height` px high in all.
fn rows(ui: &mut Ui, height: f32) {
    let top = ui.cursor().min.y;
    ui.spacing_mut().item_spacing = Vec2::new(0.0, 0.0);
    while ui.cursor().min.y - top + 40.0 <= height {
        ui.allocate_ui(Vec2::new(ui.available_width(), 40.0), |ui| {
            ui.add_space(8.0);
            Skeleton::new(120.0, 12.0).show(ui);
        });
    }
    let left = height - (ui.cursor().min.y - top);
    if left > 0.0 {
        ui.allocate_exact_size(Vec2::new(ui.available_width(), left), Sense::hover());
    }
}

fn list(ui: &mut Ui, _: &mut Pass<'_>) {
    rows(ui, LIST_HEIGHT);
}

fn tall_list(ui: &mut Ui, _: &mut Pass<'_>) {
    rows(ui, TALL_LIST);
}

/// A filter panel: a 41 px header over a body that scrolls within what it is given.
fn filters(ui: &mut Ui, _: &mut Pass<'_>) -> f32 {
    let header = 41.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), header), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, egui::Stroke::new(1.0, theme::LINE));
    let body = (ui.max_rect().bottom() - ui.cursor().min.y).max(0.0);
    egui::ScrollArea::vertical().id_salt("stand-in filters").max_height(body).auto_shrink([false, false]).show(ui, |ui| rows(ui, FILTERS_HEIGHT - header));
    FILTERS_HEIGHT
}

fn panel(ui: &mut Ui, _: &mut Pass<'_>) {
    let height = PANEL_HEIGHT.min(ui.max_rect().height());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    ui.painter().text(rect.left_top() + Vec2::new(16.0, 16.0), egui::Align2::LEFT_TOP, "ARN", egui::FontId::proportional(22.0), theme::TEXT_1);
}

/// A panel taller than the window can hold: it takes what it is given.
fn tall_panel(ui: &mut Ui, _: &mut Pass<'_>) {
    let height = ui.max_rect().height();
    ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
}

fn slots() -> Slots {
    Slots { search: Some(search), aircraft_list: Some(list), airport_panel: Some(panel), flight_panel: Some(panel), ..Slots::APP }
}

// ------------------------------------------------------------------------------------------------
// checks

fn state() -> AppState {
    AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, ViewState::INITIAL, SIZE.map(f64::from))
}

fn rgb(c: Color32) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

#[track_caller]
fn near(got: f32, want: f32, tolerance: f32, what: &str) {
    assert!((got - want).abs() <= tolerance, "{what}: {got} for {want}");
}

/// The rectangle of pixels whose colour is within 2 levels of `c` in `area`.
fn painted(image: &Image, area: Rect, c: Color32) -> Option<Rect> {
    bounds(image, area, |p| (0..3).all(|i| p[i].abs_diff(rgb(c)[i]) <= 2))
}

fn lum(p: [u8; 4]) -> f64 {
    (f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])) / 3.0
}

/// The bounds of the pixels in `area` brighter than `under` by more than 30 levels: text and icons.
fn ink(image: &Image, area: Rect, under: Color32) -> Option<Rect> {
    bounds(image, area, |p| lum(p) - lum(rgb(under)) > 30.0)
}

/// A reference picture (8-bit RGB, or a palette) as RGBA.
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

#[test]
fn shell_desktop() {
    let Some(gpu) = gpu() else { return };
    let mut s = state();
    // the airport board open: its panel at the panels' place
    s.flight.set_focus_airport(Some("ARN".into()));
    let mut canvas = Canvas::new(&gpu);
    let (placed, image) = canvas.settle(&gpu, &s, &slots());
    save(&image, "u6-shell-desktop.png");

    // the primary card: x 16–336 from top 16, its border --line round --surface
    let primary = placed.sidebar.primary.expect("the primary card");
    assert_eq!((primary.left(), primary.top(), primary.right()), (16.0, 16.0, 336.0), "{primary:?}");
    let card = painted(&image, Rect::new(0, 0, 400, 400), theme::LINE).expect("its border");
    assert_eq!((card.x0, card.y0, card.x1), (16, 16, 336), "{card:?}");
    assert_eq!(pixel(&image, 100, 25), rgb(theme::SURFACE), "its fill");
    // brand 23.8, search 38, the toolbar's 32 px button, 12 apart, 17 in from the edges
    let content = 23.8 + 12.0 + SEARCH_HEIGHT + 12.0 + 32.0;
    near(primary.height(), content + 2.0 * (sidebar::PADDING + 1.0), 0.6, "the primary card's height");
    // the map in the gaps: right of the column and between its cards
    assert_eq!(pixel(&image, 340, 100), rgb(theme::INK));
    let gap = (primary.bottom() + 6.0) as u32;
    assert_eq!(pixel(&image, 100, gap), rgb(theme::INK), "between the cards");

    // the list card 12 below, its 269 px of rows in 12 × 8 of padding
    let list = placed.sidebar.list.expect("the list card");
    // (egui puts an area's corner on the pixel grid)
    near(list.top() - primary.bottom(), sidebar::GAP, 0.5, "the cards' gap");
    assert_eq!((list.left(), list.right()), (16.0, 336.0));
    near(list.height(), LIST_HEIGHT + 2.0 * (12.0 + 1.0), 0.5, "the list card");

    // the board's panel: 360 of content and a border each side, right 16, bottom 28
    let (kind, rect) = placed.panel.expect("a panel");
    assert_eq!(kind, Panel::Airport);
    near(rect.right(), 1440.0 - 16.0, 0.01, "the panel's right");
    near(rect.bottom(), 900.0 - 28.0, 0.01, "the panel's bottom");
    near(rect.width(), shell::PANEL_WIDTH + 2.0, 0.01, "the panel's width");
    near(rect.height(), PANEL_HEIGHT + 2.0, 0.01, "the panel's height");
    let border = painted(&image, Rect::new(1000, 300, 440, 600), theme::LINE).expect("the panel's border");
    assert_eq!((border.x0, border.x1, border.y1), (1062, 1424, 872), "{border:?}");
    assert_eq!(pixel(&image, 1200, 700), rgb(theme::SURFACE), "the panel's fill");
    assert_eq!(pixel(&image, 1430, 700), rgb(theme::INK), "right of the panel");
    assert_eq!(pixel(&image, 1200, 880), rgb(theme::INK), "below the panel");
    assert_eq!(pixel(&image, 700, 450), rgb(theme::INK), "between the sidebar and the panel");

    // where today's app has them (dark-v1): the wordmark and the Filters button's ink, and the
    // search box's border (today's sits 4.5 px higher than deck's placement: the pictures' own)
    let today = reference("dark-v1-1440x900.png");
    let brand = Rect::new(30, 28, 160, 32);
    let (ours, theirs) = (ink(&image, brand, theme::SURFACE).expect("our brand"), ink(&today, brand, theme::SURFACE).expect("today's brand"));
    for (a, b, what) in [(ours.x0, theirs.x0, "left"), (ours.x1, theirs.x1, "right"), (ours.y0, theirs.y0, "top"), (ours.y1, theirs.y1, "bottom")] {
        assert!(a.abs_diff(b) <= 1, "the brand's ink {what}: {ours:?} today {theirs:?}");
    }
    let filters = Rect::new(36, 122, 80, 26);
    let (ours, theirs) = (ink(&image, filters, theme::SURFACE).expect("our Filters"), ink(&today, filters, theme::SURFACE).expect("today's Filters"));
    for (a, b, what) in [(ours.x0, theirs.x0, "left"), (ours.x1, theirs.x1, "right"), (ours.y0, theirs.y0, "top"), (ours.y1, theirs.y1, "bottom")] {
        assert!(a.abs_diff(b) <= 1, "the Filters button's ink {what}: {ours:?} today {theirs:?}");
    }
    let search_top = |image: &Image| (60..80).find(|&y| lum(pixel(image, 150, y)) > 90.0).expect("the search box's border");
    assert!(search_top(&image).abs_diff(search_top(&today)) <= 1, "the search box: {} today {}", search_top(&image), search_top(&today));
}

#[test]
fn the_column_ends_40_above_the_bottom_and_the_list_gives_way_first() {
    let Some(gpu) = gpu() else { return };
    let mut s = state();
    let tall = Slots { search: Some(search), aircraft_list: Some(tall_list), aircraft_filters: Some(filters), ..Slots::APP };
    let mut canvas = Canvas::new(&gpu);
    // the list alone: it fills the column down to 40 px above the bottom and scrolls
    let (placed, image) = canvas.settle(&gpu, &s, &tall);
    let list = placed.sidebar.list.expect("the list");
    near(list.bottom(), 900.0 - sidebar::BOTTOM, 0.5, "the column's bottom");
    assert_eq!(pixel(&image, 100, 870), rgb(theme::INK), "the map under the column");

    // the filter panel open as well: the heights CSS gives, the column still ending at 860
    assert!(s.apply(rustradar_app::state::Action::ToggleOpen(Open::Filters), NOW_MS));
    let (placed, image) = canvas.settle(&gpu, &s, &tall);
    save(&image, "u6-shell-filters.png");
    let primary = placed.sidebar.primary.expect("the primary card");
    let panel = placed.sidebar.filters.expect("the filter panel");
    let list = placed.sidebar.list.expect("the list");
    let want = sidebar::column(
        &[
            Item::fixed(primary.height()),
            Item { base: FILTERS_HEIGHT, edges: 2.0, shrink: sidebar::FILTERS_SHRINK, min: 0.0 },
            Item { base: TALL_LIST, edges: 26.0, shrink: sidebar::LIST_SHRINK, min: sidebar::LIST_MIN_HEIGHT },
        ],
        900.0 - sidebar::TOP - sidebar::BOTTOM,
        sidebar::GAP,
    );
    near(panel.top(), primary.bottom() + sidebar::GAP, 0.5, "the panel's top");
    near(panel.height(), want[1], 0.5, "the panel's height");
    near(list.top(), panel.bottom() + sidebar::GAP, 0.5, "the list's top");
    near(list.height(), want[2], 0.5, "the list's height");
    near(list.bottom(), 900.0 - sidebar::BOTTOM, 0.5, "the column's bottom");
    // the list gave up far more: of 3 × 1200 against 1 × 520 of base size
    let (gave_panel, gave_list) = (FILTERS_HEIGHT + 2.0 - want[1], TALL_LIST + 26.0 - want[2]);
    assert!(gave_list > 3.0 * gave_panel, "the list {gave_list}, the panel {gave_panel}");
}

#[test]
fn a_panel_enters_from_8_px_to_the_right() {
    let Some(gpu) = gpu() else { return };
    let mut s = state();
    let mut canvas = Canvas::new(&gpu);
    let slots = slots();
    canvas.frame(&gpu, 0.0, &s, &slots);
    canvas.frame(&gpu, 0.5, &s, &slots);
    // the board opens at 1 s: its first frame sizes it, the next shows it moving in
    s.flight.set_focus_airport(Some("ARN".into()));
    let mut seen = Vec::new();
    for time in [1.0, 1.04, 1.08, 1.2, 1.5] {
        let placed = canvas.frame(&gpu, time, &s, &slots);
        let image = canvas.read(&gpu);
        let (_, rect) = placed.panel.expect("the panel");
        seen.push((time, rect.right(), lum(pixel(&image, 1200, 700))));
    }
    let surface = lum(rgb(theme::SURFACE));
    let ink_ = lum(rgb(theme::INK));
    for &(time, right, fill) in &seen[1..] {
        let t = anim::ease_out(((time - 1.0) / 0.2).min(1.0) as f32);
        near(right, 1424.0 + 8.0 * (1.0 - t), 0.5, &format!("the right edge at {time}"));
        let want = ink_ + (surface - ink_) * f64::from(t);
        assert!((fill - want).abs() <= 1.5, "the fill at {time}: {fill} for {want} ({seen:?})");
    }
    assert!(seen[1].1 > 1424.5 && seen[1].2 < surface - 1.0, "moving in at 1.04 s: {seen:?}");
    // closed and opened again: it enters again
    s.flight.set_focus_airport(None);
    canvas.frame(&gpu, 2.0, &s, &slots);
    s.flight.set_focus_airport(Some("ARN".into()));
    canvas.frame(&gpu, 2.5, &s, &slots);
    let placed = canvas.frame(&gpu, 2.54, &s, &slots);
    assert!(placed.panel.expect("the panel").1.right() > 1424.5, "entering again");
}

#[test]
fn a_tall_panel_stops_16_from_the_top() {
    let Some(gpu) = gpu() else { return };
    let mut s = state();
    s.flight.set_focus_airport(Some("ARN".into()));
    let mut canvas = Canvas::new(&gpu);
    let (placed, _) = canvas.settle(&gpu, &s, &Slots { airport_panel: Some(tall_panel), ..Slots::APP });
    let (_, rect) = placed.panel.expect("the panel");
    // `max-height: calc(100% - 16px - 28px)` on its content box: 856 of content and the border
    near(rect.bottom(), 872.0, 0.01, "its bottom");
    near(rect.height(), 900.0 - 16.0 - 28.0 + 2.0, 0.01, "its height");
}

/// With the synthetic snapshot in: the status says its age, the toolbar counts it (`N of 236
/// worldwide` with a filter on), the Filters button wears the pill, pressed while the panel is open.
#[test]
fn a_snapshots_status_counts_and_filter_pill() {
    let Some(gpu) = gpu() else { return };
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("u6-synthetic");
    common::fixtures::write_fixture(&dir, &common::fixtures::synthetic_snapshot(NOW_MS)).expect("the fixture folder");
    let startup = Startup { basemap: Some(Basemap::Dark), fixture: Some(dir), demo: false };
    let mut s = AppState::new(startup, None, None, None, ViewState::INITIAL, SIZE.map(f64::from));
    s.persisted.filters.text = "1".into();
    assert!(s.apply(rustradar_app::state::Action::ToggleOpen(Open::Filters), NOW_MS));
    for t in [NOW_MS, NOW_MS + 16.0, NOW_MS + 32.0] {
        s.poll(t);
    }
    let total = s.aircraft.flights().len();
    assert!(total > 200 && s.aircraft.filtered_len() < total, "{} of {total}", s.aircraft.filtered_len());
    let mut canvas = Canvas::new(&gpu);
    let (placed, image) = canvas.settle(&gpu, &s, &slots());
    save(&image, "u6-shell-snapshot.png");
    let primary = placed.sidebar.primary.expect("the primary card");
    // the toolbar's row: the pressed button (--amber border) with the pill (--amber fill), the
    // counts' ink on the right, within the card's padding
    let row = Rect::new(33, 119, 286, 32);
    let button = bounds(&image, row, |p| (0..3).all(|i| p[i].abs_diff(rgb(theme::AMBER)[i]) <= 3)).expect("the pressed button");
    assert_eq!((button.x0, button.y0, button.y1), (33, 119, 151), "{button:?}");
    let counts = ink(&image, Rect::new(button.x1 + 4, 119, 319 - button.x1 - 4, 32), theme::SURFACE).expect("the counts");
    assert!(counts.x1 >= 317 && counts.x1 <= 319, "right-aligned at the padding: {counts:?}");
    assert!(counts.y0 >= 125 && counts.y1 <= 146, "centred on the row: {counts:?}");
    // the status: text at the brand row's right, not the skeleton
    let status = ink(&image, Rect::new(180, 33, 139, 24), theme::SURFACE).expect("the status");
    assert!(status.x1 >= 317 && status.x1 <= 319, "{status:?}");
    near(primary.height(), 23.8 + 12.0 + SEARCH_HEIGHT + 12.0 + 32.0 + 34.0, 0.6, "no alert");
}

// ------------------------------------------------------------------------------------------------
// through the app

/// A flight near Arlanda for the selection.
fn flight(id: u32) -> LiveFlight {
    LiveFlight { id, lat: 59.7, lon: 18.1, track: 90, alt: 12_000, speed: 250, callsign: "SAS123".into(), timestamp_ms: NOW_MS as u64 - 1000, ..Default::default() }
}

#[test]
fn the_shell_over_the_map() {
    let Some(gpu) = gpu() else { return };
    let mut w = Window::new(&gpu, state());
    w.engine.map.set_tiles(Arc::new(DirTiles::fixtures()), None);
    let image = w.settle(&gpu, NOW_MS);
    save(&image, "u6-shell-v1.png");
    let card = painted(&image, Rect::new(0, 0, 400, 400), theme::LINE).expect("the primary card's border");
    assert_eq!((card.x0, card.y0, card.x1), (16, 16, 336), "{card:?}");
    assert_eq!(pixel(&image, 100, 25), rgb(theme::SURFACE), "its fill");
    // the brand: the radar in --amber
    let amber = bounds(&image, Rect::new(30, 30, 30, 30), |p| p[2] > 200 && p[0] < 120).expect("the radar icon");
    assert!(amber.x0 >= 33 && amber.x1 <= 54, "{amber:?}");
    // the globe beside it is the map's
    assert!(lum(pixel(&image, 720, 450)) > lum(rgb(theme::INK)) + 4.0, "the globe at the centre");
    // the pointer over the card is the UI's, over the map the map's
    assert!(!w.engine.egui.is_pointer_over_egui());
}

/// The window's frames, drawn offscreen, with egui's key events.
struct Window {
    engine: Engine,
    target: Offscreen,
    egui_s: f64,
}

impl Window {
    fn new(gpu: &Gpu, state: AppState) -> Window {
        let mut engine = Engine::new(gpu, FORMAT, state);
        engine.map.set_tiles(io::offline(), None);
        // the window's UI: the shell and the keys
        engine.shell = true;
        engine.draw = false;
        Window { engine, target: Offscreen::new(gpu, SIZE), egui_s: 0.0 }
    }

    /// Frames at `now` until nothing is loading, then a drawn one: its picture.
    fn settle(&mut self, gpu: &Gpu, now: f64) -> Image {
        let started = std::time::Instant::now();
        loop {
            self.frame(gpu, Vec::new(), now);
            if !self.engine.map.pending() || started.elapsed() > rustradar_map::headless::MAX_WAIT {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        self.frame(gpu, Vec::new(), now);
        self.engine.draw = true;
        self.frame(gpu, Vec::new(), now);
        self.engine.draw = false;
        self.target.read(gpu).expect("read back")
    }

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

    /// A key pressed (and the text it typed) in the frame at `now`.
    fn key(&mut self, gpu: &Gpu, key: Key, text: Option<&str>, shift: bool, now: f64) {
        let modifiers = egui::Modifiers { shift, ..Default::default() };
        let mut events = vec![Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }];
        events.extend(text.map(|t| Event::Text(t.into())));
        self.frame(gpu, events, now);
    }
}

#[test]
fn the_keys_through_the_window() {
    let Some(gpu) = gpu() else { return };
    let mut state = state();
    state.select_flight(Some(42), SelectOptions { hint: Some(flight(42)), ..Default::default() }, NOW_MS);
    let mut w = Window::new(&gpu, state);
    w.frame(&gpu, Vec::new(), NOW_MS);
    assert!(!w.engine.state.controller.locked());

    // c: the camera follows the selected flight, locked on it from the next frame
    w.key(&gpu, Key::C, Some("c"), false, NOW_MS + 100.0);
    assert!(w.engine.state.flight.following());
    w.frame(&gpu, Vec::new(), NOW_MS + 200.0);
    assert!(w.engine.state.camera.following());
    assert!(w.engine.state.controller.locked(), "the lock");
    w.key(&gpu, Key::C, Some("c"), false, NOW_MS + 300.0);
    w.frame(&gpu, Vec::new(), NOW_MS + 400.0);
    assert!(!w.engine.state.flight.following());
    assert!(!w.engine.state.controller.locked(), "let go");

    // ?: the help; the map's own keys are off while it is open
    w.key(&gpu, Key::Questionmark, Some("?"), true, NOW_MS + 500.0);
    assert!(w.engine.state.shortcuts.help_open());
    let tilt = MapEvent::Key { key: KeyName::Char('t'), pressed: true };
    let e = &mut w.engine;
    assert!(!e.input.handle(tilt, false, &mut e.state.controller, NOW_MS + 600.0), "t does nothing under the help");
    // f does nothing either; Escape closes the help
    w.key(&gpu, Key::F, Some("f"), false, NOW_MS + 700.0);
    assert!(!w.engine.state.shortcuts.filters_open());
    w.key(&gpu, Key::Escape, None, false, NOW_MS + 800.0);
    assert!(!w.engine.state.shortcuts.help_open());
    let e = &mut w.engine;
    assert!(e.input.handle(tilt, false, &mut e.state.controller, NOW_MS + 900.0), "t tilts again");

    // f opens the filters; 2 shows the satellite map; Escape closes the flight, then the filters
    w.key(&gpu, Key::F, Some("f"), false, NOW_MS + 1000.0);
    assert!(w.engine.state.shortcuts.filters_open());
    w.key(&gpu, Key::Num2, Some("2"), false, NOW_MS + 1100.0);
    assert_eq!(w.engine.state.basemap(), Basemap::Satellite);
    w.key(&gpu, Key::Escape, None, false, NOW_MS + 1200.0);
    assert_eq!(w.engine.state.flight.selected_id(), None);
    assert!(w.engine.state.shortcuts.filters_open());
    w.key(&gpu, Key::Escape, None, false, NOW_MS + 1300.0);
    assert!(!w.engine.state.shortcuts.filters_open());
    assert!(!w.engine.state.closable());
}

/// A click on the Filters button through the window's frames opens the filter panel; the pointer
/// over the card is egui's (the map takes no gesture there), off it the map's.
#[test]
fn the_filters_button_takes_a_click() {
    let Some(gpu) = gpu() else { return };
    let mut w = Window::new(&gpu, state());
    // the app's own slots: the search box (U8) 12 below the brand, the toolbar 12 below it
    // (y 119–151)
    let at = Pos2::new(76.0, 135.0);
    let button = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
    w.frame(&gpu, Vec::new(), NOW_MS);
    w.frame(&gpu, vec![Event::PointerMoved(at)], NOW_MS + 16.0);
    w.frame(&gpu, vec![Event::PointerMoved(at)], NOW_MS + 32.0);
    assert!(w.engine.egui.is_pointer_over_egui(), "over the card");
    w.frame(&gpu, vec![button(true)], NOW_MS + 48.0);
    w.frame(&gpu, vec![button(false)], NOW_MS + 64.0);
    assert!(w.engine.state.shortcuts.filters_open(), "the Filters button toggled the panel");
    // between the cards and beside the column the pointer is the map's
    w.frame(&gpu, vec![Event::PointerMoved(Pos2::new(600.0, 400.0))], NOW_MS + 80.0);
    w.frame(&gpu, Vec::new(), NOW_MS + 96.0);
    assert!(!w.engine.egui.is_pointer_over_egui(), "over the map");
}
