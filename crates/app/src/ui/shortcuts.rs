//! ShortcutsHelp (doc 06 §21; `ShortcutsHelp.svelte`): the keyboard shortcuts' list, and the modal
//! dialog frame it shares with the keys dialog ([`dialog`], `<dialog>` opened with `showModal()`).
//!
//! **The dialog frame** ([`dialog`], both dialogs):
//! - a backdrop over the whole window in `oklch(0.1 0 0 / 0.5)` ([`theme::BACKDROP`]) that takes
//!   every press, the map's included, and closes the dialog when clicked (the click lands on the
//!   `<dialog>` itself);
//! - the `.surface` card centred in the window (`inset-block: 0; margin: auto`), its content
//!   `width` wide but no wider than the window less 32 (`max-width: calc(100vw - 32px)`), at most
//!   `max_height` of content (the help: the UA's `calc(100% - 6px - 2em)` at 14 px; the keys
//!   dialog: `calc(100dvh - 32px)`), scrolling inside the border beyond that (`overflow: auto`);
//! - entering over 180 ms from 6 px lower and transparent (`animation: pop 180ms var(--ease-out)`),
//!   again whenever it opens; the backdrop comes at once;
//! - modal: egui's modal layer keeps the pointer and the keyboard focus inside it, and Escape
//!   closes it when nothing above it took the key;
//! - the focus comes back on closing to the widget that had it when the dialog opened ([`Return`]:
//!   the browser's "previously focused element" of a modal dialog, and the page's `closeKeys`).
//!
//! **The help** ([`ui`] while `state.shortcuts.help_open()`; opened by `?` and the map controls'
//! keyboard button, closed by its close button, the backdrop, Escape (the page's `closeTop`) and
//! `?` again): 360 wide, padding `12px 16px 16px`; the header (`Keyboard shortcuts` 14 px at 650,
//! the close button `X` 16/1.75 at its right, 32 high, 8 above the rows); then one row per
//! shortcut ([`rows`]): a 1 px `--line` rule on top, `5px 0` of padding, the keys in a 72 px
//! column 12 px before the description (13 px `--text-2`, wrapping), both centred on the row's
//! height. Each key is a cap ([`cap_width`]): at least 22 px of content plus `0 5px` of padding,
//! 22 high, a 1 px `--line-control` border with a 2 px bottom, radius 4, Overpass Mono 12 px at 600
//! in `--text-1` centred in it; caps 4 apart, overflowing the 72 px column where they need more
//! (today's `1` `2` `3` run 22 px into their description; kept).
//!
//! **Checked** against Chromium's layout of today's component (server-rendered by Svelte, opened
//! with `showModal()`, laid out with app.css at 1440 × 900 and 390 × 844; `tests/dialogs.rs`): the
//! backdrop, the card's edges, every rule and cap land on Chromium's pixels; text sits a pixel
//! lower (egui's baseline, as everywhere in the app).
//!
//! Deviations:
//! - The web inspector's row (`Ctrl Alt I`) is left out: the native app has no inspector (plan
//!   section 7). The dev panel's row (`Ctrl Shift D`) is shown in debug builds, as Vite's dev
//!   server shows it.
//! - Opening a dialog leaves the keyboard focus nowhere (until Tab or a click) where Chromium puts
//!   it on the first control (the close button).
//! - A description within a pixel of its column's width can wrap where Chromium's does not, or the
//!   other way (egui's advances differ from Chromium's by a fraction of a pixel a word): on a 390 px
//!   phone `Dark map, satellite imagery or realistic 3D` stays on one row, where Chromium wraps it.
//! - The scroll bar floats over the card's right padding (egui's floating bar, shown while the
//!   pointer is over the card), so the rows keep their width as Chromium lays them out without
//!   one; a classic scroll bar in the browser takes its own width from the rows.

use std::hash::Hash;

use egui::epaint::RectShape;
use egui::{Area, Color32, Context, CornerRadius, Frame, Id, Margin, Modal, Order, Pos2, Rect, Response, ScrollArea, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, UiKind, Vec2};

use super::icons::Icon;
use super::text::{self, Style};
use super::widgets::icon_button::{self, IconButton};
use super::{anim, theme};
use crate::state::shortcuts::Open;
use crate::state::{Action, AppState};

/// The dialog's heading.
pub const TITLE: &str = "Keyboard shortcuts";
/// The close button's accessible name.
pub const CLOSE: &str = "Close";

/// Its content's width (`width: 360px`).
pub const WIDTH: f32 = 360.0;
/// How much narrower than the window a dialog's content can be at most (`max-width: calc(100vw -
/// 32px)`).
pub const WINDOW_MARGIN: f32 = 32.0;
/// The UA's `dialog:modal { max-height: calc(100% - 6px - 2em) }` at the body's 14 px: what the
/// window's height leaves the help's content.
pub const UA_MAX_HEIGHT_LESS: f32 = 6.0 + 2.0 * theme::TEXT_BASE;
/// The content's padding (`padding: var(--space-3) var(--space-4) var(--space-4)`), shared with the
/// keys dialog.
pub const PAD_TOP: f32 = theme::SPACE_3;
pub const PAD_X: f32 = theme::SPACE_4;
pub const PAD_BOTTOM: f32 = theme::SPACE_4;
/// The header's height (the close button's) and the space under it (`margin-bottom`).
pub const HEADER_HEIGHT: f32 = icon_button::SIZE;
pub const HEADER_GAP: f32 = theme::SPACE_2;
/// The close button's icon size.
pub const CLOSE_ICON: f32 = 16.0;
/// A row: the keys' column, the gap after it, the padding above and below (`grid-template-columns:
/// 72px 1fr; gap: var(--space-3); padding: 5px 0`).
pub const KEYS_COLUMN: f32 = 72.0;
pub const COLUMN_GAP: f32 = theme::SPACE_3;
pub const ROW_PAD_Y: f32 = 5.0;
/// Between the caps of one row (`dt { gap: var(--space-1) }`).
pub const CAP_GAP: f32 = theme::SPACE_1;
/// A cap (`kbd`): its content's least width and height, its side padding, its borders and radius.
pub const CAP_MIN_WIDTH: f32 = 22.0;
pub const CAP_HEIGHT: f32 = 22.0;
pub const CAP_PAD_X: f32 = 5.0;
pub const CAP_BORDER_BOTTOM: f32 = 2.0;
pub const CAP_RADIUS: u8 = 4;

/// The dialog's title (`h2`: 14 px at 650), the keys' too.
pub const fn title_style() -> Style {
    Style::ui(theme::TEXT_BASE, theme::WEIGHT_BADGE, theme::TEXT_1)
}

/// A cap's key (`font: 600 var(--text-sm) var(--font-data)`: the shorthand resets the letter
/// spacing and the line height; `--text-1`).
pub const fn cap_style() -> Style {
    Style::data(theme::TEXT_SM, 600, theme::TEXT_1).letter_spacing(0.0)
}

/// A shortcut's description (`dd`: 13 px `--text-2`).
pub const fn description_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// The shortcuts, in today's order (`SHORTCUTS`), less the web inspector's row.
pub const ROWS: [(&[&str], &str); 22] = [
    (&["P"], "Aircraft mode"),
    (&["I"], "Trains mode"),
    (&["S"], "Ships mode"),
    (&["Shift+P"], "Also show aircraft on the map, or not"),
    (&["Shift+I"], "Also show trains on the map, or not"),
    (&["Shift+S"], "Also show ships on the map, or not"),
    (&["/"], "Search the mode's traffic, or a place"),
    (&["F"], "Show or hide filters"),
    (&["C"], "Follow the selected aircraft with the camera"),
    (&["Esc"], "Close the open panel"),
    (&["+", "\u{2212}"], "Zoom in or out"),
    (&["T"], "Tilt to 3D, or look straight down"),
    (&["N"], "North up"),
    (&["1", "2", "3"], "Dark map, satellite imagery or realistic 3D"),
    (&["E"], "Terrain"),
    (&["B"], "3D buildings"),
    (&["L"], "Place names"),
    (&["R"], "Rail map"),
    (&["K"], "Aircraft labels (with the aircraft on the map)"),
    (&["W"], "Weather radar"),
    (&["D"], "Day and night"),
    (&["?"], "This list"),
];

/// The dev builds' row: what the APIs answered (`import.meta.env.DEV`).
pub const DEV_ROW: (&[&str], &str) = (&["Ctrl", "Shift", "D"], "API calls and the selected train's data (dev builds, ⌘⇧D on a Mac)");

/// The rows this build shows: [`ROWS`], and [`DEV_ROW`] in debug builds.
pub fn rows() -> Vec<(&'static [&'static str], &'static str)> {
    let mut rows = ROWS.to_vec();
    if cfg!(debug_assertions) {
        rows.push(DEV_ROW);
    }
    rows
}

// ------------------------------------------------------------------------------------------------
// the modal dialog frame

/// What [`dialog`] did this pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shown {
    /// The card, border included (as laid out: on its first pass egui lays it out unseen).
    pub rect: Rect,
    /// The backdrop was clicked, or Escape pressed with the dialog on top: close it.
    pub close: bool,
}

/// A modal dialog `id` (see the module documentation): `add` draws the content (its padding
/// included) into a `.surface` card whose content is `width` wide at most (and no wider than the
/// window less 32) and `max_height` high at most, scrolling beyond.
///
/// Placed as Chromium places a centred `<dialog>`: the content from the card's exact (fractional)
/// top, the card's fill and border on whole pixels round it (egui would put the whole area on a
/// whole pixel, moving every edge inside it by the fraction).
pub fn dialog(ctx: &Context, id: Id, width: f32, max_height: f32, add: impl FnOnce(&mut Ui)) -> Shown {
    let screen = ctx.content_rect();
    let width = width.min(screen.width() - WINDOW_MARGIN).max(0.0);
    let max_height = max_height.max(0.0);
    let t = anim::enter(ctx, id, anim::DIALOG_ENTER);
    let dy = anim::DIALOG_ENTER_DY * (1.0 - t);
    // the card's height as the last pass laid it out (on the first, egui lays the area out unseen)
    let height_id = id.with("rustradar::dialog::height");
    let height = ctx.data(|d| d.get_temp::<f32>(height_id)).unwrap_or(0.0);
    let card_width = width + 2.0 * theme::BORDER;
    let left = (screen.left() + (screen.width() - card_width) / 2.0).round();
    let top = screen.top() + (screen.height() - height) / 2.0 + dy;
    let area = Area::new(id)
        .kind(UiKind::Modal)
        .sense(Sense::hover())
        .order(Order::Foreground)
        .interactable(true)
        .fixed_pos(Pos2::new(left, top.floor()))
        .fade_in(false)
        .constrain(false);
    let modal = Modal::new(id).area(area).backdrop_color(theme::BACKDROP).frame(Frame::NONE);
    let response = modal.show(ctx, |ui| {
        ui.multiply_opacity(t);
        let fill = ui.painter().add(Shape::Noop);
        let origin = Pos2::new(left, top);
        let inner = Rect::from_min_size(origin + Vec2::splat(theme::BORDER), Vec2::new(width, max_height));
        let mut content = ui.new_child(UiBuilder::new().max_rect(inner));
        content.set_width(width);
        // the area lays its content out in the last pass's rectangle: this pass's room
        content.set_max_height(max_height);
        // the bar floats over the padding, the rows keeping their width; no fade at the edges
        // (egui's hint of more, which CSS does not draw)
        let scroll = &mut content.spacing_mut().scroll;
        scroll.floating = true;
        scroll.fade.strength = 0.0;
        ScrollArea::vertical().id_salt(id.with("scroll")).max_height(max_height).auto_shrink([false, true]).show(&mut content, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            add(ui);
        });
        let height = content.min_rect().height().min(max_height) + 2.0 * theme::BORDER;
        ui.ctx().data_mut(|d| d.insert_temp(height_id, height));
        // the card's edges on whole pixels, as Chromium paints a box's background and border
        let card = Rect::from_min_max(Pos2::new(left, top.round()), Pos2::new(left + card_width, (top + height).round()));
        ui.painter().set(fill, RectShape::new(card, CornerRadius::same(theme::RADIUS), theme::SURFACE, Stroke::new(theme::BORDER, theme::LINE), StrokeKind::Inside));
        ui.allocate_rect(card.union(content.min_rect()), Sense::hover());
        card
    });
    Shown { rect: response.inner, close: response.should_close() }
}

/// Where the keyboard focus goes back to when a dialog closes (the widget that had it when the
/// dialog opened), kept by the dialog between passes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Return {
    open: bool,
    focus: Option<Id>,
}

impl Return {
    /// The dialog `key` is `open` this pass: on opening, takes the focus from the widget that has
    /// it (remembered); on closing, gives it back. Answers `Some(open)` when the dialog has just
    /// opened or closed, `None` when it is as it was.
    pub fn track(ctx: &Context, key: Id, open: bool) -> Option<bool> {
        let key = key.with("rustradar::dialog::return");
        let was: Return = ctx.data(|d| d.get_temp(key)).unwrap_or_default();
        if was.open == open {
            return None;
        }
        let mut now = Return { open, focus: None };
        if open {
            now.focus = ctx.memory(|m| m.focused());
            if let Some(id) = now.focus {
                ctx.memory_mut(|m| m.surrender_focus(id));
            }
        } else if let Some(id) = was.focus {
            ctx.memory_mut(|m| m.request_focus(id));
        }
        ctx.data_mut(|d| d.insert_temp(key, now));
        Some(open)
    }
}

/// The header both dialogs open with: the title at the left and the close button at the right,
/// [`HEADER_HEIGHT`] high across `ui`'s width. Answers the close button's response.
pub fn header(ui: &mut Ui, title: &str) -> Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), HEADER_HEIGHT), Sense::hover());
    let galley = ui.painter().layout_job(text::job(title, &title_style()));
    ui.painter().galley(Pos2::new(rect.left(), rect.center().y - galley.size().y / 2.0), galley, Color32::PLACEHOLDER);
    let close = Rect::from_min_size(Pos2::new(rect.right() - icon_button::SIZE, rect.top()), Vec2::splat(icon_button::SIZE));
    ui.put(close, IconButton::of(Icon::X, CLOSE_ICON, CLOSE))
}

/// The content's frame inside the card: the shared padding.
pub fn padding() -> Frame {
    Frame::NONE.inner_margin(Margin { left: PAD_X as i8, right: PAD_X as i8, top: PAD_TOP as i8, bottom: PAD_BOTTOM as i8 })
}

// ------------------------------------------------------------------------------------------------
// the help

fn dialog_id() -> Id {
    Id::unique("rustradar::shortcuts::help")
}

/// The help's content's greatest height in a window `window_height` high.
pub fn max_height(window_height: f32) -> f32 {
    window_height - UA_MAX_HEIGHT_LESS
}

/// The UI pass's help for `state` (see the module documentation): its close goes to `actions`.
/// Answers the card while it is drawn.
pub fn ui(ctx: &Context, state: &AppState, actions: &mut Vec<Action>) -> Option<Rect> {
    let open = state.shortcuts.help_open();
    Return::track(ctx, dialog_id(), open);
    if !open {
        return None;
    }
    let rows = rows();
    let mut close = false;
    let shown = dialog(ctx, dialog_id(), WIDTH, max_height(ctx.content_rect().height()), |ui| {
        padding().show(ui, |ui| {
            close |= header(ui, TITLE).clicked();
            ui.add_space(HEADER_GAP);
            for (keys, what) in &rows {
                row(ui, keys, what);
            }
        });
    });
    if close || shown.close {
        actions.push(Action::Open(Open::Help, false));
    }
    Some(shown.rect)
}

/// A cap's border box width for `key`: its text, at least 22, the padding and the side borders.
pub fn cap_width(ctx: &Context, key: &str) -> f32 {
    let text = text::measure(ctx, text::job(key, &cap_style())).x;
    text.max(CAP_MIN_WIDTH) + 2.0 * CAP_PAD_X + 2.0 * theme::BORDER
}

/// A cap's border box height.
pub const CAP_BOX_HEIGHT: f32 = CAP_HEIGHT + theme::BORDER + CAP_BORDER_BOTTOM;

/// Paints the cap of `key` with its border box's top left at `at`; answers its rectangle.
pub fn paint_cap(ui: &Ui, at: Pos2, key: &str) -> Rect {
    let width = cap_width(ui.ctx(), key);
    let rect = Rect::from_min_size(at, Vec2::new(width, CAP_BOX_HEIGHT));
    let painter = ui.painter();
    // the border as the box in --line-control, the inside (transparent: the card) over it
    painter.rect_filled(rect, CornerRadius::same(CAP_RADIUS), theme::LINE_CONTROL);
    let inner = Rect::from_min_max(rect.min + Vec2::splat(theme::BORDER), rect.max - Vec2::new(theme::BORDER, CAP_BORDER_BOTTOM));
    let top = CAP_RADIUS - theme::BORDER as u8;
    let bottom = CAP_RADIUS - CAP_BORDER_BOTTOM as u8;
    painter.rect_filled(inner, CornerRadius { nw: top, ne: top, sw: bottom, se: bottom }, theme::SURFACE);
    let galley = painter.layout_job(text::job(key, &cap_style()));
    painter.galley(inner.center() - galley.size() / 2.0, galley, Color32::PLACEHOLDER);
    rect
}

/// One shortcut's row across `ui`'s width.
fn row(ui: &mut Ui, keys: &[&str], what: &str) {
    let width = ui.available_width();
    let dd_width = (width - KEYS_COLUMN - COLUMN_GAP).max(0.0);
    let style = description_style();
    let description = ui.painter().layout_job(text::wrapped(what, &style, dd_width));
    // its rows' line boxes (egui's galley puts its rows on whole pixels)
    let text_height = description.rows.len().max(1) as f32 * style.line_px();
    let inner = text_height.max(CAP_BOX_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, theme::BORDER + 2.0 * ROW_PAD_Y + inner), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    painter.rect_filled(Rect::from_min_size(rect.min, Vec2::new(width, theme::BORDER)), CornerRadius::ZERO, theme::LINE);
    let top = rect.top() + theme::BORDER + ROW_PAD_Y;
    let mut x = rect.left();
    for key in keys {
        x = paint_cap(ui, Pos2::new(x, top + (inner - CAP_BOX_HEIGHT) / 2.0), key).right() + CAP_GAP;
    }
    let at = Pos2::new(rect.left() + KEYS_COLUMN + COLUMN_GAP, top + (inner - text_height) / 2.0);
    ui.painter().galley(at, description, Color32::PLACEHOLDER);
}

/// A stable id for something inside one of the dialogs.
pub fn id(salt: impl Hash + std::fmt::Debug) -> Id {
    Id::unique("rustradar::dialogs").with(salt)
}

#[cfg(test)]
mod tests {
    use egui::{Event, Key, PointerButton, RawInput};
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::state::Startup;

    const SIZE: Vec2 = Vec2::new(1440.0, 900.0);

    fn state() -> AppState {
        AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, [1440.0, 900.0])
    }

    fn context() -> Context {
        let ctx = Context::default();
        theme::apply(&ctx);
        ctx
    }

    /// One pass at egui time `time` over `events`: the actions, and the card drawn.
    fn pass(ctx: &Context, state: &AppState, time: f64, events: Vec<Event>) -> (Vec<Action>, Option<Rect>) {
        let mut actions = Vec::new();
        let mut card = None;
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)), time: Some(time), events, ..Default::default() };
        ctx.run_ui(raw, |ui| card = super::ui(ui.ctx(), state, &mut actions)).textures_delta.clear();
        (actions, card)
    }

    fn click(at: Pos2) -> Vec<Event> {
        vec![
            Event::PointerMoved(at),
            Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() },
            Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() },
        ]
    }

    #[test]
    fn the_rows_are_todays_less_the_inspector() {
        let rows = rows();
        assert_eq!(rows.len(), if cfg!(debug_assertions) { 23 } else { 22 });
        assert_eq!(rows[0], (&["P"][..], "Aircraft mode"));
        assert_eq!(rows[10].0, ["+", "−"]);
        assert_eq!(rows[13], (&["1", "2", "3"][..], "Dark map, satellite imagery or realistic 3D"));
        assert_eq!(rows[21], (&["?"][..], "This list"));
        assert!(rows.iter().all(|(keys, what)| !keys.contains(&"Alt") && !what.starts_with("Developer tools")));
        if cfg!(debug_assertions) {
            assert_eq!(rows[22].0, ["Ctrl", "Shift", "D"]);
        }
    }

    #[test]
    fn caps_are_as_wide_as_chromium_lays_them_out() {
        let ctx = context();
        ctx.run_ui(RawInput::default(), |_| {}).textures_delta.clear();
        // Chromium: 34 for a letter, 63.75 for Shift+P, 34.19 for Esc (Overpass Mono 12 px at 600)
        assert_eq!(cap_width(&ctx, "P"), 34.0);
        assert!((cap_width(&ctx, "Shift+P") - 63.75).abs() < 0.3, "{}", cap_width(&ctx, "Shift+P"));
        assert!((cap_width(&ctx, "Esc") - 34.19).abs() < 0.3, "{}", cap_width(&ctx, "Esc"));
        assert_eq!(CAP_BOX_HEIGHT, 25.0);
    }

    #[test]
    fn the_help_is_drawn_while_open_and_closes_by_its_button_the_backdrop_and_escape() {
        let ctx = context();
        let mut s = state();
        assert_eq!(pass(&ctx, &s, 0.0, vec![]), (vec![], None));
        s.set_open(Open::Help, true);
        // the first pass lays it out unseen; then centred, 362 wide, as tall as the window allows
        let _ = pass(&ctx, &s, 0.0, vec![]);
        let (_, card) = pass(&ctx, &s, 1.0, vec![]);
        let card = card.expect("drawn");
        assert_eq!(card.width(), WIDTH + 2.0);
        assert!((card.left() - 539.0).abs() < 0.01, "{card:?}");
        assert!((card.height() - 868.0).abs() < 0.5 && (card.top() - 16.0).abs() < 0.5, "{card:?}");
        // the close button: 32 px at the header's right, 12 + 1 from the top, 16 + 1 from the right
        let button = Pos2::new(card.right() - 1.0 - PAD_X - 16.0, card.top() + 1.0 + PAD_TOP + 16.0);
        let (actions, _) = pass(&ctx, &s, 2.0, click(button));
        assert_eq!(actions, [Action::Open(Open::Help, false)]);
        // the backdrop
        let (actions, _) = pass(&ctx, &s, 3.0, click(Pos2::new(100.0, 450.0)));
        assert_eq!(actions, [Action::Open(Open::Help, false)]);
        // inside the card: nothing
        let (actions, _) = pass(&ctx, &s, 4.0, click(Pos2::new(card.center().x, card.top() + 200.0)));
        assert!(actions.is_empty());
        let escape = Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        let (actions, _) = pass(&ctx, &s, 5.0, vec![escape]);
        assert_eq!(actions, [Action::Open(Open::Help, false)]);
        s.set_open(Open::Help, false);
        assert_eq!(pass(&ctx, &s, 6.0, vec![]).1, None);
    }

    #[test]
    fn the_focus_goes_back_where_it_was_when_the_dialog_closes() {
        let ctx = context();
        let key = Id::unique("dialog");
        let button = Id::unique("the keys button");
        // a pass with the button drawn (a widget that is not drawn loses the focus) and the dialog
        let run = |open: bool| {
            let mut edge = None;
            let mut out = ctx.run_ui(RawInput::default(), |ui| {
                ui.interact(Rect::from_min_size(Pos2::ZERO, Vec2::splat(32.0)), button, Sense::click());
                edge = Return::track(ui.ctx(), key, open);
            });
            out.textures_delta.clear();
            edge
        };
        assert_eq!(run(false), None);
        ctx.memory_mut(|m| m.request_focus(button));
        run(false);
        assert_eq!(ctx.memory(|m| m.focused()), Some(button));
        assert_eq!(run(true), Some(true));
        assert_eq!(ctx.memory(|m| m.focused()), None, "taken while the dialog is open");
        assert_eq!(run(true), None);
        assert_eq!(run(false), Some(false));
        run(false);
        assert_eq!(ctx.memory(|m| m.focused()), Some(button), "given back");
    }

    #[test]
    fn the_card_narrows_to_the_window_less_32() {
        let ctx = context();
        let mut s = state();
        s.set_open(Open::Help, true);
        let phone = Vec2::new(390.0, 844.0);
        let mut card = None;
        for time in [0.0, 1.0] {
            let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, phone)), time: Some(time), ..Default::default() };
            ctx.run_ui(raw, |ui| card = super::ui(ui.ctx(), &s, &mut Vec::new())).textures_delta.clear();
        }
        let card = card.unwrap();
        // Chromium: 360 at x 15, 812 high at y 16
        assert_eq!(card.width(), 360.0);
        assert!((card.left() - 15.0).abs() < 0.01 && (card.top() - 16.0).abs() < 0.5 && (card.height() - 812.0).abs() < 0.5, "{card:?}");
    }
}
