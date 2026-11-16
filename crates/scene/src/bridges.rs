//! Bridges and tunnels: how high a line on a bridge stands, and the bridges as 3D structures.
//!
//! - **One height function** ([`rise`]): a bridge's deck (OpenMapTiles' `brunnel` a `layer` up,
//!   [`omt_deck`]; OpenRailwayMap's railway bridges, [`orm_deck`]; the backend's [`Structure`]s
//!   under the trains, [`level_at`]) rises from each end that meets the ground at no more than
//!   [`GRADE`] (smoothstep), so a ramp is part of the bridge and its ends sit on the ground with no
//!   step. A short bridge does not reach its full deck: it only climbs as far as its ramps allow.
//!   An end where the bridge goes on (another bridge piece there, or the tile's edge) stays up.
//!   A deck whose height cannot be read (NaN) is on the ground.
//! - **The tiles' bridges** ([`bridge_parts`]): a vector tile's lines split into those on the
//!   ground and the bridges, cut into pieces of at most [`PIECE_M`] and raised along [`rise`], in
//!   metres. The ground's painted line passes draw the raised pieces as the deck's surface;
//!   [`Structures`] adds what holds them up: a slab with thickness and side faces under the deck,
//!   parapets along a road bridge, piers under long spans, in the mesh pipeline's local vertices.
//! - **On the track** ([`level_at`], [`on_structures`]): the trains and signal blocks on the
//!   backend's structures, the same [`rise`] with [`RAIL_DECK_M`].

use std::f64::consts::PI;

use bytemuck::{Pod, Zeroable};
use indexmap::IndexMap;
use rustradar_core::geo::EARTH_CIRCUMFERENCE_M;
use rustradar_core::num::clamp01;
use serde_json::Value;

use crate::geom::{js_hypot2, js_max, js_min};
pub use crate::tess::vector::Cancelled;

pub use rustradar_core::model::train::Structure;

/// A raised line is cut into pieces no longer than this, so it can rise along them, metres.
pub const PIECE_M: f64 = 6.0;
/// The steepest a bridge's ramp climbs on average (the smoothstep's middle is 1.5 times this).
pub const GRADE: f64 = 0.08;
/// A bridge with both ends on the ground spends at most this share of its length on each ramp.
const RAMP_SHARE: f64 = 0.4;

/// A railway bridge's deck, metres (the map's data doesn't say how high).
pub const RAIL_DECK_M: f64 = 6.0;
/// A bridge's deck, metres: a level more for each `layer` above the first (a bridge over a bridge).
const DECK_M: f64 = 6.0;

/// A feature's properties, as a tile carries them.
pub type Properties = serde_json::Map<String, Value>;

/// How high a line's deck is, metres; 0 on the ground. The second argument reads a numeric
/// property at one of its vertices (`None` when there is none).
pub type DeckHeight = dyn Fn(&Properties, &dyn Fn(&str) -> Option<f64>) -> f64;

/// OpenMapTiles' bridges (`brunnel`): railways at a railway's deck, the rest a deck a `layer`.
pub fn omt_deck(p: &Properties, numeric: &dyn Fn(&str) -> Option<f64>) -> f64 {
    if p.get("brunnel").and_then(Value::as_str) != Some("bridge") {
        return 0.0;
    }
    let class = p.get("class").and_then(Value::as_str);
    if class == Some("rail") || class == Some("transit") {
        return RAIL_DECK_M;
    }
    // `numeric("layer") ?? Number(p.layer ?? 1)`
    let layer = numeric("layer").unwrap_or_else(|| match p.get("layer") {
        None | Some(Value::Null) => 1.0,
        Some(v) => js_number(v),
    });
    DECK_M * js_max(1.0, js_min(4.0, layer))
}

/// OpenRailwayMap's bridges (`bridge`).
pub fn orm_deck(p: &Properties, _numeric: &dyn Fn(&str) -> Option<f64>) -> f64 {
    if p.get("bridge") == Some(&Value::Bool(true)) { RAIL_DECK_M } else { 0.0 }
}

/// JavaScript's `Number(v)` for what a tile's property can be (an array or an object reads as
/// NaN; tiles carry neither).
fn js_number(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => string_to_number(s),
        Value::Array(_) | Value::Object(_) => f64::NAN,
    }
}

/// `Number(s)`: blank is 0; a decimal literal, `Infinity` (signed), or `0x`/`0o`/`0b` digits; else
/// NaN.
fn string_to_number(s: &str) -> f64 {
    let s = s.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}');
    if s.is_empty() {
        return 0.0;
    }
    match s {
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
        return digits.chars().try_fold(0.0_f64, |n, c| c.to_digit(radix).map(|d| n * f64::from(radix) + f64::from(d))).unwrap_or(f64::NAN);
    }
    let decimal = s.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'));
    if decimal && s.bytes().any(|b| b.is_ascii_digit()) { s.parse().unwrap_or(f64::NAN) } else { f64::NAN }
}

/// How high a bridge's deck is `dist` metres (along the bridge) from the nearest end where it
/// meets the ground, on a bridge whose farthest point is `span` metres from such an end, its deck
/// `deck` metres up: up from the ground over a smoothstep ramp averaging [`GRADE`], each ramp at
/// most [`RAMP_SHARE`] of a bridge with both ends on the ground (`2 · span` long), so a short
/// bridge climbs only as high as its ramps allow. With no end on the ground (`dist` infinite) the
/// deck all along; 0 for a deck that is not a number.
pub fn deck_height(dist: f64, span: f64, deck: f64) -> f64 {
    if deck.is_nan() || deck <= 0.0 {
        return 0.0;
    }
    if dist.is_infinite() || span.is_infinite() {
        return deck;
    }
    let full = deck / GRADE;
    let ramp = js_min(full, 2.0 * RAMP_SHARE * span);
    if ramp.is_nan() || ramp <= 0.0 {
        return 0.0;
    }
    let k = clamp01(dist / ramp);
    deck * ramp / full * k * k * (3.0 - 2.0 * k)
}

/// One bridge piece among others ([`reaches`]): how far its start and end are from the nearest
/// end on the ground along the bridge (infinite where none is), and the farthest any point of the
/// bridge it is part of is from one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reach {
    pub ends: [f64; 2],
    pub span: f64,
}

impl Reach {
    /// The deck's height `d` metres along a piece `length` long ([`deck_height`]).
    pub fn height(&self, d: f64, length: f64, deck: f64) -> f64 {
        deck_height(js_min(self.ends[0] + d, self.ends[1] + (length - d)), self.span, deck)
    }
}

/// The pieces of the bridges in a place (each `lengths[i]` metres, its ends at the nodes
/// `ends[i]`), joined where they share a node: how far each piece's ends are from where the
/// bridge meets the ground. A node is on the ground where one piece alone ends and `free` says
/// so (not where the bridge goes on out of the data: a tile's edge, or a structure joined to one
/// not given).
pub fn reaches<K: std::hash::Hash + Eq + Copy>(lengths: &[f64], ends: &[[K; 2]], free: impl Fn(usize, usize) -> bool) -> Vec<Reach> {
    reaches_grounded(lengths, ends, free, |_, _| false)
}

/// [`reaches`] with ends that meet the ground whatever else ends there (`grounded`: a line on the
/// ground goes on from there).
pub fn reaches_grounded<K: std::hash::Hash + Eq + Copy>(
    lengths: &[f64],
    ends: &[[K; 2]],
    free: impl Fn(usize, usize) -> bool,
    grounded: impl Fn(usize, usize) -> bool,
) -> Vec<Reach> {
    use std::collections::HashMap;
    let mut index: HashMap<K, usize> = HashMap::new();
    let nodes: Vec<[usize; 2]> = ends
        .iter()
        .map(|e| {
            e.map(|k| {
                let n = index.len();
                *index.entry(k).or_insert(n)
            })
        })
        .collect();
    let count = index.len();
    let mut degree = vec![0u32; count];
    for n in nodes.iter().flatten() {
        degree[*n] += 1;
    }
    let mut dist = vec![f64::INFINITY; count];
    for (i, n) in nodes.iter().enumerate() {
        for k in 0..2 {
            if (degree[n[k]] == 1 && free(i, k)) || grounded(i, k) {
                dist[n[k]] = 0.0;
            }
        }
    }
    // the shortest way to the ground through the pieces (few pieces: relax until settled)
    for _ in 0..=nodes.len() {
        let mut changed = false;
        for (i, &[a, b]) in nodes.iter().enumerate() {
            let len = lengths[i];
            if dist[a] + len < dist[b] {
                dist[b] = dist[a] + len;
                changed = true;
            }
            if dist[b] + len < dist[a] {
                dist[a] = dist[b] + len;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // each joined bridge's farthest point from the ground
    let mut parent: Vec<usize> = (0..count).collect();
    fn root(parent: &mut [usize], mut n: usize) -> usize {
        while parent[n] != n {
            parent[n] = parent[parent[n]];
            n = parent[n];
        }
        n
    }
    for &[a, b] in &nodes {
        let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
        parent[ra] = rb;
    }
    let mut span = vec![0.0_f64; count];
    for (i, &[a, b]) in nodes.iter().enumerate() {
        let (da, db, len) = (dist[a], dist[b], lengths[i]);
        let far = if da.is_infinite() && db.is_infinite() {
            f64::INFINITY
        } else if (da - db).abs() <= len {
            (da + db + len) / 2.0
        } else {
            js_min(da, db) + len
        };
        let r = root(&mut parent, a);
        span[r] = js_max(span[r], far);
    }
    nodes
        .iter()
        .map(|&[a, b]| {
            let r = root(&mut parent, a);
            Reach { ends: [dist[a], dist[b]], span: span[r] }
        })
        .collect()
}

/// How high a lone bridge is `d` metres along it, of `total`, its deck `deck` metres up, its ends
/// on the ground but where it goes on (`on`) ([`deck_height`]).
pub fn rise(d: f64, total: f64, deck: f64, on: [bool; 2]) -> f64 {
    let reach = reaches(&[total], &[[0u8, 1u8]], |_, k| !on[k])[0];
    reach.height(d, total, deck)
}

/// A tile's lines in the layout of loaders.gl's binary GeoJSON, as far as they are read here:
/// flat positions (`size` numbers a vertex), the vertex each path starts at (ending with the
/// vertex count), per vertex its feature's index into `properties` and its global id, and per
/// vertex the numeric properties.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lines {
    /// loaders.gl's `type` (`"LineString"`), carried through.
    pub kind: String,
    pub positions: Vec<f64>,
    pub size: usize,
    pub path_indices: Vec<u32>,
    pub feature_ids: Vec<u32>,
    pub global_feature_ids: Vec<u32>,
    pub numeric_props: IndexMap<String, Vec<f64>>,
    pub properties: Vec<Properties>,
    /// loaders.gl's `fields`, carried through.
    pub fields: Option<Vec<Value>>,
}

/// A tile's index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileIndex {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

/// Metres across the tile, at its middle's latitude.
fn tile_metres(tile: TileIndex) -> f64 {
    let n = 2f64.powi(tile.z as i32);
    let lat = (PI * (1.0 - (2.0 * (f64::from(tile.y) + 0.5)) / n)).sinh().atan();
    (EARTH_CIRCUMFERENCE_M * lat.cos()) / n
}

/// Lines being gathered: their columns.
struct Builder<'a> {
    from: &'a Lines,
    size: usize,
    positions: Vec<f64>,
    path_indices: Vec<u32>,
    feature_ids: Vec<u32>,
    global_feature_ids: Vec<u32>,
    numeric: IndexMap<String, Vec<f64>>,
}

impl<'a> Builder<'a> {
    fn new(from: &'a Lines, size: usize) -> Self {
        Self {
            from,
            size,
            positions: Vec::new(),
            path_indices: vec![0],
            feature_ids: Vec::new(),
            global_feature_ids: Vec::new(),
            numeric: from.numeric_props.keys().map(|k| (k.clone(), Vec::new())).collect(),
        }
    }

    /// One vertex, its attributes those of vertex `i` of the source.
    fn push(&mut self, i: usize, x: f64, y: f64, z: Option<f64>) {
        self.positions.extend([x, y]);
        if self.size == 3 {
            self.positions.push(z.unwrap_or(0.0));
        }
        self.feature_ids.push(self.from.feature_ids.get(i).copied().unwrap_or(0));
        self.global_feature_ids.push(self.from.global_feature_ids.get(i).copied().unwrap_or(0));
        for (k, column) in &mut self.numeric {
            column.push(self.from.numeric_props[k].get(i).copied().unwrap_or(f64::NAN));
        }
    }

    fn end(&mut self) {
        let count = (self.positions.len() / self.size) as u32;
        if count > *self.path_indices.last().unwrap_or(&0) {
            self.path_indices.push(count);
        }
    }

    fn build(self) -> Lines {
        Lines {
            kind: self.from.kind.clone(),
            positions: self.positions,
            size: self.size,
            path_indices: self.path_indices,
            feature_ids: self.feature_ids,
            global_feature_ids: self.global_feature_ids,
            numeric_props: self.numeric,
            properties: self.from.properties.clone(),
            fields: self.from.fields.clone(),
        }
    }
}


/// A tile's lines split by their deck height ([`bridge_parts`]).
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeParts {
    /// The lines on the ground, as they were.
    pub ground: Lines,
    /// The bridges, three numbers a vertex, the third the height in metres ([`rise`]), cut into
    /// pieces no longer than [`PIECE_M`] so they can rise along them.
    pub raised: Lines,
}

/// `lines` split by `height`: those on the ground, the bridges raised (or `None` when there are
/// none, for the passes to be left as they were). Paths of fewer than two vertices are dropped.
/// Bridge pieces that share an end are one bridge ([`reaches`]); an end within 0.001 of the
/// tile's edge stays up: the bridge goes on there.
pub fn bridge_parts(lines: &Lines, tile: TileIndex, height: &DeckHeight) -> Option<BridgeParts> {
    let size = lines.size;
    if size < 2 {
        return None;
    }
    let at = |k: usize| lines.positions.get(k).copied().unwrap_or(f64::NAN);
    let paths = &lines.path_indices;
    let tile_metres = tile_metres(tile);
    let empty = Properties::new();
    // each path's deck (0 on the ground)
    let decks: Vec<f64> = (0..paths.len().saturating_sub(1))
        .map(|p| {
            let (start, end) = (paths[p] as usize, paths[p + 1] as usize);
            if end < start + 2 {
                return 0.0;
            }
            let props = lines.feature_ids.get(start).and_then(|&f| lines.properties.get(f as usize)).unwrap_or(&empty);
            let numeric = |name: &str| lines.numeric_props.get(name).and_then(|c| c.get(start).copied());
            let deck = height(props, &numeric);
            if deck > 0.0 { deck } else { 0.0 }
        })
        .collect();
    if decks.iter().all(|&d| d == 0.0) {
        return None;
    }
    // the bridges' pieces joined where they meet, each end on the ground unless the tile's edge
    // cuts it there
    let key = |i: usize| (at(i * size).to_bits(), at(i * size + 1).to_bits());
    let cut = |i: usize| {
        let (x, y) = (at(i * size), at(i * size + 1));
        x <= 0.001 || x >= 0.999 || y <= 0.001 || y >= 0.999
    };
    let bridges: Vec<usize> = (0..decks.len()).filter(|&p| decks[p] > 0.0).collect();
    let along_of = |p: usize| {
        let (start, end) = (paths[p] as usize, paths[p + 1] as usize);
        let mut along = vec![0.0];
        for i in start + 1..end {
            let (dx, dy) = (at(i * size) - at((i - 1) * size), at(i * size + 1) - at((i - 1) * size + 1));
            along.push(along[along.len() - 1] + js_hypot2(dx, dy) * tile_metres);
        }
        along
    };
    let alongs: Vec<Vec<f64>> = bridges.iter().map(|&p| along_of(p)).collect();
    let lengths: Vec<f64> = alongs.iter().map(|a| a[a.len() - 1]).collect();
    let ends: Vec<[(u64, u64); 2]> = bridges.iter().map(|&p| [key(paths[p] as usize), key(paths[p + 1] as usize - 1)]).collect();
    // where a line on the ground ends the bridge meets the ground, however many pieces end there
    let grounded: std::collections::HashSet<(u64, u64)> = (0..decks.len())
        .filter(|&p| decks[p] == 0.0 && paths[p + 1] > paths[p])
        .flat_map(|p| [key(paths[p] as usize), key(paths[p + 1] as usize - 1)])
        .collect();
    let end_vertex = |b: usize, k: usize| if k == 0 { paths[bridges[b]] as usize } else { paths[bridges[b] + 1] as usize - 1 };
    let mut reach = reaches(&lengths, &ends, |b, k| !cut(end_vertex(b, k)));
    if !grounded.is_empty() {
        let on_ground: Vec<[bool; 2]> = (0..bridges.len()).map(|b| [0, 1].map(|k| grounded.contains(&key(end_vertex(b, k))))).collect();
        if on_ground.iter().flatten().any(|&g| g) {
            reach = reaches_grounded(&lengths, &ends, |b, k| !cut(end_vertex(b, k)), |b, k| on_ground[b][k]);
        }
    }
    let mut ground = Builder::new(lines, size);
    let mut raised = Builder::new(lines, 3);
    let third = |i: usize| (size == 3).then(|| at(i * size + 2));
    for (p, &deck) in decks.iter().enumerate() {
        let (start, end) = (paths[p] as usize, paths[p + 1] as usize);
        if end < start + 2 {
            continue;
        }
        if deck == 0.0 {
            for i in start..end {
                ground.push(i, at(i * size), at(i * size + 1), third(i));
            }
            ground.end();
            continue;
        }
        let b = bridges.binary_search(&p).unwrap_or(0);
        let (along, total, reach) = (&alongs[b], lengths[b], reach[b]);
        let up = |d: f64| reach.height(d, total, deck);
        for i in start..end {
            let (x, y) = (at(i * size), at(i * size + 1));
            if i > start {
                // the way there in pieces, rising as it goes
                let (px, py) = (at((i - 1) * size), at((i - 1) * size + 1));
                let (d0, d1) = (along[i - 1 - start], along[i - start]);
                let pieces = ((d1 - d0) / PIECE_M).ceil();
                for k in 1..below(pieces) {
                    let t = k as f64 / pieces;
                    raised.push(i - 1, px + (x - px) * t, py + (y - py) * t, Some(up(d0 + (d1 - d0) * t)));
                }
            }
            raised.push(i, x, y, Some(up(along[i - start])));
        }
        raised.end();
    }
    Some(BridgeParts { ground: ground.build(), raised: raised.build() })
}

/// `for (k = 1; k < pieces; k++)` as a range end: 1 for NaN and for an infinite count.
fn below(pieces: f64) -> usize {
    if pieces.is_finite() && pieces > 1.0 { pieces as usize } else { 1 }
}

/// Metres per tile unit of `tile` (at its middle's latitude), for [`Structures::push`].
pub fn metres_per_unit(tile: TileIndex) -> f64 {
    tile_metres(tile)
}

// --- the structures ----------------------------------------------------------

/// One vertex of a bridge's structure, in the mesh pipeline's local layout (`rustradar_map`'s
/// `pipelines::mesh::Vertex`): `u` east and `v` south in tile units and metres up; a normal east,
/// north, up; an RGB colour 0–1.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct StructureVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

/// The deck's slab under its surface, metres thick.
const SLAB_M: f64 = 1.1;
/// The slab's top sits this far under the deck's surface (the ground's painted lines, drawn over
/// it), metres.
const UNDER_PAINT_M: f64 = 0.06;
/// A road bridge's parapets: how tall above the deck and how thick, metres.
const PARAPET_M: f64 = 0.9;
const PARAPET_THICK_M: f64 = 0.3;
/// A railway bridge's edge beams, as its parapets (low: tracks side by side share a structure).
const RAIL_EDGE_M: f64 = 0.35;
const RAIL_EDGE_THICK_M: f64 = 0.25;
/// Piers stand at most this far apart, metres, under a deck at least [`PIER_MIN_M`] up.
const PIER_SPACING_M: f64 = 32.0;
const PIER_MIN_M: f64 = 2.5;
/// A pier's length along the bridge, metres, and its width as a share of the deck's.
const PIER_LONG_M: f64 = 1.6;
const PIER_ACROSS: f64 = 0.7;

/// Concrete: the slab's sides, its underside, the parapets and the piers (sRGB 0–1, lit by the
/// mesh pipeline).
const SIDE: [f32; 3] = [0.74, 0.73, 0.70];
const UNDERSIDE: [f32; 3] = [0.52, 0.52, 0.51];
const PARAPET: [f32; 3] = [0.84, 0.83, 0.80];
const PIER: [f32; 3] = [0.66, 0.65, 0.62];
const TOP: [f32; 3] = [0.42, 0.42, 0.42];

/// A tile's bridges as 3D structures: what holds each deck up (see the module notes), one mesh,
/// indexed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Structures {
    pub vertices: Vec<StructureVertex>,
    pub indices: Vec<u32>,
    /// Each bridge's deck outline for its shadow: per vertex of its line its left and right edges
    /// at the deck's height (tile units, metres up).
    pub decks: Vec<Vec<[[f32; 3]; 2]>>,
    /// Metres per tile unit ([`metres_per_unit`]), to cast the shadows in.
    pub metres_per_unit: f64,
}

impl Structures {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// The bytes it holds (for the tile cache's budget).
    pub fn byte_size(&self) -> usize {
        size_of_val(self.vertices.as_slice()) + size_of_val(self.indices.as_slice()) + self.decks.iter().map(|d| size_of_val(d.as_slice())).sum::<usize>()
    }

    /// One quad `a b c d` (counter-clockwise seen from where `normal` points), its normal east,
    /// north, up.
    fn quad(&mut self, corners: [[f64; 3]; 4], normal: [f64; 3], color: [f32; 3]) {
        let base = self.vertices.len() as u32;
        let normal = normal.map(|n| n as f32);
        self.vertices.extend(corners.map(|p| StructureVertex { position: p.map(|c| c as f32), normal, color }));
        self.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// A raised path's structure: `path` in tile units and metres up (a piece of
    /// [`BridgeParts::raised`]), `width` metres across, a railway's (edge beams) or a road's
    /// (parapets), `m` metres per tile unit ([`metres_per_unit`]).
    pub fn push(&mut self, path: &[[f64; 3]], width: f64, rail: bool, m: f64) {
        if width.is_nan() || width <= 0.0 || m.is_nan() || m <= 0.0 {
            return;
        }
        // in metres east and north, without repeated points
        let mut p: Vec<[f64; 3]> = Vec::with_capacity(path.len());
        for &[u, v, h] in path {
            let q = [u * m, -v * m, if h.is_finite() { h.max(0.0) } else { 0.0 }];
            if !(q[0].is_finite() && q[1].is_finite()) {
                return;
            }
            if p.last().is_none_or(|l: &[f64; 3]| (l[0] - q[0]).hypot(l[1] - q[1]) > 0.01) {
                p.push(q);
            }
        }
        if p.len() < 2 || p.iter().all(|q| q[2] <= 0.0) {
            return;
        }
        let half = width / 2.0;
        // each vertex's left (a unit normal per segment, mitred at the joints)
        let seg: Vec<[f64; 2]> = p
            .windows(2)
            .map(|w| {
                let (dx, dy) = (w[1][0] - w[0][0], w[1][1] - w[0][1]);
                let l = dx.hypot(dy);
                [-dy / l, dx / l]
            })
            .collect();
        let left: Vec<[f64; 2]> = (0..p.len())
            .map(|i| {
                let a = seg[i.saturating_sub(1).min(seg.len() - 1)];
                let b = seg[i.min(seg.len() - 1)];
                let (x, y) = (a[0] + b[0], a[1] + b[1]);
                let l = x.hypot(y);
                if l < 1e-6 {
                    return b;
                }
                // the miter: as long as it takes to keep the width, at most twice
                let cos = (x * b[0] + y * b[1]) / l;
                let k = 1.0 / cos.max(0.5);
                [x / l * k, y / l * k]
            })
            .collect();
        let to_local = |q: [f64; 3]| [q[0] / m, -q[1] / m, q[2]];
        let at = |i: usize, off: f64, z: f64| to_local([p[i][0] + left[i][0] * off, p[i][1] + left[i][1] * off, z]);
        let (wall, thick) = if rail { (RAIL_EDGE_M, RAIL_EDGE_THICK_M) } else { (PARAPET_M, PARAPET_THICK_M) };
        self.metres_per_unit = m;
        self.decks.push((0..p.len()).map(|i| [at(i, half, p[i][2]), at(i, -half, p[i][2])].map(|q| q.map(|c| c as f32))).collect());
        for i in 0..p.len() - 1 {
            let j = i + 1;
            let (hi, hj) = (p[i][2], p[j][2]);
            if hi <= 0.0 && hj <= 0.0 {
                continue;
            }
            let [nx, ny] = seg[i];
            let (top_i, top_j) = (hi - UNDER_PAINT_M, hj - UNDER_PAINT_M);
            let (bot_i, bot_j) = ((hi - SLAB_M).max(0.0), (hj - SLAB_M).max(0.0));
            // the walls along the edges grow out of the deck as it leaves the ground
            let (wall_i, wall_j) = (hi + wall.min(hi), hj + wall.min(hj));
            // the deck's top (under the paint), for the depth of what passes under it
            self.quad([at(i, -half, top_i), at(j, -half, top_j), at(j, half, top_j), at(i, half, top_i)], [0.0, 0.0, 1.0], TOP);
            // the outer faces, from the slab's bottom to the wall's top
            self.quad([at(i, half, bot_i), at(i, half, wall_i), at(j, half, wall_j), at(j, half, bot_j)], [nx, ny, 0.0], SIDE);
            self.quad([at(j, -half, bot_j), at(j, -half, wall_j), at(i, -half, wall_i), at(i, -half, bot_i)], [-nx, -ny, 0.0], SIDE);
            // the underside
            self.quad([at(i, half, bot_i), at(j, half, bot_j), at(j, -half, bot_j), at(i, -half, bot_i)], [0.0, 0.0, -1.0], UNDERSIDE);
            // the walls' inner faces and tops
            let inner = half - thick;
            self.quad([at(j, inner, top_j), at(j, inner, wall_j), at(i, inner, wall_i), at(i, inner, top_i)], [-nx, -ny, 0.0], PARAPET);
            self.quad([at(i, -inner, top_i), at(i, -inner, wall_i), at(j, -inner, wall_j), at(j, -inner, top_j)], [nx, ny, 0.0], PARAPET);
            self.quad([at(i, inner, wall_i), at(i, half, wall_i), at(j, half, wall_j), at(j, inner, wall_j)], [0.0, 0.0, 1.0], PARAPET);
            self.quad([at(i, -half, wall_i), at(i, -inner, wall_i), at(j, -inner, wall_j), at(j, -half, wall_j)], [0.0, 0.0, 1.0], PARAPET);
        }
        self.piers(&p, &seg, half, m);
    }

    /// Piers under a long span: evenly along the stretches where the slab's bottom is well up.
    fn piers(&mut self, p: &[[f64; 3]], seg: &[[f64; 2]], half: f64, m: f64) {
        let mut along = vec![0.0];
        for w in p.windows(2) {
            along.push(along[along.len() - 1] + (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]));
        }
        let total = along[along.len() - 1];
        if total < PIER_SPACING_M * 1.5 {
            return;
        }
        let count = (total / PIER_SPACING_M).round().max(1.0) as usize;
        let step = total / count as f64;
        let to_local = |q: [f64; 3]| [q[0] / m, -q[1] / m, q[2]];
        let mut i = 0;
        for k in 1..count {
            let s = step * k as f64;
            while i + 2 < along.len() && along[i + 1] < s {
                i += 1;
            }
            let t = ((s - along[i]) / (along[i + 1] - along[i]).max(1e-9)).clamp(0.0, 1.0);
            let h = p[i][2] + (p[i + 1][2] - p[i][2]) * t;
            if h < PIER_MIN_M + SLAB_M {
                continue;
            }
            let c = [p[i][0] + (p[i + 1][0] - p[i][0]) * t, p[i][1] + (p[i + 1][1] - p[i][1]) * t];
            let [nx, ny] = seg[i];
            let (ax, ay) = (ny, -nx);
            let (a, w) = (PIER_LONG_M / 2.0, half * PIER_ACROSS);
            let corner = |da: f64, dw: f64, z: f64| to_local([c[0] + ax * da + nx * dw, c[1] + ay * da + ny * dw, z]);
            let top = h - SLAB_M;
            for (da, dw, n) in [(a, 0.0, [ax, ay]), (-a, 0.0, [-ax, -ay]), (0.0, w, [nx, ny]), (0.0, -w, [-nx, -ny])] {
                // the face at `da` along or `dw` across, its two corners either side
                let (e0, e1) = if da != 0.0 { ((da, -w * da.signum()), (da, w * da.signum())) } else { ((a * dw.signum(), dw), (-a * dw.signum(), dw)) };
                self.quad([corner(e0.0, e0.1, 0.0), corner(e1.0, e1.1, 0.0), corner(e1.0, e1.1, top), corner(e0.0, e0.1, top)], [n[0], n[1], 0.0], PIER);
            }
        }
    }
}


// --- on the track ------------------------------------------------------------

/// A place, `[lon, lat]`.
pub type LonLat = [f64; 2];

/// Where a place on the track is: in a tunnel, or this high (metres); on the ground, neither.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    pub tunnel: bool,
    pub height: f64,
}

/// On the ground.
pub const GROUND: Level = Level { tunnel: false, height: 0.0 };

const M_PER_DEGREE: f64 = 111_320.0;
/// Turned more than this from a structure's line, a way crosses it rather than runs along it,
/// degrees.
const ALONG_DEG: f64 = 30.0;
/// [`level_at`]'s reach from a structure's line, metres (`within = 4`).
pub const LEVEL_WITHIN_M: f64 = 4.0;

/// A structure's bounding box `[west, south, east, north]` and how far along its line each point
/// is, metres.
struct Measured<'a> {
    structure: &'a Structure,
    bbox: [f64; 4],
    along: Vec<f64>,
    /// A bridge's reach among the other bridges it joins ([`reaches`]).
    reach: Reach,
}

/// `structures` measured, the bridges joined where they share an end.
fn measure_all(structures: &[Structure]) -> Vec<Measured<'_>> {
    let mut measured: Vec<Measured> = structures.iter().map(measure).collect();
    let bridges: Vec<usize> = (0..measured.len()).filter(|&i| !structures[i].tunnel && structures[i].line.len() >= 2).collect();
    let key = |p: &LonLat| (p[0].to_bits(), p[1].to_bits());
    let lengths: Vec<f64> = bridges.iter().map(|&i| measured[i].along[measured[i].along.len() - 1]).collect();
    let ends: Vec<[(u64, u64); 2]> = bridges.iter().map(|&i| [key(&structures[i].line[0]), key(&structures[i].line[structures[i].line.len() - 1])]).collect();
    let reach = reaches(&lengths, &ends, |b, k| !structures[bridges[b]].joined[k]);
    for (b, &i) in bridges.iter().enumerate() {
        measured[i].reach = reach[b];
    }
    measured
}

fn measure(s: &Structure) -> Measured<'_> {
    let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    let mut along = vec![0.0];
    for (i, &[lon, lat]) in s.line.iter().enumerate() {
        bbox[0] = js_min(bbox[0], lon);
        bbox[1] = js_min(bbox[1], lat);
        bbox[2] = js_max(bbox[2], lon);
        bbox[3] = js_max(bbox[3], lat);
        if i > 0 {
            let [plon, plat] = s.line[i - 1];
            along.push(along[i - 1] + js_hypot2((lon - plon) * ((lat * PI) / 180.0).cos(), lat - plat) * M_PER_DEGREE);
        }
    }
    Measured { structure: s, bbox, along, reach: Reach { ends: [0.0; 2], span: 0.0 } }
}

/// The level at `p` going `heading` (degrees; `None`: either way): that of the bridge or tunnel
/// whose line is nearest, within 4 m and running the same way (not one it passes under or over);
/// else the ground.
pub fn level_at(p: LonLat, heading: Option<f64>, structures: &[Structure]) -> Level {
    level_at_within(p, heading, structures, LEVEL_WITHIN_M)
}

/// [`level_at`] within `within` metres of a structure's line.
pub fn level_at_within(p: LonLat, heading: Option<f64>, structures: &[Structure], within: f64) -> Level {
    if structures.is_empty() {
        return GROUND;
    }
    level_among(p, heading, &measure_all(structures), within)
}

fn level_among(p: LonLat, heading: Option<f64>, structures: &[Measured], within: f64) -> Level {
    let k = ((p[1] * PI) / 180.0).cos();
    let pad = within / M_PER_DEGREE;
    let mut best = f64::INFINITY;
    let mut level = GROUND;
    for Measured { structure: s, bbox, along, reach } in structures {
        if p[0] < bbox[0] - pad / k || p[0] > bbox[2] + pad / k || p[1] < bbox[1] - pad || p[1] > bbox[3] + pad {
            continue;
        }
        for i in 1..s.line.len() {
            let (ax, ay) = ((s.line[i - 1][0] - p[0]) * k * M_PER_DEGREE, (s.line[i - 1][1] - p[1]) * M_PER_DEGREE);
            let (dx, dy) = ((s.line[i][0] - s.line[i - 1][0]) * k * M_PER_DEGREE, (s.line[i][1] - s.line[i - 1][1]) * M_PER_DEGREE);
            let len2 = dx * dx + dy * dy;
            let t = if len2 > 0.0 { clamp01(-(ax * dx + ay * dy) / len2) } else { 0.0 };
            let d = js_hypot2(ax + dx * t, ay + dy * t);
            if d > within || d >= best {
                continue;
            }
            if let Some(heading) = heading
                && len2 > 1.0
            {
                let turn = ((((heading - (dx.atan2(dy) * 180.0) / PI) % 180.0) + 180.0).abs()) % 180.0;
                if js_min(turn, 180.0 - turn) > ALONG_DEG {
                    continue;
                }
            }
            best = d;
            level = if s.tunnel {
                Level { tunnel: true, height: 0.0 }
            } else {
                Level { tunnel: false, height: reach.height(along[i - 1] + len2.sqrt() * t, along[along.len() - 1], RAIL_DECK_M) }
            };
        }
    }
    level
}

/// The course from one place to another, degrees from north (flat: fine over a few km).
fn course_of(a: LonLat, b: LonLat) -> f64 {
    ((b[0] - a[0]) * ((a[1] * PI) / 180.0).cos()).atan2(b[1] - a[1]) * 180.0 / PI
}

/// How high vertex `i` of `line` stands on the track, metres, exactly as [`on_structures`] lays it:
/// up on a bridge's deck, else (on the ground or in a tunnel) 0.
pub fn height_on(line: &[LonLat], i: usize, structures: &[Structure]) -> f64 {
    if structures.is_empty() || line.len() < 2 || i >= line.len() {
        return 0.0;
    }
    // on_structures reaches a vertex along the segment before it (the first along its own)
    let course = if i == 0 { course_of(line[0], line[1]) } else { course_of(line[i - 1], line[i]) };
    let level = level_among(line[i], Some(course), &measure_all(structures), LEVEL_WITHIN_M);
    if level.tunnel { 0.0 } else { level.height }
}

/// A stretch of a line on the track, in a tunnel or not ([`on_structures`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub tunnel: bool,
    /// `[lon, lat, metres up]`.
    pub path: Vec<[f64; 3]>,
}

/// A line on the track as it lies: cut where it goes into or out of a tunnel, and up on the
/// bridges along it.
pub fn on_structures(line: &[LonLat], structures: &[Structure]) -> Vec<Run> {
    if structures.is_empty() || line.len() < 2 {
        return vec![Run { tunnel: false, path: line.iter().map(|&[lon, lat]| [lon, lat, 0.0]).collect() }];
    }
    let measured = measure_all(structures);
    let mut out: Vec<Run> = Vec::new();
    let bridges: Vec<[f64; 4]> = measured.iter().filter(|m| !m.structure.tunnel).map(|m| m.bbox).collect();
    let pad = 10.0 / M_PER_DEGREE;
    for i in 1..line.len() {
        let (a, b) = (line[i - 1], line[i]);
        let course = course_of(a, b);
        // in pieces where it may be on a bridge, so it can rise along it
        let near_bridge = bridges.iter().any(|bbox| {
            js_max(a[0], b[0]) >= bbox[0] - pad * 2.0
                && js_min(a[0], b[0]) <= bbox[2] + pad * 2.0
                && js_max(a[1], b[1]) >= bbox[1] - pad
                && js_min(a[1], b[1]) <= bbox[3] + pad
        });
        let metres = js_hypot2((b[0] - a[0]) * ((a[1] * PI) / 180.0).cos(), b[1] - a[1]) * M_PER_DEGREE;
        let pieces = if near_bridge { js_max(1.0, (metres / PIECE_M).ceil()) } else { 1.0 };
        // `for (k = 0; k < pieces; k++)`
        let count = if pieces.is_finite() { pieces as usize } else { 0 };
        let at = |t: f64| -> LonLat { [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t] };
        for k in 0..count {
            let k = k as f64;
            let (from, to) = (at(k / pieces), at((k + 1.0) / pieces));
            let tunnel = level_among(at((k + 0.5) / pieces), Some(course), &measured, LEVEL_WITHIN_M).tunnel;
            let point = |q: LonLat| [q[0], q[1], level_among(q, Some(course), &measured, LEVEL_WITHIN_M).height];
            match out.last_mut() {
                Some(last) if last.tunnel == tunnel => last.path.push(point(to)),
                _ => out.push(Run { tunnel, path: vec![point(from), point(to)] }),
            }
        }
    }
    out
}


#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;
    use serde_json::json;

    /// East along 60°N: a degree of longitude is about 55.66 km.
    const DEG_PER_M: f64 = 1.0 / 55_660.0;

    fn at(m: f64) -> LonLat {
        at_north(m, 0.0)
    }

    fn at_north(m: f64, north: f64) -> LonLat {
        [18.0 + m * DEG_PER_M, 60.0 + north / 111_320.0]
    }

    fn structure(tunnel: bool, from: f64, to: f64, joined: [bool; 2]) -> Structure {
        Structure { tunnel, line: vec![at(from), at(to)], joined }
    }

    /// A 300 m bridge from 1000 m east, and a tunnel from 2000 m to 2500 m.
    fn level_structures() -> [Structure; 2] {
        [structure(false, 1000.0, 1300.0, [false, false]), structure(true, 2000.0, 2500.0, [false, false])]
    }

    #[test]
    fn a_ramp_climbs_at_the_grade_and_meets_the_ground() {
        let full = RAIL_DECK_M / GRADE;
        // a long bridge: on the ground at its ends, up to its deck past its ramps
        assert_eq!(rise(0.0, 400.0, RAIL_DECK_M, [false, false]), 0.0);
        assert_eq!(rise(400.0, 400.0, RAIL_DECK_M, [false, false]), 0.0);
        assert_eq!(rise(full, 400.0, RAIL_DECK_M, [false, false]), RAIL_DECK_M);
        assert_eq!(rise(200.0, 400.0, RAIL_DECK_M, [false, false]), RAIL_DECK_M);
        assert_close!(rise(full / 2.0, 400.0, RAIL_DECK_M, [false, false]), RAIL_DECK_M / 2.0, 9);
        // never steeper than the smoothstep's 1.5 times the grade
        for total in [20.0, 60.0, 150.0, 400.0] {
            let mut last = 0.0;
            for k in 1..=200 {
                let d = total * f64::from(k) / 200.0;
                let h = rise(d, total, RAIL_DECK_M, [false, false]);
                assert!((h - last).abs() <= GRADE * 1.5 * total / 200.0 + 1e-9, "{total} m at {d}");
                last = h;
            }
        }
        // a short bridge climbs only as far as its ramps allow
        assert_close!(rise(10.0, 20.0, RAIL_DECK_M, [false, false]), 20.0 * RAMP_SHARE * GRADE, 9);
        // an end where it goes on is up at its deck; both: the deck all along
        assert_eq!(rise(0.0, 200.0, RAIL_DECK_M, [true, false]), RAIL_DECK_M);
        assert_eq!(rise(200.0, 200.0, RAIL_DECK_M, [true, false]), 0.0);
        assert_eq!(rise(7.0, 20.0, RAIL_DECK_M, [true, true]), RAIL_DECK_M);
        // a deck that cannot be read is on the ground
        assert_eq!(rise(100.0, 400.0, f64::NAN, [false, false]), 0.0);
        assert_eq!(rise(100.0, 400.0, 0.0, [true, true]), 0.0);
    }

    /// Pieces that share an end are one bridge: no step where they meet, however short each is.
    #[test]
    fn pieces_that_meet_rise_as_one_bridge() {
        // 20 m, 160 m and 20 m in a row: as one 200 m bridge
        let lengths = [20.0, 160.0, 20.0];
        let r = reaches(&lengths, &[[0, 1], [1, 2], [2, 3]], |_, _| true);
        let whole = |d: f64| rise(d, 200.0, RAIL_DECK_M, [false, false]);
        for (piece, start) in [(0, 0.0), (1, 20.0), (2, 180.0)] {
            for d in [0.0, 5.0, lengths[piece]] {
                assert_close!(r[piece].height(d, lengths[piece], RAIL_DECK_M), whole(start + d), 9);
            }
        }
        // a fork on the deck: the branch is as high as the deck where it leaves it
        let r = reaches(&[100.0, 100.0, 30.0], &[[0, 1], [1, 2], [1, 3]], |_, _| true);
        assert_close!(r[2].height(0.0, 30.0, RAIL_DECK_M), r[0].height(100.0, 100.0, RAIL_DECK_M), 9);
        // no end on the ground: the deck all along
        let r = reaches(&[10.0], &[[0, 1]], |_, _| false);
        assert_eq!(r[0].height(5.0, 10.0, RAIL_DECK_M), RAIL_DECK_M);
    }

    #[test]
    fn on_the_ground_off_them() {
        assert_eq!(level_at(at(500.0), Some(90.0), &level_structures()), Level { tunnel: false, height: 0.0 });
    }

    #[test]
    fn up_on_a_bridge_as_its_deck_rises() {
        let structures = level_structures();
        for m in [1000.0, 1010.0, 1040.0, 1100.0, 1150.0, 1290.0, 1300.0] {
            let want = rise(m - 1000.0, 300.0, RAIL_DECK_M, [false, false]);
            assert_close!(level_at(at(m), Some(90.0), &structures).height, want, 6);
        }
        assert_close!(level_at(at(1150.0), Some(270.0), &structures).height, RAIL_DECK_M, 6);
        assert_close!(level_at(at(1000.0), Some(90.0), &structures).height, 0.0, 6);
    }

    #[test]
    fn an_end_where_the_bridge_goes_on_stays_up() {
        let cut = Structure { joined: [true, false], ..level_structures()[0].clone() };
        assert_close!(level_at(at(1000.0), Some(90.0), &[cut]).height, RAIL_DECK_M, 5);
    }

    #[test]
    fn in_a_tunnel() {
        assert!(level_at(at(2200.0), Some(90.0), &level_structures()).tunnel);
    }

    #[test]
    fn not_a_bridge_it_passes_under_nor_a_track_beside_it() {
        let structures = level_structures();
        assert_eq!(level_at(at(1100.0), Some(0.0), &structures).height, 0.0);
        assert_eq!(level_at(at_north(1100.0, 4.5), Some(90.0), &structures).height, 0.0);
    }

    fn track_structures() -> [Structure; 2] {
        [structure(false, 100.0, 300.0, [false, false]), structure(true, 400.0, 500.0, [false, false])]
    }

    #[test]
    fn cut_where_it_goes_into_and_out_of_a_tunnel() {
        let runs = on_structures(&[at(0.0), at(600.0)], &track_structures());
        assert_eq!(runs.iter().map(|r| r.tunnel).collect::<Vec<_>>(), [false, true, false]);
        let metres = |lon: f64| (lon - 18.0) / DEG_PER_M;
        assert_close!(metres(runs[1].path[0][0]), 400.0, -1);
        assert_close!(metres(runs[1].path[runs[1].path.len() - 1][0]), 500.0, -1);
    }

    #[test]
    fn a_line_along_a_bridge_follows_the_same_deck() {
        let runs = on_structures(&[at(0.0), at(600.0)], &track_structures());
        let metres = |p: &[f64; 3]| (p[0] - 18.0) / DEG_PER_M;
        for p in &runs[0].path {
            let m = metres(p);
            let want = if (100.0..=300.0).contains(&m) { rise(m - 100.0, 200.0, RAIL_DECK_M, [false, false]) } else { 0.0 };
            assert_close!(p[2], want, 6);
        }
        let top = runs[0].path.iter().map(|p| p[2]).fold(f64::NEG_INFINITY, js_max);
        assert!(top > RAIL_DECK_M * 0.9 && top <= RAIL_DECK_M, "{top}");
    }

    #[test]
    fn nothing_to_cut_without_structures() {
        let [a, b] = [at(0.0), at(500.0)];
        assert_eq!(on_structures(&[a, b], &[]), [Run { tunnel: false, path: vec![[a[0], a[1], 0.0], [b[0], b[1], 0.0]] }]);
    }

    const TILE: TileIndex = TileIndex { x: 2251, y: 1185, z: 12 };

    fn lines(paths: &[(&[f64], Value)]) -> Lines {
        let mut l = Lines { kind: "LineString".into(), size: 2, path_indices: vec![0], ..Lines::default() };
        for (i, (points, props)) in paths.iter().enumerate() {
            l.positions.extend_from_slice(points);
            let n = points.len() / 2;
            l.feature_ids.extend(std::iter::repeat_n(i as u32, n));
            l.global_feature_ids.extend(std::iter::repeat_n(i as u32, n));
            l.path_indices.push((l.positions.len() / 2) as u32);
            l.properties.push(props.as_object().unwrap().clone());
        }
        l
    }

    fn heights(l: &Lines) -> Vec<Vec<f64>> {
        l.path_indices.windows(2).map(|w| (w[0] as usize..w[1] as usize).map(|i| l.positions[i * 3 + 2]).collect()).collect()
    }

    #[test]
    fn a_tiles_bridges_rise_and_the_rest_stays_on_the_ground() {
        let m = tile_metres(TILE);
        let x = |metres: f64| 0.3 + metres / m;
        let l = lines(&[
            (&[0.1, 0.1, 0.2, 0.15], json!({ "class": "minor" })),
            // a 400 m road bridge a level up
            (&[x(0.0), 0.5, x(400.0), 0.5], json!({ "brunnel": "bridge", "class": "minor", "layer": 2 })),
            // a rail bridge in two pieces that meet: up where they meet
            (&[x(0.0), 0.6, x(150.0), 0.6], json!({ "brunnel": "bridge", "class": "rail" })),
            (&[x(150.0), 0.6, x(300.0), 0.6], json!({ "brunnel": "bridge", "class": "rail" })),
            // one the tile's edge cuts, and one whose deck cannot be read
            (&[0.9995, 0.7, 0.98, 0.7], json!({ "brunnel": "bridge", "class": "rail" })),
            (&[0.1, 0.9, 0.2, 0.9], json!({ "brunnel": "bridge", "layer": "x" })),
        ]);
        let parts = bridge_parts(&l, TILE, &omt_deck).unwrap();
        assert_eq!(parts.ground.positions, [0.1, 0.1, 0.2, 0.15, 0.1, 0.9, 0.2, 0.9]);
        let h = heights(&parts.raised);
        assert_eq!(h.len(), 4);
        // pieces of at most 6 m, on the ground at the ends, 12 m up in the middle
        assert_eq!(h[0].len(), (400.0 / PIECE_M).ceil() as usize + 1);
        assert_eq!((h[0][0], *h[0].last().unwrap()), (0.0, 0.0));
        assert_close!(h[0][h[0].len() / 2], 12.0, 6);
        // the two halves meet up at the deck, no step
        assert_eq!((h[1][0], *h[1].last().unwrap()), (0.0, RAIL_DECK_M));
        assert_eq!((h[2][0], *h[2].last().unwrap()), (RAIL_DECK_M, 0.0));
        // up at the tile's edge
        assert_eq!(h[3][0], RAIL_DECK_M);
        // nothing to raise: none
        assert_eq!(bridge_parts(&lines(&[(&[0.1, 0.1, 0.2, 0.15], json!({ "class": "minor" }))]), TILE, &omt_deck), None);
    }

    #[test]
    fn a_bridge_stands_on_its_structure() {
        let m = tile_metres(TILE);
        let l = lines(&[(&[0.3, 0.5, 0.3 + 400.0 / m, 0.5], json!({ "brunnel": "bridge", "class": "minor" }))]);
        let parts = bridge_parts(&l, TILE, &omt_deck).unwrap();
        let path: Vec<[f64; 3]> = parts.raised.positions.chunks(3).map(|p| [p[0], p[1], p[2]]).collect();
        let mut s = Structures::default();
        s.push(&path, 10.0, false, m);
        assert!(!s.is_empty());
        assert_eq!(s.indices.len() % 6, 0);
        let zs: Vec<f32> = s.vertices.iter().map(|v| v.position[2]).collect();
        // nothing under the ground; the parapets at most their height over the deck
        assert!(zs.iter().all(|&z| z >= -(UNDER_PAINT_M as f32)), "{zs:?}");
        let top = zs.iter().copied().fold(f32::MIN, f32::max);
        assert_close!(f64::from(top), DECK_M + PARAPET_M, 4);
        // as wide as the deck: 5 m either side of its line
        let v = |p: [f32; 3]| (f64::from(p[1]) - 0.5) * m;
        assert!(s.vertices.iter().all(|x| v(x.position).abs() <= 5.0 + 1e-3));
        // piers on the ground under the middle (none near the ends)
        let feet: Vec<f64> = s.vertices.iter().filter(|x| x.position[2] == 0.0 && x.color == PIER).map(|x| (f64::from(x.position[0]) - 0.3) * m).collect();
        assert!(!feet.is_empty());
        assert!(feet.iter().all(|&d| d > 60.0 && d < 340.0), "{feet:?}");
        // a bridge on the ground has no structure
        let mut none = Structures::default();
        none.push(&[[0.1, 0.1, 0.0], [0.2, 0.1, 0.0]], 10.0, false, m);
        assert!(none.is_empty());
    }

    #[test]
    fn the_decks_of_openmaptiles_and_openrailwaymap() {
        let deck = |p: Value| omt_deck(p.as_object().unwrap(), &|_| None);
        assert_eq!(deck(json!({})), 0.0);
        assert_eq!(deck(json!({ "brunnel": "bridge" })), 6.0);
        assert_eq!(deck(json!({ "brunnel": "bridge", "layer": 3 })), 18.0);
        assert_eq!(deck(json!({ "brunnel": "bridge", "layer": "7" })), 24.0);
        assert!(deck(json!({ "brunnel": "bridge", "layer": "x" })).is_nan());
        assert_eq!(deck(json!({ "brunnel": "bridge", "layer": null })), 6.0);
        assert_eq!(deck(json!({ "brunnel": "bridge", "layer": true })), 6.0);
        assert_eq!(deck(json!({ "brunnel": "bridge", "layer": " 0x2 " })), 12.0);
        assert_eq!(deck(json!({ "brunnel": "bridge", "layer": "" })), 6.0);
        assert_eq!(deck(json!({ "brunnel": "bridge", "class": "transit" })), 6.0);
        assert_eq!(deck(json!({ "brunnel": "tunnel", "class": "rail" })), 0.0);
        // a numeric column wins over the property
        assert_eq!(omt_deck(json!({ "brunnel": "bridge", "layer": 9 }).as_object().unwrap(), &|_| Some(2.5)), 15.0);
        let orm = |p: Value| orm_deck(p.as_object().unwrap(), &|_| None);
        assert_eq!(orm(json!({ "bridge": true })), RAIL_DECK_M);
        assert_eq!(orm(json!({ "bridge": "yes" })), 0.0);
        assert_eq!(orm(json!({})), 0.0);
    }

    #[test]
    fn strings_read_as_javascripts_number_reads_them() {
        for (s, n) in [("", 0.0), ("  ", 0.0), ("2", 2.0), (" 2.5 ", 2.5), ("-3", -3.0), ("+4", 4.0), (".5", 0.5), ("5.", 5.0), ("1e2", 100.0), ("0x1f", 31.0), ("0b101", 5.0), ("0o17", 15.0), ("Infinity", f64::INFINITY), ("-Infinity", f64::NEG_INFINITY)] {
            assert_eq!(string_to_number(s), n, "{s:?}");
        }
        for s in ["x", "1e", "inf", "NaN", "nan", "infinity", "0x", "-0x1", "1_000", "1 2", "--1", "."] {
            assert!(string_to_number(s).is_nan(), "{s:?}");
        }
    }
}
