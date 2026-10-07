//! Airlines and airports, cached on disk: the lists change rarely and the
//! airport list is a 2 MB download.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use fr24::Fr24;
use serde::{Deserialize, Serialize};

/// Refetch after a week; a stale copy is still used if the refetch fails.
const MAX_AGE_S: i64 = 7 * 86_400;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Airline {
    pub name: String,
    pub iata: String,
    pub icao: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Airport {
    /// Flightradar24's airport id, as in a flight's schedule.
    pub id: u32,
    pub name: String,
    pub iata: String,
    pub icao: String,
    pub city: String,
    pub country: String,
    pub lat: f64,
    pub lon: f64,
    /// A relative size/importance score.
    pub size: i64,
    /// IANA timezone name, e.g. `Europe/London`.
    pub timezone: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceData {
    /// Unix seconds.
    pub fetched_at: i64,
    pub airlines: Vec<Airline>,
    pub airports: Vec<Airport>,
}

fn now_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// The cached copy if it is fresh, otherwise a new download (falling back to
/// the stale copy if the download fails).
pub async fn load(fr24: &Fr24, cache_file: &Path) -> Result<ReferenceData, String> {
    let cached = std::fs::read(cache_file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ReferenceData>(&bytes).ok());
    if let Some(data) = &cached
        && now_s() - data.fetched_at < MAX_AGE_S
    {
        return Ok(data.clone());
    }
    match fetch(fr24).await {
        Ok(data) => {
            if let Some(dir) = cache_file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(bytes) = serde_json::to_vec(&data) {
                let _ = std::fs::write(cache_file, bytes);
            }
            Ok(data)
        }
        Err(e) => cached.ok_or(e),
    }
}

async fn fetch(fr24: &Fr24) -> Result<ReferenceData, String> {
    let (airlines, airports) = tokio::try_join!(fr24.airlines(), fr24.airports())
        .map_err(|e| format!("couldn't download airline and airport lists: {e}"))?;
    Ok(ReferenceData {
        fetched_at: now_s(),
        airlines: airlines
            .rows
            .into_iter()
            .filter(|a| !a.icao.is_empty())
            .map(|a| Airline {
                name: a.name,
                iata: a.iata,
                icao: a.icao,
            })
            .collect(),
        airports: airports
            .rows
            .into_iter()
            .map(|a| Airport {
                id: a.id,
                name: a.name,
                iata: a.iata,
                icao: a.icao,
                city: a.city,
                country: a.country,
                lat: a.lat,
                lon: a.lon,
                size: a.size,
                timezone: a.timezone.map(|t| t.name).unwrap_or_default(),
            })
            .collect(),
    })
}
