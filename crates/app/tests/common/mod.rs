//! What the app's integration tests share (plan section 8, item E18): the shot harness, which is
//! the map's `tests/common/shots.rs` taken in as it is (one file for both packages), the PNG reader
//! it compares golden shots with, and shots through the app's `--screenshot` path.
#![allow(dead_code)]

pub mod fixtures;
#[path = "../../../map/tests/common/shots.rs"]
pub mod shots;

use std::path::Path;

use rustradar_app::{Options, headless};
use rustradar_core::geo::Basemap;
use rustradar_map::Gpu;
use rustradar_map::headless::Image;
use rustradar_scene::camera::ViewState;

/// The clock of the app's shots: 2026-06-21 10:00 UTC (the reference pictures' summer morning).
pub const NOW_MS: f64 = 1_782_036_000_000.0;

/// A PNG as an RGBA image, or `None` if it cannot be read or is not 8-bit RGBA (what
/// `Image::save_png` writes).
pub fn read_png(path: &Path) -> Option<Image> {
    let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut reader = png::Decoder::new(file).read_info().ok()?;
    let mut rgba = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut rgba).ok()?;
    if (info.color_type, info.bit_depth) != (png::ColorType::Rgba, png::BitDepth::Eight) {
        return None;
    }
    rgba.truncate(info.buffer_size());
    Some(Image { width: info.width, height: info.height, rgba })
}

/// What `--screenshot --view … --basemap … --size … --scale … --now NOW_MS` asks for.
pub fn options(view: ViewState, basemap: Basemap, size: [u32; 2], scale: f64) -> Options {
    Options { view: Some(view), basemap: Some(basemap), size: Some(size), scale: Some(scale), now_ms: Some(NOW_MS), ..Default::default() }
}

/// The shot `options` describe, drawn on `gpu` by `--screenshot`'s path (`rustradar_app::headless`:
/// the backend started, the state, the scene builders, the map and the egui pass), its size
/// checked.
#[track_caller]
pub fn app_shot(gpu: &Gpu, options: &Options) -> Image {
    let shot = headless::render_on(gpu, options).unwrap_or_else(|e| panic!("the shot of {options:?}: {e}"));
    let (size, scale) = headless::canvas(options);
    assert_eq!([shot.image.width, shot.image.height], headless::surface_size(size, scale), "the shot's size");
    shot.image
}
