//! The aircraft (item A3; doc 03 §1–2, §5.1–5.3, §6): at each scene refresh, the flights shown,
//! posed by the motion model at the feed clock, turned into what the aircraft layers draw
//! ([`rustradar_scene::aircraft::AircraftScene`]: mesh groups with their gear, icons, label
//! candidates, the taxiways wanted).
//!
//! What it reads of the state, as `buildLayers()` reads the globe's props:
//! - `flights`: [`state::aircraft::Aircraft::shown`](crate::state::aircraft::Aircraft::shown), the
//!   filtered flights with the selected row in its place;
//! - `exaggeration`, `aircraftLabels` (the label font is embedded, so always ready), and
//!   `showAircraft` (the mode or "also show" has aircraft) from the settings;
//! - the published models, read once ([`assets::published`]): `aircraftModels.loaded` and
//!   `forFlight`;
//! - `motion.pose(f, now)` with `now` the feed clock, for each flight not culled: this needs the
//!   motion model mutably (a pose takes the flight's report in and animates the gear), which is
//!   why [`super::SceneBuilder::build`] hands the builders `&mut AppState`;
//! - `liveries.forFlight(f)` at the local clock, for each aircraft drawn with airline colours
//!   ([`state::liveries::Liveries::for_flight`](crate::state::liveries::Liveries::for_flight)),
//!   with the reference data's airlines.
//!
//! - the selected flight's row shown and whether the camera follows it (`selected`, `follow`:
//!   [`state::flight`](crate::state::flight), item A8): the row takes the feed's place in the
//!   flights drawn.
//!
//! - the flight under the pointer (`hoveredId`: [`state::pick`](crate::state::pick), item A9),
//!   drawn highlighted.
//!
//! The result is [`Aircraft::latest`], handed to the aircraft layers as `SceneInput`'s
//! `aircraft.scene` (`rustradar_map::layers::aircraft::Input`, item A4).

use rustradar_core::settings::shown_kinds;
use rustradar_map::SceneInput;
use rustradar_map::layers::aircraft::assets;
use rustradar_scene::aircraft::{AircraftBuilder, AircraftScene, Inputs};
use rustradar_scene::aircraft_labels::Labeller;

use crate::state::AppState;

/// The aircraft: icons, meshes, gear and label candidates for every aircraft shown.
#[derive(Debug, Default)]
pub struct Aircraft {
    builder: AircraftBuilder,
    /// The aircraft of the latest scene refresh.
    pub latest: AircraftScene,
    /// The aircraft labels' set across refreshes (item A7).
    labels: Labeller,
}

impl Aircraft {
    /// What the drawing reads of the state and of the scene being built.
    pub fn inputs(state: &AppState, scene: &SceneInput) -> Inputs {
        let settings = &state.persisted.settings;
        Inputs {
            view: scene.view,
            size: scene.size,
            exaggeration: settings.exaggeration,
            show_aircraft: shown_kinds(settings.mode, &settings.also_show).aircraft,
            // the map's label font is embedded: ready from the first frame (`fontsReady`)
            labelling: settings.aircraft_labels,
            selected: state.flight.selected().map(|f| f.id),
            follow: state.flight.following(),
            hovered: state.pick.hovered_id(),
        }
    }

    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]).
    pub fn build(&mut self, state: &mut AppState, scene: &mut SceneInput) {
        let inputs = Self::inputs(state, scene);
        let label_scale = state.persisted.settings.label_size;
        let (feed_now, local_now) = (scene.feed_now_ms, scene.local_now_ms);
        let AppState { aircraft, motion, liveries, taxiways, flight, .. } = state;
        let reference = aircraft.reference().map(|r| &**r);
        let shown = aircraft.shown(flight.selected());
        self.latest = self.builder.build(
            &inputs,
            assets::published(),
            shown.iter(),
            |f| motion.pose(f, feed_now),
            |f| liveries.for_flight(f, reference, local_now),
        );
        // the taxiway networks of the airports aircraft are on the ground at, close in (A11)
        taxiways.taxiways_for(&self.latest.taxi_wanted, motion, local_now);
        // what the aircraft layers draw until the next refresh
        scene.aircraft.scene = self.latest.clone();
        // their labels (A7), laid out again only when the feed, the height line or the aircraft
        // labelled change
        scene.aircraft.labels = self.labels.build(&self.latest, inputs.labelling, aircraft.generation(), |row| shown.get(row), label_scale);
    }
}

#[cfg(test)]
#[path = "../../tests/common/fixtures.rs"]
mod fixtures;

#[cfg(test)]
mod tests {
    use rustradar_core::model::{BoundingBox, LiveSnapshot, SessionInfo, TopFlight};
    use rustradar_scene::aircraft::GroupModel;
    use rustradar_scene::camera::ViewState;
    use serde_json::Value;

    use super::fixtures::{self, ARLANDA, STOCKHOLM, WORLD};
    use super::*;
    use crate::scene_builder::SceneBuilder;
    use crate::state::aircraft::{Answer, Feeds, LoadedReference};
    use crate::state::{Startup, aircraft};

    /// The clock of the app's shots (`tests/common`'s `NOW_MS`): 2026-06-21 10:00 UTC.
    const NOW_MS: f64 = 1_782_036_000_000.0;

    /// The synthetic snapshot, answered at once.
    struct Synthetic(LiveSnapshot);

    impl Feeds for Synthetic {
        fn live_flights(&mut self, _area: Option<BoundingBox>, _categories: Option<Vec<u8>>) -> Answer<Result<LiveSnapshot, String>> {
            Answer::ready(Ok(self.0.clone()))
        }

        fn top_flights(&mut self) -> Answer<Result<Vec<TopFlight>, String>> {
            Answer::ready(Ok(Vec::new()))
        }

        fn reference_data(&mut self) -> Answer<Result<LoadedReference, String>> {
            Answer::ready(Err("no reference data in this test".into()))
        }

        fn session_info(&mut self) -> Option<SessionInfo> {
            None
        }
    }

    /// `(lat, lon, zoom, pitch, bearing)`.
    type At = (f64, f64, f64, f64, f64);

    /// The named views of plan section 8.
    const V1: At = (35.0, 10.0, 1.9, 0.0, 0.0);
    const V3: At = (59.33, 18.07, 9.0, 0.0, 0.0);
    const V4: At = (59.6519, 17.9186, 13.5, 45.0, 30.0);

    /// The aircraft of a scene refresh at `at` on the shots' 1100 × 720 canvas at the shots' clock,
    /// with the synthetic snapshot in and aircraft labels on (the rest default: exaggeration 10),
    /// through the whole scene builder.
    fn scene_at((latitude, longitude, zoom, pitch, bearing): At) -> AircraftScene {
        let view = ViewState { longitude, latitude, zoom, bearing, pitch, position: None };
        let mut state = AppState::new(Startup::default(), None, None, None, view, [1100.0, 720.0]);
        state.aircraft = aircraft::Aircraft::with_feeds(Some(Box::new(Synthetic(fixtures::synthetic_snapshot(NOW_MS)))));
        state.persisted.settings.aircraft_labels = true;
        state.poll(NOW_MS);
        assert_eq!(state.aircraft.flights().len(), WORLD + STOCKHOLM + ARLANDA, "the snapshot is in");
        let clocks = state.clocks(NOW_MS);
        assert_eq!(clocks.feed_ms, NOW_MS, "the feed clock synced to the snapshot's server time");
        let mut builder = SceneBuilder::default();
        builder.build(&mut state, clocks, 1.0);
        std::mem::take(&mut builder.aircraft.latest)
    }

    /// `(group id, instances, gear)` of each mesh group, in order.
    fn groups(scene: &AircraftScene) -> Vec<(String, usize, usize)> {
        scene.groups.iter().map(|g| (g.id(), g.instances.len(), g.gear.len())).collect()
    }

    fn owned(groups: &[(&str, usize, usize)]) -> Vec<(String, usize, usize)> {
        groups.iter().map(|&(id, n, gear)| (id.to_owned(), n, gear)).collect()
    }

    #[test]
    fn golden_counts_at_v1() {
        let scene = scene_at(V1);
        let f = scene.frame;
        assert!(f.use_icons && !f.flat);
        assert_eq!((f.depth, f.livery_amount, f.detail), (0.0, 0.0, 0));
        // the near side of the globe: published models as icons, the rest as flat built-in meshes
        assert_eq!(scene.icons.len(), 134);
        assert_eq!(groups(&scene), owned(&[("k-balloon", 9, 0), ("k-ground", 10, 0), ("k-narrow", 22, 0)]));
        assert!(scene.labelled.is_empty(), "below zoom 4 only the selected aircraft is labelled");
        assert!(scene.taxi_wanted.is_empty());
    }

    #[test]
    fn golden_counts_at_v3() {
        let scene = scene_at(V3);
        assert!(scene.frame.use_icons);
        assert_eq!((scene.frame.depth, scene.frame.livery_amount), (1.0, 1.0));
        assert_eq!(scene.icons.len(), 25);
        assert_eq!(groups(&scene), owned(&[("k-balloon", 3, 0), ("k-ground", 3, 0), ("k-narrow", 5, 0)]));
        // every one drawn but the nameless
        assert_eq!(scene.labelled.len(), 33);
        assert!(scene.groups.iter().flat_map(|g| &g.instances).all(|m| m.parts.is_some()), "liveries at zoom 9");
    }

    #[test]
    fn golden_counts_at_v4() {
        let scene = scene_at(V4);
        let f = scene.frame;
        assert!(!f.use_icons && f.flat && f.taxiways);
        assert_eq!(f.detail, 1);
        assert_eq!(scene.icons.len(), 0);
        // Arlanda's six on the ground and one arrival overhead; gear on the two published models
        // with gear standing on it (the helicopter has none)
        assert_eq!(
            groups(&scene),
            owned(&[
                ("t-b773", 1, 0),
                ("t-q400", 1, 1),
                ("t-pa28", 1, 1),
                ("t-heli", 1, 0),
                ("k-balloon", 1, 0),
                ("k-ground", 1, 0),
                ("k-narrow", 1, 0),
            ])
        );
        assert_eq!(scene.labelled.len(), 7);
        assert_eq!(scene.taxi_wanted.len(), 7);
        for g in &scene.groups {
            if let GroupModel::Published { lod, .. } = g.model {
                assert_eq!(lod, 1, "{}", g.id());
            }
        }
    }

    fn close(actual: f64, expected: &Value, tolerance: f64, what: &str) {
        let expected = expected.as_f64().unwrap_or_else(|| panic!("{what}: {expected}"));
        assert!((actual - expected).abs() <= tolerance * expected.abs().max(1.0), "{what}: {actual} against today's {expected}");
    }

    fn ids(values: &Value) -> Vec<u32> {
        values.as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect()
    }

    /// Every view of `tests/data/aircraft-scene.json`, which today's loop wrote for the same
    /// snapshot under bun (`scripts/port/aircraft-scene.ts`): the same icons, groups, heights,
    /// gear, labels and taxiway asks, sizes and angles to 1e-9.
    #[test]
    fn matches_todays_loop_on_the_synthetic_snapshot() {
        let records: Vec<Value> = serde_json::from_str(include_str!("../../tests/data/aircraft-scene.json")).unwrap();
        assert_eq!(records.len(), 6);
        for r in &records {
            let name = r["name"].as_str().unwrap();
            assert_eq!((r["width"].as_f64(), r["height"].as_f64(), r["exaggeration"].as_f64()), (Some(1100.0), Some(720.0), Some(10.0)));
            let v = &r["view"];
            let at = |k: &str| v[k].as_f64().unwrap();
            let scene = scene_at((at("latitude"), at("longitude"), at("zoom"), at("pitch"), at("bearing")));
            let f = scene.frame;
            assert_eq!(f.flat, r["flat"].as_bool().unwrap(), "{name}");
            assert_eq!(f.depth, r["depth"].as_f64().unwrap(), "{name}");
            assert_eq!(f.detail as u64, r["detail"].as_u64().unwrap(), "{name}");
            assert_eq!(f.use_icons, r["useIcons"].as_bool().unwrap(), "{name}");

            let icons = r["icons"].as_array().unwrap();
            assert_eq!(scene.icons.len(), icons.len(), "{name}: icons");
            for (icon, want) in scene.icons.iter().zip(icons) {
                let what = format!("{name} icon {}", icon.id);
                assert_eq!(u64::from(icon.id), want["id"].as_u64().unwrap(), "{what}");
                assert_eq!(icon.name, want["name"].as_str().unwrap(), "{what}");
                close(icon.size_px, &want["size"], 1e-12, &what);
                close(icon.outline_size_px, &want["outline"], 1e-12, &what);
                close(icon.angle, &want["angle"], 1e-9, &what);
            }

            let groups = r["groups"].as_array().unwrap();
            assert_eq!(scene.groups.iter().map(|g| g.id()).collect::<Vec<_>>(), groups.iter().map(|g| g["id"].as_str().unwrap()).collect::<Vec<_>>(), "{name}");
            for (g, want) in scene.groups.iter().zip(groups) {
                let what = format!("{name} {}", g.id());
                assert_eq!(g.instances.iter().map(|m| m.id).collect::<Vec<_>>(), ids(&want["ids"]), "{what}");
                for (m, z) in g.instances.iter().zip(want["z"].as_array().unwrap()) {
                    close(m.position[2], z, 1e-9, &format!("{what} {} height", m.id));
                }
                assert_eq!(g.gear.iter().map(|gear| g.instances[gear.instance].id).collect::<Vec<_>>(), ids(&want["gear"]), "{what} gear");
            }

            let labelled = r["labelled"].as_array().unwrap();
            assert_eq!(scene.labelled.iter().map(|l| l.id).collect::<Vec<_>>(), labelled.iter().map(|l| l["id"].as_u64().unwrap() as u32).collect::<Vec<_>>(), "{name}: labelled");
            for (l, want) in scene.labelled.iter().zip(labelled) {
                let what = format!("{name} label {}", l.id);
                close(l.position[2], &want["z"], 1e-9, &what);
                close(l.size_m, &want["size"], 1e-12, &what);
            }
            assert_eq!(scene.taxi_wanted.iter().map(|(id, _)| *id).collect::<Vec<_>>(), ids(&r["taxi"]), "{name}: taxiways");
        }
    }

    #[test]
    fn aircraft_off_draw_nothing() {
        let view = ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, bearing: 0.0, pitch: 0.0, position: None };
        let mut state = AppState::new(Startup::default(), None, None, None, view, [1100.0, 720.0]);
        state.aircraft = aircraft::Aircraft::with_feeds(Some(Box::new(Synthetic(fixtures::synthetic_snapshot(NOW_MS)))));
        state.poll(NOW_MS);
        state.persisted.settings.mode = rustradar_core::settings::Mode::Trains;
        let clocks = state.clocks(NOW_MS);
        let mut builder = SceneBuilder::default();
        builder.build(&mut state, clocks, 1.0);
        let scene = &builder.aircraft.latest;
        assert!(scene.icons.is_empty() && scene.groups.is_empty() && scene.labelled.is_empty());
    }
}
