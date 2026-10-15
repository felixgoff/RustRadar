//! Sweden's trains from Trafikverket's open API (api.trafikinfo.trafikverket.se),
//! with the key from the keys page. Three object types make a train:
//!
//! - `TrainPosition`: where each train is, from its GPS (every train on the
//!   national network, freight included), with bearing and speed.
//! - `TrainAnnouncement`: the timetable as announced to travellers, one row per
//!   arrival or departure at a station: the product ("SJ Snabbtåg"), who runs
//!   it, where it goes, and planned, estimated and actual times.
//! - `TrainStation`: station names and places for the stations' signatures
//!   ("Cst" is Stockholm Central).
//!
//! Positions are asked for on every refresh; the announcements around now and
//! the stations change slowly, so they are cached here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::trains::{Kind, Train, unix_ms};

const URL: &str = "https://api.trafikinfo.trafikverket.se/v2/data.json";
/// How long the announcements around now are reused before asking again.
const ANNOUNCEMENTS_TTL: Duration = Duration::from_secs(180);
const STATIONS_TTL: Duration = Duration::from_secs(24 * 3600);

/// The request body: the key, then the queries.
fn request(key: &str, queries: &str) -> String {
    format!("<REQUEST><LOGIN authenticationkey=\"{}\"/>{queries}</REQUEST>", xml_escape(key))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

fn includes(fields: &[&str]) -> String {
    fields.iter().map(|f| format!("<INCLUDE>{f}</INCLUDE>")).collect()
}

/// Every train that reported a position in the last quarter of an hour.
fn positions_query() -> String {
    format!(
        "<QUERY objecttype=\"TrainPosition\" namespace=\"järnväg.trafikinfo\" schemaversion=\"1.1\">\
         <FILTER><GT name=\"TimeStamp\" value=\"$dateadd(-0.00:15:00)\"/></FILTER>{}</QUERY>",
        includes(&[
            "Train.AdvertisedTrainNumber",
            "Train.OperationalTrainNumber",
            "Train.JourneyPlanDepartureDate",
            "Position.WGS84",
            "Bearing",
            "Speed",
            "TimeStamp",
            "Status.Active",
        ])
    )
}

const ANNOUNCEMENT_FIELDS: &[&str] = &[
    "AdvertisedTrainIdent",
    "ScheduledDepartureDateTime",
    "ActivityType",
    "LocationSignature",
    "AdvertisedTimeAtLocation",
    "EstimatedTimeAtLocation",
    "TimeAtLocation",
    "ProductInformation",
    "InformationOwner",
    "Operator",
    "ToLocation",
    "FromLocation",
    "Canceled",
];

/// Departures from an hour ago to half an hour ahead: every train running
/// now has one, and they carry what the positions lack.
fn announcements_query() -> String {
    format!(
        "<QUERY objecttype=\"TrainAnnouncement\" schemaversion=\"1.9\" orderby=\"AdvertisedTimeAtLocation\">\
         <FILTER><AND><EQ name=\"ActivityType\" value=\"Avgang\"/>\
         <GT name=\"AdvertisedTimeAtLocation\" value=\"$dateadd(-0.01:00:00)\"/>\
         <LT name=\"AdvertisedTimeAtLocation\" value=\"$dateadd(0.00:30:00)\"/></AND></FILTER>{}</QUERY>",
        includes(ANNOUNCEMENT_FIELDS)
    )
}

/// Every stop of one train on one day.
fn stops_query(ident: &str, date: &str) -> String {
    let more: Vec<&str> =
        ANNOUNCEMENT_FIELDS.iter().copied().chain(["TrackAtLocation", "Deviation", "OtherInformation"]).collect();
    format!(
        "<QUERY objecttype=\"TrainAnnouncement\" schemaversion=\"1.9\" orderby=\"AdvertisedTimeAtLocation\">\
         <FILTER><AND><EQ name=\"AdvertisedTrainIdent\" value=\"{}\"/>\
         <GTE name=\"ScheduledDepartureDateTime\" value=\"{date}T00:00:00\"/>\
         <LT name=\"ScheduledDepartureDateTime\" value=\"{date}T23:59:59\"/></AND></FILTER>{}</QUERY>",
        xml_escape(ident),
        includes(&more)
    )
}

fn stations_query() -> String {
    format!(
        "<QUERY objecttype=\"TrainStation\" namespace=\"rail.infrastructure\" schemaversion=\"1.4\">{}</QUERY>",
        includes(&["LocationSignature", "AdvertisedLocationName", "AdvertisedShortLocationName", "Geometry.WGS84"])
    )
}

/// The rows of each query in the answer, in order.
fn results(body: &str) -> Result<Vec<Value>, String> {
    let json: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let results = json["RESPONSE"]["RESULT"].as_array().cloned().unwrap_or_default();
    if let Some(error) = results.iter().find_map(|r| r["ERROR"]["MESSAGE"].as_str()) {
        return Err(error.to_owned());
    }
    Ok(results)
}

fn rows<'a>(result: &'a Value, kind: &str) -> &'a [Value] {
    result[kind].as_array().map(Vec::as_slice).unwrap_or_default()
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

/// `POINT (18.05 59.33)`, longitude first.
fn point(s: &str) -> Option<(f64, f64)> {
    let inner = s.trim().strip_prefix("POINT")?.trim().strip_prefix('(')?.strip_suffix(')')?;
    let mut it = inner.split_whitespace().map(|x| x.parse::<f64>().ok());
    let (lon, lat) = (it.next()??, it.next()??);
    (lat.is_finite() && lon.is_finite()).then_some((lat, lon))
}

/// The first description in a list of `{Code, Description}`.
fn description(v: &Value) -> Option<String> {
    v.as_array()?.iter().find_map(|x| text(&x["Description"]))
}

/// The destination's signature: the last of the "to" locations.
fn last_location(v: &Value) -> Option<String> {
    let list = v.as_array()?;
    list.iter()
        .max_by_key(|x| x["Order"].as_i64().unwrap_or(0))
        .and_then(|x| text(&x["LocationName"]))
}

// --------------------------------------------------------------- stations

#[derive(Clone, Debug, PartialEq)]
pub struct Station {
    pub name: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

pub fn parse_stations(body: &str) -> Result<HashMap<String, Station>, String> {
    let results = results(body)?;
    let mut stations = HashMap::new();
    for row in results.iter().flat_map(|r| rows(r, "TrainStation")) {
        let Some(sig) = text(&row["LocationSignature"]) else { continue };
        let name = text(&row["AdvertisedLocationName"])
            .or_else(|| text(&row["AdvertisedShortLocationName"]))
            .unwrap_or_else(|| sig.clone());
        let at = row["Geometry"]["WGS84"].as_str().and_then(point);
        stations.insert(sig, Station { name, lat: at.map(|p| p.0), lon: at.map(|p| p.1) });
    }
    Ok(stations)
}

// ----------------------------------------------------------- announcements

/// What the announcements say about one train, near now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Service {
    pub product: Option<String>,
    pub owner: Option<String>,
    /// Destination's signature.
    pub to: Option<String>,
    pub delay_minutes: Option<i32>,
    /// The day of its first departure, `YYYY-MM-DD`: with the number, which run it is.
    pub date: Option<String>,
    pub canceled: bool,
}

fn minutes_late(planned: &Value, at: &Value) -> Option<i32> {
    let planned = unix_ms(planned.as_str()?)?;
    let at = unix_ms(at.as_str()?)?;
    Some(((at - planned) as f64 / 60_000.0).round() as i32)
}

/// Departures near now, per advertised train number: the run whose rows lie
/// closest to now wins when a number runs on two days.
pub fn parse_announcements(body: &str, now_ms: i64) -> Result<HashMap<String, Service>, String> {
    let results = results(body)?;
    // (number, day) -> (rows, distance of its nearest row from now)
    let mut runs: HashMap<(String, String), (Vec<&Value>, i64)> = HashMap::new();
    for row in results.iter().flat_map(|r| rows(r, "TrainAnnouncement")) {
        let Some(ident) = text(&row["AdvertisedTrainIdent"]) else { continue };
        let day = row["ScheduledDepartureDateTime"].as_str().map(|s| s.chars().take(10).collect()).unwrap_or_default();
        let gap = row["AdvertisedTimeAtLocation"].as_str().and_then(unix_ms).map_or(i64::MAX, |t| (t - now_ms).abs());
        let run = runs.entry((ident, day)).or_insert_with(|| (Vec::new(), i64::MAX));
        run.0.push(row);
        run.1 = run.1.min(gap);
    }
    let mut best: HashMap<String, (i64, Service)> = HashMap::new();
    for ((ident, day), (rows, gap)) in runs {
        let first = rows[0];
        // the latest actual time says how late it is; failing that, the next estimate
        let actual = rows.iter().rev().find_map(|r| minutes_late(&r["AdvertisedTimeAtLocation"], &r["TimeAtLocation"]));
        let estimate = || rows.iter().find_map(|r| minutes_late(&r["AdvertisedTimeAtLocation"], &r["EstimatedTimeAtLocation"]));
        let service = Service {
            product: description(&first["ProductInformation"]),
            owner: text(&first["InformationOwner"]).or_else(|| text(&first["Operator"])),
            to: last_location(&first["ToLocation"]),
            delay_minutes: actual.or_else(estimate),
            date: (!day.is_empty()).then_some(day),
            canceled: rows.iter().all(|r| r["Canceled"].as_bool() == Some(true)),
        };
        if best.get(&ident).is_none_or(|(g, _)| gap < *g) {
            best.insert(ident, (gap, service));
        }
    }
    Ok(best.into_iter().map(|(k, (_, s))| (k, s)).collect())
}

/// The kind from the product name: Trafikverket has no category of its own.
fn kind(product: Option<&str>, owner: Option<&str>) -> Kind {
    let p = product.unwrap_or_default().to_lowercase();
    let o = owner.unwrap_or_default().to_lowercase();
    if p.contains("snabbtåg") || p.contains("x2000") || p.contains("arlanda express") {
        Kind::HighSpeed
    } else if p.contains("intercity") || p.contains("nattåg") || p.contains("snälltåget") || p.contains("vy") {
        Kind::Intercity
    } else if p.contains("pendeltåg") || p.contains("pendel") || o == "sl" {
        Kind::Suburban
    } else if p.is_empty() && o.is_empty() {
        // not announced to travellers: freight, empty stock, engineering trains
        Kind::Freight
    } else {
        Kind::Regional
    }
}

// --------------------------------------------------------------- positions

/// Trains from the positions, named and described from the announcements.
pub fn parse_positions(
    body: &str,
    services: &HashMap<String, Service>,
    stations: &HashMap<String, Station>,
) -> Result<Vec<Train>, String> {
    let results = results(body)?;
    let mut trains: HashMap<String, Train> = HashMap::new();
    for row in results.iter().flat_map(|r| rows(r, "TrainPosition")) {
        if row["Status"]["Active"].as_bool() == Some(false) {
            continue;
        }
        let Some((lat, lon)) = row["Position"]["WGS84"].as_str().and_then(point) else { continue };
        let Some(at) = row["TimeStamp"].as_str().and_then(unix_ms) else { continue };
        let advertised = text(&row["Train"]["AdvertisedTrainNumber"]);
        let operational = text(&row["Train"]["OperationalTrainNumber"]);
        let Some(number) = advertised.clone().or(operational.clone()) else { continue };
        let service = advertised.as_ref().and_then(|n| services.get(n));
        let date = service
            .and_then(|s| s.date.clone())
            .or_else(|| row["Train"]["JourneyPlanDepartureDate"].as_str().map(|s| s.chars().take(10).collect()))
            .unwrap_or_default();
        let product = service.and_then(|s| s.product.clone());
        let owner = service.and_then(|s| s.owner.clone());
        let name = match &product {
            Some(p) => format!("{p} {number}"),
            None => format!("Train {number}"),
        };
        let train = Train {
            // the advertised number is what the stops are looked up by
            id: format!("trafikverket:{date}:{}", advertised.unwrap_or_else(|| format!("~{number}"))),
            source: "trafikverket",
            name,
            kind: kind(product.as_deref(), owner.as_deref()),
            lat,
            lon,
            heading: row["Bearing"].as_f64().map(|b| b as f32),
            speed_kmh: row["Speed"].as_f64().map(|s| s as f32),
            destination: service
                .and_then(|s| s.to.as_ref())
                .map(|sig| stations.get(sig).map_or_else(|| sig.clone(), |st| st.name.clone())),
            operator: owner,
            delay_minutes: service.and_then(|s| s.delay_minutes),
            position_unix: at / 1000,
            frames: Vec::new(),
        };
        // a train reports again and again: keep its newest
        match trains.get(&train.id) {
            Some(old) if old.position_unix >= train.position_unix => {}
            _ => {
                trains.insert(train.id.clone(), train);
            }
        }
    }
    Ok(trains.into_values().collect())
}

// ------------------------------------------------------------------- stops

/// A time at a stop: as planned, as now expected, and as it happened.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StopTime {
    pub planned: Option<i64>,
    pub estimated: Option<i64>,
    pub actual: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stop {
    pub name: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// Unix seconds; arrival is empty at the first stop, departure at the last.
    pub arrival: Option<StopTime>,
    pub departure: Option<StopTime>,
    pub track: Option<String>,
    pub canceled: bool,
    /// Notices at this stop: delay causes, replacement buses and so on.
    pub notes: Vec<String>,
}

fn stop_time(row: &Value) -> StopTime {
    let secs = |v: &Value| v.as_str().and_then(unix_ms).map(|ms| ms / 1000);
    StopTime {
        planned: secs(&row["AdvertisedTimeAtLocation"]),
        estimated: secs(&row["EstimatedTimeAtLocation"]),
        actual: secs(&row["TimeAtLocation"]),
    }
}

/// One train's rows, a stop per station in order (an arrival and a departure
/// at the same station make one stop).
pub fn parse_stops(body: &str, stations: &HashMap<String, Station>) -> Result<Vec<Stop>, String> {
    let results = results(body)?;
    let mut stops: Vec<(String, Stop)> = Vec::new();
    for row in results.iter().flat_map(|r| rows(r, "TrainAnnouncement")) {
        let Some(sig) = text(&row["LocationSignature"]) else { continue };
        let departure = row["ActivityType"].as_str() == Some("Avgang");
        // a departure follows its station's arrival; anything else starts a new stop
        let index = match stops.last() {
            Some((last, stop)) if *last == sig && departure && stop.departure.is_none() => stops.len() - 1,
            _ => {
                let station = stations.get(&sig);
                stops.push((
                    sig.clone(),
                    Stop {
                        name: station.map_or_else(|| sig.clone(), |s| s.name.clone()),
                        lat: station.and_then(|s| s.lat),
                        lon: station.and_then(|s| s.lon),
                        arrival: None,
                        departure: None,
                        track: None,
                        canceled: false,
                        notes: Vec::new(),
                    },
                ));
                stops.len() - 1
            }
        };
        let stop = &mut stops[index].1;
        if departure {
            stop.departure = Some(stop_time(row));
        } else {
            stop.arrival = Some(stop_time(row));
        }
        stop.track = stop.track.take().or_else(|| text(&row["TrackAtLocation"]));
        stop.canceled |= row["Canceled"].as_bool() == Some(true);
        for list in ["Deviation", "OtherInformation"] {
            for note in row[list].as_array().into_iter().flatten().filter_map(|n| text(&n["Description"])) {
                if !stop.notes.contains(&note) {
                    stop.notes.push(note);
                }
            }
        }
    }
    Ok(stops.into_iter().map(|(_, s)| s).collect())
}

// ---------------------------------------------------------------- fetching

async fn post(http: &reqwest::Client, key: &str, queries: &str) -> Result<String, String> {
    let response = http
        .post(URL)
        .header("content-type", "text/xml; charset=utf-8")
        .body(request(key, queries))
        .send()
        .await
        .map_err(|e| format!("can't reach Trafikverket: {e}"))?;
    let status = response.status();
    let body = response.text().await.map_err(|e| format!("Trafikverket: {e}"))?;
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err("Trafikverket didn't accept the API key (check it on the keys page)".into());
    }
    if !status.is_success() {
        // its errors come as JSON with a message worth showing
        let message = results(&body).err().unwrap_or_else(|| status.to_string());
        return Err(format!("Trafikverket answered {message}"));
    }
    Ok(body)
}

type Cached<T> = Mutex<Option<(Instant, String, Arc<T>)>>;

fn station_cache() -> &'static Cached<HashMap<String, Station>> {
    static C: OnceLock<Cached<HashMap<String, Station>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn service_cache() -> &'static Cached<HashMap<String, Service>> {
    static C: OnceLock<Cached<HashMap<String, Service>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// A cached value for this key while fresh, else fetched with `load` (an old
/// one stands in if that fails).
async fn cached<T>(
    cache: &Cached<T>,
    key: &str,
    ttl: Duration,
    load: impl Future<Output = Result<T, String>>,
) -> Result<Arc<T>, String> {
    let hit = cache.lock().unwrap().clone().filter(|(_, k, _)| k == key);
    if let Some((at, _, value)) = &hit
        && at.elapsed() < ttl
    {
        return Ok(value.clone());
    }
    match load.await {
        Ok(value) => {
            let value = Arc::new(value);
            *cache.lock().unwrap() = Some((Instant::now(), key.to_owned(), value.clone()));
            Ok(value)
        }
        Err(e) => hit.map(|(_, _, v)| v).ok_or(e),
    }
}

async fn stations(http: &reqwest::Client, key: &str) -> Result<Arc<HashMap<String, Station>>, String> {
    cached(station_cache(), key, STATIONS_TTL, async { parse_stations(&post(http, key, &stations_query()).await?) }).await
}

/// Every train in Sweden reporting a position.
pub async fn trains(http: &reqwest::Client, key: &str, now_ms: i64) -> Result<Vec<Train>, String> {
    let (announcements, positions) = (announcements_query(), positions_query());
    let (stations, services, positions) = futures_util::join!(
        stations(http, key),
        cached(service_cache(), key, ANNOUNCEMENTS_TTL, async {
            parse_announcements(&post(http, key, &announcements).await?, now_ms)
        }),
        post(http, key, &positions),
    );
    // names and timetables are niceties: positions alone still make trains
    let stations = stations.unwrap_or_default();
    let services = services.unwrap_or_default();
    parse_positions(&positions?, &services, &stations)
}

/// Every stop of a train, by the id `trains` gave it.
pub async fn stops(http: &reqwest::Client, key: &str, id: &str) -> Result<Vec<Stop>, String> {
    let mut parts = id.strip_prefix("trafikverket:").ok_or("not a Trafikverket train")?.splitn(2, ':');
    let (date, ident) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
    if ident.is_empty() || ident.starts_with('~') || date.len() != 10 {
        // not announced to travellers: there is no timetable to show
        return Ok(Vec::new());
    }
    let query = stops_query(ident, date);
    let (stations, body) = futures_util::join!(stations(http, key), post(http, key, &query));
    parse_stops(&body?, &stations.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATIONS: &str = r#"{"RESPONSE":{"RESULT":[{"TrainStation":[
        {"LocationSignature":"Cst","AdvertisedLocationName":"Stockholm Central","Geometry":{"WGS84":"POINT (18.0582 59.3302)"}},
        {"LocationSignature":"U","AdvertisedLocationName":"Uppsala C","Geometry":{"WGS84":"POINT (17.6467 59.8582)"}},
        {"LocationSignature":"G","AdvertisedLocationName":"Göteborg C"}]}]}}"#;

    const ANNOUNCEMENTS: &str = r#"{"RESPONSE":{"RESULT":[{"TrainAnnouncement":[
        {"AdvertisedTrainIdent":"531","ScheduledDepartureDateTime":"2026-10-14T00:00:00.000+02:00","ActivityType":"Avgang",
         "LocationSignature":"Cst","AdvertisedTimeAtLocation":"2026-10-14T10:00:00.000+02:00","TimeAtLocation":"2026-10-14T10:04:00.000+02:00",
         "ProductInformation":[{"Code":"PNA014","Description":"SJ Snabbtåg"}],"InformationOwner":"SJ","Operator":"SJ",
         "ToLocation":[{"LocationName":"G","Order":0,"Priority":1}],"Canceled":false},
        {"AdvertisedTrainIdent":"531","ScheduledDepartureDateTime":"2026-10-14T00:00:00.000+02:00","ActivityType":"Avgang",
         "LocationSignature":"Sk","AdvertisedTimeAtLocation":"2026-10-14T11:30:00.000+02:00","EstimatedTimeAtLocation":"2026-10-14T11:36:00.000+02:00",
         "ProductInformation":[{"Code":"PNA014","Description":"SJ Snabbtåg"}],"InformationOwner":"SJ",
         "ToLocation":[{"LocationName":"G","Order":0,"Priority":1}],"Canceled":false},
        {"AdvertisedTrainIdent":"2140","ScheduledDepartureDateTime":"2026-10-14T00:00:00.000+02:00","ActivityType":"Avgang",
         "LocationSignature":"Sod","AdvertisedTimeAtLocation":"2026-10-14T10:10:00.000+02:00",
         "ProductInformation":[{"Description":"Pendeltåg"}],"InformationOwner":"SL",
         "ToLocation":[{"LocationName":"U","Order":0}]}]}]}}"#;

    const POSITIONS: &str = r#"{"RESPONSE":{"RESULT":[{"TrainPosition":[
        {"Train":{"AdvertisedTrainNumber":"531","OperationalTrainNumber":"531"},"Position":{"WGS84":"POINT (17.1 59.2)"},
         "Bearing":245,"Speed":187,"TimeStamp":"2026-10-14T10:20:00.000+02:00","Status":{"Active":true}},
        {"Train":{"AdvertisedTrainNumber":"531","OperationalTrainNumber":"531"},"Position":{"WGS84":"POINT (17.0 59.1)"},
         "Bearing":240,"Speed":190,"TimeStamp":"2026-10-14T10:21:00.000+02:00","Status":{"Active":true}},
        {"Train":{"OperationalTrainNumber":"4711","JourneyPlanDepartureDate":"2026-10-14T00:00:00.000+02:00"},
         "Position":{"WGS84":"POINT (13.0 55.6)"},"TimeStamp":"2026-10-14T10:20:00.000+02:00","Status":{"Active":true}},
        {"Train":{"AdvertisedTrainNumber":"9999"},"Position":{"WGS84":"POINT (13.0 55.6)"},
         "TimeStamp":"2026-10-14T10:20:00.000+02:00","Status":{"Active":false}}]}]}}"#;

    #[test]
    fn requests_carry_the_key_escaped() {
        let body = request("a\"b", "<QUERY/>");
        assert!(body.starts_with("<REQUEST><LOGIN authenticationkey=\"a&quot;b\"/>"));
        assert!(stops_query("531", "2026-10-14").contains("value=\"2026-10-14T00:00:00\""));
    }

    #[test]
    fn points_read_longitude_first() {
        assert_eq!(point("POINT (18.0582 59.3302)"), Some((59.3302, 18.0582)));
        assert_eq!(point("LINESTRING (1 2, 3 4)"), None);
    }

    #[test]
    fn trains_are_named_and_described_from_the_announcements() {
        let now = unix_ms("2026-10-14T10:20:00+02:00").unwrap();
        let stations = parse_stations(STATIONS).unwrap();
        let services = parse_announcements(ANNOUNCEMENTS, now).unwrap();
        let mut trains = parse_positions(POSITIONS, &services, &stations).unwrap();
        trains.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(trains.len(), 2, "the inactive one is left out, the repeat merged");

        let sj = &trains[0];
        assert_eq!(sj.id, "trafikverket:2026-10-14:531");
        assert_eq!(sj.name, "SJ Snabbtåg 531");
        assert_eq!(sj.kind, Kind::HighSpeed);
        assert_eq!(sj.operator.as_deref(), Some("SJ"));
        assert_eq!(sj.destination.as_deref(), Some("Göteborg C"));
        assert_eq!(sj.delay_minutes, Some(4), "the last actual time wins over the next estimate");
        assert_eq!((sj.lat, sj.lon, sj.speed_kmh), (59.1, 17.0, Some(190.0)), "the newest report");

        let freight = &trains[1];
        assert_eq!(freight.name, "Train 4711");
        assert_eq!(freight.kind, Kind::Freight);
        assert_eq!(freight.id, "trafikverket:2026-10-14:~4711");
    }

    #[test]
    fn a_train_without_actual_times_takes_its_estimate() {
        let now = unix_ms("2026-10-14T10:20:00+02:00").unwrap();
        let services = parse_announcements(ANNOUNCEMENTS, now).unwrap();
        assert_eq!(services["2140"].owner.as_deref(), Some("SL"));
        assert_eq!(services["2140"].delay_minutes, None);
        assert_eq!(kind(Some("Pendeltåg"), Some("SL")), Kind::Suburban);
    }

    #[test]
    fn stops_merge_arrival_and_departure() {
        let body = r#"{"RESPONSE":{"RESULT":[{"TrainAnnouncement":[
            {"ActivityType":"Avgang","LocationSignature":"Cst","AdvertisedTimeAtLocation":"2026-10-14T10:00:00Z","TimeAtLocation":"2026-10-14T10:02:00Z","TrackAtLocation":"14"},
            {"ActivityType":"Ankomst","LocationSignature":"U","AdvertisedTimeAtLocation":"2026-10-14T10:40:00Z","EstimatedTimeAtLocation":"2026-10-14T10:43:00Z","TrackAtLocation":"3",
             "Deviation":[{"Code":"ANA088","Description":"Signalfel"}]},
            {"ActivityType":"Avgang","LocationSignature":"U","AdvertisedTimeAtLocation":"2026-10-14T10:42:00Z","Deviation":[{"Description":"Signalfel"}]},
            {"ActivityType":"Ankomst","LocationSignature":"G","AdvertisedTimeAtLocation":"2026-10-14T13:00:00Z","Canceled":true}]}]}}"#;
        let stops = parse_stops(body, &parse_stations(STATIONS).unwrap()).unwrap();
        assert_eq!(stops.len(), 3);
        assert_eq!(stops[0].name, "Stockholm Central");
        assert!(stops[0].arrival.is_none());
        assert_eq!(stops[0].departure.as_ref().unwrap().actual, Some(unix_ms("2026-10-14T10:02:00Z").unwrap() / 1000));
        assert_eq!(stops[1].track.as_deref(), Some("3"));
        assert!(stops[1].arrival.is_some() && stops[1].departure.is_some());
        assert_eq!(stops[1].notes, vec!["Signalfel".to_owned()], "repeated notes once");
        assert!(stops[2].canceled && stops[2].departure.is_none());
        assert_eq!(stops[2].lat, None, "a station without a place");
    }

    #[test]
    fn errors_from_the_api_are_reported() {
        let body = r#"{"RESPONSE":{"RESULT":[{"ERROR":{"SOURCE":"Request","MESSAGE":"Invalid authentication"}}]}}"#;
        assert_eq!(results(body).unwrap_err(), "Invalid authentication");
    }
}
