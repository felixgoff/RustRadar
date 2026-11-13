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
//! | [`ground::land::GroundLand`] | 3 `ground-land` (realistic) | R1 |
//! | [`ground::Ground`] | 3c–3f realistic land, land cover, city ground or ground, rail ground | R1–R3, R8–R10 |
//! | [`ground::rails::GroundRails`] | 3f `ground-rails-*`, `ground-platforms` (realistic) | R9 |
//! | [`earth::Land`] | 3i `land` | E5 |
//! | [`hillshade::Hillshade`] | 3b, 3g, 3j shaded relief | R12 |
//! | [`clouds::Clouds`] | 3h `clouds-*` | R10 |
//! | [`vector_map::VectorMap`] | 3k `map-*`, 3l `aeroway-true-*` | B2, B3 |
//! | [`rail::Rail`] | 4 rail network | T4, T7 |
//! | [`radar::Radar`] | 5a, 5b radar tiles, lifted radar, measured volumes | W1–W4 |
//! | [`night::Night`] | 6 `night` | B5 |
//! | [`buildings::Buildings`] | 7 city buildings (dark, satellite) | B7 |
//! | [`buildings_realistic::BuildingsRealistic`] | 7 city buildings (realistic) | R4–R6 |
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
//! The terrain-draping hooks (stage 9): [`Layer::drape`] says how a layer stands on the 3D terrain;
//! while the terrain is drawn the renderer leaves the [`Drape::Cover`] layers out of the map pass
//! and draws them into the terrain tiles' covers instead ([`Pass::Cover`], `terrain::cover`), whose
//! bookkeeping the terrain layer hands it ([`Layer::covers`]); a [`Drape::Part`] layer is drawn in
//! both, its flat parts where [`DrawCx::draws_draped`] says. The tiles a cover layer draws
//! differently each frame reach the covers through [`PrepareCx::update_tiles`], and what it redraws
//! otherwise through [`note_drawn_changes`] ([`drawn_changes`]).
//!
//! Tiles (plan 1.4.8; the renderer owns the runtime, item T1). A layer with tiles keeps a
//! [`TileSet`] made in `new` with [`Init::tile_set`]: its sources (names of [`crate::io`]), its
//! selection ([`DeckSelect`] for deck's rule, [`LodSelect`] for the distance LOD),
//! its limits and refinement, and its decode function, which runs on the shared worker pool
//! (bytes → `core::mvt` → `scene::tess` → CPU buffers in tile-local units). Each `prepare` it calls
//! [`PrepareCx::update_tiles`] once: the set selects for this frame's camera, collects answers,
//! decodes, and uploads decoded tiles within the frame's [`UploadBudget`], which all layers share,
//! through the layer's upload function (CPU buffers → GPU buffers, on the main thread). Then for
//! each tile of [`TileSet::visible`] it pushes the tile's frame ([`PrepareCx::tile_frame`]) and
//! draws it in `draw`. Its [`Layer::pending`] is [`TileSet::loading`] while it draws (the headless
//! path renders until no layer is pending). A layer that stops drawing (another basemap) calls
//! [`TileSet::clear`] or keeps its cache, but reports pending only while it draws.
//!
//! Lights: every lit layer reads the scene's one [`LightRig`] (`SceneInput::lights`, aimed at each
//! scene refresh, G:2723) and hands it to its mesh pipelines with [`PrepareCx::set_lights`].

pub mod aircraft;
pub mod blocks;
pub mod building_shadows;
pub mod buildings;
pub mod buildings_realistic;
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
pub mod roofs;
pub mod route;
pub mod ship_models;
pub mod ships;
pub mod sightings;
pub mod sky;
pub mod terrain;
pub mod train_cars;
pub mod train_route;
pub mod trains;
pub mod trees;
pub mod vector_map;

use std::cell::RefCell;

use crate::frame::{Bindings, FrameArena, FrameSlot, GLOBALS_GROUP, ViewFrame};
use crate::gpu::Gpu;
use crate::jobs::Cancel;
use crate::pipelines::mesh::{LightRig, MeshPipeline};
use crate::renderer::SceneInput;
use crate::textures::RealisticTextures;
pub use crate::tiles::{ByteSize, DeckSelect, LodSelect, Refinement, Select, Source, TileIndex, TileRuntime, TileSet, TileSetOptions, UploadBudget};

/// How a layer stands on 3D terrain (plan 1.4.12, stage 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drape {
    /// Drawn as it is (sky, earth, terrain, clouds).
    None,
    /// Drawn top-down into the terrain tiles' cover textures (ground, hillshade, rail map, radar
    /// ground, shadows, foot shade, night).
    Cover,
    /// Its flat parts (lines, shadows on the ground) drawn into the covers, the rest standing in
    /// the map pass: deck drapes a sublayer of a composite layer, or offsets it, by its own kind.
    Part,
    /// Raised on the CPU by the terrain height where its instances are built (buildings, trees,
    /// vehicles, labels, markers).
    Offset,
}

/// Which pass a layer is drawing into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// The map pass: [`crate::target::MAP_FORMAT`] with MSAA and depth/stencil.
    Map,
    /// A terrain cover pass (stage 9, `terrain::cover`): one terrain tile's square seen from
    /// straight above, into a texture of the same format, sample count and depth/stencil as the map
    /// pass's; only [`Drape::Cover`] and [`Drape::Part`] layers are drawn into it.
    Cover,
}

impl Pass {
    /// Whether a layer that stands on the terrain as `drape` draws in this pass: every layer in the
    /// map pass (the renderer leaves the cover layers out of it while the terrain is drawn), the
    /// cover layers and those draped in part in a cover pass too.
    pub fn draws(self, drape: Drape) -> bool {
        self == Pass::Map || drape.into_covers()
    }
}

impl Drape {
    /// Whether some of what the layer draws goes into the terrain's covers.
    pub fn into_covers(self) -> bool {
        matches!(self, Drape::Cover | Drape::Part)
    }
}

thread_local! {
    /// The tiles whose drawing changed in the layer being prepared, while the renderer listens
    /// ([`listen_for_drawn_changes`], [`drawn_changes`]): filled by [`PrepareCx::update_tiles`].
    static DRAWN_CHANGES: RefCell<Option<Vec<TileIndex>>> = const { RefCell::new(None) };
}

/// From now on (on this thread) [`PrepareCx::update_tiles`] notes the tiles whose drawing changed:
/// those that came to be drawn or stopped being drawn, or, when only their content changed, every
/// tile drawn. The renderer listens around each cover layer's `prepare` while the terrain is drawn.
pub fn listen_for_drawn_changes() {
    DRAWN_CHANGES.with(|c| *c.borrow_mut() = Some(Vec::new()));
}

/// The tiles noted since [`listen_for_drawn_changes`]; it stops listening.
pub fn drawn_changes() -> Vec<TileIndex> {
    DRAWN_CHANGES.with(|c| c.borrow_mut().take()).unwrap_or_default()
}

/// What a layer draws into the terrain's covers changed in `tiles` other than through
/// [`PrepareCx::update_tiles`] (shadows recast for a new sun, say): the covers they meet are drawn
/// again. Nothing while the renderer does not listen (the terrain not drawn).
pub fn note_drawn_changes(tiles: impl IntoIterator<Item = TileIndex>) {
    DRAWN_CHANGES.with(|c| {
        if let Some(log) = c.borrow_mut().as_mut() {
            log.extend(tiles);
        }
    });
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
    /// The renderer's tile runtime: the app's fetcher and the worker pool (also for work that is
    /// not tiles: `cx.tiles.pool`).
    pub tiles: &'a TileRuntime,
}

impl Init<'_> {
    /// The map pass's multisample state.
    pub fn multisample(&self) -> wgpu::MultisampleState {
        wgpu::MultisampleState { count: self.samples, ..Default::default() }
    }

    /// A tile set on the renderer's runtime: tiles from `sources` (a tile's zoom picks the first
    /// that serves it), chosen by `select`, within `options`, decoded by `decode` on the worker
    /// pool (`D`: CPU content; `T`: what the layer's upload makes of it). See [`TileSet::new`].
    pub fn tile_set<D: ByteSize + Send + 'static, T>(
        &self,
        sources: Vec<Source>,
        select: impl Select + 'static,
        options: TileSetOptions,
        decode: impl Fn(TileIndex, Vec<u8>, &Cancel) -> Result<Option<D>, String> + Send + Sync + 'static,
    ) -> TileSet<D, T> {
        TileSet::new(self.tiles, sources, select, options, decode)
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
    /// The renderer's tile runtime (as [`Init::tiles`]).
    pub tiles: &'a TileRuntime,
    /// What may still be uploaded this frame, shared by every layer in draw order (plan 1.4.4: 4
    /// tiles or 8 MB a frame in the window; no limit headless).
    pub budget: &'a mut UploadBudget,
    /// The realistic map's textures, shared by its layers (doc 04 §5): the stand-ins until a layer
    /// starts them loading (`ground::load_textures`), then the files' once they are all in.
    pub textures: &'a mut RealisticTextures,
}

impl PrepareCx<'_> {
    /// One frame of `set`'s work ([`TileSet::update`]) for this frame's camera, within the frame's
    /// upload budget; `upload` turns a decoded tile into what the layer draws, on the main thread.
    /// Whether the tiles drawn changed.
    pub fn update_tiles<D: ByteSize + Send + 'static, T>(&mut self, set: &mut TileSet<D, T>, mut upload: impl FnMut(&Gpu, TileIndex, D) -> T) -> bool {
        let gpu = self.gpu;
        let listening = DRAWN_CHANGES.with(|c| c.borrow().is_some());
        let before: Vec<TileIndex> = if listening { set.visible().map(|(t, _)| t).collect() } else { Vec::new() };
        let changed = set.update(&self.view.viewport, self.budget, |tile, decoded| upload(gpu, tile, decoded));
        if listening && changed {
            // (the terrain's covers: what came, what went, or all drawn when only their content changed)
            let after: Vec<TileIndex> = set.visible().map(|(t, _)| t).collect();
            let mut moved: Vec<TileIndex> = before.iter().filter(|t| !after.contains(t)).chain(after.iter().filter(|t| !before.contains(t))).copied().collect();
            if moved.is_empty() {
                moved = after;
            }
            DRAWN_CHANGES.with(|c| c.borrow_mut().as_mut().map(|log| log.extend(moved)));
        }
        changed
    }

    /// Pushes the local Mercator frame of `tile` (tile units, `v` south; `z_offset_m` metres up,
    /// [`ViewFrame::tile`]) for this frame's draws.
    pub fn tile_frame(&mut self, tile: TileIndex, z_offset_m: f64) -> FrameSlot {
        self.frames.push(&self.view.tile(tile, z_offset_m))
    }

    /// The scene's light rig (`SceneInput::lights`).
    pub fn lights(&self) -> &LightRig {
        &self.scene.lights
    }

    /// Writes this frame's lights (the scene's rig, turned for this frame's camera) into
    /// `pipeline`: every frame, for each mesh pipeline a layer draws with.
    pub fn set_lights(&self, pipeline: &MeshPipeline) {
        pipeline.set_lights(&self.gpu.queue, &self.scene.lights, self.view);
    }
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

    /// Whether a [`Drape::Part`] layer draws its flat parts in this pass: into the covers, or in
    /// the map pass while the 3D terrain is not drawn.
    pub fn draws_draped(&self) -> bool {
        self.pass == Pass::Cover || !self.scene.terrain.shown
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
    /// The terrain's covers while it is drawn (the terrain layer's, stage 9): the renderer draws
    /// the [`Drape::Cover`] and [`Drape::Part`] layers into them.
    fn covers(&mut self) -> Option<&mut terrain::cover::Covers> {
        None
    }
    /// The ground the 3D layers stand on, the heights of the terrain's tiles drawn (the terrain
    /// layer's, item Z3): the renderer stands the scene on it.
    fn ground(&self) -> Option<terrain::ground::Ground> {
        None
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
        Box::new(ground::land::GroundLand::new(init)),
        Box::new(ground::landcover::GroundLandCover::new(init)),
        Box::new(ground::Ground::new(init)),
        Box::new(ground::rails::GroundRails::new(init)),
        Box::new(earth::Land::new(init)),
        Box::new(hillshade::Hillshade::new(init)),
        Box::new(clouds::Clouds::new(init)),
        Box::new(vector_map::VectorMap::new(init)),
        Box::new(rail::Rail::new(init)),
        Box::new(radar::Radar::new(init)),
        Box::new(night::Night::new(init)),
        Box::new(buildings::Buildings::new(init)),
        Box::new(buildings_realistic::BuildingsRealistic::new(init)),
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
