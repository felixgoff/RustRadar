//! The 3D terrain surface: Mapterhorn / Mapzen tiles, RTIN meshes, `SURFACE_LOD`, and the cover
//! textures the draped layers are drawn into (doc 04 §14; plan 1.4.12).
//!
//! Stage 2 stub (E4): draws nothing until Z1, Z2 fill it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// Row 2: `terrain-surface`.
pub struct Terrain {}

impl Terrain {
    pub fn new(_init: &Init<'_>) -> Self {
        Terrain {}
    }
}

impl Layer for Terrain {
    fn name(&self) -> &'static str {
        "terrain"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
