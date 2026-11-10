//! Shaded relief from terrarium tiles, z0–12, with the opacity table (dark 1, satellite 0.45, realistic
//! 0.6), zoom fade and haze fade (doc 04 §12, §14.2).
//!
//! Stage 2 stub (E4): draws nothing until R12 fills it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// Rows 3b, 3g, 3j: shaded relief.
pub struct Hillshade {}

impl Hillshade {
    pub fn new(_init: &Init<'_>) -> Self {
        Hillshade {}
    }
}

impl Layer for Hillshade {
    fn name(&self) -> &'static str {
        "hillshade"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
