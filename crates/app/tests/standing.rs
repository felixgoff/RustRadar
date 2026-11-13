//! Item Z3 through the whole app: at a mountain village (`TERRAIN_VIEWS`' Zermatt, 46.0207 N,
//! 7.7491 E, zoom 16, pitched 60°, 800 × 500, from the committed tiles) on the realistic map with
//! the terrain on, the village's Streets GL buildings stand on the mountainside, raised by the
//! ground under each of their corners (`rustradar_map::layers::terrain::ground`): the village's
//! tiled roofs show on the slope (at sea level they would be 1.6 km under it, and none would).
//! As today, the building tiles are chosen against sea level (deck's tile layers get no `zRange`),
//! so from the camera raised over Zermatt the near ones are not asked for and the village shows
//! at the top of the picture, further down the valley. Shots in `target/shots/`:
//! `z3-zermatt-realistic.png` with the buildings, `z3-zermatt-realistic-bare.png` without.
//!
//! The GPU part needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without
//! one it prints `skipped: …` and passes.

mod common;

use common::options;
use common::shots::{gpu, save};
use rustradar_app::headless;
use rustradar_core::geo::Basemap;
use rustradar_map::io;
use rustradar_scene::camera::ViewState;

/// `TERRAIN_VIEWS`' Zermatt.
const ZERMATT: ViewState = ViewState { latitude: 46.0207, longitude: 7.7491, zoom: 16.0, pitch: 60.0, bearing: 0.0, position: None };
const SIZE: [u32; 2] = [800, 500];

#[test]
fn zermatt_s_buildings_stand_on_its_mountainside() {
    let Some(gpu) = gpu() else { return };
    let fixture = io::fixture_tiles_dir().parent().expect("the data folder").to_owned();
    let options = rustradar_app::Options { fixture: Some(fixture), ..options(ZERMATT, Basemap::Realistic, SIZE, 1.0) };
    let shot = |buildings: bool| {
        let shot = headless::render_on_with(&gpu, &options, |state| {
            state.persisted.settings.terrain = true;
            state.persisted.settings.buildings = buildings;
        })
        .expect("the shot");
        shot.image
    };
    let with = shot(true);
    let bare = shot(false);
    save(&with, "z3-zermatt-realistic.png");
    save(&bare, "z3-zermatt-realistic-bare.png");
    // the village's roofs: tiled, reddish (the picture without buildings has next to none of
    // these colours: the hillside, its woods, roads and river are green, grey and blue)
    let roofs = |image: &rustradar_map::headless::Image| image.rgba.chunks_exact(4).filter(|p| u16::from(p[0]) > u16::from(p[1]) + 20).count();
    let (standing, bare) = (roofs(&with), roofs(&bare));
    eprintln!("{standing} roof pixels with the buildings, {bare} without");
    // at sea level, as before, the village would be 1.6 km under the mountainside: no roof shows
    assert!(standing > 300, "the village's roofs show on the mountainside: {standing} px");
    assert!(bare < 30, "{bare} reddish pixels without buildings");
}
