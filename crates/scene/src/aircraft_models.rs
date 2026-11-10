//! Port of `models.ts` (doc 03 §4.5): the built-in low-poly aircraft, built procedurally so there
//! are no model files to ship. They are the fallback for types with no published model
//! ([`crate::aircraft_assets`]) and for everything until those load.
//!
//! Units are metres; the nose points along +y, +x is the right wing, +z is up, and the lowest
//! point of each model sits at z ≈ 0 (loosely: the propeller aircraft have no gear, and the
//! turboprop's fuselage is 2 m up). Flat shaded: three vertices per triangle, each triangle
//! wound so its normal faces away from an "inside" point, and each belongs to a livery
//! [`Part`].
//!
//! Transliterated as written: the vertices are worked out in `f64` as the TypeScript's
//! `number[]` and become `f32` when the mesh is built (its `Float32Array`); `Math.hypot` is
//! [`js_hypot`], which sums as the JavaScript engines do.

use std::sync::OnceLock;

use rustradar_core::liveries::{PARTS, Part};

use crate::geom::js_hypot;

type V3 = [f64; 3];

/// One flat-shaded triangle list: `x, y, z` per vertex, three vertices per triangle.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
}

impl Mesh {
    /// How many vertices it has.
    pub fn vertices(&self) -> usize {
        self.positions.len() / 3
    }
}

/// A model as one mesh (for flat, single-colour drawing) and per livery part (`ModelMeshes`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelMeshes {
    /// Every part's triangles, the parts in [`PARTS`] order.
    pub combined: Mesh,
    /// The parts the model has, in [`PARTS`] order.
    pub parts: Vec<(Part, Mesh)>,
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A section of a lofted body: an ellipse of half-width `w` and half-height `h` round
/// `(x, y, z)`, across y.
#[derive(Debug, Clone, Copy)]
struct Section {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    z: f64,
}

/// A section on the centreline (`x` left out in the TypeScript).
const fn sec(y: f64, w: f64, h: f64, z: f64) -> Section {
    Section { x: 0.0, y, w, h, z }
}

/// One part's triangles as the builder collects them, in `f64`.
#[derive(Debug, Default)]
struct Bucket {
    positions: Vec<f64>,
    normals: Vec<f64>,
}

fn part_index(part: Part) -> usize {
    match part {
        Part::Top => 0,
        Part::Belly => 1,
        Part::Wings => 2,
        Part::Tail => 3,
        Part::Engines => 4,
    }
}

struct MeshBuilder {
    /// The livery part new triangles belong to.
    part: Part,
    buckets: [Option<Bucket>; 5],
}

impl MeshBuilder {
    fn new() -> Self {
        MeshBuilder { part: Part::Top, buckets: Default::default() }
    }

    /// Adds a flat-shaded triangle, wound so its normal faces away from `inside`.
    fn tri(&mut self, a: V3, b: V3, c: V3, inside: V3) {
        let (mut b, mut c) = (b, c);
        let mut n = cross(sub(b, a), sub(c, a));
        let len = js_hypot(&n);
        if len < 1e-9 {
            return;
        }
        n = [n[0] / len, n[1] / len, n[2] / len];
        let centroid: V3 = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0];
        if dot(n, sub(centroid, inside)) < 0.0 {
            std::mem::swap(&mut b, &mut c);
            n = [-n[0], -n[1], -n[2]];
        }
        let bucket = self.buckets[part_index(self.part)].get_or_insert_with(Bucket::default);
        for v in [a, b, c] {
            bucket.positions.extend_from_slice(&v);
            bucket.normals.extend_from_slice(&n);
        }
    }

    fn quad(&mut self, a: V3, b: V3, c: V3, d: V3, inside: V3) {
        self.tri(a, b, c, inside);
        self.tri(a, c, d, inside);
    }

    /// A body of elliptical cross-sections lofted along y. With `belly`, the underside goes to
    /// the belly part, for two-tone fuselages.
    fn loft(&mut self, sections: &[Section], sides: usize, belly: bool) {
        let part = self.part;
        let rings: Vec<Vec<V3>> = sections
            .iter()
            .map(|s| {
                (0..sides)
                    .map(|i| {
                        let t = (i as f64 / sides as f64) * std::f64::consts::PI * 2.0;
                        [s.x + s.w * t.cos(), s.y, s.z + s.h * t.sin()]
                    })
                    .collect()
            })
            .collect();
        for k in 0..rings.len().saturating_sub(1) {
            let (s0, s1) = (sections[k], sections[k + 1]);
            let inside: V3 = [(s0.x + s1.x) / 2.0, (s0.y + s1.y) / 2.0, (s0.z + s1.z) / 2.0];
            for i in 0..sides {
                let j = (i + 1) % sides;
                let middle = ((i as f64 + 0.5) / sides as f64) * std::f64::consts::PI * 2.0;
                self.part = if belly && middle.sin() < -0.3 { Part::Belly } else { part };
                self.quad(rings[k][i], rings[k][j], rings[k + 1][j], rings[k + 1][i], inside);
            }
        }
        self.part = part;
        // close blunt ends
        let (Some(first), Some(last)) = (sections.first(), sections.last()) else {
            return;
        };
        for (ring, s, dir) in [(&rings[0], first, -1.0), (&rings[rings.len() - 1], last, 1.0)] {
            if s.w < 1e-6 {
                continue;
            }
            let center: V3 = [s.x, s.y, s.z];
            let inside: V3 = [s.x, s.y - dir, s.z]; // a point inside the body
            for i in 0..sides {
                self.tri(center, ring[i], ring[(i + 1) % sides], inside);
            }
        }
    }

    /// A body of revolution around the vertical axis: `profile` is `[radius, z]` pairs.
    fn lathe(&mut self, profile: &[[f64; 2]], sides: usize, at: V3) {
        let rings: Vec<Vec<V3>> = profile
            .iter()
            .map(|&[r, z]| {
                (0..sides)
                    .map(|i| {
                        let t = (i as f64 / sides as f64) * std::f64::consts::PI * 2.0;
                        [at[0] + r * t.cos(), at[1] + r * t.sin(), at[2] + z]
                    })
                    .collect()
            })
            .collect();
        for k in 0..rings.len().saturating_sub(1) {
            let inside: V3 = [at[0], at[1], at[2] + (profile[k][1] + profile[k + 1][1]) / 2.0];
            for i in 0..sides {
                let j = (i + 1) % sides;
                self.quad(rings[k][i], rings[k][j], rings[k + 1][j], rings[k + 1][i], inside);
            }
        }
    }

    /// A flat convex plate: `outline` is drawn in a local `(u, v)` plane and
    /// `place(u, v, side)` maps it to 3D, with side = ±1 for its two faces.
    fn slab(&mut self, outline: &[[f64; 2]], place: &dyn Fn(f64, f64, f64) -> V3) {
        let top: Vec<V3> = outline.iter().map(|&[u, v]| place(u, v, 1.0)).collect();
        let bottom: Vec<V3> = outline.iter().map(|&[u, v]| place(u, v, -1.0)).collect();
        let count = (top.len() + bottom.len()) as f64;
        let inside = top
            .iter()
            .chain(&bottom)
            .fold([0.0; 3], |m: V3, p| [m[0] + p[0] / count, m[1] + p[1] / count, m[2] + p[2] / count]);
        for i in 1..outline.len().saturating_sub(1) {
            self.tri(top[0], top[i], top[i + 1], inside);
            self.tri(bottom[0], bottom[i], bottom[i + 1], inside);
        }
        for i in 0..outline.len() {
            let j = (i + 1) % outline.len();
            self.quad(top[i], top[j], bottom[j], bottom[i], inside);
        }
    }

    /// An axis-aligned box from its minimum and maximum corners.
    fn r#box(&mut self, [x0, y0, z0]: V3, [x1, y1, z1]: V3) {
        self.slab(&[[x0, y0], [x1, y0], [x1, y1], [x0, y1]], &|u, v, side| [u, v, if side > 0.0 { z1 } else { z0 }]);
    }

    /// The same plate on both sides of the centreline.
    fn mirrored(&mut self, outline: &[[f64; 2]], place: &dyn Fn(f64, f64, f64) -> V3) {
        self.slab(outline, place);
        self.slab(outline, &|u, v, side| {
            let [x, y, z] = place(u, v, side);
            [-x, y, z]
        });
    }

    fn build(self) -> ModelMeshes {
        let mesh = |b: &Bucket| Mesh {
            positions: b.positions.iter().map(|&v| v as f32).collect(),
            normals: b.normals.iter().map(|&v| v as f32).collect(),
        };
        let mut combined = Mesh::default();
        let mut parts = Vec::new();
        for part in PARTS {
            if let Some(bucket) = &self.buckets[part_index(part)] {
                let m = mesh(bucket);
                combined.positions.extend_from_slice(&m.positions);
                combined.normals.extend_from_slice(&m.normals);
                parts.push((part, m));
            }
        }
        ModelMeshes { combined, parts }
    }
}

/// A swept-wing jet airliner with two or four engines under the wings.
fn airliner(l: f64, r: f64, span: f64, engines: u8) -> ModelMeshes {
    let mut m = MeshBuilder::new();
    let cz = r + 0.6;
    m.loft(
        &[
            sec(0.5 * l, 0.0, 0.0, cz - 0.15 * r),
            sec(0.475 * l, 0.45 * r, 0.42 * r, cz - 0.1 * r),
            sec(0.44 * l, 0.82 * r, 0.8 * r, cz - 0.03 * r),
            sec(0.4 * l, r, r, cz),
            sec(-0.29 * l, r, r, cz),
            sec(-0.41 * l, 0.62 * r, 0.68 * r, cz + 0.25 * r),
            sec(-0.49 * l, 0.16 * r, 0.2 * r, cz + 0.6 * r),
            sec(-0.5 * l, 0.0, 0.0, cz + 0.65 * r),
        ],
        12,
        true,
    );

    // wings: swept back about 27 degrees, tapering to a third of the root chord
    m.part = Part::Wings;
    let wing_z = cz - 0.55 * r;
    let half = span / 2.0;
    let root_x = 0.9 * r;
    let root_le = 0.09 * l;
    let root_te = -0.1 * l;
    let sweep = 0.5;
    let le_at = |x: f64| root_le - sweep * (x - root_x);
    let tip_le = le_at(half);
    let tip_te = tip_le - 0.32 * (root_le - root_te);
    let wing_t = 0.12 * r;
    m.mirrored(&[[root_x, root_le], [half, tip_le], [half, tip_te], [root_x, root_te]], &|u, v, side| {
        [u, v, wing_z + (side * wing_t) / 2.0]
    });

    // engines hang slightly ahead of and below the wing
    m.part = Part::Engines;
    let re = 0.48 * r;
    let engine_z = (re + 0.15).max(wing_z - 0.7 * re);
    let stations: &[f64] = if engines == 2 { &[0.34] } else { &[0.3, 0.56] };
    for &f in stations {
        for side_x in [-1.0, 1.0] {
            let x = side_x * (root_x + f * (half - root_x));
            let front = le_at(x.abs()) + 1.3 * re;
            m.loft(
                &[
                    Section { x, y: front, w: 0.9 * re, h: 0.9 * re, z: engine_z },
                    Section { x, y: front - 0.4 * re, w: re, h: re, z: engine_z },
                    Section { x, y: front - 2.6 * re, w: 0.85 * re, h: 0.85 * re, z: engine_z },
                    Section { x, y: front - 3.4 * re, w: 0.45 * re, h: 0.45 * re, z: engine_z },
                ],
                10,
                false,
            );
        }
    }

    // tailplane and fin
    m.part = Part::Wings;
    let stab_z = cz + 0.35 * r;
    let stab_t = 0.1 * r;
    m.mirrored(
        &[[0.5 * r, -0.36 * l], [0.17 * l, -0.455 * l], [0.17 * l, -0.495 * l], [0.5 * r, -0.46 * l]],
        &|u, v, side| [u, v, stab_z + (side * stab_t) / 2.0],
    );
    let fin_base = cz + 0.75 * r;
    let fin_top = fin_base + 0.19 * l;
    m.part = Part::Tail;
    m.slab(
        &[[-0.34 * l, fin_base], [-0.45 * l, fin_top], [-0.5 * l, fin_top], [-0.495 * l, fin_base]],
        &|u, v, side| [(side * 0.13 * r) / 2.0, u, v],
    );
    m.build()
}

/// A straight-winged propeller aircraft: high wing, single nose prop or twin nacelles.
fn prop_plane(l: f64, span: f64, twin: bool, prop: bool) -> ModelMeshes {
    let mut m = MeshBuilder::new();
    let r = 0.075 * l;
    let cz = 0.16 * l;
    m.loft(
        &[
            sec(0.5 * l, 0.25 * r, 0.25 * r, cz - 0.1 * r),
            sec(0.46 * l, 0.8 * r, 0.85 * r, cz - 0.05 * r),
            sec(0.3 * l, r, 1.2 * r, cz + 0.1 * r),
            sec(0.02 * l, r, 1.2 * r, cz + 0.15 * r),
            sec(-0.25 * l, 0.55 * r, 0.65 * r, cz + 0.35 * r),
            sec(-0.5 * l, 0.12 * r, 0.18 * r, cz + 0.6 * r),
        ],
        10,
        true,
    );
    m.part = Part::Wings;
    let wing_z = cz + 1.35 * r;
    let chord = 0.17 * l;
    let wing_t = 0.25 * r;
    m.mirrored(
        &[
            [0.5 * r, 0.2 * l],
            [span / 2.0, 0.19 * l],
            [span / 2.0, 0.19 * l - 0.75 * chord],
            [0.5 * r, 0.2 * l - chord],
        ],
        &|u, v, side| [u, v, wing_z + (side * wing_t) / 2.0],
    );
    let prop_disc = |m: &mut MeshBuilder, x: f64, y: f64, z: f64, r: f64| {
        m.slab(&[[-r, -0.06 * r], [r, -0.06 * r], [r, 0.06 * r], [-r, 0.06 * r]], &|u, v, side| {
            [x + u, y + side * 0.03, z + v]
        })
    };
    m.part = Part::Engines;
    if twin {
        for sx in [-1.0, 1.0] {
            let x = sx * 0.3 * (span / 2.0);
            let nz = wing_z - 0.6 * r;
            m.loft(
                &[
                    Section { x, y: 0.36 * l, w: 0.3 * r, h: 0.3 * r, z: nz },
                    Section { x, y: 0.32 * l, w: 0.6 * r, h: 0.7 * r, z: nz },
                    Section { x, y: 0.05 * l, w: 0.55 * r, h: 0.6 * r, z: nz },
                    Section { x, y: -0.05 * l, w: 0.2 * r, h: 0.2 * r, z: nz },
                ],
                8,
                false,
            );
            if prop {
                prop_disc(&mut m, x, 0.365 * l, nz, 0.17 * span / 2.0);
            }
        }
    } else if prop {
        prop_disc(&mut m, 0.0, 0.505 * l, cz - 0.1 * r, 0.11 * l);
    }
    m.part = Part::Wings;
    m.mirrored(
        &[[0.3 * r, -0.33 * l], [0.2 * l, -0.42 * l], [0.2 * l, -0.49 * l], [0.3 * r, -0.49 * l]],
        &|u, v, side| [u, v, cz + 0.55 * r + side * 0.06 * r],
    );
    m.part = Part::Tail;
    m.slab(
        &[
            [-0.27 * l, cz + 0.6 * r],
            [-0.42 * l, cz + 0.6 * r + 0.17 * l],
            [-0.5 * l, cz + 0.6 * r + 0.17 * l],
            [-0.5 * l, cz + 0.6 * r],
        ],
        &|u, v, side| [side * 0.08 * r, u, v],
    );
    m.build()
}

fn helicopter() -> ModelMeshes {
    let mut m = MeshBuilder::new();
    m.loft(
        &[
            sec(2.6, 0.25, 0.3, 1.3),
            sec(2.1, 0.8, 0.9, 1.4),
            sec(0.5, 1.0, 1.1, 1.5),
            sec(-1.0, 0.8, 0.95, 1.6),
            sec(-1.8, 0.3, 0.35, 1.9),
        ],
        10,
        true,
    );
    m.loft(&[sec(-1.6, 0.22, 0.22, 2.0), sec(-6.0, 0.12, 0.12, 2.2), sec(-6.3, 0.0, 0.0, 2.2)], 6, false);
    m.part = Part::Tail;
    m.slab(&[[-5.6, 2.1], [-6.2, 3.3], [-6.6, 3.3], [-6.4, 2.1]], &|u, v, side| [side * 0.05, u, v]);
    m.part = Part::Engines;
    m.r#box([-0.12, -0.12, 2.5], [0.12, 0.12, 2.95]);
    let blade = |m: &mut MeshBuilder, angle: f64| {
        let (c, s) = (angle.cos(), angle.sin());
        m.slab(&[[-5.5, -0.16], [5.5, -0.16], [5.5, 0.16], [-5.5, 0.16]], &|u, v, side| {
            [u * c - v * s, u * s + v * c, 2.97 + side * 0.03]
        });
    };
    blade(&mut m, std::f64::consts::PI / 4.0);
    blade(&mut m, -std::f64::consts::PI / 4.0);
    for sx in [-1.0, 1.0] {
        m.r#box([sx * 0.9 - 0.06, -1.5, 0.0], [sx * 0.9 + 0.06, 1.8, 0.12]);
    }
    m.build()
}

fn balloon() -> ModelMeshes {
    let mut m = MeshBuilder::new();
    m.part = Part::Tail;
    m.lathe(&[[0.6, 3.2], [3.2, 6.5], [5.6, 10.0], [6.6, 13.5], [6.2, 16.5], [4.4, 19.0], [0.01, 20.2]], 14, [0.0; 3]);
    m.part = Part::Engines;
    m.r#box([-0.7, -0.7, 0.0], [0.7, 0.7, 1.1]);
    m.build()
}

fn ground_vehicle() -> ModelMeshes {
    let mut m = MeshBuilder::new();
    m.r#box([-1.2, -3.0, 0.3], [1.2, 3.0, 1.6]);
    m.r#box([-1.1, 1.2, 1.6], [1.1, 2.8, 2.4]);
    m.build()
}

/// The built-in models (`ModelKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelKind {
    Narrow,
    Wide2,
    Wide4,
    Turboprop,
    Twin,
    Light,
    Glider,
    Heli,
    Balloon,
    Ground,
}

/// Every kind, in the TypeScript's order (`MODEL_KINDS`); `kind as usize` indexes it.
pub const MODEL_KINDS: [ModelKind; 10] = [
    ModelKind::Narrow,
    ModelKind::Wide2,
    ModelKind::Wide4,
    ModelKind::Turboprop,
    ModelKind::Twin,
    ModelKind::Light,
    ModelKind::Glider,
    ModelKind::Heli,
    ModelKind::Balloon,
    ModelKind::Ground,
];

impl ModelKind {
    /// The name in the TypeScript.
    pub fn as_str(self) -> &'static str {
        match self {
            ModelKind::Narrow => "narrow",
            ModelKind::Wide2 => "wide2",
            ModelKind::Wide4 => "wide4",
            ModelKind::Turboprop => "turboprop",
            ModelKind::Twin => "twin",
            ModelKind::Light => "light",
            ModelKind::Glider => "glider",
            ModelKind::Heli => "heli",
            ModelKind::Balloon => "balloon",
            ModelKind::Ground => "ground",
        }
    }
}

/// One kind's meshes, built afresh (the TypeScript's table in `models()`).
pub fn build_model(kind: ModelKind) -> ModelMeshes {
    match kind {
        ModelKind::Narrow => airliner(38.0, 2.0, 35.0, 2),
        ModelKind::Wide2 => airliner(64.0, 3.1, 61.0, 2),
        ModelKind::Wide4 => airliner(70.0, 3.3, 64.0, 4),
        ModelKind::Turboprop => prop_plane(25.0, 27.0, true, true),
        ModelKind::Twin => prop_plane(9.5, 12.0, true, true),
        ModelKind::Light => prop_plane(8.6, 11.0, false, true),
        ModelKind::Glider => prop_plane(6.8, 15.0, false, false),
        ModelKind::Heli => helicopter(),
        ModelKind::Balloon => balloon(),
        ModelKind::Ground => ground_vehicle(),
    }
}

/// Every kind's meshes, built once on first use (`models()`), indexed by `kind as usize`.
pub fn models() -> &'static [ModelMeshes; 10] {
    static CACHE: OnceLock<[ModelMeshes; 10]> = OnceLock::new();
    CACHE.get_or_init(|| MODEL_KINDS.map(build_model))
}

/// One kind's meshes, from [`models`].
pub fn model(kind: ModelKind) -> &'static ModelMeshes {
    &models()[kind as usize]
}

/// Flightradar24's icon class → model and a scale factor within that model (`ICON_MODELS`).
fn icon_model(icon: &str) -> Option<(ModelKind, f64)> {
    use ModelKind::*;
    Some(match icon {
        "B738" => (Narrow, 1.0),
        "A320" => (Narrow, 1.0),
        "B736" => (Narrow, 0.85),
        "B757" => (Narrow, 1.25),
        "RJ85" => (Narrow, 0.75),
        "FOKKER100" => (Narrow, 0.8),
        "LJ60" => (Narrow, 0.45),
        "FGTR" => (Narrow, 0.45),
        "CONC" => (Narrow, 1.5),
        "B767" => (Wide2, 0.85),
        "A330" => (Wide2, 0.95),
        "B777" => (Wide2, 1.05),
        "MD11" => (Wide2, 0.9),
        "A3ST" => (Wide2, 0.85),
        "A343" => (Wide4, 0.95),
        "A346" => (Wide4, 1.05),
        "B747" => (Wide4, 1.0),
        "A380" => (Wide4, 1.1),
        "A225" => (Wide4, 1.2),
        "SAT" => (Wide4, 0.5),
        "ISS" => (Wide4, 1.4),
        "Q300" => (Turboprop, 1.0),
        "C303" => (Twin, 1.0),
        "C206" => (Light, 1.0),
        "SLEI" => (Light, 1.0),
        "ASW20" => (Glider, 1.0),
        "SI2" => (Glider, 1.6),
        "EC" => (Heli, 1.0),
        "DRON" => (Heli, 0.25),
        "BALL" => (Balloon, 1.0),
        "GRND" => (Ground, 1.0),
        _ => return None,
    })
}

/// The model and scale factor for a Flightradar24 icon class; a narrow-body at 1 for any other
/// (`modelFor`).
pub fn model_for(icon: &str) -> (ModelKind, f64) {
    icon_model(icon).unwrap_or((ModelKind::Narrow, 1.0))
}

/// Largest dimension of each model (length, wingspan or rotor), in metres (`MODEL_SIZE`).
pub fn model_size(kind: ModelKind) -> f64 {
    match kind {
        ModelKind::Narrow => 38.0,
        ModelKind::Wide2 => 64.0,
        ModelKind::Wide4 => 70.0,
        ModelKind::Turboprop => 27.0,
        ModelKind::Twin => 12.0,
        ModelKind::Light => 11.0,
        ModelKind::Glider => 15.0,
        ModelKind::Heli => 11.0,
        ModelKind::Balloon => 13.2,
        ModelKind::Ground => 6.0,
    }
}

/// Real-world size (largest dimension, metres) of an aircraft of this icon class (`sizeOf`).
pub fn size_of(icon: &str) -> f64 {
    let (kind, factor) = model_for(icon);
    model_size(kind) * factor
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;

    fn bbox(p: &[f32]) -> ([f64; 3], [f64; 3]) {
        let (mut min, mut max) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for v in p.chunks_exact(3) {
            for k in 0..3 {
                min[k] = min[k].min(v[k] as f64);
                max[k] = max[k].max(v[k] as f64);
            }
        }
        (min, max)
    }

    fn sum(a: &[f32]) -> f64 {
        a.iter().map(|&v| v as f64).sum()
    }

    /// What bun gives for `models()` (`models.ts`): vertices per part, the combined mesh's
    /// bounding box and the sums of its positions and normals.
    #[allow(clippy::type_complexity)]
    const TYPESCRIPT: [(ModelKind, &[(Part, usize)], [f64; 3], [f64; 3], f64, f64); 10] = {
        use ModelKind::*;
        use Part::*;
        [
            (Narrow, &[(Top, 288), (Belly, 144), (Wings, 144), (Tail, 36), (Engines, 480)], [-17.5, -19.0, 0.19698575139045715], [17.5, 19.0, 11.319999694824219], 943.2318049669266, -10.036653896793723),
            (Wide2, &[(Top, 288), (Belly, 144), (Wings, 144), (Tail, 36), (Engines, 480)], [-30.5, -32.0, 0.22282789647579193], [30.5, 32.0, 18.184999465942383], 936.9519729614258, -11.382377810776234),
            (Wide4, &[(Top, 288), (Belly, 144), (Wings, 144), (Tail, 36), (Engines, 960)], [-32.0, -35.0, 0.2275264710187912], [32.0, 35.0, 19.674999237060547], 1040.2479314804077, -43.39367686584592),
            (Turboprop, &[(Top, 210), (Belly, 150), (Wings, 144), (Tail, 36), (Engines, 456)], [-13.5, -12.5, 2.0476229190826416], [13.5, 12.5, 9.375], 7269.625005722046, 58.68794351443648),
            (Twin, &[(Top, 210), (Belly, 150), (Wings, 144), (Tail, 36), (Engines, 456)], [-6.0, -4.75, 0.7780966758728027], [6.0, 4.75, 3.5625], 2762.4574720859528, 58.68151622638106),
            (Light, &[(Top, 210), (Belly, 150), (Wings, 144), (Tail, 36), (Engines, 36)], [-5.5, -4.300000190734863, 0.7043822407722473], [5.5, 4.373000144958496, 3.2249999046325684], 1068.8509849905968, 35.14514759927988),
            (Glider, &[(Top, 210), (Belly, 150), (Wings, 144), (Tail, 36)], [-7.5, -3.4000000953674316, 0.5569534301757812], [7.5, 3.4000000953674316, 2.549999952316284], 684.1819951534271, 35.13440588861704),
            (Heli, &[(Top, 252), (Belly, 120), (Tail, 36), (Engines, 180)], [-4.002224445343018, -6.599999904632568, 0.0], [4.002224445343018, 4.002224445343018, 3.299999952316284], 683.2599910199642, -8.306251748465005),
            (Balloon, &[(Tail, 504), (Engines, 36)], [-6.599999904632568, -6.434524059295654, 0.0], [6.599999904632568, 6.434524059295654, 20.200000762939453], 6504.6000344753265, 19.57576596736908),
            (Ground, &[(Top, 72)], [-1.2000000476837158, -3.0, 0.30000001192092896], [1.2000000476837158, 3.0, 2.4000000953674316], 178.20000278949738, 0.0),
        ]
    };

    #[test]
    fn every_model_matches_the_typescript() {
        for (kind, parts, min, max, positions, normals) in TYPESCRIPT {
            let m = model(kind);
            let got: Vec<(Part, usize)> = m.parts.iter().map(|(p, mesh)| (*p, mesh.vertices())).collect();
            assert_eq!(got, parts, "{kind:?}");
            assert_eq!(m.combined.vertices(), parts.iter().map(|p| p.1).sum::<usize>(), "{kind:?}");
            let (lo, hi) = bbox(&m.combined.positions);
            for k in 0..3 {
                assert_eq!(lo[k], min[k], "{kind:?}");
                assert_eq!(hi[k], max[k], "{kind:?}");
            }
            assert_close!(sum(&m.combined.positions), positions, 6);
            assert_close!(sum(&m.combined.normals), normals, 6);
        }
    }

    #[test]
    fn whole_flat_shaded_triangles_with_unit_normals_facing_out() {
        for kind in MODEL_KINDS {
            let m = model(kind);
            let n = m.combined.vertices();
            assert!(n > 0 && n.is_multiple_of(3), "{kind:?}");
            assert_eq!(m.combined.normals.len(), m.combined.positions.len());
            for (t, (p, nr)) in m.combined.positions.chunks_exact(9).zip(m.combined.normals.chunks_exact(9)).enumerate() {
                // one normal per triangle, of unit length
                assert_eq!(nr[0..3], nr[3..6], "{kind:?} triangle {t}");
                assert_eq!(nr[0..3], nr[6..9], "{kind:?} triangle {t}");
                assert_close!(js_hypot(&[nr[0] as f64, nr[1] as f64, nr[2] as f64]), 1.0, 5);
                // wound counter-clockwise seen from where the normal points
                let v = |i: usize| [p[3 * i] as f64, p[3 * i + 1] as f64, p[3 * i + 2] as f64];
                let c = cross(sub(v(1), v(0)), sub(v(2), v(0)));
                assert!(dot(c, [nr[0] as f64, nr[1] as f64, nr[2] as f64]) > 0.0, "{kind:?} triangle {t}");
            }
        }
    }

    #[test]
    fn the_combined_mesh_is_the_parts_in_livery_order() {
        for kind in MODEL_KINDS {
            let m = model(kind);
            let order: Vec<usize> = m.parts.iter().map(|(p, _)| part_index(*p)).collect();
            assert!(order.windows(2).all(|w| w[0] < w[1]), "{kind:?}");
            let positions: Vec<f32> = m.parts.iter().flat_map(|(_, mesh)| mesh.positions.iter().copied()).collect();
            assert_eq!(positions, m.combined.positions, "{kind:?}");
        }
        // the balloon's envelope takes the tail colour, its basket the engines'
        let balloon: Vec<Part> = model(ModelKind::Balloon).parts.iter().map(|p| p.0).collect();
        assert_eq!(balloon, [Part::Tail, Part::Engines]);
    }

    #[test]
    fn models_sit_on_the_ground_and_span_their_size() {
        for kind in MODEL_KINDS {
            let (lo, hi) = bbox(&model(kind).combined.positions);
            // none of it under the ground, and not far above it: "z ≈ 0" is loose for the
            // propeller aircraft, whose fuselages (the turboprop's 2 m up) have no gear under them
            assert!(lo[2] >= -1e-6 && lo[2] < 0.08 * model_size(kind), "{kind:?} lowest point {}", lo[2]);
            let largest = (hi[0] - lo[0]).max(hi[1] - lo[1]);
            // the size is the length, the span or the rotor: the helicopter's two blades, crossed
            // at 45°, reach 10.6 m of its 11 m rotor along y
            assert!(largest <= model_size(kind) * 1.0001, "{kind:?} {largest}");
            assert!(largest >= model_size(kind) * 0.96, "{kind:?} {largest}");
        }
    }

    #[test]
    fn icon_classes_pick_a_model_and_a_size() {
        assert_eq!(model_for("B738"), (ModelKind::Narrow, 1.0));
        assert_eq!(model_for("A380"), (ModelKind::Wide4, 1.1));
        assert_eq!(model_for("DRON"), (ModelKind::Heli, 0.25));
        assert_eq!(model_for("GRND"), (ModelKind::Ground, 1.0));
        // anything else is a narrow-body at full size
        assert_eq!(model_for("XXXX"), (ModelKind::Narrow, 1.0));
        assert_eq!(model_for(""), (ModelKind::Narrow, 1.0));
        assert_eq!(model_for("b738"), (ModelKind::Narrow, 1.0));
        assert_close!(size_of("B757"), 47.5, 9);
        assert_close!(size_of("A225"), 84.0, 9);
        assert_close!(size_of("SI2"), 24.0, 9);
        assert_close!(size_of("BALL"), 13.2, 9);
        assert_close!(size_of("unknown"), 38.0, 9);
    }
}
