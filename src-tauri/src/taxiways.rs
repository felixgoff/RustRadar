//! Taxiway, taxilane and runway centrelines around an airport, from
//! OpenStreetMap through the Overpass API: what the motion model keeps
//! aircraft taxiing on, since their reported positions wander a few metres
//! either side of the lines and cut the corners.
//!
//! Only asked for an airport where ground traffic is being watched up close,
//! one airport per call; answers are kept in memory and on disk (airports'
//! layouts change slowly), and a failure gives an empty network, so the
//! aircraft are simply drawn as reported. The data is © OpenStreetMap
//! contributors (ODbL).

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::CmdResult;

/// The main public instance, then a mirror if it fails.
const URLS: [&str; 2] = [
    "https://overpass-api.de/api/interpreter",
    "https://overpass.kumi.systems/api/interpreter",
];
const USER_AGENT: &str = concat!(
    "RustRadar/",
    env!("CARGO_PKG_VERSION"),
    " (desktop flight radar; airport taxiways for ground traffic; +https://github.com/felixgoff/RustRadar)"
);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Radius around the airport's reference point, metres: big airports reach about this far.
const RADIUS_M: f64 = 4_000.0;
/// Refetch after a month; a stale copy is still used if the refetch fails.
const MAX_AGE_S: i64 = 30 * 86_400;
/// After a failure, don't ask again for the same airport for this long.
const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Taxiway,
    Taxilane,
    Runway,
}

/// One OSM way's centreline.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub kind: Kind,
    /// The taxiway's or runway's designator, e.g. `A3` or `04L/22R`, where tagged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    /// `[lon, lat]` pairs, rounded to 1e-7° (about a centimetre).
    pub points: Vec<[f64; 2]>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cached {
    /// Unix seconds.
    fetched_at: i64,
    lines: Vec<Line>,
}

#[derive(Default)]
struct Cache {
    /// By ICAO code.
    lines: HashMap<String, std::sync::Arc<Vec<Line>>>,
    /// No calls for that airport before this moment: the last one failed.
    failed: HashMap<String, Instant>,
}

fn state() -> &'static tokio::sync::Mutex<Cache> {
    static CACHE: OnceLock<tokio::sync::Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c);
    }
    let c = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .gzip(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(CLIENT.get_or_init(|| c))
}

fn now_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// The Overpass QL query for the centrelines within `RADIUS_M` of a point.
pub fn query(lat: f64, lon: f64) -> String {
    format!(
        "[out:json][timeout:25];way[\"aeroway\"~\"^(taxiway|runway|taxilane)$\"](around:{RADIUS_M:.0},{lat:.6},{lon:.6});out geom;"
    )
}

/// `application/x-www-form-urlencoded` encoding of one value.
fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The centrelines in an Overpass `out geom` JSON answer. Ways with fewer than
/// two located points, other aeroways and areas (closed runway polygons tagged
/// `area=yes`) are left out.
pub fn parse(body: &str) -> Result<Vec<Line>, String> {
    let json: Value = serde_json::from_str(body).map_err(|e| format!("bad Overpass answer: {e}"))?;
    let elements = json["elements"].as_array().ok_or("the Overpass answer has no elements")?;
    let round = |v: f64| (v * 1e7).round() / 1e7;
    Ok(elements
        .iter()
        .filter_map(|el| {
            if el["type"].as_str() != Some("way") {
                return None;
            }
            let tags = &el["tags"];
            let kind = match tags["aeroway"].as_str()? {
                "taxiway" => Kind::Taxiway,
                "taxilane" => Kind::Taxilane,
                "runway" => Kind::Runway,
                _ => return None,
            };
            if tags["area"].as_str() == Some("yes") {
                return None;
            }
            let points: Vec<[f64; 2]> = el["geometry"]
                .as_array()?
                .iter()
                .filter_map(|p| {
                    let lat = p["lat"].as_f64().filter(|v| v.abs() <= 90.0)?;
                    let lon = p["lon"].as_f64().filter(|v| v.abs() <= 180.0)?;
                    Some([round(lon), round(lat)])
                })
                .collect();
            if points.len() < 2 {
                return None;
            }
            let r#ref = tags["ref"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            Some(Line { kind, r#ref, points })
        })
        .collect())
}

async fn fetch(lat: f64, lon: f64) -> Result<Vec<Line>, String> {
    let http = client()?;
    let body = format!("data={}", form_encode(&query(lat, lon)));
    let mut last_error = String::new();
    for url in URLS {
        let answer = http
            .post(url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body.clone())
            .send()
            .await
            .and_then(|r| r.error_for_status());
        let text = match answer {
            Ok(r) => r.text().await,
            Err(e) => Err(e),
        };
        match text {
            Ok(text) => return parse(&text),
            Err(e) => last_error = format!("couldn't download taxiways: {e}"),
        }
    }
    Err(last_error)
}

/// The cached copy if it is fresh, otherwise a new download (falling back to
/// the stale copy if the download fails).
async fn load(lat: f64, lon: f64, cache_file: Option<&Path>) -> Result<Vec<Line>, String> {
    let cached = cache_file
        .and_then(|f| std::fs::read(f).ok())
        .and_then(|bytes| serde_json::from_slice::<Cached>(&bytes).ok());
    if let Some(data) = &cached
        && now_s() - data.fetched_at < MAX_AGE_S
    {
        return Ok(data.lines.clone());
    }
    match fetch(lat, lon).await {
        Ok(lines) => {
            if let Some(file) = cache_file {
                if let Some(dir) = file.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let data = Cached { fetched_at: now_s(), lines };
                if let Ok(bytes) = serde_json::to_vec(&data) {
                    let _ = std::fs::write(file, bytes);
                }
                return Ok(data.lines);
            }
            Ok(lines)
        }
        Err(e) => cached.map(|c| c.lines).ok_or(e),
    }
}

/// An ICAO code as used for the cache: four letters or digits.
fn icao(code: &str) -> Option<String> {
    let code = code.trim().to_ascii_uppercase();
    (code.len() == 4 && code.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(code)
}

/// Taxiway, taxilane and runway centrelines within 4 km of an airport (its
/// ICAO code and reference point), from OpenStreetMap. Cached in memory and
/// on disk for a month; empty when they can't be had (offline, Overpass busy).
#[tauri::command]
pub async fn airport_taxiways(
    icao: String,
    lat: f64,
    lon: f64,
    app: AppHandle,
) -> CmdResult<std::sync::Arc<Vec<Line>>> {
    let code = self::icao(&icao).ok_or("not an ICAO airport code")?;
    if !(lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0) {
        return Err("not a position".into());
    }
    // one airport at a time: a second ask for the same one waits for the first answer
    let mut cache = state().lock().await;
    if let Some(lines) = cache.lines.get(&code) {
        return Ok(lines.clone());
    }
    if cache.failed.get(&code).is_some_and(|until| Instant::now() < *until) {
        return Ok(Default::default());
    }
    let file = app
        .path()
        .app_cache_dir()
        .ok()
        .map(|dir| dir.join("taxiways").join(format!("{code}.json")));
    match load(lat, lon, file.as_deref()).await {
        Ok(lines) => {
            let lines = std::sync::Arc::new(lines);
            cache.failed.remove(&code);
            cache.lines.insert(code, lines.clone());
            Ok(lines)
        }
        Err(e) => {
            eprintln!("taxiways for {code}: {e}");
            cache.failed.insert(code, Instant::now() + RETRY_AFTER);
            Ok(Default::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "version": 0.6,
      "elements": [
        {"type": "way", "id": 1, "bounds": {},
         "geometry": [{"lat": 55.6181234567, "lon": 12.6501}, {"lat": 55.6190, "lon": 12.6510}, {"lat": 55.6200, "lon": 12.6510}],
         "tags": {"aeroway": "taxiway", "ref": " A3 "}},
        {"type": "way", "id": 2,
         "geometry": [{"lat": 55.60, "lon": 12.60}, {"lat": 55.61, "lon": 12.62}],
         "tags": {"aeroway": "runway", "ref": "04L/22R"}},
        {"type": "way", "id": 3,
         "geometry": [{"lat": 55.60, "lon": 12.60}, {"lat": 55.60, "lon": 12.61}, {"lat": 55.61, "lon": 12.61}, {"lat": 55.60, "lon": 12.60}],
         "tags": {"aeroway": "runway", "area": "yes"}},
        {"type": "way", "id": 4,
         "geometry": [{"lat": 55.61, "lon": 12.63}, {"lat": 55.611, "lon": 12.631}],
         "tags": {"aeroway": "taxilane"}},
        {"type": "way", "id": 5,
         "geometry": [{"lat": 55.61, "lon": 12.63}],
         "tags": {"aeroway": "taxiway", "ref": "B"}},
        {"type": "way", "id": 6,
         "geometry": [{"lat": 55.61, "lon": 12.63}, {"lat": 55.62, "lon": 12.64}],
         "tags": {"aeroway": "apron"}},
        {"type": "node", "id": 7, "lat": 55.61, "lon": 12.63, "tags": {"aeroway": "holding_position"}}
      ]
    }"#;

    #[test]
    fn keeps_centrelines() {
        let lines = parse(SAMPLE).unwrap();
        assert_eq!(lines.len(), 3, "areas, single points, aprons and nodes are dropped");
        assert_eq!(lines[0].kind, Kind::Taxiway);
        assert_eq!(lines[0].r#ref.as_deref(), Some("A3"));
        assert_eq!(lines[0].points.len(), 3);
        // [lon, lat], rounded to 1e-7
        assert_eq!(lines[0].points[0], [12.6501, 55.6181235]);
        assert_eq!(lines[1].kind, Kind::Runway);
        assert_eq!(lines[1].r#ref.as_deref(), Some("04L/22R"));
        assert_eq!(lines[2].kind, Kind::Taxilane);
        assert_eq!(lines[2].r#ref, None);
    }

    #[test]
    fn serializes_compactly() {
        let lines = parse(SAMPLE).unwrap();
        let json = serde_json::to_value(&lines[2]).unwrap();
        assert_eq!(json["kind"], "taxilane");
        assert!(json.get("ref").is_none());
        assert_eq!(json["points"][0][0], 12.63);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("<html>busy</html>").is_err());
        assert!(parse("{}").is_err());
        assert!(parse(r#"{"elements": []}"#).unwrap().is_empty());
    }

    #[test]
    fn encodes_the_query() {
        let q = query(55.618, 12.656);
        assert!(q.contains("(around:4000,55.618000,12.656000)"));
        assert!(q.ends_with("out geom;"));
        assert_eq!(form_encode("a b[\"x\"~^]:;"), "a+b%5B%22x%22%7E%5E%5D%3A%3B");
        assert_eq!(icao(" ekch ").as_deref(), Some("EKCH"));
        assert_eq!(icao("CPH"), None);
    }

    /// Asks the real Overpass API; run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn downloads_copenhagen() {
        let lines = fetch(55.618, 12.656).await.unwrap();
        assert!(lines.iter().filter(|l| l.kind == Kind::Taxiway).count() > 50);
        assert!(lines.iter().any(|l| l.kind == Kind::Runway));
    }
}
