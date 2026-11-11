//! Ships on the map (item H2; doc 02 §4.1–4.5; `shipLayers`, G:1826-1876): at each scene refresh,
//! the ships the ship state puts on the map, drawn where they are expected now, as icons, and close
//! in as models ([`rustradar_scene::ships::ShipBuilder`]; the models' meshes are item H3's); the
//! selected ship's halo and ring (item H4).
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
//! with stage 7 (its hover sets [`Ships::hovered`]), so until then no ship is. Close in, the model
//! under the pointer ([`state::pick::Pick::hovered_ship_model`](crate::state::pick::Pick::hovered_ship_model),
//! item H4) is highlighted as deck's `autoHighlight` does, unless a ship is hovered in the list
//! ([`ShipsScene::highlight`]).
//!
//! The selected ship ([`state::ships::Ships::selected`](crate::state::ships::Ships::selected),
//! `selectedShip`; item H4, `shipSelectionLayers`, G:2726): its halo and ring
//! ([`ring_of`]) round its **reported** position (not where it is drawn), sized at this refresh's
//! zoom, none while it is round the back of the globe; the map draws the halo under the ships and
//! the ring over them.

use rustradar_map::SceneInput;
use rustradar_scene::ships::{ShipBuilder, ShipsScene, ring_of};

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
        // the model under the pointer (H4: deck's `autoHighlight`)
        if let Some(model) = state.pick.hovered_ship_model() {
            self.latest.highlight(model);
        }
        scene.ships.scene = self.latest.clone();
        // the selected ship's halo and ring (H4), at its report
        scene.ships.ring = state.ships.selected().and_then(|ship| ring_of(ship, &scene.view));
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

    /// Item H4: the selected ship's halo and ring stand at its report, sized for its icon at the
    /// refresh's zoom; none without a selection.
    #[test]
    fn the_selected_ships_ring_is_at_its_report() {
        let mut state = state_with(V6, &harbour(), "ring");
        let mut builder = SceneBuilder::default();
        assert_eq!(build(&mut state, &mut builder, NOW_MS).ships.ring, None, "nothing selected");
        let tug = state.ships.on_map()[0].clone();
        state.select_ship(Some(tug.clone()), NOW_MS);
        // 30 s on the tug is drawn along its frames; the ring stays at its report
        let scene = build(&mut state, &mut builder, NOW_MS + 30_000.0);
        let ring = scene.ships.ring.expect("a ring");
        assert_eq!(ring.at, [tug.lon, tug.lat]);
        let drawn = scene.ships.scene.icons[0].position;
        assert!((drawn[1] - tug.lat).abs() > 1e-4, "the tug has moved on: {drawn:?}");
        // a small tug's 15 px icon at zoom 13, no dimensions: 7.5 + 5
        assert_eq!(ring.radius_px, 15.0 / 2.0 + 5.0);
        // the tanker close in: its 150 m bow at zoom 16 sets the radius
        let tanker = state.ships.on_map()[1].clone();
        state.select_ship(Some(tanker.clone()), NOW_MS);
        state.controller.set_view(ViewState { zoom: 16.0, ..V6 });
        let ring = build(&mut state, &mut builder, NOW_MS).ships.ring.expect("a ring");
        let mpp = rustradar_core::geo::meters_per_pixel(16.0, V6.latitude);
        assert_eq!((ring.at, ring.radius_px), ([tanker.lon, tanker.lat], 150.0 / mpp + 6.0));
        state.select_ship(None, NOW_MS);
        assert_eq!(build(&mut state, &mut builder, NOW_MS).ships.ring, None);
    }

    /// Item H4: the model under the pointer is highlighted at the next refresh, as deck's
    /// `autoHighlight` does, and not once the pointer leaves it.
    #[test]
    fn the_model_under_the_pointer_is_highlighted() {
        let mut state = state_with(V6, &harbour(), "hover");
        let mut builder = SceneBuilder::default();
        let scene = build(&mut state, &mut builder, NOW_MS);
        let model = scene.ships.scene.models[0].models[0];
        assert!(!model.highlighted);
        // the tanker's hull: 55 m west of its report (bow 150 m, stern 40 m, heading west)
        let viewport = rustradar_scene::camera::viewport_for(&V6, [1440.0, 900.0]);
        let west = 55.0 / (111_320.0 * model.position[1].to_radians().cos());
        let [x, y, _] = viewport.project([model.position[0] - west, model.position[1], 0.0]);
        let wants = state.pick_frame(&scene, Some([x, y]), false, &[], NOW_MS);
        assert!(wants.scene, "a new highlight refreshes the scene at once");
        assert!(state.pick.hovering(), "the pointing hand");
        let scene = build(&mut state, &mut builder, NOW_MS);
        assert!(scene.ships.scene.models[0].models[0].highlighted);
        let tip = state.pick.tooltip().expect("the tanker's tooltip");
        assert_eq!(tip.tip.rows[0].text, "SHIP 265000002");
        assert_eq!(tip.tip.rows[1].text, "Tanker · 190 m");
        // a ship hovered in the sidebar's list takes over
        builder.ships.hovered = Some(265_000_001);
        assert!(!build(&mut state, &mut builder, NOW_MS).ships.scene.models[0].models[0].highlighted);
        builder.ships.hovered = None;
        // off the hull
        let wants = state.pick_frame(&scene, Some([x, y + 200.0]), false, &[], NOW_MS);
        assert!(wants.scene);
        assert!(!build(&mut state, &mut builder, NOW_MS).ships.scene.models[0].models[0].highlighted);
    }
}
