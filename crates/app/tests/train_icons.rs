//! Item T2's acceptance shots: the demo trains (`--demo`, `dev::demo_trains`) drawn as icons by the
//! whole app (`--screenshot`: the train state, the scene builders, the map) over the committed
//! tiles, at V3 and closer in at zoom 12, each beside the same shot without `--demo`.
//!
//! The six demo trains stand within 140 m of the view centre, one per car shape: Skånetrafiken's
//! purple, SJ's silver twice, Öresundståg's lavender, Snälltåget's green and Inlandsbanan, which has
//! no colour of its own, in its kind's mint (regional). The shot's clock stands still, so they stand
//! at the start of their frames, turned to their headings. Today's `#demo` draws nothing (no key in
//! a plain browser), so there is no reference picture with trains; the map under them is
//! `docs/port/reference/dark-v3-1440x900.png`'s.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. Shots go to `target/shots/t2-*.png`.

mod common;

use common::shots::{Rect, Rgba, V3, close, difference, gpu, save};
use common::{NOW_MS, app_shot, options};
use rustradar_app::Options;
use rustradar_app::dev::demo_trains;
use rustradar_core::geo::Basemap;
use rustradar_core::trains::train_color_of;
use rustradar_map::headless::Image;
use rustradar_scene::camera::{ViewState, viewport_for};

/// The reference pictures' canvas.
const SIZE: [u32; 2] = [1440, 900];
/// The trains' alpha (`TRAIN_ALPHA`).
const ALPHA: f64 = 240.0 / 255.0;

fn shots(view: ViewState, name: &str) -> Option<(Image, Image)> {
    let gpu = gpu()?;
    let plain = options(view, Basemap::Dark, SIZE, 1.0);
    let demo = Options { demo: true, ..plain.clone() };
    let with = app_shot(&gpu, &demo);
    let without = app_shot(&gpu, &plain);
    save(&with, &format!("t2-{name}-demo-1440x900.png"));
    save(&without, &format!("t2-{name}-1440x900.png"));
    Some((with, without))
}

/// `rgb` at the trains' alpha over the map pixel `ground`.
fn over(rgb: [u8; 3], ground: Rgba) -> Rgba {
    let c = |i: usize| (f64::from(rgb[i]) * ALPHA + f64::from(ground[i]) * (1.0 - ALPHA)).round() as u8;
    [c(0), c(1), c(2), 255]
}

/// The pixels of `rect` in `with` that are the train colour `rgb` laid over what `without` has
/// there.
fn of_colour(with: &Image, without: &Image, rect: Rect, rgb: [u8; 3]) -> Vec<(u32, u32)> {
    rect.clip(with).pixels().filter(|&(x, y)| close(with.pixel(x, y), over(rgb, without.pixel(x, y)), 8)).collect()
}

/// The demo trains' colours in their order, and where each stands on the canvas of `view`.
fn demo(view: &ViewState) -> Vec<([u8; 3], [f64; 2])> {
    let viewport = viewport_for(view, SIZE.map(f64::from));
    demo_trains(view.latitude, view.longitude, NOW_MS as i64)
        .iter()
        .map(|t| {
            let [x, y, _] = viewport.project([t.lon, t.lat, 0.0]);
            (train_color_of(t), [x, y])
        })
        .collect()
}

/// V3: the six trains as one cluster of icons at the view centre (they stand within 2 px of one
/// another at zoom 9), in their operators' and kind's colours; nothing else on the map changes.
#[test]
fn v3_demo_trains_are_icons_in_their_colours_at_the_centre() {
    let Some((with, without)) = shots(V3.view(), "v3") else { return };
    let centre = [f64::from(SIZE[0]) / 2.0, f64::from(SIZE[1]) / 2.0];
    // every pixel the trains change is within an icon's length of the centre
    let changed: Vec<(u32, u32)> =
        Rect::whole(&with).pixels().filter(|&(x, y)| difference(with.pixel(x, y), without.pixel(x, y)) > 12).collect();
    assert!(changed.len() > 150, "the trains are drawn: {} pixels", changed.len());
    for &(x, y) in &changed {
        let d = (f64::from(x) + 0.5 - centre[0]).hypot(f64::from(y) + 0.5 - centre[1]);
        assert!(d < 24.0, "a pixel changed at {x},{y}, {d:.1} px from the centre");
    }
    let rect = Rect::around(&with, centre, 24.0);
    let trains = demo(&V3.view());
    let counts: Vec<usize> = trains.iter().map(|&(rgb, _)| of_colour(&with, &without, rect, rgb).len()).collect();
    // drawn in order, the last on top: Inlandsbanan's mint, then Snälltåget's green and SJ's
    // silver where the mint leaves them (the place name is drawn over them all); Skånetrafiken's
    // purple and Öresundståg's lavender are under the others at this zoom
    assert_eq!(trains[5].0, [96, 212, 160], "Inlandsbanan in its kind's colour");
    assert!(counts[5] > 30 && counts[1] > 30 && counts[4] > 5, "{counts:?}");
    assert_eq!(counts[1], counts[3], "both SJ trains in one silver");
}

/// Closer in (zoom 12, still the globe: icons, not cars) the trains stand apart: each where it is,
/// the topmost two whole at their places, every one's colour beside its place, and Inlandsbanan's
/// carriage lying along its heading of 110°.
#[test]
fn zoom_12_demo_trains_stand_apart_turned_to_their_headings() {
    let view = ViewState { zoom: 12.0, ..V3.view() };
    let Some((with, without)) = shots(view, "z12") else { return };
    let trains = demo(&view);
    for (k, &(rgb, at)) in trains.iter().enumerate() {
        let near = of_colour(&with, &without, Rect::around(&with, at, 12.0), rgb);
        assert!(near.len() >= 8, "train {k} {rgb:?} at {at:?}: {} pixels", near.len());
    }
    // the last two drawn are on top at their places (a joint can cross the very centre)
    for &(rgb, at) in &trains[4..] {
        let middle = of_colour(&with, &without, Rect::around(&with, at, 3.0), rgb);
        assert!(middle.len() >= 4, "{rgb:?} at its place: {} pixels", middle.len());
    }
    // the mint carriage's long axis: the principal axis of its pixels, on the screen (north up)
    let (rgb, at) = trains[5];
    let pixels = of_colour(&with, &without, Rect::around(&with, at, 16.0), rgb);
    let n = pixels.len() as f64;
    let (mx, my) = pixels.iter().fold((0.0, 0.0), |(sx, sy), &(x, y)| (sx + f64::from(x), sy + f64::from(y)));
    let (mx, my) = (mx / n, my / n);
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for &(x, y) in &pixels {
        let (dx, dy) = (f64::from(x) - mx, f64::from(y) - my);
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    let axis = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    // heading 110°: x along sin 110°, y (down) along −cos 110°
    let want = (-(110f64.to_radians().cos())).atan2(110f64.to_radians().sin());
    let off = (axis - want).to_degrees().rem_euclid(180.0);
    let off = off.min(180.0 - off);
    assert!(off < 6.0, "the carriage lies {off:.1}° off its heading's line");
    assert!(sxx + syy > 0.0 && (sxx - syy).hypot(2.0 * sxy) / (sxx + syy) > 0.5, "longer than wide");
}
