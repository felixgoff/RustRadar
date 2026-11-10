//! Ships: the selection halo, 3D models from zoom 12.5, icons, the selection ring (doc 02 §4).
//!
//! Stage 2 stub (E4): draws nothing until H2–H4 fill it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// Rows 9–11: ship halo, ships, ship ring.
pub struct Ships {}

impl Ships {
    pub fn new(_init: &Init<'_>) -> Self {
        Ships {}
    }
}

impl Layer for Ships {
    fn name(&self) -> &'static str {
        "ships"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
