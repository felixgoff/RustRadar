//! The earth sphere and the land rectangle (doc 01 §5.4–5.5; rows 1 and 3i of doc 03 §3).
//!
//! - [`Earth`] (`earth`, G:451-464): deck's SimpleMeshLayer of a luma.gl `SphereGeometry` of radius
//!   6.3e6 m (about 71 km below deck's 6 370 972 m surface, so every surface layer passes the depth
//!   test against it), 36 × 72 segments, centred on the Earth's centre in CARTESIAN coordinates,
//!   in the basemap's [`ground`] colour, with the `DEFAULT` state (back faces culled, depth
//!   written: the occluder that hides the globe's far side). Drawn on the globe only: in the flat
//!   projection a sphere means nothing, as doc 01 §5.4 says. It is drawn with the instanced
//!   [`MeshPipeline`] (deck's SimpleMeshLayer shaders), one instance whose matrix is the globe's
//!   units per metre, whose offset is the Earth's centre: its vertices are luma's, in metres and
//!   in common space as they are, so the mesh reaches the poles, where local Mercator cannot go.
//!
//!   **Lit.** The app gives it `material: false`, which deck 9.4 reads as luma's default Phong
//!   material ([`Material::DECK_DEFAULT`]; see `pipelines/mesh.rs`), lit by the app's
//!   LightingEffect, so today's dark map shows its polar caps past ±85.06° in (40,39,38) to
//!   (44,43,41), not (50,50,50) (`docs/port/reference/dark-v1-1440x900.png`). Doc 01 §4 and §5.4
//!   read it as unlit; deck and today's app win. Its normals are the sphere's own: deck's
//!   `project_normal` turns them by `project_get_orientation_matrix(geometry.position)` on the
//!   globe, but SimpleMeshLayer calls it before `geometry.position` is set, while it still holds
//!   its initial `(0, 0, 1, 0)`, whose orientation matrix is the identity. The rig is the scene's
//!   one rig (`SceneInput::lights`, [`PrepareCx::set_lights`]), as deck's LightingEffect lights
//!   every layer; its key light is aimed only over buildings on the flat map (`buildings && flat`,
//!   G:2724), so wherever the earth is drawn it has its usual direction, [`LightRig::new`]'s
//!   (but for the frames between a refresh on the flat map and the next one after the camera
//!   crossed zoom 12). [`lit_color`] is the CPU twin of one of its pixels; the dark map's cap at
//!   V1 is today's to the level (test `v1_dark_matches_todays_app_at_the_centre_and_the_pole`).
//!
//!   Now that the sphere differs from the land, one thing E5's unlit sphere hid shows: on
//!   SwiftShader a few MSAA samples of its edge-on silhouette facets win the depth test against
//!   the land drawn over them, so a ring of single samples up to 2 px inside the sphere's own limb
//!   (1.1 % inside the globe's) takes its colour (28 pixels at V1, a few levels off). The same
//!   samples did it with the unlit sphere; it is the rasteriser's depth on sliver triangles.
//! - [`Land`] (`land`, G:567-585, dark only): deck's SolidPolygonLayer of the rectangle ±180°,
//!   ±85.06° in [`LAND`], `SURFACE` (culled, depth-tested, no depth written), cut as deck cuts it on
//!   the globe (math.gl's grid of 5°, then earcut per part: [`land_mesh`]) and drawn with the
//!   [`Fill`] pipeline in one local Mercator frame for the whole world. On the flat map the world's
//!   copies west and east of the nearest are drawn too, so the antimeridian shows no seam.
//!
//! Both draw only when the scene asks for them ([`Input::shown`]): a `SceneInput::new` has nothing
//! on it, and the app's scene builder sets `earth: Input::SHOWN`.

use glam::{DVec2, DVec3};
use rustradar_core::geo::Basemap;
use rustradar_scene::camera::{GLOBE_EARTH_RADIUS_M, GLOBE_RADIUS, WORLD_SIZE, lng_lat_to_world};
use rustradar_scene::geom::densify::GLOBE_RESOLUTION_DEG;
use rustradar_scene::geom::earcut::earcut;
use rustradar_scene::geom::grid_cut::{GridOptions, cut_polygon_by_grid};
use rustradar_scene::ground::POLES;

use super::{DrawCx, Init, Layer, Pass, PrepareCx};
use crate::frame::{Frame, FrameSlot, ViewFrame};
use crate::pipelines::fill::{self, Fill, Mesh, counter_clockwise_on_the_map};
use crate::pipelines::mesh::{self, Instance, Instances, LightRig, Material, MeshPipeline, Shape, Style, StyleUniform};
use crate::state::DrawState;

/// The sphere's radius (`EARTH_MESH`, G:451).
pub const EARTH_RADIUS_M: f64 = 6.3e6;
/// Its latitude segments (`nlat`).
pub const EARTH_LATITUDE_SEGMENTS: u32 = 36;
/// Its longitude segments (`nlong`).
pub const EARTH_LONGITUDE_SEGMENTS: u32 = 72;

/// The sphere's colour on the dark map: the land seen past ±85.06° and through any gap (`GROUND`,
/// G:453).
pub const GROUND_DARK: [u8; 3] = [50, 50, 50];
/// On the satellite map, where it shows only at the poles and while tiles load.
pub const GROUND_SATELLITE: [u8; 3] = [6, 16, 34];

/// The land rectangle's fill (`LAND`, G:499).
pub const LAND: [u8; 4] = [50, 50, 50, 255];
/// The land rectangle (`LAND_POLYGON`, G:569-576), longitude and latitude.
pub const LAND_POLYGON: [[f64; 2]; 4] = [[-180.0, -85.06], [180.0, -85.06], [180.0, 85.06], [-180.0, 85.06]];

/// The sphere's colour by basemap (`GROUND`, G:453): realistic is `POLES` (`realistic-ground.ts:96`).
pub fn ground(basemap: Basemap) -> [u8; 3] {
    match basemap {
        Basemap::Dark => GROUND_DARK,
        Basemap::Satellite => GROUND_SATELLITE,
        Basemap::Realistic => POLES,
    }
}

/// What the scene builder hands the earth and the land (`SceneInput::earth`).
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// Whether they are drawn at all. Off in a scene made with `SceneInput::new` (which has nothing
    /// on it); the app's scene builder always turns it on.
    pub shown: bool,
}

impl Input {
    /// The earth and, on the dark map, the land.
    pub const SHOWN: Input = Input { shown: true };
}

/// luma.gl's `SphereGeometry({ nlat, nlong })` of radius 1 (`tesselateSphere`): `(nlat + 1) ×
/// (nlong + 1)` unit vectors, row by row from the pole at +y, and its triangles, counter-clockwise
/// seen from outside. deck places it in common space as it is (no orientation), so the mesh's poles
/// lie on the equator at longitudes 180° and 0°.
pub fn sphere_mesh(nlat: u32, nlong: u32) -> (Vec<[f32; 3]>, Vec<u32>) {
    let mut positions = Vec::with_capacity(((nlat + 1) * (nlong + 1)) as usize);
    for y in 0..=nlat {
        for x in 0..=nlong {
            let theta = std::f64::consts::TAU * (f64::from(x) / f64::from(nlong));
            let phi = std::f64::consts::PI * (f64::from(y) / f64::from(nlat));
            let (sin_theta, cos_theta) = theta.sin_cos();
            let (sin_phi, cos_phi) = phi.sin_cos();
            positions.push([(cos_theta * sin_phi) as f32, cos_phi as f32, (sin_theta * sin_phi) as f32]);
        }
    }
    let around = nlong + 1;
    let mut indices = Vec::with_capacity((nlat * nlong * 6) as usize);
    for x in 0..nlong {
        for y in 0..nlat {
            indices.extend([
                y * around + x,
                y * around + x + 1,
                (y + 1) * around + x,
                (y + 1) * around + x,
                y * around + x + 1,
                (y + 1) * around + x + 1,
            ]);
        }
    }
    (positions, indices)
}

/// The land rectangle as deck's SolidPolygonLayer tessellates it on the globe: cut by math.gl's
/// `cutPolygonByGrid` on a 5° grid (with edge types), each part earcut, and every triangle turned
/// counter-clockwise on the map. Vertices are in the local Mercator frame of the whole world: anchor
/// at world `(0, 512)` (the north-west corner), scale 512, so `u = x / 512`, `v = (512 − y) / 512`.
pub fn land_mesh() -> (Vec<fill::Vertex>, Vec<u32>) {
    let positions: Vec<f64> = LAND_POLYGON.concat();
    let parts = cut_polygon_by_grid(&positions, &[], &GridOptions { edge_types: true, ..GridOptions::grid(GLOBE_RESOLUTION_DEG) });
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for part in parts {
        let points: Vec<[f64; 2]> = part.positions.chunks_exact(2).map(|p| [p[0], p[1]]).collect();
        let holes: Vec<usize> = part.hole_indices.iter().map(|i| i / 2).collect();
        let base = vertices.len() as u32;
        for &[lon, lat] in &points {
            let [x, y] = lng_lat_to_world([lon, lat]);
            vertices.push(fill::Vertex { position: [(x / WORLD_SIZE) as f32, ((WORLD_SIZE - y) / WORLD_SIZE) as f32, 0.0], color: LAND });
        }
        for t in earcut(&part.positions, &holes, 2).chunks_exact(3) {
            let triangle = counter_clockwise_on_the_map(&points, [t[0] as u32, t[1] as u32, t[2] as u32]);
            indices.extend(triangle.map(|i| base + i));
        }
    }
    (vertices, indices)
}

/// The sphere as luma's `SphereGeometry({ radius: 6.3e6, nlat: 36, nlong: 72 })` hands it to deck:
/// positions in metres (`radius · u`, rounded to f32 as its `Float32Array` does), normals the unit
/// directions `u`, no colours (white), and its indices.
pub fn earth_vertices() -> (Vec<mesh::Vertex>, Vec<u32>) {
    let (directions, indices) = sphere_mesh(EARTH_LATITUDE_SEGMENTS, EARTH_LONGITUDE_SEGMENTS);
    let vertices = directions
        .iter()
        .map(|&u| mesh::Vertex { position: u.map(|c| (EARTH_RADIUS_M * f64::from(c)) as f32), normal: u, color: mesh::Vertex::WHITE })
        .collect();
    (vertices, indices)
}

/// The globe's common units per metre (deck's `commonUnitsPerMeter` in GLOBE mode: 256 units for
/// 6 370 972 m, the same on every axis).
fn globe_units_per_metre() -> f64 {
    GLOBE_RADIUS / GLOBE_EARTH_RADIUS_M
}

/// The sphere's one instance in `view` (globe only): the mesh's metres to common units about the
/// Earth's centre, whose common offset is the origin minus the view's common centre (in f64), in
/// the basemap's ground colour, opaque (`getColor: GROUND[kind]`, alpha 255).
pub fn earth_instance(view: &ViewFrame, basemap: Basemap) -> Instance {
    let scale = globe_units_per_metre() as f32;
    let [r, g, b] = ground(basemap);
    Instance {
        columns: [[scale, 0.0, 0.0], [0.0, scale, 0.0], [0.0, 0.0, scale]],
        offset: (DVec3::ZERO - view.common_centre).as_vec3().into(),
        color: [r, g, b, 255],
        highlight: Instance::NO_HIGHLIGHT,
    }
}

/// The CPU twin of the earth's colour (0–1) at pixel `xy` (CSS pixels, the pixel's centre) of the
/// globe `view` on `basemap`, lit by `rig`: the pixel's ray against the 6.3e6 m sphere, the
/// sphere's normal there, the camera's position, and luma's Phong with
/// [`Material::DECK_DEFAULT`] ([`LightRig::shade`]). `None` on the flat map or where the ray
/// misses the sphere. (The GPU interpolates normals and positions over the 5° facets and rounds
/// to bytes: on SwiftShader the two agree within 2.5 levels, 99 % of pixels within 1.7.)
pub fn lit_color(view: &ViewFrame, rig: &LightRig, basemap: Basemap, xy: [f64; 2]) -> Option<[f64; 3]> {
    let globe = view.viewport.globe()?;
    if globe.ray_distance_to_globe_center_ratio(xy) >= EARTH_RADIUS_M / GLOBE_EARTH_RADIUS_M {
        return None;
    }
    let depth = EARTH_RADIUS_M - GLOBE_EARTH_RADIUS_M;
    let [lon, lat] = globe.unproject(xy, depth);
    let at = globe.project_position([lon, lat, depth]);
    let surface = ground(basemap).map(|c| f64::from(c) / 255.0);
    let to_camera = view.viewport.camera_position - at;
    Some(rig.shade(&Material::DECK_DEFAULT, surface, at.normalize(), to_camera, DVec3::from_array(rig.key_direction)))
}

/// Row 1 of doc 03 §3: `earth`.
pub struct Earth {
    pipeline: MeshPipeline,
    style: Style,
    shape: Shape,
    instances: Instances,
    /// Whether this frame draws it.
    visible: bool,
}

impl Earth {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let pipeline = MeshPipeline::instanced(init, "earth", DrawState::DEFAULT);
        // `material: false`: luma's default Phong material, lit (module notes); deck's opacity 1
        let style = pipeline.style(device, &StyleUniform::new(&Material::DECK_DEFAULT, 1.0, [255; 4]));
        let (vertices, indices) = earth_vertices();
        let shape = Shape::new(device, "earth", &vertices, Some(&indices));
        let instances = Instances::new(device, "earth");
        Earth { pipeline, style, shape, instances, visible: false }
    }
}

impl Layer for Earth {
    fn name(&self) -> &'static str {
        "earth"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.visible = cx.scene.earth.shown && cx.view.is_globe();
        if !self.visible {
            return;
        }
        // the scene's rig, unaimed wherever the globe is drawn (module notes)
        cx.set_lights(&self.pipeline);
        self.instances.write(&cx.gpu.device, &cx.gpu.queue, &[earth_instance(cx.view, cx.scene.basemap)]);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if !self.visible || cx.pass != Pass::Map {
            return;
        }
        self.pipeline.draw(pass, &self.style, &self.shape, &self.instances);
    }
}

/// Row 3i of doc 03 §3: `land` (dark basemap).
pub struct Land {
    fill: Fill,
    style: fill::Style,
    mesh: Mesh<fill::Vertex>,
    /// This frame's frames: the world, and on the flat map its copies west and east.
    slots: Vec<FrameSlot>,
}

impl Land {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let fill = Fill::new(init, "land", DrawState::SURFACE);
        let style = fill.style(device, fill::OPAQUE);
        let (vertices, indices) = land_mesh();
        let mesh = Mesh::new(device, "land", &vertices, &indices);
        Land { fill, style, mesh, slots: Vec::new() }
    }
}

/// `frame` moved `dx` world units east, its offset split again into hi + lo (in f64, so a copy's
/// edge meets its neighbour's exactly where they show).
fn shifted(frame: &Frame, dx: f64) -> Frame {
    let x = f64::from(frame.offset_hi[0]) + f64::from(frame.offset_lo[0]) + dx;
    let hi = x as f32;
    Frame { offset_hi: [hi, frame.offset_hi[1]], offset_lo: [(x - f64::from(hi)) as f32, frame.offset_lo[1]], ..*frame }
}

impl Layer for Land {
    fn name(&self) -> &'static str {
        "land"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.slots.clear();
        if !cx.scene.earth.shown || cx.scene.basemap != Basemap::Dark {
            return;
        }
        let world = cx.view.local(DVec2::new(0.0, WORLD_SIZE), WORLD_SIZE, 0.0);
        if cx.view.is_globe() {
            self.slots.push(cx.frames.push(&world));
        } else {
            for copy in [-1.0, 0.0, 1.0] {
                self.slots.push(cx.frames.push(&shifted(&world, copy * WORLD_SIZE)));
            }
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if self.slots.is_empty() || !cx.pass.draws(self.drape()) {
            return;
        }
        self.fill.bind(pass, &self.style);
        for slot in &self.slots {
            cx.bind_frame(pass, *slot);
            self.mesh.draw(pass);
        }
    }

    fn drape(&self) -> super::Drape {
        // stage 9: drawn into the terrain's covers while the 3D terrain is drawn
        super::Drape::Cover
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rustradar_scene::camera::{GlobeViewport, ViewState};

    use super::*;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::renderer::{MapRenderer, SceneInput};

    const INK: [u8; 3] = [30, 30, 30];
    /// The shots' canvas (plan section 8).
    const SIZE: [f64; 2] = [1100.0, 720.0];
    /// The reference pictures' canvas (`docs/port/reference/`).
    const REFERENCE_SIZE: [f64; 2] = [1440.0, 900.0];
    /// V1 (plan section 8): the initial view.
    const V1: ViewState = ViewState::INITIAL;
    /// The earth sphere's radius over deck's surface sphere.
    const EARTH_RATIO: f64 = EARTH_RADIUS_M / GLOBE_EARTH_RADIUS_M;
    /// How far (levels, each channel) the GPU's lit earth may be from [`lit_color`]: the GPU
    /// lights the 5° facets with interpolated normals in f32 and rounds to bytes. Measured on
    /// SwiftShader at V1 on satellite and realistic, every pixel more than 1.5 px inside the limb:
    /// at most 2.23 levels off, 99 % within 1.66, the mean 0.4.
    const LIT_TOLERANCE: f64 = 2.5;
    /// The land rectangle's edge, degrees of latitude.
    const LAND_EDGE: f64 = LAND_POLYGON[2][1];

    /// What a pixel (its centre, CSS pixels) should be: its colour (0–255) and the tolerance, or
    /// `None` where a test does not say.
    type Expect<'a> = &'a dyn Fn([f64; 2]) -> Option<([f64; 3], f64)>;

    #[test]
    fn the_ground_colour_follows_the_basemap() {
        assert_eq!(ground(Basemap::Dark), [50, 50, 50]);
        assert_eq!(ground(Basemap::Satellite), [6, 16, 34]);
        assert_eq!(ground(Basemap::Realistic), [226, 234, 240]);
        assert!(!Input::default().shown && Input::SHOWN.shown);
    }

    /// luma.gl's `tesselateSphere` (`@luma.gl/engine` sphere-geometry.js), and every triangle faces
    /// out.
    #[test]
    fn the_sphere_is_lumas() {
        let (positions, indices) = sphere_mesh(EARTH_LATITUDE_SEGMENTS, EARTH_LONGITUDE_SEGMENTS);
        assert_eq!(positions.len(), 37 * 73);
        assert_eq!(indices.len(), 36 * 72 * 6);
        assert_eq!(&indices[..12], &[0, 1, 73, 73, 1, 74, 73, 74, 146, 146, 74, 147]);
        assert_eq!(positions[0], [0.0, 1.0, 0.0]);
        // x = 18 (θ = 90°), y = 18 (φ = 90°): +z
        let p = positions[18 * 73 + 18];
        assert!((p[0]).abs() < 1e-7 && p[1].abs() < 1e-7 && (p[2] - 1.0).abs() < 1e-7, "{p:?}");
        assert!(positions.iter().all(|p| ((p[0] * p[0] + p[1] * p[1] + p[2] * p[2]) - 1.0).abs() < 1e-6));
        let mut faces = 0;
        for t in indices.chunks_exact(3) {
            let [a, b, c] = [0, 1, 2].map(|k| DVec3::from_array(positions[t[k] as usize].map(f64::from)));
            let normal = (b - a).cross(c - a);
            if normal.length() < 1e-9 {
                continue; // the poles' degenerate triangles
            }
            assert!(normal.dot(a + b + c) > 0.0, "{t:?} faces in");
            faces += 1;
        }
        assert_eq!(faces, 36 * 72 * 2 - 2 * 72);
    }

    /// The mesh as luma hands it to deck (positions in metres rounded to f32, the unit directions
    /// as normals, white), drawn as one instance: the globe's units per metre about the Earth's
    /// centre, in the ground colour, opaque, not highlighted.
    #[test]
    fn the_earth_is_lumas_mesh_on_one_instance() {
        let (vertices, indices) = earth_vertices();
        let (directions, sphere_indices) = sphere_mesh(EARTH_LATITUDE_SEGMENTS, EARTH_LONGITUDE_SEGMENTS);
        assert_eq!(indices, sphere_indices);
        assert_eq!(vertices.len(), directions.len());
        for (v, u) in vertices.iter().zip(&directions) {
            assert_eq!(v.normal, *u);
            assert_eq!(v.color, [1.0; 3]);
            assert_eq!(v.position, u.map(|c| (6.3e6 * f64::from(c)) as f32));
        }
        assert_eq!(vertices[0].position, [0.0, 6.3e6, 0.0]);
        let view = ViewFrame::new(&V1, SIZE, 1.0, 0.0);
        let instance = earth_instance(&view, Basemap::Satellite);
        assert_eq!((instance.color, instance.highlight), ([6, 16, 34, 255], [0; 4]));
        let s = (GLOBE_RADIUS / GLOBE_EARTH_RADIUS_M) as f32;
        assert_eq!(instance.columns, [[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, s]]);
        assert_eq!(f64::from(s), view.viewport.distance_scales.units_per_meter[2] as f32 as f64);
        let centre = DVec3::from_array(instance.offset.map(f64::from)) + view.common_centre;
        assert!(centre.length() < 1e-4, "the Earth's centre lands at {centre:?}");
        assert_eq!(earth_instance(&view, Basemap::Dark).color, [50, 50, 50, 255]);
    }

    /// deck cuts the rectangle into 72 × 36 parts (two rows of 0.06°), each into two triangles, all
    /// counter-clockwise on the map, none wider or taller than 5°, covering it exactly once.
    #[test]
    fn the_land_is_cut_as_deck_cuts_it() {
        let (vertices, indices) = land_mesh();
        assert_eq!(indices.len(), 72 * 36 * 2 * 3);
        assert!(vertices.iter().all(|v| v.color == LAND && v.position[2] == 0.0));
        let back = |v: &fill::Vertex| {
            let x = f64::from(v.position[0]) * WORLD_SIZE;
            let y = WORLD_SIZE - f64::from(v.position[1]) * WORLD_SIZE;
            rustradar_scene::camera::world_to_lng_lat([x, y])
        };
        let mut area = 0.0;
        for t in indices.chunks_exact(3) {
            let [a, b, c] = [0, 1, 2].map(|k| back(&vertices[t[k] as usize]));
            for (p, q) in [(a, b), (b, c), (c, a)] {
                assert!((p[0] - q[0]).abs() <= 5.0 + 1e-4 && (p[1] - q[1]).abs() <= 5.0 + 1e-4, "{p:?} {q:?}");
            }
            let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            assert!(cross > 0.0, "{t:?} is clockwise on the map");
            area += cross / 2.0;
        }
        assert!((area - 360.0 * 170.12).abs() < 0.1, "{area}");
        let (min_u, max_u) = vertices.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v.position[0]), hi.max(v.position[0])));
        assert_eq!((min_u, max_u), (0.0, 1.0));
    }

    #[test]
    fn a_shifted_frame_moves_by_whole_worlds() {
        let frame = Frame { offset_hi: [-511.9, 3.0], offset_lo: [1e-6, 2e-7], scale: 512.0, z_offset: 0.0, pad: [0.0; 2] };
        let east = shifted(&frame, 512.0);
        let x = f64::from(east.offset_hi[0]) + f64::from(east.offset_lo[0]);
        let expected = f64::from(-511.9f32) + f64::from(1e-6f32) + 512.0;
        assert!((x - expected).abs() < 1e-12);
        assert_eq!((east.offset_hi[1], east.offset_lo[1], east.scale), (3.0, 2e-7, 512.0));
    }

    fn reference(name: &str) -> image::RgbImage {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference").join(name);
        image::open(&path).expect("reference picture").to_rgb8()
    }

    /// The CPU twin against today's app: the earth sphere at the north pole of
    /// `dark-v1-1440x900.png` and `realistic-v1-1440x900.png` (V1 at 2026-06-21 10:00 UTC, so in
    /// sunlight and with no night layer over it; both pictures exact, not dithered) within a level,
    /// in all three channels, and not the unlit ground colour. Off the earth sphere (between its
    /// limb and deck's surface, and in space) and on the flat map it says nothing.
    #[test]
    fn the_twin_is_todays_lit_earth_at_the_pole() {
        let view = ViewFrame::new(&V1, REFERENCE_SIZE, 1.0, 0.0);
        let rig = LightRig::new();
        for (basemap, name) in [(Basemap::Dark, "dark-v1-1440x900.png"), (Basemap::Realistic, "realistic-v1-1440x900.png")] {
            let picture = reference(name);
            for [x, y] in [[700u32, 185u32], [720, 185], [740, 185], [700, 175], [740, 180]] {
                let lit = lit_color(&view, &rig, basemap, [f64::from(x) + 0.5, f64::from(y) + 0.5]).expect("on the earth").map(|c| c * 255.0);
                let got = picture.get_pixel(x, y).0;
                eprintln!("{name} ({x}, {y}): {got:?} in the picture, {lit:.1?} by the twin");
                assert!(got.iter().zip(lit).all(|(g, e)| (f64::from(*g) - e).abs() <= 1.0), "{name} ({x}, {y}): {got:?} against {lit:?}");
                assert_ne!(got, ground(basemap));
            }
        }
        let globe = view.viewport.globe().expect("globe");
        let between = (limb_radius(globe, 0.0, EARTH_RATIO) + limb_radius(globe, 0.0, 1.0)) / 2.0;
        assert_eq!(lit_color(&view, &rig, Basemap::Dark, [720.0 + between, 450.0]), None);
        assert_eq!(lit_color(&view, &rig, Basemap::Dark, [5.0, 5.0]), None);
        let flat = ViewFrame::new(&ViewState { zoom: 13.0, ..V1 }, SIZE, 1.0, 0.0);
        assert_eq!(lit_color(&flat, &rig, Basemap::Dark, [550.0, 360.0]), None);
    }

    /// Renders `view` on a `size` canvas with the earth shown (or not).
    fn render(gpu: &crate::gpu::Gpu, view: ViewState, size: [f64; 2], basemap: Basemap, shown: bool) -> (Image, MapRenderer) {
        let mut scene = SceneInput::new(view, size, 1.0, basemap);
        scene.earth = Input { shown };
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let image = render_with(gpu, &mut renderer, &scene, 1.0).expect("rendered");
        (image, renderer)
    }

    fn save(image: &Image, name: &str) {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }

    /// Where E1's camera puts the limb of the sphere of `ratio` × deck's radius, as a distance from
    /// the screen's centre along `angle` (radians, x right, y down): bisection on how far each
    /// pixel's ray passes from the globe's centre.
    fn limb_radius(globe: &GlobeViewport, angle: f64, ratio: f64) -> f64 {
        let centre = [globe.width / 2.0, globe.height / 2.0];
        let (mut inside, mut outside) = (0.0, globe.width.max(globe.height));
        for _ in 0..60 {
            let r = (inside + outside) / 2.0;
            let xy = [centre[0] + r * angle.cos(), centre[1] + r * angle.sin()];
            if globe.ray_distance_to_globe_center_ratio(xy) < ratio {
                inside = r;
            } else {
                outside = r;
            }
        }
        (inside + outside) / 2.0
    }

    /// The lit earth sphere at a pixel, from the twin.
    fn lit(view: &ViewFrame, basemap: Basemap, xy: [f64; 2]) -> Option<([f64; 3], f64)> {
        lit_color(view, &LightRig::new(), basemap, xy).map(|c| (c.map(|v| v * 255.0), LIT_TOLERANCE))
    }

    /// Whether the pixel's ray meets deck's surface past the land's ±85.06° (where the dark map
    /// shows the earth sphere): `None` within 1.5 px of that edge.
    fn past_the_land(globe: &GlobeViewport, [x, y]: [f64; 2]) -> Option<bool> {
        let past = |p: [f64; 2]| globe.unproject(p, 0.0)[1].abs() > LAND_EDGE;
        let latitude = globe.unproject([x, y], 0.0)[1].abs();
        if (latitude - LAND_EDGE).abs() > 3.0 {
            return Some(latitude > LAND_EDGE);
        }
        let here = past([x, y]);
        let around = [[-1.5, 0.0], [1.5, 0.0], [0.0, -1.5], [0.0, 1.5]];
        around.iter().all(|[dx, dy]| past([x + dx, y + dy]) == here).then_some(here)
    }

    /// Whether the pixel is within 3 px of the earth sphere's own limb (1.1 % inside deck's),
    /// along the line from the screen's centre.
    ///
    /// There, on SwiftShader, a few MSAA samples of the sphere's edge-on silhouette facets win the
    /// depth test against the land drawn over them: a quarter or half of a pixel, along a ring up
    /// to 2 px inside that limb (the facets' 5° reach further in on a larger globe): 28 pixels at
    /// V1, 42 at latitude 0, 3 at latitude 60, none elsewhere. E5's unlit earth did the same at
    /// the same pixels (seen by drawing it red), unseen because it had the land's colour; with the
    /// lit earth they are a few levels off (16 at most, where the key light lights the limb). A
    /// rasteriser's sliver-triangle depth, not the lighting: the land checks leave that ring out.
    fn near_the_earths_limb(globe: &GlobeViewport, [x, y]: [f64; 2]) -> bool {
        let on = |p: [f64; 2]| globe.ray_distance_to_globe_center_ratio(p) < EARTH_RATIO;
        let out = DVec2::new(x - globe.width / 2.0, y - globe.height / 2.0).normalize_or_zero() * 3.0;
        on([x - out.x, y - out.y]) != on([x + out.x, y + out.y])
    }

    /// What the dark map's globe shows at a pixel inside deck's limb: the land rectangle, exactly
    /// `(50,50,50)`, where the ray meets the surface within ±85.06°; past it the lit earth sphere
    /// ([`lit_color`]); `None` near the land's edge and near the earth sphere's own limb
    /// ([`near_the_earths_limb`]).
    fn dark_globe(view: &ViewFrame, xy: [f64; 2]) -> Option<([f64; 3], f64)> {
        let globe = view.viewport.globe()?;
        if near_the_earths_limb(globe, xy) {
            return None;
        }
        match past_the_land(globe, xy)? {
            false => Some((GROUND_DARK.map(f64::from), 0.0)),
            true => lit(view, Basemap::Dark, xy),
        }
    }

    /// The dark map's limb is the land's, all round (in these views no pole is near it).
    fn land_at_the_limb(_: [f64; 2]) -> Option<([f64; 3], f64)> {
        Some((GROUND_DARK.map(f64::from), 0.0))
    }

    /// The disc's edge in `sectors` equal sectors round the screen's centre: in each, the coverage
    /// of the pixels (by their centres) in a ring of ±6 px about the sector's limb (0 for
    /// `outside`, 1 for what `full` says a covered pixel there is, read on the channel that
    /// differs most; a pixel whose centre is past the limb takes the colour 1 px inside it) is the
    /// area of the disc in that ring, which gives the edge's mean radius there. `None` for sectors
    /// whose ring leaves the image or whose colour `full` does not give.
    fn edge_radii(image: &Image, limbs: &[f64], full: Expect<'_>, outside: [u8; 3]) -> Vec<Option<f64>> {
        let sectors = limbs.len();
        let centre = [f64::from(image.width) / 2.0, f64::from(image.height) / 2.0];
        let step = std::f64::consts::TAU / sectors as f64;
        let mut covered = vec![0.0; sectors];
        let mut unusable = vec![false; sectors];
        let reach = limbs.iter().fold(0.0f64, |a, &b| a.max(b)) + 6.0;
        let (x0, x1) = ((centre[0] - reach).floor() as i64, (centre[0] + reach).ceil() as i64);
        let (y0, y1) = ((centre[1] - reach).floor() as i64, (centre[1] + reach).ceil() as i64);
        for y in y0..y1 {
            for x in x0..x1 {
                let (dx, dy) = (x as f64 + 0.5 - centre[0], y as f64 + 0.5 - centre[1]);
                let r = dx.hypot(dy);
                let sector = ((dy.atan2(dx).rem_euclid(std::f64::consts::TAU) / step) as usize).min(sectors - 1);
                let limb = limbs[sector];
                if r < limb - 6.0 || r >= limb + 6.0 {
                    continue;
                }
                if x < 0 || y < 0 || x >= i64::from(image.width) || y >= i64::from(image.height) {
                    unusable[sector] = true;
                    continue;
                }
                let k = r.min(limb - 1.0) / r;
                let Some((full, _)) = full([centre[0] + dx * k, centre[1] + dy * k]) else {
                    unusable[sector] = true;
                    continue;
                };
                let channel = (0..3).max_by(|&a, &b| (full[a] - f64::from(outside[a])).abs().total_cmp(&(full[b] - f64::from(outside[b])).abs())).expect("three");
                let (lo, hi) = (f64::from(outside[channel]), full[channel]);
                let v = f64::from(image.pixel(x as u32, y as u32)[channel]);
                covered[sector] += ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
            }
        }
        (0..sectors).map(|s| (!unusable[s]).then(|| ((limbs[s] - 6.0).powi(2) + 2.0 * covered[s] / step).sqrt())).collect()
    }

    /// Checks the disc on `outside` against the limb of the sphere of `ratio` × deck's radius:
    /// every sector's edge within 1.5 px (the acceptance of E5; `rim` is a covered pixel's colour
    /// at the limb), every pixel further than 1.5 px inside the limb within `expect`'s tolerance of
    /// its colour (where it gives one), every one further than 1.5 px outside exactly `outside`.
    /// Returns the limb's radius, the mean measured edge and how many pixels inside were checked.
    fn assert_disc(image: &Image, renderer: &MapRenderer, ratio: f64, rim: Expect<'_>, expect: Expect<'_>, outside: [u8; 3], what: &str) -> (f64, f64, usize) {
        let globe = renderer.view().viewport.globe().expect("the globe");
        let sectors = 72;
        let step = std::f64::consts::TAU / sectors as f64;
        let limbs: Vec<f64> = (0..sectors).map(|s| limb_radius(globe, (s as f64 + 0.5) * step, ratio)).collect();
        let mut measured = Vec::new();
        for (s, edge) in edge_radii(image, &limbs, rim, outside).into_iter().enumerate() {
            let Some(edge) = edge else { continue };
            assert!((edge - limbs[s]).abs() <= 1.5, "{what}: the edge at {:.1}° is at {edge:.2} px, the limb at {:.2} px", (s as f64 + 0.5) * 5.0, limbs[s]);
            measured.push(edge);
        }
        assert!(measured.len() >= sectors / 3, "{what}: only {} sectors on the image", measured.len());
        let centre = [f64::from(image.width) / 2.0, f64::from(image.height) / 2.0];
        let (mut wrong, mut checked, mut worst) = (0, 0, 0.0f64);
        for y in 0..image.height {
            for x in 0..image.width {
                let xy = [f64::from(x) + 0.5, f64::from(y) + 0.5];
                let (dx, dy) = (xy[0] - centre[0], xy[1] - centre[1]);
                let r = dx.hypot(dy);
                let limb = limbs[((dy.atan2(dx).rem_euclid(std::f64::consts::TAU) / step) as usize).min(sectors - 1)];
                let p = image.pixel(x, y);
                if p[3] != 255 {
                    wrong += 1;
                } else if r > limb + 1.5 {
                    wrong += usize::from([p[0], p[1], p[2]] != outside);
                } else if r < limb - 1.5 {
                    let Some((colour, tolerance)) = expect(xy) else { continue };
                    let off = (0..3).map(|c| (f64::from(p[c]) - colour[c]).abs()).fold(0.0, f64::max);
                    worst = worst.max(off);
                    checked += 1;
                    if off > tolerance {
                        if wrong < 5 {
                            eprintln!("{what}: ({x}, {y}) is {p:?}, expected {colour:.1?} ± {tolerance}");
                        }
                        wrong += 1;
                    }
                }
            }
        }
        eprintln!("{what}: {checked} pixels inside checked, worst {worst:.2} levels off");
        assert_eq!(wrong, 0, "{what}: {wrong} pixels away from the edge are not what they should be");
        (limbs[0], measured.iter().sum::<f64>() / measured.len() as f64, checked)
    }

    /// E5's acceptance, shot V1 (plan section 8), with the earth lit as deck lights it: on the
    /// dark map the globe is a disc of land `(50,50,50)` on `(30,30,30)` whose edge is E1's
    /// projection of the limb within 1.5 px all round, and past ±85.06° (the cap above the
    /// centre) the earth sphere, lit as [`lit_color`] says, not `(50,50,50)`.
    #[test]
    fn v1_is_a_disc_of_land_on_ink_at_the_limb() {
        let Some(gpu) = gpu() else { return };
        for (size, name) in [(SIZE, "e5-v1-dark.png"), (REFERENCE_SIZE, "e5-v1-dark-1440x900.png")] {
            let (image, renderer) = render(&gpu, V1, size, Basemap::Dark, true);
            save(&image, name);
            let view = renderer.view();
            let (limb, edge, _) = assert_disc(&image, &renderer, 1.0, &land_at_the_limb, &|xy| dark_globe(view, xy), INK, name);
            eprintln!("{name}: limb {limb:.2} px, measured edge {edge:.2} px");
            assert_eq!(image.pixel(size[0] as u32 / 2, size[1] as u32 / 2), [50, 50, 50, 255]);
            // the cap past 85.06° N is in view, above the centre, and lit
            let [x, y, _] = view.viewport.project([10.0, 88.0, 0.0]);
            let p = image.pixel(x as u32, y as u32);
            let (expected, _) = lit(view, Basemap::Dark, [x.floor() + 0.5, y.floor() + 0.5]).expect("on the earth");
            assert!((0..3).all(|c| (f64::from(p[c]) - expected[c]).abs() <= LIT_TOLERANCE), "{name}: the cap is {p:?}, lit {expected:.1?}");
            assert!(p[0] < 47 && p[2] < p[0], "{name}: the cap is {p:?}");
            assert_eq!(image.pixel(2, 2), [30, 30, 30, 255]);
        }
    }

    /// Item T2's acceptance: V1 on the dark map at the reference picture's size against today's
    /// app (`docs/port/reference/dark-v1-1440x900.png`, scale 1): the disc's centre, and every
    /// pixel of the north polar cap that is more than 1.5 px from the land's edge (where only the
    /// earth sphere shows, in both pictures), within 3 levels of the picture.
    #[test]
    fn v1_dark_matches_todays_app_at_the_centre_and_the_pole() {
        let Some(gpu) = gpu() else { return };
        let picture = reference("dark-v1-1440x900.png");
        let (image, renderer) = render(&gpu, V1, REFERENCE_SIZE, Basemap::Dark, true);
        save(&image, "t2-v1-dark-1440x900.png");
        let globe = renderer.view().viewport.globe().expect("globe");
        let off = |x: u32, y: u32| {
            let (p, q) = (image.pixel(x, y), picture.get_pixel(x, y).0);
            (0..3).map(|c| (i32::from(p[c]) - i32::from(q[c])).abs()).max().unwrap_or(0)
        };
        assert!(off(720, 450) <= 3, "the centre: {:?} against {:?}", image.pixel(720, 450), picture.get_pixel(720, 450));
        let (mut cap, mut worst) = (0, 0);
        for y in 100..450 {
            for x in 400..1040 {
                let xy = [f64::from(x) + 0.5, f64::from(y) + 0.5];
                if globe.ray_distance_to_globe_center_ratio(xy) >= EARTH_RATIO || past_the_land(globe, xy) != Some(true) {
                    continue;
                }
                cap += 1;
                worst = worst.max(off(x, y));
                assert!(off(x, y) <= 3, "the cap at ({x}, {y}): {:?} against {:?} in today's app", image.pixel(x, y), picture.get_pixel(x, y));
            }
        }
        eprintln!("V1 dark against today's app: {cap} pixels of the cap, worst {worst} levels off");
        assert!(cap > 500, "only {cap} pixels of the cap");
    }

    /// The earth alone, on satellite (no imagery yet) and realistic (no sky or ground yet): a disc
    /// lit as [`lit_color`] says in the basemap's ground colour, at the limb of the 6.3e6 m
    /// sphere, 1.1 % inside deck's surface; between the two limbs, space.
    #[test]
    fn the_earth_sphere_sits_below_the_surface_lit_in_the_basemaps_colour() {
        let Some(gpu) = gpu() else { return };
        for (basemap, name) in [(Basemap::Satellite, "e5-v1-satellite.png"), (Basemap::Realistic, "e5-v1-realistic.png")] {
            let (image, renderer) = render(&gpu, V1, SIZE, basemap, true);
            save(&image, name);
            let view = renderer.view();
            let lit_here = |xy| lit(view, basemap, xy);
            let (limb, _, checked) = assert_disc(&image, &renderer, EARTH_RATIO, &lit_here, &lit_here, INK, name);
            assert!(checked > 100_000, "{name}: {checked}");
            let surface = limb_radius(view.viewport.globe().expect("globe"), 0.0, 1.0);
            // no land between the two limbs: the land is the dark map's only
            assert!(surface - limb > 3.0, "{surface} {limb}");
            let x = (550.0 + (limb + surface) / 2.0) as u32;
            assert_eq!(image.pixel(x, 360), [30, 30, 30, 255], "{name}");
            // lit, not flat: the centre (the camera light full on, its highlight) and a point near
            // the left limb differ, and neither is the ground colour as it is
            let flat = ground(basemap);
            let (centre, side) = (image.pixel(550, 360), image.pixel((550.0 - 0.9 * limb) as u32, 360));
            assert!((0..3).any(|c| centre[c].abs_diff(side[c]) > 10), "{name}: {centre:?} at the centre, {side:?} near the limb");
            assert!(centre[..3] != flat && side[..3] != flat, "{name}: {centre:?}, {side:?}");
        }
    }

    /// E5's acceptance: at the same zoom, the globe centred on latitude 60° is twice the size of the
    /// one centred on the equator (deck's `scale = 2^zoom / (π · cos lat)`). Both discs are at E1's
    /// limb within 1.5 px. What doubles is the globe's radius at the view centre's distance: the
    /// camera stands 1.5 viewport heights above the surface in both, so the disc's own width grows
    /// less (1.71 ×) by perspective; the test takes each measured disc back through that
    /// perspective (`R = 1.5 t (t + √(t² + 1))` viewport heights for a disc of `t` focal lengths)
    /// and finds the ratio 2. (At latitude 60 the lit cap past 85.06° is well inside the disc.)
    #[test]
    fn latitude_60_shows_the_globe_twice_as_large_as_the_equator() {
        let Some(gpu) = gpu() else { return };
        let mut globe_px = Vec::new();
        let mut disc_px = Vec::new();
        for (latitude, name) in [(0.0, "e5-lat0.png"), (60.0, "e5-lat60.png")] {
            let view = ViewState { latitude, ..V1 };
            let (image, renderer) = render(&gpu, view, SIZE, Basemap::Dark, true);
            save(&image, name);
            let frame = renderer.view();
            let (_, edge, _) = assert_disc(&image, &renderer, 1.0, &land_at_the_limb, &|xy| dark_globe(frame, xy), INK, name);
            let height = SIZE[1];
            let t = edge / (1.5 * height);
            let radius = 1.5 * t * (t + (t * t + 1.0).sqrt()) * height;
            let scale = renderer.view().viewport.scale;
            eprintln!("latitude {latitude}: disc {edge:.2} px, globe radius {radius:.2} px (256 · scale = {:.2})", 256.0 * scale);
            assert!((radius / (GLOBE_RADIUS * scale) - 1.0).abs() < 0.01);
            globe_px.push(radius);
            disc_px.push(edge);
        }
        let ratio = globe_px[1] / globe_px[0];
        assert!((ratio - 2.0).abs() < 0.02, "the globe grew {ratio} times");
        eprintln!("the disc grew {:.3} times, the globe {ratio:.3}", disc_px[1] / disc_px[0]);
        assert!(disc_px[1] / disc_px[0] > 1.6);
    }

    /// On the flat map there is no earth sphere, and the land covers everything, the antimeridian
    /// included (both sides of it, at zoom 16 and pitched at zoom 20); the globe at zoom 12 is
    /// land to the horizon too, and on satellite the lit earth.
    #[test]
    fn the_flat_map_is_all_land_and_has_no_earth() {
        let Some(gpu) = gpu() else { return };
        let land = [50, 50, 50, 255];
        let views = [
            // V4
            (ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 13.5, pitch: 45.0, bearing: 30.0, position: None }, land),
            (ViewState { longitude: 179.9995, latitude: 0.0, zoom: 16.0, ..V1 }, land),
            (ViewState { longitude: -179.9995, latitude: -40.0, zoom: 16.0, ..V1 }, land),
            (ViewState { longitude: 180.0, latitude: 65.0, zoom: 20.0, pitch: 60.0, bearing: 90.0, position: None }, land),
            (ViewState { longitude: 10.0, latitude: 35.0, zoom: 12.0, pitch: 60.0, bearing: 0.0, position: None }, land),
        ];
        for (i, (view, colour)) in views.into_iter().enumerate() {
            let (image, _) = render(&gpu, view, [400.0, 300.0], Basemap::Dark, true);
            if i == 0 {
                save(&image, "e5-v4-dark.png");
            }
            let wrong = image.rgba.chunks_exact(4).filter(|p| *p != colour).count();
            assert_eq!(wrong, 0, "{view:?}: {wrong} pixels are not land");
        }
        // satellite: the lit earth down to zoom 12, nothing (the ink) on the flat map
        let view = ViewState { zoom: 12.0, pitch: 60.0, ..V1 };
        let (image, renderer) = render(&gpu, view, [400.0, 300.0], Basemap::Satellite, true);
        let frame = renderer.view();
        let (mut wrong, mut worst) = (0, 0.0f64);
        for y in 0..300 {
            for x in 0..400 {
                let p = image.pixel(x, y);
                let (expected, tolerance) = lit(frame, Basemap::Satellite, [f64::from(x) + 0.5, f64::from(y) + 0.5]).expect("the earth fills the view");
                let off = (0..3).map(|c| (f64::from(p[c]) - expected[c]).abs()).fold(0.0, f64::max);
                worst = worst.max(off);
                wrong += usize::from(off > tolerance || p[3] != 255);
            }
        }
        assert_eq!(wrong, 0, "zoom 12: {wrong} pixels are not the lit earth (worst {worst:.2} levels off)");
        let view = ViewState { zoom: 12.5, pitch: 60.0, ..V1 };
        let (image, _) = render(&gpu, view, [400.0, 300.0], Basemap::Satellite, true);
        let wrong = image.rgba.chunks_exact(4).filter(|p| *p != [30, 30, 30, 255]).count();
        assert_eq!(wrong, 0, "zoom 12.5: {wrong} pixels are not ink");
    }

    /// The earth writes depth: a fill on the globe's far side, drawn after it without culling,
    /// stays hidden behind it (and shows through where the earth is not drawn).
    #[test]
    fn the_earth_hides_the_far_side() {
        let Some(gpu) = gpu() else { return };
        // on the far side of V1, 60 px or so across, still inside the far plane
        let far = DVec2::from_array(lng_lat_to_world([-150.0, -20.0]));
        let scale = crate::frame::power_of_two_scale(60.0 / 2f64.powf(V1.zoom));
        let anchor = far + DVec2::new(-scale / 2.0, scale / 2.0);
        for shown in [false, true] {
            let mut scene = SceneInput::new(V1, [400.0, 300.0], 1.0, Basemap::Satellite);
            scene.earth = Input { shown };
            let mut renderer = MapRenderer::new(&gpu, FORMAT);
            let init = renderer.init(&gpu);
            let mut squares = crate::pipelines::fill::tests::Squares::new(&init, DrawState::SURFACE_NO_CULL);
            squares.add(&init, anchor, scale, [200, 100, 50, 255], fill::OPAQUE, false);
            renderer.add_layer(Box::new(squares));
            let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
            let [x, y, _] = renderer.view().viewport.project([-150.0, -20.0, 0.0]);
            let p = image.pixel(x as u32, y as u32);
            if shown {
                let (expected, tolerance) = lit(renderer.view(), Basemap::Satellite, [x.floor() + 0.5, y.floor() + 0.5]).expect("on the earth");
                assert!((0..3).all(|c| (f64::from(p[c]) - expected[c]).abs() <= tolerance), "the earth: {p:?}, lit {expected:.1?}");
            } else {
                assert_eq!(p, [200, 100, 50, 255]);
            }
        }
    }

    /// A scene that does not ask for them draws neither.
    #[test]
    fn nothing_is_drawn_unless_the_scene_shows_it() {
        let Some(gpu) = gpu() else { return };
        let (image, _) = render(&gpu, V1, [320.0, 200.0], Basemap::Dark, false);
        assert!(image.rgba.chunks_exact(4).all(|p| p == [30, 30, 30, 255]));
    }
}
