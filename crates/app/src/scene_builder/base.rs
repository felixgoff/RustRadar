//! The base map: the vector map on the dark basemap (item B2; doc 01 §5.1, §5.6).
//!
//! Today's `buildLayers` (G:2668-2676) puts the land rectangle and then the vector map
//! (`map-globe` or `map-flat`) on the dark basemap, once the TileJSON has given the tile URL; the
//! satellite and realistic basemaps have no vector map. Here the builder only says whether it is
//! drawn ([`vector_map::Input::shown`]): the land is the earth layer's (the scene's `earth`), the
//! tiles and the projection are the map layer's, and the tile URL is the app's fetcher's
//! (`tile_fetcher::TileSources::app` reads OpenFreeMap's TileJSON once; until it has, or if it
//! cannot, no tile arrives and only the land shows, as today).
//!
//! The plain buildings (item B7; G:2694) follow the *buildings* setting
//! ([`buildings::Input::shown`]; the layer works out the flat map and the basemap), and with them
//! the scene's key light is aimed at the sun (`scene.aim_lights`, G:2723-2724: from the real sun
//! over buildings on the flat map, else from its usual direction), once the clocks are set.

use rustradar_core::geo::Basemap;
use rustradar_map::SceneInput;
use rustradar_map::layers::{buildings, vector_map};

use crate::state::AppState;

/// The base map: the vector map's tile source on the dark basemap.
#[derive(Debug, Default)]
pub struct Base {}

impl Base {
    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]).
    pub fn build(&mut self, state: &AppState, scene: &mut SceneInput) {
        scene.vector_map = vector_map::Input { shown: state.basemap() == Basemap::Dark };
        let buildings = state.persisted.settings.buildings;
        scene.buildings = buildings::Input { shown: buildings };
        scene.aim_lights(buildings);
    }
}

#[cfg(test)]
mod tests {
    use rustradar_map::{Gpu, GpuError};
    use rustradar_map::headless::Image;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::Options;
    use crate::headless;
    use crate::scene_builder::SceneBuilder;
    use crate::state::{Clocks, Startup};

    const V3: ViewState = ViewState { latitude: 59.33, longitude: 18.07, zoom: 9.0, pitch: 0.0, bearing: 0.0, position: None };

    #[test]
    fn the_vector_map_is_drawn_on_the_dark_basemap_only() {
        for (basemap, shown) in [(Basemap::Dark, true), (Basemap::Satellite, false), (Basemap::Realistic, false)] {
            let mut state = AppState::new(Startup { basemap: Some(basemap), ..Default::default() }, None, None, None, V3, [1100.0, 720.0]);
            let clocks = Clocks { local_ms: 1.7e12, feed_ms: 1.7e12 };
            let scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
            assert_eq!(scene.vector_map.shown, shown, "{basemap:?}");
        }
    }

    /// The plain buildings follow the setting (B7), and the key light is aimed at the sun over
    /// them on the flat map (G:2723-2724), not with the setting off.
    #[test]
    fn the_buildings_follow_the_setting_and_aim_the_key_light() {
        let v4 = ViewState { latitude: 59.6519, longitude: 17.9186, zoom: 13.5, pitch: 45.0, bearing: 30.0, position: None };
        let clocks = Clocks { local_ms: 1_782_036_000_000.0, feed_ms: 1_782_036_000_000.0 };
        for on in [true, false] {
            let mut state = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, v4, [1440.0, 900.0]);
            state.persisted.settings.buildings = on;
            let scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
            assert_eq!(scene.buildings.shown, on);
            assert_eq!(scene.lights, rustradar_map::renderer::scene_lights(&v4, clocks.feed_ms, on));
            assert_eq!(scene.lights == rustradar_map::pipelines::mesh::LightRig::new(), !on, "aimed only over buildings");
        }
    }

    /// The share of `image`'s pixels in the rectangle `[x0, x1) × [y0, y1)` that are `rgb`.
    fn share(image: &Image, [x0, y0, x1, y1]: [u32; 4], rgb: [u8; 3]) -> f64 {
        let mut hits = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                hits += usize::from(image.pixel(x, y)[..3] == rgb);
            }
        }
        hits as f64 / f64::from((x1 - x0) * (y1 - y0))
    }

    /// `--screenshot --view 59.33,18.07,9 --basemap dark` from the committed fixtures, through the
    /// whole app (state, scene builder, map, UI pass): the Stockholm region's sea and lakes in the
    /// vector map's water `(30,30,30)` over the land `(50,50,50)`, with its roads.
    #[test]
    fn a_dark_shot_draws_the_vector_map_from_the_fixtures() {
        let gpu = match Gpu::headless() {
            Ok(gpu) => gpu,
            Err(GpuError::NoAdapter(e)) => {
                eprintln!("skipped: {e}");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        let options = Options {
            view: Some(V3),
            basemap: Some(Basemap::Dark),
            size: Some([1100, 720]),
            scale: Some(1.0),
            now_ms: Some(1_782_036_000_000.0),
            ..Default::default()
        };
        let shot = headless::render_on(&gpu, &options).expect("the shot");
        assert!(shot.missing_tiles.is_empty(), "the fixtures lack {:?}", shot.missing_tiles);
        // the map away from the UI's corners
        let map = [300, 100, 1000, 650];
        let (sea, land) = (share(&shot.image, map, [30, 30, 30]), share(&shot.image, map, [50, 50, 50]));
        let roads = share(&shot.image, map, [88, 88, 88]) + share(&shot.image, map, [66, 66, 66]);
        eprintln!("sea {sea:.3}, land {land:.3}, roads {roads:.4}");
        assert!(sea > 0.2 && land > 0.3, "sea {sea}, land {land}");
        assert!(roads > 0.001, "roads {roads}");
    }
}
