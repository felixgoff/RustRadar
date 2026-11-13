//! The selected train's route and its selection halo and ring (item T6; doc 02 §5.7, §7; rows 13,
//! 14 and 17 of doc 03 §3, rows 10, 11 and 14 of doc 02 §2), as today's `trainRouteLayers`
//! (G:2078-2162) and `trainSelectionLayers` (G:3327-3354) draw them with deck.gl 9.4's PathLayer and
//! ScatterplotLayer. The trains layer ([`super::trains::Trains`]) draws them in its rows: the route,
//! then the halo, under the trains; the ring over them and the dev sightings.
//!
//! | Layer | Draws | Style |
//! |---|---|---|
//! | `train-route-dashes` | the route on from the leg the train is on (`way.rest`), one path per stretch | [`TRAIN_ROUTE_DASH`], [`ROUTE_WIDTH`] px, dashes [`TRAIN_ROUTE_DASHES`] along each whole path, not justified, butt caps, miter joints |
//! | `train-route-head-dashes` | from the train onto its route (`way.head`), when there is one | the same |
//! | `train-route-to-stop` | from the train to where its frames next have it stand (`wayToStop`), with two points or more | [`TRAIN_TO_STOP`], [`TO_STOP_WIDTH`] px, round caps and joints |
//! | `train-route-stop-spot` | that way's last point | a disc of [`STOP_SPOT_RADIUS`] px lying on the ground, white, its line [`TRAIN_TO_STOP`] [`STOP_SPOT_LINE`] px |
//! | `train-route-stops` | the route's stops where it stops (`stops: true`) | a disc of [`STOP_RADIUS`] px lying on the ground, [`TRAIN_ROUTE_CASING`], its line [`TRAIN_ROUTE_DASH`] [`STOP_LINE`] px |
//! | `train-selection-halo` | the selected train, where it is drawn | a billboard disc of the ring's radius, [`HALO_COLOR`] |
//! | `train-selection-ring` | the same | a billboard ring, [`RING_COLOR`], [`RING_WIDTH`] px |
//!
//! All in today's `ground` and the rings' parameters, `{cullMode: "none", depthWriteEnabled:
//! false, depthCompare: "always"}` ([`STATE`], [`DrawState::LABEL`]): over everything drawn before
//! them, the globe included (nothing hides the route on the globe's far side; the ring is left out
//! there by [`ring_of`], as today). Widths and radii are in pixels; the paths lie on the ground at
//! height 0 (deck's PathLayer, `billboard: false`); dash arrays count half line widths, as the path
//! pipeline reads deck's `dashUnits: "widths"`, so [`TRAIN_ROUTE_DASHES`] on the 3 px route is 3.3 px
//! on and 2.4 px off. Despite its name [`TRAIN_ROUTE_CASING`] is only the stations' fill: no
//! casing is drawn under the dashes (doc 02 §7).
//!
//! The plan's T6 row asks for "a dashed route ahead in `[55,142,240,210]`": that is the selected
//! flight's route ahead (doc 02 §1.4 lists it with the other blues); today's train route is the
//! black dashes of `TRAIN_ROUTE_DASH` (G:2078), and so is this one. The row's "stop … names" are
//! not drawn on the map today either (the stations are discs only; their names are the train
//! panel's), so there are none here.
//!
//! What the scene builder hands over ([`Input`]) it works out with [`route_lines`] (the CPU part of
//! `trainRouteLayers`, with `train_route.ts`'s `routeAhead` and `wayToStop` from
//! [`rustradar_core::train_route`]) and [`ring_of`] (`trainSelectionLayers`). As deck does:
//! - Each path is normalised as `normalizePath` does it: `[lon, lat]` read with height 0 and on the
//!   globe cut on the 5° grid ([`super::route::normalise`]).
//! - The route on from the train (`way.rest`) is the same lines while the train is on the same leg
//!   of it (core shares them through an `Arc`); deck then keeps the layer's attributes, and this
//!   layer keeps its mesh: it is made again only for new lines (`Arc::ptr_eq`) or when the
//!   projection swaps at zoom 12. The head and the way to the stop change at each scene refresh
//!   and are made again whenever they do.
//! - The circles are placed at each frame for that frame's camera.

use std::sync::Arc;

use glam::DVec2;
use rustradar_core::geo::cos_angle;
use rustradar_core::model::{Train, TrainRoute, f32_as_js};
use rustradar_core::num::js_round;
use rustradar_core::train_route::{LonLat, RouteCursor, route_ahead, way_to_stop};
use rustradar_core::trains::{Frame, train_icon_size};
use rustradar_scene::camera::{FLAT_ABOVE_ZOOM, ViewState, lng_lat_to_world};
use rustradar_scene::geom::path::{LocalMercator, Paint, PathBuilder};
use rustradar_scene::trains::Drawn;

use super::route::normalise;
use super::{DrawCx, Init, PrepareCx};
use crate::frame::{FrameSlot, power_of_two_scale};
use crate::pipelines::circle::{self, Circle, CircleStyle};
use crate::pipelines::path::{Dash, Mesh, Path, PathStyle, Segment, Style};
use crate::state::DrawState;

/// The route's dashes and the stations' line (`TRAIN_ROUTE_DASH`, G:2078).
pub const TRAIN_ROUTE_DASH: [u8; 4] = [0, 0, 0, 255];
/// The stations' fill (`TRAIN_ROUTE_CASING`, G:2079).
pub const TRAIN_ROUTE_CASING: [u8; 4] = [236, 238, 244, 215];
/// The route's dash and gap, in half line widths (`TRAIN_ROUTE_DASHES`, G:2080).
pub const TRAIN_ROUTE_DASHES: [f64; 2] = [2.2, 1.6];
/// The way to where the train stands next, and its spot's line (`TRAIN_TO_STOP`, G:2082).
pub const TRAIN_TO_STOP: [u8; 4] = [37, 99, 235, 255];
/// `train-route-dashes`' and `train-route-head-dashes`' width, px.
pub const ROUTE_WIDTH: f64 = 3.0;
/// `train-route-to-stop`'s width, px.
pub const TO_STOP_WIDTH: f64 = 4.0;
/// `train-route-stop-spot`: radius, fill and line width, px.
pub const STOP_SPOT_RADIUS: f64 = 5.0;
pub const STOP_SPOT_FILL: [u8; 4] = [255, 255, 255, 255];
pub const STOP_SPOT_LINE: f64 = 2.5;
/// `train-route-stops`: radius and line width, px.
pub const STOP_RADIUS: f64 = 4.5;
pub const STOP_LINE: f64 = 2.0;
/// `train-selection-halo`'s fill (G:3343).
pub const HALO_COLOR: [u8; 4] = [55, 142, 240, 56];
/// `train-selection-ring`'s line (G:3348) and its width, px.
pub const RING_COLOR: [u8; 4] = [55, 142, 240, 235];
pub const RING_WIDTH: f64 = 2.0;
/// Px the ring stands clear of the icon's half size.
pub const RING_CLEARANCE_PX: f64 = 5.0;

/// `ground` (G:2104) and the rings' parameters (G:3339): no cull, no depth write, depth always.
pub const STATE: DrawState = DrawState::LABEL;
/// The dashes' props: widths in pixels, dashes along each whole path (`highPrecisionDash`), not
/// justified; deck's butt caps and miter joints.
pub const DASH_STYLE: PathStyle = PathStyle { dash: Dash::Path, dash_justified: false, ..PathStyle::PIXELS };
/// `train-route-to-stop`'s props.
pub const TO_STOP_STYLE: PathStyle = PathStyle { cap_rounded: true, joint_rounded: true, ..PathStyle::PIXELS };
/// The stop spot's and the stations' props: lying on the ground (deck's default), filled and
/// stroked.
pub const SPOT_STYLE: CircleStyle = CircleStyle { billboard: false, filled: true, stroked: true, ..CircleStyle::PIXELS };
/// `train-selection-halo`'s props.
pub const HALO_STYLE: CircleStyle = CircleStyle { billboard: true, filled: true, stroked: false, ..CircleStyle::PIXELS };
/// `train-selection-ring`'s props.
pub const RING_STYLE: CircleStyle = CircleStyle { billboard: true, filled: false, stroked: true, ..CircleStyle::PIXELS };

/// The selected train's halo and ring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ring {
    /// Where the train is drawn, `[lon, lat]` (at height 0).
    pub at: LonLat,
    /// `round(trainIconSize(kind, heading, zoom) / 2 + 5)`, px.
    pub radius_px: f64,
}

/// What the scene builder hands this layer (`trains::Input::route`). The default draws nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Input {
    /// `train-route-dashes`: the route on from the leg the train is on (`way.rest`), each stretch a
    /// path; the same `Arc` while the lines are the same.
    pub rest: Arc<Vec<Vec<LonLat>>>,
    /// `train-route-head-dashes`: from where the train is drawn onto its route (`way.head`); none
    /// when empty.
    pub head: Vec<LonLat>,
    /// `train-route-to-stop` and, at its last point, `train-route-stop-spot`: drawn with two points
    /// or more.
    pub to_stop: Vec<LonLat>,
    /// `train-route-stops`: the route's stops where the train stops.
    pub stations: Vec<LonLat>,
    /// `train-selection-halo` and `train-selection-ring`: while a train is selected and not on the
    /// far side of the globe.
    pub ring: Option<Ring>,
}

/// What [`route_lines`] works out besides the layer's input: where along the route the train was
/// found (the next call's cursor, `routeCursor`) and whether it is on its route (`way.on`: when
/// not, the page asks for the route again, `ontrainrouteoff`).
#[derive(Debug, Clone, PartialEq)]
pub struct Routed {
    pub cursor: RouteCursor,
    pub on: bool,
}

/// The CPU part of `trainRouteLayers(route, train)` at `now_ms` (local whole Unix ms): `route` is
/// the selected train's, numbered `generation` (a new number for each route received, the
/// cursor's `route === r`); `drawn` is where the train is drawn (`trainData.positions[i]`, else
/// its report) and `frames` the frames it is drawn from (the drawn list's row, else the train's);
/// `cursor` the last call's. The layer's input without the ring, and the cursor and whether the
/// train is on its route.
pub fn route_lines(route: &TrainRoute, generation: u64, drawn: LonLat, frames: &[Frame], cursor: Option<&RouteCursor>, now_ms: i64) -> (Input, Routed) {
    // where it is drawn going next, along the track
    let ahead: Vec<LonLat> = frames.iter().filter(|f| f.2 > now_ms).map(|f| [f.1, f.0]).collect();
    let way = route_ahead(route, generation, drawn, cursor, &ahead);
    let stations = route.stops.iter().filter(|s| s.stops).map(|s| [s.lon, s.lat]).collect();
    // where the drawn train is going to stop next: the way there, and the spot
    let to_stop = way_to_stop(drawn, frames, now_ms);
    let input = Input { rest: way.rest, head: way.head, to_stop, stations, ring: None };
    (input, Routed { cursor: way.cursor, on: way.on })
}

/// `trainSelectionLayers(train, zoom, cLon, cLat)`: the halo and ring round `train` where `drawn`
/// has it (its drawn position and heading; its report when `drawn` lacks it), sized for its icon
/// at the view's zoom; none on the globe while it is round the back (`cosAngle` to the view centre
/// `<= 0`).
pub fn ring_of(train: &Train, drawn: &Drawn, view: &ViewState) -> Option<Ring> {
    let i = drawn.index_of(&train.id).filter(|&i| i < drawn.len());
    let at = match i {
        Some(i) => [drawn.positions[i][0], drawn.positions[i][1]],
        None => [train.lon, train.lat],
    };
    if view.zoom <= FLAT_ABOVE_ZOOM && cos_angle(at[0], at[1], view.longitude, view.latitude) <= 0.0 {
        return None;
    }
    let heading = match i {
        Some(i) => drawn.headings[i],
        None => train.heading.map(f32_as_js),
    };
    let radius_px = js_round(train_icon_size(train.kind, heading, view.zoom) / 2.0 + RING_CLEARANCE_PX);
    Some(Ring { at, radius_px })
}

/// The smallest frame's scale (a path of one place still needs a scale `f32` holds exactly).
const MIN_SCALE: f64 = 1.0 / 1_048_576.0;

/// `paths` (`[lon, lat]`) normalised for the globe or the flat map, in one local Mercator frame
/// round them all (anchored at the north-west corner of their extent, a power of two across).
fn in_frame(paths: &[&[LonLat]], globe: bool) -> (LocalMercator, Vec<Vec<[f64; 3]>>) {
    let normalised: Vec<Vec<[f64; 3]>> = paths
        .iter()
        .map(|path| {
            let points: Vec<[f64; 3]> = path.iter().map(|&[lon, lat]| [lon, lat, 0.0]).collect();
            normalise(&points, globe)
        })
        .collect();
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for p in normalised.iter().flatten() {
        let w = DVec2::from(lng_lat_to_world([p[0], p[1]]));
        if w.is_finite() {
            min = min.min(w);
            max = max.max(w);
        }
    }
    let local = if min.is_finite() && max.is_finite() {
        LocalMercator::new(DVec2::new(min.x, max.y), power_of_two_scale((max - min).max_element()).max(MIN_SCALE))
    } else {
        LocalMercator::new(DVec2::ZERO, 1.0)
    };
    let points = normalised
        .iter()
        .map(|path| {
            path.iter()
                .map(|&[lon, lat, h]| {
                    let [u, v] = local.from_world(DVec2::from(lng_lat_to_world([lon, lat])));
                    [u, v, h]
                })
                .collect()
        })
        .collect();
    (local, points)
}

/// The segments of `paths`, one deck row each, drawn with `paint` (whole-path dash metrics when
/// `dashed`), in one frame; none when no path has two points.
pub fn segments(paths: &[&[LonLat]], globe: bool, paint: &Paint<'_>, dashed: bool) -> Option<(LocalMercator, Vec<Segment>)> {
    let drawn: Vec<&[LonLat]> = paths.iter().copied().filter(|p| p.len() >= 2).collect();
    if drawn.is_empty() {
        return None;
    }
    let (local, points) = in_frame(&drawn, globe);
    let mut builder = if dashed { PathBuilder::measuring(local) } else { PathBuilder::new(local) };
    for path in &points {
        builder.push(path, paint);
    }
    let segments = builder.finish();
    (!segments.is_empty()).then_some((local, segments))
}

/// The dashes' paint.
fn dash_paint() -> Paint<'static> {
    Paint::solid(TRAIN_ROUTE_DASH, ROUTE_WIDTH).dashed(TRAIN_ROUTE_DASHES)
}

/// The three paths' segments, in draw order (`train-route-dashes`, `train-route-head-dashes`,
/// `train-route-to-stop`), for a globe or a flat map.
pub fn lines(input: &Input, globe: bool) -> [Option<(LocalMercator, Vec<Segment>)>; 3] {
    let rest: Vec<&[LonLat]> = input.rest.iter().map(Vec::as_slice).collect();
    let to_stop: Vec<&[LonLat]> = if input.to_stop.len() >= 2 { vec![input.to_stop.as_slice()] } else { Vec::new() };
    [
        segments(&rest, globe, &dash_paint(), true),
        segments(&[input.head.as_slice()], globe, &dash_paint(), true),
        segments(&to_stop, globe, &Paint::solid(TRAIN_TO_STOP, TO_STOP_WIDTH), false),
    ]
}

/// One drawn path: its mesh and frame.
struct Line {
    mesh: Mesh,
    local: LocalMercator,
}

impl Line {
    /// `made` in `slot`'s mesh (written again in place when there is one), or none.
    fn update(slot: Option<Line>, made: Option<(LocalMercator, Vec<Segment>)>, cx: &PrepareCx<'_>, label: &str) -> Option<Line> {
        let (device, queue) = (&cx.gpu.device, &cx.gpu.queue);
        match (slot, made) {
            (_, None) => None,
            (Some(mut line), Some((local, segments))) => {
                line.mesh.write(device, queue, label, &segments);
                line.local = local;
                Some(line)
            }
            (None, Some((local, segments))) => Some(Line { mesh: Mesh::new(device, label, &segments), local }),
        }
    }
}

/// The paths in draw order.
const REST: usize = 0;
const HEAD: usize = 1;
const TO_STOP: usize = 2;

/// Rows 13, 14 and 17 of the trains layer: the route's five layers, the halo, the ring.
pub struct SelectedTrain {
    paths: Path,
    dash_style: Style,
    to_stop_style: Style,
    lines: [Option<Line>; 3],
    /// The lines and projection the rest's mesh was made for.
    rest_for: Option<(Arc<Vec<Vec<LonLat>>>, bool)>,
    /// The head and the way to the stop, and the projection, their meshes were made for.
    head_for: Option<(Vec<LonLat>, Vec<LonLat>, bool)>,
    /// This frame's frames, one per drawn path.
    slots: [Option<FrameSlot>; 3],
    circles: Circle,
    spot_style: circle::Style,
    halo_style: circle::Style,
    ring_style: circle::Style,
    spot: circle::Instances,
    stations: circle::Instances,
    halo: circle::Instances,
    ring: circle::Instances,
}

impl SelectedTrain {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let paths = Path::new(init, "train route", STATE);
        let dash_style = paths.style(device, &DASH_STYLE);
        let to_stop_style = paths.style(device, &TO_STOP_STYLE);
        let circles = Circle::new(init, "train route circles", STATE);
        SelectedTrain {
            dash_style,
            to_stop_style,
            paths,
            lines: Default::default(),
            rest_for: None,
            head_for: None,
            slots: [None; 3],
            spot_style: circles.style(device, &SPOT_STYLE),
            halo_style: circles.style(device, &HALO_STYLE),
            ring_style: circles.style(device, &RING_STYLE),
            circles,
            spot: circle::Instances::new(device, "train-route-stop-spot"),
            stations: circle::Instances::new(device, "train-route-stops"),
            halo: circle::Instances::new(device, "train-selection-halo"),
            ring: circle::Instances::new(device, "train-selection-ring"),
        }
    }

    /// What each layer draws this frame (for tests): the segments of the dashes, the head's dashes
    /// and the way to the stop, then the stop spots, stations, halos and rings.
    pub fn counts(&self) -> [u32; 7] {
        let segments = |which: usize| if self.slots[which].is_some() { self.lines[which].as_ref().map_or(0, |l| l.mesh.len()) } else { 0 };
        [segments(REST), segments(HEAD), segments(TO_STOP), self.spot.count(), self.stations.count(), self.halo.count(), self.ring.count()]
    }

    /// One frame of `input`: the meshes made again when their lines or the projection changed,
    /// the circles placed for this frame's camera.
    pub fn update(&mut self, cx: &mut PrepareCx<'_>, input: &Input) {
        let globe = cx.view.is_globe();
        if self.rest_for.as_ref().is_none_or(|(rest, was)| !Arc::ptr_eq(rest, &input.rest) || *was != globe) {
            let [rest, _, _] = lines(&Input { rest: Arc::clone(&input.rest), ..Input::default() }, globe);
            self.lines[REST] = Line::update(self.lines[REST].take(), rest, cx, "train-route-dashes");
            self.rest_for = Some((Arc::clone(&input.rest), globe));
        }
        if self.head_for.as_ref().is_none_or(|(head, to_stop, was)| *head != input.head || *to_stop != input.to_stop || *was != globe) {
            let only = Input { head: input.head.clone(), to_stop: input.to_stop.clone(), ..Input::default() };
            let [_, head, to_stop] = lines(&only, globe);
            self.lines[HEAD] = Line::update(self.lines[HEAD].take(), head, cx, "train-route-head-dashes");
            self.lines[TO_STOP] = Line::update(self.lines[TO_STOP].take(), to_stop, cx, "train-route-to-stop");
            self.head_for = Some((input.head.clone(), input.to_stop.clone(), globe));
        }
        for (slot, line) in self.slots.iter_mut().zip(&self.lines) {
            *slot = line.as_ref().map(|l| cx.frames.push(&cx.view.local(l.local.anchor, l.local.scale, 0.0)));
        }

        let (device, queue) = (&cx.gpu.device, &cx.gpu.queue);
        let spot: Vec<circle::Instance> = match input.to_stop.last() {
            Some(&at) if input.to_stop.len() >= 2 => {
                vec![circle::Instance::at(cx.view, at, cx.scene.stands_on.at(at)).radius(STOP_SPOT_RADIUS).fill(STOP_SPOT_FILL).line(TRAIN_TO_STOP, STOP_SPOT_LINE)]
            }
            _ => Vec::new(),
        };
        self.spot.write(device, queue, &spot);
        let stations: Vec<circle::Instance> = input
            .stations
            .iter()
            .map(|&at| circle::Instance::at(cx.view, at, cx.scene.stands_on.at(at)).radius(STOP_RADIUS).fill(TRAIN_ROUTE_CASING).line(TRAIN_ROUTE_DASH, STOP_LINE))
            .collect();
        self.stations.write(device, queue, &stations);
        // (the stops and the ring on the 3D terrain, raised by the ground under them: Z3)
        let at = |ring: &Ring| circle::Instance::at(cx.view, ring.at, cx.scene.stands_on.at(ring.at)).radius(ring.radius_px);
        let halo: Vec<circle::Instance> = input.ring.iter().map(|r| at(r).fill(HALO_COLOR)).collect();
        self.halo.write(device, queue, &halo);
        let ring: Vec<circle::Instance> = input.ring.iter().map(|r| at(r).line(RING_COLOR, RING_WIDTH)).collect();
        self.ring.write(device, queue, &ring);
    }

    /// The scene's route (`trains::Input::route`), for this frame.
    pub fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        let scene = cx.scene;
        self.update(cx, &scene.trains.route);
    }

    /// Row 13: `train-route-dashes`, `train-route-head-dashes`, `train-route-to-stop`,
    /// `train-route-stop-spot`, `train-route-stops`.
    pub fn draw_route(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        for (which, style) in [(REST, &self.dash_style), (HEAD, &self.dash_style), (TO_STOP, &self.to_stop_style)] {
            let (Some(line), Some(slot)) = (&self.lines[which], self.slots[which]) else { continue };
            self.paths.begin(pass);
            self.paths.draw_mesh(pass, cx, slot, style, &line.mesh);
        }
        self.circles.draw(pass, &self.spot_style, &self.spot);
        self.circles.draw(pass, &self.spot_style, &self.stations);
    }

    /// Row 14: `train-selection-halo`, under the trains.
    pub fn draw_halo(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.circles.draw(pass, &self.halo_style, &self.halo);
    }

    /// Row 17: `train-selection-ring`, over the trains and the dev sightings.
    pub fn draw_ring(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.circles.draw(pass, &self.ring_style, &self.ring);
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::Basemap;
    use rustradar_core::model::{RouteStop, TrainKind, TrainSource};
    use rustradar_scene::camera::viewport_for;
    use rustradar_scene::trains::TrainBuilder;

    use super::*;
    use crate::gpu::Gpu;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 10:00 UTC.
    const NOW: i64 = 1_782_036_000_000;
    const SIZE: [f64; 2] = [400.0, 200.0];
    const SCALE: f64 = 2.0;
    /// Stockholm at zoom 14, straight down: the flat map.
    const FLAT: ViewState = ViewState { longitude: 18.07, latitude: 59.33, zoom: 14.0, bearing: 0.0, pitch: 0.0, position: None };
    /// The same at zoom 11: the globe, train icons at their full size.
    const GLOBE: ViewState = ViewState { zoom: 11.0, ..FLAT };

    /// `[lon, lat]` at `m` metres east and `n` north of 59.33° N, 18.07° E (flat metres, as
    /// `train-route.ts` measures).
    fn at(m: f64, n: f64) -> LonLat {
        [18.07 + m / (111_320.0 * (59.33f64).to_radians().cos()), 59.33 + n / 111_320.0]
    }

    /// `[lon, lat]` at `xy` on the canvas of `view`.
    fn ground(view: &ViewState, xy: [f64; 2]) -> LonLat {
        viewport_for(view, SIZE).unproject(xy, 0.0)
    }

    fn train(id: &str, kind: TrainKind, heading: Option<f32>, [lon, lat]: LonLat) -> Train {
        Train {
            id: id.into(),
            source: TrainSource::Trafikverket,
            name: format!("Train {id}"),
            kind,
            lat,
            lon,
            heading,
            speed_kmh: None,
            destination: None,
            operator: None,
            delay_minutes: None,
            position_unix: NOW / 1000,
            frames: Vec::new(),
            reports: Vec::new(),
            predicted: Vec::new(),
            cars: None,
            tail: Vec::new(),
            structures: Vec::new(),
            ended_unix: None,
            shape: None,
            model: None,
        }
    }

    fn stop(name: &str, [lon, lat]: LonLat, stops: bool) -> RouteStop {
        RouteStop { name: name.into(), lat, lon, track: None, stops }
    }

    /// A route east along 59.33° N from the train's place, a point every 100 m for 3 km, then a gap
    /// and a stretch on; three stops, the middle one passed through.
    fn route() -> TrainRoute {
        TrainRoute {
            segments: vec![(0..=30).map(|k| at(f64::from(k) * 100.0, 0.0)).collect(), (35..=40).map(|k| at(f64::from(k) * 100.0, 0.0)).collect()],
            stops: vec![stop("Ost", at(1000.0, 0.0), true), stop("Förbi", at(2000.0, 0.0), false), stop("Slut", at(4000.0, 0.0), true)],
            complete: true,
        }
    }

    /// Frames east at 20 m/s from 450 m, standing at 1000 m from 30 s on.
    fn frames() -> Vec<Frame> {
        let f = |m: f64, s: i64| {
            let [lon, lat] = at(m, 0.0);
            (lat, lon, NOW + s * 1000)
        };
        vec![f(450.0, 0), f(650.0, 10), f(850.0, 20), f(1000.0, 30), f(1000.0, 90), f(1200.0, 100)]
    }

    #[test]
    fn route_lines_draw_from_the_train_on_along_its_route_and_to_where_it_stands_next() {
        let r = route();
        let drawn = at(450.0, 3.0);
        let (input, routed) = route_lines(&r, 1, drawn, &frames(), None, NOW + 5_000);
        assert!(routed.on);
        assert_eq!(routed.cursor.route, 1);
        // from the train onto the route, then to the end of the leg it is on
        assert_eq!(input.head.len(), 3);
        assert_eq!(input.head[0], drawn);
        assert!((input.head[1][0] - drawn[0]).abs() < 1e-9 && (input.head[1][1] - 59.33).abs() < 1e-9, "{:?}", input.head[1]);
        assert_eq!(input.head[2], at(500.0, 0.0));
        // the rest: the first stretch from there, then the next
        assert_eq!(input.rest.len(), 2);
        assert_eq!(input.rest[0][0], at(500.0, 0.0));
        assert_eq!(input.rest[0].len(), 26);
        assert_eq!(input.rest[1].len(), 6);
        // the stops it stops at, not the one it passes
        assert_eq!(input.stations, vec![at(1000.0, 0.0), at(4000.0, 0.0)]);
        // the way to where its frames stand: the frames after now, up to the standing one
        assert_eq!(input.to_stop, vec![drawn, at(650.0, 0.0), at(850.0, 0.0), at(1000.0, 0.0)]);
        assert!(input.ring.is_none());
        // on along the same leg: the same lines, from the cursor
        let (again, _) = route_lines(&r, 1, at(480.0, 0.0), &frames(), Some(&routed.cursor), NOW + 6_500);
        assert!(Arc::ptr_eq(&again.rest, &input.rest));
        // standing there now: no way to a stop; and off the route: nothing drawn, asked again
        let (standing, _) = route_lines(&r, 1, at(1000.0, 0.0), &frames(), None, NOW + 40_000);
        assert!(standing.to_stop.is_empty());
        let (off, routed) = route_lines(&r, 1, at(450.0, 800.0), &frames(), None, NOW + 5_000);
        assert!(!routed.on);
        assert!(off.head.is_empty() && off.rest.is_empty());
        assert_eq!(off.stations.len(), 2, "the stations stay");
    }

    #[test]
    fn the_ring_is_round_the_train_where_it_is_drawn_sized_for_its_icon() {
        let mut moving = train("t", TrainKind::HighSpeed, Some(90.0), at(0.0, 0.0));
        let [lon, lat] = at(200.0, 0.0);
        moving.frames = vec![(moving.lat, moving.lon, NOW), (lat, lon, NOW + 10_000)];
        let list = [&moving];
        let mut builder = TrainBuilder::new();
        builder.build(&list, true, &GLOBE, None, NOW + 5_000);
        let ring = ring_of(&moving, builder.drawn(), &GLOBE).expect("on the near side");
        // half way along its frames: 100 m east
        let drawn = builder.drawn().positions[0];
        assert_eq!(ring.at, [drawn[0], drawn[1]]);
        assert!((ring.at[0] - at(100.0, 0.0)[0]).abs() < 1e-9);
        // a high-speed train's 24 px at zoom 11: 12 + 5
        assert_eq!(ring.radius_px, 17.0);
        // not drawn: at its report, its own heading; without one the square's 0.8 (suburban
        // 20 · 0.92 at zoom 8 · 0.8 = 14.72 px: round(12.36) = 12)
        let mut square = train("s", TrainKind::Suburban, None, at(0.0, 50.0));
        let far = ViewState { zoom: 8.0, ..GLOBE };
        assert_eq!(ring_of(&square, &Drawn::default(), &far), Some(Ring { at: at(0.0, 50.0), radius_px: 12.0 }));
        // regional at zoom 5: 21 · 0.82 / 2 + 5 = 13.61, rounded up
        square.kind = TrainKind::Regional;
        square.heading = Some(10.0);
        assert_eq!(ring_of(&square, &Drawn::default(), &ViewState { zoom: 5.0, ..GLOBE }).map(|r| r.radius_px), Some(14.0));
        // round the back of the globe: none; on the flat map nothing is round the back
        let antipode = ViewState { longitude: -161.93, latitude: -59.33, zoom: 3.0, ..GLOBE };
        assert!(ring_of(&square, &Drawn::default(), &antipode).is_none());
        assert!(ring_of(&square, &Drawn::default(), &FLAT).is_some());
    }

    #[test]
    fn the_lines_are_deck_paths_of_their_widths_colours_and_dashes() {
        let r = route();
        let (input, _) = route_lines(&r, 1, at(450.0, 3.0), &frames(), None, NOW + 5_000);
        let [rest, head, to_stop] = lines(&input, false);
        let (_, rest) = rest.expect("the rest");
        // one segment per leg: 25 + 5
        assert_eq!(rest.len(), 30);
        for s in &rest {
            assert_eq!((s.color, s.width, s.dash_array), (TRAIN_ROUTE_DASH, 3.0, [2.2, 1.6]));
        }
        // dashes measured along each stretch on its own: the second starts again at 0
        assert_eq!(rest[0].dash_flat[0], 0.0);
        assert_eq!(rest[25].dash_flat[0], 0.0);
        assert!(rest[24].dash_flat[0] > 0.0);
        let (_, head) = head.expect("the head");
        assert_eq!(head.len(), 2);
        let (_, to_stop) = to_stop.expect("the way to the stop");
        assert_eq!(to_stop.len(), 3);
        assert!(to_stop.iter().all(|s| s.color == TRAIN_TO_STOP && s.width == 4.0));
        // a way of one point, no head: nothing
        let none = Input { to_stop: vec![at(0.0, 0.0)], ..Input::default() };
        assert!(lines(&none, false).iter().all(Option::is_none));
        // on the globe a path is cut on the 5° grid: one across 20° E gets a vertex there
        let across = Input { head: vec![[19.9, 59.33], [20.1, 59.33]], ..Input::default() };
        assert_eq!(lines(&across, false)[1].as_ref().unwrap().1.len(), 1);
        assert_eq!(lines(&across, true)[1].as_ref().unwrap().1.len(), 2);
    }

    fn shoot(gpu: &Gpu, view: ViewState, input: Input, trains: &[Train]) -> Image {
        let mut scene = SceneInput::new(view, SIZE, SCALE, Basemap::Dark);
        scene.earth = crate::layers::earth::Input::SHOWN;
        scene.local_now_ms = NOW as f64;
        scene.feed_now_ms = NOW as f64;
        let list: Vec<&Train> = trains.iter().collect();
        scene.trains.scene = TrainBuilder::new().build(&list, true, &view, None, NOW);
        scene.trains.route = input;
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        render_with(gpu, &mut renderer, &scene, SCALE).expect("rendered")
    }

    fn save(image: &Image, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }

    /// `top` at its alpha over `under`.
    fn over(top: [u8; 4], under: [u8; 4]) -> [u8; 4] {
        let a = f64::from(top[3]) / 255.0;
        let c = |i: usize| (f64::from(top[i]) * a + f64::from(under[i]) * (1.0 - a)).round() as u8;
        [c(0), c(1), c(2), 255]
    }

    fn near(p: [u8; 4], want: [u8; 4], tolerance: u8) -> bool {
        (0..3).all(|c| p[c].abs_diff(want[c]) <= tolerance)
    }

    /// The device pixel at CSS `[x, y]`.
    fn pixel(image: &Image, [x, y]: [f64; 2]) -> [u8; 4] {
        image.pixel((x * SCALE) as u32, (y * SCALE) as u32)
    }

    /// How many device pixels of the column at CSS `x`, from CSS `y0` to `y1`, `pick` takes.
    fn down(image: &Image, x: f64, [y0, y1]: [f64; 2], pick: impl Fn([u8; 4]) -> bool) -> usize {
        ((y0 * SCALE) as u32..(y1 * SCALE) as u32).filter(|&y| pick(image.pixel((x * SCALE) as u32, y))).count()
    }

    /// The route drawn on the flat map: the head and the rest 3 px black dashes of 3.3 px with
    /// 2.4 px gaps, the way to the stop 4 px blue into a white spot ringed blue, the stations
    /// light discs ringed black.
    #[test]
    fn dashes_the_way_to_the_stop_and_the_stations() {
        let Some(gpu) = gpu() else { return };
        let v = FLAT;
        let p = |xy: [f64; 2]| ground(&v, xy);
        let input = Input {
            rest: Arc::new(vec![vec![p([200.0, 60.0]), p([390.0, 60.0])]]),
            head: vec![p([20.0, 60.0]), p([110.0, 60.0]), p([200.0, 60.0])],
            to_stop: vec![p([40.0, 140.0]), p([150.0, 140.0]), p([260.0, 140.0])],
            stations: vec![p([330.0, 140.0])],
            ring: None,
        };
        let image = shoot(&gpu, v, input, &[]);
        save(&image, "t6-train-route-flat.png");
        let land = pixel(&image, [200.0, 100.0]);
        let black = |c: [u8; 4]| near(c, [0, 0, 0, 255], 12);
        // across a dash: 3 px of black (6 device px), nothing else changed
        let mut widths = Vec::new();
        for k in 0..60 {
            let x = 30.0 + f64::from(k) * 0.5;
            widths.push(down(&image, x, [50.0, 70.0], black));
            let other = down(&image, x, [50.0, 70.0], |c| !black(c) && !near(c, land, 3) && !near(c, [0, 0, 0, 255], 40));
            assert!(other <= 2, "{other} px neither the dash nor the land at {x}");
        }
        let widest = *widths.iter().max().unwrap();
        assert!((5..=7).contains(&widest), "{widths:?}");
        // along the dashes: on 3.3 px, off 2.4, so 58 % of the row is black, in a period of 5.7 px
        let row: Vec<bool> = ((30.0 * SCALE) as u32..(380.0 * SCALE) as u32).map(|x| black(image.pixel(x, (60.0 * SCALE) as u32))).collect();
        let on = row.iter().filter(|&&b| b).count() as f64 / row.len() as f64;
        assert!((on - 3.3 / 5.7).abs() < 0.06, "{on}");
        let starts = row.windows(2).filter(|w| !w[0] && w[1]).count() as f64;
        let periods = row.len() as f64 / (5.7 * SCALE);
        assert!((starts - periods).abs() <= 3.0, "{starts} dashes in {periods} periods");
        // the way to the stop: 4 px of TRAIN_TO_STOP
        let blue = |c: [u8; 4]| near(c, TRAIN_TO_STOP, 8);
        assert_eq!(down(&image, 100.0, [130.0, 150.0], blue), 8);
        // the spot at its end: white within, the blue line from 3.75 to 6.25 px out
        assert!(near(pixel(&image, [260.0, 140.0]), STOP_SPOT_FILL, 6), "{:?}", pixel(&image, [260.0, 140.0]));
        assert!(near(pixel(&image, [260.0, 145.0]), TRAIN_TO_STOP, 12), "{:?}", pixel(&image, [260.0, 145.0]));
        assert!(near(pixel(&image, [260.0, 148.0]), land, 3), "{:?}", pixel(&image, [260.0, 148.0]));
        // a station: the casing's light grey over the land within 3.5 px, black from 3.5 to 5.5
        let fill = over(TRAIN_ROUTE_CASING, land);
        assert!(near(pixel(&image, [330.0, 140.0]), fill, 6), "{:?} vs {fill:?}", pixel(&image, [330.0, 140.0]));
        assert!(black(pixel(&image, [330.0, 144.5])), "{:?}", pixel(&image, [330.0, 144.5]));
        assert!(near(pixel(&image, [330.0, 147.0]), land, 3));
    }

    /// The halo under the train and the ring over it: the train's own colour over the halo, the
    /// ring's blue over the train where they cross.
    #[test]
    fn the_halo_lies_under_the_train_and_the_ring_over_it() {
        let Some(gpu) = gpu() else { return };
        let v = GLOBE;
        let centre = ground(&v, [200.0, 100.0]);
        let sj = Train { operator: Some("SJ".into()), ..train("sj", TrainKind::HighSpeed, Some(0.0), centre) };
        let mut drawn = TrainBuilder::new();
        drawn.build(&[&sj], true, &v, None, NOW);
        let ring = ring_of(&sj, drawn.drawn(), &v).expect("a ring");
        assert_eq!(ring.radius_px, 17.0);
        let image = shoot(&gpu, v, Input { ring: Some(ring), ..Input::default() }, std::slice::from_ref(&sj));
        save(&image, "t6-train-ring.png");
        let land = pixel(&image, [20.0, 20.0]);
        let halo = over(HALO_COLOR, land);
        // between the carriage and the ring: the halo over the land
        assert!(near(pixel(&image, [200.0 + 10.0, 100.0]), halo, 3), "{:?} vs {halo:?}", pixel(&image, [210.0, 100.0]));
        // the ring: 2 px of blue at 17 px, over the halo
        let ring_colour = over(RING_COLOR, halo);
        assert!(near(pixel(&image, [200.0 + 17.0, 100.0]), ring_colour, 10), "{:?} vs {ring_colour:?}", pixel(&image, [217.0, 100.0]));
        // (2 px across: device pixels more than half covered, the halo's blue 92 and the ring's 228)
        let across = ((200.0 + 14.0) as u32 * 2..(200.0 + 20.0) as u32 * 2).filter(|&x| image.pixel(x, 200)[2] >= 160).count();
        assert!((3..=5).contains(&across), "{across}");
        // outside: the land
        assert!(near(pixel(&image, [200.0 + 22.0, 100.0]), land, 2));
        // the carriage (SJ's silver, 240/255 over the halo-tinted land) at its middle
        let silver = over([232, 232, 236, 240], halo);
        assert!(near(pixel(&image, [200.0, 100.0]), silver, 8), "{:?} vs {silver:?}", pixel(&image, [200.0, 100.0]));
        // a ring small enough to cross the carriage is drawn over it
        let small = Ring { radius_px: 4.0, ..ring };
        let image = shoot(&gpu, v, Input { ring: Some(small), ..Input::default() }, std::slice::from_ref(&sj));
        let on_train = pixel(&image, [204.0, 100.0]);
        assert!(on_train[2] > on_train[0] + 60, "the ring's blue over the silver carriage: {on_train:?}");
    }
}
