//! Sweden's trains from Trafikverket's open API (api.trafikinfo.trafikverket.se),
//! with the key from the keys page. Three object types make a train:
//!
//! - `TrainPosition`: where each train is, from its GPS, with bearing and
//!   speed. An ordinary key sees the passenger trains.
//! - `TrainAnnouncement`: the timetable as announced to travellers, one row per
//!   arrival or departure at a station: the product ("SJ Snabbtåg"), who runs
//!   it, where it goes, and planned, estimated and actual times.
//! - `TrainStation`: station names and places for the stations' signatures
//!   ("Cst" is Stockholm Central).
//!
//! Positions are asked for on every refresh. Who runs a train, as what and
//! to where doesn't change during its run, so that is asked once per train
//! on the map (by number, a batch at a time) and kept; the delays come from
//! a small query of the departures around now.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::motion;
use crate::trains::{Kind, Train, unix_ms};

const URL: &str = "https://api.trafikinfo.trafikverket.se/v2/data.json";
/// How long the delays around now are reused before asking again.
const DELAYS_TTL: Duration = Duration::from_secs(60);
/// A train's run is kept this long once found; a number with none found is
/// asked again after `UNKNOWN_TTL`.
const KNOWN_TTL: Duration = Duration::from_secs(6 * 3600);
const UNKNOWN_TTL: Duration = Duration::from_secs(15 * 60);
/// Numbers asked about in one query.
const SERVICE_BATCH: usize = 80;
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

/// What runs as each of these numbers (a batch of them): every call since
/// yesterday, with the fields that don't change along the way.
fn services_query(numbers: &[String]) -> String {
    // a position names its train by its operational number (and its advertised
    // one when it has one): either finds the announcements
    let any: String = numbers
        .iter()
        .map(|n| {
            let n = xml_escape(n);
            format!(
                "<EQ name=\"OperationalTrainNumber\" value=\"{n}\"/><EQ name=\"AdvertisedTrainIdent\" value=\"{n}\"/>"
            )
        })
        .collect();
    format!(
        "<QUERY objecttype=\"TrainAnnouncement\" schemaversion=\"1.9\" orderby=\"AdvertisedTimeAtLocation\">\
         <FILTER><AND><OR>{any}</OR>\
         <GT name=\"ScheduledDepartureDateTime\" value=\"$dateadd(-1.12:00:00)\"/></AND></FILTER>{}</QUERY>",
        includes(&[
            "AdvertisedTrainIdent",
            "OperationalTrainNumber",
            "ScheduledDepartureDateTime",
            "ActivityType",
            "Advertised",
            "LocationSignature",
            "AdvertisedTimeAtLocation",
            "TrackAtLocation",
            "ProductInformation",
            "InformationOwner",
            "Operator",
            "ToLocation",
        ])
    )
}

/// Departures in the half hour either side of now, for how late each train runs.
fn delays_query() -> String {
    format!(
        "<QUERY objecttype=\"TrainAnnouncement\" schemaversion=\"1.9\">\
         <FILTER><AND><EQ name=\"ActivityType\" value=\"Avgang\"/>\
         <GT name=\"AdvertisedTimeAtLocation\" value=\"$dateadd(-0.00:30:00)\"/>\
         <LT name=\"AdvertisedTimeAtLocation\" value=\"$dateadd(0.00:30:00)\"/></AND></FILTER>{}</QUERY>",
        includes(&[
            "AdvertisedTrainIdent",
            "OperationalTrainNumber",
            "ScheduledDepartureDateTime",
            "AdvertisedTimeAtLocation",
            "EstimatedTimeAtLocation",
            "TimeAtLocation",
        ])
    )
}

/// Every stop of one train on one day.
fn stops_query(ident: &str, date: &str) -> String {
    let more: Vec<&str> = ANNOUNCEMENT_FIELDS
        .iter()
        .copied()
        .chain(["Advertised", "TrackAtLocation", "Deviation", "OtherInformation"])
        .collect();
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
    list.iter().max_by_key(|x| x["Order"].as_i64().unwrap_or(0)).and_then(|x| text(&x["LocationName"]))
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
        let Some(sig) = text(&row["LocationSignature"]) else {
            continue;
        };
        let name = text(&row["AdvertisedLocationName"])
            .or_else(|| text(&row["AdvertisedShortLocationName"]))
            .unwrap_or_else(|| sig.clone());
        let at = row["Geometry"]["WGS84"].as_str().and_then(point);
        stations.insert(sig, Station { name, lat: at.map(|p| p.0), lon: at.map(|p| p.1) });
    }
    Ok(stations)
}

// ----------------------------------------------------------- announcements

/// Who runs a train, as what, and where to: the same for its whole run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Service {
    /// The advertised number, the one travellers see (and the stops are looked up by).
    pub ident: String,
    pub product: Option<String>,
    pub owner: Option<String>,
    /// Destination's signature.
    pub to: Option<String>,
    /// The day of its first departure, `YYYY-MM-DD`: with the number, which run it is.
    pub date: String,
    /// Where it stops, in order, with the timetable's times.
    pub calls: Vec<Call>,
}

/// A stop in the timetable: the station and the planned times, Unix ms.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Call {
    pub station: String,
    pub arrive: Option<i64>,
    pub depart: Option<i64>,
    /// A stop for travellers; else a place it only passes (still on its way).
    pub stops: bool,
    /// The track it is announced at ("4b"), for the platform.
    pub track: Option<String>,
}

fn day(row: &Value) -> String {
    row["ScheduledDepartureDateTime"].as_str().map(|s| s.chars().take(10).collect()).unwrap_or_default()
}

/// A row's numbers: its advertised one (else its operational one) to group
/// it by, and its operational one when that differs.
fn numbers(row: &Value) -> Option<(String, Option<String>)> {
    let ident = text(&row["AdvertisedTrainIdent"]);
    let operational = text(&row["OperationalTrainNumber"]);
    let primary = ident.or(operational.clone())?;
    let alias = operational.filter(|o| *o != primary);
    Some((primary, alias))
}

/// Makes what is known under each number also known under its other number.
fn with_aliases<T: Clone>(mut map: HashMap<String, T>, aliases: HashMap<String, String>) -> HashMap<String, T> {
    for (alias, primary) in aliases {
        if !map.contains_key(&alias)
            && let Some(v) = map.get(&primary).cloned()
        {
            map.insert(alias, v);
        }
    }
    map
}

/// The runs in the answer (one per day it runs), under both the advertised
/// and the operational number.
pub fn parse_services(body: &str) -> Result<HashMap<String, Vec<Service>>, String> {
    let results = results(body)?;
    let mut services: HashMap<String, Vec<Service>> = HashMap::new();
    let mut aliases: HashMap<String, String> = HashMap::new();
    for row in results.iter().flat_map(|r| rows(r, "TrainAnnouncement")) {
        let Some((ident, alias)) = numbers(row) else {
            continue;
        };
        if let Some(alias) = alias {
            aliases.insert(alias, ident.clone());
        }
        let date = day(row);
        let runs = services.entry(ident.clone()).or_default();
        let at = match runs.iter().position(|s| s.date == date) {
            Some(at) => at,
            None => {
                runs.push(Service {
                    ident,
                    product: description(&row["ProductInformation"]),
                    owner: text(&row["InformationOwner"]).or_else(|| text(&row["Operator"])),
                    to: last_location(&row["ToLocation"]),
                    date,
                    calls: Vec::new(),
                });
                runs.len() - 1
            }
        };
        // the rows of places it only passes carry no product, owner or destination
        let run = &mut runs[at];
        run.product = run.product.take().or_else(|| description(&row["ProductInformation"]));
        run.owner = run.owner.take().or_else(|| text(&row["InformationOwner"])).or_else(|| text(&row["Operator"]));
        run.to = run.to.take().or_else(|| last_location(&row["ToLocation"]));
        let (Some(station), Some(time)) =
            (text(&row["LocationSignature"]), row["AdvertisedTimeAtLocation"].as_str().and_then(unix_ms))
        else {
            continue;
        };
        let departure = row["ActivityType"].as_str() == Some("Avgang");
        let calls = &mut runs[at].calls;
        // an arrival and a departure at the same station are one call, in either order
        let merge = calls.last().is_some_and(|c: &Call| {
            c.station == station && if departure { c.depart.is_none() } else { c.arrive.is_none() }
        });
        if !merge {
            calls.push(Call { station, ..Default::default() });
        }
        let call = calls.last_mut().expect("just pushed");
        if departure {
            call.depart = Some(time);
        } else {
            call.arrive = Some(time);
        }
        call.stops |= row["Advertised"].as_bool() != Some(false);
        call.track = call.track.take().or_else(|| text(&row["TrackAtLocation"]));
    }
    for run in services.values_mut().flatten() {
        run.calls.sort_by_key(|c| c.arrive.or(c.depart).unwrap_or(0));
    }
    Ok(with_aliases(services, aliases))
}

/// The run a position belongs to: the day the position names, else the latest.
fn run_of<'a>(runs: &'a [Service], date: Option<&str>) -> Option<&'a Service> {
    date.and_then(|d| runs.iter().find(|s| s.date == d)).or_else(|| runs.iter().max_by(|a, b| a.date.cmp(&b.date)))
}

fn minutes_late(planned: &Value, at: &Value) -> Option<i32> {
    let planned = unix_ms(planned.as_str()?)?;
    let at = unix_ms(at.as_str()?)?;
    Some(((at - planned) as f64 / 60_000.0).round() as i32)
}

/// How late each train runs, by number: from its latest actual time, else its
/// next estimate. A number running on two days takes the run nearest now.
pub fn parse_delays(body: &str, now_ms: i64) -> Result<HashMap<String, i32>, String> {
    let results = results(body)?;
    // (number, day) -> (rows in time order, distance of its nearest row from now)
    let mut runs: HashMap<(String, String), (Vec<&Value>, i64)> = HashMap::new();
    let mut aliases: HashMap<String, String> = HashMap::new();
    for row in results.iter().flat_map(|r| rows(r, "TrainAnnouncement")) {
        let Some((ident, alias)) = numbers(row) else {
            continue;
        };
        if let Some(alias) = alias {
            aliases.insert(alias, ident.clone());
        }
        let gap = row["AdvertisedTimeAtLocation"].as_str().and_then(unix_ms).map_or(i64::MAX, |t| (t - now_ms).abs());
        let run = runs.entry((ident, day(row))).or_insert_with(|| (Vec::new(), i64::MAX));
        run.0.push(row);
        run.1 = run.1.min(gap);
    }
    let mut best: HashMap<String, (i64, i32)> = HashMap::new();
    for ((ident, _), (mut rows, gap)) in runs {
        rows.sort_by_key(|r| r["AdvertisedTimeAtLocation"].as_str().and_then(unix_ms).unwrap_or(0));
        let actual = rows.iter().rev().find_map(|r| minutes_late(&r["AdvertisedTimeAtLocation"], &r["TimeAtLocation"]));
        let estimate =
            || rows.iter().find_map(|r| minutes_late(&r["AdvertisedTimeAtLocation"], &r["EstimatedTimeAtLocation"]));
        let Some(delay) = actual.or_else(estimate) else {
            continue;
        };
        if best.get(&ident).is_none_or(|(g, _)| gap < *g) {
            best.insert(ident, (gap, delay));
        }
    }
    Ok(with_aliases(best.into_iter().map(|(k, (_, d))| (k, d)).collect(), aliases))
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
        // not (yet) found in the announcements
        Kind::Other
    } else {
        Kind::Regional
    }
}

// --------------------------------------------------------------- positions

/// A position report, before it is matched with its timetable.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// The number travellers see, when the report gives it.
    pub advertised: Option<String>,
    /// The number the train runs under on the network.
    pub operational: Option<String>,
    /// `YYYY-MM-DD`, when the report names its run's day.
    pub date: Option<String>,
    pub lat: f64,
    pub lon: f64,
    pub bearing: Option<f32>,
    pub speed: Option<f32>,
    pub at_ms: i64,
}

impl Report {
    /// The number its timetable is looked up by: the operational one, which
    /// every report has, else the advertised one.
    pub fn lookup(&self) -> &str {
        self.operational.as_deref().or(self.advertised.as_deref()).unwrap_or_default()
    }
}

/// The newest report of each active train.
pub fn parse_positions(body: &str) -> Result<Vec<Report>, String> {
    let results = results(body)?;
    let mut newest: HashMap<String, Report> = HashMap::new();
    for row in results.iter().flat_map(|r| rows(r, "TrainPosition")) {
        if row["Status"]["Active"].as_bool() == Some(false) {
            continue;
        }
        let Some((lat, lon)) = row["Position"]["WGS84"].as_str().and_then(point) else {
            continue;
        };
        let Some(at_ms) = row["TimeStamp"].as_str().and_then(unix_ms) else {
            continue;
        };
        let advertised = text(&row["Train"]["AdvertisedTrainNumber"]);
        let operational = text(&row["Train"]["OperationalTrainNumber"]);
        if advertised.is_none() && operational.is_none() {
            continue;
        }
        let date = ["JourneyPlanDepartureDate", "OperationalTrainDepartureDate"]
            .iter()
            .find_map(|f| row["Train"][*f].as_str())
            .map(|s| s.chars().take(10).collect());
        let report = Report {
            advertised,
            operational,
            date,
            lat,
            lon,
            bearing: row["Bearing"].as_f64().map(|b| b as f32),
            speed: row["Speed"].as_f64().map(|s| s as f32),
            at_ms,
        };
        let key = report.lookup().to_owned();
        if newest.get(&key).is_none_or(|old| old.at_ms < report.at_ms) {
            newest.insert(key, report);
        }
    }
    Ok(newest.into_values().collect())
}

/// A train from its report, named and described from its timetable when known.
pub fn train(
    report: &Report,
    services: &HashMap<String, Vec<Service>>,
    delays: &HashMap<String, i32>,
    stations: &HashMap<String, Station>,
    now_ms: i64,
) -> Train {
    let runs = services.get(report.lookup()).or_else(|| services.get(report.advertised.as_deref()?));
    let service = runs.and_then(|r| run_of(r, report.date.as_deref()));
    let product = service.and_then(|s| s.product.clone());
    let owner = service.and_then(|s| s.owner.clone());
    let date = service.map(|s| s.date.clone()).or_else(|| report.date.clone()).unwrap_or_default();
    // the number travellers know it by: the timetable's, else the report's
    let advertised = service.map(|s| s.ident.clone()).or_else(|| report.advertised.clone());
    let number = advertised.clone().unwrap_or_else(|| report.lookup().to_owned());
    let delay = delays.get(report.lookup()).or_else(|| delays.get(advertised.as_deref()?)).copied();
    let stops = service.map(|s| stops_ahead(s, delay, stations, report.at_ms)).unwrap_or_default();
    let frames = motion::frames_with(
        report.lat,
        report.lon,
        report.at_ms,
        report.speed.map(|s| f64::from(s) / 3.6),
        report.bearing.map(f64::from),
        &stops,
        now_ms,
        Some(&crate::rail::Rail),
    );
    let place = frames
        .first()
        .map(|f| (f.0, f.1))
        .or_else(|| crate::rail::snap((report.lat, report.lon)))
        .unwrap_or((report.lat, report.lon));
    Train {
        // the advertised number and its day are what the stops are looked up by
        id: format!("trafikverket:{date}:{}{number}", if advertised.is_some() { "" } else { "~" }),
        source: "trafikverket",
        name: match &product {
            Some(p) => format!("{p} {number}"),
            None => format!("Train {number}"),
        },
        kind: kind(product.as_deref(), owner.as_deref()),
        // on the track: where its way starts, else the nearest track
        lat: place.0,
        lon: place.1,
        heading: report.bearing,
        speed_kmh: report.speed,
        destination: service
            .and_then(|s| s.to.as_ref())
            .map(|sig| stations.get(sig).map_or_else(|| sig.clone(), |st| st.name.clone())),
        operator: owner,
        delay_minutes: delay,
        position_unix: report.at_ms / 1000,
        frames,
    }
}

/// The run's stops still ahead at `at_ms`, with places, at the times now
/// expected: the timetable's shifted by the delay (never leaving early).
fn stops_ahead(
    run: &Service,
    delay: Option<i32>,
    stations: &HashMap<String, Station>,
    at_ms: i64,
) -> Vec<motion::Stop> {
    calls_ahead(run, delay, stations, at_ms).into_iter().map(|(_, _, stop)| stop).collect()
}

/// As `stops_ahead`, with each stop's call and station.
fn calls_ahead<'a>(
    run: &'a Service,
    delay: Option<i32>,
    stations: &'a HashMap<String, Station>,
    at_ms: i64,
) -> Vec<(&'a Call, &'a Station, motion::Stop)> {
    let late = i64::from(delay.unwrap_or(0)) * 60_000;
    run.calls
        .iter()
        .filter_map(|c| {
            let station = stations.get(&c.station)?;
            // at the platform it is announced at, where the track is known
            let place = (station.lat?, station.lon?);
            let (lat, lon) = crate::rail::platform(place, c.track.as_deref()).unwrap_or(place);
            let stop = motion::Stop {
                lat,
                lon,
                arrive_ms: c.arrive.map(|t| t + late),
                depart_ms: c.depart.map(|t| t + late.max(0)),
                pass: !c.stops,
            };
            Some((c, station, stop))
        })
        // a couple of minutes' grace: the delay is the one at its last report
        .skip_while(|(_, _, s)| s.depart_ms.or(s.arrive_ms).is_none_or(|t| t < at_ms - 120_000))
        .collect()
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
        let Some(sig) = text(&row["LocationSignature"]) else {
            continue;
        };
        // places it only passes (Advertised: false) are no stops for travellers
        if row["Advertised"].as_bool() == Some(false) {
            continue;
        }
        let departure = row["ActivityType"].as_str() == Some("Avgang");
        // an arrival and a departure at the same station make one stop, in
        // either order (they often share a minute); a third row starts another
        let index = match stops.last() {
            Some((last, stop))
                if *last == sig && if departure { stop.departure.is_none() } else { stop.arrival.is_none() } =>
            {
                stops.len() - 1
            }
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
    let sent = request(key, queries);
    let call = crate::devlog::start("Trafikverket", "POST", URL, Some(&sent));
    let response = match http.post(URL).header("content-type", "text/xml; charset=utf-8").body(sent).send().await {
        Ok(r) => r,
        Err(e) => {
            let e = format!("can't reach Trafikverket: {e}");
            if let Some(c) = call {
                c.fail(&e);
            }
            return Err(e);
        }
    };
    let status = response.status();
    let body = response.text().await.map_err(|e| format!("Trafikverket: {e}"))?;
    if let Some(c) = call {
        c.finish(status.as_u16(), body.as_bytes());
    }
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

fn delay_cache() -> &'static Cached<HashMap<String, i32>> {
    static C: OnceLock<Cached<HashMap<String, i32>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Runs per number, kept across refreshes, with when they were asked for (a
/// number with no runs is asked again after `UNKNOWN_TTL`).
type Services = HashMap<String, (Instant, Vec<Service>)>;

fn service_cache() -> &'static Mutex<(String, Services)> {
    static C: OnceLock<Mutex<(String, Services)>> = OnceLock::new();
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
    cached(station_cache(), key, STATIONS_TTL, async { parse_stations(&post(http, key, &stations_query()).await?) })
        .await
}

/// Asks for the runs of the numbers it doesn't know yet, a batch per query.
async fn learn_services(http: &reqwest::Client, key: &str, numbers: Vec<String>) -> HashMap<String, Vec<Service>> {
    let missing: Vec<String> = {
        let mut cache = service_cache().lock().unwrap();
        if cache.0 != key {
            *cache = (key.to_owned(), HashMap::new());
        }
        cache.1.retain(|_, (at, runs)| at.elapsed() < if runs.is_empty() { UNKNOWN_TTL } else { KNOWN_TTL });
        let mut missing: Vec<String> = numbers.into_iter().filter(|n| !cache.1.contains_key(n)).collect();
        missing.sort();
        missing.dedup();
        missing
    };
    let queries: Vec<String> = missing.chunks(SERVICE_BATCH).map(services_query).collect();
    let answers = futures_util::future::join_all(
        queries.iter().map(|q| async move { parse_services(&post(http, key, q).await?) }),
    )
    .await;
    let mut cache = service_cache().lock().unwrap();
    for (batch, answer) in missing.chunks(SERVICE_BATCH).zip(answers) {
        match answer {
            Ok(mut found) => {
                for number in batch {
                    cache.1.insert(number.clone(), (Instant::now(), found.remove(number).unwrap_or_default()));
                }
            }
            // asked again on the next refresh
            Err(e) => eprintln!("trafikverket: train details: {e}"),
        }
    }
    cache.1.iter().map(|(k, (_, runs))| (k.clone(), runs.clone())).collect()
}

/// Every passenger train in Sweden reporting a position.
pub async fn trains(http: &reqwest::Client, key: &str, now_ms: i64) -> Result<Vec<Train>, String> {
    let (positions, delays) = (positions_query(), delays_query());
    let (stations, delays, positions) = futures_util::future::join3(
        stations(http, key),
        cached(delay_cache(), key, DELAYS_TTL, async { parse_delays(&post(http, key, &delays).await?, now_ms) }),
        post(http, key, &positions),
    )
    .await;
    let reports = parse_positions(&positions?)?;
    let numbers = reports.iter().map(|r| r.lookup().to_owned()).filter(|n| !n.is_empty()).collect();
    let services = learn_services(http, key, numbers).await;
    // names, places and delays are niceties: positions alone still make trains
    let stations = stations.unwrap_or_else(|e| {
        eprintln!("trafikverket: stations: {e}");
        Default::default()
    });
    let delays = delays.unwrap_or_else(|e| {
        eprintln!("trafikverket: delays: {e}");
        Default::default()
    });
    let trains: Vec<Train> = reports.iter().map(|r| train(r, &services, &delays, &stations, now_ms)).collect();
    // which operational number each train runs under, for following one
    let mut numbers = operational_numbers().lock().unwrap();
    numbers.clear();
    numbers.extend(trains.iter().zip(&reports).map(|(t, r)| (t.id.clone(), r.lookup().to_owned())));
    drop(numbers);
    let mut places = last_places().lock().unwrap();
    for (t, r) in trains.iter().zip(&reports) {
        if places.get(&t.id).is_none_or(|p| p.at_ms <= r.at_ms) {
            places.insert(t.id.clone(), Seen::of(r));
        }
    }
    places.retain(|_, p| now_ms - p.at_ms < 6 * 3_600_000);
    drop(places);
    Ok(trains)
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
    let (stations, body) = futures_util::future::join(stations(http, key), post(http, key, &query)).await;
    parse_stops(&body?, &stations.unwrap_or_default())
}

// --------------------------------------------------------------- following

/// What the panel hears about the train it shows, as it happens.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TrainEvent {
    /// A new position (and with it the way ahead).
    Train { train: Box<Train> },
    /// The stops with their times as they now stand.
    Stops { stops: Vec<Stop> },
}

/// The operational number of each train on the map, by its id.
fn operational_numbers() -> &'static Mutex<HashMap<String, String>> {
    static N: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    N.get_or_init(Default::default)
}

/// A report older than this when it arrives is a replay, not news.
const STALE_REPORT_MS: i64 = 15 * 60_000;
/// Waiting before opening a stream again after it closed or failed.
const RECONNECT: Duration = Duration::from_secs(3);
/// A stream quiet this long (a train standing for a long time sends nothing) is opened again.
const STREAM_IDLE: Duration = Duration::from_secs(10 * 60);

/// The client streams are read with: no limit on the whole request (a stream
/// stays open for as long as the train is followed, and a client's overall
/// timeout would cut it off mid-body, "error decoding response body"), only
/// on connecting and on silence.
fn stream_client() -> Result<&'static reqwest::Client, String> {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = C.get() {
        return Ok(c);
    }
    let c = fr24::client_builder()
        .user_agent(concat!("RustRadar/", env!("CARGO_PKG_VERSION"), " (+https://github.com/felixgoff/RustRadar)"))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(STREAM_IDLE)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(C.get_or_init(|| c))
}

/// The URL of a stream of changes to what one query asks for.
async fn sse_url(http: &reqwest::Client, key: &str, query: &str) -> Result<String, String> {
    let body = post(http, key, query).await?;
    results(&body)?
        .iter()
        .find_map(|r| r["INFO"]["SSEURL"].as_str().map(str::to_owned))
        .ok_or_else(|| "Trafikverket gave no stream".to_owned())
}

/// The data of each event in a chunk of an event stream; the rest (a part
/// event) stays in `buf` for the next chunk.
pub fn take_events(buf: &mut String) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(end) = buf.find("\n\n") {
        let event: String = buf.drain(..end + 2).collect();
        let data: Vec<&str> =
            event.lines().filter_map(|l| l.strip_prefix("data:")).map(|d| d.strip_prefix(' ').unwrap_or(d)).collect();
        if !data.is_empty() {
            out.push(data.join("\n"));
        }
    }
    out
}

/// Reads a stream until it ends, sending each event's data on.
async fn read_stream(
    url: &str,
    kind: u8,
    tx: &tokio::sync::mpsc::UnboundedSender<(u8, String)>,
) -> Result<(), String> {
    use futures_util::StreamExt;
    let response =
        stream_client()?.get(url).header("accept", "text/event-stream").send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("stream answered {}", response.status()));
    }
    let mut stream = response.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        buf.push_str(&String::from_utf8_lossy(&chunk.map_err(|e| e.to_string())?).replace("\r\n", "\n"));
        for data in take_events(&mut buf) {
            crate::devlog::received(if kind == 0 { "Trafikverket stream: positions" } else { "Trafikverket stream: announcements" }, url, &data);
            if tx.send((kind, data)).is_err() {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// How late the train runs by its stops: the latest actual time, else the next estimate.
fn delay_of(stops: &[Stop]) -> Option<i32> {
    let times = || stops.iter().flat_map(|s| [s.arrival.as_ref(), s.departure.as_ref()]).flatten();
    let late = |t: &StopTime, at: Option<i64>| Some(((at? - t.planned?) as f64 / 60.0).round() as i32);
    let actual: Vec<i32> = times().filter_map(|t| late(t, t.actual)).collect();
    actual.last().copied().or_else(|| times().find_map(|t| late(t, t.estimated)))
}

/// Follows one train until dropped: its position from Trafikverket's stream
/// of its position reports, its stops again whenever its announcements change.
pub async fn follow(http: reqwest::Client, key: String, id: String, channel: tauri::ipc::Channel<TrainEvent>) {
    let Some(rest) = id.strip_prefix("trafikverket:") else { return };
    let (date, ident) = rest.split_once(':').unwrap_or_default();
    let ident = ident.trim_start_matches('~').to_owned();
    let number = operational_numbers().lock().unwrap().get(&id).cloned().unwrap_or_else(|| ident.clone());
    let positions = format!(
        "<QUERY sseurl=\"true\" objecttype=\"TrainPosition\" namespace=\"järnväg.trafikinfo\" schemaversion=\"1.1\" limit=\"0\">\
         <FILTER><EQ name=\"Train.OperationalTrainNumber\" value=\"{}\"/></FILTER></QUERY>",
        xml_escape(&number)
    );
    let announcements = format!(
        "<QUERY sseurl=\"true\" objecttype=\"TrainAnnouncement\" schemaversion=\"1.9\" limit=\"0\">\
         <FILTER><AND><EQ name=\"OperationalTrainNumber\" value=\"{}\"/>\
         <GTE name=\"ScheduledDepartureDateTime\" value=\"{date}T00:00:00\"/>\
         <LT name=\"ScheduledDepartureDateTime\" value=\"{date}T23:59:59\"/></AND></FILTER>\
         <INCLUDE>ActivityId</INCLUDE></QUERY>",
        xml_escape(&number)
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u8, String)>();
    // each stream, opened again whenever it closes
    let stream = |kind: u8, query: String| {
        let (http, key, tx) = (http.clone(), key.clone(), tx.clone());
        async move {
            loop {
                match sse_url(&http, &key, &query).await {
                    Ok(url) => {
                        if let Err(e) = read_stream(&url, kind, &tx).await {
                            eprintln!("trafikverket: train stream: {e}");
                        }
                    }
                    Err(e) => eprintln!("trafikverket: train stream: {e}"),
                }
                if tx.is_closed() {
                    return;
                }
                tokio::time::sleep(RECONNECT).await;
            }
        }
    };
    let streams = futures_util::future::join(stream(0, positions), stream(1, announcements));
    let handle = async {
        let mut delay = None;
        let mut last = 0;
        delay = push_stops(&http, &key, &id, &channel).await.or(delay);
        while let Some((kind, data)) = rx.recv().await {
            if kind == 1 {
                // the timetable changed: the stops as they now stand
                while let Ok((1, _)) = rx.try_recv() {}
                delay = push_stops(&http, &key, &id, &channel).await.or(delay);
                continue;
            }
            let now = crate::trains::now_ms();
            let Ok(reports) = parse_positions(&data) else { continue };
            let Some(report) = reports.into_iter().max_by_key(|r| r.at_ms) else { continue };
            if report.at_ms <= last || now - report.at_ms > STALE_REPORT_MS {
                continue;
            }
            last = report.at_ms;
            let services = known_services(&key);
            let stations = station_cache().lock().unwrap().clone().map(|(_, _, s)| s).unwrap_or_default();
            let mut delays: HashMap<String, i32> = HashMap::new();
            if let Some(d) = delay {
                delays.insert(report.lookup().to_owned(), d);
            }
            let train = train(&report, &services, &delays, &stations, now);
            last_places().lock().unwrap().insert(train.id.clone(), Seen::of(&report));
            if channel.send(TrainEvent::Train { train: Box::new(train) }).is_err() {
                return;
            }
        }
    };
    // the streams run until the handler stops (the panel closed, or another train followed)
    futures_util::future::select(Box::pin(streams), Box::pin(handle)).await;
}

/// Sends the train's stops as they stand; how late it runs by them.
async fn push_stops(
    http: &reqwest::Client,
    key: &str,
    id: &str,
    channel: &tauri::ipc::Channel<TrainEvent>,
) -> Option<i32> {
    let stops = stops(http, key, id).await.ok()?;
    let delay = delay_of(&stops);
    let _ = channel.send(TrainEvent::Stops { stops });
    delay
}

// ------------------------------------------------------------------- route

/// A train's latest report, as its motion was worked out from.
#[derive(Clone, Copy, Debug)]
struct Seen {
    lat: f64,
    lon: f64,
    at_ms: i64,
    /// m/s.
    speed: Option<f64>,
    heading: Option<f64>,
}

impl Seen {
    fn of(r: &Report) -> Seen {
        Seen {
            lat: r.lat,
            lon: r.lon,
            at_ms: r.at_ms,
            speed: r.speed.map(|s| f64::from(s) / 3.6),
            heading: r.bearing.map(f64::from),
        }
    }
}

type Places = Mutex<HashMap<String, Seen>>;

/// Where each train was last reported.
fn last_places() -> &'static Places {
    static P: OnceLock<Places> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// A place on a train's way ahead: a station it stops at, or one it passes.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteStop {
    pub name: String,
    /// At its platform where the track is known, else the station's point.
    pub lat: f64,
    pub lon: f64,
    pub track: Option<String>,
    pub stops: bool,
}

/// The way a train will take along the track, from where it is to its last
/// station, through the platforms it is announced at.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainRoute {
    /// Stretches of track, each `[lon, lat]` points; the first starts at the
    /// train. Where the track between two places isn't known (not loaded yet,
    /// or not found) there is a gap, never a guess across country.
    pub segments: Vec<Vec<[f64; 2]>>,
    /// The places still ahead.
    pub stops: Vec<RouteStop>,
    /// False while some of the track is still loading: ask again shortly.
    pub complete: bool,
}

/// A train's way ahead, by the id `trains` gave it.
pub fn route(id: &str, key: &str) -> Result<TrainRoute, String> {
    let rest = id.strip_prefix("trafikverket:").ok_or("not a Trafikverket train")?;
    let (date, ident) = rest.split_once(':').ok_or("not a Trafikverket train")?;
    let services = known_services(key);
    let run = services
        .values()
        .flatten()
        .find(|s| s.ident == ident && s.date == date)
        .ok_or("no timetable for this train yet")?
        .clone();
    let stations = station_cache().lock().unwrap().clone().map(|(_, _, s)| s).unwrap_or_default();
    let delay = delay_cache().lock().unwrap().clone().and_then(|(_, _, d)| d.get(ident).copied());
    let seen = last_places().lock().unwrap().get(id).copied();
    let at_ms = seen.map_or_else(crate::trains::now_ms, |s| s.at_ms);
    let calls = calls_ahead(&run, delay, &stations, at_ms);
    let stops: Vec<motion::Stop> = calls.iter().map(|c| c.2).collect();

    // where the train is and the way it goes from there: as its motion has it
    let (first, start) = match seen {
        Some(s) => match motion::plan((s.lat, s.lon), s.speed, s.heading, &stops, s.at_ms) {
            motion::Plan::Standing => (0, None),
            motion::Plan::Moving { next } => (next, Some(s)),
        },
        None => (0, None),
    };
    let ahead = &stops[first.min(stops.len())..];
    let ahead_calls = &calls[first.min(calls.len())..];
    let places: Vec<(f64, f64)> = ahead.iter().map(|s| (s.lat, s.lon)).collect();
    // each place's possible spots: the one the train's motion heads for first
    let spots: Vec<Vec<(f64, f64)>> = ahead_calls
        .iter()
        .map(|(call, station, stop)| {
            let mut spots = vec![(stop.lat, stop.lon)];
            if let (Some(lat), Some(lon)) = (station.lat, station.lon)
                && let Some(more) = crate::rail::platforms((lat, lon), call.track.as_deref())
            {
                spots.extend(more.into_iter().filter(|p| motion::distance(*p, (stop.lat, stop.lon)) >= 3.0));
            }
            spots
        })
        .collect();
    let (segments, complete) = match start {
        Some(s) => {
            let n = motion::route_targets(ahead);
            match crate::rail::route((s.lat, s.lon), s.heading, &places[..n], 0.0) {
                // the stretch the train is moving along, then on from the last place it reached
                Some(r) => {
                    let k = r.reached.len();
                    let mut rest_places = spots[k.saturating_sub(1)..].to_vec();
                    if k > 0
                        && let Some(reached) = rest_places.first_mut()
                    {
                        // where its motion stops there
                        *reached = vec![places[k - 1]];
                    }
                    let (mut rest, complete) = crate::rail::way_through(&rest_places);
                    match rest.first_mut() {
                        Some(next)
                            if k > 0
                                && next.first().zip(r.points.last()).is_some_and(|(a, b)| motion::distance(*a, *b) < 1.0) =>
                        {
                            let mut joined = r.points;
                            joined.extend(next.drain(1..));
                            rest[0] = joined;
                        }
                        _ => rest.insert(0, r.points),
                    }
                    (rest, complete)
                }
                None => (Vec::new(), false),
            }
        }
        None => crate::rail::way_through(&spots),
    };
    Ok(TrainRoute {
        segments: segments
            .into_iter()
            .filter(|s| s.len() >= 2)
            .map(|s| s.into_iter().map(|p| [p.1, p.0]).collect())
            .collect(),
        stops: calls[first.min(calls.len())..]
            .iter()
            .map(|(call, station, stop)| RouteStop {
                name: station.name.clone(),
                lat: stop.lat,
                lon: stop.lon,
                track: call.track.clone(),
                stops: call.stops,
            })
            .collect(),
        complete,
    })
}

/// The runs known so far, by number.
fn known_services(key: &str) -> HashMap<String, Vec<Service>> {
    let cache = service_cache().lock().unwrap();
    if cache.0 != key {
        return HashMap::new();
    }
    cache.1.iter().map(|(k, (_, runs))| (k.clone(), runs.clone())).collect()
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
        {"Train":{"AdvertisedTrainNumber":"1322"},"Position":{"WGS84":"POINT (13.1 55.66)"},"Bearing":44,
         "TimeStamp":"2026-10-14T10:20:00.000+02:00","Status":{"Active":true}},
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
    fn trains_are_named_and_described_from_their_runs() {
        let now = unix_ms("2026-10-14T10:20:00+02:00").unwrap();
        let stations = parse_stations(STATIONS).unwrap();
        let services = parse_services(ANNOUNCEMENTS).unwrap();
        let delays = parse_delays(ANNOUNCEMENTS, now).unwrap();
        let reports = parse_positions(POSITIONS).unwrap();
        let mut trains: Vec<Train> = reports.iter().map(|r| train(r, &services, &delays, &stations, now)).collect();
        trains.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(trains.len(), 3, "the inactive one is left out, the repeat merged");

        let sj = &trains[0];
        assert_eq!(sj.id, "trafikverket:2026-10-14:531");
        assert_eq!(sj.name, "SJ Snabbtåg 531");
        assert_eq!(sj.kind, Kind::HighSpeed);
        assert_eq!(sj.operator.as_deref(), Some("SJ"));
        assert_eq!(sj.destination.as_deref(), Some("Göteborg C"));
        assert_eq!(sj.delay_minutes, Some(4), "the last actual time wins over the next estimate");
        assert_eq!((sj.lat, sj.lon, sj.speed_kmh), (59.1, 17.0, Some(190.0)), "the newest report");

        let unannounced = &trains[1];
        assert_eq!(unannounced.name, "Train 4711");
        assert_eq!(unannounced.kind, Kind::Other, "not called freight: nothing says it is");
        assert_eq!(unannounced.id, "trafikverket:2026-10-14:~4711");

        let unknown = &trains[2];
        assert_eq!(unknown.id, "trafikverket::1322", "a number whose run isn't found yet");
        assert_eq!((unknown.name.as_str(), unknown.kind), ("Train 1322", Kind::Other));
    }

    #[test]
    fn a_position_takes_the_run_of_its_own_day() {
        let runs = vec![
            Service { product: Some("Öresundståg".into()), date: "2026-10-13".into(), ..Default::default() },
            Service {
                product: Some("Öresundståg".into()),
                owner: Some("Skånetrafiken".into()),
                date: "2026-10-14".into(),
                ..Default::default()
            },
        ];
        assert_eq!(run_of(&runs, Some("2026-10-13")).unwrap().date, "2026-10-13");
        assert_eq!(run_of(&runs, None).unwrap().date, "2026-10-14", "else the latest");
        assert!(
            services_query(&["1322".into(), "531".into()])
                .contains("<EQ name=\"AdvertisedTrainIdent\" value=\"1322\"/>")
        );
    }

    #[test]
    fn delays_and_kinds() {
        let now = unix_ms("2026-10-14T10:20:00+02:00").unwrap();
        let services = parse_services(ANNOUNCEMENTS).unwrap();
        assert_eq!(services["2140"][0].owner.as_deref(), Some("SL"));
        assert_eq!(parse_delays(ANNOUNCEMENTS, now).unwrap().get("2140"), None, "no times to go by");
        assert_eq!(kind(Some("Pendeltåg"), Some("SL")), Kind::Suburban);
        assert_eq!(kind(Some("Öresundståg"), Some("Skånetrafiken")), Kind::Regional);
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

        // the departure listed first when both share a minute: still one stop
        let body = r#"{"RESPONSE":{"RESULT":[{"TrainAnnouncement":[
            {"ActivityType":"Avgang","LocationSignature":"Cst","AdvertisedTimeAtLocation":"2026-10-14T10:00:00Z"},
            {"ActivityType":"Avgang","LocationSignature":"U","AdvertisedTimeAtLocation":"2026-10-14T10:28:00Z"},
            {"ActivityType":"Ankomst","LocationSignature":"U","AdvertisedTimeAtLocation":"2026-10-14T10:28:00Z"},
            {"ActivityType":"Ankomst","LocationSignature":"G","AdvertisedTimeAtLocation":"2026-10-14T13:00:00Z"}]}]}}"#;
        let stops = parse_stops(body, &HashMap::new()).unwrap();
        assert_eq!(stops.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Cst", "U", "G"]);
        assert!(stops[1].arrival.is_some() && stops[1].departure.is_some());
    }

    #[test]
    fn errors_from_the_api_are_reported() {
        let body = r#"{"RESPONSE":{"RESULT":[{"ERROR":{"SOURCE":"Request","MESSAGE":"Invalid authentication"}}]}}"#;
        assert_eq!(results(body).unwrap_err(), "Invalid authentication");
    }

    #[test]
    fn a_position_with_only_its_operational_number_finds_its_announcements() {
        let now = unix_ms("2026-10-15T08:45:00+02:00").unwrap();
        let announcements = r#"{"RESPONSE":{"RESULT":[{"TrainAnnouncement":[
            {"AdvertisedTrainIdent":"1065","OperationalTrainNumber":"11065","ScheduledDepartureDateTime":"2026-10-15T00:00:00.000+02:00",
             "ActivityType":"Avgang","LocationSignature":"Lu","AdvertisedTimeAtLocation":"2026-10-15T08:40:00.000+02:00",
             "TimeAtLocation":"2026-10-15T08:43:00.000+02:00",
             "ProductInformation":[{"Description":"Öresundståg"}],"InformationOwner":"Skånetrafiken",
             "ToLocation":[{"LocationName":"Hb","Order":0}]}]}]}}"#;
        let positions = r#"{"RESPONSE":{"RESULT":[{"TrainPosition":[
            {"Train":{"OperationalTrainNumber":"11065"},"Position":{"WGS84":"POINT (13.2 55.72)"},
             "TimeStamp":"2026-10-15T08:44:30.000+02:00","Status":{"Active":true}}]}]}}"#;
        let services = parse_services(announcements).unwrap();
        let delays = parse_delays(announcements, now).unwrap();
        let report = &parse_positions(positions).unwrap()[0];
        assert_eq!(report.lookup(), "11065");
        assert!(services_query(&["11065".into()]).contains("<EQ name=\"OperationalTrainNumber\" value=\"11065\"/>"));

        let t = train(report, &services, &delays, &HashMap::new(), now);
        assert_eq!(t.name, "Öresundståg 1065", "named by the number travellers see");
        assert_eq!(t.operator.as_deref(), Some("Skånetrafiken"));
        assert_eq!(t.id, "trafikverket:2026-10-15:1065", "its stops are found by the advertised number");
        assert_eq!(t.delay_minutes, Some(3));
    }

    #[test]
    fn event_streams_split_into_their_data() {
        let mut buf = ":Connected\n\nid: 1\ndata: {\"a\":1}\n\nid: 2\ndata: {\"b\"".to_owned();
        assert_eq!(take_events(&mut buf), vec![r#"{"a":1}"#.to_owned()], "the comment line carries no data");
        assert_eq!(buf, "id: 2\ndata: {\"b\"", "the part event waits for the rest");
        buf.push_str(":2}\n\n");
        assert_eq!(take_events(&mut buf), vec![r#"{"b":2}"#.to_owned()]);
    }

    #[test]
    fn the_delay_is_the_latest_actual_time_else_the_next_estimate() {
        let at = |planned: i64, actual: Option<i64>, estimated: Option<i64>| Some(StopTime { planned: Some(planned), actual, estimated });
        let stop = |arrival, departure| Stop {
            name: String::new(),
            lat: None,
            lon: None,
            arrival,
            departure,
            track: None,
            canceled: false,
            notes: vec![],
        };
        let stops = vec![
            stop(None, at(0, Some(120), None)),
            stop(at(600, Some(840), None), at(660, None, Some(900))),
            stop(at(1200, None, Some(1500)), None),
        ];
        assert_eq!(delay_of(&stops), Some(4));
        assert_eq!(delay_of(&stops[2..]), Some(5));
        assert_eq!(delay_of(&[]), None);
    }
}
