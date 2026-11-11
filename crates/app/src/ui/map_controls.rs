//! MapControls (doc 06 §19.1, §2.3, §27; `MapControls.svelte`): the column of small `.surface`
//! stacks at the bottom right of the map, and the popover of the menu that is open.
//!
//! **The column** ([`ui`]): `right: 16 + inset; bottom: 28`, the stacks right-aligned 8 apart,
//! each a `.surface` with 2 px of padding round 32 × 32 icon buttons (18 px icons, stroke 1.75):
//! 1. *Map layers* and *Settings* (pressed while their menu is open; a click opens it, or closes
//!    it when open), *API keys* (`KeyRound` in `--live`, and its tooltip saying so, when signed in
//!    to Flightradar24) and *Keyboard shortcuts*: each of the last two closes the menu and opens
//!    the keys dialog or the shortcuts help;
//! 2. *Zoom in* (off at zoom ≥ 18) and *Zoom out* (off at zoom ≤ 0.5): `zoomBy(±1)`;
//! 3. *Tilt* (pressed while the pitch is over 5°: then it looks straight down, else it tilts to
//!    55°) and the *compass* (`Navigation2` in `--amber`, turned by −bearing with a 150 ms ease;
//!    off and grey unless the bearing is more than 0.5° either way or the map is tilted): north
//!    up.
//!
//! `inset` is [`shell::controls_inset`] (372 while a detail panel is open), and `right` eases to it
//! over 200 ms (`transition: right 200ms var(--ease-out)`). The column is one area, so the gaps
//! between the stacks take the pointer as the `.controls` box does.
//!
//! **The popover** (one at a time, [`AppState`]'s `shortcuts.menu()`): a `.surface` with 300 px of
//! content and padding 16, its right edge 8 px left of the column and its bottom on the column's
//! (`right: calc(100% + 8px); bottom: 0` against `.controls`), growing upwards, at most the window's
//! height less 60 of content and scrolling past that; entering over 180 ms from 6 px to the right
//! and transparent. The content is [`super::menus`]. It closes on its button, on a press anywhere
//! outside the column and the popover, on Escape ([`escape`], before the page's own Escape:
//! the keyboard goes back to the menu's button), and on the keys and help buttons.
//!
//! **Checked** against `docs/port/reference/dark-v1-1440x900.png` (the column closed: every stack
//! and icon on the same pixels) and Chromium's layout of today's component with its menus open
//! (`tests/map_controls.rs`).
//!
//! Left to item U19: the phone layout (right `max(8, safe-right)`, bottom the sheet's height + 24
//! eased over 220 ms; no zoom, keys or help buttons, the compass hidden while it is off; the menus
//! as sheets of their own, [`super::menus::settings`] with its keys entry).
//!
//! Deviations: the tooltips show for a disabled button too (egui's tooltip on hover); a press that
//! closes the menu is the egui pass's press, so a press and its release in one frame both act.

use egui::{Align2, Area, Context, Event, Id, Key, Order, Rect, ScrollArea, Ui, Vec2};

use super::icons::{self, Icon};
use super::widgets::icon_button::{self, IconButton};
use super::{anim, menus, shell, theme};
use crate::state::shortcuts::{Menu, Open};
use crate::state::{Action, AppState};

/// The column's distance from the window's right edge without an inset (`right: var(--space-4)`).
pub const RIGHT: f32 = theme::SPACE_4;
/// Its distance from the bottom (`bottom: 28px`).
pub const BOTTOM: f32 = 28.0;
/// Between the stacks, and between the column and the popover (`var(--space-2)`).
pub const GAP: f32 = theme::SPACE_2;
/// A stack's padding round its buttons.
pub const STACK_PADDING: f32 = 2.0;
/// A stack's width: a button, the padding and the border.
pub const STACK_WIDTH: f32 = icon_button::SIZE + 2.0 * (STACK_PADDING + theme::BORDER);
/// The buttons' icons: 18 px, stroke 1.75.
pub const ICON: f32 = 18.0;
/// The pitch the tilt button tilts to (`TILT`).
pub const TILT: f64 = 55.0;
/// A pitch over this counts as tilted (`pitch > 5`).
pub const TILTED_ABOVE: f64 = 5.0;
/// A bearing further than this from north counts as turned (`Math.abs(bearing) > 0.5`).
pub const TURNED_BEYOND: f64 = 0.5;
/// Zoom in is off from this zoom (`zoom >= 18`), zoom out down to this one (`zoom <= 0.5`).
pub const ZOOM_IN_UNTIL: f64 = 18.0;
pub const ZOOM_OUT_UNTIL: f64 = 0.5;
/// The popover's content width (`.menu { width: 300px }`) and padding.
pub const POPOVER_WIDTH: f32 = 300.0;
pub const POPOVER_PADDING: f32 = theme::SPACE_4;
/// The window's height less this is the most the popover's content takes
/// (`max-height: calc(100vh - 60px)`).
pub const POPOVER_SPACE: f32 = 60.0;

/// Whether the map is tilted (`tilted`).
pub fn tilted(pitch: f64) -> bool {
    pitch > TILTED_ABOVE
}

/// Whether the map is turned or tilted, so the compass can reset it (`rotated`).
pub fn rotated(bearing: f64, pitch: f64) -> bool {
    bearing.abs() > TURNED_BEYOND || tilted(pitch)
}

/// Whether the session says Flightradar24 is signed in (`session?.authenticated`; the
/// snapshot's word does not count here).
pub fn signed_in(state: &AppState) -> bool {
    state.aircraft.session().is_some_and(|s| s.authenticated)
}

/// The keys button's tooltip.
pub fn keys_tooltip(signed_in: bool) -> &'static str {
    if signed_in { "API keys (signed in to Flightradar24)" } else { "API keys and Flightradar24 login" }
}

/// The tilt button's name and tooltip.
pub fn tilt_label(tilted: bool) -> (&'static str, &'static str) {
    if tilted { ("Look straight down", "Look straight down (T)") } else { ("Tilt to 3D", "Tilt to 3D (T, or drag with the right mouse button)") }
}

/// The menu open after a click on `which`'s button with `open` open (`toggle`).
pub fn toggled(open: Option<Menu>, which: Menu) -> Option<Menu> {
    if open == Some(which) { None } else { Some(which) }
}

/// Where the controls are this pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    /// The column, its gaps included.
    pub column: Rect,
    /// The three stacks, top to bottom.
    pub stacks: [Rect; 3],
    /// The popover, while a menu is open.
    pub popover: Option<Rect>,
}

impl Default for Placed {
    fn default() -> Self {
        Placed { column: Rect::NOTHING, stacks: [Rect::NOTHING; 3], popover: None }
    }
}

/// The data key of the button that opens `menu`, where Escape sends the keyboard back to.
fn trigger_key(menu: Menu) -> Id {
    Id::unique(("rustradar::map_controls::trigger", menu as u8))
}

/// Escape while a menu is open (`onkeydowncapture`, before the page's own keys: call it before
/// [`shell::ui`]): the key is taken out of egui's input, the menu closes, and its button takes the
/// keyboard.
pub fn escape(ctx: &Context, state: &AppState, actions: &mut Vec<Action>) {
    let Some(menu) = state.shortcuts.menu() else { return };
    let pressed = |e: &Event| matches!(e, Event::Key { key: Key::Escape, pressed: true, .. });
    if !ctx.input(|i| i.events.iter().any(pressed)) {
        return;
    }
    ctx.input_mut(|i| i.events.retain(|e| !pressed(e)));
    actions.push(Action::SetMenu(None));
    if let Some(button) = ctx.data(|d| d.get_temp::<Id>(trigger_key(menu))) {
        ctx.memory_mut(|m| m.request_focus(button));
    }
}

/// The controls for `state` (see the module documentation): their actions go to `actions`.
pub fn ui(ctx: &Context, state: &AppState, actions: &mut Vec<Action>) -> Placed {
    let inset = anim::value(ctx, Id::unique("rustradar::map_controls::inset"), shell::controls_inset(state), anim::CONTROLS_SHIFT);
    let right = RIGHT + inset;
    let menu = state.shortcuts.menu();
    let mut stacks = [Rect::NOTHING; 3];
    let column = Area::new(Id::unique("rustradar::map_controls"))
        .order(Order::Middle)
        .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-right, -BOTTOM))
        .fade_in(false)
        .constrain(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(0.0, GAP);
            stacks[0] = stack(ui, |ui| menus_stack(ui, state, menu, actions));
            stacks[1] = stack(ui, |ui| zoom_stack(ui, state, actions));
            stacks[2] = stack(ui, |ui| view_stack(ui, state, actions));
        })
        .response
        .rect;
    let popover = menu.map(|menu| popover(ctx, state, menu, right, actions));
    // a press anywhere outside the controls closes the menu (`onpointerdown` on the window)
    if menu.is_some()
        && let Some(at) = ctx.input(|i| if i.pointer.any_pressed() { i.pointer.press_origin() } else { None })
        && !column.contains(at)
        && !popover.is_some_and(|p| p.contains(at))
    {
        actions.push(Action::SetMenu(None));
    }
    Placed { column, stacks, popover }
}

/// A `.stack.surface`: the buttons `add` lays out one under another with 2 px of padding.
fn stack(ui: &mut Ui, add: impl FnOnce(&mut Ui)) -> Rect {
    theme::surface()
        .inner_margin(STACK_PADDING)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.set_width(icon_button::SIZE);
            add(ui);
        })
        .response
        .rect
}

/// An 18 px icon at stroke 1.75.
fn icon(icon: Icon) -> icons::Spec {
    icon.at(ICON, icons::STROKE_THIN)
}

/// Stack A: the menus, the keys dialog, the shortcuts help.
fn menus_stack(ui: &mut Ui, state: &AppState, menu: Option<Menu>, actions: &mut Vec<Action>) {
    for (which, glyph, name) in [(Menu::Layers, Icon::Layers, "Map layers"), (Menu::Settings, Icon::Settings, "Settings")] {
        let button = IconButton::new(icon(glyph), name).pressed(menu == Some(which)).tooltip(name).show(ui);
        ui.ctx().data_mut(|d| d.insert_temp(trigger_key(which), button.id));
        if button.clicked() {
            actions.push(Action::SetMenu(toggled(menu, which)));
        }
    }
    let signed_in = signed_in(state);
    let mut keys = IconButton::new(icon(Icon::KeyRound), "API keys").tooltip(keys_tooltip(signed_in));
    if signed_in {
        keys = keys.icon_color(theme::LIVE);
    }
    let close = |actions: &mut Vec<Action>| {
        if menu.is_some() {
            actions.push(Action::SetMenu(None));
        }
    };
    if keys.show(ui).clicked() {
        close(actions);
        actions.push(Action::Open(Open::Keys, true));
    }
    if IconButton::new(icon(Icon::Keyboard), "Keyboard shortcuts").tooltip("Keyboard shortcuts (?)").show(ui).clicked() {
        close(actions);
        actions.push(Action::Open(Open::Help, true));
    }
}

/// Stack B: zoom in and out.
fn zoom_stack(ui: &mut Ui, state: &AppState, actions: &mut Vec<Action>) {
    let zoom = state.controller.view().zoom;
    if IconButton::new(icon(Icon::Plus), "Zoom in").tooltip("Zoom in (+)").enabled(zoom < ZOOM_IN_UNTIL).show(ui).clicked() {
        actions.push(Action::ZoomBy(1.0));
    }
    if IconButton::new(icon(Icon::Minus), "Zoom out").tooltip("Zoom out (\u{2212})").enabled(zoom > ZOOM_OUT_UNTIL).show(ui).clicked() {
        actions.push(Action::ZoomBy(-1.0));
    }
}

/// Stack C: the tilt and the compass.
fn view_stack(ui: &mut Ui, state: &AppState, actions: &mut Vec<Action>) {
    let view = state.controller.view();
    let tilted = tilted(view.pitch);
    let (name, tip) = tilt_label(tilted);
    if IconButton::new(icon(Icon::Rotate3d), name).pressed(tilted).tooltip(tip).show(ui).clicked() {
        actions.push(Action::SetPitch(if tilted { 0.0 } else { TILT }));
    }
    let on = rotated(view.bearing, view.pitch);
    // the needle turned by −bearing degrees, the turn eased over 150 ms
    let turn = anim::value(ui.ctx(), Id::unique("rustradar::map_controls::compass"), -view.bearing as f32, anim::COMPASS);
    let mut compass = IconButton::new(icon(Icon::Navigation2).rotated(turn.to_radians()), "Reset to north up").tooltip("Reset to north up (N)").enabled(on);
    if on {
        compass = compass.icon_color(theme::AMBER);
    }
    if compass.show(ui).clicked() {
        actions.push(Action::ResetOrientation);
    }
}

/// The open menu's popover left of the column, whose right edge is `right` from the window's;
/// answers its rectangle.
fn popover(ctx: &Context, state: &AppState, menu: Menu, right: f32, actions: &mut Vec<Action>) -> Rect {
    let id = Id::unique(("rustradar::map_controls::popover", menu as u8));
    let t = anim::enter(ctx, id, anim::POPOVER_ENTER);
    let dx = anim::POPOVER_ENTER_DX * (1.0 - t);
    let max_height = (ctx.content_rect().height() - POPOVER_SPACE).max(0.0);
    Area::new(id)
        .order(Order::Middle)
        .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-(right + STACK_WIDTH + GAP) + dx, -BOTTOM))
        .fade_in(false)
        .constrain(false)
        .show(ctx, |ui| {
            ui.multiply_opacity(t);
            theme::surface().inner_margin(POPOVER_PADDING).show(ui, |ui| {
                ui.set_width(POPOVER_WIDTH);
                // the area lays its content out in the last pass's rectangle: this pass's room
                ui.set_max_height(max_height);
                ScrollArea::vertical().id_salt(id).max_height(max_height).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    // Chromium lays the content out from the box's exact top and paints the box's
                    // edges on whole pixels: the box here is a whole number of pixels tall, its
                    // bottom on the column's, and the fraction left over (as measured in the last
                    // pass) goes above the content, which so lands where Chromium puts it
                    let slack_id = id.with("slack");
                    let slack = ui.ctx().data(|d| d.get_temp::<f32>(slack_id)).unwrap_or(0.0);
                    let top = ui.cursor().top();
                    ui.add_space(slack);
                    let start = ui.cursor().top();
                    match menu {
                        Menu::Layers => menus::layers(ui, state, actions),
                        Menu::Settings => menus::settings(ui, state, actions, false),
                    }
                    let height = ui.min_rect().bottom() - start;
                    ui.ctx().data_mut(|d| d.insert_temp(slack_id, height.ceil() - height));
                    ui.expand_to_include_y(top + (slack + height).ceil());
                });
            });
        })
        .response
        .rect
}

#[cfg(test)]
mod tests {
    use egui::{PointerButton, Pos2, RawInput};
    use rustradar_core::model::SessionInfo;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::state::Startup;

    fn state_at(view: ViewState) -> AppState {
        AppState::new(Startup::default(), None, None, None, view, [1440.0, 900.0])
    }

    fn state() -> AppState {
        state_at(ViewState::INITIAL)
    }

    /// One pass of the controls (the Escape step first) at egui time `time` with `events`.
    fn pass(ctx: &Context, state: &AppState, time: f64, events: Vec<Event>) -> (Placed, Vec<Action>, Vec<Event>) {
        let mut actions = Vec::new();
        let mut placed = Placed::default();
        let mut left = Vec::new();
        let raw = RawInput { events, time: Some(time), screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0))), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            escape(ui.ctx(), state, &mut actions);
            left = ui.ctx().input(|i| i.events.clone());
            placed = super::ui(ui.ctx(), state, &mut actions);
        });
        out.textures_delta.clear();
        (placed, actions, left)
    }

    /// A click at `at` from egui time `time`: the pointer moves there, then presses and lets go,
    /// each in a pass of its own. Answers the actions of the three passes.
    fn click(ctx: &Context, state: &AppState, at: Pos2, time: f64) -> Vec<Action> {
        let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        let mut actions = pass(ctx, state, time, vec![Event::PointerMoved(at)]).1;
        actions.extend(pass(ctx, state, time + 0.05, vec![button(true)]).1);
        actions.extend(pass(ctx, state, time + 0.1, vec![button(false)]).1);
        actions
    }

    fn apply(s: &mut AppState, actions: &[Action]) {
        for &a in actions {
            s.apply(a, 0.0);
        }
    }

    /// The centre of the `n`th button (0–7, top to bottom) of the column at `placed`.
    fn button(placed: &Placed, n: usize) -> Pos2 {
        let (stack, row) = match n {
            0..4 => (0, n),
            4..6 => (1, n - 4),
            _ => (2, n - 6),
        };
        let r = placed.stacks[stack];
        Pos2::new(r.center().x, r.top() + theme::BORDER + STACK_PADDING + (row as f32 + 0.5) * icon_button::SIZE)
    }

    #[test]
    fn the_rules_of_today_s_buttons() {
        assert!(!tilted(5.0) && tilted(5.01));
        assert!(!rotated(0.5, 0.0) && rotated(-0.51, 0.0) && rotated(0.0, 6.0));
        assert_eq!(keys_tooltip(false), "API keys and Flightradar24 login");
        assert_eq!(keys_tooltip(true), "API keys (signed in to Flightradar24)");
        assert_eq!(tilt_label(false).1, "Tilt to 3D (T, or drag with the right mouse button)");
        assert_eq!(tilt_label(true), ("Look straight down", "Look straight down (T)"));
        assert_eq!(toggled(None, Menu::Layers), Some(Menu::Layers));
        assert_eq!(toggled(Some(Menu::Layers), Menu::Layers), None);
        assert_eq!(toggled(Some(Menu::Settings), Menu::Layers), Some(Menu::Layers));
        assert_eq!(STACK_WIDTH, 38.0);
    }

    #[test]
    fn the_column_where_today_s_is() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let s = state();
        pass(&ctx, &s, 0.0, Vec::new());
        let (placed, ..) = pass(&ctx, &s, 1.0, Vec::new());
        // Chromium (and dark-v1-1440x900): the column 1386–1424 × 582–872, stacks 134, 70, 70
        assert_eq!(placed.column, Rect::from_min_max(Pos2::new(1386.0, 582.0), Pos2::new(1424.0, 872.0)));
        assert_eq!(placed.stacks.map(|r| (r.top(), r.height())), [(582.0, 134.0), (724.0, 70.0), (802.0, 70.0)]);
        assert_eq!(placed.popover, None);
    }

    #[test]
    fn the_inset_eases_the_column_left_while_a_panel_is_open() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let mut s = state();
        pass(&ctx, &s, 0.0, Vec::new());
        s.airport.open("ARN", 0.0);
        let (start, ..) = pass(&ctx, &s, 1.0, Vec::new());
        let (mid, ..) = pass(&ctx, &s, 1.1, Vec::new());
        let (end, ..) = pass(&ctx, &s, 1.3, Vec::new());
        assert_eq!(start.column.right(), 1424.0, "the transition starts where it was");
        assert!(mid.column.right() < 1424.0 - 300.0 && mid.column.right() > 1052.0, "eased out: {}", mid.column.right());
        assert_eq!(end.column.right(), 1440.0 - 16.0 - 372.0);
    }

    /// The controls laid out (an anchored area is sized in its first pass), and a click on the
    /// `n`th button that applies its actions, each at a later egui time than the one before.
    struct Clicker {
        ctx: Context,
        time: std::cell::Cell<f64>,
        placed: Placed,
    }

    impl Clicker {
        fn new(s: &AppState) -> Self {
            let ctx = Context::default();
            theme::apply(&ctx);
            pass(&ctx, s, 0.0, Vec::new());
            let placed = pass(&ctx, s, 0.5, Vec::new()).0;
            Clicker { ctx, time: std::cell::Cell::new(1.0), placed }
        }

        fn click(&self, s: &mut AppState, n: usize) -> Vec<Action> {
            let t = self.time.get();
            self.time.set(t + 1.0);
            pass(&self.ctx, s, t, Vec::new());
            let actions = click(&self.ctx, s, button(&self.placed, n), t + 0.1);
            // the pointer leaves, so the next click starts afresh
            pass(&self.ctx, s, t + 0.5, vec![Event::PointerGone]);
            apply(s, &actions);
            actions
        }
    }

    #[test]
    fn the_buttons_ask_for_their_actions() {
        let mut s = state();
        let c = Clicker::new(&s);
        assert_eq!(c.click(&mut s, 0), [Action::SetMenu(Some(Menu::Layers))]);
        assert_eq!(s.shortcuts.menu(), Some(Menu::Layers));
        // Settings while the layers are open: the settings instead
        assert_eq!(c.click(&mut s, 1), [Action::SetMenu(Some(Menu::Settings))]);
        // its own button again: closed
        assert_eq!(c.click(&mut s, 1), [Action::SetMenu(None)]);
        assert_eq!(s.shortcuts.menu(), None);
        // keys and help close the menu
        s.set_menu(Some(Menu::Layers));
        assert_eq!(c.click(&mut s, 2), [Action::SetMenu(None), Action::Open(Open::Keys, true)]);
        assert!(s.shortcuts.keys_open() && s.shortcuts.menu().is_none());
        s.set_open(Open::Keys, false);
        assert_eq!(c.click(&mut s, 3), [Action::Open(Open::Help, true)]);
        assert!(s.shortcuts.help_open());
        s.set_open(Open::Help, false);
        assert_eq!(c.click(&mut s, 4), [Action::ZoomBy(1.0)]);
        assert_eq!(c.click(&mut s, 5), [Action::ZoomBy(-1.0)]);
        // north up looking straight down: the compass is off
        assert!(c.click(&mut s, 7).is_empty());
        assert_eq!(c.click(&mut s, 6), [Action::SetPitch(TILT)]);
    }

    #[test]
    fn limits_and_the_tilted_and_turned_map() {
        let mut s = state_at(ViewState { zoom: 18.0, pitch: 45.0, bearing: 30.0, ..ViewState::INITIAL });
        let c = Clicker::new(&s);
        assert!(c.click(&mut s, 4).is_empty(), "zoom in off at 18");
        assert_eq!(c.click(&mut s, 5), [Action::ZoomBy(-1.0)]);
        assert_eq!(c.click(&mut s, 6), [Action::SetPitch(0.0)], "tilted: straight down");
        assert_eq!(c.click(&mut s, 7), [Action::ResetOrientation]);
        let mut s = state_at(ViewState { zoom: 0.5, ..ViewState::INITIAL });
        let c = Clicker::new(&s);
        assert!(c.click(&mut s, 5).is_empty(), "zoom out off at 0.5");
        assert_eq!(c.click(&mut s, 4), [Action::ZoomBy(1.0)]);
    }

    #[test]
    fn the_popover_left_of_the_column_and_a_press_outside_closes_it() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let mut s = state();
        s.set_menu(Some(Menu::Settings));
        for t in [0.0, 1.0] {
            pass(&ctx, &s, t, Vec::new());
        }
        let (placed, ..) = pass(&ctx, &s, 2.0, Vec::new());
        let popover = placed.popover.expect("the popover");
        // Chromium: 1044–1378, bottom 872, 309.13 tall
        assert_eq!((popover.left(), popover.right(), popover.bottom()), (1044.0, 1378.0, 872.0));
        assert!((popover.height() - 309.13).abs() < 1.0, "{}", popover.height());
        // a press inside the popover or the column's gaps leaves it open
        let actions = click(&ctx, &s, Pos2::new(1100.0, 600.0), 3.0);
        assert!(actions.is_empty(), "{actions:?}");
        let actions = click(&ctx, &s, Pos2::new(1400.0, 720.0), 4.0);
        assert!(actions.is_empty(), "{actions:?}");
        // one on the map closes it
        let actions = click(&ctx, &s, Pos2::new(700.0, 400.0), 5.0);
        assert_eq!(actions, [Action::SetMenu(None)]);
        apply(&mut s, &actions);
        let (placed, ..) = pass(&ctx, &s, 6.0, Vec::new());
        assert_eq!(placed.popover, None);
    }

    #[test]
    fn escape_closes_the_menu_before_anything_else_and_the_button_takes_the_keyboard() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let mut s = state();
        s.set_menu(Some(Menu::Layers));
        s.set_open(Open::Help, true);
        pass(&ctx, &s, 0.0, Vec::new());
        let escape = Event::Key { key: Key::Escape, physical_key: Some(Key::Escape), pressed: true, repeat: false, modifiers: Default::default() };
        let (_, actions, left) = pass(&ctx, &s, 1.0, vec![escape.clone()]);
        assert_eq!(actions, [Action::SetMenu(None)]);
        assert!(left.is_empty(), "the page's Escape does not see it: {left:?}");
        apply(&mut s, &actions);
        assert!(s.shortcuts.help_open(), "the help stays");
        let layers = ctx.data(|d| d.get_temp::<Id>(trigger_key(Menu::Layers))).expect("the layers button");
        assert!(ctx.memory(|m| m.has_focus(layers)), "the layers button has the keyboard");
        // with no menu open, Escape goes on to the page
        let (_, actions, left) = pass(&ctx, &s, 2.0, vec![escape]);
        assert!(actions.is_empty());
        assert!(matches!(left[..], [Event::Key { key: Key::Escape, pressed: true, .. }]), "{left:?}");
    }

    #[test]
    fn the_keys_button_signed_in() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let mut s = state();
        assert!(!signed_in(&s));
        s.aircraft.set_session(SessionInfo { authenticated: false, expires: None });
        assert!(!signed_in(&s));
        s.aircraft.set_session(SessionInfo { authenticated: true, expires: None });
        assert!(signed_in(&s));
        pass(&ctx, &s, 0.0, Vec::new());
    }
}
