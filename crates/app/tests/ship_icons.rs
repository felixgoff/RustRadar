//! Item H2's acceptance shot: ships at V6 (Gothenburg harbour, zoom 13, flat) from a fixture
//! (`--fixture`'s `ships_in_view.json`, the `Ship` list `ships_in_view` answers) drawn by the whole
//! app (`--screenshot`: the ship state, the scene builders, the map) on the satellite map over the
//! committed tiles, beside the same shot with the ships off the map.
//!
//! Eleven ships stand on the Göta älv, every type colour and an unlisted type, two without a
//! heading: each small enough (its AIS length under 72 m) to be an icon at zoom 13, sized as a
//! large ship (22 px). A twelfth, a 190 m tanker, is long enough on the screen to be drawn as a model from zoom 12.5
//! (item H3): it has no icon. So: hulls in their type colours pointing along their headings, the
//! moored ones as dots, the old position faded, nothing where the tanker is. There is no reference
//! picture with ships (today's needs the backend); the map under them is
//! `docs/port/reference/satellite-v6-1440x900.png`'s, the shot without ships the comparison.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. Shots go to `target/shots/h2-*.png`.

mod common;

use std::path::{Path, PathBuf};

use common::shots::{Rect, Rgba, V6, difference, gpu, save};
use common::{app_shot_with, options};
use rustradar_app::Options;
use rustradar_app::state::AppState;
use rustradar_core::geo::Basemap;
use rustradar_core::model::Ship;
use rustradar_core::settings::Mode;
use rustradar_core::ships::ship_color;
use rustradar_map::headless::Image;
use rustradar_scene::camera::viewport_for;

/// The reference picture's canvas.
const SIZE: [u32; 2] = [1440, 900];

/// One ship of the fixture: where it stands on the canvas, its type, heading, whether old, and
/// its length (AIS bow + stern; the reference point in the middle).
struct Spot {
    xy: [f64; 2],
    kind: u8,
    heading: Option<f32>,
    old: bool,
    length: u16,
}

/// The ships on open water of the river (no land within 13 px in the shot without them), west to
/// east, and the long tanker last; all between the sidebar and the detail panel's places (stage 7
/// draws its cards over the map in the same shot).
const SPOTS: [Spot; 12] = [
    Spot { xy: [420.0, 626.0], kind: 30, heading: None, old: false, length: 40 },
    Spot { xy: [460.0, 585.0], kind: 4, heading: Some(90.0), old: true, length: 50 },
    Spot { xy: [500.0, 545.0], kind: 0, heading: Some(150.0), old: false, length: 40 },
    Spot { xy: [540.0, 500.0], kind: 8, heading: None, old: false, length: 12 },
    Spot { xy: [580.0, 481.0], kind: 7, heading: Some(20.0), old: false, length: 20 },
    Spot { xy: [620.0, 448.0], kind: 5, heading: Some(200.0), old: false, length: 24 },
    Spot { xy: [700.0, 415.0], kind: 4, heading: Some(60.0), old: false, length: 60 },
    Spot { xy: [780.0, 408.0], kind: 6, heading: Some(250.0), old: false, length: 60 },
    Spot { xy: [860.0, 396.0], kind: 3, heading: Some(100.0), old: false, length: 40 },
    Spot { xy: [900.0, 377.0], kind: 2, heading: Some(300.0), old: false, length: 28 },
    Spot { xy: [940.0, 355.0], kind: 8, heading: Some(330.0), old: false, length: 14 },
    Spot { xy: [740.0, 416.0], kind: 6, heading: Some(70.0), old: false, length: 190 },
];

/// The ships of [`SPOTS`], placed on V6's canvas.
fn ships() -> Vec<Ship> {
    let viewport = viewport_for(&V6.view(), SIZE.map(f64::from));
    SPOTS
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let [lon, lat] = viewport.unproject(s.xy, 0.0);
            // the reference point amidships; a beam of a fifth of the length (a beam over 0.7 of
            // it is not a usable size: the ship would be drawn at its class's typical length)
            let (half, side) = (s.length / 2, (s.length / 10).max(1));
            Ship {
                mmsi: 265_000_000 + i as u32,
                lat,
                lon,
                name: format!("SHIP {i}"),
                kind: s.kind,
                size: 2,
                heading: s.heading,
                old: s.old,
                dims: Some([half, s.length - half, side, side]),
                frames: Vec::new(),
            }
        })
        .collect()
}

/// A `--fixture` folder with the ships and the committed tiles.
fn folder() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("h2-ships-v6");
    std::fs::create_dir_all(&dir).expect("the fixture folder");
    std::fs::write(dir.join("ships_in_view.json"), serde_json::to_vec(&ships()).unwrap()).unwrap();
    link_tiles(&dir);
    dir
}

#[cfg(unix)]
fn link_tiles(dir: &Path) {
    let link = dir.join("tiles");
    if std::fs::symlink_metadata(&link).is_err() {
        std::os::unix::fs::symlink(rustradar_map::io::fixture_tiles_dir(), &link).expect("the tiles linked in");
    }
}

/// Elsewhere the shots draw on the bare earth: the comparisons hold all the same.
#[cfg(not(unix))]
fn link_tiles(_dir: &Path) {}

/// V6 on the satellite map with the ships on the map, and the same with them off it.
fn shots() -> Option<(Image, Image)> {
    let gpu = gpu()?;
    let options = Options { fixture: Some(folder()), ..options(V6.view(), Basemap::Satellite, SIZE, 1.0) };
    let ships_on = |state: &mut AppState| state.persisted.settings.mode = Mode::Ships;
    let with = app_shot_with(&gpu, &options, ships_on);
    let without = app_shot_with(&gpu, &options, |_| {});
    save(&with, "h2-v6-ships-1440x900.png");
    save(&without, "h2-v6-1440x900.png");
    Some((with, without))
}

/// `rgb` at `alpha` over the map pixel `ground`.
fn over(rgb: [u8; 3], alpha: u8, ground: Rgba) -> Rgba {
    let a = f64::from(alpha) / 255.0;
    let c = |i: usize| (f64::from(rgb[i]) * a + f64::from(ground[i]) * (1.0 - a)).round() as u8;
    [c(0), c(1), c(2), 255]
}

fn near(a: Rgba, b: Rgba, tolerance: u8) -> bool {
    (0..3).all(|c| a[c].abs_diff(b[c]) <= tolerance)
}

/// The pixels within `r` px of `spot` in `with` that are its type colour at its alpha laid over
/// what `without` has there.
fn of_colour(with: &Image, without: &Image, spot: &Spot, r: f64) -> Vec<(u32, u32)> {
    let alpha = if spot.old { 105 } else { 235 };
    let rgb = ship_color(spot.kind);
    Rect::around(with, spot.xy, r).clip(with).pixels().filter(|&(x, y)| near(with.pixel(x, y), over(rgb, alpha, without.pixel(x, y)), 10)).collect()
}

/// The pixels' centroid less `spot`'s place, and their principal axis as an angle on the screen
/// (radians, x right and y down, modulo π) with how elongated they are (0 round, 1 a line).
fn shape(pixels: &[(u32, u32)], spot: &Spot) -> ([f64; 2], f64, f64) {
    let n = pixels.len() as f64;
    let (mx, my) = pixels.iter().fold((0.0, 0.0), |(sx, sy), &(x, y)| (sx + f64::from(x) + 0.5, sy + f64::from(y) + 0.5));
    let (mx, my) = (mx / n, my / n);
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for &(x, y) in pixels {
        let (dx, dy) = (f64::from(x) + 0.5 - mx, f64::from(y) + 0.5 - my);
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    let axis = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let elongation = (sxx - syy).hypot(2.0 * sxy) / (sxx + syy);
    ([mx - spot.xy[0], my - spot.xy[1]], axis, elongation)
}

/// V6: every ship an icon in its type colour where it stands, each hull lying along its heading
/// with its broad stern behind (the colour's centroid sits behind the ship's place), the moored
/// ones round, the old one faded, nothing for the long tanker (a model from zoom 12.5), and
/// nothing else on the map changed.
#[test]
fn v6_ships_are_icons_in_their_type_colours_pointing_along_their_headings() {
    let Some((with, without)) = shots() else { return };
    // every pixel the ships change on the map between the sidebar and the detail panel's places
    // is within an icon's reach of an icon ship
    let icons = &SPOTS[..11];
    let map = Rect::new(352, 70, 1080 - 352, 860 - 70);
    let changed: Vec<(u32, u32)> = map.pixels().filter(|&(x, y)| difference(with.pixel(x, y), without.pixel(x, y)) > 12).collect();
    assert!(changed.len() > 1000, "the ships are drawn: {} pixels", changed.len());
    for &(x, y) in &changed {
        let d = icons.iter().map(|s| (f64::from(x) + 0.5 - s.xy[0]).hypot(f64::from(y) + 0.5 - s.xy[1])).fold(f64::INFINITY, f64::min);
        assert!(d < 16.0, "a pixel changed at {x},{y}, {d:.1} px from the nearest ship");
    }
    // the long tanker is a model at zoom 13: no icon where it stands
    let tanker = &SPOTS[11];
    let around = Rect::around(&with, tanker.xy, 8.0).clip(&with);
    assert!(around.pixels().all(|(x, y)| difference(with.pixel(x, y), without.pixel(x, y)) <= 12), "an icon at the tanker");
    for (i, spot) in icons.iter().enumerate() {
        let pixels = of_colour(&with, &without, spot, 14.0);
        let (centroid, axis, elongation) = shape(&pixels, spot);
        match spot.heading {
            None => {
                // a dot 0.62 · 22 px tall, of which the fill's 40 of 64 texels: about 8.5 px across
                assert!(pixels.len() > 24 && pixels.len() < 80, "ship {i}: {} pixels", pixels.len());
                assert!(elongation < 0.25, "ship {i} is round: {elongation:.2}");
                assert!(centroid[0].hypot(centroid[1]) < 1.0, "ship {i} centred: {centroid:?}");
            }
            Some(heading) => {
                // the hull's fill: about 52 × 27 of 64 texels at 22 px
                assert!(pixels.len() > 70, "ship {i}: {} pixels", pixels.len());
                let h = f64::from(heading).to_radians();
                // along the heading on the screen: x along sin, y (down) along −cos
                let want = (-h.cos()).atan2(h.sin());
                let off = (axis - want).to_degrees().rem_euclid(180.0);
                let off = off.min(180.0 - off);
                assert!(off < 8.0, "ship {i} lies {off:.1}° off its heading's line");
                assert!(elongation > 0.35, "ship {i} is longer than wide: {elongation:.2}");
                // the broad stern behind the pointed bow: the colour's centroid behind the place
                let ahead = centroid[0] * h.sin() - centroid[1] * h.cos();
                assert!(ahead < -0.4, "ship {i}'s colour leans {ahead:.2} px ahead");
            }
        }
    }
}
