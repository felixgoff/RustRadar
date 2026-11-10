//! The aircraft: icons, meshes, shadows and labels for every aircraft shown, and their pick records.
//!
//! Stub of the app skeleton (E17): it adds nothing to the scene. Item A3 fills it (doc 03 §1–2, §5.1–5.3, §6).

use rustradar_map::SceneInput;

use crate::state::AppState;

/// The aircraft: icons, meshes, shadows and labels for every aircraft shown, and their pick records.
#[derive(Debug, Default)]
pub struct Aircraft {}

impl Aircraft {
    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]).
    pub fn build(&mut self, _state: &AppState, _scene: &mut SceneInput) {}
}
