//! Item U8's acceptance (plan stage 7; doc 06 §5): the search box in the window's shell, from
//! canned searches in a `--fixture` folder (`search_find.json`, `search_places.json`).
//!
//! - `search-dropdown` (`target/shots/u8-search-dropdown.png`): `/` focuses the field, `sk1` is
//!   typed, the two searches settle after their debounces, and the dropdown hangs 8 px under the
//!   field and as wide (x 33–319), `--surface-raised` in its `--line` border, with `Flights and
//!   airports` over two live flights (the `Plane` in `--amber`) and an airport, the first row
//!   highlighted (`--surface-hover`), a `--line` rule 4 px under them, and `Places` with
//!   Photon's credit over two places;
//! - the closed field where today's `dark-v1` has it: its border rows, the `Search` icon, the
//!   placeholder and the `/` key cap within a pixel;
//! - `search-ships` (`target/shots/u8-search-ships.png`): the ships' local search: `Ships in
//!   view` at once with the ships' rows in their map colours, the clear button in the field;
//! - `search-messages` (`target/shots/u8-search-messages.png`): a failed flight search and no
//!   places, as wrapped messages.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::NOW_MS;
use common::shots::{Rect, bounds, components, count, gpu, pixel, save};
use egui::{Color32, Event, Key, Pos2, RawInput, Vec2};
use rustradar_app::app::Engine;
use rustradar_app::state::search::{FIXTURE_FIND, FIXTURE_PLACES};
use rustradar_app::state::{AppState, Startup};
use rustradar_app::ui::theme;
use rustradar_core::geo::Basemap;
use rustradar_core::model::Ship;
use rustradar_core::settings::Mode;
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_map::io::DirTiles;
use rustradar_scene::camera::ViewState;

/// The desktop reference pictures' canvas, CSS px, at scale 1.
const SIZE: [u32; 2] = [1440, 900];

/// What Flightradar24's search answers for `sk1`: two live flights, an operator (not shown) and
/// an airport.
const FIND: &str = r#"[
  {"id": "3a4b5c6", "label": "SK1415 / SAS1415", "type": "live", "match": "begins",
   "detail": {"lat": 59.65, "lon": 17.93, "route": "Stockholm (ARN) - Copenhagen (CPH)", "ac_type": "A20N", "callsign": "SAS1415", "flight": "SK1415", "reg": "SE-ROM"}},
  {"id": "3a4b5d1", "label": "SK1417 / SAS1417", "type": "live", "match": "begins",
   "detail": {"lat": 55.6, "lon": 12.6, "route": null, "ac_type": "CRJ9", "callsign": "SAS1417", "flight": "SK1417"}},
  {"id": "SAS", "label": "SAS Scandinavian Airlines", "type": "operator", "detail": {"operator_id": 72, "iata": "SK", "logo": null}},
  {"id": "SKX", "label": "Skelleftea Airport (SFT / ESNS)", "type": "airport", "detail": {"lat": 64.62, "lon": 21.08, "size": 10}}
]"#;

/// What Photon answers: two places.
const PLACES: &str = r#"[
  {"name": "Skansen", "label": "Skansen, Djurgardsslatten, 115 21 Stockholm, Sweden", "kind": "poi", "category": "Museum", "lat": 59.326, "lon": 18.104},
  {"name": "Skarholmen", "label": "Skarholmen, Stockholm, Sweden", "kind": "district", "lat": 59.277, "lon": 17.907}
]"#;

/// A fixture folder `name` with the synthetic snapshot, and the searches' answers that are given.
fn fixture(name: &str, find: Option<&str>, places: Option<&str>, ships: &[Ship]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    common::fixtures::write_fixture(&dir, &common::fixtures::synthetic_snapshot(NOW_MS)).expect("the fixture folder");
    for (file, body) in [(FIXTURE_FIND, find), (FIXTURE_PLACES, places)] {
        if let Some(body) = body {
            std::fs::write(dir.join(file), body).expect("a search file");
        }
    }
    if !ships.is_empty() {
        std::fs::write(dir.join("ships_in_view.json"), serde_json::to_vec(ships).unwrap()).expect("the ships");
    }
    dir
}

/// The window's frames, drawn offscreen, with egui's events; the local clock 100 ms on a frame.
struct Window {
    engine: Engine,
    target: Offscreen,
    egui_s: f64,
    now: f64,
}

impl Window {
    fn new(gpu: &Gpu, dir: PathBuf, mode: Mode, view: ViewState) -> Window {
        let startup = Startup { basemap: Some(Basemap::Dark), fixture: Some(dir), demo: false };
        let mut state = AppState::new(startup, None, None, None, view, SIZE.map(f64::from));
        state.persisted.settings.mode = mode;
        let mut engine = Engine::new(gpu, FORMAT, state);
        engine.map.set_tiles(Arc::new(DirTiles::fixtures()), None);
        engine.shell = true;
        engine.draw = false;
        Window { engine, target: Offscreen::new(gpu, SIZE), egui_s: 0.0, now: NOW_MS }
    }

    fn frame(&mut self, gpu: &Gpu, events: Vec<Event>) {
        let mut input = RawInput {
            time: Some(self.egui_s),
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(SIZE[0] as f32, SIZE[1] as f32))),
            events,
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(1.0);
        self.egui_s += 0.1;
        self.now += 100.0;
        self.engine.frame(gpu, self.target.view(), SIZE, 1.0, input, self.now);
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// `/`, then `text` typed into the field, then frames past both debounces and until the map
    /// has its tiles; then a drawn frame: its picture.
    fn search(&mut self, gpu: &Gpu, text: &str) -> Image {
        self.frame(gpu, Vec::new());
        self.frame(gpu, vec![Event::Key { key: Key::Slash, physical_key: Some(Key::Slash), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }, Event::Text("/".into())]);
        self.frame(gpu, vec![Event::Text(text.into())]);
        self.settle(gpu)
    }

    fn settle(&mut self, gpu: &Gpu) -> Image {
        let started = std::time::Instant::now();
        for _ in 0..6 {
            self.frame(gpu, Vec::new());
        }
        while self.engine.map.pending() && started.elapsed() < rustradar_map::headless::MAX_WAIT {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.frame(gpu, Vec::new());
        }
        self.engine.draw = true;
        self.frame(gpu, Vec::new());
        self.engine.draw = false;
        self.target.read(gpu).expect("read back")
    }
}

fn rgb(c: Color32) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

/// Within 2 levels of `c`.
fn is(c: Color32) -> impl Fn([u8; 4]) -> bool {
    move |p| (0..3).all(|i| p[i].abs_diff(rgb(c)[i]) <= 2)
}

fn lum(p: [u8; 4]) -> f64 {
    (f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])) / 3.0
}

/// The dropdown's fill: the largest patch of `--surface-raised` under the field (the map and the
/// cards' antialiased corners have stray pixels of it).
/// Its sides are read along a row in its top padding, its top and bottom down a column in its
/// left padding, clear of the rounded corners' antialiasing.
fn dropdown_fill(image: &Image) -> Option<Rect> {
    let patch = components(image, Rect::new(20, 100, 320, 700), is(theme::SURFACE_RAISED)).into_iter().max_by_key(|c| c.pixels)?.bounds;
    let fill = is(theme::SURFACE_RAISED);
    let y = patch.y0 + 6;
    let x0 = (patch.x0..patch.x1).find(|&x| fill(pixel(image, x, y)))?;
    let x1 = (patch.x0..patch.x1).rev().find(|&x| fill(pixel(image, x, y)))? + 1;
    let x = x0 + 2;
    let y0 = (patch.y0..patch.y1).find(|&y| fill(pixel(image, x, y)))?;
    let y1 = (patch.y0..patch.y1).rev().find(|&y| fill(pixel(image, x, y)))? + 1;
    Some(Rect { x0, y0, x1, y1 })
}

/// The highlighted row in `fill`: its top and bottom down a column in its left padding, its sides
/// along a row in its top padding (text antialiasing has pixels of `--surface-hover` too).
fn highlighted(image: &Image, fill: Rect) -> Option<Rect> {
    let hover = is(theme::SURFACE_HOVER);
    let x = fill.x0 + 7;
    let y0 = (fill.y0..fill.y1).find(|&y| hover(pixel(image, x, y)))?;
    let y1 = (y0..fill.y1).find(|&y| !hover(pixel(image, x, y))).unwrap_or(fill.y1);
    let y = y0 + 6;
    let x0 = (fill.x0..fill.x1).find(|&x| hover(pixel(image, x, y)))?;
    let x1 = (x0..fill.x1).find(|&x| !hover(pixel(image, x, y))).unwrap_or(fill.x1);
    Some(Rect { x0, y0, x1, y1 })
}

/// The bounds of the pixels in `area` brighter than `under` by more than 30 levels: text and icons.
fn ink(image: &Image, area: Rect, under: Color32) -> Option<Rect> {
    bounds(image, area, |p| lum(p) - lum(rgb(under)) > 30.0)
}

/// The rows of `area` (top to bottom) holding ink over `under`, as runs `[start, end)`.
fn ink_rows(image: &Image, area: Rect, under: Color32) -> Vec<[u32; 2]> {
    let mut runs: Vec<[u32; 2]> = Vec::new();
    for y in area.y0..area.y1 {
        let inked = (area.x0..area.x1).any(|x| lum(pixel(image, x, y)) - lum(rgb(under)) > 30.0);
        match runs.last_mut() {
            Some(run) if inked && run[1] == y => run[1] = y + 1,
            _ if inked => runs.push([y, y + 1]),
            _ => {}
        }
    }
    runs
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
fn search_dropdown() {
    let Some(gpu) = gpu() else { return };
    let dir = fixture("u8-search", Some(FIND), Some(PLACES), &[]);
    let mut w = Window::new(&gpu, dir, Mode::Aircraft, ViewState::INITIAL);
    let image = w.search(&gpu, "sk1");
    save(&image, "u8-search-dropdown.png");
    let search = &w.engine.state.search;
    assert_eq!(search.query(Mode::Aircraft), "sk1");
    assert_eq!(search.flights().results().len(), 3, "the operator left out");
    assert_eq!(search.places().results().len(), 2);

    // the field focused: its border in the accent, rows 69 and 106 as today's
    assert!(is(theme::AMBER)(pixel(&image, 150, 69)) && is(theme::AMBER)(pixel(&image, 150, 106)), "{:?} {:?}", pixel(&image, 150, 69), pixel(&image, 150, 106));
    // the dropdown: its fill from x 34 to 318 inside the border, its top 8 under the field
    let fill = dropdown_fill(&image).expect("the dropdown's fill");
    assert_eq!((fill.x0, fill.x1), (34, 318), "{fill:?}");
    assert_eq!(fill.y0, 116, "the border at 115: 8 under the field's 106.8 ({fill:?})");
    assert!(is(theme::LINE)(pixel(&image, 150, 115)), "its top border: {:?}", pixel(&image, 150, 115));
    assert!(is(theme::LINE)(pixel(&image, 33, 200)) && is(theme::LINE)(pixel(&image, 318, 200)), "its sides");
    // the first row highlighted, inside the padding, under the 28.8 px heading
    let active = highlighted(&image, fill).expect("the highlighted row");
    assert_eq!((active.x0, active.x1), (38, 314), "{active:?}");
    assert!(active.y0.abs_diff(116 + 4 + 29) <= 1, "{active:?}");
    assert!(active.height().abs_diff(51) <= 1, "label and detail: 8 + 35 + 8 ({active:?})");
    // its tile: --surface, 26 square, 8 in, the Plane in the accent
    let tile = bounds(&image, Rect::new(active.x0, active.y0, 40, active.height()), is(theme::SURFACE)).expect("the tile");
    assert_eq!((tile.x0, tile.width(), tile.height()), (46, 26, 26), "{tile:?}");
    assert!(count(&image, tile, |p| p[2] > 180 && p[0] < 120) > 20, "the Plane in --amber");
    // the label and detail start 12 after the tile
    let text = ink(&image, Rect::new(tile.x1, active.y0, 314 - tile.x1, active.height()), theme::SURFACE_HOVER).expect("the row's text");
    assert!(text.x0.abs_diff(tile.x1 + 12) <= 1, "{text:?} after {tile:?}");
    // the heading over the rows
    let heading = ink(&image, Rect::new(40, 116, 270, 30), theme::SURFACE_RAISED).expect("the heading");
    assert!(heading.x0.abs_diff(34 + 4 + 8) <= 1, "{heading:?}");

    // the places: a rule across the content under the third row, then their heading and the
    // credit on its right
    let rule = (active.y1..600).find(|&y| (40..310).all(|x| is(theme::LINE)(pixel(&image, x, y)))).expect("the rule above the places");
    assert!(rule.abs_diff(active.y1 + 51 + 51 + 4) <= 1, "two more rows of 51 (each with its detail: the type, `Airport`), 4 down: {rule}");
    let places = ink(&image, Rect::new(40, rule + 1, 270, 30), theme::SURFACE_RAISED).expect("Places and the credit");
    assert!(places.x1 >= 305, "the credit runs to the right padding: {places:?}");
    let bottom = fill.y1;
    assert!(bottom > rule + 29 + 2 * 51, "two place rows under it: {fill:?}");
    // the map shows round it
    assert!(!is(theme::SURFACE_RAISED)(pixel(&image, 330, 200)));
}

#[test]
fn the_closed_field_stands_where_today_s_does() {
    let Some(gpu) = gpu() else { return };
    let dir = fixture("u8-search-closed", None, None, &[]);
    let mut w = Window::new(&gpu, dir, Mode::Aircraft, ViewState::INITIAL);
    let image = w.settle(&gpu);
    save(&image, "u8-search-closed.png");
    let today = reference("dark-v1-1440x900.png");
    // the border rows and columns
    let border = |img: &Image, x: u32, y: u32| lum(pixel(img, x, y));
    for (what, x, y) in [("top", 150, 69), ("bottom", 150, 106), ("left", 33, 88), ("right", 318, 88)] {
        let (ours, theirs) = (border(&image, x, y), border(&today, x, y));
        assert!((ours - theirs).abs() <= 8.0, "the field's {what} border: {ours} here, {theirs} today");
    }
    // the icon, the placeholder and the key cap, each within a pixel of today's
    for (what, r) in [("the icon", Rect::new(40, 75, 22, 25)), ("the placeholder", Rect::new(64, 75, 216, 25)), ("the key cap", Rect::new(294, 72, 24, 32))] {
        let (ours, theirs) = (ink(&image, r, theme::INK).expect("ours"), ink(&today, r, theme::INK).expect("today's"));
        for (a, b, edge) in [(ours.x0, theirs.x0, "left"), (ours.x1, theirs.x1, "right"), (ours.y0, theirs.y0, "top"), (ours.y1, theirs.y1, "bottom")] {
            assert!(a.abs_diff(b) <= 1, "{what}'s {edge}: {ours:?}, today {theirs:?}");
        }
    }
}

fn ship(mmsi: u32, name: &str, kind: u8, lat: f64) -> Ship {
    Ship { mmsi, lat, lon: 18.07, name: name.into(), kind, size: 2, ..Default::default() }
}

#[test]
fn search_ships() {
    let Some(gpu) = gpu() else { return };
    let ships = [ship(265_100_001, "STENA DANICA", 3, 59.33), ship(265_100_002, "STENA JUTLANDICA", 3, 59.34), ship(265_100_003, "STENA SCANDICA", 6, 59.35), ship(265_100_004, "BIRKA", 3, 59.36)];
    let dir = fixture("u8-search-ships", None, None, &ships);
    let view = ViewState { latitude: 59.33, longitude: 18.07, zoom: 9.0, ..ViewState::INITIAL };
    let mut w = Window::new(&gpu, dir, Mode::Ships, view);
    let image = w.search(&gpu, "stena");
    save(&image, "u8-search-ships.png");
    assert_eq!(w.engine.state.search.list_query(Mode::Ships), "stena", "the ships' query narrows their list");
    // the dropdown: the heading, three rows (the places failed: no places file), the first
    // highlighted
    let fill = dropdown_fill(&image).expect("the dropdown");
    assert_eq!((fill.x0, fill.x1, fill.y0), (34, 318, 116), "{fill:?}");
    let active = highlighted(&image, fill).expect("the highlighted row");
    assert!(active.height().abs_diff(51) <= 1, "{active:?}");
    // each row's tile holds the ship's colour (a passenger ship's blue, the tanker's red)
    let passenger = [98u8, 152, 240];
    let tanker = [218u8, 92, 92];
    let near = |want: [u8; 3]| move |p: [u8; 4]| (0..3).all(|i| p[i].abs_diff(want[i]) <= 24);
    assert!(count(&image, Rect::new(46, active.y0, 26, active.height()), near(passenger)) > 10, "the first ship's icon");
    assert!(count(&image, Rect::new(46, active.y0 + 102, 26, 51), near(tanker)) > 10, "the tanker's icon, third");
    // the clear button's X in the field: the 22 px button reaches 4 px into the padding, so it is
    // centred at 306 + 4 - 11 = 299 and the X's ink (3.5 px in from its 14 px box) is 296–303
    let x = ink(&image, Rect::new(286, 75, 30, 25), theme::INK).expect("the clear button");
    assert!(x.x0.abs_diff(296) <= 1 && x.x1.abs_diff(303) <= 1, "{x:?}");
    // Places unavailable: the second group's quiet message
    let rows = ink_rows(&image, Rect::new(46, active.y0 + 153, 260, fill.y1 - active.y0 - 153), theme::SURFACE_RAISED);
    assert!(rows.len() >= 2, "the Places heading and its message: {rows:?}");
}

#[test]
fn search_messages() {
    let Some(gpu) = gpu() else { return };
    // no search files: the flight search fails; Photon finds nothing
    let dir = fixture("u8-search-messages", None, Some("[]"), &[]);
    let mut w = Window::new(&gpu, dir, Mode::Aircraft, ViewState::INITIAL);
    let image = w.search(&gpu, "zzzz");
    save(&image, "u8-search-messages.png");
    let fill = dropdown_fill(&image).expect("the dropdown");
    // the heading, the failure on two lines, the rule, the heading, the one-line message
    let rows = ink_rows(&image, Rect::new(46, fill.y0, 260, fill.height()), theme::SURFACE_RAISED);
    assert_eq!(rows.len(), 5, "{rows:?}");
    // 8 + 16.8 + 4, 8 + 2 × 18.2 + 12, 4 + 1 rule, 28.8, 8 + 18.2 + 12, and the padding and border
    let want = 2.0 + 8.0 + 28.8 + 56.4 + 5.0 + 28.8 + 38.2;
    assert!((f64::from(fill.height()) + 2.0 - want).abs() <= 2.0, "{fill:?}: {want}");
    assert!(highlighted(&image, fill).is_none(), "no row to highlight");
}
