//! The map's layers (plan 1.4.7): persistent objects in a fixed order, not descriptors diffed by id.
//!
//! [`ordered`] makes one struct per row group of the layer table of doc 03 §3, in draw order;
//! `MapRenderer` prepares and draws them in that order every frame, and each one works out from the
//! [`SceneInput`] whether it has anything to draw. Each layer file also defines the `Input` its
//! scene builder fills (a field of [`SceneInput`] of the same name).
//!
//! Stage 2 creates every file as a stub that draws nothing; the item named in its header fills it
//! and keeps `new(&Init) -> Self`. A layer added later (a new file) adds its `pub mod` line here,
//! its row in [`ordered`] and its `Input` field in `SceneInput`.
//!
//! The order, with the rows of doc 03 §3:
//!
//! | Layer | Rows | Filled by |
//! |---|---|---|
//! | [`sky::Sky`] | the realistic sky behind everything (doc 04 §3.6) | R1 |
//! | [`earth::Earth`] | 1 `earth` | E5 |
//! | [`terrain::Terrain`] | 2 `terrain-surface` | Z1, Z2 |
//! | [`imagery::Imagery`] | 3a `imagery` | B6 |
//! | [`ground::Ground`] | 3c–3f realistic land, land cover, city ground or ground, rail ground | R1–R3, R8–R10 |
//! | [`earth::Land`] | 3i `land` | E5 |
//! | [`hillshade::Hillshade`] | 3b, 3g, 3j shaded relief | R12 |
//! | [`clouds::Clouds`] | 3h `clouds-*` | R10 |
//! | [`vector_map::VectorMap`] | 3k `map-*`, 3l `aeroway-true-*` | B2, B3 |
//! | [`rail::Rail`] | 4 rail network | T4, T7 |
//! | [`radar::Radar`] | 5a, 5b radar tiles, lifted radar, measured volumes | W1–W4 |
//! | [`night::Night`] | 6 `night` | B5 |
//! | [`buildings::Buildings`] | 7 city buildings | B7, R4–R6 |
//! | [`trees::Trees`] | 8 trees | R7 |
//! | [`ships::Ships`] | 9–11 ship halo, ships, ship ring | H2–H4 |
//! | [`blocks::Blocks`] | 12 signal blocks | T5 |
//! | [`trains::Trains`] | 13–17 train route, halo, trains, sightings, ring | T2, T3, T6, T9 |
//! | [`labels::Labels`] | 18 `label-tiles`, `labels` | B4 |
//! | [`route::Route`] | 19–21 trail shadow, trail, route | A8 |
//! | [`markers::Markers`] | 22–24 route airports, focused airport, searched place | A8 |
//! | [`aircraft::Aircraft`] | 25–29 shadows, gear and models, icon outlines, icons, labels | A2, A4–A7 |
//! | [`markers::SelectionRing`] | 30 `selection-ring` | A8 |
//!
//! The terrain-draping hooks ([`Layer::drape`]) are for stage 9.

pub mod aircraft;
pub mod blocks;
pub mod buildings;
pub mod clouds;
pub mod earth;
pub mod ground;
pub mod hillshade;
pub mod imagery;
pub mod labels;
pub mod markers;
pub mod night;
pub mod radar;
pub mod rail;
pub mod route;
pub mod ships;
pub mod sky;
pub mod terrain;
pub mod trains;
pub mod trees;
pub mod vector_map;

use crate::frame::{Bindings, FrameArena, FrameSlot, GLOBALS_GROUP, ViewFrame};
use crate::gpu::Gpu;
use crate::renderer::SceneInput;

/// How a layer stands on 3D terrain (plan 1.4.12, stage 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drape {
    /// Drawn as it is (sky, earth, terrain, clouds).
    None,
    /// Drawn top-down into the terrain tiles' cover textures (ground, hillshade, rail map, radar
    /// ground, shadows, foot shade, night).
    Cover,
    /// Raised on the CPU by the terrain height where its instances are built (buildings, trees,
    /// vehicles, labels, markers).
    Offset,
}

/// Which pass a layer is drawing into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// The map pass: [`crate::target::MAP_FORMAT`] with MSAA and depth/stencil.
    Map,
    /// A terrain cover pass (stage 9): orthographic, no MSAA.
    Cover,
}

/// What a layer is made with.
pub struct Init<'a> {
    pub gpu: &'a Gpu,
    /// Groups 0 and 1, and the pipeline layout helper.
    pub bindings: &'a Bindings,
    /// The map pass's colour format.
    pub color_format: wgpu::TextureFormat,
    /// The map pass's sample count (4, or 1 where the adapter cannot).
    pub samples: u32,
}

impl Init<'_> {
    /// The map pass's multisample state.
    pub fn multisample(&self) -> wgpu::MultisampleState {
        wgpu::MultisampleState { count: self.samples, ..Default::default() }
    }
}

/// What `prepare` gets, on the main thread before encoding.
pub struct PrepareCx<'a> {
    pub gpu: &'a Gpu,
    /// This scene refresh's input.
    pub scene: &'a SceneInput,
    /// This frame's camera and globals.
    pub view: &'a ViewFrame,
    /// Where this frame's local Mercator frames go ([`ViewFrame::local`], [`ViewFrame::tile`]).
    pub frames: &'a mut FrameArena,
    /// The local clock, Unix ms.
    pub local_now_ms: f64,
}

/// What `draw` gets.
pub struct DrawCx<'a> {
    pub scene: &'a SceneInput,
    pub view: &'a ViewFrame,
    pub frames: &'a FrameArena,
    /// Group 0. At the start of the map pass the renderer binds it, and group 1 to the zero frame
    /// ([`FrameArena::DEFAULT`]); both stay bound across pipelines made with
    /// [`crate::pipelines::build`].
    pub globals: &'a wgpu::BindGroup,
    pub pass: Pass,
}

impl DrawCx<'_> {
    /// Binds the globals as group 0 (after a pipeline with a different group 0).
    pub fn bind_globals(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(GLOBALS_GROUP, self.globals, &[]);
    }

    /// Binds a frame pushed in `prepare` as group 1.
    pub fn bind_frame(&self, pass: &mut wgpu::RenderPass<'_>, slot: FrameSlot) {
        self.frames.bind(pass, slot);
    }
}

/// One row group of doc 03 §3.
pub trait Layer {
    /// A short name for logs and tests.
    fn name(&self) -> &'static str;
    /// Main thread, before encoding: react to the view and the data, request tiles, upload
    /// buffers, push frames.
    fn prepare(&mut self, cx: &mut PrepareCx<'_>);
    /// Record draw calls for one pass.
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>);
    /// How it stands on 3D terrain (stage 9).
    fn drape(&self) -> Drape {
        Drape::None
    }
    /// Whether it still waits for work (tiles, jobs, textures) that will change the picture: the
    /// headless path renders until no layer does.
    fn pending(&self) -> bool {
        false
    }
}

/// Every layer, in draw order (the table above).
pub fn ordered(init: &Init<'_>) -> Vec<Box<dyn Layer>> {
    vec![
        Box::new(sky::Sky::new(init)),
        Box::new(earth::Earth::new(init)),
        Box::new(terrain::Terrain::new(init)),
        Box::new(imagery::Imagery::new(init)),
        Box::new(ground::Ground::new(init)),
        Box::new(earth::Land::new(init)),
        Box::new(hillshade::Hillshade::new(init)),
        Box::new(clouds::Clouds::new(init)),
        Box::new(vector_map::VectorMap::new(init)),
        Box::new(rail::Rail::new(init)),
        Box::new(radar::Radar::new(init)),
        Box::new(night::Night::new(init)),
        Box::new(buildings::Buildings::new(init)),
        Box::new(trees::Trees::new(init)),
        Box::new(ships::Ships::new(init)),
        Box::new(blocks::Blocks::new(init)),
        Box::new(trains::Trains::new(init)),
        Box::new(labels::Labels::new(init)),
        Box::new(route::Route::new(init)),
        Box::new(markers::Markers::new(init)),
        Box::new(aircraft::Aircraft::new(init)),
        Box::new(markers::SelectionRing::new(init)),
    ]
}
