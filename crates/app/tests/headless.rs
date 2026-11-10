//! The `--screenshot` path end to end (E17): the app's frames (backend started, state, scene
//! builders, map, egui pass) drawn without a window, and the binary run as a person would.
//!
//! Needs a wgpu adapter; without a GPU use SwiftShader or lavapipe through `VK_ICD_FILENAMES`.
//! Without one the tests print `skipped: …` and pass.

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

use rustradar_app::headless::{self, Image};
use rustradar_app::Options;
use rustradar_core::geo::Basemap;
use rustradar_map::{Gpu, GpuError};
use rustradar_scene::camera::{GlobeViewport, ViewState};

const LAND: [u8; 4] = [50, 50, 50, 255];
const INK: [u8; 4] = [30, 30, 30, 255];

/// One device for the whole file, used one test at a time: SwiftShader crashes when several open
/// their own at once. `None` (after printing why) without an adapter.
fn gpu() -> Option<MutexGuard<'static, Gpu>> {
    static GPU: OnceLock<Option<Mutex<Gpu>>> = OnceLock::new();
    let gpu = GPU.get_or_init(|| match Gpu::headless() {
        Ok(gpu) => Some(Mutex::new(gpu)),
        Err(GpuError::NoAdapter(e)) => {
            eprintln!("skipped: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    });
    match gpu {
        Some(gpu) => Some(gpu.lock().unwrap_or_else(|poisoned| poisoned.into_inner())),
        None => {
            eprintln!("skipped: no graphics adapter");
            None
        }
    }
}

fn shots_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
    std::fs::create_dir_all(&dir).expect("shot folder");
    dir
}

/// How far from the centre E1's camera puts the limb of deck's globe along `angle` (radians, x
/// right, y down), CSS pixels.
fn limb(globe: &GlobeViewport, angle: f64) -> f64 {
    let centre = [globe.width / 2.0, globe.height / 2.0];
    let (mut inside, mut outside) = (0.0, globe.width.max(globe.height));
    for _ in 0..60 {
        let r = (inside + outside) / 2.0;
        if globe.ray_distance_to_globe_center_ratio([centre[0] + r * angle.cos(), centre[1] + r * angle.sin()]) < 1.0 {
            inside = r;
        } else {
            outside = r;
        }
    }
    (inside + outside) / 2.0
}

/// Where the land's edge is along `angle` from the image's centre, device pixels: where the red
/// channel, sampled every 0.25 px, first falls below halfway between land and ink.
fn edge(image: &Image, angle: f64) -> f64 {
    let centre = [f64::from(image.width) / 2.0, f64::from(image.height) / 2.0];
    let half = f64::from(LAND[0] + INK[0]) / 2.0;
    let mut r = 0.0;
    loop {
        let (x, y) = (centre[0] + r * angle.cos(), centre[1] + r * angle.sin());
        assert!(x >= 0.0 && y >= 0.0 && x < f64::from(image.width) && y < f64::from(image.height), "no edge at {angle}");
        if f64::from(image.pixel(x as u32, y as u32)[0]) < half {
            return r;
        }
        r += 0.25;
    }
}

/// The acceptance of E17: `--screenshot` renders the globe of E5 through `map::headless`, with the
/// backend started. The disc of land is `(50,50,50)` on `(30,30,30)`, its edge where E1 puts the
/// globe's limb (within 1.5 px, as E5's own check), at scale 1 and 2.
#[test]
fn a_shot_draws_the_globe_of_e5_with_the_backend_started() {
    let Some(gpu) = gpu() else { return };
    for (size, scale) in [([1100, 720], 1.0), ([640, 400], 2.0)] {
        let options = Options { size: Some(size), scale: Some(scale), now_ms: Some(1.76e12), ..Default::default() };
        let shot = headless::render_on(&gpu, &options).expect("shot");
        eprintln!("{size:?} at {scale}: {}", shot.adapter);
        assert!(shot.backend, "the backend started");
        let image = &shot.image;
        assert_eq!([image.width, image.height], headless::surface_size(size, scale));
        let (w, h) = (image.width, image.height);
        assert_eq!(image.pixel(w / 2, h / 2), LAND, "the globe at the centre");
        for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            assert_eq!(image.pixel(x, y), INK, "space at the corner ({x}, {y})");
        }
        let globe = GlobeViewport::new(&ViewState::INITIAL, f64::from(size[0]), f64::from(size[1]));
        let mut checked = 0;
        for i in 0..16 {
            let angle = f64::from(i) * std::f64::consts::FRAC_PI_8 + 0.1;
            let expected = limb(&globe, angle) * scale;
            // only where the limb is on the picture
            let (x, y) = (f64::from(w) / 2.0 + (expected + 3.0) * angle.cos(), f64::from(h) / 2.0 + (expected + 3.0) * angle.sin());
            if x < 0.0 || y < 0.0 || x >= f64::from(w) || y >= f64::from(h) {
                continue;
            }
            let measured = edge(image, angle);
            assert!((measured - expected).abs() <= 1.5, "{size:?} ×{scale}: edge at {measured:.2} px, the limb at {expected:.2} px");
            checked += 1;
        }
        assert!(checked >= 4, "{size:?} ×{scale}: the limb is on the picture in only {checked} directions");
        image.save_png(&shots_dir().join(format!("e17-v1-{}x{}@{scale}.png", size[0], size[1]))).expect("png");
    }
}

/// Satellite has no land rectangle: the earth's own colour fills the disc instead.
#[test]
fn a_satellite_shot_has_the_earth_but_no_land() {
    let Some(gpu) = gpu() else { return };
    let options = Options { basemap: Some(Basemap::Satellite), size: Some([640, 400]), ..Default::default() };
    let image = headless::render_on(&gpu, &options).expect("shot").image;
    let centre = image.pixel(320, 200);
    assert_ne!(centre, INK);
    assert_ne!(centre, LAND);
    assert_eq!(image.pixel(0, 0), INK);
}

/// The binary itself: `--screenshot` with every request sent to a proxy that is not there (no
/// network) starts the backend, writes the PNG and exits cleanly, without a panic.
#[test]
fn the_binary_writes_a_shot_without_the_network() {
    let Some(_gpu) = gpu() else { return };
    let out = shots_dir().join("e17-binary-v1.png");
    let _ = std::fs::remove_file(&out);
    let dead = "http://127.0.0.1:9";
    let run = Command::new(env!("CARGO_BIN_EXE_rustradar-app"))
        .args(["--screenshot", out.to_str().expect("utf-8 path"), "--size", "800x500", "--view", "35,10,1.9"])
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
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!stderr.contains("panicked"), "a panic:\n{stderr}");
    assert!(run.status.success(), "exit {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}", run.status);
    assert!(stdout.contains("backend started"), "{stdout}");
    let file = std::io::BufReader::new(std::fs::File::open(&out).expect("the PNG was written"));
    let mut reader = png::Decoder::new(file).read_info().expect("a PNG");
    let mut rgba = vec![0; reader.output_buffer_size().expect("its size")];
    let info = reader.next_frame(&mut rgba).expect("its pixels");
    assert_eq!((info.width, info.height, info.color_type), (800, 500, png::ColorType::Rgba));
    let image = Image { width: 800, height: 500, rgba };
    assert_eq!(image.pixel(400, 250), LAND);
    assert_eq!(image.pixel(0, 0), INK);
}
