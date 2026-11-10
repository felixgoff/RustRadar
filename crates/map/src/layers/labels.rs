//! Place names and airport codes: label tiles, the text/size/colour/priority table, the horizon filter,
//! collision group `"labels"` (doc 01 §5.9).
//!
//! Stage 2 stub (E4): draws nothing until B4 fills it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// Row 18: `label-tiles`, `labels`.
pub struct Labels {}

impl Labels {
    pub fn new(_init: &Init<'_>) -> Self {
        Labels {}
    }
}

impl Layer for Labels {
    fn name(&self) -> &'static str {
        "labels"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
