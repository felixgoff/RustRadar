//! Weather radar: RainViewer tiles, the ground layer, lifted layers and walls, measured volumes, GPU
//! arenas (doc 02 §3; doc 01 §5.11).
//!
//! RainViewer's tiles load and contour on the worker pool ([`tiles`], W1) once the scene carries a
//! frame and are drawn flat on the ground ([`ground`], W2); the lifted layers ([`lifted`], W3) draw
//! them from the GPU arenas ([`arena`], W2) while the view is tilted; the measured volumes
//! ([`measured`], W4) come after. Keep `new(&Init) -> Self` and the row in `layers::ordered`.

pub mod arena;
pub mod ground;
pub mod lifted;
pub mod measured;
pub mod tiles;

use super::{DrawCx, Init, Layer, PrepareCx};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// RainViewer's frame and the measured coverages blanking it (W1).
    pub tiles: tiles::Input,
    /// The measured volumes (W4). Once a set is meshed, its coverages blank RainViewer in place of
    /// `tiles.coverages` (which serve while no volume is drawn).
    pub measured: measured::Input,
}

/// Rows 5a and 5b (`weather`).
pub struct Radar {
    /// RainViewer's tiles (W1), each with echo uploaded for the ground layer; the ground layer (W2)
    /// and the lifted layers (W3) draw them.
    tiles: ground::Tiles,
    /// RainViewer's ground layer (W2).
    ground: ground::Ground,
    /// RainViewer's lifted layers and walls (W3), drawn after the ground.
    lifted: lifted::Lifted,
    /// The measured volumes (W4), drawn after RainViewer.
    measured: measured::Measured,
}

impl Radar {
    pub fn new(init: &Init<'_>) -> Self {
        Radar {
            tiles: tiles::RadarTiles::new(init),
            ground: ground::Ground::new(init),
            lifted: lifted::Lifted::new(init),
            measured: measured::Measured::new(init),
        }
    }
}

impl Layer for Radar {
    fn name(&self) -> &'static str {
        "radar"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        let scene = cx.scene;
        self.measured.prepare(cx, &scene.radar.measured);
        // RainViewer stands back where a measured volume is drawn (`setRadarCoverage`)
        let measured = self.measured.coverages();
        let blanked = (!measured.is_empty()).then(|| tiles::Input { coverages: std::sync::Arc::clone(measured), ..scene.radar.tiles.clone() });
        self.tiles.update(cx, blanked.as_ref().unwrap_or(&scene.radar.tiles), |gpu, _, radar| ground::GroundTile::upload(&gpu.device, radar));
        self.ground.prepare(cx, &self.tiles);
        // the stack from the tiles in view, with today's one height setting (`ex`)
        self.lifted.prepare(cx, &self.tiles, scene.radar.measured.exaggeration);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        self.ground.draw(pass, cx, &self.tiles);
        // `radarLayer(radarUrl), ...radarLifted(ex, lift)`: lowest height first, slices then walls
        self.lifted.draw(pass, cx);
        // after RainViewer's layers, in the backend's order
        self.measured.draw(pass, cx);
    }

    fn pending(&self) -> bool {
        self.tiles.pending() || self.measured.pending()
    }
}
