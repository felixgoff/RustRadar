//! Item U15's acceptance (plan stage 7; doc 06 §8.2, §13, §16, §24.5): the ships list, the ship
//! filter panel and the ship panel as the shell draws them with the app's slots, from a ship
//! fixture at V6 (Gothenburg harbour, zoom 13): ten ships of every type colour and an unlisted
//! type, large to small, moored and under way, one old, one unnamed, and VesselFinder's cards for
//! two of them, answered by canned feeds as the `--fixture` folder's files would be.
//!
//! - `u15-list.png`: the list sorted by size with a ship selected (and its panel), the lengths,
//!   the size classes, the old position's badge, the unnamed ship's MMSI;
//! - `u15-list-distance.png`, `u15-list-name.png`: sorted by distance; by name zoomed out, where
//!   the feed does not tell sizes apart;
//! - `u15-list-loading.png`, `u15-list-failed.png`, `u15-list-nomatch.png`: the placeholders,
//!   VesselFinder unreachable, none matching with Show all;
//! - `u15-filters.png`, `u15-filters-out.png`: the filter panel with a type, a size, the movement
//!   and old positions set; zoomed out with the sizes disabled under their hint;
//! - `u15-panel.png`, `u15-panel-free.png`, `u15-panel-loading.png`, `u15-panel-error.png`,
//!   `u15-panel-unnamed.png`: the panel with its card and photo, followed; a free-text
//!   destination, an ETA passed, an alarming status, SAR, satellite AIS and a stale report;
//!   loading; the card failed, the position old; an unnamed ship of an unknown type loading;
//! - `u15-window-v6.png`: the fixture at V6 on the satellite map through the window's engine, a
//!   ship's panel open and a row under the pointer highlighting its ship on the map.
//!
//! The Chromium layouts were made from `ShipList.svelte`, `ShipFilterPanel.svelte` and
//! `ShipPanel.svelte` server-rendered by Svelte with the same props, laid out with app.css and the
//! Overpass fonts (Playwright's Chromium, scale 1, the photo served from `u15-photo.png`); their
//! numbers are written into the checks. The detail panel stands where today's does (right 16,
//! bottom 28), so its numbers are the page's; the sidebar's cards stand lower than the bare
//! components did, so the list's and the filters' are taken from their cards' tops.
//!
//! The interaction tests drive the parts through egui and the state: a row's click picks its ship
//! and its hover reaches the ship builder, the sort, Show all, the chips, the movement, Reset and
//! close, the panel's close, Follow and Try again.
//!
//! The GPU tests need a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without
//! one they print `skipped: …` and pass.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use common::NOW_MS;
use common::shots::{Rect, bounds, gpu, pixel, save};
use egui::load::{ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint};
use egui::{Color32, ColorImage, Event, PointerButton, Pos2, RawInput, Vec2};
use rustradar_app::app::Engine;
use rustradar_app::scene_builder::SceneBuilder;
use rustradar_app::state::aircraft::Answer;
use rustradar_app::state::search::Ask;
use rustradar_app::state::shortcuts::Open;
use rustradar_app::state::ships::{ShipFeeds, Ships};
use rustradar_app::state::{Action, AppState, Clocks, Startup};
use rustradar_app::ui::shell::{self, Placed, Slots};
use rustradar_app::ui::{anim, theme};
use rustradar_core::geo::{Area, Basemap};
use rustradar_core::model::{NavStatus, Ship, ShipDetails, ship_photo_url};
use rustradar_core::settings::Mode;
use rustradar_core::ship_list::{DEFAULT_SHIP_FILTERS, ShipFilters, ShipMotion, ShipSort};
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_map::io::DirTiles;
use rustradar_scene::camera::ViewState;

/// The desktop reference pictures' canvas, CSS px, at scale 1.
const SIZE: [u32; 2] = [1440, 900];
/// V6: Gothenburg harbour, where the fixture's ships are.
const V6: ViewState = ViewState { longitude: 11.93, latitude: 57.70, zoom: 13.0, bearing: 0.0, pitch: 0.0, position: None };
/// The shots' clock in Unix seconds.
const S: i64 = (NOW_MS / 1000.0) as i64;
/// The ships with cards, and the one whose card failed.
const KATTEGAT: u32 = 265_177_000;
const TERN: u32 = 219_018_273;
const TRADER: u32 = 636_019_825;
const VINGA: u32 = 265_700_456;
const UNNAMED: u32 = 265_600_123;
/// The card's failure.
const FAILURE: &str = "VesselFinder answered 503 Service Unavailable";
/// The fixture's photo id.
const PHOTO: &str = "kattegat-express";

// ------------------------------------------------------------------------------------------------
// fixtures

#[allow(clippy::too_many_arguments)]
fn ship(mmsi: u32, lat: f64, lon: f64, name: &str, kind: u8, size: u8, heading: Option<f32>, old: bool, dims: Option<[u16; 4]>) -> Ship {
    Ship { mmsi, lat, lon, name: name.into(), kind, size, heading, old, dims, frames: Vec::new() }
}

/// The fixture's ships, in the feed's order.
fn ships() -> Vec<Ship> {
    vec![
        ship(KATTEGAT, 57.7072, 11.9441, "KATTEGAT EXPRESS", 3, 2, Some(271.0), false, Some([120, 64, 15, 15])),
        ship(265_410_000, 57.693, 11.908, "SKAGERRAK", 3, 2, None, false, Some([96, 86, 14, 14])),
        ship(TRADER, 57.689, 11.865, "NORDIC TRADER", 4, 2, Some(62.0), false, Some([200, 96, 19, 19])),
        ship(TERN, 57.6985, 11.882, "BALTIC TERN", 6, 1, Some(118.0), false, Some([84, 60, 11, 11])),
        ship(265_547_220, 57.701, 11.921, "HARBOUR TUG 7", 2, 0, Some(300.0), false, Some([16, 16, 5, 5])),
        ship(UNNAMED, 57.704, 11.95, "265600123", 0, 0, None, false, None),
        ship(265_066_000, 57.695, 11.9, "RESCUE ELSA ANDERSSON", 7, 0, Some(20.0), false, Some([10, 8, 2, 2])),
        ship(VINGA, 57.68, 11.84, "VINGA", 5, 0, Some(200.0), true, None),
        ship(265_800_789, 57.71, 11.97, "MARIANNE", 8, 0, None, false, None),
        ship(244_123_000, 57.692, 11.89, "DREDGER NOORD", 33, 1, Some(150.0), false, None),
    ]
}

/// VesselFinder's cards: the ferry's whole card, the tanker's sparse one.
fn cards() -> HashMap<u32, Result<ShipDetails, String>> {
    let kattegat = ShipDetails {
        name: Some("KATTEGAT EXPRESS".into()),
        imo: Some(9_235_517),
        mmsi: KATTEGAT,
        type_label: Some("Passenger/Ro-Ro Cargo Ship".into()),
        flag: Some("Sweden".into()),
        flag_code: Some("SE".into()),
        length_m: Some(184.0),
        beam_m: Some(30.0),
        draught_m: Some(6.3),
        max_draught_m: Some(6.6),
        gross_tonnage: Some(29_289),
        deadweight: Some(5_500),
        year_built: Some(1983),
        destination: Some("DKFDH".into()),
        eta_unix: Some(S + 12_300),
        speed_kn: Some(14.2),
        course_deg: Some(271.0),
        nav_status: Some(NavStatus { code: 0, label: "Under way using engine".into() }),
        position_unix: Some(S - 75),
        satellite: false,
        photo_id: Some(PHOTO.into()),
        sar: false,
    };
    let tern = ShipDetails {
        name: Some("BALTIC TERN".into()),
        mmsi: TERN,
        flag: Some("Denmark".into()),
        flag_code: Some("DK".into()),
        length_m: Some(144.0),
        max_draught_m: Some(8.9),
        deadweight: Some(11_250),
        destination: Some("GOTHENBURG ANCHORAGE".into()),
        eta_unix: Some(S - 46_000),
        course_deg: Some(118.0),
        nav_status: Some(NavStatus { code: 6, label: "Aground".into() }),
        position_unix: Some(S - 2_820),
        satellite: true,
        sar: true,
        ..Default::default()
    };
    HashMap::from([(KATTEGAT, Ok(kattegat)), (TERN, Ok(tern)), (VINGA, Err(FAILURE.to_owned()))])
}

/// The canned feeds: the fixture's ships (or a failure), the cards; a ship without one waits.
#[derive(Clone)]
struct Canned {
    ships: Result<Vec<Ship>, String>,
    cards: HashMap<u32, Result<ShipDetails, String>>,
    /// The cards asked for, in order.
    asked: Arc<Mutex<Vec<u32>>>,
}

impl ShipFeeds for Canned {
    fn ships_in_view(&mut self, _area: Area, _zoom: f64) -> Answer<Result<Vec<Ship>, String>> {
        Answer::ready(self.ships.clone())
    }

    fn ship_details(&mut self, mmsi: u32) -> Answer<Result<ShipDetails, String>> {
        self.asked.lock().unwrap().push(mmsi);
        match self.cards.get(&mmsi) {
            Some(card) => Answer::ready(card.clone()),
            // never answered: the panel loads
            None => std::sync::mpsc::channel().1.into(),
        }
    }
}

/// The state at `view` with ships the focus on `feeds`, polled until the ships are in (or not:
/// `feeds` `None` is the page without its backend, the ships still loading when `settle` is
/// false).
fn state_on(view: ViewState, feeds: Option<Canned>, settle: bool) -> AppState {
    let mut s = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, view, SIZE.map(f64::from));
    s.persisted.settings.mode = Mode::Ships;
    s.ships = Ships::with_feeds(feeds.map(|f| Box::new(f) as Box<dyn ShipFeeds>));
    let times: &[f64] = if settle { &[-2_000.0, -1_000.0, -300.0, 0.0] } else { &[0.0] };
    for t in times {
        s.poll(NOW_MS + t);
    }
    s
}

fn canned() -> Canned {
    Canned { ships: Ok(ships()), cards: cards(), asked: Arc::default() }
}

/// The fixture's state at V6, the ships in.
fn state() -> AppState {
    let s = state_on(V6, Some(canned()), true);
    assert!(s.ships.loaded() && s.ships.ships().len() == 10, "the fixture's ships are in");
    s
}

/// `state` with the ship `mmsi` selected and its card as the feeds answer it.
fn select(mut s: AppState, mmsi: u32) -> AppState {
    let ship = s.ships.ships().iter().find(|x| x.mmsi == mmsi).cloned().expect("a fixture ship");
    s.select_ship(Some(ship), NOW_MS);
    s.poll(NOW_MS + 10.0);
    s
}

// ------------------------------------------------------------------------------------------------
// the photo

/// A 3:2 photo of a ferry: sky, sea, a pale hull.
fn photo() -> ColorImage {
    let (w, h) = (768usize, 512usize);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let c = if (250..300).contains(&y) && (150..620).contains(&x) {
                [235, 235, 240]
            } else if y >= 300 {
                [30, 60, 90]
            } else {
                [(90 + y / 6) as u8, (140 + y / 8) as u8, 200]
            };
            rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    ColorImage::from_rgba_unmultiplied([w, h], &rgba)
}

/// Serves the fixture's photo to egui's image loading, as item U18's loader will serve
/// VesselFinder's.
struct Pictures(HashMap<String, Arc<ColorImage>>);

impl ImageLoader for Pictures {
    fn id(&self) -> &str {
        "rustradar::tests::ship_ui::pictures"
    }

    fn load(&self, _ctx: &egui::Context, uri: &str, _size_hint: SizeHint) -> ImageLoadResult {
        self.0.get(uri).map(|image| ImagePoll::Ready { image: image.clone() }).ok_or(LoadError::NotSupported)
    }

    fn forget(&self, _uri: &str) {}

    fn forget_all(&self) {}

    fn byte_size(&self) -> usize {
        0
    }
}

/// The ColorImage as a shot image, for the Chromium page's `<img>`.
fn as_image(c: &ColorImage) -> Image {
    Image { width: c.size[0] as u32, height: c.size[1] as u32, rgba: c.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect() }
}

// ------------------------------------------------------------------------------------------------
// the shell

/// A canvas tall enough for the whole filter panel over the list (the reference pictures' window
/// leaves the panel 430 px, scrolling its body).
const TALL: [u32; 2] = [1440, 1600];

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    theme::apply(&ctx);
    // the skeletons stand at their start, as in Chromium's capture with animations off
    anim::set_reduced_motion(&ctx, true);
    let pictures = HashMap::from([(ship_photo_url(PHOTO, false), Arc::new(photo()))]);
    ctx.add_image_loader(Arc::new(Pictures(pictures)));
    ctx
}

/// One pass of the shell with the app's slots over `state` on a `size` canvas with `events`:
/// where it put things, the actions, egui's output.
fn run(ctx: &egui::Context, size: [u32; 2], time: f64, state: &AppState, events: Vec<Event>) -> (Placed, Vec<Action>, egui::FullOutput) {
    let raw = RawInput {
        screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(size[0] as f32, size[1] as f32))),
        time: Some(time),
        events,
        ..Default::default()
    };
    let mut placed = Placed::default();
    let mut actions = Vec::new();
    let output = ctx.run_ui(raw, |ui| placed = shell::show(ui.ctx(), state, NOW_MS, &mut actions, &Slots::APP));
    (placed, actions, output)
}

/// egui alone onto a target cleared to `--ink`, as the window's UI pass draws it.
struct Canvas {
    ctx: egui::Context,
    egui: egui_wgpu::Renderer,
    target: Offscreen,
    size: [u32; 2],
}

impl Canvas {
    fn new(gpu: &Gpu, size: [u32; 2]) -> Self {
        let egui = egui_wgpu::Renderer::new(&gpu.device, FORMAT, egui_wgpu::RendererOptions::default());
        Canvas { ctx: context(), egui, target: Offscreen::new(gpu, size), size }
    }

    fn frame(&mut self, gpu: &Gpu, time: f64, state: &AppState) -> (Placed, Vec<Action>) {
        let (placed, actions, output) = run(&self.ctx, self.size, time, state, Vec::new());
        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: self.target.size(), pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui.update_texture(device, queue, id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("u15") });
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
        (placed, actions)
    }

    /// Frames at egui times 0–5 s (areas are sized on their first frame, the sidebar solves its
    /// heights from the last frame's measures, the panel enters over 200 ms, and a scroll bar the
    /// first frame's least height showed fades out over the next two, narrowing the list while it
    /// does), then the picture and the last frame's actions.
    fn settle(&mut self, gpu: &Gpu, state: &AppState) -> (Placed, Vec<Action>, Image) {
        let mut placed = Placed::default();
        let mut actions = Vec::new();
        for time in [0.0, 1.0, 2.0, 3.0, 4.0, 5.0] {
            (placed, actions) = self.frame(gpu, time, state);
        }
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        (placed, actions, self.target.read(gpu).expect("read back"))
    }
}

/// `state` drawn on a `size` canvas and saved as `name`; drawing alone asks nothing, and egui's
/// debug builds flag no unaligned edge (their orange).
fn shot_on(gpu: &Gpu, size: [u32; 2], state: &AppState, name: &str) -> (Placed, Image) {
    let mut canvas = Canvas::new(gpu, size);
    let (placed, actions, image) = canvas.settle(gpu, state);
    save(&image, name);
    assert!(actions.is_empty(), "{name}: drawing alone asks nothing: {actions:?}");
    let orange = common::shots::count(&image, Rect::whole(&image), |p| p[..3] == [255, 165, 0]);
    assert_eq!(orange, 0, "{name}: egui flags an unaligned edge");
    (placed, image)
}

fn shot(gpu: &Gpu, state: &AppState, name: &str) -> (Placed, Image) {
    shot_on(gpu, SIZE, state, name)
}

// ------------------------------------------------------------------------------------------------
// checks

fn rgb(c: Color32) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

fn close(p: [u8; 4], c: Color32, tolerance: u8) -> bool {
    (0..3).all(|i| p[i].abs_diff(rgb(c)[i]) <= tolerance)
}

/// `c` over `under`, as egui blends its premultiplied colours on the (non-sRGB) target.
fn over(c: Color32, under: Color32) -> Color32 {
    let a = f32::from(c.a()) / 255.0;
    let mix = |x: u8, y: u8| (f32::from(x) + f32::from(y) * (1.0 - a)).round().min(255.0) as u8;
    Color32::from_rgb(mix(c.r(), under.r()), mix(c.g(), under.g()), mix(c.b(), under.b()))
}

/// The rectangle of pixels within `tolerance` of `c` in `area`.
fn painted(image: &Image, area: Rect, c: Color32, tolerance: u8) -> Option<Rect> {
    bounds(image, area, |p| close(p, c, tolerance))
}

/// The pixels `x0..x1` × `y0..y1`.
const fn area(x0: u32, y0: u32, x1: u32, y1: u32) -> Rect {
    Rect { x0, y0, x1, y1 }
}

fn lum(p: [u8; 4]) -> f64 {
    (f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])) / 3.0
}

/// The bounds of the pixels in `area` brighter than `under` by more than 30 levels: text and icons.
fn ink(image: &Image, area: Rect, under: Color32) -> Option<Rect> {
    bounds(image, area, |p| lum(p) - lum(rgb(under)) > 30.0)
}

/// `y` px below `top`, as a pixel row.
fn at(top: u32, y: f32) -> u32 {
    (top as f32 + y).round() as u32
}

#[track_caller]
fn near(got: u32, want: f32, tolerance: f32, what: &str) {
    assert!((got as f32 - want).abs() <= tolerance, "{what}: {got} for {want}");
}

/// The rows of `x` from `y0` on that are the `--line` border or rule.
fn rules(image: &Image, x: u32, y0: u32, y1: u32) -> Vec<u32> {
    (y0..y1).filter(|&y| close(pixel(image, x, y), theme::LINE, 2)).collect()
}

/// A card's top border on the sidebar's column (x 20) from `y0`.
fn card_top(image: &Image, y0: u32) -> u32 {
    (y0..image.height).find(|&y| close(pixel(image, 20, y), theme::LINE, 2)).expect("a card's border")
}

/// The card's bottom border under `top` (the first `--line` row with the map's `--ink` under it:
/// not a rule inside the card).
fn card_bottom(image: &Image, top: u32) -> u32 {
    (top + 2..image.height - 1)
        .find(|&y| close(pixel(image, 20, y), theme::LINE, 2) && close(pixel(image, 20, y + 1), theme::INK, 2))
        .expect("the card's bottom border")
}

// ------------------------------------------------------------------------------------------------
// the list (Chromium: the card at y 200, the rows 52.98 apart from 82.78 in, 54.69 with the badge)

/// Where Chromium puts row `i`'s top below the card's top border, the rows before it plain.
fn row_top(i: usize) -> f32 {
    82.78 + 52.98 * i as f32
}

/// The swatch's middle in row `i` (8 px under the row's top, the name's 18.19 px line).
fn swatch_at(top: u32, i: usize) -> [u32; 2] {
    [37, at(top, row_top(i) + 8.0 + 9.1)]
}

#[test]
fn the_list_sorted_by_size_with_a_ship_selected() {
    let Some(gpu) = gpu() else { return };
    save(&as_image(&photo()), "u15-photo.png");
    let state = select(state(), KATTEGAT);
    let (placed, image) = shot(&gpu, &state, "u15-list.png");
    let card = placed.sidebar.list.expect("the list card");
    let top = card_top(&image, 170);
    near(top, card.min.y, 0.5, "the card's top");
    // the whole list: 654.72 px (the header, the sort, ten rows, the coverage note)
    near(card_bottom(&image, top), top as f32 + 654.72 - 1.0, 1.0, "the card's bottom border");
    // the count on the header's right, ending at 319
    let count = ink(&image, area(290, at(top, 13.0), 326, at(top, 33.0)), theme::SURFACE).expect("the count");
    near(count.x1, 319.0, 1.0, "the count's right");
    // the selected ship's row on --amber-soft: 135.77–188.75 in, 25–327 across
    let selected = over(theme::AMBER_SOFT, theme::SURFACE);
    let row = painted(&image, area(20, top + 100, 332, top + 260), selected, 3).expect("the selected row");
    near(row.y0, top as f32 + row_top(1), 1.0, "the selected row's top");
    near(row.y1, top as f32 + row_top(2), 1.0, "the selected row's bottom");
    assert!(row.x0 <= 26 && row.x1 >= 326, "{row:?}");
    // its name in the accent
    assert!(painted(&image, area(49, at(top, row_top(1) + 8.0), 180, at(top, row_top(1) + 26.0)), theme::AMBER, 40).is_some(), "the name in --amber");
    // the swatches in the ships' colours, largest first
    for (i, kind) in [4u8, 3, 3, 6, 33, 2, 7, 8, 5].into_iter().enumerate() {
        let [x, y] = swatch_at(top, i);
        let c = rustradar_core::ships::ship_color(kind);
        assert!(close(pixel(&image, x, y), Color32::from_rgb(c[0], c[1], c[2]), 12), "row {i}'s swatch: {:?}", pixel(&image, x, y));
    }
    // the measures end at 319: `296 m` on the first row
    let measure = ink(&image, area(260, at(top, row_top(0) + 8.0), 326, at(top, row_top(0) + 26.0)), theme::SURFACE).expect("296 m");
    near(measure.x1, 319.0, 1.0, "the measure's right");
    // VINGA's old position: the warn badge 246.19–319 across, 534.84–553.34 in
    let badge = painted(&image, area(200, top + 500, 326, top + 580), over(theme::WARN_SOFT, theme::SURFACE), 4).expect("the badge");
    near(badge.x0, 246.19, 1.0, "the badge's left");
    near(badge.x1, 319.0, 1.0, "the badge's right");
    near(badge.y0, top as f32 + 534.84, 1.0, "the badge's top");
    near(badge.y1, top as f32 + 553.34, 1.0, "the badge's bottom");
}

#[test]
fn the_list_by_distance_and_by_name_zoomed_out() {
    let Some(gpu) = gpu() else { return };
    let mut s = state();
    s.persisted.ship_filters.sort = ShipSort::Distance;
    let (_, image) = shot(&gpu, &s, "u15-list-distance.png");
    let top = card_top(&image, 170);
    // nearest first from the snapped centre (57.70, 11.95): the unnamed ship 440 m away first
    for (i, kind) in [0u8, 3, 8, 2, 3, 7, 33, 6, 4, 5].into_iter().enumerate() {
        let [x, y] = swatch_at(top, i);
        let c = rustradar_core::ships::ship_color(kind);
        assert!(close(pixel(&image, x, y), Color32::from_rgb(c[0], c[1], c[2]), 12), "row {i}'s swatch: {:?}", pixel(&image, x, y));
    }

    // zoomed out the feed lists every ship as medium: no size classes, the lengths stay
    let mut s = state_on(ViewState { zoom: 5.5, ..V6 }, Some(canned()), true);
    s.persisted.ship_filters.sort = ShipSort::Name;
    let (_, image) = shot(&gpu, &s, "u15-list-name.png");
    let top = card_top(&image, 170);
    for (i, kind) in [6u8, 33, 2, 3, 8, 4, 7, 3, 5, 0].into_iter().enumerate() {
        let [x, y] = swatch_at(top, i);
        let c = rustradar_core::ships::ship_color(kind);
        assert!(close(pixel(&image, x, y), Color32::from_rgb(c[0], c[1], c[2]), 12), "row {i}'s swatch: {:?}", pixel(&image, x, y));
    }
    let measure = |i: usize| ink(&image, area(240, at(top, row_top(i) + 8.0), 326, at(top, row_top(i) + 26.0)), theme::SURFACE);
    assert!(measure(0).is_some(), "BALTIC TERN's 144 m");
    assert!(measure(1).is_none(), "DREDGER NOORD: no length, no class");
    assert!(measure(4).is_none(), "MARIANNE: no length, no class");
}

#[test]
fn the_list_loading_failed_and_matching_none() {
    let Some(gpu) = gpu() else { return };
    // loading: six 40 px placeholder rows, the first's skeletons at 90.78 in (80 and 40 wide)
    let (_, image) = shot(&gpu, &state_on(V6, Some(canned()), false), "u15-list-loading.png");
    let top = card_top(&image, 170);
    near(card_bottom(&image, top), top as f32 + 335.78 - 1.0, 1.0, "the loading card's bottom");
    let first = bounds(&image, area(25, at(top, 85.0), 160, at(top, 100.0)), |p| lum(p) > lum(rgb(theme::SURFACE)) + 6.0).expect("the first skeleton");
    near(first.x0, 33.0, 1.0, "the skeleton's left");
    near(first.x1, 113.0, 1.0, "the skeleton's right (80 wide)");
    near(first.y0, top as f32 + 90.78, 1.0, "the skeleton's top");
    let right = bounds(&image, area(240, at(top, 85.0), 326, at(top, 100.0)), |p| lum(p) > lum(rgb(theme::SURFACE)) + 6.0).expect("the right skeleton");
    near(right.x0, 279.0, 1.0, "the right skeleton's left (40 wide)");

    // VesselFinder unreachable: the two-line message and the coverage (176.95)
    let (_, image) = shot(&gpu, &state_on(V6, Some(Canned { ships: Err("unreachable".into()), ..canned() }), true), "u15-list-failed.png");
    let top = card_top(&image, 170);
    near(card_bottom(&image, top), top as f32 + 176.95 - 1.0, 1.0, "the failed card's bottom");
    let coverage = ink(&image, area(25, at(top, 137.0), 330, at(top, 158.0)), theme::SURFACE).expect("the coverage line");
    near(coverage.x0, 33.0, 1.0, "the coverage's left");

    // none match: the message and Show all (33–142.55 across, 120.97 in)
    let mut s = state();
    s.persisted.ship_filters.hidden_kinds = vec![0, 2, 3, 4, 5, 6, 7, 8];
    let (_, image) = shot(&gpu, &s, "u15-list-nomatch.png");
    let top = card_top(&image, 170);
    near(card_bottom(&image, top), top as f32 + 173.97 - 1.0, 1.0, "the card's bottom");
    let button = painted(&image, area(25, top + 110, 200, top + 165), theme::LINE_CONTROL, 6).expect("Show all");
    near(button.x0, 33.0, 1.0, "Show all's left");
    near(button.x1, 142.55, 1.0, "Show all's right");
    near(button.y0, top as f32 + 120.97, 1.0, "Show all's top");
    near(button.y1, top as f32 + 152.97, 1.0, "Show all's bottom");
    // its words in the message's --text-2 (a button's colour is inherited)
    let brightest = (button.x0 + 2..button.x1 - 2).flat_map(|x| (button.y0 + 2..button.y1 - 2).map(move |y| (x, y))).map(|(x, y)| lum(pixel(&image, x, y))).fold(0.0, f64::max);
    assert!((180.0..=200.0).contains(&brightest), "Show all's words at {brightest}, --text-2 is 196");
}

// ------------------------------------------------------------------------------------------------
// the filter panel (Chromium: the card at y 100)

#[test]
fn the_filter_panel() {
    let Some(gpu) = gpu() else { return };
    let mut s = state();
    s.persisted.ship_filters = ShipFilters { hidden_kinds: vec![5], motion: ShipMotion::Moving, hidden_sizes: vec![0], hide_old: true, sort: ShipSort::Size };
    assert!(s.apply(Action::Open(Open::Filters, true), NOW_MS));
    // in today's window it gives way to the list, scrolling its body
    shot(&gpu, &s, "u15-filters.png");
    let (_, image) = shot_on(&gpu, TALL, &s, "u15-filters-whole.png");
    let top = card_top(&image, 170);
    near(card_bottom(&image, top), top as f32 + 479.36 - 1.0, 1.0, "the panel's natural height");
    assert_eq!(rules(&image, 25, top + 1, top + 60), [top + 49], "the header's rule");
    // Reset is on: its word in the accent (247.78–287 across)
    assert!(painted(&image, area(245, top + 14, 290, top + 36), theme::AMBER, 40).is_some(), "Reset on");
    // the movement: Under way chosen, the accent 130–222 across, 284.59–312.78 in
    let chosen = painted(&image, area(30, top + 270, 330, top + 330), theme::AMBER, 4).expect("the chosen movement");
    near(chosen.x0, 130.0, 1.0, "Under way's left");
    near(chosen.x1, 222.0, 1.0, "Under way's right");
    near(chosen.y0, top as f32 + 284.59, 1.0, "Under way's top");
    // Fishing left out: its chip without the check, Tanker with it (both 224.8 in)
    let check = |x0: u32| painted(&image, area(x0, at(top, 124.8), x0 + 20, at(top, 150.8)), theme::AMBER, 60);
    assert!(check(127).is_none(), "Fishing is off");
    assert!(check(212).is_some(), "Tanker is on");
    // Hide old positions on: the switch's track in the accent at 285–319, 434.86 in
    let track = painted(&image, area(270, top + 420, 330, top + 470), theme::AMBER, 4).expect("the switch on");
    near(track.x0, 285.0, 1.0, "the switch's left");
    near(track.y0, top as f32 + 434.86, 1.0, "the switch's top");

    // zoomed out: the sizes disabled under their two-line hint, Reset off (520.95)
    let mut s = state_on(ViewState { zoom: 5.5, ..V6 }, Some(canned()), true);
    assert!(s.apply(Action::Open(Open::Filters, true), NOW_MS));
    shot(&gpu, &s, "u15-filters-out.png");
    let (_, image) = shot_on(&gpu, TALL, &s, "u15-filters-out-whole.png");
    let top = card_top(&image, 170);
    // within 1.5: the FilterChips widget sets its hint in egui's whole-pixel rows (a pixel lower)
    near(card_bottom(&image, top), top as f32 + 520.95 - 1.0, 1.5, "the panel's natural height");
    assert!(painted(&image, area(245, top + 14, 290, top + 36), theme::AMBER, 40).is_none(), "Reset off");
    let hint = ink(&image, area(25, at(top, 417.0), 330, at(top, 455.0)), theme::SURFACE).expect("the sizes' hint");
    assert!(hint.height() > 24, "two lines: {hint:?}");
    let track = painted(&image, area(270, top + 460, 330, top + 510), theme::LINE_CONTROL, 4).expect("the switch off");
    near(track.y0, top as f32 + 476.45, 1.0, "the switch's top");
}

// ------------------------------------------------------------------------------------------------
// the panel (Chromium: right 16, bottom 28, as today's)

/// The panel's border box on the picture: the `--line` border's outer edges.
fn panel_box(image: &Image) -> Rect {
    painted(image, area(1000, 0, 1440, 900), theme::LINE, 2).expect("the panel's border")
}

#[test]
fn the_panel_with_its_card_and_photo() {
    let Some(gpu) = gpu() else { return };
    let mut s = select(state(), KATTEGAT);
    s.ships.set_following(true);
    let (placed, image) = shot(&gpu, &s, "u15-panel.png");
    assert_eq!(placed.panel.map(|p| p.0), Some(shell::Panel::Ship));
    let b = panel_box(&image);
    // 692.08 high from 179.92: the strip (65.89), the photo (180), the voyage (122.09), the body
    // (260.72) and the footer (61.38, its age on two lines)
    assert_eq!((b.x0, b.x1, b.y1), (1062, 1424, 872), "{b:?}");
    near(b.y0, 179.92, 1.0, "the panel's top");
    assert_eq!(rules(&image, 1065, b.y0 + 2, 871), [246, 548, 810], "the strip's, the voyage's and the footer's rules");
    // the photo: the sky under the strip, the sea at its bottom
    let sky = pixel(&image, 1100, 300);
    assert!(sky[2] > sky[0] + 60, "the sky: {sky:?}");
    assert!(close(pixel(&image, 1100, 420), Color32::from_rgb(30, 60, 90), 6), "the sea");
    // the credit at the bottom right (1318–1415)
    let credit = ink(&image, area(1300, 405, 1423, 425), Color32::from_rgb(30, 60, 90)).expect("the credit");
    near(credit.x1, 1415.0, 1.0, "the credit's right");
    // the speed in the accent, the UN/LOCODE at 22 px
    assert!(painted(&image, area(1311, 194, 1372, 214), theme::AMBER, 40).is_some(), "14.2 kn in --amber");
    let code = ink(&image, area(1075, 461, 1200, 486), theme::SURFACE).expect("DKFDH");
    assert!(code.height() >= 15, "a 22 px code: {code:?}");
    // the status badge (neutral) at 1079, 513.41
    let status = painted(&image, area(1070, 505, 1300, 540), theme::SURFACE_RAISED, 3).expect("the status badge");
    near(status.x0, 1079.0, 1.0, "the badge's left");
    near(status.y0, 513.41, 1.0, "the badge's top");
    // Follow pressed: its accent border 1189.53–1275.11, 824.81–856.81
    let follow = painted(&image, area(1150, 812, 1300, 870), theme::AMBER, 30).expect("Follow's border");
    near(follow.x0, 1189.53, 1.0, "Follow's left");
    near(follow.x1, 1275.11, 1.0, "Follow's right");
    near(follow.y0, 824.81, 1.0, "Follow's top");

    // a free-text destination, an ETA passed, aground, SAR, satellite AIS, a stale report
    let (_, image) = shot(&gpu, &select(state(), TERN), "u15-panel-free.png");
    let b = panel_box(&image);
    near(b.y0, 405.8, 1.0, "the panel's top");
    assert_eq!(rules(&image, 1065, b.y0 + 2, 871), [474, 596, 809]);
    // aground in the warn tone (1079–1134.84), SAR neutral after it
    let aground = painted(&image, area(1070, 555, 1300, 585), over(theme::WARN_SOFT, theme::SURFACE), 4).expect("Aground");
    near(aground.x1, 1134.84, 1.0, "Aground's right");
    assert!(painted(&image, area(1140, 555, 1300, 585), theme::SURFACE_RAISED, 3).is_some(), "the SAR badge");
    // the stale report's badge at 12 px (1079–1196.75, 821.61–841.61), running under Follow
    let stale = painted(&image, area(1070, 815, 1189, 850), over(theme::WARN_SOFT, theme::SURFACE), 4).expect("the stale report");
    near(stale.y0, 821.61, 1.0, "the stale badge's top");
    near(stale.y1, 841.61, 1.0, "the stale badge's bottom");
    // `via satellite AIS` under it
    assert!(ink(&image, area(1079, 844, 1160, 860), theme::SURFACE).is_some(), "via satellite AIS");

    // loading: the skeletons, the photo's block
    let (_, image) = shot(&gpu, &select(state(), TRADER), "u15-panel-loading.png");
    let b = panel_box(&image);
    near(b.y0, 390.02, 1.0, "the panel's top");
    assert_eq!(rules(&image, 1065, b.y0 + 2, 871), [458, 717, 814]);
    assert!(close(pixel(&image, 1240, 550), theme::SURFACE_RAISED, 2), "the photo's block");
    let speed = bounds(&image, area(1305, 400, 1380, 445), |p| lum(p) > lum(rgb(theme::SURFACE)) + 6.0).expect("the speed's skeletons");
    near(speed.x0, 1315.0, 1.0, "the skeletons' left");
    near(speed.y0, 410.5, 1.0, "the skeletons' top");

    // the card failed: Try again, the facts the map knows, the old position
    let (_, image) = shot(&gpu, &select(state(), VINGA), "u15-panel-error.png");
    let b = panel_box(&image);
    near(b.y0, 533.47, 1.0, "the panel's top");
    assert_eq!(rules(&image, 1065, b.y0 + 2, 871), [601, 814], "bare: no voyage");
    let retry = painted(&image, area(1070, 660, 1200, 705), theme::LINE_CONTROL, 6).expect("Try again");
    near(retry.x1, 1182.03, 1.0, "Try again's right");
    near(retry.y0, 668.03, 1.0, "Try again's top");
    let old = painted(&image, area(1070, 825, 1189, 860), over(theme::WARN_SOFT, theme::SURFACE), 4).expect("Old position");
    near(old.y0, 833.0, 1.0, "the old position's top");

    // unnamed, of an unknown type, loading: the subtitle's skeleton (140 × 12)
    let (_, image) = shot(&gpu, &select(state(), UNNAMED), "u15-panel-unnamed.png");
    let b = panel_box(&image);
    near(b.y0, 395.04, 1.0, "the panel's top");
    let sub = bounds(&image, area(1075, b.y0 + 38, 1260, b.y0 + 58), |p| lum(p) > lum(rgb(theme::SURFACE)) + 6.0).expect("the subtitle's skeleton");
    near(sub.width(), 140.0, 1.0, "the subtitle skeleton's width");
}

// ------------------------------------------------------------------------------------------------
// interactions (egui only)

/// A pass with no renderer: what was placed, the actions, the links opened.
fn step(ctx: &egui::Context, size: [u32; 2], time: f64, state: &AppState, events: Vec<Event>) -> (Placed, Vec<Action>, Vec<String>) {
    let (placed, actions, mut output) = run(ctx, size, time, state, events);
    output.textures_delta.clear();
    let opened = output.platform_output.commands.iter().filter_map(|c| if let egui::OutputCommand::OpenUrl(u) = c { Some(u.url.clone()) } else { None }).collect();
    (placed, actions, opened)
}

fn click(at: Pos2) -> Vec<Event> {
    vec![
        Event::PointerMoved(at),
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() },
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() },
    ]
}

/// Settles `state` in `ctx` on a `size` canvas, then clicks at the point `at` gives from what was
/// placed: the actions and the links opened.
fn click_at(ctx: &egui::Context, size: [u32; 2], state: &AppState, at: impl Fn(&Placed) -> Pos2) -> (Vec<Action>, Vec<String>) {
    let start = ctx.input(|i| i.time) + 1.0;
    let mut placed = Placed::default();
    for t in 0..6 {
        placed = step(ctx, size, start + f64::from(t), state, Vec::new()).0;
    }
    let p = at(&placed);
    step(ctx, size, start + 6.0, state, vec![Event::PointerMoved(p)]);
    let (_, actions, opened) = step(ctx, size, start + 6.1, state, click(p));
    let (_, after, more) = step(ctx, size, start + 6.2, state, Vec::new());
    (actions.into_iter().chain(after).collect(), opened.into_iter().chain(more).collect())
}

/// The point `y` px under the list card's top border, `x` across.
fn in_list(placed: &Placed, x: f32, y: f32) -> Pos2 {
    Pos2::new(x, placed.sidebar.list.expect("the list card").min.y + y)
}

#[test]
fn a_row_picks_its_ship_and_its_hover_reaches_the_map() {
    let ctx = context();
    let mut s = state();
    // the pointer resting on the second row (KATTEGAT EXPRESS)
    let start = ctx.input(|i| i.time) + 1.0;
    let mut placed = Placed::default();
    for t in 0..6 {
        placed = step(&ctx, SIZE, start + f64::from(t), &s, Vec::new()).0;
    }
    let on_row = in_list(&placed, 150.0, row_top(1) + 26.0);
    let (_, actions, _) = step(&ctx, SIZE, start + 6.0, &s, vec![Event::PointerMoved(on_row)]);
    assert_eq!(actions, [Action::HoverShip(Some(KATTEGAT))]);
    assert!(s.apply(Action::HoverShip(Some(KATTEGAT)), NOW_MS));
    // the ship builder draws it highlighted while ships are the focus
    let mut builder = SceneBuilder::default();
    builder.build(&mut s, Clocks { local_ms: NOW_MS, feed_ms: NOW_MS }, 1.0);
    assert_eq!(builder.ships.hovered, Some(KATTEGAT));
    let (_, actions, _) = step(&ctx, SIZE, start + 6.1, &s, Vec::new());
    assert!(actions.is_empty(), "the same hover asks nothing again: {actions:?}");
    // off the rows: none
    let (_, actions, _) = step(&ctx, SIZE, start + 6.2, &s, vec![Event::PointerMoved(Pos2::new(700.0, 500.0))]);
    assert_eq!(actions, [Action::HoverShip(None)]);
    assert!(s.apply(Action::HoverShip(None), NOW_MS));
    builder.build(&mut s, Clocks { local_ms: NOW_MS, feed_ms: NOW_MS }, 1.0);
    assert_eq!(builder.ships.hovered, None);
    // a hover left on another focus is not drawn
    s.apply(Action::HoverShip(Some(KATTEGAT)), NOW_MS);
    s.persisted.settings.mode = Mode::Aircraft;
    builder.build(&mut s, Clocks { local_ms: NOW_MS, feed_ms: NOW_MS }, 1.0);
    assert_eq!(builder.ships.hovered, None);
    s.persisted.settings.mode = Mode::Ships;
    s.apply(Action::HoverShip(None), NOW_MS);

    // a click picks the ship: selected, its card asked for, the camera flying to it
    let (actions, _) = click_at(&ctx, SIZE, &s, |p| in_list(p, 150.0, row_top(3) + 26.0));
    assert!(actions.contains(&Action::PickShip(TERN)), "{actions:?}");
    assert!(s.apply(Action::PickShip(TERN), NOW_MS));
    assert_eq!(s.ships.selected_mmsi(), Some(TERN));
    assert!(s.controller.in_transition(), "flying to the ship");
    assert!(!s.apply(Action::PickShip(1), NOW_MS), "no such ship");
}

#[test]
fn the_sort_and_show_all_through_actions() {
    let ctx = context();
    let mut s = state();
    // Distance: the sort's third option (224–316 across, 43.59–71.78 in)
    let (actions, _) = click_at(&ctx, SIZE, &s, |p| in_list(p, 270.0, 57.0));
    assert_eq!(actions, [Action::SetShipFilters]);
    assert!(s.apply(Action::SetShipFilters, NOW_MS));
    assert_eq!(s.persisted.ship_filters.sort, ShipSort::Distance);
    assert!(!s.apply(Action::SetShipFilters, NOW_MS), "handed over once");

    // none match: Show all empties the search and the filters, the sort kept
    s.persisted.ship_filters.hidden_kinds = vec![0, 2, 3, 4, 5, 6, 7, 8];
    s.persisted.ship_filters.hide_old = true;
    s.search.hand_over("tern".into());
    assert!(s.apply(Action::Search(Ask::Typed), NOW_MS));
    let (actions, _) = click_at(&ctx, SIZE, &s, |p| in_list(p, 80.0, 136.0));
    assert_eq!(actions, [Action::Search(Ask::EmptyQuery), Action::SetShipFilters]);
    for a in actions {
        assert!(s.apply(a, NOW_MS));
    }
    assert_eq!(s.persisted.ship_filters, ShipFilters { sort: ShipSort::Distance, ..DEFAULT_SHIP_FILTERS });
    assert_eq!(s.search.list_query(Mode::Ships), "");
}

/// The point `y` px under the filter card's top border, `x` across.
fn in_filters(placed: &Placed, x: f32, y: f32) -> Pos2 {
    Pos2::new(x, placed.sidebar.filters.expect("the filter card").min.y + y)
}

#[test]
fn the_filter_panel_s_chips_movement_reset_and_close() {
    let ctx = context();
    let mut s = state();
    assert!(s.apply(Action::Open(Open::Filters, true), NOW_MS));
    // Fishing (126.67–208.26 across, 124.8–150.8 in): left out
    let (actions, _) = click_at(&ctx, TALL, &s, |p| in_filters(p, 167.0, 137.8));
    assert_eq!(actions, [Action::SetShipFilters]);
    assert!(s.apply(Action::SetShipFilters, NOW_MS));
    assert_eq!(s.persisted.ship_filters.hidden_kinds, [5]);
    assert_eq!(s.ships.filtered_len(), 10, "the map's list is filtered again at the next poll");
    s.poll(NOW_MS + 20.0);
    assert_eq!(s.ships.filtered_len(), 9);
    // Moored (224–316 across, 284.59–312.78 in)
    let (actions, _) = click_at(&ctx, TALL, &s, |p| in_filters(p, 270.0, 298.0));
    assert_eq!(actions, [Action::SetShipFilters]);
    assert!(s.apply(Action::SetShipFilters, NOW_MS));
    assert_eq!(s.persisted.ship_filters.motion, ShipMotion::Moored);
    // Reset: the defaults, the sort kept
    s.persisted.ship_filters.sort = ShipSort::Name;
    let (actions, _) = click_at(&ctx, TALL, &s, |p| in_filters(p, 267.0, 25.0));
    assert_eq!(actions, [Action::SetShipFilters]);
    assert!(s.apply(Action::SetShipFilters, NOW_MS));
    assert_eq!(s.persisted.ship_filters, ShipFilters { sort: ShipSort::Name, ..DEFAULT_SHIP_FILTERS });
    // nothing to reset: the button is off
    let (actions, _) = click_at(&ctx, TALL, &s, |p| in_filters(p, 267.0, 25.0));
    assert!(actions.is_empty(), "{actions:?}");
    // the close button
    let (actions, _) = click_at(&ctx, TALL, &s, |p| in_filters(p, 311.0, 25.0));
    assert_eq!(actions, [Action::Open(Open::Filters, false)]);
    assert!(s.apply(Action::Open(Open::Filters, false), NOW_MS));
    assert!(!s.shortcuts.filters_open());
}

/// The point at `x` across and `y` under the panel's top border.
fn in_panel(placed: &Placed, x: f32, y: f32) -> Pos2 {
    let (_, rect) = placed.panel.expect("the panel");
    Pos2::new(x, rect.min.y + y)
}

#[test]
fn the_panel_s_close_follow_links_and_try_again() {
    let ctx = context();
    let mut s = select(state(), KATTEGAT);
    // Follow (1189.53–1275.11, 824.81–856.81): the camera follows the ship, as with the key C
    let (actions, _) = click_at(&ctx, SIZE, &s, |_| Pos2::new(1232.0, 840.0));
    assert_eq!(actions, [Action::ToggleFollow]);
    assert!(s.apply(Action::ToggleFollow, NOW_MS));
    assert!(s.ships.following());
    // the photo and VesselFinder open the ship's page
    let page = rustradar_core::model::ship_page_url(KATTEGAT);
    let (actions, opened) = click_at(&ctx, SIZE, &s, |p| in_panel(p, 1240.0, 66.0 + 90.0));
    assert!(actions.is_empty(), "{actions:?}");
    assert_eq!(opened, std::slice::from_ref(&page));
    let (_, opened) = click_at(&ctx, SIZE, &s, |_| Pos2::new(1345.0, 840.0));
    assert_eq!(opened, [page]);
    // the close button: the strip's last column, centred
    let (actions, _) = click_at(&ctx, SIZE, &s, |p| in_panel(p, 1424.0 - 1.0 - 20.0, 1.0 + 33.0));
    assert_eq!(actions, [Action::CloseShip]);
    assert!(s.apply(Action::CloseShip, NOW_MS));
    assert_eq!(s.ships.selected_mmsi(), None);
    assert!(!s.apply(Action::CloseShip, NOW_MS), "nothing more to close");

    // the card failed: Try again (1079–1182.03, 668.03–700.03) asks for it again, the skeletons
    // back until it answers
    let feeds = canned();
    let asked = feeds.asked.clone();
    let mut s = select(state_on(V6, Some(feeds), true), VINGA);
    assert_eq!(s.ships.card_error(), Some(FAILURE));
    let (actions, _) = click_at(&ctx, SIZE, &s, |_| Pos2::new(1130.0, 684.0));
    assert_eq!(actions, [Action::RetryShipCard]);
    assert!(s.apply(Action::RetryShipCard, NOW_MS + 20.0));
    assert_eq!(asked.lock().unwrap().iter().filter(|m| **m == VINGA).count(), 2);
    assert_eq!(s.ships.card_error(), None, "loading again");
    s.poll(NOW_MS + 30.0);
    assert_eq!(s.ships.card_error(), Some(FAILURE));
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
}

/// The fixture at V6 on the satellite map with ships the focus, through the window's engine: the
/// list, a ship's panel, and a row under the pointer highlighting its ship on the map.
#[test]
fn the_ships_through_the_window() {
    let Some(gpu) = gpu() else { return };
    let mut state = state_on(V6, Some(canned()), true);
    state.set_basemap(Basemap::Satellite, NOW_MS);
    let tern = state.ships.ships().iter().find(|x| x.mmsi == TERN).cloned().unwrap();
    state.select_ship(Some(tern), NOW_MS);
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
    for _ in 0..6 {
        w.frame(&gpu, Vec::new(), NOW_MS);
    }
    assert_eq!(w.engine.state.ships.card_details().map(|d| d.mmsi), Some(TERN), "the tanker's card");
    // the pointer on NORDIC TRADER's row: its ship highlighted on the map at the next scene
    let egui = w.engine.egui.clone();
    let row = |mmsi: u32| egui.read_response(egui::Id::unique(("rustradar::lists::ships::row", mmsi))).expect("the row").rect;
    let at = row(TRADER).center();
    w.frame(&gpu, vec![Event::PointerMoved(at)], NOW_MS + 16.0);
    assert_eq!(w.engine.state.hovered_ship, Some(TRADER));
    w.engine.draw = true;
    w.frame(&gpu, Vec::new(), NOW_MS + 32.0);
    let latest = &w.engine.scene.ships.latest;
    assert_eq!(latest.hovered.map(|i| latest.mmsis[i]), Some(TRADER), "the hovered ship on the map");
    let image = w.target.read(&gpu).expect("read back");
    save(&image, "u15-window-v6.png");
    // the list card's left border and the panel's right one over the map
    let border = |x: u32, y: std::ops::Range<u32>| y.clone().filter(|&y| close(pixel(&image, x, y), theme::LINE, 1)).count() == y.len();
    assert!(border(16, 200..600), "the list card's left edge at x 16");
    assert!(border(1423, 500..860), "the panel's right edge at right 16");
    // the selected row on --amber-soft, the hovered one on --surface-hover
    let r = row(TERN);
    let rect = area(r.min.x as u32 + 2, r.min.y as u32 + 2, r.max.x as u32 - 2, r.max.y as u32 - 2);
    assert!(painted(&image, rect, over(theme::AMBER_SOFT, theme::SURFACE), 3).is_some(), "the selected row");
    let r = row(TRADER);
    let rect = area(r.min.x as u32 + 2, r.min.y as u32 + 2, r.max.x as u32 - 2, r.max.y as u32 - 2);
    assert!(painted(&image, rect, theme::SURFACE_HOVER, 3).is_some(), "the hovered row");
}
