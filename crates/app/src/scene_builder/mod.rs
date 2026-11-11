//! The scene refresh (plan 1.4.9, 2.2 step 4): the state turned into a [`SceneInput`], one builder
//! per kind, and how often that happens ([`Pacer`], doc 01 §2.6).
//!
//! [`SceneBuilder::build`] makes the scene common to all layers (view, canvas, map scale, basemap,
//! clocks, the earth) and hands it to each builder in turn. Builders read the state and fill their
//! layers' inputs; each keeps what it needs between refreshes in its own struct. They start as
//! stubs that add nothing:
//!
//! | Builder | Fills | Item |
//! |---|---|---|
//! | [`base`] | the vector map (dark basemap), the night, the place labels, the plain buildings | B2, B4, B5, B7 |
//! | [`aircraft`] | aircraft icons, meshes, shadows, labels and their pick records | A3 |
//! | [`flight`] | the selected flight's trail, route, markers and ring | A8 |
//! | [`trains`] | train icons, cars, the selected train's route | T2, T6 |
//! | [`ships`] | ship icons, models, the selection | H2, H4 |
//! | [`radar`] | the weather radar | W5 |
//! | [`realistic`] | the realistic map's ground, buildings, trees, sky | stage 8 |

pub mod aircraft;
pub mod base;
pub mod flight;
pub mod radar;
pub mod realistic;
pub mod ships;
pub mod trains;

use rustradar_map::layers::{earth, rail};
use rustradar_map::renderer::map_scale;
use rustradar_map::SceneInput;

use crate::state::{AppState, Clocks};

/// How long the scene may go unrefreshed (doc 01 §2.6, G:3232-3243): every frame when the camera is
/// locked on a vehicle (and no move of the app's runs) close in, else by zoom; motion is invisible
/// zoomed out.
pub fn refresh_interval_ms(zoom: f64, locked: bool) -> f64 {
    if locked && zoom > 7.0 {
        0.0
    } else if zoom > 7.0 {
        50.0
    } else if zoom > 4.0 {
        150.0
    } else {
        500.0
    }
}

/// When the scene is rebuilt: on the interval of [`refresh_interval_ms`], and at once when forced
/// (a change of settings, selection, hover or data, a finished tile). A forced refresh leaves the
/// interval's tick where it was, as today's `render()` outside the loop does.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pacer {
    /// `lastTick`: when the interval last refreshed (local ms).
    last_tick: f64,
    forced: bool,
}

impl Pacer {
    /// Rebuild at the next frame whatever the interval.
    pub fn force(&mut self) {
        self.forced = true;
    }

    /// Whether to rebuild the scene in the frame at `now_ms` (`loop(t)`), and takes the refresh.
    pub fn due(&mut self, now_ms: f64, zoom: f64, locked: bool) -> bool {
        let forced = std::mem::take(&mut self.forced);
        if now_ms - self.last_tick >= refresh_interval_ms(zoom, locked) {
            self.last_tick = now_ms;
            return true;
        }
        forced
    }

    /// When the interval's next refresh is due (local ms): the event loop sleeps until then when
    /// nothing else needs a frame.
    pub fn next_due_ms(&self, zoom: f64, locked: bool) -> f64 {
        if self.forced { f64::NEG_INFINITY } else { self.last_tick + refresh_interval_ms(zoom, locked) }
    }
}

/// The builders, each with what it keeps between refreshes.
#[derive(Debug, Default)]
pub struct SceneBuilder {
    pub base: base::Base,
    pub aircraft: aircraft::Aircraft,
    pub flight: flight::Flight,
    pub trains: trains::Trains,
    pub ships: ships::Ships,
    pub radar: radar::Radar,
    pub realistic: realistic::Realistic,
}

impl SceneBuilder {
    /// The scene of `state` now: the camera's view on its canvas, the basemap, the map scale for a
    /// display of `scale_factor`, the frame's clocks, the earth; then every builder's layers.
    /// `state` is mutable for the aircraft: posing a flight feeds the motion model.
    pub fn build(&mut self, state: &mut AppState, clocks: Clocks, scale_factor: f64) -> SceneInput {
        let basemap = state.basemap();
        let mut scene = SceneInput::new(state.controller.view(), state.controller.size(), map_scale(basemap, scale_factor), basemap);
        scene.local_now_ms = clocks.local_ms;
        scene.feed_now_ms = clocks.feed_ms;
        // the sphere on the globe and the land rectangle on the dark map (doc 01 §5.4–5.5); each
        // layer works out from the view and the basemap whether it draws
        scene.earth = earth::Input::SHOWN;
        self.realistic.build(state, &mut scene);
        self.base.build(state, &mut scene);
        // the rail map (T4; G:2679) on every basemap, coloured by speed unless it shows the blocks
        let settings = &state.persisted.settings;
        scene.rail = rail::Input { shown: settings.rail_map, speeds: rustradar_scene::rail::rail_speeds(settings.rail_map, settings.rail_shows) };
        self.radar.build(state, &mut scene);
        self.ships.build(state, &mut scene);
        self.trains.build(state, &mut scene);
        // the signal blocks (T5) see this refresh's trains as drawn (G:2733-2735)
        let on_map: Vec<&rustradar_core::model::Train> = state.trains.on_map().iter().collect();
        state.blocks.build(&state.persisted.settings, &on_map, self.trains.builder().drawn(), &mut scene);
        self.flight.build(state, &mut scene);
        self.aircraft.build(state, &mut scene);
        scene
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::state::Startup;

    #[test]
    fn the_interval_follows_zoom_and_the_lock() {
        assert_eq!(refresh_interval_ms(1.9, false), 500.0);
        assert_eq!(refresh_interval_ms(4.0, false), 500.0);
        assert_eq!(refresh_interval_ms(4.5, false), 150.0);
        assert_eq!(refresh_interval_ms(7.0, false), 150.0);
        assert_eq!(refresh_interval_ms(7.5, false), 50.0);
        assert_eq!(refresh_interval_ms(7.5, true), 0.0);
        // locked far out still waits
        assert_eq!(refresh_interval_ms(7.0, true), 150.0);
        assert_eq!(refresh_interval_ms(3.0, true), 500.0);
    }

    #[test]
    fn the_pacer_ticks_on_its_interval_and_forcing_keeps_the_tick() {
        let mut p = Pacer::default();
        assert!(p.due(1000.0, 2.0, false), "the first frame refreshes");
        assert!(!p.due(1200.0, 2.0, false));
        assert_eq!(p.next_due_ms(2.0, false), 1500.0);
        p.force();
        assert_eq!(p.next_due_ms(2.0, false), f64::NEG_INFINITY);
        assert!(p.due(1300.0, 2.0, false), "forced");
        assert!(!p.due(1301.0, 2.0, false), "once");
        // the forced one did not move the tick: still due at 1500
        assert!(!p.due(1499.0, 2.0, false));
        assert!(p.due(1500.0, 2.0, false));
        // close in and locked: every frame
        assert!(p.due(1500.0, 9.0, true));
        assert!(p.due(1500.0, 9.0, true));
        assert_eq!(p.next_due_ms(9.0, true), 1500.0);
    }

    #[test]
    fn the_scene_has_the_view_the_clocks_and_the_earth() {
        let view = ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, bearing: 10.0, pitch: 30.0, position: None };
        for (basemap, scale, map_scale) in [(Basemap::Dark, 2.0, 2.0), (Basemap::Satellite, 1.0, 1.0), (Basemap::Realistic, 2.0, 1.5)] {
            let mut state = AppState::new(Startup { basemap: Some(basemap), ..Default::default() }, None, None, None, view, [1100.0, 720.0]);
            let clocks = Clocks { local_ms: 1.7e12, feed_ms: 1.7e12 + 300.0 };
            let scene = SceneBuilder::default().build(&mut state, clocks, scale);
            assert_eq!(scene.view, view);
            assert_eq!(scene.size, [1100.0, 720.0]);
            assert_eq!(scene.basemap, basemap);
            assert_eq!(scene.map_scale, map_scale);
            assert_eq!(scene.local_now_ms, 1.7e12);
            assert_eq!(scene.feed_now_ms, 1.7e12 + 300.0);
            assert!(scene.earth.shown);
        }
    }

    /// The rail map (T4) follows the setting on every basemap, coloured by speed unless it shows
    /// the signal blocks (`railSpeeds`).
    #[test]
    fn the_rail_map_follows_its_settings() {
        use rustradar_core::settings::RailShows;
        let view = ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, bearing: 0.0, pitch: 0.0, position: None };
        let clocks = Clocks { local_ms: 1.7e12, feed_ms: 1.7e12 };
        for basemap in [Basemap::Dark, Basemap::Satellite, Basemap::Realistic] {
            let mut state = AppState::new(Startup { basemap: Some(basemap), ..Default::default() }, None, None, None, view, [1100.0, 720.0]);
            let scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
            assert!(!scene.rail.shown, "off by default");
            for (shows, speeds) in [(RailShows::Speeds, true), (RailShows::Blocks, false)] {
                state.persisted.settings.rail_map = true;
                state.persisted.settings.rail_shows = shows;
                let scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
                assert_eq!((scene.rail.shown, scene.rail.speeds), (true, speeds), "{basemap:?} {shows:?}");
            }
        }
    }
}
