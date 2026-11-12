//! The weather radar on the map (item W5; doc 02 §3.5, §3.6; G:2680-2690): what the radar layer
//! (`map::layers::radar`) gets from the radar state ([`crate::state::radar`]) at each scene refresh.
//!
//! - **The weather on** (`settings.weather`): RainViewer's frame (`radarLayer(radarUrl)` when there
//!   is one: `tiles.url`, `None` draws no RainViewer) and the measured volumes in the backend's
//!   order (`for (const volume of measured) layers.push(...measuredLayers(volume, ex, lift))`),
//!   with the exaggeration setting the radar's height scale comes from (`ex`, worked out by the map
//!   for its frame's zoom).
//! - **The weather off**: no frame and no volumes, so the map lets go of RainViewer's tiles, the
//!   lifted layers' arenas (`freeArenas()`, which today's page calls with the weather off or no
//!   frame: both are a scene without a frame here) and the measured meshes.
//! - `tiles.coverages` stays empty: RainViewer stands back under the volumes the measured layer
//!   draws, whose coverages the radar layer hands to the tiles once a set is in
//!   (`setRadarCoverage` after `refreshMeasured`'s load, item W4).

use std::sync::Arc;

use rustradar_map::SceneInput;
use rustradar_map::layers::radar::{self, measured, tiles};
use rustradar_scene::radar::Coverage;

use crate::state::AppState;

/// The weather radar on the map: tiles, lifted radar, measured volumes.
#[derive(Debug, Default)]
pub struct Radar {
    /// The empty coverage list handed to the tiles (one allocation, compared by its keys).
    no_coverage: Arc<Vec<Coverage>>,
}

impl Radar {
    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]).
    pub fn build(&mut self, state: &AppState, scene: &mut SceneInput) {
        let settings = &state.persisted.settings;
        let on = settings.weather;
        let radar = &state.radar;
        scene.radar = radar::Input {
            tiles: tiles::Input { url: radar.url().filter(|_| on).map(str::to_owned), coverages: Arc::clone(&self.no_coverage) },
            measured: measured::Input {
                volumes: if on { radar.measured().to_vec() } else { Vec::new() },
                exaggeration: settings.exaggeration,
            },
        };
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::state::Startup;
    use crate::state::radar::Radar as RadarState;

    const T0: f64 = 1_782_036_000_000.0;
    const FRAME_URL: &str = "https://tilecache.rainviewer.com/v2/radar/47622fe604ca/256/{z}/{x}/{y}/2/1_1.png";

    /// The app's state on the committed fixtures, polled once at `view` with the weather `on`.
    fn state(view: ViewState, on: bool) -> AppState {
        let fixture = rustradar_map::io::fixture_tiles_dir().parent().expect("the data folder").to_owned();
        let mut s = AppState::new(Startup { fixture: Some(fixture), ..Default::default() }, None, None, None, view, [1440.0, 900.0]);
        s.persisted.settings.weather = true;
        s.poll(T0);
        s.persisted.settings.weather = on;
        s
    }

    fn built(s: &mut AppState) -> SceneInput {
        let mut scene = SceneInput::new(s.controller.view(), s.controller.size(), 1.0, Basemap::Dark);
        Radar::default().build(s, &mut scene);
        scene
    }

    fn ohio() -> ViewState {
        ViewState { longitude: -84.0, latitude: 39.0, zoom: 5.3, bearing: 0.0, pitch: 55.0, position: None }
    }

    #[test]
    fn the_weather_on_hands_over_the_frame_and_the_volumes() {
        let mut s = state(ohio(), true);
        s.persisted.settings.exaggeration = 4.0;
        let scene = built(&mut s);
        assert_eq!(scene.radar.tiles.url.as_deref(), Some(FRAME_URL));
        assert!(scene.radar.tiles.coverages.is_empty(), "the map blanks RainViewer under what it draws");
        let keys: Vec<_> = scene.radar.measured.volumes.iter().map(|v| v.key()).collect();
        assert_eq!(keys, vec![("mrms".to_owned(), 1_794_446_561)]);
        assert!(Arc::ptr_eq(&scene.radar.measured.volumes[0], &s.radar.measured()[0]), "shared, not copied");
        assert_eq!(scene.radar.measured.exaggeration, 4.0);
    }

    #[test]
    fn the_weather_off_hands_over_nothing_so_the_map_frees_it_all() {
        let mut s = state(ohio(), false);
        // the state keeps what it has, as today's `radarUrl` and `measured`
        assert_eq!(s.radar.url(), Some(FRAME_URL));
        assert_eq!(s.radar.measured().len(), 1);
        let scene = built(&mut s);
        assert_eq!(scene.radar.tiles.url, None, "no frame: the tiles and the arenas go");
        assert!(scene.radar.measured.volumes.is_empty());
        // on again: drawn at once from what is held
        s.persisted.settings.weather = true;
        let scene = built(&mut s);
        assert_eq!(scene.radar.tiles.url.as_deref(), Some(FRAME_URL));
        assert_eq!(scene.radar.measured.volumes.len(), 1);
    }

    #[test]
    fn no_frame_draws_no_rainviewer() {
        let mut s = AppState::new(Startup::default(), None, None, None, ohio(), [1440.0, 900.0]);
        s.radar = RadarState::with_feeds(None);
        s.persisted.settings.weather = true;
        s.poll(T0);
        let scene = built(&mut s);
        assert_eq!(scene.radar.tiles.url, None);
        assert!(scene.radar.measured.volumes.is_empty());
    }
}
