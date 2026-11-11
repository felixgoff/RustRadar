//! TrainList, the trains list (doc 06 §15; `TrainList.svelte`, `train-list.ts`): the trains in view
//! in the sidebar's list card while trains are the focus
//! ([`super::super::shell::Slots::train_list`]).
//!
//! What it lists ([`listed`], `trainsListed`): the loaded trains the filters keep that still run
//! (`trainsFiltered`), narrowed by the trains' search (the shell's [`Pass::query`], trimmed), in
//! the filters' sort (`sortTrains`; by distance from the map's centre snapped to 0.05° steps,
//! [`list_centre`]).
//!
//! - **Header** (`header`: a baseline row, space between, padding `0 8px 8px`): `Trains in view`
//!   (14 px at 650) and, once loaded with trains in view, how many are listed (Mono 12 px
//!   `--text-3`, en-GB grouping), its line box [`HINT_DROP`] below the title's (the two baselines,
//!   as Chromium lines them up): 27.59 px.
//! - **Sort** (padding `0 8px 8px`, tooltip `Distance is from the centre of the map`): the
//!   segmented `Delay`, `Speed`, `Name`, `Distance` without its legend; a choice changes
//!   `trainFilters.sort` ([`Action::SetTrainFilters`]).
//! - **Loading** (not loaded, not failed): six 40 px placeholder rows ([`skeleton_widths`]: `70 +
//!   (i·23 mod 50)` × 12 on the left, 52 × 12 on the right, `110 + (i·31 mod 80)` × 10 under them).
//! - **None in view** (`total === 0`): `Couldn't reach the train feeds. Trying again shortly.`
//!   (failed) or `No trains in view.`, then the coverage line (12 px `--text-3`); the message's
//!   parts 12 apart, padding 8, 13 px `--text-2`.
//! - **None match**: `No trains in view match the search and filters.` and the `.button` `Show all
//!   trains`: the search emptied ([`Ask::EmptyQuery`]) and the filters back to their defaults with
//!   the sort kept.
//! - **Rows** (at most [`LIMIT`]; buttons: a `minmax(0, 1fr) auto` grid, gaps `2px 8px`, padding 8,
//!   radius 4):
//!   - line 1: the 8 × 8 swatch (radius 2, the train's map colour, tooltip `{kind} train`) and 8 px
//!     on the name (13 px at 650, cut with `…`, tooltip the name; `--amber` when selected); on the
//!     right the delay badge (`On time`, `5 min late`, `2 min early` in its tone) or nothing;
//!   - line 2 (16 px in, 12 px `--text-2`, 4 apart): `ArrowRight` 11/2 in `--text-3` and the
//!     destination (cut, tooltip it), or the kind in `--text-3`; on the right the measure (Mono 12
//!     px): the distance when sorted by distance, else `{n} km/h` when the speed is known;
//!   - the right column is as wide as the widest of its two items and both stretch across it (the
//!     badge's fill too, its text at its left, as CSS's grid stretches an item); a row without a
//!     badge is the name's line high (52.99 px), with one the badge's (53.3);
//!   - `--surface-hover` under the pointer (150 ms), the selected train on `--amber-soft`; a click
//!     picks the train (`pickTrain`: [`Action::PickTrain`]); the pointer or the keyboard on a row
//!     highlights its train on the map, off the rows nothing ([`Action::HoverTrain`]).
//! - **Notes** (11 px `--text-3`, padding `8px 8px 4px`): past [`LIMIT`] rows `{n} more; search or
//!   filter to narrow the list.`; the coverage line (no top padding after the first note).
//!
//! Deviations: the digits are Overpass's own (no `tabular-nums` in egui); text takes egui's
//! baseline, up to a pixel lower than Chromium's (U1); a part's tooltip shows while the pointer
//! rests on that part of the row (Chromium's `title` of the innermost element).

use egui::{Align2, Color32, CornerRadius, CursorIcon, Id, Pos2, Rect, Response, Sense, Ui, Vec2};
use rustradar_core::geo::Rgb;
use rustradar_core::model::{Train, f32_as_js};
use rustradar_core::num::js_round;
use rustradar_core::text::group_thousands;
use rustradar_core::train_list::{
    DEFAULT_TRAIN_FILTERS, TRAIN_COVERAGE, TrainFilters, TrainSort, distance_km, distance_label, sort_trains, train_filter, train_matches,
};
use rustradar_core::trains::{LatLon, delay_text, delay_tone, train_color_of, train_label, train_running};
use rustradar_scene::camera::ViewState;

use crate::state::search::Ask;
use crate::state::{Action, AppState};
use crate::ui::icons::{self, Icon, STROKE_REGULAR};
use crate::ui::shell::Pass;
use crate::ui::text::{self, Style};
use crate::ui::widgets::badge::{Badge, PADDING as BADGE_PAD, Tone};
use crate::ui::widgets::button::{Button, focus_ring, transition_color};
use crate::ui::widgets::segmented::{Choice, Segmented};
use crate::ui::widgets::token_input::js_trim;
use crate::ui::widgets::{skeleton, tooltip};
use crate::ui::{anim, theme};

/// The header's title.
pub const TITLE: &str = "Trains in view";
/// The sort's tooltip, and its legend (named, not drawn).
pub const SORT_TOOLTIP: &str = "Distance is from the centre of the map";
pub const SORT_LEGEND: &str = "Sort trains by";
/// The sorts, as the segmented shows them.
pub const SORTS: [(TrainSort, &str); 4] = [(TrainSort::Delay, "Delay"), (TrainSort::Speed, "Speed"), (TrainSort::Name, "Name"), (TrainSort::Distance, "Distance")];
/// Rows drawn at most: a long list is narrowed with the search, not scrolled through.
pub const LIMIT: usize = 150;
/// What the list says with no trains in view.
pub const FAILED: &str = "Couldn't reach the train feeds. Trying again shortly.";
pub const NONE_IN_VIEW: &str = "No trains in view.";
/// What it says when the search and the filters leave none, and the button that clears them.
pub const NO_MATCH: &str = "No trains in view match the search and filters.";
pub const SHOW_ALL: &str = "Show all trains";

/// The header's and the sort's padding at the sides and below (`padding: 0 var(--space-2)
/// var(--space-2)`).
pub const PAD: f32 = theme::SPACE_2;
/// How far the count's line box starts below the title's (their baselines lined up: Chromium's
/// layout of today's markup).
pub const HINT_DROP: f32 = 2.0;
/// A row's padding, and its grid gaps: across, and between the two lines.
pub const ROW_PAD: f32 = theme::SPACE_2;
pub const ROW_GAP: Vec2 = Vec2::new(theme::SPACE_2, 2.0);
/// The swatch's side and its gap to the name.
pub const SWATCH: f32 = 8.0;
pub const NAME_GAP: f32 = theme::SPACE_2;
/// The second line's indent, its parts' gap, and the arrow.
pub const SUB_INDENT: f32 = 16.0;
pub const SUB_GAP: f32 = theme::SPACE_1;
pub const ARROW: f32 = 11.0;
/// The placeholder rows: how many, their skeletons' heights, the right one's width.
pub const SKELETON_ROWS: usize = 6;
pub const SKELETON_HEIGHT: f32 = 12.0;
pub const SKELETON_WIDE_HEIGHT: f32 = 10.0;
pub const SKELETON_RIGHT: f32 = 52.0;
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

/// A row's name (`.name`: 13 px at 650), `--amber` when selected.
pub const fn name_style(selected: bool) -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_BADGE, if selected { theme::AMBER } else { theme::TEXT_1 })
}

/// The second line (`.sub`: 12 px `--text-2`), and the kind without a destination (`.quiet`).
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

/// A colour of `trains.ts` as egui's.
pub fn color(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

/// `s` with its first letter in capitals (`capital`).
pub fn capital(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The map's centre for the distances, in 0.05° steps (`listCentre`: `Math.round(v * 20) / 20`),
/// so a moving camera sorts again now and then rather than every frame.
pub fn list_centre(view: &ViewState) -> LatLon {
    LatLon { lat: js_round(view.latitude * 20.0) / 20.0, lon: js_round(view.longitude * 20.0) / 20.0 }
}

/// The trains the list shows for `state` at local time `now_ms` with the search `query`
/// (`trainsListed`): the filtered trains that still run, those matching the query, in the sort.
pub fn listed<'a>(state: &'a AppState, query: &str, now_ms: f64) -> Vec<&'a Train> {
    let filters = &state.persisted.train_filters;
    let keep = train_filter(filters);
    let now = now_ms as i64;
    let q = js_trim(query);
    let shown = state.trains.trains().iter().filter(|t| keep(t) && train_running(t, now) && (q.is_empty() || train_matches(t, q)));
    let centre = if filters.sort == TrainSort::Distance { list_centre(&state.controller.view()) } else { LatLon { lat: 0.0, lon: 0.0 } };
    sort_trains(shown, filters.sort, centre)
}

/// The filters after Show all: the defaults with the sort kept.
pub fn shown_all(filters: &TrainFilters) -> TrainFilters {
    TrainFilters { sort: filters.sort, ..DEFAULT_TRAIN_FILTERS }
}

/// The note past [`LIMIT`] rows: `{n} more; search or filter to narrow the list.`
pub fn more_text(listed: usize) -> Option<String> {
    (listed > LIMIT).then(|| format!("{} more; search or filter to narrow the list.", group_thousands((listed - LIMIT) as f64)))
}

/// A row's measure: the distance from `centre` when sorted by distance, else the speed when known.
pub fn measure_text(train: &Train, sort: TrainSort, centre: LatLon) -> Option<String> {
    if sort == TrainSort::Distance {
        Some(distance_label(distance_km(train.lat, train.lon, centre)))
    } else {
        train.speed_kmh.map(|s| format!("{} km/h", js_round(f32_as_js(s))))
    }
}

/// A row's delay badge: its words (`delayText` with a capital) and tone (`delayTone ?? neutral`);
/// none while the delay is not known.
pub fn delay_badge(train: &Train) -> Option<(String, Tone)> {
    let delay = delay_text(train.delay_minutes);
    (!delay.is_empty()).then(|| (capital(&delay), delay_tone(train.delay_minutes).map_or(Tone::Neutral, Tone::from)))
}

/// Placeholder row `i`'s widths: the first line's left skeleton and the second line's.
pub fn skeleton_widths(i: usize) -> (f32, f32) {
    (70.0 + ((i * 23) % 50) as f32, 110.0 + ((i * 31) % 80) as f32)
}

/// What the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// No answer yet and no failure: the placeholder rows.
    Loading,
    /// No trains in view: the feeds failed (`true`) or there are none.
    NoneInView { failed: bool },
    /// Trains in view, none listed: the message and Show all.
    NoMatch,
    /// The rows.
    Rows,
}

/// What the list shows for `state` with `listed` trains listed.
pub fn body(state: &AppState, listed: usize) -> Body {
    let trains = &state.trains;
    if !trains.loaded() && !trains.failed() {
        Body::Loading
    } else if trains.trains().is_empty() {
        Body::NoneInView { failed: trains.failed() }
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

/// A row's first line: the name's line box, or the badge's (`badge_height`) when it has one.
pub fn first_line(badge_height: Option<f32>) -> f32 {
    let name = name_style(false).line_px();
    badge_height.map_or(name, |h| name.max(h))
}

/// A row's height.
pub fn row_height(badge_height: Option<f32>) -> f32 {
    2.0 * ROW_PAD + first_line(badge_height) + ROW_GAP.y + sub_style(false).line_px()
}

/// The list in the list card (the shell's [`Slot`](super::super::shell::Slot)).
pub fn show(ui: &mut Ui, pass: &mut Pass<'_>) {
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let state = pass.state;
    let query = pass.query().to_owned();
    let rows = listed(state, &query, pass.now_ms);
    let trains = &state.trains;
    let hint = (trains.loaded() && !trains.trains().is_empty()).then(|| group_thousands(rows.len() as f64));
    header(ui, hint.as_deref());
    let mut filters = state.persisted.train_filters.clone();
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
            let sort = state.persisted.train_filters.sort;
            let centre = list_centre(&state.controller.view());
            let selected = trains.selected_id();
            for t in rows.iter().take(LIMIT) {
                let response = row(ui, t, sort, centre, selected == Some(t.id.as_str()));
                if response.hovered() || response.has_focus() {
                    hovered = index_of(state, t);
                }
                if response.clicked()
                    && let Some(i) = index_of(state, t)
                {
                    pass.actions.push(Action::PickTrain(i));
                }
            }
            notes(ui, rows.len());
        }
    }
    let hovered_id = hovered.and_then(|i| trains.trains().get(i)).map(|t| t.id.as_str());
    if hovered_id != state.hovered_train.as_deref() {
        pass.actions.push(Action::HoverTrain(hovered));
    }
    if filters != state.persisted.train_filters {
        state.train_filters_handed.set(Some(filters));
        pass.actions.push(Action::SetTrainFilters);
    }
}

/// Where `train` (a row of [`listed`]) stands in the loaded trains.
fn index_of(state: &AppState, train: &Train) -> Option<usize> {
    state.trains.trains().iter().position(|t| std::ptr::eq(t, train))
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
fn sort(ui: &mut Ui, value: &mut TrainSort) {
    let width = ui.available_width();
    let choices: Vec<Choice<'_, TrainSort>> = SORTS.iter().map(|&(v, label)| Choice::new(v, label)).collect();
    let top = ui.cursor().min;
    ui.horizontal(|ui| {
        ui.add_space(PAD);
        ui.vertical(|ui| {
            ui.set_width(width - 2.0 * PAD);
            Segmented::new("rustradar::lists::trains::sort", SORT_LEGEND, &choices).hide_legend().show(ui, value);
        });
    });
    ui.add_space(PAD);
    let rect = Rect::from_min_max(top, Pos2::new(top.x + width, ui.cursor().min.y));
    tooltip::show(ui.interact(rect, Id::unique("rustradar::lists::trains::sort-tip"), Sense::hover()), SORT_TOOLTIP);
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
    ui.add_space(MESSAGE_PAD);
    ui.horizontal(|ui| {
        ui.add_space(MESSAGE_PAD);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            add(ui, width - 2.0 * MESSAGE_PAD);
        });
    });
    ui.add_space(MESSAGE_PAD);
}

/// No trains in view: why, and where trains are shown.
fn none_in_view(ui: &mut Ui, failed: bool) {
    message(ui, |ui, width| {
        text::label(ui, text::wrapped(if failed { FAILED } else { NONE_IN_VIEW }, &message_style(), width), Sense::hover());
        ui.add_space(MESSAGE_GAP);
        text::label(ui, text::wrapped(TRAIN_COVERAGE, &coverage_style(), width), Sense::hover());
    });
}

/// None match the search and the filters: the message and Show all; answers whether it was
/// clicked.
fn no_match(ui: &mut Ui) -> bool {
    let mut clicked = false;
    message(ui, |ui, width| {
        text::label(ui, text::wrapped(NO_MATCH, &message_style(), width), Sense::hover());
        ui.add_space(MESSAGE_GAP);
        clicked = Button::text_only(SHOW_ALL).show(ui).clicked();
    });
    clicked
}

/// The notes under the rows: how many more past the limit, and the coverage line.
fn notes(ui: &mut Ui, listed: usize) {
    let width = ui.available_width();
    let more = more_text(listed);
    for (i, line) in more.iter().map(String::as_str).chain([TRAIN_COVERAGE]).enumerate() {
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

/// One train's row; answers its response (clicks, hover, focus).
fn row(ui: &mut Ui, t: &Train, sort: TrainSort, centre: LatLon, selected: bool) -> Response {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let id = Id::unique(("rustradar::lists::trains::row", t.id.as_str()));
    let badge = delay_badge(t).map(|(words, tone)| Badge::new(words, tone));
    let badge_size = badge.as_ref().map(|b| b.measure(&ctx));
    let line1 = first_line(badge_size.map(|s| s.y));
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, row_height(badge_size.map(|s| s.y))), Sense::hover());
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

    // the right column: as wide as the badge or the measure, whichever is wider; both stretch
    let measure = measure_text(t, sort, centre).map(|m| painter.layout_job(text::job(&m, &measure_style())));
    let right_w = badge_size.map_or(0.0, |s| s.x).max(measure.as_ref().map_or(0.0, |m| m.size().x));
    let right = content.right() - right_w;
    let left_w = (right - ROW_GAP.x - content.left()).max(0.0);

    // line 1: the swatch and the name, the badge
    let middle = content.top() + line1 / 2.0;
    let swatch = Rect::from_min_size(Pos2::new(content.left(), middle - SWATCH / 2.0), Vec2::splat(SWATCH));
    painter.rect_filled(swatch, CornerRadius::same(theme::RADIUS_SWATCH), color(train_color_of(t)));
    let name_x = swatch.right() + NAME_GAP;
    let name = painter.layout_job(text::truncated(&t.name, &name_style(selected), (content.left() + left_w - name_x).max(0.0)));
    let name_line = name_style(false).line_px();
    let name_rect = Rect::from_min_size(Pos2::new(name_x, middle - name_line / 2.0), Vec2::new(name.size().x, name_line));
    painter.galley(Pos2::new(name_x, middle - name.size().y / 2.0), name, Color32::PLACEHOLDER);
    if let (Some((words, tone)), Some(size)) = (delay_badge(t), badge_size) {
        // stretched across the column: the fill as wide, the words at its left
        let at = Pos2::new(right, middle - size.y / 2.0);
        painter.rect_filled(Rect::from_min_size(at, Vec2::new(right_w, size.y)), CornerRadius::same(theme::RADIUS), tone.colors().0);
        let galley = painter.layout_job(text::job(&words, &Badge::new(words.as_str(), tone).style()));
        painter.galley(Pos2::new(at.x + BADGE_PAD.x, middle - galley.size().y / 2.0), galley, Color32::PLACEHOLDER);
    }

    // line 2: the arrow and the destination, or the kind; the measure
    let line2 = sub_style(false).line_px();
    let top = content.top() + line1 + ROW_GAP.y;
    let mid2 = top + line2 / 2.0;
    let mut x = content.left() + SUB_INDENT;
    let room = |x: f32| (content.left() + left_w - x).max(0.0);
    let mut dest_rect = None;
    match &t.destination {
        Some(dest) if !dest.is_empty() => {
            let arrow = Rect::from_center_size(Pos2::new(x + ARROW / 2.0, mid2), Vec2::splat(ARROW));
            icons::paint(&painter, arrow, Icon::ArrowRight.at(ARROW, STROKE_REGULAR), theme::TEXT_3);
            x += ARROW + SUB_GAP;
            let galley = painter.layout_job(text::truncated(dest, &sub_style(false), room(x)));
            dest_rect = Some(Rect::from_min_size(Pos2::new(x, top), Vec2::new(galley.size().x, line2)));
            painter.galley(Pos2::new(x, mid2 - galley.size().y / 2.0), galley, Color32::PLACEHOLDER);
        }
        _ => {
            let galley = painter.layout_job(text::truncated(train_label(t.kind), &sub_style(true), room(x)));
            painter.galley(Pos2::new(x, mid2 - galley.size().y / 2.0), galley, Color32::PLACEHOLDER);
        }
    }
    if let Some(measure) = measure {
        let at = Align2::RIGHT_CENTER.anchor_size(Pos2::new(content.right(), mid2), measure.size());
        painter.galley(at.min, measure, Color32::PLACEHOLDER);
    }
    if response.has_focus() {
        focus_ring(&painter, rect, theme::RADIUS_SM);
    }

    // the parts' titles: the swatch's kind, the name, the destination
    if let Some(pointer) = response.hover_pos() {
        let tip = if swatch.contains(pointer) {
            Some(format!("{} train", train_label(t.kind)))
        } else if Rect::from_min_max(swatch.min, name_rect.max).contains(pointer) {
            Some(t.name.clone())
        } else {
            dest_rect.filter(|r| r.contains(pointer)).and(t.destination.clone())
        };
        if let Some(tip) = tip {
            return tooltip::show(response, &tip);
        }
    }
    response
}
