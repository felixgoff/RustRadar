//! Ships: ships in view, the selected ship and its card.
//!
//! Stub of the app skeleton (E17): it holds nothing and its step does nothing. Item H1 fills it
//! (doc 06 §3.8).

use super::Cx;

/// Ships: ships in view, the selected ship and its card.
#[derive(Debug, Default)]
pub struct Ships {}

impl Ships {
    /// This frame's step (see [`super::AppState::poll`] for the order).
    pub fn poll(&mut self, _cx: &mut Cx<'_>) {}
}
