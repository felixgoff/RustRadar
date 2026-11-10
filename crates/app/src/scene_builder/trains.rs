//! Trains on the map: icons, cars, the selected train's route and ring.
//!
//! Stub of the app skeleton (E17): it adds nothing to the scene. Item T2 fills it, T6 the route part (doc 02 §5.1–5.7).

use rustradar_map::SceneInput;

use crate::state::AppState;

/// Trains on the map: icons, cars, the selected train's route and ring.
#[derive(Debug, Default)]
pub struct Trains {}

impl Trains {
    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]).
    pub fn build(&mut self, _state: &AppState, _scene: &mut SceneInput) {}
}
