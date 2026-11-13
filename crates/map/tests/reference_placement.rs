//! Item F4: the dark map at V3 against today's pictures (`docs/port/reference/dark-v3-*.png`), on both of their
//! canvases. The port's V3 stands 4–5 px lower than those pictures (T1's note, the stages 8-10 status's visual
//! difference 2) although its camera puts every point where deck.gl's own viewport does
//! (`crates/scene/tests/deck_placement.rs`): the pictures were drawn by Chromium on SwiftShader, whose sin and cos
//! in deck's globe vertex shader put the map 3.5 to 6 px higher across the canvas at zoom 9 (recorded per pixel
//! in `crates/scene/tests/data/deck/placement.json` by `scripts/port/deck-placement.mjs`, from deck's own shader
//! code and checked against a deck layer's drawing).
//!
//! These tests draw V3 from the fixtures, find the shift that best lays it over today's picture (whole pixels,
//! then a parabola through the neighbours for the fraction), and check that
//! - as drawn, it is the shift SwiftShader's error predicts for that part of the canvas (within 0.35 px), and
//! - moved by SwiftShader's recorded error, the best shift is 0 px (within 0.2 px) and more of today's water and
//!   land is matched than at any whole-pixel shift of the plain picture.
//!
//! The pictures are compared as water fractions: the dark map's sea `(30,30,30)` is 1, its land `(50,50,50)` 0,
//! the greys between a shore pixel's mix; roads, names and the UI (any other colour) are left out.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::fixture_tiles::FixtureTiles;
use common::shots::{self, V3, gpu};
use rustradar_core::geo::Basemap;
use rustradar_map::SceneInput;
use rustradar_map::headless::Image;
use rustradar_map::layers::{earth, vector_map};
use serde_json::Value;

/// A picture as water fractions, `NaN` where a pixel is neither sea, land nor a shore between them.
struct Water {
    width: usize,
    height: usize,
    w: Vec<f64>,
}

impl Water {
    fn of(image: &Image) -> Water {
        let w = image
            .rgba
            .chunks_exact(4)
            .map(|p| {
                let l = (f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])) / 3.0;
                if (29.5..=50.5).contains(&l) { ((50.0 - l) / 20.0).clamp(0.0, 1.0) } else { f64::NAN }
            })
            .collect();
        Water { width: image.width as usize, height: image.height as usize, w }
    }

    fn at(&self, x: i64, y: i64) -> f64 {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return f64::NAN;
        }
        self.w[y as usize * self.width + x as usize]
    }

    /// Bilinear between the four pixels round `(x, y)` (pixel centres at `i + 0.5`), `NaN` unless all four are
    /// sea, land or shore.
    fn sample(&self, x: f64, y: f64) -> f64 {
        let (x, y) = (x - 0.5, y - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let (a, b, c, d) = (self.at(x0, y0), self.at(x0 + 1, y0), self.at(x0, y0 + 1), self.at(x0 + 1, y0 + 1));
        a * (1.0 - fx) * (1.0 - fy) + b * fx * (1.0 - fy) + c * (1.0 - fx) * fy + d * fx * fy
    }

    /// What Chromium on SwiftShader would have drawn from this picture: each pixel `q` shows what lies at
    /// `q − shift(q)` here.
    fn moved(&self, shift: &Shift) -> Water {
        let mut w = Vec::with_capacity(self.w.len());
        for y in 0..self.height {
            for x in 0..self.width {
                let (cx, cy) = (x as f64 + 0.5, y as f64 + 0.5);
                let [dx, dy] = shift.at(cx, cy);
                w.push(self.sample(cx - dx, cy - dy));
            }
        }
        Water { width: self.width, height: self.height, w }
    }
}

/// SwiftShader's error at a reference view: drawn minus deck's project, in CSS pixels, on a grid over the canvas.
struct Shift {
    width: f64,
    height: f64,
    nx: usize,
    ny: usize,
    d: Vec<[f64; 2]>,
}

impl Shift {
    fn recorded(name: &str) -> Shift {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scene/tests/data/deck/placement.json");
        let data: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("placement.json")).expect("placement.json parses");
        let views = data["swiftshader"]["views"].as_array().expect("swiftshader.views");
        let view = views.iter().find(|v| v["name"] == name).unwrap_or_else(|| panic!("no {name} in placement.json"));
        let n = |v: &Value| v.as_f64().expect("a number");
        let grid = view["grid"].as_array().expect("grid");
        let d = view["shift"].as_array().expect("shift").iter().map(|p| [n(&p[0]), n(&p[1])]).collect();
        Shift { width: n(&view["width"]), height: n(&view["height"]), nx: n(&grid[0]) as usize, ny: n(&grid[1]) as usize, d }
    }

    /// Bilinear between the grid's points (`i / (nx − 1)` of the width, edges included).
    fn at(&self, x: f64, y: f64) -> [f64; 2] {
        let u = (x / self.width * (self.nx - 1) as f64).clamp(0.0, (self.nx - 1) as f64 - 1e-9);
        let v = (y / self.height * (self.ny - 1) as f64).clamp(0.0, (self.ny - 1) as f64 - 1e-9);
        let (i, j) = (u.floor() as usize, v.floor() as usize);
        let (fu, fv) = (u - i as f64, v - j as f64);
        let g = |i: usize, j: usize| self.d[j * self.nx + i];
        let (a, b, c, d) = (g(i, j), g(i + 1, j), g(i, j + 1), g(i + 1, j + 1));
        [0, 1].map(|k| a[k] * (1.0 - fu) * (1.0 - fv) + b[k] * fu * (1.0 - fv) + c[k] * (1.0 - fu) * fv + d[k] * fu * fv)
    }

    /// The mean over a rectangle of pixels.
    fn mean(&self, [x0, y0, x1, y1]: [usize; 4]) -> [f64; 2] {
        let mut sum = [0.0, 0.0];
        for y in y0..y1 {
            for x in x0..x1 {
                let d = self.at(x as f64 + 0.5, y as f64 + 0.5);
                sum = [sum[0] + d[0], sum[1] + d[1]];
            }
        }
        let n = ((x1 - x0) * (y1 - y0)) as f64;
        [sum[0] / n, sum[1] / n]
    }
}

/// The best offset of `ours` over `theirs` inside `rect`: `ours(x + dx, y + dy)` matches `theirs(x, y)`.
#[derive(Debug)]
struct Fit {
    /// The best whole-pixel offset (least squared difference of the water fractions).
    whole: [i64; 2],
    /// With the fraction from a parabola through its neighbours.
    sub: [f64; 2],
    /// The share of pixels on the same side of the shore (water fraction over or under ½) at `whole`.
    agreement: f64,
}

fn fit(ours: &Water, theirs: &Water, [x0, y0, x1, y1]: [usize; 4]) -> Fit {
    const DX: std::ops::RangeInclusive<i64> = -3..=3;
    const DY: std::ops::RangeInclusive<i64> = -3..=8;
    let cost = |dx: i64, dy: i64| -> (f64, f64) {
        let (mut sum, mut same, mut n) = (0.0, 0usize, 0usize);
        for y in y0..y1 {
            for x in x0..x1 {
                let t = theirs.w[y * theirs.width + x];
                let o = ours.at(x as i64 + dx, y as i64 + dy);
                if t.is_nan() || o.is_nan() {
                    continue;
                }
                sum += (o - t) * (o - t);
                same += usize::from((o > 0.5) == (t > 0.5));
                n += 1;
            }
        }
        assert!(n > 50_000, "{n} pixels to compare");
        (sum / n as f64, same as f64 / n as f64)
    };
    let mut costs = std::collections::HashMap::new();
    for dy in DY {
        for dx in DX {
            costs.insert((dx, dy), cost(dx, dy));
        }
    }
    let (&(bx, by), &(best, agreement)) = costs.iter().min_by(|a, b| a.1.0.total_cmp(&b.1.0)).expect("offsets");
    assert!(DX.contains(&(bx - 1)) && DX.contains(&(bx + 1)) && DY.contains(&(by - 1)) && DY.contains(&(by + 1)), "best at the edge of the search: {bx}, {by}");
    let parabola = |minus: f64, plus: f64| {
        let curve = minus - 2.0 * best + plus;
        if curve > 0.0 { 0.5 * (minus - plus) / curve } else { 0.0 }
    };
    let sx = bx as f64 + parabola(costs[&(bx - 1, by)].0, costs[&(bx + 1, by)].0);
    let sy = by as f64 + parabola(costs[&(bx, by - 1)].0, costs[&(bx, by + 1)].0);
    Fit { whole: [bx, by], sub: [sx, sy], agreement }
}

fn reference(name: &str) -> Image {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference").join(name);
    common::read_png(&path).unwrap_or_else(|| panic!("{} is missing", path.display()))
}

/// The dark map (earth, land and vector map) at V3 on a `size` canvas from the fixtures, saved as `name`.
fn dark_v3(size: [u32; 2], name: &str) -> Option<Image> {
    let gpu = gpu()?;
    let fixtures = Arc::new(FixtureTiles::new());
    let mut scene = SceneInput::new(V3.view(), [f64::from(size[0]), f64::from(size[1])], 1.0, Basemap::Dark);
    scene.earth = earth::Input::SHOWN;
    scene.vector_map = vector_map::Input::SHOWN;
    let (image, _) = shots::map_shot_with_tiles(&gpu, &scene, 1.0, fixtures.clone(), |_| Vec::new());
    shots::save(&image, name);
    assert!(fixtures.misses().is_empty(), "the fixtures lack {:?}", fixtures.misses());
    assert_eq!([image.width, image.height], size);
    Some(image)
}

/// V3 against today's picture at one size: `parts` are rectangles of the map clear of today's UI.
fn v3_against_today(size: [u32; 2], parts: &[(&str, [usize; 4])]) {
    let label = format!("{}x{}", size[0], size[1]);
    let Some(ours) = dark_v3(size, &format!("f4-dark-v3-{label}.png")) else { return };
    let theirs = Water::of(&reference(&format!("dark-v3-{label}.png")));
    let shift = Shift::recorded(&format!("v3-{label}"));
    let ours = Water::of(&ours);
    let moved = ours.moved(&shift);
    for &(part, rect) in parts {
        let plain = fit(&ours, &theirs, rect);
        let fixed = fit(&moved, &theirs, rect);
        // drawn minus deck's project, so our picture is that much lower and to the left of today's
        let [px, py] = shift.mean(rect).map(|d| -d);
        eprintln!(
            "V3 {label} {part}: ours over today's best at {:?} ({:+.2}, {:+.2} px; {:.4} agree), SwiftShader's error predicts \
             ({px:+.2}, {py:+.2}); moved by it: {:?} ({:+.2}, {:+.2} px; {:.4} agree)",
            plain.whole, plain.sub[0], plain.sub[1], plain.agreement, fixed.whole, fixed.sub[0], fixed.sub[1], fixed.agreement
        );
        assert!((plain.sub[0] - px).abs() < 0.35 && (plain.sub[1] - py).abs() < 0.35, "{label} {part}: {plain:?}, predicted ({px}, {py})");
        assert!(plain.whole[1] >= 4, "{label} {part}: {plain:?}");
        assert_eq!(fixed.whole, [0, 0], "{label} {part}: {fixed:?}");
        assert!(fixed.sub[0].abs() < 0.2 && fixed.sub[1].abs() < 0.2, "{label} {part}: {fixed:?}");
        assert!(fixed.agreement > plain.agreement && fixed.agreement > 0.98, "{label} {part}: {fixed:?} against {plain:?}");
    }
}

/// Desktop: the map between the sidebar and the map controls, under the mode bar and the dev readout, whole and
/// in thirds from the top (SwiftShader's error grows from 4.2 px at the top to 5.1 px at the bottom).
#[test]
fn v3_desktop_is_today_s_picture_but_for_swiftshader_s_sin_and_cos() {
    v3_against_today(
        [1440, 900],
        &[("whole", [360, 80, 1370, 880]), ("top", [360, 80, 1370, 340]), ("middle", [360, 340, 1370, 610]), ("bottom", [360, 610, 1370, 880])],
    );
}

/// Phone: the map between the top bar's alert and the bottom sheet, left of the map controls.
#[test]
fn v3_phone_is_today_s_picture_but_for_swiftshader_s_sin_and_cos() {
    v3_against_today([390, 844], &[("whole", [8, 200, 340, 740]), ("top", [8, 200, 340, 470]), ("bottom", [8, 470, 340, 740])]);
}
