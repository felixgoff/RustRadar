//! Trains: trains in view, the watched set, the selected train's stream and route, the key check, demo trains.
//!
//! Stub of the app skeleton (E17): it holds nothing and its step does nothing. Item T1 fills it
//! (doc 06 §3.3, §3.9).

use super::Cx;

/// Trains: trains in view, the watched set, the selected train's stream and route, the key check, demo trains.
#[derive(Debug, Default)]
pub struct Trains {}

impl Trains {
    /// This frame's step (see [`super::AppState::poll`] for the order).
    pub fn poll(&mut self, _cx: &mut Cx<'_>) {}
}
