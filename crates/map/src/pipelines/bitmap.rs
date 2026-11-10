//! The `bitmap` pipeline (plan 1.4.6): deck.gl's BitmapLayer as the app uses it, for Web Mercator
//! tile images drawn on the ground: the satellite imagery (doc 01 §5.8), the hillshade (doc 04 §12)
//! and the OpenRailwayMap raster. Its shader is `shaders/bitmap.wgsl`; the clouds have their own
//! (`cloud.wgsl`, item R10).
//!
//! - [`Vertex`]: `(u, v, h)` in the draw's local Mercator frame and the texture coordinates there.
//!   For a tile drawn with [`crate::frame::ViewFrame::tile`], [`tile_mesh`] makes the mesh: the
//!   texture coordinates are `(u, v)` themselves.
//! - [`Bitmap`]: the pipeline for one draw-state preset (`SURFACE` for imagery and hillshade,
//!   `SURFACE_NO_CULL` for the rail raster), its sampler (deck's: linear, linear between mip levels,
//!   clamped to the edge), [`Texture`] bind groups (group 2) and [`Style`]s (group 3).
//! - [`Params`]: deck's `tintColor`, `desaturate`, `transparentColor` and the layer's `opacity`;
//!   the fragment is deck's, line for line.
//! - [`upload_rgba8`]: a decoded image as a texture with the mip chain deck generates for its
//!   images (a 2 × 2 box filter here).
//!
//! **Mercator-correct sampling on the globe.** deck draws these tiles with
//! `_imageCoordinateSystem: CARTESIAN`: on the globe its mesh is a grid of at most 5° in longitude
//! and latitude, the fragment shader interpolates longitude and latitude linearly across a triangle
//! and turns the latitude into Mercator per fragment. Here the mesh is in local Mercator and the
//! texture coordinates are interpolated linearly in Mercator, with no transcendental function on the
//! GPU (WGSL lets `log` and `tan` be far less precise than a tile's span needs). The two differ
//! inside a cell by `sin φ · Y² / 8` radians of Mercator, `Y` the cell's Mercator height: so
//! [`tile_cells`] cuts a tile finely enough that this stays under [`MERCATOR_TOLERANCE_PX`] at the
//! deepest view zoom that draws the tile, and never coarser than deck's 5° grid. On the flat map
//! both are exact.

use bytemuck::{Pod, Zeroable};

use super::fill::{Mesh, uniform_group, uniform_layout};
use super::{Desc, build};
use crate::frame::FrameSlot;
use crate::gpu::Gpu;
use crate::layers::{DrawCx, Init};
use crate::shaders;
use crate::state::DrawState;

/// `shaders/bitmap.wgsl`.
pub const SHADER: &str = include_str!("../shaders/bitmap.wgsl");

/// The bind group of a [`Texture`].
pub const TEXTURE_GROUP: u32 = 2;
/// The bind group of a [`Style`].
pub const STYLE_GROUP: u32 = 3;

/// The most a tile's image may be displaced against deck's per-fragment Mercator, in CSS pixels,
/// at the deepest view zoom that draws the tile ([`tile_cells`]).
pub const MERCATOR_TOLERANCE_PX: f64 = 0.25;

/// deck's globe tessellation step, degrees (`GlobeView({ resolution: 5 })`).
const RESOLUTION_DEG: f64 = 5.0;

/// One vertex of a bitmap mesh.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    /// `u` east and `v` south in the frame's local units, and the height in metres.
    pub position: [f32; 3],
    /// Texture coordinates, `(0, 0)` at the image's top left.
    pub tex: [f32; 2],
}

impl Vertex {
    /// The vertex buffer layout: position at location 0, texture coordinates at location 1.
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
    };
}

/// What a BitmapLayer is given besides its image (deck's defaults in [`Params::default`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// `tintColor`: multiplies the image's RGB (the imagery's is `[150,158,180]`).
    pub tint: [u8; 3],
    /// `desaturate`, 0–1: mixes the RGB toward its mean.
    pub desaturate: f32,
    /// `transparentColor`: what shows through where the image is transparent.
    pub transparent: [u8; 4],
    /// The layer's `opacity`.
    pub opacity: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params { tint: [255, 255, 255], desaturate: 0.0, transparent: [0, 0, 0, 0], opacity: 1.0 }
    }
}

/// `bitmap.wgsl`'s `Style`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct StyleUniform {
    pub tint: [f32; 3],
    pub desaturate: f32,
    pub transparent: [f32; 4],
    pub opacity: f32,
    pub pad: [f32; 3],
}

impl From<&Params> for StyleUniform {
    /// deck's `tintColor.slice(0, 3).map(x => x / 255)`, `transparentColor.map(x => x / 255)`.
    fn from(p: &Params) -> Self {
        StyleUniform {
            tint: p.tint.map(|c| f32::from(c) / 255.0),
            desaturate: p.desaturate,
            transparent: p.transparent.map(|c| f32::from(c) / 255.0),
            opacity: p.opacity,
            pad: [0.0; 3],
        }
    }
}

/// A layer's [`Params`] on the GPU (group 3), written whenever they change.
pub struct Style {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
}

impl Style {
    pub fn set(&self, queue: &wgpu::Queue, params: &Params) {
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&StyleUniform::from(params)));
    }
}

/// An image bound for drawing (group 2): a texture view with the pipeline's sampler.
pub struct Texture {
    group: wgpu::BindGroup,
}

/// The bitmap pipeline for one draw-state preset.
pub struct Bitmap {
    pipeline: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    style_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Bitmap {
    /// The pipeline with `state` (for the map pass of `init`).
    pub fn new(init: &Init<'_>, label: &str, state: DrawState) -> Self {
        let device = &init.gpu.device;
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bitmap texture"),
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
        });
        let style_layout = uniform_layout(device, "bitmap style", size_of::<StyleUniform>());
        let module = shaders::module(device, label, SHADER);
        let pipeline = build(
            init,
            &Desc {
                label,
                module: &module,
                vertex_entry: "vs",
                fragment_entry: "fs",
                buffers: &[Some(Vertex::LAYOUT)],
                topology: wgpu::PrimitiveTopology::TriangleList,
                state,
                groups: &[&texture_layout, &style_layout],
            },
        );
        // deck's DEFAULT_TEXTURE_PARAMETERS (`@deck.gl/core` utils/texture.js)
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bitmap"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Bitmap { pipeline, texture_layout, style_layout, sampler }
    }

    /// `view` bound with this pipeline's sampler (a filterable float texture, e.g. `Rgba8Unorm`).
    pub fn texture(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> Texture {
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bitmap texture"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        Texture { group }
    }

    /// A style for this pipeline.
    pub fn style(&self, device: &wgpu::Device, params: &Params) -> Style {
        let (buffer, group) = uniform_group(device, "bitmap style", &self.style_layout, &StyleUniform::from(params));
        Style { buffer, group }
    }

    /// Sets the pipeline and `style`; then bind a frame and a texture and draw meshes.
    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>, style: &Style) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(STYLE_GROUP, &style.group, &[]);
    }

    /// Draws `mesh` textured with `texture` in the local Mercator `frame`, styled by `style`.
    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        cx: &DrawCx<'_>,
        frame: FrameSlot,
        texture: &Texture,
        style: &Style,
        mesh: &Mesh<Vertex>,
    ) {
        self.bind(pass, style);
        cx.bind_frame(pass, frame);
        pass.set_bind_group(TEXTURE_GROUP, &texture.group, &[]);
        mesh.draw(pass);
    }
}

/// How many cells each way [`tile_mesh`] cuts a tile of zoom `z` into, for an image source of
/// `tile_size` CSS pixels a tile (256 for imagery, hillshade and the rail raster).
///
/// deck's tile zoom is `round(zoom + log2(512 / tile_size))` (plan, facts checked), so the deepest
/// view that draws a tile of zoom `z` has zoom `z − log2(512 / tile_size) + 0.5`. There a world unit
/// is about `2^zoom` pixels, and a cell of Mercator height `Y = 2π / (2^z · n)` radians is off
/// deck's sampling by up to `sin φ · Y² / 8` radians, `512 / 2π` world units a radian: `n` is the
/// least that keeps that under [`MERCATOR_TOLERANCE_PX`], and at least deck's `360 / 2^z / 5`
/// columns (whose rows then span at most 5° of latitude too). From zoom 11 on (256-pixel tiles) a
/// tile is one cell.
pub fn tile_cells(z: u32, tile_size: f64) -> u32 {
    let tiles = 2f64.powi(z as i32);
    let deepest_zoom = f64::from(z) - (512.0 / tile_size).log2() + 0.5;
    let error_one_cell = std::f64::consts::PI * 128.0 * 2f64.powf(deepest_zoom) / (tiles * tiles);
    let for_mercator = (error_one_cell / MERCATOR_TOLERANCE_PX).sqrt().ceil();
    let for_degrees = (360.0 / tiles / RESOLUTION_DEG).ceil();
    for_mercator.max(for_degrees).max(1.0) as u32
}

/// A tile's image quad as a grid of `cells × cells` (at least 1) in tile units: vertex `(u, v)` with
/// the texture coordinates `(u, v)`, height 0; two triangles a cell, counter-clockwise on the map.
/// Draw it with [`crate::frame::ViewFrame::tile`].
pub fn tile_mesh(cells: u32) -> (Vec<Vertex>, Vec<u32>) {
    let n = cells.max(1);
    let side = n + 1;
    let mut vertices = Vec::with_capacity((side * side) as usize);
    for j in 0..side {
        for i in 0..side {
            let (u, v) = (i as f32 / n as f32, j as f32 / n as f32);
            vertices.push(Vertex { position: [u, v, 0.0], tex: [u, v] });
        }
    }
    let mut indices = Vec::with_capacity((n * n * 6) as usize);
    for j in 0..n {
        for i in 0..n {
            let nw = j * side + i;
            let (ne, sw, se) = (nw + 1, nw + side, nw + side + 1);
            // south-west, south-east, north-east; south-west, north-east, north-west
            indices.extend([sw, se, ne, sw, ne, nw]);
        }
    }
    (vertices, indices)
}

/// An `Rgba8Unorm` texture of `rgba` (`size` pixels, rows top first), with a full mip chain made
/// by averaging 2 × 2 texels when `mipmaps` (deck generates one for every image it draws:
/// `getMipLevelCount(width, height)` levels).
pub fn upload_rgba8(gpu: &Gpu, label: &str, size: [u32; 2], rgba: &[u8], mipmaps: bool) -> wgpu::Texture {
    let [width, height] = [size[0].max(1), size[1].max(1)];
    assert_eq!(rgba.len(), (width * height * 4) as usize, "{label}: {width} × {height} RGBA8 needs {} bytes", width * height * 4);
    let levels = if mipmaps { 32 - width.max(height).leading_zeros() } else { 1 };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut level = rgba.to_vec();
    let [mut w, mut h] = [width, height];
    for mip in 0..levels {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: mip, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &level,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        if mip + 1 < levels {
            (level, w, h) = half_size(&level, w, h);
        }
    }
    texture
}

/// The next mip level: each texel the rounded mean of the 2 × 2 above it (the last row or column
/// repeated where a size is odd).
fn half_size(rgba: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let at = |x: u32, y: u32, c: usize| u32::from(rgba[((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize + c]);
    let mut out = Vec::with_capacity((nw * nh * 4) as usize);
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..4 {
                let sum = at(2 * x, 2 * y, c) + at(2 * x + 1, 2 * y, c) + at(2 * x, 2 * y + 1, c) + at(2 * x + 1, 2 * y + 1, c);
                out.push(((sum + 2) / 4) as u8);
            }
        }
    }
    (out, nw, nh)
}

#[cfg(test)]
mod tests {
    use glam::DVec2;
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::{ViewState, WORLD_SIZE, world_to_lng_lat};
    use rustradar_scene::tiles::TileIndex;

    use super::*;
    use crate::frame::tile_extent;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::{Layer, PrepareCx};
    use crate::pipelines::fill::tests::wgsl_members;
    use crate::renderer::{MapRenderer, SceneInput};

    #[test]
    fn layouts_match_the_wgsl() {
        assert_eq!(size_of::<Vertex>(), 20);
        assert_eq!(Vertex::LAYOUT.array_stride, 20);
        let names = ["tint", "desaturate", "transparent", "opacity", "pad0", "pad1", "pad2"];
        let offsets = [0, 12, 16, 32, 36, 40, 44];
        let wgsl: Vec<(String, u32)> = names.iter().zip(offsets).map(|(n, o)| (n.to_string(), o)).collect();
        assert_eq!(wgsl_members(SHADER, "Style"), (48, wgsl));
        assert_eq!(size_of::<StyleUniform>(), 48);
        assert_eq!(std::mem::offset_of!(StyleUniform, desaturate), 12);
        assert_eq!(std::mem::offset_of!(StyleUniform, transparent), 16);
        assert_eq!(std::mem::offset_of!(StyleUniform, opacity), 32);
        let imagery = StyleUniform::from(&Params { tint: [150, 158, 180], ..Params::default() });
        assert_eq!(imagery.tint, [150.0 / 255.0, 158.0 / 255.0, 180.0 / 255.0]);
        assert_eq!((imagery.desaturate, imagery.transparent, imagery.opacity), (0.0, [0.0; 4], 1.0));
    }

    #[test]
    fn a_tile_mesh_covers_the_tile_counter_clockwise() {
        for cells in [0, 1, 3, 72] {
            let (vertices, indices) = tile_mesh(cells);
            let n = cells.max(1) as usize;
            assert_eq!(vertices.len(), (n + 1) * (n + 1));
            assert_eq!(indices.len(), n * n * 6);
            assert!(vertices.iter().all(|v| v.tex == [v.position[0], v.position[1]] && v.position[2] == 0.0));
            let mut area = 0.0;
            for t in indices.chunks(3) {
                // on the map, north up: (u, −v)
                let p = |k: usize| {
                    let q = vertices[t[k] as usize].position;
                    [f64::from(q[0]), -f64::from(q[1])]
                };
                let (a, b, c) = (p(0), p(1), p(2));
                let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                assert!(cross > 0.0, "{cells}: {t:?} is not counter-clockwise on the map");
                area += cross / 2.0;
            }
            assert!((area - 1.0).abs() < 1e-9);
            let corners = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
            assert!(corners.iter().all(|c| vertices.iter().any(|v| v.tex == *c)));
        }
    }

    /// Mercator y (radians) of a latitude in degrees.
    fn mercator(lat: f64) -> f64 {
        (std::f64::consts::FRAC_PI_4 + lat.to_radians() / 2.0).tan().ln()
    }

    /// For every zoom and every row of every tile column, the texture's Mercator at the middle of
    /// each cell edge, as deck works it out (latitude interpolated, then Mercator) and as here
    /// (Mercator interpolated), differ by less than the tolerance at the deepest view zoom; and no
    /// cell is wider or taller than 5°.
    #[test]
    fn tile_cells_keep_the_image_where_deck_puts_it() {
        let expected = [72, 36, 18, 12, 9, 6, 5, 3, 3, 2, 2, 1, 1, 1];
        for z in 0..=19u32 {
            let n = tile_cells(z, 256.0);
            if let Some(&e) = expected.get(z as usize) {
                assert_eq!(n, e, "z{z}");
            } else {
                assert_eq!(n, 1, "z{z}");
            }
            let tiles = 2u32.pow(z);
            let deepest_zoom = f64::from(z) - 0.5;
            let px_per_radian = WORLD_SIZE / std::f64::consts::TAU * 2f64.powf(deepest_zoom);
            let mut worst = 0.0f64;
            // the rows of the northern tiles (the southern mirror them)
            for y in 0..tiles.div_ceil(2) {
                for row in 0..n {
                    let world_y = |r: u32| WORLD_SIZE - (f64::from(y) + f64::from(r) / f64::from(n)) * tile_extent(z);
                    let [_, north] = world_to_lng_lat([0.0, world_y(row)]);
                    let [_, south] = world_to_lng_lat([0.0, world_y(row + 1)]);
                    assert!(north - south <= RESOLUTION_DEG + 1e-9, "z{z}: a row of {}°", north - south);
                    let deck = mercator((north + south) / 2.0);
                    let here = (mercator(north) + mercator(south)) / 2.0;
                    worst = worst.max((deck - here).abs() * px_per_radian);
                }
            }
            assert!(worst <= MERCATOR_TOLERANCE_PX, "z{z}: {worst} px");
            assert!(360.0 / f64::from(tiles) / f64::from(n) <= RESOLUTION_DEG);
        }
        // 512-pixel tiles are drawn one zoom deeper
        assert!(tile_cells(5, 512.0) > tile_cells(5, 256.0));
    }

    #[test]
    fn mip_levels_average_two_by_two() {
        let (level, w, h) = half_size(&[0, 0, 0, 255, 255, 255, 255, 255, 10, 20, 30, 40, 10, 20, 30, 40], 2, 2);
        assert_eq!((w, h), (1, 1));
        // (0 + 255 + 10 + 10 + 2) / 4 and so on
        assert_eq!(level, [69, 74, 79, 148]);
        let (level, w, h) = half_size(&[100, 0, 0, 255, 200, 0, 0, 255, 50, 0, 0, 255], 3, 1);
        assert_eq!((w, h, level), (1, 1, vec![150, 0, 0, 255]));
    }

    /// A test layer: one image on one tile with a [`Bitmap`].
    struct TileImage {
        bitmap: Bitmap,
        texture: Texture,
        style: Style,
        mesh: Mesh<Vertex>,
        tile: TileIndex,
        slot: Option<FrameSlot>,
    }

    impl Layer for TileImage {
        fn name(&self) -> &'static str {
            "test tile image"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.slot = Some(cx.frames.push(&cx.view.tile(self.tile, 0.0)));
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            self.bitmap.draw(pass, cx, self.slot.expect("prepared"), &self.texture, &self.style, &self.mesh);
        }
    }

    /// `image` (2 × 2 texels, rows top first) on `tile` with `params`, seen from `view`.
    fn render_tile(gpu: &Gpu, view: ViewState, tile: TileIndex, image: [[u8; 4]; 4], params: Params) -> (Image, MapRenderer) {
        let scene = SceneInput::new(view, [300.0, 300.0], 1.0, Basemap::Dark);
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let init = renderer.init(gpu);
        let bitmap = Bitmap::new(&init, "test bitmap", DrawState::SURFACE);
        let texture = upload_rgba8(gpu, "test image", [2, 2], image.as_flattened(), false);
        let texture = bitmap.texture(&gpu.device, &texture.create_view(&Default::default()));
        let style = bitmap.style(&gpu.device, &params);
        let (vertices, indices) = tile_mesh(tile_cells(tile.z, 256.0));
        let mesh = Mesh::new(&gpu.device, "tile", &vertices, &indices);
        renderer.add_layer(Box::new(TileImage { bitmap, texture, style, mesh, tile, slot: None }));
        let image = render_with(gpu, &mut renderer, &scene, 1.0).expect("rendered");
        (image, renderer)
    }

    /// A view centred on the middle of `tile`.
    fn centred(tile: TileIndex) -> ViewState {
        let e = tile_extent(tile.z);
        let [longitude, latitude] = world_to_lng_lat([(f64::from(tile.x) + 0.5) * e, WORLD_SIZE - (f64::from(tile.y) + 0.5) * e]);
        ViewState { longitude, latitude, ..ViewState::INITIAL }
    }

    /// The pixel where `scene::camera` puts the point `(u, v)` of `tile` (tile units).
    fn at(renderer: &MapRenderer, tile: TileIndex, u: f64, v: f64) -> [f64; 2] {
        let e = tile_extent(tile.z);
        let w = DVec2::new((f64::from(tile.x) + u) * e, WORLD_SIZE - (f64::from(tile.y) + v) * e);
        let [lon, lat] = world_to_lng_lat(w.to_array());
        let [x, y, _] = renderer.view().viewport.project([lon, lat, 0.0]);
        [x, y]
    }

    /// The four texels land on the tile's four quarters, tinted as deck tints the imagery, on the
    /// flat map and on the globe; outside the tile the ink stays.
    #[test]
    fn a_tile_image_lands_on_its_tile_tinted() {
        let Some(gpu) = gpu() else { return };
        let red = [200, 40, 40, 255];
        let green = [40, 200, 40, 255];
        let blue = [40, 40, 200, 255];
        let white = [255, 255, 255, 255];
        let tint = Params { tint: [150, 158, 180], ..Params::default() };
        let tinted = |c: [u8; 4]| -> [u8; 4] {
            let t = [150.0, 158.0, 180.0];
            let m = |i: usize| (f64::from(c[i]) * t[i] / 255.0).round() as u8;
            [m(0), m(1), m(2), 255]
        };
        // Stockholm's z14 tile on the flat map, its z8 tile on the globe (pitched and turned), both
        // about 240 px wide
        for (tile, zoom, bearing, pitch) in [(TileIndex { x: 9014, y: 4817, z: 14 }, 12.9, 0.0, 0.0), (TileIndex { x: 140, y: 75, z: 8 }, 6.9, 20.0, 40.0)] {
            let view = ViewState { zoom, bearing, pitch, ..centred(tile) };
            let (image, renderer) = render_tile(&gpu, view, tile, [red, green, blue, white], tint);
            for (u, v, texel) in [(0.25, 0.25, red), (0.75, 0.25, green), (0.25, 0.75, blue), (0.75, 0.75, white)] {
                let [x, y] = at(&renderer, tile, u, v);
                let got = image.pixel(x as u32, y as u32);
                let want = tinted(texel);
                // a texel's centre is where linear filtering gives it alone
                assert!(got.iter().zip(want).all(|(g, w)| (i32::from(*g) - i32::from(w)).abs() <= 1), "{tile:?} ({u}, {v}): {got:?} against {want:?}");
            }
            for (u, v) in [(-0.05, 0.5), (1.05, 0.5), (0.5, -0.05), (0.5, 1.05)] {
                let [x, y] = at(&renderer, tile, u, v);
                assert_eq!(image.pixel(x as u32, y as u32), [30, 30, 30, 255], "{tile:?} outside at ({u}, {v})");
            }
        }
    }

    /// On the globe the image follows Mercator, not latitude: the line between a tile's top and
    /// bottom halves lies at the latitude of the tile's middle Mercator y (55.78° on this z3 tile),
    /// not at its middle latitude (53.75°).
    #[test]
    fn on_the_globe_the_image_follows_mercator() {
        let Some(gpu) = gpu() else { return };
        let tile = TileIndex { x: 4, y: 2, z: 3 };
        let view = ViewState { zoom: 1.6, ..centred(tile) };
        let top = [250, 250, 250, 255];
        let bottom = [10, 10, 10, 255];
        let (image, renderer) = render_tile(&gpu, view, tile, [top, top, bottom, bottom], Params::default());
        // down the tile's middle meridian: where the brightness crosses half way, sub-pixel
        let [x, y0] = at(&renderer, tile, 0.5, 0.25);
        let [_, y1] = at(&renderer, tile, 0.5, 0.75);
        let column = x.floor() as u32;
        let level = |y: u32| f64::from(image.pixel(column, y)[0]);
        let mid = (250.0 + 10.0) / 2.0;
        let mut crossing = None;
        for y in y0.ceil() as u32..y1.floor() as u32 {
            let (a, b) = (level(y), level(y + 1));
            if a >= mid && b < mid {
                crossing = Some(f64::from(y) + 0.5 + (a - mid) / (a - b));
            }
        }
        let crossing = crossing.expect("the halves meet");
        let [_, mercator_middle] = at(&renderer, tile, 0.5, 0.5);
        let e = tile_extent(tile.z);
        let [_, north] = world_to_lng_lat([0.0, WORLD_SIZE - f64::from(tile.y) * e]);
        let [_, south] = world_to_lng_lat([0.0, WORLD_SIZE - f64::from(tile.y + 1) * e]);
        let [_, latitude_middle, _] = renderer.view().viewport.project([22.5, (north + south) / 2.0, 0.0]);
        assert!((crossing - mercator_middle).abs() < 1.0, "the halves meet at y {crossing}, Mercator's middle is at {mercator_middle}");
        assert!((latitude_middle - mercator_middle).abs() > 10.0, "the test cannot tell the two apart");
    }

    /// deck's fragment: desaturate, tint, opacity and the transparent colour behind.
    #[test]
    fn the_style_is_decks() {
        let Some(gpu) = gpu() else { return };
        let tile = TileIndex { x: 9014, y: 4817, z: 14 };
        let view = ViewState { zoom: 12.9, ..centred(tile) };
        let c = [200, 100, 0, 255];
        let (image, renderer) = render_tile(&gpu, view, tile, [c; 4], Params { desaturate: 0.5, opacity: 0.5, ..Params::default() });
        let [x, y] = at(&renderer, tile, 0.5, 0.5);
        // mix with the mean 100 halfway, then half over the ink: (150, 100, 50)/2 + 15
        let got = image.pixel(x as u32, y as u32);
        assert!(got.iter().zip([90, 65, 40, 255]).all(|(g, w)| (i32::from(*g) - w).abs() <= 1), "{got:?}");
        let clear = [0, 0, 0, 0];
        let (image, _) = render_tile(&gpu, view, tile, [clear; 4], Params { transparent: [0, 0, 255, 255], ..Params::default() });
        assert_eq!(image.pixel(x as u32, y as u32), [0, 0, 255, 255]);
    }
}
