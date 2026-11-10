//! `api.ts` airport DTOs (S1.5): reference data, airport boards, runways and taxi lines.
//!
//! Wire names and number types as the backend sends them (`RS:reference.rs`, `RS:board.rs`,
//! `RS:runways.rs`, `RS:taxiways.rs`).

use serde::{Deserialize, Serialize};

/// An airline of the reference data (`api.ts:167`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Airline {
    pub name: String,
    pub iata: String,
    pub icao: String,
}

/// An airport of the reference data (`api.ts:173`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Airport {
    /// Flightradar24's airport id, as in a flight's schedule.
    pub id: u32,
    pub name: String,
    pub iata: String,
    pub icao: String,
    pub city: String,
    pub country: String,
    pub lat: f64,
    pub lon: f64,
    /// Field elevation, feet, where known. Older caches lack it.
    #[serde(default)]
    pub alt: Option<i64>,
    /// A relative size/importance score.
    pub size: i64,
    /// IANA timezone name, e.g. `Europe/London`.
    pub timezone: String,
}

/// The airlines and airports the app knows (`api.ts:190`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceData {
    /// Unix seconds.
    pub fetched_at: i64,
    pub airlines: Vec<Airline>,
    pub airports: Vec<Airport>,
}

/// Which side of an airport board (`api.ts:196`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BoardMode {
    Arrivals,
    Departures,
}

/// One flight on an airport board (`api.ts:199`). The "other airport" is the destination for
/// departures and the origin for arrivals; times are Unix seconds at this airport.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// One page of an airport board (`api.ts:221`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub rows: Vec<BoardRow>,
    pub total: Option<u64>,
    pub page: u64,
    pub pages: Option<u64>,
}

/// One end of a runway, from OurAirports (`api.ts:324`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunwayEnd {
    /// e.g. `22L`
    pub ident: String,
    /// The physical end of the runway; the landing threshold may be displaced from it.
    pub lat: f64,
    pub lon: f64,
    /// Feet, where known.
    #[serde(default)]
    pub elevation_ft: Option<f64>,
    /// Feet from the physical end to the landing threshold.
    #[serde(default)]
    pub displaced_ft: f64,
}

/// A runway (`api.ts:335`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Runway {
    #[serde(default)]
    pub length_ft: Option<f64>,
    #[serde(default)]
    pub width_ft: Option<f64>,
    pub surface: String,
    /// The two ends, low-numbered first.
    pub ends: [RunwayEnd; 2],
}

/// What a [`TaxiLine`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaxiKind {
    Taxiway,
    Taxilane,
    Runway,
}

/// A taxiway, taxilane or runway centreline from OpenStreetMap, one OSM way (`api.ts:347`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaxiLine {
    pub kind: TaxiKind,
    /// Designator where tagged, e.g. `A3` or `04L/22R`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    /// `[lon, lat]` pairs.
    pub points: Vec<[f64; 2]>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn round_trip<T: Serialize + serde::de::DeserializeOwned>(wire: Value) -> T {
        let value: T = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&value).unwrap(), wire);
        value
    }

    #[test]
    fn reference_data_round_trips() {
        let data: ReferenceData = round_trip(json!({
            "fetchedAt": 1_700_000_000,
            "airlines": [{"name": "British Airways", "iata": "BA", "icao": "BAW"}],
            "airports": [{
                "id": 1, "name": "London Heathrow Airport", "iata": "LHR", "icao": "EGLL", "city": "London",
                "country": "United Kingdom", "lat": 51.4706, "lon": -0.461941, "alt": 83, "size": 330_000,
                "timezone": "Europe/London",
            }],
        }));
        assert_eq!(data.airports[0].alt, Some(83));
        assert_eq!(data.airlines[0].icao, "BAW");
    }

    #[test]
    fn airport_of_an_older_cache_has_no_elevation() {
        let airport: Airport = serde_json::from_value(json!({
            "id": 2, "name": "A", "iata": "AAA", "icao": "AAAA", "city": "C", "country": "D",
            "lat": 1.0, "lon": 2.0, "size": 3, "timezone": "UTC",
        }))
        .unwrap();
        assert_eq!(airport.alt, None);
        assert_eq!(serde_json::to_value(&airport).unwrap()["alt"], Value::Null);
    }

    #[test]
    fn landings_fixture_airports_have_the_reference_fields() {
        let landings: Value = serde_json::from_str(include_str!("../../tests/data/landings.json")).unwrap();
        let airports = landings["airports"].as_array().unwrap();
        assert_eq!(airports.len(), 6);
        for a in airports {
            // the fixture keeps `iata, icao, lat, lon, alt`; the rest is filled in
            let mut wire = a.clone();
            for (key, value) in [
                ("id", json!(0)),
                ("name", json!("")),
                ("city", json!("")),
                ("country", json!("")),
                ("size", json!(0)),
                ("timezone", json!("UTC")),
            ] {
                wire[key] = value;
            }
            let airport: Airport = round_trip(wire);
            assert_eq!(airport.icao.len(), 4);
            assert!(airport.alt.is_some());
        }
        assert_eq!(airports[0]["iata"], "AMS");
    }

    #[test]
    fn board_round_trips() {
        let board: Board = round_trip(json!({
            "rows": [{
                "flightId": 900_000_001u32, "number": "BA1", "callsign": "BAW1", "airline": "British Airways",
                "airlineIcao": "BAW", "logo": null, "airportIata": "JFK", "airportName": "New York JFK",
                "airportCity": "New York", "scheduled": 1_700_000_000, "estimated": null,
                "actual": 1_700_000_600, "status": "Departed 09:10", "statusColor": "green", "live": true,
                "aircraft": "A35K", "registration": "G-XWBA", "terminal": "5", "gate": null,
            }],
            "total": 412, "page": 1, "pages": 17,
        }));
        assert_eq!(board.rows[0].airline_icao.as_deref(), Some("BAW"));
        let empty: Board = round_trip(json!({"rows": [], "total": null, "page": 1, "pages": null}));
        assert_eq!(empty, Board { page: 1, ..Default::default() });
    }

    #[test]
    fn board_mode_is_lowercase() {
        assert_eq!(serde_json::to_value(BoardMode::Arrivals).unwrap(), json!("arrivals"));
        assert_eq!(serde_json::from_value::<BoardMode>(json!("departures")).unwrap(), BoardMode::Departures);
    }

    #[test]
    fn runway_round_trips() {
        let runway: Runway = round_trip(json!({
            "lengthFt": 12_802.0, "widthFt": 164.0, "surface": "ASP",
            "ends": [
                {"ident": "09L", "lat": 51.4775, "lon": -0.4850, "elevationFt": 79.0, "displacedFt": 1004.0},
                {"ident": "27R", "lat": 51.4777, "lon": -0.4333, "elevationFt": null, "displacedFt": 0.0},
            ],
        }));
        assert_eq!(runway.ends[0].displaced_ft, 1004.0);
        let sparse: Runway = serde_json::from_value(json!({
            "surface": "", "ends": [{"ident": "04", "lat": 1.0, "lon": 2.0}, {"ident": "22", "lat": 1.1, "lon": 2.1}],
        }))
        .unwrap();
        assert_eq!(sparse.length_ft, None);
        assert_eq!(sparse.ends[1].displaced_ft, 0.0);
    }

    #[test]
    fn taxi_line_leaves_out_a_missing_ref() {
        let line: TaxiLine =
            round_trip(json!({"kind": "taxiway", "ref": "A3", "points": [[4.76, 52.31], [4.77, 52.32]]}));
        assert_eq!(line.r#ref.as_deref(), Some("A3"));
        let bare: TaxiLine = round_trip(json!({"kind": "taxilane", "points": []}));
        assert_eq!(bare.r#ref, None);
        let runway: TaxiLine = round_trip(json!({"kind": "runway", "ref": "04L/22R", "points": [[1.0, 2.0]]}));
        assert_eq!(runway.kind, TaxiKind::Runway);
    }
}
