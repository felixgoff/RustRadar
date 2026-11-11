//! Ships on the map (item H2; doc 02 §4.1–4.5; `shipLayers`, G:1826-1876): at each scene refresh,
//! the ships the ship state puts on the map, drawn where they are expected now, as icons, and close
//! in as models ([`rustradar_scene::ships::ShipBuilder`]); the models' meshes (H3) and the selected
//! ship's halo and ring (H4) are other items'.
//!
//! What it reads of the state, as `buildLayers()` reads the globe's `ships` prop:
//! - the list: [`state::ships::Ships::on_map`](crate::state::ships::Ships::on_map) (`shipsOnMap`:
//!   the ships the filters keep, the selected one after them when they leave it out), empty when
//!   ships are off the map;
//! - when that list is a new one: whenever
//!   [`on_map_generation`](crate::state::ships::Ships::on_map_generation) changes (today's page
//!   derives `shipsOnMap` from no clock, so unlike the trains' it is not new every second);
//! - the local clock in whole milliseconds (`Date.now()`): the ships' frames are timed on this
//!   machine's clock, not the flight feed's;
//! - the camera's view and the canvas's size: icons or models, the icons' sizes and angles.
//!
//! The ship hovered in the sidebar's list (`hoveredShipMmsi`) is drawn highlighted; the list comes
//! with stage 7 (its hover sets [`Ships::hovered`]), so until then no ship is.

use rustradar_map::SceneInput;
use rustradar_scene::ships::{ShipBuilder, ShipsScene};

use crate::state::AppState;

/// Ships on the map: icons, models, the selected ship's halo and ring.
#[derive(Debug, Default)]
pub struct Ships {
    builder: ShipBuilder,
    /// The state's list generation the list was last taken in for.
    list_key: Option<u64>,
    /// The MMSI of the ship hovered in the sidebar's list (`hoveredShipMmsi`), drawn highlighted.
    pub hovered: Option<u32>,
    /// The ships of the latest scene refresh.
    pub latest: ShipsScene,
}

impl Ships {
    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]).
    pub fn build(&mut self, state: &AppState, scene: &mut SceneInput) {
        let now_ms = scene.local_now_ms.floor() as i64;
        let key = state.ships.on_map_generation();
        let new_list = self.list_key != Some(key);
        self.list_key = Some(key);
        self.latest = self.builder.build(state.ships.on_map(), new_list, &scene.view, scene.size, self.hovered, now_ms);
        scene.ships.scene = self.latest.clone();
    }

    /// Where the ships are drawn (for the models, the ring and the camera's lock).
    pub fn builder(&self) -> &ShipBuilder {
        &self.builder
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::Basemap;
    use rustradar_core::model::Ship;
    use rustradar_core::settings::Mode;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::scene_builder::SceneBuilder;
    use crate::state::ships::FIXTURE_SHIPS;
    use crate::state::{Clocks, Startup};

    /// 2026-06-21 10:00 UTC, the shots' clock.
    const NOW_MS: f64 = 1_782_036_000_000.0;
    /// V6, Gothenburg harbour.
    const V6: ViewState = ViewState { longitude: 11.93, latitude: 57.70, zoom: 13.0, bearing: 0.0, pitch: 0.0, position: None };

    fn ship(mmsi: u32, lat: f64, lon: f64, kind: u8, size: u8, heading: Option<f32>) -> Ship {
        Ship { mmsi, lat, lon, name: format!("SHIP {mmsi}"), kind, size, heading, ..Ship::default() }
    }

    /// Three ships in the harbour: a small tug heading north-east (an icon at V6), a large tanker
    /// with its dimensions heading west (a model at V6), a moored pleasure boat; the first moves
    /// along frames.
    fn harbour() -> Vec<Ship> {
        let now = NOW_MS as i64;
        let mut tug = ship(265_000_001, 57.700, 11.930, 2, 0, Some(45.0));
        tug.frames = vec![(57.700, 11.930, now), (57.701, 11.932, now + 60_000)];
        let mut tanker = ship(265_000_002, 57.702, 11.920, 6, 2, Some(270.0));
        tanker.dims = Some([150, 40, 15, 15]);
        let boat = ship(265_000_003, 57.699, 11.940, 8, 0, None);
        vec![tug, tanker, boat]
    }

    /// The app's state at `view` with `ships` as the `--fixture` folder's ships, ships on the map.
    fn state_with(view: ViewState, ships: &[Ship], name: &str) -> AppState {
        let dir = std::env::temp_dir().join(format!("rustradar-h2-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FIXTURE_SHIPS), serde_json::to_vec(ships).unwrap()).unwrap();
        let startup = Startup { basemap: Some(Basemap::Satellite), fixture: Some(dir.clone()), demo: false };
        let mut state = AppState::new(startup, None, None, None, view, [1440.0, 900.0]);
        state.persisted.settings.mode = Mode::Ships;
        state.poll(NOW_MS);
        let _ = std::fs::remove_dir_all(&dir);
        state
    }

    fn build(state: &mut AppState, builder: &mut SceneBuilder, local_ms: f64) -> SceneInput {
        builder.build(state, Clocks { local_ms, feed_ms: local_ms }, 1.0)
    }

    #[test]
    fn the_fixture_ships_are_icons_and_models_at_v6() {
        let mut state = state_with(V6, &harbour(), "v6");
        assert_eq!(state.ships.on_map().len(), 3, "the fixture's ships are on the map");
        let mut builder = SceneBuilder::default();
        let scene = build(&mut state, &mut builder, NOW_MS);
        let ships = &scene.ships.scene;
        // the 190 m tanker is long enough on screen at zoom 13 to be a model; the others are icons
        assert_eq!(ships.icons.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["hull-2", "dot-8"]);
        assert_eq!(ships.icons.iter().map(|i| i.mmsi).collect::<Vec<_>>(), [265_000_001, 265_000_003]);
        assert_eq!(ships.icons.iter().map(|i| i.color).collect::<Vec<_>>(), [[72, 198, 194, 235], [192, 136, 236, 235]]);
        assert_eq!(ships.icons.iter().map(|i| i.size_px).collect::<Vec<_>>(), [15.0, 15.0 * 0.62]);
        assert!((ships.icons[0].angle + 45.0).abs() < 0.01, "{}", ships.icons[0].angle);
        assert_eq!(ships.models.len(), 1);
        assert_eq!(ships.models[0].models[0].index, 1);
        assert_eq!(ships.models[0].models[0].heading, 270.0);
        assert!(ships.icons.iter().all(|i| !i.highlighted));
        // zoomed out to 11, all three are icons
        state.controller.set_view(ViewState { zoom: 11.0, ..V6 });
        let scene = build(&mut state, &mut builder, NOW_MS);
        assert_eq!(scene.ships.scene.icons.len(), 3);
        assert!(scene.ships.scene.models.is_empty());
    }

    #[test]
    fn the_tug_runs_on_and_a_hovered_ship_is_highlighted() {
        let mut state = state_with(V6, &harbour(), "moving");
        let mut builder = SceneBuilder::default();
        let start = build(&mut state, &mut builder, NOW_MS).ships.scene.icons[0].position;
        let later = build(&mut state, &mut builder, NOW_MS + 30_000.0).ships.scene.icons[0].position;
        assert!((later[1] - 57.7005).abs() < 1e-9 && (later[0] - 11.931).abs() < 1e-9, "{later:?} from {start:?}");
        assert_eq!(builder.ships.list_key, Some(state.ships.on_map_generation()), "the same list: no new easing");
        builder.ships.hovered = Some(265_000_003);
        let icons = build(&mut state, &mut builder, NOW_MS + 30_000.0).ships.scene.icons;
        assert_eq!(icons.iter().map(|i| i.highlighted).collect::<Vec<_>>(), [false, true]);
    }

    #[test]
    fn no_ships_no_icons() {
        let mut state = AppState::new(Startup { basemap: Some(Basemap::Satellite), fixture: None, demo: false }, None, None, None, V6, [1440.0, 900.0]);
        state.poll(NOW_MS);
        let scene = build(&mut state, &mut SceneBuilder::default(), NOW_MS);
        assert_eq!(scene.ships.scene, ShipsScene::default());
    }
}
