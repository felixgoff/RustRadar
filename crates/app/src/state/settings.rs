//! The settings file: `settings.json` loaded at start with the rules of doc 06 §23, saved 500 ms after the last change.
//!
//! Stub of the app skeleton (E17): it holds nothing and its step does nothing. Item U5 fills it
//! (doc 06 §3.2–3.3, §23).

use super::Cx;

/// The settings file: `settings.json` loaded at start with the rules of doc 06 §23, saved 500 ms after the last change.
#[derive(Debug, Default)]
pub struct Store {}

impl Store {
    /// This frame's step (see [`super::AppState::poll`] for the order).
    pub fn poll(&mut self, _cx: &mut Cx<'_>) {}
}
