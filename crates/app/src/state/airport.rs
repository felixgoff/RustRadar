//! The airport board (item U13; doc 06 §3.6, §8.3): which airport's board is open (`boardCode`),
//! and the board itself, a port of `AirportPanel.svelte`'s script (its `mode`, `board`, `failed`,
//! `busy`, `updatedAt` and `now`, `load`, the key effect and its two timers) and of the page's
//! `openAirport` and "aircraft off" effect (+page:940-958, 1070-1075, 1642-1654).
//!
//! **Open and closed** ([`AppState::open_airport`], `openAirport(code)`): the board of `code` in
//! upper case; the flight, the ship and the train are let go. It is opened from an airport code's
//! click on the map (`state::pick`), the flight panel's origin and destination codes, and an
//! airport found by the search (`state::search`). It closes with its close button and Escape
//! ([`Airport::close`], `boardCode = null`), when a ship or a train is selected (one panel at a
//! time), and when the aircraft leave the map. A flight selected while it is open (from the
//! board's rows, the map, a list) does not close it: the board is **kept alive but hidden** under
//! the flight's panel (`<div hidden={selected !== null}>`), its tab, pages and timers going on,
//! so the flight panel's back row finds it as it was.
//!
//! **The board's state** lives as long as the component would: opening an airport while none is
//! open starts it afresh (departures, nothing loaded, the panel's clock read now); opening
//! another one while it is open keeps the tab and changes the key; closing it ends it.
//! - The key is `{code}:{mode}`. When it changes (opened, another airport, the other tab): the
//!   board and `failed` are cleared, page 1 is loaded, and the two timers start again from now:
//!   every [`REFRESH_MS`] pages `1..=board.page` are loaded again ([`Ask`]s and the refresh), and
//!   every [`CLOCK_MS`] the panel's own clock ([`Airport::now_ms`]) is read.
//! - `load(key, pages)`: `busy`, then pages 1 to `pages` one after the other
//!   (`airportBoard(code, mode, p)`); when all have come and the key is still the same, the rows of
//!   all of them without repeats (by `{flightId ?? number}-{scheduled}`, the first kept) are the
//!   board, with `page = pages` and the first answer's `pages`; `failed` cleared and `updatedAt`
//!   now. A failed page stops the load and sets `failed` if the key is the same. Every load that
//!   ends (answered, failed, or for another key) clears `busy`, as the page's `finally` does,
//!   even while another load is still out.
//! - [`Airport::rows`] is the board of the current key (`board?.key === key ? board.rows :
//!   null`), [`Airport::more`] whether it has more pages, Show more loads one page more (all of
//!   them again, as today), Try again page 1.
//!
//! **Sources** ([`BoardFeeds`]): the backend, the `--fixture` folder, a test's, or none (the page
//! without Tauri: every page fails). The fixture folder's pages are
//! `airport_board/{CODE}-{mode}-{page}.json` ([`FIXTURE_BOARD`]; `mode` `departures` or
//! `arrivals`), each a `Board` as `airport_board` answers; a missing file is a failed page.
//!
//! Deviations from today's app:
//! - An ask of the UI (a tab, Show more, Try again) and an opening start their load at the next
//!   poll, a frame later, as the other parts' asks do (A1, A8).
//! - The timers are read at each poll, so they fire on the first frame at or after their time
//!   (the frame loop wakes for them); a late one fires once and keeps its cadence, as
//!   `setInterval` does.
//! - The panel's clock starts when the board opens; today the component reads `Date.now()` when it
//!   mounts, the same frame.

use std::path::PathBuf;

use rustradar_backend::Backend;
use rustradar_core::model::{Board, BoardMode, BoardRow};
use rustradar_core::settings::shown_kinds;

use super::aircraft::{Answer, NO_BACKEND};
use super::flight::SelectOptions;
use super::{AppState, Cx, Startup};
use crate::requests::Requests;

/// `REFRESH_MS`: the board's pages are loaded again this often while it is open.
pub const REFRESH_MS: f64 = 60_000.0;
/// The panel's own clock (`now`) is read this often.
pub const CLOCK_MS: f64 = 15_000.0;
/// The zoom Show on map flies to (`globe.flyTo(lon, lat, 11)`).
pub const LOCATE_ZOOM: f64 = 11.0;
/// The `--fixture` folder's board pages: `airport_board/{CODE}-{mode}-{page}.json`.
pub const FIXTURE_BOARD: &str = "airport_board";

/// The mode's name as the wire and the board's words have it (`departures`, `arrivals`).
pub fn mode_name(mode: BoardMode) -> &'static str {
    match mode {
        BoardMode::Departures => "departures",
        BoardMode::Arrivals => "arrivals",
    }
}

/// The key a row is told apart by (`${r.flightId ?? r.number}-${r.scheduled}`, `null` for what
/// is missing, as a template literal writes it).
pub fn row_key(r: &BoardRow) -> String {
    let id = match (r.flight_id, &r.number) {
        (Some(id), _) => id.to_string(),
        (None, Some(number)) => number.clone(),
        (None, None) => "null".to_owned(),
    };
    let scheduled = r.scheduled.map_or_else(|| "null".to_owned(), |s| s.to_string());
    format!("{id}-{scheduled}")
}

/// The rows of `pages` in order without repeats, the first of each key kept.
pub fn dedupe<'a>(pages: impl IntoIterator<Item = &'a Board>) -> Vec<BoardRow> {
    let mut seen = std::collections::HashSet::new();
    pages.into_iter().flat_map(|b| &b.rows).filter(|r| seen.insert(row_key(r))).cloned().collect()
}

/// Where the board's pages come from: the backend, the `--fixture` folder, or a test's.
pub trait BoardFeeds: Send {
    /// `airportBoard(code, mode, page)`.
    fn airport_board(&mut self, code: &str, mode: BoardMode, page: u32) -> Answer<Result<Board, String>>;
}

/// The backend's requests, on its runtime.
struct BackendFeeds {
    backend: Backend,
    requests: Requests,
}

impl BoardFeeds for BackendFeeds {
    fn airport_board(&mut self, code: &str, mode: BoardMode, page: u32) -> Answer<Result<Board, String>> {
        let (backend, code) = (self.backend.clone(), code.to_owned());
        self.requests.request(async move { backend.airport_board(&code, mode, Some(page)).await }).into()
    }
}

/// The `--fixture` folder's pages (see the module documentation), read when asked for.
struct FixtureFeeds {
    dir: PathBuf,
}

impl BoardFeeds for FixtureFeeds {
    fn airport_board(&mut self, code: &str, mode: BoardMode, page: u32) -> Answer<Result<Board, String>> {
        let path = self.dir.join(FIXTURE_BOARD).join(format!("{code}-{}-{page}.json", mode_name(mode)));
        let read = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice::<Board>(&bytes).map_err(|e| e.to_string()))
            .map_err(|e| format!("fixture {}: {e}", path.display()));
        Answer::ready(read)
    }
}

/// Which pages the board asks for.
enum Source {
    /// Chosen at the first poll: the fixture folder, else the backend, else none.
    Auto,
    /// None (the page without Tauri): every page fails.
    Off,
    On(Box<dyn BoardFeeds>),
}

/// The board's key (`{code}:{mode}`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    code: String,
    mode: BoardMode,
}

/// The board shown: its key, its rows, how many pages it holds and how many there are.
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    key: Key,
    rows: Vec<BoardRow>,
    page: u32,
    pages: Option<u64>,
}

/// A `load(key, pages)` out: the pages that came so far and the one on its way.
#[derive(Debug)]
struct Load {
    key: Key,
    pages: u32,
    results: Vec<Board>,
    answer: Answer<Result<Board, String>>,
}

/// What the board's panel asks (`Action::Board`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// A tab chosen (a click, Left or Right): `mode = id`.
    Mode(BoardMode),
    /// Show more: `load(key, board.page + 1)`.
    More,
    /// Try again: `load(key, 1)`.
    Retry,
    /// A row's flight: `select(id, {fly: true, fromBoard: true})`.
    Pick(u32),
    /// The close button: `boardCode = null`.
    Close,
}

/// The airport board (see the module documentation).
pub struct Airport {
    source: Source,
    started: bool,
    /// The aircraft were on the map at the last poll (`aircraftWere`).
    aircraft_were: bool,
    /// `boardCode`.
    code: Option<String>,
    mode: BoardMode,
    board: Option<Shown>,
    failed: bool,
    busy: bool,
    /// Local Unix ms of the last board loaded (`updatedAt`), 0 before.
    updated_at: f64,
    /// The panel's clock (`now`), local Unix ms.
    now_ms: f64,
    next_clock: f64,
    next_refresh: f64,
    /// Loads asked for since the last poll, with the key they were asked for.
    asks: Vec<(Key, u32)>,
    loads: Vec<Load>,
}

impl Default for Airport {
    /// The pages of the `--fixture` folder, else the backend, chosen at the first poll.
    fn default() -> Self {
        Airport::with_source(Source::Auto)
    }
}

impl std::fmt::Debug for Airport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Airport")
            .field("code", &self.code)
            .field("mode", &self.mode)
            .field("rows", &self.rows().map(<[BoardRow]>::len))
            .field("page", &self.page())
            .field("failed", &self.failed)
            .field("busy", &self.busy)
            .finish_non_exhaustive()
    }
}

/// The first tick of a timer of period `every` after `due` that is later than `now`.
fn after(due: f64, now: f64, every: f64) -> f64 {
    let mut next = due + every;
    if next <= now {
        next += ((now - next) / every).floor() * every + every;
    }
    next
}

impl Airport {
    fn with_source(source: Source) -> Self {
        Airport {
            source,
            started: false,
            aircraft_were: false,
            code: None,
            mode: BoardMode::Departures,
            board: None,
            failed: false,
            busy: false,
            updated_at: 0.0,
            now_ms: 0.0,
            next_clock: f64::INFINITY,
            next_refresh: f64::INFINITY,
            asks: Vec::new(),
            loads: Vec::new(),
        }
    }

    /// The board's pages from `feeds` instead of the fixture folder or the backend; `None`: no
    /// source at all (every page fails).
    pub fn with_feeds(feeds: Option<Box<dyn BoardFeeds>>) -> Self {
        Airport::with_source(match feeds {
            Some(feeds) => Source::On(feeds),
            None => Source::Off,
        })
    }

    /// Resolves [`Source::Auto`]: the fixture folder, else the backend, else none.
    fn resolve(&mut self, startup: &Startup, backend: Option<&Backend>, requests: Option<&Requests>) {
        if !matches!(self.source, Source::Auto) {
            return;
        }
        self.source = if let Some(dir) = &startup.fixture {
            Source::On(Box::new(FixtureFeeds { dir: dir.clone() }))
        } else if let (Some(backend), Some(requests)) = (backend, requests) {
            Source::On(Box::new(BackendFeeds { backend: backend.clone(), requests: requests.clone() }))
        } else {
            Source::Off
        };
    }

    // ---------------------------------------------------------------- what the UI reads

    /// The airport whose board is open (`boardCode`, upper case), marked on the map.
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    /// The tab shown (`mode`).
    pub fn mode(&self) -> BoardMode {
        self.mode
    }

    /// The rows of the board of this key, or `None` before it has loaded (`rows`).
    pub fn rows(&self) -> Option<&[BoardRow]> {
        let key = self.key()?;
        self.board.as_ref().filter(|b| b.key == key).map(|b| b.rows.as_slice())
    }

    /// How many pages the board holds (`board.page`), if it has loaded.
    pub fn page(&self) -> Option<u32> {
        self.board.as_ref().map(|b| b.page)
    }

    /// Whether there are more pages than the board holds (`more`).
    pub fn more(&self) -> bool {
        self.board.as_ref().is_some_and(|b| b.pages.is_some_and(|pages| u64::from(b.page) < pages))
    }

    /// The last load of this key failed (`failed`).
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// A load is out (`busy`).
    pub fn busy(&self) -> bool {
        self.busy
    }

    /// Local Unix ms of the last board loaded (`updatedAt`), 0 before.
    pub fn updated_at(&self) -> f64 {
        self.updated_at
    }

    /// The panel's clock (`now`): local Unix ms, read when the board opened and every
    /// [`CLOCK_MS`] since its key last changed.
    pub fn now_ms(&self) -> f64 {
        self.now_ms
    }

    fn key(&self) -> Option<Key> {
        self.code.clone().map(|code| Key { code, mode: self.mode })
    }

    // ---------------------------------------------------------------- the panel's functions

    /// Opens the board of `code` at local time `now` (`boardCode = code.toUpperCase()`; see the
    /// module documentation). [`AppState::open_airport`] also lets go of the flight, the ship
    /// and the train. Returns whether the board's key changed.
    pub fn open(&mut self, code: &str, now: f64) -> bool {
        let code = code.to_uppercase();
        if self.code.as_deref() == Some(code.as_str()) {
            return false;
        }
        if self.code.is_none() {
            // mounted: the component's state from its start
            self.mode = BoardMode::Departures;
            self.board = None;
            self.failed = false;
            self.busy = false;
            self.updated_at = 0.0;
            self.now_ms = now;
            self.asks.clear();
            self.loads.clear();
        }
        self.code = Some(code);
        self.key_changed(now);
        true
    }

    /// Closes the board (`boardCode = null`): what it was loading is dropped. Returns whether it
    /// was open.
    pub fn close(&mut self) -> bool {
        self.asks.clear();
        self.loads.clear();
        self.next_clock = f64::INFINITY;
        self.next_refresh = f64::INFINITY;
        self.code.take().is_some()
    }

    /// The other tab (`mode = id`). Returns whether it changed.
    pub fn set_mode(&mut self, mode: BoardMode, now: f64) -> bool {
        if self.code.is_none() || mode == self.mode {
            return false;
        }
        self.mode = mode;
        self.key_changed(now);
        true
    }

    /// Show more: `board && load(key, board.page + 1)`. Returns whether a load was asked for.
    pub fn show_more(&mut self) -> bool {
        match (self.key(), &self.board) {
            (Some(key), Some(board)) => {
                let pages = board.page + 1;
                self.asks.push((key, pages));
                true
            }
            _ => false,
        }
    }

    /// Try again: `load(key, 1)`. Returns whether a load was asked for.
    pub fn retry(&mut self) -> bool {
        match self.key() {
            Some(key) => {
                self.asks.push((key, 1));
                true
            }
            None => false,
        }
    }

    /// The key effect: the board cleared, page 1 asked for, the timers from `now`.
    fn key_changed(&mut self, now: f64) {
        self.board = None;
        self.failed = false;
        if let Some(key) = self.key() {
            self.asks.push((key, 1));
        }
        self.next_refresh = now + REFRESH_MS;
        self.next_clock = now + CLOCK_MS;
    }

    fn request(&mut self, key: &Key, page: u32) -> Answer<Result<Board, String>> {
        match &mut self.source {
            Source::On(feeds) => feeds.airport_board(&key.code, key.mode, page),
            Source::Auto | Source::Off => Answer::ready(Err(NO_BACKEND.to_owned())),
        }
    }

    /// `load(k, pages)`: `busy`, and page 1 asked for.
    fn start(&mut self, key: Key, pages: u32) {
        self.busy = true;
        let answer = self.request(&key, 1);
        self.loads.push(Load { key, pages: pages.max(1), results: Vec::new(), answer });
    }

    /// Takes the loads' answers at local time `now`, asking for each next page as its page comes.
    /// Returns whether anything ended.
    fn take_answers(&mut self, now: f64) -> bool {
        let mut ended = false;
        let mut i = 0;
        while i < self.loads.len() {
            let Some(answer) = self.loads[i].answer.poll() else {
                i += 1;
                continue;
            };
            match answer {
                Ok(board) => {
                    let load = &mut self.loads[i];
                    load.results.push(board);
                    if (load.results.len() as u32) < load.pages {
                        let (key, page) = (load.key.clone(), load.results.len() as u32 + 1);
                        self.loads[i].answer = self.request(&key, page);
                        // a page that is there already is taken in this same pass
                        continue;
                    }
                    let load = self.loads.remove(i);
                    if self.key().as_ref() == Some(&load.key) {
                        let rows = dedupe(&load.results);
                        let pages = load.results.first().and_then(|b| b.pages);
                        self.board = Some(Shown { key: load.key, rows, page: load.pages, pages });
                        self.failed = false;
                        self.updated_at = now;
                    }
                }
                Err(e) => {
                    let load = self.loads.remove(i);
                    log::debug!("airport board {}:{} failed: {e}", load.key.code, mode_name(load.key.mode));
                    if self.key().as_ref() == Some(&load.key) {
                        self.failed = true;
                    }
                }
            }
            // `finally`
            self.busy = false;
            ended = true;
        }
        ended
    }

    /// This frame's step (see [`AppState::poll`] for the order).
    pub fn poll(&mut self, cx: &mut Cx<'_>) {
        let now = cx.clocks.local_ms;
        let settings = &cx.persisted.settings;
        let on = shown_kinds(settings.mode, &settings.also_show).aircraft;
        if !self.started {
            self.started = true;
            self.aircraft_were = on;
            self.resolve(cx.startup, cx.backend, cx.requests);
        }
        // the aircraft off the map: the board closes with the flight
        if on != self.aircraft_were {
            self.aircraft_were = on;
            if !on && self.close() {
                cx.wants.scene = true;
            }
        }
        let Some(key) = self.key() else { return };
        // the panel's timers, running while it is hidden under a flight too
        if now >= self.next_clock {
            self.now_ms = now;
            self.next_clock = after(self.next_clock, now, CLOCK_MS);
        }
        if now >= self.next_refresh {
            let pages = self.board.as_ref().map_or(1, |b| b.page);
            self.asks.push((key, pages));
            self.next_refresh = after(self.next_refresh, now, REFRESH_MS);
        }
        for (key, pages) in std::mem::take(&mut self.asks) {
            self.start(key, pages);
        }
        self.take_answers(now);
        cx.wants.wake_at(self.next_clock.min(self.next_refresh));
    }
}

impl AppState {
    /// `openAirport(code)`: the board of `code` (upper case), and the flight, the ship and the
    /// train let go, at local time `now_ms`.
    pub fn open_airport(&mut self, code: &str, now_ms: f64) {
        self.airport.open(code, now_ms);
        if self.flight.selected_id().is_some() {
            self.select_flight(None, SelectOptions::default(), now_ms);
        }
        self.select_ship(None, now_ms);
        self.select_train(None, now_ms);
    }

    /// The board panel's `ask` at local time `now_ms`. Returns whether it did something.
    pub fn board_ask(&mut self, ask: Ask, now_ms: f64) -> bool {
        match ask {
            Ask::Mode(mode) => self.airport.set_mode(mode, now_ms),
            Ask::More => self.airport.show_more(),
            Ask::Retry => self.airport.retry(),
            Ask::Pick(id) => {
                self.select_flight(Some(id), SelectOptions { fly: true, from_board: true, ..Default::default() }, now_ms);
                true
            }
            Ask::Close => self.airport.close(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};

    use rustradar_core::model::{LiveFlight, LiveSnapshot};
    use rustradar_core::settings::Mode;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::persist::tests::TempDir;
    use crate::state::{Action, aircraft};
    use crate::ui::shell::{self, Panel};

    const T0: f64 = 1_782_036_000_000.0;

    fn row(id: Option<u32>, number: &str, scheduled: i64) -> BoardRow {
        BoardRow { flight_id: id, number: (!number.is_empty()).then(|| number.to_owned()), scheduled: Some(scheduled), live: id.is_some(), ..Default::default() }
    }

    fn page(rows: Vec<BoardRow>, page: u64, pages: u64) -> Board {
        Board { total: Some(rows.len() as u64), rows, page, pages: Some(pages) }
    }

    /// What the canned feeds were asked, and the pages they answer.
    #[derive(Default)]
    struct Shared {
        pages: HashMap<(String, BoardMode, u32), Result<Board, String>>,
        asked: Vec<(String, BoardMode, u32)>,
        /// Hold the answers back: their senders, in the order asked.
        hold: bool,
        held: Vec<Sender<Result<Board, String>>>,
    }

    #[derive(Clone, Default)]
    struct Canned(Arc<Mutex<Shared>>);

    impl Canned {
        fn set(&self, code: &str, mode: BoardMode, n: u32, answer: Result<Board, String>) {
            self.0.lock().unwrap().pages.insert((code.to_owned(), mode, n), answer);
        }

        fn asked(&self) -> Vec<(String, BoardMode, u32)> {
            std::mem::take(&mut self.0.lock().unwrap().asked)
        }

        fn hold(&self, on: bool) {
            self.0.lock().unwrap().hold = on;
        }

        /// The held answer `i` sent as the page it asked for.
        fn release(&self, i: usize, answer: Result<Board, String>) {
            let shared = self.0.lock().unwrap();
            // a dropped load's receiver is gone: the send goes nowhere, as today's late answer
            let _ = shared.held[i].send(answer);
        }
    }

    impl BoardFeeds for Canned {
        fn airport_board(&mut self, code: &str, mode: BoardMode, n: u32) -> Answer<Result<Board, String>> {
            let mut shared = self.0.lock().unwrap();
            shared.asked.push((code.to_owned(), mode, n));
            if shared.hold {
                let (tx, rx) = mpsc::channel();
                shared.held.push(tx);
                return rx.into();
            }
            let answer = shared.pages.get(&(code.to_owned(), mode, n)).cloned().unwrap_or_else(|| Err(format!("no page {code} {n}")));
            Answer::ready(answer)
        }
    }

    fn state_with(canned: &Canned, startup: Startup) -> AppState {
        let mut s = AppState::new(startup, None, None, None, ViewState::INITIAL, [1440.0, 900.0]);
        s.airport = Airport::with_feeds(Some(Box::new(canned.clone())));
        s.poll(T0);
        s
    }

    fn state(canned: &Canned) -> AppState {
        state_with(canned, Startup::default())
    }

    fn numbers(s: &AppState) -> Option<Vec<String>> {
        s.airport.rows().map(|rows| rows.iter().map(|r| r.number.clone().unwrap_or_default()).collect())
    }

    #[test]
    fn rows_are_told_apart_as_the_template_literal_writes_them() {
        assert_eq!(row_key(&row(Some(0x2f3a1b), "SK1415", 1_782_036_000)), "3095067-1782036000");
        assert_eq!(row_key(&row(None, "SK1415", 1_782_036_000)), "SK1415-1782036000");
        assert_eq!(row_key(&BoardRow::default()), "null-null");
        // a code-share's second number on the same flight and time is the same row
        let a = page(vec![row(Some(7), "SK1", 100), row(None, "BA2", 200)], 1, 2);
        let b = page(vec![row(Some(7), "AY9", 100), row(None, "BA2", 300), row(None, "BA2", 200)], 2, 2);
        let rows = dedupe([&a, &b]);
        let keys: Vec<_> = rows.iter().map(|r| (r.number.as_deref().unwrap(), r.scheduled.unwrap())).collect();
        assert_eq!(keys, [("SK1", 100), ("BA2", 200), ("BA2", 300)]);
        assert_eq!(mode_name(BoardMode::Departures), "departures");
        assert_eq!(mode_name(BoardMode::Arrivals), "arrivals");
    }

    #[test]
    fn opening_loads_page_one_and_the_tabs_clear_and_load_again() {
        let canned = Canned::default();
        canned.set("ARN", BoardMode::Departures, 1, Ok(page(vec![row(Some(1), "SK1", 100), row(Some(2), "SK2", 200)], 1, 2)));
        canned.set("ARN", BoardMode::Departures, 2, Ok(page(vec![row(Some(2), "SK2", 200), row(Some(3), "SK3", 300)], 2, 2)));
        canned.set("ARN", BoardMode::Arrivals, 1, Ok(page(vec![row(Some(9), "LH9", 150)], 1, 1)));
        let mut s = state(&canned);
        assert_eq!(s.airport.code(), None);
        s.open_airport("arn", T0 + 10.0);
        assert_eq!(s.airport.code(), Some("ARN"));
        assert_eq!(s.airport.mode(), BoardMode::Departures);
        assert_eq!(s.airport.rows(), None, "loading until the next poll");
        assert_eq!(s.airport.now_ms(), T0 + 10.0, "the panel's clock read when it opened");
        s.poll(T0 + 16.0);
        assert_eq!(canned.asked(), [("ARN".to_owned(), BoardMode::Departures, 1)]);
        assert_eq!(numbers(&s), Some(vec!["SK1".into(), "SK2".into()]));
        assert!(s.airport.more() && !s.airport.failed() && !s.airport.busy());
        assert_eq!(s.airport.updated_at(), T0 + 16.0);
        // the same airport again changes nothing
        assert!(!s.airport.open("ARN", T0 + 20.0));
        s.poll(T0 + 32.0);
        assert!(canned.asked().is_empty());

        // Show more: pages 1 and 2 again, one after the other, without repeats
        assert!(s.apply(Action::Board(Ask::More), T0 + 40.0));
        s.poll(T0 + 48.0);
        assert_eq!(canned.asked(), [("ARN".to_owned(), BoardMode::Departures, 1), ("ARN".to_owned(), BoardMode::Departures, 2)]);
        assert_eq!(numbers(&s), Some(vec!["SK1".into(), "SK2".into(), "SK3".into()]));
        assert_eq!(s.airport.page(), Some(2));
        assert!(!s.airport.more(), "2 of 2");

        // the other tab: cleared, then its page 1
        assert!(s.apply(Action::Board(Ask::Mode(BoardMode::Arrivals)), T0 + 50.0));
        assert_eq!(s.airport.rows(), None);
        assert!(!s.apply(Action::Board(Ask::Mode(BoardMode::Arrivals)), T0 + 51.0), "already shown");
        s.poll(T0 + 64.0);
        assert_eq!(canned.asked(), [("ARN".to_owned(), BoardMode::Arrivals, 1)]);
        assert_eq!(numbers(&s), Some(vec!["LH9".into()]));

        // another airport while open keeps the tab; closed and opened again starts afresh
        canned.set("GOT", BoardMode::Arrivals, 1, Ok(page(vec![], 1, 1)));
        s.open_airport("got", T0 + 70.0);
        assert_eq!(s.airport.mode(), BoardMode::Arrivals);
        s.poll(T0 + 80.0);
        assert_eq!(canned.asked(), [("GOT".to_owned(), BoardMode::Arrivals, 1)]);
        assert_eq!(s.airport.rows(), Some(&[][..]), "an empty board");
        assert!(s.apply(Action::Board(Ask::Close), T0 + 90.0));
        assert_eq!(s.airport.code(), None);
        assert!(!s.apply(Action::Board(Ask::Close), T0 + 91.0));
        s.open_airport("ARN", T0 + 100.0);
        assert_eq!(s.airport.mode(), BoardMode::Departures, "a new board starts on its departures");
        assert_eq!(s.airport.updated_at(), 0.0);
        assert_eq!(s.airport.now_ms(), T0 + 100.0);
    }

    #[test]
    fn the_board_reloads_its_pages_every_minute_and_its_clock_every_15_s() {
        let canned = Canned::default();
        canned.set("ARN", BoardMode::Departures, 1, Ok(page(vec![row(Some(1), "SK1", 100)], 1, 3)));
        canned.set("ARN", BoardMode::Departures, 2, Ok(page(vec![row(Some(2), "SK2", 200)], 2, 3)));
        let mut s = state(&canned);
        s.open_airport("ARN", T0);
        s.poll(T0 + 16.0);
        s.apply(Action::Board(Ask::More), T0 + 20.0);
        s.poll(T0 + 32.0);
        canned.asked();
        assert_eq!(s.airport.page(), Some(2));
        // the clock: 15 s after opening, then every 15 s
        s.poll(T0 + 14_999.0);
        assert_eq!(s.airport.now_ms(), T0);
        s.poll(T0 + 15_020.0);
        assert_eq!(s.airport.now_ms(), T0 + 15_020.0);
        s.poll(T0 + 29_000.0);
        assert_eq!(s.airport.now_ms(), T0 + 15_020.0, "the next tick is at 30 s, not 15 s after the late one");
        s.poll(T0 + 30_000.0);
        assert_eq!(s.airport.now_ms(), T0 + 30_000.0);
        assert!(canned.asked().is_empty());
        // a minute after opening: the two pages it holds, again
        let mut wants = s.poll(T0 + 59_000.0);
        assert!(canned.asked().is_empty());
        assert!(wants.wake_at.is_some_and(|at| at <= T0 + 60_000.0), "{wants:?}");
        wants = s.poll(T0 + 60_000.0);
        assert_eq!(canned.asked(), [("ARN".to_owned(), BoardMode::Departures, 1), ("ARN".to_owned(), BoardMode::Departures, 2)]);
        assert_eq!(s.airport.updated_at(), T0 + 60_000.0);
        assert!(wants.wake_at.is_some_and(|at| at <= T0 + 75_000.0), "{wants:?}");
        // a tab change starts both timers again from its time
        assert_eq!(s.airport.now_ms(), T0 + 60_000.0);
        s.apply(Action::Board(Ask::Mode(BoardMode::Arrivals)), T0 + 70_000.0);
        s.poll(T0 + 70_016.0);
        canned.asked();
        s.poll(T0 + 84_000.0);
        assert_eq!(s.airport.now_ms(), T0 + 60_000.0, "the old cadence's 75 s tick is gone");
        s.poll(T0 + 85_000.0);
        assert_eq!(s.airport.now_ms(), T0 + 85_000.0);
        s.poll(T0 + 129_000.0);
        assert!(canned.asked().is_empty(), "not a minute since the tab changed");
        s.poll(T0 + 130_000.0);
        assert_eq!(canned.asked(), [("ARN".to_owned(), BoardMode::Arrivals, 1)], "the arrivals' page 1 again (it failed: nothing held)");
    }

    #[test]
    fn a_failure_shows_try_again_and_a_failed_refresh_keeps_the_rows() {
        let canned = Canned::default();
        let mut s = state(&canned);
        s.open_airport("ARN", T0);
        s.poll(T0 + 16.0);
        assert!(s.airport.failed() && s.airport.rows().is_none() && !s.airport.busy());
        canned.set("ARN", BoardMode::Departures, 1, Ok(page(vec![row(Some(1), "SK1", 100)], 1, 1)));
        assert!(s.apply(Action::Board(Ask::Retry), T0 + 20.0));
        s.poll(T0 + 32.0);
        assert!(!s.airport.failed());
        assert_eq!(numbers(&s), Some(vec!["SK1".into()]));
        // the next refresh fails: the rows stay, failed is set
        canned.set("ARN", BoardMode::Departures, 1, Err("502".into()));
        s.poll(T0 + 60_000.0);
        assert!(s.airport.failed());
        assert_eq!(numbers(&s), Some(vec!["SK1".into()]));
        // no backend: every page fails
        let mut none = AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, [1440.0, 900.0]);
        none.poll(T0);
        none.open_airport("ARN", T0);
        none.poll(T0 + 16.0);
        assert!(none.airport.failed() && none.airport.rows().is_none());
    }

    #[test]
    fn a_load_for_another_key_is_dropped_but_ends_busy() {
        let canned = Canned::default();
        canned.hold(true);
        let mut s = state(&canned);
        s.open_airport("ARN", T0);
        s.poll(T0 + 16.0);
        assert!(s.airport.busy());
        // the arrivals asked for while the departures are on their way
        s.apply(Action::Board(Ask::Mode(BoardMode::Arrivals)), T0 + 20.0);
        s.poll(T0 + 32.0);
        assert_eq!(canned.asked(), [("ARN".to_owned(), BoardMode::Departures, 1), ("ARN".to_owned(), BoardMode::Arrivals, 1)]);
        // the departures come: not shown, and busy cleared though the arrivals are still out
        canned.release(0, Ok(page(vec![row(Some(1), "SK1", 100)], 1, 1)));
        s.poll(T0 + 48.0);
        assert_eq!(s.airport.rows(), None);
        assert!(!s.airport.busy(), "the page's finally");
        assert!(!s.airport.failed());
        // a failure for the old key would not set failed either; the arrivals' answer shows
        canned.release(1, Ok(page(vec![row(Some(9), "LH9", 150)], 1, 1)));
        s.poll(T0 + 64.0);
        assert_eq!(numbers(&s), Some(vec!["LH9".into()]));
        // a page 2 of a Show more waits for page 1
        s.apply(Action::Board(Ask::Retry), T0 + 70.0);
        s.poll(T0 + 80.0);
        assert_eq!(canned.asked(), [("ARN".to_owned(), BoardMode::Arrivals, 1)]);
        assert!(s.airport.busy());
        // closed: the load is dropped with the board
        assert!(s.airport.close());
        canned.release(2, Ok(page(vec![], 1, 1)));
        s.poll(T0 + 96.0);
        assert_eq!(s.airport.code(), None);
        assert_eq!(s.airport.rows(), None);
    }

    /// The plan's acceptance: the board is kept alive but hidden while a flight is selected, its
    /// tab, pages and timers going on, and shown again as it was when the flight closes.
    #[test]
    fn the_board_is_kept_alive_but_hidden_while_a_flight_is_selected() {
        let dir = TempDir::new("airport-board-kept");
        let live = LiveFlight { id: 8, callsign: "SAS8".into(), lat: 59.65, lon: 17.92, alt: 3_000, ..Default::default() };
        let snapshot = LiveSnapshot { flights: vec![live], server_time_ms: T0 as u64, ..Default::default() };
        std::fs::write(dir.0.join(aircraft::FIXTURE_LIVE), serde_json::to_vec(&snapshot).unwrap()).unwrap();
        let canned = Canned::default();
        canned.set("ARN", BoardMode::Arrivals, 1, Ok(page(vec![row(Some(8), "SK8", 100), row(Some(5), "SK5", 200)], 1, 2)));
        canned.set("ARN", BoardMode::Arrivals, 2, Ok(page(vec![row(Some(6), "SK6", 300)], 2, 2)));
        let mut s = state_with(&canned, Startup { fixture: Some(dir.0.clone()), ..Default::default() });
        for t in [T0 + 16.0, T0 + 32.0] {
            s.poll(t);
        }
        assert!(s.aircraft.flight(8).is_some(), "the feed has the flight");
        s.open_airport("ARN", T0 + 40.0);
        s.apply(Action::Board(Ask::Mode(BoardMode::Arrivals)), T0 + 41.0);
        s.poll(T0 + 48.0);
        s.apply(Action::Board(Ask::More), T0 + 50.0);
        s.poll(T0 + 64.0);
        assert_eq!(s.airport.page(), Some(2));
        assert_eq!(shell::panels(&s), [Panel::Airport]);
        canned.asked();

        // a row's flight: selected from the board, flown to; the board stays open under it
        assert!(s.apply(Action::Board(Ask::Pick(8)), T0 + 100.0));
        assert_eq!(s.flight.selected_id(), Some(8));
        assert!(s.flight.from_board());
        assert!(s.flight.selected().is_some());
        assert_eq!(s.airport.code(), Some("ARN"));
        assert_eq!(shell::panels(&s), [Panel::Flight], "hidden under the flight");
        assert!(shell::detail_open(&s));

        // hidden, it goes on: its clock and its minute's reload of both pages
        s.poll(T0 + 50_000.0);
        assert_eq!(s.airport.now_ms(), T0 + 50_000.0, "a late tick reads the time it fires at");
        s.poll(T0 + 60_041.0);
        assert_eq!(canned.asked(), [("ARN".to_owned(), BoardMode::Arrivals, 1), ("ARN".to_owned(), BoardMode::Arrivals, 2)]);
        assert_eq!(s.airport.updated_at(), T0 + 60_041.0);

        // Escape closes the flight first; the board shows again with its tab and pages
        assert!(s.apply(Action::CloseTop, T0 + 61_000.0));
        assert_eq!(s.flight.selected_id(), None);
        assert_eq!(shell::panels(&s), [Panel::Airport]);
        assert_eq!(s.airport.mode(), BoardMode::Arrivals);
        assert_eq!(s.airport.page(), Some(2));
        assert_eq!(numbers(&s), Some(vec!["SK8".into(), "SK5".into(), "SK6".into()]));
        // and then the board
        assert!(s.apply(Action::CloseTop, T0 + 62_000.0));
        assert_eq!(s.airport.code(), None);
        assert!(shell::panels(&s).is_empty());
    }

    #[test]
    fn opening_lets_the_others_go_and_the_aircraft_leaving_close_it() {
        let canned = Canned::default();
        let mut s = state(&canned);
        s.select_flight(Some(8), SelectOptions::default(), T0);
        assert_eq!(s.flight.selected_id(), Some(8));
        s.open_airport("ARN", T0 + 10.0);
        assert_eq!(s.flight.selected_id(), None, "openAirport lets the flight go");
        assert!(!s.flight.from_board());
        // the aircraft off the map: the board closes
        s.persisted.settings.mode = Mode::Ships;
        let wants = s.poll(T0 + 20.0);
        assert_eq!(s.airport.code(), None);
        assert!(wants.scene, "its mark leaves the map");
        // opened while they are off, it stays until they leave again
        s.open_airport("ARN", T0 + 30.0);
        s.poll(T0 + 40.0);
        assert_eq!(s.airport.code(), Some("ARN"));
        s.persisted.settings.mode = Mode::Aircraft;
        s.poll(T0 + 50.0);
        assert_eq!(s.airport.code(), Some("ARN"));
    }
}
