//! Port of `train-models.ts` (doc 02 §5.6): trains close in, each car a 3D body.
//!
//! A plain car is a unit box along +y (the way it runs) scaled to the car by the mesh layer:
//! underframe, body, dark bands of windows and a lighter roof; the leading car with a sloped,
//! glazed nose. Its shape follows the vehicles the train runs with ([`TrainShape`]). The vertex
//! colours are grey shades multiplied by the train's colour, so the body (1) takes it and the
//! windows and underframe stay dark.
//!
//! Operator models (`static/trains`, built by `scripts/trains/build.mjs`) replace the plain cars
//! once loaded: the app reads [`INDEX_FILE`] and [`MESHES_FILE`] and hands both to
//! [`TrainModels::parse`]; until then, or on an `Err`, the plain cars are drawn. Each model
//! car is unindexed and flat shaded, in the plain cars' unit box, its normals already divided by
//! the draw scale `[3.1, length, 4.1]`.

use std::collections::HashMap;
use std::sync::OnceLock;

use rustradar_core::model::TrainShape;
use serde::Deserialize;

use crate::aircraft_assets::{MeshFileError, f32s, i8_normals, u8_colors};
use crate::geom::js_hypot;

/// A car's size as drawn, metres.
pub const CAR_WIDTH_M: f64 = 3.1;
pub const CAR_HEIGHT_M: f64 = 4.1;

/// Where the operator models are, under the static assets.
pub const INDEX_FILE: &str = "trains/index.json";
pub const MESHES_FILE: &str = "trains/meshes.bin";

/// A car's triangles as `carArrays` makes them (plain `number[]`s): `x, y, z` per vertex, three
/// vertices per triangle, a grey shade per vertex (`s, s, s`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CarArrays {
    pub positions: Vec<f64>,
    pub normals: Vec<f64>,
    pub colors: Vec<f64>,
}

/// A car's triangles ready for the GPU (the `Float32Array`s of its `Geometry`): unindexed, three
/// vertices per triangle, RGB 0..1 per vertex.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CarMesh {
    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
    pub colors: Vec<f32>,
}

impl CarMesh {
    pub fn vertices(&self) -> usize {
        self.positions.len() / 3
    }
}

impl From<&CarArrays> for CarMesh {
    fn from(a: &CarArrays) -> Self {
        let f32s = |v: &[f64]| v.iter().map(|&x| x as f32).collect();
        CarMesh { positions: f32s(&a.positions), normals: f32s(&a.normals), colors: f32s(&a.colors) }
    }
}

/// The grey shades of a box's faces: its sides, ends (default the sides') and top (default the
/// sides'); the bottom is always 0.2.
#[derive(Debug, Clone, Copy)]
struct Shade {
    side: f64,
    end: Option<f64>,
    top: Option<f64>,
}

const fn side(side: f64) -> Shade {
    Shade { side, end: None, top: None }
}

fn face(a: &mut CarArrays, corners: [[f64; 3]; 4], normal: [f64; 3], s: f64) {
    for i in [0, 1, 2, 0, 2, 3] {
        a.positions.extend_from_slice(&corners[i]);
        a.normals.extend_from_slice(&normal);
        a.colors.extend_from_slice(&[s, s, s]);
    }
}

/// An axis-aligned box `[x0, x1] × [y0, y1] × [z0, z1]`, each face a shade (sides, ends, top and
/// bottom).
fn car_box(a: &mut CarArrays, [x0, x1]: [f64; 2], [y0, y1]: [f64; 2], [z0, z1]: [f64; 2], shade: Shade) {
    let end = shade.end.unwrap_or(shade.side);
    let top = shade.top.unwrap_or(shade.side);
    face(a, [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]], [1.0, 0.0, 0.0], shade.side);
    face(a, [[x0, y1, z0], [x0, y0, z0], [x0, y0, z1], [x0, y1, z1]], [-1.0, 0.0, 0.0], shade.side);
    // (the TypeScript writes this face's corners the other way round and reverses them)
    face(a, [[x0, y1, z1], [x1, y1, z1], [x1, y1, z0], [x0, y1, z0]], [0.0, 1.0, 0.0], end);
    face(a, [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]], [0.0, -1.0, 0.0], end);
    face(a, [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]], [0.0, 0.0, 1.0], top);
    face(a, [[x0, y1, z0], [x1, y1, z0], [x1, y0, z0], [x0, y0, z0]], [0.0, 0.0, -1.0], 0.2);
}

/// Per shape (`SHAPES`): where the leading car's nose starts along it (0.5 at the front: none),
/// and its rows of windows as heights `[z0, z1]` (a locomotive's cab has none along its sides).
fn shape_of(shape: TrainShape) -> (f64, &'static [[f64; 2]]) {
    match shape {
        TrainShape::HighSpeed => (0.3, &[[0.5, 0.72]]),
        TrainShape::DoubleDeck => (0.44, &[[0.3, 0.46], [0.6, 0.76]]),
        TrainShape::Regional => (0.42, &[[0.5, 0.72]]),
        TrainShape::Loco => (0.47, &[]),
        TrainShape::Railcar => (0.47, &[[0.5, 0.72]]),
    }
}

/// The shape's name in the TypeScript and on the wire.
fn shape_name(shape: TrainShape) -> &'static str {
    match shape {
        TrainShape::HighSpeed => "highSpeed",
        TrainShape::DoubleDeck => "doubleDeck",
        TrainShape::Regional => "regional",
        TrainShape::Loco => "loco",
        TrainShape::Railcar => "railcar",
    }
}

/// The shape a train's coaches take: a double-decker's have two rows of windows, every other
/// train's one (`coachShape`).
fn coach_shape(shape: TrainShape) -> TrainShape {
    if shape == TrainShape::DoubleDeck { TrainShape::DoubleDeck } else { TrainShape::Regional }
}

/// A car's triangles, unit sized (x across, y along with the front at +0.5, z up from 0 to 1)
/// (`carArrays`; the TypeScript's default shape is [`TrainShape::Regional`]).
pub fn car_arrays(lead: bool, shape: TrainShape) -> CarArrays {
    let mut a = CarArrays::default();
    let (nose, windows) = shape_of(if lead { shape } else { coach_shape(shape) });
    let front = if lead { nose } else { 0.5 };
    // bogies and underframe, set in a little
    car_box(&mut a, [-0.4, 0.4], [-0.47, 0.47], [0.0, 0.2], side(0.22));
    // the body in bands up to the roof line: body colour between the rows of windows
    let mut z = 0.2;
    for &[z0, z1] in windows.iter().chain(&[[0.88, 0.88]]) {
        if z0 > z {
            car_box(&mut a, [-0.5, 0.5], [-0.5, front], [z, z0], Shade { side: 1.0, end: Some(0.85), top: Some(0.9) });
        }
        if z1 > z0 {
            car_box(&mut a, [-0.5, 0.5], [-0.5, front], [z0, z1], Shade { side: 0.16, end: Some(0.3), top: None });
        }
        z = z1;
    }
    // the roof, a little narrower and lighter
    car_box(&mut a, [-0.42, 0.42], [-0.48, front - 0.02], [0.88, 1.0], Shade { side: 0.8, end: None, top: Some(0.75) });
    if lead {
        // the nose: down from the roof line to the buffer beam, glazed above
        let (y0, y1) = (front, 0.5);
        let y_at = |z: f64| y1 - (y1 - y0) * ((z - 0.2) / 0.68);
        let n = [0.0, 0.68, y1 - y0];
        let len = js_hypot(&[n[1], n[2]]);
        let mut slope = |z0: f64, z1: f64, s: f64| {
            let corners = [[-0.5, y_at(z0), z0], [0.5, y_at(z0), z0], [0.5, y_at(z1), z1], [-0.5, y_at(z1), z1]];
            face(&mut a, corners, [0.0, n[1] / len, n[2] / len], s);
        };
        slope(0.2, 0.5, 1.0);
        slope(0.5, 0.8, 0.12);
        slope(0.8, 0.88, 1.0);
        // its sides, under the slope: closed down to the buffer beam
        for x in [-0.5_f64, 0.5] {
            let mut corners = [[x, y0, 0.2], [x, y1, 0.2], [x, y0, 0.88]];
            if x < 0.0 {
                corners.reverse();
            }
            for c in corners {
                a.positions.extend_from_slice(&c);
                // `Math.sign(x)`: x is never 0 here
                a.normals.extend_from_slice(&[x.signum(), 0.0, 0.0]);
                a.colors.extend_from_slice(&[1.0, 1.0, 1.0]);
            }
        }
    }
    a
}

/// The plain cars' meshes by key, built once on first use (the TypeScript's `meshes` map).
fn plain_meshes() -> &'static [(String, CarMesh)] {
    static MESHES: OnceLock<Vec<(String, CarMesh)>> = OnceLock::new();
    MESHES.get_or_init(|| {
        let shapes = [TrainShape::HighSpeed, TrainShape::DoubleDeck, TrainShape::Regional, TrainShape::Loco, TrainShape::Railcar];
        let cabs = shapes.map(|shape| (car_key(true, shape), CarMesh::from(&car_arrays(true, shape))));
        let coaches = [TrainShape::Regional, TrainShape::DoubleDeck]
            .map(|shape| (car_key(false, shape), CarMesh::from(&car_arrays(false, shape))));
        cabs.into_iter().chain(coaches).collect()
    })
}

/// The mesh key of a train's lead (`"<shape>-cab"`) or following car (`"<coach shape>-coach"`).
fn car_key(lead: bool, shape: TrainShape) -> String {
    if lead { format!("{}-cab", shape_name(shape)) } else { format!("{}-coach", shape_name(coach_shape(shape))) }
}

/// A train's lead and following cars as meshes, by the shape each takes; the key names the mesh,
/// for layers to share it (`carMesh`; the TypeScript's default shape is
/// [`TrainShape::Regional`]).
pub fn car_mesh(lead: bool, shape: TrainShape) -> (&'static str, &'static CarMesh) {
    let key = car_key(lead, shape);
    let (key, mesh) = plain_meshes().iter().find(|(k, _)| *k == key).expect("every shape has a cab and a coach");
    (key.as_str(), mesh)
}

/// A model's car (`ModelCar`): in the unit box like the plain cars, and its length in metres.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelCar {
    /// `"<model>-<index>"`, for layers to share the mesh.
    pub key: String,
    pub mesh: CarMesh,
    pub length: f64,
}

/// One car in `index.json` (`PackedCar`): its length in metres, its vertex count and its
/// sections' byte offsets into `meshes.bin`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct PackedCar {
    pub length: f64,
    pub vertices: usize,
    pub positions: usize,
    pub normals: usize,
    pub colors: usize,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct PackedModel {
    cars: Vec<PackedCar>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct Index {
    models: HashMap<String, PackedModel>,
}

/// The trains' 3D models once loaded (`trainModels` after `loadTrainModels()`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrainModels {
    models: HashMap<String, Vec<ModelCar>>,
}

impl TrainModels {
    /// Both files read. Any section out of bounds or misaligned fails the whole load (the
    /// TypeScript kept the models read before the one that threw).
    pub fn parse(index_json: &[u8], meshes_bin: &[u8]) -> Result<TrainModels, MeshFileError> {
        let index: Index = serde_json::from_slice(index_json)?;
        let mut models = HashMap::with_capacity(index.models.len());
        for (name, PackedModel { cars }) in index.models {
            let cars = cars
                .iter()
                .enumerate()
                .map(|(i, car)| {
                    let count = car.vertices.saturating_mul(3);
                    // normals and colours ship as bytes; the GPU wants floats
                    let mesh = CarMesh {
                        positions: f32s(meshes_bin, car.positions, count)?,
                        normals: i8_normals(meshes_bin, car.normals, count)?,
                        colors: u8_colors(meshes_bin, car.colors, count)?,
                    };
                    Ok(ModelCar { key: format!("{name}-{i}"), mesh, length: car.length })
                })
                .collect::<Result<Vec<_>, MeshFileError>>()?;
            models.insert(name, cars);
        }
        Ok(TrainModels { models })
    }

    /// A model's cars, lead, middle and last.
    pub fn cars(&self, model: &str) -> Option<&[ModelCar]> {
        self.models.get(model).map(Vec::as_slice)
    }

    /// The models' names, in no particular order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.models.keys().map(String::as_str)
    }

    /// Car `k` of a train of `count` cars drawn with `model`: its leading car first, its last
    /// car last and the middle one between; `None` without the model (not loaded, or none for
    /// the train) (`modelCar`).
    pub fn model_car(&self, model: Option<&str>, k: usize, count: usize) -> Option<&ModelCar> {
        let cars = self.models.get(model?)?;
        cars.get(if k == 0 {
            0
        } else if k + 1 == count {
            2
        } else {
            1
        })
    }
}

/// A car laid on the map (`CarPlace`): its middle `[lon, lat]`, the way it points (degrees
/// clockwise from north), and its length in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarPlace {
    pub at: [f64; 2],
    pub heading: f64,
    pub length: f64,
}

/// Where each car of `lines` (`[lon, lat]` lines, back to front, as `core::train_cars::car_lines`
/// makes them) lies: halfway between its ends, pointing from its back to its front
/// (`carPlaces`). A car is the chord between its ends; one under half a metre is dropped.
pub fn car_places(lines: &[Vec<[f64; 2]>]) -> Vec<CarPlace> {
    lines
        .iter()
        .filter_map(|line| {
            let (&back, &front) = (line.first()?, line.last()?);
            if line.len() < 2 {
                return None;
            }
            let k = ((((back[1] + front[1]) / 2.0) * std::f64::consts::PI) / 180.0).cos();
            let (east, north) = ((front[0] - back[0]) * k * 111_320.0, (front[1] - back[1]) * 111_320.0);
            let length = js_hypot(&[east, north]);
            if length < 0.5 {
                return None;
            }
            let heading = ((east.atan2(north) * 180.0) / std::f64::consts::PI + 360.0) % 360.0;
            Some(CarPlace { at: [(back[0] + front[0]) / 2.0, (back[1] + front[1]) / 2.0], heading, length })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;

    const SHAPES: [TrainShape; 5] =
        [TrainShape::HighSpeed, TrainShape::DoubleDeck, TrainShape::Regional, TrainShape::Loco, TrainShape::Railcar];

    fn min(v: impl Iterator<Item = f64>) -> f64 {
        v.fold(f64::INFINITY, f64::min)
    }

    fn max(v: impl Iterator<Item = f64>) -> f64 {
        v.fold(f64::NEG_INFINITY, f64::max)
    }

    fn axis(p: &[f64], k: usize) -> impl Iterator<Item = f64> + '_ {
        p.iter().skip(k).step_by(3).copied()
    }

    #[test]
    fn a_car_is_a_closed_unit_box_of_triangles_its_body_taking_the_trains_colour() {
        for (lead, shape) in SHAPES.iter().flat_map(|&shape| [false, true].map(|lead| (lead, shape))) {
            let a = car_arrays(lead, shape);
            assert_eq!(a.positions.len() % 9, 0);
            assert_eq!(a.normals.len(), a.positions.len());
            assert_eq!(a.colors.len(), a.positions.len());
            assert_eq!(min(axis(&a.positions, 0)), -0.5);
            assert_eq!(max(axis(&a.positions, 0)), 0.5);
            assert_eq!(max(axis(&a.positions, 1)), 0.5);
            assert_eq!(min(axis(&a.positions, 2)), 0.0);
            assert_eq!(max(axis(&a.positions, 2)), 1.0);
            // some of it full colour (the body), some dark (windows, underframe)
            assert!(a.colors.contains(&1.0));
            assert!(a.colors.iter().any(|&c| c < 0.25));
        }
    }

    #[test]
    fn the_shapes_differ_where_their_vehicles_do() {
        // where the roof ends: further back for a longer nose
        let roof_end = |shape| {
            let p = car_arrays(true, shape).positions;
            max((0..p.len()).filter(|&i| i % 3 == 1 && p[i + 1] == 1.0).map(|i| p[i]))
        };
        assert!(roof_end(TrainShape::HighSpeed) < roof_end(TrainShape::Regional));
        assert!(roof_end(TrainShape::Regional) < roof_end(TrainShape::Loco));
        // window bands: two rows on a double-decker's coaches, none along a locomotive
        let dark = |lead, shape| car_arrays(lead, shape).colors.iter().filter(|&&c| c == 0.16).count();
        assert_eq!(dark(false, TrainShape::DoubleDeck), 2 * dark(false, TrainShape::Regional));
        assert_eq!(dark(true, TrainShape::Loco), 0);
        assert_eq!(dark(false, TrainShape::Loco), dark(false, TrainShape::Regional));
    }

    #[test]
    fn each_car_lies_halfway_along_it_pointing_from_its_back_to_its_front() {
        // 24 m east along 60°N, then one north
        let east = vec![[18.0, 60.0], [18.0 + 24.0 / 55_660.0, 60.0]];
        let north = vec![[18.0, 60.0], [18.0, 60.0 + 24.0 / 111_320.0]];
        let places = car_places(&[east, north]);
        let (a, b) = (places[0], places[1]);
        assert_close!(a.heading, 90, 0);
        assert_close!(a.length, 24, 0);
        assert_close!(a.at[0], 18.0 + 12.0 / 55_660.0, 6);
        assert_close!(b.heading, 0, 0);
        assert_eq!(car_places(&[vec![[18.0, 60.0]]]), []);
    }

    /// What bun gives for `carArrays(lead, shape)`: vertices and the sums of the positions,
    /// normals and colours.
    const TYPESCRIPT: [(TrainShape, bool, usize, f64, f64, f64); 10] = {
        use TrainShape::*;
        [
            (HighSpeed, false, 180, 100.79999999999991, 0.0, 293.93999999999977),
            (HighSpeed, true, 204, 109.0117647058822, 22.347572860130764, 350.09999999999985),
            (DoubleDeck, false, 252, 133.19999999999996, 0.0, 403.38),
            (DoubleDeck, true, 276, 151.42352941176443, 19.51242588579129, 459.5400000000001),
            (Regional, false, 180, 100.79999999999991, 0.0, 293.93999999999977),
            (Regional, true, 204, 119.44470588235293, 19.979853334024913, 350.09999999999985),
            (Loco, false, 180, 100.79999999999991, 0.0, 293.93999999999977),
            (Loco, true, 132, 80.95176470588238, 18.77585418015006, 240.66000000000037),
            (Railcar, false, 180, 100.79999999999991, 0.0, 293.93999999999977),
            (Railcar, true, 204, 123.79176470588216, 18.77585418015006, 350.09999999999985),
        ]
    };

    #[test]
    fn every_car_matches_the_typescript() {
        for (shape, lead, vertices, positions, normals, colors) in TYPESCRIPT {
            let a = car_arrays(lead, shape);
            assert_eq!(a.positions.len() / 3, vertices, "{shape:?} {lead}");
            // summed in the same order as bun's loop
            let sum = |v: &[f64]| v.iter().fold(0.0, |s, &x| s + x);
            assert_close!(sum(&a.positions), positions, 9);
            assert_close!(sum(&a.normals), normals, 9);
            assert_close!(sum(&a.colors), colors, 9);
        }
    }

    #[test]
    fn plain_cars_share_a_mesh_per_cab_and_coach() {
        assert_eq!(car_mesh(true, TrainShape::HighSpeed).0, "highSpeed-cab");
        assert_eq!(car_mesh(true, TrainShape::Loco).0, "loco-cab");
        assert_eq!(car_mesh(false, TrainShape::DoubleDeck).0, "doubleDeck-coach");
        // every other train's coaches are the regional ones
        for shape in [TrainShape::HighSpeed, TrainShape::Regional, TrainShape::Loco, TrainShape::Railcar] {
            let (key, mesh) = car_mesh(false, shape);
            assert_eq!(key, "regional-coach");
            assert!(std::ptr::eq(mesh, car_mesh(false, TrainShape::Regional).1));
        }
        let (_, cab) = car_mesh(true, TrainShape::Regional);
        assert_eq!(*cab, CarMesh::from(&car_arrays(true, TrainShape::Regional)));
        assert_eq!(cab.vertices(), 204);
    }

    fn read(name: &str) -> Vec<u8> {
        let path = format!("{}/../../static/trains/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    fn shipped() -> TrainModels {
        TrainModels::parse(&read("index.json"), &read("meshes.bin")).unwrap()
    }

    /// Doc 02 §5.6: each model's cars, lead, middle and last, as metres and vertices.
    const MODELS: [(&str, [(f64, usize); 3]); 8] = [
        ("contessa", [(26.2, 4629), (26.6, 4602), (26.3, 4602)]),
        ("x40", [(27.2, 4416), (26.4, 4506), (27.8, 4710)]),
        ("x60", [(20.35, 4314), (16.4, 4548), (20.75, 4404)]),
        ("x61-skane", [(20.0, 4569), (17.0, 4509), (20.7, 4677)]),
        ("x61-vasttrafik", [(20.0, 4557), (17.0, 4287), (20.7, 4287)]),
        ("sj-intercity", [(15.1, 4350), (26.7, 4278), (26.7, 4461)]),
        ("x55", [(26.6, 4392), (26.6, 4698), (27.2, 4488)]),
        ("x2000", [(17.7, 4425), (25.0, 4329), (22.7, 4638)]),
    ];

    #[test]
    fn the_shipped_models_have_three_cars_each() {
        assert_eq!(read("meshes.bin").len(), 1_938_232);
        let models = shipped();
        let mut names: Vec<&str> = models.names().collect();
        names.sort();
        let mut expected: Vec<&str> = MODELS.iter().map(|m| m.0).collect();
        expected.sort();
        assert_eq!(names, expected);
        for (name, cars) in MODELS {
            let got = models.cars(name).unwrap();
            let lengths: Vec<(f64, usize)> = got.iter().map(|c| (c.length, c.mesh.vertices())).collect();
            assert_eq!(lengths, cars, "{name}");
            for (i, car) in got.iter().enumerate() {
                assert_eq!(car.key, format!("{name}-{i}"));
                assert_eq!(car.mesh.vertices() % 3, 0, "{}", car.key);
                assert_eq!(car.mesh.normals.len(), car.mesh.positions.len());
                assert_eq!(car.mesh.colors.len(), car.mesh.positions.len());
                assert!(car.mesh.colors.iter().all(|c| (0.0..=1.0).contains(c)), "{}", car.key);
                assert!(car.mesh.normals.iter().all(|n| (-1.0..=1.0).contains(n)), "{}", car.key);
            }
        }
    }

    #[test]
    fn model_cars_fill_the_plain_cars_unit_box() {
        let models = shipped();
        for (name, _) in MODELS {
            for car in models.cars(name).unwrap() {
                let p: Vec<f64> = car.mesh.positions.iter().map(|&v| v as f64).collect();
                let (x0, x1) = (min(axis(&p, 0)), max(axis(&p, 0)));
                let (y0, y1) = (min(axis(&p, 1)), max(axis(&p, 1)));
                let (z0, z1) = (min(axis(&p, 2)), max(axis(&p, 2)));
                // across: the body's width, give or take mirrors and steps
                assert!(x1 - x0 > 0.95 && x1 - x0 < 1.12, "{}: across {x0}..{x1}", car.key);
                assert!((x0 + x1).abs() < 0.02, "{}", car.key);
                // along: its own length, couplers standing a little proud
                assert!(y1 - y0 > 0.98 && y1 - y0 < 1.12, "{}: along {y0}..{y1}", car.key);
                assert!(y0 > -0.56 && y1 < 0.56, "{}", car.key);
                // up: from the wheels above the rail to the roof, or a raised pantograph
                assert!(z0 > 0.0 && z0 < 0.1, "{}: from {z0}", car.key);
                assert!(z1 > 0.95 && z1 < 1.36, "{}: to {z1}", car.key);
            }
        }
        // the Contessa's leading car as `scripts/trains/build.mjs` wrote it
        let lead = &models.cars("contessa").unwrap()[0];
        let p: Vec<f64> = lead.mesh.positions.iter().map(|&v| v as f64).collect();
        assert_close!(max(axis(&p, 0)), 0.4784, 4);
        assert_close!(min(axis(&p, 1)), -0.4994, 4);
        assert_close!(max(axis(&p, 2)), 1.0278, 4);
    }

    #[test]
    fn trains_take_the_lead_middle_and_last_car_of_their_model() {
        let models = shipped();
        let key = |model: Option<&str>, k, count| models.model_car(model, k, count).map(|c| c.key.as_str());
        assert_eq!(key(Some("x2000"), 0, 6), Some("x2000-0"));
        assert_eq!(key(Some("x2000"), 1, 6), Some("x2000-1"));
        assert_eq!(key(Some("x2000"), 4, 6), Some("x2000-1"));
        assert_eq!(key(Some("x2000"), 5, 6), Some("x2000-2"));
        // a one-car train is its lead car
        assert_eq!(key(Some("x2000"), 0, 1), Some("x2000-0"));
        assert_eq!(key(Some("x40"), 1, 2), Some("x40-2"));
        assert_eq!(key(Some("nope"), 0, 3), None);
        assert_eq!(key(None, 0, 3), None);
        assert_eq!(TrainModels::default().model_car(Some("x2000"), 0, 3), None);
    }

    #[test]
    fn broken_train_files_fail_the_load() {
        let index = |positions: usize| {
            format!(r#"{{"models":{{"m":{{"cars":[{{"length":20,"vertices":3,"positions":{positions},"normals":36,"colors":48}}]}}}}}}"#)
        };
        let mut bin = vec![0u8; 60];
        bin[36..45].copy_from_slice(&[127, 0, 0x81, 0, 0, 0, 0, 0, 0]);
        bin[48..57].copy_from_slice(&[255, 0, 51, 0, 0, 0, 0, 0, 0]);
        let ok = TrainModels::parse(index(0).as_bytes(), &bin).unwrap();
        let car = &ok.cars("m").unwrap()[0];
        assert_eq!(car.mesh.normals[..3], [1.0, 0.0, -1.0]);
        assert_eq!(car.mesh.colors[..3], [1.0, 0.0, 0.2]);
        assert!(matches!(TrainModels::parse(index(1).as_bytes(), &bin), Err(MeshFileError::Misaligned { .. })));
        assert!(matches!(TrainModels::parse(index(28).as_bytes(), &bin), Err(MeshFileError::OutOfBounds { .. })));
        assert!(matches!(TrainModels::parse(b"[]", &bin), Err(MeshFileError::Index(_))));
    }

    #[test]
    fn short_cars_and_lines_are_dropped() {
        // under half a metre long
        assert_eq!(car_places(&[vec![[18.0, 60.0], [18.0, 60.0 + 0.4 / 111_320.0]]]), []);
        assert_eq!(car_places(&[vec![]]), []);
        // only the ends count: the chord between them, pointing west
        let bent = vec![[18.001, 60.0], [18.0005, 60.0001], [18.0, 60.0]];
        let [place] = car_places(&[bent])[..] else { panic!() };
        assert_close!(place.heading, 270, 9);
        assert_close!(place.at[0], 18.0005, 12);
        assert_eq!(place.at[1], 60.0);
        assert_close!(place.length, 0.001 * (60.0_f64 * std::f64::consts::PI / 180.0).cos() * 111_320.0, 6);
    }
}
