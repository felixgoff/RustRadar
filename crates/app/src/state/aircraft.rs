//! The aircraft feed: refresh timing and back-off, the clock sync, reference data, top flights, the list shown.
//!
//! Stub of the app skeleton (E17): it holds nothing and its step does nothing. Item A1 fills it
//! (doc 06 §3.1, §3.4).

use super::Cx;

/// The aircraft feed: refresh timing and back-off, the clock sync, reference data, top flights, the list shown.
#[derive(Debug, Default)]
pub struct Aircraft {}

impl Aircraft {
    /// This frame's step (see [`super::AppState::poll`] for the order).
    pub fn poll(&mut self, _cx: &mut Cx<'_>) {}
}
