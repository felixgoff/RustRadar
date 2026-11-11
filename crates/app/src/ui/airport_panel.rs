//! AirportPanel, an airport's departures and arrivals (item U13; doc 06 §8.3;
//! `AirportPanel.svelte`), in the shell's airport slot ([`super::shell::Slots::airport_panel`]),
//! drawn from `state::airport`'s board. The panel does not scroll as a whole: the strip, the tabs
//! and the footer keep their places and the board between them scrolls, down to the window's
//! height less 44 (`flex: 1; min-height: 0; overflow-y: auto`).
//!
//! 1. **Strip** (`auto 1fr auto 40px`, [`panel::strip`]): the code (`h2.data`: Overpass Mono 22
//!    px at 700, line height 1.1, `-0.01em`) over the ICAO code (Mono 12 px `--text-2`; no line
//!    while there is none, as an empty `<p>` has no height); the name (13 px at 650, `Airport`
//!    until the reference data has it; its tooltip the full name) over `city, country` (12 px
//!    `--text-2`), both cut with `…`; the clock (Mono 13 px at 650) over the zone's name (Mono 12
//!    px `--text-2`) at the panel's own clock (read every 15 s), with the tooltip `Local time at
//!    {code}` or `Your time: the airport's time zone isn't known yet`; the close button `Close
//!    airport`.
//! 2. **Tabs** (U4's [`Tabs`]): `Departures` with `PlaneTakeoff`, `Arrivals` with `PlaneLanding`
//!    (15/1.75, 8 px from the label); Left and Right toggle.
//! 3. **Board** (padding 8; at 0.75 opacity over 150 ms while a load is out):
//!    - failed with no rows: `Couldn't load the {mode} board.` (13 px `--text-2`) and 12 px under
//!      it Try again (`RefreshCw`), padding `12px 8px`;
//!    - loading: seven placeholder rows 36 px high (padding `12px 8px`, skeletons 12 px high and
//!      12 apart, [`skeleton_widths`]);
//!    - empty: `No {mode} listed right now.` (13 px `--text-2`, padding `12px 8px`);
//!    - the rows, a day heading (12 px at 600 `--text-2`, padding `12px 8px 4px`) before a row
//!      whose day differs from the row before's (before the first row unless it is `Today`). A
//!      row ([`row_height`]: a `44px 1fr auto` grid with gaps `1px 8px`, padding 8, radius 4,
//!      three line boxes, the third 0 high when it has nothing):
//!      - the time (Mono 13 px at 650, `clockAt(scheduled)` in the airport's zone); the number
//!        (`number || callsign || "—"`, Mono 13 px at 650) and the airline (12 px `--text-3`, cut
//!        with `…`) 8 px after it on its baseline; the status (12 px, `--live`, `--warn`,
//!        `--danger` by `toneOf(statusColor)`, else `--text-2`; after a 6 px `--live` dot 4 px
//!        before it when the flight is live), centred on the first line;
//!      - `To|From {city || name || "?"}` and Mono `({IATA})` (12 px `--text-2`, cut with `…`);
//!      - `[aircraft, registration, T{terminal}, Gate {gate}]` joined by ` · ` (Mono 11 px
//!        `--text-3`, wrapping; the dots Overpass Mono's own, a full mono advance: [`data_job`]);
//!      - a button only for a live flight with an id (`Show this flight`, else `Not airborne or
//!        not tracked right now` as its tooltip), `--surface-hover` under the pointer over the
//!        selected flight's `--amber-soft` (CSS's `button.row:not(:disabled):hover` is the more
//!        specific rule), no transition; a click selects the flight and flies to it from the
//!        board ([`Ask::Pick`]);
//!    - `Show more {mode}` (margin 8, disabled while a load is out: it looks the same, as app.css
//!      gives `.button:disabled` no look) while there are more pages ([`Ask::More`]).
//! 4. **Footer** ([`panel::Footer`], not sticky): `Updated {HH:MM} · times at {code}` (or
//!    `· your time` without the zone; 11 px `--text-3`) once a board has loaded, and Show on map
//!    (`LocateFixed`; disabled until the airport is known) flying there at zoom 11.
//!
//! Positions follow Chromium's layout of today's markup with the app's CSS (checked against a
//! headless Chromium render of the component with the test's fixture, `tests/airport_panel.rs`):
//! the panel's box, the strip, the tabs, every row's rectangle (within 0.15 px), the footer and
//! the buttons land on Chromium's pixels, and each line of text within 1 px across and down. A
//! line can sit 1 px (once 2 px) lower or higher than Chromium's where its line box's top falls
//! at another fraction of a pixel: egui rounds a galley's top to the pixel grid, Chromium the
//! baseline. Overpass Mono at 11 px rasterises 1 px shorter than Chromium's hinted glyphs.
//!
//! Deviations: the name's tooltip covers its whole strip cell (today the name's line alone, as
//! [`panel::strip`] gives cells their tooltip); the board's scroll bar floats over its right edge
//! (Chromium's overlay bar in the reference pictures, as the other panels); the phone's bottom
//! sheet and its peek parts are item U19's; the panel's `aria-label` is not set (egui's panels
//! carry no landmark).

use egui::text::{LayoutJob, TextWrapping};
use egui::{Color32, CornerRadius, CursorIcon, Id, Pos2, Rect, ScrollArea, Sense, Ui, Vec2};
use rustradar_core::model::{Airport, BoardMode, BoardRow};
use rustradar_core::time::{Clocks, Tone, tone_of};

use super::fonts::Face;
use super::icons::Icon;
use super::panel::{self, Cell, Fit, Footer, Piece};
use super::shell::Pass;
use super::text::{self, Style};
use super::widgets::button::{self, Button, focus_ring};
use super::widgets::tabs::{Tab, Tabs};
use super::widgets::{skeleton, tooltip};
use super::{anim, theme};
use crate::state::airport::{Ask, LOCATE_ZOOM, mode_name};
use crate::state::{Action, AppState};

/// The close button's accessible name.
pub const CLOSE_LABEL: &str = "Close airport";
/// The name cell before the reference data has the airport.
pub const NO_NAME: &str = "Airport";
/// The tabs' labels.
pub const DEPARTURES: &str = "Departures";
pub const ARRIVALS: &str = "Arrivals";
/// The retry button's text.
pub const RETRY: &str = "Try again";
/// The footer's button.
pub const SHOW_ON_MAP: &str = "Show on map";
/// A row's tooltip: a live flight, or not.
pub const SHOW_FLIGHT: &str = "Show this flight";
pub const NOT_TRACKED: &str = "Not airborne or not tracked right now";
/// What a row's flight reads without a number or a callsign.
pub const NO_NUMBER: &str = "—";

/// The board's padding (`padding: var(--space-2)`).
pub const BOARD_PAD: f32 = theme::SPACE_2;
/// A row's padding, its gaps (`gap: 1px var(--space-2)`) and its time column.
pub const ROW_PAD: f32 = theme::SPACE_2;
pub const ROW_GAP: Vec2 = Vec2::new(theme::SPACE_2, 1.0);
pub const TIME_COLUMN: f32 = 44.0;
/// Between the number and the airline (`gap: var(--space-2)`).
pub const FLIGHT_GAP: f32 = theme::SPACE_2;
/// The live dot's size and its gap to the status (`gap: var(--space-1)`).
pub const LIVE_DOT: f32 = 6.0;
pub const STATUS_GAP: f32 = theme::SPACE_1;
/// A day heading's padding: top, sides, bottom (`padding: var(--space-3) var(--space-2)
/// var(--space-1)`).
pub const DAY_PAD: [f32; 3] = [theme::SPACE_3, theme::SPACE_2, theme::SPACE_1];
/// The messages' and placeholders' padding (`padding: var(--space-3) var(--space-2)`), and their
/// gap.
pub const MESSAGE_PAD: Vec2 = Vec2::new(theme::SPACE_2, theme::SPACE_3);
pub const MESSAGE_GAP: f32 = theme::SPACE_3;
/// How many placeholder rows show while loading, their skeletons' height and gap.
pub const SKELETON_ROWS: usize = 7;
pub const SKELETON_HEIGHT: f32 = 12.0;
pub const SKELETON_GAP: f32 = theme::SPACE_3;
/// Show more's margin (`margin: var(--space-2)`).
pub const MORE_MARGIN: f32 = theme::SPACE_2;
/// The board's opacity while a load is out (`[aria-busy="true"] { opacity: 0.75 }`).
pub const BUSY_OPACITY: f32 = 0.75;

/// The code (`h2.data`).
pub const fn code_style() -> Style {
    panel::data(&panel::title_style(Face::Mono))
}

/// The strip's `.value`: 13 px at 650 (in the data face for the clock).
pub const fn value_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_BADGE, theme::TEXT_1)
}

/// A row's time and number (`.time.data`, `.number.data`: Mono 13 px at 650).
pub const fn time_style() -> Style {
    Style::data(theme::TEXT_MD, theme::WEIGHT_BADGE, theme::TEXT_1)
}

/// A row's airline (`.airline`: 12 px `--text-3`).
pub const fn airline_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// A row's status (`.status`: 12 px) in its tone's colour.
pub const fn status_style(tone: Option<Tone>) -> Style {
    let color = match tone {
        Some(Tone::Good) => theme::LIVE,
        Some(Tone::Warn) => theme::WARN,
        Some(Tone::Bad) => theme::DANGER,
        None => theme::TEXT_2,
    };
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, color)
}

/// A row's place (`.place`: 12 px `--text-2`).
pub const fn place_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// A row's third line (`.extra.data`: Mono 11 px `--text-3`).
pub const fn extra_style() -> Style {
    Style::data(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// A day heading (`.day`: 12 px at 600 `--text-2`).
pub const fn day_style() -> Style {
    Style::ui(theme::TEXT_SM, 600, theme::TEXT_2)
}

/// The messages (`.message`: 13 px `--text-2`).
pub const fn message_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// The footer's status (`.updated`: 11 px `--text-3`).
pub const fn updated_style() -> Style {
    Style::ui(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// The two tabs.
pub const TABS: [Tab<'static, BoardMode>; 2] =
    [Tab::new(BoardMode::Departures, DEPARTURES).icon(Icon::PlaneTakeoff), Tab::new(BoardMode::Arrivals, ARRIVALS).icon(Icon::PlaneLanding)];

/// `a || b` for the board's optional texts.
fn or<'a>(a: &'a Option<String>, b: &'a Option<String>) -> Option<&'a str> {
    a.as_deref().filter(|s| !s.is_empty()).or_else(|| b.as_deref().filter(|s| !s.is_empty()))
}

/// The airport's zone (`airport?.timezone`), none when unknown or empty.
pub fn zone(airport: Option<&Airport>) -> Option<&str> {
    airport.map(|a| a.timezone.as_str()).filter(|z| !z.is_empty())
}

/// The clock cell's tooltip.
pub fn clock_tooltip(code: &str, zone: Option<&str>) -> String {
    match zone {
        Some(_) => format!("Local time at {code}"),
        None => "Your time: the airport's time zone isn't known yet".to_owned(),
    }
}

/// The name cell's second line: `[city, country].filter(Boolean).join(", ")`.
pub fn place_of(airport: Option<&Airport>) -> String {
    airport.map_or_else(String::new, |a| [a.city.as_str(), a.country.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", "))
}

/// A row's flight: `number || callsign || "—"`.
pub fn number_text(r: &BoardRow) -> &str {
    or(&r.number, &r.callsign).unwrap_or(NO_NUMBER)
}

/// A row's place: `To|From`, `city || name || "?"`, and the IATA code when there is one.
pub fn place_text(mode: BoardMode, r: &BoardRow) -> (String, Option<&str>) {
    let to = match mode {
        BoardMode::Departures => "To",
        BoardMode::Arrivals => "From",
    };
    let name = or(&r.airport_city, &r.airport_name).unwrap_or("?");
    (format!("{to} {name}"), r.airport_iata.as_deref().filter(|c| !c.is_empty()))
}

/// A row's third line: `[aircraft, registration, terminal && T…, gate && Gate …]` joined by ` · `.
pub fn extra_text(r: &BoardRow) -> String {
    let terminal = r.terminal.as_deref().filter(|t| !t.is_empty()).map(|t| format!("T{t}"));
    let gate = r.gate.as_deref().filter(|g| !g.is_empty()).map(|g| format!("Gate {g}"));
    [r.aircraft.clone(), r.registration.clone(), terminal, gate].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// Whether a row is a button that can be pressed (`disabled={!r.live || !r.flightId}`).
pub fn enabled(r: &BoardRow) -> bool {
    r.live && r.flight_id.is_some_and(|id| id != 0)
}

/// A row's day (`r.scheduled ? dayAt(r.scheduled, timeZone, now) : ""`).
fn day_of(clocks: &Clocks, r: &BoardRow, zone: Option<&str>, now_ms: f64) -> String {
    match r.scheduled.filter(|&s| s != 0) {
        Some(s) => clocks.day_at(s as f64, zone, now_ms as i64),
        None => String::new(),
    }
}

/// The day heading before each row, if any: the first row's unless it is `Today`, a later row's
/// when its day differs from the row before's.
pub fn day_headings(clocks: &Clocks, rows: &[BoardRow], zone: Option<&str>, now_ms: f64) -> Vec<Option<String>> {
    let days: Vec<String> = rows.iter().map(|r| day_of(clocks, r, zone, now_ms)).collect();
    days.iter()
        .enumerate()
        .map(|(i, day)| {
            let show = if i == 0 { day != "Today" } else { *day != days[i - 1] };
            show.then(|| day.clone())
        })
        .collect()
}

/// Placeholder row `i`'s skeleton widths (`38`, `90 + (i·31 mod 60)`, `60 + (i·17 mod 30)`).
pub fn skeleton_widths(i: usize) -> [f32; 3] {
    [38.0, 90.0 + ((i * 31) % 60) as f32, 60.0 + ((i * 17) % 30) as f32]
}

/// The footer's status (`Updated … · times at {code}`), empty before a board has loaded.
pub fn updated_text(clocks: &Clocks, updated_ms: f64, code: &str, zone: Option<&str>) -> String {
    if updated_ms == 0.0 {
        return String::new();
    }
    let at = clocks.clock_at(updated_ms / 1000.0, zone);
    match zone {
        Some(_) => format!("Updated {at} · times at {code}"),
        None => format!("Updated {at} · your time"),
    }
}

/// What the board shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// The last load failed and nothing loaded: the message and Try again.
    Failed,
    /// Nothing loaded yet: the placeholders.
    Loading,
    /// A board without rows.
    Empty,
    Rows,
}

/// What the board shows for `state` (`{#if failed && !rows} … {:else if !rows} … {:else if
/// rows.length === 0} … {:else}`).
pub fn body(state: &AppState) -> Body {
    match state.airport.rows() {
        None if state.airport.failed() => Body::Failed,
        None => Body::Loading,
        Some([]) => Body::Empty,
        Some(_) => Body::Rows,
    }
}

/// A row's height: the padding, the first two line boxes, both gaps, and the third line's rows
/// `width` px wide (none when it has nothing).
pub fn row_height(ctx: &egui::Context, r: &BoardRow, width: f32) -> f32 {
    let extra = extra_text(r);
    let third = if extra.is_empty() { 0.0 } else { extra_rows(ctx, &extra, width) as f32 * extra_style().line_px() };
    2.0 * ROW_PAD + time_style().line_px() + place_style().line_px() + 2.0 * ROW_GAP.y + third
}

fn extra_rows(ctx: &egui::Context, extra: &str, width: f32) -> usize {
    ctx.fonts_mut(|f| f.layout_job(data_job(extra, &extra_style(), width))).rows.len().max(1)
}

/// `text` in the data face wrapped within `width`, each `·` Overpass Mono's own: `--font-data`
/// has no Middle Dot face, so today a dot there takes a full mono advance (Chromium: 6.67 px at
/// 11 px, as any other character), not the UI face's narrow one [`text::append`] gives.
pub fn data_job(text: &str, style: &Style, width: f32) -> LayoutJob {
    let mut job = LayoutJob { wrap: TextWrapping::wrap_at_width(width.max(0.0)), break_on_newline: true, ..Default::default() };
    job.append(text, 0.0, style.format());
    job
}

/// The width of a row's status cell (`auto`): the dot, its gap and the words.
fn status_width(ctx: &egui::Context, r: &BoardRow) -> f32 {
    let words = r.status.as_deref().unwrap_or("");
    let text = if words.is_empty() { 0.0 } else { text::measure(ctx, text::job(words, &status_style(None))).x };
    let dot = if r.live { LIVE_DOT + if words.is_empty() { 0.0 } else { STATUS_GAP } } else { 0.0 };
    dot + text
}

/// The distance from a galley's top to its first row's baseline.
fn baseline(galley: &egui::Galley) -> f32 {
    galley.rows.first().map_or(0.0, |row| row.pos.y + row.row.glyphs.first().map_or(row.row.size.y, |g| g.pos.y))
}

/// The panel in the shell's slot.
pub fn show(ui: &mut Ui, pass: &mut Pass<'_>) {
    let state = pass.state;
    let Some(code) = state.airport.code() else { return };
    let ctx = ui.ctx().clone();
    let id = Id::unique("rustradar::airport_panel");
    let clocks = Clocks::default();
    let airport = state.aircraft.reference().and_then(|r| r.airport(Some(code)));
    let zone = zone(airport);
    let width = ui.available_width();
    let max_height = ui.max_rect().height().min(ui.available_height());
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let top = ui.cursor().top();

    // the strip
    let now = state.airport.now_ms() / 1000.0;
    let icao = airport.map_or("", |a| a.icao.as_str());
    let name = airport.map_or(NO_NAME, |a| a.name.as_str());
    let line = |text: &str, style: &Style| if text.is_empty() { Piece::Custom(Vec2::ZERO, Box::new(|_, _| {})) } else { Piece::text(text, style, Fit::Ellipsis) };
    let cells = vec![
        Cell::auto(vec![Piece::text(code, &code_style(), Fit::Ellipsis), line(icao, &panel::sub_style(Face::Mono))]),
        Cell::fill(vec![line(name, &value_style()), line(&place_of(airport), &panel::sub_style(Face::Ui))]).tooltip(airport.map(|a| a.name.clone()).filter(|n| !n.is_empty())),
        Cell::auto(vec![
            Piece::text(&clocks.clock_at(now, zone), &panel::data(&value_style()), Fit::Ellipsis),
            Piece::text(&clocks.zone_name(now, zone), &panel::sub_style(Face::Mono), Fit::Ellipsis),
        ])
        .tooltip(Some(clock_tooltip(code, zone))),
    ];
    if panel::strip(ui, cells, CLOSE_LABEL).clicked() {
        pass.actions.push(Action::Board(Ask::Close));
    }

    // the tabs
    let mut mode = state.airport.mode();
    if Tabs::new("rustradar::airport_panel::tabs", &TABS).show(ui, &mut mode).changed() {
        pass.actions.push(Action::Board(Ask::Mode(mode)));
    }

    // the footer's height first: the board takes what is left
    let status = updated_text(&clocks, state.airport.updated_at(), code, zone);
    let footer = Footer {
        status: line(&status, &updated_style()),
        status_tooltip: None,
        buttons: vec![Button::new().icon(button::icon(Icon::LocateFixed)).text(SHOW_ON_MAP).enabled(airport.is_some())],
    };
    let footer_h = footer.height(&ctx, width);
    let board_max = (max_height - (ui.cursor().top() - top) - footer_h).max(0.0);
    ui.scope(|ui| {
        let scroll = &mut ui.spacing_mut().scroll;
        scroll.floating = true;
        scroll.fade.strength = 0.0;
        ScrollArea::vertical().id_salt(id.with("board")).auto_shrink([false, true]).max_height(board_max).show(ui, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let busy = anim::toggle(&ctx, id.with("busy"), state.airport.busy(), anim::HOVER);
            ui.multiply_opacity(1.0 - (1.0 - BUSY_OPACITY) * busy);
            board(ui, id, pass, &clocks, zone);
        });
    });

    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, footer_h), Sense::hover());
    let responses = footer.show(ui, rect);
    if let (Some(a), Some(locate)) = (airport, responses.first())
        && locate.clicked()
    {
        pass.actions.push(Action::FlyTo { longitude: a.lon, latitude: a.lat, zoom: Some(LOCATE_ZOOM) });
    }
}

/// The board's content inside its padding.
fn board(ui: &mut Ui, id: Id, pass: &mut Pass<'_>, clocks: &Clocks, zone: Option<&str>) {
    let state = pass.state;
    let mode = state.airport.mode();
    let width = ui.available_width() - 2.0 * BOARD_PAD;
    ui.add_space(BOARD_PAD);
    let left = ui.cursor().left() + BOARD_PAD;
    let inner = |ui: &mut Ui, h: f32| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width + 2.0 * BOARD_PAD, h), Sense::hover());
        Rect::from_min_size(Pos2::new(left, rect.top()), Vec2::new(width, h))
    };
    match body(state) {
        Body::Failed => {
            let words = format!("Couldn't load the {} board.", mode_name(mode));
            let galley = ui.painter().layout_job(text::wrapped(&words, &message_style(), width - 2.0 * MESSAGE_PAD.x));
            let retry = Button::new().icon(button::icon(Icon::RefreshCw)).text(RETRY);
            let size = retry.size(ui.ctx());
            let text_h = galley.rows.len().max(1) as f32 * message_style().line_px();
            let rect = inner(ui, 2.0 * MESSAGE_PAD.y + text_h + MESSAGE_GAP + size.y);
            let at = rect.min + MESSAGE_PAD;
            ui.painter().galley(Pos2::new(at.x, at.y + (message_style().line_px() - galley.rows.first().map_or(0.0, |r| r.size.y)) / 2.0), galley, Color32::PLACEHOLDER);
            let button_rect = Rect::from_min_size(Pos2::new(at.x, at.y + text_h + MESSAGE_GAP), size);
            if panel::put(ui, "retry", button_rect, retry).clicked() {
                pass.actions.push(Action::Board(Ask::Retry));
            }
        }
        Body::Loading => {
            for i in 0..SKELETON_ROWS {
                let rect = inner(ui, 2.0 * MESSAGE_PAD.y + SKELETON_HEIGHT);
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let mut x = rect.left() + MESSAGE_PAD.x;
                for w in skeleton_widths(i) {
                    let r = Rect::from_min_size(Pos2::new(x, rect.top() + MESSAGE_PAD.y), Vec2::new(w, SKELETON_HEIGHT));
                    skeleton::paint(ui.painter(), panel::snap_rect(r, ui.ctx().pixels_per_point()), theme::RADIUS);
                    x += w + SKELETON_GAP;
                }
            }
        }
        Body::Empty => {
            let words = format!("No {} listed right now.", mode_name(mode));
            let galley = ui.painter().layout_job(text::wrapped(&words, &message_style(), width - 2.0 * MESSAGE_PAD.x));
            let text_h = galley.rows.len().max(1) as f32 * message_style().line_px();
            let rect = inner(ui, 2.0 * MESSAGE_PAD.y + text_h);
            let at = rect.min + MESSAGE_PAD;
            ui.painter().galley(Pos2::new(at.x, at.y + (message_style().line_px() - galley.rows.first().map_or(0.0, |r| r.size.y)) / 2.0), galley, Color32::PLACEHOLDER);
        }
        Body::Rows => {
            let rows = state.airport.rows().unwrap_or_default();
            let now_ms = state.airport.now_ms();
            let headings = day_headings(clocks, rows, zone, now_ms);
            let selected = state.flight.selected_id();
            for (i, (r, heading)) in rows.iter().zip(headings).enumerate() {
                if let Some(day) = heading {
                    let line = day_style().line_px();
                    let rect = inner(ui, DAY_PAD[0] + line + DAY_PAD[2]);
                    if ui.is_rect_visible(rect) {
                        let galley = ui.painter().layout_job(text::truncated(&day, &day_style(), width - 2.0 * DAY_PAD[1]));
                        ui.painter().galley(Pos2::new(rect.left() + DAY_PAD[1], rect.top() + DAY_PAD[0] + (line - galley.size().y) / 2.0), galley, Color32::PLACEHOLDER);
                    }
                }
                let h = row_height(ui.ctx(), r, width - 2.0 * ROW_PAD - TIME_COLUMN - ROW_GAP.x);
                let rect = inner(ui, h);
                let chosen = r.flight_id.is_some() && r.flight_id == selected;
                if let Some(flight) = row(ui, id.with(("row", i)), rect, r, mode, clocks, zone, chosen) {
                    pass.actions.push(Action::Board(Ask::Pick(flight)));
                }
            }
            if state.airport.more() {
                let more = Button::new().text(format!("Show more {}", mode_name(mode))).enabled(!state.airport.busy());
                let size = more.size(ui.ctx());
                let rect = inner(ui, 2.0 * MORE_MARGIN + size.y);
                let button_rect = Rect::from_min_size(Pos2::new(rect.left() + MORE_MARGIN, rect.top() + MORE_MARGIN), size);
                if panel::put(ui, "more", button_rect, more).clicked() {
                    pass.actions.push(Action::Board(Ask::More));
                }
            }
        }
    }
    ui.add_space(BOARD_PAD);
}

/// One row of the board in `rect`; answers the flight to show when it was clicked.
#[allow(clippy::too_many_arguments)]
fn row(ui: &mut Ui, id: Id, rect: Rect, r: &BoardRow, mode: BoardMode, clocks: &Clocks, zone: Option<&str>, chosen: bool) -> Option<u32> {
    let on = enabled(r);
    let response = ui.interact(rect, id, if on { Sense::click() } else { Sense::hover() });
    let response = if on { response.on_hover_cursor(CursorIcon::PointingHand) } else { response };
    let response = tooltip::show(response, if r.live { SHOW_FLIGHT } else { NOT_TRACKED });
    if !ui.is_rect_visible(rect) {
        return response.clicked().then_some(r.flight_id).flatten();
    }
    let ctx = ui.ctx().clone();
    let painter = ui.painter().clone();
    let ppp = ctx.pixels_per_point();
    // `button.row:not(:disabled):hover` is more specific than `.row.selected`
    let fill = if on && response.hovered() {
        theme::SURFACE_HOVER
    } else if chosen {
        theme::AMBER_SOFT
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(panel::snap_rect(rect, ppp), CornerRadius::same(theme::RADIUS_SM), fill);
    let content = rect.shrink(ROW_PAD);
    let first = time_style().line_px();
    let flight_x = content.left() + TIME_COLUMN + ROW_GAP.x;
    let status_w = status_width(&ctx, r);
    let status_x = content.right() - status_w;
    let flight_w = (status_x - ROW_GAP.x - flight_x).max(0.0);

    // the first line: the time, the number and airline on its baseline, the status centred
    let time = painter.layout_job(text::job(&clocks.clock_at(r.scheduled, zone), &time_style()));
    let line_top = |g: &egui::Galley, line: f32| content.top() + (line - g.size().y) / 2.0;
    painter.galley(Pos2::new(content.left(), line_top(&time, first)), time, Color32::PLACEHOLDER);
    let number = painter.layout_job(text::job(number_text(r), &time_style()));
    let number_top = line_top(&number, first);
    let number_base = number_top + baseline(&number);
    let number_w = number.size().x;
    painter.galley(Pos2::new(flight_x, number_top), number, Color32::PLACEHOLDER);
    if let Some(airline) = r.airline.as_deref().filter(|a| !a.is_empty()) {
        let room = flight_w - number_w - FLIGHT_GAP;
        if room > 0.0 {
            let galley = painter.layout_job(text::truncated(airline, &airline_style(), room));
            let at = Pos2::new(flight_x + number_w + FLIGHT_GAP, number_base - baseline(&galley));
            painter.with_clip_rect(Rect::from_min_max(Pos2::new(flight_x, content.top()), Pos2::new(flight_x + flight_w, content.bottom()))).galley(at, galley, Color32::PLACEHOLDER);
        }
    }
    let mid = content.top() + first / 2.0;
    let mut x = status_x;
    if r.live {
        painter.circle_filled(Pos2::new(x + LIVE_DOT / 2.0, mid), LIVE_DOT / 2.0, theme::LIVE);
        x += LIVE_DOT + STATUS_GAP;
    }
    if let Some(words) = r.status.as_deref().filter(|s| !s.is_empty()) {
        let galley = painter.layout_job(text::job(words, &status_style(tone_of(r.status_color.as_deref()))));
        painter.galley(Pos2::new(x, mid - galley.size().y / 2.0), galley, Color32::PLACEHOLDER);
    }

    // the second line across the flight's and the status's columns
    let wide = content.right() - flight_x;
    let second = content.top() + first + ROW_GAP.y;
    let (words, iata) = place_text(mode, r);
    let mut job = LayoutJob::default();
    text::append(&mut job, &words, &place_style());
    if let Some(iata) = iata {
        text::append(&mut job, " ", &place_style());
        text::append(&mut job, &format!("({iata})"), &panel::data(&place_style()));
    }
    text::truncate(&mut job, wide);
    let galley = painter.layout_job(job);
    painter.galley(Pos2::new(flight_x, second + (place_style().line_px() - galley.size().y) / 2.0), galley, Color32::PLACEHOLDER);

    // the third line, when it has something
    let extra = extra_text(r);
    if !extra.is_empty() {
        let third = second + place_style().line_px() + ROW_GAP.y;
        let galley = painter.layout_job(data_job(&extra, &extra_style(), wide));
        let rows = galley.rows.len().max(1) as f32;
        let line = extra_style().line_px();
        painter.galley(Pos2::new(flight_x, third + (line * rows - galley.size().y) / 2.0 / rows), galley, Color32::PLACEHOLDER);
    }
    if response.has_focus() {
        focus_ring(&painter, rect, theme::RADIUS_SM);
    }
    if response.clicked() { r.flight_id } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(number: Option<&str>, callsign: Option<&str>) -> BoardRow {
        BoardRow { number: number.map(str::to_owned), callsign: callsign.map(str::to_owned), ..Default::default() }
    }

    #[test]
    fn a_row_s_words() {
        assert_eq!(number_text(&row(Some("SK1415"), Some("SAS1415"))), "SK1415");
        assert_eq!(number_text(&row(None, Some("NAX12K"))), "NAX12K");
        assert_eq!(number_text(&row(Some(""), None)), "—");
        let mut r = BoardRow { airport_city: Some("Copenhagen".into()), airport_name: Some("Kastrup".into()), airport_iata: Some("CPH".into()), ..Default::default() };
        assert_eq!(place_text(BoardMode::Departures, &r), ("To Copenhagen".to_owned(), Some("CPH")));
        r.airport_city = None;
        assert_eq!(place_text(BoardMode::Arrivals, &r), ("From Kastrup".to_owned(), Some("CPH")));
        assert_eq!(place_text(BoardMode::Arrivals, &BoardRow::default()), ("From ?".to_owned(), None));
        let r = BoardRow { aircraft: Some("A20N".into()), registration: Some("SE-ROR".into()), terminal: Some("5".into()), gate: Some("F62".into()), ..Default::default() };
        assert_eq!(extra_text(&r), "A20N · SE-ROR · T5 · Gate F62");
        assert_eq!(extra_text(&BoardRow { gate: Some("12".into()), ..Default::default() }), "Gate 12");
        assert_eq!(extra_text(&BoardRow::default()), "");
        assert!(enabled(&BoardRow { live: true, flight_id: Some(7), ..Default::default() }));
        assert!(!enabled(&BoardRow { live: false, flight_id: Some(7), ..Default::default() }));
        assert!(!enabled(&BoardRow { live: true, flight_id: None, ..Default::default() }));
    }

    #[test]
    fn day_headings_as_today_s_board_puts_them() {
        let clocks = Clocks::default();
        // 2026-06-21 12:00 in Stockholm
        let now = 1_782_036_000_000.0;
        let at = |s: i64| BoardRow { scheduled: Some(s), ..Default::default() };
        let today = 1_782_036_000;
        let rows = [at(today), at(today + 3600), at(today + 86_400), at(today + 86_400 + 60), at(today + 2 * 86_400)];
        let zone = Some("Europe/Stockholm");
        assert_eq!(day_headings(&clocks, &rows, zone, now), [None, None, Some("Tomorrow".into()), None, Some("Tue 23 Jun".into())]);
        // a first row of yesterday has its heading, and Today after it
        let rows = [at(today - 13 * 3600), at(today)];
        assert_eq!(day_headings(&clocks, &rows, zone, now), [Some("Yesterday".into()), Some("Today".into())]);
        // no time: an empty day, a heading of nothing before a first row
        assert_eq!(day_headings(&clocks, &[BoardRow::default(), at(today)], zone, now), [Some(String::new()), Some("Today".into())]);
    }

    #[test]
    fn the_strip_and_footer_words() {
        let clocks = Clocks::default();
        assert_eq!(clock_tooltip("ARN", Some("Europe/Stockholm")), "Local time at ARN");
        assert_eq!(clock_tooltip("ARN", None), "Your time: the airport's time zone isn't known yet");
        let a = Airport { city: "Stockholm".into(), country: "Sweden".into(), timezone: String::new(), ..Default::default() };
        assert_eq!(place_of(Some(&a)), "Stockholm, Sweden");
        assert_eq!(place_of(Some(&Airport { country: "Sweden".into(), ..Default::default() })), "Sweden");
        assert_eq!(place_of(None), "");
        assert_eq!(zone(Some(&a)), None, "an empty zone is none");
        assert_eq!(updated_text(&clocks, 0.0, "ARN", None), "");
        assert_eq!(updated_text(&clocks, 1_782_036_000_000.0, "ARN", Some("Europe/Stockholm")), "Updated 12:00 · times at ARN");
        assert_eq!(updated_text(&clocks, 1_782_036_000_000.0, "ARN", None), "Updated 10:00 · your time");
        let widths: Vec<[f32; 3]> = (0..SKELETON_ROWS).map(skeleton_widths).collect();
        assert_eq!(widths[0], [38.0, 90.0, 60.0]);
        assert_eq!(widths[1], [38.0, 121.0, 77.0]);
        assert_eq!(widths[6], [38.0, 96.0, 72.0]);
    }
}
