//! The trains close in, as their cars in 3D (item T3; doc 02 §5.5–5.6, `carLayers`,
//! G:1998-2029): row 15 of doc 02 §2 (doc 03 §3 row 15) in place of the icons, drawn by the trains
//! layer ([`super::trains`]) in the icons' slot.
//!
//! The scene builder has already laid every car out ([`CarLayers`],
//! [`rustradar_scene::trains::TrainBuilder::car_layers`]): where its middle is, the way it points,
//! its length, its level on the track and its colour, and the width and height every car is drawn
//! at. This part draws them as deck's SimpleMeshLayers with
//! [`crate::pipelines::mesh::MeshPipeline::instanced`], one draw per layer of
//! [`CarLayers::layers`] in that order (per mesh, in the order of its first car: the cars on the
//! ground and on bridges, then those in tunnels):
//!
//! | Draw | Cars | State | Colour |
//! |---|---|---|---|
//! | `train-car-meshes-<key>` | the mesh's cars not in a tunnel | [`STATE`] (`NO_CULL`: depth tested and written) | the train's at 240, or white for a model's car |
//! | `train-car-meshes-<key>-tunnel` | its cars in tunnels | [`TUNNEL_STATE`] (`THROUGH_GROUND`: depth always, not written) | the same at alpha 90 |
//!
//! - **Meshes**: a plain car is [`rustradar_scene::train_models::car_mesh`] by its key
//!   (`<shape>-cab`, `<coach shape>-coach`), grey shades in its vertex colours multiplied by the
//!   train's colour; a model's car is one of the operator models of `static/trains`, embedded
//!   ([`INDEX_JSON`], [`MESHES_BIN`]) and read once per process ([`models`]; the scene builder hands
//!   the same models to the layout, for the cars' lengths), its livery in its vertex colours. Each
//!   mesh is unindexed (three vertices a triangle) and uploaded the first time a car takes it.
//! - **Instances**: at `[lon, lat, level height]`, deck's orientation `[0, −heading, 0]` and scale
//!   `[width, length, height]` ([`instance`]: the unit box's x across, y along with the front at
//!   +0.5, z up), in a true east, north, up frame at the car (plan 1.4.2); the car under the
//!   pointer highlighted [`CAR_HIGHLIGHT`].
//! - **Light**: [`MATERIAL`] (`CAR_MESH_MATERIAL`) and the scene's light rig. The models' normals
//!   were divided by the scale `[3.1, length, 4.1]` when they were built; the pipeline multiplies
//!   normals by the instance's scale and renormalises, as deck does, so they come out right (doc 02
//!   §14.3.6).
//!
//! Instances are rebuilt in each `prepare` from the scene and this frame's view and written to the
//! GPU only when they change.
//!
//! Deviations: none intended. The meshes are on the GPU from the first car that takes them (today
//! the models arrive by `fetch` after start-up and plain cars stand in meanwhile; here they are
//! embedded).

use std::collections::HashMap;
use std::sync::OnceLock;

use glam::DVec3;
use rustradar_scene::train_models::{CarMesh, TrainModels, car_mesh};
use rustradar_scene::trains::{CAR_HIGHLIGHT, CarLayers, CarShape, DrawnCar};

use super::{DrawCx, Init, Pass, PrepareCx};
use crate::frame::ViewFrame;
use crate::pipelines::mesh::{Instance, Instances, Material, MeshPipeline, Shape, Style, StyleUniform, Vertex, orientation_matrix};
use crate::state::DrawState;

/// `static/trains/index.json`: each model's three cars, their lengths and their sections of
/// [`MESHES_BIN`] ([`rustradar_scene::train_models::INDEX_FILE`]).
pub static INDEX_JSON: &[u8] = include_bytes!("../../../../static/trains/index.json");
/// `static/trains/meshes.bin`: every model car's triangles.
pub static MESHES_BIN: &[u8] = include_bytes!("../../../../static/trains/meshes.bin");

/// The operator models, read from the embedded files once per process (`loadTrainModels()`);
/// `None` if they cannot be read (logged once: "train models unavailable"), when plain cars are
/// drawn for every train, as today until (or unless) they load.
pub fn models() -> Option<&'static TrainModels> {
    static MODELS: OnceLock<Option<TrainModels>> = OnceLock::new();
    MODELS
        .get_or_init(|| match TrainModels::parse(INDEX_JSON, MESHES_BIN) {
            Ok(models) => Some(models),
            Err(e) => {
                log::warn!("train models unavailable: {e}");
                None
            }
        })
        .as_ref()
}

/// The cars' material (`CAR_MESH_MATERIAL`, G:1960).
pub const MATERIAL: Material = Material { ambient: 0.45, diffuse: 0.6, shininess: 40.0, specular_color: [70.0; 3], unlit: false };

/// The cars on the ground and on bridges (`parameters: NO_CULL`): depth tested and written, no
/// culling.
pub const STATE: DrawState = DrawState::NO_CULL;
/// The cars in tunnels (`THROUGH_GROUND`): no culling, depth always, not written, so they are seen
/// faintly through the ground and whatever stands on it.
pub const TUNNEL_STATE: DrawState = DrawState::THROUGH_GROUND;

/// The triangles of a car's mesh: a plain car's, or a model's from [`models`] (none if they are not
/// loaded or lack it).
pub fn mesh_of(shape: &CarShape) -> Option<&'static CarMesh> {
    match shape {
        CarShape::Plain { lead, shape } => Some(car_mesh(*lead, *shape).1),
        CarShape::Model { model, index } => Some(&models()?.cars(model)?.get(*index)?.mesh),
    }
}

/// One car's instance in `view`, every car `width` by `height` metres: the unit box at its
/// middle, raised to its level's height, turned to its heading (`getOrientation: [0, −heading,
/// 0]`) and stretched to `[width, length, height]` (`getScale`), in its colour, highlighted
/// [`CAR_HIGHLIGHT`] when it is under the pointer.
pub fn instance(view: &ViewFrame, car: &DrawnCar, width: f64, height: f64) -> Instance {
    let model = orientation_matrix([0.0, -car.heading, 0.0], [width, car.length, height]);
    let highlight = if car.highlighted { CAR_HIGHLIGHT } else { Instance::NO_HIGHLIGHT };
    Instance::at(view, [car.at[0], car.at[1], car.level.height], model, DVec3::ZERO, car.color, highlight)
}

/// One draw: a layer's mesh and its instances on the GPU, kept between frames.
struct Draw {
    id: String,
    key: String,
    tunnel: bool,
    instances: Instances,
    written: Vec<Instance>,
}

/// Row 15 close in: every train's cars.
pub struct TrainCars {
    ground: MeshPipeline,
    tunnel: MeshPipeline,
    ground_style: Style,
    tunnel_style: Style,
    /// Each mesh taken so far, by key.
    shapes: HashMap<String, Shape>,
    /// This frame's draws (the first [`Self::active`]), reused across frames.
    draws: Vec<Draw>,
    active: usize,
}

impl TrainCars {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let style = StyleUniform::new(&MATERIAL, 1.0, [255; 4]);
        let ground = MeshPipeline::instanced(init, "train cars", STATE);
        let tunnel = MeshPipeline::instanced(init, "train cars in tunnels", TUNNEL_STATE);
        let (ground_style, tunnel_style) = (ground.style(device, &style), tunnel.style(device, &style));
        TrainCars { ground, tunnel, ground_style, tunnel_style, shapes: HashMap::new(), draws: Vec::new(), active: 0 }
    }

    /// Once a frame: the draws of `cars` ([`CarLayers::layers`]) in this frame's view, each mesh
    /// uploaded the first time it is taken, and the scene's lights.
    pub fn prepare(&mut self, cx: &PrepareCx<'_>, cars: &CarLayers) {
        self.active = 0;
        if cars.cars.is_empty() {
            return;
        }
        let (device, queue) = (&cx.gpu.device, &cx.gpu.queue);
        for layer in cars.layers() {
            if !self.shapes.contains_key(&layer.key) {
                // a mesh the models lack draws nothing (deck's layer without a mesh)
                let Some(mesh) = mesh_of(&layer.mesh) else { continue };
                let vertices = Vertex::from_arrays(&mesh.positions, &mesh.normals, Some(&mesh.colors));
                self.shapes.insert(layer.key.clone(), Shape::new(device, &format!("train car {}", layer.key), &vertices, None));
            }
            let instances: Vec<Instance> = layer.cars.iter().map(|&c| instance(cx.view, &cars.cars[c], cars.width, cars.height)).collect();
            if self.draws.len() == self.active {
                self.draws.push(Draw {
                    id: String::new(),
                    key: String::new(),
                    tunnel: false,
                    instances: Instances::new(device, "train car instances"),
                    written: Vec::new(),
                });
            }
            let draw = &mut self.draws[self.active];
            (draw.id, draw.key, draw.tunnel) = (layer.id, layer.key, layer.tunnel);
            if instances != draw.written {
                draw.instances.write(device, queue, &instances);
                draw.written = instances;
            }
            self.active += 1;
        }
        if self.active > 0 {
            cx.set_lights(&self.ground);
            cx.set_lights(&self.tunnel);
        }
    }

    /// Every layer's cars, in the layers' order (map pass only).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if cx.pass != Pass::Map {
            return;
        }
        for draw in &self.draws[..self.active] {
            let Some(shape) = self.shapes.get(&draw.key) else { continue };
            let (pipeline, style) = if draw.tunnel { (&self.tunnel, &self.tunnel_style) } else { (&self.ground, &self.ground_style) };
            pipeline.draw(pass, style, shape, &draw.instances);
        }
    }

    /// This frame's draws in order: deck's layer id and how many cars each draws.
    pub fn drawn(&self) -> Vec<(String, u32)> {
        self.draws[..self.active].iter().map(|d| (d.id.clone(), d.instances.count())).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use glam::DMat3;
    use rustradar_core::geo::Basemap;
    use rustradar_core::model::{Train, TrainKind, TrainShape, TrainSource};
    use rustradar_scene::bridges::{GROUND, Level};
    use rustradar_scene::camera::{ViewState, viewport_for};
    use rustradar_scene::trains::{TrainBuilder, TrainsScene};

    use super::*;
    use crate::gpu::Gpu;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::Layer;
    use crate::pipelines::mesh::local_frame;
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 10:00 UTC.
    const NOW: i64 = 1_782_036_000_000;
    /// The canvas, CSS px.
    const SIZE: [f64; 2] = [640.0, 240.0];
    /// Stockholm at zoom 16, straight down: the cars, 26 m to 41.7 px.
    const VIEW: ViewState = ViewState { longitude: 18.07, latitude: 59.33, zoom: 16.0, bearing: 0.0, pitch: 0.0, position: None };
    const M_PER_DEG: f64 = 111_320.0;

    /// `(lat, lon)` `m` metres from `at` going `heading`.
    fn ahead((lat, lon): (f64, f64), heading: f64, m: f64) -> (f64, f64) {
        let h = heading.to_radians();
        (lat + h.cos() * m / M_PER_DEG, lon + h.sin() * m / (M_PER_DEG * lat.to_radians().cos()))
    }

    /// A train of `cars` standing at `xy` on the canvas of `view`, heading `heading`, its frames
    /// starting there and its track known 400 m back.
    fn train(view: &ViewState, xy: [f64; 2], heading: f64, cars: u32) -> Train {
        let [lon, lat] = viewport_for(view, SIZE).unproject(xy, 0.0);
        let start = (lat, lon);
        Train {
            id: format!("t{}", xy[0]),
            source: TrainSource::Trafikverket,
            name: "Train 1".into(),
            kind: TrainKind::Regional,
            lat,
            lon,
            heading: Some(heading as f32),
            speed_kmh: Some(36.0),
            destination: None,
            operator: None,
            delay_minutes: None,
            position_unix: NOW / 1000,
            frames: (0..=10).map(|s| {
                let (lat, lon) = ahead(start, heading, 10.0 * f64::from(s));
                (lat, lon, NOW + i64::from(s) * 1000)
            }).collect(),
            reports: Vec::new(),
            predicted: Vec::new(),
            cars: Some(cars),
            tail: [0.0, 200.0, 400.0].map(|m| ahead(start, heading, -m)).to_vec(),
            structures: Vec::new(),
            ended_unix: None,
            shape: None,
            model: None,
        }
    }

    /// The cars of `trains` as the scene builder lays them out at `NOW` in `view`.
    fn layout(view: &ViewState, trains: &[Train]) -> CarLayers {
        let list: Vec<&Train> = trains.iter().collect();
        let mut builder = TrainBuilder::new();
        assert!(builder.build(&list, true, view, None, NOW).cars, "close in: cars");
        builder.car_layers(&list, view, SIZE, models(), NOW)
    }

    /// `cars` drawn by the trains layer over the plain map of `view` (the scene builder's
    /// wiring: `scene.trains.scene.car_layers`).
    fn shoot(gpu: &Gpu, view: ViewState, scale: f64, cars: CarLayers) -> Image {
        let mut scene = SceneInput::new(view, SIZE, scale, Basemap::Dark);
        scene.local_now_ms = NOW as f64;
        scene.feed_now_ms = NOW as f64;
        scene.trains.scene = TrainsScene { cars: true, car_layers: cars, ..TrainsScene::default() };
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        render_with(gpu, &mut renderer, &scene, scale).expect("rendered")
    }

    /// A test layer: [`TrainCars`] drawing `cars` and nothing else, keeping what it drew.
    struct Probe {
        layer: TrainCars,
        cars: CarLayers,
        drawn: Rc<RefCell<Vec<(String, u32)>>>,
    }

    impl Layer for Probe {
        fn name(&self) -> &'static str {
            "t3 cars only"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.layer.prepare(cx, &self.cars);
            *self.drawn.borrow_mut() = self.layer.drawn();
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            self.layer.draw(pass, cx);
        }
    }

    /// The draws [`TrainCars`] makes of `cars` in `view`: deck's layer ids and their cars.
    fn draws(gpu: &Gpu, view: ViewState, cars: CarLayers) -> Vec<(String, u32)> {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let drawn = Rc::new(RefCell::new(Vec::new()));
        let layer = TrainCars::new(&renderer.init(gpu));
        renderer.add_layer(Box::new(Probe { layer, cars, drawn: drawn.clone() }));
        render_with(gpu, &mut renderer, &SceneInput::new(view, SIZE, 1.0, Basemap::Dark), 1.0).expect("rendered");
        drawn.borrow().clone()
    }

    fn save(image: &Image, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }

    /// Where `car`'s ends are, `[lon, lat]`: its front and its back.
    fn ends(car: &DrawnCar) -> ([f64; 2], [f64; 2]) {
        let at = (car.at[1], car.at[0]);
        let ((f_lat, f_lon), (b_lat, b_lon)) = (ahead(at, car.heading, car.length / 2.0), ahead(at, car.heading, -car.length / 2.0));
        ([f_lon, f_lat], [b_lon, b_lat])
    }

    /// `[x, y]` on the canvas of `view` (CSS px) of `[lon, lat]` raised `z` metres.
    fn screen(view: &ViewState, [lon, lat]: [f64; 2], z: f64) -> [f64; 2] {
        let [x, y, _] = viewport_for(view, SIZE).project([lon, lat, z]);
        [x, y]
    }

    #[test]
    fn the_operator_models_are_embedded_with_their_cars_lengths() {
        let models = models().expect("static/trains reads");
        let mut names: Vec<&str> = models.names().collect();
        names.sort_unstable();
        assert_eq!(names, ["contessa", "sj-intercity", "x2000", "x40", "x55", "x60", "x61-skane", "x61-vasttrafik"]);
        // doc 02 §5.6's table
        for (model, lengths, vertices) in [
            ("contessa", [26.2, 26.6, 26.3], [4629, 4602, 4602]),
            ("x40", [27.2, 26.4, 27.8], [4416, 4506, 4710]),
            ("x60", [20.35, 16.4, 20.75], [4314, 4548, 4404]),
            ("x2000", [17.7, 25.0, 22.7], [4425, 4329, 4638]),
        ] {
            let cars = models.cars(model).unwrap();
            assert_eq!(cars.iter().map(|c| c.length).collect::<Vec<_>>(), lengths, "{model}");
            assert_eq!(cars.iter().map(|c| c.mesh.vertices()).collect::<Vec<_>>(), vertices, "{model}");
        }
        let shape = CarShape::Model { model: "x55".into(), index: 2 };
        assert_eq!(mesh_of(&shape).map(CarMesh::vertices), Some(4488));
        assert_eq!(mesh_of(&CarShape::Model { model: "x55".into(), index: 3 }), None);
        assert_eq!(mesh_of(&CarShape::Model { model: "x99".into(), index: 0 }), None);
        let plain = CarShape::Plain { lead: true, shape: TrainShape::Loco };
        assert_eq!(mesh_of(&plain).map(CarMesh::vertices), Some(132));
    }

    #[test]
    fn each_car_is_the_unit_box_turned_to_its_heading_stretched_to_its_size_at_its_level() {
        let view = ViewFrame::new(&VIEW, SIZE, 1.0, 0.0);
        let car = DrawnCar {
            at: [18.071, 59.3305],
            heading: 120.0,
            length: 25.4,
            train: 0,
            mesh: CarShape::Plain { lead: false, shape: TrainShape::Regional },
            modelled: false,
            level: Level { tunnel: false, height: 4.5 },
            color: [96, 212, 160, 240],
            highlighted: false,
        };
        let i = instance(&view, &car, 3.1, 4.1);
        assert_eq!(i.offset, <[f32; 3]>::from(view.common_offset(car.at, 4.5)), "its middle, up on its bridge");
        assert_eq!((i.color, i.highlight), ([96, 212, 160, 240], Instance::NO_HIGHLIGHT));
        // the unit box's axes: y (along) to the heading's way, 25.4 m; x (across) to its right, 3.1
        // m; z up, 4.1 m; in common units at the view centre
        let columns = DMat3::from_cols_array_2d(&i.columns.map(|c| c.map(f64::from)));
        let frame = local_frame(&view, car.at);
        let h = 120f64.to_radians();
        let want = frame * DMat3::from_cols(DVec3::new(h.cos(), -h.sin(), 0.0) * 3.1, DVec3::new(h.sin(), h.cos(), 0.0) * 25.4, DVec3::Z * 4.1);
        for (a, b) in columns.to_cols_array().iter().zip(want.to_cols_array()) {
            assert!((a - b).abs() <= 1e-6 * want.x_axis.length(), "{columns:?} against {want:?}");
        }
        assert_eq!(instance(&view, &DrawnCar { highlighted: true, ..car }, 3.1, 4.1).highlight, CAR_HIGHLIGHT);
    }

    /// Straight down at zoom 16: a train of six plain cars running east, every other car recoloured
    /// so each can be told from the next. Along the train's middle each car covers its own length
    /// on the map within a pixel, the front car's front where the train stands, the cars running
    /// back from there.
    #[test]
    fn each_car_is_as_long_on_the_screen_as_on_the_map() {
        let Some(gpu) = gpu() else { return };
        let head = [560.0, 120.0];
        let mut cars = layout(&VIEW, &[train(&VIEW, head, 90.0, 6)]);
        assert_eq!(cars.cars.len(), 6);
        let colours = [[230, 40, 40, 255], [40, 230, 40, 255]];
        for (k, car) in cars.cars.iter_mut().enumerate() {
            car.color = colours[k % 2];
        }
        let image = shoot(&gpu, VIEW, 1.0, cars.clone());
        save(&image, "t3-cars-lengths.png");
        assert_eq!(draws(&gpu, VIEW, cars.clone()), [("train-car-meshes-regional-cab".to_string(), 1), ("train-car-meshes-regional-coach".to_string(), 5)]);
        // along the row through the train's middle: each car's extent, its colour's first to last
        // pixel between its neighbours'
        let y = head[1] as u32;
        let red = |p: [u8; 4]| p[0] > 2 * p[1] && p[0] > 20;
        let green = |p: [u8; 4]| p[1] > 2 * p[0] && p[1] > 20;
        let mut extents: Vec<(f64, f64)> = Vec::new();
        let mut x = image.width;
        for (k, car) in cars.cars.iter().enumerate() {
            let is = |p| if k % 2 == 0 { red(p) } else { green(p) };
            // from the east, the first pixel of this car's colour, then on to its last
            let start = (0..x).rev().find(|&px| is(image.pixel(px, y))).unwrap_or_else(|| panic!("car {k} drawn"));
            let other = |p| if k % 2 == 0 { green(p) } else { red(p) };
            let end = (0..=start).rev().take_while(|&px| !other(image.pixel(px, y))).filter(|&px| is(image.pixel(px, y))).last().unwrap();
            extents.push((f64::from(end), f64::from(start) + 1.0));
            x = end;
            let (front, back) = ends(car);
            let want = screen(&VIEW, front, 0.0)[0] - screen(&VIEW, back, 0.0)[0];
            let got = f64::from(start + 1 - end);
            assert!((got - want).abs() <= 1.0, "car {k}: {got} px against {want:.2}");
        }
        // 25.4 m at 0.61 m/px; the front where the train stands; each car 26 m behind the last
        assert!((extents[0].1 - head[0]).abs() <= 1.0, "{extents:?}");
        for pair in extents.windows(2) {
            let step = pair[0].1 - pair[1].1;
            assert!((step - 26.0 / 0.6094).abs() <= 1.5, "{extents:?}");
        }
        // nothing north or south of the train but its width (3.1 m, 5 px)
        let across = (0..image.height).filter(|&py| red(image.pixel(extents[0].0 as u32 + 10, py))).count();
        assert!((4..=6).contains(&across), "{across} px across");
    }

    /// A coach straight down at the canvas centre: its roof (shade 0.75) lit by today's rig and
    /// `CAR_MESH_MATERIAL` as the CPU twin of the shader works it out, then tilted 60° to see its
    /// side (shade 1): each as lit, the side lit otherwise than the roof (its own normal, no key
    /// light on a north-lit face).
    #[test]
    fn roof_and_side_are_lit_as_deck_lights_them() {
        let Some(gpu) = gpu() else { return };
        let rgb = [96.0, 212.0, 160.0];
        let alpha = 240.0 / 255.0;
        let mut checks = Vec::new();
        for (pitch, name) in [(0.0, "t3-car-lit-top.png"), (60.0, "t3-car-lit-side.png")] {
            let view = ViewState { pitch, zoom: 18.0, ..VIEW };
            let centre = [SIZE[0] / 2.0, SIZE[1] / 2.0];
            let [lon, lat] = viewport_for(&view, SIZE).unproject(centre, 0.0);
            let car = DrawnCar {
                at: [lon, lat],
                heading: 90.0,
                length: 25.4,
                train: 0,
                mesh: CarShape::Plain { lead: false, shape: TrainShape::Regional },
                modelled: false,
                level: GROUND,
                color: [96, 212, 160, 240],
                highlighted: false,
            };
            let scale = 2.0;
            let image = shoot(&gpu, view, scale, CarLayers::new(vec![car], 0.15));
            save(&image, name);
            let rig = SceneInput::new(view, SIZE, scale, Basemap::Dark).lights;
            let key = DVec3::from_array(rig.key_direction);
            let ground = image.pixel(2, 2);
            // the way to the camera: up, or 30° above the south at pitch 60
            let to_camera = DVec3::new(0.0, -pitch.to_radians().sin(), pitch.to_radians().cos());
            let expect = |shade: f64, normal: DVec3| {
                let lit = rig.shade(&MATERIAL, rgb.map(|c| c / 255.0 * shade), normal, to_camera, key);
                [0, 1, 2].map(|c| (lit[c].min(1.0) * 255.0 * alpha + f64::from(ground[c]) * (1.0 - alpha)).round())
            };
            let at = |p: [f64; 2]| image.pixel((p[0] * scale) as u32, (p[1] * scale) as u32);
            // the roof's middle (4.1 m up), and, tilted, the lower body band's middle on the
            // south side (1.55 m south of the middle, 1.43 m up)
            let roof = at(screen(&view, car.at, 4.1));
            checks.push((roof, expect(0.75, DVec3::Z), format!("roof at pitch {pitch}")));
            if pitch > 0.0 {
                let (slat, slon) = ahead((lat, lon), 180.0, 1.55);
                let side = at(screen(&view, [slon, slat], 0.35 * 4.1));
                checks.push((side, expect(1.0, -DVec3::Y), "side".into()));
            }
        }
        for (got, want, what) in &checks {
            for c in 0..3 {
                assert!((f64::from(got[c]) - want[c]).abs() <= 4.0, "{what}: {got:?} against {want:?}");
            }
        }
        // lit, the side and the roof are not in their shades' ratio (1 : 0.75) as flat colours would be
        let (roof, side) = (checks[1].0, checks[2].0);
        let ratio = f64::from(side[1]) / f64::from(roof[1]);
        assert!((ratio - 1.0 / 0.75).abs() > 0.1, "side {side:?}, roof {roof:?}");
    }

    /// A car up on a bridge (6 m) and, below it, one in a tunnel: the tunnel's car is drawn after,
    /// through the bridge's car (depth always), at alpha 90; with the ordinary state it would be
    /// hidden under it.
    #[test]
    fn a_car_in_a_tunnel_is_seen_faintly_through_what_stands_over_it() {
        let Some(gpu) = gpu() else { return };
        let centre = [SIZE[0] / 2.0, SIZE[1] / 2.0];
        let [lon, lat] = viewport_for(&VIEW, SIZE).unproject(centre, 0.0);
        let car = |level: Level, color: [u8; 4]| DrawnCar {
            at: [lon, lat],
            heading: 0.0,
            length: 25.4,
            train: 0,
            mesh: CarShape::Plain { lead: false, shape: TrainShape::Regional },
            modelled: false,
            level,
            color,
            highlighted: false,
        };
        let bridge = car(Level { tunnel: false, height: 6.0 }, [232, 232, 236, 240]);
        let tunnel = car(Level { tunnel: true, height: 0.0 }, [226, 92, 196, 90]);
        let both = CarLayers::new(vec![bridge.clone(), tunnel.clone()], 0.6);
        let over = shoot(&gpu, VIEW, 1.0, both.clone());
        save(&over, "t3-car-tunnel.png");
        assert_eq!(draws(&gpu, VIEW, both), [("train-car-meshes-regional-coach".to_string(), 1), ("train-car-meshes-regional-coach-tunnel".to_string(), 1)]);
        let alone = shoot(&gpu, VIEW, 1.0, CarLayers::new(vec![bridge], 0.6));
        let faint = shoot(&gpu, VIEW, 1.0, CarLayers::new(vec![tunnel.clone()], 0.6));
        let opaque = shoot(&gpu, VIEW, 1.0, CarLayers::new(vec![DrawnCar { color: [226, 92, 196, 255], ..tunnel }], 0.6));
        let p = |image: &Image| image.pixel(centre[0] as u32, centre[1] as u32 + 4);
        let ground = over.pixel(2, 2);
        let a = 90.0 / 255.0;
        for c in 0..3 {
            // alone, a tunnel's car is its lit colour at 90 over the ground
            let want = f64::from(p(&opaque)[c]) * a + f64::from(ground[c]) * (1.0 - a);
            assert!((f64::from(p(&faint)[c]) - want).abs() <= 3.0, "{:?} against {want}", p(&faint));
            // and over the bridge's car, the same over that car
            let want = f64::from(p(&opaque)[c]) * a + f64::from(p(&alone)[c]) * (1.0 - a);
            assert!((f64::from(p(&over)[c]) - want).abs() <= 3.0, "{:?} against {want} over {:?}", p(&over), p(&alone));
        }
    }

    /// An X 2000 of five cars: its own cars at their lengths end to end (17.7, 25 × 3, 22.7 m), in
    /// its livery, the train's colour not taken.
    #[test]
    fn a_models_cars_wear_its_livery_at_their_lengths() {
        let Some(gpu) = gpu() else { return };
        let head = [600.0, 120.0];
        let mut t = train(&VIEW, head, 90.0, 5);
        t.model = Some("x2000".into());
        let cars = layout(&VIEW, &[t]);
        let keys: Vec<String> = cars.cars.iter().map(|c| c.mesh.key()).collect();
        assert_eq!(keys, ["x2000-0", "x2000-1", "x2000-1", "x2000-1", "x2000-2"]);
        let image = shoot(&gpu, VIEW, 1.0, cars.clone());
        save(&image, "t3-cars-x2000.png");
        let drawn = draws(&gpu, VIEW, cars.clone());
        assert_eq!(drawn.iter().map(|(id, n)| (id.as_str(), *n)).collect::<Vec<_>>(), [
            ("train-car-meshes-x2000-0", 1),
            ("train-car-meshes-x2000-1", 3),
            ("train-car-meshes-x2000-2", 1)
        ]);
        let ground = image.pixel(2, 2);
        let y = head[1] as u32;
        let drawn_x: Vec<u32> = (0..image.width).filter(|&x| (0..3).any(|c| image.pixel(x, y)[c].abs_diff(ground[c]) > 12)).collect();
        let (front, _) = ends(&cars.cars[0]);
        let (_, back) = ends(&cars.cars[4]);
        let want = screen(&VIEW, front, 0.0)[0] - screen(&VIEW, back, 0.0)[0];
        let got = f64::from(drawn_x[drawn_x.len() - 1] - drawn_x[0] + 1);
        assert!((got - want).abs() <= 1.5, "{got} px against {want:.2} (115.4 m)");
        // several colours of its own, none of them the regional mint it would otherwise be tinted
        let mut colours: Vec<[u8; 4]> = Vec::new();
        for x in drawn_x {
            let p = image.pixel(x, y);
            if colours.iter().all(|c| (0..3).any(|i| c[i].abs_diff(p[i]) > 40)) {
                colours.push(p);
            }
        }
        assert!(colours.len() >= 3, "{colours:?}");
    }

    /// The car under the pointer is blended toward the selection blue.
    #[test]
    fn the_car_under_the_pointer_is_highlighted() {
        let Some(gpu) = gpu() else { return };
        let mut cars = layout(&VIEW, &[train(&VIEW, [400.0, 120.0], 90.0, 2)]);
        let plain = shoot(&gpu, VIEW, 1.0, cars.clone());
        cars.cars[1].highlighted = true;
        let lit = shoot(&gpu, VIEW, 1.0, cars.clone());
        save(&lit, "t3-car-highlight.png");
        let at = |image: &Image, car: &DrawnCar| {
            let [x, y] = screen(&VIEW, car.at, 4.1);
            image.pixel(x as u32, y as u32)
        };
        let (before, after) = (at(&plain, &cars.cars[1]), at(&lit, &cars.cars[1]));
        assert!(after[2] > before[2] + 10 && after[1] < before[1], "{before:?} to {after:?}");
        assert_eq!(at(&plain, &cars.cars[0]), at(&lit, &cars.cars[0]), "the other car as it was");
    }
}
