//! Place names and airport codes on the map (item B4; doc 01 §5.9; G:610-752, 2743-2754): the CPU
//! side of today's `label-tiles` and `labels` layers. `rustradar_map::layers::labels` fetches the
//! tiles, lays the text out in the glyph atlas (`crate::text`) and draws it.
//!
//! What today's app does, and so this module:
//! - **Tiles to labels** ([`decode`]). OpenFreeMap's tiles, decoded as loaders.gl's MVTLoader does
//!   with `shape: "geojson"`, `coordinates: "wgs84"` and `layers: ["place", "aerodrome_label"]`:
//!   the `place` layer's features, then the `aerodrome_label` layer's, each in feature order (for
//!   each name the last non-empty layer of that name, `rustradar_core::mvt::layers_named`), every
//!   feature's properties as JavaScript has them ([`Prop`]) with `layerName` set to its layer.
//!   [`is_label`] keeps a feature whose geometry is one point (`"Point"`; several points make a
//!   `"MultiPoint"`) that is an airport with an `iata`, or a place of class `country`, `city`,
//!   `town`, `village` or `suburb`. Its position is loaders.gl's `projectToLngLat`.
//! - **Text, size, colour, priority** ([`label_text`], [`label_size`], [`label_color`],
//!   [`label_priority`]): the table of doc 01 §5.9 with JavaScript's coercions (`Number(p.rank ??
//!   10)`, `name_en || name:latin || name || ""`, `String(p.iata)`, a country upper-cased).
//! - **Horizon filter** ([`InView`]): at zoom ≤ 6 only labels within `cosAngle > 0.42` of the view
//!   centre, recomputed when the rounded centre or the number of features changes, and whenever
//!   new tiles arrive (`labelsInView`, `onLabelTiles`).
//! - **Marked airports** ([`without_marked`]): an airport whose code has a marker of its own (the
//!   focused airport, the selected flight's origin and destination) loses its label.
//! - **Collision** ([`collision_fades`]): highest priority first, a label is shown when its text
//!   box, grown by [`LABEL_MARGIN`], meets no box shown before it (`crate::declutter::Group`, which
//!   also keeps a label already shown ahead of an equal one, so labels hold still as the camera
//!   moves). Shown labels never overlap, none is cut by the canvas's edge, and a copy of a place
//!   from a neighbouring tile is hidden by the first.

use rustradar_core::geo::cos_angle;
use rustradar_core::mvt::{self, GeomType, Value};
use rustradar_core::num::{js_round, js_string};

use crate::declutter::{DRAWN_MIN_FADE, Group, Label, Rect};
use crate::text::LaidText;
use crate::tiles::TileIndex;

/// The source layers decoded, in this order (`layers: ["place", "aerodrome_label"]`, G:670).
pub const SOURCE_LAYERS: [&str; 2] = [PLACE, AERODROME_LABEL];
/// OpenMapTiles' layer of place names.
pub const PLACE: &str = "place";
/// OpenMapTiles' layer of airport labels.
pub const AERODROME_LABEL: &str = "aerodrome_label";
/// The place classes labelled (`PLACE_CLASSES`, G:615).
pub const PLACE_CLASSES: [&str; 5] = ["country", "city", "town", "village", "suburb"];

/// Above this zoom every label passes the horizon filter: the horizon is off the screen
/// (`labelsInView`, G:696).
pub const HORIZON_MAX_ZOOM: f64 = 6.0;
/// At zoom ≤ 6 a label is kept when the cosine of its angle from the view centre is above this
/// (within about 65.2°; G:702).
pub const HORIZON_MIN_COS: f64 = 0.42;

/// An airport's code (`labelColor`, G:647): the selection blue.
pub const AIRPORT_COLOR: [u8; 4] = [55, 142, 240, 255];
/// A country's name.
pub const COUNTRY_COLOR: [u8; 4] = [162, 172, 183, 255];
/// A city's name.
pub const CITY_COLOR: [u8; 4] = [241, 238, 231, 245];
/// A town's, village's or suburb's name.
pub const PLACE_COLOR: [u8; 4] = [179, 191, 204, 235];

/// Room kept clear round a label's text, CSS pixels.
pub const LABEL_MARGIN: f64 = 3.0;

// ---------------------------------------------------------------------------------------------
// Properties as JavaScript has them

/// A feature property as loaders.gl's GeoJSON hands it to the app: a string, a number or a
/// boolean (an absent property is `None`, JavaScript's `undefined`).
#[derive(Clone, Debug, PartialEq)]
pub enum Prop {
    Str(String),
    Num(f64),
    Bool(bool),
}

impl Prop {
    /// A tile's tag value.
    pub fn from_mvt(value: &Value<'_>) -> Prop {
        match *value {
            Value::Str(s) => Prop::Str(s.to_owned()),
            Value::Num(n) => Prop::Num(n),
            Value::Bool(b) => Prop::Bool(b),
        }
    }

    /// `Boolean(x)`: a non-empty string, a number other than 0 and NaN, `true`.
    pub fn truthy(&self) -> bool {
        match self {
            Prop::Str(s) => !s.is_empty(),
            Prop::Num(n) => *n != 0.0 && !n.is_nan(),
            Prop::Bool(b) => *b,
        }
    }

    /// `String(x)`.
    pub fn js_string(&self) -> String {
        match self {
            Prop::Str(s) => s.clone(),
            Prop::Num(n) => js_string(*n),
            Prop::Bool(b) => b.to_string(),
        }
    }

    /// `Number(x)`.
    pub fn js_number(&self) -> f64 {
        match self {
            Prop::Str(s) => string_to_number(s),
            Prop::Num(n) => *n,
            Prop::Bool(b) => f64::from(u8::from(*b)),
        }
    }

    /// `x === s` for a string `s`.
    pub fn is(&self, s: &str) -> bool {
        matches!(self, Prop::Str(v) if v == s)
    }
}

/// JavaScript's white space and line terminators, which `Number()` trims.
fn js_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

/// `Number(s)` for a string: white space trimmed, empty is 0, `Infinity` with a sign, `0x`, `0o`
/// and `0b` integers, else a decimal literal or NaN.
pub fn string_to_number(s: &str) -> f64 {
    let t = s.trim_matches(js_space);
    if t.is_empty() {
        return 0.0;
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let radix = match t.get(..2) {
        Some("0x" | "0X") => Some(16),
        Some("0o" | "0O") => Some(8),
        Some("0b" | "0B") => Some(2),
        _ => None,
    };
    if let Some(radix) = radix {
        let digits = &t[2..];
        if digits.is_empty() {
            return f64::NAN;
        }
        return digits.chars().try_fold(0.0, |n, c| c.to_digit(radix).map(|d| n * f64::from(radix) + f64::from(d))).unwrap_or(f64::NAN);
    }
    // a decimal literal: Rust's parser also takes "inf" and "nan", JavaScript does not
    if t.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-')) {
        t.parse().unwrap_or(f64::NAN)
    } else {
        f64::NAN
    }
}

/// The properties the label rules read (`LabelFeature["properties"]`): `layerName` as loaders.gl
/// sets it, the rest as the tile has them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LabelProps {
    /// [`PLACE`] or [`AERODROME_LABEL`].
    pub layer_name: String,
    pub class: Option<Prop>,
    pub name_en: Option<Prop>,
    /// `name:latin`.
    pub name_latin: Option<Prop>,
    pub name: Option<Prop>,
    pub iata: Option<Prop>,
    pub rank: Option<Prop>,
}

impl LabelProps {
    /// A feature of layer `layer_name`.
    pub fn of(layer_name: &str, feature: &mvt::Feature<'_>) -> LabelProps {
        let get = |key: &str| feature.get(key).map(Prop::from_mvt);
        LabelProps {
            layer_name: layer_name.to_owned(),
            class: get("class"),
            name_en: get("name_en"),
            name_latin: get("name:latin"),
            name: get("name"),
            iata: get("iata"),
            rank: get("rank"),
        }
    }

    fn is_airport(&self) -> bool {
        self.layer_name == AERODROME_LABEL
    }

    fn class_is(&self, class: &str) -> bool {
        self.class.as_ref().is_some_and(|c| c.is(class))
    }
}

/// `isLabel` (G:616-620): a single point that is an airport with an `iata`, or a place of one of
/// [`PLACE_CLASSES`].
pub fn is_label(single_point: bool, p: &LabelProps) -> bool {
    single_point
        && if p.is_airport() {
            p.iata.as_ref().is_some_and(Prop::truthy)
        } else {
            // String(undefined) is "undefined"
            let class = p.class.as_ref().map_or_else(|| "undefined".to_owned(), Prop::js_string);
            PLACE_CLASSES.contains(&class.as_str())
        }
}

/// `nameOf` (G:621): `String(p.name_en || p["name:latin"] || p.name || "")`.
pub fn name_of(p: &LabelProps) -> String {
    [&p.name_en, &p.name_latin, &p.name].into_iter().flatten().find(|v| v.truthy()).map_or_else(String::new, Prop::js_string)
}

/// `rank` (G:622): `Number(p.rank ?? 10)`.
pub fn rank(p: &LabelProps) -> f64 {
    p.rank.as_ref().map_or(10.0, Prop::js_number)
}

/// `labelText` (G:624-628): an airport's code, a country's name upper-cased, any other name.
pub fn label_text(p: &LabelProps) -> String {
    if p.is_airport() {
        return p.iata.as_ref().map_or_else(|| "undefined".to_owned(), Prop::js_string);
    }
    if p.class_is("country") { name_of(p).to_uppercase() } else { name_of(p) }
}

/// `labelSize` (G:630-643), CSS pixels before the label size setting.
pub fn label_size(p: &LabelProps) -> f64 {
    if p.is_airport() {
        return 12.0;
    }
    match p.class.as_ref() {
        Some(c) if c.is("country") => 12.0,
        // NaN <= 3 is false
        Some(c) if c.is("city") => {
            if rank(p) <= 3.0 {
                14.0
            } else {
                13.0
            }
        }
        Some(c) if c.is("town") => 12.0,
        _ => 11.0,
    }
}

/// `labelColor` (G:645-651).
pub fn label_color(p: &LabelProps) -> [u8; 4] {
    if p.is_airport() {
        AIRPORT_COLOR
    } else if p.class_is("country") {
        COUNTRY_COLOR
    } else if p.class_is("city") {
        CITY_COLOR
    } else {
        PLACE_COLOR
    }
}

/// `labelPriority` (G:653-658): higher wins. Airports 600; places `base − 10 · rank` with base 900
/// for a country, 800 for a city, 400 for a town, 100 for anything else.
pub fn label_priority(p: &LabelProps) -> f64 {
    if p.is_airport() {
        return 600.0;
    }
    let base = if p.class_is("country") {
        900.0
    } else if p.class_is("city") {
        800.0
    } else if p.class_is("town") {
        400.0
    } else {
        100.0
    };
    base - rank(p) * 10.0
}

// ---------------------------------------------------------------------------------------------
// Tiles to labels

/// One label of the map: where it is and how it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct MapLabel {
    /// `geometry.coordinates`, degrees.
    pub lon: f64,
    pub lat: f64,
    /// An airport's code (`String(p.iata)`), clicked to focus the airport; `None` for a place.
    pub iata: Option<String>,
    /// [`label_text`].
    pub text: String,
    /// The feature's name ([`name_of`]): an airport's tooltip says it (G:3419, item T8).
    pub name: String,
    /// [`label_size`], CSS pixels.
    pub size: f64,
    /// [`label_color`].
    pub color: [u8; 4],
    /// [`label_priority`].
    pub priority: f64,
}

impl MapLabel {
    /// The label of a feature at `[lon, lat]` with properties `p`.
    pub fn new([lon, lat]: [f64; 2], p: &LabelProps) -> MapLabel {
        MapLabel {
            lon,
            lat,
            iata: p.is_airport().then(|| p.iata.as_ref().map_or_else(|| "undefined".to_owned(), Prop::js_string)),
            text: label_text(p),
            name: name_of(p),
            size: label_size(p),
            color: label_color(p),
            priority: label_priority(p),
        }
    }
}

impl MapLabel {
    /// The same for every copy of this label, whichever tile it came from.
    pub fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&self.text, self.lon.to_bits(), self.lat.to_bits()).hash(&mut h);
        h.finish()
    }
}

/// loaders.gl's `projectToLngLat`: a point in tile units (`extent` across) of `tile` to longitude
/// and latitude.
pub fn tile_point_to_lng_lat(tile: TileIndex, extent: f64, [px, py]: [f64; 2]) -> [f64; 2] {
    let size = extent * 2f64.powi(tile.z as i32);
    let x0 = extent * f64::from(tile.x);
    let y0 = extent * f64::from(tile.y);
    let lon = ((px + x0) * 360.0) / size - 180.0;
    let y2 = 180.0 - ((py + y0) * 360.0) / size;
    let lat = (360.0 / std::f64::consts::PI) * ((y2 * std::f64::consts::PI) / 180.0).exp().atan() - 90.0;
    [lon, lat]
}

/// `getLabelTile` (G:666-675): a tile's labels, `place` first then `aerodrome_label`, each in
/// feature order, filtered by [`is_label`]. A tile that does not parse gives what could be read.
pub fn decode(tile: TileIndex, bytes: &[u8]) -> Vec<MapLabel> {
    let mut labels = Vec::new();
    for layer in mvt::layers_named(bytes, &SOURCE_LAYERS) {
        for feature in layer.features() {
            let props = LabelProps::of(layer.name, &feature);
            let mut points = feature.points();
            let first = points.next();
            let single = feature.kind == GeomType::Point && first.is_some() && points.next().is_none();
            if !is_label(single, &props) {
                continue;
            }
            let Some((x, y)) = first else { continue };
            labels.push(MapLabel::new(tile_point_to_lng_lat(tile, layer.extent, [x, y]), &props));
        }
    }
    labels
}

// ---------------------------------------------------------------------------------------------
// Which labels

/// `labelsInView` (G:694-707) with its cache: the labels facing the camera, as indices into the
/// layer's features.
#[derive(Clone, Debug, Default)]
pub struct InView {
    /// `${round(cLon)},${round(cLat)},${count}` of the cached filter; `None` as `""`.
    key: Option<[f64; 3]>,
    data: Vec<usize>,
}

impl InView {
    /// Forgets the cached filter (`visibleLabels = { key: "", data: [] }`): new tiles arrived.
    pub fn reset(&mut self) {
        self.key = None;
        self.data.clear();
    }

    /// The labels of `features` facing a camera centred on `[c_lon, c_lat]` at `zoom`: all of them
    /// above zoom 6, else those within [`HORIZON_MIN_COS`] of the centre, filtered again only when
    /// the rounded centre or the number of features changed since the last filter.
    pub fn labels(&mut self, features: &[MapLabel], c_lon: f64, c_lat: f64, zoom: f64) -> Vec<usize> {
        if zoom > HORIZON_MAX_ZOOM {
            return (0..features.len()).collect();
        }
        let key = [js_round(c_lon), js_round(c_lat), features.len() as f64];
        if self.key != Some(key) {
            self.data = (0..features.len()).filter(|&i| cos_angle(features[i].lon, features[i].lat, c_lon, c_lat) > HORIZON_MIN_COS).collect();
            self.key = Some(key);
        }
        self.data.clone()
    }
}

/// `withoutMarked` (G:709-720): `data` without the airports whose code is one of `codes` (they
/// have markers of their own).
pub fn without_marked(data: Vec<usize>, features: &[MapLabel], codes: &[String]) -> Vec<usize> {
    if codes.is_empty() {
        return data;
    }
    data.into_iter().filter(|&i| !features[i].iata.as_ref().is_some_and(|code| codes.contains(code))).collect()
}

// ---------------------------------------------------------------------------------------------
// Collision

/// The glyph quads of `text` drawn `size_px` tall (`getSize · sizeScale`) at `anchor`: CSS pixels
/// from the canvas's top left, y down, `[x0, y0, x1, y1]`. Each is the glyph's whole atlas frame
/// around its offset from the anchor (MultiIconLayer's quad), scaled by `perspective`: a billboard's
/// pixel offsets are exact at the view centre's depth and shrink with distance (the focal depth's
/// clip w over the anchor's).
pub fn glyph_quads(text: &LaidText, anchor: [f64; 2], size_px: f64, perspective: f64) -> impl Iterator<Item = [f64; 4]> + '_ {
    let k = size_px / text.font_size * perspective;
    text.glyphs.iter().map(move |g| {
        let [_, _, w, h] = g.frame;
        let [ox, oy] = g.offset;
        [anchor[0] + (ox - w / 2.0) * k, anchor[1] + (oy - h / 2.0) * k, anchor[0] + (ox + w / 2.0) * k, anchor[1] + (oy + h / 2.0) * k]
    })
}

/// The smallest rectangle round `quads` (`None` for none): a label's pick rectangle.
pub fn bounds(quads: impl IntoIterator<Item = [f64; 4]>) -> Option<Rect> {
    quads.into_iter().fold(None, |acc: Option<Rect>, [x0, y0, x1, y1]| {
        let r = Rect::new(x0, y0, x1, y1);
        Some(acc.map_or(r, |a| a.union(&r)))
    })
}

/// One label of the layer's data for [`collision_fades`].
#[derive(Clone, Copy, Debug)]
pub struct Placing<'a> {
    /// Stays the same while the same place is labelled ([`MapLabel::key`]).
    pub key: u64,
    /// Where its anchor is drawn: CSS pixels from the canvas's top left; `None` when it is not in
    /// front of the camera (nothing of it is drawn).
    pub anchor: Option<[f64; 2]>,
    /// The focal depth's clip w over the anchor's ([`glyph_quads`]).
    pub perspective: f64,
    /// [`MapLabel::priority`].
    pub priority: f64,
    /// The size it is drawn at, CSS pixels.
    pub size: f64,
    pub text: &'a LaidText,
}

/// Which labels are shown this frame (1) and which are hidden (0): see the module's docs.
pub fn collision_fades(group: &mut Group<u64>, canvas: [f64; 2], labels: &[Placing<'_>]) -> Vec<f64> {
    let inside = |r: &Rect| r.x0 >= 0.0 && r.y0 >= 0.0 && r.x1 <= canvas[0] && r.y1 <= canvas[1];
    let offered: Vec<Label<u64>> = labels
        .iter()
        .map(|l| {
            let claim = l
                .anchor
                .and_then(|a| bounds(glyph_quads(l.text, a, l.size, l.perspective)))
                .filter(inside)
                .map_or(Rect::new(f64::NAN, f64::NAN, f64::NAN, f64::NAN), |r| Rect::new(r.x0 - LABEL_MARGIN, r.y0 - LABEL_MARGIN, r.x1 + LABEL_MARGIN, r.y1 + LABEL_MARGIN));
            Label { id: l.key, priority: l.priority, claim }
        })
        .collect();
    let placed = group.place(&offered, 0.0);
    (0..labels.len()).map(|i| if placed.kept(i) { 1.0 } else { 0.0 }).collect()
}

/// Whether a label of `fade` is drawn (and pickable): deck throws a vertex under a fade of 0.0001
/// out of clip space.
pub fn drawn(fade: f64) -> bool {
    fade >= DRAWN_MIN_FADE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::LaidGlyph;

    fn s(v: &str) -> Option<Prop> {
        Some(Prop::Str(v.to_owned()))
    }

    fn n(v: f64) -> Option<Prop> {
        Some(Prop::Num(v))
    }

    fn place(class: &str, name: &str, rank: Option<Prop>) -> LabelProps {
        LabelProps { layer_name: PLACE.into(), class: s(class), name: s(name), rank, ..Default::default() }
    }

    fn airport(iata: Option<Prop>) -> LabelProps {
        LabelProps { layer_name: AERODROME_LABEL.into(), class: s("international"), name: s("Arlanda"), iata, ..Default::default() }
    }

    /// The collision pass's loops (P1: each row's span as slices, the patch's columns once, the
    /// texel bounds' `ceil`) make the very map and fades of the plain per-texel loops they
    /// replaced, here kept as written for B4.
    #[test]
    fn the_priority_formula() {
        assert_eq!(label_priority(&airport(s("ARN"))), 600.0);
        // whatever the rank says, an airport is 600
        assert_eq!(label_priority(&LabelProps { rank: n(1.0), ..airport(s("ARN")) }), 600.0);
        assert_eq!(label_priority(&place("country", "Sweden", n(1.0))), 890.0);
        assert_eq!(label_priority(&place("country", "Sweden", n(3.0))), 870.0);
        assert_eq!(label_priority(&place("city", "Stockholm", n(2.0))), 780.0);
        assert_eq!(label_priority(&place("city", "Uppsala", n(7.0))), 730.0);
        assert_eq!(label_priority(&place("town", "Märsta", n(10.0))), 300.0);
        assert_eq!(label_priority(&place("village", "Vega", n(12.0))), -20.0);
        assert_eq!(label_priority(&place("suburb", "Solna", n(16.0))), -60.0);
        // no rank reads as 10 (`p.rank ?? 10`)
        assert_eq!(label_priority(&place("city", "Somewhere", None)), 700.0);
        assert_eq!(label_priority(&place("town", "Somewhere", None)), 300.0);
        assert_eq!(label_priority(&place("village", "Somewhere", None)), 0.0);
        // Number() of a string rank, of a boolean, and of what is not a number
        assert_eq!(label_priority(&place("city", "A", s(" 4 "))), 760.0);
        assert_eq!(label_priority(&place("city", "A", s(""))), 800.0);
        assert_eq!(label_priority(&place("city", "A", Some(Prop::Bool(true)))), 790.0);
        assert!(label_priority(&place("city", "A", s("first"))).is_nan());
        // a class this table does not know counts as a village's base
        assert_eq!(label_priority(&place("hamlet", "A", n(2.0))), 80.0);
        // higher wins: a country over a big city over an airport over a town over a village
        let order = [
            place("country", "Sweden", n(1.0)),
            place("city", "Stockholm", n(1.0)),
            airport(s("ARN")),
            place("town", "Märsta", n(6.0)),
            place("village", "Vega", n(8.0)),
        ];
        assert!(order.windows(2).all(|w| label_priority(&w[0]) > label_priority(&w[1])));
    }

    /// An airport's label keeps its feature's name for its tooltip (item T8): `nameOf`, the
    /// English name first.
    #[test]
    fn a_label_keeps_its_features_name() {
        let label = MapLabel::new([17.92, 59.65], &airport(s("ARN")));
        assert_eq!((label.text.as_str(), label.name.as_str(), label.iata.as_deref()), ("ARN", "Arlanda", Some("ARN")));
        let english = LabelProps { name_en: s("Stockholm Arlanda Airport"), ..airport(s("ARN")) };
        assert_eq!(MapLabel::new([17.92, 59.65], &english).name, "Stockholm Arlanda Airport");
        assert_eq!(MapLabel::new([18.07, 59.33], &place("city", "Stockholm", n(1.0))).name, "Stockholm");
    }

    #[test]
    fn text_size_and_colour_follow_the_table() {
        let cases = [
            (airport(s("ARN")), "ARN", 12.0, AIRPORT_COLOR),
            (place("country", "Sweden", n(1.0)), "SWEDEN", 12.0, COUNTRY_COLOR),
            (place("city", "Stockholm", n(1.0)), "Stockholm", 14.0, CITY_COLOR),
            (place("city", "Uppsala", n(3.0)), "Uppsala", 14.0, CITY_COLOR),
            (place("city", "Västerås", n(4.0)), "Västerås", 13.0, CITY_COLOR),
            // no rank is 10: a small city
            (place("city", "Somewhere", None), "Somewhere", 13.0, CITY_COLOR),
            (place("town", "Märsta", n(10.0)), "Märsta", 12.0, PLACE_COLOR),
            (place("village", "Vega", n(12.0)), "Vega", 11.0, PLACE_COLOR),
            (place("suburb", "Solna", n(16.0)), "Solna", 11.0, PLACE_COLOR),
        ];
        for (props, text, size, color) in cases {
            assert_eq!(label_text(&props), text, "{props:?}");
            assert_eq!(label_size(&props), size, "{props:?}");
            assert_eq!(label_color(&props), color, "{props:?}");
        }
        // the name: name_en, then name:latin, then name, the first that is truthy
        let mut p = place("city", "Москва", n(1.0));
        assert_eq!(name_of(&p), "Москва");
        p.name_latin = s("Moskva");
        assert_eq!(name_of(&p), "Moskva");
        p.name_en = s("");
        assert_eq!(name_of(&p), "Moskva", "an empty name_en is passed over");
        p.name_en = s("Moscow");
        assert_eq!(label_text(&p), "Moscow");
        let nameless = LabelProps { name: None, ..place("town", "", None) };
        assert_eq!(label_text(&nameless), "");
        // a number as a name is written as JavaScript writes it
        assert_eq!(label_text(&LabelProps { name: n(1066.0), ..place("village", "", None) }), "1066");
        // a country upper-cased, ß and all
        assert_eq!(label_text(&place("country", "Deutschland ß", n(1.0))), "DEUTSCHLAND SS");
        assert_eq!(label_text(&airport(n(7.0))), "7");
        // NaN <= 3 is false
        assert_eq!(label_size(&place("city", "A", s("x"))), 13.0);
    }

    #[test]
    fn only_points_of_airports_with_codes_and_of_five_place_classes_are_labels() {
        assert!(is_label(true, &airport(s("ARN"))));
        assert!(!is_label(true, &airport(None)));
        assert!(!is_label(true, &airport(s(""))));
        assert!(!is_label(true, &airport(n(0.0))));
        assert!(is_label(true, &airport(n(7.0))));
        for class in PLACE_CLASSES {
            assert!(is_label(true, &place(class, "A", None)), "{class}");
            assert!(!is_label(false, &place(class, "A", None)), "{class} as a multipoint");
        }
        for class in ["state", "province", "hamlet", "isolated_dwelling", "neighbourhood", "island", "continent"] {
            assert!(!is_label(true, &place(class, "A", None)), "{class}");
        }
        assert!(!is_label(true, &LabelProps { class: None, ..place("city", "A", None) }));
        // "class" compared as String(class): a number never matches
        assert!(!is_label(true, &LabelProps { class: n(1.0), ..place("city", "A", None) }));
        // an airport needs its code, whatever its class
        assert!(!is_label(true, &LabelProps { class: s("city"), ..airport(None) }));
    }

    #[test]
    fn number_reads_strings_as_javascript_does() {
        for (text, want) in [("12", 12.0), (" 12\n", 12.0), ("", 0.0), ("  ", 0.0), ("1e3", 1000.0), (".5", 0.5), ("-3", -3.0), ("0x1A", 26.0), ("0b101", 5.0), ("0o17", 15.0), ("Infinity", f64::INFINITY), ("-Infinity", f64::NEG_INFINITY)] {
            assert_eq!(string_to_number(text), want, "{text:?}");
        }
        for text in ["inf", "nan", "NaN", "12px", "0x", "1,5", "--1", "0xG"] {
            assert!(string_to_number(text).is_nan(), "{text:?}");
        }
        assert_eq!(Prop::Bool(false).js_number(), 0.0);
        assert_eq!(Prop::Num(2.5).js_string(), "2.5");
        assert_eq!(Prop::Bool(true).js_string(), "true");
        assert!(!Prop::Num(f64::NAN).truthy());
    }

    // ---------------------------------------------------------- a small tile encoder

    fn varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push((v as u8) | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    fn bytes_field(out: &mut Vec<u8>, field: u64, b: &[u8]) {
        varint(out, (field << 3) | 2);
        varint(out, b.len() as u64);
        out.extend_from_slice(b);
    }

    fn uint_field(out: &mut Vec<u8>, field: u64, v: u64) {
        varint(out, field << 3);
        varint(out, v);
    }

    fn packed(out: &mut Vec<u8>, field: u64, values: &[u64]) {
        let mut body = Vec::new();
        for &v in values {
            varint(&mut body, v);
        }
        bytes_field(out, field, &body);
    }

    const fn zigzag(v: i64) -> u64 {
        ((v << 1) ^ (v >> 63)) as u64
    }

    #[derive(Clone, Copy)]
    enum V {
        S(&'static str),
        U(u64),
    }

    /// A feature: geometry type, tags, points (one MoveTo per point, as a multipoint is written
    /// as one MoveTo with a count).
    struct F {
        kind: u64,
        tags: Vec<(&'static str, V)>,
        points: Vec<(i64, i64)>,
    }

    fn point(tags: &[(&'static str, V)], at: (i64, i64)) -> F {
        F { kind: 1, tags: tags.to_vec(), points: vec![at] }
    }

    /// A tile of `layers` (name, features), extent 4096.
    fn encode(layers: &[(&str, Vec<F>)]) -> Vec<u8> {
        let mut tile = Vec::new();
        for (name, features) in layers {
            let mut keys: Vec<&str> = Vec::new();
            let mut values: Vec<Vec<u8>> = Vec::new();
            let mut layer = Vec::new();
            uint_field(&mut layer, 15, 2);
            bytes_field(&mut layer, 1, name.as_bytes());
            for f in features {
                let mut tags = Vec::new();
                for &(k, v) in &f.tags {
                    let ki = keys.iter().position(|&x| x == k).unwrap_or_else(|| {
                        keys.push(k);
                        keys.len() - 1
                    });
                    let mut value = Vec::new();
                    match v {
                        V::S(s) => bytes_field(&mut value, 1, s.as_bytes()),
                        V::U(n) => uint_field(&mut value, 5, n),
                    }
                    values.push(value);
                    tags.extend([ki as u64, values.len() as u64 - 1]);
                }
                let mut geometry = vec![((f.points.len() as u64) << 3) | 1];
                let (mut cx, mut cy) = (0, 0);
                for &(x, y) in &f.points {
                    geometry.extend([zigzag(x - cx), zigzag(y - cy)]);
                    (cx, cy) = (x, y);
                }
                let mut feature = Vec::new();
                packed(&mut feature, 2, &tags);
                uint_field(&mut feature, 3, f.kind);
                packed(&mut feature, 4, &geometry);
                bytes_field(&mut layer, 2, &feature);
            }
            for k in &keys {
                bytes_field(&mut layer, 3, k.as_bytes());
            }
            for v in &values {
                bytes_field(&mut layer, 4, v);
            }
            uint_field(&mut layer, 5, 4096);
            bytes_field(&mut tile, 3, &layer);
        }
        tile
    }

    #[test]
    fn a_tile_gives_its_places_then_its_airports_where_loaders_gl_puts_them() {
        let tile = TileIndex { x: 281, y: 150, z: 9 };
        // the tile stores its airports first; loaders.gl walks `layers` in the order asked
        let bytes = encode(&[
            ("aerodrome_label", vec![point(&[("iata", V::S("ARN")), ("class", V::S("international"))], (100, 200)), point(&[("class", V::S("private"))], (300, 300))]),
            ("water", vec![point(&[("class", V::S("lake"))], (1, 1))]),
            (
                "place",
                vec![
                    point(&[("class", V::S("city")), ("name", V::S("Stockholm")), ("rank", V::U(1))], (2048, 2048)),
                    point(&[("class", V::S("state")), ("name", V::S("Stockholms län"))], (10, 10)),
                    F { kind: 1, tags: vec![("class", V::S("town")), ("name", V::S("Twice"))], points: vec![(5, 5), (6, 6)] },
                    point(&[("class", V::S("country")), ("name", V::S("Sverige")), ("name_en", V::S("Sweden")), ("rank", V::U(1))], (0, 4096)),
                ],
            ),
        ]);
        let labels = decode(tile, &bytes);
        let texts: Vec<&str> = labels.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["Stockholm", "SWEDEN", "ARN"]);
        assert_eq!(labels[2].iata.as_deref(), Some("ARN"));
        assert_eq!(labels[0].iata, None);
        assert_eq!((labels[0].size, labels[0].color, labels[0].priority), (14.0, CITY_COLOR, 790.0));
        // the tile's middle and its south-west corner, by loaders.gl's formula
        let middle = tile_point_to_lng_lat(tile, 4096.0, [2048.0, 2048.0]);
        assert_eq!([labels[0].lon, labels[0].lat], middle);
        let size = 4096.0 * 512.0;
        let lon = ((2048.0 + 4096.0 * 281.0) * 360.0) / size - 180.0;
        assert_eq!(middle[0], lon);
        assert!((middle[0] - 17.9296875).abs() < 1e-12 && (middle[1] - 59.355_596).abs() < 1e-5, "{middle:?}");
        let corner = crate::tiles::osm_tile_to_lng_lat(281.0, 151.0, 9);
        assert!((labels[1].lon - corner[0]).abs() < 1e-12 && (labels[1].lat - corner[1]).abs() < 1e-9, "{labels:?} {corner:?}");
        // nothing, or bytes that are not a tile: no labels
        assert!(decode(tile, &[]).is_empty());
        assert!(decode(tile, &[0xff, 0xff, 0xff]).is_empty());
    }

    fn at(lon: f64, lat: f64, iata: Option<&str>) -> MapLabel {
        MapLabel { lon, lat, iata: iata.map(str::to_owned), text: "x".into(), name: String::new(), size: 12.0, color: PLACE_COLOR, priority: 100.0 }
    }

    #[test]
    fn far_out_only_the_labels_facing_the_camera_and_the_filter_waits_for_the_centre_to_move_a_degree() {
        // the view centre at (10, 52): Berlin and Lisbon face it; Tokyo and Sydney do not
        let features = vec![at(13.4, 52.5, None), at(-9.1, 38.7, None), at(139.7, 35.7, None), at(151.2, -33.9, None)];
        let mut in_view = InView::default();
        assert_eq!(in_view.labels(&features, 10.0, 52.0, 4.5), [0, 1]);
        // above zoom 6 everything, cache or not
        assert_eq!(in_view.labels(&features, 10.0, 52.0, 6.1), [0, 1, 2, 3]);
        assert_eq!(in_view.labels(&features, 10.0, 52.0, 6.0), [0, 1]);
        // 0.42 is the edge: a label at 65.2° is out, at 65° in
        let edge = |deg: f64| vec![at(0.0, 90.0 - deg, None)];
        assert_eq!(InView::default().labels(&edge(65.0), 0.0, 90.0, 3.0), [0]);
        assert!(InView::default().labels(&edge(65.2), 0.0, 90.0, 3.0).is_empty());
        // the cache: the same rounded centre keeps the old filter, even where it is now wrong
        let mut in_view = InView::default();
        let far = vec![at(0.0, 0.0, None), at(65.3, 0.0, None)];
        assert_eq!(in_view.labels(&far, 0.4, 0.0, 3.0), [0, 1], "cos(64.9°) > 0.42");
        assert_eq!(in_view.labels(&far, -0.4, 0.0, 3.0), [0, 1], "round(-0.4) is round(0.4)");
        assert_eq!(in_view.labels(&far, -0.6, 0.0, 3.0), [0], "a degree further: filtered again");
        // new tiles reset it
        in_view.reset();
        assert_eq!(in_view.labels(&far, 0.4, 0.0, 3.0), [0, 1]);
        // and so does another number of features
        let more = vec![at(0.0, 0.0, None), at(65.3, 0.0, None), at(1.0, 1.0, None)];
        assert_eq!(in_view.labels(&more, -0.4, 0.0, 3.0), [0, 2]);
    }

    #[test]
    fn marked_airports_lose_their_labels() {
        let features = vec![at(18.0, 59.6, Some("ARN")), at(17.9, 59.4, Some("BMA")), at(18.0, 59.3, None)];
        assert_eq!(without_marked(vec![0, 1, 2], &features, &[]), [0, 1, 2]);
        assert_eq!(without_marked(vec![0, 1, 2], &features, &["ARN".into()]), [1, 2]);
        assert_eq!(without_marked(vec![0, 1, 2], &features, &["ARN".into(), "BMA".into(), "CPH".into()]), [2]);
        // a place is never an airport, whatever its text
        let mut place = at(0.0, 0.0, None);
        place.text = "ARN".into();
        assert_eq!(without_marked(vec![0], &[place], &["ARN".into()]), [0]);
    }

    /// A label of `width` × `height` CSS pixels at size scale 1 (one glyph frame of that size at
    /// 64 px per pixel, centred on the anchor).
    fn boxed(width: f64, height: f64) -> LaidText {
        LaidText { glyphs: vec![LaidGlyph { ch: 'x', frame: [0.0, 0.0, width * 64.0, height * 64.0], offset: [0.0, 0.0] }], rect: [0.0; 4], font_size: 64.0, generation: 0 }
    }

    fn placing(anchor: [f64; 2], priority: f64, text: &LaidText) -> Placing<'_> {
        Placing { key: (anchor[0] * 1000.0 + anchor[1]) as u64, anchor: Some(anchor), perspective: 1.0, priority, size: 1.0, text }
    }

    fn fades(labels: &[Placing<'_>]) -> Vec<f64> {
        collision_fades(&mut Group::labels(), [400.0, 300.0], labels)
    }

    #[test]
    fn shown_labels_never_overlap_and_the_more_important_wins() {
        let wide = boxed(80.0, 14.0);
        // 40 px apart their boxes overlap: the city stays, whichever comes first
        for flip in [false, true] {
            let mut labels = vec![placing([100.0, 100.0], 790.0, &wide), placing([140.0, 100.0], 0.0, &wide)];
            if flip {
                labels.reverse();
            }
            let f = fades(&labels);
            let (city, village) = if flip { (f[1], f[0]) } else { (f[0], f[1]) };
            assert_eq!((city, village), (1.0, 0.0), "flip {flip}");
            assert!(drawn(city) && !drawn(village));
        }
        // apart by their width and the margins: both shown
        let f = fades(&[placing([100.0, 100.0], 790.0, &wide), placing([100.0 + 80.0 + 2.0 * LABEL_MARGIN + 0.5, 100.0], 0.0, &wide)]);
        assert_eq!(f, [1.0, 1.0]);
        // a hidden label hides nothing: 300 is clear of 890, though 600 (hidden) covered it
        let f = fades(&[placing([100.0, 100.0], 890.0, &wide), placing([160.0, 100.0], 600.0, &wide), placing([200.0, 100.0], 300.0, &wide)]);
        assert_eq!(f, [1.0, 0.0, 1.0]);
    }

    #[test]
    fn copies_of_one_place_show_once_and_a_label_shown_keeps_its_place() {
        let wide = boxed(60.0, 10.0);
        let f = fades(&[placing([100.0, 100.0], 500.0, &wide), placing([100.0, 100.0], 500.0, &wide)]);
        assert_eq!(f, [1.0, 0.0]);
        // between equals, the one already shown stays when the other comes first
        let mut group = Group::labels();
        let b = placing([130.0, 100.0], 500.0, &wide);
        assert_eq!(collision_fades(&mut group, [400.0, 300.0], &[b]), [1.0]);
        let a = placing([100.0, 100.0], 500.0, &wide);
        assert_eq!(collision_fades(&mut group, [400.0, 300.0], &[a, b]), [0.0, 1.0]);
    }

    #[test]
    fn labels_behind_the_camera_cut_by_the_edge_or_with_a_nan_box_are_not_drawn() {
        let small = boxed(6.0, 6.0);
        let f = fades(&[Placing { anchor: None, ..placing([0.0; 2], 900.0, &small) }, placing([f64::NAN, 1.0], 1.0, &small), placing([100.0, 100.0], 0.0, &small)]);
        assert_eq!(f, [0.0, 0.0, 1.0]);
        // cut by the canvas's edge: not drawn
        let f = fades(&[placing([2.0, 100.0], 0.0, &small), placing([100.0, 298.0], 0.0, &small), placing([396.0, 100.0], 0.0, &small)]);
        assert_eq!(f, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn quads_and_bounds_scale_with_the_size_and_the_perspective() {
        let text = LaidText {
            glyphs: vec![
                LaidGlyph { ch: 'a', frame: [0.0, 0.0, 40.0, 80.0], offset: [-30.0, 2.0] },
                LaidGlyph { ch: 'b', frame: [0.0, 0.0, 40.0, 80.0], offset: [30.0, 2.0] },
            ],
            rect: [-50.0, -40.0, 100.0, 80.0],
            font_size: 64.0,
            generation: 0,
        };
        // 16 px tall: a quarter of the atlas's size
        let quads: Vec<[f64; 4]> = glyph_quads(&text, [100.0, 50.0], 16.0, 1.0).collect();
        assert_eq!(quads, [[87.5, 40.5, 97.5, 60.5], [102.5, 40.5, 112.5, 60.5]]);
        assert_eq!(bounds(quads), Some(Rect::new(87.5, 40.5, 112.5, 60.5)));
        // a label farther than the focal depth is drawn smaller
        let far: Vec<[f64; 4]> = glyph_quads(&text, [100.0, 50.0], 16.0, 0.5).collect();
        assert_eq!(far[0], [93.75, 45.25, 98.75, 55.25]);
        assert_eq!(bounds(std::iter::empty()), None);
    }
}
