//! The selected flight on the map (item A8; doc 03 §8, §15; doc 02 §10): its trail and the trail's
//! shadow, its route (filed, waypoints, before, ahead), its origin and destination markers, the
//! focused airport's marker, the searched place's marker and the selection ring, as today's
//! `buildLayers` makes them (G:2743-2800, 3037-3055) with `routeLayers`, `drawnRoute`, `drawnAhead`,
//! `airportMarkers` and `placeMarker` (G:1386-1556).
//!
//! First it runs [`AppState::sync_flight`] (the selection's effects that read the feed, and the
//! camera's hand-over), so the scene shows the row, the details and the route of this refresh. Then,
//! with the aircraft on the map (`selectedPose = selected && showAircraft ? motion.pose(selected,
//! now) : null`, the feed clock's `now`):
//! - `pos`: the aircraft's drawn position, not raised onto its wheels (`at(selectedPose, ex)`, `ex`
//!   the exaggeration at this zoom);
//! - the trail: the details' trail without the points at exactly 0, 0 and those newer than the time
//!   the aircraft is drawn at (`motion.drawnTime`), each at its airport-relative drawn height
//!   (`motion.drawAltitude`, exaggerated), then `pos`; each point coloured by its altitude
//!   (`altitudeColor`), the last by the row's altitude;
//! - with a route: the filed waypoints on the ground; `drawnRoute(before)`; `drawnAhead(ahead,
//!   pos)`, the estimate from the aircraft as drawn, easing from its height into its own profile
//!   over [`AHEAD_BLEND_M`]; the origin and destination that are known as markers;
//! - the ring, while the aircraft is on the near hemisphere, `screenPx(sizeOf(icon)) · 0.6 + 4` px
//!   round `pos` (the icon class's size, not the published model's).
//!
//! Whatever the aircraft: the focused airport's marker (the board's code through the reference
//! data) and the searched place's. The label layer leaves out the airport codes that have a marker
//! of their own (`marked`: the focused airport, and with a selection the route's ends, by IATA code).

use std::cmp::Ordering;

use rustradar_core::geo::{altitude_color, cos_angle, elevation};
use rustradar_core::model::{Airport, f32_as_js};
use rustradar_core::motion::aircraft::Motion;
use rustradar_core::routes::{PathPoint, distance_km, trim_to_position};
use rustradar_core::flightpath::{DEFAULT_NEAR_M, DEFAULT_SEARCH_M};
use rustradar_core::settings::shown_kinds;
use rustradar_map::SceneInput;
use rustradar_map::layers::aircraft::assets;
use rustradar_map::layers::markers::{self, AirportMark, PlaceMark, Ring};
use rustradar_map::layers::route::{self, Trail};
use rustradar_scene::aircraft::Frame;
use rustradar_scene::aircraft_models::size_of;

use crate::state::AppState;

/// `AHEAD_BLEND_M`: over this distance the estimate ahead eases from the aircraft's drawn height
/// into its own profile.
pub const AHEAD_BLEND_M: f64 = 15_000.0;

/// `Math.max(a, b)`: NaN if either is.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}

/// JavaScript truthiness of a coordinate: not 0 and not NaN.
fn truthy(x: f32) -> bool {
    x != 0.0 && !x.is_nan()
}

/// `drawnRoute(points, id, ex)`: route heights (ft MSL) as drawn, airport-relative like the
/// aircraft and its trail, then exaggerated.
pub fn drawn_route(points: &[PathPoint], motion: &Motion, id: u32, ex: f64) -> Vec<[f64; 3]> {
    points.iter().map(|&[lon, lat, alt]| [lon, lat, elevation(motion.draw_altitude(id, alt, lat, lon), ex)]).collect()
}

/// `drawnAhead(ahead, pos, id, ex)`: the estimated path ahead from the aircraft as drawn now (the
/// route is worked out at each report, and the aircraft moves on along it between them).
pub fn drawn_ahead(ahead: &[PathPoint], pos: [f64; 3], motion: &Motion, id: u32, ex: f64) -> Vec<[f64; 3]> {
    let mut path = drawn_route(&trim_to_position(ahead, [pos[0], pos[1]], DEFAULT_SEARCH_M, DEFAULT_NEAR_M), motion, id, ex);
    let Some(first) = path.first() else { return path };
    let dz = pos[2] - first[2];
    let mut walked = 0.0;
    let mut i = 1;
    while i < path.len() && walked < AHEAD_BLEND_M {
        walked += distance_km([path[i - 1][0], path[i - 1][1]], [path[i][0], path[i][1]]) * 1000.0;
        path[i][2] = js_max(0.0, path[i][2] + dz * js_max(0.0, 1.0 - walked / AHEAD_BLEND_M));
        i += 1;
    }
    path[0] = pos;
    path
}

/// `airportCode(a)`: the IATA code, else the ICAO.
fn airport_mark(a: &Airport) -> AirportMark {
    let code = if a.iata.is_empty() { &a.icao } else { &a.iata };
    AirportMark { lon: a.lon, lat: a.lat, code: code.clone() }
}

/// The selected flight on the map: trail and its shadow, route, airport and place markers, selection ring.
#[derive(Debug, Default)]
pub struct Flight {}

impl Flight {
    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]), after
    /// [`AppState::sync_flight`]. Before the aircraft's builder: `state` is mutable for the motion
    /// model, as there.
    pub fn build(&mut self, state: &mut AppState, scene: &mut SceneInput) {
        state.sync_flight(scene.local_now_ms);
        let settings = &state.persisted.settings;
        let show_aircraft = shown_kinds(settings.mode, &settings.also_show).aircraft;
        let frame = Frame::new(&scene.view, scene.size, settings.exaggeration, assets::published().is_some());
        let ex = frame.exaggeration;
        let feed_now = scene.feed_now_ms;
        let reference = state.aircraft.reference().cloned();
        let mut route_input = route::Input::default();
        let mut marks = markers::Input::default();

        // with the aircraft off, nothing of the selected one either: no trail, route or ring
        let selected = state.flight.selected().filter(|_| show_aircraft).cloned();
        if let Some(selected) = &selected {
            let pose = state.motion.pose(selected, feed_now);
            let motion = &state.motion;
            let id = selected.id;
            let pos = [pose.lon, pose.lat, elevation(pose.alt_ft, ex)];
            // the aircraft is drawn a few seconds in the past: trail points newer than that would
            // run on past it and back, so the trail stops at the drawn time
            let drawn_s = motion.drawn_time(id, feed_now) / 1000.0;
            let trail: Vec<_> = state
                .flight
                .details()
                .map_or(&[][..], |d| &d.trail[..])
                .iter()
                .filter(|p| (truthy(p.latitude) || truthy(p.longitude)) && (p.timestamp as f64).partial_cmp(&drawn_s) != Some(Ordering::Greater))
                .collect();
            // the same airport-relative heights as the aircraft, so the trail meets it
            let mut path: Vec<[f64; 3]> = trail
                .iter()
                .map(|p| {
                    let (lon, lat) = (f32_as_js(p.longitude), f32_as_js(p.latitude));
                    [lon, lat, elevation(motion.draw_altitude(id, f64::from(p.altitude), lat, lon), ex)]
                })
                .collect();
            path.push(pos);
            let opaque = |[r, g, b]: [u8; 3]| [r, g, b, 255];
            let mut colors: Vec<[u8; 4]> = trail.iter().map(|p| opaque(altitude_color(f64::from(p.altitude)))).collect();
            colors.push(opaque(altitude_color(f64::from(selected.alt))));
            route_input.trail = Some(Trail { path, colors });

            if let Some(r) = state.flight.route() {
                if r.filed.len() > 1 {
                    route_input.filed = r.filed.clone();
                }
                if r.before.len() > 1 {
                    // ends at the trail's first point, at the same height
                    route_input.before = drawn_route(&r.before, motion, id, ex);
                }
                if r.ahead.len() > 1 {
                    route_input.ahead = drawn_ahead(&r.ahead, pos, motion, id, ex);
                }
                marks.route_airports = [&r.origin, &r.destination].into_iter().flatten().map(airport_mark).collect();
            }

            // ring the selection, drawn over neighbouring aircraft but not through the globe
            if cos_angle(pose.lon, pose.lat, frame.longitude, frame.latitude) > 0.0 {
                marks.ring = Some(Ring { position: pos, radius_px: frame.screen_px(size_of(&selected.icon)) * 0.6 + 4.0 });
            }
        }

        let focus = state.airport.code().and_then(|code| reference.as_ref().and_then(|r| r.airport(Some(code)).cloned()));
        marks.focus_airport = focus.as_ref().map(airport_mark);
        marks.place = state.flight.place().map(|p| PlaceMark { lon: p.lon, lat: p.lat, name: p.name.clone() });

        // the labels of the airports with markers of their own are left out
        let route_ends = state.flight.selected().and(state.flight.route()).map(|r| [r.origin.clone(), r.destination.clone()]);
        scene.labels.marked = std::iter::once(focus)
            .chain(route_ends.into_iter().flatten())
            .flatten()
            .map(|a| a.iata)
            .filter(|code| !code.is_empty())
            .collect();
        scene.route = route_input;
        scene.markers = marks;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `drawnAhead`: the estimate starts at the aircraft as drawn and eases from its height into
    /// its own profile over 15 km, never below the ground.
    #[test]
    fn the_estimate_ahead_starts_at_the_aircraft_and_eases_into_its_profile() {
        let motion = Motion::new();
        // a straight descent east to west, about 4.8 km between points, from 3000 ft to the ground
        let ahead: Vec<PathPoint> = (0..6).map(|i| [5.0 - 0.07 * f64::from(i), 52.3, 3000.0 - 600.0 * f64::from(i)]).collect();
        let pos = [5.0, 52.3, 1500.0];
        let path = drawn_ahead(&ahead, pos, &motion, 1, 2.0);
        assert_eq!(path[0], pos, "from the aircraft as drawn");
        let profile = drawn_route(&ahead, &motion, 1, 2.0);
        assert_eq!(profile[1][2], elevation(2400.0, 2.0), "drawRoute's heights: exaggerated feet, no airport near");
        // the first point is replaced by the aircraft, so dz is against the estimate's own start
        let dz = pos[2] - profile[0][2];
        let mut walked = 0.0;
        for i in 1..path.len() {
            walked += distance_km([profile[i - 1][0], profile[i - 1][1]], [profile[i][0], profile[i][1]]) * 1000.0;
            let expected = if walked - distance_km([profile[i - 1][0], profile[i - 1][1]], [profile[i][0], profile[i][1]]) * 1000.0 < AHEAD_BLEND_M {
                (profile[i][2] + dz * (1.0 - walked / AHEAD_BLEND_M).max(0.0)).max(0.0)
            } else {
                profile[i][2]
            };
            assert!((path[i][2] - expected).abs() < 1e-9, "point {i}: {} against {expected}", path[i][2]);
        }
        assert!(path[1][2] < profile[1][2], "eased down toward the aircraft");
        assert_eq!(path[5][2], profile[5][2], "past 15 km its own profile");
        assert!(path.iter().all(|p| p[2] >= 0.0));
    }
}
