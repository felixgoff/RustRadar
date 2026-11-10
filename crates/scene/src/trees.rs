//! Port of `tree-layer.ts` and `tree-models.ts` (S1.29; doc 04 §8): trees for the realistic map,
//! 3D trees wherever the vector tiles have woods, parks, gardens, cemeteries, orchards, scrub or
//! the gardens of residential streets, kept off the roads, railways, buildings and water of the
//! same tile. Conifers and broadleaves by latitude, each one of Streets GL's trees (fir; beech,
//! linden or oak), coloured by the season (autumn's yellows and reds, bare in winter).
//!
//! Each tile is planted once ([`plant_tile`]): its polygons and lines are rasterised into two
//! masks (what grows where, and where nothing may), then each kind of cover's own jittered grid
//! of candidate trees is tested against them. The grid is laid in the world, not the tile, so
//! trees don't line up along tile edges, and every tree's place, size and colour come from a
//! hash of its grid cell ([`hash2`]): the same trees every time.
//!
//! This is the pure part. The tile runtime of `treeLayers` (at most [`MAX_LOADING`] tiles loading,
//! [`MAX_CACHED`] planted tiles kept, a failed tile asked again only once forgotten, the shadows
//! made again when [`shadow_key`] changes) and the drawing (deck.gl's `SimpleMeshLayer` of
//! [`card_arrays`], with [`TREE_MATERIAL`] and [`SHADOW_MATERIAL`]) are the map's. Dates are
//! milliseconds since the epoch (`Date.getTime()`), never read from a clock here.
//!
//! Deviations, all for NaN input: the scanline's crossings are sorted with `total_cmp`, where the
//! TypeScript's comparator would make a NaN equal to everything; a tree whose place is NaN is
//! left out (the TypeScript's bounds checks let it through, and a mapped one would be planted).

use std::collections::HashMap;
use std::f64::consts::PI;

use rustradar_core::num::{js_round, to_int32};
use serde_json::Value;

use crate::bridges::TileIndex;
use crate::building_shadow::shadow_step;
use crate::canopy::{CanopyGrid, tallest_near};
use crate::geom::{js_hypot2, js_max, js_min};
use crate::landcover::{LandGrid, class_at, lc, wooded};
use crate::street_trees::{RegisteredTree, register_kinds};
use crate::streets_gl::{Properties, TreeKind, js_string_of, sgl_path, sgl_tree};

/// Tiles are planted from this zoom's data, OpenFreeMap's deepest.
pub const TREE_TILE_ZOOM: u32 = 14;
/// Masks, cells across a tile (about 2.7 m each at 56°N).
const GRID: usize = 512;
/// At most this many trees a tile; denser tiles are thinned evenly.
pub const MAX_TREES: usize = 24_000;
const EARTH_CIRCUMFERENCE: f64 = 40_075_016.686;
const DEG: f64 = PI / 180.0;

/// How a kind of land is planted: trees `spacing` metres apart, each there by `chance`, `height`
/// metres tall.
struct Cover {
    spacing: f64,
    chance: f64,
    /// Share of conifers; none: by latitude, as in a wood.
    conifers: Option<f64>,
    height: [f64; 2],
    /// Crown width against the mesh's own, by tree.
    width: [f64; 2],
}

const fn cover(spacing: f64, chance: f64, conifers: Option<f64>, height: [f64; 2], width: [f64; 2]) -> Cover {
    Cover { spacing, chance, conifers, height, width }
}

/// The codes in the cover mask, in drawing order: later ones win where they overlap.
const COVERS: [Cover; 13] = [
    // 0: nothing grows
    cover(1.0, 0.0, Some(0.0), [0.0, 0.0], [1.0, 1.0]),
    // 1: residential streets' gardens
    cover(15.0, 0.2, Some(0.25), [6.0, 13.0], [0.9, 1.25]),
    // 2: school, hospital and university grounds
    cover(16.0, 0.16, Some(0.2), [7.0, 14.0], [0.9, 1.3]),
    // 3: allotments
    cover(10.0, 0.18, Some(0.05), [3.0, 6.0], [1.0, 1.3]),
    // 4: gardens
    cover(12.0, 0.32, Some(0.2), [5.0, 11.0], [0.9, 1.3]),
    // 5: parks, recreation grounds, village greens
    cover(15.0, 0.42, Some(0.18), [9.0, 18.0], [0.95, 1.35]),
    // 6: cemeteries
    cover(11.0, 0.55, Some(0.3), [8.0, 16.0], [0.85, 1.2]),
    // 7: orchards: low trees in rows
    cover(5.0, 0.95, Some(0.0), [3.2, 4.8], [1.1, 1.3]),
    // 8: wetland: scattered birches and willows
    cover(10.0, 0.22, Some(0.1), [4.0, 10.0], [0.9, 1.2]),
    // 9: scrub, heath and shrubberies: bushes
    cover(5.5, 0.42, Some(0.2), [2.2, 4.5], [1.1, 1.5]),
    // 10: woods and forests
    cover(6.2, 0.92, None, [13.0, 24.0], [0.85, 1.15]),
    // 11, 12: woods mapped broadleaved, or needleleaved (Streets GL's tiles say)
    cover(6.2, 0.92, Some(0.05), [13.0, 24.0], [0.85, 1.15]),
    cover(6.2, 0.92, Some(0.97), [13.0, 24.0], [0.85, 1.15]),
];
const ORCHARD: u8 = 7;

/// A point of a decoded feature, in the tile's own coordinates (0 to 1 across, y down).
pub type Point = [f64; 2];

/// A decoded feature's geometry, GeoJSON's (MVTLoader's with `coordinates: "local"`).
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    Point(Point),
    MultiPoint(Vec<Point>),
    LineString(Vec<Point>),
    MultiLineString(Vec<Vec<Point>>),
    /// Rings, the outer one first.
    Polygon(Vec<Vec<Point>>),
    MultiPolygon(Vec<Vec<Vec<Point>>>),
}

impl Geometry {
    /// Its GeoJSON `type`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Geometry::Point(_) => "Point",
            Geometry::MultiPoint(_) => "MultiPoint",
            Geometry::LineString(_) => "LineString",
            Geometry::MultiLineString(_) => "MultiLineString",
            Geometry::Polygon(_) => "Polygon",
            Geometry::MultiPolygon(_) => "MultiPolygon",
        }
    }
}

/// A tile's decoded feature: its geometry and its tags, with its layer's name as `layerName`
/// (as loaders.gl adds it).
#[derive(Debug, Clone, PartialEq)]
pub struct Feature {
    pub geometry: Geometry,
    pub properties: Properties,
}

/// `p[key] === value` for a string `value`.
fn is(p: &Properties, key: &str, value: &str) -> bool {
    p.get(key).and_then(Value::as_str) == Some(value)
}

/// The cover a feature is, if trees grow in it (0 if not).
pub fn cover_of(p: &Properties) -> u8 {
    let layer = |name: &str| is(p, "layerName", name);
    let cls = |name: &str| is(p, "class", name);
    let sub = |names: &[&str]| names.iter().any(|name| is(p, "subclass", name));
    // Streets GL's tiles (streets_gl.rs): woods by their leaves, scrub, gardens
    if layer("natural") {
        if is(p, "type", "forest") {
            return if is(p, "leafType", "broadleaved") {
                11
            } else if is(p, "leafType", "needleleaved") {
                12
            } else {
                10
            };
        }
        return if is(p, "type", "scrub") { 9 } else { 0 };
    }
    if layer("common") {
        return if is(p, "type", "garden") { 4 } else { 0 };
    }
    if layer("landcover") {
        if cls("wood") {
            return 10;
        }
        if cls("wetland") {
            return 8;
        }
        if cls("farmland") {
            return if sub(&["orchard"]) { ORCHARD } else { 0 };
        }
        if cls("grass") {
            if sub(&["scrub", "shrubbery", "heath"]) {
                return 9;
            }
            if sub(&["park", "recreation_ground", "village_green"]) {
                return 5;
            }
            if sub(&["garden"]) {
                return 4;
            }
            if sub(&["allotments"]) {
                return 3;
            }
        }
        return 0;
    }
    if layer("landuse") {
        if cls("cemetery") {
            return 6;
        }
        if cls("residential") {
            return 1;
        }
        if cls("school") || cls("university") || cls("college") || cls("hospital") {
            return 2;
        }
    }
    0
}

/// Metres to keep clear either side of a road, path or railway line (`CLEARANCE`).
fn clearance(class: &str) -> Option<f64> {
    Some(match class {
        "motorway" => 12.0,
        "trunk" => 9.0,
        "primary" => 8.0,
        "secondary" => 7.0,
        "tertiary" => 6.0,
        "minor" => 5.0,
        "service" => 3.5,
        "busway" => 5.0,
        "raceway" => 6.0,
        "track" => 2.5,
        "path" => 1.5,
        "rail" => 3.5,
        "transit" => 3.0,
        "pier" => 2.0,
        "bridge" => 5.0,
        _ => return None,
    })
}

/// Paved or kept clear, in Streets GL's tiles.
const SGL_BARE: [&str; 12] =
    ["parking", "pitch", "playground", "dogPark", "bridge", "construction", "brownfield", "apron", "helipad", "pier", "sand", "rock"];

/// Where nothing grows (`blockOf`'s `number | true`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Block {
    /// A polygon to fill (`true`).
    Fill,
    /// A line's clearance either side, metres; nothing unless over 0.
    Clear(f64),
}

/// Where nothing grows: a polygon to fill, a line's clearance in metres, or nothing (a clearance
/// of 0). `geometry_type` is the feature's GeoJSON type.
pub fn block_of(p: &Properties, geometry_type: &str) -> Block {
    const NOTHING: Block = Block::Clear(0.0);
    let layer = |name: &str| is(p, "layerName", name);
    // `String(p.class ?? "")`
    let cls = match p.get("class") {
        None | Some(Value::Null) => String::new(),
        v => js_string_of(v).into_owned(),
    };
    let polygon = geometry_type.ends_with("Polygon");
    if layer("building") || layer("buildings") || layer("water") {
        return if polygon { Block::Fill } else { NOTHING };
    }
    // Streets GL's tiles: car parks, pitches and the like; roads and railways their own width and a little
    if layer("common") {
        return if polygon && SGL_BARE.contains(&&*js_string_of(p.get("type"))) { Block::Fill } else { NOTHING };
    }
    if layer("highways") {
        if polygon {
            return Block::Fill;
        }
        if is(p, "type", "railway") {
            return Block::Clear(3.5);
        }
        return match sgl_path(p) {
            Some(path) => Block::Clear(path.width.unwrap_or(2.0) / 2.0 + 1.5),
            None => NOTHING,
        };
    }
    if layer("landuse") {
        return if polygon && (cls == "railway" || cls == "pitch" || cls == "playground") { Block::Fill } else { NOTHING };
    }
    if layer("transportation") {
        if cls.ends_with("_construction") || cls == "ferry" {
            return NOTHING;
        }
        if polygon {
            return Block::Fill;
        }
        return Block::Clear(clearance(&cls).unwrap_or(3.0));
    }
    NOTHING
}

// --------------------------------------------------------------- rasterising

/// Every cell of `mask` whose centre is inside the polygon (even-odd over all its rings), set to
/// `value`.
fn fill_polygon(mask: &mut [u8], rings: &[Vec<Point>], value: u8) {
    let g = GRID as f64;
    let mut edges: Vec<[f64; 4]> = Vec::new();
    let mut top = f64::INFINITY;
    let mut bottom = f64::NEG_INFINITY;
    for ring in rings {
        let mut j = ring.len().wrapping_sub(1);
        for i in 0..ring.len() {
            let (x0, y0) = (ring[j][0] * g, ring[j][1] * g);
            let (x1, y1) = (ring[i][0] * g, ring[i][1] * g);
            j = i;
            if y0 == y1 {
                continue;
            }
            edges.push(if y0 < y1 { [x0, y0, x1, y1] } else { [x1, y1, x0, y0] });
            top = js_min(js_min(top, y0), y1);
            bottom = js_max(js_max(bottom, y0), y1);
        }
    }
    let from = js_max(0.0, (top - 0.5).floor());
    let to = js_min(g - 1.0, (bottom - 0.5).ceil());
    let mut crossings: Vec<f64> = Vec::new();
    let mut row = from;
    while row <= to {
        let y = row + 0.5;
        crossings.clear();
        for &[x0, y0, x1, y1] in &edges {
            if y >= y0 && y < y1 {
                crossings.push(x0 + ((y - y0) / (y1 - y0)) * (x1 - x0));
            }
        }
        crossings.sort_by(f64::total_cmp);
        let line = row * g;
        let mut k = 0;
        while k + 1 < crossings.len() {
            let a = js_max(0.0, (crossings[k] - 0.5).ceil());
            let b = js_min(g - 1.0, (crossings[k + 1] - 0.5).floor());
            let mut col = a;
            while col <= b {
                mask[(line + col) as usize] = value;
                col += 1.0;
            }
            k += 2;
        }
        row += 1.0;
    }
}

/// Every cell within `radius` cells of the line, set to `value`.
fn stroke_line(mask: &mut [u8], line: &[Point], radius: f64, value: u8) {
    let g = GRID as f64;
    let r2 = radius * radius;
    for i in 1..line.len() {
        let (ax, ay) = (line[i - 1][0] * g, line[i - 1][1] * g);
        let (bx, by) = (line[i][0] * g, line[i][1] * g);
        let (dx, dy) = (bx - ax, by - ay);
        // `dx * dx + dy * dy || 1`
        let len2 = match dx * dx + dy * dy {
            l if l == 0.0 || l.is_nan() => 1.0,
            l => l,
        };
        let x0 = js_max(0.0, (js_min(ax, bx) - radius).floor());
        let x1 = js_min(g - 1.0, (js_max(ax, bx) + radius).ceil());
        let y0 = js_max(0.0, (js_min(ay, by) - radius).floor());
        let y1 = js_min(g - 1.0, (js_max(ay, by) + radius).ceil());
        let mut row = y0;
        while row <= y1 {
            let py = row + 0.5;
            let mut col = x0;
            while col <= x1 {
                let px = col + 0.5;
                let t = js_min(1.0, js_max(0.0, ((px - ax) * dx + (py - ay) * dy) / len2));
                let (ex, ey) = (ax + t * dx - px, ay + t * dy - py);
                if ex * ex + ey * ey <= r2 {
                    mask[(row * g + col) as usize] = value;
                }
                col += 1.0;
            }
            row += 1.0;
        }
    }
}

/// A feature's polygons, each a list of rings.
fn polygons_of(g: &Geometry) -> &[Vec<Vec<Point>>] {
    match g {
        Geometry::Polygon(rings) => std::slice::from_ref(rings),
        Geometry::MultiPolygon(polygons) => polygons,
        _ => &[],
    }
}

fn lines_of(g: &Geometry) -> &[Vec<Point>] {
    match g {
        Geometry::LineString(line) => std::slice::from_ref(line),
        Geometry::MultiLineString(lines) => lines,
        _ => &[],
    }
}

// --------------------------------------------------------------- planting

/// A hash of two integers (and a salt), 0 to 1. Each is taken as JavaScript's `x | 0` does
/// (truncated, wrapped to 32 bits, NaN and infinities 0).
pub fn hash2(x: f64, y: f64, salt: f64) -> f64 {
    let [x, y, salt] = [x, y, salt].map(|v| to_int32(v) as u32);
    // `Math.imul`: the low 32 bits of the product, the same bits signed or not
    let mut h = x.wrapping_mul(0x27d4_eb2d) ^ y.wrapping_mul(0x1656_67b1) ^ salt.wrapping_mul(0x9e37_79b1);
    h = (h ^ (h >> 15)).wrapping_mul(0x85eb_ca6b);
    h = (h ^ (h >> 13)).wrapping_mul(0xc2b2_ae35);
    f64::from(h ^ (h >> 16)) / 4_294_967_296.0
}

/// How far through the year the leaves are, for broadleaves: shares of each look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeafPhase {
    pub spring: f64,
    pub summer: f64,
    pub autumn: f64,
    pub bare: f64,
}

const DAY_MS: f64 = 86_400_000.0;

/// Howard Hinnant's `days_from_civil`: days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The year of a day since 1970-01-01 (Hinnant's `civil_from_days`).
fn year_of_day(days: i64) -> i64 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    yoe + era * 400 + i64::from(mp >= 10)
}

/// `new Date(ms).getTime()` (TimeClip: whole milliseconds, NaN past ±8.64e15) and
/// `Date.UTC(date.getUTCFullYear(), 0, 1)`.
fn time_and_year_start(ms: f64) -> (f64, f64) {
    if !ms.is_finite() || ms.abs() > 8.64e15 {
        return (f64::NAN, f64::NAN);
    }
    let time = ms.trunc() + 0.0;
    let year = year_of_day((time / DAY_MS).floor() as i64);
    (time, days_from_civil(year, 1, 1) as f64 * DAY_MS)
}

/// How far through the year the leaves are, for broadleaves at `lat` on the day of `date_ms`
/// (milliseconds since the epoch): shares of each look.
pub fn leaf_phase(date_ms: f64, lat: f64) -> LeafPhase {
    // the tropics are green all year
    if lat.abs() < 23.0 {
        return LeafPhase { spring: 0.0, summer: 1.0, autumn: 0.0, bare: 0.0 };
    }
    let (time, start) = time_and_year_start(date_ms);
    let mut day = (time - start) / DAY_MS;
    // the southern hemisphere's seasons are half a year on
    if lat < 0.0 {
        day = (day + 182.5) % 365.0;
    }
    // spring later and autumn earlier further from the equator: about four days a degree from 56°
    let shift = (lat.abs() - 56.0) * 4.0;
    let leaf_out = 118.0 + shift;
    let turn = 262.0 - shift;
    let fall = 300.0 - shift * 0.8;
    let ramp = |x: f64, a: f64, b: f64| js_min(1.0, js_max(0.0, (x - a) / (b - a)));
    let leaves = ramp(day, leaf_out - 8.0, leaf_out + 8.0) * (1.0 - ramp(day, fall, fall + 18.0));
    let autumn = ramp(day, turn - 10.0, turn + 14.0) * leaves;
    let spring = (1.0 - ramp(day, leaf_out + 10.0, leaf_out + 34.0)) * leaves * (1.0 - autumn);
    let summer = js_max(0.0, leaves - autumn - spring);
    LeafPhase { spring, summer, autumn, bare: 1.0 - leaves }
}

type Rgb = [f64; 3];
/// Streets GL's trees, by their layer in the textures (realistic-textures.ts): fir, and the
/// broadleaves.
const FIR: usize = 1;
const BROADLEAVES: [usize; 4] = [0, 2, 3, 4];
/// How much larger than the tree its card is, by layer (Streets GL's, for each picture's margin).
pub const CARD_SCALE: [f64; 5] = [1.35, 1.06, 1.19, 1.02, 1.43];
/// A tree shown as its picture is: grey, which the brightness varies (220 is as it is).
const SUMMER_AS_DRAWN: Rgb = [220.0, 220.0, 220.0];
const SPRING: [Rgb; 3] = [[104.0, 146.0, 62.0], [118.0, 158.0, 70.0], [96.0, 136.0, 58.0]];
const AUTUMN: [Rgb; 6] = [
    [196.0, 150.0, 46.0],
    [204.0, 118.0, 40.0],
    [170.0, 72.0, 38.0],
    [150.0, 138.0, 52.0],
    [104.0, 108.0, 46.0],
    [214.0, 168.0, 60.0],
];
const BARE: [Rgb; 3] = [[112.0, 98.0, 86.0], [98.0, 88.0, 80.0], [122.0, 108.0, 94.0]];

/// One kind of tree's instances in a tile, shuffled so any first N of them are an even thinning.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Instances {
    /// `[lon, lat, 0]` each.
    pub positions: Vec<f64>,
    /// The mesh's 3×3 transform (column-major) and translation, 12 floats each
    /// (`SimpleMeshLayer`'s own layout).
    pub matrices: Vec<f32>,
    /// RGBA each; the alpha says which picture, and whether the colour replaces its own
    /// (`200 + species·10 + 5 if retinted`).
    pub colors: Vec<u8>,
}

impl Instances {
    /// How many trees (`length`).
    pub fn len(&self) -> usize {
        self.positions.len() / 3
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    fn with_capacity(n: usize) -> Self {
        Instances { positions: Vec::with_capacity(n * 3), matrices: Vec::with_capacity(n * 12), colors: Vec::with_capacity(n * 4) }
    }
}

/// A tile's trees (the two lists are drawn alike; the split is historical).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TileTrees {
    pub conifers: Instances,
    pub broadleaves: Instances,
}

/// What else is known of a tile's ground: its land cover (landcover.rs) and how tall its trees
/// really are (canopy.rs), each to the tile's own corners, and the towns' registered trees in it
/// (street_trees.rs).
#[derive(Debug, Clone, Copy, Default)]
pub struct Surroundings<'a> {
    pub land: Option<&'a LandGrid>,
    pub canopy: Option<&'a CanopyGrid>,
    pub registered: &'a [RegisteredTree],
}

/// Covers a canopy's heights place trees in, where they really are: residential streets to
/// cemeteries, and woods.
const PLACED_BY_CANOPY: [u8; 9] = [1, 2, 3, 4, 5, 6, 10, 11, 12];
/// Where the canopy is, the trees are sought this far apart, metres (closer than any cover plants
/// them).
const CANOPY_STEP_M: f64 = 7.0;
/// A tree is taken to stand where the canopy is this tall, metres (a wood's are left out lower:
/// clearings).
const CANOPY_TREE_M: f64 = 3.0;
const CANOPY_WOOD_M: f64 = 2.0;
/// Unmapped land with this tall a canopy grows trees, metres.
const CANOPY_UNMAPPED_M: f64 = 4.0;
/// No other tree within this many metres of a town's registered one (it is that tree); a mapped
/// one, this many.
const REGISTERED_CLEAR_M: f64 = 4.5;
const REGISTERED_MAPPED_M: f64 = 3.0;
/// The salts of the trees the canopy or the land cover alone place, on unmapped land.
const UNMAPPED_CANOPY: f64 = 20.0;
const UNMAPPED_WOOD: f64 = 21.0;
/// A row of mapped trees has one about every this many metres.
const ROW_M: f64 = 10.0;

/// A tree as it is planted.
struct Tree {
    x: f64,
    y: f64,
    rank: f64,
    conifer: bool,
    height: f64,
    width: f64,
    yaw: f64,
    colour: Rgb,
    species: usize,
    /// Its colour is the season's in place of the picture's own (spring, autumn, bare).
    retint: bool,
}

/// A tree at `x, y` (the tile's 0 to 1) of these heights and widths, its own choices from `h`; a
/// conifer, and which, if not `h`'s (`grow`).
#[allow(clippy::too_many_arguments, reason = "the TypeScript closure's eight parameters, kept as they are")]
fn grow(
    trees: &mut Vec<Tree>,
    leaves: &LeafPhase,
    [x, y]: Point,
    h: &dyn Fn(f64) -> f64,
    conifers: f64,
    height: [f64; 2],
    wide: [f64; 2],
    rank: f64,
    kind: Option<usize>,
) {
    let conifer = match kind {
        None => h(3.0) < conifers,
        Some(kind) => kind == FIR,
    };
    let mut colour = SUMMER_AS_DRAWN;
    let mut width = wide[0] + (wide[1] - wide[0]) * h(5.0);
    let species = kind.unwrap_or_else(|| if conifer { FIR } else { BROADLEAVES[(h(11.0) * BROADLEAVES.len() as f64).floor() as usize] });
    let mut retint = false;
    if !conifer {
        // each tree turns in its own time: pick its look by the season's shares
        let r = h(7.0);
        let pick = |list: &[Rgb]| list[(h(6.0) * list.len() as f64).floor() as usize];
        // (in summer, the picture's own green)
        retint = r < leaves.bare + leaves.autumn + leaves.spring;
        if r < leaves.bare {
            colour = pick(&BARE);
            width *= 0.82;
        } else if r < leaves.bare + leaves.autumn {
            colour = pick(&AUTUMN);
        } else if retint {
            colour = pick(&SPRING);
        }
    }
    // a little variety in brightness, tree to tree
    let v = 0.88 + 0.24 * h(8.0);
    let colour = colour.map(|ch| js_min(255.0, js_round(ch * v)));
    let tall = height[0] + (height[1] - height[0]) * h(4.0);
    trees.push(Tree { x, y, rank, conifer, height: tall, width, yaw: h(10.0) * 2.0 * PI, colour, species, retint });
}

/// `Number(v)` (`None` is `undefined`).
fn js_number(v: Option<&Value>) -> f64 {
    match v {
        None | Some(Value::Object(_)) => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        // a list is read as its `String`
        Some(Value::String(_) | Value::Array(_)) => string_to_number(&js_string_of(v)),
    }
}

/// JavaScript's `StringToNumber`: blanks trimmed, empty is 0, `Infinity`, `0x`/`0o`/`0b`
/// integers, else a decimal literal or NaN (Rust's parser alone would also take `inf`, `nan`).
/// The same as `overture.rs`'s and `ground.rs`'s private copies; one belongs in `core::num`.
fn string_to_number(s: &str) -> f64 {
    let blank = |c: char| {
        matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}')
            || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
    };
    let s = s.trim_matches(blank);
    match s {
        "" => return 0.0,
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let radix = match s.get(..2) {
        Some("0x" | "0X") => 16,
        Some("0o" | "0O") => 8,
        Some("0b" | "0B") => 2,
        _ => 10,
    };
    if radix != 10 {
        let digits = &s[2..];
        if digits.is_empty() {
            return f64::NAN;
        }
        return digits
            .chars()
            .try_fold(0.0, |n: f64, c| c.to_digit(radix).map(|d| n * f64::from(radix) + f64::from(d)))
            .unwrap_or(f64::NAN);
    }
    if !s.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E')) {
        return f64::NAN;
    }
    s.parse().unwrap_or(f64::NAN)
}

/// `for (let i = from; i < to; i++)` over a grid's whole cells, each as a number.
fn grid_range(from: f64, to: f64) -> impl Iterator<Item = f64> {
    // (`as` reads NaN as 0: no cells, as the TypeScript's loop then has none)
    (from as i64..to as i64).map(|i| i as f64)
}

/// A tile's trees from its decoded features (in the tile's own coordinates: 0 to 1 across, y
/// down), on the day of `date_ms` (milliseconds since the epoch), with what else is known of its
/// ground.
pub fn plant_tile(features: &[Feature], tile: TileIndex, date_ms: f64, around: &Surroundings<'_>) -> TileTrees {
    let mut cover = vec![0u8; GRID * GRID];
    let mut blocked = vec![0u8; GRID * GRID];
    let g = GRID as f64;
    let n = 2f64.powi(tile.z as i32);
    let (tx, ty) = (f64::from(tile.x), f64::from(tile.y));
    let lat_at = |y: f64| (PI * (1.0 - (2.0 * (ty + y)) / n)).sinh().atan() / DEG;
    let mid_lat = lat_at(0.5);
    let tile_metres = (EARTH_CIRCUMFERENCE * (mid_lat * DEG).cos()) / n;
    let cell_metres = tile_metres / g;

    // what grows where: lower codes first, so woods win over the parks and streets they lie in
    let mut covered: Vec<(&Feature, u8)> = features.iter().map(|f| (f, cover_of(&f.properties))).filter(|&(_, code)| code > 0).collect();
    covered.sort_by_key(|&(_, code)| code);
    // (a set in the order first met: the codes rising)
    let mut present: Vec<u8> = Vec::new();
    for &(f, code) in &covered {
        for polygon in polygons_of(&f.geometry) {
            fill_polygon(&mut cover, polygon, code);
        }
        if !present.contains(&code) {
            present.push(code);
        }
    }
    // mapped trees, one by one, and in rows (Streets GL's tiles)
    let mapped: Vec<&Feature> = features
        .iter()
        .filter(|f| (is(&f.properties, "layerName", "point") && is(&f.properties, "type", "tree")) || is(&f.properties, "type", "treeRow"))
        .collect();
    let Surroundings { land, canopy: heights, registered } = *around;
    if present.is_empty() && mapped.is_empty() && heights.is_none() && land.is_none() && registered.is_empty() {
        return TileTrees::default();
    }
    // where nothing may
    for f in features {
        match block_of(&f.properties, f.geometry.type_name()) {
            Block::Fill => {
                for polygon in polygons_of(&f.geometry) {
                    fill_polygon(&mut blocked, polygon, 1);
                }
            }
            Block::Clear(m) if m > 0.0 => {
                for line in lines_of(&f.geometry) {
                    stroke_line(&mut blocked, line, m / cell_metres, 1);
                }
            }
            Block::Clear(_) => {}
        }
    }

    // conifers: a third in the south of Sweden, three quarters in the north; the same pattern elsewhere by latitude
    let wood_conifers = js_min(0.8, js_max(0.15, 0.35 + (mid_lat.abs() - 56.0) * 0.12));
    // A wood's share of conifers at `x, y`: as its land cover says, else `fallback`.
    let conifers_at = |x: f64, y: f64, fallback: f64| {
        let c = land.map_or(lc::NONE, |land| class_at(land, x, y));
        match c {
            lc::CONIFERS => 0.95,
            lc::MIXED => 0.5,
            lc::BROADLEAVES => 0.05,
            _ => fallback,
        }
    };
    // the canopy's pixels, and how many of them make two metres (a crown's middle may miss the point)
    let near = heights.map_or(0.0, |h| js_max(0.0, js_round(2.0 / (tile_metres / h.size as f64))));
    let canopy_at = |x: f64, y: f64| heights.map_or(0.0, |h| f64::from(tallest_near(h, x, y, near as u32)));
    // A tree's height from the canopy where it is (a little under it, as the canopy is its tallest), else its cover's.
    let tall_at = |x: f64, y: f64, h: f64, fallback: [f64; 2]| -> [f64; 2] {
        let top = canopy_at(x, y);
        if top < CANOPY_TREE_M {
            return fallback;
        }
        let t = js_min(45.0, top * (0.86 + 0.14 * h));
        [t, t]
    };
    let leaves = leaf_phase(date_ms, mid_lat);
    let mut trees: Vec<Tree> = Vec::new();
    // the towns' registered trees, each where it stands, of its kind, its crown as wide as recorded
    // (kept in cells about 5 m across, to find them by)
    let mut taken: HashMap<i64, Vec<Point>> = HashMap::new();
    let cells = js_max(1.0, js_round(tile_metres / REGISTERED_CLEAR_M));
    let cell_of = |x: f64, y: f64| ((y * cells).floor() * cells + (x * cells).floor()) as i64;
    for (k, t) in registered.iter().enumerate() {
        if t.x < 0.0 || t.x >= 1.0 || t.y < 0.0 || t.y >= 1.0 {
            continue;
        }
        let (gx, gy) = (js_round((tx + t.x) * 65536.0), js_round((ty + t.y) * 65536.0));
        let h = |salt: f64| hash2(gx, gy, 1301.0 + salt);
        let which = match t.kind {
            register_kinds::CONIFER => Some(FIR),
            register_kinds::BEECH => Some(0),
            register_kinds::LINDEN => Some(if h(12.0) < 0.5 { 2 } else { 3 }),
            register_kinds::OAK => Some(4),
            _ => None,
        };
        // as tall as the canopy has it there, else as its crown suggests; as wide as its crown (the picture's crown about three quarters of it)
        let from_crown = js_min(28.0, js_max(3.0, t.crown * 1.5 + 1.0));
        let tall = tall_at(t.x, t.y, h(4.0), if t.crown > 0.0 { [from_crown, from_crown] } else { [7.0, 14.0] });
        let across = js_min(1.2, js_max(0.75, t.crown / (((tall[0] + tall[1]) / 2.0) * 0.75)));
        let wide = if t.crown > 0.0 { [across, across] } else { [0.9, 1.2] };
        let conifers = if t.kind == register_kinds::CONIFER { 1.0 } else { 0.0 };
        grow(&mut trees, &leaves, [t.x, t.y], &h, conifers, tall, wide, -1.0 - k as f64 * 1e-9, which);
        taken.entry(cell_of(t.x, t.y)).or_default().push([t.x, t.y]);
    }
    // Whether a registered tree stands within `m` metres.
    let near_registered = |x: f64, y: f64, m: f64| {
        if taken.is_empty() {
            return false;
        }
        let r = m / tile_metres;
        let (cx, cy) = ((x * cells).floor(), (y * cells).floor());
        let mut j = js_max(0.0, cy - 1.0);
        while j <= js_min(cells - 1.0, cy + 1.0) {
            let mut i = js_max(0.0, cx - 1.0);
            while i <= js_min(cells - 1.0, cx + 1.0) {
                for &[tx, ty] in taken.get(&((j * cells + i) as i64)).map_or(&[][..], Vec::as_slice) {
                    if js_hypot2(tx - x, ty - y) < r {
                        return true;
                    }
                }
                i += 1.0;
            }
            j += 1.0;
        }
        false
    };
    // a candidate's mask cell, once it is known to be in the tile
    let cell_at = |x: f64, y: f64| ((y * g).floor() * g + (x * g).floor()) as usize;
    for &code in &present {
        let c = &COVERS[usize::from(code)];
        // where the canopy says, the trees where they really stand: sought closer together, each where it is tall enough
        let placed = heights.is_some() && PLACED_BY_CANOPY.contains(&code);
        let wood = code >= 10;
        let chance = if placed && !wood { 1.0 } else { c.chance };
        let step = (if placed && !wood { CANOPY_STEP_M } else { c.spacing }) / tile_metres;
        // this cover's grid, in the world's tile units at this zoom: cells whose middle falls in the tile
        let (i0, i1) = ((tx / step).floor(), ((tx + 1.0) / step).ceil());
        let (j0, j1) = ((ty / step).floor(), ((ty + 1.0) / step).ceil());
        let jitter = if code == ORCHARD { 0.08 } else { 0.7 };
        let salt = f64::from(code) * 16.0;
        for j in grid_range(j0, j1) {
            for i in grid_range(i0, i1) {
                let h = |s: f64| hash2(i, j, salt + s);
                if h(0.0) >= chance {
                    continue;
                }
                let x = (i + 0.5 + jitter * (h(1.0) - 0.5)) * step - tx;
                let y = (j + 0.5 + jitter * (h(2.0) - 0.5)) * step - ty;
                if !(0.0..1.0).contains(&x) || !(0.0..1.0).contains(&y) {
                    continue;
                }
                let cell = cell_at(x, y);
                if cover[cell] != code || blocked[cell] != 0 {
                    continue;
                }
                if placed && canopy_at(x, y) < (if wood { CANOPY_WOOD_M } else { CANOPY_TREE_M }) {
                    continue;
                }
                if near_registered(x, y, REGISTERED_CLEAR_M) {
                    continue;
                }
                let conifers = c.conifers.unwrap_or_else(|| conifers_at(x, y, wood_conifers));
                let height = if placed { tall_at(x, y, h(4.0), c.height) } else { c.height };
                grow(&mut trees, &leaves, [x, y], &h, conifers, height, c.width, h(9.0), None);
            }
        }
    }
    // unmapped land: trees where the canopy has them (else where the land cover has woods), of the land's kind
    if heights.is_some() || land.is_some() {
        let by_canopy = heights.is_some();
        let salt = if by_canopy { UNMAPPED_CANOPY } else { UNMAPPED_WOOD } * 16.0;
        let wood = &COVERS[10];
        let step = (if by_canopy { CANOPY_STEP_M } else { wood.spacing }) / tile_metres;
        let (i0, i1) = ((tx / step).floor(), ((tx + 1.0) / step).ceil());
        let (j0, j1) = ((ty / step).floor(), ((ty + 1.0) / step).ceil());
        for j in grid_range(j0, j1) {
            for i in grid_range(i0, i1) {
                let h = |s: f64| hash2(i, j, salt + s);
                if !by_canopy && h(0.0) >= wood.chance {
                    continue;
                }
                let x = (i + 0.5 + 0.7 * (h(1.0) - 0.5)) * step - tx;
                let y = (j + 0.5 + 0.7 * (h(2.0) - 0.5)) * step - ty;
                if !(0.0..1.0).contains(&x) || !(0.0..1.0).contains(&y) {
                    continue;
                }
                let cell = cell_at(x, y);
                if cover[cell] != 0 || blocked[cell] != 0 {
                    continue;
                }
                let kind = land.map_or(lc::NONE, |land| class_at(land, x, y));
                if kind == lc::WATER || kind == lc::ARABLE {
                    continue;
                }
                if near_registered(x, y, REGISTERED_CLEAR_M) {
                    continue;
                }
                if by_canopy {
                    if canopy_at(x, y) < CANOPY_UNMAPPED_M {
                        continue;
                    }
                    // in a wood as the land cover says; else a garden's or a street's tree
                    let forest = wooded(kind);
                    let conifers = conifers_at(x, y, if forest { wood_conifers } else { 0.2 });
                    let height = tall_at(x, y, h(4.0), wood.height);
                    grow(&mut trees, &leaves, [x, y], &h, conifers, height, if forest { wood.width } else { [0.9, 1.25] }, h(9.0), None);
                } else if wooded(kind) {
                    grow(&mut trees, &leaves, [x, y], &h, conifers_at(x, y, wood_conifers), wood.height, wood.width, h(9.0), None);
                }
            }
        }
    }
    // each mapped tree where it is, of its kind (any broadleaf where it doesn't say); a row's every 10 m or so
    for f in mapped {
        let p = &f.properties;
        let sort = sgl_tree(p);
        let height = js_number(p.get("height"));
        let mapped_tall = if height > 0.0 { height } else { 0.0 };
        let mut at = |x: f64, y: f64, k: f64| {
            if !(0.0..1.0).contains(&x) || !(0.0..1.0).contains(&y) {
                return;
            }
            if near_registered(x, y, REGISTERED_MAPPED_M) {
                return;
            }
            let (gx, gy) = (js_round((tx + x) * 65536.0), js_round((ty + y) * 65536.0));
            let h = |salt: f64| hash2(gx, gy, 977.0 + k * 31.0 + salt);
            // (a linden either of Streets GL's two; a broadleaf not said, any of them)
            let which = match sort {
                TreeKind::Fir => Some(FIR),
                TreeKind::Beech => Some(0),
                TreeKind::Linden => Some(if h(12.0) < 0.5 { 2 } else { 3 }),
                TreeKind::Oak => Some(4),
                TreeKind::Broadleaf => None,
            };
            // as tall as mapped, else as the canopy has it there
            let tall = if mapped_tall != 0.0 { [mapped_tall, mapped_tall] } else { tall_at(x, y, h(4.0), [7.0, 14.0]) };
            grow(&mut trees, &leaves, [x, y], &h, 0.0, tall, [0.9, 1.2], -1.0, which);
        };
        if let Geometry::Point([x, y]) = f.geometry {
            at(x, y, 0.0);
        } else {
            for line in lines_of(&f.geometry) {
                let mut carry = 0.0;
                for i in 1..line.len() {
                    let ([x0, y0], [x1, y1]) = (line[i - 1], line[i]);
                    let metres = js_hypot2(x1 - x0, y1 - y0) * tile_metres;
                    let mut d = carry;
                    while d < metres {
                        at(x0 + ((x1 - x0) * d) / metres, y0 + ((y1 - y0) * d) / metres, i as f64);
                        d += ROW_M;
                    }
                    carry = (carry - metres) % ROW_M;
                    if carry < 0.0 {
                        carry += ROW_M;
                    }
                }
            }
        }
    }
    // shuffled (by a hash, so the same every time) and thinned to the most a tile may have
    trees.sort_by(|a, b| a.rank.total_cmp(&b.rank));
    trees.truncate(MAX_TREES);

    let lon_at = |x: f64| ((tx + x) / n) * 360.0 - 180.0;
    let build = |conifers: bool| {
        let list: Vec<&Tree> = trees.iter().filter(|t| t.conifer == conifers).collect();
        let mut out = Instances::with_capacity(list.len());
        for t in list {
            out.positions.extend_from_slice(&[lon_at(t.x), lat_at(t.y), 0.0]);
            let (s, c) = (t.yaw.sin(), t.yaw.cos());
            // the card a little larger than the tree, as Streets GL has each picture's margin
            let tall = t.height * CARD_SCALE[t.species];
            let across = tall * t.width;
            // turned by its yaw, as wide as it is, as tall
            let m = [c * across, s * across, 0.0, -s * across, c * across, 0.0, 0.0, 0.0, tall, 0.0, 0.0, 0.0];
            out.matrices.extend(m.map(|v| v as f32));
            // the alpha says which picture, and whether the colour replaces its own (realistic.ts reads it)
            let alpha = 200 + t.species * 10 + if t.retint { 5 } else { 0 };
            out.colors.extend_from_slice(&[t.colour[0] as u8, t.colour[1] as u8, t.colour[2] as u8, alpha as u8]);
        }
        out
    };
    TileTrees { conifers: build(true), broadleaves: build(false) }
}

// --------------------------------------------------------------- tiles

/// The layers read of OpenFreeMap's tiles, and of Streets GL's.
pub const TREE_LAYERS: [&str; 5] = ["landcover", "landuse", "water", "building", "transportation"];
pub const SGL_TREE_LAYERS: [&str; 6] = ["natural", "common", "water", "buildings", "highways", "point"];
/// Planted tiles kept; the least recently used go once there are more than this.
pub const MAX_CACHED: usize = 256;
/// At most this many tiles loading at once.
pub const MAX_LOADING: usize = 4;

/// A tile near a point, and how far its nearest point is, metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileAround {
    pub tile: TileIndex,
    pub distance: f64,
}

/// The tiles at `zoom` ([`TREE_TILE_ZOOM`], or Streets GL's) within `radius` metres of
/// `lon, lat`, nearest first, with their distances.
pub fn tiles_around(lon: f64, lat: f64, radius: f64, zoom: u32) -> Vec<TileAround> {
    let n = 2f64.powi(zoom as i32);
    let tx = ((lon + 180.0) / 360.0) * n;
    let ty = ((1.0 - ((lat * DEG).tan() + 1.0 / (lat * DEG).cos()).ln() / PI) / 2.0) * n;
    let tile_metres = (EARTH_CIRCUMFERENCE * (lat * DEG).cos()) / n;
    let span = radius / tile_metres;
    let mut out = Vec::new();
    let mut y = (ty - span).floor();
    while y <= (ty + span).floor() {
        let mut x = (tx - span).floor();
        while x <= (tx + span).floor() {
            if y >= 0.0 && y < n {
                // from the point to the nearest point of the tile
                let dx = js_max(js_max(x - tx, 0.0), tx - (x + 1.0));
                let dy = js_max(js_max(y - ty, 0.0), ty - (y + 1.0));
                let distance = js_hypot2(dx, dy) * tile_metres;
                if distance <= radius {
                    let tile = TileIndex { x: (((x % n) + n) % n) as u32, y: y as u32, z: zoom };
                    out.push(TileAround { tile, distance });
                }
            }
            x += 1.0;
        }
        y += 1.0;
    }
    // stable, as `sort((a, b) => a.distance - b.distance)`
    out.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    out
}

/// A layer's material (deck.gl's `material`): how much ambient and sun light it takes, and its
/// highlights.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    pub ambient: f64,
    pub diffuse: f64,
    pub shininess: f64,
    pub specular_color: [u8; 3],
}

/// A shadow's colour as it is given: lit by nothing.
pub const SHADOW_MATERIAL: Material = Material { ambient: 1.0, diffuse: 0.0, shininess: 1.0, specular_color: [0, 0, 0] };
/// The trees', unless the map gives its own.
pub const TREE_MATERIAL: Material = Material { ambient: 0.55, diffuse: 0.55, shininess: 4.0, specular_color: [12, 14, 10] };

// --------------------------------------------------------------- shadows

/// Each tree's shadow with the sun towards `sun` (east, north, up): its cards laid flat on the
/// ground away from the sun, each point as far along as its height throws it (as a building's,
/// [`shadow_step`]), just over the ground. The layer cuts them to the pictures' shapes
/// (realistic.ts's `TreeShadowExtension`), so a tree's shadow is its own, from its trunk out.
/// None with the sun down. The conifers come first, then the broadleaves.
pub fn tree_shadows(trees: &TileTrees, sun: [f64; 3]) -> Option<Instances> {
    let step = shadow_step(sun)?;
    let mut out = Instances::with_capacity(trees.conifers.len() + trees.broadleaves.len());
    for kind in [&trees.conifers, &trees.broadleaves] {
        out.positions.extend_from_slice(&kind.positions);
        out.colors.extend_from_slice(&kind.colors);
        for m in kind.matrices.chunks_exact(12) {
            let m: [f64; 12] = std::array::from_fn(|k| f64::from(m[k]));
            // [x, y, z] to [x + z·east, y + z·north, 0] after the tree's own transform
            let flat = |c: usize| [m[c] + m[c + 2] * step[0], m[c + 1] + m[c + 2] * step[1], 0.0];
            let ([a, b, c], [d, e, f], [g, h, i]) = (flat(0), flat(3), flat(6));
            let sheared = [a, b, c, d, e, f, g, h, i, m[9] + m[11] * step[0], m[10] + m[11] * step[1], 0.05];
            out.matrices.extend(sheared.map(|v| v as f32));
        }
    }
    Some(out)
}

/// Which sun a tile's shadows were cast for (`treeLayers`' `"az,alt"`: azimuth and altitude to
/// half a degree); none when the sun is too low for them (`sun[2]` at most 0.02).
pub fn shadow_key(sun: [f64; 3]) -> Option<(i64, i64)> {
    if sun[2].is_nan() || sun[2] <= 0.02 {
        return None;
    }
    let az = js_round((sun[0].atan2(sun[1]) * 180.0) / PI * 2.0);
    let alt = js_round((js_min(1.0, sun[2]).asin() * 180.0) / PI * 2.0);
    Some((az as i64, alt as i64))
}

// --------------------------------------------------------------- the mesh (`tree-models.ts`)

/// The trees' mesh as Streets GL draws them (its tree.glb): two upright cards crossed at right
/// angles, a unit wide and a unit tall, standing on the ground, each showing a picture of a whole
/// tree. A non-indexed triangle list, 12 vertices. The shader lights a card's back as the same
/// picture seen from behind, so one quad a card does for both its sides (no culling).
#[derive(Debug, Clone, PartialEq)]
pub struct CardArrays {
    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
    pub tex_coords: Vec<f32>,
}

/// The cards: one across east–west (facing south), one across north–south (facing east).
pub fn card_arrays() -> CardArrays {
    let mut a = CardArrays { positions: Vec::new(), normals: Vec::new(), tex_coords: Vec::new() };
    let cards: [([f64; 3], [f64; 3]); 2] = [([1.0, 0.0, 0.0], [0.0, -1.0, 0.0]), ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0])];
    for (across, facing) in cards {
        // (the picture's top row first: v is 0 at the top of the tree)
        for [u, z] in [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0], [1.0, 1.0], [0.0, 1.0]] {
            a.positions.extend([across[0] * (u - 0.5), across[1] * (u - 0.5), z].map(|v: f64| v as f32));
            a.normals.extend(facing.map(|v| v as f32));
            a.tex_coords.extend([u, 1.0 - z].map(|v: f64| v as f32));
        }
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn props(v: serde_json::Value) -> Properties {
        match v {
            Value::Object(map) => map,
            _ => panic!("not an object"),
        }
    }

    /// `Date.UTC(year, month, day)`, `month` from 0.
    fn utc(year: i64, month: i64, day: i64) -> f64 {
        days_from_civil(year, month + 1, day) as f64 * DAY_MS
    }

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Vec<Point>> {
        vec![vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]]]
    }

    fn feature(geometry: Geometry, properties: serde_json::Value) -> Feature {
        Feature { geometry, properties: props(properties) }
    }

    /// A tile near Malmö.
    const TILE: TileIndex = TileIndex { x: 8783, y: 5133, z: 14 };

    /// The trees back in the tile's own coordinates, with their cards' heights.
    fn places(t: &TileTrees) -> Vec<[f64; 3]> {
        let n = 2f64.powi(14);
        [&t.conifers, &t.broadleaves]
            .into_iter()
            .flat_map(|k| {
                (0..k.len()).map(move |i| {
                    let x = ((k.positions[i * 3] + 180.0) / 360.0) * n - f64::from(TILE.x);
                    let r = (k.positions[i * 3 + 1] * PI) / 180.0;
                    let y = ((1.0 - (r.tan() + 1.0 / r.cos()).ln() / PI) / 2.0) * n - f64::from(TILE.y);
                    [x, y, f64::from(k.matrices[i * 12 + 8])]
                })
            })
            .collect()
    }

    // --------------------------------------------------------------- `describe("trees")`

    #[test]
    fn woods_parks_and_gardens_grow_trees_roads_buildings_and_water_keep_them_off() {
        assert!(cover_of(&props(json!({"layerName": "landcover", "class": "wood"}))) > 0);
        assert!(cover_of(&props(json!({"layerName": "landcover", "class": "grass", "subclass": "park"}))) > 0);
        assert_eq!(cover_of(&props(json!({"layerName": "landuse", "class": "industrial"}))), 0);
        // woods win over the parks and streets they lie in
        assert!(
            cover_of(&props(json!({"layerName": "landcover", "class": "wood"})))
                > cover_of(&props(json!({"layerName": "landuse", "class": "residential"})))
        );
        assert_eq!(block_of(&props(json!({"layerName": "building"})), "Polygon"), Block::Fill);
        assert_eq!(block_of(&props(json!({"layerName": "water", "class": "lake"})), "MultiPolygon"), Block::Fill);
        assert!(matches!(block_of(&props(json!({"layerName": "transportation", "class": "rail"})), "LineString"), Block::Clear(m) if m > 2.0));
        assert!(matches!(
            block_of(&props(json!({"layerName": "transportation", "class": "rail", "brunnel": "tunnel"})), "LineString"),
            Block::Clear(m) if m > 0.0
        ));
        assert_eq!(block_of(&props(json!({"layerName": "transportation", "class": "ferry"})), "LineString"), Block::Clear(0.0));
    }

    /// A square of wood with a railway straight through it, and a building in it.
    fn malmo_wood() -> Vec<Feature> {
        vec![
            feature(Geometry::Polygon(square(0.2, 0.2, 0.6, 0.6)), json!({"layerName": "landcover", "class": "wood"})),
            feature(Geometry::LineString(vec![[0.0, 0.4], [1.0, 0.4]]), json!({"layerName": "transportation", "class": "rail"})),
            feature(Geometry::Polygon(square(0.25, 0.25, 0.3, 0.3)), json!({"layerName": "building"})),
        ]
    }

    fn summer() -> f64 {
        utc(2026, 6, 1)
    }

    #[test]
    fn a_wood_is_planted_thick_inside_it_only_and_off_its_railway_and_building() {
        let planted = plant_tile(&malmo_wood(), TILE, summer(), &Surroundings::default());
        let all = places(&planted);
        // about 550 m square at 6 m apart: thousands of trees
        assert!(all.len() > 3000, "{}", all.len());
        for &[x, y, _] in &all {
            assert!(x > 0.199 && x < 0.601 && y > 0.199 && y < 0.601, "{x} {y}");
            assert!((y - 0.4).abs() > 0.001, "{y}");
            assert!(!(x > 0.25 && x < 0.3 && y > 0.25 && y < 0.3), "{x} {y}");
        }
        // as tall as a wood's trees (their cards up to Streets GL's margin taller)
        for &[_, _, height] in &all {
            assert!((13.0..=24.0 * 1.43).contains(&height), "{height}");
        }
    }

    #[test]
    fn the_same_trees_every_time() {
        let planted = plant_tile(&malmo_wood(), TILE, summer(), &Surroundings::default());
        let again = plant_tile(&malmo_wood(), TILE, summer(), &Surroundings::default());
        assert_eq!(again.conifers.positions, planted.conifers.positions);
        assert_eq!(again.broadleaves.colors, planted.broadleaves.colors);
    }

    #[test]
    fn broadleaves_green_in_summer_turning_in_autumn_bare_in_winter_the_tropics_always_green() {
        assert_eq!(leaf_phase(utc(2026, 6, 10), 56.0).summer, 1.0);
        let october = leaf_phase(utc(2026, 9, 16), 56.0);
        assert!(october.autumn > 0.5, "{october:?}");
        assert_eq!(leaf_phase(utc(2026, 0, 15), 56.0).bare, 1.0);
        // the southern hemisphere's January is summer
        assert_eq!(leaf_phase(utc(2026, 0, 15), -45.0).summer, 1.0);
        assert_eq!(leaf_phase(utc(2026, 0, 15), 5.0).summer, 1.0);
    }

    #[test]
    fn the_tiles_around_a_point_nearest_first() {
        let tiles = tiles_around(13.0, 55.605, 1500.0, TREE_TILE_ZOOM);
        assert_eq!(tiles[0].tile, TileIndex { x: 8783, y: 5133, z: 14 });
        assert_eq!(tiles[0].distance, 0.0);
        for i in 1..tiles.len() {
            assert!(tiles[i].distance >= tiles[i - 1].distance);
        }
        assert!(tiles.len() > 3);
    }

    #[test]
    fn the_cards_two_crossed_quads_a_unit_wide_and_tall_on_the_ground_the_pictures_top_at_the_top() {
        let a = card_arrays();
        assert_eq!(a.positions.len(), 2 * 6 * 3);
        assert_eq!(a.normals.len(), a.positions.len());
        assert_eq!(a.tex_coords.len(), (a.positions.len() / 3) * 2);
        let zs: Vec<f32> = a.positions.iter().skip(2).step_by(3).copied().collect();
        assert_eq!([zs.iter().copied().fold(f32::INFINITY, f32::min), zs.iter().copied().fold(f32::NEG_INFINITY, f32::max)], [0.0, 1.0]);
        for (k, z) in zs.iter().enumerate() {
            assert_eq!(a.tex_coords[k * 2 + 1], 1.0 - z);
        }
        // the second card is the first turned a quarter
        assert_eq!(a.normals[0..3], [0.0, -1.0, 0.0]);
        assert_eq!(a.normals[18..21], [1.0, 0.0, 0.0]);
    }

    // --------------------------------------------------------------- `describe("trees from the canopy and the land cover")`

    const SIZE: usize = 64;

    /// A canopy `m` metres tall over the square from `a` to `b` (of the tile), none elsewhere.
    fn canopy_over(a: f64, b: f64, m: u8) -> CanopyGrid {
        let mut heights = vec![0u8; SIZE * SIZE];
        let s = SIZE as f64;
        for j in 0..SIZE {
            for i in 0..SIZE {
                let (fi, fj) = (i as f64, j as f64);
                if fi >= a * s && fi < b * s && fj >= a * s && fj < b * s {
                    heights[j * SIZE + i] = m;
                }
            }
        }
        CanopyGrid { size: SIZE, heights }
    }

    fn land(c: u8) -> LandGrid {
        LandGrid { size: SIZE, classes: vec![c; SIZE * SIZE] }
    }

    #[test]
    fn on_unmapped_land_trees_stand_where_the_canopy_is_as_tall_as_it() {
        let (canopy, land) = (canopy_over(0.25, 0.5, 15), land(lc::BUILT));
        let planted = plant_tile(&[], TILE, summer(), &Surroundings { canopy: Some(&canopy), land: Some(&land), registered: &[] });
        let all = places(&planted);
        assert!(all.len() > 100, "{}", all.len());
        for &[x, y, h] in &all {
            assert!(x > 0.24 && x < 0.51 && y > 0.24 && y < 0.51, "{x} {y}");
            // (a little under the canopy's top; the card a margin taller)
            assert!(h > 12.0, "{h}");
            assert!(h < 15.0 * 1.43 + 0.01, "{h}");
        }
    }

    #[test]
    fn a_woods_clearings_are_left_open_and_its_trees_are_the_land_covers_kind() {
        let wood = [feature(Geometry::Polygon(square(0.0, 0.0, 1.0, 1.0)), json!({"layerName": "landcover", "class": "wood"}))];
        let (canopy, conifers) = (canopy_over(0.0, 0.5, 20), land(lc::CONIFERS));
        let planted = plant_tile(&wood, TILE, summer(), &Surroundings { canopy: Some(&canopy), land: Some(&conifers), registered: &[] });
        for [x, y, _] in places(&planted) {
            assert!(x < 0.51 && y < 0.51, "{x} {y}");
        }
        assert!(planted.conifers.len() > planted.broadleaves.len() * 8);
        let broadleaves = land(lc::BROADLEAVES);
        let broad = plant_tile(&wood, TILE, summer(), &Surroundings { land: Some(&broadleaves), ..Default::default() });
        assert!(broad.broadleaves.len() > broad.conifers.len() * 8);
    }

    #[test]
    fn without_a_canopy_unmapped_land_the_land_cover_calls_woods_is_planted_fields_and_water_arent() {
        let only = |land: &LandGrid, canopy: Option<&CanopyGrid>| {
            places(&plant_tile(&[], TILE, summer(), &Surroundings { land: Some(land), canopy, registered: &[] })).len()
        };
        assert!(only(&land(lc::MIXED), None) > 10_000);
        assert_eq!(only(&land(lc::ARABLE), None), 0);
        assert_eq!(only(&land(lc::WATER), Some(&canopy_over(0.0, 1.0, 20))), 0);
    }

    // --------------------------------------------------------------- `describe("a town's registered trees")`

    #[test]
    fn each_stands_where_it_is_registered_of_its_kind_and_crown_and_no_other_tree_crowds_it() {
        let park = [feature(Geometry::Polygon(square(0.0, 0.0, 1.0, 1.0)), json!({"layerName": "landuse", "class": "park"}))];
        let registered = [RegisteredTree { x: 0.5, y: 0.5, kind: 1, crown: 0.0 }, RegisteredTree { x: 0.25, y: 0.75, kind: 4, crown: 8.0 }];
        let planted = plant_tile(&park, TILE, summer(), &Surroundings { registered: &registered, ..Default::default() });
        assert!(!planted.conifers.is_empty());
        let n = 2f64.powi(14);
        let lon = ((f64::from(TILE.x) + 0.25) / n) * 360.0 - 180.0;
        let b = &planted.broadleaves;
        let oak = (0..b.len()).find(|&i| (b.positions[i * 3] - lon).abs() < 1e-9).expect("the oak");
        // the oak's picture (layer 4, the alpha's tens)
        assert_eq!((b.colors[oak * 4 + 3] - 200) / 10, 4);
        // nothing else within 4.5 m of it
        let (ox, oy) = (b.positions[oak * 3], b.positions[oak * 3 + 1]);
        let per_degree = 111_320.0;
        for k in [&planted.conifers, &planted.broadleaves] {
            for i in 0..k.len() {
                let (x, y) = (k.positions[i * 3], k.positions[i * 3 + 1]);
                if x == ox && y == oy {
                    continue;
                }
                let metres = js_hypot2((x - ox) * per_degree * ((oy * PI) / 180.0).cos(), (y - oy) * per_degree);
                assert!(metres > 4.4, "{metres}");
            }
        }
    }

    // --------------------------------------------------------------- new tests (no TypeScript twin)

    #[test]
    fn hash2_wraps_as_javascript_does() {
        // (inputs and outputs from the TypeScript, run in bun)
        let table: [(f64, f64, f64, f64); 20] = [
            (0.0, 0.0, 0.0, 0.0),
            (1.0, 0.0, 0.0, 0.503745433408767),
            (0.0, 1.0, 0.0, 0.0482882943470031),
            (0.0, 0.0, 1.0, 0.5421679092105478),
            (1.0, 2.0, 3.0, 0.3797727196943015),
            (-1.0, -1.0, -1.0, 0.0055536271538585424),
            (8783.0, 5133.0, 160.0, 0.2036265095230192),
            (2147483647.0, 2147483647.0, 7.0, 0.5900410932954401),
            (2147483648.0, 0.0, 0.0, 0.6314192181453109),
            (-2147483648.0, 5.0, 9.0, 0.44643431110307574),
            (4294967301.0, 3.0, 1.0, 0.7461076178587973),
            (575668224.0, 336363520.0, 1305.0, 0.09416385577060282),
            (1e10, -1e10, 12.0, 0.49085856252349913),
            (3.7, -3.7, 2.9, 0.8119229443836957),
            (f64::NAN, 1.0, 2.0, 0.8935814101714641),
            (f64::INFINITY, f64::NEG_INFINITY, 3.0, 0.3126044198870659),
            (123456.0, 654321.0, 1113.0, 0.814322148449719),
            (-98765.0, 43210.0, 336.0, 0.8931332116480917),
            (1099511640121.0, 8589934591.0, 2147483649.0, 0.9120694338344038),
            (65535.0, 65536.0, 65537.0, 0.6916276165284216),
        ];
        for (x, y, salt, want) in table {
            assert_eq!(hash2(x, y, salt), want, "hash2({x}, {y}, {salt})");
        }
    }

    #[test]
    fn leaf_phases_as_the_typescript_has_them() {
        // [date, lat, spring, summer, autumn, bare], from bun
        let table: [(f64, f64, [f64; 4]); 6] = [
            (utc(2026, 3, 28), 56.0, [0.4375, 0.0, 0.0, 0.5625]),
            (utc(2026, 4, 10), 56.0, [0.9583333333333334, 0.04166666666666663, 0.0, 0.0]),
            (utc(2026, 9, 1), 60.0, [0.0, 0.0, 1.0, 0.0]),
            (utc(2026, 8, 20), -40.0, [0.35416666666666663, 0.6458333333333334, 0.0, 0.0]),
            (utc(2024, 1, 29), 66.5, [0.0, 0.0, 0.0, 1.0]),
            (utc(2026, 4, 2), 45.0, [0.0, 1.0, 0.0, 0.0]),
        ];
        assert_eq!(utc(2026, 3, 28), 1_777_334_400_000.0);
        assert_eq!(utc(2024, 1, 29), 1_709_164_800_000.0);
        for (date, lat, [spring, summer, autumn, bare]) in table {
            assert_eq!(leaf_phase(date, lat), LeafPhase { spring, summer, autumn, bare }, "{date} {lat}");
        }
        // a date before the epoch, and none
        assert_eq!(leaf_phase(utc(1969, 6, 10), 56.0).summer, 1.0);
        assert!(leaf_phase(f64::NAN, 56.0).bare.is_nan());
    }

    #[test]
    fn the_tiles_around_as_the_typescript_finds_them_across_the_antimeridian_too() {
        let malmo = tiles_around(13.0, 55.605, 1500.0, TREE_TILE_ZOOM);
        let want = [
            (8783, 5133, 0.0),
            (8783, 5134, 202.80058655648583),
            (8784, 5133, 491.27987005422096),
            (8784, 5134, 531.4922281916706),
            (8782, 5133, 890.4447644721758),
            (8782, 5134, 913.2469307276995),
            (8783, 5132, 1178.924047969911),
            (8784, 5132, 1277.1913801784967),
            (8782, 5132, 1477.4145624900514),
        ];
        let got: Vec<(u32, u32, f64)> = malmo.iter().map(|t| (t.tile.x, t.tile.y, t.distance)).collect();
        assert_eq!(got, want);
        let wrapped = tiles_around(-179.99, 10.0, 3000.0, 16);
        assert_eq!(wrapped.len(), 99);
        let firsts: Vec<(u32, u32)> = wrapped[..4].iter().map(|t| (t.tile.x, t.tile.y)).collect();
        assert_eq!(firsts, [(1, 30938), (2, 30938), (1, 30937), (2, 30937)]);
        assert_eq!(wrapped[1].distance, 108.12947321554532);
        assert_eq!((wrapped[18].tile.x, wrapped[18].tile.y, wrapped[18].distance), (65535, 30938, 1096.2829759564029));
        assert_eq!((wrapped[98].tile.x, wrapped[98].tile.y, wrapped[98].distance), (65532, 30936, 2997.741564768919));
        assert!(wrapped.iter().all(|t| t.tile.z == 16));
    }

    /// The sums the bun script printed for one list of a planted tile, its first tree and the one
    /// halfway through.
    struct Digest {
        len: usize,
        positions: f64,
        matrices: f64,
        colours: u64,
        first: ([f64; 3], [f32; 12], [u8; 4]),
        mid: ([f64; 3], [f32; 12], [u8; 4]),
    }

    fn tree_at(k: &Instances, i: usize) -> ([f64; 3], [f32; 12], [u8; 4]) {
        (
            k.positions[i * 3..i * 3 + 3].try_into().unwrap(),
            k.matrices[i * 12..i * 12 + 12].try_into().unwrap(),
            k.colors[i * 4..i * 4 + 4].try_into().unwrap(),
        )
    }

    fn check(name: &str, k: &Instances, want: &Digest) {
        assert_eq!(k.len(), want.len, "{name}");
        assert_eq!(k.positions.iter().sum::<f64>(), want.positions, "{name}");
        assert_eq!(k.matrices.iter().map(|&v| f64::from(v)).sum::<f64>(), want.matrices, "{name}");
        assert_eq!(k.colors.iter().map(|&v| u64::from(v)).sum::<u64>(), want.colours, "{name}");
        if want.len > 0 {
            assert_eq!(tree_at(k, 0), want.first, "{name}");
            assert_eq!(tree_at(k, want.len / 2), want.mid, "{name}");
        }
    }

    /// A card's matrix from its three numbers (turned, and as tall), each a `Float32Array`'s as
    /// JavaScript prints it.
    fn m(c: f64, s: f64, tall: f64) -> [f32; 12] {
        [c, s, 0.0, -s, c, 0.0, 0.0, 0.0, tall, 0.0, 0.0, 0.0].map(|v| v as f32)
    }

    type Printed = ([f64; 2], [f64; 3], [u8; 4]);

    fn d(len: usize, positions: f64, matrices: f64, colours: u64, first: Printed, mid: Printed) -> Digest {
        let tree = |([lon, lat], [c, s, tall], colour): Printed| ([lon, lat, 0.0], m(c, s, tall), colour);
        Digest { len, positions, matrices, colours, first: tree(first), mid: tree(mid) }
    }

    #[test]
    fn planting_matches_the_typescript_tree_for_tree() {
        // (each tile planted by `tree-layer.ts` in bun: the sums of its lists, its first tree and
        // the one halfway through)
        let july = summer();
        let a = plant_tile(&malmo_wood(), TILE, july, &Surroundings::default());
        check("A conifers", &a.conifers, &d(2167, 148667.78836306883, 44182.21680020541, 1883250,
            ([12.99385720081537, 55.610781378489584], [-2.8413171768188477, 20.3322811126709, 20.973285675048828], [194, 194, 194, 210]),
            ([12.993068583912532, 55.611354658422194], [-13.236895561218262, 9.716923713684082, 16.084415435791016], [236, 236, 236, 210])));
        check("A broadleaves", &a.broadleaves, &d(4984, 341928.5506448981, 116406.39313558862, 4399459,
            ([12.995522998651182, 55.611260215441106], [-16.084346771240234, -24.58468246459961, 25.691125869750977], [234, 234, 234, 220]),
            ([12.997582009072346, 55.61175888826702], [17.71018409729004, 19.410490036010742, 30.213071823120117], [242, 242, 242, 240])));
        // in October: the same trees, the broadleaves turning
        let b = plant_tile(&malmo_wood(), TILE, utc(2026, 9, 12), &Surroundings::default());
        assert_eq!(b.conifers, a.conifers);
        check("B broadleaves", &b.broadleaves, &d(4984, 341928.5506448981, 116406.39313558862, 2848766,
            ([12.995522998651182, 55.611260215441106], [-16.084346771240234, -24.58468246459961, 25.691125869750977], [217, 125, 43, 225]),
            ([12.997582009072346, 55.61175888826702], [17.71018409729004, 19.410490036010742, 30.213071823120117], [224, 130, 44, 245])));

        let (canopy, built) = (canopy_over(0.25, 0.5, 15), land(lc::BUILT));
        let c = plant_tile(&[], TILE, july, &Surroundings { canopy: Some(&canopy), land: Some(&built), registered: &[] });
        check("C conifers", &c.conifers, &d(490, 33616.48372583245, 7627.092436991632, 426972,
            ([12.992478232858986, 55.611127688774324], [9.868474006652832, -13.8296537399292, 14.358661651611328], [231, 231, 231, 210]),
            ([12.995071567885645, 55.611449174353695], [14.136746406555176, -1.2887860536575317, 14.883925437927246], [239, 239, 239, 210])));
        check("C broadleaves", &c.broadleaves, &d(1945, 133436.7330852808, 34122.843879895285, 1719910,
            ([12.991780513228292, 55.61000625624004], [19.009748458862305, 0.07495763152837753, 19.83318519592285], [216, 216, 216, 200]),
            ([12.993608887380873, 55.6122740829153], [-7.000748634338379, -17.55806541442871, 16.732704162597656], [226, 226, 226, 220])));

        let whole = [feature(Geometry::Polygon(square(0.0, 0.0, 1.0, 1.0)), json!({"layerName": "landcover", "class": "wood"}))];
        let (canopy, conifers) = (canopy_over(0.0, 0.5, 20), land(lc::CONIFERS));
        let dd = plant_tile(&whole, TILE, july, &Surroundings { canopy: Some(&canopy), land: Some(&conifers), registered: &[] });
        check("D conifers", &dd.conifers, &d(10882, 746546.6927733298, 217717.99737154832, 9462255,
            ([12.995522998651182, 55.611260215441106], [-12.865358352661133, -19.664506912231445, 20.549516677856445], [234, 234, 234, 210]),
            ([12.994061855085079, 55.6106203413657], [13.672598838806152, -13.118112564086914, 20.757556915283203], [237, 237, 237, 210])));
        check("D broadleaves", &dd.broadleaves, &d(599, 41093.659674795934, 12805.618118301034, 527560,
            ([12.993095171456474, 55.61497524960962], [-25.306673049926758, 14.329708099365234, 26.26926040649414], [205, 205, 205, 240]),
            ([12.990719576415898, 55.61056313016395], [23.13614845275879, -3.9037721157073975, 24.898710250854492], [232, 232, 232, 200])));

        let mixed = land(lc::MIXED);
        let e = plant_tile(&[], TILE, july, &Surroundings { land: Some(&mixed), ..Default::default() });
        check("E conifers", &e.conifers, &d(11955, 820188.3689985037, 239115.56816184986, 10406397,
            ([13.00766792063095, 55.60673964784784], [-1.4280097484588623, 22.465084075927734, 20.301185607910156], [238, 238, 238, 210]),
            ([12.98953844056578, 55.61245709008109], [-10.03177261352539, 17.739011764526367, 22.414669036865234], [217, 217, 217, 210])));
        check("E broadleaves", &e.broadleaves, &d(12045, 826360.4710362055, 281126.92537660757, 10628090,
            ([13.003603144963023, 55.61067803536491], [-24.145458221435547, -10.778027534484863, 26.860395431518555], [203, 203, 203, 200]),
            ([12.996034679962463, 55.60812607127377], [3.9229676723480225, 31.882471084594727, 30.331798553466797], [213, 213, 213, 200])));

        // a park (no cover in OpenMapTiles' landuse) with five registered trees, one off the tile
        let park = [feature(Geometry::Polygon(square(0.0, 0.0, 1.0, 1.0)), json!({"layerName": "landuse", "class": "park"}))];
        let registered = [
            RegisteredTree { x: 0.5, y: 0.5, kind: 1, crown: 0.0 },
            RegisteredTree { x: 0.25, y: 0.75, kind: 4, crown: 8.0 },
            RegisteredTree { x: 0.3, y: 0.3, kind: 3, crown: 0.0 },
            RegisteredTree { x: 0.7, y: 0.2, kind: 0, crown: 30.0 },
            RegisteredTree { x: 1.2, y: 0.2, kind: 2, crown: 3.0 },
        ];
        let f = plant_tile(&park, TILE, july, &Surroundings { registered: &registered, ..Default::default() });
        let fir = ([12.996826171875, 55.60938408809366], [7.300333023071289, 11.555602073669434, 11.825575828552246], [218, 218, 218, 210]);
        check("F conifers", &f.conifers, &d(1, 68.60621025996866, 26.426241874694824, 864, fir, fir));
        check("F broadleaves", &f.broadleaves, &d(3, 205.8162399518701, 36.331299781799316, 2703,
            ([13.001220703125028, 55.61310716821981], [-21.588815689086914, 39.89301681518555, 37.79999923706055], [245, 245, 245, 200]),
            ([12.992431640624972, 55.6118661807857], [-9.051219940185547, -7.72639799118042, 12.408469200134277], [196, 196, 196, 220])));
    }

    /// Streets GL's tags: woods by their leaves, a garden, a car park, scrub with a hole, a
    /// footway, a road, a railway, three mapped trees and two rows of them.
    fn sgl_features() -> Vec<Feature> {
        let line = |points: &[Point], p: serde_json::Value| feature(Geometry::LineString(points.to_vec()), p);
        let point = |[x, y]: Point, p: serde_json::Value| feature(Geometry::Point([x, y]), p);
        vec![
            feature(Geometry::Polygon(square(0.1, 0.1, 0.9, 0.5)), json!({"layerName": "natural", "type": "forest", "leafType": "needleleaved"})),
            feature(Geometry::Polygon(square(0.1, 0.5, 0.9, 0.9)), json!({"layerName": "natural", "type": "forest", "leafType": "broadleaved"})),
            feature(Geometry::Polygon(square(0.5, 0.0, 1.0, 1.0)), json!({"layerName": "natural", "type": "forest"})),
            feature(Geometry::Polygon(square(0.0, 0.0, 0.2, 0.2)), json!({"layerName": "common", "type": "garden"})),
            feature(Geometry::Polygon(square(0.3, 0.3, 0.4, 0.4)), json!({"layerName": "common", "type": "parking"})),
            feature(
                Geometry::Polygon(vec![
                    vec![[0.6, 0.6], [0.8, 0.6], [0.7, 0.8], [0.6, 0.6]],
                    vec![[0.65, 0.62], [0.75, 0.62], [0.7, 0.7], [0.65, 0.62]],
                ]),
                json!({"layerName": "natural", "type": "scrub"}),
            ),
            line(&[[0.0, 0.7], [0.5, 0.75], [1.0, 0.72]], json!({"layerName": "highways", "pathType": "footway"})),
            line(&[[0.2, 0.0], [0.25, 1.0]], json!({"layerName": "highways", "pathType": "primary", "lanes": 4})),
            line(&[[0.8, 0.0], [0.8, 1.0]], json!({"layerName": "highways", "type": "railway"})),
            point([0.05, 0.95], json!({"layerName": "point", "type": "tree", "genus": "Tilia", "height": 17})),
            point([0.15, 0.95], json!({"layerName": "point", "type": "tree", "genus": "Quercus"})),
            point([0.02, 0.6], json!({"layerName": "point", "type": "tree", "leafType": "needleleaved", "height": "12"})),
            line(&[[0.0, 0.98], [0.4, 0.98], [0.4, 0.92]], json!({"layerName": "highways", "type": "treeRow"})),
            feature(
                Geometry::MultiLineString(vec![vec![[0.0, 0.01], [0.99, 0.02]], vec![[0.3, 0.5], [0.3, 0.55]]]),
                json!({"layerName": "natural", "type": "treeRow", "genus": "Fagus"}),
            ),
        ]
    }

    /// OpenMapTiles' smaller covers in a residential tile: school grounds, allotments, a garden,
    /// a park, a cemetery, an orchard, a mire and heath; two lakes, a pitch, a motorway, service
    /// roads, a road being built and a line of a kind not known.
    fn omt_features() -> Vec<Feature> {
        let area = |x0: f64, y0: f64, x1: f64, y1: f64, p: serde_json::Value| feature(Geometry::Polygon(square(x0, y0, x1, y1)), p);
        let line = |points: &[Point], p: serde_json::Value| feature(Geometry::LineString(points.to_vec()), p);
        vec![
            area(0.0, 0.0, 1.0, 1.0, json!({"layerName": "landuse", "class": "residential"})),
            area(0.0, 0.0, 0.3, 0.3, json!({"layerName": "landuse", "class": "school"})),
            area(0.3, 0.0, 0.6, 0.3, json!({"layerName": "landcover", "class": "grass", "subclass": "allotments"})),
            area(0.6, 0.0, 1.0, 0.3, json!({"layerName": "landcover", "class": "grass", "subclass": "garden"})),
            area(0.0, 0.3, 0.3, 0.6, json!({"layerName": "landcover", "class": "grass", "subclass": "park"})),
            area(0.3, 0.3, 0.6, 0.6, json!({"layerName": "landuse", "class": "cemetery"})),
            area(0.6, 0.3, 1.0, 0.6, json!({"layerName": "landcover", "class": "farmland", "subclass": "orchard"})),
            area(0.0, 0.6, 0.3, 1.0, json!({"layerName": "landcover", "class": "wetland"})),
            area(0.3, 0.6, 0.6, 1.0, json!({"layerName": "landcover", "class": "grass", "subclass": "heath"})),
            feature(
                Geometry::MultiPolygon(vec![square(0.62, 0.62, 0.7, 0.7), square(0.8, 0.8, 0.9, 0.95)]),
                json!({"layerName": "water", "class": "lake"}),
            ),
            area(0.7, 0.7, 0.8, 0.75, json!({"layerName": "landuse", "class": "pitch"})),
            line(&[[0.0, 0.45], [1.0, 0.47]], json!({"layerName": "transportation", "class": "motorway"})),
            feature(
                Geometry::MultiLineString(vec![vec![[0.5, 0.0], [0.52, 1.0]], vec![[0.1, 0.1], [0.12, 0.12]]]),
                json!({"layerName": "transportation", "class": "service"}),
            ),
            line(&[[0.9, 0.0], [0.9, 1.0]], json!({"layerName": "transportation", "class": "minor_construction"})),
            line(&[[0.95, 0.0], [0.95, 1.0]], json!({"layerName": "transportation", "class": "unknown_kind"})),
        ]
    }

    #[test]
    fn mapped_trees_rows_and_every_cover_match_the_typescript() {
        let sgl = TileIndex { x: 35135, y: 20534, z: 16 };
        let g = plant_tile(&sgl_features(), sgl, utc(2026, 0, 20), &Surroundings::default());
        check("G conifers", &g.conifers, &d(970, 66555.29197413755, 19211.87479752628, 844641,
            ([13.002429199218739, 55.60752241547618], [-6.15751314163208, -13.503270149230957, 12.720000267028809], [220, 220, 220, 210]),
            ([13.007160656186386, 55.608079037516674], [11.865791320800781, 15.879146575927734, 21.390281677246094], [209, 209, 209, 210])));
        check("G broadleaves", &g.broadleaves, &d(1207, 82815.93058244046, 25220.410920824856, 629072,
            ([13.002593994140653, 55.60643639897184], [-8.189703941345215, 15.062864303588867, 20.229999542236328], [131, 116, 101, 225]),
            ([13.005677848667858, 55.60924510544047], [-11.903773307800293, -10.36966609954834, 21.43037223815918], [124, 109, 95, 235])));
        let registered = [RegisteredTree { x: 0.12, y: 0.95, kind: 3, crown: 6.0 }];
        let (canopy, broadleaves) = (canopy_over(0.3, 0.9, 9), land(lc::BROADLEAVES));
        let around = Surroundings { canopy: Some(&canopy), land: Some(&broadleaves), registered: &registered };
        let g2 = plant_tile(&sgl_features(), sgl, utc(2026, 4, 5), &around);
        check("G2 conifers", &g2.conifers, &d(307, 21064.405394182795, 2998.1529369462514, 268305,
            ([13.002429199218739, 55.60752241547618], [-6.15751314163208, -13.503270149230957, 12.720000267028809], [220, 220, 220, 210]),
            ([13.005067877670058, 55.607898056518586], [10.210891723632812, 2.87699818611145, 9.469175338745117], [218, 218, 218, 210])));
        check("G2 broadleaves", &g2.broadleaves, &d(660, 45284.46467412722, 6149.174916185439, 359076,
            ([13.002978515625017, 55.60643639897184], [-3.1777820587158203, -7.515803337097168, 10.199999809265137], [112, 158, 67, 235]),
            ([13.00642122678812, 55.60707656390056], [11.16225528717041, -0.6231063604354858, 10.605306625366211], [109, 146, 65, 205])));

        let canopy = canopy_over(0.0, 0.45, 12);
        let h = plant_tile(&omt_features(), TILE, utc(2026, 4, 1), &Surroundings { canopy: Some(&canopy), ..Default::default() });
        check("H conifers", &h.conifers, &d(1900, 130344.41148045486, 17438.698063034564, 1651107,
            ([12.985893582790311, 55.61414661515768], [7.378453731536865, -14.14918041229248, 12.641094207763672], [220, 220, 220, 210]),
            ([12.996730960154878, 55.60397767234376], [3.2615935802459717, 3.9366865158081055, 3.8043527603149414], [235, 235, 235, 210])));
        check("H broadleaves", &h.broadleaves, &d(17534, 1202965.8514069507, 147844.0539020173, 9426959,
            ([12.987896769067873, 55.612561460584445], [-15.793761253356934, 0.5918669700622559, 14.828802108764648], [104, 147, 63, 205]),
            ([13.003038692295064, 55.610534800560956], [2.7455198764801025, 6.1501617431640625, 5.520573139190674], [130, 174, 77, 225])));
        // the same south of the equator, at zoom 16, in its autumn
        let forest = land(lc::FOREST);
        let south = TileIndex { x: 30000, y: 40000, z: 16 };
        let i = plant_tile(&omt_features(), south, summer(), &Surroundings { land: Some(&forest), ..Default::default() });
        check("I conifers", &i.conifers, &d(136, -7083.527676602088, 1005.6275322437286, 118200,
            ([-15.203002535164671, -36.88178721290332], [9.799407005310059, 3.1033785343170166, 9.045774459838867], [237, 237, 237, 210]),
            ([-15.199806881787595, -36.88059802541158], [3.2801876068115234, -4.693456649780273, 6.31663703918457], [228, 228, 228, 210])));
        check("I broadleaves", &i.broadleaves, &d(1431, -74531.29996855614, 8315.723505958915, 809245,
            ([-15.203061175636464, -36.882750255626796], [1.882406234741211, 3.9705944061279297, 3.3997130393981934], [202, 155, 47, 245]),
            ([-15.200887316393391, -36.88179214506008], [-3.9679017066955566, 2.2060587406158447, 4.044647693634033], [154, 142, 53, 235])));
    }

    #[test]
    fn shadows_lay_the_cards_down_away_from_the_sun_as_the_typescript_does() {
        let a = plant_tile(&malmo_wood(), TILE, summer(), &Surroundings::default());
        let s = tree_shadows(&a, [0.3, -0.5, 0.6]).unwrap();
        assert_eq!(s.len(), 7151);
        assert_eq!(s.matrices.iter().map(|&v| f64::from(v)).sum::<f64>(), 55747.68181364238);
        let first = [-2.8413171768188477, 20.3322811126709, 0.0, -20.3322811126709, -2.8413171768188477, 0.0, -10.486642837524414, 17.477737426757812, 0.0, 0.0, 0.0, 0.05000000074505806];
        assert_eq!(s.matrices[..12], first.map(|v: f64| v as f32));
        let last = [17.03611183166504, 2.4341628551483154, 0.0, -2.4341628551483154, 17.03611183166504, 0.0, -7.922036647796631, 13.203394889831543, 0.0, 0.0, 0.0, 0.05000000074505806];
        assert_eq!(s.matrices[s.matrices.len() - 12..], last.map(|v: f64| v as f32));
        // the conifers first, then the broadleaves, positions and colours as they are
        assert_eq!(s.positions[..a.conifers.positions.len()], a.conifers.positions[..]);
        assert_eq!(s.colors[a.conifers.colors.len()..], a.broadleaves.colors[..]);
        assert_eq!(tree_shadows(&a, [0.3, -0.5, -0.1]), None);
        assert_eq!(tree_shadows(&TileTrees::default(), [0.0, 0.0, 1.0]), Some(Instances::default()));
    }

    #[test]
    fn shadows_are_cast_again_for_each_half_degree_of_sun() {
        assert_eq!(shadow_key([0.0, 1.0, 0.5]), Some((0, 60)));
        assert_eq!(shadow_key([1.0, 0.0, 0.03]), Some((180, 3)));
        assert_eq!(shadow_key([-1.0, -1.0, 2.0]), Some((-270, 180)));
        assert_eq!(shadow_key([1.0, 0.0, 0.02]), None);
        assert_eq!(shadow_key([1.0, 0.0, f64::NAN]), None);
    }

    #[test]
    fn what_streets_gl_and_openmaptiles_tags_grow_and_block() {
        let cover = |v: serde_json::Value| cover_of(&props(v));
        assert_eq!(cover(json!({"layerName": "natural", "type": "forest", "leafType": "broadleaved"})), 11);
        assert_eq!(cover(json!({"layerName": "natural", "type": "forest", "leafType": "needleleaved"})), 12);
        assert_eq!(cover(json!({"layerName": "natural", "type": "forest", "leafType": "mixed"})), 10);
        assert_eq!(cover(json!({"layerName": "natural", "type": "scrub"})), 9);
        assert_eq!(cover(json!({"layerName": "natural", "type": "wetland"})), 0);
        assert_eq!(cover(json!({"layerName": "common", "type": "garden"})), 4);
        assert_eq!(cover(json!({"layerName": "landcover", "class": "farmland", "subclass": "orchard"})), ORCHARD);
        assert_eq!(cover(json!({"layerName": "landcover", "class": "farmland"})), 0);
        assert_eq!(cover(json!({"layerName": "landcover", "class": "grass", "subclass": "heath"})), 9);
        assert_eq!(cover(json!({"layerName": "landcover", "class": "grass", "subclass": "village_green"})), 5);
        assert_eq!(cover(json!({"layerName": "landcover", "class": "grass", "subclass": "allotments"})), 3);
        assert_eq!(cover(json!({"layerName": "landcover", "class": "grass"})), 0);
        assert_eq!(cover(json!({"layerName": "landuse", "class": "college"})), 2);
        assert_eq!(cover(json!({"layerName": "landuse", "class": "cemetery"})), 6);
        assert_eq!(cover(json!({"layerName": "park", "class": "wood"})), 0);

        let block = |v: serde_json::Value, kind: &str| block_of(&props(v), kind);
        assert_eq!(block(json!({"layerName": "buildings"}), "Polygon"), Block::Fill);
        assert_eq!(block(json!({"layerName": "building"}), "LineString"), Block::Clear(0.0));
        assert_eq!(block(json!({"layerName": "common", "type": "parking"}), "MultiPolygon"), Block::Fill);
        assert_eq!(block(json!({"layerName": "common", "type": "garden"}), "Polygon"), Block::Clear(0.0));
        assert_eq!(block(json!({"layerName": "highways", "type": "railway"}), "LineString"), Block::Clear(3.5));
        // a footway its own 2 m and 1.5 m; four lanes of road 12 m
        assert_eq!(block(json!({"layerName": "highways", "pathType": "footway"}), "LineString"), Block::Clear(2.5));
        assert_eq!(block(json!({"layerName": "highways", "pathType": "primary", "lanes": 4}), "LineString"), Block::Clear(7.5));
        assert_eq!(block(json!({"layerName": "highways", "pathType": "elevator"}), "LineString"), Block::Clear(0.0));
        assert_eq!(block(json!({"layerName": "highways"}), "Polygon"), Block::Fill);
        assert_eq!(block(json!({"layerName": "landuse", "class": "pitch"}), "Polygon"), Block::Fill);
        assert_eq!(block(json!({"layerName": "landuse", "class": "pitch"}), "LineString"), Block::Clear(0.0));
        assert_eq!(block(json!({"layerName": "transportation", "class": "motorway"}), "LineString"), Block::Clear(12.0));
        assert_eq!(block(json!({"layerName": "transportation", "class": "primary_construction"}), "LineString"), Block::Clear(0.0));
        assert_eq!(block(json!({"layerName": "transportation", "class": "minor_construction"}), "Polygon"), Block::Clear(0.0));
        assert_eq!(block(json!({"layerName": "transportation", "class": "aerialway"}), "LineString"), Block::Clear(3.0));
        assert_eq!(block(json!({"layerName": "transportation"}), "LineString"), Block::Clear(3.0));
        assert_eq!(block(json!({"layerName": "transportation", "class": "pier"}), "Polygon"), Block::Fill);
        assert_eq!(block(json!({"layerName": "aeroway", "class": "runway"}), "Polygon"), Block::Clear(0.0));
    }

    #[test]
    fn the_masks_fill_cell_centres_inside_and_stroke_cells_near_a_line() {
        let mut mask = vec![0u8; GRID * GRID];
        // four cells by two: their centres from 10.5 to 13.5 and 20.5 to 21.5
        let g = GRID as f64;
        fill_polygon(&mut mask, &square(10.0 / g, 20.0 / g, 14.0 / g, 22.0 / g), 3);
        let set: Vec<(usize, usize)> = (0..GRID * GRID).filter(|&i| mask[i] != 0).map(|i| (i % GRID, i / GRID)).collect();
        assert_eq!(set, [(10, 20), (11, 20), (12, 20), (13, 20), (10, 21), (11, 21), (12, 21), (13, 21)]);
        // a hole, even-odd
        let mut mask = vec![0u8; GRID * GRID];
        let mut rings = square(0.0, 0.0, 6.0 / g, 6.0 / g);
        rings.extend(square(2.0 / g, 2.0 / g, 4.0 / g, 4.0 / g));
        fill_polygon(&mut mask, &rings, 1);
        assert_eq!(mask.iter().filter(|&&v| v != 0).count(), 36 - 4);
        assert_eq!(mask[2 * GRID + 2], 0);
        // a line along a row of centres, a cell either side
        let mut mask = vec![0u8; GRID * GRID];
        stroke_line(&mut mask, &[[5.0 / g, 100.5 / g], [9.0 / g, 100.5 / g]], 1.0, 1);
        let set: Vec<(usize, usize)> = (0..GRID * GRID).filter(|&i| mask[i] != 0).map(|i| (i % GRID, i / GRID)).collect();
        let mut want = Vec::new();
        for (row, cols) in [(99, 5..9), (100, 4..10), (101, 5..9)] {
            want.extend(cols.map(|c| (c, row)));
        }
        assert_eq!(set, want);
        // off the tile: nothing, and nothing out of bounds
        let mut mask = vec![0u8; GRID * GRID];
        stroke_line(&mut mask, &[[-1.0, -1.0], [-0.5, -1.0]], 3.0, 1);
        fill_polygon(&mut mask, &square(1.5, 1.5, 2.0, 2.0), 1);
        fill_polygon(&mut mask, &[], 1);
        assert!(mask.iter().all(|&v| v == 0));
    }

    #[test]
    fn a_tile_with_nothing_that_grows_has_no_trees() {
        let road = [feature(Geometry::LineString(vec![[0.0, 0.5], [1.0, 0.5]]), json!({"layerName": "transportation", "class": "primary"}))];
        assert_eq!(plant_tile(&road, TILE, summer(), &Surroundings::default()), TileTrees::default());
        assert_eq!(plant_tile(&[], TILE, summer(), &Surroundings::default()), TileTrees::default());
    }

    #[test]
    fn javascripts_number_of_a_tag() {
        assert_eq!(js_number(Some(&json!(17))), 17.0);
        assert_eq!(js_number(Some(&json!(" 12 "))), 12.0);
        assert_eq!(js_number(Some(&json!("0x10"))), 16.0);
        assert_eq!(js_number(Some(&json!(""))), 0.0);
        assert_eq!(js_number(Some(&json!(null))), 0.0);
        assert_eq!(js_number(Some(&json!(true))), 1.0);
        assert!(js_number(Some(&json!("12 m"))).is_nan());
        assert!(js_number(Some(&json!("inf"))).is_nan());
        assert!(js_number(None).is_nan());
        assert_eq!(js_number(Some(&json!(["7"]))), 7.0);
    }
}
