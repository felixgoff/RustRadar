//! Airport arrival and departure boards, from the JSON airport API.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Arrivals,
    Departures,
}

impl Mode {
    pub fn api(self) -> fr24::json::AirportListMode {
        match self {
            Mode::Arrivals => fr24::json::AirportListMode::Arrivals,
            Mode::Departures => fr24::json::AirportListMode::Departures,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Mode::Arrivals => "arrivals",
            Mode::Departures => "departures",
        }
    }
}

/// One flight on the board. "Other airport" is the destination for
/// departures and the origin for arrivals; times are Unix seconds for the
/// event at this airport.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardRow {
    pub flight_id: Option<u32>,
    pub number: Option<String>,
    pub callsign: Option<String>,
    pub airline: Option<String>,
    pub airline_icao: Option<String>,
    pub logo: Option<String>,
    pub airport_iata: Option<String>,
    pub airport_name: Option<String>,
    pub airport_city: Option<String>,
    pub scheduled: Option<i64>,
    pub estimated: Option<i64>,
    pub actual: Option<i64>,
    pub status: Option<String>,
    /// e.g. `green`, `yellow`, `red`, `gray`.
    pub status_color: Option<String>,
    pub live: bool,
    pub aircraft: Option<String>,
    pub registration: Option<String>,
    pub terminal: Option<String>,
    pub gate: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub rows: Vec<BoardRow>,
    pub total: Option<u64>,
    pub page: u64,
    pub pages: Option<u64>,
}

fn text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

pub fn parse(response: &Value, mode: Mode) -> Board {
    let schedule = &response["result"]["response"]["airport"]["pluginData"]["schedule"][mode.key()];
    let (here, there, event) = match mode {
        Mode::Departures => ("origin", "destination", "departure"),
        Mode::Arrivals => ("destination", "origin", "arrival"),
    };
    let rows = schedule["data"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let f = &row["flight"];
                    let other = &f["airport"][there];
                    let info = &f["airport"][here]["info"];
                    let time = &f["time"];
                    let airline = if f["airline"].is_object() {
                        &f["airline"]
                    } else {
                        &f["owner"]
                    };
                    BoardRow {
                        flight_id: f["identification"]["id"]
                            .as_str()
                            .and_then(|s| u32::from_str_radix(s, 16).ok()),
                        number: text(&f["identification"]["number"]["default"]),
                        callsign: text(&f["identification"]["callsign"]),
                        airline: text(&airline["name"]),
                        airline_icao: text(&airline["code"]["icao"]),
                        logo: text(&f["owner"]["logo"]),
                        airport_iata: text(&other["code"]["iata"]),
                        airport_name: text(&other["name"]),
                        airport_city: text(&other["position"]["region"]["city"]),
                        scheduled: time["scheduled"][event].as_i64(),
                        estimated: time["estimated"][event].as_i64(),
                        actual: time["real"][event].as_i64(),
                        status: text(&f["status"]["text"]),
                        status_color: text(&f["status"]["icon"]),
                        live: f["status"]["live"].as_bool().unwrap_or(false),
                        aircraft: text(&f["aircraft"]["model"]["code"]),
                        registration: text(&f["aircraft"]["registration"]),
                        terminal: text(&info["terminal"]),
                        gate: text(&info["gate"]),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Board {
        rows,
        total: schedule["item"]["total"].as_u64(),
        page: schedule["page"]["current"].as_u64().unwrap_or(1),
        pages: schedule["page"]["total"].as_u64(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn departures() {
        let json = serde_json::json!({"result": {"response": {"airport": {"pluginData": {"schedule": {"departures": {
            "item": {"current": 1, "total": 818},
            "page": {"current": 1, "total": 164},
            "data": [{"flight": {
                "identification": {"id": "41ffa3b1", "number": {"default": "BA890"}, "callsign": "BAW890"},
                "status": {"live": false, "text": "Estimated dep 21:25", "icon": "green"},
                "aircraft": {"model": {"code": "A20N"}, "registration": "G-TTNW"},
                "owner": {"name": "British Airways", "logo": "https://cdn.flightradar24.com/assets/airlines/logotypes/5.png"},
                "airline": {"name": "British Airways", "code": {"iata": "BA", "icao": "BAW"}},
                "airport": {
                    "origin": {"info": {"terminal": "5", "gate": "A12"}},
                    "destination": {"code": {"iata": "TBS"}, "name": "Tbilisi International Airport", "position": {"region": {"city": "Tbilisi"}}}
                },
                "time": {"scheduled": {"departure": 1791318300}, "estimated": {"departure": 1791318900}, "real": {"departure": null}}
            }}]
        }}}}}}});
        let board = parse(&json, Mode::Departures);
        assert_eq!(board.total, Some(818));
        let row = &board.rows[0];
        assert_eq!(row.flight_id, Some(0x41ffa3b1));
        assert_eq!(row.airport_iata.as_deref(), Some("TBS"));
        assert_eq!(row.airport_city.as_deref(), Some("Tbilisi"));
        assert_eq!(row.gate.as_deref(), Some("A12"));
        assert_eq!(row.estimated, Some(1791318900));
        assert_eq!(row.airline_icao.as_deref(), Some("BAW"));
    }
}
