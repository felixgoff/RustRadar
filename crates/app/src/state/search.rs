//! The search box's state (item U8; doc 06 §3.7, §5.2; `SearchBox.svelte`, `+page.svelte:750-855`,
//! `:1077-1101`): the typed query of each focus, the two searches that run as it is typed, and
//! what a picked row does. The field, the dropdown, the highlighted row and the keys are the UI's
//! (`ui::search`).
//!
//! **Two searches** run independently as the query is typed ([`Ask::Typed`], the field's
//! `oninput`), each with a phase ([`Phase`]: idle, searching, done with its results, failed), a
//! debounce and a request counter:
//! - **flights and airports** (`find(query)`, Flightradar24's search; only the `live` and
//!   `airport` entries are kept): from [`FLIGHT_MIN_CHARS`] characters, [`FLIGHT_DEBOUNCE_MS`]
//!   after the last keystroke. Not run while the focus is trains or ships (the local search);
//! - **places** (`geocode(query, near, placeLanguage())`, Photon): from [`PLACE_MIN_CHARS`]
//!   characters, [`PLACE_DEBOUNCE_MS`] after the last keystroke, ranked near the view's centre
//!   once the view is zoomed in to [`NEAR_ZOOM`] (`near`, read when the search runs).
//!
//! Each keystroke clears both timers, marks a search `searching` (or `idle` below its length,
//! which also drops the answer on its way) and lets go of the searched place's marker
//! (`onquery`). When a timer comes due ([`Search::poll`]) the search takes the query as it is
//! then, takes a new number from its counter (`++flightLatest`) and asks; an answer is kept only
//! while it carries the latest number. As today, an answer for an earlier query that lands while
//! a newer keystroke's timer runs is shown until the newer one is asked.
//!
//! **The query** ([`Search::query`]) is the aircraft search box's own (it starts empty whenever
//! the aircraft become the focus again: today's page makes a new box per focus), and the trains'
//! and ships' (`trainQuery`, `shipQuery`) are the page's: they outlive a change of focus and
//! narrow the sidebar's list too ([`Search::list_query`], read through `shell::Pass::query`).
//! A change of focus leaves both searches idle and drops what they had on the way.
//!
//! **The local search** (trains and ships): the first group of the dropdown is [`local_rows`],
//! at most [`LOCAL_MAX`] of the sidebar's list (`trainsListed`, `shipsListed`: filtered, matched
//! against the query, sorted), worked out from the loaded list without asking anyone.
//!
//! **Picking** ([`Ask::Pick`]): the box is reset (both timers cleared, both counters bumped, the
//! query emptied, both searches idle), then
//! - a live flight: `select(parseInt(id, 16), {fly: true, hint})` with the entry's position and
//!   callsign as the hint;
//! - an airport: the camera flies to it at zoom [`AIRPORT_ZOOM`], and its board opens when the
//!   entry's id is an IATA code (`/^[A-Z0-9]{3}$/`): the flight, the ship and the train let go;
//! - a train or a ship of the local search: `pickTrain`, `pickShip` (the place marker goes, the
//!   lock lets go, the camera flies there);
//! - a place: `pickPlace`: the aircraft lock lets go, the place is marked on the map
//!   (`state::flight`'s [`MarkedPlace`]) and the camera flies to [`place_view`] for the window.
//!
//! [`Ask::Clear`] is the clear button (reset, then the place marker goes); [`Ask::EmptyQuery`] the
//! lists' "Show all" (`trainQuery = ""`, nothing else).
//!
//! **The typed text** reaches the state beside its action: [`Action`](super::Action) is `Copy`
//! (the keys' `Shortcut` holds one), so the UI hands the field's new text over with
//! [`Search::hand_over`] and pushes [`Ask::Typed`]; the state takes it when the action is applied.
//!
//! **Sources**: the `--fixture` folder ([`FIXTURE_FIND`]: the `FindEntry` list `search` answers,
//! [`FIXTURE_PLACES`]: the `Place` list `geocode` answers, each for any query; a missing file is a
//! failed request), else the backend (`Backend::search`, `geocode::geocode`), else none (the page
//! without Tauri: every search fails). [`Search::with_feeds`] sets them instead (tests).
//!
//! Deviations from today's app:
//! - The interface language Photon is asked in comes from the environment (`LC_ALL`,
//!   `LC_MESSAGES`, `LANG`), not the webview's `navigator.language`; without one (Windows and
//!   macOS apps started from the desktop) places keep their local names.
//! - The typed text and its searches take effect when the frame's actions are applied, after the
//!   UI pass that read the keystroke; the field shows the text at once.
//! - An entry whose id has no hexadecimal digit selects nothing (today `parseInt` gives `NaN` and
//!   a selection nothing can match).
//! - A row picked in the frame in which an earlier action changed the lists (a filter, a new
//!   focus) is looked up in the lists as they are then.

use std::cell::Cell;
use std::path::PathBuf;

use rustradar_backend::{Backend, convert, geocode};
use rustradar_core::model::{FindDetail, FindEntry, LiveFlight, Place};
use rustradar_core::num::js_round;
use rustradar_core::places::{place_language, place_view};
use rustradar_core::settings::Mode;
use rustradar_core::ship_list::ship_name;
use rustradar_core::ships::{ship_color, ship_kind_label};
use rustradar_core::train_list::{TrainSort, sort_trains, train_filter, train_matches};
use rustradar_core::trains::{LatLon, delay_text, train_color_of, train_label, train_running};
use rustradar_core::geo::Rgb;

use super::aircraft::{Answer, NO_BACKEND};
use super::flight::{MarkedPlace, SelectOptions};
use super::{AppState, Cx, Startup};
use crate::requests::Requests;

/// The flight search waits this long after the last keystroke, ms (`FLIGHT_DEBOUNCE_MS`:
/// Flightradar24 answers fast).
pub const FLIGHT_DEBOUNCE_MS: f64 = 250.0;
/// The place search waits this long, ms (`PLACE_DEBOUNCE_MS`: Photon's fair use asks ≥ 300 ms).
pub const PLACE_DEBOUNCE_MS: f64 = 350.0;
/// The flight search needs this many characters (UTF-16 units, as `q.length` counts).
pub const FLIGHT_MIN_CHARS: usize = 2;
/// The place search needs this many (`PLACE_MIN_CHARS`).
pub const PLACE_MIN_CHARS: usize = 3;
/// The most rows a local search lists (`LOCAL_MAX`): the sidebar's list has the rest.
pub const LOCAL_MAX: usize = 6;
/// Places rank toward the view's centre from this zoom (`near`).
pub const NEAR_ZOOM: f64 = 5.0;
/// An airport picked in the search is flown to at this zoom.
pub const AIRPORT_ZOOM: f64 = 11.0;

/// The `--fixture` folder's flight search: a `FindEntry` list as `search` sends it, for any query.
pub const FIXTURE_FIND: &str = "search_find.json";
/// The `--fixture` folder's place search: a `Place` list as `geocode` sends it, for any query.
pub const FIXTURE_PLACES: &str = "search_places.json";

/// A search's phase (`Phase<T>`).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Phase<T> {
    #[default]
    Idle,
    Searching,
    Done(Vec<T>),
    Error,
}

impl<T> Phase<T> {
    /// Done or failed (`settled`).
    pub fn settled(&self) -> bool {
        matches!(self, Phase::Done(_) | Phase::Error)
    }

    /// The results once done; none otherwise.
    pub fn results(&self) -> &[T] {
        match self {
            Phase::Done(results) => results,
            _ => &[],
        }
    }
}

/// What the search box asks of the state (through [`Action::Search`](super::Action::Search)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// The field's text changed to what [`Search::hand_over`] holds (`oninput`).
    Typed,
    /// A row of the dropdown was picked (a click, Enter).
    Pick(Row),
    /// The clear button (`clear`: reset, then `onquery`).
    Clear,
    /// The focus's query emptied and nothing else (the lists' "Show all": `trainQuery = ""`).
    EmptyQuery,
}

/// A row of the dropdown, by its group and its place in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// The flight search's results ([`Search::flights`]).
    Flight(usize),
    /// The local search's rows ([`local_rows`] for the query as it is).
    Local(usize),
    /// The place search's results ([`Search::places`]).
    Place(usize),
}

/// A row of the local search: a train or a ship loaded on the map (`LocalItem`).
#[derive(Debug, Clone, PartialEq)]
pub struct LocalRow {
    /// The train's id, or the ship's MMSI as text.
    pub key: String,
    pub label: String,
    pub detail: String,
    /// Its colour on the map.
    pub color: Rgb,
}

/// Where the searches' answers come from: the backend, the `--fixture` folder, or a test's.
pub trait SearchFeeds: Send {
    /// `search(query)`: Flightradar24's entries of every type.
    fn find(&mut self, query: &str) -> Answer<Result<Vec<FindEntry>, String>>;
    /// `geocode(query, near, lang)`.
    fn geocode(&mut self, query: &str, near: Option<LatLon>, lang: Option<&'static str>) -> Answer<Result<Vec<Place>, String>>;
}

/// The backend's requests, on its runtime.
struct BackendFeeds {
    backend: Backend,
    requests: Requests,
}

impl SearchFeeds for BackendFeeds {
    fn find(&mut self, query: &str) -> Answer<Result<Vec<FindEntry>, String>> {
        let (backend, query) = (self.backend.clone(), query.to_owned());
        self.requests.request(async move { backend.search(&query).await.map(|found| found.into_iter().map(convert::find_entry).collect()) }).into()
    }

    fn geocode(&mut self, query: &str, near: Option<LatLon>, lang: Option<&'static str>) -> Answer<Result<Vec<Place>, String>> {
        let query = query.to_owned();
        self.requests.request(async move { geocode::geocode(&query, near.map(|n| n.lat), near.map(|n| n.lon), lang).await }).into()
    }
}

/// The `--fixture` folder's files (see the module documentation), read when asked for.
struct FixtureFeeds {
    dir: PathBuf,
}

/// `dir/name` read as JSON of type `$t`, or why not.
macro_rules! read_fixture {
    ($t:ty, $dir:expr, $name:expr) => {{
        let path = $dir.join($name);
        std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice::<$t>(&bytes).map_err(|e| e.to_string()))
            .map_err(|e| format!("fixture {}: {e}", path.display()))
    }};
}

impl SearchFeeds for FixtureFeeds {
    fn find(&mut self, _query: &str) -> Answer<Result<Vec<FindEntry>, String>> {
        Answer::ready(read_fixture!(Vec<FindEntry>, self.dir, FIXTURE_FIND))
    }

    fn geocode(&mut self, _query: &str, _near: Option<LatLon>, _lang: Option<&'static str>) -> Answer<Result<Vec<Place>, String>> {
        Answer::ready(read_fixture!(Vec<Place>, self.dir, FIXTURE_PLACES))
    }
}

/// No backend (the page without Tauri): every search fails at once.
struct NoFeeds;

impl SearchFeeds for NoFeeds {
    fn find(&mut self, _query: &str) -> Answer<Result<Vec<FindEntry>, String>> {
        Answer::ready(Err(NO_BACKEND.to_owned()))
    }

    fn geocode(&mut self, _query: &str, _near: Option<LatLon>, _lang: Option<&'static str>) -> Answer<Result<Vec<Place>, String>> {
        Answer::ready(Err(NO_BACKEND.to_owned()))
    }
}

/// A request out: the number it took from its counter, and its answer.
struct InFlight<T> {
    id: u64,
    answer: Answer<Result<Vec<T>, String>>,
}

/// One of the two searches: its phase, its timer (`flightTimer`), its counter (`flightLatest`)
/// and the request out.
struct Lookup<T> {
    phase: Phase<T>,
    due: Option<f64>,
    latest: u64,
    request: Option<InFlight<T>>,
}

impl<T> Default for Lookup<T> {
    fn default() -> Self {
        Lookup { phase: Phase::Idle, due: None, latest: 0, request: None }
    }
}

impl<T> Lookup<T> {
    /// `latest++; phase = idle`: idle, and an answer on its way is dropped.
    fn idle(&mut self) {
        self.latest += 1;
        self.request = None;
        self.phase = Phase::Idle;
    }

    /// `phase = searching; timer = setTimeout(run, ms)` (the timer was cleared before).
    fn arm(&mut self, at: f64) {
        self.phase = Phase::Searching;
        self.due = Some(at);
    }

    /// The timer, when it has come due at `now`.
    fn take_due(&mut self, now: f64) -> bool {
        if self.due.is_some_and(|at| now >= at) {
            self.due = None;
            true
        } else {
            false
        }
    }

    /// `const id = ++latest`, and the request.
    fn start(&mut self, answer: Answer<Result<Vec<T>, String>>) {
        self.latest += 1;
        self.request = Some(InFlight { id: self.latest, answer });
    }

    /// The answer, kept when it carries the latest number (`if (id === latest)`). Returns whether
    /// the phase changed.
    fn take_answer(&mut self, keep: impl Fn(&T) -> bool) -> bool {
        let Some(request) = &self.request else { return false };
        let Some(result) = request.answer.poll() else { return false };
        let id = request.id;
        self.request = None;
        if id != self.latest {
            return false;
        }
        self.phase = match result {
            Ok(found) => Phase::Done(found.into_iter().filter(|e| keep(e)).collect()),
            Err(_) => Phase::Error,
        };
        true
    }
}

/// Where the searches go: chosen at the first poll (or ask).
enum Source {
    Auto,
    On(Box<dyn SearchFeeds>),
}

/// The search box's state (see the module documentation).
pub struct Search {
    source: Source,
    /// The focus the box is for (`None` before the first poll).
    mode: Option<Mode>,
    /// The aircraft box's own query, and the page's `trainQuery` and `shipQuery`.
    aircraft_query: String,
    train_query: String,
    ship_query: String,
    flights: Lookup<FindEntry>,
    places: Lookup<Place>,
    /// Photon's language (`placeLanguage()`), from the environment at the first poll.
    language: Option<Option<&'static str>>,
    /// The field's text handed over by the UI for the next [`Ask::Typed`].
    typed: Cell<Option<String>>,
}

impl Default for Search {
    /// The `--fixture` folder's answers, else the backend's, chosen at the first poll.
    fn default() -> Self {
        Search::with_source(Source::Auto)
    }
}

impl std::fmt::Debug for Search {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn phase<T>(p: &Phase<T>) -> String {
            match p {
                Phase::Idle => "idle".to_string(),
                Phase::Searching => "searching".to_string(),
                Phase::Done(r) => format!("done({})", r.len()),
                Phase::Error => "error".to_string(),
            }
        }
        f.debug_struct("Search")
            .field("mode", &self.mode)
            .field("query", &self.query(self.mode.unwrap_or_default()))
            .field("flights", &phase(&self.flights.phase))
            .field("places", &phase(&self.places.phase))
            .finish_non_exhaustive()
    }
}

/// `q.length`: UTF-16 code units, as JavaScript counts a string.
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// JavaScript's `String.prototype.trim`.
fn js_trim(s: &str) -> &str {
    crate::ui::widgets::token_input::js_trim(s)
}

/// The interface language as the environment names it (`LC_ALL`, then `LC_MESSAGES`, then
/// `LANG`; POSIX's order), or empty.
pub fn system_language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"].iter().filter_map(|name| std::env::var(name).ok()).find(|v| !v.is_empty()).unwrap_or_default()
}

/// Only these can be shown on the map (`selectable`): live flights and airports.
pub fn selectable(entry: &FindEntry) -> bool {
    matches!(entry.detail, FindDetail::Live(_) | FindDetail::Airport(_))
}

/// A flight row's second line (`detailOf`): a live flight's route, else its aircraft type; an
/// airport's `Airport`.
pub fn flight_detail(entry: &FindEntry) -> String {
    match &entry.detail {
        FindDetail::Live(live) => live.route.clone().or_else(|| live.ac_type.clone()).unwrap_or_default(),
        FindDetail::Airport(_) => "Airport".to_owned(),
        _ => String::new(),
    }
}

/// `parseInt(s, 16)` for a flight id: leading white space, an optional `+`, an optional `0x`, then
/// the hexadecimal digits up to the first that is not one. `None` when there is none (`NaN`), the
/// number is negative or does not fit an id.
pub fn parse_hex_id(s: &str) -> Option<u32> {
    let s = js_trim(s);
    let (negative, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    let digits: &str = &s[..s.bytes().position(|b| !b.is_ascii_hexdigit()).unwrap_or(s.len())];
    if digits.is_empty() {
        return None;
    }
    let value = u64::from_str_radix(digits.get(..digits.len().min(16))?, 16).ok()?;
    if negative && value != 0 {
        return None;
    }
    u32::try_from(value).ok()
}

/// Whether an airport entry's id opens its board (`/^[A-Z0-9]{3}$/`).
pub fn is_board_code(id: &str) -> bool {
    id.len() == 3 && id.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// The field's placeholder for `mode`, on a phone or not.
pub fn placeholder(mode: Mode, phone: bool) -> &'static str {
    match (mode, phone) {
        (Mode::Aircraft, _) => "Flight, airport or address",
        (Mode::Trains, false) => "Train, destination or address",
        (Mode::Trains, true) => "Train or address",
        (Mode::Ships, false) => "Ship name, MMSI or address",
        (Mode::Ships, true) => "Ship or address",
    }
}

/// The local search's group heading and its noun (`heading`, `noun`); none for the aircraft.
pub fn local_words(mode: Mode) -> Option<(&'static str, &'static str)> {
    match mode {
        Mode::Aircraft => None,
        Mode::Trains => Some(("Trains in view", "trains")),
        Mode::Ships => Some(("Ships in view", "ships")),
    }
}

/// The view's centre in 1/20° steps (`listCentre`): the lists' distance sort.
pub fn list_centre(latitude: f64, longitude: f64) -> LatLon {
    LatLon { lat: js_round(latitude * 20.0) / 20.0, lon: js_round(longitude * 20.0) / 20.0 }
}

/// `local.search(query)`: the first [`LOCAL_MAX`] rows of the focus's sidebar list for `query`
/// (`trainsListed`, `shipsListed`: filtered, matched, sorted) at local time `now_ms`; none for the
/// aircraft or a blank query (`local && query.trim()`).
pub fn local_rows(state: &AppState, query: &str, now_ms: f64) -> Vec<LocalRow> {
    let q = js_trim(query);
    if q.is_empty() {
        return Vec::new();
    }
    let view = state.controller.view();
    let centre = list_centre(view.latitude, view.longitude);
    let origin = LatLon { lat: 0.0, lon: 0.0 };
    match state.persisted.settings.mode {
        Mode::Aircraft => Vec::new(),
        Mode::Trains => {
            let filters = &state.persisted.train_filters;
            let keep = train_filter(filters);
            let now = now_ms as i64;
            let filtered = state.trains.trains().iter().filter(|t| keep(t) && train_running(t, now) && train_matches(t, q));
            let sort = filters.sort;
            sort_trains(filtered, sort, if sort == TrainSort::Distance { centre } else { origin })
                .into_iter()
                .take(LOCAL_MAX)
                .map(|t| {
                    let to = t.destination.as_deref().filter(|d| !d.is_empty()).map(|d| format!("To {d}"));
                    let delay = delay_text(t.delay_minutes);
                    let state = if delay.is_empty() { train_label(t.kind).to_owned() } else { delay };
                    let detail = to.into_iter().chain([state].into_iter().filter(|s| !s.is_empty())).collect::<Vec<_>>().join(" · ");
                    LocalRow { key: t.id.clone(), label: t.name.clone(), detail, color: train_color_of(t) }
                })
                .collect()
        }
        Mode::Ships => state
            .ships
            .listed(&state.persisted.ship_filters, q, centre)
            .into_iter()
            .take(LOCAL_MAX)
            .map(|s| {
                let name = ship_name(s);
                let label = if name.is_empty() { format!("MMSI {}", s.mmsi) } else { name.to_owned() };
                LocalRow { key: s.mmsi.to_string(), label, detail: format!("{} · MMSI {}", ship_kind_label(s.kind), s.mmsi), color: ship_color(s.kind) }
            })
            .collect(),
    }
}

impl Search {
    fn with_source(source: Source) -> Self {
        Search {
            source,
            mode: None,
            aircraft_query: String::new(),
            train_query: String::new(),
            ship_query: String::new(),
            flights: Lookup::default(),
            places: Lookup::default(),
            language: None,
            typed: Cell::new(None),
        }
    }

    /// The searches go to `feeds` (tests).
    pub fn with_feeds(feeds: Box<dyn SearchFeeds>) -> Self {
        Search::with_source(Source::On(feeds))
    }

    /// Photon is asked in `language` (`placeLanguage(language)`), not the environment's.
    pub fn set_language(&mut self, language: &str) {
        self.language = Some(place_language(language));
    }

    /// The search box's text while `mode` is the focus.
    pub fn query(&self, mode: Mode) -> &str {
        match mode {
            Mode::Aircraft => &self.aircraft_query,
            Mode::Trains => &self.train_query,
            Mode::Ships => &self.ship_query,
        }
    }

    /// What narrows the sidebar's list while `mode` is the focus (`trainQuery`, `shipQuery`):
    /// the aircraft's query narrows nothing.
    pub fn list_query(&self, mode: Mode) -> &str {
        match mode {
            Mode::Aircraft => "",
            mode => self.query(mode),
        }
    }

    fn query_mut(&mut self, mode: Mode) -> &mut String {
        match mode {
            Mode::Aircraft => &mut self.aircraft_query,
            Mode::Trains => &mut self.train_query,
            Mode::Ships => &mut self.ship_query,
        }
    }

    /// The flight search (`flights`), with only the live flights and airports.
    pub fn flights(&self) -> &Phase<FindEntry> {
        &self.flights.phase
    }

    /// The place search (`places`).
    pub fn places(&self) -> &Phase<Place> {
        &self.places.phase
    }

    /// Either search is under way (`searching`: the field's spinner).
    pub fn searching(&self) -> bool {
        matches!(self.flights.phase, Phase::Searching) || matches!(self.places.phase, Phase::Searching)
    }

    /// When the next search is due, local Unix ms.
    pub fn next_due(&self) -> Option<f64> {
        match (self.flights.due, self.places.due) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The field's new text, for the [`Ask::Typed`] the UI pushes with it (see the module
    /// documentation).
    pub fn hand_over(&self, text: String) {
        self.typed.set(Some(text));
    }

    fn resolve(&mut self, startup: &Startup, backend: Option<&Backend>, requests: Option<&Requests>) {
        if self.language.is_none() {
            self.language = Some(place_language(&system_language()));
        }
        if !matches!(self.source, Source::Auto) {
            return;
        }
        self.source = Source::On(if let Some(dir) = &startup.fixture {
            Box::new(FixtureFeeds { dir: dir.clone() })
        } else if let (Some(backend), Some(requests)) = (backend, requests) {
            Box::new(BackendFeeds { backend: backend.clone(), requests: requests.clone() })
        } else {
            Box::new(NoFeeds)
        });
    }

    /// The focus is `mode`: a new focus has a new box (both searches idle, what was on its way
    /// dropped, the aircraft's own query empty). Returns whether it changed.
    fn follow_mode(&mut self, mode: Mode) -> bool {
        if self.mode == Some(mode) {
            return false;
        }
        let first = self.mode.is_none();
        self.mode = Some(mode);
        if !first {
            self.flights.due = None;
            self.places.due = None;
            self.flights.idle();
            self.places.idle();
            self.aircraft_query.clear();
        }
        true
    }

    /// `oninput` for the query as it now is, at local time `now`: both timers cleared, each
    /// search idle below its length (local: the flights always) or searching with its timer.
    fn input(&mut self, mode: Mode, now: f64) {
        self.flights.due = None;
        self.places.due = None;
        let q = js_len(js_trim(self.query(mode)));
        if mode != Mode::Aircraft || q < FLIGHT_MIN_CHARS {
            self.flights.idle();
        } else {
            self.flights.arm(now + FLIGHT_DEBOUNCE_MS);
        }
        if q < PLACE_MIN_CHARS {
            self.places.idle();
        } else {
            self.places.arm(now + PLACE_DEBOUNCE_MS);
        }
    }

    /// `reset()`'s part in the state: both timers cleared, both counters bumped, the query empty,
    /// both searches idle.
    fn reset(&mut self, mode: Mode) {
        self.flights.due = None;
        self.places.due = None;
        self.flights.idle();
        self.places.idle();
        self.query_mut(mode).clear();
    }

    /// This frame's step (see [`super::AppState::poll`] for the order): the focus, the timers
    /// that are due (`runFlights`, `runPlaces`), the answers.
    pub fn poll(&mut self, cx: &mut Cx<'_>) {
        self.resolve(cx.startup, cx.backend, cx.requests);
        let mode = cx.persisted.settings.mode;
        self.follow_mode(mode);
        let now = cx.clocks.local_ms;
        let Source::On(feeds) = &mut self.source else { return };
        let q = js_trim(match mode {
            Mode::Aircraft => &self.aircraft_query,
            Mode::Trains => &self.train_query,
            Mode::Ships => &self.ship_query,
        })
        .to_owned();
        if self.flights.take_due(now) {
            self.flights.start(feeds.find(&q));
        }
        if self.places.take_due(now) {
            // `near` as the view is when the search runs
            let view = cx.controller.view();
            let near = (view.zoom >= NEAR_ZOOM).then_some(LatLon { lat: view.latitude, lon: view.longitude });
            let lang = self.language.flatten();
            self.places.start(feeds.geocode(&q, near, lang));
        }
        self.flights.take_answer(selectable);
        self.places.take_answer(|_| true);
        if let Some(at) = self.next_due() {
            cx.wants.wake_at(at);
        }
    }
}

/// `openAirport(code)`: the board of `code`, the flight, the ship and the train let go.
fn open_board(state: &mut AppState, code: &str, now_ms: f64) {
    state.flight.set_focus_airport(Some(code.to_uppercase()));
    if state.flight.selected_id().is_some() {
        state.select_flight(None, SelectOptions::default(), now_ms);
    }
    state.select_ship(None, now_ms);
    state.select_train(None, now_ms);
}

/// What a picked row stands for, taken before the box is reset.
enum Picked {
    Flight(FindEntry),
    Local(String),
    Place(Place),
}

impl AppState {
    /// Carries out the search box's `ask` at local time `now_ms` (see the module documentation).
    /// Returns whether anything changed.
    pub fn search_ask(&mut self, ask: Ask, now_ms: f64) -> bool {
        self.search.resolve(&self.startup, self.backend.as_ref(), self.requests.as_ref());
        let mode = self.persisted.settings.mode;
        self.search.follow_mode(mode);
        match ask {
            Ask::Typed => {
                let Some(text) = self.search.typed.take() else { return false };
                *self.search.query_mut(mode) = text;
                self.search.input(mode, now_ms);
                // `onquery`: a new search lets go of the place marked
                self.flight.set_place(None);
                true
            }
            Ask::Clear => {
                self.search.reset(mode);
                self.flight.set_place(None);
                true
            }
            Ask::EmptyQuery => {
                let query = self.search.query_mut(mode);
                let changed = !query.is_empty();
                query.clear();
                changed
            }
            Ask::Pick(row) => {
                let picked = match row {
                    Row::Flight(n) => self.search.flights().results().get(n).cloned().map(Picked::Flight),
                    Row::Place(n) => self.search.places().results().get(n).cloned().map(Picked::Place),
                    Row::Local(n) => local_rows(self, self.search.query(mode), now_ms).into_iter().nth(n).map(|r| Picked::Local(r.key)),
                };
                let Some(picked) = picked else { return false };
                self.search.reset(mode);
                match picked {
                    Picked::Flight(entry) => self.pick_search(&entry, now_ms),
                    Picked::Local(key) => self.pick_local(mode, &key, now_ms),
                    Picked::Place(place) => self.pick_place(&place, now_ms),
                }
                true
            }
        }
    }

    /// `pickSearch(entry)`: an airport flies there and opens its board, a live flight is selected
    /// and flown to.
    fn pick_search(&mut self, entry: &FindEntry, now_ms: f64) {
        match &entry.detail {
            FindDetail::Airport(airport) => {
                self.controller.fly_to(airport.lon, airport.lat, Some(AIRPORT_ZOOM), now_ms);
                // airport results are keyed by IATA code
                if is_board_code(&entry.id) {
                    open_board(self, &entry.id, now_ms);
                }
            }
            FindDetail::Live(live) => {
                let Some(id) = parse_hex_id(&entry.id) else { return };
                let hint = LiveFlight { id, lat: live.lat as f32, lon: live.lon as f32, callsign: live.callsign.clone().unwrap_or_default(), ..Default::default() };
                self.select_flight(Some(id), SelectOptions { hint: Some(hint), fly: true, ..Default::default() }, now_ms);
            }
            _ => {}
        }
    }

    /// `local.onpick(key)`: the train or ship of that key in the loaded list, picked.
    fn pick_local(&mut self, mode: Mode, key: &str, now_ms: f64) {
        match mode {
            Mode::Trains => {
                if let Some(train) = self.trains.trains().iter().find(|t| t.id == key).cloned() {
                    self.pick_train(train, now_ms);
                }
            }
            Mode::Ships => {
                if let Some(ship) = self.ships.ships().iter().find(|s| s.mmsi.to_string() == key).cloned() {
                    self.pick_ship(ship, now_ms);
                }
            }
            Mode::Aircraft => {}
        }
    }

    /// `pickPlace(p)`: the camera lets go of the aircraft, the place is marked, the camera flies
    /// to it at the zoom its kind or extent asks for this window.
    fn pick_place(&mut self, place: &Place, now_ms: f64) {
        // a camera locked on an aircraft can't fly anywhere else
        self.flight.set_following(false);
        self.sync_flight(now_ms);
        self.flight.set_place(Some(MarkedPlace { lon: place.lon, lat: place.lat, name: place.name.clone() }));
        let [width, height] = self.controller.size();
        let target = place_view(place, width, height);
        self.controller.fly_to(target.lon, target.lat, Some(target.zoom), now_ms);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};

    use rustradar_core::model::{FindAirportDetail, FindLiveDetail, PlaceKind, Ship, Train};
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::state::{Action, Startup};

    /// The local clock at the first frame.
    const T0: f64 = 1_782_036_000_000.0;

    type Found = Result<Vec<FindEntry>, String>;
    type Placed = Result<Vec<Place>, String>;
    /// A place search asked for: the query, `near`, the language, and its answer's sender.
    type Geocoded = (String, Option<LatLon>, Option<&'static str>, Sender<Placed>);

    /// The searches asked for, with the senders that answer them.
    #[derive(Default)]
    struct Log {
        finds: Vec<(String, Sender<Found>)>,
        geocodes: Vec<Geocoded>,
    }

    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<Log>>);

    impl Fake {
        fn finds(&self) -> Vec<String> {
            self.0.lock().unwrap().finds.iter().map(|(q, _)| q.clone()).collect()
        }

        fn geocodes(&self) -> Vec<(String, Option<LatLon>, Option<&'static str>)> {
            self.0.lock().unwrap().geocodes.iter().map(|(q, n, l, _)| (q.clone(), *n, *l)).collect()
        }

        fn answer_find(&self, n: usize, answer: Found) {
            self.0.lock().unwrap().finds[n].1.send(answer).ok();
        }

        fn answer_geocode(&self, n: usize, answer: Placed) {
            self.0.lock().unwrap().geocodes[n].3.send(answer).ok();
        }
    }

    impl SearchFeeds for Fake {
        fn find(&mut self, query: &str) -> Answer<Found> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().finds.push((query.to_owned(), tx));
            rx.into()
        }

        fn geocode(&mut self, query: &str, near: Option<LatLon>, lang: Option<&'static str>) -> Answer<Placed> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().geocodes.push((query.to_owned(), near, lang, tx));
            rx.into()
        }
    }

    fn state_with(fake: &Fake) -> AppState {
        let mut s = AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, [1440.0, 900.0]);
        s.search = Search::with_feeds(Box::new(fake.clone()));
        s.search.set_language("en-GB");
        s
    }

    /// The field's text becomes `text` at `now` (the UI's hand-over and its action).
    fn type_(s: &mut AppState, text: &str, now: f64) {
        s.search.hand_over(text.to_owned());
        assert!(s.apply(Action::Search(Ask::Typed), now));
    }

    fn live(id: &str, callsign: &str, route: Option<&str>) -> FindEntry {
        FindEntry {
            id: id.into(),
            label: format!("{callsign} label"),
            name: None,
            match_kind: None,
            detail: FindDetail::Live(FindLiveDetail {
                lat: 59.65,
                lon: 17.92,
                schd_from: None,
                schd_to: None,
                ac_type: Some("A20N".into()),
                route: route.map(Into::into),
                logo: None,
                reg: None,
                callsign: Some(callsign.into()),
                flight: None,
                operator: None,
            }),
        }
    }

    fn airport(id: &str) -> FindEntry {
        FindEntry { id: id.into(), label: "Stockholm Arlanda Airport (ARN / ESSA)".into(), name: None, match_kind: None, detail: FindDetail::Airport(FindAirportDetail { lat: 59.6519, lon: 17.9186, size: None }) }
    }

    fn operator() -> FindEntry {
        serde_json::from_value(serde_json::json!({"id": "SAS", "label": "SAS", "type": "operator", "detail": {"operator_id": 1, "iata": "SK", "logo": null}})).unwrap()
    }

    fn place(name: &str) -> Place {
        Place { name: name.into(), label: format!("{name}, Stockholm, Sweden"), kind: PlaceKind::Poi, category: Some("Museum".into()), lat: 59.328, lon: 18.091, bbox: None }
    }

    #[test]
    fn each_search_waits_for_its_debounce_and_its_length() {
        let fake = Fake::default();
        let mut s = state_with(&fake);
        s.poll(T0);
        // one character: neither search
        type_(&mut s, "s", T0);
        assert_eq!((s.search.flights(), s.search.places()), (&Phase::Idle, &Phase::Idle));
        assert_eq!(s.search.next_due(), None);
        // two: the flights, 250 ms after the keystroke
        type_(&mut s, "sa", T0 + 100.0);
        assert_eq!(s.search.flights(), &Phase::Searching);
        assert_eq!(s.search.places(), &Phase::Idle);
        assert!(s.search.searching());
        // three: the places too, 350 ms after it; a keystroke puts both back
        type_(&mut s, "sas", T0 + 200.0);
        assert_eq!(s.search.next_due(), Some(T0 + 200.0 + FLIGHT_DEBOUNCE_MS));
        let wants = s.poll(T0 + 449.0);
        assert!(fake.finds().is_empty(), "not before 250 ms");
        assert_eq!(wants.wake_at, Some(T0 + 450.0), "the frame loop comes back for it");
        s.poll(T0 + 450.0);
        assert_eq!(fake.finds(), ["sas"]);
        assert!(fake.geocodes().is_empty());
        s.poll(T0 + 549.0);
        assert!(fake.geocodes().is_empty(), "not before 350 ms");
        s.poll(T0 + 550.0);
        assert_eq!(fake.geocodes(), [("sas".to_string(), None, Some("en"))], "the whole world: no bias at zoom 2");
        // each asked once
        s.poll(T0 + 2000.0);
        assert_eq!((fake.finds().len(), fake.geocodes().len()), (1, 1));
    }

    #[test]
    fn typing_on_restarts_the_timers_and_the_query_is_trimmed() {
        let fake = Fake::default();
        let mut s = state_with(&fake);
        s.poll(T0);
        type_(&mut s, "  sas", T0);
        for t in [100.0, 200.0, 300.0] {
            type_(&mut s, &format!("  sas{}", "1".repeat(t as usize / 100)), T0 + t);
            s.poll(T0 + t + 10.0);
        }
        assert!(fake.finds().is_empty(), "each keystroke clears the timer");
        s.poll(T0 + 300.0 + FLIGHT_DEBOUNCE_MS);
        assert_eq!(fake.finds(), ["sas111"]);
        // white space does not count towards the length
        type_(&mut s, " a ", T0 + 1000.0);
        assert_eq!(s.search.flights(), &Phase::Idle);
        // the length counts UTF-16 units, as JavaScript's
        type_(&mut s, "✈", T0 + 1100.0);
        assert_eq!(s.search.flights(), &Phase::Idle);
        type_(&mut s, "😀", T0 + 1200.0);
        assert_eq!(s.search.flights(), &Phase::Searching, "two units");
        assert_eq!(s.search.places(), &Phase::Idle);
    }

    #[test]
    fn only_the_latest_request_s_answer_is_kept() {
        let fake = Fake::default();
        let mut s = state_with(&fake);
        s.poll(T0);
        type_(&mut s, "sa", T0);
        s.poll(T0 + 250.0);
        // a newer query asked before the first answers
        type_(&mut s, "sas", T0 + 300.0);
        s.poll(T0 + 550.0);
        assert_eq!(fake.finds(), ["sa", "sas"]);
        // the first answer lands late: dropped
        fake.answer_find(0, Ok(vec![live("1", "SA1", None)]));
        s.poll(T0 + 600.0);
        assert_eq!(s.search.flights(), &Phase::Searching);
        fake.answer_find(1, Ok(vec![live("2", "SAS2", Some("ARN-CPH")), operator(), airport("ARN")]));
        s.poll(T0 + 650.0);
        let found = s.search.flights().results();
        assert_eq!(found.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["2", "ARN"], "operators are not shown");
        assert_eq!(flight_detail(&found[0]), "ARN-CPH");
        assert_eq!(flight_detail(&found[1]), "Airport");
    }

    #[test]
    fn an_earlier_answer_shows_until_the_newer_search_is_asked() {
        // today's counter moves when a search runs, not when a key is typed: the answer for
        // "sa" that lands while the timer for "sas" runs is shown
        let fake = Fake::default();
        let mut s = state_with(&fake);
        s.poll(T0);
        type_(&mut s, "sa", T0);
        s.poll(T0 + 250.0);
        type_(&mut s, "sas", T0 + 300.0);
        fake.answer_find(0, Ok(vec![live("1", "SA1", None)]));
        s.poll(T0 + 320.0);
        assert_eq!(s.search.flights().results().len(), 1);
        // below the length again: idle, and the answer on its way is dropped
        s.poll(T0 + 550.0);
        type_(&mut s, "s", T0 + 560.0);
        fake.answer_find(1, Ok(vec![live("2", "SAS2", None)]));
        s.poll(T0 + 600.0);
        assert_eq!(s.search.flights(), &Phase::Idle);
    }

    #[test]
    fn a_failure_is_its_own_phase_and_places_never_hold_flights_back() {
        let fake = Fake::default();
        let mut s = state_with(&fake);
        s.poll(T0);
        type_(&mut s, "arlanda", T0);
        s.poll(T0 + 400.0);
        fake.answer_find(0, Err("timeout".into()));
        s.poll(T0 + 410.0);
        assert_eq!(s.search.flights(), &Phase::Error);
        assert!(s.search.flights().settled());
        assert_eq!(s.search.places(), &Phase::Searching, "the places on their own");
        fake.answer_geocode(0, Ok(vec![place("Vasa Museum")]));
        s.poll(T0 + 420.0);
        assert_eq!(s.search.places().results().len(), 1);
        assert!(!s.search.searching());
    }

    #[test]
    fn places_rank_near_the_centre_when_zoomed_in() {
        let fake = Fake::default();
        let mut s = state_with(&fake);
        s.controller = rustradar_scene::controller::Controller::new(ViewState { latitude: 59.33, longitude: 18.07, zoom: 9.0, ..ViewState::INITIAL }, [1440.0, 900.0]);
        s.poll(T0);
        type_(&mut s, "Vasa", T0);
        s.poll(T0 + PLACE_DEBOUNCE_MS);
        assert_eq!(fake.geocodes(), [("Vasa".to_string(), Some(LatLon { lat: 59.33, lon: 18.07 }), Some("en"))]);
        // a language Photon has no names in: local names
        s.search.set_language("sv-SE");
        type_(&mut s, "Vasa ", T0 + 1000.0);
        s.poll(T0 + 1000.0 + PLACE_DEBOUNCE_MS);
        assert_eq!(fake.geocodes()[1].2, None);
    }

    #[test]
    fn the_local_search_asks_no_flights_and_keeps_its_query_across_focus() {
        let fake = Fake::default();
        let mut s = state_with(&fake);
        s.persisted.settings.mode = Mode::Ships;
        s.poll(T0);
        type_(&mut s, "Stena", T0);
        assert_eq!(s.search.flights(), &Phase::Idle, "local: no flights");
        assert_eq!(s.search.places(), &Phase::Searching);
        assert_eq!(s.search.list_query(Mode::Ships), "Stena");
        // a new focus: a new box, idle; the ships' query stays for when they come back
        s.persisted.settings.mode = Mode::Aircraft;
        s.poll(T0 + 100.0);
        assert_eq!(s.search.places(), &Phase::Idle);
        s.poll(T0 + 1000.0);
        assert!(fake.geocodes().is_empty(), "the timer went with the box");
        assert_eq!(s.search.query(Mode::Aircraft), "");
        assert_eq!(s.search.list_query(Mode::Aircraft), "", "the aircraft's query narrows no list");
        type_(&mut s, "sas", T0 + 1100.0);
        s.persisted.settings.mode = Mode::Ships;
        s.poll(T0 + 1200.0);
        assert_eq!(s.search.query(Mode::Ships), "Stena");
        s.persisted.settings.mode = Mode::Aircraft;
        s.poll(T0 + 1300.0);
        assert_eq!(s.search.query(Mode::Aircraft), "", "the aircraft box starts empty again");
        // the lists' "Show all" empties the query alone
        s.persisted.settings.mode = Mode::Ships;
        assert!(s.apply(Action::Search(Ask::EmptyQuery), T0 + 1400.0));
        assert_eq!(s.search.query(Mode::Ships), "");
        assert!(!s.apply(Action::Search(Ask::EmptyQuery), T0 + 1500.0));
    }

    #[test]
    fn a_keystroke_and_clearing_let_go_of_the_place() {
        let fake = Fake::default();
        let mut s = state_with(&fake);
        s.poll(T0);
        let mark = || Some(MarkedPlace { lon: 18.0, lat: 59.3, name: "Here".into() });
        s.flight.set_place(mark());
        type_(&mut s, "x", T0);
        assert!(s.flight.place().is_none());
        s.flight.set_place(mark());
        type_(&mut s, "xyz", T0 + 10.0);
        assert!(s.apply(Action::Search(Ask::Clear), T0 + 20.0));
        assert!(s.flight.place().is_none());
        assert_eq!(s.search.query(Mode::Aircraft), "");
        assert_eq!((s.search.flights(), s.search.places()), (&Phase::Idle, &Phase::Idle));
        s.poll(T0 + 2000.0);
        assert!(fake.finds().is_empty(), "the timers went with it");
        // a Typed without text handed over does nothing
        assert!(!s.apply(Action::Search(Ask::Typed), T0 + 2100.0));
    }

    #[test]
    fn picking_a_live_flight_selects_and_flies_to_it() {
        let fake = Fake::default();
        let mut s = state_with(&fake);
        s.poll(T0);
        type_(&mut s, "sas", T0);
        s.poll(T0 + 250.0);
        fake.answer_find(0, Ok(vec![live("3a4b5c6", "SAS123", None)]));
        s.poll(T0 + 300.0);
        assert!(s.apply(Action::Search(Ask::Pick(Row::Flight(0))), T0 + 400.0));
        assert_eq!(s.flight.selected_id(), Some(0x3a4b5c6));
        assert_eq!(s.flight.selected().map(|f| f.callsign.as_str()), Some("SAS123"), "the hint stands in");
        // reset: empty, idle, nothing more asked
        assert_eq!(s.search.query(Mode::Aircraft), "");
        assert_eq!(s.search.places(), &Phase::Idle);
        s.poll(T0 + 2000.0);
        assert!(fake.geocodes().is_empty(), "the place search's timer went with the reset");
        // the camera is on its way there
        s.advance_camera(T0 + 10_000.0);
        let v = s.controller.view();
        assert!((v.latitude - 59.65).abs() < 1e-3 && (v.longitude - 17.92).abs() < 1e-3, "{v:?}");
        // a row that is not there does nothing
        assert!(!s.apply(Action::Search(Ask::Pick(Row::Flight(5))), T0 + 11_000.0));
    }

    #[test]
    fn picking_an_airport_flies_there_and_opens_its_board() {
        let fake = Fake::default();
        let mut s = state_with(&fake);
        s.poll(T0);
        s.select_flight(Some(7), SelectOptions { hint: Some(LiveFlight { id: 7, lat: 50.0, lon: 10.0, ..Default::default() }), ..Default::default() }, T0);
        type_(&mut s, "arn", T0);
        s.poll(T0 + 250.0);
        fake.answer_find(0, Ok(vec![airport("ARN"), airport("essa")]));
        s.poll(T0 + 300.0);
        assert!(s.apply(Action::Search(Ask::Pick(Row::Flight(0))), T0 + 400.0));
        assert_eq!(s.flight.focus_airport(), Some("ARN"));
        assert_eq!(s.flight.selected_id(), None, "the flight lets go");
        s.advance_camera(T0 + 10_000.0);
        let v = s.controller.view();
        assert!((v.zoom - AIRPORT_ZOOM).abs() < 1e-9 && (v.latitude - 59.6519).abs() < 1e-6, "{v:?}");
        // an id that is no IATA code flies there and opens nothing
        s.flight.set_focus_airport(None);
        type_(&mut s, "essa", T0 + 11_000.0);
        s.poll(T0 + 11_250.0);
        fake.answer_find(1, Ok(vec![airport("essa")]));
        s.poll(T0 + 11_300.0);
        assert!(s.apply(Action::Search(Ask::Pick(Row::Flight(0))), T0 + 11_400.0));
        assert_eq!(s.flight.focus_airport(), None);
    }

    #[test]
    fn picking_a_place_marks_it_and_flies_to_its_view() {
        let fake = Fake::default();
        let mut s = state_with(&fake);
        s.poll(T0);
        type_(&mut s, "vasa", T0);
        s.poll(T0 + 350.0);
        fake.answer_geocode(0, Ok(vec![place("Vasa Museum")]));
        s.poll(T0 + 400.0);
        assert!(s.apply(Action::Search(Ask::Pick(Row::Place(0))), T0 + 500.0));
        let marked = s.flight.place().expect("marked");
        assert_eq!((marked.name.as_str(), marked.lat, marked.lon), ("Vasa Museum", 59.328, 18.091));
        assert!(!s.flight.following());
        s.advance_camera(T0 + 10_000.0);
        let want = place_view(&place("Vasa Museum"), 1440.0, 900.0);
        let v = s.controller.view();
        assert!((v.zoom - want.zoom).abs() < 1e-9 && (v.latitude - want.lat).abs() < 1e-6, "{v:?} {want:?}");
        // Escape's first job is the place (U6's close_top)
        assert!(s.apply(Action::CloseTop, T0 + 11_000.0));
        assert!(s.flight.place().is_none());
    }

    fn train(id: &str, name: &str, destination: Option<&str>, delay: Option<i32>, lat: f64) -> Train {
        serde_json::from_value(serde_json::json!({
            "id": id, "source": "trafikverket", "name": name, "kind": "regional", "lat": lat, "lon": 18.0,
            "heading": null, "speedKmh": null, "destination": destination, "operator": "SJ", "delayMinutes": delay,
            "positionUnix": (T0 / 1000.0) as i64, "frames": [],
        }))
        .unwrap()
    }

    /// The state on a fixture folder with `trains` and `ships`, `mode` the focus, at Stockholm,
    /// polled until both have come; the searches on `fake`.
    fn canned(dir: &crate::persist::tests::TempDir, mode: Mode, trains: &[Train], ships: &[Ship], fake: &Fake) -> AppState {
        use crate::state::{ships as ships_state, trains as trains_state};
        std::fs::write(dir.0.join(trains_state::FIXTURE_TRAINS), serde_json::to_vec(trains).unwrap()).unwrap();
        std::fs::write(dir.0.join(ships_state::FIXTURE_SHIPS), serde_json::to_vec(ships).unwrap()).unwrap();
        let startup = Startup { fixture: Some(dir.0.clone()), ..Default::default() };
        let view = ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, pitch: 0.0, bearing: 0.0, position: None };
        let mut s = AppState::new(startup, None, None, None, view, [1440.0, 900.0]);
        s.persisted.settings.mode = mode;
        for n in 0..4 {
            s.poll(T0 + 16.0 * f64::from(n));
        }
        s.search = Search::with_feeds(Box::new(fake.clone()));
        s.search.set_language("en");
        s
    }

    #[test]
    fn local_rows_are_the_lists_first_six() {
        let fake = Fake::default();
        let mut trains: Vec<Train> = (0..9).map(|n| train(&format!("trafikverket:{n}"), &format!("Pendeltåg {n}"), Some("Märsta"), Some(n), 59.0 + f64::from(n) * 0.01)).collect();
        trains.push(train("trafikverket:99", "Regional 99", None, None, 59.5));
        trains.push(train("trafikverket:98", "Regional 98", Some(""), Some(0), 59.6));
        let dir = crate::persist::tests::TempDir::new("search-trains");
        let mut s = canned(&dir, Mode::Trains, &trains, &[], &fake);
        assert_eq!(s.trains.trains().len(), trains.len(), "the trains came");
        assert!(local_rows(&s, "  ", T0).is_empty(), "a blank query lists nothing");
        let rows = local_rows(&s, "pendel", T0);
        assert_eq!(rows.len(), LOCAL_MAX);
        // sorted by delay, most late first (the trains' default sort)
        assert_eq!(rows[0].label, "Pendeltåg 8");
        assert_eq!(rows[0].detail, "To Märsta · 8 min late");
        assert_eq!(rows[0].key, "trafikverket:8");
        assert_eq!(rows[0].color, train_color_of(&s.trains.trains()[8]));
        let rows = local_rows(&s, "regional", T0);
        let details: Vec<&str> = rows.iter().map(|r| r.detail.as_str()).collect();
        assert_eq!(details, ["on time", "Regional"], "no destination; the kind when the delay is unknown");
        // picked: selected, the camera on its way at zoom 10 or closer
        type_(&mut s, "regional 99", T0);
        assert!(s.apply(Action::Search(Ask::Pick(Row::Local(0))), T0 + 100.0));
        assert_eq!(s.trains.selected_id(), Some("trafikverket:99"));
        assert_eq!(s.search.query(Mode::Trains), "", "reset");
        s.advance_camera(T0 + 10_000.0);
        assert!((s.controller.view().zoom - 10.0).abs() < 1e-9);
    }

    #[test]
    fn ships_rows_name_the_kind_and_the_mmsi() {
        let fake = Fake::default();
        let ship = |mmsi: u32, name: &str, kind: u8| Ship { mmsi, lat: 59.33, lon: 18.07, name: name.into(), kind, ..Default::default() };
        let dir = crate::persist::tests::TempDir::new("search-ships");
        let mut s = canned(&dir, Mode::Ships, &[], &[ship(265_123_456, "STENA DANICA", 3), ship(265_000_001, "265000001", 7)], &fake);
        assert_eq!(s.ships.ships().len(), 2, "the ships came");
        let rows = local_rows(&s, "stena", T0);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].label.as_str(), rows[0].detail.as_str()), ("STENA DANICA", "Passenger · MMSI 265123456"));
        assert_eq!(rows[0].color, ship_color(3));
        let rows = local_rows(&s, "265000", T0);
        assert_eq!(rows[0].label, "MMSI 265000001", "a name that is its MMSI is no name");
        type_(&mut s, "265000", T0);
        assert!(s.apply(Action::Search(Ask::Pick(Row::Local(0))), T0 + 100.0));
        assert_eq!(s.ships.selected_mmsi(), Some(265_000_001));
    }

    #[test]
    fn a_fixture_folder_answers_every_query() {
        use crate::persist::tests::TempDir;
        let dir = TempDir::new("search-fixture");
        std::fs::write(dir.0.join(FIXTURE_FIND), serde_json::to_vec(&serde_json::json!([
            {"id": "3a4b5c6", "label": "SK1415", "type": "live", "detail": {"lat": 59.6, "lon": 17.9, "route": "Stockholm (ARN) ⟶ Copenhagen (CPH)", "callsign": "SAS1415"}},
            {"id": "SAS", "label": "SAS", "type": "operator", "detail": {"operator_id": 5}},
        ])).unwrap()).unwrap();
        let startup = Startup { fixture: Some(dir.0.clone()), ..Default::default() };
        let mut s = AppState::new(startup, None, None, None, ViewState::INITIAL, [1440.0, 900.0]);
        s.poll(T0);
        type_(&mut s, "sk14", T0);
        s.poll(T0 + 400.0);
        assert_eq!(s.search.flights().results().len(), 1);
        assert_eq!(flight_detail(&s.search.flights().results()[0]), "Stockholm (ARN) ⟶ Copenhagen (CPH)");
        assert_eq!(s.search.places(), &Phase::Error, "no places file: a failed request");
        // and without any backend every search fails
        let mut s = AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, [1440.0, 900.0]);
        s.poll(T0);
        type_(&mut s, "sk14", T0);
        s.poll(T0 + 400.0);
        assert_eq!((s.search.flights(), s.search.places()), (&Phase::Error, &Phase::Error));
    }

    #[test]
    fn parse_int_16_as_javascript_reads_an_id() {
        assert_eq!(parse_hex_id("3a4b5c6"), Some(0x3a4b5c6));
        assert_eq!(parse_hex_id("  0x1F zz"), Some(0x1f));
        assert_eq!(parse_hex_id("+ff"), Some(255));
        assert_eq!(parse_hex_id("12g4"), Some(0x12));
        assert_eq!(parse_hex_id("xyz"), None, "NaN");
        assert_eq!(parse_hex_id(""), None);
        assert_eq!(parse_hex_id("-1"), None);
        assert_eq!(parse_hex_id("-0"), Some(0));
        assert_eq!(parse_hex_id("1ffffffff"), None, "too large for an id");
        assert!(is_board_code("ARN") && is_board_code("A1B"));
        assert!(!is_board_code("arn") && !is_board_code("ESSA") && !is_board_code("AR"));
    }

    #[test]
    fn the_words_of_the_box() {
        assert_eq!(placeholder(Mode::Aircraft, false), "Flight, airport or address");
        assert_eq!(placeholder(Mode::Trains, false), "Train, destination or address");
        assert_eq!(placeholder(Mode::Trains, true), "Train or address");
        assert_eq!(placeholder(Mode::Ships, false), "Ship name, MMSI or address");
        assert_eq!(placeholder(Mode::Ships, true), "Ship or address");
        assert_eq!(local_words(Mode::Trains), Some(("Trains in view", "trains")));
        assert_eq!(local_words(Mode::Ships), Some(("Ships in view", "ships")));
        assert_eq!(local_words(Mode::Aircraft), None);
        assert_eq!(list_centre(59.3293, 18.0686), LatLon { lat: 59.35, lon: 18.05 });
    }
}
