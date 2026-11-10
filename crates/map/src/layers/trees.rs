//! Trees on the realistic map: planted tiles, the tree and tree-shadow shaders, canopy heights, street
//! trees (doc 04 §8; doc 03 §3 row 8).
//!
//! Stage 2 stub (E4): draws nothing until R7 fills it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// Row 8: trees (realistic, flat, zoom ≥ 14).
pub struct Trees {}

impl Trees {
    pub fn new(_init: &Init<'_>) -> Self {
        Trees {}
    }
}

impl Layer for Trees {
    fn name(&self) -> &'static str {
        "trees"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
