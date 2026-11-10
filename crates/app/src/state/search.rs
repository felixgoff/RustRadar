//! The search box: remote search and geocoding with their debounce, and the picks.
//!
//! Stub of the app skeleton (E17): it holds nothing and its step does nothing. Item U8 fills it
//! (doc 06 §3.7, §5).

use super::Cx;

/// The search box: remote search and geocoding with their debounce, and the picks.
#[derive(Debug, Default)]
pub struct Search {}

impl Search {
    /// This frame's step (see [`super::AppState::poll`] for the order).
    pub fn poll(&mut self, _cx: &mut Cx<'_>) {}
}
