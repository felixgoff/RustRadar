//! Pure part of `overture.ts` (S1.23; doc 04 §7.7): how tall buildings are where the map's own
//! data doesn't say. Overture Maps' buildings (© OpenStreetMap, © Overture Maps Foundation,
//! ODbL) add machine-learned heights (Microsoft's) to OpenStreetMap's. They are read from
//! Overture's own PMTiles file of them, a zoom-[`ZOOM`] tile at a time, with range requests; the
//! release is the latest its catalogue names.
//!
//! The reading is the caller's: the release ([`release_of`] the JSON at [`CATALOG`]), the
//! archive at [`archive_url`], each tile's [`LAYERS`] decoded to longitude and latitude and kept
//! as [`tall_buildings`] ([`TILES_CACHED`] tiles kept, none where a tile can't be had). For a
//! Streets GL tile's buildings, `withOvertureHeights` is [`unheighted`], the tiles of their
//! middles ([`tiles_of`]) loaded, [`heights_at`] them, then [`give_heights`];
//! [`with_overture_heights`] runs the three with the loading passed in.

use std::collections::HashMap;
use std::f64::consts::PI;

use indexmap::IndexMap;
use serde_json::Value;

use crate::landcover::{js_max, js_min};
use crate::streets_gl::{Properties, js_string_of};

pub const OVERTURE_ATTRIBUTION: &str = "Building heights: © Overture Maps Foundation, © OpenStreetMap (ODbL)";
/// The catalogue naming the latest release.
pub const CATALOG: &str = "https://stac.overturemaps.org/catalog.json";
/// The release asked for when the catalogue can't be read.
pub const KNOWN_RELEASE: &str = "2026-09-23.1";
/// The zoom of the tiles read.
pub const ZOOM: u32 = 14;
/// The layers read from each tile.
pub const LAYERS: [&str; 2] = ["building", "building_part"];
/// Tiles kept by the caller.
pub const TILES_CACHED: usize = 48;

/// The release named by the catalogue (`c?.latest ?? KNOWN_RELEASE`; none where it can't be
/// read).
pub fn release_of(catalog: Option<&Value>) -> String {
    match catalog.and_then(|c| c.get("latest")) {
        None | Some(Value::Null) => KNOWN_RELEASE.to_owned(),
        latest => js_string_of(latest).into_owned(),
    }
}

/// A release's buildings.
pub fn archive_url(release: &str) -> String {
    format!("https://tiles.overturemaps.org/{release}/buildings.pmtiles")
}

/// An Overture building: its outline (longitude, latitude), its box (west, south, east, north),
/// and its height and floors where it has them (0 where not).
#[derive(Debug, Clone, PartialEq)]
pub struct Building {
    pub ring: Vec<[f64; 2]>,
    pub bbox: [f64; 4],
    pub height: f64,
    pub floors: f64,
}

/// Tall enough to say: what a building is given, metres or floors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Height {
    pub height: f64,
    pub floors: f64,
}

/// Whether a point is inside a ring (even-odd).
pub fn inside(p: [f64; 2], ring: &[[f64; 2]]) -> bool {
    let mut yes = false;
    let Some(mut j) = ring.len().checked_sub(1) else { return false };
    for i in 0..ring.len() {
        let ([xi, yi], [xj, yj]) = (ring[i], ring[j]);
        if (yi > p[1]) != (yj > p[1]) && p[0] < ((xj - xi) * (p[1] - yi)) / (yj - yi) + xi {
            yes = !yes;
        }
        j = i;
    }
    yes
}

/// A decoded feature's geometry (GeoJSON's, longitude and latitude), as far as it is read here.
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    /// Rings, the outer one first.
    Polygon(Vec<Vec<[f64; 2]>>),
    MultiPolygon(Vec<Vec<Vec<[f64; 2]>>>),
    /// Points and lines: no outline.
    Other,
}

/// A decoded feature: its geometry and its tags.
#[derive(Debug, Clone, PartialEq)]
pub struct Feature {
    pub geometry: Geometry,
    pub properties: Properties,
}

/// `Number(v) || 0`.
fn number_or_zero(v: Option<&Value>) -> f64 {
    let n = js_number(v);
    if n != 0.0 && !n.is_nan() { n } else { 0.0 }
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

/// A tile's buildings with a height or floors, from its decoded features (longitude and
/// latitude): each polygon's outer ring, with its box.
pub fn tall_buildings(features: impl IntoIterator<Item = Feature>) -> Vec<Building> {
    let mut out = Vec::new();
    for f in features {
        let height = number_or_zero(f.properties.get("height"));
        let floors = number_or_zero(f.properties.get("num_floors"));
        if !(height > 0.0 || floors > 0.0) {
            continue;
        }
        let polygons = match f.geometry {
            Geometry::Polygon(polygon) => vec![polygon],
            Geometry::MultiPolygon(polygons) => polygons,
            Geometry::Other => Vec::new(),
        };
        for polygon in polygons {
            let Some(ring) = polygon.into_iter().next() else { continue };
            if ring.len() < 3 {
                continue;
            }
            // `Math.min(...xs)` and the rest
            let (mut west, mut south, mut east, mut north) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
            for &[x, y] in &ring {
                (west, east) = (js_min(west, x), js_max(east, x));
                (south, north) = (js_min(south, y), js_max(north, y));
            }
            out.push(Building { ring, bbox: [west, south, east, north], height, floors });
        }
    }
    out
}

/// The zoom-[`ZOOM`] tile a point (longitude, latitude) is in. A NaN place is tile 0, 0 (the
/// TypeScript's tile NaN, NaN has no buildings; no building's middle is NaN).
pub fn tile_of(lon: f64, lat: f64) -> (i64, i64) {
    let n = 2f64.powi(ZOOM as i32);
    let r = (lat * PI) / 180.0;
    (
        (((lon + 180.0) / 360.0) * n).floor() as i64,
        (((1.0 - (r.tan() + 1.0 / r.cos()).ln() / PI) / 2.0) * n).floor() as i64,
    )
}

/// The tiles points are in, each once, in the order first met: the ones to load for
/// [`heights_at`].
pub fn tiles_of(points: &[[f64; 2]]) -> Vec<(i64, i64)> {
    let mut tiles: Vec<(i64, i64)> = Vec::new();
    for p in points {
        let tile = tile_of(p[0], p[1]);
        if !tiles.contains(&tile) {
            tiles.push(tile);
        }
    }
    tiles
}

/// The tallest Overture building (or part) at each point, as its height and floors (by height,
/// else, where neither has one, by floors); none where none has them. `loaded` holds the
/// [`tall_buildings`] of the points' tiles ([`tiles_of`]); a tile not there has none.
pub fn heights_at(points: &[[f64; 2]], loaded: &HashMap<(i64, i64), Vec<Building>>) -> Vec<Option<Height>> {
    points
        .iter()
        .map(|&p| {
            let mut best: Option<Height> = None;
            for b in loaded.get(&tile_of(p[0], p[1])).map_or(&[][..], Vec::as_slice) {
                if p[0] < b.bbox[0] || p[0] > b.bbox[2] || p[1] < b.bbox[1] || p[1] > b.bbox[3] || !inside(p, &b.ring) {
                    continue;
                }
                // (`!b.height`: heights are `Number(...) || 0`, never NaN)
                if best.is_none_or(|best| {
                    b.height > best.height || (b.height == 0.0 && best.height == 0.0 && b.floors > best.floors)
                }) {
                    best = Some(Height { height: b.height, floors: b.floors });
                }
            }
            best
        })
        .collect()
}

/// A binary tile's polygons (loaders.gl's), as far as they are read here: in the tile's own
/// coordinates (0 to 1), y south.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Polygons {
    /// The vertices' coordinates, [`size`](Self::size) numbers a vertex.
    pub positions: Vec<f64>,
    pub size: usize,
    /// Each vertex's feature; a feature's vertices follow each other.
    pub feature_ids: Vec<u32>,
    /// Numeric properties, a value a vertex (loaders.gl's are `Float32Array`s).
    pub numeric_props: IndexMap<String, Vec<f32>>,
}

/// A building's vertices, `start..end`, and their middle (longitude, latitude).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub at: [f64; 2],
}

/// The buildings of a Streets GL tile (`x, y, z`) with neither a height nor levels mapped: their
/// vertices and the middle of them.
pub fn unheighted(polygons: &Polygons, x: u32, y: u32, z: u32) -> Vec<Span> {
    let (ids, size) = (&polygons.feature_ids, polygons.size);
    let count = ids.len();
    let position = |i: usize| polygons.positions.get(i).copied().unwrap_or(f64::NAN);
    let own = |name: &str| polygons.numeric_props.get(name);
    let (heights, levels) = (own("height"), own("levels"));
    // `heights?.[start] || levels?.[start]`
    let mapped = |column: Option<&Vec<f32>>, at: usize| column.and_then(|c| c.get(at)).is_some_and(|&v| v != 0.0 && !v.is_nan());
    let n = 2f64.powi(z as i32);
    let mut spans = Vec::new();
    let mut start = 0;
    while start < count {
        let mut end = start;
        let (mut sx, mut sy) = (0.0, 0.0);
        while end < count && ids[end] == ids[start] {
            sx += position(end * size);
            sy += position(end * size + 1);
            end += 1;
        }
        if !(mapped(heights, start) || mapped(levels, start)) {
            let k = (end - start) as f64;
            let (px, py) = (sx / k, sy / k);
            let lon = ((f64::from(x) + px) / n) * 360.0 - 180.0;
            let lat = ((PI * (1.0 - (2.0 * (f64::from(y) + py)) / n)).sinh().atan() * 180.0) / PI;
            spans.push(Span { start, end, at: [lon, lat] });
        }
        start = end;
    }
    spans
}

/// Overture's heights (`found`, one for each span, none where none was found) given to the
/// spans' buildings: `height` where it has one, else `levels` as its floors, set on each of their
/// vertices. Nothing changes where none was found at all.
pub fn give_heights(polygons: &mut Polygons, spans: &[Span], found: &[Option<Height>]) {
    if !found.iter().any(Option::is_some) {
        return;
    }
    let count = polygons.feature_ids.len();
    // the columns there are, else new ones of zeros
    for name in ["height", "levels"] {
        polygons.numeric_props.entry(name.to_owned()).or_insert_with(|| vec![0.0; count]);
    }
    let fill = |column: &mut Vec<f32>, value: f64, s: &Span| {
        // `Float32Array.fill(value, start, end)`: the range cut to the column
        let end = s.end.min(column.len());
        if s.start < end {
            column[s.start..end].fill(value as f32);
        }
    };
    for (s, f) in spans.iter().zip(found) {
        let Some(f) = f else { continue };
        if f.height > 0.0 {
            fill(&mut polygons.numeric_props["height"], f.height, s);
        } else {
            fill(&mut polygons.numeric_props["levels"], f.floors, s);
        }
    }
}

/// A Streets GL tile's buildings (binary, in the tile's own coordinates) with Overture's heights
/// given to those it has none for (neither a height nor levels mapped): `height`, else `levels`,
/// set on each of their vertices. `heights_at` answers for the buildings' middles, as
/// [`heights_at`] does once their tiles are loaded.
pub fn with_overture_heights(
    polygons: &mut Polygons,
    x: u32,
    y: u32,
    z: u32,
    heights_at: impl FnOnce(&[[f64; 2]]) -> Vec<Option<Height>>,
) {
    let spans = unheighted(polygons, x, y, z);
    if spans.is_empty() {
        return;
    }
    let points: Vec<[f64; 2]> = spans.iter().map(|s| s.at).collect();
    let found = heights_at(&points);
    give_heights(polygons, &spans, &found);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn square(x: f64, y: f64, s: f64) -> Vec<[f64; 2]> {
        vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s], [x, y]]
    }

    fn feature(geometry: Geometry, properties: Value) -> Feature {
        let Value::Object(properties) = properties else { panic!("not an object") };
        Feature { geometry, properties }
    }

    #[test]
    fn a_point_in_a_buildings_outline() {
        assert!(inside([0.5, 0.5], &square(0.0, 0.0, 1.0)));
        assert!(!inside([1.5, 0.5], &square(0.0, 0.0, 1.0)));
    }

    #[test]
    fn only_the_buildings_with_a_height_or_floors_are_kept_each_part_of_a_multipolygon() {
        let kept = tall_buildings([
            feature(Geometry::Polygon(vec![square(0.0, 0.0, 1.0)]), json!({"height": 12})),
            feature(
                Geometry::MultiPolygon(vec![vec![square(2.0, 0.0, 1.0)], vec![square(4.0, 0.0, 1.0)]]),
                json!({"num_floors": 3}),
            ),
            feature(Geometry::Polygon(vec![square(6.0, 0.0, 1.0)]), json!({})),
        ]);
        assert_eq!(kept.iter().map(|b| [b.height, b.floors]).collect::<Vec<_>>(), [[12.0, 0.0], [0.0, 3.0], [0.0, 3.0]]);
        assert_eq!(kept[1].bbox, [2.0, 0.0, 3.0, 1.0]);
    }

    // New tests (no TypeScript twin).

    #[test]
    fn heights_and_floors_as_number_reads_them() {
        let read = |v: Value| number_or_zero(Some(&v));
        assert_eq!(read(json!(" 12.5 ")), 12.5);
        assert_eq!(read(json!("0x10")), 16.0);
        assert_eq!(read(json!("12 m")), 0.0);
        assert_eq!(read(json!("inf")), 0.0);
        assert_eq!(read(json!("Infinity")), f64::INFINITY);
        assert_eq!(read(json!(true)), 1.0);
        assert_eq!(read(json!([7])), 7.0);
        assert_eq!(read(json!({"a": 1})), 0.0);
        assert_eq!(number_or_zero(None), 0.0);
        assert_eq!(string_to_number(""), 0.0);
        assert_eq!(string_to_number(".5"), 0.5);
        assert_eq!(string_to_number("1e3"), 1000.0);
        assert!(string_to_number(".").is_nan() && string_to_number("0x").is_nan() && string_to_number("1e").is_nan());
    }

    #[test]
    fn holes_rings_too_small_and_lines_are_not_buildings() {
        let ring = vec![[0.0, 0.0], [1.0, 0.0]];
        let kept = tall_buildings([
            feature(Geometry::Polygon(vec![ring]), json!({"height": 5})),
            feature(Geometry::Polygon(vec![]), json!({"height": 5})),
            feature(Geometry::Other, json!({"height": 5})),
            feature(Geometry::Polygon(vec![square(0.0, 0.0, 2.0), square(0.5, 0.5, 1.0)]), json!({"height": -3, "num_floors": "2"})),
        ]);
        assert_eq!(kept.len(), 1);
        assert_eq!((kept[0].height, kept[0].floors, kept[0].bbox), (-3.0, 2.0, [0.0, 0.0, 2.0, 2.0]));
        // an outline's holes are not looked at, nor its edges' far side
        assert!(!inside([0.0, 0.0], &[]));
        assert!(inside([1.0, 1.0], &kept[0].ring));
        assert!(!inside([2.0, 1.0], &kept[0].ring));
    }

    #[test]
    fn the_latest_release_else_the_known_one() {
        assert_eq!(release_of(Some(&json!({"latest": "2027-01-01.0"}))), "2027-01-01.0");
        assert_eq!(release_of(Some(&json!({"latest": null}))), KNOWN_RELEASE);
        assert_eq!(release_of(Some(&json!(["x"]))), KNOWN_RELEASE);
        assert_eq!(release_of(None), KNOWN_RELEASE);
        assert_eq!(archive_url(KNOWN_RELEASE), "https://tiles.overturemaps.org/2026-09-23.1/buildings.pmtiles");
    }

    /// Lund's cathedral as an Overture building 40 m tall, a 3-floor one beside it, and two
    /// buildings in one place, one with floors only.
    fn lund() -> HashMap<(i64, i64), Vec<Building>> {
        let buildings = tall_buildings([
            feature(Geometry::Polygon(vec![square(13.193, 55.704, 0.0005)]), json!({"height": 40})),
            feature(Geometry::Polygon(vec![square(13.195, 55.704, 0.0005)]), json!({"num_floors": 3})),
            feature(Geometry::Polygon(vec![square(13.195, 55.704, 0.0002)]), json!({"num_floors": 5})),
            feature(Geometry::Polygon(vec![square(13.193, 55.704, 0.0002)]), json!({"height": 12, "num_floors": 9})),
        ]);
        assert_eq!(tile_of(13.193, 55.704), tile_of(13.1955, 55.7045));
        HashMap::from([(tile_of(13.193, 55.704), buildings)])
    }

    #[test]
    fn the_tallest_building_at_each_point() {
        let loaded = lund();
        let points = [[13.1931, 55.7041], [13.1954, 55.7044], [13.1951, 55.7041], [13.194, 55.703], [-80.0, 30.0]];
        // (the TypeScript's tiles for these)
        assert_eq!(tiles_of(&points), [(8792, 5125), (4551, 6759)]);
        assert_eq!(
            heights_at(&points, &loaded),
            [
                // the taller by height
                Some(Height { height: 40.0, floors: 0.0 }),
                Some(Height { height: 0.0, floors: 3.0 }),
                // by floors where neither has a height
                Some(Height { height: 0.0, floors: 5.0 }),
                None,
                None,
            ]
        );
        // Lund's zoom-14 tile
        assert_eq!(tile_of(13.19, 55.705), (8792, 5125));
    }

    #[test]
    fn a_tiles_buildings_without_a_height_take_overtures() {
        // a zoom-16 Streets GL tile round the cathedral, its four buildings as squares in the
        // tile's own coordinates: one on the cathedral, one with a height of its own, one with
        // levels, one on the 3-floor building
        let (z, n) = (16u32, 65536.0);
        let tx = ((13.1932 + 180.0) / 360.0) * n;
        let r = (55.7042f64 * PI) / 180.0;
        let ty = ((1.0 - (r.tan() + 1.0 / r.cos()).ln() / PI) / 2.0) * n;
        let (x, y) = (tx.floor() as u32, ty.floor() as u32);
        let (fx, fy) = (tx - tx.floor(), ty - ty.floor());
        // 0.0022° east, about two fifths of a zoom-16 tile
        let east = 0.0022 / 360.0 * n;
        let corners = |cx: f64| [cx, fy, cx + 0.01, fy, cx + 0.01, fy + 0.01, cx, fy + 0.01];
        let mut positions = Vec::new();
        for cx in [fx - 0.005, fx + 0.2, fx + 0.3, fx + east - 0.005] {
            positions.extend(corners(cx));
        }
        let mut polygons = Polygons {
            positions,
            size: 2,
            feature_ids: [0, 1, 2, 3].iter().flat_map(|&id| [id; 4]).collect(),
            numeric_props: IndexMap::from([("levels".to_owned(), [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 2.0, 2.0, 2.0, 0.0, 0.0, 0.0, 0.0].to_vec())]),
        };
        polygons.numeric_props.insert("height".to_owned(), [[0.0; 4], [7.0; 4], [0.0; 4], [0.0; 4]].concat());
        let spans = unheighted(&polygons, x, y, z);
        assert_eq!(spans.iter().map(|s| (s.start, s.end)).collect::<Vec<_>>(), [(0, 4), (12, 16)]);
        let loaded = lund();
        with_overture_heights(&mut polygons, x, y, z, |points| heights_at(points, &loaded));
        assert_eq!(polygons.numeric_props["height"], [[40.0; 4], [7.0; 4], [0.0; 4], [0.0; 4]].concat());
        assert_eq!(polygons.numeric_props["levels"], [[0.0; 4], [0.0; 4], [2.0; 4], [3.0; 4]].concat());
        // columns made where there were none, and nothing done where nothing was found
        let mut bare = Polygons { positions: corners(fx - 0.005).to_vec(), size: 2, feature_ids: vec![9; 4], ..Polygons::default() };
        with_overture_heights(&mut bare, x, y, z, |points| vec![None; points.len()]);
        assert!(bare.numeric_props.is_empty());
        with_overture_heights(&mut bare, x, y, z, |points| heights_at(points, &loaded));
        assert_eq!(bare.numeric_props.keys().collect::<Vec<_>>(), ["height", "levels"]);
        assert_eq!((&bare.numeric_props["height"], &bare.numeric_props["levels"]), (&vec![40.0; 4], &vec![0.0; 4]));
    }
}
