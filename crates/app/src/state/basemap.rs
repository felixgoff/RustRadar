//! Switching the basemap (item B9; doc 01 §2.4-2.5, doc 03 §13.2): what today's globe does when
//! its `basemap` prop changes (G:3750-3759).
//!
//! [`AppState::set_basemap`] is the one way the basemap changes after start (the keys 1, 2 and 3,
//! the basemap menu, a settings file loaded late: stage 7's actions call it). It writes the
//! setting and then, as the globe's effect does:
//! 1. **Map scale** (`useDevicePixels: pixelRatio()`): the next scene is built for the new
//!    basemap at once, with `renderer::map_scale` (the display's scale, at most 1.5 on realistic):
//!    the frame loop forces a scene refresh when the basemap or the scale factor differs from the
//!    last scene's (`app::Engine::frame`), and the scene builder reads both. This happens locked or
//!    not, as today.
//! 2. Unless the camera is locked on an aircraft (`follow && selected`: the follow limits hold
//!    whatever the basemap), **the limits** of the new basemap (`limits()`: pitch up to 75 on
//!    realistic, 60 on the others) are handed to the camera, and if it is tilted further than the
//!    new maximum, **the pitch eases to it over 600 ms** (`goTo({ pitch: maxPitch }, 600,
//!    ["pitch"])`). deck runs that transition under the new limits from its first frame, so the
//!    camera is at the new maximum from the next frame on, and the 600 ms keep the app's other
//!    moves off (`transitionUntil`), as today (the same as the lock's ease back, `camera.rs`).
//!
//! Deviation: today's ratio is read only when the basemap changes (a window moved to a display of
//! another scale keeps the old one, the source's own shortcut); here the scene follows the scale
//! factor too (doc 03 §13.2's port note).

use rustradar_core::geo::Basemap;
use rustradar_scene::controller::{Controller, EASE_BACK_MS, ViewPatch};

use super::AppState;
use super::camera::Camera;

/// How long the pitch takes to ease within a new basemap's limit (G:3757).
pub const BASEMAP_PITCH_MS: f64 = EASE_BACK_MS;

impl AppState {
    /// Shows `basemap` from `local_ms` on (see the module documentation). Returns whether it
    /// changed; the same basemap again does nothing.
    pub fn set_basemap(&mut self, basemap: Basemap, local_ms: f64) -> bool {
        if self.persisted.settings.basemap == basemap {
            return false;
        }
        self.persisted.settings.basemap = basemap;
        basemap_changed(&mut self.controller, &self.camera, basemap, local_ms);
        true
    }
}

/// The camera's part of a basemap change at `local_ms` (G:3754-3758): nothing while locked on an
/// aircraft; else the new basemap's limits, and the pitch eased to the new maximum over 600 ms
/// when it is beyond it.
pub fn basemap_changed(controller: &mut Controller, camera: &Camera, basemap: Basemap, local_ms: f64) {
    if camera.following() {
        return;
    }
    let height = controller.size()[1];
    let limits = camera.limits(basemap, height);
    controller.set_limits(limits, camera.max_zoom_now(height));
    if controller.view().pitch > limits.max_pitch {
        controller.go_to(ViewPatch { pitch: Some(limits.max_pitch), ..ViewPatch::default() }, BASEMAP_PITCH_MS, local_ms);
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::model::LiveFlight;
    use rustradar_scene::camera::{FOLLOW_MAX_PITCH, LIMITS, REALISTIC_LIMITS, ViewState};

    use super::*;
    use crate::scene_builder::SceneBuilder;
    use crate::state::{Clocks, Startup};

    const SIZE: [f64; 2] = [1100.0, 720.0];
    /// 2026-06-21 10:00 UTC.
    const T0: f64 = 1_782_036_000_000.0;

    fn tilted(pitch: f64) -> ViewState {
        ViewState { longitude: 18.0686, latitude: 59.3293, zoom: 15.5, bearing: 20.0, pitch, position: None }
    }

    fn state(basemap: Basemap, view: ViewState) -> AppState {
        AppState::new(Startup { basemap: Some(basemap), ..Default::default() }, None, None, None, view, SIZE)
    }

    /// From realistic tilted to 70° onto dark (60° at most): the pitch is at 60 from the next frame,
    /// the 600 ms of the ease keep the app's other moves off, and the dark map's limits hold.
    #[test]
    fn leaving_realistic_tilted_past_60_eases_the_pitch_to_60() {
        let mut s = state(Basemap::Realistic, tilted(70.0));
        assert_eq!(s.controller.limits(), REALISTIC_LIMITS);
        assert!(s.set_basemap(Basemap::Dark, T0));
        assert_eq!(s.basemap(), Basemap::Dark);
        assert_eq!(s.controller.limits(), LIMITS);
        assert!(s.controller.in_transition());
        assert_eq!(s.controller.transition_until(), T0 + 600.0);
        // the frame after: deck holds the transition within the new limits from its first frame
        s.advance_camera(T0 + 16.0);
        let v = s.controller.view();
        assert_eq!(v.pitch, 60.0);
        assert_eq!(ViewState { pitch: 70.0, ..v }, tilted(70.0), "only the pitch moves");
        s.advance_camera(T0 + 300.0);
        assert_eq!(s.controller.view().pitch, 60.0);
        s.advance_camera(T0 + 600.0);
        assert!(!s.controller.in_transition());
        assert_eq!(s.controller.view(), ViewState { pitch: 60.0, ..tilted(70.0) });
    }

    /// Within the new limit, or onto realistic: the limits change and nothing moves.
    #[test]
    fn within_the_new_limit_only_the_limits_change() {
        for (from, to, pitch, limits) in [
            (Basemap::Realistic, Basemap::Satellite, 60.0, LIMITS),
            (Basemap::Realistic, Basemap::Dark, 45.0, LIMITS),
            (Basemap::Dark, Basemap::Realistic, 60.0, REALISTIC_LIMITS),
            (Basemap::Dark, Basemap::Satellite, 30.0, LIMITS),
        ] {
            let mut s = state(from, tilted(pitch));
            assert!(s.set_basemap(to, T0));
            assert_eq!(s.controller.limits(), limits, "{from:?} → {to:?}");
            assert!(!s.controller.in_transition(), "{from:?} → {to:?}");
            assert_eq!(s.controller.transition_until(), 0.0);
            s.advance_camera(T0 + 16.0);
            assert_eq!(s.controller.view(), tilted(pitch));
        }
        // onto realistic a gesture may now tilt to 75
        let mut s = state(Basemap::Dark, tilted(60.0));
        s.set_basemap(Basemap::Realistic, T0);
        s.controller.set_pitch(80.0, T0);
        s.advance_camera(T0 + 1000.0);
        assert_eq!(s.controller.view().pitch, 75.0);
        // the same basemap again: nothing
        assert!(!s.set_basemap(Basemap::Realistic, T0 + 2000.0));
    }

    /// Locked on an aircraft (`follow && selected`) the follow limits stay and nothing eases.
    #[test]
    fn locked_on_an_aircraft_the_camera_is_left_alone() {
        let mut s = state(Basemap::Realistic, tilted(70.0));
        let flight = LiveFlight { id: 7, lat: 59.33, lon: 18.07, alt: 3000, typecode: "A320".into(), icon: "A320".into(), ..Default::default() };
        s.camera.set_selected(Some(flight));
        s.camera.set_follow(true);
        s.advance_camera(T0);
        let held = s.controller.limits();
        assert!(held.max_pitch > 60.0 && held.max_pitch <= FOLLOW_MAX_PITCH, "{held:?}");
        let before = s.controller.view();
        assert!(s.set_basemap(Basemap::Dark, T0 + 16.0));
        assert_eq!(s.basemap(), Basemap::Dark, "the setting changes");
        assert_eq!(s.controller.limits(), held);
        assert!(!s.controller.in_transition());
        assert_eq!(s.controller.view(), before);
    }

    /// `useDevicePixels: pixelRatio()`: the scene built after the switch has the new basemap's
    /// map scale (the display's, at most 1.5 on realistic).
    #[test]
    fn the_map_scale_follows_the_basemap() {
        let mut s = state(Basemap::Dark, tilted(0.0));
        let clocks = Clocks { local_ms: T0, feed_ms: T0 };
        let mut builder = SceneBuilder::default();
        assert_eq!(builder.build(&mut s, clocks, 2.0).map_scale, 2.0);
        s.set_basemap(Basemap::Realistic, T0);
        let scene = builder.build(&mut s, clocks, 2.0);
        assert_eq!((scene.basemap, scene.map_scale), (Basemap::Realistic, 1.5));
        s.set_basemap(Basemap::Satellite, T0);
        assert_eq!(builder.build(&mut s, clocks, 2.0).map_scale, 2.0);
    }
}
