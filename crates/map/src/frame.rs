//! The per-frame globals uniform and the per-draw [`Frame`] (plan 1.4.2): what `lib/project.wgsl`
//! reads, worked out in f64 from `scene::camera` and handed to the GPU as small f32 values.
//!
//! - [`ViewFrame::new`] makes the frame's [`Globals`]: the view-projection for common offsets, the
//!   camera of the render frame, the projection mode, the centre's latitude terms, the viewport
//!   size, the map scale and the time.
//! - [`ViewFrame::local`] and [`ViewFrame::tile`] make the [`Frame`] of one static mesh in local
//!   Mercator: its anchor minus the view centre (hi + lo f32) and its scale. Frames change with the
//!   view, so layers push theirs into the [`FrameArena`] every frame.
//! - [`ViewFrame::common_offset`] places a dynamic instance (aircraft, ship, train, marker, label).
//!
//! How this differs from the plan's sketch of the flat path (`frame_matrix = view_proj ·
//! translate(anchor − centre) · scale` in f64): a single matrix rounds the anchor's offset to f32 and
//! then cancels it against `u · scale` in f32, about 0.016 px at zoom 23 for the tile next to the
//! centre (z14, overzoomed); and it scales heights at one latitude for the whole mesh. Here the
//! offset is added as hi + lo after an exact `u · scale`, and heights use the vertex's own latitude
//! (deck's `projectPosition`, which `scene::camera` reproduces). Both paths share `(u, v)`, so one
//! tile cache serves the globe and the flat map.

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use glam::{DMat4, DVec2, DVec3, Vec3};
use rustradar_scene::camera::{self, GLOBE_RADIUS, ViewState, Viewport, WORLD_SIZE, lng_lat_to_world};
use rustradar_scene::tiles::TileIndex;

/// `globals.mode` on the globe (zoom ≤ 12).
pub const MODE_GLOBE: u32 = 0;
/// `globals.mode` on the flat map (zoom > 12).
pub const MODE_FLAT: u32 = 1;
/// The globe's render frame is turned to the view's latitude within this many degrees, where
/// Mercator y is finite (deck's shaders clamp latitude to ±89.9 too). Only a view centred within
/// 0.1° of a pole is affected, and only in how its numbers are split, not where things land.
pub const RENDER_FRAME_MAX_LATITUDE: f64 = 89.9;

/// The bind group of [`Globals`].
pub const GLOBALS_GROUP: u32 = 0;
/// The bind group of [`Frame`] (with a dynamic offset into the [`FrameArena`]).
pub const FRAME_GROUP: u32 = 1;

/// GL's clip depth `[−w, w]` to wgpu's `[0, w]`; x, y and w are unchanged, so is the handedness.
const GL_TO_WGPU_DEPTH: DMat4 =
    DMat4::from_cols_array(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.5, 1.0]);

/// `lib/project.wgsl`'s `Globals` (std140-compatible; the layout test compares the offsets).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Globals {
    /// Common offset → clip space (wgpu depth range), column-major.
    pub view_proj: [[f32; 4]; 4],
    /// Render frame → clip space; `view_proj` on the flat map.
    pub centre_cam: [[f32; 4]; 4],
    /// Globe: the view centre (with `position`) in the render frame, minus `(0, −256, 0)`; flat: 0.
    pub centre_offset: [f32; 3],
    /// [`MODE_GLOBE`] or [`MODE_FLAT`].
    pub mode: u32,
    /// `cos φc`, `sin φc` of the render frame's latitude; common units per metre at the centre;
    /// flat: the raised target's height in metres (`position[2]`), globe: 0.
    pub centre: [f32; 4],
    /// The camera's common offset.
    pub camera_common: [f32; 3],
    /// CSS pixels per common unit at the view centre (deck's `scale`).
    pub pixels_per_unit: f32,
    /// The camera in the render frame.
    pub camera_render: [f32; 3],
    /// Metres per CSS pixel at the view centre (deck's `metersPerPixel`).
    pub metres_per_pixel: f32,
    /// Canvas width and height in CSS pixels, map pixels per CSS pixel, and the local clock in
    /// seconds since the renderer's first frame (for animations).
    pub viewport: [f32; 4],
    /// Flat: world y of the view centre minus world y of the view's latitude; the rest 0.
    pub mercator: [f32; 4],
}

/// `lib/project.wgsl`'s `Frame`: one draw's local Mercator.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Frame {
    /// The anchor minus the view centre in world Mercator (y north), as hi + lo.
    pub offset_hi: [f32; 2],
    pub offset_lo: [f32; 2],
    /// World units per local unit; a power of two keeps `u · scale` exact on the GPU.
    pub scale: f32,
    /// Metres added to every vertex's height.
    pub z_offset: f32,
    pub pad: [f32; 2],
}

/// The f64 view of one frame and the [`Globals`] made from it.
///
/// Common offsets are measured from [`Self::common_centre`]: the view centre of the scene refresh
/// that placed the dynamic instances (plan 1.4.9). The camera moves at display rate in between, and
/// a frame made with [`Self::following`] folds the difference into `globals.view_proj` in f64, so
/// those instances stay put until the next refresh.
#[derive(Debug, Clone)]
pub struct ViewFrame {
    pub view: ViewState,
    /// The canvas in CSS pixels.
    pub size: [f64; 2],
    /// Map pixels per CSS pixel (plan 1.4.1).
    pub map_scale: f64,
    /// `scene::camera::viewport_for(view, size)`.
    pub viewport: Viewport,
    pub globals: Globals,
    /// What [`Self::common_offset`] subtracts (common space).
    pub common_centre: DVec3,
    /// The world Mercator point local offsets are measured from: on the globe the view's ground
    /// centre, on the flat map deck's centre (with `position`'s x and y).
    reference: DVec2,
}

impl ViewFrame {
    /// The terms of `view` on a canvas of `size` CSS pixels drawn at `map_scale`; `time_s` is the
    /// local clock for animations. Common offsets are measured from this view's own centre.
    pub fn new(view: &ViewState, size: [f64; 2], map_scale: f64, time_s: f32) -> Self {
        Self::build(view, size, map_scale, time_s, None)
    }

    /// Like [`Self::new`], with common offsets still measured from `refresh`'s centre (the view the
    /// scene builders placed the instances in). When the two views are in different projections
    /// (zoom 12 crossed) the instances' offsets mean nothing here: the app refreshes the scene, and
    /// this frame measures from its own centre meanwhile.
    pub fn following(view: &ViewState, size: [f64; 2], map_scale: f64, time_s: f32, refresh: &ViewFrame) -> Self {
        Self::build(view, size, map_scale, time_s, Some(refresh))
    }

    fn build(view: &ViewState, size: [f64; 2], map_scale: f64, time_s: f32, refresh: Option<&ViewFrame>) -> Self {
        let viewport = camera::viewport_for(view, size);
        let base = &*viewport;
        let view_proj = GL_TO_WGPU_DEPTH * base.projection_matrix * base.view_matrix_uncentered;
        let common_centre = match refresh {
            Some(r) if r.is_globe() == viewport.is_globe() => r.common_centre,
            _ => base.center,
        };
        // common offsets are from `common_centre`; the camera's matrices from this centre
        let mut shift = common_centre - base.center;
        if !viewport.is_globe() {
            shift.x = wrap_world_x(shift.x);
        }
        let camera_common = base.camera_position - base.center - shift;
        let mut globals = Globals {
            view_proj: mat(view_proj * DMat4::from_translation(shift)),
            pixels_per_unit: base.scale as f32,
            metres_per_pixel: base.meters_per_pixel() as f32,
            camera_common: camera_common.as_vec3().into(),
            viewport: [base.width as f32, base.height as f32, map_scale as f32, time_s],
            ..Zeroable::zeroed()
        };
        let reference = match &viewport {
            Viewport::Globe(globe) => {
                let latitude = globe.latitude.clamp(-RENDER_FRAME_MAX_LATITUDE, RENDER_FRAME_MAX_LATITUDE);
                // common space → render frame
                let turn = DMat4::from_rotation_x(latitude.to_radians()) * DMat4::from_rotation_z(-globe.longitude.to_radians());
                let (sin, cos) = latitude.to_radians().sin_cos();
                globals.mode = MODE_GLOBE;
                globals.centre_cam = mat(view_proj * turn.transpose());
                globals.centre_offset = (turn.transform_point3(base.center) - DVec3::new(0.0, -GLOBE_RADIUS, 0.0)).as_vec3().into();
                globals.camera_render = turn.transform_vector3(base.camera_position - base.center).as_vec3().into();
                globals.centre = [cos as f32, sin as f32, base.distance_scales.units_per_meter[2] as f32, 0.0];
                DVec2::from_array(lng_lat_to_world([globe.longitude, latitude]))
            }
            Viewport::Mercator(flat) => {
                let (sin, cos) = flat.latitude.to_radians().sin_cos();
                let raised = view.position.map_or(0.0, |p| p[2]);
                let ground_y = flat.project_flat([flat.longitude, flat.latitude])[1];
                globals.mode = MODE_FLAT;
                globals.centre_cam = mat(view_proj);
                globals.camera_render = (base.camera_position - base.center).as_vec3().into();
                globals.centre = [cos as f32, sin as f32, base.distance_scales.units_per_meter[2] as f32, raised as f32];
                globals.mercator = [(base.center.y - ground_y) as f32, 0.0, 0.0, 0.0];
                DVec2::new(base.center.x, base.center.y)
            }
        };
        ViewFrame { view: *view, size, map_scale, viewport, globals, common_centre, reference }
    }

    /// Whether this frame draws the globe (zoom ≤ 12).
    pub fn is_globe(&self) -> bool {
        self.viewport.is_globe()
    }

    /// The frame of a mesh in local Mercator: vertex `(u, v, h)` is world Mercator
    /// `anchor + (u, −v) · scale` (y north) at `h + z_offset_m` metres. `scale` should be a power of
    /// two ([`power_of_two_scale`]) and the mesh's `u` should lie in `[0, 1]`: of the world's copies
    /// (antimeridian), the one whose middle `u = 0.5` is nearest the view centre is drawn. A mesh
    /// that must show on both sides of the antimeridian at once on the flat map is split by its
    /// producer.
    pub fn local(&self, anchor: DVec2, scale: f64, z_offset_m: f64) -> Frame {
        let mut d = anchor - self.reference;
        d.x = wrap_world_x(d.x + scale / 2.0) - scale / 2.0;
        let hi = d.as_vec2();
        let lo = (d - hi.as_dvec2()).as_vec2();
        debug_assert!(scale > 0.0 && (scale as f32) as f64 == scale, "scale {scale} is not exact in f32");
        Frame { offset_hi: hi.into(), offset_lo: lo.into(), scale: scale as f32, z_offset: z_offset_m as f32, pad: [0.0; 2] }
    }

    /// The frame of a tile's mesh: tile units from its north-west corner, `u` east and `v` south in
    /// `[0, 1]`.
    pub fn tile(&self, tile: TileIndex, z_offset_m: f64) -> Frame {
        let extent = tile_extent(tile.z);
        let anchor = DVec2::new(f64::from(tile.x) * extent, WORLD_SIZE - f64::from(tile.y) * extent);
        self.local(anchor, extent, z_offset_m)
    }

    /// A geographic point (`[lon, lat]` degrees, `h` metres) as a common offset for
    /// `project_common`: deck's `projectPosition` minus [`Self::common_centre`], in f64 then f32. On
    /// the flat map the copy nearest the centre is used.
    pub fn common_offset(&self, [lon, lat]: [f64; 2], h: f64) -> Vec3 {
        let mut d = self.viewport.project_position([lon, lat, h]) - self.common_centre;
        if !self.is_globe() {
            d.x = wrap_world_x(d.x);
        }
        d.as_vec3()
    }
}

/// World Mercator units per tile unit at tile zoom `z` (512 / 2^z).
pub fn tile_extent(z: u32) -> f64 {
    WORLD_SIZE / 2f64.powi(z as i32)
}

/// The smallest power of two at least `extent`: a [`ViewFrame::local`] scale for a mesh of that
/// extent in world units.
pub fn power_of_two_scale(extent: f64) -> f64 {
    2f64.powi(extent.max(f64::MIN_POSITIVE).log2().ceil() as i32)
}

/// A world x difference moved into `[−256, 256)`.
fn wrap_world_x(dx: f64) -> f64 {
    dx - WORLD_SIZE * ((dx + WORLD_SIZE / 2.0) / WORLD_SIZE).floor()
}

fn mat(m: DMat4) -> [[f32; 4]; 4] {
    m.as_mat4().to_cols_array_2d()
}

// ---------------------------------------------------------------------------------------------
// GPU side: the globals buffer, the bind group layouts, the frame arena
// ---------------------------------------------------------------------------------------------

/// The bind group layouts every map pipeline starts with, and the globals buffer.
pub struct Bindings {
    /// Group 0: [`Globals`].
    pub globals_layout: wgpu::BindGroupLayout,
    /// Group 1: one [`Frame`] at a dynamic offset.
    pub frame_layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    globals_group: wgpu::BindGroup,
}

impl Bindings {
    pub fn new(device: &wgpu::Device) -> Self {
        let uniform = |label, dynamic: bool, size: usize| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: dynamic,
                        min_binding_size: NonZeroU64::new(size as u64),
                    },
                    count: None,
                }],
            })
        };
        let globals_layout = uniform("map globals", false, size_of::<Globals>());
        let frame_layout = uniform("map frame", true, size_of::<Frame>());
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("map globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("map globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        Bindings { globals_layout, frame_layout, globals, globals_group }
    }

    pub fn write_globals(&self, queue: &wgpu::Queue, globals: &Globals) {
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(globals));
    }

    /// The bind group of group 0.
    pub fn globals_group(&self) -> &wgpu::BindGroup {
        &self.globals_group
    }

    /// A pipeline layout of groups 0 (globals), 1 (frame) and then `extra`.
    pub fn pipeline_layout(&self, device: &wgpu::Device, label: &str, extra: &[&wgpu::BindGroupLayout]) -> wgpu::PipelineLayout {
        let mut groups = vec![Some(&self.globals_layout), Some(&self.frame_layout)];
        groups.extend(extra.iter().map(|l| Some(*l)));
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &groups, immediate_size: 0 })
    }
}

/// Where one pushed [`Frame`] sits in the [`FrameArena`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSlot(u32);

impl FrameSlot {
    /// The dynamic offset of the frame's bind group, bytes.
    pub fn offset(self) -> u32 {
        self.0
    }
}

/// This frame's [`Frame`]s, one uniform buffer bound at dynamic offsets. Cleared and refilled by
/// the layers' `prepare` every frame, uploaded once before the passes. Slot 0 always holds a zero
/// frame ([`FrameArena::DEFAULT`]), which the renderer binds at the start of the map pass, so a
/// pipeline that never reads `frame` need not bind one.
pub struct FrameArena {
    stride: usize,
    staging: Vec<u8>,
    capacity: usize,
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
}

impl FrameArena {
    const INITIAL_CAPACITY: usize = 256;

    /// The zero frame of slot 0.
    pub const DEFAULT: FrameSlot = FrameSlot(0);

    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let stride = (device.limits().min_uniform_buffer_offset_alignment as usize).max(size_of::<Frame>());
        let (buffer, group) = Self::allocate(device, layout, stride, Self::INITIAL_CAPACITY);
        let mut arena = FrameArena { stride, staging: Vec::new(), capacity: Self::INITIAL_CAPACITY, buffer, group };
        arena.clear();
        arena
    }

    fn allocate(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, stride: usize, capacity: usize) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("map frames"),
            size: (stride * capacity) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("map frames"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: NonZeroU64::new(size_of::<Frame>() as u64),
                }),
            }],
        });
        (buffer, group)
    }

    /// Forgets last frame's frames (all but the zero frame of slot 0).
    pub fn clear(&mut self) {
        self.staging.clear();
        self.push(&Frame::zeroed());
    }

    /// How many frames it holds, the zero frame included.
    pub fn count(&self) -> usize {
        self.staging.len() / self.stride
    }

    /// Adds a frame for this frame's passes.
    pub fn push(&mut self, frame: &Frame) -> FrameSlot {
        let offset = self.staging.len();
        self.staging.extend_from_slice(bytemuck::bytes_of(frame));
        self.staging.resize(offset + self.stride, 0);
        FrameSlot(offset as u32)
    }

    /// Uploads the pushed frames, growing the buffer (doubling) when they do not fit.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, layout: &wgpu::BindGroupLayout) {
        let needed = self.count();
        if needed > self.capacity {
            let capacity = needed.next_power_of_two();
            (self.buffer, self.group) = Self::allocate(device, layout, self.stride, capacity);
            self.capacity = capacity;
        }
        queue.write_buffer(&self.buffer, 0, &self.staging);
    }

    /// Binds `slot` as group 1.
    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>, slot: FrameSlot) {
        pass.set_bind_group(FRAME_GROUP, &self.group, &[slot.offset()]);
    }
}

// ---------------------------------------------------------------------------------------------
// The f32 twin of lib/project.wgsl
// ---------------------------------------------------------------------------------------------

/// `lib/project.wgsl` transcribed into Rust f32, operation for operation, for the precision test
/// and for CPU-side checks of what the GPU will do. Keep it in step with the WGSL.
pub mod model {
    use super::{Frame, Globals, MODE_FLAT};

    const RADIANS_PER_UNIT: f32 = 0.012_271_846;
    const GLOBE_RADIUS: f32 = 256.0;
    const EARTH_RADIUS_M: f32 = 6_370_972.0;
    const SMALL: f32 = 1.0;

    fn sin_vers(x: f32) -> [f32; 2] {
        let q = 0.25 * x;
        let q2 = q * q;
        let s1 = q * (1.0 - q2 / 6.0 * (1.0 - q2 / 20.0 * (1.0 - q2 / 42.0 * (1.0 - q2 / 72.0 * (1.0 - q2 / 110.0)))));
        let v1 = q2 / 2.0 * (1.0 - q2 / 12.0 * (1.0 - q2 / 30.0 * (1.0 - q2 / 56.0 * (1.0 - q2 / 90.0 * (1.0 - q2 / 132.0)))));
        let s2 = 2.0 * s1 * (1.0 - v1);
        let v2 = 2.0 * s1 * s1;
        [2.0 * s2 * (1.0 - v2), 2.0 * s2 * s2]
    }

    fn sinh_small(x: f32) -> f32 {
        if x.abs() < SMALL {
            let x2 = x * x;
            return x * (1.0 + x2 / 6.0 * (1.0 + x2 / 20.0 * (1.0 + x2 / 42.0 * (1.0 + x2 / 72.0 * (1.0 + x2 / 110.0)))));
        }
        x.sinh()
    }

    fn cosh_m1_small(x: f32) -> f32 {
        let x2 = x * x;
        if x.abs() < SMALL {
            return x2 / 2.0 * (1.0 + x2 / 12.0 * (1.0 + x2 / 30.0 * (1.0 + x2 / 56.0 * (1.0 + x2 / 90.0 * (1.0 + x2 / 132.0)))));
        }
        x.cosh() - 1.0
    }

    fn atan_small(y: f32) -> f32 {
        let r = if y.abs() > 1.0 { 1.0 / y } else { y };
        let t = r / (1.0 + (1.0 + r * r).sqrt());
        let t2 = t * t;
        let series = 1.0 / 13.0 - t2 * (1.0 / 15.0 - t2 * (1.0 / 17.0 - t2 / 19.0));
        let a = 2.0 * t * (1.0 - t2 * (1.0 / 3.0 - t2 * (1.0 / 5.0 - t2 * (1.0 / 7.0 - t2 * (1.0 / 9.0 - t2 * (1.0 / 11.0 - t2 * series))))));
        if y.abs() > 1.0 {
            return y.signum() * 0.5 * std::f32::consts::PI - a;
        }
        a
    }

    fn local_offset(f: &Frame, [u, v]: [f32; 2]) -> [f32; 2] {
        let w = [u * f.scale, -v * f.scale];
        [(w[0] + f.offset_hi[0]) + f.offset_lo[0], (w[1] + f.offset_hi[1]) + f.offset_lo[1]]
    }

    fn delta_latitude(g: &Globals, dpsi: f32) -> f32 {
        let s = sinh_small(0.5 * dpsi);
        let c = 1.0 + cosh_m1_small(0.5 * dpsi);
        2.0 * atan_small(g.centre[0] * s / (c + g.centre[1] * s))
    }

    fn globe_rel(g: &Globals, d: [f32; 2], h: f32) -> [f32; 3] {
        let cos_c = g.centre[0];
        let sin_c = g.centre[1];
        // WGSL's `round` rounds half to even
        let dx = d[0] - 512.0 * (d[0] / 512.0).round_ties_even();
        let lon = sin_vers(dx * RADIANS_PER_UNIT);
        let lat = sin_vers(delta_latitude(g, d[1] * RADIANS_PER_UNIT));
        let cos_lat = cos_c * (1.0 - lat[1]) - sin_c * lat[0];
        let lift = h / EARTH_RADIUS_M;
        let dist = GLOBE_RADIUS * (1.0 + lift);
        let p = [
            dist * (cos_lat * lon[0]),
            dist * (lat[1] + cos_lat * cos_c * lon[1]) - GLOBE_RADIUS * lift,
            dist * (lat[0] + cos_lat * sin_c * lon[1]),
        ];
        [p[0] - g.centre_offset[0], p[1] - g.centre_offset[1], p[2] - g.centre_offset[2]]
    }

    fn flat_rel(g: &Globals, d: [f32; 2], h: f32) -> [f32; 3] {
        let x = (d[1] + g.mercator[0]) * RADIANS_PER_UNIT;
        let f = cosh_m1_small(x) + g.centre[1] * sinh_small(x);
        let raised = g.centre[3];
        let z = g.centre[2] * ((h - raised) * (1.0 + f) + raised * f);
        [d[0], d[1], z]
    }

    fn mul(m: &[[f32; 4]; 4], v: [f32; 4]) -> [f32; 4] {
        let mut out = [0.0f32; 4];
        for (i, o) in out.iter_mut().enumerate() {
            *o = m[0][i] * v[0] + m[1][i] * v[1] + m[2][i] * v[2] + m[3][i] * v[3];
        }
        out
    }

    /// `project_local(local)` under `frame`: clip position and the render-frame offset.
    pub fn project_local(g: &Globals, f: &Frame, [u, v, z]: [f32; 3]) -> ([f32; 4], [f32; 3]) {
        let d = local_offset(f, [u, v]);
        let h = z + f.z_offset;
        let rel = if g.mode == MODE_FLAT { flat_rel(g, d, h) } else { globe_rel(g, d, h) };
        (mul(&g.centre_cam, [rel[0], rel[1], rel[2], 1.0]), rel)
    }

    /// `project_common(offset)`.
    pub fn project_common(g: &Globals, offset: [f32; 3]) -> [f32; 4] {
        mul(&g.view_proj, [offset[0], offset[1], offset[2], 1.0])
    }

    /// `local_basis(local)`: east, north and up at a local point, in the render frame.
    pub fn local_basis(g: &Globals, f: &Frame, [u, v]: [f32; 2]) -> [[f32; 3]; 3] {
        if g.mode == MODE_FLAT {
            return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        }
        let cos_c = g.centre[0];
        let sin_c = g.centre[1];
        let d = local_offset(f, [u, v]);
        let lon = sin_vers((d[0] - 512.0 * (d[0] / 512.0).round_ties_even()) * RADIANS_PER_UNIT);
        let lat = sin_vers(delta_latitude(g, d[1] * RADIANS_PER_UNIT));
        let cos_lat = cos_c * (1.0 - lat[1]) - sin_c * lat[0];
        let sin_lat = sin_c * (1.0 - lat[1]) + cos_c * lat[0];
        let cl = 1.0 - lon[1];
        let sl = lon[0];
        let rx = |p: [f32; 3]| [p[0], cos_c * p[1] - sin_c * p[2], sin_c * p[1] + cos_c * p[2]];
        [rx([cl, sl, 0.0]), rx([-sl * sin_lat, cl * sin_lat, cos_lat]), rx([sl * cos_lat, -cl * cos_lat, sin_lat])]
    }

    /// Clip space to CSS pixels from the top left (y down), in f32 as the GPU's viewport transform.
    pub fn clip_to_pixels(g: &Globals, clip: [f32; 4]) -> [f32; 2] {
        let x = clip[0] / clip[3];
        let y = clip[1] / clip[3];
        [(x + 1.0) * 0.5 * g.viewport[0], (1.0 - y) * 0.5 * g.viewport[1]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shaders;
    use rustradar_core::num::js_round;
    use rustradar_scene::camera::world_to_lng_lat;

    /// One test point: a vertex in a local Mercator frame, the same point as a common offset, and
    /// where `scene::camera` puts it in f64.
    struct Sample {
        frame: Frame,
        local: [f32; 3],
        offset: [f32; 3],
        expected: [f64; 2],
        about: String,
    }

    /// Points sampled over the screen of `vf` (a 13 × 13 grid, on the globe only where it is hit),
    /// at each of `heights` metres above the target, in the vector tile of the point and in a
    /// world-sized frame. The expected pixel is that of the point the f32 vertex really holds.
    fn samples(vf: &ViewFrame, heights: &[f64]) -> Vec<Sample> {
        let (view, size, vp) = (&vf.view, vf.size, &vf.viewport);
        let tile_z = js_round(view.zoom).clamp(0.0, 14.0) as u32;
        let target_z = view.position.map_or(0.0, |p| p[2]);
        let n = 12;
        let mut out = Vec::new();
        for i in 0..=n {
            for j in 0..=n {
                let px = [size[0] * (0.01 + 0.98 * i as f64 / n as f64), size[1] * (0.01 + 0.98 * j as f64 / n as f64)];
                if let Some(globe) = vp.globe()
                    && globe.ray_distance_to_globe_center_ratio(px) > 0.999
                {
                    continue;
                }
                let [lon, lat] = vp.unproject(px, target_z);
                if !lon.is_finite() || !lat.is_finite() || lat.abs() > 85.0 {
                    continue;
                }
                let world = DVec2::from_array(lng_lat_to_world([lon, lat]));
                for &h in heights {
                    let h = f64::from((h + target_z) as f32);
                    let extent = tile_extent(tile_z);
                    let last = 2f64.powi(tile_z as i32) - 1.0;
                    let tx = (world.x / extent).floor().clamp(0.0, last);
                    let ty = ((WORLD_SIZE - world.y) / extent).floor().clamp(0.0, last);
                    for (anchor, scale) in [(DVec2::new(tx * extent, WORLD_SIZE - ty * extent), extent), (DVec2::new(0.0, WORLD_SIZE), WORLD_SIZE)] {
                        let u = ((world.x - anchor.x) / scale) as f32;
                        let v = ((anchor.y - world.y) / scale) as f32;
                        let held = anchor + DVec2::new(f64::from(u) * scale, -f64::from(v) * scale);
                        let [lon, lat] = world_to_lng_lat(held.to_array());
                        let [x, y, depth] = vp.project([lon, lat, h]);
                        // in front of the camera and inside the depth range
                        let camera_space = vp.view_matrix * vp.project_position([lon, lat, h]).extend(1.0);
                        if camera_space.z >= 0.0 || !(depth > -1.0 && depth < 1.0) {
                            continue;
                        }
                        out.push(Sample {
                            frame: vf.local(anchor, scale, 0.0),
                            local: [u, v, h as f32],
                            offset: vf.common_offset([lon, lat], h).into(),
                            expected: [x, y],
                            about: format!("{view:?} {size:?} px {px:?} h {h} scale {scale}"),
                        });
                    }
                }
            }
        }
        out
    }

    /// Views of the precision sweep, each with its sample heights.
    type Views = Vec<(ViewFrame, Vec<f64>)>;

    /// The views of the precision sweep: (globe views, flat views).
    fn sweep_views() -> (Views, Views) {
        let frame = |view: ViewState, size| ViewFrame::new(&view, size, 1.0, 0.0);
        let mut globe = Vec::new();
        for step in 1..=24 {
            let zoom = 0.5 * f64::from(step);
            for latitude in [0.0, 45.0, 60.0, 80.0, -35.0] {
                for (pitch, bearing) in [(0.0, 0.0), (45.0, 210.0), (60.0, 30.0)] {
                    for size in [[1440.0, 900.0], [390.0, 844.0]] {
                        let view = ViewState { longitude: 17.9, latitude, zoom, bearing, pitch, position: None };
                        globe.push((frame(view, size), vec![0.0, 2500.0]));
                    }
                }
            }
        }
        // the antimeridian
        let view = ViewState { longitude: 179.95, latitude: -12.0, zoom: 9.0, bearing: 0.0, pitch: 30.0, position: None };
        globe.push((frame(view, [1440.0, 900.0]), vec![0.0]));

        let mut flat = Vec::new();
        for zoom in std::iter::once(12.01).chain((25..=46).map(|s| 0.5 * f64::from(s))) {
            for latitude in [0.0, 45.0, 60.0, 80.0, -60.0] {
                for (pitch, bearing) in [(0.0, 0.0), (45.0, 210.0), (60.0, 30.0), (85.0, 300.0)] {
                    let view = ViewState { longitude: 17.9, latitude, zoom, bearing, pitch, position: None };
                    let camera = camera::camera_distance_m(zoom, latitude, 900.0);
                    flat.push((frame(view, [1440.0, 900.0]), vec![0.0, 0.3 * camera]));
                    // locked on an aircraft: the target raised to its altitude
                    let raised = ViewState { position: Some([0.0, 0.0, 10_000.0]), ..view };
                    flat.push((frame(raised, [390.0, 844.0]), vec![0.0, -0.3 * camera]));
                }
            }
        }
        let view = ViewState { longitude: -179.999, latitude: 65.0, zoom: 18.0, bearing: 90.0, pitch: 50.0, position: None };
        flat.push((frame(view, [1440.0, 900.0]), vec![0.0]));
        (globe, flat)
    }

    /// How far a projection lands from `scene::camera`, worst case.
    #[derive(Default, Debug)]
    struct Worst {
        local: f64,
        common: f64,
        points: usize,
        at: String,
    }

    impl Worst {
        fn add(&mut self, sample: &Sample, local: [f32; 2], common: [f32; 2]) {
            let err = |p: [f32; 2]| (f64::from(p[0]) - sample.expected[0]).hypot(f64::from(p[1]) - sample.expected[1]);
            if err(local) > self.local {
                self.local = err(local);
                self.at = sample.about.clone();
            }
            self.common = self.common.max(err(common));
            self.points += 1;
        }
    }

    /// The acceptance of E4: the f32 formulas of `lib/project.wgsl` (as [`model`]) against
    /// `scene::camera` in f64 stay under 0.25 px on the globe at zoom 0.5–12 and under 0.01 px on the
    /// flat map at zoom 12–23, for local Mercator vertices (tile and world frames) and common offsets.
    #[test]
    fn projection_precision() {
        let (globe_views, flat_views) = sweep_views();
        let sweep = |views: &[(ViewFrame, Vec<f64>)]| {
            let mut worst = Worst::default();
            for (vf, heights) in views {
                let g = &vf.globals;
                for sample in samples(vf, heights) {
                    let (clip, _) = model::project_local(g, &sample.frame, sample.local);
                    assert!(clip[3] > 0.0, "{}: behind the camera", sample.about);
                    let common = model::project_common(g, sample.offset);
                    worst.add(&sample, model::clip_to_pixels(g, clip), model::clip_to_pixels(g, common));
                }
            }
            worst
        };
        let globe = sweep(&globe_views);
        let flat = sweep(&flat_views);
        eprintln!("globe: {globe:?}");
        eprintln!("flat:  {flat:?}");
        assert!(globe.points > 10_000 && flat.points > 10_000, "too few points: {} and {}", globe.points, flat.points);
        assert!(globe.local < 0.25, "globe, local Mercator: {} px at {}", globe.local, globe.at);
        assert!(globe.common < 0.25, "globe, common offsets: {} px", globe.common);
        assert!(flat.local < 0.01, "flat, local Mercator: {} px at {}", flat.local, flat.at);
        assert!(flat.common < 0.01, "flat, common offsets: {} px", flat.common);
    }

    /// The same sweep through the real `lib/project.wgsl` on the GPU (SwiftShader here): each
    /// point's clip position is written to its own texel of an `Rgba32Float` row and read back. This
    /// checks the WGSL against `scene::camera` as the CPU twin is checked, within the same budgets.
    #[test]
    fn project_wgsl_on_the_gpu_matches_the_camera() {
        let Some(gpu) = crate::gpu::testing::gpu() else { return };
        let format = wgpu::TextureFormat::Rgba32Float;
        if !gpu.adapter.get_texture_format_features(format).allowed_usages.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) {
            eprintln!("skipped: {format:?} is not renderable on {}", gpu.describe());
            return;
        }
        let probe = gpu::Probe::new(&gpu, format);
        let (globe_views, flat_views) = sweep_views();
        // every third view keeps the run short on a software rasteriser
        let sweep = |views: &[(ViewFrame, Vec<f64>)]| {
            let mut worst = Worst::default();
            for (vf, heights) in views.iter().step_by(3) {
                let samples = samples(vf, heights);
                let (local, common) = probe.run(&gpu, vf, &samples);
                for ((sample, l), c) in samples.iter().zip(local).zip(common) {
                    assert!(l[3] > 0.0, "{}: behind the camera on the GPU", sample.about);
                    worst.add(sample, model::clip_to_pixels(&vf.globals, l), model::clip_to_pixels(&vf.globals, c));
                }
            }
            worst
        };
        let globe = sweep(&globe_views);
        let flat = sweep(&flat_views);
        eprintln!("GPU ({}): globe {globe:?}", gpu.describe());
        eprintln!("GPU: flat {flat:?}");
        assert!(globe.points > 1000 && flat.points > 1000, "too few points: {} and {}", globe.points, flat.points);
        assert!(globe.local < 0.25 && globe.common < 0.25, "globe on the GPU: {globe:?}");
        assert!(flat.local < 0.01 && flat.common < 0.01, "flat on the GPU: {flat:?}");
    }

    /// The GPU half of [`project_wgsl_on_the_gpu_matches_the_camera`].
    mod gpu {
        use super::{Sample, ViewFrame};
        use crate::frame::{Bindings, FrameArena};
        use crate::shaders;

        const SHADER: &str = "//!include lib/project.wgsl
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) clip: vec4<f32>,
}

@vertex
fn vs_local(@location(0) local: vec3<f32>, @location(1) texel: f32) -> Out {
    return Out(vec4<f32>(texel, 0.0, 0.0, 1.0), project_local(local).clip);
}

@vertex
fn vs_common(@location(0) offset: vec3<f32>, @location(1) texel: f32) -> Out {
    return Out(vec4<f32>(texel, 0.0, 0.0, 1.0), project_common(offset));
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    return in.clip;
}
";

        pub(super) struct Probe {
            format: wgpu::TextureFormat,
            bindings: Bindings,
            local: wgpu::RenderPipeline,
            common: wgpu::RenderPipeline,
        }

        impl Probe {
            pub(super) fn new(gpu: &crate::gpu::Gpu, format: wgpu::TextureFormat) -> Self {
                let device = &gpu.device;
                let bindings = Bindings::new(device);
                let module = shaders::module(device, "projection probe", SHADER);
                let layout = bindings.pipeline_layout(device, "projection probe", &[]);
                let pipeline = |entry| {
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some(entry),
                        layout: Some(&layout),
                        vertex: wgpu::VertexState {
                            module: &module,
                            entry_point: Some(entry),
                            compilation_options: Default::default(),
                            buffers: &[Some(wgpu::VertexBufferLayout {
                                array_stride: 16,
                                step_mode: wgpu::VertexStepMode::Vertex,
                                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32],
                            })],
                        },
                        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::PointList, ..Default::default() },
                        depth_stencil: None,
                        multisample: Default::default(),
                        fragment: Some(wgpu::FragmentState {
                            module: &module,
                            entry_point: Some("fs"),
                            compilation_options: Default::default(),
                            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                        }),
                        multiview_mask: None,
                        cache: None,
                    })
                };
                Probe { format, local: pipeline("vs_local"), common: pipeline("vs_common"), bindings }
            }

            /// The clip positions of every sample, through `project_local` and `project_common`.
            pub(super) fn run(&self, gpu: &crate::gpu::Gpu, vf: &ViewFrame, samples: &[Sample]) -> (Vec<[f32; 4]>, Vec<[f32; 4]>) {
                if samples.is_empty() {
                    return (Vec::new(), Vec::new());
                }
                let device = &gpu.device;
                let width = samples.len() as u32;
                let texel = |i: usize| (i as f32 + 0.5) / width as f32 * 2.0 - 1.0;
                let mut vertices: Vec<f32> = Vec::with_capacity(samples.len() * 8);
                for (i, s) in samples.iter().enumerate() {
                    vertices.extend_from_slice(&[s.local[0], s.local[1], s.local[2], texel(i)]);
                }
                for (i, s) in samples.iter().enumerate() {
                    vertices.extend_from_slice(&[s.offset[0], s.offset[1], s.offset[2], texel(i)]);
                }
                let vertex_buffer = wgpu::util::DeviceExt::create_buffer_init(
                    device,
                    &wgpu::util::BufferInitDescriptor { label: Some("probe"), contents: bytemuck::cast_slice(&vertices), usage: wgpu::BufferUsages::VERTEX },
                );
                let mut frames = FrameArena::new(device, &self.bindings.frame_layout);
                let slots: Vec<_> = samples.iter().map(|s| frames.push(&s.frame)).collect();
                frames.upload(device, &gpu.queue, &self.bindings.frame_layout);
                self.bindings.write_globals(&gpu.queue, &vf.globals);
                let target = |label| {
                    device.create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d { width, height: 1, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: self.format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                        view_formats: &[],
                    })
                };
                let (local_target, common_target) = (target("probe local"), target("probe common"));
                let mut encoder = device.create_command_encoder(&Default::default());
                for (texture, pipeline, first) in [(&local_target, &self.local, 0u32), (&common_target, &self.common, width)] {
                    let view = texture.create_view(&Default::default());
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                        })],
                        ..Default::default()
                    });
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, self.bindings.globals_group(), &[]);
                    pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                    for (i, slot) in slots.iter().enumerate() {
                        frames.bind(&mut pass, *slot);
                        let v = first + i as u32;
                        pass.draw(v..v + 1, 0..1);
                    }
                }
                let row = (width as usize * 16).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
                let readback = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("probe readback"),
                    size: 2 * row as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                for (k, texture) in [&local_target, &common_target].into_iter().enumerate() {
                    encoder.copy_texture_to_buffer(
                        texture.as_image_copy(),
                        wgpu::TexelCopyBufferInfo {
                            buffer: &readback,
                            layout: wgpu::TexelCopyBufferLayout { offset: (k * row) as u64, bytes_per_row: Some(row as u32), rows_per_image: None },
                        },
                        wgpu::Extent3d { width, height: 1, depth_or_array_layers: 1 },
                    );
                }
                gpu.queue.submit([encoder.finish()]);
                readback.map_async(wgpu::MapMode::Read, .., |r| r.expect("mapped"));
                device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
                let data = readback.get_mapped_range(..).expect("mapped range");
                let texels = |k: usize| -> Vec<[f32; 4]> {
                    let floats: &[f32] = bytemuck::cast_slice(&data[k * row..k * row + width as usize * 16]);
                    floats.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect()
                };
                (texels(0), texels(1))
            }
        }
    }

    #[test]
    fn the_view_centre_lands_mid_screen_in_both_projections() {
        for zoom in [1.9, 11.99, 12.01, 18.0] {
            let view = ViewState { longitude: 18.07, latitude: 59.33, zoom, bearing: 20.0, pitch: 40.0, position: None };
            let vf = ViewFrame::new(&view, [1100.0, 720.0], 2.0, 0.0);
            let centre = DVec2::from_array(lng_lat_to_world([view.longitude, view.latitude]));
            // a frame anchored at the centre itself: (0, 0) is the centre
            let frame = vf.local(centre, 1.0, 0.0);
            assert!(frame.offset_hi[0].abs() < 1e-9 && frame.offset_hi[1].abs() < 1e-9, "{frame:?}");
            let (clip, rel) = model::project_local(&vf.globals, &frame, [0.0, 0.0, 0.0]);
            assert!(rel.iter().all(|c| c.abs() < 1e-9), "{rel:?}");
            let px = model::clip_to_pixels(&vf.globals, clip);
            assert!((px[0] - 550.0).abs() < 1e-3 && (px[1] - 360.0).abs() < 1e-3, "zoom {zoom}: {px:?}");
            // depth inside wgpu's [0, 1]
            let depth = clip[2] / clip[3];
            assert!(depth > 0.0 && depth < 1.0, "zoom {zoom}: depth {depth}");
            assert_eq!(vf.globals.mode, if zoom > 12.0 { MODE_FLAT } else { MODE_GLOBE });
            assert_eq!(vf.globals.viewport, [1100.0, 720.0, 2.0, 0.0]);
        }
    }

    #[test]
    fn frames_wrap_to_the_nearest_world_and_split_hi_lo() {
        let view = ViewState { longitude: 179.9, latitude: 10.0, zoom: 14.0, bearing: 0.0, pitch: 0.0, position: None };
        let vf = ViewFrame::new(&view, [800.0, 600.0], 1.0, 0.0);
        // a z14 tile just east of the antimeridian
        let frame = vf.tile(TileIndex::new(0, 7700, 14), 0.0);
        assert!(frame.offset_hi[0] > 0.0 && frame.offset_hi[0] < 1.0, "{frame:?}");
        assert_eq!(frame.scale, 512.0 / 16384.0);
        // hi + lo carries the f64 offset
        let reference = DVec2::new(vf.viewport.center.x, vf.viewport.center.y);
        let exact = wrap_world_x(0.0 - reference.x);
        let sum = f64::from(frame.offset_hi[0]) + f64::from(frame.offset_lo[0]);
        assert!((sum - exact).abs() < 1e-12, "{sum} vs {exact}");
        assert!(f64::from(frame.offset_lo[0]).abs() <= f64::from(frame.offset_hi[0]).abs() * 1e-7);
        assert_eq!(power_of_two_scale(3.0), 4.0);
        assert_eq!(power_of_two_scale(0.25), 0.25);
        assert_eq!(power_of_two_scale(512.0), 512.0);
        assert_eq!(tile_extent(0), 512.0);
        assert_eq!(tile_extent(9), 1.0);
    }

    #[test]
    fn a_raised_globe_centre_stays_mid_screen() {
        // on the globe a raised target moves the centre along the polar axis (deck); the render
        // frame's offset carries it, so a point at the raised centre is still at the centre
        let view = ViewState { longitude: 10.0, latitude: 35.0, zoom: 8.0, bearing: 0.0, pitch: 0.0, position: Some([0.0, 0.0, 30_000.0]) };
        let vf = ViewFrame::new(&view, [1000.0, 800.0], 1.0, 0.0);
        let centre = vf.viewport.center;
        let [lon, lat, h] = vf.viewport.unproject_position(centre);
        let expected = vf.viewport.project([lon, lat, h]);
        let world = DVec2::from_array(lng_lat_to_world([lon, lat]));
        let frame = vf.local(world, 1.0, 0.0);
        let (clip, _) = model::project_local(&vf.globals, &frame, [0.0, 0.0, h as f32]);
        let px = model::clip_to_pixels(&vf.globals, clip);
        assert!((f64::from(px[0]) - expected[0]).abs() < 0.01 && (f64::from(px[1]) - expected[1]).abs() < 0.01, "{px:?} vs {expected:?}");
        assert!((expected[0] - 500.0).abs() < 1e-6 && (expected[1] - 400.0).abs() < 1e-6);
    }

    #[test]
    fn the_local_basis_is_east_north_up_at_the_vertex() {
        let dot = |a: [f32; 3], b: [f32; 3]| f64::from(a[0] * b[0] + a[1] * b[1] + a[2] * b[2]);
        let unit = |a: [f32; 3]| {
            let l = dot(a, a).sqrt() as f32;
            [a[0] / l, a[1] / l, a[2] / l]
        };
        let mut checked = 0;
        for (latitude, zoom) in [(0.0, 1.0), (59.3, 4.0), (-70.0, 8.0), (80.0, 11.0)] {
            let view = ViewState { longitude: 100.0, latitude, zoom, bearing: 40.0, pitch: 30.0, position: None };
            let vf = ViewFrame::new(&view, [1000.0, 700.0], 1.0, 0.0);
            let g = &vf.globals;
            for sample in samples(&vf, &[0.0]) {
                let [u, v, _] = sample.local;
                let [east, north, up] = model::local_basis(g, &sample.frame, [u, v]);
                // up points away from the globe's centre, which sits at (0, 256, 0) below the
                // view centre in the render frame
                let (_, rel) = model::project_local(g, &sample.frame, [u, v, 0.0]);
                let from_centre = [rel[0] + g.centre_offset[0], rel[1] + g.centre_offset[1] - 256.0, rel[2] + g.centre_offset[2]];
                assert!(dot(up, unit(from_centre)) > 1.0 - 1e-6, "{}: up {up:?}", sample.about);
                // a step east and a step north move along east and north
                let step = 1e-3 / f64::from(sample.frame.scale) as f32;
                let (_, e) = model::project_local(g, &sample.frame, [u + step, v, 0.0]);
                let (_, n) = model::project_local(g, &sample.frame, [u, v - step, 0.0]);
                let moved = |p: [f32; 3]| unit([p[0] - rel[0], p[1] - rel[1], p[2] - rel[2]]);
                assert!(dot(east, moved(e)) > 0.999, "{}: east {east:?} vs {:?}", sample.about, moved(e));
                assert!(dot(north, moved(n)) > 0.999, "{}: north {north:?} vs {:?}", sample.about, moved(n));
                for a in [east, north, up] {
                    assert!((dot(a, a) - 1.0).abs() < 1e-5);
                }
                assert!(dot(east, north).abs() < 1e-5 && dot(east, up).abs() < 1e-5 && dot(north, up).abs() < 1e-5);
                checked += 1;
            }
        }
        assert!(checked > 100, "{checked}");
        // the flat map's render frame is east, north, up already
        let vf = ViewFrame::new(&ViewState { zoom: 15.0, ..ViewState::INITIAL }, [800.0, 600.0], 1.0, 0.0);
        assert_eq!(model::local_basis(&vf.globals, &Frame::zeroed(), [0.3, 0.6]), [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    }

    /// Instances placed at a scene refresh stay where the camera puts them while it moves on.
    #[test]
    fn common_offsets_follow_the_camera_between_refreshes() {
        for (refresh, now) in [
            (
                ViewState { longitude: 18.0, latitude: 59.3, zoom: 9.0, bearing: 0.0, pitch: 0.0, position: None },
                ViewState { longitude: 18.3, latitude: 59.4, zoom: 9.6, bearing: 35.0, pitch: 40.0, position: None },
            ),
            (
                ViewState { longitude: 179.999, latitude: -16.0, zoom: 15.0, bearing: 10.0, pitch: 45.0, position: None },
                ViewState { longitude: -179.999, latitude: -16.001, zoom: 15.2, bearing: 15.0, pitch: 50.0, position: Some([0.0, 0.0, 40.0]) },
            ),
        ] {
            let size = [900.0, 600.0];
            let at_refresh = ViewFrame::new(&refresh, size, 1.0, 0.0);
            let vf = ViewFrame::following(&now, size, 1.0, 0.5, &at_refresh);
            assert_eq!(vf.common_centre, at_refresh.common_centre);
            for (x, y) in [(450.0, 300.0), (100.0, 80.0), (820.0, 560.0)] {
                let [lon, lat] = vf.viewport.unproject([x, y], 25.0);
                let offset = at_refresh.common_offset([lon, lat], 25.0);
                let got = model::clip_to_pixels(&vf.globals, model::project_common(&vf.globals, offset.into()));
                let expected = vf.viewport.project([lon, lat, 25.0]);
                assert!(
                    (f64::from(got[0]) - expected[0]).hypot(f64::from(got[1]) - expected[1]) < 0.01,
                    "{now:?}: {got:?} vs {expected:?}"
                );
            }
        }
        // across zoom 12 the old offsets mean nothing: the frame measures from its own centre
        let at_refresh = ViewFrame::new(&ViewState { zoom: 11.9, ..ViewState::INITIAL }, [900.0, 600.0], 1.0, 0.0);
        let vf = ViewFrame::following(&ViewState { zoom: 12.1, ..ViewState::INITIAL }, [900.0, 600.0], 1.0, 0.0, &at_refresh);
        assert_eq!(vf.common_centre, vf.viewport.center);
    }

    /// The Rust structs and the WGSL structs have the same members at the same offsets.
    #[test]
    fn uniform_layouts_match_the_wgsl() {
        let module = shaders::tests::validate("lib/project.wgsl", shaders::PROJECT);
        let members = |name: &str| {
            let ty = module.types.iter().find(|(_, t)| t.name.as_deref() == Some(name)).unwrap_or_else(|| panic!("no struct {name}"));
            match &ty.1.inner {
                wgpu::naga::TypeInner::Struct { members, span } => {
                    (*span, members.iter().map(|m| (m.name.clone().expect("named"), m.offset)).collect::<Vec<_>>())
                }
                other => panic!("{name} is {other:?}"),
            }
        };
        macro_rules! rust_members {
            ($t:ty: $($f:ident),*) => {
                (size_of::<$t>() as u32, vec![$((stringify!($f).to_string(), std::mem::offset_of!($t, $f) as u32)),*])
            };
        }
        assert_eq!(
            members("Globals"),
            rust_members!(Globals: view_proj, centre_cam, centre_offset, mode, centre, camera_common, pixels_per_unit,
                camera_render, metres_per_pixel, viewport, mercator)
        );
        assert_eq!(members("Frame"), rust_members!(Frame: offset_hi, offset_lo, scale, z_offset, pad));
    }
}
