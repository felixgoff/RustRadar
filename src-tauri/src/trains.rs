//! Live train positions, combined from regional public feeds. All but one
//! need no key:
//!
//! - **trafikverket**: Sweden, every train on the national network, with the
//!   Trafikverket key from the keys page (see trafikverket.rs). CC0.
//! - **digitraffic**: Finland, Fintraffic's open rail API (GraphQL), every
//!   running train with its type, operator, destination and delay. CC BY 4.0.
//! - **amtrak**: the USA and Canada (Amtrak and VIA Rail), via the community
//!   Amtraker API. ODC-By 1.0; Amtraker asks to be credited prominently.
//! - **entur**: Norway (and SJ Nord's trains into Sweden), Entur's open
//!   vehicle-positions API. NLOD.
//! - **irishrail**: Ireland, Iarnród Éireann's realtime API (XML). CC BY.
//! - **vbb**: Berlin and Brandenburg, HAFAS "radar" via transport.rest. The
//!   only source here with future positions (`frames`). Deutsche Bahn's own
//!   HAFAS was shut off in January 2025 and `v6.db.transport.rest` now runs
//!   on a backend without `/radar`, so DB isn't asked; VBB's instance speaks
//!   the same format, though it was failing (503/timeouts) when this was
//!   written, which the backoff copes with.
//!
//! Only sources whose coverage meets the view are asked, in parallel; one
//! failing never fails the others. Whole-network feeds are fetched whole and
//! filtered here; a stale answer (up to `STALE_MAX`) is returned at once
//! while a fresh one is fetched in the background, since Digitraffic takes
//! several seconds to answer. Each source keeps its own gap between calls and
//! backs off after failures or a 429.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

use crate::CmdResult;

const USER_AGENT: &str = concat!(
    "RustRadar/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/felixgoff/RustRadar)"
);
/// Identifies the app to Digitraffic (raises its per-IP limit).
const DIGITRAFFIC_USER: &str = "RustRadar";
/// Entur's required client name, `<company>-<application>`.
const ENTUR_CLIENT: &str = "rustradar-desktop";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Minimum gap between two calls to the same source.
const GAP: Duration = Duration::from_secs(2);
/// A cached answer older than its source's TTL is refreshed; until this age
/// it is still shown meanwhile.
const STALE_MAX: Duration = Duration::from_secs(120);
const BACKOFF_BASE: Duration = Duration::from_secs(15);
const BACKOFF_MAX: Duration = Duration::from_secs(600);
/// Areas kept per source (radar views; whole feeds use one).
const CACHE_LIMIT: usize = 12;

const DIGITRAFFIC_URL: &str = "https://rata.digitraffic.fi/api/v2/graphql/graphql";
/// Running trains with their two latest positions (for a heading), last
/// station (destination) and latest actual time's difference (delay).
const DIGITRAFFIC_QUERY: &str = "{ currentlyRunningTrains { trainNumber departureDate commuterLineid \
    trainType { name trainCategory { name } } operator { shortCode name } \
    trainLocations(orderBy: {timestamp: DESCENDING}, take: 2) { speed timestamp location } \
    last: timeTableRows(orderBy: {scheduledTime: DESCENDING}, take: 1) { station { name } } \
    done: timeTableRows(where: {actualTime: {unequals: null}}, orderBy: {actualTime: DESCENDING}, take: 1) { differenceInMinutes } } }";
const AMTRAKER_URL: &str = "https://api.amtraker.com/v3/trains";
const ENTUR_URL: &str = "https://api.entur.io/realtime/v2/vehicles/graphql";
const ENTUR_QUERY: &str = "{ vehicles(mode: RAIL) { vehicleId lastUpdated location { latitude longitude } \
    bearing speed delay operator { name } line { lineRef publicCode lineName } destinationName \
    serviceJourney { id } } }";
/// Entur vehicles not heard from for this long are left out.
const ENTUR_MAX_AGE_S: i64 = 600;
const IRISHRAIL_URL: &str = "https://api.irishrail.ie/realtime/realtime.asmx/getCurrentTrainsXML";
const VBB_URL: &str = "https://v6.vbb.transport.rest";

/// Radar: asked only from this deck.gl zoom, for at most this much around
/// the view's centre (degrees, half-spans), how far ahead and in how many frames.
const RADAR_MIN_ZOOM: f64 = 6.0;
const RADAR_HALF_LAT: f64 = 0.6;
const RADAR_HALF_LON: f64 = 1.0;
const RADAR_DURATION_S: i64 = 30;
const RADAR_RESULTS: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    HighSpeed,
    Intercity,
    Regional,
    Suburban,
    Metro,
    Freight,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Train {
    /// Source-prefixed and stable for the run, e.g. `digitraffic:2026-10-13:2`.
    pub id: String,
    pub source: &'static str,
    /// e.g. "ICE 578", "IC 1", "Amtrak 5 California Zephyr".
    pub name: String,
    pub kind: Kind,
    pub lat: f64,
    pub lon: f64,
    /// Degrees clockwise from north.
    pub heading: Option<f32>,
    pub speed_kmh: Option<f32>,
    pub destination: Option<String>,
    pub operator: Option<String>,
    /// Minutes late (negative: early).
    pub delay_minutes: Option<i32>,
    /// When the position was reported, Unix seconds.
    pub position_unix: i64,
    /// Expected positions ahead, `(lat, lon, unix_ms)` in time order, the
    /// first at the report; empty when the source has none.
    pub frames: Vec<(f64, f64, i64)>,
}

/// Degrees; `west > east` crosses the antimeridian.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

impl Area {
    const WORLD: Area = Area { west: -180.0, south: -90.0, east: 180.0, north: 90.0 };

    /// From a view as the map reports it: longitudes wrapped, latitudes ordered.
    fn view(west: f64, south: f64, east: f64, north: f64) -> Option<Area> {
        if ![west, south, east, north].iter().all(|x| x.is_finite()) {
            return None;
        }
        let (south, north) = (south.min(north).max(-90.0), north.max(south).min(90.0));
        if east - west >= 360.0 {
            return Some(Area { south, north, ..Area::WORLD });
        }
        let wrap = |x: f64| (x + 180.0).rem_euclid(360.0) - 180.0;
        Some(Area { west: wrap(west), south, east: wrap(east), north })
    }

    fn lon_ranges(&self) -> Vec<(f64, f64)> {
        if self.west <= self.east {
            vec![(self.west, self.east)]
        } else {
            vec![(self.west, 180.0), (-180.0, self.east)]
        }
    }

    fn intersects(&self, other: &Area) -> bool {
        self.south <= other.north
            && other.south <= self.north
            && self
                .lon_ranges()
                .iter()
                .any(|(w, e)| other.lon_ranges().iter().any(|(ow, oe)| *w <= *oe && *ow <= *e))
    }

    fn contains(&self, lat: f64, lon: f64) -> bool {
        lat >= self.south
            && lat <= self.north
            && self.lon_ranges().iter().any(|(w, e)| lon >= *w && lon <= *e)
    }

    /// Widened by `share` of its size on every side (for small pans).
    fn padded(&self, share: f64) -> Area {
        let dlat = (self.north - self.south) * share;
        let width = (self.east - self.west).rem_euclid(360.0);
        if width + 2.0 * width * share >= 360.0 || (self.west == -180.0 && self.east == 180.0) {
            return Area { south: (self.south - dlat).max(-90.0), north: (self.north + dlat).min(90.0), ..Area::WORLD };
        }
        let wrap = |x: f64| (x + 180.0).rem_euclid(360.0) - 180.0;
        Area {
            west: wrap(self.west - width * share),
            south: (self.south - dlat).max(-90.0),
            east: wrap(self.east + width * share),
            north: (self.north + dlat).min(90.0),
        }
    }

    /// The view's centre, longitude-wrapped.
    fn centre(&self) -> (f64, f64) {
        let width = (self.east - self.west).rem_euclid(360.0);
        let lon = (self.west + width / 2.0 + 180.0).rem_euclid(360.0) - 180.0;
        ((self.south + self.north) / 2.0, lon)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Trafikverket,
    Digitraffic,
    Amtrak,
    Entur,
    IrishRail,
    Vbb,
}

pub const SOURCES: [Source; 6] = [Source::Trafikverket, Source::Digitraffic, Source::Amtrak, Source::Entur, Source::IrishRail, Source::Vbb];

impl Source {
    pub fn id(self) -> &'static str {
        match self {
            Source::Trafikverket => "trafikverket",
            Source::Digitraffic => "digitraffic",
            Source::Amtrak => "amtrak",
            Source::Entur => "entur",
            Source::IrishRail => "irishrail",
            Source::Vbb => "vbb",
        }
    }

    /// Where the source has trains (a little generous at the edges), as
    /// `(west, south, east, north)` boxes.
    fn coverage(self) -> &'static [(f64, f64, f64, f64)] {
        match self {
            // Sweden, and its lines to Oslo, Copenhagen and Narvik
            Source::Trafikverket => &[(10.5, 55.0, 24.5, 69.5)],
            // Finland, with the lines to the Russian border
            Source::Digitraffic => &[(19.0, 59.5, 32.0, 70.2)],
            // Amtrak's network, and VIA Rail's from Vancouver to Halifax and Churchill
            Source::Amtrak => &[(-125.0, 24.0, -59.0, 59.5)],
            // southern Norway; then Trøndelag to Narvik, with SJ Nord's and
            // the Ofoten line's runs into Sweden
            Source::Entur => &[(4.5, 57.5, 13.0, 64.5), (9.5, 62.0, 22.0, 69.0)],
            // the island of Ireland (the Enterprise reaches Belfast)
            Source::IrishRail => &[(-10.8, 51.3, -5.3, 55.5)],
            Source::Vbb => &[(11.2, 51.3, 15.0, 53.6)],
        }
    }

    fn covers(self, view: &Area) -> bool {
        self.coverage()
            .iter()
            .any(|&(west, south, east, north)| Area { west, south, east, north }.intersects(view))
    }

    /// Fetched whole (and filtered here) rather than by area.
    fn whole(self) -> bool {
        self != Source::Vbb
    }

    fn min_zoom(self) -> f64 {
        if self.whole() { 0.0 } else { RADAR_MIN_ZOOM }
    }

    fn ttl(self) -> Duration {
        Duration::from_secs(if self.whole() { 30 } else { 20 })
    }

    /// Display name in errors.
    fn label(self) -> &'static str {
        match self {
            Source::Trafikverket => "Trafikverket",
            Source::Digitraffic => "Digitraffic",
            Source::Amtrak => "Amtraker",
            Source::Entur => "Entur",
            Source::IrishRail => "Irish Rail",
            Source::Vbb => "VBB (transport.rest)",
        }
    }

    /// The cache key and area to ask for a view; whole feeds ask once for all.
    fn request(self, view: &Area) -> (String, Area) {
        if self.whole() {
            return ("all".into(), Area::WORLD);
        }
        let area = radar_area(view);
        (format!("{:.1},{:.1},{:.1},{:.1}", area.west, area.south, area.east, area.north), area)
    }
}

/// The view clipped to at most `RADAR_HALF_*` around its centre, snapped to
/// 0.1° so small pans ask the same area again.
fn radar_area(view: &Area) -> Area {
    let snap = |x: f64| (x * 10.0).round() / 10.0;
    let up = |x: f64| (x * 10.0).ceil() / 10.0;
    let (lat, lon) = view.centre();
    let (lat, lon) = (snap(lat), snap(lon));
    let half_lat = up(((view.north - view.south) / 2.0).min(RADAR_HALF_LAT)).max(0.1);
    let half_lon = up(((view.east - view.west).rem_euclid(360.0) / 2.0).min(RADAR_HALF_LON)).max(0.1);
    Area {
        west: lon - half_lon,
        south: (lat - half_lat).max(-90.0),
        east: lon + half_lon,
        north: (lat + half_lat).min(90.0),
    }
}

// ---------------------------------------------------------------- helpers

pub(crate) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or_default()
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite())
}

fn valid(lat: f64, lon: f64) -> bool {
    lat.is_finite() && lon.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0 && (lat != 0.0 || lon != 0.0)
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Unix ms of an RFC 3339 time: `2026-10-13T06:15:15.000Z`, `2026-10-12T23:11:39-07:00`.
pub fn unix_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let field = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (h, mi, sec) = (field(11..13)?, field(14..16)?, field(17..19)?);
    let mut rest = &s[19..];
    let mut ms = 0;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        let scaled: String = frac[..digits].chars().chain("000".chars()).take(3).collect();
        ms = scaled.parse::<i64>().ok()?;
        rest = &frac[digits..];
    }
    let offset_min = match rest {
        "Z" | "z" | "" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hh = rest.get(1..3)?.parse::<i64>().ok()?;
            let mm = rest.get(rest.len() - 2..)?.parse::<i64>().ok()?;
            sign * (hh * 60 + mm)
        }
    };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset_min * 60;
    Some(secs * 1000 + ms)
}

const EARTH_M: f64 = 6_371_000.0;

/// Great-circle distance, metres.
fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, la2) = (a.0.to_radians(), b.0.to_radians());
    let dlat = la2 - la1;
    let dlon = (b.1 - a.1).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_M * h.sqrt().min(1.0).asin()
}

/// Initial bearing from `a` to `b`, degrees clockwise from north.
fn bearing(a: (f64, f64), b: (f64, f64)) -> f32 {
    let (la1, la2) = (a.0.to_radians(), b.0.to_radians());
    let dlon = (b.1 - a.1).to_radians();
    let y = dlon.sin() * la2.cos();
    let x = la1.cos() * la2.sin() - la1.sin() * la2.cos() * dlon.cos();
    (y.atan2(x).to_degrees().rem_euclid(360.0)) as f32
}

/// Eight-point compass to degrees.
fn compass(s: &str) -> Option<f32> {
    let i = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"].iter().position(|c| c.eq_ignore_ascii_case(s.trim()))?;
    Some(i as f32 * 45.0)
}

// ------------------------------------------------------------- Digitraffic

/// Finland's running trains from Digitraffic's GraphQL answer.
pub fn parse_digitraffic(body: &str) -> Result<Vec<Train>, String> {
    let json: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let list = json["data"]["currentlyRunningTrains"]
        .as_array()
        .ok_or_else(|| graphql_error(&json).unwrap_or_else(|| "no train list".into()))?;
    Ok(list.iter().filter_map(digitraffic_train).collect())
}

fn graphql_error(json: &Value) -> Option<String> {
    let first = json["errors"].as_array()?.first()?;
    Some(text(&first["message"]).unwrap_or_else(|| "query refused".into()))
}

fn digitraffic_train(t: &Value) -> Option<Train> {
    let category = t["trainType"]["trainCategory"]["name"].as_str().unwrap_or_default();
    // yard moves and maintenance machines aren't trains anyone follows
    if matches!(category, "Shunting" | "On-track machines") {
        return None;
    }
    let number = t["trainNumber"].as_i64()?;
    let date = t["departureDate"].as_str().unwrap_or_default();
    let ty = text(&t["trainType"]["name"]).unwrap_or_default();
    let locations = t["trainLocations"].as_array()?;
    let point = |l: &Value| -> Option<(f64, f64)> {
        let c = l["location"].as_array()?;
        let (lon, lat) = (num(c.first()?)?, num(c.get(1)?)?);
        valid(lat, lon).then_some((lat, lon))
    };
    let latest = locations.first()?;
    let here = point(latest)?;
    let heading = locations
        .get(1)
        .and_then(point)
        .filter(|before| distance(*before, here) >= 15.0)
        .map(|before| bearing(before, here));
    let kind = match (category, ty.as_str()) {
        ("Long-distance", "S") => Kind::HighSpeed,
        ("Long-distance", "H" | "HDM" | "HSM" | "HV") => Kind::Regional,
        ("Long-distance", _) => Kind::Intercity,
        ("Commuter", _) => Kind::Suburban,
        ("Cargo", _) => Kind::Freight,
        _ => Kind::Other,
    };
    let name = match text(&t["commuterLineid"]) {
        Some(line) => format!("{line} train {number}"),
        None if ty.is_empty() => number.to_string(),
        None => format!("{ty} {number}"),
    };
    Some(Train {
        id: format!("digitraffic:{date}:{number}"),
        source: "digitraffic",
        name,
        kind,
        lat: here.0,
        lon: here.1,
        heading,
        speed_kmh: num(&latest["speed"]).map(|s| s as f32),
        destination: text(&t["last"][0]["station"]["name"])
            .map(|s| s.strip_suffix(" asema").map(str::to_owned).unwrap_or(s)),
        operator: text(&t["operator"]["name"]),
        delay_minutes: num(&t["done"][0]["differenceInMinutes"]).map(|d| d.round() as i32),
        position_unix: latest["timestamp"].as_str().and_then(unix_ms).map(|ms| ms / 1000)?,
        frames: Vec::new(),
    })
}

// ------------------------------------------------------------ Amtraker

/// Amtrak's corridor services; the rest are long-distance.
const AMTRAK_REGIONAL: &[&str] = &[
    "Northeast Regional", "Keystone", "Empire Service", "Capitol Corridor", "Pacific Surfliner",
    "San Joaquins", "Amtrak Cascades", "Hiawatha", "Downeaster", "Lincoln Service", "Wolverine",
    "Illini", "Saluki", "Piedmont", "Pere Marquette", "Blue Water", "Missouri River Runner",
    "Heartland Flyer", "Valley Flyer", "Berkshire Flyer", "Borealis", "Mardi Gras Service",
];

/// Trains on the move from Amtraker's `/v3/trains` (`{ "5": [train, ...], ... }`).
pub fn parse_amtraker(body: &str) -> Result<Vec<Train>, String> {
    let json: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let groups = json.as_object().ok_or("no train list")?;
    Ok(groups.values().filter_map(Value::as_array).flatten().filter_map(amtraker_train).collect())
}

fn amtraker_train(t: &Value) -> Option<Train> {
    if t["trainState"].as_str() != Some("Active") {
        return None;
    }
    let (lat, lon) = (num(&t["lat"])?, num(&t["lon"])?);
    if !valid(lat, lon) {
        return None;
    }
    let id = text(&t["trainID"])?;
    let provider = text(&t["provider"]).unwrap_or_else(|| "Amtrak".into());
    let (brand, operator) = match provider.as_str() {
        "Amtrak" => ("Amtrak".to_owned(), "Amtrak".to_owned()),
        "Via" => ("VIA".to_owned(), "VIA Rail".to_owned()),
        other => (other.to_owned(), other.to_owned()),
    };
    let number = text(&t["trainNumRaw"]).or_else(|| text(&t["trainNum"])).unwrap_or_default();
    let route = text(&t["routeName"]).unwrap_or_default();
    let kind = if route.contains("Acela") {
        Kind::HighSpeed
    } else if provider == "Amtrak" && AMTRAK_REGIONAL.contains(&route.as_str()) {
        Kind::Regional
    } else {
        Kind::Intercity
    };
    // lateness at the next stop: its estimated against its scheduled arrival
    let delay = t["stations"].as_array().and_then(|stations| {
        let next = stations.iter().find(|s| s["status"].as_str() != Some("Departed"))?;
        let (eta, sched) = (unix_ms(next["arr"].as_str()?)?, unix_ms(next["schArr"].as_str()?)?);
        Some(((eta - sched) as f64 / 60_000.0).round() as i32)
    });
    let position_ms = t["lastValTS"].as_str().and_then(unix_ms).or_else(|| t["updatedAt"].as_str().and_then(unix_ms))?;
    Some(Train {
        id: format!("amtrak:{id}"),
        source: "amtrak",
        name: format!("{brand} {number} {route}").trim().to_owned(),
        kind,
        lat,
        lon,
        heading: t["heading"].as_str().and_then(compass),
        speed_kmh: num(&t["velocity"]).map(|mph| (mph * 1.609_344) as f32),
        destination: text(&t["destName"]),
        operator: Some(operator),
        delay_minutes: delay,
        position_unix: position_ms / 1000,
        frames: Vec::new(),
    })
}

// ---------------------------------------------------------------- Entur

/// Norway's trains from Entur's vehicles GraphQL answer.
pub fn parse_entur(body: &str) -> Result<Vec<Train>, String> {
    let json: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let list = json["data"]["vehicles"]
        .as_array()
        .ok_or_else(|| graphql_error(&json).unwrap_or_else(|| "no vehicle list".into()))?;
    let mut seen = HashSet::new();
    Ok(list.iter().filter_map(entur_train).filter(|t| seen.insert(t.id.clone())).collect())
}

fn entur_train(v: &Value) -> Option<Train> {
    let (lat, lon) = (num(&v["location"]["latitude"])?, num(&v["location"]["longitude"])?);
    if !valid(lat, lon) {
        return None;
    }
    let key = text(&v["serviceJourney"]["id"]).or_else(|| text(&v["vehicleId"]))?;
    let code = text(&v["line"]["publicCode"]).unwrap_or_default();
    let line_name = text(&v["line"]["lineName"]);
    let codespace = v["line"]["lineRef"].as_str().and_then(|r| r.split(':').next()).unwrap_or_default();
    let kind = {
        let letters: String = code.chars().take_while(char::is_ascii_alphabetic).collect();
        match letters.as_str() {
            "FLY" => Kind::HighSpeed,
            "F" => Kind::Intercity,
            "L" => Kind::Suburban,
            _ => Kind::Regional,
        }
    };
    let name = match line_name {
        Some(n) if n != code && !code.is_empty() => format!("{code} {n}"),
        Some(n) => n,
        None => code,
    };
    let operator = text(&v["operator"]["name"]).or_else(|| {
        Some(
            match codespace {
                "VYG" => "Vy",
                "GOA" => "Go-Ahead Nordic",
                "SJN" => "SJ Nord",
                "FLT" => "Flytoget",
                "NSB" => "Vy",
                _ => return None,
            }
            .to_owned(),
        )
    });
    Some(Train {
        id: format!("entur:{key}"),
        source: "entur",
        name,
        kind,
        lat,
        lon,
        heading: num(&v["bearing"]).map(|b| b.rem_euclid(360.0) as f32),
        // SIRI VM reports speed in metres per second
        speed_kmh: num(&v["speed"]).map(|s| (s * 3.6) as f32),
        destination: text(&v["destinationName"]),
        operator,
        delay_minutes: num(&v["delay"]).map(|s| (s / 60.0).round() as i32),
        position_unix: v["lastUpdated"].as_str().and_then(unix_ms)? / 1000,
        frames: Vec::new(),
    })
}

// ------------------------------------------------------------ Irish Rail

fn xml_tag<'a>(block: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = block.find(&open)? + open.len();
    let len = block[start..].find(&format!("</{tag}>"))?;
    Some(block[start..start + len].trim())
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Running trains from `getCurrentTrainsXML`; it carries no report time, so
/// `now_ms` stands for it.
pub fn parse_irishrail(body: &str, now_ms: i64) -> Result<Vec<Train>, String> {
    if !body.contains("ArrayOfObjTrainPositions") {
        return Err("no train list".into());
    }
    Ok(body.split("<objTrainPositions>").skip(1).filter_map(|b| irishrail_train(b, now_ms)).collect())
}

fn irishrail_train(block: &str, now_ms: i64) -> Option<Train> {
    // N: not yet running, T: terminated
    if xml_tag(block, "TrainStatus")? != "R" {
        return None;
    }
    let lat = xml_tag(block, "TrainLatitude")?.parse::<f64>().ok()?;
    let lon = xml_tag(block, "TrainLongitude")?.parse::<f64>().ok()?;
    if !valid(lat, lon) {
        return None;
    }
    let code = xml_tag(block, "TrainCode")?.to_owned();
    let date: String = xml_tag(block, "TrainDate").unwrap_or_default().split_whitespace().collect();
    // "A201\n06:15 - Cork to Dublin Heuston (0 mins late)\nDeparted ...", with a literal `\n`
    let message = unescape(xml_tag(block, "PublicMessage").unwrap_or_default());
    let summary = message.split("\\n").nth(1).unwrap_or_default();
    let summary = summary.split_once(" - ").map_or(summary, |(_, s)| s);
    let (route, delay) = match summary.split_once('(') {
        Some((route, rest)) => {
            let minutes = rest.split_whitespace().next().and_then(|n| n.parse::<i32>().ok());
            // the day's rollover shows up as 1440 minutes late
            (route.trim(), minutes.filter(|m| m.abs() <= 600))
        }
        None => (summary.trim(), None),
    };
    let destination = route.split_once(" to ").map(|(_, to)| to.trim().to_owned()).filter(|s| !s.is_empty());
    let kind = match code.chars().next() {
        Some('A') => Kind::Intercity,
        Some('D') => Kind::Regional,
        Some('E' | 'P') => Kind::Suburban,
        _ => Kind::Other,
    };
    Some(Train {
        id: format!("irishrail:{date}:{code}"),
        source: "irishrail",
        name: if route.is_empty() { code.clone() } else { format!("{code} {route}") },
        kind,
        lat,
        lon,
        heading: None,
        speed_kmh: None,
        destination,
        operator: Some(if code.starts_with('E') { "DART" } else { "Iarnród Éireann" }.to_owned()),
        delay_minutes: delay,
        position_unix: now_ms / 1000,
        frames: Vec::new(),
    })
}

// --------------------------------------------------- HAFAS radar (transport.rest)

fn radar_kind(product: &str, name: &str) -> Option<Kind> {
    let high_speed = ["ICE", "TGV", "RJ", "ECE", "FR"].iter().any(|p| name.starts_with(&format!("{p} ")));
    Some(match product {
        "nationalExpress" => Kind::HighSpeed,
        "national" | "express" if high_speed => Kind::HighSpeed,
        "national" | "express" => Kind::Intercity,
        "regionalExpress" | "regionalExp" | "regional" => Kind::Regional,
        "suburban" => Kind::Suburban,
        "subway" => Kind::Metro,
        _ => return None,
    })
}

/// Rail vehicles from a transport.rest `/radar` answer (`{ movements }`, or
/// a bare list from older versions), asked for at `now_ms`.
pub fn parse_radar(body: &str, source: &'static str, now_ms: i64) -> Result<Vec<Train>, String> {
    let json: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let list = json["movements"].as_array().or(json.as_array()).ok_or("no movement list")?;
    Ok(list.iter().filter_map(|m| radar_train(m, source, now_ms)).collect())
}

fn radar_train(m: &Value, source: &'static str, now_ms: i64) -> Option<Train> {
    let line = &m["line"];
    let name = text(&line["name"])?;
    let kind = radar_kind(line["product"].as_str().unwrap_or_default(), &name)?;
    let (lat, lon) = (num(&m["location"]["latitude"])?, num(&m["location"]["longitude"])?);
    if !valid(lat, lon) {
        return None;
    }
    let key = text(&m["tripId"])
        .or_else(|| text(&m["trip"]))
        .or_else(|| m["trip"].as_i64().map(|t| t.to_string()))
        .unwrap_or_else(|| format!("{}:{name}", text(&line["fahrtNr"]).unwrap_or_default()));
    let stopovers = m["nextStopovers"].as_array().map(Vec::as_slice).unwrap_or_default();
    let delay = stopovers.iter().find_map(|s| num(&s["arrivalDelay"]).or_else(|| num(&s["departureDelay"])));
    let path: Vec<(f64, f64)> = m["polyline"]["features"]
        .as_array()
        .map(|f| {
            f.iter()
                .filter_map(|p| {
                    let c = p["geometry"]["coordinates"].as_array()?;
                    let (lon, lat) = (num(c.first()?)?, num(c.get(1)?)?);
                    valid(lat, lon).then_some((lat, lon))
                })
                .collect()
        })
        .unwrap_or_default();
    // the first stop still ahead: where the train should be at its (realtime) arrival
    let next = stopovers.iter().find_map(|s| {
        let at = ["arrival", "departure"].iter().find_map(|k| s[*k].as_str().and_then(unix_ms))?;
        let loc = &s["stop"]["location"];
        let stop = (num(&loc["latitude"])?, num(&loc["longitude"])?);
        (at > now_ms && distance(stop, (lat, lon)) > 50.0).then_some((stop, at))
    });
    let frames = radar_frames((lat, lon), &path, next, now_ms, now_ms + RADAR_DURATION_S * 1000);
    let (heading, speed_kmh) = match frames.as_slice() {
        [a, b, ..] if b.2 > a.2 => {
            let (pa, pb) = ((a.0, a.1), (b.0, b.1));
            (Some(bearing(pa, pb)), Some((distance(pa, pb) / ((b.2 - a.2) as f64 / 1000.0) * 3.6) as f32))
        }
        _ => (None, None),
    };
    Some(Train {
        id: format!("{source}:{key}"),
        source,
        name,
        kind,
        lat,
        lon,
        heading,
        speed_kmh,
        destination: text(&m["direction"]),
        operator: text(&line["operator"]["name"]),
        delay_minutes: delay.map(|s| (s / 60.0).round() as i32),
        position_unix: now_ms / 1000,
        frames,
    })
}

/// Positions ahead along `path` (the movement's polyline), at the speed that
/// brings the train from `here` to the next stop at its expected time, up to
/// `until_ms`. HAFAS gives the track but no positions per frame, so this is
/// the frontend's best guess for animating between refreshes.
fn radar_frames(
    here: (f64, f64),
    path: &[(f64, f64)],
    next: Option<((f64, f64), i64)>,
    now_ms: i64,
    until_ms: i64,
) -> Vec<(f64, f64, i64)> {
    let Some((stop, arrive_ms)) = next else { return Vec::new() };
    if path.len() < 2 || arrive_ms <= now_ms {
        return Vec::new();
    }
    // the segment the train is on (closest to it), then the vertex nearest the stop beyond it
    let seg_dist = |i: usize| {
        let (a, b) = (path[i], path[i + 1]);
        distance(a, here) + distance(here, b) - distance(a, b)
    };
    let on = (0..path.len() - 1).min_by(|&i, &j| seg_dist(i).total_cmp(&seg_dist(j))).unwrap_or(0);
    let Some(end) = (on + 1..path.len()).min_by(|&i, &j| distance(path[i], stop).total_cmp(&distance(path[j], stop)))
    else {
        return Vec::new();
    };
    let points: Vec<(f64, f64)> = std::iter::once(here).chain(path[on + 1..=end].iter().copied()).collect();
    let total: f64 = points.windows(2).map(|w| distance(w[0], w[1])).sum();
    if total < 1.0 {
        return Vec::new();
    }
    let ms_per_m = (arrive_ms - now_ms) as f64 / total;
    let mut frames = vec![(here.0, here.1, now_ms)];
    let mut t = now_ms as f64;
    for w in points.windows(2) {
        let leg = distance(w[0], w[1]);
        let t_end = t + leg * ms_per_m;
        if t_end >= until_ms as f64 {
            let f = ((until_ms as f64 - t) / (t_end - t)).clamp(0.0, 1.0);
            frames.push((w[0].0 + (w[1].0 - w[0].0) * f, w[0].1 + (w[1].1 - w[0].1) * f, until_ms));
            break;
        }
        if leg > 0.0 {
            frames.push((w[1].0, w[1].1, t_end.round() as i64));
        }
        t = t_end;
    }
    frames
}

fn radar_url(base: &str, a: &Area) -> String {
    format!(
        "{base}/radar?north={:.4}&west={:.4}&south={:.4}&east={:.4}&results={RADAR_RESULTS}&duration={RADAR_DURATION_S}&frames=3&polylines=true&language=en",
        a.north, a.west, a.south, a.east
    )
}

// ------------------------------------------------------------ fetching

/// Cached answers and politeness bookkeeping, per source.
#[derive(Default)]
struct SourceState {
    entries: HashMap<String, (Instant, Arc<Vec<Train>>)>,
    /// Keys with a background refresh under way.
    refreshing: HashSet<String>,
    next: Option<Instant>,
    blocked_until: Option<Instant>,
    failures: u32,
}

impl SourceState {
    /// A cached answer no older than `STALE_MAX`, and whether it's fresh.
    fn cached(&self, key: &str, ttl: Duration) -> Option<(bool, Arc<Vec<Train>>)> {
        let (at, trains) = self.entries.get(key)?;
        let age = at.elapsed();
        (age < STALE_MAX).then(|| (age < ttl, trains.clone()))
    }

    fn store(&mut self, key: String, trains: Arc<Vec<Train>>) {
        if self.entries.len() >= CACHE_LIMIT
            && !self.entries.contains_key(&key)
            && let Some(oldest) = self.entries.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(key, (Instant::now(), trains));
    }

    fn blocked(&self) -> Option<Duration> {
        let left = self.blocked_until?.saturating_duration_since(Instant::now());
        (!left.is_zero()).then_some(left)
    }

    /// Reserves the next call slot; returns how long to wait for it.
    fn reserve(&mut self) -> Duration {
        let now = Instant::now();
        let at = self.next.filter(|t| *t > now).unwrap_or(now);
        self.next = Some(at + GAP);
        at - now
    }

    fn failed(&mut self, retry_after: Option<Duration>) {
        self.failures = self.failures.saturating_add(1);
        let doubling = BACKOFF_BASE.saturating_mul(1 << self.failures.min(8).saturating_sub(1));
        let wait = retry_after.unwrap_or(doubling).min(BACKOFF_MAX);
        self.blocked_until = Some(Instant::now() + wait);
    }

    fn succeeded(&mut self) {
        self.failures = 0;
        self.blocked_until = None;
    }
}

fn states() -> &'static Mutex<HashMap<Source, SourceState>> {
    static STATES: OnceLock<Mutex<HashMap<Source, SourceState>>> = OnceLock::new();
    STATES.get_or_init(Default::default)
}

fn with_state<T>(source: Source, f: impl FnOnce(&mut SourceState) -> T) -> T {
    let mut states = states().lock().unwrap_or_else(|e| e.into_inner());
    f(states.entry(source).or_default())
}

/// Held for the whole of a fetch: one call per source at a time.
fn gate(source: Source) -> &'static tokio::sync::Mutex<()> {
    static GATES: OnceLock<[tokio::sync::Mutex<()>; SOURCES.len()]> = OnceLock::new();
    let gates = GATES.get_or_init(Default::default);
    &gates[SOURCES.iter().position(|s| *s == source).unwrap_or(0)]
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

type Failure = (String, Option<Duration>);

async fn send(source: Source, request: reqwest::RequestBuilder) -> Result<String, Failure> {
    let name = source.label();
    let (http, request) = request.build_split();
    let request = request.map_err(|e| (format!("{name}: {e}"), None))?;
    let sent = request.body().and_then(|b| b.as_bytes()).map(String::from_utf8_lossy);
    let call = crate::devlog::start(name, request.method().as_str(), request.url().as_str(), sent.as_deref());
    let response = match http.execute(request).await {
        Ok(r) => r,
        Err(e) => {
            let e = format!("can't reach {name}: {e}");
            if let Some(c) = call {
                c.fail(&e);
            }
            return Err((e, None));
        }
    };
    let status = response.status();
    let Some(call) = call else { return read(name, response).await };
    let text = read(name, response).await;
    match &text {
        Ok(body) => call.finish(status.as_u16(), body.as_bytes()),
        Err((e, _)) => call.fail(e),
    }
    text
}

async fn read(name: &str, response: reqwest::Response) -> Result<String, Failure> {
    let status = response.status();
    if !status.is_success() {
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let message = if status.as_u16() == 429 {
            format!("{name} asked us to slow down")
        } else {
            format!("{name} answered {status}")
        };
        return Err((message, retry_after));
    }
    response.text().await.map_err(|e| (format!("{name}: {e}"), None))
}

fn graphql_body(query: &str) -> String {
    serde_json::json!({ "query": query }).to_string()
}

/// One call to a source, parsed.
pub async fn fetch(source: Source, area: &Area) -> Result<Vec<Train>, Failure> {
    let http = client().map_err(|e| (e, None))?;
    let parsed = match source {
        Source::Trafikverket => {
            let key = crate::keys::current().trafikverket;
            return crate::trafikverket::trains(http, &key, now_ms()).await.map_err(|e| (e, None));
        }
        Source::Digitraffic => {
            let request = http
                .post(DIGITRAFFIC_URL)
                .header("Digitraffic-User", DIGITRAFFIC_USER)
                .header("content-type", "application/json")
                .body(graphql_body(DIGITRAFFIC_QUERY));
            parse_digitraffic(&send(source, request).await?)
        }
        Source::Amtrak => parse_amtraker(&send(source, http.get(AMTRAKER_URL).header("accept", "application/json")).await?),
        Source::Entur => {
            let request = http
                .post(ENTUR_URL)
                .header("ET-Client-Name", ENTUR_CLIENT)
                .header("content-type", "application/json")
                .body(graphql_body(ENTUR_QUERY));
            // Entur keeps a vehicle listed long after its last report
            let fresh_from = now_ms() / 1000 - ENTUR_MAX_AGE_S;
            parse_entur(&send(source, request).await?).map(|mut trains| {
                trains.retain(|t| t.position_unix >= fresh_from);
                trains
            })
        }
        Source::IrishRail => parse_irishrail(&send(source, http.get(IRISHRAIL_URL)).await?, now_ms()),
        Source::Vbb => {
            let body = send(source, http.get(radar_url(VBB_URL, area)).header("accept", "application/json")).await?;
            parse_radar(&body, source.id(), now_ms())
        }
    };
    parsed.map_err(|e| (format!("{}: {e}", source.label()), None))
}

/// Fetches `key` within the source's politeness rules and caches it.
async fn refresh(source: Source, key: String, area: Area) -> Result<Arc<Vec<Train>>, String> {
    let _gate = gate(source).lock().await;
    let ready = with_state(source, |s| {
        if let Some((true, hit)) = s.cached(&key, source.ttl()) {
            return Ok(Ok(hit));
        }
        if let Some(left) = s.blocked() {
            return Ok(Err(format!("{} is resting; retrying in {} s", source.label(), left.as_secs().max(1))));
        }
        Err(s.reserve())
    });
    let wait = match ready {
        Ok(done) => {
            with_state(source, |s| s.refreshing.remove(&key));
            return done;
        }
        Err(wait) => wait,
    };
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
    let result = fetch(source, &area).await;
    with_state(source, |s| {
        s.refreshing.remove(&key);
        match result {
            Ok(trains) => {
                s.succeeded();
                let trains = Arc::new(trains);
                s.store(key, trains.clone());
                Ok(trains)
            }
            Err((e, retry_after)) => {
                s.failed(retry_after);
                eprintln!("trains: {e}");
                Err(e)
            }
        }
    })
}

/// The source's trains in `view`: cached, stale while a refresh runs, or fetched now.
async fn from_source(source: Source, view: Area) -> Result<Vec<Train>, String> {
    let (key, area) = source.request(&view);
    let cached = with_state(source, |s| {
        let hit = s.cached(&key, source.ttl());
        if let Some((false, _)) = &hit
            && s.blocked().is_none()
            && s.refreshing.insert(key.clone())
        {
            tauri::async_runtime::spawn(refresh(source, key.clone(), area));
        }
        hit
    });
    let trains = match cached {
        Some((_, trains)) => trains,
        None => refresh(source, key, area).await?,
    };
    let within = view.padded(0.1);
    Ok(trains.iter().filter(|t| within.contains(t.lat, t.lon)).cloned().collect())
}

/// The sources worth asking for a view at a deck.gl zoom.
pub fn sources_for(view: &Area, zoom: f64) -> Vec<Source> {
    let keyed = !crate::keys::current().trafikverket.is_empty();
    SOURCES
        .into_iter()
        .filter(|s| zoom >= s.min_zoom() && s.covers(view) && (*s != Source::Trafikverket || keyed))
        .collect()
}

fn followed() -> &'static Mutex<Option<tauri::async_runtime::JoinHandle<()>>> {
    static F: OnceLock<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>> = OnceLock::new();
    F.get_or_init(Default::default)
}

/// Streams live updates of a train (its position and its stops) until
/// `unfollow_train` or another train is followed. Trafikverket's trains only:
/// the other sources have no stream.
#[tauri::command]
pub fn follow_train(id: String, on_event: tauri::ipc::Channel<crate::trafikverket::TrainEvent>) -> CmdResult<()> {
    let key = crate::keys::current().trafikverket;
    let task = (id.starts_with("trafikverket:") && !key.is_empty())
        .then(|| Ok::<_, String>(tauri::async_runtime::spawn(crate::trafikverket::follow(client()?.clone(), key, id, on_event))))
        .transpose()?;
    if let Some(previous) = std::mem::replace(&mut *followed().lock().unwrap_or_else(|e| e.into_inner()), task) {
        previous.abort();
    }
    Ok(())
}

#[tauri::command]
pub fn unfollow_train() {
    if let Some(task) = followed().lock().unwrap_or_else(|e| e.into_inner()).take() {
        task.abort();
    }
}

/// The whole way of a train along the track, for the sources that have its
/// timetable. Made from the track loaded so far: while `complete` is false,
/// ask again in a moment.
#[tauri::command]
pub async fn train_route(id: String) -> CmdResult<crate::trafikverket::TrainRoute> {
    let key = crate::keys::current().trafikverket;
    // a whole run is a few hundred kilometres of track: off the async threads
    tauri::async_runtime::spawn_blocking(move || crate::trafikverket::route(&id, &key))
        .await
        .map_err(|e| e.to_string())?
}

/// Every stop of a train, for the sources that have its timetable; none for the others.
#[tauri::command]
pub async fn train_stops(id: String) -> CmdResult<Vec<crate::trafikverket::Stop>> {
    if id.starts_with("trafikverket:") {
        let key = crate::keys::current().trafikverket;
        return crate::trafikverket::stops(client()?, &key, &id).await;
    }
    Ok(Vec::new())
}

/// Trains in an area (degrees; `west > east` crosses the antimeridian) at a
/// deck.gl zoom, merged from every regional source covering it. A source
/// that fails is left out (and logged); only when all fail is it an error.
#[tauri::command]
pub async fn trains_in_view(west: f64, south: f64, east: f64, north: f64, zoom: f64) -> CmdResult<Vec<Train>> {
    let view = Area::view(west, south, east, north).ok_or("not an area")?;
    let sources = sources_for(&view, if zoom.is_finite() { zoom } else { 0.0 });
    let results = futures_util::future::join_all(sources.iter().map(|s| from_source(*s, view))).await;
    let mut trains = Vec::new();
    let mut errors = Vec::new();
    for result in results {
        match result {
            Ok(found) => trains.extend(found),
            Err(e) => errors.push(e),
        }
    }
    if trains.is_empty() && !errors.is_empty() && errors.len() == sources.len() {
        return Err(errors.join("; "));
    }
    // the rest glide on along the track ahead (or their heading) between reports
    let now = now_ms();
    for t in trains.iter_mut().filter(|t| t.frames.is_empty()) {
        t.frames = crate::motion::frames_with(
            t.lat,
            t.lon,
            t.position_unix * 1000,
            t.speed_kmh.map(|s| f64::from(s) / 3.6),
            t.heading.map(f64::from),
            &[],
            now,
            Some(&crate::rail::Rail),
        );
    }
    Ok(trains)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGITRAFFIC: &str = include_str!("../tests/data/digitraffic-running.json");
    const AMTRAKER: &str = include_str!("../tests/data/amtraker-trains.json");
    const ENTUR: &str = include_str!("../tests/data/entur-vehicles.json");
    const IRISHRAIL: &str = include_str!("../tests/data/irishrail-current.xml");
    /// Built from hafas-client's documented `radar()` shape: every
    /// transport.rest radar was down when this was written.
    const RADAR: &str = include_str!("../tests/data/transportrest-radar.json");

    fn by_id<'a>(trains: &'a [Train], id: &str) -> &'a Train {
        trains.iter().find(|t| t.id == id).unwrap_or_else(|| panic!("no {id}"))
    }

    #[test]
    fn parses_times() {
        assert_eq!(unix_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_ms("2026-10-13T06:15:00Z"), Some(1_791_872_100_000));
        assert_eq!(unix_ms("2026-10-13T08:15:00+02:00"), Some(1_791_872_100_000));
        assert_eq!(unix_ms("2026-10-12T23:15:00-07:00"), Some(1_791_872_100_000));
        assert_eq!(unix_ms("2026-10-13T06:15:00.250Z"), Some(1_791_872_100_250));
        assert_eq!(unix_ms("2026-10-13T06:15:00.585246Z"), Some(1_791_872_100_585));
        assert_eq!(unix_ms("2024-02-29T12:00:00Z"), Some(1_709_208_000_000));
        assert_eq!(unix_ms("13 Oct 2026"), None);
        assert_eq!(unix_ms("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn parses_digitraffic() {
        let trains = parse_digitraffic(DIGITRAFFIC).unwrap();
        // the shunting move has no position and wouldn't be shown anyway
        assert_eq!(trains.len(), 6);
        assert!(trains.iter().all(|t| t.source == "digitraffic" && t.frames.is_empty()));

        let pendolino = by_id(&trains, "digitraffic:2026-10-13:2");
        assert_eq!(pendolino.name, "S 2");
        assert_eq!(pendolino.kind, Kind::HighSpeed);
        assert_eq!((pendolino.lat, pendolino.lon), (60.338143, 25.071518));
        assert_eq!(pendolino.speed_kmh, Some(70.0));
        assert_eq!(pendolino.destination.as_deref(), Some("Helsinki"));
        assert_eq!(pendolino.operator.as_deref(), Some("VR-Yhtymä Oyj"));
        assert_eq!(pendolino.delay_minutes, Some(6));
        assert_eq!(pendolino.position_unix, 1_791_872_115);
        // moving south-southwest between its last two reports
        let h = pendolino.heading.unwrap();
        assert!((190.0..215.0).contains(&h), "{h}");

        let ic = by_id(&trains, "digitraffic:2026-10-13:1");
        assert_eq!((ic.name.as_str(), ic.kind), ("IC 1", Kind::Intercity));
        assert_eq!(ic.heading, None, "standing still: no heading");

        let commuter = by_id(&trains, "digitraffic:2026-10-13:1608");
        assert_eq!((commuter.name.as_str(), commuter.kind), ("H train 1608", Kind::Suburban));
        assert_eq!(commuter.delay_minutes, Some(-1));
        let h = commuter.heading.unwrap();
        assert!((60.0..90.0).contains(&h), "{h}");

        let cargo = by_id(&trains, "digitraffic:2026-10-12:7012");
        assert_eq!(cargo.kind, Kind::Freight);
        assert_eq!(cargo.operator.as_deref(), Some("Fenniarail Oy"));
        assert_eq!(by_id(&trains, "digitraffic:2026-10-13:11000").kind, Kind::Other);
        assert_eq!(by_id(&trains, "digitraffic:2026-10-12:269").name, "PYO 269");

        assert!(parse_digitraffic(r#"{"errors":[{"message":"bad query"}]}"#).unwrap_err().contains("bad query"));
    }

    #[test]
    fn parses_amtraker() {
        let trains = parse_amtraker(AMTRAKER).unwrap();
        // the completed Cascades run is left out
        assert_eq!(trains.len(), 4);
        let zephyr = by_id(&trains, "amtrak:5-11");
        assert_eq!(zephyr.name, "Amtrak 5 California Zephyr");
        assert_eq!(zephyr.kind, Kind::Intercity);
        assert_eq!(zephyr.heading, Some(270.0));
        assert!((zephyr.speed_kmh.unwrap() - 124.28).abs() < 0.05);
        assert_eq!(zephyr.destination.as_deref(), Some("Emeryville"));
        assert_eq!(zephyr.operator.as_deref(), Some("Amtrak"));
        // due at Elko 03:08 against 03:23 scheduled
        assert_eq!(zephyr.delay_minutes, Some(-15));
        assert_eq!(zephyr.position_unix, unix_ms("2026-10-12T23:11:39-07:00").unwrap() / 1000);

        let canadian = by_id(&trains, "amtrak:v2-9");
        assert_eq!(canadian.name, "VIA 2 Canadian");
        assert_eq!(canadian.operator.as_deref(), Some("VIA Rail"));
        assert_eq!(canadian.heading, Some(180.0));
        assert_eq!(canadian.delay_minutes, Some(145));

        let regional = by_id(&trains, "amtrak:66-12");
        assert_eq!((regional.kind, regional.heading), (Kind::Regional, Some(45.0)));
        assert_eq!(regional.delay_minutes, Some(11), "standing at Philadelphia, 11 min late");
    }

    #[test]
    fn parses_entur() {
        let trains = parse_entur(ENTUR).unwrap();
        assert_eq!(trains.len(), 7);
        let bergen = by_id(&trains, "entur:VYG:ServiceJourney:61-LOD_617015-R");
        assert_eq!(bergen.name, "F4 Oslo S-Bergen");
        assert_eq!(bergen.kind, Kind::Intercity);
        assert_eq!(bergen.operator.as_deref(), Some("Vy"));
        assert_eq!(bergen.destination.as_deref(), Some("Bergen"));
        assert_eq!(bergen.delay_minutes, Some(3));
        assert!((bergen.heading.unwrap() - 270.45).abs() < 0.01);
        assert_eq!(bergen.speed_kmh, None);
        assert_eq!(bergen.position_unix, unix_ms("2026-10-13T06:11:44Z").unwrap() / 1000);

        let kinds: Vec<(&str, Kind)> = trains.iter().map(|t| (t.name.as_str(), t.kind)).collect();
        assert!(kinds.contains(&("L1 Spikkestad-Oslo S-Lillestrøm", Kind::Suburban)));
        assert!(kinds.contains(&("R21 Stabekk-Oslo S-Moss", Kind::Regional)));
        assert!(kinds.contains(&("FLY2", Kind::HighSpeed)));
        let flytoget = trains.iter().find(|t| t.name == "FLY2").unwrap();
        assert_eq!(flytoget.operator.as_deref(), Some("Flytoget"));
        let sorlandet = trains.iter().find(|t| t.name.starts_with("F5")).unwrap();
        assert_eq!(sorlandet.operator.as_deref(), Some("Go-Ahead Nordic"));
        assert!(trains.iter().any(|t| t.heading.is_none()));
    }

    #[test]
    fn parses_irishrail() {
        let trains = parse_irishrail(IRISHRAIL, 1_791_872_100_000).unwrap();
        // running trains only, and only those with a position
        assert_eq!(trains.len(), 1);
        let dart = &trains[0];
        assert_eq!(dart.id, "irishrail:12Oct2026:E133");
        assert_eq!(dart.name, "E133 Malahide to Greystones");
        assert_eq!(dart.kind, Kind::Suburban);
        assert_eq!(dart.destination.as_deref(), Some("Greystones"));
        assert_eq!(dart.operator.as_deref(), Some("DART"));
        assert_eq!(dart.delay_minutes, None, "the 1440-minute rollover isn't a delay");
        assert_eq!(dart.position_unix, 1_791_872_100);

        let block = "<objTrainPositions><TrainStatus>R</TrainStatus><TrainLatitude>53.3</TrainLatitude>\
            <TrainLongitude>-6.2</TrainLongitude><TrainCode>A201</TrainCode><TrainDate>13 Oct 2026</TrainDate>\
            <PublicMessage>A201\\n06:15 - Cork to Dublin Heuston (4 mins late)\\nDeparted Thurles</PublicMessage>";
        let body = format!("<ArrayOfObjTrainPositions>{block}</ArrayOfObjTrainPositions>");
        let t = &parse_irishrail(&body, 0).unwrap()[0];
        assert_eq!((t.kind, t.delay_minutes), (Kind::Intercity, Some(4)));
        assert_eq!(t.destination.as_deref(), Some("Dublin Heuston"));
        assert!(parse_irishrail("<html>down</html>", 0).is_err());
    }

    #[test]
    fn parses_radar_with_frames() {
        let now = 1_791_872_100_000; // 08:15:00 in Berlin
        let trains = parse_radar(RADAR, "vbb", now).unwrap();
        // the bus is left out
        assert_eq!(trains.len(), 3);

        let re1 = by_id(&trains, "vbb:1|36154|0|86|13102026");
        assert_eq!((re1.name.as_str(), re1.kind), ("RE1", Kind::Regional));
        assert_eq!(re1.destination.as_deref(), Some("Frankfurt (Oder)"));
        assert_eq!(re1.operator.as_deref(), Some("ODEG"));
        assert_eq!(re1.delay_minutes, Some(2));
        assert_eq!(re1.position_unix, 1_791_872_100);
        // from here towards Ostkreuz (reached at 08:16:30), cut at 30 s
        let f = &re1.frames;
        assert!(f.len() >= 2, "{f:?}");
        assert_eq!(f[0], (52.51, 13.435, now));
        assert_eq!(f.last().unwrap().2, now + 30_000);
        assert!(f.windows(2).all(|w| w[1].2 > w[0].2));
        let ahead = f.last().unwrap();
        assert!(ahead.1 > 13.435 && ahead.1 < 13.45, "a third of the way: {ahead:?}");
        let h = re1.heading.unwrap();
        assert!((90.0..110.0).contains(&h), "{h}");
        let v = re1.speed_kmh.unwrap();
        assert!((80.0..120.0).contains(&v), "{v}");

        let ice = by_id(&trains, "vbb:1|224466|0|80|13102026");
        assert_eq!((ice.name.as_str(), ice.kind), ("ICE 578", Kind::HighSpeed));
        assert!(ice.frames.is_empty() && ice.heading.is_none());

        let s5 = by_id(&trains, "vbb:1|7788|0|86|13102026");
        assert_eq!(s5.kind, Kind::Suburban);
        assert!(s5.frames.is_empty(), "no stop ahead with a time");
    }

    #[test]
    fn radar_kinds() {
        assert_eq!(radar_kind("nationalExpress", "ICE 1"), Some(Kind::HighSpeed));
        assert_eq!(radar_kind("national", "IC 2023"), Some(Kind::Intercity));
        assert_eq!(radar_kind("express", "ICE 100"), Some(Kind::HighSpeed));
        assert_eq!(radar_kind("express", "EC 41"), Some(Kind::Intercity));
        assert_eq!(radar_kind("regionalExpress", "RE 5"), Some(Kind::Regional));
        assert_eq!(radar_kind("subway", "U2"), Some(Kind::Metro));
        assert_eq!(radar_kind("tram", "M10"), None);
        assert_eq!(radar_kind("bus", "100"), None);
    }

    #[test]
    fn areas_and_coverage() {
        let berlin = Area::view(13.0, 52.3, 13.8, 52.7).unwrap();
        assert_eq!(sources_for(&berlin, 9.0), [Source::Vbb]);
        assert!(sources_for(&berlin, 5.0).is_empty(), "radar only close in");
        let helsinki = Area::view(24.5, 60.0, 25.5, 60.4).unwrap();
        assert_eq!(sources_for(&helsinki, 3.0), [Source::Digitraffic]);
        let nordics = Area::view(5.0, 58.0, 30.0, 70.0).unwrap();
        assert_eq!(sources_for(&nordics, 4.0), [Source::Digitraffic, Source::Entur]);
        let world = Area::view(-180.0, -85.0, 180.0, 85.0).unwrap();
        assert_eq!(sources_for(&world, 1.0).len(), 4);
        assert_eq!(sources_for(&world, 7.0).len(), 5);
        // across the antimeridian: Alaska to Japan sees none, a wrap to the US sees Amtrak
        let pacific = Area::view(140.0, 40.0, -140.0, 60.0).unwrap();
        assert!(sources_for(&pacific, 3.0).is_empty());
        let wide = Area::view(140.0, 30.0, -100.0, 50.0).unwrap();
        assert_eq!(sources_for(&wide, 3.0), [Source::Amtrak]);
        // unwrapped longitudes from the map
        let shifted = Area::view(384.5, 60.0, 385.5, 60.4).unwrap();
        assert!((shifted.west - 24.5).abs() < 1e-9);
        assert!(Area::view(f64::NAN, 0.0, 1.0, 1.0).is_none());
        assert!(Area::view(-200.0, -10.0, 200.0, 10.0).unwrap().contains(0.0, 179.0));
        assert!(pacific.contains(50.0, 179.5) && pacific.contains(50.0, -150.0) && !pacific.contains(50.0, 0.0));
    }

    #[test]
    fn radar_asks_a_bounded_snapped_area() {
        let big = Area::view(5.0, 47.0, 15.0, 55.0).unwrap();
        let a = radar_area(&big);
        assert!((a.north - a.south - 2.0 * RADAR_HALF_LAT).abs() < 1e-9);
        assert!((a.east - a.west - 2.0 * RADAR_HALF_LON).abs() < 1e-9);
        assert!((a.south + a.north) / 2.0 - 51.0 < 1e-9);
        // a small pan asks the same area
        let (k1, _) = Source::Vbb.request(&Area::view(13.30, 52.40, 13.50, 52.60).unwrap());
        let (k2, _) = Source::Vbb.request(&Area::view(13.31, 52.41, 13.51, 52.61).unwrap());
        assert_eq!(k1, k2);
        assert_eq!(Source::Amtrak.request(&big).0, "all");
    }

    #[test]
    fn caches_and_backs_off() {
        let mut s = SourceState::default();
        let trains = Arc::new(vec![]);
        s.store("a".into(), trains);
        assert!(matches!(s.cached("a", Duration::from_secs(30)), Some((true, _))));
        assert!(matches!(s.cached("a", Duration::ZERO), Some((false, _))), "stale but usable");
        assert!(s.cached("b", Duration::from_secs(30)).is_none());
        for i in 0..40 {
            s.store(format!("k{i}"), Arc::new(vec![]));
        }
        assert!(s.entries.len() <= CACHE_LIMIT);

        assert!(s.reserve().is_zero());
        assert!(s.reserve() > GAP - Duration::from_millis(50));
        s.failed(None);
        let first = s.blocked_until.unwrap();
        s.failed(None);
        assert!(s.blocked_until.unwrap() > first, "backoff grows");
        for _ in 0..20 {
            s.failed(None);
        }
        assert!(s.blocked_until.unwrap() <= Instant::now() + BACKOFF_MAX);
        s.failed(Some(Duration::from_secs(5)));
        assert!(s.blocked().unwrap() <= Duration::from_secs(5));
        s.succeeded();
        assert!(s.blocked().is_none() && s.failures == 0);
    }

    #[test]
    fn geometry() {
        assert_eq!(compass("NE"), Some(45.0));
        assert_eq!(compass("w"), Some(270.0));
        assert_eq!(compass("up"), None);
        let d = distance((60.0, 25.0), (61.0, 25.0));
        assert!((d - 111_195.0).abs() < 100.0);
        assert!((bearing((60.0, 25.0), (61.0, 25.0)) - 0.0).abs() < 0.01);
        assert!((bearing((60.0, 25.0), (60.0, 26.0)) - 90.0).abs() < 1.0);
    }

    #[test]
    fn serializes_camel_case() {
        let t = &parse_amtraker(AMTRAKER).unwrap()[0];
        let json = serde_json::to_value(t).unwrap();
        for key in ["speedKmh", "delayMinutes", "positionUnix", "frames", "heading", "destination", "operator"] {
            assert!(json.get(key).is_some(), "{key}");
        }
        assert!(matches!(json["kind"].as_str(), Some("intercity" | "regional" | "highSpeed")));
        let radar = parse_radar(RADAR, "vbb", 1_791_872_100_000).unwrap();
        let frames = serde_json::to_value(&radar[0].frames).unwrap();
        assert_eq!(frames[0], serde_json::json!([52.51, 13.435, 1_791_872_100_000_i64]));
    }

    // Live checks, one request each: `cargo test --lib trains -- --ignored --test-threads 1`

    async fn live(source: Source) -> Vec<Train> {
        let trains = fetch(source, &radar_area(&Area::view(13.2, 52.4, 13.6, 52.6).unwrap())).await.unwrap_or_else(|(e, _)| panic!("{e}"));
        println!("{}: {} trains; first {:?}", source.id(), trains.len(), trains.first());
        trains
    }

    #[tokio::test]
    #[ignore]
    async fn live_digitraffic() {
        assert!(!live(Source::Digitraffic).await.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn live_amtrak() {
        assert!(!live(Source::Amtrak).await.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn live_entur() {
        assert!(!live(Source::Entur).await.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn live_irishrail() {
        // the network sleeps at night
        live(Source::IrishRail).await;
    }

    #[tokio::test]
    #[ignore]
    async fn live_vbb() {
        live(Source::Vbb).await;
    }
}
