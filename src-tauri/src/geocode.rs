//! Places and addresses from Photon (komoot's geocoder over OpenStreetMap),
//! for the search box: type a street, house number, town, landmark or
//! postcode and fly there.
//!
//! Photon is built for search-as-you-type (Nominatim's policy forbids it), but
//! the public instance is a shared, free service, so it is used politely: the
//! search box debounces typed text, calls are at least a second apart, only the
//! newest query waits for its turn, recent answers are reused, and a refusal
//! (429) or server error backs off. The data is © OpenStreetMap contributors
//! (ODbL) and is credited where the results are shown.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::CmdResult;

const URL: &str = "https://photon.komoot.io/api/";
const USER_AGENT: &str = concat!(
    "RustRadar/",
    env!("CARGO_PKG_VERSION"),
    " (desktop flight radar; place search; +https://github.com/felixgoff/RustRadar)"
);
/// Minimum gap between calls.
const GAP: Duration = Duration::from_secs(1);
/// How long an answer is reused: places hardly move.
const TTL: Duration = Duration::from_secs(30 * 60);
/// Backoff after a failure: doubling from this, up to the maximum.
const BACKOFF_BASE: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
/// Cache entries kept at most (the least recently used go first).
const CACHE_LIMIT: usize = 64;
/// Asked of Photon; near-duplicates are then merged and the list cut to `SHOWN`.
const ASKED: usize = 8;
const SHOWN: usize = 6;
const MIN_QUERY: usize = 3;
const MAX_QUERY: usize = 200;
/// Languages Photon has names for; anything else gets the local names.
const LANGUAGES: [&str; 4] = ["en", "de", "fr", "it"];

/// A coarse category, for the icon and how close to fly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A house number, or a building without a name.
    Address,
    Street,
    /// A named building, landmark, shop, station, park, canal...
    Poi,
    Airport,
    /// A suburb, neighbourhood or locality within a town.
    District,
    City,
    /// A town, village or hamlet.
    Town,
    Postcode,
    /// A state, province, county or region.
    Region,
    Country,
}

#[derive(Clone, Debug, Serialize)]
pub struct Place {
    /// What the place is called: its name, or the street and number.
    pub name: String,
    /// A readable address line, e.g. `Damrak 1, 1012 LG Amsterdam, Netherlands`.
    pub label: String,
    pub kind: Kind,
    /// What a named place is, from OpenStreetMap's tag, e.g. `Hotel`, `Canal`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub lat: f64,
    pub lon: f64,
    /// `[west, south, east, north]`, degrees, for places with an extent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
}

fn text<'a>(p: &'a Value, key: &str) -> &'a str {
    p[key].as_str().map(str::trim).unwrap_or("")
}

/// Countries that write the house number before the street (`5 Avenue Anatole France`).
const NUMBER_FIRST: &[&str] = &[
    "US", "CA", "GB", "IE", "AU", "NZ", "FR", "BE", "LU", "MC", "IN", "ZA", "SG", "MY", "PH", "IL",
    "TH", "HK", "NG", "KE", "GH", "JM", "TT", "PK", "LK",
];
/// Countries that write the postcode after the town (`London TW19 7NR`).
const POSTCODE_LAST: &[&str] = &[
    "US", "CA", "GB", "IE", "AU", "NZ", "IN", "BR", "JP", "KR", "CN", "TW", "ZA", "SG", "MY", "PH",
    "TH", "HK", "NG", "KE", "PK", "LK", "ID", "AR", "MX", "RU", "UA", "KZ",
];

fn street_line(street: &str, number: &str, country: &str) -> String {
    match (street.is_empty(), number.is_empty()) {
        (true, _) => String::new(),
        (false, true) => street.to_owned(),
        (false, false) if NUMBER_FIRST.contains(&country) => format!("{number} {street}"),
        (false, false) => format!("{street} {number}"),
    }
}

fn locality_line(postcode: &str, town: &str, country: &str) -> String {
    match (postcode.is_empty(), town.is_empty()) {
        (true, _) => town.to_owned(),
        (false, true) => postcode.to_owned(),
        (false, false) if POSTCODE_LAST.contains(&country) => format!("{town} {postcode}"),
        (false, false) => format!("{postcode} {town}"),
    }
}

/// The coarse category of a feature from its OpenStreetMap tag and Photon's type.
pub fn kind_of(key: &str, value: &str, kind: &str, named: bool) -> Kind {
    match (key, value) {
        ("aeroway", "aerodrome" | "airport" | "airstrip" | "heliport") => return Kind::Airport,
        ("place", "country") => return Kind::Country,
        (
            "place",
            "state" | "province" | "region" | "county" | "district" | "archipelago"
            | "municipality",
        ) => return Kind::Region,
        ("place", "city") => return Kind::City,
        ("place", "town" | "village" | "hamlet" | "borough") => return Kind::Town,
        (
            "place",
            "suburb" | "quarter" | "neighbourhood" | "city_block" | "locality"
            | "isolated_dwelling" | "farm" | "allotments" | "plot" | "square",
        ) => return Kind::District,
        ("place", "postcode") | ("boundary", "postal_code") => return Kind::Postcode,
        ("place", "house" | "building") => return Kind::Address,
        ("highway", _) if kind == "street" => return Kind::Street,
        _ => {}
    }
    match kind {
        "country" => Kind::Country,
        "state" | "county" => Kind::Region,
        "city" => Kind::City,
        "district" | "locality" if key == "boundary" || key == "place" => Kind::District,
        "street" => Kind::Street,
        "house" if !named => Kind::Address,
        _ => Kind::Poi,
    }
}

/// `tourism=hotel` → `Hotel`; generic values say nothing and are left out.
fn category(key: &str, value: &str) -> Option<String> {
    if matches!(value, "yes" | "" | "administrative" | "house" | "building")
        || matches!(key, "place" | "boundary")
    {
        return None;
    }
    let words = value.replace('_', " ");
    let mut chars = words.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

/// The parts of the address line, most specific first, without repeats.
fn label_parts(p: &Value, kind: Kind, name: &str) -> Vec<String> {
    let country_code = text(p, "countrycode").to_ascii_uppercase();
    let cc = country_code.as_str();
    let town = [text(p, "city"), text(p, "locality"), text(p, "district")]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("");
    let postcode = text(p, "postcode");
    let mut parts = vec![name.to_owned()];
    match kind {
        Kind::Address | Kind::Poi | Kind::Street => {
            parts.push(street_line(text(p, "street"), text(p, "housenumber"), cc));
            parts.push(locality_line(postcode, town, cc));
        }
        Kind::Airport => parts.push(town.to_owned()),
        Kind::Postcode => {
            parts.push(text(p, "district").to_owned());
            parts.push(text(p, "city").to_owned());
        }
        Kind::District => parts.push(text(p, "city").to_owned()),
        Kind::City | Kind::Town | Kind::Region | Kind::Country => {}
    }
    // the state tells same-named towns apart; past street level it is noise
    if matches!(
        kind,
        Kind::City | Kind::Town | Kind::District | Kind::Postcode | Kind::Region
    ) {
        parts.push(text(p, "county").to_owned());
        parts.push(text(p, "state").to_owned());
    }
    parts.push(text(p, "country").to_owned());

    let mut out: Vec<String> = Vec::new();
    for part in parts {
        let part = part.trim();
        if !part.is_empty()
            && !out
                .iter()
                .any(|o| o.eq_ignore_ascii_case(part) || o == part)
        {
            out.push(part.to_owned());
        }
    }
    out
}

/// Photon's `extent` is `[minLon, maxLat, maxLon, minLat]`.
fn bbox(p: &Value) -> Option<[f64; 4]> {
    let e = p["extent"].as_array()?;
    let v: Vec<f64> = e.iter().filter_map(Value::as_f64).collect();
    let [west, north, east, south] = <[f64; 4]>::try_from(v).ok()?;
    let ok = [west, east].iter().all(|x| x.abs() <= 180.0)
        && [south, north].iter().all(|y| y.abs() <= 90.0)
        && south <= north;
    ok.then_some([west, south, east, north])
}

fn place(feature: &Value) -> Option<Place> {
    let coords = feature["geometry"]["coordinates"].as_array()?;
    let lon = coords.first()?.as_f64().filter(|x| x.abs() <= 180.0)?;
    let lat = coords.get(1)?.as_f64().filter(|y| y.abs() <= 90.0)?;
    let p = &feature["properties"];
    let (key, value) = (text(p, "osm_key"), text(p, "osm_value"));
    let cc = text(p, "countrycode").to_ascii_uppercase();
    let named = !text(p, "name").is_empty();
    let kind = kind_of(key, value, text(p, "type"), named);
    let name = if named {
        text(p, "name").to_owned()
    } else {
        [
            street_line(text(p, "street"), text(p, "housenumber"), &cc),
            text(p, "postcode").to_owned(),
            text(p, "city").to_owned(),
        ]
        .into_iter()
        .find(|s| !s.is_empty())?
    };
    let label = label_parts(p, kind, &name).join(", ");
    Some(Place {
        category: if kind == Kind::Poi {
            category(key, value)
        } else {
            None
        },
        name,
        label,
        kind,
        lat,
        lon,
        bbox: bbox(p),
    })
}

/// The places of a Photon response, near-duplicates merged (a canal or a long
/// road comes back once per piece), at most `SHOWN`.
pub fn parse(body: &str) -> Result<Vec<Place>, String> {
    let json: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let features = json["features"]
        .as_array()
        .ok_or("response has no features")?;
    let mut out: Vec<Place> = Vec::new();
    for place in features.iter().filter_map(place) {
        if !out.iter().any(|o| o.label == place.label) {
            out.push(place);
        }
        if out.len() == SHOWN {
            break;
        }
    }
    Ok(out)
}

/// Responses kept for reuse, and when we may call again.
#[derive(Default)]
pub struct Cache {
    /// Answer, when it was fetched, and when it was last used.
    entries: HashMap<String, (Instant, Instant, Vec<Place>)>,
    /// The earliest moment the next call may go out.
    next: Option<Instant>,
    /// No calls before this: the last ones failed or were refused.
    blocked_until: Option<Instant>,
    failures: u32,
    /// Bumped by every query; a waiting query that is no longer the newest gives up.
    generation: u64,
}

impl Cache {
    fn fresh(&mut self, key: &str) -> Option<Vec<Place>> {
        let (fetched, used, places) = self.entries.get_mut(key)?;
        if fetched.elapsed() >= TTL {
            return None;
        }
        *used = Instant::now();
        Some(places.clone())
    }

    fn store(&mut self, key: String, places: Vec<Place>) {
        if self.entries.len() >= CACHE_LIMIT
            && !self.entries.contains_key(&key)
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used, _))| *used)
                .map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
        let now = Instant::now();
        self.entries.insert(key, (now, now, places));
    }

    /// Takes the next call slot if it is free; otherwise how long until it is.
    fn try_reserve(&mut self) -> Result<(), Duration> {
        let now = Instant::now();
        match self.next {
            Some(at) if at > now => Err(at - now),
            _ => {
                self.next = Some(now + GAP);
                Ok(())
            }
        }
    }

    fn resting(&self) -> Option<Duration> {
        let left = self
            .blocked_until?
            .saturating_duration_since(Instant::now());
        (!left.is_zero()).then_some(left)
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

/// The query as sent and cached: trimmed, single-spaced, at most `MAX_QUERY` characters.
fn normalise(query: &str) -> String {
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_QUERY)
        .collect()
}

/// A bias point rounded to 0.1° (~11 km): plenty for ranking, and nearby views share answers.
fn bias(lat: Option<f64>, lon: Option<f64>) -> Option<(f64, f64)> {
    let (lat, lon) = (lat?, lon?);
    if !(lat.is_finite() && lon.is_finite()) {
        return None;
    }
    let round = |x: f64| (x * 10.0).round() / 10.0;
    Some((
        round(lat.clamp(-90.0, 90.0)),
        round((lon + 180.0).rem_euclid(360.0) - 180.0),
    ))
}

async fn request(
    query: &str,
    bias: Option<(f64, f64)>,
    lang: Option<&str>,
) -> Result<Vec<Place>, (String, bool, Option<Duration>)> {
    let mut params = vec![("q", query.to_owned()), ("limit", ASKED.to_string())];
    if let Some((lat, lon)) = bias {
        params.push(("lat", format!("{lat:.1}")));
        params.push(("lon", format!("{lon:.1}")));
    }
    if let Some(lang) = lang {
        params.push(("lang", lang.to_owned()));
    }
    let response = client()
        .map_err(|e| (e, false, None))?
        .get(URL)
        .query(&params)
        .header("accept", "application/json")
        .send()
        .await
        // offline or unreachable: rest too, so typing doesn't hammer a dead link
        .map_err(|e| (format!("can't reach Photon: {e}"), true, None))?;
    let status = response.status();
    if !status.is_success() {
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let back_off = status.as_u16() == 429 || status.is_server_error();
        let message = if status.as_u16() == 429 {
            "Photon asked us to slow down".to_owned()
        } else {
            format!("Photon answered {status}")
        };
        return Err((message, back_off, retry_after));
    }
    let body = response
        .text()
        .await
        .map_err(|e| (e.to_string(), true, None))?;
    parse(&body).map_err(|e| (e, false, None))
}

/// Places and addresses matching `query`, ranked toward `lat`/`lon` (the
/// view's centre) when given, named in `lang` where Photon knows it.
#[tauri::command]
pub async fn geocode(
    query: String,
    lat: Option<f64>,
    lon: Option<f64>,
    lang: Option<String>,
) -> CmdResult<Vec<Place>> {
    let query = normalise(&query);
    if query.chars().count() < MIN_QUERY {
        return Ok(Vec::new());
    }
    let lang = lang
        .map(|l| l.trim().to_ascii_lowercase())
        .filter(|l| LANGUAGES.contains(&l.as_str()));
    let bias = bias(lat, lon);
    let key = format!(
        "{}|{}|{:?}",
        query.to_lowercase(),
        lang.as_deref().unwrap_or(""),
        bias
    );

    let generation = {
        let mut cache = state().lock().await;
        cache.generation += 1;
        cache.generation
    };
    loop {
        let wait = {
            let mut cache = state().lock().await;
            if let Some(hit) = cache.fresh(&key) {
                return Ok(hit);
            }
            if cache.generation != generation {
                return Err("superseded by a newer search".into());
            }
            if let Some(left) = cache.resting() {
                return Err(format!(
                    "Photon is resting; retrying in {} s",
                    left.as_secs().max(1)
                ));
            }
            match cache.try_reserve() {
                Ok(()) => break,
                Err(wait) => wait,
            }
        };
        tokio::time::sleep(wait).await;
    }

    let result = request(&query, bias, lang.as_deref()).await;
    let mut cache = state().lock().await;
    match result {
        Ok(places) => {
            cache.succeeded();
            cache.store(key, places.clone());
            Ok(places)
        }
        Err((message, back_off, retry_after)) => {
            if back_off {
                cache.failed(retry_after);
            }
            Err(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<Place> {
        let body = std::fs::read_to_string(format!(
            "{}/tests/data/photon-{name}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        parse(&body).unwrap()
    }

    fn find<'a>(places: &'a [Place], name: &str) -> &'a Place {
        places
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("no {name} in {places:#?}"))
    }

    #[test]
    fn an_address() {
        let places = fixture("damrak-1-amsterdam");
        let house = find(&places, "Damrak 1");
        assert_eq!(house.kind, Kind::Address);
        assert_eq!(house.label, "Damrak 1, 1012 LG Amsterdam, Netherlands");
        assert_eq!((house.lat, house.lon), (52.3770941, 4.8979782));
        assert!(house.bbox.is_none());
        assert!(house.category.is_none());

        let hotel = find(&places, "Park Plaza Victoria Hotel");
        assert_eq!(hotel.kind, Kind::Poi);
        assert_eq!(hotel.category.as_deref(), Some("Hotel"));
        assert_eq!(
            hotel.label,
            "Park Plaza Victoria Hotel, Damrak 1-5, 1012 LG Amsterdam, Netherlands"
        );
    }

    #[test]
    fn pieces_of_one_thing_come_back_once() {
        let places = fixture("damrak-1-amsterdam");
        // three canal segments: two share a postcode, one is elsewhere in town
        let canals: Vec<_> = places.iter().filter(|p| p.name == "Damrak").collect();
        assert_eq!(canals.len(), 3);
        let labels: std::collections::HashSet<_> = canals.iter().map(|p| &p.label).collect();
        assert_eq!(labels.len(), 3);
        assert!(
            canals
                .iter()
                .all(|c| c.category.as_deref() == Some("Canal"))
        );
        // extent is [minLon, maxLat, maxLon, minLat]; bbox is [west, south, east, north]
        let first = canals[0].bbox.unwrap();
        assert_eq!(first, [4.8985257, 52.37675, 4.8988777, 52.3769698]);
    }

    #[test]
    fn a_landmark_and_streets() {
        let places = fixture("eiffel");
        let tower = find(&places, "Eiffel Tower");
        assert_eq!(tower.kind, Kind::Poi);
        assert_eq!(tower.category.as_deref(), Some("Tower"));
        // France puts the number first
        assert_eq!(
            tower.label,
            "Eiffel Tower, 5 Avenue Anatole France, 75007 Paris, France"
        );
        let street = places
            .iter()
            .find(|p| p.kind == Kind::Street)
            .expect("a street");
        assert_eq!(street.name, "Eiffel");
        assert_eq!(street.label, "Eiffel, 170501 Quito, Ecuador");
        assert!(places.len() <= SHOWN);
    }

    #[test]
    fn towns_and_regions_say_where_they_are() {
        let places = fixture("paris");
        assert_eq!(places[0].kind, Kind::City);
        assert_eq!(places[0].label, "Paris, Île-de-France, France");
        // the admin boundary of the same city is merged into it
        assert_eq!(
            places
                .iter()
                .filter(|p| p.label == "Paris, Île-de-France, France")
                .count(),
            1
        );
        let texas = places.iter().find(|p| p.label.contains("Texas")).unwrap();
        assert_eq!(texas.kind, Kind::Town);
        assert_eq!(texas.label, "Paris, Lamar, Texas, United States");

        let bavaria = &fixture("bavaria")[0];
        assert_eq!(bavaria.kind, Kind::Region);
        assert_eq!(bavaria.label, "Bavaria, Germany");
        assert!(bavaria.bbox.is_some());

        let netherlands = fixture("netherlands");
        assert_eq!(netherlands[0].kind, Kind::Country);
        assert_eq!(netherlands[0].label, "Netherlands");
    }

    #[test]
    fn postcodes_and_airports() {
        let codes = fixture("10115");
        assert!(codes.iter().all(|p| p.kind == Kind::Postcode));
        let nyc = codes
            .iter()
            .find(|p| p.label.contains("United States"))
            .unwrap();
        assert_eq!(nyc.name, "10115");
        assert_eq!(nyc.label, "10115, Manhattan, New York, United States");

        let airports = fixture("heathrow");
        let heathrow = find(&airports, "Heathrow Airport");
        assert_eq!(heathrow.kind, Kind::Airport);
        assert_eq!(heathrow.label, "Heathrow Airport, London, United Kingdom");
    }

    #[test]
    fn labels_follow_local_conventions() {
        assert_eq!(street_line("Main Street", "12", "US"), "12 Main Street");
        assert_eq!(street_line("Damrak", "1", "NL"), "Damrak 1");
        assert_eq!(street_line("", "1", "NL"), "");
        assert_eq!(locality_line("TW19 7NR", "London", "GB"), "London TW19 7NR");
        assert_eq!(
            locality_line("1012 LG", "Amsterdam", "NL"),
            "1012 LG Amsterdam"
        );
        assert_eq!(locality_line("", "Amsterdam", "NL"), "Amsterdam");
    }

    #[test]
    fn tolerates_odd_responses() {
        assert!(parse("{}").is_err());
        assert!(parse("not json").is_err());
        let places = parse(
            r#"{"features":[
                {"properties":{"name":"Nowhere"},"geometry":{"coordinates":[500,0]}},
                {"properties":{},"geometry":{"coordinates":[1,2]}},
                {"properties":{"name":"Odd","type":"other","extent":[1,2]},"geometry":{"coordinates":[1,2]}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            places.len(),
            1,
            "out of range and nameless places are dropped"
        );
        assert_eq!(places[0].label, "Odd");
        assert_eq!(places[0].kind, Kind::Poi);
        assert!(places[0].bbox.is_none());
    }

    #[test]
    fn queries_and_bias_are_normalised() {
        assert_eq!(
            normalise("  Damrak   1 \n Amsterdam "),
            "Damrak 1 Amsterdam"
        );
        assert_eq!(normalise(&"x".repeat(500)).len(), MAX_QUERY);
        assert_eq!(bias(Some(52.377), Some(4.898)), Some((52.4, 4.9)));
        assert_eq!(bias(Some(10.0), Some(190.0)), Some((10.0, -170.0)));
        assert_eq!(bias(Some(f64::NAN), Some(0.0)), None);
        assert_eq!(bias(None, Some(0.0)), None);
    }

    #[test]
    fn calls_are_spaced_and_back_off() {
        let mut cache = Cache::default();
        assert!(cache.try_reserve().is_ok());
        let wait = cache.try_reserve().unwrap_err();
        assert!(wait > GAP - Duration::from_millis(50) && wait <= GAP);

        assert!(cache.resting().is_none());
        cache.failed(None);
        let first = cache.blocked_until.unwrap();
        assert!(cache.resting().is_some());
        cache.failed(None);
        assert!(cache.blocked_until.unwrap() > first, "backoff grows");
        for _ in 0..20 {
            cache.failed(None);
        }
        assert!(cache.blocked_until.unwrap() <= Instant::now() + BACKOFF_MAX);
        cache.failed(Some(Duration::from_secs(30)));
        assert!(cache.blocked_until.unwrap() <= Instant::now() + Duration::from_secs(30));
        cache.succeeded();
        assert!(cache.resting().is_none() && cache.failures == 0);
    }

    #[test]
    fn the_cache_keeps_recently_used_answers() {
        let mut cache = Cache::default();
        cache.store("keep".into(), fixture("netherlands"));
        for i in 0..CACHE_LIMIT - 1 {
            cache.store(format!("k{i}"), Vec::new());
        }
        // touching an entry makes it the most recently used
        std::thread::sleep(Duration::from_millis(2));
        assert_eq!(cache.fresh("keep").unwrap()[0].name, "Netherlands");
        for i in 0..10 {
            cache.store(format!("new{i}"), Vec::new());
        }
        assert!(cache.entries.len() <= CACHE_LIMIT);
        assert!(cache.fresh("keep").is_some(), "recently used survives");
        assert!(cache.fresh("k0").is_none(), "least recently used goes");
        assert!(cache.fresh("missing").is_none());
    }
    /// Talks to the real service: `cargo test -p rustradar live_photon -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_photon() {
        let first_sent = Instant::now();
        let places = geocode(
            "Damrak 1 Amsterdam".into(),
            Some(52.37),
            Some(4.9),
            Some("en".into()),
        )
        .await
        .unwrap();
        assert!(
            places
                .iter()
                .any(|p| p.label == "Damrak 1, 1012 LG Amsterdam, Netherlands"),
            "{places:#?}"
        );
        // the same query again comes from the cache, even while the next slot is a second away
        let started = Instant::now();
        let again = geocode(
            "damrak  1 amsterdam".into(),
            Some(52.371),
            Some(4.901),
            Some("en".into()),
        )
        .await
        .unwrap();
        assert_eq!(again.len(), places.len());
        assert!(started.elapsed() < Duration::from_millis(50));
        // a new query goes out no sooner than a second after the last one
        let city = geocode("Rotterdam".into(), None, None, None).await.unwrap();
        assert_eq!(city[0].kind, Kind::City);
        assert!(first_sent.elapsed() >= GAP);
    }
}
