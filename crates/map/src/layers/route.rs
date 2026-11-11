//! The selected flight's trail with its ground shadow, and its route: the filed plan with its
//! waypoints, the estimate before the recorded track and the estimate ahead (item A8; doc 03
//! §8.1–8.2, doc 02 §10.1; rows 19–21 of doc 03 §3), as today's `buildLayers` (G:2757-2797) and
//! `routeLayers` (G:1419-1474) draw them with deck.gl 9.4's PathLayer and ScatterplotLayer.
//!
//! The scene builder hands the geometry in geographic coordinates ([`Input`]): the trail as drawn
//! (each point at its drawn height, the aircraft's drawn position last) with its per-vertex
//! colours, and the route's three paths. This layer is the deck layers:
//!
//! | Layer | Drawn when | Style | Draw state |
//! |---|---|---|---|
//! | `trail-shadow` | a trail | the trail with every height 0, [`TRAIL_SHADOW_COLOR`], 1.5 px, deck's butt caps and miter joints | `SURFACE_NO_CULL` |
//! | `trail` | a trail | the per-vertex colours, 3 px, rounded joints and caps | `NO_CULL` |
//! | `route-filed` | more than one filed point | [`ROUTE_FILED`], 1.5 px, solid, on the ground | `SURFACE_NO_CULL` |
//! | `route-waypoints` | the same | a ground-plane dot of 2.5 px per filed point in [`ROUTE_FILED`] | `SURFACE_NO_CULL` |
//! | `route-before` | more than one point | [`ROUTE_BEFORE`], 1.5 px, dashes `[3, 3]` along the whole path | `NO_CULL` |
//! | `route-ahead` | more than one point | [`ROUTE_AHEAD`], 2 px, dashes `[4, 3]` along the whole path | `NO_CULL` |
//!
//! in that order. Widths are in pixels. Dash arrays count half line widths, as deck 9.4's
//! PathStyleExtension does with its default `dashUnits: "widths"` (the shader's unit is the
//! stroke's half width): `[4, 3]` on the 2 px estimate ahead is 4 px on and 3 px off, `[3, 3]` on
//! the 1.5 px estimate before 2.25 px each (doc 03 §8.2 reads them as whole widths, 8 and 6 px:
//! not what deck draws). With `highPrecisionDash` they are measured along the whole path, not
//! restarted per segment.
//!
//! As deck does:
//! - Each path is normalised as `normalizePath` does it: nested coordinates read with `|| 0`, and
//!   on the globe (whose viewport has `resolution` 5) cut with math.gl's `cutPolylineByGrid` on the
//!   5° grid, one part with a vertex added at each grid line; on the flat map as it is. A path
//!   whose last point equals its first is closed (deck's default `_pathType`).
//! - The trail's colours are one per point of the trail as handed in, written into the normalised
//!   path's vertex slots in order (deck's attribute updater): each segment takes its start vertex's
//!   colour (deck's `instanceColors`, not interpolated along the segment). Where the globe's cut adds
//!   vertices, the colours after it shift by one per added vertex and the last segments get
//!   transparent black, as today.
//! - The meshes are made again when the input changes (each scene refresh moves the aircraft) or
//!   the projection swaps at zoom 12 (deck re-tessellates when the viewport's `resolution`
//!   changes), each in its own local Mercator frame around the path.

use glam::DVec2;
use rustradar_scene::camera::{GLOBE_RESOLUTION, lng_lat_to_world};
use rustradar_scene::geom::grid_cut::{PolylineGridOptions, cut_polyline_by_grid};
use rustradar_scene::geom::path::{Colors, LocalMercator, Paint, PathBuilder, PathType};

use super::{DrawCx, Init, Layer, Pass, PrepareCx};
use crate::frame::{FrameSlot, power_of_two_scale};
use crate::pipelines::circle::{self, Circle, CircleStyle};
use crate::pipelines::path::{Dash, Mesh, Path, PathStyle, Segment, Style};
use crate::state::DrawState;

/// `trail-shadow`'s colour (G:2779).
pub const TRAIL_SHADOW_COLOR: [u8; 4] = [241, 238, 231, 60];
/// `trail-shadow`'s width, px.
pub const TRAIL_SHADOW_WIDTH: f64 = 1.5;
/// `trail`'s width, px.
pub const TRAIL_WIDTH: f64 = 3.0;
/// `ROUTE_FILED` (G:1393): the filed plan and its waypoints.
pub const ROUTE_FILED: [u8; 4] = [241, 238, 231, 150];
/// `ROUTE_BEFORE` (G:1392): the estimate from the take-off to the recorded track.
pub const ROUTE_BEFORE: [u8; 4] = [241, 238, 231, 110];
/// `ROUTE_AHEAD` (G:1391): the estimate from the aircraft to touchdown.
pub const ROUTE_AHEAD: [u8; 4] = [55, 142, 240, 210];
/// `route-filed`'s and `route-before`'s widths, px.
pub const ROUTE_FILED_WIDTH: f64 = 1.5;
pub const ROUTE_BEFORE_WIDTH: f64 = 1.5;
/// `route-ahead`'s width, px.
pub const ROUTE_AHEAD_WIDTH: f64 = 2.0;
/// `route-before`'s dashes, in line widths.
pub const ROUTE_BEFORE_DASH: [f64; 2] = [3.0, 3.0];
/// `route-ahead`'s dashes, in line widths.
pub const ROUTE_AHEAD_DASH: [f64; 2] = [4.0, 3.0];
/// `route-waypoints`' radius, px.
pub const WAYPOINT_RADIUS: f64 = 2.5;

/// The selected flight's trail as drawn (`path` and `colors` of G:2766-2773).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trail {
    /// `[lon, lat, metres]`: the trail's points at their drawn heights, then the aircraft's drawn
    /// position.
    pub path: Vec<[f64; 3]>,
    /// One colour per point of `path` (the altitude colours, opaque).
    pub colors: Vec<[u8; 4]>,
}

/// What the scene builder hands this layer (`SceneInput`'s field of the same name). The default
/// draws nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Input {
    /// The trail and its shadow: there while a flight is selected and the aircraft are on the map.
    pub trail: Option<Trail>,
    /// The filed plan's waypoints, `[lon, lat]` (`route.filed`), drawn when there are more than one.
    pub filed: Vec<[f64; 2]>,
    /// `drawnRoute(route.before)`, `[lon, lat, metres]`, drawn when there are more than one.
    pub before: Vec<[f64; 3]>,
    /// `drawnAhead(route.ahead, pos)`, `[lon, lat, metres]`, drawn when there are more than one.
    pub ahead: Vec<[f64; 3]>,
}

/// One of the layer's paths: its mesh and the frame it is in.
struct Line {
    mesh: Mesh,
    local: LocalMercator,
}

/// The paths in draw order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    TrailShadow = 0,
    Trail = 1,
    Filed = 2,
    Before = 3,
    Ahead = 4,
}

const PATHS: usize = 5;

/// deck's `|| 0`: a NaN coordinate reads as 0.
fn or0(x: f64) -> f64 {
    if x.is_nan() { 0.0 } else { x }
}

/// `normalizePath(path, 3, resolution, false)`: the coordinates with `|| 0`, cut on the 5° grid on
/// the globe (one part with the crossings added), as they are on the flat map.
pub fn normalise(path: &[[f64; 3]], globe: bool) -> Vec<[f64; 3]> {
    let points: Vec<[f64; 3]> = path.iter().map(|p| p.map(or0)).collect();
    if !globe {
        return points;
    }
    let flat: Vec<f64> = points.iter().flatten().copied().collect();
    let options = PolylineGridOptions { size: 3, broken: false, grid_resolution: GLOBE_RESOLUTION, grid_offset: [0.0, 0.0] };
    cut_polyline_by_grid(&flat, &options).into_iter().next().map(|part| part.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()).unwrap_or_default()
}

/// The smallest frame's scale: a path of one point (or of points all in one place) still needs a
/// scale `f32` holds exactly.
const MIN_SCALE: f64 = 1.0 / 1_048_576.0;

/// `points` (`[lon, lat, metres]`) in a local Mercator frame of their own: anchored at the
/// north-west corner of their extent, a power of two across (so `u` and `v` lie in `[0, 1]`).
pub fn in_local(points: &[[f64; 3]]) -> (LocalMercator, Vec<[f64; 3]>) {
    let world: Vec<DVec2> = points.iter().map(|&[lon, lat, _]| DVec2::from(lng_lat_to_world([lon, lat]))).collect();
    let (min, max) = world.iter().fold((DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)), |(lo, hi), w| (lo.min(*w), hi.max(*w)));
    if world.is_empty() || !min.is_finite() || !max.is_finite() {
        return (LocalMercator::new(DVec2::ZERO, 1.0), Vec::new());
    }
    let scale = power_of_two_scale((max - min).max_element()).max(MIN_SCALE);
    let local = LocalMercator::new(DVec2::new(min.x, max.y), scale);
    let at = world.iter().zip(points).map(|(&w, &[_, _, h])| {
        let [u, v] = local.from_world(w);
        [u, v, h]
    });
    (local, at.collect())
}

/// The segments of `path` drawn with `paint` (whole-path dash metrics when `dashed`), in a frame of
/// their own; `None` for a path of fewer than two points.
fn segments(path: &[[f64; 3]], globe: bool, paint: &Paint<'_>, dashed: bool) -> Option<(LocalMercator, Vec<Segment>)> {
    if path.len() < 2 {
        return None;
    }
    let (local, points) = in_local(&normalise(path, globe));
    let mut builder = if dashed { PathBuilder::measuring(local) } else { PathBuilder::new(local) };
    builder.push(&points, paint);
    Some((local, builder.finish()))
}

/// The input's paths as the layer draws them, in draw order: each one's segments and frame (or
/// none when it is not drawn), for a globe or a flat map.
pub fn lines(input: &Input, globe: bool) -> [Option<(LocalMercator, Vec<Segment>)>; PATHS] {
    let mut out: [Option<(LocalMercator, Vec<_>)>; PATHS] = Default::default();
    if let Some(trail) = &input.trail {
        // the trail's shadow on the ground
        let ground: Vec<[f64; 3]> = trail.path.iter().map(|&[lon, lat, _]| [lon, lat, 0.0]).collect();
        out[Which::TrailShadow as usize] = segments(&ground, globe, &Paint::solid(TRAIL_SHADOW_COLOR, TRAIL_SHADOW_WIDTH), false);
        let paint = Paint { color: Colors::PerVertex(&trail.colors), width: TRAIL_WIDTH, dash_array: [0.0, 0.0], path_type: PathType::Auto };
        out[Which::Trail as usize] = segments(&trail.path, globe, &paint, false);
    }
    if input.filed.len() > 1 {
        let filed: Vec<[f64; 3]> = input.filed.iter().map(|&[lon, lat]| [lon, lat, 0.0]).collect();
        out[Which::Filed as usize] = segments(&filed, globe, &Paint::solid(ROUTE_FILED, ROUTE_FILED_WIDTH), false);
    }
    if input.before.len() > 1 {
        let paint = Paint::solid(ROUTE_BEFORE, ROUTE_BEFORE_WIDTH).dashed(ROUTE_BEFORE_DASH);
        out[Which::Before as usize] = segments(&input.before, globe, &paint, true);
    }
    if input.ahead.len() > 1 {
        let paint = Paint::solid(ROUTE_AHEAD, ROUTE_AHEAD_WIDTH).dashed(ROUTE_AHEAD_DASH);
        out[Which::Ahead as usize] = segments(&input.ahead, globe, &paint, true);
    }
    out
}

/// Rows 19–21: `trail-shadow`, `trail`, `route-filed`, `route-waypoints`, `route-before`,
/// `route-ahead`.
pub struct Route {
    /// `{...NO_CULL, depthWriteEnabled: false}`: the shadow and the filed plan.
    surface: Path,
    /// `NO_CULL`: the trail and the estimates.
    lines: Path,
    styles: [Style; PATHS],
    waypoints: Circle,
    waypoint_style: circle::Style,
    waypoint_instances: circle::Instances,
    paths: [Option<Line>; PATHS],
    /// The input and projection the meshes were made for.
    built_for: Option<(Input, bool)>,
    /// This frame's frames, one per drawn path.
    slots: [Option<FrameSlot>; PATHS],
}

impl Route {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let surface = Path::new(init, "route surface", DrawState::SURFACE_NO_CULL);
        let lines = Path::new(init, "route lines", DrawState::NO_CULL);
        let dashed = PathStyle { dash: Dash::Path, ..PathStyle::PIXELS };
        let styles = [
            surface.style(device, &PathStyle::PIXELS),
            lines.style(device, &PathStyle { joint_rounded: true, cap_rounded: true, ..PathStyle::PIXELS }),
            surface.style(device, &PathStyle::PIXELS),
            lines.style(device, &dashed),
            lines.style(device, &dashed),
        ];
        let waypoints = Circle::new(init, "route waypoints", DrawState::SURFACE_NO_CULL);
        let waypoint_style = waypoints.style(device, &CircleStyle { billboard: false, filled: true, stroked: false, ..CircleStyle::PIXELS });
        Route {
            surface,
            lines,
            styles,
            waypoints,
            waypoint_style,
            waypoint_instances: circle::Instances::new(device, "route waypoints"),
            paths: Default::default(),
            built_for: None,
            slots: [None; PATHS],
        }
    }

    /// Whether a path is drawn this frame, in draw order (for tests): trail shadow, trail, filed,
    /// before, ahead.
    pub fn drawn(&self) -> [bool; PATHS] {
        self.slots.map(|s| s.is_some())
    }

    fn pipeline(&self, which: usize) -> &Path {
        if which == Which::TrailShadow as usize || which == Which::Filed as usize { &self.surface } else { &self.lines }
    }
}

impl Layer for Route {
    fn name(&self) -> &'static str {
        "route"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        let input = &cx.scene.route;
        let globe = cx.view.is_globe();
        let device = &cx.gpu.device;
        let queue = &cx.gpu.queue;
        if self.built_for.as_ref().is_none_or(|(built, was_globe)| built != input || *was_globe != globe) {
            for (slot, made) in self.paths.iter_mut().zip(lines(input, globe)) {
                *slot = match (slot.take(), made) {
                    (_, None) => None,
                    (Some(mut line), Some((local, segments))) => {
                        line.mesh.write(device, queue, "route", &segments);
                        line.local = local;
                        Some(line)
                    }
                    (None, Some((local, segments))) => Some(Line { mesh: Mesh::new(device, "route", &segments), local }),
                };
            }
            self.built_for = Some((input.clone(), globe));
        }
        for (slot, line) in self.slots.iter_mut().zip(&self.paths) {
            *slot = line.as_ref().map(|l| cx.frames.push(&cx.view.local(l.local.anchor, l.local.scale, 0.0)));
        }
        // the waypoints' dots, placed for this frame's camera
        let dots: Vec<circle::Instance> = if input.filed.len() > 1 {
            input.filed.iter().map(|&[lon, lat]| circle::Instance::at(cx.view, [or0(lon), or0(lat)], 0.0).radius(WAYPOINT_RADIUS).fill(ROUTE_FILED)).collect()
        } else {
            Vec::new()
        };
        self.waypoint_instances.write(device, queue, &dots);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if cx.pass != Pass::Map {
            return;
        }
        for which in 0..PATHS {
            if which == Which::Before as usize {
                // `route-waypoints` comes between the filed plan and the estimates
                self.waypoints.draw(pass, &self.waypoint_style, &self.waypoint_instances);
            }
            let (Some(line), Some(slot)) = (&self.paths[which], self.slots[which]) else { continue };
            let path = self.pipeline(which);
            path.begin(pass);
            path.draw_mesh(pass, cx, slot, &self.styles[which], &line.mesh);
        }
    }

    fn drape(&self) -> super::Drape {
        // stage 9: deck drapes the PathLayers (their positions are vertex positions) and offsets
        // the scatterplot; the trail keeps its true altitude (plan 1.4.12)
        super::Drape::None
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::{ViewState, viewport_for};

    use super::*;
    use crate::gpu::testing::gpu;
    use crate::headless::{Image, render_with};
    use crate::renderer::{MapRenderer, SceneInput};
    use crate::headless::FORMAT;

    const LAND: [u8; 3] = [50, 50, 50];

    fn view(longitude: f64, latitude: f64, zoom: f64) -> ViewState {
        ViewState { longitude, latitude, zoom, pitch: 0.0, bearing: 0.0, position: None }
    }

    #[test]
    fn the_globe_cuts_a_path_on_the_five_degree_grid_and_the_flat_map_does_not() {
        let path = [[3.0, 50.0, 0.0], [7.0, 50.0, 1000.0], [f64::NAN, 51.0, 0.0]];
        let flat = normalise(&path, false);
        assert_eq!(flat, vec![[3.0, 50.0, 0.0], [7.0, 50.0, 1000.0], [0.0, 51.0, 0.0]], "|| 0, nothing added");
        let globe = normalise(&path, true);
        // a vertex where it crosses 5° E, its height interpolated
        assert_eq!(globe[0], [3.0, 50.0, 0.0]);
        assert_eq!(globe[1], [5.0, 50.0, 500.0]);
        assert_eq!(globe[2], [7.0, 50.0, 1000.0]);
        assert!(globe.len() > 3, "the leg back west to 0° crosses 5° E again: {globe:?}");
    }

    #[test]
    fn a_frame_of_its_own_holds_the_path_within_one_unit() {
        let (local, points) = in_local(&[[4.95, 52.32, 650.0], [4.77, 52.31, 0.0]]);
        for p in &points {
            assert!((0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1]), "{p:?}");
        }
        assert_eq!(points[0][2], 650.0);
        assert_eq!(local.scale, power_of_two_scale(local.scale), "a power of two");
        // a single place still gets a frame f32 holds
        let (one, at) = in_local(&[[4.95, 52.32, 0.0], [4.95, 52.32, 0.0]]);
        assert_eq!(one.scale, MIN_SCALE);
        assert_eq!(at[0][..2], [0.0, 0.0]);
    }

    #[test]
    fn which_paths_are_drawn_follows_routelayers() {
        let mut input = Input::default();
        assert!(lines(&input, true).iter().all(Option::is_none), "nothing selected");
        input.trail = Some(Trail { path: vec![[4.9, 52.3, 600.0], [4.8, 52.3, 0.0]], colors: vec![[255, 151, 0, 255], [255, 98, 0, 255]] });
        input.filed = vec![[4.0, 52.0]];
        input.before = vec![[11.0, 48.0, 0.0]];
        input.ahead = vec![[4.8, 52.3, 0.0], [4.76, 52.31, 0.0]];
        let drawn = lines(&input, false);
        assert_eq!(drawn.each_ref().map(Option::is_some), [true, true, false, false, true], "one point is not a path");
        let (_, shadow) = drawn[Which::TrailShadow as usize].as_ref().unwrap();
        assert!(shadow.iter().all(|s| s.start[2] == 0.0 && s.end[2] == 0.0 && s.color == TRAIL_SHADOW_COLOR));
        let (_, trail) = drawn[Which::Trail as usize].as_ref().unwrap();
        assert_eq!(trail.len(), 1);
        assert_eq!(trail[0].color, [255, 151, 0, 255], "a segment takes its start vertex's colour");
        assert_eq!(trail[0].start[2], 600.0);
        let (_, ahead) = drawn[Which::Ahead as usize].as_ref().unwrap();
        assert_eq!(ahead[0].dash_array, [4.0, 3.0]);
        assert!(ahead[0].dash_flat[1] > 0.0, "measured along the whole path");
    }

    /// The trail's colours fill the normalised path's vertex slots in order: where the globe's cut
    /// adds a vertex, the colours after it shift and the last segment is transparent black, as
    /// deck writes them.
    #[test]
    fn the_globe_cut_shifts_the_trail_colours_as_deck_does() {
        let colors = vec![[1, 0, 0, 255], [2, 0, 0, 255], [3, 0, 0, 255]];
        let input = Input { trail: Some(Trail { path: vec![[4.0, 52.0, 0.0], [6.0, 52.0, 0.0], [6.5, 52.0, 0.0]], colors }), ..Default::default() };
        let flat = lines(&input, false);
        let flat: Vec<[u8; 4]> = flat[Which::Trail as usize].as_ref().unwrap().1.iter().map(|s| s.color).collect();
        assert_eq!(flat, vec![[1, 0, 0, 255], [2, 0, 0, 255]]);
        let globe = lines(&input, true);
        let globe: Vec<[u8; 4]> = globe[Which::Trail as usize].as_ref().unwrap().1.iter().map(|s| s.color).collect();
        assert_eq!(globe, vec![[1, 0, 0, 255], [2, 0, 0, 255], [3, 0, 0, 255]], "4 → 5 → 6 → 6.5: three segments, three colours");
        let crossing = Input { trail: Some(Trail { path: vec![[4.0, 52.0, 0.0], [6.0, 52.0, 0.0]], colors: vec![[1, 0, 0, 255], [2, 0, 0, 255]] }), ..Default::default() };
        let globe = lines(&crossing, true);
        let globe: Vec<[u8; 4]> = globe[Which::Trail as usize].as_ref().unwrap().1.iter().map(|s| s.color).collect();
        assert_eq!(globe, vec![[1, 0, 0, 255], [2, 0, 0, 255]]);
    }

    /// Draws `input` alone over the dark map's land (no tiles) at `v` on a 640 × 400 canvas.
    fn render(gpu: &crate::gpu::Gpu, v: ViewState, input: Input) -> Image {
        let mut scene = SceneInput::new(v, [640.0, 400.0], 1.0, Basemap::Dark);
        scene.earth = crate::layers::earth::Input::SHOWN;
        scene.route = input;
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        render_with(gpu, &mut renderer, &scene, 1.0).expect("rendered")
    }

    /// Where `[lon, lat, h]` is on the 640 × 400 canvas at `v`, CSS px.
    fn screen(v: ViewState, [lon, lat, h]: [f64; 3]) -> [f64; 2] {
        let vp = viewport_for(&v, [640.0, 400.0]);
        let p = vp.project([lon, lat, h]);
        [p[0], p[1]]
    }

    /// How many pixels in the column at `x` between `y0` and `y1` are not the land.
    fn across(image: &Image, x: u32, y0: u32, y1: u32, pick: impl Fn([u8; 4]) -> bool) -> usize {
        (y0..y1).filter(|&y| pick(image.pixel(x, y))).count()
    }

    fn save(image: &Image, name: &str) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/shots");
        std::fs::create_dir_all(dir).expect("shot folder");
        image.save_png(&std::path::Path::new(dir).join(name)).expect("png");
    }

    /// A trail running east-west across the view at zoom 10 (globe) and 12.5 (flat): 3 px wide in
    /// its colours, its 1.5 px shadow `[241,238,231,60]` over the land, no gap at the projection
    /// swap.
    #[test]
    fn the_trail_is_three_pixels_wide_in_its_colours_over_its_shadow() {
        let Some(gpu) = gpu() else { return };
        for (zoom, name) in [(10.0, "a8-route-trail-z10.png"), (12.5, "a8-route-trail-z12.5.png")] {
            let v = view(4.86, 52.315, zoom);
            // one degree of longitude is plenty to cross the canvas at both zooms
            let trail = Trail {
                path: vec![[4.0, 52.32, 0.0], [5.0, 52.32, 0.0], [5.6, 52.32, 0.0]],
                colors: vec![[255, 151, 0, 255], [255, 98, 0, 255], [255, 98, 0, 255]],
            };
            let image = render(&gpu, v, Input { trail: Some(trail), ..Default::default() });
            save(&image, name);
            let [_, y] = screen(v, [4.5, 52.32, 0.0]);
            let (y0, y1) = ((y - 8.0) as u32, (y + 8.0) as u32);
            let [x_west, _] = screen(v, [4.5, 52.32, 0.0]);
            let [x_east, _] = screen(v, [5.2, 52.32, 0.0]);
            for (x, color) in [(x_west.clamp(20.0, 620.0), [255, 151, 0]), (x_east.clamp(20.0, 620.0), [255, 98, 0])] {
                if !(20.0..620.0).contains(&x) {
                    continue;
                }
                let x = x as u32;
                let full = across(&image, x, y0, y1, |p| (0..3).all(|c| p[c].abs_diff(color[c]) <= 2));
                let touched = across(&image, x, y0, y1, |p| p[..3] != LAND);
                assert!((2..=4).contains(&full), "zoom {zoom}: {full} px of {color:?} at x {x}");
                assert!(touched <= 6, "zoom {zoom}: {touched} px off the land at x {x}");
            }
        }
    }

    /// The shadow alone (the trail on the ground hides it, so lift the trail): 1.5 px of
    /// `[241,238,231,60]` over the land, a band of about `(95,94,94)`.
    #[test]
    fn the_trails_shadow_is_one_and_a_half_pixels_on_the_ground() {
        let Some(gpu) = gpu() else { return };
        let v = ViewState { pitch: 50.0, ..view(4.86, 52.315, 11.0) };
        // lifted 3 km: on the screen well above its shadow at pitch 50
        let trail = Trail { path: vec![[4.6, 52.30, 3000.0], [5.1, 52.30, 3000.0]], colors: vec![[255, 98, 0, 255]; 2] };
        let image = render(&gpu, v, Input { trail: Some(trail), ..Default::default() });
        save(&image, "a8-route-shadow.png");
        let [x, y] = screen(v, [4.86, 52.30, 0.0]);
        let [_, y_trail] = screen(v, [4.86, 52.30, 3000.0]);
        assert!(y - y_trail > 20.0, "the trail stands clear of its shadow: {y} {y_trail}");
        let column: Vec<[u8; 4]> = ((y - 6.0) as u32..(y + 6.0) as u32).map(|py| image.pixel(x as u32, py)).collect();
        // the shadow's coverage across the line, from its colour over the land
        let expected = 50.0 + (241.0 - 50.0) * 60.0 / 255.0;
        let coverage: f64 = column.iter().map(|p| (f64::from(p[0]) - 50.0) / (expected - 50.0)).sum();
        assert!((1.2..=1.9).contains(&coverage), "the shadow covers {coverage} px: {column:?}");
        let darkest = column.iter().map(|p| p[0]).max().unwrap();
        assert!(f64::from(darkest) <= expected + 1.0, "no pixel brighter than one shadow: {darkest}");
    }

    /// The estimate ahead in `ROUTE_AHEAD` with dashes of 4 px and gaps of 3 px (half widths of
    /// its 2 px line), the filed plan solid, its waypoints dots of 2.5 px.
    #[test]
    fn the_route_is_dashed_ahead_and_solid_where_filed() {
        let Some(gpu) = gpu() else { return };
        let v = view(4.86, 52.315, 11.0);
        let input = Input {
            filed: vec![[4.78, 52.29], [4.94, 52.29]],
            ahead: vec![[4.6, 52.34, 0.0], [4.8, 52.34, 0.0], [5.1, 52.34, 0.0]],
            ..Default::default()
        };
        let image = render(&gpu, v, input);
        save(&image, "a8-route-dashes.png");
        // the path runs past both sides of the canvas: across all of it
        let [_, y_ahead] = screen(v, [4.86, 52.34, 0.0]);
        let blue = |p: [u8; 4]| u16::from(p[2]) > u16::from(p[0]) + 60;
        let row: Vec<bool> = (0..image.width).map(|x| ((y_ahead - 2.0) as u32..(y_ahead + 3.0) as u32).any(|y| blue(image.pixel(x, y)))).collect();
        let mut runs = Vec::new();
        let mut run = (row[0], 0usize);
        for &on in &row {
            if on == run.0 {
                run.1 += 1;
            } else {
                runs.push(run);
                run = (on, 1);
            }
        }
        // the first and the last run may be cut by the canvas's edges
        let dashes: Vec<usize> = runs.iter().skip(1).filter(|r| r.0).map(|r| r.1).collect();
        let gaps: Vec<usize> = runs.iter().skip(1).filter(|r| !r.0).map(|r| r.1).collect();
        assert!(dashes.len() >= 5, "{runs:?}");
        assert!(dashes.iter().all(|&d| (3..=5).contains(&d)), "dashes of about 4 px: {dashes:?}");
        assert!(gaps.iter().all(|&g| (2..=4).contains(&g)), "gaps of about 3 px: {gaps:?}");
        let period = (dashes.iter().sum::<usize>() + gaps.iter().sum::<usize>()) as f64 / dashes.len() as f64;
        assert!((period - 7.0).abs() < 0.3, "a period of 7 px: {period}");
        // the filed plan: solid, lighter than the land
        let [x, y_filed] = screen(v, [4.86, 52.29, 0.0]);
        let filed = across(&image, x as u32, (y_filed - 4.0) as u32, (y_filed + 4.0) as u32, |p| p[0] > 70);
        assert!((1..=3).contains(&filed), "{filed}");
        // a waypoint's dot: about π · 2.5² px of the filed colour at its end
        let [wx, wy] = screen(v, [4.94, 52.29, 0.0]);
        assert!(wx > 10.0 && wx < f64::from(image.width) - 10.0, "on the canvas: {wx}");
        let mut dot = 0.0;
        for py in (wy - 5.0) as u32..(wy + 5.0) as u32 {
            for px in (wx - 5.0) as u32..(wx + 5.0) as u32 {
                let p = image.pixel(px, py);
                dot += (f64::from(p[0]) - 50.0) / (50.0 + (241.0 - 50.0) * 150.0 / 255.0 - 50.0);
            }
        }
        assert!(dot > 14.0 && dot < 30.0, "the dot and the line's end cover {dot} px");
    }
}
