//! Hover and click on the map: what is under the pointer, the hover highlight and card, the click rules.
//!
//! Stub of the app skeleton (E17): it holds nothing and its step does nothing. Item A9 fills it
//! (doc 03 §10, doc 01 §7).

use super::Cx;

/// Hover and click on the map: what is under the pointer, the hover highlight and card, the click rules.
#[derive(Debug, Default)]
pub struct Pick {}

impl Pick {
    /// This frame's step (see [`super::AppState::poll`] for the order).
    pub fn poll(&mut self, _cx: &mut Cx<'_>) {}
}
