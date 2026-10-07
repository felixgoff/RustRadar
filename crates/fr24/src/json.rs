//! JSON API: flight list, airport list, playback and search.
//!
//! The flight list, airport list and playback responses are large and loosely
//! structured, so they are returned as [`serde_json::Value`]. See upstream
//! `fr24/types/json.py` for their full shape. Search results are typed.

use serde::{Deserialize, Serialize};

use crate::grpc::flight_id_hex;
use crate::headers::now_s;

/// Parameters to fetch metadata/history of flights for *either* a given
/// aircraft registration or flight number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlightListParams {
    pub query: FlightListQuery,
    /// Page number
    pub page: u32,
    /// Number of results per page - use `100` if authenticated.
    pub limit: u32,
    /// Show flights with ATD before this Unix timestamp. `None` means "now".
    pub timestamp: Option<i64>,
    /// If `true`, do not return scheduled or live flights.
    pub historic_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlightListQuery {
    /// Aircraft registration (e.g. `B-HUJ`)
    Reg(String),
    /// Flight number (e.g. `CX8747`)
    Flight(String),
}

impl FlightListParams {
    pub fn new(query: FlightListQuery) -> Self {
        Self {
            query,
            page: 1,
            limit: 10,
            timestamp: None,
            historic_only: false,
        }
    }

    pub(crate) fn query_pairs(&self) -> Vec<(&'static str, String)> {
        let (fetch_by, value) = match &self.query {
            FlightListQuery::Reg(r) => ("reg", r),
            FlightListQuery::Flight(f) => ("flight", f),
        };
        let mut q = vec![
            ("query", value.clone()),
            ("fetchBy", fetch_by.into()),
            ("page", self.page.to_string()),
            ("limit", self.limit.to_string()),
        ];
        if self.historic_only {
            q.push(("filterBy", "historic".into()));
        }
        q.push((
            "timestamp",
            self.timestamp.unwrap_or_else(now_s).to_string(),
        ));
        q
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AirportListMode {
    Arrivals,
    Departures,
    Ground,
}

impl AirportListMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Arrivals => "arrivals",
            Self::Departures => "departures",
            Self::Ground => "ground",
        }
    }
}

/// Fetch aircraft arriving, departing or on ground at a given airport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AirportListParams {
    /// IATA airport code (e.g. `HKG`)
    pub airport: String,
    pub mode: AirportListMode,
    pub page: u32,
    /// Number of results per page - use `100` if authenticated.
    pub limit: u32,
    /// Show flights with STA before this timestamp. `None` means "now".
    pub timestamp: Option<i64>,
}

impl AirportListParams {
    pub fn new(airport: impl Into<String>, mode: AirportListMode) -> Self {
        Self {
            airport: airport.into(),
            mode,
            page: 1,
            limit: 10,
            timestamp: None,
        }
    }

    pub(crate) fn query_pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("code", self.airport.clone()),
            ("plugin[]", "schedule".into()),
            ("plugin-setting[schedule][mode]", self.mode.as_str().into()),
            ("page", self.page.to_string()),
            ("limit", self.limit.to_string()),
            (
                "plugin-setting[schedule][timestamp]",
                self.timestamp.unwrap_or_else(now_s).to_string(),
            ),
        ]
    }
}

/// Fetch historical track playback data for a given flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackParams {
    pub flight_id: u32,
    /// Actual time of departure (ATD) of the historic flight, Unix seconds.
    /// Optional, but it is recommended to include it.
    pub timestamp: Option<i64>,
}

impl PlaybackParams {
    pub(crate) fn query_pairs(&self) -> Vec<(&'static str, String)> {
        let mut q = vec![("flightId", flight_id_hex(self.flight_id))];
        if let Some(ts) = self.timestamp {
            q.push(("timestamp", ts.to_string()));
        }
        q
    }
}

/// General search: airports, operators, live flights, schedules (`HKG-CDG`)
/// or aircraft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindParams {
    pub query: String,
    pub limit: u32,
}

impl FindParams {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            limit: 50,
        }
    }

    pub(crate) fn query_pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("query", self.query.clone()),
            ("limit", self.limit.to_string()),
        ]
    }
}

//
// find response
//

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Find {
    pub results: Vec<FindEntry>,
    #[serde(default)]
    pub stats: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "RawFindEntry")]
pub struct FindEntry {
    pub id: String,
    pub label: String,
    pub name: Option<String>,
    /// How the query matched, e.g. `begins`, `iata`, `route`.
    #[serde(rename = "match")]
    pub match_kind: Option<String>,
    #[serde(flatten)]
    pub detail: FindDetail,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "detail", rename_all = "snake_case")]
pub enum FindDetail {
    Airport(FindAirportDetail),
    Operator(FindOperatorDetail),
    Live(FindLiveDetail),
    Schedule(FindScheduleDetail),
    Aircraft(FindAircraftDetail),
    /// An entry type that is not known to this library.
    Unknown {
        kind: String,
        detail: serde_json::Value,
    },
}

impl FindDetail {
    fn from_parts(kind: String, detail: serde_json::Value) -> Self {
        fn parse<T: serde::de::DeserializeOwned>(v: &serde_json::Value) -> Option<T> {
            serde_json::from_value(v.clone()).ok()
        }
        let parsed = match kind.as_str() {
            "airport" => parse(&detail).map(Self::Airport),
            "operator" => parse(&detail).map(Self::Operator),
            "live" => parse(&detail).map(Self::Live),
            "schedule" => parse(&detail).map(Self::Schedule),
            "aircraft" => parse(&detail).map(Self::Aircraft),
            _ => None,
        };
        parsed.unwrap_or(Self::Unknown { kind, detail })
    }
}

#[derive(Deserialize)]
struct RawFindEntry {
    #[serde(default)]
    id: String,
    #[serde(default)]
    label: String,
    name: Option<String>,
    #[serde(rename = "match")]
    match_kind: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    detail: serde_json::Value,
}

impl From<RawFindEntry> for FindEntry {
    fn from(raw: RawFindEntry) -> Self {
        Self {
            id: raw.id,
            label: raw.label,
            name: raw.name,
            match_kind: raw.match_kind,
            detail: FindDetail::from_parts(raw.kind, raw.detail),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindAirportDetail {
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub size: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindOperatorDetail {
    pub operator_id: Option<i64>,
    pub iata: Option<String>,
    pub logo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindLiveDetail {
    pub lat: f64,
    pub lon: f64,
    pub schd_from: Option<String>,
    pub schd_to: Option<String>,
    pub ac_type: Option<String>,
    pub route: Option<String>,
    pub logo: Option<String>,
    pub reg: Option<String>,
    pub callsign: Option<String>,
    pub flight: Option<String>,
    pub operator: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindScheduleDetail {
    pub flight: Option<String>,
    pub logo: Option<String>,
    pub callsign: Option<String>,
    pub operator: Option<String>,
    pub operator_id: Option<i64>,
    pub schd_from: Option<String>,
    pub schd_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FindAircraftDetail {
    /// Owner ICAO code
    pub owner: Option<String>,
    /// Aircraft type
    pub equip: Option<String>,
    pub hex: Option<String>,
    pub operator_id: Option<i64>,
    pub logo: Option<String>,
}

//
// flight list rows
//

/// One flight of a [`FlightListParams`] response, flattened.
///
/// Port of upstream `flight_list_dict`, with the airports' IATA codes and the
/// status colour added. Times are Unix seconds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FlightListRecord {
    pub flight_id: Option<u32>,
    pub number: Option<String>,
    pub callsign: Option<String>,
    pub icao24: Option<u32>,
    pub registration: Option<String>,
    pub typecode: Option<String>,
    pub origin: Option<String>,
    pub origin_iata: Option<String>,
    pub destination: Option<String>,
    pub destination_iata: Option<String>,
    pub status: Option<String>,
    /// e.g. `green`, `yellow`, `red`, `gray`.
    pub status_color: Option<String>,
    pub live: bool,
    pub stod: Option<i64>,
    pub etod: Option<i64>,
    pub atod: Option<i64>,
    pub stoa: Option<i64>,
    pub etoa: Option<i64>,
    pub atoa: Option<i64>,
}

impl FlightListRecord {
    /// Every row in a flight list response (`.result.response.data`).
    pub fn all(response: &serde_json::Value) -> Vec<Self> {
        response["result"]["response"]["data"]
            .as_array()
            .map(|rows| rows.iter().map(Self::from_entry).collect())
            .unwrap_or_default()
    }

    pub fn from_entry(entry: &serde_json::Value) -> Self {
        let text = |v: &serde_json::Value| v.as_str().map(str::to_owned);
        let hex = |v: &serde_json::Value| v.as_str().and_then(|s| u32::from_str_radix(s, 16).ok());
        let time = &entry["time"];
        let airport = |side: &str, code: &str| text(&entry["airport"][side]["code"][code]);
        Self {
            flight_id: hex(&entry["identification"]["id"]),
            number: text(&entry["identification"]["number"]["default"]),
            callsign: text(&entry["identification"]["callsign"]),
            icao24: hex(&entry["aircraft"]["hex"]),
            registration: text(&entry["aircraft"]["registration"]),
            typecode: text(&entry["aircraft"]["model"]["code"]),
            origin: airport("origin", "icao"),
            origin_iata: airport("origin", "iata"),
            destination: airport("destination", "icao"),
            destination_iata: airport("destination", "iata"),
            status: text(&entry["status"]["text"]),
            status_color: text(&entry["status"]["icon"]),
            live: entry["status"]["live"].as_bool().unwrap_or(false),
            stod: time["scheduled"]["departure"].as_i64(),
            etod: time["estimated"]["departure"].as_i64(),
            atod: time["real"]["departure"].as_i64(),
            stoa: time["scheduled"]["arrival"].as_i64(),
            etoa: time["estimated"]["arrival"].as_i64(),
            atoa: time["real"]["arrival"].as_i64(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flight_list_rows() {
        let json = serde_json::json!({"result": {"response": {"data": [{
            "identification": {"id": "41ffa3b1", "number": {"default": "BA890"}, "callsign": "BAW890"},
            "status": {"live": true, "text": "Estimated dep 21:25", "icon": "green"},
            "aircraft": {"model": {"code": "A20N"}, "registration": "G-TTNW", "hex": "4080E0"},
            "airport": {"origin": {"code": {"iata": "LHR", "icao": "EGLL"}}, "destination": null},
            "time": {"scheduled": {"departure": 1791318300, "arrival": 1791336900}, "real": {"departure": null}, "estimated": {}}
        }]}}});
        let rows = FlightListRecord::all(&json);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.flight_id, Some(0x41ffa3b1));
        assert_eq!(r.icao24, Some(0x4080E0));
        assert_eq!(r.origin_iata.as_deref(), Some("LHR"));
        assert_eq!(r.destination, None);
        assert_eq!(r.stod, Some(1791318300));
        assert_eq!(r.atod, None);
        assert!(r.live);
    }

    #[test]
    fn find_entries() {
        let json = r#"{"results":[
            {"id":"CPA","label":"Cathay Pacific (CPA / CX)","detail":{"operator_id":57,"iata":"CX"},"type":"operator","match":"iata","name":"Cathay Pacific"},
            {"id":"41ffc766","label":"CX251 / CPA251 / B77W (B-KPD)","detail":{"lat":22.2,"lon":114.1,"schd_from":"HKG","schd_to":"LHR","ac_type":"B77W","callsign":"CPA251","flight":"CX251"},"type":"live","match":"begins"},
            {"id":"x","label":"y","detail":{"foo":1},"type":"spaceship"}
        ],"stats":{}}"#;
        let find: Find = serde_json::from_str(json).unwrap();
        assert!(matches!(find.results[0].detail, FindDetail::Operator(_)));
        let FindDetail::Live(live) = &find.results[1].detail else {
            panic!()
        };
        assert_eq!(live.callsign.as_deref(), Some("CPA251"));
        assert!(
            matches!(&find.results[2].detail, FindDetail::Unknown { kind, .. } if kind == "spaceship")
        );

        let out = serde_json::to_value(&find.results[1]).unwrap();
        assert_eq!(out["type"], "live");
        assert_eq!(out["detail"]["flight"], "CX251");
    }
}
