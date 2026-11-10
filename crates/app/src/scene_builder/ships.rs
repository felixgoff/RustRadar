//! Ships on the map: icons, models, the selected ship's halo and ring.
//!
//! Stub of the app skeleton (E17): it adds nothing to the scene. Item H2 fills it, H4 the selection part (doc 02 §4.1–4.8).

use rustradar_map::SceneInput;

use crate::state::AppState;

/// Ships on the map: icons, models, the selected ship's halo and ring.
#[derive(Debug, Default)]
pub struct Ships {}

impl Ships {
    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]).
    pub fn build(&mut self, _state: &AppState, _scene: &mut SceneInput) {}
}
