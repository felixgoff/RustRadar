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
            "TrainComposition",
        ])
    )
}

/// How many cars (locomotives counted) a row's car order lists:
/// "Vagnsordning Lok-2-3-4-5[-6]" is six, "Vagnsordning 11,10,12" three; none without one.
fn cars_in(composition: &Value) -> Option<u32> {
    composition.as_array()?.iter().find_map(|c| {
        let order = c["Description"].as_str()?.strip_prefix("Vagnsordning")?;
        let n = order.split(|ch: char| !ch.is_alphanumeric()).filter(|w| !w.is_empty()).count();
        (n > 0).then_some(n as u32)
    })
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

fn product_code(v: &Value) -> Option<String> {
    v.as_array()?.iter().find_map(|x| text(&x["Code"]))
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
    /// The product's code ("PNA044"), for its kind of train (`train_class`).
    pub product_code: Option<String>,
    pub owner: Option<String>,
    /// Destination's signature.
    pub to: Option<String>,
    /// The day of its first departure, `YYYY-MM-DD`: with the number, which run it is.
    pub date: String,
    /// Where it stops, in order, with the timetable's times.
    pub calls: Vec<Call>,
    /// How many cars (and locomotives) its car order lists, where it has one.
    pub cars: Option<u32>,
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
                    product_code: product_code(&row["ProductInformation"]),
                    owner: text(&row["InformationOwner"]).or_else(|| text(&row["Operator"])),
                    to: last_location(&row["ToLocation"]),
                    date,
                    calls: Vec::new(),
                    cars: None,
                });
                runs.len() - 1
            }
        };
        // the rows of places it only passes carry no product, owner or destination
        let run = &mut runs[at];
        run.product = run.product.take().or_else(|| description(&row["ProductInformation"]));
        run.product_code = run.product_code.take().or_else(|| product_code(&row["ProductInformation"]));
        run.owner = run.owner.take().or_else(|| text(&row["InformationOwner"])).or_else(|| text(&row["Operator"]));
        run.to = run.to.take().or_else(|| last_location(&row["ToLocation"]));
        run.cars = run.cars.or_else(|| cars_in(&row["TrainComposition"]));
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
        products_seen(run);
    }
    Ok(with_aliases(services, aliases))
}

/// A product as the API names it, and the kind of train taken for it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductSeen {
    pub code: Option<String>,
    pub name: Option<String>,
    pub class: crate::train_class::TrainClass,
    /// Whether the table knows it; else the default class is taken.
    pub known: bool,
    /// Runs seen with it.
    pub runs: usize,
}

type Products = Mutex<HashMap<(Option<String>, Option<String>), usize>>;

fn products() -> &'static Products {
    static P: OnceLock<Products> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// Dev builds: remember each product seen (and say so the first time).
fn products_seen(run: &Service) {
    if !crate::devlog::ENABLED {
        return;
    }
    let key = (run.product_code.clone(), run.product.clone());
    let mut seen = products().lock().unwrap();
    let count = seen.entry(key).or_insert(0);
    if *count == 0 {
        let class = crate::train_class::class_of(run.product_code.as_deref(), run.product.as_deref());
        eprintln!(
            "[product] {} {:?} → {} ({} km/h, {:?}){}",
            run.product_code.as_deref().unwrap_or("-"),
            run.product.as_deref().unwrap_or("-"),
            class.vehicle,
            class.top_kmh,
            class.category,
            if crate::train_class::known(run.product_code.as_deref(), run.product.as_deref()) { "" } else { "  ← not in the table" },
        );
    }
    *count += 1;
}

/// Dev builds: every product seen so far, with the class taken for it.
#[tauri::command]
pub fn product_codes() -> Vec<ProductSeen> {
    let seen = products().lock().unwrap();
    let mut out: Vec<ProductSeen> = seen
        .iter()
        .map(|((code, name), runs)| ProductSeen {
            code: code.clone(),
            name: name.clone(),
            class: crate::train_class::class_of(code.as_deref(), name.as_deref()),
            known: crate::train_class::known(code.as_deref(), name.as_deref()),
            runs: *runs,
        })
        .collect();
    out.sort_by(|a, b| a.code.cmp(&b.code).then(a.name.cmp(&b.name)));
    out
}

/// How a train is drawn, kept per train so the next report carries it on
/// from where it is drawn instead of starting it anew.
#[derive(Clone)]
struct Drawn {
    /// The report it was worked out from.
    fix: motion::Fix,
    /// When it was worked out.
    made_ms: i64,
    sim: motion::Sim,
}

type Plans = Mutex<HashMap<String, Drawn>>;

fn plans() -> &'static Plans {
    static P: OnceLock<Plans> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// The same report is worked out again this often, to stay on the horizon, ms.
const REWORK_MS: i64 = 60_000;
/// Drawn longer ago than this, a train starts anew, ms.
const CONTINUE_MS: i64 = 5 * 60_000;
/// Reported at the same place (without a speed) this much later, a train is standing, ms:
/// a moving train's reports repeat its place for up to most of a minute.
const STOOD_MS: i64 = 60_000;

/// A train's frames (`motion::simulate`): from where it is drawn, following
/// where the real train most likely is by its newest report. The same
/// report gives the same frames for a while (the map refreshing doesn't
/// move it); a report that only repeats the last one's place (its position
/// not updated) changes nothing, unless it says the train has stopped.
fn frames_for(
    report: &Report,
    prev: Option<&Report>,
    stops: &[motion::Stop],
    now_ms: i64,
    rail: &crate::rail::Rail,
) -> motion::Sim {
    // tests share no state between them
    if cfg!(test) {
        let prev = prev.map(Report::fix);
        return motion::simulate(None, &report.fix(), prev.as_ref(), stops, now_ms, Some(rail));
    }
    frames_in(plans(), report, prev, stops, now_ms, rail)
}

/// `frames_for`, with how the trains are drawn kept in `plans`.
fn frames_in(
    plans: &Plans,
    report: &Report,
    prev: Option<&Report>,
    stops: &[motion::Stop],
    now_ms: i64,
    rail: &crate::rail::Rail,
) -> motion::Sim {
    let mut latest = report.fix();
    let prev = prev.map(Report::fix);
    let key = report.lookup().to_owned();
    let old = plans.lock().unwrap().get(&key).cloned().filter(|d| now_ms - d.made_ms < CONTINUE_MS);
    // an older report than it is drawn from (one source lagging another) changes nothing
    if let Some(d) = &old
        && latest.t_ms < d.fix.t_ms
    {
        return d.sim.clone();
    }
    if let Some(d) = &old
        && motion::distance((d.fix.lat, d.fix.lon), (latest.lat, latest.lon)) < SAME_PLACE_M
    {
        match latest.speed {
            // says it has stopped there
            Some(v) if v < 1.0 => {}
            // not saying, but there a while: it is standing
            None if latest.t_ms - d.fix.t_ms >= STOOD_MS => latest.speed = Some(0.0),
            // its position only not updated
            _ => latest = d.fix,
        }
    }
    if let Some(d) = &old
        && latest == d.fix
    {
        if now_ms - d.made_ms < REWORK_MS {
            return d.sim.clone();
        }
        // standing, and nothing new: still there
        if let [.., a, b] = d.sim.frames.as_slice()
            && (a.0, a.1) == (b.0, b.1)
            && b.2 <= now_ms + motion::HORIZON_MS
        {
            let mut sim = d.sim.clone();
            if let Some(last) = sim.frames.last_mut() {
                last.2 = now_ms + motion::HORIZON_MS;
            }
            plans.lock().unwrap().insert(key, Drawn { fix: latest, made_ms: now_ms, sim: sim.clone() });
            return sim;
        }
    }
    let drawn = old.as_ref().map(|d| d.sim.frames.as_slice());
    let sim = motion::simulate(drawn, &latest, prev.as_ref(), stops, now_ms, Some(rail));
    let mut plans = plans.lock().unwrap();
    plans.retain(|_, d| now_ms - d.made_ms < CONTINUE_MS);
    plans.insert(key, Drawn { fix: latest, made_ms: now_ms, sim: sim.clone() });
    sim
}

/// How the train with this report's number is drawn now, if it is.
fn drawn_now(lookup: &str, now_ms: i64) -> Option<motion::Drawn> {
    let plans = plans().lock().unwrap();
    plans.get(lookup).and_then(|d| motion::drawn_at(&d.sim.frames, now_ms))
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
    /// The report as the motion takes it.
    pub fn fix(&self) -> motion::Fix {
        motion::Fix {
            lat: self.lat,
            lon: self.lon,
            t_ms: self.at_ms,
            speed: self.speed.map(|s| f64::from(s) / 3.6),
            heading: self.bearing.map(f64::from),
        }
    }

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
    prev: Option<&Report>,
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
    // along the track, on its limits for its kind of train
    let class = service.map(|s| crate::train_class::class_of(s.product_code.as_deref(), s.product.as_deref()));
    let rail = crate::rail::Rail { class };
    let motion::Sim { frames, predicted } = frames_for(report, prev, &stops, now_ms, &rail);
    // the track behind it, as long as its cars (three where not known), for them to be drawn on
    let cars = service.and_then(|s| s.cars).unwrap_or(DEFAULT_CARS);
    let tail = tail_of(&frames, report, f64::from(cars) * CAR_M + TAIL_SPARE_M);
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
        reports: if crate::devlog::ENABLED {
            let sight = |r: &Report, latest| crate::trains::Sighting {
                lat: r.lat,
                lon: r.lon,
                unix_ms: r.at_ms,
                speed_kmh: r.speed,
                latest,
            };
            prev.map(|p| sight(p, false)).into_iter().chain([sight(report, true)]).collect()
        } else {
            Vec::new()
        },
        predicted: if crate::devlog::ENABLED { predicted } else { Vec::new() },
        cars: service.and_then(|s| s.cars),
        tail,
    }
}

/// A train's cars where its operator doesn't say how many.
const DEFAULT_CARS: u32 = 4;
/// How long a car is drawn, metres (a coach is about 25 m, with its coupling).
pub const CAR_M: f64 = 26.0;
/// The track behind a train reaches this much further than its cars, metres.
const TAIL_SPARE_M: f64 = 40.0;

/// The track behind where a train's frames start, back the way it came
/// (its drawn heading, else its report's), `length` metres; empty where the
/// track isn't known.
fn tail_of(frames: &[motion::Frame], report: &Report, length: f64) -> Vec<(f64, f64)> {
    let Some(first) = frames.first() else { return Vec::new() };
    let heading = motion::drawn_at(frames, first.2)
        .and_then(|d| d.heading)
        .or(report.bearing.map(f64::from))
        .filter(|h| h.is_finite());
    let Some(h) = heading else { return Vec::new() };
    crate::rail::track_from((first.0, first.1), (h + 180.0) % 360.0, length).unwrap_or_default()
}

/// The run's stops still ahead at `at_ms`, with places, at the times now
/// expected: the timetable's shifted by the delay (never leaving early).
/// A stop due this long before a report is behind the train whatever its place says.
const PASSED_BY_CLOCK_MS: i64 = 30 * 60_000;

fn stops_ahead(
    run: &Service,
    delay: Option<i32>,
    stations: &HashMap<String, Station>,
    at_ms: i64,
) -> Vec<motion::Stop> {
    calls_ahead(run, delay, stations, at_ms).into_iter().map(|(_, _, stop, _)| stop).collect()
}

/// A stop ahead with its call and station, and the way the train runs through it.
type CallAhead<'a> = (&'a Call, &'a Station, motion::Stop, Option<f64>);

/// As `stops_ahead`, with each stop's call and station, and the way the train runs through it.
fn calls_ahead<'a>(
    run: &'a Service,
    delay: Option<i32>,
    stations: &'a HashMap<String, Station>,
    at_ms: i64,
) -> Vec<CallAhead<'a>> {
    let late = i64::from(delay.unwrap_or(0)) * 60_000;
    let mut placed: Vec<(&Call, &Station, (f64, f64))> = run
        .calls
        .iter()
        .filter_map(|c| {
            let station = stations.get(&c.station)?;
            Some((c, station, (station.lat?, station.lon?)))
        })
        .collect();
    // places due the same minute come in any order: the nearer the one before first
    let time = |c: &Call| c.arrive.or(c.depart);
    for i in 1..placed.len().saturating_sub(1) {
        let (before, a, b) = (placed[i - 1].2, &placed[i], &placed[i + 1]);
        if time(a.0) == time(b.0) && motion::distance(before, b.2) < motion::distance(before, a.2) {
            placed.swap(i, i + 1);
        }
    }
    // by the clock only long gone ones: which it has passed is told by where
    // it is (`motion::plan`), so a late train still calls at the next one
    let gone = |c: &Call| {
        let due = c.depart.map(|t| t + late.max(0)).or(c.arrive.map(|t| t + late));
        due.is_none_or(|t| t < at_ms - PASSED_BY_CLOCK_MS)
    };
    placed
        .iter()
        .enumerate()
        .skip_while(|(_, (c, _, _))| gone(c))
        .map(|(i, &(c, station, place))| {
            // the way it runs through: from the station before to the one after
            let (before, after) = (placed[i.saturating_sub(1)].2, placed[(i + 1).min(placed.len() - 1)].2);
            let heading = (motion::distance(before, after) > 1.0).then(|| motion::course(before, after));
            // at the platform it is announced at, else on the track running its way, where the track is known
            let (lat, lon) = crate::rail::platform(place, c.track.as_deref(), heading).unwrap_or(place);
            let stop = motion::Stop {
                lat,
                lon,
                arrive_ms: c.arrive.map(|t| t + late),
                depart_ms: c.depart.map(|t| t + late.max(0)),
                pass: !c.stops,
            };
            (c, station, stop, heading)
        })
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
    let trains: Vec<Train> =
        reports.iter().map(|r| train(r, previous(r).as_ref(), &services, &delays, &stations, now_ms)).collect();
    remember(&trains, &reports, now_ms, true);
    Ok(trains)
}

/// Notes which number each train runs under (for following it) and where
/// it was last seen (for its route); `all`: these are all the trains now.
fn remember(trains: &[Train], reports: &[Report], now_ms: i64, all: bool) {
    note_operators(trains, reports, all);
    let mut numbers = operational_numbers().lock().unwrap();
    if all {
        numbers.clear();
    }
    numbers.extend(trains.iter().zip(reports).map(|(t, r)| (t.id.clone(), r.lookup().to_owned())));
    drop(numbers);
    let mut places = last_places().lock().unwrap();
    for (t, r) in trains.iter().zip(reports) {
        if places.get(&t.id).is_none_or(|p| p.latest.t_ms <= r.at_ms) {
            places.insert(t.id.clone(), Seen { latest: r.fix() });
        }
    }
    places.retain(|_, p| now_ms - p.latest.t_ms < 6 * 3_600_000);
    drop(places);
    history().lock().unwrap().retain(|_, h| now_ms - h.0.at_ms < 6 * 3_600_000);
}

// ------------------------------------------------------------- live streams

/// The fields a live stream of positions needs.
const POSITION_STREAM_FIELDS: &[&str] = &[
    "Train.AdvertisedTrainNumber",
    "Train.OperationalTrainNumber",
    "Train.JourneyPlanDepartureDate",
    "Position.WGS84",
    "Bearing",
    "Speed",
    "TimeStamp",
];

fn position_stream(filter: &str, include: bool) -> String {
    format!(
        "<QUERY sseurl=\"true\" objecttype=\"TrainPosition\" namespace=\"järnväg.trafikinfo\" schemaversion=\"1.1\" limit=\"0\">{filter}{}</QUERY>",
        if include { includes(POSITION_STREAM_FIELDS) } else { String::new() }
    )
}

/// A stream of these trains' positions (by operational number), in the shape
/// a single train's stream has.
pub fn trains_query(numbers: &[String]) -> String {
    let eqs: String = numbers
        .iter()
        .map(|n| format!("<EQ name=\"Train.OperationalTrainNumber\" value=\"{}\"/>", xml_escape(n)))
        .collect();
    position_stream(&format!("<FILTER><OR>{eqs}</OR></FILTER>"), true)
}

/// Reports gathered this long before the trains they move are worked out.
const STREAM_BATCH: Duration = Duration::from_millis(1000);
/// How often a stream checks it is still wanted, and with the same key.
const STREAM_CHECK: Duration = Duration::from_secs(5);
/// The live streams count as live while one sent something this recently.
const LIVE_FOR_MS: i64 = 60_000;

/// When a live stream (of an operator's trains) last brought a report, Unix ms.
static STREAMS_AT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Whether the operators' live streams are bringing reports.
pub fn streams_live() -> bool {
    crate::trains::now_ms() - STREAMS_AT.load(std::sync::atomic::Ordering::Relaxed) < LIVE_FOR_MS
}

// ------------------------------------------------------ each operator, live

/// At most this many trains in one stream: an operator with more has several.
const MAX_STREAM_TRAINS: usize = 150;
/// An operator's stream is opened again for trains new to it at most this often.
const REOPEN_FOR_NEW: Duration = Duration::from_secs(60);

/// Each operator's trains (operational numbers), by the name the map shows
/// as its operator ("" when not known), as last seen.
type OperatorTrains = Mutex<HashMap<String, std::collections::BTreeSet<String>>>;

fn operator_trains() -> &'static OperatorTrains {
    static O: OnceLock<OperatorTrains> = OnceLock::new();
    O.get_or_init(Default::default)
}

/// The operators the map leaves out: their trains aren't streamed.
fn hidden_operators() -> &'static Mutex<Vec<String>> {
    static H: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    H.get_or_init(Default::default)
}

/// The operators the map leaves out (as `Train::operator` names them, "" for unknown).
pub fn hide_operators(names: Vec<String>) {
    *hidden_operators().lock().unwrap() = names;
}

/// Notes each train's operator and number; `all`: these are all the trains now.
fn note_operators(trains: &[Train], reports: &[Report], all: bool) {
    let mut ops = operator_trains().lock().unwrap();
    if all {
        ops.clear();
    }
    for (t, r) in trains.iter().zip(reports) {
        let number = r.lookup();
        if number.is_empty() {
            continue;
        }
        let op = t.operator.clone().unwrap_or_default();
        if !all {
            for (other, numbers) in ops.iter_mut() {
                if *other != op {
                    numbers.remove(number);
                }
            }
        }
        ops.entry(op).or_default().insert(number.to_owned());
    }
    ops.retain(|_, numbers| !numbers.is_empty());
}

/// The streams wanted: by operator and part, the numbers in each.
fn operator_streams_wanted() -> HashMap<(String, usize), Vec<String>> {
    let hidden = hidden_operators().lock().unwrap().clone();
    let ops = operator_trains().lock().unwrap();
    let mut out = HashMap::new();
    for (op, numbers) in ops.iter().filter(|(op, _)| !hidden.contains(op)) {
        let numbers: Vec<String> = numbers.iter().cloned().collect();
        for (i, part) in numbers.chunks(MAX_STREAM_TRAINS).enumerate() {
            out.insert((op.clone(), i), part.to_vec());
        }
    }
    out
}

/// Streams the positions of each operator's trains (Trafikverket's
/// positions say nothing of operators: by their numbers, from the
/// timetable) while the map shows Trafikverket's trains, but not those of
/// operators the map leaves out. Each report moves its train in the trains'
/// cache at once, and the page is told (`trains-changed`) to show it. An
/// operator's stream is opened again when trains new to it start running
/// (now and then), and closed when it has none left or is left out.
pub fn start_operator_streams(app: tauri::AppHandle) {
    use std::sync::atomic::Ordering::Relaxed;
    type Running = HashMap<(String, usize), (Vec<String>, Instant, tauri::async_runtime::JoinHandle<()>)>;
    tauri::async_runtime::spawn(async move {
        let mut running: Running = HashMap::new();
        let mut running_key = String::new();
        loop {
            tokio::time::sleep(STREAM_CHECK).await;
            let key = crate::keys::current().trafikverket;
            if key != running_key {
                for (_, (_, _, task)) in running.drain() {
                    task.abort();
                }
                running_key = key.clone();
            }
            let wanted = if key.is_empty() || !crate::trains::trafikverket_wanted() {
                HashMap::new()
            } else {
                operator_streams_wanted()
            };
            running.retain(|k, (_, _, task)| {
                let keep = wanted.contains_key(k);
                if !keep {
                    task.abort();
                }
                keep
            });
            for (k, numbers) in wanted {
                match running.get(&k) {
                    // as streamed, or fewer (trains that have ended): left as it is
                    Some((streamed, _, _)) if numbers.iter().all(|n| streamed.binary_search(n).is_ok()) => continue,
                    // new trains: opened again, now and then
                    Some((_, opened, _)) if opened.elapsed() < REOPEN_FOR_NEW => continue,
                    _ => {}
                }
                if let Some((_, _, task)) = running.remove(&k) {
                    task.abort();
                }
                let label = format!("operator {}{}", if k.0.is_empty() { "unknown" } else { &k.0 }, if k.1 > 0 { format!(" ({})", k.1 + 1) } else { String::new() });
                if crate::devlog::ENABLED {
                    eprintln!("[stream] {label}: {} trains", numbers.len());
                }
                let (app, query, key) = (app.clone(), trains_query(&numbers), key.clone());
                let task = tauri::async_runtime::spawn(async move {
                    loop {
                        let still = || crate::keys::current().trafikverket == key;
                        let heard = |_: usize| STREAMS_AT.store(crate::trains::now_ms(), Relaxed);
                        stream_positions(&app, &key, &query, &label, still, heard).await;
                        tokio::time::sleep(RECONNECT).await;
                    }
                });
                running.insert(k, (numbers, Instant::now(), task));
            }
        }
    });
}

/// One stream of positions, until it ends or `wanted` turns false. Reports
/// move their trains (`move_trains`); `heard` is told how many came.
async fn stream_positions(
    app: &tauri::AppHandle,
    key: &str,
    query: &str,
    label: &str,
    wanted: impl Fn() -> bool,
    heard: impl Fn(usize),
) {
    let Ok(http) = stream_client() else { return };
    let url = match sse_url(http, key, query).await {
        Ok(url) => url,
        Err(e) => {
            eprintln!("trafikverket: {label} stream: {e}");
            return;
        }
    };
    if crate::devlog::ENABLED {
        eprintln!("[stream] {label}: open");
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u8, String)>();
    let read = async {
        if let Err(e) = read_stream(&url, 2, &tx).await {
            eprintln!("trafikverket: {label} stream: {e}");
        }
    };
    let still_wanted = async {
        while wanted() {
            tokio::time::sleep(STREAM_CHECK).await;
        }
    };
    let handle = async {
        let mut pending: HashMap<String, Report> = HashMap::new();
        let mut tick = tokio::time::interval(STREAM_BATCH);
        let mut first = true;
        loop {
            tokio::select! {
                got = rx.recv() => {
                    let Some((_, data)) = got else { return };
                    let reports = parse_positions(&data).unwrap_or_default();
                    if reports.is_empty() {
                        continue;
                    }
                    heard(reports.len());
                    if first && crate::devlog::ENABLED {
                        eprintln!("[stream] {label}: live");
                    }
                    first = false;
                    for r in reports {
                        let k = r.lookup().to_owned();
                        if pending.get(&k).is_none_or(|old| old.at_ms < r.at_ms) {
                            pending.insert(k, r);
                        }
                    }
                }
                _ = tick.tick() => {
                    if !pending.is_empty() {
                        let reports: Vec<Report> = pending.drain().map(|(_, r)| r).collect();
                        move_trains(app, http, key, reports).await;
                    }
                }
            }
        }
    };
    tokio::select! {
        _ = read => {}
        _ = still_wanted => {}
        _ = handle => {}
    }
}

// ------------------------------------------------------- the trains in view

/// At most this many trains in view are streamed.
pub const MAX_WATCHED: usize = 40;
/// A new set of trains in view is streamed once it has held this long.
const WATCH_SETTLE: Duration = Duration::from_secs(2);

/// The trains the page shows close up (operational numbers), and since when.
fn watched() -> &'static Mutex<(Vec<String>, Instant)> {
    static W: OnceLock<Mutex<(Vec<String>, Instant)>> = OnceLock::new();
    W.get_or_init(|| Mutex::new((Vec::new(), Instant::now())))
}

/// The operational numbers of these trains (by the ids `trains` gave them),
/// sorted, without repeats, at most `MAX_WATCHED`.
fn numbers_of(ids: &[String]) -> Vec<String> {
    let known = operational_numbers().lock().unwrap();
    let mut numbers: Vec<String> = ids.iter().filter_map(|id| known.get(id).cloned()).take(MAX_WATCHED).collect();
    numbers.sort();
    numbers.dedup();
    numbers
}

/// The trains the page shows close up: streamed live, like a followed one.
pub fn watch(ids: &[String]) {
    let numbers = numbers_of(ids);
    let mut w = watched().lock().unwrap();
    if w.0 != numbers {
        *w = (numbers, Instant::now());
    }
}

/// Keeps one live stream for the trains in view (`watch`), reopened when
/// the set changes (once it settles) and closed when it empties.
pub fn start_view_streams(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut running: Option<(Vec<String>, tauri::async_runtime::JoinHandle<()>)> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let key = crate::keys::current().trafikverket;
            let (numbers, since) = watched().lock().unwrap().clone();
            let wanted = if key.is_empty() || !crate::trains::trafikverket_wanted() { Vec::new() } else { numbers };
            if running.as_ref().is_some_and(|(n, _)| *n == wanted) || since.elapsed() < WATCH_SETTLE {
                continue;
            }
            if let Some((_, task)) = running.take() {
                task.abort();
            }
            if wanted.is_empty() {
                continue;
            }
            if crate::devlog::ENABLED {
                eprintln!("[stream] view: {} trains", wanted.len());
            }
            let (app, query, key) = (app.clone(), trains_query(&wanted), key.clone());
            let task = tauri::async_runtime::spawn(async move {
                loop {
                    let still = || crate::keys::current().trafikverket == key;
                    stream_positions(&app, &key, &query, "view", still, |_| {}).await;
                    tokio::time::sleep(RECONNECT).await;
                }
            });
            running = Some((wanted, task));
        }
    });
}

/// The trains these reports move, worked out and put in the trains' cache.
async fn move_trains(app: &tauri::AppHandle, http: &reqwest::Client, key: &str, reports: Vec<Report>) {
    use tauri::Emitter;
    let now_ms = crate::trains::now_ms();
    // only what is newer than what the map has
    let reports: Vec<Report> = reports
        .into_iter()
        .filter(|r| now_ms - r.at_ms < STALE_REPORT_MS)
        .filter(|r| history().lock().unwrap().get(r.lookup()).is_none_or(|h| h.0.at_ms < r.at_ms))
        .collect();
    if reports.is_empty() {
        return;
    }
    let numbers = reports.iter().map(|r| r.lookup().to_owned()).collect();
    let services = learn_services(http, key, numbers).await;
    let stations = station_cache().lock().unwrap().clone().map(|(_, _, s)| s).unwrap_or_default();
    let delays = delay_cache().lock().unwrap().clone().map(|(_, _, d)| d).unwrap_or_default();
    // the way along the track is worked out off the async threads
    let worked = tauri::async_runtime::spawn_blocking(move || {
        let trains: Vec<Train> = reports
            .iter()
            .map(|r| train(r, previous(r).as_ref(), &services, &delays, &stations, now_ms))
            .collect();
        (trains, reports)
    })
    .await;
    let Ok((trains, reports)) = worked else { return };
    remember(&trains, &reports, now_ms, false);
    if crate::trains::patch(crate::trains::Source::Trafikverket, trains) {
        let _ = app.emit("trains-changed", ());
    }
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
            let source = match kind {
                0 => "Trafikverket stream: positions",
                1 => "Trafikverket stream: announcements",
                _ => "Trafikverket stream: country",
            };
            crate::devlog::received(source, url, &data);
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
            let prev = previous(&report);
            let train = train(&report, prev.as_ref(), &services, &delays, &stations, now);
            last_places().lock().unwrap().insert(train.id.clone(), Seen { latest: report.fix() });
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

/// A train's latest report.
#[derive(Clone, Copy, Debug)]
struct Seen {
    latest: motion::Fix,
}

/// Each train's latest report and the one before, by its number.
type History = Mutex<HashMap<String, (Report, Option<Report>)>>;

fn history() -> &'static History {
    static H: OnceLock<History> = OnceLock::new();
    H.get_or_init(Default::default)
}

/// Reports this close to the one before are the same place: the train
/// hasn't moved (or the report repeats itself), metres.
const SAME_PLACE_M: f64 = 2.0;

/// Whether a report puts the train where the one before did.
fn same_place(a: &Report, b: &Report) -> bool {
    motion::distance((a.lat, a.lon), (b.lat, b.lon)) < SAME_PLACE_M
}

/// The report before this one of the same train; this one is remembered.
/// One that puts it where the last one did changes nothing: the last one
/// stays the newest, the one before it stays.
fn previous(report: &Report) -> Option<Report> {
    let mut h = history().lock().unwrap();
    let key = report.lookup().to_owned();
    match h.get_mut(&key) {
        Some((latest, prev)) if latest.at_ms == report.at_ms || same_place(latest, report) => {
            // there a while, not saying how fast: it stands there (and moving on from it, leaves)
            if latest.speed.is_none() && report.speed.is_none() && report.at_ms - latest.at_ms >= STOOD_MS {
                latest.speed = Some(0.0);
            }
            prev.clone()
        }
        Some((latest, _)) if latest.at_ms < report.at_ms => {
            let prev = latest.clone();
            h.insert(key, (report.clone(), Some(prev.clone())));
            Some(prev)
        }
        // an older one (a stream catching up): nothing to draw from
        Some(_) => None,
        None => {
            h.insert(key, (report.clone(), None));
            None
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
    let now = crate::trains::now_ms();
    let lookup = operational_numbers().lock().unwrap().get(id).cloned();
    let drawn = lookup.and_then(|n| drawn_now(&n, now));
    Ok(route_from(&run, delay, &stations, seen.map(|s| s.latest), drawn, now))
}

/// `route`: the way on for a run from where the train is `drawn` at `now`
/// (else where it was last `seen`).
fn route_from(
    run: &Service,
    delay: Option<i32>,
    stations: &HashMap<String, Station>,
    seen: Option<motion::Fix>,
    drawn: Option<motion::Drawn>,
    now: i64,
) -> TrainRoute {
    let at_ms = seen.map_or(now, |s| s.t_ms);
    let calls = calls_ahead(run, delay, stations, at_ms);
    let stops: Vec<motion::Stop> = calls.iter().map(|c| c.2).collect();

    // from where the train is drawn now (else where it was last seen): its
    // place, speed and the way it goes
    let start = drawn.map(|d| (d.at, Some(d.v), d.heading)).or_else(|| seen.map(|s| ((s.lat, s.lon), s.speed, s.heading)));
    // the station it stands at (it goes on from there), else the next on its way
    let (first, onwards) = match start.map(|(at, v, h)| motion::plan(at, v, h, &stops, now)) {
        Some(motion::Plan::Standing { at }) => (at, at + 1),
        Some(motion::Plan::Moving { next }) => (next, next),
        None => (0, 0),
    };
    // each place's possible spots, the one its motion heads for first
    let mut places: Vec<Vec<(f64, f64)>> = start.iter().map(|s| vec![s.0]).collect();
    places.extend(calls[onwards.min(calls.len())..].iter().map(|(call, station, stop, heading)| {
        let mut spots = vec![(stop.lat, stop.lon)];
        if let (Some(lat), Some(lon)) = (station.lat, station.lon)
            && let Some(more) = crate::rail::platforms((lat, lon), call.track.as_deref(), *heading)
        {
            spots.extend(more.into_iter().filter(|p| motion::distance(*p, (stop.lat, stop.lon)) >= 3.0));
        }
        spots
    }));
    // on its way: on the way it is going, never back the way it came
    let heading = start.filter(|s| s.1.is_some_and(|v| v >= 0.5)).and_then(|s| s.2);
    let (segments, complete, passed) = crate::rail::way_through(&places, heading);
    // (and not the places it has passed)
    let ahead_calls = &calls[(first + passed).min(calls.len())..];
    TrainRoute {
        segments: segments
            .into_iter()
            .filter(|s| s.len() >= 2)
            .map(|s| s.into_iter().map(|p| [p.1, p.0]).collect())
            .collect(),
        stops: ahead_calls
            .iter()
            .map(|(call, station, stop, _)| RouteStop {
                name: station.name.clone(),
                lat: stop.lat,
                lon: stop.lon,
                track: call.track.clone(),
                stops: call.stops,
            })
            .collect(),
        complete,
    }
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
    fn a_trains_cars_are_counted_from_its_car_order() {
        let order = |d: &str| serde_json::json!([{ "Code": "TNA001", "Description": d }]);
        assert_eq!(cars_in(&order("Vagnsordning 3-2-1")), Some(3));
        assert_eq!(cars_in(&order("Vagnsordning Lok-2-3-4-5[-6]")), Some(6));
        assert_eq!(cars_in(&order("Vagnsordning 11,10,12,13,14,15,16")), Some(7));
        assert_eq!(cars_in(&order("Vagnsordning 14-13-12-11-10-8, 7-6-5-4-3-1")), Some(12));
        assert_eq!(cars_in(&order("Vagnsordning A-B-C-D-E")), Some(5));
        assert_eq!(cars_in(&order("Bakre delen går låst.")), None);
        assert_eq!(cars_in(&serde_json::Value::Null), None);
    }

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
        let mut trains: Vec<Train> = reports.iter().map(|r| train(r, None, &services, &delays, &stations, now)).collect();
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

        let t = train(report, None, &services, &delays, &HashMap::new(), now);
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

    #[test]
    fn the_trains_in_view_are_one_stream() {
        let q = trains_query(&["1061".into(), "2<3".into()]);
        assert!(q.contains(r#"sseurl="true""#) && q.contains(r#"limit="0""#));
        assert!(q.contains(r#"<OR><EQ name="Train.OperationalTrainNumber" value="1061"/>"#));
        assert!(q.contains("2&lt;3"), "escaped");
        assert!(q.contains("<INCLUDE>Position.WGS84</INCLUDE>"));
        // ids to their numbers: known ones only, sorted, no repeats, capped
        {
            let mut n = operational_numbers().lock().unwrap();
            n.insert("trafikverket:2026-10-15:540".into(), "540".into());
            n.insert("trafikverket:2026-10-15:88".into(), "98".into());
            for i in 0..60 {
                n.insert(format!("trafikverket:t:{i}"), format!("9{i:03}"));
            }
        }
        let ids: Vec<String> =
            ["trafikverket:2026-10-15:88", "trafikverket:2026-10-15:540", "trafikverket:2026-10-15:540", "unknown"]
                .map(String::from)
                .to_vec();
        assert_eq!(numbers_of(&ids), vec!["540".to_string(), "98".to_string()]);
        let many: Vec<String> = (0..60).map(|i| format!("trafikverket:t:{i}")).collect();
        assert_eq!(numbers_of(&many).len(), MAX_WATCHED);
    }

    #[test]
    fn each_operator_is_streamed_apart_but_not_those_left_out() {
        let train = |id: &str, op: Option<&str>| Train {
            id: id.into(),
            source: "trafikverket",
            name: id.into(),
            kind: Kind::Regional,
            lat: 59.0,
            lon: 18.0,
            heading: None,
            speed_kmh: None,
            destination: None,
            operator: op.map(String::from),
            delay_minutes: None,
            position_unix: 0,
            frames: Vec::new(),
            reports: Vec::new(),
            predicted: Vec::new(),
            cars: None,
            tail: Vec::new(),
        };
        let report = |n: &str| Report {
            advertised: None,
            operational: Some(n.into()),
            date: None,
            lat: 59.0,
            lon: 18.0,
            bearing: None,
            speed: None,
            at_ms: 0,
        };
        let mut trains = vec![train("a", Some("SJ")), train("b", Some("SL")), train("c", None)];
        let mut reports = vec![report("1"), report("2"), report("3")];
        for i in 0..(MAX_STREAM_TRAINS + 10) {
            trains.push(train(&format!("sl{i}"), Some("SL")));
            reports.push(report(&format!("9{i:04}")));
        }
        note_operators(&trains, &reports, true);
        hide_operators(vec!["SJ".into()]);
        let wanted = operator_streams_wanted();
        hide_operators(Vec::new());
        assert!(!wanted.keys().any(|(op, _)| op == "SJ"), "left out");
        assert_eq!(wanted[&(String::new(), 0)], vec!["3".to_string()], "the unknown ones together");
        // SL in two parts
        assert_eq!(wanted[&("SL".to_string(), 0)].len(), MAX_STREAM_TRAINS);
        assert_eq!(wanted[&("SL".to_string(), 1)].len(), 11);
        // a train the stream says another operator runs moves over
        note_operators(&[train("x", Some("SJ"))], &[report("2")], false);
        assert!(operator_trains().lock().unwrap()["SJ"].contains("2"));
        assert!(!operator_trains().lock().unwrap()["SL"].contains("2"));
        assert!(trains_query(&["1".into()]).contains(r#"<EQ name="Train.OperationalTrainNumber" value="1"/>"#));
    }

    /// Replays recorded live positions (`LIVE_DIR`: `stations.json`,
    /// `services.json`, `stream.jsonl` of `{at, data}` stream events) through
    /// the app's own pipeline on real tiles (`RAIL_TILES`), writing each
    /// report and how it was drawn to `LIVE_DIR/replay.csv`.
    #[test]
    #[ignore]
    fn replay_live() {
        let dir = std::env::var("LIVE_DIR").unwrap();
        crate::rail::load_tiles_for_tests(&std::env::var("RAIL_TILES").unwrap());
        let stations = parse_stations(&std::fs::read_to_string(format!("{dir}/stations.json")).unwrap()).unwrap();
        let services = parse_services(&std::fs::read_to_string(format!("{dir}/services.json")).unwrap()).unwrap();
        let mut events: Vec<(i64, Vec<Report>)> = std::fs::read_to_string(format!("{dir}/stream.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| {
                let v: Value = serde_json::from_str(l).ok()?;
                Some((v["at"].as_i64()?, parse_positions(v["data"].as_str()?).ok()?))
            })
            .collect();
        events.sort_by_key(|e| e.0);
        let plans: Plans = Default::default();
        let mut out = String::from("now,train,t,lat,lon,speed,next,next_name,next_m,drawn_lat,drawn_lon,pred_end_lat,pred_end_lon,pred_end_to_next_m\n");
        for (now, reports) in events {
            let only = std::env::var("REPLAY_ONLY").ok();
            // (`REPLAY_ONLY`: one train's number, to look at it alone)
            for r in reports
                .iter()
                .filter(|r| now - r.at_ms < STALE_REPORT_MS)
                .filter(|r| only.as_deref().is_none_or(|o| o == r.lookup()))
            {
                let prev = previous(r);
                let runs = services.get(r.lookup());
                let service = runs.and_then(|runs| run_of(runs, r.date.as_deref()));
                let stops = service.map(|s| stops_ahead(s, None, &stations, r.at_ms)).unwrap_or_default();
                let names: Vec<String> = service
                    .map(|s| calls_ahead(s, None, &stations, r.at_ms).iter().map(|c| c.1.name.clone()).collect())
                    .unwrap_or_default();
                let class = service.map(|s| crate::train_class::class_of(s.product_code.as_deref(), s.product.as_deref()));
                let rail = crate::rail::Rail { class };
                let sim = frames_in(&plans, r, prev.as_ref(), &stops, now, &rail);
                let fix = r.fix();
                let next = match motion::plan((fix.lat, fix.lon), fix.speed, fix.heading, &stops, fix.t_ms) {
                    motion::Plan::Standing { at } | motion::Plan::Moving { next: at } => at,
                };
                let (next_name, next_m, next_at) = match stops.get(next) {
                    Some(st) => (
                        names.get(next).cloned().unwrap_or_default(),
                        motion::distance((r.lat, r.lon), (st.lat, st.lon)),
                        Some((st.lat, st.lon)),
                    ),
                    None => (String::new(), f64::NAN, None),
                };
                let drawn = motion::sample(&sim.frames, now).map(|s| s.0).or(sim.frames.first().map(|f| (f.0, f.1)));
                let end = sim.predicted.last().map(|f| (f.0, f.1));
                let end_to_next = end.zip(next_at).map_or(f64::NAN, |(a, b)| motion::distance(a, b));
                let (dl, dn) = drawn.unwrap_or((f64::NAN, f64::NAN));
                let (el, en) = end.unwrap_or((f64::NAN, f64::NAN));
                out += &format!(
                    "{now},{},{},{:.6},{:.6},{},{next},{next_name},{next_m:.0},{dl:.6},{dn:.6},{el:.6},{en:.6},{end_to_next:.0}\n",
                    r.lookup(),
                    r.at_ms,
                    r.lat,
                    r.lon,
                    r.speed.map_or(String::new(), |v| v.to_string()),
                );
            }
        }
        std::fs::write(format!("{dir}/replay.csv"), out).unwrap();
    }

    /// Prints the queries the app asks for these trains' timetables (`NUMBERS`), comma separated, and the stations.
    #[test]
    #[ignore]
    fn print_queries() {
        let numbers: Vec<String> = std::env::var("NUMBERS").unwrap().split(',').map(str::to_owned).collect();
        for chunk in numbers.chunks(SERVICE_BATCH) {
            println!("Q {}", services_query(chunk));
        }
        println!("S {}", stations_query());
    }

    /// Times working out the trains' frames over recorded live data (`LIVE_DIR`, `RAIL_TILES`).
    #[test]
    #[ignore]
    fn bench_frames() {
        let dir = std::env::var("LIVE_DIR").unwrap();
        crate::rail::load_tiles_for_tests(&std::env::var("RAIL_TILES").unwrap());
        let stations = parse_stations(&std::fs::read_to_string(format!("{dir}/stations.json")).unwrap()).unwrap();
        let services = parse_services(&std::fs::read_to_string(format!("{dir}/services.json")).unwrap()).unwrap();
        let mut events: Vec<(i64, Vec<Report>)> = std::fs::read_to_string(format!("{dir}/stream.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| {
                let v: Value = serde_json::from_str(l).ok()?;
                Some((v["at"].as_i64()?, parse_positions(v["data"].as_str()?).ok()?))
            })
            .collect();
        events.sort_by_key(|e| e.0);
        let limit: usize = std::env::var("BENCH_EVENTS").ok().and_then(|n| n.parse().ok()).unwrap_or(40);
        let plans: Plans = Default::default();
        let (mut n, t0) = (0, Instant::now());
        let mut slowest = (Duration::ZERO, String::new());
        for (now, reports) in events.into_iter().take(limit) {
            for r in reports.iter().filter(|r| now - r.at_ms < STALE_REPORT_MS) {
                let prev = previous(r);
                let service = services.get(r.lookup()).and_then(|runs| run_of(runs, r.date.as_deref()));
                let stops = service.map(|s| stops_ahead(s, None, &stations, r.at_ms)).unwrap_or_default();
                let class = service.map(|s| crate::train_class::class_of(s.product_code.as_deref(), s.product.as_deref()));
                let rail = crate::rail::Rail { class };
                let t = Instant::now();
                let sim = frames_in(&plans, r, prev.as_ref(), &stops, now, &rail);
                if std::env::var("NO_TAIL").is_err() {
                    tail_of(&sim.frames, r, f64::from(DEFAULT_CARS) * CAR_M + TAIL_SPARE_M);
                }
                if t.elapsed() > slowest.0 {
                    slowest = (t.elapsed(), r.lookup().to_owned());
                }
                n += 1;
            }
        }
        eprintln!(
            "{n} trains in {:.1} s: {:.1} ms each; slowest {:.0} ms ({})",
            t0.elapsed().as_secs_f64(),
            t0.elapsed().as_secs_f64() * 1000.0 / n as f64,
            slowest.0.as_secs_f64() * 1000.0,
            slowest.1
        );
    }

    /// As `replay_live`, the route of each train at each report, from where
    /// it is drawn: how far its start is from the train, and how far the train
    /// drawn over the next 30 s (until the route is asked for again) is from
    /// it, to `LIVE_DIR/routes.csv`.
    #[test]
    #[ignore]
    fn replay_routes() {
        let dir = std::env::var("LIVE_DIR").unwrap();
        crate::rail::load_tiles_for_tests(&std::env::var("RAIL_TILES").unwrap());
        let stations = parse_stations(&std::fs::read_to_string(format!("{dir}/stations.json")).unwrap()).unwrap();
        let services = parse_services(&std::fs::read_to_string(format!("{dir}/services.json")).unwrap()).unwrap();
        let mut events: Vec<(i64, Vec<Report>)> = std::fs::read_to_string(format!("{dir}/stream.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| {
                let v: Value = serde_json::from_str(l).ok()?;
                Some((v["at"].as_i64()?, parse_positions(v["data"].as_str()?).ok()?))
            })
            .collect();
        events.sort_by_key(|e| e.0);
        let plans: Plans = Default::default();
        let only = std::env::var("REPLAY_ONLY").ok();
        // the nearest a point is to a line of `[lon, lat]` points, metres, and how far along it
        let off = |p: (f64, f64), line: &[[f64; 2]]| -> (f64, usize) {
            line.windows(2)
                .enumerate()
                .map(|(j, w)| {
                    let (q, _) = motion::project(p, (w[0][1], w[0][0]), (w[1][1], w[1][0]));
                    (motion::distance(p, q), j)
                })
                .fold((f64::INFINITY, 0), |a, b| if b.0 < a.0 { b } else { a })
        };
        let mut out = String::from("now,train,v,drawn_lat,drawn_lon,segments,start_lat,start_lon,start_gap,max_off,back,first_m,stops\n");
        for (now, reports) in events {
            for r in reports
                .iter()
                .filter(|r| now - r.at_ms < STALE_REPORT_MS)
                .filter(|r| only.as_deref().is_none_or(|o| o.split(',').any(|o| o == r.lookup())))
            {
                let prev = previous(r);
                let Some(service) = services.get(r.lookup()).and_then(|runs| run_of(runs, r.date.as_deref())) else {
                    continue;
                };
                let stops = stops_ahead(service, None, &stations, r.at_ms);
                let class = crate::train_class::class_of(service.product_code.as_deref(), service.product.as_deref());
                let rail = crate::rail::Rail { class: Some(class) };
                let sim = frames_in(&plans, r, prev.as_ref(), &stops, now, &rail);
                let Some(drawn) = motion::drawn_at(&sim.frames, now) else { continue };
                let route = route_from(service, None, &stations, Some(r.fix()), Some(drawn), now);
                let first: &[[f64; 2]] = route.segments.first().map_or(&[], |s| s.as_slice());
                let start = first.first().map_or((f64::NAN, f64::NAN), |p| (p[1], p[0]));
                let gap = motion::distance(drawn.at, start);
                // the train drawn on over the next 30 s: how far off the route, and whether it
                // is found further back along it than before
                let (mut max_off, mut back, mut was) = (0.0f64, false, 0);
                for dt in (0..=30_000).step_by(2_000) {
                    let Some(d) = motion::drawn_at(&sim.frames, now + dt) else { break };
                    let (o, j) = off(d.at, first);
                    max_off = max_off.max(o);
                    back |= j < was;
                    was = j;
                }
                let first_m: f64 = first.windows(2).map(|w| motion::distance((w[0][1], w[0][0]), (w[1][1], w[1][0]))).sum();
                let names: Vec<&str> = route.stops.iter().map(|s| s.name.as_str()).take(2).collect();
                out += &format!(
                    "{now},{},{:.1},{:.6},{:.6},{},{:.6},{:.6},{gap:.0},{max_off:.0},{back},{first_m:.0},{}\n",
                    r.lookup(),
                    drawn.v,
                    drawn.at.0,
                    drawn.at.1,
                    route.segments.len(),
                    start.0,
                    start.1,
                    names.join("|"),
                );
            }
        }
        std::fs::write(format!("{dir}/routes.csv"), out).unwrap();
    }

    /// As `replay_routes`, as the app goes: each train drawn every half
    /// second from its latest frames, its route asked for every 30 s, to
    /// `LIVE_DIR/timeline.jsonl` (`{train, t, at: [lon, lat]}` and `{train, t,
    /// route: [[lon, lat]…]…}`), for the page's way of drawing it to be tried on.
    #[test]
    #[ignore]
    fn replay_route_timeline() {
        let dir = std::env::var("LIVE_DIR").unwrap();
        crate::rail::load_tiles_for_tests(&std::env::var("RAIL_TILES").unwrap());
        let stations = parse_stations(&std::fs::read_to_string(format!("{dir}/stations.json")).unwrap()).unwrap();
        let services = parse_services(&std::fs::read_to_string(format!("{dir}/services.json")).unwrap()).unwrap();
        let mut events: Vec<(i64, Vec<Report>)> = std::fs::read_to_string(format!("{dir}/stream.jsonl"))
            .unwrap()
            .lines()
            .filter_map(|l| {
                let v: Value = serde_json::from_str(l).ok()?;
                Some((v["at"].as_i64()?, parse_positions(v["data"].as_str()?).ok()?))
            })
            .collect();
        events.sort_by_key(|e| e.0);
        let plans: Plans = Default::default();
        let only = std::env::var("REPLAY_ONLY").ok();
        // each train's frames, and when its route was last asked for
        let mut drawn: HashMap<String, (Vec<motion::Frame>, i64)> = HashMap::new();
        let mut out = String::new();
        let mut last = events.first().map_or(0, |e| e.0);
        for (now, reports) in events {
            for (train, (frames, _)) in &drawn {
                for t in (last..now).step_by(500) {
                    if let Some(d) = motion::drawn_at(frames, t) {
                        out += &format!("{{\"train\":\"{train}\",\"t\":{t},\"at\":[{},{}]}}\n", d.at.1, d.at.0);
                    }
                }
            }
            last = now;
            for r in reports
                .iter()
                .filter(|r| now - r.at_ms < STALE_REPORT_MS)
                .filter(|r| only.as_deref().is_none_or(|o| o.split(',').any(|o| o == r.lookup())))
            {
                let prev = previous(r);
                let Some(service) = services.get(r.lookup()).and_then(|runs| run_of(runs, r.date.as_deref())) else {
                    continue;
                };
                let stops = stops_ahead(service, None, &stations, r.at_ms);
                if !drawn.contains_key(r.lookup()) {
                    // where its timetable has it stop, and when
                    let calls: Vec<String> = calls_ahead(service, None, &stations, r.at_ms)
                        .iter()
                        .filter(|c| c.0.stops)
                        .map(|c| format!("[{},{},\"{}\",{}]", c.2.lon, c.2.lat, c.1.name, c.2.arrive_ms.unwrap_or(0)))
                        .collect();
                    out += &format!("{{\"train\":\"{}\",\"t\":{now},\"calls\":[{}]}}\n", r.lookup(), calls.join(","));
                }
                let class = crate::train_class::class_of(service.product_code.as_deref(), service.product.as_deref());
                let rail = crate::rail::Rail { class: Some(class) };
                let sim = frames_in(&plans, r, prev.as_ref(), &stops, now, &rail);
                let asked = drawn.get(r.lookup()).map_or(i64::MIN / 2, |d| d.1);
                let asked = if now - asked >= 30_000 {
                    let at = motion::drawn_at(&sim.frames, now);
                    let route = route_from(service, None, &stations, Some(r.fix()), at, now);
                    out += &format!(
                        "{{\"train\":\"{}\",\"t\":{now},\"route\":{}}}\n",
                        r.lookup(),
                        serde_json::to_string(&route.segments).unwrap()
                    );
                    now
                } else {
                    asked
                };
                drawn.insert(r.lookup().to_owned(), (sim.frames, asked));
            }
        }
        std::fs::write(format!("{dir}/timeline.jsonl"), out).unwrap();
    }
}
