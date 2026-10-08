mod board;
mod dwd;
mod liveries;
mod mrms;
mod opensky;
mod radar;
mod reference;
mod session;

use std::collections::{HashMap, HashSet};
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
    /// The newest volume from each 3D radar source, keyed by its name.
    radar: tokio::sync::Mutex<HashMap<&'static str, Arc<radar::Volume>>>,
    /// Sources with a fetch under way, so overlapping calls don't duplicate it.
    radar_fetching: Mutex<HashSet<&'static str>>,
    /// OpenSky tracks per aircraft, and whether it told us to back off.
    opensky: tokio::sync::Mutex<opensky::Cache>,
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

/// Sources of measured 3D reflectivity with the region each one covers. A
/// volume costs tens of megabytes to fetch, so a source is only asked for
/// when its region is actually in view. DWD's figures are the extent of its
/// WN composite grid, written out here to keep the table self-contained.
const RADAR_SOURCES: [(&'static str, radar::Bounds); 2] = [
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

/// The newest 3D radar volumes for the sources visible in `area` (or
/// everywhere, when it is `None`): NOAA MRMS over North America and DWD over
/// central Europe, fetched at most every few minutes. Anything already
/// cached is returned either way. The images themselves are then read with
/// [`radar_image`].
#[tauri::command]
async fn radar_frame(
    area: Option<BoundingBox>,
    state: State<'_, AppState>,
) -> CmdResult<Vec<radar::VolumeInfo>> {
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
#[tauri::command]
async fn radar_image(
    source: String,
    name: String,
    state: State<'_, AppState>,
) -> CmdResult<tauri::ipc::Response> {
    let volume = state
        .radar
        .lock()
        .await
        .get(source.as_str())
        .cloned()
        .ok_or("no such radar source")?;
    let bytes = volume.image(&name).ok_or("no image by that name")?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// The flight an aircraft is on now, as tracked by the OpenSky Network.
/// `icao24` is the six-digit hex Mode S address.
#[tauri::command]
async fn opensky_track(
    icao24: String,
    state: State<'_, AppState>,
) -> CmdResult<Vec<opensky::TrackPoint>> {
    let http = state.client().http().clone();
    let mut cache = state.opensky.lock().await;
    opensky::track(&http, &mut cache, &icao24).await
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
            radar: tokio::sync::Mutex::new(HashMap::new()),
            radar_fetching: Mutex::new(HashSet::new()),
            opensky: tokio::sync::Mutex::new(opensky::Cache::default()),
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
            radar_frame,
            radar_image,
            opensky_track,
            session::session_info,
            session::sign_in,
            session::sign_out,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox(south: f32, north: f32, west: f32, east: f32) -> BoundingBox {
        BoundingBox {
            south,
            north,
            west,
            east,
        }
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
}
