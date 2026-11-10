//! Buildings: plain extrusions on dark and satellite (`BUILDING_COLOR`, sun-aimed key light, stencil
//! ground shadows), and on realistic the styled buildings, foot shade, shadows and roofs (doc 01 §9.3;
//! doc 04 §7).
//!
//! Stage 2 stub (E4): draws nothing until B7, R4–R6 fill it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// Row 7: city buildings (flat map).
pub struct Buildings {}

impl Buildings {
    pub fn new(_init: &Init<'_>) -> Self {
        Buildings {}
    }
}

impl Layer for Buildings {
    fn name(&self) -> &'static str {
        "buildings"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
