//! Night shading: four stacked quad sets in `[2,6,14,46]`, recomputed when `floor(feed_now / 60 000)`
//! changes, not on realistic (doc 01 §5.10).
//!
//! Stage 2 stub (E4): draws nothing until B5 fills it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// Row 6: `night` (`daylight`, not realistic).
pub struct Night {}

impl Night {
    pub fn new(_init: &Init<'_>) -> Self {
        Night {}
    }
}

impl Layer for Night {
    fn name(&self) -> &'static str {
        "night"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
