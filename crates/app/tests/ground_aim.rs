//! Item Z4's acceptance: the camera's aim on the ground under it at a mountain village
//! (`TERRAIN_VIEWS`: Zermatt, 46.0207 N, 7.7491 E, zoom 16, pitched 60°, 800 × 500) from the
//! committed tiles (Mapterhorn's zoom-12 ground tile and the surface's zooms 11 to 16).
//!
//! - **The aim.** With the terrain on, the app's camera state takes the ground tile from the
//!   fixtures and its target's height eases in 23 frames of 30 % to within half a metre of the
//!   tile's pixel under the centre (1603 m; X1 recorded Zermatt's round 1608 m for the view);
//!   with it off the target stays on the sea.
//! - **The same ground point at the centre, terrain on and off, within 3 px.** Off, the flat map's
//!   centre is the view's longitude and latitude by construction. On, the centre pixel's ray from
//!   the raised camera is cast onto the 3D surface's own triangles (Z1's RTIN mesh of the tile the
//!   surface draws there, decoded from the fixtures as the layer decodes it: the CPU's account of
//!   the depth the GPU writes); where it meets the ground, projected into the terrain-off view,
//!   lies within 3 px of its centre. A target 30 m off the ground misses by far more, so the
//!   check tells a good aim from a bad one.
//! - **Shots** (`target/shots/z4-*.png`): the surface from the settled camera (the scene the app
//!   builds, with the surface the app does not hand the map yet: stage 9's draping, Z2), the same
//!   from a camera left on the sea (inside the mountain), and the whole app's terrain-off and
//!   terrain-on pictures through `--screenshot`'s path (whose loop waits for the aim to rest).
//!
//! The GPU parts need a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without
//! one they print `skipped: …` and pass. The aim and the ray cast against the finest tile run
//! without one.

mod common;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use common::shots::{gpu, save};
use common::{NOW_MS, app_shot_with, options};
use rustradar_app::headless;
use rustradar_app::scene_builder::SceneBuilder;
use rustradar_app::state::{AppState, Startup};
use rustradar_core::geo::Basemap;
use rustradar_map::MapRenderer;
use rustradar_map::headless::{FORMAT, render_with};
use rustradar_map::io::{self, DirTiles, Fetch, Held};
use rustradar_map::jobs::{Cancel, Pool};
use rustradar_map::layers::terrain::mesh::{SurfaceMesh, decode, loader_options};
use rustradar_map::layers::terrain::{self, Terrain};
use rustradar_map::layers::{DrawCx, Layer, PrepareCx};
use rustradar_map::tiles::TileRuntime;
use rustradar_scene::camera::{ViewState, viewport_for};
use rustradar_scene::hillshade::{decode_terrarium, terrain_surface};
use rustradar_scene::tiles::TileIndex;

/// `TERRAIN_VIEWS`' Zermatt.
const ZERMATT: ViewState = ViewState { latitude: 46.0207, longitude: 7.7491, zoom: 16.0, pitch: 60.0, bearing: 0.0, position: None };
const SIZE: [u32; 2] = [800, 500];
/// The ground under Zermatt's centre as X1 recorded it (`View::ground`: the village's round
/// figure), metres.
const GROUND_M: f64 = 1608.0;
/// The acceptance's bound, CSS pixels.
const WITHIN_PX: f64 = 3.0;

fn size() -> [f64; 2] {
    [f64::from(SIZE[0]), f64::from(SIZE[1])]
}

/// The app's state at Zermatt, the terrain `terrain`, its ground's tiles from the committed
/// fixtures, after frames 50 ms apart (the scene refresh's) until the aim rests: the state and
/// how many frames moved the aim.
fn settled(terrain: bool) -> (AppState, usize) {
    let mut state = AppState::new(Startup::default(), None, None, None, ZERMATT, size());
    state.persisted.settings.terrain = terrain;
    let runtime = TileRuntime::new(Arc::new(DirTiles::fixtures()), Arc::new(Pool::new(2)));
    state.camera.set_ground_tiles(runtime.clone());
    let (mut t, mut moves) = (NOW_MS, 0);
    loop {
        runtime.pool.wait_idle();
        let (moved, _) = state.advance_camera(t);
        moves += usize::from(moved);
        if !state.camera.aiming() {
            return (state, moves);
        }
        assert!(moves < 200, "the aim never rests");
        t += 50.0;
    }
}

/// A tile's surface as the layer draws it: the RTIN's own triangles (the skirt's left out) in the
/// tile's frame, bucketed on a grid for lookups.
struct Surface {
    tile: TileIndex,
    mesh: SurfaceMesh,
    cells: Vec<Vec<usize>>,
}

/// The lookup grid's cells a side.
const CELLS: usize = 64;

impl Surface {
    /// The fixtures' Mapterhorn tile `tile`, decoded as the terrain layer decodes it.
    fn of(tile: TileIndex) -> Surface {
        let mut fetch = Fetch::start(&DirTiles::fixtures(), Arc::from(io::MAPTERHORN), tile);
        let bytes = fetch.poll().expect("a folder answers at once").expect("read").expect("the fixtures hold it");
        let mesh = decode(tile, &bytes, &Cancel::new(), &loader_options(&terrain_surface(&[]))).expect("decoded").expect("a mesh");
        let mut cells = vec![Vec::new(); CELLS * CELLS];
        for t in 0..mesh.surface_triangles {
            let corners = Self::corners(&mesh, t);
            let (lo, hi) = (|a: usize| corners.iter().map(|c| c[a]).fold(f64::INFINITY, f64::min), |a: usize| corners.iter().map(|c| c[a]).fold(f64::NEG_INFINITY, f64::max));
            let cell = |x: f64| ((x * CELLS as f64).floor().max(0.0) as usize).min(CELLS - 1);
            for cy in cell(lo(1))..=cell(hi(1)) {
                for cx in cell(lo(0))..=cell(hi(0)) {
                    cells[cy * CELLS + cx].push(t);
                }
            }
        }
        Surface { tile, mesh, cells }
    }

    fn corners(mesh: &SurfaceMesh, t: usize) -> [[f64; 3]; 3] {
        let v = |k: usize| mesh.vertices[mesh.indices[3 * t + k] as usize].map(f64::from);
        [v(0), v(1), v(2)]
    }

    /// `[u, v]` of a place in the tile's frame (Web Mercator, `v` south, tile units).
    fn uv(&self, lon: f64, lat: f64) -> [f64; 2] {
        let [x, y] = tile_xy(lon, lat, self.tile.z);
        [x - f64::from(self.tile.x), y - f64::from(self.tile.y)]
    }

    /// The drawn surface's height at a place, metres (raised to the sea as the shader raises
    /// it); `None` off the tile.
    fn height(&self, lon: f64, lat: f64) -> Option<f64> {
        let [u, v] = self.uv(lon, lat);
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return None;
        }
        let cell = ((v * CELLS as f64) as usize) * CELLS + (u * CELLS as f64) as usize;
        self.cells[cell].iter().find_map(|&t| {
            let [a, b, c] = Self::corners(&self.mesh, t);
            let det = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
            if det.abs() < 1e-18 {
                return None;
            }
            let l1 = ((b[1] - c[1]) * (u - c[0]) + (c[0] - b[0]) * (v - c[1])) / det;
            let l2 = ((c[1] - a[1]) * (u - c[0]) + (a[0] - c[0]) * (v - c[1])) / det;
            let l3 = 1.0 - l1 - l2;
            let inside = [l1, l2, l3].iter().all(|&l| l >= -1e-9);
            inside.then(|| (l1 * a[2] + l2 * b[2] + l3 * c[2]).max(0.0))
        })
    }
}

/// The surface over several tiles: at each place the tile `pick` names, decoded when first needed.
struct Ground {
    pick: Box<dyn Fn(f64, f64) -> Option<TileIndex>>,
    decoded: RefCell<HashMap<TileIndex, Rc<Surface>>>,
}

impl Ground {
    /// The fixtures' finest tiles, zoom 16, wherever the ray goes.
    fn finest() -> Ground {
        Ground::by(|lon, lat| {
            let [x, y] = tile_xy(lon, lat, 16);
            Some(TileIndex::new(x.floor() as u32, y.floor() as u32, 16))
        })
    }

    /// The tiles the surface draws (`drawn`); nothing where none of them is.
    fn drawn(drawn: Vec<TileIndex>) -> Ground {
        Ground::by(move |lon, lat| drawn.iter().copied().find(|&t| covers(t, lon, lat)))
    }

    fn by(pick: impl Fn(f64, f64) -> Option<TileIndex> + 'static) -> Ground {
        Ground { pick: Box::new(pick), decoded: RefCell::new(HashMap::new()) }
    }

    /// The tile under a place.
    fn tile(&self, lon: f64, lat: f64) -> Option<TileIndex> {
        (self.pick)(lon, lat)
    }

    /// The drawn surface's height at a place, metres; `None` where no tile is.
    fn height(&self, lon: f64, lat: f64) -> Option<f64> {
        let tile = self.tile(lon, lat)?;
        let surface = Rc::clone(self.decoded.borrow_mut().entry(tile).or_insert_with(|| Rc::new(Surface::of(tile))));
        surface.height(lon, lat)
    }
}

/// Where the ray of `view`'s centre pixel first meets `ground`: `[lon, lat, metres]`. The ray is
/// walked down from the camera in metre steps of height, then the crossing is halved down to a
/// millimetre.
fn centre_ground(view: &ViewState, ground: &Ground) -> [f64; 3] {
    let viewport = viewport_for(view, size());
    let centre = [size()[0] / 2.0, size()[1] / 2.0];
    let camera = viewport.unproject_position(viewport.camera_position);
    // whether the ray at height `h` is above the ground under it (or over no tile)
    let above = |h: f64| {
        let [lon, lat] = viewport.unproject(centre, h);
        ground.height(lon, lat).is_none_or(|g| h > g)
    };
    let mut high = camera[2] - 1.0;
    assert!(above(high), "the camera is above the ground: {camera:?}");
    let mut low = high - 1.0;
    while above(low) {
        high = low;
        low -= 1.0;
        assert!(low > camera[2] - 5000.0, "the ray never meets the ground");
    }
    while high - low > 1e-3 {
        let mid = (high + low) / 2.0;
        if above(mid) { high = mid } else { low = mid }
    }
    let [lon, lat] = viewport.unproject(centre, low);
    assert!(ground.height(lon, lat).is_some(), "the ray meets a tile");
    [lon, lat, low]
}

/// How far from the centre of the terrain-off view (the flat map at sea level) the ground point
/// at the centre of `view` lies, pixels.
fn off_centre_px(view: &ViewState, ground: &Ground) -> f64 {
    let [lon, lat, _] = centre_ground(view, ground);
    let [x, y, _] = viewport_for(&ZERMATT, size()).project([lon, lat, 0.0]);
    (x - size()[0] / 2.0).hypot(y - size()[1] / 2.0)
}

/// A place in Web Mercator tile units at zoom `z`.
fn tile_xy(lon: f64, lat: f64, z: u32) -> [f64; 2] {
    let n = 2f64.powi(z as i32);
    let r = lat.to_radians();
    [(lon + 180.0) / 360.0 * n, (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0 * n]
}

/// Whether `tile` covers a place.
fn covers(tile: TileIndex, lon: f64, lat: f64) -> bool {
    let [x, y] = tile_xy(lon, lat, tile.z);
    x.floor() == f64::from(tile.x) && y.floor() == f64::from(tile.y)
}

/// The ground under Zermatt's centre as `groundHeight` reads it: the nearest pixel of the
/// fixtures' zoom-12 Mapterhorn tile there, metres.
fn zermatts_ground_pixel() -> f64 {
    let [x, y] = tile_xy(ZERMATT.longitude, ZERMATT.latitude, 12);
    let tile = TileIndex::new(x.floor() as u32, y.floor() as u32, 12);
    let mut fetch = Fetch::start(&DirTiles::fixtures(), Arc::from(io::MAPTERHORN), tile);
    let bytes = fetch.poll().expect("a folder answers at once").expect("read").expect("the fixtures hold the ground tile");
    let picture = rustradar_backend::images::decode(&bytes).expect("a picture");
    assert_eq!((picture.width, picture.height), (512, 512));
    let heights = decode_terrarium(&picture.rgba, None);
    let (px, py) = (((x % 1.0) * 512.0).floor() as usize, ((y % 1.0) * 512.0).floor() as usize);
    f64::from(heights[py * 512 + px]).max(0.0)
}

#[test]
fn zermatts_camera_aims_at_its_ground_with_the_terrain_on() {
    let (on, moves) = settled(true);
    let target = on.controller.view().position.expect("raised")[2];
    let ground = zermatts_ground_pixel();
    eprintln!("the target rests at {target:.3} m after {moves} frames; the ground tile's pixel {ground:.3} m (X1's round figure {GROUND_M} m)");
    assert!((target - ground).abs() <= 0.5, "{target} against {ground}");
    assert!((ground - GROUND_M).abs() < 10.0, "{ground}");
    assert!((22..=23).contains(&moves), "{moves}");
    assert_eq!(ViewState { position: None, ..on.controller.view() }, ZERMATT, "only the height moved");
    assert_eq!(on.camera.ground().kept(), 1);
    // the terrain off: the target on the sea, nothing asked
    let (off, moves) = settled(false);
    assert_eq!((moves, off.controller.view(), off.camera.ground().kept()), (0, ZERMATT, 0));
}

#[test]
fn the_view_centre_is_the_same_ground_point_with_the_terrain_on_and_off() {
    let (on, _) = settled(true);
    let raised = on.controller.view();
    let surface = Ground::finest();
    let ground = centre_ground(&raised, &surface);
    let off = off_centre_px(&raised, &surface);
    let target = raised.position.unwrap()[2];
    eprintln!("from the raised camera ({target:.2} m) the centre meets the ground at {ground:?}, {off:.2} px from the flat map's centre");
    assert!(off <= WITHIN_PX, "{off} px");
    assert!((ground[2] - target).abs() < WITHIN_PX, "the surface under the centre {} m", ground[2]);
    // aimed 30 m above or below the ground, the centre is somewhere else
    for miss in [-30.0, 30.0] {
        let wrong = ViewState { position: Some([0.0, 0.0, raised.position.unwrap()[2] + miss]), ..raised };
        let px = off_centre_px(&wrong, &surface);
        eprintln!("aimed {miss:+} m off the ground: {px:.1} px");
        assert!(px > 5.0 * WITHIN_PX, "{miss} m: {px} px");
    }
}

/// A terrain layer of the renderer's own, prepared with the others (not drawn: the map's own
/// draws), to read the tiles the surface draws after a shot.
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

#[test]
fn the_surface_from_the_raised_camera_meets_the_centre_where_the_flat_map_has_it() {
    let Some(gpu) = gpu() else { return };
    let (mut on, _) = settled(true);
    let clocks = on.clocks(NOW_MS);
    let mut scene = SceneBuilder::default().build(&mut on, clocks, 1.0);
    scene.terrain = terrain::Input { shown: true };
    let mut renderer = MapRenderer::new(&gpu, FORMAT);
    let fixtures = Arc::new(DirTiles::fixtures());
    renderer.set_tiles(fixtures.clone(), None);
    let watch = Rc::new(RefCell::new(Terrain::new(&renderer.init(&gpu))));
    renderer.add_layer(Box::new(Watch(Rc::clone(&watch))));
    let surface_shot = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
    save(&surface_shot, "z4-zermatt-surface.png");
    // the camera left on the sea: inside the mountain
    let mut sunk = scene.clone();
    sunk.view.position = None;
    let sunk_shot = render_with(&gpu, &mut MapRenderer::new(&gpu, FORMAT), &sunk, 1.0).expect("rendered");
    save(&sunk_shot, "z4-zermatt-surface-sunk.png");

    // the tile the surface draws under the centre, and where the centre's ray meets it
    let raised = on.controller.view();
    let drawn: Vec<TileIndex> = watch.borrow().tiles().visible().map(|(t, _)| t).collect();
    let [lon, lat] = [raised.longitude, raised.latitude];
    let under: Vec<TileIndex> = drawn.iter().copied().filter(|&t| covers(t, lon, lat)).collect();
    eprintln!("{} tiles drawn; under the centre {under:?}", drawn.len());
    assert_eq!(under.len(), 1, "one tile draws each place");
    // the settled selection is the fixtures' (a tile asked for before the elevation range grew,
    // while the tiles were taken to lie on the sea, may not be)
    let selected = watch.borrow().tiles().selected().to_vec();
    let missing: Vec<TileIndex> = selected.iter().copied().filter(|&t| fixtures.held(io::MAPTERHORN, t) == Held::Missing).collect();
    eprintln!("{} tiles selected; asked for on the way and not in the fixtures: {:?}", selected.len(), fixtures.misses());
    assert!(missing.is_empty(), "the fixtures lack surface tiles the settled camera selects: {missing:?}");
    let surface = Ground::drawn(drawn);
    let off = off_centre_px(&raised, &surface);
    eprintln!("on the drawn tile the centre's ground point is {off:.2} px from the flat map's centre");
    assert!(off <= WITHIN_PX, "{off} px");
    // the picture is the surface's, not the clear colour, around the centre
    let centre = surface_shot.pixel(SIZE[0] / 2, SIZE[1] / 2);
    assert_ne!(centre, sunk_shot.pixel(SIZE[0] / 2, SIZE[1] / 2), "the raised camera sees another place than the sunk one");
    eprintln!("centre pixel raised {centre:?}, sunk {:?}", sunk_shot.pixel(SIZE[0] / 2, SIZE[1] / 2));
}

#[test]
fn the_whole_app_settles_on_the_ground_before_its_shot() {
    let Some(gpu) = gpu() else { return };
    let fixture = io::fixture_tiles_dir().parent().expect("the data folder").to_owned();
    let options = rustradar_app::Options { fixture: Some(fixture), ..options(ZERMATT, Basemap::Dark, SIZE, 1.0) };
    let off = app_shot_with(&gpu, &options, |state| state.persisted.settings.terrain = false);
    save(&off, "z4-zermatt-app-off.png");
    let started = std::time::Instant::now();
    let shot = headless::render_on_with(&gpu, &options, |state| state.persisted.settings.terrain = true).expect("the shot");
    eprintln!("the terrain-on shot took {} ms", started.elapsed().as_millis());
    // (the 3D surface, drawn with the terrain on since Z2, also asks for its passing selections
    // while the camera rises and its range widens; X1 holds its settled ones, which
    // `the_surface_from_the_raised_camera_meets_the_centre_where_the_flat_map_has_it` checks)
    let missing: Vec<_> = shot.missing_tiles.iter().filter(|(source, _)| source != io::MAPTERHORN).collect();
    assert!(missing.is_empty(), "the fixtures lack tiles the raised camera asks for: {missing:?}");
    let on = shot.image;
    save(&on, "z4-zermatt-app-on.png");
    let changed = off.rgba.chunks_exact(4).zip(on.rgba.chunks_exact(4)).filter(|(a, b)| a != b).count();
    eprintln!("{changed} of {} pixels differ", SIZE[0] * SIZE[1]);
    assert!(changed > 0);
}
