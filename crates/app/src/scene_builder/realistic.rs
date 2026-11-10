//! The realistic map: sky, ground, buildings, trees, clouds.
//!
//! Stub of the app skeleton (E17): it adds nothing to the scene. Stage 8 (R1–R12) fills it (doc 04).

use rustradar_map::SceneInput;

use crate::state::AppState;

/// The realistic map: sky, ground, buildings, trees, clouds.
#[derive(Debug, Default)]
pub struct Realistic {}

impl Realistic {
    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]).
    pub fn build(&mut self, _state: &AppState, _scene: &mut SceneInput) {}
}
