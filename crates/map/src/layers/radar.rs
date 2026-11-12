//! Weather radar: RainViewer tiles, the ground layer, lifted layers and walls, measured volumes, GPU
//! arenas (doc 02 §3; doc 01 §5.11).
//!
//! Stage 2 stub (E4): draws nothing until W1–W4 fill it; keep `new(&Init) -> Self` and
//! the row in `layers::ordered`.

pub mod tiles;

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// RainViewer's frame and the measured coverages blanking it (W1).
    pub tiles: tiles::Input,
}

/// Rows 5a and 5b (`weather`).
pub struct Radar {
    /// RainViewer's tiles (W1); the ground layer (W2) and the lifted layers (W3) draw them.
    tiles: tiles::RadarTiles,
}

impl Radar {
    pub fn new(init: &Init<'_>) -> Self {
        Radar { tiles: tiles::RadarTiles::new(init) }
    }
}

impl Layer for Radar {
    fn name(&self) -> &'static str {
        "radar"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        let scene = cx.scene;
        self.tiles.update(cx, &scene.radar.tiles, |_, _, _| ());
    }

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}

    fn pending(&self) -> bool {
        self.tiles.pending()
    }
}
