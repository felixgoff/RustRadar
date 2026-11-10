//! Item A10's acceptance shot: locked on a climbing aircraft, the camera keeps it at the centre of
//! the screen within 2 px, on the globe and on the flat map, frame after frame of the climb.
//!
//! The app's state runs the lock as the window does (`AppState::advance_camera` at 60 frames a
//! second, the follow lock of `state::camera` re-aiming each frame), and its scene builders make
//! the scene the map draws, over the committed tile fixtures. The aircraft layers are not drawn
//! yet (items A4, A5), so a test layer marks where the aircraft is drawn: a small disc at the pose
//! the motion model gives at the frame's feed clock, raised as the aircraft builder raises it
//! (`elevation(altFt, effectiveExaggeration(exaggeration, zoom))`). The shot measures the disc's
//! centre on the image the GPU drew.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. Shots go to `target/shots/a10-lock-*.png`.

mod common;

use std::sync::Arc;

use common::shots::{Rgba, gpu, map_shot_with_tiles, save};
use rustradar_app::scene_builder::SceneBuilder;
use rustradar_app::state::{AppState, Startup};
use rustradar_core::geo::{Basemap, effective_exaggeration, elevation};
use rustradar_core::model::LiveFlight;
use rustradar_map::headless::Image;
use rustradar_map::io::DirTiles;
use rustradar_map::layers::{DrawCx, Init, Layer, PrepareCx};
use rustradar_map::pipelines::circle;
use rustradar_map::state::DrawState;
use rustradar_scene::camera::ViewState;

/// The local clock at the first frame: the app's shot clock (2026-06-21 10:00 UTC).
const T0: f64 = common::NOW_MS;
/// The marker's colour: nothing on the dark map is magenta.
const MARK: Rgba = [255, 0, 255, 255];
/// The marker's radius, CSS px.
const MARK_PX: f64 = 4.0;

/// Climbing out of Arlanda's runway 01L at 3 000 ft/min from 1 500 ft, 180 kt on a track of 10°:
/// the report the feed has at local time `t`, one every 8 s, each reported 2 s before it arrives,
/// climbed and moved on along the track since the first.
fn climbing(t: f64) -> LiveFlight {
    let n = ((t - T0) / 8000.0).floor();
    let at = T0 + n * 8000.0 - 2000.0;
    // 180 kt for 8 s is 741 m; along 10° that is 0.00657° north and 0.00229° east at 59.65°N
    let (dlat, dlon) = (0.006_567 * n, 0.002_287 * n);
    LiveFlight {
        id: 7,
        lat: (59.6519 + dlat) as f32,
        lon: (17.9186 + dlon) as f32,
        track: 10,
        alt: 1500 + (n as i32) * 400,
        speed: 180,
        vspeed: Some(3000),
        timestamp_ms: at as u64,
        callsign: "SAS1".into(),
        typecode: "A20N".into(),
        icon: "A320".into(),
        ..Default::default()
    }
}

/// A test layer: one disc of [`MARK`] at `[lon, lat, metres]`, drawn over everything.
struct Marker {
    pipeline: circle::Circle,
    style: circle::Style,
    instances: circle::Instances,
    at: [f64; 3],
}

impl Marker {
    fn layer(init: &Init<'_>, at: [f64; 3]) -> Box<dyn Layer> {
        let device = &init.gpu.device;
        let pipeline = circle::Circle::new(init, "a10 marker", DrawState::LABEL);
        let style = pipeline.style(device, &circle::CircleStyle { billboard: true, filled: true, stroked: false, ..circle::CircleStyle::PIXELS });
        Box::new(Marker { pipeline, style, instances: circle::Instances::new(device, "a10 marker"), at })
    }
}

impl Layer for Marker {
    fn name(&self) -> &'static str {
        "a10 marker"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        let [lon, lat, h] = self.at;
        let disc = circle::Instance::at(cx.view, [lon, lat], h).radius(MARK_PX).fill(MARK);
        self.instances.write(&cx.gpu.device, &cx.gpu.queue, &[disc]);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {
        self.pipeline.draw(pass, &self.style, &self.instances);
    }
}

/// The centre of the marker's pixels (device px) and how many there are.
fn marker_centre(image: &Image) -> Option<([f64; 2], usize)> {
    let (mut sx, mut sy, mut n) = (0.0, 0.0, 0usize);
    for y in 0..image.height {
        for x in 0..image.width {
            let [r, g, b, _] = image.pixel(x, y);
            if r > 200 && g < 80 && b > 200 {
                sx += f64::from(x) + 0.5;
                sy += f64::from(y) + 0.5;
                n += 1;
            }
        }
    }
    (n > 0).then(|| ([sx / n as f64, sy / n as f64], n))
}

/// Where the aircraft is drawn in the frame at local time `t` of `state` (the pose at the frame's
/// feed clock, raised as the aircraft builder raises it), and its pose's altitude, feet.
fn drawn_at(state: &mut AppState, t: f64) -> ([f64; 3], f64) {
    let clocks = state.clocks(t);
    let pose = state.motion.pose(&climbing(t), clocks.feed_ms);
    let zoom = state.controller.view().zoom;
    let up = elevation(pose.alt_ft, effective_exaggeration(state.persisted.settings.exaggeration, zoom));
    ([pose.lon, pose.lat, up], pose.alt_ft)
}

/// The shot of `state`'s frame at local time `t` with the marker at `at`, saved as `name`: the
/// marker's distance from the centre of the screen, CSS px.
fn shot(gpu: &rustradar_map::Gpu, state: &mut AppState, builder: &mut SceneBuilder, t: f64, scale: f64, at: [f64; 3], name: &str) -> f64 {
    let tiles = Arc::new(DirTiles::fixtures());
    let clocks = state.clocks(t);
    let view = state.controller.view();
    let scene = builder.build(state, clocks, scale);
    assert_eq!(scene.view, view);
    let (image, _) = map_shot_with_tiles(gpu, &scene, scale, tiles, |init| vec![Marker::layer(init, at)]);
    save(&image, name);
    let ([x, y], n) = marker_centre(&image).unwrap_or_else(|| panic!("{name}: the aircraft's marker is on screen"));
    let disc = std::f64::consts::PI * (MARK_PX * scale).powi(2);
    assert!(n as f64 > 0.6 * disc, "{name}: the whole marker shows ({n} px)");
    let [w, h] = state.controller.size();
    (x / scale - w / 2.0).hypot(y / scale - h / 2.0)
}

/// Item A10's acceptance: locked on the climbing aircraft from `start`, 20 s at 60 frames a second;
/// at 0, 10 and 20 s the shot shows the aircraft's marker within 2 px of the centre of the screen.
/// The same 20 s with the camera left alone show it well away from the centre: the lock does it.
#[test]
fn locked_on_a_climbing_aircraft_it_stays_at_the_centre_of_the_screen() {
    let Some(gpu) = gpu() else { return };
    let size = [1100.0, 720.0];
    let globe = ViewState { longitude: 17.95, latitude: 59.6, zoom: 9.0, bearing: 10.0, pitch: 50.0, position: None };
    let flat = ViewState { longitude: 17.92, latitude: 59.65, zoom: 14.0, bearing: 200.0, pitch: 60.0, position: None };
    let dark = || Startup { basemap: Some(Basemap::Dark), ..Default::default() };
    for (name, start, scale) in [("globe", globe, 1.0), ("flat", flat, 1.0), ("flat-x2", flat, 2.0)] {
        let mut state = AppState::new(dark(), None, None, None, start, size);
        state.camera.set_follow(true);
        let mut free = AppState::new(dark(), None, None, None, start, size);
        let mut builder = SceneBuilder::default();
        let mut altitudes = Vec::new();
        for frame in 0..=1200 {
            let t = T0 + f64::from(frame) * (1000.0 / 60.0);
            state.camera.set_selected(Some(climbing(t)));
            state.advance_camera(t);
            free.advance_camera(t);
            if frame % 600 != 0 {
                continue;
            }
            assert!(state.camera.following() && state.locked_steady(t), "{name}: locked");
            let (at, alt_ft) = drawn_at(&mut state, t);
            altitudes.push(alt_ft);
            let second = frame / 60;
            let off = shot(&gpu, &mut state, &mut builder, t, scale, at, &format!("a10-lock-{name}-{second:02}s.png"));
            let view = state.controller.view();
            eprintln!("{name} at {second} s: {alt_ft:.0} ft, zoom {:.2}, pitch {:.1}, the aircraft {off:.3} px from the centre", view.zoom, view.pitch);
            assert!(off <= 2.0, "{name} at {second} s: the aircraft is {off} px from the centre");
            if frame == 1200 && scale == 1.0 {
                let (at, _) = drawn_at(&mut free, t);
                let off = shot(&gpu, &mut free, &mut SceneBuilder::default(), t, scale, at, &format!("a10-free-{name}-{second:02}s.png"));
                eprintln!("{name} at {second} s, the camera left alone: {off:.1} px from the centre");
                assert!(off > 40.0, "{name}: without the lock the aircraft has moved away ({off} px)");
            }
        }
        assert!(altitudes.windows(2).all(|w| w[1] > w[0] + 400.0), "{name}: it climbs ({altitudes:?})");
    }
}
