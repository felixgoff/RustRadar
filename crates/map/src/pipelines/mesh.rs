//! The `mesh` pipeline (plan 1.4.6): deck.gl's SimpleMeshLayer and its lit extruded polygons, with
//! the scene's light rig (doc 01 §4, §9.1; doc 03 §5.2) and Phong materials. Its shader is
//! `shaders/mesh.wgsl`; the `shadow` variants ([`super::shadow`]) are this pipeline (and `fill`)
//! with the `SHADOW` draw state.
//!
//! - [`Vertex`]: one vertex layout for both entries: position, normal, RGB colour (deck's mesh
//!   `colors`, 1 where a mesh has none).
//! - [`MeshPipeline::instanced`]: aircraft, gear, ship models, train cars, shadows. Vertices are in
//!   model metres (x right, y forward, z up); each [`Instance`] carries the 3 × 3 matrix from model
//!   metres to common units and the common offset of its origin, worked out on the CPU in f64 from a
//!   true east/north/up frame at the instance ([`Instance::at`], [`local_frame`]; plan 1.4.2), so
//!   deck's globe-frame half turn (`yawOffset = 180`) is not reproduced (doc 03 §20.3). Phong per
//!   fragment, as SimpleMeshLayer's fragment shader.
//! - [`MeshPipeline::local`]: the lit extruded polygons (plain buildings, item B7). Vertices are in
//!   the draw's local Mercator frame (`(u, v, h)`, [`crate::frame::ViewFrame::tile`]) with
//!   east/north/up normals, coloured by the draw's [`StyleUniform::color`]. Phong per vertex, as
//!   SolidPolygonLayer's vertex shader.
//! - [`LightRig`]: ambient white × 1, camera light `[255,248,235]` × 0.55 (a point light at the
//!   camera), key light `[255,244,225]` × 0.7 along `[-1,-2,-3]`, and
//!   [`LightRig::aim_key_light`] (G:417-445). [`LightsUniform`] is it for one frame:
//!   [`MeshPipeline::set_lights`].
//! - [`Material`]: luma.gl's Phong material (ambient, diffuse, shininess, specular colour, unlit),
//!   with [`Material::aircraft`] (`meshMaterial(depth)`, doc 01 §9.1) and [`Material::DECK_DEFAULT`].
//!   [`StyleUniform`] is one draw's material, opacity and (local entry) colour: [`Style`].
//! - Highlight: deck's `picking_filterHighlightColor`, `mix(rgb, highlight.rgb, highlight.a)` with
//!   the alpha kept for an opaque colour, before the lighting; per instance ([`Instance::highlight`],
//!   alpha 0 for none).
//! - Normals: transformed by the instance's matrix (its scale, not the inverse transpose) and
//!   renormalised, as deck's `project_normal(instanceModelMatrix * normals)` (doc 02 §14.3.6).
//! - [`LightRig::shade`]: the lighting in f64 on the CPU, for tests and for producers that need to
//!   know a face's colour.
//!
//! Lighting space. deck lights in common space in both projections, and its key light's direction
//! is a fixed common-space vector, so on the globe (zoom ≤ 12) the light comes from a different
//! side at each place on the Earth; [`LightRig::aim_key_light`] aims it only on the flat map, whose
//! common space is east, north, up. The instanced entry lights in common space as deck does; the
//! local entry in the render frame, with the key light's direction turned into it on the CPU
//! ([`LightsUniform::new`]).
//!
//! What deck 9.4 does with `material: false` on a lit layer (the earth sphere and the aircraft
//! shadows are SimpleMeshLayers given it; a flat SolidPolygonLayer is never lit): luma's
//! `ShaderInputs.setProps` reads it as `{}` and so draws with its default Phong material,
//! [`Material::DECK_DEFAULT`], lit, exactly as `material: true`; only an `unlit: true` material
//! leaves the colour alone. Checked by running luma's `ShaderInputs` on `{ phongMaterial: false }`
//! under node, and against today's app: the earth sphere's lit colour at the pole of the reference
//! picture (test `material_false_is_lit_by_decks_default`). The specs read `material: false` as
//! unlit. The test `building_roofs_match_the_reference_picture` checks the rig, `aim_key_light`
//! and the Phong formula against the plain buildings' roofs of the reference picture at V4.

use std::num::NonZeroU64;
use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use glam::{DMat3, DVec3};
use rustradar_core::num::js_round;

use super::{Desc, build};
use crate::frame::{FrameSlot, RENDER_FRAME_MAX_LATITUDE, ViewFrame};
use crate::layers::{DrawCx, Init};
use crate::shaders;
use crate::state::DrawState;

/// `shaders/mesh.wgsl`.
pub const SHADER: &str = include_str!("../shaders/mesh.wgsl");

/// The bind group of the lights (binding 0) and the draw's [`StyleUniform`] (binding 1).
pub const STYLE_GROUP: u32 = 2;

/// The key light's usual direction (`KEY_LIGHT`, G:417), before deck normalises it.
pub const KEY_LIGHT: [f64; 3] = [-1.0, -2.0, -3.0];

/// One vertex of a mesh.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    /// Instanced: model metres (x right, y forward, z up). Local: `u` east and `v` south in the
    /// frame's units, and the height in metres.
    pub position: [f32; 3],
    /// Instanced: model space. Local: east, north, up. Need not be unit length.
    pub normal: [f32; 3],
    /// RGB, 0–1 (deck multiplies it by the instance colour).
    pub color: [f32; 3],
}

impl Vertex {
    /// White, deck's colour for a mesh without `colors`.
    pub const WHITE: [f32; 3] = [1.0; 3];

    /// The vertex buffer layout: position, normal and colour at locations 0, 1, 2.
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3],
    };

    /// Vertices from flat arrays of `x, y, z` positions and normals and, if the mesh has them,
    /// `r, g, b` colours (0–1; white where `colors` is `None`), as `scene` keeps its meshes.
    ///
    /// # Panics
    /// If the arrays' lengths disagree.
    pub fn from_arrays(positions: &[f32], normals: &[f32], colors: Option<&[f32]>) -> Vec<Vertex> {
        assert_eq!(positions.len() % 3, 0, "positions come in threes");
        assert_eq!(positions.len(), normals.len(), "one normal per position");
        if let Some(colors) = colors {
            assert_eq!(positions.len(), colors.len(), "one colour per position");
        }
        (0..positions.len() / 3)
            .map(|i| {
                let three = |a: &[f32]| [a[3 * i], a[3 * i + 1], a[3 * i + 2]];
                Vertex { position: three(positions), normal: three(normals), color: colors.map_or(Self::WHITE, three) }
            })
            .collect()
    }
}

/// One instance of an instanced mesh: where and how its model lands, in common space.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Instance {
    /// The columns of the matrix from model metres to common units.
    pub columns: [[f32; 3]; 3],
    /// The common offset of the model's origin ([`ViewFrame::common_offset`]).
    pub offset: [f32; 3],
    /// deck's `getColor`, RGBA bytes; the RGB multiplies the vertex colours.
    pub color: [u8; 4],
    /// The hover highlight (deck's `highlightColor` on the hovered instance), alpha 0 for none.
    pub highlight: [u8; 4],
}

impl Instance {
    /// No highlight.
    pub const NO_HIGHLIGHT: [u8; 4] = [0; 4];

    /// The instance buffer layout: the three columns, the offset, the colour and the highlight at
    /// locations 3 to 8.
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: size_of::<Instance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &wgpu::vertex_attr_array![
            3 => Float32x3, 4 => Float32x3, 5 => Float32x3, 6 => Float32x3, 7 => Unorm8x4, 8 => Unorm8x4
        ],
    };

    /// A model placed at `[lon, lat, metres]` in `view`: `model` takes model metres to metres east,
    /// north and up at that point (deck's `getOrientation` and `getScale`, [`orientation_matrix`],
    /// or the upper 3 × 3 of `getTransformMatrix`), and `translation` (metres east, north, up) moves
    /// it (deck's `getTranslation`). deck's `pos = M · p + translation`, in metres, then to common
    /// units ([`local_frame`]).
    pub fn at(view: &ViewFrame, [lon, lat, h]: [f64; 3], model: DMat3, translation: DVec3, color: [u8; 4], highlight: [u8; 4]) -> Instance {
        let frame = local_frame(view, [lon, lat]);
        let linear = frame * model;
        let offset = view.common_offset([lon, lat], h) + (frame * translation).as_vec3();
        Instance {
            columns: [linear.x_axis, linear.y_axis, linear.z_axis].map(|c| c.as_vec3().into()),
            offset: offset.into(),
            color,
            highlight,
        }
    }

    /// [`Self::at`] with a column-major 4 × 4 `transform` (deck's `getTransformMatrix`, such as
    /// `core::sun::shadow_matrix`): its upper 3 × 3 is the model matrix and its last column's
    /// `x, y, z` the translation, as deck's `getExtendedMat3FromMat4` reads it.
    pub fn from_transform(view: &ViewFrame, position: [f64; 3], transform: &[f64; 16], color: [u8; 4], highlight: [u8; 4]) -> Instance {
        let t = transform;
        let model = DMat3::from_cols(DVec3::new(t[0], t[1], t[2]), DVec3::new(t[4], t[5], t[6]), DVec3::new(t[8], t[9], t[10]));
        Self::at(view, position, model, DVec3::new(t[12], t[13], t[14]), color, highlight)
    }
}

/// East, north and up at `[lon, lat]` in `view`'s common space, as the columns of a matrix, each
/// one common unit per metre long: model metres to common units for a mesh standing there.
///
/// The scale is deck's `project_size`: on the globe the globe's constant (256 units for
/// 6 370 972 m), on the flat map the units per metre at the view centre (deck's
/// `commonUnitsPerMeter` at its coordinate origin, the view centre in its auto-offset mode), for
/// every instance (doc 03 §20.4: the size of an aircraft far from the centre is as deck draws it).
pub fn local_frame(view: &ViewFrame, [lon, lat]: [f64; 2]) -> DMat3 {
    let units_per_metre = view.viewport.distance_scales.units_per_meter[2];
    let enu = if view.is_globe() {
        let (sin_lon, cos_lon) = lon.to_radians().sin_cos();
        let (sin_lat, cos_lat) = lat.to_radians().sin_cos();
        // common space: x = sin λ cos φ, y = −cos λ cos φ, z = sin φ
        DMat3::from_cols(
            DVec3::new(cos_lon, sin_lon, 0.0),
            DVec3::new(-sin_lat * sin_lon, sin_lat * cos_lon, cos_lat),
            DVec3::new(cos_lat * sin_lon, -cos_lat * cos_lon, sin_lat),
        )
    } else {
        DMat3::IDENTITY
    };
    enu * units_per_metre
}

/// deck's instance rotation and scale (`calculateTransformMatrix`, `@deck.gl/mesh-layers`
/// `utils/matrix.ts`; doc 03 §5.1): `orientation` is what deck calls `[pitch, yaw, roll]`, in
/// degrees, `scale` per model axis: `R = Rz(o[1]) · Ry(o[0]) · Rx(o[2])`, then `diag(scale)`. deck's
/// "pitch" turns about the model's y axis and its "roll" about x, so the app passes
/// `[bank, −heading, pitch]` for a nose along +y (bank drops the right wing, pitch lifts the nose).
pub fn orientation_matrix(orientation: [f64; 3], [scx, scy, scz]: [f64; 3]) -> DMat3 {
    let [pitch, yaw, roll] = orientation.map(f64::to_radians);
    let (sr, cr) = roll.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sw, cw) = yaw.sin_cos();
    DMat3::from_cols(
        DVec3::new(scx * cw * cp, scx * sw * cp, scx * -sp),
        DVec3::new(scy * (-sw * cr + cw * sp * sr), scy * (cw * cr + sw * sp * sr), scy * cp * sr),
        DVec3::new(scz * (sw * sr + cw * sp * cr), scz * (-cw * sr + sw * sp * cr), scz * cp * cr),
    )
}

/// luma.gl's Phong material (`phongMaterial` uniforms; deck's `material` prop).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    /// How much of the ambient light the surface takes.
    pub ambient: f64,
    /// How much of each light's Lambert term.
    pub diffuse: f64,
    pub shininess: f64,
    /// The highlights' colour, 0–255 (divided by 255 in the shader, luma's `floatColors_normalize`).
    pub specular_color: [f64; 3],
    /// The surface colour as it is, no light at all.
    pub unlit: bool,
}

impl Material {
    /// luma's defaults, which deck 9.4 also draws `material: true` and `material: false` with.
    pub const DECK_DEFAULT: Material =
        Material { ambient: 0.35, diffuse: 0.6, shininess: 32.0, specular_color: [38.25; 3], unlit: false };

    /// luma's `unlit`: the surface colour as it is.
    pub const UNLIT: Material = Material { unlit: true, ..Self::DECK_DEFAULT };

    /// The aircraft's material at depth 1 (`MESH_MATERIAL`, G:398): most of the light ambient, so
    /// white aircraft read as white against the dark map.
    pub const AIRCRAFT: Material =
        Material { ambient: 0.72, diffuse: 0.42, shininess: 48.0, specular_color: [255.0, 236.0, 200.0], unlit: false };

    /// The aircraft's material at model depth `depth` (`meshMaterial`, G:401-413; doc 01 §9.1): lit
    /// at depth 1, the flat colour at depth 0 (ambient 1, no diffuse, no highlight).
    pub fn aircraft(depth: f64) -> Material {
        let m = Self::AIRCRAFT;
        Material {
            ambient: 1.0 + (m.ambient - 1.0) * depth,
            diffuse: m.diffuse * depth,
            shininess: m.shininess,
            specular_color: m.specular_color.map(|c| js_round(c * depth)),
            unlit: false,
        }
    }
}

/// One light: a colour in bytes and an intensity (deck's light props).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light {
    pub color: [f64; 3],
    pub intensity: f64,
}

impl Light {
    /// What the shader gets: `color / 255 · intensity` (luma's `convertColor`).
    pub fn normalized(&self) -> [f64; 3] {
        self.color.map(|c| c / 255.0 * self.intensity)
    }
}

/// The scene's lights (deck's `LightingEffect`, G:417-427): ambient, a camera light and the key
/// light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightRig {
    /// `AmbientLight`, white × 1.
    pub ambient: Light,
    /// `CameraLight`, `[255,248,235]` × 0.55: a point light at the camera, no attenuation.
    pub camera: Light,
    /// The `DirectionalLight` `keyLight`, `[255,244,225]` × 0.7.
    pub key: Light,
    /// The direction the key light shines along, unit length, in common space: [`key_light_unit`]
    /// unless aimed ([`Self::aim_key_light`]).
    pub key_direction: [f64; 3],
}

impl Default for LightRig {
    fn default() -> Self {
        Self::new()
    }
}

impl LightRig {
    /// The rig with the key light in its usual direction.
    pub fn new() -> Self {
        LightRig {
            ambient: Light { color: [255.0, 255.0, 255.0], intensity: 1.0 },
            camera: Light { color: [255.0, 248.0, 235.0], intensity: 0.55 },
            key: Light { color: [255.0, 244.0, 225.0], intensity: 0.7 },
            key_direction: key_light_unit(),
        }
    }

    /// The rig with the key light aimed ([`Self::aim_key_light`]).
    pub fn aimed(to_sun: [f64; 3], weight: f64) -> Self {
        let mut rig = Self::new();
        rig.aim_key_light(to_sun, weight);
        rig
    }

    /// Points the key light along the real sun's rays, by `weight` (0..1) of the way from its usual
    /// direction (`aimKeyLight`, G:428-445; doc 01 §9.1), so sunlit walls face the sun and agree
    /// with the shadows. `to_sun` is the direction toward the sun as east, north, up (only the flat
    /// map's common space is that, so the app aims only there: weight
    /// `buildings && flat ? sunlight(up) · clamp(zoom − 12, 0, 1) : 0`). From the sun's bearing,
    /// but no steeper than the usual light: a high sun straight onto the roofs would leave every
    /// wall dark. Always from the usual direction, not from the last aim.
    pub fn aim_key_light(&mut self, to_sun: [f64; 3], weight: f64) {
        let unit = key_light_unit();
        let across = or_one(to_sun[0].hypot(to_sun[1]));
        let up = js_min(to_sun[2], key_light_up());
        let flat = (1.0 - up * up).sqrt() / across;
        let ray = [-to_sun[0] * flat, -to_sun[1] * flat, -up];
        let mut d = [0.0; 3];
        for i in 0..3 {
            d[i] = unit[i] + (ray[i] - unit[i]) * weight;
        }
        let n = or_one((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt());
        self.key_direction = d.map(|c| c / n);
    }

    /// The colour the shader gives a fragment (f64, 0–1), the CPU twin of `mesh.wgsl`'s `shade`:
    /// `surface` (0–1, highlight already mixed in) with unit `normal`, `to_camera` the vector from
    /// the fragment to the camera, and the key light's direction, all in one space (common space
    /// for the instanced entry; the render frame's key direction for the local entry).
    pub fn shade(&self, material: &Material, surface: [f64; 3], normal: DVec3, to_camera: DVec3, key_direction: DVec3) -> [f64; 3] {
        if material.unlit {
            return surface;
        }
        let surface = DVec3::from_array(surface);
        let view = to_camera.normalize_or_zero();
        let one = |l: DVec3, color: [f64; 3]| {
            let halfway = (l + view).normalize_or_zero();
            let mut lambertian = l.dot(normal);
            let mut specular = 0.0;
            if lambertian > 0.0 {
                let angle = normal.dot(halfway).max(0.0);
                if angle > 0.0 {
                    specular = angle.powf(material.shininess);
                }
            }
            lambertian = lambertian.max(0.0);
            (lambertian * material.diffuse * surface + specular * DVec3::from_array(material.specular_color) / 255.0)
                * DVec3::from_array(color)
        };
        let mut color = material.ambient * surface * DVec3::from_array(self.ambient.normalized());
        color += one(view, self.camera.normalized());
        color += one(-key_direction, self.key.normalized());
        color.to_array()
    }
}

/// The key light's usual direction, normalised as deck's `DirectionalLight` does it (math.gl
/// `Vector3.normalize`: times the reciprocal of the length).
pub fn key_light_unit() -> [f64; 3] {
    let [x, y, z] = KEY_LIGHT;
    let len = x * x + y * y + z * z;
    let scale = if len > 0.0 { 1.0 / len.sqrt() } else { len };
    [x * scale, y * scale, z * scale]
}

/// The sine of the usual key light's altitude, about 53° (`KEY_LIGHT_UP`, G:421).
pub fn key_light_up() -> f64 {
    -key_light_unit()[2]
}

/// JavaScript's `x || 1`.
fn or_one(x: f64) -> f64 {
    if x == 0.0 || x.is_nan() { 1.0 } else { x }
}

/// JavaScript's `Math.min(a, b)`: NaN if either is.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}

/// `mesh.wgsl`'s `Lights`: the rig for one frame (std140; the layout test compares the offsets).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct LightsUniform {
    pub ambient: [f32; 3],
    pub pad0: f32,
    pub camera: [f32; 3],
    pub pad1: f32,
    pub key: [f32; 3],
    pub pad2: f32,
    /// The key light's direction in common space.
    pub key_common: [f32; 3],
    pub pad3: f32,
    /// The same in the render frame.
    pub key_render: [f32; 3],
    pub pad4: f32,
}

impl LightsUniform {
    /// `rig` for a frame of `view`.
    pub fn new(rig: &LightRig, view: &ViewFrame) -> Self {
        let key = DVec3::from_array(rig.key_direction);
        LightsUniform {
            ambient: f32s(rig.ambient.normalized()),
            camera: f32s(rig.camera.normalized()),
            key: f32s(rig.key.normalized()),
            key_common: key.as_vec3().into(),
            key_render: (render_turn(view) * key).as_vec3().into(),
            ..Zeroable::zeroed()
        }
    }
}

/// Common space to the render frame (`lib/project.wgsl`): on the globe `Rx(φc) · Rz(−λc)` of the
/// view centre (latitude within ±89.9°, as `frame` turns it); the identity on the flat map.
pub fn render_turn(view: &ViewFrame) -> DMat3 {
    match view.viewport.globe() {
        Some(globe) => {
            let latitude = globe.latitude.clamp(-RENDER_FRAME_MAX_LATITUDE, RENDER_FRAME_MAX_LATITUDE);
            DMat3::from_rotation_x(latitude.to_radians()) * DMat3::from_rotation_z(-globe.longitude.to_radians())
        }
        None => DMat3::IDENTITY,
    }
}

fn f32s(v: [f64; 3]) -> [f32; 3] {
    v.map(|c| c as f32)
}

/// `mesh.wgsl`'s `Style`: one draw's material, opacity and (local entry) colour.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct StyleUniform {
    /// The local entry's fill colour (deck's `getFillColor`), RGBA 0–1; instanced draws take their
    /// instances' colours instead.
    pub color: [f32; 4],
    /// `specularColor`, 0–255.
    pub specular: [f32; 3],
    pub ambient: f32,
    pub diffuse: f32,
    pub shininess: f32,
    /// deck's layer opacity, multiplying the alpha.
    pub opacity: f32,
    /// 1 for an unlit material.
    pub unlit: u32,
}

impl StyleUniform {
    /// `material` at `opacity`, with `color` (bytes) for the local entry.
    pub fn new(material: &Material, opacity: f64, color: [u8; 4]) -> Self {
        StyleUniform {
            color: color.map(|c| f32::from(c) / 255.0),
            specular: f32s(material.specular_color),
            ambient: material.ambient as f32,
            diffuse: material.diffuse as f32,
            shininess: material.shininess as f32,
            opacity: opacity as f32,
            unlit: u32::from(material.unlit),
        }
    }
}

/// One draw's style (group 2: the pipeline's lights and this [`StyleUniform`]). Use it with the
/// pipeline that made it.
pub struct Style {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
}

impl Style {
    /// Rewrites the style (a new material, opacity or colour).
    pub fn set(&self, queue: &wgpu::Queue, style: &StyleUniform) {
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(style));
    }
}

/// A mesh's vertices on the GPU, indexed (`u32`) or not (three vertices per triangle).
pub struct Shape {
    vertices: wgpu::Buffer,
    indices: Option<wgpu::Buffer>,
    /// Indices if indexed, else vertices.
    elements: u32,
}

impl Shape {
    /// Uploads `vertices` and, for an indexed mesh, three `indices` per triangle. Empty input makes
    /// a shape that draws nothing.
    pub fn new(device: &wgpu::Device, label: &str, vertices: &[Vertex], indices: Option<&[u32]>) -> Self {
        let elements = if vertices.is_empty() { 0 } else { indices.map_or(vertices.len(), <[u32]>::len) as u32 };
        Shape {
            vertices: padded_buffer(device, label, bytemuck::cast_slice(vertices), wgpu::BufferUsages::VERTEX),
            indices: indices.map(|i| padded_buffer(device, label, bytemuck::cast_slice(i), wgpu::BufferUsages::INDEX)),
            elements,
        }
    }

    /// [`Self::new`] with `u16` indices (the aircraft meshes' `indices`).
    pub fn with_u16_indices(device: &wgpu::Device, label: &str, vertices: &[Vertex], indices: &[u16]) -> Self {
        let wide: Vec<u32> = indices.iter().map(|&i| u32::from(i)).collect();
        Self::new(device, label, vertices, Some(&wide))
    }

    /// How many indices (indexed) or vertices it draws.
    pub fn elements(&self) -> u32 {
        self.elements
    }

    fn bind(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        if let Some(indices) = &self.indices {
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        }
    }

    fn draw_elements(&self, pass: &mut wgpu::RenderPass<'_>, elements: Range<u32>, instances: Range<u32>) {
        if self.indices.is_some() {
            pass.draw_indexed(elements, 0, instances);
        } else {
            pass.draw(elements, instances);
        }
    }
}

/// A buffer of `contents`, at least 16 bytes (wgpu refuses empty ones).
fn padded_buffer(device: &wgpu::Device, label: &str, contents: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    let mut padded = contents.to_vec();
    padded.resize(contents.len().max(16), 0);
    wgpu::util::DeviceExt::create_buffer_init(device, &wgpu::util::BufferInitDescriptor { label: Some(label), contents: &padded, usage })
}

/// Instances on the GPU, rewritten at each scene refresh (the buffer grows by doubling).
pub struct Instances {
    buffer: wgpu::Buffer,
    capacity: usize,
    count: u32,
    label: String,
}

impl Instances {
    const INITIAL_CAPACITY: usize = 16;

    pub fn new(device: &wgpu::Device, label: &str) -> Self {
        Instances {
            buffer: Self::allocate(device, label, Self::INITIAL_CAPACITY),
            capacity: Self::INITIAL_CAPACITY,
            count: 0,
            label: label.to_string(),
        }
    }

    fn allocate(device: &wgpu::Device, label: &str, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (capacity * size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Replaces the instances.
    pub fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, instances: &[Instance]) {
        if instances.len() > self.capacity {
            self.capacity = instances.len().next_power_of_two();
            self.buffer = Self::allocate(device, &self.label, self.capacity);
        }
        if !instances.is_empty() {
            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(instances));
        }
        self.count = instances.len() as u32;
    }

    /// How many there are.
    pub fn count(&self) -> u32 {
        self.count
    }
}

/// Which vertex entry a [`MeshPipeline`] draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// `vs` + `fs`: instanced, model metres, Phong per fragment.
    Instanced,
    /// `vs_local` + `fs_local`: local Mercator, Phong per vertex.
    Local,
}

/// The mesh pipeline for one entry and one draw-state preset, with its lights buffer.
pub struct MeshPipeline {
    pipeline: wgpu::RenderPipeline,
    entry: Entry,
    group_layout: wgpu::BindGroupLayout,
    lights: wgpu::Buffer,
}

impl MeshPipeline {
    /// The instanced pipeline (SimpleMeshLayer) with `state`: `NO_CULL` for the aircraft and gear,
    /// `SHIP` / `TRAIN` for ship models and cars, `SHADOW` for shadows ([`super::shadow::mesh`]).
    pub fn instanced(init: &Init<'_>, label: &str, state: DrawState) -> Self {
        Self::new(init, label, state, Entry::Instanced)
    }

    /// The local Mercator pipeline (extruded polygons) with `state`.
    pub fn local(init: &Init<'_>, label: &str, state: DrawState) -> Self {
        Self::new(init, label, state, Entry::Local)
    }

    fn new(init: &Init<'_>, label: &str, state: DrawState, entry: Entry) -> Self {
        let device = &init.gpu.device;
        let uniform = |binding, size: usize| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(size as u64),
            },
            count: None,
        };
        let group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh lights and style"),
            entries: &[uniform(0, size_of::<LightsUniform>()), uniform(1, size_of::<StyleUniform>())],
        });
        let lights = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("mesh lights"),
                contents: bytemuck::bytes_of(&LightsUniform::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let module = shaders::module(device, label, SHADER);
        let (vertex_entry, fragment_entry, buffers): (_, _, &[Option<wgpu::VertexBufferLayout<'_>>]) = match entry {
            Entry::Instanced => ("vs", "fs", &[Some(Vertex::LAYOUT), Some(Instance::LAYOUT)]),
            Entry::Local => ("vs_local", "fs_local", &[Some(Vertex::LAYOUT)]),
        };
        let pipeline = build(
            init,
            &Desc {
                label,
                module: &module,
                vertex_entry,
                fragment_entry,
                buffers,
                topology: wgpu::PrimitiveTopology::TriangleList,
                state,
                groups: &[&group_layout],
            },
        );
        MeshPipeline { pipeline, entry, group_layout, lights }
    }

    /// Which entry it draws with.
    pub fn entry(&self) -> Entry {
        self.entry
    }

    /// This frame's lights (every frame, in `prepare`: the render frame turns with the camera).
    pub fn set_lights(&self, queue: &wgpu::Queue, rig: &LightRig, view: &ViewFrame) {
        queue.write_buffer(&self.lights, 0, bytemuck::bytes_of(&LightsUniform::new(rig, view)));
    }

    /// A style for draws with this pipeline.
    pub fn style(&self, device: &wgpu::Device, style: &StyleUniform) -> Style {
        let buffer = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("mesh style"),
                contents: bytemuck::bytes_of(style),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh lights and style"),
            layout: &self.group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.lights.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: buffer.as_entire_binding() },
            ],
        });
        Style { buffer, group }
    }

    /// Draws every instance of `instances` with all of `shape` (instanced entry).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, style: &Style, shape: &Shape, instances: &Instances) {
        self.draw_range(pass, style, shape, 0..shape.elements(), instances);
    }

    /// Draws every instance with the `elements` (indices, or vertices if unindexed) of `shape`: one
    /// livery part of an aircraft mesh (`ModelLod::parts`).
    pub fn draw_range(&self, pass: &mut wgpu::RenderPass<'_>, style: &Style, shape: &Shape, elements: Range<u32>, instances: &Instances) {
        debug_assert_eq!(self.entry, Entry::Instanced, "a local pipeline draws with draw_local");
        if instances.count == 0 || elements.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(STYLE_GROUP, &style.group, &[]);
        shape.bind(pass);
        pass.set_vertex_buffer(1, instances.buffer.slice(..));
        shape.draw_elements(pass, elements, 0..instances.count);
    }

    /// Draws `shape` in the local Mercator `frame` (local entry).
    pub fn draw_local(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, frame: FrameSlot, style: &Style, shape: &Shape) {
        debug_assert_eq!(self.entry, Entry::Local, "an instanced pipeline draws with draw");
        if shape.elements == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(STYLE_GROUP, &style.group, &[]);
        cx.bind_frame(pass, frame);
        shape.bind(pass);
        shape.draw_elements(pass, 0..shape.elements, 0..1);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use glam::DVec2;
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::{ViewState, lng_lat_to_world};

    use super::*;
    use crate::frame::model;
    use crate::gpu::Gpu;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::{Layer, PrepareCx};
    use crate::pipelines::fill::tests::wgsl_members;
    use crate::renderer::{MapRenderer, SceneInput};

    fn close(a: [f64; 3], b: [f64; 3], tolerance: f64) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= tolerance)
    }

    #[test]
    fn layouts_match_the_wgsl() {
        assert_eq!(size_of::<Vertex>(), 36);
        assert_eq!(Vertex::LAYOUT.array_stride, 36);
        assert_eq!(size_of::<Instance>(), 56);
        assert_eq!(std::mem::offset_of!(Instance, offset), 36);
        assert_eq!(std::mem::offset_of!(Instance, color), 48);
        assert_eq!(std::mem::offset_of!(Instance, highlight), 52);
        let offsets: Vec<u64> = Instance::LAYOUT.attributes.iter().map(|a| a.offset).collect();
        assert_eq!(offsets, [0, 12, 24, 36, 48, 52]);
        let (size, members) = wgsl_members(SHADER, "Lights");
        assert_eq!(size as usize, size_of::<LightsUniform>());
        let rust = [
            std::mem::offset_of!(LightsUniform, ambient),
            std::mem::offset_of!(LightsUniform, camera),
            std::mem::offset_of!(LightsUniform, key),
            std::mem::offset_of!(LightsUniform, key_common),
            std::mem::offset_of!(LightsUniform, key_render),
        ];
        assert_eq!(members.iter().map(|(_, o)| *o as usize).collect::<Vec<_>>(), rust);
        let (size, members) = wgsl_members(SHADER, "Style");
        assert_eq!(size as usize, size_of::<StyleUniform>());
        let rust = [
            ("color", std::mem::offset_of!(StyleUniform, color)),
            ("specular", std::mem::offset_of!(StyleUniform, specular)),
            ("ambient", std::mem::offset_of!(StyleUniform, ambient)),
            ("diffuse", std::mem::offset_of!(StyleUniform, diffuse)),
            ("shininess", std::mem::offset_of!(StyleUniform, shininess)),
            ("opacity", std::mem::offset_of!(StyleUniform, opacity)),
            ("unlit", std::mem::offset_of!(StyleUniform, unlit)),
        ];
        assert_eq!(members, rust.map(|(n, o)| (n.to_string(), o as u32)));
    }

    #[test]
    fn vertices_come_from_flat_arrays() {
        let v = Vertex::from_arrays(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[0.0, 0.0, 1.0, 0.0, 1.0, 0.0], None);
        assert_eq!(v.len(), 2);
        assert_eq!(v[1], Vertex { position: [4.0, 5.0, 6.0], normal: [0.0, 1.0, 0.0], color: Vertex::WHITE });
        let c = Vertex::from_arrays(&[0.0; 3], &[0.0; 3], Some(&[0.25, 0.5, 0.75]));
        assert_eq!(c[0].color, [0.25, 0.5, 0.75]);
    }

    /// The light rig of G:417-427 and the colours the shader gets (luma's `convertColor`).
    #[test]
    fn the_light_rig_is_the_apps() {
        let rig = LightRig::new();
        assert_eq!(rig.ambient.normalized(), [1.0; 3]);
        assert!(close(rig.camera.normalized(), [0.55, 248.0 / 255.0 * 0.55, 235.0 / 255.0 * 0.55], 1e-15));
        assert!(close(rig.key.normalized(), [0.7, 244.0 / 255.0 * 0.7, 225.0 / 255.0 * 0.7], 1e-15));
        // math.gl's Vector3([-1, -2, -3]).normalize(), printed by node
        assert_eq!(rig.key_direction, [-0.2672612419124244, -0.5345224838248488, -0.8017837257372732]);
        assert_eq!(key_light_up(), 0.8017837257372732);
        // about 53° above the horizon
        assert!((key_light_up().asin().to_degrees() - 53.3).abs() < 0.1);
    }

    /// `aimKeyLight` against the TypeScript's own function (G:428-445, with math.gl's `Vector3`) run
    /// under node on these inputs.
    #[test]
    // 0.70710678 is the input node was given, not an approximation of 1/√2 to correct
    #[allow(clippy::approx_constant)]
    fn aim_key_light_matches_the_typescript() {
        let cases: [([f64; 3], f64, [f64; 3]); 9] = [
            ([0.3, 0.4, 0.866], 1.0, [-0.35856858280031806, -0.47809144373375745, -0.8017837257372732]),
            ([0.3, 0.4, 0.866], 0.5, [-0.313366544105043, -0.5070377192990526, -0.8029409444309572]),
            ([0.6, -0.8, 0.0], 1.0, [-0.6, 0.8, 0.0]),
            ([-0.5, 0.1, 0.2], 0.25, [0.05018091694663873, -0.5667882284173824, -0.8223338614588843]),
            ([0.0, 0.0, 1.0], 1.0, [0.0, 0.0, -1.0]),
            ([0.0, 0.0, -1.0], 1.0, [0.0, 0.0, 1.0]),
            ([0.1, 0.2, 0.3], 0.0, [-0.2672612419124244, -0.5345224838248488, -0.8017837257372732]),
            ([0.70710678, 0.0, 0.70710678], 0.75, [-0.6265042077347345, -0.1402006055739734, -0.7667048114413813]),
            ([0.0, 0.0, 0.0], 0.5, [-0.2672612419124244, -0.5345224838248488, -0.8017837257372732]),
        ];
        for (to_sun, weight, expected) in cases {
            let mut rig = LightRig::new();
            // aiming twice gives the same: it always starts from the usual direction
            rig.aim_key_light([0.9, 0.1, 0.3], 1.0);
            rig.aim_key_light(to_sun, weight);
            assert!(close(rig.key_direction, expected, 2e-16), "{to_sun:?} {weight}: {:?} against {expected:?}", rig.key_direction);
            assert_eq!(LightRig::aimed(to_sun, weight), rig);
        }
    }

    /// `meshMaterial(depth)` (G:401-413): the flat colour at depth 0, `MESH_MATERIAL` at 1.
    #[test]
    fn aircraft_material_by_depth() {
        let flat = Material::aircraft(0.0);
        assert_eq!((flat.ambient, flat.diffuse, flat.shininess, flat.specular_color), (1.0, 0.0, 48.0, [0.0; 3]));
        assert_eq!(Material::aircraft(1.0), Material::AIRCRAFT);
        let half = Material::aircraft(0.5);
        assert_eq!((half.ambient, half.diffuse), (0.86, 0.21));
        // Math.round(255 * 0.5) = 128, Math.round(236 * 0.5) = 118, Math.round(200 * 0.5) = 100
        assert_eq!(half.specular_color, [128.0, 118.0, 100.0]);
        assert_eq!(Material::aircraft(0.3).specular_color, [77.0, 71.0, 60.0]);
    }

    /// At depth 0 the lighting gives the surface colour back exactly, whatever the normal; an unlit
    /// material too; a lit face toward the key light is brighter than one away from it.
    #[test]
    fn shading_on_the_cpu() {
        let rig = LightRig::new();
        let key = DVec3::from_array(rig.key_direction);
        let surface = [0.8, 0.6, 0.4];
        for normal in [DVec3::Z, DVec3::X, -DVec3::Y, DVec3::new(0.6, 0.0, 0.8)] {
            for material in [Material::aircraft(0.0), Material::UNLIT] {
                assert_eq!(rig.shade(&material, surface, normal, DVec3::new(0.0, -3.0, 4.0), key), surface);
            }
        }
        let lit = Material::aircraft(1.0);
        let toward = rig.shade(&lit, surface, -key, DVec3::Z, key);
        let away = rig.shade(&lit, surface, key, DVec3::Z, key);
        assert!(toward[0] > away[0] + 0.1, "{toward:?} {away:?}");
        // away from every light: ambient only
        let dark = rig.shade(&lit, surface, -DVec3::Z, DVec3::Z, -DVec3::Z);
        assert!(close(dark, surface.map(|c| c * 0.72), 1e-15), "{dark:?}");
    }

    /// deck's rotation as the app uses it (`[bank, −heading, pitch]`, doc 03 §5.1): the yaw turns
    /// the nose (+y) clockwise to the heading, the third angle lifts the nose, the first drops the
    /// right wing; scale per model axis.
    #[test]
    fn orientation_turns_as_decks_matrix() {
        let east = orientation_matrix([0.0, -90.0, 0.0], [1.0; 3]) * DVec3::Y;
        assert!(east.abs_diff_eq(DVec3::X, 1e-15), "{east}");
        let nose = orientation_matrix([0.0, 0.0, 10.0], [1.0; 3]) * DVec3::Y;
        assert!(nose.z > 0.17 && nose.z < 0.18, "{nose}");
        let wing = orientation_matrix([20.0, 0.0, 0.0], [1.0; 3]) * DVec3::X;
        assert!(wing.z < -0.34 && wing.z > -0.35, "{wing}");
        // the order: Rz(yaw) · Ry(bank) · Rx(pitch)
        let composed = DMat3::from_rotation_z(30f64.to_radians()) * DMat3::from_rotation_y(20f64.to_radians()) * DMat3::from_rotation_x(10f64.to_radians());
        assert!(orientation_matrix([20.0, 30.0, 10.0], [1.0; 3]).abs_diff_eq(composed, 1e-15));
        let scaled = orientation_matrix([0.0; 3], [2.0, 3.0, 4.0]);
        assert_eq!(scaled, DMat3::from_diagonal(DVec3::new(2.0, 3.0, 4.0)));
        // the shadow matrix of core::sun starts from the same rotation: with the sun overhead it is
        // the rotation with its z row zeroed
        let (o, s) = ([5.0, -30.0, 12.0], [1.5, 1.5, 0.7]);
        let m = orientation_matrix(o, s);
        let shadow = rustradar_core::sun::shadow_matrix(o, s, [0.0, 0.0, 1.0], 100.0, 0.0);
        for c in 0..3 {
            assert!((shadow[4 * c] - m.col(c).x).abs() < 1e-15 && (shadow[4 * c + 1] - m.col(c).y).abs() < 1e-15);
        }
    }

    /// The local frame is east, north and up at the point, one common unit per metre long, in both
    /// projections (finite differences of `project_position`).
    #[test]
    fn local_frames_are_east_north_up() {
        for view in [
            ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, pitch: 40.0, bearing: 20.0, position: None },
            ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 15.0, pitch: 45.0, bearing: 30.0, position: None },
        ] {
            let vf = ViewFrame::new(&view, [400.0, 300.0], 1.0, 0.0);
            for [lon, lat] in [[view.longitude, view.latitude], [view.longitude + 0.01, view.latitude - 0.005]] {
                let f = local_frame(&vf, [lon, lat]);
                let p = |lon: f64, lat: f64, h: f64| vf.viewport.project_position([lon, lat, h]);
                let here = p(lon, lat, 0.0);
                let step = 1e-6;
                let metres_per_degree = 6_370_972.0f64.to_radians();
                let east = (p(lon + step, lat, 0.0) - here).normalize();
                let north = (p(lon, lat + step, 0.0) - here).normalize();
                let up = (p(lon, lat, 10.0) - here).normalize();
                for (col, dir) in [(f.x_axis, east), (f.y_axis, north), (f.z_axis, up)] {
                    assert!(col.normalize().abs_diff_eq(dir, 1e-6), "{view:?}: {col} against {dir}");
                    let upm = vf.viewport.distance_scales.units_per_meter[2];
                    assert!((col.length() / upm - 1.0).abs() < 1e-12);
                }
                if vf.is_globe() {
                    // on the globe a metre east is as long as a metre on the sphere
                    let d = (p(lon + step, lat, 0.0) - here).length() / (step * metres_per_degree * lat.to_radians().cos());
                    assert!((d / f.x_axis.length() - 1.0).abs() < 1e-6);
                }
            }
        }
    }

    /// An instance from a 4 × 4 transform (the shadow matrix) is the one placed with its upper 3 × 3
    /// and its last column's translation; its origin lands at the common offset of its position.
    #[test]
    fn instances_from_a_transform() {
        let view = ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 14.0, pitch: 30.0, bearing: 10.0, position: None };
        let vf = ViewFrame::new(&view, [300.0, 200.0], 1.0, 0.0);
        let t = rustradar_core::sun::shadow_matrix([2.0, -40.0, 5.0], [3.0, 3.0, 1.2], [0.2, -0.4, 0.8], 250.0, 0.0);
        let position = [17.92, 59.65, 0.0];
        let color = [4, 8, 16, 100];
        let a = Instance::from_transform(&vf, position, &t, color, Instance::NO_HIGHLIGHT);
        let model = DMat3::from_cols(DVec3::new(t[0], t[1], t[2]), DVec3::new(t[4], t[5], t[6]), DVec3::new(t[8], t[9], t[10]));
        let b = Instance::at(&vf, position, model, DVec3::new(t[12], t[13], t[14]), color, Instance::NO_HIGHLIGHT);
        assert_eq!(a, b);
        let plain = Instance::at(&vf, position, DMat3::IDENTITY, DVec3::ZERO, color, [1, 2, 3, 4]);
        assert_eq!(plain.offset, <[f32; 3]>::from(vf.common_offset([17.92, 59.65], 0.0)));
        assert_eq!(plain.highlight, [1, 2, 3, 4]);
        // the flat map's columns are a metre in common units, at the view centre
        let upm = vf.viewport.distance_scales.units_per_meter[2] as f32;
        assert_eq!(plain.columns, [[upm, 0.0, 0.0], [0.0, upm, 0.0], [0.0, 0.0, upm]]);
        // the shadow's translation moves it away from the sun
        let shift = DVec3::from_array(a.offset.map(f64::from)) - DVec3::from_array(plain.offset.map(f64::from));
        assert!(shift.x < 0.0 && shift.y > 0.0 && shift.z == 0.0, "{shift}");
    }

    /// The lights' render-frame direction is the common one turned as `frame` turns the globe.
    #[test]
    fn lights_turn_into_the_render_frame() {
        let rig = LightRig::new();
        let flat = ViewFrame::new(&ViewState { longitude: 18.0, latitude: 59.0, zoom: 14.0, ..ViewState::INITIAL }, [100.0, 100.0], 1.0, 0.0);
        let u = LightsUniform::new(&rig, &flat);
        assert_eq!(u.key_common, u.key_render);
        let globe = ViewFrame::new(&ViewState { longitude: 18.0, latitude: 59.0, zoom: 8.0, ..ViewState::INITIAL }, [100.0, 100.0], 1.0, 0.0);
        let u = LightsUniform::new(&rig, &globe);
        // the turn takes the centre's up to −y and its east to +x (lib/project.wgsl)
        let turn = render_turn(&globe);
        let f = local_frame(&globe, [18.0, 59.0]).mul_scalar(1.0 / globe.viewport.distance_scales.units_per_meter[2]);
        assert!((turn * f.z_axis).abs_diff_eq(-DVec3::Y, 1e-12));
        assert!((turn * f.x_axis).abs_diff_eq(DVec3::X, 1e-12));
        assert!((turn * f.y_axis).abs_diff_eq(DVec3::Z, 1e-12));
        let expected = turn * DVec3::from_array(rig.key_direction);
        assert!(u.key_render.iter().zip(expected.to_array()).all(|(a, b)| (f64::from(*a) - b).abs() < 1e-7));
    }

    /// The light rig, `aim_key_light`, luma's Phong and a material against today's app itself: the
    /// roofs of the plain buildings at Arlanda in the reference picture `dark-v4-1440x900.png`
    /// (view V4 at 2026-06-21 10:00 UTC, `docs/port/reference/README.md`) are the colour the CPU
    /// twin of the shader gives a roof there, with the key light aimed at that sun as the app aims
    /// it over buildings (weight `sunlight(up) · clamp(zoom − 12, 0, 1)`, G:2723-2724), the
    /// buildings' material and colour (`BUILDING_MATERIAL`, `BUILDING_COLOR`, G:972-978), and the
    /// camera light from where deck's camera is. (The GPU agrees with the twin: the tests above.)
    #[test]
    fn building_roofs_match_the_reference_picture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference/dark-v4-1440x900.png");
        let reference = image::open(&path).expect("reference picture").to_rgb8();
        let view = ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 13.5, pitch: 45.0, bearing: 30.0, position: None };
        let vf = ViewFrame::new(&view, [1440.0, 900.0], 1.0, 0.0);
        let now = 1_782_036_000_000.0;
        let to_sun = rustradar_core::sun::local_sun(rustradar_core::sun::sun_direction(now), view.latitude, view.longitude);
        let weight = rustradar_core::sun::sunlight(to_sun[2]) * (view.zoom - 12.0).clamp(0.0, 1.0);
        assert_eq!(weight, 1.0);
        let rig = LightRig::aimed(to_sun, weight);
        let material = Material { ambient: 0.42, diffuse: 0.6, shininess: 16.0, specular_color: [60.0, 66.0, 80.0], unlit: false };
        let color = [58.0, 68.0, 84.0].map(|c: f64| c / 255.0);
        let camera = glam::Vec3::from(vf.globals.camera_common).as_dvec3();
        // two hangar roofs south-east of the terminals, some 10 to 20 m up
        for [x, y] in [[1145u32, 712u32], [1060, 760]] {
            let got = reference.get_pixel(x, y).0;
            for h in [10.0, 20.0] {
                let lnglat = vf.viewport.unproject([f64::from(x) + 0.5, f64::from(y) + 0.5], h);
                let at = vf.common_offset(lnglat, h).as_dvec3();
                let expected = rig.shade(&material, color, DVec3::Z, camera - at, DVec3::from_array(rig.key_direction)).map(|c| c * 255.0);
                eprintln!("roof at ({x}, {y}), {h} m: {got:?} in the picture, {expected:.1?} by the twin");
                assert!(got.iter().zip(expected).all(|(g, e)| (f64::from(*g) - e).abs() <= 1.0), "({x}, {y}): {got:?} against {expected:?}");
            }
        }
        // the usual key light would have lit them otherwise
        let usual = LightRig::new().shade(&material, color, DVec3::Z, DVec3::new(0.0, -0.6, 0.8), DVec3::from_array(key_light_unit()));
        assert!((usual[0] * 255.0 - 63.4).abs() > 5.0, "{usual:?}");
    }

    /// deck 9.4 lights a SimpleMeshLayer given `material: false` with luma's default material: the
    /// earth sphere (`GROUND` `[50,50,50]`, G:451-464) where it shows past the land's ±85.06°, at
    /// the north pole in the reference picture `dark-v1-1440x900.png`, has the colour the CPU twin
    /// gives it with [`Material::DECK_DEFAULT`] and the usual rig (its normals are the sphere's,
    /// in common space), not `[50,50,50]`.
    #[test]
    fn material_false_is_lit_by_decks_default() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference/dark-v1-1440x900.png");
        let reference = image::open(&path).expect("reference picture").to_rgb8();
        let view = ViewState { longitude: 10.0, latitude: 35.0, zoom: 1.9, pitch: 0.0, bearing: 0.0, position: None };
        let vf = ViewFrame::new(&view, [1440.0, 900.0], 1.0, 0.0);
        let rig = LightRig::new();
        let camera = glam::Vec3::from(vf.globals.camera_common).as_dvec3();
        // the sphere of 6.3e6 m lies 70 972 m below deck's globe
        let depth = 6.3e6 - 6_370_972.0;
        for x in [700u32, 720, 740] {
            let y = 185;
            let lnglat = vf.viewport.unproject([f64::from(x) + 0.5, f64::from(y) + 0.5], depth);
            assert!(lnglat[1] > 85.06, "{lnglat:?} is not past the land");
            let p = vf.viewport.project_position([lnglat[0], lnglat[1], depth]);
            let lit = rig.shade(&Material::DECK_DEFAULT, [50.0 / 255.0; 3], p.normalize(), camera - (p - vf.common_centre), DVec3::from_array(rig.key_direction));
            let expected = lit.map(|c| c * 255.0);
            let got = reference.get_pixel(x, y).0;
            eprintln!("earth at ({x}, {y}): {got:?} in the picture, {expected:.1?} by the twin");
            assert!(got.iter().zip(expected).all(|(g, e)| (f64::from(*g) - e).abs() <= 1.0), "({x}, {y}): {got:?} against {expected:?}");
            assert_ne!(got, [50, 50, 50]);
        }
    }

    /// One placed mesh of a test scene.
    pub(crate) struct Placed {
        pub(crate) position: [f64; 3],
        pub(crate) model: DMat3,
        pub(crate) translation: DVec3,
        pub(crate) color: [u8; 4],
        pub(crate) highlight: [u8; 4],
    }

    struct Draw {
        shape: Shape,
        style: Style,
        placed: Vec<Placed>,
        instances: Instances,
    }

    /// A test layer: shapes drawn instanced with one [`MeshPipeline`], placed in `prepare` from
    /// geographic positions as a scene builder would, in the order added.
    pub(crate) struct Meshes {
        pipeline: MeshPipeline,
        rig: LightRig,
        draws: Vec<Draw>,
    }

    impl Meshes {
        pub(crate) fn new(init: &Init<'_>, state: DrawState, rig: LightRig) -> Self {
            Meshes { pipeline: MeshPipeline::instanced(init, "test mesh", state), rig, draws: Vec::new() }
        }

        pub(crate) fn add(&mut self, init: &Init<'_>, vertices: &[Vertex], indices: Option<&[u32]>, style: StyleUniform, placed: Vec<Placed>) {
            let device = &init.gpu.device;
            self.draws.push(Draw {
                shape: Shape::new(device, "test shape", vertices, indices),
                style: self.pipeline.style(device, &style),
                placed,
                instances: Instances::new(device, "test instances"),
            });
        }
    }

    impl Layer for Meshes {
        fn name(&self) -> &'static str {
            "test meshes"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.pipeline.set_lights(&cx.gpu.queue, &self.rig, cx.view);
            for draw in &mut self.draws {
                let instances: Vec<Instance> =
                    draw.placed.iter().map(|p| Instance::at(cx.view, p.position, p.model, p.translation, p.color, p.highlight)).collect();
                draw.instances.write(&cx.gpu.device, &cx.gpu.queue, &instances);
            }
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {
            for draw in &self.draws {
                self.pipeline.draw(pass, &draw.style, &draw.shape, &draw.instances);
            }
        }
    }

    /// A cube of side 1 about the origin, its faces' normals outward, counter-clockwise seen from
    /// outside (front faces in the culled presets).
    pub(crate) fn unit_cube() -> (Vec<Vertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for axis in 0..3 {
            for sign in [-1.0f32, 1.0] {
                let mut n = [0.0f32; 3];
                n[axis] = sign;
                // two axes spanning the face, with u × v = n
                let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
                let base = vertices.len() as u32;
                for (s, t) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
                    let mut p = [0.0f32; 3];
                    p[axis] = 0.5 * sign;
                    p[a] = s;
                    p[b] = t * sign;
                    vertices.push(Vertex { position: p, normal: n, color: Vertex::WHITE });
                }
                indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
        (vertices, indices)
    }

    pub(crate) fn render(gpu: &Gpu, view: ViewState, size: [f64; 2], build: impl FnOnce(&Init<'_>) -> Box<dyn Layer>) -> Image {
        let scene = SceneInput::new(view, size, 1.0, Basemap::Dark);
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let layer = build(&renderer.init(gpu));
        renderer.add_layer(layer);
        render_with(gpu, &mut renderer, &scene, 1.0).expect("rendered")
    }

    pub(crate) fn save_shot(image: &Image, name: &str) {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }

    /// Where a common offset lands on the canvas, CSS pixels from the top left.
    fn pixel_of(view: &ViewFrame, offset: DVec3) -> [f64; 2] {
        let clip = model::project_common(&view.globals, offset.as_vec3().into());
        model::clip_to_pixels(&view.globals, clip).map(f64::from)
    }

    /// The acceptance shot: a unit cube instance standing at a known place shows its top and two
    /// sides in three different brightnesses, each the colour the CPU twin of the lighting gives
    /// for that face (within 2 levels); with the material at depth 0 every face is exactly the
    /// instance colour; a hover highlight mixes in by its alpha. On the flat map and on the globe,
    /// with back faces culled, so the cube's winding and its east/north/up frame are right too.
    #[test]
    fn a_lit_cube_shows_three_faces() {
        let Some(gpu) = gpu() else { return };
        let size = [240.0, 240.0];
        let views = [
            ("flat", ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 16.0, pitch: 50.0, bearing: 30.0, position: None }),
            ("globe", ViewState { longitude: 18.07, latitude: 59.33, zoom: 10.0, pitch: 50.0, bearing: 30.0, position: None }),
        ];
        let color = [200, 170, 120, 255];
        for (name, view) in views {
            let vf = ViewFrame::new(&view, size, 1.0, 0.0);
            // about 70 px across
            let side = 70.0 * vf.viewport.meters_per_pixel();
            let position = [view.longitude, view.latitude, side / 2.0];
            let placed = |highlight| Placed {
                position,
                model: orientation_matrix([0.0, 0.0, 0.0], [side; 3]),
                translation: DVec3::ZERO,
                color,
                highlight,
            };
            let (vertices, indices) = unit_cube();
            let shot = |material: Material, highlight: [u8; 4]| {
                render(&gpu, view, size, |init| {
                    let mut m = Meshes::new(init, DrawState::DEFAULT, LightRig::new());
                    m.add(init, &vertices, Some(&indices), StyleUniform::new(&material, 1.0, [255; 4]), vec![placed(highlight)]);
                    Box::new(m)
                })
            };
            let lit_material = Material::aircraft(1.0);
            let lit = shot(lit_material, Instance::NO_HIGHLIGHT);
            save_shot(&lit, &format!("e7-cube-{name}.png"));
            let flat = shot(Material::aircraft(0.0), Instance::NO_HIGHLIGHT);
            let hovered = shot(Material::aircraft(0.0), [55, 142, 240, 150]);
            // the three faces turned toward the camera (from the south-south-west, above): top,
            // south and west, at their centres
            let rig = LightRig::new();
            let key = DVec3::from_array(rig.key_direction);
            let frame = local_frame(&vf, [position[0], position[1]]);
            let centre = vf.common_offset([position[0], position[1]], position[2]).as_dvec3();
            let camera = glam::Vec3::from(vf.globals.camera_common).as_dvec3();
            let surface = [color[0], color[1], color[2]].map(|c| f64::from(c) / 255.0);
            let mixed: [f64; 3] = std::array::from_fn(|i| f64::from(color[i]) + (f64::from([55u8, 142, 240][i]) - f64::from(color[i])) * 150.0 / 255.0);
            let mut seen = Vec::new();
            for (face, normal_enu) in [("top", DVec3::Z), ("south", -DVec3::Y), ("west", -DVec3::X)] {
                let at = centre + frame * (normal_enu * side / 2.0);
                let [x, y] = pixel_of(&vf, at);
                let [x, y] = [x.floor() as u32, y.floor() as u32];
                let normal = (frame * normal_enu).normalize();
                let expected = rig.shade(&lit_material, surface, normal, camera - at, key).map(|c| (c * 255.0).min(255.0));
                let got = lit.pixel(x, y);
                eprintln!("{name} {face} at ({x}, {y}): {got:?}, expected {expected:.1?}");
                assert!(
                    got.iter().zip(expected).all(|(g, e)| (f64::from(*g) - e).abs() <= 2.0) && got[3] == 255,
                    "{name} {face}: {got:?} against {expected:?}"
                );
                seen.push(got);
                // at depth 0: exactly the instance colour
                assert_eq!(flat.pixel(x, y), color, "{name} {face} at depth 0");
                let got = hovered.pixel(x, y);
                assert!(got.iter().zip(mixed).all(|(g, e)| (f64::from(*g) - e).abs() <= 1.0), "{name} {face} hovered: {got:?} against {mixed:?}");
            }
            // three different brightnesses
            let brightness: Vec<u32> = seen.iter().map(|p| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])).collect();
            assert!(brightness[0] != brightness[1] && brightness[1] != brightness[2] && brightness[0] != brightness[2], "{name}: {seen:?}");
            for i in 0..3 {
                for j in 0..i {
                    assert!(brightness[i].abs_diff(brightness[j]) >= 6, "{name}: faces {j} and {i} look alike: {seen:?}");
                }
            }
            // the ink around it is untouched
            assert_eq!(lit.pixel(3, 3), [30, 30, 30, 255]);
        }
    }

    /// Normals go through the instance's matrix, scale included, and are renormalised (deck's
    /// `project_normal(instanceModelMatrix * normals)`; doc 02 §14.3.6): a flat square whose normals
    /// lean north at 45°, drawn with a z scale of 4, is lit as if they leaned 76° up, not 14° (the
    /// inverse transpose).
    #[test]
    fn normals_follow_the_instance_scale() {
        let Some(gpu) = gpu() else { return };
        let size = [200.0, 200.0];
        let view = ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 16.0, pitch: 50.0, bearing: 30.0, position: None };
        let vf = ViewFrame::new(&view, size, 1.0, 0.0);
        let side = 60.0 * vf.viewport.meters_per_pixel();
        let leaning = [0.0, std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2];
        let square = [[-0.5f32, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]]
            .map(|[x, y]| Vertex { position: [x, y, 0.0], normal: leaning, color: Vertex::WHITE });
        let scale = [side, side, 4.0 * side];
        let material = Material::DECK_DEFAULT;
        let image = render(&gpu, view, size, |init| {
            let mut m = Meshes::new(init, DrawState::NO_CULL, LightRig::new());
            let placed = Placed {
                position: [view.longitude, view.latitude, 0.0],
                model: orientation_matrix([0.0; 3], scale),
                translation: DVec3::ZERO,
                color: [180, 180, 180, 255],
                highlight: Instance::NO_HIGHLIGHT,
            };
            m.add(init, &square, Some(&[0, 1, 2, 0, 2, 3]), StyleUniform::new(&material, 1.0, [255; 4]), vec![placed]);
            Box::new(m)
        });
        let at = vf.common_offset([view.longitude, view.latitude], 0.0).as_dvec3();
        let [x, y] = pixel_of(&vf, at).map(|c| c.floor() as u32);
        let rig = LightRig::new();
        let camera = glam::Vec3::from(vf.globals.camera_common).as_dvec3();
        let n = DVec3::from_array(leaning.map(f64::from));
        let colour = |normal: DVec3| rig.shade(&material, [180.0 / 255.0; 3], normal.normalize(), camera - at, DVec3::from_array(rig.key_direction)).map(|c| c * 255.0);
        let deck = colour(DVec3::new(0.0, n.y, 4.0 * n.z));
        let inverse_transpose = colour(DVec3::new(0.0, n.y, n.z / 4.0));
        let got = image.pixel(x, y);
        eprintln!("leaning normals: {got:?}; by the scale {deck:.1?}, by its inverse transpose {inverse_transpose:.1?}");
        assert!(got.iter().zip(deck).all(|(g, e)| (f64::from(*g) - e).abs() <= 2.0), "{got:?} against {deck:?}");
        assert!((deck[0] - inverse_transpose[0]).abs() > 10.0);
    }

    /// A test layer of extruded boxes in local Mercator, lit per vertex.
    struct Blocks {
        pipeline: MeshPipeline,
        shape: Shape,
        style: Style,
        anchor: DVec2,
        scale: f64,
        slot: Option<FrameSlot>,
    }

    impl Layer for Blocks {
        fn name(&self) -> &'static str {
            "test blocks"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.pipeline.set_lights(&cx.gpu.queue, &LightRig::new(), cx.view);
            self.slot = Some(cx.frames.push(&cx.view.local(self.anchor, self.scale, 0.0)));
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            if let Some(slot) = self.slot {
                self.pipeline.draw_local(pass, cx, slot, &self.style, &self.shape);
            }
        }
    }

    /// The local entry (extruded polygons): a box in local Mercator, its normals east/north/up, lit
    /// per vertex. Its top and two walls take three brightnesses, the colours of the CPU twin in
    /// the render frame (within 2 levels; per-vertex lighting interpolated over a face differs a
    /// little from the value at its centre); at depth 0 the fill colour exactly.
    #[test]
    fn a_lit_block_in_local_mercator() {
        let Some(gpu) = gpu() else { return };
        let size = [240.0, 240.0];
        let view = ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 16.0, pitch: 50.0, bearing: 30.0, position: None };
        let vf = ViewFrame::new(&view, size, 1.0, 0.0);
        let centre = DVec2::from_array(lng_lat_to_world([view.longitude, view.latitude]));
        // a box 60 px wide, its middle at the centre, 60 px tall
        let scale = crate::frame::power_of_two_scale(60.0 / 2f64.powf(view.zoom));
        let width = 60.0 / 2f64.powf(view.zoom) / scale;
        let anchor = centre + DVec2::new(-scale * width / 2.0, scale * width / 2.0);
        let height = 60.0 * vf.viewport.meters_per_pixel();
        // the unit cube moved into [0, w] × [0, w] × [0, h] in (u, v south, h): its y axis flips
        let (cube, indices) = unit_cube();
        let vertices: Vec<Vertex> = cube
            .iter()
            .map(|v| Vertex {
                position: [(v.position[0] + 0.5) * width as f32, (0.5 - v.position[1]) * width as f32, (v.position[2] + 0.5) * height as f32],
                ..*v
            })
            .collect();
        // (the same points on the Earth as the cube's, written in (u, v south): its winding outside
        // is unchanged)
        let fill = [58, 68, 84, 255];
        let shot = |material: Material| {
            render(&gpu, view, size, |init| {
                let pipeline = MeshPipeline::local(init, "test blocks", DrawState::DEFAULT);
                let device = &init.gpu.device;
                let style = pipeline.style(device, &StyleUniform::new(&material, 1.0, fill));
                let shape = Shape::new(device, "block", &vertices, Some(&indices));
                Box::new(Blocks { pipeline, shape, style, anchor, scale, slot: None })
            })
        };
        let material = Material::DECK_DEFAULT;
        let lit = shot(material);
        save_shot(&lit, "e7-block.png");
        let flat = shot(Material { ambient: 1.0, diffuse: 0.0, specular_color: [0.0; 3], ..material });
        let rig = LightRig::new();
        let camera = glam::Vec3::from(vf.globals.camera_render).as_dvec3();
        let mut seen = Vec::new();
        let middle = |dx: f64, dy: f64, h: f64| {
            let lnglat = rustradar_scene::camera::world_to_lng_lat([centre.x + dx * scale * width, centre.y + dy * scale * width]);
            vf.common_offset(lnglat, h).as_dvec3()
        };
        for (face, normal, at) in [
            ("top", DVec3::Z, middle(0.0, 0.0, height)),
            ("south", -DVec3::Y, middle(0.0, -0.5, height / 2.0)),
            ("west", -DVec3::X, middle(-0.5, 0.0, height / 2.0)),
        ] {
            let [x, y] = pixel_of(&vf, at);
            let rgb = [fill[0], fill[1], fill[2]].map(|c| f64::from(c) / 255.0);
            let expected = rig.shade(&material, rgb, normal, camera - at, DVec3::from_array(rig.key_direction)).map(|c| c * 255.0);
            let got = lit.pixel(x.floor() as u32, y.floor() as u32);
            eprintln!("block {face} at ({x:.1}, {y:.1}): {got:?}, expected {expected:.1?}");
            assert!(got.iter().zip(expected).all(|(g, e)| (f64::from(*g) - e).abs() <= 2.0), "{face}: {got:?} against {expected:?}");
            assert_eq!(flat.pixel(x.floor() as u32, y.floor() as u32), fill, "{face} unlit");
            seen.push(u32::from(got[0]) + u32::from(got[1]) + u32::from(got[2]));
        }
        assert!(seen[0] != seen[1] && seen[1] != seen[2] && seen[0] != seen[2], "{seen:?}");
    }
}
