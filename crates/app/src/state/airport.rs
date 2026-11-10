//! The airport board: arrivals and departures of the airport opened.
//!
//! Stub of the app skeleton (E17): it holds nothing and its step does nothing. Item U13 fills it
//! (doc 06 §3.6, §8.3).

use super::Cx;

/// The airport board: arrivals and departures of the airport opened.
#[derive(Debug, Default)]
pub struct Airport {}

impl Airport {
    /// This frame's step (see [`super::AppState::poll`] for the order).
    pub fn poll(&mut self, _cx: &mut Cx<'_>) {}
}
