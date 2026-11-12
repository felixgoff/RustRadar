//! The phone layout (item U19; doc 06 §2.2, §2.4, §2.5, §17, §19.1; `phone.svelte.ts`,
//! `+page.svelte:1362-1760`, `1935-2024`, `BottomSheet.svelte`, `MapControls.svelte`'s phone
//! rules): the map fills the screen, the search and mode buttons ride along the top, the lists,
//! filters and details live in one bottom sheet, the map controls are a slim rail above it.
//!
//! **When** ([`Layout::of`], `PHONE_QUERY`): the window is at most 899 px wide or at most 500 px
//! high, re-read every frame; a phone **on its side** is at most 500 px high and wider than high.
//! The desktop window cannot get there (its smallest is 900 × 600, plan section 7), so the layout is
//! for phones; tests draw it at a phone's size on any machine. Every place below is measured from
//! the window's whole area (`viewport-fit=cover`) and kept clear of the safe areas egui is given
//! (`RawInput::safe_area_insets`: the notch, the status bar, the home bar).
//!
//! What [`ui`] draws, in the page's order (the shell's keys first, as on the desktop):
//! 1. **The top bar** (`.sidebar` on phones, z 3): at `top: max(8, safe-top)`, `left: max(8,
//!    safe-left)`, `right: max(8, safe-right)` (on its side: 320 px wide down the left), the
//!    primary card with 4 px of padding round one wrapping row 4 px apart: the search box (item
//!    U8's, its phone words, no key cap, the dropdown as wide as the bar: [`search::search_box`])
//!    taking what the mode buttons leave, the mode buttons as 44 × 36 icons ([`modebar::row`]), and
//!    under them, on a row of its own, the aircraft feed's alert or note ([`sidebar::notice`]). No
//!    brand and no status. While a detail is open the bar slides up out of sight (`translateY(-100%
//!    - 8px)`) and fades over 220 ms, and is gone once it has; it comes back the same way.
//! 2. **The bottom sheet** ([`sheet`], z 4): labelled `Details` with a detail open, else `Flights`,
//!    `Trains` or `Ships`; clear 12 at the top with a detail, else 64. For the lists its head is the
//!    toolbar turned round (`row-reverse`): the counts on the left (the number at 14 px, the noun
//!    `flights`, `trains` or `ships` shown), the Filters button on the right; its body (scrolling)
//!    the filter panel while open (its own card, `margin: 0 8px 8px`) and the list without its card
//!    (`padding: 12px 8px`). With a detail open the body is the open panel without its chrome,
//!    filling the body and scrolling itself, its summary first and marked for `peek` (the panels'
//!    own phone order). The sheet's detent is the state's ([`crate::state::AppState::sheet`]); the
//!    handle's asks go back as [`Action::Sheet`].
//! 3. **The map controls' rail**: `right: max(8, safe-right)`, `bottom` the sheet's height + 24
//!    (on its side: 24), eased over 220 ms; only Map layers and Settings, Tilt, and the compass
//!    while the map is turned or tilted. A menu opens as a sheet of its own (z 6): fixed along the
//!    bottom, the window's width, `max-height: 70dvh` of content and scrolling, 16 px of padding
//!    (at least the side safe areas, and the home bar's under it), top corners 12, entering over
//!    180 ms from 24 px below and transparent; Settings with its API keys entry
//!    ([`menus::settings`] for phones). A press outside the rail and the menu closes it, as do its
//!    button and Escape ([`map_controls::escape`], which the UI pass runs first).
//! 4. **The credits** ([`attribution::show`]): upright, across the window on the sheet at 7 px with
//!    square corners; on its side, at the bottom right clear of 360 px. In debug builds the dev
//!    readout at the top right, over the bar (z 5).
//!
//! **Checked** against `docs/port/reference/dark-v1-390x844.png` and `satellite-v6-390x844.png`
//! (`tests/phone.rs`, through the window's engine over the map): the top bar's card and field
//! edges, the chosen mode's cell, the ships' placeholder, the sheet's top edge at peek and its
//! handle's pill on the same pixels; the Filters button, the rail's stacks, the credits and the
//! ships' sort under the head within a pixel. The alert under the field differs (today's
//! plain-browser alert is web-only, plan section 6). With a 47 px top and 34 px bottom safe area:
//! the list's sheet at peek, half and full, a flight's and a train's summary at peek, the
//! Settings menu's sheet; a phone on its side with 47 px side notches.
//!
//! Deviations:
//! - The search box's dropdown hangs under the top bar as it was last drawn (the bar learns its
//!   height by drawing): a frame late when an alert comes or goes.
//! - Keyboard focus does not go back to the menu's button when Escape closes a menu on a phone
//!   (the rail's buttons are not the desktop column's).

use egui::{Align, Align2, Area, Context, CornerRadius, Id, LayerId, Layout as EguiLayout, Margin, Order, Pos2, Rect, ScrollArea, Sense, Stroke, Ui, UiBuilder, Vec2};
use rustradar_core::settings::Mode;
use rustradar_core::text::group_thousands;

use super::attribution::{self, Attribution, Place};
use super::icons::{self, Icon};
use super::map_controls::{self, rotated, tilt_label, tilted, toggled};
use super::modebar::{self, Bar};
use super::search;
use super::sheet::{self, Detent, Props};
use super::shell::{self, Panel, Pass, Slots};
use super::sidebar::{self, Counts, Notice};
use super::text::{self, Style};
use super::widgets::alert::Alert;
use super::widgets::button::{self, Button};
use super::widgets::icon_button::{self, IconButton};
use super::{anim, menus, panel, theme};
use crate::state::shortcuts::{Menu, Open};
use crate::state::{Action, AppState};

/// The phone query's width: at most this wide (`max-width: 899px`).
pub const MAX_WIDTH: f32 = 899.0;
/// … or at most this high (`max-height: 500px`).
pub const MAX_HEIGHT: f32 = 500.0;
/// The top bar's least distance from the window's edges (`var(--space-2)`).
pub const EDGE: f32 = theme::SPACE_2;
/// The primary card's padding and its parts' gap on phones (`var(--space-1)`).
pub const PADDING: f32 = theme::SPACE_1;
/// The top bar on a phone on its side (`width: 320px`).
pub const LANDSCAPE_WIDTH: f32 = 320.0;
/// The rail's least distance from the right edge, and how far above the sheet it stands.
pub const RAIL_EDGE: f32 = theme::SPACE_2;
pub const RAIL_ABOVE: f32 = 24.0;
/// A menu's sheet: the most of the window's height its content takes (`max-height: 70dvh`), its
/// padding and its top corners.
pub const MENU_HEIGHT: f32 = 0.7;
pub const MENU_PADDING: f32 = theme::SPACE_4;
pub const MENU_RADIUS: u8 = 12;
/// The filter panel's margin in the sheet (`margin: 0 8px 8px`), and the list's padding
/// (`padding: 12px 8px`).
pub const FILTERS_MARGIN: f32 = theme::SPACE_2;
pub const LIST_PADDING: Vec2 = Vec2::new(theme::SPACE_2, theme::SPACE_3);

/// Whether a window `size` CSS px is phone-sized (`(max-width: 899px), (max-height: 500px)`).
pub fn is_phone(size: Vec2) -> bool {
    size.x <= MAX_WIDTH || size.y <= MAX_HEIGHT
}

/// Whether a window `size` is a phone on its side (`(max-height: 500px) and (orientation:
/// landscape)`).
pub fn is_landscape(size: Vec2) -> bool {
    size.y <= MAX_HEIGHT && size.x > size.y
}

/// The phone layout of this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// The window's whole area.
    pub viewport: Rect,
    /// What the notch, the bars and the home indicator take of it.
    pub insets: egui::SafeAreaInsets,
    /// A phone on its side.
    pub landscape: bool,
}

impl Layout {
    /// The phone layout when the window is phone-sized, else `None` (the desktop's).
    pub fn of(ctx: &Context) -> Option<Layout> {
        let viewport = ctx.viewport_rect();
        is_phone(viewport.size()).then(|| Layout { viewport, insets: ctx.input(|i| i.safe_area_insets()), landscape: is_landscape(viewport.size()) })
    }

    fn inset(&self) -> egui::epaint::MarginF32 {
        self.insets.0
    }

    /// The top bar's left, top and width (`max(8, safe-*)`; on its side 320 wide).
    pub fn top_bar(&self) -> (f32, f32, f32) {
        let i = self.inset();
        let left = self.viewport.left() + EDGE.max(i.left);
        let top = self.viewport.top() + EDGE.max(i.top);
        let width = if self.landscape { LANDSCAPE_WIDTH } else { self.viewport.right() - EDGE.max(i.right) - left };
        (left, top, width)
    }
}

/// On a phone, whether the bottom sheet is above its peek detent (`layout.phone && sheetDetent !==
/// "peek"`): closing the top thing lowers it last of all.
pub fn sheet_raised(state: &AppState) -> bool {
    let [w, h] = state.controller.size();
    is_phone(Vec2::new(w as f32, h as f32)) && state.sheet.detent() != Detent::Peek
}

/// Where the phone layout put its parts this frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Placed {
    /// The top bar's card (none while hidden).
    pub top_bar: Option<Rect>,
    /// The search field, the mode buttons' row.
    pub search: Option<Rect>,
    pub modes: Option<Rect>,
    /// The sheet.
    pub sheet: Option<sheet::Shown>,
    /// The rail and its stacks, the open menu's sheet.
    pub rail: Option<Rect>,
    pub stacks: [Option<Rect>; 2],
    pub menu: Option<Rect>,
    /// The credits.
    pub credits: Option<Rect>,
}

/// The phone layout for `state` at local time `now_ms` with the app's components, when the window
/// is phone-sized: answers `None` (and draws nothing) otherwise.
pub fn ui(ctx: &Context, state: &AppState, now_ms: f64, actions: &mut Vec<Action>) -> Option<Placed> {
    let layout = Layout::of(ctx)?;
    Some(show(ctx, layout, state, now_ms, actions, &Slots::APP))
}

/// The phone layout with the components in `slots` (see the module documentation).
pub fn show(ctx: &Context, layout: Layout, state: &AppState, now_ms: f64, actions: &mut Vec<Action>, slots: &Slots) -> Placed {
    shell::keys(ctx, state, actions);
    let mut placed = Placed::default();
    let detail = shell::detail_open(state);
    {
        let mut pass = Pass { state, now_ms, actions };
        top_bar(ctx, layout, &mut pass, slots, detail, &mut placed);
    }

    // the sheet: the list, or the details in its place
    let mode = state.persisted.settings.mode;
    let board_alone = state.airport.code().is_some() && state.flight.selected().is_none();
    let label = if detail {
        "Details"
    } else {
        match mode {
            Mode::Aircraft => "Flights",
            Mode::Trains => "Trains",
            Mode::Ships => "Ships",
        }
    };
    let props = Props {
        label,
        detent: state.sheet.detent_with(detail, board_alone),
        clear: if detail { sheet::CLEAR_DETAIL } else { sheet::CLEAR_LIST },
        landscape: layout.landscape,
    };
    let shown = {
        let mut pass = Pass { state, now_ms, actions };
        let pass = std::cell::RefCell::new(&mut pass);
        let mut head = |ui: &mut Ui| toolbar(ui, &mut pass.borrow_mut());
        let mut body = |ui: &mut Ui| {
            let mut pass = pass.borrow_mut();
            if detail { panels(ui, &mut pass, slots) } else { list_view(ui, &mut pass, slots) }
        };
        let head: Option<&mut dyn FnMut(&mut Ui)> = if detail { None } else { Some(&mut head) };
        sheet::show(ctx, &props, head, &mut body)
    };
    if let Some(detent) = shown.ask {
        actions.push(Action::Sheet(detent));
    }
    placed.sheet = Some(shown);

    rail(ctx, layout, state, actions, shown.height, &mut placed);

    // the credits on the sheet, the readout at the top right in debug builds
    if cfg!(debug_assertions) {
        attribution::dev_readout(ctx, state);
    }
    let place = if layout.landscape { Place::PhoneLandscape } else { Place::Phone { sheet_height: shown.height } };
    placed.credits = Some(attribution::show(ctx, &Attribution::of(state, now_ms), place));
    placed
}

// ------------------------------------------------------------------------------------------------
// the top bar

fn top_bar_id() -> Id {
    Id::unique("rustradar::phone::top_bar")
}

/// The top bar (see the module documentation).
fn top_bar(ctx: &Context, layout: Layout, pass: &mut Pass<'_>, slots: &Slots, detail: bool, placed: &mut Placed) {
    let id = top_bar_id();
    // slides up and fades while a detail is open, hidden once it has
    let hidden = anim::toggle(ctx, id.with("hide"), detail, anim::TOP_BAR);
    if detail && hidden >= 1.0 {
        return;
    }
    let (left, top, width) = layout.top_bar();
    let last: Option<Rect> = ctx.data(|d| d.get_temp(id.with("rect")));
    let height = last.map_or(0.0, |r| r.height());
    let dy = -(height + EDGE) * hidden;
    let inner_width = width - 2.0 * (PADDING + theme::BORDER);
    let mut search_rect = None;
    let mut modes_rect = None;
    let card = Area::new(id)
        .order(Order::Middle)
        .fixed_pos(Pos2::new(left, top + dy))
        .fade_in(false)
        .constrain(false)
        .show(ctx, |ui| {
            ui.multiply_opacity(1.0 - hidden);
            theme::surface().inner_margin(Margin::same(PADDING as i8)).show(ui, |ui| {
                ui.set_width(inner_width);
                ui.spacing_mut().item_spacing = Vec2::new(PADDING, PADDING);
                let row = super::widgets::text_field::SEARCH_HEIGHT.max(modebar::PHONE_CELL.y);
                // the mode buttons at the row's right end, the search in what they leave
                let anchor = last.unwrap_or_else(|| Rect::from_min_size(Pos2::new(left, top), Vec2::new(width, row + 2.0 * (PADDING + theme::BORDER))));
                ui.allocate_ui_with_layout(Vec2::new(inner_width, row), EguiLayout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    let modes = modebar::row(ui, &Bar::of(pass.state), modebar::Layout::Phone, pass.actions);
                    modes_rect = Some(modes);
                    let rest = ui.available_rect_before_wrap();
                    let field = Rect::from_min_max(rest.min, Pos2::new(modes.left() - PADDING, rest.max.y));
                    // the search box's slot filled: its phone form
                    if slots.search.is_some() {
                        let mut child = ui.new_child(UiBuilder::new().id_salt("rustradar::phone::search").max_rect(field).layout(EguiLayout::top_down(Align::Min)));
                        let shown = search::search_box(&mut child, pass, search::Layout { phone: true, anchor: Some(anchor) });
                        search_rect = Some(shown.field);
                    }
                });
                if let Some(notice) = sidebar::notice(pass.state, pass.now_ms) {
                    alert(ui, pass, notice);
                }
            });
        })
        .response
        .rect;
    ctx.data_mut(|d| d.insert_temp(id.with("rect"), card));
    // above the credits (z 3 over z 2), under the sheet, which follows it (z 4)
    ctx.memory_mut(|m| m.areas_mut().move_to_top(LayerId::new(Order::Middle, id)));
    placed.top_bar = Some(card);
    placed.search = search_rect;
    placed.modes = modes_rect;
}

/// The aircraft feed's alert or note, a row of its own (`flex-basis: 100%`).
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
            text::label(ui, text::wrapped(&text, &sidebar::note_style(), width), Sense::hover());
        }
    }
    // the countdown ticks every second
    ui.ctx().request_repaint_after_secs(1.0);
}

// ------------------------------------------------------------------------------------------------
// the sheet's content

/// The counts' number on a phone (`.counts .count`: 14 px, weight 650, `--text-1`).
pub const fn count_style() -> Style {
    Style::ui(theme::TEXT_BASE, theme::WEIGHT_BADGE, theme::TEXT_1)
}

/// The counts' noun for `mode`, shown on phones (`.noun`).
pub fn noun(mode: Mode) -> &'static str {
    match mode {
        Mode::Aircraft => "flights",
        Mode::Trains => "trains",
        Mode::Ships => "ships",
    }
}

/// The words after the number on a phone, as the page's white space collapses them: ` of 5,678
/// flights in view`, ` trains in view`.
pub fn counts_rest(counts: &Counts, mode: Mode) -> String {
    let of = if counts.listed != counts.total { format!(" of {}", group_thousands(counts.total as f64)) } else { String::new() };
    format!("{of} {} {}", noun(mode), counts.where_)
}

/// The sheet's head: the toolbar turned round, the counts on the left and the Filters button on the
/// right, centred on the row.
fn toolbar(ui: &mut Ui, pass: &mut Pass<'_>) {
    let state = pass.state;
    let open = state.shortcuts.filters_open();
    let n = sidebar::active_filters(state);
    let filters = Button::new().icon(button::icon(Icon::Funnel)).text("Filters").count(n as u32).pressed(open).tooltip("Filters (F)");
    let size = filters.size(ui.ctx());
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), size.y), Sense::hover());
    let at = Rect::from_min_size(Pos2::new(row.right() - size.x, row.top()), size);
    if panel::put(ui, "rustradar::phone::filters", at, filters).clicked() {
        pass.actions.push(Action::ToggleOpen(Open::Filters));
    }
    let mode = state.persisted.settings.mode;
    let Some(counts) = sidebar::counts(state, pass.now_ms, pass.query()) else { return };
    let mut job = text::job(&group_thousands(counts.listed as f64), &count_style());
    text::append(&mut job, &counts_rest(&counts, mode), &sidebar::counts_style());
    let galley = ui.painter().layout_job(text::tabular(job));
    let rect = Align2::LEFT_CENTER.align_size_within_rect(galley.size(), Rect::from_min_max(row.min, Pos2::new(at.left(), row.bottom())));
    ui.painter().galley(rect.min, galley, theme::TEXT_2);
}

/// Floating scroll bars with no fade at the edges, as the panels have them.
fn floating_scroll(ui: &mut Ui) {
    let scroll = &mut ui.spacing_mut().scroll;
    scroll.floating = true;
    scroll.fade.strength = 0.0;
}

fn filters_height_id() -> Id {
    Id::unique("rustradar::phone::filters_height")
}

/// The list view in the sheet's body: the filter panel while open, then the list, scrolling.
fn list_view(ui: &mut Ui, pass: &mut Pass<'_>, slots: &Slots) {
    let mode = pass.state.persisted.settings.mode;
    let filters = if pass.state.shortcuts.filters_open() { slots.filters(mode) } else { None };
    let list = slots.list(mode);
    let width = ui.available_width();
    let height = ui.max_rect().height();
    floating_scroll(ui);
    ScrollArea::vertical().id_salt("rustradar::phone::list").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        if let Some(slot) = filters {
            // its own card at its natural height (`flex: none`), 8 px in and 8 above the list
            let natural: f32 = ui.ctx().data(|d| d.get_temp(filters_height_id())).unwrap_or(0.0);
            let card_width = width - 2.0 * FILTERS_MARGIN;
            let top = ui.cursor().top();
            let rect = Rect::from_min_size(Pos2::new(ui.max_rect().left() + FILTERS_MARGIN, top), Vec2::new(card_width, natural + 2.0 * theme::BORDER));
            let mut measured = natural;
            let mut child = ui.new_child(UiBuilder::new().id_salt("rustradar::phone::filters").max_rect(rect).layout(EguiLayout::top_down(Align::Min)));
            theme::surface().show(&mut child, |ui| {
                ui.set_width(card_width - 2.0 * theme::BORDER);
                ui.set_max_height(natural);
                let start = ui.cursor().min;
                measured = slot(ui, pass);
                ui.expand_to_include_rect(Rect::from_min_size(start, Vec2::new(0.0, natural)));
            });
            ui.ctx().data_mut(|d| d.insert_temp(filters_height_id(), measured));
            ui.allocate_exact_size(Vec2::new(width, rect.height() + FILTERS_MARGIN), Sense::hover());
        }
        if let Some(slot) = list {
            // no card: the list's padding alone
            egui::Frame::new().inner_margin(Margin::symmetric(LIST_PADDING.x as i8, LIST_PADDING.y as i8)).show(ui, |ui| {
                ui.set_width(width - 2.0 * LIST_PADDING.x);
                slot(ui, pass);
            });
        }
    });
}

/// The open detail panel filling the sheet's body (the panels scroll themselves and mark their
/// summary).
fn panels(ui: &mut Ui, pass: &mut Pass<'_>, slots: &Slots) {
    let body = ui.max_rect();
    for kind in shell::panels(pass.state) {
        let slot = match kind {
            Panel::Airport => slots.airport_panel,
            Panel::Flight => slots.flight_panel,
            Panel::Ship => slots.ship_panel,
            Panel::Train => slots.train_panel,
        };
        if let Some(slot) = slot {
            let mut child = ui.new_child(UiBuilder::new().id_salt(("rustradar::phone::panel", kind)).max_rect(body).layout(EguiLayout::top_down(Align::Min)));
            child.set_width(body.width());
            child.set_max_height(body.height());
            slot(&mut child, pass);
        }
    }
}

// ------------------------------------------------------------------------------------------------
// the map controls' rail

fn rail_id() -> Id {
    Id::unique("rustradar::phone::rail")
}

/// An 18 px icon at stroke 1.75, as the rail's buttons draw it.
fn rail_icon(icon: Icon) -> icons::Spec {
    icon.at(map_controls::ICON, icons::STROKE_THIN)
}

/// A `.stack.surface` of 32 px buttons with 2 px of padding.
fn stack(ui: &mut Ui, add: impl FnOnce(&mut Ui)) -> Rect {
    theme::surface()
        .inner_margin(map_controls::STACK_PADDING)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.set_width(icon_button::SIZE);
            add(ui);
        })
        .response
        .rect
}

/// The rail above a sheet `sheet_height` high, and the open menu's own sheet.
fn rail(ctx: &Context, layout: Layout, state: &AppState, actions: &mut Vec<Action>, sheet_height: f32, placed: &mut Placed) {
    let id = rail_id();
    let i = layout.inset();
    let right = layout.viewport.right() - RAIL_EDGE.max(i.right);
    let target = if layout.landscape { RAIL_ABOVE } else { sheet_height + RAIL_ABOVE };
    let bottom = anim::value(ctx, id.with("bottom"), target, anim::CONTROLS_SHIFT_PHONE);
    let menu = state.shortcuts.menu();
    let view = state.controller.view();
    let mut stacks = [None; 2];
    let column = Area::new(id)
        .order(Order::Middle)
        .pivot(Align2::RIGHT_BOTTOM)
        .fixed_pos(Pos2::new(right, layout.viewport.bottom() - bottom))
        .fade_in(false)
        .constrain(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(0.0, map_controls::GAP);
            stacks[0] = Some(stack(ui, |ui| {
                for (which, glyph, name) in [(Menu::Layers, Icon::Layers, "Map layers"), (Menu::Settings, Icon::Settings, "Settings")] {
                    if IconButton::new(rail_icon(glyph), name).pressed(menu == Some(which)).tooltip(name).show(ui).clicked() {
                        actions.push(Action::SetMenu(toggled(menu, which)));
                    }
                }
            }));
            stacks[1] = Some(stack(ui, |ui| {
                let tilted = tilted(view.pitch);
                let (name, tip) = tilt_label(tilted);
                if IconButton::new(rail_icon(Icon::Rotate3d), name).pressed(tilted).tooltip(tip).show(ui).clicked() {
                    actions.push(Action::SetPitch(if tilted { 0.0 } else { map_controls::TILT }));
                }
                // the compass only while it can do something
                if rotated(view.bearing, view.pitch) {
                    let turn = anim::value(ui.ctx(), id.with("compass"), -view.bearing as f32, anim::COMPASS);
                    let compass = IconButton::new(rail_icon(Icon::Navigation2).rotated(turn.to_radians()), "Reset to north up").tooltip("Reset to north up (N)").icon_color(theme::AMBER);
                    if compass.show(ui).clicked() {
                        actions.push(Action::ResetOrientation);
                    }
                }
            }));
        })
        .response
        .rect;
    let popover = menu.map(|menu| menu_sheet(ctx, layout, state, menu, actions));
    // a press anywhere outside the rail and the menu closes it
    if menu.is_some()
        && let Some(at) = ctx.input(|i| if i.pointer.any_pressed() { i.pointer.press_origin() } else { None })
        && !column.contains(at)
        && !popover.is_some_and(|p| p.contains(at))
    {
        actions.push(Action::SetMenu(None));
    }
    placed.rail = Some(column);
    placed.stacks = stacks;
    placed.menu = popover;
}

/// The open menu as a sheet of its own along the bottom (see the module documentation).
fn menu_sheet(ctx: &Context, layout: Layout, state: &AppState, menu: Menu, actions: &mut Vec<Action>) -> Rect {
    let id = Id::unique(("rustradar::phone::menu", menu as u8));
    let t = anim::enter(ctx, id, anim::POPOVER_ENTER);
    let dy = anim::POPOVER_ENTER_DY_PHONE * (1.0 - t);
    let i = layout.inset();
    let vp = layout.viewport;
    let pad_left = MENU_PADDING.max(i.left);
    let pad_right = MENU_PADDING.max(i.right);
    let pad_bottom = MENU_PADDING + i.bottom;
    let width = vp.width() - 2.0 * theme::BORDER - pad_left - pad_right;
    // (whole pixels: egui puts the area's corner on the pixel grid, the box's bottom stays on the
    // window's)
    let max_height = (vp.height() * MENU_HEIGHT).max(0.0).floor();
    Area::new(id)
        .order(Order::Foreground)
        .pivot(Align2::LEFT_BOTTOM)
        .fixed_pos(Pos2::new(vp.left(), vp.bottom() + dy))
        .fade_in(false)
        // over the home bar too: clipped to the whole window
        .constrain_to(vp)
        .constrain(false)
        .show(ctx, |ui| {
            ui.multiply_opacity(t);
            let margin = egui::epaint::MarginF32 { left: pad_left, right: pad_right, top: MENU_PADDING, bottom: pad_bottom };
            egui::Frame::new()
                .fill(theme::SURFACE)
                .stroke(Stroke::new(theme::BORDER, theme::LINE))
                .corner_radius(CornerRadius { nw: MENU_RADIUS, ne: MENU_RADIUS, sw: 0, se: 0 })
                .inner_margin(margin)
                .show(ui, |ui| {
                    ui.set_width(width);
                    // an area lays its content out in the last pass's rectangle: this pass's room
                    ui.set_max_height(max_height);
                    ScrollArea::vertical().id_salt(id).max_height(max_height).auto_shrink([false, true]).show(ui, |ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing = Vec2::ZERO;
                        let top = ui.min_rect().top();
                        match menu {
                            Menu::Layers => menus::layers(ui, state, actions),
                            Menu::Settings => menus::settings(ui, state, actions, true),
                        }
                        let height = ui.min_rect().bottom() - top;
                        ui.expand_to_include_y(top + height.ceil());
                    });
                });
        })
        .response
        .rect
}

#[cfg(test)]
mod tests {
    use egui::{Event, RawInput, SafeAreaInsets};
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::state::Startup;

    /// 2026-06-21 10:00 UTC.
    const NOW_MS: f64 = 1_782_036_000_000.0;

    #[test]
    fn the_phone_query() {
        assert!(is_phone(Vec2::new(390.0, 844.0)));
        assert!(is_phone(Vec2::new(899.0, 1200.0)));
        assert!(!is_phone(Vec2::new(900.0, 600.0)), "the desktop's smallest window");
        assert!(is_phone(Vec2::new(1400.0, 500.0)), "short as a phone on its side");
        assert!(!is_landscape(Vec2::new(390.0, 844.0)));
        assert!(is_landscape(Vec2::new(844.0, 390.0)));
        assert!(!is_landscape(Vec2::new(480.0, 500.0)) && !is_landscape(Vec2::new(400.0, 400.0)), "not wider than high");
    }

    #[test]
    fn escape_and_back_lower_a_raised_sheet_last_on_a_phone() {
        let mut s = state([390.0, 844.0]);
        assert!(!s.apply(Action::CloseTop, 0.0), "nothing to close");
        assert!(s.apply(Action::Sheet(Detent::Full), 0.0));
        assert!(s.closable());
        assert!(s.apply(Action::ToggleOpen(Open::Filters), 0.0));
        // the filter panel first, then the sheet, then nothing
        assert!(s.apply(Action::CloseTop, 0.0));
        assert!(!s.shortcuts.filters_open());
        assert_eq!(s.sheet.detent(), Detent::Full);
        assert!(s.apply(Action::CloseTop, 0.0));
        assert_eq!(s.sheet.detent(), Detent::Peek);
        assert!(!s.apply(Action::CloseTop, 0.0));
        // on the desktop the sheet is not there to lower
        let mut desk = state([1440.0, 900.0]);
        desk.apply(Action::Sheet(Detent::Half), 0.0);
        assert!(!desk.closable());
        assert!(!desk.apply(Action::CloseTop, 0.0));
    }

    #[test]
    fn the_counts_with_their_noun() {
        let c = Counts { listed: 1234, total: 5678, where_: "in view" };
        assert_eq!(counts_rest(&c, Mode::Aircraft), " of 5,678 flights in view");
        let c = Counts { listed: 12, total: 12, where_: "worldwide" };
        assert_eq!(counts_rest(&c, Mode::Aircraft), " flights worldwide");
        let c = Counts { listed: 3, total: 3, where_: "in view" };
        assert_eq!(counts_rest(&c, Mode::Ships), " ships in view");
    }

    fn state(size: [f64; 2]) -> AppState {
        AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, ViewState::INITIAL, size)
    }

    fn insets(top: f32, bottom: f32) -> SafeAreaInsets {
        SafeAreaInsets(egui::epaint::MarginF32 { left: 0.0, right: 0.0, top, bottom })
    }

    /// One pass of the phone layout at `size` with `insets` at egui time `time`.
    fn pass(ctx: &Context, s: &AppState, size: Vec2, insets: SafeAreaInsets, time: f64, events: Vec<Event>) -> (Option<Placed>, Vec<Action>) {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), time: Some(time), safe_area_insets: Some(insets), events, ..Default::default() };
        let mut actions = Vec::new();
        let mut placed = None;
        let mut out = ctx.run_ui(raw, |ui| placed = super::ui(ui.ctx(), s, NOW_MS, &mut actions));
        out.textures_delta.clear();
        (placed, actions)
    }

    #[test]
    fn the_desktop_draws_none_of_it() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let s = state([1440.0, 900.0]);
        let (placed, _) = pass(&ctx, &s, Vec2::new(1440.0, 900.0), insets(0.0, 0.0), 0.0, vec![]);
        assert!(placed.is_none());
    }

    #[test]
    fn the_top_bar_the_sheet_and_the_rail_kept_clear_of_the_safe_areas() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let size = Vec2::new(390.0, 844.0);
        let s = state([390.0, 844.0]);
        let phone = insets(47.0, 34.0);
        let mut placed = None;
        for t in [0.0, 1.0, 2.0] {
            placed = pass(&ctx, &s, size, phone, t, vec![]).0;
        }
        let placed = placed.expect("the phone layout");
        // the card at max(8, 47) from the top, 8 from the sides
        let bar = placed.top_bar.expect("the top bar");
        assert_eq!((bar.left(), bar.top(), bar.right()), (8.0, 47.0, 382.0), "{bar:?}");
        // the search and the two mode cells (aircraft, ships) in one row, 4 apart
        let modes = placed.modes.expect("the modes");
        assert_eq!(modes.size(), Vec2::new(90.0, 36.0), "{modes:?}");
        assert_eq!(modes.right(), 382.0 - 5.0);
        let field = placed.search.expect("the field");
        assert_eq!((field.left(), field.right(), field.height()), (13.0, modes.left() - 4.0, 38.0), "{field:?}");
        assert_eq!(modes.center().y, field.center().y);
        // the sheet at peek: 90 of content, the home bar's 34 under it, its border
        let sheet = placed.sheet.expect("the sheet");
        assert_eq!(sheet.height, 90.0 + 34.0 + 1.0);
        assert_eq!(sheet.rect.top(), 844.0 - 125.0);
        // the rail 24 above it, 8 from the right: Layers and Settings, Tilt (no compass)
        let rail = placed.rail.expect("the rail");
        assert_eq!((rail.right(), rail.bottom()), (382.0, 844.0 - 125.0 - 24.0), "{rail:?}");
        let [a, c] = placed.stacks.map(|s| s.expect("a stack"));
        assert_eq!((a.height(), c.height()), (2.0 * 32.0 + 6.0, 32.0 + 6.0));
        assert_eq!(c.top() - a.bottom(), 8.0);
        // the credits on the sheet, across the window
        let credits = placed.credits.expect("the credits");
        assert_eq!((credits.left(), credits.right(), credits.bottom()), (0.0, 390.0, sheet.rect.top()), "{credits:?}");
    }

    #[test]
    fn a_detail_takes_the_sheet_and_the_bar_steps_aside() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let size = Vec2::new(390.0, 844.0);
        let mut s = state([390.0, 844.0]);
        let none = insets(0.0, 0.0);
        pass(&ctx, &s, size, none, 0.0, vec![]);
        s.airport.open("ARN", 0.0);
        // the board alone opens the sheet half up, before the state has followed
        let (placed, _) = pass(&ctx, &s, size, none, 1.0, vec![]);
        let placed = placed.expect("phone");
        assert!(placed.top_bar.is_some(), "sliding away");
        let (placed, _) = pass(&ctx, &s, size, none, 1.11, vec![]);
        let bar = placed.expect("phone").top_bar.expect("still sliding");
        assert!(bar.top() < 8.0, "{bar:?}");
        let (placed, _) = pass(&ctx, &s, size, none, 1.3, vec![]);
        let placed = placed.expect("phone");
        assert_eq!(placed.top_bar, None, "gone once it has slid away");
        assert_eq!(placed.sheet.expect("sheet").heights.half, 422.0);
        assert_eq!(placed.sheet.expect("sheet").content, 422.0, "half up");
        // its peek: the handle, the board's strip, tabs and footer
        let peek = placed.sheet.expect("sheet").heights.peek;
        assert!(peek > 22.0 + 60.0 + 40.0 + 50.0 && peek < 300.0, "{peek}");
        // closed: the bar comes back
        s.airport.close();
        let (placed, _) = pass(&ctx, &s, size, none, 2.0, vec![]);
        assert!(placed.expect("phone").top_bar.is_some());
    }

    #[test]
    fn a_menu_opens_as_a_sheet_and_a_press_outside_closes_it() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let size = Vec2::new(390.0, 844.0);
        let mut s = state([390.0, 844.0]);
        let phone = insets(47.0, 34.0);
        pass(&ctx, &s, size, phone, 0.0, vec![]);
        assert!(s.apply(Action::SetMenu(Some(Menu::Settings)), 0.0));
        let mut placed = None;
        for t in [1.0, 2.0, 3.0] {
            placed = pass(&ctx, &s, size, phone, t, vec![]).0;
        }
        let menu = placed.expect("phone").menu.expect("the menu's sheet");
        assert_eq!((menu.left(), menu.right(), menu.bottom()), (0.0, 390.0, 844.0), "{menu:?}");
        assert!(menu.height() <= 844.0 * 0.7 + 32.0 + 34.0 + 2.0 + 0.5, "{menu:?}");
        // a press on the map above it
        let at = Pos2::new(100.0, 200.0);
        let press = vec![Event::PointerMoved(at), Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }];
        let (_, actions) = pass(&ctx, &s, size, phone, 4.0, press);
        assert!(actions.contains(&Action::SetMenu(None)), "{actions:?}");
    }

    #[test]
    fn a_flight_s_summary_sets_the_peek_and_the_rail_follows_the_sheet() {
        use crate::state::flight::SelectOptions;
        use rustradar_core::model::LiveFlight;
        let ctx = Context::default();
        theme::apply(&ctx);
        let size = Vec2::new(390.0, 844.0);
        let mut s = state([390.0, 844.0]);
        let hint = LiveFlight { id: 7, callsign: "SAS1".into(), ..Default::default() };
        s.select_flight(Some(7), SelectOptions { hint: Some(hint), ..Default::default() }, 0.0);
        let none = insets(0.0, 0.0);
        let mut placed = None;
        for t in [0.0, 1.0, 2.0, 3.0] {
            s.poll(NOW_MS);
            placed = pass(&ctx, &s, size, none, t, vec![]).0;
        }
        let placed = placed.expect("phone");
        let sheet = placed.sheet.expect("the sheet");
        // the handle and the panel's strip, route and footer: more than the list's 90, less than half
        assert!(sheet.heights.peek > 200.0 && sheet.heights.peek < 422.0, "{:?}", sheet.heights);
        assert_eq!(sheet.content, sheet.heights.peek);
        assert_eq!(sheet.heights.full, 844.0 - sheet::CLEAR_DETAIL);
        assert_eq!(placed.top_bar, None, "the search steps aside");
        let rail = placed.rail.expect("the rail");
        assert_eq!(rail.bottom(), sheet.rect.top() - RAIL_ABOVE, "{rail:?} on {:?}", sheet.rect);
    }

    #[test]
    fn on_its_side_the_bar_and_the_sheet_run_down_the_left() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let size = Vec2::new(844.0, 390.0);
        let s = state([844.0, 390.0]);
        let side = SafeAreaInsets(egui::epaint::MarginF32 { left: 47.0, right: 47.0, top: 0.0, bottom: 21.0 });
        let mut placed = None;
        for t in [0.0, 1.0, 2.0] {
            placed = pass(&ctx, &s, size, side, t, vec![]).0;
        }
        let placed = placed.expect("phone");
        let bar = placed.top_bar.expect("bar");
        assert_eq!((bar.left(), bar.top(), bar.width()), (47.0, 8.0, 320.0));
        let sheet = placed.sheet.expect("sheet");
        assert_eq!((sheet.rect.left(), sheet.rect.width()), (47.0, 320.0));
        // the rail 24 from the bottom whatever the sheet, 47 from the right
        let rail = placed.rail.expect("rail");
        assert_eq!((rail.right(), rail.bottom()), (844.0 - 47.0, 390.0 - 24.0));
    }
}
