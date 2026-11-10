//! `api.ts` flight DTOs (S1.5): live flights, a flight's details, search results, an aircraft's
//! history, livery colours and the sign-in state.
//!
//! The wire names and the number types are those the backend sends today (`RS:lib.rs`,
//! `crates/fr24/src/{grpc,json}.rs`), so a value serialises to the same JSON as the struct it
//! replaces. Where the backend's record carries more than `api.ts` reads (`squawk` of a top
//! flight, `ems`, `position_buffer`, `icao24`), the field is kept and defaults when absent.

use serde::{Deserialize, Serialize};

/// A compact view of a live flight, sent for every aircraft on the globe (`api.ts:5`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveFlight {
    pub id: u32,
    pub lat: f32,
    pub lon: f32,
    /// Degrees clockwise from north
    pub track: i32,
    /// Feet
    pub alt: i32,
    /// Knots
    pub speed: i32,
    pub on_ground: bool,
    pub timestamp_ms: u64,
    pub callsign: String,
    pub flight: String,
    pub reg: String,
    pub typecode: String,
    pub origin: String,
    pub destination: String,
    /// Flightradar24's icon class, e.g. `A320`, `B747`, `EC` (helicopter).
    pub icon: String,
    /// The aircraft's recent history (Flightradar24's position buffer):
    /// `[Δlat, Δlon, Δms]` back from the report, each point at
    /// `lat - Δlat·1e-5`, `lon - Δlon·1e-5` and `timestamp_ms - Δms`, newest first
    /// (Δms increasing). Up to ~10 s, overlapping the previous reports' positions.
    /// Optional in `api.ts`; the backend always sends it.
    #[serde(default)]
    pub positions: Vec<[i32; 3]>,
    /// Feet per minute; only when signed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vspeed: Option<i32>,
}

/// One answer of `live_flights` (`api.ts:36`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSnapshot {
    pub flights: Vec<LiveFlight>,
    pub server_time_ms: u64,
    pub requests: usize,
    pub errors: Vec<String>,
    pub elapsed_ms: u64,
    pub authenticated: bool,
}

/// One point of a flight's trail (`api.ts:45`, `fr24` `TrailPointRecord`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TrailPoint {
    /// Unix seconds
    pub timestamp: u64,
    pub latitude: f32,
    pub longitude: f32,
    /// Feet
    pub altitude: i32,
    /// Knots
    pub ground_speed: u32,
    /// Degrees clockwise from north
    pub track: u32,
    /// Feet per minute
    pub vertical_speed: i32,
    pub source: i32,
}

/// A photo of an aircraft (`api.ts:56`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AircraftImage {
    pub url: String,
    pub copyright: String,
    pub thumbnail: String,
    pub medium: String,
    pub large: String,
}

/// A filed flight plan; only sent to Flightradar24 subscribers (`api.ts:65`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FlightPlan {
    /// ICAO code of the departure airport.
    pub departure: String,
    /// ICAO code of the destination airport.
    pub destination: String,
    /// ICAO field 15 route string.
    pub route: String,
    pub length: f64,
    /// Fixed-point `[lat, lon]` pairs of undocumented scale.
    pub waypoints: Vec<[i32; 2]>,
    /// ICAO codes of the alternate airports.
    pub alternates: Vec<String>,
}

/// `FlightDetails.aircraft` (`api.ts:77`, `fr24` `AircraftRecord`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AircraftInfo {
    /// ICAO 24-bit address
    pub icao_address: u32,
    pub reg: String,
    pub typecode: String,
    /// e.g. `Airbus A350-941`
    pub full_description: String,
    pub service: String,
    pub registered_owners: String,
    /// Manufacturer serial number, when published.
    pub msn: Option<String>,
    /// e.g. `2017-06-28`, when published.
    pub birth_date: Option<String>,
    /// Years, when published.
    pub age: Option<u32>,
    pub images: Vec<AircraftImage>,
}

/// `FlightDetails.schedule` (`api.ts:89`, `fr24` `ScheduleRecord`). Times are Unix seconds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleInfo {
    pub flight_number: String,
    /// Flightradar24 id of the operating airline.
    pub operated_by_id: u32,
    /// Flightradar24 id of the airline whose livery the aircraft wears.
    pub painted_as_id: u32,
    pub origin_id: u32,
    pub destination_id: u32,
    pub diverted_id: u32,
    pub scheduled_departure: u32,
    pub scheduled_arrival: u32,
    pub actual_departure: u32,
    pub actual_arrival: u32,
    pub arr_terminal: String,
    pub arr_gate: String,
}

/// `FlightDetails.progress` (`api.ts:103`, `fr24` `FlightProgressRecord`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlightProgress {
    /// Metres
    pub traversed_distance: u32,
    /// Metres
    pub remaining_distance: u32,
    /// Seconds
    pub elapsed_time: u32,
    /// Seconds
    pub remaining_time: u32,
    /// Unix seconds
    pub eta: u32,
    /// Metres
    pub great_circle_distance: u32,
    /// Seconds
    pub mean_flight_time: u32,
    pub flight_stage: String,
    pub delay_status: String,
    pub progress_pct: u32,
}

/// Enhanced Mode-S data (`fr24` `EmsRecord`); `api.ts` does not read it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmsInfo {
    pub ias: i32,
    pub tas: i32,
    pub mach: i32,
    pub mcp: i32,
    pub fms: i32,
    pub oat: i32,
    pub qnh: i32,
    pub wind_dir: i32,
    pub wind_speed: i32,
    pub altitude_gps: i32,
    pub agpsdiff: i32,
    pub apflags: i32,
    pub rs: i32,
}

/// `FlightDetails.flight` (`api.ts:115`, `fr24` `FlightInfoRecord`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FlightInfo {
    /// Unix milliseconds
    pub timestamp_ms: u64,
    pub flightid: u32,
    pub latitude: f32,
    pub longitude: f32,
    pub track: i32,
    /// Feet
    pub altitude: i32,
    /// Knots
    pub ground_speed: i32,
    /// Feet per minute
    pub vertical_speed: i32,
    pub on_ground: bool,
    pub callsign: String,
    pub squawk: i32,
    pub source: String,
    /// Free-form, e.g. `Shannon UIR`
    pub airspace: String,
    #[serde(default)]
    pub ems: EmsInfo,
}

/// One entry of a flight's position buffer (`fr24` `RecentPositionRecord`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentPosition {
    /// Offset from the current latitude, 1e5 degrees
    pub delta_lat: i32,
    /// Offset from the current longitude, 1e5 degrees
    pub delta_lon: i32,
    /// Delta time, milliseconds
    pub delta_ms: u32,
}

/// Details of a flight (`api.ts:76`, `fr24` `FlightDetailsRecord`).
///
/// Sections are `None` when absent from the message: later messages of a followed flight only
/// contain `schedule`, `progress` and `flight`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FlightDetails {
    pub aircraft: Option<AircraftInfo>,
    pub schedule: Option<ScheduleInfo>,
    pub progress: Option<FlightProgress>,
    pub flight: Option<FlightInfo>,
    pub flight_plan: Option<FlightPlan>,
    pub trail: Vec<TrailPoint>,
    #[serde(default)]
    pub position_buffer: Vec<RecentPosition>,
}

/// What a followed flight streams (`api.ts:134`, `RS:lib.rs:261`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
pub enum FollowEvent {
    /// The latest merged details, including the full trail.
    Details(Box<FlightDetails>),
    /// A subscription failed; it will be retried.
    Error(String),
    /// Following stopped, e.g. because the flight is no longer live.
    Ended(String),
}

/// One of the most tracked flights (`api.ts:139`, `fr24` `TopFlightRecord`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopFlight {
    pub flight_id: u32,
    pub live_clicks: u32,
    pub total_clicks: u32,
    pub flight_number: String,
    pub callsign: String,
    #[serde(default)]
    pub squawk: u32,
    pub from_iata: String,
    pub from_city: String,
    pub to_iata: String,
    pub to_city: String,
    #[serde(rename = "type")]
    pub typecode: String,
    pub full_description: String,
}

/// One search result (`api.ts:160`, `fr24` `FindEntry`): `{id, label, name, match, type, detail}`.
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

/// The `type` and `detail` of a [`FindEntry`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "detail", rename_all = "snake_case")]
pub enum FindDetail {
    Airport(FindAirportDetail),
    Operator(FindOperatorDetail),
    Live(FindLiveDetail),
    Schedule(FindScheduleDetail),
    Aircraft(FindAircraftDetail),
    /// An entry type that is not known, or a known one whose detail does not parse.
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

/// One past or scheduled flight of an aircraft; times are Unix seconds (`api.ts:229`, `fr24`
/// `FlightListRecord`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HistoryRow {
    pub flight_id: Option<u32>,
    pub number: Option<String>,
    pub callsign: Option<String>,
    #[serde(default)]
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

/// Livery colours derived from an airline's logo, RGB (`api.ts:250`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogoColors {
    pub primary: [u8; 3],
    pub secondary: Option<[u8; 3]>,
}

/// The Flightradar24 sign-in state (`api.ts:255`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub authenticated: bool,
    /// Unix seconds when the token expires, if known.
    pub expires: Option<i64>,
}

/// An area in degrees; `west > east` wraps the antimeridian (`api.ts:262`, `fr24` `BoundingBox`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct BoundingBox {
    /// Latitude, minimum, degrees
    pub south: f32,
    /// Latitude, maximum, degrees
    pub north: f32,
    /// Longitude, minimum, degrees
    pub west: f32,
    /// Longitude, maximum, degrees
    pub east: f32,
}

/// Flightradar24 answers gRPC status 8 (resource exhausted) when polled too hard
/// (`api.ts:277`: `/status 8|too many requests/i`).
pub fn is_rate_limited(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("status 8") || lower.contains("too many requests")
}

/// Flightradar24's logo for an airline id, such as a schedule's `painted_as_id` (`api.ts:597`).
pub fn airline_logo_url(airline_id: u32) -> String {
    format!("https://cdn.flightradar24.com/assets/airlines/logotypes/{airline_id}.png")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// Deserialises `wire` into `T` and checks that it serialises back to the same JSON.
    fn round_trip<T: Serialize + serde::de::DeserializeOwned>(wire: Value) -> T {
        let value: T = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&value).unwrap(), wire);
        value
    }

    #[derive(Deserialize)]
    struct Landings {
        flights: Vec<LandingFlight>,
    }

    #[derive(Deserialize)]
    struct LandingFlight {
        callsign: String,
        id: u32,
        typecode: String,
        origin: String,
        destination: String,
        reports: Vec<LandingReport>,
    }

    /// `[timestampMs, lat, lon, alt, speed, track, onGround 0/1, positions]`
    type LandingReport = (u64, f32, f32, i32, i32, i32, u8, Vec<[i32; 3]>);

    #[test]
    fn landings_fixture_deserialises_into_live_flights() {
        let landings: Landings = serde_json::from_str(include_str!("../../tests/data/landings.json")).unwrap();
        assert_eq!(landings.flights.len(), 9);
        let mut reports = 0;
        for f in &landings.flights {
            assert!(!f.reports.is_empty());
            for (timestamp_ms, lat, lon, alt, speed, track, on_ground, positions) in &f.reports {
                // the report as `live_flights` sends it
                let wire = json!({
                    "id": f.id, "lat": lat, "lon": lon, "track": track, "alt": alt, "speed": speed,
                    "onGround": *on_ground == 1, "timestampMs": timestamp_ms, "callsign": f.callsign,
                    "flight": "", "reg": "", "typecode": f.typecode, "origin": f.origin,
                    "destination": f.destination, "icon": "", "positions": positions,
                });
                let flight: LiveFlight = round_trip(wire);
                assert_eq!(flight.timestamp_ms, *timestamp_ms);
                assert_eq!(flight.positions, *positions);
                assert_eq!(flight.vspeed, None);
                reports += 1;
            }
        }
        assert!(reports > 100, "{reports} reports");
        let first = &landings.flights[0];
        assert_eq!(first.callsign, "KLM76F");
        assert_eq!(first.reports[0].0, 1_791_566_548_741);
        assert_eq!(first.reports[0].7[0], [-4, -219, 2080]);
    }

    #[test]
    fn live_flight_takes_what_api_ts_marks_optional() {
        let flight: LiveFlight = serde_json::from_value(json!({
            "id": 7, "lat": 51.5, "lon": -0.25, "track": 270, "alt": 36000, "speed": 450,
            "onGround": false, "timestampMs": 1_700_000_000_000u64, "callsign": "BAW1", "flight": "BA1",
            "reg": "G-XWBA", "typecode": "A35K", "origin": "LHR", "destination": "JFK", "icon": "A350",
        }))
        .unwrap();
        assert!(flight.positions.is_empty());
        assert_eq!(flight.vspeed, None);
        let signed_in: LiveFlight = round_trip(json!({
            "id": 7, "lat": 51.5, "lon": -0.25, "track": 270, "alt": 36000, "speed": 450,
            "onGround": false, "timestampMs": 1_700_000_000_000u64, "callsign": "BAW1", "flight": "BA1",
            "reg": "G-XWBA", "typecode": "A35K", "origin": "LHR", "destination": "JFK", "icon": "A350",
            "positions": [[1, 2, 900]], "vspeed": -640,
        }));
        assert_eq!(signed_in.vspeed, Some(-640));
    }

    #[test]
    fn live_snapshot_round_trips() {
        let snapshot: LiveSnapshot = round_trip(json!({
            "flights": [], "serverTimeMs": 1_700_000_000_123u64, "requests": 4,
            "errors": ["status 8"], "elapsedMs": 812, "authenticated": true,
        }));
        assert_eq!(snapshot.requests, 4);
    }

    fn details_wire() -> Value {
        json!({
            "aircraft": {
                "icao_address": 4_221_787, "reg": "G-XWBA", "typecode": "A35K",
                "full_description": "Airbus A350-1041", "service": "PASSENGER",
                "registered_owners": "British Airways", "msn": "326", "birth_date": null, "age": 6,
                "images": [{"url": "u", "copyright": "c", "thumbnail": "t", "medium": "m", "large": "l"}],
            },
            "schedule": {
                "flight_number": "BA1", "operated_by_id": 81, "painted_as_id": 81, "origin_id": 1,
                "destination_id": 2, "diverted_id": 0, "scheduled_departure": 1_700_000_000,
                "scheduled_arrival": 1_700_030_000, "actual_departure": 1_700_000_600,
                "actual_arrival": 0, "arr_terminal": "7", "arr_gate": "",
            },
            "progress": {
                "traversed_distance": 1_000_000, "remaining_distance": 4_500_000, "elapsed_time": 4000,
                "remaining_time": 20_000, "eta": 1_700_029_000, "great_circle_distance": 5_540_000,
                "mean_flight_time": 25_000, "flight_stage": "AIRBORNE", "delay_status": "GREEN",
                "progress_pct": 18,
            },
            "flight": {
                "timestamp_ms": 1_700_004_600_000u64, "flightid": 900_000_001, "latitude": 53.25,
                "longitude": -8.5, "track": 281, "altitude": 38000, "ground_speed": 470,
                "vertical_speed": 0, "on_ground": false, "callsign": "BAW1", "squawk": 2201,
                "source": "ADSB", "airspace": "Shannon UIR",
                "ems": {
                    "ias": 270, "tas": 480, "mach": 840, "mcp": 38000, "fms": 38000, "oat": -56,
                    "qnh": 1013, "wind_dir": 250, "wind_speed": 60, "altitude_gps": 38500,
                    "agpsdiff": 500, "apflags": 3, "rs": 0,
                },
            },
            "flight_plan": {
                "departure": "EGLL", "destination": "KJFK", "route": "N0480F380 CPT UL9 KENET",
                "length": 5601.5, "waypoints": [[514775, -4614], [406398, -737789]],
                "alternates": ["KEWR"],
            },
            "trail": [{
                "timestamp": 1_700_004_590u64, "latitude": 53.25, "longitude": -8.5, "altitude": 38000,
                "ground_speed": 470, "track": 281, "vertical_speed": 0, "source": 0,
            }],
            "position_buffer": [{"delta_lat": 3, "delta_lon": -41, "delta_ms": 1900}],
        })
    }

    #[test]
    fn flight_details_round_trip() {
        let details: FlightDetails = round_trip(details_wire());
        assert_eq!(details.aircraft.as_ref().unwrap().msn.as_deref(), Some("326"));
        assert_eq!(details.aircraft.as_ref().unwrap().birth_date, None);
        assert_eq!(details.flight_plan.as_ref().unwrap().waypoints[1], [406_398, -737_789]);
        assert_eq!(details.trail[0].ground_speed, 470);
    }

    #[test]
    fn flight_details_as_api_ts_types_them() {
        // no `ems`, no `position_buffer`, every section absent
        let details: FlightDetails = serde_json::from_value(json!({
            "aircraft": null, "schedule": null, "progress": null,
            "flight": {
                "timestamp_ms": 1, "flightid": 2, "latitude": 1.5, "longitude": 2.5, "track": 3,
                "altitude": 4, "ground_speed": 5, "vertical_speed": 6, "on_ground": true,
                "callsign": "X", "squawk": 7000, "source": "MLAT", "airspace": "",
            },
            "flight_plan": null, "trail": [],
        }))
        .unwrap();
        assert_eq!(details.flight.unwrap().ems, EmsInfo::default());
        assert!(details.position_buffer.is_empty());
        assert_eq!(details.aircraft, None);
    }

    #[test]
    fn follow_event_is_tagged_with_event_and_data() {
        let details: FollowEvent = round_trip(json!({"event": "details", "data": details_wire()}));
        assert!(matches!(details, FollowEvent::Details(d) if d.schedule.is_some()));
        assert_eq!(
            round_trip::<FollowEvent>(json!({"event": "error", "data": "status 14"})),
            FollowEvent::Error("status 14".to_owned())
        );
        assert_eq!(
            round_trip::<FollowEvent>(json!({"event": "ended", "data": "landed"})),
            FollowEvent::Ended("landed".to_owned())
        );
    }

    #[test]
    fn top_flight_names_its_type_field_type() {
        let top: TopFlight = round_trip(json!({
            "flight_id": 1, "live_clicks": 2, "total_clicks": 3, "flight_number": "QF1",
            "callsign": "QFA1", "squawk": 1234, "from_iata": "SYD", "from_city": "Sydney",
            "to_iata": "LHR", "to_city": "London", "type": "A388", "full_description": "Airbus A380-842",
        }));
        assert_eq!(top.typecode, "A388");
    }

    #[test]
    fn find_entries_by_type() {
        let airport: FindEntry = round_trip(json!({
            "id": "LHR", "label": "London Heathrow Airport (LHR / EGLL)", "name": "London Heathrow Airport",
            "match": "iata", "type": "airport", "detail": {"lat": 51.4706, "lon": -0.461941, "size": 330_000.0},
        }));
        assert!(matches!(airport.detail, FindDetail::Airport(ref d) if d.size == Some(330_000.0)));
        assert_eq!(airport.match_kind.as_deref(), Some("iata"));

        let live: FindEntry = round_trip(json!({
            "id": "3a1b2c3d", "label": "BA1 / BAW1", "name": null, "match": "begins", "type": "live",
            "detail": {
                "lat": 51.0, "lon": -1.0, "schd_from": "LHR", "schd_to": "JFK", "ac_type": "A35K",
                "route": "London (LHR) ⟶ New York (JFK)", "logo": null, "reg": "G-XWBA",
                "callsign": "BAW1", "flight": "BA1", "operator": "BAW",
            },
        }));
        assert!(matches!(live.detail, FindDetail::Live(ref d) if d.callsign.as_deref() == Some("BAW1")));

        let operator: FindEntry = round_trip(json!({
            "id": "BAW", "label": "British Airways", "name": "British Airways", "match": "begins",
            "type": "operator", "detail": {"operator_id": 81, "iata": "BA", "logo": null},
        }));
        assert!(matches!(operator.detail, FindDetail::Operator(_)));

        let schedule: FindEntry = serde_json::from_value(json!({
            "id": "BA1", "label": "BA1", "type": "schedule",
            "detail": {"flight": "BA1", "logo": null, "callsign": "BAW1", "operator": "BAW", "operator_id": 81},
        }))
        .unwrap();
        assert!(matches!(schedule.detail, FindDetail::Schedule(ref d) if d.schd_from.is_none()));

        let aircraft: FindEntry = serde_json::from_value(json!({
            "id": "G-XWBA", "label": "G-XWBA", "type": "aircraft",
            "detail": {"owner": "BAW", "equip": "A35K", "hex": "406B5B"},
        }))
        .unwrap();
        assert!(matches!(aircraft.detail, FindDetail::Aircraft(ref d) if d.operator_id.is_none()));
    }

    #[test]
    fn find_entries_of_an_unknown_type_keep_their_detail() {
        let entry: FindEntry =
            serde_json::from_value(json!({"id": "x", "label": "y", "type": "blimp", "detail": {"a": 1}})).unwrap();
        assert_eq!(entry.detail, FindDetail::Unknown { kind: "blimp".to_owned(), detail: json!({"a": 1}) });
        // what the app receives for it
        assert_eq!(
            serde_json::to_value(&entry).unwrap(),
            json!({
                "id": "x", "label": "y", "name": null, "match": null,
                "type": "unknown", "detail": {"kind": "blimp", "detail": {"a": 1}},
            })
        );
        // a known type whose detail does not parse is unknown too
        let broken: FindEntry =
            serde_json::from_value(json!({"type": "airport", "detail": {"lat": "north"}})).unwrap();
        assert!(matches!(broken.detail, FindDetail::Unknown { ref kind, .. } if kind == "airport"));
        assert_eq!(broken.id, "");
    }

    #[test]
    fn history_row_round_trips() {
        let row: HistoryRow = round_trip(json!({
            "flight_id": 900_000_001u32, "number": "BA1", "callsign": "BAW1", "icao24": 4_221_787,
            "registration": "G-XWBA", "typecode": "A35K", "origin": "EGLL", "origin_iata": "LHR",
            "destination": "KJFK", "destination_iata": "JFK", "status": "Landed 14:02",
            "status_color": "green", "live": false, "stod": 1_700_000_000, "etod": null,
            "atod": 1_700_000_600, "stoa": 1_700_030_000, "etoa": null, "atoa": 1_700_029_500,
        }));
        assert_eq!(row.atoa, Some(1_700_029_500));
        // `api.ts` has no `icao24`
        let bare: HistoryRow = serde_json::from_value(json!({
            "flight_id": null, "number": null, "callsign": null, "registration": null, "typecode": null,
            "origin": null, "origin_iata": null, "destination": null, "destination_iata": null,
            "status": null, "status_color": null, "live": true, "stod": null, "etod": null,
            "atod": null, "stoa": null, "etoa": null, "atoa": null,
        }))
        .unwrap();
        assert_eq!(bare, HistoryRow { live: true, ..Default::default() });
    }

    #[test]
    fn logo_colors_session_and_bounding_box_round_trip() {
        let colors: LogoColors = round_trip(json!({"primary": [1, 2, 3], "secondary": null}));
        assert_eq!(colors.primary, [1, 2, 3]);
        let two: LogoColors = round_trip(json!({"primary": [255, 0, 0], "secondary": [0, 0, 255]}));
        assert_eq!(two.secondary, Some([0, 0, 255]));
        let session: SessionInfo = round_trip(json!({"authenticated": true, "expires": 1_700_000_000}));
        assert_eq!(session.expires, Some(1_700_000_000));
        let area: BoundingBox = round_trip(json!({"south": -10.5, "north": 20.25, "west": 170.0, "east": -170.0}));
        assert!(area.west > area.east);
    }

    #[test]
    fn is_rate_limited_matches_status_8_and_too_many_requests() {
        assert!(is_rate_limited("grpc error: status 8 (resource exhausted)"));
        assert!(is_rate_limited("STATUS 8"));
        assert!(is_rate_limited("429 Too Many Requests"));
        assert!(!is_rate_limited("status 14"));
        assert!(!is_rate_limited("status: 8"));
        assert!(!is_rate_limited(""));
    }

    #[test]
    fn airline_logo_url_by_id() {
        assert_eq!(airline_logo_url(81), "https://cdn.flightradar24.com/assets/airlines/logotypes/81.png");
    }
}
