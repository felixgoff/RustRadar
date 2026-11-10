//! The weather radar: RainViewer frames and the measured volumes for the view.
//!
//! Stub of the app skeleton (E17): it holds nothing and its step does nothing. Item W5 fills it
//! (doc 02 §3.6, §14.2).

use super::Cx;

/// The weather radar: RainViewer frames and the measured volumes for the view.
#[derive(Debug, Default)]
pub struct Radar {}

impl Radar {
    /// This frame's step (see [`super::AppState::poll`] for the order).
    pub fn poll(&mut self, _cx: &mut Cx<'_>) {}
}
