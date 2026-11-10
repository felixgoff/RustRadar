//! Satellite imagery: Esri World Imagery tiles, 256 px, zoom 0–19, tint `[150,158,180]`, `SURFACE`,
//! Mercator-correct sampling on the globe (doc 01 §5.2, §5.8).
//!
//! Stage 2 stub (E4): draws nothing until B6 fills it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Input {}

/// Row 3a: `imagery` (satellite basemap).
pub struct Imagery {}

impl Imagery {
    pub fn new(_init: &Init<'_>) -> Self {
        Imagery {}
    }
}

impl Layer for Imagery {
    fn name(&self) -> &'static str {
        "imagery"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
