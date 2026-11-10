//! Port of `aircraft.ts` (doc 03 §4.1–4.4): real aircraft shapes, built from the Flightradar24
//! 3D models by `scripts/aircraft/build.mjs` and shipped in `static/aircraft/`.
//!
//! Each type comes at two levels of detail (a couple of hundred triangles for the wide view, a
//! couple of thousand up close) plus a top-down silhouette used when the map is flat, and most
//! carry landing gear as a separate mesh. Geometry is in metres in RustRadar's frame: nose along
//! +y, right wing +x, up +z.
//!
//! The fetching belongs to the app: it reads [`INDEX_FILE`] and [`MESHES_FILE`] and hands both
//! to [`AircraftModels::parse`]. On an error the TypeScript only warns and keeps drawing the
//! procedural models ([`crate::aircraft_models`]); the caller does the same with the `Err`.
//!
//! `meshes.bin` is a headerless run of little-endian sections, each 4-byte aligned; every
//! offset and count comes from `index.json`. A mesh (a level of detail or a gear) is
//! `vertices × 3` `f32` positions, `vertices × 3` `i8` normals (`n / 127`) and
//! `triangles × 3` `u16` indices. A level's index buffer is ordered by livery part, so a part is
//! a range of it ([`ModelLod::part_indices`]): in wgpu, one vertex and one index buffer per level
//! and a `draw_indexed` range per part.
//!
//! The section readers ([`f32s`], [`i8_normals`], [`u8_colors`], [`u16s`]) are shared with
//! [`crate::train_models`], whose `meshes.bin` is laid out the same way.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;

use indexmap::IndexMap;
use rustradar_core::liveries::Part;
use serde::Deserialize;
use serde::de::{Deserializer, MapAccess, Visitor};

/// Where the files are, under the static assets (`BASE` and the names `aircraft.ts` fetches).
pub const INDEX_FILE: &str = "aircraft/index.json";
pub const MESHES_FILE: &str = "aircraft/meshes.bin";
/// The silhouettes' atlas: white, the silhouette in alpha (doc 03 §4.4).
pub const ICON_ATLAS: &str = "aircraft/icons.ktx2";
/// The silhouettes grown into a border, drawn under them.
pub const OUTLINE_ATLAS: &str = "aircraft/icons-outline.ktx2";

/// Levels of detail, coarsest first.
pub const LOD_COUNT: usize = 2;

/// Why a model file could not be read: what JavaScript's `fetch(...).json()` or a typed-array
/// constructor would have thrown.
#[derive(Debug)]
pub enum MeshFileError {
    /// `index.json` is not JSON of the expected shape.
    Index(serde_json::Error),
    /// A section runs past the end of `meshes.bin` (a `RangeError` in JavaScript).
    OutOfBounds { offset: usize, bytes: usize, len: usize },
    /// A section does not start on a multiple of its element size (also a `RangeError`).
    Misaligned { offset: usize, align: usize },
}

impl fmt::Display for MeshFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MeshFileError::Index(e) => write!(f, "model index: {e}"),
            MeshFileError::OutOfBounds { offset, bytes, len } => {
                write!(f, "mesh section of {bytes} bytes at {offset} runs past the end of the {len}-byte file")
            }
            MeshFileError::Misaligned { offset, align } => {
                write!(f, "mesh section at {offset} is not aligned to {align} bytes")
            }
        }
    }
}

impl std::error::Error for MeshFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MeshFileError::Index(e) => Some(e),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for MeshFileError {
    fn from(e: serde_json::Error) -> Self {
        MeshFileError::Index(e)
    }
}

/// The bytes of `count` elements of `size` bytes at `offset`, as a typed-array view of them
/// demands: aligned to the element size and inside the buffer.
fn section(buffer: &[u8], offset: usize, count: usize, size: usize) -> Result<&[u8], MeshFileError> {
    if !offset.is_multiple_of(size) {
        return Err(MeshFileError::Misaligned { offset, align: size });
    }
    let bytes = count.saturating_mul(size);
    match offset.checked_add(bytes) {
        Some(end) if end <= buffer.len() => Ok(&buffer[offset..end]),
        _ => Err(MeshFileError::OutOfBounds { offset, bytes, len: buffer.len() }),
    }
}

/// `new Float32Array(buffer, offset, count)`, copied.
pub fn f32s(buffer: &[u8], offset: usize, count: usize) -> Result<Vec<f32>, MeshFileError> {
    let bytes = section(buffer, offset, count, 4)?;
    Ok(bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

/// `new Uint16Array(buffer, offset, count)`, copied.
pub fn u16s(buffer: &[u8], offset: usize, count: usize) -> Result<Vec<u16>, MeshFileError> {
    let bytes = section(buffer, offset, count, 2)?;
    Ok(bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect())
}

/// Normals shipped as signed bytes, as floats: `Float32Array.from(new Int8Array(buffer, offset,
/// count), (v) => v / 127)`.
pub fn i8_normals(buffer: &[u8], offset: usize, count: usize) -> Result<Vec<f32>, MeshFileError> {
    let bytes = section(buffer, offset, count, 1)?;
    Ok(bytes.iter().map(|&b| (b as i8 as f64 / 127.0) as f32).collect())
}

/// Colours shipped as bytes, as floats 0..1: `Float32Array.from(new Uint8Array(buffer, offset,
/// count), (v) => v / 255)`.
pub fn u8_colors(buffer: &[u8], offset: usize, count: usize) -> Result<Vec<f32>, MeshFileError> {
    let bytes = section(buffer, offset, count, 1)?;
    Ok(bytes.iter().map(|&b| (b as f64 / 255.0) as f32).collect())
}

/// A JSON object read as its entries in the file's order (`Object.entries` of it).
fn ordered<'de, D, V>(deserializer: D) -> Result<Vec<(String, V)>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct Entries<V>(std::marker::PhantomData<V>);
    impl<'de, V: Deserialize<'de>> Visitor<'de> for Entries<V> {
        type Value = Vec<(String, V)>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an object")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut entries = Vec::with_capacity(map.size_hint().unwrap_or(0));
            while let Some(entry) = map.next_entry()? {
                entries.push(entry);
            }
            Ok(entries)
        }
    }
    deserializer.deserialize_map(Entries(std::marker::PhantomData))
}

/// One level of detail in `index.json` (`PackedLod`). Offsets are bytes into `meshes.bin`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PackedLod {
    pub positions: usize,
    pub normals: usize,
    pub indices: usize,
    pub vertices: usize,
    pub triangles: usize,
    /// Livery part name to its `[first triangle, count]` run of the index buffer, in the file's
    /// order.
    #[serde(deserialize_with = "ordered")]
    pub parts: Vec<(String, [usize; 2])>,
}

/// The landing gear in `index.json` (`PackedGear`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackedGear {
    pub positions: usize,
    pub normals: usize,
    pub indices: usize,
    pub vertices: usize,
    pub triangles: usize,
    /// Height of the plane the struts hang from; the whole gear lies below it.
    pub attach_z: f64,
}

/// One model in `index.json` (`PackedModel`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PackedModel {
    /// Largest dimension in metres.
    pub size: f64,
    pub span: f64,
    pub length: f64,
    pub lods: Vec<PackedLod>,
    /// Absent for types that never show gear (helicopter, glider).
    #[serde(default)]
    pub gear: Option<PackedGear>,
}

/// The silhouettes grown into a border, in larger cells padded by `pad` px.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct OutlineCells {
    pub cell: u32,
    pub pad: u32,
}

/// The silhouette atlas's layout: square cells of `cell` px, `columns` to a row, one per name.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct IconCells {
    pub cell: u32,
    pub columns: u32,
    pub names: Vec<String>,
    #[serde(default)]
    pub outline: Option<OutlineCells>,
}

/// `static/aircraft/index.json` as written (`Index`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AircraftIndex {
    #[serde(deserialize_with = "ordered")]
    pub models: Vec<(String, PackedModel)>,
    /// ICAO type designator to model name.
    pub types: HashMap<String, String>,
    /// Flightradar24 icon class to model name, for types not in `types`.
    pub icons: HashMap<String, String>,
    /// Triangle budgets of the levels of detail (informational).
    pub lods: Vec<u32>,
    pub icon: IconCells,
}

impl AircraftIndex {
    /// `index.json` read (`r.json() as Index`).
    pub fn parse(json: &[u8]) -> Result<AircraftIndex, MeshFileError> {
        Ok(serde_json::from_slice(json)?)
    }
}

/// An indexed triangle list: `x, y, z` per vertex and three indices per triangle.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IndexedMesh {
    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
    pub indices: Vec<u16>,
}

impl IndexedMesh {
    /// One packed mesh's sections read from the file (`mesh`).
    fn read(buffer: &[u8], positions: usize, normals: usize, indices: usize, vertices: usize, triangles: usize) -> Result<Self, MeshFileError> {
        Ok(IndexedMesh {
            positions: f32s(buffer, positions, vertices.saturating_mul(3))?,
            // normals ship as signed bytes; the GPU wants floats
            normals: i8_normals(buffer, normals, vertices.saturating_mul(3))?,
            indices: u16s(buffer, indices, triangles.saturating_mul(3))?,
        })
    }

    pub fn vertices(&self) -> usize {
        self.positions.len() / 3
    }

    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
}

/// One level of detail (`ModelLod`): the mesh, whose whole index buffer draws every triangle
/// (the TypeScript's `combined`), and each livery part's range of that index buffer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelLod {
    pub mesh: IndexedMesh,
    /// Each part's indices (not triangles), in the file's order. Ranges are clamped to the index
    /// buffer, as `subarray` clamps.
    pub parts: Vec<(Part, Range<u32>)>,
}

impl ModelLod {
    fn read(buffer: &[u8], lod: &PackedLod) -> Result<Self, MeshFileError> {
        let mesh = IndexedMesh::read(buffer, lod.positions, lod.normals, lod.indices, lod.vertices, lod.triangles)?;
        let len = mesh.indices.len();
        let parts = lod
            .parts
            .iter()
            // a name that is no livery part has no colour to take; none is in the shipped file
            .filter_map(|(name, [start, count])| {
                let part = Part::from_name(name)?;
                let begin = start.saturating_mul(3).min(len);
                let end = start.saturating_add(*count).saturating_mul(3).clamp(begin, len);
                Some((part, begin as u32..end as u32))
            })
            .collect();
        Ok(ModelLod { mesh, parts })
    }

    /// A part's indices (`indices.subarray(start * 3, (start + count) * 3)`).
    pub fn part_indices(&self, range: &Range<u32>) -> &[u16] {
        &self.mesh.indices[range.start as usize..range.end as usize]
    }
}

/// The landing gear (`ModelGear`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelGear {
    /// Nose and main gear, extended, in the same frame as the levels of detail.
    pub mesh: IndexedMesh,
    /// Height (model z, metres) of the plane the struts hang from. Scaling the gear vertically
    /// about it, from 1 down to 0, retracts it into the airframe.
    pub attach_z: f64,
}

/// A published model (`AircraftModel`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AircraftModel {
    pub name: String,
    /// Largest dimension in metres.
    pub size: f64,
    pub span: f64,
    pub length: f64,
    /// Coarsest first.
    pub lods: Vec<ModelLod>,
    /// Landing gear, or `None` for types without any.
    pub gear: Option<ModelGear>,
}

/// Where a silhouette is in its atlas, px (an entry of deck.gl's icon mapping). All are white
/// alpha masks, so the instance colour tints them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconCell {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub mask: bool,
}

/// Each name's cell in an atlas of square `size` px cells, `columns` to a row (`mapping`).
fn mapping(names: &[String], columns: u32, size: u32) -> IndexMap<String, IconCell> {
    let columns = columns.max(1) as usize;
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let cell = IconCell {
                x: (i % columns) as u32 * size,
                y: (i / columns) as u32 * size,
                width: size,
                height: size,
                mask: true,
            };
            (name.clone(), cell)
        })
        .collect()
}

/// The published models once loaded (`AircraftModels` after `read()`): meshes, the lookup by
/// type and by icon, and the silhouette atlases' layouts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AircraftModels {
    models: HashMap<String, AircraftModel>,
    by_type: HashMap<String, String>,
    by_icon: HashMap<String, String>,
    /// The silhouettes' cells in [`ICON_ATLAS`], by model name.
    pub icon_mapping: IndexMap<String, IconCell>,
    /// [`OUTLINE_ATLAS`] when the build made one: the silhouettes' border, drawn under them.
    pub outline_atlas: Option<&'static str>,
    /// The border's cells in [`OUTLINE_ATLAS`]; empty without one.
    pub outline_mapping: IndexMap<String, IconCell>,
    /// How much bigger an outline cell is than a silhouette cell (1 without an outline).
    pub outline_scale: f64,
}

impl AircraftModels {
    /// Both files read (`read()`): every model's levels of detail and gear, the lookups and the
    /// atlas layouts. Any section out of bounds or misaligned fails the whole load.
    pub fn parse(index_json: &[u8], meshes_bin: &[u8]) -> Result<AircraftModels, MeshFileError> {
        AircraftModels::from_index(AircraftIndex::parse(index_json)?, meshes_bin)
    }

    /// [`AircraftModels::parse`] with `index.json` already read.
    pub fn from_index(index: AircraftIndex, buffer: &[u8]) -> Result<AircraftModels, MeshFileError> {
        let mut models = HashMap::with_capacity(index.models.len());
        for (name, packed) in &index.models {
            let lods = packed.lods.iter().map(|lod| ModelLod::read(buffer, lod)).collect::<Result<Vec<_>, _>>()?;
            let gear = match &packed.gear {
                None => None,
                Some(g) => Some(ModelGear {
                    mesh: IndexedMesh::read(buffer, g.positions, g.normals, g.indices, g.vertices, g.triangles)?,
                    attach_z: g.attach_z,
                }),
            };
            let model =
                AircraftModel { name: name.clone(), size: packed.size, span: packed.span, length: packed.length, lods, gear };
            models.insert(name.clone(), model);
        }
        let IconCells { cell, columns, names, outline } = &index.icon;
        let mut loaded = AircraftModels {
            models,
            by_type: index.types,
            by_icon: index.icons,
            icon_mapping: mapping(names, *columns, *cell),
            outline_atlas: None,
            outline_mapping: IndexMap::new(),
            outline_scale: 1.0,
        };
        if let Some(outline) = outline {
            loaded.outline_mapping = mapping(names, *columns, outline.cell);
            loaded.outline_atlas = Some(OUTLINE_ATLAS);
            loaded.outline_scale = outline.cell as f64 / *cell as f64;
        }
        Ok(loaded)
    }

    /// The closest published model for a flight, or `None` to fall back (`forFlight`): by its
    /// ICAO type, else by its Flightradar24 icon class.
    pub fn for_flight(&self, typecode: &str, icon: &str) -> Option<&AircraftModel> {
        let name = self.by_type.get(&typecode.to_uppercase()).or_else(|| self.by_icon.get(icon))?;
        self.models.get(name)
    }

    /// A model by its name.
    pub fn get(&self, name: &str) -> Option<&AircraftModel> {
        self.models.get(name)
    }

    /// How many models there are.
    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Every model, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = &AircraftModel> {
        self.models.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;

    fn read(name: &str) -> Vec<u8> {
        let path = format!("{}/../../static/aircraft/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    fn shipped() -> (AircraftIndex, AircraftModels) {
        let (index, bin) = (read("index.json"), read("meshes.bin"));
        (AircraftIndex::parse(&index).unwrap(), AircraftModels::parse(&index, &bin).unwrap())
    }

    fn bbox(p: &[f32]) -> ([f64; 3], [f64; 3]) {
        let (mut min, mut max) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for v in p.chunks_exact(3) {
            for k in 0..3 {
                min[k] = min[k].min(v[k] as f64);
                max[k] = max[k].max(v[k] as f64);
            }
        }
        (min, max)
    }

    const NAMES: &str = "a318 a319 a320 a321 a332 a333 a343 a346 a359 a380 an225 ask21 atr42 b736 b737 b738 b739 \
                         b744 b748 b752 b753 b762 b763 b764 b772 b773 b788 b789 bae146 beluga citation crj700 crj900 \
                         cs100 cs300 e170 e190 heli pa28 q400";

    #[test]
    fn the_shipped_index_has_forty_models_and_their_lookups() {
        let (index, models) = shipped();
        let names: Vec<&str> = index.models.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, NAMES.split_whitespace().collect::<Vec<_>>());
        assert_eq!(models.len(), 40);
        assert_eq!(index.types.len(), 138);
        assert_eq!(index.icons.len(), 24);
        assert_eq!(index.lods, [220, 2600]);
        assert_eq!((index.icon.cell, index.icon.columns), (128, 7));
        assert_eq!(index.icon.outline, Some(OutlineCells { cell: 152, pad: 12 }));
        // the atlas has a cell per model, in the models' order
        assert_eq!(index.icon.names, names);
        // every lookup lands on a model
        assert!(index.types.values().chain(index.icons.values()).all(|name| models.get(name).is_some()));
        let gearless: Vec<&str> =
            index.models.iter().filter(|(_, m)| m.gear.is_none()).map(|(n, _)| n.as_str()).collect();
        assert_eq!(gearless, ["ask21", "heli"]);
    }

    #[test]
    fn a318_reads_as_the_index_describes_it() {
        let (_, models) = shipped();
        let a318 = models.get("a318").unwrap();
        assert_eq!(a318.name, "a318");
        assert_eq!((a318.size, a318.span, a318.length), (33.91, 33.91, 31.71));
        assert_eq!(a318.lods.len(), LOD_COUNT);
        let counts: Vec<(usize, usize)> = a318.lods.iter().map(|l| (l.mesh.vertices(), l.mesh.triangles())).collect();
        assert_eq!(counts, [(95, 215), (1092, 2695)]);
        let parts: Vec<(Part, Range<u32>)> = a318.lods[0].parts.clone();
        assert_eq!(
            parts,
            [(Part::Top, 0..66), (Part::Belly, 66..327), (Part::Wings, 327..606), (Part::Tail, 606..645)]
        );
        let gear = a318.gear.as_ref().unwrap();
        assert_eq!((gear.mesh.vertices(), gear.mesh.triangles()), (204, 162));
        assert_eq!(gear.attach_z, -3.064);
        // the first vertex as the file has it
        let bin = read("meshes.bin");
        assert_eq!(a318.lods[0].mesh.positions[0], f32::from_le_bytes([bin[0], bin[1], bin[2], bin[3]]));
        assert_eq!(a318.lods[0].mesh.normals[0], (bin[1140] as i8 as f64 / 127.0) as f32);
    }

    #[test]
    fn every_level_of_detail_is_whole_and_fits_its_model() {
        let (_, models) = shipped();
        let mut most = 0;
        for m in models.iter() {
            assert_eq!(m.lods.len(), LOD_COUNT, "{}", m.name);
            assert_eq!(m.size, m.span.max(m.length), "{}", m.name);
            for (level, lod) in m.lods.iter().enumerate() {
                let mesh = &lod.mesh;
                most = most.max(mesh.vertices());
                assert!(mesh.indices.iter().all(|&i| (i as usize) < mesh.vertices()), "{} lod {level}", m.name);
                // the parts run one after another over the whole index buffer; no engines part
                let mut at = 0;
                for (part, range) in &lod.parts {
                    assert_ne!(*part, Part::Engines, "{}", m.name);
                    assert_eq!(range.start, at, "{} lod {level}", m.name);
                    assert_eq!(lod.part_indices(range).len(), range.len());
                    at = range.end;
                }
                assert_eq!(at as usize, mesh.indices.len(), "{} lod {level}", m.name);
                // unit normals, to the bytes' precision (a few vertices carry none)
                for n in mesh.normals.chunks_exact(3) {
                    let len = ((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]) as f64).sqrt();
                    assert!(len == 0.0 || (0.99..1.01).contains(&len), "{} lod {level}: {len}", m.name);
                }
                // centred, spanning the model's span across and its length along, a little
                // less for the coarse level
                let (lo, hi) = bbox(&mesh.positions);
                let (across, along) = (hi[0] - lo[0], hi[1] - lo[1]);
                let slack = if level == 0 { 0.9 } else { 0.98 };
                assert!(across <= m.span * 1.001 && across >= m.span * slack, "{} lod {level}: {across}", m.name);
                assert!(along <= m.length * 1.001 && along >= m.length * slack, "{} lod {level}: {along}", m.name);
                assert!((lo[0] + hi[0]).abs() < 0.04 * m.size, "{} lod {level}", m.name);
                assert!((lo[1] + hi[1]).abs() < 0.04 * m.size, "{} lod {level}", m.name);
            }
            if let Some(gear) = &m.gear {
                // the gear hangs below its attachment plane (rounded to a millimetre in the file)
                let (lo, hi) = bbox(&gear.mesh.positions);
                assert!(hi[2] <= gear.attach_z + 1e-3, "{}: gear up to {} above {}", m.name, hi[2], gear.attach_z);
                assert!(lo[2] < gear.attach_z && gear.attach_z < 0.0, "{}", m.name);
                assert!(gear.mesh.indices.iter().all(|&i| (i as usize) < gear.mesh.vertices()), "{}", m.name);
            }
        }
        assert_eq!(most, 1579);
        // the largest and the smallest
        assert_eq!(models.get("an225").unwrap().size, 87.92);
        assert_eq!(models.get("pa28").unwrap().size, 10.55);
        let pa28 = bbox(&models.get("pa28").unwrap().lods[1].mesh.positions);
        assert_close!(pa28.0[0], -5.266328811645508, 9);
        assert_close!(pa28.1[0], 5.266478061676025, 9);
    }

    #[test]
    fn flights_find_their_model_by_type_then_by_icon() {
        let (_, models) = shipped();
        let name = |typecode: &str, icon: &str| models.for_flight(typecode, icon).map(|m| m.name.as_str());
        assert_eq!(name("B738", ""), Some("b738"));
        // types are looked up in upper case
        assert_eq!(name("a20n", "B738"), Some("a320"));
        assert_eq!(name("b738", "XXXX"), Some("b738"));
        // a type without a model goes by its icon class
        assert_eq!(name("ZZZZ", "B757"), Some("b752"));
        assert_eq!(name("", "EC"), Some("heli"));
        assert_eq!(name("", "C206"), Some("pa28"));
        // icon classes are not upper-cased
        assert_eq!(name("ZZZZ", "b757"), None);
        assert_eq!(name("ZZZZ", "XXXX"), None);
    }

    #[test]
    fn silhouettes_have_a_cell_each_in_both_atlases() {
        let (_, models) = shipped();
        assert_eq!(models.icon_mapping.len(), 40);
        assert_eq!(models.icon_mapping["a318"], IconCell { x: 0, y: 0, width: 128, height: 128, mask: true });
        // the eighth model starts the second row of seven
        assert_eq!(models.icon_mapping["a346"], IconCell { x: 0, y: 128, width: 128, height: 128, mask: true });
        assert_eq!(models.icon_mapping["q400"], IconCell { x: 4 * 128, y: 5 * 128, width: 128, height: 128, mask: true });
        assert_eq!(models.outline_atlas, Some(OUTLINE_ATLAS));
        assert_eq!(models.outline_mapping["q400"], IconCell { x: 4 * 152, y: 5 * 152, width: 152, height: 152, mask: true });
        assert_eq!(models.outline_scale, 1.1875);
    }

    #[test]
    fn an_index_without_an_outline_has_none() {
        let json = br#"{"models":{},"types":{},"icons":{},"lods":[1,2],"icon":{"cell":64,"columns":2,"names":["a","b","c"]}}"#;
        let models = AircraftModels::parse(json, &[]).unwrap();
        assert!(models.is_empty());
        assert_eq!(models.outline_atlas, None);
        assert!(models.outline_mapping.is_empty());
        assert_eq!(models.outline_scale, 1.0);
        assert_eq!(models.icon_mapping["c"], IconCell { x: 0, y: 64, width: 64, height: 64, mask: true });
    }

    #[test]
    fn broken_files_fail_the_load() {
        let lod = |positions: usize| {
            format!(
                r#"{{"models":{{"m":{{"size":1,"span":1,"length":1,"lods":[{{"positions":{positions},"normals":36,"indices":48,"vertices":3,"triangles":1,"parts":{{"tail":[0,1],"top":[0,9]}}}}]}}}},
                "types":{{}},"icons":{{}},"lods":[],"icon":{{"cell":1,"columns":1,"names":[]}}}}"#
            )
        };
        let mut bin = vec![0u8; 56];
        for (i, v) in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0].iter().enumerate() {
            bin[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        bin[36..45].copy_from_slice(&[0, 0, 127, 0, 0, 127, 0, 0, 0x81]);
        bin[48..54].copy_from_slice(&[0, 0, 1, 0, 2, 0]);
        let ok = AircraftModels::parse(lod(0).as_bytes(), &bin).unwrap();
        let lod0 = &ok.get("m").unwrap().lods[0];
        assert_eq!(lod0.mesh.indices, [0, 1, 2]);
        assert_eq!(lod0.mesh.normals[2], 1.0);
        assert_eq!(lod0.mesh.normals[8], -1.0);
        // the parts as the file orders them; a run past the end is clamped, as `subarray` does
        assert_eq!(lod0.parts, [(Part::Tail, 0..3), (Part::Top, 0..3)]);
        assert!(matches!(AircraftModels::parse(lod(2).as_bytes(), &bin), Err(MeshFileError::Misaligned { offset: 2, align: 4 })));
        assert!(matches!(AircraftModels::parse(lod(40).as_bytes(), &bin), Err(MeshFileError::OutOfBounds { .. })));
        assert!(matches!(AircraftModels::parse(&bin, &bin), Err(MeshFileError::Index(_))));
        assert!(AircraftModels::parse(lod(0).as_bytes(), &bin[..50]).is_err());
    }
}
