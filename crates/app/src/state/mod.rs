//! The app's state machine (plan 2.2 steps 1–3, doc 06 §3): one [`AppState`] on the main thread,
//! with one file per feed or concern.
//!
//! Each frame the window (and the headless path) calls, in this order:
//! 1. [`AppState::poll`]: drains the backend's events, then lets every part take its answers and
//!    start the requests whose time has come, through a [`Cx`].
//! 2. [`AppState::advance_camera`]: the running transition, then the camera's own per-frame work
//!    (follow lock, ground height) in [`camera`].
//! 3. The scene builders read the state ([`crate::scene_builder`]); the UI reads it and returns
//!    [`Action`]s, applied with [`AppState::apply`].
//!
//! The parts and who fills them (each file starts as a stub that holds nothing and does nothing):
//!
//! | Part | Concern | Item |
//! |---|---|---|
//! | [`settings`] | `settings.json`: load, debounced save | U5 |
//! | [`shortcuts`] | keyboard shortcuts → actions | U6 |
//! | [`camera`] | camera lock on an aircraft, ground height under the camera | A10, Z4 |
//! | [`pick`] | hover and click on the map | A9 |
//! | [`aircraft`] | the aircraft feed, top flights, reference data | A1 |
//! | [`flight`] | the selected flight: follow stream, OpenSky, adsb.lol | A8 |
//! | [`liveries`] | airline livery colours | A2 |
//! | [`taxiways`] | taxiway networks for the motion model | A11 |
//! | [`airport`] | the airport board | U13 |
//! | [`trains`] | trains in view, the selected train, its route | T1 |
//! | [`blocks`] | signal blocks | T5 |
//! | [`ships`] | ships in view, the selected ship | H1 |
//! | [`weather`] | live weather for the realistic sky | R11 |
//! | [`radar`] | weather radar frames | W5 |
//! | [`search`] | the search box's remote search and geocoding | U8 |

pub mod aircraft;
pub mod airport;
pub mod basemap;
pub mod blocks;
pub mod camera;
pub mod flight;
pub mod liveries;
pub mod pick;
pub mod radar;
pub mod search;
pub mod settings;
pub mod ships;
pub mod shortcuts;
pub mod taxiways;
pub mod trains;
pub mod weather;

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{SystemTime, UNIX_EPOCH};

use rustradar_backend::{Backend, BackendEvent};
use rustradar_core::filters::Filters;
use rustradar_core::geo::Basemap;
use rustradar_core::motion::aircraft::Motion;
use rustradar_core::settings::{Mode, Persisted};
use rustradar_scene::camera::{self as cam, ViewState};
use rustradar_scene::controller::Controller;

use crate::requests::Requests;

/// What the command line asked of the state (plan section 8).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Startup {
    /// The basemap to show, over the saved setting (`--basemap`). The settings store ([`settings`])
    /// keeps it when it loads the file.
    pub basemap: Option<Basemap>,
    /// Canned feeds from this folder instead of the network (`--fixture`): the feeds read it.
    pub fixture: Option<PathBuf>,
    /// The demo trains (`--demo`, today's `#demo`; doc 06 §3.9).
    pub demo: bool,
}

/// The two clocks of a frame (doc 03 §20.12), Unix milliseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clocks {
    /// The local clock: ships, trains, camera transitions, the realistic sky.
    pub local_ms: f64,
    /// The feed clock, synced to the aircraft feed's server ([`Motion::now`]): aircraft, their
    /// shadows, the night layer.
    pub feed_ms: f64,
}

/// The local clock: the system clock, or (`--now`) one that starts at a given time and runs on,
/// or one that stands still (shots).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clock {
    /// Added to the system clock.
    offset_ms: f64,
    /// A clock that stands still at this time.
    fixed_ms: Option<f64>,
}

/// The system clock, Unix milliseconds.
pub fn system_now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

impl Clock {
    /// The system clock.
    pub fn system() -> Self {
        Clock { offset_ms: 0.0, fixed_ms: None }
    }

    /// A clock that reads `unix_ms` now and runs on from there.
    pub fn starting_at(unix_ms: f64) -> Self {
        Clock { offset_ms: unix_ms - system_now_ms(), fixed_ms: None }
    }

    /// A clock that always reads `unix_ms`.
    pub fn fixed(unix_ms: f64) -> Self {
        Clock { offset_ms: 0.0, fixed_ms: Some(unix_ms) }
    }

    /// Unix milliseconds now.
    pub fn now_ms(&self) -> f64 {
        self.fixed_ms.unwrap_or_else(|| system_now_ms() + self.offset_ms)
    }
}

/// What a part of the state may ask for at the end of its step.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Wants {
    /// Rebuild the scene this frame (new data, a selection or a hover changed: plan 1.4.9).
    pub scene: bool,
    /// Come back no later than this (local Unix ms): the next feed request, a debounce, a retry.
    pub wake_at: Option<f64>,
}

impl Wants {
    /// Asks for a frame by `at_ms` at the latest; the earliest asked for wins.
    pub fn wake_at(&mut self, at_ms: f64) {
        self.wake_at = Some(self.wake_at.map_or(at_ms, |t| t.min(at_ms)));
    }

    /// Both asks.
    pub fn merge(&mut self, other: Wants) {
        self.scene |= other.scene;
        if let Some(at) = other.wake_at {
            self.wake_at(at);
        }
    }
}

/// What a part gets each frame: the clocks, the camera, the shared state, the backend and the way to
/// start requests, this frame's backend events, and where to put its [`Wants`].
pub struct Cx<'a> {
    pub clocks: Clocks,
    /// The camera: parts may move it (a fly-to when details arrive, the follow lock).
    pub controller: &'a mut Controller,
    /// The aircraft motion model (fed by the aircraft feed, asked by the scene builder).
    pub motion: &'a mut Motion,
    /// Settings and filters.
    pub persisted: &'a mut Persisted,
    /// `None` when the backend could not start (no folders): the map runs without feeds.
    pub backend: Option<&'a Backend>,
    /// `None` without a runtime (unit tests).
    pub requests: Option<&'a Requests>,
    pub startup: &'a Startup,
    /// The backend's events since the last frame, in order.
    pub events: &'a [BackendEvent],
    pub wants: &'a mut Wants,
}

/// What the UI and the keys ask the state to do (plan 2.2: actions, not callbacks). The UI
/// returns them from its pass; [`AppState::apply`] carries them out after it. Stage 7 adds the
/// rest (select, filter, open a panel, ...).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// Close the top thing (Escape, Android back; doc 06 §3.11).
    CloseTop,
    /// `zoomBy(delta)`: keys `+ = -`, the map controls.
    ZoomBy(f64),
    /// North up, looking straight down: key `n`, the compass.
    ResetOrientation,
    /// Tilt to `pitch` degrees: key `t`, the tilt button.
    SetPitch(f64),
    /// `flyTo(lon, lat, zoom?)`.
    FlyTo { longitude: f64, latitude: f64, zoom: Option<f64> },
    /// The focus (the mode bar, keys `p i s`; the trains only while offered): [`AppState::set_mode`].
    SetMode(Mode),
    /// Another kind also on the map or not (the mode bar's eye, `Shift+P I S`):
    /// [`AppState::toggle_shown`].
    ToggleShown(Mode),
    /// The camera follows the selected flight, train or ship, or lets go (key `c`, the panels'
    /// Follow): [`AppState::toggle_following`].
    ToggleFollow,
    /// Shows a basemap (keys `1 2 3`, the layers menu): [`AppState::set_basemap`].
    SetBasemap(Basemap),
    /// A setting on or off (keys `l b k d w r e`, the menus): [`AppState::toggle_setting`].
    Toggle(shortcuts::Toggle),
    /// Opens or closes the filter panel, the shortcuts help, the keys dialog or the dev panel
    /// ([`AppState::set_open`]).
    Open(shortcuts::Open, bool),
    /// The same, the other way from how it is (key `f`, the Filters button, key `?`).
    ToggleOpen(shortcuts::Open),
    /// The map controls' menu open, or none ([`AppState::set_menu`]).
    SetMenu(Option<shortcuts::Menu>),
    /// "Retry now" under the aircraft feed's error (`refresh()`).
    RetryFeed,
    /// The search box's typing, picks and clearing ([`AppState::search_ask`]).
    Search(search::Ask),
    /// A list's pick: `select(id, {fly: true})` (the most tracked list, U9).
    PickFlight(u32),
    /// The most tracked list's "Try again" (`refreshTop`).
    RetryTop,
    /// The aircraft filter panel's change: the filters it handed over in
    /// [`AppState::filters_handed`] replace `persisted.filters` (U9).
    SetFilters,
    /// The airport board's tabs, Show more, Try again, rows and close ([`AppState::board_ask`],
    /// U13).
    Board(airport::Ask),
}

/// The app's state: the camera, the clocks' sync, settings, and one part per feed.
pub struct AppState {
    pub startup: Startup,
    pub backend: Option<Backend>,
    pub requests: Option<Requests>,
    events: Option<Receiver<BackendEvent>>,
    pub persisted: Persisted,
    /// The camera (deck's controller and the app's moves).
    pub controller: Controller,
    pub motion: Motion,
    /// A newer version is ready (`BackendEvent::UpdateReady`), for the update toast.
    pub update_ready: Option<String>,
    pub settings: settings::Store,
    pub shortcuts: shortcuts::Shortcuts,
    pub camera: camera::Camera,
    pub pick: pick::Pick,
    pub aircraft: aircraft::Aircraft,
    pub flight: flight::Flight,
    pub liveries: liveries::Liveries,
    pub taxiways: taxiways::Taxiways,
    pub airport: airport::Airport,
    pub trains: trains::Trains,
    pub blocks: blocks::Blocks,
    pub ships: ships::Ships,
    pub weather: weather::Weather,
    pub radar: radar::Radar,
    pub search: search::Search,
    /// The aircraft filter panel's new filters, for the [`Action::SetFilters`] it pushes with
    /// them (the action is `Copy`).
    pub filters_handed: Cell<Option<Filters>>,
}

impl AppState {
    /// The state at start: the camera at `view` on a canvas of `size` CSS pixels, default settings
    /// with the command line's basemap, nothing loaded. `backend` and `events` are what
    /// `Backend::start` gave (none in tests or when it could not start).
    pub fn new(
        startup: Startup,
        backend: Option<Backend>,
        requests: Option<Requests>,
        events: Option<Receiver<BackendEvent>>,
        view: ViewState,
        size: [f64; 2],
    ) -> Self {
        // the saved settings from the backend's config folder, `--basemap` over them (U5)
        let (settings, persisted) = settings::Store::open(backend.as_ref().map(Backend::dirs), &startup);
        let mut controller = Controller::new(view, size);
        // `createDeck` starts free (`controllerFor(false)`) with the basemap's limits
        controller.set_limits(cam::limits(persisted.settings.basemap, None), cam::max_zoom_now(None));
        AppState {
            startup,
            backend,
            requests,
            events,
            persisted,
            controller,
            motion: Motion::new(),
            update_ready: None,
            settings,
            shortcuts: Default::default(),
            camera: Default::default(),
            pick: Default::default(),
            aircraft: Default::default(),
            flight: Default::default(),
            liveries: Default::default(),
            taxiways: Default::default(),
            airport: Default::default(),
            trains: Default::default(),
            blocks: Default::default(),
            ships: Default::default(),
            weather: Default::default(),
            radar: Default::default(),
            search: Default::default(),
            filters_handed: Cell::new(None),
        }
    }

    /// The basemap drawn now.
    pub fn basemap(&self) -> Basemap {
        self.persisted.settings.basemap
    }

    /// The frame's clocks for the local time `local_ms`.
    pub fn clocks(&self, local_ms: f64) -> Clocks {
        Clocks { local_ms, feed_ms: self.motion.now(local_ms) }
    }

    /// Whether the camera follows a vehicle with no transition of the app's running: then the scene
    /// is rebuilt every frame close in (doc 01 §2.6, `locked && Date.now() > transitionUntil`).
    pub fn locked_steady(&self, local_ms: f64) -> bool {
        self.controller.locked() && local_ms > self.controller.transition_until()
    }

    /// Step 2 of the frame loop: the backend's events, then every part's answers and requests.
    pub fn poll(&mut self, local_ms: f64) -> Wants {
        let events: Vec<BackendEvent> = self.events.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        let mut wants = Wants::default();
        for event in &events {
            if let BackendEvent::UpdateReady { version } = event {
                self.update_ready = Some(version.clone());
                wants.scene = true;
            }
        }
        let clocks = self.clocks(local_ms);
        let mut cx = Cx {
            clocks,
            controller: &mut self.controller,
            motion: &mut self.motion,
            persisted: &mut self.persisted,
            backend: self.backend.as_ref(),
            requests: self.requests.as_ref(),
            startup: &self.startup,
            events: &events,
            wants: &mut wants,
        };
        self.settings.poll(&mut cx);
        self.aircraft.poll(&mut cx);
        self.flight.poll(&mut cx);
        self.liveries.poll(&mut cx);
        self.taxiways.poll(&mut cx);
        self.airport.poll(&mut cx);
        self.trains.set_flight_open(self.flight.selected_id().is_some());
        self.trains.poll(&mut cx);
        self.blocks.poll(&mut cx);
        let other_panel = self.flight.selected_id().is_some() || self.trains.selected_id().is_some() || self.airport.code().is_some();
        self.ships.set_other_panel_open(other_panel);
        self.ships.poll(&mut cx);
        self.weather.poll(&mut cx);
        self.radar.poll(&mut cx);
        self.search.poll(&mut cx);
        self.pick.poll(&mut cx);
        self.shortcuts.poll(&mut cx);
        wants
    }

    /// Step 3: advances the running transition, then the camera's own work. Returns whether the
    /// camera moved, and what [`camera`] wants.
    pub fn advance_camera(&mut self, local_ms: f64) -> (bool, Wants) {
        let before = self.controller.view();
        self.controller.update(local_ms);
        let mut wants = Wants::default();
        let clocks = self.clocks(local_ms);
        let mut cx = Cx {
            clocks,
            controller: &mut self.controller,
            motion: &mut self.motion,
            persisted: &mut self.persisted,
            backend: self.backend.as_ref(),
            requests: self.requests.as_ref(),
            startup: &self.startup,
            events: &[],
            wants: &mut wants,
        };
        self.camera.poll(&mut cx);
        (self.controller.view() != before, wants)
    }

    /// Carries out an action of the UI or the keys at `local_ms`. Returns whether it did
    /// something (for [`Action::CloseTop`]: whether something was closed; on Android the app
    /// leaves only when nothing was).
    pub fn apply(&mut self, action: Action, local_ms: f64) -> bool {
        match action {
            Action::CloseTop => self.close_top(local_ms),
            Action::ZoomBy(delta) => {
                self.controller.zoom_by(delta, local_ms);
                true
            }
            Action::ResetOrientation => {
                self.controller.reset_orientation(local_ms);
                true
            }
            Action::SetPitch(pitch) => {
                self.controller.set_pitch(pitch, local_ms);
                true
            }
            Action::FlyTo { longitude, latitude, zoom } => {
                self.controller.fly_to(longitude, latitude, zoom, local_ms);
                true
            }
            Action::SetMode(mode) => self.set_mode(mode),
            Action::ToggleShown(kind) => self.toggle_shown(kind),
            Action::ToggleFollow => self.toggle_following(local_ms),
            Action::SetBasemap(basemap) => self.set_basemap(basemap, local_ms),
            Action::Toggle(what) => self.toggle_setting(what),
            Action::Open(what, open) => self.set_open(what, open),
            Action::ToggleOpen(what) => self.set_open(what, !self.shortcuts.is_open(what)),
            Action::SetMenu(menu) => self.set_menu(menu),
            Action::RetryFeed => {
                self.aircraft.retry();
                true
            }
            Action::Search(ask) => self.search_ask(ask, local_ms),
            Action::PickFlight(id) => {
                self.select_flight(Some(id), flight::SelectOptions { fly: true, ..Default::default() }, local_ms);
                true
            }
            Action::RetryTop => {
                self.aircraft.retry_top();
                true
            }
            Action::SetFilters => match self.filters_handed.take() {
                Some(filters) if filters != self.persisted.filters => {
                    self.persisted.filters = filters;
                    true
                }
                _ => false,
            },
            Action::Board(ask) => self.board_ask(ask, local_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use rustradar_scene::camera::{LIMITS, REALISTIC_LIMITS};
    use rustradar_scene::controller::ZOOM_BY_MS;

    use super::*;

    fn state(startup: Startup) -> AppState {
        AppState::new(startup, None, None, None, ViewState::INITIAL, [1440.0, 900.0])
    }

    #[test]
    fn the_command_line_basemap_wins_and_sets_its_limits() {
        let dark = state(Startup::default());
        assert_eq!(dark.basemap(), Basemap::Dark);
        assert_eq!(dark.controller.limits(), LIMITS);
        assert_eq!(dark.controller.view(), ViewState::INITIAL);
        let realistic = state(Startup { basemap: Some(Basemap::Realistic), ..Default::default() });
        assert_eq!(realistic.basemap(), Basemap::Realistic);
        assert_eq!(realistic.controller.limits(), REALISTIC_LIMITS);
        assert_eq!(realistic.controller.max_zoom_now(), 20.0);
    }

    #[test]
    fn clocks_fixed_running_and_synced() {
        assert_eq!(Clock::fixed(1.7e12).now_ms(), 1.7e12);
        let running = Clock::starting_at(1.0e12);
        let a = running.now_ms();
        assert!((a - 1.0e12).abs() < 1000.0, "{a}");
        assert!(running.now_ms() >= a);
        let system = Clock::system().now_ms();
        assert!((system - system_now_ms()).abs() < 1000.0);
        // before the first snapshot the feed clock is the local one
        let mut s = state(Startup::default());
        assert_eq!(s.clocks(1.7e12), Clocks { local_ms: 1.7e12, feed_ms: 1.7e12 });
        s.motion.sync_clock(1.7e12 + 2500.0, 1.7e12);
        assert_eq!(s.clocks(1.7e12).feed_ms, 1.7e12 + 2500.0);
    }

    #[test]
    fn poll_drains_the_backend_events_and_keeps_the_update() {
        let (tx, rx) = mpsc::channel();
        let mut s = AppState::new(Startup::default(), None, None, Some(rx), ViewState::INITIAL, [800.0, 600.0]);
        assert_eq!(s.poll(0.0), Wants::default());
        tx.send(BackendEvent::TrainsChanged).unwrap();
        tx.send(BackendEvent::UpdateReady { version: "2.0.0".into() }).unwrap();
        let wants = s.poll(1.0);
        assert!(wants.scene);
        assert_eq!(s.update_ready.as_deref(), Some("2.0.0"));
        // drained: nothing the second time
        assert_eq!(s.poll(2.0), Wants::default());
    }

    #[test]
    fn camera_actions_run_the_controllers_moves() {
        let mut s = state(Startup::default());
        let t0 = 1.0e12;
        assert!(s.apply(Action::ZoomBy(1.0), t0));
        let (moved, _) = s.advance_camera(t0 + ZOOM_BY_MS / 2.0);
        assert!(moved);
        assert!(s.controller.in_transition());
        let (moved, _) = s.advance_camera(t0 + ZOOM_BY_MS);
        assert!(moved);
        assert!(!s.controller.in_transition());
        assert!((s.controller.view().zoom - 2.9).abs() < 1e-9, "{:?}", s.controller.view());
        let (moved, _) = s.advance_camera(t0 + ZOOM_BY_MS + 16.0);
        assert!(!moved, "nothing runs once the transition has ended");
        assert!(!s.apply(Action::CloseTop, t0), "nothing is open to close");
        assert!(s.apply(Action::SetPitch(55.0), t0 + 1000.0));
        s.advance_camera(t0 + 2000.0);
        assert_eq!(s.controller.view().pitch, 55.0);
        assert!(s.apply(Action::ResetOrientation, t0 + 3000.0));
        s.advance_camera(t0 + 4000.0);
        assert_eq!(s.controller.view().pitch, 0.0);
        assert!(s.apply(Action::FlyTo { longitude: 18.07, latitude: 59.33, zoom: Some(9.0) }, t0 + 5000.0));
        s.advance_camera(t0 + 7000.0);
        let v = s.controller.view();
        assert!((v.longitude - 18.07).abs() < 1e-9 && (v.latitude - 59.33).abs() < 1e-9, "{v:?}");
    }

    #[test]
    fn wants_keep_the_earliest_wake() {
        let mut w = Wants::default();
        w.wake_at(500.0);
        w.wake_at(200.0);
        w.wake_at(900.0);
        assert_eq!(w.wake_at, Some(200.0));
        let mut other = Wants { scene: true, wake_at: Some(100.0) };
        other.merge(w);
        assert_eq!(other, Wants { scene: true, wake_at: Some(100.0) });
    }

    #[test]
    fn locked_steady_waits_for_the_apps_own_moves() {
        let mut s = state(Startup::default());
        assert!(!s.locked_steady(0.0));
        s.controller.set_locked(true);
        assert!(s.locked_steady(1.0));
        s.controller.fly_to(18.0, 59.0, None, 10.0);
        assert!(!s.locked_steady(500.0), "a fly-to is running");
        assert!(s.locked_steady(10.0 + 1600.0));
    }
}
