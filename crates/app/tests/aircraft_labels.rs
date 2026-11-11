//! Item A7's acceptance shot: the aircraft labels with the stage-4 synthetic snapshot
//! (`common::fixtures`), read as the `--fixture` folder's `live_flights.json`, at V3 with the
//! aircraft labels on.
//!
//! The app's state reads the snapshot from the folder and its scene builders make the scene, which
//! is drawn over the committed tiles. A probe layer runs the map's aircraft label part on the
//! scene's labels, as the aircraft layer runs it, and keeps what it draws: where each label went,
//! its fade and its boxes. Today's pictures (`docs/port/reference/`) were taken without the backend
//! and hold no aircraft; the shots are looked at beside them for the map underneath.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. Shots go to `target/shots/a7-*.png`.

mod common;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use common::NOW_MS;
use common::fixtures;
use common::shots::{V2, V3, gpu, map_shot_with_tiles, save};
use rustradar_app::scene_builder::SceneBuilder;
use rustradar_app::state::{AppState, Startup};
use rustradar_core::geo::Basemap;
use rustradar_core::model::{LiveFlight, LiveSnapshot};
use rustradar_map::headless::Image;
use rustradar_map::io::DirTiles;
use rustradar_map::layers::aircraft::labels::{Labels, Placement, Shown};
use rustradar_map::layers::{DrawCx, Layer, PrepareCx};
use rustradar_map::{Gpu, SceneInput};
use rustradar_scene::aircraft_labels::AIRCRAFT_LABEL_SELECTED;
use rustradar_scene::camera::ViewState;
use rustradar_scene::declutter::Rect;

/// The reference pictures' canvas.
const SIZE: [u32; 2] = [1440, 900];

/// A `--fixture` folder holding the synthetic snapshot as `live_flights.json` and no tiles.
fn snapshot_folder() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("a7-synthetic");
    fixtures::write_fixture(&dir, &fixtures::synthetic_snapshot(NOW_MS)).expect("the fixture folder");
    dir
}

/// The app's state at `view` on the dark map with the snapshot of `folder` read and the aircraft
/// labels on, at the shots' clock.
fn state_at(folder: &Path, view: ViewState) -> AppState {
    let startup = Startup { basemap: Some(Basemap::Dark), fixture: Some(folder.to_path_buf()), demo: false };
    let mut state = AppState::new(startup, None, None, None, view, SIZE.map(f64::from));
    state.persisted.settings.aircraft_labels = true;
    state.poll(NOW_MS);
    assert!(!state.aircraft.flights().is_empty(), "the snapshot is in");
    state
}

/// The scene the app's scene builders make at `view` from the feed of `folder`.
fn app_scene(folder: &Path, view: ViewState) -> SceneInput {
    let mut state = state_at(folder, view);
    let clocks = state.clocks(NOW_MS);
    SceneBuilder::default().build(&mut state, clocks, 1.0)
}

/// What the probe saw: the labels drawn, and every label as placed.
#[derive(Default)]
struct Seen {
    shown: Vec<Shown>,
    placements: Vec<Placement>,
}

/// A probe: the aircraft label part run on the scene's labels as the aircraft layer runs it,
/// drawing nothing itself, keeping what it would draw.
struct Probe {
    labels: Labels,
    seen: Rc<RefCell<Seen>>,
}

impl Layer for Probe {
    fn name(&self) -> &'static str {
        "a7 probe"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.labels.prepare(cx, &cx.scene.aircraft.labels);
        *self.seen.borrow_mut() = Seen { shown: self.labels.shown().to_vec(), placements: self.labels.placements().to_vec() };
    }

    fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
}

/// `scene` drawn over the committed tiles, and what its labels did.
fn shot(gpu: &Gpu, scene: &SceneInput, name: &str) -> (Image, Seen) {
    let seen = Rc::new(RefCell::new(Seen::default()));
    let probe = seen.clone();
    let (image, _) = map_shot_with_tiles(gpu, scene, 1.0, Arc::new(DirTiles::fixtures()), move |init| {
        vec![Box::new(Probe { labels: Labels::new(init), seen: probe }) as Box<dyn Layer>]
    });
    save(&image, name);
    (image, seen.take())
}

/// A label's fill pixel: light grey to white, or the selection blue (the halo is dark navy, the
/// roads and borders darker greys).
fn is_text(p: [u8; 4]) -> bool {
    let [r, g, b, _] = p;
    r.min(g).min(b) >= 120 || (u16::from(b) > u16::from(r) + 80 && b >= 150)
}

fn text_pixels(image: &Image, r: Rect) -> usize {
    let clip = |v: f64, hi: u32| (v.round().max(0.0) as u32).min(hi);
    let (x0, x1, y0, y1) = (clip(r.x0, image.width), clip(r.x1, image.width), clip(r.y0, image.height), clip(r.y1, image.height));
    (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| is_text(image.pixel(x, y))).count()
}

/// Shown fully (the crisp curve's top: the fill at its own alpha).
fn full(s: &Shown) -> bool {
    s.fade >= 0.65
}

/// The labels of a shot as today's collision filter leaves them: each drawn one's callsign right
/// of its aircraft and its height under it; the text of no two drawn in full overlapping; and every
/// labelled aircraft on the screen either labelled in full or with another label's claimed box over
/// its point (a hidden label's box still covers).
fn check(image: &Image, scene: &SceneInput, seen: &Seen) {
    let labels = &scene.aircraft.labels;
    assert_eq!(seen.placements.len(), labels.len());
    for s in &seen.shown {
        let i = labels.set.index[&s.id];
        let gap = labels.gaps[i] * s.perspective;
        let name = s.name.expect("a callsign");
        let height = s.height_rect.expect("a height line");
        assert!((name.x0 - (s.anchor[0] + gap)).abs() < 1.5 * s.perspective, "{}: {name:?} from {:?}", s.text, s.anchor);
        assert!(name.y1 < s.anchor[1] + 3.0 && name.y0 < s.anchor[1] - 6.0, "{}: above", s.text);
        assert!(height.y0 > s.anchor[1] - 3.0 && height.y1 > s.anchor[1] + 6.0, "{}: below", s.text);
        assert!(height.x0 > s.anchor[0], "{}: right", s.text);
        assert!(s.name_color != AIRCRAFT_LABEL_SELECTED, "no selection");
        if full(s) && name.x1 < f64::from(SIZE[0]) && name.y0 > 0.0 {
            assert!(text_pixels(image, name) > 15, "{}: drawn", s.text);
        }
    }
    let full_ones: Vec<&Shown> = seen.shown.iter().filter(|s| full(s)).collect();
    for (k, a) in full_ones.iter().enumerate() {
        let a_text = a.name.unwrap().union(&a.height_rect.unwrap());
        for b in &full_ones[k + 1..] {
            let b_text = b.name.unwrap().union(&b.height_rect.unwrap());
            assert!(!a_text.overlaps(&b_text), "{} and {} overlap: {a_text:?} {b_text:?}", a.text, b.text);
        }
    }
    let [w, h] = SIZE.map(f64::from);
    for p in &seen.placements {
        let Some([x, y]) = p.anchor else { continue };
        if p.fade >= 0.65 || x < 3.0 || y < 3.0 || x > w - 3.0 || y > h - 3.0 {
            continue;
        }
        let covered = seen.placements.iter().any(|q| {
            q.id != p.id && q.priority >= p.priority && q.claimed.is_some_and(|c| c.x0 <= x + 2.0 && x - 2.0 <= c.x1 && c.y0 <= y + 2.0 && y - 2.0 <= c.y1)
        });
        assert!(covered, "{} faded to {} with nothing over it at {:?}", p.id, p.fade, p.anchor);
    }
}

/// Plan A7: V3 with the labels on, the plan's synthetic snapshot. Most of its Stockholm traffic
/// is drawn beyond this canvas; the labels of those on it all show.
#[test]
fn v3_callsigns_right_of_their_aircraft() {
    let Some(gpu) = gpu() else { return };
    let built = app_scene(&snapshot_folder(), V3.view());
    let labels = &built.aircraft.labels;
    let labelled = &built.aircraft.scene.labelled;
    assert!(labelled.len() > 20, "{} labelled", labelled.len());
    assert_eq!(labels.len(), labelled.len(), "one label per labelled aircraft");
    assert!(labels.with_height(), "zoom 9: the height line");
    let (image, seen) = shot(&gpu, &built, "a7-v3-labels.png");
    let on_screen = seen.placements.iter().filter(|p| p.anchor.is_some_and(|[x, y]| (0.0..1440.0).contains(&x) && (0.0..900.0).contains(&y))).count();
    eprintln!("V3: {} labelled, {on_screen} on the canvas, {} drawn", labelled.len(), seen.shown.len());
    assert!(seen.shown.len() >= 4 && seen.shown.len() == on_screen, "every label on the canvas drawn");
    check(&image, &built, &seen);
}

/// The busiest airspace at V3: 160 named aircraft spread over the canvas at low altitude. Many
/// lose their label to a more important neighbour's claimed box, the rest show without
/// overlapping.
#[test]
fn v3_dense_traffic_none_overlapping() {
    let Some(gpu) = gpu() else { return };
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("a7-dense");
    fixtures::write_fixture(&dir, &dense_snapshot()).expect("the fixture folder");
    let built = app_scene(&dir, V3.view());
    let labels = &built.aircraft.labels;
    let (image, seen) = shot(&gpu, &built, "a7-v3-dense.png");
    let full_count = seen.shown.iter().filter(|s| full(s)).count();
    eprintln!("dense V3: {} labelled, {} drawn, {full_count} in full", labels.len(), seen.shown.len());
    assert!(labels.len() >= 150, "{}", labels.len());
    assert!(full_count >= 30 && full_count < labels.len() - 30, "{full_count} of {} in full", labels.len());
    check(&image, &built, &seen);
}

/// 160 flights within the canvas of V3 at 1440 × 900, from 1 000 to 9 000 ft, named `TST000` on.
fn dense_snapshot() -> LiveSnapshot {
    let mut snapshot = fixtures::synthetic_snapshot(NOW_MS);
    let types = [("A20N", "A320"), ("B738", "B738"), ("E190", "E190"), ("AT76", "ATR72"), ("A359", "A330")];
    snapshot.flights = (0..160u32)
        .map(|n| {
            // an integer hash, as the synthetic snapshot's
            let unit = |salt: u32| {
                let mut x = n.wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
                x ^= x >> 16;
                x = x.wrapping_mul(0x7FEB_352D);
                x ^= x >> 15;
                x = x.wrapping_mul(0x846C_A68B);
                x ^= x >> 16;
                f64::from(x) / 4_294_967_296.0
            };
            let (typecode, icon) = types[n as usize % types.len()];
            LiveFlight {
                id: 5000 + n,
                lat: (59.33 + 1.0 * (unit(1) - 0.5)) as f32,
                lon: (18.07 + 3.4 * (unit(2) - 0.5)) as f32,
                alt: 1000 + (8000.0 * unit(3)) as i32,
                speed: 250,
                track: (360.0 * unit(4)) as i32,
                timestamp_ms: NOW_MS as u64 - 2000,
                callsign: format!("TST{n:03}"),
                typecode: typecode.into(),
                icon: icon.into(),
                ..Default::default()
            }
        })
        .collect();
    snapshot
}

/// V2 (zoom 4.5): no height line, the claimed boxes spread 1.5 times, labels kept further apart.
#[test]
fn v2_callsigns_alone_kept_further_apart() {
    let Some(gpu) = gpu() else { return };
    let built = app_scene(&snapshot_folder(), V2.view());
    let labels = &built.aircraft.labels;
    assert!(!labels.with_height(), "zoom 4.5: no height line");
    assert!((labels.layout.spread - 1.5).abs() < 1e-9);
    let (image, seen) = shot(&gpu, &built, "a7-v2-labels.png");
    let shown = &seen.shown;
    eprintln!("V2: {} labelled, {} drawn, {} in full", labels.len(), shown.len(), shown.iter().filter(|s| full(s)).count());
    assert!(!shown.is_empty());
    for s in shown.iter().filter(|s| full(s)) {
        let name = s.name.unwrap();
        assert!(s.height_rect.is_none() && s.height.is_empty());
        // centred on its aircraft's middle
        assert!(((name.y0 + name.y1) / 2.0 - s.anchor[1]).abs() < 2.0, "{}: {name:?} at {:?}", s.text, s.anchor);
        if name.x1 < f64::from(SIZE[0]) {
            assert!(text_pixels(&image, name) > 10, "{}: drawn", s.text);
        }
    }
}
