//! Trains: the selected train's route, the selection halo, icons or car meshes (with tunnel layers),
//! dev sightings, the selection ring (doc 02 §5, §7, §13.1).
//!
//! Stage 2 stub (E4): draws nothing until T2, T3, T6, T9 fill it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// Rows 13–17: train route, halo, trains, sightings, ring.
pub struct Trains {}

impl Trains {
    pub fn new(_init: &Init<'_>) -> Self {
        Trains {}
    }
}

impl Layer for Trains {
    fn name(&self) -> &'static str {
        "trains"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
