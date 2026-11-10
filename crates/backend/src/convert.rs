//! Flightradar24's records as `rustradar_core::model` types. The backend's requests answer with
//! the records as `fr24` has them (`flight_details`, `top_flights`, `search`, `aircraft_history`,
//! doc 07 §10.2); these turn them into the app's types, field for field, with the same JSON.
//!
//! Functions rather than `From` impls: both sides belong to other crates (`fr24` and
//! `rustradar-core`), and the orphan rule allows such an impl in neither of them but those two.
//!
//! Every other type of `api.ts` the backend sends is `rustradar_core::model`'s own (S1.4).
//! `tests/data/wire.json` holds what the backend sent for each of them before its structs were
//! replaced; the tests here check that the core types send the same.

use fr24::grpc::{
    AircraftImageRecord, AircraftRecord, BoundingBox, EmsRecord, FlightDetailsRecord, FlightInfoRecord,
    FlightPlanRecord, FlightProgressRecord, RecentPositionRecord, ScheduleRecord, TopFlightRecord, TrailPointRecord,
};
use fr24::json::{
    FindAircraftDetail, FindAirportDetail, FindDetail, FindEntry, FindLiveDetail, FindOperatorDetail,
    FindScheduleDetail, FlightListRecord,
};
use rustradar_core::model;

/// A flight's details (`flight_details`, and each message of `follow_flight`).
pub fn flight_details(record: FlightDetailsRecord) -> model::FlightDetails {
    let FlightDetailsRecord { aircraft: a, schedule: s, progress: p, flight: f, flight_plan, trail, position_buffer } =
        record;
    model::FlightDetails {
        aircraft: a.map(aircraft),
        schedule: s.map(schedule),
        progress: p.map(progress),
        flight: f.map(flight_info),
        flight_plan: flight_plan.map(plan),
        trail: trail.into_iter().map(trail_point).collect(),
        position_buffer: position_buffer.into_iter().map(recent_position).collect(),
    }
}

fn aircraft(record: AircraftRecord) -> model::AircraftInfo {
    let AircraftRecord {
        icao_address,
        reg,
        typecode,
        full_description,
        service,
        registered_owners,
        msn,
        birth_date,
        age,
        images,
    } = record;
    model::AircraftInfo {
        icao_address,
        reg,
        typecode,
        full_description,
        service,
        registered_owners,
        msn,
        birth_date,
        age,
        images: images.into_iter().map(image).collect(),
    }
}

fn image(record: AircraftImageRecord) -> model::AircraftImage {
    let AircraftImageRecord { url, copyright, thumbnail, medium, large } = record;
    model::AircraftImage { url, copyright, thumbnail, medium, large }
}

fn schedule(record: ScheduleRecord) -> model::ScheduleInfo {
    let ScheduleRecord {
        flight_number,
        operated_by_id,
        painted_as_id,
        origin_id,
        destination_id,
        diverted_id,
        scheduled_departure,
        scheduled_arrival,
        actual_departure,
        actual_arrival,
        arr_terminal,
        arr_gate,
    } = record;
    model::ScheduleInfo {
        flight_number,
        operated_by_id,
        painted_as_id,
        origin_id,
        destination_id,
        diverted_id,
        scheduled_departure,
        scheduled_arrival,
        actual_departure,
        actual_arrival,
        arr_terminal,
        arr_gate,
    }
}

fn progress(record: FlightProgressRecord) -> model::FlightProgress {
    let FlightProgressRecord {
        traversed_distance,
        remaining_distance,
        elapsed_time,
        remaining_time,
        eta,
        great_circle_distance,
        mean_flight_time,
        flight_stage,
        delay_status,
        progress_pct,
    } = record;
    model::FlightProgress {
        traversed_distance,
        remaining_distance,
        elapsed_time,
        remaining_time,
        eta,
        great_circle_distance,
        mean_flight_time,
        flight_stage,
        delay_status,
        progress_pct,
    }
}

fn flight_info(record: FlightInfoRecord) -> model::FlightInfo {
    let FlightInfoRecord {
        timestamp_ms,
        flightid,
        latitude,
        longitude,
        track,
        altitude,
        ground_speed,
        vertical_speed,
        on_ground,
        callsign,
        squawk,
        source,
        airspace,
        ems: e,
    } = record;
    model::FlightInfo {
        timestamp_ms,
        flightid,
        latitude,
        longitude,
        track,
        altitude,
        ground_speed,
        vertical_speed,
        on_ground,
        callsign,
        squawk,
        source,
        airspace,
        ems: ems(e),
    }
}

fn ems(record: EmsRecord) -> model::EmsInfo {
    let EmsRecord { ias, tas, mach, mcp, fms, oat, qnh, wind_dir, wind_speed, altitude_gps, agpsdiff, apflags, rs } =
        record;
    model::EmsInfo { ias, tas, mach, mcp, fms, oat, qnh, wind_dir, wind_speed, altitude_gps, agpsdiff, apflags, rs }
}

fn plan(record: FlightPlanRecord) -> model::FlightPlan {
    let FlightPlanRecord { departure, destination, route, length, waypoints, alternates } = record;
    model::FlightPlan { departure, destination, route, length, waypoints, alternates }
}

fn trail_point(record: TrailPointRecord) -> model::TrailPoint {
    let TrailPointRecord { timestamp, latitude, longitude, altitude, ground_speed, track, vertical_speed, source } =
        record;
    model::TrailPoint { timestamp, latitude, longitude, altitude, ground_speed, track, vertical_speed, source }
}

fn recent_position(record: RecentPositionRecord) -> model::RecentPosition {
    let RecentPositionRecord { delta_lat, delta_lon, delta_ms } = record;
    model::RecentPosition { delta_lat, delta_lon, delta_ms }
}

/// One of the most tracked flights (`top_flights`).
pub fn top_flight(record: TopFlightRecord) -> model::TopFlight {
    let TopFlightRecord {
        flight_id,
        live_clicks,
        total_clicks,
        flight_number,
        callsign,
        squawk,
        from_iata,
        from_city,
        to_iata,
        to_city,
        typecode,
        full_description,
    } = record;
    model::TopFlight {
        flight_id,
        live_clicks,
        total_clicks,
        flight_number,
        callsign,
        squawk,
        from_iata,
        from_city,
        to_iata,
        to_city,
        typecode,
        full_description,
    }
}

/// One search result (`search`).
pub fn find_entry(entry: FindEntry) -> model::FindEntry {
    let FindEntry { id, label, name, match_kind, detail } = entry;
    model::FindEntry { id, label, name, match_kind, detail: find_detail(detail) }
}

fn find_detail(detail: FindDetail) -> model::FindDetail {
    match detail {
        FindDetail::Airport(FindAirportDetail { lat, lon, size }) => {
            model::FindDetail::Airport(model::FindAirportDetail { lat, lon, size })
        }
        FindDetail::Operator(FindOperatorDetail { operator_id, iata, logo }) => {
            model::FindDetail::Operator(model::FindOperatorDetail { operator_id, iata, logo })
        }
        FindDetail::Live(FindLiveDetail {
            lat,
            lon,
            schd_from,
            schd_to,
            ac_type,
            route,
            logo,
            reg,
            callsign,
            flight,
            operator,
        }) => model::FindDetail::Live(model::FindLiveDetail {
            lat,
            lon,
            schd_from,
            schd_to,
            ac_type,
            route,
            logo,
            reg,
            callsign,
            flight,
            operator,
        }),
        FindDetail::Schedule(FindScheduleDetail { flight, logo, callsign, operator, operator_id, schd_from, schd_to }) => {
            model::FindDetail::Schedule(model::FindScheduleDetail {
                flight,
                logo,
                callsign,
                operator,
                operator_id,
                schd_from,
                schd_to,
            })
        }
        FindDetail::Aircraft(FindAircraftDetail { owner, equip, hex, operator_id, logo }) => {
            model::FindDetail::Aircraft(model::FindAircraftDetail { owner, equip, hex, operator_id, logo })
        }
        FindDetail::Unknown { kind, detail } => model::FindDetail::Unknown { kind, detail },
    }
}

/// One past or scheduled flight of an aircraft (`aircraft_history`).
pub fn history_row(record: FlightListRecord) -> model::HistoryRow {
    let FlightListRecord {
        flight_id,
        number,
        callsign,
        icao24,
        registration,
        typecode,
        origin,
        origin_iata,
        destination,
        destination_iata,
        status,
        status_color,
        live,
        stod,
        etod,
        atod,
        stoa,
        etoa,
        atoa,
    } = record;
    model::HistoryRow {
        flight_id,
        number,
        callsign,
        icao24,
        registration,
        typecode,
        origin,
        origin_iata,
        destination,
        destination_iata,
        status,
        status_color,
        live,
        stod,
        etod,
        atod,
        stoa,
        etoa,
        atoa,
    }
}

/// An area as the app holds it, from `fr24`'s.
pub fn bounding_box(area: BoundingBox) -> model::BoundingBox {
    let BoundingBox { south, north, west, east } = area;
    model::BoundingBox { south, north, west, east }
}

/// An area as `live_flights` and `radar_frame` take it, from the app's.
pub fn fr24_bounding_box(area: model::BoundingBox) -> BoundingBox {
    let model::BoundingBox { south, north, west, east } = area;
    BoundingBox { south, north, west, east }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use serde::Serialize;
    use serde_json::{Value, json};

    use super::*;
    use crate::{FollowEvent, adsblol, board, geocode, radar, runways, ships};

    /// `tests/data/wire.json`: what the backend sent before S1.4, written from its own structs.
    fn fixture() -> Value {
        serde_json::from_str(include_str!("../tests/data/wire.json")).unwrap()
    }

    /// The JSON the page gets: through text, as Tauri sends it (an `f32` reads as its shortest
    /// decimal there, not as the `f64` it widens to in `serde_json::to_value`).
    fn wire<T: Serialize>(value: &T) -> Value {
        serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
    }

    /// Reads every sample of each named type into that type and checks it writes the sample back;
    /// the names checked.
    macro_rules! same_wire {
        ($types:expr, $($name:literal => $ty:ty),* $(,)?) => {{
            let mut checked: Vec<&str> = Vec::new();
            $(
                let samples = $types[$name].as_array().unwrap_or_else(|| panic!("no {} in wire.json", $name));
                assert!(!samples.is_empty(), "no {} samples", $name);
                for sample in samples {
                    let value: $ty = serde_json::from_value(sample.clone()).unwrap_or_else(|e| panic!("{}: {e}", $name));
                    assert_eq!(wire(&value), *sample, "{}", $name);
                }
                checked.push($name);
            )*
            checked
        }};
    }

    #[test]
    fn every_api_type_sends_what_the_backend_sent() {
        let fixture = fixture();
        let types = &fixture["types"];
        let mut checked = same_wire!(types,
            "LiveFlight" => model::LiveFlight,
            "LiveSnapshot" => model::LiveSnapshot,
            "FollowEvent" => model::FollowEvent,
            "TrainBlock" => model::TrainBlock,
            "Structure" => model::Structure,
            "BoardRow" => model::BoardRow,
            "Board" => model::Board,
            "PlaceKind" => model::PlaceKind,
            "Keys" => model::Keys,
            "LogoColors" => model::LogoColors,
            "OpenSkyPoint" => model::OpenSkyPoint,
            "RadarBounds" => model::RadarBounds,
            "RadarVolume" => model::RadarVolume,
            "Airline" => model::Airline,
            "Airport" => model::Airport,
            "ReferenceData" => model::ReferenceData,
            "RunwayEnd" => model::RunwayEnd,
            "Runway" => model::Runway,
            "RunwayData" => runways::RunwayData,
            "RoadPiece" => model::RoadPiece,
            "SessionInfo" => model::SessionInfo,
            "Ship" => model::Ship,
            "NavStatus" => model::NavStatus,
            "ShipDetails" => model::ShipDetails,
            "TaxiKind" => model::TaxiKind,
            "TaxiLine" => model::TaxiLine,
            "StopTime" => model::StopTime,
            "TrainStop" => model::TrainStop,
            "TrafikverketKeyStatus" => model::TrafikverketKeyStatus,
            "RouteStop" => model::RouteStop,
            "TrainRoute" => model::TrainRoute,
            "TrainShape" => model::TrainShape,
            "TrainPhoto" => model::TrainPhoto,
            "TrainMedia" => model::TrainMedia,
            "TrainKind" => model::TrainKind,
            "Sighting" => model::Sighting,
            "Train" => model::Train,
            "TrainEvent" => model::TrainEvent,
            "TrainBlocks" => model::TrainBlocks,
            "Weather" => model::Weather,
        );
        // nothing in the fixture goes unchecked
        let mut written: Vec<&str> = types.as_object().unwrap().keys().map(String::as_str).collect();
        checked.sort_unstable();
        written.sort_unstable();
        assert_eq!(checked, written);
    }

    #[test]
    fn the_fixtures_parse_to_what_they_did() {
        let fixture = fixture();
        let parsed = &fixture["parsed"];
        let frankfurt = adsblol::parse(include_str!("../tests/data/adsblol-frankfurt.json")).unwrap();
        assert_eq!(wire(&frankfurt), parsed["adsblol-frankfurt.json"]["value"]);
        assert_eq!(frankfurt.aircraft.len(), 56);
        for name in ["10115", "bavaria", "damrak-1-amsterdam", "eiffel", "heathrow", "netherlands", "paris"] {
            let body = std::fs::read_to_string(format!("{}/tests/data/photon-{name}.json", env!("CARGO_MANIFEST_DIR")))
                .unwrap();
            let places = geocode::parse(&body).unwrap();
            assert!(!places.is_empty(), "{name}");
            assert_eq!(wire(&places), parsed[format!("photon-{name}.json")]["value"], "{name}");
        }
        for zoom in [3u8, 8, 14] {
            let buf =
                std::fs::read(format!("{}/tests/data/vesselfinder-z{zoom}.bin", env!("CARGO_MANIFEST_DIR"))).unwrap();
            let ships: Vec<ships::Ship> = ships::parse(&buf, zoom).into_iter().take(30).collect();
            assert_eq!(ships.len(), 30);
            assert_eq!(wire(&ships), parsed[format!("vesselfinder-z{zoom}.bin")]["value"], "zoom {zoom}");
        }
    }

    #[test]
    fn a_radar_volume_is_described_as_before() {
        // the volume the fixture's `RadarVolume` was written from
        let dwd = radar::Bounds { west: 1.4, south: 45.6, east: 18.9, north: 56.3 };
        let volume = radar::Volume {
            source: "dwd",
            time: 1_791_566_100,
            fetched: Instant::now(),
            bounds: dwd,
            ground: vec![0; 4],
            levels: vec![(1.0, Some(vec![1])), (1.5, None), (3.0, Some(vec![2])), (12.25, None)],
            coverage: vec![1, 0],
            coverage_size: (900, 1100),
        };
        assert_eq!(wire(&volume.info()), fixture()["types"]["RadarVolume"][0]);
    }

    #[test]
    fn board_modes_are_read_as_before() {
        let arrivals: board::Mode = serde_json::from_value(json!("arrivals")).unwrap();
        let departures: board::Mode = serde_json::from_value(json!("departures")).unwrap();
        assert_eq!(board::api(arrivals), fr24::json::AirportListMode::Arrivals);
        assert_eq!(board::api(departures), fr24::json::AirportListMode::Departures);
        assert!(serde_json::from_value::<board::Mode>(json!("Arrivals")).is_err());
    }

    #[test]
    fn flight_details_convert_field_for_field() {
        let fixture = fixture();
        let records = fixture["fr24"]["FlightDetailsRecord"].as_array().unwrap();
        assert_eq!(records.len(), 2);
        for (i, sample) in records.iter().enumerate() {
            let record: FlightDetailsRecord = serde_json::from_value(sample.clone()).unwrap();
            let details = flight_details(record.clone());
            assert_eq!(serde_json::to_value(&details).unwrap(), serde_json::to_value(&record).unwrap());
            // and a followed flight's stream sends what it did
            let event = FollowEvent::Details(Box::new(details));
            assert_eq!(wire(&event), fixture["types"]["FollowEvent"][i]);
        }
        let full = flight_details(serde_json::from_value(records[0].clone()).unwrap());
        assert_eq!(full.trail.len(), 2);
        assert_eq!(full.flight.unwrap().ems.wind_dir, 250);
        assert_eq!(full.aircraft.unwrap().images[0].copyright, "A. Spotter");
    }

    #[test]
    fn top_flights_convert_field_for_field() {
        let fixture = fixture();
        for sample in fixture["fr24"]["TopFlightRecord"].as_array().unwrap() {
            let record: TopFlightRecord = serde_json::from_value(sample.clone()).unwrap();
            let top = top_flight(record.clone());
            assert_eq!(serde_json::to_value(&top).unwrap(), serde_json::to_value(&record).unwrap());
            assert_eq!(top.squawk, record.squawk);
        }
    }

    #[test]
    fn search_results_convert_field_for_field() {
        let find: fr24::json::Find = serde_json::from_value(fixture()["fr24"]["find"].clone()).unwrap();
        assert_eq!(find.results.len(), 7);
        let entries: Vec<model::FindEntry> = find.results.iter().cloned().map(find_entry).collect();
        for (entry, record) in entries.iter().zip(&find.results) {
            assert_eq!(serde_json::to_value(entry).unwrap(), serde_json::to_value(record).unwrap());
        }
        use model::FindDetail as D;
        assert!(matches!(entries[0].detail, D::Airport(ref d) if d.size == Some(330_000.0)));
        assert!(matches!(entries[1].detail, D::Operator(ref d) if d.operator_id == Some(57)));
        assert!(matches!(entries[2].detail, D::Live(ref d) if d.reg.as_deref() == Some("B-KPD")));
        assert!(matches!(entries[3].detail, D::Schedule(ref d) if d.schd_to.as_deref() == Some("LHR")));
        assert!(matches!(entries[4].detail, D::Aircraft(ref d) if d.hex.as_deref() == Some("780A3B")));
        assert!(matches!(entries[5].detail, D::Unknown { ref kind, .. } if kind == "spaceship"));
        assert!(matches!(entries[6].detail, D::Unknown { ref kind, .. } if kind == "airport"));
    }

    #[test]
    fn aircraft_history_converts_field_for_field() {
        let rows = FlightListRecord::all(&fixture()["fr24"]["flight_list"]);
        assert_eq!(rows.len(), 2);
        for record in &rows {
            let row = history_row(record.clone());
            assert_eq!(serde_json::to_value(&row).unwrap(), serde_json::to_value(record).unwrap());
        }
        let row = history_row(rows[0].clone());
        assert_eq!((row.icao24, row.destination_iata.as_deref(), row.etoa), (Some(0x4080E0), Some("BUD"), Some(1_791_337_200)));
        assert_eq!(history_row(rows[1].clone()), model::HistoryRow::default());
    }

    #[test]
    fn bounding_boxes_convert_both_ways() {
        let area = BoundingBox { south: 47.25, north: 54.1, west: 170.3, east: -170.7 };
        let app = bounding_box(area);
        assert_eq!(serde_json::to_value(app).unwrap(), serde_json::to_value(area).unwrap());
        assert!(app.west > app.east);
        assert_eq!(fr24_bounding_box(app), area);
    }
}
