//! ShipList, the ships list (doc 06 §16; `ShipList.svelte`, `ship-list.ts`): the ships in view in
//! the sidebar's list card while ships are the focus ([`super::super::shell::Slots::ship_list`]).
//!
//! What it lists ([`listed`], `shipsListed`): the loaded ships the filters keep, narrowed by the
//! ships' search (the shell's [`Pass::query`], trimmed), in the filters' sort (`sortShips`; by
//! distance from the map's centre snapped to 0.05° steps, [`list_centre`]).
//!
//! - **Header** (`header`: a baseline row, space between, padding `0 8px 8px`): `Ships in view`
//!   (14 px at 650) and, once loaded with ships in view, how many are listed (Mono 12 px
//!   `--text-3`, en-GB grouping), its line box [`HINT_DROP`] below the title's (their baselines
//!   lined up): 27.59 px.
//! - **Sort** (padding `0 8px 8px`, tooltip `Distance is from the centre of the map`): the
//!   segmented `Size`, `Name`, `Distance` without its legend; a choice changes `shipFilters.sort`
//!   ([`Action::SetShipFilters`]).
//! - **Loading** (not loaded, not failed): six 40 px placeholder rows ([`skeleton_widths`]: `80 +
//!   (i·23 mod 60)` × 12 on the left, 40 × 12 on the right, `100 + (i·31 mod 70)` × 10 under them).
//! - **None in view** (`total === 0`): `Couldn't reach VesselFinder. Trying again in a minute.`
//!   (failed) or `No ships in view.`, then the coverage line (12 px `--text-3`); the message's
//!   parts 12 apart, padding 8, 13 px `--text-2`.
//! - **None match**: `No ships in view match the search and filters.` and the `.button` `Show all
//!   ships`: the search emptied ([`Ask::EmptyQuery`]) and the filters back to their defaults with
//!   the sort kept.
//! - **Rows** (at most [`LIMIT`]; buttons: a `minmax(0, 1fr) auto` grid, gaps `2px 8px`, padding 8,
//!   radius 4, items centred):
//!   - line 1: the 8 px round swatch in the ship's map colour and 8 px on the name (Mono 13 px at
//!     650, cut with `…`, tooltip the name; `--amber` when selected), or for a ship that never
//!     broadcast one `MMSI ` and the number in Mono (weight 500, `--text-2`); on the right the
//!     measure (Mono 12 px `--text-2`, right-aligned, [`measure_text`]): the distance when sorted
//!     by distance, else the length (`{bow + stern} m`) or, where the feed tells sizes apart, the
//!     size class;
//!   - line 2 (16 px in, 12 px `--text-2`, 8 apart): the short type (`shipKindShort`, cut with
//!     `…`) and `Under way` (a heading known) or `Moored` in `--text-3`; on the right the warn
//!     badge `Old position` (tooltip `This ship hasn't reported its position lately`) when the
//!     position is old;
//!   - the right column is as wide as the wider of the measure and the badge; a row is 52.99 px
//!     high, with the badge 54.69 (its line box under the name's);
//!   - `--surface-hover` under the pointer (150 ms), the selected ship on `--amber-soft`; a click
//!     picks the ship (`pickShip`: [`Action::PickShip`]); the pointer or the keyboard on a row
//!     highlights its ship on the map, off the rows nothing ([`Action::HoverShip`]).
//! - **Notes** (11 px `--text-3`, padding `8px 8px 4px`): past [`LIMIT`] rows `{n} more; search or
//!   filter to narrow the list.`; the coverage line (no top padding after the first note).
//!
//! **Checked** against Chromium's layout of today's component, server-rendered by Svelte with the
//! same props and laid out with app.css (`tests/ship_ui.rs`): the header, the sort, the rows'
//! boxes, swatches, measures and badges, the placeholders and the messages land on Chromium's
//! pixels or within one.
//!
//! Deviations: text takes egui's baseline, up to a pixel lower than Chromium's (U1); a part's
//! tooltip shows while the pointer rests on that part of the row (Chromium's `title` of the
//! innermost element).

use egui::{Align2, Color32, CornerRadius, CursorIcon, Id, Pos2, Rect, Response, Sense, Ui, Vec2};
use rustradar_core::geo::Rgb;
use rustradar_core::model::Ship;
use rustradar_core::ship_list::{DEFAULT_SHIP_FILTERS, SHIP_COVERAGE, SHIP_SIZES, ShipFilters, ShipSort, ship_kind_short, ship_length, ship_name};
use rustradar_core::ships::ship_color;
use rustradar_core::text::group_thousands;
use rustradar_core::train_list::{distance_km, distance_label};
use rustradar_core::trains::LatLon;
use rustradar_scene::camera::ViewState;

use crate::state::search::Ask;
use crate::state::ships::sizes_known;
use crate::state::{Action, AppState};
use crate::ui::panel;
use crate::ui::shell::Pass;
use crate::ui::text::{self, Style};
use crate::ui::widgets::badge::{Badge, Tone};
use crate::ui::widgets::button::{Button, focus_ring, transition_color};
use crate::ui::widgets::segmented::{Choice, Segmented};
use crate::ui::widgets::{skeleton, tooltip};
use crate::ui::{anim, theme};

/// The header's title.
pub const TITLE: &str = "Ships in view";
/// The sort's tooltip, and its legend (named, not drawn).
pub const SORT_TOOLTIP: &str = "Distance is from the centre of the map";
pub const SORT_LEGEND: &str = "Sort ships by";
/// The sorts, as the segmented shows them.
pub const SORTS: [(ShipSort, &str); 3] = [(ShipSort::Size, "Size"), (ShipSort::Name, "Name"), (ShipSort::Distance, "Distance")];
/// Rows drawn at most: a long list is narrowed with the search, not scrolled through.
pub const LIMIT: usize = 150;
/// What the list says with no ships in view.
pub const FAILED: &str = "Couldn't reach VesselFinder. Trying again in a minute.";
pub const NONE_IN_VIEW: &str = "No ships in view.";
/// What it says when the search and the filters leave none, and the button that clears them.
pub const NO_MATCH: &str = "No ships in view match the search and filters.";
pub const SHOW_ALL: &str = "Show all ships";
/// An unnamed ship's prefix before its MMSI.
pub const MMSI_PREFIX: &str = "MMSI ";
/// The ship's motion on the second line.
pub const UNDER_WAY: &str = "Under way";
pub const MOORED: &str = "Moored";
/// The old position's badge and its tooltip.
pub const OLD_POSITION: &str = "Old position";
pub const OLD_TOOLTIP: &str = "This ship hasn't reported its position lately";

/// The header's and the sort's padding at the sides and below (`padding: 0 var(--space-2)
/// var(--space-2)`).
pub const PAD: f32 = theme::SPACE_2;
/// How far the count's line box starts below the title's (their baselines lined up: Chromium's
/// layout of today's markup).
pub const HINT_DROP: f32 = 2.0;
/// A row's padding, and its grid gaps: across, and between the two lines.
pub const ROW_PAD: f32 = theme::SPACE_2;
pub const ROW_GAP: Vec2 = Vec2::new(theme::SPACE_2, 2.0);
/// The swatch's diameter and its gap to the name (`.name { gap: var(--space-2) }`).
pub const SWATCH: f32 = 8.0;
pub const NAME_GAP: f32 = theme::SPACE_2;
/// The second line's indent and its parts' gap.
pub const SUB_INDENT: f32 = 16.0;
pub const SUB_GAP: f32 = theme::SPACE_2;
/// The placeholder rows: how many, their skeletons' heights, the right one's width.
pub const SKELETON_ROWS: usize = 6;
pub const SKELETON_HEIGHT: f32 = 12.0;
pub const SKELETON_WIDE_HEIGHT: f32 = 10.0;
pub const SKELETON_RIGHT: f32 = 40.0;
/// The message's padding and the gap between its parts (`.message`).
pub const MESSAGE_PAD: f32 = theme::SPACE_2;
pub const MESSAGE_GAP: f32 = theme::SPACE_3;
/// The notes' padding: top and sides, bottom (`padding: var(--space-2) var(--space-2)
/// var(--space-1)`).
pub const NOTE_PAD: f32 = theme::SPACE_2;
pub const NOTE_PAD_BOTTOM: f32 = theme::SPACE_1;

/// The title (`h2`: 14 px at 650).
pub const fn title_style() -> Style {
    Style::ui(theme::TEXT_BASE, theme::WEIGHT_BADGE, theme::TEXT_1)
}

/// The count on the header's right (`.hint.data`: Mono 12 px `--text-3`).
pub const fn hint_style() -> Style {
    Style::data(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// A row's name (`.name .text.data`: Mono 13 px at 650), `--amber` when selected.
pub const fn name_style(selected: bool) -> Style {
    Style::data(theme::TEXT_MD, theme::WEIGHT_BADGE, if selected { theme::AMBER } else { theme::TEXT_1 })
}

/// An unnamed ship's `MMSI ` (`.unnamed`: weight 500, `--text-2`), `--amber` when selected (the
/// selected row's `.text` rule wins).
pub const fn unnamed_style(selected: bool) -> Style {
    Style::ui(theme::TEXT_MD, 500, if selected { theme::AMBER } else { theme::TEXT_2 })
}

/// The second line (`.sub`: 12 px `--text-2`), and the motion (`.state`: `--text-3`).
pub const fn sub_style(quiet: bool) -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, if quiet { theme::TEXT_3 } else { theme::TEXT_2 })
}

/// The measure (`.measure.data`: Mono 12 px `--text-2`).
pub const fn measure_style() -> Style {
    Style::data(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// The message (`.message`: 13 px `--text-2`).
pub const fn message_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// The coverage line in the message (`.coverage`: 12 px `--text-3`).
pub const fn coverage_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// The notes under the rows (`.more`, `.foot`: 11 px `--text-3`).
pub const fn note_style() -> Style {
    Style::ui(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// A colour of `ships.ts` as egui's.
pub fn color(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

/// The map's centre for the distances, in 0.05° steps (`listCentre`).
pub fn list_centre(view: &ViewState) -> LatLon {
    crate::state::search::list_centre(view.latitude, view.longitude)
}

/// The ships the list shows for `state` with the search `query` (`shipsListed`): the filtered
/// ships matching the query, in the sort.
pub fn listed<'a>(state: &'a AppState, query: &str) -> Vec<&'a Ship> {
    state.ships.listed(&state.persisted.ship_filters, query, list_centre(&state.controller.view()))
}

/// The filters after Show all: the defaults with the sort kept.
pub fn shown_all(filters: &ShipFilters) -> ShipFilters {
    ShipFilters { sort: filters.sort, ..DEFAULT_SHIP_FILTERS }
}

/// The note past [`LIMIT`] rows: `{n} more; search or filter to narrow the list.`
pub fn more_text(listed: usize) -> Option<String> {
    (listed > LIMIT).then(|| format!("{} more; search or filter to narrow the list.", group_thousands((listed - LIMIT) as f64)))
}

/// A row's measure (`size(s)`, or the distance): the distance from `centre` when sorted by
/// distance; else the length when the feed gives dimensions (a length of 0 reads as none), else
/// the size class where `sizes_known`, else nothing.
pub fn measure_text(ship: &Ship, sort: ShipSort, centre: LatLon, sizes_known: bool) -> String {
    if sort == ShipSort::Distance {
        return distance_label(distance_km(ship.lat, ship.lon, centre));
    }
    match ship_length(ship).filter(|l| *l != 0) {
        Some(length) => format!("{length} m"),
        None if sizes_known => SHIP_SIZES.iter().find(|(size, _)| *size == ship.size).map_or("", |(_, label)| *label).to_owned(),
        None => String::new(),
    }
}

/// The second line's motion: `Under way` with a heading, else `Moored`.
pub fn motion_text(ship: &Ship) -> &'static str {
    if ship.heading.is_some() { UNDER_WAY } else { MOORED }
}

/// Placeholder row `i`'s widths: the first line's left skeleton and the second line's.
pub fn skeleton_widths(i: usize) -> (f32, f32) {
    (80.0 + ((i * 23) % 60) as f32, 100.0 + ((i * 31) % 70) as f32)
}

/// What the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// No answer yet and no failure: the placeholder rows.
    Loading,
    /// No ships in view: VesselFinder failed (`true`) or there are none.
    NoneInView { failed: bool },
    /// Ships in view, none listed: the message and Show all.
    NoMatch,
    /// The rows.
    Rows,
}

/// What the list shows for `state` with `listed` ships listed.
pub fn body(state: &AppState, listed: usize) -> Body {
    let ships = &state.ships;
    if !ships.loaded() && !ships.failed() {
        Body::Loading
    } else if ships.ships().is_empty() {
        Body::NoneInView { failed: ships.failed() }
    } else if listed == 0 {
        Body::NoMatch
    } else {
        Body::Rows
    }
}

/// The header's height: the count's line box below its drop or the title's, and the padding.
pub fn header_height() -> f32 {
    title_style().line_px().max(HINT_DROP + hint_style().line_px()) + PAD
}

/// A placeholder row's height.
pub const fn skeleton_row_height() -> f32 {
    2.0 * ROW_PAD + SKELETON_HEIGHT + ROW_GAP.y + SKELETON_WIDE_HEIGHT
}

/// A row's first line: the name's line box (taller than the measure's).
pub fn first_line() -> f32 {
    name_style(false).line_px().max(measure_style().line_px())
}

/// A row's second line: the sub's line box, or the badge's (`badge_height`) when it has one.
pub fn second_line(badge_height: Option<f32>) -> f32 {
    let sub = sub_style(false).line_px();
    badge_height.map_or(sub, |h| sub.max(h))
}

/// A row's height.
pub fn row_height(badge_height: Option<f32>) -> f32 {
    2.0 * ROW_PAD + first_line() + ROW_GAP.y + second_line(badge_height)
}

/// The old position's badge.
pub fn old_badge() -> Badge {
    Badge::new(OLD_POSITION, Tone::Warn)
}

/// `text` in `style` wrapped across `width`, taking its CSS line boxes (rows × the line height,
/// not egui's whole-pixel rows) from `ui`'s layout; answers its rectangle.
pub fn paragraph(ui: &mut Ui, text: &str, style: &Style, width: f32) -> Rect {
    let laid = panel::Piece::text(text, style, panel::Fit::Wrap).lay_out(ui.ctx(), width);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, laid.size.y), Sense::hover());
    laid.paint(ui.painter(), rect.min);
    rect
}

/// The list in the list card (the shell's [`Slot`](super::super::shell::Slot)).
pub fn show(ui: &mut Ui, pass: &mut Pass<'_>) {
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let state = pass.state;
    let query = pass.query().to_owned();
    let rows = listed(state, &query);
    let ships = &state.ships;
    let hint = (ships.loaded() && !ships.ships().is_empty()).then(|| group_thousands(rows.len() as f64));
    // on a phone the sheet's head already says how many are in view (U19)
    if !crate::ui::sheet::in_sheet(ui.ctx()) {
        header(ui, hint.as_deref());
    }
    let mut filters = state.persisted.ship_filters.clone();
    sort(ui, &mut filters.sort);
    let mut hovered = None;
    match body(state, rows.len()) {
        Body::Loading => loading(ui),
        Body::NoneInView { failed } => none_in_view(ui, failed),
        Body::NoMatch => {
            if no_match(ui) {
                pass.actions.push(Action::Search(Ask::EmptyQuery));
                filters = shown_all(&filters);
            }
        }
        Body::Rows => {
            let sort = state.persisted.ship_filters.sort;
            let centre = list_centre(&state.controller.view());
            let known = sizes_known(state.controller.view().zoom);
            let selected = ships.selected_mmsi();
            for s in rows.iter().take(LIMIT) {
                let response = row(ui, s, &measure_text(s, sort, centre, known), selected == Some(s.mmsi));
                if response.hovered() || response.has_focus() {
                    hovered = Some(s.mmsi);
                }
                if response.clicked() {
                    pass.actions.push(Action::PickShip(s.mmsi));
                }
            }
            notes(ui, rows.len());
        }
    }
    if hovered != state.hovered_ship {
        pass.actions.push(Action::HoverShip(hovered));
    }
    if filters != state.persisted.ship_filters {
        state.ship_filters_handed.set(Some(filters));
        pass.actions.push(Action::SetShipFilters);
    }
}

/// The header: the title, and the count on the right.
fn header(ui: &mut Ui, hint: Option<&str>) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, header_height()), Sense::hover());
    let painter = ui.painter();
    let line = |style: &Style, galley: &egui::Galley| (style.line_px() - galley.size().y) / 2.0;
    let title = painter.layout_job(text::job(TITLE, &title_style()));
    let top = rect.top() + line(&title_style(), &title);
    painter.galley(Pos2::new(rect.left() + PAD, top), title, Color32::PLACEHOLDER);
    if let Some(hint) = hint {
        let galley = painter.layout_job(text::job(hint, &hint_style()));
        let top = rect.top() + HINT_DROP + line(&hint_style(), &galley);
        painter.galley(Pos2::new(rect.right() - PAD - galley.size().x, top), galley, Color32::PLACEHOLDER);
    }
}

/// The sort's segmented, its tooltip over the whole `.sort` box.
fn sort(ui: &mut Ui, value: &mut ShipSort) {
    let width = ui.available_width();
    let choices: Vec<Choice<'_, ShipSort>> = SORTS.iter().map(|&(v, label)| Choice::new(v, label)).collect();
    let top = ui.cursor().min;
    ui.horizontal(|ui| {
        ui.add_space(PAD);
        ui.vertical(|ui| {
            ui.set_width(width - 2.0 * PAD);
            Segmented::new("rustradar::lists::ships::sort", SORT_LEGEND, &choices).hide_legend().show(ui, value);
        });
    });
    ui.add_space(PAD);
    let rect = Rect::from_min_max(top, Pos2::new(top.x + width, ui.cursor().min.y));
    tooltip::show(ui.interact(rect, Id::unique("rustradar::lists::ships::sort-tip"), Sense::hover()), SORT_TOOLTIP);
}

/// The six placeholder rows.
fn loading(ui: &mut Ui) {
    let width = ui.available_width();
    for i in 0..SKELETON_ROWS {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, skeleton_row_height()), Sense::hover());
        if !ui.is_rect_visible(rect) {
            continue;
        }
        let (left, wide) = skeleton_widths(i);
        let top = rect.top() + ROW_PAD;
        let x = rect.left() + ROW_PAD;
        let painter = ui.painter();
        skeleton::paint(painter, Rect::from_min_size(Pos2::new(x, top), Vec2::new(left, SKELETON_HEIGHT)), theme::RADIUS);
        let right = Rect::from_min_size(Pos2::new(rect.right() - ROW_PAD - SKELETON_RIGHT, top), Vec2::new(SKELETON_RIGHT, SKELETON_HEIGHT));
        skeleton::paint(painter, right, theme::RADIUS);
        let second = top + SKELETON_HEIGHT + ROW_GAP.y;
        skeleton::paint(painter, Rect::from_min_size(Pos2::new(x, second), Vec2::new(wide, SKELETON_WIDE_HEIGHT)), theme::RADIUS);
    }
}

/// `.message`: its parts top-down, 12 apart, padding 8 round them.
fn message(ui: &mut Ui, add: impl FnOnce(&mut Ui, f32)) {
    let width = ui.available_width();
    let inner = width - 2.0 * MESSAGE_PAD;
    let at = Rect::from_min_size(ui.cursor().min + Vec2::splat(MESSAGE_PAD), Vec2::new(inner, f32::INFINITY));
    let height = panel::region(ui, "rustradar::lists::ships::message", at, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        ui.set_width(inner);
        add(ui, inner);
        ui.min_rect().height()
    });
    ui.allocate_exact_size(Vec2::new(width, height + 2.0 * MESSAGE_PAD), Sense::hover());
}

/// No ships in view: why, and where ships come from.
fn none_in_view(ui: &mut Ui, failed: bool) {
    message(ui, |ui, width| {
        paragraph(ui, if failed { FAILED } else { NONE_IN_VIEW }, &message_style(), width);
        ui.add_space(MESSAGE_GAP);
        paragraph(ui, SHIP_COVERAGE, &coverage_style(), width);
    });
}

/// None match the search and the filters: the message and Show all; answers whether it was
/// clicked.
fn no_match(ui: &mut Ui) -> bool {
    let mut clicked = false;
    message(ui, |ui, width| {
        paragraph(ui, NO_MATCH, &message_style(), width);
        ui.add_space(MESSAGE_GAP);
        // a button takes the message's colour (`button { color: inherit }`)
        clicked = Button::text_only(SHOW_ALL).color(theme::TEXT_2).show(ui).clicked();
    });
    clicked
}

/// The notes under the rows: how many more past the limit, and the coverage line.
fn notes(ui: &mut Ui, listed: usize) {
    let width = ui.available_width();
    let more = more_text(listed);
    for (i, line) in more.iter().map(String::as_str).chain([SHIP_COVERAGE]).enumerate() {
        let top = if i == 0 { NOTE_PAD } else { 0.0 };
        let job = text::wrapped(line, &note_style(), width - 2.0 * NOTE_PAD);
        let galley = ui.painter().layout_job(job);
        let rows = galley.rows.len().max(1) as f32;
        let height = top + rows * note_style().line_px() + NOTE_PAD_BOTTOM;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        let at = Pos2::new(rect.left() + NOTE_PAD, rect.top() + top + (rows * note_style().line_px() - galley.size().y) / 2.0);
        ui.painter().galley(at, galley, Color32::PLACEHOLDER);
    }
}

/// One ship's row with its `measure`; answers its response (clicks, hover, focus).
fn row(ui: &mut Ui, s: &Ship, measure: &str, selected: bool) -> Response {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let id = Id::unique(("rustradar::lists::ships::row", s.mmsi));
    let badge = s.old.then(old_badge);
    let badge_size = badge.as_ref().map(|b| b.measure(&ctx));
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, row_height(badge_size.map(|b| b.y))), Sense::hover());
    let response = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    if !ui.is_rect_visible(rect) {
        return response;
    }
    // `button.row.active` comes after `button.row:hover`: the selection wins under the pointer
    let fill = if selected {
        theme::AMBER_SOFT
    } else if response.hovered() {
        theme::SURFACE_HOVER
    } else {
        Color32::TRANSPARENT
    };
    let fill = transition_color(&ctx, id.with("fill"), fill, anim::HOVER);
    let painter = ui.painter().clone();
    painter.rect_filled(rect, CornerRadius::same(theme::RADIUS_SM), fill);
    let content = rect.shrink(ROW_PAD);

    // the right column: as wide as the measure or the badge, whichever is wider
    let measure = (!measure.is_empty()).then(|| painter.layout_job(text::job(measure, &measure_style())));
    let right_w = badge_size.map_or(0.0, |b| b.x).max(measure.as_ref().map_or(0.0, |m| m.size().x));
    // the grid's gap stands between the columns even when the right one is empty
    let left_w = (content.width() - right_w - ROW_GAP.x).max(0.0);
    let left_right = content.left() + left_w;

    // line 1: the swatch and the name, the measure
    let line1 = first_line();
    let middle = content.top() + line1 / 2.0;
    painter.circle_filled(Pos2::new(content.left() + SWATCH / 2.0, middle), SWATCH / 2.0, color(ship_color(s.kind)));
    let name_x = content.left() + SWATCH + NAME_GAP;
    let name = ship_name(s);
    let room = (left_right - name_x).max(0.0);
    let job = if name.is_empty() {
        let mut job = egui::text::LayoutJob::default();
        text::append(&mut job, MMSI_PREFIX, &unnamed_style(selected));
        text::append(&mut job, &s.mmsi.to_string(), &panel::data(&unnamed_style(selected)));
        text::truncate(&mut job, room);
        job
    } else {
        text::truncated(name, &name_style(selected), room)
    };
    let galley = painter.layout_job(job);
    let name_rect = Rect::from_min_size(Pos2::new(name_x, middle - line1 / 2.0), Vec2::new(galley.size().x, line1));
    painter.galley(Pos2::new(name_x, middle - galley.size().y / 2.0), galley, Color32::PLACEHOLDER);
    if let Some(measure) = measure {
        let at = Align2::RIGHT_CENTER.anchor_size(Pos2::new(content.right(), middle), measure.size());
        painter.galley(at.min, measure, Color32::PLACEHOLDER);
    }

    // line 2: the short type and the motion, the old position's badge
    let line2 = second_line(badge_size.map(|b| b.y));
    let mid2 = content.top() + line1 + ROW_GAP.y + line2 / 2.0;
    let x = content.left() + SUB_INDENT;
    let motion = painter.layout_job(text::job(motion_text(s), &sub_style(true)));
    let kind_room = (left_right - x - SUB_GAP - motion.size().x).max(0.0);
    let kind = painter.layout_job(text::truncated(ship_kind_short(s.kind), &sub_style(false), kind_room));
    let motion_x = x + kind.size().x + SUB_GAP;
    painter.galley(Pos2::new(x, mid2 - kind.size().y / 2.0), kind, Color32::PLACEHOLDER);
    painter.galley(Pos2::new(motion_x, mid2 - motion.size().y / 2.0), motion, Color32::PLACEHOLDER);
    let mut badge_rect = None;
    if let (Some(badge), Some(size)) = (badge, badge_size) {
        badge_rect = Some(badge.paint(&painter, Pos2::new(content.right() - size.x, mid2 - size.y / 2.0)));
    }
    if response.has_focus() {
        focus_ring(&painter, rect, theme::RADIUS_SM);
    }

    // the parts' titles: the name, the badge
    if let Some(pointer) = response.hover_pos() {
        let tip = if !name.is_empty() && name_rect.contains(pointer) {
            Some(name)
        } else {
            badge_rect.filter(|r| r.contains(pointer)).map(|_| OLD_TOOLTIP)
        };
        if let Some(tip) = tip {
            return tooltip::show(response, tip);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ship(over: impl FnOnce(&mut Ship)) -> Ship {
        let mut s = Ship { mmsi: 265_177_000, lat: 57.7072, lon: 11.9441, name: "KATTEGAT EXPRESS".into(), kind: 3, size: 2, heading: Some(271.0), ..Ship::default() };
        over(&mut s);
        s
    }

    const CENTRE: LatLon = LatLon { lat: 57.7, lon: 11.95 };

    #[test]
    fn the_measure_is_the_distance_the_length_or_the_size_class() {
        let s = ship(|s| s.dims = Some([120, 64, 15, 15]));
        assert_eq!(measure_text(&s, ShipSort::Size, CENTRE, true), "184 m");
        assert_eq!(measure_text(&s, ShipSort::Name, CENTRE, false), "184 m");
        // Chromium's layout of today's list: 870 m from the snapped centre
        assert_eq!(measure_text(&s, ShipSort::Distance, CENTRE, true), "870 m");
        // no dimensions: the size class where the feed tells them apart, else nothing
        let s = ship(|s| s.size = 0);
        assert_eq!(measure_text(&s, ShipSort::Size, CENTRE, true), "Small");
        assert_eq!(measure_text(&s, ShipSort::Size, CENTRE, false), "");
        // a length of 0 is no length (`if (length)`); an unknown class reads as nothing
        let s = ship(|s| {
            s.dims = Some([0, 0, 3, 3]);
            s.size = 1;
        });
        assert_eq!(measure_text(&s, ShipSort::Size, CENTRE, true), "Medium");
        let s = ship(|s| s.size = 7);
        assert_eq!(measure_text(&s, ShipSort::Name, CENTRE, true), "");
    }

    #[test]
    fn rows_motion_placeholders_notes_and_show_all() {
        assert_eq!(motion_text(&ship(|_| {})), "Under way");
        assert_eq!(motion_text(&ship(|s| s.heading = None)), "Moored");
        assert_eq!(skeleton_widths(0), (80.0, 100.0));
        assert_eq!(skeleton_widths(3), (89.0, 123.0));
        assert_eq!(skeleton_widths(5), (135.0, 115.0));
        assert_eq!(more_text(150), None);
        assert_eq!(more_text(1_402).as_deref(), Some("1,252 more; search or filter to narrow the list."));
        let mut filters = DEFAULT_SHIP_FILTERS;
        filters.hidden_kinds = vec![4, 5];
        filters.hide_old = true;
        filters.sort = ShipSort::Distance;
        assert_eq!(shown_all(&filters), ShipFilters { sort: ShipSort::Distance, ..DEFAULT_SHIP_FILTERS });
    }

    #[test]
    fn heights_as_chromium_lays_out_the_rows() {
        // the header 27.59, a row 52.99 and with the badge 54.69, a placeholder 40
        assert!((header_height() - 27.6).abs() < 0.02, "{}", header_height());
        assert!((row_height(None) - 52.99).abs() < 0.02, "{}", row_height(None));
        assert!((row_height(Some(18.5)) - 54.7).abs() < 0.02, "{}", row_height(Some(18.5)));
        assert_eq!(skeleton_row_height(), 40.0);
    }
}
