//! `ground-city` (item R3; doc 04 §1 row 5a, §6.6, §6.9, §13; `realistic-ground.ts`'s
//! `cityGroundLayer`, `RG:820-858`; G:1041, G:2660): the realistic map's ground on the flat map,
//! its tiles picked by their distance from the camera, Streets GL's close by and OpenMapTiles'
//! further off, drawn with the ground shader.
//!
//! What today's app does, and so this layer:
//! - **When.** On the realistic basemap while the view is the flat map (zoom > 12), in place of
//!   `ground-globe` ([`super::vector::GroundGlobe`]): today's layer list holds one or the other,
//!   so deck drops this layer's tiles when the globe comes back, and here they are cleared.
//! - **Tiles by distance** (`LodMVTLayer` with `GROUND_LOD`, `lod-tileset.ts`): [`LodSelect`] over
//!   [`ground_lod`] (levels 9–14 and 16, bias 0.5, nothing past 80 km; 512-px tiles, no zoom
//!   offset, no `minZoom`), each tile's zoom from its distance to the camera (doc 04 §13.2), with
//!   the look-ahead while zooming (§13.3: the view a level further on, at most 32 tiles, after
//!   every tile in view). Refined by `keepCoarserUntilIn` ([`Refinement::KeepCoarserUntilIn`],
//!   §13.4): a coarser tile is drawn in place of finer ones until they are all in, never with
//!   them, and stays for a finer one that came back empty or failed, so no place is drawn twice
//!   and none is left bare while its tiles load. deck's TileLayer defaults otherwise: 6 requests
//!   at a time (`maxRequests`), a cache of five times the selection.
//! - **Two sources** (`cityGroundSources`, §13.5): zooms 9–14 from OpenMapTiles (OpenFreeMap,
//!   `io::OPENFREEMAP`) reading [`GROUND_LAYERS`], zoom 16 from Streets GL (`io::STREETS_GL`,
//!   `backend::streets_gl`: its only zoom; `sglFetch` asks for no other) reading
//!   [`SGL_GROUND_LAYERS`] (`common`, `natural`, `water`, `highways`: each field, garden, wood
//!   and road as mapped, through `scene::ground`'s rules for Streets GL's `type`s, `sglPath`'s
//!   lanes, widths and surfaces and `fieldPaint`'s rows). Each tile is decoded on the worker pool
//!   by `scene::tess::ground::tessellate` the flat map's way (binary tiles in tile units, lines
//!   open, the bridges split off and raised to their decks), uploaded within the frame's budget,
//!   and drawn in its tile frame, coarse tiles first.
//! - **Each tile's passes** (`GroundTile.renderLayers`, doc 04 §6.9), in this order: its areas,
//!   its lines' edging, its lines (so no road's pavement covers another's asphalt where they
//!   meet), then its bridges: `-bridge-shade` (every bridge flat on the ground in
//!   `BRIDGE_SHADE`, `max(width, edge width) + 2` m wide, unlit: the plain path pipeline, not the
//!   ground shader), `-bridge-edges` and `-bridge-lines` raised to their decks (the ground
//!   shader), and `-bridge-decks` (the raised roads' bridges in NONE, alpha 0, with the depth
//!   written so what is drawn later passes under them; a railway's 0 m wide). `SURFACE_NO_CULL`
//!   throughout (no depth written but by the decks, nothing culled); lines in metres with round
//!   joints and caps, at least 0.6 px wide on OpenMapTiles' tiles and as wide as they are on
//!   Streets GL's (`renderCityGround`: `lineWidthMinPixels: z >= 16 ? 0 : 0.6`; [`line_width`]).
//! - **Clipped to the tile's square** (MVTLayer's `ClipExtension` on the flat map, every pass):
//!   with the stencil, as the vector map does ([`vector_map::CLIP_MARK`], [`vector_map::CLIPPED`],
//!   [`vector_map::CLIP_RESET`]: the square raises the stencil, the passes draw where it is
//!   raised, the square sets it back to 0), so a tile's buffer never shows.
//!
//! Not here: the survey's road widths (`withSurvey`, item R9), the land cover under it (R8), the
//! ground rails and platforms over it (R9).
//!
//! Deviations, invisible:
//! - deck's clip discards by the fragment's own map position, the stencil by where the tile's
//!   square lies on the screen, at the ground: a raised bridge near a tile's edge is cut where
//!   the ground square's edge is seen, not where its deck crosses the edge. Both tiles carry the
//!   bridge in their buffer, so it still shows once and whole;
//! - the clip is per MSAA sample (antialiased) where deck's discard is per pixel.
//!
//! [`TileOverlay`] draws each tile drawn as a square in a colour of its own (its zoom, its column
//! and row), half see-through, instead of the ground: a debug view (tests check with it that the
//! tiles meet without a gap and never overlap).

use std::collections::HashMap;

use rustradar_scene::ground::{GROUND_LAYERS, SGL_GROUND_LAYERS};
use rustradar_scene::lod::{Lod, TILE_SIZE};
use rustradar_scene::streets_gl::SGL_ZOOM;
use rustradar_scene::tess::ground::{self as tess, GroundTile};

use super::vector::Decoded;
use super::{GroundShader, GroundVertex, LineMesh, LineWidth};
use crate::frame::FrameSlot;
use crate::io;
use crate::jobs::Cancel;
use crate::layers::vector_map::{CLIP_MARK, CLIP_RESET, CLIPPED, MAX_REQUESTS};
use crate::layers::{DrawCx, Init, LodSelect, PrepareCx, Refinement, Source, TileIndex, TileSet, TileSetOptions};
use crate::pipelines::fill::{self, Fill};
use crate::pipelines::path::{self, Path, PathStyle, Units};
use crate::state::DrawState;

/// The zooms OpenMapTiles' tiles serve in the city ground (`cityGroundSources`).
pub const OMT_ZOOMS: [u32; 6] = [9, 10, 11, 12, 13, 14];

/// `GROUND_LOD` (G:1041): the realistic ground on the flat map by its distance, Streets GL's zoom
/// 16 close by, OpenMapTiles' at every zoom they have further off, nothing past 80 km.
pub fn ground_lod() -> Lod {
    Lod { levels: vec![9, 10, 11, 12, 13, 14, SGL_ZOOM], bias: 0.5, max_distance: 80_000.0 }
}

/// The city ground's sources by zoom: OpenFreeMap at [`OMT_ZOOMS`], Streets GL at its zoom.
pub fn sources() -> Vec<Source> {
    vec![Source::at(io::OPENFREEMAP, OMT_ZOOMS.to_vec()), Source::at(io::STREETS_GL, vec![SGL_ZOOM])]
}

/// How narrow a tile's lines may get (`renderCityGround`): Streets GL's as wide as they are,
/// OpenMapTiles' at least a hairline.
pub fn line_width(tile: TileIndex) -> LineWidth {
    if tile.z >= SGL_ZOOM { LineWidth::Exact } else { LineWidth::Hairline }
}

/// The source layers a tile reads: Streets GL's at its zoom, else OpenMapTiles'.
pub fn layers(tile: TileIndex) -> &'static [&'static str] {
    if tile.z >= SGL_ZOOM { &SGL_GROUND_LAYERS } else { &GROUND_LAYERS }
}

/// The decode step, on the worker pool: the flat map's tessellation of the tile's source layers
/// (`None` for a tile with nothing to draw).
pub fn decode(tile: TileIndex, bytes: &[u8], cancel: &Cancel) -> Result<Option<Decoded>, String> {
    match tess::tessellate(tile, bytes, layers(tile), true, &|| cancel.is_cancelled()) {
        Ok(decoded) if decoded.is_empty() => Ok(None),
        Ok(decoded) => Ok(Some(Decoded(decoded))),
        Err(tess::Cancelled) => Err("cancelled".into()),
    }
}

/// A tile on the GPU: its seven passes.
pub struct CityMeshes {
    pub areas: fill::Mesh<GroundVertex>,
    pub edges: LineMesh,
    pub lines: LineMesh,
    /// Plain RGBA segments (the plain path pipeline).
    pub bridge_shade: path::Mesh,
    pub bridge_edges: LineMesh,
    pub bridge_lines: LineMesh,
    /// Plain RGBA segments, alpha 0, for the depth buffer.
    pub bridge_decks: path::Mesh,
}

/// The upload step, on the main thread.
pub fn upload(device: &wgpu::Device, decoded: &GroundTile) -> CityMeshes {
    CityMeshes {
        areas: fill::Mesh::new(device, "city ground areas", &decoded.vertices, &decoded.indices),
        edges: LineMesh::new(device, "city ground edges", &decoded.edges),
        lines: LineMesh::new(device, "city ground lines", &decoded.lines),
        bridge_shade: path::Mesh::new(device, "city ground bridge shade", &decoded.bridge_shade),
        bridge_edges: LineMesh::new(device, "city ground bridge edges", &decoded.bridge_edges),
        bridge_lines: LineMesh::new(device, "city ground bridge lines", &decoded.bridge_lines),
        bridge_decks: path::Mesh::new(device, "city ground bridge decks", &decoded.bridge_decks),
    }
}

/// `-bridge-decks`: inside the tile's square, unculled, depth written.
pub const DECKS: DrawState = DrawState { depth_write: true, ..CLIPPED };

/// The plain passes' path props for a tile's [`LineWidth`]: the layer's (metres, round joints and
/// caps, its minimum width).
pub fn plain_style(width: LineWidth) -> PathStyle {
    PathStyle { width_units: Units::Meters, width_min_pixels: width.min_pixels(), joint_rounded: true, cap_rounded: true, ..PathStyle::DEFAULT }
}

/// `[Hairline, Exact]`'s index.
fn width_index(width: LineWidth) -> usize {
    match width {
        LineWidth::Hairline => 0,
        LineWidth::Exact => 1,
    }
}

/// The tile's square, `[0, 1]²` in tile units, and the stencil passes that clip a tile to it.
struct Clip {
    square: fill::Mesh<fill::Vertex>,
    mark: Fill,
    mark_style: fill::Style,
    reset: Fill,
    reset_style: fill::Style,
}

impl Clip {
    fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|[u, v]| fill::Vertex { position: [u, v, 0.0], color: [0; 4] });
        let square = fill::Mesh::new(device, "city ground clip", &corners, &[0, 1, 2, 0, 2, 3]);
        let mark = Fill::new(init, "city ground clip", CLIP_MARK);
        let reset = Fill::new(init, "city ground clip reset", CLIP_RESET);
        let mark_style = mark.style(device, fill::OPAQUE);
        let reset_style = reset.style(device, fill::OPAQUE);
        Clip { square, mark, mark_style, reset, reset_style }
    }
}

/// `ground-city`'s tiles and their pipelines.
pub struct GroundCity {
    tiles: TileSet<Decoded, CityMeshes>,
    /// The ground shader's areas and lines inside the tile's square ([`CLIPPED`]).
    areas: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    /// The plain path pipeline: the bridges' shade ([`CLIPPED`]) and decks ([`DECKS`]), with
    /// [`plain_style`] for each [`LineWidth`].
    shade: Path,
    shade_styles: [path::Style; 2],
    decks: Path,
    deck_styles: [path::Style; 2],
    clip: Clip,
    shown: bool,
    /// This frame's tiles in draw order, each with its frame.
    drawn: Vec<(FrameSlot, TileIndex)>,
}

impl GroundCity {
    /// The layer, its ground pipelines made with `shader`.
    pub fn new(init: &Init<'_>, shader: &GroundShader) -> Self {
        let device = &init.gpu.device;
        let tiles = init.tile_set(
            sources(),
            LodSelect::new(ground_lod(), TILE_SIZE),
            TileSetOptions { max_requests: MAX_REQUESTS, refinement: Refinement::KeepCoarserUntilIn, ..Default::default() },
            |tile, bytes, cancel| decode(tile, &bytes, cancel),
        );
        let shade = Path::new(init, "city ground bridge shade", CLIPPED);
        let decks = Path::new(init, "city ground bridge decks", DECKS);
        let styles = |p: &Path| [LineWidth::Hairline, LineWidth::Exact].map(|w| p.style(device, &plain_style(w)));
        GroundCity {
            tiles,
            areas: shader.areas(init, "city ground areas", CLIPPED),
            lines: shader.lines(init, "city ground lines", CLIPPED),
            shade_styles: styles(&shade),
            deck_styles: styles(&decks),
            shade,
            decks,
            clip: Clip::new(init),
            shown: false,
            drawn: Vec::new(),
        }
    }

    /// The tile set (tests, the overlay).
    pub fn tiles(&self) -> &TileSet<Decoded, CityMeshes> {
        &self.tiles
    }

    /// This frame's tiles in draw order (coarse to fine).
    pub fn drawn(&self) -> impl Iterator<Item = TileIndex> + '_ {
        self.drawn.iter().map(|&(_, tile)| tile)
    }

    /// This frame: the tiles for the camera when `shown` (the realistic map's ground on the flat
    /// map), else none (and the cache dropped, as deck drops the layer).
    pub fn prepare(&mut self, cx: &mut PrepareCx<'_>, shown: bool) {
        self.drawn.clear();
        self.shown = shown;
        if !shown {
            if !self.tiles.is_empty() {
                self.tiles.clear();
            }
            return;
        }
        cx.update_tiles(&mut self.tiles, |gpu, _tile, decoded| upload(&gpu.device, &decoded.0));
        for (tile, _) in self.tiles.visible() {
            self.drawn.push((cx.tile_frame(tile, 0.0), tile));
        }
    }

    /// Each tile's seven passes inside its square, after `shader`'s `prepare` this frame.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, shader: &GroundShader) {
        let clip = &self.clip;
        for &(slot, tile) in &self.drawn {
            let Some(m) = self.tiles.get(tile) else { continue };
            let width = line_width(tile);
            let w = width_index(width);
            clip.mark.draw(pass, cx, slot, &clip.mark_style, &clip.square);
            if m.areas.index_count() > 0 {
                shader.begin(pass, &self.areas, width);
                shader.draw_areas(pass, cx, slot, &m.areas);
            }
            if !m.edges.is_empty() || !m.lines.is_empty() {
                shader.begin(pass, &self.lines, width);
                shader.draw_lines(pass, cx, slot, &m.edges);
                shader.draw_lines(pass, cx, slot, &m.lines);
            }
            if !m.bridge_shade.is_empty() {
                self.shade.draw(pass, cx, slot, &self.shade_styles[w], &m.bridge_shade);
            }
            if !m.bridge_edges.is_empty() || !m.bridge_lines.is_empty() {
                shader.begin(pass, &self.lines, width);
                shader.draw_lines(pass, cx, slot, &m.bridge_edges);
                shader.draw_lines(pass, cx, slot, &m.bridge_lines);
            }
            if !m.bridge_decks.is_empty() {
                self.decks.draw(pass, cx, slot, &self.deck_styles[w], &m.bridge_decks);
            }
            clip.reset.draw(pass, cx, slot, &clip.reset_style, &clip.square);
        }
    }

    /// Whether tiles in view are still to come.
    pub fn pending(&self) -> bool {
        self.shown && self.tiles.loading()
    }
}

/// A debug view of the tiles a [`GroundCity`] draws: each one's square, half see-through, in its
/// [`TileOverlay::colour`], drawn without a clip (so a place two tiles cover shows twice).
pub struct TileOverlay {
    fill: Fill,
    style: fill::Style,
    /// Each drawn tile's square in its colour.
    squares: HashMap<TileIndex, fill::Mesh<fill::Vertex>>,
}

impl TileOverlay {
    /// The overlay's alpha (128 of 255).
    pub const ALPHA: u8 = 128;

    pub fn new(init: &Init<'_>) -> Self {
        let fill = Fill::new(init, "city ground tile overlay", DrawState::SURFACE_NO_CULL);
        let style = fill.style(&init.gpu.device, fill::OPAQUE);
        TileOverlay { fill, style, squares: HashMap::new() }
    }

    /// A tile's colour: red full, green its zoom × 15, blue full on the tiles whose column and
    /// row add up to an odd number (a checkerboard at each zoom), alpha [`Self::ALPHA`].
    pub fn colour(tile: TileIndex) -> [u8; 4] {
        let odd = (tile.x + tile.y) % 2 == 1;
        [255, (tile.z * 15).min(255) as u8, if odd { 255 } else { 0 }, Self::ALPHA]
    }

    /// The squares of the tiles `city` draws this frame (after its `prepare`).
    pub fn prepare(&mut self, cx: &PrepareCx<'_>, city: &GroundCity) {
        let device = &cx.gpu.device;
        self.squares.retain(|tile, _| city.drawn.iter().any(|&(_, t)| t == *tile));
        for &(_, tile) in &city.drawn {
            self.squares.entry(tile).or_insert_with(|| {
                let color = Self::colour(tile);
                let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|[u, v]| fill::Vertex { position: [u, v, 0.0], color });
                fill::Mesh::new(device, "city ground tile overlay", &corners, &[0, 1, 2, 0, 2, 3])
            });
        }
    }

    /// The square of each tile `city` draws this frame, in its colour.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, city: &GroundCity) {
        for &(slot, tile) in &city.drawn {
            if let Some(square) = self.squares.get(&tile) {
                self.fill.draw(pass, cx, slot, &self.style, square);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::{Arc, Mutex};

    use rustradar_core::geo::Basemap;
    use rustradar_core::weather::PRESETS;
    use rustradar_scene::camera::{ViewState, lng_lat_to_world, world_to_lng_lat};
    use rustradar_scene::geom::path::LocalMercator;
    use rustradar_scene::ground::WATER_ID;

    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{DirTiles, FetchRequest, FetchResult, TileFetcher};
    use crate::layers::{Layer, Pass, earth, ground, sky};
    use crate::renderer::{MapRenderer, SceneInput};
    use crate::target::CLEAR_RGB;

    /// 2026-06-21 10:00 UTC: noon in Stockholm (the reference pictures' clock).
    const MIDSUMMER: f64 = 1_782_036_000_000.0;
    /// V5 (plan section 8): Stockholm city, pitched, at today's pictures' size.
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

    /// The source a city tile comes from.
    fn source_of(tile: TileIndex) -> &'static str {
        if tile.z == SGL_ZOOM { io::STREETS_GL } else { io::OPENFREEMAP }
    }

    /// Whether `drawn` covers every place of `selected` once at most: no drawn tile lies in
    /// another, and every selected tile lies in a drawn one or is covered whole by drawn finer
    /// ones.
    fn partition(drawn: &[TileIndex], selected: &[TileIndex]) -> Result<(), String> {
        for (i, &a) in drawn.iter().enumerate() {
            if let Some(&b) = drawn[i + 1..].iter().find(|&&b| a.contains(b) || b.contains(a)) {
                return Err(format!("{a:?} and {b:?} are both drawn"));
            }
        }
        fn covered(tile: TileIndex, drawn: &[TileIndex]) -> bool {
            if drawn.iter().any(|d| d.contains(tile)) {
                return true;
            }
            tile.z < 20 && drawn.iter().any(|&d| tile.contains(d)) && tile.children().iter().all(|&c| covered(c, drawn))
        }
        match selected.iter().find(|&&s| !covered(s, drawn)) {
            Some(gap) => Err(format!("nothing drawn over {gap:?}")),
            None => Ok(()),
        }
    }

    /// The tile of `drawn` holding the point `[lon, lat]`.
    fn tile_at(drawn: &[TileIndex], lng_lat: [f64; 2]) -> Option<TileIndex> {
        let [x, y] = lng_lat_to_world(lng_lat);
        drawn.iter().copied().find(|t| {
            let n = 2f64.powi(t.z as i32);
            let (tx, ty) = (x / 512.0 * n, (512.0 - y) / 512.0 * n);
            tx.floor() == f64::from(t.x) && ty.floor() == f64::from(t.y)
        })
    }

    /// What the overlay layer saw each frame.
    #[derive(Debug, Default, Clone)]
    struct Seen {
        drawn: Vec<TileIndex>,
        selected: Vec<TileIndex>,
        /// Selected tiles still on their way.
        loading: Vec<TileIndex>,
    }

    /// A layer of its own for the tests: the city ground's tiles (shown on the flat map whatever
    /// the scene says) drawn as [`TileOverlay`]'s squares, noting what it drew.
    struct Overlay {
        city: GroundCity,
        overlay: TileOverlay,
        seen: Arc<Mutex<Seen>>,
    }

    impl Overlay {
        fn new(renderer: &MapRenderer, gpu: &crate::gpu::Gpu) -> (Self, Arc<Mutex<Seen>>) {
            let init = renderer.init(gpu);
            let shader = GroundShader::new(&init, "test ground");
            let seen = Arc::new(Mutex::new(Seen::default()));
            (Overlay { city: GroundCity::new(&init, &shader), overlay: TileOverlay::new(&init), seen: seen.clone() }, seen)
        }
    }

    impl Layer for Overlay {
        fn name(&self) -> &'static str {
            "city ground tile overlay"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.city.prepare(cx, !cx.view.is_globe());
            self.overlay.prepare(cx, &self.city);
            let tiles = self.city.tiles();
            let selected = tiles.selected().to_vec();
            let loading = selected.iter().copied().filter(|&t| matches!(tiles.state(t), Some(s) if !matches!(s, crate::tiles::TileState::Loaded | crate::tiles::TileState::Empty | crate::tiles::TileState::Failed))).collect();
            *self.seen.lock().unwrap() = Seen { drawn: self.city.drawn().collect(), selected, loading };
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            if cx.pass == Pass::Map {
                self.overlay.draw(pass, cx, &self.city);
            }
        }

        fn pending(&self) -> bool {
            self.city.pending()
        }
    }

    #[test]
    fn the_lod_and_the_sources_are_today_s() {
        let lod = ground_lod();
        assert_eq!(lod.levels, [9, 10, 11, 12, 13, 14, 16]);
        assert_eq!((lod.bias, lod.max_distance), (0.5, 80_000.0));
        let sources = sources();
        let serving = |z: u32| sources.iter().filter(|s| s.zooms.as_ref().is_some_and(|zooms| zooms.contains(&z))).map(|s| &*s.name).collect::<Vec<_>>();
        for z in 9..=14 {
            assert_eq!(serving(z), [io::OPENFREEMAP], "zoom {z}");
        }
        assert_eq!(serving(16), [io::STREETS_GL]);
        assert!(serving(15).is_empty() && serving(8).is_empty());
        assert_eq!(line_width(TileIndex::new(36057, 19273, 16)), LineWidth::Exact);
        assert_eq!(line_width(TileIndex::new(9014, 4818, 14)), LineWidth::Hairline);
        assert_eq!(plain_style(LineWidth::Hairline).width_min_pixels, 0.6);
        assert_eq!(plain_style(LineWidth::Exact).width_min_pixels, 0.0);
        assert!(DECKS.depth_write && DECKS.cull.is_none() && DECKS.stencil == CLIPPED.stencil);
    }

    /// A tile decodes by its zoom's source: OpenMapTiles' layers below 16, Streets GL's at 16, the
    /// flat map's way (a Streets GL tile has its fields, gardens and roads; a bridge raised).
    #[test]
    fn a_tile_decodes_from_its_zoom_s_source() {
        let fixtures = DirTiles::fixtures();
        let omt = TileIndex::new(9014, 4818, 14);
        let bytes = fixtures.read(io::OPENFREEMAP, omt).expect("read").expect("a V5 tile");
        let decoded = decode(omt, &bytes, &Cancel::new()).expect("decoded").expect("ground");
        assert!(!decoded.0.indices.is_empty() && !decoded.0.lines.is_empty());
        assert_eq!(decoded.0, tess::tessellate(omt, &bytes, &GROUND_LAYERS, true, &|| false).expect("tessellated"));
        let sgl = fixtures.tiles(io::STREETS_GL).into_iter().find(|&t| matches!(fixtures.held(io::STREETS_GL, t), io::Held::Tile(_))).expect("a Streets GL tile");
        let bytes = fixtures.read(io::STREETS_GL, sgl).expect("read").expect("a tile");
        let decoded = decode(sgl, &bytes, &Cancel::new()).expect("decoded").expect("ground");
        assert_eq!(decoded.0, tess::tessellate(sgl, &bytes, &SGL_GROUND_LAYERS, true, &|| false).expect("tessellated"));
        // (OpenMapTiles' layers are not in a Streets GL tile)
        assert!(matches!(decode(TileIndex::new(sgl.x / 4, sgl.y / 4, 14), &bytes, &Cancel::new()), Ok(None)));
        assert!(matches!(decode(omt, &[], &Cancel::new()), Ok(None)));
        let cancel = Cancel::new();
        cancel.cancel();
        assert!(decode(sgl, &bytes, &cancel).is_err());
    }

    /// The overlay's colour channel of a pixel covered `n` times by squares of value `v` over the
    /// map's clear colour.
    fn layered(v: u8, n: u32) -> f64 {
        let a = f64::from(TileOverlay::ALPHA) / 255.0;
        (0..n).fold(f64::from(CLEAR_RGB[0]), |c, _| f64::from(v) * a + c * (1.0 - a))
    }

    /// A fetcher with a tile everywhere: every OpenMapTiles request gets one fixture tile's bytes,
    /// every Streets GL request another's (what is in them does not matter to the overlay; each
    /// decodes to something). Streets GL's answers can be held back.
    struct Everywhere {
        omt: Vec<u8>,
        sgl: Vec<u8>,
        /// While `Some`, Streets GL's requests wait here.
        held: Mutex<Option<Vec<Sender<FetchResult>>>>,
    }

    impl Everywhere {
        fn new(hold: bool) -> Arc<Self> {
            let fixtures = DirTiles::fixtures();
            let bytes = |source: &str, tile: TileIndex| fixtures.read(source, tile).expect("read").expect("a fixture tile");
            Arc::new(Everywhere {
                omt: bytes(io::OPENFREEMAP, TileIndex::new(9014, 4818, 14)),
                sgl: bytes(io::STREETS_GL, TileIndex::new(36057, 19273, 16)),
                held: Mutex::new(hold.then(Vec::new)),
            })
        }

        fn hold(&self) {
            self.held.lock().unwrap().get_or_insert_with(Vec::new);
        }

        /// Answers every Streets GL request held, and holds no more.
        fn release(&self) {
            for tx in self.held.lock().unwrap().take().unwrap_or_default() {
                let _ = tx.send(Ok(Some(self.sgl.clone())));
            }
        }
    }

    impl TileFetcher for Everywhere {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            let (tx, rx) = mpsc::channel();
            if &*request.source == io::STREETS_GL {
                if let Some(waiting) = self.held.lock().unwrap().as_mut() {
                    waiting.push(tx);
                    return rx;
                }
                let _ = tx.send(Ok(Some(self.sgl.clone())));
            } else {
                assert_eq!(&*request.source, io::OPENFREEMAP);
                let _ = tx.send(Ok(Some(self.omt.clone())));
            }
            rx
        }
    }

    /// A renderer drawing nothing but the city ground's [`TileOverlay`] (over the map's clear
    /// colour), its tiles from `fetcher`.
    fn overlay_renderer(gpu: &crate::gpu::Gpu, fetcher: Arc<dyn TileFetcher>) -> (MapRenderer, Arc<Mutex<Seen>>) {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        renderer.set_tiles(fetcher, None);
        let (overlay, seen) = Overlay::new(&renderer, gpu);
        renderer.add_layer(Box::new(overlay));
        (renderer, seen)
    }

    /// The realistic map's scene of `view` with nothing shown (the overlay draws on its own).
    fn bare(view: ViewState) -> SceneInput {
        let mut scene = SceneInput::new(view, SIZE, 1.0, Basemap::Realistic);
        scene.local_now_ms = MIDSUMMER;
        scene
    }

    /// The overlay's checks on `image` of `view`: every pixel whose point on the ground lies in a
    /// selected tile is covered exactly once, every other pixel not at all (none twice anywhere),
    /// and each pixel's colour is that of the drawn tile the CPU finds under it. Rows above
    /// `ground_from` (the horizon's neighbourhood) are only checked not to be covered twice. The
    /// zoom drawn at each pixel of the middle column, bottom to top.
    fn check_overlay(image: &Image, view: &ViewState, seen: &Seen, ground_from: u32) -> Vec<u32> {
        let Seen { drawn, selected, .. } = seen;
        partition(drawn, selected).expect("the tiles drawn cover the selection once");
        let frame = ViewFrame::new(view, SIZE, 1.0, 0.0);
        let point = |x: f64, y: f64| frame.viewport.unproject([x, y], 0.0);
        let (once, twice) = (layered(255, 1), layered(255, 2));
        let (mut gaps, mut doubles, mut strays, mut checked, mut wrong) = (0, 0, 0, 0, Vec::new());
        for y in 0..image.height {
            for x in 0..image.width {
                let p = image.pixel(x, y);
                let r = f64::from(p[0]);
                if (r - twice).abs() < (r - once).abs() {
                    doubles += 1;
                }
                if y < ground_from || x % 3 != 0 || y % 3 != 0 {
                    continue;
                }
                let (fx, fy) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                let in_selection = tile_at(selected, point(fx, fy)).is_some();
                // (not within a pixel and a half of a tile's edge: a seam blends two squares)
                let near_edge = [(-1.5, 0.0), (1.5, 0.0), (0.0, -1.5), (0.0, 1.5)].iter().any(|&(dx, dy)| tile_at(drawn, point(fx + dx, fy + dy)) != tile_at(drawn, point(fx, fy)));
                let covered = (r - once).abs() <= 8.0;
                if in_selection && !covered && !near_edge {
                    gaps += 1;
                }
                if !in_selection && (r - f64::from(CLEAR_RGB[0])).abs() > 8.0 && !near_edge {
                    strays += 1;
                }
                if let Some(tile) = tile_at(drawn, point(fx, fy))
                    && !near_edge
                {
                    checked += 1;
                    let want = TileOverlay::colour(tile);
                    if (1..3).any(|c| (f64::from(p[c]) - layered(want[c], 1)).abs() > 4.0) {
                        wrong.push(([x, y], tile, p));
                    }
                }
            }
        }
        eprintln!(
            "{} tiles drawn; {checked} pixels checked: {gaps} uncovered in the selection, {strays} covered outside it, {doubles} covered twice, {} of another tile's colour {:?}",
            drawn.len(),
            wrong.len(),
            &wrong[..wrong.len().min(4)]
        );
        assert!(checked > 10_000);
        assert_eq!((gaps, strays, doubles), (0, 0, 0));
        assert!(wrong.is_empty());
        (ground_from..image.height).rev().step_by(10).filter_map(|y| tile_at(drawn, point(720.5, f64::from(y) + 0.5)).map(|t| t.z)).collect()
    }

    /// The zooms drawn, with how many tiles each.
    fn by_zoom(drawn: &[TileIndex]) -> BTreeMap<u32, usize> {
        let mut zooms = BTreeMap::new();
        for t in drawn {
            *zooms.entry(t.z).or_default() += 1;
        }
        zooms
    }

    /// R3's acceptance, by the tile-id overlay, at V5 from the fixtures: each tile the city ground
    /// draws is a half see-through square in its own colour. Every pixel is covered exactly once
    /// (no gap, no place drawn twice) by the tile the CPU finds on the ground under it, every
    /// tile the selection asked for is in, and none is missing from the fixtures. At V5 every
    /// tile is near enough for Streets GL's zoom 16 (the farthest ground in view, at the top, is
    /// 3.7 view heights from the camera: the LOD wants zoom 15.5 and up there, and 16 is the next
    /// level), as in today's app; [`tilted_to_75_near_tiles_are_zoom_16_and_far_ones_coarser`]
    /// sees the coarser levels further off.
    #[test]
    fn v5_from_the_fixtures_is_streets_gl_s_zoom_16_drawn_once_over() {
        let Some(gpu) = gpu() else { return };
        let fixtures = Arc::new(DirTiles::fixtures());
        let (mut renderer, seen) = overlay_renderer(&gpu, fixtures.clone());
        let image = render_with(&gpu, &mut renderer, &bare(V5), 1.0).expect("rendered");
        image.save_png(&shots_dir().join("r3-city-overlay-v5.png")).expect("png");
        assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
        assert!(fixtures.misses().is_empty(), "tiles the fixtures lack: {:?}", fixtures.misses());
        let seen = seen.lock().unwrap().clone();
        eprintln!("V5: tiles drawn by zoom {:?}", by_zoom(&seen.drawn));
        assert_eq!(seen.drawn.iter().copied().collect::<HashSet<_>>(), seen.selected.iter().copied().collect::<HashSet<_>>(), "every selected tile is in");
        assert!(seen.drawn.iter().all(|t| t.z == SGL_ZOOM), "{:?}", by_zoom(&seen.drawn));
        let zooms = check_overlay(&image, &V5, &seen, 0);
        assert!(zooms.iter().all(|&z| z == SGL_ZOOM));
    }

    /// The tile-id overlay at V5 tilted to the realistic map's 75° (the horizon in view): near
    /// tiles are Streets GL's zoom 16, and further off OpenMapTiles' 14 and 13 (up to deck's far
    /// plane, 15 view heights out), each
    /// level further up the screen no finer than the one below it; every pixel of the ground in
    /// the selection is covered once by the tile under it, the far ground past it (and the sky)
    /// by none. (Every tile from [`Everywhere`]: the fixtures hold V5's tiles only.)
    #[test]
    fn tilted_to_75_near_tiles_are_zoom_16_and_far_ones_coarser() {
        let Some(gpu) = gpu() else { return };
        let (mut renderer, seen) = overlay_renderer(&gpu, Everywhere::new(false));
        let view = ViewState { pitch: 75.0, ..V5 };
        let image = render_with(&gpu, &mut renderer, &bare(view), 1.0).expect("rendered");
        image.save_png(&shots_dir().join("r3-city-overlay-v5-75.png")).expect("png");
        assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
        let seen = seen.lock().unwrap().clone();
        let zooms = by_zoom(&seen.drawn);
        eprintln!("V5 at 75°: tiles drawn by zoom {zooms:?}");
        assert_eq!(seen.drawn.iter().copied().collect::<HashSet<_>>(), seen.selected.iter().copied().collect::<HashSet<_>>());
        // (deck's far plane is 10 camera heights out: nothing past 15 view heights, where the LOD
        // wants 13)
        assert!(zooms.contains_key(&SGL_ZOOM) && zooms.contains_key(&14) && zooms.contains_key(&13), "{zooms:?}");
        assert!(!zooms.contains_key(&15) && zooms.keys().all(|&z| z >= 13));
        // the horizon is 0.098 of the height down at 75° (fovy 36.87°): the ground from 0.15
        let column = check_overlay(&image, &view, &seen, 135);
        eprintln!("zooms up the middle: {column:?}");
        assert_eq!(column[0], SGL_ZOOM);
        assert!(column.windows(2).all(|w| w[1] <= w[0]), "{column:?}");
        assert!(*column.last().expect("rows") <= 13);
    }

    /// Zooming in (§13.4): while Streets GL's zoom 16 tiles are on their way, the coarser tiles
    /// already in stand in for them (no gap, nothing drawn twice); once they are in, they are drawn
    /// and the coarser ones over them are not.
    #[test]
    fn zooming_in_the_coarser_tiles_stand_in_until_the_finer_are_in() {
        let Some(gpu) = gpu() else { return };
        let fetcher = Everywhere::new(false);
        let (mut renderer, seen) = overlay_renderer(&gpu, fetcher.clone());
        // at zoom 13 the tiles in view are OpenMapTiles' 14 and coarser (even straight under the
        // camera, 0.75 view heights away, the LOD wants 14)
        let far = ViewState { zoom: 13.0, ..V5 };
        render_with(&gpu, &mut renderer, &bare(far), 1.0).expect("rendered");
        let before = seen.lock().unwrap().clone();
        eprintln!("at zoom 13: {:?}", by_zoom(&before.drawn));
        assert!(before.drawn.iter().all(|t| t.z <= 14) && before.drawn.iter().any(|t| t.z == 14));
        partition(&before.drawn, &before.selected).expect("covered once");
        // in to V5 with Streets GL holding its tiles back
        fetcher.hold();
        renderer.update(bare(V5));
        let mut stand_ins = 0;
        for frame in 0..30 {
            renderer.prepare(&gpu, MIDSUMMER);
            let now = seen.lock().unwrap().clone();
            let fine: Vec<TileIndex> = now.selected.iter().copied().filter(|t| t.z == SGL_ZOOM).collect();
            assert!(!fine.is_empty() && fine.iter().all(|t| now.loading.contains(t)), "zoom 16's tiles are held back");
            assert!(now.drawn.iter().all(|t| t.z <= 14), "a zoom-16 tile drawn while held: {:?}", by_zoom(&now.drawn));
            // each zoom-16 tile wanted has a coarser one drawn over its place, and nothing is twice
            partition(&now.drawn, &fine).unwrap_or_else(|e| panic!("frame {frame}: {e}"));
            stand_ins = now.drawn.iter().filter(|d| fine.iter().any(|&f| d.contains(f))).count();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        eprintln!("{stand_ins} coarser tiles stood in for zoom 16's while they were held");
        assert!(stand_ins > 0);
        // they come in: drawn, and nothing coarser over them
        fetcher.release();
        render_with(&gpu, &mut renderer, &bare(V5), 1.0).expect("rendered");
        let after = seen.lock().unwrap().clone();
        assert!(after.drawn.iter().all(|t| t.z == SGL_ZOOM), "{:?}", by_zoom(&after.drawn));
        partition(&after.drawn, &after.selected).expect("covered once");
        assert_eq!(after.drawn.len(), after.selected.len());
    }

    /// A pixel no area of the drawn tiles covers (the land under them), and one near a line.
    const LAND: u8 = 254;
    const NEAR_LINE: u8 = 255;

    /// Which surface the city ground shows at each pixel of `view`, worked out on the CPU from the
    /// fixture tiles `drawn`: each pixel belongs to the drawn tile its point on the ground lies in
    /// (the tile clip), where the last of that tile's triangles over the pixel's centre gives the
    /// surface ([`LAND`] where none does), and [`NEAR_LINE`] within a pixel and a half of a line
    /// (its edging and itself, bridges too, as wide as they are drawn there).
    struct SurfaceMap {
        width: usize,
        height: usize,
        surfaces: Vec<u8>,
    }

    impl SurfaceMap {
        fn new(view: &ViewFrame, drawn: &[TileIndex], [width, height]: [usize; 2]) -> Self {
            let fixtures = DirTiles::fixtures();
            let owner: Vec<Option<usize>> = (0..width * height)
                .map(|i| {
                    let lng_lat = view.viewport.unproject([(i % width) as f64 + 0.5, (i / width) as f64 + 0.5], 0.0);
                    tile_at(drawn, lng_lat).and_then(|t| drawn.iter().position(|&d| d == t))
                })
                .collect();
            let mut surfaces = vec![LAND; width * height];
            for (k, &tile) in drawn.iter().enumerate() {
                let bytes = fixtures.read(source_of(tile), tile).expect("read").expect("a fixture tile");
                let Some(Decoded(ground)) = decode(tile, &bytes, &Cancel::new()).expect("decoded") else { continue };
                let local = LocalMercator::tile(tile);
                let world = |u: f64, v: f64| local.to_world([u, v]).to_array();
                let screen = |w: [f64; 2]| {
                    let [x, y, _] = view.viewport.project([world_to_lng_lat(w)[0], world_to_lng_lat(w)[1], 0.0]);
                    [x, y]
                };
                let mine = |x: usize, y: usize| owner[y * width + x] == Some(k);
                let points: Vec<[f64; 2]> = ground.vertices.iter().map(|v| screen(world(f64::from(v.position[0]), f64::from(v.position[1])))).collect();
                for tri in ground.indices.chunks_exact(3) {
                    let [a, b, c] = [0, 1, 2].map(|i| points[tri[i] as usize]);
                    let surface = ground.vertices[tri[0] as usize].surface;
                    let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                    if area == 0.0 || !area.is_finite() {
                        continue;
                    }
                    let edge = |p: [f64; 2], q: [f64; 2], x: f64, y: f64| ((q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0])) * area.signum();
                    let (x0, x1) = (a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize, (a[0].max(b[0]).max(c[0]).ceil().max(0.0) as usize).min(width));
                    let (y0, y1) = (a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize, (a[1].max(b[1]).max(c[1]).ceil().max(0.0) as usize).min(height));
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                            if mine(x, y) && edge(a, b, px, py) >= 0.0 && edge(b, c, px, py) >= 0.0 && edge(c, a, px, py) >= 0.0 {
                                surfaces[y * width + x] = surface;
                            }
                        }
                    }
                }
                // the lines: half their width on the screen at each end (metres along the map's
                // axes, the larger), a pixel and a half more
                let metres = 40_075_016.686 * f64::to_radians(world_to_lng_lat(world(0.5, 0.5))[1]).cos() / 2f64.powi(tile.z as i32);
                for seg in ground.line_passes().iter().flat_map(|(_, s)| s.iter()) {
                    let half_units = f64::from(seg.width) / 2.0 / metres;
                    let ends = [seg.start, seg.end].map(|p| {
                        let (u, v) = (f64::from(p[0]), f64::from(p[1]));
                        let at = screen(world(u, v));
                        let across = [screen(world(u + half_units, v)), screen(world(u, v + half_units))]
                            .iter()
                            .map(|q| (q[0] - at[0]).hypot(q[1] - at[1]))
                            .fold(0.0f64, f64::max);
                        (at, across)
                    });
                    let ([a, half_a], [b, half_b]) = ([ends[0].0, [ends[0].1, 0.0]], [ends[1].0, [ends[1].1, 0.0]]);
                    let half = half_a[0].max(half_b[0]).max(0.3) + 1.5;
                    let (x0, x1) = ((a[0].min(b[0]) - half).floor().max(0.0) as usize, ((a[0].max(b[0]) + half).ceil().max(0.0) as usize).min(width));
                    let (y0, y1) = ((a[1].min(b[1]) - half).floor().max(0.0) as usize, ((a[1].max(b[1]) + half).ceil().max(0.0) as usize).min(height));
                    let d = [b[0] - a[0], b[1] - a[1]];
                    let length2 = d[0] * d[0] + d[1] * d[1];
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let q = [x as f64 + 0.5 - a[0], y as f64 + 0.5 - a[1]];
                            let t = if length2 > 0.0 { ((q[0] * d[0] + q[1] * d[1]) / length2).clamp(0.0, 1.0) } else { 0.0 };
                            if (q[0] - t * d[0]).hypot(q[1] - t * d[1]) <= half && mine(x, y) {
                                surfaces[y * width + x] = NEAR_LINE;
                            }
                        }
                    }
                }
            }
            SurfaceMap { width, height, surfaces }
        }

        /// The surface at `(x, y)` when every pixel within 3 px has it too.
        fn inner(&self, x: usize, y: usize) -> Option<u8> {
            if x < 3 || y < 3 || x + 3 >= self.width || y + 3 >= self.height {
                return None;
            }
            let s = self.surfaces[y * self.width + x];
            let same = (y - 3..=y + 3).all(|y| (x - 3..=x + 3).all(|x| self.surfaces[y * self.width + x] == s));
            (same && s != NEAR_LINE).then_some(s)
        }
    }

    /// The per-channel median of `colours`.
    fn median(colours: &mut [[u8; 3]]) -> [u8; 3] {
        std::array::from_fn(|i| {
            colours.sort_by_key(|c| c[i]);
            colours[colours.len() / 2][i]
        })
    }

    /// V5 on the realistic map (today's reference clock, the sunny sky), the city ground from
    /// the fixtures: Streets GL's water, gardens, pavements and roads close in. What today's
    /// picture shows of them between its buildings, trees and shadows (R4–R7) is in the same hues:
    /// the water, the gardens and the paving each within 6 levels of `realistic-v5-1440x900.png`'s
    /// median at the same pixels (the classes worked out on the CPU from the same tiles); the
    /// water apart from the rest.
    #[test]
    fn v5_shows_streets_gl_s_ground_in_today_s_hues() {
        let Some(gpu) = gpu() else { return };
        let fixtures = Arc::new(DirTiles::fixtures());
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(fixtures.clone(), None);
        let mut scene = SceneInput::new(V5, SIZE, 1.0, Basemap::Realistic);
        scene.local_now_ms = MIDSUMMER;
        scene.earth = earth::Input::SHOWN;
        scene.sky = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
        scene.ground = ground::Input::SHOWN;
        let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        image.save_png(&shots_dir().join("r3-city-v5.png")).expect("png");
        assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
        assert!(fixtures.misses().is_empty(), "tiles the fixtures lack: {:?}", fixtures.misses());
        let view = ViewFrame::new(&V5, SIZE, 1.0, 0.0);
        let drawn: Vec<TileIndex> = {
            let mut selected = rustradar_scene::lod::distance_tile_indices(&view.viewport, &ground_lod(), None, 0.0);
            selected.sort_by_key(|t| t.z);
            selected
        };
        let map = SurfaceMap::new(&view, &drawn, [1440, 900]);
        let today = reference("realistic-v5-1440x900.png");
        // (today's pictures stand about 4.5 px higher than deck's own viewport, T1)
        let mut classes: BTreeMap<u8, [Vec<[u8; 3]>; 2]> = BTreeMap::new();
        // the map in today's picture: right of the sidebar, left of the map controls, under the
        // mode bar, above the credits
        for y in (76..880).step_by(2) {
            for x in (340..1380).step_by(2) {
                if let Some(s) = map.inner(x as usize, y as usize) {
                    let (mine, theirs) = (image.pixel(x, y), today.pixel(x, y - 4));
                    let class = classes.entry(s).or_default();
                    class[0].push([mine[0], mine[1], mine[2]]);
                    class[1].push([theirs[0], theirs[1], theirs[2]]);
                }
            }
        }
        let mut medians = BTreeMap::new();
        for (&surface, [mine, theirs]) in &mut classes {
            if mine.len() < 100 {
                continue;
            }
            let (m, t) = (median(mine), median(theirs));
            eprintln!("surface {surface:3}: {} pixels, ours {m:?}, today's {t:?}", mine.len());
            medians.insert(surface, (m, t));
        }
        // what lies open between today's buildings: the water, the gardens and the pavements
        for surface in [WATER_ID, 5, 9] {
            let (ours, today) = *medians.get(&surface).unwrap_or_else(|| panic!("no surface {surface} in {:?}", medians.keys()));
            assert!((0..3).all(|i| ours[i].abs_diff(today[i]) <= 6), "surface {surface}: {ours:?} against today's {today:?}");
        }
        // the water apart from the land, the gardens, the soil and the paving
        let distance = |a: [u8; 3], b: [u8; 3]| (0..3).map(|i| (f64::from(a[i]) - f64::from(b[i])).powi(2)).sum::<f64>().sqrt();
        for surface in [LAND, 5, 8, 9] {
            if let Some(&(colour, _)) = medians.get(&surface) {
                assert!(distance(medians[&WATER_ID].0, colour) > 24.0, "the water and surface {surface} alike");
            }
        }
    }
}
