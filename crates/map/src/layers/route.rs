//! The selected flight's trail (with its ground shadow) and route: filed, waypoints, before, ahead (doc
//! 03 §8.1–8.2).
//!
//! Stage 2 stub (E4): draws nothing until A8 fills it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Input {}

/// Rows 19–21: `trail-shadow`, `trail`, `route-*`.
pub struct Route {}

impl Route {
    pub fn new(_init: &Init<'_>) -> Self {
        Route {}
    }
}

impl Layer for Route {
    fn name(&self) -> &'static str {
        "route"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
