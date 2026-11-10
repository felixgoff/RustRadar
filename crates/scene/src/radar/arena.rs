//! Port of `radar-arena.ts` (S1.27; doc 02 §3.5): the lifted RainViewer layers' buffers, kept on
//! the GPU between tile changes.
//!
//! Each height is one layer over every tile in view, so the stack blends in height order across
//! tile borders (one layer per tile would draw a seam where two tiles' stacks meet). Merging the
//! tiles into fresh arrays each time the view brought in or dropped a tile re-uploaded every tile
//! at every height, some 40 MB for a stormy tilted view, several times a second while panning.
//! Here each height's layer draws from buffers it owns and edits in place: a tile coming into
//! view is appended, one leaving has its triangles collapsed, and only when the buffers fill up
//! are they packed afresh.
//!
//! This crate has no GPU: [`MeshArena`] does the bookkeeping and talks to buffers through
//! [`ArenaDevice`], which the map crate implements with wgpu (`queue.write_buffer` on
//! sub-ranges) and the tests with plain memory. luma.gl's `Device` and deck.gl's `ArenaLayer`
//! (which only told the layer how many indices to draw) stay behind: the renderer draws
//! [`MeshArena::index_count`] `u32` indices from [`MeshArena::data`] and redraws when
//! [`MeshArena::version`] changes.
//!
//! Tiles are keyed by any hashable key (the TypeScript keyed them by object identity); `pieces`
//! is an [`IndexMap`] so that, as a JavaScript `Map`, it is iterated in insertion order, which is
//! the order tiles are appended and drawn in.

use std::hash::Hash;

use indexmap::IndexMap;

use super::mesh::RadarMesh;

/// What a buffer holds, for the GPU's usage flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferUsage {
    /// Positions (`f32` × size per vertex) or colours (RGBA `u8` per vertex).
    Vertex,
    /// `u32` indices.
    Index,
}

/// The GPU side of a [`MeshArena`]: buffers that can be created, written in part and destroyed.
pub trait ArenaDevice {
    type Buffer;
    /// A buffer of `byte_length` bytes, filled with zeros (as WebGL's are: indices not yet
    /// written then draw nothing).
    fn create_buffer(&mut self, usage: BufferUsage, byte_length: usize) -> Self::Buffer;
    /// Writes `data` at `byte_offset`; the arena never writes past a buffer's end.
    fn write(&mut self, buffer: &Self::Buffer, byte_offset: usize, data: &[u8]);
    /// Releases a buffer.
    fn destroy(&mut self, buffer: Self::Buffer);
}

/// A mesh's bands from `from_band` up: one tile's share of a layer (`ArenaPiece`).
#[derive(Debug, Clone, Copy)]
pub struct ArenaPiece<'a> {
    pub mesh: &'a RadarMesh,
    pub from_band: usize,
}

/// The buffers a layer draws from (`ArenaData`): a new value, with a new `id`, only when they
/// are reallocated.
#[derive(Debug)]
pub struct ArenaBuffers<B> {
    /// `size` `f32`s per vertex, `vertex_capacity` vertices.
    pub positions: B,
    /// RGBA `u8` per vertex.
    pub colors: B,
    /// `index_capacity` `u32`s.
    pub indices: B,
    pub vertex_capacity: usize,
    pub index_capacity: usize,
    /// Tells one allocation from the next.
    pub id: u64,
}

/// Packed buffers are this much larger than what they hold, leaving room to append.
pub const GROWTH: f64 = 2.0;
/// Smallest buffers, in vertices (indices get three times as many), so a light shower doesn't
/// reallocate on every tile.
pub const MIN_VERTICES: usize = 1 << 16;

/// Buffers for `vertices` and `indices`: what a pack allocates (`capacityFor`).
pub fn capacity_for(vertices: usize, indices: usize) -> (usize, usize) {
    (MIN_VERTICES.max((vertices as f64 * GROWTH).ceil() as usize), (3 * MIN_VERTICES).max((indices as f64 * GROWTH).ceil() as usize))
}

/// Where a tile sits in the buffers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Slot {
    vertex: usize,
    index: usize,
    index_count: usize,
}

/// A piece's runs: first index, first vertex, and how many of each.
struct Span {
    index: usize,
    vertex: usize,
    index_count: usize,
    vertex_count: usize,
}

fn span(piece: &ArenaPiece<'_>) -> Span {
    let (index, vertex) = piece.mesh.band_range(piece.from_band);
    Span { index, vertex, index_count: piece.mesh.indices.len() - index, vertex_count: piece.mesh.vertex_count() - vertex }
}

/// One layer's buffers and which tile sits where in them (`MeshArena`).
#[derive(Debug)]
pub struct MeshArena<K, B> {
    /// Values per vertex: 2 for slices, 3 for walls.
    size: usize,
    data: Option<ArenaBuffers<B>>,
    index_count: usize,
    version: u64,
    vertex_capacity: usize,
    index_capacity: usize,
    vertex_end: usize,
    slots: IndexMap<K, Slot>,
    allocations: u64,
}

impl<K: Hash + Eq + Clone, B> MeshArena<K, B> {
    /// An empty arena for meshes of `size` values per vertex.
    pub fn new(size: usize) -> Self {
        Self {
            size,
            data: None,
            index_count: 0,
            version: 0,
            vertex_capacity: 0,
            index_capacity: 0,
            vertex_end: 0,
            slots: IndexMap::new(),
            allocations: 0,
        }
    }

    /// The layer's buffers; `None` when nothing is held.
    pub fn data(&self) -> Option<&ArenaBuffers<B>> {
        self.data.as_ref()
    }

    /// Indices in use, up to the last appended piece: what the layer draws.
    pub fn index_count(&self) -> usize {
        self.index_count
    }

    /// Counts changes to the buffers: the renderer redraws when it changes.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Makes the buffers hold exactly `pieces` (keyed by tile), uploading only what is new to
    /// them.
    pub fn update<D: ArenaDevice<Buffer = B>>(&mut self, device: &mut D, pieces: &IndexMap<K, ArenaPiece<'_>>) {
        let version = self.version;
        let gone: Vec<K> = self.slots.keys().filter(|k| !pieces.contains_key(*k)).cloned().collect();
        for key in gone {
            let Some(slot) = self.slots.shift_remove(&key) else { continue };
            // collapsed onto vertex 0: drawn as nothing, and only these indices are written
            if let Some(data) = &self.data {
                device.write(&data.indices, slot.index * 4, &vec![0; slot.index_count * 4]);
            }
            self.version = version + 1;
        }
        let mut vertices = self.vertex_end;
        let mut indices = self.index_count;
        let mut live_vertices = 0;
        let mut live_indices = 0;
        for (key, piece) in pieces {
            let s = span(piece);
            live_vertices += s.vertex_count;
            live_indices += s.index_count;
            if self.slots.contains_key(key) {
                continue;
            }
            vertices += s.vertex_count;
            indices += s.index_count;
        }
        // packed when full, or when what is left would fit in far smaller buffers (so collapsed
        // triangles don't pile up), or freed when nothing is left
        let fit = capacity_for(live_vertices, live_indices);
        let full = vertices > self.vertex_capacity || indices > self.index_capacity;
        let shrink = self.vertex_capacity > 2 * fit.0 || self.index_capacity > 2 * fit.1;
        if full || shrink || (self.data.is_some() && live_indices == 0) {
            self.pack(device, pieces);
            self.version = version + 1;
            return;
        }
        for (key, piece) in pieces {
            if !self.slots.contains_key(key) {
                self.append(device, key, piece);
            }
        }
    }

    /// Writes every piece afresh from the start, in buffers sized for them with room to grow.
    fn pack<D: ArenaDevice<Buffer = B>>(&mut self, device: &mut D, pieces: &IndexMap<K, ArenaPiece<'_>>) {
        let mut vertices = 0;
        let mut indices = 0;
        for piece in pieces.values() {
            let s = span(piece);
            vertices += s.vertex_count;
            indices += s.index_count;
        }
        let stale = self.index_count;
        self.slots.clear();
        self.vertex_end = 0;
        self.index_count = 0;
        if indices == 0 {
            self.destroy(device);
            return;
        }
        let (vertex_capacity, index_capacity) = capacity_for(vertices, indices);
        // the same buffers do if they leave room to append and aren't far too big
        let keep = self.data.is_some()
            && self.vertex_capacity >= vertex_capacity
            && self.index_capacity >= index_capacity
            && self.vertex_capacity <= 2 * vertex_capacity
            && self.index_capacity <= 2 * index_capacity;
        if !keep {
            self.destroy(device);
            self.vertex_capacity = vertex_capacity;
            self.index_capacity = index_capacity;
            self.allocations += 1;
            self.data = Some(ArenaBuffers {
                positions: device.create_buffer(BufferUsage::Vertex, vertex_capacity * self.size * 4),
                colors: device.create_buffer(BufferUsage::Vertex, vertex_capacity * 4),
                indices: device.create_buffer(BufferUsage::Index, index_capacity * 4),
                vertex_capacity,
                index_capacity,
                id: self.allocations,
            });
        }
        for (key, piece) in pieces {
            self.append(device, key, piece);
        }
        // what was drawn past the new end is no longer
        if keep
            && stale > self.index_count
            && let Some(data) = &self.data
        {
            device.write(&data.indices, self.index_count * 4, &vec![0; (stale - self.index_count) * 4]);
        }
    }

    fn append<D: ArenaDevice<Buffer = B>>(&mut self, device: &mut D, key: &K, piece: &ArenaPiece<'_>) {
        let s = span(piece);
        if s.index_count == 0 {
            return;
        }
        let Some(data) = &self.data else { return };
        let mesh = piece.mesh;
        let size = self.size;
        let at = Slot { vertex: self.vertex_end, index: self.index_count, index_count: s.index_count };
        // the tile's own arrays, written as they are; only the indices need moving
        let positions = &mesh.positions[s.vertex * size..(s.vertex + s.vertex_count) * size];
        device.write(&data.positions, at.vertex * size * 4, bytemuck::cast_slice(positions));
        device.write(&data.colors, at.vertex * 4, &mesh.colors[s.vertex * 4..(s.vertex + s.vertex_count) * 4]);
        // `mesh.indices[index + i] + shift` in a Uint32Array: the shift can be negative
        let shift = (at.vertex as u32).wrapping_sub(s.vertex as u32);
        let shifted: Vec<u32> = mesh.indices[s.index..s.index + s.index_count].iter().map(|&i| i.wrapping_add(shift)).collect();
        device.write(&data.indices, at.index * 4, bytemuck::cast_slice(&shifted));
        self.slots.insert(key.clone(), at);
        self.vertex_end += s.vertex_count;
        self.index_count += s.index_count;
        self.version += 1;
    }

    /// Releases the buffers and forgets every tile.
    pub fn destroy<D: ArenaDevice<Buffer = B>>(&mut self, device: &mut D) {
        if let Some(data) = self.data.take() {
            device.destroy(data.positions);
            device.destroy(data.colors);
            device.destroy(data.indices);
        }
        self.vertex_capacity = 0;
        self.index_capacity = 0;
        self.slots.clear();
        self.vertex_end = 0;
        self.index_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::radar::mesh::{BandPolygon, WallLine, build_mesh, build_walls, merge_meshes};

    /// A stand-in for the GPU's buffers: plain memory, counting what is written.
    struct FakeBuffer {
        bytes: Vec<u8>,
        destroyed: bool,
    }

    #[derive(Default)]
    struct FakeDevice {
        written: usize,
        buffers: Vec<FakeBuffer>,
    }

    impl ArenaDevice for FakeDevice {
        type Buffer = usize;

        fn create_buffer(&mut self, _usage: BufferUsage, byte_length: usize) -> usize {
            // zeroed, as WebGL's are
            self.buffers.push(FakeBuffer { bytes: vec![0; byte_length], destroyed: false });
            self.buffers.len() - 1
        }

        fn write(&mut self, buffer: &usize, byte_offset: usize, data: &[u8]) {
            let buffer = &mut self.buffers[*buffer];
            assert!(byte_offset + data.len() <= buffer.bytes.len());
            buffer.bytes[byte_offset..byte_offset + data.len()].copy_from_slice(data);
            self.written += data.len();
        }

        fn destroy(&mut self, buffer: usize) {
            self.buffers[buffer].destroyed = true;
        }
    }

    /// A square of `band` at (x, y), a degree across.
    fn square(band: usize, x: f64, y: f64) -> BandPolygon {
        BandPolygon { band, positions: vec![x, y, x + 1.0, y, x + 1.0, y + 1.0, x, y + 1.0], hole_indices: vec![] }
    }

    /// A tile's mesh: a few squares in several bands, offset so tiles differ.
    fn tile(offset: f64, count: usize) -> RadarMesh {
        build_mesh(&(0..count).map(|i| square(i % 4, offset + 2.0 * i as f64, offset)).collect::<Vec<_>>())
    }

    /// A triangle as its corners' positions (as bits) and colours, corner by corner.
    type Tri = Vec<u32>;

    fn triangles(index: &[u32], positions: &[f32], colors: &[u8], size: usize) -> Vec<Tri> {
        let mut out = Vec::new();
        for t in index.chunks_exact(3) {
            if t[0] == t[1] && t[1] == t[2] {
                continue; // collapsed
            }
            let mut tri = Vec::with_capacity(3 * (size + 4));
            for &v in t {
                let v = v as usize;
                tri.extend(positions[v * size..v * size + size].iter().map(|x| x.to_bits()));
                tri.extend(colors[v * 4..v * 4 + 4].iter().map(|&c| u32::from(c)));
            }
            out.push(tri);
        }
        out
    }

    /// The triangles drawn, as position and colour per corner; collapsed ones left out.
    fn drawn(arena: &MeshArena<usize, usize>, device: &FakeDevice, size: usize) -> Vec<Tri> {
        let data = arena.data().expect("buffers");
        let bytes = |b: usize| &device.buffers[b].bytes;
        let positions: Vec<f32> = bytes(data.positions).chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let index: Vec<u32> = bytes(data.indices).chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        triangles(&index[..arena.index_count()], &positions, bytes(data.colors), size)
    }

    fn merged(parts: &[(&RadarMesh, usize)], size: usize) -> Vec<Tri> {
        let data = merge_meshes(parts).expect("something to merge");
        triangles(&data.indices, &data.positions, &data.colors, size)
    }

    fn sorted(mut a: Vec<Tri>) -> Vec<Tri> {
        a.sort();
        a
    }

    fn pieces<'a>(parts: &[(usize, &'a RadarMesh, usize)]) -> IndexMap<usize, ArenaPiece<'a>> {
        parts.iter().map(|&(key, mesh, from_band)| (key, ArenaPiece { mesh, from_band })).collect()
    }

    fn tiles() -> [RadarMesh; 3] {
        [tile(0.0, 6), tile(20.0, 6), tile(40.0, 6)]
    }

    /// Bytes of a mesh's arrays, as the TypeScript's `byteLength`s add up.
    fn bytes(m: &RadarMesh) -> usize {
        m.positions.len() * 4 + m.colors.len() + m.indices.len() * 4
    }

    #[test]
    fn draws_the_same_triangles_in_the_same_order_as_merging_the_tiles() {
        let [a, b, _] = tiles();
        let mut device = FakeDevice::default();
        let mut arena = MeshArena::new(2);
        let pieces = pieces(&[(0, &a, 1), (1, &b, 1)]);
        arena.update(&mut device, &pieces);
        assert_eq!(drawn(&arena, &device, 2), merged(&[(&a, 1), (&b, 1)], 2));
    }

    #[test]
    fn a_tile_coming_into_view_is_appended_one_leaving_collapsed_and_the_rest_left_alone() {
        let [a, b, c] = tiles();
        let mut device = FakeDevice::default();
        let mut arena = MeshArena::new(2);
        arena.update(&mut device, &pieces(&[(0, &a, 0), (1, &b, 0)]));
        let data = arena.data().map(|d| d.id);
        let version = arena.version();
        device.written = 0;
        arena.update(&mut device, &pieces(&[(1, &b, 0), (2, &c, 0)]));
        // the same buffers: nothing to upload afresh
        assert_eq!(arena.data().map(|d| d.id), data);
        assert_ne!(arena.version(), version);
        assert_eq!(sorted(drawn(&arena, &device, 2)), sorted(merged(&[(&b, 0), (&c, 0)], 2)));
        // only the new tile and the leaving tile's indices were written, not `b`
        assert_eq!(device.written, bytes(&c) + a.indices.len() * 4);
    }

    #[test]
    fn nothing_changes_for_the_same_tiles() {
        let [a, _, _] = tiles();
        let mut device = FakeDevice::default();
        let mut arena = MeshArena::new(2);
        let pieces = pieces(&[(0, &a, 0)]);
        arena.update(&mut device, &pieces);
        let version = arena.version();
        device.written = 0;
        arena.update(&mut device, &pieces.clone());
        assert_eq!(device.written, 0);
        assert_eq!(arena.version(), version);
    }

    #[test]
    fn walls_keep_their_height_and_are_packed_afresh_once_the_buffers_are_full() {
        let mut device = FakeDevice::default();
        let mut arena = MeshArena::new(3);
        // a long ring: enough vertices to outgrow the smallest buffers in a few tiles
        let ring = |offset: f64| {
            let positions = (0..30_000).map(|i| offset + if i % 2 == 1 { i as f64 * 1e-4 } else { i as f64 * 2e-4 }).collect();
            build_walls(&[WallLine { band: 2, positions }])
        };
        let meshes: Vec<RadarMesh> = (0..6).map(|i| ring(i as f64 * 10.0)).collect();
        let mut data = None;
        let mut reallocated = 0;
        for i in 0..meshes.len() {
            // a sliding window of two tiles, as in a pan
            let window: Vec<(usize, &RadarMesh, usize)> = (i.saturating_sub(1)..=i).map(|k| (k, &meshes[k], 0)).collect();
            arena.update(&mut device, &pieces(&window));
            let id = arena.data().map(|d| d.id);
            if id != data {
                reallocated += 1;
            }
            data = id;
            let parts: Vec<(&RadarMesh, usize)> = window.iter().map(|&(_, m, f)| (m, f)).collect();
            assert_eq!(sorted(drawn(&arena, &device, 3)), sorted(merged(&parts, 3)));
        }
        assert!(reallocated > 1, "{reallocated}");
        // the replaced buffers were released
        assert_eq!(device.buffers.iter().filter(|b| !b.destroyed).count(), 3);
    }

    #[test]
    fn bands_below_from_band_are_left_out() {
        let [a, _, _] = tiles();
        let mut device = FakeDevice::default();
        let mut arena = MeshArena::new(2);
        arena.update(&mut device, &pieces(&[(0, &a, 3)]));
        assert_eq!(drawn(&arena, &device, 2), merged(&[(&a, 3)], 2));
        assert!(drawn(&arena, &device, 2).len() < merged(&[(&a, 0)], 2).len());
    }

    #[test]
    fn an_empty_view_frees_the_buffers() {
        let [a, _, _] = tiles();
        let mut device = FakeDevice::default();
        let mut arena = MeshArena::new(2);
        arena.update(&mut device, &pieces(&[(0, &a, 0)]));
        arena.update(&mut device, &IndexMap::new());
        assert!(arena.data().is_none());
        assert!(device.buffers.iter().all(|b| b.destroyed));
    }

    #[test]
    fn capacities_grow_twice_from_the_minimum() {
        assert_eq!(capacity_for(0, 0), (65_536, 196_608));
        assert_eq!(capacity_for(40_000, 100_001), (80_000, 200_002));
    }
}
