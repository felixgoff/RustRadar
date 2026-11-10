//! The weather radar on the map: tiles, lifted radar, measured volumes.
//!
//! Stub of the app skeleton (E17): it adds nothing to the scene. Item W5 fills it (doc 02 §3.6, §14.2).

use rustradar_map::SceneInput;

use crate::state::AppState;

/// The weather radar on the map: tiles, lifted radar, measured volumes.
#[derive(Debug, Default)]
pub struct Radar {}

impl Radar {
    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]).
    pub fn build(&mut self, _state: &AppState, _scene: &mut SceneInput) {}
}
