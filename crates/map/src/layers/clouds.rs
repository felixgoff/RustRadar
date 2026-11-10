//! The realistic map's clouds: the EUMETSAT picture at 10 km, opacity `1 − smoothstep(5.5, 6.5, zoom)`,
//! one quad on the globe, strips on the flat map (doc 04 §9).
//!
//! Stage 2 stub (E4): draws nothing until R10 fills it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// Row 3h: `clouds-*` (realistic basemap).
pub struct Clouds {}

impl Clouds {
    pub fn new(_init: &Init<'_>) -> Self {
        Clouds {}
    }
}

impl Layer for Clouds {
    fn name(&self) -> &'static str {
        "clouds"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
