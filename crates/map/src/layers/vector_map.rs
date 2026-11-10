//! The dark basemap's vector map (item B2; doc 01 §5.1, §5.6; row 3k of doc 03 §3): OpenFreeMap's
//! tiles of `water`, `boundary`, `transportation` and `aeroway` as today's app draws them with
//! deck.gl 9.4's MVTLayer (`map-globe` / `map-flat`, G:587-608), over the land rectangle and under
//! everything else. The true-width runways (row 3l, `aeroway-true-*`, doc 01 §5.7) are item B3's
//! part of this file.
//!
//! What today's app does, and so this layer:
//! - **When.** On the dark basemap only, once the TileJSON has given the tile URL (G:2672). The
//!   scene builder sets [`Input::shown`] (`app::scene_builder::base`); the URL is the app's
//!   fetcher's business (`tile_fetcher::TileSources::app` reads the TileJSON once, never
//!   hard-coded): if the TileJSON cannot be read, every tile settles empty and the map shows only
//!   the land, as today's app shows no vector layer then.
//! - **Tiles.** deck's rule for 512-px tiles, zooms 0–14 (`tiles::VECTOR_TILES`: `round(zoom)`,
//!   over-zoomed past 14), TileLayer's defaults otherwise: `maxRequests: 6` (deck's default; the
//!   app sets none, and plan 1.4.8's "8 unless the spec says otherwise" yields to deck),
//!   `best-available` refinement, a cache of five times the selection. Each tile is decoded on the
//!   worker pool by `scene::tess::vector::tessellate` (item B1) into fills and path segments in
//!   tile units, uploaded within the frame's budget, and drawn in its tile frame
//!   ([`PrepareCx::tile_frame`]). Tiles draw in deck's order: the cache's creation order sorted by
//!   zoom, so finer tiles cover the coarser ones standing in for them ([`TileSet::visible`]).
//! - **One tile set for both projections** (plan 1.4.2). deck keeps a layer and a cache per
//!   projection (`map-globe` up to zoom 12, `map-flat` above), so crossing zoom 12 refetches and
//!   redecodes every tile and can flash the map empty. Here one [`TileSet`] serves both: B1
//!   tessellates a tile the way deck draws it in the projection that asks for it (z ≤ 12 the
//!   globe's way, z13–14 the flat map's), and the camera's frame puts the same mesh on the sphere
//!   or the plane. Crossing zoom 12 asks for no new tile (test
//!   `crossing_zoom_12_asks_for_no_new_tile`).
//! - **Order within a tile.** All of a tile's fills, then all of its lines, each in feature order
//!   (deck's GeoJsonLayer: polygon fills, then line strings), tile after tile; `SURFACE_NO_CULL`
//!   for both (`parameters: SURFACE_NO_CULL`: depth-tested against the earth sphere, no depth
//!   written, nothing culled, as earcut's triangles turn either way). Fills are opaque in their
//!   vertex colours (`getFillColor`); lines are deck's PathLayer defaults with widths in pixels
//!   (`lineWidthUnits: "pixels"`: butt caps, miter joints of limit 4, no dashes, no clamp).
//! - **Clipped on the flat map, not on the globe.** MVTLayer gives each tile's sublayers a
//!   `ClipExtension` (bounds `[0, 0, 1, 1]` in tile units) when the viewport is not geospatial
//!   (`mvt-layer.js`, `renderSubLayers`: `!this.context.viewport.resolution`), so on the flat map a
//!   tile's polygons and lines are cut at the tile's square and the buffer the tiles carry round
//!   them never shows; the globe's tiles (`resolution: 5`) are not clipped, and a tile's buffer
//!   draws over its neighbour drawn before it. Neither PathLayer nor SolidPolygonLayer has
//!   `instancePositions`, so deck clips per fragment: a fragment whose common position lies
//!   outside the square is discarded. Here the square is cut with the stencil buffer, which the
//!   shared pipelines leave untouched until the shadows: per tile on the flat map, the tile's
//!   square is drawn with alpha 0 and depth test `Always`, raising the stencil off 0 ([`CLIP_MARK`];
//!   alpha 0 under deck's blending changes no colour), the tile's fills and lines are drawn where
//!   the stencil is not 0 ([`CLIPPED`]), and the square is drawn once more setting it back to 0
//!   ([`CLIP_RESET`]). The stencil reference stays 0 throughout, and every pixel the next layer sees
//!   holds 0 again, as the `SHADOW` preset needs. Deviation, invisible: deck's discard is per pixel
//!   (aliased), the stencil per MSAA sample, so the cut at a tile's edge is antialiased; two tiles
//!   still meet without a gap or an overlap.
//!
//! When the layer stops drawing (another basemap) it drops its tiles, as deck finalizes a layer
//! that leaves the list; it is [`Layer::pending`] only while it draws ([`TileSet::loading`]).
//!
//! The decoded tile, [`VectorTile`], is scene's; this file gives it the tile cache's
//! [`ByteSize`].

use rustradar_core::geo::Basemap;
use rustradar_scene::tess::vector::{self, FillVertex, VectorTile};
use rustradar_scene::tiles::VECTOR_TILES;

use super::{ByteSize, DeckSelect, DrawCx, Init, Layer, Pass, PrepareCx, Source, TileIndex, TileSet, TileSetOptions};
use crate::frame::FrameSlot;
use crate::io;
use crate::jobs::Cancel;
use crate::pipelines::fill::{self, Fill};
use crate::pipelines::path::{self, Path, PathStyle};
use crate::state::DrawState;

/// deck TileLayer's `maxRequests` default, which the app's MVTLayers keep.
pub const MAX_REQUESTS: usize = 6;

/// The flat map's clip, step 1: the tile's square raises the stencil off 0 wherever it covers a
/// sample, whatever the depth, and writes no colour (its vertices have alpha 0).
pub const CLIP_MARK: DrawState = DrawState {
    cull: None,
    depth_write: false,
    depth_compare: wgpu::CompareFunction::Always,
    stencil: Some(wgpu::StencilFaceState {
        compare: wgpu::CompareFunction::Always,
        fail_op: wgpu::StencilOperation::Keep,
        depth_fail_op: wgpu::StencilOperation::Keep,
        pass_op: wgpu::StencilOperation::IncrementClamp,
    }),
};

/// The flat map's clip, step 2: `SURFACE_NO_CULL`, drawn only inside the tile's square (stencil
/// not 0; the reference is 0).
pub const CLIPPED: DrawState = DrawState {
    stencil: Some(wgpu::StencilFaceState {
        compare: wgpu::CompareFunction::NotEqual,
        fail_op: wgpu::StencilOperation::Keep,
        depth_fail_op: wgpu::StencilOperation::Keep,
        pass_op: wgpu::StencilOperation::Keep,
    }),
    ..DrawState::SURFACE_NO_CULL
};

/// The flat map's clip, step 3: the square again, setting the stencil back to 0.
pub const CLIP_RESET: DrawState = DrawState {
    stencil: Some(wgpu::StencilFaceState {
        compare: wgpu::CompareFunction::Always,
        fail_op: wgpu::StencilOperation::Zero,
        depth_fail_op: wgpu::StencilOperation::Zero,
        pass_op: wgpu::StencilOperation::Zero,
    }),
    ..CLIP_MARK
};

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// Whether the vector map is drawn: on the dark basemap (G:2668-2676). Off in a scene made
    /// with `SceneInput::new`; the layer also draws only when the scene's basemap is dark.
    pub shown: bool,
}

impl Input {
    /// The vector map drawn.
    pub const SHOWN: Input = Input { shown: true };
}

impl ByteSize for VectorTile {
    fn byte_size(&self) -> usize {
        VectorTile::byte_size(self)
    }
}

/// A tile on the GPU: its fills and its lines, in tile units.
pub struct TileMeshes {
    pub fills: fill::Mesh<fill::Vertex>,
    pub lines: path::Mesh,
}

/// The decode step, on the worker pool: the tile's bytes tessellated by B1 (`None` for a tile
/// with nothing to draw).
pub fn decode(tile: TileIndex, bytes: &[u8], cancel: &Cancel) -> Result<Option<VectorTile>, String> {
    match vector::tessellate(tile, bytes, &|| cancel.is_cancelled()) {
        Ok(decoded) if decoded.is_empty() => Ok(None),
        Ok(decoded) => Ok(Some(decoded)),
        Err(vector::Cancelled) => Err("cancelled".into()),
    }
}

/// The upload step, on the main thread: a decoded tile's buffers.
pub fn upload(device: &wgpu::Device, decoded: &VectorTile) -> TileMeshes {
    // `FillVertex` is `fill::Vertex`'s layout (16 bytes, `#[repr(C)]`)
    let vertices: &[fill::Vertex] = bytemuck::cast_slice::<FillVertex, fill::Vertex>(&decoded.fill_vertices);
    TileMeshes {
        fills: fill::Mesh::new(device, "vector map fills", vertices, &decoded.fill_indices),
        lines: path::Mesh::new(device, "vector map lines", &decoded.lines),
    }
}

/// The fill and path pipelines of one draw state, with their styles.
struct Draws {
    fill: Fill,
    fill_style: fill::Style,
    path: Path,
    path_style: path::Style,
}

impl Draws {
    fn new(init: &Init<'_>, label: &str, state: DrawState) -> Self {
        let device = &init.gpu.device;
        let fill = Fill::new(init, label, state);
        let fill_style = fill.style(device, fill::OPAQUE);
        let path = Path::new(init, label, state);
        // `lineWidthUnits: "pixels"`, PathLayer's defaults otherwise
        let path_style = path.style(device, &PathStyle::PIXELS);
        Draws { fill, fill_style, path, path_style }
    }

    /// A tile's fills, then its lines.
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, slot: FrameSlot, meshes: &TileMeshes) {
        if meshes.fills.index_count() > 0 {
            self.fill.draw(pass, cx, slot, &self.fill_style, &meshes.fills);
        }
        if !meshes.lines.is_empty() {
            self.path.draw(pass, cx, slot, &self.path_style, &meshes.lines);
        }
    }
}

/// Rows 3k and 3l: `map-*`, `aeroway-true-*` (dark basemap).
pub struct VectorMap {
    tiles: TileSet<VectorTile, TileMeshes>,
    /// The globe's draws (`SURFACE_NO_CULL`).
    globe: Draws,
    /// The flat map's draws, inside the tile's square ([`CLIPPED`]).
    clipped: Draws,
    /// The tile's square, `[0, 1]²` in tile units, with alpha 0, and its two stencil passes.
    square: fill::Mesh<fill::Vertex>,
    mark: Fill,
    mark_style: fill::Style,
    reset: Fill,
    reset_style: fill::Style,
    /// Whether this frame draws (the dark basemap with [`Input::shown`]).
    shown: bool,
    /// Whether this frame is the flat map (tiles clipped to their squares).
    flat: bool,
    /// This frame's tiles in draw order, each with its frame.
    drawn: Vec<(FrameSlot, TileIndex)>,
}

impl VectorMap {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let tiles = init.tile_set(
            vec![Source::new(io::OPENFREEMAP)],
            DeckSelect(VECTOR_TILES),
            TileSetOptions { max_requests: MAX_REQUESTS, ..Default::default() },
            |tile, bytes, cancel| decode(tile, &bytes, cancel),
        );
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|[u, v]| fill::Vertex { position: [u, v, 0.0], color: [0; 4] });
        let square = fill::Mesh::new(device, "vector map clip", &corners, &[0, 1, 2, 0, 2, 3]);
        let mark = Fill::new(init, "vector map clip", CLIP_MARK);
        let reset = Fill::new(init, "vector map clip reset", CLIP_RESET);
        let mark_style = mark.style(device, fill::OPAQUE);
        let reset_style = reset.style(device, fill::OPAQUE);
        VectorMap {
            tiles,
            globe: Draws::new(init, "vector map", DrawState::SURFACE_NO_CULL),
            clipped: Draws::new(init, "vector map clipped", CLIPPED),
            square,
            mark,
            mark_style,
            reset,
            reset_style,
            shown: false,
            flat: false,
            drawn: Vec::new(),
        }
    }

    /// The tile set (for tests and the true-width runways).
    pub fn tiles(&self) -> &TileSet<VectorTile, TileMeshes> {
        &self.tiles
    }
}

impl Layer for VectorMap {
    fn name(&self) -> &'static str {
        "vector-map"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.drawn.clear();
        self.shown = cx.scene.vector_map.shown && cx.scene.basemap == Basemap::Dark;
        if !self.shown {
            // deck drops a layer that leaves the list, and its tiles with it
            if !self.tiles.is_empty() {
                self.tiles.clear();
            }
            return;
        }
        self.flat = !cx.view.is_globe();
        cx.update_tiles(&mut self.tiles, |gpu, _tile, decoded| upload(&gpu.device, &decoded));
        for (tile, _) in self.tiles.visible() {
            self.drawn.push((cx.tile_frame(tile, 0.0), tile));
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if cx.pass != Pass::Map {
            return;
        }
        for &(slot, tile) in &self.drawn {
            let Some(meshes) = self.tiles.get(tile) else { continue };
            if self.flat {
                self.mark.draw(pass, cx, slot, &self.mark_style, &self.square);
                self.clipped.draw(pass, cx, slot, meshes);
                self.reset.draw(pass, cx, slot, &self.reset_style, &self.square);
            } else {
                self.globe.draw(pass, cx, slot, meshes);
            }
        }
    }

    fn pending(&self) -> bool {
        self.shown && self.tiles.loading()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path as FilePath, PathBuf};
    use std::sync::mpsc::Receiver;
    use std::sync::{Arc, Mutex};

    use rustradar_scene::camera::ViewState;
    use rustradar_scene::tess::vector::{BORDER, BORDER_STATE, LAND, ROAD_MAJOR, ROAD_MINOR, RUNWAY, TAXIWAY, WATER};
    use rustradar_scene::tiles::osm_tile_to_lng_lat;

    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{DirTiles, FetchRequest, FetchResult, TileFetcher};
    use crate::layers::earth;
    use crate::renderer::{MapRenderer, SceneInput};

    fn view(latitude: f64, longitude: f64, zoom: f64, pitch: f64, bearing: f64) -> ViewState {
        ViewState { latitude, longitude, zoom, pitch, bearing, position: None }
    }

    /// The plan's named views (section 8).
    fn v2() -> ViewState {
        view(52.0, 10.0, 4.5, 0.0, 0.0)
    }
    fn v3() -> ViewState {
        view(59.33, 18.07, 9.0, 0.0, 0.0)
    }
    fn v4() -> ViewState {
        view(59.6519, 17.9186, 13.5, 45.0, 30.0)
    }

    /// The fixture tiles, with every request noted.
    struct Noted {
        tiles: DirTiles,
        asked: Mutex<Vec<(String, TileIndex)>>,
    }

    impl Noted {
        fn new() -> Arc<Self> {
            Arc::new(Noted { tiles: DirTiles::fixtures(), asked: Mutex::new(Vec::new()) })
        }

        fn asked(&self) -> Vec<(String, TileIndex)> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl TileFetcher for Noted {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            self.asked.lock().unwrap().push((request.source.to_string(), request.tile));
            self.tiles.fetch(request)
        }
    }

    /// The dark map's scene of `view` on a `size` canvas: the earth, the land and the vector map.
    fn dark(view: ViewState, size: [f64; 2]) -> SceneInput {
        let mut scene = SceneInput::new(view, size, 1.0, Basemap::Dark);
        scene.earth = earth::Input::SHOWN;
        scene.vector_map = Input::SHOWN;
        scene
    }

    fn renderer(gpu: &crate::gpu::Gpu, fetcher: Arc<dyn TileFetcher>) -> MapRenderer {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        renderer.set_tiles(fetcher, None);
        renderer
    }

    fn shots_dir() -> PathBuf {
        let dir = FilePath::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    fn save(image: &Image, name: &str) {
        image.save_png(&shots_dir().join(name)).expect("the shot written");
    }

    fn reference(name: &str) -> Image {
        let path = FilePath::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference").join(name);
        let rgba = image::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).into_rgba8();
        Image { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() }
    }

    /// Pixels of `image` in `[x0, x1) × [y0, y1)` whose RGB is `rgb`.
    fn count(image: &Image, [x0, y0, x1, y1]: [u32; 4], rgb: [u8; 3]) -> usize {
        (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| image.pixel(x, y)[..3] == rgb).count()
    }

    fn rgb(c: [u8; 4]) -> [u8; 3] {
        [c[0], c[1], c[2]]
    }

    /// The map in today's pictures at 1440 × 900: right of the sidebar, left of the map controls,
    /// under the mode bar and the dev readout.
    const MAP: [u32; 4] = [360, 80, 1370, 880];

    /// How many of today's pixels of each of `classes` (inside [`MAP`]) this shot gives the same
    /// colour, at the best shift within 6 px (today's pictures stand about 4.5 px higher and 0.5 px
    /// left of deck's own viewport; T1): the share per class, and the shift.
    fn agreement(ours: &Image, today: &Image, classes: &[[u8; 3]]) -> (Vec<f64>, [i32; 2]) {
        let [x0, y0, x1, y1] = MAP;
        let mut best = (Vec::new(), [0, 0], -1.0);
        for dy in -6..=6 {
            for dx in -6..=6 {
                let mut total = vec![0usize; classes.len()];
                let mut kept = vec![0usize; classes.len()];
                for y in y0..y1 {
                    for x in x0..x1 {
                        let theirs = rgb(today.pixel(x, y));
                        let Some(class) = classes.iter().position(|&c| c == theirs) else { continue };
                        total[class] += 1;
                        let mine = rgb(ours.pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32));
                        kept[class] += usize::from(mine == theirs);
                    }
                }
                let shares: Vec<f64> = kept.iter().zip(&total).map(|(&k, &t)| if t == 0 { 1.0 } else { k as f64 / t as f64 }).collect();
                let score: f64 = shares.iter().sum();
                if score > best.2 {
                    best = (shares, [dx, dy], score);
                }
            }
        }
        (best.0, best.1)
    }

    /// A shot of the dark map at `view` and 1440 × 900 from the fixtures, saved as `name`, with no
    /// tile missing.
    fn shot(view: ViewState, name: &str) -> Option<Image> {
        let gpu = gpu()?;
        let fixtures = Noted::new();
        let mut renderer = renderer(&gpu, fixtures.clone());
        let image = render_with(&gpu, &mut renderer, &dark(view, [1440.0, 900.0]), 1.0).expect("rendered");
        save(&image, name);
        assert!(!renderer.pending(), "still loading: {:?}", renderer.pending_layers());
        assert!(fixtures.tiles.misses().is_empty(), "the fixtures lack {:?}", fixtures.tiles.misses());
        assert!(fixtures.asked().iter().all(|(source, _)| source == io::OPENFREEMAP));
        Some(image)
    }

    #[test]
    fn a_fixture_tile_decodes_to_fills_and_lines_of_the_palette() {
        let tile = TileIndex::new(281, 150, 9);
        let bytes = DirTiles::fixtures().read(io::OPENFREEMAP, tile).expect("read").expect("a V3 tile");
        let decoded = decode(tile, &bytes, &Cancel::new()).expect("decoded").expect("something to draw");
        assert!(!decoded.fill_indices.is_empty() && !decoded.lines.is_empty());
        assert!(decoded.fill_vertices.iter().any(|v| v.color == WATER));
        assert_eq!(ByteSize::byte_size(&decoded), decoded.byte_size());
        // nothing in it: nothing to draw
        assert_eq!(decode(tile, &[], &Cancel::new()), Ok(None));
        // cancelled: stopped
        let cancel = Cancel::new();
        cancel.cancel();
        assert!(decode(tile, &bytes, &cancel).is_err());
    }

    /// V2 (Europe, zoom 4.5, z4–5 tiles on the globe): the sea in the map's water over the land,
    /// country and state borders, motorways; where today's app has sea and land this shot has
    /// them too.
    #[test]
    fn v2_draws_sea_land_borders_and_roads_where_today_s_app_does() {
        let Some(image) = shot(v2(), "b2-dark-v2-1440x900.png") else { return };
        let (sea, land) = (count(&image, MAP, rgb(WATER)), count(&image, MAP, rgb(LAND)));
        let borders = count(&image, MAP, rgb(BORDER)) + count(&image, MAP, rgb(BORDER_STATE));
        let roads = count(&image, MAP, rgb(ROAD_MAJOR)) + count(&image, MAP, rgb(ROAD_MINOR));
        let (shares, shift) = agreement(&image, &reference("dark-v2-1440x900.png"), &[rgb(WATER), rgb(LAND)]);
        eprintln!("V2: sea {sea}, land {land}, borders {borders}, roads {roads}; today's sea kept {:.4}, land {:.4} at {shift:?}", shares[0], shares[1]);
        assert!(sea > 100_000 && land > 200_000, "sea {sea}, land {land}");
        assert!(borders > 2_000, "{borders} border pixels");
        assert!(roads > 2_000, "{roads} road pixels");
        assert!(shares[0] > 0.98 && shares[1] > 0.96, "{shares:?}");
    }

    /// V3 (Stockholm, zoom 9, z9 tiles on the globe): the archipelago's water and the lakes, the
    /// roads.
    #[test]
    fn v3_draws_the_water_and_the_roads_where_today_s_app_does() {
        let Some(image) = shot(v3(), "b2-dark-v3-1440x900.png") else { return };
        let (sea, land) = (count(&image, MAP, rgb(WATER)), count(&image, MAP, rgb(LAND)));
        let major = count(&image, MAP, rgb(ROAD_MAJOR));
        let minor = count(&image, MAP, rgb(ROAD_MINOR));
        let (shares, shift) = agreement(&image, &reference("dark-v3-1440x900.png"), &[rgb(WATER), rgb(LAND)]);
        eprintln!("V3: sea {sea}, land {land}, major roads {major}, minor {minor}; today's sea kept {:.4}, land {:.4} at {shift:?}", shares[0], shares[1]);
        assert!(sea > 100_000 && land > 200_000, "sea {sea}, land {land}");
        // (a 0.6 px minor road seldom covers a whole pixel: few pixels take its exact colour)
        assert!(major > 2_000 && minor > 100, "roads {major} major, {minor} minor");
        // (today's picture stands 4.5 px off: half a pixel of every shore of the archipelago is
        // lost at the best whole-pixel shift; 0.959 and 0.960 here)
        assert!(shares[0] > 0.95 && shares[1] > 0.95, "{shares:?}");
    }

    /// V4 (Arlanda, zoom 13.5 pitched 45° and turned 30°, z14 tiles on the flat map): the
    /// runways, taxiways and aprons over the land, the roads round the airport.
    #[test]
    fn v4_draws_the_airport_on_the_flat_map() {
        let Some(image) = shot(v4(), "b2-dark-v4-1440x900.png") else { return };
        let land = count(&image, MAP, rgb(LAND));
        let runway = count(&image, MAP, rgb(RUNWAY));
        let taxiway = count(&image, MAP, rgb(TAXIWAY));
        let apron = count(&image, MAP, [60, 60, 60]);
        let roads = count(&image, MAP, rgb(ROAD_MAJOR)) + count(&image, MAP, rgb(ROAD_MINOR));
        let (shares, shift) = agreement(&image, &reference("dark-v4-1440x900.png"), &[rgb(LAND), [60, 60, 60]]);
        eprintln!(
            "V4: land {land}, runway {runway}, taxiway {taxiway}, apron {apron}, roads {roads}; today's land kept {:.4}, apron {:.4} at {shift:?}",
            shares[0], shares[1]
        );
        assert!(land > 200_000, "land {land}");
        assert!(runway > 1_000 && taxiway > 500 && apron > 10_000, "runway {runway}, taxiway {taxiway}, apron {apron}");
        assert!(roads > 500, "roads {roads}");
        assert!(shares[0] > 0.98 && shares[1] > 0.98, "{shares:?}");
    }

    /// Plan B2: one tile set serves both projections, so at V3 the flat map at zoom 12.1 asks for
    /// no tile the globe at 11.9 has not already asked for, at each of the shots' sizes.
    #[test]
    fn crossing_zoom_12_asks_for_no_new_tile() {
        let Some(gpu) = gpu() else { return };
        for size in [[1440.0, 900.0], [1100.0, 720.0], [390.0, 844.0]] {
            let fixtures = Noted::new();
            let mut renderer = renderer(&gpu, fixtures.clone());
            let globe = render_with(&gpu, &mut renderer, &dark(ViewState { zoom: 11.9, ..v3() }, size), 1.0).expect("rendered");
            let before = fixtures.asked();
            assert!(!before.is_empty() && before.iter().all(|(_, t)| t.z == 12), "{before:?}");
            let flat = render_with(&gpu, &mut renderer, &dark(ViewState { zoom: 12.1, ..v3() }, size), 1.0).expect("rendered");
            let after = fixtures.asked();
            eprintln!("{size:?}: {} tiles at 11.9, {} requests after 12.1", before.len(), after.len());
            assert_eq!(after, before, "crossing zoom 12 asked again");
            assert!(fixtures.tiles.misses().is_empty(), "the fixtures lack {:?}", fixtures.tiles.misses());
            for image in [&globe, &flat] {
                let whole = [0, 0, image.width, image.height];
                assert!(count(image, whole, rgb(WATER)) > 1_000 && count(image, whole, rgb(LAND)) > 1_000);
            }
            if size == [1440.0, 900.0] {
                save(&globe, "b2-dark-v3-z11.9-1440x900.png");
                save(&flat, "b2-dark-v3-z12.1-1440x900.png");
            }
        }
    }

    /// Only one tile of the fixtures: the one under the view's centre.
    struct OneTile(TileIndex, DirTiles);

    impl TileFetcher for OneTile {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            if request.tile == self.0 { self.1.fetch(request) } else { io::answered(Ok(None)) }
        }
    }

    /// The tile of zoom `z` under the view's centre.
    fn tile_under(view: &ViewState, z: u32) -> TileIndex {
        let n = f64::from(1u32 << z);
        let phi = view.latitude.to_radians();
        let x = ((view.longitude + 180.0) / 360.0 * n).floor() as u32;
        let y = ((1.0 - (phi.tan() + 1.0 / phi.cos()).ln() / std::f64::consts::PI) / 2.0 * n).floor() as u32;
        TileIndex::new(x, y, z)
    }

    /// deck clips a tile to its square on the flat map (MVTLayer's ClipExtension) and not on the
    /// globe: with only the centre's z12 tile there, at V3 zoom 12.1 nothing but land lies outside
    /// its square, while at 11.9 the tile's buffer draws water and roads a few pixels beyond it.
    #[test]
    fn a_tile_is_clipped_to_its_square_on_the_flat_map_only() {
        let Some(gpu) = gpu() else { return };
        let size = [1440.0, 900.0];
        for (zoom, clipped) in [(12.1, true), (11.9, false)] {
            let view = ViewState { zoom, ..v3() };
            let tile = tile_under(&view, 12);
            let mut renderer = renderer(&gpu, Arc::new(OneTile(tile, DirTiles::fixtures())));
            let image = render_with(&gpu, &mut renderer, &dark(view, size), 1.0).expect("rendered");
            save(&image, &format!("b2-one-tile-z{zoom}.png"));
            // the square on the screen (straight down, north up: a rectangle)
            let frame = ViewFrame::new(&view, size, 1.0, 0.0);
            let corner = |x: u32, y: u32| {
                let [lon, lat] = osm_tile_to_lng_lat(f64::from(x), f64::from(y), 12);
                frame.viewport.project([lon, lat, 0.0])
            };
            let (nw, se) = (corner(tile.x, tile.y), corner(tile.x + 1, tile.y + 1));
            let (mut inside, mut outside, mut beyond) = (0usize, 0usize, 0.0f64);
            for y in 0..image.height {
                for x in 0..image.width {
                    let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                    // how far outside the square, in pixels (negative inside)
                    let d = (nw[0] - px).max(px - se[0]).max(nw[1] - py).max(py - se[1]);
                    let drawn = rgb(image.pixel(x, y)) != rgb(LAND);
                    if d < -1.0 {
                        inside += usize::from(drawn);
                    } else if d > 1.0 && drawn {
                        outside += 1;
                        beyond = beyond.max(d);
                    }
                }
            }
            eprintln!("zoom {zoom}: tile {tile:?} at {nw:?}..{se:?}: {inside} pixels drawn inside, {outside} outside, up to {beyond:.1} px out");
            assert!(inside > 10_000, "{inside} pixels drawn inside");
            if clipped {
                assert_eq!(outside, 0, "{outside} pixels drawn outside the square, up to {beyond} px");
            } else {
                // OpenMapTiles' buffer: 64 of 4096 units, 7.5 px of a 478 px tile
                assert!(outside > 100 && beyond < 9.0, "{outside} pixels drawn outside, up to {beyond} px");
            }
        }
    }

    /// The layer draws only on the dark map with its input on, and asks for no tile otherwise.
    #[test]
    fn nothing_is_asked_for_off_the_dark_map() {
        let Some(gpu) = gpu() else { return };
        for (basemap, shown) in [(Basemap::Satellite, true), (Basemap::Realistic, true), (Basemap::Dark, false)] {
            let fixtures = Noted::new();
            let mut renderer = renderer(&gpu, fixtures.clone());
            let mut scene = dark(v3(), [320.0, 200.0]);
            scene.basemap = basemap;
            scene.vector_map.shown = shown;
            render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
            assert!(fixtures.asked().is_empty(), "{basemap:?}, shown {shown}: asked {:?}", fixtures.asked());
            assert!(!renderer.pending());
        }
    }
}
