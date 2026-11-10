//! Weather radar: RainViewer tiles, the ground layer, lifted layers and walls, measured volumes, GPU
//! arenas (doc 02 §3; doc 01 §5.11).
//!
//! Stage 2 stub (E4): draws nothing until W1–W4 fill it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Input {}

/// Rows 5a and 5b (`weather`).
pub struct Radar {}

impl Radar {
    pub fn new(_init: &Init<'_>) -> Self {
        Radar {}
    }
}

impl Layer for Radar {
    fn name(&self) -> &'static str {
        "radar"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
