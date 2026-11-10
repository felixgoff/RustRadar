//! The aircraft feed (A1; doc 06 §3.1, §3.4; `+page.svelte:100-142`, `:866-985`, `:1335-1349`):
//! when the live flights are asked for and what is kept of them, the most tracked flights, the
//! reference data, the sign-in state the feed reports, and the list the map draws.
//!
//! Today's page runs this on `setTimeout`s, `setInterval`s and `$effect`s. Here every timer is a
//! deadline in local Unix ms, checked in [`Aircraft::poll`] once a frame and handed to the frame
//! loop as [`Wants::wake_at`](super::Wants); every effect is a comparison with what the last
//! poll saw. A poll runs, in this order:
//!
//! 1. the first poll only: what the page does on mount (`refreshTop` for the aircraft focus,
//!    `reference.load()`, `sessionInfo()`, `refresh()`, the 60 s top-flights interval);
//! 2. the effects, in the page's order: the view-change trigger (3° of arc or 1.5 zoom levels
//!    from the fetched view, settled for 800 ms), aircraft on or off the map, a change of the
//!    category filter (`refreshSoon`), a change of focus to aircraft (`refreshTop`);
//! 3. what the UI asked since the last poll ([`Aircraft::retry`], [`Aircraft::refresh_soon`],
//!    [`Aircraft::retry_top`]);
//! 4. the timers that are due: the view trigger's debounce, the next refresh, the top-flights
//!    interval;
//! 5. the answers that have arrived (`refresh`'s `try … finally`, `refreshTop`,
//!    `reference.load`, then `motion.setAirports`);
//! 6. the filtered list, again when the snapshot or the filters changed (a scene refresh).
//!
//! Where the answers come from ([`Feeds`]): the `--fixture` folder ([`FIXTURE_LIVE`],
//! [`FIXTURE_TOP`], [`FIXTURE_REFERENCE`]: the wire JSON of `live_flights`, `top_flights` and
//! `reference_data`, read again at each request; a missing file answers as a failed request),
//! else the backend ([`Backend::live_flights`], [`Backend::top_flights`],
//! [`Backend::reference_data`], [`Backend::session_info`]), else nothing ([`NO_BACKEND`], as the
//! page without Tauri: no request, no timer). [`Aircraft::with_feeds`] sets them instead (tests,
//! and shots without a fixture, which must not touch the network: `with_feeds(None)`).
//!
//! What the others read: [`Aircraft::shown`] (what the globe draws: the filtered flights with
//! the selected flight substituted in, or appended when filtered out; the selected row is
//! `state::flight`'s), [`Aircraft::flight`] (the feed's row by id), the sidebar's
//! [`Aircraft::freshness`], [`Aircraft::retry_in`], [`Aircraft::feed_error`],
//! [`Aircraft::rate_limited`], [`Aircraft::partial`], [`Aircraft::top`], and
//! [`Aircraft::reference`].
//!
//! Deviations from the page, all within a frame or invisible:
//! - The UI's asks ([`Aircraft::retry`], [`Aircraft::refresh_soon`], [`Aircraft::retry_top`])
//!   take effect at the next poll, the next frame, not inside the click.
//! - The view trigger restarts its debounce when the view's values change. The page restarts it
//!   on every report of the view, even of the same values (a vehicle lock standing still).
//! - `sessionInfo()` is read at once (it is a lock read in the backend), not on a later tick.
//! - Top-flights answers that overlap are taken in the order they were asked, as they arrive.
//! - A fixture's snapshot is not cut to the area or the categories asked for (the server did
//!   that; a `LiveFlight` carries no category).
//!
//! Left to the items that own them: deselecting the flight and closing the airport board when
//! the aircraft leave the map (`state::flight`, `state::airport`: they see the same
//! `shown_kinds`), `unfollowFlight()` on mount (no stream outlives the native app), the 1 s clock
//! tick of the sidebar's texts (the UI).

use std::collections::HashMap;
use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use rustradar_backend::{Backend, convert};
use rustradar_core::filters::{Filters, matcher, server_categories};
use rustradar_core::freshness::{FeedClock, Freshness, Round, after_round, feed_freshness};
use rustradar_core::geo::{self, Area, cos_angle, view_area};
use rustradar_core::model::{
    Airport, BoundingBox, LiveFlight, LiveSnapshot, ReferenceData, SessionInfo, TopFlight, is_rate_limited,
};
use rustradar_core::reference::Reference;
use rustradar_core::settings::{Mode, shown_kinds};
use rustradar_scene::camera::ViewState;

use super::Cx;
use crate::requests::{Pending, Requests};

/// The feed's interval when the view covers the world (no area), ms (`WORLD_REFRESH_MS`).
pub const WORLD_REFRESH_MS: i64 = 15_000;
/// The feed's interval for a regional view, ms (`AREA_REFRESH_MS`).
pub const AREA_REFRESH_MS: i64 = 8_000;
/// The least spacing between two feed requests, ms (`MIN_GAP_MS`).
pub const MIN_GAP_MS: f64 = 4_000.0;
/// The interval's largest multiplier while rate limited: it doubles, 1, 2, 4, 6 (`MAX_BACKOFF`).
pub const MAX_BACKOFF: i64 = 6;
/// The most tracked flights are asked for this often while aircraft are the focus, ms.
pub const TOP_REFRESH_MS: f64 = 60_000.0;
/// `refreshSoon` waits at least this long, ms.
pub const SOON_MS: f64 = 300.0;
/// The view trigger fires once the view has stood this long, ms.
pub const VIEW_SETTLE_MS: f64 = 800.0;
/// The view trigger: the view's centre this far (degrees of arc) from the fetched view's...
pub const VIEW_MOVE_DEG: f64 = 3.0;
/// ... or its zoom more than this far from the fetched view's.
pub const VIEW_ZOOM_CHANGE: f64 = 1.5;

/// The feed's error when the backend did not start and there is no fixture folder (the page's
/// "needs its Rust backend" without Tauri).
pub const NO_BACKEND: &str = "RustRadar's backend did not start, so there is no live data.";
/// The feed's error when it was turned off ([`Aircraft::with_feeds`] with `None`: a shot
/// without a fixture folder).
pub const NO_FEED: &str = "No live data in this run.";

/// The `--fixture` folder's snapshot: a `LiveSnapshot` as `live_flights` sends it.
pub const FIXTURE_LIVE: &str = "live_flights.json";
/// The `--fixture` folder's most tracked flights: a `TopFlight` list as `top_flights` sends it.
pub const FIXTURE_TOP: &str = "top_flights.json";
/// The `--fixture` folder's airlines and airports: `ReferenceData` as `reference_data` sends it
/// (the backend's `reference.json` cache has the same form).
pub const FIXTURE_REFERENCE: &str = "reference_data.json";

/// An answer on its way: a backend request's [`Pending`], or a channel's (canned answers).
pub enum Answer<T> {
    Request(Pending<T>),
    Channel(Receiver<T>),
}

impl<T> Answer<T> {
    /// An answer already there.
    pub fn ready(value: T) -> Self {
        Answer::Request(Pending::ready(value))
    }

    /// The answer, once, if it has arrived.
    pub fn poll(&self) -> Option<T> {
        match self {
            Answer::Request(pending) => pending.poll(),
            Answer::Channel(rx) => rx.try_recv().ok(),
        }
    }
}

impl<T> From<Pending<T>> for Answer<T> {
    fn from(pending: Pending<T>) -> Self {
        Answer::Request(pending)
    }
}

impl<T> From<Receiver<T>> for Answer<T> {
    fn from(rx: Receiver<T>) -> Self {
        Answer::Channel(rx)
    }
}

impl<T> std::fmt::Debug for Answer<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Answer::Request(_) => "Answer::Request",
            Answer::Channel(_) => "Answer::Channel",
        })
    }
}

/// The reference data as the app keeps it: the indexes, and the airports shared with the motion
/// model (`motion.setAirports(reference.airports)`). Built where it was loaded, off the main
/// thread.
#[derive(Debug, Clone)]
pub struct LoadedReference {
    pub reference: Arc<Reference>,
    pub airports: Vec<Arc<Airport>>,
}

impl LoadedReference {
    pub fn new(data: ReferenceData) -> Self {
        let airports = data.airports.iter().cloned().map(Arc::new).collect();
        LoadedReference { reference: Arc::new(Reference::new(data)), airports }
    }
}

/// Where the feed's answers come from: the backend, the `--fixture` folder, or a test's.
pub trait Feeds: Send {
    /// `liveFlights(area, serverCategories(filters))`: `None` is the whole world, or every
    /// category.
    fn live_flights(&mut self, area: Option<BoundingBox>, categories: Option<Vec<u8>>) -> Answer<Result<LiveSnapshot, String>>;
    /// `topFlights()`.
    fn top_flights(&mut self) -> Answer<Result<Vec<TopFlight>, String>>;
    /// `referenceData()`, indexed.
    fn reference_data(&mut self) -> Answer<Result<LoadedReference, String>>;
    /// `sessionInfo()`; `None` where nobody signs in (a fixture).
    fn session_info(&mut self) -> Option<SessionInfo>;
}

/// The backend's requests, on its runtime.
struct BackendFeeds {
    backend: Backend,
    requests: Requests,
}

impl Feeds for BackendFeeds {
    fn live_flights(&mut self, area: Option<BoundingBox>, categories: Option<Vec<u8>>) -> Answer<Result<LiveSnapshot, String>> {
        let backend = self.backend.clone();
        let services = categories.map(|c| c.into_iter().map(i32::from).collect());
        let area = area.map(convert::fr24_bounding_box);
        self.requests.request(async move { backend.live_flights(area, services).await }).into()
    }

    fn top_flights(&mut self) -> Answer<Result<Vec<TopFlight>, String>> {
        let backend = self.backend.clone();
        self.requests
            .request(async move { backend.top_flights().await.map(|top| top.into_iter().map(convert::top_flight).collect()) })
            .into()
    }

    fn reference_data(&mut self) -> Answer<Result<LoadedReference, String>> {
        let backend = self.backend.clone();
        self.requests
            .request(async move { backend.reference_data().await.map(|data| LoadedReference::new(ReferenceData::clone(&data))) })
            .into()
    }

    fn session_info(&mut self) -> Option<SessionInfo> {
        Some(self.backend.session_info())
    }
}

/// The `--fixture` folder's files, read when asked for (on the main thread: shots want the answer
/// in the frame that asked).
struct FixtureFeeds {
    dir: PathBuf,
}

/// The bytes of `name` in `dir`, or why not.
fn read_fixture(dir: &Path, name: &str) -> Result<Vec<u8>, String> {
    let path = dir.join(name);
    std::fs::read(&path).map_err(|e| format!("fixture {}: {e}", path.display()))
}

fn fixture_error(dir: &Path, name: &str, e: serde_json::Error) -> String {
    format!("fixture {}: {e}", dir.join(name).display())
}

impl Feeds for FixtureFeeds {
    fn live_flights(&mut self, _area: Option<BoundingBox>, _categories: Option<Vec<u8>>) -> Answer<Result<LiveSnapshot, String>> {
        let dir = &self.dir;
        Answer::ready(read_fixture(dir, FIXTURE_LIVE).and_then(|bytes| {
            serde_json::from_slice::<LiveSnapshot>(&bytes).map_err(|e| fixture_error(dir, FIXTURE_LIVE, e))
        }))
    }

    fn top_flights(&mut self) -> Answer<Result<Vec<TopFlight>, String>> {
        let dir = &self.dir;
        Answer::ready(read_fixture(dir, FIXTURE_TOP).and_then(|bytes| {
            serde_json::from_slice::<Vec<TopFlight>>(&bytes).map_err(|e| fixture_error(dir, FIXTURE_TOP, e))
        }))
    }

    fn reference_data(&mut self) -> Answer<Result<LoadedReference, String>> {
        let dir = &self.dir;
        Answer::ready(read_fixture(dir, FIXTURE_REFERENCE).and_then(|bytes| {
            serde_json::from_slice::<ReferenceData>(&bytes)
                .map(LoadedReference::new)
                .map_err(|e| fixture_error(dir, FIXTURE_REFERENCE, e))
        }))
    }

    fn session_info(&mut self) -> Option<SessionInfo> {
        None
    }
}

/// Which feeds the state uses.
enum Source {
    /// Chosen at the first poll: the fixture folder, else the backend, else none.
    Auto,
    /// None, and why (the page without Tauri).
    Off(&'static str),
    On(Box<dyn Feeds>),
}

/// The request out for the feed (`loading`), and the area it asked for.
#[derive(Debug)]
struct InFlight {
    answer: Answer<Result<LiveSnapshot, String>>,
    area: Option<Area>,
}

/// What the latest request asked for (`fetchedArea`): its area and the view it was made for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FetchedArea {
    pub area: Option<Area>,
    pub view: ViewState,
}

/// What the globe draws ([`Aircraft::shown`]): the filtered flights in the feed's order, with
/// the selected flight in its feed row's place, or after them when the filters leave it out.
#[derive(Debug, Clone, Copy)]
pub struct Shown<'a> {
    flights: &'a [LiveFlight],
    /// Indexes into `flights`, in order.
    filtered: &'a [usize],
    selected: Option<&'a LiveFlight>,
    /// Where `selected` stands in for a filtered flight; `None` with a selection: after them.
    at: Option<usize>,
}

impl<'a> Shown<'a> {
    pub fn len(&self) -> usize {
        self.filtered.len() + usize::from(self.selected.is_some() && self.at.is_none())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The `i`-th flight drawn.
    pub fn get(&self, i: usize) -> Option<&'a LiveFlight> {
        if self.at == Some(i) {
            return self.selected;
        }
        match self.filtered.get(i) {
            Some(&k) => Some(&self.flights[k]),
            None if i == self.filtered.len() && self.at.is_none() => self.selected,
            None => None,
        }
    }

    /// Every flight drawn, in order.
    pub fn iter(&self) -> impl Iterator<Item = &'a LiveFlight> + 'a {
        let shown = *self;
        (0..shown.len()).filter_map(move |i| shown.get(i))
    }

    /// The list as the page's `shown` array.
    pub fn to_vec(&self) -> Vec<LiveFlight> {
        self.iter().cloned().collect()
    }
}

/// The aircraft feed: refresh timing and back-off, the clock sync, reference data, top flights,
/// the list shown.
pub struct Aircraft {
    source: Source,
    started: bool,

    // the snapshot (`snapshot`), and what is derived from it
    snapshot: Option<LiveSnapshot>,
    /// `byId`: the index of each id's row (the last one with that id, as `new Map(...)`).
    by_id: HashMap<u32, usize>,
    /// Bumped with each snapshot set or dropped.
    snapshot_generation: u64,
    /// `filtered`: indexes into the snapshot's flights.
    filtered: Vec<usize>,
    /// The snapshot generation and the filters `filtered` was made for.
    filtered_for: Option<(u64, Filters)>,
    /// Bumped whenever `filtered` is made again.
    generation: u64,

    // the refresh rounds
    request: Option<InFlight>,
    feed_error: Option<String>,
    rate_limited: bool,
    /// `nextRefreshAt`: when the latest scheduled refresh is (or was) due; 0 when stopped.
    next_refresh_at: f64,
    /// `refreshTimer`: when the scheduled refresh fires, until it has.
    refresh_due: Option<f64>,
    shown_feed: Option<FeedClock>,
    regional: bool,
    backoff: i64,
    last_request_at: f64,
    refresh_again: bool,
    fetched_area: Option<FetchedArea>,
    /// `viewTimer`: when the view trigger's debounce ends.
    view_due: Option<f64>,
    /// The view the last poll saw (the view trigger's effect runs when it changes).
    last_view: Option<ViewState>,
    /// `aircraftWere`.
    aircraft_were: bool,
    /// `fetchedCategories`.
    fetched_categories: Vec<u8>,
    /// The focus the last poll saw (the top-flights effect).
    focus_was: Option<Mode>,

    // the UI's asks, for the next poll
    retry_asked: bool,
    soon_asked: bool,
    top_asked: bool,

    // the most tracked flights
    top: Option<Vec<TopFlight>>,
    top_failed: bool,
    top_requests: Vec<Answer<Result<Vec<TopFlight>, String>>>,
    /// The 60 s interval's next tick.
    top_tick_at: Option<f64>,

    // the reference data (`reference.svelte.ts`) and the sign-in state
    reference: Option<LoadedReference>,
    reference_error: Option<String>,
    reference_request: Option<Answer<Result<LoadedReference, String>>>,
    session: Option<SessionInfo>,
}

impl Default for Aircraft {
    /// The feed on the `--fixture` folder, else the backend, chosen at the first poll.
    fn default() -> Self {
        Aircraft::with_source(Source::Auto)
    }
}

impl std::fmt::Debug for Aircraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Aircraft")
            .field("live", &self.live())
            .field("flights", &self.flights().len())
            .field("filtered", &self.filtered.len())
            .field("loading", &self.loading())
            .field("feed_error", &self.feed_error)
            .field("backoff", &self.backoff)
            .field("next_refresh_at", &self.next_refresh_at)
            .field("shown_feed", &self.shown_feed)
            .finish_non_exhaustive()
    }
}

/// `scheduleRefresh`'s next interval tick: on the interval's own schedule, or a whole interval
/// from now after a long stall (no burst of missed ticks).
fn next_tick(due: f64, now: f64, every: f64) -> f64 {
    let next = due + every;
    if next <= now { now + every } else { next }
}

/// The area as `live_flights` takes it (the page's numbers become `f32` on the way to Rust).
fn bounding_box(area: Area) -> BoundingBox {
    BoundingBox { south: area.south as f32, north: area.north as f32, west: area.west as f32, east: area.east as f32 }
}

impl Aircraft {
    fn with_source(source: Source) -> Self {
        Aircraft {
            source,
            started: false,
            snapshot: None,
            by_id: HashMap::new(),
            snapshot_generation: 0,
            filtered: Vec::new(),
            filtered_for: None,
            generation: 0,
            request: None,
            feed_error: None,
            rate_limited: false,
            next_refresh_at: 0.0,
            refresh_due: None,
            shown_feed: None,
            regional: false,
            backoff: 1,
            last_request_at: 0.0,
            refresh_again: false,
            fetched_area: None,
            view_due: None,
            last_view: None,
            aircraft_were: false,
            fetched_categories: Vec::new(),
            focus_was: None,
            retry_asked: false,
            soon_asked: false,
            top_asked: false,
            top: None,
            top_failed: false,
            top_requests: Vec::new(),
            top_tick_at: None,
            reference: None,
            reference_error: None,
            reference_request: None,
            session: None,
        }
    }

    /// The feed on `feeds` instead of the fixture folder or the backend; `None`: no feed at all
    /// ([`NO_FEED`]; a shot without a fixture folder, which must not reach the network).
    pub fn with_feeds(feeds: Option<Box<dyn Feeds>>) -> Self {
        Aircraft::with_source(match feeds {
            Some(feeds) => Source::On(feeds),
            None => Source::Off(NO_FEED),
        })
    }

    /// This frame's step (see [`super::AppState::poll`] for the order, and the module's
    /// documentation for the steps).
    pub fn poll(&mut self, cx: &mut Cx<'_>) {
        let now = cx.clocks.local_ms;
        let view = cx.controller.view();
        let settings = &cx.persisted.settings;
        let on = shown_kinds(settings.mode, &settings.also_show).aircraft;
        let focus = settings.mode;

        if !self.started {
            self.start(cx, view, on, focus, now);
        } else {
            self.view_changed(view, now);
            self.aircraft_shown(on, now);
            // categories are filtered by the server: changing them needs a new snapshot
            if cx.persisted.filters.categories != self.fetched_categories {
                self.fetched_categories = cx.persisted.filters.categories.clone();
                self.soon(on, now);
            }
            // the most tracked list is the aircraft sidebar's: fetched while aircraft are the focus
            if self.focus_was != Some(focus) {
                self.focus_was = Some(focus);
                if focus == Mode::Aircraft && self.live() {
                    self.refresh_top();
                }
            }
        }

        if std::mem::take(&mut self.soon_asked) {
            self.soon(on, now);
        }
        if std::mem::take(&mut self.retry_asked) {
            self.refresh(cx, on, now);
        }
        if std::mem::take(&mut self.top_asked) {
            self.refresh_top();
        }

        if let Some(due) = self.view_due
            && now >= due
        {
            self.view_due = None;
            let wait = (self.last_request_at + MIN_GAP_MS - now).max(0.0);
            if now + wait < self.next_refresh_at {
                self.schedule_refresh(wait, now);
            }
        }
        if let Some(due) = self.refresh_due
            && now >= due
        {
            self.refresh_due = None;
            self.refresh(cx, on, now);
        }
        if let Some(due) = self.top_tick_at
            && now >= due
        {
            self.top_tick_at = Some(next_tick(due, now, TOP_REFRESH_MS));
            if focus == Mode::Aircraft {
                self.refresh_top();
            }
        }

        self.take_answers(cx, on, now);

        if self.refilter(&cx.persisted.filters) {
            cx.wants.scene = true;
        }
        for due in [self.view_due, self.refresh_due, self.top_tick_at].into_iter().flatten() {
            cx.wants.wake_at(due);
        }
    }

    /// The page's mount: its effects' first runs, then `onMount`.
    fn start(&mut self, cx: &mut Cx<'_>, view: ViewState, on: bool, focus: Mode, now: f64) {
        self.started = true;
        self.last_view = Some(view);
        self.aircraft_were = on;
        self.fetched_categories = cx.persisted.filters.categories.clone();
        self.focus_was = Some(focus);
        if matches!(self.source, Source::Auto) {
            self.source = if let Some(dir) = &cx.startup.fixture {
                log::info!("aircraft feed from the fixture folder {}", dir.display());
                Source::On(Box::new(FixtureFeeds { dir: dir.clone() }))
            } else if let (Some(backend), Some(requests)) = (cx.backend, cx.requests) {
                Source::On(Box::new(BackendFeeds { backend: backend.clone(), requests: requests.clone() }))
            } else {
                Source::Off(NO_BACKEND)
            };
        }
        let Source::On(feeds) = &mut self.source else {
            if let Source::Off(why) = self.source {
                self.feed_error = Some(why.to_owned());
            }
            return;
        };
        self.session = feeds.session_info();
        if focus == Mode::Aircraft {
            self.refresh_top();
        }
        self.load_reference();
        self.refresh(cx, on, now);
        self.top_tick_at = Some(now + TOP_REFRESH_MS);
    }

    /// The view trigger's effect (`+page:924-938`): after a large move, fetch the new area soon
    /// instead of waiting a full interval.
    fn view_changed(&mut self, view: ViewState, now: f64) {
        if self.last_view == Some(view) {
            return;
        }
        self.last_view = Some(view);
        let Some(fetched) = self.fetched_area else { return };
        if self.backoff > 1 {
            return;
        }
        let before = fetched.view;
        let moved =
            cos_angle(view.longitude, view.latitude, before.longitude, before.latitude) < (VIEW_MOVE_DEG * PI / 180.0).cos();
        let zoomed = (view.zoom - before.zoom).abs() > VIEW_ZOOM_CHANGE;
        if !moved && !zoomed {
            return;
        }
        self.view_due = Some(now + VIEW_SETTLE_MS);
    }

    /// The aircraft on or off the map (`+page:944-962`). Off: the feed stops and what it brought
    /// goes (what comes back on is a fresh feed, not aircraft extrapolated from minutes ago). On:
    /// it fetches at once, or as soon as the gap between requests allows.
    fn aircraft_shown(&mut self, on: bool, now: f64) {
        if on == self.aircraft_were {
            return;
        }
        self.aircraft_were = on;
        if !on {
            self.refresh_due = None;
            self.next_refresh_at = 0.0;
            self.set_snapshot(None);
            self.shown_feed = None;
        } else if self.live() && self.request.is_none() {
            self.schedule_refresh((self.last_request_at + MIN_GAP_MS - now).max(0.0), now);
        }
    }

    /// `scheduleRefresh(ms)`.
    fn schedule_refresh(&mut self, ms: f64, now: f64) {
        self.next_refresh_at = now + ms;
        self.refresh_due = Some(now + ms);
    }

    /// `refreshSoon()`: refresh soon, without crowding Flightradar24.
    fn soon(&mut self, on: bool, now: f64) {
        if !self.live() || self.backoff > 1 || !on {
            return;
        }
        if self.request.is_some() {
            self.refresh_again = true;
            return;
        }
        self.schedule_refresh((self.last_request_at + MIN_GAP_MS - now).max(SOON_MS), now);
    }

    /// `refresh()`'s start: with the aircraft off the map no live fetch at all, and none
    /// scheduled until they are back.
    fn refresh(&mut self, cx: &Cx<'_>, on: bool, now: f64) {
        if self.request.is_some() || !on {
            return;
        }
        self.refresh_again = false;
        let view = cx.controller.view();
        let [width, height] = cx.controller.size();
        let area = view_area(
            geo::View { longitude: view.longitude, latitude: view.latitude, zoom: view.zoom, pitch: view.pitch },
            width,
            height,
        );
        self.fetched_area = Some(FetchedArea { area, view });
        self.last_request_at = now;
        let categories = server_categories(&cx.persisted.filters);
        let answer = match &mut self.source {
            Source::On(feeds) => feeds.live_flights(area.map(bounding_box), categories),
            Source::Off(why) => Answer::ready(Err((*why).to_owned())),
            Source::Auto => Answer::ready(Err(NO_BACKEND.to_owned())),
        };
        self.request = Some(InFlight { answer, area });
    }

    /// `refresh()`'s end, from its `await` to its `finally`.
    fn settle(&mut self, answer: Result<LiveSnapshot, String>, area: Option<Area>, cx: &mut Cx<'_>, on: bool, now: f64) {
        let mut limited = false;
        let mut arrived = false;
        match answer {
            // the aircraft left the map while the request was out: drop what it brought
            Ok(_) if !on => {}
            Ok(next) => {
                // the launch sign-in finishes after the first ask: the feed tells which client it used
                if let Some(session) = self.session
                    && next.authenticated != session.authenticated
                    && let Source::On(feeds) = &mut self.source
                    && let Some(info) = feeds.session_info()
                {
                    self.session = Some(info);
                }
                // aircraft are drawn on the feed's clock, so a wrong system clock can't misplace them
                cx.motion.sync_clock(next.server_time_ms as f64, now);
                self.regional = area.is_some();
                arrived = true;
                self.feed_error = None;
                limited = next.errors.iter().any(|e| is_rate_limited(e));
                self.set_snapshot(Some(next));
            }
            Err(e) => {
                limited = is_rate_limited(&e);
                self.feed_error = Some(e);
            }
        }
        self.rate_limited = limited;
        self.backoff = if limited { (self.backoff * 2).min(MAX_BACKOFF) } else { 1 };
        let interval = if area.is_some() { AREA_REFRESH_MS } else { WORLD_REFRESH_MS } * self.backoff;
        // only new data moves the clock: a failure's longer backoff can't make old data look fresh
        self.shown_feed = after_round(self.shown_feed, Round { arrived, at: now as i64, interval });
        let next = if self.refresh_again && !limited { MIN_GAP_MS } else { interval as f64 };
        self.schedule_refresh(next, now);
    }

    /// `refreshTop()`.
    fn refresh_top(&mut self) {
        let answer = match &mut self.source {
            Source::On(feeds) => feeds.top_flights(),
            Source::Off(why) => Answer::ready(Err((*why).to_owned())),
            Source::Auto => Answer::ready(Err(NO_BACKEND.to_owned())),
        };
        self.top_requests.push(answer);
    }

    /// `reference.load()`: once, and again only after a failure.
    fn load_reference(&mut self) {
        if self.reference.is_some() || self.reference_request.is_some() {
            return;
        }
        if let Source::On(feeds) = &mut self.source {
            self.reference_request = Some(feeds.reference_data());
        }
    }

    fn take_answers(&mut self, cx: &mut Cx<'_>, on: bool, now: f64) {
        if let Some(answer) = self.request.as_ref().and_then(|r| r.answer.poll()) {
            let area = self.request.take().and_then(|r| r.area);
            self.settle(answer, area, cx, on, now);
        }
        let mut i = 0;
        while i < self.top_requests.len() {
            match self.top_requests[i].poll() {
                Some(answer) => {
                    self.top_requests.remove(i);
                    match answer {
                        Ok(top) => {
                            self.top = Some(top);
                            self.top_failed = false;
                        }
                        Err(_) => self.top_failed = true,
                    }
                }
                None => i += 1,
            }
        }
        if let Some(answer) = self.reference_request.as_ref().and_then(Answer::poll) {
            self.reference_request = None;
            match answer {
                Ok(loaded) => {
                    // field elevations, for landings, takeoffs and heights near airports
                    cx.motion.set_airports(&loaded.airports);
                    self.reference = Some(loaded);
                    cx.wants.scene = true;
                }
                Err(e) => {
                    log::warn!("reference data: {e}");
                    self.reference_error = Some(e);
                }
            }
        }
    }

    fn set_snapshot(&mut self, snapshot: Option<LiveSnapshot>) {
        self.by_id.clear();
        if let Some(s) = &snapshot {
            self.by_id.reserve(s.flights.len());
            for (i, f) in s.flights.iter().enumerate() {
                self.by_id.insert(f.id, i);
            }
        }
        self.snapshot = snapshot;
        self.snapshot_generation += 1;
    }

    /// `filtered` again if the snapshot or the filters changed since. Returns whether what is
    /// shown changed: another snapshot (set or dropped), or other flights kept.
    fn refilter(&mut self, filters: &Filters) -> bool {
        let snapshot_changed = match &self.filtered_for {
            Some((generation, f)) if *generation == self.snapshot_generation && f == filters => return false,
            Some((generation, _)) => *generation != self.snapshot_generation,
            // the first time: nothing was shown before, nor is yet unless a snapshot came
            None => self.snapshot_generation != 0,
        };
        let keep = matcher(filters);
        let flights = self.snapshot.as_ref().map_or(&[][..], |s| &s.flights[..]);
        let filtered: Vec<usize> = flights.iter().enumerate().filter(|(_, f)| keep(f)).map(|(i, _)| i).collect();
        self.filtered_for = Some((self.snapshot_generation, filters.clone()));
        let changed = snapshot_changed || filtered != self.filtered;
        self.filtered = filtered;
        if changed {
            self.generation += 1;
        }
        changed
    }

    // ------------------------------------------------------------------------- the UI's asks

    /// "Retry now" (`onclick={refresh}`): a refresh at the next poll, unless one is out.
    pub fn retry(&mut self) {
        self.retry_asked = true;
    }

    /// `refreshSoon()` at the next poll: after signing in or out (doc 06 §3.5). A no-op while
    /// backing off or with the aircraft off the map.
    pub fn refresh_soon(&mut self) {
        self.soon_asked = true;
    }

    /// The most tracked list's retry (`onretry={refreshTop}`), at the next poll.
    pub fn retry_top(&mut self) {
        self.top_asked = true;
    }

    /// The session after signing in or out (`session = await signIn(...)`); then
    /// [`Self::refresh_soon`].
    pub fn set_session(&mut self, session: SessionInfo) {
        self.session = Some(session);
    }

    // ------------------------------------------------------------------------- what it holds

    /// Whether there is a feed at all (the page's `isTauri()`).
    pub fn live(&self) -> bool {
        matches!(self.source, Source::On(_))
    }

    /// The latest snapshot (`snapshot`); `None` before the first and while aircraft are off.
    pub fn snapshot(&self) -> Option<&LiveSnapshot> {
        self.snapshot.as_ref()
    }

    /// Every flight of the snapshot (`flights`).
    pub fn flights(&self) -> &[LiveFlight] {
        self.snapshot.as_ref().map_or(&[], |s| &s.flights)
    }

    /// The snapshot's row for `id` (`byId.get(id)`).
    pub fn flight(&self, id: u32) -> Option<&LiveFlight> {
        let i = *self.by_id.get(&id)?;
        self.flights().get(i)
    }

    /// The flights the filters keep (`filtered`), in the feed's order, as of the last poll.
    pub fn filtered(&self) -> impl Iterator<Item = &LiveFlight> + '_ {
        let flights = self.flights();
        self.filtered.iter().map(move |&i| &flights[i])
    }

    /// How many flights the filters keep (`filtered.length`).
    pub fn filtered_len(&self) -> usize {
        self.filtered.len()
    }

    /// What the globe draws (`shown`): the filtered flights, with `selected` (the page's
    /// `selected` row, from `state::flight`) in its feed row's place, or after them when the
    /// filters leave it out. The substitution keeps the model and the trail's tip on the same
    /// row when the follow stream is fresher than the feed.
    pub fn shown<'a>(&'a self, selected: Option<&'a LiveFlight>) -> Shown<'a> {
        let flights = self.flights();
        let at = selected.and_then(|s| self.filtered.iter().position(|&i| flights[i].id == s.id));
        Shown { flights, filtered: &self.filtered, selected, at }
    }

    /// Bumped whenever [`Self::filtered`] (so [`Self::shown`]) is made again: a new snapshot, the
    /// aircraft off, other filters.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether a request is out (`loading`).
    pub fn loading(&self) -> bool {
        self.request.is_some()
    }

    /// The latest round's error (`feedError`), or why there is no feed.
    pub fn feed_error(&self) -> Option<&str> {
        self.feed_error.as_deref()
    }

    /// Whether Flightradar24 is limiting requests (`rateLimited`).
    pub fn rate_limited(&self) -> bool {
        self.rate_limited
    }

    /// The interval's multiplier: 1, or 2, 4, 6 while rate limited.
    pub fn backoff(&self) -> i64 {
        self.backoff
    }

    /// When the next refresh is due (`nextRefreshAt`, local Unix ms); 0 while stopped.
    pub fn next_refresh_at(&self) -> f64 {
        self.next_refresh_at
    }

    /// Seconds until the next refresh, for "Trying again in … s" (`retryIn`).
    pub fn retry_in(&self, now_ms: f64) -> i64 {
        ((self.next_refresh_at - now_ms) / 1000.0).ceil().max(0.0) as i64
    }

    /// When the snapshot on show arrived and the schedule it was fetched on (`shownFeed`).
    pub fn shown_feed(&self) -> Option<FeedClock> {
        self.shown_feed
    }

    /// The age of the snapshot on show, as the sidebar says it (`freshness`, shown while
    /// aircraft are the focus). `now_ms` is the 1 s clock tick's.
    pub fn freshness(&self, now_ms: f64) -> Option<Freshness> {
        self.shown_feed.map(|f| feed_freshness((now_ms as i64 - f.at).max(0), f.interval))
    }

    /// Whether the latest snapshot covers only the visible area (`regional`).
    pub fn regional(&self) -> bool {
        self.regional
    }

    /// How many regions failed in the latest snapshot (`partial`).
    pub fn partial(&self) -> usize {
        self.snapshot.as_ref().map_or(0, |s| s.errors.len())
    }

    /// What the latest request asked for (`fetchedArea`).
    pub fn fetched_area(&self) -> Option<FetchedArea> {
        self.fetched_area
    }

    /// The most tracked flights (`top`), once loaded.
    pub fn top(&self) -> Option<&[TopFlight]> {
        self.top.as_deref()
    }

    /// Whether the latest ask for the most tracked flights failed (`topFailed`).
    pub fn top_failed(&self) -> bool {
        self.top_failed
    }

    /// The airlines and airports, once loaded (`reference.ready`).
    pub fn reference(&self) -> Option<&Arc<Reference>> {
        self.reference.as_ref().map(|r| &r.reference)
    }

    /// The airports as the motion model holds them; empty until loaded.
    pub fn reference_airports(&self) -> &[Arc<Airport>] {
        self.reference.as_ref().map_or(&[], |r| &r.airports)
    }

    /// Why the reference data did not load (`reference.error`).
    pub fn reference_error(&self) -> Option<&str> {
        self.reference_error.as_deref()
    }

    /// The Flightradar24 sign-in state (`session`), once read.
    pub fn session(&self) -> Option<SessionInfo> {
        self.session
    }

    /// Whether the feed is signed in (`authenticated`: the session's word, else the snapshot's).
    pub fn authenticated(&self) -> bool {
        self.session.map(|s| s.authenticated).or(self.snapshot.as_ref().map(|s| s.authenticated)).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::mpsc::{self, Sender};

    use rustradar_core::motion::aircraft::Motion;
    use rustradar_core::settings::Persisted;
    use rustradar_scene::controller::Controller;

    use super::*;
    use crate::state::{Clocks, Startup, Wants};

    const T0: f64 = 1.7e12;

    type Live = Result<LiveSnapshot, String>;
    /// A feed request: its area, its categories, and where its answer goes.
    type LiveCall = (Option<BoundingBox>, Option<Vec<u8>>, Sender<Live>);

    /// What the feed asked for, with the senders the test answers on.
    #[derive(Default)]
    struct Calls {
        live: Vec<LiveCall>,
        top: Vec<Sender<Result<Vec<TopFlight>, String>>>,
        reference: Vec<Sender<Result<LoadedReference, String>>>,
        session: Option<SessionInfo>,
        sessions_read: usize,
    }

    /// Canned feeds: every request waits until the test answers it.
    #[derive(Clone, Default)]
    struct Canned(Arc<Mutex<Calls>>);

    impl Feeds for Canned {
        fn live_flights(&mut self, area: Option<BoundingBox>, categories: Option<Vec<u8>>) -> Answer<Live> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().live.push((area, categories, tx));
            rx.into()
        }

        fn top_flights(&mut self) -> Answer<Result<Vec<TopFlight>, String>> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().top.push(tx);
            rx.into()
        }

        fn reference_data(&mut self) -> Answer<Result<LoadedReference, String>> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().reference.push(tx);
            rx.into()
        }

        fn session_info(&mut self) -> Option<SessionInfo> {
            let mut calls = self.0.lock().unwrap();
            calls.sessions_read += 1;
            calls.session
        }
    }

    impl Canned {
        fn feed(&self) -> Aircraft {
            Aircraft::with_feeds(Some(Box::new(self.clone())))
        }

        fn lives(&self) -> usize {
            self.0.lock().unwrap().live.len()
        }

        fn tops(&self) -> usize {
            self.0.lock().unwrap().top.len()
        }

        /// The latest feed request's area and categories.
        fn asked(&self) -> (Option<BoundingBox>, Option<Vec<u8>>) {
            let calls = self.0.lock().unwrap();
            let (area, categories, _) = calls.live.last().expect("a request");
            (*area, categories.clone())
        }

        /// Answers the latest feed request.
        fn answer(&self, answer: Live) {
            self.0.lock().unwrap().live.last().expect("a request").2.send(answer).unwrap();
        }

        fn answer_top(&self, i: usize, answer: Result<Vec<TopFlight>, String>) {
            self.0.lock().unwrap().top[i].send(answer).unwrap();
        }
    }

    /// What a poll reads besides the feed: the camera, the motion model, settings, the startup.
    struct World {
        controller: Controller,
        motion: Motion,
        persisted: Persisted,
        startup: Startup,
    }

    fn view(latitude: f64, longitude: f64, zoom: f64) -> ViewState {
        ViewState { longitude, latitude, zoom, bearing: 0.0, pitch: 0.0, position: None }
    }

    /// Stockholm (V3): a regional view.
    const V3: (f64, f64, f64) = (59.33, 18.07, 9.0);

    impl World {
        fn at((lat, lon, zoom): (f64, f64, f64)) -> Self {
            World {
                controller: Controller::new(view(lat, lon, zoom), [1440.0, 900.0]),
                motion: Motion::new(),
                persisted: Persisted::default(),
                startup: Startup::default(),
            }
        }

        fn poll(&mut self, aircraft: &mut Aircraft, now: f64) -> Wants {
            let mut wants = Wants::default();
            let clocks = Clocks { local_ms: now, feed_ms: self.motion.now(now) };
            let mut cx = Cx {
                clocks,
                controller: &mut self.controller,
                motion: &mut self.motion,
                persisted: &mut self.persisted,
                backend: None,
                requests: None,
                startup: &self.startup,
                events: &[],
                wants: &mut wants,
            };
            aircraft.poll(&mut cx);
            wants
        }

        fn look_at(&mut self, (lat, lon, zoom): (f64, f64, f64)) {
            self.controller.set_view(view(lat, lon, zoom));
        }
    }

    fn flight(id: u32, alt: i32, on_ground: bool) -> LiveFlight {
        LiveFlight { id, alt, on_ground, callsign: format!("SAS{id}"), lat: 59.6, lon: 17.9, ..Default::default() }
    }

    fn snapshot(server_time_ms: f64, errors: &[&str]) -> LiveSnapshot {
        LiveSnapshot {
            flights: vec![flight(1, 30_000, false), flight(2, 0, true), flight(3, 12_000, false)],
            server_time_ms: server_time_ms as u64,
            requests: 3,
            errors: errors.iter().map(|e| (*e).to_owned()).collect(),
            elapsed_ms: 120,
            authenticated: false,
        }
    }

    const LIMITED: &str = "0..30: status 8: too many requests";

    #[test]
    fn mount_asks_for_top_flights_reference_session_and_a_regional_snapshot() {
        let canned = Canned::default();
        let mut feed = canned.feed();
        let mut world = World::at(V3);
        let wants = world.poll(&mut feed, T0);
        assert!(feed.live() && feed.loading());
        assert_eq!(canned.lives(), 1);
        assert_eq!(canned.tops(), 1, "aircraft are the focus");
        let calls = canned.0.lock().unwrap();
        assert_eq!(calls.reference.len(), 1);
        assert_eq!(calls.sessions_read, 1);
        drop(calls);
        // the area of `viewArea` for the view and the window, every category
        let (area, categories) = canned.asked();
        let expected = view_area(geo::View { longitude: 18.07, latitude: 59.33, zoom: 9.0, pitch: 0.0 }, 1440.0, 900.0)
            .expect("a regional view");
        assert_eq!(area, Some(bounding_box(expected)));
        assert_eq!(categories, None);
        assert_eq!(feed.fetched_area().map(|f| f.area), Some(Some(expected)));
        // the top flights' interval is the only timer while the request is out
        assert_eq!(wants.wake_at, Some(T0 + TOP_REFRESH_MS));
        // nothing more is asked while it is out
        world.poll(&mut feed, T0 + 100.0);
        assert_eq!(canned.lives(), 1);
    }

    #[test]
    fn a_regional_view_refreshes_every_8_s_and_syncs_the_feed_clock() {
        let canned = Canned::default();
        let mut feed = canned.feed();
        let mut world = World::at(V3);
        world.poll(&mut feed, T0);
        canned.answer(Ok(snapshot(T0 + 2500.0, &[])));
        let wants = world.poll(&mut feed, T0 + 300.0);
        assert!(!feed.loading());
        assert!(wants.scene, "a new snapshot rebuilds the scene");
        assert_eq!(feed.flights().len(), 3);
        assert!(feed.regional());
        assert_eq!(feed.feed_error(), None);
        assert_eq!(feed.next_refresh_at(), T0 + 300.0 + 8000.0);
        assert_eq!(wants.wake_at, Some(T0 + 8300.0));
        // aircraft are drawn on the server's clock
        assert_eq!(world.motion.now(T0 + 300.0), T0 + 2500.0);
        assert_eq!(feed.shown_feed(), Some(FeedClock { at: (T0 + 300.0) as i64, interval: 8000 }));
        world.poll(&mut feed, T0 + 8299.0);
        assert_eq!(canned.lives(), 1, "not before the interval");
        world.poll(&mut feed, T0 + 8300.0);
        assert_eq!(canned.lives(), 2);
        assert!(feed.loading());
    }

    #[test]
    fn the_world_view_refreshes_every_15_s() {
        let canned = Canned::default();
        let mut feed = canned.feed();
        let mut world = World::at((35.0, 10.0, 1.9));
        world.poll(&mut feed, T0);
        assert_eq!(canned.asked().0, None, "most of the globe in sight: the whole world");
        canned.answer(Ok(snapshot(T0, &[])));
        world.poll(&mut feed, T0 + 1000.0);
        assert!(!feed.regional());
        assert_eq!(feed.next_refresh_at(), T0 + 1000.0 + 15_000.0);
        assert_eq!(feed.shown_feed().map(|f| f.interval), Some(15_000));
    }

    #[test]
    fn rate_limiting_backs_off_1_2_4_6_and_a_clean_round_resets_it() {
        let canned = Canned::default();
        let mut feed = canned.feed();
        let mut world = World::at(V3);
        let mut now = T0;
        world.poll(&mut feed, now);
        assert_eq!(feed.backoff(), 1);
        let mut seen = vec![feed.backoff()];
        // a failed request that says so, then snapshots whose regions say so
        let rounds: [Live; 4] = [
            Err("Flightradar24: grpc status 8 (resource exhausted)".into()),
            Ok(snapshot(T0, &[LIMITED])),
            Err("Too Many Requests".into()),
            Ok(snapshot(T0, &[LIMITED])),
        ];
        for round in rounds {
            canned.answer(round);
            now += 500.0;
            world.poll(&mut feed, now);
            assert!(feed.rate_limited());
            assert_eq!(feed.next_refresh_at(), now + (AREA_REFRESH_MS * feed.backoff()) as f64);
            seen.push(feed.backoff());
            // the next request comes when the stretched interval has passed
            now = feed.next_refresh_at();
            world.poll(&mut feed, now);
            assert!(feed.loading());
        }
        assert_eq!(seen, [1, 2, 4, 6, 6]);
        canned.answer(Ok(snapshot(T0, &["30..60: deadline exceeded"])));
        now += 500.0;
        world.poll(&mut feed, now);
        assert!(!feed.rate_limited());
        assert_eq!(feed.backoff(), 1);
        assert_eq!(feed.partial(), 1);
        assert_eq!(feed.next_refresh_at(), now + 8000.0);
        // a failure that is not a rate limit keeps the plain interval
        now = feed.next_refresh_at();
        world.poll(&mut feed, now);
        canned.answer(Err("connection refused".into()));
        now += 500.0;
        world.poll(&mut feed, now);
        assert_eq!(feed.feed_error(), Some("connection refused"));
        assert!(!feed.rate_limited());
        assert_eq!(feed.backoff(), 1);
        assert_eq!(feed.next_refresh_at(), now + 8000.0);
        assert_eq!(feed.retry_in(now + 2100.0), 6);
        assert_eq!(feed.flights().len(), 3, "a failed round keeps the snapshot on show");
    }

    #[test]
    fn only_a_successful_round_moves_the_freshness_clock() {
        let canned = Canned::default();
        let mut feed = canned.feed();
        let mut world = World::at(V3);
        world.poll(&mut feed, T0);
        assert_eq!(feed.shown_feed(), None);
        canned.answer(Ok(snapshot(T0, &[])));
        let t1 = T0 + 400.0;
        world.poll(&mut feed, t1);
        let first = FeedClock { at: t1 as i64, interval: 8000 };
        assert_eq!(feed.shown_feed(), Some(first));
        // two rate-limited failures: the interval stretches, the clock stays
        let mut now = feed.next_refresh_at();
        for _ in 0..2 {
            world.poll(&mut feed, now);
            canned.answer(Err("status 8".into()));
            now += 300.0;
            world.poll(&mut feed, now);
            assert_eq!(feed.shown_feed(), Some(first));
            now = feed.next_refresh_at();
        }
        assert_eq!(feed.backoff(), 4);
        // so the data goes stale on its own schedule, not the stretched one
        let fresh = feed.freshness(t1 + 29_000.0).expect("a freshness");
        assert!(!fresh.stale, "{fresh:?}");
        let old = feed.freshness(t1 + 31_000.0).expect("a freshness");
        assert!(old.stale && old.label == "Data 31 s old", "{old:?}");
        // a snapshot that arrives, though limited, restarts it on the stretched schedule
        world.poll(&mut feed, now);
        canned.answer(Ok(snapshot(T0, &[LIMITED])));
        world.poll(&mut feed, now + 300.0);
        assert_eq!(feed.shown_feed(), Some(FeedClock { at: (now + 300.0) as i64, interval: 8000 * 6 }));
        assert_eq!(feed.freshness(now + 300.0 + 5_000.0).map(|f| f.label), Some("Updated 5 s ago".to_owned()));
    }

    #[test]
    fn refresh_soon_keeps_the_minimum_gap() {
        let canned = Canned::default();
        let mut feed = canned.feed();
        let mut world = World::at(V3);
        world.poll(&mut feed, T0);
        // asked while the request is out: the next comes the minimum gap after it ends
        feed.refresh_soon();
        world.poll(&mut feed, T0 + 100.0);
        assert_eq!(canned.lives(), 1);
        canned.answer(Ok(snapshot(T0, &[])));
        world.poll(&mut feed, T0 + 300.0);
        assert_eq!(feed.next_refresh_at(), T0 + 300.0 + MIN_GAP_MS);
        world.poll(&mut feed, T0 + 4300.0);
        assert_eq!(canned.lives(), 2);
        canned.answer(Ok(snapshot(T0, &[])));
        world.poll(&mut feed, T0 + 4500.0);
        assert_eq!(feed.next_refresh_at(), T0 + 4500.0 + 8000.0, "asked once, served once");
        // asked between rounds: as soon as the gap since the last request allows...
        feed.refresh_soon();
        world.poll(&mut feed, T0 + 5000.0);
        assert_eq!(feed.next_refresh_at(), T0 + 4300.0 + MIN_GAP_MS);
        // ... and never sooner than 300 ms
        world.poll(&mut feed, T0 + 8300.0);
        assert_eq!(canned.lives(), 3);
        canned.answer(Ok(snapshot(T0, &[])));
        world.poll(&mut feed, T0 + 12_100.0);
        feed.refresh_soon();
        world.poll(&mut feed, T0 + 12_200.0);
        assert_eq!(feed.next_refresh_at(), T0 + 12_200.0 + SOON_MS);
        // asked while a request is out that comes back limited: the stretched interval wins
        // over the gap
        world.poll(&mut feed, T0 + 12_500.0);
        assert_eq!(canned.lives(), 4);
        feed.refresh_soon();
        world.poll(&mut feed, T0 + 12_600.0);
        canned.answer(Err("status 8".into()));
        world.poll(&mut feed, T0 + 13_000.0);
        let next = feed.next_refresh_at();
        assert_eq!(next, T0 + 13_000.0 + 16_000.0);
        // while backing off it does nothing
        feed.refresh_soon();
        world.poll(&mut feed, T0 + 14_000.0);
        assert_eq!(feed.next_refresh_at(), next);
        world.poll(&mut feed, next - 1.0);
        assert_eq!(canned.lives(), 4);
        world.poll(&mut feed, next);
        assert_eq!(canned.lives(), 5);
        assert_eq!(feed.backoff(), 2);
        feed.retry();
        world.poll(&mut feed, next + 10.0);
        assert_eq!(canned.lives(), 5, "a retry while a request is out does nothing");
    }

    #[test]
    fn a_large_move_refreshes_800_ms_after_the_view_settles() {
        let canned = Canned::default();
        let mut feed = canned.feed();
        let mut world = World::at(V3);
        world.poll(&mut feed, T0);
        canned.answer(Ok(snapshot(T0, &[])));
        world.poll(&mut feed, T0 + 300.0);
        let next = T0 + 8300.0;
        assert_eq!(feed.next_refresh_at(), next);
        // 2° of arc and a zoom change of 1: too small
        world.look_at((V3.0, V3.1 + 2.0 / V3.0.to_radians().cos(), V3.2 + 1.0));
        let wants = world.poll(&mut feed, T0 + 500.0);
        assert_eq!(wants.wake_at, Some(next), "no debounce for a small move");
        // 5° east: the debounce starts, and starts again while the view keeps moving
        world.look_at((V3.0, V3.1 + 10.0, V3.2));
        let wants = world.poll(&mut feed, T0 + 1000.0);
        assert_eq!(wants.wake_at, Some(T0 + 1800.0));
        world.look_at((V3.0, V3.1 + 11.0, V3.2));
        world.poll(&mut feed, T0 + 1500.0);
        world.poll(&mut feed, T0 + 1800.0);
        assert_eq!(feed.next_refresh_at(), next, "the view moved 300 ms ago");
        // settled: the refresh comes as soon as the gap since the last request allows
        world.poll(&mut feed, T0 + 2300.0);
        assert_eq!(feed.next_refresh_at(), T0 + MIN_GAP_MS);
        world.poll(&mut feed, T0 + 4000.0);
        assert_eq!(canned.lives(), 2);
        let area = canned.asked().0.expect("regional");
        assert!(f64::from(area.west) < V3.1 + 11.0 && f64::from(area.east) > V3.1 + 11.0, "the new view's area: {area:?}");
        canned.answer(Ok(snapshot(T0, &[])));
        world.poll(&mut feed, T0 + 4200.0);
        // a zoom of 2 levels counts as a large move
        world.look_at((V3.0, V3.1 + 11.0, V3.2 + 2.0));
        world.poll(&mut feed, T0 + 5000.0);
        world.poll(&mut feed, T0 + 5800.0);
        assert_eq!(feed.next_refresh_at(), T0 + 8000.0, "the gap after the request at 4 s");
        world.poll(&mut feed, T0 + 8000.0);
        assert_eq!(canned.lives(), 3);
        canned.answer(Ok(snapshot(T0, &[])));
        world.poll(&mut feed, T0 + 8200.0);
        assert_eq!(feed.next_refresh_at(), T0 + 16_200.0);
        // a debounce that ends while the refresh is out brings nothing forward
        world.look_at((V3.0, V3.1, V3.2 + 2.0));
        world.poll(&mut feed, T0 + 15_500.0);
        let wants = world.poll(&mut feed, T0 + 16_200.0);
        assert_eq!(canned.lives(), 4);
        assert_eq!(wants.wake_at, Some(T0 + 16_300.0), "the debounce is still to end");
        let wants = world.poll(&mut feed, T0 + 16_300.0);
        assert_eq!(wants.wake_at, Some(T0 + TOP_REFRESH_MS), "it ended and scheduled nothing");
        canned.answer(Err("status 8".into()));
        world.poll(&mut feed, T0 + 16_400.0);
        assert_eq!(canned.lives(), 4);
        let next = feed.next_refresh_at();
        assert_eq!(next, T0 + 16_400.0 + 16_000.0, "the round's own schedule");
        // backing off: a move asks for nothing
        world.look_at((V3.0 - 10.0, V3.1, V3.2));
        let wants = world.poll(&mut feed, T0 + 17_000.0);
        assert_eq!(wants.wake_at, Some(next));
        world.poll(&mut feed, T0 + 18_000.0);
        assert_eq!(feed.next_refresh_at(), next);
    }

    #[test]
    fn aircraft_off_the_map_stop_the_feed_and_drop_its_snapshot() {
        let canned = Canned::default();
        let mut feed = canned.feed();
        let mut world = World::at(V3);
        world.poll(&mut feed, T0);
        canned.answer(Ok(snapshot(T0, &[])));
        world.poll(&mut feed, T0 + 300.0);
        let generation = feed.generation();
        world.persisted.settings.mode = Mode::Ships;
        let wants = world.poll(&mut feed, T0 + 1000.0);
        assert!(wants.scene);
        assert!(feed.generation() > generation);
        assert!(feed.snapshot().is_none() && feed.flights().is_empty() && feed.shown(None).is_empty());
        assert_eq!(feed.shown_feed(), None);
        assert_eq!(feed.next_refresh_at(), 0.0);
        world.poll(&mut feed, T0 + 60_000.0);
        assert_eq!(canned.lives(), 1, "nothing asked while off");
        // a request the retry button starts while off is not made
        feed.retry();
        world.poll(&mut feed, T0 + 61_000.0);
        assert_eq!(canned.lives(), 1);
        // shown again with the focus elsewhere: fetched at once
        world.persisted.settings.also_show = vec![Mode::Aircraft];
        world.poll(&mut feed, T0 + 62_000.0);
        assert_eq!(canned.lives(), 2);
        // and only as soon as the gap allows, if it was just asked
        canned.answer(Ok(snapshot(T0, &[])));
        world.poll(&mut feed, T0 + 62_100.0);
        world.persisted.settings.also_show.clear();
        world.poll(&mut feed, T0 + 62_200.0);
        world.persisted.settings.also_show = vec![Mode::Aircraft];
        world.poll(&mut feed, T0 + 63_000.0);
        assert_eq!(feed.next_refresh_at(), T0 + 66_000.0);
    }

    #[test]
    fn a_snapshot_arriving_after_the_aircraft_went_off_is_dropped() {
        let canned = Canned::default();
        let mut feed = canned.feed();
        let mut world = World::at(V3);
        world.poll(&mut feed, T0);
        world.persisted.settings.mode = Mode::Trains;
        world.poll(&mut feed, T0 + 100.0);
        canned.answer(Ok(snapshot(T0 + 9_999.0, &[])));
        world.poll(&mut feed, T0 + 200.0);
        assert!(feed.snapshot().is_none());
        assert_eq!(feed.shown_feed(), None);
        assert_eq!(world.motion.now(T0 + 200.0), T0 + 200.0, "its clock is not taken either");
        // the page still schedules the next round, which then finds the aircraft off
        assert_eq!(feed.next_refresh_at(), T0 + 8200.0);
        world.poll(&mut feed, T0 + 8200.0);
        assert_eq!(canned.lives(), 1);
    }

    #[test]
    fn a_category_change_refreshes_soon_with_the_new_categories() {
        let canned = Canned::default();
        let mut feed = canned.feed();
        let mut world = World::at(V3);
        world.poll(&mut feed, T0);
        canned.answer(Ok(snapshot(T0, &[])));
        world.poll(&mut feed, T0 + 300.0);
        world.persisted.filters.categories = vec![0, 1];
        world.poll(&mut feed, T0 + 1000.0);
        assert_eq!(feed.next_refresh_at(), T0 + MIN_GAP_MS);
        world.poll(&mut feed, T0 + MIN_GAP_MS);
        assert_eq!(canned.lives(), 2);
        assert_eq!(canned.asked().1, Some(vec![0, 1]));
        // the other filters apply here, with no new request
        canned.answer(Ok(snapshot(T0, &[])));
        world.poll(&mut feed, T0 + 4200.0);
        world.persisted.filters.on_ground = false;
        let wants = world.poll(&mut feed, T0 + 5000.0);
        assert!(wants.scene);
        assert_eq!(canned.lives(), 2);
        assert_eq!(feed.filtered().map(|f| f.id).collect::<Vec<_>>(), [1, 3]);
        // nothing changed, or other filters that keep the same flights: no scene refresh asked
        assert!(!world.poll(&mut feed, T0 + 5100.0).scene);
        let generation = feed.generation();
        world.persisted.filters.altitude = (1000, 40_000);
        assert!(!world.poll(&mut feed, T0 + 5200.0).scene);
        assert_eq!(feed.generation(), generation);
    }

    #[test]
    fn top_flights_on_focus_and_every_minute_while_aircraft_are_the_focus() {
        let canned = Canned::default();
        let mut feed = canned.feed();
        let mut world = World::at(V3);
        world.poll(&mut feed, T0);
        assert_eq!(canned.tops(), 1);
        canned.answer_top(0, Err("offline".into()));
        world.poll(&mut feed, T0 + 100.0);
        assert!(feed.top_failed() && feed.top().is_none());
        world.poll(&mut feed, T0 + 60_000.0);
        assert_eq!(canned.tops(), 2);
        let top = TopFlight { flight_id: 7, callsign: "SAS7".into(), ..Default::default() };
        canned.answer_top(1, Ok(vec![top.clone()]));
        world.poll(&mut feed, T0 + 60_100.0);
        assert!(!feed.top_failed());
        assert_eq!(feed.top(), Some(&[top][..]));
        // another focus: no tick asks
        world.persisted.settings.mode = Mode::Ships;
        world.poll(&mut feed, T0 + 70_000.0);
        world.poll(&mut feed, T0 + 120_000.0);
        assert_eq!(canned.tops(), 2);
        // back to aircraft: at once, then on the interval's own schedule
        world.persisted.settings.mode = Mode::Aircraft;
        world.poll(&mut feed, T0 + 130_000.0);
        assert_eq!(canned.tops(), 3);
        let wants = world.poll(&mut feed, T0 + 131_000.0);
        assert_eq!(wants.wake_at.map(|t| t.min(T0 + 180_000.0)), Some(T0 + 180_000.0));
        world.poll(&mut feed, T0 + 180_000.0);
        assert_eq!(canned.tops(), 4);
        feed.retry_top();
        world.poll(&mut feed, T0 + 180_500.0);
        assert_eq!(canned.tops(), 5);
    }

    #[test]
    fn shown_is_the_filtered_list_with_the_selected_flight_kept() {
        let canned = Canned::default();
        let mut feed = canned.feed();
        let mut world = World::at(V3);
        world.persisted.filters.on_ground = false;
        world.poll(&mut feed, T0);
        canned.answer(Ok(snapshot(T0, &[])));
        world.poll(&mut feed, T0 + 300.0);
        assert_eq!(feed.filtered_len(), 2);
        let ids = |shown: Shown<'_>| shown.iter().map(|f| f.id).collect::<Vec<_>>();
        assert_eq!(ids(feed.shown(None)), [1, 3]);
        // the selection substituted in its row's place: the follow stream's fresher row
        let fresher = LiveFlight { alt: 31_000, ..flight(3, 0, false) };
        let shown = feed.shown(Some(&fresher));
        assert_eq!(ids(shown), [1, 3]);
        assert_eq!(shown.get(1).map(|f| f.alt), Some(31_000));
        assert_eq!(shown.to_vec()[1], fresher);
        // filtered out (on the ground): added after the others
        let ground = feed.flight(2).expect("in the feed").clone();
        let shown = feed.shown(Some(&ground));
        assert_eq!(shown.len(), 3);
        assert_eq!(ids(shown), [1, 3, 2]);
        assert_eq!(shown.get(3), None);
        // not in the feed at all (a search hint): added too
        let hint = flight(99, 0, false);
        assert_eq!(ids(feed.shown(Some(&hint))), [1, 3, 99]);
        assert!(feed.flight(99).is_none());
    }

    #[test]
    fn the_session_is_read_again_when_the_feed_signs_in_otherwise() {
        let canned = Canned::default();
        canned.0.lock().unwrap().session = Some(SessionInfo { authenticated: false, expires: None });
        let mut feed = canned.feed();
        let mut world = World::at(V3);
        world.poll(&mut feed, T0);
        assert!(!feed.authenticated());
        // the launch sign-in finished meanwhile: the feed used the signed-in client
        canned.0.lock().unwrap().session = Some(SessionInfo { authenticated: true, expires: Some(1) });
        canned.answer(Ok(LiveSnapshot { authenticated: true, ..snapshot(T0, &[]) }));
        world.poll(&mut feed, T0 + 300.0);
        assert_eq!(canned.0.lock().unwrap().sessions_read, 2);
        assert_eq!(feed.session(), Some(SessionInfo { authenticated: true, expires: Some(1) }));
        assert!(feed.authenticated());
        // the same word again: not read
        world.poll(&mut feed, T0 + 8300.0);
        canned.answer(Ok(LiveSnapshot { authenticated: true, ..snapshot(T0, &[]) }));
        world.poll(&mut feed, T0 + 8400.0);
        assert_eq!(canned.0.lock().unwrap().sessions_read, 2);
        feed.set_session(SessionInfo { authenticated: false, expires: None });
        assert!(!feed.authenticated(), "the session's word over the snapshot's");
    }

    #[test]
    fn the_reference_data_loads_once_into_the_motion_model() {
        let canned = Canned::default();
        let mut feed = canned.feed();
        let mut world = World::at(V3);
        world.poll(&mut feed, T0);
        let arlanda = Airport {
            id: 1,
            name: "Stockholm Arlanda".into(),
            iata: "ARN".into(),
            icao: "ESSA".into(),
            lat: 59.6519,
            lon: 17.9186,
            alt: Some(137),
            ..Default::default()
        };
        let data = ReferenceData { fetched_at: 0, airlines: Vec::new(), airports: vec![arlanda] };
        canned.0.lock().unwrap().reference[0].send(Ok(LoadedReference::new(data))).unwrap();
        let wants = world.poll(&mut feed, T0 + 100.0);
        assert!(wants.scene);
        let reference = feed.reference().expect("loaded");
        assert_eq!(reference.airport(Some("arn")).map(|a| a.icao.as_str()), Some("ESSA"));
        assert_eq!(feed.reference_airports().len(), 1);
        // the motion model has the field: an aircraft on the ground at Arlanda stands on it
        let on_ground = LiveFlight { on_ground: true, lat: 59.6519, lon: 17.9186, timestamp_ms: T0 as u64, ..flight(5, 0, true) };
        let pose = world.motion.pose(&on_ground, T0);
        assert!(world.motion.ground_field(5).is_some_and(|ap| ap.icao == "ESSA"), "{pose:?}");
        world.poll(&mut feed, T0 + 60_000.0);
        assert_eq!(canned.0.lock().unwrap().reference.len(), 1, "loaded once");
    }

    /// A folder of its own under the system's temp folder, removed at the end.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("rustradar-a1-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp folder");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_fixture_folder_feeds_the_state_in_the_frame_that_asks() {
        let dir = TempDir::new("fixture");
        let wire = serde_json::to_vec(&snapshot(T0 + 1000.0, &[])).unwrap();
        std::fs::write(dir.0.join(FIXTURE_LIVE), wire).unwrap();
        let top = vec![TopFlight { flight_id: 9, ..Default::default() }];
        std::fs::write(dir.0.join(FIXTURE_TOP), serde_json::to_vec(&top).unwrap()).unwrap();
        let mut feed = Aircraft::default();
        let mut world = World::at(V3);
        world.startup.fixture = Some(dir.0.clone());
        let wants = world.poll(&mut feed, T0);
        assert!(feed.live());
        assert!(wants.scene);
        assert_eq!(feed.flights().len(), 3, "answered at once");
        assert_eq!(world.motion.now(T0), T0 + 1000.0);
        assert_eq!(feed.top(), Some(&top[..]));
        assert!(feed.reference_error().is_some_and(|e| e.contains(FIXTURE_REFERENCE)), "{:?}", feed.reference_error());
        assert_eq!(feed.session(), None);
        assert_eq!(feed.next_refresh_at(), T0 + 8000.0);
        // read again at the next round: a missing file fails as a request does
        std::fs::remove_file(dir.0.join(FIXTURE_LIVE)).unwrap();
        world.poll(&mut feed, T0 + 8000.0);
        assert!(feed.feed_error().is_some_and(|e| e.contains(FIXTURE_LIVE)), "{:?}", feed.feed_error());
        assert_eq!(feed.flights().len(), 3);
        assert_eq!(feed.next_refresh_at(), T0 + 16_000.0);
    }

    #[test]
    fn without_a_backend_or_a_fixture_there_is_no_feed() {
        let mut feed = Aircraft::default();
        let mut world = World::at(V3);
        let wants = world.poll(&mut feed, T0);
        assert!(!feed.live());
        assert_eq!(feed.feed_error(), Some(NO_BACKEND));
        assert_eq!(wants.wake_at, None, "no timer runs");
        assert!(!wants.scene, "nothing to show, nothing to rebuild");
        assert!(!feed.loading() && feed.top().is_none());
        world.poll(&mut feed, T0 + 100_000.0);
        assert!(!feed.loading());
        // the retry fails as the page's request without Tauri did, and keeps trying
        feed.retry();
        world.poll(&mut feed, T0 + 100_100.0);
        assert_eq!(feed.feed_error(), Some(NO_BACKEND));
        assert_eq!(feed.next_refresh_at(), T0 + 100_100.0 + 8000.0);
        let mut off = Aircraft::with_feeds(None);
        world.poll(&mut off, T0);
        assert_eq!(off.feed_error(), Some(NO_FEED));
        assert!(!off.live());
    }
}
