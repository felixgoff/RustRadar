//! Ships close in, as 3D models (item H3; doc 02 §4.6, `shipMeshLayers`, G:1761-1812): row 10 of
//! doc 03 §3 (doc 02 §2 row 7), drawn by the ships layer ([`super::ships`]) after the selection
//! halo and before the icons.
//!
//! The scene builder has already split the ships ([`ShipsScene::models`],
//! [`rustradar_scene::ships::ShipBuilder::build`]): from zoom 12.5, in quarter-zoom steps, every
//! ship with a heading whose hull would be at least 14 px long, one group per class in the order
//! each class first appears in the list, each model where its ship is drawn now. This part draws
//! them as deck's SimpleMeshLayers `ship-mesh-<class>` with
//! [`crate::pipelines::mesh::MeshPipeline::instanced`], one draw per group in that order:
//!
//! - **Meshes**: the procedural models of [`rustradar_scene::ship_models`] (`shipMeshes()`), one
//!   unit long with the bow on +y, +x to starboard, +z up from the waterline, three vertices a
//!   triangle with their normals and paint colours; each class's mesh is uploaded the first time a
//!   ship takes it.
//! - **Instances** ([`instance`]): at the ship's drawn `[lon, lat, 0]`, deck's orientation
//!   `[0, −heading, 0]` (`yawOffset − heading` with `yawOffset = 0` on the flat map) and scale
//!   `[beam / model beam, length, up]` (`getScale`), then moved by the hull's offset `[east, north,
//!   0]` metres (`getTranslation`, so that the AIS reference point lands on the reported position),
//!   in a true east, north, up frame at the ship (plan 1.4.2). The globe's half turn (`yawOffset =
//!   180`, `sign = −1`) is not reproduced: the frame is true there too, and models are drawn only
//!   from zoom 12.5, on the flat map.
//! - **Colour**: [`rustradar_scene::ships::SHIP_MESH_COLOR`] (white: the mesh's own paints), or
//!   [`rustradar_scene::ships::SHIP_MESH_OLD_COLOR`] (alpha 120) for an old position; the ship
//!   hovered in the sidebar's list, or under the pointer, blended toward
//!   [`MESH_HIGHLIGHT`] (deck's `highlightedObjectIndex`, `autoHighlight`).
//! - **Light**: [`MATERIAL`] (`SHIP_MESH_MATERIAL`: ambient 0.5, diffuse 0.48, shininess 24,
//!   specular 40) and the scene's light rig (sun, camera and ambient, as the aircraft). The
//!   pipeline multiplies the normals by the instance's scale and renormalises them, as deck does.
//! - **State**: [`STATE`] (`parameters: NO_CULL`): no culling, depth tested and written, unlike
//!   the icons' `SHIP_PARAMETERS`.
//!
//! Instances are rebuilt in each `prepare` from the scene and this frame's view and written to the
//! GPU only when they change.
//!
//! As in deck, an old ship at alpha 120 is drawn triangle by triangle in its mesh's order, each
//! blending over the triangles drawn before it that it passes the depth test against.
//!
//! Deviations: none intended. The models are pickable and highlighted under the pointer: the pick
//! records mark the one picked ([`ShipsScene::highlight`], item H4).

use glam::DVec3;
use rustradar_scene::ship_models::{SHIP_CLASSES, ShipClass, ship_meshes};
use rustradar_scene::ships::{MESH_HIGHLIGHT, ModelGroup, ShipModel};

use super::{DrawCx, Init, Pass, PrepareCx};
use crate::frame::ViewFrame;
use crate::pipelines::mesh::{Instance, Instances, Material, MeshPipeline, Shape, Style, StyleUniform, Vertex, orientation_matrix};
use crate::state::DrawState;

#[cfg(doc)]
use rustradar_scene::ships::ShipsScene;

/// The models' material (`SHIP_MESH_MATERIAL`, G:1610): the aircraft's lights on darker, coloured
/// paint, with less ambient than the white airframes need, so decks seen from straight above keep
/// their colours instead of washing out.
pub const MATERIAL: Material = Material { ambient: 0.5, diffuse: 0.48, shininess: 24.0, specular_color: [40.0; 3], unlit: false };

/// The models' draw state (`parameters: NO_CULL`): no culling, depth tested and written.
pub const STATE: DrawState = DrawState::NO_CULL;

/// deck's layer id of a class's models.
pub fn layer_id(cls: ShipClass) -> String {
    format!("ship-mesh-{}", cls.as_str())
}

/// One model's instance in `view`: the class's unit-long mesh turned to the heading
/// (`getOrientation: [0, −heading, 0]`), stretched to the ship (`getScale`), moved by the hull's
/// offset east and north (`getTranslation`), at the ship's drawn place on the water, in its colour,
/// highlighted [`MESH_HIGHLIGHT`] when hovered.
pub fn instance(view: &ViewFrame, m: &ShipModel) -> Instance {
    let model = orientation_matrix([0.0, -m.heading, 0.0], m.scale);
    let highlight = if m.highlighted { MESH_HIGHLIGHT } else { Instance::NO_HIGHLIGHT };
    Instance::at(view, m.position, model, DVec3::new(m.east, m.north, 0.0), m.color, highlight)
}

/// One draw: a class's instances on the GPU, kept between frames.
struct Draw {
    cls: ShipClass,
    instances: Instances,
    written: Vec<Instance>,
}

/// Row 10 close in: the ships drawn as models.
pub struct ShipModels {
    pipeline: MeshPipeline,
    style: Style,
    /// Each class's mesh once a ship has taken it, indexed by `cls as usize`.
    shapes: [Option<Shape>; SHIP_CLASSES.len()],
    /// This frame's draws (the first [`Self::active`]), reused across frames.
    draws: Vec<Draw>,
    active: usize,
}

impl ShipModels {
    pub fn new(init: &Init<'_>) -> Self {
        let pipeline = MeshPipeline::instanced(init, "ship models", STATE);
        let style = pipeline.style(&init.gpu.device, &StyleUniform::new(&MATERIAL, 1.0, [255; 4]));
        ShipModels { pipeline, style, shapes: Default::default(), draws: Vec::new(), active: 0 }
    }

    /// Once a frame: the draws of `groups` ([`ShipsScene::models`]) in this frame's view, each
    /// class's mesh uploaded the first time it is taken, and the scene's lights.
    pub fn prepare(&mut self, cx: &PrepareCx<'_>, groups: &[ModelGroup]) {
        self.active = 0;
        let (device, queue) = (&cx.gpu.device, &cx.gpu.queue);
        for group in groups.iter().filter(|g| !g.models.is_empty()) {
            let shape = &mut self.shapes[group.cls as usize];
            if shape.is_none() {
                let mesh = &ship_meshes()[group.cls as usize];
                let vertices = Vertex::from_arrays(&mesh.positions, &mesh.normals, Some(&mesh.colors));
                *shape = Some(Shape::new(device, &format!("ship model {}", group.cls.as_str()), &vertices, None));
            }
            let instances: Vec<Instance> = group.models.iter().map(|m| instance(cx.view, m)).collect();
            if self.draws.len() == self.active {
                self.draws.push(Draw { cls: group.cls, instances: Instances::new(device, "ship model instances"), written: Vec::new() });
            }
            let draw = &mut self.draws[self.active];
            draw.cls = group.cls;
            if instances != draw.written {
                draw.instances.write(device, queue, &instances);
                draw.written = instances;
            }
            self.active += 1;
        }
        if self.active > 0 {
            cx.set_lights(&self.pipeline);
        }
    }

    /// Every class's models, in the groups' order (map pass only).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if cx.pass != Pass::Map {
            return;
        }
        for draw in &self.draws[..self.active] {
            let Some(shape) = &self.shapes[draw.cls as usize] else { continue };
            self.pipeline.draw(pass, &self.style, shape, &draw.instances);
        }
    }

    /// This frame's draws in order: deck's layer id and how many models each draws.
    pub fn drawn(&self) -> Vec<(String, u32)> {
        self.draws[..self.active].iter().map(|d| (layer_id(d.cls), d.instances.count())).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use glam::DMat3;
    use rustradar_core::geo::{Basemap, meters_per_pixel};
    use rustradar_core::model::Ship;
    use rustradar_scene::camera::{ViewState, viewport_for};
    use rustradar_scene::ship_models::{ship_class_of, ship_placement};
    use rustradar_scene::ships::{SHIP_MESH_COLOR, SHIP_MESH_OLD_COLOR, ShipBuilder, ShipsScene};

    use super::*;
    use crate::gpu::Gpu;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::Layer;
    use crate::pipelines::mesh::local_frame;
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 10:00 UTC.
    const NOW: i64 = 1_782_036_000_000;
    /// The canvas of the V6 shots, CSS px.
    const SIZE: [f64; 2] = [1440.0, 900.0];
    /// V6 at zoom 15 (Gothenburg harbour), straight down: 1.28 m a pixel.
    const VIEW: ViewState = ViewState { longitude: 11.93, latitude: 57.70, zoom: 15.0, bearing: 0.0, pitch: 0.0, position: None };
    const M_PER_DEG: f64 = 111_320.0;

    /// `(lat, lon)` `m` metres from `at` going `heading`.
    fn ahead((lat, lon): (f64, f64), heading: f64, m: f64) -> (f64, f64) {
        let h = heading.to_radians();
        (lat + h.cos() * m / M_PER_DEG, lon + h.sin() * m / (M_PER_DEG * lat.to_radians().cos()))
    }

    /// A ship of `kind` heading `heading` standing at `xy` on the canvas of `view`, `dims` from its
    /// AIS reference point (bow, stern, port, starboard).
    fn ship(mmsi: u32, kind: u8, heading: f32, dims: [u16; 4], xy: [f64; 2], view: &ViewState) -> Ship {
        let [lon, lat] = viewport_for(view, SIZE).unproject(xy, 0.0);
        Ship { mmsi, lat, lon, name: format!("SHIP {mmsi}"), kind, size: 1, heading: Some(heading), dims: Some(dims), ..Ship::default() }
    }

    /// The ships as the scene builder hands them over at `NOW` in `view`, the one with MMSI
    /// `hovered` hovered in the list.
    fn ships_scene(view: &ViewState, ships: &[Ship], hovered: Option<u32>) -> ShipsScene {
        ShipBuilder::new().build(ships, true, view, SIZE, hovered, NOW)
    }

    /// `ships` drawn by the ships layer over the plain map of `view` (the scene builder's wiring:
    /// `scene.ships.scene`).
    fn shoot(gpu: &Gpu, view: ViewState, scale: f64, ships: ShipsScene) -> Image {
        let mut scene = SceneInput::new(view, SIZE, scale, Basemap::Dark);
        scene.local_now_ms = NOW as f64;
        scene.feed_now_ms = NOW as f64;
        scene.ships.scene = ships;
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        render_with(gpu, &mut renderer, &scene, scale).expect("rendered")
    }

    /// A test layer: [`ShipModels`] drawing `groups` and nothing else, keeping what it drew.
    struct Probe {
        layer: ShipModels,
        groups: Vec<ModelGroup>,
        drawn: Rc<RefCell<Vec<(String, u32)>>>,
    }

    impl Layer for Probe {
        fn name(&self) -> &'static str {
            "h3 ship models only"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.layer.prepare(cx, &self.groups);
            *self.drawn.borrow_mut() = self.layer.drawn();
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            self.layer.draw(pass, cx);
        }
    }

    /// The draws [`ShipModels`] makes of `groups` in `view`: deck's layer ids and their models.
    fn draws(gpu: &Gpu, view: ViewState, groups: Vec<ModelGroup>) -> Vec<(String, u32)> {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let drawn = Rc::new(RefCell::new(Vec::new()));
        let layer = ShipModels::new(&renderer.init(gpu));
        renderer.add_layer(Box::new(Probe { layer, groups, drawn: drawn.clone() }));
        render_with(gpu, &mut renderer, &SceneInput::new(view, [400.0, 200.0], 1.0, Basemap::Dark), 1.0).expect("rendered");
        drawn.borrow().clone()
    }

    fn save(image: &Image, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }

    /// `[x, y]` on the canvas of `view` (CSS px) of `[lon, lat]` raised `z` metres.
    fn screen(view: &ViewState, [lon, lat]: [f64; 2], z: f64) -> [f64; 2] {
        let [x, y, _] = viewport_for(view, SIZE).project([lon, lat, z]);
        [x, y]
    }

    /// How far what is drawn reaches along the line through `[x0, y0]` going `[dx, dy]` (a unit
    /// step along a row or a column), px: the run of pixels that differ from the ground, each end
    /// pixel counted by how much of the drawn colour it shows (against the strongest of the three
    /// pixels inside it), as its share of the pixel covered.
    fn extent(image: &Image, [x0, y0]: [i64; 2], [dx, dy]: [i64; 2]) -> f64 {
        let ground = image.pixel(2, 2).map(f64::from);
        let at = |k: i64| image.pixel((x0 + k * dx) as u32, (y0 + k * dy) as u32).map(f64::from);
        let off = |k: i64| {
            let p = at(k);
            [p[0] - ground[0], p[1] - ground[1], p[2] - ground[2]]
        };
        let size = |v: [f64; 3]| v.iter().map(|c| c * c).sum::<f64>().sqrt();
        let inside = |k: i64| {
            let (x, y) = (x0 + k * dx, y0 + k * dy);
            x >= 0 && y >= 0 && x < i64::from(image.width) && y < i64::from(image.height)
        };
        let on = |k: i64| inside(k) && size(off(k)) > 6.0;
        let (mut a, mut b) = (0, 0);
        while on(a - 1) {
            a -= 1;
        }
        while on(b + 1) {
            b += 1;
        }
        let share = |end: i64, inner: [i64; 3]| {
            let q = inner.map(off).into_iter().max_by(|u, v| size(*u).total_cmp(&size(*v))).unwrap();
            let p = off(end);
            ((p[0] * q[0] + p[1] * q[1] + p[2] * q[2]) / (size(q) * size(q))).clamp(0.0, 1.0)
        };
        (b - a - 1) as f64 + share(a, [a + 1, a + 2, a + 3]) + share(b, [b - 1, b - 2, b - 3])
    }

    #[test]
    fn each_model_is_its_classs_mesh_turned_to_its_heading_stretched_and_offset() {
        let view = ViewFrame::new(&VIEW, SIZE, 1.0, 0.0);
        let mut s = ship(1, 4, 120.0, [150, 50, 4, 16], [720.0, 450.0], &VIEW);
        s.old = true;
        let scene = ships_scene(&VIEW, std::slice::from_ref(&s), None);
        assert_eq!(scene.models.len(), 1);
        let m = scene.models[0].models[0];
        let place = ship_placement(&s);
        assert_eq!(ship_class_of(&s), ShipClass::Container);
        assert_eq!((m.scale, m.heading), (place.scale, 120.0));
        let i = instance(&view, &m);
        assert_eq!((i.color, i.highlight), (SHIP_MESH_OLD_COLOR, Instance::NO_HIGHLIGHT));
        // the hull's centre 50 m ahead and 6 m to starboard of the reported position
        let frame = local_frame(&view, [m.position[0], m.position[1]]);
        let h = 120f64.to_radians();
        let (east, north) = (6.0 * h.cos() + 50.0 * h.sin(), -6.0 * h.sin() + 50.0 * h.cos());
        assert!((m.east - east).abs() < 1e-9 && (m.north - north).abs() < 1e-9, "{m:?}");
        let want = view.common_offset([s.lon, s.lat], 0.0).as_dvec3() + frame * DVec3::new(east, north, 0.0);
        let got = glam::Vec3::from(i.offset).as_dvec3();
        assert!((got - want).length() < 1e-4 * frame.x_axis.length(), "{got} against {want}");
        // the model's axes: y (along, bow at +0.5) to the heading, its length; x (across) to
        // starboard, the beam over the model's; z up, `up`
        let columns = DMat3::from_cols_array_2d(&i.columns.map(|c| c.map(f64::from)));
        let [sx, sy, sz] = m.scale;
        let want = frame * DMat3::from_cols(DVec3::new(h.cos(), -h.sin(), 0.0) * sx, DVec3::new(h.sin(), h.cos(), 0.0) * sy, DVec3::Z * sz);
        for (a, b) in columns.to_cols_array().iter().zip(want.to_cols_array()) {
            assert!((a - b).abs() <= 1e-6 * want.y_axis.length(), "{columns:?} against {want:?}");
        }
        assert_eq!(instance(&view, &ShipModel { highlighted: true, color: SHIP_MESH_COLOR, ..m }).highlight, MESH_HIGHLIGHT);
        assert_eq!(layer_id(ShipClass::Sar), "ship-mesh-sar");
        assert_eq!((MATERIAL.ambient, MATERIAL.diffuse, MATERIAL.shininess, MATERIAL.specular_color), (0.5, 0.48, 24.0, [40.0; 3]));
        assert_eq!(STATE, DrawState::NO_CULL);
    }

    /// One ship of every class: ten draws `ship-mesh-<class>` in the order each class comes up in
    /// the list, each its class's ships; none without models.
    #[test]
    fn one_draw_per_class_in_the_order_they_come() {
        let Some(gpu) = gpu() else { return };
        // kind and length for each class, the cargo ships' class by MMSI
        let fleet: [(u32, u8, u16, ShipClass); 11] = [
            (1, 2, 30, ShipClass::Tug),
            (2, 4, 200, ShipClass::Container),
            (3, 6, 120, ShipClass::Tanker),
            (6, 4, 200, ShipClass::Bulk),
            (5, 4, 90, ShipClass::Cargo),
            (7, 3, 80, ShipClass::Passenger),
            (8, 5, 20, ShipClass::Fishing),
            (9, 8, 12, ShipClass::Pleasure),
            (10, 7, 20, ShipClass::Sar),
            (11, 0, 60, ShipClass::Generic),
            (12, 6, 100, ShipClass::Tanker),
        ];
        let view = ViewState { zoom: 17.0, ..VIEW };
        let ships: Vec<Ship> = fleet
            .iter()
            .enumerate()
            .map(|(k, &(mmsi, kind, length, _))| ship(mmsi, kind, 0.0, [length / 2, length / 2, 2, 2], [60.0 + 120.0 * k as f64, 450.0], &view))
            .collect();
        for (s, f) in ships.iter().zip(&fleet) {
            assert_eq!(ship_class_of(s), f.3, "{}", s.mmsi);
        }
        let scene = ships_scene(&view, &ships, None);
        let want: Vec<(String, u32)> = fleet[..10].iter().map(|f| (layer_id(f.3), if f.3 == ShipClass::Tanker { 2 } else { 1 })).collect();
        assert_eq!(draws(&gpu, view, scene.models), want);
        assert_eq!(draws(&gpu, view, Vec::new()), []);
        assert_eq!(draws(&gpu, view, vec![ModelGroup { cls: ShipClass::Tug, models: Vec::new() }]), []);
        // each class's mesh draws
        let image = shoot(&gpu, view, 1.0, ships_scene(&view, &ships, None));
        save(&image, "h3-ship-classes.png");
        let ground = image.pixel(2, 2);
        for (k, s) in ships.iter().enumerate().take(10) {
            let [x, y] = screen(&view, [s.lon, s.lat], 0.0);
            let drawn = (y as u32 - 30..y as u32 + 30).filter(|&py| (0..3).any(|c| image.pixel(x as u32, py)[c].abs_diff(ground[c]) > 12)).count();
            assert!(drawn > 4, "ship {k} ({:?}) draws {drawn} px", fleet[k].3);
        }
    }

    /// V6 at zoom 15 straight down, the whole canvas: a 100 m cargo ship heading east, an 80 m
    /// passenger ship heading north, a 30 m tug heading west, each with its hull's middle (not its
    /// AIS reference point) on the middle of the canvas's centre pixel in turn, so the pixels'
    /// centres run along the hull's centre line out to its pointed bow (and the camera's
    /// perspective pushes what stands up only a fraction of a pixel outward). Along the hull's line
    /// its model is as long as the ship, `length / mpp` px, within a pixel; across, its beam.
    #[test]
    fn v6_at_zoom_15_a_hull_is_as_long_on_the_screen_as_the_ship() {
        let Some(gpu) = gpu() else { return };
        let mpp = meters_per_pixel(VIEW.zoom, VIEW.latitude);
        let middle = [SIZE[0] / 2.0 + 0.5, SIZE[1] / 2.0 + 0.5];
        for (mmsi, kind, heading, dims, name) in [
            (1, 4, 90.0, [60, 40, 7, 9], "h3-v6-z15-cargo.png"),
            (2, 3, 0.0, [40, 40, 7, 7], "h3-v6-z15-passenger.png"),
            (3, 2, 270.0, [12, 18, 5, 5], "h3-v6-z15-tug.png"),
        ] {
            // the reported position: back from the hull's middle by the hull's offset
            let mut s = ship(mmsi, kind, heading, dims, middle, &VIEW);
            let place = ship_placement(&s);
            let (lat, lon) = ahead((s.lat, s.lon), f64::from(heading), -place.ahead);
            (s.lat, s.lon) = ahead((lat, lon), f64::from(heading) + 90.0, -place.right);
            let scene = ships_scene(&VIEW, std::slice::from_ref(&s), None);
            assert!(scene.icons.is_empty(), "{name}: a model, not an icon");
            let image = shoot(&gpu, VIEW, 1.0, scene);
            save(&image, name);
            let (along, across) = if heading == 0.0 { ([0, 1], [1, 0]) } else { ([1, 0], [0, 1]) };
            let middle = [middle[0] as i64, middle[1] as i64];
            let got = extent(&image, middle, along);
            let want = place.length / mpp;
            eprintln!("{name}: {got:.2} px long, {want:.2} px the ship ({} m)", place.length);
            assert!((got - want).abs() <= 1.0, "{name}: {got:.2} px against {want:.2}");
            // across, the beam (the bridge wings and bulwarks stand within it)
            let wide = extent(&image, middle, across);
            let beam = place.beam / mpp;
            assert!((wide - beam).abs() <= 1.0, "{name}: {wide:.2} px wide against {beam:.2}");
        }
    }

    /// A generic ship heading east, straight down and then tilted 60° to see its starboard side
    /// (south), at zoom 18, opaque and drawn as the scene's rig and `SHIP_MESH_MATERIAL` light it,
    /// as the CPU twin of the shader works it out: the forecastle's top (off-white, facing up) and
    /// the deckhouse's starboard side (white, facing south, away from the key light) each as lit,
    /// not as flat paint. An old position is faded to alpha 120 over the map.
    #[test]
    fn decks_and_sides_are_lit_as_deck_lights_them() {
        let Some(gpu) = gpu() else { return };
        const OFF_WHITE: [f64; 3] = [214.0, 216.0, 212.0];
        const WHITE: [f64; 3] = [236.0, 238.0, 240.0];
        let mut checks: Vec<([u8; 4], [f64; 3], String)> = Vec::new();
        let scale = 2.0;
        for pitch in [0.0, 60.0] {
            let view = ViewState { pitch, zoom: 18.0, ..VIEW };
            let centre = [SIZE[0] / 2.0, SIZE[1] / 2.0];
            let s = ship(1, 0, 90.0, [30, 30, 5, 5], centre, &view);
            assert_eq!(ship_class_of(&s), ShipClass::Generic);
            let place = ship_placement(&s);
            let [_, length, up] = place.scale;
            let image = shoot(&gpu, view, scale, ships_scene(&view, std::slice::from_ref(&s), None));
            save(&image, &format!("h3-ship-lit-pitch-{pitch}.png"));
            let rig = SceneInput::new(view, SIZE, scale, Basemap::Dark).lights;
            let key = DVec3::from_array(rig.key_direction);
            let frame = ViewFrame::new(&view, SIZE, scale, 0.0);
            let camera = glam::Vec3::from(frame.globals.camera_common).as_dvec3();
            let lit = |rgb: [f64; 3], normal: DVec3, at: [f64; 2], h: f64| {
                let to_camera = camera - frame.common_offset(at, h).as_dvec3();
                rig.shade(&MATERIAL, rgb.map(|c| c / 255.0), normal, to_camera, key).map(|c| c.min(1.0) * 255.0)
            };
            let pixel = |at: [f64; 2], h: f64| {
                let [x, y] = screen(&view, at, h);
                image.pixel((x * scale) as u32, (y * scale) as u32)
            };
            if pitch == 0.0 {
                // the forecastle's top: 0.36..0.44 of the length forward, 0.085 up
                let (lat, lon) = ahead((s.lat, s.lon), 90.0, 0.4 * length);
                let h = 0.085 * up;
                checks.push((pixel([lon, lat], h), lit(OFF_WHITE, DVec3::Z, [lon, lat], h), "forecastle top".into()));
            } else {
                // the deckhouse's starboard face: 0.4 of the beam to starboard (south), between
                // 0.38 and 0.22 of the length aft, its middle 0.10 up
                let (lat, lon) = ahead((s.lat, s.lon), 270.0, 0.3 * length);
                let (lat, lon) = ahead((lat, lon), 180.0, 0.4 * place.beam);
                let h = 0.10 * up;
                checks.push((pixel([lon, lat], h), lit(WHITE, -DVec3::Y, [lon, lat], h), "deckhouse side".into()));
            }
        }
        for (got, want, what) in &checks {
            eprintln!("{what}: {got:?} against {want:?}");
            for c in 0..3 {
                assert!((f64::from(got[c]) - want[c]).abs() <= 3.0, "{what}: {got:?} against {want:?}");
            }
        }
        // the side and the top are not in their paints' ratio, as flat colours would be
        let (top, side) = (checks[0].0, checks[1].0);
        let ratio = f64::from(side[1]) / f64::from(top[1]);
        assert!((ratio - WHITE[1] / OFF_WHITE[1]).abs() > 0.1, "side {side:?}, top {top:?}");
        // an old position: the same forecastle at alpha 120 over the map
        let view = ViewState { zoom: 18.0, ..VIEW };
        let centre = [SIZE[0] / 2.0, SIZE[1] / 2.0];
        let mut s = ship(1, 0, 90.0, [30, 30, 5, 5], centre, &view);
        s.old = true;
        let image = shoot(&gpu, view, scale, ships_scene(&view, std::slice::from_ref(&s), None));
        let ground = image.pixel(2, 2);
        let [_, length, up] = ship_placement(&s).scale;
        let (lat, lon) = ahead((s.lat, s.lon), 90.0, 0.4 * length);
        let [x, y] = screen(&view, [lon, lat], 0.085 * up);
        let p = image.pixel((x * scale) as u32, (y * scale) as u32);
        save(&image, "h3-ship-old.png");
        // the forecastle's top is the nearest face there, at 120 over whatever the model's faces
        // drawn before it (the deck, the keel) left over the map
        let a = 120.0 / 255.0;
        for c in 0..3 {
            let (top, p) = (f64::from(checks[0].0[c]), f64::from(p[c]));
            assert!(p >= a * top - 3.0 && p <= a * top + (1.0 - a) * 255.0 + 3.0, "{p} against {top} at 120 over anything");
            assert!(p < top - 20.0, "faded: {p} against {top}");
        }
        assert!((0..3).any(|c| p[c].abs_diff(ground[c]) > 30), "drawn: {p:?} over {ground:?}");
    }

    /// The ship hovered in the sidebar's list is blended toward the selection blue; the other as
    /// it was.
    #[test]
    fn the_hovered_ship_is_highlighted() {
        let Some(gpu) = gpu() else { return };
        let ships = [ship(1, 6, 90.0, [40, 40, 6, 6], [620.0, 450.0], &VIEW), ship(2, 6, 90.0, [40, 40, 6, 6], [820.0, 450.0], &VIEW)];
        let plain = shoot(&gpu, VIEW, 1.0, ships_scene(&VIEW, &ships, None));
        let lit = shoot(&gpu, VIEW, 1.0, ships_scene(&VIEW, &ships, Some(2)));
        save(&lit, "h3-ship-highlight.png");
        let at = |image: &Image, s: &Ship| {
            let [x, y] = screen(&VIEW, [s.lon, s.lat], 3.0);
            image.pixel(x as u32, y as u32)
        };
        let (before, after) = (at(&plain, &ships[1]), at(&lit, &ships[1]));
        assert!(after[2] > before[2] + 5 && u16::from(after[0]) + 30 < u16::from(before[0]), "{before:?} to {after:?}");
        assert_eq!(at(&plain, &ships[0]), at(&lit, &ships[0]), "the other ship as it was");
    }
}
