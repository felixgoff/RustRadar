//! The earth sphere and the land rectangle (doc 01 §5.4–5.5): a sphere of radius 6.3e6 m, 36 × 72
//! segments, coloured by basemap, `DEFAULT` state, globe only; the `land` rectangle ±180, ±85.06
//! densified to 5° (dark only).
//!
//! Stage 2 stub (E4): draws nothing until E5 fills it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// Row 1 of doc 03 §3: `earth`.
pub struct Earth {}

impl Earth {
    pub fn new(_init: &Init<'_>) -> Self {
        Earth {}
    }
}

impl Layer for Earth {
    fn name(&self) -> &'static str {
        "earth"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}

/// Row 3i: `land` (dark basemap).
pub struct Land {}

impl Land {
    pub fn new(_init: &Init<'_>) -> Self {
        Land {}
    }
}

impl Layer for Land {
    fn name(&self) -> &'static str {
        "land"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
