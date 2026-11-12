//! Item R12's shots: the shaded relief drawn by the whole app (`--screenshot`'s path: the settings,
//! the scene builder's `scene.hillshade`, the map's hillshade layer on its terrarium tiles) at the
//! fixtures' Alps (`RELIEF_VIEWS`: 46.0 N, 7.75 E, zoom 9.45, 1100 × 720) from the committed tiles.
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
//! Trap for stage 9: with the terrain on, Z1's 3D surface will be drawn too (on the globe as on
//! the flat map); these shots compare the terrain on against off, so the surface must leave the
//! picture's colours where it drapes them as they are, or these comparisons need its own base.

mod common;

use common::shots::{LAND, gpu, save};
use common::{app_shot_with, options};
use rustradar_app::Options;
use rustradar_app::scene_builder::SceneBuilder;
use rustradar_app::state::{AppState, Clocks, Startup};
use rustradar_core::geo::Basemap;
use rustradar_map::headless::Image;
use rustradar_map::io;
use rustradar_map::layers::hillshade::{self, layer_for};
use rustradar_scene::camera::ViewState;

/// `RELIEF_VIEWS`' Alps.
const ALPS: ViewState = ViewState { latitude: 46.0, longitude: 7.75, zoom: 9.45, pitch: 0.0, bearing: 0.0, position: None };
const SIZE: [u32; 2] = [1100, 720];
/// The hillshade's shadow colour (`HILLSHADE_STYLE.shadow`).
const SHADOW: [u8; 3] = [4, 8, 16];

/// The app's shot of the Alps on `basemap` from the committed fixtures, the names off, the
/// terrain setting `terrain`.
fn shot(gpu: &rustradar_map::Gpu, basemap: Basemap, terrain: bool) -> Image {
    let fixture = io::fixture_tiles_dir().parent().expect("the data folder").to_owned();
    let options = Options { fixture: Some(fixture), ..options(ALPS, basemap, SIZE, 1.0) };
    app_shot_with(gpu, &options, |state| {
        state.persisted.settings.labels = false;
        state.persisted.settings.terrain = terrain;
    })
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
