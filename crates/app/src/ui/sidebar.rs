//! The desktop sidebar (doc 06 §2.3, §2.8, §2.9; `+page.svelte:1432-1610`, `1767-1897`): a column
//! 320 px wide at left 16, top 16, down to 40 px above the bottom, its cards 12 apart, the map
//! taking the pointer in the gaps (`pointer-events: none` on the column, each card its own egui
//! [`Area`]):
//!
//! 1. **The primary card** (`.primary`, `flex: none`, padding 16, its parts 12 apart):
//!    - the brand (`h1`: the Radar icon at 20 px in `--amber`, `RustRadar` at 17 px weight 750,
//!      letter spacing −0.01 em, 8 apart) and the feed's status on the right (12 px `--text-3`):
//!      for the aircraft the snapshot's age, a warn badge at 12 px once it is stale (its tooltip
//!      the time of the last update), `Updating…` while a request is out (its tooltip the
//!      snapshot's requests and milliseconds), a 90 × 12 skeleton before the first snapshot
//!      unless the feed failed; for trains and ships their list's age the same way, the skeleton
//!      until the first answer;
//!    - the search box (item U8's slot, [`super::shell::Slots::search`]);
//!    - the toolbar: the Filters button (the Funnel icon, `Filters`, the active-filter pill, pressed
//!      while the panel is open, tooltip `Filters (F)`) and the counts on the right (`1,234 of
//!      5,678 in view`: the number 13 px weight 650 `--text-1`, the rest 12 px `--text-2`;
//!      `worldwide` for a world snapshot);
//!    - one alert for the aircraft: Flightradar24 limiting requests (warn), the feed failing
//!      (error, its message and "Retry now", disabled while a request is out), or a note when
//!      some regions did not load.
//! 2. **The filter panel** while open (`flex-shrink: 1`, `min-height: 0`): the focus's panel, items
//!    U9, U14 and U15's slots, which scroll their own body.
//! 3. **The list card** (`flex: 0 3 auto`, `min-height: 96px`, padding 12 × 8, scrolling): the
//!    focus's list, items U9, U14 and U15's slots.
//!
//! **Heights** ([`column()`]): nothing grows; when the cards do not fit, CSS's flexible lengths
//! shrink the filter panel and the list in proportion to `flex-shrink × flex base size` (the list
//! three times as fast as the panel for the same height), the list no lower than 96 px of content,
//! the primary card not at all. Each card's natural height is measured as it is drawn and the
//! heights are solved from the last frame's measures (an egui area learns its size by drawing).
//!
//! The status's and counts' `tabular-nums` are Overpass's tabular figures (`ui::text::tabular`),
//! the stale badge's tooltip reads the viewer's time zone (`AppState::viewer_clocks`) and text
//! stands on Chromium's baselines (`ui::text`'s hook; item F1).
//!
//! Deviations:
//! - A card whose slot no item fills yet (the search box, a filter panel, a list) is not drawn.

use egui::{Align2, Area, Context, Id, Margin, Order, Pos2, Rect, ScrollArea, Sense, Ui, Vec2};
use rustradar_core::filters::active_count;
use rustradar_core::freshness::Freshness;
use rustradar_core::settings::Mode;
use rustradar_core::ship_list::{ship_filter, ship_filter_count, ship_matches};
use rustradar_core::text::group_thousands;
use rustradar_core::train_list::{train_filter, train_filter_count, train_matches};
use rustradar_core::trains::train_running;

use super::icons::{self, Icon};
use super::shell::{Pass, Slots};
use super::text::{self, Style};
use super::theme;
use super::widgets::alert::Alert;
use super::widgets::badge::{Badge, Tone};
use super::widgets::button::{self, Button};
use super::widgets::skeleton;
use super::widgets::tooltip;
use crate::state::shortcuts::Open;
use crate::state::{Action, AppState};

/// The column's left edge, CSS px (`left: var(--space-4)`).
pub const LEFT: f32 = theme::SPACE_4;
/// Its top (`top: var(--space-4)`).
pub const TOP: f32 = theme::SPACE_4;
/// How far above the window's bottom it ends (`bottom: 40px`: clear of the attribution).
pub const BOTTOM: f32 = 40.0;
/// Its width (`width: 320px`); the cards fill it, borders included.
pub const WIDTH: f32 = 320.0;
/// Between the cards (`gap: var(--space-3)`), and between the primary card's parts.
pub const GAP: f32 = theme::SPACE_3;
/// The primary card's padding (`padding: var(--space-4)`).
pub const PADDING: f32 = theme::SPACE_4;
/// The list card's padding, across and down (`padding: var(--space-3) var(--space-2)`).
pub const LIST_PADDING: Vec2 = Vec2::new(theme::SPACE_2, theme::SPACE_3);
/// The list's least height, content box (`min-height: 96px`).
pub const LIST_MIN_HEIGHT: f32 = 96.0;
/// The list's `flex-shrink`: with the filters open, the list gives way first.
pub const LIST_SHRINK: f32 = 3.0;
/// The filter panel's `flex-shrink`.
pub const FILTERS_SHRINK: f32 = 1.0;
/// The brand's icon, px (`<Radar size={20} strokeWidth={1.75} />`).
pub const BRAND_ICON: f32 = 20.0;
/// The status's skeleton before the first answer (`width: 90px; height: 12px`).
pub const STATUS_SKELETON: Vec2 = Vec2::new(90.0, 12.0);

/// The wordmark's text (`h1`: 17 px, weight 750, −0.01 em).
pub const fn wordmark_style() -> Style {
    Style::ui(theme::TEXT_LG, theme::WEIGHT_WORDMARK, theme::TEXT_1).letter_spacing(theme::LETTER_SPACING_TIGHT)
}

/// The status's text (`.status`: 12 px `--text-3`).
pub const fn status_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// The counts' text (`.counts`: 12 px `--text-2`).
pub const fn counts_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// The counts' number (`.count`: 13 px weight 650 `--text-1`).
pub const fn count_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_BADGE, theme::TEXT_1)
}

/// The note under the toolbar (`.note`: 12 px `--text-2`).
pub const fn note_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

// ------------------------------------------------------------------------------------------------
// the height solver

/// A card of the column, as CSS's flexible lengths see it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Item {
    /// The content box's height laid out without limit (`flex-basis: auto`).
    pub base: f32,
    /// Padding and borders, top and bottom (the cards are `box-sizing: content-box`).
    pub edges: f32,
    /// `flex-shrink` (0: `flex: none`).
    pub shrink: f32,
    /// `min-height`, content box.
    pub min: f32,
}

impl Item {
    /// A card that keeps its natural height (`flex: none`).
    pub fn fixed(outer: f32) -> Item {
        Item { base: outer, edges: 0.0, shrink: 0.0, min: 0.0 }
    }
}

/// The outer heights of `items` stacked with `gap` between them in a column `available` high
/// (CSS Flexbox §9.7, "Resolving Flexible Lengths", as Chromium runs it for a column whose items
/// do not grow): at their natural heights when they fit; else each unfrozen item gives up the
/// overflow in proportion to `flex-shrink × flex base size`, an item held at its `min-height` is
/// frozen there and the rest shared again. The column may still overflow when what cannot shrink
/// is taller than it.
pub fn column(items: &[Item], available: f32, gap: f32) -> Vec<f32> {
    let inner = available - gap * items.len().saturating_sub(1) as f32;
    let hypothetical: Vec<f32> = items.iter().map(|i| i.base.max(i.min)).collect();
    let natural: f32 = items.iter().zip(&hypothetical).map(|(i, h)| h + i.edges).sum();
    if natural <= inner {
        return items.iter().zip(&hypothetical).map(|(i, h)| h + i.edges).collect();
    }
    // inflexible: no shrink, or held up by the minimum already
    let mut target = hypothetical.clone();
    let mut frozen: Vec<bool> = items.iter().zip(&hypothetical).map(|(i, &h)| i.shrink == 0.0 || i.base < h).collect();
    let used = |target: &[f32], frozen: &[bool]| -> f32 {
        items.iter().enumerate().map(|(n, i)| i.edges + if frozen[n] { target[n] } else { i.base }).sum()
    };
    let initial = inner - used(&target, &frozen);
    while frozen.iter().any(|f| !f) {
        let mut remaining = inner - used(&target, &frozen);
        let factors: f32 = items.iter().zip(&frozen).filter(|(_, f)| !**f).map(|(i, _)| i.shrink).sum();
        if factors < 1.0 && (initial * factors).abs() < remaining.abs() {
            remaining = initial * factors;
        }
        let scaled: f32 = items.iter().zip(&frozen).filter(|(_, f)| !**f).map(|(i, _)| i.shrink * i.base).sum();
        let mut violation = 0.0;
        let mut adjusted = vec![0.0; items.len()];
        for (n, item) in items.iter().enumerate() {
            if frozen[n] {
                continue;
            }
            let share = if scaled > 0.0 { item.shrink * item.base / scaled } else { 0.0 };
            let unclamped = item.base + remaining * share;
            let clamped = unclamped.max(item.min).max(0.0);
            adjusted[n] = clamped - unclamped;
            violation += adjusted[n];
            target[n] = clamped;
        }
        for n in 0..items.len() {
            if frozen[n] {
                continue;
            }
            // all frozen when nothing was clamped; else the ones clamped up to their minimum
            frozen[n] = violation == 0.0 || (violation > 0.0 && adjusted[n] > 0.0) || (violation < 0.0 && adjusted[n] < 0.0);
        }
    }
    items.iter().zip(&target).map(|(i, t)| t + i.edges).collect()
}

// ------------------------------------------------------------------------------------------------
// what the cards say

/// The feed's status at the brand's right (`p.status`).
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// A plain label (`Updated 5 s ago`, `Updating…`), with its tooltip if any.
    Text { label: String, tooltip: Option<String> },
    /// Stale: the label in a warn badge at 12 px.
    Stale { label: String, tooltip: Option<String> },
    /// Nothing has come yet: the 90 × 12 skeleton.
    Waiting,
    /// Nothing (the aircraft feed failed before its first snapshot).
    None,
}

/// What the status says at local time `now_ms` (`+page.svelte:1435-1462`).
pub fn status(state: &AppState, now_ms: f64) -> Status {
    let freshness = |f: Option<Freshness>| f.map(|f| (f.stale, f.label));
    match state.persisted.settings.mode {
        Mode::Aircraft => {
            let aircraft = &state.aircraft;
            let fresh = freshness(aircraft.freshness(now_ms));
            match (aircraft.snapshot(), fresh) {
                (Some(_), Some((true, label))) => {
                    let at = aircraft.shown_feed().map_or(0, |f| f.at);
                    let clock = state.viewer_clocks.clock_at((at as f64 / 1000.0).floor(), None);
                    let tooltip = format!("Last update at {clock}{}", if aircraft.loading() { "; updating now" } else { "" });
                    Status::Stale { label, tooltip: Some(tooltip) }
                }
                (Some(snapshot), fresh) => {
                    let label = if aircraft.loading() { "Updating…".to_string() } else { fresh.map(|(_, l)| l).unwrap_or_default() };
                    Status::Text { label, tooltip: Some(format!("{} requests in {} ms", snapshot.requests, snapshot.elapsed_ms)) }
                }
                (None, _) if aircraft.feed_error().is_none() => Status::Waiting,
                (None, _) => Status::None,
            }
        }
        mode => {
            let list = match mode {
                Mode::Trains => freshness(state.trains.freshness(now_ms)),
                _ => freshness(state.ships.freshness(now_ms)),
            };
            match list {
                Some((true, label)) => Status::Stale { label, tooltip: None },
                Some((false, label)) => Status::Text { label, tooltip: None },
                None => Status::Waiting,
            }
        }
    }
}

/// The toolbar's counts (`p.counts`): how many are listed, of how many, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counts {
    pub listed: usize,
    pub total: usize,
    /// `in view`, or `worldwide` for a snapshot of the whole world.
    pub where_: &'static str,
}

impl Counts {
    /// The words after the number, as the page's white space collapses them (the noun is hidden
    /// on desktop): ` of 5,678 in view`, or ` in view` when nothing is filtered out.
    pub fn rest(&self) -> String {
        if self.listed != self.total {
            format!(" of {} {}", group_thousands(self.total as f64), self.where_)
        } else {
            format!(" {}", self.where_)
        }
    }
}

/// The counts at local time `now_ms`, with `query` the focus's search (the trains' and ships'
/// lists match it; item U8's): `None` until the focus's first answer.
pub fn counts(state: &AppState, now_ms: f64, query: &str) -> Option<Counts> {
    let q = super::widgets::token_input::js_trim(query);
    match state.persisted.settings.mode {
        Mode::Aircraft => {
            let aircraft = &state.aircraft;
            aircraft.snapshot()?;
            let where_ = if aircraft.regional() { "in view" } else { "worldwide" };
            Some(Counts { listed: aircraft.filtered_len(), total: aircraft.flights().len(), where_ })
        }
        Mode::Trains => {
            let trains = &state.trains;
            if !trains.loaded() {
                return None;
            }
            let keep = train_filter(&state.persisted.train_filters);
            let now = now_ms as i64;
            let listed = trains.trains().iter().filter(|t| keep(t) && train_running(t, now) && (q.is_empty() || train_matches(t, q))).count();
            Some(Counts { listed, total: trains.trains().len(), where_: "in view" })
        }
        Mode::Ships => {
            let ships = &state.ships;
            if !ships.loaded() {
                return None;
            }
            let keep = ship_filter(&state.persisted.ship_filters);
            let listed = ships.ships().iter().filter(|s| keep(s) && (q.is_empty() || ship_matches(s, q))).count();
            Some(Counts { listed, total: ships.ships().len(), where_: "in view" })
        }
    }
}

/// How many filter groups are active for the focus (the Filters button's pill).
pub fn active_filters(state: &AppState) -> usize {
    let p = &state.persisted;
    match p.settings.mode {
        Mode::Aircraft => active_count(&p.filters),
        Mode::Trains => train_filter_count(&p.train_filters),
        Mode::Ships => ship_filter_count(&p.ship_filters),
    }
}

/// The aircraft feed's alert under the toolbar (`+page.svelte:1485-1517`).
#[derive(Debug, Clone, PartialEq)]
pub enum Notice {
    /// Flightradar24 is limiting requests (warn).
    Limited { retry_in: i64 },
    /// The feed failed (error), with its message; Retry is off while a request is out.
    Failed { retry_in: i64, error: String, loading: bool },
    /// Some regions did not load this round.
    Partial(usize),
}

impl Notice {
    /// The alert's or the note's text.
    pub fn text(&self) -> String {
        match self {
            Notice::Limited { retry_in } => format!("Flightradar24 is limiting requests. Slowing down; next update in {retry_in} s."),
            Notice::Failed { retry_in, .. } => format!("Can't reach Flightradar24. Trying again in {retry_in} s."),
            Notice::Partial(n) => format!("{n} region{} didn't load this round; some aircraft may be missing.", if *n == 1 { "" } else { "s" }),
        }
    }
}

/// The alert or note at local time `now_ms`: only while aircraft are the focus (the trains' and
/// ships' failures are told in their lists).
pub fn notice(state: &AppState, now_ms: f64) -> Option<Notice> {
    if state.persisted.settings.mode != Mode::Aircraft {
        return None;
    }
    let aircraft = &state.aircraft;
    let retry_in = aircraft.retry_in(now_ms);
    if aircraft.rate_limited() {
        Some(Notice::Limited { retry_in })
    } else if let Some(error) = aircraft.feed_error() {
        Some(Notice::Failed { retry_in, error: error.to_string(), loading: aircraft.loading() })
    } else {
        let partial = aircraft.partial();
        (partial > 0).then_some(Notice::Partial(partial))
    }
}

// ------------------------------------------------------------------------------------------------
// drawing

/// Where the sidebar's cards were drawn this frame (screen points).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Placed {
    pub primary: Option<Rect>,
    pub filters: Option<Rect>,
    pub list: Option<Rect>,
}

/// The natural heights measured last frame, by card.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Measured {
    filters: f32,
    list: f32,
}

fn measured_id() -> Id {
    Id::unique("rustradar::sidebar::measured")
}

/// A card's area: fixed at `at`, no fade on its first frame, kept where it is put.
fn card_area(id: &str, at: Pos2) -> Area {
    Area::new(Id::unique(("rustradar::sidebar", id))).order(Order::Middle).fixed_pos(at).fade_in(false).constrain(false)
}

/// Draws the sidebar onto `ctx` for `pass` with the components in `slots`.
pub fn show(ctx: &Context, pass: &mut Pass<'_>, slots: &Slots) -> Placed {
    let screen = ctx.content_rect();
    let mut placed = Placed::default();
    let primary = card_area("primary", Pos2::new(screen.left() + LEFT, screen.top() + TOP))
        .show(ctx, |ui| {
            theme::surface().inner_margin(Margin::same(PADDING as i8)).show(ui, |ui| {
                let width = WIDTH - 2.0 * (PADDING + theme::BORDER);
                ui.set_width(width);
                primary(ui, pass, slots);
            });
        })
        .response
        .rect;
    placed.primary = Some(primary);

    let mode = pass.state.persisted.settings.mode;
    let filters = if pass.state.shortcuts.filters_open() { slots.filters(mode) } else { None };
    let list = slots.list(mode);
    let measured: Measured = ctx.data(|d| d.get_temp(measured_id())).unwrap_or_default();
    let mut items = vec![Item::fixed(primary.height())];
    if filters.is_some() {
        items.push(Item { base: measured.filters, edges: 2.0 * theme::BORDER, shrink: FILTERS_SHRINK, min: 0.0 });
    }
    if list.is_some() {
        items.push(Item { base: measured.list, edges: 2.0 * (LIST_PADDING.y + theme::BORDER), shrink: LIST_SHRINK, min: LIST_MIN_HEIGHT });
    }
    let available = screen.height() - TOP - BOTTOM;
    let heights = column(&items, available, GAP);
    let mut next = Measured::default();
    let mut y = primary.bottom() + GAP;
    let mut heights = heights.into_iter().skip(1);

    if let Some(slot) = filters {
        let outer = heights.next().unwrap_or_default();
        let at = Pos2::new(screen.left() + LEFT, y);
        let inner = (outer - 2.0 * theme::BORDER).max(0.0);
        let shown = card_area("filters", at).show(ctx, |ui| {
            theme::surface().show(ui, |ui| {
                ui.set_width(WIDTH - 2.0 * theme::BORDER);
                let top = ui.cursor().min;
                ui.set_max_height(inner);
                next.filters = slot(ui, pass);
                // the card is the height it is given, whatever the panel took of it
                ui.expand_to_include_rect(Rect::from_min_size(top, Vec2::new(0.0, inner)));
            });
        });
        placed.filters = Some(shown.response.rect);
        y += outer + GAP;
    }

    if let Some(slot) = list {
        let outer = heights.next().unwrap_or_default();
        let at = Pos2::new(screen.left() + LEFT, y);
        let inner = (outer - 2.0 * (LIST_PADDING.y + theme::BORDER)).max(0.0);
        let shown = card_area("list", at).show(ctx, |ui| {
            let margin = Margin::symmetric(LIST_PADDING.x as i8, LIST_PADDING.y as i8);
            theme::surface().inner_margin(margin).show(ui, |ui| {
                let width = WIDTH - 2.0 * (LIST_PADDING.x + theme::BORDER);
                ui.set_width(width);
                // an area lays its content out in last frame's rectangle: this frame's height
                ui.set_max_height(inner);
                let out = ScrollArea::vertical().id_salt("rustradar::sidebar::list").max_height(inner).auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    slot(ui, pass);
                });
                next.list = out.content_size.y;
            });
        });
        placed.list = Some(shown.response.rect);
    }
    ctx.data_mut(|d| d.insert_temp(measured_id(), next));
    placed
}

/// The primary card's parts, 12 apart.
fn primary(ui: &mut Ui, pass: &mut Pass<'_>, slots: &Slots) {
    ui.spacing_mut().item_spacing = Vec2::new(GAP, GAP);
    brand(ui, pass);
    if let Some(search) = slots.search {
        search(ui, pass);
    }
    toolbar(ui, pass);
    if let Some(notice) = notice(pass.state, pass.now_ms) {
        alert(ui, pass, notice);
    }
}

/// The brand row: the icon and the wordmark, the status on the right, centred on the row
/// (`align-items: center`), the row as high as the wordmark's line box.
fn brand(ui: &mut Ui, pass: &Pass<'_>) {
    let style = wordmark_style();
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), style.line_px()), Sense::hover());
    let painter = ui.painter();
    let icon = Rect::from_min_size(Pos2::new(row.left(), row.center().y - BRAND_ICON / 2.0), Vec2::splat(BRAND_ICON));
    icons::paint(painter, icon, Icon::Radar.at(BRAND_ICON, icons::STROKE_THIN), theme::AMBER);
    let galley = painter.layout_job(text::job("RustRadar", &style));
    painter.galley(Pos2::new(icon.right() + theme::SPACE_2, row.top()), galley, theme::TEXT_1);

    match status(pass.state, pass.now_ms) {
        Status::Text { label, tooltip } => {
            let style = status_style();
            let galley = ui.painter().layout_job(text::tabular(text::job(&label, &style)));
            let rect = Align2::RIGHT_CENTER.align_size_within_rect(Vec2::new(galley.size().x, style.line_px()), row);
            ui.painter().galley(rect.min, galley, theme::TEXT_3);
            if let Some(tip) = tooltip {
                tooltip::show(ui.interact(rect, Id::unique("rustradar::sidebar::status"), Sense::hover()), &tip);
            }
            // the ages tick every second (`now`)
            ui.ctx().request_repaint_after_secs(1.0);
        }
        Status::Stale { label, tooltip } => {
            let badge = Badge::new(label, Tone::Warn).size(theme::TEXT_SM).tabular();
            let size = badge.measure(ui.ctx());
            let rect = Align2::RIGHT_CENTER.align_size_within_rect(size, row);
            badge.paint(ui.painter(), rect.min);
            if let Some(tip) = tooltip {
                tooltip::show(ui.interact(rect, Id::unique("rustradar::sidebar::status"), Sense::hover()), &tip);
            }
            ui.ctx().request_repaint_after_secs(1.0);
        }
        Status::Waiting => {
            // an inline-block on the 12 px line's baseline: the line box is 17.8 px, the skeleton
            // its top 12 (Overpass at 12 px: ascent 11, descent 5, no half-leading)
            let line = 12.0 * theme::LINE_HEIGHT;
            let ascent = (12.0f32 * 0.883).round();
            let line_box = STATUS_SKELETON.y - ascent + line;
            let top = row.top() + (row.height() - line_box) / 2.0;
            let rect = Rect::from_min_size(Pos2::new(row.right() - STATUS_SKELETON.x, top), STATUS_SKELETON);
            skeleton::paint(ui.painter(), rect, theme::RADIUS);
        }
        Status::None => {}
    }
}

/// The toolbar: the Filters button and the counts, space between, centred on the row.
fn toolbar(ui: &mut Ui, pass: &mut Pass<'_>) {
    let open = pass.state.shortcuts.filters_open();
    let n = active_filters(pass.state);
    let row_top = ui.cursor().min.y;
    let pressed = ui
        .horizontal(|ui| {
            let filters = Button::new().icon(button::icon(Icon::Funnel)).text("Filters").count(n as u32).pressed(open).tooltip("Filters (F)").show(ui);
            if filters.clicked() {
                pass.actions.push(Action::ToggleOpen(Open::Filters));
            }
            filters.rect
        })
        .inner;
    let Some(counts) = counts(pass.state, pass.now_ms, pass.query()) else { return };
    let mut job = text::job(&group_thousands(counts.listed as f64), &count_style());
    text::append(&mut job, &counts.rest(), &counts_style());
    let galley = ui.painter().layout_job(text::tabular(job));
    let row = Rect::from_min_max(Pos2::new(pressed.right(), row_top), Pos2::new(ui.max_rect().right(), pressed.bottom()));
    let rect = Align2::RIGHT_CENTER.align_size_within_rect(galley.size(), row);
    ui.painter().galley(rect.min, galley, theme::TEXT_2);
}

/// The aircraft feed's alert or note.
fn alert(ui: &mut Ui, pass: &mut Pass<'_>, notice: Notice) {
    let text = notice.text();
    match notice {
        Notice::Limited { .. } => {
            Alert::warn(text).show(ui);
        }
        Notice::Failed { error, loading, .. } => {
            let mut retry = false;
            Alert::error(text).detail(error).show_with(ui, |ui| {
                retry = Button::new().icon(button::icon(Icon::RefreshCw)).text("Retry now").enabled(!loading).show(ui).clicked();
            });
            if retry {
                pass.actions.push(Action::RetryFeed);
            }
        }
        Notice::Partial(_) => {
            let width = ui.available_width();
            text::label(ui, text::wrapped(&text, &note_style(), width), Sense::hover());
        }
    }
    // the countdown ticks every second
    ui.ctx().request_repaint_after_secs(1.0);
}

#[cfg(test)]
mod tests {
    use rustradar_core::model::{LiveFlight, LiveSnapshot};
    use rustradar_core::ship_list::DEFAULT_SHIP_FILTERS;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::state::Startup;

    fn near(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
    }

    #[test]
    fn cards_that_fit_keep_their_natural_heights() {
        // today's dark-v1: the primary card 248, the list 295 (269 of rows), 900 − 56 high
        let primary = Item::fixed(248.0);
        let list = Item { base: 269.0, edges: 26.0, shrink: LIST_SHRINK, min: LIST_MIN_HEIGHT };
        assert!(near(&column(&[primary, list], 844.0, GAP), &[248.0, 295.0]));
        // a short list is held at 96 px of content
        let short = Item { base: 40.0, ..list };
        assert!(near(&column(&[primary, short], 844.0, GAP), &[248.0, 122.0]));
        // nothing grows
        let filters = Item { base: 300.0, edges: 2.0, shrink: FILTERS_SHRINK, min: 0.0 };
        assert!(near(&column(&[primary, filters, list], 900.0, GAP), &[248.0, 302.0, 295.0]));
    }

    #[test]
    fn the_list_shrinks_three_times_as_fast_as_the_filter_panel() {
        // the same content height each, 40 px too many
        let filters = Item { base: 300.0, edges: 0.0, shrink: FILTERS_SHRINK, min: 0.0 };
        let list = Item { base: 300.0, edges: 0.0, shrink: LIST_SHRINK, min: LIST_MIN_HEIGHT };
        let h = column(&[Item::fixed(200.0), filters, list], 200.0 + 600.0 + 2.0 * GAP - 40.0, GAP);
        assert!(near(&h, &[200.0, 290.0, 270.0]), "{h:?}");
        // the shares go by flex-shrink × base size: a list half as tall gives up 1.5 × as much
        let half = Item { base: 150.0, ..list };
        let h = column(&[Item::fixed(200.0), filters, half], 200.0 + 450.0 + 2.0 * GAP - 50.0, GAP);
        assert!(near(&h, &[200.0, 280.0, 120.0]), "{h:?}");
        // the primary card never shrinks
        assert_eq!(h[0], 200.0);
    }

    #[test]
    fn the_list_stops_at_96_and_the_filter_panel_gives_the_rest() {
        // a 600 px window: 544 for the column; primary 248, filters 300, list 300 (274 of rows)
        let primary = Item::fixed(248.0);
        let filters = Item { base: 298.0, edges: 2.0, shrink: FILTERS_SHRINK, min: 0.0 };
        let list = Item { base: 274.0, edges: 26.0, shrink: LIST_SHRINK, min: LIST_MIN_HEIGHT };
        let h = column(&[primary, filters, list], 544.0, GAP);
        // the list would fall to 33 px of rows: held at 96 (122 outer), the panel takes the rest
        assert!(near(&h, &[248.0, 150.0, 122.0]), "{h:?}");
        assert!((h.iter().sum::<f32>() + 2.0 * GAP - 544.0).abs() < 1e-3, "the column is full");
        // a little too tall: both give way, the list three times as fast for its height
        let h = column(&[primary, filters, list], 820.0, GAP);
        let over = 248.0 + 300.0 + 300.0 + 24.0 - 820.0;
        let (sf, sl) = (298.0, 3.0 * 274.0);
        assert!(near(&h, &[248.0, 300.0 - over * sf / (sf + sl), 300.0 - over * sl / (sf + sl)]), "{h:?}");
    }

    #[test]
    fn a_column_that_cannot_shrink_overflows() {
        let primary = Item::fixed(400.0);
        let list = Item { base: 200.0, edges: 26.0, shrink: LIST_SHRINK, min: LIST_MIN_HEIGHT };
        let h = column(&[primary, list], 300.0, GAP);
        assert!(near(&h, &[400.0, 122.0]), "{h:?}");
        assert!(near(&column(&[], 100.0, GAP), &[]));
        // nothing that can shrink
        assert!(near(&column(&[Item::fixed(80.0), Item::fixed(80.0)], 100.0, GAP), &[80.0, 80.0]));
    }

    fn state() -> AppState {
        AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, [1440.0, 900.0])
    }

    #[test]
    fn the_counts_say_what_the_page_says() {
        let c = Counts { listed: 1234, total: 5678, where_: "in view" };
        assert_eq!(c.rest(), " of 5,678 in view");
        assert_eq!(Counts { listed: 12, total: 12, where_: "worldwide" }.rest(), " worldwide");
        // nothing until the focus's first answer
        let s = state();
        assert_eq!(counts(&s, 0.0, ""), None);
        let mut s = state();
        s.persisted.settings.mode = Mode::Ships;
        assert_eq!(counts(&s, 0.0, ""), None);
        assert_eq!(active_filters(&s), 0);
        s.persisted.ship_filters = DEFAULT_SHIP_FILTERS.clone();
        assert_eq!(active_filters(&s), 0);
    }

    #[test]
    fn the_status_before_and_after_the_first_snapshot() {
        let s = state();
        assert_eq!(status(&s, 0.0), Status::Waiting, "the skeleton");
        let mut s = state();
        s.persisted.settings.mode = Mode::Trains;
        assert_eq!(status(&s, 0.0), Status::Waiting);
        assert_eq!(notice(&s, 0.0), None, "the trains tell their own failures");
    }

    #[test]
    fn the_notices_wording() {
        assert_eq!(Notice::Limited { retry_in: 12 }.text(), "Flightradar24 is limiting requests. Slowing down; next update in 12 s.");
        assert_eq!(Notice::Failed { retry_in: 0, error: "timeout".into(), loading: false }.text(), "Can't reach Flightradar24. Trying again in 0 s.");
        assert_eq!(Notice::Partial(1).text(), "1 region didn't load this round; some aircraft may be missing.");
        assert_eq!(Notice::Partial(3).text(), "3 regions didn't load this round; some aircraft may be missing.");
    }

    /// A snapshot from a fixture folder through the aircraft feed: the counts, the status and its
    /// tooltip.
    #[test]
    fn a_snapshot_gives_counts_and_an_age() {
        use crate::persist::tests::TempDir;
        use crate::state::aircraft;
        const T0: f64 = 1_782_036_000_000.0;
        let dir = TempDir::new("sidebar-snapshot");
        let flights = (0..3).map(|i| LiveFlight { id: 7 + i, callsign: format!("SAS{i}"), lat: 59.4, lon: 18.0, alt: 12_000, ..Default::default() }).collect();
        let snapshot = LiveSnapshot { flights, server_time_ms: T0 as u64, requests: 4, elapsed_ms: 321, ..Default::default() };
        std::fs::write(dir.0.join(aircraft::FIXTURE_LIVE), serde_json::to_vec(&snapshot).unwrap()).unwrap();
        let startup = Startup { fixture: Some(dir.0.clone()), ..Default::default() };
        let mut s = AppState::new(startup, None, None, None, ViewState::INITIAL, [1440.0, 900.0]);
        for t in [T0, T0 + 16.0, T0 + 32.0] {
            s.poll(t);
        }
        assert!(s.aircraft.snapshot().is_some());
        let c = counts(&s, T0 + 1000.0, "").expect("counts");
        assert_eq!((c.listed, c.total), (3, 3));
        assert_eq!(c.where_, if s.aircraft.regional() { "in view" } else { "worldwide" });
        match status(&s, T0 + 5000.0) {
            Status::Text { label, tooltip } => {
                assert!(label.starts_with("Updated") || label == "Updating…", "{label}");
                assert_eq!(tooltip.as_deref(), Some("4 requests in 321 ms"));
            }
            other => panic!("{other:?}"),
        }
        // long after: stale, in a badge, with the time of the last update
        match status(&s, T0 + 3_600_000.0) {
            Status::Stale { label, tooltip } => {
                assert!(label.starts_with("Data "), "{label}");
                assert!(tooltip.expect("tooltip").starts_with("Last update at "));
            }
            other => panic!("{other:?}"),
        }
        // the time in the viewer's zone (F1): UTC in tests and shots, the system's in the window
        let tip = |s: &AppState| match status(s, T0 + 3_600_000.0) {
            Status::Stale { tooltip, .. } => tooltip.unwrap_or_default(),
            other => panic!("{other:?}"),
        };
        assert!(tip(&s).starts_with("Last update at 10:00"), "{}", tip(&s));
        s.viewer_clocks = rustradar_core::time::Clocks::new("Europe/Stockholm");
        assert!(tip(&s).starts_with("Last update at 12:00"), "{}", tip(&s));
        assert_eq!(notice(&s, T0), None);
    }
}
