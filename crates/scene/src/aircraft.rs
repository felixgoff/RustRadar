//! The aircraft's part of each scene refresh (item A3; doc 03 §1–2, §5.1–5.3, §6; doc 01 §9.1):
//! the preamble of `buildLayers()` and its per-flight loop (G:2520-2636), with the CPU parts of the
//! mesh, gear and icon layers that follow it (G:2870-3025).
//!
//! [`AircraftBuilder::build`] takes the view, the settings, the published models (if they loaded)
//! and the flights shown, poses each flight with the motion model (a closure: the caller's
//! `motion.pose(f, now)`), and returns an [`AircraftScene`]: plain data the aircraft layers draw,
//! in the order today's app pushes them:
//!
//! - [`AircraftScene::frame`]: the preamble ([`Frame`]): `flat`, the altitude exaggeration, metres
//!   per pixel, the size floor, model depth, the cull radius, how much livery shows, icons or
//!   meshes, the level of detail.
//! - [`AircraftScene::groups`]: the mesh groups, one per published model (`t-<name>`) or built-in
//!   kind (`k-<kind>`), in order of first appearance in the feed (the TypeScript's `Map`). Each
//!   holds its [`MeshInstance`]s (raised onto their wheels, oriented, scaled, coloured as a whole
//!   and per livery part), the hovered instance, and the [`GearInstance`]s.
//! - [`AircraftScene::icons`]: the published models drawn as silhouettes (flat, wide views), with
//!   their sizes, screen angles and colours.
//! - [`AircraftScene::labelled`]: the label candidates, at the very positions the aircraft are
//!   drawn at (item A7 lays them out).
//! - [`AircraftScene::taxi_wanted`]: the flights whose airport's taxiways the motion model may want
//!   (zoom ≥ 12: `taxiwaysFor(id, phase)`, item A11).
//!
//! Formulas, constants and the order of the loop are the TypeScript's; `Math.round` is
//! [`js_round`], `clamp01` [`clamp01`], a colour stored in deck's `unorm8` attribute is
//! [`clamp_u8`] (a `Uint8ClampedArray` store).
//!
//! Deviations, decided by the plan (1.4.2, doc 03 §20.3):
//! - **No `yawOffset`.** deck's globe frame for mesh instances is the flat one turned half a turn
//!   about up, so today's app passes `180 − heading` on the globe. The map builds a true
//!   east/north/up frame at each aircraft, so [`orientation_of`] is `[bank, −heading, pitch]` in
//!   both projections (and the shadows' sun is not flipped either, item A6).
//! - Positions stay `[lon, lat, metres]`: the map projects them in `f64` (plan 1.4.2).

use std::collections::HashMap;

use indexmap::IndexMap;
use rustradar_core::geo::{AIRCRAFT_COLOR, Rgb, SELECTED_COLOR, cos_angle, effective_exaggeration, elevation, meters_per_pixel, visible_radius};
use rustradar_core::labels::aircraft_label;
use rustradar_core::liveries::{Livery, PARTS, Part};
use rustradar_core::model::{LiveFlight, f32_as_js};
use rustradar_core::motion::aircraft::{Phase, Pose};
use rustradar_core::num::{clamp_u8, clamp01, js_round};

use crate::aircraft_assets::{AircraftModel, AircraftModels};
use crate::aircraft_models::{self, ModelKind, model_for, model_size, size_of};
use crate::camera::{FLAT_ABOVE_ZOOM, GlobeViewport, ViewState};
use crate::screen::screen_angle;

/// The size floor on the whole globe, px (`MIN_PX_GLOBE`).
pub const MIN_PX_GLOBE: f64 = 8.0;
/// The size floor at city level, px (`MIN_PX_CITY`).
pub const MIN_PX_CITY: f64 = 24.0;
/// Up to this tilt (degrees) the view reads as a flat map, where silhouettes belong (`FLAT_PITCH`).
pub const FLAT_PITCH: f64 = 5.0;
/// From this zoom the detailed meshes are worth their triangles, and icons give way to meshes
/// (`CLOSE_ZOOM`).
pub const CLOSE_ZOOM: f64 = 11.0;
/// Model depth from which landing gear is drawn, about zoom 8.4 (`GEAR_DEPTH`).
pub const GEAR_DEPTH: f64 = 0.8;
/// Struts and tyres in one grey (`GEAR_COLOR`).
pub const GEAR_COLOR: Rgb = [178, 182, 190];
/// The hover highlight of an aircraft's mesh or icon (`highlightColor`).
pub const HIGHLIGHT_COLOR: [u8; 4] = [55, 142, 240, 150];
/// The silhouettes' border colour (`OUTLINE_COLOR`, the app's navy ink, nearly opaque).
pub const OUTLINE_COLOR: [u8; 4] = [8, 12, 20, 215];
/// How far ahead (screen pixels at the view centre) an icon's heading is sampled
/// (`ICON_HEADING_STEP_PX`).
pub const ICON_HEADING_STEP_PX: f64 = 20.0;
/// The silhouettes are drawn 94 % of the way across their cell: an icon's size is its aircraft's
/// screen size times this, so the silhouette itself spans [`Frame::screen_px`].
pub const ICON_FILL: f64 = 1.06;
/// The thinnest a mesh is flattened to (`Math.max(0.04, depth)`): silhouettes zoomed out.
pub const MIN_FLATTEN: f64 = 0.04;
/// Below this zoom only the selected aircraft gets a label (`AIRCRAFT_LABEL_MIN_ZOOM`).
pub const AIRCRAFT_LABEL_MIN_ZOOM: f64 = 4.0;
/// Up to this zoom only aircraft well short of the horizon get a label...
pub const LABEL_FAR_ZOOM: f64 = 6.0;
/// ... those whose reported position's angle from the view centre has a cosine above this (about
/// 65°).
pub const LABEL_FACING: f64 = 0.42;
/// From this zoom the airports aircraft are on the ground at get their taxiways
/// (`taxiways.ts` `TAXIWAY_ZOOM`).
pub const TAXIWAY_ZOOM: f64 = 12.0;

/// `Math.min(a, b)`: NaN when either is.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}

/// `Math.max(a, b)`: NaN when either is.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}

/// The size floor at `zoom`, px (`minPxAt`, G:339): 8 at zoom ≤ 3 rising to 24 at zoom ≥ 10.
pub fn min_px_at(zoom: f64) -> f64 {
    MIN_PX_GLOBE + (MIN_PX_CITY - MIN_PX_GLOBE) * clamp01((zoom - 3.0) / 7.0)
}

/// Model depth at `zoom` (`depthAt`, G:343): flat silhouettes (0) at zoom ≤ 6, full depth (1) from
/// zoom 9, in steps of 0.1.
pub fn depth_at(zoom: f64) -> f64 {
    js_round(clamp01((zoom - 6.0) / 3.0) * 10.0) / 10.0
}

/// How much of the airline colours shows (`liveryAmount`, G:2547): none at zoom ≤ 6, all from
/// zoom 8, in steps of 0.1.
pub fn livery_amount(zoom: f64) -> f64 {
    js_round(clamp01((zoom - 6.0) / 2.0) * 10.0) / 10.0
}

/// Whether published models are drawn as top-down silhouettes (`useIcons`, G:2550): straight down
/// and wide, once the models have loaded.
pub fn use_icons(zoom: f64, pitch: f64, models_loaded: bool) -> bool {
    pitch <= FLAT_PITCH && zoom < CLOSE_ZOOM && models_loaded
}

/// The published meshes' level of detail (`detail`, G:2551): 1 from zoom 11.
pub fn detail(zoom: f64) -> usize {
    if zoom >= CLOSE_ZOOM { 1 } else { 0 }
}

/// How thick a mesh is drawn at model depth `depth` (`Math.max(0.04, depth)`).
pub fn flatten(depth: f64) -> f64 {
    js_max(MIN_FLATTEN, depth)
}

/// deck's `[pitch, yaw, roll]` for a pose (`orientationOf`, G:1572-1576) with the yaw offset
/// dropped (see the module's documentation): deck's "pitch" turns the mesh about its own y (nose)
/// axis, so it takes the aircraft's bank; its "roll" turns about x (the wing), the aircraft's
/// pitch; the yaw turns the nose (+y, north) clockwise to the heading.
pub fn orientation_of(pose: &Pose) -> [f64; 3] {
    [pose.bank, 0.0 - pose.heading, pose.pitch]
}

/// Where the mesh's up axis points after `orientation` (`upAxis`, G:1583-1586): the third column
/// of deck's rotation (`@deck.gl/mesh-layers` `utils/matrix.ts`).
pub fn up_axis([p, y, r]: [f64; 3]) -> [f64; 3] {
    let (sp, cp) = p.to_radians().sin_cos();
    let (sy, cy) = y.to_radians().sin_cos();
    let (sr, cr) = r.to_radians().sin_cos();
    [sy * sr + cy * sp * cr, -cy * sr + sy * sp * cr, cp * cr]
}

/// The lowest point of a mesh, model z in metres, and never above 0 (`lowestZ`, G:376-387): where
/// it stands. `positions` is `x, y, z` per vertex.
pub fn lowest_z(positions: &[f32]) -> f64 {
    let mut z = 0.0;
    for v in positions.chunks_exact(3) {
        z = js_min(z, f64::from(v[2]));
    }
    z
}

/// `mix(a, b, t)` per channel (G:1558), unrounded, as deck's `getColor` returns it.
pub fn mix(a: Rgb, b: Rgb, t: f64) -> [f64; 3] {
    [0, 1, 2].map(|k| f64::from(a[k]) + (f64::from(b[k]) - f64::from(a[k])) * t)
}

/// An RGB colour as deck's `unorm8` colour attribute stores it (a `Uint8ClampedArray`, alpha
/// filled with 255).
pub fn unorm8([r, g, b]: [f64; 3]) -> [u8; 4] {
    [clamp_u8(r), clamp_u8(g), clamp_u8(b), 255]
}

/// An opaque byte colour.
pub fn opaque([r, g, b]: Rgb) -> [u8; 4] {
    [r, g, b, 255]
}

/// The preamble of `buildLayers()` (G:2520-2581), worked out once per refresh from the view.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Frame {
    pub zoom: f64,
    pub pitch: f64,
    pub bearing: f64,
    /// The view centre (`cLon`, `cLat`).
    pub longitude: f64,
    pub latitude: f64,
    /// The flat map (`zoom > 12`) rather than the globe.
    pub flat: bool,
    /// The altitude exaggeration at this zoom (`ex`, doc 03 §1.2): the setting at zoom ≤ 6, 1 from
    /// zoom 11.
    pub exaggeration: f64,
    /// Metres per CSS pixel at the view centre (`metersPerPixel`): every aircraft is sized with it
    /// (doc 03 §20.4).
    pub mpp: f64,
    /// The size floor, px ([`min_px_at`]).
    pub min_px: f64,
    /// Model depth ([`depth_at`]); the meshes' material is `meshMaterial(depth)`.
    pub depth: f64,
    /// The cull radius, degrees (`visibleRadius`), and its cosine.
    pub radius: f64,
    pub min_cos: f64,
    /// [`livery_amount`]: 0 draws each aircraft in one colour, more draws each livery part.
    pub livery_amount: f64,
    /// [`use_icons`].
    pub use_icons: bool,
    /// [`detail`].
    pub detail: usize,
    /// Zoom ≥ 12: taxiways are wanted for ground traffic.
    pub taxiways: bool,
}

impl Frame {
    /// The preamble for `view` on a canvas of `size` CSS pixels, with the altitude exaggeration
    /// setting `exaggeration`, the published models loaded or not.
    pub fn new(view: &ViewState, [width, height]: [f64; 2], exaggeration: f64, models_loaded: bool) -> Frame {
        let ViewState { longitude, latitude, zoom, bearing, pitch, .. } = *view;
        let radius = visible_radius(zoom, pitch, width, height);
        Frame {
            zoom,
            pitch,
            bearing,
            longitude,
            latitude,
            flat: zoom > FLAT_ABOVE_ZOOM,
            exaggeration: effective_exaggeration(exaggeration, zoom),
            mpp: meters_per_pixel(zoom, latitude),
            min_px: min_px_at(zoom),
            depth: depth_at(zoom),
            radius,
            min_cos: (radius * std::f64::consts::PI / 180.0).cos(),
            livery_amount: livery_amount(zoom),
            use_icons: use_icons(zoom, pitch, models_loaded),
            detail: detail(zoom),
            taxiways: zoom >= TAXIWAY_ZOOM,
        }
    }

    /// On-screen size, px, of an aircraft whose largest dimension is `metres` (`screenPx`,
    /// G:2531-2537): never smaller than its true size, and never smaller than the floor, which
    /// itself says big from small (an A380 about 1.8× an A320 at the floor, a Cessna 0.6×).
    pub fn screen_px(&self, metres: f64) -> f64 {
        let ratio = (metres / model_size(ModelKind::Narrow)).powf(0.8);
        let floor = self.min_px * js_min(2.0, js_max(0.6, ratio));
        js_max(floor, metres / self.mpp)
    }

    /// The mesh scale factor of an aircraft of `metres` drawn with a model of size `divisor`
    /// (`screenPx(metres) · mpp / divisor`, doc 03 §5.1): 1 for a published model at true size.
    pub fn mesh_scale(&self, metres: f64, divisor: f64) -> f64 {
        (self.screen_px(metres) * self.mpp) / divisor
    }

    /// How thick meshes are drawn ([`flatten`] of the depth).
    pub fn flatten(&self) -> f64 {
        flatten(self.depth)
    }
}

/// What the aircraft drawing reads of the app's state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inputs {
    /// The camera the scene is built for.
    pub view: ViewState,
    /// The canvas in CSS pixels (`container.clientWidth`, `clientHeight`).
    pub size: [f64; 2],
    /// The altitude exaggeration setting (`exaggeration`).
    pub exaggeration: f64,
    /// Aircraft on the map (`showAircraft`: the mode or "also show" has them).
    pub show_aircraft: bool,
    /// Aircraft labels on and their font ready (`aircraftLabels && fontsReady`).
    pub labelling: bool,
    /// The selected flight's id (`selected?.id`).
    pub selected: Option<u32>,
    /// The camera follows the selected flight (`follow`).
    pub follow: bool,
    /// The flight under the pointer (`hoveredId`).
    pub hovered: Option<u32>,
}

/// Which model a group draws.
#[derive(Debug, Clone, PartialEq)]
pub enum GroupModel {
    /// A published model (`real`): its name, largest dimension (metres), the level of detail drawn
    /// (`min(detail, lods.length − 1)`), and its gear's attach plane if it has gear.
    Published { name: String, size: f64, lod: usize, gear_attach_z: Option<f64> },
    /// A built-in procedural model ([`crate::aircraft_models`]).
    BuiltIn(ModelKind),
}

/// One aircraft drawn as a mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshInstance {
    /// The flight's id.
    pub id: u32,
    /// Where the flight is in the flights handed to [`AircraftBuilder::build`].
    pub row: usize,
    /// `motion.pose(f, now)`.
    pub pose: Pose,
    /// The aircraft's real size, metres (`real?.size ?? sizeOf(f.icon)`).
    pub size_m: f64,
    /// `[lon, lat, metres]`: at its exaggerated altitude, raised onto its wheels (G:2623-2636).
    pub position: [f64; 3],
    /// deck's `getOrientation` ([`orientation_of`]).
    pub orientation: [f64; 3],
    /// deck's `getScale`: `[s, s, s · flatten]`.
    pub scale: [f64; 3],
    /// The colour of the whole aircraft, drawn when [`Frame::livery_amount`] is 0: selection blue
    /// for the selected flight, else white.
    pub color: [u8; 4],
    /// With [`Frame::livery_amount`] above 0: each part's colour, white mixed toward the livery by
    /// that amount, in [`PARTS`] order. The selected aircraft is not tinted blue then (doc 03
    /// §20.1).
    pub parts: Option<[[u8; 4]; 5]>,
}

impl MeshInstance {
    /// The colour `part` is drawn in: its livery colour, or the whole aircraft's without liveries.
    pub fn part_color(&self, part: Part) -> [u8; 4] {
        match self.parts {
            Some(parts) => parts[PARTS.iter().position(|&p| p == part).unwrap_or(0)],
            None => self.color,
        }
    }
}

/// One aircraft's landing gear (doc 03 §5.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GearInstance {
    /// Its aircraft in [`Group::instances`].
    pub instance: usize,
    /// The aircraft's position and orientation.
    pub position: [f64; 3],
    pub orientation: [f64; 3],
    /// `[s, s, s · flatten · gear]`: squashed up into the airframe about its attach plane.
    pub scale: [f64; 3],
    /// Metres east, north and up (deck's `getTranslation`): along the aircraft's up axis, so the
    /// attach plane stays put while the gear retracts.
    pub translation: [f64; 3],
    /// [`GEAR_COLOR`].
    pub color: [u8; 4],
}

/// The aircraft drawn with one model: one instanced draw (per livery part).
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub model: GroupModel,
    /// `modelFor(icon)` of the flight that opened the group (the model drawn when it is built-in).
    pub kind: ModelKind,
    /// In feed order.
    pub instances: Vec<MeshInstance>,
    /// The hovered aircraft's instance (`highlightedObjectIndex`), highlighted [`HIGHLIGHT_COLOR`].
    pub highlighted: Option<usize>,
    /// The gear of the aircraft with it down, when the model has gear and depth is at least
    /// [`GEAR_DEPTH`] (layer `gear-{id}`, drawn before the airframes).
    pub gear: Vec<GearInstance>,
}

impl Group {
    /// The group's id in today's layer ids: `t-<name>` or `k-<kind>`.
    pub fn id(&self) -> String {
        match &self.model {
            GroupModel::Published { name, .. } => format!("t-{name}"),
            GroupModel::BuiltIn(kind) => format!("k-{}", kind.as_str()),
        }
    }

    /// What the scale is divided by: the published model's size, or the built-in's nominal one
    /// (`MODEL_SIZE`).
    pub fn divisor(&self) -> f64 {
        match &self.model {
            GroupModel::Published { size, .. } => *size,
            GroupModel::BuiltIn(kind) => model_size(*kind),
        }
    }
}

/// One aircraft drawn as a silhouette (doc 03 §6).
#[derive(Debug, Clone, PartialEq)]
pub struct Icon {
    pub id: u32,
    /// Where the flight is in the flights handed to [`AircraftBuilder::build`].
    pub row: usize,
    pub pose: Pose,
    /// `[lon, lat, metres]` at its exaggerated altitude (icons are not raised onto wheels).
    pub position: [f64; 3],
    /// The published model's name: its cell in both atlases (`iconNames[i]`).
    pub name: String,
    /// The published model's size, metres (`iconSizes[i]`).
    pub size_m: f64,
    /// The icon's size, px (`screenPx(size) · 1.06`).
    pub size_px: f64,
    /// The outline's size, px (`screenPx(size) · 1.06 · outlineScale`).
    pub outline_size_px: f64,
    /// Anticlockwise screen rotation of the north-up silhouette, degrees (deck's `getAngle`):
    /// `screenAngle(…)`, or `bearing − heading` where that has no direction.
    pub angle: f64,
    /// Selection blue for the selected flight, else white.
    pub color: [u8; 4],
}

/// An aircraft that gets a label (doc 03 §7.2): where it is drawn and how big it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Labelled {
    pub id: u32,
    /// Where the flight is in the flights handed to [`AircraftBuilder::build`].
    pub row: usize,
    /// Where the aircraft is drawn: a mesh's raised position, an icon's position.
    pub position: [f64; 3],
    /// `real?.size ?? sizeOf(f.icon)`, metres.
    pub size_m: f64,
}

/// Everything the aircraft layers draw at one scene refresh.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AircraftScene {
    pub frame: Frame,
    /// The selected and the hovered flight, as handed in.
    pub selected: Option<u32>,
    pub hovered: Option<u32>,
    /// The mesh groups, in order of their first aircraft in the feed.
    pub groups: Vec<Group>,
    /// The silhouettes, in feed order.
    pub icons: Vec<Icon>,
    /// The hovered aircraft's icon (`iconIndex.get(hoveredId)`), highlighted [`HIGHLIGHT_COLOR`].
    pub hovered_icon: Option<usize>,
    /// The aircraft to label, in feed order.
    pub labelled: Vec<Labelled>,
    /// Zoom ≥ 12: each drawn flight's id and phase, in feed order (`taxiwaysFor(f.id, pose.phase)`).
    pub taxi_wanted: Vec<(u32, Phase)>,
}

impl AircraftScene {
    /// How many aircraft are drawn as meshes.
    pub fn mesh_count(&self) -> usize {
        self.groups.iter().map(|g| g.instances.len()).sum()
    }

    /// How many gear instances are drawn.
    pub fn gear_count(&self) -> usize {
        self.groups.iter().map(|g| g.gear.len()).sum()
    }
}

/// Which mesh an aircraft of a group stands on (the cache key of `lowestZ`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum StandsOn {
    /// A published model's gear (by the models' address and the name), or a level of detail.
    Gear(usize, String),
    Lod(usize, String, usize),
    BuiltIn(ModelKind),
}

/// A group's key: `t-<name>` or `k-<kind>` (G:2610).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum GroupKey<'m> {
    Published(&'m str),
    BuiltIn(ModelKind),
}

/// A group being filled, with the published model it draws (`group.real`).
struct Filling<'m> {
    real: Option<&'m AircraftModel>,
    group: Group,
}

/// The aircraft's scene builder: keeps the meshes' lowest points between refreshes (`lowestZ`'s
/// cache).
#[derive(Debug, Default)]
pub struct AircraftBuilder {
    lowest: HashMap<StandsOn, f64>,
}

impl AircraftBuilder {
    pub fn new() -> Self {
        AircraftBuilder::default()
    }

    /// How far below its origin the mesh a group stands on reaches (`−lowestZ(mesh)` of
    /// `real?.gear?.geometry ?? real?.lods[min(detail, …)].combined ?? models()[kind].combined`).
    fn wheels(&mut self, models: Option<&AircraftModels>, real: Option<&AircraftModel>, kind: ModelKind, detail: usize) -> f64 {
        let at = models.map_or(0, |m| m as *const AircraftModels as usize);
        let (key, positions): (StandsOn, Option<&[f32]>) = match real {
            Some(real) => match &real.gear {
                Some(gear) => (StandsOn::Gear(at, real.name.clone()), Some(&gear.mesh.positions)),
                None => {
                    let lod = detail.min(real.lods.len().saturating_sub(1));
                    (StandsOn::Lod(at, real.name.clone(), lod), real.lods.get(lod).map(|l| l.mesh.positions.as_slice()))
                }
            },
            None => (StandsOn::BuiltIn(kind), Some(aircraft_models::model(kind).combined.positions.as_slice())),
        };
        let z = *self.lowest.entry(key).or_insert_with(|| positions.map_or(0.0, lowest_z));
        -z
    }

    /// The aircraft of one scene refresh (G:2520-2636 and the CPU parts of G:2870-3025).
    ///
    /// `flights` are the flights shown (`flights`, in feed order); `models` the published models,
    /// `None` until (or unless) they load (`aircraftModels.loaded`). For each flight not culled,
    /// in order, `pose` is asked once (`motion.pose(f, now)` at the feed clock); `livery` is asked
    /// for each aircraft drawn as a mesh while liveries show (`liveries.forFlight(f)`, which may
    /// queue the airline's logo colours), in the same order as today.
    pub fn build<'f>(
        &mut self,
        inputs: &Inputs,
        models: Option<&AircraftModels>,
        flights: impl IntoIterator<Item = &'f LiveFlight>,
        mut pose: impl FnMut(&LiveFlight) -> Pose,
        mut livery: impl FnMut(&LiveFlight) -> Livery,
    ) -> AircraftScene {
        let frame = Frame::new(&inputs.view, inputs.size, inputs.exaggeration, models.is_some());
        let (c_lon, c_lat) = (frame.longitude, frame.latitude);
        let zoom = frame.zoom;
        // the aircraft the camera is locked to is always drawn, even if the camera is aimed far
        // enough ahead of it to put it past the cull radius
        let locked_on = if inputs.follow { inputs.selected } else { None };
        let selected = inputs.selected;
        let label_all = inputs.labelling && zoom >= AIRCRAFT_LABEL_MIN_ZOOM;

        let mut scene = AircraftScene { frame, selected, hovered: inputs.hovered, ..Default::default() };
        let mut groups: IndexMap<GroupKey<'_>, Filling<'_>> = IndexMap::new();
        // per label: the group and instance whose raised position it takes
        let mut label_at: Vec<Option<(usize, usize)>> = Vec::new();

        if inputs.show_aircraft {
            for (row, f) in flights.into_iter().enumerate() {
                let facing = cos_angle(f32_as_js(f.lon), f32_as_js(f.lat), c_lon, c_lat);
                if Some(f.id) != locked_on && facing <= frame.min_cos {
                    continue;
                }
                let real = models.and_then(|m| m.for_flight(&f.typecode, &f.icon));
                let pose = pose(f);
                if frame.taxiways {
                    scene.taxi_wanted.push((f.id, pose.phase));
                }
                let position = [pose.lon, pose.lat, elevation(pose.alt_ft, frame.exaggeration)];
                let size_m = real.map_or_else(|| size_of(&f.icon), |r| r.size);
                let labelled = inputs.labelling
                    && (label_all || Some(f.id) == selected)
                    && (zoom > LABEL_FAR_ZOOM || facing > LABEL_FACING || Some(f.id) == locked_on)
                    && !aircraft_label(f).is_empty();
                let color = opaque(if Some(f.id) == selected { SELECTED_COLOR } else { AIRCRAFT_COLOR });

                if let (true, Some(real)) = (frame.use_icons, real) {
                    if labelled {
                        scene.labelled.push(Labelled { id: f.id, row, position, size_m });
                        label_at.push(None);
                    }
                    if inputs.hovered == Some(f.id) {
                        scene.hovered_icon = Some(scene.icons.len());
                    }
                    let screen = frame.screen_px(real.size);
                    let outline_scale = models.map_or(1.0, |m| m.outline_scale);
                    scene.icons.push(Icon {
                        id: f.id,
                        row,
                        pose,
                        position,
                        name: real.name.clone(),
                        size_m: real.size,
                        size_px: screen * ICON_FILL,
                        outline_size_px: screen * ICON_FILL * outline_scale,
                        // set below, once every icon is in
                        angle: 0.0,
                        color,
                    });
                    continue;
                }

                // balloons, ground vehicles and anything else with no published model
                let (kind, _) = model_for(&f.icon);
                let key = match real {
                    Some(real) => GroupKey::Published(&real.name),
                    None => GroupKey::BuiltIn(kind),
                };
                let entry = groups.entry(key);
                let g = entry.index();
                let filling = entry.or_insert_with(|| Filling {
                    real,
                    group: Group {
                        model: match real {
                            Some(real) => GroupModel::Published {
                                name: real.name.clone(),
                                size: real.size,
                                lod: frame.detail.min(real.lods.len().saturating_sub(1)),
                                gear_attach_z: real.gear.as_ref().map(|gear| gear.attach_z),
                            },
                            None => GroupModel::BuiltIn(kind),
                        },
                        kind,
                        instances: Vec::new(),
                        highlighted: None,
                        gear: Vec::new(),
                    },
                });
                let group = &mut filling.group;
                let i = group.instances.len();
                if labelled {
                    scene.labelled.push(Labelled { id: f.id, row, position, size_m });
                    label_at.push(Some((g, i)));
                }
                if inputs.hovered == Some(f.id) {
                    group.highlighted = Some(i);
                }
                let s = frame.mesh_scale(size_m, group.divisor());
                let parts = (frame.livery_amount > 0.0).then(|| {
                    let livery = livery(f);
                    PARTS.map(|part| unorm8(mix(AIRCRAFT_COLOR, livery[part], frame.livery_amount)))
                });
                group.instances.push(MeshInstance {
                    id: f.id,
                    row,
                    pose,
                    size_m,
                    position,
                    orientation: orientation_of(&pose),
                    scale: [s, s, s * frame.flatten()],
                    color,
                    parts,
                });
            }
        }

        // Models are centred on their bounding box, so an aircraft placed by its origin at height 0
        // would stand with its wheels below the map: raise each by how far its wheels hang below
        // the origin at the size it is drawn, so one on the ground stands on them.
        for filling in groups.values_mut() {
            let wheels = self.wheels(models, filling.real, filling.group.kind, frame.detail);
            let flat = frame.flatten();
            for instance in &mut filling.group.instances {
                // `s` as the scale has it, worked out again as the TypeScript does
                instance.position[2] += wheels * instance.scale[0] * flat;
            }
        }
        for (labelled, at) in scene.labelled.iter_mut().zip(&label_at) {
            if let Some((g, i)) = *at {
                labelled.position = groups[g].group.instances[i].position;
            }
        }

        // gear only once the models have depth; a silhouette has nowhere to hang it
        if frame.depth >= GEAR_DEPTH {
            let flat = frame.flatten();
            for filling in groups.values_mut() {
                let Some(attach_z) = filling.real.and_then(|r| r.gear.as_ref()).map(|g| g.attach_z) else { continue };
                let group = &mut filling.group;
                for (i, m) in group.instances.iter().enumerate() {
                    let gear = m.pose.gear;
                    // `pose.gear > 0` (NaN is not)
                    if gear.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
                        continue;
                    }
                    let s = m.scale[0];
                    let lift = attach_z * (1.0 - gear) * s * flat;
                    let [x, y, z] = up_axis(m.orientation);
                    group.gear.push(GearInstance {
                        instance: i,
                        position: m.position,
                        orientation: m.orientation,
                        scale: [s, s, s * flat * gear],
                        translation: [x * lift, y * lift, z * lift],
                        color: opaque(GEAR_COLOR),
                    });
                }
            }
        }

        if !scene.icons.is_empty() {
            // each icon turned to its heading as seen on screen, which away from the middle of the
            // globe is not simply `bearing − heading`; icons only show below zoom 11, where the view
            // is always the globe projection
            let viewport = GlobeViewport::new(&inputs.view, inputs.size[0], inputs.size[1]);
            let step = ICON_HEADING_STEP_PX * frame.mpp;
            for icon in &mut scene.icons {
                icon.angle = screen_angle(&viewport, icon.position, icon.pose.heading, step)
                    .unwrap_or(frame.bearing - icon.pose.heading);
            }
        }

        scene.groups = groups.into_values().map(|f| f.group).collect();
        scene
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;
    use rustradar_core::liveries::DEFAULT_LIVERY;

    fn shipped() -> AircraftModels {
        let read = |name: &str| {
            let path = format!("{}/../../static/aircraft/{name}", env!("CARGO_MANIFEST_DIR"));
            std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
        };
        AircraftModels::parse(&read("index.json"), &read("meshes.bin")).unwrap()
    }

    fn view(lat: f64, lon: f64, zoom: f64, pitch: f64) -> ViewState {
        ViewState { longitude: lon, latitude: lat, zoom, bearing: 0.0, pitch, position: None }
    }

    fn inputs(view: ViewState) -> Inputs {
        Inputs {
            view,
            size: [1100.0, 720.0],
            exaggeration: 1.0,
            show_aircraft: true,
            labelling: true,
            selected: None,
            follow: false,
            hovered: None,
        }
    }

    fn flight(id: u32, lat: f32, lon: f32, alt: i32, typecode: &str, icon: &str) -> LiveFlight {
        LiveFlight {
            id,
            lat,
            lon,
            alt,
            track: 90,
            speed: 400,
            on_ground: alt == 0,
            callsign: format!("TST{id}"),
            typecode: typecode.into(),
            icon: icon.into(),
            ..Default::default()
        }
    }

    /// The reported position as the pose, wheels down on the ground.
    fn pose(f: &LiveFlight) -> Pose {
        Pose {
            lon: f32_as_js(f.lon),
            lat: f32_as_js(f.lat),
            alt_ft: f64::from(f.alt),
            heading: f64::from(f.track),
            track: f64::from(f.track),
            pitch: 0.0,
            bank: 0.0,
            gear: if f.on_ground { 1.0 } else { 0.0 },
            phase: if f.on_ground { Phase::Parked } else { Phase::Cruise },
        }
    }

    fn build(inputs: &Inputs, models: Option<&AircraftModels>, flights: &[LiveFlight]) -> AircraftScene {
        AircraftBuilder::new().build(inputs, models, flights, pose, |_| DEFAULT_LIVERY)
    }

    #[test]
    fn the_size_floor_at_zoom_3_6_9_10_11() {
        assert_eq!(min_px_at(1.9), 8.0);
        assert_eq!(min_px_at(3.0), 8.0);
        assert_close!(min_px_at(6.0), 8.0 + 16.0 * 3.0 / 7.0, 12);
        assert_close!(min_px_at(9.0), 8.0 + 16.0 * 6.0 / 7.0, 12);
        assert_eq!(min_px_at(10.0), 24.0);
        assert_eq!(min_px_at(11.0), 24.0);
        assert_eq!(min_px_at(16.0), 24.0);
    }

    #[test]
    fn model_depth_in_tenths_from_zoom_6_to_9() {
        assert_eq!(depth_at(3.0), 0.0);
        assert_eq!(depth_at(6.0), 0.0);
        // (6.1 − 6)/3 · 10 = 0.33: rounds to 0; 6.15 gives 0.5000000000000012, a tenth
        assert_eq!(depth_at(6.1), 0.0);
        assert_eq!(depth_at(6.15), 0.1);
        assert_eq!(depth_at(7.5), 0.5);
        // the gear's threshold: depth 0.8 at zoom 8.4, 0.7 at 8.2
        assert_eq!(depth_at(8.4), GEAR_DEPTH);
        assert_eq!(depth_at(8.2), 0.7);
        assert_eq!(depth_at(9.0), 1.0);
        assert_eq!(depth_at(10.0), 1.0);
        assert_eq!(depth_at(11.0), 1.0);
        assert_eq!(flatten(0.0), MIN_FLATTEN);
        assert_eq!(flatten(0.5), 0.5);
    }

    #[test]
    fn liveries_fade_in_over_zoom_6_to_8() {
        assert_eq!(livery_amount(3.0), 0.0);
        assert_eq!(livery_amount(6.0), 0.0);
        // 0.4999999999999982 tenths: still none
        assert_eq!(livery_amount(6.1), 0.0);
        assert_eq!(livery_amount(6.2), 0.1);
        assert_eq!(livery_amount(7.0), 0.5);
        assert_eq!(livery_amount(8.0), 1.0);
        assert_eq!(livery_amount(9.0), 1.0);
        assert_eq!(livery_amount(11.0), 1.0);
    }

    #[test]
    fn icons_below_zoom_11_up_to_pitch_5_once_loaded() {
        assert!(use_icons(1.9, 0.0, true));
        assert!(use_icons(10.0, 5.0, true));
        assert!(use_icons(10.999, 0.0, true));
        assert!(!use_icons(11.0, 0.0, true));
        assert!(!use_icons(10.0, 5.001, true));
        assert!(!use_icons(3.0, 0.0, false), "never without the published models");
        assert_eq!(detail(9.0), 0);
        assert_eq!(detail(10.0), 0);
        assert_eq!(detail(10.999), 0);
        assert_eq!(detail(11.0), 1);
        assert_eq!(detail(15.0), 1);
    }

    #[test]
    fn the_preamble_follows_the_view() {
        let f = Frame::new(&view(59.33, 18.07, 9.0, 0.0), [1100.0, 720.0], 3.0, true);
        assert!(!f.flat);
        assert_eq!(f.depth, 1.0);
        assert_eq!(f.livery_amount, 1.0);
        assert!(f.use_icons);
        assert_eq!(f.detail, 0);
        assert!(!f.taxiways);
        // exaggeration: the setting at zoom ≤ 6, true scale from 11, linear between
        assert_close!(f.exaggeration, 1.0 + 2.0 * 0.4, 12);
        assert_eq!(Frame::new(&view(0.0, 0.0, 6.0, 0.0), [800.0, 600.0], 3.0, true).exaggeration, 3.0);
        assert_eq!(Frame::new(&view(0.0, 0.0, 3.0, 0.0), [800.0, 600.0], 3.0, true).exaggeration, 3.0);
        assert_eq!(Frame::new(&view(0.0, 0.0, 11.0, 0.0), [800.0, 600.0], 3.0, true).exaggeration, 1.0);
        let close = Frame::new(&view(59.65, 17.92, 13.5, 45.0), [1100.0, 720.0], 3.0, true);
        assert!(close.flat && close.taxiways && !close.use_icons);
        assert_eq!(close.detail, 1);
        assert_close!(close.mpp, meters_per_pixel(13.5, 59.65), 12);
        // the cull radius: visibleRadius, capped at 100°
        assert_close!(f.radius, visible_radius(9.0, 0.0, 1100.0, 720.0), 12);
        assert_close!(f.min_cos, f.radius.to_radians().cos(), 12);
        assert_eq!(Frame::new(&view(0.0, 0.0, 1.0, 60.0), [1100.0, 720.0], 1.0, true).radius, 100.0);
    }

    #[test]
    fn screen_size_floors_and_true_size() {
        let far = Frame::new(&view(0.0, 0.0, 3.0, 0.0), [1100.0, 720.0], 1.0, true);
        assert_eq!(far.min_px, 8.0);
        // an A320-class at the floor; an A380 about 1.81 times it; a PA-28 at the 0.6 clamp; the
        // An-225 at the 2 clamp would need 89.4 m
        assert_close!(far.screen_px(38.0), 8.0, 12);
        assert_close!(far.screen_px(79.8), 8.0 * (79.8f64 / 38.0).powf(0.8), 12);
        assert_close!(far.screen_px(79.8) / far.screen_px(38.0), 1.81, 3);
        assert_close!(far.screen_px(10.55), 8.0 * 0.6, 12);
        assert_close!(far.screen_px(200.0), 16.0, 12);
        // close in: true size, no ceiling
        let close = Frame::new(&view(59.33, 18.07, 16.0, 0.0), [1100.0, 720.0], 1.0, true);
        assert_eq!(close.min_px, 24.0);
        assert_close!(close.screen_px(38.0), 38.0 / close.mpp, 12);
        assert!(close.screen_px(38.0) > 60.0);
        // a published model's scale is 1 at true size, more below it
        assert_eq!(close.mesh_scale(38.0, 38.0), 1.0);
        assert_close!(far.mesh_scale(38.0, 38.0), 8.0 * far.mpp / 38.0, 12);
        // a built-in model of nominal size 38 m stretched for a 32.3 m aircraft (B736's 0.85)
        assert_close!(close.mesh_scale(32.3, 38.0), 0.85, 12);
    }

    #[test]
    fn orientation_and_up_axis_as_deck_turns_the_mesh() {
        let p = Pose { lon: 0.0, lat: 0.0, alt_ft: 0.0, heading: 90.0, track: 90.0, pitch: 3.0, bank: -10.0, gear: 1.0, phase: Phase::Climb };
        assert_eq!(orientation_of(&p), [-10.0, -90.0, 3.0]);
        // level: up is up, whatever the heading
        let up = up_axis([0.0, -90.0, 0.0]);
        assert_close!(up[0], 0.0, 12);
        assert_close!(up[1], 0.0, 12);
        assert_close!(up[2], 1.0, 12);
        // nose up 10° heading north: up leans south (−y)
        let up = up_axis([0.0, 0.0, 10.0]);
        assert_close!(up[1], -(10f64.to_radians().sin()), 12);
        assert_close!(up[2], 10f64.to_radians().cos(), 12);
        // right wing down 20° heading north: up leans west... deck's "pitch" turns about y
        let up = up_axis([20.0, 0.0, 0.0]);
        assert_close!(up[0], 20f64.to_radians().sin(), 12);
        assert_close!(up[2], 20f64.to_radians().cos(), 12);
        assert_eq!(lowest_z(&[0.0, 0.0, 1.0, 5.0, 5.0, 2.0]), 0.0, "never above 0");
        assert_eq!(lowest_z(&[0.0, 0.0, 1.0, 5.0, 5.0, -2.5]), -2.5);
    }

    #[test]
    fn colours_mix_toward_the_livery_and_store_as_deck_does() {
        assert_eq!(mix([255, 255, 255], [0, 0, 0], 0.5), [127.5, 127.5, 127.5]);
        // a Uint8ClampedArray store rounds a half to even
        assert_eq!(unorm8([127.5, 128.5, 129.5]), [128, 128, 130, 255]);
        assert_eq!(unorm8([-3.0, 300.0, f64::NAN]), [0, 255, 0, 255]);
        assert_eq!(opaque(SELECTED_COLOR), [55, 142, 240, 255]);
    }

    #[test]
    fn culled_beyond_the_radius_unless_locked_on() {
        let models = shipped();
        let mut inputs = inputs(view(59.33, 18.07, 9.0, 0.0));
        let near = flight(1, 59.4, 18.1, 30_000, "A320", "A320");
        // 30° of arc away: well past the radius at zoom 9
        let far = flight(2, 40.0, -10.0, 30_000, "A320", "A320");
        let scene = build(&inputs, Some(&models), &[near.clone(), far.clone()]);
        assert_eq!(scene.icons.iter().map(|i| i.id).collect::<Vec<_>>(), [1]);
        inputs.selected = Some(2);
        let scene = build(&inputs, Some(&models), &[near.clone(), far.clone()]);
        assert_eq!(scene.icons.len(), 1, "selected but not followed: culled");
        inputs.follow = true;
        let scene = build(&inputs, Some(&models), &[near.clone(), far.clone()]);
        assert_eq!(scene.icons.iter().map(|i| i.id).collect::<Vec<_>>(), [1, 2], "the locked aircraft is always drawn");
        assert_eq!(scene.icons[1].color, [55, 142, 240, 255]);
        assert_eq!(scene.icons[0].color, [255, 255, 255, 255]);
        inputs.show_aircraft = false;
        let scene = build(&inputs, Some(&models), &[near, far]);
        assert!(scene.icons.is_empty() && scene.groups.is_empty() && scene.labelled.is_empty(), "aircraft off");
    }

    #[test]
    fn icons_for_published_models_meshes_for_the_rest_and_at_pitch_over_5() {
        let models = shipped();
        let flights = [
            flight(1, 59.30, 18.00, 20_000, "A20N", "A320"),
            flight(2, 59.31, 18.01, 20_000, "", "BALL"),
            flight(3, 59.32, 18.02, 20_000, "B738", "B738"),
            flight(4, 59.33, 18.03, 20_000, "ZZZZ", "ZZZZ"),
            flight(5, 59.34, 18.04, 20_000, "", "BALL"),
            flight(6, 59.35, 18.05, 20_000, "A321", "A320"),
        ];
        let flat = build(&inputs(view(59.33, 18.07, 9.0, 5.0)), Some(&models), &flights);
        assert_eq!(flat.icons.iter().map(|i| (i.id, i.name.as_str())).collect::<Vec<_>>(), [(1, "a320"), (3, "b738"), (6, "a321")]);
        // with no published model: built-in meshes even among icons, grouped by kind in order of
        // first appearance; an unknown type is a narrow-body
        let ids: Vec<String> = flat.groups.iter().map(Group::id).collect();
        assert_eq!(ids, ["k-balloon", "k-narrow"]);
        assert_eq!(flat.groups[0].instances.iter().map(|m| m.id).collect::<Vec<_>>(), [2, 5]);
        assert_eq!(flat.groups[1].model, GroupModel::BuiltIn(ModelKind::Narrow));

        let tilted = build(&inputs(view(59.33, 18.07, 9.0, 5.5)), Some(&models), &flights);
        assert!(tilted.icons.is_empty());
        let ids: Vec<String> = tilted.groups.iter().map(Group::id).collect();
        assert_eq!(ids, ["t-a320", "k-balloon", "t-b738", "k-narrow", "t-a321"]);
        let GroupModel::Published { name, size, lod, gear_attach_z } = &tilted.groups[0].model else { panic!() };
        assert_eq!((name.as_str(), *lod), ("a320", 0));
        assert_eq!(*size, models.get("a320").unwrap().size);
        assert!(gear_attach_z.is_some());

        // without the published models, everything is a built-in mesh and there are no icons
        let none = build(&inputs(view(59.33, 18.07, 9.0, 0.0)), None, &flights);
        assert!(none.icons.is_empty());
        let ids: Vec<String> = none.groups.iter().map(Group::id).collect();
        assert_eq!(ids, ["k-narrow", "k-balloon"]);
        assert_eq!(none.mesh_count(), 6);
        // the detailed level from zoom 11
        let close = build(&inputs(view(59.33, 18.07, 11.0, 0.0)), Some(&models), &flights);
        let GroupModel::Published { lod, .. } = &close.groups[0].model else { panic!() };
        assert_eq!(*lod, 1);
    }

    #[test]
    fn icons_are_sized_and_turned_to_their_heading_on_screen() {
        let models = shipped();
        let v = view(59.33, 18.07, 9.0, 0.0);
        let mut f = flight(1, 59.33, 18.07, 0, "A388", "A380");
        f.track = 90;
        let scene = build(&inputs(v), Some(&models), &[f]);
        let icon = &scene.icons[0];
        let size = models.get("a380").unwrap().size;
        assert_close!(icon.size_px, scene.frame.screen_px(size) * 1.06, 12);
        assert_close!(icon.outline_size_px, icon.size_px * 152.0 / 128.0, 12);
        // east on a north-up view at the centre: a quarter turn clockwise, so −90 anticlockwise
        // (0.012° less: 20 px east the meridians already converge)
        assert_close!(icon.angle, -90.0, 1);
        assert!(icon.angle != -90.0, "projected, not bearing − heading");
        // on a rotated view the world has turned under it: bearing − heading
        let rotated = ViewState { bearing: 30.0, ..v };
        let scene = build(&inputs(rotated), Some(&models), &[flight(1, 59.33, 18.07, 0, "A388", "A380")]);
        assert_close!(scene.icons[0].angle, 30.0 - 90.0, 1);
        // icons are not raised onto wheels
        assert_eq!(scene.icons[0].position[2], 0.0);
    }

    #[test]
    fn meshes_stand_on_their_wheels() {
        let models = shipped();
        let v = view(59.65, 17.92, 16.0, 45.0);
        let on_ground = flight(1, 59.65, 17.92, 0, "A320", "A320");
        let built_in = flight(2, 59.651, 17.921, 0, "", "GRND");
        let scene = build(&inputs(v), Some(&models), &[on_ground, built_in]);
        let a320 = models.get("a320").unwrap();
        let wheels = -lowest_z(&a320.gear.as_ref().unwrap().mesh.positions);
        assert!(wheels > 1.0, "the gear hangs metres below the origin: {wheels}");
        let m = &scene.groups[0].instances[0];
        assert_eq!(m.scale, [1.0, 1.0, 1.0], "true size close in");
        assert_close!(m.position[2], wheels, 12);
        // built-in models already stand on z ≈ 0
        let g = &scene.groups[1].instances[0];
        assert!(g.position[2].abs() < 1e-6, "{}", g.position[2]);
        // zoomed out the raise shrinks with the flattened, floored scale
        let far = build(&inputs(view(59.65, 17.92, 7.0, 10.0)), Some(&models), &[flight(1, 59.65, 17.92, 0, "A320", "A320")]);
        let m = &far.groups[0].instances[0];
        let frame = far.frame;
        let lod_wheels = -lowest_z(&a320.gear.as_ref().unwrap().mesh.positions);
        let s = frame.screen_px(a320.size) * frame.mpp / a320.size;
        assert_close!(m.scale[0], s, 12);
        assert_close!(m.scale[2], s * frame.depth, 12);
        assert_close!(m.position[2], lod_wheels * s * frame.depth, 12);
        // without gear (the glider) it stands on its lowest point
        let glider = build(&inputs(v), Some(&models), &[flight(3, 59.65, 17.92, 0, "AS21", "ASW20")]);
        let ask21 = models.get("ask21").unwrap();
        assert!(ask21.gear.is_none());
        let m = &glider.groups[0].instances[0];
        assert_close!(m.position[2], -lowest_z(&ask21.lods[1].mesh.positions), 12);
    }

    #[test]
    fn altitude_exaggerated_until_zoom_11() {
        let models = shipped();
        let at = |zoom: f64| {
            let mut i = inputs(view(59.33, 18.07, zoom, 30.0));
            i.exaggeration = 3.0;
            build(&i, Some(&models), &[flight(7, 59.33, 18.07, 10_000, "", "BALL")]).groups[0].instances[0].position[2]
        };
        let metres = 10_000.0 * 0.3048;
        // the balloon's lowest point is at 0, so the raise is nothing
        assert_close!(at(6.0), metres * 3.0, 9);
        assert_close!(at(8.5), metres * 2.0, 9);
        assert_close!(at(11.0), metres, 9);
        assert_close!(at(14.0), metres, 9);
    }

    #[test]
    fn colours_selected_blue_without_liveries_parts_with_them() {
        let models = shipped();
        let flights = [flight(1, 59.33, 18.07, 30_000, "A320", "A320"), flight(2, 59.34, 18.08, 30_000, "A320", "A320")];
        let mut i = inputs(view(59.33, 18.07, 6.0, 30.0));
        i.selected = Some(2);
        let mut asked = Vec::new();
        let livery = Livery { top: [0, 0, 0], belly: [2, 2, 2], wings: [4, 4, 4], tail: [255, 0, 0], engines: [10, 20, 30] };
        let scene = AircraftBuilder::new().build(&i, Some(&models), &flights, pose, |f| {
            asked.push(f.id);
            livery
        });
        assert!(asked.is_empty(), "no livery asked for at zoom 6");
        let g = &scene.groups[0];
        assert_eq!(g.instances[0].color, [255, 255, 255, 255]);
        assert_eq!(g.instances[1].color, [55, 142, 240, 255]);
        assert_eq!(g.instances[1].parts, None);
        assert_eq!(g.instances[1].part_color(Part::Tail), [55, 142, 240, 255]);

        i.view.zoom = 7.0;
        let scene = AircraftBuilder::new().build(&i, Some(&models), &flights, pose, |f| {
            asked.push(f.id);
            livery
        });
        assert_eq!(asked, [1, 2], "once per aircraft drawn as a mesh, in feed order");
        let m = &scene.groups[0].instances[1];
        // half way from white: 127.5 → 128, 128.5 → 128, 129.5 → 130 (ties to even)
        assert_eq!(m.part_color(Part::Top), [128, 128, 128, 255]);
        assert_eq!(m.part_color(Part::Belly), [128, 128, 128, 255]);
        assert_eq!(m.part_color(Part::Wings), [130, 130, 130, 255]);
        assert_eq!(m.part_color(Part::Tail), [255, 128, 128, 255]);
        assert_eq!(m.part_color(Part::Engines), [132, 138, 142, 255]);
        // the selection does not tint the parts
        assert_eq!(scene.groups[0].instances[0].parts, m.parts);

        // icons ask for no livery
        asked.clear();
        i.view.pitch = 0.0;
        let scene = AircraftBuilder::new().build(&i, Some(&models), &flights, pose, |f| {
            asked.push(f.id);
            livery
        });
        assert_eq!(scene.icons.len(), 2);
        assert!(asked.is_empty());
    }

    #[test]
    fn gear_from_depth_0_8_while_it_is_down() {
        let models = shipped();
        let a320 = models.get("a320").unwrap();
        let attach = a320.gear.as_ref().unwrap().attach_z;
        let mut half = |f: &LiveFlight| Pose { gear: 0.5, ..pose(f) };
        let flights = [flight(1, 59.65, 17.92, 1500, "A320", "A320"), flight(2, 59.66, 17.93, 1500, "", "BALL")];
        let at = |zoom: f64, pose: &mut dyn FnMut(&LiveFlight) -> Pose| {
            AircraftBuilder::new().build(&inputs(view(59.65, 17.92, zoom, 30.0)), Some(&models), &flights, pose, |_| DEFAULT_LIVERY)
        };
        assert_eq!(at(8.2, &mut half).gear_count(), 0, "depth 0.7");
        let scene = at(8.4, &mut half);
        assert_eq!(scene.gear_count(), 1, "the balloon has no gear");
        let g = &scene.groups[0].gear[0];
        let m = &scene.groups[0].instances[0];
        let flat = 0.8;
        assert_eq!(g.instance, 0);
        assert_eq!(g.position, m.position);
        assert_eq!(g.color, [178, 182, 190, 255]);
        assert_close!(g.scale[2], m.scale[0] * flat * 0.5, 12);
        // level: the lift is straight up, keeping the attach plane where it was
        assert_close!(g.translation[2], attach * 0.5 * m.scale[0] * flat, 12);
        assert!(g.translation[0].abs() < 1e-12 && g.translation[1].abs() < 1e-12);
        // retracted: none
        assert_eq!(at(14.0, &mut pose).gear_count(), 0);
        let mut down = |f: &LiveFlight| Pose { gear: 1.0, ..pose(f) };
        let scene = at(14.0, &mut down);
        assert_eq!(scene.gear_count(), 1);
        assert_eq!(scene.groups[0].gear[0].translation, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn labels_for_whom_and_where() {
        let models = shipped();
        // 4° east of the centre the cosine is above 0.42; 70° south it is not
        let near = flight(1, 52.0, 14.0, 30_000, "A320", "A320");
        let far = flight(2, -18.0, 10.0, 30_000, "A320", "A320");
        let mut nameless = flight(3, 52.1, 10.1, 30_000, "", "BALL");
        nameless.callsign = "  ".into();
        let ball = flight(4, 52.2, 10.2, 30_000, "", "BALL");
        let flights = [near, far, nameless, ball];
        let ids = |scene: &AircraftScene| scene.labelled.iter().map(|l| l.id).collect::<Vec<_>>();

        let mut i = inputs(view(52.0, 10.0, 3.9, 0.0));
        i.selected = Some(2);
        assert!(build(&i, Some(&models), &flights).labelled.is_empty(), "below zoom 4 only the selected, and it is past the horizon filter");
        i.selected = Some(1);
        assert_eq!(ids(&build(&i, Some(&models), &flights)), [1]);
        i.view.zoom = 4.0;
        i.selected = None;
        // zoom 4 at 1100 × 720 culls past about 57°: the far one is not drawn at all
        assert_eq!(ids(&build(&i, Some(&models), &flights)), [1, 4]);
        i.view.zoom = 2.5;
        i.view.pitch = 20.0;
        i.selected = Some(2);
        i.follow = true;
        let scene = build(&i, Some(&models), &flights);
        assert_eq!(scene.icons.len() + scene.mesh_count(), 4, "all drawn");
        assert_eq!(ids(&scene), [2], "below zoom 4: the selected, past the horizon filter because it is locked on");
        // tilted to 60° the cull radius at zoom 5 is about 84°
        i.view.zoom = 5.0;
        i.view.pitch = 60.0;
        i.follow = false;
        i.selected = None;
        let scene = build(&i, Some(&models), &flights);
        assert_eq!(scene.icons.len() + scene.mesh_count(), 4);
        assert_eq!(ids(&scene), [1, 4], "zoom ≤ 6: only short of the horizon; never without a name");
        i.selected = Some(2);
        assert_eq!(ids(&build(&i, Some(&models), &flights)), [1, 4], "selected, still past the horizon filter");
        i.follow = true;
        assert_eq!(ids(&build(&i, Some(&models), &flights)), [1, 2, 4], "locked on: labelled");
        // past zoom 6 every aircraft drawn is labelled (none drawn is that far out any more)
        i.view.zoom = 6.01;
        i.follow = false;
        i.selected = None;
        assert_eq!(ids(&build(&i, Some(&models), &flights)), [1, 4]);
        i.labelling = false;
        assert!(build(&i, Some(&models), &flights).labelled.is_empty());

        // a mesh's label follows it onto its wheels; sizes are the real ones
        let mut i = inputs(view(59.65, 17.92, 13.5, 45.0));
        i.labelling = true;
        let scene = build(&i, Some(&models), &[flight(9, 59.65, 17.92, 0, "A320", "A320"), flight(10, 59.651, 17.92, 0, "", "CONC")]);
        assert_eq!(scene.labelled[0].position, scene.groups[0].instances[0].position);
        assert!(scene.labelled[0].position[2] > 1.0);
        assert_eq!(scene.labelled[0].size_m, models.get("a320").unwrap().size);
        // no published model for Concorde: the built-in narrow-body's 38 m at 1.5
        assert_close!(scene.labelled[1].size_m, 38.0 * 1.5, 12);
    }

    #[test]
    fn hover_highlights_and_taxiways_close_in() {
        let models = shipped();
        let flights = [flight(1, 59.65, 17.92, 0, "A320", "A320"), flight(2, 59.651, 17.921, 0, "B738", "B738")];
        let mut i = inputs(view(59.65, 17.92, 10.0, 0.0));
        i.hovered = Some(2);
        let scene = build(&i, Some(&models), &flights);
        assert_eq!(scene.hovered_icon, Some(1));
        assert!(scene.taxi_wanted.is_empty(), "below zoom 12");
        i.view.zoom = 12.0;
        let scene = build(&i, Some(&models), &flights);
        assert_eq!(scene.hovered_icon, None);
        assert_eq!(scene.groups.iter().map(|g| g.highlighted).collect::<Vec<_>>(), [None, Some(0)]);
        assert_eq!(scene.taxi_wanted, [(1, Phase::Parked), (2, Phase::Parked)]);
    }
}
