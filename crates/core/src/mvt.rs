//! The one Mapbox Vector Tile decoder of the app (S1.21), extracted from the hand-written protobuf
//! reader of the backend's `rail.rs`: a tile's layers, and each layer's features with their
//! geometry type, their geometry in the tile's own units (`0..extent`, y down, the buffer round
//! the tile a little outside) and their typed tags.
//!
//! Nothing is generated from the spec's `.proto`: the format (vector tile spec 2.1) is small
//! enough to read by hand. Where this reads a tile differently from loaders.gl, which decoded the
//! TypeScript app's tiles, it says so where it does:
//!
//! - a feature without a property has none here; loaders.gl's binary tiles read a numeric one as
//!   0, and the TypeScript's rules were written against that: [`num_or_zero`] reads a property as
//!   they did (doc 04 §17.9);
//! - [`layers_named`] picks layers as loaders.gl's `layers` option did: in the order asked for,
//!   the last non-empty layer of each name;
//! - a feature's geometry ends at a command the spec doesn't have, or at a coordinate cut short
//!   (loaders.gl throws, and the whole tile is lost);
//! - rings are open (loaders.gl repeats a ring's first point at its `ClosePath`).

/// A line or a ring, in the tile's units.
pub type Line = Vec<(f64, f64)>;

/// The extent of a layer that doesn't say.
pub const EXTENT_DEFAULT: f64 = 4096.0;

// ------------------------------------------------------------------ protobuf

#[derive(Clone)]
struct Pb<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Pb<'a> {
    fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let byte = *self.b.get(self.i)?;
            self.i += 1;
            v |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return Some(v);
            }
            shift += 7;
            if shift > 63 {
                return None;
            }
        }
    }

    /// The next field: its number, and its value (a varint, or the bytes of
    /// a length-delimited or fixed-width one).
    fn field(&mut self) -> Option<(u64, Result<u64, &'a [u8]>)> {
        let key = self.varint()?;
        let value = match key & 7 {
            0 => Ok(self.varint()?),
            1 => {
                let b = self.b.get(self.i..self.i + 8)?;
                self.i += 8;
                Err(b)
            }
            2 => {
                let len = self.varint()? as usize;
                let b = self.b.get(self.i..self.i.checked_add(len)?)?;
                self.i += len;
                Err(b)
            }
            5 => {
                let b = self.b.get(self.i..self.i + 4)?;
                self.i += 4;
                Err(b)
            }
            _ => return None,
        };
        Some((key >> 3, value))
    }
}

/// A protobuf message's fields in order: each one's number, and its value (`Ok` a varint, `Err`
/// the bytes of a length-delimited or fixed-width one). Ends at the first it can't read.
pub fn fields(b: &[u8]) -> impl Iterator<Item = (u64, Result<u64, &[u8]>)> {
    let mut pb = Pb { b, i: 0 };
    std::iter::from_fn(move || pb.field())
}

/// The varints of a packed repeated field, up to the first it can't read.
pub fn packed(b: &[u8]) -> Vec<u64> {
    let mut pb = Pb { b, i: 0 };
    std::iter::from_fn(|| if pb.i < pb.b.len() { pb.varint() } else { None }).collect()
}

/// A zigzag-encoded varint's value.
const fn unzigzag(n: u64) -> i64 {
    ((n >> 1) as i64) ^ -((n & 1) as i64)
}

// ------------------------------------------------------------------ values

/// A tag's value: the spec's string; its float, double, int, uint and sint, all as a number (as
/// JavaScript has them); or its bool.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value<'a> {
    Str(&'a str),
    Num(f64),
    Bool(bool),
}

impl<'a> Value<'a> {
    /// A layer's `Value` message: its first field the spec knows; none if it has none (or a
    /// string that isn't UTF-8).
    fn parse(b: &'a [u8]) -> Option<Value<'a>> {
        fields(b).find_map(|(f, v)| match (f, v) {
            (1, Err(s)) => std::str::from_utf8(s).ok().map(Value::Str),
            (2, Err(b)) => Some(Value::Num(f64::from(f32::from_le_bytes(b.try_into().ok()?)))),
            (3, Err(b)) => Some(Value::Num(f64::from_le_bytes(b.try_into().ok()?))),
            (4, Ok(n)) => Some(Value::Num(n as i64 as f64)),
            (5, Ok(n)) => Some(Value::Num(n as f64)),
            (6, Ok(n)) => Some(Value::Num(unzigzag(n) as f64)),
            (7, Ok(n)) => Some(Value::Bool(n != 0)),
            _ => None,
        })
    }

    /// As JSON, as the TypeScript had a feature's properties: a whole number as an integer, a
    /// number that isn't finite as null (JSON has no such number).
    pub fn to_json(&self) -> serde_json::Value {
        match *self {
            Value::Str(s) => serde_json::Value::from(s),
            Value::Num(n) if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_992.0 => serde_json::Value::from(n as i64),
            Value::Num(n) => serde_json::Value::from(n),
            Value::Bool(b) => serde_json::Value::from(b),
        }
    }
}

/// `some()` of the TypeScript's rules: a finite number other than 0, else none. loaders.gl's
/// binary tiles read a numeric property a feature doesn't have as 0, so there absent and 0 were
/// one, and the rules took both as absent; this reads a property as they did.
pub fn num_or_zero(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(&Value::Num(n)) if n.is_finite() && n != 0.0 => Some(n),
        _ => None,
    }
}

// ------------------------------------------------------------------ geometry

/// A feature's geometry type (the spec's numbers).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GeomType {
    #[default]
    Unknown = 0,
    Point = 1,
    LineString = 2,
    Polygon = 3,
}

impl GeomType {
    fn of(n: u64) -> GeomType {
        match n {
            1 => GeomType::Point,
            2 => GeomType::LineString,
            3 => GeomType::Polygon,
            _ => GeomType::Unknown,
        }
    }
}

/// One step of a feature's geometry, the point in the tile's units (the cursor applied).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    MoveTo((f64, f64)),
    LineTo((f64, f64)),
    ClosePath,
}

/// A feature's geometry, step by step: MoveTo starts a part, LineTo goes on, ClosePath ends a
/// ring; coordinates are zigzag deltas from the cursor, which carries on from part to part. A
/// ClosePath is one step whatever its count says (the spec says 1); a command the spec doesn't
/// have, or a coordinate cut short, ends the geometry.
#[derive(Clone)]
pub struct Commands<'a> {
    pb: Pb<'a>,
    id: u64,
    left: u64,
    x: i64,
    y: i64,
    done: bool,
}

impl Iterator for Commands<'_> {
    type Item = Command;

    fn next(&mut self) -> Option<Command> {
        while !self.done {
            if self.left == 0 {
                if self.pb.i >= self.pb.b.len() {
                    break;
                }
                let Some(c) = self.pb.varint() else { break };
                (self.id, self.left) = (c & 7, c >> 3);
                match self.id {
                    1 | 2 => continue,
                    7 => {
                        self.left = 0;
                        return Some(Command::ClosePath);
                    }
                    _ => break,
                }
            }
            self.left -= 1;
            let (Some(dx), Some(dy)) = (self.pb.varint(), self.pb.varint()) else { break };
            self.x = self.x.wrapping_add(unzigzag(dx));
            self.y = self.y.wrapping_add(unzigzag(dy));
            let p = (self.x as f64, self.y as f64);
            return Some(if self.id == 1 { Command::MoveTo(p) } else { Command::LineTo(p) });
        }
        self.done = true;
        None
    }
}

// ------------------------------------------------------------------ features

/// One feature of a layer.
#[derive(Clone, Debug, PartialEq)]
pub struct Feature<'a> {
    pub id: Option<u64>,
    pub kind: GeomType,
    /// Its tags, each key once, in the order first given (a later value for a key replaces the
    /// earlier, as in loaders.gl). A tag whose key or value the layer doesn't have is left out.
    pub tags: Vec<(&'a str, Value<'a>)>,
    geometry: &'a [u8],
}

impl<'a> Feature<'a> {
    fn parse(b: &'a [u8], keys: &[&'a str], values: &[Option<Value<'a>>]) -> Feature<'a> {
        let mut f = Feature { id: None, kind: GeomType::Unknown, tags: Vec::new(), geometry: &[] };
        for (ff, v) in fields(b) {
            match (ff, v) {
                (1, Ok(id)) => f.id = Some(id),
                (2, Err(t)) => {
                    for kv in packed(t).chunks(2) {
                        if let [k, v] = kv
                            && let (Some(&k), Some(&Some(v))) = (keys.get(*k as usize), values.get(*v as usize))
                        {
                            match f.tags.iter_mut().find(|(key, _)| *key == k) {
                                Some(tag) => tag.1 = v,
                                None => f.tags.push((k, v)),
                            }
                        }
                    }
                }
                (3, Ok(k)) => f.kind = GeomType::of(k),
                (4, Err(g)) => f.geometry = g,
                _ => {}
            }
        }
        f
    }

    /// A tag's value.
    pub fn get(&self, key: &str) -> Option<&Value<'a>> {
        self.tags.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    /// A string tag.
    pub fn text(&self, key: &str) -> Option<&'a str> {
        match self.get(key) {
            Some(&Value::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// A numeric tag, if finite.
    pub fn num(&self, key: &str) -> Option<f64> {
        match self.get(key) {
            Some(&Value::Num(n)) if n.is_finite() => Some(n),
            _ => None,
        }
    }

    /// A numeric tag as the TypeScript's rules read it: [`num_or_zero`].
    pub fn num_or_zero(&self, key: &str) -> Option<f64> {
        num_or_zero(self.get(key))
    }

    /// Whether a bool tag is there and true.
    pub fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some(Value::Bool(true)))
    }

    /// Its tags as JSON (`Value::to_json`), as the TypeScript had a feature's properties.
    pub fn json_properties(&self) -> serde_json::Map<String, serde_json::Value> {
        self.tags.iter().map(|(k, v)| ((*k).to_owned(), v.to_json())).collect()
    }

    /// Its geometry, step by step.
    pub fn commands(&self) -> Commands<'a> {
        Commands { pb: Pb { b: self.geometry, i: 0 }, id: 0, left: 0, x: 0, y: 0, done: false }
    }

    /// Every point of its geometry, in order (a point feature's points).
    pub fn points(&self) -> impl Iterator<Item = (f64, f64)> + use<'a> {
        self.commands().filter_map(|c| match c {
            Command::MoveTo(p) | Command::LineTo(p) => Some(p),
            Command::ClosePath => None,
        })
    }

    /// Its geometry as lines: each MoveTo starts one, LineTo goes on; lines of fewer than two
    /// points are left out.
    pub fn lines(&self) -> Vec<Line> {
        let mut lines = Vec::new();
        let mut line: Line = Vec::new();
        for c in self.commands() {
            match c {
                Command::MoveTo(p) => {
                    if line.len() > 1 {
                        lines.push(std::mem::take(&mut line));
                    }
                    line.clear();
                    line.push(p);
                }
                Command::LineTo(p) => line.push(p),
                Command::ClosePath => {}
            }
        }
        if line.len() > 1 {
            lines.push(line);
        }
        lines
    }

    /// Its geometry as rings: each from a MoveTo to its ClosePath, open (the first point not
    /// repeated), in the tile's order (outer rings clockwise with y down, each followed by its
    /// holes). Rings of fewer than three points, and one not closed, are left out.
    pub fn rings(&self) -> Vec<Line> {
        let mut rings = Vec::new();
        let mut ring: Line = Vec::new();
        for c in self.commands() {
            match c {
                Command::MoveTo(p) => {
                    ring.clear();
                    ring.push(p);
                }
                Command::LineTo(p) => ring.push(p),
                Command::ClosePath => {
                    if ring.len() >= 3 {
                        rings.push(std::mem::take(&mut ring));
                    }
                    ring.clear();
                }
            }
        }
        rings
    }
}

// ------------------------------------------------------------------ layers

/// One layer of a tile.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer<'a> {
    pub name: &'a str,
    /// The spec's version (1 where the layer doesn't say).
    pub version: u64,
    /// The units across the tile ([`EXTENT_DEFAULT`] where the layer doesn't say).
    pub extent: f64,
    pub keys: Vec<&'a str>,
    /// Its values; none where one couldn't be read (a tag naming it is left out).
    pub values: Vec<Option<Value<'a>>>,
    features: Vec<&'a [u8]>,
}

impl<'a> Layer<'a> {
    /// A tile's `Layer` message. A name or key that isn't UTF-8 reads as "".
    pub fn parse(b: &'a [u8]) -> Layer<'a> {
        let mut layer =
            Layer { name: "", version: 1, extent: EXTENT_DEFAULT, keys: Vec::new(), values: Vec::new(), features: Vec::new() };
        for (lf, v) in fields(b) {
            match (lf, v) {
                (1, Err(b)) => layer.name = std::str::from_utf8(b).unwrap_or_default(),
                (2, Err(b)) => layer.features.push(b),
                (3, Err(b)) => layer.keys.push(std::str::from_utf8(b).unwrap_or_default()),
                (4, Err(b)) => layer.values.push(Value::parse(b)),
                (5, Ok(e)) => layer.extent = e as f64,
                (15, Ok(v)) => layer.version = v,
                _ => {}
            }
        }
        layer
    }

    /// How many features it has.
    pub fn len(&self) -> usize {
        self.features.len()
    }

    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    /// Its features, in order.
    pub fn features(&self) -> impl Iterator<Item = Feature<'a>> + '_ {
        self.features.iter().map(|b| Feature::parse(b, &self.keys, &self.values))
    }

    /// Its `i`th feature.
    pub fn feature(&self, i: usize) -> Option<Feature<'a>> {
        self.features.get(i).map(|b| Feature::parse(b, &self.keys, &self.values))
    }
}

/// A tile's `Layer` messages, in order.
fn layer_messages(pbf: &[u8]) -> impl Iterator<Item = &[u8]> {
    fields(pbf).filter_map(|(f, v)| match (f, v) {
        (3, Err(layer)) => Some(layer),
        _ => None,
    })
}

/// A `Layer` message's name, without reading the rest of it.
fn name_of(layer: &[u8]) -> &str {
    fields(layer)
        .filter_map(|(f, v)| match (f, v) {
            (1, Err(b)) => Some(std::str::from_utf8(b).unwrap_or_default()),
            _ => None,
        })
        .last()
        .unwrap_or_default()
}

/// A tile's layers, in the tile's order (empty ones too).
pub fn layers(pbf: &[u8]) -> impl Iterator<Item = Layer<'_>> {
    layer_messages(pbf).map(Layer::parse)
}

/// The layers of these names, as loaders.gl's `layers` option picks them: in the order named,
/// for each name the last layer of that name with any features, none where there is none. Only
/// the layers picked are read.
pub fn layers_named<'a>(pbf: &'a [u8], names: &[&str]) -> Vec<Layer<'a>> {
    let messages: Vec<(&str, &[u8])> = layer_messages(pbf).map(|b| (name_of(b), b)).collect();
    names
        .iter()
        .filter_map(|name| {
            messages.iter().rev().filter(|(n, _)| n == name).map(|(_, b)| Layer::parse(b)).find(|l| !l.is_empty())
        })
        .collect()
}

/// The layer of this name ([`layers_named`] for one).
pub fn layer<'a>(pbf: &'a [u8], name: &str) -> Option<Layer<'a>> {
    layers_named(pbf, &[name]).pop()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------- a small encoder for the tests

    fn varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push((v as u8) | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    fn tag(out: &mut Vec<u8>, field: u64, wire: u64) {
        varint(out, (field << 3) | wire);
    }

    fn uint(out: &mut Vec<u8>, field: u64, v: u64) {
        tag(out, field, 0);
        varint(out, v);
    }

    fn bytes(out: &mut Vec<u8>, field: u64, b: &[u8]) {
        tag(out, field, 2);
        varint(out, b.len() as u64);
        out.extend_from_slice(b);
    }

    fn packed_field(out: &mut Vec<u8>, field: u64, values: &[u64]) {
        let mut body = Vec::new();
        for &v in values {
            varint(&mut body, v);
        }
        bytes(out, field, &body);
    }

    const fn zigzag(v: i64) -> u64 {
        ((v << 1) ^ (v >> 63)) as u64
    }

    /// A geometry's command integers: `(id, points)` steps of absolute points, the cursor carried.
    fn geometry(steps: &[(u64, &[(i64, i64)])]) -> Vec<u64> {
        let (mut cx, mut cy) = (0, 0);
        let mut out = Vec::new();
        for &(id, points) in steps {
            out.push(((points.len().max(usize::from(id == 7)) as u64) << 3) | id);
            for &(x, y) in points {
                out.push(zigzag(x - cx));
                out.push(zigzag(y - cy));
                (cx, cy) = (x, y);
            }
        }
        out
    }

    fn feature(id: Option<u64>, kind: u64, tags: &[u64], commands: &[u64]) -> Vec<u8> {
        let mut f = Vec::new();
        if let Some(id) = id {
            uint(&mut f, 1, id);
        }
        packed_field(&mut f, 2, tags);
        uint(&mut f, 3, kind);
        packed_field(&mut f, 4, commands);
        f
    }

    fn value_str(s: &str) -> Vec<u8> {
        let mut v = Vec::new();
        bytes(&mut v, 1, s.as_bytes());
        v
    }

    fn value_uint(n: u64) -> Vec<u8> {
        let mut v = Vec::new();
        uint(&mut v, 5, n);
        v
    }

    /// A layer: its name, features, keys and values; extent and version only if given.
    fn layer_bytes(name: &str, features: &[Vec<u8>], keys: &[&str], values: &[Vec<u8>], extent: Option<u64>) -> Vec<u8> {
        let mut l = Vec::new();
        uint(&mut l, 15, 2);
        bytes(&mut l, 1, name.as_bytes());
        for f in features {
            bytes(&mut l, 2, f);
        }
        for k in keys {
            bytes(&mut l, 3, k.as_bytes());
        }
        for v in values {
            bytes(&mut l, 4, v);
        }
        if let Some(e) = extent {
            uint(&mut l, 5, e);
        }
        l
    }

    fn tile(layers: &[Vec<u8>]) -> Vec<u8> {
        let mut t = Vec::new();
        for l in layers {
            bytes(&mut t, 3, l);
        }
        t
    }

    /// A tile of one layer `name` with one feature of geometry `commands`.
    fn one(name: &str, kind: u64, commands: &[u64]) -> Vec<u8> {
        tile(&[layer_bytes(name, &[feature(None, kind, &[], commands)], &[], &[], None)])
    }

    // ---------------------------------------------------------- protobuf

    #[test]
    fn fields_of_every_wire_type_are_read() {
        let mut m = Vec::new();
        uint(&mut m, 1, 300);
        tag(&mut m, 2, 1);
        m.extend(1.5f64.to_le_bytes());
        bytes(&mut m, 3, b"abc");
        tag(&mut m, 4, 5);
        m.extend(2.5f32.to_le_bytes());
        uint(&mut m, 5, u64::MAX);
        let f: Vec<_> = fields(&m).collect();
        assert_eq!(f.len(), 5);
        assert_eq!(f[0], (1, Ok(300)));
        assert_eq!(f[1], (2, Err(&1.5f64.to_le_bytes()[..])));
        assert_eq!(f[2], (3, Err(&b"abc"[..])));
        assert_eq!(f[3], (4, Err(&2.5f32.to_le_bytes()[..])));
        assert_eq!(f[4], (5, Ok(u64::MAX)));
    }

    #[test]
    fn reading_stops_at_what_cannot_be_read() {
        // a group (wire type 3) is not a vector tile's
        let mut m = Vec::new();
        uint(&mut m, 1, 7);
        tag(&mut m, 2, 3);
        uint(&mut m, 3, 8);
        assert_eq!(fields(&m).count(), 1);
        // bytes running past the end
        let mut m = Vec::new();
        uint(&mut m, 1, 7);
        tag(&mut m, 2, 2);
        varint(&mut m, 10);
        m.extend(b"abc");
        assert_eq!(fields(&m).count(), 1);
        // a varint of more than 64 bits, and one cut short
        assert_eq!(packed(&[1, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 1]), vec![1]);
        assert_eq!(packed(&[5, 0x80]), vec![5]);
        assert_eq!(packed(&[0x80, 0x20, 3]), vec![4096, 3]);
    }

    // ---------------------------------------------------------- values

    #[test]
    fn values_of_every_type() {
        let message = |field: u64, wire: u64, body: &[u8]| {
            let mut v = Vec::new();
            tag(&mut v, field, wire);
            v.extend_from_slice(body);
            v
        };
        let mut s = Vec::new();
        varint(&mut s, 4);
        s.extend(b"Lund");
        assert_eq!(Value::parse(&message(1, 2, &s)), Some(Value::Str("Lund")));
        assert_eq!(Value::parse(&message(2, 5, &0.1f32.to_le_bytes())), Some(Value::Num(f64::from(0.1f32))));
        assert_eq!(Value::parse(&message(3, 1, &0.1f64.to_le_bytes())), Some(Value::Num(0.1)));
        let mut n = Vec::new();
        varint(&mut n, (-5i64) as u64);
        assert_eq!(Value::parse(&message(4, 0, &n)), Some(Value::Num(-5.0)), "int: two's complement");
        assert_eq!(Value::parse(&message(5, 0, &[0x80, 0x20])), Some(Value::Num(4096.0)));
        assert_eq!(Value::parse(&message(6, 0, &[9])), Some(Value::Num(-5.0)), "sint: zigzag");
        assert_eq!(Value::parse(&message(7, 0, &[1])), Some(Value::Bool(true)));
        assert_eq!(Value::parse(&message(7, 0, &[0])), Some(Value::Bool(false)));
        assert_eq!(Value::parse(&message(8, 0, &[1])), None, "a field the spec doesn't have");
        let mut bad = Vec::new();
        varint(&mut bad, 2);
        bad.extend([0xff, 0xfe]);
        assert_eq!(Value::parse(&message(1, 2, &bad)), None, "not UTF-8");
    }

    #[test]
    fn values_as_json() {
        assert_eq!(Value::Str("a").to_json(), serde_json::json!("a"));
        assert_eq!(Value::Num(160.0).to_json(), serde_json::json!(160));
        assert_eq!(Value::Num(160.0).to_json().as_i64(), Some(160));
        assert_eq!(Value::Num(-2.5).to_json(), serde_json::json!(-2.5));
        assert_eq!(Value::Num(f64::NAN).to_json(), serde_json::Value::Null);
        assert_eq!(Value::Num(1e300).to_json().as_f64(), Some(1e300));
        assert_eq!(Value::Bool(true).to_json(), serde_json::json!(true));
    }

    #[test]
    fn num_or_zero_reads_absent_and_zero_alike() {
        assert_eq!(num_or_zero(None), None);
        assert_eq!(num_or_zero(Some(&Value::Num(0.0))), None);
        assert_eq!(num_or_zero(Some(&Value::Num(-0.0))), None);
        assert_eq!(num_or_zero(Some(&Value::Num(12.5))), Some(12.5));
        assert_eq!(num_or_zero(Some(&Value::Num(-3.0))), Some(-3.0));
        assert_eq!(num_or_zero(Some(&Value::Num(f64::INFINITY))), None);
        assert_eq!(num_or_zero(Some(&Value::Str("12"))), None);
        assert_eq!(num_or_zero(Some(&Value::Bool(true))), None);
    }

    // ---------------------------------------------------------- layers and features

    #[test]
    fn a_layer_and_its_features() {
        let keys = ["class", "height", "tunnel", "name"];
        let values = [value_str("rail"), value_uint(12), {
            let mut v = Vec::new();
            uint(&mut v, 7, 1);
            v
        }];
        let line = geometry(&[(1, &[(10, 20)]), (2, &[(30, 20), (30, 50)])]);
        let features = [feature(Some(7), 2, &[0, 0, 1, 1, 2, 2], &line), feature(None, 3, &[], &[])];
        let t = tile(&[layer_bytes("railway", &features, &keys, &values, Some(512))]);
        let l = layers(&t).next().unwrap();
        assert_eq!((l.name, l.version, l.extent, l.len()), ("railway", 2, 512.0, 2));
        let f = l.feature(0).unwrap();
        assert_eq!(f.id, Some(7));
        assert_eq!(f.kind, GeomType::LineString);
        assert_eq!(f.text("class"), Some("rail"));
        assert_eq!(f.num("height"), Some(12.0));
        assert_eq!(f.num_or_zero("height"), Some(12.0));
        assert!(f.flag("tunnel") && !f.flag("bridge"));
        assert_eq!(f.text("name"), None);
        assert_eq!(f.lines(), vec![vec![(10.0, 20.0), (30.0, 20.0), (30.0, 50.0)]]);
        let props = f.json_properties();
        assert_eq!(props.len(), 3);
        assert_eq!(props["class"], serde_json::json!("rail"));
        assert_eq!(props["height"], serde_json::json!(12));
        assert_eq!(props["tunnel"], serde_json::json!(true));
        let g = l.features().nth(1).unwrap();
        assert_eq!((g.id, g.kind, g.tags.len()), (None, GeomType::Polygon, 0));
        assert!(l.feature(2).is_none());
    }

    #[test]
    fn a_layer_that_says_nothing_has_the_defaults() {
        let mut l = Vec::new();
        bytes(&mut l, 1, b"water");
        bytes(&mut l, 2, &feature(None, 9, &[], &[]));
        let l = Layer::parse(&l);
        assert_eq!((l.name, l.version, l.extent), ("water", 1, 4096.0));
        assert_eq!(l.feature(0).unwrap().kind, GeomType::Unknown);
    }

    #[test]
    fn tags_each_key_once_the_later_value_winning() {
        let keys = ["a", "b"];
        let values = [value_uint(1), value_uint(2), vec![0x40, 1]];
        // a=1, b=2, a=2; then a key and a value the layer doesn't have, a value it couldn't read, and an odd one out
        let f = feature(None, 1, &[0, 0, 1, 1, 0, 1, 5, 0, 1, 9, 1, 2, 0], &[]);
        let t = tile(&[layer_bytes("x", &[f], &keys, &values, None)]);
        let l = layers(&t).next().unwrap();
        assert_eq!(l.values[2], None);
        assert_eq!(l.feature(0).unwrap().tags, vec![("a", Value::Num(2.0)), ("b", Value::Num(2.0))]);
    }

    #[test]
    fn layers_are_picked_by_name_as_loaders_gl_picks_them() {
        let f = || feature(None, 1, &[], &geometry(&[(1, &[(1, 1)])]));
        let t = tile(&[
            layer_bytes("water", &[f()], &[], &[], None),
            layer_bytes("roads", &[f()], &[], &[], Some(1)),
            layer_bytes("roads", &[f(), f()], &[], &[], Some(2)),
            layer_bytes("roads", &[], &[], &[], Some(3)),
            layer_bytes("empty", &[], &[], &[], None),
        ]);
        assert_eq!(layers(&t).map(|l| l.name).collect::<Vec<_>>(), ["water", "roads", "roads", "roads", "empty"]);
        // in the order asked, the last non-empty of each name, none for one missing or empty
        let picked = layers_named(&t, &["roads", "nothing", "empty", "water"]);
        assert_eq!(picked.iter().map(|l| (l.name, l.extent)).collect::<Vec<_>>(), [("roads", 2.0), ("water", 4096.0)]);
        assert_eq!(layer(&t, "roads").unwrap().len(), 2);
        assert!(layer(&t, "empty").is_none());
        assert!(layers(&[]).next().is_none());
    }

    // ---------------------------------------------------------- geometry

    #[test]
    fn commands_carry_the_cursor_through_zigzag_deltas() {
        let g = geometry(&[(1, &[(5, 5)]), (2, &[(-64, 4160)]), (7, &[]), (1, &[(3, -2)])]);
        let t = one("x", 3, &g);
        let f = layers(&t).next().unwrap().feature(0).unwrap();
        assert_eq!(
            f.commands().collect::<Vec<_>>(),
            [
                Command::MoveTo((5.0, 5.0)),
                Command::LineTo((-64.0, 4160.0)),
                Command::ClosePath,
                Command::MoveTo((3.0, -2.0))
            ]
        );
    }

    #[test]
    fn lines_start_at_each_move_to() {
        // a line, a lone point (left out), a line; then a MoveTo of two points (each its own start)
        let g = geometry(&[
            (1, &[(0, 0)]),
            (2, &[(10, 0), (10, 10)]),
            (1, &[(50, 50)]),
            (1, &[(20, 20)]),
            (2, &[(30, 30)]),
            (7, &[]),
            (1, &[(1, 1), (2, 2)]),
            (2, &[(3, 3)]),
        ]);
        let t = one("x", 2, &g);
        let f = layers(&t).next().unwrap().feature(0).unwrap();
        assert_eq!(
            f.lines(),
            vec![
                vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)],
                vec![(20.0, 20.0), (30.0, 30.0)],
                vec![(2.0, 2.0), (3.0, 3.0)]
            ]
        );
    }

    #[test]
    fn rings_end_at_their_close_path_and_stay_open() {
        // an outer ring (clockwise, y down), its hole, a ring too small, one never closed
        let outer: &[(i64, i64)] = &[(100, 100), (200, 100), (200, 200), (100, 200)];
        let hole: &[(i64, i64)] = &[(120, 120), (120, 180), (180, 180), (180, 120)];
        let g = geometry(&[
            (1, &outer[..1]),
            (2, &outer[1..]),
            (7, &[]),
            (1, &hole[..1]),
            (2, &hole[1..]),
            (7, &[]),
            (1, &[(0, 0)]),
            (2, &[(5, 5)]),
            (7, &[]),
            (1, &[(300, 300)]),
            (2, &[(400, 300), (400, 400)]),
        ]);
        let t = one("building", 3, &g);
        let f = layers(&t).next().unwrap().feature(0).unwrap();
        let pts = |r: &[(i64, i64)]| r.iter().map(|&(x, y)| (x as f64, y as f64)).collect::<Vec<_>>();
        assert_eq!(f.rings(), vec![pts(outer), pts(hole)]);
        assert_eq!(f.points().count(), 4 + 4 + 2 + 3);
    }

    #[test]
    fn points_of_a_multi_point() {
        let t = one("poi", 1, &geometry(&[(1, &[(2048, 2048), (-10, 4100)])]));
        let f = layers(&t).next().unwrap().feature(0).unwrap();
        assert_eq!(f.kind, GeomType::Point);
        assert_eq!(f.points().collect::<Vec<_>>(), [(2048.0, 2048.0), (-10.0, 4100.0)]);
    }

    #[test]
    fn geometry_ends_at_an_unknown_command_or_a_point_cut_short() {
        let mut g = geometry(&[(1, &[(1, 1)]), (2, &[(2, 2)])]);
        g.push((1 << 3) | 3);
        g.extend([2, 2]);
        g.extend(geometry(&[(2, &[(9, 9)])]));
        let t = one("x", 2, &g);
        let f = layers(&t).next().unwrap().feature(0).unwrap();
        assert_eq!(f.lines(), vec![vec![(1.0, 1.0), (2.0, 2.0)]]);
        // a LineTo of two points with one and a half given
        let t = one("x", 2, &[9, 2, 2, (2 << 3) | 2, 2, 2, 2]);
        let f = layers(&t).next().unwrap().feature(0).unwrap();
        let mut c = f.commands();
        assert_eq!(c.next(), Some(Command::MoveTo((1.0, 1.0))));
        assert_eq!(c.next(), Some(Command::LineTo((2.0, 2.0))));
        assert_eq!(c.next(), None);
        assert_eq!(c.next(), None);
        // a command of no points steps over
        let t = one("x", 2, &[1, 9, 2, 2, 2, 10, 2, 2]);
        assert_eq!(layers(&t).next().unwrap().feature(0).unwrap().lines(), vec![vec![(1.0, 1.0), (2.0, 2.0)]]);
    }
}
