//! The 3D terrain's heights on the CPU (item Z3; doc 04 §14.3, §14.5; doc 03 §9; plan 1.4.12):
//! what the layers that stand on the terrain are raised by.
//!
//! Today's app raises them with deck.gl's `TerrainExtension` in its *offset* mode: every layer with
//! per-instance anchors (`instancePositions`: mesh, icon, scatterplot and text layers) or
//! `extruded` takes the ground's height under its anchor (`geometry.worldPosition` with its height
//! set to 0) from a height map and adds it to its own height (`terrain/shader-module.js`). The height
//! map is the drawn terrain mesh rendered into a float texture over the view (`HeightMapBuilder`, at
//! most 2048 texels across) and sampled with linear filtering, so the height an object gets is the
//! drawn surface's, read bilinearly between the texels.
//!
//! Here (plan 1.4.12: "bilinear lookup in the decoded height tiles when instance buffers are built"):
//! - **Per tile** ([`HeightGrid`]): each terrain tile's drawn surface, its RTIN mesh with the
//!   sea-level clamp of its vertex shader (`SeaLevelExtension`: no vertex below 0 m), sampled at the
//!   points of a regular [`GRID_SIDE`]² grid over the tile's square ([`HeightGrid::from_mesh`], on
//!   the worker pool with the mesh). Read bilinearly between them ([`HeightGrid::at`]), as deck reads
//!   its height map; so an object stands where the surface is drawn, not where the height image's
//!   pixels say (they are up to the mesh's error, 2 m, apart).
//! - **Over the view** ([`Heights`]): the tiles the surface draws this frame, each point read from
//!   the deepest of them that holds it (the surface's refinement draws no place twice), nothing where
//!   none does (deck adds nothing outside its height map either).
//!
//! Deviation: deck's height map is one texture over the view, so its texels grow with the view's
//! extent (several metres close in at a pitched view); a grid per tile is as fine as its tile
//! (about 3 m at zoom 16 in the Alps, coarser further out), and needs no pass on the GPU.

use std::collections::HashMap;
use std::sync::Arc;

use crate::camera::{WORLD_SIZE, lng_lat_to_world};
use crate::tiles::TileIndex;

/// Points across a tile's [`HeightGrid`]: a sample every 4 pixels of a 512 px Mapterhorn tile (2 of
/// a 256 px Mapzen one), 65 KB a tile.
pub const GRID_SIDE: usize = 129;

/// deck's `MAX_LATITUDE` of the Web Mercator world: the heights' world ends there.
const MAX_LATITUDE: f64 = 85.051_129;

/// A terrain tile's surface as heights at the points of a regular grid over its square: point
/// `(i, j)` at `u = i / (side − 1)` east and `v = j / (side − 1)` south in tile units (the tile's
/// north-west corner at `(0, 0)`), metres.
#[derive(Debug, Clone, PartialEq)]
pub struct HeightGrid {
    side: usize,
    /// Row after row from the north, `side` a row.
    heights: Vec<f32>,
}

impl HeightGrid {
    /// A grid of `side`² heights, row after row from the north (`side` at least 2).
    pub fn new(side: usize, heights: Vec<f32>) -> Self {
        assert!(side >= 2 && heights.len() == side * side, "a {side}² grid needs {} heights, not {}", side * side, heights.len());
        HeightGrid { side, heights }
    }

    /// A grid of `side`² heights `metres(u, v)` at its points.
    pub fn from_fn(side: usize, metres: impl Fn(f64, f64) -> f64) -> Self {
        let step = 1.0 / (side - 1) as f64;
        let heights = (0..side * side).map(|k| metres((k % side) as f64 * step, (k / side) as f64 * step) as f32).collect();
        HeightGrid::new(side, heights)
    }

    /// The drawn surface of a terrain tile's mesh at the points of a `side`² grid: `vertices` in
    /// the tile's frame (`u` east, `v` south in tile units, metres up), three `indices` a triangle,
    /// the first `triangles` of them the surface's (the skirt's after them are left out). Each
    /// vertex is raised to the sea first, as the surface's vertex shader does, then each point takes
    /// the height of the triangle it lies in, linearly between its corners. A point no triangle
    /// covers (none, for the loader's meshes, which reach a pixel past the square) takes the
    /// nearest covered point's height in its row, else 0.
    pub fn from_mesh(vertices: &[[f32; 3]], indices: &[u32], triangles: usize, side: usize) -> Self {
        let mut heights = vec![f32::NAN; side * side];
        let last = (side - 1) as f64;
        let point = |i: u32| {
            let [u, v, h] = vertices[i as usize];
            [f64::from(u) * last, f64::from(v) * last, f64::from(h).max(0.0)]
        };
        for t in indices.chunks_exact(3).take(triangles) {
            let [a, b, c] = [point(t[0]), point(t[1]), point(t[2])];
            let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            if area == 0.0 || !area.is_finite() {
                continue;
            }
            // (a point on an edge belongs to both triangles: either gives the edge's height)
            let slack = 1e-9 * area.abs();
            let lo = |k: usize| (a[k].min(b[k]).min(c[k]) - 1e-9).ceil().max(0.0);
            let hi = |k: usize| (a[k].max(b[k]).max(c[k]) + 1e-9).floor().min(last);
            let (i0, i1, j0, j1) = (lo(0), hi(0), lo(1), hi(1));
            if i0 > i1 || j0 > j1 {
                continue;
            }
            for j in j0 as usize..=j1 as usize {
                for i in i0 as usize..=i1 as usize {
                    let (x, y) = (i as f64, j as f64);
                    // twice the areas opposite each corner, of the triangle's sign
                    let wa = (b[0] - x) * (c[1] - y) - (b[1] - y) * (c[0] - x);
                    let wb = (c[0] - x) * (a[1] - y) - (c[1] - y) * (a[0] - x);
                    let wc = area - wa - wb;
                    let inside = |w: f64| if area > 0.0 { w >= -slack } else { w <= slack };
                    if inside(wa) && inside(wb) && inside(wc) {
                        heights[j * side + i] = ((wa * a[2] + wb * b[2] + wc * c[2]) / area) as f32;
                    }
                }
            }
        }
        for row in heights.chunks_exact_mut(side) {
            fill_row(row);
        }
        HeightGrid { side, heights }
    }

    /// Points across.
    pub fn side(&self) -> usize {
        self.side
    }

    /// The heights, row after row from the north.
    pub fn heights(&self) -> &[f32] {
        &self.heights
    }

    /// The height at `(u, v)` in tile units (`v` south), metres: bilinear between the four grid
    /// points round it (as a texture's linear filter reads its texels), a point outside the square
    /// read at its nearest edge.
    pub fn at(&self, u: f64, v: f64) -> f64 {
        let last = (self.side - 1) as f64;
        let x = (u.clamp(0.0, 1.0) * last).min(last);
        let y = (v.clamp(0.0, 1.0) * last).min(last);
        let (i, j) = ((x.floor() as usize).min(self.side - 2), (y.floor() as usize).min(self.side - 2));
        let (s, t) = (x - i as f64, y - j as f64);
        let h = |i: usize, j: usize| f64::from(self.heights[j * self.side + i]);
        let north = h(i, j) + (h(i + 1, j) - h(i, j)) * s;
        let south = h(i, j + 1) + (h(i + 1, j + 1) - h(i, j + 1)) * s;
        north + (south - north) * t
    }

    /// The bytes it holds.
    pub fn byte_size(&self) -> usize {
        self.heights.len() * size_of::<f32>()
    }
}

/// The points of a grid row no triangle covered take their nearest covered neighbour's height in
/// the row (the earlier of two as near), or 0 in a row with none.
fn fill_row(row: &mut [f32]) {
    if !row.iter().any(|h| h.is_nan()) {
        return;
    }
    let known: Vec<usize> = (0..row.len()).filter(|&i| !row[i].is_nan()).collect();
    if known.is_empty() {
        row.fill(0.0);
        return;
    }
    for i in 0..row.len() {
        if row[i].is_nan() {
            let nearest = *known.iter().min_by_key(|&&k| k.abs_diff(i)).expect("a covered point");
            row[i] = row[nearest];
        }
    }
}

/// The heights of the terrain drawn this frame: its tiles, each with its [`HeightGrid`].
#[derive(Debug, Clone, Default)]
pub struct Heights {
    tiles: HashMap<TileIndex, Arc<HeightGrid>>,
    /// The tiles' zooms, deepest first.
    zooms: Vec<u32>,
}

impl Heights {
    /// The heights of `tiles` (the surface's tiles drawn, which overlap nowhere; where they would,
    /// the deeper tile is read).
    pub fn new(tiles: impl IntoIterator<Item = (TileIndex, Arc<HeightGrid>)>) -> Self {
        let tiles: HashMap<TileIndex, Arc<HeightGrid>> = tiles.into_iter().collect();
        let mut zooms: Vec<u32> = tiles.keys().map(|t| t.z).collect();
        zooms.sort_unstable_by(|a, b| b.cmp(a));
        zooms.dedup();
        Heights { tiles, zooms }
    }

    /// Whether it holds no tile (no height anywhere).
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// The tiles it reads, in no order.
    pub fn tiles(&self) -> impl Iterator<Item = TileIndex> + '_ {
        self.tiles.keys().copied()
    }

    /// The tiles with their grids, in no order.
    pub fn grids(&self) -> impl Iterator<Item = (TileIndex, &Arc<HeightGrid>)> + '_ {
        self.tiles.iter().map(|(&t, g)| (t, g))
    }

    /// The ground's height at the flat map's world point `[x, y]` (512 units round the world, `y`
    /// north; any copy of the world), metres; `None` where no tile holds it.
    pub fn at_world(&self, [x, y]: [f64; 2]) -> Option<f64> {
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        let x = x.rem_euclid(WORLD_SIZE);
        let down = WORLD_SIZE - y;
        for &z in &self.zooms {
            let n = 2f64.powi(z as i32);
            let extent = WORLD_SIZE / n;
            let (column, row) = ((x / extent).floor(), (down / extent).floor());
            if !(0.0..=n).contains(&row) {
                continue;
            }
            // (the world's south and east edges are its last row's and column's)
            let (column, row) = (column.min(n - 1.0), row.min(n - 1.0));
            let tile = TileIndex::new(column as u32, row as u32, z);
            if let Some(grid) = self.tiles.get(&tile) {
                return Some(grid.at(x / extent - column, down / extent - row));
            }
        }
        None
    }

    /// The ground's height at `[lon, lat]` (degrees; the latitude held within the Web Mercator
    /// world), metres; `None` where no tile holds it.
    pub fn at(&self, [lon, lat]: [f64; 2]) -> Option<f64> {
        if self.tiles.is_empty() {
            return None;
        }
        let [x, y] = lng_lat_to_world([lon, lat.clamp(-MAX_LATITUDE, MAX_LATITUDE)]);
        self.at_world([x, y.clamp(0.0, WORLD_SIZE)])
    }

    /// The bytes its grids hold.
    pub fn byte_size(&self) -> usize {
        self.tiles.values().map(|g| g.byte_size()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::world_to_lng_lat;

    fn near(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() <= tolerance
    }

    /// Bilinear between the four points round a place, as a texture's linear filter reads: exact
    /// at the points, along an edge linear between its two ends, inside the cell the mix of all four
    /// by the place's fractions (a saddle `u·v` comes back exactly), and a plane anywhere.
    #[test]
    fn heights_are_read_bilinearly_between_the_grid_points() {
        // 3 × 3: points at u, v = 0, 0.5, 1
        let grid = HeightGrid::new(3, vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 100.0]);
        assert_eq!(grid.at(0.0, 0.0), 10.0);
        assert_eq!(grid.at(0.5, 0.0), 20.0);
        assert_eq!(grid.at(1.0, 1.0), 100.0);
        assert_eq!(grid.at(0.5, 0.5), 50.0);
        // along the north edge, a quarter of the way: 10 + (20 − 10)·0.5
        assert_eq!(grid.at(0.25, 0.0), 15.0);
        // in the south-east cell (50, 60, 80, 100) at its fractions 0.5, 0.5
        assert_eq!(grid.at(0.75, 0.75), (50.0 + 60.0 + 80.0 + 100.0) / 4.0);
        // fractions 0.2 east and 0.6 south of point (1, 1): north 50 + 10·0.2, south 80 + 20·0.2
        let (north, south) = (50.0 + 10.0 * 0.2, 80.0 + 20.0 * 0.2);
        assert!(near(grid.at(0.6, 0.8), north + (south - north) * 0.6, 1e-9));

        let saddle = HeightGrid::from_fn(2, |u, v| 400.0 * u * v);
        for (u, v) in [(0.3, 0.7), (0.9, 0.1), (0.5, 0.5)] {
            assert!(near(saddle.at(u, v), 400.0 * u * v, 1e-9), "{u} {v}");
        }
        let plane = |u: f64, v: f64| 1500.0 + 300.0 * u - 120.0 * v;
        let grid = HeightGrid::from_fn(GRID_SIDE, plane);
        for (u, v) in [(0.0, 0.0), (0.123, 0.456), (0.999, 0.5), (1.0, 1.0), (0.31, 0.92)] {
            assert!(near(grid.at(u, v), plane(u, v), 1e-3), "{u} {v}: {} against {}", grid.at(u, v), plane(u, v));
        }
    }

    /// A place outside the square is read at its nearest edge (no extrapolation).
    #[test]
    fn outside_the_square_the_edge_is_read() {
        let grid = HeightGrid::from_fn(5, |u, v| 100.0 * u + 10.0 * v);
        assert_eq!(grid.at(-0.5, 0.5), grid.at(0.0, 0.5));
        assert_eq!(grid.at(1.5, 2.0), grid.at(1.0, 1.0));
        assert!(near(grid.at(1.5, 2.0), 110.0, 1e-9));
    }

    /// Two triangles over the square (and a pixel past it, as the loader's meshes reach) make a
    /// plane: every grid point takes the plane's height, so every place reads it; the skirt's
    /// triangles after the surface's change nothing.
    #[test]
    fn a_mesh_is_sampled_where_its_triangles_are() {
        let plane = |u: f32, v: f32| 2000.0 + 250.0 * u - 80.0 * v;
        let o = -1.0 / 512.0;
        let e = 1.0 + 1.0 / 512.0;
        let corners = [[o, o], [e, o], [e, e], [o, e]];
        let mut vertices: Vec<[f32; 3]> = corners.iter().map(|&[u, v]| [u, v, plane(u, v)]).collect();
        // a skirt's vertex far below
        vertices.push([0.5, 0.5, -5000.0]);
        let indices = [0, 1, 2, 0, 2, 3, 0, 1, 4];
        let grid = HeightGrid::from_mesh(&vertices, &indices, 2, 9);
        assert!(grid.heights().iter().all(|h| h.is_finite()));
        for (u, v) in [(0.0, 0.0), (0.5, 0.5), (0.1, 0.9), (1.0, 0.25), (0.77, 0.33)] {
            let want = f64::from(plane(u as f32, v as f32));
            assert!(near(grid.at(u, v), want, 1e-2), "{u} {v}: {} against {want}", grid.at(u, v));
        }
    }

    /// The surface's vertex shader raises every vertex below the sea to it before the triangles are
    /// drawn, so the grid reads the triangles between the raised corners: a sea floor reads 0, a
    /// coast between a raised corner and a hill the line between 0 and the hill.
    #[test]
    fn the_sea_floor_is_raised_to_the_sea_first() {
        let vertices = [[0.0, 0.0, -40.0], [1.0, 0.0, 100.0], [1.0, 1.0, 100.0], [0.0, 1.0, -40.0]];
        let indices = [0, 1, 2, 0, 2, 3];
        let grid = HeightGrid::from_mesh(&vertices, &indices, 2, 5);
        assert_eq!(grid.at(0.0, 0.5), 0.0);
        // half way from the raised west edge (0) to the east one (100)
        assert!(near(grid.at(0.5, 0.5), 50.0, 1e-4), "{}", grid.at(0.5, 0.5));
        assert!(grid.heights().iter().all(|&h| h >= 0.0));
    }

    /// A row's points no triangle covers take the nearest covered point's height in the row.
    #[test]
    fn uncovered_points_take_their_neighbours() {
        let mut row = [f32::NAN, 3.0, f32::NAN, f32::NAN, 7.0, f32::NAN];
        fill_row(&mut row);
        assert_eq!(row, [3.0, 3.0, 3.0, 7.0, 7.0, 7.0]);
        let mut none = [f32::NAN; 3];
        fill_row(&mut none);
        assert_eq!(none, [0.0; 3]);
    }

    /// Over the view each place is read from the deepest drawn tile that holds it, in that tile's
    /// own units; a place no tile holds has no height; a copy of the world east or west reads the
    /// same; longitude and latitude go through the flat map's world.
    #[test]
    fn a_place_is_read_from_the_deepest_tile_that_holds_it() {
        let coarse = TileIndex::new(1066, 723, 11);
        let fine = TileIndex::new(4265, 2894, 13);
        assert!(coarse.contains(fine));
        let heights = Heights::new([
            (coarse, Arc::new(HeightGrid::from_fn(GRID_SIDE, |_, _| 1000.0))),
            (fine, Arc::new(HeightGrid::from_fn(GRID_SIDE, |u, v| 2000.0 + 100.0 * u + 10.0 * v))),
        ]);
        let extent = WORLD_SIZE / 2f64.powi(13);
        // a quarter of the way east and half way south in the fine tile
        let world = [(4265.0 + 0.25) * extent, WORLD_SIZE - (2894.0 + 0.5) * extent];
        assert!(near(heights.at_world(world).expect("held"), 2030.0, 1e-3));
        assert!(near(heights.at_world([world[0] + WORLD_SIZE, world[1]]).expect("held"), 2030.0, 1e-3));
        assert!(near(heights.at_world([world[0] - 2.0 * WORLD_SIZE, world[1]]).expect("held"), 2030.0, 1e-3));
        let lng_lat = world_to_lng_lat(world);
        assert!(near(heights.at(lng_lat).expect("held"), 2030.0, 1e-3));
        // elsewhere in the coarse tile, outside the fine one
        let elsewhere = [(1066.0 + 0.9) * 4.0 * extent, WORLD_SIZE - (723.0 + 0.9) * 4.0 * extent];
        assert_eq!(heights.at_world(elsewhere), Some(1000.0));
        // outside both, and with no tiles at all
        assert_eq!(heights.at([0.0, 0.0]), None);
        assert_eq!(heights.at_world([f64::NAN, 0.0]), None);
        assert_eq!(Heights::default().at(lng_lat), None);
        assert!(Heights::default().is_empty());
        assert_eq!(heights.byte_size(), 2 * GRID_SIDE * GRID_SIDE * 4);
    }

    /// The poles and the antimeridian: a latitude past the Web Mercator world's is held at its edge
    /// (the top row of tiles), and the last column of the world is the east edge's.
    #[test]
    fn the_world_s_edges_are_held() {
        let heights = Heights::new([
            (TileIndex::new(0, 0, 1), Arc::new(HeightGrid::from_fn(3, |_, _| 7.0))),
            (TileIndex::new(1, 1, 1), Arc::new(HeightGrid::from_fn(3, |_, _| 9.0))),
        ]);
        assert_eq!(heights.at([-90.0, 89.9]), Some(7.0));
        assert_eq!(heights.at([179.999, -60.0]), Some(9.0));
        // the south-east corner of the world is the last tile's
        assert_eq!(heights.at_world([WORLD_SIZE - 1e-9, 0.0]), Some(9.0));
        assert_eq!(heights.at_world([0.0, WORLD_SIZE]), Some(7.0));
        assert_eq!(heights.at_world([0.0, -1.0]), None);
        assert_eq!(heights.at([90.0, 60.0]), None);
    }
}
