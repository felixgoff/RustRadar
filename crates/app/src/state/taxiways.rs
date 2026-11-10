//! Taxiway networks for the motion model: which airports are loaded, an LRU of 6, a retry after 5 min.
//!
//! Stub of the app skeleton (E17): it holds nothing and its step does nothing. Item A11 fills it
//! (doc 03 §16).

use super::Cx;

/// Taxiway networks for the motion model: which airports are loaded, an LRU of 6, a retry after 5 min.
#[derive(Debug, Default)]
pub struct Taxiways {}

impl Taxiways {
    /// This frame's step (see [`super::AppState::poll`] for the order).
    pub fn poll(&mut self, _cx: &mut Cx<'_>) {}
}
