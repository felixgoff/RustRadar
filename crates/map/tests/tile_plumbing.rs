//! Item T1: a tile flows fetch → decode on the worker pool → upload within the frame's budget →
//! draw, in a headless shot, through the renderer's tile runtime and the layer API every tile layer
//! of stage 3 on uses (`MapRenderer::set_tiles`, `Init::tile_set`, `PrepareCx::update_tiles`,
//! `PrepareCx::tile_frame`, `TileSet::loading` as the layer's `pending`).
//!
//! The reference layer, [`Water`], draws the `water` polygons of OpenFreeMap's tiles (the E16
//! fixtures) with deck's tile rule for 512-px vector tiles, triangulated on the pool, in a blue no
//! map layer uses. At V3 (Stockholm, zoom 9, 1440 × 900) the blue must lie where today's app draws
//! its sea and lakes (`(30,30,30)` in `docs/port/reference/dark-v3-1440x900.png`) and not where it
//! draws land (`(50,50,50)`): that checks the tiles were fetched, decoded, uploaded and drawn, each
//! in its place.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

use common::fixture_tiles::FixtureTiles;
use common::shots::{self, Rect, Rgba, V3, V4, V5, gpu};
use rustradar_core::geo::Basemap;
use rustradar_core::mvt::{self, GeomType};
use rustradar_map::SceneInput;
use rustradar_map::frame::FrameSlot;
use rustradar_map::io;
use rustradar_map::jobs::Cancel;
use rustradar_map::layers::{ByteSize, DeckSelect, DrawCx, Init, Layer, PrepareCx, Source, TileIndex, TileSet, TileSetOptions};
use rustradar_map::pipelines::fill::{self, Fill};
use rustradar_map::state::DrawState;
use rustradar_map::tiles::{TileState, UPLOAD_TILES_PER_FRAME};
use rustradar_scene::camera::{ViewState, world_to_lng_lat};
use rustradar_scene::geom::earcut::earcut;
use rustradar_scene::tiles::VECTOR_TILES;

/// The reference layer's colour: no map layer draws it.
const BLUE: Rgba = [40, 90, 200, 255];
/// Today's sea and land on the dark map.
const SEA: Rgba = [30, 30, 30, 255];
const LAND: Rgba = [50, 50, 50, 255];

/// A tile's water, triangulated, in tile units (`u` east, `v` south, 0–1).
struct WaterTile {
    vertices: Vec<fill::Vertex>,
    indices: Vec<u32>,
}

impl ByteSize for WaterTile {
    fn byte_size(&self) -> usize {
        self.vertices.byte_size() + self.indices.byte_size()
    }
}

/// The surveyor's formula in tile coordinates (`y` down): positive for an outer ring (MVT 2.1,
/// 4.3.4.4), negative for a hole.
fn signed_area(ring: &[(f64, f64)]) -> f64 {
    let n = ring.len();
    (0..n).map(|i| ring[i].0 * ring[(i + 1) % n].1 - ring[(i + 1) % n].0 * ring[i].1).sum::<f64>() / 2.0
}

/// The decode step, on the pool: the tile's `water` polygons, each outer ring with its holes, cut
/// into triangles.
fn decode_water(bytes: &[u8], cancel: &Cancel) -> Result<Option<WaterTile>, String> {
    let Some(layer) = mvt::layer(bytes, "water") else { return Ok(None) };
    let extent = layer.extent;
    let mut tile = WaterTile { vertices: Vec::new(), indices: Vec::new() };
    let mut add = |polygon: &[Vec<(f64, f64)>]| {
        let mut positions = Vec::new();
        let mut holes = Vec::new();
        for (i, ring) in polygon.iter().enumerate() {
            if i > 0 {
                holes.push(positions.len() / 2);
            }
            positions.extend(ring.iter().flat_map(|&(x, y)| [x / extent, y / extent]));
        }
        let base = tile.vertices.len() as u32;
        tile.vertices.extend(positions.chunks_exact(2).map(|p| fill::Vertex { position: [p[0] as f32, p[1] as f32, 0.0], color: BLUE }));
        tile.indices.extend(earcut(&positions, &holes, 2).into_iter().map(|i| base + i as u32));
    };
    for feature in layer.features() {
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        if feature.kind != GeomType::Polygon {
            continue;
        }
        let mut polygon: Vec<Vec<(f64, f64)>> = Vec::new();
        for ring in feature.rings() {
            if signed_area(&ring) > 0.0 && !polygon.is_empty() {
                add(&polygon);
                polygon.clear();
            }
            polygon.push(ring);
        }
        if !polygon.is_empty() {
            add(&polygon);
        }
    }
    Ok((!tile.indices.is_empty()).then_some(tile))
}

/// What the layer saw, for the test to read after the shot.
#[derive(Default)]
struct Report {
    /// Tiles uploaded in each frame.
    uploads: Vec<usize>,
    /// The threads the decoding ran on.
    decoded_on: Vec<ThreadId>,
    /// The selection and its states at the last frame.
    selected: Vec<(TileIndex, Option<TileState>)>,
    /// Tiles drawn at the last frame.
    drawn: usize,
}

/// The reference tile layer: OpenFreeMap's water in blue.
struct Water {
    tiles: TileSet<WaterTile, fill::Mesh<fill::Vertex>>,
    fill: Fill,
    style: fill::Style,
    drawn: Vec<(FrameSlot, TileIndex)>,
    report: Arc<Mutex<Report>>,
}

impl Water {
    fn new(init: &Init<'_>, report: Arc<Mutex<Report>>) -> Self {
        let seen = Arc::clone(&report);
        let tiles = init.tile_set(vec![Source::new(io::OPENFREEMAP)], DeckSelect(VECTOR_TILES), TileSetOptions::default(), move |_tile, bytes, cancel| {
            seen.lock().unwrap().decoded_on.push(std::thread::current().id());
            decode_water(&bytes, cancel)
        });
        let fill = Fill::new(init, "reference water", DrawState::SURFACE_NO_CULL);
        let style = fill.style(&init.gpu.device, fill::OPAQUE);
        Water { tiles, fill, style, drawn: Vec::new(), report }
    }
}

impl Layer for Water {
    fn name(&self) -> &'static str {
        "reference-water"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        let mut uploaded = 0;
        cx.update_tiles(&mut self.tiles, |gpu, _tile, water| {
            uploaded += 1;
            fill::Mesh::new(&gpu.device, "reference water", &water.vertices, &water.indices)
        });
        self.drawn.clear();
        for (tile, _) in self.tiles.visible() {
            self.drawn.push((cx.tile_frame(tile, 0.0), tile));
        }
        let mut report = self.report.lock().unwrap();
        report.uploads.push(uploaded);
        report.selected = self.tiles.selected().iter().map(|&t| (t, self.tiles.state(t))).collect();
        report.drawn = self.drawn.len();
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        for &(slot, tile) in &self.drawn {
            if let Some(mesh) = self.tiles.get(tile) {
                self.fill.draw(pass, cx, slot, &self.style, mesh);
            }
        }
    }

    fn pending(&self) -> bool {
        self.tiles.loading()
    }
}

fn reference(name: &str) -> Option<rustradar_map::headless::Image> {
    common::read_png(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference").join(name))
}

#[test]
fn a_tile_is_fetched_decoded_on_the_pool_uploaded_and_drawn_in_a_shot() {
    let Some(gpu) = gpu() else { return };
    let fixtures = Arc::new(FixtureTiles::new());
    let report = Arc::new(Mutex::new(Report::default()));
    let mut scene = SceneInput::new(V3.view(), [1440.0, 900.0], 1.0, Basemap::Dark);
    scene.earth = rustradar_map::layers::earth::Input::SHOWN;
    let r = Arc::clone(&report);
    let (image, _) = shots::map_shot_with_tiles(&gpu, &scene, 1.0, fixtures.clone(), move |init| vec![Box::new(Water::new(init, r))]);
    shots::save(&image, "t1-tiles-water-v3.png");

    let report = report.lock().unwrap();
    // fetched: every tile the selection asked for is in the fixture set
    assert!(fixtures.misses().is_empty(), "the fixtures lack {:?}", fixtures.misses());
    // the shot waited until the selection had settled, each tile with its water or none
    assert!(report.selected.len() >= 6, "{} tiles selected", report.selected.len());
    assert!(report.selected.iter().all(|(t, s)| t.z == 9 && matches!(s, Some(TileState::Loaded | TileState::Empty))), "{:?}", report.selected);
    let loaded = report.selected.iter().filter(|(_, s)| *s == Some(TileState::Loaded)).count();
    assert!(loaded >= 6 && report.drawn >= loaded, "{loaded} loaded, {} drawn", report.drawn);
    // decoded on the pool, off the thread that drew
    let main = std::thread::current().id();
    assert!(report.decoded_on.len() >= report.selected.len() && report.decoded_on.iter().all(|&t| t != main));
    // uploaded within the frame's budget, over several frames
    assert_eq!(report.uploads.iter().sum::<usize>(), loaded);
    assert!(report.uploads.iter().all(|&n| n <= UPLOAD_TILES_PER_FRAME), "{:?}", report.uploads);
    assert!(report.uploads.iter().filter(|&&n| n > 0).count() >= loaded.div_ceil(UPLOAD_TILES_PER_FRAME), "{:?}", report.uploads);

    // drawn in place: the blue is today's water, the rest today's land. Today's pictures stand
    // about 4.5 px higher (and 0.5 px left) than deck's own viewport puts them, at 1440 × 900 and at
    // 390 × 844 alike, while this shot is within 0.3 px of `scene::camera` (deck's viewport to 1e-6
    // px): the best match is looked for within 6 px of each other and its shift printed.
    let Some(today) = reference("dark-v3-1440x900.png") else {
        panic!("docs/port/reference/dark-v3-1440x900.png is missing");
    };
    // the map between the sidebar and the map controls, under the mode bar and the dev readout
    let map = Rect { x0: 360, y0: 80, x1: 1370, y1: 880 };
    // each pixel as water (0), land (1) or neither (2), ours and today's
    let classes = |image: &rustradar_map::headless::Image, water: Rgba| -> Vec<u8> {
        image.rgba.chunks_exact(4).map(|p| if p == water { 0 } else if p == LAND { 1 } else { 2 }).collect()
    };
    let (ours, theirs) = (classes(&image, BLUE), classes(&today, SEA));
    let width = image.width as usize;
    let agreement = |dx: i32, dy: i32| {
        let (mut sea, mut sea_blue, mut land, mut land_left) = (0usize, 0usize, 0usize, 0usize);
        for (x, y) in map.pixels() {
            let mine = ours[(y as i32 + dy) as usize * width + (x as i32 + dx) as usize];
            match theirs[y as usize * width + x as usize] {
                0 => {
                    sea += 1;
                    sea_blue += usize::from(mine == 0);
                }
                1 => {
                    land += 1;
                    land_left += usize::from(mine == 1);
                }
                _ => {}
            }
        }
        assert!(sea > 100_000 && land > 100_000, "{sea} sea and {land} land pixels to compare");
        (sea_blue as f64 / sea as f64, land_left as f64 / land as f64)
    };
    let mut best = (0.0, 0.0, 0, 0);
    for dy in -6..=6 {
        for dx in -6..=6 {
            let (water, land) = agreement(dx, dy);
            if water + land > best.0 + best.1 {
                best = (water, land, dx, dy);
            }
        }
    }
    let (water_kept, land_kept, dx, dy) = best;
    let edges = map.pixels().filter(|&(x, y)| ours[y as usize * width + x as usize] == 2).count();
    let edge_share = edges as f64 / map.len() as f64;
    eprintln!(
        "today's water drawn blue: {water_kept:.4}, today's land left land: {land_kept:.4}, with this shot {dx},{dy} px from today's; \
         shore pixels (MSAA) {edge_share:.4}"
    );
    assert!(water_kept > 0.95, "only {water_kept} of today's water is drawn");
    assert!(land_kept > 0.95, "{} of today's land is drawn otherwise", 1.0 - land_kept);
    assert!(edge_share < 0.05, "{edge_share} of the pixels are neither blue nor land");
}

/// A small red square in a tile's frame ([`PrepareCx::tile_frame`]) around a point of the tile.
struct Mark {
    fill: Fill,
    style: fill::Style,
    mesh: fill::Mesh<fill::Vertex>,
    tile: TileIndex,
    slot: Option<FrameSlot>,
}

impl Mark {
    fn new(init: &Init<'_>, tile: TileIndex, [u, v]: [f32; 2], half: f32) -> Self {
        let corners = [[u - half, v - half], [u + half, v - half], [u + half, v + half], [u - half, v + half]];
        let vertices = corners.map(|[u, v]| fill::Vertex { position: [u, v, 0.0], color: [255, 0, 0, 255] });
        let fill = Fill::new(init, "mark", DrawState::SURFACE_NO_CULL);
        let style = fill.style(&init.gpu.device, fill::OPAQUE);
        let mesh = fill::Mesh::new(&init.gpu.device, "mark", &vertices, &[0, 1, 2, 0, 2, 3]);
        Mark { fill, style, mesh, tile, slot: None }
    }
}

impl Layer for Mark {
    fn name(&self) -> &'static str {
        "mark"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.slot = Some(cx.tile_frame(self.tile, 0.0));
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        self.fill.draw(pass, cx, self.slot.expect("prepared"), &self.style, &self.mesh);
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

/// A tile's content is drawn where the camera puts it: a point of a tile, drawn in the tile's
/// frame, lands within half a pixel of `scene::camera`'s projection of it, on the globe and on the
/// flat map, straight down and pitched.
#[test]
fn a_tile_frame_puts_its_points_where_the_camera_projects_them() {
    let Some(gpu) = gpu() else { return };
    let mut views = vec![V3.view(), V4.view(), V5.view()];
    views.push(ViewState { zoom: 5.0, ..V3.view() });
    views.push(ViewState { zoom: 11.9, pitch: 40.0, bearing: -25.0, ..V3.view() });
    for view in views {
        let scene = SceneInput::new(view, [800.0, 600.0], 1.0, Basemap::Dark);
        let tile = tile_under(&view, view.zoom.round() as u32);
        let point = [0.5f32, 0.5];
        // about 12 px across
        let half = (6.0 / 512.0 / 2f64.powf(view.zoom - f64::from(tile.z))) as f32;
        let (image, frame) = shots::map_shot(&gpu, &scene, 1.0, |init| vec![Box::new(Mark::new(init, tile, point, half))]);
        // the red's centroid, weighted by its coverage
        let (mut x, mut y, mut weight) = (0.0, 0.0, 0.0);
        for (px, py) in Rect::whole(&image).pixels() {
            let [r, g, ..] = shots::pixel(&image, px, py);
            if r > g {
                let w = f64::from(r - g) / 205.0;
                x += (f64::from(px) + 0.5) * w;
                y += (f64::from(py) + 0.5) * w;
                weight += w;
            }
        }
        assert!(weight > 50.0, "{view:?}: the mark covers {weight} px");
        let n = f64::from(1u32 << tile.z);
        let world = [(f64::from(tile.x) + 0.5) / n * 512.0, 512.0 - (f64::from(tile.y) + 0.5) / n * 512.0];
        let [lon, lat] = world_to_lng_lat(world);
        let [ex, ey, _] = frame.viewport.project([lon, lat, 0.0]);
        let error = (x / weight - ex).hypot(y / weight - ey);
        eprintln!("zoom {} pitch {}: drawn {:.2},{:.2}, projected {ex:.2},{ey:.2}: {error:.3} px", view.zoom, view.pitch, x / weight, y / weight);
        assert!(error < 0.5, "{view:?}: {error} px off");
    }
}
