//! Port of `ship-models.ts` (doc 02 §4.6): low-poly ship meshes, built procedurally like the
//! aircraft ([`crate::aircraft_models`]), and how each ship is sized and placed from its AIS
//! report.
//!
//! Every model is one unit long: the bow points along +y and y runs from -0.5 (stern) to 0.5
//! (bow), +x is starboard, +z is up and z = 0 is the waterline. The beam is the class's typical
//! share of the length ([`ShipModelArrays::beam`]) and is stretched per ship to its real beam;
//! heights are in the same units.
//!
//! Transliterated as written: vertices are worked out in `f64` and become `f32` when a model is
//! built (its `Float32Array`s), `Math.hypot` is [`js_hypot`], and the
//! container stacks come from the same 32-bit LCG.

use std::sync::OnceLock;

use rustradar_core::model::Ship;

use crate::geom::js_hypot;

type V3 = [f64; 3];
/// A colour, 0..255 per channel; shaded paints have fractions.
type Rgb = [f64; 3];

/// The models ships are drawn with (`ShipClass`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShipClass {
    Container,
    Tanker,
    Bulk,
    Cargo,
    Passenger,
    Tug,
    Fishing,
    Pleasure,
    Sar,
    Generic,
}

/// Every class, in the TypeScript's order (`SHIP_CLASSES`); `cls as usize` indexes it.
pub const SHIP_CLASSES: [ShipClass; 10] = [
    ShipClass::Container,
    ShipClass::Tanker,
    ShipClass::Bulk,
    ShipClass::Cargo,
    ShipClass::Passenger,
    ShipClass::Tug,
    ShipClass::Fishing,
    ShipClass::Pleasure,
    ShipClass::Sar,
    ShipClass::Generic,
];

impl ShipClass {
    /// The name in the TypeScript (and in the layer ids, `ship-mesh-<class>`).
    pub fn as_str(self) -> &'static str {
        match self {
            ShipClass::Container => "container",
            ShipClass::Tanker => "tanker",
            ShipClass::Bulk => "bulk",
            ShipClass::Cargo => "cargo",
            ShipClass::Passenger => "passenger",
            ShipClass::Tug => "tug",
            ShipClass::Fishing => "fishing",
            ShipClass::Pleasure => "pleasure",
            ShipClass::Sar => "sar",
            ShipClass::Generic => "generic",
        }
    }
}

/// One model's triangles, flat shaded: three vertices per triangle (`ShipModelArrays`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShipModelArrays {
    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
    /// Per vertex, 0..1.
    pub colors: Vec<f32>,
    /// Beam as a share of the length, the model's full width.
    pub beam: f64,
    /// Height of the superstructure's top above the waterline (masts not counted).
    pub top: f64,
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

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn shade(c: Rgb, k: f64) -> Rgb {
    [(c[0] * k).min(255.0), (c[1] * k).min(255.0), (c[2] * k).min(255.0)]
}

// shared paints
const WHITE: Rgb = [236.0, 238.0, 240.0];
const OFF_WHITE: Rgb = [214.0, 216.0, 212.0];
const WINDOW: Rgb = [34.0, 44.0, 62.0];
const ANTIFOULING: Rgb = [112.0, 34.0, 30.0];
const BOOT_TOP: Rgb = [26.0, 26.0, 30.0];
const STEEL_DECK: Rgb = [104.0, 108.0, 106.0];
const RUST_DECK: Rgb = [128.0, 70.0, 56.0];
const TEAK: Rgb = [176.0, 138.0, 96.0];
const FUNNEL: Rgb = [44.0, 46.0, 52.0];
const CRANE: Rgb = [218.0, 176.0, 60.0];
const MAST: Rgb = [200.0, 202.0, 204.0];
const ORANGE: Rgb = [236.0, 112.0, 36.0];
const CONTAINERS: [Rgb; 9] = [
    [178.0, 58.0, 48.0],
    [46.0, 92.0, 150.0],
    [64.0, 130.0, 84.0],
    [214.0, 140.0, 52.0],
    [150.0, 150.0, 146.0],
    [222.0, 222.0, 214.0],
    [92.0, 70.0, 120.0],
    [40.0, 120.0, 150.0],
    [190.0, 160.0, 60.0],
];

/// VesselFinder's type colours (`ships.ts` `shipColor`, `core::ships`): unlisted types take the
/// unknown's grey.
fn ship_color(kind: u8) -> Rgb {
    rustradar_core::ships::ship_color(kind).map(f64::from)
}

/// The type colour, a touch darker, for the hull's topsides.
fn topsides_of(kind: u8) -> Rgb {
    shade(ship_color(kind), 0.82)
}

/// The hull's shape and paints (`HullSpec`); `None` takes the TypeScript's default.
#[derive(Debug, Clone, Copy)]
struct HullSpec {
    beam: f64,
    freeboard: f64,
    draft: f64,
    sheer_bow: Option<f64>,
    sheer_stern: Option<f64>,
    /// Share of the length over which the bow narrows to its point.
    bow_length: Option<f64>,
    /// The transom's width as a share of the beam.
    transom: Option<f64>,
    rake: Option<f64>,
    overhang: Option<f64>,
    topsides: Rgb,
    deck: Rgb,
    /// The keel's width as a share of the beam.
    bottom: Option<f64>,
    /// The bulwark's colour (default the topsides') and how much heavier than usual it is.
    rail: Option<Rgb>,
    rail_scale: Option<f64>,
}

impl HullSpec {
    fn new(beam: f64, freeboard: f64, draft: f64, topsides: Rgb, deck: Rgb) -> Self {
        HullSpec {
            beam,
            freeboard,
            draft,
            sheer_bow: None,
            sheer_stern: None,
            bow_length: None,
            transom: None,
            rake: None,
            overhang: None,
            topsides,
            deck,
            bottom: None,
            rail: None,
            rail_scale: None,
        }
    }
}

/// A hull station: its deck edge, the top of the boot-top, the waterline and the keel, to
/// starboard.
#[derive(Debug, Clone, Copy)]
struct Station {
    deck: V3,
    boot: V3,
    water: V3,
    keel: V3,
}

fn mirror(p: V3) -> V3 {
    [-p[0], p[1], p[2]]
}

fn same(p: V3) -> V3 {
    p
}

/// A side of the ship: 1 starboard, -1 port, and how a starboard point maps to it.
type Side = (f64, fn(V3) -> V3);

#[derive(Default)]
struct ShipBuilder {
    positions: Vec<f64>,
    normals: Vec<f64>,
    colors: Vec<f64>,
}

impl ShipBuilder {
    /// A flat-shaded triangle, wound and lit so its normal faces away from `inside`.
    fn tri(&mut self, a: V3, b: V3, c: V3, inside: V3, color: Rgb) {
        let (mut b, mut c) = (b, c);
        let mut n = cross(sub(b, a), sub(c, a));
        let len = js_hypot(&n);
        if len < 1e-12 {
            return;
        }
        n = [n[0] / len, n[1] / len, n[2] / len];
        let centroid: V3 = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0];
        if dot(n, sub(centroid, inside)) < 0.0 {
            std::mem::swap(&mut b, &mut c);
            n = [-n[0], -n[1], -n[2]];
        }
        for v in [a, b, c] {
            self.positions.extend_from_slice(&v);
            self.normals.extend_from_slice(&n);
            self.colors.extend_from_slice(&[color[0] / 255.0, color[1] / 255.0, color[2] / 255.0]);
        }
    }

    fn quad(&mut self, a: V3, b: V3, c: V3, d: V3, inside: V3, color: Rgb) {
        self.tri(a, b, c, inside, color);
        self.tri(a, c, d, inside, color);
    }

    /// A convex six-sided solid from its eight corners: the bottom four, then the top four above
    /// them, each going round the same way.
    fn solid(&mut self, corners: [V3; 8], color: Rgb, top: Rgb) {
        let inside = corners.iter().fold([0.0; 3], |m: V3, p| [m[0] + p[0] / 8.0, m[1] + p[1] / 8.0, m[2] + p[2] / 8.0]);
        let [b0, b1, b2, b3, t0, t1, t2, t3] = corners;
        self.quad(b0, b1, b2, b3, inside, color);
        self.quad(t0, t1, t2, t3, inside, top);
        self.quad(b0, b1, t1, t0, inside, color);
        self.quad(b1, b2, t2, t1, inside, color);
        self.quad(b2, b3, t3, t2, inside, color);
        self.quad(b3, b0, t0, t3, inside, color);
    }

    /// An axis-aligned box from its minimum and maximum corners, a different colour on top.
    fn box_top(&mut self, [x0, y0, z0]: V3, [x1, y1, z1]: V3, color: Rgb, top: Rgb) {
        self.solid(
            [
                [x0, y0, z0],
                [x1, y0, z0],
                [x1, y1, z0],
                [x0, y1, z0],
                [x0, y0, z1],
                [x1, y0, z1],
                [x1, y1, z1],
                [x0, y1, z1],
            ],
            color,
            top,
        );
    }

    /// An axis-aligned box from its minimum and maximum corners, one colour.
    fn r#box(&mut self, min: V3, max: V3, color: Rgb) {
        self.box_top(min, max, color, color);
    }

    /// A box centred on the centreline, a different colour on top.
    #[allow(clippy::too_many_arguments)]
    fn block_top(&mut self, half_width: f64, y0: f64, y1: f64, z0: f64, z1: f64, color: Rgb, top: Rgb) {
        self.box_top([-half_width, y0, z0], [half_width, y1, z1], color, top);
    }

    /// A box centred on the centreline, one colour.
    fn block(&mut self, half_width: f64, y0: f64, y1: f64, z0: f64, z1: f64, color: Rgb) {
        self.block_top(half_width, y0, y1, z0, z1, color, color);
    }

    /// A thin square rod from `a` to `b`, `r` its half-thickness.
    fn rod(&mut self, a: V3, b: V3, r: f64, color: Rgb) {
        let d = sub(b, a);
        let len = js_hypot(&d);
        if len < 1e-9 {
            return;
        }
        let axis: V3 = [d[0] / len, d[1] / len, d[2] / len];
        // any direction across the rod, then a second one across both
        let helper: V3 = if axis[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
        let mut u = cross(axis, helper);
        let ul = js_hypot(&u);
        u = [(u[0] / ul) * r, (u[1] / ul) * r, (u[2] / ul) * r];
        let v = cross(axis, u);
        let ring = |p: V3| -> [V3; 4] {
            [
                [p[0] - u[0] - v[0], p[1] - u[1] - v[1], p[2] - u[2] - v[2]],
                [p[0] + u[0] - v[0], p[1] + u[1] - v[1], p[2] + u[2] - v[2]],
                [p[0] + u[0] + v[0], p[1] + u[1] + v[1], p[2] + u[2] + v[2]],
                [p[0] - u[0] + v[0], p[1] - u[1] + v[1], p[2] - u[2] + v[2]],
            ]
        };
        let ([a0, a1, a2, a3], [b0, b1, b2, b3]) = (ring(a), ring(b));
        self.solid([a0, a1, a2, a3, b0, b1, b2, b3], color, color);
    }

    /// The hull: stations lofted from the stern (y = -0.5) to a pointed bow (y = 0.5), with
    /// sheer, flare above the waterline, a black boot-top at it and red antifouling below, and a
    /// flat deck on top. (The TypeScript returns the deck's height along y, which no caller
    /// reads.)
    fn hull(&mut self, h: HullSpec) {
        let HullSpec { beam, freeboard, draft, topsides, deck, .. } = h;
        let sheer_bow = h.sheer_bow.unwrap_or(freeboard * 0.5);
        let sheer_stern = h.sheer_stern.unwrap_or(freeboard * 0.15);
        let bow_length = h.bow_length.unwrap_or(0.22);
        let transom = h.transom.unwrap_or(0.85);
        let rake = h.rake.unwrap_or(0.03);
        let overhang = h.overhang.unwrap_or(0.02);
        let bottom = h.bottom.unwrap_or(0.75);
        let rail_color = h.rail.unwrap_or(topsides);
        let rail_scale = h.rail_scale.unwrap_or(1.0);
        let half = beam / 2.0;
        let boot = (freeboard * 0.25).min(0.006);
        let rail_height = (freeboard * 0.18).min(0.006) * rail_scale;
        let rail_width = beam * 0.04 * rail_scale;
        let steps = 16;
        let mut stations: Vec<f64> = (0..=steps).map(|i| i as f64 / steps as f64).collect();
        // the bow's taper gets extra stations so its curve stays smooth
        for i in 1..6 {
            stations.push(1.0 - bow_length + (bow_length * i as f64) / 6.0);
        }
        stations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let width = |s: f64| -> f64 {
            let from_bow = (s - (1.0 - bow_length)) / bow_length;
            if from_bow > 0.0 {
                return (1.0 - from_bow.powf(1.7)).max(0.0).powf(0.75);
            }
            if s < 0.14 {
                return lerp(transom, 1.0, (s / 0.14).sqrt());
            }
            1.0
        };
        let deck_z = |s: f64| -> f64 {
            freeboard
                + sheer_bow * ((s - 0.55) / 0.45).max(0.0).powf(2.0)
                + sheer_stern * ((0.15 - s) / 0.15).max(0.0).powf(2.0)
        };
        let at = |s: f64| -> Station {
            let w = width(s) * half;
            let dz = deck_z(s);
            // the waterline and keel are shorter than the deck: a raked stem and a stern overhang
            let y_water = lerp(-0.5 + overhang, 0.5 - rake, s);
            let y_keel = lerp(-0.5 + overhang * 2.0, 0.5 - rake * 1.6, s);
            let y_deck = -0.5 + s;
            let y_boot = lerp(y_water, y_deck, boot / dz);
            Station {
                deck: [w, y_deck, dz],
                boot: [w * 0.97, y_boot, boot],
                water: [w * 0.94, y_water, 0.0],
                keel: [w * bottom, y_keel, -draft],
            }
        };
        let ring: Vec<Station> = stations.iter().map(|&s| at(s)).collect();
        let sides: [Side; 2] = [(1.0, same), (-1.0, mirror)];
        for k in 0..ring.len() - 1 {
            let (a, b) = (ring[k], ring[k + 1]);
            let inside: V3 = [0.0, (a.deck[1] + b.deck[1]) / 2.0, (freeboard - draft) / 2.0];
            for (side, p) in sides {
                let out: V3 = [inside[0] - side, inside[1], inside[2]];
                self.quad(p(a.deck), p(b.deck), p(b.boot), p(a.boot), out, topsides);
                self.quad(p(a.boot), p(b.boot), p(b.water), p(a.water), out, BOOT_TOP);
                self.quad(p(a.water), p(b.water), p(b.keel), p(a.keel), out, ANTIFOULING);
            }
            self.quad(a.deck, b.deck, mirror(b.deck), mirror(a.deck), [0.0, inside[1], -1.0], deck);
            // a bulwark round the deck edge in the hull's colour, so the type shows from above too
            let rail = |p: V3, dw: f64, dz: f64| -> V3 { [(p[0] - dw).max(0.0), p[1], p[2] + dz] };
            for (side, p) in sides {
                let (ao, bo) = (rail(a.deck, 0.0, rail_height), rail(b.deck, 0.0, rail_height));
                let (ai, bi) = (rail(a.deck, rail_width, rail_height), rail(b.deck, rail_width, rail_height));
                let (af, bf) = (rail(a.deck, rail_width, 0.0), rail(b.deck, rail_width, 0.0));
                let mid: V3 = [side * (a.deck[0] - rail_width / 2.0).max(0.0), inside[1], a.deck[2] + rail_height / 2.0];
                // outside, top and inside faces, each lit away from the rail's middle
                self.quad(p(a.deck), p(b.deck), p(bo), p(ao), [mid[0] - side, mid[1], mid[2]], rail_color);
                self.quad(p(ao), p(bo), p(bi), p(ai), [mid[0], mid[1], mid[2] - 1.0], rail_color);
                self.quad(p(ai), p(bi), p(bf), p(af), [mid[0] + side, mid[1], mid[2]], shade(rail_color, 0.8));
            }
            self.quad(a.keel, b.keel, mirror(b.keel), mirror(a.keel), [0.0, inside[1], 1.0], ANTIFOULING);
        }
        // the transom
        let t = ring[0];
        let aft: V3 = [0.0, 1.0, 0.0];
        self.quad(t.deck, mirror(t.deck), mirror(t.boot), t.boot, aft, topsides);
        self.quad(t.boot, mirror(t.boot), mirror(t.water), t.water, aft, BOOT_TOP);
        self.quad(t.water, mirror(t.water), mirror(t.keel), t.keel, aft, ANTIFOULING);
    }

    fn build(self, beam: f64, top: f64) -> ShipModelArrays {
        let f32s = |v: Vec<f64>| v.into_iter().map(|x| x as f32).collect();
        ShipModelArrays { positions: f32s(self.positions), normals: f32s(self.normals), colors: f32s(self.colors), beam, top }
    }
}

/// A small deterministic sequence, so the container stacks look the same every time.
fn seeded(seed: u32) -> impl FnMut() -> f64 {
    let mut s = seed;
    move || {
        // `(s * 1664525 + 1013904223) >>> 0`: exact in a double, then reduced mod 2³²
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        s as f64 / 2f64.powi(32)
    }
}

/// An aft bridge: a deckhouse with a window band, bridge wings and a funnel behind it.
fn aft_bridge(m: &mut ShipBuilder, beam: f64, y0: f64, y1: f64, z0: f64, height: f64, funnel: bool) {
    let hw = beam * 0.4;
    let top = z0 + height;
    m.block(hw, y0, y1, z0, top - height * 0.12, WHITE);
    // the wheelhouse on top, a dark band of windows across its front, and wings to the ship's sides
    m.block(hw * 0.92, y0 + (y1 - y0) * 0.25, y1, top - height * 0.12, top, WHITE);
    m.r#box([-hw * 0.93, y1 - 0.002, top - height * 0.1], [hw * 0.93, y1 + 0.001, top - height * 0.03], WINDOW);
    m.r#box([-beam * 0.5, y1 - (y1 - y0) * 0.35, top - height * 0.13], [beam * 0.5, y1, top - height * 0.1], OFF_WHITE);
    if funnel {
        let fy = y0 - (y1 - y0) * 0.05;
        m.block(beam * 0.1, fy, fy + (y1 - y0) * 0.4, top - height * 0.3, top + height * 0.12, FUNNEL);
    }
}

fn container() -> ShipModelArrays {
    let mut m = ShipBuilder::default();
    let beam = 0.14;
    let freeboard = 0.04;
    m.hull(HullSpec {
        bow_length: Some(0.25),
        sheer_bow: Some(0.02),
        ..HullSpec::new(beam, freeboard, 0.02, topsides_of(4), STEEL_DECK)
    });
    // a forecastle and a breakwater ahead of the first bay
    m.block(beam * 0.32, 0.36, 0.43, freeboard, freeboard + 0.022, OFF_WHITE);
    let mut rand = seeded(7);
    let rows = 5;
    let bay_length = 0.036;
    let gap = 0.005;
    let mut stack = |m: &mut ShipBuilder, y0: f64, y1: f64, narrow: f64| {
        for r in 0..rows {
            let x0 = -beam * 0.46 * narrow + ((beam * 0.92 * narrow) / rows as f64) * r as f64;
            let x1 = x0 + (beam * 0.92 * narrow) / rows as f64 - 0.0012;
            let tiers = 3.0 + (rand() * 4.0).floor();
            let color = CONTAINERS[(rand() * CONTAINERS.len() as f64).floor() as usize];
            m.r#box([x0, y0, freeboard], [x1, y1, freeboard + tiers * 0.0085], color);
        }
    };
    // bays ahead of the bridge, then two behind it
    let mut y = 0.34;
    while y - bay_length > -0.24 {
        stack(&mut m, y - bay_length, y, if y > 0.22 { 0.7 } else { 1.0 });
        y -= bay_length + gap;
    }
    stack(&mut m, -0.42, -0.42 + bay_length, 0.85);
    aft_bridge(&mut m, beam, -0.36, -0.28, freeboard, 0.085, true);
    m.build(beam, freeboard + 0.085 * 1.12)
}

fn tanker() -> ShipModelArrays {
    let mut m = ShipBuilder::default();
    let beam = 0.17;
    let freeboard = 0.032;
    m.hull(HullSpec {
        bow_length: Some(0.2),
        transom: Some(0.9),
        ..HullSpec::new(beam, freeboard, 0.022, topsides_of(6), RUST_DECK)
    });
    m.block(beam * 0.3, 0.38, 0.45, freeboard, freeboard + 0.018, OFF_WHITE);
    // the pipe run along the centreline, the manifold across midships, and a catwalk over them
    m.block(beam * 0.06, -0.3, 0.38, freeboard, freeboard + 0.006, OFF_WHITE);
    m.r#box([-beam * 0.42, -0.02, freeboard], [beam * 0.42, 0.02, freeboard + 0.008], OFF_WHITE);
    m.r#box([-beam * 0.1, 0.06, freeboard], [beam * 0.1, 0.075, freeboard + 0.01], CRANE);
    let mut y = -0.25;
    while y < 0.36 {
        m.block(beam * 0.02, y, y + 0.008, freeboard, freeboard + 0.012, OFF_WHITE);
        y += 0.08;
    }
    aft_bridge(&mut m, beam, -0.42, -0.33, freeboard, 0.08, true);
    m.build(beam, freeboard + 0.08 * 1.12)
}

/// Bulk carriers (large hatch covers) and general cargo ships (smaller hatches, deck cranes).
fn bulk(cranes: bool) -> ShipModelArrays {
    let mut m = ShipBuilder::default();
    let beam = 0.16;
    let freeboard = if cranes { 0.055 } else { 0.045 };
    m.hull(HullSpec {
        bow_length: Some(0.2),
        ..HullSpec::new(beam, freeboard, 0.022, topsides_of(4), if cranes { STEEL_DECK } else { RUST_DECK })
    });
    m.block(beam * 0.34, 0.38, 0.45, freeboard, freeboard + 0.02, OFF_WHITE);
    let holds = if cranes { 4 } else { 7 };
    let span = 0.62;
    let start = -0.27;
    let each = span / holds as f64;
    for i in 0..holds {
        let y0 = start + i as f64 * each + each * 0.08;
        let y1 = start + (i + 1) as f64 * each - each * 0.08;
        let (side, top) = if cranes {
            ([64.0, 92.0, 120.0], [84.0, 116.0, 148.0])
        } else {
            ([150.0, 104.0, 70.0], [170.0, 120.0, 80.0])
        };
        m.block_top(beam * 0.32, y0, y1, freeboard, freeboard + 0.012, side, top);
        if cranes && i < holds - 1 {
            // a crane post between holds, its jib reaching forward over the next one
            let y = y1 + each * 0.08;
            let x = (if i % 2 != 0 { -1.0 } else { 1.0 }) * beam * 0.18;
            m.rod([x, y, freeboard], [x, y, freeboard + 0.07], 0.005, CRANE);
            m.rod([x, y, freeboard + 0.055], [x * 0.3, y + each * 0.85, freeboard + 0.1], 0.0028, CRANE);
        }
    }
    aft_bridge(&mut m, beam, -0.42, -0.33, freeboard, 0.075, true);
    m.build(beam, freeboard + 0.075 * 1.12)
}

fn passenger() -> ShipModelArrays {
    let mut m = ShipBuilder::default();
    let beam = 0.15;
    let freeboard = 0.045;
    m.hull(HullSpec {
        bow_length: Some(0.26),
        sheer_bow: Some(0.015),
        transom: Some(0.9),
        ..HullSpec::new(beam, freeboard, 0.02, topsides_of(3), TEAK)
    });
    // tiers of decks, each stepping back at the front, with a band of windows along both sides
    let tiers = 6;
    let tier = 0.021;
    let mut z = freeboard;
    for i in 0..tiers {
        let i = i as f64;
        let front = 0.3 - i * 0.022;
        let back = -0.43 + i * 0.012;
        let hw = beam * (0.49 - i * 0.012);
        m.block(hw, back, front, z, z + tier, WHITE);
        m.r#box([-hw - 0.0012, back + 0.01, z + tier * 0.35], [hw + 0.0012, front - 0.008, z + tier * 0.7], WINDOW);
        m.r#box([-hw * 0.9, front - 0.002, z + tier * 0.35], [hw * 0.9, front + 0.0012, z + tier * 0.7], WINDOW);
        z += tier;
    }
    // the funnel, aft of midships
    m.block_top(beam * 0.14, -0.27, -0.17, z, z + 0.03, topsides_of(3), FUNNEL);
    m.build(beam, z + 0.03)
}

fn tug() -> ShipModelArrays {
    let mut m = ShipBuilder::default();
    let beam = 0.34;
    let freeboard = 0.07;
    m.hull(HullSpec {
        sheer_bow: Some(0.07),
        sheer_stern: Some(0.01),
        bow_length: Some(0.32),
        transom: Some(0.8),
        bottom: Some(0.7),
        rake: Some(0.02),
        overhang: Some(0.03),
        rail: Some(BOOT_TOP),
        rail_scale: Some(2.5),
        ..HullSpec::new(beam, freeboard, 0.05, topsides_of(2), STEEL_DECK)
    });
    // (the black bulwark is its fender belt)
    // the deckhouse, with the wheelhouse on top and windows all round
    m.block(beam * 0.32, -0.08, 0.2, freeboard, freeboard + 0.1, WHITE);
    m.block_top(beam * 0.27, -0.02, 0.18, freeboard + 0.1, freeboard + 0.19, WHITE, topsides_of(2));
    m.r#box([-beam * 0.275, -0.025, freeboard + 0.13], [beam * 0.275, 0.185, freeboard + 0.17], WINDOW);
    // twin stacks behind it and the tow hook aft
    m.r#box([-beam * 0.12, -0.17, freeboard], [-beam * 0.02, -0.11, freeboard + 0.17], FUNNEL);
    m.r#box([beam * 0.02, -0.17, freeboard], [beam * 0.12, -0.11, freeboard + 0.17], FUNNEL);
    m.block(beam * 0.12, -0.36, -0.28, freeboard, freeboard + 0.04, CRANE);
    m.rod([0.0, 0.08, freeboard + 0.19], [0.0, 0.08, freeboard + 0.3], 0.006, MAST);
    m.build(beam, freeboard + 0.19)
}

fn fishing() -> ShipModelArrays {
    let mut m = ShipBuilder::default();
    let beam = 0.28;
    let freeboard = 0.065;
    m.hull(HullSpec {
        sheer_bow: Some(0.06),
        sheer_stern: Some(0.01),
        bow_length: Some(0.3),
        transom: Some(0.75),
        bottom: Some(0.6),
        ..HullSpec::new(beam, freeboard, 0.045, topsides_of(5), STEEL_DECK)
    });
    // a wheelhouse forward
    m.block(beam * 0.34, 0.08, 0.3, freeboard, freeboard + 0.08, WHITE);
    m.block(beam * 0.3, 0.12, 0.28, freeboard + 0.08, freeboard + 0.15, WHITE);
    m.r#box([-beam * 0.305, 0.2, freeboard + 0.1], [beam * 0.305, 0.282, freeboard + 0.135], WINDOW);
    // a mast behind it, and the A-frame gantry over the stern
    m.rod([0.0, 0.06, freeboard], [0.0, 0.06, freeboard + 0.32], 0.006, MAST);
    let gz = freeboard + 0.22;
    for x in [-1.0, 1.0] {
        m.rod([x * beam * 0.4, -0.45, freeboard], [x * beam * 0.36, -0.4, gz], 0.008, ORANGE);
    }
    m.rod([-beam * 0.37, -0.4, gz], [beam * 0.37, -0.4, gz], 0.008, ORANGE);
    // a winch drum
    m.block(beam * 0.22, -0.2, -0.12, freeboard, freeboard + 0.04, CRANE);
    m.build(beam, freeboard + 0.15)
}

fn pleasure() -> ShipModelArrays {
    let mut m = ShipBuilder::default();
    let beam = 0.3;
    let freeboard = 0.07;
    m.hull(HullSpec {
        sheer_bow: Some(0.03),
        bow_length: Some(0.48),
        transom: Some(0.8),
        bottom: Some(0.35),
        rake: Some(0.05),
        overhang: Some(0.04),
        ..HullSpec::new(beam, freeboard, 0.04, topsides_of(8), TEAK)
    });
    // a low coachroof with a dark window strip, the cockpit aft
    m.block(beam * 0.28, -0.12, 0.2, freeboard, freeboard + 0.04, WHITE);
    m.r#box([-beam * 0.285, -0.08, freeboard + 0.015], [beam * 0.285, 0.16, freeboard + 0.03], WINDOW);
    m.r#box([-beam * 0.3, -0.42, freeboard], [beam * 0.3, -0.2, freeboard + 0.005], topsides_of(8));
    // the mast and boom
    m.rod([0.0, 0.12, freeboard + 0.04], [0.0, 0.12, 0.95], 0.007, MAST);
    m.rod([0.0, 0.12, freeboard + 0.1], [0.0, -0.3, freeboard + 0.1], 0.006, MAST);
    m.build(beam, freeboard + 0.04)
}

fn sar() -> ShipModelArrays {
    let mut m = ShipBuilder::default();
    let beam = 0.3;
    let freeboard = 0.06;
    m.hull(HullSpec {
        sheer_bow: Some(0.035),
        bow_length: Some(0.4),
        transom: Some(0.92),
        bottom: Some(0.3),
        rake: Some(0.04),
        ..HullSpec::new(beam, freeboard, 0.035, [44.0, 62.0, 104.0], STEEL_DECK)
    });
    // a bright orange cabin with a wraparound screen, and a radar mast
    m.block(beam * 0.36, -0.18, 0.2, freeboard, freeboard + 0.1, ORANGE);
    m.r#box([-beam * 0.365, -0.05, freeboard + 0.055], [beam * 0.365, 0.205, freeboard + 0.085], WINDOW);
    m.block(beam * 0.28, -0.12, 0.08, freeboard + 0.1, freeboard + 0.14, ORANGE);
    m.rod([0.0, -0.05, freeboard + 0.14], [0.0, -0.05, freeboard + 0.25], 0.006, MAST);
    m.r#box([-beam * 0.2, -0.065, freeboard + 0.23], [beam * 0.2, -0.035, freeboard + 0.24], MAST);
    m.build(beam, freeboard + 0.14)
}

fn generic() -> ShipModelArrays {
    let mut m = ShipBuilder::default();
    let beam = 0.18;
    let freeboard = 0.06;
    m.hull(HullSpec { bow_length: Some(0.25), ..HullSpec::new(beam, freeboard, 0.025, topsides_of(0), STEEL_DECK) });
    m.block(beam * 0.3, 0.36, 0.44, freeboard, freeboard + 0.025, OFF_WHITE);
    aft_bridge(&mut m, beam, -0.38, -0.22, freeboard, 0.09, true);
    m.block(beam * 0.3, -0.1, 0.25, freeboard, freeboard + 0.015, [120.0, 124.0, 120.0]);
    m.build(beam, freeboard + 0.09 * 1.12)
}

/// One class's model as triangle arrays, built afresh (`shipModelArrays`).
pub fn ship_model_arrays(cls: ShipClass) -> ShipModelArrays {
    match cls {
        ShipClass::Container => container(),
        ShipClass::Tanker => tanker(),
        ShipClass::Bulk => bulk(false),
        ShipClass::Cargo => bulk(true),
        ShipClass::Passenger => passenger(),
        ShipClass::Tug => tug(),
        ShipClass::Fishing => fishing(),
        ShipClass::Pleasure => pleasure(),
        ShipClass::Sar => sar(),
        ShipClass::Generic => generic(),
    }
}

/// Every class's model, built once on first use (`shipMeshes()`), indexed by `cls as usize`.
pub fn ship_meshes() -> &'static [ShipModelArrays; 10] {
    static MESHES: OnceLock<[ShipModelArrays; 10]> = OnceLock::new();
    MESHES.get_or_init(|| SHIP_CLASSES.map(ship_model_arrays))
}

/// Beam and top of a class's model (`shipModelShape`), from [`ship_meshes`].
fn ship_model_shape(cls: ShipClass) -> (f64, f64) {
    let m = &ship_meshes()[cls as usize];
    (m.beam, m.top)
}

/// Typical lengths in metres by class and size bucket (small, medium, large), for ships without
/// dimensions (`TYPICAL_LENGTH`).
pub fn typical_length(cls: ShipClass) -> [f64; 3] {
    match cls {
        ShipClass::Container => [140.0, 220.0, 330.0],
        ShipClass::Tanker => [90.0, 180.0, 270.0],
        ShipClass::Bulk => [130.0, 190.0, 250.0],
        ShipClass::Cargo => [60.0, 95.0, 130.0],
        ShipClass::Passenger => [40.0, 110.0, 230.0],
        ShipClass::Tug => [20.0, 28.0, 36.0],
        ShipClass::Fishing => [14.0, 24.0, 45.0],
        ShipClass::Pleasure => [8.0, 12.0, 18.0],
        ShipClass::Sar => [10.0, 16.0, 24.0],
        ShipClass::Generic => [25.0, 60.0, 120.0],
    }
}

/// Usable AIS dimensions: a believable length and beam (`usableDims`).
pub fn usable_dims(dims: Option<[u16; 4]>) -> Option<[u16; 4]> {
    let [bow, stern, port, stbd] = dims?.map(f64::from);
    let length = bow + stern;
    let beam = port + stbd;
    if !((3.0..=500.0).contains(&length) && beam >= 1.0 && beam <= length * 0.7) {
        return None;
    }
    dims
}

/// The model a ship is drawn with, from its type colour and size (`shipClassOf`).
pub fn ship_class_of(ship: &Ship) -> ShipClass {
    let dims = usable_dims(ship.dims);
    let length = dims.map(|d| d[0] as f64 + d[1] as f64);
    // without dimensions, only "large" counts as long
    let long = |metres: f64| length.unwrap_or_else(|| [0.0, 0.0, f64::INFINITY].get(ship.size as usize).copied().unwrap_or(0.0)) >= metres;
    match ship.kind {
        2 => {
            if long(55.0) {
                ShipClass::Generic
            } else {
                ShipClass::Tug
            }
        }
        3 => ShipClass::Passenger,
        4 => {
            // the feed doesn't say which cargo ship: big ones are container ships or bulk
            // carriers (fixed per ship), smaller ones general cargo
            if !long(150.0) {
                ShipClass::Cargo
            } else if ship.mmsi.is_multiple_of(3) {
                ShipClass::Bulk
            } else {
                ShipClass::Container
            }
        }
        5 => {
            if long(70.0) {
                ShipClass::Generic
            } else {
                ShipClass::Fishing
            }
        }
        6 => ShipClass::Tanker,
        7 => {
            if long(45.0) {
                ShipClass::Generic
            } else {
                ShipClass::Sar
            }
        }
        8 => {
            if long(30.0) {
                ShipClass::Passenger
            } else {
                ShipClass::Pleasure
            }
        }
        _ => ShipClass::Generic,
    }
}

/// The tallest a superstructure is drawn, metres; taller models are squashed to it.
pub const MAX_SUPERSTRUCTURE_M: f64 = 40.0;

/// How big a ship is drawn and where its hull sits (`ShipPlacement`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShipPlacement {
    pub cls: ShipClass,
    pub length: f64,
    pub beam: f64,
    /// Mesh scale: metres per model unit across, along and up.
    pub scale: [f64; 3],
    /// The hull's centre from the AIS reference point, metres to starboard and ahead.
    pub right: f64,
    pub ahead: f64,
}

/// How big a ship is drawn and where its hull sits around the reported position: from its AIS
/// dimensions (reference point to bow, stern, port and starboard) or, without them, a typical
/// size for its class, centred (`shipPlacement`, with the class its default).
pub fn ship_placement(ship: &Ship) -> ShipPlacement {
    ship_placement_as(ship, ship_class_of(ship))
}

/// [`ship_placement`] drawn as `cls` (`shipPlacement(ship, cls)`).
pub fn ship_placement_as(ship: &Ship, cls: ShipClass) -> ShipPlacement {
    let (model_beam, model_top) = ship_model_shape(cls);
    let dims = usable_dims(ship.dims);
    let length: f64;
    let beam: f64;
    let mut right = 0.0;
    let mut ahead = 0.0;
    if let Some(dims) = dims {
        let [bow, stern, port, stbd] = dims.map(f64::from);
        length = bow + stern;
        beam = port + stbd;
        ahead = (bow - stern) / 2.0;
        right = (stbd - port) / 2.0;
    } else {
        let typical = typical_length(cls);
        length = typical.get(ship.size as usize).copied().unwrap_or(typical[1]);
        beam = length * model_beam;
    }
    // heights follow the length, but no superstructure towers over 40 m
    let up = length.min(MAX_SUPERSTRUCTURE_M / model_top);
    ShipPlacement { cls, length, beam, scale: [beam / model_beam, length, up], right, ahead }
}

/// A starboard/ahead offset in metres turned to `heading` (degrees from north), as east and
/// north (`offsetEastNorth`).
pub fn offset_east_north(right: f64, ahead: f64, heading: f64) -> [f64; 2] {
    let h = (heading * std::f64::consts::PI) / 180.0;
    let (s, c) = (h.sin(), h.cos());
    [right * c + ahead * s, -right * s + ahead * c]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;

    /// `shipModelArrays(cls)` checked as `ship-models.test.ts` does for every class.
    fn whole_triangles_unit_normals_inside_the_unit_box(cls: ShipClass) {
        let m = ship_model_arrays(cls);
        let vertices = m.positions.len() / 3;
        assert!(vertices > 0);
        assert_eq!(vertices % 3, 0);
        // low-poly: a few hundred triangles to a couple of thousand
        assert!(vertices / 3 < 3000);
        assert_eq!(m.normals.len(), m.positions.len());
        assert_eq!(m.colors.len(), m.positions.len());
        let (mut min_z, mut max_z, mut min_y, mut max_y, mut max_x) =
            (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY, 0.0_f64);
        for i in (0..m.positions.len()).step_by(3) {
            let [x, y, z] = [m.positions[i] as f64, m.positions[i + 1] as f64, m.positions[i + 2] as f64];
            assert!((x + y + z).is_finite());
            assert_close!(js_hypot(&[m.normals[i] as f64, m.normals[i + 1] as f64, m.normals[i + 2] as f64]), 1, 5);
            for k in 0..3 {
                assert!(m.colors[i + k] >= 0.0);
                assert!(m.colors[i + k] <= 1.0);
            }
            max_x = max_x.max(x.abs());
            (min_y, max_y, min_z, max_z) = (min_y.min(y), max_y.max(y), min_z.min(z), max_z.max(z));
        }
        // one unit long, bow to +y, the beam its own share of that
        assert_close!(min_y, -0.5, 3);
        assert_close!(max_y, 0.5, 3);
        // (fenders and window bands may stand a hair proud of the hull)
        assert!(max_x <= (m.beam / 2.0) * 1.02);
        assert!(max_x >= m.beam / 2.0 - 1e-3);
        assert!(m.beam <= 0.5);
        // sits on the waterline with only a little below it
        assert!(min_z < 0.0);
        assert!(min_z > -0.1);
        assert!(max_z <= 1.0);
        assert!(m.top > 0.0);
        assert!(m.top <= max_z + 1e-6);
    }

    #[test]
    fn container_whole_triangles_unit_normals_inside_the_unit_box() {
        whole_triangles_unit_normals_inside_the_unit_box(ShipClass::Container);
    }

    #[test]
    fn tanker_whole_triangles_unit_normals_inside_the_unit_box() {
        whole_triangles_unit_normals_inside_the_unit_box(ShipClass::Tanker);
    }

    #[test]
    fn bulk_whole_triangles_unit_normals_inside_the_unit_box() {
        whole_triangles_unit_normals_inside_the_unit_box(ShipClass::Bulk);
    }

    #[test]
    fn cargo_whole_triangles_unit_normals_inside_the_unit_box() {
        whole_triangles_unit_normals_inside_the_unit_box(ShipClass::Cargo);
    }

    #[test]
    fn passenger_whole_triangles_unit_normals_inside_the_unit_box() {
        whole_triangles_unit_normals_inside_the_unit_box(ShipClass::Passenger);
    }

    #[test]
    fn tug_whole_triangles_unit_normals_inside_the_unit_box() {
        whole_triangles_unit_normals_inside_the_unit_box(ShipClass::Tug);
    }

    #[test]
    fn fishing_whole_triangles_unit_normals_inside_the_unit_box() {
        whole_triangles_unit_normals_inside_the_unit_box(ShipClass::Fishing);
    }

    #[test]
    fn pleasure_whole_triangles_unit_normals_inside_the_unit_box() {
        whole_triangles_unit_normals_inside_the_unit_box(ShipClass::Pleasure);
    }

    #[test]
    fn sar_whole_triangles_unit_normals_inside_the_unit_box() {
        whole_triangles_unit_normals_inside_the_unit_box(ShipClass::Sar);
    }

    #[test]
    fn generic_whole_triangles_unit_normals_inside_the_unit_box() {
        whole_triangles_unit_normals_inside_the_unit_box(ShipClass::Generic);
    }

    #[test]
    fn the_hulls_normals_face_outward_the_bows_sides_lean_forward_the_deck_up() {
        let m = ship_model_arrays(ShipClass::Generic);
        let mut forward = 0;
        let mut up = 0;
        for i in (0..m.positions.len()).step_by(3) {
            if m.positions[i + 1] > 0.4 && m.normals[i].abs() > 0.3 {
                forward += if m.normals[i + 1] > 0.0 { 1 } else { -1 };
            }
            if m.normals[i + 2] > 0.99 {
                up += 1;
            }
        }
        assert!(forward > 0);
        assert!(up > 0);
    }

    fn ship(kind: u8, length: Option<u16>, mmsi: u32, size: u8) -> Ship {
        Ship { kind, size, mmsi, dims: length.map(|l| [l / 2, l / 2, 5, 5]), ..Ship::default() }
    }

    #[test]
    fn by_type_colour_and_length() {
        let class = |kind, length| ship_class_of(&ship(kind, Some(length), 1, 1));
        assert_eq!(class(6, 200), ShipClass::Tanker);
        assert_eq!(class(3, 150), ShipClass::Passenger);
        assert_eq!(class(2, 30), ShipClass::Tug);
        assert_eq!(class(5, 20), ShipClass::Fishing);
        assert_eq!(class(8, 12), ShipClass::Pleasure);
        assert_eq!(class(7, 18), ShipClass::Sar);
        assert_eq!(class(0, 80), ShipClass::Generic);
        assert_eq!(class(9, 80), ShipClass::Generic);
        assert_eq!(class(4, 90), ShipClass::Cargo);
        assert_eq!(ship_class_of(&ship(4, Some(300), 1, 1)), ShipClass::Container);
        assert_eq!(ship_class_of(&ship(4, Some(300), 3, 1)), ShipClass::Bulk);
        // without dimensions the size bucket decides
        assert_eq!(ship_class_of(&ship(4, None, 1, 2)), ShipClass::Container);
        assert_eq!(ship_class_of(&ship(4, None, 1, 0)), ShipClass::Cargo);
    }

    fn dims(kind: u8, size: u8, dims: Option<[u16; 4]>) -> Ship {
        Ship { kind, size, mmsi: 1, dims, ..Ship::default() }
    }

    #[test]
    fn true_length_and_beam_centred_between_bow_and_stern_port_and_starboard() {
        let p = ship_placement(&dims(6, 2, Some([200, 50, 10, 30])));
        assert_eq!(p.cls, ShipClass::Tanker);
        assert_eq!(p.length, 250.0);
        assert_eq!(p.beam, 40.0);
        assert_eq!(p.ahead, 75.0);
        assert_eq!(p.right, 10.0);
        let m = ship_model_arrays(ShipClass::Tanker);
        assert_close!(p.scale[0] * m.beam, 40, 6);
        assert_eq!(p.scale[1], 250.0);
        // the bridge is squashed to the cap
        assert!(p.scale[2] * m.top <= MAX_SUPERSTRUCTURE_M + 1e-6);
    }

    #[test]
    fn a_small_boat_keeps_its_proportions() {
        let p = ship_placement(&dims(8, 0, Some([6, 6, 2, 2])));
        assert_eq!(p.scale[2], 12.0);
    }

    #[test]
    fn without_dimensions_a_typical_size_centred() {
        let p = ship_placement(&dims(2, 1, None));
        assert_eq!(p.length, typical_length(ShipClass::Tug)[1]);
        assert_eq!(p.right, 0.0);
        assert_eq!(p.ahead, 0.0);
        assert_close!(p.scale[0], p.scale[1], 6);
        // nonsense dimensions are ignored
        assert_eq!(ship_placement(&dims(2, 1, Some([0, 0, 0, 0]))).length, typical_length(ShipClass::Tug)[1]);
    }

    #[test]
    fn offsets_turn_with_the_heading() {
        let [e, n] = offset_east_north(0.0, 100.0, 90.0);
        assert_close!(e, 100, 9);
        assert_close!(n, 0, 9);
        let [e2, n2] = offset_east_north(10.0, 0.0, 0.0);
        assert_close!(e2, 10, 9); // starboard is east heading north
        assert_close!(n2, 0, 9);
    }

    /// What bun gives for `shipModelArrays(cls)`: vertices, beam, top, the bounding box and the
    /// sums of the positions, normals and colours.
    #[allow(clippy::type_complexity)]
    const TYPESCRIPT: [(ShipClass, usize, f64, f64, [f64; 3], [f64; 3], [f64; 3]); 10] = {
        use ShipClass::*;
        [
            (Container, 4602, 0.14, 0.13520000000000001, [-0.07000000029802322, -0.5, -0.019999999552965164], [0.07000000029802322, 0.5, 0.13519999384880066], [351.77549064718187, 29.643530011260737, 5885.111618600786]),
            (Tanker, 2382, 0.17, 0.12160000000000001, [-0.08500000089406967, -0.5, -0.02199999988079071], [0.08500000089406967, 0.5, 0.12160000205039978], [178.09459922322966, 10.156273201711883, 3099.471610330045]),
            (Bulk, 2238, 0.16, 0.129, [-0.07999999821186066, -0.5, -0.02199999988079071], [0.07999999821186066, 0.5, 0.1289999932050705], [189.41704156063463, 32.103355478502635, 2499.900047980249]),
            (Cargo, 2346, 0.16, 0.139, [-0.07999999821186066, -0.5, -0.02199999988079071], [0.07999999821186066, 0.5, 0.1576528698205948], [236.00291397422518, 37.94042196731607, 2750.2059260979295]),
            (Passenger, 2454, 0.15, 0.20099999999999998, [-0.07500000298023224, -0.5, -0.019999999552965164], [0.07500000298023224, 0.5, 0.20100000500679016], [270.561636261642, 34.179500419673786, 2945.1595547273755]),
            (Tug, 2022, 0.34, 0.26, [-0.17000000178813934, -0.5, -0.05000000074505806], [0.17000000178813934, 0.5, 0.3700000047683716], [247.6471806038171, 3.0036210986436345, 1512.5016588494182]),
            (Fishing, 1974, 0.28, 0.215, [-0.14000000059604645, -0.5, -0.04500000178813934], [0.14000000059604645, 0.5, 0.38499999046325684], [198.66253375075758, -28.204919218551368, 2534.20706178993]),
            (Pleasure, 1950, 0.3, 0.11000000000000001, [-0.15000000596046448, -0.5, -0.03999999910593033], [0.15000000596046448, 0.5, 0.949999988079071], [185.24631082452834, -87.24240603041835, 2637.4044679775834]),
            (Sar, 1950, 0.3, 0.2, [-0.15000000596046448, -0.5, -0.03500000014901161], [0.15000000596046448, 0.5, 0.3100000023841858], [196.7887719552964, -95.6947768181999, 1587.223549760878]),
            (Generic, 1938, 0.18, 0.1608, [-0.09000000357627869, -0.5, -0.02500000037252903], [0.09000000357627869, 0.5, 0.1607999950647354], [163.0780261978507, 29.065417526711826, 2298.6682443246245]),
        ]
    };

    #[test]
    fn every_class_matches_the_typescript() {
        for (cls, vertices, beam, top, min, max, sums) in TYPESCRIPT {
            let m = &ship_meshes()[cls as usize];
            assert_eq!(m.positions.len() / 3, vertices, "{cls:?}");
            assert_eq!((m.beam, m.top), (beam, top), "{cls:?}");
            let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            for v in m.positions.chunks_exact(3) {
                for k in 0..3 {
                    lo[k] = lo[k].min(v[k] as f64);
                    hi[k] = hi[k].max(v[k] as f64);
                }
            }
            assert_eq!((lo, hi), (min, max), "{cls:?}");
            let sum = |a: &[f32]| a.iter().map(|&v| v as f64).sum::<f64>();
            assert_close!(sum(&m.positions), sums[0], 6);
            assert_close!(sum(&m.normals), sums[1], 6);
            assert_close!(sum(&m.colors), sums[2], 6);
        }
    }

    #[test]
    fn the_container_stacks_follow_the_lcg() {
        // `seeded(7)`'s first draws, as bun prints them
        let mut rand = seeded(7);
        let draws: Vec<f64> = (0..3).map(|_| rand()).collect();
        assert_eq!(draws, [0.23878083983436227, 0.9134932646993548, 0.6124916663393378]);
    }

    #[test]
    fn classes_have_names_and_typical_lengths_grow() {
        let names: Vec<&str> = SHIP_CLASSES.iter().map(|c| c.as_str()).collect();
        assert_eq!(names, ["container", "tanker", "bulk", "cargo", "passenger", "tug", "fishing", "pleasure", "sar", "generic"]);
        for cls in SHIP_CLASSES {
            let [s, m, l] = typical_length(cls);
            assert!(s < m && m < l, "{cls:?}");
        }
        // a size bucket the feed never sends takes the medium length
        assert_eq!(ship_placement(&dims(2, 7, None)).length, 28.0);
        // and counts as short
        assert_eq!(ship_class_of(&dims(4, 7, None)), ShipClass::Cargo);
    }

    #[test]
    fn usable_dims_want_a_believable_length_and_beam() {
        assert_eq!(usable_dims(None), None);
        assert_eq!(usable_dims(Some([2, 1, 1, 1])), Some([2, 1, 1, 1]));
        assert_eq!(usable_dims(Some([1, 1, 1, 1])), None);
        assert_eq!(usable_dims(Some([250, 250, 5, 5])), Some([250, 250, 5, 5]));
        assert_eq!(usable_dims(Some([251, 250, 5, 5])), None);
        // a beam over 0.7 of the length, or under a metre, is nonsense
        assert_eq!(usable_dims(Some([5, 5, 4, 3])), Some([5, 5, 4, 3]));
        assert_eq!(usable_dims(Some([5, 5, 4, 4])), None);
        assert_eq!(usable_dims(Some([5, 5, 0, 0])), None);
    }
}
