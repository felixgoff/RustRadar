//! The aircraft's assets on the GPU (item A2; doc 03 §4; `aircraft.ts`, `models.ts`, G:1235-1251):
//! the published models' meshes at both levels of detail with their landing gear, the built-in
//! procedural models, and the two silhouette atlases.
//!
//! - [`published`]: `static/aircraft/index.json` and `meshes.bin`, embedded ([`INDEX_JSON`],
//!   [`MESHES_BIN`]), read once per process into `scene`'s [`AircraftModels`]
//!   (`aircraftModels.load()`). The scene builder looks flights up in it (`forFlight`, a model's
//!   size, its gear's `attachZ`, a mesh's lowest point), and its `is_some()` is today's
//!   `aircraftModels.loaded`. If the files cannot be read it is `None`, with a warning, and as
//!   today the built-in models draw every aircraft and icons are never used.
//! - [`AircraftAssets`]: what the aircraft layers draw with, made once in the layer's `new`.
//!   Every published model's levels of detail and gear are uploaded there, and the ten built-in
//!   models ([`rustradar_scene::aircraft_models`]); the atlases start transcoding on the worker
//!   pool. Each `prepare` calls [`AircraftAssets::poll`] (uploads an atlas once it is transcoded:
//!   the pool wakes the loop, as today's `render()` on load) and the layer's `pending` includes
//!   [`AircraftAssets::pending`] (the headless path waits for the atlases).
//! - [`AircraftShape`]: one mesh with its livery parts, drawn with
//!   [`crate::pipelines::mesh::MeshPipeline`]: `draw_range(…, shape.combined(), …)` for the whole
//!   aircraft in one colour (the TypeScript's `combined`), `draw_range(…, range, …)` for each of
//!   [`AircraftShape::parts`]. A published level of detail is one vertex and one index buffer whose
//!   index buffer is ordered by part, so a part is a range of indices (doc 03 §4.2); a built-in
//!   model is one unindexed buffer, three vertices a triangle, its parts concatenated in
//!   [`rustradar_core::liveries::PARTS`] order, so a part is a range of vertices (doc 03 §4.5).
//!   Vertices carry no colours (deck's white), so the instance colour is the aircraft's.
//! - [`GearShape`]: the landing gear, and the plane its struts hang from.
//! - [`Atlas`]: `icons.ktx2` (white, the silhouette in alpha) and `icons-outline.ktx2` (the
//!   silhouettes grown by 9 cell pixels), transcoded to the device's format
//!   ([`crate::textures::Target`]), 2D, not sRGB, clamped, with their own mip levels
//!   ([`AIRCRAFT_ICONS`], [`AIRCRAFT_ICONS_OUTLINE`]); [`Atlas::texture`] is `None` while it
//!   loads or if it failed (doc 03 §4.4: the icon layer is then not drawn), [`Atlas::icon`] a
//!   model's cell as the icon pipeline's [`IconDef`] (white masks, tinted by the instance colour).
//!
//! In the aircraft layer (`layers/aircraft.rs`):
//!
//! ```ignore
//! // new(init)
//! let assets = assets::AircraftAssets::new(init);
//! // prepare(cx)
//! self.assets.poll(cx.gpu);
//! // pending()
//! self.assets.pending()
//! ```
//!
//! Deviations, both invisible: the files are embedded, so the meshes are there from the first
//! frame (today they arrive by `fetch` a moment after mount, the built-in models standing in
//! meanwhile); the atlases start loading when the layer is made, not at the first frame that
//! draws icons. Each mesh is uploaded once, not once per deck layer that draws it.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;

use rustradar_core::liveries::Part;
use rustradar_scene::aircraft_assets::{AircraftModels, IconCell, IndexedMesh, ModelLod};
use rustradar_scene::aircraft_models::{self, MODEL_KINDS, ModelKind, ModelMeshes};

use crate::gpu::Gpu;
use crate::layers::Init;
use crate::pipelines::icon::IconDef;
use crate::pipelines::mesh::{Shape, Vertex};
use crate::textures::{AIRCRAFT_ICONS, AIRCRAFT_ICONS_OUTLINE, Ktx2Texture, LoadOptions, Texture, TextureFile};

/// `static/aircraft/index.json`: the models, the lookups by type and icon class, the atlases'
/// layout ([`rustradar_scene::aircraft_assets::INDEX_FILE`]).
pub static INDEX_JSON: &[u8] = include_bytes!("../../../../../static/aircraft/index.json");
/// `static/aircraft/meshes.bin`: every mesh the index points into.
pub static MESHES_BIN: &[u8] = include_bytes!("../../../../../static/aircraft/meshes.bin");
/// `static/aircraft/icons.ktx2`: the silhouettes, 7 × 6 cells of 128 px.
pub static ICONS_KTX2: &[u8] = include_bytes!("../../../../../static/aircraft/icons.ktx2");
/// `static/aircraft/icons-outline.ktx2`: their borders, 7 × 6 cells of 152 px.
pub static OUTLINE_KTX2: &[u8] = include_bytes!("../../../../../static/aircraft/icons-outline.ktx2");

/// The embedded bytes of an aircraft file by its path under `static/` (the paths of
/// [`rustradar_scene::aircraft_assets`] and of the [`TextureFile`]s).
pub fn embedded(path: &str) -> Option<&'static [u8]> {
    Some(match path {
        "aircraft/index.json" => INDEX_JSON,
        "aircraft/meshes.bin" => MESHES_BIN,
        "aircraft/icons.ktx2" => ICONS_KTX2,
        "aircraft/icons-outline.ktx2" => OUTLINE_KTX2,
        _ => return None,
    })
}

/// The published models, read from the embedded files once per process (`aircraftModels.load()`);
/// `None` if they cannot be read (logged once: "aircraft models unavailable"), which is today's
/// `loaded == false`: built-in models for everything, no icons.
pub fn published() -> Option<&'static AircraftModels> {
    static MODELS: OnceLock<Option<AircraftModels>> = OnceLock::new();
    MODELS
        .get_or_init(|| match AircraftModels::parse(INDEX_JSON, MESHES_BIN) {
            Ok(models) => Some(models),
            Err(e) => {
                // the procedural models stay as the fallback; nothing else breaks
                log::warn!("aircraft models unavailable: {e}");
                None
            }
        })
        .as_ref()
}

/// One mesh on the GPU with its livery parts: a published level of detail (indexed) or a
/// built-in model (unindexed).
pub struct AircraftShape {
    pub shape: Shape,
    /// Each part's elements of [`Self::shape`] (indices if indexed, else vertices), in the
    /// file's order (published) or [`rustradar_core::liveries::PARTS`] order (built-in). Only
    /// the parts the mesh has.
    pub parts: Vec<(Part, Range<u32>)>,
}

impl AircraftShape {
    /// Every element: the whole aircraft (the TypeScript's `combined` geometry).
    pub fn combined(&self) -> Range<u32> {
        0..self.shape.elements()
    }

    /// The elements of `part`, if the mesh has it.
    pub fn part(&self, part: Part) -> Option<Range<u32>> {
        self.parts.iter().find(|(p, _)| *p == part).map(|(_, range)| range.clone())
    }

    /// A published level of detail: its vertices, its index buffer, its parts' index ranges.
    fn indexed(device: &wgpu::Device, label: &str, lod: &ModelLod) -> AircraftShape {
        AircraftShape { shape: indexed_shape(device, label, &lod.mesh), parts: lod.parts.clone() }
    }

    /// A built-in model: its combined mesh, each part the run of vertices it adds to it.
    fn unindexed(device: &wgpu::Device, label: &str, model: &ModelMeshes) -> AircraftShape {
        let vertices = Vertex::from_arrays(&model.combined.positions, &model.combined.normals, None);
        let mut at = 0u32;
        let parts = model
            .parts
            .iter()
            .map(|(part, mesh)| {
                let n = mesh.vertices() as u32;
                let range = at..at + n;
                at += n;
                (*part, range)
            })
            .collect();
        debug_assert_eq!(at as usize, vertices.len(), "{label}: the combined mesh is its parts one after another");
        AircraftShape { shape: Shape::new(device, label, &vertices, None), parts }
    }
}

/// An atlas's cells by model name, from `scene`'s mapping.
fn cells<'a>(mapping: impl Iterator<Item = (&'a String, &'a IconCell)>) -> HashMap<String, IconCell> {
    mapping.map(|(name, cell)| (name.clone(), *cell)).collect()
}

fn indexed_shape(device: &wgpu::Device, label: &str, mesh: &IndexedMesh) -> Shape {
    let vertices = Vertex::from_arrays(&mesh.positions, &mesh.normals, None);
    Shape::with_u16_indices(device, label, &vertices, &mesh.indices)
}

/// The landing gear on the GPU (`ModelGear`).
pub struct GearShape {
    /// Nose and main gear, extended, in the levels of detail's frame.
    pub shape: Shape,
    /// Height (model z, metres) of the plane the struts hang from: the gear is scaled vertically
    /// about it to retract (doc 03 §5.3).
    pub attach_z: f64,
}

/// A published model on the GPU (`AircraftModel`).
pub struct ModelShapes {
    pub name: String,
    /// Largest dimension in metres.
    pub size: f64,
    /// Coarsest first.
    pub lods: Vec<AircraftShape>,
    /// `None` for types without gear (`ask21`, `heli`).
    pub gear: Option<GearShape>,
}

impl ModelShapes {
    /// The level of detail for `detail` (`real.lods[Math.min(detail, real.lods.length - 1)]`);
    /// `None` only for a model with no level at all (none in the shipped file).
    pub fn lod(&self, detail: usize) -> Option<&AircraftShape> {
        self.lods.get(detail.min(self.lods.len().saturating_sub(1)))
    }
}

/// A silhouette atlas: its texture once transcoded and uploaded, and each model's cell.
pub struct Atlas {
    file: TextureFile,
    /// `None` when there is no such atlas to load (the models did not load; an index without
    /// outlines).
    texture: Option<Ktx2Texture>,
    cells: HashMap<String, IconCell>,
}

impl Atlas {
    /// `file` transcoding on the pool for a device that takes `options`, with `cells`.
    fn load(init: &Init<'_>, file: TextureFile, options: LoadOptions, cells: HashMap<String, IconCell>) -> Atlas {
        let texture = embedded(file.path).map(|bytes| Ktx2Texture::load(&init.tiles.pool, file, Cow::Borrowed(bytes), options));
        Atlas { file, texture, cells }
    }

    /// No atlas: nothing to draw.
    fn none(file: TextureFile) -> Atlas {
        Atlas { file, texture: None, cells: HashMap::new() }
    }

    /// The file it is made from.
    pub fn file(&self) -> &TextureFile {
        &self.file
    }

    /// The texture, once loaded (`atlas(url)`: `null` while it loads or if it failed).
    pub fn texture(&self) -> Option<&Texture> {
        self.texture.as_ref()?.texture()
    }

    /// Its size in texels (mip level 0), once loaded: the icon pipeline's `atlas_size`.
    pub fn size(&self) -> Option<[u32; 2]> {
        self.texture().map(|t| t.size)
    }

    /// The cell of the model `name` (the published model's name, `iconNames[i]`), anchored at its
    /// middle and a mask; `None` for a name the atlas has no cell for (deck draws nothing then).
    pub fn icon(&self, name: &str) -> Option<IconDef> {
        let c = self.cells.get(name)?;
        Some(IconDef::cell(f64::from(c.x), f64::from(c.y), f64::from(c.width), f64::from(c.height), c.mask))
    }

    /// How many cells it has.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Still transcoding.
    pub fn pending(&self) -> bool {
        self.texture.as_ref().is_some_and(Ktx2Texture::pending)
    }

    /// Uploads the texture once transcoded; true when it just became ready.
    fn poll(&mut self, gpu: &Gpu) -> bool {
        self.texture.as_mut().is_some_and(|t| t.poll(&gpu.device, &gpu.queue))
    }
}

/// The aircraft's meshes and atlases on the GPU (see the module's documentation).
pub struct AircraftAssets {
    /// The published models by name; empty when they did not load.
    models: HashMap<String, ModelShapes>,
    loaded: bool,
    /// The built-in models, in [`MODEL_KINDS`] order (`kind as usize`).
    built_in: Vec<AircraftShape>,
    icons: Atlas,
    outline: Atlas,
    /// `outlineScale`: an outline cell's size over a silhouette cell's (1 without outlines).
    outline_scale: f64,
}

impl AircraftAssets {
    /// The published models of [`published`] and the built-in ones, uploaded; the atlases
    /// transcoding on `init`'s worker pool.
    pub fn new(init: &Init<'_>) -> Self {
        Self::with_models(init, published())
    }

    /// [`Self::new`] with `models` as the published models (`None`: as if they had failed to load).
    pub fn with_models(init: &Init<'_>, models: Option<&AircraftModels>) -> Self {
        let device = &init.gpu.device;
        let built_in = MODEL_KINDS
            .iter()
            .map(|&kind| AircraftShape::unindexed(device, &format!("aircraft built-in {}", kind.as_str()), aircraft_models::model(kind)))
            .collect();
        let Some(models) = models else {
            return AircraftAssets {
                models: HashMap::new(),
                loaded: false,
                built_in,
                icons: Atlas::none(AIRCRAFT_ICONS),
                outline: Atlas::none(AIRCRAFT_ICONS_OUTLINE),
                outline_scale: 1.0,
            };
        };
        let shapes = models
            .iter()
            .map(|m| {
                let lods = m
                    .lods
                    .iter()
                    .enumerate()
                    .map(|(level, lod)| AircraftShape::indexed(device, &format!("aircraft {} lod {level}", m.name), lod))
                    .collect();
                let gear = m.gear.as_ref().map(|g| GearShape {
                    shape: indexed_shape(device, &format!("aircraft {} gear", m.name), &g.mesh),
                    attach_z: g.attach_z,
                });
                (m.name.clone(), ModelShapes { name: m.name.clone(), size: m.size, lods, gear })
            })
            .collect();
        let options = LoadOptions::for_device(device);
        let icons = Atlas::load(init, AIRCRAFT_ICONS, options, cells(models.icon_mapping.iter()));
        let outline = match models.outline_atlas {
            Some(_) => Atlas::load(init, AIRCRAFT_ICONS_OUTLINE, options, cells(models.outline_mapping.iter())),
            None => Atlas::none(AIRCRAFT_ICONS_OUTLINE),
        };
        AircraftAssets { models: shapes, loaded: true, built_in, icons, outline, outline_scale: models.outline_scale }
    }

    /// Once a frame, in `prepare`: uploads an atlas that has finished transcoding. True when one
    /// just became ready (the icon layer can draw from this frame on).
    pub fn poll(&mut self, gpu: &Gpu) -> bool {
        let icons = self.icons.poll(gpu);
        let outline = self.outline.poll(gpu);
        icons || outline
    }

    /// Whether an atlas is still transcoding.
    pub fn pending(&self) -> bool {
        self.icons.pending() || self.outline.pending()
    }

    /// Whether the published models loaded (`aircraftModels.loaded`).
    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// A published model by name (`AircraftModel::name`, as the scene builder's lookup gives it).
    pub fn model(&self, name: &str) -> Option<&ModelShapes> {
        self.models.get(name)
    }

    /// Every published model, in no particular order.
    pub fn models(&self) -> impl Iterator<Item = &ModelShapes> {
        self.models.values()
    }

    /// A built-in model (`models()[kind]`).
    pub fn built_in(&self, kind: ModelKind) -> &AircraftShape {
        &self.built_in[kind as usize]
    }

    /// The silhouettes (`iconAtlas`, `iconMapping`).
    pub fn icons(&self) -> &Atlas {
        &self.icons
    }

    /// The silhouettes' borders (`outlineAtlas`, `outlineMapping`).
    pub fn outline(&self) -> &Atlas {
        &self.outline
    }

    /// `outlineScale`: an outline's size in pixels is its icon's times this (1.1875).
    pub fn outline_scale(&self) -> f64 {
        self.outline_scale
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::textures::{Target, transcode};

    /// A static file as it is on disk.
    fn static_file(path: &str) -> Vec<u8> {
        let full = format!("{}/../../static/{path}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&full).unwrap_or_else(|e| panic!("{full}: {e}"))
    }

    /// The bounding box `[x0, y0, x1, y1]` (inclusive texels) of the texels of `cell` in an RGBA
    /// image `width` wide whose alpha is at least `alpha`, and how many there are.
    fn alpha_box(rgba: &[u8], width: u32, cell: &IconCell, alpha: u8) -> (Option<[u32; 4]>, usize) {
        let mut found: Option<[u32; 4]> = None;
        let mut count = 0;
        for y in cell.y..cell.y + cell.height {
            for x in cell.x..cell.x + cell.width {
                if rgba[((y * width + x) * 4 + 3) as usize] >= alpha {
                    count += 1;
                    let b = found.get_or_insert([x, y, x, y]);
                    *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
                }
            }
        }
        (found, count)
    }

    #[test]
    fn the_embedded_files_are_the_static_ones() {
        for path in ["aircraft/index.json", "aircraft/meshes.bin", AIRCRAFT_ICONS.path, AIRCRAFT_ICONS_OUTLINE.path] {
            assert!(embedded(path).unwrap() == static_file(path).as_slice(), "{path}");
        }
        assert_eq!(embedded(rustradar_scene::aircraft_assets::INDEX_FILE), Some(INDEX_JSON));
        assert_eq!(embedded(rustradar_scene::aircraft_assets::MESHES_FILE), Some(MESHES_BIN));
        assert_eq!(embedded(rustradar_scene::aircraft_assets::ICON_ATLAS), Some(ICONS_KTX2));
        assert_eq!(embedded(rustradar_scene::aircraft_assets::OUTLINE_ATLAS), Some(OUTLINE_KTX2));
        assert_eq!(embedded("aircraft/other.bin"), None);
    }

    #[test]
    fn the_published_models_are_read_once() {
        let started = Instant::now();
        let parsed = AircraftModels::parse(INDEX_JSON, MESHES_BIN).unwrap();
        eprintln!("index.json and meshes.bin read in {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
        let models = published().expect("the shipped files read");
        assert!(std::ptr::eq(models, published().unwrap()), "once per process");
        assert_eq!(*models, parsed);
        assert_eq!(models.len(), 40);
    }

    /// The acceptance's second half on the files: every silhouette's cell holds its model, centred,
    /// across it the model's span at 94 % of the cell per `size` (`build.mjs:595`), and nothing
    /// bleeds over a cell's edge; every outline is its silhouette grown by 9 texels in the padded
    /// cell. Every type and icon class the index maps resolves to a non-empty cell. (That the nose
    /// points up is checked against the meshes in `icons_and_meshes_agree`.)
    #[test]
    fn atlas_cells_of_the_icon_classes_are_non_empty() {
        let models = published().unwrap();
        let index = rustradar_scene::aircraft_assets::AircraftIndex::parse(INDEX_JSON).unwrap();
        let icons = transcode(ICONS_KTX2, Target::Rgba).unwrap();
        let outline = transcode(OUTLINE_KTX2, Target::Rgba).unwrap();
        assert_eq!((icons.width, icons.height, icons.levels.len()), (896, 768, 10));
        assert_eq!((outline.width, outline.height, outline.levels.len()), (1064, 912, 11));
        let (icons, outline) = (&icons.levels[0].data, &outline.levels[0].data);
        assert_eq!(models.icon_mapping.len(), 40);
        let (mut worst_span, mut lengths_close) = (0.0f64, 0);
        for (name, cell) in &models.icon_mapping {
            let model = models.get(name).unwrap();
            let (Some(b), count) = alpha_box(icons, 896, cell, 128) else { panic!("{name}: an empty cell") };
            // no silhouette touches its cell's edge
            let (Some(all), _) = alpha_box(icons, 896, cell, 1) else { unreachable!() };
            assert!(all[0] > cell.x && all[1] > cell.y && all[2] < cell.x + cell.width - 1 && all[3] < cell.y + cell.height - 1, "{name}: {all:?} reaches the edge of {cell:?}");
            // its width is the span and its height the length, at 0.94 · cell / size
            let scale = 0.94 * f64::from(cell.width) / model.size;
            let (w, h) = (f64::from(b[2] - b[0] + 1), f64::from(b[3] - b[1] + 1));
            let (span, length) = (model.span * scale, model.length * scale);
            worst_span = worst_span.max((w - span).abs());
            assert!((w - span).abs() <= 1.5, "{name}: {w} texels across, the model's span {span:.1}");
            // along it the length, less where the source's nose or tail ends in parts too thin to
            // cover half a texel (34 of the shipped 40 are within 2.5; b736 is 15 short)
            assert!(h <= length + 1.5 && h >= 0.85 * length, "{name}: {h} texels along, the model's length {length:.1}");
            lengths_close += usize::from((h - length).abs() <= 2.5);
            // centred
            let centre = [f64::from(b[0] + b[2] + 1) / 2.0 - f64::from(cell.x), f64::from(b[1] + b[3] + 1) / 2.0 - f64::from(cell.y)];
            assert!(centre.iter().all(|c| (c - 64.0).abs() <= 0.04 * 120.0 + 1.0), "{name}: centred at {centre:?}");
            assert!(count as f64 > 0.05 * span * length, "{name}: {count} texels");
            // the outline: the same silhouette 12 texels in, grown by 9
            let o = &models.outline_mapping[name];
            assert_eq!((o.x, o.y, o.width), (cell.x / 128 * 152, cell.y / 128 * 152, 152));
            let (Some(ob), ocount) = alpha_box(outline, 1064, o, 128) else { panic!("{name}: an empty outline cell") };
            let grown = [b[0] - cell.x + 12 - 9, b[1] - cell.y + 12 - 9, b[2] - cell.x + 12 + 9, b[3] - cell.y + 12 + 9];
            let got = [ob[0] - o.x, ob[1] - o.y, ob[2] - o.x, ob[3] - o.y];
            assert!(got.iter().zip(grown).all(|(g, e)| g.abs_diff(e) <= 1), "{name}: outline {got:?}, the silhouette grown {grown:?}");
            assert!(ocount > count, "{name}");
        }
        eprintln!("silhouettes within {worst_span:.2} texels of their models' span; {lengths_close} of 40 within 2.5 of their length");
        assert!(lengths_close >= 30, "{lengths_close} of 40 silhouettes as long as their models");
        // every lookup lands on a cell
        for (key, name) in index.types.iter().chain(&index.icons) {
            assert!(models.icon_mapping.contains_key(name), "{key} → {name}: no cell");
        }
        assert_eq!(models.for_flight("", "A320").map(|m| models.icon_mapping[&m.name]), Some(models.icon_mapping["a320"]));
    }

    // ---- on the GPU ----

    use std::cell::Cell;
    use std::rc::Rc;

    use glam::DVec3;
    use rustradar_core::geo::Basemap;
    use rustradar_scene::aircraft_models::model_size;
    use rustradar_scene::camera::ViewState;

    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::{DrawCx, Layer, PrepareCx};
    use crate::pipelines::icon::{self, Batch, IconPipeline, IconStyle};
    use crate::pipelines::mesh::{Instance, Instances, LightRig, Material, MeshPipeline, Style, StyleUniform, orientation_matrix};
    use crate::renderer::{MapRenderer, SceneInput};
    use crate::state::DrawState;

    /// A sheet cell's side, CSS pixels.
    const CELL: f64 = 72.0;
    const COLUMNS: usize = 10;
    /// A model's size (its largest dimension) on the sheet, CSS pixels.
    const MODEL_PX: f64 = 56.0;
    const WHITE: [u8; 4] = [255; 4];
    /// `GEAR_COLOR`, opaque.
    const GEAR: [u8; 4] = [178, 182, 190, 255];
    /// Arlanda, flat and straight down: every cell the same scale. Noon there on the equinox, so
    /// a night layer leaves the sheet alone.
    const VIEW: ViewState = ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 16.0, pitch: 0.0, bearing: 0.0, position: None };
    const NOON_MS: f64 = 1_774_005_000_000.0;

    /// What a cell of the sheet shows.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum What {
        Lod(&'static str, usize),
        Gear(&'static str),
        BuiltIn(ModelKind),
        Icon(&'static str),
        Outline(&'static str),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Mode {
        /// Every mesh in one draw.
        Combined,
        /// Every livery part in a draw of its own.
        Parts,
        /// The silhouettes and their outlines.
        Icons,
    }

    /// The published models in the atlas's order.
    fn names() -> Vec<&'static str> {
        published().unwrap().icon_mapping.keys().map(String::as_str).collect()
    }

    /// The cell at `index` of block `block` (each block 4 rows), its centre in CSS pixels.
    fn centre(block: usize, index: usize) -> [f64; 2] {
        let (row, column) = (block * 4 + index / COLUMNS, index % COLUMNS);
        [(column as f64 + 0.5) * CELL, (row as f64 + 0.5) * CELL]
    }

    /// The sheet: LOD 0 (block 0), LOD 1 (block 1), the gear (block 2), the built-in models
    /// (block 3) as meshes; the silhouettes (block 0) and the outlines (block 1) as icons.
    fn cells(mode: Mode) -> Vec<(What, [f64; 2])> {
        let names = names();
        let mut cells = Vec::new();
        if mode == Mode::Icons {
            for (i, &name) in names.iter().enumerate() {
                cells.push((What::Icon(name), centre(0, i)));
                cells.push((What::Outline(name), centre(1, i)));
            }
            return cells;
        }
        for (i, &name) in names.iter().enumerate() {
            cells.push((What::Lod(name, 0), centre(0, i)));
            cells.push((What::Lod(name, 1), centre(1, i)));
            cells.push((What::Gear(name), centre(2, i)));
        }
        for (i, &kind) in MODEL_KINDS.iter().enumerate() {
            cells.push((What::BuiltIn(kind), centre(3, i)));
        }
        cells
    }

    /// The model axes a cell shows across (east, right) and up the screen (north): the airframes
    /// from above (x, y), the gear from its right side (y, z: nose to the right, struts up).
    fn axes(what: What) -> [usize; 2] {
        if matches!(what, What::Gear(_)) { [1, 2] } else { [0, 1] }
    }

    /// What is drawn MODEL_PX big, metres: a model's size (the published one's, or the built-in
    /// kind's `MODEL_SIZE`); for a gear, its own extent from the side (at the airframe's scale it
    /// would be a few pixels).
    fn size_m(what: What) -> f64 {
        let models = published().unwrap();
        match what {
            What::Lod(name, _) | What::Icon(name) | What::Outline(name) => models.get(name).unwrap().size,
            What::Gear(_) => {
                let e = extent(mesh_positions(what), axes(what));
                (e[2] - e[0]).max(e[3] - e[1])
            }
            What::BuiltIn(kind) => model_size(kind),
        }
    }

    /// How much of its depth (toward the camera) a mesh keeps on the sheet: flattened, it lies at
    /// one distance from the camera, so the perspective of a cell far from the view's centre
    /// neither grows nor moves it (a wing 16 px nearer the camera, 300 px out, lands 3 px further
    /// out).
    const FLAT: f64 = 1e-3;

    /// The model matrix and translation (metres east, north, up) that show a cell's mesh `s` times
    /// its size along [`axes`], its middle at the instance's position, flattened toward the camera.
    fn placement(what: What, s: f64) -> (glam::DMat3, DVec3) {
        let [across, up] = axes(what);
        let e = extent(mesh_positions(what), [across, up]);
        let middle = DVec3::new(-(e[0] + e[2]) / 2.0 * s, -(e[1] + e[3]) / 2.0 * s, 0.0);
        if [across, up] == [0, 1] {
            return (orientation_matrix([0.0; 3], [s, s, s * FLAT]), middle);
        }
        // y east, z north, so x up (toward the camera)
        let model = glam::DMat3::from_cols(DVec3::new(0.0, 0.0, s * FLAT), DVec3::new(s, 0.0, 0.0), DVec3::new(0.0, s, 0.0));
        (model, middle)
    }

    /// The CPU mesh of a cell's mesh: `scene`'s, which the assets were uploaded from.
    fn mesh_positions(what: What) -> &'static [f32] {
        let models = published().unwrap();
        match what {
            What::Lod(name, level) => &models.get(name).unwrap().lods[level].mesh.positions,
            What::Gear(name) => models.get(name).unwrap().gear.as_ref().map_or(&[], |g| &g.mesh.positions),
            What::BuiltIn(kind) => &aircraft_models::model(kind).combined.positions,
            What::Icon(_) | What::Outline(_) => &[],
        }
    }

    /// A test layer: the sheet of `mode` drawn from an [`AircraftAssets`], as the aircraft layers
    /// will draw from it (flat colours, depth 0, so a pixel is exactly the instance colour).
    struct Sheet {
        assets: AircraftAssets,
        meshes: MeshPipeline,
        style: Style,
        icons: IconPipeline,
        mode: Rc<Cell<Mode>>,
        drawn: Mode,
        instances: Vec<(What, Instances)>,
        /// The silhouettes' and the outlines' batches, once their atlases are in.
        batches: [Option<Batch>; 2],
        polls: Rc<Cell<usize>>,
    }

    impl Sheet {
        fn new(init: &crate::layers::Init<'_>, mode: Rc<Cell<Mode>>, polls: Rc<Cell<usize>>) -> Sheet {
            let device = &init.gpu.device;
            let meshes = MeshPipeline::instanced(init, "sheet meshes", DrawState::NO_CULL);
            let style = meshes.style(device, &StyleUniform::new(&Material::aircraft(0.0), 1.0, [255; 4]));
            Sheet {
                assets: AircraftAssets::new(init),
                meshes,
                style,
                icons: IconPipeline::new(init, "sheet icons", DrawState::NO_CULL),
                drawn: mode.get(),
                mode,
                instances: Vec::new(),
                batches: [None, None],
                polls,
            }
        }

        fn shape(&self, what: What) -> (&Shape, Vec<Range<u32>>) {
            let parts = |s: &AircraftShape| s.parts.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>();
            let pick = |s: &'_ AircraftShape| -> Vec<Range<u32>> { if self.drawn == Mode::Parts { parts(s) } else { vec![s.combined()] } };
            match what {
                What::Lod(name, level) => {
                    let s = &self.assets.model(name).unwrap().lods[level];
                    (&s.shape, pick(s))
                }
                What::Gear(name) => {
                    let gear = self.assets.model(name).unwrap().gear.as_ref().unwrap();
                    let whole = 0..gear.shape.elements();
                    (&gear.shape, vec![whole])
                }
                What::BuiltIn(kind) => {
                    let s = self.assets.built_in(kind);
                    (&s.shape, pick(s))
                }
                What::Icon(_) | What::Outline(_) => unreachable!("not a mesh"),
            }
        }
    }

    impl Layer for Sheet {
        fn name(&self) -> &'static str {
            "aircraft sheet"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            let (device, queue) = (&cx.gpu.device, &cx.gpu.queue);
            self.polls.set(self.polls.get() + 1);
            self.assets.poll(cx.gpu);
            self.drawn = self.mode.get();
            self.meshes.set_lights(queue, &LightRig::new(), cx.view);
            let vp = &cx.view.viewport;
            let mpp = vp.meters_per_pixel();
            let at = |xy: [f64; 2]| {
                let [lon, lat] = vp.unproject(xy, 0.0);
                [lon, lat, 0.0]
            };
            self.instances.clear();
            let mut icons = Vec::new();
            let mut outlines = Vec::new();
            for (what, xy) in cells(self.drawn) {
                let size = size_m(what);
                match what {
                    What::Icon(name) => {
                        // the cell's height so the model's size is MODEL_PX, as the meshes
                        let def = self.assets.icons().icon(name).unwrap();
                        icons.push(icon::Instance::at(cx.view, at(xy), &def, MODEL_PX / 0.94, 0.0, WHITE));
                    }
                    What::Outline(name) => {
                        let def = self.assets.outline().icon(name).unwrap();
                        let px = MODEL_PX / 0.94 * self.assets.outline_scale();
                        outlines.push(icon::Instance::at(cx.view, at(xy), &def, px, 0.0, WHITE));
                    }
                    _ => {
                        if matches!(what, What::Gear(name) if self.assets.model(name).unwrap().gear.is_none()) {
                            continue;
                        }
                        let s = MODEL_PX * mpp / size;
                        let color = if matches!(what, What::Gear(_)) { GEAR } else { WHITE };
                        let (model, middle) = placement(what, s);
                        let instance = Instance::at(cx.view, at(xy), model, middle, color, Instance::NO_HIGHLIGHT);
                        let mut buffer = Instances::new(device, "sheet instance");
                        buffer.write(device, queue, &[instance]);
                        self.instances.push((what, buffer));
                    }
                }
            }
            for (slot, (atlas, list)) in self.batches.iter_mut().zip([(self.assets.icons(), icons), (self.assets.outline(), outlines)]) {
                let Some(texture) = atlas.texture() else { continue };
                let batch = slot.get_or_insert_with(|| self.icons.batch(device, "sheet icons", &texture.view, texture.size));
                batch.set_style(queue, &IconStyle::DEFAULT, cx.view);
                batch.set_instances(device, queue, &list);
            }
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {
            for (what, instances) in &self.instances {
                let (shape, ranges) = self.shape(*what);
                for range in ranges {
                    self.meshes.draw_range(pass, &self.style, shape, range, instances);
                }
            }
            if self.drawn == Mode::Icons {
                self.icons.begin(pass);
                for batch in self.batches.iter().flatten() {
                    self.icons.draw_batch(pass, batch);
                }
            }
        }

        fn pending(&self) -> bool {
            self.assets.pending()
        }
    }

    /// The canvas: four blocks of four rows.
    const SIZE: [f64; 2] = [COLUMNS as f64 * CELL, 16.0 * CELL];

    fn shoot(gpu: &Gpu, renderer: &mut MapRenderer) -> Image {
        let mut scene = SceneInput::new(VIEW, SIZE, 1.0, Basemap::Dark);
        scene.feed_now_ms = NOON_MS;
        scene.local_now_ms = NOON_MS;
        render_with(gpu, renderer, &scene, 1.0).expect("rendered")
    }

    fn save(image: &Image, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }

    /// The pixels of the cell round `xy` at least half way from the ink to `color`: their bounding
    /// box (inclusive pixels, relative to the cell) and the set of them (row-major in the cell).
    fn covered(image: &Image, [cx, cy]: [f64; 2], color: [u8; 4]) -> (Option<[i32; 4]>, Vec<bool>) {
        let half = (30 + u32::from(color[1])) / 2;
        let (x0, y0) = ((cx - CELL / 2.0) as u32, (cy - CELL / 2.0) as u32);
        let mut mask = Vec::new();
        let mut b: Option<[i32; 4]> = None;
        for y in 0..CELL as u32 {
            for x in 0..CELL as u32 {
                let on = u32::from(image.pixel(x0 + x, y0 + y)[1]) >= half;
                mask.push(on);
                if on {
                    let (x, y) = (x as i32, y as i32);
                    let r = b.get_or_insert([x, y, x, y]);
                    *r = [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)];
                }
            }
        }
        (b, mask)
    }

    /// The extent `[a0, b0, a1, b1]` of `positions` along the model axes `[a, b]`, metres.
    fn extent(positions: &[f32], [a, b]: [usize; 2]) -> [f64; 4] {
        let mut e = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for p in positions.chunks_exact(3) {
            let (x, y) = (f64::from(p[a]), f64::from(p[b]));
            e = [e[0].min(x), e[1].min(y), e[2].max(x), e[3].max(y)];
        }
        e
    }

    /// Vulkan's (and D3D's) standard positions of the four samples in a pixel.
    const SAMPLES: [[f64; 2]; 4] = [[0.375, 0.125], [0.875, 0.375], [0.125, 0.625], [0.625, 0.875]];

    /// How many of each of the cell's pixels' four samples (row-major) the CPU mesh of `what`,
    /// placed as the sheet places it, covers: what the GPU should draw, edge for edge.
    fn rasterized(what: What) -> Vec<u32> {
        let models = published().unwrap();
        let positions = mesh_positions(what);
        let indices: Vec<usize> = match what {
            What::Lod(name, level) => models.get(name).unwrap().lods[level].mesh.indices.iter().map(|&i| usize::from(i)).collect(),
            What::Gear(name) => models.get(name).unwrap().gear.as_ref().unwrap().mesh.indices.iter().map(|&i| usize::from(i)).collect(),
            _ => (0..positions.len() / 3).collect(),
        };
        let [a, b] = axes(what);
        let e = extent(positions, [a, b]);
        let px_per_m = MODEL_PX / size_m(what);
        let to_px = |i: usize| {
            let p = &positions[3 * i..3 * i + 3];
            [CELL / 2.0 + (f64::from(p[a]) - (e[0] + e[2]) / 2.0) * px_per_m, CELL / 2.0 - (f64::from(p[b]) - (e[1] + e[3]) / 2.0) * px_per_m]
        };
        let side = CELL as usize;
        let mut hits = vec![0u8; side * side];
        let edge = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
        for t in indices.chunks_exact(3) {
            let [p, q, r] = [to_px(t[0]), to_px(t[1]), to_px(t[2])];
            if edge(p, q, r) == 0.0 {
                continue;
            }
            let x0 = p[0].min(q[0]).min(r[0]).floor().max(0.0) as usize;
            let x1 = (p[0].max(q[0]).max(r[0]).ceil() as usize).min(side);
            let y0 = p[1].min(q[1]).min(r[1]).floor().max(0.0) as usize;
            let y1 = (p[1].max(q[1]).max(r[1]).ceil() as usize).min(side);
            for y in y0..y1 {
                for x in x0..x1 {
                    let mut mask = 0u8;
                    for (k, [sx, sy]) in SAMPLES.iter().enumerate() {
                        let at = [x as f64 + sx, y as f64 + sy];
                        let d = [edge(p, q, at), edge(q, r, at), edge(r, p, at)];
                        if d.iter().all(|&v| v >= 0.0) || d.iter().all(|&v| v <= 0.0) {
                            mask |= 1 << k;
                        }
                    }
                    hits[y * side + x] |= mask;
                }
            }
        }
        hits.iter().map(|&m| m.count_ones()).collect()
    }

    /// The acceptance's first half: every model of `index.json` uploads, at both levels of
    /// detail, with its gear, and so do the built-in models, each with the elements and the parts
    /// its CPU mesh has; drawn at a known scale (the airframes from above, nose up, the gear from
    /// the side), each covers the samples its CPU triangles cover, give or take a sample an edge
    /// grazes (two, for a few pixels), and drawing it part by part gives exactly the picture of
    /// drawing it whole. Without the
    /// published models only the built-in ones are there.
    #[test]
    fn every_model_uploads_and_draws() {
        let Some(gpu) = gpu() else { return };
        let models = published().unwrap();
        let mode = Rc::new(Cell::new(Mode::Combined));
        let polls = Rc::new(Cell::new(0));
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        let sheet = Sheet::new(&renderer.init(&gpu), mode.clone(), polls.clone());
        let assets = &sheet.assets;
        assert!(assets.loaded());
        assert_eq!(assets.models().count(), 40);
        for model in models.iter() {
            let shapes = assets.model(&model.name).unwrap();
            assert_eq!((shapes.name.as_str(), shapes.size), (model.name.as_str(), model.size));
            assert_eq!(shapes.lods.len(), model.lods.len(), "{}", model.name);
            for (level, (gpu_lod, lod)) in shapes.lods.iter().zip(&model.lods).enumerate() {
                assert_eq!(gpu_lod.shape.elements() as usize, lod.mesh.indices.len(), "{} lod {level}", model.name);
                assert_eq!(gpu_lod.parts, lod.parts, "{} lod {level}", model.name);
                assert_eq!(gpu_lod.combined(), 0..lod.mesh.indices.len() as u32);
            }
            assert!(std::ptr::eq(shapes.lod(5).unwrap(), &shapes.lods[1]), "past the finest level, the finest");
            assert!(std::ptr::eq(shapes.lod(0).unwrap(), &shapes.lods[0]));
            match (&shapes.gear, &model.gear) {
                (Some(g), Some(cpu)) => {
                    assert_eq!(g.shape.elements() as usize, cpu.mesh.indices.len(), "{} gear", model.name);
                    assert_eq!(g.attach_z, cpu.attach_z);
                }
                (None, None) => {}
                _ => panic!("{}: gear on one side only", model.name),
            }
        }
        for kind in MODEL_KINDS {
            let cpu = aircraft_models::model(kind);
            let shape = assets.built_in(kind);
            assert_eq!(shape.shape.elements() as usize, cpu.combined.vertices(), "{kind:?}");
            let parts: Vec<Part> = shape.parts.iter().map(|(p, _)| *p).collect();
            assert_eq!(parts, cpu.parts.iter().map(|(p, _)| *p).collect::<Vec<_>>(), "{kind:?}");
            // the parts are the combined mesh's runs of vertices, one after another
            let mut at = 0;
            for ((part, range), (_, mesh)) in shape.parts.iter().zip(&cpu.parts) {
                assert_eq!(range.start, at, "{kind:?} {part:?}");
                assert_eq!(range.len(), mesh.vertices());
                let first = range.start as usize * 3;
                assert_eq!(&cpu.combined.positions[first..first + 3], &mesh.positions[..3], "{kind:?} {part:?}");
                at = range.end;
            }
            assert_eq!(at, shape.shape.elements());
            assert_eq!(shape.part(Part::Engines).is_some(), cpu.parts.iter().any(|(p, _)| *p == Part::Engines));
        }
        assert_eq!(assets.model("a318").unwrap().lods[0].part(Part::Belly), Some(66..327));
        assert_eq!(assets.model("a318").unwrap().lods[0].part(Part::Engines), None);
        renderer.add_layer(Box::new(sheet));

        let whole = shoot(&gpu, &mut renderer);
        save(&whole, "a2-models.png");
        mode.set(Mode::Parts);
        let by_parts = shoot(&gpu, &mut renderer);
        assert!(polls.get() >= 4, "the layer was prepared");
        let differ = whole.rgba.chunks_exact(4).zip(by_parts.rgba.chunks_exact(4)).filter(|(a, b)| a != b).count();
        assert_eq!(differ, 0, "drawn part by part, {differ} pixels differ from the whole");

        let (mut drawn, mut all_off, mut all_edges, mut all_two_off) = (0, 0, 0, 0);
        for (what, xy) in cells(Mode::Combined) {
            let color = if matches!(what, What::Gear(_)) { GEAR } else { WHITE };
            if matches!(what, What::Gear(name) if models.get(name).unwrap().gear.is_none()) {
                assert_eq!(covered(&whole, xy, GEAR).0, None, "{what:?}: no gear, nothing drawn");
                continue;
            }
            assert!(covered(&whole, xy, color).0.is_some(), "{what:?}: nothing drawn");
            // each pixel's samples covered, read from its colour (the ink's to the mesh's in
            // quarters), against the CPU triangles' at the standard sample positions: an edge
            // passing within a hair of a sample may fall either side of it
            let expected = rasterized(what);
            let side = CELL as usize;
            let (x0, y0) = ((xy[0] - CELL / 2.0) as u32, (xy[1] - CELL / 2.0) as u32);
            let (mut edges, mut off, mut two_off) = (0, 0, 0);
            for (i, &want) in expected.iter().enumerate() {
                let g = i32::from(whole.pixel(x0 + (i % side) as u32, y0 + (i / side) as u32)[1]);
                let range = i32::from(color[1]) - 30;
                let got = ((g - 30) * 4 + range / 2).div_euclid(range);
                let want = want as i32;
                assert!((got - want).abs() <= 2, "{what:?}: pixel ({}, {}) has {got} of 4 samples, its triangles {want}", i % side, i / side);
                edges += usize::from((1..4).contains(&want) || (1..4).contains(&got));
                off += usize::from(got != want);
                two_off += usize::from((got - want).abs() == 2);
            }
            // two samples off: an edge of slope ½ (the line through two of the four samples)
            // within a hair of both, a few pixels of all the edges
            assert!(two_off <= 2, "{what:?}: {two_off} pixels two samples off their triangles");
            all_two_off += two_off;
            (all_off, all_edges) = (all_off + off, all_edges + edges);
            drawn += 1;
        }
        eprintln!("{drawn} meshes drawn; {all_off} of {all_edges} edge pixels a sample off their triangles, {all_two_off} two");
        assert!(all_two_off <= 20, "{all_two_off}");
        assert_eq!(drawn, 40 * 2 + 38 + 10);

        // without the published models: the built-in ones, no atlas, nothing to wait for
        let init = renderer.init(&gpu);
        let bare = AircraftAssets::with_models(&init, None);
        assert!(!bare.loaded() && bare.models().next().is_none() && bare.model("a318").is_none());
        assert!(!bare.pending() && bare.icons().texture().is_none() && bare.outline().texture().is_none());
        assert!(bare.icons().is_empty() && bare.icons().icon("a318").is_none());
        assert_eq!(bare.outline_scale(), 1.0);
        assert_eq!(bare.built_in(ModelKind::Balloon).shape.elements() as usize, aircraft_models::model(ModelKind::Balloon).combined.vertices());
    }

    /// The atlases on the GPU: transcoded on the worker pool to the device's format, pending until
    /// then and polled in by the layer; drawn with the icon pipeline at the meshes' scale, every
    /// cell shows its model, nose up, covering the pixels its mesh covers seen from above (and
    /// fewer when turned upside down); each outline is its silhouette grown by 9 cell texels all
    /// round.
    #[test]
    fn icons_and_meshes_agree() {
        let Some(gpu) = gpu() else { return };
        let mode = Rc::new(Cell::new(Mode::Combined));
        let polls = Rc::new(Cell::new(0));
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        let sheet = Sheet::new(&renderer.init(&gpu), mode.clone(), polls.clone());
        assert!(sheet.assets.pending(), "the atlases transcode on the pool");
        assert!(sheet.assets.icons().texture().is_none());
        assert_eq!(sheet.assets.icons().len(), 40);
        assert_eq!(sheet.assets.outline().len(), 40);
        assert_eq!(sheet.assets.outline_scale(), 1.1875);
        assert_eq!(sheet.assets.icons().icon("a346"), Some(IconDef::cell(0.0, 128.0, 128.0, 128.0, true)));
        assert_eq!(sheet.assets.outline().icon("q400"), Some(IconDef::cell(608.0, 760.0, 152.0, 152.0, true)));
        assert_eq!(sheet.assets.icons().icon("concorde"), None);
        renderer.add_layer(Box::new(sheet));
        let meshes = shoot(&gpu, &mut renderer);
        mode.set(Mode::Icons);
        let icons = shoot(&gpu, &mut renderer);
        save(&icons, "a2-icons.png");
        assert!(!renderer.pending(), "both atlases in");

        let names = names();
        let mut ious = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let (Some(mb), mesh) = covered(&meshes, centre(1, i), WHITE) else { panic!("{name}: no mesh") };
            let (Some(ib), icon) = covered(&icons, centre(0, i), WHITE) else { panic!("{name}: an empty icon") };
            let (Some(ob), outline) = covered(&icons, centre(1, i), WHITE) else { panic!("{name}: an empty outline") };
            // the icon covers what the mesh covers, the right way up
            let side = CELL as usize;
            let iou = |flip: bool| {
                let (mut both, mut either) = (0, 0);
                for y in 0..side {
                    for x in 0..side {
                        let m = mesh[y * side + x];
                        let k = icon[if flip { side - 1 - y } else { y } * side + x];
                        both += usize::from(m && k);
                        either += usize::from(m || k);
                    }
                }
                both as f64 / either as f64
            };
            let (upright, upside_down) = (iou(false), iou(true));
            eprintln!("{name}: icon over mesh {upright:.3}, turned upside down {upside_down:.3}; icon {ib:?} mesh {mb:?}");
            ious.push((upright, upside_down));
            assert!(upright >= 0.85, "{name}: the icon and the mesh share {upright:.3} of what they cover");
            assert!(upright >= upside_down + 0.2, "{name}: upright {upright:.3}, upside down {upside_down:.3}");
            // the outline holds the icon, grown 9 texels of 128 at this size all round
            let grow = 9.0 * MODEL_PX / 0.94 / 128.0;
            let side_ok = |o: i32, b: i32, sign: f64| (f64::from(o - b) * sign - grow).abs() <= 1.5;
            assert!(
                side_ok(ob[0], ib[0], -1.0) && side_ok(ob[1], ib[1], -1.0) && side_ok(ob[2], ib[2], 1.0) && side_ok(ob[3], ib[3], 1.0),
                "{name}: outline {ob:?} round the icon {ib:?}, {grow:.2} px out"
            );
            assert!(icon.iter().zip(&outline).all(|(&k, &o)| !k || o), "{name}: the outline covers the icon");
        }
        let mean = |f: fn(&(f64, f64)) -> f64| ious.iter().map(f).sum::<f64>() / ious.len() as f64;
        eprintln!("icons over meshes: {:.3} of their pixels shared on average, {:.3} turned upside down", mean(|r| r.0), mean(|r| r.1));
    }
}
