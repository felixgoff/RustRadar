//! Pure part of `realistic-ground.ts` (S1.28; doc 04 §6.1–6.5): what the realistic map's ground
//! is made of, from the map's own data, as Streets GL draws it.
//!
//! Every ground feature is given a "paint", an RGBA colour: RGB a tint (sRGB, multiplying the
//! texture; white leaves it as it is), alpha 255 less the id of its [`Surface`] (a texture, how it
//! is laid, how rough it is, whether snow lies on it). [`area_paint`] paints areas,
//! [`line_of`] gives a line its width, paint and edging (pavements, ballast). The tiles' layers
//! and the shader are the map crate's; [`surface_table`] gives the shader its tables.
//!
//! A feature's tags are [`Properties`] (`Record<string, unknown>`), with the tile layer's name in
//! `layerName` as deck.gl's loader puts it; the JavaScript conversions the TypeScript leans on
//! (`String(v)`, `Number(v)`, truthiness, `===`) are written out so the tables match exactly what
//! they matched there. Unlike the TypeScript's plain objects, the tables here have no
//! `Object.prototype` names in them (`constructor`, `toString`): a tag of that name finds nothing.

use std::f64::consts::PI;

use rustradar_core::num::js_round;
use serde_json::Value;

use crate::geom::{js_hypot2, js_max};
use crate::landcover::{LandClass, lc};
use crate::rail::{SpeedKind, speed_kind};
use crate::streets_gl::{Surface as SglSurface, js_string_of, sgl_path};

pub use crate::streets_gl::Properties;

/// A paint: tint (sRGB) and 255 less the surface's id.
pub type Rgba = [u8; 4];

// --------------------------------------------------------------- surfaces

/// The ground's texture layers, in the order the shaders index them (`realistic-textures.ts`
/// `GROUND_TEXTURES`, doc 04 §5.2).
pub const GROUND_TEXTURES: [&str; 23] = [
    "grass",
    "park",
    "forest",
    "farmland0",
    "farmland1",
    "farmland2",
    "garden",
    "sand",
    "rock",
    "soil",
    "pavement",
    "asphalt",
    "road",
    "road-unmarked",
    "gravel",
    "railway",
    "cobblestone",
    "dirt",
    "pitch",
    "cycleway",
    "wood",
    "tram",
    "terrain",
];

/// A kind of ground: its texture (or water), how many metres a repeat of it covers (0: laid along
/// a line, across its whole width), how rough it is, whether snow lies on it (1) or turns to slush
/// (0.5), and whether rain makes it shine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surface {
    /// One of [`GROUND_TEXTURES`], or `"water"` (drawn by the shader, no texture).
    pub texture: &'static str,
    pub metres: f64,
    pub rough: f64,
    pub snow: f64,
    pub paved: bool,
    /// Drawn in this colour (linear) instead of its texture: the roads and paths, plain.
    pub flat: Option<[f64; 3]>,
}

impl Surface {
    /// Its layer in the ground's texture arrays (`GS_LAYER`): -1 for water.
    pub fn layer(&self) -> f64 {
        if self.texture == "water" {
            return -1.0;
        }
        GROUND_TEXTURES.iter().position(|&t| t == self.texture).map_or(-1.0, |i| i as f64)
    }
}

/// A road's plain colour (linear): asphalt's.
const ROAD: [f64; 3] = [0.075, 0.075, 0.08];

const fn ground(texture: &'static str, metres: f64, rough: f64, snow: f64) -> Surface {
    Surface { texture, metres, rough, snow, paved: false, flat: None }
}

const fn paved(texture: &'static str, metres: f64, rough: f64, snow: f64, flat: Option<[f64; 3]>) -> Surface {
    Surface { texture, metres, rough, snow, paved: true, flat }
}

/// The surfaces, by the id a feature's paint carries in its alpha (255 less the id); each
/// texture's metres as Streets GL lays it.
const SURFACES: [(usize, Surface); 28] = [
    // where nothing is mapped: Streets GL's terrain (its detail repeats every 76 m)
    (0, ground("terrain", 76.0, 0.9, 1.0)),
    (1, ground("park", 20.0, 0.9, 1.0)),
    (2, ground("forest", 15.0, 0.95, 1.0)),
    (3, ground("farmland0", 50.0, 0.95, 1.0)),
    (4, ground("grass", 25.0, 0.95, 1.0)),
    (5, ground("garden", 16.0, 0.92, 1.0)),
    (6, ground("sand", 12.0, 0.9, 1.0)),
    (7, ground("rock", 32.0, 0.85, 1.0)),
    (8, ground("soil", 25.0, 0.95, 1.0)),
    (9, paved("pavement", 10.0, 0.8, 0.5, None)),
    (10, paved("asphalt", 20.0, 0.85, 0.5, None)),
    (11, ground("water", 0.0, 0.05, 0.0)),
    (12, ground("gravel", 8.0, 0.9, 1.0)),
    (13, paved("cobblestone", 6.0, 0.75, 0.5, None)),
    // glaciers and ice caps: snow, whatever the weather
    (14, ground("sand", 10.0, 0.6, 1.0)),
    // a pitch (one for every game: the map doesn't say which), and wooden planks
    (15, ground("pitch", 20.0, 0.9, 1.0)),
    (16, ground("wood", 4.0, 0.8, 1.0)),
    // a field as mapped (Streets GL's tiles): its crop and the lie of its rows in its colour
    (17, ground("farmland0", 50.0, 0.95, 1.0)),
    // lines: laid along them
    // (roads and paths in plain colours, the railways' tracks and the piers' boards in their textures)
    (32, paved("road", 0.0, 0.8, 0.5, Some(ROAD))),
    (33, paved("road-unmarked", 0.0, 0.8, 0.5, Some(ROAD))),
    (34, Surface { flat: Some([0.26, 0.2, 0.13]), ..ground("dirt", 0.0, 0.95, 1.0) }),
    (35, ground("railway", 0.0, 0.8, 0.5)),
    (39, paved("tram", 0.0, 0.8, 0.5, Some(ROAD))),
    (40, ground("wood", 0.0, 0.8, 1.0)),
    // lines with a texture of the world's
    (36, paved("pavement", 10.0, 0.8, 0.5, Some([0.3, 0.3, 0.29]))),
    (37, paved("asphalt", 20.0, 0.8, 0.5, Some(ROAD))),
    (38, Surface { flat: Some([0.32, 0.29, 0.25]), ..ground("gravel", 8.0, 0.9, 1.0) }),
    (41, paved("cycleway", 8.0, 0.8, 0.5, Some([0.2, 0.1, 0.085]))),
];

/// One more than the highest surface id: a paint whose id is this or more is not drawn.
pub const SURFACE_COUNT: usize = 42;

/// Farmland (fields laid out by the shader), a field as mapped, water, ice.
pub const FARMLAND: u8 = 3;
pub const FIELD: u8 = 17;
pub const WATER_ID: u8 = 11;
pub const ICE: u8 = 14;

/// The surface of an id; none for an unused one.
pub fn surface(id: usize) -> Option<&'static Surface> {
    SURFACES.iter().find(|(i, _)| *i == id).map(|(_, s)| s)
}

/// A shader table of one of the surfaces' properties, by id (`GS_LAYER`, `GS_METRES`, …; unused
/// ids take id 0's row), e.g. `surface_table(Surface::layer)`. The TypeScript wrote them into
/// the GLSL with three decimals, which every value here has or fewer.
pub fn surface_table(value: impl Fn(&Surface) -> f64) -> [f64; SURFACE_COUNT] {
    let unmapped = &SURFACES[0].1;
    std::array::from_fn(|i| value(surface(i).unwrap_or(unmapped)))
}

// --------------------------------------------------------------- paints

const WHITE: [u8; 3] = [255, 255, 255];

/// A paint of surface `id` tinted `rgb` (white: the texture as it is).
pub const fn paint(id: u8, rgb: [u8; 3]) -> Rgba {
    [rgb[0], rgb[1], rgb[2], 255 - id]
}

/// Nothing: left out.
pub const NONE: Rgba = [0, 0, 0, 0];

/// The ground where nothing else is mapped, as in Streets GL: its terrain in its biome's colour (temperate).
pub const LAND: Rgba = paint(0, [58, 81, 29]);
/// The globe itself, showing only past the map's edge at the poles: ice (sRGB).
pub const POLES: [u8; 3] = [226, 234, 240];

/// The areas' paints (`AREAS`); tints multiply a texture's own colours.
pub mod area {
    use super::{FARMLAND as FARMLAND_ID, Rgba, WATER_ID, WHITE, paint};

    pub const OCEAN: Rgba = paint(WATER_ID, [200, 214, 222]);
    pub const WATER: Rgba = paint(WATER_ID, WHITE);
    pub const POOL: Rgba = paint(WATER_ID, [150, 255, 255]);
    pub const WOOD: Rgba = paint(2, WHITE);
    pub const SCRUB: Rgba = paint(2, [230, 236, 214]);
    pub const GRASS: Rgba = paint(4, WHITE);
    pub const PARK: Rgba = paint(1, WHITE);
    pub const MEADOW: Rgba = paint(4, [255, 246, 206]);
    pub const GOLF: Rgba = paint(1, [236, 255, 228]);
    pub const HEATH: Rgba = paint(4, [236, 220, 196]);
    pub const FARMLAND: Rgba = paint(FARMLAND_ID, WHITE);
    pub const ORCHARD: Rgba = paint(5, [232, 240, 214]);
    pub const ALLOTMENTS: Rgba = paint(8, [226, 236, 206]);
    pub const WETLAND: Rgba = paint(4, [196, 214, 200]);
    pub const SAND: Rgba = paint(6, WHITE);
    pub const ROCK: Rgba = paint(7, WHITE);
    pub const ICE: Rgba = paint(14, WHITE);
    pub const RESIDENTIAL: Rgba = paint(5, WHITE);
    pub const INSTITUTIONAL: Rgba = paint(5, [236, 236, 226]);
    pub const COMMERCIAL: Rgba = paint(9, WHITE);
    pub const INDUSTRIAL: Rgba = paint(10, [236, 236, 236]);
    pub const RAILWAY: Rgba = paint(12, WHITE);
    pub const CEMETERY: Rgba = paint(1, [228, 236, 222]);
    pub const PITCH: Rgba = paint(15, WHITE);
    pub const ATHLETICS: Rgba = paint(8, [255, 150, 120]);
    pub const PLAYGROUND: Rgba = paint(6, WHITE);
    pub const QUARRY: Rgba = paint(8, [236, 228, 214]);
    pub const AERODROME: Rgba = paint(4, [236, 244, 222]);
    pub const APRON: Rgba = paint(10, [255, 255, 255]);
    pub const RUNWAY: Rgba = paint(10, [180, 180, 186]);
    pub const TAXIWAY: Rgba = paint(10, [214, 214, 218]);
    pub const SQUARE: Rgba = paint(13, WHITE);
    pub const PLATFORM: Rgba = paint(9, WHITE);
    pub const PIER: Rgba = paint(16, WHITE);
    pub const BRIDGE: Rgba = paint(9, WHITE);
}

/// The paint of a land cover class ([`lc`], `LAND_COVER_PAINTS`) where the map's data has
/// nothing: woods, felled woods, mires, fields, grass, scrub, heath, bare rock, ice. Towns and
/// water are left to the map's own data (none).
pub fn land_cover_paint(class: LandClass) -> Option<Rgba> {
    Some(match class {
        lc::CONIFERS | lc::MIXED | lc::BROADLEAVES | lc::FOREST => area::WOOD,
        lc::CLEARCUT => area::HEATH,
        lc::WETLAND => area::WETLAND,
        lc::ARABLE => area::FARMLAND,
        lc::GRASS => area::MEADOW,
        lc::SHRUB => area::SCRUB,
        lc::HEATH => area::HEATH,
        lc::BARE => area::ROCK,
        lc::SNOW => area::ICE,
        _ => return None,
    })
}

// --------------------------------------------------------------- JavaScript's conversions

/// `p[key] === value` for a string `value`.
fn is(p: &Properties, key: &str, value: &str) -> bool {
    p.get(key).and_then(Value::as_str) == Some(value)
}

/// `String(p[key] ?? "")`.
fn text<'a>(p: &'a Properties, key: &str) -> std::borrow::Cow<'a, str> {
    match p.get(key) {
        None | Some(Value::Null) => "".into(),
        v => js_string_of(v),
    }
}

/// Whether a tag is truthy (`if (p.service)`).
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
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

/// `x || 0` for a number: NaN and -0 are 0.
fn or_zero(x: f64) -> f64 {
    if x != 0.0 && !x.is_nan() { x } else { 0.0 }
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

// --------------------------------------------------------------- areas

/// An area's paint, or [`NONE`] (left out).
pub fn area_paint(p: &Properties) -> Rgba {
    let (cls, sub) = (text(p, "class"), text(p, "subclass"));
    let (cls, sub) = (&*cls, &*sub);
    match p.get("layerName").and_then(Value::as_str) {
        Some("water") => match cls {
            "ocean" => area::OCEAN,
            "swimming_pool" => area::POOL,
            _ => area::WATER,
        },
        Some("landcover") => match cls {
            "wood" => area::WOOD,
            "farmland" => match sub {
                "orchard" | "vineyard" | "plant_nursery" => area::ORCHARD,
                _ => area::FARMLAND,
            },
            "grass" => match sub {
                "scrub" | "shrubbery" => area::SCRUB,
                "heath" | "fell" => area::HEATH,
                "golf_course" => area::GOLF,
                "meadow" | "grassland" => area::MEADOW,
                "allotments" => area::ALLOTMENTS,
                "park" | "garden" | "village_green" | "recreation_ground" => area::PARK,
                _ => area::GRASS,
            },
            "wetland" => area::WETLAND,
            "sand" => area::SAND,
            "rock" => area::ROCK,
            "ice" => area::ICE,
            _ => NONE,
        },
        Some("landuse") => match cls {
            "residential" => area::RESIDENTIAL,
            "school" | "university" | "college" | "kindergarten" | "hospital" | "library" | "stadium" => area::INSTITUTIONAL,
            "commercial" | "retail" | "garages" | "bus_station" | "dam" => area::COMMERCIAL,
            "industrial" => area::INDUSTRIAL,
            "railway" => area::RAILWAY,
            "cemetery" => area::CEMETERY,
            "pitch" => area::PITCH,
            "track" => area::ATHLETICS,
            "playground" => area::PLAYGROUND,
            "quarry" => area::QUARRY,
            "theme_park" | "zoo" => area::PARK,
            // suburbs, quarters and neighbourhoods are names, not ground
            _ => NONE,
        },
        Some("aeroway") => match cls {
            "aerodrome" => area::AERODROME,
            "apron" => area::APRON,
            "runway" => area::RUNWAY,
            "taxiway" => area::TAXIWAY,
            _ => NONE,
        },
        Some("transportation") => {
            if cls == "pier" {
                area::PIER
            } else if cls == "bridge" {
                area::BRIDGE
            } else if sub == "platform" {
                area::PLATFORM
            } else {
                area::SQUARE
            }
        }
        // Streets GL's tiles (streets_gl), by `type`, as Streets GL draws them
        Some("common" | "natural") => {
            if is(p, "type", "farmland") {
                field_paint(p)
            } else {
                sgl_area(&js_string_of(p.get("type"))).unwrap_or(NONE)
            }
        }
        Some("highways") => {
            if is(p, "pathType", "pedestrian") || is(p, "pathType", "footway") || is(p, "pathType", "platform") {
                area::PLATFORM
            } else {
                area::APRON
            }
        }
        Some(PLATFORMS) => area::PLATFORM,
        _ => NONE,
    }
}

/// A mapped field's paint: in its colour, the lie of its rows (along its longer side, from the
/// oriented box Streets GL's tiles give it: red, 0 to 180°), its crop (green, one of three) and
/// its shade (blue), the last two picked by its OpenStreetMap id.
fn field_paint(p: &Properties) -> Rgba {
    let at = |k: &str| or_zero(js_number(p.get(k)));
    let (ax, ay) = (at("@ombb10") - at("@ombb00"), at("@ombb11") - at("@ombb01"));
    let (bx, by) = (at("@ombb20") - at("@ombb10"), at("@ombb21") - at("@ombb11"));
    let (dx, dy) = if js_hypot2(ax, ay) >= js_hypot2(bx, by) { (ax, ay) } else { (bx, by) };
    let angle = (((dy.atan2(dx) % PI) + PI) % PI) / PI;
    let id = or_zero(js_number(p.get("osmId")));
    let h = |salt: f64| {
        let x = (id * 12.9898 + salt * 78.233).sin() * 43758.5453;
        x - x.floor()
    };
    [js_round(angle * 255.0) as u8, ((h(1.0) * 3.0).floor() * 100.0) as u8, js_round(h(2.0) * 255.0) as u8, 255 - FIELD]
}

/// `SGL_AREAS`: Streets GL's area `type`s.
fn sgl_area(kind: &str) -> Option<Rgba> {
    Some(match kind {
        "forest" => area::WOOD,
        "scrub" => area::SCRUB,
        "grass" => area::GRASS,
        "garden" => area::RESIDENTIAL,
        "farmland" => area::FARMLAND,
        "fairway" => area::GOLF,
        "pitch" | "playground" | "dogPark" => area::PITCH,
        "sand" => area::SAND,
        "rock" => area::ROCK,
        "construction" | "brownfield" => area::QUARRY,
        "parking" | "apron" | "helipad" => area::APRON,
        "bridge" => area::BRIDGE,
        "pier" => area::PIER,
        _ => return None,
    })
}

// --------------------------------------------------------------- lines

/// A line's surface: its full width in metres and paint, and its edging either side (pavements,
/// ballast) and paint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub width: f64,
    pub paint: Rgba,
    pub edge: f64,
    pub edge_paint: Rgba,
}

impl Line {
    /// The width its edging is drawn at, under the line itself: 0 with none.
    pub fn edge_width(&self) -> f64 {
        if self.edge != 0.0 && !self.edge.is_nan() { self.width + 2.0 * self.edge } else { 0.0 }
    }
}

const PAVEMENT: Rgba = paint(36, WHITE);
const SHOULDER: Rgba = paint(37, [210, 210, 210]);
const BALLAST: Rgba = paint(38, WHITE);

const fn road(width: f64, marked: bool, edge: f64, edge_paint: Rgba) -> Line {
    Line { width, paint: paint(if marked { 32 } else { 33 }, WHITE), edge, edge_paint }
}

const fn plain(width: f64, paint: Rgba) -> Line {
    Line { width, paint, edge: 0.0, edge_paint: NONE }
}

/// OpenMapTiles' `transportation` classes (`ROADS`).
fn road_line(class: &str) -> Option<Line> {
    Some(match class {
        "motorway" => road(11.0, true, 1.5, SHOULDER),
        "trunk" => road(10.0, true, 1.5, SHOULDER),
        "primary" => road(9.0, true, 2.0, PAVEMENT),
        "secondary" => road(8.0, true, 2.0, PAVEMENT),
        "tertiary" => road(7.0, false, 1.8, PAVEMENT),
        "minor" => road(6.0, false, 1.6, PAVEMENT),
        "busway" => road(7.0, false, 1.8, PAVEMENT),
        "service" => road(4.0, false, 0.0, PAVEMENT),
        "raceway" => road(10.0, true, 0.0, PAVEMENT),
        "track" => plain(3.0, paint(34, WHITE)),
        // the track itself, rails on sleepers, on its ballast
        "rail" => RAIL,
        "transit" => TRANSIT,
        "pier" => plain(3.0, paint(40, WHITE)),
        _ => return None,
    })
}

const RAIL: Line = Line { width: 2.6, paint: paint(35, WHITE), edge: 1.0, edge_paint: BALLAST };
const TRANSIT: Line = Line { width: 2.2, paint: paint(35, WHITE), edge: 0.6, edge_paint: BALLAST };
const PEDESTRIAN: Line = plain(5.0, paint(36, WHITE));

/// The paths' subclasses (`PATHS`).
fn path_line(subclass: &str) -> Option<Line> {
    Some(match subclass {
        "footway" => plain(2.0, paint(36, WHITE)),
        "cycleway" => plain(2.5, paint(41, WHITE)),
        "pedestrian" => PEDESTRIAN,
        "path" => plain(1.6, paint(38, [236, 226, 206])),
        _ => return None,
    })
}

/// Waterways' widths, metres (`WATERWAYS`).
fn waterway_width(class: &str) -> Option<f64> {
    Some(match class {
        "river" => 18.0,
        "canal" => 12.0,
        "stream" => 3.0,
        "drain" | "ditch" => 1.5,
        _ => return None,
    })
}

/// Runways and taxiways (`AEROWAYS`).
fn aeroway_line(class: &str) -> Option<Line> {
    Some(match class {
        "runway" => plain(45.0, paint(37, [180, 180, 186])),
        "taxiway" => plain(18.0, paint(37, [214, 214, 218])),
        _ => return None,
    })
}

const TRAM: Line = plain(2.2, paint(39, WHITE));
const NO_LINE: Line = plain(0.0, NONE);

/// Streets GL's road surfaces as the ground's line paints (`SGL_SURFACES`): asphalt the road's own.
fn sgl_surface_paint(surface: SglSurface) -> Rgba {
    match surface {
        SglSurface::Asphalt => paint(37, WHITE),
        SglSurface::Concrete => paint(37, [226, 226, 222]),
        SglSurface::Cobblestone => paint(13, WHITE),
        SglSurface::Dirt => paint(34, WHITE),
        SglSurface::Sand => paint(34, [255, 236, 200]),
        SglSurface::Wood => paint(40, WHITE),
    }
}

/// OpenRailwayMap's tracks (the app's rail tiles, `scene::rail`): the very ones the trains run on.
pub const RAIL_LINES: &str = "railway_line_high";
/// OpenRailwayMap's stations' platforms, served (by the same tiles) at [`PLATFORM_ZOOM`] alone.
pub const PLATFORMS: &str = "standard_railway_platforms";
pub const PLATFORM_ZOOM: u32 = 15;
/// Platforms this near the camera are drawn, metres (`PLATFORM_LOD.maxDistance`): further off
/// they are too low to tell from the ground.
pub const PLATFORM_MAX_DISTANCE_M: f64 = 4000.0;
/// A platform's height above the rails, metres (Sweden's are 0.55 to 0.73 m).
pub const PLATFORM_M: f64 = 0.6;
/// The OpenMapTiles layers the ground is drawn from, and Streets GL's.
pub const GROUND_LAYERS: [&str; 6] = ["landuse", "landcover", "aeroway", "water", "waterway", "transportation"];
pub const SGL_GROUND_LAYERS: [&str; 4] = ["common", "natural", "water", "highways"];
/// Far out, the main roads and rivers stay a hairline this wide, pixels (but Streets GL's, close
/// in, as wide as they are).
pub const LINE_MIN_PIXELS: f64 = 0.6;
/// The shade a bridge throws on what it crosses (not lit: a shadow), RGBA.
pub const BRIDGE_SHADE: Rgba = [16, 20, 14, 70];

/// In the app the ground's railways are OpenRailwayMap's, as current as the track the trains run
/// on; the maps' own (out of date in places) are left out then, but for trams' rails in the street
/// (`OWN_RAILS = isTauri()`, always so in the native app).
pub const OWN_RAILS: bool = true;

/// How a line is drawn on the ground: a road, railway, path, river or runway (tunnels and ferries
/// not at all). In the app: the maps' own railways left out ([`OWN_RAILS`]).
pub fn line_of(p: &Properties) -> Line {
    line_with(p, OWN_RAILS)
}

/// [`line_of`] with the maps' own railways left out (`own_rails`, the app's way) or drawn (the
/// web page's, where the TypeScript's tests ran).
pub fn line_with(p: &Properties, own_rails: bool) -> Line {
    let (cls, sub) = (text(p, "class"), text(p, "subclass"));
    let (cls, sub) = (&*cls, &*sub);
    match p.get("layerName").and_then(Value::as_str) {
        Some("transportation") => {
            if is(p, "brunnel", "tunnel") || cls == "ferry" || cls.ends_with("_construction") {
                return NO_LINE;
            }
            if cls == "path" {
                let fallback = if sub == "steps" || sub == "platform" { "footway" } else { "path" };
                return path_line(sub).or_else(|| path_line(fallback)).unwrap_or(NO_LINE);
            }
            // a tram's rails are set in the street; light rail runs on its own ballast, as a railway
            if cls == "transit" && sub == "tram" {
                return TRAM;
            }
            if own_rails && (cls == "rail" || cls == "transit") {
                return NO_LINE;
            }
            as_surveyed(road_line(cls).unwrap_or(NO_LINE), p, false)
        }
        Some("waterway") => {
            if is(p, "brunnel", "tunnel") {
                return NO_LINE;
            }
            plain(waterway_width(cls).unwrap_or(2.0), area::WATER)
        }
        Some("aeroway") => aeroway_line(cls).unwrap_or(NO_LINE),
        // Streets GL's tiles: roads as wide as their lanes, of their own surface; railways; waterways
        Some("highways") => {
            if is(p, "type", "railway") {
                return if is(p, "railwayType", "tram") {
                    TRAM
                } else if own_rails {
                    NO_LINE
                } else {
                    RAIL
                };
            }
            let Some(path) = sgl_path(p) else { return NO_LINE };
            let own = aeroway_line(path.kind).or_else(|| path_line(path.kind)).or_else(|| road_line(path.kind)).unwrap_or(NO_LINE);
            let surface = sgl_surface_paint(path.surface);
            let mapped_width = js_number(p.get("width")) > 0.0;
            let sidewalk = truthy(p.get("sidewalkSide"));
            let line = Line {
                width: path.width.unwrap_or(own.width),
                // a road's own surface where it isn't asphalt; a path's as mapped
                paint: if path.surface != SglSurface::Asphalt || path_line(path.kind).is_some() { surface } else { own.paint },
                edge: if sidewalk { js_max(own.edge, 2.0) } else { own.edge },
                edge_paint: if sidewalk { PAVEMENT } else { own.edge_paint },
            };
            as_surveyed(line, p, mapped_width || matches!(p.get("surface"), Some(Value::String(_))))
        }
        Some("water") => {
            if !is(p, "type", "waterway") {
                return NO_LINE;
            }
            let width = or_zero(js_number(p.get("width")));
            let width = if width != 0.0 { width } else { waterway_width(&js_string_of(p.get("waterwayType"))).unwrap_or(2.0) };
            plain(width, area::WATER)
        }
        // OpenRailwayMap's tracks on their ballast (not in tunnels, nor trams': the maps' own, in the street)
        Some(RAIL_LINES) => {
            let kind = if truthy(p.get("tunnel")) { None } else { speed_kind(p) };
            match kind {
                Some(SpeedKind::Transit) => {
                    if is(p, "feature", "tram") {
                        NO_LINE
                    } else {
                        TRANSIT
                    }
                }
                Some(_) => RAIL,
                None => NO_LINE,
            }
        }
        // a platform mapped as a line, flat (as an area it stands up)
        Some(PLATFORMS) => PEDESTRIAN,
        _ => NO_LINE,
    }
}

/// A road as Trafikverket's survey has it (`road_widths`, its `nvdbWidth` and `nvdbGravel`),
/// where it has it: as wide as it is (unless `mapped` already says), and gravel where it is.
fn as_surveyed(line: Line, p: &Properties, mapped: bool) -> Line {
    let width = or_zero(js_number(p.get("nvdbWidth")));
    if width == 0.0 || mapped {
        return line;
    }
    let paint = if js_number(p.get("nvdbGravel")) > 0.0 { paint(34, WHITE) } else { line.paint };
    Line { width, paint, ..line }
}

/// Whether a tile's line is a road the survey may have (not a railway, path, runway or pier):
/// `road_widths`' `is_road`.
pub fn is_car_road(p: &Properties) -> bool {
    if is(p, "layerName", "transportation") {
        let cls = js_string_of(p.get("class"));
        return road_line(&cls).is_some() && !["rail", "transit", "pier", "track"].contains(&&*cls);
    }
    if !is(p, "layerName", "highways") || is(p, "type", "railway") {
        return false;
    }
    sgl_path(p).is_some_and(|path| road_line(path.kind).is_some() && path.kind != "track")
}

/// Whether a line is a railway's (its bridge decks are the rail map's).
pub fn on_rails(p: &Properties) -> bool {
    is(p, "class", "rail") || is(p, "class", "transit") || is(p, "layerName", RAIL_LINES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn f(v: Value) -> Properties {
        v.as_object().expect("an object").clone()
    }
    /// The surface is carried in the alpha: 255 less its id.
    fn surface_of(rgba: Rgba) -> u8 {
        255 - rgba[3]
    }

    // realistic map: the ground

    #[test]
    fn each_kind_of_area_has_its_own_surface_names_of_places_none() {
        assert_eq!(surface_of(area_paint(&f(json!({ "layerName": "water", "class": "lake" })))), 11);
        assert_eq!(surface_of(area_paint(&f(json!({ "layerName": "landcover", "class": "wood", "subclass": "forest" })))), 2);
        assert_eq!(surface_of(area_paint(&f(json!({ "layerName": "landcover", "class": "farmland", "subclass": "farmland" })))), 3);
        assert_eq!(surface_of(area_paint(&f(json!({ "layerName": "landuse", "class": "railway" })))), 12);
        assert_eq!(area_paint(&f(json!({ "layerName": "landuse", "class": "suburb" })))[3], 0);
        assert_eq!(area_paint(&f(json!({ "layerName": "building" })))[3], 0);
    }

    #[test]
    fn roads_at_their_widths_with_pavements_railways_on_ballast_tunnels_not_at_all() {
        // as bun ran it: outside the app (`isTauri()` false), the maps' own railways drawn
        let line_of = |v: Value| line_with(&f(v), false);
        let motorway = line_of(json!({ "layerName": "transportation", "class": "motorway" }));
        let minor = line_of(json!({ "layerName": "transportation", "class": "minor" }));
        assert!(motorway.width > minor.width);
        assert!(minor.edge > 0.0);
        assert_eq!(surface_of(motorway.paint), 32);
        assert_eq!(surface_of(minor.paint), 33);
        let rail = line_of(json!({ "layerName": "transportation", "class": "rail", "subclass": "rail" }));
        assert_eq!(surface_of(rail.paint), 35);
        assert_eq!(surface_of(rail.edge_paint), 38);
        assert_eq!(line_of(json!({ "layerName": "transportation", "class": "rail", "brunnel": "tunnel" })).width, 0.0);
        assert_eq!(line_of(json!({ "layerName": "transportation", "class": "ferry" })).width, 0.0);
        assert_eq!(line_of(json!({ "layerName": "transportation", "class": "path", "subclass": "footway" })).width, 2.0);
        assert_eq!(surface_of(line_of(json!({ "layerName": "waterway", "class": "river" })).paint), 11);
        // trams' rails in the street, light rail on ballast; piers of planks; cycleways and pitches their own
        assert_eq!(surface_of(line_of(json!({ "layerName": "transportation", "class": "transit", "subclass": "tram" })).paint), 39);
        let light_rail = line_of(json!({ "layerName": "transportation", "class": "transit", "subclass": "light_rail" }));
        assert_eq!(surface_of(light_rail.edge_paint), 38);
        assert_eq!(surface_of(line_of(json!({ "layerName": "transportation", "class": "pier" })).paint), 40);
        assert_eq!(surface_of(line_of(json!({ "layerName": "transportation", "class": "path", "subclass": "cycleway" })).paint), 41);
        assert_eq!(surface_of(area_paint(&f(json!({ "layerName": "landuse", "class": "pitch" })))), 15);
    }

    #[test]
    fn openrailwaymap_s_tracks_on_ballast_but_not_in_tunnels_out_of_use_or_trams_its_platforms_paved() {
        let orm = |extra: Value| {
            let mut p = f(json!({ "layerName": "railway_line_high", "state": "present" }));
            p.extend(f(extra));
            line_of(&p)
        };
        assert_eq!(surface_of(orm(json!({ "feature": "rail" })).paint), 35);
        assert_eq!(surface_of(orm(json!({ "feature": "rail", "service": "siding" })).edge_paint), 38);
        assert!(orm(json!({ "feature": "light_rail" })).width > 0.0);
        assert_eq!(orm(json!({ "feature": "rail", "tunnel": true })).width, 0.0);
        assert_eq!(orm(json!({ "feature": "rail", "state": "disused" })).width, 0.0);
        assert_eq!(orm(json!({ "feature": "tram" })).width, 0.0);
        assert_eq!(surface_of(area_paint(&f(json!({ "layerName": "standard_railway_platforms" })))), 9);
        assert!(line_of(&f(json!({ "layerName": "standard_railway_platforms" }))).width > 0.0);
    }

    // new tests

    #[test]
    fn in_the_app_the_maps_own_railways_are_left_out_but_trams_rails_in_the_street() {
        for p in [
            json!({ "layerName": "transportation", "class": "rail", "subclass": "rail" }),
            json!({ "layerName": "transportation", "class": "transit", "subclass": "light_rail" }),
            json!({ "layerName": "highways", "type": "railway", "railwayType": "rail" }),
        ] {
            assert_eq!(line_of(&f(p.clone())), NO_LINE, "{p}");
            assert!(line_with(&f(p), false).width > 0.0);
        }
        assert_eq!(line_of(&f(json!({ "layerName": "transportation", "class": "transit", "subclass": "tram" }))), TRAM);
        assert_eq!(line_of(&f(json!({ "layerName": "highways", "type": "railway", "railwayType": "tram" }))), TRAM);
    }

    #[test]
    fn the_surface_tables_are_the_shaders() {
        assert_eq!(SURFACE_COUNT, SURFACES.iter().map(|(i, _)| i + 1).max().unwrap_or(0));
        // doc 04 §6.2's GS_LAYER: unused ids (18 to 31) take id 0's row
        let mut layers = vec![22.0, 1.0, 2.0, 3.0, 0.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, -1.0, 14.0, 16.0, 7.0, 18.0, 20.0, 3.0];
        layers.extend([22.0; 14]);
        layers.extend([12.0, 13.0, 17.0, 15.0, 10.0, 11.0, 14.0, 21.0, 20.0, 19.0]);
        assert_eq!(surface_table(Surface::layer).to_vec(), layers);
        let metres = surface_table(|s| s.metres);
        assert_eq!((metres[0], metres[11], metres[20], metres[32], metres[36], metres[41]), (76.0, 0.0, 76.0, 0.0, 10.0, 8.0));
        let paved = surface_table(|s| f64::from(u8::from(s.paved)));
        // pavement, asphalt, cobbles, the roads, footways, shoulders, tram rails, cycleways
        let paved_ids: Vec<usize> = (0..SURFACE_COUNT).filter(|&i| paved[i] == 1.0).collect();
        assert_eq!(paved_ids, [9, 10, 13, 32, 33, 36, 37, 39, 41]);
        let flat_r = surface_table(|s| s.flat.map_or(-1.0, |c| c[0]));
        assert_eq!((flat_r[0], flat_r[32], flat_r[34], flat_r[35], flat_r[41]), (-1.0, 0.075, 0.26, -1.0, 0.2));
        assert_eq!(surface_table(|s| s.snow)[11], 0.0);
        // every paint's surface is one of them; nothing is drawn from SURFACE_COUNT up
        assert!(surface(usize::from(surface_of(LAND))).is_some());
        assert!(usize::from(surface_of(NONE)) >= SURFACE_COUNT);
        assert_eq!(land_cover_paint(lc::CLEARCUT), Some(area::HEATH));
        assert_eq!(land_cover_paint(lc::BUILT), None);
    }

    #[test]
    fn line_accessors() {
        let minor = line_of(&f(json!({ "layerName": "transportation", "class": "minor" })));
        assert_eq!(minor.edge_width(), 6.0 + 2.0 * 1.6);
        assert_eq!(line_of(&f(json!({ "layerName": "transportation", "class": "service" }))).edge_width(), 0.0);
        assert!(is_car_road(&f(json!({ "layerName": "transportation", "class": "minor" }))));
        assert!(!is_car_road(&f(json!({ "layerName": "transportation", "class": "track" }))));
        assert!(!is_car_road(&f(json!({ "layerName": "transportation", "class": "rail" }))));
        assert!(!is_car_road(&f(json!({ "layerName": "transportation", "class": "path" }))));
        assert!(is_car_road(&f(json!({ "layerName": "highways", "pathType": "residential" }))));
        assert!(!is_car_road(&f(json!({ "layerName": "highways", "pathType": "footway" }))));
        assert!(!is_car_road(&f(json!({ "layerName": "highways", "type": "railway", "pathType": "primary" }))));
        assert!(!is_car_road(&f(json!({ "layerName": "waterway", "class": "minor" }))));
        assert!(on_rails(&f(json!({ "layerName": "transportation", "class": "transit" }))));
        assert!(on_rails(&f(json!({ "layerName": "railway_line_high" }))));
        assert!(!on_rails(&f(json!({ "layerName": "transportation", "class": "minor" }))));
    }

    /// The TypeScript's answers (bun, outside the app) for the same tags.
    #[test]
    fn area_paints_match_the_typescript() {
        let cases = [
            (json!({"layerName": "water", "class": "ocean"}), [200, 214, 222, 244]),
            (json!({"layerName": "water", "class": "swimming_pool"}), [150, 255, 255, 244]),
            (json!({"layerName": "water"}), [255, 255, 255, 244]),
            (json!({"layerName": "landcover", "class": "farmland", "subclass": "vineyard"}), [232, 240, 214, 250]),
            (json!({"layerName": "landcover", "class": "grass", "subclass": "fell"}), [236, 220, 196, 251]),
            (json!({"layerName": "landcover", "class": "grass", "subclass": "golf_course"}), [236, 255, 228, 254]),
            (json!({"layerName": "landcover", "class": "grass", "subclass": "grassland"}), [255, 246, 206, 251]),
            (json!({"layerName": "landcover", "class": "grass", "subclass": "allotments"}), [226, 236, 206, 247]),
            (json!({"layerName": "landcover", "class": "grass", "subclass": "village_green"}), [255, 255, 255, 254]),
            (json!({"layerName": "landcover", "class": "grass"}), [255, 255, 255, 251]),
            (json!({"layerName": "landcover", "class": "wetland"}), [196, 214, 200, 251]),
            (json!({"layerName": "landcover", "class": "ice"}), [255, 255, 255, 241]),
            (json!({"layerName": "landcover", "class": "other"}), [0, 0, 0, 0]),
            (json!({"layerName": "landuse", "class": "hospital"}), [236, 236, 226, 250]),
            (json!({"layerName": "landuse", "class": "dam"}), [255, 255, 255, 246]),
            (json!({"layerName": "landuse", "class": "industrial"}), [236, 236, 236, 245]),
            (json!({"layerName": "landuse", "class": "cemetery"}), [228, 236, 222, 254]),
            (json!({"layerName": "landuse", "class": "track"}), [255, 150, 120, 247]),
            (json!({"layerName": "landuse", "class": "zoo"}), [255, 255, 255, 254]),
            (json!({"layerName": "landuse", "class": "quarry"}), [236, 228, 214, 247]),
            (json!({"layerName": "aeroway", "class": "runway"}), [180, 180, 186, 245]),
            (json!({"layerName": "aeroway", "class": "gate"}), [0, 0, 0, 0]),
            (json!({"layerName": "transportation", "class": "pier"}), [255, 255, 255, 239]),
            (json!({"layerName": "transportation", "class": "bridge"}), [255, 255, 255, 246]),
            (json!({"layerName": "transportation", "class": "minor", "subclass": "platform"}), [255, 255, 255, 246]),
            (json!({"layerName": "transportation", "class": "minor"}), [255, 255, 255, 242]),
            (json!({"layerName": "common", "type": "garden"}), [255, 255, 255, 250]),
            (json!({"layerName": "natural", "type": "dogPark"}), [255, 255, 255, 240]),
            (json!({"layerName": "common", "type": "nothing"}), [0, 0, 0, 0]),
            (json!({"layerName": "common"}), [0, 0, 0, 0]),
            (json!({"layerName": "common", "type": "farmland", "osmId": 123456, "@ombb00": 0.1, "@ombb01": 0.2, "@ombb10": 0.5, "@ombb11": 0.25, "@ombb20": 0.52, "@ombb21": 0.4}), [10, 100, 168, 238]),
            (json!({"layerName": "common", "type": "farmland", "osmId": "987", "@ombb00": 0, "@ombb01": 0, "@ombb10": 0.01, "@ombb11": 0.3, "@ombb20": 0.4, "@ombb21": 0.31}), [2, 0, 129, 238]),
            (json!({"layerName": "common", "type": "farmland", "osmId": 5, "@ombb00": 0, "@ombb01": 0, "@ombb10": -0.3, "@ombb11": -0.01, "@ombb20": -0.3, "@ombb21": -0.02}), [3, 200, 137, 238]),
            (json!({"layerName": "common", "type": "farmland"}), [0, 0, 7, 238]),
            (json!({"layerName": "highways", "pathType": "footway"}), [255, 255, 255, 246]),
            (json!({"layerName": "highways", "pathType": "primary"}), [255, 255, 255, 245]),
            (json!({"layerName": "standard_railway_platforms"}), [255, 255, 255, 246]),
            (json!({"layerName": 5}), [0, 0, 0, 0]),
            (json!({"class": "lake"}), [0, 0, 0, 0]),
        ];
        for (p, expected) in cases {
            assert_eq!(area_paint(&f(p.clone())), expected, "{p}");
        }
    }

    /// The TypeScript's answers (bun, outside the app) for the same tags: width, paint, edge, edge paint.
    #[test]
    fn lines_match_the_typescript() {
        let cases = [
            (json!({"layerName": "transportation", "class": "motorway"}), (11.0, [255, 255, 255, 223], 1.5, [210, 210, 210, 218])),
            (json!({"layerName": "transportation", "class": "trunk"}), (10.0, [255, 255, 255, 223], 1.5, [210, 210, 210, 218])),
            (json!({"layerName": "transportation", "class": "primary"}), (9.0, [255, 255, 255, 223], 2.0, [255, 255, 255, 219])),
            (json!({"layerName": "transportation", "class": "service"}), (4.0, [255, 255, 255, 222], 0.0, [255, 255, 255, 219])),
            (json!({"layerName": "transportation", "class": "raceway"}), (10.0, [255, 255, 255, 223], 0.0, [255, 255, 255, 219])),
            (json!({"layerName": "transportation", "class": "track"}), (3.0, [255, 255, 255, 221], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "transportation", "class": "busway"}), (7.0, [255, 255, 255, 222], 1.8, [255, 255, 255, 219])),
            (json!({"layerName": "transportation", "class": "pier"}), (3.0, [255, 255, 255, 215], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "transportation", "class": "nothing"}), (0.0, [0, 0, 0, 0], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "transportation", "class": "nothing", "nvdbWidth": 7.5}), (7.5, [0, 0, 0, 0], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "transportation", "class": "minor", "nvdbWidth": 7.5}), (7.5, [255, 255, 255, 222], 1.6, [255, 255, 255, 219])),
            (json!({"layerName": "transportation", "class": "minor", "nvdbWidth": 5.2, "nvdbGravel": 1}), (5.2, [255, 255, 255, 221], 1.6, [255, 255, 255, 219])),
            (json!({"layerName": "transportation", "class": "minor", "nvdbWidth": "4", "nvdbGravel": "0"}), (4.0, [255, 255, 255, 222], 1.6, [255, 255, 255, 219])),
            (json!({"layerName": "transportation", "class": "minor", "nvdbWidth": -3}), (-3.0, [255, 255, 255, 222], 1.6, [255, 255, 255, 219])),
            (json!({"layerName": "transportation", "class": "minor", "brunnel": "bridge"}), (6.0, [255, 255, 255, 222], 1.6, [255, 255, 255, 219])),
            (json!({"layerName": "transportation", "class": "minor", "brunnel": "tunnel"}), (0.0, [0, 0, 0, 0], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "transportation", "class": "primary_construction"}), (0.0, [0, 0, 0, 0], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "transportation", "class": "path", "subclass": "steps"}), (2.0, [255, 255, 255, 219], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "transportation", "class": "path", "subclass": "platform"}), (2.0, [255, 255, 255, 219], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "transportation", "class": "path", "subclass": "bridleway"}), (1.6, [236, 226, 206, 217], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "transportation", "class": "path", "subclass": "pedestrian"}), (5.0, [255, 255, 255, 219], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "transportation", "class": "path"}), (1.6, [236, 226, 206, 217], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "transportation", "class": "transit", "subclass": "subway"}), (2.2, [255, 255, 255, 220], 0.6, [255, 255, 255, 217])),
            (json!({"layerName": "waterway", "class": "river"}), (18.0, [255, 255, 255, 244], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "waterway", "class": "canal"}), (12.0, [255, 255, 255, 244], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "waterway", "class": "unknown"}), (2.0, [255, 255, 255, 244], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "waterway", "class": "stream", "brunnel": "tunnel"}), (0.0, [0, 0, 0, 0], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "aeroway", "class": "runway"}), (45.0, [180, 180, 186, 218], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "aeroway", "class": "taxiway"}), (18.0, [214, 214, 218, 218], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "aeroway", "class": "apron"}), (0.0, [0, 0, 0, 0], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "highways", "type": "railway", "railwayType": "tram"}), (2.2, [255, 255, 255, 216], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "highways", "type": "railway", "railwayType": "rail"}), (2.6, [255, 255, 255, 220], 1.0, [255, 255, 255, 217])),
            (json!({"layerName": "highways", "pathType": "primary"}), (6.0, [255, 255, 255, 223], 2.0, [255, 255, 255, 219])),
            (json!({"layerName": "highways", "pathType": "primary", "lanes": 4}), (12.0, [255, 255, 255, 223], 2.0, [255, 255, 255, 219])),
            (json!({"layerName": "highways", "pathType": "primary", "width": 14}), (14.0, [255, 255, 255, 223], 2.0, [255, 255, 255, 219])),
            (json!({"layerName": "highways", "pathType": "primary", "surface": "sett"}), (6.0, [255, 255, 255, 242], 2.0, [255, 255, 255, 219])),
            (json!({"layerName": "highways", "pathType": "primary", "surface": "asphalt"}), (6.0, [255, 255, 255, 223], 2.0, [255, 255, 255, 219])),
            (json!({"layerName": "highways", "pathType": "primary", "nvdbWidth": 9.5}), (9.5, [255, 255, 255, 223], 2.0, [255, 255, 255, 219])),
            (json!({"layerName": "highways", "pathType": "primary", "nvdbWidth": 9.5, "nvdbGravel": 1, "width": 12}), (12.0, [255, 255, 255, 223], 2.0, [255, 255, 255, 219])),
            (json!({"layerName": "highways", "pathType": "residential", "sidewalkSide": "both"}), (6.0, [255, 255, 255, 222], 2.0, [255, 255, 255, 219])),
            (json!({"layerName": "highways", "pathType": "service", "sidewalkSide": "left", "nvdbWidth": 3}), (3.0, [255, 255, 255, 222], 2.0, [255, 255, 255, 219])),
            (json!({"layerName": "highways", "pathType": "motorway", "sidewalkSide": ""}), (6.0, [255, 255, 255, 223], 1.5, [210, 210, 210, 218])),
            (json!({"layerName": "highways", "pathType": "footway"}), (2.0, [255, 255, 255, 221], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "highways", "pathType": "footway", "surface": "asphalt"}), (2.0, [255, 255, 255, 218], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "highways", "pathType": "cycleway", "surface": "paved"}), (2.5, [255, 255, 255, 218], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "highways", "pathType": "track"}), (4.0, [255, 255, 255, 221], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "highways", "pathType": "runway"}), (45.0, [180, 180, 186, 218], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "highways", "pathType": "pedestrian", "surface": "wood"}), (8.0, [255, 255, 255, 215], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "highways", "pathType": "unknown"}), (0.0, [0, 0, 0, 0], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "highways", "pathType": "raceway", "surface": "sand"}), (6.0, [255, 236, 200, 221], 0.0, [255, 255, 255, 219])),
            (json!({"layerName": "water", "type": "waterway", "waterwayType": "canal"}), (12.0, [255, 255, 255, 244], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "water", "type": "waterway", "width": 7}), (7.0, [255, 255, 255, 244], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "water", "type": "waterway", "width": "x", "waterwayType": "nothing"}), (2.0, [255, 255, 255, 244], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "water", "type": "water"}), (0.0, [0, 0, 0, 0], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "railway_line_high", "state": "present", "feature": "rail"}), (2.6, [255, 255, 255, 220], 1.0, [255, 255, 255, 217])),
            (json!({"layerName": "railway_line_high", "feature": "narrow_gauge", "service": "yard"}), (2.6, [255, 255, 255, 220], 1.0, [255, 255, 255, 217])),
            (json!({"layerName": "railway_line_high", "feature": "subway"}), (2.2, [255, 255, 255, 220], 0.6, [255, 255, 255, 217])),
            (json!({"layerName": "railway_line_high", "feature": "monorail", "tunnel": false}), (2.2, [255, 255, 255, 220], 0.6, [255, 255, 255, 217])),
            (json!({"layerName": "railway_line_high", "feature": "rail", "tunnel": 1}), (0.0, [0, 0, 0, 0], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "railway_line_high", "feature": "rail", "state": null}), (0.0, [0, 0, 0, 0], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "railway_line_high", "feature": "funicular"}), (0.0, [0, 0, 0, 0], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "standard_railway_platforms"}), (5.0, [255, 255, 255, 219], 0.0, [0, 0, 0, 0])),
            (json!({"layerName": "other"}), (0.0, [0, 0, 0, 0], 0.0, [0, 0, 0, 0])),
        ];
        for (p, (width, paint, edge, edge_paint)) in cases {
            let expected = Line { width, paint, edge, edge_paint };
            assert_eq!(line_with(&f(p.clone()), false), expected, "{p}");
        }
    }
}
