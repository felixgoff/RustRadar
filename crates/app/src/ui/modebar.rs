//! The mode bar (doc 06 §6.2, §2.3; `ModeBar.svelte`, `Segmented.svelte`, `+page.svelte:1416-1430`):
//! which kind of traffic the sidebar is for, and which others the map shows with it.
//!
//! **Place** ([`show`], desktop): centred between the sidebar and the detail panels (`.wrap` at top
//! 16, left and right 352 = 320 + 2 × 16, `justify-content: center`; only the bar itself takes the
//! pointer). The bar is a `.surface` (`--surface`, a 1 px `--line` border, radius 4) with 3 px of
//! padding round one row: today's Segmented with its own box taken off (`display: flex; padding: 0;
//! border: 0`, its 2 px gap kept).
//!
//! **Options**, in the page's order, those offered ([`Bar::offered`]: aircraft and ships always, the
//! trains while Trafikverket has accepted the key or while they are the focus): `Aircraft` (Plane,
//! key P), `Trains` (TrainFront, I), `Ships` (Ship, S). Each is the Lucide icon at 15 px (stroke
//! 1.75), the name and, while that kind is on the map and loaded, its count ([`count_text`]: Overpass
//! Mono 12 px at 500 in `--text-3`; in the chosen option the white at 0.75), 7 px apart, `6px 12px`
//! of padding, 13 px `--text-2`, radius 4; under the pointer `--surface-hover` (150 ms); the chosen
//! one `--amber` with white text at 650. A kind also shown on the map reads `--text-1`. Tooltip
//! `Aircraft (P)`. A click (or an arrow key on the focused option: a radio group, wrapping) makes it
//! the focus: [`Action::SetMode`].
//!
//! **Eyes**: after each option but the focus, a 24 px button with the Eye icon (14 px, stroke 1.75,
//! `--text-3`; under the pointer `--surface-hover` and `--text-1`; on: `--amber`), tooltip `Also show
//! trains on the map (Shift+I)`: [`Action::ToggleShown`]. It is folded away (width 0, opacity 0,
//! `margin-left: -2px`) until the bar is under the pointer or holds the keyboard focus, or while it
//! is on; then 24 px wide, `margin: 0 2px 0 -4px`, opaque, over 150 ms (`--ease-out`).
//!
//! **Phones** ([`Layout::Phone`], item U19 puts [`row`] in its top bar): no chrome, each option a
//! 44 × 36 cell with the icon alone (the name and count only for assistive technology).
//!
//! Deviations:
//! - The count's `tabular-nums`: Overpass Mono's figures are tabular already.
//! - Keyboard focus is egui's: only the keyboard moves it (no focus on a click), so it stands for
//!   `:focus-visible`.

use egui::{Area, Color32, Context, CornerRadius, CursorIcon, Galley, Id, LayerId, Order, Pos2, Rect, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};
use rustradar_core::settings::{MODES, Mode, ShownKinds, shown_kinds};
use rustradar_core::text::group_thousands;
use std::sync::Arc;

use super::icons::{self, Icon};
use super::text::{self, Style};
use super::widgets::button::transition_color;
use super::widgets::segmented::{self, roving};
use super::widgets::tooltip;
use super::{anim, theme};
use crate::state::{Action, AppState};

/// The bar's top (`top: var(--space-4)`).
pub const TOP: f32 = theme::SPACE_4;
/// What the wrapper leaves on either side: the sidebar's 320 and twice 16 (`left`, `right`).
pub const CLEAR: f32 = 320.0 + 2.0 * theme::SPACE_4;
/// The bar's padding inside its border (`padding: 3px`).
pub const PADDING: f32 = 3.0;
/// Between the row's items (the options box's `gap: 2px`).
pub const GAP: f32 = segmented::GAP;
/// An option's padding: across, down (`padding: 6px var(--space-3)`).
pub const OPTION_PADDING: Vec2 = Vec2::new(theme::SPACE_3, 6.0);
/// Between an option's icon, name and count (`gap: 7px`).
pub const OPTION_GAP: f32 = 7.0;
/// The option's icon (`size={15} strokeWidth={1.75}`).
pub const ICON_SIZE: f32 = 15.0;
/// The count's size and weight.
pub const COUNT_SIZE: f32 = theme::TEXT_SM;
pub const COUNT_WEIGHT: u16 = 500;
/// The count's opacity in the chosen option.
pub const COUNT_OPACITY_CHOSEN: f32 = 0.75;
/// The chosen option's weight (Segmented's `font-weight: 650`).
pub const CHOSEN_WEIGHT: u16 = 650;
/// The eye's icon (`size={14} strokeWidth={1.75}`).
pub const EYE_ICON_SIZE: f32 = 14.0;
/// The eye's width unfolded.
pub const EYE_WIDTH: f32 = 24.0;
/// Its left margin folded and unfolded, and its right margin unfolded (0 folded).
pub const EYE_MARGIN_LEFT_FOLDED: f32 = -2.0;
pub const EYE_MARGIN_LEFT: f32 = -4.0;
pub const EYE_MARGIN_RIGHT: f32 = 2.0;
/// The eye's focus outline sits inside it (`outline-offset: -2px`).
pub const EYE_FOCUS_OFFSET: f32 = -2.0;
/// A phone's option cell (`width: 44px; height: 36px`).
pub const PHONE_CELL: Vec2 = Vec2::new(44.0, 36.0);

/// An option: its kind, name and key (`MODES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    pub mode: Mode,
    pub label: &'static str,
    pub key: char,
}

/// The options in the page's order.
pub const CHOICES: [Choice; 3] = [
    Choice { mode: Mode::Aircraft, label: "Aircraft", key: 'P' },
    Choice { mode: Mode::Trains, label: "Trains", key: 'I' },
    Choice { mode: Mode::Ships, label: "Ships", key: 'S' },
];

impl Choice {
    /// Its icon (`ICONS`).
    pub const fn icon(self) -> Icon {
        match self.mode {
            Mode::Aircraft => Icon::Plane,
            Mode::Trains => Icon::TrainFront,
            Mode::Ships => Icon::Ship,
        }
    }

    /// The option's tooltip: `Aircraft (P)`.
    pub fn tooltip(self) -> String {
        format!("{} ({})", self.label, self.key)
    }

    /// The eye's name: `Also show aircraft on the map`.
    pub fn eye_label(self) -> String {
        format!("Also show {} on the map", self.label.to_lowercase())
    }

    /// The eye's tooltip: `Also show aircraft on the map (Shift+P)`.
    pub fn eye_tooltip(self) -> String {
        format!("{} (Shift+{})", self.eye_label(), self.key)
    }
}

/// A count as the bar writes it (`Intl.NumberFormat("en-GB", {useGrouping: "min2"})`): digits
/// grouped only from five up (`1443`, `12,345`).
pub fn count_text(n: usize) -> String {
    if n >= 10_000 { group_thousands(n as f64) } else { n.to_string() }
}

/// What the bar shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    /// The focus (`settings.mode`).
    pub mode: Mode,
    /// The kinds on the map: the focus and those also shown (`shown`).
    pub shown: ShownKinds,
    /// The options offered (`offered`).
    pub offered: ShownKinds,
    /// How many of each kind are loaded, by [`MODES`]; `None` while that kind is off the map or has
    /// not loaded (`counts`).
    pub counts: [Option<usize>; 3],
}

impl Bar {
    /// The bar for `state`: aircraft counted once a snapshot is in (`flights.length`), trains and
    /// ships once their first answer since they came on is (`trains.length`, `ships.length`).
    pub fn of(state: &AppState) -> Bar {
        let settings = &state.persisted.settings;
        let shown = shown_kinds(settings.mode, &settings.also_show);
        let counts = [
            (shown.aircraft && state.aircraft.snapshot().is_some()).then(|| state.aircraft.flights().len()),
            (shown.trains && state.trains.loaded()).then(|| state.trains.trains().len()),
            (shown.ships && state.ships.loaded()).then(|| state.ships.ships().len()),
        ];
        Bar { mode: settings.mode, shown, offered: state.offered_modes(), counts }
    }

    /// The options offered, in order.
    pub fn choices(&self) -> impl Iterator<Item = Choice> + '_ {
        CHOICES.into_iter().filter(|c| self.offered.shows(c.mode))
    }

    fn count(&self, mode: Mode) -> Option<usize> {
        self.counts[MODES.iter().position(|m| *m == mode).unwrap_or(0)]
    }
}

/// The desktop's bar with names and counts, or a phone's icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Desktop,
    Phone,
}

/// One item of the row: an option or an eye.
#[derive(Debug, Clone)]
enum Part {
    Option { choice: Choice, name: Option<Arc<Galley>>, count: Option<Arc<Galley>> },
    Eye { choice: Choice, reveal: f32 },
}

/// An item and its place across the row.
#[derive(Debug, Clone)]
struct Item {
    part: Part,
    x: f32,
    w: f32,
}

/// The row laid out.
#[derive(Debug, Clone)]
struct Row {
    items: Vec<Item>,
    size: Vec2,
}

/// The ids of the bar's parts under `id`.
fn option_id(id: Id, mode: Mode) -> Id {
    id.with(("option", mode))
}

fn eye_id(id: Id, mode: Mode) -> Id {
    id.with(("eye", mode))
}

fn zone_id(id: Id) -> Id {
    id.with("zone")
}

/// The option's name and count laid out in their colours.
fn texts(ctx: &Context, bar: &Bar, choice: Choice) -> (Arc<Galley>, Option<Arc<Galley>>) {
    let chosen = choice.mode == bar.mode;
    let (weight, color) = if chosen {
        (CHOSEN_WEIGHT, theme::AMBER_INK)
    } else if bar.shown.shows(choice.mode) {
        (theme::WEIGHT_REGULAR, theme::TEXT_1)
    } else {
        (theme::WEIGHT_REGULAR, theme::TEXT_2)
    };
    let name = ctx.fonts_mut(|f| f.layout_job(text::job(choice.label, &Style::ui(theme::TEXT_MD, weight, color))));
    let count_color = if chosen { theme::AMBER_INK.gamma_multiply(COUNT_OPACITY_CHOSEN) } else { theme::TEXT_3 };
    let count = bar.count(choice.mode).map(|n| ctx.fonts_mut(|f| f.layout_job(text::job(&count_text(n), &Style::data(COUNT_SIZE, COUNT_WEIGHT, count_color)))));
    (name, count)
}

/// Lays the row out for `bar`: the options' widths from their text, the eyes as far unfolded as
/// they are now. `layer` is where the row is drawn: the pointer over the bar's zone there (as last
/// drawn) or the keyboard in the bar unfolds the eyes.
fn lay(ctx: &Context, layer: LayerId, bar: &Bar, layout: Layout, id: Id) -> Row {
    let zone: Option<Rect> = ctx.data(|d| d.get_temp(zone_id(id)));
    let pointed = zone.is_some_and(|r| ctx.rect_contains_pointer(layer, r));
    let focused = ctx.memory(|m| m.focused());
    let keyboard = focused.is_some_and(|f| CHOICES.iter().any(|c| f == option_id(id, c.mode) || f == eye_id(id, c.mode)));
    let height = match layout {
        Layout::Desktop => OPTION_PADDING.y * 2.0 + Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_2).line_px(),
        Layout::Phone => PHONE_CELL.y,
    };
    let mut items = Vec::new();
    let mut x = 0.0;
    for choice in bar.choices() {
        let (name, count, w) = match layout {
            Layout::Desktop => {
                let (name, count) = texts(ctx, bar, choice);
                let w = OPTION_PADDING.x * 2.0 + ICON_SIZE + OPTION_GAP + name.size().x + count.as_ref().map_or(0.0, |c| OPTION_GAP + c.size().x);
                (Some(name), count, w)
            }
            Layout::Phone => (None, None, PHONE_CELL.x),
        };
        if !items.is_empty() {
            x += GAP;
        }
        items.push(Item { part: Part::Option { choice, name, count }, x, w });
        x += w;
        // the focus is always on the map: no eye of its own
        if choice.mode != bar.mode {
            let on = bar.shown.shows(choice.mode);
            let reveal = anim::toggle(ctx, eye_id(id, choice.mode).with("reveal"), pointed || keyboard || on, anim::EYE_REVEAL);
            let margin_left = EYE_MARGIN_LEFT_FOLDED + (EYE_MARGIN_LEFT - EYE_MARGIN_LEFT_FOLDED) * reveal;
            x += GAP + margin_left;
            let w = EYE_WIDTH * reveal;
            items.push(Item { part: Part::Eye { choice, reveal }, x, w });
            x += w + EYE_MARGIN_RIGHT * reveal;
        }
    }
    Row { items, size: Vec2::new(x, height) }
}

/// Draws `row` with its top left at `origin` in `ui`, taking the clicks and keys into `actions`.
fn draw(ui: &mut Ui, origin: Pos2, row: &Row, bar: &Bar, id: Id, actions: &mut Vec<Action>) {
    let height = row.size.y;
    let painter = ui.painter().clone();
    let offered: Vec<Choice> = bar.choices().collect();
    for item in &row.items {
        let rect = Rect::from_min_size(origin + Vec2::new(item.x, 0.0), Vec2::new(item.w, height));
        match &item.part {
            Part::Option { choice, name, count } => {
                let chosen = choice.mode == bar.mode;
                let oid = option_id(id, choice.mode);
                // a radio group: only the chosen option is in the tab order
                let sense = if chosen { Sense::click() } else { Sense::CLICK };
                let response = ui.interact(rect, oid, sense).on_hover_cursor(CursorIcon::PointingHand);
                response.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, chosen, choice.label));
                let response = tooltip::show(response, &choice.tooltip());
                let mut look = segmented::look(ui, oid, chosen, response.hovered(), false);
                if !chosen && bar.shown.shows(choice.mode) {
                    look.text = theme::TEXT_1;
                }
                segmented::paint_option(&painter, rect, &look);
                paint_option_content(&painter, rect, choice.icon(), look.text, name.as_ref(), count.as_ref());
                if response.clicked() && !chosen {
                    actions.push(Action::SetMode(choice.mode));
                }
                if chosen {
                    let step = roving(ui, &response, true);
                    if response.has_focus() {
                        segmented::paint_focus(&painter, rect, f32::from(theme::RADIUS), segmented::FOCUS_OFFSET);
                    }
                    if step != 0
                        && let Some(at) = offered.iter().position(|c| c.mode == choice.mode)
                    {
                        let next = offered[(at as i64 + i64::from(step)).rem_euclid(offered.len() as i64) as usize];
                        actions.push(Action::SetMode(next.mode));
                        ui.memory_mut(|m| m.request_focus(option_id(id, next.mode)));
                    }
                }
            }
            Part::Eye { choice, reveal } => {
                let on = bar.shown.shows(choice.mode);
                let eid = eye_id(id, choice.mode);
                // folded, it still takes the keyboard (Tab unfolds the eyes)
                let response = ui.interact(rect, eid, Sense::click());
                response.widget_info(|| WidgetInfo::selected(Role::Button, true, on, choice.eye_label()));
                let response = if *reveal > 0.0 {
                    tooltip::show(response.on_hover_cursor(CursorIcon::PointingHand), &choice.eye_tooltip())
                } else {
                    response
                };
                let hovered = response.hovered();
                let fill = transition_color(ui.ctx(), eid.with("fill"), if hovered { theme::SURFACE_HOVER } else { Color32::TRANSPARENT }, anim::HOVER);
                // `.show.on` comes after `.show:hover`
                let color = if on {
                    theme::AMBER
                } else if hovered {
                    theme::TEXT_1
                } else {
                    theme::TEXT_3
                };
                let mut eye = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
                eye.multiply_opacity(*reveal);
                if fill != Color32::TRANSPARENT {
                    eye.rect_filled(rect, CornerRadius::same(theme::RADIUS_SM), fill);
                }
                let spec = Icon::Eye.at(EYE_ICON_SIZE, icons::STROKE_THIN);
                let icon = Rect::from_center_size(Pos2::new(rect.left() + EYE_WIDTH * reveal / 2.0, rect.center().y), spec.size_vec());
                icons::paint(&eye, icon, spec, color);
                if response.has_focus() {
                    let mut ring = painter.clone();
                    ring.multiply_opacity(*reveal);
                    segmented::paint_focus(&ring, rect, f32::from(theme::RADIUS_SM), EYE_FOCUS_OFFSET);
                }
                if response.clicked() {
                    actions.push(Action::ToggleShown(choice.mode));
                }
            }
        }
    }
}

/// An option's icon, then its name and count 7 px apart, from 12 px in (the desktop), or the icon
/// alone in the middle (a phone).
fn paint_option_content(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32, name: Option<&Arc<Galley>>, count: Option<&Arc<Galley>>) {
    let spec = icon.at(ICON_SIZE, icons::STROKE_THIN);
    let y = rect.center().y;
    let Some(name) = name else {
        icons::paint(painter, Rect::from_center_size(rect.center(), spec.size_vec()), spec, color);
        return;
    };
    let mut x = rect.left() + OPTION_PADDING.x;
    icons::paint(painter, Rect::from_min_size(Pos2::new(x, y - ICON_SIZE / 2.0), spec.size_vec()), spec, color);
    x += ICON_SIZE + OPTION_GAP;
    painter.galley(Pos2::new(x, y - name.size().y / 2.0), name.clone(), Color32::PLACEHOLDER);
    x += name.size().x;
    if let Some(count) = count {
        x += OPTION_GAP;
        painter.galley(Pos2::new(x, y - count.size().y / 2.0), count.clone(), Color32::PLACEHOLDER);
    }
}

/// The bar's row in `ui` at its cursor (a phone's top bar, item U19): answers its rectangle. The
/// eyes unfold while the pointer is over the row.
pub fn row(ui: &mut Ui, bar: &Bar, layout: Layout, actions: &mut Vec<Action>) -> Rect {
    let id = ui.make_persistent_id("rustradar::modebar::row");
    let laid = lay(ui.ctx(), ui.layer_id(), bar, layout, id);
    let (rect, _) = ui.allocate_exact_size(laid.size, Sense::hover());
    draw(ui, rect.min, &laid, bar, id, actions);
    ui.ctx().data_mut(|d| d.insert_temp(zone_id(id), rect));
    rect
}

/// The desktop's bar for `bar`: its rectangle, the surface's outer edge.
pub fn show(ctx: &Context, bar: &Bar, actions: &mut Vec<Action>) -> Rect {
    let area = Id::unique("rustradar::modebar");
    let id = area.with("row");
    let laid = lay(ctx, LayerId::new(Order::Middle, area), bar, Layout::Desktop, id);
    let inset = theme::BORDER + PADDING;
    let size = laid.size + Vec2::splat(2.0 * inset);
    let screen = ctx.content_rect();
    // `justify-content: center` in the wrapper: centred even where it overflows it
    let left = screen.left() + CLEAR + (screen.width() - 2.0 * CLEAR - size.x) / 2.0;
    let rect = Rect::from_min_size(Pos2::new(left, screen.top() + TOP), size);
    Area::new(area).order(Order::Middle).fixed_pos(rect.min).fade_in(false).constrain(false).show(ctx, |ui| {
        let (surface, _) = ui.allocate_exact_size(size, Sense::hover());
        ui.painter().rect(surface, CornerRadius::same(theme::RADIUS), theme::SURFACE, Stroke::new(theme::BORDER, theme::LINE), StrokeKind::Inside);
        draw(ui, surface.min + Vec2::splat(inset), &laid, bar, id, actions);
    });
    // `.modebar:hover`: the surface, border and padding with it
    ctx.data_mut(|d| d.insert_temp(zone_id(id), rect));
    rect
}

/// The desktop's bar for `state`, its clicks and keys into `actions`.
pub fn ui(ctx: &Context, state: &AppState, actions: &mut Vec<Action>) -> Rect {
    show(ctx, &Bar::of(state), actions)
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::state::Startup;

    const W: f32 = 1440.0;

    fn bar(mode: Mode, also: &[Mode], trains: bool) -> Bar {
        Bar {
            mode,
            shown: shown_kinds(mode, also),
            offered: crate::state::settings::offered(mode, trains),
            counts: [None; 3],
        }
    }

    /// One pass of `ctx` at `time` with `events` drawing the desktop bar for `b`: its rectangle and
    /// the actions.
    fn pass(ctx: &Context, time: f64, events: Vec<egui::Event>, b: &Bar) -> (Rect, Vec<Action>) {
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(W, 900.0))), time: Some(time), events, ..Default::default() };
        let mut rect = Rect::NOTHING;
        let mut actions = Vec::new();
        let mut output = ctx.run_ui(raw, |ui| rect = show(ui.ctx(), b, &mut actions));
        output.textures_delta.clear();
        (rect, actions)
    }

    fn context() -> Context {
        let ctx = Context::default();
        theme::apply(&ctx);
        ctx
    }

    fn click(at: Pos2) -> [egui::Event; 2] {
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        [press(true), press(false)]
    }

    #[test]
    fn counts_group_from_five_digits() {
        assert_eq!(count_text(0), "0");
        assert_eq!(count_text(1443), "1443");
        assert_eq!(count_text(9999), "9999");
        assert_eq!(count_text(12_345), "12,345");
        assert_eq!(count_text(1_234_567), "1,234,567");
    }

    #[test]
    fn the_options_tooltips_and_icons() {
        let names: Vec<String> = CHOICES.iter().map(|c| c.tooltip()).collect();
        assert_eq!(names, ["Aircraft (P)", "Trains (I)", "Ships (S)"]);
        assert_eq!(CHOICES[1].eye_tooltip(), "Also show trains on the map (Shift+I)");
        assert_eq!(CHOICES.map(Choice::icon), [Icon::Plane, Icon::TrainFront, Icon::Ship]);
        assert_eq!(CHOICES.map(|c| c.mode), MODES);
    }

    #[test]
    fn what_the_bar_offers_and_counts_from_the_state() {
        let mut s = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, ViewState::INITIAL, [1440.0, 900.0]);
        let b = Bar::of(&s);
        assert_eq!(b.mode, Mode::Aircraft);
        assert_eq!(b.offered, ShownKinds { aircraft: true, trains: false, ships: true }, "no key: no trains");
        assert_eq!(b.counts, [None; 3], "nothing loaded");
        assert_eq!(b.choices().map(|c| c.label).collect::<Vec<_>>(), ["Aircraft", "Ships"]);
        // the demo trains: offered, on the map and counted
        let mut demo = AppState::new(Startup { basemap: Some(Basemap::Dark), demo: true, ..Default::default() }, None, None, None, ViewState::INITIAL, [1440.0, 900.0]);
        for t in [1.0e12, 1.0e12 + 16.0, 1.0e12 + 32.0] {
            demo.poll(t);
        }
        let b = Bar::of(&demo);
        assert!(b.offered.trains && b.shown.trains);
        assert_eq!(b.counts[1], Some(demo.trains.trains().len()));
        assert_eq!(b.counts[2], None, "ships are off the map");
        // ships on: counted once their first answer is in
        assert!(s.apply(Action::ToggleShown(Mode::Ships), 0.0));
        assert!(Bar::of(&s).shown.ships);
    }

    #[test]
    fn today_s_bar_is_180_wide_centred_between_352_and_w_minus_352() {
        let ctx = context();
        let b = bar(Mode::Aircraft, &[], false);
        let (rect, _) = pass(&ctx, 0.0, vec![], &b);
        // dark-v1: x 630–810, y 16–54.2 (Aircraft 92.5 of accent, Ships 77.5, 2 apart, 3 + 1 round)
        assert!((rect.width() - 180.0).abs() < 2.0, "{rect:?}");
        assert!((rect.height() - 38.2).abs() < 0.01, "{rect:?}");
        assert_eq!(rect.top(), 16.0);
        assert!((rect.center().x - 720.0).abs() < 0.01, "{rect:?}");
        // a narrower window: still centred between the two
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1100.0, 700.0))), time: Some(1.0), ..Default::default() };
        let mut narrow = Rect::NOTHING;
        let _ = ctx.run_ui(raw, |ui| narrow = show(ui.ctx(), &b, &mut Vec::new()));
        assert!((narrow.center().x - 550.0).abs() < 0.01 && (narrow.left() - (352.0 + (1100.0 - 704.0 - narrow.width()) / 2.0)).abs() < 0.01, "{narrow:?}");
    }

    #[test]
    fn a_click_chooses_the_focus_and_the_eye_shows_another_kind() {
        let ctx = context();
        let b = bar(Mode::Aircraft, &[], true);
        let (rect, _) = pass(&ctx, 0.0, vec![], &b);
        pass(&ctx, 0.1, vec![], &b);
        // folded: the bar is three options and two gaps wide
        let folded = rect.width();
        // the pointer on the bar unfolds the eyes over 150 ms
        let inside = Pos2::new(rect.left() + 20.0, rect.center().y);
        pass(&ctx, 0.2, vec![egui::Event::PointerMoved(inside)], &b);
        let (open, _) = pass(&ctx, 0.5, vec![], &b);
        // two eyes: each 24 wide with −4 and +2 of margin where it took 0 with −2
        assert!((open.width() - folded - 2.0 * EYE_WIDTH).abs() < 0.01, "{folded} → {}", open.width());
        // Ships is the last option: its eye the last 26 px of the row, inside the padding
        let eye = Pos2::new(open.right() - 4.0 - 2.0 - 12.0, open.center().y);
        pass(&ctx, 0.6, vec![egui::Event::PointerMoved(eye)], &b);
        let mut actions = Vec::new();
        for (n, e) in click(eye).into_iter().enumerate() {
            actions.extend(pass(&ctx, 0.7 + 0.05 * n as f64, vec![e], &b).1);
        }
        assert_eq!(actions, [Action::ToggleShown(Mode::Ships)]);
        // a click on the Ships option itself makes it the focus
        let ships = Pos2::new(open.right() - 4.0 - 2.0 - 26.0 - 30.0, open.center().y);
        pass(&ctx, 0.9, vec![egui::Event::PointerMoved(ships)], &b);
        let mut actions = Vec::new();
        for (n, e) in click(ships).into_iter().enumerate() {
            actions.extend(pass(&ctx, 1.0 + 0.05 * n as f64, vec![e], &b).1);
        }
        assert_eq!(actions, [Action::SetMode(Mode::Ships)]);
        // the chosen option does nothing
        let aircraft = Pos2::new(open.left() + 20.0, open.center().y);
        pass(&ctx, 1.2, vec![egui::Event::PointerMoved(aircraft)], &b);
        let mut actions = Vec::new();
        for (n, e) in click(aircraft).into_iter().enumerate() {
            actions.extend(pass(&ctx, 1.3 + 0.05 * n as f64, vec![e], &b).1);
        }
        assert!(actions.is_empty(), "{actions:?}");
        // the pointer away: the eyes fold again, one that is on stays
        let shown = bar(Mode::Aircraft, &[Mode::Trains], true);
        pass(&ctx, 2.0, vec![egui::Event::PointerMoved(Pos2::new(700.0, 500.0))], &shown);
        let (after, _) = pass(&ctx, 2.5, vec![], &shown);
        assert!((after.width() - folded - EYE_WIDTH).abs() < 0.01, "the trains' eye stays: {}", after.width());
    }

    #[test]
    fn tab_reaches_the_chosen_option_and_the_arrows_choose() {
        let ctx = context();
        // the focus first in the row: the first in the tab order (the radio group's one stop)
        let b = bar(Mode::Aircraft, &[], true);
        let key = |key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        let (folded, _) = pass(&ctx, 0.0, vec![], &b);
        pass(&ctx, 0.1, vec![], &b);
        pass(&ctx, 0.2, vec![key(egui::Key::Tab)], &b);
        let focused = ctx.memory(|m| m.focused());
        assert_eq!(focused, Some(option_id(Id::unique("rustradar::modebar").with("row"), Mode::Aircraft)));
        // Left from the first wraps to the last
        let (_, actions) = pass(&ctx, 0.3, vec![key(egui::Key::ArrowLeft)], &b);
        assert_eq!(actions, [Action::SetMode(Mode::Ships)]);
        // the keyboard in the bar unfolds the eyes, with the pointer nowhere near
        let (rect, _) = pass(&ctx, 0.8, vec![], &b);
        assert!((rect.width() - folded.width() - 2.0 * EYE_WIDTH).abs() < 0.01, "{} {}", rect.width(), folded.width());
    }
}
