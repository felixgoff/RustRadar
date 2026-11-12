//! The RainViewer ground layer (item W2; doc 02 §3.5 "RainViewer ground layer"): today's
//! `renderRadarTile` (G:1204-1215), the radar TileLayer's sub-layer for each tile.
//!
//! - **What is drawn.** Every visible RainViewer tile with echo ([`RadarTiles::visible`], coarse to
//!   fine: deck's `best-available` tiles), its slices whole (`tile.ground`, `polygonData(slices)`:
//!   every band, flat on the ground) in the NWS palette's colours and alphas, unlit
//!   (`material: false`), at opacity [`OPACITY`] (0.55), with `SURFACE_NO_CULL` (both faces, since
//!   the triangles' winding is not kept consistent; no depth written; less-equal). A dry tile draws
//!   nothing.
//! - **Uploaded once** ([`GroundTile::upload`], the tile runtime's upload within the frame's budget):
//!   the tile's slices in its own frame (`RadarTile::slice_vertices`, tile units from its
//!   north-west corner) with their triangles, so the flat map and the globe draw the same buffers
//!   and a tile never loads again at zoom 12. Each frame pushes each tile's frame
//!   ([`crate::layers::PrepareCx::tile_frame`]) and draws it with `radar.wgsl` (the measured
//!   volumes' pipelines, [`RadarPipelines`]).
//! - The lifted layers (W3) draw from the same tiles through the arenas ([`super::arena`]); the
//!   stack is built from the tiles in view, not here, so tiles loaded at different moments never
//!   disagree on the camera's tilt or the height scale.
//!
//! Deviations: none in what is drawn. Each tile is its own draw with its own frame (deck: one
//! SolidPolygonLayer per tile, the same).

use super::measured::{RadarPipelines, Style, StyleUniform};
use super::tiles::{RadarTile, RadarTiles};
use crate::frame::FrameSlot;
use crate::layers::{DrawCx, Init, PrepareCx, TileIndex};
use crate::pipelines::fill;

/// The ground layer's opacity (`renderRadarTile`'s `opacity: 0.55`).
pub const OPACITY: f64 = 0.55;

/// What the upload makes of a RainViewer tile with echo: its slices (every band) on the GPU, in the
/// tile's frame.
pub struct GroundTile {
    mesh: fill::Mesh<fill::Vertex>,
}

impl GroundTile {
    /// `tile`'s slices with their triangles (`tile.ground`).
    pub fn upload(device: &wgpu::Device, tile: &RadarTile) -> Self {
        GroundTile { mesh: fill::Mesh::new(device, "radar tile ground", &tile.slice_vertices, &tile.mesh.slices.indices) }
    }

    /// Indices drawn (three a triangle).
    pub fn index_count(&self) -> u32 {
        self.mesh.index_count()
    }
}

/// RainViewer's tiles with their ground meshes on the GPU.
pub type Tiles = RadarTiles<GroundTile>;

/// The ground layer: one draw per visible tile with echo.
pub struct Ground {
    pipelines: RadarPipelines,
    style: Style,
    /// This frame's tiles and their frames, in drawing order.
    draws: Vec<(TileIndex, FrameSlot)>,
}

impl Ground {
    pub fn new(init: &Init<'_>) -> Self {
        let pipelines = RadarPipelines::new(init, "radar ground");
        let style = pipelines.style(&init.gpu.device, StyleUniform::new(OPACITY, 0.0));
        Ground { pipelines, style, draws: Vec::new() }
    }

    /// This frame's draws: a frame for each visible tile with echo, on the ground.
    pub fn prepare(&mut self, cx: &mut PrepareCx<'_>, tiles: &Tiles) {
        self.draws.clear();
        for (index, _, _) in tiles.visible() {
            self.draws.push((index, cx.tile_frame(index, 0.0)));
        }
    }

    /// How many tiles this frame draws.
    pub fn draw_count(&self) -> usize {
        self.draws.len()
    }

    /// Records this frame's draws, from the tiles `prepare` saw.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, tiles: &Tiles) {
        if self.draws.is_empty() {
            return;
        }
        self.pipelines.bind(pass, false, &self.style);
        for &(index, frame) in &self.draws {
            let Some((_, tile)) = tiles.get(index) else { continue };
            cx.bind_frame(pass, frame);
            tile.mesh.draw(pass);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::{ViewState, lng_lat_to_world};
    use rustradar_scene::radar::{self, BAND_COUNT, band_color};

    use super::super::tiles;
    use super::*;
    use crate::frame::{ViewFrame, tile_extent};
    use crate::gpu::Gpu;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{self, DirTiles, TileFetcher};
    use crate::layers::{earth, vector_map};
    use crate::renderer::{MapRenderer, SceneInput};

    const FRAME: &str = "https://tilecache.rainviewer.com/v2/radar/47622fe604ca/256/{z}/{x}/{y}/2/1_1.png";

    fn v2() -> ViewState {
        ViewState { latitude: 52.0, longitude: 10.0, zoom: 4.5, pitch: 0.0, bearing: 0.0, position: None }
    }

    fn shots_dir() -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    /// The dark map of `view` on a `size` canvas from the committed tiles, with the fixture
    /// RainViewer frame when `radar`, saved as `name` when it has one; and the ground layer's draws.
    fn dark_shot(gpu: &Gpu, view: ViewState, size: [f64; 2], radar: bool, name: Option<&str>) -> Image {
        let mut scene = SceneInput::new(view, size, 1.0, Basemap::Dark);
        scene.earth = earth::Input::SHOWN;
        scene.vector_map = vector_map::Input::SHOWN;
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let tiles: Arc<dyn TileFetcher> = Arc::new(DirTiles::fixtures());
        renderer.set_tiles(tiles, None);
        let input = tiles::Input { url: Some(FRAME.into()), ..Default::default() };
        if radar && super::super::lifted::lift_for(view.pitch) > 0.0 {
            // tilted, the radar layer stands its stack up over the ground too (W3, tested in
            // `lifted.rs`): the ground layer alone here, last over the map as the radar row is
            let mut probe = Probe::new(&renderer.init(gpu));
            probe.input = input;
            renderer.add_layer(Box::new(probe));
        } else if radar {
            scene.radar.tiles = input;
        }
        let image = render_with(gpu, &mut renderer, &scene, 1.0).expect("rendered");
        assert!(!renderer.pending(), "still loading: {:?}", renderer.pending_layers());
        if let Some(name) = name {
            image.save_png(&shots_dir().join(name)).expect("the shot written");
        }
        image
    }

    /// deck's blending of `color` at `opacity` over `under` (gamma space, as the map pass blends).
    fn over(under: [u8; 4], [r, g, b, a]: [u8; 4], opacity: f64) -> [f64; 3] {
        let alpha = f64::from(a) / 255.0 * opacity;
        [r, g, b].map(f64::from).iter().zip(&under).map(|(c, u)| f64::from(*u) + (c - f64::from(*u)) * alpha).collect::<Vec<_>>().try_into().unwrap()
    }

    /// Whether the 3 × 3 pixels round `(x, y)` are one colour in `image` (no edge there).
    fn uniform(image: &Image, x: u32, y: u32) -> bool {
        let c = image.pixel(x, y);
        (x - 1..=x + 1).all(|i| (y - 1..=y + 1).all(|j| image.pixel(i, j) == c))
    }

    /// The top band of the shortest stack of two to six bands, weakest first, that drawn over
    /// `under` gives `got` (within rounding), if any.
    fn stacked(under: [u8; 4], got: [u8; 4]) -> Option<usize> {
        fn search(under: [u8; 4], got: [u8; 4], from: usize, left: usize) -> Option<usize> {
            (from..BAND_COUNT).find_map(|band| {
                let c = over(under, band_color(band), OPACITY).map(|c| c.round() as u8);
                let next = [c[0], c[1], c[2], 255];
                if left == 1 {
                    let off = (0..3).map(|i| (i32::from(next[i]) - i32::from(got[i])).abs()).max().unwrap();
                    (off <= 2).then_some(band)
                } else {
                    search(next, got, band + 1, left - 1)
                }
            })
        }
        (2..=6).find_map(|n| search(under, got, 0, n))
    }

    /// The blurred reflectivity `process_tile` contours, for each fixture tile asked.
    struct Fields(std::collections::HashMap<TileIndex, Option<Vec<f32>>>);

    impl Fields {
        /// The bands of the blurred field's four samples round world point `[x, y]` in the `z`
        /// tile under it: a band contoured there lies between their lowest and highest.
        fn bands_round(&mut self, [x, y]: [f64; 2], z: u32) -> Option<(usize, usize)> {
            let extent = tile_extent(z);
            let (fx, fy) = (x / extent, (rustradar_scene::camera::WORLD_SIZE - y) / extent);
            let tile = TileIndex::new(fx.floor() as u32, fy.floor() as u32, z);
            // (a tile the fixtures do not hold was not drawn)
            let field = self
                .0
                .entry(tile)
                .or_insert_with(|| {
                    let path = io::fixture_tiles_dir().join(format!("rainviewer/{}/{}/{}.png", tile.z, tile.x, tile.y));
                    let image = image::load_from_memory(&std::fs::read(path).ok()?).ok()?.into_rgba8();
                    let dbz: Vec<f32> = image.as_raw().chunks_exact(4).map(|p| radar::reflectivity(p[0], p[1], p[2], p[3]).max(0.0) as f32).collect();
                    Some(radar::mesh::blur(&dbz, 256, 256))
                })
                .as_ref()?;
            // the samples sit at the pixels' middles
            let (px, py) = ((fx - fx.floor()) * 256.0 - 0.5, (fy - fy.floor()) * 256.0 - 0.5);
            let band = |dbz: f32| ((f64::from(dbz) - 5.0) / 5.0).floor();
            let mut range: Option<(f64, f64)> = None;
            for (dx, dy) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                let (c, r) = ((px.floor() + dx).clamp(0.0, 255.0) as usize, (py.floor() + dy).clamp(0.0, 255.0) as usize);
                let b = band(field[r * 256 + c]);
                range = Some(range.map_or((b, b), |(lo, hi)| (lo.min(b), hi.max(b))));
            }
            let (lo, hi) = range?;
            (hi >= 0.0).then(|| (lo.max(0.0) as usize, (hi as usize).min(BAND_COUNT - 1)))
        }
    }

    /// A triangle in world Mercator and its band.
    type Triangle = ([[f64; 2]; 3], usize);

    /// The tile zooms a V2 view draws: its selection's (6), the coarser one deck takes far out on
    /// the globe (5, 4), and pitched the finer one near the camera (7).
    const ZOOMS: [u32; 4] = [7, 6, 5, 4];

    /// The fixture tiles' own meshes, as world-Mercator triangles with their bands.
    struct Meshes(std::collections::HashMap<TileIndex, Vec<Triangle>>);

    impl Meshes {
        /// The bands of the triangles of the `z` tile's own mesh (`process_tile`'s) that cover
        /// world point `p`, in drawing order.
        fn bands_covering(&mut self, p: [f64; 2], z: u32) -> Vec<usize> {
            let extent = tile_extent(z);
            let tile = TileIndex::new((p[0] / extent).floor() as u32, ((rustradar_scene::camera::WORLD_SIZE - p[1]) / extent).floor() as u32, z);
            let triangles = self.0.entry(tile).or_insert_with(|| {
                let path = io::fixture_tiles_dir().join(format!("rainviewer/{}/{}/{}.png", tile.z, tile.x, tile.y));
                let Some(radar) = std::fs::read(path).ok().and_then(|bytes| tiles::decode(tile, &bytes, &[], &crate::jobs::Cancel::new()).ok()?.tile) else {
                    return Vec::new();
                };
                let s = &radar.mesh.slices;
                let world: Vec<[f64; 2]> = s.positions.chunks_exact(2).map(|q| lng_lat_to_world([f64::from(q[0]), f64::from(q[1])])).collect();
                let band = |t: usize| (0..BAND_COUNT).rfind(|&b| s.band_starts[b] as usize <= t).unwrap();
                s.indices.chunks_exact(3).enumerate().map(|(k, t)| ([0, 1, 2].map(|c| world[t[c] as usize]), band(3 * k))).collect()
            });
            let cross = |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
            triangles
                .iter()
                .filter(|([a, b, c], _)| {
                    let (d1, d2, d3) = (cross(*a, *b), cross(*b, *c), cross(*c, *a));
                    (d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0)
                })
                .map(|(_, band)| *band)
                .collect()
        }
    }

    /// The acceptance shot: V2 over the dark map with the committed RainViewer frame (at two sizes,
    /// and pitched 45°). Wherever the radar is one colour it is one of the NWS bands at 55 % over
    /// the map below it; the bands from light rain (cyan) to heavy (red) are all there, and each
    /// sits on the ground where the frame's picture has that band (its blurred reflectivity, as
    /// contoured). A few places show bands stacked: there a tile's own mesh covers the place more
    /// than once, as today's does (the meshes are `processTile`'s, checked value for value by W1).
    #[test]
    fn v2_shows_the_nws_bands_at_55_percent_over_the_dark_map() {
        let Some(gpu) = gpu() else { return };
        // pitched 45° the ground layer still lies on the ground (the stack is W3's)
        let pitched = ViewState { pitch: 45.0, ..v2() };
        for (view, size, name) in [
            (v2(), [1100.0, 720.0], "w2-radar-v2.png"),
            (v2(), [1440.0, 900.0], "w2-radar-v2-1440x900.png"),
            (pitched, [1100.0, 720.0], "w2-radar-v2-pitched.png"),
        ] {
            let base = dark_shot(&gpu, view, size, false, None);
            let image = dark_shot(&gpu, view, size, true, Some(name));
            let viewport = &ViewFrame::new(&view, size, 1.0, 0.0).viewport;
            let mut fields = Fields(Default::default());
            let mut meshes = Meshes(Default::default());
            let (mut changed, mut inside, mut matched, mut placed, mut stacks) = (0usize, 0usize, 0usize, 0usize, 0usize);
            let mut stacked_top = [0usize; BAND_COUNT];
            let mut bands = [0usize; BAND_COUNT];
            for y in 1..image.height - 1 {
                for x in 1..image.width - 1 {
                    let (before, after) = (base.pixel(x, y), image.pixel(x, y));
                    if before == after {
                        continue;
                    }
                    changed += 1;
                    if !uniform(&image, x, y) || !uniform(&base, x, y) {
                        continue;
                    }
                    inside += 1;
                    let off = |want: [f64; 3]| (0..3).map(|i| (want[i] - f64::from(after[i])).abs()).fold(0.0, f64::max);
                    let (miss, band) = (0..BAND_COUNT).map(|band| (off(over(before, band_color(band), OPACITY)), band)).min_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
                    if miss > 1.5 {
                        // today's meshing covers some places more than once (math.gl's grid cut
                        // loses holes of a band's polygon, as `measured.rs` found for MRMS): the
                        // bands there drawn one over another, weakest first
                        let stack = stacked(before, after).unwrap_or_else(|| panic!("{name} ({x}, {y}): {after:?} over {before:?} is no band nor a stack of them (band {band} off by {miss})"));
                        // and the stack is in the data: a tile's own mesh covers the place more
                        // than once, up to that band
                        let [lon, lat] = viewport.unproject([f64::from(x) + 0.5, f64::from(y) + 0.5], 0.0);
                        let world = lng_lat_to_world([lon, lat]);
                        let covering = ZOOMS.map(|z| meshes.bands_covering(world, z));
                        assert!(
                            covering.iter().any(|bands| bands.len() >= 2 && bands.iter().max() == Some(&stack)),
                            "{name} ({x}, {y}): a stack up to band {stack} where the tiles' meshes hold {covering:?}"
                        );
                        stacks += 1;
                        stacked_top[stack] += 1;
                        continue;
                    }
                    matched += 1;
                    bands[band] += 1;
                    let [lon, lat] = viewport.unproject([f64::from(x) + 0.5, f64::from(y) + 0.5], 0.0);
                    let world = lng_lat_to_world([lon, lat]);
                    if ZOOMS.iter().any(|&z| fields.bands_round(world, z).is_some_and(|(lo, hi)| (lo..=hi).contains(&band))) {
                        placed += 1;
                    }
                }
            }
            eprintln!("{name}: {changed} px changed, {inside} inside a band, {matched} one band, {stacks} stacked (by top band {stacked_top:?}); placed {placed}; by band {bands:?}");
            assert!(changed > 50_000, "{name}: {changed} px of radar");
            assert!(matched * 100 >= inside * 97, "{name}: {matched} of {inside} one band");
            // light rain (cyan) to 45 dBZ (orange) wherever there is room, and some red
            assert!(bands[..9].iter().all(|&n| n > 0) && bands[9..12].iter().any(|&n| n > 0), "{name}: {bands:?}");
            assert!(placed * 100 >= matched * 97, "{name}: {placed} of {matched} where the picture has their band");
        }
    }

    /// The ground draws each visible tile with echo once, and nothing with the weather off.
    #[test]
    fn each_visible_tile_is_drawn_once_and_none_without_rainviewer() {
        let Some(gpu) = gpu() else { return };
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        let tiles: Arc<dyn TileFetcher> = Arc::new(DirTiles::fixtures());
        renderer.set_tiles(tiles, None);
        // (the scene's own radar stays off: the probe has its own tiles)
        let scene = SceneInput::new(v2(), [1100.0, 720.0], 1.0, Basemap::Dark);
        let mut probe = Probe::new(&renderer.init(&gpu));
        let seen = Arc::clone(&probe.seen);
        probe.input = tiles::Input { url: Some(FRAME.into()), ..Default::default() };
        renderer.add_layer(Box::new(probe));
        render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        let (draws, visible, indices) = *seen.lock().unwrap();
        assert!(draws >= 20, "{draws}");
        assert_eq!(draws, visible);
        assert!(indices > 1_000_000, "{indices}");
        // no frame: nothing to draw
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        let probe = Probe::new(&renderer.init(&gpu));
        let seen = Arc::clone(&probe.seen);
        renderer.add_layer(Box::new(probe));
        render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        assert_eq!(*seen.lock().unwrap(), (0, 0, 0));
    }

    /// The ground layer on its own tiles, reporting its draws, the visible tiles and their indices.
    struct Probe {
        tiles: Tiles,
        ground: Ground,
        input: tiles::Input,
        seen: Arc<std::sync::Mutex<(usize, usize, u64)>>,
    }

    impl Probe {
        fn new(init: &Init<'_>) -> Self {
            Probe { tiles: RadarTiles::new(init), ground: Ground::new(init), input: tiles::Input::default(), seen: Default::default() }
        }
    }

    impl crate::layers::Layer for Probe {
        fn name(&self) -> &'static str {
            "ground probe"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.tiles.update(cx, &self.input, |gpu, _, radar| GroundTile::upload(&gpu.device, radar));
            self.ground.prepare(cx, &self.tiles);
            let indices = self.tiles.visible().map(|(_, _, g)| u64::from(g.index_count())).sum();
            *self.seen.lock().unwrap() = (self.ground.draw_count(), self.tiles.visible().count(), indices);
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            self.ground.draw(pass, cx, &self.tiles);
        }

        fn pending(&self) -> bool {
            self.tiles.pending()
        }
    }
}
