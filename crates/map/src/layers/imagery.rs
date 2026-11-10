//! Satellite imagery (item B6; doc 01 §5.2, §5.8; row 3a of doc 03 §3): Esri World Imagery under
//! everything else on the satellite basemap, as today's `imageryLayer` (G:466-490) draws it with
//! deck.gl 9.4's TileLayer and a BitmapLayer per tile.
//!
//! What today's app does, and so this layer:
//! - **When.** On the satellite basemap, always (G:2649-2650: `if (basemap === "satellite")
//!   layers.push(imageryLayer())`, no setting, no tile URL to wait for). The layer works that out
//!   from [`SceneInput::basemap`](crate::renderer::SceneInput::basemap) itself, so the scene builder
//!   has nothing to hand it ([`Input`] is empty). Over the earth sphere, which stays
//!   `[6,16,34]` (`layers::earth`), under every other layer; there is no land rectangle, no vector
//!   map, no runways, roads or borders on this basemap.
//! - **Tiles.** `…/World_Imagery/MapServer/tile/{z}/{y}/{x}` (the app's fetcher maps
//!   [`io::ESRI`] to it, `tile_fetcher::TileSources::app`), `tileSize: 256`, `minZoom: 0`,
//!   `maxZoom: 19`: deck's rule for 256-px tiles, `round(zoom + 1)` clamped to 0–19, stretched
//!   past 19 (`scene::tiles::IMAGERY_TILES`); `maxRequests: 12` ([`MAX_REQUESTS`]); TileLayer's
//!   defaults otherwise: `best-available` refinement (a loaded ancestor or descendants stand in
//!   while a tile loads) and a cache of five times the selection. One tile set serves both
//!   projections, as deck's one `imagery` layer keeps one cache across the swap at zoom 12 (and
//!   both select `round(zoom + 1)`: crossing it asks for no new tile, test
//!   `crossing_zoom_12_asks_for_no_new_tile`).
//! - **Decoded off the drawing thread.** loaders.gl's ImageLoader turns a tile's JPEG into an
//!   `ImageBitmap` in the browser; here [`decode`] turns it into RGBA8 on the worker pool (the
//!   `image` crate), and the upload makes an `Rgba8Unorm` texture with the full mip chain deck's
//!   `createTexture` generates for every image (`getMipLevelCount`, `generateMipmapsWebGL`),
//!   sampled with deck's defaults (linear, linear between mip levels, clamped to the edge): the
//!   [`Bitmap`] pipeline's.
//! - **Each tile a BitmapLayer** over the tile's bounding box with `_imageCoordinateSystem:
//!   CARTESIAN`, `tintColor: [150,158,180]` ([`TINT`]: the image's RGB times `(150, 158, 180) /
//!   255`, darkened with a blue cast so the traffic stands out), opaque, `parameters: SURFACE`
//!   (back faces culled, depth-tested against the earth sphere so the far side is hidden, no depth
//!   written). On the flat map deck draws the box as one quad with linear texture coordinates; on
//!   the globe (`resolution: 5`) as a grid of at most 5° whose fragments turn their longitude and
//!   latitude into Mercator to sample the image, so the picture follows Web Mercator inside the
//!   tile. Here each tile is `pipelines::bitmap::tile_mesh` in its tile frame
//!   ([`PrepareCx::tile_frame`]): a grid in local Mercator, at least deck's 5° and fine enough
//!   (`tile_cells`) that interpolating Mercator across a cell stays within 0.25 px of deck's
//!   per-fragment Mercator at the deepest zoom that selects the tile (E5's pipeline; one cell from
//!   z11 on, so one quad on the flat map, as deck's).
//! - **Order.** Tiles draw in deck's order: the cache's creation order sorted by zoom, so finer
//!   tiles cover the coarser ones standing in for them ([`TileSet::visible`]). TileLayer's sublayers
//!   are not clipped (only MVTLayer clips), and image tiles carry no buffer to clip.
//!
//! When the layer stops drawing (another basemap) it drops its tiles, as deck finalizes a layer
//! that leaves the list; it is [`Layer::pending`] only while it draws ([`TileSet::loading`]).
//!
//! Checked against today's pictures (`docs/port/reference/satellite-*.png`, tests below): V2 and
//! V7 on the globe and V6 on the flat map match colour for colour where no label, building or UI
//! covers them (means within 1 level), V6 to the pixel and the globe's views within 1–2 px. (So
//! the imagery does not show the 4.5 px offset T1 measured on the dark map's vector tiles.) Far
//! north, as at V7, deck's selection on the globe leaves the top row of tiles out and the earth
//! sphere shows there, in today's picture as here.
//!
//! Deviations, invisible or nearly:
//! - The picture's colours come from the `image` crate's JPEG decoder, not the browser's: the two
//!   may round a level or two apart here and there.
//! - On the globe the texture is interpolated in Mercator across a cell of the grid above rather
//!   than per fragment from interpolated degrees (within 0.25 px where the tile is selected; a
//!   coarser tile standing in for a finer one while it loads may be a little further off).
//! - A tile the server has nothing for (404), or whose image cannot be read, does not stand in for
//!   its ancestors: the coarser picture stays drawn there (the tile runtime's rule; deck 9.4 counts
//!   such a tile loaded and leaves a hole).
//! - Terrain (stage 9): deck drapes the imagery over the relief (`TerrainExtension`, doc 01 §5.3);
//!   this layer draws only in the map pass and stays [`Drape::None`](super::Drape::None) until the
//!   terrain items give it a cover pass.

use rustradar_core::geo::Basemap;
use rustradar_scene::tiles::IMAGERY_TILES;

use super::{ByteSize, DeckSelect, DrawCx, Init, Layer, Pass, PrepareCx, Source, TileIndex, TileSet, TileSetOptions};
use crate::frame::FrameSlot;
use crate::gpu::Gpu;
use crate::io;
use crate::jobs::Cancel;
use crate::pipelines::bitmap::{self, Bitmap, Params, Vertex, tile_cells, tile_mesh, upload_rgba8};
use crate::pipelines::fill::Mesh;
use crate::state::DrawState;

/// The imagery TileLayer's `maxRequests` (G:487).
pub const MAX_REQUESTS: usize = 12;

/// The imagery's `tintColor` (G:476): darkened, with a slight blue cast.
pub const TINT: [u8; 3] = [150, 158, 180];

/// Esri's tile size in CSS pixels (`tileSize: 256`).
const TILE_SIZE: f64 = 256.0;

/// The deepest tile zoom the imagery asks for (`maxZoom: 19`; deeper views stretch these).
const MAX_TILE_ZOOM: u32 = 19;

/// What the scene builder hands this layer (`SceneInput::imagery`): nothing. The layer draws on
/// the satellite basemap (`SceneInput::basemap`), as today's app always does there.
#[derive(Debug, Clone, Default)]
pub struct Input {}

/// A tile's picture, decoded on the worker pool: RGBA8, rows top first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub size: [u32; 2],
    pub rgba: Vec<u8>,
}

impl ByteSize for Picture {
    fn byte_size(&self) -> usize {
        self.rgba.len()
    }
}

/// The decode step, on the worker pool: a tile's image (Esri sends JPEG; PNG and WebP read too)
/// as RGBA8. `None` for no bytes; an error for bytes that are no image, or once cancelled.
pub fn decode(_tile: TileIndex, bytes: &[u8], cancel: &Cancel) -> Result<Option<Picture>, String> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if cancel.is_cancelled() {
        return Err("cancelled".into());
    }
    let image = image::load_from_memory(bytes).map_err(|e| format!("imagery tile: {e}"))?.into_rgba8();
    if cancel.is_cancelled() {
        return Err("cancelled".into());
    }
    Ok(Some(Picture { size: [image.width(), image.height()], rgba: image.into_raw() }))
}

/// A tile's picture on the GPU: the texture with its mip chain, bound with the pipeline's sampler.
pub struct TileTexture {
    pub bound: bitmap::Texture,
    /// Kept with the bind group that samples it.
    pub texture: wgpu::Texture,
}

/// The upload step, on the main thread: the picture as a texture with deck's mip chain.
pub fn upload(gpu: &Gpu, bitmap: &Bitmap, picture: &Picture) -> TileTexture {
    let texture = upload_rgba8(gpu, "imagery tile", picture.size, &picture.rgba, true);
    let bound = bitmap.texture(&gpu.device, &texture.create_view(&Default::default()));
    TileTexture { bound, texture }
}

/// Row 3a: `imagery` (satellite basemap).
pub struct Imagery {
    tiles: TileSet<Picture, TileTexture>,
    /// `SURFACE`.
    bitmap: Bitmap,
    style: bitmap::Style,
    /// A tile's grid for each tile zoom 0–19 (`tile_mesh(tile_cells(z, 256))`), in tile units.
    meshes: Vec<Mesh<Vertex>>,
    /// Whether this frame draws (the satellite basemap).
    shown: bool,
    /// This frame's tiles in draw order, each with its frame.
    drawn: Vec<(FrameSlot, TileIndex)>,
}

impl Imagery {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let tiles = init.tile_set(
            vec![Source::new(io::ESRI)],
            DeckSelect(IMAGERY_TILES),
            TileSetOptions { max_requests: MAX_REQUESTS, ..Default::default() },
            |tile, bytes, cancel| decode(tile, &bytes, cancel),
        );
        let bitmap = Bitmap::new(init, "imagery", DrawState::SURFACE);
        let style = bitmap.style(device, &Params { tint: TINT, ..Params::default() });
        let meshes = (0..=MAX_TILE_ZOOM)
            .map(|z| {
                let (vertices, indices) = tile_mesh(tile_cells(z, TILE_SIZE));
                Mesh::new(device, "imagery tile", &vertices, &indices)
            })
            .collect();
        Imagery { tiles, bitmap, style, meshes, shown: false, drawn: Vec::new() }
    }

    /// The tile set (for tests).
    pub fn tiles(&self) -> &TileSet<Picture, TileTexture> {
        &self.tiles
    }
}

impl Layer for Imagery {
    fn name(&self) -> &'static str {
        "imagery"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.drawn.clear();
        self.shown = cx.scene.basemap == Basemap::Satellite;
        if !self.shown {
            // deck drops a layer that leaves the list, and its tiles with it
            if !self.tiles.is_empty() {
                self.tiles.clear();
            }
            return;
        }
        let bitmap = &self.bitmap;
        cx.update_tiles(&mut self.tiles, |gpu, _tile, picture| upload(gpu, bitmap, &picture));
        for (tile, _) in self.tiles.visible() {
            self.drawn.push((cx.tile_frame(tile, 0.0), tile));
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if cx.pass != Pass::Map || self.drawn.is_empty() {
            return;
        }
        for &(slot, tile) in &self.drawn {
            let Some(texture) = self.tiles.get(tile) else { continue };
            let mesh = &self.meshes[tile.z.min(MAX_TILE_ZOOM) as usize];
            self.bitmap.draw(pass, cx, slot, &texture.bound, &self.style, mesh);
        }
    }

    fn pending(&self) -> bool {
        self.shown && self.tiles.loading()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::Receiver;
    use std::sync::{Arc, Mutex};

    use rustradar_scene::camera::{ViewState, WORLD_SIZE, lng_lat_to_world};

    use super::*;
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
    fn v6() -> ViewState {
        view(57.70, 11.93, 13.0, 0.0, 0.0)
    }
    fn v7() -> ViewState {
        view(78.2, 15.6, 6.0, 0.0, 0.0)
    }

    /// A fetcher with every request noted.
    struct Noted<F> {
        tiles: F,
        asked: Mutex<Vec<(String, TileIndex)>>,
    }

    impl<F> Noted<F> {
        fn new(tiles: F) -> Arc<Self> {
            Arc::new(Noted { tiles, asked: Mutex::new(Vec::new()) })
        }

        fn asked(&self) -> Vec<(String, TileIndex)> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl<F: TileFetcher> TileFetcher for Noted<F> {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            self.asked.lock().unwrap().push((request.source.to_string(), request.tile));
            self.tiles.fetch(request)
        }
    }

    /// Every Esri tile one plain colour (a PNG of 4 × 4 pixels).
    struct Plain([u8; 3]);

    impl TileFetcher for Plain {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            if &*request.source != io::ESRI {
                return io::answered(Ok(None));
            }
            let image = image::RgbImage::from_pixel(4, 4, image::Rgb(self.0));
            let mut png = Vec::new();
            image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("a PNG");
            io::answered(Ok(Some(png)))
        }
    }

    /// The satellite map's scene of `view` on a `size` canvas: the earth under the imagery.
    fn satellite(view: ViewState, size: [f64; 2]) -> SceneInput {
        let mut scene = SceneInput::new(view, size, 1.0, Basemap::Satellite);
        scene.earth = earth::Input::SHOWN;
        scene
    }

    fn renderer(gpu: &Gpu, fetcher: Arc<dyn TileFetcher>) -> MapRenderer {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        renderer.set_tiles(fetcher, None);
        renderer
    }

    fn shots_dir() -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    fn reference(name: &str) -> Image {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference").join(name);
        let rgba = image::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).into_rgba8();
        Image { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() }
    }

    fn rgb(c: [u8; 4]) -> [u8; 3] {
        [c[0], c[1], c[2]]
    }

    /// A shot of the satellite map at `view` and 1440 × 900 from the fixtures, saved as `name`,
    /// with no tile missing, and the renderer that drew it.
    fn shot(gpu: &Gpu, view: ViewState, name: &str) -> (Image, MapRenderer) {
        let fixtures = Noted::new(DirTiles::fixtures());
        let mut renderer = renderer(gpu, fixtures.clone());
        let image = render_with(gpu, &mut renderer, &satellite(view, [1440.0, 900.0]), 1.0).expect("rendered");
        image.save_png(&shots_dir().join(name)).expect("the shot written");
        assert!(!renderer.pending(), "still loading: {:?}", renderer.pending_layers());
        assert!(fixtures.tiles.misses().is_empty(), "the fixtures lack {:?}", fixtures.tiles.misses());
        assert!(!fixtures.asked().is_empty() && fixtures.asked().iter().all(|(source, _)| source == io::ESRI), "{:?}", fixtures.asked());
        (image, renderer)
    }

    /// The fixture picture of `tile`, decoded.
    fn picture(tile: TileIndex) -> Picture {
        let bytes = DirTiles::fixtures().read(io::ESRI, tile).expect("read").unwrap_or_else(|| panic!("the fixtures hold {tile:?}"));
        decode(tile, &bytes, &Cancel::new()).expect("decoded").expect("a picture")
    }

    /// The raw texel of the tiles of zoom `z` under `[lon, lat]` (nearest), from the fixtures.
    fn raw_texel(pictures: &mut std::collections::HashMap<TileIndex, Picture>, z: u32, [lon, lat]: [f64; 2]) -> [u8; 3] {
        let [wx, wy] = lng_lat_to_world([lon, lat]);
        let n = f64::from(1u32 << z);
        let (tx, ty) = (wx / WORLD_SIZE * n, (WORLD_SIZE - wy) / WORLD_SIZE * n);
        let tile = TileIndex::new(tx.floor() as u32, ty.floor() as u32, z);
        let p = pictures.entry(tile).or_insert_with(|| picture(tile));
        let px = ((tx.fract() * f64::from(p.size[0])) as u32).min(p.size[0] - 1);
        let py = ((ty.fract() * f64::from(p.size[1])) as u32).min(p.size[1] - 1);
        let i = ((py * p.size[0] + px) * 4) as usize;
        [p.rgba[i], p.rgba[i + 1], p.rgba[i + 2]]
    }

    /// The map in today's pictures at 1440 × 900: right of the sidebar, left of the map controls,
    /// under the mode bar and the dev readout.
    const MAP: [u32; 4] = [360, 80, 1370, 880];

    fn diff(a: [u8; 3], b: [u8; 3]) -> u8 {
        (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0)
    }

    /// The share of today's pixels in [`MAP`] this shot matches within `tolerance` levels on every
    /// channel, at the best shift within 6 px (T1: today's pictures stand about 4.5 px higher), the
    /// shift, and the mean colours of the two (shot shifted).
    fn agreement(ours: &Image, today: &Image, tolerance: u8) -> (f64, [i32; 2], [f64; 3], [f64; 3]) {
        let [x0, y0, x1, y1] = MAP;
        let mut best = (-1.0, [0, 0]);
        for dy in -6..=6 {
            for dx in -6..=6 {
                let mut kept = 0usize;
                for y in (y0..y1).step_by(2) {
                    for x in (x0..x1).step_by(2) {
                        let mine = rgb(ours.pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32));
                        kept += usize::from(diff(mine, rgb(today.pixel(x, y))) <= tolerance);
                    }
                }
                let share = kept as f64 / f64::from(((x1 - x0) / 2) * ((y1 - y0) / 2));
                if share > best.0 {
                    best = (share, [dx, dy]);
                }
            }
        }
        let [dx, dy] = best.1;
        let (mut mine, mut theirs) = ([0.0; 3], [0.0; 3]);
        for y in y0..y1 {
            for x in x0..x1 {
                let m = rgb(ours.pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32));
                let t = rgb(today.pixel(x, y));
                for i in 0..3 {
                    mine[i] += f64::from(m[i]);
                    theirs[i] += f64::from(t[i]);
                }
            }
        }
        let n = f64::from((x1 - x0) * (y1 - y0));
        (best.0, best.1, mine.map(|c| c / n), theirs.map(|c| c / n))
    }

    /// The mean colour of `image` in `[x0, x1) × [y0, y1)`, the picture shifted by `[dx, dy]`.
    fn mean(image: &Image, [x0, y0, x1, y1]: [u32; 4], [dx, dy]: [i32; 2]) -> [f64; 3] {
        let mut sum = [0.0; 3];
        for y in y0..y1 {
            for x in x0..x1 {
                let p = image.pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32);
                for i in 0..3 {
                    sum[i] += f64::from(p[i]);
                }
            }
        }
        sum.map(|c| c / f64::from((x1 - x0) * (y1 - y0)))
    }

    fn within(a: [f64; 3], b: [f64; 3], levels: f64) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() <= levels)
    }

    #[test]
    fn a_fixture_tile_decodes_to_its_picture() {
        let tile = TileIndex::new(564, 300, 10);
        let bytes = DirTiles::fixtures().read(io::ESRI, tile).expect("read").expect("a V3 tile");
        let decoded = decode(tile, &bytes, &Cancel::new()).expect("decoded").expect("a picture");
        assert_eq!(decoded.size, [256, 256]);
        assert_eq!(decoded.rgba.len(), 256 * 256 * 4);
        assert_eq!(ByteSize::byte_size(&decoded), 256 * 256 * 4);
        assert!(decoded.rgba.chunks_exact(4).all(|p| p[3] == 255), "a JPEG is opaque");
        assert!(decoded.rgba.chunks_exact(4).any(|p| p[..3] != decoded.rgba[..3]), "not one colour");
        // nothing there: nothing to draw; not an image: an error
        assert_eq!(decode(tile, &[], &Cancel::new()), Ok(None));
        assert!(decode(tile, b"<html>not found</html>", &Cancel::new()).is_err());
        // cancelled: stopped
        let cancel = Cancel::new();
        cancel.cancel();
        assert!(decode(tile, &bytes, &cancel).is_err());
    }

    /// deck's TileLayer as the app sets it up: 256-px tiles to zoom 19, 12 requests at once,
    /// best-available.
    #[test]
    fn the_tile_set_is_the_imagery_tile_layer_s() {
        let Some(gpu) = gpu() else { return };
        let renderer = MapRenderer::new(&gpu, FORMAT);
        let init = renderer.init(&gpu);
        let imagery = Imagery::new(&init);
        let options = imagery.tiles().options();
        assert_eq!(options.max_requests, 12);
        assert!(matches!(options.refinement, crate::layers::Refinement::BestAvailable));
        assert_eq!((IMAGERY_TILES.tile_size, IMAGERY_TILES.max_zoom), (256.0, Some(19.0)));
        assert_eq!(imagery.meshes.len(), 20);
    }

    /// Item B6's acceptance: at V3 on satellite, the Baltic in the view's south-east corner (all
    /// water on the dark map of the same view) is blue-dominant, and its mean colour is the mean of
    /// the fixture tiles' texels there times `tintColor / 255`, within 3 levels.
    #[test]
    fn v3_sea_is_the_raw_picture_darkened_by_the_tint() {
        let Some(gpu) = gpu() else { return };
        let sea = [1300, 700, 1440, 900];
        // the dark map of the same view says it is sea
        let fixtures = Arc::new(DirTiles::fixtures());
        let mut dark = SceneInput::new(v3(), [1440.0, 900.0], 1.0, Basemap::Dark);
        dark.earth = earth::Input::SHOWN;
        dark.vector_map = crate::layers::vector_map::Input::SHOWN;
        let map = render_with(&gpu, &mut renderer(&gpu, fixtures.clone()), &dark, 1.0).expect("rendered");
        let [x0, y0, x1, y1] = sea;
        let water = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| rgb(map.pixel(x, y)) == [30, 30, 30]).count();
        assert!(water as f64 >= 0.999 * f64::from((x1 - x0) * (y1 - y0)), "{water} water pixels");

        let (image, renderer) = shot(&gpu, v3(), "b6-satellite-v3-1440x900.png");
        let drawn = mean(&image, sea, [0, 0]);
        // the texels under each pixel's centre, from the z10 tiles V3 selects (round(9 + 1))
        let mut pictures = std::collections::HashMap::new();
        let mut raw = [0.0; 3];
        for y in y0..y1 {
            for x in x0..x1 {
                let lng_lat = renderer.view().viewport.unproject([f64::from(x) + 0.5, f64::from(y) + 0.5], 0.0);
                let texel = raw_texel(&mut pictures, 10, lng_lat);
                for i in 0..3 {
                    raw[i] += f64::from(texel[i]);
                }
            }
        }
        let raw = raw.map(|c| c / f64::from((x1 - x0) * (y1 - y0)));
        let tinted: [f64; 3] = std::array::from_fn(|i| raw[i] * f64::from(TINT[i]) / 255.0);
        eprintln!("V3 sea: drawn {drawn:.2?}, raw {raw:.2?}, raw tinted {tinted:.2?}, from {} tiles", pictures.len());
        assert!(drawn[2] > drawn[0] && drawn[2] > drawn[1], "not blue-dominant: {drawn:?}");
        assert!(drawn.iter().zip(raw).all(|(d, r)| *d < r), "not darker: {drawn:?} against {raw:?}");
        assert!(within(drawn, tinted, 3.0), "{drawn:?} against the tinted texels {tinted:?}");
    }

    /// V2 on the globe at zoom 4.5 (z6 tiles) against today's picture: the same colours where no
    /// label covers it, and most pixels within 8 levels, about a pixel off (the reference stands
    /// ~1 px higher on the globe; the vector map's 4.5 px of T1 is not seen here).
    #[test]
    fn v2_looks_like_today_s_picture() {
        let Some(gpu) = gpu() else { return };
        let (image, _) = shot(&gpu, v2(), "b6-satellite-v2-1440x900.png");
        let today = reference("satellite-v2-1440x900.png");
        let (share, shift, ours, theirs) = agreement(&image, &today, 8);
        eprintln!("V2: {share:.4} of today's pixels within 8 levels at {shift:?}; means {ours:.2?} against {theirs:.2?}");
        assert!(share > 0.75, "{share}");
        assert!(shift[0].abs() <= 1 && shift[1].abs() <= 2, "{shift:?}");
        // land and sea where today's picture has no label
        for region in [[1150, 540, 1290, 590], [1000, 700, 1100, 760], [400, 150, 560, 300]] {
            let (a, b) = (mean(&image, region, shift), mean(&today, region, [0, 0]));
            eprintln!("V2 {region:?}: {a:.2?} against {b:.2?}");
            assert!(within(a, b, 2.0), "{region:?}: {a:?} against today's {b:?}");
        }
    }

    /// V6 on the flat map at zoom 13 (z14 tiles): lined up with today's picture to the pixel.
    /// (Today's picture has B7's buildings over the imagery, so fewer of its pixels match.)
    #[test]
    fn v6_lines_up_with_today_s_picture_on_the_flat_map() {
        let Some(gpu) = gpu() else { return };
        let (image, renderer) = shot(&gpu, v6(), "b6-satellite-v6-1440x900.png");
        assert!(!renderer.view().is_globe());
        let today = reference("satellite-v6-1440x900.png");
        let (share, shift, ours, theirs) = agreement(&image, &today, 8);
        eprintln!("V6: {share:.4} of today's pixels within 8 levels at {shift:?}; means {ours:.2?} against {theirs:.2?}");
        assert_eq!(shift, [0, 0]);
        assert!(share > 0.7, "{share}");
        // the river's water, west of the bridge and in the eastern harbour (no building, no label)
        for water in [[390, 600, 440, 630], [980, 200, 1030, 230]] {
            let (a, b) = (mean(&image, water, shift), mean(&today, water, [0, 0]));
            eprintln!("V6 water {water:?}: {a:.2?} against {b:.2?}");
            assert!(within(a, b, 2.0), "{water:?}: {a:?} against today's {b:?}");
        }
    }

    /// V7 over Svalbard at zoom 6 (z7 tiles): the imagery far north, and along the top edge the
    /// earth sphere where deck's selection on the globe leaves the tiles out, as in today's picture.
    #[test]
    fn v7_far_north_looks_like_today_s_picture() {
        let Some(gpu) = gpu() else { return };
        let (image, renderer) = shot(&gpu, v7(), "b6-satellite-v7-1440x900.png");
        let today = reference("satellite-v7-1440x900.png");
        let (share, shift, ours, theirs) = agreement(&image, &today, 8);
        eprintln!("V7: {share:.4} of today's pixels within 8 levels at {shift:?}; means {ours:.2?} against {theirs:.2?}");
        assert!(share > 0.7, "{share}");
        assert!(shift[0].abs() <= 1 && shift[1].abs() <= 2, "{shift:?}");
        assert!(within(ours, theirs, 2.0), "{ours:?} against today's {theirs:?}");
        // along the top edge deck's selection on the globe stops at the z7 tiles of row 16 (no
        // tile of row 15 is selected, nor fetched for the fixtures), and the earth sphere shows
        // above them, in today's picture as here: down each column, the picture starts within a
        // pixel of row 16's northern edge (79.17° N), and within two of where today's does
        let [_, edge] = rustradar_scene::camera::world_to_lng_lat([0.0, WORLD_SIZE - 16.0 * WORLD_SIZE / 128.0]);
        // (the first of three rows far from the sphere's colour at the top)
        let starts = |picture: &Image, x: u32| {
            let sphere = rgb(picture.pixel(x, 0));
            (0..60).find(|&y| (y..y + 3).all(|y| diff(rgb(picture.pixel(x, y)), sphere) > 20)).expect("the imagery")
        };
        for x in [400, 1000] {
            let lat = |y: u32| renderer.view().viewport.unproject([f64::from(x) + 0.5, f64::from(y) + 0.5], 0.0)[1];
            let row_16 = (0..60).find(|&y| lat(y) < edge).expect("row 16's edge on the picture");
            let (ours, theirs) = (starts(&image, x), starts(&today, x));
            eprintln!("V7 column {x}: {edge:.3}° N at row {row_16}, the picture from row {ours} here, {theirs} today");
            assert!(row_16 > 3, "column {x}: the edge at row {row_16}");
            assert!(ours.abs_diff(row_16) <= 1, "column {x}: the picture starts at row {ours}, {edge}° N at {row_16}");
            assert!(ours.abs_diff(theirs) <= 2, "column {x}: the picture starts at row {ours}, today's at {theirs}");
        }
    }

    /// V4 (Arlanda on the flat map, pitched 45° and turned 30°, z15 tiles): the imagery fills the
    /// whole picture, no tile missing.
    #[test]
    fn v4_fills_the_pitched_flat_map() {
        let Some(gpu) = gpu() else { return };
        let (image, renderer) = shot(&gpu, v4(), "b6-satellite-v4-1440x900.png");
        assert!(!renderer.view().is_globe());
        let ink = image.rgba.chunks_exact(4).filter(|p| p[..3] == [30, 30, 30]).count();
        assert!(ink < 10, "{ink} pixels of ink");
    }

    /// The imagery is asked for and drawn on the satellite basemap only, tinted as deck tints it;
    /// leaving the basemap drops the tiles, so coming back asks again.
    #[test]
    fn only_the_satellite_basemap_has_imagery() {
        let Some(gpu) = gpu() else { return };
        let fetcher = Noted::new(Plain([200, 100, 50]));
        let mut renderer = renderer(&gpu, fetcher.clone());
        let size = [320.0, 200.0];
        // (200, 100, 50) × (150, 158, 180) / 255
        let tinted = [118, 62, 35];
        for basemap in [Basemap::Dark, Basemap::Realistic] {
            let mut scene = satellite(v3(), size);
            scene.basemap = basemap;
            let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
            assert!(fetcher.asked().is_empty(), "{basemap:?}: asked {:?}", fetcher.asked());
            assert_ne!(rgb(image.pixel(160, 100)), tinted);
            assert!(!renderer.pending());
        }
        let image = render_with(&gpu, &mut renderer, &satellite(v3(), size), 1.0).expect("rendered");
        // z10 (round(9 + 1)) under the view, and on the globe a coarser tile further off
        let asked = fetcher.asked();
        assert!(asked.iter().all(|(source, t)| source == io::ESRI && t.z <= 10), "{asked:?}");
        assert!(asked.iter().filter(|(_, t)| t.z == 10).count() >= 4, "{asked:?}");
        let centre = rgb(image.pixel(160, 100));
        assert!(centre.iter().zip(tinted).all(|(c, t)| c.abs_diff(t) <= 1), "{centre:?}");
        // away and back: asked again
        let mut dark = satellite(v3(), size);
        dark.basemap = Basemap::Dark;
        render_with(&gpu, &mut renderer, &dark, 1.0).expect("rendered");
        render_with(&gpu, &mut renderer, &satellite(v3(), size), 1.0).expect("rendered");
        assert_eq!(fetcher.asked().len(), 2 * asked.len());
    }

    /// One tile set serves both projections, as deck's one `imagery` layer keeps its cache, and
    /// both sides of zoom 12 select `round(zoom + 1)` = 13: at V3, the flat map at 12.1 asks for no
    /// tile the globe at 11.9 has not, at each of the shots' sizes, and both draw the picture.
    #[test]
    fn crossing_zoom_12_asks_for_no_new_tile() {
        let Some(gpu) = gpu() else { return };
        for size in [[1440.0, 900.0], [1100.0, 720.0], [390.0, 844.0]] {
            let fetcher = Noted::new(Plain([200, 100, 50]));
            let mut renderer = renderer(&gpu, fetcher.clone());
            let globe = render_with(&gpu, &mut renderer, &satellite(ViewState { zoom: 11.9, ..v3() }, size), 1.0).expect("rendered");
            let before = fetcher.asked();
            assert!(!before.is_empty() && before.iter().all(|(_, t)| t.z == 13), "{before:?}");
            let flat = render_with(&gpu, &mut renderer, &satellite(ViewState { zoom: 12.1, ..v3() }, size), 1.0).expect("rendered");
            eprintln!("{size:?}: {} tiles at 11.9, {} requests after 12.1", before.len(), fetcher.asked().len());
            assert_eq!(fetcher.asked(), before, "crossing zoom 12 asked again");
            for image in [&globe, &flat] {
                let whole = image.rgba.chunks_exact(4).filter(|p| p[..3].iter().zip([118, 62, 35]).all(|(c, t)| c.abs_diff(t) <= 1)).count();
                assert_eq!(whole, (image.width * image.height) as usize, "{size:?}: the picture does not fill the view");
            }
        }
    }
}
