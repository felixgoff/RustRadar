//! The measured radar volumes (item W4; doc 02 §3.3, §3.5): NOAA MRMS over North America and DWD
//! over central Europe, as the backend reduces them, drawn as today's `measuredLayers`
//! (G:1356-1384) draws them, after RainViewer's layers.
//!
//! - **What comes in** ([`Input`]): the volumes in the backend's order, each with its band
//!   polygons as they are ([`Volume`]: the ground composite, one list per height of its
//!   `RadarVolume::levels`, the coverage mask). The backend hands its isobands over as structs
//!   (`backend::radar::Polygon`, `Backend::radar_volumes`); the scene builder turns each into a
//!   [`BandPolygon`] with [`band_polygon`], which drops what `parseVolumePolygons` drops. Neither the
//!   blob nor `parseVolumePolygons` is on this path any more. With them the height exaggeration
//!   setting.
//! - **Meshed on the worker pool** ([`mesh`], a radar worker's `volume` job, `processVolume`):
//!   `scene::radar::process_volume_polygons` (the ground mesh, each height's slices with their walls
//!   from `polygonWalls`, the coverage and its key), then every mesh's vertices into the volume's
//!   own local Mercator frame ([`frame_of`]: its north-west corner, a power of two of its extent),
//!   so the upload is a copy. Uploaded within the frame's budget, one volume at a time.
//! - **A new set** (`refreshMeasured`): a volume already drawn with the same source and time is
//!   kept, the others meshed; the set drawn stays until the whole new one is in (`measured =
//!   await Promise.all(...)`), then the two swap at once and the coverages with them. A set that is
//!   the one drawn changes nothing. A meshing that fails (a job that panicked) keeps the old set,
//!   as an error does today, and the same set is not tried again until the input changes.
//! - **Coverage** ([`Measured::coverages`]): the drawn volumes' coverages in their order, for the
//!   RainViewer tiles to be blanked under (`setRadarCoverage(measured.map(v => v.coverage))` once
//!   a set is in; `layers/radar.rs` hands them to the tiles).
//! - **Layers**, per volume in order ([`layers`]): the ground (`<source>-ground-<time>`) at
//!   opacity 0.7 with `SURFACE_NO_CULL`; then, only while the view is tilted (`lift > 0`,
//!   [`lift_for`]), each height above 1 km lowest first (the lowest one is already in the ground
//!   composite): its slices (`<source>-<km>-<time>`) at `elevation(km · 3280.84 ft, scale)` and its
//!   walls (`<source>-wall-<km>-<time>`) from the previous drawn height (the ground for the first)
//!   up to it, both at opacity `0.18 · lift`, with `RADAR_LIFTED`. `scale` is the effective
//!   exaggeration at this frame's zoom rounded to tenths ([`height_scale`]). Worked out each frame
//!   from this frame's camera, as `buildLayers` does each render.
//! - **Drawn** with [`RadarPipelines`] (`shaders/radar.wgsl`): the fill pipeline's vertices and
//!   blending, unlit, both faces, no depth written, plus the walls' stretch (`radarPolygons`' model
//!   matrix: the walls' unit height times `z − bottom`, lifted by `bottom` through the frame's
//!   `z_offset`), so a change of tilt or height scale moves the layers without touching their
//!   buffers. The RainViewer walls (W3) can draw with the same pipelines.
//!
//! Found on the way, kept for parity: today's meshing covers parts of some 4° cells twice. On the
//! recorded scan the triangles of the ground in the cell 84–80° W, 40–44° N add up to 16.85 square
//! degrees in a cell of 16 (math.gl's `cutPolygonByGrid` on a polygon with a hundred holes keeps a
//! piece whose holes are lost); `processVolume` under bun gives the same triangles, index for index,
//! so today's app draws the same lighter patch round Lake Erie (`w4-mrms-lakes.png`). A
//! fix belongs in `scene::geom::grid_cut` (or in cutting before the holes are punched), after parity.
//!
//! Deviations:
//! - Today's volumes are re-triangulated only when their scan changes, as here; the GPU buffers of
//!   a volume no longer drawn are freed at the swap (deck frees them with the layers).
//! - Polygons come as `f32` longitude and latitude, as the blob carried them, so the triangles are
//!   today's; their vertices go to the GPU in local Mercator (`f32` relative to the volume's
//!   corner) instead of deck's `f32` longitude and latitude, which places them at least as
//!   exactly.

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::DVec2;
use rustradar_core::geo::{effective_exaggeration, elevation};
use rustradar_core::model::{RadarBounds, RadarVolume};
use rustradar_core::num::{clamp01, js_round, js_string};
use rustradar_scene::camera::lng_lat_to_world;
use rustradar_scene::radar::{BAND_COUNT, BandPolygon, Coverage, RadarMesh, VolumeMesh, process_volume_polygons};

use crate::frame::{FrameSlot, power_of_two_scale};
use crate::jobs::{Job, JobPoll};
use crate::layers::terrain::cover::DrawnArea;
use crate::layers::{DrawCx, Init, PrepareCx};
use crate::pipelines::fill::{self, uniform_group, uniform_layout};
use crate::pipelines::{Desc, build};
use crate::shaders;
use crate::state::DrawState;

/// `shaders/radar.wgsl`.
pub const SHADER: &str = include_str!("../../shaders/radar.wgsl");

/// The bind group of the [`StyleUniform`].
pub const STYLE_GROUP: u32 = 2;

/// The ground layer's opacity (`measuredLayers`).
pub const GROUND_OPACITY: f64 = 0.7;

/// A height's opacity at full lift (`opacity = 0.18 * lift`).
pub const LEVEL_OPACITY: f64 = 0.18;

/// Feet in a kilometre (`FEET_PER_KM`, G:1155): heights go through `elevation`, which takes feet.
pub const FEET_PER_KM: f64 = 3280.84;

/// Heights at or below this (km) are left to the ground composite (`l.km > 1`).
pub const GROUND_LEVEL_KM: f64 = 1.0;

/// How far the stack stands up at `pitch` degrees (`liftFor`, G:1160): 0 looking straight down
/// (below 11.5°, by the rounding), 1 from 40° on, in tenths.
pub fn lift_for(pitch: f64) -> f64 {
    js_round(clamp01((pitch - 10.0) / 30.0) * 10.0) / 10.0
}

/// The radar's height scale (`Math.round(ex * 10) / 10` of `effectiveExaggeration(exaggeration,
/// zoom)`).
pub fn height_scale(exaggeration: f64, zoom: f64) -> f64 {
    js_round(effective_exaggeration(exaggeration, zoom) * 10.0) / 10.0
}

/// The height in metres a level of `km` is drawn at (`elevation(km * FEET_PER_KM, scale)`).
pub fn level_height(km: f64, scale: f64) -> f64 {
    elevation(km * FEET_PER_KM, scale)
}

/// Which volume this is: its source and scan time (`v.source === f.source && v.time === f.time`).
pub type VolumeKey = (String, i64);

/// One measured volume as the scene builder hands it over.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Volume {
    /// What the backend says of it (`radar_frame`'s `RadarVolume`): source, scan time, bounds,
    /// heights, the coverage mask's size.
    pub frame: RadarVolume,
    /// The ground composite's band polygons.
    pub ground: Vec<BandPolygon>,
    /// One list per height of `frame.levels`, in that order; a missing one is no echo.
    pub levels: Vec<Vec<BandPolygon>>,
    /// One byte per coverage cell, north row first: 1 where a radar sees.
    pub coverage: Vec<u8>,
}

impl Volume {
    pub fn key(&self) -> VolumeKey {
        (self.frame.source.clone(), self.frame.time)
    }
}

/// One of the backend's isoband polygons (`band`, then `[lon, lat]` rings, the outer one first,
/// none closed) as the meshing takes it, or `None` for one `parseVolumePolygons` drops: a band past
/// the palette, no rings, or an outer ring of fewer than 3 points. Holes of fewer than 3 points are
/// left out.
pub fn band_polygon(band: u8, rings: &[Vec<[f32; 2]>]) -> Option<BandPolygon> {
    let band = usize::from(band);
    if band >= BAND_COUNT || rings.first().is_none_or(|outer| outer.len() < 3) {
        return None;
    }
    let mut positions = Vec::with_capacity(rings.iter().map(|r| r.len() * 2).sum());
    let mut hole_indices = Vec::with_capacity(rings.len() - 1);
    for (i, ring) in rings.iter().enumerate() {
        if i > 0 {
            if ring.len() < 3 {
                continue;
            }
            hole_indices.push(positions.len() / 2);
        }
        positions.extend(ring.iter().flat_map(|&[lon, lat]| [f64::from(lon), f64::from(lat)]));
    }
    Some(BandPolygon { band, positions, hole_indices })
}

/// What the scene builder hands the measured layers (`radar::Input::measured`).
#[derive(Clone)]
pub struct Input {
    /// The volumes to draw, in the backend's order (`measured`); empty with the weather off.
    pub volumes: Vec<Arc<Volume>>,
    /// The height exaggeration setting (`exaggeration`).
    pub exaggeration: f64,
}

impl Default for Input {
    fn default() -> Self {
        Input { volumes: Vec::new(), exaggeration: 1.0 }
    }
}

impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // the polygons are megabytes: their keys say which volumes they are
        let keys: Vec<VolumeKey> = self.volumes.iter().map(|v| v.key()).collect();
        f.debug_struct("Input").field("volumes", &keys).field("exaggeration", &self.exaggeration).finish()
    }
}

/// One mesh on its way to the GPU: vertices in the volume's frame and three indices a triangle.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Part {
    pub vertices: Vec<fill::Vertex>,
    pub indices: Vec<u32>,
}

impl Part {
    fn byte_size(&self) -> usize {
        self.vertices.len() * size_of::<fill::Vertex>() + self.indices.len() * 4
    }
}

/// A height of a meshed volume.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshedLevel {
    /// The height, km (`f32` read as JavaScript reads it).
    pub km: f64,
    /// Every band of the height, flat (`polygonData(slices)`).
    pub slices: Part,
    /// Its walls, the third coordinate 0 at the foot and 1 at the top.
    pub walls: Option<Part>,
}

/// A volume as the worker pool hands it over (`VolumeMesh`, its vertices in the volume's frame).
#[derive(Debug, Clone, PartialEq)]
pub struct Meshed {
    pub key: VolumeKey,
    /// The frame's north-west corner in world Mercator (y north) and its scale ([`frame_of`]).
    pub anchor: DVec2,
    pub scale: f64,
    pub ground: Option<Part>,
    /// The heights with echo, in the volume's order.
    pub levels: Vec<MeshedLevel>,
    pub coverage: Coverage,
}

impl Meshed {
    fn byte_size(&self) -> usize {
        self.ground.as_ref().map_or(0, Part::byte_size)
            + self.levels.iter().map(|l| l.slices.byte_size() + l.walls.as_ref().map_or(0, Part::byte_size)).sum::<usize>()
            + self.coverage.mask.len()
    }
}

/// A volume's local Mercator frame: the north-west corner of its bounds in world Mercator and the
/// smallest power of two at least its extent, so its vertices have `u` east and `v` south in
/// `[0, 1]`.
pub fn frame_of(bounds: &RadarBounds) -> (DVec2, f64) {
    let [west, north] = lng_lat_to_world([bounds.west, bounds.north]);
    let [east, south] = lng_lat_to_world([bounds.east, bounds.south]);
    (DVec2::new(west, north), power_of_two_scale((east - west).max(north - south)))
}

/// A mesh's vertices in the frame of `anchor` and `scale`: `u` east and `v` south, from the mesh's
/// longitude and latitude (in `f64` through deck's Web Mercator); `h` is the mesh's third
/// coordinate when it has one (the walls' unit height), else 0; the colours as they are.
pub fn vertices(mesh: &RadarMesh, anchor: DVec2, scale: f64) -> Vec<fill::Vertex> {
    let size = mesh.size.max(2);
    mesh.positions
        .chunks_exact(size)
        .zip(mesh.colors.chunks_exact(4))
        .map(|(p, c)| {
            let [x, y] = lng_lat_to_world([f64::from(p[0]), f64::from(p[1])]);
            let h = if size > 2 { p[2] } else { 0.0 };
            fill::Vertex { position: [((x - anchor.x) / scale) as f32, ((anchor.y - y) / scale) as f32, h], color: [c[0], c[1], c[2], c[3]] }
        })
        .collect()
}

/// The meshing job, on the worker pool (a radar worker's `volume` job): `processVolume` on the
/// polygons, then the vertices into the volume's frame.
pub fn mesh(volume: &Volume) -> Meshed {
    let VolumeMesh { ground, levels, coverage } = process_volume_polygons(&volume.frame, &volume.ground, volume.coverage.clone(), &volume.levels);
    let (anchor, scale) = frame_of(&volume.frame.bounds);
    let part = |mesh: &RadarMesh| Part { vertices: vertices(mesh, anchor, scale), indices: mesh.indices.clone() };
    Meshed {
        key: volume.key(),
        anchor,
        scale,
        ground: ground.as_ref().map(part),
        levels: levels.iter().map(|l| MeshedLevel { km: l.km, slices: part(&l.slices), walls: l.walls.as_ref().map(part) }).collect(),
        coverage,
    }
}

/// What one draw of [`layers`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    /// The ground composite.
    Ground,
    /// The slices of the volume's height at this index.
    Slices(usize),
    /// The walls of the volume's height at this index.
    Walls(usize),
}

/// One of a volume's layers this frame (a `radarPolygons` call of `measuredLayers`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerSpec {
    pub what: What,
    /// The height in km (0 for the ground).
    pub km: f64,
    /// Multiplies the vertices' alpha.
    pub opacity: f64,
    /// Metres: the slices' height, the walls' top.
    pub z: f64,
    /// Metres: the walls' foot (0 otherwise).
    pub bottom: f64,
}

impl LayerSpec {
    /// Whether it is drawn with `RADAR_LIFTED` rather than `SURFACE_NO_CULL`.
    pub fn lifted(&self) -> bool {
        self.what != What::Ground
    }

    /// Today's layer id for the volume `key` (`<source>-ground-<time>`, `<source>-<km>-<time>`,
    /// `<source>-wall-<km>-<time>`).
    pub fn id(&self, (source, time): &VolumeKey) -> String {
        match self.what {
            What::Ground => format!("{source}-ground-{time}"),
            What::Slices(_) => format!("{source}-{}-{time}", js_string(self.km)),
            What::Walls(_) => format!("{source}-wall-{}-{time}", js_string(self.km)),
        }
    }
}

/// A volume's layers in draw order (`measuredLayers(volume, ex, lift)`): the ground, if it has
/// one, then while `lift > 0` each height above 1 km lowest first, its slices then its walls (if it
/// has any) down to the height below. `levels` is the volume's heights with echo: km and whether it
/// has walls.
pub fn layers(has_ground: bool, levels: &[(f64, bool)], exaggeration: f64, zoom: f64, pitch: f64) -> Vec<LayerSpec> {
    let mut out = Vec::new();
    if has_ground {
        out.push(LayerSpec { what: What::Ground, km: 0.0, opacity: GROUND_OPACITY, z: 0.0, bottom: 0.0 });
    }
    let lift = lift_for(pitch);
    if lift == 0.0 {
        return out;
    }
    // the lowest height is already in the ground composite
    let scale = height_scale(exaggeration, zoom);
    let mut bottom = 0.0;
    for (i, &(km, walls)) in levels.iter().enumerate().filter(|(_, (km, _))| *km > GROUND_LEVEL_KM) {
        let z = level_height(km, scale);
        let opacity = LEVEL_OPACITY * lift;
        out.push(LayerSpec { what: What::Slices(i), km, opacity, z, bottom: 0.0 });
        // walled down to the layer below, the lowest to the ground, so the stack has no gaps
        if walls {
            out.push(LayerSpec { what: What::Walls(i), km, opacity, z, bottom });
        }
        bottom = z;
    }
    out
}

/// `radar.wgsl`'s `Style`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct StyleUniform {
    /// Multiplies each vertex's RGBA (0–1): `(1, 1, 1, opacity)`.
    pub color: [f32; 4],
    /// Metres per unit of the vertices' third coordinate: `z − bottom` for walls.
    pub z_scale: f32,
    pub pad: [f32; 3],
}

impl StyleUniform {
    /// A layer's opacity and its walls' stretch.
    pub fn new(opacity: f64, z_scale: f64) -> Self {
        StyleUniform { color: [1.0, 1.0, 1.0, opacity as f32], z_scale: z_scale as f32, pad: [0.0; 3] }
    }
}

/// A draw's [`StyleUniform`] on the GPU (group 2).
pub struct Style {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
    value: StyleUniform,
}

impl Style {
    /// Writes `value` unless it is what the buffer holds.
    pub fn set(&mut self, queue: &wgpu::Queue, value: StyleUniform) {
        if value != self.value {
            queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&value));
            self.value = value;
        }
    }

    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(STYLE_GROUP, &self.group, &[]);
    }
}

/// The radar's two pipelines on `radar.wgsl` (`radarPolygons`): `SURFACE_NO_CULL` for the ground,
/// `RADAR_LIFTED` for the lifted slices and walls. Vertices are [`fill::Vertex`], meshes
/// [`fill::Mesh`]; one style layout serves both.
pub struct RadarPipelines {
    ground: wgpu::RenderPipeline,
    lifted: wgpu::RenderPipeline,
    style_layout: wgpu::BindGroupLayout,
}

impl RadarPipelines {
    pub fn new(init: &Init<'_>, label: &str) -> Self {
        let device = &init.gpu.device;
        let style_layout = uniform_layout(device, "radar style", size_of::<StyleUniform>());
        let module = shaders::module(device, label, SHADER);
        let pipeline = |state| {
            build(
                init,
                &Desc {
                    label,
                    module: &module,
                    vertex_entry: "vs",
                    fragment_entry: "fs",
                    buffers: &[Some(fill::Vertex::LAYOUT)],
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    state,
                    groups: &[&style_layout],
                },
            )
        };
        RadarPipelines { ground: pipeline(DrawState::SURFACE_NO_CULL), lifted: pipeline(DrawState::RADAR_LIFTED), style_layout }
    }

    /// A style for these pipelines.
    pub fn style(&self, device: &wgpu::Device, value: StyleUniform) -> Style {
        let (buffer, group) = uniform_group(device, "radar style", &self.style_layout, &value);
        Style { buffer, group, value }
    }

    /// Sets the ground's or the lifted layers' pipeline and `style`; then bind a frame and draw.
    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>, lifted: bool, style: &Style) {
        pass.set_pipeline(if lifted { &self.lifted } else { &self.ground });
        style.bind(pass);
    }
}

/// A height of a drawn volume on the GPU.
struct GpuLevel {
    km: f64,
    slices: fill::Mesh<fill::Vertex>,
    walls: Option<fill::Mesh<fill::Vertex>>,
}

/// A volume on the GPU.
struct Drawn {
    key: VolumeKey,
    anchor: DVec2,
    scale: f64,
    ground: Option<fill::Mesh<fill::Vertex>>,
    levels: Vec<GpuLevel>,
    coverage: Coverage,
}

impl Drawn {
    fn upload(device: &wgpu::Device, meshed: Meshed) -> Self {
        let mesh = |label: &str, part: &Part| fill::Mesh::new(device, label, &part.vertices, &part.indices);
        Drawn {
            ground: meshed.ground.as_ref().map(|p| mesh("radar ground", p)),
            levels: meshed
                .levels
                .iter()
                .map(|l| GpuLevel { km: l.km, slices: mesh("radar level", &l.slices), walls: l.walls.as_ref().map(|w| mesh("radar walls", w)) })
                .collect(),
            key: meshed.key,
            anchor: meshed.anchor,
            scale: meshed.scale,
            coverage: meshed.coverage,
        }
    }

    fn mesh(&self, what: What) -> Option<&fill::Mesh<fill::Vertex>> {
        match what {
            What::Ground => self.ground.as_ref(),
            What::Slices(i) => self.levels.get(i).map(|l| &l.slices),
            What::Walls(i) => self.levels.get(i).and_then(|l| l.walls.as_ref()),
        }
    }
}

/// A volume of the set being loaded.
enum Slot {
    /// Drawn already (the same source and time).
    Held(Arc<Drawn>),
    Meshing(Job<Meshed>),
    /// Meshed, waiting for the upload budget.
    Meshed(Box<Meshed>),
    Uploaded(Arc<Drawn>),
}

/// The set being loaded: the input's keys and a slot for each.
struct Loading {
    keys: Vec<VolumeKey>,
    slots: Vec<Slot>,
}

/// One draw of this frame.
struct Draw {
    volume: usize,
    what: What,
    lifted: bool,
    frame: FrameSlot,
    style: usize,
}

/// The measured volumes' layers (rows 5b of the layer table, after RainViewer's).
pub struct Measured {
    pipelines: RadarPipelines,
    /// The set drawn (today's `measured`).
    drawn: Vec<Arc<Drawn>>,
    /// Their coverages, in their order.
    coverages: Arc<Vec<Coverage>>,
    loading: Option<Loading>,
    /// A set whose meshing failed: not tried again until the input asks for another.
    failed: Option<Vec<VolumeKey>>,
    styles: Vec<Style>,
    draws: Vec<Draw>,
}

impl Measured {
    pub fn new(init: &Init<'_>) -> Self {
        Measured {
            pipelines: RadarPipelines::new(init, "radar measured"),
            drawn: Vec::new(),
            coverages: Arc::new(Vec::new()),
            loading: None,
            failed: None,
            styles: Vec::new(),
            draws: Vec::new(),
        }
    }

    /// The drawn volumes' coverages, in their order: where RainViewer stands back.
    pub fn coverages(&self) -> &Arc<Vec<Coverage>> {
        &self.coverages
    }

    /// The volumes drawn (for tests and logs).
    pub fn drawn(&self) -> Vec<VolumeKey> {
        self.drawn.iter().map(|d| d.key.clone()).collect()
    }

    /// How many draws this frame makes (for tests).
    pub fn draw_count(&self) -> usize {
        self.draws.len()
    }

    /// Whether a set is still being meshed or uploaded.
    pub fn pending(&self) -> bool {
        self.loading.is_some()
    }

    /// One frame's work: a new set started, meshes taken in and uploaded within the frame's
    /// budget, the sets swapped once the new one is in, and this frame's draws placed for its
    /// camera.
    pub fn prepare(&mut self, cx: &mut PrepareCx<'_>, input: &Input) {
        self.load(cx, input);
        self.place(cx, input.exaggeration);
    }

    fn load(&mut self, cx: &mut PrepareCx<'_>, input: &Input) {
        let wanted: Vec<VolumeKey> = input.volumes.iter().map(|v| v.key()).collect();
        let target = self.loading.as_ref().map_or_else(|| self.drawn.iter().map(|d| &d.key).collect::<Vec<_>>(), |l| l.keys.iter().collect());
        let fresh = target.len() != wanted.len() || target.iter().zip(&wanted).any(|(a, b)| *a != b);
        if fresh && self.failed.as_ref() != Some(&wanted) {
            self.failed = None;
            self.start(cx, input, wanted);
        }
        let Some(loading) = &mut self.loading else { return };
        let mut failed = false;
        let (device, budget) = (&cx.gpu.device, &mut *cx.budget);
        loading.slots = std::mem::take(&mut loading.slots)
            .into_iter()
            .map(|slot| {
                let slot = match slot {
                    Slot::Meshing(mut job) => match job.poll() {
                        JobPoll::Pending => Slot::Meshing(job),
                        JobPoll::Done(meshed) => Slot::Meshed(Box::new(meshed)),
                        JobPoll::Cancelled | JobPoll::Lost => {
                            failed = true;
                            Slot::Meshing(job)
                        }
                    },
                    other => other,
                };
                // one volume at a time, within what the frame may still upload
                match slot {
                    Slot::Meshed(meshed) if budget.take(meshed.byte_size()) => Slot::Uploaded(Arc::new(Drawn::upload(device, *meshed))),
                    other => other,
                }
            })
            .collect();
        if failed {
            // the old set stays (`console.warn("measured radar unavailable:", e)`)
            log::warn!("measured radar unavailable: a volume of {:?} could not be meshed", loading.keys);
            self.failed = self.loading.take().map(|l| l.keys);
            return;
        }
        if loading.slots.iter().all(|s| matches!(s, Slot::Held(_) | Slot::Uploaded(_))) {
            let Some(loading) = self.loading.take() else { return };
            self.drawn = loading
                .slots
                .into_iter()
                .map(|s| match s {
                    Slot::Held(d) | Slot::Uploaded(d) => d,
                    Slot::Meshing(_) | Slot::Meshed(_) => unreachable!("every slot is in"),
                })
                .collect();
            // RainViewer stands back where some service measures the weather in 3D
            self.coverages = Arc::new(self.drawn.iter().map(|d| d.coverage.clone()).collect());
        }
    }

    /// Starts loading `wanted` (the keys of `input.volumes`): what is drawn or already on its way
    /// is kept, the rest meshed on the pool.
    fn start(&mut self, cx: &PrepareCx<'_>, input: &Input, wanted: Vec<VolumeKey>) {
        let mut earlier: HashMap<VolumeKey, Slot> = HashMap::new();
        if let Some(old) = self.loading.take() {
            earlier.extend(old.keys.into_iter().zip(old.slots));
        }
        let slots = input
            .volumes
            .iter()
            .zip(&wanted)
            .map(|(volume, key)| {
                if let Some(drawn) = self.drawn.iter().find(|d| &d.key == key) {
                    return Slot::Held(Arc::clone(drawn));
                }
                if let Some(slot) = earlier.remove(key) {
                    return slot;
                }
                let volume = Arc::clone(volume);
                Slot::Meshing(cx.tiles.pool.spawn(0.0, move |_| mesh(&volume)))
            })
            .collect();
        self.loading = Some(Loading { keys: wanted, slots });
    }

    /// This frame's draws and their styles, for this frame's camera.
    fn place(&mut self, cx: &mut PrepareCx<'_>, exaggeration: f64) {
        self.draws.clear();
        let view = cx.view.view;
        for (v, drawn) in self.drawn.iter().enumerate() {
            let levels: Vec<(f64, bool)> = drawn.levels.iter().map(|l| (l.km, l.walls.is_some())).collect();
            for spec in layers(drawn.ground.is_some(), &levels, exaggeration, view.zoom, view.pitch) {
                let (z_offset, z_scale) = match spec.what {
                    What::Ground => (0.0, 0.0),
                    What::Slices(_) => (spec.z, 0.0),
                    What::Walls(_) => (spec.bottom, spec.z - spec.bottom),
                };
                let value = StyleUniform::new(spec.opacity, z_scale);
                let style = self.draws.len();
                if style == self.styles.len() {
                    self.styles.push(self.pipelines.style(&cx.gpu.device, value));
                } else {
                    self.styles[style].set(&cx.gpu.queue, value);
                }
                let frame = cx.frames.push(&cx.view.local(drawn.anchor, drawn.scale, z_offset));
                self.draws.push(Draw { volume: v, what: spec.what, lifted: spec.lifted(), frame, style });
            }
        }
    }

    /// Where the drawn volumes' ground composites lie (item G3: what is draped over the 3D terrain),
    /// world Mercator: the union of their frames, none without a ground.
    pub fn ground_area(&self) -> Option<DrawnArea> {
        self.drawn.iter().filter(|d| d.ground.is_some()).fold(None, |area, d| {
            let frame = DrawnArea::world([d.anchor.x, d.anchor.y - d.scale], [d.anchor.x + d.scale, d.anchor.y], 1.0);
            DrawnArea::either(area, Some(frame))
        })
    }

    /// Records this frame's draws.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        self.draw_parts(pass, cx, true, true);
    }

    /// Records this frame's draws of the ground composites when `ground`, of the heights above
    /// them when `lifted` (on the 3D terrain deck drapes the grounds into the covers, item G3).
    pub fn draw_parts(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, ground: bool, lifted: bool) {
        for draw in &self.draws {
            let wanted = if draw.lifted { lifted } else { ground };
            if !wanted {
                continue;
            }
            let Some(mesh) = self.drawn.get(draw.volume).and_then(|d| d.mesh(draw.what)) else { continue };
            self.pipelines.bind(pass, draw.lifted, &self.styles[draw.style]);
            cx.bind_frame(pass, draw.frame);
            mesh.draw(pass);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use rustradar_core::assert_close;
    use rustradar_core::geo::Basemap;
    use rustradar_core::model::f32_as_js;
    use rustradar_scene::camera::{ViewState, world_to_lng_lat};
    use rustradar_scene::radar::band_color;
    use rustradar_scene::radar::mesh::parse_volume_polygons;

    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::Gpu;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{DirTiles, TileFetcher};
    use crate::layers::{Layer, earth, vector_map};
    use crate::renderer::{MapRenderer, SceneInput};

    fn recorded_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/radar/mrms")
    }

    fn recorded_file(name: &str) -> Vec<u8> {
        let path = recorded_dir().join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// The number after `"key":` in `json` (the map has no JSON reader; `volume.json` is flat).
    fn number(json: &str, key: &str) -> f64 {
        let at = json.find(&format!("\"{key}\":")).unwrap_or_else(|| panic!("no {key}")) + key.len() + 3;
        let text: String = json[at..].chars().skip_while(|c| c.is_whitespace()).take_while(|c| c.is_ascii_digit() || "-.eE+".contains(*c)).collect();
        text.parse().unwrap_or_else(|e| panic!("{key}: {text:?} {e}"))
    }

    /// The recorded volume's `RadarVolume` (X1's `volume.json`).
    fn recorded_frame() -> RadarVolume {
        let json = String::from_utf8(recorded_file("volume.json")).unwrap();
        let list = &json[json.find("\"levels\"").unwrap()..];
        let list = &list[list.find('[').unwrap() + 1..list.find(']').unwrap()];
        RadarVolume {
            source: "mrms".into(),
            time: number(&json, "time") as i64,
            bounds: RadarBounds { west: number(&json, "west"), south: number(&json, "south"), east: number(&json, "east"), north: number(&json, "north") },
            levels: list.split(',').map(|v| v.trim().parse().unwrap()).collect(),
            coverage_width: number(&json, "coverageWidth") as u32,
            coverage_height: number(&json, "coverageHeight") as u32,
        }
    }

    /// A recorded blob read as the backend's polygons are: band, then `[lon, lat]` rings
    /// (`backend::radar::encode`'s layout).
    fn shapes(blob: &[u8]) -> Vec<(u8, Vec<Vec<[f32; 2]>>)> {
        let word = |at: usize| u32::from_le_bytes(blob[at..at + 4].try_into().unwrap()) as usize;
        let float = |at: usize| f32::from_le_bytes(blob[at..at + 4].try_into().unwrap());
        let mut at = 4;
        let mut out = Vec::new();
        for _ in 0..word(0) {
            let (band, count) = (blob[at], word(at + 4));
            at += 8;
            let mut rings = Vec::new();
            for _ in 0..count {
                let n = word(at);
                at += 4;
                rings.push((0..n).map(|i| [float(at + 8 * i), float(at + 8 * i + 4)]).collect());
                at += 8 * n;
            }
            out.push((band, rings));
        }
        assert_eq!(at, blob.len());
        out
    }

    /// The recorded MRMS volume (X1) as the scene builder hands it over: the backend's polygons
    /// through [`band_polygon`].
    fn recorded() -> Arc<Volume> {
        let frame = recorded_frame();
        let polygons = |name: &str| shapes(&recorded_file(name)).iter().filter_map(|(band, rings)| band_polygon(*band, rings)).collect::<Vec<_>>();
        let levels = frame.levels.iter().map(|km| polygons(&format!("{km}.bin"))).collect();
        Arc::new(Volume { ground: polygons("ground.bin"), levels, coverage: recorded_file("coverage.bin"), frame })
    }

    /// A small storm: `size` degrees square at `[lon, lat]` (its south-west corner), band 9 on the
    /// ground and at each of `levels` km, its mask all seen.
    fn tower(source: &str, time: i64, [lon, lat]: [f64; 2], size: f64, levels: &[f32]) -> Arc<Volume> {
        let square = BandPolygon { band: 9, positions: vec![lon, lat, lon + size, lat, lon + size, lat + size, lon, lat + size], hole_indices: Vec::new() };
        let bounds = RadarBounds { west: lon - 1.0, south: lat - 1.0, east: lon + size + 1.0, north: lat + size + 1.0 };
        Arc::new(Volume {
            frame: RadarVolume { source: source.into(), time, bounds, levels: levels.to_vec(), coverage_width: 2, coverage_height: 2 },
            ground: vec![square.clone()],
            levels: levels.iter().map(|_| vec![square.clone()]).collect(),
            coverage: vec![1; 4],
        })
    }

    #[test]
    fn the_style_is_the_shader_s() {
        let (size, members) = crate::pipelines::fill::tests::wgsl_members(SHADER, "Style");
        assert_eq!(size as usize, size_of::<StyleUniform>());
        let names: Vec<(&str, u32)> = members.iter().map(|(n, o)| (n.as_str(), *o)).collect();
        assert_eq!(names, [("color", 0), ("z_scale", 16), ("pad0", 20), ("pad1", 24), ("pad2", 28)]);
        assert_eq!(std::mem::offset_of!(StyleUniform, z_scale), 16);
        assert_eq!(StyleUniform::new(0.18, 2500.0), StyleUniform { color: [1.0, 1.0, 1.0, 0.18], z_scale: 2500.0, pad: [0.0; 3] });
    }

    #[test]
    fn lift_and_height_scale_are_today_s() {
        // 0 looking down and to 11.5°, by the rounding; tenths up to 1 at 40°
        let lifts: Vec<f64> = [0.0, 10.0, 11.4, 11.5, 25.0, 39.0, 40.0, 60.0, 85.0].map(lift_for).to_vec();
        assert_eq!(lifts, [0.0, 0.0, 0.0, 0.1, 0.5, 1.0, 1.0, 1.0, 1.0]);
        assert_eq!(lift_for(17.5), 0.3);
        // the exaggeration fades out between zoom 6 and 11, in tenths
        assert_eq!(height_scale(10.0, 4.0), 10.0);
        assert_eq!(height_scale(10.0, 8.5), 5.5);
        assert_eq!(height_scale(10.0, 7.33), 7.6);
        assert_eq!(height_scale(10.0, 11.0), 1.0);
        assert_eq!(height_scale(1.0, 3.0), 1.0);
        // km → feet → metres: not quite a thousand to the km
        assert_close!(level_height(2.0, 1.0), 2.0 * 3280.84 * 0.3048, 12);
        assert_close!(level_height(4.5, 10.0), 4.5 * 3280.84 * 0.3048 * 10.0, 12);
        assert_eq!(level_height(-1.0, 3.0), 0.0);
    }

    /// `measuredLayers`: the ground at 0.7, then while tilted each height above 1 km, its slices and
    /// its walls from the height below, at 0.18 · lift.
    #[test]
    fn a_volume_s_layers_come_in_today_s_order() {
        let key: VolumeKey = ("mrms".into(), 1_794_446_561);
        let levels = [(1.0, true), (2.0, true), (3.0, false), (4.5, true)];
        let flat = layers(true, &levels, 10.0, 5.0, 0.0);
        assert_eq!(flat.iter().map(|l| l.id(&key)).collect::<Vec<_>>(), ["mrms-ground-1794446561"]);
        assert_eq!((flat[0].opacity, flat[0].z, flat[0].lifted()), (0.7, 0.0, false));
        let tilted = layers(true, &levels, 10.0, 5.0, 45.0);
        let ids: Vec<String> = tilted.iter().map(|l| l.id(&key)).collect();
        assert_eq!(
            ids,
            ["mrms-ground-1794446561", "mrms-2-1794446561", "mrms-wall-2-1794446561", "mrms-3-1794446561", "mrms-4.5-1794446561", "mrms-wall-4.5-1794446561"]
        );
        let z = |km: f64| level_height(km, 10.0);
        // the first drawn height's walls run to the ground; the next ones to the height below,
        // whether or not it had walls
        let walls: Vec<(f64, f64)> = tilted.iter().filter(|l| matches!(l.what, What::Walls(_))).map(|l| (l.bottom, l.z)).collect();
        assert_eq!(walls, [(0.0, z(2.0)), (z(3.0), z(4.5))]);
        assert_eq!(tilted.iter().map(|l| l.what).collect::<Vec<_>>()[1..], [What::Slices(1), What::Walls(1), What::Slices(2), What::Slices(3), What::Walls(3)]);
        assert!(tilted[1..].iter().all(|l| l.lifted() && l.opacity == LEVEL_OPACITY));
        // half lifted, half as opaque; zoomed in, true heights
        let half = layers(false, &levels, 10.0, 11.0, 25.0);
        assert_eq!(half[0].what, What::Slices(1));
        assert_close!(half[0].opacity, 0.09, 15);
        assert_eq!(half[0].z, level_height(2.0, 1.0));
        assert!(layers(false, &levels, 10.0, 5.0, 5.0).is_empty());
    }

    /// The backend's polygons through [`band_polygon`] are what `parseVolumePolygons` reads from the
    /// same isobands as a blob, on every recorded layer; and it drops what that drops.
    #[test]
    fn band_polygons_are_what_parse_volume_polygons_reads() {
        let frame = recorded_frame();
        for name in std::iter::once("ground".to_string()).chain(frame.levels.iter().map(|km| km.to_string())) {
            let blob = recorded_file(&format!("{name}.bin"));
            let ours: Vec<BandPolygon> = shapes(&blob).iter().filter_map(|(band, rings)| band_polygon(*band, rings)).collect();
            let theirs = parse_volume_polygons(&blob).unwrap();
            assert!(!ours.is_empty(), "{name}");
            assert_eq!(ours, theirs, "{name}");
        }
        let triangle = vec![[0.0f32, 0.0], [1.0, 0.0], [0.0, 1.0]];
        assert!(band_polygon(14, std::slice::from_ref(&triangle)).is_none());
        assert!(band_polygon(3, &[]).is_none());
        assert!(band_polygon(3, &[triangle[..2].to_vec()]).is_none());
        let holed = band_polygon(13, &[triangle.clone(), triangle[..2].to_vec(), triangle.clone()]).unwrap();
        assert_eq!((holed.band, holed.positions.len(), holed.hole_indices), (13, 12, vec![3]));
    }

    /// The pool's job is `processVolume`'s meshes, in the volume's frame: the blob path's triangles
    /// index for index, every vertex back on its longitude and latitude, the coverage and its key.
    #[test]
    fn meshing_is_process_volume_s_in_the_volume_s_frame() {
        let volume = recorded();
        let meshed = mesh(&volume);
        let blobs: Vec<Vec<u8>> = volume.frame.levels.iter().map(|km| recorded_file(&format!("{km}.bin"))).collect();
        let theirs = rustradar_scene::radar::process_volume(&volume.frame, &recorded_file("ground.bin"), volume.coverage.clone(), &blobs.iter().map(Vec::as_slice).collect::<Vec<_>>()).unwrap();
        assert_eq!(meshed.key, ("mrms".to_string(), 1_794_446_561));
        assert_eq!(meshed.coverage, theirs.coverage);
        assert!(meshed.coverage.key.starts_with("mrms:-130,20,-60,55:700x350:"), "{}", meshed.coverage.key);
        // every MRMS height has echo aloft at this scan
        assert_eq!(meshed.levels.iter().map(|l| l.km).collect::<Vec<_>>(), volume.frame.levels.iter().map(|&km| f32_as_js(km)).collect::<Vec<_>>());
        let (anchor, scale) = frame_of(&volume.frame.bounds);
        assert_eq!((meshed.anchor, meshed.scale, scale), (anchor, 128.0, 128.0));
        let check = |ours: &Part, mesh: &RadarMesh, label: &str| {
            assert_eq!(ours.indices, mesh.indices, "{label}");
            assert_eq!(ours.vertices.len(), mesh.vertex_count(), "{label}");
            for (k, v) in ours.vertices.iter().enumerate() {
                let p = &mesh.positions[k * mesh.size..];
                assert_eq!(v.color[..], mesh.colors[k * 4..k * 4 + 4], "{label}");
                assert!((0.0..=1.0).contains(&v.position[0]) && (0.0..=1.0).contains(&v.position[1]), "{label} {v:?}");
                let world = anchor + DVec2::new(f64::from(v.position[0]), -f64::from(v.position[1])) * scale;
                let [lon, lat] = world_to_lng_lat(world.to_array());
                assert!((lon - f64::from(p[0])).abs() < 2e-5 && (lat - f64::from(p[1])).abs() < 2e-5, "{label}: {lon},{lat} for {p:?}");
                assert_eq!(v.position[2], if mesh.size == 3 { p[2] } else { 0.0 }, "{label}");
            }
        };
        check(meshed.ground.as_ref().unwrap(), theirs.ground.as_ref().unwrap(), "ground");
        assert!(meshed.ground.as_ref().unwrap().indices.len() > 10_000);
        for (ours, theirs) in meshed.levels.iter().zip(&theirs.levels) {
            check(&ours.slices, &theirs.slices, &format!("{} km", ours.km));
            check(ours.walls.as_ref().unwrap(), theirs.walls.as_ref().unwrap(), &format!("{} km walls", ours.km));
        }
    }

    /// What a test layer saw after each prepare: the volumes drawn, their coverages' keys, the
    /// draws, and whether a set was loading.
    #[derive(Debug, Default, Clone, PartialEq)]
    struct Seen {
        drawn: Vec<VolumeKey>,
        coverages: Vec<String>,
        draws: usize,
        pending: bool,
    }

    /// [`Measured`] on its own as a layer, reporting what it holds.
    struct Probe {
        measured: Measured,
        seen: Rc<RefCell<Seen>>,
    }

    impl Layer for Probe {
        fn name(&self) -> &'static str {
            "measured probe"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            let scene = cx.scene;
            self.measured.prepare(cx, &scene.radar.measured);
            *self.seen.borrow_mut() = Seen {
                drawn: self.measured.drawn(),
                coverages: self.measured.coverages().iter().map(|c| c.key.clone()).collect(),
                draws: self.measured.draw_count(),
                pending: self.measured.pending(),
            };
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            self.measured.draw(pass, cx);
        }

        fn pending(&self) -> bool {
            self.measured.pending()
        }
    }

    /// Prepares `scene` until nothing is pending (at most 5 s): what the probe saw first and last.
    fn settle(gpu: &Gpu, renderer: &mut MapRenderer, seen: &Rc<RefCell<Seen>>, scene: &SceneInput) -> (Seen, Seen) {
        renderer.update(scene.clone());
        renderer.prepare(gpu, 0.0);
        let first = seen.borrow().clone();
        let started = std::time::Instant::now();
        while renderer.pending() && started.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(5));
            renderer.prepare(gpu, 0.0);
        }
        (first, seen.borrow().clone())
    }

    /// `refreshMeasured`: a set drawn stays until the whole new one is in, then both swap with the
    /// coverages; a volume already drawn is kept; an empty set clears everything at once.
    #[test]
    fn a_new_set_replaces_the_drawn_one_once_it_is_in() {
        let Some(gpu) = gpu() else { return };
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        let seen = Rc::new(RefCell::new(Seen::default()));
        let probe = Probe { measured: Measured::new(&renderer.init(&gpu)), seen: Rc::clone(&seen) };
        renderer.add_layer(Box::new(probe));
        let view = ViewState { longitude: -95.5, latitude: 38.5, zoom: 5.0, pitch: 45.0, ..ViewState::INITIAL };
        let mut scene = SceneInput::new(view, [200.0, 200.0], 1.0, Basemap::Dark);
        let a = tower("mrms", 100, [-96.0, 38.0], 0.5, &[1.0, 2.0, 4.0]);
        let b = tower("dwd", 200, [10.0, 50.0], 0.5, &[1.5, 3.0]);
        let key = |v: &Arc<Volume>| v.key();

        scene.radar.measured = Input { volumes: vec![Arc::clone(&a)], exaggeration: 10.0 };
        let (first, last) = settle(&gpu, &mut renderer, &seen, &scene);
        assert!(first.pending && first.drawn.is_empty() && first.coverages.is_empty(), "{first:?}");
        assert_eq!(last.drawn, [key(&a)]);
        assert!(!last.pending && last.coverages.len() == 1 && last.coverages[0].starts_with("mrms:-97,37,-94.5,39.5:2x2:"), "{last:?}");
        // pitched 45°: the ground, then 2 and 4 km, slices and walls
        assert_eq!(last.draws, 5);

        scene.radar.measured.volumes.push(Arc::clone(&b));
        let (first, last) = settle(&gpu, &mut renderer, &seen, &scene);
        assert_eq!(first.drawn, [key(&a)], "the old set is drawn while the new one meshes");
        assert!(first.pending);
        assert_eq!(last.drawn, [key(&a), key(&b)]);
        assert_eq!(last.coverages.len(), 2);
        assert_eq!(last.draws, 5 + 5);
        // the same set again: nothing to load
        let (first, _) = settle(&gpu, &mut renderer, &seen, &scene);
        assert!(!first.pending && first.drawn.len() == 2);

        // a new scan of one source replaces it in place; looking straight down, only the grounds
        let newer = tower("mrms", 101, [-96.0, 38.0], 0.5, &[1.0, 2.0, 4.0]);
        scene.radar.measured.volumes[0] = Arc::clone(&newer);
        scene.view.pitch = 0.0;
        let (first, last) = settle(&gpu, &mut renderer, &seen, &scene);
        assert_eq!(first.drawn, [key(&a), key(&b)]);
        assert_eq!(last.drawn, [key(&newer), key(&b)]);
        assert_eq!(last.draws, 2);

        scene.radar.measured.volumes.clear();
        let (first, _) = settle(&gpu, &mut renderer, &seen, &scene);
        assert_eq!(first, Seen::default());
    }

    fn shots_dir() -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    /// The dark map of `view` on a `size` canvas from the committed tiles, with `measured` on it,
    /// saved as `name` when it has one.
    fn dark_shot(gpu: &Gpu, view: ViewState, size: [f64; 2], measured: Input, name: Option<&str>) -> Image {
        let mut scene = SceneInput::new(view, size, 1.0, Basemap::Dark);
        scene.earth = earth::Input::SHOWN;
        scene.vector_map = vector_map::Input::SHOWN;
        scene.radar.measured = measured;
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let tiles: Arc<dyn TileFetcher> = Arc::new(DirTiles::fixtures());
        renderer.set_tiles(tiles, None);
        let image = render_with(gpu, &mut renderer, &scene, 1.0).expect("rendered");
        assert!(!renderer.pending(), "still loading: {:?}", renderer.pending_layers());
        if let Some(name) = name {
            image.save_png(&shots_dir().join(name)).expect("the shot written");
        }
        image
    }

    /// deck's blending of `color` at `opacity` over `under` (gamma space, as the map pass blends).
    fn over(under: [u8; 4], [r, g, b, a]: [u8; 4], opacity: f64) -> [f64; 3] {
        let alpha = f64::from(a) / 255.0 * opacity;
        [r, g, b].map(f64::from).iter().zip(&under).map(|(c, u)| f64::from(*u) + (c - f64::from(*u)) * alpha).collect::<Vec<_>>().try_into().unwrap()
    }

    /// Whether the 3 × 3 pixels round `(x, y)` are one colour in `image` (no edge there).
    fn uniform(image: &Image, x: u32, y: u32) -> bool {
        let c = image.pixel(x, y);
        (x - 1..=x + 1).all(|i| (y - 1..=y + 1).all(|j| image.pixel(i, j) == c))
    }

    /// The recorded MRMS scan over the dark globe, looking straight down at the United States
    /// (the whole of it, then closer over the Great Lakes): only the ground composite, every pixel
    /// it covers one of the NWS bands at 70 % over the map, the bands from light rain (cyan) to
    /// heavy (red) all there, nothing outside MRMS's bounds.
    #[test]
    fn the_recorded_scan_lies_on_the_globe_in_the_nws_colours_at_70_percent() {
        let Some(gpu) = gpu() else { return };
        let size = [1100.0, 720.0];
        let input = Input { volumes: vec![recorded()], exaggeration: 10.0 };
        let whole = ViewState { longitude: -96.0, latitude: 38.0, zoom: 2.4, ..ViewState::INITIAL };
        let lakes = ViewState { longitude: -82.0, latitude: 41.0, zoom: 5.3, ..ViewState::INITIAL };
        // (the whole country is too small at 2.4 for many pixels to lie well inside a band)
        for (view, name, at_least) in [(whole, "w4-mrms-globe.png", 0), (lakes, "w4-mrms-lakes.png", 20_000)] {
            let base = dark_shot(&gpu, view, size, Input::default(), None);
            let radar = dark_shot(&gpu, view, size, input.clone(), Some(name));
            let viewport = &ViewFrame::new(&view, size, 1.0, 0.0).viewport;
            let (mut changed, mut matched, mut outside) = (0usize, 0usize, 0usize);
            let mut bands = [0usize; BAND_COUNT];
            let mut doubled: Vec<[f64; 2]> = Vec::new();
            for y in 1..radar.height - 1 {
                for x in 1..radar.width - 1 {
                    let (before, after) = (base.pixel(x, y), radar.pixel(x, y));
                    if before == after {
                        continue;
                    }
                    changed += 1;
                    let [lon, lat] = viewport.unproject([f64::from(x) + 0.5, f64::from(y) + 0.5], 0.0);
                    if !(-130.5..=-59.5).contains(&lon) || !(19.5..=55.5).contains(&lat) {
                        outside += 1;
                    }
                    if !uniform(&radar, x, y) || !uniform(&base, x, y) {
                        continue;
                    }
                    let off = |want: [f64; 3]| (0..3).map(|i| (want[i] - f64::from(after[i])).abs()).fold(0.0, f64::max);
                    let best = (0..BAND_COUNT).map(|band| (off(over(before, band_color(band), GROUND_OPACITY)), band)).min_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
                    if best.0 <= 1.5 {
                        matched += 1;
                        bands[best.1] += 1;
                        continue;
                    }
                    // today's meshing covers a few places twice (see the module's notes): one band
                    // over another there
                    let twice = (0..BAND_COUNT * BAND_COUNT)
                        .map(|k| {
                            let lower = over(before, band_color(k / BAND_COUNT), GROUND_OPACITY).map(|c| c.round() as u8);
                            off(over([lower[0], lower[1], lower[2], 255], band_color(k % BAND_COUNT), GROUND_OPACITY))
                        })
                        .fold(f64::MAX, f64::min);
                    assert!(twice <= 2.5, "{name} ({x}, {y}) {after:?} over {before:?}: band {} off by {}, two by {twice}", best.1, best.0);
                    doubled.push([lon, lat]);
                }
            }
            eprintln!("{name}: {changed} pixels changed, {matched} inside a band; by band {bands:?}; {} covered twice", doubled.len());
            if let Some(first) = doubled.first() {
                let range = doubled.iter().fold([*first, *first], |[lo, hi], p| [[lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]]);
                eprintln!("{name}: covered twice within {range:?}");
            }
            assert_eq!(outside, 0, "{name}");
            assert!(changed > 5_000, "{name}: {changed}");
            if at_least > 0 {
                assert!(matched > at_least, "{name}: {matched}");
                assert!(bands[..7].iter().all(|&n| n > 0), "{name}: {bands:?}");
            }
        }
    }

    /// Tilted, the recorded scan stands up: the heights above 1 km stacked over the ground at
    /// 0.18 each with their walls, so the storm cores over the Ohio Valley grow columns; straight
    /// down the same scene is the ground alone, to the pixel.
    #[test]
    fn the_recorded_scan_stands_up_when_tilted() {
        let Some(gpu) = gpu() else { return };
        let size = [1100.0, 720.0];
        let input = Input { volumes: vec![recorded()], exaggeration: 10.0 };
        let mut ground_only = input.clone();
        Arc::make_mut(&mut ground_only.volumes[0]).levels.iter_mut().for_each(Vec::clear);
        let tilted = ViewState { longitude: -86.0, latitude: 37.0, zoom: 5.5, pitch: 55.0, bearing: 20.0, position: None };
        let flat = ViewState { pitch: 0.0, ..tilted };
        let differ = |a: &Image, b: &Image| -> usize {
            (0..a.height).step_by(2).flat_map(|y| (0..a.width).step_by(2).map(move |x| (x, y))).filter(|&(x, y)| a.pixel(x, y) != b.pixel(x, y)).count()
        };
        let up = dark_shot(&gpu, tilted, size, input.clone(), Some("w4-mrms-tilted.png"));
        let up_ground = dark_shot(&gpu, tilted, size, ground_only.clone(), Some("w4-mrms-tilted-ground.png"));
        let base = dark_shot(&gpu, tilted, size, Input::default(), None);
        let (stack, ground) = (differ(&up, &up_ground), differ(&up_ground, &base));
        eprintln!("MRMS tilted: the ground changes {ground} pixels (of a quarter's sample), the heights {stack} more");
        assert!(ground > 5_000 && stack > 2_000, "{ground}, {stack}");
        // the columns rise off their footprints: much of the stack is drawn where the ground
        // radar is not
        let beyond = (0..up.height)
            .step_by(2)
            .flat_map(|y| (0..up.width).step_by(2).map(move |x| (x, y)))
            .filter(|&(x, y)| up.pixel(x, y) != up_ground.pixel(x, y) && up_ground.pixel(x, y) == base.pixel(x, y))
            .count();
        assert!(beyond * 5 > stack, "{beyond} of {stack} off the ground's footprint");
        // straight down: no lifted layer at all
        let down = dark_shot(&gpu, flat, size, input, Some("w4-mrms-down.png"));
        let down_ground = dark_shot(&gpu, flat, size, ground_only, None);
        assert_eq!(differ(&down, &down_ground), 0);
    }

    /// A small tower of band 9 (red) pitched 60°: its top slice is drawn at its height (red just
    /// below the projected north edge of the 4 km slice, none just above it), and walls join it to
    /// the slice below (red midway between the two heights over the tower's middle, where no slice
    /// reaches); at pitch 5 only the ground is drawn.
    #[test]
    fn slices_stand_at_their_heights_with_walls_between() {
        let Some(gpu) = gpu() else { return };
        let size = [600.0, 600.0];
        let [lon, lat, side] = [-95.55, 38.45, 0.1];
        let input = Input { volumes: vec![tower("mrms", 1, [lon, lat], side, &[1.0, 2.0, 4.0])], exaggeration: 10.0 };
        let view = ViewState { longitude: lon + side / 2.0, latitude: lat + side / 2.0, zoom: 7.0, pitch: 60.0, bearing: 0.0, position: None };
        let image = dark_shot(&gpu, view, size, input.clone(), Some("w4-tower.png"));
        let base = dark_shot(&gpu, view, size, Input::default(), None);
        let viewport = &ViewFrame::new(&view, size, 1.0, 0.0).viewport;
        let scale = height_scale(10.0, view.zoom);
        assert_eq!(scale, 8.2);
        let (z2, z4) = (level_height(2.0, scale), level_height(4.0, scale));
        let at = |lon: f64, lat: f64, h: f64| {
            let [x, y, _] = viewport.project([lon, lat, h]);
            (x.round() as u32, y.round() as u32)
        };
        let red = |(x, y): (u32, u32)| {
            let (p, b) = (image.pixel(x, y), base.pixel(x, y));
            i32::from(p[0]) - i32::from(b[0]) > 20 && p[1] <= b[1] && p[2] <= b[2]
        };
        // the 4 km slice's far (north) edge, middle: the top of the tower on the screen
        let (x, y) = at(lon + side / 2.0, lat + side, z4);
        assert!(red((x, y + 3)), "below the top edge at ({x}, {y}): {:?}", image.pixel(x, y + 3));
        assert!(!red((x, y - 4)), "above the top edge at ({x}, {y}): {:?}", image.pixel(x, y - 4));
        // midway up between 2 and 4 km over the middle: only walls are there
        let mid = at(lon + side / 2.0, lat + side / 2.0, (z2 + z4) / 2.0);
        let slices = [at(lon + side / 2.0, lat, z2), at(lon + side / 2.0, lat + side, z2), at(lon + side / 2.0, lat, z4), at(lon + side / 2.0, lat + side, z4)];
        // (the slices' screen spans in that column do not reach it)
        let (top2, bottom2) = (slices[1].1.min(slices[0].1), slices[1].1.max(slices[0].1));
        let (top4, bottom4) = (slices[3].1.min(slices[2].1), slices[3].1.max(slices[2].1));
        assert!(mid.1 > bottom4 + 2 && mid.1 + 2 < top2, "{mid:?} between {top4}..{bottom4} and {top2}..{bottom2}");
        assert!(red(mid), "midway at {mid:?}: {:?} over {:?}", image.pixel(mid.0, mid.1), base.pixel(mid.0, mid.1));
        // nearly straight down: the ground alone, under the tower
        let low = ViewState { pitch: 5.0, ..view };
        let image = dark_shot(&gpu, low, size, input, None);
        let base = dark_shot(&gpu, low, size, Input::default(), None);
        let viewport = &ViewFrame::new(&low, size, 1.0, 0.0).viewport;
        let [x, y, _] = viewport.project([lon + side / 2.0, lat + side / 2.0, 0.0]);
        let (x, y) = (x.round() as u32, y.round() as u32);
        let want = over(base.pixel(x, y), band_color(9), GROUND_OPACITY);
        let got = image.pixel(x, y);
        assert!((0..3).all(|i| (f64::from(got[i]) - want[i]).abs() <= 1.5), "{got:?} against {want:?}");
        let [x, y, _] = viewport.project([lon + side / 2.0, lat + side * 1.5, level_height(4.0, height_scale(10.0, low.zoom))]);
        assert_eq!(image.pixel(x.round() as u32, y.round() as u32), base.pixel(x.round() as u32, y.round() as u32));
    }
}
