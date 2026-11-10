//! The shot harness (plan section 8, item E18): the named views V1–V8, a shared device that skips
//! without an adapter, and the measures shot tests assert with. Assertions are properties, not
//! golden images: colours at chosen pixels ([`pixel`], [`assert_color`]), how much of a rectangle
//! is a colour ([`coverage`], [`area`]), sizes in pixels ([`disc_radius`], [`disc_radii`],
//! [`bounds`], [`coverage_box`]), connected components ([`components`]) and "no pixel darker
//! than" ([`darker_than`]). They hold across rasterisers (SwiftShader, lavapipe, real GPUs).
//!
//! [`map_shot`] draws a scene with test layers through the map's headless path; the app's
//! `tests/common` adds shots through `--screenshot`'s path.
//!
//! Shots are written to `target/shots/` for review ([`save`]). With `SHOT_GOLDEN=1` each one is
//! also compared with `target/shots/golden/<name>`, which the first such run records (and
//! `SHOT_GOLDEN=record` records again): a before/after check for a change that should not move a
//! pixel. Nothing golden is committed.
//!
//! This file is the map's `tests/common/shots.rs`; the app's tests take the same file in with
//! `#[path]` (`crates/app/tests/common/mod.rs`), so it names only crates both packages depend on
//! (`rustradar-map`, `rustradar-scene`, `wgpu`), and the PNG reader comes from the including
//! module (`super::read_png`), as the two packages decode PNG with different crates.
//!
//! Coordinates are device pixels of the image, `x` right and `y` down from the top left; a pixel
//! `(x, y)` covers `[x, x + 1) × [y, y + 1)` and its centre is `(x + 0.5, y + 0.5)`.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use rustradar_map::frame::ViewFrame;
use rustradar_map::headless::{FORMAT, Image, render_with};
use rustradar_map::layers::{Init, Layer};
use rustradar_map::{Gpu, GpuError, MapRenderer, SceneInput};
use rustradar_scene::camera::ViewState;

/// An RGBA pixel, gamma-space bytes as read back.
pub type Rgba = [u8; 4];

/// The map's clear colour, `--ink`: the space round the globe (plan 1.4.1).
pub const INK: Rgba = [30, 30, 30, 255];
/// The dark map's land and the earth sphere on it (E5).
pub const LAND: Rgba = [50, 50, 50, 255];
/// The earth sphere on satellite before imagery (E5).
pub const SATELLITE_EARTH: Rgba = [6, 16, 34, 255];
/// The shots' canvas in CSS pixels (plan section 8, the app's `SHOT_SIZE`).
pub const SHOT_SIZE: [u32; 2] = [1100, 720];

/// One of the named views of plan section 8.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NamedView {
    pub name: &'static str,
    pub latitude: f64,
    pub longitude: f64,
    pub zoom: f64,
    pub pitch: f64,
    pub bearing: f64,
    /// What the view is for (the table's last column).
    pub used_for: &'static str,
}

impl NamedView {
    /// The camera.
    pub const fn view(&self) -> ViewState {
        ViewState { longitude: self.longitude, latitude: self.latitude, zoom: self.zoom, bearing: self.bearing, pitch: self.pitch, position: None }
    }

    /// The view as `--view` takes it: `lat,lon,zoom,pitch,bearing`.
    pub fn arg(&self) -> String {
        format!("{},{},{},{},{}", self.latitude, self.longitude, self.zoom, self.pitch, self.bearing)
    }

    /// Whether the view draws the flat map (zoom above 12) rather than the globe.
    pub fn is_flat(&self) -> bool {
        self.view().is_flat()
    }
}

const fn named(name: &'static str, [latitude, longitude, zoom, pitch, bearing]: [f64; 5], used_for: &'static str) -> NamedView {
    NamedView { name, latitude, longitude, zoom, pitch, bearing, used_for }
}

/// The initial view; the whole globe.
pub const V1: NamedView = named("V1", [35.0, 10.0, 1.9, 0.0, 0.0], "the initial view; whole globe");
/// Europe; labels, radar, aircraft icons.
pub const V2: NamedView = named("V2", [52.0, 10.0, 4.5, 0.0, 0.0], "Europe; labels, radar, aircraft icons");
/// Stockholm region; vector map, trains, rail map.
pub const V3: NamedView = named("V3", [59.33, 18.07, 9.0, 0.0, 0.0], "Stockholm region; vector map, trains, rail map");
/// Arlanda, flat map, pitched; runways, aircraft models.
pub const V4: NamedView = named("V4", [59.6519, 17.9186, 13.5, 45.0, 30.0], "Arlanda, flat map, pitched; runways, aircraft models");
/// Stockholm city; buildings, realistic ground, trees.
pub const V5: NamedView = named("V5", [59.3293, 18.0686, 15.5, 60.0, 20.0], "Stockholm city; buildings, realistic ground, trees");
/// Gothenburg harbour; ships.
pub const V6: NamedView = named("V6", [57.70, 11.93, 13.0, 0.0, 0.0], "Gothenburg harbour; ships");
/// Svalbard; latitude coupling, far-side filters.
pub const V7: NamedView = named("V7", [78.2, 15.6, 6.0, 0.0, 0.0], "Svalbard; latitude coupling, far-side filters");
/// The antimeridian wrap.
pub const V8: NamedView = named("V8", [0.0, 179.5, 3.0, 0.0, 0.0], "antimeridian wrap");

/// V1 to V8 in order.
pub const VIEWS: [NamedView; 8] = [V1, V2, V3, V4, V5, V6, V7, V8];

/// The named view called `name` (`"V1"` … `"V8"`).
pub fn named_view(name: &str) -> Option<NamedView> {
    VIEWS.into_iter().find(|v| v.name == name)
}

/// One device for the whole test binary, used one test at a time: SwiftShader crashes when several
/// tests open their own at once. `None`, after printing `skipped: …`, when there is no adapter, so
/// a GPU test starts with `let Some(gpu) = gpu() else { return };` and passes without one.
pub fn gpu() -> Option<MutexGuard<'static, Gpu>> {
    static GPU: OnceLock<Option<Mutex<Gpu>>> = OnceLock::new();
    let gpu = GPU.get_or_init(|| match Gpu::headless() {
        Ok(gpu) => {
            eprintln!("shots drawn with {}", gpu.describe());
            Some(Mutex::new(gpu))
        }
        Err(GpuError::NoAdapter(e)) => {
            eprintln!("skipped: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    });
    match gpu {
        Some(gpu) => Some(gpu.lock().unwrap_or_else(|poisoned| poisoned.into_inner())),
        None => {
            eprintln!("skipped: no graphics adapter (set VK_ICD_FILENAMES to a software Vulkan such as SwiftShader or lavapipe)");
            None
        }
    }
}

/// Draws `scene` as a window of `scale_factor` shows it (without the UI): the map's own layers,
/// then the test layers `build` makes (drawn last, in order), through `map::headless`. The image
/// and the view of the frame it was drawn with.
pub fn map_shot(gpu: &Gpu, scene: &SceneInput, scale_factor: f64, build: impl FnOnce(&Init<'_>) -> Vec<Box<dyn Layer>>) -> (Image, ViewFrame) {
    let mut renderer = MapRenderer::new(gpu, FORMAT);
    let layers = build(&renderer.init(gpu));
    for layer in layers {
        renderer.add_layer(layer);
    }
    let image = render_with(gpu, &mut renderer, scene, scale_factor).expect("the map rendered");
    (image, renderer.view().clone())
}

/// A rectangle of pixels, `x0..x1` × `y0..y1` (ends excluded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Rect {
    /// `width × height` pixels from `(x, y)`.
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Rect {
        Rect { x0: x, y0: y, x1: x + width, y1: y + height }
    }

    /// The whole image.
    pub fn whole(image: &Image) -> Rect {
        Rect::new(0, 0, image.width, image.height)
    }

    /// The pixels whose centres lie within `r` (each way, a square) of the point `centre`, clipped
    /// to `image`.
    pub fn around(image: &Image, [cx, cy]: [f64; 2], r: f64) -> Rect {
        let lo = |c: f64| (c - r - 0.5).ceil().max(0.0) as u32;
        let hi = |c: f64, n: u32| ((c + r - 0.5).floor() + 1.0).clamp(0.0, f64::from(n)) as u32;
        Rect { x0: lo(cx), y0: lo(cy), x1: hi(cx, image.width), y1: hi(cy, image.height) }.clip(image)
    }

    /// This rectangle within `image`.
    pub fn clip(self, image: &Image) -> Rect {
        let x1 = self.x1.min(image.width);
        let y1 = self.y1.min(image.height);
        Rect { x0: self.x0.min(x1), y0: self.y0.min(y1), x1, y1 }
    }

    pub fn width(&self) -> u32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> u32 {
        self.y1 - self.y0
    }

    /// How many pixels it holds.
    pub fn len(&self) -> usize {
        self.width() as usize * self.height() as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, x: u32, y: u32) -> bool {
        (self.x0..self.x1).contains(&x) && (self.y0..self.y1).contains(&y)
    }

    /// Its pixels, row by row.
    pub fn pixels(self) -> impl Iterator<Item = (u32, u32)> {
        (self.y0..self.y1).flat_map(move |y| (self.x0..self.x1).map(move |x| (x, y)))
    }

    /// Its centre, as a point.
    pub fn centre(&self) -> [f64; 2] {
        [f64::from(self.x0 + self.x1) / 2.0, f64::from(self.y0 + self.y1) / 2.0]
    }
}

/// The pixel at `(x, y)` (panics, naming the image's size, outside it).
#[track_caller]
pub fn pixel(image: &Image, x: u32, y: u32) -> Rgba {
    assert!(x < image.width && y < image.height, "pixel ({x}, {y}) is outside the {} × {} image", image.width, image.height);
    image.pixel(x, y)
}

/// The pixel that holds the point `[x, y]`, or `None` off the image.
pub fn pixel_at(image: &Image, [x, y]: [f64; 2]) -> Option<Rgba> {
    (x >= 0.0 && y >= 0.0 && x < f64::from(image.width) && y < f64::from(image.height)).then(|| image.pixel(x as u32, y as u32))
}

/// The largest difference between two pixels over the four channels.
pub fn difference(a: Rgba, b: Rgba) -> u8 {
    (0..4).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0)
}

/// Whether `a` is `b` within `tolerance` levels in each channel.
pub fn close(a: Rgba, b: Rgba, tolerance: u8) -> bool {
    difference(a, b) <= tolerance
}

/// Asserts the pixel at `(x, y)` is `expected` within `tolerance` levels in each channel.
#[track_caller]
pub fn assert_color(image: &Image, x: u32, y: u32, expected: Rgba, tolerance: u8) {
    let got = pixel(image, x, y);
    assert!(close(got, expected, tolerance), "pixel ({x}, {y}) is {got:?}, not {expected:?} ± {tolerance}");
}

/// The share (0–1) of the pixels of `rect` that are `colour` within `tolerance` levels in each
/// channel; 0 for an empty rectangle.
pub fn coverage(image: &Image, rect: Rect, colour: Rgba, tolerance: u8) -> f64 {
    let rect = rect.clip(image);
    if rect.is_empty() {
        return 0.0;
    }
    let hits = rect.pixels().filter(|&(x, y)| close(image.pixel(x, y), colour, tolerance)).count();
    hits as f64 / rect.len() as f64
}

/// How many pixels of `rect` `pick` keeps.
pub fn count(image: &Image, rect: Rect, pick: impl Fn(Rgba) -> bool) -> usize {
    rect.clip(image).pixels().filter(|&(x, y)| pick(image.pixel(x, y))).count()
}

/// The channel (0–2) on which `a` and `b` differ most.
pub fn telling_channel(a: Rgba, b: Rgba) -> usize {
    (0..3).max_by_key(|&c| a[c].abs_diff(b[c])).unwrap_or(0)
}

/// How far `p` is from `from` toward `to`, 0–1, read on the channel on which they differ most:
/// the coverage of an antialiased shape of `to` drawn opaque over `from`.
pub fn mix(p: Rgba, from: Rgba, to: Rgba) -> f64 {
    let c = telling_channel(from, to);
    let (lo, hi) = (f64::from(from[c]), f64::from(to[c]));
    if hi == lo {
        return if p[c] == to[c] { 1.0 } else { 0.0 };
    }
    ((f64::from(p[c]) - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// The area, in pixels², that an antialiased shape of `colour` covers in `rect` over
/// `background`: the sum of each pixel's [`mix`]. A line's width is its area over its length.
pub fn area(image: &Image, rect: Rect, background: Rgba, colour: Rgba) -> f64 {
    rect.clip(image).pixels().map(|(x, y)| mix(image.pixel(x, y), background, colour)).sum()
}

/// A weight per pixel of the whole image, rows top first (for [`coverage_box`]).
pub fn weights(image: &Image, weight: impl Fn(Rgba) -> f64) -> Vec<f64> {
    Rect::whole(image).pixels().map(|(x, y)| weight(image.pixel(x, y)).clamp(0.0, 1.0)).collect()
}

/// The ink box of a coverage picture (`size` pixels, rows top first, values 0–1): where the
/// largest coverage of the columns, and of the rows, crosses one half, interpolated between pixel
/// centres. Left, top, right, bottom in pixels (pixel `x` spans `x` to `x + 1`); `None` if nothing
/// reaches one half. This is how an
/// edge is placed to a fraction of a pixel on any rasteriser (E10's check against epaint).
pub fn coverage_box(coverage: &[f64], [width, height]: [usize; 2]) -> Option<[f64; 4]> {
    assert_eq!(coverage.len(), width * height, "a {width} × {height} picture");
    let columns: Vec<f64> = (0..width).map(|x| (0..height).map(|y| coverage[y * width + x]).fold(0.0, f64::max)).collect();
    let rows: Vec<f64> = (0..height).map(|y| coverage[y * width..(y + 1) * width].iter().copied().fold(0.0, f64::max)).collect();
    let rising = |p: &[f64]| {
        let i = p.iter().position(|&c| c >= 0.5)?;
        let before = if i == 0 { 0.0 } else { p[i - 1] };
        Some((i as f64 - 0.5) + (0.5 - before) / (p[i] - before))
    };
    let falling = |p: &[f64]| {
        let j = p.iter().rposition(|&c| c >= 0.5)?;
        let after = p.get(j + 1).copied().unwrap_or(0.0);
        Some((j as f64 + 0.5) + (p[j] - 0.5) / (p[j] - after))
    };
    Some([rising(&columns)?, rising(&rows)?, falling(&columns)?, falling(&rows)?])
}

/// The smallest rectangle holding every pixel of `rect` that `pick` keeps.
pub fn bounds(image: &Image, rect: Rect, pick: impl Fn(Rgba) -> bool) -> Option<Rect> {
    let mut found: Option<Rect> = None;
    for (x, y) in rect.clip(image).pixels() {
        if pick(image.pixel(x, y)) {
            found = Some(match found {
                None => Rect::new(x, y, 1, 1),
                Some(b) => Rect { x0: b.x0.min(x), y0: b.y0.min(y), x1: b.x1.max(x + 1), y1: b.y1.max(y + 1) },
            });
        }
    }
    found
}

/// A connected set of pixels ([`components`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    /// How many pixels.
    pub pixels: usize,
    pub bounds: Rect,
    /// The mean of its pixels' centres.
    pub centroid: [f64; 2],
}

/// The 4-connected components of the pixels of `rect` that `pick` keeps, in the order their first
/// pixels come row by row.
pub fn components(image: &Image, rect: Rect, pick: impl Fn(Rgba) -> bool) -> Vec<Component> {
    let rect = rect.clip(image);
    let (w, h) = (rect.width() as usize, rect.height() as usize);
    let kept: Vec<bool> = rect.pixels().map(|(x, y)| pick(image.pixel(x, y))).collect();
    let mut seen = vec![false; w * h];
    let mut found = Vec::new();
    for start in 0..w * h {
        if !kept[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        let mut queue = VecDeque::from([start]);
        let (mut pixels, mut sum) = (0usize, [0.0f64; 2]);
        let (mut lo, mut hi) = ([usize::MAX; 2], [0usize; 2]);
        while let Some(i) = queue.pop_front() {
            let (x, y) = (i % w, i / w);
            pixels += 1;
            sum = [sum[0] + x as f64 + 0.5, sum[1] + y as f64 + 0.5];
            lo = [lo[0].min(x), lo[1].min(y)];
            hi = [hi[0].max(x + 1), hi[1].max(y + 1)];
            let mut visit = |j: usize| {
                if kept[j] && !seen[j] {
                    seen[j] = true;
                    queue.push_back(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        let (x0, y0) = (rect.x0 as usize, rect.y0 as usize);
        found.push(Component {
            pixels,
            bounds: Rect { x0: (x0 + lo[0]) as u32, y0: (y0 + lo[1]) as u32, x1: (x0 + hi[0]) as u32, y1: (y0 + hi[1]) as u32 },
            centroid: [x0 as f64 + sum[0] / pixels as f64, y0 as f64 + sum[1] / pixels as f64],
        });
    }
    found
}

/// The first pixel of `rect`, row by row, with a colour channel darker than `rgb`'s, or `None`
/// when no pixel is darker than it ("no pixel darker than").
pub fn darker_than(image: &Image, rect: Rect, rgb: [u8; 3]) -> Option<(u32, u32, Rgba)> {
    rect.clip(image).pixels().map(|(x, y)| (x, y, image.pixel(x, y))).find(|(_, _, p)| (0..3).any(|c| p[c] < rgb[c]))
}

/// The darkest and the brightest pixel of `rect` by the sum of their colour channels.
pub fn darkest_and_brightest(image: &Image, rect: Rect) -> Option<(Rgba, Rgba)> {
    let sum = |p: &Rgba| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
    let pixels = || rect.clip(image).pixels().map(|(x, y)| image.pixel(x, y));
    Some((pixels().min_by_key(sum)?, pixels().max_by_key(sum)?))
}

/// The image's value on `channel` at the point `[x, y]`, interpolated bilinearly between pixel
/// centres (edges held), or `None` off the image.
pub fn sample(image: &Image, [x, y]: [f64; 2], channel: usize) -> Option<f64> {
    if x < 0.0 || y < 0.0 || x >= f64::from(image.width) || y >= f64::from(image.height) {
        return None;
    }
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let at = |xi: f64, yi: f64| {
        let xi = xi.clamp(0.0, f64::from(image.width - 1)) as u32;
        let yi = yi.clamp(0.0, f64::from(image.height - 1)) as u32;
        f64::from(image.pixel(xi, yi)[channel])
    };
    let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1.0, y0) * tx;
    let bottom = at(x0, y0 + 1.0) * (1.0 - tx) + at(x0 + 1.0, y0 + 1.0) * tx;
    Some(top * (1.0 - ty) + bottom * ty)
}

/// The radius of a disc of `inside` on `outside` round `centre` (a point, device pixels), along
/// `angle` (radians, x right, y down): walking out from the centre in steps of 1/16 px, where the
/// image (sampled bilinearly on the channel that tells the two apart) first crosses halfway from
/// `inside` to `outside`. `None` if the ray leaves the image first. Good to a few tenths of a
/// pixel on an antialiased edge; [`disc_radii`] is steadier.
pub fn disc_radius(image: &Image, centre: [f64; 2], angle: f64, inside: Rgba, outside: Rgba) -> Option<f64> {
    let c = telling_channel(inside, outside);
    let half = (f64::from(inside[c]) + f64::from(outside[c])) / 2.0;
    let rising = inside[c] < outside[c];
    let (dx, dy) = (angle.cos(), angle.sin());
    let step = 1.0 / 16.0;
    let mut previous: Option<(f64, f64)> = None;
    let mut r = 0.0;
    loop {
        let v = sample(image, [centre[0] + r * dx, centre[1] + r * dy], c)?;
        let crossed = if rising { v >= half } else { v <= half };
        if crossed {
            return Some(match previous {
                Some((r0, v0)) if v != v0 => r0 + (half - v0) / (v - v0) * (r - r0),
                _ => r,
            });
        }
        previous = Some((r, v));
        r += step;
    }
}

/// The edge of a disc of `inside` on `outside` round `centre` (a point, device pixels), whose
/// radius is about `near`, in each of `sectors` equal sectors: the radius at which an ideal edge
/// would give the pixels whose centres lie in the sector's part of the ring `near ± 6` px the same
/// total coverage ([`mix`]) as the image does, each pixel covered by `R − r + ½` (clamped to 0–1)
/// at a centre `r` from `centre`. Fitting over the same pixels the image has cancels how many
/// pixel centres happen to fall in a narrow sector. Sector `s` is centred on the angle
/// `(s + 0.5) · 2π / sectors` (x right, y down). `None` for a sector whose ring leaves the image.
/// (E5's measure.)
pub fn disc_radii(image: &Image, centre: [f64; 2], near: f64, sectors: usize, inside: Rgba, outside: Rgba) -> Vec<Option<f64>> {
    let (r0, r1) = ((near - 6.0).max(0.0), near + 6.0);
    let step = std::f64::consts::TAU / sectors as f64;
    // per sector: each pixel's distance and coverage
    let mut rings: Vec<Vec<(f64, f64)>> = vec![Vec::new(); sectors];
    let mut clipped = vec![false; sectors];
    let (x0, x1) = ((centre[0] - r1).floor() as i64, (centre[0] + r1).ceil() as i64);
    let (y0, y1) = ((centre[1] - r1).floor() as i64, (centre[1] + r1).ceil() as i64);
    for y in y0..y1 {
        for x in x0..x1 {
            let (dx, dy) = (x as f64 + 0.5 - centre[0], y as f64 + 0.5 - centre[1]);
            let r = dx.hypot(dy);
            if r < r0 || r >= r1 {
                continue;
            }
            let sector = ((dy.atan2(dx).rem_euclid(std::f64::consts::TAU) / step) as usize).min(sectors - 1);
            if x < 0 || y < 0 || x >= i64::from(image.width) || y >= i64::from(image.height) {
                clipped[sector] = true;
                continue;
            }
            rings[sector].push((r, mix(image.pixel(x as u32, y as u32), outside, inside)));
        }
    }
    rings
        .iter()
        .zip(clipped)
        .map(|(ring, clipped)| {
            if clipped || ring.is_empty() {
                return None;
            }
            let covered: f64 = ring.iter().map(|(_, c)| c).sum();
            let ideal = |radius: f64| ring.iter().map(|(r, _)| (radius - r + 0.5).clamp(0.0, 1.0)).sum::<f64>();
            let (mut lo, mut hi) = (r0 - 1.0, r1 + 1.0);
            for _ in 0..50 {
                let mid = (lo + hi) / 2.0;
                if ideal(mid) < covered {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            Some((lo + hi) / 2.0)
        })
        .collect()
}

/// `target/shots`, made if need be.
pub fn shots_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
    std::fs::create_dir_all(&dir).expect("the shot folder");
    dir.canonicalize().unwrap_or(dir)
}

/// Writes `image` to `target/shots/<name>` (a `.png` name) for review. With `SHOT_GOLDEN=1` it is
/// compared first with `target/shots/golden/<name>`, recorded there if that is missing;
/// `SHOT_GOLDEN=record` records it again. A difference fails the test ([`compare`]).
#[track_caller]
pub fn save(image: &Image, name: &str) {
    let dir = shots_dir();
    image.save_png(&dir.join(name)).unwrap_or_else(|e| panic!("writing {name}: {e}"));
    let mode = std::env::var("SHOT_GOLDEN").unwrap_or_default();
    if mode.is_empty() || mode == "0" {
        return;
    }
    let golden_dir = dir.join("golden");
    std::fs::create_dir_all(&golden_dir).expect("the golden folder");
    let golden = golden_dir.join(name);
    if mode != "record"
        && let Some(recorded) = super::read_png(&golden)
    {
        if let Err(why) = compare(image, &recorded, GOLDEN_TOLERANCE, GOLDEN_OUTLIERS) {
            panic!("{name} differs from {}: {why}", golden.display());
        }
        return;
    }
    image.save_png(&golden).unwrap_or_else(|e| panic!("recording {name}: {e}"));
    eprintln!("recorded {}", golden.display());
}

/// How many levels a golden comparison lets a channel move.
pub const GOLDEN_TOLERANCE: u8 = 2;
/// The share of pixels a golden comparison lets move further (rasteriser noise on edges).
pub const GOLDEN_OUTLIERS: f64 = 0.001;

/// `Ok` when `image` is `golden`'s size and at most `outliers` (a share) of its pixels differ by
/// more than `tolerance` levels in any channel; otherwise what differs.
pub fn compare(image: &Image, golden: &Image, tolerance: u8, outliers: f64) -> Result<(), String> {
    if [image.width, image.height] != [golden.width, golden.height] {
        return Err(format!("{} × {} against {} × {}", image.width, image.height, golden.width, golden.height));
    }
    let mut moved = 0usize;
    let mut worst: Option<(u32, u32, u8)> = None;
    for (x, y) in Rect::whole(image).pixels() {
        let d = difference(image.pixel(x, y), golden.pixel(x, y));
        if d > tolerance {
            moved += 1;
            if worst.is_none_or(|(_, _, w)| d > w) {
                worst = Some((x, y, d));
            }
        }
    }
    let allowed = (outliers * (image.width as f64) * (image.height as f64)).floor() as usize;
    match worst {
        Some((x, y, d)) if moved > allowed => {
            Err(format!("{moved} pixels moved by more than {tolerance} levels (at most {allowed} may), the most at ({x}, {y}) by {d}"))
        }
        _ => Ok(()),
    }
}
