//! The 3D surface's tiles, from bytes to the GPU (item Z1; doc 04 §14.1, §14.3): each height tile
//! decoded on the worker pool into deck.gl's `TerrainLayer` mesh, uploaded, and drawn with
//! `shaders/terrain_surface.wgsl`.
//!
//! - **Decode** ([`decode`], `getTiledTerrainData` → `TerrainLoader`): the bytes are Mapterhorn's
//!   512 px WebP or, where it has none, Mapzen's 256 px PNG (the app's fetcher stands one in for the
//!   other, `backend::terrain`), read to exact pixels by the `image` crate (as `createImageBitmap`
//!   with no colour conversion); then `scene::terrain::martini::terrain_mesh` with what deck's
//!   layer hands the loader: the terrarium decoder, `meshMaxError` 2 m
//!   ([`effective_mesh_max_error`]), the 4 m skirt of a tiled layer ([`skirt_height`]) and the tile's
//!   bounds grown by one pixel of the layer's 512 px `tileSize` on every side
//!   ([`overlapped_bounds`]). The vertices go into the tile's local Mercator frame: `u` east and `v`
//!   south in tile units, the height in metres ([`SurfaceMesh`]).
//! - **Upload** ([`SurfaceGpu::new`]): one vertex and one index buffer a tile.
//! - **Draw** ([`SurfacePipeline`]): `NO_CULL` (deck's default parameters: depth tested and
//!   written, both faces), the sea-level clamp in the vertex shader, the colour `MESH_COLOR` lit by
//!   the scene's rig with luma's default Phong material per fragment and flat-shaded normals.
//!
//! Deviations:
//! - On the globe deck places the mesh in degrees (`projectFlat` is the identity there, so the bounds
//!   are longitudes and latitudes and each row of the image is spaced evenly in latitude, clamped to
//!   ±180° and ±90°); here every tile is in its Web Mercator frame on both projections, as its image
//!   is, so its rows land where they belong on the globe too.
//! - A tile that is not square with a power-of-two side would go to loaders.gl's Delatin, which is
//!   not ported: it fails to decode (neither source sends one).

use std::sync::Arc;

use bytemuck::Zeroable;
use rustradar_scene::hillshade::{MESH_COLOR, TerrainSurface};
use rustradar_scene::terrain::heights::{GRID_SIDE, HeightGrid};
use rustradar_scene::terrain::martini::{
    TerrainMesh, TerrainOptions, effective_mesh_max_error, overlapped_bounds, skirt_height, terrain_mesh,
};

use crate::frame::FrameSlot;
use crate::gpu::Gpu;
use crate::jobs::Cancel;
use crate::layers::{ByteSize, DrawCx, Init, PrepareCx, TileIndex};
use crate::pipelines::mesh::{LightsUniform, Material, StyleUniform};
use crate::pipelines::{Desc, build};
use crate::shaders;
use crate::state::DrawState;

/// `shaders/terrain_surface.wgsl`.
pub const SHADER: &str = include_str!("../../shaders/terrain_surface.wgsl");

/// The bind group of the lights (binding 0) and the surface's style (binding 1).
pub const STYLE_GROUP: u32 = 2;

/// The layer's `tileSize`: the pixels a tile's bounds are grown by are of this size, whatever the
/// image's (`getOverlappedBounds(bounds, this.props.tileSize, …)`).
pub const TILE_SIZE: f64 = 512.0;

/// A tile's bounds as the loader gets them, in tile units with y north from the tile's south edge:
/// `[0, 0, 1, 1]` grown by one pixel of [`TILE_SIZE`] on every side (deck's common-space bounds
/// on the flat map, `getOverlappedBounds`, measured in the tile).
pub fn tile_bounds() -> [f64; 4] {
    overlapped_bounds([0.0, 0.0, 1.0, 1.0], TILE_SIZE, false)
}

/// The loader's options for the layer `surface` (`loadTerrain` for a tiled layer).
pub fn loader_options(surface: &TerrainSurface) -> TerrainOptions {
    TerrainOptions {
        mesh_max_error: effective_mesh_max_error(surface.mesh_max_error),
        bounds: Some(tile_bounds()),
        elevation_decoder: surface.decoder,
        skirt_height: skirt_height(surface.mesh_max_error, true),
    }
}

/// A height tile's mesh in its local Mercator frame, made on the worker pool.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceMesh {
    /// `u` east and `v` south in tile units (a pixel of overlap past 0 and 1), the height in metres
    /// (the sea floor's below 0: the shader raises it).
    pub vertices: Vec<[f32; 3]>,
    /// Each vertex's slope, metres of height per tile unit east and south, averaged over the
    /// triangles round it (a skirt's vertex takes its top's): the shaders light the surface smooth
    /// from it.
    pub slopes: Vec<[f32; 2]>,
    /// Three a triangle: the surface's, then the skirt's.
    pub indices: Vec<u32>,
    /// The ground's lowest and highest heights in the tile, metres: the mesh's bounding box before
    /// the skirt (`header.boundingBox`), which the layer's elevation range grows from.
    pub z_range: [f64; 2],
    /// The image's side, pixels (512 for Mapterhorn, 256 for Mapzen).
    pub size: u32,
    /// The RTIN's own triangles, without the skirt's.
    pub surface_triangles: usize,
    /// The surface as drawn, on a grid (item Z3): what the 3D layers stand on.
    pub heights: Arc<HeightGrid>,
}

impl ByteSize for SurfaceMesh {
    fn byte_size(&self) -> usize {
        self.vertices.len() * size_of::<[f32; 5]>() + self.indices.len() * size_of::<u32>() + self.heights.byte_size()
    }
}

impl SurfaceMesh {
    /// The loader's mesh (positions within [`tile_bounds`], y north) in the tile's frame.
    pub fn from_loader(mesh: &TerrainMesh, size: u32) -> Self {
        let vertices: Vec<[f32; 3]> = mesh.attributes.positions.chunks_exact(3).map(|p| [p[0], (1.0 - f64::from(p[1])) as f32, p[2]]).collect();
        let [low, high] = mesh.bounding_box;
        // (Z3: the drawn surface, raised to the sea as its shader raises it, on a grid)
        let heights = Arc::new(HeightGrid::from_mesh(&vertices, &mesh.indices, mesh.surface_triangles, GRID_SIDE));
        let slopes = slopes(&vertices, &mesh.indices, mesh.surface_triangles);
        SurfaceMesh { vertices, slopes, indices: mesh.indices.clone(), z_range: [low[2], high[2]], size, surface_triangles: mesh.surface_triangles, heights }
    }
}

/// Each vertex's slope ([`SurfaceMesh::slopes`]): every surface triangle's plane, weighted by its
/// area, with the heights raised to the sea as the shaders raise them; a vertex no surface triangle
/// holds (the skirt's) takes the slope of the surface vertex at its place.
pub fn slopes(vertices: &[[f32; 3]], indices: &[u32], surface_triangles: usize) -> Vec<[f32; 2]> {
    let mut sums = vec![[0.0f64; 3]; vertices.len()];
    let at = |i: u32| {
        let [u, v, h] = vertices[i as usize];
        [f64::from(u), f64::from(v), f64::from(h.max(0.0))]
    };
    for t in indices.chunks_exact(3).take(surface_triangles) {
        let [a, b, c] = [at(t[0]), at(t[1]), at(t[2])];
        let (e1, e2) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let det = e1[0] * e2[1] - e2[0] * e1[1];
        if det.abs() < 1e-12 {
            continue;
        }
        let gu = (e1[2] * e2[1] - e2[2] * e1[1]) / det;
        let gv = (e1[0] * e2[2] - e2[0] * e1[2]) / det;
        let w = det.abs();
        for &i in t {
            let s = &mut sums[i as usize];
            s[0] += gu * w;
            s[1] += gv * w;
            s[2] += w;
        }
    }
    let key = |p: &[f32; 3]| (p[0].to_bits(), p[1].to_bits());
    let mut on_surface = std::collections::HashMap::new();
    for (i, s) in sums.iter().enumerate() {
        if s[2] > 0.0 {
            on_surface.insert(key(&vertices[i]), i);
        }
    }
    (0..vertices.len())
        .map(|i| {
            let s = if sums[i][2] > 0.0 { sums[i] } else { on_surface.get(&key(&vertices[i])).map_or([0.0; 3], |&j| sums[j]) };
            if s[2] > 0.0 { [(s[0] / s[2]) as f32, (s[1] / s[2]) as f32] } else { [0.0; 2] }
        })
        .collect()
}

/// The decode step, on the worker pool: a height tile's bytes to its mesh. `None` for no bytes; an
/// error for bytes that are no picture, a picture martini cannot take, or once cancelled.
pub fn decode(_tile: TileIndex, bytes: &[u8], cancel: &Cancel, options: &TerrainOptions) -> Result<Option<SurfaceMesh>, String> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if cancel.is_cancelled() {
        return Err("cancelled".into());
    }
    let image = image::load_from_memory(bytes).map_err(|e| format!("height tile: {e}"))?.into_rgba8();
    if cancel.is_cancelled() {
        return Err("cancelled".into());
    }
    let (width, height) = image.dimensions();
    let mesh = terrain_mesh(image.as_raw(), width as usize, height as usize, options)?;
    Ok(Some(SurfaceMesh::from_loader(&mesh, width)))
}

/// The vertex buffer's contents: each vertex's place and slope.
fn interleaved(mesh: &SurfaceMesh) -> Vec<[f32; 5]> {
    mesh.vertices.iter().zip(&mesh.slopes).map(|(&[u, v, h], &[su, sv])| [u, v, h, su, sv]).collect()
}

/// A tile's mesh on the GPU.
pub struct SurfaceGpu {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    elements: u32,
    z_range: [f64; 2],
    heights: Arc<HeightGrid>,
}

impl SurfaceGpu {
    /// The upload step, on the main thread.
    pub fn new(gpu: &Gpu, mesh: &SurfaceMesh) -> Self {
        let buffer = |label, contents: &[u8], usage| {
            wgpu::util::DeviceExt::create_buffer_init(&gpu.device, &wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
        };
        SurfaceGpu {
            vertices: buffer("terrain vertices", bytemuck::cast_slice(&interleaved(mesh)), wgpu::BufferUsages::VERTEX),
            indices: buffer("terrain indices", bytemuck::cast_slice(&mesh.indices), wgpu::BufferUsages::INDEX),
            elements: mesh.indices.len() as u32,
            z_range: mesh.z_range,
            heights: Arc::clone(&mesh.heights),
        }
    }

    /// The surface as drawn, on a grid (item Z3).
    pub fn heights(&self) -> &Arc<HeightGrid> {
        &self.heights
    }

    /// The vertex buffer: `[f32; 5]` a vertex, (`u`, `v`, metres) at location 0 and its slope at
    /// location 1.
    pub fn vertices(&self) -> &wgpu::Buffer {
        &self.vertices
    }

    /// The `u32` index buffer.
    pub fn indices(&self) -> &wgpu::Buffer {
        &self.indices
    }

    /// Indices to draw.
    pub fn elements(&self) -> u32 {
        self.elements
    }

    /// The ground's lowest and highest heights in the tile, metres.
    pub fn z_range(&self) -> [f64; 2] {
        self.z_range
    }

    /// Draws the mesh with the pipeline and frame bound.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.elements, 0, 0..1);
    }
}

/// The surface's vertex layout: the place at location 0, the slope at location 1.
pub const VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: size_of::<[f32; 5]>() as u64,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
};

/// The surface's colour and material: `MESH_COLOR`, opaque, luma's default Phong material (deck's
/// `material: false`), opacity 1.
pub fn style_uniform() -> StyleUniform {
    let [r, g, b] = MESH_COLOR;
    StyleUniform::new(&Material::DECK_DEFAULT, 1.0, [r, g, b, 255])
}

/// `shaders/terrain_surface.wgsl`'s pipeline with its lights and style.
pub struct SurfacePipeline {
    pipeline: wgpu::RenderPipeline,
    lights: wgpu::Buffer,
    group: wgpu::BindGroup,
}

impl SurfacePipeline {
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
            label: Some("terrain lights and style"),
            entries: &[uniform(0, size_of::<LightsUniform>()), uniform(1, size_of::<StyleUniform>())],
        });
        let module = shaders::module(device, "terrain surface", SHADER);
        let pipeline = build(
            init,
            &Desc {
                label: "terrain surface",
                module: &module,
                vertex_entry: "vs",
                fragment_entry: "fs",
                buffers: &[Some(VERTEX_LAYOUT)],
                topology: wgpu::PrimitiveTopology::TriangleList,
                state: DrawState::NO_CULL,
                groups: &[&layout],
            },
        );
        let init_buffer = |label, contents: &[u8]| {
            wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST },
            )
        };
        let lights = init_buffer("terrain lights", bytemuck::bytes_of(&LightsUniform::zeroed()));
        let style = init_buffer("terrain style", bytemuck::bytes_of(&style_uniform()));
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain lights and style"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: lights.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: style.as_entire_binding() },
            ],
        });
        SurfacePipeline { pipeline, lights, group }
    }

    /// This frame's lights: the scene's rig, turned for this frame's camera.
    pub fn prepare(&self, cx: &PrepareCx<'_>) {
        cx.gpu.queue.write_buffer(&self.lights, 0, bytemuck::bytes_of(&LightsUniform::new(cx.lights(), cx.view)));
    }

    /// Draws each tile in its frame.
    pub fn draw<'t>(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, tiles: impl IntoIterator<Item = (FrameSlot, &'t SurfaceGpu)>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(STYLE_GROUP, &self.group, &[]);
        for (slot, tile) in tiles {
            cx.bind_frame(pass, slot);
            tile.draw(pass);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipelines::fill::tests::wgsl_members;
    use crate::pipelines::mesh;
    use rustradar_scene::hillshade::{TERRARIUM_DECODER, terrain_surface};

    /// Terrarium PNG pixels for heights in whole metres.
    pub(crate) fn terrarium_png(size: u32, metres: impl Fn(u32, u32) -> f64) -> Vec<u8> {
        let image = image::RgbImage::from_fn(size, size, |x, y| {
            let v = ((metres(x, y) + 32768.0) * 256.0).round() as u32;
            image::Rgb([(v >> 16) as u8, (v >> 8) as u8, v as u8])
        });
        let mut png = Vec::new();
        image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("a PNG");
        png
    }

    /// A plane's slope everywhere on it, its skirt's vertices taking their tops'; the sea flat.
    #[test]
    fn slopes_are_the_planes_and_the_skirt_takes_its_tops() {
        let h = |u: f32, v: f32| 100.0 * u + 50.0 * v + 10.0;
        let mut vertices: Vec<[f32; 3]> = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]].iter().map(|&[u, v]| [u, v, h(u, v)]).collect();
        // a skirt vertex under the first corner
        vertices.push([0.0, 0.0, h(0.0, 0.0) - 4.0]);
        let indices = vec![0, 1, 2, 1, 3, 2, 0, 4, 1];
        for s in slopes(&vertices, &indices, 2) {
            assert!((s[0] - 100.0).abs() < 1e-3 && (s[1] - 50.0).abs() < 1e-3, "{s:?}");
        }
        let sea: Vec<[f32; 3]> = vertices.iter().map(|&[u, v, _]| [u, v, -5.0]).collect();
        assert!(slopes(&sea, &indices, 2).iter().all(|s| *s == [0.0, 0.0]));
    }

    /// The shader's uniforms are `mesh.rs`'s, member for member.
    #[test]
    fn the_shader_takes_the_mesh_pipelines_uniforms() {
        for name in ["Lights", "Style"] {
            assert_eq!(wgsl_members(SHADER, name), wgsl_members(mesh::SHADER, name), "{name}");
        }
        assert_eq!(VERTEX_LAYOUT.array_stride, 20);
        let style = style_uniform();
        assert_eq!(style.color, [34.0 / 255.0, 39.0 / 255.0, 48.0 / 255.0, 1.0]);
        assert_eq!((style.ambient, style.diffuse, style.shininess, style.unlit), (0.35, 0.6, 32.0, 0));
    }

    /// What deck's layer hands the loader: 2 m, the 4 m skirt, a pixel of 512 of overlap.
    #[test]
    fn the_loader_gets_decks_options() {
        let options = loader_options(&terrain_surface(&[]));
        assert_eq!(options.mesh_max_error, 2.0);
        assert_eq!(options.skirt_height, 4.0);
        let pad = 1.0 / 512.0;
        assert_eq!(options.bounds, Some([-pad, -pad, 1.0 + pad, 1.0 + pad]));
        assert_eq!(options.elevation_decoder, TERRARIUM_DECODER);
    }

    /// A Mapzen-sized tile sloping down to the south-east decodes to its mesh in the tile's frame:
    /// the north-west corner at `(−1/512, −1/512)` holds the first pixel's height, the south-east at
    /// `(1 + 1/512)²` the last's (back-filled), the skirt 4 m below; the range is the ground's.
    #[test]
    fn a_tile_decodes_into_its_frame() {
        let png = terrarium_png(256, |x, y| 3000.0 - 4.0 * f64::from(x) - 2.0 * f64::from(y));
        let options = loader_options(&terrain_surface(&[]));
        let mesh = decode(TileIndex::new(1, 2, 3), &png, &Cancel::default(), &options).expect("decoded").expect("a mesh");
        assert_eq!(mesh.size, 256);
        let pad = 1.0f32 / 512.0;
        let at = |u: f32, v: f32| mesh.vertices.iter().find(|p| (p[0] - u).abs() < 1e-6 && (p[1] - v).abs() < 1e-6).map(|p| p[2]);
        assert_eq!(at(-pad, -pad), Some(3000.0));
        assert_eq!(at(1.0 + pad, 1.0 + pad), Some(3000.0 - 4.0 * 255.0 - 2.0 * 255.0));
        assert_eq!(mesh.z_range, [3000.0 - 4.0 * 255.0 - 2.0 * 255.0, 3000.0]);
        // a plane: a handful of triangles, and the skirt hangs 4 m under the edges
        assert!(mesh.surface_triangles < 64, "{} triangles", mesh.surface_triangles);
        assert!(mesh.indices.len() > mesh.surface_triangles * 3);
        let lowest = mesh.vertices.iter().map(|p| p[2]).fold(f32::INFINITY, f32::min);
        assert_eq!(lowest, 3000.0 - 4.0 * 255.0 - 2.0 * 255.0 - 4.0);
        assert!(mesh.byte_size() > 0);
        // nothing, no picture, a picture martini cannot take
        assert_eq!(decode(TileIndex::ROOT, &[], &Cancel::default(), &options), Ok(None));
        assert!(decode(TileIndex::ROOT, b"not a picture", &Cancel::default(), &options).is_err());
        let odd = image::RgbImage::from_pixel(300, 300, image::Rgb([128, 0, 0]));
        let mut png = Vec::new();
        odd.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("a PNG");
        assert!(decode(TileIndex::ROOT, &png, &Cancel::default(), &options).is_err());
    }

    /// A committed Mapterhorn tile (lossless WebP, 512 px) north of Zermatt decodes to a mesh whose
    /// heights are the Mattertal's slopes (2249 to 3216 m).
    #[test]
    fn a_fixture_tile_decodes() {
        let path = crate::io::fixture_tiles_dir().join("mapterhorn/14/8544/5824.webp");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skipped: no {}", path.display());
            return;
        };
        let options = loader_options(&terrain_surface(&[]));
        let mesh = decode(TileIndex::new(8544, 5824, 14), &bytes, &Cancel::default(), &options).expect("decoded").expect("a mesh");
        assert_eq!(mesh.size, 512);
        eprintln!("{} triangles, heights {:?}", mesh.surface_triangles, mesh.z_range);
        assert!(mesh.z_range[0] > 1000.0 && mesh.z_range[1] < 4700.0 && mesh.z_range[1] - mesh.z_range[0] > 500.0, "{:?}", mesh.z_range);
    }
}
