//! The 3D surface's meshes (item Z1; doc 04 §14.3, §14.5): a height tile to the triangles deck.gl's
//! `TerrainLayer` drew, as loaders.gl 4.5.3's `TerrainLoader` makes them
//! (`@loaders.gl/terrain/dist/lib/parse-terrain.js`, `helpers/skirt.js`) with mapbox/martini 0.2.0's
//! RTIN (`@mapbox/martini/index.js`), transliterated with their arithmetic in the same order.
//!
//! The steps of `makeTerrainMeshFromImage` for a square tile whose side is a power of two (every
//! Mapterhorn tile, 512 px, and every Mapzen tile, 256 px), [`terrain_mesh`]:
//! 1. [`terrain_grid`] (`getTerrain`): the pixels decoded with deck's `elevationDecoder` into a
//!    `(w + 1) × (h + 1)` grid of `f32` heights, its last row and column back-filled from the row
//!    and column before them (martini needs `2^n + 1` points a side);
//! 2. [`Martini`] and [`Tile`] (`new Martini(w + 1).createTile(terrain)`): the error of every
//!    triangle of the right-triangulated irregular network, then [`Tile::get_mesh`] at the layer's
//!    `meshMaxError`: the coarsest triangles whose error is within it;
//! 3. [`mesh_attributes`] (`getMeshAttributes`): each vertex `x · xScale + minX`,
//!    `−y · yScale + maxY` within the tile's `bounds` (north up) and its height in metres, with
//!    texture coordinates `x / w`, `y / h`;
//! 4. [`bounding_box`] (`getMeshBoundingBox`) before the skirt, so its heights are the ground's;
//! 5. [`add_skirt`] (`addSkirt`): every outside edge hung down by `skirtHeight` metres, so
//!    neighbouring tiles of other zooms leave no crack between them.
//!
//! What deck's `TerrainLayer` 9.4.0 hands the loader: [`effective_mesh_max_error`] (never below
//! 1 m), [`skirt_height`] (twice that for tiled data) and [`overlapped_bounds`] (each tile's
//! bounds grown by one pixel of `tileSize` on every side, `TILE_OVERLAP_PIXELS`).
//!
//! Differences from the JavaScript:
//! - a tile that is not square with a power-of-two side would go to loaders.gl's Delatin
//!   tessellator, which is not ported: [`terrain_mesh`] refuses it (no Mapterhorn or Mapzen tile
//!   is one);
//! - a grid side that is not `2^n + 1` (or below 2) is an error value, not a thrown `Error`;
//! - martini's scratch index grid is the mesh call's own, not shared by every tile of a size.

use crate::geom::js_max;
use crate::hillshade::ElevationDecoder;

/// deck's `MIN_TERRAIN_MESH_MAX_ERROR`: no mesh is finer than this, metres.
pub const MIN_TERRAIN_MESH_MAX_ERROR: f64 = 1.0;
/// deck's `TILE_OVERLAP_PIXELS`: each tile's mesh reaches this many pixels (of the layer's
/// `tileSize`) into its neighbours.
pub const TILE_OVERLAP_PIXELS: f64 = 1.0;
/// deck's `MAX_LATITUDE` and `MAX_LONGITUDE`, which the globe's bounds are clamped to.
pub const MAX_LATITUDE: f64 = 90.0;
pub const MAX_LONGITUDE: f64 = 180.0;

/// A typed array's read: past the end is `undefined`, which arithmetic makes NaN.
fn at(values: &[f32], i: usize) -> f64 {
    values.get(i).map_or(f64::NAN, |&v| f64::from(v))
}

// ------------------------------------------------------------------ martini

/// mapbox/martini's `Martini`: the coordinates of every possible triangle of an RTIN tile of
/// `grid_size` points a side, by its index in the implicit binary tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Martini {
    grid_size: usize,
    num_triangles: usize,
    num_parent_triangles: usize,
    /// `ax, ay, bx, by` of each triangle (its hypotenuse; the right angle follows from them).
    coords: Vec<u16>,
}

impl Martini {
    /// The triangles of a grid `grid_size` points a side: `2^n + 1` (martini's default is 257).
    pub fn new(grid_size: usize) -> Result<Self, String> {
        let tile_size = grid_size.wrapping_sub(1);
        if grid_size < 2 || tile_size & (tile_size - 1) != 0 || tile_size > usize::from(u16::MAX) {
            return Err(format!("Expected grid size to be 2^n+1, got {grid_size}."));
        }
        let num_triangles = tile_size * tile_size * 2 - 2;
        // (the JavaScript's count is −1 for a 2-point grid: no parent at all)
        let num_parent_triangles = num_triangles.saturating_sub(tile_size * tile_size);
        let t = tile_size as i64;
        let mut coords = vec![0u16; num_triangles * 4];
        // get triangle coordinates from its index in an implicit binary tree
        for i in 0..num_triangles {
            let mut id = i + 2;
            let (mut ax, mut ay, mut bx, mut by, mut cx, mut cy) = (0i64, 0i64, 0i64, 0i64, 0i64, 0i64);
            if id & 1 != 0 {
                // bottom-left triangle
                bx = t;
                by = t;
                cx = t;
            } else {
                // top-right triangle
                ax = t;
                ay = t;
                cy = t;
            }
            loop {
                id >>= 1;
                if id <= 1 {
                    break;
                }
                let mx = (ax + bx) >> 1;
                let my = (ay + by) >> 1;
                if id & 1 != 0 {
                    // left half
                    bx = ax;
                    by = ay;
                    ax = cx;
                    ay = cy;
                } else {
                    // right half
                    ax = bx;
                    ay = by;
                    bx = cx;
                    by = cy;
                }
                cx = mx;
                cy = my;
            }
            let k = i * 4;
            coords[k] = ax as u16;
            coords[k + 1] = ay as u16;
            coords[k + 2] = bx as u16;
            coords[k + 3] = by as u16;
        }
        Ok(Martini { grid_size, num_triangles, num_parent_triangles, coords })
    }

    /// Points a side.
    pub fn grid_size(&self) -> usize {
        self.grid_size
    }

    /// The triangles of the whole hierarchy (`numTriangles`).
    pub fn num_triangles(&self) -> usize {
        self.num_triangles
    }

    /// An RTIN tile of `terrain`, `grid_size²` heights, row by row from the north-west
    /// (`createTile`): every triangle's error worked out ([`Tile::errors`]).
    pub fn create_tile(&self, terrain: Vec<f32>) -> Result<Tile<'_>, String> {
        let size = self.grid_size;
        if terrain.len() != size * size {
            return Err(format!("Expected terrain data of length {} ({size} x {size}), got {}.", size * size, terrain.len()));
        }
        let errors = vec![0f32; terrain.len()];
        let mut tile = Tile { martini: self, terrain, errors };
        tile.update();
        Ok(tile)
    }
}

/// mapbox/martini's `Tile`: a grid of heights and the error of each triangle of its hierarchy,
/// kept at the midpoint of the triangle's hypotenuse.
#[derive(Debug, Clone)]
pub struct Tile<'a> {
    martini: &'a Martini,
    terrain: Vec<f32>,
    errors: Vec<f32>,
}

/// What `getMesh` walks with: martini's two recursive closures and the state they share.
struct Walk<'a> {
    size: usize,
    errors: &'a [f32],
    max_error: f64,
    /// Each grid point's vertex index plus one, 0 for none yet.
    indices: Vec<u32>,
    num_vertices: u32,
    num_triangles: usize,
    vertices: Vec<u16>,
    triangles: Vec<u32>,
}

impl Walk<'_> {
    /// Whether the triangle is split: it is larger than one cell and its hypotenuse's midpoint is
    /// in error by more than the limit.
    fn split(&self, ax: i64, ay: i64, cx: i64, cy: i64, mx: i64, my: i64) -> bool {
        (ax - cx).abs() + (ay - cy).abs() > 1 && at(self.errors, my as usize * self.size + mx as usize) > self.max_error
    }

    /// `countElements`: find used vertices (and assign each an index), and count triangles.
    fn count(&mut self, ax: i64, ay: i64, bx: i64, by: i64, cx: i64, cy: i64) {
        let mx = (ax + bx) >> 1;
        let my = (ay + by) >> 1;
        if self.split(ax, ay, cx, cy, mx, my) {
            self.count(cx, cy, ax, ay, mx, my);
            self.count(bx, by, cx, cy, mx, my);
        } else {
            for (x, y) in [(ax, ay), (bx, by), (cx, cy)] {
                let i = y as usize * self.size + x as usize;
                if self.indices[i] == 0 {
                    self.num_vertices += 1;
                    self.indices[i] = self.num_vertices;
                }
            }
            self.num_triangles += 1;
        }
    }

    /// `processTriangle`: fill the vertices and triangles.
    fn process(&mut self, ax: i64, ay: i64, bx: i64, by: i64, cx: i64, cy: i64) {
        let mx = (ax + bx) >> 1;
        let my = (ay + by) >> 1;
        if self.split(ax, ay, cx, cy, mx, my) {
            // triangle doesn't approximate the surface well enough; drill down further
            self.process(cx, cy, ax, ay, mx, my);
            self.process(bx, by, cx, cy, mx, my);
        } else {
            // add a triangle
            let (a, b, c) = {
                let index = |x: i64, y: i64| self.indices[y as usize * self.size + x as usize] - 1;
                (index(ax, ay), index(bx, by), index(cx, cy))
            };
            for (v, x, y) in [(a, ax, ay), (b, bx, by), (c, cx, cy)] {
                self.vertices[2 * v as usize] = x as u16;
                self.vertices[2 * v as usize + 1] = y as u16;
            }
            self.triangles.extend([a, b, c]);
        }
    }
}

impl Tile<'_> {
    /// The heights, `grid_size²`, row by row.
    pub fn terrain(&self) -> &[f32] {
        &self.terrain
    }

    /// Each triangle's error, metres, at the midpoint of its hypotenuse: how far the height there
    /// is from the hypotenuse's, or any of its children's error if larger (`errors`).
    pub fn errors(&self) -> &[f32] {
        &self.errors
    }

    /// `update`: iterate over all possible triangles, starting from the smallest level.
    fn update(&mut self) {
        let Martini { num_triangles, num_parent_triangles, ref coords, grid_size: size } = *self.martini;
        let (terrain, errors) = (&self.terrain, &mut self.errors);
        for i in (0..num_triangles).rev() {
            let k = i * 4;
            let ax = i64::from(coords[k]);
            let ay = i64::from(coords[k + 1]);
            let bx = i64::from(coords[k + 2]);
            let by = i64::from(coords[k + 3]);
            let mx = (ax + bx) >> 1;
            let my = (ay + by) >> 1;
            let cx = mx + my - ay;
            let cy = my + ax - mx;
            let index = |x: i64, y: i64| y as usize * size + x as usize;
            // calculate error in the middle of the long edge of the triangle
            let interpolated_height = (at(terrain, index(ax, ay)) + at(terrain, index(bx, by))) / 2.0;
            let middle_index = index(mx, my);
            let middle_error = (interpolated_height - at(terrain, middle_index)).abs();
            errors[middle_index] = js_max(f64::from(errors[middle_index]), middle_error) as f32;
            if i < num_parent_triangles {
                // bigger triangles; accumulate error with children
                let left_child_index = index((ax + cx) >> 1, (ay + cy) >> 1);
                let right_child_index = index((bx + cx) >> 1, (by + cy) >> 1);
                let most = js_max(js_max(f64::from(errors[middle_index]), at(errors, left_child_index)), at(errors, right_child_index));
                errors[middle_index] = most as f32;
            }
        }
    }

    /// `getMesh(maxError)`: the coarsest triangles within `max_error` metres, as grid points
    /// (`x, y` pairs, the north-west corner first) and three vertex indices a triangle.
    pub fn get_mesh(&self, max_error: f64) -> (Vec<u16>, Vec<u32>) {
        let size = self.martini.grid_size;
        let max = (size - 1) as i64;
        let mut walk = Walk {
            size,
            errors: &self.errors,
            max_error,
            indices: vec![0; size * size],
            num_vertices: 0,
            num_triangles: 0,
            vertices: Vec::new(),
            triangles: Vec::new(),
        };
        // retrieve mesh in two stages that both traverse the error map:
        // - count: find used vertices (and assign each an index), and count triangles
        // - process: fill the vertices and triangles
        walk.count(0, 0, max, max, max, 0);
        walk.count(max, max, 0, 0, 0, max);
        walk.vertices = vec![0; walk.num_vertices as usize * 2];
        walk.triangles = Vec::with_capacity(walk.num_triangles * 3);
        walk.process(0, 0, max, max, max, 0);
        walk.process(max, max, 0, 0, 0, max);
        (walk.vertices, walk.triangles)
    }
}

// ------------------------------------------------------------------ loaders.gl's TerrainLoader

/// `getTerrain`: the heights of `width × height` RGBA pixels by `decoder`, in a grid one point
/// wider and higher (`terrain[y·(w+1) + x]`); for martini (`martini`) its last row and column
/// back-filled from those before them, as the JavaScript writes it (for a square tile).
pub fn terrain_grid(rgba: &[u8], width: usize, height: usize, decoder: &ElevationDecoder, martini: bool) -> Vec<f32> {
    let ElevationDecoder { r_scaler, g_scaler, b_scaler, offset } = *decoder;
    let byte = |k: usize| rgba.get(k).map_or(f64::NAN, |&b| f64::from(b));
    let mut terrain = vec![0f32; (width + 1) * (height + 1)];
    // (a typed array ignores a write past its end)
    let put = |terrain: &mut Vec<f32>, i: usize, v: f64| {
        if let Some(t) = terrain.get_mut(i) {
            *t = v as f32;
        }
    };
    // decode terrain values
    let mut i = 0;
    for y in 0..height {
        for _x in 0..width {
            let k = i * 4;
            let (r, g, b) = (byte(k), byte(k + 1), byte(k + 2));
            put(&mut terrain, i + y, r * r_scaler + g * g_scaler + b * b_scaler + offset);
            i += 1;
        }
    }
    if martini {
        // backfill bottom border
        for i in (width + 1) * width..(width + 1) * width + width {
            let v = at(&terrain, i - width - 1);
            put(&mut terrain, i, v);
        }
        // backfill right border
        let mut i = height;
        for _y in 0..height + 1 {
            let v = if i == 0 { f64::NAN } else { at(&terrain, i - 1) };
            put(&mut terrain, i, v);
            i += height + 1;
        }
    }
    terrain
}

/// A mesh's vertices (`POSITION`, `TEXCOORD_0`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshAttributes {
    /// `x, y, z`: within the bounds (y north) and metres up, `f32` as the loader's arrays.
    pub positions: Vec<f32>,
    /// `u, v`: the image's, 0 to 1 (v down).
    pub tex_coords: Vec<f32>,
}

/// `getMeshAttributes`: martini's grid points (`x, y` pairs) placed within `bounds`
/// (`[minX, minY, maxX, maxY]`, y north; `None`: the pixels, `[0, 0, width, height]`) at the
/// grid's heights.
pub fn mesh_attributes(vertices: &[u16], terrain: &[f32], width: usize, height: usize, bounds: Option<[f64; 4]>) -> MeshAttributes {
    let grid_size = width + 1;
    let count = vertices.len() / 2;
    let [min_x, min_y, max_x, max_y] = bounds.unwrap_or([0.0, 0.0, width as f64, height as f64]);
    let x_scale = (max_x - min_x) / width as f64;
    let y_scale = (max_y - min_y) / height as f64;
    let mut positions = vec![0f32; count * 3];
    let mut tex_coords = vec![0f32; count * 2];
    for i in 0..count {
        let x = f64::from(vertices[i * 2]);
        let y = f64::from(vertices[i * 2 + 1]);
        let pixel = vertices[i * 2 + 1] as usize * grid_size + vertices[i * 2] as usize;
        positions[3 * i] = (x * x_scale + min_x) as f32;
        positions[3 * i + 1] = (-y * y_scale + max_y) as f32;
        positions[3 * i + 2] = at(terrain, pixel) as f32;
        tex_coords[2 * i] = (x / width as f64) as f32;
        tex_coords[2 * i + 1] = (y / height as f64) as f32;
    }
    MeshAttributes { positions, tex_coords }
}

/// `getMeshBoundingBox` (`@loaders.gl/schema-utils`): the least and greatest `x, y, z` of the
/// positions; ±∞ for none.
pub fn bounding_box(positions: &[f32]) -> [[f64; 3]; 2] {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for p in positions.chunks_exact(3) {
        for c in 0..3 {
            let v = f64::from(p[c]);
            min[c] = if v < min[c] { v } else { min[c] };
            max[c] = if v > max[c] { v } else { max[c] };
        }
    }
    [min, max]
}

/// `getOutsideEdgesFromTriangles`: the edges only one triangle has, in the order of a stable sort
/// by their lower and then their higher vertex.
pub fn outside_edges(triangles: &[u32]) -> Vec<[u32; 2]> {
    let mut edges: Vec<[u32; 2]> = Vec::with_capacity(triangles.len());
    for t in triangles.chunks_exact(3) {
        edges.push([t[0], t[1]]);
        edges.push([t[1], t[2]]);
        edges.push([t[2], t[0]]);
    }
    let low = |e: &[u32; 2]| e[0].min(e[1]);
    let high = |e: &[u32; 2]| e[0].max(e[1]);
    edges.sort_by(|a, b| low(a).cmp(&low(b)).then(high(a).cmp(&high(b))));
    let mut outside = Vec::new();
    let mut index = 0;
    while index < edges.len() {
        let next = edges.get(index + 1);
        if next.is_some_and(|n| edges[index][0] == n[1] && edges[index][1] == n[0]) {
            index += 2;
        } else {
            outside.push(edges[index]);
            index += 1;
        }
    }
    outside
}

/// `addSkirt`: for each outside edge two new vertices `skirt_height` metres below its ends (same
/// texture coordinates) and two triangles down to them, after the mesh's own.
pub fn add_skirt(attributes: &mut MeshAttributes, triangles: &mut Vec<u32>, skirt_height: f64) {
    let edges = outside_edges(triangles);
    let positions_length = attributes.positions.len();
    let base = (positions_length / 3) as u32;
    let mut new_position = vec![0f32; edges.len() * 6];
    let mut new_tex_coord = vec![0f32; edges.len() * 4];
    let mut new_triangles = vec![0u32; edges.len() * 6];
    let (positions, tex_coords) = (&attributes.positions, &attributes.tex_coords);
    for (edge_index, edge) in edges.iter().enumerate() {
        let vertex1 = edge_index * 2;
        let vertex2 = edge_index * 2 + 1;
        for (vertex, end) in [(vertex1, edge[0] as usize), (vertex2, edge[1] as usize)] {
            new_position[vertex * 3..vertex * 3 + 3].copy_from_slice(&positions[end * 3..end * 3 + 3]);
            // put down elevation on the skirt height
            new_position[vertex * 3 + 2] = (f64::from(new_position[vertex * 3 + 2]) - skirt_height) as f32;
            // same texture coordinates for the skirt's vertices
            new_tex_coord[vertex * 2..vertex * 2 + 2].copy_from_slice(&tex_coords[end * 2..end * 2 + 2]);
        }
        let t = edge_index * 2 * 3;
        new_triangles[t] = edge[0];
        new_triangles[t + 1] = base + vertex2 as u32;
        new_triangles[t + 2] = edge[1];
        new_triangles[t + 3] = base + vertex2 as u32;
        new_triangles[t + 4] = edge[0];
        new_triangles[t + 5] = base + vertex1 as u32;
    }
    attributes.positions.extend_from_slice(&new_position);
    attributes.tex_coords.extend_from_slice(&new_tex_coord);
    triangles.extend_from_slice(&new_triangles);
}

/// The loader's terrain options (`loadOptions.terrain`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainOptions {
    /// Metres (deck hands over [`effective_mesh_max_error`]).
    pub mesh_max_error: f64,
    /// `[minX, minY, maxX, maxY]`, y north; `None`: the pixels.
    pub bounds: Option<[f64; 4]>,
    pub elevation_decoder: ElevationDecoder,
    /// Metres; 0: no skirt.
    pub skirt_height: f64,
}

/// A height tile's mesh as the loader hands it over.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainMesh {
    /// The surface's vertices, then the skirt's.
    pub attributes: MeshAttributes,
    /// Three vertex indices a triangle (`indices`): the surface's, then the skirt's.
    pub indices: Vec<u32>,
    /// The surface's vertices before the skirt (`header.boundingBox`): its `z` range is the ground's
    /// heights in the tile, which deck's `zRange` grows from.
    pub bounding_box: [[f64; 3]; 2],
    /// Martini's vertices and triangles, without the skirt (`vertexCount` is all of `indices`).
    pub surface_vertices: usize,
    pub surface_triangles: usize,
}

/// `makeTerrainMeshFromImage` with the `auto` tessellator: martini for a square tile whose side is
/// a power of two, the only kind there is here (else an error: Delatin is not ported).
pub fn terrain_mesh(rgba: &[u8], width: usize, height: usize, options: &TerrainOptions) -> Result<TerrainMesh, String> {
    if !(width == height && width > 0 && height & (width - 1) == 0) {
        return Err(format!("a {width} x {height} terrain tile is not square with a power-of-two side (loaders.gl's Delatin is not ported)"));
    }
    let terrain = terrain_grid(rgba, width, height, &options.elevation_decoder, true);
    let martini = Martini::new(width + 1)?;
    let tile = martini.create_tile(terrain)?;
    let (vertices, mut triangles) = tile.get_mesh(options.mesh_max_error);
    let mut attributes = mesh_attributes(&vertices, tile.terrain(), width, height, options.bounds);
    // the bounding box before the skirt, so that its heights are not skewed
    let bounding_box = bounding_box(&attributes.positions);
    let (surface_vertices, surface_triangles) = (vertices.len() / 2, triangles.len() / 3);
    // (`if (terrainOptions.skirtHeight)`: 0 and NaN add none)
    if options.skirt_height != 0.0 && !options.skirt_height.is_nan() {
        add_skirt(&mut attributes, &mut triangles, options.skirt_height);
    }
    Ok(TerrainMesh { attributes, indices: triangles, bounding_box, surface_vertices, surface_triangles })
}

// ------------------------------------------------------------------ deck.gl's TerrainLayer

/// `getEffectiveMeshMaxError`: the layer's `meshMaxError`, never below
/// [`MIN_TERRAIN_MESH_MAX_ERROR`] (that, for one that is not a positive number).
pub fn effective_mesh_max_error(mesh_max_error: f64) -> f64 {
    if !mesh_max_error.is_finite() || mesh_max_error <= 0.0 {
        return MIN_TERRAIN_MESH_MAX_ERROR;
    }
    js_max(mesh_max_error, MIN_TERRAIN_MESH_MAX_ERROR)
}

/// The skirt `loadTerrain` asks for: twice the effective error for tiled data, else none.
pub fn skirt_height(mesh_max_error: f64, tiled: bool) -> f64 {
    if tiled { effective_mesh_max_error(mesh_max_error) * 2.0 } else { 0.0 }
}

/// `getOverlappedBounds`: `bounds` grown by [`TILE_OVERLAP_PIXELS`] of a `tile_size`-pixel tile on
/// every side; on the globe (`clamp_lng_lat`, bounds in degrees) kept within ±180° and ±90°.
pub fn overlapped_bounds(bounds: [f64; 4], tile_size: f64, clamp_lng_lat: bool) -> [f64; 4] {
    let x_pad = ((bounds[2] - bounds[0]) / tile_size) * TILE_OVERLAP_PIXELS;
    let y_pad = ((bounds[3] - bounds[1]) / tile_size) * TILE_OVERLAP_PIXELS;
    let overlapped = [bounds[0] - x_pad, bounds[1] - y_pad, bounds[2] + x_pad, bounds[3] + y_pad];
    if !clamp_lng_lat {
        return overlapped;
    }
    [
        js_max(overlapped[0], -MAX_LONGITUDE),
        js_max(overlapped[1], -MAX_LATITUDE),
        crate::geom::js_min(overlapped[2], MAX_LONGITUDE),
        crate::geom::js_min(overlapped[3], MAX_LATITUDE),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hillshade::TERRARIUM_DECODER;

    /// Terrarium RGBA for heights given in 256ths of a metre above −32768 m (`R·256 + G + B/256`).
    fn terrarium(width: usize, height: usize, units: impl Fn(usize, usize) -> u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                let v = units(x, y);
                rgba.extend([(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]);
            }
        }
        rgba
    }

    /// `metres` above sea level in 256ths above the terrarium offset.
    fn units(metres: f64) -> u32 {
        ((metres + 32768.0) * 256.0).round() as u32
    }

    /// A ridge running north to south at column 300 of a 512 px tile, falling 7 m a pixel to the
    /// east and west and 3 m a pixel from row 200, with a roughness of up to 1 m: whole 256ths, so
    /// JavaScript and Rust read the same heights.
    fn ridge(x: usize, y: usize) -> u32 {
        let (x, y) = (x as i64, y as i64);
        let metres = 2000 - 7 * (x - 300).abs() - 3 * (y - 200).abs();
        let rough = (x * y * 31) % 17;
        (((metres + 32768) * 256) + rough * 16) as u32
    }

    /// The flat 512 px tile through the whole loader: two triangles, four vertices at the corners,
    /// the skirt round its four sides.
    #[test]
    fn a_flat_tile_is_two_triangles() {
        let rgba = terrarium(512, 512, |_, _| units(1608.0));
        let terrain = terrain_grid(&rgba, 512, 512, &TERRARIUM_DECODER, true);
        assert_eq!(terrain.len(), 513 * 513);
        assert!(terrain.iter().all(|&h| h == 1608.0));
        let martini = Martini::new(513).expect("2^9 + 1");
        let tile = martini.create_tile(terrain).expect("513²");
        assert!(tile.errors().iter().all(|&e| e == 0.0));
        let (vertices, triangles) = tile.get_mesh(2.0);
        assert_eq!(vertices, vec![0, 0, 512, 512, 512, 0, 0, 512]);
        assert_eq!(triangles, vec![0, 1, 2, 1, 0, 3]);
        // with no error allowed, still two
        assert_eq!(tile.get_mesh(0.0).1.len(), 6);
        let options = TerrainOptions { mesh_max_error: 2.0, bounds: Some([0.0, 0.0, 1.0, 1.0]), elevation_decoder: TERRARIUM_DECODER, skirt_height: 4.0 };
        let mesh = terrain_mesh(&rgba, 512, 512, &options).expect("a mesh");
        assert_eq!((mesh.surface_vertices, mesh.surface_triangles), (4, 2));
        assert_eq!(mesh.bounding_box, [[0.0, 0.0, 1608.0], [1.0, 1.0, 1608.0]]);
        // four outside edges, two skirt vertices and two triangles each
        assert_eq!(mesh.attributes.positions.len(), (4 + 8) * 3);
        assert_eq!(mesh.indices.len(), (2 + 8) * 3);
        assert!(mesh.attributes.positions[12..].chunks(3).all(|p| p[2] == 1604.0));
        // the north-west corner is (0, 1) north up, texture (0, 0)
        assert_eq!(&mesh.attributes.positions[..3], &[0.0, 1.0, 1608.0]);
        assert_eq!(&mesh.attributes.tex_coords[..2], &[0.0, 0.0]);
    }

    /// The mesh on a ridge at 2 m, martini's bound: every triangle kept that is larger than one
    /// cell has the error at its hypotenuse's midpoint (which holds the largest of its own and its
    /// descendants') within 2 m, and every triangle kept but the two roots came from a parent whose
    /// error was over 2 m (the coarsest that are within it); the triangles cover every grid point
    /// once their edges are counted, and a larger error gives fewer triangles.
    ///
    /// Martini measures each midpoint against its own level's hypotenuse, not against the triangle
    /// finally kept, so the surface can stray a little further from a grid point under a large
    /// triangle: 2.6 m at most here (checked below 3 m).
    #[test]
    fn the_error_bound_holds_on_a_ridge() {
        let size = 512;
        let rgba = terrarium(size, size, ridge);
        let terrain = terrain_grid(&rgba, size, size, &TERRARIUM_DECODER, true);
        let martini = Martini::new(size + 1).expect("2^9 + 1");
        let tile = martini.create_tile(terrain.clone()).expect("513²");
        let grid = size + 1;
        let (vertices, triangles) = tile.get_mesh(2.0);
        let point = |v: u32| (i64::from(vertices[2 * v as usize]), i64::from(vertices[2 * v as usize + 1]));
        let height = |x: i64, y: i64| f64::from(terrain[y as usize * grid + x as usize]);
        let mut checked = 0usize;
        let mut worst: f64 = 0.0;
        let mut covered = vec![false; grid * grid];
        for t in triangles.chunks_exact(3) {
            let [a, b, c] = [point(t[0]), point(t[1]), point(t[2])];
            // martini's triangles: a and b the hypotenuse, c the right angle
            let (mx, my) = ((a.0 + b.0) >> 1, (a.1 + b.1) >> 1);
            if (a.0 - c.0).abs() + (a.1 - c.1).abs() > 1 {
                assert!(f64::from(tile.errors()[my as usize * grid + mx as usize]) <= 2.0);
            }
            // its parent's hypotenuse had its midpoint at the right angle, and was split
            let root = c == (512, 0) || c == (0, 512);
            assert!(root || f64::from(tile.errors()[c.1 as usize * grid + c.0 as usize]) > 2.0);
            // the plane through the three corners at each grid point inside or on the triangle
            let area = ((b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)) as f64;
            let (x0, x1) = (a.0.min(b.0).min(c.0), a.0.max(b.0).max(c.0));
            let (y0, y1) = (a.1.min(b.1).min(c.1), a.1.max(b.1).max(c.1));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let wa = ((b.0 - x) * (c.1 - y) - (b.1 - y) * (c.0 - x)) as f64 / area;
                    let wb = ((c.0 - x) * (a.1 - y) - (c.1 - y) * (a.0 - x)) as f64 / area;
                    let wc = 1.0 - wa - wb;
                    if wa < -1e-12 || wb < -1e-12 || wc < -1e-12 {
                        continue;
                    }
                    let surface = wa * height(a.0, a.1) + wb * height(b.0, b.1) + wc * height(c.0, c.1);
                    worst = worst.max((surface - height(x, y)).abs());
                    covered[y as usize * grid + x as usize] = true;
                    checked += 1;
                }
            }
        }
        assert!(covered.iter().all(|&c| c), "every grid point lies in a triangle");
        eprintln!("{} triangles, {} vertices, {checked} points, worst {worst:.3} m", triangles.len() / 3, vertices.len() / 2);
        assert!(worst < 3.0, "worst {worst} m");
        let coarser = tile.get_mesh(10.0).1.len();
        assert!(coarser < triangles.len() && triangles.len() < tile.get_mesh(0.5).1.len());
    }

    /// A bowl in the west of a 256 px tile (Mapzen's size), below sea level at its bottom, with
    /// fractions of a metre: whole 256ths, as `golden.mjs` makes it.
    fn valley(x: usize, y: usize) -> u32 {
        let (x, y) = (x as i64, y as i64);
        let d = (x - 40) * (x - 40) + (y - 200) * (y - 200);
        let metres = d.div_euclid(9) - 600;
        ((metres + 32768) * 256 + (x * 7 + y * 13) % 256) as u32
    }

    /// FNV-1a over little-endian 32-bit words, as `golden.mjs` hashes its arrays.
    fn fnv(words: impl IntoIterator<Item = u32>) -> u32 {
        let mut h: u32 = 0x811c_9dc5;
        for w in words {
            for s in (0..32).step_by(8) {
                h ^= (w >> s) & 255;
                h = h.wrapping_mul(0x0100_0193);
            }
        }
        h
    }

    /// The whole loader against loaders.gl 4.5.3's own `makeTerrainMeshFromImage` (with
    /// `@mapbox/martini` 0.2.0) run under bun on the same two tiles: a 512 px ridge falling below
    /// sea level and a 256 px bowl, each at 2 and 5 m with deck's 4 m skirt and one pixel of
    /// overlap: the same number of every array and the same bits in each (FNV-1a of the `f32`
    /// positions and texture coordinates and of the indices), and the same bounding box.
    #[test]
    fn meshes_are_the_loaders_bit_for_bit() {
        // (name, side, max error, positions, texture coordinates, indices, their hashes, box z)
        type Golden = (&'static str, usize, f64, [usize; 3], [u32; 3], [f64; 2]);
        let goldens: [Golden; 4] = [
            ("ridge", 512, 2.0, [9657, 6438, 15555], [2246464215, 64139608, 4268724506], [-1033.0, 2000.8125]),
            ("ridge", 512, 5.0, [2223, 1482, 3990], [3556876068, 121097606, 2789363523], [-1033.0, 2000.8125]),
            ("valley", 256, 2.0, [12246, 8164, 21480], [1710222862, 671728442, 114860121], [-599.75, 8980.97265625]),
            ("valley", 256, 5.0, [8496, 5664, 14034], [2796039480, 571200429, 1192977957], [-599.75, 8980.97265625]),
        ];
        let pad = 1.0 / 512.0;
        for (name, size, error, lengths, hashes, z) in goldens {
            let rgba = if name == "ridge" { terrarium(size, size, ridge) } else { terrarium(size, size, valley) };
            let options = TerrainOptions {
                mesh_max_error: error,
                bounds: Some([-pad, -pad, 1.0 + pad, 1.0 + pad]),
                elevation_decoder: TERRARIUM_DECODER,
                skirt_height: 4.0,
            };
            let mesh = terrain_mesh(&rgba, size, size, &options).expect("a mesh");
            let MeshAttributes { positions, tex_coords } = &mesh.attributes;
            assert_eq!([positions.len(), tex_coords.len(), mesh.indices.len()], lengths, "{name} at {error} m");
            let bits = |v: &[f32]| v.iter().map(|f| f.to_bits()).collect::<Vec<u32>>();
            assert_eq!([fnv(bits(positions)), fnv(bits(tex_coords)), fnv(mesh.indices.iter().copied())], hashes, "{name} at {error} m");
            assert_eq!(mesh.bounding_box, [[-pad, -pad, z[0]], [1.0 + pad, 1.0 + pad, z[1]]], "{name} at {error} m");
        }
    }

    /// The grid: the pixels decoded with the terrarium decoder in `f32`, the last row a copy of the
    /// row above, the last column of the column before (the corner the last pixel's).
    #[test]
    fn the_grid_is_back_filled() {
        let rgba = terrarium(4, 4, |x, y| units(f64::from((10 * y + x) as u32)) + 128);
        let grid = terrain_grid(&rgba, 4, 4, &TERRARIUM_DECODER, true);
        let at = |x: usize, y: usize| grid[y * 5 + x];
        assert_eq!(at(0, 0), 0.5);
        assert_eq!(at(3, 2), 23.5);
        assert_eq!(at(2, 4), at(2, 3));
        assert_eq!(at(4, 1), at(3, 1));
        assert_eq!(at(4, 4), 33.5);
        // without the back-fill the border stays 0
        assert_eq!(terrain_grid(&rgba, 4, 4, &TERRARIUM_DECODER, false)[24], 0.0);
    }

    /// Grids martini cannot take, and tiles the loader's martini path cannot.
    #[test]
    fn grid_sizes_are_two_to_the_n_plus_one() {
        assert!(Martini::new(257).is_ok() && Martini::new(513).is_ok() && Martini::new(2).is_ok());
        assert_eq!(Martini::new(512), Err("Expected grid size to be 2^n+1, got 512.".into()));
        assert!(Martini::new(1).is_err() && Martini::new(0).is_err());
        let martini = Martini::new(5).expect("5");
        assert_eq!(martini.num_triangles(), 30);
        assert!(martini.create_tile(vec![0.0; 24]).is_err());
        let options = TerrainOptions { mesh_max_error: 2.0, bounds: None, elevation_decoder: TERRARIUM_DECODER, skirt_height: 0.0 };
        assert!(terrain_mesh(&[0; 4 * 300 * 300], 300, 300, &options).is_err());
        assert!(terrain_mesh(&[0; 4 * 256 * 128], 256, 128, &options).is_err());
    }

    /// deck's options: the error never below 1 m, the skirt twice it for tiles, the bounds one
    /// pixel of the tile size wider on each side (clamped on the globe).
    #[test]
    fn deck_hands_the_loader_its_options() {
        assert_eq!(effective_mesh_max_error(2.0), 2.0);
        assert_eq!(effective_mesh_max_error(0.5), 1.0);
        assert_eq!(effective_mesh_max_error(0.0), 1.0);
        assert_eq!(effective_mesh_max_error(f64::NAN), 1.0);
        assert_eq!(effective_mesh_max_error(f64::INFINITY), 1.0);
        assert_eq!(skirt_height(2.0, true), 4.0);
        assert_eq!(skirt_height(2.0, false), 0.0);
        assert_eq!(overlapped_bounds([0.0, 0.0, 512.0, 256.0], 512.0, false), [-1.0, -0.5, 513.0, 256.5]);
        assert_eq!(overlapped_bounds([-180.0, 80.0, 0.0, 90.0], 512.0, true), [-180.0, 80.0 - 10.0 / 512.0, 180.0 / 512.0, 90.0]);
    }
}
