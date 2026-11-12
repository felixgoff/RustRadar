//! The realistic map's ground: base land, land cover, city ground (flat) or ground (globe), ground
//! rails and platforms, the Earth picture (doc 04 §5–6, §11; doc 02 §8.6).
//!
//! Stage 2 stub (E4): draws nothing until R1–R3, R8–R10 fill it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

pub mod land;

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// Rows 3c–3f (realistic basemap).
pub struct Ground {}

impl Ground {
    pub fn new(_init: &Init<'_>) -> Self {
        Ground {}
    }
}

impl Layer for Ground {
    fn name(&self) -> &'static str {
        "ground"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
