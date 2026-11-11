//! Item A4's acceptance shots: the aircraft icons with the stage-4 synthetic snapshot
//! (`common::fixtures`), read as the `--fixture` folder's `live_flights.json`.
//!
//! The app's state reads the snapshot from the folder and its scene builders make the scene. The
//! icon shots keep only the scene's icons (the meshes, shadows and labels are other items' and
//! are checked by their own shots) and draw them over the map with no tiles; the same scene
//! without the icons is drawn too, and the pixels that differ are the silhouettes and their
//! borders. The map shot draws the app's whole scene over the committed tiles, to be looked at
//! beside `docs/port/reference/dark-v2-1440x900.png` (which has no aircraft: it was taken without
//! the backend).
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. Shots go to `target/shots/a4-*.png`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::fixtures::{self, ARLANDA, STOCKHOLM, WORLD};
use common::shots::{Component, Rect, V1, V2, V3, components, gpu, map_shot, map_shot_with_tiles, save};
use common::NOW_MS;
use rustradar_app::scene_builder::SceneBuilder;
use rustradar_app::scene_builder::aircraft::Aircraft;
use rustradar_app::state::{AppState, Startup};
use rustradar_core::geo::Basemap;
use rustradar_map::headless::Image;
use rustradar_map::io::DirTiles;
use rustradar_map::layers::aircraft::assets;
use rustradar_map::{Gpu, SceneInput};
use rustradar_scene::aircraft::{AircraftBuilder, AircraftScene, Icon};
use rustradar_scene::camera::{GLOBE_RADIUS, ViewState, viewport_for};

/// The reference pictures' canvas.
const SIZE: [u32; 2] = [1440, 900];

/// Fewer pixels than this, next to a blob, make a crumb of its edge ([`aircraft_blobs`]).
const CRUMB: usize = 4;

/// A `--fixture` folder holding the synthetic snapshot as `live_flights.json` and no tiles.
fn snapshot_folder() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("a4-synthetic");
    fixtures::write_fixture(&dir, &fixtures::synthetic_snapshot(NOW_MS)).expect("the fixture folder");
    dir
}

/// The app's state at `view` on the dark map with the snapshot of `folder` read, at the shots'
/// clock.
fn state_at(folder: &Path, view: ViewState) -> AppState {
    let startup = Startup { basemap: Some(Basemap::Dark), fixture: Some(folder.to_path_buf()), demo: false };
    let mut state = AppState::new(startup, None, None, None, view, SIZE.map(f64::from));
    state.poll(NOW_MS);
    assert_eq!(state.aircraft.flights().len(), WORLD + STOCKHOLM + ARLANDA, "the snapshot is in");
    state
}

/// The snapshot at `view` as the app makes it: the scene builders' scene, and its aircraft built
/// again with `selected` as the selected flight (A3's builder called as the scene builder calls
/// it, with the selection item A8 will hand it).
fn app_scene(view: ViewState, selected: Option<u32>) -> (SceneInput, AircraftScene) {
    let mut state = state_at(&snapshot_folder(), view);
    let clocks = state.clocks(NOW_MS);
    let built = SceneBuilder::default().build(&mut state, clocks, 1.0);
    let mut inputs = Aircraft::inputs(&state, &built);
    inputs.selected = selected;
    let (feed_now, local_now) = (built.feed_now_ms, built.local_now_ms);
    let AppState { aircraft: feed, motion, liveries, .. } = &mut state;
    let reference = feed.reference().map(|r| &**r);
    let aircraft = AircraftBuilder::new().build(&inputs, assets::published(), feed.shown(None).iter(), |f| motion.pose(f, feed_now), |f| {
        liveries.for_flight(f, reference, local_now)
    });
    if selected.is_none() {
        assert_eq!(aircraft.icons, built.aircraft.scene.icons, "the scene builder's icons");
    }
    (built, aircraft)
}

/// `built`'s map with `icons` as the only aircraft.
fn with_icons(built: &SceneInput, aircraft: &AircraftScene, icons: Vec<Icon>) -> SceneInput {
    let mut scene = SceneInput::new(built.view, built.size, built.map_scale, built.basemap);
    scene.local_now_ms = built.local_now_ms;
    scene.feed_now_ms = built.feed_now_ms;
    scene.earth = built.earth.clone();
    scene.aircraft.scene = AircraftScene { frame: aircraft.frame, icons, hovered_icon: aircraft.hovered_icon, ..Default::default() };
    scene
}

/// The icons of one view: the shot with them and without them.
struct Shot {
    aircraft: AircraftScene,
    /// The icons drawn wholly on the canvas, in full view ([`on_screen`]).
    at: Vec<(usize, [f64; 2])>,
    /// Those near the globe's limb, which the sphere may partly hide.
    grazing: Vec<(usize, [f64; 2])>,
    image: Image,
    plain: Image,
}

impl Shot {
    /// Pixel `(x, y)` with the icons, and without.
    fn pixels(&self, x: u32, y: u32) -> ([u8; 4], [u8; 4]) {
        (self.image.pixel(x, y), self.plain.pixel(x, y))
    }

    /// How many pixels within `r` (CSS px: the shots are at scale 1) of `[x, y]` `pick` keeps.
    fn count_within(&self, [x, y]: [f64; 2], r: f64, pick: impl Fn([u8; 4], [u8; 4]) -> bool) -> usize {
        let near = |px: u32, py: u32| (f64::from(px) + 0.5 - x).hypot(f64::from(py) + 0.5 - y) <= r;
        Rect::around(&self.image, [x, y], r).pixels().filter(|&(px, py)| near(px, py) && pick(self.image.pixel(px, py), self.plain.pixel(px, py))).count()
    }
}

/// The snapshot's icons at `view` (with `selected` selected) over the map with no tiles, saved as
/// `name`.
fn icon_shot(gpu: &Gpu, view: ViewState, selected: Option<u32>, name: &str) -> Shot {
    let (built, aircraft) = app_scene(view, selected);
    assert!(aircraft.frame.use_icons, "straight down below zoom 11: icons");
    let scene = with_icons(&built, &aircraft, aircraft.icons.clone());
    let at = on_screen(&scene, &aircraft.icons, Sight::Clear);
    let grazing = on_screen(&scene, &aircraft.icons, Sight::Grazing);
    let (image, _) = map_shot(gpu, &scene, 1.0, |_| Vec::new());
    let (plain, _) = map_shot(gpu, &with_icons(&built, &aircraft, Vec::new()), 1.0, |_| Vec::new());
    save(&image, name);
    Shot { aircraft, at, grazing, image, plain }
}

/// A silhouette's or a border's pixel: off the map without icons by more than 20 levels (the
/// border's faint outer edge left out).
fn marked(p: [u8; 4], map: [u8; 4]) -> bool {
    (0..3).any(|c| p[c].abs_diff(map[c]) > 20)
}

/// A silhouette's pixel: brighter than the map by at least 15 levels (a border only darkens it).
/// The smallest icons, a light aircraft at the globe's size floor (8 · 0.6 px), are drawn from the
/// atlas's smallest mip levels and are faint grey, as in deck.
fn lit(p: [u8; 4], map: [u8; 4]) -> bool {
    u16::from(p[0].max(p[1]).max(p[2])) >= u16::from(map[0].max(map[1]).max(map[2])) + 15
}

/// How far a silhouette and its border reach from the icon's middle, CSS px: the silhouette lies
/// within a circle a little more than `screenPx` across (its largest dimension, the wing tips set
/// back from the middle), the border 9 cell texels beyond it, plus a pixel of antialiased edge.
fn extent(icon: &Icon) -> f64 {
    icon.size_px / 1.06 * 0.55 + 9.0 / 128.0 * icon.size_px + 1.0
}

/// How far apart (CSS px) two icons' middles must be for their borders never to touch.
fn reach(a: &Icon, b: &Icon) -> f64 {
    extent(a) + extent(b)
}

/// How the earth stands between the camera and an icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sight {
    /// In full view: the flat map, or on the globe on the camera's side, not near the limb.
    Clear,
    /// Near the limb: the sphere's depth may hide part of the quad (as deck's does).
    Grazing,
    /// Behind the limb: the sphere's depth hides it (as deck's does).
    Hidden,
}

/// [`Sight`] of an aircraft at `position` from the camera of `scene`.
fn sight(scene: &SceneInput, position: [f64; 3]) -> Sight {
    let viewport = viewport_for(&scene.view, scene.size);
    if !viewport.is_globe() {
        return Sight::Clear;
    }
    let (p, camera) = (viewport.project_position(position), viewport.camera_position);
    let ray = p - camera;
    // the point of the line of sight nearest the earth's centre
    let t = (-camera.dot(ray) / ray.length_squared()).clamp(0.0, 1.0);
    if t < 1.0 && (camera + ray * t).length() < GLOBE_RADIUS {
        return Sight::Hidden;
    }
    // the cosine between the way up there and the way to the camera
    let facing = p.normalize().dot(-ray.normalize());
    if facing > 0.25 { Sight::Clear } else { Sight::Grazing }
}

/// Where each icon's aircraft is on the canvas, CSS px, for those drawn wholly on it (within
/// [`extent`] of the position, inside the canvas) and seen as `seen`.
fn on_screen(scene: &SceneInput, icons: &[Icon], seen: Sight) -> Vec<(usize, [f64; 2])> {
    let viewport = viewport_for(&scene.view, scene.size);
    icons
        .iter()
        .enumerate()
        .filter_map(|(i, icon)| {
            let [x, y, _] = viewport.project(icon.position);
            let e = extent(icon);
            let inside = x >= e && y >= e && x + e <= scene.size[0] && y + e <= scene.size[1];
            (inside && sight(scene, icon.position) == seen).then_some((i, [x, y]))
        })
        .collect()
}

/// The aircraft blobs of `shot`: the 8-connected groups of [`marked`] pixels (a small silhouette's
/// antialiased wings touch only at their corners), each holding [`lit`] ones; crumbs of an edge
/// cut off from their icon (fewer than [`CRUMB`] pixels, 3 px or less from a larger blob) left
/// out, and blobs on the canvas's edge (an aircraft partly off it, not counted by [`on_screen`]).
fn aircraft_blobs(shot: &Shot) -> Vec<Component> {
    let (w, h) = (shot.image.width as usize, shot.image.height as usize);
    let kept: Vec<bool> = (0..w * h).map(|i| {
        let (p, map) = shot.pixels((i % w) as u32, (i / w) as u32);
        marked(p, map)
    }).collect();
    let mut seen = vec![false; w * h];
    let mut blobs = Vec::new();
    for start in 0..w * h {
        if !kept[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        let mut stack = vec![start];
        let (mut pixels, mut sum, mut lo, mut hi) = (0usize, [0.0f64; 2], [usize::MAX; 2], [0usize; 2]);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            pixels += 1;
            sum = [sum[0] + x as f64 + 0.5, sum[1] + y as f64 + 0.5];
            lo = [lo[0].min(x), lo[1].min(y)];
            hi = [hi[0].max(x + 1), hi[1].max(y + 1)];
            for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if kept[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        let bounds = Rect::new(lo[0] as u32, lo[1] as u32, (hi[0] - lo[0]) as u32, (hi[1] - lo[1]) as u32);
        blobs.push(Component { pixels, bounds, centroid: [sum[0] / pixels as f64, sum[1] / pixels as f64] });
    }
    let gap = |a: &Component, b: &Component| {
        let gap = |lo0: u32, hi0: u32, lo1: u32, hi1: u32| lo1.saturating_sub(hi0).max(lo0.saturating_sub(hi1));
        let (a, b) = (a.bounds, b.bounds);
        gap(a.x0, a.x1, b.x0, b.x1).max(gap(a.y0, a.y1, b.y0, b.y1))
    };
    let crumb = |c: &Component| c.pixels < CRUMB && blobs.iter().any(|b| b.pixels >= CRUMB && gap(c, b) <= 3);
    let blobs: Vec<Component> = blobs.iter().filter(|b| !crumb(b)).cloned().collect();
    for blob in &blobs {
        let any = blob.bounds.pixels().any(|(x, y)| {
            let (p, map) = shot.pixels(x, y);
            lit(p, map)
        });
        assert!(any, "a blob at {:?} with no silhouette in it", blob.centroid);
    }
    let (width, height) = (shot.image.width, shot.image.height);
    blobs.into_iter().filter(|b| b.bounds.x0 > 0 && b.bounds.y0 > 0 && b.bounds.x1 < width && b.bounds.y1 < height).collect()
}

/// How many groups `icons` make when any two within [`reach`] of each other are one group.
fn groups(icons: &[Icon], at: &[(usize, [f64; 2])]) -> usize {
    fn root(parent: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while parent[r] != r {
            r = parent[r];
        }
        parent[i] = r;
        r
    }
    let mut parent: Vec<usize> = (0..at.len()).collect();
    for a in 0..at.len() {
        for b in a + 1..at.len() {
            let ((ia, pa), (ib, pb)) = (at[a], at[b]);
            if (pa[0] - pb[0]).hypot(pa[1] - pb[1]) < reach(&icons[ia], &icons[ib]) {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                parent[ra] = rb;
            }
        }
    }
    (0..at.len()).filter(|&i| root(&mut parent, i) == i).count()
}

/// The icons on screen with no other within [`reach`].
fn lone(icons: &[Icon], at: &[(usize, [f64; 2])]) -> Vec<(usize, [f64; 2])> {
    at.iter()
        .copied()
        .filter(|&(i, [x, y])| at.iter().all(|&(j, [u, v])| i == j || (x - u).hypot(y - v) >= reach(&icons[i], &icons[j])))
        .collect()
}

/// The blobs within `r` of `[x, y]` (their centroids).
fn blobs_near(blobs: &[Component], [x, y]: [f64; 2], r: f64) -> Vec<&Component> {
    blobs.iter().filter(|b| (b.centroid[0] - x).hypot(b.centroid[1] - y) < r).collect()
}

/// The blobs of the snapshot's icons at `view`, with the groups the icons on screen make and
/// those standing alone; checked: each blob is on an icon, each icon on screen is drawn, each
/// icon alone is one blob of its own.
struct Counted {
    shot: Shot,
    /// The icons in full view clear of the limb's.
    counted: usize,
    blobs: Vec<Component>,
    groups: usize,
    lone: Vec<(usize, [f64; 2])>,
}

fn counted(gpu: &Gpu, view: ViewState, name: &str) -> Counted {
    let shot = icon_shot(gpu, view, None, &format!("a4-{name}-icons.png"));
    let icons = &shot.aircraft.icons;
    // the blobs of the icons near the limb are not counted (the sphere may cut one in two)
    let near_limb = |b: &Component| shot.grazing.iter().any(|&(i, xy)| (b.centroid[0] - xy[0]).hypot(b.centroid[1] - xy[1]) < reach(&icons[i], &icons[i]));
    let all_blobs = aircraft_blobs(&shot);
    let clear_of_limb: Vec<(usize, [f64; 2])> = shot
        .at
        .iter()
        .copied()
        .filter(|&(i, [x, y])| shot.grazing.iter().all(|&(j, [u, v])| (x - u).hypot(y - v) >= reach(&icons[i], &icons[j]) + extent(&icons[j])))
        .collect();
    let on_counted = |b: &Component| clear_of_limb.iter().any(|&(i, xy)| (b.centroid[0] - xy[0]).hypot(b.centroid[1] - xy[1]) < extent(&icons[i]));
    let blobs: Vec<Component> = all_blobs.iter().filter(|b| !near_limb(b) && on_counted(b)).cloned().collect();
    let (groups, lone) = (groups(icons, &clear_of_limb), lone(icons, &clear_of_limb));
    eprintln!(
        "{name}: {} icons built, {} in full view on screen ({} clear of the limb) in {groups} groups, {} alone, {} near the limb; {} blobs",
        icons.len(),
        shot.at.len(),
        clear_of_limb.len(),
        lone.len(),
        shot.grazing.len(),
        blobs.len()
    );
    let viewport = viewport_for(&view, SIZE.map(f64::from));
    for blob in &all_blobs {
        let near = icons.iter().any(|icon| {
            let [x, y, _] = viewport.project(icon.position);
            (blob.centroid[0] - x).hypot(blob.centroid[1] - y) < extent(icon)
        });
        assert!(near, "{name}: a blob at {:?} with no aircraft under it", blob.centroid);
    }
    for &(i, xy) in &shot.at {
        let drawn = shot.count_within(xy, extent(&icons[i]), lit);
        assert!(drawn >= 2, "{name}: icon {} ({}) at {xy:?}: {drawn} lit pixels", icons[i].id, icons[i].name);
    }
    for &(i, xy) in &lone {
        let own = blobs_near(&blobs, xy, extent(&icons[i]));
        assert_eq!(own.len(), 1, "{name}: icon {} ({}) alone at {xy:?}: {} blobs", icons[i].id, icons[i].name, own.len());
    }
    Counted { shot, blobs, groups, lone, counted: clear_of_limb.len() }
}

/// The counts of [`counted`]: each lone icon one blob (checked there); the crowded ones at least
/// one blob per group (any two whose borders could touch are one) and, as a border drawn over a
/// lower neighbour may split it, at most one per icon, within 5 %.
fn assert_counts(c: &Counted, name: &str) {
    let most = c.counted + (0.05 * c.counted as f64).ceil() as usize;
    assert!(c.blobs.len() >= c.groups && c.blobs.len() <= most, "{name}: {} blobs for {} icons in {} groups", c.blobs.len(), c.counted, c.groups);
}

/// Item A4's acceptance, first half: at V2 every icon on screen shows as a silhouette in its border
/// and nothing else is drawn. The 30 flights round Stockholm crowd into a patch some 70 px across
/// at zoom 4.5, where silhouettes overlap, so the blobs are counted against the groups the icons
/// make ([`assert_counts`]); those standing alone make one blob each.
#[test]
fn v2_with_the_synthetic_snapshot_shows_a_silhouette_for_every_icon_on_screen() {
    let Some(gpu) = gpu() else { return };
    let c = counted(&gpu, V2.view(), "v2");
    assert!(c.counted >= 20, "the snapshot puts aircraft over Europe and round Stockholm ({} on screen)", c.counted);
    assert!(c.lone.len() >= 3);
    assert_counts(&c, "v2");
}

/// The same on the whole globe (V1: the 200 flights worldwide, a quarter of them crowding another
/// at zoom 1.9, those near the limb partly behind the earth and left out), and round Stockholm at
/// zoom 9 (V3), where no icon crowds another: there the blobs are the icons on screen. The largest
/// silhouettes are white through the middle.
#[test]
fn on_the_globe_and_close_in_each_icon_is_drawn() {
    let Some(gpu) = gpu() else { return };
    let v1 = counted(&gpu, V1.view(), "v1");
    assert!(v1.counted >= 50 && v1.lone.len() >= 30, "v1: {} counted, {} alone", v1.counted, v1.lone.len());
    assert_counts(&v1, "v1");
    let v3 = counted(&gpu, V3.view(), "v3");
    assert!(v3.counted >= 5 && v3.groups == v3.counted, "v3: {} icons in {} groups", v3.counted, v3.groups);
    assert_eq!(v3.blobs.len(), v3.counted, "v3: one blob per icon");
    for c in [&v1, &v3] {
        let white = c.shot.image.rgba.chunks_exact(4).filter(|p| p[0] >= 250 && p[1] >= 250 && p[2] >= 250).count();
        assert!(white > 10, "{white} white pixels");
    }
}

/// Item A4's acceptance, second half: a selected aircraft's icon is selection blue `(55,142,240)`
/// and the others stay as they were; the largest icon standing alone at V2 is selected.
#[test]
fn a_selected_icon_is_selection_blue() {
    let Some(gpu) = gpu() else { return };
    let plain = icon_shot(&gpu, V2.view(), None, "a4-v2-icons.png");
    let icons = &plain.aircraft.icons;
    let alone = lone(icons, &plain.at);
    let &(chosen, xy) = alone.iter().max_by(|a, b| icons[a.0].size_px.total_cmp(&icons[b.0].size_px)).expect("an icon alone");
    let id = icons[chosen].id;
    let selected = icon_shot(&gpu, V2.view(), Some(id), "a4-v2-selected.png");
    let icon = selected.aircraft.icons.iter().find(|i| i.id == id).expect("still an icon");
    assert_eq!(icon.color, [55, 142, 240, 255]);
    assert!(selected.aircraft.icons.iter().filter(|i| i.id != id).all(|i| i.color == [255; 4]));
    let r = extent(icon);
    let blue = |p: [u8; 4], _: [u8; 4]| p[0].abs_diff(55) <= 2 && p[1].abs_diff(142) <= 2 && p[2].abs_diff(240) <= 2;
    let white = |p: [u8; 4], _: [u8; 4]| p[0] >= 250 && p[1] >= 250 && p[2] >= 250;
    let (blue_now, white_now) = (selected.count_within(xy, r, blue), selected.count_within(xy, r, white));
    let (blue_before, white_before) = (plain.count_within(xy, r, blue), plain.count_within(xy, r, white));
    eprintln!("icon {id} ({}, {:.1} px) at {xy:?}: {blue_now} blue and {white_now} white px selected, {blue_before} and {white_before} before", icon.name, icon.size_px);
    assert!(blue_now >= 5 && white_now == 0, "selected: {blue_now} blue, {white_now} white");
    assert!(white_before >= 5 && blue_before == 0, "not selected: {blue_before} blue, {white_before} white");
    let elsewhere = Rect::whole(&selected.image)
        .pixels()
        .filter(|&(px, py)| (f64::from(px) - xy[0]).hypot(f64::from(py) - xy[1]) > r && selected.image.pixel(px, py) != plain.image.pixel(px, py))
        .count();
    assert_eq!(elsewhere, 0, "nothing else changed");
}

/// The app's whole V2 scene with the snapshot over the committed tiles, to be looked at beside
/// today's picture: the silhouettes' white middles (pure white: the place names are
/// `(241,238,231)`) are all at aircraft. Most are icons; the aircraft with no published model
/// (balloons, ground vehicles: item A5's built-in meshes, white too) are drawn as meshes beside
/// them.
#[test]
fn the_app_draws_the_icons_over_the_map_at_v2() {
    let Some(gpu) = gpu() else { return };
    let (built, _) = app_scene(V2.view(), None);
    let (image, _) = map_shot_with_tiles(&gpu, &built, 1.0, Arc::new(DirTiles::fixtures()), |_| Vec::new());
    save(&image, "a4-v2-map.png");
    let aircraft = &built.aircraft.scene;
    let icons = &aircraft.icons;
    let at = on_screen(&built, icons, Sight::Clear);
    let white = components(&image, Rect::whole(&image), |p| p[0] >= 250 && p[1] >= 250 && p[2] >= 250);
    let viewport = viewport_for(&built.view, built.size);
    let at_icon = |blob: &Component| {
        icons.iter().any(|icon| {
            let [x, y, _] = viewport.project(icon.position);
            (blob.centroid[0] - x).hypot(blob.centroid[1] - y) < extent(icon)
        })
    };
    // a mesh spans `screenPx` along its largest dimension, about its position
    let at_mesh = |blob: &Component| {
        aircraft.groups.iter().flat_map(|g| &g.instances).any(|m| {
            let [x, y, _] = viewport.project(m.position);
            (blob.centroid[0] - x).hypot(blob.centroid[1] - y) < aircraft.frame.screen_px(m.size_m) * 0.6 + 1.0
        })
    };
    let on_icons = white.iter().filter(|b| at_icon(b)).count();
    eprintln!(
        "the app's V2: {} pure white blobs, {on_icons} at icons, for {} icons on screen and {} meshes",
        white.len(),
        at.len(),
        aircraft.mesh_count()
    );
    assert!(on_icons >= 3);
    for blob in &white {
        assert!(at_icon(blob) || at_mesh(blob), "pure white at {:?} with no aircraft under it", blob.centroid);
    }
}
