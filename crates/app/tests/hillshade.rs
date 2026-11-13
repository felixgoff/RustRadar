//! Item R12's shots: the shaded relief as the app draws it (the settings, the scene builder's
//! `scene.hillshade`, the map's hillshade layer on its terrarium tiles) at the fixtures' Alps
//! (`RELIEF_VIEWS`: 46.0 N, 7.75 E, zoom 9.45, 1100 × 720) from the committed tiles.
//!
//! - The scene hands the layer the *terrain* setting on every basemap; the layer draws today's
//!   choice (G:2649-2671): always on realistic, on dark and satellite with the terrain on.
//! - Dark with the terrain on against off: the relief darkens much of the map, and lights the
//!   slopes facing the north-west.
//! - Satellite with the terrain on against off: over the same strongly shaded bare land the
//!   imagery takes 0.45 of the shadow colour dark takes (`HILLSHADE_OPACITY`).
//!
//! The names are off (the relief lies under them). The realistic map's relief and its haze, and
//! the whole opacity table, are the map's tests (`layers::hillshade`). Needs a wgpu adapter
//! (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints `skipped: …` and
//! passes. Shots go to `target/shots/r12-app-*.png`.
//!
//! Stage 9: with the terrain on the app also draws the 3D surface (Z1) and drapes the relief over
//! it, into its tiles' covers (Z2); the fixtures hold no surface for this view (X1: the Alps view
//! takes no 3D surface), and the surface lights what is draped over it. So the shots are the app's
//! scene (`SceneBuilder`) drawn by the map with the surface left out: the relief drawn flat, as it
//! is drawn into the covers. The draping itself is `layers::terrain::cover`'s tests.

mod common;

use std::sync::Arc;

use common::shots::{LAND, gpu, save};
use rustradar_app::scene_builder::SceneBuilder;
use rustradar_app::state::{AppState, Clocks, Startup};
use rustradar_core::geo::Basemap;
use rustradar_map::MapRenderer;
use rustradar_map::headless::{FORMAT, Image, render_with};
use rustradar_map::io::DirTiles;
use rustradar_map::layers::hillshade::{self, layer_for};
use rustradar_scene::camera::ViewState;

/// `RELIEF_VIEWS`' Alps.
const ALPS: ViewState = ViewState { latitude: 46.0, longitude: 7.75, zoom: 9.45, pitch: 0.0, bearing: 0.0, position: None };
const SIZE: [u32; 2] = [1100, 720];
/// The hillshade's shadow colour (`HILLSHADE_STYLE.shadow`).
const SHADOW: [u8; 3] = [4, 8, 16];

/// The app's scene of the Alps on `basemap` (the names off, the terrain setting `terrain`) drawn
/// by the map from the committed fixtures, the 3D surface left out (see the module's docs).
fn shot(gpu: &rustradar_map::Gpu, basemap: Basemap, terrain: bool) -> Image {
    let size = [f64::from(SIZE[0]), f64::from(SIZE[1])];
    let mut state = AppState::new(Startup { basemap: Some(basemap), ..Default::default() }, None, None, None, ALPS, size);
    state.persisted.settings.labels = false;
    state.persisted.settings.terrain = terrain;
    let clocks = Clocks { local_ms: common::NOW_MS, feed_ms: common::NOW_MS };
    let mut scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
    assert_eq!(scene.terrain.shown, terrain, "the app draws the 3D surface with the terrain on");
    scene.terrain.shown = false;
    let mut renderer = MapRenderer::new(gpu, FORMAT);
    renderer.set_tiles(Arc::new(DirTiles::fixtures()), None);
    let image = render_with(gpu, &mut renderer, &scene, 1.0).expect("rendered");
    assert_eq!([image.width, image.height], SIZE);
    image
}

/// The share of the shadow colour `over` took over `base` at a pixel (least squares over its
/// channels).
fn shadow_share(base: [u8; 4], over: [u8; 4]) -> f64 {
    let (n, d) = (0..3).fold((0.0, 0.0), |(n, d), c| {
        let (b, o, s) = (f64::from(base[c]), f64::from(over[c]), f64::from(SHADOW[c]));
        (n + (b - o) * (b - s), d + (b - s) * (b - s))
    });
    n / d
}

/// The scene builder hands the hillshade the *terrain* setting on every basemap, and the layer
/// draws on realistic always, on dark and satellite only with the terrain on.
#[test]
fn the_scene_hands_the_relief_the_terrain_setting() {
    for basemap in [Basemap::Dark, Basemap::Satellite, Basemap::Realistic] {
        for terrain in [false, true] {
            let mut state = AppState::new(Startup { basemap: Some(basemap), ..Default::default() }, None, None, None, ALPS, [1100.0, 720.0]);
            state.persisted.settings.terrain = terrain;
            let clocks = Clocks { local_ms: common::NOW_MS, feed_ms: common::NOW_MS };
            let scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
            assert_eq!(scene.hillshade, hillshade::Input { shown: true, terrain }, "{basemap:?}");
            let drawn = layer_for(scene.basemap, scene.hillshade.terrain, false, ALPS.zoom).is_some();
            assert_eq!(drawn, terrain || basemap == Basemap::Realistic, "{basemap:?} terrain {terrain}");
        }
    }
}

/// The Alps through the app on dark and satellite, the terrain off and on: the relief shows on
/// both with it on, and the satellite map takes 0.45 of what the dark map takes of the shadow.
#[test]
fn the_alps_in_relief_with_the_terrain_on() {
    let Some(gpu) = gpu() else { return };
    let dark = (shot(&gpu, Basemap::Dark, false), shot(&gpu, Basemap::Dark, true));
    let satellite = (shot(&gpu, Basemap::Satellite, false), shot(&gpu, Basemap::Satellite, true));
    save(&dark.1, "r12-app-alps-dark.png");
    save(&satellite.1, "r12-app-alps-satellite.png");
    let total = (SIZE[0] * SIZE[1]) as usize;
    for (name, (off, on), share) in [("dark", &dark, 4), ("satellite", &satellite, 6)] {
        let (mut darker, mut lighter) = (0usize, 0usize);
        for (b, o) in off.rgba.chunks_exact(4).zip(on.rgba.chunks_exact(4)) {
            let change: i32 = (0..3).map(|c| i32::from(o[c]) - i32::from(b[c])).sum();
            darker += usize::from(change <= -6);
            lighter += usize::from(change >= 3);
        }
        eprintln!("{name}: {darker} of {total} darker, {lighter} lighter");
        assert!(darker * share > total, "{name}: {darker} of {total} darker");
        assert!(lighter * 20 > total, "{name}: {lighter} of {total} lighter");
    }
    // over strongly shaded bare land of the dark map (nothing of the vector map over the relief)
    let mut ratios = Vec::new();
    for y in 0..SIZE[1] {
        for x in 0..SIZE[0] {
            if dark.0.pixel(x, y) != LAND {
                continue;
            }
            let a = shadow_share(dark.0.pixel(x, y), dark.1.pixel(x, y));
            if a >= 0.25 {
                ratios.push(shadow_share(satellite.0.pixel(x, y), satellite.1.pixel(x, y)) / a);
            }
        }
    }
    assert!(ratios.len() > 5000, "{} strongly shaded bare pixels", ratios.len());
    ratios.sort_by(f64::total_cmp);
    let ratio = ratios[ratios.len() / 2];
    eprintln!("{} pixels: satellite / dark {ratio:.3}", ratios.len());
    assert!((ratio - 0.45).abs() < 0.04, "satellite / dark {ratio}");
}
