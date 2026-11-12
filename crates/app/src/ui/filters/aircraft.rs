//! FilterPanel, the aircraft filters (doc 06 §11; `FilterPanel.svelte`, `filters.ts`): the
//! sidebar's filter panel while aircraft are the focus and the filters are open
//! ([`super::super::shell::Slots::aircraft_filters`]).
//!
//! **Header** (shared by the three filter panels; a row centred on its 32 px button, padding
//! `8px 8px 8px 16px`, 8 apart, a 1 px `--line` rule under it: 49 px): `Filters` (14 px at 650);
//! `{shown} of {total} shown` (12 px `--text-3`, taking the rest of the row); the text button
//! `Reset` (disabled while no filter is active); the 32 px icon button `X` 16/1.75 (tooltip `Close
//! filters (F)`).
//!
//! **Body** (padding 16, its groups 16 apart, scrolling within the height the sidebar gives):
//! 1. `Callsign, flight or registration` (12 px at 600 `--text-2`, 4 px over the field): the text
//!    field (36 px, 13 px) with the placeholder `e.g. BAW, LH400, D-AIM`: `filters.text`.
//! 2. `Categories` with `All` and `None` on the right (a legend 8 px over the chips): the twelve
//!    categories of `filters.ts` as chips, kept ascending ([`toggle_sorted`]).
//! 3. `Altitude` with the range on the right in Mono 12 px `--text-1` ([`altitude_text`]), 8 px
//!    over the two-thumb slider (0 to [`MAX_ALTITUDE`] in steps of [`STEP`], a step apart), then
//!    8 px under it the two checkboxes 16 apart, `In the air` and `On the ground` (13 px, the box
//!    8 px before its word).
//! 4. The token inputs `Airlines`, `Aircraft types` and `Airports` ([`suggest_airlines`],
//!    [`suggest_types`], [`suggest_airports`]; free entry: three letters, a type code of 2–4
//!    letters or digits, three letters; chips `{ICAO} {name}`, the type, `{IATA} {city}`).
//!
//! **Changes** reach the state as one [`Action::SetFilters`]: the panel edits a copy of
//! `persisted.filters` as `bind:filters` did, and when the copy differs at the end of the pass it
//! hands it over ([`AppState::filters_handed`]: the action is `Copy`) and pushes the action. The
//! state replaces the filters with it; `state::aircraft`'s next poll filters the snapshot again
//! with `core::filters::matcher` (and asks for a new snapshot when the categories changed), the
//! settings store saves them. Reset restores `DEFAULT_FILTERS`; the close button is
//! `Action::Open(Open::Filters, false)`.
//!
//! **Checkbox** ([`checkbox`]): Chromium's native box with `color-scheme: dark` and
//! `accent-color: --amber`, as it draws today's: 13 × 13, radius 2; off a 1 px `(133,133,133)`
//! border round `(59,59,59)`; on filled `--amber` with Chromium's check (from 20 % across and 50 %
//! down, 20 % down-right, then up to 20 % from the right and the top; stroke 16 % of the box) in
//! `(59,59,59)`; the whole label toggles it.
//!
//! **Android**: the text field and the three token inputs report their text to the soft
//! keyboard's bridge each frame they are shown (`platform::android::text::report`).
//!
//! Deviations: the live counts behind the suggestions are counted again when the snapshot or the
//! filters change, not on every change of the flights' array; Chromium's checkbox lightens its
//! border under the pointer, this one keeps its colours. The count's `tabular-nums` is Overpass's
//! tabular figures (`ui::text::tabular`, F1).

use std::cell::OnceCell;
use std::collections::HashMap;
use std::sync::Arc;

use egui::{Color32, CornerRadius, CursorIcon, Frame, Id, Margin, Pos2, Rect, ScrollArea, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, Vec2};
use rustradar_core::filters::{CATEGORIES, Filters, MAX_ALTITUDE, active_count};
use rustradar_core::model::LiveFlight;
use rustradar_core::reference::{Reference, airline_icao};
use rustradar_core::text::{group_thousands, locale_cmp};

use crate::state::shortcuts::Open;
use crate::state::{Action, AppState};
use crate::ui::icons::Icon;
use crate::ui::shell::Pass;
use crate::ui::text::{self, Style};
use crate::ui::theme;
use crate::ui::widgets::button::focus_ring;
use crate::ui::widgets::chips::{self, Chip, Kind, toggle_sorted};
use crate::ui::widgets::icon_button::{self, IconButton};
use crate::ui::widgets::range_slider::DualRange;
use crate::ui::widgets::text_field::TextField;
use crate::ui::widgets::token_input::{self, Suggestion, TokenInput, js_trim, three_letters, type_code};

/// The altitude slider's step, ft (`STEP`).
pub const STEP: i32 = 1_000;
/// How many suggestions a token input lists (`SUGGESTIONS`).
pub const SUGGESTIONS: usize = 6;

/// The header's padding: top and bottom, right, left (`padding: 8px 8px 8px 16px`).
pub const HEADER_PAD: f32 = theme::SPACE_2;
pub const HEADER_PAD_LEFT: f32 = theme::SPACE_4;
/// Between the header's parts.
pub const HEADER_GAP: f32 = theme::SPACE_2;
/// The header's height: its 32 px button, the padding and the rule under it.
pub const HEADER_HEIGHT: f32 = icon_button::SIZE + 2.0 * HEADER_PAD + theme::BORDER;
/// The body's padding and the gap between its groups.
pub const BODY_PAD: f32 = theme::SPACE_4;
pub const GROUP_GAP: f32 = theme::SPACE_4;
/// Between a label and its field.
pub const LABEL_GAP: f32 = theme::SPACE_1;
/// Under a fieldset's legend (`margin-bottom`), and between the fieldset's parts (`gap`).
pub const LEGEND_GAP: f32 = theme::SPACE_2;
/// Between the checkboxes (`.checks { gap }`), and between a box and its word.
pub const CHECKS_GAP: f32 = theme::SPACE_4;
pub const CHECK_LABEL_GAP: f32 = theme::SPACE_2;
/// The native checkbox's side and radius.
pub const CHECKBOX: f32 = 13.0;
pub const CHECKBOX_RADIUS: u8 = 2;
/// Its border off, and its fill off (and its check on).
pub const CHECKBOX_BORDER: Color32 = Color32::from_rgb(133, 133, 133);
pub const CHECKBOX_FILL: Color32 = Color32::from_rgb(59, 59, 59);

/// The header's title.
pub const TITLE: &str = "Filters";
/// The close button's name and tooltip.
pub const CLOSE: &str = "Close filters";
pub const CLOSE_TOOLTIP: &str = "Close filters (F)";
pub const TEXT_LABEL: &str = "Callsign, flight or registration";
pub const TEXT_PLACEHOLDER: &str = "e.g. BAW, LH400, D-AIM";
pub const AIRLINES_PLACEHOLDER: &str = "Name or code";
pub const TYPES_PLACEHOLDER: &str = "e.g. A388 or B77";
pub const TYPES_HINT: &str = "A type code, or its start: B77 matches every Boeing 777.";
pub const AIRPORTS_PLACEHOLDER: &str = "City, name or code";
pub const AIRPORTS_HINT: &str = "Flights departing from or flying to any of them.";

/// The title (`h2`: 14 px at 650).
pub const fn title_style() -> Style {
    Style::ui(theme::TEXT_BASE, theme::WEIGHT_BADGE, theme::TEXT_1)
}

/// `{shown} of {total} shown` (`.shown`: 12 px `--text-3`).
pub const fn shown_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// A group's label (`.label`: 12 px at 600 `--text-2`).
pub const fn label_style() -> Style {
    Style::ui(theme::TEXT_SM, 600, theme::TEXT_2)
}

/// The altitude's value (`.value.data`: Mono 12 px `--text-1`).
pub const fn value_style() -> Style {
    Style::data(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_1)
}

/// A checkbox's word (`.checks`: 13 px).
pub const fn check_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_1)
}

/// `n.toLocaleString()`.
fn count(n: usize) -> String {
    group_thousands(n as f64)
}

/// The header's count: `{shown} of {total} shown`.
pub fn shown_text(shown: usize, total: usize) -> String {
    format!("{} of {} shown", count(shown), count(total))
}

/// The altitude's value on the right of its label (`altitudeText`).
pub fn altitude_text((low, high): (i32, i32)) -> String {
    let ft = |v: i32| format!("{} ft", group_thousands(f64::from(v)));
    if low == 0 && high >= MAX_ALTITUDE {
        "Any altitude".to_owned()
    } else if high >= MAX_ALTITUDE {
        format!("Above {}", ft(low))
    } else if low == 0 {
        format!("Below {}", ft(high))
    } else {
        format!("{} – {}", group_thousands(f64::from(low)), ft(high))
    }
}

// ------------------------------------------------------------------------------------------------
// the suggestions

/// The live counts behind the suggestions (`airlineCounts`, `typeCounts`, `airportCounts`): how
/// many of the loaded flights fly for each airline, are of each type, and leave from or fly to
/// each airport.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tallies {
    airlines: HashMap<String, usize>,
    /// In the order the feed first names them (a `Map`'s order), with an index by code.
    types: Vec<(String, usize)>,
    type_index: HashMap<String, usize>,
    airports: HashMap<String, usize>,
}

impl Tallies {
    /// The counts over `flights` (everything loaded, before filtering).
    pub fn new(flights: &[LiveFlight]) -> Self {
        let mut t = Tallies::default();
        for f in flights {
            if let Some(icao) = airline_icao(&f.callsign) {
                *t.airlines.entry(icao.to_owned()).or_default() += 1;
            }
            if !f.typecode.is_empty() {
                match t.type_index.get(&f.typecode) {
                    Some(&i) => t.types[i].1 += 1,
                    None => {
                        t.type_index.insert(f.typecode.clone(), t.types.len());
                        t.types.push((f.typecode.clone(), 1));
                    }
                }
            }
            if !f.origin.is_empty() {
                *t.airports.entry(f.origin.clone()).or_default() += 1;
            }
        }
        for f in flights {
            if !f.destination.is_empty() {
                *t.airports.entry(f.destination.clone()).or_default() += 1;
            }
        }
        t
    }

    /// Live flights of the airline with ICAO designator `icao`.
    pub fn airline(&self, icao: &str) -> usize {
        self.airlines.get(icao).copied().unwrap_or(0)
    }

    /// Live flights from or to the airport with IATA code `iata`.
    pub fn airport(&self, iata: &str) -> usize {
        self.airports.get(iata).copied().unwrap_or(0)
    }
}

/// A suggestion's `{n} live` (`live`): empty for none.
fn live(n: usize) -> String {
    if n == 0 { String::new() } else { format!("{} live", count(n)) }
}

/// `[parts].filter(Boolean).join(" · ")`.
fn joined(parts: &[&str]) -> String {
    parts.iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join(" · ")
}

/// A suggestion with `detail` unless it is empty (`{#if s.detail}`).
fn suggestion(value: &str, label: String, detail: String) -> Suggestion {
    let s = Suggestion::new(value, label);
    if detail.is_empty() { s } else { s.detail(detail) }
}

/// The airlines for `query` given those `picked` (`suggestAirlines`): airlines with an ICAO
/// designator not picked yet; with a query, those whose designator starts with it, whose IATA code
/// is it or whose name holds it; without one, those with live flights. Exact codes first (an
/// airline without an IATA code counts as exact for an empty query, as `a.iata === query` does
/// today), then by live flights, then by name.
pub fn suggest_airlines(reference: Option<&Reference>, tallies: &Tallies, query: &str, picked: &[String]) -> Vec<Suggestion> {
    let Some(reference) = reference else { return Vec::new() };
    let query = js_trim(query).to_uppercase();
    let mut found: Vec<_> = reference
        .airlines()
        .iter()
        .filter(|a| {
            !a.icao.is_empty()
                && !picked.contains(&a.icao)
                && if query.is_empty() {
                    tallies.airlines.contains_key(&a.icao)
                } else {
                    a.icao.starts_with(&query) || a.iata == query || a.name.to_uppercase().contains(&query)
                }
        })
        .map(|a| (a, tallies.airline(&a.icao), a.icao == query || a.iata == query))
        .collect();
    found.sort_by(|x, y| y.2.cmp(&x.2).then(y.1.cmp(&x.1)).then_with(|| locale_cmp(&x.0.name, &y.0.name)));
    found.into_iter().take(SUGGESTIONS).map(|(a, n, _)| suggestion(&a.icao, a.name.clone(), joined(&[&a.icao, &live(n)]))).collect()
}

/// The aircraft types for `query` (`suggestTypes`): the type codes in the feed that start with it
/// and are not picked, by live flights (ties in the feed's order).
pub fn suggest_types(tallies: &Tallies, query: &str, picked: &[String]) -> Vec<Suggestion> {
    let query = js_trim(query).to_uppercase();
    let mut found: Vec<&(String, usize)> = tallies.types.iter().filter(|(code, _)| code.starts_with(&query) && !picked.contains(code)).collect();
    found.sort_by_key(|t| std::cmp::Reverse(t.1));
    found.into_iter().take(SUGGESTIONS).map(|(code, n)| suggestion(code, code.clone(), live(*n))).collect()
}

/// The airports for `query` (`suggestAirports`): airports with an IATA code not picked yet; with
/// a query, those whose IATA or ICAO code starts with it or whose name or city holds it; without
/// one, those with live flights. Exact codes first (an empty ICAO code counts for an empty query),
/// then by live flights, then by size; labelled `{city} · {name}`.
pub fn suggest_airports(reference: Option<&Reference>, tallies: &Tallies, query: &str, picked: &[String]) -> Vec<Suggestion> {
    let Some(reference) = reference else { return Vec::new() };
    let query = js_trim(query).to_uppercase();
    let mut found: Vec<_> = reference
        .airports()
        .iter()
        .filter(|a| {
            !a.iata.is_empty()
                && !picked.contains(&a.iata)
                && if query.is_empty() {
                    tallies.airports.contains_key(&a.iata)
                } else {
                    a.iata.starts_with(&query) || a.icao.starts_with(&query) || a.name.to_uppercase().contains(&query) || a.city.to_uppercase().contains(&query)
                }
        })
        .map(|a| (a, tallies.airport(&a.iata), a.iata == query || a.icao == query))
        .collect();
    found.sort_by(|x, y| y.2.cmp(&x.2).then(y.1.cmp(&x.1)).then(y.0.size.cmp(&x.0.size)));
    found
        .into_iter()
        .take(SUGGESTIONS)
        .map(|(a, n, _)| {
            let label = if a.city.is_empty() { a.name.clone() } else { format!("{} · {}", a.city, a.name) };
            suggestion(&a.iata, label, joined(&[&a.iata, &live(n)]))
        })
        .collect()
}

/// An airline chip's text (`airlineChip`): `{ICAO} {name}`, or the code alone.
pub fn airline_chip(reference: Option<&Reference>, icao: &str) -> String {
    match reference.and_then(|r| r.airline(Some(icao))) {
        Some(a) if !a.name.is_empty() => format!("{icao} {}", a.name),
        _ => icao.to_owned(),
    }
}

/// An airport chip's text (`airportChip`): `{IATA} {city}`, or the code alone.
pub fn airport_chip(reference: Option<&Reference>, iata: &str) -> String {
    match reference.and_then(|r| r.airport(Some(iata))) {
        Some(a) if !a.city.is_empty() => format!("{iata} {}", a.city),
        _ => iata.to_owned(),
    }
}

/// The tallies for the snapshot as `state` holds it, counted once per change of the snapshot or
/// the filters (`state::aircraft`'s generation) and kept in `ctx` between frames.
fn tallies(ctx: &egui::Context, state: &AppState) -> Arc<Tallies> {
    let id = Id::unique("rustradar::filters::aircraft::tallies");
    let generation = state.aircraft.generation();
    if let Some((g, t)) = ctx.data(|d| d.get_temp::<(u64, Arc<Tallies>)>(id))
        && g == generation
    {
        return t;
    }
    let t = Arc::new(Tallies::new(state.aircraft.flights()));
    ctx.data_mut(|d| d.insert_temp(id, (generation, t.clone())));
    t
}

// ------------------------------------------------------------------------------------------------
// drawing

/// The panel in the sidebar (the shell's [`FilterSlot`](super::super::shell::FilterSlot)): draws
/// within the height `ui` allows and answers its natural height, header and whole body.
pub fn show(ui: &mut Ui, pass: &mut Pass<'_>) -> f32 {
    let state = pass.state;
    let mut filters = state.persisted.filters.clone();
    let available = ui.available_height();
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let header = header(ui, pass, &mut filters);
    let body_max = (available - header).max(0.0);
    let out = ScrollArea::vertical().id_salt("rustradar::filters::aircraft::body").max_height(body_max).auto_shrink([false, true]).show(ui, |ui| {
        Frame::NONE.inner_margin(Margin::same(BODY_PAD as i8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            body(ui, state, &mut filters);
        });
    });
    if filters != state.persisted.filters {
        state.filters_handed.set(Some(filters));
        pass.actions.push(Action::SetFilters);
    }
    header + out.content_size.y
}

/// The header; answers its height.
fn header(ui: &mut Ui, pass: &mut Pass<'_>, filters: &mut Filters) -> f32 {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, HEADER_HEIGHT), Sense::hover());
    let row = Rect::from_min_max(
        Pos2::new(rect.left() + HEADER_PAD_LEFT, rect.top() + HEADER_PAD),
        Pos2::new(rect.right() - HEADER_PAD, rect.top() + HEADER_PAD + icon_button::SIZE),
    );
    let middle = row.center().y;
    // the close button on the right, Reset before it
    let close = Rect::from_min_size(Pos2::new(row.right() - icon_button::SIZE, row.top()), Vec2::splat(icon_button::SIZE));
    // in child uis: the header's own allocation is the layout's
    let closed = IconButton::of(Icon::X, 16.0, CLOSE).tooltip(CLOSE_TOOLTIP).show(&mut ui.new_child(UiBuilder::new().max_rect(close))).clicked();
    let reset_label = "Reset";
    let reset_w = text::measure(ui.ctx(), text::job(reset_label, &Style::ui(theme::TEXT_SM, 600, theme::AMBER))).x + 2.0 * chips::LINK_PAD.x;
    let reset_h = Style::ui(theme::TEXT_SM, 600, theme::AMBER).line_px() + 2.0 * chips::LINK_PAD.y;
    let reset = Rect::from_min_size(Pos2::new(close.left() - HEADER_GAP - reset_w, middle - reset_h / 2.0), Vec2::new(reset_w, reset_h));
    let active = active_count(filters) > 0;
    let reset = chips::text_button(&mut ui.new_child(UiBuilder::new().max_rect(reset)), Id::unique("rustradar::filters::aircraft::reset"), reset_label, active).clicked();

    let painter = ui.painter();
    let title = painter.layout_job(text::job(TITLE, &title_style()));
    let title_w = title.size().x;
    painter.galley(Pos2::new(row.left(), middle - title.size().y / 2.0), title, Color32::PLACEHOLDER);
    let state = pass.state;
    let shown = painter.layout_job(text::tabular(text::job(&shown_text(state.aircraft.filtered_len(), state.aircraft.flights().len()), &shown_style())));
    painter.galley(Pos2::new(row.left() + title_w + HEADER_GAP, middle - shown.size().y / 2.0), shown, Color32::PLACEHOLDER);
    painter.hline(rect.x_range(), rect.bottom() - theme::BORDER / 2.0, Stroke::new(theme::BORDER, theme::LINE));

    if reset {
        *filters = Filters::default();
    }
    if closed {
        pass.actions.push(Action::Open(Open::Filters, false));
    }
    HEADER_HEIGHT
}

/// A group's label (the caller adds the gap under it).
fn label(ui: &mut Ui, label: &str) {
    text::label(ui, text::job(label, &label_style()), Sense::hover());
}

/// The body's groups.
fn body(ui: &mut Ui, state: &AppState, filters: &mut Filters) {
    let ctx = ui.ctx().clone();
    // 1. the text
    label(ui, TEXT_LABEL);
    ui.add_space(LABEL_GAP);
    let field = TextField::new("rustradar::filters::aircraft::text", &mut filters.text).placeholder(TEXT_PLACEHOLDER).show(ui);
    crate::platform::android::text::report(&ctx, field.response.id, &filters.text);
    ui.add_space(GROUP_GAP);

    // 2. the categories
    let links = chips::legend(ui, Id::unique("rustradar::filters::aircraft::categories"), "Categories", &[("All", true), ("None", true)]);
    if links[0].clicked() {
        filters.categories = CATEGORIES.iter().map(|c| c.id).collect();
    }
    if links[1].clicked() {
        filters.categories.clear();
    }
    ui.add_space(LEGEND_GAP);
    let on: Vec<Chip<'_>> = CATEGORIES.iter().map(|c| Chip::new(Kind::Category, c.label, filters.categories.contains(&c.id))).collect();
    let clicked: Vec<u8> = chips::chips(ui, Id::unique("rustradar::filters::aircraft::chips"), &on, true)
        .iter()
        .zip(CATEGORIES.iter())
        .filter(|(r, _)| r.clicked())
        .map(|(_, c)| c.id)
        .collect();
    for id in clicked {
        toggle_sorted(&mut filters.categories, id);
    }
    ui.add_space(GROUP_GAP);

    // 3. the altitude
    altitude_legend(ui, filters.altitude);
    ui.add_space(LEGEND_GAP);
    let (mut low, mut high) = (f64::from(filters.altitude.0), f64::from(filters.altitude.1));
    DualRange::new("rustradar::filters::aircraft::altitude", 0.0, f64::from(MAX_ALTITUDE), f64::from(STEP)).show(ui, &mut low, &mut high);
    filters.altitude = (low as i32, high as i32);
    ui.add_space(LEGEND_GAP);
    // the boxes' row (`.checks`: a flex row 16 apart on the 13 px line)
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), check_style().line_px()), Sense::hover());
    let first = checkbox(ui, Id::unique("rustradar::filters::aircraft::airborne"), row.left_top(), "In the air", &mut filters.airborne);
    let next = Pos2::new(first.rect.right() + CHECKS_GAP, row.top());
    checkbox(ui, Id::unique("rustradar::filters::aircraft::on_ground"), next, "On the ground", &mut filters.on_ground);
    ui.add_space(GROUP_GAP);

    // 4. the token inputs
    let reference = state.aircraft.reference().map(Arc::as_ref);
    let counts = OnceCell::new();
    let tallies = || counts.get_or_init(|| tallies(&ctx, state)).clone();
    let salt = "rustradar::filters::aircraft::airlines";
    TokenInput::new(salt, "Airlines", |q, picked| suggest_airlines(reference, &tallies(), q, picked))
        .free(three_letters)
        .chip(|v| airline_chip(reference, v))
        .placeholder(AIRLINES_PLACEHOLDER)
        .show(ui, &mut filters.airlines);
    report_token(ui, salt);
    ui.add_space(GROUP_GAP);
    let salt = "rustradar::filters::aircraft::types";
    TokenInput::new(salt, "Aircraft types", |q, picked| suggest_types(&tallies(), q, picked))
        .free(type_code)
        .placeholder(TYPES_PLACEHOLDER)
        .hint(TYPES_HINT)
        .show(ui, &mut filters.types);
    report_token(ui, salt);
    ui.add_space(GROUP_GAP);
    let salt = "rustradar::filters::aircraft::airports";
    TokenInput::new(salt, "Airports", |q, picked| suggest_airports(reference, &tallies(), q, picked))
        .free(three_letters)
        .chip(|v| airport_chip(reference, v))
        .placeholder(AIRPORTS_PLACEHOLDER)
        .hint(AIRPORTS_HINT)
        .show(ui, &mut filters.airports);
    report_token(ui, salt);
}

/// The soft keyboard's bridge learns a token input's query (the input made with `salt` in `ui`).
fn report_token(ui: &Ui, salt: &str) {
    let id = ui.make_persistent_id(Id::NULL.with(salt));
    let query = ui.data(|d| d.get_temp::<token_input::State>(id)).map(|s| s.query).unwrap_or_default();
    crate::platform::android::text::report(ui.ctx(), id.with("edit"), &query);
}

/// The altitude's legend: the label, and the range on the right, both on the 12 px line.
fn altitude_legend(ui: &mut Ui, altitude: (i32, i32)) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, label_style().line_px()), Sense::hover());
    let painter = ui.painter();
    let label = painter.layout_job(text::job("Altitude", &label_style()));
    painter.galley(rect.min, label, Color32::PLACEHOLDER);
    let value = painter.layout_job(text::job(&altitude_text(altitude), &value_style()));
    painter.galley(Pos2::new(rect.right() - value.size().x, rect.top()), value, Color32::PLACEHOLDER);
}

/// Chromium's check mark in a box at `rect` (`NativeThemeBase::PaintCheckbox`).
pub fn check_points(rect: Rect) -> [Pos2; 3] {
    let (w, h) = (rect.width(), rect.height());
    let start = Pos2::new(rect.left() + w * 0.2, rect.top() + h * 0.5);
    let corner = start + Vec2::new(w * 0.2, h * 0.2);
    let end = Pos2::new(rect.right() - w * 0.2, rect.top() + h * 0.2);
    [start, corner, end]
}

/// A labelled native checkbox over `value` with its top left at `at` (the box 8 px before its
/// word, centred on the 13 px line); the whole label toggles it. Takes no room in the layout: the
/// caller's row does. Answers the label's response.
pub fn checkbox(ui: &mut Ui, id: Id, at: Pos2, label: &str, value: &mut bool) -> egui::Response {
    let galley = ui.painter().layout_job(text::job(label, &check_style()));
    let size = Vec2::new(CHECKBOX + CHECK_LABEL_GAP + galley.size().x, check_style().line_px());
    let rect = Rect::from_min_size(at, size);
    let response = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    if response.clicked() {
        *value = !*value;
    }
    let painter = ui.painter();
    let check = Rect::from_min_size(Pos2::new(rect.left(), rect.center().y - CHECKBOX / 2.0), Vec2::splat(CHECKBOX));
    if *value {
        painter.rect_filled(check, CornerRadius::same(CHECKBOX_RADIUS), theme::AMBER);
        let stroke = Stroke::new(CHECKBOX * 0.16, CHECKBOX_FILL);
        painter.add(Shape::line(check_points(check).to_vec(), stroke));
    } else {
        painter.rect(check, CornerRadius::same(CHECKBOX_RADIUS), CHECKBOX_FILL, Stroke::new(theme::BORDER, CHECKBOX_BORDER), StrokeKind::Inside);
    }
    painter.galley(Pos2::new(check.right() + CHECK_LABEL_GAP, rect.top()), galley, Color32::PLACEHOLDER);
    if response.has_focus() {
        focus_ring(painter, check, CHECKBOX_RADIUS);
    }
    response
}

#[cfg(test)]
mod tests {
    use egui::{Context, Event, Key, PointerButton, RawInput};
    use rustradar_core::filters::matcher;
    use rustradar_core::model::{Airline, Airport, LiveSnapshot, ReferenceData};
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::persist::tests::TempDir;
    use crate::state::{Startup, aircraft};
    use crate::ui::shell::{self, Slots};

    const T0: f64 = 1_782_036_000_000.0;

    fn airline(icao: &str, iata: &str, name: &str) -> Airline {
        Airline { icao: icao.into(), iata: iata.into(), name: name.into() }
    }

    fn airport(iata: &str, icao: &str, city: &str, name: &str, size: i64) -> Airport {
        Airport { iata: iata.into(), icao: icao.into(), city: city.into(), name: name.into(), size, ..Default::default() }
    }

    fn reference() -> ReferenceData {
        ReferenceData {
            fetched_at: 0,
            airlines: vec![
                airline("SAS", "SK", "SAS Scandinavian Airlines"),
                airline("DLH", "LH", "Lufthansa"),
                airline("BAW", "BA", "British Airways"),
                airline("NOZ", "", "Norwegian Air Sweden"),
                airline("KLM", "KL", "KLM Royal Dutch Airlines"),
                airline("", "XX", "No designator"),
                airline("SAA", "SA", "South African Airways"),
            ],
            airports: vec![
                airport("ARN", "ESSA", "Stockholm", "Arlanda", 90),
                airport("BMA", "ESSB", "Stockholm", "Bromma", 40),
                airport("LHR", "EGLL", "London", "Heathrow", 100),
                airport("CPH", "EKCH", "Copenhagen", "Kastrup", 80),
                airport("", "ESKN", "Stockholm", "Skavsta", 30),
                airport("XYZ", "", "", "Nowhere Field", 1),
            ],
        }
    }

    fn flight(id: u32, callsign: &str, typecode: &str, origin: &str, destination: &str, alt: i32, on_ground: bool) -> LiveFlight {
        LiveFlight {
            id,
            callsign: callsign.into(),
            typecode: typecode.into(),
            origin: origin.into(),
            destination: destination.into(),
            alt,
            on_ground,
            lat: 59.6,
            lon: 17.9,
            ..Default::default()
        }
    }

    fn flights() -> Vec<LiveFlight> {
        vec![
            flight(1, "SAS1415", "A20N", "ARN", "CPH", 30_000, false),
            flight(2, "SAS7", "A333", "CPH", "ARN", 0, true),
            flight(3, "DLH4XK", "A20N", "FRA", "ARN", 12_000, false),
            flight(4, "BAW283", "B77W", "LHR", "", 38_000, false),
            flight(5, "NOZ12", "B738", "ARN", "", 0, true),
            flight(6, "N12345", "C172", "", "", 2_000, false),
            flight(7, "SAS2", "B77L", "", "LHR", 41_000, false),
        ]
    }

    #[test]
    fn the_header_and_the_altitude_say_what_today_s_do() {
        assert_eq!(shown_text(1234, 5678), "1,234 of 5,678 shown");
        assert_eq!(shown_text(0, 0), "0 of 0 shown");
        assert_eq!(altitude_text((0, MAX_ALTITUDE)), "Any altitude");
        assert_eq!(altitude_text((20_000, MAX_ALTITUDE)), "Above 20,000 ft");
        assert_eq!(altitude_text((0, 12_000)), "Below 12,000 ft");
        assert_eq!(altitude_text((10_000, 35_000)), "10,000 – 35,000 ft");
        assert_eq!(altitude_text((0, 1_000)), "Below 1,000 ft");
        assert_eq!(HEADER_HEIGHT, 49.0, "Chromium's layout of the header");
    }

    #[test]
    fn the_live_counts() {
        let t = Tallies::new(&flights());
        assert_eq!(t.airline("SAS"), 3);
        assert_eq!(t.airline("DLH"), 1);
        assert_eq!(t.airline("N12"), 0, "N12345 is no airline callsign");
        // origins, then destinations
        assert_eq!(t.airport("ARN"), 4);
        assert_eq!(t.airport("LHR"), 2);
        assert_eq!(t.airport("FRA"), 1);
        // in the feed's order of first sight
        let types: Vec<(&str, usize)> = t.types.iter().map(|(c, n)| (c.as_str(), *n)).collect();
        assert_eq!(types, [("A20N", 2), ("A333", 1), ("B77W", 1), ("B738", 1), ("C172", 1), ("B77L", 1)]);
    }

    #[test]
    fn airline_suggestions() {
        let r = Reference::new(reference());
        let t = Tallies::new(&flights());
        let values = |s: Vec<Suggestion>| s.into_iter().map(|s| (s.value, s.label, s.detail)).collect::<Vec<_>>();
        // no query: those with live flights; an airline with no IATA code counts as exact for the
        // empty query (`a.iata === query`), then by live flights, then by name
        let none = values(suggest_airlines(Some(&r), &t, "", &[]));
        assert_eq!(
            none,
            [
                ("NOZ".into(), "Norwegian Air Sweden".into(), Some("NOZ · 1 live".into())),
                ("SAS".into(), "SAS Scandinavian Airlines".into(), Some("SAS · 3 live".into())),
                ("BAW".into(), "British Airways".into(), Some("BAW · 1 live".into())),
                ("DLH".into(), "Lufthansa".into(), Some("DLH · 1 live".into())),
            ]
        );
        // a query: designator prefix, IATA code, or a name holding it; exact first
        let sa = values(suggest_airlines(Some(&r), &t, " sa ", &[]));
        let codes: Vec<&str> = sa.iter().map(|(v, _, _)| v.as_str()).collect();
        assert_eq!(codes, ["SAA", "SAS", "DLH"], "SA is South African's IATA code, LUFTHANSA holds it");
        assert_eq!(sa[0].2.as_deref(), Some("SAA"), "no live flights: the code alone");
        let lines = values(suggest_airlines(Some(&r), &t, "airlines", &[]));
        assert_eq!(lines.iter().map(|(v, _, _)| v.as_str()).collect::<Vec<_>>(), ["SAS", "KLM"]);
        // picked ones are not suggested, nor airlines without a designator
        assert!(suggest_airlines(Some(&r), &t, "", &["SAS".into()]).iter().all(|s| s.value != "SAS"));
        assert!(suggest_airlines(Some(&r), &t, "XX", &[]).is_empty());
        // without the reference data there is nothing to suggest
        assert!(suggest_airlines(None, &t, "", &[]).is_empty());
    }

    #[test]
    fn type_suggestions() {
        let t = Tallies::new(&flights());
        let codes = |q: &str, picked: &[String]| suggest_types(&t, q, picked).into_iter().map(|s| (s.value, s.detail)).collect::<Vec<_>>();
        assert_eq!(
            codes("", &[]),
            [
                ("A20N".into(), Some("2 live".into())),
                ("A333".into(), Some("1 live".into())),
                ("B77W".into(), Some("1 live".into())),
                ("B738".into(), Some("1 live".into())),
                ("C172".into(), Some("1 live".into())),
                ("B77L".into(), Some("1 live".into())),
            ]
        );
        assert_eq!(codes("b7", &["B738".into()]).iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(), ["B77W", "B77L"]);
        assert!(codes("Z", &[]).is_empty());
        let many: Vec<LiveFlight> = (0..10).map(|i| flight(i, "X", &format!("T{i:03}"), "", "", 0, false)).collect();
        assert_eq!(suggest_types(&Tallies::new(&many), "", &[]).len(), SUGGESTIONS);
    }

    #[test]
    fn airport_suggestions() {
        let r = Reference::new(reference());
        let t = Tallies::new(&flights());
        let found = |q: &str| suggest_airports(Some(&r), &t, q, &[]).into_iter().map(|s| (s.value, s.label, s.detail)).collect::<Vec<_>>();
        // no query: with live flights, by count; an airport without ICAO would be exact (none live)
        assert_eq!(
            found(""),
            [
                ("ARN".into(), "Stockholm · Arlanda".into(), Some("ARN · 4 live".into())),
                ("LHR".into(), "London · Heathrow".into(), Some("LHR · 2 live".into())),
                ("CPH".into(), "Copenhagen · Kastrup".into(), Some("CPH · 2 live".into())),
            ]
        );
        // the city: by live flights, then size; an airport without IATA code is never suggested
        assert_eq!(found("stockholm").iter().map(|(v, _, _)| v.as_str()).collect::<Vec<_>>(), ["ARN", "BMA"]);
        // an ICAO code is exact
        assert_eq!(found("essb")[0].0, "BMA");
        assert_eq!(found("ess").iter().map(|(v, _, _)| v.as_str()).collect::<Vec<_>>(), ["ARN", "BMA"]);
        // no city: the name alone
        assert_eq!(found("xyz"), [("XYZ".into(), "Nowhere Field".into(), Some("XYZ".into()))]);
    }

    #[test]
    fn chips_name_their_values() {
        let r = Reference::new(reference());
        assert_eq!(airline_chip(Some(&r), "SAS"), "SAS SAS Scandinavian Airlines");
        assert_eq!(airline_chip(Some(&r), "QQQ"), "QQQ");
        assert_eq!(airline_chip(None, "SAS"), "SAS");
        assert_eq!(airport_chip(Some(&r), "ARN"), "ARN Stockholm");
        assert_eq!(airport_chip(Some(&r), "arn"), "arn Stockholm", "found in any case, written as chosen");
        assert_eq!(airport_chip(Some(&r), "XYZ"), "XYZ", "no city");
    }

    #[test]
    fn chromium_s_check_mark() {
        let box_ = Rect::from_min_size(Pos2::new(377.0, 431.0), Vec2::splat(CHECKBOX));
        let [a, b, c] = check_points(box_);
        let near = |p: Pos2, x: f32, y: f32| (p.x - x).abs() < 1e-4 && (p.y - y).abs() < 1e-4;
        assert!(near(a, 379.6, 437.5) && near(b, 382.2, 440.1) && near(c, 387.4, 433.6), "{a:?} {b:?} {c:?}");
    }

    // --------------------------------------------------------------------------------------------
    // through the panel and the actions

    /// A state on the fixture folder `dir` (a snapshot of [`flights`] and the reference data), its
    /// feed and reference loaded, the filters open.
    fn state(dir: &TempDir) -> AppState {
        let snapshot = LiveSnapshot { flights: flights(), server_time_ms: T0 as u64, ..Default::default() };
        std::fs::write(dir.0.join(aircraft::FIXTURE_LIVE), serde_json::to_vec(&snapshot).unwrap()).unwrap();
        std::fs::write(dir.0.join(aircraft::FIXTURE_REFERENCE), serde_json::to_vec(&reference()).unwrap()).unwrap();
        let startup = Startup { fixture: Some(dir.0.clone()), ..Default::default() };
        let mut s = AppState::new(startup, None, None, None, ViewState::INITIAL, [1440.0, 900.0]);
        for t in [T0, T0 + 16.0, T0 + 32.0] {
            s.poll(t);
        }
        assert_eq!(s.aircraft.flights().len(), 7);
        assert!(s.aircraft.reference().is_some());
        assert!(s.apply(Action::Open(Open::Filters, true), T0 + 40.0));
        s
    }

    /// One pass of the shell with the app's slots over `events`: the actions.
    fn pass(ctx: &Context, state: &AppState, time: f64, events: Vec<Event>) -> Vec<Action> {
        let mut actions = Vec::new();
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0))),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| {
            shell::show(ui.ctx(), state, T0, &mut actions, &Slots::APP);
        });
        output.textures_delta.clear();
        actions
    }

    /// A click at `at` over three passes from egui time `time`.
    fn click(ctx: &Context, state: &AppState, at: Pos2, time: f64) -> Vec<Action> {
        let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
        let mut actions = pass(ctx, state, time, vec![Event::PointerMoved(at)]);
        actions.extend(pass(ctx, state, time + 0.05, vec![button(true)]));
        actions.extend(pass(ctx, state, time + 0.1, vec![button(false)]));
        actions
    }

    /// Applies `actions` and polls: the filters reach the aircraft feed's list.
    fn apply(s: &mut AppState, actions: Vec<Action>, t: f64) {
        for a in actions {
            s.apply(a, t);
        }
        s.poll(t + 1.0);
    }

    fn context() -> Context {
        let ctx = Context::default();
        crate::ui::theme::apply(&ctx);
        ctx
    }

    fn rect_of(ctx: &Context, id: Id) -> Rect {
        ctx.read_response(id).unwrap_or_else(|| panic!("{id:?} drawn")).rect
    }

    /// How many of the snapshot's flights `filters` keep, by `core::filters::matcher`.
    fn kept(filters: &Filters) -> usize {
        let keep = matcher(filters);
        flights().iter().filter(|f| keep(f)).count()
    }

    #[test]
    fn a_checkbox_and_a_chip_reach_the_filtered_list() {
        let dir = TempDir::new("filters-check");
        let mut s = state(&dir);
        assert_eq!(s.aircraft.filtered_len(), 7);
        let ctx = context();
        for time in [0.0, 1.0, 2.0] {
            assert!(pass(&ctx, &s, time, Vec::new()).is_empty(), "drawing changes nothing");
        }
        // On the ground off: the two on the ground leave the list
        let ground = rect_of(&ctx, Id::unique("rustradar::filters::aircraft::on_ground"));
        let actions = click(&ctx, &s, ground.center(), 3.0);
        assert_eq!(actions, [Action::SetFilters]);
        apply(&mut s, actions, T0 + 100.0);
        assert!(!s.persisted.filters.on_ground);
        assert_eq!(s.aircraft.filtered_len(), kept(&s.persisted.filters));
        assert_eq!(s.aircraft.filtered_len(), 5);
        // a category chip: Passenger off (the server's filter: a new snapshot is asked for)
        for time in [4.0, 5.0] {
            pass(&ctx, &s, time, Vec::new());
        }
        let passenger = rect_of(&ctx, Id::unique("rustradar::filters::aircraft::chips").with(0usize));
        let actions = click(&ctx, &s, passenger.center(), 6.0);
        assert_eq!(actions, [Action::SetFilters]);
        apply(&mut s, actions, T0 + 200.0);
        assert_eq!(s.persisted.filters.categories, (1..12).collect::<Vec<u8>>());
        assert_eq!(rustradar_core::filters::active_count(&s.persisted.filters), 2);
        // and back on: kept ascending
        for time in [7.0, 8.0] {
            pass(&ctx, &s, time, Vec::new());
        }
        let actions = click(&ctx, &s, passenger.center(), 9.0);
        apply(&mut s, actions, T0 + 300.0);
        assert_eq!(s.persisted.filters.categories, (0..12).collect::<Vec<u8>>());
        // Reset restores the defaults
        for time in [10.0, 11.0] {
            pass(&ctx, &s, time, Vec::new());
        }
        let reset = rect_of(&ctx, Id::unique("rustradar::filters::aircraft::reset"));
        let actions = click(&ctx, &s, reset.center(), 12.0);
        assert_eq!(actions, [Action::SetFilters]);
        apply(&mut s, actions, T0 + 400.0);
        assert_eq!(s.persisted.filters, Filters::default());
        assert_eq!(s.aircraft.filtered_len(), 7);
        // with nothing to reset, Reset does nothing
        for time in [13.0, 14.0] {
            pass(&ctx, &s, time, Vec::new());
        }
        assert!(click(&ctx, &s, reset.center(), 15.0).is_empty());
        // a SetFilters with nothing handed over, or the same filters, changes nothing
        assert!(!s.apply(Action::SetFilters, T0 + 500.0));
        s.filters_handed.set(Some(Filters::default()));
        assert!(!s.apply(Action::SetFilters, T0 + 500.0));
    }

    #[test]
    fn typing_in_the_fields_reaches_the_filtered_list() {
        let dir = TempDir::new("filters-text");
        let mut s = state(&dir);
        let ctx = context();
        for time in [0.0, 1.0, 2.0] {
            pass(&ctx, &s, time, Vec::new());
        }
        let card = ctx.memory(|m| m.area_rect(Id::unique(("rustradar::sidebar", "filters")))).expect("the filter panel");
        // the text field: under the header, the body's padding and the label
        let field_top = card.top() + theme::BORDER + HEADER_HEIGHT + BODY_PAD + label_style().line_px() + LABEL_GAP;
        let at = Pos2::new(card.left() + 60.0, field_top + 18.0);
        assert!(click(&ctx, &s, at, 3.0).is_empty(), "focusing changes nothing");
        assert!(ctx.text_edit_focused());
        let actions = pass(&ctx, &s, 3.5, vec![Event::Text("sas".into())]);
        assert_eq!(actions, [Action::SetFilters]);
        apply(&mut s, actions, T0 + 100.0);
        assert_eq!(s.persisted.filters.text, "sas");
        assert_eq!(s.aircraft.filtered_len(), 3, "callsigns holding SAS");
        assert_eq!(s.aircraft.filtered_len(), kept(&s.persisted.filters));

        // the airlines: 16 under the checkboxes, its label, then the field
        for time in [4.0, 5.0] {
            pass(&ctx, &s, time, Vec::new());
        }
        let checks = rect_of(&ctx, Id::unique("rustradar::filters::aircraft::on_ground"));
        let at = Pos2::new(card.left() + 120.0, checks.bottom() + GROUP_GAP + label_style().line_px() + LABEL_GAP + 21.0);
        assert!(click(&ctx, &s, at, 6.0).is_empty());
        assert!(ctx.text_edit_focused());
        assert!(pass(&ctx, &s, 6.5, vec![Event::Text("dl".into())]).is_empty(), "the query is the input's own");
        let enter = Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        let actions = pass(&ctx, &s, 7.0, vec![enter]);
        assert_eq!(actions, [Action::SetFilters]);
        apply(&mut s, actions, T0 + 200.0);
        assert_eq!(s.persisted.filters.airlines, ["DLH"], "the first suggestion");
        // the text still holds SAS: no flight is both
        assert_eq!(s.aircraft.filtered_len(), 0);
        assert_eq!(s.aircraft.filtered_len(), kept(&s.persisted.filters));
    }

    #[test]
    fn the_close_button_closes_the_panel() {
        let dir = TempDir::new("filters-close");
        let mut s = state(&dir);
        let ctx = context();
        for time in [0.0, 1.0, 2.0] {
            pass(&ctx, &s, time, Vec::new());
        }
        let card = ctx.memory(|m| m.area_rect(Id::unique(("rustradar::sidebar", "filters")))).expect("the filter panel");
        // 8 in from the right, 8 down, 32 square
        let at = Pos2::new(card.right() - theme::BORDER - HEADER_PAD - 16.0, card.top() + theme::BORDER + HEADER_PAD + 16.0);
        let actions = click(&ctx, &s, at, 3.0);
        assert_eq!(actions, [Action::Open(Open::Filters, false)]);
        apply(&mut s, actions, T0 + 100.0);
        assert!(!s.shortcuts.filters_open());
    }

    /// The panel's natural height is the header and the whole body, whatever it was given.
    #[test]
    fn the_natural_height_is_today_s() {
        let dir = TempDir::new("filters-height");
        let s = state(&dir);
        let ctx = context();
        let mut natural = 0.0;
        for time in [0.0, 1.0, 2.0] {
            let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0))), time: Some(time), ..Default::default() };
            let mut actions = Vec::new();
            let mut output = ctx.run_ui(raw, |ui| {
                egui::Area::new(Id::unique("test")).fixed_pos(Pos2::new(16.0, 16.0)).show(ui.ctx(), |ui| {
                    ui.set_width(318.0);
                    ui.set_max_height(300.0);
                    let mut p = Pass { state: &s, now_ms: T0, actions: &mut actions };
                    natural = show(ui, &mut p);
                });
            });
            output.textures_delta.clear();
        }
        // Chromium: 49 + 671.75 for the default filters (the body's scroll bar narrows the
        // chips' row here, and they still wrap as Chromium's do)
        assert!((natural - 720.75).abs() < 1.0, "{natural}");
    }
}
