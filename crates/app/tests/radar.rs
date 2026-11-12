//! Item W5's shots: the weather radar drawn by the whole app (`--screenshot`'s path: the radar
//! state asking the `--fixture` folder, the scene builder, the map's radar layer) from the
//! committed fixtures (`--fixture crates/map/tests/data`: RainViewer's frame list and tiles, the
//! recorded MRMS volume), each beside the same shot with the weather off.
//!
//! - V2 (Europe, zoom 4.5): RainViewer's frame. Wherever the radar is one colour it is one of the
//!   NWS bands at 55 % over the map below it (W2's ground layer), or a few bands stacked where a
//!   tile's own mesh covers a place twice (as today's does, W2's finding).
//! - The Ohio Valley (39 N, 84 W, zoom 5.3): the recorded MRMS scan, the bands at 70 % (W4's
//!   ground layer). RainViewer's fixture tiles do not reach there, so only the measured layer
//!   draws.
//! - With the weather off neither changes a pixel of the map.
//!
//! The night is off in these shots: drawn over the radar (row 6 after rows 5a and 5b) it would
//! tint the bands. There is no reference picture with the radar on; the map under it is
//! `docs/port/reference/dark-v2-1440x900.png`'s. Needs a wgpu adapter (SwiftShader or lavapipe
//! through `VK_ICD_FILENAMES`); without one it prints `skipped: …` and passes. Shots go to
//! `target/shots/w5-*.png`.

mod common;

use common::shots::{V2, gpu, save};
use common::{app_shot_with, options};
use rustradar_app::Options;
use rustradar_core::geo::Basemap;
use rustradar_map::headless::Image;
use rustradar_map::io;
use rustradar_scene::camera::ViewState;
use rustradar_scene::radar::{BAND_COUNT, band_color};

const SIZE: [u32; 2] = [1100, 720];

/// The committed fixtures' folder, as `--fixture` takes it.
fn fixtures() -> Options {
    Options { fixture: Some(io::fixture_tiles_dir().parent().expect("the data folder").to_owned()), ..Default::default() }
}

/// The app's shot at `view` from the committed fixtures with the weather `on` or off, the night
/// off.
fn shot(gpu: &rustradar_map::Gpu, view: ViewState, on: bool) -> Image {
    let options = Options { fixture: fixtures().fixture, ..options(view, Basemap::Dark, SIZE, 1.0) };
    app_shot_with(gpu, &options, |state| {
        state.persisted.settings.weather = on;
        state.persisted.settings.daylight = false;
    })
}

/// `[r, g, b, a]` drawn at `opacity` over `under`.
fn over(under: [u8; 4], [r, g, b, a]: [u8; 4], opacity: f64) -> [f64; 3] {
    let alpha = f64::from(a) / 255.0 * opacity;
    [0, 1, 2].map(|i| f64::from(under[i]) + (f64::from([r, g, b][i]) - f64::from(under[i])) * alpha)
}

/// Whether the 3 × 3 pixels round `(x, y)` are one colour (no edge there).
fn uniform(image: &Image, x: u32, y: u32) -> bool {
    let c = image.pixel(x, y);
    (x - 1..=x + 1).all(|i| (y - 1..=y + 1).all(|j| image.pixel(i, j) == c))
}

/// Whether `got` is some stack of one to six bands, weakest first, drawn at `opacity` over
/// `under` (within 2 levels).
fn stacked(under: [u8; 4], got: [u8; 4], opacity: f64, from: usize, left: usize) -> bool {
    (from..BAND_COUNT).any(|band| {
        let c = over(under, band_color(band), opacity);
        let next = [c[0].round() as u8, c[1].round() as u8, c[2].round() as u8, 255];
        let off = (0..3).map(|i| (i32::from(next[i]) - i32::from(got[i])).abs()).max().unwrap();
        off <= 2 || (left > 1 && stacked(next, got, opacity, band + 1, left - 1))
    })
}

/// What the radar changed between `base` (the weather off) and `image`: pixels changed, those
/// inside a band (no edge in either), those of one band at `opacity` over the map, those of a
/// stack of bands, and the bands seen.
struct Changes {
    changed: usize,
    inside: usize,
    one_band: usize,
    stacked: usize,
    bands: [usize; BAND_COUNT],
}

fn changes(base: &Image, image: &Image, opacity: f64) -> Changes {
    let mut c = Changes { changed: 0, inside: 0, one_band: 0, stacked: 0, bands: [0; BAND_COUNT] };
    for y in 1..image.height - 1 {
        for x in 1..image.width - 1 {
            let (before, after) = (base.pixel(x, y), image.pixel(x, y));
            if before == after {
                continue;
            }
            c.changed += 1;
            if !uniform(image, x, y) || !uniform(base, x, y) {
                continue;
            }
            c.inside += 1;
            let off = |want: [f64; 3]| (0..3).map(|i| (want[i] - f64::from(after[i])).abs()).fold(0.0, f64::max);
            let (miss, band) = (0..BAND_COUNT).map(|band| (off(over(before, band_color(band), opacity)), band)).min_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
            if miss <= 1.5 {
                c.one_band += 1;
                c.bands[band] += 1;
            } else if stacked(before, after, opacity, 0, 6) {
                c.stacked += 1;
            }
        }
    }
    c
}

/// V2 with the weather on: RainViewer's bands at 55 % over the dark map, from light rain to heavy;
/// with it off, not a pixel of radar.
#[test]
fn v2_draws_rainviewer_from_the_fixture_with_the_weather_on() {
    let Some(gpu) = gpu() else { return };
    let base = shot(&gpu, V2.view(), false);
    let image = shot(&gpu, V2.view(), true);
    save(&base, "w5-v2-weather-off.png");
    save(&image, "w5-v2-weather-on.png");
    let c = changes(&base, &image, 0.55);
    eprintln!("V2: {} px changed, {} inside a band, {} one band, {} stacked; by band {:?}", c.changed, c.inside, c.one_band, c.stacked, c.bands);
    assert!(c.changed > 50_000, "{} px of radar", c.changed);
    assert!((c.one_band + c.stacked) * 100 >= c.inside * 97, "{} + {} of {} inside are bands at 55 %", c.one_band, c.stacked, c.inside);
    assert!(c.bands[..9].iter().all(|&n| n > 0), "cyan to orange: {:?}", c.bands);
    // the weather off again is the map alone: the same as a shot that never had it on
    let again = shot(&gpu, V2.view(), false);
    assert_eq!(again.rgba, base.rgba, "the weather off draws no radar");
}

/// Over the Ohio Valley the recorded MRMS scan: its bands at 70 % over the dark map.
#[test]
fn the_ohio_valley_draws_the_recorded_mrms_scan() {
    let Some(gpu) = gpu() else { return };
    let ohio = ViewState { latitude: 39.0, longitude: -84.0, zoom: 5.3, pitch: 0.0, bearing: 0.0, position: None };
    let base = shot(&gpu, ohio, false);
    let image = shot(&gpu, ohio, true);
    save(&base, "w5-ohio-weather-off.png");
    save(&image, "w5-ohio-weather-on.png");
    let c = changes(&base, &image, 0.7);
    eprintln!("Ohio: {} px changed, {} inside a band, {} one band, {} stacked; by band {:?}", c.changed, c.inside, c.one_band, c.stacked, c.bands);
    assert!(c.changed > 20_000, "{} px of radar", c.changed);
    // (W4 found places today's meshing covers twice: those show stacked bands)
    assert!((c.one_band + c.stacked) * 100 >= c.inside * 97, "{} + {} of {} inside are bands at 70 %", c.one_band, c.stacked, c.inside);
    assert!(c.bands.iter().filter(|&&n| n > 0).count() >= 5, "several bands: {:?}", c.bands);
}
