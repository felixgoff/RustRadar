//! Port of `streets-gl.ts` (S1.23; doc 04 §6.6): Streets GL's own vector tiles
//! (tiles.streets.gl, its author's server; MIT code, © OpenStreetMap contributors data), which
//! the realistic map reads close in: each building's height, colours and materials, each field,
//! wood and tree, each road's lanes and surface. Only zoom 16 is served.
//!
//! Their schema, as Streets GL's tile processing reads it: layers `buildings`, `common`
//! (gardens, grass, fields, pitches, car parks...), `natural` (woods, scrub, tree rows), `water`
//! (water, waterways), `highways` (roads, paths, railways), `barriers` and `point` (single trees,
//! benches, lamps), each feature's kind in `type`.
//!
//! A feature's tags are [`Properties`], JSON values as the TypeScript's `Record<string, unknown>`
//! has them; the JavaScript conversions it leans on (`String(v)`, `typeof v === "number"`) are
//! written out here so the tables match exactly what they matched there.

use std::borrow::Cow;

use rustradar_core::num::{js_string, to_int32};
use serde_json::Value;

use crate::landcover::{js_max, js_min};

/// The tiles; only [`SGL_ZOOM`] is served.
pub const SGL_TILES: &str = "https://tiles.streets.gl/vector/{z}/{x}/{y}";
pub const SGL_ZOOM: u32 = 16;
/// The map draws from these tiles from here (deck.gl rounds the view's zoom to the tiles', so
/// from half a zoom under theirs). A tile layer's `minZoom` is this, not theirs: deck.gl hides a
/// layer under its `minZoom` whatever tiles it loads.
pub const SGL_FROM_ZOOM: f64 = SGL_ZOOM as f64 - 0.5;
/// Their credit, with OpenStreetMap's.
pub const SGL_ATTRIBUTION: &str = "Map data close in: Streets GL (tiles.streets.gl), © OpenStreetMap contributors";

/// A feature's tags (`Record<string, unknown>`).
pub type Properties = serde_json::Map<String, Value>;

/// [`SGL_TILES`] for a tile.
pub fn tile_url(x: u32, y: u32, z: u32) -> String {
    SGL_TILES.replace("{z}", &z.to_string()).replace("{x}", &x.to_string()).replace("{y}", &y.to_string())
}

/// Whether a tile's URL is one Streets GL serves (`sglFetch`: deck.gl wants coarser tiles towards
/// a tilted view's horizon, which it doesn't serve; those are not asked for).
pub fn served(url: &str) -> bool {
    url.contains(&format!("/vector/{SGL_ZOOM}/"))
}

/// `String(v)` (`None` is `undefined`): what a tag reads as where the TypeScript looks it up in
/// a table by name.
pub(crate) fn js_string_of(v: Option<&Value>) -> Cow<'_, str> {
    match v {
        None => Cow::Borrowed("undefined"),
        Some(Value::Null) => Cow::Borrowed("null"),
        Some(Value::Bool(b)) => Cow::Borrowed(if *b { "true" } else { "false" }),
        Some(Value::Number(n)) => Cow::Owned(js_string(n.as_f64().unwrap_or(f64::NAN))),
        Some(Value::String(s)) => Cow::Borrowed(s),
        // `Array.prototype.toString`: its elements joined by commas, null ones empty
        Some(Value::Array(items)) => Cow::Owned(
            items
                .iter()
                .map(|item| if item.is_null() { Cow::Borrowed("") } else { js_string_of(Some(item)) })
                .collect::<Vec<_>>()
                .join(","),
        ),
        Some(Value::Object(_)) => Cow::Borrowed("[object Object]"),
    }
}

/// `num`: a finite number, else none.
fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|n| n.is_finite())
}

/// `some`: [`num`], but 0 is none too (a tile decoded to binary has a number the feature doesn't
/// map as 0, so where 0 can't be meant it is none).
fn some(v: Option<&Value>) -> Option<f64> {
    num(v).filter(|&n| n != 0.0)
}

/// The CSS colours Streets GL turns OpenStreetMap's colour names into, back to their names
/// (which the map tones down).
fn colour_name(rgb: u32) -> Option<&'static str> {
    Some(match rgb {
        0xffffff => "white",
        0x808080 => "grey",
        0xd3d3d3 => "lightgrey",
        0xa9a9a9 => "darkgrey",
        0xc0c0c0 => "silver",
        0xff0000 => "red",
        0x800000 => "maroon",
        0xa52a2a => "brown",
        0xa0522d => "brown",
        0xd2b48c => "tan",
        0xf5f5dc => "beige",
        0xffff00 => "yellow",
        0xffd700 => "gold",
        0xffa500 => "orange",
        0xffc0cb => "pink",
        0x008000 => "green",
        0x0000ff => "blue",
        _ => return None,
    })
}

/// A Streets GL colour (`0xRRGGBB`): a colour's name where it is a named one, else `#rrggbb`
/// (black, 0, as none).
pub fn hex_colour(v: Option<&Value>) -> Option<String> {
    let n = some(v)?;
    // `n & 0xffffff`: through a 32-bit integer
    let rgb = (to_int32(n) & 0xffffff) as u32;
    Some(colour_name(rgb).map_or_else(|| format!("#{rgb:06x}"), str::to_owned))
}

/// A façade's material, as Streets GL names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Facade {
    Brick,
    CementBlock,
    Wood,
    Plaster,
    Glass,
}

impl Facade {
    /// The TypeScript's name.
    pub fn as_str(self) -> &'static str {
        match self {
            Facade::Brick => "brick",
            Facade::CementBlock => "cementBlock",
            Facade::Wood => "wood",
            Facade::Plaster => "plaster",
            Facade::Glass => "glass",
        }
    }
}

/// A roof's material, as Streets GL names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoofMaterial {
    Tiles,
    Metal,
    Concrete,
    Eternit,
    Thatch,
    Grass,
    Glass,
    Tar,
}

impl RoofMaterial {
    /// The TypeScript's name.
    pub fn as_str(self) -> &'static str {
        match self {
            RoofMaterial::Tiles => "tiles",
            RoofMaterial::Metal => "metal",
            RoofMaterial::Concrete => "concrete",
            RoofMaterial::Eternit => "eternit",
            RoofMaterial::Thatch => "thatch",
            RoofMaterial::Grass => "grass",
            RoofMaterial::Glass => "glass",
            RoofMaterial::Tar => "tar",
        }
    }
}

/// A building's shape and looks, as Streets GL works them out.
#[derive(Debug, Clone, PartialEq)]
pub struct SglBuilding {
    /// To the top of the roof, metres.
    pub height: f64,
    /// Where the walls start, metres up (a building over a passage, a roof on posts).
    pub min_height: f64,
    /// The roof's own height, metres (0 when flat).
    pub roof_height: f64,
    /// One of [`ROOF_TYPES`].
    pub roof_type: &'static str,
    /// The walls' colour ([`hex_colour`]), where mapped.
    pub colour: Option<String>,
    /// Mapped materials (none where not mapped).
    pub facade: Option<Facade>,
    pub roof: Option<RoofMaterial>,
    pub roof_colour: Option<String>,
}

/// `FACADES`: OpenStreetMap's `building:material` to Streets GL's façades.
fn facade_of(material: &str) -> Option<Facade> {
    Some(match material {
        "brick" => Facade::Brick,
        "cement_block" | "block" => Facade::CementBlock,
        "wood" => Facade::Wood,
        "plaster" | "plastered" | "concrete" | "hard" => Facade::Plaster,
        "glass" | "mirror" => Facade::Glass,
        _ => return None,
    })
}

/// `ROOFS`: OpenStreetMap's `roof:material` to Streets GL's roofs.
fn roof_of(material: &str) -> Option<RoofMaterial> {
    Some(match material {
        "tile" | "tiles" | "roof_tiles" | "slate" => RoofMaterial::Tiles,
        "metal" | "metal_sheet" | "metal sheet" | "tin" | "copper" | "zinc" => RoofMaterial::Metal,
        "concrete" | "asphalt" => RoofMaterial::Concrete,
        "eternit" | "asbestos" => RoofMaterial::Eternit,
        "thatch" => RoofMaterial::Thatch,
        "grass" => RoofMaterial::Grass,
        "glass" => RoofMaterial::Glass,
        "tar_paper" => RoofMaterial::Tar,
        _ => return None,
    })
}

/// The roof shapes Streets GL draws; any other is flat.
pub const ROOF_TYPES: [&str; 12] = [
    "flat",
    "hipped",
    "gabled",
    "gambrel",
    "pyramidal",
    "onion",
    "dome",
    "round",
    "skillion",
    "mansard",
    "quadruple_saltbox",
    "saltbox",
];

/// A building's shape and looks from its tags, as Streets GL works them out
/// (`getBuildingParams`): a level is 4 m, one when not mapped.
pub fn sgl_building(p: &Properties) -> SglBuilding {
    const LEVEL: f64 = 4.0;
    let named = js_string_of(p.get("roofType"));
    let roof_type = ROOF_TYPES.into_iter().find(|&t| t == named).unwrap_or("flat");
    let levels_of_roof = some(p.get("roofLevels")).unwrap_or(if roof_type == "flat" { 0.0 } else { 1.0 });
    let mut roof_height = some(p.get("roofHeight")).unwrap_or(levels_of_roof * LEVEL);
    let height = some(p.get("height"));
    let min_height = num(p.get("minHeight"));
    let levels = some(p.get("levels"));
    let min_level = num(p.get("minLevel"));
    if let Some(height) = height {
        roof_height = js_min(roof_height, height - min_height.unwrap_or(0.0));
    }
    // (a tile decoded to binary has 0 for a minLevel not mapped: then one level, not none)
    let height = height.unwrap_or_else(|| {
        let below = match min_level {
            Some(m) if m != 0.0 => m + 1.0,
            _ => 1.0,
        };
        levels.unwrap_or(below) * LEVEL + roof_height
    });
    let mut min_height = min_height.unwrap_or_else(|| js_min(min_level.unwrap_or(0.0) * LEVEL, height));
    // a roof on posts: only its roof
    if p.get("buildingType").and_then(Value::as_str) == Some("roof") {
        min_height = js_max(min_height, height - js_max(roof_height, 1.0));
    }
    SglBuilding {
        height,
        min_height,
        roof_height: js_max(0.0, roof_height),
        roof_type,
        colour: hex_colour(p.get("color")),
        facade: facade_of(&js_string_of(p.get("material"))),
        roof: roof_of(&js_string_of(p.get("roofMaterial"))),
        roof_colour: hex_colour(p.get("roofColor")),
    }
}

/// A road's surface, as Streets GL draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    Asphalt,
    Dirt,
    Cobblestone,
    Concrete,
    Sand,
    Wood,
}

impl Surface {
    /// The TypeScript's name.
    pub fn as_str(self) -> &'static str {
        match self {
            Surface::Asphalt => "asphalt",
            Surface::Dirt => "dirt",
            Surface::Cobblestone => "cobblestone",
            Surface::Concrete => "concrete",
            Surface::Sand => "sand",
            Surface::Wood => "wood",
        }
    }
}

/// How a Streets GL road is laid, as their tiles' `pathType` and tags give it: its kind (the
/// realistic ground's), width (none: the kind's own) and surface.
#[derive(Debug, Clone, PartialEq)]
pub struct SglPath {
    pub kind: &'static str,
    pub width: Option<f64>,
    pub surface: Surface,
}

/// `SURFACES`: OpenStreetMap's `surface` to Streets GL's.
fn surface_of(surface: &str) -> Option<Surface> {
    Some(match surface {
        "asphalt" | "paved" => Surface::Asphalt,
        "unpaved" | "ground" | "gravel" | "dirt" | "grass" | "compacted" | "fine_gravel" | "earth" => Surface::Dirt,
        "concrete" => Surface::Concrete,
        "paving_stones" | "sett" | "cobblestone" | "pebblestone" => Surface::Cobblestone,
        "sand" => Surface::Sand,
        "wood" => Surface::Wood,
        _ => return None,
    })
}

/// A road as the realistic ground has it (its `ROADS` and `PATHS`), its lanes when not mapped,
/// and its width where the kind has its own.
struct Kind {
    kind: &'static str,
    lanes: f64,
    width: Option<f64>,
}

/// `KINDS`: Streets GL's `pathType` to a [`Kind`].
fn kind_of(path_type: &str) -> Option<Kind> {
    let (kind, lanes, width) = match path_type {
        "motorway" => ("motorway", 2.0, None),
        "motorway_link" => ("motorway", 1.0, None),
        "trunk" => ("trunk", 2.0, None),
        "trunk_link" => ("trunk", 1.0, None),
        "primary" => ("primary", 2.0, None),
        "primary_link" => ("primary", 1.0, None),
        "secondary" => ("secondary", 2.0, None),
        "secondary_link" => ("secondary", 1.0, None),
        "tertiary" => ("tertiary", 2.0, None),
        "tertiary_link" => ("tertiary", 1.0, None),
        "residential" => ("minor", 2.0, None),
        "unclassified" => ("minor", 2.0, None),
        "living_street" => ("minor", 2.0, None),
        "service" => ("service", 2.0, None),
        "track" => ("track", 1.0, None),
        "busway" => ("busway", 1.0, None),
        "raceway" => ("raceway", 1.0, Some(6.0)),
        "footway" => ("footway", 1.0, None),
        "path" => ("path", 1.0, None),
        "steps" => ("footway", 1.0, None),
        "platform" => ("pedestrian", 1.0, None),
        "pedestrian" => ("pedestrian", 1.0, Some(8.0)),
        "cycleway" => ("cycleway", 1.0, None),
        "runway" => ("runway", 1.0, Some(45.0)),
        "taxiway" => ("taxiway", 1.0, Some(20.0)),
        _ => return None,
    };
    Some(Kind { kind, lanes, width })
}

/// The kinds for walking and cycling.
const FOOT: [&str; 4] = ["footway", "path", "pedestrian", "cycleway"];

/// A road or path's kind, width (none: the kind's own) and surface; none for one Streets GL
/// doesn't draw.
pub fn sgl_path(p: &Properties) -> Option<SglPath> {
    let k = kind_of(&js_string_of(p.get("pathType")))?;
    let width = some(p.get("width"));
    let foot = FOOT.contains(&k.kind);
    let surface = surface_of(&js_string_of(p.get("surface")))
        .unwrap_or(if foot || k.kind == "track" { Surface::Dirt } else { Surface::Asphalt });
    if width.is_some() || k.width.is_some() || foot {
        return Some(SglPath { kind: k.kind, width: width.or(k.width), surface });
    }
    // a roadway as wide as its lanes, 3 m each (one alone is 4 m)
    let oneway = p.get("oneway") == Some(&Value::Bool(true));
    let lanes = some(p.get("lanes")).unwrap_or(if oneway { js_max(1.0, (k.lanes / 2.0).floor()) } else { k.lanes });
    Some(SglPath { kind: k.kind, width: Some(if lanes == 1.0 { 4.0 } else { lanes * 3.0 }), surface })
}

/// Streets GL's tree kinds, from `leafType` and `genus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeKind {
    Fir,
    Beech,
    Linden,
    Oak,
    /// Any broadleaf.
    Broadleaf,
}

impl TreeKind {
    /// The TypeScript's name.
    pub fn as_str(self) -> &'static str {
        match self {
            TreeKind::Fir => "fir",
            TreeKind::Beech => "beech",
            TreeKind::Linden => "linden",
            TreeKind::Oak => "oak",
            TreeKind::Broadleaf => "broadleaf",
        }
    }
}

/// A tree's kind: a fir, a beech, linden or oak, else any broadleaf.
pub fn sgl_tree(p: &Properties) -> TreeKind {
    // `String(p.genus ?? "").toLowerCase()`
    let genus = match p.get("genus") {
        None | Some(Value::Null) => String::new(),
        genus => js_string_of(genus).to_lowercase(),
    };
    if p.get("leafType").and_then(Value::as_str) == Some("needleleaved") || ["abies", "picea", "pinus"].contains(&genus.as_str()) {
        return TreeKind::Fir;
    }
    match genus.as_str() {
        "fagus" => TreeKind::Beech,
        "tilia" => TreeKind::Linden,
        "quercus" => TreeKind::Oak,
        _ => TreeKind::Broadleaf,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Tags from a JSON object.
    fn props(v: Value) -> Properties {
        match v {
            Value::Object(map) => map,
            other => panic!("not an object: {other}"),
        }
    }

    // describe("sglBuilding")

    #[test]
    fn levels_of_4_m_a_pitched_roof_a_level_more() {
        let b = sgl_building(&props(json!({"levels": 3})));
        assert_eq!((b.height, b.min_height, b.roof_height, b.roof_type), (12.0, 0.0, 0.0, "flat"));
        let b = sgl_building(&props(json!({"levels": 2, "roofType": "gabled"})));
        assert_eq!((b.height, b.roof_height, b.roof_type), (12.0, 4.0, "gabled"));
        assert_eq!(sgl_building(&props(json!({}))).height, 4.0);
        // as a tile decoded to binary has a building nothing is mapped for: one level, not none
        assert_eq!(sgl_building(&props(json!({"height": 0, "levels": 0, "minLevel": 0}))).height, 4.0);
        let b = sgl_building(&props(json!({"minLevel": 2})));
        assert_eq!((b.height, b.min_height), (12.0, 8.0));
    }

    #[test]
    fn a_mapped_height_holds_its_roof() {
        let b = sgl_building(&props(json!({"height": 9, "roofType": "hipped", "roofHeight": 12})));
        assert_eq!((b.height, b.roof_height), (9.0, 9.0));
    }

    #[test]
    fn numbers_a_binary_tile_fills_with_0_are_not_mapped() {
        let b = sgl_building(&props(json!({"height": 0, "levels": 0, "roofHeight": 0, "roofLevels": 0, "color": 0})));
        assert_eq!((b.height, b.roof_height, b.colour), (4.0, 0.0, None));
    }

    #[test]
    fn a_roof_on_posts_is_only_its_roof() {
        let canopy = sgl_building(&props(json!({"buildingType": "roof", "height": 5})));
        assert!(canopy.min_height >= 4.0, "{}", canopy.min_height);
    }

    #[test]
    fn materials_and_colours_as_mapped() {
        let b = sgl_building(&props(
            json!({"material": "brick", "roofMaterial": "roof_tiles", "color": 0xffff00, "roofColor": 0x123456}),
        ));
        assert_eq!(b.facade, Some(Facade::Brick));
        assert_eq!(b.roof, Some(RoofMaterial::Tiles));
        assert_eq!(b.colour.as_deref(), Some("yellow"));
        assert_eq!(b.roof_colour.as_deref(), Some("#123456"));
    }

    // describe("sglPath")

    #[test]
    fn roads_as_wide_as_their_lanes() {
        assert_eq!(
            sgl_path(&props(json!({"pathType": "residential"}))),
            Some(SglPath { kind: "minor", width: Some(6.0), surface: Surface::Asphalt })
        );
        assert_eq!(
            sgl_path(&props(json!({"pathType": "primary", "lanes": 4}))),
            Some(SglPath { kind: "primary", width: Some(12.0), surface: Surface::Asphalt })
        );
        assert_eq!(sgl_path(&props(json!({"pathType": "secondary", "oneway": true}))).and_then(|p| p.width), Some(4.0));
    }

    #[test]
    fn paths_surfaces_and_mapped_widths() {
        assert_eq!(
            sgl_path(&props(json!({"pathType": "footway"}))),
            Some(SglPath { kind: "footway", width: None, surface: Surface::Dirt })
        );
        assert_eq!(
            sgl_path(&props(json!({"pathType": "service", "surface": "sett", "width": 5}))),
            Some(SglPath { kind: "service", width: Some(5.0), surface: Surface::Cobblestone })
        );
        assert_eq!(sgl_path(&props(json!({"pathType": "runway"}))).and_then(|p| p.width), Some(45.0));
        assert_eq!(sgl_path(&props(json!({"pathType": "construction"}))), None);
    }

    #[test]
    fn tree_kinds() {
        assert_eq!(sgl_tree(&props(json!({"leafType": "needleleaved"}))), TreeKind::Fir);
        assert_eq!(sgl_tree(&props(json!({"genus": "Quercus"}))), TreeKind::Oak);
        assert_eq!(sgl_tree(&props(json!({}))), TreeKind::Broadleaf);
    }

    #[test]
    fn named_colours_back_to_their_names() {
        assert_eq!(hex_colour(Some(&json!(0xffffff))).as_deref(), Some("white"));
        assert_eq!(hex_colour(Some(&json!(0x334455))).as_deref(), Some("#334455"));
        assert_eq!(hex_colour(None), None);
    }

    // New tests (no TypeScript twin).

    #[test]
    fn only_zoom_16_is_asked_for() {
        assert_eq!(tile_url(35210, 20520, 16), "https://tiles.streets.gl/vector/16/35210/20520");
        assert!(served(&tile_url(35210, 20520, 16)));
        assert!(!served(&tile_url(17605, 10260, 15)));
        assert_eq!(SGL_FROM_ZOOM, 15.5);
    }

    #[test]
    fn tags_read_as_javascript_reads_them() {
        // `String(v)`: a number, a list, nothing
        assert_eq!(js_string_of(Some(&json!(4.0))), "4");
        assert_eq!(js_string_of(Some(&json!(["gabled"]))), "gabled");
        assert_eq!(js_string_of(Some(&json!([1, null, "a"]))), "1,,a");
        assert_eq!(js_string_of(None), "undefined");
        assert_eq!(sgl_building(&props(json!({"roofType": ["gabled"]}))).roof_type, "gabled");
        assert_eq!(sgl_building(&props(json!({"roofType": "Gabled"}))).roof_type, "flat");
        // a colour through 32 bits: masked, fractions dropped, black as `#000000` unless it is 0
        assert_eq!(hex_colour(Some(&json!(0x1ffffff))).as_deref(), Some("white"));
        assert_eq!(hex_colour(Some(&json!(0x1000000))).as_deref(), Some("#000000"));
        assert_eq!(hex_colour(Some(&json!(255.9))).as_deref(), Some("blue"));
        assert_eq!(hex_colour(Some(&json!(-1))).as_deref(), Some("white"));
        assert_eq!(hex_colour(Some(&json!(0x2a))).as_deref(), Some("#00002a"));
        assert_eq!(hex_colour(Some(&json!("#ff0000"))), None);
        // the oneway is only `true` itself
        assert_eq!(sgl_path(&props(json!({"pathType": "primary", "oneway": "yes"}))).and_then(|p| p.width), Some(6.0));
    }

    #[test]
    fn a_building_over_a_passage_and_one_mapped_to_the_metre() {
        let b = sgl_building(&props(json!({"levels": 2, "minLevel": 1, "roofLevels": 1, "roofType": "skillion"})));
        assert_eq!((b.height, b.min_height, b.roof_height), (12.0, 4.0, 4.0));
        let b = sgl_building(&props(json!({"height": 20, "minHeight": 18, "roofHeight": 5, "roofType": "dome"})));
        assert_eq!((b.height, b.min_height, b.roof_height, b.roof_type), (20.0, 18.0, 2.0, "dome"));
        // a roof on posts with a pitched roof: its roof, and a metre at least
        let b = sgl_building(&props(json!({"buildingType": "roof", "height": 6, "roofHeight": 0.5, "roofType": "hipped"})));
        assert_eq!(b.min_height, 5.0);
        let b = sgl_building(&props(json!({"material": "mirror", "roofMaterial": "metal sheet", "roofColor": 0xa0522d})));
        assert_eq!((b.facade, b.roof, b.roof_colour.as_deref()), (Some(Facade::Glass), Some(RoofMaterial::Metal), Some("brown")));
        assert_eq!(Facade::CementBlock.as_str(), "cementBlock");
    }

    #[test]
    fn every_kind_of_way_its_width_and_surface() {
        let width = |tags: Value| sgl_path(&props(tags)).and_then(|p| p.width);
        // a oneway motorway is one lane of 4 m; a link is one lane either way
        assert_eq!(width(json!({"pathType": "motorway", "oneway": true})), Some(4.0));
        assert_eq!(width(json!({"pathType": "trunk_link"})), Some(4.0));
        assert_eq!(width(json!({"pathType": "motorway", "lanes": 3, "oneway": true})), Some(9.0));
        assert_eq!(width(json!({"pathType": "pedestrian"})), Some(8.0));
        assert_eq!(width(json!({"pathType": "taxiway", "width": 0})), Some(20.0));
        let track = sgl_path(&props(json!({"pathType": "track"}))).unwrap();
        assert_eq!((track.kind, track.width, track.surface), ("track", Some(4.0), Surface::Dirt));
        let steps = sgl_path(&props(json!({"pathType": "steps", "surface": "wood"}))).unwrap();
        assert_eq!((steps.kind, steps.width, steps.surface), ("footway", None, Surface::Wood));
        assert_eq!(sgl_path(&props(json!({"pathType": "platform"}))).unwrap().kind, "pedestrian");
        assert_eq!(sgl_tree(&props(json!({"genus": "PICEA"}))), TreeKind::Fir);
        assert_eq!(sgl_tree(&props(json!({"genus": "Tilia", "leafType": "broadleaved"}))), TreeKind::Linden);
        assert_eq!(sgl_tree(&props(json!({"genus": "fagus"}))).as_str(), "beech");
        assert_eq!(sgl_tree(&props(json!({"genus": null}))), TreeKind::Broadleaf);
    }
}
