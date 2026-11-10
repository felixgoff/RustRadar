//! The commands and protocols the frontend calls, each the backend's function
//! behind the name and arguments it calls it by. Until stage 12 the app is
//! this shim over `rustradar-backend`.

use std::collections::HashMap;
use std::sync::Arc;

use fr24::grpc::{BoundingBox, FlightDetailsRecord, TopFlightRecord};
use fr24::json::{FindEntry, FlightListRecord};
use rustradar_backend::{
    Backend, CmdResult, FollowEvent, Following, LiveSnapshot, adsblol, board, devlog, geocode, keys, liveries,
    opensky, radar, reference, roads, runways, session, ships, taxiways, trafikverket, train_media, trains,
    weather,
};
use tauri::State;
use tauri::ipc::Channel;

/// Hands a backend stream's events to the page's channel until the page
/// stops listening (dropping the stream then ends it) or the stream ends
/// (replaced, stopped, or over).
fn forward<T: tauri::ipc::IpcResponse + Send + 'static>(mut following: Following<T>, channel: Channel<T>) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = following.rx.recv().await {
            if channel.send(event).is_err() {
                return;
            }
        }
    });
}

// ---------------------------------------------------------------- Flightradar24

/// Live flights within `area` (or the whole world), optionally only the
/// given categories (`fr24::proto::common::Service` values).
#[tauri::command]
pub async fn live_flights(
    area: Option<BoundingBox>,
    services: Option<Vec<i32>>,
    state: State<'_, Backend>,
) -> CmdResult<LiveSnapshot> {
    state.live_flights(area, services).await
}

#[tauri::command]
pub async fn flight_details(flight_id: u32, state: State<'_, Backend>) -> CmdResult<FlightDetailsRecord> {
    state.flight_details(flight_id).await
}

/// Stream live updates for a flight. Replaces any previously followed flight.
#[tauri::command]
pub fn follow_flight(flight_id: u32, on_event: Channel<FollowEvent>, state: State<'_, Backend>) {
    forward(state.follow_flight(flight_id), on_event);
}

#[tauri::command]
pub fn unfollow_flight(state: State<'_, Backend>) {
    state.unfollow_flight();
}

/// The most tracked flights right now.
#[tauri::command]
pub async fn top_flights(state: State<'_, Backend>) -> CmdResult<Vec<TopFlightRecord>> {
    state.top_flights().await
}

/// Search for live flights, airports, operators, schedules and aircraft.
#[tauri::command]
pub async fn search(query: String, state: State<'_, Backend>) -> CmdResult<Vec<FindEntry>> {
    state.search(&query).await
}

/// Airlines and airports, from a disk cache refreshed weekly.
#[tauri::command]
pub async fn reference_data(state: State<'_, Backend>) -> CmdResult<Arc<reference::ReferenceData>> {
    state.reference_data().await
}

/// The runways of the given airports (ICAO codes); airports not known are left out.
#[tauri::command]
pub async fn airport_runways(
    airports: Vec<String>,
    state: State<'_, Backend>,
) -> CmdResult<HashMap<String, Vec<runways::Runway>>> {
    state.airport_runways(airports).await
}

/// Arrivals or departures at an airport (IATA code), one page at a time.
#[tauri::command]
pub async fn airport_board(
    code: String,
    mode: board::Mode,
    page: Option<u32>,
    state: State<'_, Backend>,
) -> CmdResult<board::Board> {
    state.airport_board(&code, mode, page).await
}

/// The latest flights flown by an aircraft, newest first.
#[tauri::command]
pub async fn aircraft_history(registration: String, state: State<'_, Backend>) -> CmdResult<Vec<FlightListRecord>> {
    state.aircraft_history(&registration).await
}

/// Livery colours derived from airline logos, per IATA code.
#[tauri::command]
pub async fn airline_colors(
    iatas: Vec<String>,
    state: State<'_, Backend>,
) -> CmdResult<HashMap<String, Option<liveries::LogoColors>>> {
    state.airline_colors(iatas).await
}

/// The newest 3D radar volumes for the sources visible in `area`.
#[tauri::command]
pub async fn radar_frame(area: Option<BoundingBox>, state: State<'_, Backend>) -> CmdResult<Vec<radar::VolumeInfo>> {
    state.radar_frame(area).await
}

/// One layer of a source's current volume, as raw bytes (an `ArrayBuffer` to the page).
#[tauri::command]
pub async fn radar_image(source: String, name: String, state: State<'_, Backend>) -> CmdResult<tauri::ipc::Response> {
    Ok(tauri::ipc::Response::new(state.radar_image(&source, &name).await?))
}

/// The flight an aircraft is on now, as tracked by the OpenSky Network.
#[tauri::command]
pub async fn opensky_track(icao24: String, state: State<'_, Backend>) -> CmdResult<Vec<opensky::TrackPoint>> {
    state.opensky_track(&icao24).await
}

/// The web inspector, for bug reports (Cmd/Ctrl+Alt+I); the context menu stays off.
/// Phones have their own: Safari's Develop menu, or chrome://inspect.
#[tauri::command]
pub fn open_devtools(window: tauri::WebviewWindow) {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    window.open_devtools();
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let _ = window;
}

// ---------------------------------------------------------------- other feeds

/// One aircraft by its ICAO 24-bit address.
#[tauri::command]
pub async fn adsb_hex(hex: String) -> CmdResult<adsblol::Snapshot> {
    adsblol::hex(&hex).await
}

/// Aircraft using a callsign, for when the Mode S address isn't known yet.
#[tauri::command]
pub async fn adsb_callsign(callsign: String) -> CmdResult<adsblol::Snapshot> {
    adsblol::callsign(&callsign).await
}

/// Places and addresses matching `query`, ranked toward `lat`/`lon` (the
/// view's centre) when given, named in `lang` where Photon knows it.
#[tauri::command]
pub async fn geocode(
    query: String,
    lat: Option<f64>,
    lon: Option<f64>,
    lang: Option<String>,
) -> CmdResult<Vec<geocode::Place>> {
    geocode::geocode(&query, lat, lon, lang.as_deref()).await
}

/// Ships in an area (degrees; `west > east` crosses the antimeridian) at a
/// deck.gl zoom, as many as VesselFinder shows at that zoom.
#[tauri::command]
pub async fn ships_in_view(west: f64, south: f64, east: f64, north: f64, zoom: f64) -> CmdResult<Vec<ships::Ship>> {
    ships::in_view(west, south, east, north, zoom).await
}

/// One ship's particulars, voyage and last report.
#[tauri::command]
pub async fn ship_details(mmsi: u32) -> CmdResult<ships::ShipDetails> {
    ships::details(mmsi).await
}

/// The weather at `lat`, `lon` now.
#[tauri::command]
pub async fn weather_at(lat: f64, lon: f64) -> CmdResult<weather::Weather> {
    weather::at(lat, lon).await
}

/// Taxiway, taxilane and runway centrelines within 4 km of an airport.
#[tauri::command]
pub async fn airport_taxiways(
    icao: String,
    lat: f64,
    lon: f64,
    state: State<'_, Backend>,
) -> CmdResult<Arc<Vec<taxiways::Line>>> {
    taxiways::airport(&icao, lat, lon, &state.dirs().cache).await
}

/// The numbered roads' stretches around a point, for the realistic map.
#[tauri::command]
pub async fn road_widths(lon: f64, lat: f64) -> CmdResult<Arc<Vec<roads::RoadPiece>>> {
    roads::widths(lon, lat).await
}

// ---------------------------------------------------------------- session, keys, export

#[tauri::command]
pub fn session_info(state: State<'_, Backend>) -> session::SessionInfo {
    state.session_info()
}

#[tauri::command]
pub async fn sign_in(email: String, password: String, state: State<'_, Backend>) -> CmdResult<session::SessionInfo> {
    state.sign_in(&email, password).await
}

#[tauri::command]
pub fn sign_out(state: State<'_, Backend>) -> session::SessionInfo {
    state.sign_out()
}

#[tauri::command]
pub fn get_keys(state: State<'_, Backend>) -> keys::Keys {
    state.keys()
}

/// Saves the keys and puts them in use.
#[tauri::command]
pub fn save_keys(keys: keys::Keys, state: State<'_, Backend>) -> CmdResult<keys::Keys> {
    state.save_keys(keys)
}

/// Writes an exported file to the Downloads folder; the full path written.
#[tauri::command]
pub async fn save_export(file_name: String, contents: String, state: State<'_, Backend>) -> CmdResult<String> {
    state.save_export(&file_name, &contents).map(|p| p.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------- trains

/// Trains in an area (degrees; `west > east` crosses the antimeridian) at a deck.gl zoom.
#[tauri::command]
pub async fn trains_in_view(west: f64, south: f64, east: f64, north: f64, zoom: f64) -> CmdResult<Vec<trains::Train>> {
    trains::in_view(west, south, east, north, zoom).await
}

/// Every stop of a train, for the sources that have its timetable; none for the others.
#[tauri::command]
pub async fn train_stops(id: String) -> CmdResult<Vec<trafikverket::Stop>> {
    trains::stops(&id).await
}

/// The whole way of a train along the track.
#[tauri::command]
pub async fn train_route(id: String) -> CmdResult<trafikverket::TrainRoute> {
    trains::route(id).await
}

/// The signal blocks starting in an area, and whether its track and signals are all here.
#[tauri::command]
pub async fn train_blocks(west: f64, south: f64, east: f64, north: f64) -> CmdResult<trains::TrainBlocks> {
    trains::blocks(west, south, east, north).await
}

/// The trains the map shows close up (by id): streamed live, like a followed one.
#[tauri::command]
pub fn watch_trains(ids: Vec<String>) {
    trains::watch(ids);
}

/// The operators the map leaves out: their trains aren't streamed live.
#[tauri::command]
pub fn hide_train_operators(names: Vec<String>) {
    trains::hide_operators(names);
}

/// Streams live updates of a train (its position and its stops) until
/// `unfollow_train` or another train is followed.
#[tauri::command]
pub fn follow_train(
    id: String,
    on_event: Channel<trafikverket::TrainEvent>,
    state: State<'_, Backend>,
) -> CmdResult<()> {
    forward(trains::follow(id, state.waker())?, on_event);
    Ok(())
}

#[tauri::command]
pub fn unfollow_train() {
    trains::unfollow();
}

/// A photo of the train and its operator's logo, as far as Wikimedia has them.
#[tauri::command]
pub async fn train_media(
    source: String,
    product: Option<String>,
    operator: Option<String>,
) -> CmdResult<train_media::TrainMedia> {
    Ok(train_media::lookup(&source, product, operator).await)
}

/// Dev panel: every train product seen so far, with the class taken for it.
#[tauri::command]
pub fn product_codes() -> Vec<trafikverket::ProductSeen> {
    trafikverket::product_codes()
}

/// Whether the saved Trafikverket key works: "ok", "invalid" or "none".
#[tauri::command]
pub async fn trafikverket_key_status() -> CmdResult<trafikverket::KeyStatus> {
    trafikverket::key_status().await
}

// ---------------------------------------------------------------- dev log

/// The calls recorded after `since` (an entry id), oldest first; none in release builds.
#[tauri::command]
pub fn dev_log(since: Option<u64>) -> Vec<devlog::Entry> {
    devlog::entries(since)
}

#[tauri::command]
pub fn dev_log_clear() {
    devlog::clear();
}

// ---------------------------------------------------------------- tile protocols

/// The answer of a tile protocol to `/{z}/{x}/{y}`: 400 for anything else,
/// 204 where there is no tile, 502 when it couldn't be fetched.
fn tile_response(tile: Option<Result<Option<Vec<u8>>, String>>) -> tauri::http::Response<Vec<u8>> {
    let (status, body) = match tile {
        None => (400, Vec::new()),
        Some(Ok(None)) => (204, Vec::new()),
        Some(Ok(Some(bytes))) => (200, bytes),
        // the backend has logged it
        Some(Err(_)) => (502, Vec::new()),
    };
    tauri::http::Response::builder()
        .status(status)
        .header("Content-Type", "application/x-protobuf")
        .header("Access-Control-Allow-Origin", "*")
        .body(body)
        .unwrap_or_default()
}

/// The three numbers of a tile's path, `/{z}/{x}/{y}`.
fn tile_of(path: &str) -> Option<(u32, u32, u32)> {
    let parts: Vec<u32> = path.trim_matches('/').split('/').filter_map(|p| p.parse().ok()).collect();
    let [z, x, y] = parts[..] else { return None };
    Some((z, x, y))
}

/// The `railtiles:` protocol the map draws the network from: `/{z}/{x}/{y}`
/// (and at the platforms' zoom, stations' platforms).
pub async fn rail_tiles(path: &str) -> tauri::http::Response<Vec<u8>> {
    let tile = match tile_of(path) {
        Some((z, x, y)) => Some(rustradar_backend::rail::tile(z, x, y).await.map(|t| t.map(|b| b.to_vec()))),
        None => None,
    };
    tile_response(tile)
}

/// The `buildingtiles:` protocol the map draws far-off buildings from: `/{z}/{x}/{y}`.
pub async fn building_tiles(path: &str) -> tauri::http::Response<Vec<u8>> {
    let tile = match tile_of(path) {
        Some((z, x, y)) => Some(rustradar_backend::building_lod::tile(z, x, y).await),
        None => None,
    };
    tile_response(tile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_paths_are_three_numbers() {
        assert_eq!(tile_of("/14/8802/5373"), Some((14, 8802, 5373)));
        assert_eq!(tile_of("14/8802/5373/"), Some((14, 8802, 5373)));
        assert_eq!(tile_of("/14/8802"), None);
        assert_eq!(tile_of("/14/8802/5373/1"), None);
        assert_eq!(tile_of("/14/x/5373"), None);
    }

    #[test]
    fn tile_answers_keep_their_statuses() {
        let status = |tile| tile_response(tile).status().as_u16();
        assert_eq!(status(None), 400);
        assert_eq!(status(Some(Ok(None))), 204);
        assert_eq!(status(Some(Err("unreachable".into()))), 502);
        let found = tile_response(Some(Ok(Some(vec![1, 2, 3]))));
        assert_eq!(found.status().as_u16(), 200);
        assert_eq!(found.headers()["Content-Type"], "application/x-protobuf");
        assert_eq!(found.headers()["Access-Control-Allow-Origin"], "*");
        assert_eq!(found.body(), &[1, 2, 3]);
    }
}
