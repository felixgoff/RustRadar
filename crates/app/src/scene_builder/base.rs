//! The base map: the vector map's tile source on the dark basemap.
//!
//! Stub of the app skeleton (E17): it adds nothing to the scene. Item B2 fills it (doc 01 §5.1, §5.6).

use rustradar_map::SceneInput;

use crate::state::AppState;

/// The base map: the vector map's tile source on the dark basemap.
#[derive(Debug, Default)]
pub struct Base {}

impl Base {
    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]).
    pub fn build(&mut self, _state: &AppState, _scene: &mut SceneInput) {}
}
