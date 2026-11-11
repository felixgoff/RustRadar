//! The desktop shell (doc 06 §2.3, §2.8–2.10, §3.11, §3.13; `+page.svelte:1412-1760`): what the
//! UI pass draws over the map and where, and the page's keys.
//!
//! [`ui`] runs once per frame inside the egui pass (`app::Engine::frame`, step 5) while
//! `app::Engine::shell` is on: the window turns it on; headless shots (`--screenshot`) and the map's
//! tests leave it off and draw the map alone, as they were written to.
//! 1. **the keys** of the frame (egui's key events) through [`AppState::shortcut`]: their actions
//!    are queued with the others, a handled key (and the text it typed) is taken out of egui's
//!    input (`preventDefault`), `/` asks the search box to take the keyboard
//!    ([`take_focus_search`]);
//! 2. **the sidebar** ([`super::sidebar`]): the primary card, the filter panel, the list card;
//! 3. **the detail panels** (§2.10), in the page's order, all at the same place: the airport board
//!    (kept but not drawn while a flight is shown over it, so going back finds its tab and pages),
//!    the flight, the ship, the train. Selecting one closes the others in the state, so at most
//!    one is drawn. Each is an egui [`Area`] anchored at right 16, bottom 28, 360 px of content
//!    wide (a 1 px border round it: `.surface` is `content-box`), at most the window's height less
//!    16 and 28, growing upwards, entering over 200 ms from 8 px to the right and transparent
//!    (`animation: enter 200ms var(--ease-out)`, again whenever it is drawn after a frame without
//!    it).
//!
//! **Slots.** The components inside the shell are other items': [`Slots`] names one function per
//! slot, and [`Slots::APP`] is the app's set, where each item puts its function: the search box
//! (U8), the three filter panels (U9, U14, U15; a filter panel's function answers its natural
//! height, header and whole body, and scrolls its own body within the height it is given), the
//! three lists (U9, U14, U15; the card scrolls them), the four detail panels (U10, U13, U14, U15:
//! the panel's own content in the area this module places, which it scrolls itself). A slot no item
//! fills is not drawn. Tests draw the shell with stand-ins ([`show`]).
//!
//! What other items read here: [`detail_open`] and [`controls_inset`] (the map controls move 372 px
//! left while a panel is open, U16), [`take_focus_search`] (U8), [`PANEL_WIDTH`] and the panel
//! area ([`panel`]).
//!
//! Deviations:
//! - There is no `--ui <fixture>` shot of the plan's stage 7 (the command line and the headless
//!   path are not this item's): the shell's shots are made by its tests (`tests/shell.rs`), with
//!   stand-ins in the slots, and through the window's engine with the shell on.
//! - The keys of one frame are read against the state before any of them took effect (today each
//!   key sees the one before it): `?` and then `f` within one frame both act.
//! - The phone layout (top bar, bottom sheet) is item U19's; the shell draws the desktop layout.

use egui::{Align2, Area, Context, Event, Id, Key, Order, Rect, Ui, Vec2};
use rustradar_core::settings::Mode;

use super::sidebar::{self, Placed as SidebarPlaced};
use super::{anim, theme};
use crate::input::Modifiers;
use crate::state::shortcuts::{Press, Shortcut, Typed};
use crate::state::{Action, AppState};

/// A detail panel's distance from the window's right edge (`right: var(--space-4)`).
pub const PANEL_RIGHT: f32 = theme::SPACE_4;
/// Its distance from the bottom, clear of the attribution line (`bottom: 28px`).
pub const PANEL_BOTTOM: f32 = 28.0;
/// Its content's width (`width: 360px`).
pub const PANEL_WIDTH: f32 = 360.0;
/// How far the map controls move left while a panel is open (`PANEL_INSET = 360 + 12`).
pub const PANEL_INSET: f32 = 372.0;

/// What the shell's slots get: the state as of this frame's poll, the local time, and where to
/// put actions.
pub struct Pass<'a> {
    pub state: &'a AppState,
    /// Local Unix ms: the ages and countdowns read it (today's 1 s `now`).
    pub now_ms: f64,
    pub actions: &'a mut Vec<Action>,
}

impl Pass<'_> {
    /// The focus's search query (`trainQuery`, `shipQuery`): the trains' and ships' lists and
    /// counts match it (`state::search`, U8; the aircraft's query narrows no list).
    pub fn query(&self) -> &str {
        self.state.search.list_query(self.state.persisted.settings.mode)
    }
}

/// A slot's component: draws into `ui` from the pass's state.
pub type Slot = fn(&mut Ui, &mut Pass<'_>);
/// A filter panel: draws into `ui` (at most its height) and answers its natural height, header
/// and whole body, for the sidebar's heights.
pub type FilterSlot = fn(&mut Ui, &mut Pass<'_>) -> f32;

/// The components the shell places (see the module documentation).
#[derive(Debug, Clone, Copy, Default)]
pub struct Slots {
    /// The search box in the primary card (U8).
    pub search: Option<Slot>,
    /// The filter panels, by focus (U9, U14, U15).
    pub aircraft_filters: Option<FilterSlot>,
    pub train_filters: Option<FilterSlot>,
    pub ship_filters: Option<FilterSlot>,
    /// The lists, by focus (U9: TopFlights, U14, U15).
    pub aircraft_list: Option<Slot>,
    pub train_list: Option<Slot>,
    pub ship_list: Option<Slot>,
    /// The detail panels (U13, U10, U15, U14).
    pub airport_panel: Option<Slot>,
    pub flight_panel: Option<Slot>,
    pub ship_panel: Option<Slot>,
    pub train_panel: Option<Slot>,
}

impl Slots {
    /// The app's components: each item puts its function in its slot here.
    pub const APP: Slots = Slots {
        search: Some(super::search::show),
        aircraft_filters: Some(super::filters::aircraft::show),
        train_filters: None,
        ship_filters: None,
        aircraft_list: Some(super::lists::flights::show),
        train_list: None,
        ship_list: None,
        airport_panel: None,
        flight_panel: None,
        ship_panel: None,
        train_panel: None,
    };

    /// The focus's filter panel.
    pub fn filters(&self, mode: Mode) -> Option<FilterSlot> {
        match mode {
            Mode::Aircraft => self.aircraft_filters,
            Mode::Trains => self.train_filters,
            Mode::Ships => self.ship_filters,
        }
    }

    /// The focus's list.
    pub fn list(&self, mode: Mode) -> Option<Slot> {
        match mode {
            Mode::Aircraft => self.aircraft_list,
            Mode::Trains => self.train_list,
            Mode::Ships => self.ship_list,
        }
    }

    fn panel(&self, panel: Panel) -> Option<Slot> {
        match panel {
            Panel::Airport => self.airport_panel,
            Panel::Flight => self.flight_panel,
            Panel::Ship => self.ship_panel,
            Panel::Train => self.train_panel,
        }
    }
}

/// A detail panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Panel {
    Airport,
    Flight,
    Ship,
    Train,
}

/// The detail panels to draw, in the page's order (§2.10): the airport board unless a flight is
/// shown over it, the flight, the ship, the train.
pub fn panels(state: &AppState) -> Vec<Panel> {
    let flight = state.flight.selected().is_some();
    let mut open = Vec::new();
    if state.airport.code().is_some() && !flight {
        open.push(Panel::Airport);
    }
    if flight {
        open.push(Panel::Flight);
    }
    if state.ships.selected().is_some() {
        open.push(Panel::Ship);
    }
    if state.trains.selected().is_some() {
        open.push(Panel::Train);
    }
    open
}

/// Whether a flight, an airport board, a ship or a train is open (`detailOpen`).
pub fn detail_open(state: &AppState) -> bool {
    state.flight.selected().is_some() || state.airport.code().is_some() || state.ships.selected().is_some() || state.trains.selected().is_some()
}

/// How far the map controls stand left of their place (`inset`): [`PANEL_INSET`] while a detail
/// panel is open on the desktop, else 0 (item U16 eases `right` over 200 ms).
pub fn controls_inset(state: &AppState) -> f32 {
    if detail_open(state) { PANEL_INSET } else { 0.0 }
}

/// Where the shell put its parts this frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Placed {
    pub sidebar: SidebarPlaced,
    /// The detail panel drawn, if any.
    pub panel: Option<(Panel, Rect)>,
}

/// The UI pass's shell for `state` at local time `now_ms`, with the app's components.
pub fn ui(ctx: &Context, state: &AppState, now_ms: f64, actions: &mut Vec<Action>) -> Placed {
    show(ctx, state, now_ms, actions, &Slots::APP)
}

/// The shell with the components in `slots`.
pub fn show(ctx: &Context, state: &AppState, now_ms: f64, actions: &mut Vec<Action>, slots: &Slots) -> Placed {
    keys(ctx, state, actions);
    let mut pass = Pass { state, now_ms, actions };
    let sidebar = sidebar::show(ctx, &mut pass, slots);
    let mut drawn = None;
    for kind in panels(state) {
        if let Some(slot) = slots.panel(kind) {
            drawn = Some((kind, panel(ctx, kind, |ui| slot(ui, &mut pass))));
        }
    }
    Placed { sidebar, panel: drawn }
}

/// A detail panel's area with `add` as its content (360 wide, at most the window's height less 44),
/// anchored at right 16, bottom 28, entering as today's panels do. Answers its rectangle.
pub fn panel(ctx: &Context, kind: Panel, add: impl FnOnce(&mut Ui)) -> Rect {
    let id = Id::unique(("rustradar::shell::panel", kind));
    let t = anim::enter(ctx, id, anim::PANEL_ENTER);
    let dx = anim::PANEL_ENTER_DX * (1.0 - t);
    let screen = ctx.content_rect();
    let max_height = (screen.height() - theme::SPACE_4 - PANEL_BOTTOM).max(0.0);
    Area::new(id)
        .order(Order::Middle)
        .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-PANEL_RIGHT + dx, -PANEL_BOTTOM))
        .fade_in(false)
        .constrain(false)
        .show(ctx, |ui| {
            ui.multiply_opacity(t);
            theme::surface().show(ui, |ui| {
                ui.set_width(PANEL_WIDTH);
                ui.set_max_height(max_height);
                add(ui);
            });
        })
        .response
        .rect
}

fn focus_search_id() -> Id {
    Id::unique("rustradar::shell::focus_search")
}

/// Whether `/` asked the search box to take the keyboard in this pass, and takes the ask (item
/// U8 calls it before drawing its field, and gives the field the focus when it is true).
pub fn take_focus_search(ctx: &Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(focus_search_id())).unwrap_or(false)
}

/// The page's key for egui's `key` with `modifiers`, `physical` its place on the keyboard.
pub fn press(key: Key, physical: Option<Key>, modifiers: egui::Modifiers) -> Press {
    let typed = match key {
        Key::Escape => Typed::Escape,
        _ => {
            let name = key.symbol_or_name();
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                // egui names letters in upper case: the case is Shift's
                (Some(c), None) if c.is_ascii_alphabetic() => Typed::Char(if modifiers.shift { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() }),
                // digits and punctuation as typed (the arrows' symbols are not)
                (Some(c), None) if c.is_ascii_graphic() => Typed::Char(c),
                _ => Typed::Other,
            }
        }
    };
    let modifiers = Modifiers { shift: modifiers.shift, ctrl: modifiers.ctrl, alt: modifiers.alt, meta: modifiers.mac_cmd };
    Press { key: typed, key_d: physical == Some(Key::D), modifiers }
}

/// The frame's keys (see the module documentation).
fn keys(ctx: &Context, state: &AppState, actions: &mut Vec<Action>) {
    let typing = ctx.text_edit_focused();
    let events = ctx.input(|i| i.events.clone());
    let mut taken = vec![false; events.len()];
    for (n, event) in events.iter().enumerate() {
        let Event::Key { key, physical_key, pressed: true, modifiers, .. } = event else { continue };
        let Some(shortcut) = state.shortcut(&press(*key, *physical_key, *modifiers), typing) else { continue };
        taken[n] = true;
        // the character the key typed goes with it
        if let Some(Event::Text(_)) = events.get(n + 1) {
            taken[n + 1] = true;
        }
        match shortcut {
            Shortcut::Act(action) => actions.push(action),
            Shortcut::FocusSearch => {
                ctx.data_mut(|d| d.insert_temp(focus_search_id(), true));
            }
            Shortcut::Nothing => {}
        }
    }
    if taken.contains(&true) {
        ctx.input_mut(|i| {
            let mut n = 0;
            i.events.retain(|_| {
                n += 1;
                !taken[n - 1]
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::state::Startup;
    use crate::state::shortcuts::Open;

    fn state() -> AppState {
        AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, [1440.0, 900.0])
    }

    fn key_event(key: Key, modifiers: egui::Modifiers) -> Event {
        Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }
    }

    /// One egui pass over `events` drawing the shell: the actions and what reached the UI after it.
    fn pass(ctx: &Context, state: &AppState, events: Vec<Event>) -> (Vec<Action>, Vec<Event>) {
        let mut actions = Vec::new();
        let mut left = Vec::new();
        let raw = egui::RawInput { events, time: Some(0.0), ..Default::default() };
        let mut output = ctx.run_ui(raw, |ui| {
            show(ui.ctx(), state, 0.0, &mut actions, &Slots::default());
            left = ui.ctx().input(|i| i.events.clone());
        });
        output.textures_delta.clear();
        (actions, left)
    }

    #[test]
    fn egui_keys_as_the_page_reads_them() {
        let none = egui::Modifiers::default();
        let shift = egui::Modifiers { shift: true, ..Default::default() };
        assert_eq!(press(Key::P, Some(Key::P), none).key, Typed::Char('p'));
        assert_eq!(press(Key::P, Some(Key::P), shift).key, Typed::Char('P'));
        assert_eq!(press(Key::Slash, None, none).key, Typed::Char('/'));
        assert_eq!(press(Key::Questionmark, Some(Key::Slash), shift).key, Typed::Char('?'));
        assert_eq!(press(Key::Num1, None, none).key, Typed::Char('1'));
        assert_eq!(press(Key::Escape, None, none).key, Typed::Escape);
        assert_eq!(press(Key::ArrowLeft, None, none).key, Typed::Other);
        assert_eq!(press(Key::F5, None, none).key, Typed::Other);
        let ctrl_shift = egui::Modifiers { ctrl: true, shift: true, command: true, ..Default::default() };
        let p = press(Key::E, Some(Key::D), ctrl_shift);
        assert!(p.key_d && p.modifiers.ctrl && p.modifiers.shift && !p.modifiers.meta);
        let cmd = egui::Modifiers { mac_cmd: true, command: true, ..Default::default() };
        assert!(press(Key::F, None, cmd).modifiers.meta);
    }

    #[test]
    fn a_shortcut_is_taken_out_of_egui_s_input_with_its_text() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let s = state();
        let none = egui::Modifiers::default();
        let (actions, left) = pass(
            &ctx,
            &s,
            vec![key_event(Key::F, none), Event::Text("f".into()), key_event(Key::Num2, none), Event::Text("2".into()), key_event(Key::X, none), Event::Text("x".into())],
        );
        assert_eq!(actions, [Action::ToggleOpen(Open::Filters), Action::SetBasemap(Basemap::Satellite)]);
        // x is no shortcut: it stays for the UI
        assert_eq!(left, [key_event(Key::X, none), Event::Text("x".into())]);
        // `/` asks the search box for the keyboard, and does not type itself
        let (actions, left) = pass(&ctx, &s, vec![key_event(Key::Slash, none), Event::Text("/".into())]);
        assert!(actions.is_empty());
        assert!(left.is_empty());
        assert!(take_focus_search(&ctx));
        assert!(!take_focus_search(&ctx), "taken once");
    }

    #[test]
    fn the_panels_drawn_and_the_controls_inset() {
        let mut s = state();
        assert!(panels(&s).is_empty());
        assert!(!detail_open(&s));
        assert_eq!(controls_inset(&s), 0.0);
        s.airport.open("ARN", 0.0);
        assert_eq!(panels(&s), [Panel::Airport]);
        assert!(detail_open(&s));
        assert_eq!(controls_inset(&s), 372.0);
    }
}
