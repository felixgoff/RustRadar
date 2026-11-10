//! Port of `radar-mesh.ts` (S1.27; doc 02 §3.1, §3.4): the NWS palette, band polygons, their
//! triangulation (`buildMesh`), the walls (`buildWalls`, `polygonWalls`), the contouring of a
//! reflectivity grid (`contourBands`), `simplifyLine`, `mergeMeshes` and `blur`.
//!
//! Every source ends up as reflectivity bands of 5 dBZ in the NWS palette, triangulated once on
//! load, so nothing is re-tessellated as the camera moves. Marching squares and the even-odd
//! fill are in [`super::iso`].
//!
//! Deck.gl's binary `PolygonData` becomes [`MergedMesh`] (plain arrays: in wgpu, a band range of
//! a [`RadarMesh`] is just an index sub-range with a base vertex). Triangles come from
//! [`crate::geom::earcut`] (a port of earcut 2.2.4): math.gl's earcut's own. Deviations, all
//! where the TypeScript would throw: [`parse_volume_polygons`] returns an error for a truncated
//! blob.

use rustradar_core::num::{js_round, js_string};

use super::iso::{IsoRings, even_odd, ring_area, ring_box};
use crate::geom::earcut::earcut;
use crate::geom::grid_cut::{GridOptions, cut_polygon_by_grid};
use crate::geom::{js_hypot2, js_max, js_min};

/// The usual NWS reflectivity colours, in 5 dBZ steps from 5 dBZ: band `i` covers
/// `[5 + 5i, 10 + 5i)` dBZ, the last one everything above (doc 02 §3.1).
///
/// The same table as the private `PALETTE` of `backend::radar` (`RS:radar.rs:110`), which bands
/// the measured volumes. The backend cannot depend on this crate; until the constant moves to
/// `rustradar-core`, a test here reads the backend's source and checks the two are equal.
pub const PALETTE: [[u8; 3]; 14] = [
    [0x04, 0xe9, 0xe7],
    [0x01, 0x9f, 0xf4],
    [0x03, 0x00, 0xf4],
    [0x02, 0xfd, 0x02],
    [0x01, 0xc5, 0x01],
    [0x00, 0x8e, 0x00],
    [0xfd, 0xf8, 0x02],
    [0xe5, 0xbc, 0x00],
    [0xfd, 0x95, 0x00],
    [0xfd, 0x00, 0x00],
    [0xd4, 0x00, 0x00],
    [0xbc, 0x00, 0x00],
    [0xf8, 0x00, 0xfd],
    [0x98, 0x54, 0xc6],
];

/// Number of reflectivity bands.
pub const BAND_COUNT: usize = PALETTE.len();

/// The lowest reflectivity (dBZ) in a band.
pub fn band_dbz(band: usize) -> f64 {
    5.0 + 5.0 * band as f64
}

/// The band's RGBA: light echoes are translucent, so the map shows through drizzle
/// (`clamp(150 + (dBZ − 5)·4, 150, 235)`, as `color()` in `backend::radar`).
pub fn band_color(band: usize) -> [u8; 4] {
    let alpha = js_min(235.0, js_max(150.0, 150.0 + (band_dbz(band) - 5.0) * 4.0));
    let [r, g, b] = PALETTE[band];
    [r, g, b, alpha as u8]
}

/// One filled polygon of a single band: flat `[x0, y0, x1, y1, …]` positions, the outer ring
/// first, then the holes, which start at the vertex indices in `hole_indices` (earcut's
/// convention).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BandPolygon {
    pub band: usize,
    pub positions: Vec<f64>,
    pub hole_indices: Vec<usize>,
}

/// A measured volume's blob ended before its counts said it would.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Truncated {
    /// Where the missing bytes start.
    pub offset: usize,
    /// The blob's length.
    pub length: usize,
}

impl std::fmt::Display for Truncated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "radar polygons truncated at byte {} (of {})", self.offset, self.length)
    }
}

impl std::error::Error for Truncated {}

/// Parses a measured volume level from the backend (`parseVolumePolygons`). Little-endian:
/// `u32 polygon_count`, then per polygon `u8 band, u8 + u16 reserved, u32 ring_count` and per
/// ring `u32 point_count` followed by that many `f32 lon, f32 lat`. Ring 0 is the outer ring,
/// the rest are holes; rings are not closed. An empty blob means no echo. Polygons of a band
/// past the palette, without rings or with an outer ring under 3 points are dropped, as are holes
/// under 3 points.
pub fn parse_volume_polygons(bytes: &[u8]) -> Result<Vec<BandPolygon>, Truncated> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let length = bytes.len();
    let need = |offset: usize, count: usize| {
        if offset.checked_add(count).is_none_or(|end| end > length) { Err(Truncated { offset, length }) } else { Ok(()) }
    };
    let u32_at = |offset: usize| u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]);
    need(0, 4)?;
    let count = u32_at(0);
    let mut offset = 4;
    let mut polygons = Vec::new();
    for _ in 0..count {
        need(offset, 8)?;
        let band = usize::from(bytes[offset]);
        let ring_count = u32_at(offset + 4);
        offset += 8;
        let mut rings: Vec<Vec<f64>> = Vec::new();
        for _ in 0..ring_count {
            need(offset, 4)?;
            let points = u32_at(offset) as usize;
            offset += 4;
            need(offset, points.saturating_mul(8))?;
            let ring: Vec<f64> = bytes[offset..offset + points * 8]
                .chunks_exact(4)
                .map(|b| f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])))
                .collect();
            offset += points * 8;
            rings.push(ring);
        }
        // a hole too small to enclose anything is dropped; so is a polygon whose outer ring is
        if band >= BAND_COUNT || rings.is_empty() || rings[0].len() < 6 {
            continue;
        }
        let mut positions = Vec::new();
        let mut hole_indices = Vec::new();
        for (i, ring) in rings.iter().enumerate() {
            if i > 0 && ring.len() < 6 {
                continue;
            }
            if i > 0 {
                hole_indices.push(positions.len() / 2);
            }
            positions.extend_from_slice(ring);
        }
        polygons.push(BandPolygon { band, positions, hole_indices });
    }
    Ok(polygons)
}

/// Triangles of all bands, sorted by band so a band and everything above it is one run of
/// `indices`, and its vertices one run at the end of `positions` (`RadarMesh`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RadarMesh {
    /// Values per vertex in `positions`: 2 for the flat slices, 3 for walls.
    pub size: usize,
    /// Longitude, latitude (and for walls, 0 at the foot or 1 at the top).
    pub positions: Vec<f32>,
    /// RGBA per vertex.
    pub colors: Vec<u8>,
    pub indices: Vec<u32>,
    /// Where each band's triangles start in `indices`; `BAND_COUNT + 1` entries.
    pub band_starts: [u32; BAND_COUNT + 1],
    /// Where each band's vertices start (in vertices, not values); `BAND_COUNT + 1` entries.
    pub band_vertex_starts: [u32; BAND_COUNT + 1],
}

impl RadarMesh {
    /// Number of vertices.
    pub fn vertex_count(&self) -> usize {
        self.positions.len().checked_div(self.size).unwrap_or(0)
    }

    /// The index and vertex ranges of the bands from `from_band` up (clamped to the bands):
    /// `(first index, first vertex)`. A layer that keeps only the stronger bands draws
    /// `indices[first index..]`, whose values all lie at or past the first vertex.
    pub fn band_range(&self, from_band: usize) -> (usize, usize) {
        let band = from_band.min(BAND_COUNT);
        (self.band_starts[band] as usize, self.band_vertex_starts[band] as usize)
    }
}

/// Edges are drawn straight through the globe, not along it: anything wider than this (degrees)
/// is cut up first, as deck.gl does for its own polygons, so the radar doesn't sink under the
/// curve of the earth. Cutting also keeps earcut fast on continent-wide polygons with thousands
/// of holes.
pub const GRID_DEG: f64 = 4.0;

/// [`build_mesh_on_grid`] with the [`GRID_DEG`] grid (`buildMesh(polygons)`).
pub fn build_mesh(polygons: &[BandPolygon]) -> RadarMesh {
    build_mesh_on_grid(polygons, GRID_DEG)
}

/// The polygons triangulated, in band order (`buildMesh(polygons, gridDeg)`): a polygon whose
/// box spans more than one cell of a `grid_deg` grid is cut on that grid first. Positions are
/// stored as `f32`; each vertex has its band's colour.
pub fn build_mesh_on_grid(polygons: &[BandPolygon], grid_deg: f64) -> RadarMesh {
    let mut sorted: Vec<&BandPolygon> = polygons.iter().collect();
    sorted.sort_by_key(|p| p.band);
    // triangulated first and copied out once, into arrays of the right size
    struct Piece {
        band: usize,
        positions: Vec<f64>,
        triangles: Vec<usize>,
    }
    let mut pieces: Vec<Piece> = Vec::new();
    let mut values = 0;
    let mut triangle_count = 0;
    for polygon in sorted {
        if !needs_cut(&polygon.positions, grid_deg) {
            let triangles = earcut(&polygon.positions, &polygon.hole_indices, 2);
            if triangles.is_empty() {
                continue;
            }
            values += polygon.positions.len();
            triangle_count += triangles.len();
            pieces.push(Piece { band: polygon.band, positions: polygon.positions.clone(), triangles });
            continue;
        }
        let holes: Vec<usize> = polygon.hole_indices.iter().map(|i| i * 2).collect();
        for piece in cut_polygon_by_grid(&polygon.positions, &holes, &GridOptions::grid(grid_deg)) {
            let holes: Vec<usize> = piece.hole_indices.iter().map(|i| i / 2).collect();
            let triangles = earcut(&piece.positions, &holes, 2);
            if triangles.is_empty() {
                continue;
            }
            values += piece.positions.len();
            triangle_count += triangles.len();
            pieces.push(Piece { band: polygon.band, positions: piece.positions, triangles });
        }
    }
    let mut positions = Vec::with_capacity(values);
    let mut colors = Vec::with_capacity(values * 2);
    let mut indices = Vec::with_capacity(triangle_count);
    let mut band_starts = [0; BAND_COUNT + 1];
    let mut band_vertex_starts = [0; BAND_COUNT + 1];
    let mut band = 0;
    let mut vertex = 0_u32;
    for piece in &pieces {
        while band < piece.band {
            band += 1;
            band_starts[band] = indices.len() as u32;
            band_vertex_starts[band] = vertex;
        }
        let color = band_color(piece.band);
        positions.extend(piece.positions.iter().map(|&v| v as f32));
        let count = (piece.positions.len() / 2) as u32;
        for _ in 0..count {
            colors.extend_from_slice(&color);
        }
        indices.extend(piece.triangles.iter().map(|&t| vertex + t as u32));
        vertex += count;
    }
    while band < BAND_COUNT {
        band += 1;
        band_starts[band] = indices.len() as u32;
        band_vertex_starts[band] = vertex;
    }
    RadarMesh { size: 2, positions, colors, indices, band_starts, band_vertex_starts }
}

/// Whether the polygon's box spans more than one cell of the grid (`needsCut`). True for empty
/// positions, which then cut into nothing.
fn needs_cut(positions: &[f64], grid: f64) -> bool {
    let [x0, y0, x1, y1] = ring_box(positions);
    (x0 / grid).floor() != (x1 / grid).floor() || (y0 / grid).floor() != (y1 / grid).floor()
}

/// Several meshes' bands as one drawable: what `mergeMeshes` hands deck.gl, as plain arrays.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MergedMesh {
    /// Values per vertex, as in [`RadarMesh::size`].
    pub size: usize,
    pub positions: Vec<f32>,
    pub colors: Vec<u8>,
    pub indices: Vec<u32>,
}

/// The mesh, or only its bands from `from_band` up, as one drawable; `None` when that is nothing
/// (`polygonData`). Only the vertices those bands use are kept.
pub fn polygon_data(mesh: &RadarMesh, from_band: usize) -> Option<MergedMesh> {
    merge_meshes(&[(mesh, from_band)])
}

/// Several meshes (of the same `size`), each from its own `from_band` up, as one drawable, in
/// the order given (`mergeMeshes`). `None` when there is nothing.
pub fn merge_meshes(parts: &[(&RadarMesh, usize)]) -> Option<MergedMesh> {
    let mut size = 2;
    let mut vertices = 0;
    let mut index_count = 0;
    let mut used: Vec<(&RadarMesh, usize, usize)> = Vec::new();
    for &(mesh, from_band) in parts {
        let (index, vertex) = mesh.band_range(from_band);
        if index >= mesh.indices.len() {
            continue;
        }
        size = mesh.size;
        used.push((mesh, vertex, index));
        vertices += mesh.vertex_count() - vertex;
        index_count += mesh.indices.len() - index;
    }
    if used.is_empty() {
        return None;
    }
    let mut positions = Vec::with_capacity(vertices * size);
    let mut colors = Vec::with_capacity(vertices * 4);
    let mut indices = Vec::with_capacity(index_count);
    let mut v = 0_u32;
    for (mesh, vertex, index) in used {
        positions.extend_from_slice(&mesh.positions[vertex * size..]);
        colors.extend_from_slice(&mesh.colors[vertex * 4..]);
        // `source[k] + shift` in a Uint32Array: the shift can be negative
        let shift = v.wrapping_sub(vertex as u32);
        indices.extend(mesh.indices[index..].iter().map(|&i| i.wrapping_add(shift)));
        v += (mesh.vertex_count() - vertex) as u32;
    }
    Some(MergedMesh { size, positions, colors, indices })
}

/// The outline of one band where it borders weaker echo (or none), as an open polyline
/// `[x0, y0, x1, y1, …]`; a whole ring repeats its first point at the end. Each boundary
/// between two bands is one line, in the stronger band.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WallLine {
    pub band: usize,
    pub positions: Vec<f64>,
}

/// Walls are a little fainter than the slices they hang from: seen edge-on, they stack up.
pub const WALL_ALPHA: f64 = 0.75;

/// Vertical walls along the lines, from the ground (z = 0) to unit height (z = 1): a layer
/// stretches them between two heights (`buildWalls`). Two triangles per segment, no
/// tessellation; sorted by band like [`build_mesh`], so [`RadarMesh::band_range`] gives the walls
/// of a layer that only keeps the stronger bands. Colour: the band's, alpha × [`WALL_ALPHA`].
pub fn build_walls(lines: &[WallLine]) -> RadarMesh {
    let mut sorted: Vec<&WallLine> = lines.iter().collect();
    sorted.sort_by_key(|l| l.band);
    let points: usize = sorted.iter().map(|l| l.positions.len() / 2).sum();
    let mut positions = Vec::with_capacity(points * 6);
    let mut colors = Vec::with_capacity(points * 8);
    let mut indices = Vec::with_capacity(points * 6);
    let mut band_starts = [0; BAND_COUNT + 1];
    let mut band_vertex_starts = [0; BAND_COUNT + 1];
    let mut band = 0;
    let mut vertex = 0_u32;
    for line in sorted {
        while band < line.band {
            band += 1;
            band_starts[band] = indices.len() as u32;
            band_vertex_starts[band] = vertex;
        }
        let [r, g, b, a] = band_color(line.band);
        let alpha = js_round(f64::from(a) * WALL_ALPHA) as u8;
        let p = &line.positions;
        let mut previous: Option<u32> = None;
        for i in (0..p.len() / 2 * 2).step_by(2) {
            let (x, y) = (p[i], p[i + 1]);
            // a repeated point would only add a wall of no width
            if previous.is_some() && x == p[i - 2] && y == p[i - 1] {
                continue;
            }
            positions.extend_from_slice(&[x as f32, y as f32, 0.0, x as f32, y as f32, 1.0]);
            colors.extend_from_slice(&[r, g, b, alpha, r, g, b, alpha]);
            if let Some(previous) = previous {
                indices.extend_from_slice(&[previous, previous + 1, vertex, previous + 1, vertex + 1, vertex]);
            }
            previous = Some(vertex);
            vertex += 2;
        }
    }
    while band < BAND_COUNT {
        band += 1;
        band_starts[band] = indices.len() as u32;
        band_vertex_starts[band] = vertex;
    }
    RadarMesh { size: 3, positions, colors, indices, band_starts, band_vertex_starts }
}

/// The wall lines of a measured level ([`parse_volume_polygons`]), from the rings as they come,
/// before any grid cutting, so no wall runs along a cut (`polygonWalls`). Every outer ring is a
/// wall. Bands that touch share their boundary: the backend gives it to both, as one band's
/// outer ring and the other's hole, the same points reversed. Such a hole is left out and its
/// outer ring takes the stronger of the two bands; only a hole with nothing filling it is a wall
/// of its own.
pub fn polygon_walls(polygons: &[BandPolygon]) -> Vec<WallLine> {
    let mut lines: Vec<WallLine> = Vec::new();
    let mut outers: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for polygon in polygons {
        let end = polygon_ring_ends(polygon.positions.len(), &polygon.hole_indices)[0];
        let key = ring_key(&polygon.positions, 0, end);
        if let Some(&known) = outers.get(&key) {
            lines[known].band = lines[known].band.max(polygon.band);
            continue;
        }
        outers.insert(key, lines.len());
        lines.push(WallLine { band: polygon.band, positions: closed_ring(&polygon.positions, 0, end) });
    }
    let mut holes: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for polygon in polygons {
        let ends = polygon_ring_ends(polygon.positions.len(), &polygon.hole_indices);
        for (h, &hole) in polygon.hole_indices.iter().enumerate() {
            let (start, end) = (hole * 2, ends[h + 1]);
            let key = ring_key(&polygon.positions, start, end);
            if let Some(&known) = outers.get(&key).or_else(|| holes.get(&key)) {
                lines[known].band = lines[known].band.max(polygon.band);
                continue;
            }
            holes.insert(key, lines.len());
            lines.push(WallLine { band: polygon.band, positions: closed_ring(&polygon.positions, start, end) });
        }
    }
    lines
}

/// Where each ring of a polygon ends in its flat positions: the outer ring, then each hole.
fn polygon_ring_ends(length: usize, hole_indices: &[usize]) -> Vec<usize> {
    hole_indices.iter().map(|i| i * 2).chain([length]).collect()
}

/// The ring from `start` to `end` with its first point repeated at the end.
fn closed_ring(positions: &[f64], start: usize, end: usize) -> Vec<f64> {
    let value = |i: usize| positions.get(i).copied().unwrap_or(f64::NAN);
    let mut ring: Vec<f64> = (start..end.max(start)).map(value).collect();
    ring.push(value(start));
    ring.push(value(start + 1));
    ring
}

/// Identifies a ring whichever way round and from whichever point it starts (`ringKey`): its
/// point count, its lowest point (west first, then south) and its centroid, rounded well below
/// the data's resolution.
fn ring_key(positions: &[f64], start: usize, end: usize) -> String {
    let q = |i: usize| js_round(positions.get(i).copied().unwrap_or(f64::NAN) * 1e5);
    let (mut mx, mut my) = (f64::INFINITY, f64::INFINITY);
    let (mut sx, mut sy) = (0.0, 0.0);
    let mut i = start;
    while i < end {
        let (x, y) = (q(i), q(i + 1));
        sx += x;
        sy += y;
        if x < mx || (x == mx && y < my) {
            (mx, my) = (x, y);
        }
        i += 2;
    }
    let n = (end.saturating_sub(start) / 2) as f64;
    format!(
        "{}:{},{}:{},{}",
        js_string(n),
        js_string(mx),
        js_string(my),
        js_string(js_round(sx / n)),
        js_string(js_round(sy / n))
    )
}

/// Clips a closed flat ring to the unit square as wall lines (`clipRingToUnit`). Stretches
/// running along the square's edge are left out: that is where the grid ends, not the echo, and
/// the next tile carries on from there.
fn clip_ring_to_unit(ring: &[f64], band: usize, out: &mut Vec<WallLine>) {
    let n = ring.len() / 2;
    // the line being extended, by its place in `out`
    let mut line: Option<usize> = None;
    for i in 0..n {
        let j = (i + 1) % n;
        let (ax, ay, bx, by) = (ring[i * 2], ring[i * 2 + 1], ring[j * 2], ring[j * 2 + 1]);
        let clipped = match clip_segment(ax, ay, bx, by) {
            Some(c) if !along_edge(c) => c,
            _ => {
                line = None;
                continue;
            }
        };
        let [px, py, qx, qy] = clipped;
        // carries on from the last segment unless that one was cut short
        match line {
            Some(l) if out[l].positions[out[l].positions.len() - 2] == px && out[l].positions[out[l].positions.len() - 1] == py => {
                out[l].positions.extend_from_slice(&[qx, qy]);
            }
            _ => {
                line = Some(out.len());
                out.push(WallLine { band, positions: vec![px, py, qx, qy] });
            }
        }
    }
}

/// Liang–Barsky against the unit square: what of the segment lies inside, or `None`
/// (`clipSegment`). Ends that aren't clipped keep their exact values, so lines join up.
fn clip_segment(ax: f64, ay: f64, bx: f64, by: f64) -> Option<[f64; 4]> {
    let (dx, dy) = (bx - ax, by - ay);
    let (mut t0, mut t1) = (0.0, 1.0);
    for (p, q) in [(-dx, ax), (dx, 1.0 - ax), (-dy, ay), (dy, 1.0 - ay)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = js_max(t0, t);
            } else {
                t1 = js_min(t1, t);
            }
        }
    }
    if t0 >= t1 {
        return None;
    }
    Some([
        if t0 == 0.0 { ax } else { ax + t0 * dx },
        if t0 == 0.0 { ay } else { ay + t0 * dy },
        if t1 == 1.0 { bx } else { ax + t1 * dx },
        if t1 == 1.0 { by } else { ay + t1 * dy },
    ])
}

/// How close (in unit coordinates) both ends must be to an edge of the square for a segment to
/// run along it.
const EDGE: f64 = 1e-9;

fn along_edge([px, py, qx, qy]: [f64; 4]) -> bool {
    let on = |a: f64, b: f64, edge: f64| (a - edge).abs() < EDGE && (b - edge).abs() < EDGE;
    on(px, qx, 0.0) || on(px, qx, 1.0) || on(py, qy, 0.0) || on(py, qy, 1.0)
}

/// How far (in cells) a wall may stray from its contour: walls are simplified, the slices are not.
pub const WALL_TOLERANCE: f64 = 0.1;

/// Cells of padding round the grid in [`contour_bands`]: the edge values repeated, then nothing.
const PAD: usize = 2;

/// Contours a reflectivity grid (dBZ, row 0 at the top; anything below 5 is no echo) into one set
/// of polygons per 5 dBZ band, in unit coordinates: x from 0 (left edge of the grid) to 1 (right
/// edge), y from 0 (top) to 1 (bottom) (`contourBands`). The polygons fill exactly to the grid's
/// edges, so neighbouring grids join without a seam.
///
/// With `walls`, the outlines of the bands are added to it as well, clipped to the grid and in
/// the same coordinates, for [`build_walls`]. They come from the contours themselves, before the
/// polygons are cut into pieces, so they run only where the reflectivity changes band: each
/// threshold's rings once, in the band that starts there, and nothing along the grid's edges or
/// cuts. They are simplified to within a tenth of a cell of the contours.
pub fn contour_bands(values: &[f32], width: usize, height: usize, walls: Option<&mut Vec<WallLine>>) -> Vec<BandPolygon> {
    let mut max = f64::NEG_INFINITY;
    for &v in values {
        if f64::from(v) > max {
            max = f64::from(v);
        }
    }
    // `!(max >= 5)`: also when there is no value at all (or only NaN)
    if max.partial_cmp(&band_dbz(0)).is_none_or(|o| o.is_lt()) || width == 0 || height == 0 {
        return Vec::new();
    }

    // Two cells of padding: the edge values repeated, then nothing. Contours then run exactly as
    // they would if the grid went on, out to the repeated cells, and close in the outer ring,
    // clear of the grid. Without it, the rings would close along the grid edge itself, where the
    // rings of neighbouring thresholds would overlap.
    let w = width + 2 * PAD;
    let h = height + 2 * PAD;
    let mut padded = vec![0.0_f32; w * h];
    for y in 1..h - 1 {
        let sy = (y as isize - PAD as isize).clamp(0, height as isize - 1) as usize;
        for x in 1..w - 1 {
            let sx = (x as isize - PAD as isize).clamp(0, width as isize - 1) as usize;
            padded[y * w + x] = values[sy * width + sx];
        }
    }

    let thresholds: Vec<f64> = (0..BAND_COUNT).map(band_dbz).take_while(|&t| t <= max).collect();
    let mut tracer = IsoRings::new(w, h);
    let (wf, hf, padf) = (width as f64, height as f64, PAD as f64);
    // rings of everything at or above each threshold, in unit coordinates
    let above: Vec<Vec<Vec<f64>>> = thresholds
        .iter()
        .map(|&t| {
            let mut rings = Vec::new();
            tracer.trace(&padded, t, |ring| {
                let flat: Vec<f64> = ring
                    .chunks_exact(2)
                    .flat_map(|p| [(p[0] - padf) / wf, (p[1] - padf) / hf])
                    .collect();
                // specks a fraction of a cell across are noise; dropped from the threshold, they
                // vanish from both bands that share the ring
                if ring_area(&flat).abs() * wf * hf >= 0.3 {
                    rings.push(flat);
                }
            });
            rings
        })
        .collect();
    if let Some(walls) = walls {
        let from = walls.len();
        for (band, rings) in above.iter().enumerate() {
            for ring in rings {
                clip_ring_to_unit(ring, band, walls);
            }
        }
        // a tenth of a cell is far below what shows, and drops nearly half the points
        let tolerance = WALL_TOLERANCE / wf.max(hf);
        for wall in &mut walls[from..] {
            wall.positions = simplify_line(&wall.positions, tolerance);
        }
    }

    let mut out = Vec::new();
    let options = GridOptions::grid(1.0 / 8.0);
    for band in 0..thresholds.len() {
        // a band is what is above its threshold but not above the next: with rings that never
        // cross, that is the even-odd fill of both sets
        let mut rings = above[band].clone();
        if let Some(next) = above.get(band + 1) {
            rings.extend(next.iter().cloned());
        }
        for polygon in even_odd(&rings) {
            // cut on a grid that has the grid's edges among its lines: that clips to the grid,
            // and smaller pieces with fewer holes each keep earcut fast on speckled tiles
            let holes: Vec<usize> = polygon.hole_indices.iter().map(|i| i * 2).collect();
            for piece in cut_polygon_by_grid(&polygon.positions, &holes, &options) {
                // keep what lies within the grid; the padding falls away
                let end = piece.hole_indices.first().copied().unwrap_or(piece.positions.len());
                let [cx, cy] = centre(&piece.positions, end);
                if cx <= 0.0 || cx >= 1.0 || cy <= 0.0 || cy >= 1.0 {
                    continue;
                }
                let hole_indices = piece.hole_indices.iter().map(|i| i / 2).collect();
                out.push(BandPolygon { band, positions: piece.positions, hole_indices });
            }
        }
    }
    out
}

/// Douglas–Peucker: the points of a flat polyline that keep it within `tolerance` of the
/// original (`simplifyLine`). The ends are always kept, so lines still meet where they did, and
/// a closed ring stays closed: where the chord is a point, distances are from that point.
pub fn simplify_line(p: &[f64], tolerance: f64) -> Vec<f64> {
    let n = p.len() / 2;
    if n < 3 {
        return p.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![0, n - 1];
    while let (Some(b), Some(a)) = (stack.pop(), stack.pop()) {
        let (ax, ay) = (p[a * 2], p[a * 2 + 1]);
        let (dx, dy) = (p[b * 2] - ax, p[b * 2 + 1] - ay);
        let length = js_hypot2(dx, dy);
        let mut worst = tolerance;
        let mut at = None;
        for i in a + 1..b {
            let (x, y) = (p[i * 2] - ax, p[i * 2 + 1] - ay);
            // distance from the chord, or from its start where the chord is a point (a closed ring)
            let d = if length != 0.0 && !length.is_nan() { (x * dy - y * dx).abs() / length } else { js_hypot2(x, y) };
            if d > worst {
                (worst, at) = (d, Some(i));
            }
        }
        if let Some(at) = at {
            keep[at] = true;
            stack.extend_from_slice(&[a, at, at, b]);
        }
    }
    (0..n).filter(|&i| keep[i]).flat_map(|i| [p[i * 2], p[i * 2 + 1]]).collect()
}

/// The centre of the box of `positions[..end]`.
fn centre(positions: &[f64], end: usize) -> [f64; 2] {
    let [x0, y0, x1, y1] = ring_box(&positions[..end.min(positions.len())]);
    [(x0 + x1) / 2.0, (y0 + y1) / 2.0]
}

/// Light 3×3 binomial blur, edges clamped: takes the stair-steps out of the contours (`blur`).
/// Sums in `f64`, stores in `f32`, as the TypeScript's `Float32Array`s do.
pub fn blur(values: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut tmp = vec![0.0_f32; values.len()];
    let mut out = vec![0.0_f32; values.len()];
    if width == 0 {
        return out;
    }
    let v = |a: &[f32], i: usize| f64::from(a[i]);
    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            let l = v(values, row + x.saturating_sub(1));
            let r = v(values, row + (x + 1).min(width - 1));
            tmp[row + x] = ((l + 2.0 * v(values, row + x) + r) / 4.0) as f32;
        }
    }
    for y in 0..height {
        let up = y.saturating_sub(1) * width;
        let down = (y + 1).min(height - 1) * width;
        for x in 0..width {
            out[y * width + x] = ((v(&tmp, up + x) + 2.0 * v(&tmp, y * width + x) + v(&tmp, down + x)) / 4.0) as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;

    #[test]
    fn the_palette_is_the_backends() {
        // `backend::radar` keeps its own private copy for the measured volumes; once it uses a
        // shared constant instead there is nothing left to compare
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../backend/src/radar.rs");
        let Ok(source) = std::fs::read_to_string(path) else { return };
        let Some(start) = source.find("const PALETTE: [[u8; 3]; 14] = [") else { return };
        let body = &source[start + "const PALETTE: [[u8; 3]; 14] = [".len()..];
        let body = &body[..body.find("];").expect("the end of the backend's PALETTE")];
        let theirs: Vec<u8> = body
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter_map(|t| t.strip_prefix("0x"))
            .map(|t| u8::from_str_radix(t, 16).expect("a hex byte"))
            .collect();
        assert_eq!(theirs, PALETTE.concat());
    }

    #[test]
    fn band_colours_follow_the_palette_with_translucent_light_echoes() {
        let alphas: Vec<u8> = (0..BAND_COUNT).map(|b| band_color(b)[3]).collect();
        assert_eq!(alphas, [150, 170, 190, 210, 230, 235, 235, 235, 235, 235, 235, 235, 235, 235]);
        for (band, rgb) in PALETTE.iter().enumerate() {
            assert_eq!(band_color(band)[..3], rgb[..]);
        }
        assert_eq!(band_color(0), [4, 233, 231, 150]);
        assert_eq!(band_color(13), [152, 84, 198, 235]);
        assert_eq!(band_dbz(0), 5.0);
        assert_eq!(band_dbz(13), 70.0);
        // walls: three quarters, rounded half up
        let walls = build_walls(&[
            WallLine { band: 0, positions: vec![0.0, 0.0, 1.0, 0.0] },
            WallLine { band: 13, positions: vec![0.0, 0.0, 1.0, 0.0] },
        ]);
        assert_eq!(walls.colors[3], 113);
        assert_eq!(walls.colors[walls.colors.len() - 1], 176);
    }

    /// The synthetic storms the TypeScript checksums were taken on: a few cones of falling
    /// reflectivity with noise, from a 32-bit LCG.
    fn field(seed: u32, w: usize, h: usize) -> Vec<f32> {
        let mut s = seed;
        let mut rnd = move || {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            s
        };
        let n = 3 + rnd() % 4;
        let storms: Vec<[f64; 4]> = (0..n)
            .map(|_| {
                let cx = f64::from(rnd() % (w as u32 * 2)) - w as f64 / 2.0;
                let cy = f64::from(rnd() % (h as u32 * 2)) - h as f64 / 2.0;
                [cx, cy, 20.0 + f64::from(rnd() % 50), 0.02 + f64::from(rnd() % 100) / 1000.0]
            })
            .collect();
        let mut values = vec![0.0_f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let (xf, yf) = (x as f64, y as f64);
                let mut v = 0.0;
                for &[cx, cy, peak, k] in &storms {
                    v = js_max(v, peak - ((xf - cx) * (xf - cx) + (yf - cy) * (yf - cy)) * k);
                }
                let noise = f64::from(rnd() % 1000) / 250.0 - 2.0;
                values[y * w + x] = js_max(0.0, v + noise) as f32;
            }
        }
        values
    }

    /// `sum += v * (k % 97 + 1)` over every value of every array in turn.
    fn checksum<'a>(arrays: impl IntoIterator<Item = &'a [f64]>) -> f64 {
        let mut sum = 0.0;
        let mut k = 0;
        for a in arrays {
            for &v in a {
                sum += v * ((k % 97) + 1) as f64;
                k += 1;
            }
        }
        sum
    }

    #[test]
    fn contour_bands_match_the_typescript() {
        // bun ran `blur` and `contourBands` on the same fields: these are its numbers, and every
        // polygon and wall came out identical value for value
        for (seed, w, h, blurred, bands, holes, positions, walls, wall_positions) in [
            (1, 48, 40, 1230261.794241134, [41, 39, 35, 29, 25, 19, 14, 4, 0, 0, 0, 0, 0, 0], 165, 88051.45294201396, 21, 8847.485973887104),
            (3, 31, 17, 906320.2446060181, [64, 64, 64, 64, 64, 54, 38, 30, 22, 14, 6, 0, 0, 0], 420, 205359.71164213805, 8, 5294.566591768038),
        ] {
            let b = blur(&field(seed, w, h), w, h);
            let as_f64: Vec<f64> = b.iter().map(|&v| f64::from(v)).collect();
            assert_eq!(checksum([as_f64.as_slice()]), blurred, "blur {seed}");
            let mut lines = Vec::new();
            let polygons = contour_bands(&b, w, h, Some(&mut lines));
            let mut per_band = [0; BAND_COUNT];
            polygons.iter().for_each(|p| per_band[p.band] += 1);
            assert_eq!(per_band, bands, "{seed}");
            assert_eq!(polygons.iter().map(|p| p.hole_indices.len()).sum::<usize>(), holes);
            assert_eq!(checksum(polygons.iter().map(|p| p.positions.as_slice())), positions, "{seed}");
            assert_eq!(lines.len(), walls);
            assert_eq!(checksum(lines.iter().map(|l| l.positions.as_slice())), wall_positions, "{seed}");
        }
    }

    #[test]
    fn wall_lines_skip_tile_edges() {
        let (w, h) = (48, 40);
        let mut lines = Vec::new();
        let polygons = contour_bands(&blur(&field(1, w, h), w, h), w, h, Some(&mut lines));
        assert!(!polygons.is_empty() && !lines.is_empty());
        let on_edge = |v: f64| v == 0.0 || v == 1.0;
        let mut ending_on_edge = 0;
        for line in &lines {
            let p = &line.positions;
            assert!(p.len() >= 4);
            assert!(p.iter().all(|v| (0.0..=1.0).contains(v)), "a wall outside the tile: {p:?}");
            for s in p.windows(4).step_by(2) {
                assert!(!along_edge([s[0], s[1], s[2], s[3]]), "a wall along the tile's edge: {s:?}");
            }
            let n = p.len();
            if on_edge(p[0]) || on_edge(p[1]) || on_edge(p[n - 2]) || on_edge(p[n - 1]) {
                ending_on_edge += 1;
            }
        }
        // the storms run off the tile: their walls stop at its edge, exactly on it
        assert!(ending_on_edge > 0);

        // a storm that fills the grid has its contours in the padding: polygons to the edges,
        // walls nowhere. The weaker bands' rings run round it outside the grid too: inside, the
        // TypeScript leaves each of their cells as a piece with a hole as large as itself
        // (64 per band here), which earcut makes nothing of
        let mut lines = Vec::new();
        let full = contour_bands(&[30.0; 12 * 10], 12, 10, Some(&mut lines));
        assert!(lines.is_empty(), "{lines:?}");
        let mut area = [0.0; BAND_COUNT];
        let mut count = [0; BAND_COUNT];
        for p in &full {
            let end = p.hole_indices.first().map_or(p.positions.len(), |h| h * 2);
            area[p.band] += ring_area(&p.positions[..end]).abs() - ring_area(&p.positions[end..]).abs();
            count[p.band] += 1;
        }
        assert_eq!(count[..6], [64; 6]);
        assert_close!(area[5], 1.0, 12);
        assert!(area[..5].iter().all(|&a| a == 0.0), "{area:?}");
        let mesh = build_mesh(&full);
        assert_eq!(mesh.band_starts[5], 0, "triangles in an empty band: {:?}", mesh.band_starts);
        assert_eq!(mesh.indices.len(), 64 * 6);

        // one wholly inside: closed walls of each band it reaches
        let mut values = vec![0.0_f32; 20 * 20];
        for y in 6..14 {
            for x in 5..15 {
                values[y * 20 + x] = if (8..12).contains(&y) && (8..12).contains(&x) { 40.0 } else { 20.0 };
            }
        }
        let mut lines = Vec::new();
        contour_bands(&blur(&values, 20, 20), 20, 20, Some(&mut lines));
        let mut bands: Vec<usize> = lines.iter().map(|l| l.band).collect();
        bands.dedup();
        assert_eq!(bands, (0..=7).collect::<Vec<_>>());
        for line in &lines {
            let p = &line.positions;
            assert_eq!(p[..2], p[p.len() - 2..], "an inland wall is a closed ring");
        }
    }

    fn square(band: usize, x: f64, y: f64, size: f64) -> BandPolygon {
        BandPolygon { band, positions: vec![x, y, x + size, y, x + size, y + size, x, y + size], hole_indices: vec![] }
    }

    fn triangle_area(mesh: &RadarMesh, from: usize, to: usize) -> f64 {
        let p = |v: u32| (f64::from(mesh.positions[v as usize * 2]), f64::from(mesh.positions[v as usize * 2 + 1]));
        mesh.indices[from..to]
            .chunks_exact(3)
            .map(|t| {
                let ((ax, ay), (bx, by), (cx, cy)) = (p(t[0]), p(t[1]), p(t[2]));
                ((bx - ax) * (cy - ay) - (by - ay) * (cx - ax)).abs() / 2.0
            })
            .sum()
    }

    #[test]
    fn build_mesh_sorts_by_band_and_cuts_wide_polygons_on_the_grid() {
        let mut holed = square(3, 0.5, 0.5, 2.0);
        holed.positions.extend_from_slice(&[1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0, 1.0]);
        holed.hole_indices = vec![4];
        let polygons = [square(5, 10.0, 0.5, 1.0), holed, square(3, -30.0, -1.0, 9.0), square(0, 0.0, 0.0, 0.5)];
        let mesh = build_mesh(&polygons);
        assert_eq!(mesh.size, 2);
        // band 0, then the two of band 3 in their order, then band 5
        // (the 9-degree square in nine pieces of four corners)
        assert_eq!(mesh.band_vertex_starts[..7], [0, 4, 4, 4, 4 + 8 + 36, 48, 52]);
        assert_eq!(mesh.band_starts[..7], [0, 6, 6, 6, 6 + 24 + 54, 84, 90]);
        assert_eq!(mesh.band_starts[BAND_COUNT] as usize, mesh.indices.len());
        assert_eq!(mesh.band_vertex_starts[BAND_COUNT] as usize, mesh.vertex_count());
        // the holed square: 4 of 3 area units left; the 9-degree square cut on the 4-degree grid
        assert_close!(triangle_area(&mesh, 6, 30), 3.0, 12);
        let wide = mesh.band_starts[3] as usize + 24..mesh.band_starts[4] as usize;
        assert_close!(triangle_area(&mesh, wide.start, wide.end), 81.0, 9);
        let cut_vertices = (mesh.band_vertex_starts[4] - 12) as usize;
        assert!(cut_vertices > 4, "cut into pieces: {cut_vertices} vertices");
        let xs: Vec<f32> = (12..12 + cut_vertices).map(|v| mesh.positions[v * 2]).collect();
        for line in [-28.0, -24.0] {
            assert!(xs.contains(&line), "a cut at {line}");
        }
        // every vertex has its band's colour
        for v in 0..mesh.vertex_count() {
            let band = (0..BAND_COUNT).rfind(|&b| mesh.band_vertex_starts[b] as usize <= v).unwrap();
            assert_eq!(mesh.colors[v * 4..v * 4 + 4], band_color(band));
        }
        // nothing to draw
        let empty = build_mesh(&[]);
        assert!(empty.indices.is_empty() && empty.band_starts == [0; BAND_COUNT + 1]);
        assert_eq!(empty.band_range(20), (0, 0));
    }

    #[test]
    fn merging_rebases_each_meshes_bands() {
        let a = build_mesh(&[square(0, 0.0, 0.0, 1.0), square(2, 2.0, 0.0, 1.0)]);
        let b = build_mesh(&[square(1, 5.0, 0.0, 1.0), square(3, 7.0, 0.0, 1.0)]);
        let merged = merge_meshes(&[(&a, 1), (&b, 3)]).unwrap();
        assert_eq!(merged.size, 2);
        assert_eq!(merged.positions.len(), 16);
        assert_eq!(merged.positions[..2], [2.0, 0.0]);
        assert_eq!(merged.positions[8..10], [7.0, 0.0]);
        assert!(merged.indices[..6].iter().all(|&i| i < 4));
        assert!(merged.indices[6..].iter().all(|&i| (4..8).contains(&i)));
        assert_eq!(merged.colors[..4], band_color(2));
        assert_eq!(merged.colors[16..20], band_color(3));
        assert_eq!(polygon_data(&a, 0).unwrap().indices, a.indices);
        assert!(polygon_data(&a, 3).is_none());
        assert!(merge_meshes(&[]).is_none());
    }

    #[test]
    fn walls_hang_two_triangles_from_each_segment() {
        let lines = [
            WallLine { band: 4, positions: vec![0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0] },
            WallLine { band: 1, positions: vec![5.0, 5.0, 6.0, 5.0] },
        ];
        let walls = build_walls(&lines);
        assert_eq!(walls.size, 3);
        // band 1's line first; the repeated point of the other is skipped
        assert_eq!(walls.vertex_count(), 4 + 6);
        assert_eq!(walls.positions[..12], [5.0, 5.0, 0.0, 5.0, 5.0, 1.0, 6.0, 5.0, 0.0, 6.0, 5.0, 1.0]);
        assert_eq!(walls.indices[..6], [0, 1, 2, 1, 3, 2]);
        assert_eq!(walls.indices[6..], [4, 5, 6, 5, 7, 6, 6, 7, 8, 7, 9, 8]);
        assert_eq!(walls.band_starts[..6], [0, 0, 6, 6, 6, 18]);
        assert_eq!(walls.band_vertex_starts[..6], [0, 0, 4, 4, 4, 10]);
        assert_eq!(walls.colors[3], js_round(f64::from(band_color(1)[3]) * WALL_ALPHA) as u8);
    }

    #[test]
    fn polygon_walls_give_shared_boundaries_once_in_the_stronger_band() {
        let ring = [0.0, 0.0, 4.0, 0.0, 4.0, 4.0, 0.0, 4.0];
        let inner = [1.0, 1.0, 1.0, 3.0, 3.0, 3.0, 3.0, 1.0];
        // the inner square as band 2's outer ring, started elsewhere and reversed
        let inner_outer = [3.0, 3.0, 1.0, 3.0, 1.0, 1.0, 3.0, 1.0];
        let lone_hole = [10.0, 10.0, 10.0, 11.0, 11.0, 11.0, 11.0, 10.0];
        let polygons = [
            BandPolygon { band: 1, positions: [ring, inner].concat(), hole_indices: vec![4] },
            BandPolygon { band: 2, positions: inner_outer.to_vec(), hole_indices: vec![] },
            // the same outer ring again, in band 0: one wall, still band 1
            BandPolygon { band: 0, positions: ring.to_vec(), hole_indices: vec![] },
            BandPolygon { band: 3, positions: [[9.0, 9.0, 12.0, 9.0, 12.0, 12.0, 9.0, 12.0], lone_hole].concat(), hole_indices: vec![4] },
        ];
        let walls = polygon_walls(&polygons);
        let summary: Vec<(usize, usize)> = walls.iter().map(|w| (w.band, w.positions.len() / 2)).collect();
        // outer rings: the big square (band 1), the inner square (band 2, which its hole in
        // band 1 doesn't raise), the far square; then the lone hole, a wall of its own
        assert_eq!(summary, [(1, 5), (2, 5), (3, 5), (3, 5)]);
        assert_eq!(walls[0].positions, [ring.as_slice(), &ring[..2]].concat());
        assert_eq!(walls[3].positions, [lone_hole.as_slice(), &lone_hole[..2]].concat());
    }

    #[test]
    fn parse_volume_polygons_reads_the_backends_blobs() {
        let mut blob = 4u32.to_le_bytes().to_vec();
        let mut polygon = |band: u8, rings: &[&[f32]]| {
            blob.extend_from_slice(&[band, 0, 0, 0]);
            blob.extend_from_slice(&(rings.len() as u32).to_le_bytes());
            for ring in rings {
                blob.extend_from_slice(&((ring.len() / 2) as u32).to_le_bytes());
                ring.iter().for_each(|v| blob.extend_from_slice(&v.to_le_bytes()));
            }
        };
        let outer: &[f32] = &[0.0, 0.0, 1.5, 0.0, 1.5, 1.5];
        polygon(3, &[outer, &[0.5, 0.5, 0.6, 0.6], &[0.25, 0.25, 0.5, 0.25, 0.5, 0.5]]);
        polygon(14, &[outer]);
        polygon(2, &[&[0.0, 0.0, 1.0, 1.0]]);
        polygon(0, &[]);
        let polygons = parse_volume_polygons(&blob).unwrap();
        // the hole of under three points, the band past the palette, the thin and the empty
        // polygon are dropped
        assert_eq!(polygons.len(), 1);
        assert_eq!(polygons[0].band, 3);
        assert_eq!(polygons[0].hole_indices, [3]);
        assert_eq!(polygons[0].positions, [0.0, 0.0, 1.5, 0.0, 1.5, 1.5, 0.25, 0.25, 0.5, 0.25, 0.5, 0.5]);
        assert_eq!(parse_volume_polygons(&[]), Ok(vec![]));
        assert_eq!(parse_volume_polygons(&0u32.to_le_bytes()), Ok(vec![]));
        assert_eq!(parse_volume_polygons(&[1, 0]), Err(Truncated { offset: 0, length: 2 }));
        let cut = &blob[..blob.len() - 1];
        let err = parse_volume_polygons(cut).unwrap_err();
        assert_eq!(err.length, cut.len());
        assert_eq!(err.to_string(), format!("radar polygons truncated at byte {} (of {})", err.offset, cut.len()));
    }

    #[test]
    fn simplify_line_keeps_the_ends_and_the_corners() {
        // short lines are left as they are
        assert_eq!(simplify_line(&[0.0, 0.0, 1.0, 1.0], 10.0), [0.0, 0.0, 1.0, 1.0]);
        // a corner stays, points along the straight runs go
        let l = [0.0, 0.0, 1.0, 0.01, 2.0, 0.0, 2.0, 1.0, 2.0, 2.0];
        assert_eq!(simplify_line(&l, 0.1), [0.0, 0.0, 2.0, 0.0, 2.0, 2.0]);
        assert_eq!(simplify_line(&l, 0.001), [0.0, 0.0, 1.0, 0.01, 2.0, 0.0, 2.0, 2.0]);
        // a closed ring: distances from its start, so the far corner and the next stay
        let ring = [0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0];
        assert_eq!(simplify_line(&ring, 0.1), [0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(simplify_line(&ring, 1.2), [0.0, 0.0, 1.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn blur_is_a_clamped_binomial_kernel() {
        assert_eq!(blur(&[7.0; 12], 4, 3), vec![7.0; 12]);
        let mut impulse = vec![0.0_f32; 25];
        impulse[12] = 16.0;
        let b = blur(&impulse, 5, 5);
        assert_eq!(b[12], 4.0);
        assert_eq!(b[11], 2.0);
        assert_eq!(b[7], 2.0);
        assert_eq!(b[6], 1.0);
        assert_eq!(b[0], 0.0);
        // at the edge the missing neighbour is the edge value itself
        let mut corner = vec![0.0_f32; 9];
        corner[0] = 16.0;
        assert_eq!(blur(&corner, 3, 3)[0], 9.0);
        assert!(blur(&[], 0, 0).is_empty());
    }
}
