//! Port of the pure part of `rail-layer.ts` (S1.26; doc 02 §8, doc 05 §13.6): the rail map over
//! the dark map.
//!
//! The default is drawn from the OpenFreeMap vector tiles the map already uses (OpenMapTiles
//! schema, `transportation` layer): class `rail` (subclass rail, narrow_gauge, preserved,
//! funicular) and `transit` (light_rail, subway, tram, monorail), with `service` (yard, siding,
//! spur, crossover) and `brunnel` (bridge, tunnel). The tiles carry rail from zoom 8. Mainlines are
//! coral, sidings and yards dimmer and thinner, urban transit a muted violet, tunnels dashed and
//! faint. Stations (`poi` class railway) are marked from zoom 12.
//!
//! In the app (unless the signal blocks are shown: Globe.svelte's `railSpeeds`) the network is
//! instead the backend's own OpenRailwayMap tracks, coloured by their speed limit
//! ([`speed_kind`], [`rail_speed_color`], [`speed_labels`]). OpenRailwayMap's
//! "standard" raster can go on top close in; it is off by default and the app never turns it on
//! (doc 02 §8.5), but its rules ([`Backoff`] and the constants) are kept.
//!
//! What every layer is drawn with is here; the layers themselves (deck.gl's `MVTLayer`,
//! `TileLayer`, `BitmapLayer`, `TextLayer`) belong to the map crate. Transliterated as written, with
//! these differences:
//! - a feature is its properties ([`Properties`], as `bridges` has them), not `{ properties }`;
//! - colours are values: the TypeScript keeps one tunnel colour object per colour so that deck.gl
//!   sees the same object each time, which is moot here;
//! - [`rail_layers`] says which layers to draw and how (a [`RailLayer`] each, with deck.gl's layer
//!   ids); the overview's lazy loading is the app's, so whether it is in is an option
//!   ([`RailOptions::overview`]); the web-only options (`workerUrl`, `fetchImage`,
//!   `openRailwayMapUrl`) are gone;
//! - [`Backoff`] takes the time (`now_ms`) instead of reading a clock;
//! - [`speed_color`] takes `Option<f64>`: `None` for anything that is not a number;
//! - [`speed_labels`] gives a line without points (which the TypeScript would choke on) the
//!   position `[NaN, NaN]`.

use std::borrow::Cow;

use rustradar_core::num::{js_round, js_string};
use serde_json::Value;

pub use crate::bridges::{Coordinates, Geometry, LineFeature, Properties};
use crate::geom::{js_hypot2, js_max, js_min};
pub use crate::rail_overview::Rgba;
use crate::rail_overview::RailOverviewColors;

/// Transparent: what is not drawn.
pub const NONE: Rgba = [0, 0, 0, 0];
// Coral, the colour railway maps reach for: it stands apart from the grey roads and borders, the
// amber runways and aircraft, and the blue water.
/// Mainline track.
pub const RAIL_MAIN: Rgba = [240, 112, 96, 245];
/// Narrow gauge, heritage lines and funiculars.
pub const RAIL_MINOR: Rgba = [214, 128, 112, 225];
/// Yards, sidings, spurs and crossovers.
pub const RAIL_SERVICE: Rgba = [168, 102, 94, 200];
/// Light rail, trams, subways and monorails: violet, apart from the railways.
pub const RAIL_TRANSIT: Rgba = [178, 146, 240, 225];
/// Tunnels are drawn at this fraction of their colour's alpha (and dashed).
const TUNNEL_ALPHA: f64 = 0.55;

/// The overview's colours (`railLayers` passes `{ main: RAIL_MAIN, minor: RAIL_MINOR }`).
pub const OVERVIEW_COLORS: RailOverviewColors = RailOverviewColors { main: RAIL_MAIN, minor: RAIL_MINOR };

/// The tiles carry rail lines from this zoom.
pub const RAIL_MIN_ZOOM: f64 = 8.0;
/// The network's tiles go up to this zoom (over-zoomed past it).
pub const RAIL_MAX_TILE_ZOOM: f64 = 14.0;
/// Stations are marked from this zoom.
pub const STATION_MIN_ZOOM: f64 = 12.0;
/// The stations' tiles go up to this zoom (over-zoomed past it).
pub const STATION_MAX_TILE_ZOOM: f64 = 14.0;
/// OpenRailwayMap's raster, when enabled, from this zoom.
pub const OPENRAILWAYMAP_MIN_ZOOM: f64 = 10.0;

/// JavaScript's truthiness of a property (`if (p.service)`): absent, `null`, `false`, `0` and `""`
/// are false.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// `String(v)` of a property (`undefined` when absent).
fn js_to_string(v: Option<&Value>) -> Cow<'_, str> {
    match v {
        None => Cow::Borrowed("undefined"),
        Some(v) => value_string(v),
    }
}

fn value_string(v: &Value) -> Cow<'_, str> {
    match v {
        Value::Null => Cow::Borrowed("null"),
        Value::Bool(b) => Cow::Borrowed(if *b { "true" } else { "false" }),
        Value::Number(n) => Cow::Owned(js_string(n.as_f64().unwrap_or(f64::NAN))),
        Value::String(s) => Cow::Borrowed(s),
        // `Array.prototype.join`: null elements are empty
        Value::Array(items) => Cow::Owned(
            items.iter().map(|i| if i.is_null() { Cow::Borrowed("") } else { value_string(i) }).collect::<Vec<_>>().join(","),
        ),
        Value::Object(_) => Cow::Borrowed("[object Object]"),
    }
}

/// Whether a property is the string `s` (`p.x === "s"`).
fn is(v: Option<&Value>, s: &str) -> bool {
    matches!(v, Some(Value::String(v)) if v == s)
}

/// A property that is a number (`typeof v === "number"`).
fn number(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64(),
        _ => None,
    }
}

/// What kind of rail a `transportation` feature is (`RailKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RailKind {
    Main,
    Minor,
    Service,
    Transit,
}

impl RailKind {
    /// The kind's line colour (`KIND_COLOR`).
    pub fn color(self) -> Rgba {
        match self {
            RailKind::Main => RAIL_MAIN,
            RailKind::Minor => RAIL_MINOR,
            RailKind::Service => RAIL_SERVICE,
            RailKind::Transit => RAIL_TRANSIT,
        }
    }

    /// The kind's line width, pixels before the zoom scale (`KIND_WIDTH`).
    pub fn width(self) -> f64 {
        match self {
            RailKind::Main => 1.9,
            RailKind::Minor => 1.3,
            RailKind::Service => 0.8,
            RailKind::Transit => 1.2,
        }
    }
}

/// What kind of rail a `transportation` feature is, or `None` for anything else (roads, paths,
/// ferries).
pub fn rail_kind(p: &Properties) -> Option<RailKind> {
    if p.contains_key("layerName") && !is(p.get("layerName"), "transportation") {
        return None;
    }
    if !is(p.get("class"), "rail") && !is(p.get("class"), "transit") {
        return None;
    }
    if truthy(p.get("service")) {
        return Some(RailKind::Service);
    }
    if is(p.get("class"), "transit") {
        return Some(RailKind::Transit);
    }
    // a rail line without a subclass is taken for a mainline
    if !p.contains_key("subclass") || is(p.get("subclass"), "rail") { Some(RailKind::Main) } else { Some(RailKind::Minor) }
}

/// A colour as it is drawn in a tunnel: fainter (`tunnelOf`).
pub fn tunnel_of(c: Rgba) -> Rgba {
    [c[0], c[1], c[2], js_round(f64::from(c[3]) * TUNNEL_ALPHA) as u8]
}

/// Line colour of a feature: its rail kind's, fainter in tunnels; transparent for anything but
/// rail.
pub fn rail_color(p: &Properties) -> Rgba {
    let Some(kind) = rail_kind(p) else { return NONE };
    if is(p.get("brunnel"), "tunnel") { tunnel_of(kind.color()) } else { kind.color() }
}

/// Line width (pixels, before the zoom scale) of a feature; 0 for anything but rail.
pub fn rail_width(p: &Properties) -> f64 {
    rail_kind(p).map_or(0.0, RailKind::width)
}

/// No dashes.
pub const SOLID: [f64; 2] = [0.0, 0.0];
/// Tunnels' dashes: dash and gap, in line widths (drawn justified: `dashJustified: true`).
pub const TUNNEL_DASH: [f64; 2] = [3.0, 2.5];

/// Dash pattern (in line widths): tunnels dashed, the rest solid.
pub fn rail_dash(p: &Properties) -> [f64; 2] {
    if is(p.get("brunnel"), "tunnel") { TUNNEL_DASH } else { SOLID }
}

/// Lines thicken a little as the view closes in: x0.8 at zoom 8 to x1.8 at 15 (the layers'
/// `lineWidthScale`).
pub fn rail_width_scale(zoom: f64) -> f64 {
    let t = js_min(1.0, js_max(0.0, (zoom - 8.0) / 7.0));
    js_round((0.8 + t) * 10.0) / 10.0
}

const STATION_SUBCLASSES: [&str; 2] = ["station", "halt"];

/// Whether a `poi` feature is a railway station or halt (tram and subway stops are left to the
/// base map).
pub fn is_station(p: &Properties) -> bool {
    is(p.get("class"), "railway")
        && STATION_SUBCLASSES.contains(&js_to_string(p.get("subclass")).as_ref())
        && (!p.contains_key("layerName") || is(p.get("layerName"), "poi"))
}

/// A station's fill.
pub const STATION_FILL: Rgba = [214, 220, 232, 255];
/// A station's ring.
pub const STATION_RING: Rgba = [8, 12, 20, 230];
/// The ring's width, pixels.
pub const STATION_RING_WIDTH: f64 = 1.5;

/// A station's circle radius, pixels: 3.5 for a station, 2.5 for a halt, 0 for anything else.
pub fn station_radius(p: &Properties) -> f64 {
    if !is_station(p) {
        0.0
    } else if is(p.get("subclass"), "station") {
        3.5
    } else {
        2.5
    }
}

/// A station's fill colour; transparent for anything else.
pub fn station_fill(p: &Properties) -> Rgba {
    if is_station(p) { STATION_FILL } else { NONE }
}

/// A station's ring colour; transparent for anything else.
pub fn station_ring(p: &Properties) -> Rgba {
    if is_station(p) { STATION_RING } else { NONE }
}

// --- OpenRailwayMap ---------------------------------------------------------

/// OpenRailwayMap's "standard" style (track usage). Tiles are 512 px images for 256 px tiles.
pub const OPENRAILWAYMAP_URL: &str = "https://tiles.openrailwaymap.org/standard/{z}/{x}/{y}.png";
/// The attribution OpenRailwayMap's terms require (with the OpenStreetMap credit the map already
/// gives).
pub const OPENRAILWAYMAP_ATTRIBUTION: &str = "Rail overlay: © OpenStreetMap contributors, style CC-BY-SA 2.0 OpenRailwayMap";
/// Requests time out after this long, as its terms ask (ms).
pub const ORM_TIMEOUT_MS: i64 = 5_000;
/// After an HTTP 429, or several failures in a row, no requests for this long (ms).
pub const ORM_BACKOFF_MS: i64 = 15 * 60_000;
const ORM_FAILURES_BEFORE_BACKOFF: u32 = 4;
/// The raster's tiles go up to this zoom.
pub const OPENRAILWAYMAP_MAX_ZOOM: f64 = 18.0;
/// The raster's tile size, pixels.
pub const OPENRAILWAYMAP_TILE_SIZE: u32 = 256;
/// Light use: at most this many requests at a time.
pub const OPENRAILWAYMAP_MAX_REQUESTS: usize = 4;
/// And keep this many tiles once fetched.
pub const OPENRAILWAYMAP_CACHE_TILES: usize = 200;
/// The raster's opacity, so it sits in the dark map.
pub const OPENRAILWAYMAP_OPACITY: f64 = 0.85;

/// Keeps OpenRailwayMap's terms: a request in flight at most [`ORM_TIMEOUT_MS`]; after a 429 or a
/// run of failures, a long pause with no requests at all.
///
/// A tile fetch asks [`Backoff::paused`] first and fetches nothing while it is; then reports
/// [`Backoff::succeeded`] (an answer, a tile or none) or [`Backoff::failed`] (`true` for an HTTP
/// 429). The view moving on cancels a request: that's no failure of theirs, and is not reported.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Backoff {
    until: i64,
    failures: u32,
}

impl Backoff {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether no requests may go out now.
    pub fn paused(&self, now_ms: i64) -> bool {
        now_ms < self.until
    }

    pub fn succeeded(&mut self) {
        self.failures = 0;
    }

    pub fn failed(&mut self, rate_limited: bool, now_ms: i64) {
        self.failures += 1;
        if rate_limited || self.failures >= ORM_FAILURES_BEFORE_BACKOFF {
            self.until = now_ms + ORM_BACKOFF_MS;
            self.failures = 0;
        }
    }
}

// --- Speeds: OpenRailwayMap's tracks, through the backend -------------------

/// The railway as the backend routes trains on it: OpenRailwayMap's tracks
/// (`railway_line_high`), each with its speed limit, drawn up to the zoom routing uses, then
/// scaled up: a train on the track is on the line drawn. These are the tiles' zooms, (min, max).
pub const SPEED_TILE_ZOOMS: (f64, f64) = (7.0, 12.0);
/// Speed labels on the lines from this zoom.
pub const SPEED_LABEL_MIN_ZOOM: f64 = 12.5;
/// OpenRailwayMap's credit, with the OpenStreetMap one.
pub const RAIL_SPEED_ATTRIBUTION: &str = "Rail: OpenRailwayMap, © OpenStreetMap contributors";
/// The speed tiles' layer.
pub const SPEED_TILE_LAYER: &str = "railway_line_high";

/// The webview's tile URL: Windows and Android webviews reach custom protocols as
/// `http://<scheme>.localhost`. The native map calls the backend's tile function instead (doc 02
/// §8.2); kept for its test.
pub fn rail_tiles_url(user_agent: &str) -> &'static str {
    if user_agent.contains("Windows") || user_agent.contains("Android") {
        "http://railtiles.localhost/{z}/{x}/{y}"
    } else {
        "railtiles://localhost/{z}/{x}/{y}"
    }
}

/// A speed band: each limit up to `up_to` km/h gets the band's colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedBand {
    pub up_to: f64,
    pub color: Rgba,
    /// The legend's label.
    pub label: &'static str,
}

/// The speed bands, km/h: cool for slow, hot for fast. (The legend writes the last one "250+".)
pub const SPEED_BANDS: [SpeedBand; 6] = [
    SpeedBand { up_to: 40.0, color: [104, 128, 255, 240], label: "40" },
    SpeedBand { up_to: 80.0, color: [72, 186, 232, 240], label: "80" },
    SpeedBand { up_to: 120.0, color: [86, 206, 128, 240], label: "120" },
    SpeedBand { up_to: 160.0, color: [228, 212, 84, 245], label: "160" },
    SpeedBand { up_to: 200.0, color: [246, 146, 62, 245], label: "200" },
    SpeedBand { up_to: f64::INFINITY, color: [242, 78, 104, 250], label: "250" },
];
/// Track whose limit isn't mapped.
pub const SPEED_UNKNOWN: Rgba = [138, 142, 156, 190];

/// The colour of a speed limit, km/h (`None`: not a number).
pub fn speed_color(kmh: Option<f64>) -> Rgba {
    let Some(kmh) = kmh.filter(|&k| k > 0.0) else { return SPEED_UNKNOWN };
    SPEED_BANDS.iter().find(|b| kmh <= b.up_to).unwrap_or(&SPEED_BANDS[SPEED_BANDS.len() - 1]).color
}

const MAIN_FEATURES: [&str; 2] = ["rail", "narrow_gauge"];
const TRANSIT_FEATURES: [&str; 4] = ["tram", "subway", "light_rail", "monorail"];

/// How an OpenRailwayMap track is drawn ([`speed_kind`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpeedKind {
    /// A running line, by its limit.
    Line,
    /// A service track, by its limit.
    Service,
    /// Urban transit, violet.
    Transit,
}

impl SpeedKind {
    /// The kind's line width, pixels before the zoom scale (`SPEED_WIDTH`).
    pub fn width(self) -> f64 {
        match self {
            SpeedKind::Line => 2.2,
            SpeedKind::Service => 0.9,
            SpeedKind::Transit => 1.1,
        }
    }
}

/// How an OpenRailwayMap track is drawn: running line, service track, urban transit, or not at all.
pub fn speed_kind(p: &Properties) -> Option<SpeedKind> {
    if p.contains_key("state") && !is(p.get("state"), "present") {
        return None;
    }
    let feature = js_to_string(p.get("feature"));
    if TRANSIT_FEATURES.contains(&feature.as_ref()) {
        return Some(SpeedKind::Transit);
    }
    if !MAIN_FEATURES.contains(&feature.as_ref()) {
        return None;
    }
    Some(if truthy(p.get("service")) { SpeedKind::Service } else { SpeedKind::Line })
}

/// Line colour: running lines and service tracks by their limit, transit violet, tunnels fainter.
pub fn rail_speed_color(p: &Properties) -> Rgba {
    let Some(kind) = speed_kind(p) else { return NONE };
    let c = if kind == SpeedKind::Transit { RAIL_TRANSIT } else { speed_color(number(p.get("maxspeed"))) };
    if truthy(p.get("tunnel")) { tunnel_of(c) } else { c }
}

/// Line width (pixels, before the zoom scale) of a track; 0 for what is not drawn.
pub fn rail_speed_width(p: &Properties) -> f64 {
    speed_kind(p).map_or(0.0, SpeedKind::width)
}

/// Dash pattern (in line widths): tunnels dashed, the rest solid.
pub fn rail_speed_dash(p: &Properties) -> [f64; 2] {
    if truthy(p.get("tunnel")) { TUNNEL_DASH } else { SOLID }
}

/// A speed limit written on its line.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeedLabel {
    /// In the features' units.
    pub position: [f64; 2],
    pub text: String,
    /// The label's background: the limit's colour.
    pub color: Rgba,
}

/// The labels' text colour.
pub const SPEED_LABEL_TEXT: Rgba = [12, 14, 20, 255];
/// The labels' text size, pixels.
pub const SPEED_LABEL_SIZE_PX: f64 = 10.0;
/// The labels' font weight.
pub const SPEED_LABEL_WEIGHT: u16 = 650;
/// Padding round a label's text, pixels (x, y).
pub const SPEED_LABEL_PADDING: [f64; 2] = [3.0, 1.0];
/// A line shorter than this fraction of its tile carries no label ([`speed_labels`]'s default).
pub const SPEED_LABEL_MIN_LENGTH: f64 = 0.06;
/// No label this close (as a fraction of the tile) to another of the same limit.
pub const SPEED_LABEL_SPACING: f64 = 0.18;

fn at(p: &[f64], k: usize) -> f64 {
    p.get(k).copied().unwrap_or(f64::NAN)
}

fn length_of(line: &[Vec<f64>]) -> f64 {
    let mut d = 0.0;
    for i in 1..line.len() {
        d += js_hypot2(at(&line[i], 0) - at(&line[i - 1], 0), at(&line[i], 1) - at(&line[i - 1], 1));
    }
    d
}

/// The point halfway along a line.
fn midpoint(line: &[Vec<f64>]) -> [f64; 2] {
    let mut left = length_of(line) / 2.0;
    for i in 1..line.len() {
        let (a, b) = (&line[i - 1], &line[i]);
        let d = js_hypot2(at(b, 0) - at(a, 0), at(b, 1) - at(a, 1));
        if d >= left && d > 0.0 {
            return [at(a, 0) + ((at(b, 0) - at(a, 0)) * left) / d, at(a, 1) + ((at(b, 1) - at(a, 1)) * left) / d];
        }
        left -= d;
    }
    line.last().map_or([f64::NAN; 2], |p| [at(p, 0), at(p, 1)])
}

/// Where a tile's speed labels go: one on each running line long enough to carry one, longest
/// first, and none near another of the same speed (a double track says it once). `size` is the
/// tile's width in the features' units (1 for tile-local coordinates, the tile's span in degrees
/// on the globe); lines shorter than `min_length` of it and labels closer than `spacing` of it are
/// left out ([`SPEED_LABEL_MIN_LENGTH`] and [`SPEED_LABEL_SPACING`] in the map).
pub fn speed_labels(features: &[LineFeature], size: f64, min_length: f64, spacing: f64) -> Vec<SpeedLabel> {
    struct Candidate<'a> {
        line: &'a [Vec<f64>],
        length: f64,
        kmh: f64,
    }
    let mut candidates = Vec::new();
    for f in features {
        // (`typeof kmh !== "number" || !(kmh > 0)`)
        let Some(kmh) = number(f.properties.get("maxspeed")).filter(|&k| k > 0.0) else { continue };
        if speed_kind(&f.properties) != Some(SpeedKind::Line) {
            continue;
        }
        let lines: &[Vec<Vec<f64>>] = match (f.geometry.kind.as_str(), &f.geometry.coordinates) {
            ("LineString", Coordinates::Line(line)) => std::slice::from_ref(line),
            ("MultiLineString", Coordinates::Lines(lines)) => lines,
            _ => &[],
        };
        for line in lines {
            let length = length_of(line);
            if length >= min_length * size {
                candidates.push(Candidate { line, length, kmh });
            }
        }
    }
    // stable, longest first (no NaN gets this far; two infinities are equal, as `NaN` from
    // `b.length - a.length` reads in a JavaScript sort)
    candidates.sort_by(|a, b| b.length.partial_cmp(&a.length).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<SpeedLabel> = Vec::new();
    let near = spacing * size;
    for c in candidates {
        let position = midpoint(c.line);
        let text = js_string(js_round(c.kmh));
        if out.iter().any(|l| l.text == text && js_hypot2(l.position[0] - position[0], l.position[1] - position[1]) < near) {
            continue;
        }
        out.push(SpeedLabel { position, text, color: speed_color(Some(c.kmh)) });
    }
    out
}

// --- the layers ---------------------------------------------------------------

/// One of the rail map's layers, as [`rail_layers`] lists them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RailLayer {
    /// Zoomed out, the shipped overview (`rail_overview`), in [`OVERVIEW_COLORS`].
    Overview { flat: bool },
    /// The backend's tracks ([`SPEED_TILE_LAYER`], tile zooms [`SPEED_TILE_ZOOMS`]) coloured by
    /// their limits ([`rail_speed_color`], [`rail_speed_width`], [`rail_speed_dash`]); in the flat
    /// projection their bridges raised with `bridges::orm_deck`. `labels`: with
    /// [`speed_labels`] on each tile (from [`SPEED_LABEL_MIN_ZOOM`]).
    Speed { flat: bool, labels: bool },
    /// OpenFreeMap's rail lines (layer `transportation`, tile zooms [`RAIL_MIN_ZOOM`] to
    /// [`RAIL_MAX_TILE_ZOOM`]): [`rail_color`], [`rail_width`], [`rail_dash`]; in the flat
    /// projection their bridges raised with `bridges::omt_deck`.
    Network { flat: bool },
    /// OpenRailwayMap's raster, one layer for both projections.
    OpenRailwayMap,
    /// Stations and halts as small rings (layer `poi`, tile zooms [`STATION_MIN_ZOOM`] to
    /// [`STATION_MAX_TILE_ZOOM`]): [`station_radius`], [`station_fill`], [`station_ring`].
    Stations { flat: bool },
}

impl RailLayer {
    /// deck.gl's layer id: each projection has its own layer (and tile cache), as the map does.
    pub fn id(&self) -> &'static str {
        match *self {
            RailLayer::Overview { flat } => crate::rail_overview::rail_overview_layer_id(flat),
            RailLayer::Speed { flat, .. } => {
                if flat {
                    "rail-speed-flat"
                } else {
                    "rail-speed-globe"
                }
            }
            RailLayer::Network { flat } => {
                if flat {
                    "rail-flat"
                } else {
                    "rail-globe"
                }
            }
            RailLayer::OpenRailwayMap => "rail-openrailwaymap",
            RailLayer::Stations { flat } => {
                if flat {
                    "rail-stations-flat"
                } else {
                    "rail-stations-globe"
                }
            }
        }
    }
}

/// What the rail layers are drawn for (`RailOptions`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RailOptions<'a> {
    /// OpenFreeMap's vector tile URL template (from the TileJSON); `None` while it loads.
    pub tile_url: Option<&'a str>,
    /// The flat Web Mercator projection is in use (zoom above 12).
    pub flat: bool,
    pub zoom: f64,
    /// Whether the shipped overview is in (`railOverview()` gives it once loaded; never if it
    /// can't be had).
    pub overview: bool,
    /// Mark stations and halts from zoom 12. Default true.
    pub stations: bool,
    /// Add OpenRailwayMap's raster from zoom 10. Default false: its terms ask for a unique
    /// User-Agent.
    pub open_railway_map: bool,
    /// Draw the network from the backend's OpenRailwayMap tracks, coloured by speed limit. Else
    /// OpenFreeMap's rail lines. Default false.
    pub speeds: bool,
}

impl<'a> RailOptions<'a> {
    /// The defaults: stations on, the raster and the speeds off, no overview yet.
    pub fn new(tile_url: Option<&'a str>, flat: bool, zoom: f64) -> Self {
        Self { tile_url, flat, zoom, overview: false, stations: true, open_railway_map: false, speeds: false }
    }
}

/// The rail layers for the current view, in drawing order. They go after the map (the vector map
/// and the aeroways) and before the buildings, ships and labels: like the map they are ground
/// layers drawn in layer order. With OpenRailwayMap on, the vector network stays underneath it:
/// the raster covers the same lines in its own colours and the vector lines fill in while its
/// tiles load.
pub fn rail_layers(o: &RailOptions) -> Vec<RailLayer> {
    let RailOptions { tile_url, flat, zoom, overview, stations, open_railway_map, speeds } = *o;
    let tiles = tile_url.is_some_and(|u| !u.is_empty());
    let mut layers = Vec::new();
    // zoomed out, where the tiles have no rail yet, the shipped overview; the two hand over at one
    // zoom, as the overview is coarse and doubles the tiles' lines wherever both are drawn
    if zoom < RAIL_MIN_ZOOM && overview {
        layers.push(RailLayer::Overview { flat });
    }
    if speeds && zoom >= RAIL_MIN_ZOOM {
        layers.push(RailLayer::Speed { flat, labels: zoom >= SPEED_LABEL_MIN_ZOOM });
    } else if tiles && zoom >= RAIL_MIN_ZOOM {
        layers.push(RailLayer::Network { flat });
    }
    if open_railway_map && zoom >= OPENRAILWAYMAP_MIN_ZOOM {
        layers.push(RailLayer::OpenRailwayMap);
    }
    if tiles && stations && zoom >= STATION_MIN_ZOOM {
        layers.push(RailLayer::Stations { flat });
    }
    layers
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn props(v: Value) -> Properties {
        v.as_object().expect("an object").clone()
    }

    /// `t(p)`: a `transportation` feature's properties.
    fn t(p: Value) -> Properties {
        let mut out = props(json!({ "layerName": "transportation" }));
        out.extend(props(p));
        out
    }

    #[test]
    fn rail_features_by_kind() {
        assert_eq!(rail_kind(&t(json!({ "class": "rail", "subclass": "rail" }))), Some(RailKind::Main));
        assert_eq!(rail_kind(&t(json!({ "class": "rail" }))), Some(RailKind::Main));
        assert_eq!(rail_kind(&t(json!({ "class": "rail", "subclass": "narrow_gauge" }))), Some(RailKind::Minor));
        assert_eq!(rail_kind(&t(json!({ "class": "rail", "subclass": "preserved" }))), Some(RailKind::Minor));
        assert_eq!(rail_kind(&t(json!({ "class": "rail", "subclass": "rail", "service": "yard" }))), Some(RailKind::Service));
        assert_eq!(rail_kind(&t(json!({ "class": "transit", "subclass": "subway", "service": "siding" }))), Some(RailKind::Service));
        assert_eq!(rail_kind(&t(json!({ "class": "transit", "subclass": "tram" }))), Some(RailKind::Transit));
        assert_eq!(rail_kind(&t(json!({ "class": "transit", "subclass": "light_rail" }))), Some(RailKind::Transit));
    }

    #[test]
    fn roads_paths_and_other_layers_are_not_rail() {
        assert_eq!(rail_kind(&t(json!({ "class": "motorway" }))), None);
        assert_eq!(rail_kind(&t(json!({ "class": "path" }))), None);
        assert_eq!(rail_kind(&props(json!({ "layerName": "aeroway", "class": "rail" }))), None);
        assert_eq!(rail_color(&t(json!({ "class": "primary" })))[3], 0);
        assert_eq!(rail_width(&t(json!({ "class": "primary" }))), 0.0);
    }

    #[test]
    fn mainlines_are_the_brightest_and_widest_service_track_the_dimmest_and_thinnest() {
        let main = t(json!({ "class": "rail", "subclass": "rail" }));
        let yard = t(json!({ "class": "rail", "subclass": "rail", "service": "yard" }));
        let tram = t(json!({ "class": "transit", "subclass": "tram" }));
        assert_eq!(rail_color(&main), RAIL_MAIN);
        assert_eq!(rail_color(&yard), RAIL_SERVICE);
        assert_eq!(rail_color(&tram), RAIL_TRANSIT);
        assert!(rail_width(&main) > rail_width(&tram));
        assert!(rail_width(&yard) < rail_width(&tram));
    }

    #[test]
    fn tunnels_are_fainter_and_dashed_the_same_colour_object_each_time() {
        let tunnel = t(json!({ "class": "rail", "subclass": "rail", "brunnel": "tunnel" }));
        let c = rail_color(&tunnel);
        assert_eq!(c[..3], RAIL_MAIN[..3]);
        assert!(c[3] < RAIL_MAIN[3]);
        // (colours are values here: the same value each time)
        assert_eq!(rail_color(&tunnel), c);
        assert!(rail_dash(&tunnel)[0] > 0.0);
        assert_eq!(rail_dash(&t(json!({ "class": "rail", "brunnel": "bridge" }))), [0.0, 0.0]);
    }

    #[test]
    fn lines_thicken_as_the_view_closes_in_within_bounds() {
        assert_eq!(rail_width_scale(5.0), 0.8);
        assert!(rail_width_scale(10.0) > rail_width_scale(8.0));
        assert_eq!(rail_width_scale(20.0), 1.8);
    }

    #[test]
    fn stations_and_halts_not_tram_or_subway_stops() {
        assert!(is_station(&props(json!({ "layerName": "poi", "class": "railway", "subclass": "station" }))));
        assert!(is_station(&props(json!({ "layerName": "poi", "class": "railway", "subclass": "halt" }))));
        assert!(!is_station(&props(json!({ "layerName": "poi", "class": "railway", "subclass": "tram_stop" }))));
        assert!(!is_station(&props(json!({ "layerName": "poi", "class": "bus", "subclass": "station" }))));
    }

    #[test]
    fn layers_by_zoom_the_network_from_8_stations_from_12_open_railway_map_only_when_asked_from_10() {
        let tile_url = "https://tiles.example/{z}/{x}/{y}.pbf";
        let base = RailOptions::new(Some(tile_url), false, 9.0);
        let ids = |o: RailOptions| rail_layers(&o).iter().map(RailLayer::id).collect::<Vec<_>>();
        assert!(ids(RailOptions { zoom: 6.0, ..base }).is_empty());
        assert_eq!(ids(RailOptions { zoom: 9.0, ..base }), ["rail-globe"]);
        assert_eq!(ids(RailOptions { zoom: 12.0, ..base }), ["rail-globe", "rail-stations-globe"]);
        assert_eq!(ids(RailOptions { zoom: 13.0, flat: true, ..base }), ["rail-flat", "rail-stations-flat"]);
        assert_eq!(ids(RailOptions { zoom: 13.0, flat: true, stations: false, ..base }), ["rail-flat"]);
        assert_eq!(ids(RailOptions { zoom: 9.0, open_railway_map: true, ..base }), ["rail-globe"]);
        assert_eq!(ids(RailOptions { zoom: 11.0, open_railway_map: true, ..base }), ["rail-globe", "rail-openrailwaymap"]);
        // no vector tiles yet: only the raster, if on
        assert_eq!(
            ids(RailOptions { open_railway_map: true, ..RailOptions::new(None, false, 11.0) }),
            ["rail-openrailwaymap"]
        );
    }

    #[test]
    fn open_railway_map_backoff_a_429_pauses_at_once_other_failures_after_a_run_of_them() {
        let mut now = 0;
        let mut b = Backoff::new();
        assert!(!b.paused(now));
        b.failed(false, now);
        b.failed(false, now);
        b.succeeded();
        b.failed(false, now);
        b.failed(false, now);
        b.failed(false, now);
        assert!(!b.paused(now));
        b.failed(false, now);
        assert!(b.paused(now));
        now += 16 * 60_000;
        assert!(!b.paused(now));
        b.failed(true, now);
        assert!(b.paused(now));
    }

    /// `line(coordinates, properties)` of the speed-limit tests.
    fn line(coordinates: &[[f64; 2]], properties: Value) -> LineFeature {
        let mut p = props(json!({ "feature": "rail", "state": "present" }));
        p.extend(props(properties));
        LineFeature {
            geometry: Geometry { kind: "LineString".into(), coordinates: Coordinates::Line(coordinates.iter().map(|c| c.to_vec()).collect()) },
            properties: p,
        }
    }

    #[test]
    fn each_limit_gets_its_bands_colour_unmapped_and_other_railways_their_own() {
        assert_eq!(speed_color(Some(30.0)), SPEED_BANDS[0].color);
        assert_eq!(speed_color(Some(80.0)), SPEED_BANDS[1].color);
        assert_eq!(speed_color(Some(81.0)), SPEED_BANDS[2].color);
        assert_eq!(speed_color(Some(320.0)), SPEED_BANDS[SPEED_BANDS.len() - 1].color);
        assert_eq!(speed_color(None), SPEED_UNKNOWN);
        assert_eq!(rail_speed_color(&props(json!({ "feature": "tram", "maxspeed": 50 }))), RAIL_TRANSIT);
        assert_eq!(rail_speed_color(&props(json!({ "feature": "rail", "state": "razed", "maxspeed": 50 })))[3], 0);
        assert!(rail_speed_width(&props(json!({ "feature": "rail", "service": "siding" }))) < rail_speed_width(&props(json!({ "feature": "rail" }))));
    }

    #[test]
    fn one_label_per_stretch_a_double_track_says_its_limit_once_sidings_not_at_all() {
        let labels = speed_labels(
            &[
                line(&[[0.0, 0.5], [1.0, 0.5]], json!({ "maxspeed": 160 })),
                line(&[[0.0, 0.505], [1.0, 0.505]], json!({ "maxspeed": 160 })),
                line(&[[0.0, 0.8], [1.0, 0.8]], json!({ "maxspeed": 80 })),
                line(&[[0.0, 0.2], [1.0, 0.2]], json!({ "maxspeed": 40, "service": "siding" })),
                line(&[[0.0, 0.9], [0.01, 0.9]], json!({ "maxspeed": 120 })),
            ],
            1.0,
            SPEED_LABEL_MIN_LENGTH,
            SPEED_LABEL_SPACING,
        );
        let mut texts: Vec<&str> = labels.iter().map(|l| l.text.as_str()).collect();
        texts.sort_unstable();
        assert_eq!(texts, ["160", "80"]);
        assert_eq!(labels.iter().find(|l| l.text == "160").expect("a 160 label").position, [0.5, 0.5]);
    }

    #[test]
    fn the_tiles_come_through_the_apps_protocol() {
        assert_eq!(rail_tiles_url("Mozilla/5.0 (Macintosh)"), "railtiles://localhost/{z}/{x}/{y}");
        assert_eq!(rail_tiles_url("Mozilla/5.0 (Linux; Android 14)"), "http://railtiles.localhost/{z}/{x}/{y}");
    }

    // --- new tests: what the TypeScript's tests leave implicit -------------------

    #[test]
    fn the_overview_is_drawn_below_zoom_8_once_it_is_in_and_the_speed_map_takes_the_networks_place() {
        let base = RailOptions { overview: true, ..RailOptions::new(Some("u"), false, 6.0) };
        let ids = |o: RailOptions| rail_layers(&o).iter().map(RailLayer::id).collect::<Vec<_>>();
        assert_eq!(ids(base), ["rail-overview-globe"]);
        assert_eq!(ids(RailOptions { zoom: 7.99, flat: true, ..base }), ["rail-overview-flat"]);
        assert_eq!(ids(RailOptions { zoom: 8.0, ..base }), ["rail-globe"]);
        let speeds = RailOptions { speeds: true, ..base };
        assert_eq!(rail_layers(&RailOptions { zoom: 9.0, ..speeds }), [RailLayer::Speed { flat: false, labels: false }]);
        assert_eq!(
            rail_layers(&RailOptions { zoom: 12.5, flat: true, ..speeds }),
            [RailLayer::Speed { flat: true, labels: true }, RailLayer::Stations { flat: true }]
        );
        // the speed map needs no tile URL; an empty one counts as none
        assert_eq!(ids(RailOptions { zoom: 13.0, tile_url: None, ..speeds }), ["rail-speed-globe"]);
        assert!(ids(RailOptions { zoom: 13.0, tile_url: Some(""), ..base }).is_empty());
    }

    #[test]
    fn tunnel_alpha_and_the_width_scale_round_as_javascript_does() {
        // round(245 · 0.55) = round(134.75), round(200 · 0.55) = round(110.00000000000001)
        assert_eq!(tunnel_of(RAIL_MAIN), [240, 112, 96, 135]);
        assert_eq!(tunnel_of(RAIL_SERVICE), [168, 102, 94, 110]);
        assert_eq!(tunnel_of(RAIL_TRANSIT)[3], 124);
        assert_eq!(rail_width_scale(8.0), 0.8);
        assert_eq!(rail_width_scale(11.5), 1.3);
        assert_eq!(rail_width_scale(15.0), 1.8);
        assert!(rail_width_scale(f64::NAN).is_nan());
    }

    #[test]
    fn properties_read_as_javascript_reads_them() {
        // a falsy `service` is no service track; `subclass: null` is not a missing one
        assert_eq!(rail_kind(&t(json!({ "class": "rail", "service": "" }))), Some(RailKind::Main));
        assert_eq!(rail_kind(&t(json!({ "class": "rail", "service": 0 }))), Some(RailKind::Main));
        assert_eq!(rail_kind(&t(json!({ "class": "rail", "subclass": null }))), Some(RailKind::Minor));
        assert_eq!(rail_kind(&props(json!({ "class": "rail" }))), Some(RailKind::Main));
        // `String(p.subclass)`, `String(p.feature)`
        assert!(is_station(&props(json!({ "class": "railway", "subclass": ["halt"] }))));
        assert!(!is_station(&props(json!({ "class": "railway" }))));
        assert_eq!(speed_kind(&props(json!({ "feature": ["tram"] }))), Some(SpeedKind::Transit));
        assert_eq!(speed_kind(&props(json!({ "feature": "rail", "state": null }))), None);
        assert_eq!(speed_kind(&props(json!({ "feature": "narrow_gauge", "service": "yard" }))), Some(SpeedKind::Service));
        assert_eq!(speed_kind(&props(json!({ "feature": "funicular" }))), None);
        // a limit that isn't a number, or isn't positive, isn't mapped
        assert_eq!(rail_speed_color(&props(json!({ "feature": "rail", "maxspeed": "160" }))), SPEED_UNKNOWN);
        assert_eq!(rail_speed_color(&props(json!({ "feature": "rail", "maxspeed": 0 }))), SPEED_UNKNOWN);
        assert_eq!(rail_speed_color(&props(json!({ "feature": "rail", "maxspeed": 200, "tunnel": true }))), tunnel_of(SPEED_BANDS[4].color));
        assert_eq!(rail_speed_dash(&props(json!({ "feature": "rail", "tunnel": 1 }))), TUNNEL_DASH);
        assert_eq!(rail_speed_dash(&props(json!({ "feature": "rail", "tunnel": false }))), SOLID);
    }

    #[test]
    fn station_rings_by_kind() {
        let station = props(json!({ "layerName": "poi", "class": "railway", "subclass": "station" }));
        let halt = props(json!({ "class": "railway", "subclass": "halt" }));
        let stop = props(json!({ "class": "railway", "subclass": "tram_stop" }));
        assert_eq!((station_radius(&station), station_radius(&halt), station_radius(&stop)), (3.5, 2.5, 0.0));
        assert_eq!((station_fill(&station), station_ring(&halt)), (STATION_FILL, STATION_RING));
        assert_eq!((station_fill(&stop), station_ring(&stop)), (NONE, NONE));
    }

    #[test]
    fn labels_go_halfway_along_longest_first_and_round_their_limits() {
        let bent = LineFeature {
            geometry: Geometry {
                kind: "MultiLineString".into(),
                coordinates: Coordinates::Lines(vec![
                    vec![vec![0.0, 0.0], vec![0.3, 0.0], vec![0.3, 0.5]],
                    vec![vec![0.9, 0.0], vec![0.9, 0.1]],
                ]),
            },
            properties: props(json!({ "feature": "rail", "maxspeed": 99.5 })),
        };
        // (the expected values are the TypeScript's for the same input, to the bit)
        let labels = speed_labels(&[line(&[[0.0, 0.9], [0.2, 0.9]], json!({ "maxspeed": 70 })), bent], 1.0, 0.06, 0.18);
        let got: Vec<(&str, [f64; 2], Rgba)> = labels.iter().map(|l| (l.text.as_str(), l.position, l.color)).collect();
        assert_eq!(
            got,
            [
                // the bent line (0.8 long) first, halfway along: 0.1 up its second leg
                ("100", [0.3, 0.10000000000000003], SPEED_BANDS[2].color),
                ("70", [0.10000000000000002, 0.9], SPEED_BANDS[1].color),
                ("100", [0.9, 0.05000000000000001], SPEED_BANDS[2].color),
            ]
        );
        // `size` scales the shortest line that carries a label
        let short = [line(&[[10.0, 50.0], [10.05, 50.0]], json!({ "maxspeed": 120 }))];
        assert!(speed_labels(&short, 1.0, 0.06, 0.18).is_empty());
        let degrees = speed_labels(&short, 0.5, 0.06, 0.18);
        assert_eq!(degrees, [SpeedLabel { position: [10.025, 50.0], text: "120".into(), color: SPEED_BANDS[2].color }]);
    }
}
