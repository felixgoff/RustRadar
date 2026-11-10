//! The commands and protocols whose work moved to `rustradar-backend`: each
//! is the backend's function behind the name and arguments the frontend calls.

use std::sync::Arc;

use rustradar_backend::{adsblol, devlog, geocode, session, ships, train_media, weather};
use tauri::State;

use crate::{AppState, CmdResult};

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

#[tauri::command]
pub fn session_info(state: State<'_, AppState>) -> session::SessionInfo {
    session::session_info(&state.client())
}

#[tauri::command]
pub async fn sign_in(
    email: String,
    password: String,
    state: State<'_, AppState>,
) -> CmdResult<session::SessionInfo> {
    let (fr24, session) = session::sign_in(&state.client(), &email, password).await?;
    *state.fr24.write().unwrap() = Arc::new(fr24);
    Ok(session)
}

#[tauri::command]
pub fn sign_out(state: State<'_, AppState>) -> session::SessionInfo {
    let (fr24, session) = session::sign_out(&state.client());
    *state.fr24.write().unwrap() = Arc::new(fr24);
    session
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

/// The calls recorded after `since` (an entry id), oldest first; none in release builds.
#[tauri::command]
pub fn dev_log(since: Option<u64>) -> Vec<devlog::Entry> {
    devlog::entries(since)
}

#[tauri::command]
pub fn dev_log_clear() {
    devlog::clear();
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
