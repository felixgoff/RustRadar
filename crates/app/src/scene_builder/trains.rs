//! Trains on the map (item T2; doc 02 §5.1–5.5; `trainLayers`, G:2031-2071): at each scene refresh,
//! the trains the train state puts on the map, drawn where they are expected now, as icons further
//! out ([`rustradar_scene::trains::TrainBuilder`]); close in, as their cars (item T3,
//! [`TrainBuilder::car_layers`] with the operator models of
//! [`rustradar_map::layers::train_cars::models`]); the selected train's halo, ring and route (item
//! T6, [`rustradar_map::layers::train_route`]; doc 02 §5.7, §7).
//!
//! What it reads of the state, as `buildLayers()` reads the globe's `trains` prop:
//! - the list: [`state::trains::Trains::on_map`](crate::state::trains::Trains::on_map)
//!   (`trainsOnMap`: the trains the filters keep that still run, the selected one as last
//!   reported), empty when trains are off the map;
//! - when that list is a new one: whenever
//!   [`on_map_generation`](crate::state::trains::Trains::on_map_generation) changes, and at each
//!   whole second of the local clock, because today's page derives the list from its 1 s clock
//!   (`trainRunning(t, now)`), so the globe gets a new array every second and eases every train
//!   again from where it is drawn (a train mid-way through its easing starts a new one from there);
//! - the local clock in whole milliseconds (`Date.now()`): the trains' frames are timed on this
//!   machine's clock, not the flight feed's;
//! - the camera's view: icons or cars, the icons' size step.
//!
//! The train hovered in the sidebar's list (`hoveredTrainId`) is drawn highlighted; the list comes
//! with stage 7 (its hover sets [`Trains::hovered`]), so until then no train is.
//!
//! The selected train ([`state::trains::Trains::selected`](crate::state::trains::Trains::selected),
//! `selectedTrain`), in `buildLayers()`'s order (G:2732-2737):
//! - its halo and ring (`trainSelectionLayers`, [`ring_of`]) are worked out **before** the trains
//!   are stepped on to this refresh, so they stand where the train was drawn at the last refresh
//!   (its report before the first); at 50 ms refreshes that is a train's step of a frame behind,
//!   as today's;
//! - its route (`trainRouteLayers`, [`route_lines`]), while the state has one
//!   ([`state::trains::Trains::route`](crate::state::trains::Trains::route)), is worked out after:
//!   from where the train is drawn now (its report when it is not drawn) along the frames of its
//!   row of the list the trains are drawn from, the search on along the route starting where the
//!   last refresh found it (`routeCursor`, kept here; a new route, numbered by
//!   [`route_generation`](crate::state::trains::Trains::route_generation), starts again from its
//!   start). Found off its route, the state is told
//!   ([`route_off`](crate::state::trains::Trains::route_off), `ontrainrouteoff`), which asks for it
//!   again at most every 3 s.

use rustradar_core::model::Train;
use rustradar_core::train_route::RouteCursor;
use rustradar_map::SceneInput;
use rustradar_map::layers::train_route::{self, ring_of, route_lines};
use rustradar_scene::trains::{TrainBuilder, TrainsScene};

use crate::state::AppState;

/// Trains on the map: icons, cars, the selected train's route and ring.
#[derive(Debug, Default)]
pub struct Trains {
    builder: TrainBuilder,
    /// What the list was last taken in for: the state's generation and the local clock's second.
    list_key: Option<(u64, i64)>,
    /// The id of the train hovered in the sidebar's list (`hoveredTrainId`), drawn highlighted.
    pub hovered: Option<String>,
    /// The trains of the latest scene refresh.
    pub latest: TrainsScene,
    /// Where along its route the selected train was last found (`routeCursor`): the next refresh
    /// searches on from there.
    route_cursor: Option<RouteCursor>,
}

impl Trains {
    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]); tells
    /// the train state when the selected train is off its route.
    pub fn build(&mut self, state: &mut AppState, scene: &mut SceneInput) {
        let now_ms = scene.local_now_ms.floor() as i64;
        // the selected train's halo and ring, before the trains step on (G:2732)
        let ring = state.trains.selected().and_then(|train| ring_of(train, self.builder.drawn(), &scene.view));
        let on_map = state.trains.on_map();
        let list: Vec<&Train> = on_map.iter().collect();
        let key = (state.trains.on_map_generation(), now_ms.div_euclid(1000));
        let new_list = self.list_key != Some(key);
        self.list_key = Some(key);
        self.latest = self.builder.build(&list, new_list, &scene.view, self.hovered.as_deref(), now_ms);
        // close in, each train's cars where it is drawn (T3)
        if self.latest.cars {
            let models = rustradar_map::layers::train_cars::models();
            self.latest.car_layers = self.builder.car_layers(&list, &scene.view, scene.size, models, now_ms);
        }
        scene.trains.scene = self.latest.clone();
        // the selected train's way ahead from where it is drawn now (G:2737), under the trains
        let (route, on) = self.route(&state.trains, &list, now_ms);
        scene.trains.route = train_route::Input { ring, ..route };
        if !on {
            state.trains.route_off(scene.local_now_ms);
        }
    }

    /// `trainRouteLayers(trainRoute, selectedTrain)` (G:2091-2102) for the trains of `list` as
    /// drawn at this refresh, at `now_ms`: the route's lines and stops, and whether the selected
    /// train is on its route (true without a route or a selected train). Keeps the cursor.
    fn route(&mut self, trains: &crate::state::trains::Trains, list: &[&Train], now_ms: i64) -> (train_route::Input, bool) {
        let (Some(route), Some(train)) = (trains.route(), trains.selected()) else {
            return (train_route::Input::default(), true);
        };
        let drawn = self.builder.drawn();
        // where it is drawn, and the frames it is drawn from (its row of the drawn list), else its
        // report and its own frames
        let row = drawn.index_of(&train.id).and_then(|i| Some((drawn.positions.get(i)?, *list.get(i)?)));
        let (at, frames) = match row {
            Some((p, row)) => ([p[0], p[1]], row.frames.as_slice()),
            None => ([train.lon, train.lat], train.frames.as_slice()),
        };
        let (input, routed) = route_lines(route, trains.route_generation(), at, frames, self.route_cursor.as_ref(), now_ms);
        self.route_cursor = Some(routed.cursor);
        (input, routed.on)
    }

    /// Where the trains are drawn (for the cars, the route and the ring).
    pub fn builder(&self) -> &TrainBuilder {
        &self.builder
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rustradar_core::geo::{Area, Basemap};
    use rustradar_core::model::{RouteStop, TrafikverketKeyStatus, TrainRoute};
    use rustradar_core::settings::Mode;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::scene_builder::SceneBuilder;
    use crate::state::aircraft::Answer;
    use crate::state::trains::{TrainFeeds, TrainStream, Trains as TrainState};
    use crate::state::{Clocks, Startup};

    /// 2026-06-21 10:00 UTC, the shots' clock.
    const NOW_MS: f64 = 1_782_036_000_000.0;
    const V3: ViewState = ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, bearing: 0.0, pitch: 0.0, position: None };

    fn demo_state(view: ViewState) -> AppState {
        let startup = Startup { basemap: Some(Basemap::Dark), fixture: None, demo: true };
        let mut state = AppState::new(startup, None, None, None, view, [1440.0, 900.0]);
        state.poll(NOW_MS);
        state
    }

    fn build(state: &mut AppState, builder: &mut SceneBuilder, local_ms: f64) -> SceneInput {
        builder.build(state, Clocks { local_ms, feed_ms: local_ms }, 1.0)
    }

    #[test]
    fn the_demo_trains_are_icons_at_v3_in_their_colours() {
        let mut state = demo_state(V3);
        assert_eq!(state.trains.on_map().len(), 6, "the demo trains are on the map");
        let mut builder = SceneBuilder::default();
        let scene = build(&mut state, &mut builder, NOW_MS);
        let icons = &scene.trains.scene.icons;
        assert!(!scene.trains.scene.cars);
        assert_eq!(icons.len(), 6);
        let names: Vec<&str> = icons.iter().map(|i| i.name.as_str()).collect();
        // Skånetrafiken, SJ, Öresundståg, SJ, Snälltåget, and Inlandsbanan (no colour of its own:
        // its kind's)
        assert_eq!(names, ["car-op4", "car-op0", "car-op5", "car-op0", "car-op7", "car-regional"]);
        let colours: Vec<[u8; 4]> = icons.iter().map(|i| i.color).collect();
        assert_eq!(
            colours,
            [[196, 92, 222, 240], [232, 232, 236, 240], [150, 132, 246, 240], [232, 232, 236, 240], [120, 214, 92, 240], [96, 212, 160, 240]]
        );
        // turned to their headings, at their reported places at the start of their frames
        let angles: Vec<f64> = icons.iter().map(|i| i.angle).collect();
        for (angle, heading) in angles.iter().zip([70.0, 250.0, 160.0, 340.0, 20.0, 110.0]) {
            assert!((angle + heading).abs() < 0.01, "{angles:?}");
        }
        for (icon, train) in icons.iter().zip(state.trains.on_map().iter()) {
            assert!((icon.position[0] - train.lon).abs() < 1e-9 && (icon.position[1] - train.lat).abs() < 1e-9);
            assert_eq!(icon.size_px, rustradar_core::trains::train_icon_size(train.kind, Some(f64::from(train.heading.unwrap())), 9.0));
            assert!(!icon.highlighted);
        }
    }

    #[test]
    fn the_moving_demo_trains_run_on_and_the_list_is_taken_in_again_each_second() {
        let mut state = demo_state(V3);
        let mut builder = SceneBuilder::default();
        let metres = |a: &[f64; 3], b: &[f64; 3]| {
            let k = a[1].to_radians().cos();
            ((b[1] - a[1]) * 111_320.0).hypot((b[0] - a[0]) * 111_320.0 * k)
        };
        let start = build(&mut state, &mut builder, NOW_MS).trains.scene.icons;
        // 0.9 s on, within the same second: the two moving ones (14 and 22 m/s) have gone on along
        // their frames, the others stand
        let on = build(&mut state, &mut builder, NOW_MS + 900.0).trains.scene.icons;
        assert!((metres(&start[0].position, &on[0].position) - 12.6).abs() < 0.01, "{}", metres(&start[0].position, &on[0].position));
        assert!((metres(&start[1].position, &on[1].position) - 19.8).abs() < 0.01);
        for k in 2..6 {
            assert_eq!(start[k].position, on[k].position);
        }
        // the next second's list is taken in again: each train eases on from where it is drawn, so
        // at first it is still there
        let next = build(&mut state, &mut builder, NOW_MS + 1000.0).trains.scene.icons;
        assert!(builder.trains.list_key.is_some_and(|(_, s)| s == NOW_MS as i64 / 1000 + 1));
        for k in 0..2 {
            assert!(metres(&on[k].position, &next[k].position) < 1e-6, "{k}");
        }
        // and then on, never back, catching up with its frames (2.1 m on them in 150 ms, less the
        // 1.4 m eased away)
        let after = build(&mut state, &mut builder, NOW_MS + 1050.0).trains.scene.icons;
        let gone = metres(&on[0].position, &after[0].position);
        assert!(gone > 0.5 && gone < 0.8, "{gone}");
        // hovered in the list: highlighted
        builder.trains.hovered = Some("trafikverket:demo:2".into());
        let icons = build(&mut state, &mut builder, NOW_MS + 1100.0).trains.scene.icons;
        assert_eq!(icons.iter().map(|i| i.highlighted).collect::<Vec<_>>(), [false, false, true, false, false, false]);
    }

    /// The selected train's halo and ring stand where it was drawn at the last refresh (its report
    /// before the first), sized for its icon at this zoom; a demo train has no route.
    #[test]
    fn the_selected_trains_ring_is_where_the_last_refresh_drew_it() {
        let mut state = demo_state(V3);
        let train = state.trains.on_map().get(0).cloned().expect("Skånetrafiken's demo train");
        state.select_train(Some(train.clone()), NOW_MS);
        state.poll(NOW_MS);
        assert_eq!(state.trains.selected_id(), Some(train.id.as_str()));
        let mut builder = SceneBuilder::default();
        let first = build(&mut state, &mut builder, NOW_MS);
        let ring = first.trains.route.ring.expect("a ring");
        assert_eq!(ring.at, [train.lon, train.lat]);
        // a regional train's 21 px at zoom 9: `Math.round(10.5 + 5)`
        let size = rustradar_core::trains::train_icon_size(train.kind, Some(70.0), 9.0);
        assert_eq!(ring.radius_px, rustradar_core::num::js_round(size / 2.0 + 5.0));
        assert_eq!(ring.radius_px, 16.0);
        // no route: nothing else, and nothing asked
        assert_eq!(first.trains.route, train_route::Input { ring: Some(ring), ..Default::default() });
        // 0.9 s on the train has gone 12.6 m, the ring is still where the last refresh drew it
        let on = build(&mut state, &mut builder, NOW_MS + 900.0);
        assert_eq!(on.trains.route.ring.map(|r| r.at), Some([train.lon, train.lat]));
        let drawn = on.trains.scene.icons[0].position;
        assert!((drawn[0] - train.lon).abs() > 1e-5, "it moved: {drawn:?}");
        // and at the next refresh there
        let next = build(&mut state, &mut builder, NOW_MS + 950.0);
        assert_eq!(next.trains.route.ring.map(|r| r.at), Some([drawn[0], drawn[1]]));
        // round the back of the globe, none; deselected, none
        state.controller.set_view(ViewState { longitude: -161.93, latitude: -59.33, zoom: 3.0, ..V3 });
        assert!(build(&mut state, &mut builder, NOW_MS + 1000.0).trains.route.ring.is_none());
        state.controller.set_view(V3);
        state.select_train(None, NOW_MS + 1000.0);
        state.poll(NOW_MS + 1000.0);
        assert_eq!(build(&mut state, &mut builder, NOW_MS + 1050.0).trains.route, train_route::Input::default());
    }

    /// Canned train feeds: the key "ok", `list` in view, `route` for every train, every answer at
    /// once; counts the route requests.
    struct Feeds {
        list: Vec<Train>,
        route: TrainRoute,
        routes: Arc<AtomicUsize>,
    }

    impl TrainFeeds for Feeds {
        fn key_status(&mut self) -> Answer<Result<TrafikverketKeyStatus, String>> {
            Answer::ready(Ok(TrafikverketKeyStatus::Ok))
        }

        fn trains_in_view(&mut self, _area: Area, _zoom: f64) -> Answer<Result<Vec<Train>, String>> {
            Answer::ready(Ok(self.list.clone()))
        }

        fn watch_trains(&mut self, _ids: Vec<String>) {}

        fn hide_train_operators(&mut self, _names: Vec<String>) {}

        fn follow_train(&mut self, _id: &str) -> Result<TrainStream, String> {
            Ok(TrainStream::Channel(std::sync::mpsc::channel().1))
        }

        fn unfollow_train(&mut self) {}

        fn train_route(&mut self, _id: &str) -> Answer<Result<TrainRoute, String>> {
            self.routes.fetch_add(1, Ordering::SeqCst);
            Answer::ready(Ok(self.route.clone()))
        }
    }

    /// `[lon, lat]` `m` metres east and `n` north of V3's centre (flat metres).
    fn at(m: f64, n: f64) -> [f64; 2] {
        [18.07 + m / (111_320.0 * 59.33f64.to_radians().cos()), 59.33 + n / 111_320.0]
    }

    /// A train at V3's centre going east at 20 m/s from `NOW_MS`, standing at 1 km from 50 s on.
    fn eastbound() -> Train {
        let mut train = crate::dev::demo_trains(59.33, 18.07, NOW_MS as i64).remove(0);
        train.id = "trafikverket:2026-06-21:8710".into();
        train.heading = Some(90.0);
        train.tail = Vec::new();
        let frame = |m: f64, s: i64| {
            let [lon, lat] = at(m, 0.0);
            (lat, lon, NOW_MS as i64 + s * 1000)
        };
        train.frames = vec![frame(0.0, 0), frame(200.0, 10), frame(400.0, 20), frame(1000.0, 50), frame(1000.0, 110)];
        [train.lon, train.lat] = at(0.0, 0.0);
        train
    }

    /// A route east from 200 m behind the train for 3 km, a point every 100 m, stopping at 1 km and
    /// 3 km, passing through at 2 km; `north` metres off the train's track.
    fn route_east(north: f64) -> TrainRoute {
        let stop = |name: &str, m: f64, stops: bool| {
            let [lon, lat] = at(m, north);
            RouteStop { name: name.into(), lat, lon, track: None, stops }
        };
        TrainRoute {
            segments: vec![(-2..=30).map(|k| at(f64::from(k) * 100.0, north)).collect()],
            stops: vec![stop("Ett", 1000.0, true), stop("Två", 2000.0, false), stop("Tre", 3000.0, true)],
            complete: true,
        }
    }

    /// The whole state on [`Feeds`] at `view` with `train` in view and selected, its route in.
    fn selected_on_route(view: ViewState, train: &Train, route: TrainRoute) -> (AppState, Arc<AtomicUsize>) {
        let routes = Arc::new(AtomicUsize::new(0));
        let mut state = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, view, [1440.0, 900.0]);
        state.persisted.settings.mode = Mode::Trains;
        state.trains = TrainState::with_feeds(Some(Box::new(Feeds { list: vec![train.clone()], route, routes: Arc::clone(&routes) })));
        // the key, then the trains once the camera has stood 600 ms
        for t in [NOW_MS - 1000.0, NOW_MS - 1000.0, NOW_MS - 400.0, NOW_MS - 400.0] {
            state.poll(t);
        }
        assert_eq!(state.trains.on_map().len(), 1, "the train is on the map");
        state.select_train(Some(train.clone()), NOW_MS - 400.0);
        state.poll(NOW_MS - 400.0);
        state.poll(NOW_MS - 400.0);
        assert!(state.trains.route().is_some(), "the route is in");
        assert_eq!(routes.load(Ordering::SeqCst), 1);
        (state, routes)
    }

    /// The route from where the selected train is drawn now along its route, the stops it stops
    /// at, the way to where its frames stand; the search goes on from the last refresh's place.
    #[test]
    fn the_route_runs_on_from_where_the_selected_train_is_drawn() {
        let train = eastbound();
        let (mut state, routes) = selected_on_route(V3, &train, route_east(0.0));
        let mut builder = SceneBuilder::default();
        let scene = build(&mut state, &mut builder, NOW_MS + 7_500.0);
        let route = &scene.trains.route;
        // drawn 150 m on along its frames: the head from there onto the route and to the end of
        // its leg, the rest on from there
        let drawn = scene.trains.scene.icons[0].position;
        assert!((drawn[0] - at(150.0, 0.0)[0]).abs() < 1e-9 && (drawn[1] - 59.33).abs() < 1e-9, "{drawn:?}");
        assert_eq!(route.head.len(), 3);
        assert_eq!(route.head[0], [drawn[0], drawn[1]]);
        assert!((route.head[1][0] - drawn[0]).abs() < 1e-12 && (route.head[1][1] - 59.33).abs() < 1e-12, "{:?}", route.head[1]);
        assert_eq!(route.head[2], at(200.0, 0.0));
        assert_eq!(route.rest.len(), 1);
        assert_eq!(route.rest[0].first(), Some(&at(200.0, 0.0)));
        assert_eq!(route.rest[0].last(), Some(&at(3000.0, 0.0)));
        assert_eq!(route.stations, vec![at(1000.0, 0.0), at(3000.0, 0.0)]);
        // where it stands next: its frames after now up to the first standing one
        assert_eq!(route.to_stop, vec![[drawn[0], drawn[1]], at(200.0, 0.0), at(400.0, 0.0), at(1000.0, 0.0)]);
        // the ring where it was drawn before (its report: this is the first refresh)
        assert_eq!(route.ring.map(|r| r.at), Some(at(0.0, 0.0)));
        // refreshed every 50 ms on: the head always from where it is drawn, the same lines while
        // it is on the same leg (the cursor kept them), the lines from the next leg's end once
        // it is on that
        let mut rest = Arc::clone(&route.rest);
        let mut legs = 0;
        for k in 1..=100 {
            let scene = build(&mut state, &mut builder, NOW_MS + 7_500.0 + f64::from(k) * 50.0);
            let drawn = scene.trains.scene.icons[0].position;
            let route = scene.trains.route;
            assert_eq!(route.head[0], [drawn[0], drawn[1]]);
            if !Arc::ptr_eq(&route.rest, &rest) {
                legs += 1;
                assert_eq!(route.rest[0].first(), Some(&route.head[2]));
                assert!(route.rest[0].len() < rest[0].len(), "on along the route");
                rest = route.rest;
            }
        }
        // from 150 m to 200-odd m (the list taken in again each second eases it on behind its
        // frames): one leg on
        assert_eq!(legs, 1);
        assert_eq!(rest[0].first(), Some(&at(300.0, 0.0)));
        // on its route throughout: nothing asked again
        assert_eq!(routes.load(Ordering::SeqCst), 1);
    }

    /// Off its route (more than 300 m from it) nothing of it is drawn but its stops, and the train
    /// state asks for it again, at most every 3 s.
    #[test]
    fn off_its_route_the_route_is_asked_again() {
        let train = eastbound();
        let (mut state, routes) = selected_on_route(V3, &train, route_east(500.0));
        let mut builder = SceneBuilder::default();
        // asked 2.6 s before: not yet
        let scene = build(&mut state, &mut builder, NOW_MS + 2_500.0);
        assert!(scene.trains.route.head.is_empty() && scene.trains.route.rest.is_empty());
        assert_eq!(scene.trains.route.stations.len(), 2);
        assert_eq!(routes.load(Ordering::SeqCst), 1);
        // 3 s after the last ask: again
        build(&mut state, &mut builder, NOW_MS + 2_600.0);
        assert_eq!(routes.load(Ordering::SeqCst), 2);
        // its answer in, still off: not again for 3 s
        state.poll(NOW_MS + 2_700.0);
        build(&mut state, &mut builder, NOW_MS + 5_550.0);
        assert_eq!(routes.load(Ordering::SeqCst), 2);
        build(&mut state, &mut builder, NOW_MS + 5_600.0);
        assert_eq!(routes.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn close_in_no_icons_and_none_without_trains() {
        let close = ViewState { zoom: 15.0, ..V3 };
        let mut state = demo_state(close);
        let mut builder = SceneBuilder::default();
        let scene = build(&mut state, &mut builder, NOW_MS);
        assert!(scene.trains.scene.cars);
        assert!(scene.trains.scene.icons.is_empty());
        // without --demo, no fixture and no backend: no trains, no icons
        let mut state = AppState::new(Startup { basemap: Some(Basemap::Dark), fixture: None, demo: false }, None, None, None, V3, [1440.0, 900.0]);
        state.poll(NOW_MS);
        let scene = build(&mut state, &mut builder, NOW_MS);
        assert_eq!(scene.trains.scene, TrainsScene::default());
    }
}
