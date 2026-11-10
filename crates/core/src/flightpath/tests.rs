//! `routes.test.ts` (15 cases, which test `flightpath.ts`), the golden routes of S1.17, and new
//! tests for the parts the TypeScript tests do not reach.

use super::dubins::{dubins, turn_then_straight};
use super::*;
use crate::assert_close;
use crate::model::{Airport, LiveFlight, Runway, RunwayEnd, TrailPoint, f32_as_js};
use crate::num::wrap180;

const NM: f64 = 1852.0;
const FT: f64 = 0.3048;

fn runway_end(ident: &str, lat: f64, lon: f64, elevation_ft: f64, displaced_ft: f64) -> RunwayEnd {
    RunwayEnd { ident: ident.into(), lat, lon, elevation_ft: Some(elevation_ft), displaced_ft }
}

fn runway(length_ft: f64, surface: &str, ends: [RunwayEnd; 2]) -> Runway {
    Runway { length_ft: Some(length_ft), width_ft: Some(148.0), surface: surface.into(), ends }
}

/// Copenhagen Kastrup's runways, as OurAirports lists them.
fn ekch() -> Vec<Runway> {
    vec![
        runway(11811.0, "ASP", [
            runway_end("04L", 55.592201, 12.603536, 13.0, 0.0),
            runway_end("22R", 55.616543, 12.641174, 14.0, 1967.0),
        ]),
        runway(10827.0, "ASP", [
            runway_end("04R", 55.6031, 12.633, 12.0, 0.0),
            runway_end("22L", 55.6254, 12.6676, 8.0, 0.0),
        ]),
        runway(9186.0, "ASPH", [
            runway_end("12", 55.626293, 12.633331, 13.0, 2313.0),
            runway_end("30", 55.612522, 12.670532, 8.0, 886.0),
        ]),
    ]
}

/// `runwayEnds(EKCH, 17)`.
fn ends() -> Vec<RunwayEndInfo> {
    runway_ends(&ekch(), 17.0)
}

fn end<'a>(ends: &'a [RunwayEndInfo], ident: &str) -> &'a RunwayEndInfo {
    ends.iter().find(|e| e.ident == ident).unwrap()
}

const CPH: LonLat = [12.656, 55.618];

fn ll(p: &[f64]) -> LonLat {
    [p[0], p[1]]
}

/// The largest change of direction (degrees) between consecutive legs longer than `min_m`.
fn max_kink<const N: usize>(path: &[[f64; N]], min_m: f64) -> f64 {
    let mut worst: f64 = 0.0;
    let mut previous: Option<f64> = None;
    for i in 1..path.len() {
        if distance_m(ll(&path[i - 1]), ll(&path[i])) < min_m {
            continue;
        }
        let course = bearing(ll(&path[i - 1]), ll(&path[i]));
        if let Some(previous) = previous {
            worst = worst.max(wrap180(course - previous).abs());
        }
        previous = Some(course);
    }
    worst
}

/// A point on an end's extended centreline, `nm` before the threshold and `offset_m` to the right.
fn on_approach(e: &RunwayEndInfo, nm: f64, offset_m: f64) -> LonLat {
    let p = destination(e.threshold, e.heading + 180.0, nm * NM);
    if offset_m != 0.0 { destination(p, e.heading + 90.0, offset_m) } else { p }
}

fn aircraft(at: LonLat, track: f64, alt_ft: f64, speed_kt: f64) -> Aircraft {
    Aircraft { lon: at[0], lat: at[1], track, alt_ft, speed_kt }
}

fn traffic_at(id: u32, at: LonLat, track: f64, alt: f64, speed: f64) -> Traffic {
    Traffic { id, lat: at[1], lon: at[0], track, alt, speed, on_ground: false }
}

fn copenhagen() -> Airport {
    Airport {
        id: 1,
        name: "Copenhagen".into(),
        iata: "CPH".into(),
        icao: "EKCH".into(),
        lat: CPH[1],
        lon: CPH[0],
        alt: Some(17),
        size: 1,
        ..Airport::default()
    }
}

// --- routes.test.ts ----------------------------------------------------------------------------

#[test]
fn runway_ends_both_directions_displaced_thresholds_true_headings() {
    let ends = ends();
    assert_eq!(ends.len(), 6);
    assert!(wrap180(end(&ends, "22L").heading - 221.0).abs() < 1.0);
    assert!(wrap180(end(&ends, "04R").heading - 41.0).abs() < 1.0);
    // 22R's threshold is displaced 1,967 ft along the runway
    assert_close!(distance_m(end(&ends, "22R").start, end(&ends, "22R").threshold), 1967.0 * FT, 0);
    assert_eq!(end(&ends, "22L").elevation_ft, 8.0);
}

#[test]
fn runway_choice_the_one_the_aircraft_is_lined_up_with() {
    let ends = ends();
    let ac = aircraft(on_approach(end(&ends, "22L"), 6.0, 150.0), 221.0, 1_900.0, 160.0);
    let choice = choose_arrival_runway(&ends, &ac, &[], None).unwrap();
    assert_eq!(choice.end.ident, "22L");
    assert_eq!(choice.reason, RunwayReason::Aligned);
    // and its parallel when that is the one it is on
    let on_22r = aircraft(on_approach(end(&ends, "22R"), 6.0, 0.0), 221.0, 1_900.0, 160.0);
    assert_eq!(choose_arrival_runway(&ends, &on_22r, &[], None).unwrap().end.ident, "22R");
}

#[test]
fn runway_choice_the_direction_other_traffic_is_landing() {
    let ends = ends();
    // east of the field heading north: no runway lines up with that
    let ac = aircraft([12.813, 55.618], 22.0, 4_250.0, 220.0);
    let landing22 = traffic_at(9, on_approach(end(&ends, "22L"), 5.0, 50.0), 221.0, 1_600.0, 150.0);
    let choice = choose_arrival_runway(&ends, &ac, &[landing22], None).unwrap();
    assert_eq!(choice.end.ident, "22L");
    assert_eq!(choice.reason, RunwayReason::Traffic);
    // a departure climbing out on 04L says the flow is 04
    let e04l = end(&ends, "04L");
    let past04l = destination(e04l.start, e04l.heading, e04l.length_m + 3_000.0);
    let departing04 = traffic_at(8, past04l, 41.0, 1_500.0, 180.0);
    let flow04 = choose_arrival_runway(&ends, &ac, &[departing04], None).unwrap();
    assert!(wrap180(flow04.end.heading - 41.0).abs() < 2.0);
    assert_eq!(flow04.reason, RunwayReason::Traffic);
    // the aircraft itself and unrelated traffic count for nothing
    let own = Traffic { id: 1, ..landing22 };
    assert_eq!(choose_arrival_runway(&ends, &ac, &[own], Some(1)).unwrap().reason, RunwayReason::Guessed);
    let high = Traffic { alt: 9_000.0, ..landing22 };
    assert_eq!(choose_arrival_runway(&ends, &ac, &[high], None).unwrap().reason, RunwayReason::Guessed);
}

#[test]
fn runway_choice_without_evidence_a_guess_the_end_reached_most_directly() {
    let ends = ends();
    // straight in from the north-east, but still far out and not yet aligned
    let ac = aircraft(destination(CPH, 41.0, 60_000.0), 230.0, 12_000.0, 280.0);
    let choice = choose_arrival_runway(&ends, &ac, &[], None).unwrap();
    assert_eq!(choice.reason, RunwayReason::Guessed);
    assert!(wrap180(choice.end.heading - 221.0).abs() < 2.0);
}

#[test]
fn final_approach_on_the_extended_centreline_then_the_threshold_and_the_aiming_point() {
    let ends = ends();
    let e = end(&ends, "22L");
    let path = arrival_path(&aircraft([12.813, 55.618], 22.0, 4_250.0, 220.0), e);
    let last = path[path.len() - 1];
    assert!(distance_m(ll(&last), destination(e.threshold, e.heading, AIM_M)) < 1.0);
    assert!(path.iter().any(|p| distance_m(ll(&p[..]), e.threshold) < 1.0));
    // the last 5 NM before the threshold lie on the centreline
    // (the downwind leg runs alongside, kilometres off to the side)
    let last_5nm: Vec<&PathPoint> = path
        .iter()
        .filter(|p| {
            let CentrelineOffset { along, cross } = centreline_offset(e, ll(&p[..]));
            along > 0.0 && along <= 5.0 * NM && cross.abs() < 1_000.0
        })
        .collect();
    assert!(last_5nm.len() > 2);
    for p in last_5nm {
        assert!(centreline_offset(e, ll(&p[..])).cross.abs() < 5.0);
    }
}

#[test]
fn final_approach_heights_follow_the_3_glide_path_to_the_touchdown_zone() {
    let ends = ends();
    let e = end(&ends, "22L");
    let path = arrival_path(&aircraft([12.813, 55.618], 22.0, 4_250.0, 220.0), e);
    let ft_per_nm = ((3.0 * std::f64::consts::PI) / 180.0).tan() * (NM / FT); // ≈ 318
    for p in &path {
        let CentrelineOffset { along, cross } = centreline_offset(e, ll(&p[..]));
        if !(0.0..=8.0 * NM).contains(&along) || cross.abs() > 5.0 {
            continue;
        }
        let expected = e.elevation_ft + ((along + AIM_M) / NM) * ft_per_nm;
        assert!((p[2] - expected).abs() < 5.0);
    }
    // 50 ft over the threshold, on the ground at the aiming point
    let at_threshold = path.iter().find(|p| distance_m(ll(&p[..]), e.threshold) < 1.0).unwrap();
    assert_close!(at_threshold[2] - e.elevation_ft, 50.0, 0);
    assert_close!(path[path.len() - 1][2], e.elevation_ft, 3);
    // never climbs, and starts at the aircraft's altitude
    assert_eq!(path[0][2], 4_250.0);
    for i in 1..path.len() {
        assert!(path[i][2] <= path[i - 1][2] + 1e-9);
    }
}

#[test]
fn an_aircraft_below_the_glide_path_flies_level_until_it_meets_it() {
    let ends = ends();
    let e = end(&ends, "22L");
    let path = arrival_path(&aircraft(on_approach(e, 14.0, 3_000.0), 260.0, 2_000.0, 200.0), e);
    let level_until = path.iter().position(|p| p[2] < 2_000.0 - 1.0).unwrap();
    assert!(level_until > 0);
    // where it starts down it is on the glide path, about 6 NM out
    let CentrelineOffset { along, .. } = centreline_offset(e, ll(&path[level_until]));
    assert!(along / NM > 5.0);
    assert!(along / NM < 7.0);
}

#[test]
fn the_path_leaves_along_the_aircrafts_track_and_turns_smoothly() {
    let ends = ends();
    let e = end(&ends, "22L");
    for (at, track, alt) in [
        ([12.813, 55.618], 22.0, 4_250.0), // downwind, as in the recording
        ([12.893, 55.715], 306.0, 3_650.0), // base
        ([12.4, 55.5], 90.0, 5_000.0),     // from the south-west, passing the field
        ([12.9, 55.55], 200.0, 6_000.0),   // heading away from the final
    ] {
        let path = arrival_path(&aircraft(at, track, alt, 220.0), e);
        assert!(wrap180(bearing(ll(&path[0]), ll(&path[1])) - track).abs() < 3.0);
        assert!(max_kink(&path, 1.0) < 6.0);
    }
}

#[test]
fn dubins_paths_arrive_at_the_pose_asked_for_without_kinks() {
    let mut seed: f64 = 7.0;
    let mut random = || {
        seed = (seed * 16807.0) % 2147483647.0;
        seed / 2147483647.0
    };
    let course = |a: [f64; 2], b: [f64; 2]| ((b[0] - a[0]).atan2(b[1] - a[1]) * 180.0) / std::f64::consts::PI;
    for _ in 0..500 {
        let (x0, y0, h0, h1) = ((random() - 0.5) * 40_000.0, (random() - 0.5) * 40_000.0, random() * 360.0, random() * 360.0);
        let r = 1_000.0 + random() * 5_000.0;
        let points = dubins(x0, y0, h0, 0.0, 0.0, h1, r);
        let n = points.len();
        assert!(points[n - 1][0].hypot(points[n - 1][1]) < 1e-6);
        assert!(wrap180(course(points[0], points[1]) - h0).abs() < 2.0);
        assert!(wrap180(course(points[n - 2], points[n - 1]) - h1).abs() < 2.0);
        for k in 2..n {
            if (points[k][0] - points[k - 1][0]).hypot(points[k][1] - points[k - 1][1]) < 1.0 {
                continue;
            }
            assert!(wrap180(course(points[k - 1], points[k]) - course(points[k - 2], points[k - 1])).abs() < 3.01);
        }
    }
}

#[test]
fn far_away_the_en_route_part_is_the_great_circle_only_the_ends_are_shaped() {
    let ends = ends();
    let e = end(&ends, "22L");
    // over Bergen, heading roughly for Copenhagen
    let from: LonLat = [5.2, 60.3];
    let course = bearing(from, CPH);
    let path = arrival_path(&aircraft(from, course, 36_000.0, 450.0), e);
    assert!(max_kink(&path, 1.0) < 6.0);
    let total = distance_m(from, CPH);
    // the points more than 100 km from both ends lie on one great circle...
    let middle: Vec<&PathPoint> =
        path.iter().filter(|p| distance_m(from, ll(&p[..])) > 100_000.0 && distance_m(ll(&p[..]), CPH) > 100_000.0).collect();
    assert!(middle.len() > 5);
    let circle = great_circle(ll(middle[0]), ll(middle[middle.len() - 1]), 2.0);
    let off_circle = |p: LonLat| circle.iter().map(|&q| distance_m(p, q)).fold(f64::INFINITY, f64::min);
    for p in &middle {
        assert!(off_circle(ll(&p[..])) < 1_100.0);
    }
    // ...the one from the aircraft, give or take the airport's size
    let first = ll(middle[0]);
    assert!(wrap180(bearing(first, ll(middle[middle.len() - 1])) - bearing(first, CPH)).abs() < 2.0);
    // cruise until the descent: level for the first half
    for p in &path {
        if distance_m(from, ll(&p[..])) < total / 2.0 {
            assert_eq!(p[2], 36_000.0);
        }
    }
}

#[test]
fn far_away_and_turning_joins_the_great_circle_smoothly_from_the_current_track() {
    let ends = ends();
    let from: LonLat = [5.2, 60.3];
    let path = arrival_path(&aircraft(from, 300.0, 20_000.0, 400.0), end(&ends, "22L"));
    assert!(wrap180(bearing(ll(&path[0]), ll(&path[1])) - 300.0).abs() < 3.0);
    assert!(max_kink(&path, 1.0) < 6.0);
}

#[test]
fn departure_rolls_on_the_runway_climbs_out_straight_ahead_then_turns_onto_the_first_tracked_track() {
    let ends = ends();
    let e = end(&ends, "22R");
    let to = aircraft([12.2, 55.4], 250.0, 9_000.0, 300.0);
    let path = departure_path(e, &to);
    assert!(distance_m(ll(&path[0]), e.start) < 1.0);
    assert_eq!(path[0][2], e.elevation_ft);
    assert!(distance_m(ll(&path[path.len() - 1]), [to.lon, to.lat]) < 1.0);
    assert_eq!(path[path.len() - 1][2], 9_000.0);
    // straight ahead on the centreline for the first few kilometres
    for p in &path {
        let CentrelineOffset { along, cross } = centreline_offset(e, ll(&p[..]));
        if -along < 6_000.0 {
            assert!(cross.abs() < 5.0);
        }
    }
    let n = path.len();
    assert!(wrap180(bearing(ll(&path[n - 2]), ll(&path[n - 1])) - 250.0).abs() < 3.0);
    assert!(max_kink(&path, 1.0) < 6.0);
    for i in 1..n {
        assert!(path[i][2] >= path[i - 1][2]);
    }
}

#[test]
fn trimming_starts_the_path_at_the_aircraft_as_it_moves_along_it() {
    let ends = ends();
    let path = arrival_path(&aircraft([12.813, 55.618], 22.0, 4_250.0, 220.0), end(&ends, "22L"));
    let moved = destination(ll(&path[0]), 22.0, 900.0);
    let trimmed = trim_to_position(&path, moved, DEFAULT_SEARCH_M, DEFAULT_NEAR_M);
    assert_eq!(trimmed[0][0], moved[0]);
    assert!(distance_m(ll(&trimmed[1]), ll(&path[0])) > 900.0);
    assert!(trimmed.len() < path.len() + 1);
    assert!(max_kink(&trimmed, 1.0) < 6.0);
}

#[test]
fn a_whole_estimate_names_its_runways_and_stays_an_estimate() {
    let flight = LiveFlight { id: 1, lat: 55.618, lon: 12.813, track: 22, alt: 4_250, speed: 220, on_ground: false, ..LiveFlight::default() };
    let cph = copenhagen();
    let ekch = ekch();
    let estimate = estimate_route(&EstimateInput {
        flight: &flight,
        trail_start: None,
        origin: None,
        destination: Some(&cph),
        origin_runways: &[],
        destination_runways: &ekch,
        traffic: &[],
    });
    assert_eq!(estimate.kind, RouteKind::Estimated);
    assert_eq!(estimate.arrival.unwrap().reason, RunwayReason::Guessed);
    assert!(estimate.ahead.len() > 10);
    // without runways it still ends at the airport, without a kink at the aircraft
    let bare = estimate_route(&EstimateInput {
        flight: &flight,
        trail_start: None,
        origin: None,
        destination: Some(&cph),
        origin_runways: &[],
        destination_runways: &[],
        traffic: &[],
    });
    assert_eq!(bare.arrival, None);
    assert!(distance_m(ll(&bare.ahead[bare.ahead.len() - 1]), CPH) < 1.0);
    assert!(max_kink(&bare.ahead, 1.0) < 6.0);
}

#[test]
fn an_aircraft_already_over_the_runway_rolls_out_along_it_rather_than_going_round_to_land_again() {
    let ends = ends();
    let e = end(&ends, "22L");
    // past the threshold in the flare, then on the landing roll: Flightradar24 still says airborne
    for (past_m, alt_ft, speed_kt) in [(150.0, 40.0, 140.0), (900.0, 17.0, 120.0), (1_500.0, 17.0, 60.0)] {
        let at = destination(e.threshold, e.heading, past_m);
        let ac = aircraft(at, e.heading, alt_ft, speed_kt);
        let choice = choose_arrival_runway(&ends, &ac, &[], None).unwrap();
        assert_eq!(choice.end.ident, "22L");
        assert_eq!(choice.reason, RunwayReason::Aligned);
        let path = arrival_path(&ac, e);
        // straight ahead down the runway, never back towards the approach
        for p in &path[1..] {
            assert!(wrap180(bearing(at, ll(&p[..])) - e.heading).abs() < 1.0);
        }
        assert!(distance_m(at, ll(&path[path.len() - 1])) < 2_000.0);
    }
}

// --- the golden routes of S1.17 ----------------------------------------------------------------

/// A JSON number that must be there.
fn num(v: &serde_json::Value, key: &str) -> f64 {
    v[key].as_f64().unwrap_or_else(|| panic!("{key} in {v}"))
}

/// The golden file's flight: `lat` and `lon` are the shortest decimals of an `f32`.
fn golden_flight(v: &serde_json::Value) -> LiveFlight {
    let (lat, lon) = (num(v, "lat") as f32, num(v, "lon") as f32);
    assert_eq!((f32_as_js(lat), f32_as_js(lon)), (num(v, "lat"), num(v, "lon")));
    LiveFlight {
        id: num(v, "id") as u32,
        lat,
        lon,
        track: num(v, "track") as i32,
        alt: num(v, "alt") as i32,
        speed: num(v, "speed") as i32,
        on_ground: v["onGround"].as_bool().unwrap(),
        ..LiveFlight::default()
    }
}

fn golden_traffic(v: &serde_json::Value) -> Traffic {
    Traffic {
        id: num(v, "id") as u32,
        lat: num(v, "lat"),
        lon: num(v, "lon"),
        track: num(v, "track"),
        alt: num(v, "alt"),
        speed: num(v, "speed"),
        on_ground: v["onGround"].as_bool().unwrap(),
    }
}

fn golden_points(v: &serde_json::Value) -> Vec<PathPoint> {
    serde_json::from_value(v.clone()).unwrap()
}

#[test]
fn estimate_route_matches_the_golden_routes() {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/data/golden/routes.json")).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 13);
    let (mut worst_deg, mut worst_ft) = (0.0_f64, 0.0_f64);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        let flight = golden_flight(&input["flight"]);
        let trail_start: Option<TrailPoint> =
            input.get("trailStart").map(|v| serde_json::from_value(v.clone()).unwrap());
        if let Some(t) = &trail_start {
            assert_eq!(f32_as_js(t.latitude), num(&input["trailStart"], "latitude"), "{name}");
        }
        let airport = |key: &str| -> Option<Airport> { input.get(key).map(|v| serde_json::from_value(v.clone()).unwrap()) };
        let (origin, destination) = (airport("origin"), airport("destination"));
        let runways = |key: &str| -> Vec<Runway> { serde_json::from_value(input[key].clone()).unwrap() };
        let (origin_runways, destination_runways) = (runways("originRunways"), runways("destinationRunways"));
        let traffic: Vec<Traffic> = input["traffic"].as_array().unwrap().iter().map(golden_traffic).collect();
        let estimate = estimate_route(&EstimateInput {
            flight: &flight,
            trail_start: trail_start.as_ref(),
            origin: origin.as_ref(),
            destination: destination.as_ref(),
            origin_runways: &origin_runways,
            destination_runways: &destination_runways,
            traffic: &traffic,
        });

        let output = &case["output"];
        assert_eq!(serde_json::to_value(estimate.kind).unwrap(), output["kind"], "{name}: kind");
        assert_eq!(serde_json::to_value(&estimate.departure).unwrap(), output["departure"], "{name}: departure");
        assert_eq!(serde_json::to_value(&estimate.arrival).unwrap(), output["arrival"], "{name}: arrival");
        for (part, actual) in [("before", &estimate.before), ("ahead", &estimate.ahead)] {
            let expected = golden_points(&output[part]);
            assert_eq!(actual.len(), expected.len(), "{name}: {part} points");
            for (i, (a, e)) in actual.iter().zip(&expected).enumerate() {
                let deg = (a[0] - e[0]).abs().max((a[1] - e[1]).abs());
                let ft = (a[2] - e[2]).abs();
                assert!(deg < 1e-9, "{name}: {part}[{i}] {a:?} against {e:?}");
                assert!(ft < 1e-6, "{name}: {part}[{i}] {a:?} against {e:?}");
                worst_deg = worst_deg.max(deg);
                worst_ft = worst_ft.max(ft);
            }
        }
    }
    eprintln!("golden routes: largest difference {worst_deg:e}° and {worst_ft:e} ft");
}

// --- new tests ---------------------------------------------------------------------------------

#[test]
fn constants_are_the_typescripts() {
    // the literals are what JavaScript computes; the platform's tan agrees to the last bit or so
    assert_close!(GLIDE_TAN, (GLIDE_DEG * DEG).tan(), 16);
    assert_close!(AIM_M, (THRESHOLD_CROSSING_FT * FT_M) / GLIDE_TAN, 12);
    assert_close!(AIM_M, 290.8, 1);
    assert_eq!(ARC_STEP, 3.0 * (std::f64::consts::PI / 180.0));
}

#[test]
fn js_min_and_max_keep_nan_and_order_zeros() {
    assert!(js_min(f64::NAN, 1.0).is_nan());
    assert!(js_min(1.0, f64::NAN).is_nan());
    assert!(js_max(f64::NAN, 1.0).is_nan());
    assert!(js_max(1.0, f64::NAN).is_nan());
    assert!(js_min(0.0, -0.0).is_sign_negative());
    assert!(js_min(-0.0, 0.0).is_sign_negative());
    assert!(js_max(-0.0, 0.0).is_sign_positive());
    assert!(js_max(0.0, -0.0).is_sign_positive());
    assert_eq!(js_min(2.0, 3.0), 2.0);
    assert_eq!(js_max(2.0, 3.0), 3.0);
    assert_eq!(clamp(5.0, 0.0, 1.0), 1.0);
    assert_eq!(clamp(-5.0, 0.0, 1.0), 0.0);
    assert!(clamp(f64::NAN, 0.0, 1.0).is_nan());
    assert_eq!(mod2pi(-0.5), TAU - 0.5);
    assert_eq!(mod2pi(TAU + 0.5), 0.5);
}

#[test]
fn turn_radius_at_the_usual_bank() {
    // the TypeScript's turnRadius(140), (220), (480)
    assert_close!(turn_radius(140.0), 1_134.329_066_131_528_4, 9);
    assert_close!(turn_radius(220.0), 2_801.098_306_161_53, 9);
    assert_close!(turn_radius(480.0), 13_334.153_920_240_009, 9);
    assert_eq!(turn_radius(0.0), 0.0);
}

#[test]
fn the_local_frame_keeps_distances_and_courses_from_its_centre() {
    let p = destination(CPH, 30.0, 50_000.0);
    let [x, y] = to_local(CPH, p);
    assert_close!(x.hypot(y), 50_000.0, 6);
    assert_close!(x.atan2(y) / DEG, 30.0, 9);
    let back = from_local(CPH, x, y);
    assert_close!(back[0], p[0], 9);
    assert_close!(back[1], p[1], 9);
}

#[test]
fn turn_then_straight_turns_once_then_flies_straight_to_the_point() {
    // heading north, the point to the east: a right turn, then straight east
    let points = turn_then_straight(0.0, 0.0, 0.0, 20_000.0, 0.0, 2_000.0);
    assert_eq!(points[0], [0.0, 0.0]);
    assert_eq!(*points.last().unwrap(), [20_000.0, 0.0]);
    let course = |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]).atan2(b[1] - a[1]) / DEG;
    assert!(course(points[0], points[1]).abs() < 2.0);
    let n = points.len();
    // the straight leg runs along the tangent from the right turn's circle (centre (2000, 0))
    assert_close!(course(points[n - 2], points[n - 1]), 90.0 + (2_000.0_f64 / 18_000.0).asin() / DEG, 6);
    for k in 2..n {
        let turn = wrap180(course(points[k - 1], points[k]) - course(points[k - 2], points[k - 1]));
        // only right turns (clockwise, so the compass course grows), never more than 3° a step
        assert!((-1e-9..3.01).contains(&turn), "{turn}");
    }
}

#[test]
fn dubins_falls_back_to_the_straight_line_and_samples_exact_ends() {
    // the same pose: nothing to fly
    let same = dubins(0.0, 0.0, 90.0, 0.0, 0.0, 90.0, 1_000.0);
    assert_eq!(same, vec![[0.0, 0.0]]);
    // straight ahead: one straight piece every 2 km
    let ahead = dubins(-10_000.0, 0.0, 90.0, 0.0, 0.0, 90.0, 1_000.0);
    assert_eq!(ahead.len(), 6);
    assert_eq!(ahead[0], [-10_000.0, 0.0]);
    assert_eq!(ahead[5], [0.0, 0.0]);
    for (i, p) in ahead.iter().enumerate() {
        assert_close!(p[0], -10_000.0 + 2_000.0 * i as f64, 6);
        assert_close!(p[1], 0.0, 6);
    }
}

#[test]
fn hard_surfaces_win_and_short_runways_are_dropped() {
    let grass = runway(6000.0, "GRS", [runway_end("09", 50.0, 10.0, 100.0, 0.0), runway_end("27", 50.0, 10.03, 100.0, 0.0)]);
    let concrete =
        runway(6000.0, "concrete", [runway_end("18", 50.02, 10.01, 90.0, 0.0), runway_end("36", 50.0, 10.01, 90.0, 0.0)]);
    // a hard surface hides the grass, matched in any case
    let ends = runway_ends(&[grass.clone(), concrete.clone()], 0.0);
    assert_eq!(ends.iter().map(|e| e.ident.as_str()).collect::<Vec<_>>(), ["18", "36"]);
    // without one every runway is usable
    let ends = runway_ends(std::slice::from_ref(&grass), 0.0);
    assert_eq!(ends.len(), 2);
    assert_close!(ends[0].heading, 90.0, 1);
    // a runway far shorter than the longest goes, but 1800 m always stays
    let short = runway(3000.0, "ASP", [runway_end("05", 50.0, 10.0, 0.0, 0.0), runway_end("23", 50.005, 10.005, 0.0, 0.0)]);
    let ends = runway_ends(&[concrete.clone(), short], 0.0);
    assert_eq!(ends.iter().map(|e| e.ident.as_str()).collect::<Vec<_>>(), ["18", "36"]);
    // an end without an elevation takes the field's
    let mut unknown = concrete;
    unknown.ends[0].elevation_ft = None;
    let ends = runway_ends(&[unknown], 321.0);
    assert_eq!((ends[0].elevation_ft, ends[1].elevation_ft), (321.0, 90.0));
    assert!(runway_ends(&[], 0.0).is_empty());
}

#[test]
fn runway_use_counts_each_flight_once_for_its_best_aligned_end() {
    let ends = ends();
    let e22l = end(&ends, "22L");
    let e04r = end(&ends, "04R");
    let i22l = ends.iter().position(|e| e.ident == "22L").unwrap();
    let i04r = ends.iter().position(|e| e.ident == "04R").unwrap();
    let on_final = traffic_at(1, on_approach(e22l, 4.0, 20.0), 221.0, 1_300.0, 150.0);
    let rolling = Traffic { on_ground: true, ..traffic_at(2, destination(e04r.start, e04r.heading, 500.0), 41.0, 0.0, 120.0) };
    let taxiing = Traffic { speed: 20.0, ..rolling };
    let wrong_way = Traffic { track: 180.0, ..on_final };
    let usage = runway_use(&ends, &[on_final, rolling, taxiing, wrong_way], None);
    assert_eq!(usage.len(), ends.len());
    assert_eq!(usage[i22l], RunwayUse { arrivals: 1, departures: 0 });
    assert_eq!(usage[i04r], RunwayUse { arrivals: 0, departures: 1 });
    let total: u32 = usage.iter().map(|u| u.arrivals + u.departures).sum();
    assert_eq!(total, 2);
    // the ignored id counts for nothing
    let usage = runway_use(&ends, &[on_final], Some(1));
    assert!(usage.iter().all(|u| *u == RunwayUse::default()));
}

#[test]
fn departure_runway_follows_the_flow_in_use() {
    let ends = ends();
    let to = aircraft([12.2, 55.4], 250.0, 9_000.0, 300.0);
    let guessed = choose_departure_runway(&ends, &to, &[]).unwrap();
    assert_eq!(guessed.reason, RunwayReason::Guessed);
    // a departure rolling on 04R: the flow is 04, whatever the shortest way would be
    let e04r = end(&ends, "04R");
    let rolling = Traffic { on_ground: true, ..traffic_at(2, destination(e04r.start, e04r.heading, 500.0), 41.0, 0.0, 120.0) };
    let choice = choose_departure_runway(&ends, &to, &[rolling]).unwrap();
    assert_eq!(choice.reason, RunwayReason::Traffic);
    assert!(wrap180(choice.end.heading - 41.0).abs() < 2.0);
    assert!(choose_departure_runway(&[], &to, &[]).is_none());
    assert!(choose_arrival_runway(&[], &to, &[], None).is_none());
}

#[test]
fn trimming_short_paths_and_paths_that_are_far_away() {
    assert!(trim_to_position(&[], CPH, DEFAULT_SEARCH_M, DEFAULT_NEAR_M).is_empty());
    let one = [[12.0, 55.0, 1_000.0]];
    assert_eq!(trim_to_position(&one, CPH, DEFAULT_SEARCH_M, DEFAULT_NEAR_M), one.to_vec());
    // nothing near: `here` replaces the first point and keeps its altitude
    let path = [[12.0, 55.0, 1_000.0], [12.1, 55.0, 900.0], [12.2, 55.0, 800.0]];
    let far = [14.0, 57.0];
    let trimmed = trim_to_position(&path, far, DEFAULT_SEARCH_M, DEFAULT_NEAR_M);
    assert_eq!(trimmed, vec![[14.0, 57.0, 1_000.0], path[1], path[2]]);
    // halfway along the second leg: the altitude is interpolated
    let halfway = [12.15, 55.0];
    let trimmed = trim_to_position(&path, halfway, DEFAULT_SEARCH_M, DEFAULT_NEAR_M);
    assert_eq!(trimmed.len(), 2);
    assert_eq!(ll(&trimmed[0]), halfway);
    assert_close!(trimmed[0][2], 850.0, 0);
    assert_eq!(trimmed[1], path[2]);
}

#[test]
fn estimates_without_airports_on_the_ground_and_from_the_gate() {
    let cph = copenhagen();
    let ekch = ekch();
    let base = LiveFlight { id: 1, lat: 55.618, lon: 12.813, track: 22, alt: 4_250, speed: 220, ..LiveFlight::default() };
    let input = EstimateInput {
        flight: &base,
        trail_start: None,
        origin: None,
        destination: None,
        origin_runways: &[],
        destination_runways: &ekch,
        traffic: &[],
    };
    let none = estimate_route(&input);
    assert_eq!(none, Estimate { kind: RouteKind::None, before: vec![], ahead: vec![], departure: None, arrival: None });
    // on the ground: nothing ahead, still an estimate
    let parked = LiveFlight { on_ground: true, ..base.clone() };
    let ground = estimate_route(&EstimateInput { flight: &parked, destination: Some(&cph), ..input });
    assert_eq!((ground.kind, ground.ahead.len(), ground.arrival), (RouteKind::Estimated, 0, None));
    // a trail that begins at the gate leaves nothing to fill before it
    let gate = TrailPoint { latitude: 55.62, longitude: 12.65, ..TrailPoint::default() };
    let from_gate = estimate_route(&EstimateInput { origin: Some(&cph), trail_start: Some(&gate), origin_runways: &ekch, ..input });
    assert_eq!((from_gate.before.len(), from_gate.departure), (0, None));
    // a real gap: the climb from the runway, at 250 kt when the trail point has no speed
    let first = TrailPoint { latitude: 55.9, longitude: 13.1, track: 35, altitude: 11_000, ..TrailPoint::default() };
    let gap = estimate_route(&EstimateInput { origin: Some(&cph), trail_start: Some(&first), origin_runways: &ekch, ..input });
    let departure = gap.departure.unwrap();
    assert_eq!(departure.reason, RunwayReason::Guessed);
    assert_eq!(gap.before[gap.before.len() - 1][2], 11_000.0);
    assert!(distance_m(ll(&gap.before[gap.before.len() - 1]), [f32_as_js(13.1), f32_as_js(55.9)]) < 1.0);
}

#[test]
fn near_keeps_the_traffic_within_30_km_of_the_airport() {
    let cph = copenhagen();
    let at = |km: f64, course: f64| {
        let p = destination(CPH, course, km * 1000.0);
        traffic_at(1, p, 0.0, 1_000.0, 150.0)
    };
    let kept = estimate::near(&[at(5.0, 0.0), at(29.0, 90.0), at(31.0, 180.0), at(29.9, 270.0)], &cph);
    assert_eq!(kept.len(), 3);
}
