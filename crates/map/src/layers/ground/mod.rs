//! The realistic map's ground (item R2; doc 04 §5–6; `realistic-ground.ts`): the ground shader
//! (`shaders/ground.wgsl`) with its surface table and textures, and the layers drawn with it.
//!
//! - [`land`] (row 3, `ground-land`, R1): the land under everything, one rectangle in the paint
//!   `LAND` (unmapped land in Streets GL's temperate green).
//! - [`Ground`] (rows 3c–3f): on the globe [`vector::GroundGlobe`] (`ground-globe`: OpenMapTiles
//!   z0–14 by deck's own selection, R2); on the flat map [`city::GroundCity`] (`ground-city`:
//!   OpenMapTiles z9–14 and Streets GL z16 by distance, R3), and the land cover (R8) and ground
//!   rails and platforms (R9) on both.
//!
//! **The shader** (doc 04 §6.8, §16): [`GroundShader`] holds what every ground draw shares: the
//! light (`lib/rlight.wgsl`, group 2, [`look::LightUniform`]), and group 3 ([`GROUND_GROUP`]): the
//! surface table ([`GroundUniform`], made from `scene::ground`'s `SURFACES`, which Rust owns), the
//! lines' minimum width ([`LineWidth`]: 0.6 px far out, none for Streets GL's), the ground's two
//! texture arrays, the water's ripples, the Earth's picture and its lights at night, and their
//! samplers (doc 04 §5.1: repeat both ways with anisotropy 8; the Earth round the world and not
//! over the poles). Its pipelines ([`GroundShader::areas`], [`GroundShader::lines`]) take a
//! [`DrawState`]: `SURFACE` for the land, `SURFACE_NO_CULL` for the tiles. Paints are explicit:
//! an area's [`GroundVertex`] and a line's [`PathSegment`] carry the tint and the surface id
//! (`scene::tess::ground::explicit`).
//!
//! **Textures** (doc 04 §5.3, plan 1.4.13): the realistic map's thirteen KTX2 files are embedded
//! ([`texture_bytes`]) and transcoded on the worker pool into the renderer's shared
//! [`crate::textures::RealisticTextures`] ([`PrepareCx::textures`]), started by the first
//! realistic layer that draws with them ([`load_textures`]: the ground, when the scene shows it,
//! today's `realisticTextures(device)` on a realistic layer's first draw) and swapped in at once
//! when the last file is in; until then the 1×1 stand-ins (white, flat, the Earth of alpha 0:
//! "not loaded") are bound, as today's `realisticTextures` does. A layer's bind group is made again
//! when they swap ([`GroundShader::prepare`]), and a layer that draws with them is pending until
//! they are in (the headless shots wait for them, as today's reference pictures did).
//!
//! **When** (G:2650-2663): on the realistic basemap when the scene builder shows the ground
//! ([`Input::shown`], `app::scene_builder::realistic`), the globe's tiles (`ground-globe`) while
//! the view is the globe (zoom ≤ 12), the city ground's (`ground-city`) on the flat map.
//! `ground-flat` is never drawn today (the flat map takes the city ground).

pub mod city;
pub mod earth_picture;
pub mod land;
pub mod landcover;
pub mod vector;

use std::borrow::Cow;

use bytemuck::{Pod, Zeroable};
use rustradar_core::geo::Basemap;
use rustradar_scene::ground::{LINE_MIN_PIXELS, SURFACE_COUNT, surface_table};
pub use rustradar_scene::tess::ground::GroundVertex;

use super::{DrawCx, Init, Layer, Pass, PrepareCx};
use crate::frame::FrameSlot;
use crate::look::{LightUniform, Look};
use crate::pipelines::path::{MAX_SAFE_INTEGER, Segment as PathSegment};
use crate::pipelines::{Desc, build};
use crate::shaders;
use crate::state::DrawState;
use crate::textures::{Bytes, LoadOptions, Sampling};

/// `shaders/ground.wgsl`.
pub const SHADER: &str = include_str!("../../shaders/ground.wgsl");

/// The bind group of the ground's table and textures (after the light's, group 2).
pub const GROUND_GROUP: u32 = 3;

/// What the scene builder hands the ground (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Input {
    /// Whether the realistic map's ground is drawn (its tiles; the textures load once it is):
    /// on the realistic basemap, where the app's realistic scene builder sets it. Off in a scene
    /// made with `SceneInput::new`.
    pub shown: bool,
}

impl Input {
    /// The ground drawn.
    pub const SHOWN: Input = Input { shown: true };
}

// ------------------------------------------------------------------ the surface table

/// The table's rows: two per surface id, `(GS_LAYER, GS_METRES, GS_ROUGH, GS_SNOW)` and
/// `(GS_PAVED, GS_FLAT_R, GS_FLAT_G, GS_FLAT_B)`, unused ids taking id 0's (`scene::ground`'s
/// `surface_table`; a flat colour of none is −1, water's layer −1).
pub fn surface_rows() -> [[f32; 4]; 2 * SURFACE_COUNT] {
    let layer = surface_table(|s| s.layer());
    let metres = surface_table(|s| s.metres);
    let rough = surface_table(|s| s.rough);
    let snow = surface_table(|s| s.snow);
    let paved = surface_table(|s| if s.paved { 1.0 } else { 0.0 });
    let flat = |c: usize| surface_table(move |s| s.flat.map_or(-1.0, |f| f[c]));
    let (r, g, b) = (flat(0), flat(1), flat(2));
    std::array::from_fn(|k| {
        let i = k / 2;
        let row = if k % 2 == 0 { [layer[i], metres[i], rough[i], snow[i]] } else { [paved[i], r[i], g[i], b[i]] };
        row.map(|v| v as f32)
    })
}

/// How narrow the ground's lines may get on the screen (deck's `lineWidthMinPixels`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineWidth {
    /// Far out the main roads and rivers stay a hairline of 0.6 px (OpenMapTiles' ground, the
    /// ground rails).
    Hairline,
    /// As wide as they are (Streets GL's ground close in, the platforms' lines: deck's default 0).
    Exact,
}

impl LineWidth {
    /// The minimum, CSS pixels.
    pub fn min_pixels(self) -> f64 {
        match self {
            LineWidth::Hairline => LINE_MIN_PIXELS,
            LineWidth::Exact => 0.0,
        }
    }
}

/// `ground.wgsl`'s `Ground`: the surface table and the lines' width limits (1360 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GroundUniform {
    pub surfaces: [[f32; 4]; 2 * SURFACE_COUNT],
    /// `widthMinPixels`, `widthMaxPixels` (deck's default, `Number.MAX_SAFE_INTEGER`), unused.
    pub line: [f32; 4],
}

impl GroundUniform {
    pub fn new(width: LineWidth) -> Self {
        GroundUniform { surfaces: surface_rows(), line: [width.min_pixels() as f32, MAX_SAFE_INTEGER as f32, 0.0, 0.0] }
    }
}

// ------------------------------------------------------------------ vertex layouts

/// An area's vertex buffer: [`GroundVertex`], position at location 0, tint and surface
/// (`Uint8x4`) at location 1.
pub const AREA_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: size_of::<GroundVertex>() as u64,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint8x4],
};

/// A line's instance buffer: [`PathSegment`] as far as the ground reads it (its four points, its
/// width, its paint at byte 52 as `Uint8x4`, its cap bits at byte 80), locations 1–7.
pub const SEGMENT_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: size_of::<PathSegment>() as u64,
    step_mode: wgpu::VertexStepMode::Instance,
    attributes: &[
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 1 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 2 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 24, shader_location: 3 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 36, shader_location: 4 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 48, shader_location: 5 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint8x4, offset: 52, shader_location: 6 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 80, shader_location: 7 },
    ],
};

/// The corners of a segment (deck's `SEGMENT_POSITIONS`, as `pipelines::path`'s).
const CORNERS: [[f32; 2]; 6] = [[0.0, 0.0], [0.0, -1.0], [0.0, 1.0], [1.0, -1.0], [1.0, 1.0], [1.0, 0.0]];
/// deck's `SEGMENT_INDICES`.
const CORNER_INDICES: [u16; 12] = [0, 1, 2, 1, 4, 2, 1, 3, 4, 3, 5, 4];
const CORNER_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 8,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![0 => Float32x2],
};

/// A tile's lines on the GPU: an instance buffer of segments.
pub struct LineMesh {
    buffer: Option<wgpu::Buffer>,
    count: u32,
}

impl LineMesh {
    /// Uploads `segments` (none makes a mesh that draws nothing).
    pub fn new(device: &wgpu::Device, label: &str, segments: &[PathSegment]) -> Self {
        let buffer = (!segments.is_empty()).then(|| {
            wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytemuck::cast_slice(segments), usage: wgpu::BufferUsages::VERTEX },
            )
        });
        LineMesh { buffer, count: segments.len() as u32 }
    }

    pub fn len(&self) -> u32 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

// ------------------------------------------------------------------ textures

/// The realistic map's files, embedded (plan 1.6): the bytes of `path` (a `textures::TextureFile`
/// path under `static/`), `None` for any other.
pub fn texture_bytes(path: &str) -> Option<Bytes> {
    let bytes: &'static [u8] = match path {
        "realistic/building.color.ktx2" => include_bytes!("../../../../../static/realistic/building.color.ktx2"),
        "realistic/building.normal.ktx2" => include_bytes!("../../../../../static/realistic/building.normal.ktx2"),
        "realistic/building.mask.ktx2" => include_bytes!("../../../../../static/realistic/building.mask.ktx2"),
        "realistic/building.glow.ktx2" => include_bytes!("../../../../../static/realistic/building.glow.ktx2"),
        "realistic/ground.color.ktx2" => include_bytes!("../../../../../static/realistic/ground.color.ktx2"),
        "realistic/ground.normal.ktx2" => include_bytes!("../../../../../static/realistic/ground.normal.ktx2"),
        "realistic/tree.color.ktx2" => include_bytes!("../../../../../static/realistic/tree.color.ktx2"),
        "realistic/tree.normal.ktx2" => include_bytes!("../../../../../static/realistic/tree.normal.ktx2"),
        "realistic/tree.mask.ktx2" => include_bytes!("../../../../../static/realistic/tree.mask.ktx2"),
        "realistic/water.normal.ktx2" => include_bytes!("../../../../../static/realistic/water.normal.ktx2"),
        "realistic/earth.color.ktx2" => include_bytes!("../../../../../static/realistic/earth.color.ktx2"),
        "realistic/tree-volume.normal.ktx2" => include_bytes!("../../../../../static/realistic/tree-volume.normal.ktx2"),
        "realistic/earth.night.ktx2" => include_bytes!("../../../../../static/realistic/earth.night.ktx2"),
        _ => return None,
    };
    Some(Cow::Borrowed(bytes))
}

/// Starts the realistic textures loading on the worker pool (the first time; for this device's
/// formats, from [`texture_bytes`]) and takes in what has arrived: every realistic layer that
/// draws with them calls it in `prepare`.
pub fn load_textures(cx: &mut PrepareCx<'_>) {
    let options = LoadOptions::for_device(&cx.gpu.device);
    cx.textures.start(&cx.tiles.pool, options, texture_bytes);
    cx.textures.poll(&cx.gpu.device, &cx.gpu.queue);
}

// ------------------------------------------------------------------ the shader

/// What every draw of the ground shares: the module, the light (group 2), the table and the
/// textures (group 3, one bind group per [`LineWidth`]), the lines' corners. A layer keeps one,
/// makes its pipelines with it, and calls [`Self::prepare`] each frame it draws.
pub struct GroundShader {
    module: wgpu::ShaderModule,
    light: LightUniform,
    layout: wgpu::BindGroupLayout,
    /// The table with each [`LineWidth`]: hairline, exact.
    uniforms: [wgpu::Buffer; 2],
    repeat: wgpu::Sampler,
    globe: wgpu::Sampler,
    /// Group 3 for each [`LineWidth`], made from the textures as they were (`bound`).
    groups: Option<[wgpu::BindGroup; 2]>,
    bound: Option<bool>,
    corners: wgpu::Buffer,
    corner_indices: wgpu::Buffer,
}

impl GroundShader {
    pub fn new(init: &Init<'_>, label: &str) -> Self {
        let device = &init.gpu.device;
        let module = shaders::module(device, label, SHADER);
        let light = LightUniform::new(device);
        let texture = |binding, dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: dimension,
                multisampled: false,
            },
            count: None,
        };
        let sampler = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ground"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(size_of::<GroundUniform>() as u64),
                    },
                    count: None,
                },
                texture(1, wgpu::TextureViewDimension::D2Array),
                texture(2, wgpu::TextureViewDimension::D2Array),
                texture(3, wgpu::TextureViewDimension::D2),
                texture(4, wgpu::TextureViewDimension::D2),
                texture(5, wgpu::TextureViewDimension::D2),
                sampler(6),
                sampler(7),
            ],
        });
        let uniform = |width| {
            wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some("ground table"),
                    contents: bytemuck::bytes_of(&GroundUniform::new(width)),
                    usage: wgpu::BufferUsages::UNIFORM,
                },
            )
        };
        let buffer = |contents: &[u8], usage| {
            wgpu::util::DeviceExt::create_buffer_init(device, &wgpu::util::BufferInitDescriptor { label: Some("ground line corners"), contents, usage })
        };
        GroundShader {
            module,
            light,
            layout,
            uniforms: [uniform(LineWidth::Hairline), uniform(LineWidth::Exact)],
            repeat: Sampling::Repeat.sampler(device),
            globe: Sampling::Globe.sampler(device),
            groups: None,
            bound: None,
            corners: buffer(bytemuck::cast_slice(&CORNERS), wgpu::BufferUsages::VERTEX),
            corner_indices: buffer(bytemuck::cast_slice(&CORNER_INDICES), wgpu::BufferUsages::INDEX),
        }
    }

    /// The areas' pipeline (`vs_area`, `fs_area`) with `state`.
    pub fn areas(&self, init: &Init<'_>, label: &str, state: DrawState) -> wgpu::RenderPipeline {
        self.pipeline(init, label, "vs_area", "fs_area", &[Some(AREA_LAYOUT)], state)
    }

    /// The lines' pipeline (`vs_line`, `fs_line`) with `state`.
    pub fn lines(&self, init: &Init<'_>, label: &str, state: DrawState) -> wgpu::RenderPipeline {
        self.pipeline(init, label, "vs_line", "fs_line", &[Some(CORNER_LAYOUT), Some(SEGMENT_LAYOUT)], state)
    }

    fn pipeline(
        &self,
        init: &Init<'_>,
        label: &str,
        vertex_entry: &str,
        fragment_entry: &str,
        buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
        state: DrawState,
    ) -> wgpu::RenderPipeline {
        build(
            init,
            &Desc {
                label,
                module: &self.module,
                vertex_entry,
                fragment_entry,
                buffers,
                topology: wgpu::PrimitiveTopology::TriangleList,
                state,
                groups: &[self.light.layout(), &self.layout],
            },
        )
    }

    /// This frame's light (written, and returned for what the layer works out on the CPU), and
    /// group 3 made again if the textures have swapped since.
    pub fn prepare(&mut self, cx: &PrepareCx<'_>) -> Look {
        let ready = cx.textures.ready;
        if self.bound != Some(ready) {
            let device = &cx.gpu.device;
            let t = &*cx.textures;
            let group = |uniform: &wgpu::Buffer| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("ground"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&t.ground.color.view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&t.ground.normal.view) },
                        wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&t.water.view) },
                        wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&t.earth.view) },
                        wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&t.night.view) },
                        wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&self.repeat) },
                        wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(&self.globe) },
                    ],
                })
            };
            self.groups = Some([group(&self.uniforms[0]), group(&self.uniforms[1])]);
            self.bound = Some(ready);
        }
        self.light.prepare(cx)
    }

    /// Sets `pipeline` and binds groups 2 and 3 (the lines' minimum `width`). After
    /// [`Self::prepare`].
    pub fn begin(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline, width: LineWidth) {
        pass.set_pipeline(pipeline);
        self.light.bind(pass);
        if let Some(groups) = &self.groups {
            let index = match width {
                LineWidth::Hairline => 0,
                LineWidth::Exact => 1,
            };
            pass.set_bind_group(GROUND_GROUP, &groups[index], &[]);
        }
    }

    /// Draws a tile's areas in its frame, after [`Self::begin`] with an areas' pipeline.
    pub fn draw_areas(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, frame: FrameSlot, mesh: &crate::pipelines::fill::Mesh<GroundVertex>) {
        if mesh.index_count() == 0 {
            return;
        }
        cx.bind_frame(pass, frame);
        mesh.draw(pass);
    }

    /// Draws a tile's lines in its frame, after [`Self::begin`] with a lines' pipeline.
    pub fn draw_lines(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, frame: FrameSlot, mesh: &LineMesh) {
        let Some(buffer) = &mesh.buffer else { return };
        cx.bind_frame(pass, frame);
        pass.set_vertex_buffer(0, self.corners.slice(..));
        pass.set_index_buffer(self.corner_indices.slice(..), wgpu::IndexFormat::Uint16);
        pass.set_vertex_buffer(1, buffer.slice(..));
        pass.draw_indexed(0..CORNER_INDICES.len() as u32, 0, 0..mesh.count);
    }
}

// ------------------------------------------------------------------ the layer

/// Rows 3c–3f (realistic basemap): the globe's ground tiles (R2), the city ground (R3); the land
/// cover, ground rails and platforms come with R8 and R9.
pub struct Ground {
    shader: GroundShader,
    areas: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    globe: vector::GroundGlobe,
    /// The flat map's tiles by distance (R3).
    city: city::GroundCity,
    /// Whether this frame draws, and whether the textures were still loading.
    shown: bool,
    textures_loading: bool,
    /// This frame's light (for tests and later layers' CPU work).
    look: Option<Look>,
}

impl Ground {
    pub fn new(init: &Init<'_>) -> Self {
        let shader = GroundShader::new(init, "ground");
        let areas = shader.areas(init, "ground areas", DrawState::SURFACE_NO_CULL);
        let lines = shader.lines(init, "ground lines", DrawState::SURFACE_NO_CULL);
        let city = city::GroundCity::new(init, &shader);
        Ground { shader, areas, lines, globe: vector::GroundGlobe::new(init), city, shown: false, textures_loading: false, look: None }
    }

    /// The globe's tiles (tests).
    pub fn globe(&self) -> &vector::GroundGlobe {
        &self.globe
    }

    /// The flat map's tiles (tests).
    pub fn city(&self) -> &city::GroundCity {
        &self.city
    }

    /// The light of the last frame drawn.
    pub fn look(&self) -> Option<&Look> {
        self.look.as_ref()
    }
}

impl Layer for Ground {
    fn name(&self) -> &'static str {
        "ground"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.shown = cx.scene.ground.shown && cx.scene.basemap == Basemap::Realistic;
        let globe = self.shown && cx.view.is_globe();
        // `ground-globe` leaves deck's list off the globe, its tiles with it
        self.globe.prepare(cx, globe);
        // `ground-city` on the flat map in its place (R3)
        self.city.prepare(cx, self.shown && !globe);
        if !self.shown {
            self.look = None;
            return;
        }
        load_textures(cx);
        self.textures_loading = cx.textures.pending();
        self.look = Some(self.shader.prepare(cx));
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if !self.shown || cx.pass != Pass::Map {
            return;
        }
        self.globe.draw(pass, cx, &self.shader, &self.areas, &self.lines);
        self.city.draw(pass, cx, &self.shader);
    }

    fn drape(&self) -> super::Drape {
        super::Drape::Cover
    }

    fn pending(&self) -> bool {
        self.shown && (self.textures_loading || self.globe.pending() || self.city.pending())
    }
}

/// The ground shader in f64 (`shaders/ground.wgsl`'s `ground_colour`) for a fragment of a surface
/// that is neither water nor a field, from what its textures gave: for tests and for colours the
/// CPU must know. The light is `look::twin`'s.
pub mod twin {
    use glam::{DVec3, DVec4};
    use rustradar_scene::ground::{ICE, surface};

    use crate::look::Look;
    use crate::textures::{Target, TextureFile, Transcoded, transcode};
    use crate::look::twin::{self as light, Vary};

    const LUM: DVec3 = DVec3::new(0.2126, 0.7152, 0.0722);

    fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
        let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// What a fragment's textures and place gave.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Fragment {
        /// The surface id (neither water nor a field).
        pub id: usize,
        /// The paint's tint, sRGB 0–1.
        pub tint: DVec3,
        /// The colour array's texel (linear) and the normal array's (0–1, as stored).
        pub texel: DVec3,
        pub normal_texel: DVec3,
        /// The unevenness: `1 + 0.28 · grain · (noise − 0.5)`.
        pub grain_factor: f64,
        /// The Earth's picture there (linear, alpha 0 while not loaded) and its lights (as sampled).
        pub earth: DVec4,
        pub night: DVec3,
        /// The layer's mean colour (linear).
        pub mean: DVec3,
        /// How many metres one pixel spans there (`rl_cells(rl_t) · periodMetres`).
        pub metres_per_pixel: f64,
        /// A line's (no town lights).
        pub line: bool,
    }

    impl Fragment {
        /// Surface `id` in `tint` with the stand-in textures (white, flat; no Earth, no lights)
        /// and no unevenness.
        pub fn stand_in(id: usize, tint: [u8; 3]) -> Fragment {
            Fragment {
                id,
                tint: DVec3::from_array(tint.map(f64::from)) / 255.0,
                texel: DVec3::ONE,
                normal_texel: DVec3::new(128.0, 128.0, 255.0) / 255.0,
                grain_factor: 1.0,
                earth: DVec4::ZERO,
                night: DVec3::ZERO,
                mean: DVec3::ONE,
                metres_per_pixel: 0.0,
                line: false,
            }
        }
    }

    /// The colour (0–255, unrounded) of `f` where the light's varyings are `rl`, under `look`
    /// (no clouds' shadows: `cloud_shadow` 0, as in every preset).
    pub fn colour(look: &Look, rl: &Vary, f: &Fragment) -> [f64; 3] {
        let s = surface(f.id).or(surface(0)).expect("surface 0");
        let tint = light::linear(f.tint);
        let v = rl.eye.normalize();
        let nm = f.normal_texel * 2.0 - 1.0;
        let mut n = DVec3::new(nm.x, nm.y, nm.z.max(0.3)).normalize();
        let mut albedo = f.texel * tint * f.grain_factor;
        if let Some(flat) = s.flat {
            albedo = DVec3::from_array(flat) * tint;
            n = DVec3::Z;
        }
        let earth = f.earth.truncate();
        let far = f.earth.w * smoothstep(120.0, 1200.0, f.metres_per_pixel);
        if f.id == 0 {
            let mean = f.mean * tint;
            let dry = f.earth.w * smoothstep(0.03, 0.15, (earth.x - earth.y) / (earth.x + earth.y + 1e-4));
            albedo = albedo.lerp(earth * albedo.dot(LUM) / mean.dot(LUM).max(1e-4), dry);
        }
        if f.id != usize::from(ICE) {
            albedo = albedo.lerp(earth, far);
        }
        let snow = if f.id == usize::from(ICE) { 1.0 } else { look.snow * s.snow * (1.0 - far) };
        albedo = albedo.lerp(DVec3::new(0.82, 0.84, 0.88) * (0.75 + 0.25 * s.snow), snow);
        let paved = if s.paved { 1.0 } else { 0.0 };
        let wet = look.wet * (0.35 + 0.65 * paved) * (1.0 - far);
        albedo *= 1.0 - 0.45 * wet;
        let rough = s.rough + (0.12 - s.rough) * (wet * paved * (1.0 - look.snow));
        n = n.lerp(DVec3::Z, wet * 0.6).normalize();
        let mut lit = light::shade(look, rl, albedo, n, v, rough, 0.0, 1.0, 1.0, 1.0);
        if !f.line && look.globe > 0.0 {
            let dark = 1.0 - smoothstep(-0.12, 0.03, rl.sun.z);
            lit += light::linear(f.night) * 0.55 * look.globe * dark;
        }
        light::display(look, light::aerial(look, rl, lit))
    }

    /// One of the realistic map's files decoded on the CPU (tests, and colours the CPU must know):
    /// every level in RGBA8, as the transcoder gives them for a device with no compression.
    pub struct Picture {
        pub transcoded: Transcoded,
        /// The texels are sRGB colours (decoded to linear light when sampled), else data.
        pub srgb: bool,
    }

    impl Picture {
        /// The embedded `file` (`super::texture_bytes`), every level in RGBA8.
        pub fn load(file: TextureFile) -> Option<Picture> {
            let bytes = super::texture_bytes(file.path)?;
            let transcoded = transcode(&bytes, Target::Rgba).ok()?;
            Some(Picture { transcoded, srgb: file.srgb })
        }

        /// Texel `(x, y)` of `layer` of `level`, linear (or data) and alpha, 0–1.
        pub fn texel(&self, level: usize, layer: u32, x: u32, y: u32) -> DVec4 {
            let l = &self.transcoded.levels[level];
            let i = (((layer * l.height + y) * l.width + x) * 4) as usize;
            let c = DVec4::from_array(std::array::from_fn(|k| f64::from(l.data[i + k]) / 255.0));
            if self.srgb { DVec4::new(srgb_to_linear(c.x), srgb_to_linear(c.y), srgb_to_linear(c.z), c.w) } else { c }
        }

        /// The first layer at `(u, v)` of level 0, filtered linearly in linear light, round the
        /// world east to west and clamped north and south (the Earth's sampler: what the GPU reads
        /// where a pixel spans less than a texel).
        pub fn sample(&self, u: f64, v: f64) -> DVec4 {
            let l = &self.transcoded.levels[0];
            let (w, h) = (f64::from(l.width), f64::from(l.height));
            let (x, y) = (u * w - 0.5, (v * h - 0.5).clamp(0.0, h - 1.0));
            let (x0, y0) = (x.floor(), y.floor());
            let (fx, fy) = (x - x0, y - y0);
            let wrap = |i: f64| i.rem_euclid(w) as u32;
            let row = |i: f64| (i.clamp(0.0, h - 1.0)) as u32;
            let at = |dx: f64, dy: f64| self.texel(0, 0, wrap(x0 + dx), row(y0 + dy));
            at(0.0, 0.0).lerp(at(1.0, 0.0), fx).lerp(at(0.0, 1.0).lerp(at(1.0, 1.0), fx), fy)
        }

        /// The last level's texel of `layer`: the layer's mean (`textureLod(…, 16.0)`).
        pub fn mean(&self, layer: u32) -> DVec4 {
            self.texel(self.transcoded.levels.len() - 1, layer, 0, 0)
        }
    }

    /// What the land's colour far out comes from (doc 04 §6.8's `far`: from 1200 m a pixel the
    /// Earth picture's own colours), decoded on the CPU: the Earth's picture, its lights at night,
    /// and the terrain texture's mean normal (what a pixel spanning many of its repeats reads).
    pub struct FarOut {
        pub earth: Picture,
        pub night: Picture,
        pub terrain_normal: DVec3,
    }

    impl FarOut {
        pub fn load() -> Option<FarOut> {
            let normal = Picture::load(crate::textures::GROUND_NORMAL)?;
            Some(FarOut {
                earth: Picture::load(crate::textures::EARTH_COLOR)?,
                night: Picture::load(crate::textures::EARTH_NIGHT)?,
                terrain_normal: normal.mean(22).truncate(),
            })
        }

        /// The land's colour (surface 0, unmapped) at `[lon, lat]`, where the light's varyings are
        /// `rl`, under `look`, with a pixel spanning far more than 1200 m: the lowest and the
        /// highest of each channel over the places within a texel of it (the GPU reads the
        /// pictures' coordinates interpolated across the globe's flat 5° facets, up to a third of a
        /// texel off the sphere's own place).
        pub fn land_range(&self, look: &Look, rl: &Vary, [lon, lat]: [f64; 2]) -> ([f64; 3], [f64; 3]) {
            let (mut low, mut high) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            for du in [-1.0, -0.5, 0.0, 0.5, 1.0] {
                for dv in [-1.0, -0.5, 0.0, 0.5, 1.0] {
                    let c = self.at(look, rl, [lon, lat], [du, dv]);
                    for i in 0..3 {
                        low[i] = low[i].min(c[i]);
                        high[i] = high[i].max(c[i]);
                    }
                }
            }
            (low, high)
        }

        /// The land's colour far out at `[lon, lat]` itself (see [`Self::land_range`]).
        pub fn land_colour(&self, look: &Look, rl: &Vary, lon_lat: [f64; 2]) -> [f64; 3] {
            self.at(look, rl, lon_lat, [0.0, 0.0])
        }

        /// ... `texels` (u, v) of the pictures away.
        fn at(&self, look: &Look, rl: &Vary, [lon, lat]: [f64; 2], [du, dv]: [f64; 2]) -> [f64; 3] {
            let (u, v) = ((lon + 180.0) / 360.0 + du / 4096.0, (90.0 - lat) / 180.0 + dv / 2048.0);
            let land = rustradar_scene::ground::LAND;
            let fragment = Fragment {
                normal_texel: self.terrain_normal,
                earth: self.earth.sample(u, v),
                night: self.night.sample(u, v).truncate(),
                metres_per_pixel: 1e6,
                ..Fragment::stand_in(0, [land[0], land[1], land[2]])
            };
            colour(look, rl, &fragment)
        }
    }

    /// The sRGB transfer function's inverse (what a GPU does to an sRGB texel).
    pub fn srgb_to_linear(c: f64) -> f64 {
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    }
}

#[cfg(test)]
mod tests {
    use glam::{DVec2, DVec4};
    use rustradar_core::weather::{Conditions, PRESETS};
    use rustradar_scene::camera::{ViewState, lng_lat_to_world};
    use rustradar_scene::ground::GROUND_TEXTURES;

    use super::twin::Fragment;
    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::{earth, sky};
    use crate::look::{Look, RLight, twin as light};
    use crate::pipelines::fill::tests::wgsl_members;
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 11:20 UTC: local noon at 10° E at midsummer (V2's centre).
    const V2_NOON: f64 = 1_782_040_800_000.0;
    /// 2026-06-21 10:00 UTC: noon in Stockholm (the reference pictures' clock).
    const MIDSUMMER: f64 = 1_782_036_000_000.0;
    /// 2026-12-21 22:00 UTC: night over Europe.
    const MIDWINTER_NIGHT: f64 = 1_797_890_400_000.0;
    const SIZE: [f64; 2] = [1100.0, 720.0];
    /// V2 (plan section 8): Europe.
    const V2: ViewState = ViewState { latitude: 52.0, longitude: 10.0, zoom: 4.5, pitch: 0.0, bearing: 0.0, position: None };

    fn save(image: &Image, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }

    /// The table holds `scene::ground`'s surfaces two rows an id (unused ids id 0's), and the
    /// uniform is laid out as the shader's `Ground`.
    #[test]
    fn the_table_is_scene_grounds_surfaces() {
        let rows = surface_rows();
        let layer = surface_table(|s| s.layer());
        for id in 0..SURFACE_COUNT {
            assert_eq!(f64::from(rows[2 * id][0]), layer[id], "id {id}");
        }
        // water: no texture; a road: its flat asphalt; the railway: textured; an unused id: id 0's
        assert_eq!(rows[2 * 11], [-1.0, 0.0, 0.05, 0.0]);
        assert_eq!(rows[2 * 32 + 1], [1.0, 0.075, 0.075, 0.08]);
        assert_eq!(rows[2 * 35], [15.0, 0.0, 0.8, 0.5]);
        assert_eq!(rows[2 * 35 + 1], [0.0, -1.0, -1.0, -1.0]);
        assert_eq!((rows[2 * 20], rows[2 * 20 + 1]), (rows[0], rows[1]));
        assert_eq!(GROUND_TEXTURES[22], "terrain");
        let (size, members) = wgsl_members(SHADER, "Ground");
        assert_eq!((size as usize, size_of::<GroundUniform>()), (1360, 1360));
        assert_eq!(members, vec![("surfaces".to_string(), 0), ("line".to_string(), 1344)]);
        assert_eq!(GroundUniform::new(LineWidth::Hairline).line[0], 0.6);
        assert_eq!(GroundUniform::new(LineWidth::Exact).line[0], 0.0);
        // every file of the realistic set is embedded
        for file in crate::textures::REALISTIC_FILES {
            assert!(texture_bytes(file.path).is_some_and(|b| crate::textures::is_ktx2(&b)), "{}", file.path);
        }
        assert!(texture_bytes("realistic/nothing.ktx2").is_none());
    }

    /// A realistic scene of `view` with the earth, the land and the ground shown.
    fn realistic(view: ViewState, now_ms: f64, weather: Option<Conditions>) -> SceneInput {
        let mut scene = SceneInput::new(view, SIZE, 1.0, Basemap::Realistic);
        scene.local_now_ms = now_ms;
        scene.earth = earth::Input::SHOWN;
        scene.sky = sky::Input { shown: true, weather, daylight: true };
        scene.ground = Input::SHOWN;
        scene
    }

    /// Far out over the globe (V2, no tiles: the land everywhere) the ground's colour is the
    /// Earth picture's, lit (doc 04 §6.8's `far`), and on the night side its towns' lights are
    /// added. Against the CPU's twin from the pictures decoded on the CPU, read anywhere within a
    /// texel of the pixel's place (the GPU interpolates the pictures' coordinates across the
    /// globe's flat 5° facets, which puts a pixel up to a third of a texel off the sphere's own
    /// place, and at a snowfield's edge that is tens of levels): no colour off on average, 98 % of
    /// the sampled pixels within 4 levels, none beyond 8. The textures load once the ground is
    /// shown, and the shot waits for them.
    #[test]
    fn far_out_the_ground_is_the_earth_picture_lit() {
        let Some(gpu) = gpu() else { return };
        let far = twin::FarOut::load().expect("the pictures");
        for (now, name) in [(V2_NOON, "noon"), (MIDWINTER_NIGHT, "night")] {
            let mut renderer = MapRenderer::new(&gpu, FORMAT);
            let scene = realistic(V2, now, Some(PRESETS.sunny));
            let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
            save(&image, &format!("r2-ground-v2-{name}.png"));
            assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
            let view = ViewFrame::new(&V2, SIZE, 1.0, 0.0);
            let look = crate::look::frame_look(&scene.sky, &V2, SIZE, now);
            let light = RLight::new(&look, &view, 0.0);
            let mut worst = (0.0, [0, 0]);
            let mut compared = 0;
            let mut bias = [0.0; 3];
            let mut within = 0;
            for y in (13..720).step_by(27) {
                for x in (13..1100).step_by(27) {
                    let xy = [f64::from(x) + 0.5, f64::from(y) + 0.5];
                    let [lon, lat] = view.viewport.unproject(xy, 0.0);
                    if !(lon.is_finite() && lat.is_finite()) || lat.abs() > 85.0 {
                        continue;
                    }
                    let rl = light::vary(&light, &view, [lon, lat, 0.0]);
                    let (low, high) = far.land_range(&look, &rl, [lon, lat]);
                    let got = image.pixel(x, y);
                    let centre = far.land_colour(&look, &rl, [lon, lat]);
                    for i in 0..3 {
                        bias[i] += f64::from(got[i]) - centre[i];
                    }
                    let off = (0..3).map(|i| (low[i] - f64::from(got[i])).max(f64::from(got[i]) - high[i]).max(0.0)).fold(0.0, f64::max);
                    compared += 1;
                    if off <= 4.0 {
                        within += 1;
                    }
                    if off > worst.0 {
                        worst = (off, [x, y]);
                    }
                }
            }
            let [x, y] = worst.1;
            eprintln!(
                "V2 {name}: {within} of {compared} pixels within 4 levels, worst {:.2} at {x},{y} ({:?}), mean off {:?}",
                worst.0,
                image.pixel(x, y),
                bias.map(|b| b / f64::from(compared))
            );
            let bias = bias.map(|b| b / f64::from(compared));
            assert!(compared > 800);
            // no colour off on the whole, almost every pixel within 4 levels, none far off (where
            // the pictures' detail is sharp the compressed texture and the RGBA copy part a little)
            assert!(bias.iter().all(|b| b.abs() < 1.0), "{name}: biased by {bias:?}");
            assert!(f64::from(within) >= 0.98 * f64::from(compared), "{name}: {within} of {compared} within 4 levels");
            assert!(worst.0 <= 8.0, "{name}: {:.2} levels off at {x},{y}", worst.0);
        }
    }

    /// A test layer: two areas side by side drawn with the ground shader, `left` and `right`
    /// surfaces, round the view's centre.
    struct Patches {
        shader: GroundShader,
        pipeline: wgpu::RenderPipeline,
        mesh: crate::pipelines::fill::Mesh<GroundVertex>,
        slot: Option<FrameSlot>,
        anchor: DVec2,
    }

    /// Local units of [`Patches`] (world units each, a power of two): 181 px at zoom 15.5.
    const PATCH_SCALE: f64 = 1.0 / 256.0;

    impl Patches {
        fn new(init: &Init<'_>, anchor: DVec2, left: [u8; 4], right: [u8; 4]) -> Self {
            let shader = GroundShader::new(init, "test patches");
            let pipeline = shader.areas(init, "test patches", DrawState::SURFACE_NO_CULL);
            let quad = |u0: f32, u1: f32, paint: [u8; 4]| {
                [[u0, -1.5], [u1, -1.5], [u1, 1.5], [u0, 1.5]].map(|[u, v]| GroundVertex { position: [u, v, 0.0], tint: [paint[0], paint[1], paint[2]], surface: paint[3] })
            };
            let vertices: Vec<GroundVertex> = quad(-2.5, -0.1, left).into_iter().chain(quad(0.1, 2.5, right)).collect();
            let mesh = crate::pipelines::fill::Mesh::new(&init.gpu.device, "test patches", &vertices, &[0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
            Patches { shader, pipeline, mesh, slot: None, anchor }
        }
    }

    impl Layer for Patches {
        fn name(&self) -> &'static str {
            "test patches"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.shader.prepare(cx);
            self.slot = Some(cx.frames.push(&cx.view.local(self.anchor, PATCH_SCALE, 0.0)));
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            self.shader.begin(pass, &self.pipeline, LineWidth::Hairline);
            if let Some(slot) = self.slot {
                self.shader.draw_areas(pass, cx, slot, &self.mesh);
            }
        }
    }

    /// A test layer: ground lines in a local frame round the view's centre, with the textures
    /// loading (as the ground loads them).
    struct Strokes {
        shader: GroundShader,
        pipeline: wgpu::RenderPipeline,
        mesh: LineMesh,
        slot: Option<FrameSlot>,
        anchor: DVec2,
        loading: bool,
    }

    impl Layer for Strokes {
        fn name(&self) -> &'static str {
            "test strokes"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            load_textures(cx);
            self.loading = cx.textures.pending();
            self.shader.prepare(cx);
            self.slot = Some(cx.frames.push(&cx.view.local(self.anchor, STROKE_SCALE, 0.0)));
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            self.shader.begin(pass, &self.pipeline, LineWidth::Hairline);
            if let Some(slot) = self.slot {
                self.shader.draw_lines(pass, cx, slot, &self.mesh);
            }
        }

        fn pending(&self) -> bool {
            self.loading
        }
    }

    /// Local units of [`Strokes`] (world units each): 128 px at zoom 17.
    const STROKE_SCALE: f64 = 1.0 / 1024.0;

    /// The ground's lines as deck's PathLayer draws them with the ground's props: metres wide
    /// (`widthUnits: "meters"`), round caps reaching half the width past each end, at least
    /// 0.6 px wide far out; a road in its plain asphalt, a railway's track in its texture, laid
    /// along it (`geometry.uv`: across 0 to 1, a repeat each full width of travel), its sleepers
    /// across it. Straight down at zoom 17 over Stockholm.
    #[test]
    fn ground_lines_are_metres_wide_with_round_ends() {
        use rustradar_scene::geom::path::{LocalMercator, Paint, PathBuilder, PathType};
        let Some(gpu) = gpu() else { return };
        let view = ViewState { latitude: 59.3293, longitude: 18.0686, zoom: 17.0, pitch: 0.0, bearing: 0.0, position: None };
        let anchor = DVec2::from_array(lng_lat_to_world([view.longitude, view.latitude]));
        let local = LocalMercator::new(anchor, STROKE_SCALE);
        let mut paths = PathBuilder::new(local);
        let explicit = |id: u8| rustradar_scene::tess::ground::explicit(rustradar_scene::ground::paint(id, [255, 255, 255]));
        // a road 11 m wide 128 px above the centre, a railway's track 2.6 m wide 128 px below,
        // both from 256 px left of the centre to 256 px right of it
        paths.push(&[[-2.0, -1.0, 0.0], [2.0, -1.0, 0.0]], &Paint { path_type: PathType::Open, ..Paint::solid(explicit(32), 11.0) });
        paths.push(&[[-2.0, 1.0, 0.0], [2.0, 1.0, 0.0]], &Paint { path_type: PathType::Open, ..Paint::solid(explicit(35), 2.6) });
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        let init = renderer.init(&gpu);
        let shader = GroundShader::new(&init, "test strokes");
        let pipeline = shader.lines(&init, "test strokes", DrawState::SURFACE_NO_CULL);
        let mesh = LineMesh::new(&gpu.device, "test strokes", &paths.finish());
        renderer.add_layer(Box::new(Strokes { shader, pipeline, mesh, slot: None, anchor, loading: true }));
        let mut scene = SceneInput::new(view, SIZE, 1.0, Basemap::Realistic);
        scene.local_now_ms = MIDSUMMER;
        // (the light of a fine noon, the sky not drawn: the lines on the map's own background)
        scene.sky = sky::Input { shown: false, weather: Some(PRESETS.sunny), daylight: true };
        let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        save(&image, "r2-ground-lines.png");
        let background = image.pixel(5, 5);
        let frame = ViewFrame::new(&view, SIZE, 1.0, 0.0);
        let px_per_metre = f64::from(frame.globals.centre[2]) * f64::from(frame.globals.pixels_per_unit);
        // the road's column at the centre: covered from top to bottom over its width
        let covered = |x: u32, y0: u32, y1: u32| (y0..y1).filter(|&y| image.pixel(x, y) != background).count();
        let road = covered(550, 200, 280) as f64;
        let expected = 11.0 * px_per_metre;
        eprintln!("road {road} px across, 11 m = {expected:.2} px");
        assert!((road - expected).abs() <= 1.0, "{road} px for {expected:.2}");
        // its round end: half the width past the end, on the axis only
        let end = (550.0 + 256.0) as u32;
        let reach = (end..end + 20).take_while(|&x| image.pixel(x, 232) != background).count() as f64;
        eprintln!("the cap reaches {reach} px past the end ({:.2} expected)", expected / 2.0);
        assert!((reach - expected / 2.0).abs() <= 1.0);
        let corner = image.pixel(end + (expected / 2.0 - 1.0) as u32, 232 - (expected / 2.0 - 1.0) as u32);
        assert_eq!(corner, background, "round, not square: {corner:?}");
        // the railway: textured along its length, its pattern repeating every full width
        let width = 2.6 * px_per_metre;
        let row: Vec<f64> = (300..800).map(|x| f64::from(image.pixel(x, 488)[1])).collect();
        let mean = row.iter().sum::<f64>() / row.len() as f64;
        let wobble = |lag: usize| row.iter().zip(&row[lag..]).map(|(a, b)| (a - mean) * (b - mean)).sum::<f64>() / (row.len() - lag) as f64;
        let period = (2..40).max_by(|&a, &b| wobble(a).total_cmp(&wobble(b))).expect("lags");
        eprintln!("the track's pattern repeats every {period} px; a full width is {width:.2} px");
        assert!(row.iter().any(|&g| (g - mean).abs() > 8.0), "a plain line: {row:?}");
        assert!((period as f64 - width).abs() <= 1.5 || (period as f64 - 2.0 * width).abs() <= 1.5, "{period} px");
    }

    /// Doc 04 §6.8's weather on the ground: the rain darkens what is paved (a road, its plain
    /// asphalt) more than what is not (a dirt track), and makes it shine. Close in on the flat map
    /// (V5's centre from straight above, the camera under the clouds, so the preset's own weather),
    /// at noon, each patch within 3 levels of the CPU's twin: their colours are plain, no texture
    /// in them.
    #[test]
    fn the_rain_darkens_what_is_paved() {
        let Some(gpu) = gpu() else { return };
        let view = ViewState { latitude: 59.3293, longitude: 18.0686, zoom: 15.5, pitch: 0.0, bearing: 0.0, position: None };
        let anchor = DVec2::from_array(lng_lat_to_world([view.longitude, view.latitude]));
        let road = rustradar_scene::tess::ground::explicit(rustradar_scene::ground::paint(32, [255, 255, 255]));
        let dirt = rustradar_scene::tess::ground::explicit(rustradar_scene::ground::paint(34, [255, 255, 255]));
        let mut shades = Vec::new();
        for (name, weather) in [("sunny", PRESETS.sunny), ("rain", PRESETS.rain)] {
            let mut renderer = MapRenderer::new(&gpu, FORMAT);
            let patches = Patches::new(&renderer.init(&gpu), anchor, road, dirt);
            renderer.add_layer(Box::new(patches));
            let mut scene = SceneInput::new(view, SIZE, 1.0, Basemap::Realistic);
            scene.local_now_ms = MIDSUMMER;
            scene.sky = sky::Input { shown: true, weather: Some(weather), daylight: true };
            let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
            save(&image, &format!("r2-ground-rain-{name}.png"));
            let frame = ViewFrame::new(&view, SIZE, 1.0, 0.0);
            let look = crate::look::frame_look(&scene.sky, &view, SIZE, MIDSUMMER);
            assert_eq!(look.wet, weather.wet, "under the clouds: the preset's own weather");
            let light = RLight::new(&look, &frame, 0.0);
            let mut means = Vec::new();
            for (x0, id) in [(400, 32), (700, 34)] {
                let mut sum = DVec4::ZERO;
                for (x, y) in [(x0 - 60, 300), (x0, 360), (x0 + 60, 420)] {
                    let xy = [f64::from(x) + 0.5, f64::from(y) + 0.5];
                    let [lon, lat] = frame.viewport.unproject(xy, 0.0);
                    let rl = light::vary(&light, &frame, [lon, lat, 0.0]);
                    let expected = twin::colour(&look, &rl, &Fragment::stand_in(id, [255, 255, 255]));
                    let got = image.pixel(x, y);
                    eprintln!("{name} surface {id} at {x},{y}: {got:?}, CPU {expected:?}");
                    assert!((0..3).all(|i| (f64::from(got[i]) - expected[i]).abs() <= 3.0), "{name} {id}: {got:?} vs {expected:?}");
                    sum += DVec4::from_array(got.map(f64::from));
                }
                means.push(sum / 3.0);
            }
            shades.push(means);
        }
        let (sunny, rain) = (&shades[0], &shades[1]);
        eprintln!("rain over sun: road {:.3}, dirt {:.3}", rain[0].x / sunny[0].x, rain[1].x / sunny[1].x);
        assert!(rain[0].x < sunny[0].x - 20.0 && rain[1].x < sunny[1].x - 20.0, "the rain darkens both: {rain:?} {sunny:?}");
        // under the rain's own light, wet against dry: the road's asphalt darker by the wet, the
        // dirt by a third as much (its colour a little lower)
        let view_frame = ViewFrame::new(&view, SIZE, 1.0, 0.0);
        let input = sky::Input { shown: true, weather: Some(PRESETS.rain), daylight: true };
        let wet = crate::look::frame_look(&input, &view, SIZE, MIDSUMMER);
        let dry = Look { wet: 0.0, ..wet };
        let light = RLight::new(&wet, &view_frame, 0.0);
        let [lon, lat] = view_frame.viewport.unproject([400.5, 360.5], 0.0);
        let rl = light::vary(&light, &view_frame, [lon, lat, 0.0]);
        let albedo_only = |look: &Look, id: usize| {
            // the albedo's share alone: with the roughness and the normal of the dry ground
            let s = rustradar_scene::ground::surface(id).expect("a surface");
            let flat = glam::DVec3::from_array(s.flat.expect("a plain colour"));
            let paved = if s.paved { 1.0 } else { 0.0 };
            let wet = look.wet * (0.35 + 0.65 * paved);
            flat * (1.0 - 0.45 * wet)
        };
        let road_darkening = albedo_only(&wet, 32).x / albedo_only(&dry, 32).x;
        let dirt_darkening = albedo_only(&wet, 34).x / albedo_only(&dry, 34).x;
        assert!((road_darkening - 0.55).abs() < 1e-9 && (dirt_darkening - 0.8425).abs() < 1e-9);
        let road_dry = twin::colour(&dry, &rl, &Fragment::stand_in(32, [255, 255, 255]));
        let road_wet = twin::colour(&wet, &rl, &Fragment::stand_in(32, [255, 255, 255]));
        eprintln!("the road under the rain's light: dry {road_dry:?}, wet {road_wet:?}, drawn {:?}", rain[0]);
        assert!(road_wet[0] < road_dry[0] - 3.0, "wet asphalt is darker: {road_wet:?} vs {road_dry:?}");
        assert!((rain[0].x - road_wet[0]).abs() <= 3.0 && rain[0].x < road_dry[0] - 3.0);
    }

    /// Doc 04 §6.8's snow: under the snow preset the land lies white (grass in the land's green,
    /// its `GS_SNOW` 1: the snow's own colour) and the roads take slush (a marked road, its
    /// `GS_SNOW` 0.5: half way to a greyer white), while ice (id 14) is snow in any weather. Close
    /// in on the flat map (V5's centre from straight above, under the clouds) at noon, each patch
    /// within 3 levels of the CPU's twin; and under the same light the snow's ground is whiter
    /// than the bare ground the twin gives without it.
    #[test]
    fn snow_lies_on_the_land_and_slush_on_the_roads() {
        use rustradar_scene::ground::{ICE, paint};
        use rustradar_scene::tess::ground::explicit;
        let Some(gpu) = gpu() else { return };
        let view = ViewState { latitude: 59.3293, longitude: 18.0686, zoom: 15.5, pitch: 0.0, bearing: 0.0, position: None };
        let anchor = DVec2::from_array(lng_lat_to_world([view.longitude, view.latitude]));
        let green = [58, 81, 29];
        let grass = paint(4, green);
        let road = paint(32, [255, 255, 255]);
        let ice = paint(ICE, [255, 255, 255]);
        let luma = |c: [f64; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let spread = |c: [f64; 3]| c.iter().copied().fold(f64::NEG_INFINITY, f64::max) - c.iter().copied().fold(f64::INFINITY, f64::min);
        for (name, weather, left, right) in [("snow", PRESETS.snow, grass, road), ("ice", PRESETS.sunny, ice, grass)] {
            let mut renderer = MapRenderer::new(&gpu, FORMAT);
            let patches = Patches::new(&renderer.init(&gpu), anchor, explicit(left), explicit(right));
            renderer.add_layer(Box::new(patches));
            let mut scene = SceneInput::new(view, SIZE, 1.0, Basemap::Realistic);
            scene.local_now_ms = MIDSUMMER;
            scene.sky = sky::Input { shown: true, weather: Some(weather), daylight: true };
            let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
            save(&image, &format!("r2-ground-snow-{name}.png"));
            let frame = ViewFrame::new(&view, SIZE, 1.0, 0.0);
            let look = crate::look::frame_look(&scene.sky, &view, SIZE, MIDSUMMER);
            assert_eq!(look.snow, weather.snow, "under the clouds: the preset's own weather");
            let bare = Look { snow: 0.0, ..look };
            let light = RLight::new(&look, &frame, 0.0);
            let mut drawn = Vec::new();
            for (x0, p) in [(400, left), (700, right)] {
                let id = usize::from(255 - p[3]);
                let fragment = Fragment::stand_in(id, [p[0], p[1], p[2]]);
                let mut sum = [0.0; 3];
                let mut without = [0.0; 3];
                for (x, y) in [(x0 - 60, 300), (x0, 360), (x0 + 60, 420)] {
                    let [lon, lat] = frame.viewport.unproject([f64::from(x) + 0.5, f64::from(y) + 0.5], 0.0);
                    let rl = light::vary(&light, &frame, [lon, lat, 0.0]);
                    let expected = twin::colour(&look, &rl, &fragment);
                    let got = image.pixel(x, y);
                    eprintln!("{name} surface {id} at {x},{y}: {got:?}, CPU {expected:?}");
                    assert!((0..3).all(|i| (f64::from(got[i]) - expected[i]).abs() <= 3.0), "{name} {id}: {got:?} vs {expected:?}");
                    let bare = twin::colour(&bare, &rl, &fragment);
                    for i in 0..3 {
                        sum[i] += f64::from(got[i]) / 3.0;
                        without[i] += bare[i] / 3.0;
                    }
                }
                eprintln!("{name} surface {id}: drawn {sum:?}, bare {without:?}");
                drawn.push((id, sum, without));
            }
            if name == "snow" {
                // the green land white under the snow: brighter and nearly grey; the road's asphalt
                // lighter with slush, but not as white as the land's snow
                let (_, grass, bare_grass) = drawn[0];
                let (_, road, bare_road) = drawn[1];
                assert!(luma(grass) > luma(bare_grass) + 40.0 && spread(grass) < 0.5 * spread(bare_grass), "{grass:?} against {bare_grass:?}");
                assert!(luma(road) > luma(bare_road) + 20.0 && luma(road) < luma(grass) - 10.0, "{road:?} against {bare_road:?} and {grass:?}");
            } else {
                // in the sun, ice is snow and the grass beside it green
                let (_, ice, _) = drawn[0];
                let (_, grass, bare_grass) = drawn[1];
                assert!((0..3).all(|i| (grass[i] - bare_grass[i]).abs() <= 3.0), "no snow on the grass: {grass:?} vs {bare_grass:?}");
                assert!(luma(ice) > luma(grass) + 40.0 && spread(ice) < 0.5 * spread(grass), "{ice:?} beside {grass:?}");
            }
        }
    }
}
