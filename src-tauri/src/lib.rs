mod board;
mod liveries;
mod reference;
mod session;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use fr24::Fr24;
use fr24::grpc::{
    BoundingBox, FlightDetailsParams, FlightDetailsRecord, FollowFlightParams, LiveFeedParams,
    TopFlightRecord, TopFlightsParams,
};
use fr24::json::{
    AirportListParams, FindEntry, FindParams, FlightListParams, FlightListQuery, FlightListRecord,
};
use fr24::proto::common::Icon;
use futures_util::StreamExt;
use serde::Serialize;
use tauri::async_runtime::JoinHandle;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

/// Number of concurrent requests when fetching the world feed.
const WORLD_CONCURRENCY: usize = 8;
/// Give up following a flight after this many consecutive failed subscriptions.
const FOLLOW_MAX_FAILURES: u32 = 3;
const FOLLOW_RETRY_DELAY: Duration = Duration::from_secs(3);

struct AppState {
    /// Swapped out when signing in or out.
    fr24: RwLock<Arc<Fr24>>,
    /// The `FollowFlight` stream of the selected flight.
    follow: Mutex<Option<JoinHandle<()>>>,
    /// Airlines and airports, loaded once.
    reference: tokio::sync::Mutex<Option<Arc<reference::ReferenceData>>>,
    /// Logo-derived livery colours per IATA code, loaded from disk on first use.
    colors: tokio::sync::Mutex<Option<liveries::ColorCache>>,
}

impl AppState {
    fn client(&self) -> Arc<Fr24> {
        self.fr24.read().unwrap().clone()
    }
}

type CmdResult<T> = Result<T, String>;

fn to_string(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn cache_file(app: &AppHandle, name: &str) -> CmdResult<PathBuf> {
    Ok(app.path().app_cache_dir().map_err(to_string)?.join(name))
}

/// A compact view of a live flight, sent for every aircraft on the globe.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveFlight {
    id: u32,
    lat: f32,
    lon: f32,
    /// Degrees clockwise from north
    track: i32,
    /// Feet
    alt: i32,
    /// Knots
    speed: i32,
    on_ground: bool,
    timestamp_ms: u64,
    callsign: String,
    flight: String,
    reg: String,
    typecode: String,
    origin: String,
    destination: String,
    /// Flightradar24's icon class, e.g. `A320`, `B744`, `EC` (helicopter)
    icon: String,
}

impl From<fr24::proto::common::Flight> for LiveFlight {
    fn from(f: fr24::proto::common::Flight) -> Self {
        let extra = f.extra_info.unwrap_or_default();
        let route = extra.route.unwrap_or_default();
        Self {
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
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveSnapshot {
    flights: Vec<LiveFlight>,
    server_time_ms: u64,
    requests: usize,
    errors: Vec<String>,
    elapsed_ms: u64,
    authenticated: bool,
}

/// Live flights within `area` (or the whole world), optionally only the
/// given categories (`fr24::proto::common::Service` values).
#[tauri::command]
async fn live_flights(
    area: Option<BoundingBox>,
    services: Option<Vec<i32>>,
    state: State<'_, AppState>,
) -> CmdResult<LiveSnapshot> {
    let fr24 = state.client();
    let start = Instant::now();
    let area = area.unwrap_or(fr24::bbox::WORLD);
    let mut template = LiveFeedParams {
        limit: fr24.live_feed_limit(),
        ..LiveFeedParams::new(area)
    };
    if let Some(services) = services.filter(|s| !s.is_empty()) {
        template.services = services;
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
        flights: world.flights.into_iter().map(LiveFlight::from).collect(),
        elapsed_ms: start.elapsed().as_millis() as u64,
        authenticated: fr24.auth().is_some(),
    })
}

#[tauri::command]
async fn flight_details(
    flight_id: u32,
    state: State<'_, AppState>,
) -> CmdResult<FlightDetailsRecord> {
    let response = state
        .client()
        .flight_details(&FlightDetailsParams::new(flight_id))
        .await
        .map_err(to_string)?;
    Ok(FlightDetailsRecord::from(&response))
}

#[derive(Clone, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
enum FollowEvent {
    /// The latest merged details, including the full trail.
    Details(Box<FlightDetailsRecord>),
    /// A subscription failed; it will be retried.
    Error(String),
    /// Following stopped, e.g. because the flight is no longer live.
    Ended(String),
}

/// Stream live updates for a flight. Replaces any previously followed flight.
#[tauri::command]
fn follow_flight(flight_id: u32, on_event: Channel<FollowEvent>, state: State<'_, AppState>) {
    let task = tauri::async_runtime::spawn(follow_task(state.client(), flight_id, on_event));
    if let Some(previous) = state.follow.lock().unwrap().replace(task) {
        previous.abort();
    }
}

#[tauri::command]
fn unfollow_flight(state: State<'_, AppState>) {
    if let Some(task) = state.follow.lock().unwrap().take() {
        task.abort();
    }
}

async fn follow_task(fr24: Arc<Fr24>, flight_id: u32, channel: Channel<FollowEvent>) {
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
                                .send(FollowEvent::Details(Box::new(details.clone())))
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

/// The most tracked flights right now.
#[tauri::command]
async fn top_flights(state: State<'_, AppState>) -> CmdResult<Vec<TopFlightRecord>> {
    let response = state
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
#[tauri::command]
async fn search(query: String, state: State<'_, AppState>) -> CmdResult<Vec<FindEntry>> {
    let query = query.trim();
    if query.len() < 2 {
        return Ok(Vec::new());
    }
    let find = state
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
#[tauri::command]
async fn reference_data(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<Arc<reference::ReferenceData>> {
    let mut slot = state.reference.lock().await;
    if let Some(data) = slot.as_ref() {
        return Ok(data.clone());
    }
    let path = cache_file(&app, "reference.json")?;
    let data = Arc::new(reference::load(&state.client(), &path).await?);
    *slot = Some(data.clone());
    Ok(data)
}

/// Arrivals or departures at an airport (IATA code), one page at a time.
#[tauri::command]
async fn airport_board(
    code: String,
    mode: board::Mode,
    page: Option<u32>,
    state: State<'_, AppState>,
) -> CmdResult<board::Board> {
    let params = AirportListParams {
        page: page.unwrap_or(1),
        limit: 25,
        ..AirportListParams::new(code.trim().to_uppercase(), mode.api())
    };
    let response = state
        .client()
        .airport_list(&params)
        .await
        .map_err(to_string)?;
    Ok(board::parse(&response, mode))
}

/// The latest flights flown by an aircraft, newest first.
#[tauri::command]
async fn aircraft_history(
    registration: String,
    state: State<'_, AppState>,
) -> CmdResult<Vec<FlightListRecord>> {
    let params = FlightListParams {
        limit: 10,
        ..FlightListParams::new(FlightListQuery::Reg(registration.trim().to_owned()))
    };
    let response = state
        .client()
        .flight_list(&params)
        .await
        .map_err(to_string)?;
    Ok(FlightListRecord::all(&response))
}

/// Livery colours derived from airline logos, per IATA code. Unknown codes
/// are fetched (at most 40 per call) and remembered on disk.
#[tauri::command]
async fn airline_colors(
    iatas: Vec<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<HashMap<String, Option<liveries::LogoColors>>> {
    let path = cache_file(&app, "livery-colors.json")?;
    let mut slot = state.colors.lock().await;
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
        let fr24 = state.client();
        let fetched: Vec<(String, Option<liveries::LogoColors>)> =
            futures_util::stream::iter(missing)
                .map(|code| {
                    let http = fr24.http().clone();
                    async move {
                        let colors = liveries::fetch(&http, &code).await;
                        (code, colors)
                    }
                })
                .buffer_unordered(6)
                .collect()
                .await;
        cache.extend(fetched);
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let fr24 = Fr24::new().expect("failed to build http client");
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            fr24: RwLock::new(Arc::new(fr24)),
            follow: Mutex::new(None),
            reference: tokio::sync::Mutex::new(None),
            colors: tokio::sync::Mutex::new(None),
        })
        .setup(|app| {
            // log in with credentials from the environment or the fr24 config
            // file, if any; until then (or without them) requests are anonymous
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = handle.state::<AppState>();
                let mut fr24 = Fr24::clone(&state.client());
                match fr24.login(None).await {
                    Ok(Some(_)) => *state.fr24.write().unwrap() = Arc::new(fr24),
                    Ok(None) => {}
                    Err(e) => eprintln!("fr24 login failed, continuing anonymously: {e}"),
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            live_flights,
            flight_details,
            follow_flight,
            unfollow_flight,
            top_flights,
            search,
            reference_data,
            airport_board,
            aircraft_history,
            airline_colors,
            session::session_info,
            session::sign_in,
            session::sign_out,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
