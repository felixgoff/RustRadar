//! Item U19's acceptance (plan stage 7; doc 06 §2.2, §2.4–2.5, §3.12, §17, §19.1): the phone
//! layout as the window's engine draws it over the map at 390 × 844.
//!
//! - beside today's `docs/port/reference/dark-v1-390x844.png` (`u19-phone-v1.png`): the top bar's
//!   card and search field, the chosen mode's cell, the sheet's top edge at `peek`, its handle, the
//!   Filters button in its head, the rail's stacks above it and the credits on it;
//! - with a 47 px top and 34 px bottom safe area injected, the list's sheet at `peek`, `half` and
//!   `full` (`u19-sheet-{peek,half,full}.png`): the bar under the notch, the sheet's heights, the
//!   home bar's padding under the content, the rail riding on the sheet;
//! - a flight's details in the sheet (`u19-flight-{peek,full}.png`): the bar gone, the strip, the
//!   route and the footer at `peek`, the rest under the route when raised;
//! - the Settings menu as a sheet of its own (`u19-menu.png`).
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::shots::{Rect, bounds, gpu, pixel, save};
use common::{NOW_MS, fixtures};
use egui::{Color32, Pos2, SafeAreaInsets, Vec2};
use rustradar_app::app::Engine;
use rustradar_app::state::flight::SelectOptions;
use rustradar_app::state::shortcuts::Menu;
use rustradar_app::state::{Action, AppState, Startup};
use rustradar_app::ui::sheet::Detent;
use rustradar_app::ui::theme;
use rustradar_core::geo::Basemap;
use rustradar_core::settings::Mode;
use rustradar_core::model::{FlightDetails, FlightProgress};
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_map::io::DirTiles;
use rustradar_scene::camera::ViewState;

/// The phone reference pictures' canvas, CSS px, at scale 1.
const SIZE: [u32; 2] = [390, 844];

fn insets(top: f32, bottom: f32) -> SafeAreaInsets {
    SafeAreaInsets(egui::epaint::MarginF32 { left: 0.0, right: 0.0, top, bottom })
}

/// The window: the engine with the shell on, drawing onto an offscreen target.
struct Window {
    engine: Engine,
    target: Offscreen,
    egui_s: f64,
    insets: SafeAreaInsets,
    size: [u32; 2],
}

impl Window {
    fn new(gpu: &Gpu, state: AppState, insets: SafeAreaInsets) -> Self {
        Window::sized(gpu, state, insets, SIZE)
    }

    fn sized(gpu: &Gpu, state: AppState, insets: SafeAreaInsets, size: [u32; 2]) -> Self {
        let mut engine = Engine::new(gpu, FORMAT, state);
        engine.map.set_tiles(Arc::new(DirTiles::fixtures()), None);
        engine.shell = true;
        engine.draw = false;
        Window { engine, target: Offscreen::new(gpu, size), egui_s: 0.0, insets, size }
    }

    /// A frame at local time `now`, egui's clock a quarter of a second on (every transition of
    /// 220 ms is over by the next).
    fn frame(&mut self, gpu: &Gpu, now: f64) {
        let mut input = egui::RawInput {
            time: Some(self.egui_s),
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(self.size[0] as f32, self.size[1] as f32))),
            safe_area_insets: Some(self.insets),
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(1.0);
        self.egui_s += 0.25;
        self.engine.frame(gpu, self.target.view(), self.size, 1.0, input, now);
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("the frame");
    }

    /// Frames until the map's tiles are in.
    fn settle(&mut self, gpu: &Gpu, now: f64) {
        let started = std::time::Instant::now();
        loop {
            self.frame(gpu, now);
            if !self.engine.map.pending() || started.elapsed() > rustradar_map::headless::MAX_WAIT {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Frames for the UI to settle (the sheet measures its summary in one, eases in the next, the
    /// rail follows it in the one after), the last one drawn and read back.
    fn shot(&mut self, gpu: &Gpu, now: f64) -> Image {
        for _ in 0..4 {
            self.frame(gpu, now);
        }
        self.engine.draw = true;
        self.frame(gpu, now);
        self.engine.draw = false;
        self.target.read(gpu).expect("read back")
    }
}

fn rgb(c: Color32) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

fn lum(p: [u8; 4]) -> f64 {
    (f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])) / 3.0
}

fn near(p: [u8; 4], c: Color32, tolerance: u8) -> bool {
    (0..3).all(|i| p[i].abs_diff(rgb(c)[i]) <= tolerance)
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

/// The first row from `y0` down at column `x` whose pixel is `c` (within 3 levels).
fn row_of(image: &Image, x: u32, y0: u32, y1: u32, c: Color32) -> Option<u32> {
    (y0..y1).find(|&y| near(pixel(image, x, y), c, 3))
}

/// The first column from `x0` right on row `y` whose pixel is `c` (within 3 levels).
fn column_of(image: &Image, y: u32, x0: u32, x1: u32, c: Color32) -> Option<u32> {
    (x0..x1).find(|&x| near(pixel(image, x, y), c, 3))
}

/// The sheet's top border, from its handle's pill (36 × 4 of `--line-control` in the middle of the
/// 22 px under the border): the dark map's land is `--surface`'s colour, so the border alone does
/// not say where the sheet begins. Checks the border's row and the handle's fill.
fn sheet_top(image: &Image) -> u32 {
    let pill = |y: u32| (180..210).all(|x| (y..y + 4).all(|y| near(pixel(image, x, y), theme::LINE_CONTROL, 16)));
    let pill_y = (20..SIZE[1] - 10).rev().find(|&y| pill(y)).expect("the handle's pill");
    let top = pill_y - 10;
    // (left of the dev readout, right of the 12 px corner)
    assert!(near(pixel(image, 20, top), theme::LINE, 3), "the sheet's border at {top}: {:?}", pixel(image, 20, top));
    for y in [top + 1, top + 8, top + 16] {
        assert!(near(pixel(image, 100, y), theme::SURFACE, 3), "the handle's fill at {y}: {:?}", pixel(image, 100, y));
    }
    top
}

#[track_caller]
fn within(what: &str, ours: u32, theirs: u32, tolerance: u32) {
    assert!(ours.abs_diff(theirs) <= tolerance, "{what}: ours {ours}, today {theirs}");
}

fn dark_state(startup: Startup) -> AppState {
    AppState::new(Startup { basemap: Some(Basemap::Dark), ..startup }, None, None, None, ViewState::INITIAL, SIZE.map(f64::from))
}

/// The phone layout over the V1 globe beside today's picture: the same places for the bar, the
/// field, the chosen mode, the sheet and its handle and head, the rail and the credits.
#[test]
fn the_phone_beside_today() {
    let Some(gpu) = gpu() else { return };
    let mut window = Window::new(&gpu, dark_state(Startup::default()), insets(0.0, 0.0));
    window.settle(&gpu, NOW_MS);
    let image = window.shot(&gpu, NOW_MS);
    save(&image, "u19-phone-v1.png");
    let today = reference("dark-v1-390x844.png");

    // the top bar's card: its border at top 8 and left 8, the search field's 5 px in
    for (what, x, y0, y1) in [("the card's top", 100, 0, 30), ("the field's top", 100, 10, 30)] {
        let c = if what.contains("card") { theme::LINE } else { theme::LINE_CONTROL };
        within(what, row_of(&image, x, y0, y1, c).expect(what), row_of(&today, x, y0, y1, c).expect(what), 0);
    }
    for (what, y, c) in [("the card's left", 30, theme::LINE), ("the field's left", 30, theme::LINE_CONTROL)] {
        within(what, column_of(&image, y, 0, 30, c).expect(what), column_of(&today, y, 0, 30, c).expect(what), 0);
    }
    // the chosen mode's cell (darkened by the readout over it in both): 287–330 × 14–49
    let blue = |p: [u8; 4]| i32::from(p[2]) - i32::from(p[0]) > 30;
    let area = Rect::new(250, 8, 140, 50);
    let (ours, theirs) = (bounds(&image, area, blue).expect("our cell"), bounds(&today, area, blue).expect("today's cell"));
    for (what, a, b) in [("left", ours.x0, theirs.x0), ("top", ours.y0, theirs.y0), ("right", ours.x1, theirs.x1), ("bottom", ours.y1, theirs.y1)] {
        within(&format!("the mode cell's {what}"), a, b, 1);
    }

    // the sheet at peek: 90 of content and its border, 753 down
    within("the sheet's top", sheet_top(&image), sheet_top(&today), 0);
    assert_eq!(sheet_top(&image), 753);
    // its handle's pill: 36 × 4 in the middle of the 22 px under the border
    let pill = |image: &Image| bounds(image, Rect::new(150, 754, 90, 22), |p| near(p, theme::LINE_CONTROL, 16)).expect("the pill");
    let (ours, theirs) = (pill(&image), pill(&today));
    assert_eq!((ours.x0, ours.y0, ours.x1, ours.y1), (theirs.x0, theirs.y0, theirs.x1, theirs.y1), "the pill");
    // the Filters button at the head's right, 16 in
    let button = |image: &Image| bounds(image, Rect::new(200, 776, 190, 40), |p| near(p, theme::LINE_CONTROL, 6)).expect("the Filters button's border");
    let (ours, theirs) = (button(&image), button(&today));
    for (what, a, b) in [("left", ours.x0, theirs.x0), ("top", ours.y0, theirs.y0), ("right", ours.x1, theirs.x1), ("bottom", ours.y1, theirs.y1)] {
        within(&format!("the Filters button's {what}"), a, b, 1);
    }

    // the rail 24 above the sheet, 8 from the right: its stacks' borders
    let rail = |image: &Image| bounds(image, Rect::new(330, 560, 60, 192), |p| near(p, theme::LINE, 2) || near(p, theme::SURFACE, 1)).expect("the rail");
    let (ours, theirs) = (rail(&image), rail(&today));
    for (what, a, b) in [("left", ours.x0, theirs.x0), ("top", ours.y0, theirs.y0), ("right", ours.x1, theirs.x1), ("bottom", ours.y1, theirs.y1)] {
        within(&format!("the rail's {what}"), a, b, 1);
    }
    // the credits on the sheet, across the window: their dark strip right above its border
    for image in [&image, &today] {
        let strip = pixel(image, 2, 747);
        assert!(lum(strip) < 26.0, "the strip's box: {strip:?}");
    }
    let text = |image: &Image| bounds(image, Rect::new(0, 740, 390, 13), |p| lum(p) > 70.0).expect("the credits' text");
    let (ours, theirs) = (text(&image), text(&today));
    within("the credits' first letter", ours.x0, theirs.x0, 1);
    within("the credits' top", ours.y0, theirs.y0, 1);
}

/// The list's sheet at each detent with a 47 px notch and a 34 px home bar.
#[test]
fn peek_half_and_full_with_the_safe_areas() {
    let Some(gpu) = gpu() else { return };
    let mut window = Window::new(&gpu, dark_state(Startup::default()), insets(47.0, 34.0));
    window.settle(&gpu, NOW_MS);
    // (content heights: 90; round(844 / 2); 844 − 64, held to 844 − 64 − 47 − 34)
    for (detent, content, name) in [(Detent::Peek, 90, "peek"), (Detent::Half, 422, "half"), (Detent::Full, 699, "full")] {
        window.engine.state.apply(Action::Sheet(detent), NOW_MS);
        let image = window.shot(&gpu, NOW_MS);
        save(&image, &format!("u19-sheet-{name}.png"));
        let top = 844 - 34 - content - 1;
        assert_eq!(sheet_top(&image), top, "the sheet's top at {name}");
        // the home bar's padding is the sheet's own fill, to the window's bottom
        assert_eq!(pixel(&image, 200, 843), rgb(theme::SURFACE), "under the content at {name}");
        // the bar under the notch: its border at 47
        assert_eq!(row_of(&image, 100, 0, 80, theme::LINE), Some(47), "the top bar at {name}");
        // the rail rides 24 above the sheet, 8 from the right (at full it is under the sheet)
        if detent != Detent::Full {
            let bottom = top - 24 - 1;
            assert_eq!(column_of(&image, bottom - 10, 340, 390, theme::LINE).map(|x| x + 38), Some(390 - 8), "the rail's left at {name}");
            assert_eq!(pixel(&image, 360, bottom), rgb(theme::LINE), "the rail's bottom border at {name}");
            assert_eq!(pixel(&image, 360, bottom - 1), rgb(theme::SURFACE), "inside the rail at {name}");
            assert!(!near(pixel(&image, 360, bottom + 1), theme::LINE, 3), "the rail's bottom at {name}");
        }
    }
}

/// A `--fixture` folder holding the synthetic snapshot, with details for flight `id`.
fn flight_folder(id: u32) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("u19-synthetic");
    fixtures::write_fixture(&dir, &fixtures::synthetic_snapshot(NOW_MS)).expect("the fixture folder");
    let details = FlightDetails { progress: Some(FlightProgress { traversed_distance: 300_000, remaining_distance: 400_000, progress_pct: 43, ..Default::default() }), ..Default::default() };
    let folder = dir.join("flight_details");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(format!("{id}.json")), serde_json::to_vec(&details).unwrap()).unwrap();
    dir
}

/// A flight's details in the sheet: lowered to the strip, the route and the footer, the bar gone;
/// raised, the tabs under the route.
#[test]
fn a_flight_in_the_sheet() {
    let Some(gpu) = gpu() else { return };
    let id = fixtures::FIRST_ID;
    let startup = Startup { fixture: Some(flight_folder(id)), ..Default::default() };
    let mut state = dark_state(startup);
    state.poll(NOW_MS);
    state.poll(NOW_MS + 16.0);
    assert!(state.aircraft.snapshot().is_some(), "the fixture's snapshot");
    let mut window = Window::new(&gpu, state, insets(47.0, 34.0));
    window.settle(&gpu, NOW_MS);
    window.engine.state.apply(Action::Sheet(Detent::Half), NOW_MS);
    window.frame(&gpu, NOW_MS);
    window.engine.state.select_flight(Some(id), SelectOptions::default(), NOW_MS);
    let image = window.shot(&gpu, NOW_MS + 100.0);
    save(&image, "u19-flight-peek.png");
    assert_eq!(window.engine.state.sheet.detent(), Detent::Peek, "a detail opens lowered");
    // the bar has slid away: nothing of its border under the notch
    assert_eq!(row_of(&image, 100, 40, 120, theme::LINE_CONTROL), None, "the search field");
    // the sheet holds the handle, the strip, the route and the footer: the strip's close button
    // near its top, the footer's top rule a footer above the home bar
    let top = sheet_top(&image);
    assert!(top > 400 && top < 700, "a peek, not half: {top}");
    let close = bounds(&image, Rect::new(340, top + 22, 50, 60), |p| lum(p) > 150.0).expect("the close button's X");
    assert!(close.y0 < top + 22 + 40, "the strip right under the handle: {close:?} under {top}");
    let rules: Vec<u32> = (top + 23..844 - 34).filter(|&y| near(pixel(&image, 200, y), theme::LINE, 2)).collect();
    assert!(rules.len() >= 3, "the strip's, the route's and the footer's rules: {rules:?}");
    let footer_rule = *rules.last().expect("the footer's rule");
    assert!((844 - 34 - footer_rule) > 40 && (844 - 34 - footer_rule) < 70, "the footer at the bottom: {rules:?}");

    // raised: the tabs follow the route (doc 06 §7.6), the photo after them
    window.engine.state.apply(Action::Sheet(Detent::Full), NOW_MS);
    let full = window.shot(&gpu, NOW_MS + 200.0);
    save(&full, "u19-flight-full.png");
    // full, held clear of the notch: 844 − 12 − 47 − 34 of content (its handle under the readout)
    let raised = 844 - 34 - (844 - 12 - 47 - 34) - 1;
    assert_eq!(pixel(&full, 20, raised), rgb(theme::LINE), "the sheet's border");
    assert_eq!(pixel(&full, 20, raised + 2), rgb(theme::SURFACE), "the handle");
    assert!(!near(pixel(&full, 20, raised - 1), theme::SURFACE, 3), "above the sheet");
    let rules_full: Vec<u32> = (raised + 23..844 - 34).filter(|&y| near(pixel(&full, 200, y), theme::LINE, 2)).collect();
    // the strip's and the route's rules where they were at peek, relative to the sheet's top
    for (a, b) in rules.iter().zip(&rules_full).take(2) {
        assert_eq!(a - top, b - raised, "the summary first: {rules:?} at peek, {rules_full:?} raised");
    }
}

/// The Settings menu as a sheet of its own along the bottom, over the list's sheet, with its keys
/// entry.
#[test]
fn a_menu_as_a_sheet() {
    let Some(gpu) = gpu() else { return };
    let mut window = Window::new(&gpu, dark_state(Startup::default()), insets(47.0, 34.0));
    window.settle(&gpu, NOW_MS);
    window.engine.state.apply(Action::SetMenu(Some(Menu::Settings)), NOW_MS);
    let image = window.shot(&gpu, NOW_MS);
    save(&image, "u19-menu.png");
    // full width, its top border with the 12 px corners, its fill to the window's bottom
    // (its left border runs up the window's edge to its 12 px corner)
    let side = (0..843).rev().take_while(|&y| near(pixel(&image, 0, y), theme::LINE, 3)).last().expect("its left border");
    let top = (side - 14..side).find(|&y| near(pixel(&image, 200, y), theme::LINE, 3)).expect("its top border");
    assert_eq!(side - top, 12, "the top-left corner, 12 px round");
    assert!(top < 844 - 200, "a menu's height: {top}");
    assert_eq!(pixel(&image, 389, top + 40), rgb(theme::LINE), "its right border");
    assert_eq!(pixel(&image, 200, 843), rgb(theme::LINE), "its bottom border at the window's bottom");
    // the keys entry: a button with --line-control's border in the lower part
    let keys = bounds(&image, Rect::new(16, top, 360, 844 - 34 - top), |p| near(p, theme::LINE_CONTROL, 3));
    assert!(keys.is_some(), "the controls of the menu");
}

/// A train's details in the sheet: the strip, the journey and the footer at `peek`, the footer
/// with Follow alone (the source is credited in the facts and along the bottom of the map).
#[test]
fn a_train_in_the_sheet() {
    let Some(gpu) = gpu() else { return };
    let mut state = dark_state(Startup { demo: true, ..Default::default() });
    state.persisted.settings.mode = Mode::Trains;
    for t in [NOW_MS, NOW_MS + 16.0, NOW_MS + 32.0] {
        state.poll(t);
    }
    assert!(state.trains.loaded() && !state.trains.trains().is_empty(), "the demo trains");
    let mut window = Window::new(&gpu, state, insets(47.0, 34.0));
    window.settle(&gpu, NOW_MS);
    assert!(window.engine.state.apply(Action::PickTrain(0), NOW_MS));
    let image = window.shot(&gpu, NOW_MS + 100.0);
    save(&image, "u19-train-peek.png");
    let top = sheet_top(&image);
    assert!(top > 400 && top < 750, "a peek: {top}");
    // the footer's buttons: Follow alone at the right (with the source's button they would reach
    // past the middle)
    // (a button's side: a column of --line-control down most of its 32 px)
    let sides: Vec<u32> = (0..390).filter(|&x| (844 - 34 - 56..844 - 34).filter(|&y| near(pixel(&image, x, y), theme::LINE_CONTROL, 3)).count() >= 20).collect();
    assert_eq!(sides.len(), 2, "one button's two sides: {sides:?}");
    assert!(sides[0] > 240 && sides[1] == 390 - 16 - 1, "Follow at the right: {sides:?}");
}

/// A phone on its side: the bar and the sheet down the left, 320 wide, clear of the side notch;
/// the rail 24 from the bottom at the right.
#[test]
fn on_its_side() {
    let Some(gpu) = gpu() else { return };
    let size = [844, 390];
    let side = SafeAreaInsets(egui::epaint::MarginF32 { left: 47.0, right: 47.0, top: 0.0, bottom: 21.0 });
    let state = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, ViewState::INITIAL, size.map(f64::from));
    let mut window = Window::sized(&gpu, state, side, size);
    window.settle(&gpu, NOW_MS);
    let image = window.shot(&gpu, NOW_MS);
    save(&image, "u19-landscape.png");
    // the bar's card: left 47, top 8, 320 wide
    assert_eq!(column_of(&image, 30, 0, 100, theme::LINE), Some(47), "the bar's left");
    assert_eq!(row_of(&image, 100, 0, 30, theme::LINE), Some(8), "the bar's top");
    assert_eq!(pixel(&image, 47 + 319, 30), rgb(theme::LINE), "the bar's right");
    // the sheet's handle in the middle of its 320 px, the home bar's 21 under its content
    // (90 of content, the home bar's 21 and the border: its top at 390 − 112, the pill 10 under)
    let top = 390 - 112;
    assert_eq!(pixel(&image, 100, top), rgb(theme::LINE), "the sheet's border");
    let pill = bounds(&image, Rect::new(47, top + 1, 320, 21), |p| near(p, theme::LINE_CONTROL, 16)).expect("the handle's pill");
    assert_eq!((pill.x0, pill.y0, pill.x1, pill.y1), (47 + 160 - 18, top + 10, 47 + 160 + 18, top + 14), "{pill:?}");
    assert_eq!(pixel(&image, 200, 389), rgb(theme::SURFACE), "under the content");
    assert!(!near(pixel(&image, 47 + 330, 389), theme::SURFACE, 3), "the map right of the sheet");
    // the rail at the right, 24 from the bottom
    let bottom = 390 - 24 - 1;
    assert_eq!(pixel(&image, 844 - 47 - 20, bottom), rgb(theme::LINE), "the rail's bottom border");
    assert_eq!(pixel(&image, 844 - 47 - 1, bottom - 10), rgb(theme::LINE), "the rail's right border");
    // the credits at the window's bottom right, clear of 360 px, under the home bar
    let credits = bounds(&image, Rect::new(360, 375, 484, 15), |p| lum(p) > 70.0).expect("the credits' text");
    assert!(credits.y1 >= 386 && credits.x1 > 830, "{credits:?}");
}

/// The ships' phone layout over V6 on the satellite map beside today's `satellite-v6-390x844.png`:
/// the phone's placeholder, the ships' cell chosen, and in the sheet at peek the list's sort right
/// under the head (the list's header is the sheet's head on a phone).
#[test]
fn ships_beside_today() {
    let Some(gpu) = gpu() else { return };
    let v6 = ViewState { longitude: 11.93, latitude: 57.70, zoom: 13.0, bearing: 0.0, pitch: 0.0, position: None };
    let mut state = AppState::new(Startup { basemap: Some(Basemap::Satellite), ..Default::default() }, None, None, None, v6, SIZE.map(f64::from));
    state.persisted.settings.mode = Mode::Ships;
    let mut window = Window::new(&gpu, state, insets(0.0, 0.0));
    window.settle(&gpu, NOW_MS);
    let image = window.shot(&gpu, NOW_MS);
    save(&image, "u19-ships-v6.png");
    let today = reference("satellite-v6-390x844.png");
    // the placeholder `Ship or address`: its ink where today's is
    let words = Rect::new(45, 18, 135, 26);
    let ink = |image: &Image| bounds(image, words, |p| lum(p) > 110.0).expect("the placeholder");
    let (ours, theirs) = (ink(&image), ink(&today));
    for (what, a, b) in [("left", ours.x0, theirs.x0), ("right", ours.x1, theirs.x1), ("top", ours.y0, theirs.y0), ("bottom", ours.y1, theirs.y1)] {
        within(&format!("the placeholder's {what}"), a, b, 1);
    }
    // the ships' cell chosen at the row's right end (under the readout in both)
    let blue = |p: [u8; 4]| i32::from(p[2]) - i32::from(p[0]) > 30;
    let area = Rect::new(250, 8, 140, 50);
    let (ours, theirs) = (bounds(&image, area, blue).expect("our cell"), bounds(&today, area, blue).expect("today's cell"));
    for (what, a, b) in [("left", ours.x0, theirs.x0), ("right", ours.x1, theirs.x1)] {
        within(&format!("the ships' cell's {what}"), a, b, 1);
    }
    // the sheet at peek, and the sort's box under its head
    assert_eq!(sheet_top(&image), sheet_top(&today));
    let sort_top = |image: &Image| (812..844).find(|&y| near(pixel(image, 100, y), theme::LINE_CONTROL, 6)).expect("the sort's border");
    within("the sort's top", sort_top(&image), sort_top(&today), 1);
}
