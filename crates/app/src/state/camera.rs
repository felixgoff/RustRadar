//! The camera's own work each frame, after the transition step: the lock on an aircraft (its
//! limits, re-aiming, the ease back when it ends) and the ground height under the camera.
//!
//! Stub of the app skeleton (E17): it holds nothing and its step does nothing. Items A10 (doc 01
//! §2.4, §3.9; doc 03 §11.4) and Z4 (doc 04 §14.4) fill it.

use super::Cx;

/// The camera lock and the ground height.
#[derive(Debug, Default)]
pub struct Camera {}

impl Camera {
    /// Each frame after the controller's transition step ([`super::AppState::advance_camera`]).
    pub fn poll(&mut self, _cx: &mut Cx<'_>) {}
}
