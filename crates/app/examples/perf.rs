//! Performance measurements of item P1 (plan, "Performance (P1)"). Meant for release builds:
//!
//! ```text
//! cargo run -p rustradar-app --release --example perf -- decode [--repeat N] [--top N] [--step NAME] [--only Z/X/Y]
//! cargo run -p rustradar-app --release --example perf -- frame [--aircraft N] [--view LAT,LON,ZOOM[,PITCH[,BEARING]]]
//!                                                            [--size WxH] [--frames N] [--drawn N] [--warmup N] [--labels] [--pan] [--trace]
//!                                                            [--basemap B] [--no-map-labels] [--no-night] [--draw-settle]
//! cargo run -p rustradar-app --release --example perf -- fixture DIR [--aircraft N] [--view …] [--size WxH]
//! ```
//!
//! - `decode`: every committed fixture tile (`crates/map/tests/data/tiles`) through the decode step
//!   each tile layer runs on the worker pool: the vector map (`vector_map::decode`, MVT decode and
//!   tessellation), the place labels (`labels_map::decode`), the plain buildings (z13–14,
//!   `buildings::decode`) and the satellite imagery (`imagery::decode`, JPEG). Prints the mean,
//!   the median and the worst per zoom, and the slowest tiles.
//! - `frame`: `N` synthetic aircraft (default 2000) spread over the view (default V2, 1440 × 900)
//!   as a `--fixture` feed, the dark map from the committed tiles, run by the app's engine on
//!   SwiftShader until settled (undrawn, as a shot), `--warmup` undrawn frames moving as the measured
//!   ones (default 30: new tiles and glyphs come in once); then `--drawn` drawn frames with the scene
//!   refresh forced every frame, timing the whole frame's CPU (`Engine::frame`: poll, builders,
//!   egui, prepare, encoding, submit) and the wait for the GPU; then as many rounds of the scene
//!   builders alone (`SceneBuilder::build`) and of the map's CPU side alone (`MapRenderer::update`
//!   and `prepare`). `--pan` moves the camera every frame (nothing laid out for the last frame is
//!   kept), `--labels` turns the aircraft labels on.
//!
//! - `fixture`: a `--fixture` folder for the app's `--screenshot` with `N` synthetic aircraft over
//!   the view (as `frame` makes them) and the committed tiles linked in as `tiles`.
//!
//! Needs a Vulkan adapter for `frame` (here SwiftShader:
//! `VK_ICD_FILENAMES=/opt/pw-browsers/chromium-1243/chrome-linux64/vk_swiftshader_icd.json`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustradar_app::app::Engine;
use rustradar_app::headless::{FORMAT, Offscreen};
use rustradar_app::state::{AppState, Startup};
use rustradar_core::geo::Basemap;
use rustradar_core::model::{LiveFlight, LiveSnapshot};
use rustradar_map::Gpu;
use rustradar_map::io::{self, DirTiles};
use rustradar_map::jobs::Cancel;
use rustradar_map::layers::{buildings, imagery, vector_map};
use rustradar_scene::camera::ViewState;
use rustradar_scene::labels_map;
use rustradar_scene::tiles::TileIndex;

type Error = Box<dyn std::error::Error>;

/// The shots' clock: 2026-06-21 10:00 UTC.
const NOW_MS: f64 = 1_782_036_000_000.0;

fn main() -> Result<(), Error> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();
    match command.as_str() {
        "decode" => decode(&rest),
        "frame" => frame(&rest),
        "fixture" => write_fixture(&rest),
        _ => {
            eprintln!("usage: perf decode … | perf frame … | perf fixture DIR … (see the file's notes)");
            Ok(())
        }
    }
}

/// The value after `flag`, parsed.
fn arg<T: std::str::FromStr>(args: &[String], flag: &str) -> Option<T> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok())
}

fn has(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

/// Milliseconds.
fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Mean, median, 95th percentile and worst of `v` (milliseconds).
fn stats(v: &[f64]) -> (f64, f64, f64, f64) {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let at = |q: f64| s[((s.len() - 1) as f64 * q).round() as usize];
    (s.iter().sum::<f64>() / s.len() as f64, at(0.5), at(0.95), s[s.len() - 1])
}

// ---------------------------------------------------------------- decode

/// One decode step: its name, the source it reads, the zooms it decodes (none: every zoom) and
/// the step itself (returns a size to keep the work from being optimised away).
struct Step {
    name: &'static str,
    source: &'static str,
    zooms: Option<&'static [u32]>,
    run: fn(TileIndex, &[u8]) -> usize,
}

fn vector_step(tile: TileIndex, bytes: &[u8]) -> usize {
    vector_map::decode(tile, bytes, &Cancel::new()).ok().flatten().map_or(0, |t| t.fill_indices.len() / 3 + (t.lines.len() << 32))
}

fn labels_step(tile: TileIndex, bytes: &[u8]) -> usize {
    labels_map::decode(tile, bytes).len()
}

fn buildings_step(tile: TileIndex, bytes: &[u8]) -> usize {
    buildings::decode(tile, bytes, &Cancel::new()).ok().flatten().map_or(0, |_| 1)
}

fn imagery_step(tile: TileIndex, bytes: &[u8]) -> usize {
    imagery::decode(tile, bytes, &Cancel::new()).ok().flatten().map_or(0, |p| p.rgba.len())
}

const STEPS: [Step; 4] = [
    Step { name: "vector map", source: io::OPENFREEMAP, zooms: None, run: vector_step },
    Step { name: "labels", source: io::OPENFREEMAP, zooms: None, run: labels_step },
    Step { name: "buildings", source: io::OPENFREEMAP, zooms: Some(&rustradar_scene::tess::buildings::SOURCE_ZOOMS), run: buildings_step },
    Step { name: "imagery", source: io::ESRI, zooms: None, run: imagery_step },
];

fn decode(args: &[String]) -> Result<(), Error> {
    let repeat: usize = arg(args, "--repeat").unwrap_or(3).max(1);
    let top: usize = arg(args, "--top").unwrap_or(5);
    // `--only z/x/y`: one tile (for a profiler); `--step NAME`: one step
    let only: Option<String> = arg(args, "--only");
    let only_step: Option<String> = arg(args, "--step");
    let tiles = DirTiles::fixtures();
    println!("tiles from {}; each step run {repeat} times per tile, the fastest kept", tiles.root().display());
    for step in &STEPS {
        if only_step.as_ref().is_some_and(|name| !step.name.starts_with(name.as_str())) {
            continue;
        }
        let mut by_zoom: BTreeMap<u32, Vec<f64>> = BTreeMap::new();
        let mut slowest: Vec<(f64, TileIndex, usize)> = Vec::new();
        // what each zoom's tiles made (the vector map: triangles and path segments)
        let mut made: BTreeMap<u32, usize> = BTreeMap::new();
        let mut list = tiles.tiles(step.source);
        list.sort_by_key(|t| (t.z, t.x, t.y));
        for tile in list {
            if step.zooms.is_some_and(|z| !z.contains(&tile.z)) {
                continue;
            }
            if only.as_ref().is_some_and(|o| *o != format!("{}/{}/{}", tile.z, tile.x, tile.y)) {
                continue;
            }
            let Ok(Some(bytes)) = tiles.read(step.source, tile) else { continue };
            let mut best = f64::INFINITY;
            let mut out = 0;
            for _ in 0..repeat {
                let t = Instant::now();
                out = std::hint::black_box((step.run)(tile, std::hint::black_box(&bytes)));
                best = best.min(ms(t.elapsed()));
            }
            by_zoom.entry(tile.z).or_default().push(best);
            slowest.push((best, tile, bytes.len()));
            *made.entry(tile.z).or_default() += out;
        }
        println!("\n{} ({}):", step.name, step.source);
        let vector = step.name == "vector map";
        println!("  zoom  tiles   mean ms  median ms   worst ms{}", if vector { "  triangles  segments" } else { "" });
        let mut all = Vec::new();
        for (z, v) in &by_zoom {
            let (mean, median, _, worst) = stats(v);
            let m = made.get(z).copied().unwrap_or(0);
            let counts = if vector { format!("  {:>9}  {:>8}", m & 0xffff_ffff, m >> 32) } else { String::new() };
            println!("  {z:>4}  {:>5}  {mean:>8.2}  {median:>9.2}  {worst:>9.2}{counts}", v.len());
            all.extend(v);
        }
        if !all.is_empty() {
            let (mean, median, p95, worst) = stats(&all);
            println!("  all   {:>5}  {mean:>8.2}  {median:>9.2}  {worst:>9.2}  (p95 {p95:.2}, total {:.0} ms)", all.len(), all.iter().sum::<f64>());
        }
        slowest.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (t, tile, len) in slowest.iter().take(top) {
            println!("  slowest: {}/{}/{} ({} KB) {t:.2} ms", tile.z, tile.x, tile.y, len / 1024);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- frame

/// A deterministic number in [0, 1) for `(n, salt)` (as the app tests' synthetic snapshot).
fn unit(n: u32, salt: u32) -> f64 {
    let mut x = n.wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    f64::from(x) / 4_294_967_296.0
}

/// `(ICAO type, icon class)`, cycled (the app tests' synthetic snapshot's types).
const TYPES: [(&str, &str); 16] = [
    ("A20N", "A320"),
    ("B738", "B738"),
    ("A359", "A330"),
    ("B77W", "B777"),
    ("E190", "E190"),
    ("CRJ9", "CRJ9"),
    ("DH8D", "Q300"),
    ("C172", "C206"),
    ("EC35", "EC"),
    ("BALL", "BALL"),
    ("", "GRND"),
    ("CONC", "CONC"),
    ("A388", "A380"),
    ("B744", "B747"),
    ("ZZZZ", ""),
    ("GLID", "GLID"),
];

/// `count` flights spread over `[south, west, north, east]`, airborne, every tenth nameless.
fn snapshot(count: u32, [south, west, north, east]: [f64; 4]) -> LiveSnapshot {
    let flights = (0..count)
        .map(|n| {
            let (typecode, icon) = TYPES[n as usize % TYPES.len()];
            let named = n % 10 != 9;
            let name = |prefix: &str| if named { format!("{prefix}{n:04}") } else { String::new() };
            LiveFlight {
                id: 1000 + n,
                lat: (south + (north - south) * unit(n, 1)) as f32,
                lon: (west + (east - west) * unit(n, 2)) as f32,
                track: (360.0 * unit(n, 5)) as i32,
                alt: 1000 + (40_000.0 * unit(n, 3)) as i32,
                speed: 150 + (350.0 * unit(n, 4)) as i32,
                on_ground: false,
                timestamp_ms: (NOW_MS - 2000.0) as u64,
                callsign: name("SYN"),
                flight: name("SY"),
                reg: name("SE-"),
                typecode: typecode.to_owned(),
                origin: String::new(),
                destination: String::new(),
                icon: icon.to_owned(),
                positions: Vec::new(),
                vspeed: None,
            }
        })
        .collect();
    LiveSnapshot { flights, server_time_ms: NOW_MS as u64, requests: 1, errors: Vec::new(), elapsed_ms: 120, authenticated: false }
}

/// `LAT,LON,ZOOM[,PITCH[,BEARING]]`.
fn parse_view(text: &str) -> Option<ViewState> {
    let n: Vec<f64> = text.split(',').map(|v| v.trim().parse().ok()).collect::<Option<_>>()?;
    let (latitude, longitude, zoom) = (*n.first()?, *n.get(1)?, *n.get(2)?);
    let pitch = n.get(3).copied().unwrap_or(0.0);
    let bearing = n.get(4).copied().unwrap_or(0.0);
    Some(ViewState { longitude, latitude, zoom, bearing, pitch, position: None })
}

/// A `--fixture` folder holding `snapshot` (the tiles are the committed set, given to the map).
fn fixture_dir(snapshot: &LiveSnapshot) -> Result<PathBuf, Error> {
    let dir = std::env::temp_dir().join(format!("rustradar-perf-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("live_flights.json"), serde_json::to_vec(snapshot)?)?;
    Ok(dir)
}

/// `fixture DIR [--aircraft N] [--view …] [--size WxH]`: a `--fixture` folder for the app's shots:
/// `N` synthetic aircraft over the view (as `frame` makes them) and the committed tiles linked in.
fn write_fixture(args: &[String]) -> Result<(), Error> {
    let dir = PathBuf::from(args.first().ok_or("fixture wants a folder")?);
    let count: u32 = arg(args, "--aircraft").unwrap_or(2000);
    let view = args.iter().position(|a| a == "--view").and_then(|i| args.get(i + 1)).and_then(|v| parse_view(v));
    let view = view.unwrap_or(ViewState { longitude: 10.0, latitude: 52.0, zoom: 4.5, bearing: 0.0, pitch: 0.0, position: None });
    let size: [f64; 2] = args
        .iter()
        .position(|a| a == "--size")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.split_once('x').and_then(|(w, h)| Some([w.parse().ok()?, h.parse().ok()?])))
        .unwrap_or([1440.0, 900.0]);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("live_flights.json"), serde_json::to_vec(&snapshot(count, visible_bounds(&view, size)))?)?;
    let tiles = dir.join("tiles");
    if !tiles.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(io::fixture_tiles_dir(), &tiles)?;
    }
    println!("wrote {} aircraft into {}", count, dir.display());
    Ok(())
}

fn frame(args: &[String]) -> Result<(), Error> {
    let count: u32 = arg(args, "--aircraft").unwrap_or(2000);
    let frames: usize = arg(args, "--frames").unwrap_or(120).max(1);
    let view = args.iter().position(|a| a == "--view").and_then(|i| args.get(i + 1)).and_then(|v| parse_view(v));
    let view = view.unwrap_or(ViewState { longitude: 10.0, latitude: 52.0, zoom: 4.5, bearing: 0.0, pitch: 0.0, position: None });
    let size: [u32; 2] = args
        .iter()
        .position(|a| a == "--size")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.split_once('x').and_then(|(w, h)| Some([w.parse().ok()?, h.parse().ok()?])))
        .unwrap_or([1440, 900]);
    let labels = has(args, "--labels");
    let pan = has(args, "--pan");
    let trace = has(args, "--trace");
    let basemap = match args.iter().position(|a| a == "--basemap").and_then(|i| args.get(i + 1)).map(String::as_str) {
        Some("satellite") => Basemap::Satellite,
        Some("realistic") => Basemap::Realistic,
        _ => Basemap::Dark,
    };

    let gpu = Gpu::headless()?;
    let css = size.map(f64::from);
    // the aircraft over the visible part of the map
    let bounds = visible_bounds(&view, css);
    let dir = fixture_dir(&snapshot(count, bounds))?;
    let startup = Startup { basemap: Some(basemap), fixture: Some(dir.clone()), demo: false };
    let mut state = AppState::new(startup, None, None, None, view, css);
    state.persisted.settings.aircraft_labels = labels;
    // layers off, to see what each one costs
    if has(args, "--no-map-labels") {
        state.persisted.settings.labels = false;
    }
    if has(args, "--no-night") {
        state.persisted.settings.daylight = false;
    }
    let mut engine = Engine::new(&gpu, FORMAT, state);
    engine.map.set_tiles(Arc::new(DirTiles::at(io::fixture_tiles_dir())) as Arc<dyn io::TileFetcher>, None);
    let target = Offscreen::new(&gpu, size);
    let input = |t: f64| egui::RawInput {
        time: Some(t),
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0] as f32, size[1] as f32))),
        ..Default::default()
    };
    let poll = |gpu: &Gpu| gpu.device.poll(wgpu::PollType::wait_indefinitely()).map(|_| ());

    // settle: the tiles in, the frames undrawn as a shot's are (--draw-settle draws them)
    engine.draw = has(args, "--draw-settle");
    let started = Instant::now();
    let mut settle_frames = 0;
    loop {
        let t = Instant::now();
        engine.frame(&gpu, target.view(), size, 1.0, input(0.0), NOW_MS);
        let cpu = ms(t.elapsed());
        let t = Instant::now();
        poll(&gpu)?;
        settle_frames += 1;
        if trace {
            println!(
                "  settle frame {settle_frames}: at {:.0} ms, CPU {cpu:.1} ms, GPU {:.1} ms, pending: {}",
                ms(started.elapsed()),
                ms(t.elapsed()),
                engine.map.pending_layers().join(", ")
            );
        }
        if !engine.map.pending() || started.elapsed() > Duration::from_secs(300) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    println!(
        "{count} aircraft at {:.2},{:.2} zoom {} on {}x{}, labels {}: settled in {:.0} ms ({settle_frames} frames){}",
        view.latitude,
        view.longitude,
        view.zoom,
        size[0],
        size[1],
        if labels { "on" } else { "off" },
        ms(started.elapsed()),
        if engine.map.pending() { format!(", still pending: {}", engine.map.pending_layers().join(", ")) } else { String::new() }
    );
    let latest = &engine.scene.aircraft.latest;
    println!(
        "  drawn: {} icons, {} mesh instances, {} labelled",
        latest.icons.len(),
        latest.groups.iter().map(|g| g.instances.len()).sum::<usize>(),
        latest.labelled.len()
    );

    // warm-up: undrawn frames moving as the measured ones will, so what the first frames after
    // settling do once (new tiles' uploads, new glyphs) is not counted
    let warmup: usize = arg(args, "--warmup").unwrap_or(30);
    let mut now = NOW_MS;
    for i in 0..warmup {
        now += 16.0;
        if pan {
            pan_step(&mut engine);
        }
        engine.pacer.force();
        engine.frame(&gpu, target.view(), size, 1.0, input(0.5 + i as f64 / 600.0), now);
        poll(&gpu)?;
    }
    // 1: whole frames, drawn, the scene refresh forced each frame (as when the camera is locked
    // on an aircraft close in); with --pan the camera moves each frame, so nothing laid out for
    // the last one is kept
    engine.draw = true;
    let (mut whole, mut gpu_wait) = (Vec::new(), Vec::new());
    let drawn: usize = arg(args, "--drawn").unwrap_or(frames);
    for i in 0..drawn {
        now += 16.0;
        if pan {
            pan_step(&mut engine);
        }
        engine.pacer.force();
        let t = Instant::now();
        measured_frame(&mut engine, &gpu, &target, size, input(1.0 + i as f64 / 60.0), now);
        whole.push(ms(t.elapsed()));
        let t = Instant::now();
        poll(&gpu)?;
        gpu_wait.push(ms(t.elapsed()));
    }
    // 2: the scene builders, then the map's CPU side (update and prepare, no drawing), alone
    let (build, map_cpu) = cpu_loop(&gpu, &mut engine, frames, pan, &mut now, css);
    println!("  {frames} frames each, the scene refresh forced each frame{}:", if pan { ", the camera panning" } else { "" });
    println!("                        mean ms  median ms   p95 ms  worst ms");
    for (name, v) in [("whole frame (CPU)", &whole), ("GPU wait (SwiftShader)", &gpu_wait), ("scene builders", &build), ("map update+prepare", &map_cpu)] {
        if v.is_empty() {
            continue;
        }
        let (mean, median, p95, worst) = stats(v);
        println!("  {name:<22} {mean:>7.2}  {median:>9.2}  {p95:>7.2}  {worst:>8.2}");
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// One measured frame (a function of its own for a profiler's `--toggle-collect=*measured_frame*`).
#[inline(never)]
fn measured_frame(engine: &mut Engine, gpu: &Gpu, target: &Offscreen, size: [u32; 2], input: egui::RawInput, now: f64) {
    engine.frame(gpu, target.view(), size, 1.0, input, now);
}

/// Moves the camera east by a few pixels' worth.
fn pan_step(engine: &mut Engine) {
    let mut v = engine.state.controller.view();
    v.longitude += 0.002 * 2f64.powf(-v.zoom + 9.0);
    engine.state.controller.set_view(v);
}

/// The scene builders, then the map's update and prepare, `frames` times, timed apart (a
/// function of its own for a profiler's `--toggle-collect=*cpu_loop*`).
#[inline(never)]
fn cpu_loop(gpu: &Gpu, engine: &mut Engine, frames: usize, pan: bool, now: &mut f64, css: [f64; 2]) -> (Vec<f64>, Vec<f64>) {
    let (mut build, mut map_cpu) = (Vec::new(), Vec::new());
    for _ in 0..frames {
        *now += 16.0;
        if pan {
            pan_step(engine);
        }
        let clocks = engine.state.clocks(*now);
        let t = Instant::now();
        let scene = engine.scene.build(&mut engine.state, clocks, 1.0);
        build.push(ms(t.elapsed()));
        let t = Instant::now();
        engine.map.set_camera(engine.state.controller.view(), css);
        engine.map.update(scene);
        engine.map.prepare(gpu, *now);
        map_cpu.push(ms(t.elapsed()));
        gpu.queue.submit(std::iter::empty::<wgpu::CommandBuffer>());
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    }
    (build, map_cpu)
}

/// The `[south, west, north, east]` the canvas of `size` shows, from points 5 % in from its
/// corners and edges (on the globe, beside the limb where it does not reach).
fn visible_bounds(view: &ViewState, size: [f64; 2]) -> [f64; 4] {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for fx in [0.05, 0.5, 0.95] {
        for fy in [0.05, 0.5, 0.95] {
            if let Some([lon, lat]) = rustradar_scene::controller::map_at(view, size, [size[0] * fx, size[1] * fy]) {
                b = [b[0].min(lat), b[1].min(lon), b[2].max(lat), b[3].max(lon)];
            }
        }
    }
    b
}
