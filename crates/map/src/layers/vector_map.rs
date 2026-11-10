//! The dark basemap's vector map: OpenFreeMap tiles of `water`, `boundary`, `transportation`, `aeroway`
//! as fills and paths, one `TileSet` for both projections, `SURFACE_NO_CULL`; and the true-width
//! runways (doc 01 §5.6–5.7).
//!
//! Stage 2 stub (E4): draws nothing until B2, B3 fill it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// Rows 3k and 3l: `map-*`, `aeroway-true-*` (dark basemap).
pub struct VectorMap {}

impl VectorMap {
    pub fn new(_init: &Init<'_>) -> Self {
        VectorMap {}
    }
}

impl Layer for VectorMap {
    fn name(&self) -> &'static str {
        "vector-map"
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) {}

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}
