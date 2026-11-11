//! Trains on the map (item T2; doc 02 §5.1–5.5; `trainLayers`, G:2031-2071): at each scene refresh,
//! the trains the train state puts on the map, drawn where they are expected now, as icons further
//! out ([`rustradar_scene::trains::TrainBuilder`]); cars close in (T3) and the selected train's
//! route and ring (T6) are other items'.
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

use rustradar_core::model::Train;
use rustradar_map::SceneInput;
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
}

impl Trains {
    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]).
    pub fn build(&mut self, state: &AppState, scene: &mut SceneInput) {
        let now_ms = scene.local_now_ms.floor() as i64;
        let on_map = state.trains.on_map();
        let list: Vec<&Train> = on_map.iter().collect();
        let key = (state.trains.on_map_generation(), now_ms.div_euclid(1000));
        let new_list = self.list_key != Some(key);
        self.list_key = Some(key);
        self.latest = self.builder.build(&list, new_list, &scene.view, self.hovered.as_deref(), now_ms);
        scene.trains.scene = self.latest.clone();
    }

    /// Where the trains are drawn (for the cars, the route and the ring).
    pub fn builder(&self) -> &TrainBuilder {
        &self.builder
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::scene_builder::SceneBuilder;
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
