//! The weather where the map is looking, for the realistic map's light: how
//! cloudy and foggy it is, and whether it rains or snows. From MET Norway's
//! Locationforecast (api.met.no, CC BY 4.0), which asks to be told who is
//! calling (the client's User-Agent) and to be asked no more often than its
//! answers go stale: each place is kept for ten minutes.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::CmdResult;

const FORECAST: &str = "https://api.met.no/weatherapi/locationforecast/2.0/complete";
/// A place's weather is kept this long (MET's own answers are good for about half an hour).
const FRESH: Duration = Duration::from_secs(600);
/// Places this close share their weather: about 10 km.
const GRID_DEG: f64 = 0.1;
const MAX_CACHED: usize = 256;

/// The weather now, as the realistic map needs it.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Weather {
    /// How much of the sky is cloud, 0 to 1.
    pub cloud: f64,
    /// Of it, low cloud (the kind that hides the sun), 0 to 1.
    pub cloud_low: f64,
    /// Fog over the place, 0 to 1.
    pub fog: f64,
    /// Rain or snow expected in the next hour, mm.
    pub precipitation: f64,
    /// MET's symbol for the next hour: `clearsky_day`, `partlycloudy_night`, `rain`, `snow`, `fog`, …
    pub symbol: String,
    /// The air's temperature, °C.
    pub temperature: f64,
}

/// The weather in a Locationforecast answer: its first (current) time step.
pub fn parse(body: &Value) -> Option<Weather> {
    let step = body["properties"]["timeseries"].get(0)?;
    let now = &step["data"]["instant"]["details"];
    let next = &step["data"]["next_1_hours"];
    let fraction = |v: &Value| v.as_f64().map(|p| (p / 100.0).clamp(0.0, 1.0));
    Some(Weather {
        cloud: fraction(&now["cloud_area_fraction"])?,
        cloud_low: fraction(&now["cloud_area_fraction_low"]).unwrap_or(0.0),
        fog: fraction(&now["fog_area_fraction"]).unwrap_or(0.0),
        precipitation: next["details"]["precipitation_amount"].as_f64().unwrap_or(0.0).max(0.0),
        symbol: next["summary"]["symbol_code"].as_str().unwrap_or("").to_owned(),
        temperature: now["air_temperature"].as_f64().unwrap_or(10.0),
    })
}

/// The grid cell a place's weather is kept under.
fn cell(lat: f64, lon: f64) -> (i32, i32) {
    ((lat / GRID_DEG).round() as i32, (lon / GRID_DEG).round() as i32)
}

type Cache = HashMap<(i32, i32), (Instant, Weather)>;
fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The weather at `lat`, `lon` now.
#[tauri::command]
pub async fn weather_at(lat: f64, lon: f64) -> CmdResult<Weather> {
    if !lat.is_finite() || !lon.is_finite() || lat.abs() > 90.0 {
        return Err("no such place".into());
    }
    let key = cell(lat, lon);
    if let Some((at, weather)) = cache().lock().map_err(|e| e.to_string())?.get(&key)
        && at.elapsed() < FRESH
    {
        return Ok(weather.clone());
    }
    // the cell's middle, to two decimals as MET asks (no more than four)
    let (clat, clon) = (key.0 as f64 * GRID_DEG, key.1 as f64 * GRID_DEG);
    let url = format!("{FORECAST}?lat={clat:.2}&lon={clon:.2}");
    let client = crate::rail::http().ok_or("no HTTP client")?;
    let call = crate::devlog::start("MET Norway", "GET", &url, None);
    let answer = async {
        let response = client.get(&url).send().await.map_err(|e| format!("can't reach MET Norway: {e}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("MET Norway answered {status}"));
        }
        response.bytes().await.map_err(|e| e.to_string())
    }
    .await;
    if let Some(c) = call {
        match &answer {
            Ok(body) => c.finish(200, body),
            Err(e) => c.fail(e),
        }
    }
    let body: Value = serde_json::from_slice(&answer?).map_err(|e| e.to_string())?;
    let weather = parse(&body).ok_or("MET Norway's answer had no weather in it")?;
    let mut cache = cache().lock().map_err(|e| e.to_string())?;
    if cache.len() >= MAX_CACHED {
        cache.retain(|_, (at, _)| at.elapsed() < FRESH);
    }
    cache.insert(key, (Instant::now(), weather.clone()));
    Ok(weather)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_weather_now_is_read_from_the_first_time_step() {
        let body = json!({"properties": {"timeseries": [
            {"time": "2026-10-16T17:00:00Z", "data": {
                "instant": {"details": {"cloud_area_fraction": 99.5, "cloud_area_fraction_low": 95.6, "fog_area_fraction": 0.0, "air_temperature": 15.7}},
                "next_1_hours": {"summary": {"symbol_code": "cloudy"}, "details": {"precipitation_amount": 0.2}}
            }},
            {"time": "2026-10-16T18:00:00Z", "data": {"instant": {"details": {"cloud_area_fraction": 0.0}}}}
        ]}});
        let w = parse(&body).unwrap();
        assert!((w.cloud - 0.995).abs() < 1e-9);
        assert!((w.cloud_low - 0.956).abs() < 1e-9);
        assert_eq!(w.fog, 0.0);
        assert_eq!(w.symbol, "cloudy");
        assert!((w.precipitation - 0.2).abs() < 1e-9);
        assert!((w.temperature - 15.7).abs() < 1e-9);
    }

    #[test]
    fn an_answer_without_weather_is_none() {
        assert_eq!(parse(&json!({})), None);
        assert_eq!(parse(&json!({"properties": {"timeseries": []}})), None);
    }

    #[test]
    fn nearby_places_share_a_cell() {
        assert_eq!(cell(55.604, 13.001), cell(55.596, 12.996));
        assert_ne!(cell(55.604, 13.001), cell(55.704, 13.001));
    }
}
