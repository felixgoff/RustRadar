//! `ground-land-cover` (item R8; doc 04 §1 row 3b, §11; `landcover-ground.ts`'s
//! `landCoverGroundLayer`, G:2656): the realistic map's ground where the map's data says
//! nothing, what the land cover has there (woods, fields, mires, heath, rock, snow) in the
//! ground's own surfaces, drawn with the ground shader over the grass that lies everywhere
//! (`ground-land`) and under every mapped area (`ground-city`), so it shows only where the map
//! leaves the land unmapped.
//!
//! What today's app does, and so this layer:
//! - **When.** On the realistic basemap on the flat map (zoom > 12) while the scene shows the
//!   ground ([`super::Input`]); on the globe the layer leaves deck's list, its tiles with it.
//! - **Tiles.** deck's TileLayer: 256-px tiles ([`TILE_SIZE`]) of zoom [`MIN_ZOOM`] to
//!   [`MAX_ZOOM`] by deck's own rule (deeper views stretch zoom 14's: finer is finer than the
//!   land cover itself), at most [`MAX_REQUESTS`] loading, best-available refinement, a cache of
//!   five times the selection. Each tile's land cover (`landCover`, `backend::landcover`: NMD's
//!   classes in Sweden, WorldCover's where NMD is clear or not to be had) is asked of the tile
//!   fetcher as the source [`LAND_COVER`], whose answer is its classes ([`grid_of`]); the trees
//!   (`layers::trees`) ask the same source, so the backend's one cache serves both, as today's.
//! - **Each tile** ([`tessellate`], on the worker pool): `scene::landcover_ground`'s outlines of
//!   each kind of ground (classes sharing a paint traced as one, the grid softened twice, the 0.5
//!   contour with its holes, nothing under 6 cells) in longitude and latitude (`placed`), filled
//!   as deck's SolidPolygonLayer fills them: each polygon normalised (rings closed, the outline
//!   clockwise and the holes counter-clockwise), projected to the flat map's common space
//!   (`projectFlat`) and cut into triangles there by earcut 2.2.4 (`scene::geom::earcut`), in
//!   the data's order, each vertex carrying its area's paint as the ground shader reads it (the
//!   tint and the surface id, `tess::ground::explicit`), in the tile's frame. `SURFACE_NO_CULL`
//!   (no depth written, nothing culled), drawn with the ground shader (`GROUND_SURFACES`), tile by
//!   tile; not clipped (a TileLayer's own sub-layers are not, and the outlines lie within their
//!   tile).
//!
//! A `--fixture` folder holds the pictures the servers sent (`tiles/nmd`, `tiles/worldcover`), not
//! the classes: [`fixture_land_cover`] puts them together for `io::DirTiles` as
//! `backend::landcover` does, decoding them with [`land_classes`].
//!
//! How it is checked (the tests below): the tiles are today's (zoom 10–14 by deck's rule, z14 at
//! V4 and V5, none on the globe); a tile's triangles cover its outlines' area exactly, in the
//! paints and ids of its kinds of ground; the fixture folder's land cover is NMD's pictures'
//! classes; and at V4 from the fixtures, where the map leaves the land unmapped and NMD has
//! forest, the map shows woods in today's hues (`realistic-v4-1440x900.png`), where it showed the
//! grass of unmapped land without the land cover.
//!
//! Deviations, none visible: the vertices are kept in the tile's frame (deck draws them from
//! longitude and latitude in 64-bit), the triangles are earcut's own of the same projected
//! points.

use glam::DVec2;
use rustradar_core::geo::Basemap;
use rustradar_scene::camera::lng_lat_to_world;
use rustradar_scene::geom::earcut::earcut;
use rustradar_scene::geom::path::LocalMercator;
use rustradar_scene::landcover::{self, LandClass, LandGrid};
use rustradar_scene::landcover_ground::{self, CoverArea, Ring};
use rustradar_scene::tess::ground::explicit;
use rustradar_scene::tiles::TilesetOptions;

use super::{GroundShader, GroundVertex, LineWidth, load_textures};
use crate::frame::FrameSlot;
use crate::io::{DirTiles, FetchResult};
use crate::jobs::Cancel;
use crate::layers::{ByteSize, DeckSelect, DrawCx, Init, Layer, Pass, PrepareCx, Source, TileIndex, TileSet, TileSetOptions};
use crate::pipelines::fill::Mesh;
use crate::state::DrawState;

pub use rustradar_scene::landcover_ground::{MAX_REQUESTS, MAX_ZOOM, MIN_ZOOM, TILE_SIZE};

/// The source of a tile's classes (`backend::landcover::NAME`): the answer is `size²` bytes, one
/// class a cell from the tile's north-west corner, row by row.
pub const LAND_COVER: &str = "landcover";
/// The fixture folders' NMD pictures (`backend::landcover::NMD`).
pub const NMD: &str = "nmd";
/// The fixture folders' WorldCover pictures (`backend::landcover::WORLDCOVER`).
pub const WORLDCOVER: &str = "worldcover";

/// The layer's tiles (`landCoverGroundLayer`): 256 px, zooms 10 to 14, deck's rule.
pub const TILES: TilesetOptions = TilesetOptions::new(TILE_SIZE as f64, MIN_ZOOM as f64, MAX_ZOOM as f64);

/// The flat map's common space never reaches past these `y` (deck's `projectFlat` clamps, as its
/// shader clamps the latitude to ±89.9°).
const WORLD_Y: [f64; 2] = [-318.0, 830.0];

// ------------------------------------------------------------------ the land cover's classes

/// A land cover picture's pixels (`pixels`: as stored, no colour management, drawn to 256 × 256
/// without smoothing) to its classes (NMD's colours, or WorldCover's codes); none for bytes that
/// are no picture.
pub fn land_classes(png: &[u8], nmd: bool) -> Option<Vec<LandClass>> {
    let image = image::load_from_memory(png).ok()?.to_rgba8();
    let (w, h) = image.dimensions();
    let size = landcover::SIZE;
    if w == 0 || h == 0 {
        return None;
    }
    let rgba = if (w as usize, h as usize) == (size, size) {
        image.into_raw()
    } else {
        let mut out = Vec::with_capacity(size * size * 4);
        for y in 0..size {
            let sy = (((y as f64 + 0.5) * f64::from(h)) / size as f64) as u32;
            for x in 0..size {
                let sx = (((x as f64 + 0.5) * f64::from(w)) / size as f64) as u32;
                out.extend_from_slice(&image.get_pixel(sx.min(w - 1), sy.min(h - 1)).0);
            }
        }
        out
    };
    Some(if nmd { landcover::nmd_classes(&rgba) } else { landcover::world_cover_classes(&rgba) })
}

/// The answer for [`LAND_COVER`] from a folder of tiles (`io::DirTiles`): its NMD picture of the
/// tile when the tile is in Sweden, its WorldCover picture when that is not to be had or is clear
/// somewhere (from zoom 6), put together (`landCover`); `Ok(None)` when neither is there. A
/// picture the folder lacks is noted as the folder's miss; one that can't be read is none.
pub fn fixture_land_cover(dir: &DirTiles, tile: TileIndex) -> FetchResult {
    let read = |source: &str, nmd: bool| dir.read(source, tile).ok().flatten().and_then(|png| land_classes(&png, nmd));
    let nmd = if landcover::in_sweden(tile.x, tile.y, tile.z) { read(NMD, true) } else { None };
    // past NMD's edge, WorldCover's (its mosaic is drawn from zoom 6)
    let world = if landcover::wants_world_cover(nmd.as_deref()) && tile.z >= landcover::WORLD_COVER_MIN_ZOOM { read(WORLDCOVER, false) } else { None };
    Ok(landcover::combine(nmd, world).map(|grid| grid.classes))
}

/// An answer's grid: `size²` classes, `size` its square root; none for anything else.
pub fn grid_of(bytes: &[u8]) -> Option<LandGrid> {
    let size = (bytes.len() as f64).sqrt() as usize;
    (size > 0 && size * size == bytes.len()).then(|| LandGrid { size, classes: bytes.to_vec() })
}

// ------------------------------------------------------------------ a tile's areas

/// A tile's areas, ready for the GPU: the triangles of every outline in the tile's frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decoded {
    pub vertices: Vec<GroundVertex>,
    /// Three indices into [`Self::vertices`] a triangle, area after area in the outlines' order.
    pub indices: Vec<u32>,
}

impl ByteSize for Decoded {
    fn byte_size(&self) -> usize {
        size_of_val(self.vertices.as_slice()) + size_of_val(self.indices.as_slice())
    }
}

/// math.gl's signed area of the ring `points[start..end]` (two numbers a vertex, closed):
/// positive clockwise with y up.
fn math_gl_area(points: &[f64], start: usize, end: usize) -> f64 {
    let mut area = 0.0;
    let mut j = end - 2;
    let mut i = start;
    while i < end {
        area += (points[i] - points[j]) * (points[i + 1] + points[j + 1]);
        j = i;
        i += 2;
    }
    area / 2.0
}

/// math.gl's `modifyPolygonWindingDirection`: reverses the ring `points[start..end]` unless the
/// sign of its area is `direction` (1 clockwise, −1 counter-clockwise; an area of 0 is neither).
fn set_winding(points: &mut [f64], start: usize, end: usize, direction: f64) {
    let area = math_gl_area(points, start, end);
    let sign = if area > 0.0 {
        1.0
    } else if area < 0.0 {
        -1.0
    } else {
        0.0
    };
    if sign != direction {
        let ring = &mut points[start..end];
        let n = ring.len() / 2;
        for k in 0..n / 2 {
            ring.swap(2 * k, 2 * (n - 1 - k));
            ring.swap(2 * k + 1, 2 * (n - 1 - k) + 1);
        }
    }
}

/// deck's `Polygon.normalize` of a polygon in longitude and latitude (its outline, then its
/// holes): the rings one after another (two numbers a vertex), each closed (its first point
/// repeated at its end unless it already is), the outline clockwise and the holes
/// counter-clockwise; and where each hole starts, in values.
fn normalize(rings: &[Ring]) -> (Vec<f64>, Vec<usize>) {
    let mut positions = Vec::new();
    let mut holes = Vec::new();
    for (i, ring) in rings.iter().enumerate() {
        let start = positions.len();
        if i > 0 {
            holes.push(start);
        }
        positions.extend(ring.iter().flatten());
        if let (Some(first), Some(last)) = (ring.first(), ring.last())
            && first != last
        {
            positions.extend(first);
        }
        let end = positions.len();
        if end > start {
            set_winding(&mut positions, start, end, if i == 0 { 1.0 } else { -1.0 });
        }
    }
    (positions, holes)
}

/// One area as deck's SolidPolygonLayer fills it on the flat map: normalised, projected with
/// `projectFlat`, cut into triangles there by earcut; appended to `out` in `local`'s frame.
fn fill(out: &mut Decoded, local: &LocalMercator, area: &CoverArea) {
    let (positions, holes) = normalize(&area.polygon);
    let world: Vec<f64> = positions
        .chunks_exact(2)
        .flat_map(|p| {
            let [x, y] = lng_lat_to_world([p[0], p[1]]);
            [x, y.clamp(WORLD_Y[0], WORLD_Y[1])]
        })
        .collect();
    let holes: Vec<usize> = holes.iter().map(|h| h / 2).collect();
    let indices = earcut(&world, &holes, 2);
    let base = out.vertices.len() as u32;
    let [r, g, b, surface] = explicit(area.paint);
    out.vertices.extend(world.chunks_exact(2).map(|w| {
        let [u, v] = local.from_world(DVec2::new(w[0], w[1]));
        GroundVertex { position: [u as f32, v as f32, 0.0], tint: [r, g, b], surface }
    }));
    out.indices.extend(indices.into_iter().map(|i| base + i as u32));
}

/// A tile's land cover as the layer draws it (see the module docs), or `None` if `cancelled` said
/// so between areas.
pub fn tessellate(tile: TileIndex, grid: &LandGrid, cancelled: &dyn Fn() -> bool) -> Option<Decoded> {
    let local = LocalMercator::tile(tile);
    let mut out = Decoded::default();
    for area in landcover_ground::placed(tile, grid) {
        if cancelled() {
            return None;
        }
        fill(&mut out, &local, &area);
    }
    Some(out)
}

/// The decode step, on the worker pool: an answer's classes to the tile's triangles (`None` for a
/// tile with nothing to draw).
pub fn decode(tile: TileIndex, bytes: &[u8], cancel: &Cancel) -> Result<Option<Decoded>, String> {
    let Some(grid) = grid_of(bytes) else { return Ok(None) };
    match tessellate(tile, &grid, &|| cancel.is_cancelled()) {
        None => Err("cancelled".into()),
        Some(decoded) if decoded.indices.is_empty() => Ok(None),
        Some(decoded) => Ok(Some(decoded)),
    }
}

// ------------------------------------------------------------------ the layer

/// Row 3b of doc 04 §1: `ground-land-cover`.
pub struct GroundLandCover {
    shader: GroundShader,
    pipeline: wgpu::RenderPipeline,
    tiles: TileSet<Decoded, Mesh<GroundVertex>>,
    shown: bool,
    /// This frame's tiles in draw order, each with its frame.
    slots: Vec<(FrameSlot, TileIndex)>,
}

impl GroundLandCover {
    pub fn new(init: &Init<'_>) -> Self {
        let shader = GroundShader::new(init, "ground-land-cover");
        let pipeline = shader.areas(init, "ground-land-cover", DrawState::SURFACE_NO_CULL);
        let tiles = init.tile_set(
            vec![Source::new(LAND_COVER)],
            DeckSelect(TILES),
            TileSetOptions { max_requests: MAX_REQUESTS, ..Default::default() },
            |tile, bytes, cancel| decode(tile, &bytes, cancel),
        );
        GroundLandCover { shader, pipeline, tiles, shown: false, slots: Vec::new() }
    }

    /// The tile set (tests).
    pub fn tiles(&self) -> &TileSet<Decoded, Mesh<GroundVertex>> {
        &self.tiles
    }

    /// Whether the last frame drew the layer.
    pub fn shown(&self) -> bool {
        self.shown
    }
}

impl Layer for GroundLandCover {
    fn name(&self) -> &'static str {
        "ground-land-cover"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.slots.clear();
        // (G:2656: the realistic map's flat projection)
        self.shown = cx.scene.ground.shown && cx.scene.basemap == Basemap::Realistic && !cx.view.is_globe();
        if !self.shown {
            // deck drops a layer that leaves the list, and its tiles with it
            if !self.tiles.is_empty() {
                self.tiles.clear();
            }
            return;
        }
        load_textures(cx);
        self.shader.prepare(cx);
        cx.update_tiles(&mut self.tiles, |gpu, _tile, decoded| Mesh::new(&gpu.device, "ground-land-cover", &decoded.vertices, &decoded.indices));
        for (tile, _) in self.tiles.visible() {
            self.slots.push((cx.tile_frame(tile, 0.0), tile));
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if self.slots.is_empty() || cx.pass != Pass::Map {
            return;
        }
        self.shader.begin(pass, &self.pipeline, LineWidth::Hairline);
        for &(slot, tile) in &self.slots {
            if let Some(mesh) = self.tiles.get(tile) {
                self.shader.draw_areas(pass, cx, slot, mesh);
            }
        }
    }

    fn drape(&self) -> crate::layers::Drape {
        crate::layers::Drape::Cover
    }

    fn pending(&self) -> bool {
        self.shown && self.tiles.loading()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use rustradar_core::weather::PRESETS;
    use rustradar_scene::camera::ViewState;
    use rustradar_scene::ground::area;
    use rustradar_scene::landcover::{class_at, lc, wooded};
    use rustradar_scene::tiles::tile_indices;

    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{FetchResult, FnFetcher, TileFetcher};
    use crate::layers::{earth, ground, hillshade, sky};
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 10:00 UTC: noon in Stockholm (the reference pictures' clock).
    const MIDSUMMER: f64 = 1_782_036_000_000.0;
    /// V4 (plan section 8): Arlanda, pitched, at today's pictures' size.
    const V4: ViewState = ViewState { latitude: 59.6519, longitude: 17.9186, zoom: 13.5, pitch: 45.0, bearing: 30.0, position: None };
    const V5: ViewState = ViewState { latitude: 59.3293, longitude: 18.0686, zoom: 15.5, pitch: 60.0, bearing: 20.0, position: None };
    const SIZE: [f64; 2] = [1440.0, 900.0];

    fn shots_dir() -> PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    fn reference(name: &str) -> Image {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference").join(name);
        let rgba = image::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).into_rgba8();
        Image { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() }
    }

    /// `landcover.test.ts`'s grid: a wood, a field and a town in three bands of columns, and a
    /// scrap of heath too small to keep.
    fn three_bands() -> LandGrid {
        let size = 32;
        let mut classes = vec![lc::NONE; size * size];
        for y in 0..size {
            for x in 0..size {
                classes[y * size + x] = if x < 12 {
                    lc::CONIFERS
                } else if x < 24 {
                    lc::ARABLE
                } else {
                    lc::BUILT
                };
            }
        }
        classes[5 * size + 5] = lc::HEATH;
        LandGrid { size, classes }
    }

    /// A field with a pond of open mire in it, and a wood round both: a polygon with a hole.
    fn holed() -> LandGrid {
        let size = 40;
        let mut classes = vec![lc::MIXED; size * size];
        for y in 8..32 {
            for x in 8..32 {
                classes[y * size + x] = if (14..26).contains(&x) && (14..26).contains(&y) { lc::WETLAND } else { lc::ARABLE };
            }
        }
        LandGrid { size, classes }
    }

    /// Twice a polygon's area (outline less holes) in the flat map's common space.
    fn polygon_area(polygon: &[Ring]) -> f64 {
        let ring = |r: &Ring| {
            let w: Vec<[f64; 2]> = r.iter().map(|&p| lng_lat_to_world(p)).collect();
            let mut s = 0.0;
            for i in 0..w.len() {
                let (a, b) = (w[i], w[(i + 1) % w.len()]);
                s += a[0] * b[1] - b[0] * a[1];
            }
            s.abs()
        };
        ring(&polygon[0]) - polygon[1..].iter().map(ring).sum::<f64>()
    }

    /// Twice the area of a run of triangles, in the flat map's common space.
    fn triangles_area(tile: TileIndex, vertices: &[GroundVertex], indices: &[u32]) -> f64 {
        let local = LocalMercator::tile(tile);
        let world = |i: u32| {
            let p = vertices[i as usize].position;
            local.to_world([f64::from(p[0]), f64::from(p[1])])
        };
        indices
            .chunks_exact(3)
            .map(|t| {
                let (a, b, c) = (world(t[0]), world(t[1]), world(t[2]));
                ((b - a).perp_dot(c - a)).abs()
            })
            .sum()
    }

    #[test]
    fn the_layer_is_today_s() {
        assert_eq!((MIN_ZOOM, MAX_ZOOM, TILE_SIZE, MAX_REQUESTS), (10, 14, 256, 4));
        assert_eq!((LAND_COVER, NMD, WORLDCOVER), ("landcover", "nmd", "worldcover"));
        // deck's rule for 256-px tiles: round(zoom + 1), zoom 14 at V4 and V5, none under 8.5
        for view in [V4, V5, ViewState { zoom: 18.0, ..V5 }] {
            let viewport = ViewFrame::new(&view, SIZE, 1.0, 0.0).viewport;
            let tiles = tile_indices(&viewport, &TILES);
            assert!(!tiles.is_empty());
            assert!(tiles.iter().all(|t| t.z == 14), "{view:?}");
        }
        let at = |zoom: f64| tile_indices(&ViewFrame::new(&ViewState { zoom, pitch: 0.0, ..V4 }, SIZE, 1.0, 0.0).viewport, &TILES);
        assert!(at(12.6).iter().all(|t| t.z == 14));
        assert!(at(12.4).iter().all(|t| t.z == 13));
        assert!(at(8.4).is_empty());
    }

    /// Each kind of ground's outline (with its holes) is filled exactly: the triangles of an area
    /// cover its outline's area, every vertex in its paint's tint and surface id, all within the
    /// tile; towns and scraps left out.
    #[test]
    fn a_tile_s_outlines_are_filled_as_deck_fills_them() {
        let tile = TileIndex::new(9007, 4787, 14);
        for grid in [three_bands(), holed()] {
            let areas = landcover_ground::placed(tile, &grid);
            assert!(!areas.is_empty());
            let decoded = tessellate(tile, &grid, &|| false).expect("not cancelled");
            let mut local = Decoded::default();
            let frame = LocalMercator::tile(tile);
            for a in &areas {
                let (v0, i0) = (local.vertices.len(), local.indices.len());
                fill(&mut local, &frame, a);
                let want = polygon_area(&a.polygon);
                let got = triangles_area(tile, &local.vertices, &local.indices[i0..]);
                assert!((got - want).abs() <= want * 1e-6, "{:?}: {got} against {want}", a.paint);
                let [r, g, b, id] = explicit(a.paint);
                assert!(local.vertices[v0..].iter().all(|v| v.tint == [r, g, b] && v.surface == id));
            }
            assert_eq!(decoded, local);
            for v in &decoded.vertices {
                assert!(v.position.iter().take(2).all(|&c| (-1e-6..=1.0 + 1e-6).contains(&c)), "{v:?}");
                assert_eq!(v.position[2], 0.0);
            }
            assert!(decoded.indices.iter().all(|&i| (i as usize) < decoded.vertices.len()));
        }
        // the wood, the field, and in `holed` the mire in the field's hole
        let paints = |grid: &LandGrid| landcover_ground::placed(tile, grid).iter().map(|a| a.paint).collect::<Vec<_>>();
        assert_eq!(paints(&three_bands()), [area::WOOD, area::FARMLAND]);
        let holed_areas = landcover_ground::placed(tile, &holed());
        assert_eq!(holed_areas.iter().map(|a| a.paint).collect::<Vec<_>>(), [area::WOOD, area::FARMLAND, area::WETLAND]);
        assert!(holed_areas[1].polygon.len() == 2, "the field has the mire as its hole");
        // nothing to draw: towns and water alone, an answer of no grid
        let town = LandGrid { size: 16, classes: vec![lc::BUILT; 256] };
        assert_eq!(decode(tile, &town.classes, &Cancel::new()), Ok(None));
        assert_eq!(decode(tile, &[1, 2, 3], &Cancel::new()), Ok(None));
        assert_eq!(decode(tile, &[], &Cancel::new()), Ok(None));
        // cancelled between areas
        assert_eq!(tessellate(tile, &three_bands(), &|| true), None);
    }

    /// The normalised rings: closed, the outline clockwise and the holes counter-clockwise in
    /// longitude and latitude (y up), whichever way they came.
    #[test]
    fn rings_are_normalised_as_deck_normalises_them() {
        let square = |cw: bool, s: f64| -> Ring {
            let mut r = vec![[0.0, 0.0], [0.0, s], [s, s], [s, 0.0]];
            if !cw {
                r.reverse();
            }
            r
        };
        for (outer_cw, hole_cw) in [(true, true), (false, false), (true, false), (false, true)] {
            let mut hole = square(hole_cw, 1.0);
            for p in &mut hole {
                p[0] += 1.0;
                p[1] += 1.0;
            }
            let mut outer = square(outer_cw, 4.0);
            outer.push(outer[0]);
            let (positions, holes) = normalize(&[outer, hole]);
            assert_eq!(holes, [10]);
            assert_eq!(positions.len(), 20);
            assert!(math_gl_area(&positions, 0, 10) > 0.0);
            assert!(math_gl_area(&positions, 10, 20) < 0.0);
            assert_eq!(positions[..2], positions[8..10]);
            assert_eq!(positions[10..12], positions[18..20]);
        }
    }

    /// The fixture folder's land cover is NMD's pictures' classes (none of the fixture's Swedish
    /// tiles is clear anywhere, so WorldCover is never read for them); a tile it lacks is noted.
    #[test]
    fn a_fixture_folder_s_land_cover_is_nmd_s_classes() {
        let dir = DirTiles::fixtures();
        for tile in dir.tiles(NMD) {
            let png = std::fs::read(dir.root().join(format!("{NMD}/{}/{}/{}.png", tile.z, tile.x, tile.y))).unwrap();
            let classes = land_classes(&png, true).expect("a picture");
            assert!(!classes.contains(&lc::NONE), "{tile:?}");
            let answer = fixture_land_cover(&dir, tile).expect("read").expect("classes");
            assert_eq!(answer, classes);
            assert_eq!(grid_of(&answer).map(|g| g.size), Some(256));
        }
        assert!(dir.misses().is_empty(), "{:?}", dir.misses());
        // Arlanda's west, as PIL counts its classes (backend::landcover's test has the same)
        let answer = fixture_land_cover(&dir, TileIndex::new(9007, 4787, 14)).unwrap().unwrap();
        assert_eq!(answer.iter().filter(|&&c| c == lc::CONIFERS).count(), 10873);
        assert_eq!(answer.iter().filter(|&&c| c == lc::CLEARCUT).count(), 13577);
        // a tile the folder lacks: none, noted
        assert_eq!(fixture_land_cover(&dir, TileIndex::new(1, 1, 14)), Ok(None));
        assert_eq!(dir.misses(), [(WORLDCOVER.to_owned(), TileIndex::new(1, 1, 14))]);
        // and through the folder's fetcher
        assert_eq!(dir.read(LAND_COVER, TileIndex::new(9007, 4787, 14)), Ok(Some(answer)));
    }

    // ------------------------------------------------------------------ on the GPU

    /// The fixtures, with the land cover there or not.
    fn fetcher(land_cover: bool) -> Arc<dyn TileFetcher> {
        let dir = DirTiles::fixtures();
        Arc::new(FnFetcher(move |source: &str, tile: TileIndex| -> FetchResult {
            if source == LAND_COVER && !land_cover {
                return Ok(None);
            }
            dir.read(source, tile)
        }))
    }

    /// The realistic map of `view` as the app's scene builder has it (sky, earth, ground, relief),
    /// sunny at midsummer noon.
    fn realistic(view: ViewState) -> SceneInput {
        let mut scene = SceneInput::new(view, SIZE, 1.0, Basemap::Realistic);
        scene.local_now_ms = MIDSUMMER;
        scene.feed_now_ms = MIDSUMMER;
        scene.earth = earth::Input::SHOWN;
        scene.sky = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
        scene.ground = ground::Input::SHOWN;
        scene.hillshade = hillshade::Input { shown: true, terrain: false };
        scene
    }

    fn median(values: &mut [[u8; 3]]) -> [u8; 3] {
        std::array::from_fn(|c| {
            let mut v: Vec<u8> = values.iter().map(|p| p[c]).collect();
            v.sort_unstable();
            v[v.len() / 2]
        })
    }

    fn distance(a: [u8; 3], b: [u8; 3]) -> f64 {
        (0..3).map(|i| (f64::from(a[i]) - f64::from(b[i])).powi(2)).sum::<f64>().sqrt()
    }

    /// The acceptance shot (plan R8): V4 from the fixtures. Where the map leaves the land unmapped
    /// and NMD has forest all round (every class within two cells wooded, so the softened outline
    /// holds the place), the land cover makes the map's grass into woods: those pixels change, and
    /// they come out in today's hues (`realistic-v4-1440x900.png`), where without it they were the
    /// unmapped land's grass, far from today's. And the land cover shows nowhere the map's own
    /// areas lie: no pixel changes where the CPU finds no land cover.
    #[test]
    fn v4_s_unmapped_forest_shows_as_woods() {
        let Some(gpu) = gpu() else { return };
        let scene = realistic(V4);
        let shoot = |land_cover: bool| {
            let mut renderer = MapRenderer::new(&gpu, FORMAT);
            renderer.set_tiles(fetcher(land_cover), None);
            let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
            assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
            image
        };
        let (with, without) = (shoot(true), shoot(false));
        with.save_png(&shots_dir().join("r8-v4-land-cover.png")).expect("png");
        without.save_png(&shots_dir().join("r8-v4-without.png")).expect("png");
        let today = reference("realistic-v4-1440x900.png");
        // the land cover's classes under each pixel, from the fixtures' z14 tiles
        let dir = DirTiles::fixtures();
        let mut grids: HashMap<TileIndex, Option<LandGrid>> = HashMap::new();
        let view = ViewFrame::new(&V4, SIZE, 1.0, 0.0);
        let mut class_at_point = |[lon, lat]: [f64; 2], dx: i32, dy: i32| -> Option<LandClass> {
            let [wx, wy] = lng_lat_to_world([lon, lat]);
            let n = 2f64.powi(14);
            let (tx, ty) = (wx / 512.0 * n, (512.0 - wy) / 512.0 * n);
            let (cx, cy) = ((tx.fract() * 256.0).floor() as i32 + dx, (ty.fract() * 256.0).floor() as i32 + dy);
            let tile = TileIndex::new((tx.floor() as i64 + i64::from(cx.div_euclid(256))) as u32, (ty.floor() as i64 + i64::from(cy.div_euclid(256))) as u32, 14);
            let grid = grids.entry(tile).or_insert_with(|| fixture_land_cover(&dir, tile).ok().flatten().and_then(|c| grid_of(&c))).as_ref()?;
            Some(class_at(grid, (f64::from(cx.rem_euclid(256)) + 0.5) / 256.0, (f64::from(cy.rem_euclid(256)) + 0.5) / 256.0))
        };
        let (mut forest, mut changed_elsewhere, mut checked) = (Vec::new(), 0, 0);
        // the map in today's picture: right of the sidebar, left of the map controls, under the
        // mode bar, above the credits (today's pictures stand about 4.5 px higher, T1)
        for y in (80..880).step_by(3) {
            for x in (340..1380).step_by(3) {
                let point = view.viewport.unproject([f64::from(x) + 0.5, f64::from(y) + 0.5], 0.0);
                let (a, b) = (with.pixel(x, y), without.pixel(x, y));
                let changed = (0..3).any(|c| a[c].abs_diff(b[c]) > 6);
                let deep = (-2..=2).all(|dy| (-2..=2).all(|dx| class_at_point(point, dx, dy).is_some_and(wooded)));
                let none_near = (-4..=4).all(|dy| (-4..=4).all(|dx| class_at_point(point, dx, dy).is_none_or(|c| matches!(c, lc::BUILT | lc::WATER | lc::NONE))));
                checked += 1;
                if none_near && changed {
                    changed_elsewhere += 1;
                }
                if deep && changed {
                    let t = today.pixel(x, y - 4);
                    forest.push(([a[0], a[1], a[2]], [b[0], b[1], b[2]], [t[0], t[1], t[2]]));
                }
            }
        }
        eprintln!("{} unmapped forest pixels of {checked}; {changed_elsewhere} changed with no land cover near", forest.len());
        assert!(forest.len() > 300, "too little unmapped forest: {}", forest.len());
        assert_eq!(changed_elsewhere, 0);
        let (mut ours, mut bare, mut theirs): (Vec<_>, Vec<_>, Vec<_>) = (Vec::new(), Vec::new(), Vec::new());
        for &(a, b, t) in &forest {
            ours.push(a);
            bare.push(b);
            theirs.push(t);
        }
        let (ours, bare, theirs) = (median(&mut ours), median(&mut bare), median(&mut theirs));
        eprintln!("unmapped forest: with the land cover {ours:?}, without {bare:?}, today's {theirs:?}");
        assert!(distance(ours, theirs) <= 10.0, "{ours:?} against today's {theirs:?}");
        assert!(distance(bare, theirs) > distance(ours, theirs) + 8.0, "without {bare:?}, with {ours:?}, today's {theirs:?}");
    }

    /// Only the realistic flat map asks for land cover: not the globe, not the dark map, not a
    /// scene without the ground.
    #[test]
    fn land_cover_is_asked_for_only_on_the_realistic_flat_map() {
        let Some(gpu) = gpu() else { return };
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let noted = asked.clone();
        let dir = DirTiles::fixtures();
        let fetcher: Arc<dyn TileFetcher> = Arc::new(FnFetcher(move |source: &str, tile: TileIndex| -> FetchResult {
            if source == LAND_COVER {
                noted.lock().unwrap().push(tile);
            }
            dir.read(source, tile)
        }));
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(fetcher, None);
        let globe = ViewState { zoom: 11.0, ..V4 };
        let mut dark = realistic(V4);
        dark.basemap = Basemap::Dark;
        let mut bare = realistic(V4);
        bare.ground = ground::Input::default();
        for scene in [realistic(globe), dark, bare] {
            render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
            assert!(asked.lock().unwrap().is_empty(), "{:?}", scene.basemap);
        }
        render_with(&gpu, &mut renderer, &realistic(V4), 1.0).expect("rendered");
        let asked = asked.lock().unwrap().clone();
        let want = tile_indices(&ViewFrame::new(&V4, SIZE, 1.0, 0.0).viewport, &TILES);
        assert!(!asked.is_empty() && asked.iter().all(|t| want.contains(t)), "{asked:?}");
    }
}
