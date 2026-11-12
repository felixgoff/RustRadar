//! The lifted RainViewer layers' GPU arenas (item W2; doc 02 §3.5 "GPU arenas"; `radar-arena.ts`,
//! and `arenas`, `liftedArenas` and `freeArenas` of G:1226-1300).
//!
//! Each height of the lifted stack (item W3) is one layer over every tile in view, so the stack
//! blends in height order across tile borders: one layer per tile would draw a seam where two
//! tiles' stacks meet. Merging the tiles afresh whenever the view brought in or dropped a tile
//! re-uploaded every tile at every height (some 40 MB for a stormy tilted view, several times a
//! second while panning), so each height's layer draws from buffers it owns and edits in place: a
//! tile coming into view is appended, one leaving has its triangles collapsed onto vertex 0, and
//! only when the buffers fill up, or would fit in far smaller ones, or hold nothing, are they packed
//! afresh, [`GROWTH`] (2) times what they hold and at least [`MIN_VERTICES`] (65 536) vertices and
//! three times as many indices.
//!
//! - **The bookkeeping** is `scene::radar::arena::MeshArena` (S1.27, the port of `MeshArena` with
//!   its tests). [`GpuDevice`] is its GPU side: wgpu buffers, which start zeroed as WebGL's do (so
//!   indices not yet written draw nothing), written in part with `queue.write_buffer`, released at
//!   a repack. [`GpuArena`] is one layer's arena with its draw (`ArenaLayer`: the indices in use,
//!   positions and colours in two vertex buffers).
//! - **The arenas** ([`Arenas`]): a slices arena (2 values a vertex) and a walls arena (3) for each
//!   height of `RADAR_LEVELS_KM`, made on first use (`arenas ??= …`) and filled, whenever the tiles
//!   in view have a new version (`arenasVersion !== radarVersion`), with each tile in view that
//!   reaches the height: its slices and walls from that level's band up, in the order of the tiles
//!   in view (`liftedArenas`). [`Arenas::free`] drops them (`freeArenas`: weather off, no
//!   RainViewer, or looking straight down), and the next update starts afresh.
//! - **One frame for all tiles.** An arena draws every tile in view in one call, so its vertices
//!   cannot be in each tile's own frame (`RadarTile::slice_vertices`): they are in the world's
//!   ([`ANCHOR`], [`SCALE`]: `u` east and `v` south from the world's north-west corner, 512 world
//!   units a unit), made once per tile while it is in view ([`world_mesh`], from the tile's own
//!   vertices in `f64`). The arenas take those meshes as today's took `tile.slices` and `tile.walls`
//!   in degrees, and [`frame`] places them, so the vertex shader is `radar.wgsl`'s, unchanged: a
//!   slice at its height through the frame's `z_offset`, a wall's unit height stretched by the
//!   style's `z_scale` from its foot.
//!
//! Deviations:
//! - deck drew the arenas' `f32` longitudes and latitudes; here each vertex is an `f32` of the
//!   world's frame, good to about 3·10⁻⁵ world units: under 0.15 px up to zoom 12, a pixel at zoom
//!   15 and 8 at zoom 18, on cells a kilometre or more across, and the same for every tile, so
//!   seams still meet. The ground layer ([`super::ground`]) keeps each tile's own frame.
//! - The arenas' frame is the world's one copy, as deck's single copy of longitudes: on the flat map
//!   a lifted layer across the antimeridian shows only on the side of the view centre's world copy,
//!   as today. On the globe every vertex is placed by its own longitude difference.
//! - Buffers given up at a repack or by [`Arenas::free`] are dropped (wgpu frees them once the GPU
//!   is done with them) rather than destroyed at once.
//! - A tile's arena meshes are kept while it is in view, besides its own meshes (today the arenas
//!   read `tile.slices` and `tile.walls` themselves): 23 to 28 bytes a vertex while lifted.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use glam::DVec2;
use rustradar_scene::camera::WORLD_SIZE;
use rustradar_scene::radar::arena::{ArenaDevice, ArenaPiece, BufferUsage, MeshArena};
pub use rustradar_scene::radar::arena::{GROWTH, MIN_VERTICES, capacity_for};
use rustradar_scene::radar::{RADAR_LEVELS_KM, RadarMesh};

use super::measured::{SHADER, STYLE_GROUP, StyleUniform};
use super::tiles::RadarTile;
use crate::frame::{Frame, FrameSlot, ViewFrame};
use crate::gpu::Gpu;
use crate::layers::{DrawCx, Init, TileIndex};
use crate::pipelines::fill::{self, uniform_group, uniform_layout};
use crate::pipelines::{Desc, build};
use crate::shaders;
use crate::state::DrawState;

/// The arenas' frame: the world's north-west corner in world Mercator (y north).
pub const ANCHOR: DVec2 = DVec2::new(0.0, WORLD_SIZE);

/// World units per unit of the arenas' frame: the whole world is `[0, 1]` in `u` and `v`.
pub const SCALE: f64 = WORLD_SIZE;

/// The frame an arena is drawn in this frame, raised by `z_offset_m` metres (a slice's height, or a
/// wall's foot).
pub fn frame(view: &ViewFrame, z_offset_m: f64) -> Frame {
    view.local(ANCHOR, SCALE, z_offset_m)
}

/// A tile's `mesh` (its slices or its walls) in the arenas' frame, from `vertices`, the same mesh in
/// the tile's own frame (`RadarTile::slice_vertices` or `wall_vertices`): `u` and `v` moved from
/// tile units to the world's in `f64`, the walls' unit height (`h`) as it is; the colours, triangles
/// and band starts the mesh's own.
pub fn world_mesh(tile: TileIndex, mesh: &RadarMesh, vertices: &[fill::Vertex]) -> RadarMesh {
    let size = mesh.size;
    let tiles = 2f64.powi(tile.z as i32);
    let (x, y) = (f64::from(tile.x), f64::from(tile.y));
    let mut positions = Vec::with_capacity(vertices.len() * size);
    for v in vertices {
        let [u, v, h] = v.position;
        positions.push(((x + f64::from(u)) / tiles) as f32);
        positions.push(((y + f64::from(v)) / tiles) as f32);
        if size > 2 {
            positions.push(h);
        }
    }
    RadarMesh {
        size,
        positions,
        colors: mesh.colors.clone(),
        indices: mesh.indices.clone(),
        band_starts: mesh.band_starts,
        band_vertex_starts: mesh.band_vertex_starts,
    }
}

/// A tile as an arena knows it: by identity, as the TypeScript's `Map` keyed by the tile object, so
/// a tile decoded again (for a new coverage) is a new piece. Holding it keeps its address from being
/// reused while it is in an arena.
#[derive(Clone)]
pub struct TileKey(pub Arc<RadarTile>);

impl PartialEq for TileKey {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for TileKey {}

impl Hash for TileKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

impl std::fmt::Debug for TileKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TileKey({})", self.0.mesh.id)
    }
}

/// Usages tests add to the arenas' buffers, to read them back.
const READ_BACK: wgpu::BufferUsages = if cfg!(test) { wgpu::BufferUsages::COPY_SRC } else { wgpu::BufferUsages::empty() };

/// The GPU side of a [`MeshArena`]: wgpu buffers, zeroed when made (a buffer not mapped at
/// creation is), written in part through the queue.
pub struct GpuDevice<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    written: usize,
}

impl<'a> GpuDevice<'a> {
    pub fn new(gpu: &'a Gpu) -> Self {
        GpuDevice { device: &gpu.device, queue: &gpu.queue, written: 0 }
    }

    /// Bytes written so far.
    pub fn written(&self) -> usize {
        self.written
    }
}

impl ArenaDevice for GpuDevice<'_> {
    type Buffer = wgpu::Buffer;

    fn create_buffer(&mut self, usage: BufferUsage, byte_length: usize) -> wgpu::Buffer {
        let usage = match usage {
            BufferUsage::Vertex => wgpu::BufferUsages::VERTEX,
            BufferUsage::Index => wgpu::BufferUsages::INDEX,
        } | wgpu::BufferUsages::COPY_DST
            | READ_BACK;
        self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("radar arena"), size: byte_length as u64, usage, mapped_at_creation: false })
    }

    fn write(&mut self, buffer: &wgpu::Buffer, byte_offset: usize, data: &[u8]) {
        // every write is whole f32s, u32s or RGBA quadruples at a multiple of 4: what wgpu asks
        if data.is_empty() {
            return;
        }
        self.queue.write_buffer(buffer, byte_offset as u64, data);
        self.written += data.len();
    }

    fn destroy(&mut self, buffer: wgpu::Buffer) {
        // freed once the GPU is done with it
        drop(buffer);
    }
}

/// One lifted layer's buffers on the GPU (`MeshArena` with its `ArenaLayer`): positions (`size`
/// `f32`s a vertex, in the arenas' frame) and RGBA colours in two vertex buffers, `u32` indices.
pub struct GpuArena {
    arena: MeshArena<TileKey, wgpu::Buffer>,
    size: usize,
}

impl std::fmt::Debug for GpuArena {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuArena")
            .field("size", &self.size)
            .field("capacity", &self.capacity())
            .field("index_count", &self.index_count())
            .field("version", &self.version())
            .finish()
    }
}

impl GpuArena {
    /// An empty arena for meshes of `size` values a vertex: 2 for slices, 3 for walls.
    pub fn new(size: usize) -> Self {
        GpuArena { arena: MeshArena::new(size), size }
    }

    /// Values a vertex.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Makes the buffers hold exactly `pieces` (each tile's mesh from a band up, in this order),
    /// uploading only what is new to them (`update`). The bytes written.
    pub fn update<'m>(&mut self, gpu: &Gpu, pieces: impl IntoIterator<Item = (TileKey, ArenaPiece<'m>)>) -> usize {
        // in order and keyed, as the JavaScript `Map` was
        let pieces = pieces.into_iter().collect();
        let mut device = GpuDevice::new(gpu);
        self.arena.update(&mut device, &pieces);
        device.written()
    }

    /// Whether it has buffers (`data`): nothing to draw without.
    pub fn has_data(&self) -> bool {
        self.arena.data().is_some()
    }

    /// Indices in use, up to the last piece appended: what it draws.
    pub fn index_count(&self) -> usize {
        self.arena.index_count()
    }

    /// Moves on with every change to the buffers.
    pub fn version(&self) -> u64 {
        self.arena.version()
    }

    /// Which allocation of buffers it holds (a new one at each reallocation).
    pub fn allocation(&self) -> Option<u64> {
        self.arena.data().map(|d| d.id)
    }

    /// The buffers' room, vertices and indices.
    pub fn capacity(&self) -> Option<(usize, usize)> {
        self.arena.data().map(|d| (d.vertex_capacity, d.index_capacity))
    }

    /// Sets its buffers (positions in vertex slot 0, colours in 1, the indices) and draws the
    /// indices in use, with the pipeline and bind groups already set.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some(data) = self.arena.data() else { return };
        let count = self.arena.index_count() as u32;
        if count == 0 {
            return;
        }
        pass.set_vertex_buffer(0, data.positions.slice(..));
        pass.set_vertex_buffer(1, data.colors.slice(..));
        pass.set_index_buffer(data.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..count, 0, 0..1);
    }

    /// The buffers themselves (tests read them back).
    #[cfg(test)]
    fn buffers(&self) -> Option<&rustradar_scene::radar::arena::ArenaBuffers<wgpu::Buffer>> {
        self.arena.data()
    }
}

/// A height's two arenas.
#[derive(Debug)]
pub struct LevelArena {
    /// The tiles' slices from the height's band up (2 values a vertex).
    pub slices: GpuArena,
    /// Their walls likewise (3 values a vertex).
    pub walls: GpuArena,
}

/// A tile's meshes in the arenas' frame, made once while it is in view.
struct WorldMeshes {
    slices: RadarMesh,
    walls: Option<RadarMesh>,
}

impl WorldMeshes {
    fn of(tile: &RadarTile) -> Self {
        WorldMeshes {
            slices: world_mesh(tile.index, &tile.mesh.slices, &tile.slice_vertices),
            walls: tile.mesh.walls.as_ref().map(|walls| world_mesh(tile.index, walls, &tile.wall_vertices)),
        }
    }
}

/// The lifted layers' buffers (`arenas`, `arenasVersion`): a slices and a walls arena per height of
/// `RADAR_LEVELS_KM`, updated in place as tiles come and go, so a pan uploads only the tiles it
/// brings into view; freed while nothing lifted is drawn.
#[derive(Default)]
pub struct Arenas {
    levels: Option<Vec<LevelArena>>,
    /// The version of the tiles in view the arenas hold (`-1` today when none).
    version: Option<u64>,
    /// The meshes of the tiles in the arenas, in the arenas' frame.
    meshes: HashMap<TileKey, WorldMeshes>,
}

impl std::fmt::Debug for Arenas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Arenas").field("levels", &self.levels).field("version", &self.version).field("tiles", &self.meshes.len()).finish()
    }
}

impl Arenas {
    /// None made yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// `liftedArenas`: the arenas made if they are not, then, when `version` is not the one they
    /// hold, each height's arenas made to hold exactly the tiles of `in_view` (the radar tiles'
    /// `in_view()`, in that order) that reach it, from that level's band up: slices, and walls for
    /// a tile that has them. Each tile's levels run up from the lowest without gaps, so a tile's
    /// level `i` is height `i`. The bytes written.
    pub fn update(&mut self, gpu: &Gpu, in_view: &[(TileIndex, Arc<RadarTile>)], version: u64) -> usize {
        let levels = self.levels.get_or_insert_with(|| RADAR_LEVELS_KM.iter().map(|_| LevelArena { slices: GpuArena::new(2), walls: GpuArena::new(3) }).collect());
        if self.version == Some(version) {
            return 0;
        }
        self.version = Some(version);
        let keys: Vec<TileKey> = in_view.iter().map(|(_, tile)| TileKey(Arc::clone(tile))).collect();
        // the meshes of the tiles in view, made once each: the others are let go
        let current: HashSet<&TileKey> = keys.iter().collect();
        self.meshes.retain(|key, _| current.contains(key));
        for key in &keys {
            self.meshes.entry(key.clone()).or_insert_with(|| WorldMeshes::of(&key.0));
        }
        let meshes = &self.meshes;
        let mut written = 0;
        for (i, level) in levels.iter_mut().enumerate() {
            let reaching = || keys.iter().filter_map(move |key| key.0.mesh.levels.get(i).map(|l| (key, &meshes[key], l.from_band)));
            written += level.slices.update(gpu, reaching().map(|(key, m, from_band)| (key.clone(), ArenaPiece { mesh: &m.slices, from_band })));
            written += level.walls.update(gpu, reaching().filter_map(|(key, m, from_band)| Some((key.clone(), ArenaPiece { mesh: m.walls.as_ref()?, from_band }))));
        }
        written
    }

    /// The heights' arenas, lowest first (none until the first update, or after [`Self::free`]).
    pub fn levels(&self) -> &[LevelArena] {
        self.levels.as_deref().unwrap_or(&[])
    }

    /// The version of the tiles in view they hold.
    pub fn version(&self) -> Option<u64> {
        self.version
    }

    /// How many tiles' meshes are held for them.
    pub fn held(&self) -> usize {
        self.meshes.len()
    }

    /// `freeArenas`: every buffer and mesh let go; the next update starts afresh.
    pub fn free(&mut self) {
        self.levels = None;
        self.version = None;
        self.meshes.clear();
    }
}

/// An arena draw's [`StyleUniform`] on the GPU (group 2): the layer's opacity and the walls'
/// stretch.
pub struct ArenaStyle {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
    value: StyleUniform,
}

impl ArenaStyle {
    /// Writes `value` unless it is what the buffer holds.
    pub fn set(&mut self, queue: &wgpu::Queue, value: StyleUniform) {
        if value != self.value {
            queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&value));
            self.value = value;
        }
    }

    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(STYLE_GROUP, &self.group, &[]);
    }
}

/// What draws an arena (`ArenaLayer`, a SolidPolygonLayer over the arena's buffers): `radar.wgsl`
/// with the positions and colours from two vertex buffers, one pipeline for slices (2 values a
/// vertex, the height 0) and one for walls (3), in one draw state (`RADAR_LIFTED` for the stack).
pub struct ArenaPipelines {
    slices: wgpu::RenderPipeline,
    walls: wgpu::RenderPipeline,
    style_layout: wgpu::BindGroupLayout,
}

impl ArenaPipelines {
    pub fn new(init: &Init<'_>, label: &str, state: DrawState) -> Self {
        let device = &init.gpu.device;
        let style_layout = uniform_layout(device, "radar arena style", size_of::<StyleUniform>());
        let module = shaders::module(device, label, SHADER);
        let colors = [wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4, offset: 0, shader_location: 1 }];
        let pipeline = |format: wgpu::VertexFormat, size: u64| {
            let positions = [wgpu::VertexAttribute { format, offset: 0, shader_location: 0 }];
            build(
                init,
                &Desc {
                    label,
                    module: &module,
                    vertex_entry: "vs",
                    fragment_entry: "fs",
                    buffers: &[
                        Some(wgpu::VertexBufferLayout { array_stride: size * 4, step_mode: wgpu::VertexStepMode::Vertex, attributes: &positions }),
                        Some(wgpu::VertexBufferLayout { array_stride: 4, step_mode: wgpu::VertexStepMode::Vertex, attributes: &colors }),
                    ],
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    state,
                    groups: &[&style_layout],
                },
            )
        };
        // the slices' two values feed the shader's three as (u, v, 0)
        ArenaPipelines { slices: pipeline(wgpu::VertexFormat::Float32x2, 2), walls: pipeline(wgpu::VertexFormat::Float32x3, 3), style_layout }
    }

    /// A style for these pipelines.
    pub fn style(&self, device: &wgpu::Device, value: StyleUniform) -> ArenaStyle {
        let (buffer, group) = uniform_group(device, "radar arena style", &self.style_layout, &value);
        ArenaStyle { buffer, group, value }
    }

    /// Draws `arena` in `frame` (pushed in `prepare`, from [`frame`]) with `style`.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, arena: &GpuArena, frame: FrameSlot, style: &ArenaStyle) {
        if arena.index_count() == 0 {
            return;
        }
        pass.set_pipeline(if arena.size() == 2 { &self.slices } else { &self.walls });
        style.bind(pass);
        cx.bind_frame(pass, frame);
        arena.draw(pass);
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::{ViewState, lng_lat_to_world};
    use rustradar_scene::radar::mesh::merge_meshes;

    use super::super::tiles::{self, RadarTiles, decode};
    use super::*;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{self, DirTiles, TileFetcher};
    use crate::jobs::Cancel;
    use crate::layers::{Layer, PrepareCx};
    use crate::renderer::{MapRenderer, SceneInput};

    fn fixture(tile: TileIndex) -> Arc<RadarTile> {
        let path = io::fixture_tiles_dir().join(format!("rainviewer/{}/{}/{}.png", tile.z, tile.x, tile.y));
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        decode(tile, &bytes, &[], &Cancel::new()).unwrap().tile.expect("a tile with echo")
    }

    /// Three wet tiles over Germany and Poland: 6/35/22, its eastern and its southern neighbours.
    fn three() -> [Arc<RadarTile>; 3] {
        [TileIndex::new(35, 22, 6), TileIndex::new(36, 22, 6), TileIndex::new(35, 23, 6)].map(fixture)
    }

    #[test]
    fn world_meshes_are_the_tiles_in_the_world_s_frame() {
        let tile = fixture(TileIndex::new(35, 22, 6));
        for (mesh, vertices) in [(&tile.mesh.slices, &tile.slice_vertices), (tile.mesh.walls.as_ref().unwrap(), &tile.wall_vertices)] {
            let world = world_mesh(tile.index, mesh, vertices);
            assert_eq!((world.size, world.positions.len()), (mesh.size, mesh.positions.len()));
            assert_eq!((&world.colors, &world.indices), (&mesh.colors, &mesh.indices));
            assert_eq!((world.band_starts, world.band_vertex_starts), (mesh.band_starts, mesh.band_vertex_starts));
            let mut worst: f64 = 0.0;
            for (p, w) in mesh.positions.chunks_exact(mesh.size).zip(world.positions.chunks_exact(mesh.size)) {
                // deck's Web Mercator in f64 from the mesh's degrees: the frame is the world's
                let [x, y] = lng_lat_to_world([f64::from(p[0]), f64::from(p[1])]);
                let want = [x / WORLD_SIZE, (WORLD_SIZE - y) / WORLD_SIZE];
                worst = worst.max((f64::from(w[0]) - want[0]).abs()).max((f64::from(w[1]) - want[1]).abs());
                if mesh.size == 3 {
                    assert_eq!(w[2], p[2], "the walls' unit height");
                }
            }
            // within an f32 step of the world's [0, 1] (3e-5 world units)
            assert!(worst < 1.2e-7, "{worst}");
        }
    }

    /// An arena's buffers as the GPU holds them: positions, colours, indices.
    fn read_back(gpu: &Gpu, arena: &GpuArena) -> (Vec<f32>, Vec<u8>, Vec<u32>) {
        let data = arena.buffers().expect("buffers");
        let read = |buffer: &wgpu::Buffer| -> Vec<u8> {
            let out = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("arena read back"),
                size: buffer.size(),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            encoder.copy_buffer_to_buffer(buffer, 0, &out, 0, buffer.size());
            gpu.queue.submit([encoder.finish()]);
            out.map_async(wgpu::MapMode::Read, .., |r| r.expect("mapped"));
            gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
            out.get_mapped_range(..).expect("mapped range").to_vec()
        };
        (bytemuck::cast_slice(&read(&data.positions)).to_vec(), read(&data.colors), bytemuck::cast_slice(&read(&data.indices)).to_vec())
    }

    /// The triangles drawn, as each corner's position bits and colour; collapsed ones left out.
    fn triangles(index: &[u32], positions: &[f32], colors: &[u8], size: usize) -> Vec<Vec<u32>> {
        let mut out = Vec::new();
        for t in index.chunks_exact(3) {
            if t[0] == t[1] && t[1] == t[2] {
                continue;
            }
            let mut tri = Vec::new();
            for &v in t {
                let v = v as usize;
                tri.extend(positions[v * size..v * size + size].iter().map(|x| x.to_bits()));
                tri.extend(colors[v * 4..v * 4 + 4].iter().map(|&c| u32::from(c)));
            }
            out.push(tri);
        }
        out
    }

    fn drawn(gpu: &Gpu, arena: &GpuArena) -> Vec<Vec<u32>> {
        let (positions, colors, indices) = read_back(gpu, arena);
        triangles(&indices[..arena.index_count()], &positions, &colors, arena.size())
    }

    fn merged(parts: &[(&RadarMesh, usize)]) -> Vec<Vec<u32>> {
        let m = merge_meshes(parts).expect("something to merge");
        triangles(&m.indices, &m.positions, &m.colors, m.size)
    }

    fn sorted(mut a: Vec<Vec<u32>>) -> Vec<Vec<u32>> {
        a.sort();
        a
    }

    /// The bytes of a mesh's arrays from `from_band` up, as the TypeScript test adds them.
    fn bytes_from(mesh: &RadarMesh, from_band: usize) -> usize {
        let (index, vertex) = mesh.band_range(from_band);
        (mesh.vertex_count() - vertex) * (mesh.size * 4 + 4) + (mesh.indices.len() - index) * 4
    }

    #[test]
    fn an_arena_on_the_gpu_holds_what_merging_its_tiles_gives_and_is_edited_in_place() {
        let Some(gpu) = gpu() else { return };
        let [a, b, c] = three();
        let world = |t: &Arc<RadarTile>| world_mesh(t.index, &t.mesh.slices, &t.slice_vertices);
        let (wa, wb, wc) = (world(&a), world(&b), world(&c));
        let key = |t: &Arc<RadarTile>| TileKey(Arc::clone(t));
        let mut arena = GpuArena::new(2);
        // the stronger echoes: small enough to append into the smallest buffers
        let band = 4;
        let written = arena.update(&gpu, [(key(&a), ArenaPiece { mesh: &wa, from_band: band }), (key(&b), ArenaPiece { mesh: &wb, from_band: band })]);
        assert_eq!(written, bytes_from(&wa, band) + bytes_from(&wb, band));
        assert_eq!(arena.capacity(), Some((MIN_VERTICES, 3 * MIN_VERTICES)));
        // the same triangles, in the same order, as merging the tiles
        assert_eq!(drawn(&gpu, &arena), merged(&[(&wa, band), (&wb, band)]));

        // `a` leaves and `c` comes: the same buffers, only `c` and `a`'s indices written
        let (allocation, version) = (arena.allocation(), arena.version());
        let written = arena.update(&gpu, [(key(&b), ArenaPiece { mesh: &wb, from_band: band }), (key(&c), ArenaPiece { mesh: &wc, from_band: band })]);
        assert_eq!(arena.allocation(), allocation);
        assert_ne!(arena.version(), version);
        let (index, _) = wa.band_range(band);
        assert_eq!(written, bytes_from(&wc, band) + (wa.indices.len() - index) * 4);
        assert_eq!(sorted(drawn(&gpu, &arena)), sorted(merged(&[(&wb, band), (&wc, band)])));
        // nothing new: nothing written
        assert_eq!(arena.update(&gpu, [(key(&b), ArenaPiece { mesh: &wb, from_band: band }), (key(&c), ArenaPiece { mesh: &wc, from_band: band })]), 0);

        // the same two decoded again (a new coverage: new tiles to the arena), every band:
        // collapsed, then past the smallest buffers, so packed afresh into larger ones
        let (b2, c2) = (Arc::new((*b).clone()), Arc::new((*c).clone()));
        let written = arena.update(&gpu, [(key(&b2), ArenaPiece { mesh: &wb, from_band: 0 }), (key(&c2), ArenaPiece { mesh: &wc, from_band: 0 })]);
        assert_ne!(arena.allocation(), allocation);
        let vertices = wb.vertex_count() + wc.vertex_count();
        let indices = wb.indices.len() + wc.indices.len();
        assert_eq!(arena.capacity(), Some(capacity_for(vertices, indices)));
        let collapsed = (wb.indices.len() - wb.band_range(band).0 + wc.indices.len() - wc.band_range(band).0) * 4;
        assert_eq!(written, collapsed + bytes_from(&wb, 0) + bytes_from(&wc, 0));
        assert_eq!(drawn(&gpu, &arena), merged(&[(&wb, 0), (&wc, 0)]));

        // an empty view lets the buffers go
        arena.update(&gpu, []);
        assert!(!arena.has_data());
        assert_eq!(arena.index_count(), 0);
    }

    #[test]
    fn each_height_holds_the_tiles_that_reach_it_from_its_band_up() {
        let Some(gpu) = gpu() else { return };
        let tiles = three();
        let in_view: Vec<(TileIndex, Arc<RadarTile>)> = tiles.iter().map(|t| (t.index, Arc::clone(t))).collect();
        let mut arenas = Arenas::new();
        assert!(arenas.levels().is_empty());
        let written = arenas.update(&gpu, &in_view, 1);
        assert!(written > 0);
        assert_eq!(arenas.levels().len(), RADAR_LEVELS_KM.len());
        assert_eq!((arenas.version(), arenas.held()), (Some(1), 3));
        for (i, level) in arenas.levels().iter().enumerate() {
            let reaching: Vec<(&RadarTile, usize)> = tiles.iter().filter_map(|t| t.mesh.levels.get(i).map(|l| (&**t, l.from_band))).collect();
            let count = |walls: bool| -> usize {
                reaching
                    .iter()
                    .map(|(t, from_band)| {
                        let mesh = if walls { t.mesh.walls.as_ref().unwrap() } else { &t.mesh.slices };
                        mesh.indices.len() - mesh.band_range(*from_band).0
                    })
                    .sum()
            };
            assert_eq!(level.slices.index_count(), count(false), "height {i}");
            assert_eq!(level.walls.index_count(), count(true), "height {i}");
            // 6/36/22 alone reaches 12 km: one tile's bands there
            assert_eq!(level.slices.has_data(), !reaching.is_empty(), "height {i}");
            if let Some(&(t, from_band)) = reaching.first() {
                let world = world_mesh(t.index, &t.mesh.slices, &t.slice_vertices);
                // the first tile's slices lead each height's arena
                let first = merged(&[(&world, from_band)]);
                assert_eq!(drawn(&gpu, &level.slices)[..first.len()], first[..], "height {i}");
            }
        }
        assert!(arenas.levels()[6].slices.has_data() && arenas.levels()[6].slices.index_count() < arenas.levels()[0].slices.index_count());
        // the same version: nothing to do
        assert_eq!(arenas.update(&gpu, &in_view, 1), 0);
        // the view moves on without 6/35/23: only the collapsed indices are written, and its
        // meshes are let go
        let written = arenas.update(&gpu, &in_view[..2], 2);
        let gone = &tiles[2];
        let collapsed: usize = gone
            .mesh
            .levels
            .iter()
            .map(|l| {
                let walls = gone.mesh.walls.as_ref().unwrap();
                (gone.mesh.slices.indices.len() - gone.mesh.slices.band_range(l.from_band).0 + walls.indices.len() - walls.band_range(l.from_band).0) * 4
            })
            .sum();
        assert_eq!(written, collapsed);
        assert_eq!(arenas.held(), 2);
        // freed: nothing kept, made again by the next update
        arenas.free();
        assert!(arenas.levels().is_empty() && arenas.version().is_none() && arenas.held() == 0);
        arenas.update(&gpu, &in_view, 2);
        assert!(arenas.levels()[0].slices.has_data());
    }

    /// The radar tiles of `input` drawn from one arena per draw state: every band of every tile in
    /// view, at the ground layer's opacity, on the ground.
    struct ArenaProbe {
        tiles: RadarTiles,
        input: tiles::Input,
        arena: GpuArena,
        pipelines: ArenaPipelines,
        style: ArenaStyle,
        frame: Option<FrameSlot>,
        version: u64,
    }

    impl Layer for ArenaProbe {
        fn name(&self) -> &'static str {
            "arena probe"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.tiles.update(cx, &self.input, |_, _, _| ());
            if self.version != self.tiles.version() {
                self.version = self.tiles.version();
                let meshes: Vec<(TileKey, RadarMesh)> =
                    self.tiles.in_view().iter().map(|(_, t)| (TileKey(Arc::clone(t)), world_mesh(t.index, &t.mesh.slices, &t.slice_vertices))).collect();
                self.arena.update(cx.gpu, meshes.iter().map(|(k, m)| (k.clone(), ArenaPiece { mesh: m, from_band: 0 })));
            }
            self.frame = Some(cx.frames.push(&frame(cx.view, 0.0)));
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            if let Some(frame) = self.frame {
                self.pipelines.draw(pass, cx, &self.arena, frame, &self.style);
            }
        }

        fn pending(&self) -> bool {
            self.tiles.pending()
        }
    }

    const FRAME: &str = "https://tilecache.rainviewer.com/v2/radar/47622fe604ca/256/{z}/{x}/{y}/2/1_1.png";

    /// The radar of the fixture frame on ink (no map), at `view` on a `size` canvas: drawn by the
    /// radar layer (the ground, per tile in its frame), or by one arena (`arena`).
    fn radar_shot(gpu: &Gpu, view: ViewState, size: [f64; 2], arena: bool) -> (Image, usize) {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let tiles: Arc<dyn TileFetcher> = Arc::new(DirTiles::fixtures());
        renderer.set_tiles(tiles, None);
        let mut scene = SceneInput::new(view, size, 1.0, Basemap::Dark);
        let input = tiles::Input { url: Some(FRAME.into()), ..Default::default() };
        if arena {
            let init = renderer.init(gpu);
            let pipelines = ArenaPipelines::new(&init, "arena probe", DrawState::SURFACE_NO_CULL);
            let style = pipelines.style(&gpu.device, StyleUniform::new(super::super::ground::OPACITY, 0.0));
            let probe = ArenaProbe { tiles: RadarTiles::new(&init), input, arena: GpuArena::new(2), pipelines, style, frame: None, version: 0 };
            renderer.add_layer(Box::new(probe));
        } else {
            scene.radar.tiles = input;
        }
        let image = render_with(gpu, &mut renderer, &scene, 1.0).expect("rendered");
        assert!(!renderer.pending(), "still loading: {:?}", renderer.pending_layers());
        let differs = (0..image.height).flat_map(|y| (0..image.width).map(move |x| (x, y))).filter(|&(x, y)| image.pixel(x, y) != [30, 30, 30, 255]).count();
        (image, differs)
    }

    /// One arena over every tile in view, in the world's frame, draws the radar where the ground
    /// layer draws it tile by tile in each tile's frame: on the globe (V2) and on the flat map
    /// close in (zoom 13 over Pomerania, a z7 tile stretched), to within the odd edge pixel.
    #[test]
    fn an_arena_draws_where_the_tiles_do_on_the_globe_and_the_flat_map() {
        let Some(gpu) = gpu() else { return };
        let v2 = ViewState { latitude: 52.0, longitude: 10.0, zoom: 4.5, pitch: 0.0, bearing: 0.0, position: None };
        // close in on the strongest echo of a z7 fixture tile (Pomerania): the middle of the first
        // triangle of its highest band
        let tile = fixture(TileIndex::new(70, 38, 7));
        let s = &tile.mesh.slices;
        let top = (0..s.band_starts.len() - 1).rev().find(|&b| s.band_starts[b] < s.band_starts[b + 1]).unwrap();
        let t = s.band_starts[top] as usize;
        let corner = |k: usize| {
            let i = s.indices[t + k] as usize;
            [f64::from(s.positions[2 * i]), f64::from(s.positions[2 * i + 1])]
        };
        let [lon, lat] = [0, 1].map(|axis| (corner(0)[axis] + corner(1)[axis] + corner(2)[axis]) / 3.0);
        let close = ViewState { latitude: lat, longitude: lon, zoom: 13.0, pitch: 0.0, bearing: 0.0, position: None };
        for (view, size, name) in [(v2, [1100.0, 720.0], "V2"), (close, [600.0, 400.0], "zoom 13")] {
            let (ground, painted) = radar_shot(&gpu, view, size, false);
            let (arena, arena_painted) = radar_shot(&gpu, view, size, true);
            let mut off = 0usize;
            let mut worst = 0i32;
            for y in 0..ground.height {
                for x in 0..ground.width {
                    let (g, a) = (ground.pixel(x, y), arena.pixel(x, y));
                    let d = (0..3).map(|i| (i32::from(g[i]) - i32::from(a[i])).abs()).max().unwrap();
                    if d > 0 {
                        off += 1;
                        worst = worst.max(d);
                    }
                }
            }
            eprintln!("{name}: ground {painted} px, arena {arena_painted} px, {off} differ (worst {worst})");
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
            std::fs::create_dir_all(&dir).expect("the shot folder");
            let file = name.replace(' ', "");
            ground.save_png(&dir.join(format!("w2-ground-{file}.png"))).expect("the shot written");
            arena.save_png(&dir.join(format!("w2-arena-{file}.png"))).expect("the shot written");
            assert!(painted > 1_000, "{name}: {painted} px of radar");
            assert!(off * 200 < painted, "{name}: {off} of {painted} px differ");
        }
    }
}
