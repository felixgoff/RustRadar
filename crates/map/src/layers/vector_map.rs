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
//! **True-width runways** (item B3; doc 01 §5.7; G:543-565, 2672-2676). Close in, a runway's 4 px
//! centre line would be thinner than the runway, so once a runway 45 m wide is at least 4 px wide
//! at the view's centre (`scene::tess::vector::true_aeroways`, `45 / metersPerPixel(zoom, lat) >=
//! 4`), today's app draws the vector map's runway lines 0 px wide and adds `aeroway-true-globe` /
//! `aeroway-true-flat` right after it: the same tiles' `aeroway` lines, runways in `RUNWAY` 45 m
//! wide (`lineWidthUnits: "meters"`, PathLayer's defaults otherwise), every other line invisible,
//! no fills, `SURFACE_NO_CULL`, and on the flat map clipped to each tile's square like the map.
//! Here each tile with runway lines uploads two more meshes ([`RunwayMeshes`]): its lines without
//! the runways, which the map draws in place of all of them while the switch is on, and the
//! runways 45 m wide, drawn after every tile of the map (so over the taxiways that cross them, as
//! today), tile after tile in the map's order, through the same stencil clip on the flat map. A
//! runway mapped as an area is still filled by the map at every zoom (`vectorFill`).
//! Deviations, invisible once the tiles are in: deck's aeroway layer is an MVTLayer of its own,
//! with its own tile cache that starts loading when the switch turns on (the runways missing
//! meanwhile, since the map has already dropped them); here it shares the map's tiles and draws
//! at once. Today the switch is worked out when the layers are built (each scene refresh, at most
//! 50 ms apart at these zooms); here it is worked out every frame from the frame's camera, the
//! camera the metre widths are measured with.
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
    /// Every line, the runway centre lines 4 px wide (drawn while the true-width runways are off).
    pub lines: path::Mesh,
    /// What a tile with runway centre lines draws while the true-width runways are on (B3).
    pub runways: Option<RunwayMeshes>,
}

impl TileMeshes {
    /// The lines the map draws: all of them, or without the runways while they are drawn at true
    /// width (`getLineWidth` 0 for a runway, G:600-603).
    pub fn map_lines(&self, true_width: bool) -> &path::Mesh {
        match &self.runways {
            Some(runways) if true_width => &runways.others,
            _ => &self.lines,
        }
    }
}

/// A tile's lines while the true-width runways are on (item B3).
pub struct RunwayMeshes {
    /// The map's lines without the runway centre lines (`VectorTile::non_runway_lines`).
    pub others: path::Mesh,
    /// The runway centre lines 45 m wide (`VectorTile::true_width_runways`), widths in metres.
    pub true_width: path::Mesh,
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
    // the true-width switch's two meshes, only for a tile with runway centre lines
    let runways = (!decoded.runway_lines.is_empty()).then(|| RunwayMeshes {
        others: path::Mesh::new(device, "vector map lines without runways", &decoded.non_runway_lines()),
        true_width: path::Mesh::new(device, "runways at true width", &decoded.true_width_runways()),
    });
    TileMeshes {
        fills: fill::Mesh::new(device, "vector map fills", vertices, &decoded.fill_indices),
        lines: path::Mesh::new(device, "vector map lines", &decoded.lines),
        runways,
    }
}

/// The fill and path pipelines of one draw state, with their styles.
struct Draws {
    fill: Fill,
    fill_style: fill::Style,
    path: Path,
    path_style: path::Style,
    /// The true-width runways' style (B3).
    runway_style: path::Style,
}

impl Draws {
    fn new(init: &Init<'_>, label: &str, state: DrawState) -> Self {
        let device = &init.gpu.device;
        let fill = Fill::new(init, label, state);
        let fill_style = fill.style(device, fill::OPAQUE);
        let path = Path::new(init, label, state);
        // `lineWidthUnits: "pixels"`, PathLayer's defaults otherwise
        let path_style = path.style(device, &PathStyle::PIXELS);
        // the aeroway layer's `lineWidthUnits: "meters"`, PathLayer's defaults otherwise
        let runway_style = path.style(device, &PathStyle::DEFAULT);
        Draws { fill, fill_style, path, path_style, runway_style }
    }

    /// A tile's fills, then its lines (without the runways while they are drawn at true width).
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, slot: FrameSlot, meshes: &TileMeshes, true_width: bool) {
        if meshes.fills.index_count() > 0 {
            self.fill.draw(pass, cx, slot, &self.fill_style, &meshes.fills);
        }
        let lines = meshes.map_lines(true_width);
        if !lines.is_empty() {
            self.path.draw(pass, cx, slot, &self.path_style, lines);
        }
    }

    /// A tile's runways at true width.
    fn draw_runways(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, slot: FrameSlot, runways: &path::Mesh) {
        self.path.draw(pass, cx, slot, &self.runway_style, runways);
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
    /// Whether this frame draws the runways at true width (`trueAeroways`, B3).
    true_width: bool,
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
            true_width: false,
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
        self.true_width = vector::true_aeroways(cx.view.view.zoom, cx.view.view.latitude);
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
                self.clipped.draw(pass, cx, slot, meshes, self.true_width);
                self.reset.draw(pass, cx, slot, &self.reset_style, &self.square);
            } else {
                self.globe.draw(pass, cx, slot, meshes, self.true_width);
            }
        }
        if !self.true_width {
            return;
        }
        // `aeroway-true-*`, after the whole map: each tile's runways 45 m wide, clipped alike
        for &(slot, tile) in &self.drawn {
            let Some(runways) = self.tiles.get(tile).and_then(|m| m.runways.as_ref()) else { continue };
            if runways.true_width.is_empty() {
                continue;
            }
            if self.flat {
                self.mark.draw(pass, cx, slot, &self.mark_style, &self.square);
                self.clipped.draw_runways(pass, cx, slot, &runways.true_width);
                self.reset.draw(pass, cx, slot, &self.reset_style, &self.square);
            } else {
                self.globe.draw_runways(pass, cx, slot, &runways.true_width);
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

    use rustradar_scene::camera::{ViewState, world_to_lng_lat};
    use rustradar_scene::geom::path::LocalMercator;
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

    // ------------------------------------------------------------- true-width runways (B3)

    /// deck's metres in degrees of latitude (math.gl's `EARTH_CIRCUMFERENCE`, 40.03e6 m, which
    /// the globe's radius of 6 370 972 m also gives within 5 parts in a million).
    const DEG_PER_M: f64 = 360.0 / 40.03e6;

    /// The runway centre lines of fixture tile `tile` as `[lon, lat]` ends, each cut to the tile's
    /// square (the flat map clips there; past it the next tile draws the runway on).
    fn runways_of(tile: TileIndex) -> Vec<[[f64; 2]; 2]> {
        let bytes = DirTiles::fixtures().read(io::OPENFREEMAP, tile).expect("read");
        let Some(decoded) = bytes.and_then(|b| decode(tile, &b, &Cancel::new()).expect("decoded")) else { return Vec::new() };
        let local = LocalMercator::tile(tile);
        let lng_lat = |[u, v]: [f64; 2]| {
            let w = local.to_world([u, v]);
            world_to_lng_lat([w.x, w.y])
        };
        let mut out = Vec::new();
        for s in decoded.true_width_runways() {
            let (a, b) = ([f64::from(s.start[0]), f64::from(s.start[1])], [f64::from(s.end[0]), f64::from(s.end[1])]);
            // Liang–Barsky against [0, 1]²
            let (mut t0, mut t1) = (0.0f64, 1.0f64);
            for axis in 0..2 {
                let d = b[axis] - a[axis];
                for (p, q) in [(-d, a[axis]), (d, 1.0 - a[axis])] {
                    if p == 0.0 {
                        if q < 0.0 {
                            t1 = -1.0;
                        }
                    } else if p < 0.0 {
                        t0 = t0.max(q / p);
                    } else {
                        t1 = t1.min(q / p);
                    }
                }
            }
            if t1 > t0 {
                let at = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                out.push([lng_lat(at(t0)), lng_lat(at(t1))]);
            }
        }
        out
    }

    /// A runway on the screen: its centre line and its two edges `half_m` metres to either side,
    /// each as two screen points.
    struct Band {
        centre: [[f64; 2]; 2],
        edges: [[[f64; 2]; 2]; 2],
    }

    impl Band {
        fn new(frame: &ViewFrame, [a, b]: [[f64; 2]; 2], half_m: f64) -> Self {
            let screen = |p: [f64; 2]| {
                let s = frame.viewport.project([p[0], p[1], 0.0]);
                [s[0], s[1]]
            };
            // the ground's normal, in metres east and north, then in degrees at each end
            let cos = ((a[1] + b[1]) / 2.0).to_radians().cos();
            let (east, north) = ((b[0] - a[0]) * cos, b[1] - a[1]);
            let len = east.hypot(north);
            let normal = [-north / len * half_m, east / len * half_m];
            let shift = |p: [f64; 2], k: f64| [p[0] + k * normal[0] * DEG_PER_M / p[1].to_radians().cos(), p[1] + k * normal[1] * DEG_PER_M];
            Band {
                centre: [screen(a), screen(b)],
                edges: [[screen(shift(a, 1.0)), screen(shift(b, 1.0))], [screen(shift(a, -1.0)), screen(shift(b, -1.0))]],
            }
        }
    }

    /// Where the screen line through `p` and `q` crosses the height `y`: x, and how far from `p`
    /// to `q` (0 at `p`, 1 at `q`).
    fn crossing([p, q]: [[f64; 2]; 2], y: f64) -> (f64, f64) {
        let t = (y - p[1]) / (q[1] - p[1]);
        (p[0] + (q[0] - p[0]) * t, t)
    }

    /// How much of a pixel of colour `c` the runway covers over the background `b`: `None` when
    /// `c` is no blend of the two (something else is drawn there).
    fn runway_share(c: [u8; 3], b: [u8; 3]) -> Option<f64> {
        let r = rgb(RUNWAY).map(f64::from);
        let (c, b) = (c.map(f64::from), b.map(f64::from));
        let d: [f64; 3] = std::array::from_fn(|i| r[i] - b[i]);
        let t = ((0..3).map(|i| (c[i] - b[i]) * d[i]).sum::<f64>() / (0..3).map(|i| d[i] * d[i]).sum::<f64>()).clamp(0.0, 1.0);
        let off = (0..3).map(|i| (c[i] - b[i] - t * d[i]).abs()).fold(0.0, f64::max);
        (off <= 6.0).then_some(t)
    }

    /// One pixel row across a runway: its width along the row in the shot (the runway colour's
    /// cover of each pixel summed) and between the band's edges; and the sine of the angle
    /// between the band and the row (a width along the row times it is the width across the
    /// runway).
    #[derive(Debug, Clone, Copy)]
    struct Row {
        measured: f64,
        expected: f64,
        sin: f64,
    }

    /// What the rows of `bands` measure in `image`, between 10 % and 90 % of each band's length
    /// (its ends left out), on rows whose background on both sides is plain grey and nothing else
    /// drawn there; and how many pixels inside the bands (1.5 px in from either edge) are not
    /// exactly the runway's colour.
    fn measure(image: &Image, bands: &[Band]) -> (Vec<Row>, usize, usize) {
        let (mut rows, mut inside, mut foreign) = (Vec::new(), 0, 0);
        for band in bands {
            let [p, q] = band.centre;
            let dir = [q[0] - p[0], q[1] - p[1]];
            let sin = dir[1].abs() / dir[0].hypot(dir[1]);
            if sin < 0.3 {
                continue;
            }
            let (y0, y1) = (p[1].min(q[1]).ceil() as i64, p[1].max(q[1]).floor() as i64);
            for y in y0.max(10)..y1.min(i64::from(image.height) - 10) {
                let yc = y as f64 + 0.5;
                let (xc, t) = crossing(band.centre, yc);
                let (xa, _) = crossing(band.edges[0], yc);
                let (xb, _) = crossing(band.edges[1], yc);
                let (left, right) = (xa.min(xb), xa.max(xb));
                if !(0.1..=0.9).contains(&t) || left < 10.0 || right > f64::from(image.width) - 10.0 {
                    continue;
                }
                let px = |x: i64| rgb(image.pixel(x as u32, y as u32));
                for x in (left + 1.5).ceil() as i64..(right - 1.5).floor() as i64 {
                    inside += 1;
                    foreign += usize::from(px(x) != rgb(RUNWAY));
                }
                let (from, to) = (left.floor() as i64, right.floor() as i64);
                let (bl, br) = (px(from - 3), px(to + 3));
                if bl[0] != bl[1] || bl[1] != bl[2] || br[0] != br[1] || br[1] != br[2] {
                    continue;
                }
                let shares: Option<Vec<f64>> =
                    (from - 1..=to + 1).map(|x| runway_share(px(x), if (x as f64 + 0.5) < xc { bl } else { br })).collect();
                let Some(shares) = shares else { continue };
                let measured: f64 = shares.iter().sum();
                rows.push(Row { measured, expected: right - left, sin });
            }
        }
        (rows, inside, foreign)
    }

    fn median(mut v: Vec<f64>) -> f64 {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    }

    /// The mean of `v` without its lowest and highest tenth. (A pixel holds four samples, so
    /// each row's width is a multiple of a quarter pixel: a median would round, a mean does not.)
    fn trimmed_mean(mut v: Vec<f64>) -> f64 {
        v.sort_by(f64::total_cmp);
        let cut = v.len() / 10;
        let kept = &v[cut..v.len() - cut];
        kept.iter().sum::<f64>() / kept.len() as f64
    }

    /// The share of `v` at or below `limit`.
    fn share_within(v: &[f64], limit: f64) -> f64 {
        v.iter().filter(|&&x| x <= limit).count() as f64 / v.len() as f64
    }

    /// A shot of the dark map from the fixtures at `view` on a `size` canvas, saved as `name`,
    /// with no tile missing; and the tiles it asked for.
    fn runway_shot(view: ViewState, size: [f64; 2], name: &str) -> Option<(Image, Vec<TileIndex>)> {
        let gpu = gpu()?;
        let fixtures = Noted::new();
        let mut renderer = renderer(&gpu, fixtures.clone());
        let image = render_with(&gpu, &mut renderer, &dark(view, size), 1.0).expect("rendered");
        save(&image, name);
        assert!(!renderer.pending(), "still loading: {:?}", renderer.pending_layers());
        assert!(fixtures.tiles.misses().is_empty(), "the fixtures lack {:?}", fixtures.tiles.misses());
        Some((image, fixtures.asked().into_iter().map(|(_, tile)| tile).collect()))
    }

    /// The runway bands `half_m` metres to either side of the runway lines of `tiles`.
    fn bands(frame: &ViewFrame, tiles: &[TileIndex], half_m: f64) -> Vec<Band> {
        tiles.iter().flat_map(|&tile| runways_of(tile)).map(|line| Band::new(frame, line, half_m)).collect()
    }

    /// The plan's acceptance of B3 (shot V4, Arlanda at zoom 13.5 pitched 45° and turned 30°,
    /// where `trueAeroways` is on): the runways are 45 m wide. Across each pixel row the runway's
    /// colour is as wide as the 45 m band projected through the camera within 1 px (within 0.1 px
    /// on average), drawn over the taxiways that cross it, as in today's picture.
    #[test]
    fn v4_draws_the_runways_45_m_wide() {
        assert!(vector::true_aeroways(v4().zoom, v4().latitude));
        let size = [1440.0, 900.0];
        let Some((image, asked)) = runway_shot(v4(), size, "b3-dark-v4-1440x900.png") else { return };
        let frame = ViewFrame::new(&v4(), size, 1.0, 0.0);
        let tiles: Vec<TileIndex> = asked.into_iter().filter(|t| t.z == 14).collect();
        let (rows, inside, foreign) = measure(&image, &bands(&frame, &tiles, vector::RUNWAY_WIDTH_M / 2.0));
        let signed: Vec<f64> = rows.iter().map(|r| r.measured - r.expected).collect();
        let errors: Vec<f64> = signed.iter().map(|e| e.abs()).collect();
        let bias = trimmed_mean(signed);
        let across = trimmed_mean(rows.iter().map(|r| r.measured * r.sin).collect());
        let mpp = rustradar_core::geo::meters_per_pixel(v4().zoom, v4().latitude);
        let (shares, shift) = agreement(&image, &reference("dark-v4-1440x900.png"), &[rgb(RUNWAY)]);
        eprintln!(
            "V4: {} rows, error mean {bias:+.3} px, median {:.3} px, max {:.3} px, {:.4} within 1 px; across {across:.2} px (45 / mpp {:.2} px at the centre, unpitched); {foreign} of {inside} pixels inside not runway; today's runway kept {:.4} at {shift:?}",
            rows.len(),
            median(errors.clone()),
            errors.iter().copied().fold(0.0, f64::max),
            share_within(&errors, 1.0),
            45.0 / mpp,
            shares[0]
        );
        assert!(rows.len() > 300, "{} rows measured", rows.len());
        assert!(bias.abs() < 0.1, "mean error {bias}");
        assert!(share_within(&errors, 1.0) > 0.99, "{} within 1 px", share_within(&errors, 1.0));
        assert_eq!(foreign, 0, "{foreign} of {inside} pixels inside the runways are not runway");
        assert!(shares[0] > 0.9, "today's runway pixels kept: {}", shares[0]);
    }

    /// The plan's "width in px equals 45 / mpp within 1 px" taken literally: V4 straight down
    /// (pitch 0, north up), where the ground's scale is the same across the view.
    #[test]
    fn v4_straight_down_draws_the_runways_45_over_mpp_px_wide() {
        let view = ViewState { pitch: 0.0, bearing: 0.0, ..v4() };
        let size = [1440.0, 900.0];
        let Some((image, asked)) = runway_shot(view, size, "b3-dark-v4-straight-down-1440x900.png") else { return };
        let frame = ViewFrame::new(&view, size, 1.0, 0.0);
        let tiles: Vec<TileIndex> = asked.into_iter().filter(|t| t.z == 14).collect();
        let (rows, inside, foreign) = measure(&image, &bands(&frame, &tiles, vector::RUNWAY_WIDTH_M / 2.0));
        let px = 45.0 / rustradar_core::geo::meters_per_pixel(view.zoom, view.latitude);
        let across: Vec<f64> = rows.iter().map(|r| r.measured * r.sin).collect();
        let errors: Vec<f64> = across.iter().map(|w| (w - px).abs()).collect();
        let mean = trimmed_mean(across);
        eprintln!(
            "V4 straight down: {} rows, 45 / mpp = {px:.3} px, across {mean:.3} px on average, error median {:.3}, max {:.3}; {foreign} of {inside} inside not runway",
            rows.len(),
            median(errors.clone()),
            errors.iter().copied().fold(0.0, f64::max)
        );
        assert!(rows.len() > 300, "{} rows measured", rows.len());
        assert!((mean - px).abs() < 0.1, "{mean} px across on average, 45 / mpp {px}");
        assert!(share_within(&errors, 1.0) > 0.99, "{} within 1 px", share_within(&errors, 1.0));
        assert_eq!(foreign, 0, "{foreign} of {inside} pixels inside the runways are not runway");
    }

    /// At the switch nothing jumps (doc 01 §5.7): at Arlanda on the globe, just below the zoom
    /// where 45 m are 4 px the runway is the map's 4 px line, just above it the 45 m line, each as
    /// wide as it should be within a tenth of a pixel on average, so the two differ by little more
    /// than the 0.06 px the zoom step itself makes.
    #[test]
    fn nothing_jumps_where_the_runways_turn_true_width() {
        // the view on the middle of the z12 tile's runway line, straight down
        let tile = TileIndex::new(2251, 1197, 12);
        let line = *runways_of(tile).iter().max_by(|a, b| {
            let len = |l: &[[f64; 2]; 2]| (l[1][0] - l[0][0]).hypot(l[1][1] - l[0][1]);
            len(a).total_cmp(&len(b))
        })
        .expect("a runway line at z12");
        let [lon, lat] = [(line[0][0] + line[1][0]) / 2.0, (line[0][1] + line[1][1]) / 2.0];
        let switch = (rustradar_core::geo::EARTH_CIRCUMFERENCE_M * lat.to_radians().cos() * vector::RUNWAY_PX / (512.0 * vector::RUNWAY_WIDTH_M)).log2();
        let size = [480.0, 320.0];
        let mut widths = Vec::new();
        for (zoom, on) in [(switch - 0.02, false), (switch + 0.02, true)] {
            assert_eq!(vector::true_aeroways(zoom, lat), on, "zoom {zoom}");
            let view = view(lat, lon, zoom, 0.0, 0.0);
            let name = format!("b3-switch-{}.png", if on { "above" } else { "below" });
            let Some((image, _)) = runway_shot(view, size, &name) else { return };
            let frame = ViewFrame::new(&view, size, 1.0, 0.0);
            assert!(frame.viewport.is_globe());
            let mpp = frame.viewport.meters_per_pixel();
            // the 4 px line below the switch, the 45 m one above
            let expected = if on { vector::RUNWAY_WIDTH_M / mpp } else { vector::RUNWAY_PX };
            let (rows, _, _) = measure(&image, &[Band::new(&frame, line, expected / 2.0 * mpp)]);
            let across = trimmed_mean(rows.iter().map(|r| r.measured * r.sin).collect());
            eprintln!("zoom {zoom:.4} (switch {switch:.4}): {} rows, {across:.3} px across, {expected:.3} px expected", rows.len());
            assert!(rows.len() > 50, "{} rows measured", rows.len());
            assert!((across - expected).abs() < 0.1, "zoom {zoom}: {across} px across, {expected} expected");
            widths.push(across);
        }
        assert!((widths[1] - widths[0]).abs() < 0.2, "the runway jumps from {} to {} px", widths[0], widths[1]);
    }

    /// The true-width runways are clipped to their tile's square on the flat map, as the map is:
    /// with only the tile under V4's centre served, at V4 straight down, its runway line runs past
    /// the square in the tile's buffer and no runway colour shows beyond it.
    #[test]
    fn the_true_width_runways_are_clipped_to_their_tile_on_the_flat_map() {
        let Some(gpu) = gpu() else { return };
        let view = ViewState { pitch: 0.0, bearing: 0.0, ..v4() };
        let size = [1440.0, 900.0];
        let tile = tile_under(&view, 14);
        // the precondition: the tile's runway line reaches past its square
        let bytes = DirTiles::fixtures().read(io::OPENFREEMAP, tile).expect("read").expect("the tile");
        let decoded = decode(tile, &bytes, &Cancel::new()).expect("decoded").expect("something to draw");
        let past = |p: [f32; 3]| p[0] < 0.0 || p[0] > 1.0 || p[1] < 0.0 || p[1] > 1.0;
        assert!(decoded.true_width_runways().iter().any(|s| past(s.start) || past(s.end)), "{tile:?}: no runway line past the square");
        let mut renderer = renderer(&gpu, Arc::new(OneTile(tile, DirTiles::fixtures())));
        let image = render_with(&gpu, &mut renderer, &dark(view, size), 1.0).expect("rendered");
        save(&image, "b3-one-tile-v4-straight-down.png");
        let frame = ViewFrame::new(&view, size, 1.0, 0.0);
        let corner = |x: u32, y: u32| {
            let [lon, lat] = osm_tile_to_lng_lat(f64::from(x), f64::from(y), 14);
            frame.viewport.project([lon, lat, 0.0])
        };
        let (nw, se) = (corner(tile.x, tile.y), corner(tile.x + 1, tile.y + 1));
        let (mut inside, mut outside) = (0usize, 0usize);
        for y in 0..image.height {
            for x in 0..image.width {
                let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                let d = (nw[0] - px).max(px - se[0]).max(nw[1] - py).max(py - se[1]);
                let runway = rgb(image.pixel(x, y)) == rgb(RUNWAY);
                if d < -1.0 {
                    inside += usize::from(runway);
                } else if d > 1.0 {
                    outside += usize::from(runway);
                }
            }
        }
        eprintln!("{tile:?} at {nw:?}..{se:?}: {inside} runway pixels inside, {outside} outside");
        assert!(inside > 1_000, "{inside} runway pixels inside");
        assert_eq!(outside, 0, "{outside} runway pixels outside the square");
    }
}
