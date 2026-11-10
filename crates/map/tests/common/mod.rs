pub mod fixture_tiles;
pub mod shots;

/// A PNG as an RGBA image, or `None` if it cannot be read (the reader `shots::save` compares golden
/// shots with; the app's `tests/common` has its own, as it decodes PNG with another crate).
pub fn read_png(path: &std::path::Path) -> Option<rustradar_map::headless::Image> {
    let rgba = image::open(path).ok()?.into_rgba8();
    Some(rustradar_map::headless::Image { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() })
}
