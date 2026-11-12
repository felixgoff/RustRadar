//! The realistic map's buildings (item R4; doc 04 §7.1–7.3; row 9 of doc 04 §1; row 7 of doc 03
//! §3): `buildings-city-realistic` (`cityBuildingLayer` with `realistic`, G:1039-1092, through
//! `BuildingLayer`, `building-layer.ts`, with `BuildingExtension`, `realistic.ts`), each building
//! in its own façade, roof and colour, a window to each bay, lit by the realistic map's light and
//! its windows lit at night. The tessellation is `rustradar_scene::tess::buildings`'s realistic
//! half ([`tess::tessellate_realistic`]); the shader is `shaders/building.wgsl`.
//!
//! What today's app does, and so this layer:
//! - **When.** With the *buildings* setting on, on the flat map (zoom > 12), on the realistic
//!   basemap (G:2694; [`super::buildings::Buildings`] draws the other basemaps'). The scene builder
//!   sets `SceneInput::buildings` from the setting (`app::scene_builder::base`); this layer works
//!   out the projection and the basemap. When it stops drawing it drops its tiles, as deck
//!   finalizes a layer that leaves the list.
//! - **Tiles** by their distance from the camera (`LodMVTLayer`): [`LodSelect`] over
//!   `tess::city_lod()` (`CITY_LOD`: levels 13, 14, 16, bias 0.5, 25 km) with the look-ahead while
//!   zooming, OpenFreeMap's `building` layer at zooms 13 and 14 and Streets GL's `buildings` at its
//!   zoom 16 ([`sources`], `cityBuildingSources`; `io::STREETS_GL` asks for no other zoom, as
//!   `sglFetch` does), 8 requests at once (`maxRequests: 8`), refined by `keepCoarserUntilIn`
//!   ([`Refinement::KeepCoarserUntilIn`]: a coarser tile in place of finer ones until they are all
//!   in, never with them). Decoded on the worker pool, uploaded within the frame's budget, drawn
//!   coarse to fine. So at V5 (zoom 15.5, pitched 60°) every tile is Streets GL's.
//! - **Each tile**: a near one (zoom ≥ 14, `CITY_CLOSE_ZOOM`) extruded, its walls and flat roofs in
//!   the building shader ([`BuildingShader`]: Streets GL's texture arrays, `textures::
//!   BuildingTextures`, through the realistic light, `look::LightUniform`); a far one (zoom 13)
//!   flat on the ground in each building's colour and deck's alpha (`255 − code`), unlit: as
//!   today's app draws it (plan section 7: "as today"; the building extension needs `extruded`).
//!   Both with `NO_CULL` (depth tested and written), clipped to the tile's square by fragment
//!   (MVTLayer's `ClipExtension` on the flat map; the shader's `discard`), as the plain buildings.
//! - **Textures** (doc 04 §5.3): the realistic map's set, started by the first realistic layer that
//!   draws ([`super::ground::load_textures`]); the stand-ins (white, flat, no glow) are bound until
//!   every file is in, and the layer is pending meanwhile (the shots wait for them).
//!
//! For the items after this one: R5 (foot shade, shadows) draws each near tile's
//! [`TileMeshes::footprints`] (cast heights) before its buildings, R6 (pitched roofs) each Streets
//! GL tile's roof mesh after them, through `vs_roof` of the same shader; both go in [`Layer::draw`]'s
//! loop below, tile by tile, in that order (`renderSubLayers`: `[...ground, buildings, ...over]`).
//!
//! How it is checked (the tests below): V5 from the fixtures at noon, each of the nearest walls
//! facing the camera sampled along its top storey on the screen: its brightness repeats once a bay
//! (a window to each bay, `textures::BUILDING_TEXTURES`' window layers); at midwinter night the
//! windows' middles are brighter and warmer than the wall between them. Far tiles lie flat in each
//! building's colour and alpha. The layer asks for Streets GL's tiles only on the realistic flat
//! map with the setting on.
//!
//! Deviations, invisible: the walls are vertices of their own with their bays and storeys worked
//! out on the CPU (doc 04 §16.3), not deck's instanced sides; the textures' mip bias is a scale of
//! the gradients (`building.wgsl`'s header); the clip is per pixel, as deck's discard.

use rustradar_core::geo::Basemap;
use rustradar_scene::lod::TILE_SIZE;
use rustradar_scene::streets_gl::SGL_ZOOM;
use rustradar_scene::tess::buildings::{self as tess, Footprints, RealisticTile, RealisticVertex};

use super::{ByteSize, DrawCx, Init, Layer, LodSelect, Pass, PrepareCx, Refinement, Source, TileIndex, TileSet, TileSetOptions};
use crate::frame::FrameSlot;
use crate::io;
use crate::jobs::Cancel;
use crate::layers::ground::load_textures;
use crate::look::{LightUniform, Look};
use crate::pipelines::fill;
use crate::pipelines::{Desc, build};
use crate::shaders;
use crate::state::DrawState;
use crate::textures::Sampling;

/// `shaders/building.wgsl`.
pub const SHADER: &str = include_str!("../shaders/building.wgsl");

/// The bind group of the buildings' textures (after the light's, group 2).
pub const BUILDING_GROUP: u32 = 3;

/// [`RealisticVertex`]'s buffer: position, normal, uv (`Float32x3`, `Float32x3`, `Float32x2`), the
/// tint (`Unorm8x4`) and the style (`Uint8x4`), locations 0–4.
pub const VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: size_of::<RealisticVertex>() as u64,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Unorm8x4, 4 => Uint8x4],
};

/// The realistic buildings' sources by zoom (`cityBuildingSources` with `realistic`): OpenFreeMap
/// at 13 and 14, Streets GL at its zoom.
pub fn sources() -> Vec<Source> {
    vec![Source::at(io::OPENFREEMAP, tess::SOURCE_ZOOMS.to_vec()), Source::at(io::STREETS_GL, vec![SGL_ZOOM])]
}

impl ByteSize for RealisticTile {
    fn byte_size(&self) -> usize {
        RealisticTile::byte_size(self)
    }
}

/// The decode step, on the worker pool (`None` for a tile with no buildings).
pub fn decode(tile: TileIndex, bytes: &[u8], cancel: &Cancel) -> Result<Option<RealisticTile>, String> {
    match tess::tessellate_realistic(tile, bytes, tess::CITY_CLOSE_ZOOM, &|| cancel.is_cancelled()) {
        Ok(decoded) if decoded.is_empty() => Ok(None),
        Ok(decoded) => Ok(Some(decoded)),
        Err(tess::Cancelled) => Err("cancelled".into()),
    }
}

/// A tile on the GPU.
pub struct TileMeshes {
    /// Standing in the building shader (zoom ≥ 14), or flat and unlit.
    pub extruded: bool,
    /// Streets GL's (zoom 16).
    pub sgl: bool,
    pub mesh: fill::Mesh<RealisticVertex>,
    /// What the tile's shadows and foot shade are cast from (R5).
    pub footprints: Footprints,
}

/// The upload step, on the main thread.
pub fn upload(device: &wgpu::Device, decoded: RealisticTile) -> TileMeshes {
    TileMeshes {
        extruded: decoded.extruded,
        sgl: decoded.sgl,
        mesh: fill::Mesh::new(device, "realistic buildings", &decoded.vertices, &decoded.indices),
        footprints: decoded.footprints,
    }
}

/// What every draw of the building shader shares: the module, the light (group 2) and the
/// textures with their sampler (group 3, made again when the textures swap in). A layer keeps one,
/// makes its pipelines with it and calls [`Self::prepare`] each frame it draws.
pub struct BuildingShader {
    module: wgpu::ShaderModule,
    light: LightUniform,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Group 3, made from the textures as they were (`bound`: ready or not).
    group: Option<wgpu::BindGroup>,
    bound: Option<bool>,
}

impl BuildingShader {
    pub fn new(init: &Init<'_>, label: &str) -> Self {
        let device = &init.gpu.device;
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("buildings"),
            entries: &[
                texture(0),
                texture(1),
                texture(2),
                texture(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        BuildingShader {
            module: shaders::module(device, label, SHADER),
            light: LightUniform::new(device),
            layout,
            sampler: Sampling::Repeat.sampler(device),
            group: None,
            bound: None,
        }
    }

    /// A pipeline of `vertex_entry` (`vs_building`, `vs_roof`) and `fragment_entry`
    /// (`fs_building`, `fs_flat`) with `state`, for [`RealisticVertex`]'s buffer.
    pub fn pipeline(&self, init: &Init<'_>, label: &str, vertex_entry: &str, fragment_entry: &str, state: DrawState) -> wgpu::RenderPipeline {
        build(
            init,
            &Desc {
                label,
                module: &self.module,
                vertex_entry,
                fragment_entry,
                buffers: &[Some(VERTEX_LAYOUT)],
                topology: wgpu::PrimitiveTopology::TriangleList,
                state,
                groups: &[self.light.layout(), &self.layout],
            },
        )
    }

    /// This frame's light (written, and returned), and group 3 made again if the textures have
    /// swapped since.
    pub fn prepare(&mut self, cx: &PrepareCx<'_>) -> Look {
        let ready = cx.textures.ready;
        if self.bound != Some(ready) {
            let t = &cx.textures.buildings;
            self.group = Some(cx.gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("buildings"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&t.color.view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&t.normal.view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&t.mask.view) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&t.glow.view) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            }));
            self.bound = Some(ready);
        }
        self.light.prepare(cx)
    }

    /// Sets `pipeline` and binds groups 2 and 3. After [`Self::prepare`].
    pub fn begin(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline) {
        pass.set_pipeline(pipeline);
        self.light.bind(pass);
        if let Some(group) = &self.group {
            pass.set_bind_group(BUILDING_GROUP, group, &[]);
        }
    }
}

/// Row 9 of doc 04 §1: `buildings-city-realistic`.
pub struct BuildingsRealistic {
    tiles: TileSet<RealisticTile, TileMeshes>,
    shader: BuildingShader,
    /// Near tiles: walls and roofs in the building shader.
    walls: wgpu::RenderPipeline,
    /// Far tiles: footprints flat, unlit.
    flat: wgpu::RenderPipeline,
    /// Whether this frame draws, and whether the textures were still loading.
    shown: bool,
    textures_loading: bool,
    /// This frame's light.
    look: Option<Look>,
    /// This frame's tiles in draw order, each with its frame.
    drawn: Vec<(FrameSlot, TileIndex)>,
}

impl BuildingsRealistic {
    pub fn new(init: &Init<'_>) -> Self {
        let tiles = init.tile_set(
            sources(),
            LodSelect::new(tess::city_lod(), TILE_SIZE),
            TileSetOptions { max_requests: tess::MAX_REQUESTS, refinement: Refinement::KeepCoarserUntilIn, ..Default::default() },
            |tile, bytes, cancel| decode(tile, &bytes, cancel),
        );
        let shader = BuildingShader::new(init, "realistic buildings");
        let walls = shader.pipeline(init, "realistic buildings", "vs_building", "fs_building", DrawState::NO_CULL);
        let flat = shader.pipeline(init, "realistic buildings, far", "vs_building", "fs_flat", DrawState::NO_CULL);
        BuildingsRealistic { tiles, shader, walls, flat, shown: false, textures_loading: false, look: None, drawn: Vec::new() }
    }

    /// The tile set (tests).
    pub fn tiles(&self) -> &TileSet<RealisticTile, TileMeshes> {
        &self.tiles
    }

    /// This frame's tiles in draw order (coarse to fine).
    pub fn drawn(&self) -> impl Iterator<Item = TileIndex> + '_ {
        self.drawn.iter().map(|&(_, tile)| tile)
    }

    /// The light of the last frame drawn.
    pub fn look(&self) -> Option<&Look> {
        self.look.as_ref()
    }
}

impl Layer for BuildingsRealistic {
    fn name(&self) -> &'static str {
        "buildings-realistic"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.drawn.clear();
        self.shown = cx.scene.buildings.shown && !cx.view.is_globe() && cx.scene.basemap == Basemap::Realistic;
        if !self.shown {
            // deck drops a layer that leaves the list, its tiles with it
            if !self.tiles.is_empty() {
                self.tiles.clear();
            }
            self.textures_loading = false;
            self.look = None;
            return;
        }
        cx.update_tiles(&mut self.tiles, |gpu, _tile, decoded| upload(&gpu.device, decoded));
        load_textures(cx);
        self.textures_loading = cx.textures.pending();
        self.look = Some(self.shader.prepare(cx));
        for (tile, _) in self.tiles.visible() {
            self.drawn.push((cx.tile_frame(tile, 0.0), tile));
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if !self.shown || cx.pass != Pass::Map {
            return;
        }
        for &(slot, tile) in &self.drawn {
            let Some(meshes) = self.tiles.get(tile) else { continue };
            if meshes.mesh.index_count() == 0 {
                continue;
            }
            // (R5: a near tile's foot shade, then its shadows, on the ground under its buildings)
            // far off, flat: only the roofs show from there
            self.shader.begin(pass, if meshes.extruded { &self.walls } else { &self.flat });
            cx.bind_frame(pass, slot);
            meshes.mesh.draw(pass);
            // (R6: a Streets GL tile's pitched roofs over its buildings)
        }
    }

    fn drape(&self) -> super::Drape {
        super::Drape::Offset
    }

    fn pending(&self) -> bool {
        self.shown && (self.tiles.loading() || self.textures_loading)
    }
}
