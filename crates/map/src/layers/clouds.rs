//! The realistic map's clouds (item R10; doc 04 §9; `clouds.ts`, `realistic.ts`'s
//! `CloudExtension`, R:892-916; G:2665-2668, G:3606-3638): EUMETSAT's infrared picture of the
//! world's clouds at [`CLOUD_HEIGHT_M`], over the ground but under the map's own lines, trains and
//! aircraft (row 3h: after the hillshade, before the rail map and the radar).
//!
//! - **When** (G:2665-2668): on the realistic basemap, once the scene carries a picture
//!   ([`Input::picture`]: the app's `backend::clouds` feed, asked for while the clouds show) and
//!   while the zoom shows them: opacity [`cloud_opacity`] = `1 − smoothstep(5.5, 6.5, zoom)` of
//!   this frame's camera, none from 6.5 on.
//! - **Where** (`cloudLayers`): on the globe one BitmapLayer over [`CLOUD_BOUNDS`], which deck lays
//!   as a mesh of 5° in longitude and latitude (`createMesh(bounds, resolution 5)`: [`globe_mesh`],
//!   the picture's coordinates linear in degrees); on the flat map (zoom > 12, where the clouds are
//!   gone: kept as today has it) the strips a degree of latitude high round the view's latitude
//!   (`cloudStrips`, `scene::clouds::cloud_strips`: [`strips_mesh`]), each one quad.
//! - **How** (`shaders/cloud.wgsl`; `CLOUD_PARAMETERS`: no culling, no depth written, tested
//!   against the globe's depth so the clouds over its far side stay behind it): BitmapLayer's
//!   texel with its alpha times the opacity, then `cover = smoothstep(0.33, 0.62, grey) · alpha`,
//!   white 0.9 lit by the sun (`rl_sunLight · max(toSun.z, 0)`) and the sky
//!   (`ambient · rl_dayHere`), through the air ([`twin`]).
//! - **The picture** ([`decode`], on the worker pool): the JPEG decoded to its red channel (what
//!   the shader reads; the picture is grey) with the mip chain deck generates for an image
//!   (`getMipLevelCount`: ⌊log2(4096)⌋ + 1 = 13 levels, each texel the mean of the 2 × 2 above
//!   it), uploaded as one `R8Unorm` texture with deck's sampler (linear, linear between levels,
//!   clamped). Levels larger than the device allows are left out. The layer is pending while a
//!   picture decodes (the headless path waits for it); the picture drawn stays until the next one
//!   is in, as today's `image` is swapped once `createImageBitmap` is done.
//!
//! Deviations: the picture is one channel (10 MB with its levels, not 42 MB of RGBA); its texture
//! is kept while the realistic basemap shows, also zoomed in where it is not drawn (deck drops a
//! layer's texture with the layer and uploads the picture again on zooming out); a picture that
//! does not decode is not tried again (today's `createImageBitmap` fails before the picture is
//! taken, which the backend's check of its header stands in for); the flat map's strips sample
//! the one texture at their rows instead of a canvas cut out per strip (the same texels; at a
//! strip's edge the filtering reaches the next row instead of repeating its own).

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::DVec2;
use rustradar_core::geo::Basemap;
use rustradar_scene::camera::{WORLD_SIZE, lng_lat_to_world};
pub use rustradar_scene::clouds::{CLOUD_BOUNDS, CLOUD_HEIGHT_M, CLOUDS_FULL_ZOOM, CLOUDS_GONE_ZOOM, cloud_opacity};
use rustradar_scene::clouds::{Corners, STRIP_REACH, cloud_strips, globe_corners};

use super::{DrawCx, Init, Layer, Pass, PrepareCx};
use crate::frame::{Frame, FrameSlot};
use crate::jobs::{Cancel, Job, JobPoll};
use crate::look::{LightUniform, Look};
use crate::pipelines::bitmap::Vertex;
use crate::pipelines::fill::Mesh;
use crate::pipelines::{Desc, build};
use crate::shaders;
use crate::state::DrawState;

/// `shaders/cloud.wgsl`.
pub const SHADER: &str = include_str!("../shaders/cloud.wgsl");

/// The bind group of the picture, its sampler and the opacity (after the light's, group 2).
pub const CLOUD_GROUP: u32 = 3;

/// deck's globe tessellation step, degrees (`GlobeView({ resolution: 5 })`).
pub const RESOLUTION_DEG: f64 = 5.0;

/// When the picture decodes among the pool's jobs: after the tiles in view.
const DECODE_PRIORITY: f64 = 10.0;

/// A cloud picture as EUMETSAT sends it: the JPEG's bytes, shared with the app's feed
/// (`backend::clouds::Picture::jpeg`). Two are the same picture when they share the bytes.
#[derive(Clone)]
pub struct Picture(pub Arc<[u8]>);

impl Picture {
    /// Whether `other` is this picture (the same bytes, not a copy of them).
    pub fn same(&self, other: &Picture) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({} bytes)", self.0.len())
    }
}

/// What the scene builder hands the clouds (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// The latest cloud picture (`cloudPicture`'s), `None` until one has come: no clouds.
    pub picture: Option<Picture>,
}

/// `cloud.wgsl`'s `Cloud`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct CloudUniform {
    /// The layer's opacity.
    pub opacity: f32,
    pub pad: [f32; 3],
}

// ------------------------------------------------------------------ meshes

/// `[lon, lat]` in the world's local frame (`ViewFrame::local` anchored at the world's north-west
/// corner, 512 units a local unit): `u` east, `v` south, both 0–1.
fn world_local([lon, lat]: [f64; 2]) -> [f32; 2] {
    let [x, y] = lng_lat_to_world([lon, lat]);
    [(x / WORLD_SIZE) as f32, ((WORLD_SIZE - y) / WORLD_SIZE) as f32]
}

/// A corner's vertex: its place in the world's local frame, its height, the picture there.
fn vertex([lon, lat, h]: [f64; 3], tex: [f64; 2]) -> Vertex {
    let [u, v] = world_local([lon, lat]);
    Vertex { position: [u, v, h as f32], tex: [tex[0] as f32, tex[1] as f32] }
}

/// deck's `createMesh(bounds, resolution)` (`bitmap-layer/create-mesh.js`) for `bounds` (lower
/// left, upper left, upper right, lower right; each `[lon, lat, metres]`): a grid of
/// `⌈span / resolution⌉ + 1` points each way, column by column from the west, each column from the
/// south, positions interpolated in degrees and the texture coordinates `(u, 1 − v)` with them;
/// two triangles a cell in deck's order.
pub fn bitmap_mesh(bounds: &Corners, resolution: f64) -> (Vec<Vertex>, Vec<u32>) {
    let max_x_span = (bounds[0][0] - bounds[3][0]).abs().max((bounds[1][0] - bounds[2][0]).abs());
    let max_y_span = (bounds[1][1] - bounds[0][1]).abs().max((bounds[2][1] - bounds[3][1]).abs());
    let u_count = (max_x_span / resolution).ceil() as u32 + 1;
    let v_count = (max_y_span / resolution).ceil() as u32 + 1;
    let lerp = |a: [f64; 3], b: [f64; 3], t: f64| std::array::from_fn::<f64, 3, _>(|i| a[i] + (b[i] - a[i]) * t);
    let mut vertices = Vec::with_capacity((u_count * v_count) as usize);
    let mut indices = Vec::with_capacity(((u_count - 1) * (v_count - 1) * 6) as usize);
    let mut index = 0;
    for u in 0..u_count {
        let ut = f64::from(u) / f64::from(u_count - 1);
        for v in 0..v_count {
            let vt = f64::from(v) / f64::from(v_count - 1);
            let p = lerp(lerp(bounds[0], bounds[1], vt), lerp(bounds[3], bounds[2], vt), ut);
            vertices.push(vertex(p, [ut, 1.0 - vt]));
            if u > 0 && v > 0 {
                indices.extend([index - v_count, index - v_count - 1, index - 1, index - v_count, index - 1, index]);
            }
            index += 1;
        }
    }
    (vertices, indices)
}

/// The globe's clouds: the picture over [`CLOUD_BOUNDS`] at [`CLOUD_HEIGHT_M`] in deck's 5° mesh
/// (73 × 35 points).
pub fn globe_mesh() -> (Vec<Vertex>, Vec<u32>) {
    bitmap_mesh(&globe_corners(), RESOLUTION_DEG)
}

/// deck's `createQuad` texture coordinates: lower left, upper left, upper right, lower right.
const QUAD_TEX: [[f64; 2]; 4] = [[0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
/// deck's `DEFAULT_INDICES`.
const QUAD_INDICES: [u32; 6] = [0, 2, 1, 0, 3, 2];

/// The flat map's clouds round `lat` for a picture `height` rows high: each strip of
/// `cloud_strips(height, lat, 3)` as deck's one quad, its texture coordinates its rows of the
/// picture (`y0..y1`), which its canvas held today.
pub fn strips_mesh(height: u32, lat: f64) -> (Vec<Vertex>, Vec<u32>) {
    let strips = cloud_strips(height, lat, STRIP_REACH);
    let mut vertices = Vec::with_capacity(strips.len() * 4);
    let mut indices = Vec::with_capacity(strips.len() * 6);
    for strip in &strips {
        let first = vertices.len() as u32;
        let (top, rows) = (f64::from(strip.y0), f64::from(strip.y1 - strip.y0));
        for (corner, [u, v]) in strip.corners.iter().zip(QUAD_TEX) {
            vertices.push(vertex(*corner, [u, (top + v * rows) / f64::from(height)]));
        }
        indices.extend(QUAD_INDICES.map(|i| first + i));
    }
    (vertices, indices)
}

// ------------------------------------------------------------------ the picture

/// A decoded picture: its red channel, one row after another from the top, level by level from
/// the largest kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    /// The size of the first level kept.
    pub width: u32,
    pub height: u32,
    /// How many of the file's largest levels were left out (too large for the device).
    pub skipped: u32,
    pub levels: Vec<Vec<u8>>,
    /// The picture's own height, rows (the flat map's strips are cut by it).
    pub rows: u32,
}

/// The next level down: each texel the rounded mean of the 2 × 2 above it (the last row or
/// column repeated where a size is odd), as `pipelines::bitmap` makes deck's mips.
pub fn half_size(grey: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let at = |x: u32, y: u32| u32::from(grey[(y.min(h - 1) * w + x.min(w - 1)) as usize]);
    let mut out = Vec::with_capacity((nw * nh) as usize);
    for y in 0..nh {
        for x in 0..nw {
            let sum = at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1);
            out.push(((sum + 2) / 4) as u8);
        }
    }
    (out, nw, nh)
}

/// The picture `jpeg` (any image the decoder reads) as the shader samples it: its red channel and
/// deck's mip chain (`⌊log2(max(w, h))⌋ + 1` levels), from the first level no side of which is
/// over `max_size` (the last kept whatever its size). On the worker pool.
pub fn decode(jpeg: &[u8], max_size: u32, cancel: &Cancel) -> Result<Decoded, String> {
    let image = image::load_from_memory(jpeg).map_err(|e| format!("the cloud picture: {e}"))?;
    let (mut w, mut h) = (image.width().max(1), image.height().max(1));
    let rows = h;
    let mut level = match image {
        image::DynamicImage::ImageLuma8(grey) => grey.into_raw(),
        other => other.into_rgb8().pixels().map(|p| p[0]).collect(),
    };
    let count = 32 - w.max(h).leading_zeros();
    let mut levels = Vec::with_capacity(count as usize);
    let mut skipped = 0;
    let (mut width, mut height) = (w, h);
    for i in 0..count {
        if cancel.is_cancelled() {
            return Err("cancelled".to_owned());
        }
        if w.max(h) > max_size && i + 1 < count {
            skipped += 1;
        } else {
            if levels.is_empty() {
                (width, height) = (w, h);
            }
            levels.push(level.clone());
        }
        if i + 1 < count {
            (level, w, h) = half_size(&level, w, h);
        }
    }
    Ok(Decoded { width, height, skipped, levels, rows })
}

/// `decoded` as an `R8Unorm` texture with its levels.
fn upload(device: &wgpu::Device, queue: &wgpu::Queue, decoded: &Decoded) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cloud picture"),
        size: wgpu::Extent3d { width: decoded.width, height: decoded.height, depth_or_array_layers: 1 },
        mip_level_count: decoded.levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let [mut w, mut h] = [decoded.width, decoded.height];
    for (mip, level) in decoded.levels.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: mip as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            level,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        (w, h) = ((w / 2).max(1), (h / 2).max(1));
    }
    texture
}

// ------------------------------------------------------------------ the layer

/// The picture on the GPU.
struct Shown {
    picture: Picture,
    group: wgpu::BindGroup,
    height: u32,
}

/// Row 3h: `clouds-globe-0` / `clouds-flat-N` (realistic basemap).
pub struct Clouds {
    light: LightUniform,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    opacity: f32,
    globe: Mesh<Vertex>,
    /// The flat map's strips: for which degree of latitude and picture height.
    strips: Option<(i32, u32, Mesh<Vertex>)>,
    shown: Option<Shown>,
    decoding: Option<(Picture, Job<Result<Decoded, String>>)>,
    /// The last picture that did not decode (not tried again).
    failed: Option<Picture>,
    /// This frame's frames (none: not drawn), on the flat map with the world's copies.
    slots: Vec<FrameSlot>,
    flat: bool,
    look: Option<Look>,
}

/// `frame` moved `dx` world units east, its offset split again into hi + lo.
fn shifted(frame: &Frame, dx: f64) -> Frame {
    let x = f64::from(frame.offset_hi[0]) + f64::from(frame.offset_lo[0]) + dx;
    let hi = x as f32;
    Frame { offset_hi: [hi, frame.offset_hi[1]], offset_lo: [(x - f64::from(hi)) as f32, frame.offset_lo[1]], ..*frame }
}

impl Clouds {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let light = LightUniform::new(device);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("clouds"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(size_of::<CloudUniform>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let module = shaders::module(device, "clouds", SHADER);
        let pipeline = build(
            init,
            &Desc {
                label: "clouds",
                module: &module,
                vertex_entry: "vs",
                fragment_entry: "fs",
                buffers: &[Some(Vertex::LAYOUT)],
                topology: wgpu::PrimitiveTopology::TriangleList,
                state: DrawState::CLOUD,
                groups: &[light.layout(), &layout],
            },
        );
        // deck's DEFAULT_TEXTURE_PARAMETERS (`@deck.gl/core` utils/texture.js)
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("clouds"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let uniform = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("clouds"),
                contents: bytemuck::bytes_of(&CloudUniform { opacity: 0.0, pad: [0.0; 3] }),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let (vertices, indices) = globe_mesh();
        let globe = Mesh::new(device, "clouds globe", &vertices, &indices);
        Clouds {
            light,
            pipeline,
            layout,
            sampler,
            uniform,
            opacity: 0.0,
            globe,
            strips: None,
            shown: None,
            decoding: None,
            failed: None,
            slots: Vec::new(),
            flat: false,
            look: None,
        }
    }

    /// The opacity of the last frame drawn (0: none).
    pub fn opacity(&self) -> f32 {
        if self.slots.is_empty() { 0.0 } else { self.opacity }
    }

    /// The light of the last frame drawn.
    pub fn look(&self) -> Option<&Look> {
        self.look.as_ref()
    }

    /// Whether `picture` is the one on the GPU.
    pub fn shows(&self, picture: &Picture) -> bool {
        self.shown.as_ref().is_some_and(|s| s.picture.same(picture))
    }

    /// Takes a decoded picture in, if its job is done.
    fn take_decoded(&mut self, cx: &PrepareCx<'_>) {
        let Some((picture, job)) = &mut self.decoding else { return };
        let decoded = match job.poll() {
            JobPoll::Pending => return,
            JobPoll::Done(Ok(decoded)) => decoded,
            JobPoll::Done(Err(e)) => {
                log::warn!("clouds: {e}");
                self.failed = Some(picture.clone());
                self.decoding = None;
                return;
            }
            JobPoll::Cancelled | JobPoll::Lost => {
                self.decoding = None;
                return;
            }
        };
        let texture = upload(&cx.gpu.device, &cx.gpu.queue, &decoded);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let group = cx.gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("clouds"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: self.uniform.as_entire_binding() },
            ],
        });
        self.shown = Some(Shown { picture: picture.clone(), group, height: decoded.rows });
        self.decoding = None;
    }
}

impl Layer for Clouds {
    fn name(&self) -> &'static str {
        "clouds"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.slots.clear();
        self.look = None;
        // off the realistic basemap, or with no picture: nothing, and nothing kept
        let picture = cx.scene.clouds.picture.as_ref().filter(|_| cx.scene.basemap == Basemap::Realistic);
        let Some(picture) = picture else {
            self.shown = None;
            self.decoding = None;
            self.strips = None;
            return;
        };
        let opacity = cloud_opacity(cx.view.view.zoom) as f32;
        // a new picture decodes while the clouds show; the one drawn stays until it is in
        let known = self.shows(picture)
            || self.decoding.as_ref().is_some_and(|(p, _)| p.same(picture))
            || self.failed.as_ref().is_some_and(|p| p.same(picture));
        if !known && opacity > 0.0 {
            let (jpeg, max_size) = (picture.0.clone(), cx.gpu.device.limits().max_texture_dimension_2d);
            let job = cx.tiles.pool.spawn(DECODE_PRIORITY, move |cancel| decode(&jpeg, max_size, cancel));
            self.decoding = Some((picture.clone(), job));
        }
        self.take_decoded(cx);
        let Some(shown) = &self.shown else { return };
        if opacity <= 0.0 {
            return;
        }
        if opacity != self.opacity {
            self.opacity = opacity;
            cx.gpu.queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&CloudUniform { opacity, pad: [0.0; 3] }));
        }
        self.look = Some(self.light.prepare(cx));
        let world = cx.view.local(DVec2::new(0.0, WORLD_SIZE), WORLD_SIZE, 0.0);
        self.flat = !cx.view.is_globe();
        if self.flat {
            // the strips round the view's latitude, made again when it moves to another degree
            let lat = cx.view.view.latitude;
            let degree = lat.floor() as i32;
            if self.strips.as_ref().is_none_or(|(d, h, _)| (*d, *h) != (degree, shown.height)) {
                let (vertices, indices) = strips_mesh(shown.height, lat);
                self.strips = Some((degree, shown.height, Mesh::new(&cx.gpu.device, "clouds strips", &vertices, &indices)));
            }
            for copy in [-1.0, 0.0, 1.0] {
                self.slots.push(cx.frames.push(&shifted(&world, copy * WORLD_SIZE)));
            }
        } else {
            self.slots.push(cx.frames.push(&world));
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        let Some(shown) = &self.shown else { return };
        if self.slots.is_empty() || cx.pass != Pass::Map {
            return;
        }
        let mesh = if self.flat {
            match &self.strips {
                Some((_, _, mesh)) => mesh,
                None => return,
            }
        } else {
            &self.globe
        };
        pass.set_pipeline(&self.pipeline);
        self.light.bind(pass);
        pass.set_bind_group(CLOUD_GROUP, &shown.group, &[]);
        for slot in &self.slots {
            cx.bind_frame(pass, *slot);
            mesh.draw(pass);
        }
    }

    fn pending(&self) -> bool {
        self.decoding.is_some()
    }
}

/// `cloud.wgsl` on the CPU (f64): a cloud's colour where the picture's grey is `grey` (0–1), lit
/// by `look` where the light's varyings are `rl` (`look::twin::vary` at the cloud's height), at
/// `opacity`.
pub mod twin {
    use glam::DVec3;

    use crate::look::Look;
    use crate::look::twin::{Vary, aerial, day_here, display, sun_light};

    fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
        let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// The cloud's colour (0–255, unrounded) and its alpha (`cover`, 0–1).
    pub fn colour(look: &Look, rl: &Vary, grey: f64, opacity: f64) -> ([f64; 3], f64) {
        let cover = smoothstep(0.33, 0.62, grey) * opacity;
        let to_sun = rl.sun.normalize();
        let lit = DVec3::splat(0.9) * (sun_light(look, rl) * to_sun.z.max(0.0) + DVec3::splat(look.ambient * day_here(look, rl)));
        (display(look, aerial(look, rl, lit)), cover)
    }

    /// The cloud over `under` (0–255) as the map pass blends it (deck's `src-alpha,
    /// one-minus-src-alpha`).
    pub fn over(cloud: ([f64; 3], f64), under: [f64; 3]) -> [f64; 3] {
        let (c, a) = cloud;
        std::array::from_fn(|i| c[i] * a + under[i] * (1.0 - a))
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::weather::PRESETS;
    use rustradar_scene::camera::ViewState;
    use rustradar_scene::clouds::cloud_strip;

    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::{earth, ground, sky};
    use crate::look::{self, RLight};
    use crate::pipelines::fill::tests::wgsl_members;
    use crate::renderer::{MapRenderer, SceneInput};

    /// The committed cloud picture (`cargo xtask fixtures`, item X1).
    fn fixture() -> Vec<u8> {
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/clouds/worldcloudmap_ir108.jpg")).expect("the fixture picture")
    }

    #[test]
    fn the_uniform_matches_the_wgsl() {
        assert_eq!(size_of::<CloudUniform>(), 16);
        let names = ["opacity", "pad0", "pad1", "pad2"];
        let wgsl: Vec<(String, u32)> = names.iter().zip([0, 4, 8, 12]).map(|(n, o)| (n.to_string(), o)).collect();
        assert_eq!(wgsl_members(SHADER, "Cloud"), (16, wgsl));
    }

    /// deck's mesh of the globe's quad: 73 columns of 35 points, 5° apart, the texture's `v` down
    /// from the north in degrees, two triangles a cell; every triangle within one cell.
    #[test]
    fn the_globe_mesh_is_decks_5_degree_grid() {
        let (vertices, indices) = globe_mesh();
        assert_eq!((vertices.len(), indices.len()), (73 * 35, 72 * 34 * 6));
        let at = |lon: f64, lat: f64| world_local([lon, lat]);
        // the first column from the south-west corner up, the last at the east edge
        assert_eq!(vertices[0].position, [at(-180.0, -85.0)[0], at(-180.0, -85.0)[1], 10_000.0]);
        assert_eq!(vertices[0].tex, [0.0, 1.0]);
        assert_eq!(vertices[34].tex, [0.0, 0.0]);
        assert_eq!(&vertices[34].position[..2], &at(-180.0, 85.0));
        assert_eq!(vertices[35].tex, [1.0 / 72.0, 1.0]);
        let last = vertices[73 * 35 - 1];
        assert_eq!((&last.position[..2], last.tex), (&at(180.0, 85.0)[..], [1.0, 0.0]));
        // deck's first cell: (35, 0, 34), (35, 34, 36)… for the vertex at column 1, row 1
        assert_eq!(&indices[..6], &[1, 0, 35, 1, 35, 36]);
        // the texture's v is linear in latitude (the picture is equirectangular)
        let lat_of = |v: &Vertex| rustradar_scene::camera::world_to_lng_lat([0.0, WORLD_SIZE - f64::from(v.position[1]) * WORLD_SIZE])[1];
        for v in &vertices {
            let lat = lat_of(v);
            assert!((f64::from(v.tex[1]) - (85.0 - lat) / 170.0).abs() < 1e-5, "{lat}: {:?}", v.tex);
        }
        for t in indices.chunks(3) {
            let lats: Vec<f64> = t.iter().map(|&i| lat_of(&vertices[i as usize])).collect();
            let us: Vec<f32> = t.iter().map(|&i| vertices[i as usize].tex[0]).collect();
            let span = |v: &[f64]| v.iter().cloned().fold(f64::MIN, f64::max) - v.iter().cloned().fold(f64::MAX, f64::min);
            assert!(span(&lats) <= 5.0 + 1e-3);
            assert!(f64::from(us.iter().cloned().fold(f32::MIN, f32::max) - us.iter().cloned().fold(f32::MAX, f32::min)) * 360.0 <= 5.0 + 1e-4);
        }
    }

    /// The flat map's strips: `cloudStrips`' quads, their texture the rows each strip's canvas
    /// held.
    #[test]
    fn the_strips_take_their_rows_of_the_picture() {
        let (vertices, indices) = strips_mesh(1934, 55.6);
        assert_eq!((vertices.len(), indices.len()), (7 * 4, 7 * 6));
        assert_eq!(&indices[6..12], &[4, 6, 5, 4, 7, 6]);
        let strip = cloud_strip(1934, 52).unwrap();
        let first = &vertices[..4];
        for (v, (corner, tex)) in first.iter().zip(strip.corners.iter().zip(QUAD_TEX)) {
            let local = world_local([corner[0], corner[1]]);
            assert_eq!(v.position, [local[0], local[1], 10_000.0]);
            let rows = if tex[1] == 0.0 { strip.y0 } else { strip.y1 };
            assert_eq!(v.tex, [tex[0] as f32, (f64::from(rows) / 1934.0) as f32]);
        }
        // a strip's top edge is where its first row's top is: the picture's v there is linear in
        // latitude, as on the globe
        let top = (85.0 - strip.corners[1][1]) / 170.0;
        assert!((f64::from(first[1].tex[1]) - top).abs() < 1e-6);
        assert!(strips_mesh(1934, f64::NAN).0.is_empty());
    }

    #[test]
    fn the_picture_decodes_to_its_red_and_decks_mips() {
        let jpeg = fixture();
        let started = std::time::Instant::now();
        let d = decode(&jpeg, 8192, &Cancel::new()).expect("decoded");
        eprintln!("decoded in {:?}", started.elapsed());
        assert_eq!((d.width, d.height, d.skipped, d.levels.len()), (4096, 1934, 0, 13));
        let reference = image::load_from_memory(&jpeg).unwrap().into_rgb8();
        assert_eq!(reference.get_pixel(1234, 567)[0], d.levels[0][567 * 4096 + 1234]);
        let sizes: Vec<usize> = d.levels.iter().map(Vec::len).collect();
        assert_eq!(sizes, [4096 * 1934, 2048 * 967, 1024 * 483, 512 * 241, 256 * 120, 128 * 60, 64 * 30, 32 * 15, 16 * 7, 8 * 3, 4, 2, 1]);
        // a level's texel is the mean of the four above it
        let above = |x: usize, y: usize| u32::from(d.levels[0][y * 4096 + x]);
        let mean = (above(200, 300) + above(201, 300) + above(200, 301) + above(201, 301) + 2) / 4;
        assert_eq!(u32::from(d.levels[1][150 * 2048 + 100]), mean);
        // a device of 2048: from 2048 × 967
        let small = decode(&jpeg, 2048, &Cancel::new()).unwrap();
        assert_eq!((small.width, small.height, small.skipped, small.levels.len()), (2048, 967, 1, 12));
        assert_eq!(small.levels[0], d.levels[1]);
        assert!(decode(b"no picture", 8192, &Cancel::new()).is_err());
        let cancel = Cancel::new();
        cancel.cancel();
        assert!(decode(&jpeg, 8192, &cancel).is_err());
    }

    #[test]
    fn half_sizes_round_the_mean_and_repeat_the_odd_edge() {
        let (out, w, h) = half_size(&[0, 10, 255, 20, 30, 255], 3, 2);
        assert_eq!((out, w, h), (vec![15], 1, 1));
        let (out, w, h) = half_size(&[1, 2, 3], 1, 3);
        assert_eq!((out, w, h), (vec![2], 1, 1));
    }

    /// V1 (plan section 8): the whole globe.
    const V1: ViewState = ViewState { latitude: 35.0, longitude: 10.0, zoom: 1.9, pitch: 0.0, bearing: 0.0, position: None };
    /// 2026-06-21 10:00 UTC: the reference pictures' clock.
    const MIDSUMMER: f64 = 1_782_036_000_000.0;
    /// 2026-06-21 18:30 UTC: the sun setting over Europe and Africa (V1's night side to the east).
    const DUSK: f64 = 1_782_066_600_000.0;
    const SIZE: [f64; 2] = [1100.0, 720.0];
    const SUNNY: sky::Input = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
    /// How far (levels, each channel) a pixel may be from the CPU's range.
    const TOLERANCE: f64 = 3.0;

    /// Keeps a shot in `target/shots/` for review.
    fn save(image: &Image, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }

    /// The realistic globe as the app's scene builder has it (the sky, the earth, the ground and
    /// its textures), with or without `picture`.
    fn scene(view: ViewState, now_ms: f64, picture: Option<&Picture>) -> SceneInput {
        let mut scene = SceneInput::new(view, SIZE, 1.0, Basemap::Realistic);
        scene.local_now_ms = now_ms;
        scene.feed_now_ms = now_ms;
        scene.earth = earth::Input::SHOWN;
        scene.sky = SUNNY;
        scene.ground = ground::Input::SHOWN;
        scene.clouds = Input { picture: picture.cloned() };
        scene
    }

    /// `scene` drawn by a renderer of its own, the ground's tiles from the committed fixtures.
    fn shot(gpu: &crate::gpu::Gpu, scene: &SceneInput) -> (Image, MapRenderer) {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        renderer.set_tiles(std::sync::Arc::new(crate::io::DirTiles::fixtures()), None);
        let image = render_with(gpu, &mut renderer, scene, 1.0).expect("rendered");
        assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
        (image, renderer)
    }

    /// The picture's grey (0–1) over the texels within `reach` of `[lon, lat]`: the lowest and
    /// the highest (the GPU's filtered sample lies between them).
    fn grey_range(d: &Decoded, [lon, lat]: [f64; 2], reach: i64) -> (f64, f64) {
        let (w, h) = (i64::from(d.width), i64::from(d.height));
        let x = ((lon + 180.0) / 360.0 * w as f64) as i64;
        let y = ((85.0 - lat) / 170.0 * h as f64) as i64;
        let (mut low, mut high) = (255u8, 0u8);
        for dy in -reach..=reach {
            for dx in -reach..=reach {
                let g = d.levels[0][((y + dy).clamp(0, h - 1) * w + (x + dx).rem_euclid(w)) as usize];
                low = low.min(g);
                high = high.max(g);
            }
        }
        (f64::from(low) / 255.0, f64::from(high) / 255.0)
    }

    /// What the CPU says of the pixel `xy`: the clouds' colour over `under` (the pixel drawn
    /// without them) for the least and the most cloud the picture has within `reach` texels of the
    /// place the pixel sees at the clouds' height, and that least and most `cover`; `None` where
    /// the pixel sees no cloud sphere (off the globe, near its limb, past the picture's edges).
    fn expected(look: &Look, view: &ViewFrame, d: &Decoded, xy: [f64; 2], under: [u8; 4], opacity: f64, reach: i64) -> Option<([[f64; 3]; 2], [f64; 2], look::twin::Vary)> {
        if let Some(globe) = view.viewport.globe()
            && globe.ray_distance_to_globe_center_ratio(xy) >= 0.75
        {
            return None;
        }
        let [lon, lat] = view.viewport.unproject(xy, CLOUD_HEIGHT_M);
        if !(lon.is_finite() && lat.is_finite()) || lat.abs() > 84.0 {
            return None;
        }
        let rl = look::twin::vary(&RLight::new(look, view, 0.0), view, [lon, lat, CLOUD_HEIGHT_M]);
        let (low, high) = grey_range(d, [lon, lat], reach);
        let under = [f64::from(under[0]), f64::from(under[1]), f64::from(under[2])];
        let (thin, thick) = (twin::colour(look, &rl, low, opacity), twin::colour(look, &rl, high, opacity));
        Some(([twin::over(thin, under), twin::over(thick, under)], [thin.1, thick.1], rl))
    }

    /// How far `got` lies outside the range between `a` and `b`, the most of its channels.
    fn outside(got: [u8; 4], [a, b]: [[f64; 3]; 2]) -> f64 {
        (0..3).map(|i| (a[i].min(b[i]) - f64::from(got[i])).max(f64::from(got[i]) - a[i].max(b[i])).max(0.0)).fold(0.0, f64::max)
    }

    /// Item R10's acceptance, in the map: V1 on the realistic globe (the committed tiles) with the
    /// fixture's picture, at the reference pictures' summer morning and at dusk. Every sampled
    /// pixel where the globe meets the clouds' sphere away from its limb is the CPU's clouds
    /// (`twin`) over the globe drawn without them, within 3 levels of the range the picture's grey
    /// round the place gives; where it is clear all round, the globe shows through unchanged; thick
    /// cloud (at least 0.75 cover all round) is white in the sun and dark on the night side. The layer waits for the picture to
    /// decode.
    #[test]
    fn v1_shows_the_fixture_clouds_lit_as_the_cpu_says() {
        let Some(gpu) = gpu() else { return };
        let picture = Picture(fixture().into());
        let decoded = decode(&picture.0, 8192, &Cancel::new()).unwrap();
        let (mut thick_lit, mut thick_dark) = (Vec::new(), Vec::new());
        for (now, name) in [(MIDSUMMER, "noon"), (DUSK, "dusk")] {
            let (bare, _) = shot(&gpu, &scene(V1, now, None));
            let (clouded, renderer) = shot(&gpu, &scene(V1, now, Some(&picture)));
            save(&bare, &format!("r10-v1-{name}-bare.png"));
            save(&clouded, &format!("r10-v1-{name}-clouds.png"));
            drop(renderer);
            let look = look::frame_look(&SUNNY, &V1, SIZE, now);
            let view = ViewFrame::new(&V1, SIZE, 1.0, 0.0);
            let (mut compared, mut thick, mut clear, mut worst) = (0, 0, 0, (0.0f64, [0u32; 2]));
            for y in (4..720).step_by(9) {
                for x in (4..1100).step_by(9) {
                    let xy = [f64::from(x) + 0.5, f64::from(y) + 0.5];
                    let under = bare.pixel(x, y);
                    let Some((range, cover, rl)) = expected(&look, &view, &decoded, xy, under, 1.0, 12) else { continue };
                    let got = clouded.pixel(x, y);
                    compared += 1;
                    let off = outside(got, range);
                    if off > worst.0 {
                        worst = (off, [x, y]);
                    }
                    if cover[1] == 0.0 {
                        clear += 1;
                        assert_eq!(got, under, "clear sky at {x},{y}");
                    } else if cover[0] >= 0.75 {
                        thick += 1;
                        let luma = 0.2126 * f64::from(got[0]) + 0.7152 * f64::from(got[1]) + 0.0722 * f64::from(got[2]);
                        let up = rl.sun.normalize().z;
                        if up > 0.3 {
                            thick_lit.push(luma);
                        } else if up < -0.2 {
                            thick_dark.push(luma);
                        }
                    }
                }
            }
            let [x, y] = worst.1;
            eprintln!("{name}: {compared} pixels, {thick} thick, {clear} clear; worst {:.2} levels at {x},{y}", worst.0);
            assert!(compared > 1000 && thick >= 5 && clear > 100, "{compared} pixels, {thick} thick, {clear} clear");
            assert!(worst.0 <= TOLERANCE, "{:.2} levels off at {x},{y}: {:?}", worst.0, clouded.pixel(x, y));
        }
        // white in the sun, dark on the night side
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        eprintln!("thick cloud: {} in the sun (mean luma {:.1}), {} at night ({:.1})", thick_lit.len(), mean(&thick_lit), thick_dark.len(), mean(&thick_dark));
        assert!(thick_lit.len() >= 5 && !thick_dark.is_empty(), "{} lit, {} dark", thick_lit.len(), thick_dark.len());
        assert!(mean(&thick_lit) > 180.0 && mean(&thick_dark) < 40.0);
    }

    /// The clouds fade between zoom 5.5 and 6.5: half way through (over the thick clouds off
    /// Senegal in the fixture's picture) they are drawn at half their cover over the ground; from
    /// 6.5 none is drawn and nothing waits for the picture.
    #[test]
    fn the_clouds_fade_out_as_the_view_zooms_in() {
        let Some(gpu) = gpu() else { return };
        let picture = Picture(fixture().into());
        let decoded = decode(&picture.0, 8192, &Cancel::new()).unwrap();
        let view = |zoom: f64| ViewState { latitude: 13.8, longitude: -17.2, zoom, pitch: 0.0, bearing: 0.0, position: None };
        let half = view(6.0);
        let (bare, _) = shot(&gpu, &scene(half, MIDSUMMER, None));
        let (clouded, _) = shot(&gpu, &scene(half, MIDSUMMER, Some(&picture)));
        save(&clouded, "r10-zoom6-clouds.png");
        let look = look::frame_look(&SUNNY, &half, SIZE, MIDSUMMER);
        let frame = ViewFrame::new(&half, SIZE, 1.0, 0.0);
        let (mut compared, mut thick, mut worst) = (0, 0, (0.0f64, [0u32; 2]));
        for y in (5..720).step_by(10) {
            for x in (5..1100).step_by(10) {
                let xy = [f64::from(x) + 0.5, f64::from(y) + 0.5];
                let Some((range, cover, _)) = expected(&look, &frame, &decoded, xy, bare.pixel(x, y), 0.5, 2) else { continue };
                compared += 1;
                if cover[0] >= 0.45 {
                    thick += 1;
                }
                let off = outside(clouded.pixel(x, y), range);
                if off > worst.0 {
                    worst = (off, [x, y]);
                }
            }
        }
        let [x, y] = worst.1;
        eprintln!("zoom 6: {compared} pixels, {thick} thick; worst {:.2} levels at {x},{y}", worst.0);
        assert!(compared > 5000 && thick > 50, "{compared} pixels, {thick} thick");
        assert!(worst.0 <= TOLERANCE, "{:.2} levels off at {x},{y}: {:?}", worst.0, clouded.pixel(x, y));
        // from 6.5 on: none
        let gone = view(6.5);
        let (bare, _) = shot(&gpu, &scene(gone, MIDSUMMER, None));
        let (clouded, renderer) = shot(&gpu, &scene(gone, MIDSUMMER, Some(&picture)));
        assert!(bare.rgba == clouded.rgba);
        assert!(!renderer.pending(), "nothing waits for a picture that is not drawn");
    }

    /// No picture, or another basemap: no clouds, nothing pending; a picture that is no picture is
    /// not drawn.
    #[test]
    fn without_a_picture_or_off_the_realistic_map_nothing_is_drawn() {
        let Some(gpu) = gpu() else { return };
        let picture = Picture(fixture().into());
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        let mut dark = scene(V1, MIDSUMMER, Some(&picture));
        dark.basemap = Basemap::Dark;
        let with = render_with(&gpu, &mut renderer, &dark, 1.0).unwrap();
        dark.clouds = Input::default();
        let without = render_with(&gpu, &mut renderer, &dark, 1.0).unwrap();
        assert!(with.rgba == without.rgba);
        assert!(!renderer.pending());
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        let broken = Picture(b"no picture".to_vec().into());
        let bad = render_with(&gpu, &mut renderer, &scene(V1, MIDSUMMER, Some(&broken)), 1.0).unwrap();
        let none = render_with(&gpu, &mut renderer, &scene(V1, MIDSUMMER, None), 1.0).unwrap();
        assert!(bad.rgba == none.rgba);
        assert!(!renderer.pending());
    }
}
