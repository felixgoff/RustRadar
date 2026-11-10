//! Airline livery colours, from the backend's logo colours.
//!
//! Stub of the app skeleton (E17): it holds nothing and its step does nothing. Item A2 fills it
//! (doc 03 §4, §17).

use super::Cx;

/// Airline livery colours, from the backend's logo colours.
#[derive(Debug, Default)]
pub struct Liveries {}

impl Liveries {
    /// This frame's step (see [`super::AppState::poll`] for the order).
    pub fn poll(&mut self, _cx: &mut Cx<'_>) {}
}
