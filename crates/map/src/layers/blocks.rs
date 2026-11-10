//! Signal blocks and signals: casing, blocks, tunnels, signals, signal names, block labels (doc 02 §6).
//!
//! Stage 2 stub (E4): draws nothing until T5 fills it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Input {}

/// Row 12: signal blocks.
pub struct Blocks {}

impl Blocks {
    pub fn new(_init: &Init<'_>) -> Self {
        Blocks {}
    }
}

impl Layer for Blocks {
    fn name(&self) -> &'static str {
        "blocks"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
