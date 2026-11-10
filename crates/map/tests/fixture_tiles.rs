//! The tile fixtures (plan E16): the set `cargo xtask fixtures` committed to `tests/data/tiles`
//! stays within its budget, covers the shot views, decodes, and is served by the file-backed
//! fetcher of `common/fixture_tiles.rs`.

mod common;

use std::fs;
use std::path::Path;

use common::fixture_tiles::{FixtureTiles, Held, fixtures_dir};
use rustradar_core::mvt;
use rustradar_map::io::{Fetch, FetchError};
use rustradar_scene::camera::{ViewState, viewport_for};
use rustradar_scene::tiles::{IMAGERY_TILES, RADAR_TILES, TileIndex, TilesetOptions, VECTOR_TILES, tile_indices};

/// The views of plan section 8 the set covers: `(name, lat, lon, zoom, pitch, bearing)`.
const VIEWS: [(&str, f64, f64, f64, f64, f64); 4] = [
    ("V3", 59.33, 18.07, 9.0, 0.0, 0.0),
    ("V4", 59.6519, 17.9186, 13.5, 45.0, 30.0),
    ("V5", 59.3293, 18.0686, 15.5, 60.0, 20.0),
    ("V6", 57.70, 11.93, 13.0, 0.0, 0.0),
];

/// The hillshade's tiles: 256 px, zooms 0 to 12.
const HILLSHADE_TILES: TilesetOptions = TilesetOptions::new(256.0, 0.0, 12.0);

fn view_state((_, latitude, longitude, zoom, pitch, bearing): (&str, f64, f64, f64, f64, f64)) -> ViewState {
    ViewState { longitude, latitude, zoom, pitch, bearing, position: None }
}

fn file_bytes(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| if e.path().is_dir() { file_bytes(&e.path()) } else { e.metadata().unwrap().len() })
        .sum()
}

/// The tile at `(lat, lon)` at zoom `z`.
fn tile_at(lat: f64, lon: f64, z: u32) -> TileIndex {
    let n = f64::from(1u32 << z);
    let phi = lat.to_radians();
    let x = ((lon + 180.0) / 360.0 * n).floor() as u32;
    let y = ((1.0 - (phi.tan() + 1.0 / phi.cos()).ln() / std::f64::consts::PI) / 2.0 * n).floor() as u32;
    TileIndex::new(x, y, z)
}

#[test]
fn the_set_stays_under_40_mb() {
    let bytes = file_bytes(&fixtures_dir());
    assert!(bytes < 40_000_000, "{bytes} bytes");
}

#[test]
fn the_set_holds_the_five_sources_and_a_notice() {
    let tiles = FixtureTiles::new();
    assert_eq!(tiles.sources(), ["esri", "openfreemap", "rainviewer", "streets-gl", "terrarium"]);
    let notice = fs::read_to_string(fixtures_dir().join("NOTICE")).unwrap();
    for source in tiles.sources() {
        assert!(notice.contains(&format!("\n{source}/\n")), "the NOTICE says nothing of {source}");
    }
    // what the templates were read from: OpenFreeMap's TileJSON, RainViewer's frame list
    let tilejson = fs::read_to_string(fixtures_dir().join("openfreemap/tilejson.json")).unwrap();
    assert!(tilejson.contains("\"tiles\":[\"https://tiles.openfreemap.org/planet/"), "{tilejson:.200}");
    let maps = fs::read_to_string(fixtures_dir().join("rainviewer/weather-maps.json")).unwrap();
    assert!(maps.contains("\"past\""), "{maps:.200}");
}

#[test]
fn a_held_tile_is_served_and_any_other_reads_as_nothing() {
    let tiles = FixtureTiles::new();
    let world = tiles.read("openfreemap", TileIndex::ROOT).unwrap().unwrap();
    assert_eq!(world, fs::read(fixtures_dir().join("openfreemap/0/0/0.pbf")).unwrap());
    assert!(tiles.misses().is_empty());
    // a tile the set lacks, a source it lacks, a name that would leave the folder
    assert_eq!(tiles.read("openfreemap", TileIndex::new(0, 0, 15)), Ok(None));
    assert_eq!(tiles.read("mapterhorn", TileIndex::ROOT), Ok(None));
    assert_eq!(tiles.held("../tiles/openfreemap", TileIndex::ROOT), Held::NotHeld);
    assert_eq!(tiles.held("", TileIndex::ROOT), Held::NotHeld);
    assert_eq!(
        tiles.misses(),
        [("openfreemap".to_owned(), TileIndex::new(0, 0, 15)), ("mapterhorn".to_owned(), TileIndex::ROOT)]
    );
}

#[test]
fn a_nothing_marker_reads_as_nothing_and_a_broken_file_fails() {
    let root = std::env::temp_dir().join(format!("rustradar-fixture-tiles-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("streets-gl/16/1")).unwrap();
    fs::write(root.join("streets-gl/16/1/2.none"), b"").unwrap();
    fs::write(root.join("streets-gl/16/1/3.pbf"), b"\x1a\x00").unwrap();
    // a folder where the tile's file should be cannot be read
    fs::create_dir_all(root.join("streets-gl/16/1/4.pbf")).unwrap();
    let tiles = FixtureTiles::at(&root);
    assert_eq!(tiles.held("streets-gl", TileIndex::new(1, 2, 16)), Held::Nothing);
    assert_eq!(tiles.read("streets-gl", TileIndex::new(1, 2, 16)), Ok(None));
    assert_eq!(tiles.read("streets-gl", TileIndex::new(1, 3, 16)), Ok(Some(b"\x1a\x00".to_vec())));
    assert!(matches!(tiles.read("streets-gl", TileIndex::new(1, 4, 16)), Err(FetchError::Failed(_))));
    assert!(tiles.misses().is_empty(), "{:?}", tiles.misses());
    assert_eq!(
        tiles.tiles("streets-gl"),
        [TileIndex::new(1, 2, 16), TileIndex::new(1, 3, 16), TileIndex::new(1, 4, 16)]
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn it_answers_through_the_fetch_contract() {
    let tiles = FixtureTiles::new();
    let tile = tile_at(57.70, 11.93, 14);
    let mut fetch = Fetch::start(&tiles, "esri".into(), tile);
    let bytes = fetch.poll().expect("answered at once").unwrap().expect("a tile");
    assert_eq!(&bytes[..3], b"\xff\xd8\xff", "a JPEG");
    let mut fetch = Fetch::start(&tiles, "esri".into(), TileIndex::new(0, 0, 20));
    assert_eq!(fetch.poll(), Some(Ok(None)));
}

#[test]
fn the_world_is_there_at_zooms_0_to_2() {
    let tiles = FixtureTiles::new();
    for z in 0..=2 {
        for x in 0..1 << z {
            for y in 0..1 << z {
                assert!(
                    matches!(tiles.held("openfreemap", TileIndex::new(x, y, z)), Held::Tile(_)),
                    "openfreemap {z}/{x}/{y}"
                );
            }
        }
    }
}

#[test]
fn every_tile_the_shot_views_select_is_held() {
    let tiles = FixtureTiles::new();
    let sources = [
        ("openfreemap", VECTOR_TILES),
        ("esri", IMAGERY_TILES),
        ("terrarium", HILLSHADE_TILES),
        ("rainviewer", RADAR_TILES),
    ];
    let mut asked = 0;
    for view in VIEWS {
        // the shot command's default size and the reference shots' sizes
        for size in [[1100.0, 720.0], [1440.0, 900.0], [390.0, 844.0]] {
            let viewport = viewport_for(&view_state(view), size);
            for (source, opts) in sources {
                for tile in tile_indices(&viewport, &opts) {
                    asked += 1;
                    assert_ne!(tiles.held(source, tile), Held::NotHeld, "{} {size:?}: {source} {}", view.0, tile.id());
                }
            }
        }
    }
    assert!(asked > 500, "{asked}");
    // Streets GL's tile under V5's centre (it is closer in than 15.5)
    let (_, lat, lon, ..) = VIEWS[2];
    assert!(matches!(tiles.held("streets-gl", tile_at(lat, lon, 16)), Held::Tile(_)));
}

#[test]
fn vector_tiles_decode_into_their_layers() {
    let tiles = FixtureTiles::new();
    let names = |source: &str, tile: TileIndex| -> Vec<String> {
        let bytes = tiles.read(source, tile).unwrap().unwrap();
        mvt::layers(&bytes).map(|l| l.name.to_owned()).collect()
    };
    for source in ["openfreemap", "streets-gl"] {
        let held = tiles.tiles(source);
        assert!(!held.is_empty(), "{source}");
        for tile in held {
            let layers = names(source, tile);
            assert!(!layers.is_empty(), "{source} {} has no layers", tile.id());
        }
    }
    // the dark map's source layers at the whole world, and buildings in Stockholm at zoom 14
    let world = names("openfreemap", TileIndex::ROOT);
    for layer in ["water", "boundary", "place"] {
        assert!(world.iter().any(|l| l == layer), "z0 lacks {layer}: {world:?}");
    }
    let city = names("openfreemap", tile_at(59.3293, 18.0686, 14));
    for layer in ["water", "transportation", "building", "place"] {
        assert!(city.iter().any(|l| l == layer), "z14 lacks {layer}: {city:?}");
    }
    let bytes = tiles.read("openfreemap", tile_at(59.3293, 18.0686, 14)).unwrap().unwrap();
    let buildings = mvt::layer(&bytes, "building").unwrap();
    assert!(buildings.len() > 100, "{}", buildings.len());
    let sgl = names("streets-gl", tile_at(59.3293, 18.0686, 16));
    assert!(sgl.iter().any(|l| l == "buildings"), "{sgl:?}");
}

#[test]
fn raster_tiles_decode_at_256_px() {
    let tiles = FixtureTiles::new();
    for (source, format) in [
        ("esri", image::ImageFormat::Jpeg),
        ("terrarium", image::ImageFormat::Png),
        ("rainviewer", image::ImageFormat::Png),
    ] {
        let held = tiles.tiles(source);
        assert!(!held.is_empty(), "{source}");
        for tile in held {
            let bytes = tiles.read(source, tile).unwrap().unwrap();
            let picture = image::load_from_memory_with_format(&bytes, format)
                .unwrap_or_else(|e| panic!("{source} {}: {e}", tile.id()));
            assert_eq!((picture.width(), picture.height()), (256, 256), "{source} {}", tile.id());
        }
    }
    // the heights under Stockholm: terrarium's R·256 + G + B/256 − 32768 is near sea level
    let bytes = tiles.read("terrarium", tile_at(59.3293, 18.0686, 12)).unwrap().unwrap();
    let rgb = image::load_from_memory(&bytes).unwrap().to_rgb8();
    let mut heights: Vec<f64> =
        rgb.pixels().map(|p| f64::from(p[0]) * 256.0 + f64::from(p[1]) + f64::from(p[2]) / 256.0 - 32768.0).collect();
    heights.sort_by(f64::total_cmp);
    let median = heights[heights.len() / 2];
    assert!((-5.0..80.0).contains(&median), "{median}");
}
