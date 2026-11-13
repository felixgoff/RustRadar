//! What is draped over the 3D terrain (item Z2; doc 04 §12, §14.3, §14.5; plan 1.4.4, 1.4.12): the
//! terrain tiles' cover textures, the passes that draw the draped layers into them, and the surface
//! drawn with them (`shaders/terrain.wgsl`).
//!
//! Today's app puts deck.gl's `TerrainExtension` on every layer but the earth, the surface and the
//! clouds once the *terrain* setting is on (`onTerrain`, G:3059-3067). The flat ones (no extrusion,
//! no instance positions) are draped: `TerrainEffect` keeps a `TerrainCover` for each terrain tile,
//! draws them into it from straight above (`TerrainPass.renderTerrainCover`: an orthographic Web
//! Mercator viewport over the tile's square, a texture cleared transparent, blending on, depth
//! `always`), skips them in the frame itself (`TERRAIN_MODE_SKIP`), and the tile's mesh samples its
//! cover (`TERRAIN_MODE_USE_COVER`). Here:
//!
//! - **Which layers** ([`Drape::Cover`](crate::layers::Drape::Cover)): the renderer leaves the
//!   cover layers out of the map pass while the terrain is drawn, and draws them in each cover pass
//!   ([`Pass::Cover`](crate::layers::Pass::Cover)) with the [`Globals`] of [`cover_globals`]: the
//!   frames the layers pushed for the map's camera are measured from the view centre, which these
//!   globals keep, so the same frames land in the tile's square. In a cover pass `globals.mode` is
//!   the flat map's, `globals.mercator.y` is 1 (`rl_clear` then gives 1: the draped ground takes no
//!   air, as `rlight.cover` does in deck's pass), and `globals.camera_render` is the map's own
//!   camera (`aimMapEye`, `rlight.eye`), so what the realistic ground lights by the view (water's
//!   sheen) is lit from where the map is seen. Pixels are the cover's texels (deck draws the pass at
//!   `devicePixelRatio` 1), so lines given in pixels are that many texels wide.
//! - **The covers** ([`Covers`], `scene::terrain::cover`): one `Rgba8Unorm` texture a terrain tile
//!   drawn, its size by today's patched rule (1024 texels; the deepest tiles with the view's zoom,
//!   to 2048), kept within [`BUDGET`], drawn at most [`COVERS_PER_FRAME`] a frame, never-drawn
//!   first. A cover is drawn again when a cover layer's tiles drawn change within its square
//!   ([`crate::layers::drawn_changes`]), and every cover when what the cover layers draw changes
//!   everywhere ([`CoverLook`]: the basemap, the projection, the weather and *daylight*, the
//!   ground's and the relief's settings and the relief's zoom-stepped opacity, the dark map's land,
//!   roads and runways, the night's minute, the realistic textures arriving). Animated effects
//!   (water, the clouds' shadows) stay as they were when the cover was drawn, as today.
//! - **The passes** ([`CoverJob`]): encoded and submitted in the renderer's `prepare` (so a shot,
//!   which only prepares while it waits, has its covers), each into a [`CHUNK`]-sized scratch with
//!   the map pass's sample count and depth and stencil (the layers' pipelines are the map pass's),
//!   resolved and copied into the cover; a 2048 cover is four chunks. The stencil is cleared to 0,
//!   so the ground's tile clips and the shadows' rule work as in the map pass.
//! - **The surface** ([`TerrainPipeline`]): `terrain.wgsl`: the cover over `MESH_COLOR` and lit as
//!   the bare mesh is (Z1), with the realistic map's air over it (`TERRAIN_AIR`, `fs_air`).
//!
//! Deviations, each with its reason:
//! - Budget and pace: deck keeps a cover for every cached terrain tile and draws all that need it
//!   at once; here at most two a frame (a tile shows the bare mesh colour, or its last picture, a
//!   few frames longer) and within the memory budget, the smallest on the screen halved first.
//! - The cover passes draw with the map pass's 4× MSAA (resolved), where deck's framebuffer has
//!   none: the layers' pipelines are built for the map pass. Edges in the cover are smoother.
//! - deck's cover framebuffer has no depth or stencil attachment, so its shadows darken twice where
//!   they overlap; here the stencil keeps the shadows' rule (plan 1.4.12). Depth is constant in a
//!   cover pass, so depth-tested and depth-writing draws all land, in order, as with `always`.
//! - Layers that drape part of what they draw (the rail map's lines, the signal blocks' lines, the
//!   train route, the radar's ground, the buildings' shadows and foot shade) are not cover layers
//!   yet: they draw as they are, at sea level under the surface. The selected flight's trail and
//!   route keep their true altitude (plan 1.4.12). The platforms (an extruded area, deck: offset)
//!   are drawn flat into the cover with the rest of the ground rails.
//! - What changes a cover layer's picture without changing its tiles or its settings (a tile's
//!   content decoded again with the same index set is caught; a style a layer works out from the
//!   zoom other than the relief's opacity is not) shows at the cover's next drawing.

use std::collections::HashMap;

use bytemuck::Zeroable;
use glam::{DMat4, DVec3};
use rustradar_core::geo::Basemap;
use rustradar_scene::camera::{WORLD_SIZE, lng_lat_to_world, mercator_units_per_meter, world_to_lng_lat};
use rustradar_scene::hillshade::HillshadeLayer;
pub use rustradar_scene::terrain::cover::{
    BUDGET, COVERS_PER_FRAME, Cover, CoverCache, CoverState, MAX_COVER_SIZE, Want, cover_bytes, cover_size, cover_zoom,
};
use rustradar_scene::tess::vector::true_aeroways;

use super::mesh::{SurfaceGpu, VERTEX_LAYOUT, style_uniform};
use crate::frame::{FrameSlot, Globals, MODE_FLAT, ViewFrame, tile_extent};
use crate::gpu::{DEPTH_FORMAT, Gpu};
use crate::layers::{DrawCx, Init, PrepareCx, TileIndex, TileSet, ground, hillshade, night, sky};
use crate::look::{RLight, frame_look, light_origin};
use crate::pipelines::mesh::LightsUniform;
use crate::pipelines::{Desc, build};
use crate::renderer::SceneInput;
use crate::shaders;
use crate::state::DrawState;
use crate::target::MAP_FORMAT;

/// `shaders/terrain.wgsl`.
pub const SHADER: &str = include_str!("../../shaders/terrain.wgsl");

/// The bind group of the light (binding 0), the rig (1) and the style (2).
pub const STYLE_GROUP: u32 = 2;
/// The bind group of a tile's cover (texture 0, sampler 1).
pub const COVER_GROUP: u32 = 3;
/// The side of the scratch a cover pass draws into, texels: a cover up to this size is one pass,
/// a larger one (2048) is cut into passes of this size.
pub const CHUNK: u32 = 1024;
/// `globals.mercator.y` in a cover pass (`lib/rlight.wgsl`'s `rl_clear` reads it).
pub const COVER_PASS: f32 = 1.0;
/// The deepest zoom of the surface's tiles (`maxZoom: SURFACE_LOD.levels.length − 1`).
pub const MAX_ZOOM: u32 = 16;

/// What the cover layers draw everywhere: when it changes, every cover is drawn again (deck's
/// `layerNeedsRedraw` of a draped layer that is not tiled, or whose props change).
#[derive(Debug, Clone, PartialEq)]
pub struct CoverLook {
    basemap: Basemap,
    globe: bool,
    sky: sky::Input,
    ground: ground::Input,
    /// The relief as the layer draws it, opacity in its zoom's steps (`hillshade::layer_for`).
    hillshade: Option<HillshadeLayer>,
    /// The dark map's land.
    land: bool,
    /// The dark map's roads and water, and whether its runways are at their true width.
    vector_map: Option<bool>,
    /// The night's minute on the dark and satellite maps (its bands are made each minute).
    night: Option<i64>,
    /// The realistic textures are still loading (the ground draws with stand-ins until then).
    textures_pending: bool,
}

/// [`CoverLook`] of `scene` seen in `view`, the realistic textures still loading or not.
pub fn cover_look(scene: &SceneInput, view: &ViewFrame, textures_pending: bool) -> CoverLook {
    let globe = view.is_globe();
    let dark = scene.basemap == Basemap::Dark;
    CoverLook {
        basemap: scene.basemap,
        globe,
        sky: scene.sky,
        ground: scene.ground.clone(),
        hillshade: scene.hillshade.shown.then(|| hillshade::layer_for(scene.basemap, scene.hillshade.terrain, !globe, view.view.zoom)).flatten(),
        land: dark && scene.earth.shown,
        vector_map: (dark && scene.vector_map.shown).then(|| true_aeroways(view.view.zoom, view.view.latitude)),
        night: night::drawn(scene).then(|| (scene.feed_now_ms / 60_000.0).floor() as i64),
        textures_pending,
    }
}

/// The map's camera in `view`: world Mercator `[x, y]` and metres above the sea (`aimMapEye`:
/// `viewport.unprojectPosition(viewport.cameraPosition)`).
pub fn map_eye(view: &ViewFrame) -> ([f64; 2], f64) {
    let [lng, lat, h] = view.viewport.unproject_position(view.viewport.camera_position);
    (lng_lat_to_world([lng, lat]), h)
}

/// A world x difference moved into `[−256, 256)`.
fn wrap_world_x(dx: f64) -> f64 {
    dx - WORLD_SIZE * ((dx + WORLD_SIZE / 2.0) / WORLD_SIZE).floor()
}

fn mat(m: DMat4) -> [[f32; 4]; 4] {
    m.as_mat4().to_cols_array_2d()
}

/// The globals of a cover pass over `tile`'s square, its cover `size` texels a side drawn in `n × n`
/// chunks, this one `i` east and `j` south of the north-west one, for the frames of `view` (whose
/// view centre local offsets are measured from, as theirs are).
///
/// An orthographic camera straight down over the chunk's square, north up (row 0 north, as the
/// tile's `v` runs south); every depth 0.5, so depth tests and writes keep the drawing order. The
/// flat map's terms at the tile's middle: its latitude, common units per metre and the Mercator y
/// from the view centre to it (heights scale from there, `flat_rel`); pixels are texels; the camera
/// is the map's ([`map_eye`]); `mercator.y` is [`COVER_PASS`]. `view_proj` is the same camera:
/// common offsets (the map's dynamic instances) are not drawn into covers.
pub fn cover_globals(view: &ViewFrame, tile: TileIndex, size: u32, n: u32, i: u32, j: u32) -> Globals {
    let reference = light_origin(view);
    let frame = view.tile(tile, 0.0);
    // the tile's north-west corner from the view centre, as its frame has it
    let ax = f64::from(frame.offset_hi[0]) + f64::from(frame.offset_lo[0]);
    let ay = f64::from(frame.offset_hi[1]) + f64::from(frame.offset_lo[1]);
    let extent = tile_extent(tile.z);
    let chunk = extent / f64::from(n);
    let west = ax + chunk * f64::from(i);
    let south = ay - chunk * f64::from(j + 1);
    let s = 2.0 / chunk;
    let ortho = DMat4::from_cols_array(&[s, 0.0, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -s * west - 1.0, -s * south - 1.0, 0.5, 1.0]);
    let middle_y = ay - extent / 2.0;
    let latitude = world_to_lng_lat([reference.x + ax + extent / 2.0, reference.y + middle_y])[1];
    let units_per_metre = mercator_units_per_meter(latitude);
    let (sin, cos) = latitude.to_radians().sin_cos();
    let (eye_world, eye_height) = map_eye(view);
    let eye = DVec3::new(wrap_world_x(eye_world[0] - reference.x), eye_world[1] - reference.y, eye_height * units_per_metre);
    let texels = f64::from(size / n);
    let pixels_per_unit = f64::from(size) / extent;
    Globals {
        view_proj: mat(ortho),
        centre_cam: mat(ortho),
        centre_offset: [0.0; 3],
        mode: MODE_FLAT,
        centre: [cos as f32, sin as f32, units_per_metre as f32, 0.0],
        camera_common: eye.as_vec3().into(),
        pixels_per_unit: pixels_per_unit as f32,
        camera_render: eye.as_vec3().into(),
        metres_per_pixel: (1.0 / (pixels_per_unit * units_per_metre)) as f32,
        viewport: [texels as f32, texels as f32, 1.0, view.globals.viewport[3]],
        mercator: [(-middle_y) as f32, COVER_PASS, 0.0, 0.0],
    }
}

/// How large `tile` is on the screen of `view` ([`Want::weight`]): its extent over its middle's
/// distance from the map's camera (at the middle of `z_range`, its heights).
pub fn screen_weight(view: &ViewFrame, tile: TileIndex, z_range: [f64; 2]) -> f64 {
    let extent = tile_extent(tile.z);
    let middle = [(f64::from(tile.x) + 0.5) * extent, WORLD_SIZE - (f64::from(tile.y) + 0.5) * extent];
    let (eye, height) = map_eye(view);
    let latitude = world_to_lng_lat(middle)[1];
    let ground = 0.5 * (z_range[0].max(0.0) + z_range[1].max(0.0));
    let d = DVec3::new(wrap_world_x(eye[0] - middle[0]), eye[1] - middle[1], (height - ground) * mercator_units_per_meter(latitude));
    extent / d.length().max(extent * 1e-6)
}

/// A cover on the GPU.
struct CoverTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    group: wgpu::BindGroup,
}

/// What a cover pass draws into: a [`CHUNK`]² colour of the map pass's sample count, its
/// depth/stencil, and (with MSAA) a single-sample one to resolve into before copying.
struct Scratch {
    colour: wgpu::Texture,
    colour_view: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolve: Option<(wgpu::Texture, wgpu::TextureView)>,
}

impl Scratch {
    fn new(device: &wgpu::Device, samples: u32) -> Self {
        let texture = |label, format, sample_count, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: CHUNK, height: CHUNK, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let colour = texture("terrain cover scratch", MAP_FORMAT, samples, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let depth = texture("terrain cover depth", DEPTH_FORMAT, samples, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let resolve = (samples > 1).then(|| {
            let t = texture("terrain cover resolve", MAP_FORMAT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
            let v = t.create_view(&Default::default());
            (t, v)
        });
        Scratch { colour_view: colour.create_view(&Default::default()), colour, depth: depth.create_view(&Default::default()), resolve }
    }

    fn bytes(samples: u32) -> usize {
        let texels = CHUNK as usize * CHUNK as usize;
        // colour and depth/stencil at the sample count, 4 bytes each, and the resolve
        texels * 4 * 2 * samples as usize + if samples > 1 { texels * 4 } else { 0 }
    }
}

/// Where one chunk's pass ends up.
enum Target {
    /// Resolved (or drawn) straight into the cover: the chunk is the whole cover and [`CHUNK`]
    /// wide.
    Cover(wgpu::TextureView),
    /// Copied from the scratch into the cover at this texel.
    Copy { texture: wgpu::Texture, origin: [u32; 2] },
}

/// One chunk of a cover pass.
struct Chunk {
    globals: wgpu::BindGroup,
    texels: u32,
    into: Target,
}

/// One cover to draw this frame (its chunks), made by [`Covers::jobs`].
pub struct CoverJob {
    pub tile: TileIndex,
    chunks: Vec<Chunk>,
    colour: wgpu::Texture,
    colour_view: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolve: Option<(wgpu::Texture, wgpu::TextureView)>,
}

impl CoverJob {
    /// Records the cover's passes: each chunk cleared transparent (depth 1, stencil 0) and drawn by
    /// `draw` with the chunk's globals (bind them as group 0; the stencil reference and the zero
    /// frame are the caller's, as in the map pass), resolved or copied into the cover.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, draw: &mut dyn FnMut(&mut wgpu::RenderPass<'_>, &wgpu::BindGroup)) {
        for chunk in &self.chunks {
            let msaa = self.resolve.is_some();
            let (view, resolve_target) = match (&chunk.into, &self.resolve) {
                (Target::Cover(cover), Some(_)) => (&self.colour_view, Some(cover)),
                (Target::Cover(cover), None) => (cover, None),
                (Target::Copy { .. }, Some((_, resolve))) => (&self.colour_view, Some(resolve)),
                (Target::Copy { .. }, None) => (&self.colour_view, None),
            };
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("terrain cover"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: if msaa { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth,
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                        stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
                    }),
                    ..Default::default()
                });
                let texels = chunk.texels as f32;
                pass.set_viewport(0.0, 0.0, texels, texels, 0.0, 1.0);
                draw(&mut pass, &chunk.globals);
            }
            if let Target::Copy { texture, origin } = &chunk.into {
                let source = self.resolve.as_ref().map_or(&self.colour, |(t, _)| t);
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo { texture: source, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: origin[0], y: origin[1], z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d { width: chunk.texels, height: chunk.texels, depth_or_array_layers: 1 },
                );
            }
        }
    }
}

/// The terrain tiles' covers: their textures, their bookkeeping, and what their passes need.
pub struct Covers {
    cache: CoverCache,
    textures: HashMap<TileIndex, CoverTexture>,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// A transparent texel, for a tile with no cover (yet).
    none: wgpu::BindGroup,
    samples: u32,
    globals_layout: wgpu::BindGroupLayout,
    /// The cover passes' globals, reused frame to frame.
    globals: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    scratch: Option<Scratch>,
    look: Option<CoverLook>,
    /// Covers drawn since made (for tests and logs).
    drawn: usize,
}

impl Covers {
    pub fn new(init: &Init<'_>) -> Self {
        Self::with_budget(init, BUDGET)
    }

    /// Covers within `budget` bytes (tests).
    pub fn with_budget(init: &Init<'_>, budget: usize) -> Self {
        let device = &init.gpu.device;
        let layout = cover_layout(device);
        // deck's cover texture: linear, no mipmaps; clamped at the tile's edges (its overlap and skirt)
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain cover"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blank = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terrain no cover"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MAP_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        init.gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &blank, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &[0, 0, 0, 0],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: None },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        let none = cover_group(device, &layout, &blank.create_view(&Default::default()), &sampler);
        Covers {
            cache: CoverCache::new(budget),
            textures: HashMap::new(),
            layout,
            sampler,
            none,
            samples: init.samples,
            globals_layout: init.bindings.globals_layout.clone(),
            globals: Vec::new(),
            scratch: None,
            look: None,
            drawn: 0,
        }
    }

    /// The bookkeeping (sizes, states, the budget).
    pub fn cache(&self) -> &CoverCache {
        &self.cache
    }

    /// The bytes the covers' textures hold (within the budget).
    pub fn bytes(&self) -> usize {
        self.cache.bytes()
    }

    /// The bytes the cover passes' scratch holds while the terrain is drawn (outside the budget, as
    /// the map target's attachments are).
    pub fn scratch_bytes(&self) -> usize {
        if self.scratch.is_some() { Scratch::bytes(self.samples) } else { 0 }
    }

    /// Covers drawn since made.
    pub fn drawn(&self) -> usize {
        self.drawn
    }

    /// Drawn tiles whose covers are not yet as they should be: the headless path waits for them.
    pub fn waiting(&self) -> usize {
        self.cache.waiting()
    }

    /// Lets every cover go (the terrain turned off), and the passes' scratch.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.textures.clear();
        self.scratch = None;
        self.globals.clear();
        self.look = None;
    }

    /// This frame's terrain `tiles` drawn (with their content): a cover for each, within the budget
    /// (the tile set says which of the others it still keeps).
    pub fn prepare(&mut self, cx: &PrepareCx<'_>, tiles: &TileSet<super::mesh::SurfaceMesh, SurfaceGpu>) {
        let zoom = cx.view.view.zoom;
        let wants: Vec<Want> = tiles
            .visible()
            .map(|(tile, gpu)| Want {
                tile,
                size: cover_size(tile.z, cover_zoom(tile.z, MAX_ZOOM, zoom)),
                weight: screen_weight(cx.view, tile, gpu.z_range()),
            })
            .collect();
        let plan = self.cache.plan(&wants, |tile| tiles.state(tile).is_some());
        let device = &cx.gpu.device;
        for tile in plan.free {
            self.textures.remove(&tile);
        }
        for (tile, size) in plan.allocate {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("terrain cover"),
                size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: MAP_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let group = cover_group(device, &self.layout, &view, &self.sampler);
            self.textures.insert(tile, CoverTexture { texture, view, group });
        }
    }

    /// What the cover layers draw everywhere this frame: a change puts every cover out of date.
    pub fn set_look(&mut self, look: CoverLook) {
        if self.look.as_ref().is_some_and(|l| *l != look) {
            self.cache.invalidate_all();
        }
        self.look = Some(look);
    }

    /// The tiles of cover layers drawn differently this frame: the covers they meet are out of date.
    pub fn invalidate(&mut self, changed: &[TileIndex]) {
        self.cache.invalidate(changed);
    }

    /// The covers to draw now (at most `max`), with their passes' globals for `view` written.
    pub fn jobs(&mut self, gpu: &Gpu, view: &ViewFrame, max: usize) -> Vec<CoverJob> {
        let tiles = self.cache.next(max);
        if tiles.is_empty() {
            return Vec::new();
        }
        let scratch = self.scratch.get_or_insert_with(|| Scratch::new(&gpu.device, self.samples));
        let mut used = 0;
        let mut jobs = Vec::new();
        for tile in tiles {
            let (Some(cover), Some(entry)) = (self.textures.get(&tile), self.cache.get(tile)) else { continue };
            let size = entry.size;
            let n = size.div_ceil(CHUNK).max(1);
            let texels = size / n;
            let mut chunks = Vec::new();
            for j in 0..n {
                for i in 0..n {
                    if used == self.globals.len() {
                        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("terrain cover globals"),
                            size: size_of::<Globals>() as u64,
                            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        });
                        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("terrain cover globals"),
                            layout: &self.globals_layout,
                            entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
                        });
                        self.globals.push((buffer, group));
                    }
                    let (buffer, group) = &self.globals[used];
                    used += 1;
                    gpu.queue.write_buffer(buffer, 0, bytemuck::bytes_of(&cover_globals(view, tile, size, n, i, j)));
                    let into = if n == 1 && texels == CHUNK {
                        Target::Cover(cover.view.clone())
                    } else {
                        Target::Copy { texture: cover.texture.clone(), origin: [i * texels, j * texels] }
                    };
                    chunks.push(Chunk { globals: group.clone(), texels, into });
                }
            }
            self.drawn += 1;
            jobs.push(CoverJob {
                tile,
                chunks,
                colour: scratch.colour.clone(),
                colour_view: scratch.colour_view.clone(),
                depth: scratch.depth.clone(),
                resolve: scratch.resolve.clone(),
            });
        }
        jobs
    }

    /// The bind group of `tile`'s cover, or of no cover.
    pub fn group(&self, tile: TileIndex) -> &wgpu::BindGroup {
        match (self.textures.get(&tile), self.cache.get(tile)) {
            (Some(texture), Some(cover)) if cover.state != CoverState::Fresh => &texture.group,
            _ => &self.none,
        }
    }

    /// `tile`'s cover texture, if it has one (tests).
    pub fn texture(&self, tile: TileIndex) -> Option<&wgpu::Texture> {
        self.textures.get(&tile).map(|t| &t.texture)
    }
}

fn cover_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("terrain cover"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

fn cover_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, view: &wgpu::TextureView, sampler: &wgpu::Sampler) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("terrain cover"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}

/// `shaders/terrain.wgsl`'s two pipelines (`fs`, and `fs_air` for the realistic map) with the
/// light, the rig and the style.
pub struct TerrainPipeline {
    plain: wgpu::RenderPipeline,
    air: wgpu::RenderPipeline,
    rlight: wgpu::Buffer,
    lights: wgpu::Buffer,
    group: wgpu::BindGroup,
}

impl TerrainPipeline {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let uniform = |binding, size: usize| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: std::num::NonZeroU64::new(size as u64),
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain light, rig and style"),
            entries: &[
                uniform(0, size_of::<RLight>()),
                uniform(1, size_of::<LightsUniform>()),
                uniform(2, size_of::<crate::pipelines::mesh::StyleUniform>()),
            ],
        });
        let covers = cover_layout(device);
        let module = shaders::module(device, "terrain", SHADER);
        let pipeline = |label, fragment_entry| {
            build(
                init,
                &Desc {
                    label,
                    module: &module,
                    vertex_entry: "vs",
                    fragment_entry,
                    buffers: &[Some(VERTEX_LAYOUT)],
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    state: DrawState::NO_CULL,
                    groups: &[&layout, &covers],
                },
            )
        };
        let buffer = |label, contents: &[u8]| {
            wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST },
            )
        };
        let rlight = buffer("terrain light", bytemuck::bytes_of(&RLight::zeroed()));
        let lights = buffer("terrain rig", bytemuck::bytes_of(&LightsUniform::zeroed()));
        let style = buffer("terrain style", bytemuck::bytes_of(&style_uniform()));
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain light, rig and style"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: rlight.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: lights.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: style.as_entire_binding() },
            ],
        });
        TerrainPipeline { plain: pipeline("terrain", "fs"), air: pipeline("terrain air", "fs_air"), rlight, lights, group }
    }

    /// This frame's rig (turned for the camera) and light (the realistic map's look for this frame,
    /// `TerrainAirExtension.draw`'s `rlightProps`).
    pub fn prepare(&self, cx: &PrepareCx<'_>) {
        let queue = &cx.gpu.queue;
        queue.write_buffer(&self.lights, 0, bytemuck::bytes_of(&LightsUniform::new(cx.lights(), cx.view)));
        let look = frame_look(&cx.scene.sky, &cx.view.view, cx.view.size, cx.local_now_ms);
        queue.write_buffer(&self.rlight, 0, bytemuck::bytes_of(&RLight::new(&look, cx.view, f64::from(cx.view.globals.viewport[3]))));
    }

    /// Draws each tile in its frame with its cover; with the air over it on the realistic map.
    pub fn draw<'t>(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, covers: &Covers, tiles: impl IntoIterator<Item = (FrameSlot, TileIndex, &'t SurfaceGpu)>) {
        pass.set_pipeline(if cx.scene.basemap == Basemap::Realistic { &self.air } else { &self.plain });
        pass.set_bind_group(STYLE_GROUP, &self.group, &[]);
        for (slot, tile, gpu) in tiles {
            cx.bind_frame(pass, slot);
            pass.set_bind_group(COVER_GROUP, covers.group(tile), &[]);
            gpu.draw(pass);
        }
    }
}
