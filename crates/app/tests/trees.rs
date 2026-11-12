//! Item R7's checks through the whole app: the realistic map's trees as the scene builder shows
//! them and the app's tile fetcher feeds them.
//!
//! - **The canopy heights** (`backend::canopy`, the map's `layers::trees::CANOPY`): the backend's
//!   constants and rules are the scene's (`scene::canopy`; the backend cannot see it, so they are
//!   kept equal by hand), and for every tile of the committed recording (`canopy/`, item X1) the
//!   map's own reader of a `--fixture` folder (`io::DirTiles`) answers the very grid the backend's
//!   reader puts together from the same ranges (which `backend::canopy`'s tests hold to the
//!   network's, and the map's tests to geotiff.js's under bun).
//! - **The scene** shows the trees on the realistic map (no setting: the map draws them on the flat
//!   map from zoom 14).
//! - **Humlegården** (zoom 16, the fixtures' park) through `--screenshot`'s path with the committed
//!   fixtures: the park's middle is full of sunlit crowns; the shot lacks no tile the trees ask for
//!   but the ground the fixtures hold only for the close-up tiles (land cover, canopy).
//!
//! Needs a wgpu adapter for the shot (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without
//! one it prints `skipped: …` and passes. Shots go to `target/shots/r7-*.png`.

mod common;

use common::shots::{gpu, save};
use common::{NOW_MS, options};
use rustradar_app::headless;
use rustradar_app::scene_builder::SceneBuilder;
use rustradar_app::state::{AppState, Clocks, Startup};
use rustradar_backend::canopy as backend;
use rustradar_core::geo::Basemap;
use rustradar_map::io::{self, DirTiles};
use rustradar_map::layers::trees;
use rustradar_scene::camera::ViewState;
use rustradar_scene::canopy as scene;
use rustradar_scene::tiles::TileIndex;

/// `CLOSE_UP_VIEWS`' Humlegården.
const HUMLEGARDEN: ViewState = ViewState { latitude: 59.3398, longitude: 18.0727, zoom: 16.0, pitch: 45.0, bearing: 20.0, position: None };
const SIZE: [u32; 2] = [1100, 720];

/// The zoom-16 tiles the committed canopy recording holds (its JSON's `tiles`).
fn recorded_tiles() -> Vec<TileIndex> {
    let data = io::fixture_tiles_dir().parent().expect("the data folder").join(backend::FIXTURE);
    let mut tiles = Vec::new();
    for entry in std::fs::read_dir(&data).expect("the canopy recording") {
        let path = entry.expect("an entry").path();
        if path.extension().is_some_and(|e| e == "json") {
            let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            for t in json["tiles"].as_array().expect("tiles") {
                let n = |i: usize| t[i].as_u64().unwrap() as u32;
                tiles.push(TileIndex::new(n(1), n(2), n(0)));
            }
        }
    }
    tiles
}

#[test]
fn the_map_reads_the_fixtures_canopy_as_the_backend_does() {
    assert_eq!((trees::CANOPY, trees::CANOPY_FIXTURE), (backend::NAME, backend::FIXTURE));
    assert_eq!((scene::CHM, scene::CANOPY_ATTRIBUTION, scene::FILE_ZOOM, scene::FILES_CACHED, scene::MAX_CACHED), (backend::CHM, backend::ATTRIBUTION, backend::FILE_ZOOM, backend::FILES_CACHED, backend::MAX_CACHED));
    assert_eq!(scene::WANT, backend::WANT);
    // the rules, over a spread of tiles of every zoom from the files' own
    for z in 10..=20 {
        let n = 1u32 << z;
        for (x, y) in (0..50u64).map(|i| (((i * 7919 + 13) % u64::from(n)) as u32, ((i * 104_729 + 7) % u64::from(n)) as u32)) {
            let (a, b) = (scene::place_in_file(x, y, z), backend::place_in_file(x, y, z));
            assert_eq!((&a.key, a.x, a.y, a.span), (&b.key, b.x, b.y, b.span), "{z}/{x}/{y}");
            assert_eq!(scene::file_url(&a.key), backend::file_url(&b.key));
            for width in [512, 4096, 32768] {
                let (wa, wb) = (scene::window(width, &a), backend::window(width, &b));
                assert_eq!((wa.x0, wa.y0, wa.size), (wb.x0, wb.y0, wb.size), "{z}/{x}/{y} in {width}");
            }
            let widths = [32768, 16384, 8192, 4096, 2048, 1024, 512];
            assert_eq!(scene::pick_image(&widths, a.span), backend::pick_image(&widths, b.span));
        }
    }
    // every recorded tile: the map's fixture reader is the backend's
    let data = io::fixture_tiles_dir().parent().expect("the data folder").to_owned();
    let canopy = backend::Canopy::fixture(&data).expect("the committed recording");
    let dir = DirTiles::at(io::fixture_tiles_dir());
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    let tiles = recorded_tiles();
    assert_eq!(tiles.len(), 25);
    for tile in tiles {
        let grid = rt.block_on(canopy.tile(tile.x, tile.y, tile.z)).unwrap_or_else(|| panic!("{tile:?}"));
        assert_eq!(dir.read(trees::CANOPY, tile), Ok(Some(grid.heights.clone())), "{tile:?}");
    }
    assert!(dir.misses().is_empty(), "{:?}", dir.misses());
}

/// The scene shows the trees (R7), which the map draws on the realistic flat map alone.
#[test]
fn the_scene_shows_the_trees() {
    for basemap in [Basemap::Realistic, Basemap::Dark] {
        let mut state = AppState::new(Startup { basemap: Some(basemap), ..Default::default() }, None, None, None, HUMLEGARDEN, [1100.0, 720.0]);
        let clocks = Clocks { local_ms: NOW_MS, feed_ms: NOW_MS };
        let scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
        assert_eq!(scene.trees, trees::Input::SHOWN);
    }
}

/// The share of a rectangle's pixels in a sunlit crown's yellow green (bright green over red, a
/// low blue: neither the park's grass nor its paths, nor the dark map).
fn crowns(image: &rustradar_map::headless::Image, [x0, y0, x1, y1]: [u32; 4]) -> f64 {
    let mut hit = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let [r, g, b, _] = image.pixel(x, y);
            hit += usize::from(g > 150 && r > 110 && b < 110);
        }
    }
    hit as f64 / f64::from((x1 - x0) * (y1 - y0))
}

#[test]
fn humlegarden_through_the_whole_app_is_full_of_trees() {
    let Some(gpu) = gpu() else { return };
    let fixture = io::fixture_tiles_dir().parent().expect("the data folder").to_owned();
    let park = [400, 300, 750, 550];
    let realistic = rustradar_app::Options { fixture: Some(fixture.clone()), ..options(HUMLEGARDEN, Basemap::Realistic, SIZE, 1.0) };
    let shot = headless::render_on(&gpu, &realistic).expect("the shot");
    save(&shot.image, "r7-app-humlegarden.png");
    let lacking: Vec<&(String, String)> = shot.missing_tiles.iter().filter(|(source, _)| !["canopy", "nmd", "worldcover", "landcover"].contains(&source.as_str())).collect();
    assert!(lacking.is_empty(), "the fixtures lack {lacking:?}");
    let dark = rustradar_app::Options { fixture: Some(fixture), ..options(HUMLEGARDEN, Basemap::Dark, SIZE, 1.0) };
    let plain = headless::render_on(&gpu, &dark).expect("the shot").image;
    let (trees, none) = (crowns(&shot.image, park), crowns(&plain, park));
    eprintln!("crowns over {:.1} % of the park (the dark map {:.1} %)", 100.0 * trees, 100.0 * none);
    assert!(trees > 0.15, "{trees}");
    assert!(none < 0.01, "{none}");
}
