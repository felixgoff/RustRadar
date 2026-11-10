//! The shot tests (plan section 8, item E18): the acceptance shots of items E5 to E10 written
//! against the shared harness of `common/shots.rs` (the named views V1–V8, [`pixel`],
//! [`assert_color`], [`coverage`], [`area`], [`disc_radius`], [`components`], …), and that
//! harness's own checks on pictures drawn on the CPU.
//!
//! - E5 (the globe) through the app's `--screenshot` path, as a person would run it, on the named
//!   views;
//! - E6 to E10 (the path, mesh, shadow, icon, circle and text pipelines) through the map's
//!   headless path, each with a small test layer drawn over the app's own empty map.
//!
//! Needs a wgpu adapter: on a machine without a GPU, a software Vulkan (SwiftShader or lavapipe)
//! through `VK_ICD_FILENAMES`. Without one every GPU test prints `skipped: …` and passes. Shots go
//! to `target/shots/e18-*.png`; `SHOT_GOLDEN=1` compares them with a recorded set as well.

mod common;

use std::f64::consts::TAU;

use common::shots::*;
use common::{app_shot, options};
use rustradar_app::Options;
use rustradar_core::geo::Basemap;
use rustradar_map::frame::{FrameSlot, ViewFrame, model, power_of_two_scale};
use rustradar_map::headless::Image;
use rustradar_map::layers::{DrawCx, Init, Layer, PrepareCx, earth};
use rustradar_map::pipelines::{circle, fill, icon, mesh, path, shadow, text};
use rustradar_map::state::DrawState;
use rustradar_map::SceneInput;
use rustradar_scene::camera::{GLOBE_EARTH_RADIUS_M, GLOBE_RADIUS, GlobeViewport, ViewState, lng_lat_to_world, world_to_lng_lat};
use rustradar_scene::geom::path::{LocalMercator, Paint, PathBuilder};
use rustradar_scene::text::{Face, FontSettings, GlyphAtlas, OVERPASS, TextLayout};

const WHITE: Rgba = [255, 255, 255, 255];
const STOCKHOLM: [f64; 2] = [V3.longitude, V3.latitude];
const ARLANDA: [f64; 2] = [V4.longitude, V4.latitude];

fn view([longitude, latitude]: [f64; 2], zoom: f64, pitch: f64, bearing: f64) -> ViewState {
    ViewState { longitude, latitude, zoom, pitch, bearing, position: None }
}

/// A `width × height` picture with `colour(x, y)` at each pixel.
fn image_of(width: u32, height: u32, colour: impl Fn(u32, u32) -> Rgba) -> Image {
    let rgba = (0..height).flat_map(|y| (0..width).map(move |x| (x, y))).flat_map(|(x, y)| colour(x, y)).collect();
    Image { width, height, rgba }
}

/// `top` at alpha `a` (0–255) times `coverage` blended over `bottom`, as the GPU blends it.
fn over(bottom: Rgba, top: Rgba, a: u8, coverage: f64) -> [f64; 3] {
    let a = f64::from(a) / 255.0 * coverage;
    std::array::from_fn(|i| f64::from(top[i]) * a + f64::from(bottom[i]) * (1.0 - a))
}

fn byte(c: [f64; 3]) -> Rgba {
    [c[0].round() as u8, c[1].round() as u8, c[2].round() as u8, 255]
}

/// The share of the pixel `[x, x + 1) × [y, y + 1)` that `inside(px, py)` holds, by 16 × 16
/// samples.
fn supersampled(x: u32, y: u32, inside: impl Fn(f64, f64) -> bool) -> f64 {
    let n = 16;
    let hits = (0..n * n).filter(|i| inside(f64::from(x) + (f64::from(i % n) + 0.5) / f64::from(n), f64::from(y) + (f64::from(i / n) + 0.5) / f64::from(n))).count();
    hits as f64 / f64::from(n * n)
}

/// `colour` over `background` by `coverage`, rounded to bytes.
fn blend(background: Rgba, colour: Rgba, coverage: f64) -> Rgba {
    byte(std::array::from_fn(|i| f64::from(background[i]) + (f64::from(colour[i]) - f64::from(background[i])) * coverage))
}

// ---------------------------------------------------------------------------------------------
// The harness itself, on pictures drawn on the CPU

#[test]
fn pixels_are_read_from_the_top_left_and_colours_compared_by_channel() {
    let image = image_of(4, 3, |x, y| [x as u8 * 10, y as u8 * 10, 7, 255]);
    assert_eq!(pixel(&image, 2, 1), [20, 10, 7, 255]);
    assert_color(&image, 3, 2, [30, 20, 7, 255], 0);
    assert_color(&image, 3, 2, [32, 18, 7, 255], 2);
    assert!(!close([30, 20, 7, 255], [33, 20, 7, 255], 2));
    assert_eq!(difference([30, 20, 7, 255], [30, 20, 7, 250]), 5);
    assert_eq!(pixel_at(&image, [2.9, 0.2]), Some([20, 0, 7, 255]));
    assert_eq!(pixel_at(&image, [4.0, 0.0]), None);
    assert_eq!(pixel_at(&image, [-0.1, 0.0]), None);
    assert!(!close(pixel(&image, 0, 0), [1, 0, 7, 255], 0), "a colour off by one level fails at tolerance 0");
    // rectangles: pixels by their centres, clipped
    assert_eq!(Rect::around(&image, [2.0, 1.6], 1.0), Rect { x0: 1, y0: 1, x1: 3, y1: 3 });
    assert_eq!(Rect::around(&image, [2.0, 1.5], 1.0), Rect { x0: 1, y0: 0, x1: 3, y1: 3 }, "centres on the edge count");
    assert_eq!(Rect::around(&image, [0.0, 0.0], 5.0), Rect::whole(&image));
    assert_eq!(Rect::new(1, 1, 10, 10).clip(&image), Rect { x0: 1, y0: 1, x1: 4, y1: 3 });
    assert_eq!(Rect::new(1, 0, 2, 3).pixels().count(), 6);
}

#[test]
fn coverage_is_the_share_of_pixels_within_the_tolerance() {
    let red = [252, 0, 0, 255];
    let image = image_of(10, 10, |x, _| if x < 3 { red } else { INK });
    assert_eq!(coverage(&image, Rect::whole(&image), red, 0), 0.3);
    assert_eq!(coverage(&image, Rect::new(0, 0, 3, 10), [255, 0, 0, 255], 3), 1.0);
    assert_eq!(coverage(&image, Rect::new(0, 0, 3, 10), [255, 0, 0, 255], 2), 0.0);
    assert_eq!(coverage(&image, Rect::new(2, 0, 2, 5), INK, 0), 0.5);
    assert_eq!(coverage(&image, Rect::new(20, 20, 2, 2), INK, 0), 0.0, "off the image");
    assert_eq!(count(&image, Rect::whole(&image), |p| p == INK), 70);
    assert_eq!(darker_than(&image, Rect::whole(&image), [30, 0, 0]), None);
    assert_eq!(darker_than(&image, Rect::whole(&image), [31, 0, 0]), Some((3, 0, INK)));
    assert_eq!(darkest_and_brightest(&image, Rect::whole(&image)), Some((INK, red)));
}

/// [`area`] gives an antialiased line's width from its coverage: a vertical line 1.5 px wide.
#[test]
fn area_measures_an_antialiased_line() {
    let (left, right) = (4.25, 5.75);
    let image = image_of(12, 20, |x, _| {
        let overlap = (f64::from(x + 1).min(right) - f64::from(x).max(left)).max(0.0);
        blend(INK, WHITE, overlap)
    });
    let width = area(&image, Rect::whole(&image), INK, WHITE) / 20.0;
    assert!((width - 1.5).abs() < 0.01, "{width}");
    assert_eq!(mix([142, 142, 142, 255], INK, WHITE), 112.0 / 225.0);
    assert_eq!(mix([0, 0, 0, 255], INK, WHITE), 0.0, "clamped");
}

/// [`disc_radius`] and [`disc_radii`] find the edge of an antialiased disc to a fraction of a
/// pixel, bright on dark and in the dark map's two greys.
#[test]
fn disc_radius_finds_the_edge_of_an_antialiased_disc() {
    let (centre, radius) = ([64.2, 60.7], 40.3);
    for (inside, outside, ray, sector) in [(WHITE, INK, 0.1, 0.05), (LAND, INK, 0.3, 0.1), (INK, SATELLITE_EARTH, 0.3, 0.1)] {
        let image = image_of(128, 128, |x, y| {
            let c = supersampled(x, y, |px, py| (px - centre[0]).hypot(py - centre[1]) < radius);
            blend(outside, inside, c)
        });
        for i in 0..16 {
            let angle = f64::from(i) * TAU / 16.0 + 0.05;
            let r = disc_radius(&image, centre, angle, inside, outside).expect("on the image");
            assert!((r - radius).abs() < ray, "{inside:?} on {outside:?}: {r} at {angle}");
        }
        let radii = disc_radii(&image, centre, 40.0, 36, inside, outside);
        assert_eq!(radii.len(), 36);
        for r in radii {
            let r = r.expect("every sector on the image");
            assert!((r - radius).abs() < sector, "{inside:?} on {outside:?}: sector at {r}");
        }
    }
    // a ring that leaves the picture gives no radius there
    let image = image_of(64, 64, |x, y| if (f64::from(x) + 0.5 - 32.0).hypot(f64::from(y) + 0.5 - 32.0) < 30.0 { WHITE } else { INK });
    let radii = disc_radii(&image, [32.0, 32.0], 30.0, 4, WHITE, INK);
    assert!(radii.iter().all(Option::is_none), "{radii:?}");
    assert_eq!(disc_radius(&image_of(8, 8, |_, _| WHITE), [4.0, 4.0], 0.0, WHITE, INK), None);
}

#[test]
fn components_count_dashes_and_bounds_hold_them() {
    // five dashes 3 × 2 with gaps of 2, and two pixels touching only at a corner
    let image = image_of(30, 8, |x, y| {
        let dash = (1..3).contains(&y) && x < 25 && x % 5 < 3;
        let corner = (x, y) == (5, 5) || (x, y) == (6, 6);
        if dash || corner { WHITE } else { INK }
    });
    let found = components(&image, Rect::whole(&image), |p| p == WHITE);
    assert_eq!(found.len(), 7);
    assert_eq!(found[0], Component { pixels: 6, bounds: Rect::new(0, 1, 3, 2), centroid: [1.5, 2.0] });
    assert_eq!(found[4].bounds, Rect::new(20, 1, 3, 2));
    assert_eq!((found[5].pixels, found[5].bounds), (1, Rect::new(5, 5, 1, 1)));
    // within a rectangle, cut where it cuts
    let cut = components(&image, Rect::new(1, 0, 6, 8), |p| p == WHITE);
    assert_eq!(cut.iter().map(|c| c.bounds).collect::<Vec<_>>(), [Rect::new(1, 1, 2, 2), Rect::new(5, 1, 2, 2), Rect::new(5, 5, 1, 1), Rect::new(6, 6, 1, 1)]);
    assert_eq!(bounds(&image, Rect::whole(&image), |p| p == WHITE), Some(Rect::new(0, 1, 23, 6)));
    assert_eq!(bounds(&image, Rect::new(25, 0, 5, 8), |p| p == WHITE), None);
}

#[test]
fn coverage_box_places_edges_to_a_fraction_of_a_pixel() {
    let ([l, t, r, b], size) = ([3.3, 2.25, 9.6, 5.5], [14usize, 9usize]);
    let coverage: Vec<f64> = (0..size[1])
        .flat_map(|y| (0..size[0]).map(move |x| (x, y)))
        .map(|(x, y)| {
            let w = (((x + 1) as f64).min(r) - (x as f64).max(l)).max(0.0);
            let h = (((y + 1) as f64).min(b) - (y as f64).max(t)).max(0.0);
            w * h
        })
        .collect();
    let found = coverage_box(&coverage, size).expect("ink");
    for (got, want) in found.iter().zip([l, t, r, b]) {
        assert!((got - want).abs() < 0.25, "{found:?}");
    }
    assert_eq!(coverage_box(&[0.2; 4], [2, 2]), None);
    let image = image_of(3, 1, |x, _| [x as u8 * 100, 0, 0, 255]);
    assert_eq!(weights(&image, |p| f64::from(p[0]) / 100.0), [0.0, 1.0, 1.0]);
}

#[test]
fn golden_comparison_lets_rasteriser_noise_through_but_not_a_change() {
    let golden = image_of(100, 100, |x, y| [x as u8, y as u8, 30, 255]);
    assert_eq!(compare(&golden, &golden, GOLDEN_TOLERANCE, GOLDEN_OUTLIERS), Ok(()));
    let noisy = image_of(100, 100, |x, y| [x as u8 + 2, y as u8, if (x, y) == (7, 7) { 90 } else { 28 }, 255]);
    assert_eq!(compare(&noisy, &golden, GOLDEN_TOLERANCE, GOLDEN_OUTLIERS), Ok(()), "10 outliers of 10 000 may move");
    let moved = image_of(100, 100, |x, y| [x as u8, y as u8, if x < 10 { 40 } else { 30 }, 255]);
    let why = compare(&moved, &golden, GOLDEN_TOLERANCE, GOLDEN_OUTLIERS).expect_err("a change");
    assert!(why.starts_with("1000 pixels moved"), "{why}");
    assert!(compare(&image_of(10, 10, |_, _| INK), &golden, 2, 0.0).is_err(), "another size");
    // what `save` reads golden shots back with
    let file = shots_dir().join("e18-harness-roundtrip.png");
    golden.save_png(&file).expect("written");
    assert_eq!(common::read_png(&file), Some(golden));
    std::fs::remove_file(&file).expect("removed");
    assert_eq!(common::read_png(&file), None);
}

/// The named views are the table of plan section 8, and `--view` takes them as written.
#[test]
fn the_named_views_are_those_of_section_8() {
    assert_eq!(VIEWS.map(|v| v.name), ["V1", "V2", "V3", "V4", "V5", "V6", "V7", "V8"]);
    assert_eq!(V1.view(), ViewState::INITIAL);
    assert_eq!(V4.arg(), "59.6519,17.9186,13.5,45,30");
    assert_eq!(V8.view(), view([179.5, 0.0], 3.0, 0.0, 0.0));
    assert_eq!(VIEWS.iter().filter(|v| v.is_flat()).map(|v| v.name).collect::<Vec<_>>(), ["V4", "V5", "V6"]);
    assert_eq!(named_view("V7"), Some(V7));
    assert_eq!(named_view("V9"), None);
    for named in VIEWS {
        let parsed = Options::parse(["--view".to_string(), named.arg()]).expect("taken");
        assert_eq!(parsed.view, Some(named.view()), "{}", named.name);
    }
    assert_eq!(SHOT_SIZE, rustradar_app::SHOT_SIZE);
}

// ---------------------------------------------------------------------------------------------
// E5: the globe, through the app's `--screenshot` path

/// How far from the canvas's centre E1's camera puts the limb of a sphere of `ratio` × deck's
/// globe along `angle` (radians, x right, y down), CSS pixels: bisection on how far each pixel's
/// ray passes from the globe's centre.
fn limb(globe: &GlobeViewport, angle: f64, ratio: f64) -> f64 {
    let centre = [globe.width / 2.0, globe.height / 2.0];
    let (mut inside, mut outside) = (0.0, globe.width.max(globe.height));
    for _ in 0..60 {
        let r = (inside + outside) / 2.0;
        if globe.ray_distance_to_globe_center_ratio([centre[0] + r * angle.cos(), centre[1] + r * angle.sin()]) < ratio {
            inside = r;
        } else {
            outside = r;
        }
    }
    (inside + outside) / 2.0
}

/// The earth sphere's radius over deck's surface sphere.
const EARTH_RATIO: f64 = earth::EARTH_RADIUS_M / GLOBE_EARTH_RADIUS_M;
/// How far (levels, each channel) the GPU's lit earth may be from [`earth::lit_color`] (the
/// earth layer's own `LIT_TOLERANCE`, measured by T2 on SwiftShader).
const LIT_TOLERANCE: f64 = 2.5;

/// The E5 shots' options: [`options`] with an empty `--fixture` folder, so no tile is drawn and
/// the globe is E5's earth and land alone (the dark map's sea, B2's, has the ink's colour; the
/// vector map is checked by its own shots in `map::layers::vector_map`).
fn globe_options(view: ViewState, basemap: Basemap, size: [u32; 2], scale: f64) -> Options {
    let empty = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("e18-no-tiles");
    std::fs::create_dir_all(&empty).expect("an empty fixture folder");
    Options { fixture: Some(empty), ..options(view, basemap, size, scale) }
}

/// What a tile-less shot of `frame` (CSS pixels; `scale` device pixels each) shows on `basemap` at
/// the device pixel centre `xy`, with how many levels it may be off: on the dark map the land
/// rectangle, exactly `(50,50,50)`, where the pixel's ray meets the surface within ±85.06°; past
/// that (the polar caps), and on satellite everywhere, the earth sphere lit as deck lights it
/// ([`earth::lit_color`] with the unaimed rig, which is the scene's on the globe; T2). `None` off
/// the earth, within 1.5 px of the land's edge, and within 3 px of the earth sphere's own limb,
/// where on SwiftShader single MSAA samples of its edge-on facets win the depth test against the
/// land (T2's trap; the unlit earth did the same, unseen).
fn globe_colour(frame: &ViewFrame, basemap: Basemap, scale: f64, xy: [f64; 2]) -> Option<([f64; 3], f64)> {
    let globe = frame.viewport.globe()?;
    let css = [xy[0] / scale, xy[1] / scale];
    let on_earth = |p: [f64; 2]| globe.ray_distance_to_globe_center_ratio(p) < EARTH_RATIO;
    let (dx, dy) = (css[0] - globe.width / 2.0, css[1] - globe.height / 2.0);
    let out = dx.hypot(dy).max(1e-9);
    let (ox, oy) = (3.0 * dx / out, 3.0 * dy / out);
    if on_earth([css[0] - ox, css[1] - oy]) != on_earth([css[0] + ox, css[1] + oy]) {
        return None;
    }
    let lit = || earth::lit_color(frame, &mesh::LightRig::new(), basemap, css).map(|c| (c.map(|v| v * 255.0), LIT_TOLERANCE));
    if basemap != Basemap::Dark {
        return lit();
    }
    if globe.ray_distance_to_globe_center_ratio(css) >= 1.0 {
        return None;
    }
    let edge = earth::LAND_POLYGON[2][1];
    let past = |p: [f64; 2]| globe.unproject(p, 0.0)[1].abs() > edge;
    let here = past(css);
    let around = [[-1.5, 0.0], [1.5, 0.0], [0.0, -1.5], [0.0, 1.5]];
    if !around.iter().all(|[ax, ay]| past([css[0] + ax, css[1] + ay]) == here) {
        return None;
    }
    if here { lit() } else { Some(([f64::from(LAND[0]), f64::from(LAND[1]), f64::from(LAND[2])], 0.0)) }
}

/// The largest channel difference between `p` and `colour`.
fn off_by(p: Rgba, colour: [f64; 3]) -> f64 {
    (0..3).map(|c| (f64::from(p[c]) - colour[c]).abs()).fold(0.0, f64::max)
}

/// Checks the disc on ink round the image's centre against the limb of the sphere of `ratio` ×
/// deck's globe in a tile-less shot of `frame` on `basemap` (at `scale`): its edge within 1.5 px
/// in every sector on the image (E5's acceptance), the colour of a covered pixel at the limb
/// taken as [`globe_colour`] gives it just inside (the land on the dark map; the lit sphere on
/// satellite); along four rays too where the disc's rim is one colour (dark); every pixel of the
/// disc's middle what [`globe_colour`] says (the land exactly, the lit caps within their
/// tolerance); the corners ink. Returns the mean measured edge, device pixels.
#[track_caller]
fn assert_disc_at_the_limb(image: &Image, frame: &ViewFrame, scale: f64, ratio: f64, basemap: Basemap, what: &str) -> f64 {
    let globe = frame.viewport.globe().expect("the globe");
    let centre = Rect::whole(image).centre();
    let near = limb(globe, 0.0, ratio) * scale;
    let sectors = 72;
    // a covered pixel's colour at the limb, per sector: the expected colour 4 CSS px inside it
    let rim = |angle: f64| -> Rgba {
        if basemap == Basemap::Dark {
            return LAND;
        }
        let r = (limb(globe, angle, ratio) - 4.0) * scale;
        let (colour, _) = globe_colour(frame, basemap, scale, [centre[0] + r * angle.cos(), centre[1] + r * angle.sin()]).expect("the lit earth inside its limb");
        byte(colour)
    };
    let mut radii: std::collections::HashMap<Rgba, Vec<Option<f64>>> = std::collections::HashMap::new();
    let mut edges = Vec::new();
    for s in 0..sectors {
        let angle = (s as f64 + 0.5) * TAU / sectors as f64;
        let inside = rim(angle);
        let Some(edge) = radii.entry(inside).or_insert_with(|| disc_radii(image, centre, near, sectors, inside, INK))[s] else { continue };
        let expected = limb(globe, angle, ratio) * scale;
        assert!((edge - expected).abs() <= 1.5, "{what}: the edge at {:.1}° is at {edge:.2} px, E1's limb at {expected:.2} px", angle.to_degrees());
        edges.push(edge);
    }
    assert!(edges.len() >= sectors / 3, "{what}: only {} sectors on the picture", edges.len());
    if basemap == Basemap::Dark {
        for angle in [0.3, 1.9, 3.5, 5.1] {
            let expected = limb(globe, angle, ratio) * scale;
            if let Some(edge) = disc_radius(image, centre, angle, LAND, INK) {
                assert!((edge - expected).abs() <= 1.5, "{what}: along {angle} the edge is at {edge:.2} px, the limb at {expected:.2} px");
            }
        }
    }
    let middle = Rect::around(image, centre, 0.6 * near);
    let (mut checked, mut wrong, mut worst) = (0usize, 0usize, 0.0f64);
    for (x, y) in middle.pixels() {
        let Some((colour, tolerance)) = globe_colour(frame, basemap, scale, [f64::from(x) + 0.5, f64::from(y) + 0.5]) else { continue };
        let off = off_by(pixel(image, x, y), colour);
        worst = worst.max(off);
        checked += 1;
        if off > tolerance {
            if wrong < 5 {
                eprintln!("{what}: ({x}, {y}) is {:?}, expected {colour:.1?} ± {tolerance}", pixel(image, x, y));
            }
            wrong += 1;
        }
    }
    eprintln!("{what}: {checked} pixels of the disc's middle checked, worst {worst:.2} levels off");
    assert!(checked * 10 >= middle.pixels().count() * 9, "{what}: only {checked} pixels of the disc's middle checked");
    assert_eq!(wrong, 0, "{what}: {wrong} pixels of the disc's middle are not what they should be");
    for corner in [Rect::new(0, 0, 8, 8), Rect::new(image.width - 8, image.height - 8, 8, 8)] {
        assert_eq!(coverage(image, corner, INK, 0), 1.0, "{what}: the corner {corner:?}");
    }
    edges.iter().sum::<f64>() / edges.len() as f64
}

/// Item E5's acceptance, shot V1 through `--screenshot`'s path (the backend started, no tiles):
/// on the dark map a disc of `(50,50,50)` on `(30,30,30)` whose edge is E1's projection of the
/// globe's limb within 1.5 px all round, at the shot size and scale 1 and on a smaller canvas at
/// scale 2; past ±85.06° (the cap above the centre) the lit earth sphere (T2).
#[test]
fn v1_is_a_disc_of_land_on_ink_at_the_limb() {
    let Some(gpu) = gpu() else { return };
    for (size, scale) in [(SHOT_SIZE, 1.0), ([640, 400], 2.0)] {
        let image = app_shot(&gpu, &globe_options(V1.view(), Basemap::Dark, size, scale));
        save(&image, &format!("e18-v1-dark-{}x{}@{scale}.png", size[0], size[1]));
        let frame = ViewFrame::new(&V1.view(), [f64::from(size[0]), f64::from(size[1])], scale, 0.0);
        let edge = assert_disc_at_the_limb(&image, &frame, scale, 1.0, Basemap::Dark, &format!("V1 {size:?} at {scale}"));
        let globe = frame.viewport.globe().expect("the globe");
        eprintln!("V1 {size:?} at {scale}: mean edge {edge:.2} px, limb {:.2} px", limb(globe, 0.0, 1.0) * scale);
        // the cap past 85.06° N is above the centre, the lit earth sphere, not land (on the shot
        // size's canvas; the smaller one cuts it off)
        let [x, y, _] = frame.viewport.project([10.0, 88.0, 0.0]);
        if size == SHOT_SIZE {
            assert!(y > 0.0, "88° N is on the picture at {x:.1}, {y:.1}");
            let (x, y) = ((x * scale) as u32, (y * scale) as u32);
            let (lit, tolerance) = globe_colour(&frame, Basemap::Dark, scale, [f64::from(x) + 0.5, f64::from(y) + 0.5]).expect("the cap");
            let p = pixel(&image, x, y);
            assert!(tolerance > 0.0 && off_by(p, lit) <= tolerance && p != LAND, "the cap is {p:?}, lit {lit:.1?}");
        }
    }
}

/// On satellite (no imagery yet) the disc is the earth sphere, lit as deck lights it (T2), at the
/// limb of the 6.3e6 m sphere, 1.1 % inside deck's surface; between the two limbs, space.
#[test]
fn v1_on_satellite_is_the_earth_sphere_below_the_surface() {
    let Some(gpu) = gpu() else { return };
    let image = app_shot(&gpu, &globe_options(V1.view(), Basemap::Satellite, SHOT_SIZE, 1.0));
    save(&image, "e18-v1-satellite.png");
    let frame = ViewFrame::new(&V1.view(), [1100.0, 720.0], 1.0, 0.0);
    let globe = frame.viewport.globe().expect("the globe");
    let ratio = EARTH_RATIO;
    assert_disc_at_the_limb(&image, &frame, 1.0, ratio, Basemap::Satellite, "V1 satellite");
    let (sphere, surface) = (limb(globe, 0.0, ratio), limb(globe, 0.0, 1.0));
    assert!(surface - sphere > 3.0, "{sphere} {surface}");
    assert_color(&image, (550.0 + (sphere + surface) / 2.0) as u32, 360, INK, 0);
}

/// Item E5's acceptance: at the same zoom the globe centred on latitude 60 is twice as large as
/// the one centred on the equator (deck's `scale = 2^zoom / (π · cos lat)`). What doubles is the
/// globe's radius at the view centre's distance; the disc itself grows less (about 1.7 times), by
/// perspective: the camera stands 1.5 viewport heights above the surface, so a disc of `t` focal
/// lengths is a globe of `R = 1.5 t (t + √(t² + 1))` viewport heights. Both shots through the
/// app's `--view`.
#[test]
fn latitude_60_shows_the_globe_twice_as_large_as_the_equator() {
    let Some(gpu) = gpu() else { return };
    let mut globe_px = Vec::new();
    let mut disc_px = Vec::new();
    for latitude in [0.0, 60.0] {
        let v = ViewState { latitude, ..V1.view() };
        let image = app_shot(&gpu, &globe_options(v, Basemap::Dark, SHOT_SIZE, 1.0));
        save(&image, &format!("e18-latitude-{latitude}.png"));
        let frame = ViewFrame::new(&v, [1100.0, 720.0], 1.0, 0.0);
        let globe = frame.viewport.globe().expect("the globe");
        let edge = assert_disc_at_the_limb(&image, &frame, 1.0, 1.0, Basemap::Dark, &format!("latitude {latitude}"));
        let height = 720.0;
        let t = edge / (1.5 * height);
        let radius = 1.5 * t * (t + (t * t + 1.0).sqrt()) * height;
        eprintln!("latitude {latitude}: disc {edge:.2} px, globe {radius:.2} px, 256 · scale = {:.2}", GLOBE_RADIUS * globe.scale);
        assert!((radius / (GLOBE_RADIUS * globe.scale) - 1.0).abs() < 0.01, "latitude {latitude}");
        globe_px.push(radius);
        disc_px.push(edge);
    }
    let ratio = globe_px[1] / globe_px[0];
    assert!((ratio - 2.0).abs() < 0.02, "the globe grew {ratio} times");
    assert!(disc_px[1] / disc_px[0] > 1.6, "the disc grew {} times", disc_px[1] / disc_px[0]);
}

/// Every named view through `--screenshot`'s path on the dark map with no tiles (E5's earth and
/// land alone): the flat views are land from edge to edge; on the globe views every pixel whose
/// ray hits the globe well inside its limb is land (past ±85.06° the lit earth sphere, T2) and
/// every one well outside is space, as E1's camera says (sampled every 10 px); V8 shows no seam
/// where the land meets itself at the antimeridian.
#[test]
fn every_named_view_shows_land_where_e1_puts_the_globe() {
    let Some(gpu) = gpu() else { return };
    for named in VIEWS {
        let image = app_shot(&gpu, &globe_options(named.view(), Basemap::Dark, SHOT_SIZE, 1.0));
        save(&image, &format!("e18-{}-dark.png", named.name.to_lowercase()));
        if named.is_flat() {
            assert_eq!(coverage(&image, Rect::whole(&image), LAND, 0), 1.0, "{} is land everywhere", named.name);
            continue;
        }
        let frame = ViewFrame::new(&named.view(), [1100.0, 720.0], 1.0, 0.0);
        let globe = frame.viewport.globe().expect("the globe");
        let (mut land, mut cap, mut space) = (0, 0, 0);
        for (x, y) in Rect::whole(&image).pixels().filter(|(x, y)| x % 10 == 5 && y % 10 == 5) {
            let xy = [f64::from(x) + 0.5, f64::from(y) + 0.5];
            let ratio = globe.ray_distance_to_globe_center_ratio(xy);
            if ratio < 0.985 {
                let Some((colour, tolerance)) = globe_colour(&frame, Basemap::Dark, 1.0, xy) else { continue };
                let p = pixel(&image, x, y);
                assert!(off_by(p, colour) <= tolerance, "{}: ({x}, {y}) on the globe is {p:?}, expected {colour:.1?} ± {tolerance}", named.name);
                if tolerance == 0.0 { land += 1 } else { cap += 1 }
            } else if ratio > 1.01 {
                assert_eq!(pixel(&image, x, y), INK, "{}: ({x}, {y}) is in space", named.name);
                space += 1;
            }
        }
        eprintln!("{}: {land} samples of land, {cap} of the lit polar caps, {space} of space", named.name);
        assert!(land > 100, "{}", named.name);
        if named.name == "V8" {
            let mut checked = 0;
            for lat in (-60..=60).step_by(5) {
                let [x, y, _] = globe.project([180.0, f64::from(lat), 0.0]);
                let on_the_picture = x > 4.0 && y > 4.0 && x < 1096.0 && y < 716.0;
                if on_the_picture && globe.ray_distance_to_globe_center_ratio([x, y]) < 0.985 {
                    assert_eq!(coverage(&image, Rect::around(&image, [x, y], 3.0), LAND, 0), 1.0, "V8: a seam at 180°, {lat}°");
                    checked += 1;
                }
            }
            assert!(checked >= 10, "V8: the antimeridian is on the globe at {checked} latitudes");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// E6: the path pipeline

/// A test layer of paths, each mesh in its own local Mercator frame with its own style.
struct Paths {
    pipeline: path::Path,
    meshes: Vec<(path::Mesh, path::Style, LocalMercator)>,
    slots: Vec<FrameSlot>,
}

impl Paths {
    fn layer(init: &Init<'_>, items: Vec<(PathBuilder, path::PathStyle)>) -> Box<dyn Layer> {
        let device = &init.gpu.device;
        let pipeline = path::Path::new(init, "e18 paths", DrawState::SURFACE_NO_CULL);
        let meshes = items
            .into_iter()
            .map(|(builder, style)| {
                let local = builder.local();
                (path::Mesh::new(device, "e18 path", &builder.finish()), pipeline.style(device, &style), local)
            })
            .collect();
        Box::new(Paths { pipeline, meshes, slots: Vec::new() })
    }
}

impl Layer for Paths {
    fn name(&self) -> &'static str {
        "e18 paths"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.slots = self.meshes.iter().map(|(_, _, l)| cx.frames.push(&cx.view.local(l.anchor, l.scale, 0.0))).collect();
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        self.pipeline.begin(pass);
        for ((mesh, style, _), slot) in self.meshes.iter().zip(&self.slots) {
            self.pipeline.draw_mesh(pass, cx, *slot, style, mesh);
        }
    }
}

/// A local Mercator frame round world point `centre` for a mesh `half` world units each way.
fn frame_around(centre: [f64; 2], half: f64) -> LocalMercator {
    let scale = power_of_two_scale(2.0 * half);
    LocalMercator::new([centre[0] - scale / 2.0, centre[1] + scale / 2.0].into(), scale)
}

/// A straight line in world Mercator through `centre`, `angle` degrees east of north, `half`
/// world units each way, cut into `pieces`, on the ground; in `local`.
fn line(local: &LocalMercator, centre: [f64; 2], angle: f64, half: f64, pieces: usize) -> Vec<[f64; 3]> {
    let (s, c) = angle.to_radians().sin_cos();
    (0..=pieces)
        .map(|i| {
            let t = -1.0 + 2.0 * i as f64 / pieces as f64;
            let [u, v] = local.from_world([centre[0] + s * half * t, centre[1] + c * half * t].into());
            [u, v, 0.0]
        })
        .collect()
}

/// Where the map point `world` lands on the screen, CSS pixels.
fn on_screen(vf: &ViewFrame, world: [f64; 2]) -> [f64; 2] {
    let [lon, lat] = world_to_lng_lat(world);
    let [x, y, _] = vf.viewport.project([lon, lat, 0.0]);
    [x, y]
}

/// Item E6's acceptance: a 1 px and a 0.6 px line keep their width at zoom 3 and 11 (the globe)
/// and 15 (the flat map), at pitch 0 and 60, measured by their coverage within 15 %. The line
/// runs into the view (8° east of north): deck's lines lie on the ground, so one running across a
/// pitched view foreshortens (E6 checks that against the camera).
#[test]
fn one_and_six_tenths_pixel_lines_keep_their_width() {
    let Some(gpu) = gpu() else { return };
    let size = [300.0, 300.0];
    for zoom in [3.0, 11.0, 15.0] {
        for pitch in [0.0, 60.0] {
            for px in [1.0, 0.6] {
                let v = view(STOCKHOLM, zoom, pitch, 0.0);
                let centre = lng_lat_to_world(STOCKHOLM);
                // 150 px each way, under 5° a piece on the globe
                let half = 150.0 / 2f64.powf(zoom);
                let local = frame_around(centre, half);
                let mut b = PathBuilder::new(local);
                b.push(&line(&local, centre, 8.0, half, 64), &Paint::solid(WHITE, px));
                let scene = SceneInput::new(v, size, 1.0, Basemap::Dark);
                let (image, vf) = map_shot(&gpu, &scene, 1.0, |init| vec![Paths::layer(init, vec![(b, path::PathStyle::PIXELS)])]);
                if px == 1.0 {
                    save(&image, &format!("e18-line-z{zoom}-p{pitch}.png"));
                }
                // the line's direction on the screen at the centre, and its length in a box of
                // 48 px round it (it crosses the box's full height)
                let (s, c) = 8f64.to_radians().sin_cos();
                let step = half / 100.0;
                let [a, b] = [-1.0, 1.0].map(|k| on_screen(&vf, [centre[0] + k * s * step, centre[1] + k * c * step]));
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let r = 24.0;
                let length = 2.0 * r * dx.hypot(dy) / dx.abs().max(dy.abs());
                let width = area(&image, Rect::around(&image, [150.0, 150.0], r), INK, WHITE) / length;
                let about = format!("zoom {zoom} pitch {pitch} {px} px: {width:.3} px wide");
                eprintln!("{about}");
                assert!((width / px - 1.0).abs() < 0.15, "{about}");
                assert_eq!(coverage(&image, Rect::new(0, 0, 20, 20), INK, 0), 1.0, "{about}: the ink round it");
            }
        }
    }
}

/// Item E6's acceptance: a dashed line shows the expected number of dashes. A 2 px line (half
/// width 1 px) of 41 segments of 10 px with dashes `[4, 3]` half widths: along the whole path the
/// pattern runs on across the joints (a dash every 7 px: 59), per segment it starts again at each
/// joint (42 runs); justified, a whole number of periods fits (60 and 42). Without the extension
/// it is one solid line. Counted as connected components.
#[test]
fn a_dashed_line_shows_the_expected_dashes() {
    let Some(gpu) = gpu() else { return };
    let v = view(ARLANDA, 15.0, 0.0, 0.0);
    let centre = lng_lat_to_world(ARLANDA);
    let half = 205.0 / 2f64.powf(15.0);
    let local = frame_around(centre, half);
    let points = line(&local, centre, 90.0, half, 41);
    let scene = SceneInput::new(v, [500.0, 60.0], 1.0, Basemap::Dark);
    let bright = |p: Rgba| p[0] > 142;
    for (dash, justified, expected) in [
        (path::Dash::Path, false, 59),
        (path::Dash::Segment, false, 42),
        (path::Dash::Path, true, 60),
        (path::Dash::Segment, true, 42),
        (path::Dash::None, false, 1),
    ] {
        let mut b = PathBuilder::measuring(local);
        b.push(&points, &Paint::solid(WHITE, 2.0).dashed([4.0, 3.0]));
        let style = path::PathStyle { dash, dash_justified: justified, ..path::PathStyle::PIXELS };
        let (image, _) = map_shot(&gpu, &scene, 1.0, |init| vec![Paths::layer(init, vec![(b, style)])]);
        save(&image, &format!("e18-dash-{dash:?}-{justified}.png").to_lowercase());
        let dashes = components(&image, Rect::whole(&image), bright);
        assert_eq!(dashes.len(), expected, "{dash:?} justified {justified}: {:?}", dashes.iter().map(|d| d.bounds).collect::<Vec<_>>());
        // the line spans x 45 to 455 along the middle two rows
        let all = bounds(&image, Rect::whole(&image), bright).expect("drawn");
        assert_eq!((all.x0, all.x1, all.y0, all.y1), (45, 455, 29, 31), "{dash:?} justified {justified}");
        if dash == path::Dash::Path && !justified {
            assert!(dashes.iter().enumerate().all(|(i, d)| d.bounds == Rect::new(45 + 7 * i as u32, 29, 4, 2)), "a dash every 7 px");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// E7: the mesh pipeline and the shadow variants

/// Where a mesh layer's instances go in a frame's view.
type Place = Box<dyn Fn(&ViewFrame) -> Vec<mesh::Instance>>;

/// A test layer: one shape, its instances placed in each frame's view.
struct Meshes {
    pipeline: mesh::MeshPipeline,
    shape: mesh::Shape,
    style: mesh::Style,
    instances: mesh::Instances,
    place: Place,
}

impl Meshes {
    fn layer(init: &Init<'_>, state: DrawState, (vertices, indices): &(Vec<mesh::Vertex>, Vec<u32>), material: mesh::Material, place: Place) -> Box<dyn Layer> {
        let device = &init.gpu.device;
        let pipeline = mesh::MeshPipeline::instanced(init, "e18 meshes", state);
        let style = pipeline.style(device, &mesh::StyleUniform::new(&material, 1.0, [255; 4]));
        let shape = mesh::Shape::new(device, "e18 shape", vertices, Some(indices));
        Box::new(Meshes { pipeline, shape, style, instances: mesh::Instances::new(device, "e18 instances"), place })
    }
}

impl Layer for Meshes {
    fn name(&self) -> &'static str {
        "e18 meshes"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.pipeline.set_lights(&cx.gpu.queue, &mesh::LightRig::new(), cx.view);
        self.instances.write(&cx.gpu.device, &cx.gpu.queue, &(self.place)(cx.view));
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {
        self.pipeline.draw(pass, &self.style, &self.shape, &self.instances);
    }
}

/// A cube of side 1 about the origin, its faces' normals outward, counter-clockwise seen from
/// outside (front faces in the culled presets).
fn unit_cube() -> (Vec<mesh::Vertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let mut normal = [0.0f32; 3];
            normal[axis] = sign;
            let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
            let base = vertices.len() as u32;
            for (s, t) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
                let mut position = [0.0f32; 3];
                position[axis] = 0.5 * sign;
                position[a] = s;
                position[b] = t * sign;
                vertices.push(mesh::Vertex { position, normal, color: mesh::Vertex::WHITE });
            }
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    (vertices, indices)
}

/// Item E7's acceptance: a unit cube instance standing at a known place shows its top and two
/// sides in three different brightnesses; with the material at depth 0 every face is exactly the
/// instance colour; a hover highlight mixes in by its alpha. On the flat map (zoom 16) and on the
/// globe (zoom 10), pitched 50° and turned 30°, back faces culled.
#[test]
fn a_lit_cube_shows_three_faces_and_at_depth_0_its_colour() {
    let Some(gpu) = gpu() else { return };
    let size = [240.0, 240.0];
    let color = [200, 170, 120, 255];
    let highlight = [55, 142, 240, 150];
    let cube = unit_cube();
    for (name, v) in [("flat", view(ARLANDA, 16.0, 50.0, 30.0)), ("globe", view(STOCKHOLM, 10.0, 50.0, 30.0))] {
        let vf = ViewFrame::new(&v, size, 1.0, 0.0);
        // about 70 px across, standing on the ground at the view centre
        let side = 70.0 * vf.viewport.meters_per_pixel();
        let position = [v.longitude, v.latitude, side / 2.0];
        let shot = |material: mesh::Material, highlight: Rgba| {
            let scene = SceneInput::new(v, size, 1.0, Basemap::Dark);
            let place = move |vf: &ViewFrame| vec![mesh::Instance::at(vf, position, mesh::orientation_matrix([0.0; 3], [side; 3]), Default::default(), color, highlight)];
            map_shot(&gpu, &scene, 1.0, |init| vec![Meshes::layer(init, DrawState::DEFAULT, &cube, material, Box::new(place))]).0
        };
        let lit = shot(mesh::Material::aircraft(1.0), mesh::Instance::NO_HIGHLIGHT);
        let flat = shot(mesh::Material::aircraft(0.0), mesh::Instance::NO_HIGHLIGHT);
        let hovered = shot(mesh::Material::aircraft(0.0), highlight);
        save(&lit, &format!("e18-cube-{name}.png"));
        // the faces turned toward the camera (it looks from the south-south-west, from above):
        // top, south and west, at their centres
        let enu = mesh::local_frame(&vf, [v.longitude, v.latitude]);
        let middle = vf.common_offset([v.longitude, v.latitude], position[2]).as_dvec3();
        let mixed = byte(over(color, highlight, highlight[3], 1.0));
        let mut faces = Vec::new();
        for (face, out) in [("top", enu.z_axis), ("south", -enu.y_axis), ("west", -enu.x_axis)] {
            let at = middle + out * (side / 2.0);
            let clip = model::project_common(&vf.globals, at.as_vec3().into());
            let [x, y] = model::clip_to_pixels(&vf.globals, clip).map(|c| c.floor() as u32);
            let p = pixel(&lit, x, y);
            eprintln!("{name} {face} at ({x}, {y}): {p:?}");
            assert_eq!(p[3], 255, "{name} {face}");
            assert_color(&flat, x, y, color, 0);
            assert_color(&hovered, x, y, mixed, 1);
            faces.push(u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]));
        }
        for i in 0..3 {
            for j in 0..i {
                assert!(faces[i].abs_diff(faces[j]) >= 6, "{name}: faces {j} and {i} look alike: {faces:?}");
            }
        }
        assert_eq!(coverage(&lit, Rect::new(0, 0, 20, 20), INK, 0), 1.0, "{name}: the ink round it");
    }
}

/// A test layer of squares of ground, each its own fill mesh in its own frame.
struct Squares {
    fill: fill::Fill,
    style: fill::Style,
    squares: Vec<(fill::Mesh<fill::Vertex>, [f64; 2], f64)>,
    slots: Vec<FrameSlot>,
}

impl Squares {
    /// Squares of `scale` world units with their north-west corners at `anchors`, in `color`.
    fn layer(init: &Init<'_>, fill: fill::Fill, anchors: &[[f64; 2]], scale: f64, color: Rgba) -> Box<dyn Layer> {
        let device = &init.gpu.device;
        let vertices: Vec<fill::Vertex> = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|[u, v]| fill::Vertex { position: [u, v, 0.0], color }).into();
        let squares = anchors.iter().map(|&a| (fill::Mesh::new(device, "e18 square", &vertices, &[0, 1, 2, 0, 2, 3]), a, scale)).collect();
        let style = fill.style(device, fill::OPAQUE);
        Box::new(Squares { fill, style, squares, slots: Vec::new() })
    }
}

impl Layer for Squares {
    fn name(&self) -> &'static str {
        "e18 squares"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.slots = self.squares.iter().map(|(_, anchor, scale)| cx.frames.push(&cx.view.local((*anchor).into(), *scale, 0.0))).collect();
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        for ((mesh, _, _), slot) in self.squares.iter().zip(&self.slots) {
            self.fill.draw(pass, cx, *slot, &self.style, mesh);
        }
    }
}

/// Item E7's acceptance: two overlapping shadow quads drawn with the `SHADOW` state (the shadow
/// fill pipeline) darken their overlap once; as the control, without the stencil rule
/// (`SURFACE_NO_CULL`) the overlap darkens twice.
#[test]
fn overlapping_shadows_darken_their_overlap_once() {
    let Some(gpu) = gpu() else { return };
    let size = [240.0, 200.0];
    let v = view(ARLANDA, 15.0, 40.0, 20.0);
    let centre = lng_lat_to_world(ARLANDA);
    let scale = power_of_two_scale(50.0 / 2f64.powf(v.zoom));
    // the second square over the first's east half
    let a = [centre[0] - scale * 0.75, centre[1] + scale * 0.5];
    let anchors = [a, [a[0] + scale * 0.5, a[1]]];
    let shadow_colour = [4, 8, 16, 128];
    let once = byte(over(INK, shadow_colour, shadow_colour[3], 1.0));
    let twice = byte(over(once, shadow_colour, shadow_colour[3], 1.0));
    let scene = SceneInput::new(v, size, 1.0, Basemap::Dark);
    for stencil in [true, false] {
        let (image, vf) = map_shot(&gpu, &scene, 1.0, |init| {
            let pipeline = if stencil { shadow::fill(init, "e18 shadows") } else { fill::Fill::new(init, "e18 control", DrawState::SURFACE_NO_CULL) };
            vec![Squares::layer(init, pipeline, &anchors, scale, shadow_colour)]
        });
        if stencil {
            save(&image, "e18-shadows.png");
        }
        // the first square alone, the overlap, the second alone: along the squares' middle
        let at = |x: f64| on_screen(&vf, [a[0] + scale * x, a[1] - scale * 0.5]).map(|c| c.floor() as u32);
        let [left, overlap, right] = [0.25, 0.75, 1.25].map(at);
        assert_color(&image, left[0], left[1], once, 1);
        assert_color(&image, right[0], right[1], once, 1);
        assert_color(&image, overlap[0], overlap[1], if stencil { once } else { twice }, 1);
        assert_color(&image, 2, 2, INK, 0);
    }
}

// ---------------------------------------------------------------------------------------------
// E8: the icon pipeline

/// The test atlas: 64 × 32 texels; the cell `UP` (16 × 16 at 2, 2) red over blue, inside a block
/// of its colours 2 texels wider each way so its quad's edge samples the cell's colours.
const ATLAS: [u32; 2] = [64, 32];
const UP: icon::IconDef = icon::IconDef::cell(2.0, 2.0, 16.0, 16.0, false);

fn atlas_texels() -> Vec<u8> {
    (0..ATLAS[1])
        .flat_map(|y| (0..ATLAS[0]).map(move |x| (x, y)))
        .flat_map(|(x, y)| match (x < 20, y) {
            (true, 0..10) => [255, 0, 0, 255],
            (true, 10..20) => [0, 0, 255, 255],
            _ => [0, 0, 0, 0],
        })
        .collect()
}

/// A test layer of icons: one batch per style, each with one icon, placed in each frame's view.
struct Icons {
    pipeline: icon::IconPipeline,
    batches: Vec<(icon::Batch, icon::IconStyle, [f64; 2], f64)>,
}

impl Icons {
    fn layer(init: &Init<'_>, icons: &[(icon::IconStyle, [f64; 2], f64)]) -> Box<dyn Layer> {
        let gpu = init.gpu;
        let pipeline = icon::IconPipeline::new(init, "e18 icons", DrawState::NO_CULL);
        let texture = rustradar_map::pipelines::bitmap::upload_rgba8(gpu, "e18 atlas", ATLAS, &atlas_texels(), false);
        let atlas = texture.create_view(&Default::default());
        let batches = icons.iter().map(|&(style, at, size)| (pipeline.batch(&gpu.device, "e18 icons", &atlas, ATLAS), style, at, size)).collect();
        Box::new(Icons { pipeline, batches })
    }
}

impl Layer for Icons {
    fn name(&self) -> &'static str {
        "e18 icons"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        for (batch, style, at, size) in &mut self.batches {
            batch.set_style(&cx.gpu.queue, style, cx.view);
            batch.set_instances(&cx.gpu.device, &cx.gpu.queue, &[icon::Instance::at(cx.view, [at[0], at[1], 0.0], &UP, *size, 0.0, WHITE)]);
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {
        self.pipeline.begin(pass);
        for (batch, ..) in &self.batches {
            self.pipeline.draw_batch(pass, batch);
        }
    }
}

/// Item E8's acceptance: a 48 px billboard icon stays upright and the same size at pitch 0 and 60
/// (and turned 30°); a ground-plane icon beside it lies on the map, so pitched 60° it foreshortens
/// to about half its height and area. On the globe (zoom 5) and the flat map (zoom 14).
#[test]
fn a_billboard_icon_stays_upright_and_a_ground_icon_foreshortens() {
    let Some(gpu) = gpu() else { return };
    const PX: f64 = 48.0;
    let size = [240.0, 240.0];
    let red = |p: Rgba| p[0] > 142 && p[2] < 142;
    let blue = |p: Rgba| p[2] > 142 && p[0] < 142;
    let icon_px = |p: Rgba| p[0] > 142 || p[2] > 142;
    for zoom in [5.0, 14.0] {
        let mut level = None;
        for (pitch, bearing) in [(0.0, 0.0), (60.0, 0.0), (60.0, 30.0)] {
            let v = view(STOCKHOLM, zoom, pitch, bearing);
            let vp = rustradar_scene::camera::viewport_for(&v, size);
            let west = vp.unproject([60.0, 120.0], 0.0);
            let east = vp.unproject([180.0, 120.0], 0.0);
            let scene = SceneInput::new(v, size, 1.0, Basemap::Dark);
            let (image, _) = map_shot(&gpu, &scene, 1.0, |init| vec![Icons::layer(init, &[(icon::IconStyle::DEFAULT, west, PX), (icon::IconStyle::GROUND, east, PX)])]);
            save(&image, &format!("e18-icons-z{zoom}-p{pitch}-b{bearing}.png"));
            let about = format!("zoom {zoom} pitch {pitch} bearing {bearing}");
            let (left, right) = (Rect::new(0, 0, 120, 240), Rect::new(120, 0, 120, 240));

            // the billboard: one square of 48 px, red straight above blue
            let parts = components(&image, left, icon_px);
            assert_eq!(parts.len(), 1, "{about}: {parts:?}");
            let billboard = &parts[0];
            let (w, h) = (f64::from(billboard.bounds.width()), f64::from(billboard.bounds.height()));
            assert!((w - PX).abs() <= 1.0 && (h - PX).abs() <= 1.0, "{about}: {billboard:?}");
            assert!((billboard.bounds.centre()[0] - 60.0).abs() <= 1.0 && (billboard.bounds.centre()[1] - 120.0).abs() <= 1.0, "{about}: {billboard:?}");
            let (r, b) = (&components(&image, left, red)[0], &components(&image, left, blue)[0]);
            assert!((r.centroid[0] - b.centroid[0]).abs() < 0.5 && (b.centroid[1] - r.centroid[1] - PX / 2.0).abs() < 1.5, "{about}: red {r:?} over blue {b:?}");
            match level {
                None => level = Some(billboard.pixels),
                Some(n) => assert!((billboard.pixels as f64 / n as f64 - 1.0).abs() < 0.03, "{about}: {} px against {n} at pitch 0", billboard.pixels),
            }

            // the ground-plane icon
            let ground = &components(&image, right, icon_px)[0];
            let ratio = ground.pixels as f64 / billboard.pixels as f64;
            eprintln!("{about}: billboard {:?}, ground {:?}, area ratio {ratio:.3}", billboard.bounds, ground.bounds);
            if pitch == 0.0 {
                assert!((ratio - 1.0).abs() < 0.03, "{about}: flat on the screen the ground icon is the billboard's square ({ratio})");
            } else if bearing == 0.0 {
                let tall = f64::from(ground.bounds.height()) / PX;
                assert!(tall > 0.4 && tall < 0.6, "{about}: the ground icon is {tall} as tall");
                assert!(ratio > 0.4 && ratio < 0.6, "{about}: the ground icon has {ratio} of the area");
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// E9: the circle pipeline

/// A test layer of circles: one pipeline and style per item, its circles placed in each frame's
/// view at `(at, radius, fill, line, line width)`.
type Circle = ([f64; 2], f64, Rgba, Rgba, f64);

struct Circles {
    items: Vec<(circle::Circle, circle::Style, circle::Instances, Vec<Circle>)>,
}

impl Circles {
    fn layer(init: &Init<'_>, items: Vec<(circle::CircleStyle, DrawState, Vec<Circle>)>) -> Box<dyn Layer> {
        let device = &init.gpu.device;
        let items = items
            .into_iter()
            .map(|(style, state, circles)| {
                let pipeline = circle::Circle::new(init, "e18 circles", state);
                let style = pipeline.style(device, &style);
                (pipeline, style, circle::Instances::new(device, "e18 circles"), circles)
            })
            .collect();
        Box::new(Circles { items })
    }
}

impl Layer for Circles {
    fn name(&self) -> &'static str {
        "e18 circles"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        for (_, _, instances, circles) in &mut self.items {
            let list: Vec<circle::Instance> =
                circles.iter().map(|&(at, radius, fill, line, width)| circle::Instance::at(cx.view, at, 0.0).radius(radius).fill(fill).line(line, width)).collect();
            instances.write(&cx.gpu.device, &cx.gpu.queue, &list);
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {
        for (pipeline, style, instances, _) in &self.items {
            pipeline.draw(pass, style, instances);
        }
    }
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Item E9's acceptance: the ship and train selection, a ring of 2 px stroke `[55,142,240,235]`
/// over a halo `[55,142,240,56]` of radius 20 px, billboards with the `LABEL` state, over the land:
/// inside the halo its colour over the land, on the ring's middle line the ring over the halo's
/// smoothed edge, outside the land untouched; drawn alone the ring is 2 px wide. On the globe
/// (zoom 9) and on the flat map pitched 45° and turned 30°, at scale 1 and 2.
#[test]
fn a_ring_over_a_halo() {
    let Some(gpu) = gpu() else { return };
    const BLUE: Rgba = [55, 142, 240, 255];
    const HALO: Rgba = [55, 142, 240, 56];
    const RING: Rgba = [55, 142, 240, 235];
    let radius = 20.0;
    let size = [120.0, 100.0];
    let billboard = circle::CircleStyle { billboard: true, ..circle::CircleStyle::PIXELS };
    let halo_style = circle::CircleStyle { filled: true, stroked: false, ..billboard };
    let ring_style = circle::CircleStyle { filled: false, stroked: true, ..billboard };
    for (name, v) in [("globe", view(STOCKHOLM, 9.0, 0.0, 0.0)), ("flat", view(ARLANDA, 13.5, 45.0, 30.0))] {
        for s in [1.0, 2.0] {
            let at = [v.longitude, v.latitude];
            let mut scene = SceneInput::new(v, size, s, Basemap::Dark);
            scene.earth = earth::Input::SHOWN;
            let halo = (halo_style, DrawState::LABEL, vec![(at, radius, HALO, circle::DEFAULT_COLOR, 1.0)]);
            let ring = (ring_style, DrawState::LABEL, vec![(at, radius, circle::DEFAULT_COLOR, RING, 2.0)]);
            let (image, _) = map_shot(&gpu, &scene, s, |init| vec![Circles::layer(init, vec![halo.clone(), ring.clone()])]);
            if s == 2.0 {
                save(&image, &format!("e18-ring-{name}.png"));
            }
            let what = format!("{name} at scale {s}");
            // the pixel `d` CSS px right of the centre, and how far its centre is from it
            let probe = |d: f64| {
                let x = ((60.0 + d) * s) as u32;
                let y = (50.0 * s) as u32;
                (x, y, ((f64::from(x) + 0.5) / s - 60.0).hypot((f64::from(y) + 0.5) / s - 50.0))
            };
            let (x, y, _) = probe(10.0);
            assert_color(&image, x, y, byte(over(LAND, BLUE, HALO[3], 1.0)), 1);
            let (x, y, d) = probe(radius);
            let under = byte(over(LAND, BLUE, HALO[3], smoothstep(d - 0.5, d + 0.5, radius)));
            assert_color(&image, x, y, byte(over(under, BLUE, RING[3], 1.0)), 2);
            let (x, y, _) = probe(radius + 3.0);
            assert_color(&image, x, y, LAND, 0);
            assert_eq!(coverage(&image, Rect::new(0, 0, 10, 10), LAND, 0), 1.0, "{what}: the land round it");

            // the ring alone: its coverage over the land is an annulus 2 px wide round 20 px
            let (alone, _) = map_shot(&gpu, &scene, s, |init| vec![Circles::layer(init, vec![ring.clone()])]);
            let full = byte(over(LAND, BLUE, RING[3], 1.0));
            let covered = area(&alone, Rect::around(&alone, [60.0 * s, 50.0 * s], (radius + 4.0) * s), LAND, full);
            let width = covered / (TAU * radius * s * s);
            eprintln!("{what}: the ring is {width:.3} px wide");
            assert!((width - 2.0).abs() < 0.1, "{what}: the ring is {width} px wide");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// E10: SDF text

/// A test layer of labels in the app's label style (SDF, outline 3), on one atlas.
struct Labels {
    pipeline: text::TextPipeline,
    cpu: GlyphAtlas,
    atlas: text::AtlasTexture,
    batch: Option<text::TextBatch>,
    labels: Vec<(&'static str, [f64; 2], f64, Rgba)>,
}

impl Labels {
    fn layer(init: &Init<'_>, labels: Vec<(&'static str, [f64; 2], f64, Rgba)>) -> Box<dyn Layer> {
        let pipeline = text::TextPipeline::new(init, "e18 labels", DrawState::LABEL);
        let mut cpu = GlyphAtlas::new(FontSettings::LABELS);
        let atlas = text::AtlasTexture::new(init.gpu, "e18 glyphs", &mut cpu);
        Box::new(Labels { pipeline, cpu, atlas, batch: None, labels })
    }
}

impl Layer for Labels {
    fn name(&self) -> &'static str {
        "e18 labels"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        let mut glyphs = Vec::new();
        for &(words, [lon, lat], size, color) in &self.labels {
            let laid = self.cpu.layout(Face::LABEL, words, &TextLayout::DEFAULT);
            glyphs.extend(text::Label::at(cx.view, [lon, lat, 0.0], size, color).glyphs(&laid));
        }
        self.atlas.sync(cx.gpu, &mut self.cpu);
        let batch = self.batch.get_or_insert_with(|| self.pipeline.batch(&cx.gpu.device, "e18 labels", &self.atlas));
        batch.set_style(&cx.gpu.queue, &text::TextStyle::LABELS, &text::BackgroundStyle::DEFAULT, &FontSettings::LABELS, cx.view);
        batch.set_glyphs(&cx.gpu.device, &cx.gpu.queue, &glyphs);
        batch.set_backgrounds(&cx.gpu.device, &cx.gpu.queue, &[]);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {
        if let Some(batch) = &self.batch {
            self.pipeline.draw(pass, batch);
        }
    }
}

/// The coverage of `words` laid out by epaint in Overpass at weight 600 (the app's `ui-600` family,
/// plan 1.5) at `size` points and `ppp` pixels per point, the first glyph's pen at `pen` (CSS px,
/// at its baseline): its glyphs' atlas texels where epaint's tessellator puts them, on a picture
/// of `picture` device pixels.
fn epaint_coverage(words: &str, size: f32, ppp: f32, [pen_x, baseline]: [f64; 2], picture: [usize; 2]) -> Vec<f64> {
    use egui::epaint::text::{FontData, FontDefinitions, FontFamily, FontId, FontTweak, Fonts, TextOptions, VariationCoords};
    let mut definitions = FontDefinitions::empty();
    let tweak = FontTweak { coords: VariationCoords::new([(b"wght", 600.0)]), ..Default::default() };
    definitions.font_data.insert("overpass".into(), std::sync::Arc::new(FontData::from_static(OVERPASS[0]).tweak(tweak)));
    let family = FontFamily::Name("ui-600".into());
    definitions.families.insert(family.clone(), vec!["overpass".into()]);
    let mut fonts = Fonts::new(TextOptions::default(), definitions);
    let galley = fonts.with_pixels_per_point(ppp).layout_no_wrap(words.to_string(), FontId::new(size, family), egui::Color32::WHITE);
    let atlas = fonts.image();
    let row = &galley.rows[0];
    let first = &row.glyphs[0];
    // the galley where its first pen and baseline land on ours, on whole pixels as egui paints it
    let ppp64 = f64::from(ppp);
    let snap = |v: f64| (v * ppp64).round() / ppp64;
    let origin = [snap(pen_x - f64::from(row.pos.x + first.pos.x)), snap(baseline - f64::from(row.pos.y + first.pos.y))];
    let [width, height] = picture;
    let mut coverage = vec![0.0f64; width * height];
    for g in &row.glyphs {
        let uv = g.uv_rect;
        if uv.is_nothing() {
            continue;
        }
        // `tessellate_glyphs`: the glyph's left top rounded to pixels
        let lt = [((g.pos.x + uv.offset.x) * ppp).round() / ppp, ((g.pos.y + uv.offset.y) * ppp).round() / ppp];
        let at = [((origin[0] + f64::from(row.pos.x + lt[0])) * ppp64).round() as i64, ((origin[1] + f64::from(row.pos.y + lt[1])) * ppp64).round() as i64];
        for ty in uv.min[1]..uv.max[1] {
            for tx in uv.min[0]..uv.max[0] {
                let (x, y) = (at[0] + i64::from(tx - uv.min[0]), at[1] + i64::from(ty - uv.min[1]));
                if x < 0 || y < 0 || x >= width as i64 || y >= height as i64 {
                    continue;
                }
                let a = f64::from(atlas.pixels[usize::from(ty) * atlas.size[0] + usize::from(tx)].a()) / 255.0;
                let i = y as usize * width + x as usize;
                coverage[i] = coverage[i].max(a);
            }
        }
    }
    coverage
}

/// Item E10's acceptance: "Stockholm" at 14 px in `[241,238,231,245]` with the labels' dark halo
/// (outline 3 in `[6,12,19,220]`); its ink box is within 1 px of the same string laid out by
/// epaint at 14 px in `ui-600`, with the pen and the baseline where deck's layout puts them. At
/// map scale 1 and 2, on the dark map at V3.
#[test]
fn stockholm_at_14_px_has_a_dark_halo_and_epaints_ink_box() {
    let Some(gpu) = gpu() else { return };
    const CITY: Rgba = [241, 238, 231, 245];
    for s in [1.0, 2.0] {
        let scene = SceneInput::new(V3.view(), [240.0, 120.0], s, Basemap::Dark);
        let (image, vf) = map_shot(&gpu, &scene, s, |init| vec![Labels::layer(init, vec![("Stockholm", STOCKHOLM, 14.0, CITY)])]);
        save(&image, &format!("e18-stockholm@{s}.png"));
        let what = format!("scale {s}");
        let [ax, ay, _] = vf.viewport.project([STOCKHOLM[0], STOCKHOLM[1], 0.0]);
        // deck's pen and baseline (anchor middle, baseline center): the row starts half its width
        // left of the anchor, its baseline `baseline_offset` atlas px (at 64 px) below it
        let mut atlas = GlyphAtlas::new(FontSettings::LABELS);
        let laid = atlas.layout(Face::LABEL, "Stockholm", &TextLayout::DEFAULT);
        let pen = [ax + laid.box_px(14.0)[0], ay + atlas.baseline_offset(Face::LABEL) * 14.0 / 64.0];
        let picture = [image.width as usize, image.height as usize];
        let theirs = coverage_box(&epaint_coverage("Stockholm", 14.0, s as f32, pen, picture), picture).expect("epaint's ink");
        // the fill over the halo: from the halo's green (12 at 220/255 over the ink) to the fill's
        // (238 at 245/255, drawn in two passes)
        let (halo_g, fill_g) = (12.0 * 220.0 / 255.0 + 30.0 * 35.0 / 255.0, 237.7);
        let ours = coverage_box(&weights(&image, |p| (f64::from(p[1]) - halo_g) / (fill_g - halo_g)), picture).expect("our ink");
        eprintln!("{what}: ink box {:?}, epaint's {:?} (CSS px)", ours.map(|v| v / s), theirs.map(|v| v / s));
        for (side, (a, b)) in ["left", "top", "right", "bottom"].iter().zip(ours.iter().zip(&theirs)) {
            assert!((a - b).abs() / s <= 1.0, "{what}: the ink box's {side} is {} px, epaint's {} px", a / s, b / s);
        }
        // the fill, nearly opaque: the brightest pixel
        let whole = Rect::whole(&image);
        let (darkest, brightest) = darkest_and_brightest(&image, whole).expect("pixels");
        let fill = byte(std::array::from_fn(|c| f64::from(CITY[c]) * 0.961 + (f64::from(CITY[c]) * 0.961 + 30.0 * 0.039) * 0.039));
        assert!(close(brightest, fill, 4), "{what}: the fill is {brightest:?}, not {fill:?}");
        // the halo: darker than the map round the letters, and only close to them
        assert!(darkest[1] <= 18 && darkest[2] <= 24, "{what}: the halo's darkest is {darkest:?}");
        let ink = bounds(&image, whole, |p| p[1] > 122).expect("ink");
        let halo = bounds(&image, whole, |p| p[1] < 24).expect("a halo");
        for (h, i) in [(halo.x0, ink.x0), (halo.y0, ink.y0), (halo.x1, ink.x1), (halo.y1, ink.y1)] {
            assert!(f64::from(h.abs_diff(i)) <= 2.0 * s, "{what}: the halo {halo:?} strays from the ink {ink:?}");
        }
        assert!(count(&image, whole, |p| p[1] < 24) as f64 > 0.4 * count(&image, whole, |p| p[1] > 122) as f64, "{what}: a thin halo");
        assert_eq!(coverage(&image, Rect::new(0, 0, 10, 10), INK, 0), 1.0, "{what}: the map round it");
    }
}
