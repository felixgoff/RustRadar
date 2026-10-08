//! OpenSky Network flight tracks: the path an aircraft has flown so far,
//! built from the community's ADS-B receivers. It often reaches back further
//! than Flightradar24's trail, so it fills in the start of a route.
//!
//! The anonymous allowance is small (about 400 calls a day, shared by every
//! endpoint), so tracks are cached per aircraft and only fetched on demand.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

const URL: &str = "https://opensky-network.org/api/tracks/all";
/// A track grows slowly: reuse one for this long.
const TTL: Duration = Duration::from_secs(240);
const FEET_PER_METRE: f64 = 3.280_84;

#[derive(Clone, Serialize)]
pub struct TrackPoint {
    /// Unix seconds
    pub timestamp: i64,
    pub latitude: f32,
    pub longitude: f32,
    /// Barometric altitude, feet
    pub altitude: i32,
    /// Degrees clockwise from north
    pub track: u32,
    pub on_ground: bool,
}

#[derive(Default)]
pub struct Cache {
    tracks: HashMap<String, (Instant, Vec<TrackPoint>)>,
    /// No calls before this moment: OpenSky asked us to back off.
    blocked_until: Option<Instant>,
}

pub fn is_icao24(s: &str) -> bool {
    s.len() == 6 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Points of a `/tracks/all` response; entries without a position are skipped.
pub fn parse(body: &str) -> Result<Vec<TrackPoint>, String> {
    let json: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let path = json["path"].as_array().ok_or("response has no path")?;
    Ok(path
        .iter()
        .filter_map(|p| {
            let p = p.as_array()?;
            Some(TrackPoint {
                timestamp: p.first()?.as_i64()?,
                latitude: p.get(1)?.as_f64()? as f32,
                longitude: p.get(2)?.as_f64()? as f32,
                altitude: (p.get(3)?.as_f64().unwrap_or(0.0) * FEET_PER_METRE).round() as i32,
                track: p
                    .get(4)
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    .rem_euclid(360.0) as u32,
                on_ground: p.get(5).and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect())
}

/// The track of the aircraft's current flight; empty when OpenSky has none.
pub async fn track(
    http: &reqwest::Client,
    cache: &mut Cache,
    icao24: &str,
) -> Result<Vec<TrackPoint>, String> {
    if !is_icao24(icao24) {
        return Err("not an ICAO 24-bit address".into());
    }
    let icao24 = icao24.to_ascii_lowercase();
    if let Some((at, points)) = cache.tracks.get(&icao24)
        && at.elapsed() < TTL
    {
        return Ok(points.clone());
    }
    if let Some(until) = cache.blocked_until {
        if Instant::now() < until {
            return Err("OpenSky asked us to slow down".into());
        }
        cache.blocked_until = None;
    }
    let response = http
        .get(URL)
        .query(&[("icao24", icao24.as_str()), ("time", "0")])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    match response.status().as_u16() {
        200 => {}
        // no flight in progress, or one OpenSky never saw
        404 => {
            cache.tracks.insert(icao24, (Instant::now(), Vec::new()));
            return Ok(Vec::new());
        }
        429 => {
            let wait = response
                .headers()
                .get("x-rate-limit-retry-after-seconds")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(600)
                .min(86_400);
            cache.blocked_until = Some(Instant::now() + Duration::from_secs(wait));
            return Err("OpenSky's daily allowance is used up".into());
        }
        status => return Err(format!("OpenSky answered {status}")),
    }
    let points = parse(&response.text().await.map_err(|e| e.to_string())?)?;
    cache
        .tracks
        .insert(icao24, (Instant::now(), points.clone()));
    Ok(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_track() {
        let body = r#"{"icao24":"39de4b","startTime":1.0,"endTime":2.0,"path":[
            [1791399022,48.7212,2.369,304,254,false],
            [1791399030,null,null,null,null,false],
            [1791399047,48.7162,2.3415,609.6,-1,true]]}"#;
        let points = parse(body).unwrap();
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].altitude, 997);
        assert_eq!(points[1].altitude, 2000);
        assert_eq!(points[1].track, 359);
        assert!(points[1].on_ground);
    }

    #[test]
    fn checks_addresses() {
        assert!(is_icao24("39DE4b"));
        assert!(!is_icao24("39de4"));
        assert!(!is_icao24("zzzzzz"));
    }
}
