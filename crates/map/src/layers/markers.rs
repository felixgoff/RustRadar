//! Markers: the route's airports, the focused airport, the searched place, and the selected aircraft's
//! ring (doc 03 §8.3–8.4; doc 02 §10).
//!
//! Stage 2 stub (E4): draws nothing until A8 fills it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// Rows 22–24: route airports, focused airport, searched place.
pub struct Markers {}

impl Markers {
    pub fn new(_init: &Init<'_>) -> Self {
        Markers {}
    }
}

impl Layer for Markers {
    fn name(&self) -> &'static str {
        "markers"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}

/// Row 30: `selection-ring`, drawn after the aircraft.
pub struct SelectionRing {}

impl SelectionRing {
    pub fn new(_init: &Init<'_>) -> Self {
        SelectionRing {}
    }
}

impl Layer for SelectionRing {
    fn name(&self) -> &'static str {
        "selection-ring"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
