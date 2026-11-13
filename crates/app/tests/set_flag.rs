//! Item F3's shots: `--set KEY=VALUE` through the binary's own path, from the committed fixtures
//! (`--fixture crates/map/tests/data`, `--now` the reference pictures' summer morning), each
//! beside the same command line without it or with another value.
//!
//! - `--set terrain=true` at Zermatt (`TERRAIN_VIEWS`: 46.0207 N, 7.7491 E, zoom 16, pitched 60°,
//!   800 × 500, realistic): with the terrain off the village stands at sea level and fills the
//!   picture with its roofs; with it on the camera stands over the mountainside and most of the
//!   picture is the bare slope, the village further up the valley (Z3's `standing.rs`, gap 4 of
//!   the stages 8-10 status).
//! - `--set sky=rain` at V4 (realistic): the rain preset's streaks fall where the map's own drops
//!   are (R11's check in `weather_effects.rs`); `--set sky=sunny` has none along the same lines.
//! - `--set weather=true --set daylight=false` at V2 (dark), run as the binary itself with every
//!   request sent to a proxy that is not there: RainViewer's bands from the fixture at 55 % over
//!   the map, against the same run without the radar (W5's check in `radar.rs`).
//! - A wrong `--set` makes the binary exit with an error that lists the keys.
//!
//! The shots are drawn by `Options::parse` and `headless::render_on` as `main` draws them
//! (`headless::save` is `render` and a PNG), or by the binary. No reference picture shows the
//! terrain, the rain or the radar. Needs a wgpu adapter (SwiftShader or lavapipe through
//! `VK_ICD_FILENAMES`); without one the GPU tests print `skipped: …` and pass. Shots go to
//! `target/shots/f3-*.png`.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::shots::{V2, V4, gpu, save, shots_dir};
use common::{NOW_MS, read_png};
use rustradar_app::{Options, headless};
use rustradar_core::geo::Basemap;
use rustradar_core::weather::PRESETS;
use rustradar_map::Gpu;
use rustradar_map::headless::Image;
use rustradar_map::io;
use rustradar_map::layers::sky;
use rustradar_map::post::{DEPTHS, Rain, rain_shown};
use rustradar_scene::camera::ViewState;
use rustradar_scene::radar::{BAND_COUNT, band_color};

/// The committed fixtures' folder, as `--fixture` takes it.
fn fixtures() -> PathBuf {
    io::fixture_tiles_dir().parent().expect("the data folder").to_owned()
}

/// `--screenshot … --now NOW_MS --fixture <the committed fixtures>` and `args`, as the binary
/// reads them.
fn command_line(args: &[&str]) -> Options {
    let fixtures = fixtures();
    let now = NOW_MS.to_string();
    let mut all = vec!["--screenshot", "f3.png", "--now", &now, "--fixture", fixtures.to_str().expect("a UTF-8 path")];
    all.extend_from_slice(args);
    Options::parse(all.iter().map(|a| a.to_string())).unwrap_or_else(|e| panic!("{all:?}: {e}"))
}

/// The shot of `args` (with [`command_line`]'s), drawn on `gpu` as `main` draws it.
fn shot(gpu: &Gpu, args: &[&str]) -> Image {
    let options = command_line(args);
    let shot = headless::render_on(gpu, &options).unwrap_or_else(|e| panic!("the shot of {args:?}: {e}"));
    let (size, scale) = headless::canvas(&options);
    assert_eq!([shot.image.width, shot.image.height], headless::surface_size(size, scale));
    shot.image
}

/// `--view` for `view`.
fn view_arg(view: &ViewState) -> String {
    format!("{},{},{},{},{}", view.latitude, view.longitude, view.zoom, view.pitch, view.bearing)
}

/// The village's roofs and other reddish pixels (`standing.rs`'s count): `[whole picture, its
/// lower half]`.
fn roofs(image: &Image) -> [usize; 2] {
    let mut n = [0, 0];
    for y in 0..image.height {
        for x in 0..image.width {
            let [r, g, ..] = image.pixel(x, y);
            if u16::from(r) > u16::from(g) + 20 {
                n[0] += 1;
                if y >= image.height / 2 {
                    n[1] += 1;
                }
            }
        }
    }
    n
}

/// `--set terrain=true` changes the Zermatt picture: from the village at sea level to the
/// mountainside the camera stands over.
#[test]
fn set_terrain_true_raises_zermatt_s_mountainside() {
    let Some(gpu) = gpu() else { return };
    let zermatt = ViewState { latitude: 46.0207, longitude: 7.7491, zoom: 16.0, pitch: 60.0, bearing: 0.0, position: None };
    let view = view_arg(&zermatt);
    let base = ["--view", &view, "--basemap", "realistic", "--size", "800x500"];
    let flat = shot(&gpu, &base);
    let raised = shot(&gpu, &[&base[..], &["--set", "terrain=true"]].concat());
    save(&flat, "f3-zermatt-terrain-off.png");
    save(&raised, "f3-zermatt-terrain-on.png");
    let changed = (0..flat.height)
        .flat_map(|y| (0..flat.width).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let (a, b) = (flat.pixel(x, y), raised.pixel(x, y));
            (0..3).any(|i| a[i].abs_diff(b[i]) > 24)
        })
        .count();
    let share = changed as f64 / f64::from(flat.width * flat.height);
    let ([flat_roofs, flat_low], [raised_roofs, raised_low]) = (roofs(&flat), roofs(&raised));
    eprintln!("Zermatt: {:.1} % of the picture changed; roof pixels {flat_roofs} ({flat_low} in the lower half) off, {raised_roofs} ({raised_low}) on", share * 100.0);
    assert!(share > 0.5, "most of the picture changes: {:.1} %", share * 100.0);
    // off: the village fills the picture; on: a bare slope at the centre and below, the village
    // up the valley at the top (Z3 counted 517 roof pixels there)
    assert!(flat_low > 10_000, "the village at sea level fills the lower half: {flat_low} px");
    assert!(raised_low < 50, "the slope in front of the raised camera is bare: {raised_low} px");
    assert!(raised_roofs > 300 && raised_roofs < flat_roofs / 10, "the village up the valley: {raised_roofs} px");
}

/// The luma of a pixel.
fn luma(p: [u8; 4]) -> f64 {
    0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2])
}

/// The mean luma along the middle of the nearest depth's streaks that the map draws for `input`
/// over `view` on a `size` canvas, and five pixels to either side of them (R11's measure).
fn streak_contrast(image: &Image, view: &ViewState, size: [f64; 2], input: &sky::Input) -> (f64, f64) {
    let shown = rain_shown(Basemap::Realistic, input, view, size, NOW_MS);
    let mut rain = Rain::new();
    rain.show(shown, size, NOW_MS);
    rain.step(NOW_MS);
    let streaks = rain.streaks();
    let near: Vec<_> = streaks.iter().filter(|s| s.width == DEPTHS[2].line).collect();
    assert!(near.len() > 50, "{} near streaks", near.len());
    let inside = |x: f64, y: f64| x >= 0.0 && y >= 0.0 && x < size[0] && y < size[1];
    let at = |x: f64, y: f64| luma(image.pixel(x as u32, y as u32));
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
            on += at(x, y);
            beside += (at(lx, ly) + at(rx, ry)) / 2.0;
            n += 1.0;
        }
    }
    (on / n, beside / n)
}

/// `--set sky=rain`: the rain preset's streaks over V4, where the map's drops for it are; under
/// `--set sky=sunny` the same lines are the picture's own.
#[test]
fn set_sky_rain_shows_the_rain_preset_s_streaks() {
    let Some(gpu) = gpu() else { return };
    let v4 = V4.view();
    let view = view_arg(&v4);
    let base = ["--view", &view, "--basemap", "realistic", "--size", "1100x720"];
    let rain = shot(&gpu, &[&base[..], &["--set", "sky=rain"]].concat());
    let sunny = shot(&gpu, &[&base[..], &["--set", "sky=sunny"]].concat());
    save(&rain, "f3-v4-sky-rain.png");
    save(&sunny, "f3-v4-sky-sunny.png");
    let size = [1100.0, 720.0];
    let input = sky::Input { shown: true, weather: Some(PRESETS.rain), daylight: true };
    let shown = rain_shown(Basemap::Realistic, &input, &v4, size, NOW_MS);
    assert!(shown.falls() && shown.amount == 1.0, "{shown:?}");
    let (on, beside) = streak_contrast(&rain, &v4, size, &input);
    let (dry_on, dry_beside) = streak_contrast(&sunny, &v4, size, &input);
    eprintln!("V4: along the near streaks {on:.1}, beside them {beside:.1} in the rain; {dry_on:.1} and {dry_beside:.1} in the sun");
    // a near streak is (215, 223, 235) at 0.4 over the wet ground: some 40 levels at its line
    assert!(on - beside > 15.0, "rain: on {on:.1}, beside {beside:.1}");
    assert!((dry_on - dry_beside).abs() < 4.0, "sun: on {dry_on:.1}, beside {dry_beside:.1}");
}

/// `[r, g, b, a]` drawn at `opacity` over `under`.
fn over(under: [u8; 4], [r, g, b, a]: [u8; 4], opacity: f64) -> [f64; 3] {
    let alpha = f64::from(a) / 255.0 * opacity;
    [0, 1, 2].map(|i| f64::from(under[i]) + (f64::from([r, g, b][i]) - f64::from(under[i])) * alpha)
}

/// Whether the 3 × 3 pixels round `(x, y)` are one colour.
fn uniform(image: &Image, x: u32, y: u32) -> bool {
    let c = image.pixel(x, y);
    (x - 1..=x + 1).all(|i| (y - 1..=y + 1).all(|j| image.pixel(i, j) == c))
}

/// Whether `got` is a stack of one to six bands, weakest first, at `opacity` over `under`.
fn stacked(under: [u8; 4], got: [u8; 4], opacity: f64, from: usize, left: usize) -> bool {
    (from..BAND_COUNT).any(|band| {
        let c = over(under, band_color(band), opacity);
        let next = [c[0].round() as u8, c[1].round() as u8, c[2].round() as u8, 255];
        let off = (0..3).map(|i| next[i].abs_diff(got[i])).max().unwrap();
        off <= 2 || (left > 1 && stacked(next, got, opacity, band + 1, left - 1))
    })
}

/// What the radar changed between `base` and `image` (W5's measure): pixels changed, those inside
/// a band, those of one band at `opacity` over the map, those of a stack of bands, and the bands.
fn radar_changes(base: &Image, image: &Image, opacity: f64) -> (usize, usize, usize, usize, [usize; BAND_COUNT]) {
    let (mut changed, mut inside, mut one_band, mut stacks, mut bands) = (0, 0, 0, 0, [0; BAND_COUNT]);
    for y in 1..image.height - 1 {
        for x in 1..image.width - 1 {
            let (before, after) = (base.pixel(x, y), image.pixel(x, y));
            if before == after {
                continue;
            }
            changed += 1;
            if !uniform(image, x, y) || !uniform(base, x, y) {
                continue;
            }
            inside += 1;
            let off = |want: [f64; 3]| (0..3).map(|i| (want[i] - f64::from(after[i])).abs()).fold(0.0, f64::max);
            let (miss, band) = (0..BAND_COUNT).map(|band| (off(over(before, band_color(band), opacity)), band)).min_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
            if miss <= 1.5 {
                one_band += 1;
                bands[band] += 1;
            } else if stacked(before, after, opacity, 0, 6) {
                stacks += 1;
            }
        }
    }
    (changed, inside, one_band, stacks, bands)
}

/// Runs the binary with `args` and every request sent to a proxy that is not there; returns
/// its standard output and error.
fn run_binary(args: &[&str]) -> (bool, String, String) {
    let dead = "http://127.0.0.1:9";
    let run = Command::new(env!("CARGO_BIN_EXE_rustradar-app"))
        .args(args)
        .env("HTTPS_PROXY", dead)
        .env("HTTP_PROXY", dead)
        .env("ALL_PROXY", dead)
        .env("https_proxy", dead)
        .env("http_proxy", dead)
        .env("all_proxy", dead)
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .output()
        .expect("the binary runs");
    (run.status.success(), String::from_utf8_lossy(&run.stdout).into_owned(), String::from_utf8_lossy(&run.stderr).into_owned())
}

/// The binary's shot of `args` into `target/shots/<name>`, read back.
fn binary_shot(name: &str, args: &[&str]) -> Image {
    let out = shots_dir().join(name);
    let _ = std::fs::remove_file(&out);
    let fixtures = fixtures();
    let now = NOW_MS.to_string();
    let mut all = vec!["--screenshot", out.to_str().expect("a UTF-8 path"), "--now", &now, "--fixture", fixtures.to_str().expect("a UTF-8 path")];
    all.extend_from_slice(args);
    let (ok, stdout, stderr) = run_binary(&all);
    assert!(!stderr.contains("panicked"), "a panic:\n{stderr}");
    assert!(ok, "{all:?}\nstdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(stdout.contains("wrote"), "{stdout}");
    read_png(Path::new(&out)).unwrap_or_else(|| panic!("{} is an RGBA PNG", out.display()))
}

/// The binary with `--set weather=true --set daylight=false` at V2: RainViewer's bands from the
/// fixture at 55 % over the dark map, from light rain to heavy; with only `daylight=false`, not a
/// pixel of radar (the same counts as W5's `radar.rs`).
#[test]
fn the_binary_with_set_weather_true_draws_the_fixture_s_radar_at_v2() {
    // the device is not used here, but the binary's own must not run beside another test's
    let Some(_gpu) = gpu() else { return };
    let view = V2.arg();
    let base = ["--view", &view, "--basemap", "dark", "--size", "1100x720", "--set", "daylight=false"];
    let off = binary_shot("f3-v2-weather-off.png", &base);
    let on = binary_shot("f3-v2-weather-on.png", &[&base[..], &["--set", "weather=true"]].concat());
    assert_eq!([on.width, on.height], [1100, 720]);
    let (changed, inside, one_band, stacks, bands) = radar_changes(&off, &on, 0.55);
    eprintln!("V2 through the binary: {changed} px changed, {inside} inside a band, {one_band} one band, {stacks} stacked; by band {bands:?}");
    assert!(changed > 50_000, "{changed} px of radar");
    assert!((one_band + stacks) * 100 >= inside * 97, "{one_band} + {stacks} of {inside} inside are bands at 55 %");
    assert!(bands[..9].iter().all(|&n| n > 0), "cyan to orange: {bands:?}");
}

/// A wrong `--set` stops the binary before anything is drawn, with an error that says what was
/// wrong and lists what is right.
#[test]
fn the_binary_refuses_a_wrong_set() {
    let (ok, _, stderr) = run_binary(&["--set", "tilt=true"]);
    assert!(!ok);
    assert!(stderr.contains("--set: there is no setting \"tilt\"; the settings: mode, alsoShow, basemap, labels, labelSize"), "{stderr}");
    assert!(stderr.contains("usage: rustradar-app"), "{stderr}");
    let (ok, _, stderr) = run_binary(&["--set", "sky=storm"]);
    assert!(!ok);
    assert!(stderr.contains("--set sky wants live, sunny, cloudy, rain, fog or snow, not \"storm\""), "{stderr}");
}
