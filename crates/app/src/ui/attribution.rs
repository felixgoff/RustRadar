//! The attribution strip and the dev readout (doc 06 §2.6, §2.7, §24.4–24.5; `+page.svelte:1267-1299`,
//! `:1311-1327`, `:1745-1758`, `:1898-1934`, `:2004-2023`).
//!
//! **The strip** ([`show`]): one line along the bottom right of the window (`right: 0; bottom: 0;
//! max-width: calc(100% - 352px)`), `1px 4px` of padding, its top-left corner rounded 4, on
//! `oklch(0.15 0 0 / 0.6)`, 8 px Overpass at line height 1.3 in `--text-3`, cut with an ellipsis
//! when it does not fit (`white-space: nowrap; overflow: hidden; text-overflow: ellipsis`). Its
//! tooltip (`title`) is the whole text. What it says ([`Attribution::of`]): first the credits of the
//! trains on the map (`trainCredits(trainsOnMap)` while trains are on it), so the ellipsis cannot
//! hide them, as links (`--text-2`, underlined in white at 25 % 2 px under the baseline, `--text-1`
//! under the pointer, a click opens the page in the system's browser); then, in the page's order,
//! each credit whose condition holds ([`credits`], the 18 rows of doc 06 §2.6), all joined by
//! `" · "`. On phones ([`Place`]) it sits on the bottom sheet across the window at 7 px with square
//! corners, or (landscape) at the bottom right clear of 360 px; item U19 places it there.
//!
//! **The dev readout** ([`dev_readout`], debug builds only: `import.meta.env.DEV`): a `<pre>` at
//! top 8, right 8, `4px 6px` of padding, radius 4, on `oklch(0.15 0 0 / 0.75)`, 10 px monospace at
//! line height 1.35 in `--text-2`, under the pointer's notice (`pointer-events: none`), over the
//! sidebar, the panels and the map controls (z-index 5): the view, the basemap (`· Streets GL` close
//! in on the realistic map) with the frames a second, and how many aircraft, trains and ships are
//! loaded ([`readout_lines`]).
//!
//! [`ui`] draws both on the desktop; the window's UI pass calls it after the shell.
//!
//! Deviations:
//! - "Weather: MET Norway" (row 17) needs the live weather, which `state::weather` does not hold
//!   until item R11: [`Conditions::of`] reads it as not loaded, so the credit is not shown yet.
//! - The readout's monospace is Overpass Mono (today's `var(--font-mono, ui-monospace, monospace)`
//!   names no font the app has: the system's monospace), about 2 % wider.
//! - Its frames a second count the frames the window draws: the app draws when something changes,
//!   not at the display's rate as `requestAnimationFrame` does, so a still map reads low. The count
//!   is taken when a frame is drawn; with nothing drawn it keeps the last figure, as today's does
//!   while the page is hidden.
//! - A train credit's link opens through egui's `OpenUrl`, which the desktop window hands to the
//!   system's browser (egui-winit's `links`); phones need the platform glue's opener (U20, U19).
//! - The underline is 1 px (Chromium takes the font's underline thickness, rounded to at least a
//!   device pixel).

use std::borrow::Cow;
use std::sync::Arc;

use egui::text::{LayoutJob, TextWrapping};
use egui::{Area, Color32, Context, CornerRadius, CursorIcon, Galley, Id, Order, Pos2, Rect, Sense, Ui, Vec2};
use rustradar_core::geo::Basemap;
use rustradar_core::num::to_fixed;
use rustradar_core::settings::shown_kinds;
use rustradar_core::text::group_thousands;
use rustradar_core::trains::{Credit, train_credits};
use rustradar_core::weather::Sky;
use rustradar_scene::camera::ViewState;
use rustradar_scene::canopy::CANOPY_ATTRIBUTION;
use rustradar_scene::clouds::CLOUDS_ATTRIBUTION;
use rustradar_scene::hillshade::TERRAIN_ATTRIBUTION;
use rustradar_scene::landcover::LANDCOVER_ATTRIBUTION;
use rustradar_scene::overture::OVERTURE_ATTRIBUTION;
use rustradar_scene::rail::rail_attribution;
use rustradar_scene::street_trees::STREET_TREES_ATTRIBUTION;
use rustradar_scene::streets_gl::{SGL_ATTRIBUTION, SGL_ZOOM};

use super::text::{self, Style};
use super::theme;
use super::widgets::tooltip;
use crate::state::AppState;

/// Between two credits.
pub const SEPARATOR: &str = " · ";
/// Row 1, signed out.
pub const FLIGHT_DATA: &str = "Flight data: Flightradar24 (unofficial API)";
/// Row 1, signed in.
pub const FLIGHT_DATA_SIGNED_IN: &str = "Flight data: Flightradar24 (unofficial API, signed in)";
/// Row 2: DWD's open data licence asks to be named wherever its radar is shown.
pub const RADAR: &str = "Radar: NOAA MRMS, Deutscher Wetterdienst, RainViewer";
/// Row 3.
pub const IMAGERY: &str = "Imagery: Esri, Maxar, Earthstar Geographics";
/// Row 4.
pub const OPENSKY_TRACK: &str = "Track: OpenSky Network";
/// Row 5: ODbL asks for attribution wherever the data is shown.
pub const ADSB_LOL: &str = "Aircraft data: © adsb.lol contributors, ODbL";
/// Row 6.
pub const SHIPS: &str = "Ships © VesselFinder";
/// Row 9.
pub const TEXTURES: &str = "Textures: Streets GL (MIT)";
/// Row 14 (Globe's `ROAD_WIDTHS_ZOOM`; the app asks with the Trafikverket key).
pub const ROAD_WIDTHS: &str = "Road widths: Trafikverket NVDB";
/// Row 15 with the day and night on.
pub const EARTH_DAY_NIGHT: &str = "Earth: NASA Blue Marble, Black Marble";
/// Row 15 with the night off.
pub const EARTH_DAY: &str = "Earth: NASA Blue Marble";
/// Row 17.
pub const WEATHER: &str = "Weather: MET Norway";
/// Row 18: always.
pub const MAP: &str = "Map, names and buildings: OpenFreeMap, © OpenMapTiles, © OpenStreetMap contributors";

/// From this zoom the realistic map's land cover is credited (row 12).
pub const LANDCOVER_ZOOM: f64 = 12.0;
/// From this zoom the trees close in are (row 13: Globe's `TREES_ZOOM`).
pub const TREES_ZOOM: f64 = 14.5;
/// From this zoom the road widths are (row 14).
pub const ROAD_WIDTHS_ZOOM: f64 = 14.0;
/// Below this zoom the clouds are (row 16: from about 15 km up).
pub const CLOUDS_ZOOM: f64 = 11.0;

/// The strip's text size on the desktop (`font-size: 8px`).
pub const SIZE: f32 = theme::TEXT_ATTRIBUTION;
/// On phones (`font-size: 7px`).
pub const SIZE_PHONE: f32 = theme::TEXT_ATTRIBUTION_PHONE;
/// `line-height: 1.3`.
pub const LINE_HEIGHT: f32 = 1.3;
/// `padding: 1px var(--space-1)`: across, down.
pub const PADDING: Vec2 = Vec2::new(theme::SPACE_1, 1.0);
/// What the strip leaves clear on the left of the window (`max-width: calc(100% - 352px)`).
pub const CLEAR: f32 = 352.0;
/// On a phone on its side (`max-width: calc(100% - 360px)`).
pub const CLEAR_LANDSCAPE: f32 = 360.0;
/// A link's underline below the baseline (`text-underline-offset: 2px`).
pub const UNDERLINE_OFFSET: f32 = 2.0;
/// The underline's thickness.
pub const UNDERLINE_WIDTH: f32 = 1.0;

/// The readout's place: top and right (`var(--space-2)`).
pub const READOUT_INSET: f32 = theme::SPACE_2;
/// Its padding (`4px 6px`): across, down.
pub const READOUT_PADDING: Vec2 = Vec2::new(6.0, 4.0);

/// What the strip's credits depend on: the conditions of doc 06 §2.6's table, as the page reads them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Conditions {
    /// Aircraft on the map (`aircraftOn`).
    pub aircraft_on: bool,
    /// Signed in to Flightradar24 (`authenticated`).
    pub authenticated: bool,
    /// `settings.weather`: the radar.
    pub weather: bool,
    pub basemap: Basemap,
    /// OpenSky's points in front of the selected flight's trail (`merged.added`).
    pub opensky_added: usize,
    /// adsb.lol's measured data on show (`measured`).
    pub measured: bool,
    /// Ships on the map (`shipsOn`).
    pub ships_on: bool,
    /// `settings.railMap`.
    pub rail_map: bool,
    /// `settings.terrain`.
    pub terrain: bool,
    /// The view's zoom (`view.zoom`).
    pub zoom: f64,
    /// `settings.daylight`.
    pub daylight: bool,
    /// `settings.sky`.
    pub sky: Sky,
    /// The live weather has arrived (`liveWeather`).
    pub live_weather: bool,
}

impl Default for Conditions {
    /// Nothing on but the defaults' basemap, at zoom 0.
    fn default() -> Self {
        Conditions {
            aircraft_on: false,
            authenticated: false,
            weather: false,
            basemap: Basemap::Dark,
            opensky_added: 0,
            measured: false,
            ships_on: false,
            rail_map: false,
            terrain: false,
            zoom: 0.0,
            daylight: false,
            sky: Sky::Sunny,
            live_weather: false,
        }
    }
}

impl Conditions {
    /// The conditions as `state` has them at local time `now_ms` (the page's 1 s `now`, which
    /// adsb.lol's data ages by).
    pub fn of(state: &AppState, now_ms: f64) -> Conditions {
        let settings = &state.persisted.settings;
        let kinds = shown_kinds(settings.mode, &settings.also_show);
        Conditions {
            aircraft_on: kinds.aircraft,
            authenticated: state.aircraft.authenticated(),
            weather: settings.weather,
            basemap: settings.basemap,
            opensky_added: state.flight.opensky_added(),
            measured: state.flight.measured(now_ms, settings.adsb).is_some(),
            ships_on: kinds.ships,
            rail_map: settings.rail_map,
            terrain: settings.terrain,
            zoom: state.controller.view().zoom,
            daylight: settings.daylight,
            sky: settings.sky,
            // `state::weather` holds no weather until item R11 (see the module documentation)
            live_weather: false,
        }
    }

    /// The realistic sky follows the day's weather (`weatherLive`).
    pub fn weather_live(&self) -> bool {
        self.basemap == Basemap::Realistic && self.sky == Sky::Live
    }
}

/// The credits after the trains', in the page's order (`attribution`): each row of doc 06 §2.6
/// whose condition holds. Every string is the app's own (the app is always "in the app": rows 7
/// and 14 hold whenever their setting and zoom do).
pub fn credits(c: &Conditions) -> Vec<Cow<'static, str>> {
    let realistic = c.basemap == Basemap::Realistic;
    let close_in = realistic && c.zoom >= f64::from(SGL_ZOOM) - 0.5;
    let rows: [Option<Cow<'static, str>>; 18] = [
        c.aircraft_on.then_some(Cow::Borrowed(if c.authenticated { FLIGHT_DATA_SIGNED_IN } else { FLIGHT_DATA })),
        c.weather.then_some(Cow::Borrowed(RADAR)),
        (c.basemap == Basemap::Satellite).then_some(Cow::Borrowed(IMAGERY)),
        (c.opensky_added > 0).then_some(Cow::Borrowed(OPENSKY_TRACK)),
        c.measured.then_some(Cow::Borrowed(ADSB_LOL)),
        c.ships_on.then_some(Cow::Borrowed(SHIPS)),
        // the rail map in the app is OpenRailwayMap's
        rail_attribution(c.rail_map).map(Cow::Borrowed),
        c.terrain.then_some(Cow::Borrowed(TERRAIN_ATTRIBUTION)),
        realistic.then_some(Cow::Borrowed(TEXTURES)),
        close_in.then_some(Cow::Borrowed(SGL_ATTRIBUTION)),
        close_in.then_some(Cow::Borrowed(OVERTURE_ATTRIBUTION)),
        // the ground where the map leaves it unmapped, and what plants the trees close in
        (realistic && c.zoom >= LANDCOVER_ZOOM).then_some(Cow::Borrowed(LANDCOVER_ATTRIBUTION)),
        (realistic && c.zoom >= TREES_ZOOM).then(|| Cow::Owned(format!("{CANOPY_ATTRIBUTION}{SEPARATOR}{STREET_TREES_ATTRIBUTION}"))),
        (realistic && c.zoom >= ROAD_WIDTHS_ZOOM).then_some(Cow::Borrowed(ROAD_WIDTHS)),
        realistic.then_some(Cow::Borrowed(if c.daylight { EARTH_DAY_NIGHT } else { EARTH_DAY })),
        (realistic && c.zoom < CLOUDS_ZOOM).then_some(Cow::Borrowed(CLOUDS_ATTRIBUTION)),
        (c.weather_live() && c.live_weather).then_some(Cow::Borrowed(WEATHER)),
        Some(Cow::Borrowed(MAP)),
    ];
    rows.into_iter().flatten().collect()
}

/// What the strip says: the trains' credits (links), then the others.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Attribution {
    /// `trainCredit`: the credits of the trains on the map, while trains are on it.
    pub trains: Vec<Credit>,
    /// `attribution`.
    pub credits: Vec<Cow<'static, str>>,
}

impl Attribution {
    /// The strip for `state` at local time `now_ms`.
    pub fn of(state: &AppState, now_ms: f64) -> Attribution {
        let settings = &state.persisted.settings;
        let trains_on = shown_kinds(settings.mode, &settings.also_show).trains;
        let trains = if trains_on { train_credits(state.trains.on_map().iter()) } else { Vec::new() };
        Attribution { trains, credits: credits(&Conditions::of(state, now_ms)) }
    }

    /// The whole line, as its tooltip and its text read: every credit joined by `" · "`.
    pub fn text(&self) -> String {
        let all: Vec<&str> = self.trains.iter().map(|c| c.text).chain(self.credits.iter().map(|c| c.as_ref())).collect();
        all.join(SEPARATOR)
    }
}

/// Where the strip goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Place {
    /// The desktop: bottom right, at most the window less [`CLEAR`] wide, 8 px, its top-left corner
    /// rounded.
    Desktop,
    /// A phone upright: on the bottom sheet (`bottom: var(--sheet-h)`, the sheet's height in px),
    /// across the window, 7 px, square.
    Phone { sheet_height: f32 },
    /// A phone on its side: bottom right, at most the window less [`CLEAR_LANDSCAPE`] wide, 7 px,
    /// square.
    PhoneLandscape,
}

impl Place {
    fn size(self) -> f32 {
        match self {
            Place::Desktop => SIZE,
            Place::Phone { .. } | Place::PhoneLandscape => SIZE_PHONE,
        }
    }
}

/// The strip's text: the trains' credits in `--text-2` (`hovered`'s in `--text-1`), the rest and
/// the separators in `--text-3`. Answers the job and each train credit's characters.
fn job(line: &Attribution, size: f32, hovered: Option<usize>) -> (LayoutJob, Vec<std::ops::Range<usize>>) {
    let base = Style::ui(size, theme::WEIGHT_REGULAR, theme::TEXT_3).line_height(LINE_HEIGHT);
    let mut job = LayoutJob { wrap: TextWrapping::no_max_width(), break_on_newline: false, ..Default::default() };
    let mut links = Vec::with_capacity(line.trains.len());
    let chars = |job: &LayoutJob| job.text.chars().count();
    for (i, credit) in line.trains.iter().enumerate() {
        if i > 0 {
            text::append(&mut job, SEPARATOR, &base);
        }
        let start = chars(&job);
        let color = if hovered == Some(i) { theme::TEXT_1 } else { theme::TEXT_2 };
        text::append(&mut job, credit.text, &base.color(color));
        links.push(start..chars(&job));
    }
    for (i, credit) in line.credits.iter().enumerate() {
        if i > 0 || !line.trains.is_empty() {
            text::append(&mut job, SEPARATOR, &base);
        }
        text::append(&mut job, credit, &base);
    }
    (job, links)
}

/// The glyphs of `chars` on the galley's one row, relative to the galley: their box and their
/// baseline. `None` when the ellipsis cut them all.
fn span(galley: &Galley, chars: &std::ops::Range<usize>) -> Option<(Rect, f32)> {
    let row = galley.rows.first()?;
    // with the ellipsis the row's last glyph is `…`, which is the paragraph's, not the link's
    let shown = row.glyphs.len() - usize::from(galley.elided);
    let glyphs = row.glyphs.get(chars.start.min(shown)..chars.end.min(shown))?;
    let (first, last) = (glyphs.first()?, glyphs.last()?);
    let left = row.pos.x + first.pos.x;
    let right = row.pos.x + last.pos.x + last.advance_width;
    Some((Rect::from_x_y_ranges(left..=right, row.pos.y..=row.pos.y + row.size.y), row.pos.y + first.pos.y))
}

/// The strip's rectangle in `screen` for a line `text_w` wide whose line box is `line` high, its
/// edges on whole pixels as Chromium paints a box's background (`8 × 1.3 + 2` = 12.4 px: 12).
fn strip_rect(screen: Rect, place: Place, text_w: f32, line: f32) -> Rect {
    let h = (line + 2.0 * PADDING.y).round();
    match place {
        Place::Desktop | Place::PhoneLandscape => {
            let clear = if place == Place::Desktop { CLEAR } else { CLEAR_LANDSCAPE };
            let w = (text_w + 2.0 * PADDING.x).min((screen.width() - clear).max(0.0));
            Rect::from_min_max(Pos2::new((screen.right() - w).round(), screen.bottom() - h), screen.max)
        }
        Place::Phone { sheet_height } => {
            let bottom = (screen.bottom() - sheet_height).round();
            Rect::from_min_max(Pos2::new(screen.left(), bottom - h), Pos2::new(screen.right(), bottom))
        }
    }
}

/// Draws the strip for `line` at `place`: answers its rectangle. A click on a train credit opens
/// its page.
pub fn show(ctx: &Context, line: &Attribution, place: Place) -> Rect {
    let screen = ctx.content_rect();
    let size = place.size();
    let id = Id::unique("rustradar::attribution");
    let (natural, links) = job(line, size, None);
    let natural = ctx.fonts_mut(|f| f.layout_job(natural));
    let line_px = Style::ui(size, theme::WEIGHT_REGULAR, theme::TEXT_3).line_height(LINE_HEIGHT).line_px();
    let rect = strip_rect(screen, place, natural.size().x, line_px);
    let room = (rect.width() - 2.0 * PADDING.x).max(0.0);
    let lay = |hovered: Option<usize>| {
        let mut job = job(line, size, hovered).0;
        text::truncate(&mut job, room);
        ctx.fonts_mut(|f| f.layout_job(job))
    };
    let mut galley = lay(None);
    let radius = if place == Place::Desktop { CornerRadius { nw: theme::RADIUS_SM, ..CornerRadius::ZERO } } else { CornerRadius::ZERO };
    let full = line.text();
    Area::new(id).order(Order::Middle).fixed_pos(rect.min).fade_in(false).constrain(false).show(ctx, |ui: &mut Ui| {
        let (rect, strip) = ui.allocate_exact_size(rect.size(), Sense::hover());
        // the line box sits 1 px down; the galley's rows are on whole pixels
        let origin = Pos2::new(rect.left() + PADDING.x, rect.center().y - galley.size().y / 2.0);
        // the links, from the line as cut
        let mut hovered = None;
        let mut spans = Vec::with_capacity(links.len());
        for (i, (chars, credit)) in links.iter().zip(&line.trains).enumerate() {
            let Some((at, baseline)) = span(&galley, chars) else { continue };
            let at = at.translate(origin.to_vec2());
            let response = ui.interact(at, id.with(i), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            if response.hovered() {
                hovered = Some(i);
            }
            if response.clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(credit.url));
            }
            spans.push((at, origin.y + baseline));
        }
        if hovered.is_some() {
            galley = lay(hovered);
        }
        let painter = ui.painter();
        painter.rect_filled(rect, radius, theme::ATTRIBUTION_BG);
        painter.galley(origin, galley.clone(), Color32::PLACEHOLDER);
        for (at, baseline) in spans {
            let top = (baseline + UNDERLINE_OFFSET).round();
            painter.rect_filled(Rect::from_x_y_ranges(at.x_range(), top..=top + UNDERLINE_WIDTH), 0.0, theme::ATTRIBUTION_UNDERLINE);
        }
        tooltip::show(strip, &full);
    });
    rect
}

/// The readout's four lines: the view, where it looks, the basemap with the frames a second, and
/// how many aircraft, trains and ships are loaded (`count`: the locale's grouping, English here).
pub fn readout_lines(view: &ViewState, basemap: Basemap, fps: f64, counts: [usize; 3]) -> [String; 4] {
    let streets_gl = if basemap == Basemap::Realistic && view.zoom >= f64::from(SGL_ZOOM) - 0.5 { " · Streets GL" } else { "" };
    let count = |n: usize| group_thousands(n as f64);
    [
        format!("z {}  pitch {}°  bearing {}°", to_fixed(view.zoom, 2), to_fixed(view.pitch, 0), to_fixed(view.bearing, 0)),
        format!("{}, {}", to_fixed(view.latitude, 5), to_fixed(view.longitude, 5)),
        format!("{}{streets_gl}  {} fps", basemap.as_str(), rustradar_core::num::js_string(fps)),
        format!("{} aircraft · {} trains · {} ships", count(counts[0]), count(counts[1]), count(counts[2])),
    ]
}

/// The frames counted since `since` (egui's time, ms) at frame `frame`, and the last figure.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fps {
    frame: u64,
    since: f64,
    fps: f64,
}

/// The frames a second as the readout shows them: each pass of a new frame counts, and once 1000 ms
/// have gone the figure becomes `round(frames × 1000 / elapsed)` and the count starts again.
pub fn fps(ctx: &Context) -> f64 {
    let id = Id::unique("rustradar::attribution::fps");
    let (now, frame) = (ctx.input(|i| i.time) * 1000.0, ctx.cumulative_frame_nr());
    let mut fps = ctx.data(|d| d.get_temp::<Fps>(id)).unwrap_or(Fps { frame, since: now, fps: 0.0 });
    if now - fps.since >= 1000.0 {
        fps.fps = ((frame - fps.frame) as f64 * 1000.0 / (now - fps.since)).round();
        fps.frame = frame;
        fps.since = now;
    }
    ctx.data_mut(|d| d.insert_temp(id, fps));
    fps.fps
}

/// Draws the dev readout for `state` (whatever the build: [`ui`] draws it in debug builds only);
/// answers its rectangle.
pub fn dev_readout(ctx: &Context, state: &AppState) -> Rect {
    let counts = [state.aircraft.flights().len(), state.trains.trains().len(), state.ships.ships().len()];
    let lines = readout_lines(&state.controller.view(), state.basemap(), fps(ctx), counts);
    readout(ctx, &lines)
}

/// The readout box with `lines`, at top 8, right 8.
pub fn readout(ctx: &Context, lines: &[String]) -> Rect {
    // `<pre>`: its spaces kept, one face for every character (the middle dot is the font's own)
    let style = Style::data(theme::TEXT_DEV_READOUT, theme::WEIGHT_REGULAR, theme::TEXT_2).line_height(theme::LINE_HEIGHT_DEV_READOUT).letter_spacing(0.0);
    // a galley a line, each 13.5 px under the last (one galley rounds every row to 14)
    let galleys: Vec<Arc<Galley>> = lines
        .iter()
        .map(|line| {
            let mut job = LayoutJob { wrap: TextWrapping::no_max_width(), break_on_newline: false, ..Default::default() };
            job.append(line, 0.0, style.format());
            ctx.fonts_mut(|f| f.layout_job(job))
        })
        .collect();
    let line_px = style.line_px();
    let width = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max);
    let screen = ctx.content_rect();
    let size = Vec2::new(width, line_px * lines.len() as f32) + 2.0 * READOUT_PADDING;
    let rect = Rect::from_min_size(Pos2::new(screen.right() - READOUT_INSET - size.x, screen.top() + READOUT_INSET), size);
    Area::new(Id::unique("rustradar::attribution::readout"))
        .order(Order::Foreground)
        .fixed_pos(rect.min)
        .interactable(false)
        .fade_in(false)
        .constrain(false)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(theme::RADIUS_SM), theme::DEV_READOUT_BG);
            for (i, galley) in galleys.into_iter().enumerate() {
                // each line box centred on its 13.5 px
                let top = rect.top() + READOUT_PADDING.y + line_px * (i as f32 + 0.5) - galley.size().y / 2.0;
                ui.painter().galley(Pos2::new(rect.left() + READOUT_PADDING.x, top), galley, Color32::PLACEHOLDER);
            }
        });
    rect
}

/// The desktop's strip for `state` at local time `now_ms`, and the dev readout in debug builds.
pub fn ui(ctx: &Context, state: &AppState, now_ms: f64) {
    if cfg!(debug_assertions) {
        dev_readout(ctx, state);
    }
    show(ctx, &Attribution::of(state, now_ms), Place::Desktop);
}

#[cfg(test)]
mod tests {
    use rustradar_core::model::{Train, TrainKind, TrainSource};
    use rustradar_core::trains::{MODEL_CREDIT, train_credit};
    use rustradar_scene::rail::RAIL_SPEED_ATTRIBUTION;

    use super::*;

    /// Every row of doc 06 §2.6 with its condition, in the documented order.
    const ROWS: [&str; 18] = [
        FLIGHT_DATA,
        RADAR,
        IMAGERY,
        OPENSKY_TRACK,
        ADSB_LOL,
        SHIPS,
        RAIL_SPEED_ATTRIBUTION,
        TERRAIN_ATTRIBUTION,
        TEXTURES,
        SGL_ATTRIBUTION,
        OVERTURE_ATTRIBUTION,
        LANDCOVER_ATTRIBUTION,
        "Canopy height: Meta & WRI (CC BY 4.0), via TGE Labs · Street trees: Malmö stad, Umeå kommun (CC0)",
        ROAD_WIDTHS,
        EARTH_DAY_NIGHT,
        CLOUDS_ATTRIBUTION,
        WEATHER,
        MAP,
    ];

    /// Conditions that turn row `n` (1-based) on, and nothing else that could: the realistic rows
    /// on the realistic map at a zoom only they need.
    fn only(n: usize) -> Conditions {
        let c = Conditions::default();
        let realistic = |zoom: f64| Conditions { basemap: Basemap::Realistic, zoom, ..c };
        match n {
            1 => Conditions { aircraft_on: true, ..c },
            2 => Conditions { weather: true, ..c },
            3 => Conditions { basemap: Basemap::Satellite, ..c },
            4 => Conditions { opensky_added: 3, ..c },
            5 => Conditions { measured: true, ..c },
            6 => Conditions { ships_on: true, ..c },
            7 => Conditions { rail_map: true, ..c },
            8 => Conditions { terrain: true, ..c },
            9..=16 => Conditions { daylight: true, ..realistic([11.0, 15.5, 15.5, 12.0, 14.5, 14.0, 11.0, 10.9][n - 9]) },
            17 => Conditions { sky: Sky::Live, live_weather: true, daylight: true, ..realistic(11.0) },
            _ => c,
        }
    }

    #[track_caller]
    fn in_order(got: &[Cow<'static, str>]) {
        let mut at = 0;
        for credit in got {
            let n = ROWS[at..].iter().position(|r| *r == credit.as_ref()).unwrap_or_else(|| panic!("{credit:?} out of order in {got:?}"));
            at += n + 1;
        }
    }

    #[test]
    fn each_of_the_18_conditions_brings_its_credit_in_the_documented_order() {
        for n in 1..=18 {
            let got = credits(&only(n));
            assert!(got.iter().any(|c| c == ROWS[n - 1]), "row {n}: {got:?}");
            in_order(&got);
            // the map's credit closes every line
            assert_eq!(got.last().map(|c| c.as_ref()), Some(MAP), "row {n}");
        }
        // and none without it: nothing on, only the map's
        assert_eq!(credits(&Conditions::default()), [MAP]);
    }

    #[test]
    fn everything_at_once_in_the_page_s_order() {
        let all = Conditions {
            aircraft_on: true,
            authenticated: false,
            weather: true,
            basemap: Basemap::Realistic,
            opensky_added: 1,
            measured: true,
            ships_on: true,
            rail_map: true,
            terrain: true,
            zoom: 16.0,
            daylight: true,
            sky: Sky::Live,
            live_weather: true,
        };
        // close in: all but the satellite's imagery and the clouds
        let got = credits(&all);
        let want: Vec<&str> = ROWS.iter().enumerate().filter(|(i, _)| ![2, 15].contains(i)).map(|(_, r)| *r).collect();
        assert_eq!(got, want);
        // far out: the clouds, none of the close-in rows
        let got = credits(&Conditions { zoom: 5.0, ..all });
        let want: Vec<&str> = ROWS.iter().enumerate().filter(|(i, _)| ![2, 9, 10, 11, 12, 13].contains(i)).map(|(_, r)| *r).collect();
        assert_eq!(got, want);
        // on satellite: the imagery, none of the realistic rows
        let got = credits(&Conditions { basemap: Basemap::Satellite, ..all });
        let want: Vec<&str> = ROWS.iter().enumerate().filter(|(i, _)| *i < 8 || *i == 17).map(|(_, r)| *r).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn the_rows_conditions_at_their_edges() {
        let realistic = |zoom: f64| Conditions { basemap: Basemap::Realistic, zoom, ..Conditions::default() };
        let has = |c: Conditions, s: &str| credits(&c).iter().any(|x| x == s);
        assert!(has(Conditions { aircraft_on: true, authenticated: true, ..Default::default() }, FLIGHT_DATA_SIGNED_IN));
        assert!(!has(Conditions { authenticated: true, ..Default::default() }, FLIGHT_DATA_SIGNED_IN), "only with aircraft on");
        assert!(has(realistic(15.5), SGL_ATTRIBUTION) && !has(realistic(15.49), SGL_ATTRIBUTION));
        assert!(has(realistic(12.0), LANDCOVER_ATTRIBUTION) && !has(realistic(11.99), LANDCOVER_ATTRIBUTION));
        assert!(has(realistic(14.5), ROWS[12]) && !has(realistic(14.49), ROWS[12]));
        assert!(has(realistic(14.0), ROAD_WIDTHS) && !has(realistic(13.99), ROAD_WIDTHS));
        assert!(has(realistic(10.99), CLOUDS_ATTRIBUTION) && !has(realistic(11.0), CLOUDS_ATTRIBUTION));
        assert!(has(realistic(3.0), EARTH_DAY) && !has(realistic(3.0), EARTH_DAY_NIGHT));
        // the live sky without its weather loaded, or the weather on another basemap: nothing
        assert!(!has(Conditions { sky: Sky::Live, ..realistic(3.0) }, WEATHER));
        assert!(!has(Conditions { sky: Sky::Live, live_weather: true, ..Conditions::default() }, WEATHER));
        assert!(!has(Conditions { sky: Sky::Sunny, live_weather: true, ..realistic(3.0) }, WEATHER));
        // the realistic rows only there
        for basemap in [Basemap::Dark, Basemap::Satellite] {
            let got = credits(&Conditions { basemap, zoom: 16.0, daylight: true, ..Default::default() });
            assert!(got.iter().all(|c| !ROWS[8..17].contains(&c.as_ref())), "{basemap:?}: {got:?}");
        }
    }

    fn train(model: Option<&str>) -> Train {
        Train {
            id: "t".into(),
            source: TrainSource::Trafikverket,
            name: "1".into(),
            kind: TrainKind::Regional,
            lat: 59.33,
            lon: 18.07,
            heading: None,
            speed_kmh: None,
            destination: None,
            operator: None,
            delay_minutes: None,
            position_unix: 1_000,
            frames: vec![],
            reports: vec![],
            predicted: vec![],
            cars: None,
            tail: vec![],
            structures: vec![],
            ended_unix: None,
            shape: None,
            model: model.map(Into::into),
        }
    }

    #[test]
    fn the_train_credits_come_first_and_the_line_reads_as_the_tooltip() {
        let line = Attribution {
            trains: train_credits(&[train(None), train(Some("x2000"))]),
            credits: credits(&Conditions { aircraft_on: true, ..Default::default() }),
        };
        assert_eq!(line.trains, [train_credit(TrainSource::Trafikverket), MODEL_CREDIT]);
        assert_eq!(
            line.text(),
            "Trains in Sweden: Trafikverket, CC0 · 3D trains: Lardman · Flight data: Flightradar24 (unofficial API) · \
             Map, names and buildings: OpenFreeMap, © OpenMapTiles, © OpenStreetMap contributors"
        );
        // the job: the train credits first, as links, each its own characters
        let (job, links) = job(&line, SIZE, None);
        assert_eq!(job.text, line.text());
        let chars: Vec<char> = job.text.chars().collect();
        let texts: Vec<String> = links.iter().map(|r| chars[r.clone()].iter().collect()).collect();
        assert_eq!(texts, ["Trains in Sweden: Trafikverket, CC0", "3D trains: Lardman"]);
        assert_eq!(links[0].start, 0);
        // without trains the separator starts after the first credit
        let plain = Attribution { trains: vec![], credits: line.credits.clone() };
        assert!(plain.text().starts_with("Flight data"));
        assert_eq!(job_text(&plain), plain.text());
    }

    fn job_text(line: &Attribution) -> String {
        job(line, SIZE, None).0.text
    }

    #[test]
    fn the_readout_s_lines() {
        let view = ViewState { latitude: 35.0, longitude: 10.0, zoom: 1.9, bearing: -29.6, pitch: 0.4, ..ViewState::INITIAL };
        assert_eq!(
            readout_lines(&view, Basemap::Dark, 0.0, [0, 0, 0]),
            ["z 1.90  pitch 0°  bearing -30°", "35.00000, 10.00000", "dark  0 fps", "0 aircraft · 0 trains · 0 ships"]
        );
        let close = ViewState { zoom: 15.5, ..view };
        assert_eq!(readout_lines(&close, Basemap::Realistic, 58.0, [12_345, 7, 1_000])[2..], ["realistic · Streets GL  58 fps", "12,345 aircraft · 7 trains · 1,000 ships"]);
        assert_eq!(readout_lines(&close, Basemap::Satellite, 58.0, [0, 0, 0])[2], "satellite  58 fps");
    }

    #[test]
    fn frames_a_second_over_a_second_of_frames() {
        let ctx = Context::default();
        let mut seen = Vec::new();
        // 30 frames at 25 ms, then 4 at 500 ms
        let times: Vec<f64> = (0..30).map(|i| f64::from(i) * 0.025).chain((1..=4).map(|i| 0.725 + f64::from(i) * 0.5)).collect();
        for t in times {
            let raw = egui::RawInput { time: Some(t), ..Default::default() };
            ctx.run_ui(raw, |ui| seen.push(fps(ui.ctx()))).textures_delta.clear();
        }
        assert_eq!(seen[0], 0.0);
        assert_eq!(seen[29], 0.0, "not a second yet");
        // the 31st frame comes at 1.225 s: 30 frames after the first in 1225 ms
        assert_eq!(seen[30], (30.0f64 * 1000.0 / 1225.0).round());
        assert_eq!(seen[31], 24.0, "kept until the next second is up");
        assert_eq!(seen[32], 2.0, "2 frames in 1000 ms");
        assert_eq!(seen[33], 2.0);
    }

    /// One pass of `ctx` at `time` with `events`, drawing the strip for `line` in a 1440 × 900 window.
    fn pass(ctx: &Context, time: f64, events: Vec<egui::Event>, line: &Attribution) -> (Rect, egui::PlatformOutput) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0))),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut rect = Rect::NOTHING;
        let mut output = ctx.run_ui(raw, |ui| rect = show(ui.ctx(), line, Place::Desktop));
        output.textures_delta.clear();
        (rect, output.platform_output)
    }

    #[test]
    fn the_strip_on_phones() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(390.0, 844.0));
        let line = 7.0 * LINE_HEIGHT;
        // upright: on the sheet, across the window, 7 × 1.3 + 2 = 11.1 px on whole pixels
        let rect = strip_rect(screen, Place::Phone { sheet_height: 140.6 }, 900.0, line);
        assert_eq!(rect, Rect::from_min_max(Pos2::new(0.0, 692.0), Pos2::new(390.0, 703.0)));
        // on its side: bottom right, clear of 360, shrunk to its text
        let side = Rect::from_min_size(Pos2::ZERO, Vec2::new(844.0, 390.0));
        let rect = strip_rect(side, Place::PhoneLandscape, 900.0, line);
        assert_eq!(rect, Rect::from_min_max(Pos2::new(360.0, 379.0), Pos2::new(844.0, 390.0)));
        let rect = strip_rect(side, Place::PhoneLandscape, 100.0, line);
        assert_eq!(rect.left(), 844.0 - 108.0);
        assert_eq!((Place::Desktop.size(), Place::PhoneLandscape.size()), (8.0, 7.0));
    }

    #[test]
    fn the_strip_hugs_the_bottom_right_and_cuts_at_352_from_the_left() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let short = Attribution { trains: vec![], credits: credits(&Conditions { aircraft_on: true, ..Default::default() }) };
        let (rect, _) = pass(&ctx, 0.0, vec![], &short);
        assert_eq!(rect.max, Pos2::new(1440.0, 900.0));
        // 8 × 1.3 of line and 1 px above and below, on whole pixels as Chromium paints it
        assert_eq!(rect.height(), 12.0, "{rect:?}");
        // today's line is 483 px wide in dark-v1 (957 to 1440)
        assert!((rect.width() - 483.0).abs() < 12.0, "{rect:?}");
        // a line wider than the window less 352 is cut there
        let long = Attribution {
            trains: vec![],
            credits: credits(&Conditions { aircraft_on: true, weather: true, ships_on: true, rail_map: true, terrain: true, ..Default::default() }),
        };
        let (rect, _) = pass(&ctx, 0.1, vec![], &long);
        assert_eq!(rect.left(), 352.0, "{rect:?}");
    }

    #[test]
    fn a_train_credit_is_a_link() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let line = Attribution { trains: train_credits(&[train(None)]), credits: credits(&Conditions::default()) };
        // the area's first pass only sizes it
        pass(&ctx, 0.0, vec![], &line);
        let (rect, _) = pass(&ctx, 0.05, vec![], &line);
        // the first credit starts 4 px in: click on its middle
        let at = Pos2::new(rect.left() + 30.0, rect.center().y);
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let (_, out) = pass(&ctx, 0.1, vec![egui::Event::PointerMoved(at)], &line);
        assert_eq!(out.cursor_icon, CursorIcon::PointingHand);
        pass(&ctx, 0.2, vec![press(true)], &line);
        let (_, out) = pass(&ctx, 0.3, vec![press(false)], &line);
        let opened: Vec<_> = out.commands.iter().filter_map(|c| if let egui::OutputCommand::OpenUrl(u) = c { Some(u.url.clone()) } else { None }).collect();
        assert_eq!(opened, ["https://data.trafikverket.se/"]);
        // the map's credit is no link
        let at = Pos2::new(rect.right() - 20.0, rect.center().y);
        let (_, out) = pass(&ctx, 0.4, vec![egui::Event::PointerMoved(at)], &line);
        assert_ne!(out.cursor_icon, CursorIcon::PointingHand);
    }
}
