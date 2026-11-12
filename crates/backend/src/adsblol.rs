//! adsb.lol: a community ADS-B network with an open API. Its aircraft carry
//! what the transponders themselves report, which Flightradar24's anonymous
//! feed leaves out: the nose's true heading, bank angle, turn and vertical
//! rates, air data (IAS, TAS, Mach, wind, temperature) and the autopilot's
//! selected altitude and heading.
//!
//! Only the selected aircraft is asked for. The API is free and
//! unauthenticated for now (an API key is announced for the future), so it is
//! used politely: a minimum gap between calls, a short cache, and backoff when
//! it errs or asks us to slow down. The data is ODbL
//! and must be credited wherever it is shown.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::CmdResult;

const BASE: &str = "https://api.adsb.lol/v2";
const USER_AGENT: &str = concat!(
    "RustRadar/",
    env!("CARGO_PKG_VERSION"),
    " (desktop flight radar; +https://github.com/felixgoff/RustRadar)"
);
/// Minimum gap between calls, and how long an answer is reused.
const GAP: Duration = Duration::from_secs(2);
const TTL: Duration = Duration::from_millis(1_500);
/// Backoff after a failure: doubling from this, up to the maximum.
const BACKOFF_BASE: Duration = Duration::from_secs(10);
const BACKOFF_MAX: Duration = Duration::from_secs(600);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
/// Cache entries kept at most (old ones are dropped first).
const CACHE_LIMIT: usize = 32;

// One aircraft as adsb.lol reports it, and an answer of them: `rustradar_core::model`.
pub use rustradar_core::model::{AdsbAircraft as Aircraft, AdsbSnapshot as Snapshot};

/// Responses kept for reuse, and when we may call again.
#[derive(Default)]
pub struct Cache {
    entries: HashMap<String, (Instant, Snapshot)>,
    /// The earliest moment the next call may go out.
    next: Option<Instant>,
    /// No calls before this: the last ones failed or were refused.
    blocked_until: Option<Instant>,
    failures: u32,
}

impl Cache {
    fn fresh(&self, key: &str) -> Option<Snapshot> {
        let (at, snapshot) = self.entries.get(key)?;
        (at.elapsed() < TTL).then(|| snapshot.clone())
    }

    fn store(&mut self, key: String, snapshot: Snapshot) {
        if self.entries.len() >= CACHE_LIMIT
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(key, (Instant::now(), snapshot));
    }

    /// Reserves the next call slot; returns how long to wait for it.
    fn reserve(&mut self) -> Duration {
        self.reserve_at(Instant::now())
    }

    /// [`Self::reserve`] at `now`.
    fn reserve_at(&mut self, now: Instant) -> Duration {
        let at = self.next.filter(|t| *t > now).unwrap_or(now);
        self.next = Some(at + GAP);
        at - now
    }

    fn failed(&mut self, retry_after: Option<Duration>) {
        self.failed_at(Instant::now(), retry_after);
    }

    /// [`Self::failed`] at `now`.
    fn failed_at(&mut self, now: Instant, retry_after: Option<Duration>) {
        self.failures = self.failures.saturating_add(1);
        let doubling = BACKOFF_BASE.saturating_mul(1 << self.failures.min(8).saturating_sub(1));
        let wait = retry_after.unwrap_or(doubling).min(BACKOFF_MAX);
        self.blocked_until = Some(now + wait);
    }

    fn succeeded(&mut self) {
        self.failures = 0;
        self.blocked_until = None;
    }
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
    let c = fr24::client_builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .gzip(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(CLIENT.get_or_init(|| c))
}

fn text(v: &Value) -> String {
    v.as_str().map(|s| s.trim().to_owned()).unwrap_or_default()
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite())
}

fn int(v: &Value) -> Option<i32> {
    num(v).map(|x| x.round() as i32)
}

fn float(v: &Value) -> Option<f32> {
    num(v).map(|x| x as f32)
}

/// Parses one aircraft; `now_ms` is the response's clock.
fn aircraft(a: &Value, now_ms: u64) -> Option<Aircraft> {
    let hex = text(&a["hex"]).to_ascii_lowercase();
    if hex.is_empty() {
        return None;
    }
    let ago = |field: &str| num(&a[field]).map(|s| (s.max(0.0) * 1000.0).round() as u64);
    let lat = num(&a["lat"]).filter(|x| x.abs() <= 90.0);
    let lon = num(&a["lon"]).filter(|x| x.abs() <= 180.0);
    let positioned = lat.is_some() && lon.is_some();
    let on_ground = a["alt_baro"].as_str() == Some("ground");
    let mlat_fields = a["mlat"].as_array();
    let source = text(&a["type"]);
    let emergency = text(&a["emergency"]);
    Some(Aircraft {
        hex,
        callsign: text(&a["flight"]),
        reg: text(&a["r"]),
        typecode: text(&a["t"]),
        mlat: source == "mlat"
            || mlat_fields.is_some_and(|f| f.iter().any(|v| v.as_str() == Some("lat"))),
        source,
        lat: lat.filter(|_| positioned),
        lon: lon.filter(|_| positioned),
        position_ms: if positioned {
            Some(now_ms.saturating_sub(ago("seen_pos").unwrap_or(0)))
        } else {
            None
        },
        seen_ms: now_ms.saturating_sub(ago("seen").unwrap_or(0)),
        on_ground,
        alt_baro: if on_ground { None } else { int(&a["alt_baro"]) },
        alt_geom: int(&a["alt_geom"]),
        gs: float(&a["gs"]),
        track: float(&a["track"]).map(|t| t.rem_euclid(360.0)),
        true_heading: float(&a["true_heading"]).map(|t| t.rem_euclid(360.0)),
        mag_heading: float(&a["mag_heading"]).map(|t| t.rem_euclid(360.0)),
        roll: float(&a["roll"]),
        track_rate: float(&a["track_rate"]),
        baro_rate: int(&a["baro_rate"]),
        geom_rate: int(&a["geom_rate"]),
        ias: int(&a["ias"]),
        tas: int(&a["tas"]),
        mach: float(&a["mach"]),
        wind_dir: int(&a["wd"]),
        wind_speed: int(&a["ws"]),
        oat: int(&a["oat"]),
        tat: int(&a["tat"]),
        nav_altitude_mcp: int(&a["nav_altitude_mcp"]),
        nav_altitude_fms: int(&a["nav_altitude_fms"]),
        nav_heading: float(&a["nav_heading"]).map(|t| t.rem_euclid(360.0)),
        nav_qnh: float(&a["nav_qnh"]),
        nav_modes: a["nav_modes"]
            .as_array()
            .map(|m| {
                m.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        squawk: text(&a["squawk"]),
        emergency: if emergency == "none" {
            String::new()
        } else {
            emergency
        },
        category: text(&a["category"]),
    })
}

/// A `/v2/...` response: `{ ac: [...], now, ... }`.
pub fn parse(body: &str) -> Result<Snapshot, String> {
    let json: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let now_ms = json["now"]
        .as_f64()
        .filter(|n| n.is_finite() && *n > 0.0)
        .ok_or("response has no time")? as u64;
    let list = match &json["ac"] {
        Value::Array(list) => list.as_slice(),
        Value::Null => &[],
        _ => return Err("response has no aircraft list".into()),
    };
    Ok(Snapshot {
        now_ms,
        aircraft: list.iter().filter_map(|a| aircraft(a, now_ms)).collect(),
    })
}

/// Calls `url` within the politeness rules, reusing a recent answer under `key`.
async fn fetch(url: String, key: String) -> Result<Snapshot, String> {
    let wait = {
        let mut cache = state().lock().await;
        if let Some(hit) = cache.fresh(&key) {
            return Ok(hit);
        }
        if let Some(until) = cache.blocked_until {
            let left = until.saturating_duration_since(Instant::now());
            if !left.is_zero() {
                return Err(format!(
                    "adsb.lol is resting; retrying in {} s",
                    left.as_secs().max(1)
                ));
            }
        }
        cache.reserve()
    };
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
    let result = request(&url).await;
    let mut cache = state().lock().await;
    match result {
        Ok(snapshot) => {
            cache.succeeded();
            cache.store(key, snapshot.clone());
            Ok(snapshot)
        }
        Err((e, retry_after)) => {
            cache.failed(retry_after);
            Err(e)
        }
    }
}

async fn request(url: &str) -> Result<Snapshot, (String, Option<Duration>)> {
    let response = client()
        .map_err(|e| (e, None))?
        .get(url)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|e| (format!("can't reach adsb.lol: {e}"), None))?;
    let status = response.status();
    if !status.is_success() {
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let message = if status.as_u16() == 429 {
            "adsb.lol asked us to slow down".to_owned()
        } else {
            format!("adsb.lol answered {status}")
        };
        return Err((message, retry_after));
    }
    let body = response.text().await.map_err(|e| (e.to_string(), None))?;
    parse(&body).map_err(|e| (e, None))
}

/// One aircraft by its ICAO 24-bit address.
pub async fn hex(hex: &str) -> CmdResult<Snapshot> {
    let hex = hex.trim().to_ascii_lowercase();
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("not an ICAO 24-bit address".into());
    }
    fetch(format!("{BASE}/hex/{hex}"), format!("hex:{hex}")).await
}

/// Aircraft using a callsign, for when the Mode S address isn't known yet.
pub async fn callsign(callsign: &str) -> CmdResult<Snapshot> {
    let callsign = callsign.trim().to_ascii_uppercase();
    if callsign.is_empty()
        || callsign.len() > 8
        || !callsign.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return Err("not a callsign".into());
    }
    fetch(
        format!("{BASE}/callsign/{callsign}"),
        format!("callsign:{callsign}"),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRANKFURT: &str = include_str!("../tests/data/adsblol-frankfurt.json");

    #[test]
    fn parses_the_sample() {
        let s = parse(FRANKFURT).unwrap();
        assert_eq!(s.now_ms, 1_791_557_054_001);
        assert_eq!(s.aircraft.len(), 56);

        let uae = s.aircraft.iter().find(|a| a.hex == "89617e").unwrap();
        assert_eq!(uae.callsign, "UAE1CL");
        assert_eq!(uae.reg, "A6-EGF");
        assert_eq!(uae.typecode, "B77W");
        assert_eq!(uae.source, "adsb_icao");
        assert!(!uae.mlat && !uae.on_ground);
        assert_eq!(uae.alt_baro, Some(35_000));
        assert_eq!(uae.alt_geom, Some(35_875));
        assert_eq!(uae.true_heading, Some(110.35));
        assert_eq!(uae.track, Some(108.07));
        assert_eq!(uae.roll, Some(0.0));
        assert_eq!(
            (uae.ias, uae.tas, uae.mach),
            (Some(290), Some(490), Some(0.848))
        );
        assert_eq!(
            (uae.wind_dir, uae.wind_speed, uae.oat),
            (Some(270), Some(61), Some(-53))
        );
        assert_eq!(uae.nav_altitude_mcp, Some(35_008));
        assert_eq!(uae.position_ms, Some(s.now_ms));
        assert!(uae.emergency.is_empty(), "`none` is no emergency");

        // a multilaterated light aircraft: position a little older than the response
        let sr22 = s.aircraft.iter().find(|a| a.hex == "a1d523").unwrap();
        assert!(sr22.mlat);
        assert_eq!(sr22.position_ms, Some(s.now_ms - 1_249));
        assert_eq!(sr22.seen_ms, s.now_ms - 800);
        assert_eq!(sr22.true_heading, None);
        assert_eq!(sr22.baro_rate, Some(2_107));
    }

    #[test]
    fn ground_altitude_means_on_ground() {
        let s = parse(FRANKFURT).unwrap();
        let ground: Vec<_> = s.aircraft.iter().filter(|a| a.on_ground).collect();
        assert_eq!(ground.len(), 3);
        assert!(ground.iter().all(|a| a.alt_baro.is_none()));
    }

    #[test]
    fn tolerates_odd_responses() {
        let s = parse(r#"{"ac":null,"now":1700000000000}"#).unwrap();
        assert!(s.aircraft.is_empty());
        assert!(parse(r#"{"ac":[]}"#).is_err());
        let s = parse(
            r#"{"now":1700000000000,"ac":[{"hex":"ABC123","flight":"TEST1   ","lat":91,"lon":5,
                "alt_baro":"ground","seen":"x","track":-10,"nav_modes":["autopilot","vnav"]},{"flight":"NOHEX"}]}"#,
        )
        .unwrap();
        assert_eq!(s.aircraft.len(), 1);
        let a = &s.aircraft[0];
        assert_eq!(a.hex, "abc123");
        assert_eq!(a.callsign, "TEST1");
        assert!(a.lat.is_none() && a.lon.is_none() && a.position_ms.is_none());
        assert!(a.on_ground);
        assert_eq!(a.track, Some(350.0));
        assert_eq!(a.nav_modes, ["autopilot", "vnav"]);
        assert_eq!(a.seen_ms, 1_700_000_000_000);
    }

    /// On a clock of the test's own (`*_at`), so a loaded machine's pauses between the lines
    /// change nothing: every slot and every backoff is checked to the nanosecond.
    #[test]
    fn calls_are_spaced_and_back_off() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut cache = Cache::default();
        assert!(cache.reserve_at(t0).is_zero());
        let second = cache.reserve_at(t0);
        assert_eq!(second, GAP, "the second call waits a whole gap");
        let third = cache.reserve_at(t0 + ms(100));
        assert_eq!(third, second + GAP - ms(100), "the third a gap after the second");
        // once the reserved slots have passed, a call goes at once and starts a new gap
        let later = t0 + 3 * GAP + ms(1);
        assert!(cache.reserve_at(later).is_zero());
        assert_eq!(cache.reserve_at(later + ms(500)), GAP - ms(500));
        // the wall clock's version reserves from now
        assert!(Cache::default().reserve().is_zero());

        cache.failed_at(t0, None);
        let first = cache.blocked_until.unwrap();
        assert_eq!(first, t0 + BACKOFF_BASE);
        cache.failed_at(t0, None);
        assert!(cache.blocked_until.unwrap() > first, "backoff grows");
        assert_eq!(cache.blocked_until.unwrap(), t0 + 2 * BACKOFF_BASE, "doubling");
        for _ in 0..20 {
            cache.failed_at(t0, None);
        }
        assert!(cache.blocked_until.unwrap() <= t0 + BACKOFF_MAX);
        assert_eq!(cache.blocked_until.unwrap(), t0 + BACKOFF_MAX, "up to the maximum");
        cache.failed_at(t0, Some(Duration::from_secs(30)));
        assert!(cache.blocked_until.unwrap() <= t0 + Duration::from_secs(30));
        assert_eq!(cache.blocked_until.unwrap(), t0 + Duration::from_secs(30), "the server's Retry-After");
        cache.failed_at(t0, Some(Duration::from_secs(3_600)));
        assert_eq!(cache.blocked_until.unwrap(), t0 + BACKOFF_MAX, "a Retry-After past the maximum is cut to it");
        cache.succeeded();
        assert!(cache.blocked_until.is_none() && cache.failures == 0);
        // the wall clock's version blocks from now
        let before = Instant::now();
        cache.failed(None);
        let until = cache.blocked_until.unwrap();
        assert!(until >= before + BACKOFF_BASE && until <= Instant::now() + BACKOFF_BASE);
    }

    #[test]
    fn the_cache_reuses_and_stays_small() {
        let mut cache = Cache::default();
        cache.store(
            "a".into(),
            Snapshot {
                now_ms: 1,
                aircraft: vec![],
            },
        );
        assert_eq!(cache.fresh("a").unwrap().now_ms, 1);
        assert!(cache.fresh("b").is_none());
        for i in 0..100 {
            cache.store(format!("k{i}"), Snapshot::default());
        }
        assert!(cache.entries.len() <= CACHE_LIMIT);
    }
}
