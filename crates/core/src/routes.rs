//! Port of `routes.ts` (S1.16; doc 05 §8.2, doc 03 §15.1): routes for the selected flight, the
//! filed flight plan when Flightradar24 shares it, otherwise an estimate shaped by
//! [`crate::flightpath`]; and the flown track with OpenSky's earlier part in front.
//! The AIRAC cycle arithmetic of `routes.ts` is [`crate::airac`].
//!
//! The TypeScript's `routeFor` read the global `reference`; here the [`Reference`] and the
//! [`Runways`] bookkeeping are passed in, with `now_ms` for the runway download's retry.

use serde::Serialize;

pub use crate::flightpath::{LonLat, PathPoint, RouteKind, RunwayGuess, distance_km, great_circle, trim_to_position};
use crate::flightpath::{EstimateInput, Traffic, estimate_route};
use crate::model::{Airport, FlightDetails, FlightPlan, LiveFlight, OpenSkyPoint, Runway, TrailPoint, f32_as_js};
use crate::num::js_round;
use crate::reference::{Reference, Runways};

/// Flight plan waypoints arrive as integers of undocumented scale (the proto says degrees, which
/// would be far too coarse for waypoints). Pick the scale (1, 1e4, 1e5, 1e6 or 1e7) that puts the
/// route's start nearest the departure airport. Without one, take the smallest divisor that gives
/// valid coordinates: dividing by too much also gives valid ones, squashed toward 0°, 0°. Empty
/// when no scale gives valid coordinates.
pub fn decode_waypoints(plan: &FlightPlan, departure: Option<&Airport>) -> Vec<LonLat> {
    if plan.waypoints.is_empty() {
        return Vec::new();
    }
    let candidates: Vec<Vec<LonLat>> = [1.0, 1e4, 1e5, 1e6, 1e7]
        .into_iter()
        .map(|scale| plan.waypoints.iter().map(|&[lat, lon]| [lon as f64 / scale, lat as f64 / scale]).collect())
        .filter(|points: &Vec<LonLat>| points.iter().all(|[lon, lat]| lat.abs() <= 90.0 && lon.abs() <= 180.0))
        .collect();
    let Some(first) = candidates.first() else {
        return Vec::new();
    };
    let Some(departure) = departure else {
        return first.clone();
    };
    let home: LonLat = [departure.lon, departure.lat];
    // `reduce` without a seed: the first seeds, and ties keep the earlier
    let mut best = first;
    for points in &candidates[1..] {
        if distance_km(points[0], home) < distance_km(best[0], home) {
            best = points;
        }
    }
    best.clone()
}

/// [`merge_trail`]'s answer.
#[derive(Debug, Clone, PartialEq)]
pub struct MergedTrail {
    pub trail: Vec<TrailPoint>,
    /// How many of OpenSky's points were put in front.
    pub added: usize,
}

/// The flown track, with OpenSky's earlier part in front of Flightradar24's when OpenSky saw
/// more of the flight (Flightradar24's own points win where both have them: they are newer and
/// carry speeds). OpenSky's points more than 30 s before Flightradar24's first timed one are put
/// in front when there are at least two; each takes its speed from the move to the next point
/// (knots, rounded, at most 700), a vertical speed of 0 and source -1.
pub fn merge_trail(trail: Vec<TrailPoint>, opensky: &[OpenSkyPoint]) -> MergedTrail {
    // `?? Infinity`: without a timed point, every OpenSky point is earlier
    let first = trail.iter().find(|p| p.timestamp > 0).map(|p| p.timestamp as i64);
    let earlier: Vec<&OpenSkyPoint> =
        opensky.iter().filter(|p| first.is_none_or(|first| p.timestamp < first - 30)).collect();
    if earlier.len() < 2 {
        return MergedTrail { trail, added: 0 };
    }
    let mut merged: Vec<TrailPoint> = earlier
        .iter()
        .enumerate()
        .map(|(i, p)| {
            // OpenSky has no speed: take it from the move to the next point
            let next = match earlier.get(i + 1) {
                Some(n) => Some((n.timestamp, f32_as_js(n.longitude), f32_as_js(n.latitude))),
                None => trail.first().map(|t| (t.timestamp as i64, f32_as_js(t.longitude), f32_as_js(t.latitude))),
            };
            let mut speed = 0.0;
            if let Some((timestamp, lon, lat)) = next
                && timestamp > p.timestamp
            {
                let km = distance_km([f32_as_js(p.longitude), f32_as_js(p.latitude)], [lon, lat]);
                let knots = js_round((km / 1.852 / (timestamp - p.timestamp) as f64) * 3600.0);
                speed = if knots.is_nan() { f64::NAN } else { knots.min(700.0) };
            }
            TrailPoint {
                timestamp: p.timestamp as u64,
                latitude: p.latitude,
                longitude: p.longitude,
                altitude: p.altitude,
                // NaN (from a NaN position) is stored as 0
                ground_speed: speed as u32,
                track: p.track,
                vertical_speed: 0,
                source: -1,
            }
        })
        .collect();
    let added = merged.len();
    merged.extend(trail);
    MergedTrail { trail: merged, added }
}

/// The selected flight's route.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Route {
    pub origin: Option<Airport>,
    pub destination: Option<Airport>,
    /// `Filed`: the subscriber flight plan; `Estimated`: the app's own guess; `None`: no
    /// airport is known.
    pub kind: RouteKind,
    /// The filed route, origin to destination.
    pub filed: Vec<LonLat>,
    /// Estimated: from the take-off roll to where the recorded track begins, with heights (ft MSL).
    pub before: Vec<PathPoint>,
    /// Estimated: from the aircraft to touchdown, with heights (ft MSL).
    pub ahead: Vec<PathPoint>,
    /// The runway `before` departs from, when runways are known.
    pub departure: Option<RunwayGuess>,
    /// The runway `ahead` lands on, when runways are known.
    pub arrival: Option<RunwayGuess>,
}

/// JavaScript truthiness of a number: not 0 and not NaN.
fn truthy(x: f32) -> bool {
    x != 0.0 && !x.is_nan()
}

/// The selected flight's route. `traffic` (the other flights on the globe) is read for which
/// runways are in use at the two airports.
///
/// Origin: the schedule's `origin_id`, else the flight's `origin`, else the plan's departure.
/// Destination: the schedule's `diverted_id` (or `destination_id` when it is 0), else the
/// flight's `destination`, else the plan's destination. More than one decoded waypoint is a
/// `Filed` route; otherwise [`estimate_route`] runs from the first trail point with a position,
/// with each airport's runways as `runways` knows them (none while they load: asking queues the
/// download, as `reference.runways` did).
pub fn route_for(
    flight: &LiveFlight,
    details: Option<&FlightDetails>,
    traffic: &[LiveFlight],
    reference: &Reference,
    runways: &mut Runways,
    now_ms: i64,
) -> Route {
    let schedule = details.and_then(|d| d.schedule.as_ref());
    let plan = details.and_then(|d| d.flight_plan.as_ref());
    let origin = reference
        .airport_by_id(schedule.map(|s| s.origin_id))
        .or_else(|| reference.airport(Some(&flight.origin)))
        .or_else(|| reference.airport(plan.map(|p| p.departure.as_str())));
    let destination = reference
        .airport_by_id(schedule.map(|s| if s.diverted_id != 0 { s.diverted_id } else { s.destination_id }))
        .or_else(|| reference.airport(Some(&flight.destination)))
        .or_else(|| reference.airport(plan.map(|p| p.destination.as_str())));

    let filed = plan.map(|p| decode_waypoints(p, origin)).unwrap_or_default();
    if filed.len() > 1 {
        return Route {
            origin: origin.cloned(),
            destination: destination.cloned(),
            kind: RouteKind::Filed,
            filed,
            before: Vec::new(),
            ahead: Vec::new(),
            departure: None,
            arrival: None,
        };
    }
    let trail_start = details.and_then(|d| d.trail.iter().find(|p| truthy(p.latitude) || truthy(p.longitude)));
    let mut runways_of = |airport: Option<&Airport>| -> Vec<Runway> {
        match airport {
            Some(a) => runways.get(Some(a), now_ms).map(<[Runway]>::to_vec).unwrap_or_default(),
            None => Vec::new(),
        }
    };
    let origin_runways = runways_of(origin);
    let destination_runways = runways_of(destination);
    let traffic: Vec<Traffic> = traffic.iter().map(Traffic::from).collect();
    let estimate = estimate_route(&EstimateInput {
        flight,
        trail_start,
        origin,
        destination,
        origin_runways: &origin_runways,
        destination_runways: &destination_runways,
        traffic: &traffic,
    });
    Route {
        origin: origin.cloned(),
        destination: destination.cloned(),
        filed: Vec::new(),
        kind: estimate.kind,
        before: estimate.before,
        ahead: estimate.ahead,
        departure: estimate.departure,
        arrival: estimate.arrival,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flightpath::{RunwayReason, runway_ends};
    use crate::model::{ReferenceData, RunwayEnd, ScheduleInfo};
    use std::collections::HashMap;

    fn plan(waypoints: &[[i32; 2]]) -> FlightPlan {
        FlightPlan { waypoints: waypoints.to_vec(), ..FlightPlan::default() }
    }

    fn at(lon: f64, lat: f64) -> Airport {
        Airport { lon, lat, ..Airport::default() }
    }

    // Expected values in these tests are the TypeScript's answers for the same inputs.

    #[test]
    fn decode_waypoints_picks_the_scale_nearest_the_departure() {
        let route = [[556_180, 126_560], [596_519, 179_186]];
        // without a departure: the smallest divisor that gives valid coordinates (1e4)
        assert_eq!(decode_waypoints(&plan(&route), None), vec![[12.656, 55.618], [17.9186, 59.6519]]);
        assert_eq!(decode_waypoints(&plan(&route), Some(&at(12.656, 55.618))), vec![[12.656, 55.618], [17.9186, 59.6519]]);
        // a departure near 0°, 0° prefers the squashed reading
        assert_eq!(decode_waypoints(&plan(&route), Some(&at(1.2656, 5.5618))), vec![[1.2656, 5.5618], [1.79186, 5.96519]]);
        let micro = [[55_618_000, 12_656_000], [59_651_900, 17_918_600]];
        assert_eq!(decode_waypoints(&plan(&micro), Some(&at(12.656, 55.618))), vec![[12.656, 55.618], [17.9186, 59.6519]]);
        let south = [[-336_800, 1_511_800], [-338_600, 1_512_100]];
        assert_eq!(decode_waypoints(&plan(&south), Some(&at(151.18, -33.94))), vec![[151.18, -33.68], [151.21, -33.86]]);
    }

    #[test]
    fn decode_waypoints_gives_nothing_for_no_waypoints_or_no_valid_scale() {
        assert!(decode_waypoints(&plan(&[]), Some(&at(12.656, 55.618))).is_empty());
        assert!(decode_waypoints(&plan(&[]), None).is_empty());
        // 2e9 / 1e7 = 200: not a latitude at any scale
        assert!(decode_waypoints(&plan(&[[2_000_000_000, 0]]), Some(&at(12.656, 55.618))).is_empty());
        assert!(decode_waypoints(&plan(&[[2_000_000_000, 0]]), None).is_empty());
    }

    fn trail_point(timestamp: u64, latitude: f32, longitude: f32) -> TrailPoint {
        TrailPoint { timestamp, latitude, longitude, altitude: 30_000, ground_speed: 450, track: 90, vertical_speed: 0, source: 0 }
    }

    fn opensky(timestamp: i64, latitude: f32, longitude: f32) -> OpenSkyPoint {
        OpenSkyPoint { timestamp, latitude, longitude, altitude: 1_000, track: 45, on_ground: false }
    }

    fn added(timestamp: u64, latitude: f32, longitude: f32, ground_speed: u32) -> TrailPoint {
        TrailPoint { timestamp, latitude, longitude, altitude: 1_000, ground_speed, track: 45, vertical_speed: 0, source: -1 }
    }

    fn fr24_trail() -> Vec<TrailPoint> {
        vec![trail_point(1_000_000, 55.9, 13.1), trail_point(1_000_010, 55.91, 13.12)]
    }

    fn opensky_track() -> Vec<OpenSkyPoint> {
        vec![
            opensky(999_900, 55.62, 12.65),
            opensky(999_940, 55.7, 12.8),
            // within 30 s of Flightradar24's first point: left out
            opensky(999_975, 55.8, 12.9),
            opensky(999_990, 55.85, 13.0),
        ]
    }

    #[test]
    fn merge_trail_puts_opensky_earlier_points_in_front() {
        let merged = merge_trail(fr24_trail(), &opensky_track());
        assert_eq!(merged.added, 2);
        let mut expected = vec![added(999_900, 55.62, 12.65, 629), added(999_940, 55.7, 12.8, 700)];
        expected.extend(fr24_trail());
        assert_eq!(merged.trail, expected);
    }

    #[test]
    fn merge_trail_takes_the_speed_from_the_move_to_the_next_point() {
        // an untimed first point is still the next point; its timestamp 0 gives no speed
        let mut trail = vec![trail_point(0, 55.9, 13.1)];
        trail.extend(fr24_trail());
        let merged = merge_trail(trail.clone(), &opensky_track());
        assert_eq!(merged.added, 2);
        let mut expected = vec![added(999_900, 55.62, 12.65, 629), added(999_940, 55.7, 12.8, 0)];
        expected.extend(trail);
        assert_eq!(merged.trail, expected);
        // at most 700 kt
        let fast = merge_trail(fr24_trail(), &[opensky(999_000, 50.0, 10.0), opensky(999_001, 51.0, 10.0)]);
        assert_eq!(fast.trail[..2], [added(999_000, 50.0, 10.0, 700), added(999_001, 51.0, 10.0, 700)]);
        // no trail at all: every point is earlier, and the last has nothing to move to
        let alone = merge_trail(Vec::new(), &[opensky(10, 55.0, 12.0), opensky(70, 55.1, 12.0)]);
        assert_eq!(alone, MergedTrail { trail: vec![added(10, 55.0, 12.0, 360), added(70, 55.1, 12.0, 0)], added: 2 });
    }

    #[test]
    fn merge_trail_needs_two_earlier_points() {
        let one = merge_trail(fr24_trail(), &[opensky(999_960, 55.7, 12.8), opensky(999_975, 55.8, 12.9)]);
        assert_eq!(one, MergedTrail { trail: fr24_trail(), added: 0 });
        assert_eq!(merge_trail(fr24_trail(), &[]), MergedTrail { trail: fr24_trail(), added: 0 });
    }

    fn airport(id: u32, iata: &str, icao: &str, lat: f64, lon: f64, alt: i64) -> Airport {
        Airport { id, iata: iata.into(), icao: icao.into(), name: iata.into(), lat, lon, alt: Some(alt), size: 1, ..Airport::default() }
    }

    fn reference() -> Reference {
        Reference::new(ReferenceData {
            fetched_at: 0,
            airlines: Vec::new(),
            airports: vec![
                airport(1, "CPH", "EKCH", 55.618, 12.656, 17),
                airport(2, "ARN", "ESSA", 59.6519, 17.9186, 137),
                airport(3, "GOT", "ESGG", 57.6628, 12.2798, 506),
            ],
        })
    }

    fn end(ident: &str, lat: f64, lon: f64, elevation_ft: f64) -> RunwayEnd {
        RunwayEnd { ident: ident.into(), lat, lon, elevation_ft: Some(elevation_ft), displaced_ft: 0.0 }
    }

    /// Arlanda's 01L/19R and 08/26.
    fn essa() -> Vec<Runway> {
        vec![
            Runway {
                length_ft: Some(10830.0),
                width_ft: Some(148.0),
                surface: "ASP".into(),
                ends: [end("01L", 59.6372, 17.9133, 120.0), end("19R", 59.6661, 17.9237, 137.0)],
            },
            Runway {
                length_ft: Some(8202.0),
                width_ft: Some(148.0),
                surface: "ASP".into(),
                ends: [end("08", 59.6604, 17.9367, 115.0), end("26", 59.6634, 17.9805, 137.0)],
            },
        ]
    }

    fn flight() -> LiveFlight {
        LiveFlight {
            id: 7,
            lat: 58.9,
            lon: 16.9,
            track: 25,
            alt: 15_000,
            speed: 320,
            origin: "CPH".into(),
            destination: "arn".into(),
            ..LiveFlight::default()
        }
    }

    fn details(schedule: Option<ScheduleInfo>, flight_plan: Option<FlightPlan>, trail: Vec<TrailPoint>) -> FlightDetails {
        FlightDetails { schedule, flight_plan, trail, ..FlightDetails::default() }
    }

    #[test]
    fn route_for_estimates_and_asks_for_the_runways() {
        let reference = reference();
        let mut runways = Runways::new();
        let f = flight();
        let route = route_for(&f, None, &[], &reference, &mut runways, 0);
        assert_eq!(route.origin.as_ref().map(|a| a.icao.as_str()), Some("EKCH"));
        assert_eq!(route.destination.as_ref().map(|a| a.icao.as_str()), Some("ESSA"));
        assert_eq!(route.kind, RouteKind::Estimated);
        assert!(route.filed.is_empty() && route.before.is_empty());
        // the runways are not known yet: straight to the airport, and both are asked for, origin first
        assert_eq!(route.arrival, None);
        let last = route.ahead[route.ahead.len() - 1];
        assert!(distance_km([last[0], last[1]], [17.9186, 59.6519]) < 0.001);
        assert_eq!(runways.take_wanted(), ["EKCH", "ESSA"]);
        // once they have loaded, the estimate lands on a runway
        let found = HashMap::from([("ESSA".to_owned(), essa())]);
        runways.settled("EKCH", Some(&found), 10);
        runways.settled("ESSA", Some(&found), 10);
        let route = route_for(&f, None, &[], &reference, &mut runways, 20);
        let arrival = route.arrival.clone().unwrap();
        assert_eq!(arrival.reason, RunwayReason::Guessed);
        assert!(runways.take_wanted().is_empty());
        // the same as the estimate itself, for the same airports
        let arn = reference.airport(Some("ESSA")).unwrap();
        let estimate = estimate_route(&EstimateInput {
            flight: &f,
            trail_start: None,
            origin: reference.airport(Some("CPH")),
            destination: Some(arn),
            origin_runways: &[],
            destination_runways: &essa(),
            traffic: &[],
        });
        assert_eq!(route.ahead, estimate.ahead);
        assert_eq!(route.arrival, estimate.arrival);
        let ends = runway_ends(&essa(), 137.0);
        assert!(ends.iter().any(|e| e.ident == arrival.runway));
    }

    #[test]
    fn route_for_prefers_the_schedule_then_the_flight_then_the_plan() {
        let reference = reference();
        let mut runways = Runways::new();
        let f = flight();
        let icao = |a: &Option<Airport>| a.as_ref().map(|a| a.icao.clone());
        // the schedule's ids win; a diversion wins over the destination
        let schedule = ScheduleInfo { origin_id: 3, destination_id: 1, diverted_id: 2, ..ScheduleInfo::default() };
        let d = details(Some(schedule.clone()), None, Vec::new());
        let route = route_for(&f, Some(&d), &[], &reference, &mut runways, 0);
        assert_eq!((icao(&route.origin), icao(&route.destination)), (Some("ESGG".into()), Some("ESSA".into())));
        let undiverted = details(Some(ScheduleInfo { diverted_id: 0, ..schedule }), None, Vec::new());
        let route = route_for(&f, Some(&undiverted), &[], &reference, &mut runways, 0);
        assert_eq!(icao(&route.destination), Some("EKCH".into()));
        // unknown ids fall back to the flight's codes, and those to the plan's
        let unknown = ScheduleInfo { origin_id: 99, destination_id: 98, ..ScheduleInfo::default() };
        let filed_codes = FlightPlan { departure: "ESGG".into(), destination: "EKCH".into(), ..FlightPlan::default() };
        let d = details(Some(unknown), Some(filed_codes), Vec::new());
        let route = route_for(&f, Some(&d), &[], &reference, &mut runways, 0);
        assert_eq!((icao(&route.origin), icao(&route.destination)), (Some("EKCH".into()), Some("ESSA".into())));
        let anonymous = LiveFlight { origin: String::new(), destination: "XXX".into(), ..f.clone() };
        let route = route_for(&anonymous, Some(&d), &[], &reference, &mut runways, 0);
        assert_eq!((icao(&route.origin), icao(&route.destination)), (Some("ESGG".into()), Some("EKCH".into())));
        // no airport at all
        let lost = LiveFlight { origin: String::new(), destination: String::new(), ..f };
        let route = route_for(&lost, None, &[], &reference, &mut runways, 0);
        assert_eq!(route.kind, RouteKind::None);
        assert!(route.ahead.is_empty() && route.before.is_empty() && route.origin.is_none());
    }

    #[test]
    fn route_for_shows_a_filed_plan_as_it_is() {
        let reference = reference();
        let mut runways = Runways::new();
        let f = flight();
        let filed_plan = plan(&[[556_180, 126_560], [580_000, 150_000], [596_519, 179_186]]);
        let d = details(None, Some(filed_plan), Vec::new());
        let route = route_for(&f, Some(&d), &[], &reference, &mut runways, 0);
        assert_eq!(route.kind, RouteKind::Filed);
        assert_eq!(route.filed, vec![[12.656, 55.618], [15.0, 58.0], [17.9186, 59.6519]]);
        assert!(route.before.is_empty() && route.ahead.is_empty());
        assert_eq!((route.departure, route.arrival), (None, None));
        // a filed route needs no runways
        assert!(runways.take_wanted().is_empty());
        // a single waypoint is no route: estimated
        let d = details(None, Some(plan(&[[556_180, 126_560]])), Vec::new());
        let route = route_for(&f, Some(&d), &[], &reference, &mut runways, 0);
        assert_eq!(route.kind, RouteKind::Estimated);
        assert!(route.filed.is_empty());
    }

    #[test]
    fn route_for_fills_the_gap_before_the_first_trail_point_with_a_position() {
        let reference = reference();
        let mut runways = Runways::new();
        let f = flight();
        let first = TrailPoint { timestamp: 5, latitude: 55.9, longitude: 13.1, altitude: 11_000, ground_speed: 300, track: 35, ..TrailPoint::default() };
        let d = details(None, None, vec![TrailPoint { timestamp: 1, ..TrailPoint::default() }, first]);
        let route = route_for(&f, Some(&d), &[], &reference, &mut runways, 0);
        let cph = reference.airport(Some("EKCH")).unwrap();
        let estimate = estimate_route(&EstimateInput {
            flight: &f,
            trail_start: Some(&first),
            origin: Some(cph),
            destination: reference.airport(Some("ESSA")),
            origin_runways: &[],
            destination_runways: &[],
            traffic: &[],
        });
        assert!(!route.before.is_empty());
        assert_eq!(route.before, estimate.before);
        assert_eq!(route.before[route.before.len() - 1][2], 11_000.0);
    }

    #[test]
    fn route_for_reads_the_runway_in_use_from_the_traffic() {
        let reference = reference();
        let mut runways = Runways::new();
        let found = HashMap::from([("ESSA".to_owned(), essa())]);
        let _ = route_for(&flight(), None, &[], &reference, &mut runways, 0);
        for code in runways.take_wanted() {
            runways.settled(&code, Some(&found), 0);
        }
        // two arrivals on final for 19R, 4 and 6 NM out
        let e19r = runway_ends(&essa(), 137.0).into_iter().find(|e| e.ident == "19R").unwrap();
        let on_final = |id: u32, nm: f64| {
            let p = crate::flightpath::destination(e19r.threshold, e19r.heading + 180.0, nm * 1852.0);
            LiveFlight { id, lat: p[1] as f32, lon: p[0] as f32, track: 192, alt: 1_400, speed: 150, ..LiveFlight::default() }
        };
        let traffic = [on_final(21, 4.0), on_final(22, 6.0), flight()];
        let route = route_for(&flight(), None, &traffic, &reference, &mut runways, 0);
        assert_eq!(route.arrival, Some(RunwayGuess { runway: "19R".into(), reason: RunwayReason::Traffic }));
    }
}
