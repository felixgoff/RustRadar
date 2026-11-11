//! Port of `bridges.ts` (S1.24; doc 02 §9, doc 05 §12.8): bridges in the air and tunnels
//! underground.
//!
//! A vector tile's lines split into those on the ground and those on a bridge, the bridges raised
//! to their deck (rising to it from each end, so they meet the ground where their ways do), and a
//! copy of them flat, for their shadow on what they cross. For tiles in the flat projection, whose
//! coordinates are the tile's own (0 to 1 across it).
//!
//! And the app's own bridges and tunnels on the track ([`Structure`], from the backend's tiles),
//! for what is drawn on it: the signal blocks and the trains.
//!
//! Transliterated as written, with these differences:
//! - raised heights are **metres** (the TypeScript multiplies them into deck.gl's common units with
//!   `512 / (circumference · cos lat)`; the native tile meshes carry z in metres, doc 02 §9.2 and
//!   doc 00 §1.4.2);
//! - no caches: the TypeScript keeps each result in a `WeakMap` by input object; callers here
//!   keep what they need per tile;
//! - [`Lines`] carries numbers as `f64` and ids as `u32`, as the typed arrays do; a read past the
//!   end of an input column reads NaN (or 0 for an id), where JavaScript reads `undefined`;
//! - a GeoJSON line without points is left out, where the TypeScript throws; a segment whose
//!   piece count is infinite (an infinite coordinate) is not cut, where it would never return;
//! - a feature whose coordinates do not match its geometry type is left on the ground.
//!
//! `Math.min`/`Math.max`/`Math.hypot` are the geometry kernel's NaN-propagating versions.
//!
//! **Draw lists** (item T7; doc 02 §8.1, §8.2, §9.2): the rail map's bridges on the flat map, what
//! the map's rail layer (`rustradar_map`'s `layers::rail`) draws in place of a tile's lines there
//! when the tile has bridges. Today's app splits them off only where deck's tiles are the flat
//! map's (`renderNetworkTile` on loaders.gl's binary tiles, `renderSpeedTile` when the tile is in
//! local coordinates, rail-layer.ts:134-143, 413-424); on the globe every line is drawn as it is
//! (`crate::rail`'s tiles). Each tile then draws as two sublayers:
//! - the lines on the ground: the tile's lines without its bridges, in their order, styled as
//!   `crate::rail` styles them ([`RaisedRail::ground`]);
//! - `-bridges` (`RAISED_NO_CULL`: depth test always, so over the tile's other lines whatever
//!   their order): the bridges ([`RaisedRail::raised`]), OpenFreeMap's network with [`omt_deck`]
//!   through [`bridge_parts`] ([`network_bridges`]), the speed map's tracks with [`orm_deck`]
//!   through [`raise_features`] ([`speed_bridges`]): up to their 6 m decks over 40 m (less on a
//!   short bridge) from each end the tile does not cut, in pieces of at most 6 m.
//!
//! **How high, as today.** The TypeScript writes those heights in deck's common units
//! (`× 512 / (circumference · cos φ)` at the tile's middle), but the sublayers are deck's
//! `CARTESIAN` tiles, whose `z` deck reads as metres (`commonUnitsPerWorldUnit[2] =
//! unitsPerMeter`): today's bridges stand [`rail_map_lift`] metres up for each metre of deck, a
//! 6 m deck 0.15 mm at Stockholm, on the ground to the eye, and do not rise at their ends (seen in
//! today's app at Söderström, pitched: the main line's bridge where its ground is, no ramp at the
//! tunnel's mouth). The draw lists keep that: the deck heights of `bridge_parts` and
//! `raise_features` (metres) times [`rail_map_lift`]. What shows of the split is the order: a
//! tile's bridges are drawn after its other lines. (The trains and the signal blocks on the
//! track, from the backend's structures with [`level_at`] and [`on_structures`], are in
//! longitude and latitude, whose heights deck reads as metres: they do stand 6 m up on a bridge.)
//!
//! The flat map's tiles are loaders.gl's (in the tile's units, flat: no 5° cut): the network's
//! binary (open paths, each part of a multi-line its own), the speed map's GeoJSON (a line whose
//! last point is its first a loop). On the flat map MVTLayer's `ClipExtension` cuts each tile's
//! lines at its square, fragment by fragment, by where they stand (`x`, `y`), not where they show.
//! The ground lines are the layer's to clip (the stencil, as the network's other lines); a raised
//! line shows higher up the screen than the ground it stands on, so the layer's stencil square
//! would cut it in the wrong place: its pieces are cut at the tile's square here instead, and
//! where the square cut a line the segment runs on into the vertex beyond (a straight joint, no
//! cap: the next tile's piece starts where this one ends). Deviations, none visible: the cut end
//! is square to the line, not along the tile's edge (the next tile's start fills the same sliver);
//! a dash fits whole periods to the cut segment rather than to the whole one.

use std::f64::consts::PI;

use indexmap::IndexMap;
use rustradar_core::geo::EARTH_CIRCUMFERENCE_M;
use rustradar_core::mvt::{self, GeomType};
use rustradar_core::num::clamp01;
use serde_json::Value;

use crate::geom::path::{Colors, END_CAP, LocalMercator, Paint, PathBuilder, PathSegment, PathType, START_CAP};
use crate::geom::{js_hypot2, js_max, js_min};
use crate::rail::{NETWORK_TILE_LAYER, SPEED_TILE_LAYER, rail_color, rail_dash, rail_speed_color, rail_speed_dash, rail_speed_width, rail_width};
pub use crate::tess::vector::Cancelled;

pub use rustradar_core::model::train::Structure;

/// Up to its deck over this much of each end, metres (less on a short bridge).
const RAMP_M: f64 = 40.0;
/// A raised line is cut into pieces no longer than this, so it can rise along them, metres.
const PIECE_M: f64 = 6.0;

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

/// How high a bridge is `d` metres along it, of `total`: up to its deck over a ramp at each end,
/// but not at an end where it goes on (`on`).
fn rise(d: f64, total: f64, deck: f64, on: [bool; 2]) -> f64 {
    let ramp = js_min(RAMP_M, total * 0.3);
    if ramp <= 0.0 {
        return deck;
    }
    let from_start = if on[0] { 1.0 } else { d / ramp };
    let from_end = if on[1] { 1.0 } else { (total - d) / ramp };
    let k = js_max(0.0, js_min(1.0, js_min(from_start, from_end)));
    deck * k * k * (3.0 - 2.0 * k)
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
    /// The bridges, three numbers a vertex, the third the height in metres, cut into pieces no
    /// longer than 6 m so they can rise along them.
    pub raised: Lines,
    /// The bridges flat, as they were, for their shadows.
    pub under: Lines,
}

/// `lines` split by `height`: those on the ground, the bridges raised (or `None` when there are
/// none, for the layers to be left as they were). Paths of fewer than two vertices are dropped.
/// An end of a bridge within 0.001 of the tile's edge stays up: the rest of it is the next tile's.
pub fn bridge_parts(lines: &Lines, tile: TileIndex, height: &DeckHeight) -> Option<BridgeParts> {
    let size = lines.size;
    if size < 2 {
        return None;
    }
    let at = |k: usize| lines.positions.get(k).copied().unwrap_or(f64::NAN);
    let paths = &lines.path_indices;
    let tile_metres = tile_metres(tile);
    let mut ground = Builder::new(lines, size);
    let mut raised = Builder::new(lines, 3);
    let mut under = Builder::new(lines, size);
    let empty = Properties::new();
    let mut any = false;
    for p in 0..paths.len().saturating_sub(1) {
        let (start, end) = (paths[p] as usize, paths[p + 1] as usize);
        if end < start + 2 {
            continue;
        }
        let props = lines.feature_ids.get(start).and_then(|&f| lines.properties.get(f as usize)).unwrap_or(&empty);
        let numeric = |name: &str| lines.numeric_props.get(name).and_then(|c| c.get(start).copied());
        let deck = height(props, &numeric);
        let third = |i: usize| (size == 3).then(|| at(i * size + 2));
        if deck <= 0.0 {
            for i in start..end {
                ground.push(i, at(i * size), at(i * size + 1), third(i));
            }
            ground.end();
            continue;
        }
        any = true;
        // its length, and how far along each vertex is
        let mut along = vec![0.0];
        for i in start + 1..end {
            let (dx, dy) = (at(i * size) - at((i - 1) * size), at(i * size + 1) - at((i - 1) * size + 1));
            along.push(along[along.len() - 1] + js_hypot2(dx, dy) * tile_metres);
        }
        let total = along[along.len() - 1];
        // an end where the tile cuts the bridge stays up: the rest of it is the next tile's
        let cut = |i: usize| {
            let (x, y) = (at(i * size), at(i * size + 1));
            x <= 0.001 || x >= 0.999 || y <= 0.001 || y >= 0.999
        };
        let on = [cut(start), cut(end - 1)];
        let up = |d: f64| rise(d, total, deck, on);
        for i in start..end {
            let (x, y) = (at(i * size), at(i * size + 1));
            under.push(i, x, y, third(i));
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
        under.end();
    }
    any.then(|| BridgeParts { ground: ground.build(), raised: raised.build(), under: under.build() })
}

/// `for (k = 1; k < pieces; k++)` as a range end: 1 for NaN and for an infinite count.
fn below(pieces: f64) -> usize {
    if pieces.is_finite() && pieces > 1.0 { pieces as usize } else { 1 }
}

/// A point of a GeoJSON line: x, y and anything after.
pub type Coord = Vec<f64>;

/// GeoJSON coordinates, by how deeply they nest.
#[derive(Debug, Clone, PartialEq)]
pub enum Coordinates {
    /// `Point`.
    Point(Coord),
    /// `LineString`, `MultiPoint`.
    Line(Vec<Coord>),
    /// `MultiLineString`, `Polygon`.
    Lines(Vec<Vec<Coord>>),
    /// `MultiPolygon`.
    Polygons(Vec<Vec<Vec<Coord>>>),
}

/// A feature's geometry: its GeoJSON type and coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    pub kind: String,
    pub coordinates: Coordinates,
}

/// A tile's GeoJSON feature, in the tile's own coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct LineFeature {
    pub geometry: Geometry,
    pub properties: Properties,
}

/// A tile's features split by their deck height ([`raise_features`]).
#[derive(Debug, Clone, PartialEq)]
pub struct RaisedFeatures {
    pub ground: Vec<LineFeature>,
    /// The bridges, each point `[x, y, metres up]`, cut into pieces no longer than 6 m.
    pub raised: Vec<LineFeature>,
}

/// The same as [`bridge_parts`] for a tile's GeoJSON features (in its own coordinates): those on
/// the ground, and the bridges' lines raised to their decks (or `None` when there are none). Only
/// `LineString` and `MultiLineString` features are raised; `numeric` reads a property that is a
/// number.
pub fn raise_features(features: &[LineFeature], tile: TileIndex, height: &DeckHeight) -> Option<RaisedFeatures> {
    let tile_metres = tile_metres(tile);
    let c = |p: &Coord, k: usize| p.get(k).copied().unwrap_or(f64::NAN);
    let cut = |p: &Coord| c(p, 0) <= 0.001 || c(p, 0) >= 0.999 || c(p, 1) <= 0.001 || c(p, 1) >= 0.999;
    let raise = |line: &Vec<Coord>, deck: f64| -> Vec<Coord> {
        let (Some(first), Some(last)) = (line.first(), line.last()) else { return Vec::new() };
        let mut along = vec![0.0];
        for i in 1..line.len() {
            along.push(along[i - 1] + js_hypot2(c(&line[i], 0) - c(&line[i - 1], 0), c(&line[i], 1) - c(&line[i - 1], 1)) * tile_metres);
        }
        let total = along[along.len() - 1];
        let on = [cut(first), cut(last)];
        let up = |d: f64| rise(d, total, deck, on);
        let mut out = vec![vec![c(first, 0), c(first, 1), up(0.0)]];
        for i in 1..line.len() {
            let pieces = js_max(1.0, ((along[i] - along[i - 1]) / PIECE_M).ceil());
            // `for (k = 1; k <= pieces; k++)`
            let count = if pieces.is_finite() { pieces as usize } else { 0 };
            for k in 1..=count {
                let t = k as f64 / pieces;
                let (a, b) = (&line[i - 1], &line[i]);
                out.push(vec![
                    c(a, 0) + (c(b, 0) - c(a, 0)) * t,
                    c(a, 1) + (c(b, 1) - c(a, 1)) * t,
                    up(along[i - 1] + (along[i] - along[i - 1]) * t),
                ]);
            }
        }
        out
    };
    let (mut ground, mut raised) = (Vec::new(), Vec::new());
    for f in features {
        let numeric = |name: &str| f.properties.get(name).and_then(Value::as_f64);
        let deck = height(&f.properties, &numeric);
        let coordinates = match (f.geometry.kind.as_str(), &f.geometry.coordinates) {
            _ if deck <= 0.0 => None,
            ("LineString", Coordinates::Line(line)) => Some(Coordinates::Line(raise(line, deck))),
            ("MultiLineString", Coordinates::Lines(lines)) => Some(Coordinates::Lines(lines.iter().map(|l| raise(l, deck)).collect())),
            _ => None,
        };
        match coordinates {
            None => ground.push(f.clone()),
            Some(coordinates) => raised.push(LineFeature {
                geometry: Geometry { kind: f.geometry.kind.clone(), coordinates },
                properties: f.properties.clone(),
            }),
        }
    }
    (!raised.is_empty()).then_some(RaisedFeatures { ground, raised })
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
    Measured { structure: s, bbox, along }
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
    let measured: Vec<Measured> = structures.iter().map(measure).collect();
    level_among(p, heading, &measured, within)
}

fn level_among(p: LonLat, heading: Option<f64>, structures: &[Measured], within: f64) -> Level {
    let k = ((p[1] * PI) / 180.0).cos();
    let pad = within / M_PER_DEGREE;
    let mut best = f64::INFINITY;
    let mut level = GROUND;
    for Measured { structure: s, bbox, along } in structures {
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
                Level { tunnel: false, height: rise(along[i - 1] + len2.sqrt() * t, along[along.len() - 1], RAIL_DECK_M, s.joined) }
            };
        }
    }
    level
}

/// The course from one place to another, degrees from north (flat: fine over a few km).
fn course_of(a: LonLat, b: LonLat) -> f64 {
    ((b[0] - a[0]) * ((a[1] * PI) / 180.0).cos()).atan2(b[1] - a[1]) * 180.0 / PI
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
    let measured: Vec<Measured> = structures.iter().map(measure).collect();
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

// --- the rail map's bridges on the flat map (item T7) --------------------------

/// The flat map draws its rail tiles from this zoom (deck's rule picks z12 from just above zoom
/// 12, where the flat map begins; the speed map's tiles stop at z12): the tiles worth raising.
pub const FLAT_MIN_Z: u32 = 12;

/// One rail tile as the flat map draws it when it has bridges (see the module notes), in the
/// tile's units ([`LocalMercator::tile`]: `u` east, `v` south, metres up), widths in pixels before
/// the layer's width scale, as `crate::rail::RailTile`'s lines.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RaisedRail {
    /// The tile's lines on the ground: every drawn line but the bridges, in the tile's order.
    pub ground: Vec<PathSegment>,
    /// `-bridges`: the bridges at the height today's app draws them ([`rail_map_lift`] of their
    /// decks), cut at the tile's square.
    pub raised: Vec<PathSegment>,
}

impl RaisedRail {
    /// The bytes it holds (for the tile cache's budget).
    pub fn byte_size(&self) -> usize {
        size_of_val(self.ground.as_slice()) + size_of_val(self.raised.as_slice())
    }
}

/// How high today's app draws the rail map's bridges on the flat map, metres for each metre of
/// deck, at `tile` (see the module notes): `bridgeParts`' and `raiseFeatures`' common units per
/// metre at the tile's middle, `512 / (circumference · cos φ)`, which deck reads as metres. About
/// 2.5e-5 at Stockholm.
pub fn rail_map_lift(tile: TileIndex) -> f64 {
    let n = 2f64.powi(tile.z as i32);
    let lat = (PI * (1.0 - (2.0 * (f64::from(tile.y) + 0.5)) / n)).sinh().atan();
    512.0 / (EARTH_CIRCUMFERENCE_M * lat.cos())
}

/// `path` (`[u, v, metres of deck]`) at the height today's app draws it.
fn lifted(mut path: Vec<[f64; 3]>, lift: f64) -> Vec<[f64; 3]> {
    for p in &mut path {
        p[2] *= lift;
    }
    path
}

/// A feature's properties as loaders.gl hands them to the accessors: its tags, and `layerName`
/// (the MVT loader's `layerProperty`), as `crate::rail` reads them.
fn properties_of(feature: &mvt::Feature<'_>, layer: &str) -> Properties {
    let mut p = feature.json_properties();
    p.insert("layerName".into(), Value::from(layer));
    p
}

/// The network's paint of a line (`railNetworkLayer`'s accessors), an open path.
fn network_paint(p: &Properties) -> Paint<'static> {
    Paint { color: Colors::Same(rail_color(p)), width: rail_width(p), dash_array: rail_dash(p), path_type: PathType::Open }
}

/// The speed map's paint of a track (`railSpeedLayer`'s accessors), a GeoJSON line.
fn speed_paint(p: &Properties) -> Paint<'static> {
    Paint { color: Colors::Same(rail_speed_color(p)), width: rail_speed_width(p), dash_array: rail_speed_dash(p), path_type: PathType::Auto }
}

/// The flat map's rail network of one OpenFreeMap tile (`bytes`) at `tile` with its bridges split
/// off and raised (`renderNetworkTile`: `bridgeParts(data.lines, tile, omtDeck)` on loaders.gl's
/// binary lines, heights as today's app draws them), or `None` when it has no bridge (the tile's
/// lines are drawn as they are). Only the lines `crate::rail::network_tile` draws are read (rail
/// and transit of a colour): what deck draws in no colour and 0 px wide adds nothing, and each
/// line is raised or not on its own. `cancelled` is asked between features.
pub fn network_bridges(tile: TileIndex, bytes: &[u8], cancelled: &dyn Fn() -> bool) -> Result<Option<RaisedRail>, Cancelled> {
    let mut lines = Lines { kind: "LineString".into(), size: 2, path_indices: vec![0], ..Lines::default() };
    for layer in mvt::layers_named(bytes, &[NETWORK_TILE_LAYER]) {
        for feature in layer.features() {
            if cancelled() {
                return Err(Cancelled);
            }
            if feature.kind != GeomType::LineString || !matches!(feature.text("class"), Some("rail" | "transit")) {
                continue;
            }
            let p = properties_of(&feature, layer.name);
            if rail_color(&p)[3] == 0 {
                continue;
            }
            let id = lines.properties.len() as u32;
            lines.properties.push(p);
            for line in feature.lines() {
                for &(x, y) in &line {
                    lines.positions.extend([x / layer.extent, y / layer.extent]);
                    lines.feature_ids.push(id);
                    lines.global_feature_ids.push(id);
                }
                lines.path_indices.push((lines.positions.len() / 2) as u32);
            }
        }
    }
    let Some(parts) = bridge_parts(&lines, tile, &omt_deck) else { return Ok(None) };
    let local = LocalMercator::tile(tile);
    let mut ground = PathBuilder::new(local);
    each_path(&parts.ground, |p, path| ground.push(&path, &network_paint(p)));
    let (mut raised, lift) = (Vec::new(), rail_map_lift(tile));
    each_path(&parts.raised, |p, path| push_inside(&mut raised, local, &lifted(path, lift), &network_paint(p)));
    Ok(Some(RaisedRail { ground: ground.finish(), raised }))
}

/// The flat map's speed map of one backend rail tile (`bytes`) at `tile` with its bridges split
/// off and raised (`renderSpeedTile`'s local tiles: `raiseFeatures(features, tile, ormDeck)` on
/// the tile's GeoJSON features in its units, heights as today's app draws them), or `None` when
/// it has no bridge. Tracks drawn in no colour are
/// left out; the speed labels are `crate::rail::speed_tile`'s, from the tracks as they lie.
/// `cancelled` is asked between features.
pub fn speed_bridges(tile: TileIndex, bytes: &[u8], cancelled: &dyn Fn() -> bool) -> Result<Option<RaisedRail>, Cancelled> {
    let mut features: Vec<LineFeature> = Vec::new();
    for layer in mvt::layers_named(bytes, &[SPEED_TILE_LAYER]) {
        for feature in layer.features() {
            if cancelled() {
                return Err(Cancelled);
            }
            // the GeoJsonLayer's points are 0 px and its polygons neither filled nor stroked
            if feature.kind != GeomType::LineString {
                continue;
            }
            let mut parts: Vec<Vec<Coord>> = feature.lines().iter().map(|l| l.iter().map(|&(x, y)| vec![x / layer.extent, y / layer.extent]).collect()).collect();
            let geometry = if parts.len() == 1 {
                Geometry { kind: "LineString".into(), coordinates: Coordinates::Line(parts.remove(0)) }
            } else {
                Geometry { kind: "MultiLineString".into(), coordinates: Coordinates::Lines(parts) }
            };
            features.push(LineFeature { geometry, properties: properties_of(&feature, layer.name) });
        }
    }
    let Some(parts) = raise_features(&features, tile, &orm_deck) else { return Ok(None) };
    let local = LocalMercator::tile(tile);
    let lines_of = |f: &LineFeature| -> Vec<Vec<[f64; 3]>> {
        let point = |c: &Coord| [c.first().copied().unwrap_or(f64::NAN), c.get(1).copied().unwrap_or(f64::NAN), c.get(2).copied().unwrap_or(0.0)];
        match &f.geometry.coordinates {
            Coordinates::Line(line) => vec![line.iter().map(point).collect()],
            Coordinates::Lines(lines) => lines.iter().map(|l| l.iter().map(point).collect()).collect(),
            Coordinates::Point(_) | Coordinates::Polygons(_) => Vec::new(),
        }
    };
    let mut ground = PathBuilder::new(local);
    for f in &parts.ground {
        let paint = speed_paint(&f.properties);
        if let Colors::Same(c) = paint.color
            && c[3] > 0
        {
            for line in lines_of(f) {
                ground.push(&line, &paint);
            }
        }
    }
    let (mut raised, lift) = (Vec::new(), rail_map_lift(tile));
    for f in &parts.raised {
        let paint = speed_paint(&f.properties);
        if let Colors::Same(c) = paint.color
            && c[3] > 0
        {
            for line in lines_of(f) {
                push_inside(&mut raised, local, &lifted(line, lift), &paint);
            }
        }
    }
    Ok(Some(RaisedRail { ground: ground.finish(), raised }))
}

/// Each path of `lines` with its feature's properties, as `[u, v, metres]`.
fn each_path(lines: &Lines, mut f: impl FnMut(&Properties, Vec<[f64; 3]>)) {
    let empty = Properties::new();
    let size = lines.size;
    for w in lines.path_indices.windows(2) {
        let (start, end) = (w[0] as usize, w[1] as usize);
        let p = lines.feature_ids.get(start).and_then(|&i| lines.properties.get(i as usize)).unwrap_or(&empty);
        let path = (start..end)
            .map(|i| {
                let at = |k: usize| lines.positions.get(i * size + k).copied().unwrap_or(f64::NAN);
                [at(0), at(1), if size == 3 { at(2) } else { 0.0 }]
            })
            .collect();
        f(p, path);
    }
}

/// A stretch of a path inside a tile's square ([`inside_square`]).
#[derive(Debug, Clone, PartialEq)]
struct Inside {
    points: Vec<[f64; 3]>,
    /// The vertex before the first point, where the square cut the path there.
    before: Option<[f64; 3]>,
    /// The vertex after the last point, where the square cut the path there.
    after: Option<[f64; 3]>,
}

/// The stretch `[t0, t1]` of the segment from `a` to `b` that lies in the square `[0, 1]²` of
/// `u` and `v` (edges included), or `None` (Liang–Barsky).
fn clip_segment(a: [f64; 3], b: [f64; 3]) -> Option<(f64, f64)> {
    let (du, dv) = (b[0] - a[0], b[1] - a[1]);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [(-du, a[0]), (du, 1.0 - a[0]), (-dv, a[1]), (dv, 1.0 - a[1])] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t1 {
                return None;
            }
            t0 = t0.max(r);
        } else {
            if r < t0 {
                return None;
            }
            t1 = t1.min(r);
        }
    }
    Some((t0, t1))
}

fn lerp(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * t)
}

/// `path` (tile units, metres up) cut at the tile's square by `u` and `v` alone, as deck's
/// `ClipExtension` cuts a tile's lines by where they stand: the stretches inside, in order, each
/// with the vertex beyond each end the square cut.
fn inside_square(path: &[[f64; 3]]) -> Vec<Inside> {
    let mut out = Vec::new();
    let mut current: Option<Inside> = None;
    for i in 1..path.len() {
        let (a, b) = (path[i - 1], path[i]);
        match clip_segment(a, b) {
            Some((t0, t1)) if t0 < t1 => {
                let part = current.get_or_insert_with(|| {
                    if t0 > 0.0 {
                        Inside { points: vec![lerp(a, b, t0)], before: Some(a), after: None }
                    } else {
                        // in from outside at a vertex on the edge, or the path's start
                        Inside { points: vec![a], before: (i > 1).then(|| path[i - 2]), after: None }
                    }
                });
                if t1 < 1.0 {
                    part.points.push(lerp(a, b, t1));
                    part.after = Some(b);
                    out.extend(current.take());
                } else {
                    part.points.push(b);
                }
            }
            // out from a vertex on the edge (or not in at all)
            _ => {
                if let Some(mut part) = current.take() {
                    part.after = Some(b);
                    out.push(part);
                }
            }
        }
    }
    out.extend(current);
    out
}

/// The segments of `path` in `paint` inside the tile's square: a path wholly inside as it is, a
/// stretch the square cuts open and running on into the vertex beyond each cut end (a straight
/// joint, no cap).
fn push_inside(out: &mut Vec<PathSegment>, local: LocalMercator, path: &[[f64; 3]], paint: &Paint<'_>) {
    for part in inside_square(path) {
        let mut b = PathBuilder::new(local);
        if part.before.is_none() && part.after.is_none() {
            b.push(&part.points, paint);
            out.extend(b.finish());
            continue;
        }
        b.push(&part.points, &Paint { path_type: PathType::Open, ..*paint });
        let mut segments = b.finish();
        if let (Some(q), Some(s)) = (part.before, segments.first_mut()) {
            s.left = q.map(|c| c as f32);
            s.types &= !START_CAP;
        }
        if let (Some(q), Some(s)) = (part.after, segments.last_mut()) {
            s.right = q.map(|c| c as f32);
            s.types &= !END_CAP;
        }
        out.extend(segments);
    }
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

    /// A 200 m bridge from 1000 m east, and a tunnel from 2000 m to 2500 m.
    fn level_structures() -> [Structure; 2] {
        [structure(false, 1000.0, 1200.0, [false, false]), structure(true, 2000.0, 2500.0, [false, false])]
    }

    #[test]
    fn on_the_ground_off_them() {
        assert_eq!(level_at(at(500.0), Some(90.0), &level_structures()), Level { tunnel: false, height: 0.0 });
    }

    #[test]
    fn up_on_a_bridge_rising_from_its_ends() {
        let structures = level_structures();
        assert_close!(level_at(at(1100.0), Some(90.0), &structures).height, RAIL_DECK_M, 5);
        assert_close!(level_at(at(1000.0), Some(90.0), &structures).height, 0.0, 5);
        let ramp = level_at(at(1020.0), Some(270.0), &structures).height;
        assert!(ramp > 0.0);
        assert!(ramp < RAIL_DECK_M);
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
        [structure(false, 100.0, 200.0, [false, false]), structure(true, 300.0, 400.0, [false, false])]
    }

    #[test]
    fn cut_where_it_goes_into_and_out_of_a_tunnel() {
        let runs = on_structures(&[at(0.0), at(500.0)], &track_structures());
        assert_eq!(runs.iter().map(|r| r.tunnel).collect::<Vec<_>>(), [false, true, false]);
        let metres = |lon: f64| (lon - 18.0) / DEG_PER_M;
        assert_close!(metres(runs[1].path[0][0]), 300.0, -1);
        assert_close!(metres(runs[1].path[runs[1].path.len() - 1][0]), 400.0, -1);
    }

    #[test]
    fn up_along_the_bridge_in_pieces() {
        let runs = on_structures(&[at(0.0), at(500.0)], &track_structures());
        let top = runs[0].path.iter().map(|p| p[2]).fold(f64::NEG_INFINITY, js_max);
        assert_close!(top, RAIL_DECK_M, 5);
        assert_eq!(runs[0].path[0][2], 0.0);
    }

    #[test]
    fn nothing_to_cut_without_structures() {
        let [a, b] = [at(0.0), at(500.0)];
        assert_eq!(on_structures(&[a, b], &[]), [Run { tunnel: false, path: vec![[a[0], a[1], 0.0], [b[0], b[1], 0.0]] }]);
    }

    const TILE: TileIndex = TileIndex { x: 2251, y: 1185, z: 12 };

    fn line(bridge: bool) -> LineFeature {
        LineFeature {
            geometry: Geometry { kind: "LineString".into(), coordinates: Coordinates::Line(vec![vec![0.2, 0.5], vec![0.4, 0.5]]) },
            properties: json!({ "bridge": bridge }).as_object().unwrap().clone(),
        }
    }

    #[test]
    fn bridges_up_the_rest_left_on_the_ground() {
        let parts = raise_features(&[line(false), line(true)], TILE, &orm_deck).unwrap();
        assert_eq!(parts.ground.len(), 1);
        assert_eq!(parts.raised.len(), 1);
        let Coordinates::Line(raised) = &parts.raised[0].geometry.coordinates else { panic!("not a line") };
        let z: Vec<f64> = raised.iter().map(|c| c[2]).collect();
        assert_eq!(z[0], 0.0);
        assert!(z.iter().copied().fold(f64::NEG_INFINITY, js_max) > 0.0);
    }

    #[test]
    fn none_without_bridges() {
        assert_eq!(raise_features(&[line(false)], TILE, &orm_deck), None);
    }
    /// The structures and samples of the parity tests below: a curved bridge, a tunnel and a
    /// bridge that goes on past its east end.
    fn curved_structures() -> [Structure; 3] {
        let line = |points: &[(f64, f64)]| points.iter().map(|&(m, n)| at_north(m, n)).collect();
        [
            Structure { tunnel: false, line: line(&[(100.0, 0.0), (150.0, 3.0), (210.0, 5.0), (260.0, 4.0)]), joined: [false, false] },
            Structure { tunnel: true, line: line(&[(300.0, 0.0), (400.0, -2.0)]), joined: [false, false] },
            Structure { tunnel: false, line: line(&[(450.0, -2.0), (520.0, -1.0)]), joined: [false, true] },
        ]
    }

    #[test]
    fn the_same_levels_as_bridges_ts() {
        // levelAt in bun at the same places (east, north, heading): every height the same
        let structures = curved_structures();
        #[rustfmt::skip]
        let samples: [(f64, f64, Option<f64>, bool, f64); 22] = [
            (101.89999999999999, -2.0, None, false, 0.0344649132218084),
            (109.19999999999999, 2.5, Some(260.0), false, 0.8275355620308407),
            (123.79999999999998, -2.0, Some(95.0), false, 3.8094140843116433),
            (131.1, 2.5, Some(80.0), false, 5.2556306485498965),
            (145.70000000000002, 2.5, None, false, 6.0),
            (160.30000000000004, 0.0, Some(260.0), false, 6.0),
            (174.90000000000006, 0.0, Some(95.0), false, 6.0),
            (189.50000000000009, 4.4, Some(80.0), false, 6.0),
            (211.40000000000012, 4.4, None, false, 6.0),
            (233.30000000000015, 2.5, Some(260.0), false, 4.444036912757059),
            (255.2000000000002, 2.5, Some(95.0), false, 0.23552812215008576),
            (306.30000000000024, -2.0, Some(80.0), true, 0.0),
            (320.90000000000026, -2.0, None, true, 0.0),
            (328.2000000000003, 2.5, Some(260.0), true, 0.0),
            (342.8000000000003, 2.5, Some(95.0), true, 0.0),
            (357.4000000000003, 2.5, Some(80.0), true, 0.0),
            (372.00000000000034, 2.5, None, true, 0.0),
            (393.9000000000004, -2.0, Some(260.0), true, 0.0),
            (459.6000000000005, -2.0, Some(95.0), false, 2.6143981323561536),
            (481.5000000000005, -2.0, Some(80.0), false, 6.0),
            (496.10000000000053, 0.0, None, false, 6.0),
            (510.70000000000056, -2.0, Some(260.0), false, 6.0),
        ];
        for (m, north, heading, tunnel, height) in samples {
            assert_eq!(level_at(at_north(m, north), heading, &structures), Level { tunnel, height }, "{m} {north} {heading:?}");
        }
        // crossing them, or too far to the side: the ground
        assert_eq!(level_at(at_north(145.7, 2.5), Some(10.0), &structures), GROUND);
        assert_eq!(level_at(at_north(330.0, 6.0), Some(95.0), &structures), GROUND);
    }

    #[test]
    fn the_same_runs_as_bridges_ts() {
        let line = [at_north(0.0, 0.0), at_north(130.0, 1.0), at_north(250.0, 4.2), at_north(380.0, -1.5), at_north(560.0, -1.0)];
        let runs = on_structures(&line, &curved_structures());
        assert_eq!(runs.iter().map(|r| (r.tunnel, r.path.len())).collect::<Vec<_>>(), [(false, 52), (true, 19), (false, 28)]);
        let (into, out_of) = ([18.005340868258582, 60.00001910952863, 0.0], [18.007244444959603, 59.99998710488797, 0.0]);
        assert_eq!((runs[0].path[0], runs[0].path[51]), ([18.0, 60.0, 0.0], into));
        assert_eq!((runs[1].path[0], runs[1].path[18]), (into, out_of));
        assert_eq!((runs[2].path[0], runs[2].path[27]), (out_of, [18.0100610851599, 59.99999101688825, 0.0]));
        assert_eq!(runs.iter().flat_map(|r| r.path.iter().map(|p| p[2])).fold(f64::NEG_INFINITY, js_max), RAIL_DECK_M);
    }

    #[test]
    fn a_tiles_lines_split_raised_and_kept_as_bridge_parts_splits_them() {
        let properties: Vec<Properties> = [
            json!({ "brunnel": "bridge", "class": "minor", "layer": "2" }),
            json!({ "class": "rail" }),
            json!({ "brunnel": "bridge", "class": "rail" }),
            json!({ "brunnel": "bridge" }),
        ]
        .into_iter()
        .map(|p| p.as_object().unwrap().clone())
        .collect();
        // a road on the ground, a road bridge a level up, a lone vertex, a rail bridge the tile's
        // edge cuts, and a bridge with no class
        #[rustfmt::skip]
        let positions = vec![
            0.1, 0.1, 0.2, 0.15,
            0.3, 0.3, 0.32, 0.31, 0.35, 0.33,
            0.5, 0.5, 0.6, 0.55,
            0.9995, 0.7, 0.98, 0.72, 0.95, 0.71,
            0.01, 0.9, 0.02, 0.91,
        ];
        let feature_ids = vec![1, 1, 0, 0, 0, 1, 1, 2, 2, 2, 3, 3];
        let lines = Lines {
            kind: "LineString".into(),
            positions,
            size: 2,
            path_indices: vec![0, 2, 5, 7, 10, 12],
            global_feature_ids: feature_ids.iter().map(|f| f + 100).collect(),
            feature_ids,
            numeric_props: IndexMap::from([("speed".to_string(), (0..12).map(|i| f64::from(i) * 10.0).collect())]),
            properties,
            fields: None,
        };
        // bridgeParts in bun on the same tile: the same columns, heights in metres
        let parts = bridge_parts(&lines, TILE, &omt_deck).unwrap();
        assert_eq!(parts.ground.positions, [0.1, 0.1, 0.2, 0.15, 0.5, 0.5, 0.6, 0.55]);
        assert_eq!((parts.ground.path_indices.as_slice(), parts.ground.feature_ids.as_slice()), (&[0, 2, 4][..], &[1, 1, 1, 1][..]));
        assert_eq!(parts.ground.numeric_props["speed"], [0.0, 10.0, 50.0, 60.0]);
        assert_eq!(parts.under.path_indices, [0, 3, 6, 8]);
        assert_eq!(parts.under.feature_ids, [0, 0, 0, 2, 2, 2, 3, 3]);
        assert_eq!(parts.under.global_feature_ids, [100, 100, 100, 102, 102, 102, 103, 103]);
        assert_eq!(parts.under.positions, lines.positions[4..10].iter().chain(&lines.positions[14..]).copied().collect::<Vec<_>>());
        assert_eq!((parts.raised.size, parts.raised.path_indices.as_slice()), (3, &[0, 50, 100, 113][..]));
        let ids: Vec<u32> = [(0, 50), (2, 50), (3, 13)].iter().flat_map(|&(id, n)| std::iter::repeat_n(id, n)).collect();
        assert_eq!(parts.raised.feature_ids, ids);
        // the pieces carry the attributes of the vertex they start from
        let speeds: Vec<f64> = [(20.0, 19), (30.0, 30), (40.0, 1), (70.0, 23), (80.0, 26), (90.0, 1), (100.0, 12), (110.0, 1)]
            .iter()
            .flat_map(|&(s, n)| std::iter::repeat_n(s, n))
            .collect();
        assert_eq!(parts.raised.numeric_props["speed"], speeds);
        let z: Vec<f64> = parts.raised.positions.chunks(3).map(|p| p[2]).collect();
        // the road bridge a level up (12 m) up and down its ramps; the rail bridge up at the
        // tile's edge, down at its other end; the last one 6 m up between its ramps
        let up = [0.0, 0.667503, 2.388274, 4.739702, 7.299181, 9.6441, 11.351851];
        let down = [11.502044, 9.86289, 7.512479, 4.900899, 2.478235, 0.694573, 0.0];
        let rail_down = [5.790278, 4.99284, 3.817378, 2.497128, 1.265325, 0.355204, 0.0];
        let last = [0.0, 1.131687, 3.497942, 5.555556, 6.0, 6.0, 6.0, 6.0, 6.0, 5.555556, 3.497942, 1.131687, 0.0];
        let expected: Vec<f64> = up
            .into_iter()
            .chain(std::iter::repeat_n(12.0, 36))
            .chain(down)
            .chain(std::iter::repeat_n(6.0, 43))
            .chain(rail_down)
            .chain(last)
            .collect();
        assert_eq!(z.len(), expected.len());
        for (got, want) in z.iter().zip(&expected) {
            assert_close!(*got, *want, 5);
        }
        // nothing to raise: none
        let flat = Lines { properties: vec![Properties::new(); 4], ..lines.clone() };
        assert_eq!(bridge_parts(&flat, TILE, &omt_deck), None);
    }

    #[test]
    fn raised_features_stay_up_where_the_tile_cuts_them() {
        let lines = vec![vec![vec![0.0005, 0.3], vec![0.1, 0.3]], vec![vec![0.6, 0.6], vec![0.6005, 0.6001]]];
        let bridge = LineFeature {
            geometry: Geometry { kind: "MultiLineString".into(), coordinates: Coordinates::Lines(lines) },
            properties: json!({ "bridge": true }).as_object().unwrap().clone(),
        };
        let point = LineFeature { geometry: Geometry { kind: "Point".into(), coordinates: Coordinates::Point(vec![0.2, 0.2]) }, ..bridge.clone() };
        let parts = raise_features(&[bridge, point.clone()], TILE, &orm_deck).unwrap();
        assert_eq!(parts.ground, [point]);
        let Coordinates::Lines(raised) = &parts.raised[0].geometry.coordinates else { panic!("not lines") };
        // from the tile's west edge: up already, down at its other end
        assert_eq!(raised[0][0], [0.0005, 0.3, RAIL_DECK_M]);
        assert_eq!(raised[0].last().unwrap()[2], 0.0);
        // 2.5 m in the middle of the tile: one piece, its ramps meet at the ground
        assert_eq!(raised[1], [vec![0.6, 0.6, 0.0], vec![0.6005, 0.6001, 0.0]]);
    }

    #[test]
    fn the_decks_of_openmaptiles_and_openrailwaymap() {
        let deck = |p: Value| omt_deck(p.as_object().unwrap(), &|_| None);
        // omtDeck in bun on the same properties
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

    // --- T7 -------------------------------------------------------------------------------

    /// A bridge's height `d` metres along it, of `total`, with ends that ramp (`on`: those that do
    /// not): smoothstep up to 6 m over 40 m (30 % of a short bridge) from each end that ramps.
    fn deck_at(d: f64, total: f64, on: [bool; 2]) -> f64 {
        let ramp = (total * 0.3).min(40.0);
        let k = (if on[0] { 1.0 } else { d / ramp }).min(if on[1] { 1.0 } else { (total - d) / ramp }).clamp(0.0, 1.0);
        RAIL_DECK_M * k * k * (3.0 - 2.0 * k)
    }

    /// T7's acceptance: a line along a bridge structure is at the deck's height along it and
    /// ramps up and down at its ends; before and after, and along a bridge that goes on past an
    /// end, as the structure says.
    #[test]
    fn a_line_along_a_bridge_structure_rises_to_its_deck_and_ramps_at_its_ends() {
        let metres = |p: &[f64; 3]| (p[0] - 18.0) / DEG_PER_M;
        let bridge = structure(false, 1000.0, 1300.0, [false, false]);
        let runs = on_structures(&[at(900.0), at(1400.0)], std::slice::from_ref(&bridge));
        assert_eq!(runs.len(), 1);
        let path = &runs[0].path;
        assert!(!runs[0].tunnel);
        // in pieces of at most 6 m near the bridge: 84 over the 500 m
        assert_eq!(path.len(), 85);
        let mut seen = [0; 3];
        for p in path {
            let m = metres(p);
            let want = if m < 1000.0 { 0.0 } else { deck_at(m - 1000.0, 300.0, [false, false]) };
            assert_close!(p[2], want, 6);
            match m {
                m if (1000.0..1040.0).contains(&m) => {
                    assert!(p[2] < RAIL_DECK_M);
                    seen[0] += 1;
                }
                m if (1040.0..=1260.0).contains(&m) => {
                    assert_eq!(p[2], RAIL_DECK_M, "{m} m");
                    seen[1] += 1;
                }
                m if (1260.0..1300.0).contains(&m) => {
                    assert!(p[2] > 0.0 && p[2] < RAIL_DECK_M);
                    seen[2] += 1;
                }
                m if !(1000.0..=1300.0).contains(&m) => assert_eq!(p[2], 0.0, "{m} m"),
                _ => {}
            }
        }
        // both ramps and the deck between them are sampled
        assert!(seen.iter().all(|&n| n >= 6), "{seen:?}");
        // halfway up its ramp, half the deck's height
        assert_close!(level_at(at(1020.0), Some(90.0), std::slice::from_ref(&bridge)).height, RAIL_DECK_M / 2.0, 9);
        assert_close!(level_at(at(1280.0), Some(270.0), std::slice::from_ref(&bridge)).height, RAIL_DECK_M / 2.0, 9);
        // a bridge that goes on past its west end: up from there
        let goes_on = structure(false, 1000.0, 1300.0, [true, false]);
        let runs = on_structures(&[at(1000.0), at(1100.0)], &[goes_on]);
        assert!(runs[0].path.iter().all(|p| p[2] == RAIL_DECK_M), "{:?}", runs[0].path);
    }

    /// A tag value of a test tile.
    #[derive(Clone, Copy)]
    enum V {
        S(&'static str),
        U(u64),
        B(bool),
    }

    /// A line feature of a test tile: tags and parts (points in tile units of 4096).
    type TestLine = (&'static [(&'static str, V)], &'static [&'static [(i64, i64)]]);

    fn varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push((v as u8) | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    fn field_bytes(out: &mut Vec<u8>, field: u64, b: &[u8]) {
        varint(out, (field << 3) | 2);
        varint(out, b.len() as u64);
        out.extend_from_slice(b);
    }

    fn field_uint(out: &mut Vec<u8>, field: u64, v: u64) {
        varint(out, field << 3);
        varint(out, v);
    }

    fn field_packed(out: &mut Vec<u8>, field: u64, values: &[u64]) {
        let mut body = Vec::new();
        for &v in values {
            varint(&mut body, v);
        }
        field_bytes(out, field, &body);
    }

    /// A vector tile of one layer `name` of lines, extent 4096.
    fn lines_tile(name: &str, lines: &[TestLine]) -> Vec<u8> {
        let zigzag = |v: i64| ((v << 1) ^ (v >> 63)) as u64;
        let (mut keys, mut values): (Vec<&str>, Vec<Vec<u8>>) = (Vec::new(), Vec::new());
        let mut layer = Vec::new();
        field_uint(&mut layer, 15, 2);
        field_bytes(&mut layer, 1, name.as_bytes());
        for (tags, parts) in lines {
            let mut t = Vec::new();
            for &(k, v) in *tags {
                let ki = keys.iter().position(|&x| x == k).unwrap_or_else(|| {
                    keys.push(k);
                    keys.len() - 1
                });
                let mut value = Vec::new();
                match v {
                    V::S(s) => field_bytes(&mut value, 1, s.as_bytes()),
                    V::U(n) => field_uint(&mut value, 5, n),
                    V::B(b) => field_uint(&mut value, 7, u64::from(b)),
                }
                values.push(value);
                t.extend([ki as u64, values.len() as u64 - 1]);
            }
            let (mut cx, mut cy) = (0, 0);
            let mut geometry = Vec::new();
            for part in *parts {
                for (k, &(x, y)) in part.iter().enumerate() {
                    if k == 0 {
                        geometry.push((1 << 3) | 1);
                    } else if k == 1 {
                        geometry.push(((part.len() as u64 - 1) << 3) | 2);
                    }
                    geometry.extend([zigzag(x - cx), zigzag(y - cy)]);
                    (cx, cy) = (x, y);
                }
            }
            let mut feature = Vec::new();
            field_packed(&mut feature, 2, &t);
            field_uint(&mut feature, 3, 2);
            field_packed(&mut feature, 4, &geometry);
            field_bytes(&mut layer, 2, &feature);
        }
        for k in &keys {
            field_bytes(&mut layer, 3, k.as_bytes());
        }
        for v in &values {
            field_bytes(&mut layer, 4, v);
        }
        field_uint(&mut layer, 5, 4096);
        let mut tile = Vec::new();
        field_bytes(&mut tile, 3, &layer);
        tile
    }

    fn never() -> bool {
        false
    }

    /// The z14 tile south of Stockholm Central (Söderström's bridges).
    const NETWORK_TILE: TileIndex = TileIndex { x: 9014, y: 4818, z: 14 };

    const RAIL: (&str, V) = ("class", V::S("rail"));
    const BRIDGE: (&str, V) = ("brunnel", V::S("bridge"));

    /// Across the tile: a main line on the ground at a quarter of the way down, a road bridge
    /// above it (no rail: not drawn), a 312 m rail bridge in the middle, a tram bridge the
    /// tile's east edge cuts, a rail tunnel three quarters down.
    fn network_lines() -> Vec<TestLine> {
        vec![
            (&[RAIL], &[&[(0, 1024), (4096, 1024)]]),
            (&[("class", V::S("minor")), BRIDGE], &[&[(0, 512), (4096, 512)]]),
            (&[RAIL, BRIDGE], &[&[(1024, 2048), (2048, 2048)]]),
            (&[("class", V::S("transit")), ("subclass", V::S("tram")), BRIDGE], &[&[(3584, 2560), (4160, 2560)]]),
            (&[RAIL, ("brunnel", V::S("tunnel"))], &[&[(0, 3072), (4096, 3072)]]),
        ]
    }

    /// The segments' starts and their last end, as `[u, v, metres]`.
    fn vertices(segments: &[PathSegment]) -> Vec<[f64; 3]> {
        let mut out: Vec<[f64; 3]> = segments.iter().map(|s| s.start.map(f64::from)).collect();
        out.extend(segments.last().map(|s| s.end.map(f64::from)));
        out
    }

    /// T7's draw lists, the network: on the flat map the tile's rail bridges are raised to their
    /// decks (up over 40 m from each end), the rest left on the ground as `crate::rail` styles it.
    #[test]
    fn the_flat_networks_bridges_rise_to_their_decks_and_the_rest_stays_on_the_ground() {
        use crate::rail::{RAIL_MAIN, RAIL_TRANSIT, tunnel_of};
        let bytes = lines_tile(NETWORK_TILE_LAYER, &network_lines());
        let parts = network_bridges(NETWORK_TILE, &bytes, &never).expect("decoded").expect("it has bridges");
        // on the ground: the main line and the faint, dashed tunnel; the bridges and the road not
        let ground: Vec<(Rgba4, f32, [f32; 2], f32)> = parts.ground.iter().map(|s| (s.color, s.width, s.dash_array, s.start[1])).collect();
        assert_eq!(ground, [(RAIL_MAIN, 1.9, [0.0, 0.0], 0.25), (tunnel_of(RAIL_MAIN), 1.9, [3.0, 2.5], 0.75)]);
        assert!(parts.ground.iter().all(|s| s.start[2] == 0.0 && s.end[2] == 0.0));
        // raised: the rail bridge, then the tram's
        let rail: Vec<&PathSegment> = parts.raised.iter().filter(|s| s.start[1] == 0.5).collect();
        let tram: Vec<&PathSegment> = parts.raised.iter().filter(|s| s.start[1] == 0.625).collect();
        assert_eq!(rail.len() + tram.len(), parts.raised.len());
        assert!(rail.iter().all(|s| (s.color, s.width, s.dash_array) == (RAIL_MAIN, 1.9, [0.0, 0.0])));
        assert!(tram.iter().all(|s| (s.color, s.width) == (RAIL_TRANSIT, 1.2)));
        let tile_metres = tile_metres(NETWORK_TILE);
        let total = 0.25 * tile_metres;
        // heights as today's app draws them: each metre of deck 2.5e-5 m up
        let lift = rail_map_lift(NETWORK_TILE);
        let top = f64::from((RAIL_DECK_M * lift) as f32);
        let rail = vertices(&rail.into_iter().copied().collect::<Vec<_>>());
        // in pieces of at most 6 m, from one end to the other
        assert_eq!(rail.len(), (total / 6.0).ceil() as usize + 1);
        assert_eq!((rail[0][0], rail[rail.len() - 1][0]), (0.25, 0.5));
        for w in rail.windows(2) {
            assert!((w[1][0] - w[0][0]) * tile_metres <= 6.0 + 1e-6);
        }
        // up from the ground at each end to the 6 m deck over 40 m, smoothly (to 4 digits: the
        // vertices read back from the segments' f32 tile units)
        for p in &rail {
            assert_close!(p[2] / lift, deck_at((p[0] - 0.25) * tile_metres, total, [false, false]), 4);
        }
        assert_eq!((rail[0][2], rail[rail.len() - 1][2]), (0.0, 0.0));
        assert!(rail.iter().filter(|p| p[2] == top).count() > 30);
        let ramp: Vec<f64> = rail.iter().take_while(|p| p[2] < top).map(|p| p[2]).collect();
        assert!(ramp.windows(2).all(|w| w[1] > w[0]) && ramp.len() == 7, "{ramp:?}");
        // the tram's bridge: up from its west end, still up where the tile's edge cuts it (it goes
        // on into the next tile), which is where it stops, running on into the vertex beyond
        let tram_vertices = vertices(&tram.iter().map(|s| **s).collect::<Vec<_>>());
        assert_eq!(tram_vertices[0][2], 0.0);
        let last = tram_vertices[tram_vertices.len() - 1];
        assert_eq!((last[0], last[2]), (1.0, top));
        let (first, end) = (tram[0], tram[tram.len() - 1]);
        assert_eq!(first.types, START_CAP);
        assert_eq!(end.types, 0, "no cap where the tile cuts it");
        assert!(end.right[0] > 1.0 && f64::from(end.right[2]) == top, "{:?}", end.right);
    }

    type Rgba4 = [u8; 4];

    #[test]
    fn no_bridge_nothing_raised() {
        let lines = network_lines();
        // the main line, the road bridge (not drawn) and the tunnel
        let bytes = lines_tile(NETWORK_TILE_LAYER, &[lines[0], lines[1], lines[4]]);
        assert_eq!(network_bridges(NETWORK_TILE, &bytes, &never), Ok(None));
        assert_eq!(network_bridges(NETWORK_TILE, &[], &never), Ok(None));
        let all = lines_tile(NETWORK_TILE_LAYER, &lines);
        assert_eq!(network_bridges(NETWORK_TILE, &all, &|| true), Err(Cancelled));
        // another layer's lines are not the network's
        assert_eq!(network_bridges(NETWORK_TILE, &lines_tile("water", &lines), &never), Ok(None));
    }

    /// The z12 tile over Stockholm Central.
    const SPEED_TILE: TileIndex = TileIndex { x: 2253, y: 1204, z: 12 };

    /// T7's draw lists, the speed map: OpenRailwayMap's bridges (`bridge` true) raised, by their
    /// limit's colour; the rest on the ground as the speed map draws it.
    #[test]
    fn the_flat_speed_maps_bridges_rise_to_their_decks() {
        use crate::rail::{RAIL_TRANSIT, SPEED_BANDS, SPEED_UNKNOWN, tunnel_of};
        let tracks: Vec<TestLine> = vec![
            (&[("feature", V::S("rail")), ("maxspeed", V::U(160))], &[&[(0, 1024), (4096, 1024)]]),
            (&[("feature", V::S("rail")), ("maxspeed", V::U(160)), ("bridge", V::B(true))], &[&[(1024, 2048), (1536, 2048)]]),
            // OpenRailwayMap's bridges are booleans: "yes" is on the ground
            (&[("feature", V::S("rail")), ("bridge", V::S("yes"))], &[&[(0, 2560), (4096, 2560)]]),
            (&[("feature", V::S("tram")), ("bridge", V::B(true))], &[&[(2048, 3072), (2560, 3072)], &[(2048, 3200), (2560, 3200)]]),
            (&[("feature", V::S("rail")), ("maxspeed", V::U(40)), ("tunnel", V::B(true))], &[&[(0, 3584), (4096, 3584)]]),
            // not drawn: no colour, so nothing raised
            (&[("feature", V::S("abandoned")), ("bridge", V::B(true))], &[&[(0, 4000), (4096, 4000)]]),
        ];
        let bytes = lines_tile(SPEED_TILE_LAYER, &tracks);
        let parts = speed_bridges(SPEED_TILE, &bytes, &never).expect("decoded").expect("it has bridges");
        let ground: Vec<(Rgba4, f32, [f32; 2])> = parts.ground.iter().map(|s| (s.color, s.width, s.dash_array)).collect();
        let tunnel = tunnel_of(SPEED_BANDS[0].color);
        assert_eq!(ground, [(SPEED_BANDS[3].color, 2.2, [0.0, 0.0]), (SPEED_UNKNOWN, 2.2, [0.0, 0.0]), (tunnel, 2.2, [3.0, 2.5])]);
        let line: Vec<PathSegment> = parts.raised.iter().filter(|s| s.start[1] == 0.5).copied().collect();
        let tram: Vec<PathSegment> = parts.raised.iter().filter(|s| s.start[1] > 0.7).copied().collect();
        assert_eq!(line.len() + tram.len(), parts.raised.len());
        assert!(line.iter().all(|s| (s.color, s.width) == (SPEED_BANDS[3].color, 2.2)));
        // both parts of the tram's bridge, each up and down
        assert!(tram.iter().all(|s| (s.color, s.width) == (RAIL_TRANSIT, 1.1)));
        assert_eq!(tram.iter().filter(|s| s.types == START_CAP).count(), 2);
        let tile_metres = tile_metres(SPEED_TILE);
        let total = 0.125 * tile_metres;
        let lift = rail_map_lift(SPEED_TILE);
        let line = vertices(&line);
        assert_eq!(line.len(), (total / 6.0).ceil() as usize + 1);
        for p in &line {
            assert_close!(p[2] / lift, deck_at((p[0] - 0.25) * tile_metres, total, [false, false]), 4);
        }
        assert_eq!(line.iter().map(|p| p[2]).fold(f64::NEG_INFINITY, js_max), f64::from((RAIL_DECK_M * lift) as f32));
        // a tile without bridges: none
        let plain = lines_tile(SPEED_TILE_LAYER, &[tracks[0], tracks[2], tracks[4]]);
        assert_eq!(speed_bridges(SPEED_TILE, &plain, &never), Ok(None));
        assert_eq!(speed_bridges(SPEED_TILE, &bytes, &|| true), Err(Cancelled));
    }

    /// Today's app writes the bridges' heights in deck's common units where deck reads metres: a
    /// 6 m deck stands 0.15 mm up at Stockholm, as `bridgeParts`' `commonPerMetre` there.
    #[test]
    fn the_rail_maps_bridges_stand_as_high_as_todays() {
        let lift = rail_map_lift(NETWORK_TILE);
        assert_close!(RAIL_DECK_M * lift * 1000.0, 0.15, 2);
        // `512 / (EARTH_CIRCUMFERENCE · cos lat)` at the tile's middle
        assert_close!(lift, 512.0 / (tile_metres(NETWORK_TILE) * 2f64.powi(14)), 15);
        assert_close!(rail_map_lift(TileIndex { x: 0, y: 0, z: 1 }) * EARTH_CIRCUMFERENCE_M / 512.0, 1.0 / (PI * (1.0 - 1.0 / 2.0)).sinh().atan().cos(), 12);
        // the same at every zoom for the same place
        assert_close!(rail_map_lift(SPEED_TILE) / rail_map_lift(TileIndex { x: 2253 * 4 + 1, y: 1204 * 4 + 1, z: 14 }), 1.0, 3);
    }

    #[test]
    fn a_path_is_cut_at_the_tiles_square() {
        let inside = |path: &[[f64; 3]]| inside_square(path);
        // across: in at the west edge, out at the east, heights along
        let across = [[-0.5, 0.5, 0.0], [0.5, 0.5, 2.0], [1.5, 0.5, 4.0]];
        assert_eq!(
            inside(&across),
            [Inside { points: vec![[0.0, 0.5, 1.0], [0.5, 0.5, 2.0], [1.0, 0.5, 3.0]], before: Some(across[0]), after: Some(across[2]) }]
        );
        // wholly inside: as it is
        let within = [[0.1, 0.1, 0.0], [0.9, 0.9, 6.0]];
        assert_eq!(inside(&within), [Inside { points: within.to_vec(), before: None, after: None }]);
        // wholly outside, or touching a corner: nothing
        assert!(inside(&[[1.5, 0.5, 0.0], [2.5, 0.5, 0.0]]).is_empty());
        assert!(inside(&[[1.5, 0.5, 0.0], [0.5, -0.5, 0.0]]).is_empty());
        // out and in again: two stretches
        let back = [[0.5, 0.5, 0.0], [1.5, 0.5, 0.0], [1.5, 0.75, 0.0], [0.5, 0.75, 0.0]];
        assert_eq!(
            inside(&back),
            [
                Inside { points: vec![[0.5, 0.5, 0.0], [1.0, 0.5, 0.0]], before: None, after: Some(back[1]) },
                Inside { points: vec![[1.0, 0.75, 0.0], [0.5, 0.75, 0.0]], before: Some(back[2]), after: None },
            ]
        );
        // in and out through vertices on the edge: the vertices beyond carry on
        let edges = [[-1.0, 0.5, 0.0], [0.0, 0.5, 0.0], [0.5, 0.8, 0.0], [1.0, 0.5, 0.0], [2.0, 0.5, 0.0]];
        assert_eq!(inside(&edges), [Inside { points: edges[1..4].to_vec(), before: Some(edges[0]), after: Some(edges[4]) }]);
        // the segments: a cut end runs on into the vertex beyond, without a cap
        let local = LocalMercator::tile(NETWORK_TILE);
        let mut out = Vec::new();
        push_inside(&mut out, local, &across, &Paint::solid([1, 2, 3, 4], 2.0));
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].left, out[0].start, out[0].types), ([-0.5, 0.5, 0.0], [0.0, 0.5, 1.0], 0));
        assert_eq!((out[1].end, out[1].right, out[1].types), ([1.0, 0.5, 3.0], [1.5, 0.5, 4.0], 0));
        // a path wholly inside keeps its caps
        let mut whole = Vec::new();
        push_inside(&mut whole, local, &within, &Paint::solid([1, 2, 3, 4], 2.0));
        assert_eq!(whole.iter().map(|s| s.types).collect::<Vec<_>>(), [START_CAP | END_CAP]);
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
