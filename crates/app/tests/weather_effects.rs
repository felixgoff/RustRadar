//! Item R11's shots: the realistic map's rain and tilt-shift drawn by the whole app
//! (`--screenshot`'s path: the settings, the scene builder, the map's post effects) from the
//! committed fixtures (`--fixture crates/map/tests/data`).
//!
//! - **Rain**: V4 (Arlanda, zoom 13.5, pitched 45°: the camera 2.6 km up, under the clouds) with
//!   the *rain* sky. The streaks fall where the map's own drops are (`map::post::Rain`, made from
//!   the same seed for the same shown rain and canvas): along the nearest depth's streaks the
//!   picture is lighter than a few pixels beside them, by about what a streak's colour and opacity
//!   give over the rainy ground.
//! - **Tilt-shift**: V3 (Stockholm region, zoom 9) on a fine day with the setting on and off. The
//!   rows between 42 % and 58 % of the height are the same picture; the top and bottom rows lose
//!   most of their edges, and with it on they are less sharp than the middle.
//!
//! There is no reference picture with the rain or the tilt-shift on; the maps under them are
//! `docs/port/reference/realistic-v3-*.png`'s and `realistic-v4-*.png`'s. Needs a wgpu adapter
//! (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints `skipped: …` and
//! passes. Shots go to `target/shots/r11-*.png`.

mod common;

use common::shots::{V3, V4, gpu, save};
use common::{NOW_MS, app_shot_with, options};
use rustradar_app::Options;
use rustradar_core::geo::Basemap;
use rustradar_core::weather::{PRESETS, Sky};
use rustradar_map::headless::Image;
use rustradar_map::io;
use rustradar_map::layers::sky;
use rustradar_map::post::{DEPTHS, Rain, rain_shown};

const SIZE: [u32; 2] = [1100, 720];

/// The app's realistic shot at `view` from the committed fixtures with the sky and the tilt-shift
/// set.
fn shot(gpu: &rustradar_map::Gpu, view: rustradar_scene::camera::ViewState, sky: Sky, tilt_shift: bool) -> Image {
    shot_at(gpu, view, sky, tilt_shift, 1.0)
}

/// [`shot`] on a screen of `scale` device pixels per CSS pixel.
fn shot_at(gpu: &rustradar_map::Gpu, view: rustradar_scene::camera::ViewState, sky: Sky, tilt_shift: bool, scale: f64) -> Image {
    let fixture = io::fixture_tiles_dir().parent().expect("the data folder").to_owned();
    let options = Options { fixture: Some(fixture), ..options(view, Basemap::Realistic, SIZE, scale) };
    app_shot_with(gpu, &options, |state| {
        state.persisted.settings.sky = sky;
        state.persisted.settings.tilt_shift = tilt_shift;
    })
}

fn luma(p: [u8; 4]) -> f64 {
    0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2])
}

#[test]
fn v4_in_the_rain_shows_the_streaks_where_the_drops_are() {
    let Some(gpu) = gpu() else { return };
    let view = V4.view();
    let image = shot(&gpu, view, Sky::Rain, false);
    save(&image, "r11-rain-v4.png");
    // the map's drops for this rain and canvas, as its post effects made them
    let size = [f64::from(SIZE[0]), f64::from(SIZE[1])];
    let input = sky::Input { shown: true, weather: Some(PRESETS.rain), daylight: true };
    let shown = rain_shown(Basemap::Realistic, &input, &view, size, NOW_MS);
    assert!(shown.falls() && shown.amount == 1.0, "{shown:?}");
    let mut rain = Rain::new();
    rain.show(shown, size, NOW_MS);
    rain.step(NOW_MS);
    let streaks = rain.streaks();
    let near: Vec<_> = streaks.iter().filter(|s| s.width == DEPTHS[2].line).collect();
    assert!(near.len() > 50, "{} near streaks", near.len());
    // along each near streak's middle, against a few pixels to either side
    let inside = |x: f64, y: f64| x >= 0.0 && y >= 0.0 && x < size[0] && y < size[1];
    let (mut on, mut beside, mut n) = (0.0, 0.0, 0.0);
    for s in &near {
        let (dx, dy) = (s.tail[0] - s.head[0], s.tail[1] - s.head[1]);
        let length = (dx * dx + dy * dy).sqrt();
        let (ax, ay) = (-dy / length, dx / length);
        for i in 3..=7 {
            let t = f64::from(i) / 10.0;
            let (x, y) = (s.head[0] + dx * t, s.head[1] + dy * t);
            let (lx, ly, rx, ry) = (x + ax * 5.0, y + ay * 5.0, x - ax * 5.0, y - ay * 5.0);
            if !(inside(x, y) && inside(lx, ly) && inside(rx, ry)) {
                continue;
            }
            let at = |x: f64, y: f64| luma(image.pixel(x as u32, y as u32));
            on += at(x, y);
            beside += (at(lx, ly) + at(rx, ry)) / 2.0;
            n += 1.0;
        }
    }
    let (on, beside) = (on / n, beside / n);
    eprintln!("rain at V4: {} streaks; along the near ones {on:.1}, beside them {beside:.1} ({n} points)", streaks.len());
    // a near streak is (215, 223, 235) at 0.4 over ground of about 90: some 40 levels at its line
    assert!(on - beside > 15.0, "on {on:.1}, beside {beside:.1}");
}

#[test]
fn v3_with_the_tilt_shift_is_blurred_at_the_top_and_bottom_only() {
    let Some(gpu) = gpu() else { return };
    let view = V3.view();
    let sharp = shot(&gpu, view, Sky::Sunny, false);
    let tilted = shot(&gpu, view, Sky::Sunny, true);
    save(&sharp, "r11-tilt-off-v3.png");
    save(&tilted, "r11-tilt-on-v3.png");
    let [w, h] = SIZE;
    // the middle band is the picture itself
    for y in (h * 43 / 100)..(h * 57 / 100) {
        for x in 0..w {
            assert_eq!(tilted.pixel(x, y), sharp.pixel(x, y), "({x}, {y})");
        }
    }
    // how sharp a band of rows is: the mean luma step to the right and down
    let sharpness = |image: &Image, rows: std::ops::Range<u32>| {
        let (mut sum, mut n) = (0.0, 0.0);
        for y in rows {
            for x in 0..w - 1 {
                let p = luma(image.pixel(x, y));
                sum += (luma(image.pixel(x + 1, y)) - p).abs() + (luma(image.pixel(x, (y + 1).min(h - 1))) - p).abs();
                n += 1.0;
            }
        }
        sum / n
    };
    let top = 0..h * 5 / 100;
    let middle = h * 45 / 100..h * 55 / 100;
    let bottom = h * 95 / 100..h - 1;
    let [top_off, middle_off, bottom_off] = [top.clone(), middle.clone(), bottom.clone()].map(|r| sharpness(&sharp, r));
    let [top_on, middle_on, bottom_on] = [top, middle, bottom].map(|r| sharpness(&tilted, r));
    eprintln!("sharpness off: top {top_off:.2}, middle {middle_off:.2}, bottom {bottom_off:.2}; on: top {top_on:.2}, middle {middle_on:.2}, bottom {bottom_on:.2}");
    assert_eq!(middle_on, middle_off);
    assert!(top_on < 0.5 * top_off && bottom_on < 0.5 * bottom_off, "the edges' sharpness drops by more than half");
    assert!(top_on < middle_on && bottom_on < middle_on, "with it on the top and bottom are less sharp than the middle");
}

/// On a screen of two device pixels per CSS pixel the realistic map is drawn at 1.5 (its cap) and
/// scaled up: the tilt-shift blurs at the map's own texels (σ 3 and 7.5 of them) and the last pass
/// scales up as the composite does, so the middle rows are the composite's to the pixel.
#[test]
fn v3_on_a_sharp_screen_is_blurred_at_the_map_s_scale() {
    let Some(gpu) = gpu() else { return };
    let view = V3.view();
    let sharp = shot_at(&gpu, view, Sky::Sunny, false, 2.0);
    let tilted = shot_at(&gpu, view, Sky::Sunny, true, 2.0);
    save(&tilted, "r11-tilt-on-v3-2x.png");
    let [w, h] = [SIZE[0] * 2, SIZE[1] * 2];
    assert_eq!([tilted.width, tilted.height], [w, h]);
    for y in (h * 44 / 100)..(h * 56 / 100) {
        for x in 0..w {
            assert_eq!(tilted.pixel(x, y), sharp.pixel(x, y), "({x}, {y})");
        }
    }
    // the top and bottom rows lose most of their edges
    let steps = |image: &Image, rows: std::ops::Range<u32>| {
        let (mut sum, mut n) = (0.0, 0.0);
        for y in rows {
            for x in 0..w - 1 {
                sum += (luma(image.pixel(x + 1, y)) - luma(image.pixel(x, y))).abs();
                n += 1.0;
            }
        }
        sum / n
    };
    for rows in [0..h * 5 / 100, h * 95 / 100..h] {
        let (off, on) = (steps(&sharp, rows.clone()), steps(&tilted, rows.clone()));
        eprintln!("2x rows {rows:?}: steps off {off:.2}, on {on:.2}");
        assert!(on < 0.5 * off, "rows {rows:?}: {on:.2} against {off:.2}");
    }
}
