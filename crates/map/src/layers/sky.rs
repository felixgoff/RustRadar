//! The sky behind the realistic map: the gradient of `scene::look` drawn first, over the clear colour
//! (doc 04 §3.6; doc 03 §12).
//!
//! Stage 2 stub (E4): draws nothing until R1 fills it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// The realistic sky (drawn only on `Basemap::Realistic`).
pub struct Sky {}

impl Sky {
    pub fn new(_init: &Init<'_>) -> Self {
        Sky {}
    }
}

impl Layer for Sky {
    fn name(&self) -> &'static str {
        "sky"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
