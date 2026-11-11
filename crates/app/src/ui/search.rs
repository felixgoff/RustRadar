//! The search box (item U8; doc 06 §5; `SearchBox.svelte`): the field in the sidebar's primary
//! card and the dropdown under it. What the searches find and what a pick does are the state's
//! (`state::search`); this module draws them and keeps what the component kept for itself:
//! whether the dropdown is open, the highlighted row and the 120 ms close after a blur.
//!
//! **The field** (§5.1) is U4's [`TextField`] at the search height (36 px of content and its
//! border), 14 px text, the `Search` icon 16/1.75 (`--text-3`, `--text-2` and the `--amber`
//! border while focused), the placeholder of the focus ([`placeholder`]), Chromium's own 1 × 2 px
//! padding round the input, and exactly one adornment at its right end: the spinner (14/2,
//! 900 ms) while a search is under way; else, empty, the `/` key cap (20 × 20 with its border,
//! mono 11 px weight 600 `--text-3`, tooltip `Press / to search`; none on phones); else, for the
//! trains and ships, the clear button (22 × 22, 4 px into the padding, `X` 14/1.75 in `--text-3`,
//! `--surface-hover` and `--text-1` under the pointer, tooltip `Clear the search`).
//!
//! **Behaviour** (§5.2): every change of the text hands it to the state ([`Ask::Typed`]), opens
//! the dropdown and lets go of the highlight; focusing opens it; a blur closes it 120 ms later.
//! It shows while open and, for the trains and ships, the query is not blank, else while one of
//! the two searches has settled. Down and Up move the highlight through the rows of both groups,
//! wrapping, and scroll it into view; Enter picks the highlighted row (the first when none is);
//! Escape closes the dropdown and lets go of the keyboard, and goes no further (the shell's keys
//! leave a focused field alone). The pointer entering a row highlights it; a click picks it, the
//! field keeping the keyboard while the pointer presses on the dropdown (`mousedown`'s
//! `preventDefault`). A pick or the clear button closes the dropdown and lets go of the keyboard.
//! `/` (the shell's [`take_focus_search`]) focuses the field and selects its text.
//!
//! **The dropdown** (§5.3): its own egui area over the sidebar (z 10: `Order::Foreground`) 8 px
//! under the field and as wide (on phones the caller gives the top bar's rectangle instead:
//! [`Layout::anchor`]), at most `min(440, 70 % of the window)` of content high and scrolling,
//! padding 4, `--surface-raised` in a 1 px `--line` border, radius 4. Its groups: the first is
//! the local search's (`Trains in view`, `Ships in view`) or the flights and airports (headed
//! `Flights and airports` only when the places show), the second `Places` (whenever the place
//! search is not idle), a 1 px `--line` above it 4 px down, with `© OpenStreetMap contributors,
//! via Photon` on the right of its heading while it has results. Headings 12 px weight 600
//! `--text-3`, padding 8 8 4; messages 13 px (`--text-3` when quiet, with a 12 px spinner while
//! searching), padding 8 8 12, wrapping; rows: padding 8, gap 12, radius 4, `--surface-hover`
//! when highlighted, a 26 × 26 `--surface` tile with the kind's icon at 14/1.75 (a flight's
//! `Plane` in `--amber`, an airport's `MapPin` and a place's icon by its kind in `--text-2`, a
//! train's or ship's in its map colour), the label 13 px and the detail 12 px `--text-3`, each on
//! one line cut with an ellipsis; a place's detail has its full address as tooltip.
//!
//! **Android**: the field reports its text to the soft keyboard's bridge every frame it is shown
//! (`platform::android::text::report`), so the app's own changes (a pick, the clear button) reach
//! the keyboard.
//!
//! Deviations:
//! - No `combobox`/`listbox` roles or live region (egui's AccessKit output is left as egui makes
//!   it).
//! - The dropdown does not animate (nor does today's).
//! - A heading's credit stands 1 px below the heading's top, where CSS's baseline alignment of the
//!   11 px credit with the 12 px heading puts it, rather than on egui's own baseline.

use std::borrow::Cow;

use egui::emath::GuiRounding;
use egui::scroll_area::ScrollBarVisibility;
use egui::{Area, Context, CornerRadius, Event, Frame, Id, Key, LayerId, Margin, Order, Pos2, Rect, ScrollArea, Sense, Stroke, StrokeKind, SurrenderFocusOn, Ui, Vec2};
use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use rustradar_core::model::{FindDetail, FindEntry, Place, PlaceKind};
use rustradar_core::num::js_string;
use rustradar_core::places::place_detail;
use rustradar_core::settings::Mode;

use super::icons::{self, Icon};
use super::shell::{Pass, take_focus_search};
use super::text::{self, Style};
use super::theme;
use super::widgets::spinner;
use super::widgets::text_field::{self, TextField};
use super::widgets::tooltip;
use crate::state::search::{Ask, FLIGHT_MIN_CHARS, LocalRow, PLACE_MIN_CHARS, Phase, Row, flight_detail, local_rows, local_words, placeholder};
use crate::state::{Action, AppState};

/// The dropdown's distance under the field (`top: calc(100% + var(--space-2))`).
pub const DROPDOWN_GAP: f32 = theme::SPACE_2;
/// Its content's most height, px, and as a share of the window's height (`min(440px, 70vh)`).
pub const DROPDOWN_MAX: f32 = 440.0;
pub const DROPDOWN_MAX_VH: f32 = 0.7;
/// Its padding (`--space-1`).
pub const DROPDOWN_PADDING: f32 = theme::SPACE_1;
/// A blur closes the dropdown this much later, s (`setTimeout(…, 120)`).
pub const BLUR_CLOSE: f64 = 0.120;
/// A row's padding (`--space-2`) and the gap between its tile and its text (`--space-3`).
pub const ROW_PADDING: f32 = theme::SPACE_2;
pub const ROW_GAP: f32 = theme::SPACE_3;
/// The row's tile (`.kind`: 26 × 26) and its icon.
pub const TILE: f32 = 26.0;
pub const ROW_ICON: f32 = 14.0;
/// The `/` key cap: 18 × 18 inside its 1 px border.
pub const KEY_CAP: f32 = 18.0 + 2.0 * theme::BORDER;
/// The clear button, and how far it reaches into the field's padding (`margin-right: -4px`).
pub const CLEAR: f32 = 22.0;
pub const CLEAR_OVERHANG: f32 = 4.0;
/// The field's icon (`<Search size={16} strokeWidth={1.75} />`).
pub const FIELD_ICON: f32 = 16.0;
/// A group heading's padding: top, sides, bottom (`8px 8px 4px`).
pub const HEADING_PADDING: [f32; 3] = [theme::SPACE_2, theme::SPACE_2, theme::SPACE_1];
/// A message's padding: top, sides, bottom (`8px 8px 12px`).
pub const MESSAGE_PADDING: [f32; 3] = [theme::SPACE_2, theme::SPACE_2, theme::SPACE_3];
/// The second group's distance below the first (`margin-top: var(--space-1)`), over its border.
pub const GROUP_GAP: f32 = theme::SPACE_1;

/// The headings' text (`.group`: 12 px weight 600 `--text-3`).
pub const fn heading_style() -> Style {
    Style::ui(theme::TEXT_SM, 600, theme::TEXT_3)
}

/// The credit beside `Places` (`.credit`: 11 px weight 400 `--text-3`).
pub const fn credit_style() -> Style {
    Style::ui(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// A message's text (`.message`: 13 px `--text-2`, `--text-3` when quiet).
pub const fn message_style(quiet: bool) -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, if quiet { theme::TEXT_3 } else { theme::TEXT_2 })
}

/// A row's label (13 px, `--text-1`) and detail (12 px `--text-3`).
pub const fn label_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_1)
}

pub const fn detail_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// The key cap's `/` (`font: 600 var(--text-xs) var(--font-data)`).
pub const fn key_cap_style() -> Style {
    Style::data(theme::TEXT_XS, 600, theme::TEXT_3).letter_spacing(0.0)
}

/// Where the box is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Layout {
    /// The phone's words and no key cap.
    pub phone: bool,
    /// The rectangle the dropdown hangs under and is as wide as (phones: the top bar's); the
    /// field's when `None`.
    pub anchor: Option<Rect>,
}

/// Where the box was drawn this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shown {
    /// The field, its border included.
    pub field: Rect,
    /// The dropdown, when it shows.
    pub dropdown: Option<Rect>,
    /// The field has the keyboard.
    pub focused: bool,
}

/// The desktop sidebar's search box: the shell's [`super::shell::Slots::search`].
pub fn show(ui: &mut Ui, pass: &mut Pass<'_>) {
    search_box(ui, pass, Layout::default());
}

/// What the component keeps between frames (its own `$state`), for the focus it was made for.
#[derive(Debug, Clone, Default, PartialEq)]
struct Memory {
    mode: Option<Mode>,
    /// `open`.
    open: bool,
    /// `activeKey`: the highlighted row, by key, so it stays put when the other group's results
    /// arrive.
    active: Option<String>,
    /// When the blur's timer closes the dropdown (egui's clock, s).
    close_at: Option<f64>,
    /// The field had the keyboard at the end of the last frame (for `onfocus`, `onblur`).
    focused: bool,
    /// The row under the pointer last frame (`mouseenter` is entering a row).
    hovered: Option<String>,
    /// The highlight moved by a key: scroll it into view.
    reveal: bool,
    /// The dropdown showed last frame.
    shown: bool,
}

fn memory_id() -> Id {
    Id::unique("rustradar::search")
}

fn dropdown_id() -> Id {
    Id::unique("rustradar::search::results")
}

/// A row of the dropdown, from the state's results or the local search.
#[derive(Debug, Clone, Copy)]
enum Item<'a> {
    Flight(usize, &'a FindEntry),
    Local(usize, &'a LocalRow),
    Place(usize, &'a Place),
}

/// A place kind's name as the page writes it.
fn kind_name(kind: PlaceKind) -> &'static str {
    match kind {
        PlaceKind::Address => "address",
        PlaceKind::Street => "street",
        PlaceKind::Poi => "poi",
        PlaceKind::Airport => "airport",
        PlaceKind::District => "district",
        PlaceKind::City => "city",
        PlaceKind::Town => "town",
        PlaceKind::Postcode => "postcode",
        PlaceKind::Region => "region",
        PlaceKind::Country => "country",
    }
}

/// A place's icon (`PLACE_ICON`).
pub fn place_icon(kind: PlaceKind) -> Icon {
    match kind {
        PlaceKind::Address => Icon::House,
        PlaceKind::Street => Icon::Signpost,
        PlaceKind::Poi => Icon::Landmark,
        PlaceKind::Airport => Icon::PlaneLanding,
        PlaceKind::District | PlaceKind::City | PlaceKind::Town => Icon::Building,
        PlaceKind::Postcode => Icon::Mailbox,
        PlaceKind::Region => Icon::Map,
        PlaceKind::Country => Icon::Flag,
    }
}

impl Item<'_> {
    /// The page's key: `f:{type}:{id}`, `l:{key}`, `p:{kind}:{label}:{lat},{lon}`.
    fn key(&self) -> String {
        match self {
            Item::Flight(_, e) => format!("f:{}:{}", if matches!(e.detail, FindDetail::Airport(_)) { "airport" } else { "live" }, e.id),
            Item::Local(_, l) => format!("l:{}", l.key),
            Item::Place(_, p) => format!("p:{}:{}:{},{}", kind_name(p.kind), p.label, js_string(p.lat), js_string(p.lon)),
        }
    }

    fn row(&self) -> Row {
        match self {
            Item::Flight(n, _) => Row::Flight(*n),
            Item::Local(n, _) => Row::Local(*n),
            Item::Place(n, _) => Row::Place(*n),
        }
    }
}

/// What the dropdown shows for one text of the field (the component's deriveds).
struct Model<'a> {
    mode: Mode,
    /// The local search's heading and noun (trains, ships).
    local: Option<(&'static str, &'static str)>,
    flights: Cow<'a, Phase<FindEntry>>,
    places: Cow<'a, Phase<Place>>,
    local_rows: Vec<LocalRow>,
    /// `query.trim()`.
    query: String,
}

impl<'a> Model<'a> {
    /// For `text` in the field at local time `now_ms`; `typed` when it changed this frame, so the
    /// searches stand where the keystroke leaves them (`oninput`) before the state has heard.
    fn new(state: &'a AppState, mode: Mode, text: &str, typed: bool, now_ms: f64) -> Self {
        let local = local_words(mode);
        let query = crate::ui::widgets::token_input::js_trim(text).to_owned();
        let (flights, places) = if typed {
            let n = query.encode_utf16().count();
            let flights = if local.is_some() || n < FLIGHT_MIN_CHARS { Phase::Idle } else { Phase::Searching };
            let places = if n < PLACE_MIN_CHARS { Phase::Idle } else { Phase::Searching };
            (Cow::Owned(flights), Cow::Owned(places))
        } else {
            (Cow::Borrowed(state.search.flights()), Cow::Borrowed(state.search.places()))
        };
        let local_rows = if local.is_some() { local_rows(state, text, now_ms) } else { Vec::new() };
        Model { mode, local, flights, places, local_rows, query }
    }

    /// `visible` but for `open`.
    fn showable(&self) -> bool {
        if self.local.is_some() { !self.query.is_empty() } else { self.flights.settled() || self.places.settled() }
    }

    fn searching(&self) -> bool {
        matches!(*self.flights, Phase::Searching) || matches!(*self.places, Phase::Searching)
    }

    fn show_places(&self) -> bool {
        !matches!(*self.places, Phase::Idle)
    }

    /// `firstItems` (the local rows or the flights) and the places, in order.
    fn items(&self) -> Vec<Item<'_>> {
        let first: Vec<Item<'_>> = if self.local.is_some() {
            self.local_rows.iter().enumerate().map(|(n, l)| Item::Local(n, l)).collect()
        } else {
            self.flights.results().iter().enumerate().map(|(n, e)| Item::Flight(n, e)).collect()
        };
        first.into_iter().chain(self.places.results().iter().enumerate().map(|(n, p)| Item::Place(n, p))).collect()
    }
}

/// `active`: the highlighted row's place, the first when the key is none or gone.
fn active_index(items: &[Item<'_>], key: Option<&str>) -> usize {
    key.and_then(|k| items.iter().position(|i| i.key() == k)).unwrap_or(0)
}

/// Takes the presses of `key` out of this frame's input, and answers how many there were.
fn take_presses(ctx: &Context, key: Key) -> usize {
    ctx.input_mut(|i| {
        let before = i.events.len();
        i.events.retain(|e| !matches!(e, Event::Key { key: k, pressed: true, .. } if *k == key));
        before - i.events.len()
    })
}

fn pressed(ctx: &Context, key: Key) -> bool {
    ctx.input(|i| i.events.iter().any(|e| matches!(e, Event::Key { key: k, pressed: true, .. } if *k == key)))
}

/// Selects the whole text of the field `id` (`input.select()`).
fn select_all(ctx: &Context, id: Id, text: &str) {
    let mut edit = TextEditState::load(ctx, id).unwrap_or_default();
    edit.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(text.chars().count()))));
    edit.store(ctx, id);
}

/// The search box for `pass`'s focus, laid out as `layout` says.
pub fn search_box(ui: &mut Ui, pass: &mut Pass<'_>, layout: Layout) -> Shown {
    let ctx = ui.ctx().clone();
    let state = pass.state;
    let mode = state.persisted.settings.mode;
    let time = ctx.input(|i| i.time);
    let mut mem: Memory = ctx.data(|d| d.get_temp(memory_id())).unwrap_or_default();
    // a new focus has a new box
    if mem.mode != Some(mode) {
        mem = Memory { mode: Some(mode), ..Default::default() };
    }
    // the blur's timer
    if let Some(at) = mem.close_at {
        if time >= at {
            mem.open = false;
            mem.close_at = None;
        } else {
            ctx.request_repaint_after_secs((at - time) as f32);
        }
    }

    let salt = ("rustradar::search::field", mode);
    let edit_id = ui.make_persistent_id(Id::NULL.with(salt)).with("edit");
    // `/`: focus and select all (`focus()`)
    let before = state.search.query(mode).to_owned();
    if take_focus_search(&ctx) {
        ctx.memory_mut(|m| m.request_focus(edit_id));
        select_all(&ctx, edit_id, &before);
    }
    let had_focus = ctx.memory(|m| m.has_focus(edit_id));

    // Down and Up, before the field reads them (`preventDefault`)
    if had_focus {
        let model = Model::new(state, mode, &before, false, pass.now_ms);
        let items = model.items();
        if mem.open && model.showable() && !items.is_empty() {
            // each press one row on (`move(±1)`), wrapping
            let steps = take_presses(&ctx, Key::ArrowDown) as i64 - take_presses(&ctx, Key::ArrowUp) as i64;
            if steps != 0 {
                let n = items.len() as i64;
                let active = active_index(&items, mem.active.as_deref()) as i64;
                mem.active = Some(items[(active + steps).rem_euclid(n) as usize].key());
                mem.reveal = true;
            }
        }
    }

    // the field; while the pointer is on the dropdown a press there keeps the keyboard here
    let pointer_on_dropdown = mem.shown && ctx.pointer_interact_pos().and_then(|p| ctx.layer_id_at(p)) == Some(LayerId::new(Order::Foreground, dropdown_id()));
    let surrender = ctx.options(|o| o.input_options.surrender_focus_on);
    if pointer_on_dropdown {
        ctx.options_mut(|o| o.input_options.surrender_focus_on = SurrenderFocusOn::Never);
    }
    let pre = Model::new(state, mode, &before, false, pass.now_ms);
    let trailing = if pre.searching() {
        spinner::SIZE
    } else if before.is_empty() {
        if layout.phone { 0.0 } else { KEY_CAP }
    } else if pre.local.is_some() {
        CLEAR - CLEAR_OVERHANG
    } else {
        0.0
    };
    let mut text = before.clone();
    let field = TextField::new(salt, &mut text)
        .height(text_field::SEARCH_HEIGHT)
        .size(theme::TEXT_BASE)
        .icon(Icon::Search.at(FIELD_ICON, icons::STROKE_THIN))
        .trailing(trailing)
        .native_padding()
        .placeholder(placeholder(mode, layout.phone))
        .show(ui);
    ctx.options_mut(|o| o.input_options.surrender_focus_on = surrender);
    debug_assert_eq!(field.response.id, edit_id, "the field's edit id");

    // `oninput`
    let typed = text != before;
    if typed {
        state.search.hand_over(text.clone());
        pass.actions.push(Action::Search(Ask::Typed));
        mem.open = true;
        mem.active = None;
    }
    // `onfocus`, `onblur`
    let focused = field.focused;
    if focused && !mem.focused {
        mem.open = true;
    }
    if !focused && mem.focused {
        mem.close_at = Some(time + BLUR_CLOSE);
        ctx.request_repaint_after_secs(BLUR_CLOSE as f32);
    }
    mem.focused = focused;

    let model = Model::new(state, mode, &text, typed, pass.now_ms);
    let items = model.items();
    let mut blur = false;
    let mut pick: Option<Row> = None;
    if had_focus {
        // Enter picks the highlighted row
        if pressed(&ctx, Key::Enter)
            && mem.open
            && model.showable()
            && let Some(item) = items.get(active_index(&items, mem.active.as_deref()))
        {
            take_presses(&ctx, Key::Enter);
            pick = Some(item.row());
        }
        // Escape closes and lets go, and goes no further
        if take_presses(&ctx, Key::Escape) > 0 {
            mem.open = false;
            blur = true;
        }
    }

    // the one adornment
    let color = if focused { theme::TEXT_2 } else { theme::TEXT_3 };
    let right = field.trailing.right();
    let middle = field.frame.center().y;
    let mut cleared = false;
    if model.searching() {
        let rect = Rect::from_center_size(Pos2::new(right - spinner::SIZE / 2.0, middle), Vec2::splat(spinner::SIZE));
        spinner::paint(ui.painter(), rect, spinner::SIZE, color);
    } else if text.is_empty() {
        if !layout.phone {
            let rect = Rect::from_center_size(Pos2::new(right - KEY_CAP / 2.0, middle), Vec2::splat(KEY_CAP));
            ui.painter().rect_stroke(rect, CornerRadius::same(theme::RADIUS_SM), Stroke::new(theme::BORDER, theme::LINE_CONTROL), StrokeKind::Inside);
            let slash = ui.painter().layout_job(text::job("/", &key_cap_style()));
            ui.painter().galley(rect.center() - slash.size() / 2.0, slash, theme::TEXT_3);
            tooltip::show(ui.interact(rect, Id::unique("rustradar::search::key"), Sense::hover()), "Press / to search");
        }
    } else if model.local.is_some() {
        let rect = Rect::from_center_size(Pos2::new(right + CLEAR_OVERHANG - CLEAR / 2.0, middle), Vec2::splat(CLEAR));
        let response = ui.interact(rect, Id::unique("rustradar::search::clear"), Sense::click());
        let hovered = response.hovered();
        if hovered {
            ui.painter().rect_filled(rect, CornerRadius::same(theme::RADIUS_SM), theme::SURFACE_HOVER);
        }
        icons::paint(ui.painter(), rect, Icon::X.at(ROW_ICON, icons::STROKE_THIN), if hovered { theme::TEXT_1 } else { theme::TEXT_3 });
        if tooltip::show(response, "Clear the search").clicked() {
            cleared = true;
        }
    }
    if cleared {
        pass.actions.push(Action::Search(Ask::Clear));
        mem.open = false;
        mem.active = None;
        blur = true;
    }

    // the dropdown
    let visible = mem.open && model.showable() && !cleared && pick.is_none() && !blur;
    let mut dropdown = None;
    if visible {
        let anchor = layout.anchor.unwrap_or(field.frame);
        let fresh = !mem.shown;
        let (rect, clicked) = results(&ctx, anchor, &model, &items, &mut mem, fresh);
        dropdown = Some(rect);
        if let Some(row) = clicked {
            pick = Some(row);
        }
    }
    mem.shown = dropdown.is_some();
    if let Some(row) = pick {
        // `pick`: reset (the state's part with the action), close, blur
        pass.actions.push(Action::Search(Ask::Pick(row)));
        mem.open = false;
        mem.active = None;
        blur = true;
    }
    if blur {
        ctx.memory_mut(|m| m.surrender_focus(edit_id));
    }
    // the soft keyboard's bridge learns the field's text, the app's changes included
    let reported = if pick.is_some() || cleared { String::new() } else { text };
    crate::platform::android::text::report(&ctx, edit_id, &reported);
    ctx.data_mut(|d| d.insert_temp(memory_id(), mem));
    Shown { field: field.frame, dropdown, focused: focused && !blur }
}

/// The dropdown hanging under `anchor`: its rectangle, and the row clicked. `fresh`: it was not
/// shown last frame (a new list starts at its top).
fn results(ctx: &Context, anchor: Rect, model: &Model<'_>, items: &[Item<'_>], mem: &mut Memory, fresh: bool) -> (Rect, Option<Row>) {
    let width = anchor.width();
    let inner = width - 2.0 * (theme::BORDER + DROPDOWN_PADDING);
    let max_height = DROPDOWN_MAX.min(DROPDOWN_MAX_VH * ctx.content_rect().height());
    let active = active_index(items, mem.active.as_deref());
    let mut clicked = None;
    let mut hovered_now = None;
    // Chromium paints the box from its exact top to its exact bottom, each snapped to the pixel
    // grid; egui puts the area's corner on the grid: the rows' height is cut to match, so the
    // bottom border lands on today's row
    let top = anchor.bottom() + DROPDOWN_GAP;
    let edges = 2.0 * (theme::BORDER + DROPDOWN_PADDING);
    let ppp = ctx.pixels_per_point();
    let snap = |y: f32| (y * ppp).round() / ppp;
    let content = content_height(ctx, model, items, inner);
    let height = (snap(top + edges + content.min(max_height)) - snap(top) - edges).max(0.0);
    // the fraction of a pixel cut off by the snapping is no reason to scroll: a scroll bar only
    // when the rows run past the most height
    let bar = if content > max_height { ScrollBarVisibility::VisibleWhenNeeded } else { ScrollBarVisibility::AlwaysHidden };
    let area = Area::new(dropdown_id())
        .order(Order::Foreground)
        .fixed_pos(Pos2::new(anchor.left(), top))
        .fade_in(false)
        .constrain(false)
        .show(ctx, |ui| {
            Frame::new()
                .fill(theme::SURFACE_RAISED)
                .stroke(Stroke::new(theme::BORDER, theme::LINE))
                .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                .inner_margin(Margin::same(DROPDOWN_PADDING as i8))
                .show(ui, |ui| {
                    ui.set_width(inner);
                    ui.set_max_width(inner);
                    ui.set_max_height(max_height);
                    let mut scroll = ScrollArea::vertical()
                        .id_salt("rustradar::search::scroll")
                        .max_height(height)
                        .min_scrolled_height(height)
                        .auto_shrink([false, false])
                        .scroll_bar_visibility(bar)
                        // `scrollIntoView` jumps
                        .animated(false);
                    if fresh {
                        scroll = scroll.vertical_scroll_offset(0.0);
                    }
                    scroll.show(ui, |ui| {
                        // a scroll bar takes its room from the rows, not the box
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        let mut n = 0;
                        let mut row = |ui: &mut Ui, item: &Item<'_>, n: usize| {
                            let response = row(ui, item, model.mode, n == active);
                            if response.hovered() {
                                hovered_now = Some(item.key());
                            }
                            if n == active && mem.reveal {
                                ui.scroll_to_rect(response.rect, None);
                                mem.reveal = false;
                            }
                            if response.clicked() {
                                clicked = Some(item.row());
                            }
                        };
                        let first = if model.local.is_some() { model.local_rows.len() } else { model.flights.results().len() };
                        // the first group
                        match model.local {
                            Some((heading_text, noun)) => {
                                heading(ui, heading_text, false);
                                if first == 0 {
                                    message(ui, &format!("No {noun} in view match \u{201c}{}\u{201d}.", model.query), false, false);
                                }
                            }
                            None => {
                                if model.show_places() {
                                    heading(ui, "Flights and airports", false);
                                }
                                match &*model.flights {
                                    Phase::Searching => message(ui, "Searching flights\u{2026}", true, true),
                                    Phase::Error => message(ui, "Flight search failed. Check your connection and try again.", false, false),
                                    _ if first == 0 => message(ui, &format!("No live flights or airports match \u{201c}{}\u{201d}.", model.query), false, false),
                                    _ => {}
                                }
                            }
                        }
                        for item in &items[..first] {
                            row(ui, item, n);
                            n += 1;
                        }
                        // the places
                        if model.show_places() {
                            ui.add_space(GROUP_GAP);
                            let (line, _) = ui.allocate_exact_size(Vec2::new(inner, theme::BORDER), Sense::hover());
                            // boxes are painted on whole pixels, as Chromium snaps them
                            ui.painter().rect_filled(line.round_to_pixels(ui.pixels_per_point()), CornerRadius::ZERO, theme::LINE);
                            heading(ui, "Places", !model.places.results().is_empty());
                            match &*model.places {
                                Phase::Searching => message(ui, "Looking up places\u{2026}", true, true),
                                Phase::Error => message(ui, "Places unavailable", true, false),
                                _ if model.places.results().is_empty() => message(ui, &format!("No places match \u{201c}{}\u{201d}.", model.query), false, false),
                                _ => {}
                            }
                            for item in &items[first..] {
                                row(ui, item, n);
                                n += 1;
                            }
                        }
                    });
                });
        });
    // `mouseenter`: the pointer entering a row highlights it
    if hovered_now.is_some() && hovered_now != mem.hovered {
        mem.active = hovered_now.clone();
    }
    mem.hovered = hovered_now;
    (area.response.rect, clicked)
}

/// The dropdown's rows, headings and messages laid out `inner` wide: their whole height, as
/// [`results`] draws them.
fn content_height(ctx: &Context, model: &Model<'_>, items: &[Item<'_>], inner: f32) -> f32 {
    let heading = HEADING_PADDING[0] + heading_style().line_px() + HEADING_PADDING[2];
    let message = |words: &str, quiet: bool, spin: bool| message_height(ctx, words, quiet, spin, inner);
    let first = if model.local.is_some() { model.local_rows.len() } else { model.flights.results().len() };
    let mut height = 0.0;
    match model.local {
        Some((_, noun)) => {
            height += heading;
            if first == 0 {
                height += message(&format!("No {noun} in view match \u{201c}{}\u{201d}.", model.query), false, false);
            }
        }
        None => {
            if model.show_places() {
                height += heading;
            }
            height += match &*model.flights {
                Phase::Searching => message("Searching flights\u{2026}", true, true),
                Phase::Error => message("Flight search failed. Check your connection and try again.", false, false),
                _ if first == 0 => message(&format!("No live flights or airports match \u{201c}{}\u{201d}.", model.query), false, false),
                _ => 0.0,
            };
        }
    }
    if model.show_places() {
        height += GROUP_GAP + theme::BORDER + heading;
        height += match &*model.places {
            Phase::Searching => message("Looking up places\u{2026}", true, true),
            Phase::Error => message("Places unavailable", true, false),
            _ if model.places.results().is_empty() => message(&format!("No places match \u{201c}{}\u{201d}.", model.query), false, false),
            _ => 0.0,
        };
    }
    height + items.iter().map(row_height).sum::<f32>()
}

/// What a message's text wraps to within a dropdown `inner` wide: its lines.
fn message_lines(ctx: &Context, words: &str, quiet: bool, spin: bool, inner: f32) -> Vec<String> {
    let [_, side, _] = MESSAGE_PADDING;
    let lead = if spin { spinner::SIZE_SMALL + theme::SPACE_2 } else { 0.0 };
    let galley = ctx.fonts_mut(|f| f.layout_job(text::wrapped(words, &message_style(quiet), inner - 2.0 * side - lead)));
    galley.rows.iter().map(|r| r.row.text()).collect()
}

/// A message's height: its padding and a 18.2 px line box per line.
fn message_height(ctx: &Context, words: &str, quiet: bool, spin: bool, inner: f32) -> f32 {
    let [top, _, bottom] = MESSAGE_PADDING;
    top + message_lines(ctx, words, quiet, spin, inner).len() as f32 * message_style(quiet).line_px() + bottom
}

/// What a row shows: its icon and colour, label, detail and the detail's tooltip.
fn row_parts(item: &Item<'_>, mode: Mode) -> (Icon, egui::Color32, String, String, Option<String>) {
    match item {
        Item::Flight(_, e) => {
            let live = matches!(e.detail, FindDetail::Live(_));
            (if live { Icon::Plane } else { Icon::MapPin }, if live { theme::AMBER } else { theme::TEXT_2 }, e.label.clone(), flight_detail(e), None)
        }
        Item::Local(_, l) => {
            // `local.icon`: the focus's
            let icon = if mode == Mode::Ships { Icon::Ship } else { Icon::TrainFront };
            (icon, egui::Color32::from_rgb(l.color[0], l.color[1], l.color[2]), l.label.clone(), l.detail.clone(), None)
        }
        Item::Place(_, p) => (place_icon(p.kind), theme::TEXT_2, p.name.clone(), place_detail(p), Some(p.label.clone())),
    }
}

/// Whether a row has a second line.
fn has_detail(item: &Item<'_>) -> bool {
    match item {
        Item::Flight(_, e) => !flight_detail(e).is_empty(),
        Item::Local(_, l) => !l.detail.is_empty(),
        Item::Place(_, p) => !place_detail(p).is_empty(),
    }
}

/// A row's height: its padding round the tile or its two lines, whichever is taller.
fn row_height(item: &Item<'_>) -> f32 {
    let column = label_style().line_px() + if has_detail(item) { detail_style().line_px() } else { 0.0 };
    2.0 * ROW_PADDING + column.max(TILE)
}

/// A group's heading (`h3.group`), with the Photon credit on its right when `credit`.
fn heading(ui: &mut Ui, label: &str, credit: bool) {
    let [top, side, bottom] = HEADING_PADDING;
    let style = heading_style();
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, top + style.line_px() + bottom), Sense::hover());
    let painter = ui.painter();
    let galley = painter.layout_job(text::job(label, &style));
    let at = Pos2::new(rect.left() + side, rect.top() + top);
    let label_right = at.x + galley.size().x;
    painter.galley(at, galley, theme::TEXT_3);
    if credit {
        // baseline-aligned with the 12 px heading: the 11 px line box 1 px lower
        let room = rect.right() - side - (label_right + theme::SPACE_2);
        let galley = painter.layout_job(text::truncated("\u{a9} OpenStreetMap contributors, via Photon", &credit_style(), room));
        let x = rect.right() - side - galley.size().x;
        painter.galley(Pos2::new(x, at.y + 1.0), galley, theme::TEXT_3);
    }
}

/// A message (`p.message`), quiet or not, with a 12 px spinner first when `spin`.
fn message(ui: &mut Ui, words: &str, quiet: bool, spin: bool) {
    let [top, side, bottom] = MESSAGE_PADDING;
    let style = message_style(quiet);
    let width = ui.available_width();
    let lead = if spin { spinner::SIZE_SMALL + theme::SPACE_2 } else { 0.0 };
    // CSS's line boxes are 1.4 × 13 = 18.2 px apart, where egui's rows take whole pixels: each
    // line is set on its own at its line box
    let lines = message_lines(ui.ctx(), words, quiet, spin, width);
    let height = lines.len() as f32 * style.line_px();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, top + height + bottom), Sense::hover());
    let body = Rect::from_min_size(Pos2::new(rect.left() + side, rect.top() + top), Vec2::new(width - 2.0 * side, height));
    if spin {
        let spot = Rect::from_center_size(Pos2::new(body.left() + spinner::SIZE_SMALL / 2.0, body.center().y), Vec2::splat(spinner::SIZE_SMALL));
        spinner::paint(ui.painter(), spot, spinner::SIZE_SMALL, style.color);
    }
    for (n, line) in lines.iter().enumerate() {
        let galley = ui.painter().layout_job(text::job(js_trim_end(line), &style));
        ui.painter().galley(Pos2::new(body.left() + lead, body.top() + n as f32 * style.line_px()), galley, style.color);
    }
}

/// A wrapped row's text without the space it broke at.
fn js_trim_end(line: &str) -> &str {
    line.trim_end_matches(' ')
}

/// One row (`button[role=option]`): its tile, label and detail, highlighted when `active`.
fn row(ui: &mut Ui, item: &Item<'_>, mode: Mode, active: bool) -> egui::Response {
    let (icon, color, label, detail, title) = row_parts(item, mode);
    let label_style = label_style();
    let detail_style = detail_style();
    let column = label_style.line_px() + if detail.is_empty() { 0.0 } else { detail_style.line_px() };
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, row_height(item)), Sense::click());
    let painter = ui.painter();
    let ppp = ui.pixels_per_point();
    if active {
        painter.rect_filled(rect.round_to_pixels(ppp), CornerRadius::same(theme::RADIUS_SM), theme::SURFACE_HOVER);
    }
    let tile = Rect::from_min_size(Pos2::new(rect.left() + ROW_PADDING, rect.center().y - TILE / 2.0), Vec2::splat(TILE)).round_to_pixels(ppp);
    painter.rect_filled(tile, CornerRadius::same(theme::RADIUS_SM), theme::SURFACE);
    icons::paint(painter, tile, icon.at(ROW_ICON, icons::STROKE_THIN), color);
    let left = tile.right() + ROW_GAP;
    let room = rect.right() - ROW_PADDING - left;
    let top = rect.center().y - column / 2.0;
    let galley = painter.layout_job(text::truncated(&label, &label_style, room));
    painter.galley(Pos2::new(left, top), galley, theme::TEXT_1);
    if !detail.is_empty() {
        let galley = painter.layout_job(text::truncated(&detail, &detail_style, room));
        let at = Rect::from_min_size(Pos2::new(left, top + label_style.line_px()), galley.size());
        painter.galley(at.min, galley, theme::TEXT_3);
        if let Some(title) = title {
            tooltip::show(ui.interact(at, response.id.with("detail"), Sense::hover()), &title);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use rustradar_core::model::{FindAirportDetail, FindLiveDetail};
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::state::Startup;
    use crate::state::search::{Search, SearchFeeds};
    use crate::state::aircraft::Answer;
    use crate::ui::shell::{self, Slots};

    const T0: f64 = 1_782_036_000_000.0;

    /// Answers every search at once with the same results.
    struct Canned {
        found: Vec<FindEntry>,
        places: Vec<Place>,
    }

    impl SearchFeeds for Canned {
        fn find(&mut self, _query: &str) -> Answer<Result<Vec<FindEntry>, String>> {
            Answer::ready(Ok(self.found.clone()))
        }

        fn geocode(&mut self, _query: &str, _near: Option<rustradar_core::trains::LatLon>, _lang: Option<&'static str>) -> Answer<Result<Vec<Place>, String>> {
            Answer::ready(Ok(self.places.clone()))
        }
    }

    fn live(id: &str, label: &str) -> FindEntry {
        FindEntry {
            id: id.into(),
            label: label.into(),
            name: None,
            match_kind: None,
            detail: FindDetail::Live(FindLiveDetail { lat: 59.6, lon: 17.9, schd_from: None, schd_to: None, ac_type: Some("A20N".into()), route: Some("ARN - CPH".into()), logo: None, reg: None, callsign: Some(label.into()), flight: None, operator: None }),
        }
    }

    fn place(name: &str) -> Place {
        Place { name: name.into(), label: format!("{name}, Stockholm, Sweden"), kind: PlaceKind::Poi, category: Some("Museum".into()), lat: 59.328, lon: 18.091, bbox: None }
    }

    /// A state whose searches answer with two flights, an airport and two places.
    fn state() -> AppState {
        let mut s = AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, [1440.0, 900.0]);
        let airport = FindEntry { id: "ARN".into(), label: "Stockholm Arlanda Airport".into(), name: None, match_kind: None, detail: FindDetail::Airport(FindAirportDetail { lat: 59.65, lon: 17.92, size: None }) };
        s.search = Search::with_feeds(Box::new(Canned { found: vec![live("3a4b5c6", "SK1415"), live("3a4b5c7", "SK1417"), airport], places: vec![place("Vasa Museum"), place("Vasastan")] }));
        s.poll(T0);
        s
    }

    /// The shell with the search box, its actions applied and a poll after each frame, 50 ms
    /// apart.
    struct Harness {
        ctx: Context,
        state: AppState,
        time: f64,
    }

    fn shown_of(ui: &mut Ui, pass: &mut Pass<'_>) {
        let shown = search_box(ui, pass, Layout::default());
        ui.ctx().data_mut(|d| d.insert_temp(Id::unique("test::shown"), Some(shown)));
    }

    impl Harness {
        fn new() -> Harness {
            let ctx = Context::default();
            theme::apply(&ctx);
            Harness { ctx, state: state(), time: 0.0 }
        }

        fn frame(&mut self, events: Vec<Event>) -> Option<Shown> {
            self.time += 0.05;
            let now = T0 + self.time * 1000.0;
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0))), time: Some(self.time), events, ..Default::default() };
            let mut actions = Vec::new();
            let slots = Slots { search: Some(shown_of), ..Slots::default() };
            let state = &self.state;
            let mut out = self.ctx.run_ui(raw, |ui| {
                shell::show(ui.ctx(), state, now, &mut actions, &slots);
            });
            out.textures_delta.clear();
            for action in actions {
                self.state.apply(action, now);
            }
            self.state.poll(now);
            self.ctx.data(|d| d.get_temp::<Option<Shown>>(Id::unique("test::shown"))).flatten()
        }

        fn key(&mut self, key: Key) -> Option<Shown> {
            self.frame(vec![Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }])
        }

        fn typed(&mut self, text: &str) -> Option<Shown> {
            self.frame(vec![Event::Text(text.into())])
        }

        /// Frames until the debounces are over.
        fn settle(&mut self) -> Option<Shown> {
            for _ in 0..12 {
                self.frame(Vec::new());
            }
            self.frame(Vec::new())
        }
    }

    #[test]
    fn slash_focuses_and_typing_opens_the_dropdown_when_a_search_settles() {
        let mut h = Harness::new();
        let shown = h.frame(Vec::new()).expect("shown");
        assert_eq!(shown.field.height(), text_field::SEARCH_HEIGHT);
        assert!(!shown.focused && shown.dropdown.is_none());
        // `/` through the shell's keys: the field takes the keyboard
        h.frame(vec![Event::Key { key: Key::Slash, physical_key: Some(Key::Slash), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }, Event::Text("/".into())]);
        let shown = h.frame(Vec::new()).expect("shown");
        assert!(shown.focused, "focused by /");
        assert_eq!(h.state.search.query(Mode::Aircraft), "", "the / did not type itself");
        h.typed("sk");
        assert_eq!(h.state.search.query(Mode::Aircraft), "sk");
        assert!(h.state.search.searching());
        assert!(h.frame(Vec::new()).unwrap().dropdown.is_none(), "nothing has settled yet");
        let shown = h.settle().expect("shown");
        let dropdown = shown.dropdown.expect("the flights settled: the dropdown");
        assert_eq!(dropdown.left(), shown.field.left());
        assert_eq!(dropdown.width(), shown.field.width());
        // (egui puts an area's corner on the pixel grid, as Chromium paints a box's edge)
        assert!((dropdown.top() - (shown.field.bottom() + DROPDOWN_GAP)).abs() <= 0.5, "{dropdown:?} under {:?}", shown.field);
        assert_eq!(h.state.search.flights().results().len(), 3);
    }

    #[test]
    fn arrows_move_the_highlight_and_enter_picks_it() {
        let mut h = Harness::new();
        h.frame(Vec::new());
        h.frame(vec![Event::Key { key: Key::Slash, physical_key: Some(Key::Slash), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
        h.typed("sk1");
        h.settle();
        // three flight rows and two places: Down twice, Up once, Down once: the third row
        for key in [Key::ArrowDown, Key::ArrowDown, Key::ArrowUp, Key::ArrowDown] {
            h.key(key);
        }
        let mem: Memory = h.ctx.data(|d| d.get_temp(memory_id())).unwrap();
        assert_eq!(mem.active.as_deref(), Some("f:airport:ARN"));
        h.key(Key::Enter);
        assert_eq!(h.state.flight.focus_airport(), Some("ARN"), "the airport's board");
        assert_eq!(h.state.search.query(Mode::Aircraft), "", "reset");
        let shown = h.frame(Vec::new()).unwrap();
        assert!(!shown.focused && shown.dropdown.is_none(), "closed and blurred");
    }

    #[test]
    fn up_from_the_first_row_wraps_to_the_last_place() {
        let mut h = Harness::new();
        h.frame(Vec::new());
        h.frame(vec![Event::Key { key: Key::Slash, physical_key: Some(Key::Slash), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
        h.typed("vas");
        h.settle();
        h.key(Key::ArrowUp);
        let mem: Memory = h.ctx.data(|d| d.get_temp(memory_id())).unwrap();
        assert_eq!(mem.active.as_deref(), Some("p:poi:Vasastan, Stockholm, Sweden:59.328,18.091"));
        h.key(Key::Enter);
        assert_eq!(h.state.flight.place().map(|p| p.name.as_str()), Some("Vasastan"));
    }

    #[test]
    fn escape_closes_and_blurs_and_does_not_close_the_top_thing() {
        let mut h = Harness::new();
        h.state.flight.set_focus_airport(Some("ARN".into()));
        h.frame(Vec::new());
        h.frame(vec![Event::Key { key: Key::Slash, physical_key: Some(Key::Slash), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
        h.typed("sk1");
        assert!(h.settle().unwrap().dropdown.is_some());
        let shown = h.key(Key::Escape).unwrap();
        assert!(shown.dropdown.is_none());
        let shown = h.frame(Vec::new()).unwrap();
        assert!(!shown.focused, "blurred");
        assert_eq!(h.state.flight.focus_airport(), Some("ARN"), "the board stays: Escape went no further");
        assert_eq!(h.state.search.query(Mode::Aircraft), "sk1", "the text stays");
        // focused again: open again, the results still there
        h.frame(vec![Event::Key { key: Key::Slash, physical_key: Some(Key::Slash), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
        assert!(h.frame(Vec::new()).unwrap().dropdown.is_some());
        // and the next Escape, unfocused, is the shell's: the board closes
        h.key(Key::Escape);
        h.frame(Vec::new());
        h.key(Key::Escape);
        assert_eq!(h.state.flight.focus_airport(), None);
    }

    #[test]
    fn a_blur_closes_the_dropdown_120_ms_later() {
        let mut h = Harness::new();
        h.frame(Vec::new());
        h.frame(vec![Event::Key { key: Key::Slash, physical_key: Some(Key::Slash), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
        h.typed("sk1");
        let field = h.settle().unwrap().field;
        // a click on the map, far from the box
        let at = Pos2::new(800.0, 500.0);
        let button = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        h.frame(vec![Event::PointerMoved(at), button(true)]);
        let shown = h.frame(vec![button(false)]).unwrap();
        assert!(!shown.focused);
        assert!(shown.dropdown.is_some(), "still open just after the blur");
        assert!(h.frame(Vec::new()).unwrap().dropdown.is_some(), "50 ms after");
        assert!(h.frame(Vec::new()).unwrap().dropdown.is_some(), "100 ms after");
        let shown = h.frame(Vec::new()).unwrap();
        assert!(shown.dropdown.is_none(), "closed 120 ms after it");
        assert!(field.width() > 0.0);
    }

    #[test]
    fn a_click_on_a_row_picks_it_and_keeps_the_keyboard_until_then() {
        let mut h = Harness::new();
        h.frame(Vec::new());
        h.frame(vec![Event::Key { key: Key::Slash, physical_key: Some(Key::Slash), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
        h.typed("sk1");
        let dropdown = h.settle().unwrap().dropdown.expect("open");
        // the second row: 4 of padding, a 28.8 heading (the places show), then 51 px rows
        let at = Pos2::new(dropdown.center().x, dropdown.top() + 1.0 + 4.0 + 28.8 + 51.0 + 25.0);
        h.frame(vec![Event::PointerMoved(at)]);
        h.frame(Vec::new());
        let button = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let shown = h.frame(vec![button(true)]).unwrap();
        assert!(shown.focused, "a press on the dropdown keeps the keyboard");
        let mem: Memory = h.ctx.data(|d| d.get_temp(memory_id())).unwrap();
        assert_eq!(mem.active.as_deref(), Some("f:live:3a4b5c7"), "the pointer entering highlights");
        h.frame(vec![button(false)]);
        assert_eq!(h.state.flight.selected_id(), Some(0x3a4b5c7), "SK1417 picked");
        assert!(!h.frame(Vec::new()).unwrap().focused);
    }

    #[test]
    fn the_local_search_shows_without_asking_and_clears() {
        let mut h = Harness::new();
        h.state.persisted.settings.mode = Mode::Ships;
        h.frame(Vec::new());
        h.frame(vec![Event::Key { key: Key::Slash, physical_key: Some(Key::Slash), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
        let shown = h.typed("x").unwrap();
        assert!(shown.dropdown.is_some(), "local: at once, with its message");
        assert_eq!(h.state.search.list_query(Mode::Ships), "x");
        // the clear button at the field's right
        let at = Pos2::new(shown.field.right() - 1.0 - 12.0 + 4.0 - 11.0, shown.field.center().y);
        let button = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        h.frame(vec![Event::PointerMoved(at)]);
        h.frame(vec![button(true)]);
        h.frame(vec![button(false)]);
        assert_eq!(h.state.search.query(Mode::Ships), "", "cleared");
        let shown = h.frame(Vec::new()).unwrap();
        assert!(!shown.focused && shown.dropdown.is_none());
    }

    /// Twenty flights run past the dropdown's 440 px: Down past the last row in sight scrolls the
    /// highlight into view, so the row the pointer then enters at the bottom is that one.
    #[test]
    fn the_highlight_scrolls_into_view() {
        let mut h = Harness::new();
        let found = (0..20).map(|n| live(&format!("3a4b{n:03x}"), &format!("SK{n}"))).collect();
        h.state.search = Search::with_feeds(Box::new(Canned { found, places: Vec::new() }));
        h.frame(Vec::new());
        h.frame(vec![Event::Key { key: Key::Slash, physical_key: Some(Key::Slash), pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]);
        h.typed("sk");
        let dropdown = h.settle().unwrap().dropdown.expect("open");
        // 440 of rows (snapped as Chromium snaps), the border and the padding; as wide as the field,
        // the scroll bar inside
        assert!((dropdown.height() - (DROPDOWN_MAX + 10.0)).abs() <= 1.0, "{dropdown:?}");
        let field = h.frame(Vec::new()).unwrap().field;
        assert_eq!(dropdown.width(), field.width(), "{dropdown:?} under {field:?}");
        for _ in 0..12 {
            h.key(Key::ArrowDown);
        }
        let mem: Memory = h.ctx.data(|d| d.get_temp(memory_id())).unwrap();
        assert_eq!(mem.active.as_deref(), Some("f:live:3a4b00c"), "twelve rows down");
        h.frame(Vec::new());
        // the pointer enters the bottom row in sight
        let at = Pos2::new(dropdown.center().x, dropdown.bottom() - 1.0 - 4.0 - 10.0);
        h.frame(vec![Event::PointerMoved(at)]);
        h.frame(Vec::new());
        let mem: Memory = h.ctx.data(|d| d.get_temp(memory_id())).unwrap();
        assert_eq!(mem.active.as_deref(), Some("f:live:3a4b00c"), "row 12 was scrolled to the bottom");
        h.key(Key::Enter);
        assert_eq!(h.state.flight.selected_id(), Some(0x3a4b00c));
    }

    /// The heights CSS gives: a heading 8 + 16.8 + 4, a message 8 + 18.2 per line + 12, a row
    /// 8 + max(26, 18.2 + 16.8) + 8, or 8 + 26 + 8 without a detail.
    #[test]
    fn the_parts_are_as_tall_as_today_s() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let mut heights = Vec::new();
        let entry = live("1", "SK1");
        let mut bare = live("2", "SK2");
        if let FindDetail::Live(d) = &mut bare.detail {
            d.route = None;
            d.ac_type = None;
        }
        let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 800.0))), ..Default::default() }, |ui| {
            ui.set_width(276.0);
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let mut at = |ui: &mut Ui, f: &dyn Fn(&mut Ui)| {
                let top = ui.cursor().min.y;
                f(ui);
                heights.push(ui.cursor().min.y - top);
            };
            at(ui, &|ui| heading(ui, "Places", true));
            at(ui, &|ui| message(ui, "Places unavailable", true, false));
            at(ui, &|ui| message(ui, "Flight search failed. Check your connection and try again.", false, false));
            at(ui, &|ui| message(ui, "Searching flights\u{2026}", true, true));
            at(ui, &|ui| {
                row(ui, &Item::Flight(0, &entry), Mode::Aircraft, false);
            });
            at(ui, &|ui| {
                row(ui, &Item::Flight(1, &bare), Mode::Aircraft, false);
            });
        });
        out.textures_delta.clear();
        let want = [28.8, 38.2, 56.4, 38.2, 51.0, 42.0];
        for (got, want) in heights.iter().zip(want) {
            assert!((got - want).abs() <= 1.0 / 32.0, "{heights:?} for {want:?}: egui keeps sizes in 32nds");
        }
    }

    #[test]
    fn the_rows_keys_and_icons() {
        let e = live("3a4b5c6", "SK1415");
        assert_eq!(Item::Flight(0, &e).key(), "f:live:3a4b5c6");
        let p = Place { lat: 59.5, lon: 18.0, ..place("Vasa") };
        assert_eq!(Item::Place(0, &p).key(), "p:poi:Vasa, Stockholm, Sweden:59.5,18");
        assert_eq!(place_icon(PlaceKind::Town), Icon::Building);
        assert_eq!(place_icon(PlaceKind::Postcode), Icon::Mailbox);
        assert_eq!(place_icon(PlaceKind::Country), Icon::Flag);
    }
}
