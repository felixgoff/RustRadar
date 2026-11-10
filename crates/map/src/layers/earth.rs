//! The earth sphere and the land rectangle (doc 01 §5.4–5.5; rows 1 and 3i of doc 03 §3).
//!
//! - [`Earth`] (`earth`, G:451-464): deck's SimpleMeshLayer of a luma.gl `SphereGeometry` of radius
//!   6.3e6 m (about 71 km below deck's 6 370 972 m surface, so every surface layer passes the depth
//!   test against it), 36 × 72 segments, centred on the Earth's centre in CARTESIAN coordinates,
//!   unlit, in the basemap's [`ground`] colour, with the `DEFAULT` state (back faces culled, depth
//!   written: the occluder that hides the globe's far side). Drawn on the globe only: in the flat
//!   projection a sphere means nothing, as doc 01 §5.4 says. Its own shader, `shaders/earth.wgsl`,
//!   as it reaches the poles, where local Mercator cannot go.
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
use crate::frame::{Frame, FrameSlot};
use crate::pipelines::fill::{self, Fill, Mesh, counter_clockwise_on_the_map, uniform_group, uniform_layout};
use crate::pipelines::{self, Desc};
use crate::shaders;
use crate::state::DrawState;

/// `shaders/earth.wgsl`.
pub const SHADER: &str = include_str!("../shaders/earth.wgsl");

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

/// `earth.wgsl`'s `Earth`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EarthUniform {
    /// The sphere's centre as a common offset.
    pub centre: [f32; 3],
    /// Its radius, common units.
    pub radius: f32,
    /// RGBA, 0–1.
    pub color: [f32; 4],
}

/// Row 1 of doc 03 §3: `earth`.
pub struct Earth {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    group: wgpu::BindGroup,
    mesh: Mesh<[f32; 3]>,
    /// Whether this frame draws it.
    visible: bool,
}

impl Earth {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let layout = uniform_layout(device, "earth", size_of::<EarthUniform>());
        let module = shaders::module(device, "earth", SHADER);
        let pipeline = pipelines::build(
            init,
            &Desc {
                label: "earth",
                module: &module,
                vertex_entry: "vs",
                fragment_entry: "fs",
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<[f32; 3]>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
                topology: wgpu::PrimitiveTopology::TriangleList,
                state: DrawState::DEFAULT,
                groups: &[&layout],
            },
        );
        let (uniform, group) = uniform_group(device, "earth", &layout, &EarthUniform { centre: [0.0; 3], radius: 0.0, color: [0.0; 4] });
        let (positions, indices) = sphere_mesh(EARTH_LATITUDE_SEGMENTS, EARTH_LONGITUDE_SEGMENTS);
        let mesh = Mesh::new(device, "earth", &positions, &indices);
        Earth { pipeline, uniform, group, mesh, visible: false }
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
        // deck: the mesh's metres times the globe's units per metre, about the common origin
        let radius = EARTH_RADIUS_M * GLOBE_RADIUS / GLOBE_EARTH_RADIUS_M;
        let [r, g, b] = ground(cx.scene.basemap).map(|c| f32::from(c) / 255.0);
        let uniform = EarthUniform { centre: (DVec3::ZERO - cx.view.common_centre).as_vec3().into(), radius: radius as f32, color: [r, g, b, 1.0] };
        cx.gpu.queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if !self.visible || cx.pass != Pass::Map {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(2, &self.group, &[]);
        self.mesh.draw(pass);
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
        if self.slots.is_empty() || cx.pass != Pass::Map {
            return;
        }
        self.fill.bind(pass, &self.style);
        for slot in &self.slots {
            cx.bind_frame(pass, *slot);
            self.mesh.draw(pass);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rustradar_scene::camera::{GlobeViewport, ViewState};

    use super::*;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::pipelines::fill::tests::wgsl_members;
    use crate::renderer::{MapRenderer, SceneInput};

    const INK: [u8; 3] = [30, 30, 30];
    /// The shots' canvas (plan section 8).
    const SIZE: [f64; 2] = [1100.0, 720.0];
    /// V1 (plan section 8): the initial view.
    const V1: ViewState = ViewState::INITIAL;
    /// The earth sphere's radius over deck's surface sphere.
    const EARTH_RATIO: f64 = EARTH_RADIUS_M / GLOBE_EARTH_RADIUS_M;

    #[test]
    fn layouts_match_the_wgsl() {
        let wgsl = vec![("centre".to_string(), 0), ("radius".to_string(), 12), ("color".to_string(), 16)];
        assert_eq!(wgsl_members(SHADER, "Earth"), (32, wgsl));
        assert_eq!(size_of::<EarthUniform>(), 32);
        assert_eq!(std::mem::offset_of!(EarthUniform, radius), 12);
        assert_eq!(std::mem::offset_of!(EarthUniform, color), 16);
    }

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

    /// Renders `view` on a `SIZE` canvas with the earth shown (or not).
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

    /// The disc's edge in `sectors` equal sectors round the screen's centre: in each, the coverage
    /// of the pixels (by their centres) in a ring of ±6 px about `near` (0 for `outside`, 1 for
    /// `inside`, read on the channel that differs most) is the area of the disc in that ring, which
    /// gives the edge's mean radius there. `None` for sectors whose ring leaves the image.
    fn edge_radii(image: &Image, near: f64, sectors: usize, inside: [u8; 3], outside: [u8; 3]) -> Vec<Option<f64>> {
        let channel = (0..3).max_by_key(|&c| (i32::from(inside[c]) - i32::from(outside[c])).abs()).expect("three");
        let (lo, hi) = (f64::from(outside[channel]), f64::from(inside[channel]));
        let centre = [f64::from(image.width) / 2.0, f64::from(image.height) / 2.0];
        let (r0, r1) = (near - 6.0, near + 6.0);
        let step = std::f64::consts::TAU / sectors as f64;
        let mut covered = vec![0.0; sectors];
        let mut clipped = vec![false; sectors];
        let (x0, x1) = ((centre[0] - r1).floor() as i64, (centre[0] + r1).ceil() as i64);
        let (y0, y1) = ((centre[1] - r1).floor() as i64, (centre[1] + r1).ceil() as i64);
        for y in y0..y1 {
            for x in x0..x1 {
                let (dx, dy) = (x as f64 + 0.5 - centre[0], y as f64 + 0.5 - centre[1]);
                let r = dx.hypot(dy);
                if r < r0 || r >= r1 {
                    continue;
                }
                let sector = ((dy.atan2(dx).rem_euclid(std::f64::consts::TAU) / step) as usize).min(sectors - 1);
                if x < 0 || y < 0 || x >= i64::from(image.width) || y >= i64::from(image.height) {
                    clipped[sector] = true;
                    continue;
                }
                let v = f64::from(image.pixel(x as u32, y as u32)[channel]);
                covered[sector] += ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
            }
        }
        (0..sectors).map(|s| (!clipped[s]).then(|| (r0 * r0 + 2.0 * covered[s] / step).sqrt())).collect()
    }

    /// Checks the disc of `inside` on `outside` against the limb of the sphere of `ratio` × deck's
    /// radius: every sector's edge within 1.5 px (the acceptance of E5), every pixel further than
    /// 1.5 px inside the limb exactly `inside`, every one further than 1.5 px outside exactly
    /// `outside`. Returns the limb's radius and the mean measured edge.
    fn assert_disc(image: &Image, renderer: &MapRenderer, ratio: f64, inside: [u8; 3], outside: [u8; 3], what: &str) -> (f64, f64) {
        let globe = renderer.view().viewport.globe().expect("the globe");
        let limb = limb_radius(globe, 0.0, ratio);
        let sectors = 72;
        let mut measured = Vec::new();
        for (s, edge) in edge_radii(image, limb, sectors, inside, outside).into_iter().enumerate() {
            let Some(edge) = edge else { continue };
            let angle = (s as f64 + 0.5) * std::f64::consts::TAU / sectors as f64;
            let expected = limb_radius(globe, angle, ratio);
            assert!((edge - expected).abs() <= 1.5, "{what}: the edge at {:.1}° is at {edge:.2} px, the limb at {expected:.2} px", angle.to_degrees());
            measured.push(edge);
        }
        assert!(measured.len() >= sectors / 3, "{what}: only {} sectors on the image", measured.len());
        let centre = [f64::from(image.width) / 2.0, f64::from(image.height) / 2.0];
        let mut wrong = 0;
        for y in 0..image.height {
            for x in 0..image.width {
                let r = (f64::from(x) + 0.5 - centre[0]).hypot(f64::from(y) + 0.5 - centre[1]);
                let p = image.pixel(x, y);
                let rgb = [p[0], p[1], p[2]];
                if (r < limb - 1.5 && rgb != inside) || (r > limb + 1.5 && rgb != outside) || p[3] != 255 {
                    wrong += 1;
                }
            }
        }
        assert_eq!(wrong, 0, "{what}: {wrong} pixels away from the edge are neither {inside:?} inside nor {outside:?} outside");
        (limb, measured.iter().sum::<f64>() / measured.len() as f64)
    }

    /// E5's acceptance, shot V1 (plan section 8): on the dark map the globe is a disc of
    /// `(50,50,50)` on `(30,30,30)` (the land rectangle on deck's surface, the earth sphere past
    /// ±85.06°), whose edge is E1's projection of the limb within 1.5 px all round.
    #[test]
    fn v1_is_a_disc_of_land_on_ink_at_the_limb() {
        let Some(gpu) = gpu() else { return };
        let (image, renderer) = render(&gpu, V1, SIZE, Basemap::Dark, true);
        save(&image, "e5-v1-dark.png");
        let (limb, edge) = assert_disc(&image, &renderer, 1.0, GROUND_DARK, INK, "V1 dark");
        eprintln!("V1 dark: limb {limb:.2} px, measured edge {edge:.2} px");
        assert_eq!(image.pixel(550, 360), [50, 50, 50, 255]);
        // the cap past 85.06° N (the earth sphere's own colour) is in view, above the centre
        let [x, y, _] = renderer.view().viewport.project([10.0, 88.0, 0.0]);
        assert_eq!(image.pixel(x as u32, y as u32), [50, 50, 50, 255]);
        assert_eq!(image.pixel(2, 2), [30, 30, 30, 255]);
        // the reference shots' canvas (docs/port/reference/dark-v1-1440x900.png: limb at 297–299 px)
        let (image, renderer) = render(&gpu, V1, [1440.0, 900.0], Basemap::Dark, true);
        save(&image, "e5-v1-dark-1440x900.png");
        let (limb, edge) = assert_disc(&image, &renderer, 1.0, GROUND_DARK, INK, "V1 dark 1440 × 900");
        eprintln!("V1 dark 1440 × 900: limb {limb:.2} px, measured edge {edge:.2} px");
    }

    /// The earth alone, on satellite (no imagery yet) and realistic (no sky or ground yet): a disc in
    /// the basemap's ground colour at the limb of the 6.3e6 m sphere, 1.1 % inside deck's surface.
    #[test]
    fn the_earth_sphere_sits_below_the_surface_in_the_basemaps_colour() {
        let Some(gpu) = gpu() else { return };
        for (basemap, name) in [(Basemap::Satellite, "e5-v1-satellite.png"), (Basemap::Realistic, "e5-v1-realistic.png")] {
            let (image, renderer) = render(&gpu, V1, SIZE, basemap, true);
            save(&image, name);
            let (limb, _) = assert_disc(&image, &renderer, EARTH_RATIO, ground(basemap), INK, name);
            let surface = limb_radius(renderer.view().viewport.globe().expect("globe"), 0.0, 1.0);
            // no land between the two limbs: the land is the dark map's only
            assert!(surface - limb > 3.0, "{surface} {limb}");
            let x = (550.0 + (limb + surface) / 2.0) as u32;
            assert_eq!(image.pixel(x, 360), [30, 30, 30, 255], "{name}");
        }
    }

    /// E5's acceptance: at the same zoom, the globe centred on latitude 60° is twice the size of the
    /// one centred on the equator (deck's `scale = 2^zoom / (π · cos lat)`). Both discs are at E1's
    /// limb within 1.5 px. What doubles is the globe's radius at the view centre's distance: the
    /// camera stands 1.5 viewport heights above the surface in both, so the disc's own width grows
    /// less (1.71 ×) by perspective; the test takes each measured disc back through that
    /// perspective (`R = 1.5 t (t + √(t² + 1))` viewport heights for a disc of `t` focal lengths)
    /// and finds the ratio 2.
    #[test]
    fn latitude_60_shows_the_globe_twice_as_large_as_the_equator() {
        let Some(gpu) = gpu() else { return };
        let mut globe_px = Vec::new();
        let mut disc_px = Vec::new();
        for (latitude, name) in [(0.0, "e5-lat0.png"), (60.0, "e5-lat60.png")] {
            let view = ViewState { latitude, ..V1 };
            let (image, renderer) = render(&gpu, view, SIZE, Basemap::Dark, true);
            save(&image, name);
            let (_, edge) = assert_disc(&image, &renderer, 1.0, GROUND_DARK, INK, name);
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
    /// land to the horizon too, and so is the earth on satellite.
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
        // satellite: the earth down to zoom 12, nothing (the ink) on the flat map
        for (zoom, colour) in [(12.0, [6, 16, 34, 255]), (12.5, [30, 30, 30, 255])] {
            let view = ViewState { zoom, pitch: 60.0, ..V1 };
            let (image, _) = render(&gpu, view, [400.0, 300.0], Basemap::Satellite, true);
            let wrong = image.rgba.chunks_exact(4).filter(|p| *p != colour).count();
            assert_eq!(wrong, 0, "zoom {zoom}: {wrong} pixels are not {colour:?}");
        }
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
            let expected = if shown { [6, 16, 34, 255] } else { [200, 100, 50, 255] };
            assert_eq!(image.pixel(x as u32, y as u32), expected, "earth shown: {shown}");
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
