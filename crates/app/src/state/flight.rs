//! The selected flight (item A8; doc 06 §3.5): the selection and its follow stream with its
//! states, the row shown for it (`selected`), OpenSky's earlier track merged into the trail,
//! adsb.lol's measured data handed to the motion model, the route worked out for it, and the camera
//! lock's inputs. A port of `+page.svelte`'s `select`, `startFollow`, `followEnded`,
//! `toggleFollow`, the `selected`, `merged` and `route` deriveds and the OpenSky, adsb.lol, auto-resume
//! and "aircraft off" effects (+page:118-128, 277-413, 940-1068, 1103-1108, 1196-1207).
//!
//! **Two steps.** [`Flight::poll`] runs in [`AppState::poll`] each frame: it takes the follow
//! stream's events, the OpenSky, adsb.lol and runway answers, starts the requests whose timer is
//! due, and asks for a scene refresh when anything arrived. What needs the aircraft feed or the
//! camera (`byId`, the motion model, the camera's lock) runs in [`AppState::sync_flight`]: the row
//! shown for the selection, the follow stream's "last seen" when it ends, the auto-resume, the
//! adsb.lol query and its match, the route, and the camera's `set_selected` and `set_follow`. The
//! flight scene builder calls it at the start of each scene refresh (`scene_builder::flight`),
//! which the poll forces whenever something arrived, so both run in the same frame, as the page's
//! effects run after the answer that set them off; [`AppState::select_flight`] and the other UI
//! asks call it too. (`AppState::poll` does not call it; were it to, after `self.flight.poll`, a
//! second call in the frame would change nothing, and the camera would get a stream's relock in
//! the same frame instead of the next: see the deviations.)
//!
//! **Selecting** ([`AppState::select_flight`], `select(id, {hint, fly, follow, fromBoard})`):
//! - the same id without `fly`: a flight whose stream ended is asked for again (another click
//!   asks whether it is live), else nothing;
//! - otherwise the selection, the hint, `follow` and `fromBoard` are set, the details dropped; no
//!   id stops the stream (`unfollowFlight`); with `fly` the camera flies to the known position now
//!   (the feed's row, else the hint), or when the first details arrive; then the stream starts. A
//!   click on the map selects without flying; only the panel's Follow or C lock the camera.
//! - the follow stream ([`FollowState`]): `connecting` (or `reconnecting` when resuming); details
//!   → `live` (the camera locks again if it was locked when the stream ended, and a pending fly-to
//!   flies); an error → `reconnecting`; ended (or no stream at all: no backend) → `ended`, with the
//!   newest report then shown kept as "last seen", the camera let go and remembered for a resume.
//! - auto-resume: while ended, a newer report of the selected id in the feed than the last seen
//!   asks for the stream again, as `reconnecting`.
//!
//! **The row shown** ([`Flight::selected`]): the feed's row (or the hint) unless the stream's
//! report is newer, then a row made from the stream (position, track, altitude, ground speed,
//! ground flag, time, callsign) with the feed's names as fallbacks. The aircraft scene builder
//! draws it in place of the feed's row, so the model and the trail's tip agree.
//!
//! **OpenSky.** Once the details give the aircraft's Mode S address (six lower-case hex digits),
//! its OpenSky track is asked for at once and every [`OPENSKY_REFRESH_MS`]; [`Flight::details`]
//! is the details with OpenSky's earlier points in front of the trail (`mergeTrail`).
//!
//! **adsb.lol.** With the setting on and a flight selected with an address or a callsign, adsb.lol
//! is asked every [`ADSB_POLL_MS`] × `min(2^failures, 16)` (the next ask after each answer); a
//! match (`matchAdsb` against the row shown) is kept with when it was seen on this clock and handed
//! to the motion model (`motion.measure`). [`Flight::measured`] shows it for [`ADSB_SHOW_MS`] after
//! that. Deselecting or turning the setting off hands the motion model none.
//!
//! **The route** ([`Flight::route`]): `routeFor(selected, details, flights)` with the reference
//! data and the runways known so far, made again when one of them changes. The runways an estimate
//! asks for are downloaded one airport at a time (`airportRunways([code])`); a failed one is asked
//! for again a minute later, at the next route that wants it (`reference.svelte.ts`).
//!
//! **Map marks** held here until their owners exist: the focused airport (the board's code, item
//! U13: [`Flight::set_focus_airport`]) and the searched place (item U8: [`Flight::set_place`]),
//! which the flight scene builder draws.
//!
//! **Sources** ([`FlightFeeds`]): the backend, the `--fixture` folder, a test's, or none (no
//! backend: a stream "fails" at once, as `followFlight` rejects outside Tauri, and nothing is
//! asked). The fixture folder's files:
//! - `flight_details/{id}.json`: the `FlightDetails` the stream sends once (then it stays live); a
//!   missing file ends the stream;
//! - `opensky/{icao24}.json`: the OpenSky track (`OpenSkyPoint`s);
//! - `adsb/hex-{icao24}.json`, `adsb/callsign-{CALLSIGN}.json`: adsb.lol's answer (`AdsbSnapshot`);
//! - `runways.json`: runways by ICAO code, as `airport_runways` answers.
//!
//! A missing file is a failed request.
//!
//! Deviations from today's app:
//! - A follow stream that cannot start (no backend) ends at once, not a microtask later.
//! - A stream replaced by a new one (another selection, a resume) is dropped with the events it
//!   had not delivered; today a late event of the same id from the old channel still lands.
//! - The camera gets the row shown and `following` when they change, at the end of the step that
//!   changed them; the camera's step ran before it in that frame, so the lock follows one frame
//!   later.
//! - Not ported yet, for want of their owners: closing the ship and the train on a selection
//!   (items H1, T1), closing the airport board when the aircraft leave the map (U13), the window's
//!   visibility (adsb.lol is asked while the app runs).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, OnceLock};

use rustradar_backend::{Backend, FollowEvent, Following, adsblol};
use rustradar_core::adsb::{FlightKey, match_adsb};
use rustradar_core::model::{AdsbAircraft, AdsbSnapshot, FlightDetails, LiveFlight, OpenSkyPoint, ReferenceData, Runway, f32_as_js};
use rustradar_core::motion::aircraft::{Measured, Motion};
use rustradar_core::reference::{Reference, Runways};
use rustradar_core::routes::{Route, merge_trail, route_for};
use rustradar_core::settings::shown_kinds;
use rustradar_scene::controller::Controller;

use super::aircraft::{Aircraft, Answer, NO_BACKEND};
use super::{AppState, Cx};
use crate::requests::Requests;

/// `OPENSKY_REFRESH_MS`: the OpenSky track is asked for again this often while the flight is open.
pub const OPENSKY_REFRESH_MS: f64 = 300_000.0;
/// `ADSB_POLL_MS`: adsb.lol is asked this long after its last answer, times the back-off.
pub const ADSB_POLL_MS: f64 = 3_000.0;
/// `ADSB_MAX_BACKOFF`: the back-off's largest factor (`min(2^failures, 16)`).
pub const ADSB_MAX_BACKOFF: f64 = 16.0;
/// `ADSB_SHOW_MS`: measured data is shown this long after the aircraft last reported.
pub const ADSB_SHOW_MS: f64 = 60_000.0;

/// The `--fixture` folder's stream details: `flight_details/{id}.json`.
pub const FIXTURE_DETAILS: &str = "flight_details";
/// The `--fixture` folder's OpenSky tracks: `opensky/{icao24}.json`.
pub const FIXTURE_OPENSKY: &str = "opensky";
/// The `--fixture` folder's adsb.lol answers: `adsb/hex-{icao24}.json`, `adsb/callsign-{CS}.json`.
pub const FIXTURE_ADSB: &str = "adsb";
/// The `--fixture` folder's runways by ICAO code.
pub const FIXTURE_RUNWAYS: &str = "runways.json";

/// The follow stream's state (`FollowState`, `api.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FollowState {
    #[default]
    Connecting,
    Live,
    Reconnecting,
    Ended,
}

impl FollowState {
    /// The name the flight panel shows it by.
    pub fn as_str(self) -> &'static str {
        match self {
            FollowState::Connecting => "connecting",
            FollowState::Live => "live",
            FollowState::Reconnecting => "reconnecting",
            FollowState::Ended => "ended",
        }
    }
}

/// `select`'s options.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SelectOptions {
    /// What is known of the flight before the feed has it (a search pick).
    pub hint: Option<LiveFlight>,
    /// Fly the camera to it (picks from lists, search, history, the board); a click does not.
    pub fly: bool,
    /// Lock the camera on it.
    pub follow: bool,
    /// Opened from an airport board (the panel's back button).
    pub from_board: bool,
}

/// A followed flight's events: the backend's stream, or a channel (fixtures, tests).
pub enum FollowStream {
    Backend(Following<FollowEvent>),
    Channel(Receiver<FollowEvent>),
}

impl FollowStream {
    /// The next event, if one is waiting.
    pub fn try_recv(&mut self) -> Option<FollowEvent> {
        match self {
            FollowStream::Backend(following) => following.try_recv(),
            FollowStream::Channel(rx) => rx.try_recv().ok(),
        }
    }
}

impl std::fmt::Debug for FollowStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FollowStream::Backend(_) => "FollowStream::Backend",
            FollowStream::Channel(_) => "FollowStream::Channel",
        })
    }
}

/// What adsb.lol is asked by: the Mode S address, else the callsign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdsbQuery {
    Hex(String),
    Callsign(String),
}

/// Where the selected flight's answers come from: the backend, the `--fixture` folder, or a test's.
pub trait FlightFeeds: Send {
    /// `followFlight(id, …)`: the stream, or why it could not start (`followFlight` rejected).
    fn follow_flight(&mut self, id: u32) -> Result<FollowStream, String>;
    /// `unfollowFlight()`.
    fn unfollow_flight(&mut self);
    /// `openskyTrack(icao24)`.
    fn opensky_track(&mut self, icao24: &str) -> Answer<Result<Vec<OpenSkyPoint>, String>>;
    /// `adsbHex(hex)` or `adsbCallsign(callsign)`.
    fn adsb(&mut self, query: &AdsbQuery) -> Answer<Result<AdsbSnapshot, String>>;
    /// `airportRunways(codes)`.
    fn airport_runways(&mut self, codes: Vec<String>) -> Answer<Result<HashMap<String, Vec<Runway>>, String>>;
}

/// The backend's streams and requests, on its runtime.
struct BackendFeeds {
    backend: Backend,
    requests: Requests,
}

impl FlightFeeds for BackendFeeds {
    fn follow_flight(&mut self, id: u32) -> Result<FollowStream, String> {
        Ok(FollowStream::Backend(self.backend.follow_flight(id)))
    }

    fn unfollow_flight(&mut self) {
        self.backend.unfollow_flight();
    }

    fn opensky_track(&mut self, icao24: &str) -> Answer<Result<Vec<OpenSkyPoint>, String>> {
        let (backend, icao24) = (self.backend.clone(), icao24.to_owned());
        self.requests.request(async move { backend.opensky_track(&icao24).await }).into()
    }

    fn adsb(&mut self, query: &AdsbQuery) -> Answer<Result<AdsbSnapshot, String>> {
        let query = query.clone();
        self.requests
            .request(async move {
                match query {
                    AdsbQuery::Hex(hex) => adsblol::hex(&hex).await,
                    AdsbQuery::Callsign(callsign) => adsblol::callsign(&callsign).await,
                }
            })
            .into()
    }

    fn airport_runways(&mut self, codes: Vec<String>) -> Answer<Result<HashMap<String, Vec<Runway>>, String>> {
        let backend = self.backend.clone();
        self.requests.request(async move { backend.airport_runways(codes).await }).into()
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

impl FlightFeeds for FixtureFeeds {
    fn follow_flight(&mut self, id: u32) -> Result<FollowStream, String> {
        let (tx, rx) = mpsc::channel();
        // the receiver is right here: the sends cannot fail
        let _ = match read_json!(FlightDetails, self.dir, format!("{FIXTURE_DETAILS}/{id}.json")) {
            Ok(details) => tx.send(FollowEvent::Details(Box::new(details))),
            Err(e) => tx.send(FollowEvent::Ended(e)),
        };
        Ok(FollowStream::Channel(rx))
    }

    fn unfollow_flight(&mut self) {}

    fn opensky_track(&mut self, icao24: &str) -> Answer<Result<Vec<OpenSkyPoint>, String>> {
        Answer::ready(read_json!(Vec<OpenSkyPoint>, self.dir, format!("{FIXTURE_OPENSKY}/{icao24}.json")))
    }

    fn adsb(&mut self, query: &AdsbQuery) -> Answer<Result<AdsbSnapshot, String>> {
        let name = match query {
            AdsbQuery::Hex(hex) => format!("{FIXTURE_ADSB}/hex-{hex}.json"),
            AdsbQuery::Callsign(callsign) => format!("{FIXTURE_ADSB}/callsign-{callsign}.json"),
        };
        Answer::ready(read_json!(AdsbSnapshot, self.dir, name))
    }

    fn airport_runways(&mut self, codes: Vec<String>) -> Answer<Result<HashMap<String, Vec<Runway>>, String>> {
        Answer::ready(read_json!(HashMap<String, Vec<Runway>>, self.dir, FIXTURE_RUNWAYS).map(|mut all| {
            codes.iter().filter_map(|code| all.remove_entry(&code.trim().to_ascii_uppercase())).collect()
        }))
    }
}

/// Runways by ICAO code, as `airport_runways` answers.
pub type RunwaysFound = HashMap<String, Vec<Runway>>;

/// Which feeds the selected flight uses.
enum Source {
    /// Chosen at the first poll (or selection): the fixture folder, else the backend, else none.
    Auto,
    /// None (the page without Tauri).
    Off,
    On(Box<dyn FlightFeeds>),
}

/// The searched place, marked on the map until dismissed (`place`).
#[derive(Debug, Clone, PartialEq)]
pub struct MarkedPlace {
    pub lon: f64,
    pub lat: f64,
    pub name: String,
}

/// adsb.lol's latest match for the selected aircraft (`adsb`).
#[derive(Debug, Clone, PartialEq)]
pub struct AdsbSeen {
    pub id: u32,
    pub aircraft: AdsbAircraft,
    /// When it last reported, on this machine's clock (Unix ms).
    pub seen_at: f64,
}

/// What the route was last made from.
#[derive(Debug, Clone, PartialEq)]
struct RouteKey {
    selected: LiveFlight,
    details: (u64, u64),
    feed: u64,
    reference: Option<usize>,
    runways: u64,
}

/// The selected flight: the follow stream and its states, OpenSky's track, adsb.lol's
/// measurements, the route.
pub struct Flight {
    source: Source,
    started: bool,
    /// The aircraft were on the map at the last poll (`aircraftWere`).
    aircraft_were: bool,

    // the selection (`selectedId`, `hint`, `following`, `fromBoard`, `flyPending`, `flyZoom`)
    selected_id: Option<u32>,
    hint: Option<LiveFlight>,
    following: bool,
    relock_on_resume: bool,
    from_board: bool,
    fly_pending: bool,
    fly_zoom: Option<f64>,

    // the follow stream (`followState`, `endedSeenMs`, `details`)
    stream: Option<FollowStream>,
    follow_state: FollowState,
    ended_seen_ms: f64,
    /// The stream ended and `endedSeenMs` waits for the row shown ([`AppState::sync_flight`]).
    ended_seen_pending: bool,
    details: Option<FlightDetails>,
    details_version: u64,
    /// The row shown, as of the last sync.
    selected: Option<LiveFlight>,

    // OpenSky (`openskyPoints`, the track's effect)
    opensky: Option<(String, Vec<OpenSkyPoint>)>,
    opensky_version: u64,
    /// The address the track's timer runs for, its requests out and its next tick.
    opensky_for: Option<String>,
    opensky_requests: Vec<Answer<Result<Vec<OpenSkyPoint>, String>>>,
    opensky_due: Option<f64>,
    /// `merged`: the details with OpenSky's points in front, for `(details, opensky)` versions.
    merged: Option<FlightDetails>,
    merged_added: usize,
    merged_for: (u64, u64),

    // adsb.lol (`adsbQuery`, the poll's effect, `adsb`, `measuredId`)
    adsb_key: Option<(u32, AdsbQuery)>,
    adsb_request: Option<Answer<Result<AdsbSnapshot, String>>>,
    adsb_due: Option<f64>,
    adsb_failures: u32,
    /// An answer waiting to be matched against the row shown.
    adsb_answer: Option<AdsbSnapshot>,
    adsb: Option<AdsbSeen>,
    measured_id: Option<u32>,

    // the route and the runways it asks for
    runways: Runways,
    runway_requests: Vec<(String, Answer<Result<RunwaysFound, String>>)>,
    runways_version: u64,
    route: Option<Route>,
    route_for: Option<RouteKey>,

    // map marks of other items
    place: Option<MarkedPlace>,
    focus_airport: Option<String>,

    /// The row and `following` last handed to the camera (its own start: none, not following).
    handed: (Option<LiveFlight>, bool),
}

impl Default for Flight {
    /// The feeds of the `--fixture` folder, else the backend, chosen at the first poll.
    fn default() -> Self {
        Flight::with_source(Source::Auto)
    }
}

impl std::fmt::Debug for Flight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Flight")
            .field("selected_id", &self.selected_id)
            .field("follow_state", &self.follow_state)
            .field("following", &self.following)
            .field("details", &self.details.is_some())
            .field("route", &self.route.as_ref().map(|r| r.kind))
            .finish_non_exhaustive()
    }
}

/// `details.aircraft.icao_address.toString(16).padStart(6, "0")`, when there is one (0 is none).
pub fn icao24_of(details: Option<&FlightDetails>) -> Option<String> {
    details.and_then(|d| d.aircraft.as_ref()).filter(|a| a.icao_address != 0).map(|a| format!("{:06x}", a.icao_address))
}

/// `a || b`: the first non-empty string.
fn or<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() { b } else { a }
}

/// The row shown for flight `id` (`selected`, +page:277-299): the feed's row, or `hint`, unless the
/// follow stream's report is newer; then a row made from the stream's report with the feed's names
/// as fallbacks.
pub fn selected_row(id: u32, feed: Option<&LiveFlight>, hint: Option<&LiveFlight>, details: Option<&FlightDetails>) -> Option<LiveFlight> {
    let feed = feed.or(hint);
    let Some(live) = details.and_then(|d| d.flight.as_ref()) else { return feed.cloned() };
    if let Some(feed) = feed
        && feed.timestamp_ms > live.timestamp_ms
    {
        return Some(feed.clone());
    }
    let schedule = details.and_then(|d| d.schedule.as_ref());
    let aircraft = details.and_then(|d| d.aircraft.as_ref());
    let name = |pick: fn(&LiveFlight) -> &str| feed.map_or("", pick);
    Some(LiveFlight {
        id,
        lat: live.latitude,
        lon: live.longitude,
        track: live.track,
        alt: live.altitude,
        speed: live.ground_speed,
        on_ground: live.on_ground,
        timestamp_ms: live.timestamp_ms,
        callsign: or(&live.callsign, name(|f| &f.callsign)).to_owned(),
        flight: or(name(|f| &f.flight), schedule.map_or("", |s| &s.flight_number)).to_owned(),
        reg: or(name(|f| &f.reg), aircraft.map_or("", |a| &a.reg)).to_owned(),
        typecode: or(name(|f| &f.typecode), aircraft.map_or("", |a| &a.typecode)).to_owned(),
        origin: name(|f| &f.origin).to_owned(),
        destination: name(|f| &f.destination).to_owned(),
        icon: name(|f| &f.icon).to_owned(),
        positions: Vec::new(),
        vspeed: None,
    })
}

/// adsb.lol's aircraft as the motion model takes it (`motion.measure(id, match)`: the TypeScript
/// passed the aircraft itself, whose fields `Measured` names).
pub fn measured(a: &AdsbAircraft) -> Measured {
    Measured {
        position_ms: a.position_ms.map(|v| v as f64),
        lat: a.lat,
        lon: a.lon,
        mlat: a.mlat,
        seen_ms: a.seen_ms as f64,
        on_ground: a.on_ground,
        alt_baro: a.alt_baro.map(f64::from),
        nav_qnh: a.nav_qnh.map(f32_as_js),
        gs: a.gs.map(f32_as_js),
        track: a.track.map(f32_as_js),
        true_heading: a.true_heading.map(f32_as_js),
        mag_heading: a.mag_heading.map(f32_as_js),
        roll: a.roll.map(f32_as_js),
        track_rate: a.track_rate.map(f32_as_js),
        baro_rate: a.baro_rate.map(f64::from),
        geom_rate: a.geom_rate.map(f64::from),
        tas: a.tas.map(f64::from),
        nav_altitude_mcp: a.nav_altitude_mcp.map(f64::from),
        nav_altitude_fms: a.nav_altitude_fms.map(f64::from),
        nav_heading: a.nav_heading.map(f32_as_js),
        // left out when empty
        nav_modes: (!a.nav_modes.is_empty()).then(|| a.nav_modes.clone()),
    }
}

/// No reference data yet: no airport is found (`reference.airport…` before `ready`).
fn no_reference() -> &'static Reference {
    static EMPTY: OnceLock<Reference> = OnceLock::new();
    EMPTY.get_or_init(|| Reference::new(ReferenceData::default()))
}

impl Flight {
    fn with_source(source: Source) -> Self {
        Flight {
            source,
            started: false,
            aircraft_were: false,
            selected_id: None,
            hint: None,
            following: false,
            relock_on_resume: false,
            from_board: false,
            fly_pending: false,
            fly_zoom: None,
            stream: None,
            follow_state: FollowState::Connecting,
            ended_seen_ms: 0.0,
            ended_seen_pending: false,
            details: None,
            details_version: 0,
            selected: None,
            opensky: None,
            opensky_version: 0,
            opensky_for: None,
            opensky_requests: Vec::new(),
            opensky_due: None,
            merged: None,
            merged_added: 0,
            merged_for: (0, 0),
            adsb_key: None,
            adsb_request: None,
            adsb_due: None,
            adsb_failures: 0,
            adsb_answer: None,
            adsb: None,
            measured_id: None,
            runways: Runways::new(),
            runway_requests: Vec::new(),
            runways_version: 0,
            route: None,
            route_for: None,
            place: None,
            focus_airport: None,
            handed: (None, false),
        }
    }

    /// The selected flight on `feeds` instead of the fixture folder or the backend; `None`: no
    /// source at all (streams end at once, nothing is asked).
    pub fn with_feeds(feeds: Option<Box<dyn FlightFeeds>>) -> Self {
        Flight::with_source(match feeds {
            Some(feeds) => Source::On(feeds),
            None => Source::Off,
        })
    }

    /// Resolves [`Source::Auto`]: the fixture folder, else the backend, else none.
    fn resolve(&mut self, fixture: Option<&Path>, backend: Option<&Backend>, requests: Option<&Requests>) {
        if !matches!(self.source, Source::Auto) {
            return;
        }
        self.source = if let Some(dir) = fixture {
            log::info!("selected flight's details from the fixture folder {}", dir.display());
            Source::On(Box::new(FixtureFeeds { dir: dir.to_path_buf() }))
        } else if let (Some(backend), Some(requests)) = (backend, requests) {
            Source::On(Box::new(BackendFeeds { backend: backend.clone(), requests: requests.clone() }))
        } else {
            Source::Off
        };
    }

    fn feeds(&mut self) -> Option<&mut (dyn FlightFeeds + 'static)> {
        match &mut self.source {
            Source::On(feeds) => Some(feeds.as_mut()),
            Source::Auto | Source::Off => None,
        }
    }

    // ---------------------------------------------------------------- what the rest reads

    /// The selected flight's id (`selectedId`).
    pub fn selected_id(&self) -> Option<u32> {
        self.selected_id
    }

    /// The row shown for the selection (`selected`), as of the last [`AppState::sync_flight`].
    pub fn selected(&self) -> Option<&LiveFlight> {
        self.selected.as_ref()
    }

    /// What is known of the selection before the feed has it (`hint`).
    pub fn hint(&self) -> Option<&LiveFlight> {
        self.hint.as_ref()
    }

    /// The follow stream's state (`followState`).
    pub fn follow_state(&self) -> FollowState {
        self.follow_state
    }

    /// The newest report when the stream ended (`endedSeenMs`, feed clock ms): the panel's "Last
    /// seen" while ended, else 0.
    pub fn last_seen_ms(&self) -> f64 {
        if self.follow_state == FollowState::Ended { self.ended_seen_ms } else { 0.0 }
    }

    /// The camera follows the selected aircraft (`following`).
    pub fn following(&self) -> bool {
        self.following
    }

    /// Opened from an airport board (`fromBoard`).
    pub fn from_board(&self) -> bool {
        self.from_board
    }

    /// The details as streamed (`details`).
    pub fn raw_details(&self) -> Option<&FlightDetails> {
        self.details.as_ref()
    }

    /// The details with OpenSky's earlier points in front of the trail (`shownDetails`).
    pub fn details(&self) -> Option<&FlightDetails> {
        self.merged.as_ref()
    }

    /// How many of OpenSky's points are in front of the trail (`merged.added`: the credit).
    pub fn opensky_added(&self) -> usize {
        self.merged_added
    }

    /// The aircraft's Mode S address from the details (`icao24`).
    pub fn icao24(&self) -> Option<String> {
        icao24_of(self.details.as_ref())
    }

    /// The selected flight's route (`route`), as of the last sync.
    pub fn route(&self) -> Option<&Route> {
        self.route.as_ref()
    }

    /// adsb.lol's latest match (`adsb`).
    pub fn adsb(&self) -> Option<&AdsbSeen> {
        self.adsb.as_ref()
    }

    /// The measured data shown at local time `now_ms` with the adsb setting `adsb_on` (`measured`):
    /// the match for the selected flight while it reported less than [`ADSB_SHOW_MS`] ago.
    pub fn measured(&self, now_ms: f64, adsb_on: bool) -> Option<&AdsbAircraft> {
        self.adsb
            .as_ref()
            .filter(|a| Some(a.id) == self.selected_id && adsb_on && now_ms - a.seen_at < ADSB_SHOW_MS)
            .map(|a| &a.aircraft)
    }

    /// The searched place marked on the map.
    pub fn place(&self) -> Option<&MarkedPlace> {
        self.place.as_ref()
    }

    /// Marks the searched place, or clears it (`place = p` in `pickPlace`; `null` on a new query,
    /// on Escape, and on any selection on the map). Item U8 calls it.
    pub fn set_place(&mut self, place: Option<MarkedPlace>) {
        self.place = place;
    }

    /// The airport whose board is open (`boardCode`), marked on the map.
    pub fn focus_airport(&self) -> Option<&str> {
        self.focus_airport.as_deref()
    }

    /// The airport whose board is open, or none (item U13's `boardCode`).
    pub fn set_focus_airport(&mut self, code: Option<String>) {
        self.focus_airport = code.map(|c| c.to_uppercase());
    }

    /// Lets go of the camera (`following = false`: `pickPlace`, a picked train or ship).
    pub fn set_following(&mut self, following: bool) {
        self.following = following;
    }

    // ---------------------------------------------------------------- the page's functions

    /// `select(id, opts)` with the feed's row of `id` as `known`, flying the camera with
    /// `controller` at local time `now`.
    fn select(&mut self, id: Option<u32>, opts: SelectOptions, known: Option<&LiveFlight>, controller: &mut Controller, now: f64) {
        if id == self.selected_id && !opts.fly {
            // another click on a flight that stopped reporting asks again whether it is live
            if let Some(id) = id
                && self.follow_state == FollowState::Ended
            {
                self.start_follow(id, false);
            }
            return;
        }
        self.selected_id = id;
        // (one panel at a time: the ship and the train close, items H1 and T1)
        self.details = None;
        self.details_version += 1;
        let known = known.cloned().or_else(|| opts.hint.clone());
        self.hint = opts.hint;
        self.fly_pending = false;
        self.following = opts.follow;
        self.relock_on_resume = false;
        self.from_board = opts.from_board;
        let Some(id) = id else {
            self.stream = None;
            if let Some(feeds) = self.feeds() {
                feeds.unfollow_flight();
            }
            return;
        };
        if opts.fly {
            // picks from a list fly in; a click on the map leaves the camera alone
            self.fly_zoom = None;
            match known {
                Some(f) => controller.fly_to(f32_as_js(f.lon), f32_as_js(f.lat), self.fly_zoom, now),
                None => self.fly_pending = true,
            }
        }
        self.start_follow(id, false);
    }

    /// `startFollow(id, resuming)`.
    fn start_follow(&mut self, id: u32, resuming: bool) {
        self.follow_state = if resuming { FollowState::Reconnecting } else { FollowState::Connecting };
        let started = match self.feeds() {
            Some(feeds) => feeds.follow_flight(id),
            None => Err(NO_BACKEND.to_owned()),
        };
        match started {
            Ok(stream) => self.stream = Some(stream),
            Err(e) => {
                log::debug!("following flight {id}: {e}");
                // a rejected call ends it
                self.stream = None;
                self.follow_ended();
            }
        }
    }

    /// `followEnded()`: its last values stay on show, and the camera lets go. `endedSeenMs` is
    /// taken from the row shown at the next sync.
    fn follow_ended(&mut self) {
        self.follow_state = FollowState::Ended;
        self.ended_seen_pending = true;
        // nothing moves any more to keep in frame; Follow (or C) can still lock on again
        self.relock_on_resume |= self.following;
        self.following = false;
    }

    /// The follow stream's events since the last frame, in order.
    fn take_events(&mut self, controller: &mut Controller, now: f64) -> bool {
        let mut any = false;
        while let Some(event) = self.stream.as_mut().and_then(FollowStream::try_recv) {
            any = true;
            match event {
                FollowEvent::Details(details) => {
                    self.details = Some(*details);
                    self.details_version += 1;
                    self.follow_state = FollowState::Live;
                    if self.relock_on_resume {
                        self.following = true;
                    }
                    self.relock_on_resume = false;
                    if self.fly_pending
                        && let Some(f) = self.details.as_ref().and_then(|d| d.flight.as_ref())
                    {
                        self.fly_pending = false;
                        controller.fly_to(f32_as_js(f.longitude), f32_as_js(f.latitude), self.fly_zoom, now);
                    }
                }
                FollowEvent::Error(_) => self.follow_state = FollowState::Reconnecting,
                FollowEvent::Ended(_) => self.follow_ended(),
            }
        }
        any
    }

    /// The OpenSky track's effect: a new address starts its timer (now, then every 5 min), and
    /// its answers.
    fn run_opensky(&mut self, now: f64) -> bool {
        let code = self.icao24();
        if code != self.opensky_for {
            self.opensky_for = code.clone();
            self.opensky_requests.clear();
            self.opensky_due = code.as_ref().map(|_| now);
        }
        let mut arrived = false;
        let mut kept = Vec::new();
        for request in std::mem::take(&mut self.opensky_requests) {
            match request.poll() {
                Some(Ok(points)) => {
                    if let Some(code) = &code {
                        self.opensky = Some((code.clone(), points));
                        self.opensky_version += 1;
                        arrived = true;
                    }
                }
                // no track, or OpenSky's allowance is used up: Flightradar24's trail stands
                Some(Err(_)) => {}
                None => kept.push(request),
            }
        }
        self.opensky_requests = kept;
        if let (Some(code), Some(due)) = (&code, self.opensky_due)
            && now >= due
        {
            let mut next = due + OPENSKY_REFRESH_MS;
            if next <= now {
                next = now + OPENSKY_REFRESH_MS;
            }
            self.opensky_due = Some(next);
            let code = code.clone();
            if let Some(feeds) = self.feeds() {
                let request = feeds.opensky_track(&code);
                self.opensky_requests.push(request);
            }
        }
        arrived
    }

    /// The adsb.lol poll's timer and its answers.
    fn run_adsb(&mut self, now: f64) -> bool {
        let mut arrived = false;
        if let Some(answer) = self.adsb_request.as_ref().and_then(Answer::poll) {
            self.adsb_request = None;
            match answer {
                Ok(snapshot) => {
                    self.adsb_answer = Some(snapshot);
                    self.adsb_failures = 0;
                    arrived = true;
                }
                // unreachable or resting: Flightradar24's data stands alone meanwhile
                Err(_) => self.adsb_failures += 1,
            }
            let factor = 2f64.powi(self.adsb_failures.min(30) as i32).min(ADSB_MAX_BACKOFF);
            self.adsb_due = Some(now + ADSB_POLL_MS * factor);
        }
        if let (Some((_, query)), Some(due), None) = (&self.adsb_key, self.adsb_due, &self.adsb_request)
            && now >= due
        {
            let query = query.clone();
            self.adsb_due = None;
            if let Some(feeds) = self.feeds() {
                let request = feeds.adsb(&query);
                self.adsb_request = Some(request);
            }
        }
        arrived
    }

    /// The runway downloads' answers.
    fn run_runways(&mut self, now: f64) -> bool {
        let mut arrived = false;
        let mut kept = Vec::new();
        for (code, request) in std::mem::take(&mut self.runway_requests) {
            match request.poll() {
                Some(found) => {
                    // offline, or the download failed: asked again a minute later
                    self.runways.settled(&code, found.as_ref().ok(), now as i64);
                    self.runways_version += 1;
                    arrived = true;
                }
                None => kept.push((code, request)),
            }
        }
        self.runway_requests = kept;
        arrived
    }

    /// The merged details (`merged`), made again when the details or OpenSky's track changed.
    fn merge(&mut self) {
        let key = (self.details_version, self.opensky_version);
        if key == self.merged_for {
            return;
        }
        self.merged_for = key;
        let icao24 = self.icao24();
        (self.merged, self.merged_added) = match &self.details {
            None => (None, 0),
            Some(details) => {
                let own: &[OpenSkyPoint] = match &self.opensky {
                    Some((code, points)) if Some(code) == icao24.as_ref() => points,
                    _ => &[],
                };
                let merged = merge_trail(details.trail.clone(), own);
                if merged.added > 0 {
                    (Some(FlightDetails { trail: merged.trail, ..details.clone() }), merged.added)
                } else {
                    (Some(details.clone()), 0)
                }
            }
        };
    }

    /// This frame's step (see [`AppState::poll`] for the order, and the module documentation).
    pub fn poll(&mut self, cx: &mut Cx<'_>) {
        let now = cx.clocks.local_ms;
        let settings = &cx.persisted.settings;
        let on = shown_kinds(settings.mode, &settings.also_show).aircraft;
        if !self.started {
            self.started = true;
            self.aircraft_were = on;
            self.resolve(cx.startup.fixture.as_deref(), cx.backend, cx.requests);
            // on mount: a stream left over from before is let go of
            if let Some(feeds) = self.feeds() {
                feeds.unfollow_flight();
            }
        }
        // the aircraft off the map: the open flight closes (the stream and the camera's lock with it)
        if on != self.aircraft_were {
            self.aircraft_were = on;
            if !on && self.selected_id.is_some() {
                self.select(None, SelectOptions::default(), None, cx.controller, now);
                cx.wants.scene = true;
            }
        }
        let mut changed = self.take_events(cx.controller, now);
        changed |= self.run_opensky(now);
        changed |= self.run_adsb(now);
        changed |= self.run_runways(now);
        if changed {
            cx.wants.scene = true;
        }
        for due in [self.opensky_due, self.adsb_due].into_iter().flatten() {
            cx.wants.wake_at(due);
        }
    }

    /// The page's effects that read the feed (see [`AppState::sync_flight`]).
    fn sync(&mut self, aircraft: &Aircraft, motion: &mut Motion, adsb_on: bool, now: f64) {
        self.merge();
        let feed_row = self.selected_id.and_then(|id| aircraft.flight(id));
        let row = |me: &Self| me.selected_id.and_then(|id| selected_row(id, feed_row, me.hint.as_ref(), me.details.as_ref()));
        // `followEnded`'s `endedSeenMs = selected?.timestampMs ?? 0`
        if self.ended_seen_pending {
            self.ended_seen_pending = false;
            self.ended_seen_ms = row(self).map_or(0.0, |f| f.timestamp_ms as f64);
        }
        // auto-resume: a newer report in the feed shows it still flies (at most once per feed
        // refresh, since each end records the newest report seen)
        if let Some(id) = self.selected_id
            && self.follow_state == FollowState::Ended
        {
            let reported = feed_row.map_or(0.0, |f| f.timestamp_ms as f64);
            if reported > self.ended_seen_ms {
                self.start_follow(id, true);
                if self.ended_seen_pending {
                    self.ended_seen_pending = false;
                    self.ended_seen_ms = row(self).map_or(0.0, |f| f.timestamp_ms as f64);
                }
            }
        }
        self.selected = row(self);

        // adsb.lol: the query's effect, then the measured effect, then a waiting answer
        let icao24 = self.icao24();
        let callsign = self.selected.as_ref().map_or_else(String::new, |f| f.callsign.clone());
        let key = match (adsb_on, self.selected_id) {
            (true, Some(id)) if icao24.is_some() || !callsign.is_empty() => {
                Some((id, icao24.clone().map_or_else(|| AdsbQuery::Callsign(callsign.clone()), AdsbQuery::Hex)))
            }
            _ => None,
        };
        if key != self.adsb_key {
            self.adsb_key = key;
            self.adsb_request = None;
            self.adsb_answer = None;
            self.adsb_failures = 0;
            self.adsb_due = self.adsb_key.as_ref().map(|_| now);
            self.run_adsb(now);
        }
        let want = if adsb_on { self.selected_id } else { None };
        if let Some(id) = self.measured_id
            && Some(id) != want
        {
            motion.measure(id, None, motion.now(now));
        }
        self.measured_id = want;
        if self.adsb.as_ref().map(|a| a.id) != want {
            self.adsb = None;
        }
        if let (Some(snapshot), Some((id, query))) = (self.adsb_answer.take(), &self.adsb_key) {
            // without the address, the callsign match is checked against where the flight is
            let f = self.selected.as_ref();
            let (hex, callsign) = match query {
                AdsbQuery::Hex(hex) => (Some(hex.as_str()), callsign.as_str()),
                AdsbQuery::Callsign(callsign) => (None, callsign.as_str()),
            };
            let key = FlightKey {
                hex,
                callsign,
                reg: f.map_or("", |f| f.reg.as_str()),
                lat: f.map_or(f64::NAN, |f| f32_as_js(f.lat)),
                lon: f.map_or(f64::NAN, |f| f32_as_js(f.lon)),
            };
            if let Some(found) = match_adsb(&snapshot.aircraft, &key) {
                // its `now` is when adsb.lol answered: the age of the data on this clock
                let seen_at = now - (snapshot.now_ms as f64 - found.seen_ms as f64).max(0.0);
                self.adsb = Some(AdsbSeen { id: *id, aircraft: found.clone(), seen_at });
                motion.measure(*id, Some(&measured(found)), motion.now(now));
            }
        }

        // the route: made again when what it is made from changed
        let reference = aircraft.reference();
        let route_key = self.selected.as_ref().map(|selected| RouteKey {
            selected: selected.clone(),
            details: self.merged_for,
            feed: aircraft.generation(),
            reference: reference.map(|r| Arc::as_ptr(r) as usize),
            runways: self.runways_version,
        });
        if route_key != self.route_for {
            self.route = self.selected.as_ref().map(|selected| {
                let reference = reference.map_or(no_reference(), |r| &**r);
                route_for(selected, self.merged.as_ref(), aircraft.flights(), reference, &mut self.runways, now as i64)
            });
            self.route_for = route_key;
            for code in self.runways.take_wanted() {
                match &mut self.source {
                    Source::On(feeds) => {
                        let request = feeds.airport_runways(vec![code.clone()]);
                        self.runway_requests.push((code, request));
                    }
                    // outside Tauri no runways are known
                    Source::Auto | Source::Off => {
                        self.runways.settled(&code, Some(&HashMap::new()), now as i64);
                        self.runways_version += 1;
                    }
                }
            }
        }
    }
}

impl AppState {
    /// `select(id, opts)` at local time `now_ms` (a click on the map, a list, the search, the
    /// board, Escape with `None`), then [`Self::sync_flight`].
    pub fn select_flight(&mut self, id: Option<u32>, opts: SelectOptions, now_ms: f64) {
        self.flight.resolve(self.startup.fixture.as_deref(), self.backend.as_ref(), self.requests.as_ref());
        let known = id.and_then(|id| self.aircraft.flight(id));
        self.flight.select(id, opts, known, &mut self.controller, now_ms);
        self.sync_flight(now_ms);
    }

    /// `toggleFollow()` (the panel's Follow, the key C): the viewer's own choice, which a resumed
    /// stream does not override.
    pub fn toggle_follow(&mut self, now_ms: f64) {
        self.flight.following = !self.flight.following;
        self.flight.relock_on_resume = false;
        self.sync_flight(now_ms);
    }

    /// The selected flight's stream asked for again (after sign-in or sign-out: the flight plan
    /// needs the session).
    pub fn restart_follow(&mut self, now_ms: f64) {
        if let Some(id) = self.flight.selected_id {
            self.flight.start_follow(id, false);
            self.sync_flight(now_ms);
        }
    }

    /// The selected flight's effects that read the aircraft feed and the motion model, and its
    /// hand-over to the camera (see the module documentation): the row shown, the stream's last
    /// seen report, the auto-resume, adsb.lol's query and match, the route, then
    /// `camera.set_selected` and `camera.set_follow`.
    pub fn sync_flight(&mut self, now_ms: f64) {
        let adsb_on = self.persisted.settings.adsb;
        self.flight.sync(&self.aircraft, &mut self.motion, adsb_on, now_ms);
        // handed over when they change (`state::camera`'s contract), so the camera keeps what
        // nothing here has changed
        let now = (self.flight.selected.clone(), self.flight.following);
        if now != self.flight.handed {
            self.camera.set_selected(now.0.clone());
            self.camera.set_follow(now.1);
            self.flight.handed = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::mpsc::Sender;

    use rustradar_core::model::{Airport, AircraftInfo, BoundingBox, FlightInfo, LiveSnapshot, RunwayEnd, ScheduleInfo, SessionInfo, TopFlight, TrailPoint};
    use rustradar_core::geo::{effective_exaggeration, elevation};
    use rustradar_core::settings::Mode;
    use rustradar_scene::camera::{ViewState, viewport_for};

    use super::*;
    use crate::state::Startup;
    use crate::state::aircraft::{AREA_REFRESH_MS, Feeds, LoadedReference};

    /// The local clock at the first frame.
    const T0: f64 = 1_782_036_000_000.0;
    /// The flight the tests select, and its Mode S address.
    const ID: u32 = 7;
    const HEX: u32 = 0x4ca7b4;

    /// Where a test answers a request.
    type Answering<T> = Sender<Result<T, String>>;

    /// The requests the flight state made, with the senders that answer them.
    #[derive(Default)]
    struct Log {
        follows: Vec<(u32, Sender<FollowEvent>)>,
        unfollows: usize,
        opensky: Vec<(String, Answering<Vec<OpenSkyPoint>>)>,
        adsb: Vec<(AdsbQuery, Answering<AdsbSnapshot>)>,
        runways: Vec<(Vec<String>, Answering<RunwaysFound>)>,
    }

    struct Fake(Arc<Mutex<Log>>);

    impl FlightFeeds for Fake {
        fn follow_flight(&mut self, id: u32) -> Result<FollowStream, String> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().follows.push((id, tx));
            Ok(FollowStream::Channel(rx))
        }

        fn unfollow_flight(&mut self) {
            self.0.lock().unwrap().unfollows += 1;
        }

        fn opensky_track(&mut self, icao24: &str) -> Answer<Result<Vec<OpenSkyPoint>, String>> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().opensky.push((icao24.to_owned(), tx));
            rx.into()
        }

        fn adsb(&mut self, query: &AdsbQuery) -> Answer<Result<AdsbSnapshot, String>> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().adsb.push((query.clone(), tx));
            rx.into()
        }

        fn airport_runways(&mut self, codes: Vec<String>) -> Answer<Result<HashMap<String, Vec<Runway>>, String>> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().runways.push((codes, tx));
            rx.into()
        }
    }

    /// The aircraft feed: whatever snapshot the test holds, answered at once.
    struct Feed(Arc<Mutex<LiveSnapshot>>);

    impl Feeds for Feed {
        fn live_flights(&mut self, _area: Option<BoundingBox>, _categories: Option<Vec<u8>>) -> Answer<Result<LiveSnapshot, String>> {
            Answer::ready(Ok(self.0.lock().unwrap().clone()))
        }

        fn top_flights(&mut self) -> Answer<Result<Vec<TopFlight>, String>> {
            Answer::ready(Ok(Vec::new()))
        }

        fn reference_data(&mut self) -> Answer<Result<LoadedReference, String>> {
            Answer::ready(Err("none in these tests".into()))
        }

        fn session_info(&mut self) -> Option<SessionInfo> {
            None
        }
    }

    /// A report of flight `id` at `ts` (Unix ms) over Stockholm.
    fn row(id: u32, ts: f64) -> LiveFlight {
        LiveFlight {
            id,
            lat: 59.4,
            lon: 18.1,
            track: 90,
            alt: 12_000,
            speed: 300,
            timestamp_ms: ts as u64,
            callsign: "SAS123".into(),
            flight: "SK123".into(),
            reg: "SE-ROA".into(),
            typecode: "A20N".into(),
            origin: "ARN".into(),
            destination: "CPH".into(),
            icon: "A320".into(),
            ..Default::default()
        }
    }

    /// The follow stream's details with a report at `ts` (Unix ms) and a short trail.
    fn details(ts: f64) -> FlightDetails {
        FlightDetails {
            aircraft: Some(AircraftInfo { icao_address: HEX, reg: "SE-ROB".into(), typecode: "A21N".into(), ..Default::default() }),
            schedule: Some(ScheduleInfo { flight_number: "SK999".into(), ..Default::default() }),
            flight: Some(FlightInfo {
                timestamp_ms: ts as u64,
                latitude: 59.41,
                longitude: 18.12,
                track: 95,
                altitude: 12_200,
                ground_speed: 310,
                callsign: String::new(),
                ..Default::default()
            }),
            trail: vec![
                TrailPoint { timestamp: (ts / 1000.0) as u64 - 60, latitude: 59.40, longitude: 17.90, altitude: 9000, ..Default::default() },
                TrailPoint { timestamp: (ts / 1000.0) as u64 - 30, latitude: 59.40, longitude: 18.00, altitude: 11_000, ..Default::default() },
            ],
            ..Default::default()
        }
    }

    struct World {
        state: AppState,
        log: Arc<Mutex<Log>>,
        snapshot: Arc<Mutex<LiveSnapshot>>,
    }

    impl World {
        /// The app at Stockholm (a regional view: the feed every 8 s) with flight [`ID`] in the feed.
        fn new() -> World {
            let view = ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, pitch: 0.0, bearing: 0.0, position: None };
            let mut state = AppState::new(Startup::default(), None, None, None, view, [1100.0, 720.0]);
            let snapshot = Arc::new(Mutex::new(LiveSnapshot { flights: vec![row(ID, T0 - 2000.0)], server_time_ms: T0 as u64, ..Default::default() }));
            state.aircraft = Aircraft::with_feeds(Some(Box::new(Feed(snapshot.clone()))));
            let log = Arc::new(Mutex::new(Log::default()));
            state.flight = Flight::with_feeds(Some(Box::new(Fake(log.clone()))));
            let mut world = World { state, log, snapshot };
            world.frame(T0);
            world
        }

        /// One frame at `t`: the poll, the camera, the scene refresh's sync.
        fn frame(&mut self, t: f64) {
            self.state.poll(t);
            self.state.advance_camera(t);
            self.state.sync_flight(t);
        }

        /// The latest follow stream's sender, and how many were started.
        fn stream(&self) -> (Sender<FollowEvent>, usize) {
            let log = self.log.lock().unwrap();
            (log.follows.last().expect("a stream").1.clone(), log.follows.len())
        }

        fn send(&mut self, event: FollowEvent, t: f64) {
            let (tx, _) = self.stream();
            tx.send(event).unwrap();
            self.frame(t);
        }

        fn flight(&self) -> &Flight {
            &self.state.flight
        }
    }

    #[test]
    fn the_follow_stream_goes_connecting_live_reconnecting_ended() {
        let mut w = World::new();
        assert_eq!(w.flight().selected_id(), None);
        assert_eq!(w.log.lock().unwrap().unfollows, 1, "on mount the stream left over is let go of");
        w.state.select_flight(Some(ID), SelectOptions::default(), T0 + 100.0);
        assert_eq!(w.flight().follow_state(), FollowState::Connecting);
        assert_eq!(w.stream().1, 1);
        assert_eq!(w.log.lock().unwrap().follows[0].0, ID);
        assert_eq!(w.flight().selected().map(|f| f.timestamp_ms), Some((T0 - 2000.0) as u64), "the feed's row");
        w.send(FollowEvent::Details(Box::new(details(T0 - 500.0))), T0 + 200.0);
        assert_eq!(w.flight().follow_state(), FollowState::Live);
        assert!(w.flight().details().is_some());
        assert_eq!(w.flight().selected().map(|f| f.timestamp_ms), Some((T0 - 500.0) as u64), "the stream's newer report");
        w.send(FollowEvent::Error("subscription failed".into()), T0 + 300.0);
        assert_eq!(w.flight().follow_state(), FollowState::Reconnecting);
        assert!(w.flight().details().is_some(), "the details stay");
        w.send(FollowEvent::Details(Box::new(details(T0 + 200.0))), T0 + 400.0);
        assert_eq!(w.flight().follow_state(), FollowState::Live);
        w.send(FollowEvent::Ended("flight is no longer live".into()), T0 + 500.0);
        assert_eq!(w.flight().follow_state(), FollowState::Ended);
        assert_eq!(w.flight().last_seen_ms(), T0 + 200.0, "the newest report shown then");
        assert_eq!(w.stream().1, 1, "no resume: the feed has nothing newer");
    }

    #[test]
    fn an_ended_stream_resumes_when_the_feed_has_a_newer_report_and_locks_on_again() {
        let mut w = World::new();
        w.state.select_flight(Some(ID), SelectOptions { follow: true, ..Default::default() }, T0 + 100.0);
        w.send(FollowEvent::Details(Box::new(details(T0 - 1000.0))), T0 + 200.0);
        assert!(w.flight().following() && w.state.camera.following(), "locked on");
        w.send(FollowEvent::Ended("rate limited".into()), T0 + 300.0);
        assert_eq!(w.flight().follow_state(), FollowState::Ended);
        assert!(!w.flight().following() && !w.state.camera.following(), "the camera lets go");
        // the last seen report is the feed's (newer than the stream's)
        assert_eq!(w.flight().last_seen_ms(), T0 - 1000.0);
        let mut t = T0 + 300.0;
        while t < T0 + f64::from(AREA_REFRESH_MS as i32) + 200.0 {
            t += 100.0;
            w.frame(t);
            assert_eq!(w.stream().1, 1, "nothing newer in the feed yet ({t})");
        }
        // the next round brings a newer report: asked again, as reconnecting
        w.snapshot.lock().unwrap().flights = vec![row(ID, T0 + 7000.0)];
        let mut asked = None;
        while t < T0 + 30_000.0 && asked.is_none() {
            t += 100.0;
            w.frame(t);
            if w.stream().1 == 2 {
                asked = Some(t);
            }
        }
        assert!(asked.is_some(), "resumed");
        assert_eq!(w.flight().follow_state(), FollowState::Reconnecting);
        assert!(!w.flight().following());
        w.send(FollowEvent::Details(Box::new(details(T0 + 8000.0))), t + 100.0);
        assert_eq!(w.flight().follow_state(), FollowState::Live);
        assert!(w.flight().following() && w.state.camera.following(), "locked on again, as before it ended");
    }

    #[test]
    fn toggling_follow_is_the_viewers_choice_a_resume_does_not_override() {
        let mut w = World::new();
        w.state.select_flight(Some(ID), SelectOptions { follow: true, ..Default::default() }, T0 + 100.0);
        w.send(FollowEvent::Ended("gone".into()), T0 + 200.0);
        // C while ended: on, then off again; the resume must not lock on
        w.state.toggle_follow(T0 + 300.0);
        assert!(w.flight().following());
        w.state.toggle_follow(T0 + 400.0);
        assert!(!w.flight().following());
        w.state.select_flight(Some(ID), SelectOptions::default(), T0 + 500.0);
        assert_eq!(w.flight().follow_state(), FollowState::Connecting, "another click on an ended flight asks again");
        w.send(FollowEvent::Details(Box::new(details(T0))), T0 + 600.0);
        assert!(!w.flight().following());
        assert!(!w.state.camera.following());
    }

    #[test]
    fn selecting_the_same_flight_again_does_nothing_while_it_is_live() {
        let mut w = World::new();
        w.state.select_flight(Some(ID), SelectOptions::default(), T0 + 100.0);
        w.send(FollowEvent::Details(Box::new(details(T0))), T0 + 200.0);
        w.state.select_flight(Some(ID), SelectOptions::default(), T0 + 300.0);
        assert_eq!(w.stream().1, 1);
        assert_eq!(w.flight().follow_state(), FollowState::Live);
        assert!(w.flight().details().is_some());
        // with fly it is a new selection: the details go, the stream starts again
        w.state.select_flight(Some(ID), SelectOptions { fly: true, ..Default::default() }, T0 + 400.0);
        assert_eq!(w.stream().1, 2);
        assert!(w.flight().details().is_none());
    }

    #[test]
    fn deselecting_stops_the_stream_and_its_late_events_are_dropped() {
        let mut w = World::new();
        w.state.select_flight(Some(ID), SelectOptions { follow: true, ..Default::default() }, T0 + 100.0);
        let (tx, _) = w.stream();
        let unfollows = w.log.lock().unwrap().unfollows;
        w.state.select_flight(None, SelectOptions::default(), T0 + 200.0);
        assert_eq!(w.log.lock().unwrap().unfollows, unfollows + 1);
        assert_eq!(w.flight().selected_id(), None);
        assert!(w.flight().selected().is_none() && w.state.camera.selected().is_none());
        assert!(!w.state.camera.following());
        // a late event of the stream let go of goes nowhere
        assert!(tx.send(FollowEvent::Details(Box::new(details(T0)))).is_err());
        w.frame(T0 + 300.0);
        assert!(w.flight().details().is_none());
        assert!(w.flight().route().is_none());
    }

    #[test]
    fn without_a_backend_the_stream_ends_at_once() {
        let mut w = World::new();
        w.state.flight = Flight::with_feeds(None);
        w.frame(T0 + 50.0);
        w.state.select_flight(Some(ID), SelectOptions { follow: true, ..Default::default() }, T0 + 100.0);
        assert_eq!(w.flight().follow_state(), FollowState::Ended);
        assert_eq!(w.flight().last_seen_ms(), T0 - 2000.0, "the feed's report");
        assert!(!w.flight().following());
        assert_eq!(w.flight().selected().map(|f| f.id), Some(ID), "still selected, its row shown");
    }

    #[test]
    fn the_row_shown_is_the_newer_of_the_feed_and_the_stream() {
        let feed = row(ID, T0);
        let mut d = details(T0 - 1.0);
        assert_eq!(selected_row(ID, Some(&feed), None, Some(&d)), Some(feed.clone()), "the feed is newer");
        assert_eq!(selected_row(ID, Some(&feed), None, None), Some(feed.clone()));
        assert_eq!(selected_row(ID, None, None, None), None);
        d.flight.as_mut().unwrap().timestamp_ms = (T0 + 1.0) as u64;
        let shown = selected_row(ID, Some(&feed), None, Some(&d)).unwrap();
        assert_eq!((shown.lat, shown.lon, shown.track, shown.alt, shown.speed), (59.41, 18.12, 95, 12_200, 310));
        assert_eq!(shown.timestamp_ms, (T0 + 1.0) as u64);
        // the stream's empty callsign falls back on the feed's; the names come from the feed
        assert_eq!((shown.callsign.as_str(), shown.flight.as_str(), shown.reg.as_str(), shown.typecode.as_str()), ("SAS123", "SK123", "SE-ROA", "A20N"));
        assert_eq!((shown.origin.as_str(), shown.destination.as_str(), shown.icon.as_str()), ("ARN", "CPH", "A320"));
        // without the feed: the details' names, and nothing for the rest
        let alone = selected_row(ID, None, None, Some(&d)).unwrap();
        assert_eq!((alone.callsign.as_str(), alone.flight.as_str(), alone.reg.as_str(), alone.typecode.as_str()), ("", "SK999", "SE-ROB", "A21N"));
        assert_eq!((alone.origin.as_str(), alone.icon.as_str()), ("", ""));
        // a hint stands in for the feed
        let hint = LiveFlight { id: ID, lat: 1.0, lon: 2.0, callsign: "HINT1".into(), ..Default::default() };
        assert_eq!(selected_row(ID, None, Some(&hint), None), Some(hint.clone()));
        assert_eq!(selected_row(ID, None, Some(&hint), Some(&d)).unwrap().callsign, "HINT1");
    }

    #[test]
    fn a_pick_flies_to_the_known_position_or_when_the_details_arrive() {
        let mut w = World::new();
        w.state.select_flight(Some(ID), SelectOptions { fly: true, ..Default::default() }, T0 + 100.0);
        assert!(w.state.controller.in_transition(), "flies to the feed's row at once");
        w.frame(T0 + 10_000.0);
        let v = w.state.controller.view();
        assert!((v.longitude - 18.1).abs() < 1e-4 && (v.latitude - 59.4).abs() < 1e-4, "{v:?}");
        // a flight the feed does not have yet: when its details arrive
        w.state.select_flight(Some(99), SelectOptions { fly: true, ..Default::default() }, T0 + 11_000.0);
        assert!(!w.state.controller.in_transition());
        let mut d = details(T0);
        d.flight.as_mut().unwrap().longitude = 18.3;
        w.send(FollowEvent::Details(Box::new(d)), T0 + 11_100.0);
        assert!(w.state.controller.in_transition());
        w.frame(T0 + 20_000.0);
        assert!((w.state.controller.view().longitude - 18.3).abs() < 1e-4);
        // a click selects without moving the camera
        let before = w.state.controller.view();
        w.state.select_flight(Some(ID), SelectOptions::default(), T0 + 21_000.0);
        assert!(!w.state.controller.in_transition());
        assert_eq!(w.state.controller.view(), before);
    }

    #[test]
    fn opensky_is_asked_once_the_address_is_known_then_every_five_minutes_and_merged_in_front() {
        let mut w = World::new();
        w.state.select_flight(Some(ID), SelectOptions::default(), T0 + 100.0);
        w.frame(T0 + 200.0);
        assert!(w.log.lock().unwrap().opensky.is_empty(), "no address yet");
        // asked in the frame the address arrives
        w.send(FollowEvent::Details(Box::new(details(T0))), T0 + 300.0);
        let asked: Vec<String> = w.log.lock().unwrap().opensky.iter().map(|(h, _)| h.clone()).collect();
        assert_eq!(asked, vec!["4ca7b4".to_owned()]);
        w.frame(T0 + 400.0);
        assert_eq!(w.log.lock().unwrap().opensky.len(), 1);
        // two earlier points from OpenSky go in front of the trail
        let first = details(T0).trail[0].timestamp as i64;
        let points = vec![
            OpenSkyPoint { timestamp: first - 600, latitude: 59.3, longitude: 17.5, altitude: 3000, track: 80, on_ground: false },
            OpenSkyPoint { timestamp: first - 300, latitude: 59.35, longitude: 17.7, altitude: 6000, track: 80, on_ground: false },
        ];
        w.log.lock().unwrap().opensky[0].1.send(Ok(points)).unwrap();
        w.frame(T0 + 500.0);
        assert_eq!(w.flight().opensky_added(), 2);
        assert_eq!(w.flight().details().unwrap().trail.len(), 4);
        assert_eq!(w.flight().raw_details().unwrap().trail.len(), 2, "the stream's own details unchanged");
        w.frame(T0 + 300.0 + OPENSKY_REFRESH_MS - 100.0);
        assert_eq!(w.log.lock().unwrap().opensky.len(), 1);
        w.frame(T0 + 300.0 + OPENSKY_REFRESH_MS);
        assert_eq!(w.log.lock().unwrap().opensky.len(), 2, "every five minutes");
        // another flight: its own address, and the old track no longer merged
        w.state.select_flight(None, SelectOptions::default(), T0 + 600_000.0);
        w.frame(T0 + 600_100.0);
        assert_eq!(w.flight().opensky_added(), 0);
        w.frame(T0 + 900_000.0);
        assert_eq!(w.log.lock().unwrap().opensky.len(), 2, "the timer stopped with the selection");
    }

    #[test]
    fn adsb_lol_is_polled_every_three_seconds_backing_off_and_its_match_shown_for_a_minute() {
        let mut w = World::new();
        w.state.select_flight(Some(ID), SelectOptions::default(), T0 + 100.0);
        // no address yet: by the callsign
        let first = w.log.lock().unwrap().adsb.first().map(|(q, _)| q.clone());
        assert_eq!(first, Some(AdsbQuery::Callsign("SAS123".into())));
        // the address arrives: a new query, asked at once
        w.send(FollowEvent::Details(Box::new(details(T0))), T0 + 200.0);
        let queries: Vec<AdsbQuery> = w.log.lock().unwrap().adsb.iter().map(|(q, _)| q.clone()).collect();
        assert_eq!(queries.last(), Some(&AdsbQuery::Hex("4ca7b4".into())));
        let n = queries.len();
        // failures back off: 6, 12, 24, 48, 48 s after each answer
        let mut t = T0 + 200.0;
        for (i, wait) in [6_000.0, 12_000.0, 24_000.0, 48_000.0, 48_000.0].into_iter().enumerate() {
            w.log.lock().unwrap().adsb.last().unwrap().1.send(Err("resting".into())).unwrap();
            t += 10.0;
            w.frame(t);
            w.frame(t + wait - 10.0);
            assert_eq!(w.log.lock().unwrap().adsb.len(), n + i, "not before {wait} ms");
            t += wait;
            w.frame(t);
            assert_eq!(w.log.lock().unwrap().adsb.len(), n + i + 1, "after {wait} ms");
        }
        // an answer with the aircraft: shown, and the next ask 3 s later
        let aircraft = AdsbAircraft { hex: "4ca7b4".into(), lat: Some(59.41), lon: Some(18.12), seen_ms: 1_000, roll: Some(12.0), ..Default::default() };
        let snapshot = AdsbSnapshot { now_ms: 3_000, aircraft: vec![aircraft] };
        w.log.lock().unwrap().adsb.last().unwrap().1.send(Ok(snapshot)).unwrap();
        t += 10.0;
        w.frame(t);
        let seen = w.flight().adsb().cloned().expect("a match");
        assert_eq!(seen.id, ID);
        assert_eq!(seen.seen_at, t - 2_000.0, "seen 2 s before adsb.lol answered");
        assert!(w.flight().measured(t, true).is_some());
        assert!(w.flight().measured(seen.seen_at + ADSB_SHOW_MS, true).is_none(), "shown for a minute");
        let asked = w.log.lock().unwrap().adsb.len();
        w.frame(t + ADSB_POLL_MS);
        assert_eq!(w.log.lock().unwrap().adsb.len(), asked + 1, "3 s after a good answer");
        // the setting off: no more asks, nothing shown
        w.state.persisted.settings.adsb = false;
        w.frame(t + ADSB_POLL_MS + 10.0);
        assert!(w.flight().adsb().is_none());
        assert!(w.flight().measured(t, false).is_none());
        let asked = w.log.lock().unwrap().adsb.len();
        w.frame(t + 60_000.0);
        assert_eq!(w.log.lock().unwrap().adsb.len(), asked);
    }

    /// Item A10's lock engages through this state: a click selects without moving the camera; C
    /// (or a selection with `follow`) hands the camera the row shown and the lock, and each frame
    /// then holds the aircraft as drawn at the screen's centre; deselecting lets go.
    #[test]
    fn following_the_selected_flight_locks_the_camera_on_it_and_deselecting_lets_go() {
        let mut w = World::new();
        let before = w.state.controller.view();
        w.state.select_flight(Some(ID), SelectOptions::default(), T0 + 100.0);
        w.frame(T0 + 200.0);
        assert_eq!(w.state.camera.selected().map(|f| f.id), Some(ID), "the row shown handed over");
        assert!(!w.state.camera.following() && !w.state.controller.locked(), "a click does not lock");
        assert_eq!(w.state.controller.view(), before);
        w.state.toggle_follow(T0 + 300.0);
        assert!(w.state.camera.following(), "handed over at once");
        w.send(FollowEvent::Details(Box::new(details(T0 + 250.0))), T0 + 400.0);
        for k in 0..6 {
            let t = T0 + 500.0 + 1_000.0 * f64::from(k);
            w.frame(t);
            assert!(w.state.controller.locked());
            let shown = w.flight().selected().cloned().expect("a row shown");
            assert_eq!(w.state.camera.selected(), Some(&shown), "the stream's newer report, handed over");
            let pose = w.state.motion.pose(&shown, w.state.clocks(t).feed_ms);
            let v = w.state.controller.view();
            let size = w.state.controller.size();
            let h = elevation(pose.alt_ft, effective_exaggeration(w.state.persisted.settings.exaggeration, v.zoom));
            let [x, y, _] = viewport_for(&v, size).project([pose.lon, pose.lat, h]);
            let off = (x - size[0] / 2.0).hypot(y - size[1] / 2.0);
            assert!(off < 1.0, "frame {k}: the aircraft {off} px from the centre");
        }
        w.state.select_flight(None, SelectOptions::default(), T0 + 7_000.0);
        assert!(w.state.camera.selected().is_none() && !w.state.camera.following());
        w.frame(T0 + 7_100.0);
        assert!(!w.state.controller.locked(), "let go");
    }

    #[test]
    fn the_aircraft_off_the_map_close_the_flight() {
        let mut w = World::new();
        w.state.select_flight(Some(ID), SelectOptions { follow: true, ..Default::default() }, T0 + 100.0);
        w.state.persisted.settings.mode = Mode::Ships;
        w.frame(T0 + 200.0);
        assert_eq!(w.flight().selected_id(), None);
        assert!(!w.state.camera.following());
    }

    #[test]
    fn the_route_asks_for_the_runways_it_needs_and_is_made_again_when_they_arrive() {
        let mut w = World::new();
        let data = ReferenceData {
            airports: vec![
                Airport { id: 1, iata: "ARN".into(), icao: "ESSA".into(), lat: 59.6519, lon: 17.9186, ..Default::default() },
                Airport { id: 2, iata: "CPH".into(), icao: "EKCH".into(), lat: 55.618, lon: 12.656, ..Default::default() },
            ],
            ..Default::default()
        };
        // the reference data through a feed of its own
        struct WithReference(Arc<Mutex<LiveSnapshot>>, ReferenceData);
        impl Feeds for WithReference {
            fn live_flights(&mut self, _area: Option<BoundingBox>, _categories: Option<Vec<u8>>) -> Answer<Result<LiveSnapshot, String>> {
                Answer::ready(Ok(self.0.lock().unwrap().clone()))
            }
            fn top_flights(&mut self) -> Answer<Result<Vec<TopFlight>, String>> {
                Answer::ready(Ok(Vec::new()))
            }
            fn reference_data(&mut self) -> Answer<Result<LoadedReference, String>> {
                Answer::ready(Ok(LoadedReference::new(self.1.clone())))
            }
            fn session_info(&mut self) -> Option<SessionInfo> {
                None
            }
        }
        w.state.aircraft = Aircraft::with_feeds(Some(Box::new(WithReference(w.snapshot.clone(), data))));
        w.frame(T0 + 50.0);
        assert!(w.state.aircraft.reference().is_some());
        w.state.select_flight(Some(ID), SelectOptions::default(), T0 + 100.0);
        let route = w.flight().route().expect("a route");
        assert_eq!(route.origin.as_ref().map(|a| a.iata.as_str()), Some("ARN"));
        assert_eq!(route.destination.as_ref().map(|a| a.iata.as_str()), Some("CPH"));
        let asked: Vec<Vec<String>> = w.log.lock().unwrap().runways.iter().map(|(c, _)| c.clone()).collect();
        assert_eq!(asked, vec![vec!["ESSA".to_owned()], vec!["EKCH".to_owned()]], "one airport at a time");
        let before = route.clone();
        // EKCH's runways arrive: the route is made again with them
        let end = |ident: &str, lat: f64, lon: f64| RunwayEnd { ident: ident.into(), lat, lon, ..Default::default() };
        let runway = Runway { length_ft: Some(11_811.0), width_ft: None, surface: "ASP".into(), ends: [end("04R", 55.6003, 12.6260), end("22L", 55.6290, 12.6668)] };
        let found = HashMap::from([("EKCH".to_owned(), vec![runway])]);
        w.log.lock().unwrap().runways[1].1.send(Ok(found)).unwrap();
        // ESSA's download fails: asked again a minute later, at a route that wants it
        w.log.lock().unwrap().runways[0].1.send(Err("offline".into())).unwrap();
        w.frame(T0 + 200.0);
        let after = w.flight().route().expect("a route");
        assert_ne!(after.arrival, before.arrival, "the arrival runway now known: {:?}", after.arrival);
        assert_eq!(w.log.lock().unwrap().runways.len(), 2);
    }
}
