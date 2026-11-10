//! The selected flight on the map: trail and its shadow, route, airport and place markers, selection ring.
//!
//! Stub of the app skeleton (E17): it adds nothing to the scene. Item A8 fills it (doc 03 §8, §15; doc 02 §10).

use rustradar_map::SceneInput;

use crate::state::AppState;

/// The selected flight on the map: trail and its shadow, route, airport and place markers, selection ring.
#[derive(Debug, Default)]
pub struct Flight {}

impl Flight {
    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]).
    pub fn build(&mut self, _state: &AppState, _scene: &mut SceneInput) {}
}
