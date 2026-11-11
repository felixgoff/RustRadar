//! Item A5's acceptance shot: aircraft on the ground on Arlanda's runways at V4, drawn as lit 3D
//! models, each lying along its runway.
//!
//! The runways are the committed vector tiles' own centre lines (OpenFreeMap's `aeroway` runways,
//! read with the vector map's tessellation). Airliners stand on them at points V4 shows, each
//! reported with its runway's course (or the opposite one) as its track, parked, with no callsign
//! (so no label is drawn). They are written to a `--fixture` folder's `live_flights.json`; the app's
//! state reads it, the motion model poses them and the app's scene builders make the scene, which
//! the map draws over the committed tiles, as `--screenshot` does. The same scene with no flight is
//! the background; the pixels brighter than it are the aircraft (their shadows only darken).
//!
//! Each aircraft's pixels (its blob) are laid back onto the ground plane at the height of the
//! model's middle, and the principal axis of that footprint is taken. V4 looks down at 45°, so the
//! fin and the fuselage's height tilt a footprint's axis off the nose axis: by 1° to 10° on these
//! aircraft, more the further the heading is from the view's (the Q400's high tail most). So the
//! axis is read back as a heading through the model's own shape: the CPU places the same model's
//! triangles (deck's formulas, in f64) turned by a few degrees either way, measures each footprint
//! the same way, and the turn whose axis matches the blob's gives the heading drawn. That heading
//! must lie within 2° of its runway's course (the worst is 1.5°, the B738's), and the blob must be
//! the CPU's model as posed rather than half a turn away.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. Shots go to `target/shots/a5-*.png`.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::NOW_MS;
use common::shots::{V4, gpu, save};
use rustradar_app::scene_builder::SceneBuilder;
use rustradar_app::state::{AppState, Startup};
use rustradar_core::geo::Basemap;
use rustradar_core::geo::sphere::{bearing, destination, distance_m};
use rustradar_core::model::{LiveFlight, LiveSnapshot};
use rustradar_map::SceneInput;
use rustradar_map::frame::ViewFrame;
use rustradar_map::headless::Image;
use rustradar_map::layers::aircraft::assets::published;
use rustradar_map::pipelines::mesh::{local_frame, orientation_matrix};
use rustradar_scene::aircraft::{Group, GroupModel, MeshInstance};
use rustradar_map::io::{DirTiles, fixture_tiles_dir};
use rustradar_scene::camera::{viewport_for, world_to_lng_lat};
use rustradar_scene::geom::path::LocalMercator;
use rustradar_scene::tess::vector::tessellate;
use rustradar_scene::tiles::TileIndex;

/// The canvas, CSS px, drawn at three times its size (the models stand at their 24 px floor).
const SIZE: [u32; 2] = [1100, 720];
const SCALE: f64 = 3.0;

/// The types that stand on the runways, cycled through: published models long for their span, so
/// a footprint's longer axis is its nose axis.
const TYPES: [(&str, &str); 6] = [("A321", "A320"), ("B738", "B738"), ("B744", "B747"), ("E190", "E190"), ("B753", "B757"), ("DH8D", "Q300")];

/// One runway centre line's segment of a tile: its two ends, `[lon, lat]`.
type Segment = [[f64; 2]; 2];

/// The runway centre lines of the committed z14 tiles round V4 (a tile's lines reach a little past
/// its edges, so a stretch can come twice).
fn runway_segments() -> Vec<Segment> {
    let dir = fixture_tiles_dir().join("openfreemap/14");
    let mut segments = Vec::new();
    for x in 9003..=9012 {
        for y in 4784..=4793 {
            let Ok(bytes) = std::fs::read(dir.join(format!("{x}/{y}.pbf"))) else { continue };
            let tile = TileIndex::new(x, y, 14);
            let decoded = tessellate(tile, &bytes, &|| false).expect("the tile decodes");
            let local = LocalMercator::tile(tile);
            let at = |p: [f32; 3]| world_to_lng_lat(local.to_world([f64::from(p[0]), f64::from(p[1])]).to_array());
            for range in &decoded.runway_lines {
                for s in &decoded.lines[range.start as usize..range.end as usize] {
                    segments.push([at(s.start), at(s.end)]);
                }
            }
        }
    }
    segments
}

/// Where the aircraft stand: points along the runways, 150 m apart, that V4 shows well inside the
/// canvas, at least 100 px from one another; each with its runway's course. At most `n`.
fn stands(n: usize) -> Vec<([f64; 2], f64)> {
    let viewport = viewport_for(&V4.view(), SIZE.map(f64::from));
    let mut segments = runway_segments();
    assert!(!segments.is_empty(), "Arlanda's runways are in the committed tiles");
    segments.sort_by(|a, b| distance_m(b[0], b[1]).total_cmp(&distance_m(a[0], a[1])));
    let mut stands: Vec<([f64; 2], f64, [f64; 2])> = Vec::new();
    for [a, b] in segments {
        let length = distance_m(a, b);
        let course = bearing(a, b);
        let mut along = 75.0;
        while along < length && stands.len() < n {
            let p = destination(a, course, along);
            along += 150.0;
            let [x, y, _] = viewport.project([p[0], p[1], 0.0]);
            let inside = (90.0..f64::from(SIZE[0]) - 90.0).contains(&x) && (90.0..f64::from(SIZE[1]) - 60.0).contains(&y);
            if inside && stands.iter().all(|(_, _, s)| (s[0] - x).hypot(s[1] - y) > 100.0) {
                stands.push((p, course, [x, y]));
            }
        }
    }
    stands.into_iter().map(|(p, course, _)| (p, course)).collect()
}

/// The aircraft: the `i`-th parked at `stands[i]`, turned along its runway, every other one the
/// opposite way, reported two seconds before the clock, with no callsign, flight or registration.
fn flights(stands: &[([f64; 2], f64)]) -> Vec<LiveFlight> {
    stands
        .iter()
        .enumerate()
        .map(|(i, &([lon, lat], course))| {
            let (typecode, icon) = TYPES[i % TYPES.len()];
            let track = if i % 2 == 0 { course } else { course + 180.0 };
            LiveFlight {
                id: 5000 + i as u32,
                lat: lat as f32,
                lon: lon as f32,
                track: track.round() as i32 % 360,
                alt: 0,
                speed: 0,
                on_ground: true,
                timestamp_ms: (NOW_MS - 2000.0) as u64,
                typecode: typecode.into(),
                icon: icon.into(),
                ..Default::default()
            }
        })
        .collect()
}

/// A `--fixture` folder holding `flights` as the feed's snapshot (its server time the clock).
fn folder(name: &str, flights: Vec<LiveFlight>) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let snapshot = LiveSnapshot { flights, server_time_ms: NOW_MS as u64, requests: 1, ..Default::default() };
    common::fixtures::write_fixture(&dir, &snapshot).expect("the fixture folder");
    dir
}

/// The scene the app makes of the folder's snapshot at V4, as `--screenshot` does.
fn app_scene(dir: PathBuf, flights: usize) -> SceneInput {
    let startup = Startup { basemap: Some(Basemap::Dark), fixture: Some(dir), demo: false };
    let mut state = AppState::new(startup, None, None, None, V4.view(), SIZE.map(f64::from));
    state.poll(NOW_MS);
    assert_eq!(state.aircraft.flights().len(), flights, "the snapshot is in");
    let clocks = state.clocks(NOW_MS);
    SceneBuilder::default().build(&mut state, clocks, SCALE)
}

/// `scene` drawn by the map's layers over the committed tiles.
fn shot(gpu: &rustradar_map::Gpu, scene: &SceneInput) -> Image {
    common::shots::map_shot_with_tiles(gpu, scene, SCALE, Arc::new(DirTiles::fixtures()), |_| Vec::new()).0
}

/// The pixels of `image` brighter than `bg` (the channels' sum up by more than 60 levels).
fn brighter(image: &Image, bg: &Image) -> Vec<bool> {
    image.rgba.chunks_exact(4).zip(bg.rgba.chunks_exact(4)).map(|(p, b)| (0..3).map(|c| i32::from(p[c]) - i32::from(b[c])).sum::<i32>() > 60).collect()
}

/// The 8-connected group of `mask`'s pixels holding the one nearest `[x, y]` (device px), if one
/// lies within `reach`.
fn blob_at(mask: &[bool], [w, h]: [u32; 2], [x, y]: [f64; 2], reach: f64) -> Vec<[u32; 2]> {
    let r = reach.ceil() as i64;
    let mut seed = None;
    let mut best = f64::INFINITY;
    for yy in (y as i64 - r).max(0)..(y as i64 + r).min(i64::from(h)) {
        for xx in (x as i64 - r).max(0)..(x as i64 + r).min(i64::from(w)) {
            let d = (xx as f64 + 0.5 - x).hypot(yy as f64 + 0.5 - y);
            if mask[(yy as u32 * w + xx as u32) as usize] && d < best && d <= reach {
                best = d;
                seed = Some([xx as u32, yy as u32]);
            }
        }
    }
    let Some(seed) = seed else { return Vec::new() };
    let mut seen = vec![false; mask.len()];
    let mut stack = vec![seed];
    seen[(seed[1] * w + seed[0]) as usize] = true;
    let mut blob = Vec::new();
    while let Some([px, py]) = stack.pop() {
        blob.push([px, py]);
        for dy in -1i64..=1 {
            for dx in -1i64..=1 {
                let (nx, ny) = (i64::from(px) + dx, i64::from(py) + dy);
                if nx < 0 || ny < 0 || nx >= i64::from(w) || ny >= i64::from(h) {
                    continue;
                }
                let i = (ny as u32 * w + nx as u32) as usize;
                if mask[i] && !seen[i] {
                    seen[i] = true;
                    stack.push([nx as u32, ny as u32]);
                }
            }
        }
    }
    blob
}

/// The device pixels whose centres the CPU puts inside `m`'s airframe and gear, the aircraft
/// turned `turn` degrees clockwise from its heading: the triangles of its model placed by deck's
/// formulas in f64.
fn cpu_blob(view: &ViewFrame, group: &Group, m: &MeshInstance, turn: f64) -> Vec<[u32; 2]> {
    let GroupModel::Published { name, lod, .. } = &group.model else { panic!("a published model") };
    let model = published().unwrap().get(name).unwrap();
    let [w, h] = [(view.size[0] * SCALE) as u32, (view.size[1] * SCALE) as u32];
    let mut inside = std::collections::BTreeSet::new();
    let frame = local_frame(view, [m.position[0], m.position[1]]);
    let origin = view.viewport.project_position(m.position);
    // a vector of the camera's type
    let vector = |[x, y, z]: [f64; 3]| {
        let mut v = origin;
        (v.x, v.y, v.z) = (x, y, z);
        v
    };
    // deck's yaw is minus the heading; the gear's lift turns with it
    let orientation = [m.orientation[0], m.orientation[1] - turn, m.orientation[2]];
    let (sin, cos) = (-turn).to_radians().sin_cos();
    let turned = |[x, y, z]: [f64; 3]| [x * cos - y * sin, x * sin + y * cos, z];
    let mut meshes = vec![(&model.lods[*lod].mesh, m.scale, [0.0; 3])];
    for g in group.gear.iter().filter(|g| group.instances[g.instance].id == m.id) {
        meshes.push((&model.gear.as_ref().unwrap().mesh, g.scale, turned(g.translation)));
    }
    for (mesh, scale, translation) in meshes {
        let r = orientation_matrix(orientation, scale);
        let screen = |i: u16| {
            let p = &mesh.positions[3 * usize::from(i)..3 * usize::from(i) + 3];
            let common = origin + frame * (r * vector([0, 1, 2].map(|k| f64::from(p[k]))) + vector(translation));
            let [x, y, _] = view.viewport.world_to_pixels(common);
            [x * SCALE, y * SCALE]
        };
        let edge = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
        for t in mesh.indices.chunks_exact(3) {
            let [a, b, c] = [screen(t[0]), screen(t[1]), screen(t[2])];
            let area = edge(a, b, c);
            if area == 0.0 {
                continue;
            }
            let x0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as u32;
            let x1 = (a[0].max(b[0]).max(c[0]).ceil().max(0.0) as u32).min(w);
            let y0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as u32;
            let y1 = (a[1].max(b[1]).max(c[1]).ceil().max(0.0) as u32).min(h);
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = [f64::from(x) + 0.5, f64::from(y) + 0.5];
                    let [u, v, s] = [edge(b, c, p), edge(c, a, p), edge(a, b, p)].map(|e| e / area);
                    if u >= 0.0 && v >= 0.0 && s >= 0.0 {
                        inside.insert([x, y]);
                    }
                }
            }
        }
    }
    inside.into_iter().collect()
}

/// The longer principal axis of `points` (metres east and north), degrees clockwise from north in
/// [0, 180), and the ratio of the two second moments.
fn principal_axis(points: &[[f64; 2]]) -> (f64, f64) {
    let n = points.len() as f64;
    let [me, mn] = [0, 1].map(|k| points.iter().map(|p| p[k]).sum::<f64>() / n);
    let (mut see, mut snn, mut sen) = (0.0, 0.0, 0.0);
    for [e, no] in points {
        let (de, dn) = (e - me, no - mn);
        see += de * de;
        snn += dn * dn;
        sen += de * dn;
    }
    // the direction (east, north) of the larger eigenvalue, as a course
    let theta = 0.5 * (2.0 * sen).atan2(see - snn);
    let (e, n) = (theta.cos(), theta.sin());
    let course = e.atan2(n).to_degrees().rem_euclid(180.0);
    let mean = (see + snn) / 2.0;
    let spread = (((see - snn) / 2.0).powi(2) + sen * sen).sqrt();
    (course, (mean + spread) / (mean - spread))
}

/// The difference of two axes, degrees, modulo half a turn.
fn axis_turn(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(180.0);
    d.min(180.0 - d)
}

/// Item A5's acceptance: at V4, aircraft on the ground at Arlanda lie along their runways: each
/// model's footprint has its principal axis within 2° of the runway's course.
#[test]
fn at_v4_aircraft_on_arlandas_runways_lie_along_them() {
    let Some(gpu) = gpu() else { return };
    let stands = stands(8);
    let flights = flights(&stands);
    eprintln!("{} aircraft on the runways", flights.len());
    assert!(flights.len() >= 6, "{} stands on screen", flights.len());
    let scene = app_scene(folder("a5-arlanda", flights.clone()), flights.len());
    let bg_scene = app_scene(folder("a5-arlanda-empty", Vec::new()), 0);
    let aircraft = &scene.aircraft.scene;
    assert!(aircraft.icons.is_empty(), "V4 is pitched: models, not icons");
    assert_eq!(aircraft.mesh_count(), flights.len(), "every one drawn as a model");

    let image = shot(&gpu, &scene);
    let bg = shot(&gpu, &bg_scene);
    save(&image, "a5-v4-arlanda.png");
    save(&bg, "a5-v4-arlanda-plain.png");
    let size = [image.width, image.height];
    let mask = brighter(&image, &bg);
    let viewport = viewport_for(&scene.view, scene.size);
    let vf = ViewFrame::new(&scene.view, scene.size, SCALE, 0.0);
    let metres_per_degree = 6_371_008.8 * std::f64::consts::PI / 180.0;
    let mut measured = 0;
    for group in &aircraft.groups {
        for m in &group.instances {
            let f = flights.iter().find(|f| f.id == m.id).unwrap();
            let runway = stands[(m.id - 5000) as usize].1;
            // the motion model keeps a parked aircraft where and as it is reported
            assert!(axis_turn(m.pose.heading, f64::from(f.track)) < 1e-9, "aircraft {}: posed {} for a track of {}", m.id, m.pose.heading, f.track);
            let [lon, lat, h] = m.position;
            let [x, y, _] = viewport.project([lon, lat, h]);
            let blob = blob_at(&mask, size, [x * SCALE, y * SCALE], 6.0 * SCALE);
            assert!(blob.len() > 100, "aircraft {}: {} px", m.id, blob.len());
            // each pixel back on the ground plane at the model's middle, metres from its origin
            let cos = lat.to_radians().cos();
            let lay = |pixels: &[[u32; 2]]| -> Vec<[f64; 2]> {
                pixels
                    .iter()
                    .map(|&[px, py]| {
                        let [plon, plat] = viewport.unproject([(f64::from(px) + 0.5) / SCALE, (f64::from(py) + 0.5) / SCALE], h);
                        [(plon - lon) * cos * metres_per_degree, (plat - lat) * metres_per_degree]
                    })
                    .collect()
            };
            let (axis, ratio) = principal_axis(&lay(&blob));
            // the footprint's axis read as a heading through the model's own shape: the turn of
            // the CPU's model whose footprint has that axis (seen at 45°, the fin and the
            // fuselage's height tilt a footprint's axis by a few degrees, the more the further the
            // heading is from the view's)
            let cpu_axis = |turn: f64| principal_axis(&lay(&cpu_blob(&vf, group, m, turn))).0;
            let fit = |turns: &mut dyn Iterator<Item = f64>| turns.min_by(|a, b| axis_turn(cpu_axis(*a), axis).total_cmp(&axis_turn(cpu_axis(*b), axis))).unwrap();
            let coarse = fit(&mut (-12..=12).map(|k| f64::from(k) * 0.5));
            let turn = fit(&mut (-10..=10).map(|k| coarse + f64::from(k) * 0.05));
            let heading = m.pose.heading + turn;
            let off = axis_turn(heading, runway);
            // and not half a turn away: the blob is the CPU's model as posed, not turned round
            let pixels = |b: &[[u32; 2]]| b.iter().map(|p| (p[1] * size[0] + p[0]) as usize).collect::<std::collections::HashSet<usize>>();
            let (gpu, right, wrong) = (pixels(&blob), pixels(&cpu_blob(&vf, group, m, 0.0)), pixels(&cpu_blob(&vf, group, m, 180.0)));
            let iou = |c: &std::collections::HashSet<usize>| gpu.intersection(c).count() as f64 / gpu.union(c).count() as f64;
            let (right, wrong) = (iou(&right), iou(&wrong));
            eprintln!(
                "aircraft {} ({}, {}): {} px; its footprint's axis {axis:.2}° (moments {ratio:.2}:1), {:.2}° off the runway's {:.2}°, {:.2}° off the CPU's model's; read as a heading {heading:.2}° (the model turned {turn:+.2}°): {off:.2}° off the runway; {right:.2} of the CPU's pixels, {wrong:.2} of them half turned",
                m.id,
                f.typecode,
                group.id(),
                blob.len(),
                axis_turn(axis, runway),
                runway.rem_euclid(180.0),
                axis_turn(axis, cpu_axis(0.0)),
            );
            assert!(ratio > 1.2 && off < 2.0, "aircraft {}: {off}° off its runway", m.id);
            assert!(right > 0.6 && wrong < right - 0.1, "aircraft {}: {right} of the CPU's pixels, {wrong} half turned", m.id);
            measured += 1;
        }
    }
    assert_eq!(measured, flights.len());
}
