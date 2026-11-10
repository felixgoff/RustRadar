//! Aircraft: shadows, landing gear and models (per group and part), icon outlines and icons, labels
//! (doc 03 §4–7).
//!
//! Stage 2 stub (E4): draws nothing until A2, A4–A7 fill it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// Rows 25–29: shadows, gear and models, icon outlines, icons, aircraft labels.
pub struct Aircraft {}

impl Aircraft {
    pub fn new(_init: &Init<'_>) -> Self {
        Aircraft {}
    }
}

impl Layer for Aircraft {
    fn name(&self) -> &'static str {
        "aircraft"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
