//! `ground-globe` (item R2; doc 04 §1 row 5b, §6.9; `realistic-ground.ts`'s `groundLayer`,
//! `RG:792-818`, G:2660-2661): the realistic map's ground on the globe, OpenMapTiles' tiles drawn
//! with the ground shader.
//!
//! What today's app does, and so this layer:
//! - **When.** On the realistic basemap while the view is the globe (zoom ≤ 12): on the flat map
//!   the city ground takes over (`ground-city`, item R3), and `ground-globe` leaves deck's list,
//!   its tiles with it. Drawn after `ground-land` and before the land cover's and rails' layers.
//! - **Tiles.** OpenFreeMap's (the vector map's source, `io::OPENFREEMAP`) by deck's own rule
//!   for 512-px tiles, zooms 0–14 (`tiles::VECTOR_TILES`; on the globe no deeper than z12), deck's
//!   TileLayer defaults: 6 requests at a time, best-available refinement. Each tile is decoded on
//!   the worker pool by `scene::tess::ground::tessellate` (the globe's GeoJSON way: cut on the 5°
//!   grid; [`rustradar_scene::ground::GROUND_LAYERS`] in that order), uploaded within the frame's
//!   budget, and drawn in its tile frame, coarse tiles first.
//! - **Three passes a tile** (`GroundTile`): its areas, its lines' edging, its lines, tile after
//!   tile; `SURFACE_NO_CULL` (depth-tested against the earth sphere, no depth written, unculled);
//!   lines in metres at least 0.6 px wide (`lineWidthMinPixels: 0.6`), round joints and caps.
//!   Not clipped to the tile (deck clips only on the flat map).

use rustradar_scene::ground::GROUND_LAYERS;
use rustradar_scene::tess::ground::{self as tess, GroundTile, GroundVertex};
use rustradar_scene::tiles::VECTOR_TILES;

use super::{GroundShader, LineMesh, LineWidth};
use crate::frame::FrameSlot;
use crate::io;
use crate::jobs::Cancel;
use crate::layers::vector_map::MAX_REQUESTS;
use crate::layers::{ByteSize, DeckSelect, DrawCx, Init, PrepareCx, Source, TileIndex, TileSet, TileSetOptions};
use crate::pipelines::fill::Mesh;

/// A decoded tile's bytes, for the cache's budget.
pub struct Decoded(pub GroundTile);

impl ByteSize for Decoded {
    fn byte_size(&self) -> usize {
        self.0.byte_size()
    }
}

/// A tile on the GPU: its three passes.
pub struct TileMeshes {
    pub areas: Mesh<GroundVertex>,
    pub edges: LineMesh,
    pub lines: LineMesh,
}

/// The decode step, on the worker pool (`None` for a tile with nothing to draw).
pub fn decode(tile: TileIndex, bytes: &[u8], cancel: &Cancel) -> Result<Option<Decoded>, String> {
    match tess::tessellate(tile, bytes, &GROUND_LAYERS, false, &|| cancel.is_cancelled()) {
        Ok(decoded) if decoded.is_empty() => Ok(None),
        Ok(decoded) => Ok(Some(Decoded(decoded))),
        Err(tess::Cancelled) => Err("cancelled".into()),
    }
}

/// The upload step, on the main thread.
pub fn upload(device: &wgpu::Device, decoded: &GroundTile) -> TileMeshes {
    TileMeshes {
        areas: Mesh::new(device, "ground areas", &decoded.vertices, &decoded.indices),
        edges: LineMesh::new(device, "ground edges", &decoded.edges),
        lines: LineMesh::new(device, "ground lines", &decoded.lines),
    }
}

/// `ground-globe`'s tiles.
pub struct GroundGlobe {
    tiles: TileSet<Decoded, TileMeshes>,
    shown: bool,
    /// This frame's tiles in draw order, each with its frame.
    drawn: Vec<(FrameSlot, TileIndex)>,
}

impl GroundGlobe {
    pub fn new(init: &Init<'_>) -> Self {
        let tiles = init.tile_set(
            vec![Source::new(io::OPENFREEMAP)],
            DeckSelect(VECTOR_TILES),
            TileSetOptions { max_requests: MAX_REQUESTS, ..Default::default() },
            |tile, bytes, cancel| decode(tile, &bytes, cancel),
        );
        GroundGlobe { tiles, shown: false, drawn: Vec::new() }
    }

    /// The tile set (tests).
    pub fn tiles(&self) -> &TileSet<Decoded, TileMeshes> {
        &self.tiles
    }

    /// This frame: the tiles for the camera when `shown`, else none (and the cache dropped).
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

    /// Each tile's areas, edging and lines.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, shader: &GroundShader, areas: &wgpu::RenderPipeline, lines: &wgpu::RenderPipeline) {
        for &(slot, tile) in &self.drawn {
            let Some(meshes) = self.tiles.get(tile) else { continue };
            if meshes.areas.index_count() > 0 {
                shader.begin(pass, areas, LineWidth::Hairline);
                shader.draw_areas(pass, cx, slot, &meshes.areas);
            }
            if !meshes.edges.is_empty() || !meshes.lines.is_empty() {
                shader.begin(pass, lines, LineWidth::Hairline);
                shader.draw_lines(pass, cx, slot, &meshes.edges);
                shader.draw_lines(pass, cx, slot, &meshes.lines);
            }
        }
    }

    /// Whether tiles in view are still to come.
    pub fn pending(&self) -> bool {
        self.shown && self.tiles.loading()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use rustradar_core::geo::Basemap;
    use rustradar_core::weather::PRESETS;
    use rustradar_scene::camera::{ViewState, world_to_lng_lat};
    use rustradar_scene::geom::path::LocalMercator;
    use rustradar_scene::ground::WATER_ID;
    use rustradar_scene::tiles::tile_indices;

    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::DirTiles;
    use crate::layers::{earth, ground, sky};
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 10:00 UTC: noon in Stockholm (the reference pictures' clock).
    const MIDSUMMER: f64 = 1_782_036_000_000.0;
    /// V3 (plan section 8): the Stockholm region, at today's pictures' size.
    const V3: ViewState = ViewState { latitude: 59.33, longitude: 18.07, zoom: 9.0, pitch: 0.0, bearing: 0.0, position: None };
    const SIZE: [f64; 2] = [1440.0, 900.0];
    /// The map in today's pictures at 1440 × 900: right of the sidebar, left of the map controls,
    /// under the mode bar and the dev readout, above the credits.
    const MAP: [u32; 4] = [360, 80, 1370, 880];
    /// A pixel the CPU's map gives no area (the land under the tiles), and one near a line.
    const LAND: u8 = 254;
    const NEAR_LINE: u8 = 255;

    fn reference(name: &str) -> Image {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference").join(name);
        let rgba = image::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).into_rgba8();
        Image { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() }
    }

    /// Which surface the ground shows at each pixel, worked out on the CPU from the same tiles
    /// tessellated (each triangle filled in draw order, the last one over a pixel's centre
    /// counting), [`LAND`] where no area is; and how far each pixel's centre is from the nearest
    /// line of the given surfaces' (the roads), in pixels.
    struct SurfaceMap {
        width: usize,
        surfaces: Vec<u8>,
        road_distance: Vec<f32>,
    }

    impl SurfaceMap {
        fn new(view: &ViewFrame, size: [usize; 2], roads: &[u8]) -> Self {
            let [width, height] = size;
            let mut map = SurfaceMap { width, surfaces: vec![LAND; width * height], road_distance: vec![f32::INFINITY; width * height] };
            let mut tiles = tile_indices(&view.viewport, &VECTOR_TILES);
            tiles.sort_by_key(|t| t.z);
            let fixtures = DirTiles::fixtures();
            let mut lines = Vec::new();
            for tile in tiles {
                let crate::io::Held::Tile(path) = fixtures.held(io::OPENFREEMAP, tile) else { continue };
                let bytes = std::fs::read(path).expect("the fixture");
                let ground = tess::tessellate(tile, &bytes, &GROUND_LAYERS, false, &|| false).expect("not cancelled");
                let local = LocalMercator::tile(tile);
                let pixel = |p: [f32; 3]| {
                    let [lon, lat] = world_to_lng_lat(local.to_world([f64::from(p[0]), f64::from(p[1])]).to_array());
                    let [x, y, _] = view.viewport.project([lon, lat, 0.0]);
                    [x, y]
                };
                let screen: Vec<[f64; 2]> = ground.vertices.iter().map(|v| pixel(v.position)).collect();
                for tri in ground.indices.chunks_exact(3) {
                    let [a, b, c] = [0, 1, 2].map(|k| screen[tri[k] as usize]);
                    map.fill(a, b, c, ground.vertices[tri[0] as usize].surface, height);
                }
                for s in ground.edges.iter().chain(&ground.lines) {
                    lines.push((pixel(s.start), pixel(s.end), s.color[3], f64::from(s.width)));
                }
            }
            // what lies near a line (its edging or itself, at least 0.6 px wide, the hairline's
            // blend and a pixel more) is no area's; a road's own pixels are measured
            let mpp = view.viewport.meters_per_pixel();
            for &(a, b, surface, width) in &lines {
                let half = (width / mpp).max(0.6) / 2.0 + 1.5;
                let road = roads.contains(&surface);
                // (a road's distance measured a few pixels further out, for the land beside it)
                map.near(a, b, if road { half.max(4.0) } else { half }, height, |m, i, d| {
                    if d <= half {
                        m.surfaces[i] = NEAR_LINE;
                    }
                    if road {
                        m.road_distance[i] = m.road_distance[i].min(d as f32);
                    }
                });
            }
            map
        }

        /// Fills the triangle `a b c` (pixels) with `surface`, pixel centres inside.
        fn fill(&mut self, a: [f64; 2], b: [f64; 2], c: [f64; 2], surface: u8, height: usize) {
            let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            if area == 0.0 || !area.is_finite() {
                return;
            }
            let x0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
            let x1 = (a[0].max(b[0]).max(c[0]).ceil().max(0.0) as usize).min(self.width);
            let y0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
            let y1 = (a[1].max(b[1]).max(c[1]).ceil().max(0.0) as usize).min(height);
            let edge = |p: [f64; 2], q: [f64; 2], x: f64, y: f64| ((q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0])) * area.signum();
            for y in y0..y1 {
                for x in x0..x1 {
                    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                    if edge(a, b, px, py) >= 0.0 && edge(b, c, px, py) >= 0.0 && edge(c, a, px, py) >= 0.0 {
                        self.surfaces[y * self.width + x] = surface;
                    }
                }
            }
        }

        /// Calls `f` for each pixel whose centre lies within `half` px of the segment `a b`.
        fn near(&mut self, a: [f64; 2], b: [f64; 2], half: f64, height: usize, mut f: impl FnMut(&mut Self, usize, f64)) {
            let x0 = (a[0].min(b[0]) - half).floor().max(0.0) as usize;
            let x1 = ((a[0].max(b[0]) + half).ceil().max(0.0) as usize).min(self.width);
            let y0 = (a[1].min(b[1]) - half).floor().max(0.0) as usize;
            let y1 = ((a[1].max(b[1]) + half).ceil().max(0.0) as usize).min(height);
            let d = [b[0] - a[0], b[1] - a[1]];
            let length2 = d[0] * d[0] + d[1] * d[1];
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = [x as f64 + 0.5 - a[0], y as f64 + 0.5 - a[1]];
                    let t = if length2 > 0.0 { ((p[0] * d[0] + p[1] * d[1]) / length2).clamp(0.0, 1.0) } else { 0.0 };
                    let distance = (p[0] - t * d[0]).hypot(p[1] - t * d[1]);
                    if distance <= half {
                        f(self, y * self.width + x, distance);
                    }
                }
            }
        }

        /// The surface at `(x, y)` when every pixel within 2 px has it too.
        fn inner(&self, x: usize, y: usize) -> Option<u8> {
            let s = self.surfaces[y * self.width + x];
            let same = (y - 2..=y + 2).all(|y| (x - 2..=x + 2).all(|x| self.surfaces[y * self.width + x] == s));
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

    fn distance(a: [u8; 3], b: [u8; 3]) -> f64 {
        (0..3).map(|i| (f64::from(a[i]) - f64::from(b[i])).powi(2)).sum::<f64>().sqrt()
    }

    /// R2's acceptance (V3 realistic, today's reference clock and sunny sky): the globe's ground
    /// from the fixture tiles shows water, woods, fields, gardens, towns and the land under them
    /// in today's hues, each class's median colour within 8 levels of `realistic-v3-1440x900.png`'s
    /// at the same pixels (which also has the hillshade, R12, and the place names), the classes
    /// well apart; and the roads show as hairlines apart from the land round them, as today's do
    /// (as bright against it, and their colour within 12 levels: today's picture is a 256-colour
    /// dither). The classes are worked out on the CPU from the same tiles.
    #[test]
    fn v3_shows_water_woods_fields_and_roads_in_todays_hues() {
        let Some(gpu) = gpu() else { return };
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(Arc::new(DirTiles::fixtures()), None);
        let mut scene = SceneInput::new(V3, SIZE, 1.0, Basemap::Realistic);
        scene.local_now_ms = MIDSUMMER;
        scene.earth = earth::Input::SHOWN;
        scene.sky = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
        scene.ground = ground::Input::SHOWN;
        let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join("r2-ground-v3.png")).expect("png");
        assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
        let today = reference("realistic-v3-1440x900.png");
        let view = ViewFrame::new(&V3, SIZE, 1.0, 0.0);
        // marked and unmarked roads
        let map = SurfaceMap::new(&view, [1440, 900], &[32, 33]);
        // today's pictures stand about 4.5 px higher than deck's own viewport (T1)
        let theirs = |x: u32, y: u32| today.pixel(x, y - 4);
        // per surface our colours and today's; a road's middle and the land beside it: our
        // colours and their pixels
        let mut classes: BTreeMap<u8, [Vec<[u8; 3]>; 2]> = BTreeMap::new();
        let (mut road_mine, mut road_at, mut beside_mine, mut beside_at) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let [x0, y0, x1, y1] = MAP;
        for y in (y0..y1).step_by(2) {
            for x in (x0..x1).step_by(2) {
                let (mine, today) = (image.pixel(x, y), theirs(x, y));
                let (mine, today) = ([mine[0], mine[1], mine[2]], [today[0], today[1], today[2]]);
                if let Some(s) = map.inner(x as usize, y as usize) {
                    let class = classes.entry(s).or_default();
                    class[0].push(mine);
                    class[1].push(today);
                }
                // a road's middle, and land a few pixels off it
                let d = map.road_distance[y as usize * 1440 + x as usize];
                if d <= 0.3 {
                    road_mine.push(mine);
                    road_at.push([x, y]);
                } else if (2.5..=3.5).contains(&d) {
                    beside_mine.push(mine);
                    beside_at.push([x, y]);
                }
            }
        }
        let mut medians = BTreeMap::new();
        for (&surface, [mine, today]) in &mut classes {
            if mine.len() < 200 {
                continue;
            }
            let (m, t) = (median(mine), median(today));
            eprintln!("surface {surface:3}: {} pixels, ours {m:?}, today's {t:?}", mine.len());
            assert!((0..3).all(|i| m[i].abs_diff(t[i]) <= 8), "surface {surface}: {m:?} against today's {t:?}");
            medians.insert(surface, m);
        }
        // water, woods, fields, gardens and towns, and the land: each its own colour
        for surface in [WATER_ID, 2, 3, 5, LAND] {
            assert!(medians.contains_key(&surface), "no surface {surface} in {:?}", medians.keys());
        }
        for (a, &ca) in &medians {
            for (b, &cb) in &medians {
                if a < b && [*a, *b].iter().all(|s| [WATER_ID, 2, 3, 5].contains(s)) {
                    assert!(distance(ca, cb) > 8.0, "surfaces {a} and {b} alike: {ca:?} {cb:?}");
                }
            }
        }
        // the roads: hairlines apart from the land beside them, as today's
        // today's hairlines sit where today's offset puts them, to a part of a pixel: the offset
        // within a pixel of T1's that sets them most apart from the land
        let today_at = |at: &[[u32; 2]], dx: i32, dy: i32| {
            let mut colours: Vec<[u8; 3]> = at.iter().map(|&[x, y]| today.pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32)).map(|p| [p[0], p[1], p[2]]).collect();
            median(&mut colours)
        };
        let (road_today, beside_today) = (-1..=1)
            .flat_map(|dx| (-6..=-3).map(move |dy| (dx, dy)))
            .map(|(dx, dy)| (today_at(&road_at, dx, dy), today_at(&beside_at, dx, dy)))
            .max_by(|a, b| distance(a.0, a.1).total_cmp(&distance(b.0, b.1)))
            .expect("offsets");
        let (road, beside) = (median(&mut road_mine), median(&mut beside_mine));
        eprintln!("roads: {} pixels, ours {road:?} beside {beside:?}; today's {road_today:?} beside {beside_today:?}", road_mine.len());
        assert!(road_mine.len() > 500);
        assert!(distance(road, beside) > 6.0, "{road:?} beside {beside:?}");
        assert!(distance(road_today, beside_today) > 6.0);
        // as bright against the land as today's (within 2 levels of luma); in colour within 12
        // levels, today's picture being a 256-colour dither, which keeps no hairline's own blue
        let luma = |c: [u8; 3]| 0.2126 * f64::from(c[0]) + 0.7152 * f64::from(c[1]) + 0.0722 * f64::from(c[2]);
        let (contrast, contrast_today) = (luma(road) - luma(beside), luma(road_today) - luma(beside_today));
        eprintln!("roads' luma over the land beside them: ours {contrast:.2}, today's {contrast_today:.2}");
        assert!((contrast - contrast_today).abs() <= 2.0, "roads' contrast {contrast:.2} against today's {contrast_today:.2}");
        assert!((0..3).all(|i| road[i].abs_diff(road_today[i]) <= 12), "roads {road:?} against today's {road_today:?}");
    }
}
