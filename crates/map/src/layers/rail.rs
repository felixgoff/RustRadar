//! The rail map: the network from OpenFreeMap tiles, the overview below zoom 8, stations, the speed map
//! with labels; bridges and tunnels (doc 02 §8–9).
//!
//! Stage 2 stub (E4): draws nothing until T4, T7 fill it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Input {}

/// Row 4: the rail network (`railMap`).
pub struct Rail {}

impl Rail {
    pub fn new(_init: &Init<'_>) -> Self {
        Rail {}
    }
}

impl Layer for Rail {
    fn name(&self) -> &'static str {
        "rail"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
