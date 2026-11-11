//! RustRadar's backend without Tauri: HTTP, caches, feeds and streams.
//!
//! The modules moved here from `src-tauri/src` (plan 1.2, doc 07 §10). This file holds what replaces
//! the Tauri facilities they leaned on: the folders ([`Dirs`]), events to the UI ([`BackendEvent`],
//! [`Notifier`]), ordered streams to one caller ([`Following`]) and the runtime background work is
//! spawned on ([`rt`]); and [`Backend`], what was Tauri's managed `AppState`, with the requests that
//! use it (Flightradar24, reference data, radar, session, keys, export). Requests that only use
//! module state stay free functions in their modules (`trains::in_view`, `ships::details`, ...).

pub mod adsblol;
pub mod board;
pub mod building_lod;
pub mod convert;
pub mod devlog;
pub mod dwd;
pub mod export;
pub mod flow;
pub mod geocode;
pub mod images;
pub mod keys;
pub mod lines;
pub mod liveries;
pub mod mobile_update;
pub mod mrms;
pub mod opensky;
pub mod osm_platforms;
pub mod radar;
pub mod rail;
pub mod rail_motion;
pub mod reference;
pub mod roads;
pub mod runways;
pub mod session;
pub mod ships;
pub mod sth;
pub mod taxiways;
pub mod tiles;
pub mod trafikverket;
pub mod train_class;
pub mod train_media;
pub mod trains;
pub mod weather;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fr24::Fr24;
use fr24::grpc::{
    BoundingBox, FlightDetailsParams, FlightDetailsRecord, FollowFlightParams, LiveFeedParams, TopFlightRecord,
    TopFlightsParams,
};
use fr24::json::{AirportListParams, FindEntry, FindParams, FlightListParams, FlightListQuery, FlightListRecord};
use fr24::proto::common::Icon;
use futures_util::StreamExt;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::AbortHandle;

/// What every request answers with: the UI prints the error's text as it is.
pub type CmdResult<T> = Result<T, String>;

/// Whether the track, signal and platform stores stay off the network and only answer from what they
/// hold: in this crate's tests.
pub(crate) const OFFLINE: bool = cfg!(test);

/// The app's identifier: its folders are named after it, as Tauri named them.
pub const APP_ID: &str = "com.felix.rustradar";

/// Unix milliseconds now.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or_default()
}

/// The folders the backend reads and writes, resolved once by the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dirs {
    /// Everything that can be fetched again: reference data, tiles, colours.
    pub cache: PathBuf,
    /// What the user entered: `keys.json`.
    pub config: PathBuf,
    /// Where exports are saved; none on phones.
    pub downloads: Option<PathBuf>,
}

impl Dirs {
    /// The folders of a desktop install, the same ones the Tauri build used, so the keys file and
    /// every cache carry over. None when the system names no cache or config folder (and on phones,
    /// which have their own).
    pub fn desktop() -> Option<Self> {
        Some(Self {
            cache: dirs::cache_dir()?.join(APP_ID),
            config: dirs::config_dir()?.join(APP_ID),
            downloads: dirs::download_dir().or_else(dirs::home_dir),
        })
    }

    /// A file in the cache folder.
    pub fn cache_file(&self, name: &str) -> PathBuf {
        self.cache.join(name)
    }

    /// A file in the config folder.
    pub fn config_file(&self, name: &str) -> PathBuf {
        self.config.join(name)
    }

    /// Tells the modules that keep files between runs where to: the rail network's tiles (and beside
    /// them the signals'), the far-off buildings and the stations' platforms. Only the first call counts.
    pub fn share_cache(&self) {
        osm_platforms::set_cache_dir(self.cache.clone());
        building_lod::set_cache_dir(self.cache.clone());
        rail::set_cache_dir(self.cache.clone());
    }
}

/// Something background work tells the UI without having been asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendEvent {
    /// A live stream moved trains in the cache: ask `trains::in_view` again (throttle 1500 ms).
    TrainsChanged,
    /// A newer version is ready: downloaded (desktop) or downloadable (Android).
    UpdateReady { version: String },
}

/// Called after every event or finished request, so the UI thread draws again.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// How background work reaches the UI: an event on the queue, then a wake. Cheap to clone.
#[derive(Clone)]
pub struct Notifier {
    events: std::sync::mpsc::Sender<BackendEvent>,
    wake: Wake,
}

impl Notifier {
    pub fn new(events: std::sync::mpsc::Sender<BackendEvent>, wake: Wake) -> Self {
        Self { events, wake }
    }

    /// Queues `event` and wakes the UI. False once the UI's end of the queue is gone.
    pub fn notify(&self, event: BackendEvent) -> bool {
        let sent = self.events.send(event).is_ok();
        (self.wake)();
        sent
    }

    /// Wakes the UI without an event: a request finished, a stream delivered.
    pub fn wake(&self) {
        (self.wake)();
    }

    /// The wake on its own, for a [`Following`].
    pub fn waker(&self) -> Wake {
        self.wake.clone()
    }
}

/// The sending end of a [`Following`]: what a `tauri::ipc::Channel` was to the stream's task.
pub struct FollowSender<T> {
    tx: UnboundedSender<T>,
    wake: Wake,
}

impl<T> Clone for FollowSender<T> {
    fn clone(&self) -> Self {
        Self { tx: self.tx.clone(), wake: self.wake.clone() }
    }
}

impl<T> FollowSender<T> {
    /// Sends `event` and wakes the UI. An error (with the event back) once the receiver is gone,
    /// which is when the task should end.
    pub fn send(&self, event: T) -> Result<(), T> {
        let sent = self.tx.send(event).map_err(|e| e.0);
        if sent.is_ok() {
            (self.wake)();
        }
        sent
    }

    /// Whether the receiver is gone.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// A running stream: events arrive in order; dropping it (or calling [`Following::stop`]) aborts
/// the task.
pub struct Following<T> {
    pub rx: UnboundedReceiver<T>,
    task: AbortHandle,
}

impl<T: Send + 'static> Following<T> {
    /// Starts `task` on the backend's runtime ([`rt`]), handing it the sending end. `wake` is called
    /// after every event sent.
    pub fn spawn<F, Fut>(wake: Wake, task: F) -> Self
    where
        F: FnOnce(FollowSender<T>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self::spawn_on(rt(), wake, task)
    }

    /// As [`Following::spawn`], on a runtime of the caller's choosing.
    pub fn spawn_on<F, Fut>(rt: &Handle, wake: Wake, task: F) -> Self
    where
        F: FnOnce(FollowSender<T>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (tx, rx) = unbounded_channel();
        let task = rt.spawn(task(FollowSender { tx, wake })).abort_handle();
        Self { rx, task }
    }
}

impl<T> Following<T> {
    /// The next event if one is waiting: what the UI drains each frame.
    pub fn try_recv(&mut self) -> Option<T> {
        self.rx.try_recv().ok()
    }

    /// The handle the task can be aborted with by whoever replaces this stream.
    pub fn abort_handle(&self) -> AbortHandle {
        self.task.clone()
    }

    /// Whether the task has ended, by itself or aborted.
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// Ends the stream.
    pub fn stop(self) {}
}

impl<T> Drop for Following<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn rt_cell() -> &'static OnceLock<Handle> {
    static RT: OnceLock<Handle> = OnceLock::new();
    &RT
}

/// Sets the runtime background work is spawned on. Call it once at start, before anything else in
/// this crate; a second call is ignored (false).
pub fn set_rt(handle: Handle) -> bool {
    rt_cell().set(handle).is_ok()
}

/// The runtime background work is spawned on, from any thread: several spawns happen in synchronous
/// functions (`rail::tracks_of`, `osm_platforms::near`, the signal tiles), which the UI thread may
/// call, and `tokio::spawn` panics outside a runtime.
///
/// Panics when [`set_rt`] was never called and the caller isn't inside a runtime either (inside
/// one, that runtime is taken, which is what a `#[tokio::test]` gets).
pub fn rt() -> &'static Handle {
    rt_cell().get_or_init(|| {
        Handle::try_current().expect("rustradar_backend::set_rt must be called before background work starts")
    })
}

// ------------------------------------------------------------------ the backend

/// Number of concurrent requests when fetching the world feed.
const WORLD_CONCURRENCY: usize = 8;
/// Give up following a flight after this many consecutive failed subscriptions.
const FOLLOW_MAX_FAILURES: u32 = 3;
const FOLLOW_RETRY_DELAY: Duration = Duration::from_secs(3);

/// What the UI asks things of: the Flightradar24 client and what is loaded once, the folders, the
/// runtime and the way back to the UI. Cheap to clone; every clone is the same backend.
#[derive(Clone)]
pub struct Backend {
    inner: Arc<Inner>,
}

struct Inner {
    /// Swapped out when signing in or out.
    fr24: RwLock<Arc<Fr24>>,
    /// The `FollowFlight` stream of the selected flight.
    follow: Mutex<Option<AbortHandle>>,
    /// Airlines and airports, loaded once.
    reference: tokio::sync::Mutex<Option<Arc<reference::ReferenceData>>>,
    /// Runways by airport, loaded once.
    runways: tokio::sync::Mutex<Option<Arc<runways::RunwayData>>>,
    /// Logo-derived livery colours per IATA code, loaded from disk on first use.
    colors: tokio::sync::Mutex<Option<liveries::ColorCache>>,
    /// The newest volume from each 3D radar source, keyed by its name.
    radar: tokio::sync::Mutex<HashMap<&'static str, Arc<radar::Volume>>>,
    /// Sources with a fetch under way, so overlapping calls don't duplicate it.
    radar_fetching: Mutex<HashSet<&'static str>>,
    /// OpenSky tracks per aircraft, and whether it told us to back off.
    opensky: tokio::sync::Mutex<opensky::Cache>,
    dirs: Dirs,
    rt: Handle,
    /// The events and the wake, to the UI.
    notifier: Notifier,
}

impl Inner {
    fn client(&self) -> Arc<Fr24> {
        self.fr24.read().unwrap().clone()
    }
}

fn to_string(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// A live flight, an answer of `live_flights`, and what a followed flight's stream brings
// (`Backend::follow_flight`): `rustradar_core::model`.
pub use rustradar_core::model::{FollowEvent, LiveFlight, LiveSnapshot};

/// Furthest back a buffered position may be; anything else is corrupt
/// (upstream occasionally sends an underflowed delta near 2^32).
const MAX_BUFFER_MS: u32 = 60_000;

/// The usable part of a position buffer (recent history, newest first):
/// strictly increasing times back from the report, within range.
fn buffered_positions(buffer: Option<&fr24::proto::common::PositionBuffer>) -> Vec<[i32; 3]> {
    let mut last = 0;
    let mut out = Vec::new();
    for p in buffer.map(|b| b.recent_positions_list.as_slice()).unwrap_or_default() {
        if p.delta_ms > last && p.delta_ms <= MAX_BUFFER_MS {
            last = p.delta_ms;
            out.push([p.delta_lat, p.delta_lon, p.delta_ms as i32]);
        }
    }
    out
}

/// A live feed's flight as the globe gets it (without `vspeed`).
fn live_flight(f: fr24::proto::common::Flight) -> LiveFlight {
    let positions = buffered_positions(f.position_buffer.as_ref());
    let extra = f.extra_info.unwrap_or_default();
    let route = extra.route.unwrap_or_default();
    LiveFlight {
        id: f.flightid as u32,
        lat: f.lat,
        lon: f.lon,
        track: f.track,
        alt: f.alt,
        speed: f.speed,
        on_ground: f.on_ground,
        timestamp_ms: f.timestamp_ms,
        callsign: f.callsign,
        flight: extra.flight,
        reg: extra.reg,
        typecode: extra.r#type,
        origin: route.from,
        destination: route.to,
        icon: Icon::try_from(f.icon)
            .map(|i| i.as_str_name().to_owned())
            .unwrap_or_default(),
        positions,
        vspeed: None,
    }
}

async fn follow_task(fr24: Arc<Fr24>, flight_id: u32, channel: FollowSender<FollowEvent>) {
    let mut details = FlightDetailsRecord::default();
    let mut failures = 0;
    loop {
        let mut received = false;
        match fr24
            .follow_flight(&FollowFlightParams::new(flight_id))
            .await
        {
            Ok(stream) => {
                let mut stream = std::pin::pin!(stream);
                while let Some(message) = stream.next().await {
                    match message {
                        Ok(message) => {
                            received = true;
                            let update = FlightDetailsRecord::from(&message);
                            // the first message of a subscription is a full snapshot
                            if update.aircraft.is_some() {
                                details = update;
                            } else {
                                details.apply_update(update);
                            }
                            if channel
                                .send(FollowEvent::Details(Box::new(convert::flight_details(details.clone()))))
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(e) => {
                            let _ = channel.send(FollowEvent::Error(e.to_string()));
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                let _ = channel.send(FollowEvent::Error(e.to_string()));
            }
        }
        failures = if received { 0 } else { failures + 1 };
        if failures >= FOLLOW_MAX_FAILURES {
            let _ = channel.send(FollowEvent::Ended("flight is no longer live".into()));
            return;
        }
        tokio::time::sleep(FOLLOW_RETRY_DELAY).await;
    }
}

/// Sources of measured 3D reflectivity with the region each one covers. A
/// volume costs tens of megabytes to fetch, so a source is only asked for
/// when its region is actually in view. DWD's figures are the extent of its
/// WN composite grid, written out here to keep the table self-contained.
const RADAR_SOURCES: [(&str, radar::Bounds); 2] = [
    ("mrms", mrms::BOUNDS),
    (
        "dwd",
        radar::Bounds {
            west: 1.4,
            south: 45.6,
            east: 18.9,
            north: 56.3,
        },
    ),
];

/// Whether a source's region overlaps the view; `None` is the whole world.
fn in_view(bounds: &radar::Bounds, area: Option<&BoundingBox>) -> bool {
    let Some(area) = area else {
        return true;
    };
    if f64::from(area.south) > bounds.north || f64::from(area.north) < bounds.south {
        return false;
    }
    let (west, east) = (f64::from(area.west), f64::from(area.east));
    if west > east {
        // the view straddles the antimeridian, so it is really two spans:
        // west..180 and -180..east. Neither radar region wraps itself.
        bounds.east >= west || bounds.west <= east
    } else {
        bounds.west <= east && bounds.east >= west
    }
}

impl Backend {
    /// Builds the HTTP client, stores the folders, loads `keys.json`, starts the two Trafikverket
    /// supervisors and the launch login. `rt` is where background work runs (also made [`rt`] if
    /// none is set yet); `events` is how it reaches the UI; `wake` is called after every event or
    /// stream message so the UI thread draws again.
    pub fn start(
        dirs: Dirs,
        rt: Handle,
        events: std::sync::mpsc::Sender<BackendEvent>,
        wake: Wake,
    ) -> CmdResult<Backend> {
        set_rt(rt.clone());
        let fr24 = Fr24::new().map_err(|e| format!("failed to build http client: {e}"))?;
        let backend = Backend::with(fr24, dirs, rt, Notifier::new(events, wake));
        backend.inner.dirs.share_cache();
        // every Swedish train's position as it comes, while the map shows them
        trafikverket::start_operator_streams(backend.inner.notifier.clone());
        // and the trains in view, close up, as live as a followed one
        trafikverket::start_view_streams(backend.inner.notifier.clone());
        // log in with the login saved on the keys page, else with credentials
        // from the environment or the fr24 config file, if any; until then
        // (or without them) requests are anonymous
        let saved = keys::load(&backend.inner.dirs);
        backend.log_in(saved);
        Ok(backend)
    }

    /// A backend around `fr24` that has started nothing.
    fn with(fr24: Fr24, dirs: Dirs, rt: Handle, notifier: Notifier) -> Backend {
        Backend {
            inner: Arc::new(Inner {
                fr24: RwLock::new(Arc::new(fr24)),
                follow: Mutex::new(None),
                reference: tokio::sync::Mutex::new(None),
                runways: tokio::sync::Mutex::new(None),
                colors: tokio::sync::Mutex::new(None),
                radar: tokio::sync::Mutex::new(HashMap::new()),
                radar_fetching: Mutex::new(HashSet::new()),
                opensky: tokio::sync::Mutex::new(opensky::Cache::default()),
                dirs,
                rt,
                notifier,
            }),
        }
    }

    /// The launch login, in the background: a sign-in or sign-out made meanwhile wins.
    fn log_in(&self, saved: keys::Keys) {
        let inner = self.inner.clone();
        self.inner.rt.spawn(async move {
            let before = inner.client();
            let mut fr24 = Fr24::clone(&before);
            let creds = (!saved.fr24_email.is_empty() && !saved.fr24_password.is_empty()).then_some(
                fr24::auth::Credentials::UsernamePassword {
                    username: saved.fr24_email,
                    password: saved.fr24_password,
                },
            );
            match fr24.login(creds).await {
                Ok(Some(_)) => {
                    // a sign-in or sign-out made meanwhile wins
                    let swapped = {
                        let mut slot = inner.fr24.write().unwrap();
                        let swap = Arc::ptr_eq(&slot, &before);
                        if swap {
                            *slot = Arc::new(fr24);
                        }
                        swap
                    };
                    if swapped {
                        // the session shown has changed
                        inner.notifier.wake();
                    }
                }
                Ok(None) => {}
                Err(e) => eprintln!("fr24 login failed, continuing anonymously: {e}"),
            }
        });
    }

    /// The Flightradar24 client in use now (signed in or not).
    pub fn client(&self) -> Arc<Fr24> {
        self.inner.client()
    }

    /// The folders it reads and writes.
    pub fn dirs(&self) -> &Dirs {
        &self.inner.dirs
    }

    /// The runtime its background work runs on.
    pub fn rt(&self) -> &Handle {
        &self.inner.rt
    }

    /// The UI's wake, for streams started outside it (`trains::follow`).
    pub fn waker(&self) -> Wake {
        self.inner.notifier.waker()
    }

    // ------------------------------------------------------------ Flightradar24

    /// Live flights within `area` (or the whole world), optionally only the
    /// given categories (`fr24::proto::common::Service` values).
    pub async fn live_flights(&self, area: Option<BoundingBox>, services: Option<Vec<i32>>) -> CmdResult<LiveSnapshot> {
        let fr24 = self.client();
        let start = Instant::now();
        let area = area.unwrap_or(fr24::bbox::WORLD);
        let mut template = LiveFeedParams {
            limit: fr24.live_feed_limit(),
            ..LiveFeedParams::new(area)
        };
        if let Some(services) = services.filter(|s| !s.is_empty()) {
            template.services = services;
        }
        // vertical speed is a signed-in field; anonymous requests get four, all used
        let signed_in = fr24.auth().is_some();
        if signed_in {
            template.fields.push(fr24::grpc::LiveFeedField::Vspeed);
        }
        let world = fr24
            .live_feed_area(&template, area, WORLD_CONCURRENCY)
            .await;
        if world.flights.is_empty()
            && let Some((_, e)) = world.errors.first()
        {
            return Err(e.to_string());
        }
        Ok(LiveSnapshot {
            server_time_ms: world.server_time_ms,
            requests: world.requests,
            errors: world
                .errors
                .iter()
                .map(|(b, e)| format!("{:.0}..{:.0}: {e}", b.west, b.east))
                .collect(),
            flights: world
                .flights
                .into_iter()
                .map(|f| {
                    let vspeed = f.extra_info.as_ref().map(|e| e.vspeed);
                    LiveFlight {
                        vspeed: if signed_in { vspeed } else { None },
                        ..live_flight(f)
                    }
                })
                .collect(),
            elapsed_ms: start.elapsed().as_millis() as u64,
            authenticated: fr24.auth().is_some(),
        })
    }

    pub async fn flight_details(&self, flight_id: u32) -> CmdResult<FlightDetailsRecord> {
        let response = self
            .client()
            .flight_details(&FlightDetailsParams::new(flight_id))
            .await
            .map_err(to_string)?;
        Ok(FlightDetailsRecord::from(&response))
    }

    /// Streams live updates for a flight. Replaces any previously followed flight.
    pub fn follow_flight(&self, flight_id: u32) -> Following<FollowEvent> {
        let fr24 = self.client();
        self.follow(move |tx| follow_task(fr24, flight_id, tx))
    }

    /// Starts `task` as the followed flight's stream, in place of the one before.
    fn follow<F, Fut>(&self, task: F) -> Following<FollowEvent>
    where
        F: FnOnce(FollowSender<FollowEvent>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let following = Following::spawn_on(&self.inner.rt, self.waker(), task);
        if let Some(previous) = self.inner.follow.lock().unwrap().replace(following.abort_handle()) {
            previous.abort();
        }
        following
    }

    pub fn unfollow_flight(&self) {
        if let Some(task) = self.inner.follow.lock().unwrap().take() {
            task.abort();
        }
    }

    /// The most tracked flights right now.
    pub async fn top_flights(&self) -> CmdResult<Vec<TopFlightRecord>> {
        let response = self
            .client()
            .top_flights(&TopFlightsParams { limit: 10 })
            .await
            .map_err(to_string)?;
        Ok(response
            .scoreboard_list
            .iter()
            .map(TopFlightRecord::from)
            .collect())
    }

    /// Search for live flights, airports, operators, schedules and aircraft.
    pub async fn search(&self, query: &str) -> CmdResult<Vec<FindEntry>> {
        let query = query.trim();
        if query.len() < 2 {
            return Ok(Vec::new());
        }
        let find = self
            .client()
            .find(&FindParams {
                query: query.to_owned(),
                limit: 20,
            })
            .await
            .map_err(to_string)?;
        Ok(find.results)
    }

    /// Airlines and airports, from a disk cache refreshed weekly.
    pub async fn reference_data(&self) -> CmdResult<Arc<reference::ReferenceData>> {
        let mut slot = self.inner.reference.lock().await;
        if let Some(data) = slot.as_ref() {
            return Ok(data.clone());
        }
        let path = self.inner.dirs.cache_file("reference.json");
        let data = Arc::new(reference::load(&self.client(), &path).await?);
        *slot = Some(data.clone());
        Ok(data)
    }

    /// The runways of the given airports (ICAO codes), from a disk cache of
    /// OurAirports' list refreshed weekly. Airports it doesn't know are left out.
    pub async fn airport_runways(&self, airports: Vec<String>) -> CmdResult<HashMap<String, Vec<runways::Runway>>> {
        let data = {
            let mut slot = self.inner.runways.lock().await;
            match slot.as_ref() {
                Some(data) => data.clone(),
                None => {
                    let path = self.inner.dirs.cache_file("runways.json");
                    let http = self.client().http().clone();
                    let data = Arc::new(runways::load(&http, &path).await?);
                    *slot = Some(data.clone());
                    data
                }
            }
        };
        Ok(airports
            .into_iter()
            .take(16)
            .filter_map(|code| {
                let code = code.trim().to_ascii_uppercase();
                let list = data.airports.get(&code)?.clone();
                Some((code, list))
            })
            .collect())
    }

    /// Arrivals or departures at an airport (IATA code), one page at a time.
    pub async fn airport_board(&self, code: &str, mode: board::Mode, page: Option<u32>) -> CmdResult<board::Board> {
        let params = AirportListParams {
            page: page.unwrap_or(1),
            limit: 25,
            ..AirportListParams::new(code.trim().to_uppercase(), board::api(mode))
        };
        let response = self
            .client()
            .airport_list(&params)
            .await
            .map_err(to_string)?;
        Ok(board::parse(&response, mode))
    }

    /// The latest flights flown by an aircraft, newest first.
    pub async fn aircraft_history(&self, registration: &str) -> CmdResult<Vec<FlightListRecord>> {
        let params = FlightListParams {
            limit: 10,
            ..FlightListParams::new(FlightListQuery::Reg(registration.trim().to_owned()))
        };
        let response = self
            .client()
            .flight_list(&params)
            .await
            .map_err(to_string)?;
        Ok(FlightListRecord::all(&response))
    }

    /// Livery colours derived from airline logos, per IATA code. Unknown codes
    /// are fetched (at most 40 per call) and remembered on disk.
    pub async fn airline_colors(&self, iatas: Vec<String>) -> CmdResult<HashMap<String, Option<liveries::LogoColors>>> {
        let path = self.inner.dirs.cache_file("livery-colors-2.json");
        let mut slot = self.inner.colors.lock().await;
        let cache = slot.get_or_insert_with(|| liveries::read_cache(&path));
        let wanted: Vec<String> = iatas
            .into_iter()
            .filter(|code| liveries::is_iata(code))
            .take(40)
            .collect();
        let missing: Vec<String> = wanted
            .iter()
            .filter(|code| !cache.contains_key(*code))
            .cloned()
            .collect();
        if !missing.is_empty() {
            let fr24 = self.client();
            let fetched: Vec<Option<(String, Option<liveries::LogoColors>)>> =
                futures_util::stream::iter(missing)
                    .map(|code| {
                        let http = fr24.http().clone();
                        async move { liveries::fetch(&http, &code).await.ok().map(|colors| (code, colors)) }
                    })
                    .buffer_unordered(6)
                    .collect()
                    .await;
            cache.extend(fetched.into_iter().flatten());
            liveries::write_cache(&path, cache);
        }
        Ok(wanted
            .into_iter()
            .map(|code| {
                let colors = cache.get(&code).copied().flatten();
                (code, colors)
            })
            .collect())
    }

    /// The flight an aircraft is on now, as tracked by the OpenSky Network.
    /// `icao24` is the six-digit hex Mode S address.
    pub async fn opensky_track(&self, icao24: &str) -> CmdResult<Vec<opensky::TrackPoint>> {
        let http = self.client().http().clone();
        let mut cache = self.inner.opensky.lock().await;
        opensky::track(&http, &mut cache, icao24).await
    }

    // ------------------------------------------------------------ session and keys

    pub fn session_info(&self) -> session::SessionInfo {
        session::session_info(&self.client())
    }

    pub async fn sign_in(&self, email: &str, password: String) -> CmdResult<session::SessionInfo> {
        let (fr24, session) = session::sign_in(&self.client(), email, password).await?;
        *self.inner.fr24.write().unwrap() = Arc::new(fr24);
        Ok(session)
    }

    pub fn sign_out(&self) -> session::SessionInfo {
        let (fr24, session) = session::sign_out(&self.client());
        *self.inner.fr24.write().unwrap() = Arc::new(fr24);
        session
    }

    /// The keys in use now.
    pub fn keys(&self) -> keys::Keys {
        keys::current()
    }

    /// Saves the keys to `keys.json` and puts them in use. Blocking file I/O.
    pub fn save_keys(&self, keys: keys::Keys) -> CmdResult<keys::Keys> {
        keys::save(&self.inner.dirs, keys)
    }

    // ------------------------------------------------------------ radar

    /// The newest 3D radar volumes for the sources visible in `area` (or
    /// everywhere, when it is `None`): NOAA MRMS over North America and DWD over
    /// central Europe, fetched at most every few minutes. Anything already
    /// cached is returned either way. The images themselves are then read with
    /// [`Backend::radar_image`].
    pub async fn radar_frame(&self, area: Option<BoundingBox>) -> CmdResult<Vec<radar::VolumeInfo>> {
        let state = &self.inner;
        let http = state.client().http().clone();
        let stale: Vec<&'static str> = {
            let held = state.radar.lock().await;
            RADAR_SOURCES
                .iter()
                .filter(|(source, bounds)| {
                    in_view(bounds, area.as_ref())
                        && held
                            .get(source)
                            .is_none_or(|v| v.fetched.elapsed() >= radar::MAX_AGE)
                })
                .map(|(source, _)| *source)
                .collect()
        };
        // the frontend polls on a timer without waiting for the previous call, and
        // a fetch can outlast one tick, so claim each source for the duration
        let claimed: Vec<&'static str> = {
            let mut fetching = state.radar_fetching.lock().unwrap();
            stale
                .into_iter()
                .filter(|source| fetching.insert(source))
                .collect()
        };
        if !claimed.is_empty() {
            // the two sources are independent, so fetch them side by side
            let fetched = futures_util::future::join_all(claimed.iter().map(|&source| {
                let http = http.clone();
                async move {
                    let volume = match source {
                        "dwd" => dwd::fetch(&http).await,
                        _ => mrms::fetch(&http).await,
                    };
                    (source, volume)
                }
            }))
            .await;
            {
                let mut held = state.radar.lock().await;
                for (source, volume) in fetched {
                    match volume {
                        Ok(volume) => {
                            held.insert(source, Arc::new(volume));
                        }
                        // keep showing the previous volume if a refresh fails
                        Err(e) => eprintln!("{source} radar unavailable: {e}"),
                    }
                }
            }
            let mut fetching = state.radar_fetching.lock().unwrap();
            for source in &claimed {
                fetching.remove(source);
            }
        }
        let held = state.radar.lock().await;
        // only a fetch we actually ran and that produced nothing is a failure;
        // a view with no radar coverage legitimately has nothing to show
        if held.is_empty() && !claimed.is_empty() {
            return Err("no radar volume could be loaded".into());
        }
        Ok(held.values().map(|v| v.info()).collect())
    }

    /// One layer of a source's current volume: `ground` or a height in km such as
    /// `3` (isoband shapes, see `radar::contour`), or `coverage` (one byte per
    /// mask cell).
    pub async fn radar_image(&self, source: &str, name: &str) -> CmdResult<Vec<u8>> {
        let volume = self
            .inner
            .radar
            .lock()
            .await
            .get(source)
            .cloned()
            .ok_or("no such radar source")?;
        let bytes = volume.image(name).ok_or("no image by that name")?;
        Ok(bytes)
    }

    // ------------------------------------------------------------ export

    /// Writes an exported file (built by the caller) to the Downloads folder
    /// without overwriting anything there; where it went. Blocking file I/O.
    pub fn save_export(&self, file_name: &str, contents: &str) -> CmdResult<PathBuf> {
        export::save(self.inner.dirs.downloads.as_deref(), file_name, contents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn counter() -> (Arc<AtomicUsize>, Wake) {
        let n = Arc::new(AtomicUsize::new(0));
        let m = n.clone();
        (n, Arc::new(move || {
            m.fetch_add(1, Ordering::SeqCst);
        }))
    }

    #[test]
    fn a_notifier_queues_the_event_and_wakes_the_ui() {
        let (woken, wake) = counter();
        let (tx, rx) = std::sync::mpsc::channel();
        let notifier = Notifier::new(tx, wake);
        assert!(notifier.clone().notify(BackendEvent::TrainsChanged));
        assert!(notifier.notify(BackendEvent::UpdateReady { version: "1.2.3".into() }));
        assert_eq!(rx.try_recv(), Ok(BackendEvent::TrainsChanged));
        assert_eq!(rx.try_recv(), Ok(BackendEvent::UpdateReady { version: "1.2.3".into() }));
        assert_eq!(woken.load(Ordering::SeqCst), 2);
        notifier.wake();
        assert_eq!(woken.load(Ordering::SeqCst), 3);
        // the UI gone: nothing to queue on, said so
        drop(rx);
        assert!(!notifier.notify(BackendEvent::TrainsChanged));
    }

    #[tokio::test]
    async fn a_following_delivers_in_order_and_wakes_after_each() {
        let (woken, wake) = counter();
        let mut following = Following::spawn_on(&Handle::current(), wake, |tx| async move {
            for i in 0..3 {
                tx.send(i).unwrap();
            }
        });
        let mut got = Vec::new();
        while let Some(v) = following.rx.recv().await {
            got.push(v);
        }
        assert_eq!(got, [0, 1, 2]);
        assert_eq!(woken.load(Ordering::SeqCst), 3);
        assert_eq!(following.try_recv(), None);
    }

    #[tokio::test]
    async fn dropping_a_following_aborts_its_task() {
        let (_, wake) = counter();
        let (ended_tx, ended_rx) = tokio::sync::oneshot::channel::<()>();
        let following = Following::<u32>::spawn_on(&Handle::current(), wake, |tx| async move {
            // held until the task is dropped, which closes the other end
            let _held = ended_tx;
            tx.send(1).unwrap();
            std::future::pending::<()>().await;
        });
        let task = following.abort_handle();
        drop(following);
        tokio::time::timeout(Duration::from_secs(5), ended_rx).await.expect("the task was aborted").unwrap_err();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !task.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn sending_fails_once_the_receiver_is_gone() {
        let (woken, wake) = counter();
        let (tx, rx) = unbounded_channel();
        let sender = FollowSender { tx, wake };
        assert_eq!(sender.send(1), Ok(()));
        drop(rx);
        assert!(sender.is_closed());
        assert_eq!(sender.send(2), Err(2));
        assert_eq!(woken.load(Ordering::SeqCst), 1, "no wake for an event nobody gets");
    }

    #[test]
    fn desktop_folders_are_named_after_the_app() {
        // CI containers may have no home at all: then there is nothing to check
        let Some(dirs) = Dirs::desktop() else { return };
        assert!(dirs.cache.ends_with(APP_ID) && dirs.config.ends_with(APP_ID));
        assert_eq!(dirs.cache_file("reference.json"), dirs.cache.join("reference.json"));
        assert_eq!(dirs.config_file("keys.json"), dirs.config.join("keys.json"));
    }

    #[tokio::test]
    async fn the_runtime_is_the_callers_until_one_is_set() {
        // the first use inside a runtime takes that runtime, unless a test has set one (only
        // `lasting_rt`, which outlives every test, is ever set)
        let n = rt().spawn(async { 7 }).await.unwrap();
        assert_eq!(n, 7);
    }

    fn bbox(south: f32, north: f32, west: f32, east: f32) -> BoundingBox {
        BoundingBox {
            south,
            north,
            west,
            east,
        }
    }

    #[test]
    fn buffered_positions_keep_only_sane_increasing_times() {
        use fr24::proto::common::{PositionBuffer, RecentPosition};
        let at = |delta_ms| RecentPosition { delta_lat: 1, delta_lon: 2, delta_ms };
        let buffer = PositionBuffer {
            recent_positions_list: vec![at(2000), at(1500), at(4000), at(4_294_961_250), at(6000)],
        };
        let kept: Vec<i32> = buffered_positions(Some(&buffer)).iter().map(|p| p[2]).collect();
        assert_eq!(kept, [2000, 4000, 6000]);
        assert!(buffered_positions(None).is_empty());
    }

    #[test]
    fn only_sources_in_view_are_fetched() {
        let mrms = RADAR_SOURCES[0].1;
        let dwd = RADAR_SOURCES[1].1;
        // no view at all means the whole globe
        assert!(in_view(&mrms, None) && in_view(&dwd, None));
        // central Europe sees DWD only, the US plains MRMS only
        let europe = bbox(47.0, 54.0, 5.0, 15.0);
        assert!(in_view(&dwd, Some(&europe)) && !in_view(&mrms, Some(&europe)));
        let plains = bbox(35.0, 45.0, -105.0, -95.0);
        assert!(in_view(&mrms, Some(&plains)) && !in_view(&dwd, Some(&plains)));
        // the south Pacific sees neither
        let pacific = bbox(-40.0, -10.0, -150.0, -120.0);
        assert!(!in_view(&mrms, Some(&pacific)) && !in_view(&dwd, Some(&pacific)));
    }

    #[test]
    fn views_across_the_antimeridian() {
        let mrms = RADAR_SOURCES[0].1;
        let dwd = RADAR_SOURCES[1].1;
        // Alaska to Japan: west of the dateline, so no US east of -130
        let far_north = bbox(40.0, 60.0, 140.0, -140.0);
        assert!(!in_view(&mrms, Some(&far_north)) && !in_view(&dwd, Some(&far_north)));
        // a wrap wide enough to reach back over the CONUS grid
        let wide = bbox(20.0, 55.0, 140.0, -100.0);
        assert!(in_view(&mrms, Some(&wide)) && !in_view(&dwd, Some(&wide)));
    }

    /// Folders of its own for one test, removed when it ends.
    struct TempDirs(Dirs);

    impl TempDirs {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("rustradar-backend-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            Self(Dirs { cache: root.join("cache"), config: root.join("config"), downloads: Some(root.join("downloads")) })
        }
    }

    impl Drop for TempDirs {
        fn drop(&mut self) {
            if let Some(root) = self.0.cache.parent() {
                let _ = std::fs::remove_dir_all(root);
            }
        }
    }

    /// A runtime that outlives every test: one a test runtime's end would take
    /// down must not become [`rt`], which outlives them all.
    fn lasting_rt() -> Handle {
        static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        RT.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap()
        })
        .handle()
        .clone()
    }

    /// A backend that has started nothing (no streams, no login).
    fn quiet_backend(dirs: Dirs, rt: Handle) -> (Backend, std::sync::mpsc::Receiver<BackendEvent>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (_, wake) = counter();
        (Backend::with(Fr24::new().unwrap(), dirs, rt, Notifier::new(tx, wake)), rx)
    }

    #[test]
    fn keys_are_saved_in_the_config_folder_and_read_at_start() {
        // the only test that sets the keys in use, which are one process-wide cell
        let temp = TempDirs::new("keys");
        let (backend, _events) = quiet_backend(temp.0.clone(), lasting_rt());
        // no Flightradar24 login here: starting would sign in with it
        let keys = keys::Keys { trafiklab_gtfs: " gtfs \n".into(), trafiklab_oxyfi: "oxyfi".into(), ..Default::default() };
        let saved = backend.save_keys(keys).unwrap();
        assert_eq!(saved.trafiklab_gtfs, "gtfs");
        assert_eq!(backend.keys(), saved);
        let file = temp.0.config.join("keys.json");
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(json["trafiklabGtfs"], "gtfs");
        assert_eq!(json["fr24Password"], "");
        assert!(!temp.0.config.join("keys.json.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }

        // a new start reads them back
        *keys::cell().write().unwrap() = keys::Keys::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        let started = Backend::start(temp.0.clone(), lasting_rt(), tx, Arc::new(|| {})).unwrap();
        assert_eq!(started.keys(), saved);
        assert_eq!(started.dirs(), &temp.0);
    }

    #[test]
    fn exports_go_to_the_downloads_folder() {
        let temp = TempDirs::new("export");
        let (backend, _events) = quiet_backend(temp.0.clone(), lasting_rt());
        let first = backend.save_export("BAW117 LHR-JFK.kml", "<kml/>").unwrap();
        assert_eq!(first, temp.0.downloads.clone().unwrap().join("BAW117 LHR-JFK.kml"));
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "<kml/>");
        let second = backend.save_export("BAW117 LHR-JFK.kml", "<kml/>").unwrap();
        assert!(second.ends_with("BAW117 LHR-JFK (2).kml"));
        // phones have no Downloads folder
        let (phone, _events) = quiet_backend(Dirs { downloads: None, ..temp.0.clone() }, lasting_rt());
        assert_eq!(phone.save_export("a.kml", "").unwrap_err(), "No Downloads folder to save to");
    }

    #[tokio::test]
    async fn radar_images_need_a_volume() {
        let temp = TempDirs::new("radar");
        let (backend, _events) = quiet_backend(temp.0.clone(), Handle::current());
        assert_eq!(backend.radar_image("mrms", "3").await.unwrap_err(), "no such radar source");
        // a view nothing covers fetches nothing, and that is no failure
        let pacific = bbox(-40.0, -10.0, -150.0, -120.0);
        assert!(backend.radar_frame(Some(pacific)).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn signing_out_swaps_the_client_for_every_clone() {
        let temp = TempDirs::new("session");
        let (backend, _events) = quiet_backend(temp.0.clone(), Handle::current());
        let other = backend.clone();
        let before = backend.client();
        let session = backend.sign_out();
        assert!(!session.authenticated && session.expires.is_none());
        assert!(!Arc::ptr_eq(&before, &other.client()), "a clone sees the new client");
        assert!(!other.session_info().authenticated);
    }

    #[tokio::test]
    async fn following_a_flight_replaces_the_one_before() {
        let temp = TempDirs::new("follow");
        let (backend, _events) = quiet_backend(temp.0.clone(), Handle::current());
        // streams that never end by themselves (and never reach the network)
        let first = backend.follow(|_| std::future::pending());
        let second = backend.follow(|_| std::future::pending());
        let ended = |f: &Following<FollowEvent>| {
            let task = f.abort_handle();
            async move {
                tokio::time::timeout(Duration::from_secs(5), async {
                    while !task.is_finished() {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .is_ok()
            }
        };
        assert!(ended(&first).await, "the first flight is no longer followed");
        assert!(!second.is_finished());
        backend.unfollow_flight();
        assert!(ended(&second).await, "nor is any once unfollowed");
    }
}
