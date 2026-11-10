//! Signal blocks: fetching them and the taken blocks.
//!
//! Stub of the app skeleton (E17): it holds nothing and its step does nothing. Item T5 fills it
//! (doc 02 §6).

use super::Cx;

/// Signal blocks: fetching them and the taken blocks.
#[derive(Debug, Default)]
pub struct Blocks {}

impl Blocks {
    /// This frame's step (see [`super::AppState::poll`] for the order).
    pub fn poll(&mut self, _cx: &mut Cx<'_>) {}
}
