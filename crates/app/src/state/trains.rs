//! Trains (item T1; doc 06 §3.3, §3.9; `+page.svelte:160-183`, `:538-807`, `:1185-1190`,
//! `:1328-1334`): whether trains are offered (the Trafikverket key check), the trains in view and
//! when they are asked for, the trains watched live close in, the selected train with its live
//! stream and its route, the operators left out, and the list the map draws. `--demo` swaps the
//! backend's trains for the made-up ones of [`crate::dev::demo_trains`].
//!
//! Today's page runs this on `setTimeout`s and `$effect`s. Here every timer is a deadline in local
//! Unix ms, checked in [`Trains::poll`] once a frame and handed to the frame loop as
//! [`Wants::wake_at`](super::Wants); every effect is a comparison with what the last poll saw. A
//! poll runs, in this order:
//!
//! 1. the first poll only: the source (below), the key check (`checkTrafikverketKey` on mount),
//!    and the effects' first runs;
//! 2. the effects, in the page's order: the key check the keys dialog asked for
//!    ([`Trains::check_key`]) or its 30 s retry; the view effect (a new view, or trains on or
//!    off the map: the 600 ms settle, or everything cleared and nothing watched); the trains off
//!    the map let go of the selected one; the operators left out sent to the backend; a flight
//!    selected closes the train (one panel at a time; [`Trains::set_flight_open`]);
//! 3. the backend's `TrainsChanged`: the trains asked for again, at most every 1.5 s;
//! 4. the timers that are due: the settle, the 10 s refresh, the throttled reload, the route's;
//! 5. the answers: the key (no key or a refused one turns the trains mode back to aircraft and
//!    takes trains off the map), the trains in view (dropped when the camera has gone elsewhere;
//!    then the watched set), the route, the live stream's train and stops;
//! 6. the selected train as now known (the stream's report when it is not older than the list's,
//!    else the list's row, else the last one seen) and the list the map draws.
//!
//! **Timing** (`+page:542-690`): the trains in view are asked for [`TRAINS_SETTLE_MS`] after the
//! camera stands, then every [`TRAINS_REFRESH_MS`] (the timer is set again as each request
//! starts, so a camera that never settles still gets them); an answer for a view the camera has
//! since left (its centre further than half the visible radius from where it asked) is dropped,
//! and a failure keeps the trains on show. Zoomed in to [`LIVE_ZOOM`] the [`MAX_LIVE`] nearest of
//! them are watched live (`watchTrains`, sent only when the ids change); the backend's
//! `TrainsChanged` reloads at most every [`TRAINS_LIVE_MS`]. The route of a selected Trafikverket
//! train is asked for at once, again after [`ROUTE_LOADING_MS`] while incomplete,
//! [`ROUTE_REFRESH_MS`] when complete and a third of that after an error; the route layer's "off
//! its route" ([`Trains::route_off`]) asks again at most every [`ROUTE_OFF_MS`] and never while
//! asking. A key check that fails (Trafikverket out of reach) is tried again after
//! [`KEY_RETRY_MS`].
//!
//! **Selecting** ([`AppState::select_train`], `selectTrain`): the same train again does nothing;
//! another one closes the flight and the airport board (and the ship, item H1), starts its live
//! stream (`followTrain`, `unfollowTrain` when it changes) and its route, and lets go of the
//! camera's vehicle lock ([`Trains::following`]). [`AppState::pick_train`] is a pick from the list
//! or the search: the place marker goes, the aircraft lock lets go, the train is selected and the
//! camera flies to it at zoom 10 or closer.
//!
//! **What the others read**: [`Trains::on_map`] (`trainsOnMap`: the trains the filters keep and
//! that still run, with the selected train in its row's place or after them;
//! [`Trains::on_map_generation`] changes with it), [`Trains::selected`], [`Trains::live_stops`],
//! [`Trains::route`] (and its generation), [`Trains::offered`] and [`Trains::key_status`] (the mode
//! bar, the `i` key), [`Trains::trains`], [`Trains::loaded`], [`Trains::failed`] and
//! [`Trains::freshness`] (the list), [`Trains::watched`].
//!
//! **Sources**: `--demo` (the demo trains through the view centre each time the camera settles,
//! as `loadTrains` makes them without the backend: no refresh timer, nothing watched, no stream,
//! no route); else the `--fixture` folder ([`FIXTURE_TRAINS`]: the `Train` list `trains_in_view`
//! answers, read again at each request, its presence answering the key check "ok", else "none";
//! [`FIXTURE_ROUTES`]: routes by train id; no stream); else the backend (`trains::in_view`,
//! `watch`, `hide_operators`, `follow`, `unfollow`, `route`, `trafikverket::key_status`); else none
//! (the page without Tauri: the key check answers "none", nothing is asked). [`Trains::with_feeds`]
//! sets them instead (tests).
//!
//! Deviations from today's app:
//! - `--demo` works: the key check answers "ok" at once, the trains are put on the map at start
//!   (`alsoShow`) when the saved settings leave them off, and the demo trains count as an answer
//!   (the list is loaded, not failed). Today's `#demo` draws nothing: in a plain browser the key
//!   check answers "none" and turns the trains mode off (`docs/port/reference/README.md`).
//! - The canned sources (demo, fixture) are asked at the first frame, not 600 ms after it, so a
//!   shot (whose clock stands still) has them; later views settle as today.
//! - The view effect restarts its settle when the view's values change; the page restarts it on
//!   every report of the view, even of the same values.
//! - The window's visibility is not checked (`document.visibilityState`): the trains are asked for
//!   while the app runs.
//! - A key check asked while one is out drops the first one's answer; today the later of the two
//!   answers to arrive wins.
//! - The keys dialog's check starts at the next poll; a flight selected closes the train at the
//!   next poll, one frame later.
//! - The plan's "a shorter answer while loading does not replace a fuller set" is the signal
//!   blocks' rule (`Globe.svelte:2235`, doc 02 §6.1, item T5); today's train list and route have no
//!   such rule: every answer kept replaces the one before, and only a failure keeps the trains on
//!   show (tested here as today's app does it).
//!
//! Left to the items that own them: closing the ship when a train is selected and the train when a
//! ship is (H1: `select_train(None)`), a click on empty map letting go of the train
//! (`state::pick`), `closeTop` (U6), the airport board (U13), the keys dialog calling
//! [`Trains::check_key`] when it closes (U17), the camera's vehicle lock (it reads
//! [`Trains::following`]).

use std::collections::HashMap;
use std::f64::consts::PI;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use rustradar_backend::{Following, trafikverket, trains as backend_trains};
use rustradar_backend::{Backend, BackendEvent};
use rustradar_core::freshness::{Freshness, feed_freshness};
use rustradar_core::geo::{self, Area, cos_angle, view_area, visible_radius};
use rustradar_core::model::{TrafikverketKeyStatus as KeyStatus, Train, TrainEvent, TrainRoute, TrainSource, TrainStop};
use rustradar_core::settings::{Mode, shown_kinds};
use rustradar_core::train_list::{TrainFilters, train_filter};
use rustradar_core::trains::train_running;
use rustradar_scene::camera::ViewState;
use rustradar_scene::geom::js_max;

use super::aircraft::Answer;
use super::flight::SelectOptions;
use super::{AppState, Cx, Startup};
use crate::dev::demo_trains;
use crate::requests::Requests;

/// The camera stands this long before the trains in view are asked for, ms (`TRAINS_SETTLE_MS`).
pub const TRAINS_SETTLE_MS: f64 = 600.0;
/// The trains in view are asked for again this often, ms (`TRAINS_REFRESH_MS`).
pub const TRAINS_REFRESH_MS: f64 = 10_000.0;
/// Zoomed in this far, the trains in view are watched live (`LIVE_ZOOM`).
pub const LIVE_ZOOM: f64 = 9.0;
/// At most this many of them, nearest the centre first (`MAX_LIVE`).
pub const MAX_LIVE: usize = 40;
/// The backend's `TrainsChanged` reloads at most this often, ms (`TRAINS_LIVE_MS`).
pub const TRAINS_LIVE_MS: f64 = 1_500.0;
/// The route is asked for again this soon while its track is still loading, ms.
pub const ROUTE_LOADING_MS: f64 = 2_500.0;
/// The route is asked for again this often once complete (a third of it after an error), ms.
pub const ROUTE_REFRESH_MS: f64 = 30_000.0;
/// A train drawn off its route has it asked for again at most this often, ms.
pub const ROUTE_OFF_MS: f64 = 3_000.0;
/// The key check is tried again this long after it failed (Trafikverket out of reach), ms.
pub const KEY_RETRY_MS: f64 = 30_000.0;
/// The area asked for when most of the globe is in sight.
pub const WORLD: Area = Area { south: -90.0, north: 90.0, west: -180.0, east: 180.0 };

/// The `--fixture` folder's trains: a `Train` list as `trains_in_view` sends it. Its presence
/// answers the key check "ok".
pub const FIXTURE_TRAINS: &str = "trains_in_view.json";
/// The `--fixture` folder's routes: `{ train id: TrainRoute }`; a train without one fails.
pub const FIXTURE_ROUTES: &str = "train_routes.json";

/// A selected train's live news (`followTrain`'s channel): the backend's stream, or a channel
/// (fixtures, tests).
pub enum TrainStream {
    Backend(Following<TrainEvent>),
    Channel(Receiver<TrainEvent>),
}

impl TrainStream {
    /// The next event, if one is waiting.
    pub fn try_recv(&mut self) -> Option<TrainEvent> {
        match self {
            TrainStream::Backend(following) => following.try_recv(),
            TrainStream::Channel(rx) => rx.try_recv().ok(),
        }
    }
}

impl std::fmt::Debug for TrainStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TrainStream::Backend(_) => "TrainStream::Backend",
            TrainStream::Channel(_) => "TrainStream::Channel",
        })
    }
}

/// Where the trains' answers come from: the backend, the `--fixture` folder, or a test's.
pub trait TrainFeeds: Send {
    /// `trafikverketKeyStatus()`: an error when Trafikverket cannot be reached.
    fn key_status(&mut self) -> Answer<Result<KeyStatus, String>>;
    /// `trainsInView(area, zoom)`.
    fn trains_in_view(&mut self, area: Area, zoom: f64) -> Answer<Result<Vec<Train>, String>>;
    /// `watchTrains(ids)`.
    fn watch_trains(&mut self, ids: Vec<String>);
    /// `hideTrainOperators(names)`.
    fn hide_train_operators(&mut self, names: Vec<String>);
    /// `followTrain(id, …)`: the stream, or why it could not start.
    fn follow_train(&mut self, id: &str) -> Result<TrainStream, String>;
    /// `unfollowTrain()`.
    fn unfollow_train(&mut self);
    /// `trainRoute(id)`.
    fn train_route(&mut self, id: &str) -> Answer<Result<TrainRoute, String>>;
}

/// The backend's requests and streams, on its runtime (the backend has started: the keys are
/// read).
struct BackendFeeds {
    requests: Requests,
}

impl TrainFeeds for BackendFeeds {
    fn key_status(&mut self) -> Answer<Result<KeyStatus, String>> {
        self.requests.request(trafikverket::key_status()).into()
    }

    fn trains_in_view(&mut self, area: Area, zoom: f64) -> Answer<Result<Vec<Train>, String>> {
        self.requests.request(backend_trains::in_view(area.west, area.south, area.east, area.north, zoom)).into()
    }

    fn watch_trains(&mut self, ids: Vec<String>) {
        backend_trains::watch(ids);
    }

    fn hide_train_operators(&mut self, names: Vec<String>) {
        backend_trains::hide_operators(names);
    }

    fn follow_train(&mut self, id: &str) -> Result<TrainStream, String> {
        backend_trains::follow(id.to_owned(), self.requests.wake()).map(TrainStream::Backend)
    }

    fn unfollow_train(&mut self) {
        backend_trains::unfollow();
    }

    fn train_route(&mut self, id: &str) -> Answer<Result<TrainRoute, String>> {
        self.requests.request(backend_trains::route(id.to_owned())).into()
    }
}

/// The `--fixture` folder's files (see the module documentation), read when asked for.
struct FixtureFeeds {
    dir: PathBuf,
}

/// `dir/name` read as JSON of type `$t`, or why not.
macro_rules! read_json {
    ($t:ty, $dir:expr, $name:expr) => {{
        let path = $dir.join($name);
        std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice::<$t>(&bytes).map_err(|e| e.to_string()))
            .map_err(|e| format!("fixture {}: {e}", path.display()))
    }};
}

impl TrainFeeds for FixtureFeeds {
    fn key_status(&mut self) -> Answer<Result<KeyStatus, String>> {
        Answer::ready(Ok(if self.dir.join(FIXTURE_TRAINS).is_file() { KeyStatus::Ok } else { KeyStatus::None }))
    }

    fn trains_in_view(&mut self, _area: Area, _zoom: f64) -> Answer<Result<Vec<Train>, String>> {
        Answer::ready(read_json!(Vec<Train>, self.dir, FIXTURE_TRAINS))
    }

    fn watch_trains(&mut self, _ids: Vec<String>) {}

    fn hide_train_operators(&mut self, _names: Vec<String>) {}

    fn follow_train(&mut self, _id: &str) -> Result<TrainStream, String> {
        // nothing ever comes: the sender is gone
        let (_, rx) = mpsc::channel();
        Ok(TrainStream::Channel(rx))
    }

    fn unfollow_train(&mut self) {}

    fn train_route(&mut self, id: &str) -> Answer<Result<TrainRoute, String>> {
        Answer::ready(read_json!(HashMap<String, TrainRoute>, self.dir, FIXTURE_ROUTES).and_then(|mut routes| {
            routes.remove(id).ok_or_else(|| format!("fixture {}: no route for {id}", self.dir.join(FIXTURE_ROUTES).display()))
        }))
    }
}

/// Which trains the state shows.
enum Source {
    /// Chosen at the first poll: `--demo`, else the fixture folder, else the backend, else none.
    Auto,
    /// None (the page without Tauri).
    Off,
    /// The demo trains (`--demo`).
    Demo,
    On(Box<dyn TrainFeeds>),
}

impl Source {
    /// Answers that come at once, so they are asked at the first frame (see the deviations).
    fn canned(&self, fixture: bool) -> bool {
        matches!(self, Source::Demo) || (fixture && matches!(self, Source::On(_)))
    }
}

/// The request out for the trains in view, and the view it asked for (`asked`).
#[derive(Debug)]
struct InFlight {
    answer: Answer<Result<Vec<Train>, String>>,
    asked: ViewState,
}

/// What the map draws ([`Trains::on_map`], `trainsOnMap`): the trains the filters keep and that
/// still run, in the list's order, with the selected train in its row's place, or after them when
/// the filters leave it out.
#[derive(Debug, Clone, Copy)]
pub struct OnMap<'a> {
    trains: &'a [Train],
    /// Indexes into `trains`, in order.
    filtered: &'a [usize],
    selected: Option<&'a Train>,
    /// Where `selected` stands in for a filtered train; `None` with a selection: after them.
    at: Option<usize>,
}

impl<'a> OnMap<'a> {
    pub fn len(&self) -> usize {
        self.filtered.len() + usize::from(self.selected.is_some() && self.at.is_none())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The `i`-th train drawn.
    pub fn get(&self, i: usize) -> Option<&'a Train> {
        if self.at == Some(i) {
            return self.selected;
        }
        match self.filtered.get(i) {
            Some(&k) => Some(&self.trains[k]),
            None if i == self.filtered.len() && self.at.is_none() => self.selected,
            None => None,
        }
    }

    /// Every train drawn, in order.
    pub fn iter(&self) -> impl Iterator<Item = &'a Train> + 'a {
        let shown = *self;
        (0..shown.len()).filter_map(move |i| shown.get(i))
    }

    /// The list as the page's `trainsOnMap` array.
    pub fn to_vec(&self) -> Vec<Train> {
        self.iter().cloned().collect()
    }
}

/// What the list the map draws was last made from.
#[derive(Debug, Clone, PartialEq, Default)]
struct OnMapKey {
    trains: u64,
    picked: u64,
    filtered: Vec<usize>,
    at: Option<usize>,
}

/// JavaScript's default `sort()` order of strings: by UTF-16 code units.
fn js_string_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// The ids `watchInView` watches in `list` from `v`: none when `on` is false or the camera is out
/// beyond [`LIVE_ZOOM`]; else Trafikverket's trains, nearest the centre first, the first
/// [`MAX_LIVE`], in id order.
pub fn watched_ids(list: &[Train], v: &ViewState, on: bool) -> Vec<String> {
    let close = on && v.zoom >= LIVE_ZOOM;
    if !close {
        return Vec::new();
    }
    let mut near: Vec<(&str, f64)> = list
        .iter()
        .filter(|t| t.source == TrainSource::Trafikverket)
        .map(|t| (t.id.as_str(), cos_angle(v.longitude, v.latitude, t.lon, t.lat)))
        .collect();
    // `(a, b) => b.d - a.d`: a stable sort, the nearest (largest cosine) first
    near.sort_by(|a, b| (b.1 - a.1).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut ids: Vec<String> = near.into_iter().take(MAX_LIVE).map(|(id, _)| id.to_owned()).collect();
    ids.sort_by(|a, b| js_string_cmp(a, b));
    ids
}

/// Whether an answer asked for at `asked` is for a view the camera has left by `v` on a canvas of
/// `size` CSS pixels: its centre further than half the visible radius away (`away`).
pub fn left_behind(asked: &ViewState, v: &ViewState, size: [f64; 2]) -> bool {
    let radius = visible_radius(v.zoom, v.pitch, size[0], size[1]);
    cos_angle(v.longitude, v.latitude, asked.longitude, asked.latitude) < (radius * PI / 360.0).cos()
}

/// Trains: the key check, the trains in view and their timing, the watched set, the selected
/// train's stream and route, the list the map draws.
pub struct Trains {
    source: Source,
    started: bool,

    // the key check (`trafikverketKey`, `keyRetry`)
    key: Option<KeyStatus>,
    key_request: Option<Answer<Result<KeyStatus, String>>>,
    key_retry_at: Option<f64>,
    key_asked: bool,

    // the trains in view
    trains: Vec<Train>,
    /// Bumped whenever `trains` is set.
    trains_generation: u64,
    /// `trainsAt`: when the latest answer since the trains came on arrived; 0 before the first.
    trains_at: f64,
    failed: bool,
    request: Option<InFlight>,
    /// `trainsSettle`.
    settle_due: Option<f64>,
    /// `trainsRefresh`.
    refresh_due: Option<f64>,
    /// The view the last poll saw.
    last_view: Option<ViewState>,
    /// Trains on the map at the last poll.
    trains_were: bool,
    /// `trainsLiveAt`.
    live_at: f64,
    /// `trainsLivePending`: when the throttled reload fires.
    live_due: Option<f64>,
    /// `watchedKey`: the ids last watched, joined with commas.
    watched_key: String,
    /// The operators last sent as left out (`None` before the first).
    hidden_sent: Option<Vec<String>>,
    /// A flight is selected (from the state's poll).
    flight_open: bool,

    // the selected train
    /// `pickedTrain`, kept at `selectedTrain` (the stream's report, the list's row, or the last
    /// seen).
    picked: Option<Train>,
    picked_generation: u64,
    /// The id the stream and route effects last ran for (`followedId`).
    followed_id: Option<String>,
    stream: Option<TrainStream>,
    /// A stream was asked for: `unfollowTrain` when the selection changes.
    stream_asked: bool,
    live_train: Option<Train>,
    live_generation: u64,
    live_stops: Option<Vec<TrainStop>>,
    /// The generations `picked` was last brought up to date with.
    selected_for: (u64, u64, u64),
    /// `followingVehicle` for the train: the camera follows it.
    following: bool,

    // the route (`shownRoute` and its effect's `live`, `timer`, `asking`, `askedAt`)
    route: Option<TrainRoute>,
    route_generation: u64,
    /// The effect runs for the selected train (`reloadRoute` is set).
    route_live: bool,
    route_request: Option<Answer<Result<TrainRoute, String>>>,
    route_asked_at: f64,
    route_due: Option<f64>,

    // the list the map draws
    filtered: Vec<usize>,
    at: Option<usize>,
    on_map_for: OnMapKey,
    on_map_generation: u64,
}

impl Default for Trains {
    /// The trains of `--demo`, else the `--fixture` folder, else the backend, chosen at the first
    /// poll.
    fn default() -> Self {
        Trains::with_source(Source::Auto)
    }
}

impl std::fmt::Debug for Trains {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Trains")
            .field("key", &self.key)
            .field("trains", &self.trains.len())
            .field("on_map", &self.filtered.len())
            .field("trains_at", &self.trains_at)
            .field("failed", &self.failed)
            .field("loading", &self.request.is_some())
            .field("selected", &self.selected_id())
            .field("watched", &self.watched_key)
            .finish_non_exhaustive()
    }
}

impl Trains {
    fn with_source(source: Source) -> Self {
        Trains {
            source,
            started: false,
            key: None,
            key_request: None,
            key_retry_at: None,
            key_asked: false,
            trains: Vec::new(),
            trains_generation: 0,
            trains_at: 0.0,
            failed: false,
            request: None,
            settle_due: None,
            refresh_due: None,
            last_view: None,
            trains_were: false,
            live_at: 0.0,
            live_due: None,
            watched_key: String::new(),
            hidden_sent: None,
            flight_open: false,
            picked: None,
            picked_generation: 0,
            followed_id: None,
            stream: None,
            stream_asked: false,
            live_train: None,
            live_generation: 0,
            live_stops: None,
            selected_for: (0, 0, 0),
            following: false,
            route: None,
            route_generation: 0,
            route_live: false,
            route_request: None,
            route_asked_at: 0.0,
            route_due: None,
            filtered: Vec::new(),
            at: None,
            on_map_for: OnMapKey::default(),
            on_map_generation: 0,
        }
    }

    /// The trains on `feeds` instead of the demo, the fixture folder or the backend; `None`: none
    /// at all (the key check answers "none").
    pub fn with_feeds(feeds: Option<Box<dyn TrainFeeds>>) -> Self {
        Trains::with_source(match feeds {
            Some(feeds) => Source::On(feeds),
            None => Source::Off,
        })
    }

    /// The demo trains (`--demo`), whatever the startup says.
    pub fn demo() -> Self {
        Trains::with_source(Source::Demo)
    }

    // ---------------------------------------------------------------- what the others read

    /// What the key check said (`trafikverketKey`): `None` while it is not known.
    pub fn key_status(&self) -> Option<KeyStatus> {
        self.key
    }

    /// Whether trains are offered (`trainsOffered`: Trafikverket accepted the key).
    pub fn offered(&self) -> bool {
        self.key == Some(KeyStatus::Ok)
    }

    /// The trains in view as last answered (`trains`), unfiltered.
    pub fn trains(&self) -> &[Train] {
        &self.trains
    }

    /// Changes whenever [`Self::trains`] is set again.
    pub fn trains_generation(&self) -> u64 {
        self.trains_generation
    }

    /// When the latest answer since the trains came on arrived (`trainsAt`, local Unix ms); 0
    /// before the first.
    pub fn trains_at(&self) -> f64 {
        self.trains_at
    }

    /// Whether an answer has arrived since the trains came on (`trainsAt > 0`).
    pub fn loaded(&self) -> bool {
        self.trains_at > 0.0
    }

    /// Whether the latest request failed (`trainsFailed`; the trains on show stay). Without a
    /// source the list says so too (`failed || !isTauri()`).
    pub fn failed(&self) -> bool {
        self.failed || matches!(self.source, Source::Off)
    }

    /// Whether a request for the trains in view is out.
    pub fn loading(&self) -> bool {
        self.request.is_some()
    }

    /// The sidebar's age of the trains (`feedFreshness(now - trainsAt, TRAINS_REFRESH_MS)`),
    /// once they have arrived; the page shows it while trains are the focus.
    pub fn freshness(&self, now_ms: f64) -> Option<Freshness> {
        self.loaded().then(|| feed_freshness(js_max(0.0, now_ms - self.trains_at) as i64, TRAINS_REFRESH_MS as i64))
    }

    /// The ids watched live, as last sent (`watchedKey` split).
    pub fn watched(&self) -> Vec<&str> {
        if self.watched_key.is_empty() { Vec::new() } else { self.watched_key.split(',').collect() }
    }

    /// The list the map draws (`trainsOnMap`).
    pub fn on_map(&self) -> OnMap<'_> {
        OnMap { trains: &self.trains, filtered: &self.filtered, selected: self.picked.as_ref(), at: self.at }
    }

    /// Changes whenever [`Self::on_map`] does (a new answer, the filters, a train's run ended, the
    /// selection or its report). The page's array is new at each 1 s clock tick as well.
    pub fn on_map_generation(&self) -> u64 {
        self.on_map_generation
    }

    /// The selected train as now known (`selectedTrain`).
    pub fn selected(&self) -> Option<&Train> {
        self.picked.as_ref()
    }

    /// The selected train's id.
    pub fn selected_id(&self) -> Option<&str> {
        self.picked.as_ref().map(|t| t.id.as_str())
    }

    /// The selected train's stops as its live stream last sent them (`liveStops`).
    pub fn live_stops(&self) -> Option<&[TrainStop]> {
        self.live_stops.as_deref()
    }

    /// The selected train's way along the track as the backend last worked it out
    /// (`shownRoute`).
    pub fn route(&self) -> Option<&TrainRoute> {
        self.route.as_ref()
    }

    /// Changes whenever [`Self::route`] does.
    pub fn route_generation(&self) -> u64 {
        self.route_generation
    }

    /// Whether the camera follows the selected train (`followingVehicle`).
    pub fn following(&self) -> bool {
        self.following
    }

    // ---------------------------------------------------------------- what the others ask

    /// The keys dialog closed (`closeKeys` → `checkTrafikverketKey`): the key is checked again at
    /// the next poll.
    pub fn check_key(&mut self) {
        self.key_asked = true;
    }

    /// The route layer found the train off the route shown (`ontrainrouteoff` → `reloadRoute`):
    /// asked again now, unless a request is out or the last one is less than [`ROUTE_OFF_MS`] old.
    pub fn route_off(&mut self, now_ms: f64) {
        if self.route_live && self.route_request.is_none() && now_ms - self.route_asked_at >= ROUTE_OFF_MS {
            self.load_route(now_ms);
        }
    }

    /// The key C or the panel's Follow with a train selected (`followingVehicle =
    /// !followingVehicle`).
    pub fn toggle_following(&mut self) {
        self.following = !self.following;
    }

    /// Locks or lets go of the camera on the selected train.
    pub fn set_following(&mut self, following: bool) {
        self.following = following;
    }

    /// Whether a flight is selected, as the state's poll hands it over before this part's step:
    /// a selected flight closes the train (`select(id)` → `selectTrain(null)`).
    pub fn set_flight_open(&mut self, open: bool) {
        self.flight_open = open;
    }

    // ---------------------------------------------------------------- the page's functions

    /// The source, once: `--demo`, else the fixture folder, else the backend, else none.
    fn resolve(&mut self, startup: &Startup, backend: Option<&Backend>, requests: Option<&Requests>) {
        if !matches!(self.source, Source::Auto) {
            return;
        }
        self.source = if startup.demo {
            log::info!("the demo trains");
            Source::Demo
        } else if let Some(dir) = &startup.fixture {
            Source::On(Box::new(FixtureFeeds { dir: dir.clone() }))
        } else if let (Some(_), Some(requests)) = (backend, requests) {
            Source::On(Box::new(BackendFeeds { requests: requests.clone() }))
        } else {
            Source::Off
        };
    }

    /// `checkTrafikverketKey()`: the retry cleared; without the backend "none" at once (the demo:
    /// "ok"); else asked.
    fn check_key_now(&mut self, cx: &mut Cx<'_>) {
        self.key_retry_at = None;
        match &mut self.source {
            Source::On(feeds) => self.key_request = Some(feeds.key_status()),
            Source::Demo => self.set_key(KeyStatus::Ok, cx),
            Source::Off | Source::Auto => self.set_key(KeyStatus::None, cx),
        }
    }

    /// `trafikverketKey = status`, and its effect: without a key that works, the trains mode
    /// becomes aircraft and the trains leave the map. While the answer is unknown a trains mode
    /// already chosen stays.
    fn set_key(&mut self, status: KeyStatus, cx: &mut Cx<'_>) {
        let changed = self.key != Some(status);
        self.key = Some(status);
        if changed && matches!(status, KeyStatus::None | KeyStatus::Invalid) {
            let settings = &mut cx.persisted.settings;
            if settings.mode == Mode::Trains {
                settings.mode = Mode::Aircraft;
                cx.wants.scene = true;
            }
            if settings.also_show.contains(&Mode::Trains) {
                settings.also_show.retain(|k| *k != Mode::Trains);
                cx.wants.scene = true;
            }
        }
    }

    /// The view effect: trains off the map stop everything and drop what they brought; on, the
    /// trains are asked for once the camera has stood [`TRAINS_SETTLE_MS`].
    fn view_effect(&mut self, view: ViewState, on: bool, now: f64) {
        if !on {
            self.settle_due = None;
            self.refresh_due = None;
            // `trainsRequest++`: an answer on its way is dropped
            self.request = None;
            if !self.trains.is_empty() {
                self.set_trains(Vec::new());
            }
            self.trains_at = 0.0;
            self.failed = false;
            // no more live trains in view
            self.watch_in_view(&view, on);
            return;
        }
        self.settle_due = Some(now + TRAINS_SETTLE_MS);
    }

    /// `loadTrains()`.
    fn load_trains(&mut self, view: ViewState, size: [f64; 2], on: bool, now: f64) {
        // `stopTrains()`
        self.settle_due = None;
        self.refresh_due = None;
        if matches!(self.source, Source::Demo) {
            // made-up trains in the middle of the view
            if on {
                self.set_trains(demo_trains(view.latitude, view.longitude, now as i64));
                self.trains_at = now;
                self.failed = false;
            }
            return;
        }
        let Source::On(feeds) = &mut self.source else { return };
        if !on {
            return;
        }
        self.refresh_due = Some(now + TRAINS_REFRESH_MS);
        let area = view_area(geo::View { longitude: view.longitude, latitude: view.latitude, zoom: view.zoom, pitch: view.pitch }, size[0], size[1])
            .unwrap_or(WORLD);
        let answer = feeds.trains_in_view(area, view.zoom);
        // a request out before this one is dropped (`request !== trainsRequest`)
        self.request = Some(InFlight { answer, asked: view });
    }

    /// The answer to `loadTrains`'s request, if it has arrived: kept unless the trains left the
    /// map or the camera has gone elsewhere since it asked; then the watched set. A failure keeps
    /// the trains on show.
    fn take_trains(&mut self, view: ViewState, size: [f64; 2], on: bool, now: f64) -> bool {
        let Some(answer) = self.request.as_ref().and_then(|r| r.answer.poll()) else { return false };
        let Some(InFlight { asked, .. }) = self.request.take() else { return false };
        match answer {
            Ok(list) => {
                if !on || left_behind(&asked, &view, size) {
                    return false;
                }
                self.set_trains(list);
                self.trains_at = now;
                self.failed = false;
                self.watch_in_view(&view, on);
                true
            }
            // every source failed or is resting: the trains on show stay
            Err(e) => {
                log::debug!("trains in view: {e}");
                self.failed = true;
                true
            }
        }
    }

    fn set_trains(&mut self, trains: Vec<Train>) {
        self.trains = trains;
        self.trains_generation += 1;
    }

    /// `watchInView(list, v)`: the backend's trains only; sent when the ids change.
    fn watch_in_view(&mut self, v: &ViewState, on: bool) {
        let Source::On(feeds) = &mut self.source else { return };
        let ids = watched_ids(&self.trains, v, on);
        let key = ids.join(",");
        if key == self.watched_key {
            return;
        }
        self.watched_key = key;
        feeds.watch_trains(ids);
    }

    /// The effects of a new selection (`followedId` changed): the camera lets go, the old stream
    /// is let go of and the new one started, the route asked for.
    fn followed_changed(&mut self, now: f64) {
        let id = self.picked.as_ref().map(|t| t.id.clone());
        if id == self.followed_id {
            return;
        }
        self.followed_id = id.clone();
        // another train (or none): the camera lets go
        self.following = false;
        // the stream: the old one's cleanup, then the new one
        self.live_train = None;
        self.live_generation += 1;
        self.live_stops = None;
        self.stream = None;
        if std::mem::take(&mut self.stream_asked)
            && let Source::On(feeds) = &mut self.source
        {
            feeds.unfollow_train();
        }
        if let (Some(id), Source::On(feeds)) = (&id, &mut self.source) {
            match feeds.follow_train(id) {
                Ok(stream) => self.stream = Some(stream),
                Err(e) => log::debug!("following train {id}: {e}"),
            }
            self.stream_asked = true;
        }
        // the route: the old one's answers are dropped
        if self.route.take().is_some() {
            self.route_generation += 1;
        }
        self.route_request = None;
        self.route_due = None;
        self.route_live = matches!(self.source, Source::On(_)) && id.as_deref().is_some_and(|id| id.starts_with("trafikverket:"));
        if self.route_live {
            self.load_route(now);
        }
    }

    /// The route effect's `load()`.
    fn load_route(&mut self, now: f64) {
        self.route_due = None;
        let (Some(id), Source::On(feeds)) = (&self.followed_id, &mut self.source) else { return };
        self.route_request = Some(feeds.train_route(id));
        self.route_asked_at = now;
    }

    /// The route's answer, if it has arrived, and its timer.
    fn take_route(&mut self, now: f64) -> bool {
        let Some(answer) = self.route_request.as_ref().and_then(Answer::poll) else { return false };
        self.route_request = None;
        match answer {
            Ok(route) => {
                self.route_due = Some(now + if route.complete { ROUTE_REFRESH_MS } else { ROUTE_LOADING_MS });
                self.route = Some(route);
                self.route_generation += 1;
                true
            }
            // no timetable yet (it is learned with the next refresh): ask again
            Err(e) => {
                log::debug!("train route: {e}");
                self.route_due = Some(now + ROUTE_REFRESH_MS / 3.0);
                false
            }
        }
    }

    /// The selected train's stream: its reports and its stops.
    fn take_stream(&mut self) -> bool {
        let mut any = false;
        while let Some(event) = self.stream.as_mut().and_then(TrainStream::try_recv) {
            match event {
                TrainEvent::Train { train } => {
                    if self.followed_id.as_deref() == Some(train.id.as_str()) {
                        self.live_train = Some(*train);
                        self.live_generation += 1;
                        any = true;
                    }
                }
                TrainEvent::Stops { stops } => {
                    self.live_stops = Some(stops);
                    any = true;
                }
            }
        }
        any
    }

    /// `selectedTrain`, and the effect that keeps `pickedTrain` at it: the stream's report if it
    /// is the same train and not older than the list's, else the list's row, else the last seen.
    fn refresh_selected(&mut self) -> bool {
        let key = (self.trains_generation, self.live_generation, self.picked_generation);
        if key == self.selected_for {
            return false;
        }
        self.selected_for = key;
        let Some(picked) = &self.picked else { return false };
        let listed = self.trains.iter().find(|t| t.id == picked.id).unwrap_or(picked);
        let selected = match &self.live_train {
            Some(live) if live.id == picked.id && live.position_unix >= listed.position_unix => live,
            _ => listed,
        };
        if selected == picked {
            return false;
        }
        let selected = selected.clone();
        self.picked = Some(selected);
        self.picked_generation += 1;
        self.selected_for.2 = self.picked_generation;
        true
    }

    /// `trainsFiltered` and `trainsOnMap`: made again when the trains, the filters, a train's run
    /// or the selection changed.
    fn refilter(&mut self, filters: &TrainFilters, now: f64) -> bool {
        let keep = train_filter(filters);
        let now = now as i64;
        let filtered: Vec<usize> =
            self.trains.iter().enumerate().filter(|(_, t)| keep(t) && train_running(t, now)).map(|(i, _)| i).collect();
        let at = self.picked.as_ref().and_then(|p| filtered.iter().position(|&i| self.trains[i].id == p.id));
        let key = OnMapKey { trains: self.trains_generation, picked: self.picked_generation, filtered, at };
        if key == self.on_map_for {
            return false;
        }
        self.filtered.clone_from(&key.filtered);
        self.at = key.at;
        self.on_map_for = key;
        self.on_map_generation += 1;
        true
    }

    /// `selectTrain(train)`'s own part: the same train again does nothing; returns whether the
    /// selection changed.
    fn pick(&mut self, train: Option<Train>, now: f64) -> bool {
        if let Some(t) = &train
            && self.selected_id() == Some(t.id.as_str())
        {
            return false;
        }
        if self.picked.is_none() && train.is_none() {
            return false;
        }
        self.picked = train;
        self.picked_generation += 1;
        self.followed_changed(now);
        true
    }

    /// This frame's step (see [`super::AppState::poll`] for the order, and the module's
    /// documentation for the steps).
    pub fn poll(&mut self, cx: &mut Cx<'_>) {
        let now = cx.clocks.local_ms;
        let view = cx.controller.view();
        let size = cx.controller.size();
        let mut changed = false;

        if !self.started {
            self.start(cx, view, size, now);
        } else {
            // the keys dialog closed, or the retry is due
            let retry = self.key_retry_at.is_some_and(|at| now >= at);
            if std::mem::take(&mut self.key_asked) || retry {
                self.check_key_now(cx);
            }
            let settings = &cx.persisted.settings;
            let on = shown_kinds(settings.mode, &settings.also_show).trains;
            if self.last_view != Some(view) || on != self.trains_were {
                self.last_view = Some(view);
                self.view_effect(view, on, now);
            }
            // the trains off the map: nothing left to show
            if on != self.trains_were {
                self.trains_were = on;
                if !on {
                    changed |= self.pick(None, now);
                }
            }
            self.hidden_effect(&cx.persisted.train_filters);
        }
        // one panel at a time: a flight selected closes the train
        if self.flight_open && self.picked.is_some() {
            changed |= self.pick(None, now);
        }
        let settings = &cx.persisted.settings;
        let on = shown_kinds(settings.mode, &settings.also_show).trains;

        // the backend moved Trafikverket's trains: asked again for the view, at most every 1.5 s
        for event in cx.events {
            if matches!(event, BackendEvent::TrainsChanged) && on && self.live_due.is_none() {
                self.live_due = Some(now + js_max(0.0, self.live_at + TRAINS_LIVE_MS - now));
            }
        }

        // the timers
        if self.settle_due.is_some_and(|at| now >= at) || self.refresh_due.is_some_and(|at| now >= at) {
            self.load_trains(view, size, on, now);
        }
        if self.live_due.is_some_and(|at| now >= at) {
            self.live_due = None;
            self.live_at = now;
            self.load_trains(view, size, on, now);
        }
        if self.route_due.is_some_and(|at| now >= at) {
            self.load_route(now);
        }

        // the answers
        if let Some(answer) = self.key_request.as_ref().and_then(Answer::poll) {
            self.key_request = None;
            match answer {
                Ok(status) => self.set_key(status, cx),
                // Trafikverket out of reach: the key may be fine, ask again shortly
                Err(e) => {
                    log::debug!("Trafikverket key check: {e}");
                    self.key_retry_at = Some(now + KEY_RETRY_MS);
                }
            }
        }
        changed |= self.take_trains(view, size, on, now);
        changed |= self.take_route(now);
        changed |= self.take_stream();

        // the selected train and the list the map draws
        changed |= self.refresh_selected();
        changed |= self.refilter(&cx.persisted.train_filters, now);
        if changed {
            cx.wants.scene = true;
        }
        for due in [self.settle_due, self.refresh_due, self.live_due, self.route_due, self.key_retry_at].into_iter().flatten() {
            cx.wants.wake_at(due);
        }
    }

    /// The page's mount: the source, the key check, the effects' first runs.
    fn start(&mut self, cx: &mut Cx<'_>, view: ViewState, size: [f64; 2], now: f64) {
        self.started = true;
        self.resolve(cx.startup, cx.backend, cx.requests);
        // `--demo` puts the trains on the map (the focus stays)
        if matches!(self.source, Source::Demo) {
            let settings = &mut cx.persisted.settings;
            if !shown_kinds(settings.mode, &settings.also_show).trains {
                settings.also_show.push(Mode::Trains);
            }
        }
        self.check_key_now(cx);
        let settings = &cx.persisted.settings;
        let on = shown_kinds(settings.mode, &settings.also_show).trains;
        self.last_view = Some(view);
        self.trains_were = on;
        if on && self.source.canned(cx.startup.fixture.is_some()) {
            self.load_trains(view, size, on, now);
        } else {
            self.view_effect(view, on, now);
        }
        self.hidden_effect(&cx.persisted.train_filters);
    }

    /// The operators left out are not streamed live either (`hideTrainOperators`): sent at the
    /// start and whenever they change.
    fn hidden_effect(&mut self, filters: &TrainFilters) {
        if self.hidden_sent.as_ref() == Some(&filters.hidden_operators) {
            return;
        }
        self.hidden_sent = Some(filters.hidden_operators.clone());
        if let Source::On(feeds) = &mut self.source {
            feeds.hide_train_operators(filters.hidden_operators.clone());
        }
    }
}

impl AppState {
    /// `selectTrain(train)` at local time `now_ms` (a click on the map, the list, Escape with
    /// `None`): the same train again does nothing; another one closes the flight and the airport
    /// board, and starts its stream and its route.
    pub fn select_train(&mut self, train: Option<Train>, now_ms: f64) {
        self.trains.resolve(&self.startup, self.backend.as_ref(), self.requests.as_ref());
        let opening = train.is_some();
        if !self.trains.pick(train, now_ms) || !opening {
            return;
        }
        // one panel at a time; the focus (and the sidebar) stays where it is
        if self.flight.selected_id().is_some() {
            self.select_flight(None, SelectOptions::default(), now_ms);
        }
        // (the ship closes too: item H1)
        self.flight.set_focus_airport(None);
    }

    /// `pickTrain(train)`: picked from the list or the search, so the place marker goes, the
    /// aircraft lock lets go, the train is selected and the camera flies to it, at zoom 10 or
    /// closer.
    pub fn pick_train(&mut self, train: Train, now_ms: f64) {
        self.flight.set_place(None);
        self.flight.set_following(false);
        let (lon, lat) = (train.lon, train.lat);
        self.select_train(Some(train), now_ms);
        self.sync_flight(now_ms);
        let zoom = js_max(self.controller.view().zoom, 10.0);
        self.controller.fly_to(lon, lat, Some(zoom), now_ms);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex};

    use rustradar_core::model::{RouteStop, TrainKind};
    use rustradar_core::train_list::DEFAULT_TRAIN_FILTERS;

    use super::*;

    /// The local clock at the first frame.
    const T0: f64 = 1_782_036_000_000.0;
    /// Stockholm (V3): watched live, close in.
    const V3: (f64, f64, f64) = (59.33, 18.07, 9.0);
    const SIZE: [f64; 2] = [1440.0, 900.0];

    type Listed = Result<Vec<Train>, String>;
    type Routed = Result<TrainRoute, String>;

    /// The requests the trains state made, with the senders that answer them.
    #[derive(Default)]
    struct Log {
        keys: Vec<Sender<Result<KeyStatus, String>>>,
        asked: Vec<(Area, f64, Sender<Listed>)>,
        watched: Vec<Vec<String>>,
        hidden: Vec<Vec<String>>,
        follows: Vec<(String, Sender<TrainEvent>)>,
        unfollows: usize,
        routes: Vec<(String, Sender<Routed>)>,
    }

    /// Canned feeds: every request waits until the test answers it.
    #[derive(Clone, Default)]
    struct Canned(Arc<Mutex<Log>>);

    impl TrainFeeds for Canned {
        fn key_status(&mut self) -> Answer<Result<KeyStatus, String>> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().keys.push(tx);
            rx.into()
        }

        fn trains_in_view(&mut self, area: Area, zoom: f64) -> Answer<Listed> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().asked.push((area, zoom, tx));
            rx.into()
        }

        fn watch_trains(&mut self, ids: Vec<String>) {
            self.0.lock().unwrap().watched.push(ids);
        }

        fn hide_train_operators(&mut self, names: Vec<String>) {
            self.0.lock().unwrap().hidden.push(names);
        }

        fn follow_train(&mut self, id: &str) -> Result<TrainStream, String> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().follows.push((id.to_owned(), tx));
            Ok(TrainStream::Channel(rx))
        }

        fn unfollow_train(&mut self) {
            self.0.lock().unwrap().unfollows += 1;
        }

        fn train_route(&mut self, id: &str) -> Answer<Routed> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().routes.push((id.to_owned(), tx));
            rx.into()
        }
    }

    impl Canned {
        fn log(&self) -> std::sync::MutexGuard<'_, Log> {
            self.0.lock().unwrap()
        }

        fn requests(&self) -> usize {
            self.log().asked.len()
        }

        fn answer_key(&self, status: Result<KeyStatus, String>) {
            self.log().keys.last().expect("a key check").send(status).unwrap();
        }

        /// Answers the latest trains request.
        fn answer(&self, list: Listed) {
            self.log().asked.last().expect("a request").2.send(list).unwrap();
        }

        fn routes(&self) -> usize {
            self.log().routes.len()
        }

        fn answer_route(&self, route: Routed) {
            self.log().routes.last().expect("a route request").1.send(route).unwrap();
        }

        fn send(&self, event: TrainEvent) {
            self.log().follows.last().expect("a stream").1.send(event).unwrap();
        }
    }

    fn view((latitude, longitude, zoom): (f64, f64, f64)) -> ViewState {
        ViewState { longitude, latitude, zoom, bearing: 0.0, pitch: 0.0, position: None }
    }

    /// The whole app's state on canned train feeds at `at`, trains the focus, with backend
    /// events sent on the returned channel.
    fn app(canned: &Canned, at: (f64, f64, f64)) -> (AppState, Sender<BackendEvent>) {
        let (tx, rx) = mpsc::channel();
        let mut s = AppState::new(Startup::default(), None, None, Some(rx), view(at), SIZE);
        s.persisted.settings.mode = Mode::Trains;
        s.trains = Trains::with_feeds(Some(Box::new(canned.clone())));
        (s, tx)
    }

    /// A Trafikverket train at `(lat, lon)`, reported at `position_unix`.
    fn train(n: u32, lat: f64, lon: f64) -> Train {
        Train {
            id: format!("trafikverket:2026-06-21:{n}"),
            source: TrainSource::Trafikverket,
            name: format!("Pendeltåg {n}"),
            kind: TrainKind::Suburban,
            lat,
            lon,
            heading: Some(90.0),
            speed_kmh: Some(60.0),
            destination: Some("Södertälje centrum".into()),
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

    fn route(complete: bool, stops: usize) -> TrainRoute {
        TrainRoute {
            segments: vec![vec![[18.07, 59.33], [18.0, 59.2]]],
            stops: (0..stops).map(|i| RouteStop { name: format!("Stop {i}"), lat: 59.3, lon: 18.0, track: None, stops: true }).collect(),
            complete,
        }
    }

    /// The first poll with the key answered "ok", trains the focus at V3: nothing asked yet (the
    /// camera has to settle).
    fn started(canned: &Canned) -> (AppState, Sender<BackendEvent>) {
        let (mut s, tx) = app(canned, V3);
        s.poll(T0);
        canned.answer_key(Ok(KeyStatus::Ok));
        s.poll(T0);
        assert!(s.trains.offered());
        (s, tx)
    }

    #[test]
    fn the_camera_settles_600_ms_then_every_10_s() {
        let canned = Canned::default();
        let (mut s, _tx) = started(&canned);
        assert_eq!(canned.requests(), 0, "nothing before the camera has stood 600 ms");
        let wants = s.poll(T0 + 599.0);
        assert_eq!(wants.wake_at, Some(T0 + TRAINS_SETTLE_MS));
        assert_eq!(canned.requests(), 0);
        s.poll(T0 + 600.0);
        assert_eq!(canned.requests(), 1);
        {
            let log = canned.log();
            let (area, zoom, _) = &log.asked[0];
            assert_eq!(*zoom, 9.0);
            let expected = view_area(geo::View { longitude: 18.07, latitude: 59.33, zoom: 9.0, pitch: 0.0 }, 1440.0, 900.0).unwrap();
            assert_eq!(*area, expected);
        }
        // the refresh is set again as each request starts, whenever its answer comes
        assert_eq!(s.poll(T0 + 700.0).wake_at, Some(T0 + 600.0 + TRAINS_REFRESH_MS));
        s.poll(T0 + 600.0 + TRAINS_REFRESH_MS - 1.0);
        assert_eq!(canned.requests(), 1);
        s.poll(T0 + 600.0 + TRAINS_REFRESH_MS);
        assert_eq!(canned.requests(), 2, "a second request without an answer to the first");
        s.poll(T0 + 600.0 + 2.0 * TRAINS_REFRESH_MS);
        assert_eq!(canned.requests(), 3);
    }

    #[test]
    fn a_moving_camera_waits_and_the_refresh_still_comes() {
        let canned = Canned::default();
        let (mut s, _tx) = started(&canned);
        s.poll(T0 + 600.0);
        assert_eq!(canned.requests(), 1);
        // the camera moves every 300 ms: the settle never ends, the 10 s refresh still comes
        let mut t = T0 + 600.0;
        let mut lon = 18.07;
        while t < T0 + 600.0 + TRAINS_REFRESH_MS + 1000.0 {
            t += 300.0;
            lon += 0.0001;
            s.controller.set_view(view((59.33, lon, 9.0)));
            // a move alone changes nothing on the map: the scene keeps its own pace
            assert!(!s.poll(t).scene);
        }
        assert_eq!(canned.requests(), 2, "only the refresh asked");
        // it stands: 600 ms later it is asked for
        let stood = t;
        s.poll(stood + 599.0);
        assert_eq!(canned.requests(), 2);
        s.poll(stood + 600.0);
        assert_eq!(canned.requests(), 3);
        // the same view reported again is no move
        s.controller.set_view(view((59.33, lon, 9.0)));
        s.poll(stood + 700.0);
        s.poll(stood + 1300.0);
        assert_eq!(canned.requests(), 3);
    }

    #[test]
    fn an_answer_is_kept_and_the_nearest_40_watched_close_in() {
        let canned = Canned::default();
        let (mut s, _tx) = started(&canned);
        s.poll(T0 + 600.0);
        // 50 trains on a line east of the centre, listed far to near
        let list: Vec<Train> = (0..50).rev().map(|n| train(n, 59.33, 18.07 + 0.001 * f64::from(n))).collect();
        canned.answer(Ok(list.clone()));
        let wants = s.poll(T0 + 800.0);
        assert!(wants.scene);
        assert_eq!(s.trains.trains(), &list[..]);
        assert_eq!(s.trains.trains_at(), T0 + 800.0);
        assert!(s.trains.loaded() && !s.trains.failed());
        assert_eq!(s.trains.on_map().len(), 50);
        // the 40 nearest (n = 0..40), in id order (as strings: "…:10" before "…:2")
        let mut nearest: Vec<String> = (0..40).map(|n| format!("trafikverket:2026-06-21:{n}")).collect();
        nearest.sort();
        assert_eq!(canned.log().watched, [nearest.clone()]);
        assert_eq!(s.trains.watched(), nearest.iter().map(String::as_str).collect::<Vec<_>>());
        // the same set again is not sent again
        s.poll(T0 + 600.0 + TRAINS_REFRESH_MS);
        canned.answer(Ok(list.clone()));
        s.poll(T0 + 600.0 + TRAINS_REFRESH_MS + 100.0);
        assert_eq!(canned.log().watched.len(), 1);
        // zoomed out below 9: the next answer watches none
        s.controller.set_view(view((59.33, 18.07, 8.9)));
        s.poll(T0 + 20_000.0);
        s.poll(T0 + 20_600.0);
        canned.answer(Ok(list));
        s.poll(T0 + 20_700.0);
        assert_eq!(canned.log().watched.last().unwrap(), &Vec::<String>::new());
        assert!(s.trains.watched().is_empty());
    }

    #[test]
    fn watched_ids_are_trafikverkets_nearest_first_then_in_utf16_order() {
        let v = view(V3);
        let list = vec![train(1, 59.33, 18.08), train(2, 59.33, 18.071), train(3, 59.5, 18.07)];
        assert_eq!(watched_ids(&list, &v, true), ["trafikverket:2026-06-21:1", "trafikverket:2026-06-21:2", "trafikverket:2026-06-21:3"]);
        assert!(watched_ids(&list, &v, false).is_empty(), "trains off the map");
        assert!(watched_ids(&list, &view((59.33, 18.07, 8.99)), true).is_empty(), "out beyond zoom 9");
        // the 40 kept are the nearest, whatever their ids
        let many: Vec<Train> = (0..45).map(|n| train(n, 59.33, 18.07 + 0.01 * f64::from(45 - n))).collect();
        let ids = watched_ids(&many, &v, true);
        assert_eq!(ids.len(), MAX_LIVE);
        assert!(!ids.contains(&"trafikverket:2026-06-21:0".to_owned()), "the farthest left out");
        assert!(ids.contains(&"trafikverket:2026-06-21:44".to_owned()));
        // JavaScript's sort: UTF-16 units, so U+FF5E sorts after U+1F600's surrogates (UTF-8 bytes
        // would put it before)
        assert_eq!(js_string_cmp("\u{ff5e}", "\u{1f600}"), std::cmp::Ordering::Greater);
        assert_eq!("\u{ff5e}".cmp("\u{1f600}"), std::cmp::Ordering::Less);
    }

    #[test]
    fn an_answer_for_a_view_left_behind_is_dropped_and_a_failure_keeps_the_trains() {
        let canned = Canned::default();
        let (mut s, _tx) = started(&canned);
        s.poll(T0 + 600.0);
        let first = vec![train(1, 59.33, 18.07)];
        canned.answer(Ok(first.clone()));
        s.poll(T0 + 700.0);
        assert_eq!(s.trains.trains(), &first[..]);
        // asked for again, then the camera jumps to Gothenburg before the answer
        s.poll(T0 + 600.0 + TRAINS_REFRESH_MS);
        s.controller.set_view(view((57.70, 11.97, 9.0)));
        canned.answer(Ok(vec![train(2, 59.33, 18.07)]));
        s.poll(T0 + 600.0 + TRAINS_REFRESH_MS + 100.0);
        assert_eq!(s.trains.trains(), &first[..], "an answer for Stockholm is not shown over Gothenburg");
        assert_eq!(s.trains.trains_at(), T0 + 700.0);
        // a small move is no reason to drop it
        assert!(!left_behind(&view(V3), &view((59.34, 18.08, 9.0)), SIZE));
        assert!(left_behind(&view(V3), &view((57.70, 11.97, 9.0)), SIZE));
        // Gothenburg's own request, once the camera has stood there: a failure keeps the trains
        // on show, and the list says so
        s.poll(T0 + 10_700.0 + TRAINS_SETTLE_MS);
        assert_eq!(canned.requests(), 3);
        canned.answer(Err("Trafikverket is resting".into()));
        s.poll(T0 + 11_400.0);
        assert!(s.trains.failed());
        assert_eq!(s.trains.trains(), &first[..]);
        // the next answer clears it
        s.poll(T0 + 11_300.0 + TRAINS_REFRESH_MS);
        assert_eq!(canned.requests(), 4);
        canned.answer(Ok(vec![]));
        s.poll(T0 + 21_400.0);
        assert!(!s.trains.failed());
        assert!(s.trains.trains().is_empty());
    }

    #[test]
    fn every_answer_kept_replaces_the_set_even_a_shorter_one() {
        // the plan's "a shorter answer while loading does not replace a fuller set" is the signal
        // blocks' rule (T5); today's trains and routes take every answer they keep
        let canned = Canned::default();
        let (mut s, _tx) = started(&canned);
        s.poll(T0 + 600.0);
        canned.answer(Ok((0..10).map(|n| train(n, 59.33, 18.07)).collect()));
        s.poll(T0 + 700.0);
        assert_eq!(s.trains.trains().len(), 10);
        s.poll(T0 + 600.0 + TRAINS_REFRESH_MS);
        canned.answer(Ok((0..3).map(|n| train(n, 59.33, 18.07)).collect()));
        s.poll(T0 + 600.0 + TRAINS_REFRESH_MS + 100.0);
        assert_eq!(s.trains.trains().len(), 3);
        // a newer request drops the older one's answer, however long it is
        s.poll(T0 + 600.0 + 2.0 * TRAINS_REFRESH_MS);
        s.controller.set_view(view((59.331, 18.07, 9.0)));
        s.poll(T0 + 600.0 + 2.0 * TRAINS_REFRESH_MS + 100.0);
        s.poll(T0 + 600.0 + 2.0 * TRAINS_REFRESH_MS + 700.0);
        assert_eq!(canned.requests(), 4);
        // (its receiver is gone with it)
        let _ = canned.log().asked[2].2.send(Ok((0..20).map(|n| train(n, 59.33, 18.07)).collect()));
        s.poll(T0 + 600.0 + 2.0 * TRAINS_REFRESH_MS + 800.0);
        assert_eq!(s.trains.trains().len(), 3, "the dropped request's answer is not taken");
        canned.answer(Ok((0..2).map(|n| train(n, 59.33, 18.07)).collect()));
        s.poll(T0 + 600.0 + 2.0 * TRAINS_REFRESH_MS + 900.0);
        assert_eq!(s.trains.trains().len(), 2);
    }

    #[test]
    fn trains_changed_reloads_at_most_every_1500_ms() {
        let canned = Canned::default();
        let (mut s, tx) = started(&canned);
        s.poll(T0 + 600.0);
        assert_eq!(canned.requests(), 1);
        // the first change after a quiet while reloads at once
        tx.send(BackendEvent::TrainsChanged).unwrap();
        s.poll(T0 + 1000.0);
        assert_eq!(canned.requests(), 2);
        // more within 1.5 s wait for it, and come as one
        tx.send(BackendEvent::TrainsChanged).unwrap();
        let wants = s.poll(T0 + 1200.0);
        assert_eq!(wants.wake_at, Some(T0 + 2500.0));
        tx.send(BackendEvent::TrainsChanged).unwrap();
        s.poll(T0 + 2000.0);
        assert_eq!(canned.requests(), 2);
        s.poll(T0 + 2499.0);
        assert_eq!(canned.requests(), 2);
        s.poll(T0 + 2500.0);
        assert_eq!(canned.requests(), 3);
        // the reload restarted the 10 s refresh
        assert_eq!(s.poll(T0 + 2600.0).wake_at, Some(T0 + 2500.0 + TRAINS_REFRESH_MS));
        // with the trains off the map the backend's changes ask nothing
        s.persisted.settings.mode = Mode::Aircraft;
        s.poll(T0 + 5000.0);
        tx.send(BackendEvent::TrainsChanged).unwrap();
        s.poll(T0 + 6000.0);
        s.poll(T0 + 9000.0);
        assert_eq!(canned.requests(), 3);
    }

    #[test]
    fn trains_off_the_map_stop_clear_unwatch_and_deselect() {
        let canned = Canned::default();
        let (mut s, _tx) = started(&canned);
        s.poll(T0 + 600.0);
        let list = vec![train(1, 59.33, 18.07), train(2, 59.331, 18.071)];
        canned.answer(Ok(list.clone()));
        s.poll(T0 + 700.0);
        s.select_train(Some(list[0].clone()), T0 + 800.0);
        assert_eq!(s.trains.selected_id(), Some("trafikverket:2026-06-21:1"));
        // a request out when they go off
        s.poll(T0 + 600.0 + TRAINS_REFRESH_MS);
        assert_eq!(canned.requests(), 2);
        s.persisted.settings.mode = Mode::Aircraft;
        let wants = s.poll(T0 + 11_000.0);
        assert!(wants.scene);
        assert!(s.trains.trains().is_empty() && !s.trains.loaded() && !s.trains.failed());
        assert!(s.trains.on_map().is_empty());
        assert_eq!(s.trains.selected(), None, "the selected train closes");
        assert_eq!(canned.log().unfollows, 1);
        assert_eq!(canned.log().watched.last().unwrap(), &Vec::<String>::new(), "no more live trains in view");
        // the answer on its way is dropped (nobody waits for it any more); no timer is left
        assert!(canned.log().asked.last().unwrap().2.send(Ok(list.clone())).is_err());
        let wants = s.poll(T0 + 11_100.0);
        assert!(s.trains.trains().is_empty());
        assert_eq!(wants.wake_at, None);
        s.poll(T0 + 60_000.0);
        assert_eq!(canned.requests(), 2);
        // back on: asked once the camera has stood 600 ms
        s.persisted.settings.also_show = vec![Mode::Trains];
        s.poll(T0 + 70_000.0);
        assert_eq!(canned.requests(), 2);
        s.poll(T0 + 70_600.0);
        assert_eq!(canned.requests(), 3);
    }

    #[test]
    fn the_key_check_offers_trains_or_turns_the_trains_mode_off() {
        let canned = Canned::default();
        let (mut s, _tx) = app(&canned, V3);
        s.persisted.settings.also_show = vec![Mode::Ships, Mode::Trains];
        s.poll(T0);
        assert_eq!(canned.log().keys.len(), 1, "asked on mount");
        // while unknown, the trains mode chosen stays
        assert_eq!(s.trains.key_status(), None);
        assert!(!s.trains.offered());
        s.poll(T0 + 100.0);
        assert_eq!(s.persisted.settings.mode, Mode::Trains);
        canned.answer_key(Ok(KeyStatus::Invalid));
        let wants = s.poll(T0 + 200.0);
        assert!(wants.scene);
        assert_eq!(s.trains.key_status(), Some(KeyStatus::Invalid));
        assert_eq!(s.persisted.settings.mode, Mode::Aircraft);
        assert_eq!(s.persisted.settings.also_show, [Mode::Ships]);
        // trains off: the settle never asks
        s.poll(T0 + 2000.0);
        assert_eq!(canned.requests(), 0);
        // the keys dialog closes with a new key: checked again, now fine
        s.trains.check_key();
        s.poll(T0 + 3000.0);
        assert_eq!(canned.log().keys.len(), 2);
        canned.answer_key(Ok(KeyStatus::Ok));
        s.poll(T0 + 3100.0);
        assert!(s.trains.offered());
        assert_eq!(s.persisted.settings.mode, Mode::Aircraft, "an offer chooses nothing");
    }

    #[test]
    fn a_key_check_that_fails_is_tried_again_after_30_s() {
        let canned = Canned::default();
        let (mut s, _tx) = app(&canned, V3);
        s.poll(T0);
        canned.answer_key(Err("can't reach Trafikverket".into()));
        let wants = s.poll(T0 + 100.0);
        assert_eq!(s.trains.key_status(), None, "the key may be fine");
        assert_eq!(s.persisted.settings.mode, Mode::Trains);
        assert_eq!(wants.wake_at, Some(T0 + 600.0), "the settle first");
        s.poll(T0 + 100.0 + KEY_RETRY_MS - 1.0);
        assert_eq!(canned.log().keys.len(), 1);
        s.poll(T0 + 100.0 + KEY_RETRY_MS);
        assert_eq!(canned.log().keys.len(), 2);
        canned.answer_key(Ok(KeyStatus::None));
        s.poll(T0 + 100.0 + KEY_RETRY_MS + 10.0);
        assert_eq!(s.trains.key_status(), Some(KeyStatus::None));
        assert_eq!(s.persisted.settings.mode, Mode::Aircraft);
        // asked by the dialog while a retry waits: the retry is cleared
        let canned = Canned::default();
        let (mut s, _tx) = app(&canned, V3);
        s.poll(T0);
        canned.answer_key(Err("offline".into()));
        s.poll(T0 + 100.0);
        s.trains.check_key();
        s.poll(T0 + 200.0);
        assert_eq!(canned.log().keys.len(), 2);
        s.poll(T0 + 100.0 + KEY_RETRY_MS);
        assert_eq!(canned.log().keys.len(), 2, "no retry after the dialog's check");
    }

    #[test]
    fn without_a_backend_there_are_no_trains() {
        let (_tx, rx) = mpsc::channel();
        let mut s = AppState::new(Startup::default(), None, None, Some(rx), view(V3), SIZE);
        s.persisted.settings.mode = Mode::Trains;
        let wants = s.poll(T0);
        assert!(wants.scene);
        assert_eq!(s.trains.key_status(), Some(KeyStatus::None));
        assert_eq!(s.persisted.settings.mode, Mode::Aircraft);
        assert!(s.trains.failed(), "the list says the trains could not come");
        s.poll(T0 + 1000.0);
        assert!(s.trains.trains().is_empty());
    }

    #[test]
    fn the_route_reloads_on_its_timers() {
        let canned = Canned::default();
        let (mut s, _tx) = started(&canned);
        let t = train(7, 59.33, 18.07);
        s.select_train(Some(t.clone()), T0 + 1000.0);
        assert_eq!(canned.routes(), 1, "asked for at once");
        assert_eq!(canned.log().routes[0].0, t.id);
        // still loading: again after 2.5 s
        canned.answer_route(Ok(route(false, 1)));
        let wants = s.poll(T0 + 1100.0);
        assert!(wants.scene);
        assert_eq!(s.trains.route(), Some(&route(false, 1)));
        assert_eq!(wants.wake_at, Some(T0 + 1100.0 + ROUTE_LOADING_MS), "before the trains' 10 s refresh");
        s.poll(T0 + 1100.0 + ROUTE_LOADING_MS - 1.0);
        assert_eq!(canned.routes(), 1);
        s.poll(T0 + 1100.0 + ROUTE_LOADING_MS);
        assert_eq!(canned.routes(), 2);
        // complete: every 30 s
        let at = T0 + 1100.0 + ROUTE_LOADING_MS + 50.0;
        canned.answer_route(Ok(route(true, 2)));
        s.poll(at);
        assert_eq!(s.trains.route(), Some(&route(true, 2)));
        s.poll(at + ROUTE_REFRESH_MS - 1.0);
        assert_eq!(canned.routes(), 2);
        s.poll(at + ROUTE_REFRESH_MS);
        assert_eq!(canned.routes(), 3);
        // an error: again after 10 s, the route on show kept
        let at = at + ROUTE_REFRESH_MS + 50.0;
        canned.answer_route(Err("no timetable yet".into()));
        s.poll(at);
        assert_eq!(s.trains.route(), Some(&route(true, 2)));
        s.poll(at + ROUTE_REFRESH_MS / 3.0 - 1.0);
        assert_eq!(canned.routes(), 3);
        s.poll(at + 10_000.0);
        assert_eq!(canned.routes(), 4);
        // a shorter route while loading replaces the one shown (today's `shownRoute = r`)
        canned.answer_route(Ok(route(false, 0)));
        s.poll(at + 10_050.0);
        assert_eq!(s.trains.route(), Some(&route(false, 0)));
    }

    #[test]
    fn off_its_route_the_route_is_asked_again_at_most_every_3_s() {
        let canned = Canned::default();
        let (mut s, _tx) = started(&canned);
        s.select_train(Some(train(7, 59.33, 18.07)), T0 + 1000.0);
        // not while asking
        s.trains.route_off(T0 + 5000.0);
        assert_eq!(canned.routes(), 1);
        canned.answer_route(Ok(route(true, 1)));
        s.poll(T0 + 1500.0);
        // less than 3 s after the last ask
        s.trains.route_off(T0 + 3999.0);
        assert_eq!(canned.routes(), 1);
        s.trains.route_off(T0 + 4000.0);
        assert_eq!(canned.routes(), 2);
        // the reload cleared the 30 s timer: the next one runs from its answer
        canned.answer_route(Ok(route(true, 1)));
        s.poll(T0 + 4100.0);
        s.poll(T0 + 1500.0 + ROUTE_REFRESH_MS);
        assert_eq!(canned.routes(), 2);
        s.poll(T0 + 4100.0 + ROUTE_REFRESH_MS);
        assert_eq!(canned.routes(), 3);
        // a train of another source has no route, and nothing reloads it
        let mut other = train(8, 59.33, 18.07);
        other.id = "sl:8".into();
        s.select_train(Some(other), T0 + 40_000.0);
        assert_eq!(s.trains.route(), None);
        s.trains.route_off(T0 + 50_000.0);
        s.poll(T0 + 200_000.0);
        assert_eq!(canned.routes(), 3);
    }

    #[test]
    fn the_selected_train_streams_and_shows_the_newer_report() {
        let canned = Canned::default();
        let (mut s, _tx) = started(&canned);
        s.poll(T0 + 600.0);
        let listed = train(7, 59.33, 18.07);
        canned.answer(Ok(vec![listed.clone(), train(8, 59.34, 18.08)]));
        s.poll(T0 + 700.0);
        s.select_train(Some(listed.clone()), T0 + 800.0);
        assert_eq!(canned.log().follows.len(), 1);
        assert_eq!(canned.log().follows[0].0, listed.id);
        // the same train again does nothing
        s.select_train(Some(listed.clone()), T0 + 900.0);
        assert_eq!(canned.log().follows.len(), 1);
        assert_eq!(canned.routes(), 1);
        // the stream's newer report is shown, in the list's place on the map
        let mut live = listed.clone();
        live.lat = 59.335;
        live.position_unix += 5;
        canned.send(TrainEvent::Train { train: Box::new(live.clone()) });
        canned.send(TrainEvent::Stops { stops: vec![TrainStop { name: "Flemingsberg".into(), ..Default::default() }] });
        let wants = s.poll(T0 + 1000.0);
        assert!(wants.scene);
        assert_eq!(s.trains.selected(), Some(&live));
        assert_eq!(s.trains.live_stops().map(|s| s.len()), Some(1));
        let on_map: Vec<Train> = s.trains.on_map().to_vec();
        assert_eq!(on_map.len(), 2);
        assert_eq!(on_map[0], live, "drawn from its live report");
        // another train's report on the stream is not this one's
        let mut stray = train(8, 59.0, 18.0);
        stray.position_unix += 60;
        canned.send(TrainEvent::Train { train: Box::new(stray) });
        s.poll(T0 + 1100.0);
        assert_eq!(s.trains.selected(), Some(&live));
        // the list's report is newer: the list's row
        s.poll(T0 + 600.0 + TRAINS_REFRESH_MS);
        let mut newer = listed.clone();
        newer.lat = 59.34;
        newer.position_unix += 10;
        canned.answer(Ok(vec![newer.clone()]));
        s.poll(T0 + 600.0 + TRAINS_REFRESH_MS + 100.0);
        assert_eq!(s.trains.selected(), Some(&newer));
        // the list drops it: the last seen stays, drawn after the others
        s.poll(T0 + 600.0 + 2.0 * TRAINS_REFRESH_MS);
        canned.answer(Ok(vec![train(9, 59.3, 18.0)]));
        s.poll(T0 + 600.0 + 2.0 * TRAINS_REFRESH_MS + 100.0);
        assert_eq!(s.trains.selected(), Some(&newer));
        let ids: Vec<String> = s.trains.on_map().iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids, ["trafikverket:2026-06-21:9", "trafikverket:2026-06-21:7"]);
        // another train: the old stream is let go of, the new one started; the route too
        s.select_train(Some(train(9, 59.3, 18.0)), T0 + 30_000.0);
        assert_eq!(canned.log().unfollows, 1);
        assert_eq!(canned.log().follows.len(), 2);
        assert_eq!(s.trains.live_stops(), None);
        assert_eq!(canned.routes(), 2);
        // none: let go of
        s.select_train(None, T0 + 31_000.0);
        assert_eq!(canned.log().unfollows, 2);
        assert_eq!(canned.log().follows.len(), 2);
        assert_eq!(s.trains.selected(), None);
    }

    #[test]
    fn the_map_draws_the_filtered_running_trains_and_the_selection() {
        let canned = Canned::default();
        let (mut s, _tx) = started(&canned);
        s.poll(T0 + 600.0);
        let mut ended = train(3, 59.33, 18.07);
        ended.ended_unix = Some((T0 / 1000.0) as i64);
        let mut other = train(4, 59.33, 18.07);
        other.operator = Some("Mälartåg".into());
        canned.answer(Ok(vec![train(1, 59.33, 18.07), train(2, 59.33, 18.07), ended, other.clone()]));
        s.poll(T0 + 700.0);
        let ids = |s: &AppState| s.trains.on_map().iter().map(|t| t.id.rsplit(':').next().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(ids(&s), ["1", "2", "3", "4"]);
        let generation = s.trains.on_map_generation();
        // a train 30 s past the end of its run goes; the list only changes then
        s.poll(T0 + 30_000.0);
        assert_eq!(s.trains.on_map_generation(), generation);
        let wants = s.poll(T0 + 30_001.0);
        assert!(wants.scene);
        assert_eq!(ids(&s), ["1", "2", "4"]);
        // the filters narrow the map too; the selection stays drawn after the rest
        s.select_train(Some(other.clone()), T0 + 30_100.0);
        s.persisted.train_filters = TrainFilters { hidden_operators: vec!["Mälartåg".into()], ..DEFAULT_TRAIN_FILTERS };
        s.poll(T0 + 30_200.0);
        assert_eq!(ids(&s), ["1", "2", "4"]);
        assert_eq!(s.trains.on_map().get(2), Some(&other));
        assert_eq!(s.trains.on_map().get(3), None);
        // and sent to the backend, which streams them no more
        assert_eq!(canned.log().hidden, [vec![], vec!["Mälartåg".to_owned()]]);
        s.poll(T0 + 30_300.0);
        assert_eq!(canned.log().hidden.len(), 2, "sent when they change");
    }

    #[test]
    fn a_train_closes_the_flight_and_the_board_and_a_flight_the_train() {
        let canned = Canned::default();
        let (mut s, _tx) = started(&canned);
        s.flight.set_focus_airport(Some("arn".into()));
        s.select_flight(Some(42), SelectOptions::default(), T0 + 100.0);
        assert_eq!(s.flight.selected_id(), Some(42));
        s.select_train(Some(train(7, 59.33, 18.07)), T0 + 200.0);
        assert_eq!(s.flight.selected_id(), None);
        assert_eq!(s.flight.focus_airport(), None);
        assert_eq!(s.trains.selected_id(), Some("trafikverket:2026-06-21:7"));
        s.poll(T0 + 300.0);
        assert!(s.trains.selected().is_some(), "no flight is open");
        // a flight selected afterwards closes the train at the next poll
        s.select_flight(Some(43), SelectOptions::default(), T0 + 400.0);
        let wants = s.poll(T0 + 500.0);
        assert!(wants.scene);
        assert_eq!(s.trains.selected(), None);
        assert_eq!(canned.log().unfollows, 1);
        assert_eq!(s.flight.selected_id(), Some(43));
    }

    #[test]
    fn a_pick_flies_to_the_train_and_lets_go_of_the_locks() {
        let canned = Canned::default();
        let (mut s, _tx) = started(&canned);
        s.flight.set_place(Some(crate::state::flight::MarkedPlace { lon: 18.0, lat: 59.0, name: "Solna".into() }));
        s.flight.set_following(true);
        s.trains.set_following(true);
        let t = train(7, 59.6, 17.9);
        s.pick_train(t.clone(), T0 + 100.0);
        assert!(s.flight.place().is_none());
        assert!(!s.flight.following());
        assert!(!s.trains.following(), "a new train: the camera lets go");
        assert_eq!(s.trains.selected(), Some(&t));
        s.advance_camera(T0 + 100.0 + 10_000.0);
        let v = s.controller.view();
        assert!((v.longitude - 17.9).abs() < 1e-9 && (v.latitude - 59.6).abs() < 1e-9, "{v:?}");
        assert_eq!(v.zoom, 10.0, "zoom 10 from 9");
        // already closer in: the zoom stays
        s.controller.set_view(view((59.6, 17.9, 13.0)));
        s.pick_train(train(8, 59.61, 17.91), T0 + 20_000.0);
        s.advance_camera(T0 + 40_000.0);
        assert_eq!(s.controller.view().zoom, 13.0);
        // the vehicle lock: C toggles it, another train lets go
        s.trains.toggle_following();
        assert!(s.trains.following());
        s.select_train(Some(train(9, 59.6, 17.9)), T0 + 41_000.0);
        assert!(!s.trains.following());
    }

    #[test]
    fn the_demo_trains_run_through_the_view_centre_without_the_backend() {
        let (_tx, rx) = mpsc::channel();
        let mut s = AppState::new(Startup { demo: true, ..Default::default() }, None, None, Some(rx), view(V3), SIZE);
        let wants = s.poll(T0);
        assert!(wants.scene);
        assert!(s.trains.offered(), "the demo stands in for Trafikverket");
        assert_eq!(s.persisted.settings.mode, Mode::Aircraft, "the focus stays");
        assert_eq!(s.persisted.settings.also_show, [Mode::Trains], "on the map");
        // at once on the first frame: a shot's clock stands still
        assert_eq!(s.trains.trains(), &demo_trains(59.33, 18.07, T0 as i64)[..]);
        assert!(s.trains.loaded() && !s.trains.failed());
        assert_eq!(s.trains.on_map().len(), 6);
        assert_eq!(wants.wake_at, None, "no refresh timer for the demo");
        // a new view: made again round its centre once the camera has stood 600 ms
        s.controller.set_view(view((57.7, 11.97, 12.0)));
        s.poll(T0 + 1000.0);
        s.poll(T0 + 1599.0);
        assert_eq!(s.trains.trains()[0].lat, demo_trains(59.33, 18.07, T0 as i64)[0].lat);
        s.poll(T0 + 1600.0);
        assert_eq!(s.trains.trains(), &demo_trains(57.7, 11.97, (T0 + 1600.0) as i64)[..]);
        // a selected demo train has no stream and no route
        let t = s.trains.trains()[1].clone();
        s.select_train(Some(t.clone()), T0 + 2000.0);
        s.poll(T0 + 2100.0);
        assert_eq!(s.trains.selected(), Some(&t));
        assert_eq!(s.trains.route(), None);
        s.trains.route_off(T0 + 10_000.0);
        assert_eq!(s.poll(T0 + 10_000.0).wake_at, None);
    }

    #[test]
    fn a_fixture_folder_answers_the_key_the_trains_and_the_routes() {
        let dir = std::env::temp_dir().join(format!("rustradar-trains-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let t = train(7, 59.33, 18.07);
        std::fs::write(dir.join(FIXTURE_TRAINS), serde_json::to_vec(&vec![t.clone()]).unwrap()).unwrap();
        let routes: HashMap<String, TrainRoute> = [(t.id.clone(), route(true, 2))].into_iter().collect();
        std::fs::write(dir.join(FIXTURE_ROUTES), serde_json::to_vec(&routes).unwrap()).unwrap();
        let (_tx, rx) = mpsc::channel();
        let startup = Startup { fixture: Some(dir.clone()), ..Default::default() };
        let mut s = AppState::new(startup, None, None, Some(rx), view(V3), SIZE);
        s.persisted.settings.mode = Mode::Trains;
        s.poll(T0);
        assert!(s.trains.offered());
        assert_eq!(s.trains.trains(), std::slice::from_ref(&t), "at the first frame");
        s.select_train(Some(t.clone()), T0 + 100.0);
        s.poll(T0 + 200.0);
        assert_eq!(s.trains.route(), Some(&route(true, 2)));
        // a folder without trains: none offered
        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let (_tx, rx) = mpsc::channel();
        let mut s = AppState::new(Startup { fixture: Some(empty), ..Default::default() }, None, None, Some(rx), view(V3), SIZE);
        s.persisted.settings.mode = Mode::Trains;
        s.poll(T0);
        assert_eq!(s.trains.key_status(), Some(KeyStatus::None));
        assert_eq!(s.persisted.settings.mode, Mode::Aircraft);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_backend_without_a_key_offers_no_trains_and_reaches_nothing() {
        // the backend's own functions on the app's runtime, as the window runs them; no key is
        // saved in this process, so none of them may reach the network
        if !rustradar_backend::keys::current().trafikverket.is_empty() {
            return;
        }
        let rt = crate::requests::runtime().expect("runtime");
        rustradar_backend::set_rt(rt.handle().clone());
        let mut feeds = BackendFeeds { requests: Requests::new(rt.handle().clone(), crate::requests::no_wake()) };
        fn wait<T>(answer: Answer<T>) -> T {
            let started = std::time::Instant::now();
            loop {
                if let Some(v) = answer.poll() {
                    return v;
                }
                assert!(started.elapsed() < std::time::Duration::from_secs(10), "no answer");
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        assert_eq!(wait(feeds.key_status()), Ok(KeyStatus::None));
        let area = view_area(geo::View { longitude: 18.07, latitude: 59.33, zoom: 9.0, pitch: 0.0 }, 1440.0, 900.0).unwrap();
        assert_eq!(wait(feeds.trains_in_view(area, 9.0)), Ok(vec![]), "no source without the key");
        // the stream starts and sends nothing; letting go of it and the watch are lock writes
        let mut stream = feeds.follow_train("trafikverket:2026-06-21:7").expect("a stream");
        assert!(stream.try_recv().is_none());
        feeds.unfollow_train();
        feeds.watch_trains(vec![]);
        feeds.hide_train_operators(vec![]);
    }

    #[test]
    fn freshness_once_the_trains_have_come() {
        let canned = Canned::default();
        let (mut s, _tx) = started(&canned);
        assert_eq!(s.trains.freshness(T0 + 1000.0), None);
        s.poll(T0 + 600.0);
        canned.answer(Ok(vec![]));
        s.poll(T0 + 1000.0);
        assert_eq!(s.trains.freshness(T0 + 6000.0).map(|f| f.stale), Some(false));
        assert_eq!(s.trains.freshness(T0 + 1000.0 + 120_000.0).map(|f| f.stale), Some(true));
    }
}
