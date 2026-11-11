//! The page's keys and what they open and close (doc 06 §3.11, §3.13; `+page.svelte:1133-1147`,
//! `1202-1265`): the filter panel, the shortcuts help, the keys dialog, the map controls' menus and
//! the dev panel, the keyboard shortcuts turned into [`Action`]s, and `closeTop` (Escape, Android's
//! back).
//!
//! **What is open.** [`Shortcuts`] holds the page's `filtersOpen`, `helpOpen`, `keysOpen`, `menu`
//! and `devOpen`. The UI reads them here and changes them with [`Action::Open`],
//! [`Action::ToggleOpen`] and [`Action::SetMenu`] (the Filters button and the filter panel's close
//! (U9, U14, U15), the map controls' buttons and menus (U16), the dialogs' close (U17), the dev
//! panel (U20)). Closing the keys dialog asks the Trafikverket key again (`closeKeys` →
//! `checkTrafikverketKey`). The phone sheet's detent (`sheetDetent`) is item U19's: [`AppState::close_top`]
//! lowers it there, last.
//!
//! **The keys** ([`AppState::shortcut`], the page's `onkeydown`), in its order:
//! 1. dev builds: Ctrl/Cmd+Shift+D (by the key's place, `code === "KeyD"`) shows or hides the dev
//!    panel;
//! 2. nothing while typing in a text field, while the keys dialog is open (it is modal and takes its
//!    own keys, Escape included), or with Ctrl, Cmd or Alt held;
//! 3. Escape closes the top thing ([`Action::CloseTop`]);
//! 4. while the shortcuts help is open only `?` works;
//! 5. a letter with Shift is its own shortcut (`P`, `I`, `S`), Caps Lock alone counts as the plain
//!    letter; then the table:
//!
//! | Key | Action |
//! |---|---|
//! | `/` | the search box takes the keyboard ([`Shortcut::FocusSearch`]) |
//! | `?` | the shortcuts help, open or closed |
//! | `f` | the filter panel, open or closed |
//! | `c` | a flight selected: the camera follows it or lets go; else a train or ship selected: the same for it ([`Action::ToggleFollow`]) |
//! | `1` `2` `3` | basemap dark, satellite, realistic |
//! | `l` `b` | place labels, 3D buildings |
//! | `p` `i` `s` | the focus: aircraft, trains (only while offered), ships |
//! | `P` `I` `S` | that kind also on the map or not (`I` only while trains are offered) |
//! | `k` | aircraft labels (only while aircraft are on the map) |
//! | `d` `w` `r` `e` | day and night, weather radar, rail map, terrain |
//!
//! A key of the table is taken (`preventDefault`) even when its action does nothing now (`i`
//! without trains, `c` with nothing selected): [`Shortcut::Nothing`].
//!
//! The camera's keys `t`, `n`, `+`, `=`, `-` and the arrows are `crate::input`'s (item B8), which
//! reads the same gate from [`crate::input::KeyGate`]: the app sets its `typing` from egui's focused
//! text field and its `shortcuts_off` from [`Shortcuts::shortcuts_off`] after each egui pass.
//!
//! Deviations:
//! - Ctrl/Cmd+Alt+I (the web inspector) has no counterpart and does nothing (doc 06 §30.5).
//! - The keys are read from egui's events in the UI pass (`crate::ui::shell`), so a key takes
//!   effect in the frame after it is pressed, as every action does (plan 2.2).
//! - Today one `followingVehicle` flag serves the train and the ship; here each keeps its own
//!   (`Trains::following`, `Ships::following`), and `c` turns the selected one's.

use rustradar_core::geo::Basemap;
use rustradar_core::settings::{Mode, shown_kinds};

use super::flight::SelectOptions;
use super::{Action, AppState, Cx};
use crate::input::Modifiers;

/// The map controls' menu that is open (`menu`, item U16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Layers,
    Settings,
}

/// What the page opens and closes with a flag of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Open {
    /// The filter panel (`filtersOpen`): key F, the Filters button.
    Filters,
    /// The shortcuts help (`helpOpen`): key ?, the map controls' keyboard button.
    Help,
    /// The keys dialog (`keysOpen`): the map controls' key button, a panel's sign-in prompt.
    Keys,
    /// The dev panel (`devOpen`, dev builds): Ctrl/Cmd+Shift+D.
    Dev,
}

/// A setting the keys (and the menus) turn on and off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    /// `l`: place labels.
    Labels,
    /// `b`: 3D buildings.
    Buildings,
    /// `k`: callsigns next to the aircraft.
    AircraftLabels,
    /// `d`: day and night shading.
    Daylight,
    /// `w`: the weather radar.
    Weather,
    /// `r`: the rail map.
    RailMap,
    /// `e`: terrain.
    Terrain,
}

/// What a key typed, as the DOM's `key` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typed {
    /// The character the key types with the modifiers held (`p`, `P`, `?`, `1`).
    Char(char),
    Escape,
    /// Anything else (a named key the table has nothing for).
    Other,
}

/// A key pressed (a `keydown`, repeats included).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Press {
    pub key: Typed,
    /// The key in the D key's place (`e.code === "KeyD"`), whatever it types.
    pub key_d: bool,
    pub modifiers: Modifiers,
}

impl Press {
    /// `key` pressed with no modifier.
    pub fn plain(key: Typed) -> Press {
        Press { key, key_d: false, modifiers: Modifiers::default() }
    }
}

/// What a shortcut does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shortcut {
    /// Carry out this action (after the UI pass, with the others).
    Act(Action),
    /// `/`: the search box takes the keyboard (the UI does it, in the same pass).
    FocusSearch,
    /// A key of the table whose action does nothing now: taken all the same.
    Nothing,
}

/// What the page's keys open and close (see the module documentation).
#[derive(Debug, Default)]
pub struct Shortcuts {
    filters_open: bool,
    help_open: bool,
    keys_open: bool,
    menu: Option<Menu>,
    dev_open: bool,
}

impl Shortcuts {
    /// Whether `what` is open.
    pub fn is_open(&self, what: Open) -> bool {
        match what {
            Open::Filters => self.filters_open,
            Open::Help => self.help_open,
            Open::Keys => self.keys_open,
            Open::Dev => self.dev_open,
        }
    }

    /// The filter panel is open (`filtersOpen`).
    pub fn filters_open(&self) -> bool {
        self.filters_open
    }

    /// The shortcuts help is open (`helpOpen`).
    pub fn help_open(&self) -> bool {
        self.help_open
    }

    /// The keys dialog is open (`keysOpen`).
    pub fn keys_open(&self) -> bool {
        self.keys_open
    }

    /// The map controls' menu that is open (`menu`).
    pub fn menu(&self) -> Option<Menu> {
        self.menu
    }

    /// The dev panel is open (`devOpen`).
    pub fn dev_open(&self) -> bool {
        self.dev_open
    }

    /// Whether the map's own keys are off ([`crate::input::KeyGate::shortcuts_off`]): the keys
    /// dialog is modal, and the help lets only `?` and Escape through.
    pub fn shortcuts_off(&self) -> bool {
        self.keys_open || self.help_open
    }

    /// This frame's step (see [`super::AppState::poll`] for the order): nothing runs on a clock
    /// here; the keys come with the UI pass and change things through actions.
    pub fn poll(&mut self, _cx: &mut Cx<'_>) {}

    fn set(&mut self, what: Open, open: bool) -> bool {
        let flag = match what {
            Open::Filters => &mut self.filters_open,
            Open::Help => &mut self.help_open,
            Open::Keys => &mut self.keys_open,
            Open::Dev => &mut self.dev_open,
        };
        std::mem::replace(flag, open) != open
    }
}

/// The table's key for `typed` with Shift held or not: a letter with Shift is its upper case, any
/// other letter its lower case (so Caps Lock alone counts as the plain letter); anything else as
/// typed (`+page.svelte:1259`).
pub fn table_key(typed: char, shift: bool) -> char {
    let mut lower = typed.to_lowercase();
    let mut upper = typed.to_uppercase();
    let (l, u) = (lower.next(), upper.next());
    let single = lower.next().is_none() && upper.next().is_none();
    match (l, u) {
        (Some(l), Some(u)) if single && l != u => {
            if shift {
                u
            } else {
                l
            }
        }
        _ => typed,
    }
}

impl AppState {
    /// The shortcut `press` makes (the page's `onkeydown`; see the module documentation), with
    /// `typing` when a text field has the keyboard. `None`: not a shortcut, the key goes on to
    /// whatever else wants it.
    pub fn shortcut(&self, press: &Press, typing: bool) -> Option<Shortcut> {
        let m = press.modifiers;
        if cfg!(debug_assertions) && (m.ctrl || m.meta) && m.shift && press.key_d {
            return Some(Shortcut::Act(Action::ToggleOpen(Open::Dev)));
        }
        // the keys page is modal: it handles its own keys, Escape included
        if typing || self.shortcuts.keys_open || m.ctrl || m.meta || m.alt {
            return None;
        }
        let typed = match press.key {
            Typed::Escape => return Some(Shortcut::Act(Action::CloseTop)),
            Typed::Char(c) => c,
            Typed::Other => return None,
        };
        if self.shortcuts.help_open && typed != '?' {
            return None;
        }
        let settings = &self.persisted.settings;
        let offered = self.trains.offered();
        let act = |action: Action| Some(Shortcut::Act(action));
        let when = |ok: bool, action: Action| Some(if ok { Shortcut::Act(action) } else { Shortcut::Nothing });
        match table_key(typed, m.shift) {
            '/' => Some(Shortcut::FocusSearch),
            '?' => act(Action::ToggleOpen(Open::Help)),
            'f' => act(Action::ToggleOpen(Open::Filters)),
            'c' => {
                let selected = self.flight.selected_id().is_some() || self.trains.selected().is_some() || self.ships.selected().is_some();
                when(selected, Action::ToggleFollow)
            }
            '1' => act(Action::SetBasemap(Basemap::Dark)),
            '2' => act(Action::SetBasemap(Basemap::Satellite)),
            '3' => act(Action::SetBasemap(Basemap::Realistic)),
            'l' => act(Action::Toggle(Toggle::Labels)),
            'b' => act(Action::Toggle(Toggle::Buildings)),
            // the focus: Planes, traIns (T tilts), Ships; with Shift, that kind also shown on the map or not
            'p' => act(Action::SetMode(Mode::Aircraft)),
            'i' => when(offered, Action::SetMode(Mode::Trains)),
            's' => act(Action::SetMode(Mode::Ships)),
            'P' => act(Action::ToggleShown(Mode::Aircraft)),
            'I' => when(offered, Action::ToggleShown(Mode::Trains)),
            'S' => act(Action::ToggleShown(Mode::Ships)),
            'k' => when(shown_kinds(settings.mode, &settings.also_show).aircraft, Action::Toggle(Toggle::AircraftLabels)),
            'd' => act(Action::Toggle(Toggle::Daylight)),
            'w' => act(Action::Toggle(Toggle::Weather)),
            'r' => act(Action::Toggle(Toggle::RailMap)),
            'e' => act(Action::Toggle(Toggle::Terrain)),
            _ => None,
        }
    }

    /// `closeTop()` at local time `now_ms` (Escape, Android's back gesture): closes the first of
    /// the keys dialog, the shortcuts help, the map controls' menu, the place marker, the selected
    /// flight, ship and train, the airport board and the filter panel that is open. Returns
    /// whether anything was (on Android the app leaves when nothing was). The phone sheet's
    /// lowering to its peek detent comes last, with item U19.
    pub fn close_top(&mut self, now_ms: f64) -> bool {
        if self.shortcuts.keys_open {
            self.set_open(Open::Keys, false);
        } else if self.shortcuts.help_open {
            self.shortcuts.help_open = false;
        } else if self.shortcuts.menu.is_some() {
            self.shortcuts.menu = None;
        } else if self.flight.place().is_some() {
            self.flight.set_place(None);
        } else if self.flight.selected_id().is_some() {
            self.select_flight(None, SelectOptions::default(), now_ms);
        } else if self.ships.selected_mmsi().is_some() {
            self.select_ship(None, now_ms);
        } else if self.trains.selected_id().is_some() {
            self.select_train(None, now_ms);
        } else if self.airport.code().is_some() {
            self.airport.close();
        } else if self.shortcuts.filters_open {
            self.shortcuts.filters_open = false;
        } else {
            return false;
        }
        true
    }

    /// Whether [`Self::close_top`] would close something (the page's `closable`: Android keeps a
    /// history entry for the back gesture while it is true).
    pub fn closable(&self) -> bool {
        let s = &self.shortcuts;
        s.keys_open
            || s.help_open
            || s.menu.is_some()
            || self.flight.place().is_some()
            || self.flight.selected_id().is_some()
            || self.ships.selected_mmsi().is_some()
            || self.trains.selected_id().is_some()
            || self.airport.code().is_some()
            || s.filters_open
    }

    /// Opens or closes `what`; closing the keys dialog asks the Trafikverket key again
    /// (`closeKeys`). Returns whether it changed.
    pub fn set_open(&mut self, what: Open, open: bool) -> bool {
        let changed = self.shortcuts.set(what, open);
        if changed && what == Open::Keys && !open {
            self.trains.check_key();
        }
        changed
    }

    /// The map controls' menu open, or none.
    pub fn set_menu(&mut self, menu: Option<Menu>) -> bool {
        std::mem::replace(&mut self.shortcuts.menu, menu) != menu
    }

    /// The key C, the panels' Follow (`toggleFollow` with a flight selected, else
    /// `followingVehicle = !followingVehicle` with a train or a ship): the camera follows what is
    /// selected or lets go. Returns whether anything was selected.
    pub fn toggle_following(&mut self, now_ms: f64) -> bool {
        if self.flight.selected_id().is_some() {
            self.toggle_follow(now_ms);
        } else if self.trains.selected().is_some() {
            self.trains.toggle_following();
        } else if self.ships.selected().is_some() {
            self.ships.toggle_following();
        } else {
            return false;
        }
        true
    }

    /// Turns `what` on or off in the settings (saved and drawn from the next poll on).
    pub fn toggle_setting(&mut self, what: Toggle) -> bool {
        let s = &mut self.persisted.settings;
        let flag = match what {
            Toggle::Labels => &mut s.labels,
            Toggle::Buildings => &mut s.buildings,
            Toggle::AircraftLabels => &mut s.aircraft_labels,
            Toggle::Daylight => &mut s.daylight,
            Toggle::Weather => &mut s.weather,
            Toggle::RailMap => &mut s.rail_map,
            Toggle::Terrain => &mut s.terrain,
        };
        *flag = !*flag;
        true
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::model::{LiveFlight, Ship, Train, TrainKind, TrainSource};
    use rustradar_scene::camera::ViewState;

    use super::super::flight::MarkedPlace;
    use super::super::{Action, Startup, trains};
    use super::*;
    use crate::persist::tests::TempDir;

    const T0: f64 = 1.782e12;

    fn state() -> AppState {
        AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, [1440.0, 900.0])
    }

    fn key(c: char) -> Press {
        Press::plain(Typed::Char(c))
    }

    fn shift(c: char) -> Press {
        Press { modifiers: Modifiers { shift: true, ..Default::default() }, ..key(c) }
    }

    fn act(s: &AppState, press: Press) -> Option<Action> {
        match s.shortcut(&press, false) {
            Some(Shortcut::Act(a)) => Some(a),
            _ => None,
        }
    }

    fn flight(id: u32) -> LiveFlight {
        LiveFlight { id, lat: 59.65, lon: 17.92, callsign: "SAS123".into(), timestamp_ms: (T0 - 1000.0) as u64, ..Default::default() }
    }

    fn ship(mmsi: u32) -> Ship {
        Ship { mmsi, name: "STENA DANICA".into(), lat: 57.7, lon: 11.9, ..Default::default() }
    }

    fn train(id: &str) -> Train {
        Train {
            id: id.into(),
            source: TrainSource::Trafikverket,
            name: "Pendeltåg 2245".into(),
            kind: TrainKind::Suburban,
            lat: 59.33,
            lon: 18.06,
            heading: Some(90.0),
            speed_kmh: Some(60.0),
            destination: None,
            operator: Some("SL".into()),
            delay_minutes: Some(0),
            position_unix: (T0 / 1000.0) as i64,
            frames: Vec::new(),
            reports: Vec::new(),
            predicted: Vec::new(),
            cars: None,
            tail: Vec::new(),
            structures: Vec::new(),
            ended_unix: None,
            shape: None,
            model: None,
        }
    }

    /// The demo's trains: Trafikverket "said yes" to the key, the trains are offered.
    fn offer_trains(s: &mut AppState) {
        s.trains = trains::Trains::demo();
        s.poll(T0);
        assert!(s.trains.offered());
    }

    #[test]
    fn the_table_of_keys() {
        let s = state();
        assert_eq!(s.shortcut(&key('/'), false), Some(Shortcut::FocusSearch));
        assert_eq!(act(&s, key('?')), Some(Action::ToggleOpen(Open::Help)));
        assert_eq!(act(&s, key('f')), Some(Action::ToggleOpen(Open::Filters)));
        assert_eq!(act(&s, key('1')), Some(Action::SetBasemap(Basemap::Dark)));
        assert_eq!(act(&s, key('2')), Some(Action::SetBasemap(Basemap::Satellite)));
        assert_eq!(act(&s, key('3')), Some(Action::SetBasemap(Basemap::Realistic)));
        assert_eq!(act(&s, key('l')), Some(Action::Toggle(Toggle::Labels)));
        assert_eq!(act(&s, key('b')), Some(Action::Toggle(Toggle::Buildings)));
        assert_eq!(act(&s, key('p')), Some(Action::SetMode(Mode::Aircraft)));
        assert_eq!(act(&s, key('s')), Some(Action::SetMode(Mode::Ships)));
        assert_eq!(act(&s, shift('P')), Some(Action::ToggleShown(Mode::Aircraft)));
        assert_eq!(act(&s, shift('S')), Some(Action::ToggleShown(Mode::Ships)));
        assert_eq!(act(&s, key('k')), Some(Action::Toggle(Toggle::AircraftLabels)));
        assert_eq!(act(&s, key('d')), Some(Action::Toggle(Toggle::Daylight)));
        assert_eq!(act(&s, key('w')), Some(Action::Toggle(Toggle::Weather)));
        assert_eq!(act(&s, key('r')), Some(Action::Toggle(Toggle::RailMap)));
        assert_eq!(act(&s, key('e')), Some(Action::Toggle(Toggle::Terrain)));
        assert_eq!(s.shortcut(&Press::plain(Typed::Escape), false), Some(Shortcut::Act(Action::CloseTop)));
        // the camera's keys are crate::input's; other keys are nobody's
        for c in ['t', 'n', '+', '=', '-', 'x', 'q', '4', '!'] {
            assert_eq!(s.shortcut(&key(c), false), None, "{c:?}");
        }
        assert_eq!(s.shortcut(&Press::plain(Typed::Other), false), None);
    }

    #[test]
    fn shift_makes_a_letter_its_own_shortcut_and_caps_lock_alone_does_not() {
        let s = state();
        // Shift+p types "P": the focus does not change, aircraft go on or off the map
        assert_eq!(act(&s, shift('P')), Some(Action::ToggleShown(Mode::Aircraft)));
        // Caps Lock alone types "P" without Shift: the plain p
        assert_eq!(act(&s, key('P')), Some(Action::SetMode(Mode::Aircraft)));
        // Shift with Caps Lock types "p": still Shift+P
        assert_eq!(act(&s, shift('p')), Some(Action::ToggleShown(Mode::Aircraft)));
        // Shift+L and Shift+F are no shortcut of their own
        assert_eq!(act(&s, shift('L')), None);
        assert_eq!(act(&s, shift('F')), None);
        // not letters: as typed, Shift or not
        assert_eq!(act(&s, shift('?')), Some(Action::ToggleOpen(Open::Help)));
        assert_eq!(table_key('/', true), '/');
        assert_eq!(table_key('é', true), 'É');
        assert_eq!(table_key('ß', true), 'ß', "no single upper case: as typed");
    }

    #[test]
    fn nothing_while_typing_with_the_keys_dialog_or_with_ctrl_cmd_alt() {
        let mut s = state();
        assert_eq!(s.shortcut(&key('f'), true), None, "typing in a field");
        assert_eq!(s.shortcut(&Press::plain(Typed::Escape), true), None, "Escape is the field's");
        for m in [
            Modifiers { ctrl: true, ..Default::default() },
            Modifiers { meta: true, ..Default::default() },
            Modifiers { alt: true, ..Default::default() },
        ] {
            assert_eq!(s.shortcut(&Press { modifiers: m, ..key('f') }, false), None, "{m:?}");
        }
        // Ctrl/Cmd+Alt+I (the web inspector) is nothing here
        let inspector = Press { modifiers: Modifiers { ctrl: true, alt: true, ..Default::default() }, ..key('i') };
        assert_eq!(s.shortcut(&inspector, false), None);
        assert!(s.set_open(Open::Keys, true));
        assert_eq!(s.shortcut(&key('f'), false), None);
        assert_eq!(s.shortcut(&Press::plain(Typed::Escape), false), None, "the dialog takes its own Escape");
        assert_eq!(s.shortcut(&key('?'), false), None);
        assert!(s.shortcuts.shortcuts_off());
    }

    #[test]
    fn with_the_help_open_only_the_question_mark_and_escape_work() {
        let mut s = state();
        assert!(s.apply(Action::ToggleOpen(Open::Help), T0));
        assert!(s.shortcuts.help_open());
        assert!(s.shortcuts.shortcuts_off(), "the map's keys are off too");
        assert_eq!(s.shortcut(&key('f'), false), None);
        assert_eq!(s.shortcut(&key('/'), false), None);
        assert_eq!(act(&s, key('?')), Some(Action::ToggleOpen(Open::Help)));
        assert_eq!(act(&s, Press::plain(Typed::Escape)), Some(Action::CloseTop));
        assert!(s.apply(Action::ToggleOpen(Open::Help), T0));
        assert!(!s.shortcuts.help_open());
        assert!(!s.shortcuts.shortcuts_off());
    }

    #[test]
    fn the_dev_panel_by_the_d_keys_place_in_dev_builds() {
        let mut s = state();
        let ctrl_shift = Modifiers { ctrl: true, shift: true, ..Default::default() };
        // an AZERTY or Dvorak D key types something else; its place is D
        let press = Press { key: Typed::Char('E'), key_d: true, modifiers: ctrl_shift };
        let cmd = Press { modifiers: Modifiers { meta: true, shift: true, ..Default::default() }, ..press };
        if cfg!(debug_assertions) {
            assert_eq!(s.shortcut(&press, false), Some(Shortcut::Act(Action::ToggleOpen(Open::Dev))));
            assert_eq!(s.shortcut(&cmd, false), Some(Shortcut::Act(Action::ToggleOpen(Open::Dev))));
            // before the typing check, as today
            assert_eq!(s.shortcut(&press, true), Some(Shortcut::Act(Action::ToggleOpen(Open::Dev))));
            assert!(s.apply(Action::ToggleOpen(Open::Dev), T0));
            assert!(s.shortcuts.dev_open());
        } else {
            assert_eq!(s.shortcut(&press, false), None);
        }
        // without Shift, or a plain d: day and night
        assert_eq!(s.shortcut(&Press { modifiers: Modifiers { ctrl: true, ..Default::default() }, ..press }, false), None);
        assert_eq!(act(&s, Press { key: Typed::Char('d'), key_d: true, modifiers: Modifiers::default() }), Some(Action::Toggle(Toggle::Daylight)));
    }

    #[test]
    fn the_trains_keys_wait_for_trafikverket() {
        let mut s = state();
        assert!(!s.trains.offered());
        assert_eq!(s.shortcut(&key('i'), false), Some(Shortcut::Nothing), "taken, does nothing");
        assert_eq!(s.shortcut(&shift('I'), false), Some(Shortcut::Nothing));
        offer_trains(&mut s);
        assert_eq!(act(&s, key('i')), Some(Action::SetMode(Mode::Trains)));
        assert_eq!(act(&s, shift('I')), Some(Action::ToggleShown(Mode::Trains)));
        assert!(s.apply(Action::SetMode(Mode::Trains), T0));
        assert_eq!(s.persisted.settings.mode, Mode::Trains);
        // the aircraft labels only while aircraft are on the map
        assert_eq!(s.shortcut(&key('k'), false), Some(Shortcut::Nothing));
        assert!(s.apply(Action::ToggleShown(Mode::Aircraft), T0));
        assert_eq!(act(&s, key('k')), Some(Action::Toggle(Toggle::AircraftLabels)));
    }

    #[test]
    fn the_settings_keys_turn_their_setting() {
        let mut s = state();
        let before = s.persisted.settings.clone();
        for t in [Toggle::Labels, Toggle::Buildings, Toggle::AircraftLabels, Toggle::Daylight, Toggle::Weather, Toggle::RailMap, Toggle::Terrain] {
            assert!(s.apply(Action::Toggle(t), T0));
        }
        let after = &s.persisted.settings;
        assert_eq!(after.labels, !before.labels);
        assert_eq!(after.buildings, !before.buildings);
        assert_eq!(after.aircraft_labels, !before.aircraft_labels);
        assert_eq!(after.daylight, !before.daylight);
        assert_eq!(after.weather, !before.weather);
        assert_eq!(after.rail_map, !before.rail_map);
        assert_eq!(after.terrain, !before.terrain);
        assert!(s.apply(Action::SetBasemap(Basemap::Satellite), T0));
        assert_eq!(s.basemap(), Basemap::Satellite);
        assert!(!s.apply(Action::SetBasemap(Basemap::Satellite), T0), "the same basemap again");
    }

    #[test]
    fn c_follows_the_selected_flight_with_the_camera_lock() {
        let mut s = state();
        assert_eq!(s.shortcut(&key('c'), false), Some(Shortcut::Nothing), "nothing selected");
        assert!(!s.apply(Action::ToggleFollow, T0));
        s.select_flight(Some(42), SelectOptions { hint: Some(flight(42)), ..Default::default() }, T0);
        assert_eq!(act(&s, key('c')), Some(Action::ToggleFollow));
        assert!(!s.flight.following());
        assert!(s.apply(Action::ToggleFollow, T0));
        assert!(s.flight.following());
        // handed to the camera: it locks on the aircraft (state::camera)
        assert!(s.camera.follow());
        assert!(s.camera.following(), "follow && selected");
        s.advance_camera(T0 + 16.0);
        assert!(s.controller.locked(), "the camera's controller is locked on it");
        assert!(s.apply(Action::ToggleFollow, T0 + 1000.0));
        assert!(!s.flight.following());
        s.advance_camera(T0 + 1016.0);
        assert!(!s.controller.locked());
    }

    #[test]
    fn c_follows_the_selected_train_or_ship() {
        let mut s = state();
        offer_trains(&mut s);
        s.select_train(Some(train("trafikverket:2245")), T0);
        assert_eq!(act(&s, key('c')), Some(Action::ToggleFollow));
        assert!(s.apply(Action::ToggleFollow, T0));
        assert!(s.trains.following());
        assert!(s.apply(Action::ToggleFollow, T0));
        assert!(!s.trains.following());
        s.select_train(None, T0);
        s.select_ship(Some(ship(265_000_001)), T0);
        assert_eq!(act(&s, key('c')), Some(Action::ToggleFollow));
        assert!(s.apply(Action::ToggleFollow, T0));
        assert!(s.ships.following());
    }

    #[test]
    fn escape_closes_the_top_thing_in_todays_order() {
        let mut s = state();
        assert!(!s.closable());
        assert!(!s.close_top(T0), "nothing open");
        // everything open at once, bottom to top: the filters, the board, a flight opened from it
        // (the board stays under it), the searched place, a menu, the help and the keys dialog
        assert!(s.apply(Action::ToggleOpen(Open::Filters), T0));
        s.airport.open("arn", T0);
        s.select_flight(Some(42), SelectOptions { hint: Some(flight(42)), from_board: true, ..Default::default() }, T0);
        assert_eq!(s.airport.code(), Some("ARN"), "kept under the flight");
        s.flight.set_place(Some(MarkedPlace { lon: 18.07, lat: 59.33, name: "Stockholm".into() }));
        assert!(s.apply(Action::SetMenu(Some(Menu::Layers)), T0));
        assert!(s.apply(Action::Open(Open::Help, true), T0));
        assert!(s.apply(Action::Open(Open::Keys, true), T0));
        assert!(s.closable());

        let mut closed = Vec::new();
        while s.closable() {
            let before = (
                s.shortcuts.keys_open(),
                s.shortcuts.help_open(),
                s.shortcuts.menu().is_some(),
                s.flight.place().is_some(),
                s.flight.selected_id().is_some(),
                s.airport.code().is_some(),
                s.shortcuts.filters_open(),
            );
            assert!(s.apply(Action::CloseTop, T0));
            closed.push(match before {
                (true, ..) => "keys",
                (_, true, ..) => "help",
                (_, _, true, ..) => "menu",
                (_, _, _, true, ..) => "place",
                (_, _, _, _, true, ..) => "flight",
                (_, _, _, _, _, true, _) => "board",
                _ => "filters",
            });
        }
        assert_eq!(closed, ["keys", "help", "menu", "place", "flight", "board", "filters"]);
        assert!(!s.apply(Action::CloseTop, T0), "then nothing");
    }

    #[test]
    fn escape_closes_a_ship_before_a_train_and_both_before_the_board() {
        let mut s = state();
        s.select_ship(Some(ship(1)), T0);
        assert!(s.close_top(T0));
        assert_eq!(s.ships.selected_mmsi(), None);
        offer_trains(&mut s);
        s.select_train(Some(train("trafikverket:1")), T0);
        assert!(s.close_top(T0));
        assert_eq!(s.trains.selected_id(), None);
        assert!(!s.close_top(T0));
    }

    #[test]
    fn closing_the_keys_dialog_asks_the_key_again() {
        // a fixture folder answers the key check "ok" once it holds trains
        let dir = TempDir::new("shortcuts-keys");
        let startup = Startup { fixture: Some(dir.0.clone()), ..Default::default() };
        let mut s = AppState::new(startup, None, None, None, ViewState::INITIAL, [1440.0, 900.0]);
        s.poll(T0);
        s.poll(T0 + 16.0);
        assert!(!s.trains.offered(), "no key");
        assert!(s.apply(Action::Open(Open::Keys, true), T0));
        assert!(!s.apply(Action::Open(Open::Keys, true), T0), "already open");
        // a key is given in the dialog
        std::fs::write(dir.0.join(trains::FIXTURE_TRAINS), serde_json::to_vec(&vec![train("trafikverket:1")]).unwrap()).unwrap();
        s.poll(T0 + 1000.0);
        assert!(!s.trains.offered(), "asked only when the dialog closes");
        assert!(s.apply(Action::CloseTop, T0 + 2000.0));
        assert!(!s.shortcuts.keys_open());
        s.poll(T0 + 2016.0);
        s.poll(T0 + 2032.0);
        assert!(s.trains.offered(), "checkTrafikverketKey");
        assert!(!s.apply(Action::SetMenu(None), T0));
        assert!(s.apply(Action::SetMenu(Some(Menu::Settings)), T0));
        assert_eq!(s.shortcuts.menu(), Some(Menu::Settings));
    }
}
