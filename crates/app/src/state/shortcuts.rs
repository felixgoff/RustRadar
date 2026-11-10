//! Keyboard shortcuts: the keys of doc 06 §3.13 turned into actions.
//!
//! Stub of the app skeleton (E17): it holds nothing and its step does nothing. Item U6 fills it
//! (doc 06 §3.13).

use super::Cx;

/// Keyboard shortcuts: the keys of doc 06 §3.13 turned into actions.
#[derive(Debug, Default)]
pub struct Shortcuts {}

impl Shortcuts {
    /// This frame's step (see [`super::AppState::poll`] for the order).
    pub fn poll(&mut self, _cx: &mut Cx<'_>) {}
}
