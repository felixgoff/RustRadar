//! The 3D terrain surface (item Z1; doc 04 §14.1, §14.3; plan 1.4.12; row 2 `terrain-surface` of
//! doc 03 §3): today's `terrainSurfaceLayer` (`terrain-layer.ts:331-447`), a deck.gl
//! `TerrainLayer` whose tiles are picked by their distance from the camera (`LodTerrainLayer`),
//! drawn with the *terrain* setting on, on every basemap, on the globe as on the flat map (G:2639-
//! 2645: `relief = terrain`).
//!
//! What today's app does, and so this layer:
//! - **Tiles** ([`io::MAPTERHORN`], `MAPTERHORN_URL`): Mapterhorn's 512 px terrarium WebP, and
//!   where it has none (404) Mapzen's terrarium PNG of the same `z/x/y` (`terrainFetch`: the app's
//!   fetcher asks `backend::terrain`, which falls back; a `--fixture` folder answers what it holds).
//!   By distance ([`LodSelect`] with [`surface_lod`]: every zoom 0 to 16, bias 0.5, no farthest
//!   distance; 512 px tiles, `minZoom` 0) with the look-ahead while zooming, refined by
//!   `keepCoarserUntilIn` (`LodTileLayer`), at most 8 requests (`maxRequests: 8`), deck's default
//!   cache (five times the selection).
//! - **Elevation range** (`LodTerrainLayer.widen`): the heights the tiles' bounding volumes span
//!   start at sea level (none known) and widen as each tile loads to take in its mesh's bounding box
//!   (the ground's lowest and highest in it, the sea floor included); a wider range selects again.
//!   deck's own `onViewportLoad` widens the same way once every tile in view is in, which adds
//!   nothing to it. The layer leaving the list (the terrain off) forgets the range with its tiles.
//! - **Meshes** ([`mesh::decode`] on the worker pool): loaders.gl's `TerrainLoader` through
//!   `scene::terrain::martini`: the RTIN at `meshMaxError` 2 m on the back-filled `513²` (or
//!   `257²`) grid, the 4 m skirt, a pixel of overlap.
//! - **Drawing** ([`mesh::SurfacePipeline`]): `MESH_COLOR` `[34, 39, 48]` lit by the scene's rig
//!   (luma's default Phong material, which deck 9.4 draws `material: false` with) with flat-shaded
//!   normals, no vertex below the sea (`SeaLevelExtension`), depth written, both faces.
//!
//! Not here yet (the terrain items after Z1): what is draped over the surface, its cover textures
//! and the realistic map's air over it (`TERRAIN_AIR`, Z2); the 3D layers raised onto it (Z3); the
//! camera's target raised to the ground (Z4). Until Z2 draws the 2D layers into covers they draw in
//! the map pass as before, under the surface wherever it stands above the sea, so the app's scene
//! builder does not hand this layer the setting yet ([`Input::shown`] stays off in the app).
//!
//! Deviations: on the globe the meshes are in their Web Mercator frames (deck: degrees, rows evenly
//! in latitude; [`mesh`]); tiles neither source has are nothing there and settle (deck: a failed
//! tile), and either way a coarser tile that loaded stands in.
//!
//! Seen in the shots: the loader's 4 m skirts stand almost edge-on where a tile meets its
//! neighbour, and the rasteriser lets a sample of them through there now and then: a faint line
//! one pixel wide along a seam (10 levels darker on SwiftShader, seen from straight above; gone
//! without the skirts or with ten times the overlap). deck draws the same geometry with the same
//! depth test; Z2's draped colour on the skirts will show it the same way.

pub mod mesh;

use std::sync::{Arc, Mutex, PoisonError};

use rustradar_scene::camera::Viewport;
use rustradar_scene::geom::{js_max, js_min};
use rustradar_scene::hillshade::terrain_surface;
use rustradar_scene::lod::{Lod, Selection};
use rustradar_scene::terrain::martini::TerrainOptions;

use super::{DrawCx, Init, Layer, LodSelect, Pass, PrepareCx, Refinement, Select, Source, TileIndex, TileSet, TileSetOptions};
use crate::frame::FrameSlot;
use crate::io;
use mesh::{SurfaceGpu, SurfaceMesh, SurfacePipeline};

/// What the scene builder hands this layer (`SceneInput::terrain`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Input {
    /// The *terrain* setting: the surface is drawn (on any basemap, in both projections).
    pub shown: bool,
}

/// `SURFACE_LOD` (`terrain-layer.ts:375`): the surface's tiles by their distance from the camera,
/// every zoom from the layer's `minZoom` to its `maxZoom` (0 to 16, Mapterhorn's deepest), bias
/// 0.5, no farthest distance.
pub fn surface_lod() -> Lod {
    let surface = terrain_surface(&[]);
    Lod { levels: (surface.min_zoom..=surface.max_zoom).collect(), bias: surface.lod_bias, max_distance: f64::INFINITY }
}

/// The elevation range a tile's bounding box widens `range` to (`LodTerrainLayer.widen`): `None`
/// when it holds the box already.
pub fn widen(range: Option<[f64; 2]>, [low, high]: [f64; 2]) -> Option<[f64; 2]> {
    match range {
        Some([lo, hi]) if !(low < lo || high > hi) => None,
        _ => Some([js_min(low, range.map_or(f64::INFINITY, |r| r[0])), js_max(high, range.map_or(f64::NEG_INFINITY, |r| r[1]))]),
    }
}

/// The distance selection with the layer's elevation range now (`zRange`, shared with the layer,
/// which widens it as tiles load).
struct SurfaceSelect {
    lod: LodSelect,
    z_range: Arc<Mutex<Option<[f64; 2]>>>,
}

impl Select for SurfaceSelect {
    fn select(&mut self, viewport: &Viewport, cached: &dyn Fn(TileIndex) -> bool) -> Selection {
        self.lod.z_range = *self.z_range.lock().unwrap_or_else(PoisonError::into_inner);
        self.lod.select(viewport, cached)
    }

    fn is_ahead(&self, tile: TileIndex) -> bool {
        self.lod.is_ahead(tile)
    }

    fn min_zoom(&self) -> u32 {
        self.lod.min_zoom()
    }
}

/// Row 2: `terrain-surface`.
pub struct Terrain {
    tiles: TileSet<SurfaceMesh, SurfaceGpu>,
    /// `zRange`: the ground's lowest and highest heights of the tiles loaded, metres.
    z_range: Arc<Mutex<Option<[f64; 2]>>>,
    /// The range widened since the last selection: the next update selects again.
    reselect: bool,
    pipeline: SurfacePipeline,
    /// This frame's tiles, each with its frame.
    slots: Vec<(FrameSlot, TileIndex)>,
    shown: bool,
}

impl Terrain {
    pub fn new(init: &Init<'_>) -> Self {
        let surface = terrain_surface(&[]);
        let options: TerrainOptions = mesh::loader_options(&surface);
        let z_range = Arc::new(Mutex::new(None));
        let mut lod = LodSelect::new(surface_lod(), surface.tile_size as f64);
        lod.min_zoom = Some(f64::from(surface.min_zoom));
        let tiles = init.tile_set(
            vec![Source::new(io::MAPTERHORN)],
            SurfaceSelect { lod, z_range: Arc::clone(&z_range) },
            TileSetOptions { max_requests: surface.max_requests, refinement: Refinement::KeepCoarserUntilIn, ..Default::default() },
            move |tile, bytes, cancel| mesh::decode(tile, &bytes, cancel, &options),
        );
        Terrain { tiles, z_range, reselect: false, pipeline: SurfacePipeline::new(init), slots: Vec::new(), shown: false }
    }

    /// The tile set (for the terrain items after this one, and tests).
    pub fn tiles(&self) -> &TileSet<SurfaceMesh, SurfaceGpu> {
        &self.tiles
    }

    /// The elevation range now (`zRange`), metres; `None` until a tile has loaded.
    pub fn z_range(&self) -> Option<[f64; 2]> {
        *self.z_range.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the last frame drew the surface.
    pub fn shown(&self) -> bool {
        self.shown
    }

    /// This frame's tiles in draw order, each with its frame.
    pub fn slots(&self) -> &[(FrameSlot, TileIndex)] {
        &self.slots
    }
}

impl Layer for Terrain {
    fn name(&self) -> &'static str {
        "terrain"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.slots.clear();
        self.shown = cx.scene.terrain.shown;
        if !self.shown {
            // deck finalizes a layer that leaves the list: its tiles and its range go
            if !self.tiles.is_empty() {
                self.tiles.clear();
            }
            *self.z_range.lock().unwrap_or_else(PoisonError::into_inner) = None;
            self.reselect = false;
            return;
        }
        if std::mem::take(&mut self.reselect) {
            self.tiles.reselect();
        }
        let mut range = self.z_range();
        let mut widened = false;
        cx.update_tiles(&mut self.tiles, |gpu, _tile, mesh| {
            // `onTileLoad`: the range takes in each tile as it comes
            if let Some(wider) = widen(range, mesh.z_range) {
                range = Some(wider);
                widened = true;
            }
            SurfaceGpu::new(gpu, &mesh)
        });
        if widened {
            *self.z_range.lock().unwrap_or_else(PoisonError::into_inner) = range;
            self.reselect = true;
        }
        self.pipeline.prepare(cx);
        for (tile, _) in self.tiles.visible() {
            self.slots.push((cx.tile_frame(tile, 0.0), tile));
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if !self.shown || cx.pass != Pass::Map || self.slots.is_empty() {
            return;
        }
        let tiles = self.slots.iter().filter_map(|&(slot, tile)| Some((slot, self.tiles.get(tile)?)));
        self.pipeline.draw(pass, cx, tiles);
    }

    fn pending(&self) -> bool {
        self.shown && (self.reselect || self.tiles.loading())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use glam::DVec3;
    use rustradar_core::geo::Basemap;
    use rustradar_core::weather::PRESETS;
    use rustradar_scene::camera::ViewState;
    use rustradar_scene::hillshade::MESH_COLOR;

    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{DirTiles, FetchResult, FnFetcher, Held};
    use crate::layers::sky;
    use crate::pipelines::mesh::{Material, render_turn};
    use crate::renderer::{MapRenderer, SceneInput};

    /// `SKYLINE_VIEWS`' Zermatt skyline (X1): looking north down the Mattertal at pitch 75 from
    /// above Zermatt, the target raised to the ground there (1608 m, as the terrain's camera aim
    /// has it, Z4), on the 800 × 500 canvas the fixtures were fetched for.
    const SKYLINE: ViewState =
        ViewState { latitude: 46.0207, longitude: 7.7491, zoom: 16.0, pitch: 75.0, bearing: 0.0, position: Some([0.0, 0.0, 1608.0]) };
    const SKYLINE_SIZE: [f64; 2] = [800.0, 500.0];
    /// 2026-06-21 10:00 UTC (the reference pictures' summer morning): noon in Zermatt.
    const MIDSUMMER: f64 = 1_782_036_000_000.0;

    fn shots_dir() -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    /// Terrarium PNG pixels for heights in metres (to 1/256 m).
    fn terrarium_png(size: u32, metres: impl Fn(u32, u32) -> f64) -> Vec<u8> {
        let image = image::RgbImage::from_fn(size, size, |x, y| {
            let v = ((metres(x, y) + 32768.0) * 256.0).round() as u32;
            image::Rgb([(v >> 16) as u8, (v >> 8) as u8, v as u8])
        });
        let mut png = Vec::new();
        image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("a PNG");
        png
    }

    /// A fetcher whose every Mapterhorn tile is the same 256 px picture of `metres`, nothing else.
    fn every_tile(metres: impl Fn(u32, u32) -> f64 + Send + Sync + 'static) -> Arc<FnFetcher<impl Fn(&str, TileIndex) -> FetchResult + Send + Sync>> {
        let png = terrarium_png(256, metres);
        Arc::new(FnFetcher(move |source: &str, _tile: TileIndex| Ok((source == io::MAPTERHORN).then(|| png.clone()))))
    }

    /// The surface alone (nothing else on the map) of `view` on a `size` canvas.
    fn surface_scene(view: ViewState, size: [f64; 2], basemap: Basemap, shown: bool) -> SceneInput {
        let mut scene = SceneInput::new(view, size, 1.0, basemap);
        scene.local_now_ms = MIDSUMMER;
        scene.feed_now_ms = MIDSUMMER;
        scene.terrain = Input { shown };
        scene
    }

    /// A terrain layer of the renderer's own, prepared with the others (not drawn: the map's own
    /// draws), to read its tiles and range after a shot.
    struct Watch(Rc<RefCell<Terrain>>);

    impl Layer for Watch {
        fn name(&self) -> &'static str {
            "terrain watch"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.0.borrow_mut().prepare(cx);
        }

        fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}

        fn pending(&self) -> bool {
            self.0.borrow().pending()
        }
    }

    fn watched(renderer: &mut MapRenderer, gpu: &crate::Gpu) -> Rc<RefCell<Terrain>> {
        let terrain = Rc::new(RefCell::new(Terrain::new(&renderer.init(gpu))));
        renderer.add_layer(Box::new(Watch(Rc::clone(&terrain))));
        terrain
    }

    fn luma(p: [u8; 4]) -> f64 {
        0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2])
    }

    // ------------------------------------------------------------------ the CPU side

    /// `SURFACE_LOD` and the layer's settings (`terrainSurfaceLayer`).
    #[test]
    fn the_layer_is_todays() {
        let lod = surface_lod();
        assert_eq!(lod.levels, (0..=16).collect::<Vec<u32>>());
        assert_eq!((lod.bias, lod.max_distance), (0.5, f64::INFINITY));
        let surface = terrain_surface(&[]);
        assert_eq!((surface.tile_size, surface.max_requests, surface.mesh_max_error), (512, 8, 2.0));
        assert_eq!(surface.color, [34, 39, 48]);
        assert_eq!(Input::default(), Input { shown: false });
    }

    /// `LodTerrainLayer.widen`: the first box is the range; one inside it changes nothing; one
    /// reaching past either end widens that end.
    #[test]
    fn the_range_widens_with_each_tile() {
        assert_eq!(widen(None, [1200.0, 3400.0]), Some([1200.0, 3400.0]));
        assert_eq!(widen(Some([1200.0, 3400.0]), [1500.0, 3000.0]), None);
        assert_eq!(widen(Some([1200.0, 3400.0]), [1200.0, 3400.0]), None);
        assert_eq!(widen(Some([1200.0, 3400.0]), [-20.0, 900.0]), Some([-20.0, 3400.0]));
        assert_eq!(widen(Some([1200.0, 3400.0]), [2000.0, 4634.0]), Some([1200.0, 4634.0]));
        assert_eq!(widen(Some([1200.0, 3400.0]), [f64::NAN, f64::NAN]), None);
    }

    // ------------------------------------------------------------------ on the GPU

    /// The surface's colour as deck lights it: flat ground seen from straight above is
    /// `MESH_COLOR` lit by the rig (luma's default Phong, the camera light's highlight and the key
    /// light) as the CPU's twin shades it with the up normal; a slope falling to the east is lit as
    /// its own normal is (the flat-shading normals face the right way); nothing of the layer is
    /// drawn with the terrain off.
    #[test]
    fn the_surface_is_lit_as_the_rig_lights_it() {
        let Some(gpu) = gpu() else { return };
        let view = ViewState { latitude: 46.0, longitude: 7.75, zoom: 13.0, pitch: 0.0, bearing: 0.0, position: None };
        let size = [400.0, 300.0];
        let frame = ViewFrame::new(&view, size, 1.0, 0.0);
        let upm = frame.viewport.distance_scales.units_per_meter[2];
        let camera = frame.viewport.camera_position - frame.viewport.center;
        let scene = surface_scene(view, size, Basemap::Dark, true);
        let key = render_turn(&frame) * DVec3::from_array(scene.lights.key_direction);
        let surface = MESH_COLOR.map(|c| f64::from(c) / 255.0);
        let twin = |normal: DVec3, height: f64| {
            let rgb = scene.lights.shade(&Material::DECK_DEFAULT, surface, normal.normalize(), camera - DVec3::new(0.0, 0.0, height * upm), key);
            rgb.map(|c| (c * 255.0).round())
        };
        let centre = |image: &Image| image.pixel(200, 150);
        let near = |got: [u8; 4], want: [f64; 3]| (0..3).all(|c| (f64::from(got[c]) - want[c]).abs() <= 2.0);

        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(every_tile(|_, _| 500.0), None);
        let flat = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        let want = twin(DVec3::Z, 500.0);
        eprintln!("flat: {:?} against the twin's {want:?}", centre(&flat));
        assert!(near(centre(&flat), want), "flat {:?} against {want:?}", centre(&flat));
        flat.save_png(&shots_dir().join("z1-flat-from-above.png")).expect("saved");

        // 10 m a pixel down to the east, 2.6 to 4 km high: the camera, 6 km up, is that much closer,
        // so the tile under the centre is finer than the view's zoom
        let mut sloped = MapRenderer::new(&gpu, FORMAT);
        sloped.set_tiles(every_tile(|x, _| 4000.0 - 10.0 * f64::from(x)), None);
        let watch = watched(&mut sloped, &gpu);
        let image = render_with(&gpu, &mut sloped, &scene, 1.0).expect("rendered");
        let tiles: Vec<TileIndex> = watch.borrow().tiles().visible().map(|(t, _)| t).collect();
        let world = rustradar_scene::camera::lng_lat_to_world([view.longitude, view.latitude]);
        let at = tiles.iter().find(|t| {
            let extent = 512.0 / f64::from(1u32 << t.z);
            let (u, v) = (world[0] / extent - f64::from(t.x), (512.0 - world[1]) / extent - f64::from(t.y));
            (0.0..1.0).contains(&u) && (0.0..1.0).contains(&v)
        });
        let tile = *at.expect("a tile under the centre");
        assert!(tile.z > 13, "{tile:?}");
        let extent = 512.0 / f64::from(1u32 << tile.z);
        let pad = 1.0 / 512.0;
        // metres a tile unit and the height under the centre, as the mesh has them
        let dh_du = -10.0 * 256.0 / (1.0 + 2.0 * pad);
        let u = world[0] / extent - f64::from(tile.x);
        let height = 4000.0 + dh_du * (u + pad);
        let normal = DVec3::new(-dh_du * upm / extent, 0.0, 1.0);
        let want = twin(normal, height);
        eprintln!("slope: {:?} against the twin's {want:?} (flat {:?})", centre(&image), centre(&flat));
        assert!(near(centre(&image), want), "slope {:?} against {want:?}", centre(&image));
        assert!(luma(centre(&image)) < luma(centre(&flat)) - 5.0, "a slope facing away from the key light is darker");

        // the layer off draws nothing of it
        let off = render_with(&gpu, &mut renderer, &surface_scene(view, size, Basemap::Dark, false), 1.0).expect("rendered");
        assert!(off.rgba.chunks_exact(4).all(|p| p == off.rgba[..4].as_ref()), "the clear colour alone");
    }

    /// `SeaLevelExtension`: the sea floor is drawn at sea level, so a tile 300 m below the sea
    /// draws as one at sea level does (pitched, where heights move the picture), and one 300 m
    /// above it does not.
    #[test]
    fn nothing_is_drawn_below_the_sea() {
        let Some(gpu) = gpu() else { return };
        let view = ViewState { latitude: 46.0, longitude: 7.75, zoom: 13.0, pitch: 50.0, bearing: 20.0, position: None };
        let scene = surface_scene(view, [400.0, 300.0], Basemap::Dark, true);
        let shot = |metres: f64| {
            let mut renderer = MapRenderer::new(&gpu, FORMAT);
            renderer.set_tiles(every_tile(move |x, y| metres + f64::from((x + y) % 2) * 0.0), None);
            render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered")
        };
        let (sea, floor, hill) = (shot(0.0), shot(-300.0), shot(300.0));
        assert!(sea == floor, "the sea floor is raised to the sea");
        assert!(sea != hill);
    }

    /// The acceptance shot: the Zermatt skyline from the committed fixtures on the realistic map,
    /// the surface over the sky (X1's view: pitch 75, as pitch 60 shows no sky at all). The camera
    /// stands 322 m over Zermatt, so the Mattertal's walls rise over the top of the picture all
    /// round and the sky shows only down the valley to the north-north-east: every column has
    /// ground, in every one it rises above where a plain at Zermatt's height (a flat tile at 1608 m
    /// everywhere, whose straight horizon is where the far plane cuts it) meets the sky, and where
    /// the sky shows its lower edge is the ridges' jagged line, the sky above it untouched. The
    /// elevation range grew from the tiles' meshes past 4400 m (Lake Geneva's 396 m to 4501 m), and
    /// the tiles selected with it are the fixtures' (X1 chose them for 0 to 4634 m).
    #[test]
    fn the_alps_stand_against_the_sky() {
        let Some(gpu) = gpu() else { return };
        let mut scene = surface_scene(SKYLINE, SKYLINE_SIZE, Basemap::Realistic, true);
        scene.sky = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
        let fixtures = Arc::new(DirTiles::fixtures());
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(fixtures.clone(), None);
        let watch = watched(&mut renderer, &gpu);
        let alps = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        alps.save_png(&shots_dir().join("z1-skyline-realistic.png")).expect("saved");
        let sky_only = {
            let mut scene = scene.clone();
            scene.terrain = Input { shown: false };
            render_with(&gpu, &mut MapRenderer::new(&gpu, FORMAT), &scene, 1.0).expect("rendered")
        };
        let plain = {
            let mut renderer = MapRenderer::new(&gpu, FORMAT);
            renderer.set_tiles(every_tile(|_, _| 1608.0), None);
            render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered")
        };
        plain.save_png(&shots_dir().join("z1-skyline-plain.png")).expect("saved");

        let [w, h] = [SKYLINE_SIZE[0] as u32, SKYLINE_SIZE[1] as u32];
        // the first row from the top that is not the sky alone
        let top = |image: &Image| -> Vec<u32> { (0..w).map(|x| (0..h).find(|&y| image.pixel(x, y) != sky_only.pixel(x, y)).unwrap_or(h)).collect() };
        let (ridge, horizon) = (top(&alps), top(&plain));
        assert!(ridge.iter().all(|&y| y < h), "ground in every column");
        let (low, high) = (*horizon.iter().min().unwrap(), *horizon.iter().max().unwrap());
        assert!(high - low <= 1 && low > 20, "the plain's horizon is straight: rows {low} to {high}");
        let above = ridge.iter().filter(|&&y| y + 10 < low).count();
        eprintln!("the plain meets the sky at row {low}; the Alps above it by 10 px or more in {above} of {w} columns");
        assert_eq!(above, w as usize, "the valley's walls rise above the plain's horizon");
        // where the sky shows: a gap some 90 px wide, its lower edge the ridges' line
        let open: Vec<u32> = (0..w).filter(|&x| ridge[x as usize] > 0).collect();
        let sky_pixels: u32 = ridge.iter().sum();
        let edge: Vec<u32> = open.iter().map(|&x| ridge[x as usize]).collect();
        let (shallow, deep) = (*edge.iter().min().unwrap(), *edge.iter().max().unwrap());
        // the edge's ups and downs, level runs taken as one
        let mut steps = edge.clone();
        steps.dedup();
        let turns = steps.windows(3).filter(|e| (e[1] > e[0] && e[1] > e[2]) || (e[1] < e[0] && e[1] < e[2])).count();
        eprintln!("the sky in {} columns ({} to {}), {sky_pixels} px; its edge rows {shallow} to {deep}, {turns} turns", open.len(), open[0], open[open.len() - 1]);
        assert!(open.len() >= 60 && sky_pixels >= 600, "the sky down the valley: {} columns, {sky_pixels} px", open.len());
        assert!(open.windows(2).all(|c| c[1] == c[0] + 1), "one gap");
        assert!(deep - shallow >= 8 && turns >= 4, "a jagged ridge line: rows {shallow} to {deep}, {turns} turns");
        for &x in &open {
            for y in 0..ridge[x as usize] {
                assert_eq!(alps.pixel(x, y), sky_only.pixel(x, y), "the sky at {x}, {y}");
            }
        }
        // the ground in the mesh colour, lit: far darker than the sky
        let mean = |pixels: &mut dyn Iterator<Item = [u8; 4]>| {
            let (sum, n) = pixels.fold((0.0, 0usize), |(s, n), p| (s + luma(p), n + 1));
            sum / n as f64
        };
        let sky_luma = mean(&mut open.iter().flat_map(|&x| (0..ridge[x as usize]).map(move |y| (x, y))).map(|(x, y)| alps.pixel(x, y)));
        let ground_luma = mean(&mut (0..w).flat_map(|x| (ridge[x as usize] + 2..h).map(move |y| (x, y))).map(|(x, y)| alps.pixel(x, y)));
        eprintln!("sky luma {sky_luma:.1}, ground luma {ground_luma:.1}");
        assert!(ground_luma + 60.0 < sky_luma, "ground {ground_luma} against the sky {sky_luma}");

        let terrain = watch.borrow();
        let range = terrain.z_range().expect("tiles loaded");
        eprintln!("elevation range {range:?}, {} tiles drawn", terrain.tiles().visible().count());
        assert!(range[0] < 1608.0 && range[1] > 4400.0, "{range:?}");
        let selected = terrain.tiles().selected();
        let held = selected.iter().filter(|t| fixtures.held(io::MAPTERHORN, **t) != Held::Missing).count();
        eprintln!("{held} of {} selected tiles held by the fixtures", selected.len());
        assert_eq!(held, selected.len(), "the settled selection is the fixtures'");
        assert!(terrain.tiles().visible().all(|(t, _)| t.z <= 16));
    }
}
