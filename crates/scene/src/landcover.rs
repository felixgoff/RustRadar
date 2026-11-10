//! Port of `landcover.ts` (S1.23; doc 04 §11): what covers the land, 10 m by 10 m, for the
//! realistic map where the map's own data says nothing (and which trees a wood has, which it
//! seldom says):
//!
//! - In Sweden, Naturvårdsverket's Nationella Marktäckedata (NMD2023, CC0): pine, spruce and
//!   broadleaved forest apart, wetland, heath, fields. Its WMS draws each class in its own exact
//!   colour, read back here to the class.
//! - Elsewhere (and past Sweden's borders and waters, where NMD is clear), ESA WorldCover 2021
//!   (CC BY 4.0) through Microsoft's Planetary Computer, whose tiles carry the class codes
//!   themselves.
//!
//! Each is read as a picture of a map tile, a class per pixel. This is the pure part: the
//! classes, the colour and code tables, the URLs, and how the two sources are put together.
//! Fetching, decoding the pictures (no colour management, no premultiplying, nearest-neighbour
//! to [`SIZE`]²) and the cache of [`MAX_CACHED`] tiles (failures forgotten) are the caller's. A
//! tile's land cover, as `landCover` gets it:
//!
//! 1. NMD's classes ([`nmd_classes`] of the picture at [`nmd_url`]) when [`in_sweden`];
//! 2. WorldCover's classes ([`world_cover_classes`] of the picture at [`world_cover_url`], the
//!    mosaic registered once at [`WORLD_COVER_REGISTER`]) when [`wants_world_cover`];
//! 3. the two put together by [`combine`].

use std::collections::HashMap;
use std::sync::LazyLock;

use rustradar_core::num::to_fixed;

/// The classes, the map's own: what grows where, as the trees and the ground want it
/// (`landcover.ts` `LC`).
pub mod lc {
    pub const NONE: u8 = 0;
    pub const CONIFERS: u8 = 1;
    pub const MIXED: u8 = 2;
    pub const BROADLEAVES: u8 = 3;
    /// Trees, kind not known (WorldCover).
    pub const FOREST: u8 = 4;
    /// Felled, growing back.
    pub const CLEARCUT: u8 = 5;
    pub const WETLAND: u8 = 6;
    pub const ARABLE: u8 = 7;
    pub const GRASS: u8 = 8;
    pub const SHRUB: u8 = 9;
    pub const HEATH: u8 = 10;
    pub const BARE: u8 = 11;
    pub const BUILT: u8 = 12;
    pub const WATER: u8 = 13;
    pub const SNOW: u8 = 14;
}

/// One of the [`lc`] classes.
pub type LandClass = u8;

/// Whether a class is woodland.
pub fn wooded(c: LandClass) -> bool {
    (lc::CONIFERS..=lc::FOREST).contains(&c)
}

/// A tile's classes, `size` × `size` from its north-west corner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LandGrid {
    pub size: usize,
    pub classes: Vec<LandClass>,
}

/// The class at a tile's `x, y` (0 to 1, y down).
pub fn class_at(g: &LandGrid, x: f64, y: f64) -> LandClass {
    grid_value(g.size, &g.classes, x, y)
}

/// `values[min(size − 1, max(0, floor(y·size)))·size + min(size − 1, max(0, floor(x·size)))]`,
/// `classAt`'s and `canopy.ts` `heightAt`'s lookup. Where the TypeScript reads `undefined` (a NaN
/// place, an empty grid, one shorter than `size`²) this reads 0, which every caller takes the
/// same way (not wooded, no canopy).
pub(crate) fn grid_value(size: usize, values: &[u8], x: f64, y: f64) -> u8 {
    let s = size as f64;
    let at = |v: f64| js_min(s - 1.0, js_max(0.0, (v * s).floor()));
    let index = at(y) * s + at(x);
    if index.is_nan() || index < 0.0 {
        return 0;
    }
    values.get(index as usize).copied().unwrap_or(0)
}

/// `Math.min(a, b)`: NaN when either is, and -0 below +0 (`f64::min` drops a NaN instead).
pub(crate) fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b || (a == b && a.is_sign_negative()) {
        a
    } else {
        b
    }
}

/// `Math.max(a, b)`: NaN when either is, and +0 above -0 (`f64::max` drops a NaN instead).
pub(crate) fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b || (a == b && a.is_sign_positive()) {
        a
    } else {
        b
    }
}

/// NMD2023 (v2) colours (`0xRRGGBB`), as its legend gives them, to the map's classes, in the
/// TypeScript's order (`landcover.ts:48-64`; a colour named twice would take the later class).
const NMD: &[(LandClass, &[u32])] = &[
    // forest on firm ground and on wetland: pine, spruce, mixed conifers; conifers with broadleaves; broadleaves; felled
    (lc::CONIFERS, &[0x6E8C05, 0x2D5F00, 0x4E7000, 0x598C55, 0x305E50, 0x23735A]),
    (lc::MIXED, &[0x38A800, 0x438870]),
    (lc::BROADLEAVES, &[0x4CE600, 0xAAFF00, 0x97E600, 0x89CD9B, 0xA5F578, 0xABCD78, 0x00E6A9, 0x55FF00]),
    (lc::CLEARCUT, &[0xCDCD66, 0x898944]),
    // open wetland, of every kind
    (lc::WETLAND, &[0xC29ED7, 0x894465, 0xCD6699, 0xF57AB6, 0xD69DBC, 0x73004C, 0xA80084, 0xE600A9, 0xFF00C5]),
    (lc::WETLAND, &[0x704489, 0xAA66CD, 0xCA7AF5, 0x4C0073, 0x8400A8, 0xA900E6, 0xC500FF]),
    (lc::ARABLE, &[0xFFFFBE]),
    (lc::BARE, &[0xE1E1E1, 0x800080]),
    (lc::SHRUB, &[0xCCD79E, 0xABC9A6, 0x9EB591]),
    (lc::HEATH, &[0xD7C29E, 0xCDAA66, 0x897044]),
    (lc::GRASS, &[0xFFEBAF, 0xFFD37F, 0xFFBF7F]),
    (lc::BUILT, &[0x5A1414, 0xE5464B, 0x191919]),
    (lc::WATER, &[0x6699CD, 0x8ACCFA]),
];

/// [`NMD`] as a map, built as the TypeScript builds its record.
static NMD_CLASSES: LazyLock<HashMap<u32, LandClass>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    for &(class, colours) in NMD {
        for &colour in colours {
            map.insert(colour, class);
        }
    }
    map
});

/// WorldCover's codes to the map's classes.
fn world_cover_class(code: u8) -> LandClass {
    match code {
        10 => lc::FOREST,
        20 => lc::SHRUB,
        30 => lc::GRASS,
        40 => lc::ARABLE,
        50 => lc::BUILT,
        60 => lc::BARE,
        70 => lc::SNOW,
        80 => lc::WATER,
        90 => lc::WETLAND,
        95 => lc::BROADLEAVES,
        100 => lc::HEATH,
        _ => lc::NONE,
    }
}

/// An NMD picture's pixels (RGBA) to classes: none where it is clear or not one of its colours.
pub fn nmd_classes(rgba: &[u8]) -> Vec<LandClass> {
    rgba.chunks_exact(4)
        .map(|px| {
            if px[3] < 128 {
                return lc::NONE;
            }
            let colour = (u32::from(px[0]) << 16) | (u32::from(px[1]) << 8) | u32::from(px[2]);
            NMD_CLASSES.get(&colour).copied().unwrap_or(lc::NONE)
        })
        .collect()
}

/// A WorldCover picture's pixels (its grey the code) to classes.
pub fn world_cover_classes(rgba: &[u8]) -> Vec<LandClass> {
    rgba.chunks_exact(4).map(|px| if px[3] < 128 { lc::NONE } else { world_cover_class(px[0]) }).collect()
}

pub const LANDCOVER_ATTRIBUTION: &str = "Land cover: NMD2023 Naturvårdsverket · ESA WorldCover 2021";

/// Pixels across a tile read (and classes across a [`LandGrid`]).
pub const SIZE: usize = 256;
/// Half the web mercator world, metres.
const HALF_WORLD: f64 = 20_037_508.342789244;
/// Tiles kept by the caller's cache, the most recently used last; a failure is asked again once
/// forgotten.
pub const MAX_CACHED: usize = 96;

/// A tile's corners in web mercator metres: west, south, east, north.
pub fn bbox(x: u32, y: u32, z: u32) -> [f64; 4] {
    let span = (2.0 * HALF_WORLD) / 2f64.powi(z as i32);
    let west = -HALF_WORLD + f64::from(x) * span;
    let north = HALF_WORLD - f64::from(y) * span;
    [west, north - span, west + span, north]
}

/// Whether a tile's middle is in Sweden's box (NMD's).
pub fn in_sweden(x: u32, y: u32, z: u32) -> bool {
    let n = 2f64.powi(z as i32);
    let lon = ((f64::from(x) + 0.5) / n) * 360.0 - 180.0;
    let lat = ((std::f64::consts::PI * (1.0 - (2.0 * (f64::from(y) + 0.5)) / n)).sinh().atan() * 180.0)
        / std::f64::consts::PI;
    lon > 10.5 && lon < 24.5 && lat > 55.0 && lat < 69.2
}

const NMD_WMS: &str = "https://geodata.naturvardsverket.se/geoserver/wms";

/// NMD's picture of a tile: its WMS, [`SIZE`] pixels across, clear where it has nothing.
pub fn nmd_url(x: u32, y: u32, z: u32) -> String {
    let bbox = bbox(x, y, z).map(|v| to_fixed(v, 3)).join(",");
    format!(
        "{NMD_WMS}?service=WMS&version=1.3.0&request=GetMap&layers=lc-nmd:LC.LandCoverRaster.Bas.2023.v2.x&styles=&crs=EPSG:3857\
         &bbox={bbox}&width={SIZE}&height={SIZE}&format=image/png&transparent=true"
    )
}

const PC: &str = "https://planetarycomputer.microsoft.com/api/data/v1";
/// Where the WorldCover 2021 mosaic is registered: a POST of [`WORLD_COVER_SEARCH`] as JSON,
/// answered with its id ([`mosaic_id`]; a hash of the query, the same each time, so once is
/// enough; a failure is asked again later).
pub const WORLD_COVER_REGISTER: &str = "https://planetarycomputer.microsoft.com/api/data/v1/mosaic/register";
/// The body registered, as `JSON.stringify` writes it.
pub const WORLD_COVER_SEARCH: &str = r#"{"collections":["esa-worldcover"],"filter-lang":"cql2-json","filter":{"op":"anyinteracts","args":[{"property":"datetime"},{"interval":["2021-01-01T23:59:59Z","2021-12-31T23:59:59Z"]}]}}"#;
/// WorldCover's mosaic is drawn from this zoom.
pub const WORLD_COVER_MIN_ZOOM: u32 = 6;

/// The mosaic's id in the register's answer (`j?.searchid ?? null`, and an empty one is none).
pub fn mosaic_id(answer: &serde_json::Value) -> Option<&str> {
    answer.get("searchid").and_then(serde_json::Value::as_str).filter(|id| !id.is_empty())
}

/// WorldCover's picture of a tile in the mosaic `id`; none under [`WORLD_COVER_MIN_ZOOM`].
pub fn world_cover_url(id: &str, x: u32, y: u32, z: u32) -> Option<String> {
    if z < WORLD_COVER_MIN_ZOOM {
        return None;
    }
    Some(format!("{PC}/mosaic/{id}/tiles/WebMercatorQuad/{z}/{x}/{y}@1x?assets=map&collection=esa-worldcover&format=png"))
}

/// Whether WorldCover is wanted for a tile with these NMD classes: none (out of Sweden, or not to
/// be had), or past NMD's edge somewhere in it.
pub fn wants_world_cover(nmd: Option<&[LandClass]>) -> bool {
    nmd.is_none_or(|classes| classes.contains(&lc::NONE))
}

/// A tile's land cover from NMD's classes and WorldCover's, either missing: NMD's where it has
/// one, WorldCover's where it is clear; none when neither came.
pub fn combine(nmd: Option<Vec<LandClass>>, world: Option<Vec<LandClass>>) -> Option<LandGrid> {
    let classes = match (nmd, world) {
        (Some(mut classes), Some(world)) => {
            for (i, c) in classes.iter_mut().enumerate() {
                if *c == lc::NONE {
                    // (a store of `undefined` into a Uint8Array is 0)
                    *c = world.get(i).copied().unwrap_or(lc::NONE);
                }
            }
            Some(classes)
        }
        (None, world) => world,
        (nmd, None) => nmd,
    };
    classes.map(|classes| LandGrid { size: SIZE, classes })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `landcover.test.ts` `px`: pixels from RGBA quadruples.
    fn px(colours: &[[u8; 4]]) -> Vec<u8> {
        colours.concat()
    }

    #[test]
    fn nmds_colours_back_to_their_classes_pine_birch_a_mire_a_field_clear_and_unknown_are_none() {
        let c = nmd_classes(&px(&[
            [0x6e, 0x8c, 0x05, 255],
            [0x4c, 0xe6, 0x00, 255],
            [0xc2, 0x9e, 0xd7, 255],
            [0xff, 0xff, 0xbe, 255],
            [0x6e, 0x8c, 0x05, 0],
            [1, 2, 3, 255],
        ]));
        assert_eq!(c, [lc::CONIFERS, lc::BROADLEAVES, lc::WETLAND, lc::ARABLE, lc::NONE, lc::NONE]);
    }

    #[test]
    fn worldcovers_codes_its_grey() {
        let c = world_cover_classes(&px(&[[10, 10, 10, 255], [40, 40, 40, 255], [80, 80, 80, 255], [50, 50, 50, 0]]));
        assert_eq!(c, [lc::FOREST, lc::ARABLE, lc::WATER, lc::NONE]);
    }

    #[test]
    fn a_class_at_a_place_in_its_tile() {
        let g = LandGrid { size: 2, classes: vec![1, 2, 3, 4] };
        assert_eq!(class_at(&g, 0.1, 0.1), 1);
        assert_eq!(class_at(&g, 0.9, 0.9), 4);
        assert_eq!(class_at(&g, 1.0, 0.0), 2);
    }

    // (`the canopy's files by quadkey: Lund's` is in `canopy.rs`)

    #[test]
    fn a_towns_register_file_back_to_its_trees_in_a_tile() {
        use crate::street_trees::{parse_town, trees_in_tile};
        let mut buffer = vec![0u8; 8 + 20];
        buffer[..4].copy_from_slice(&[82, 83, 84, 49]);
        buffer[4..8].copy_from_slice(&2u32.to_le_bytes());
        // sorted by latitude: one in the tile at zoom 14 round Malmö, one north of it
        buffer[8..12].copy_from_slice(&13_000_000i32.to_le_bytes());
        buffer[12..16].copy_from_slice(&55_605_000i32.to_le_bytes());
        buffer[16] = 4;
        buffer[17] = 12;
        buffer[18..22].copy_from_slice(&13_000_000i32.to_le_bytes());
        buffer[22..26].copy_from_slice(&59_000_000i32.to_le_bytes());
        let town = parse_town(&buffer).unwrap();
        let found = trees_in_tile(&town, 8783, 5133, 14);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, 4);
        assert_eq!(found[0].crown, 6.0);
        assert!(found[0].x > 0.0);
        assert!(found[0].x < 1.0);
    }

    // (`land cover traced into rounded outlines: a wood and a field, towns and water left out` is in
    // `landcover_ground.rs`, beside `cover_outlines`)

    // New tests (no TypeScript twin).

    #[test]
    fn no_nmd_colour_is_named_twice_and_every_one_is_read_back() {
        let count: usize = NMD.iter().map(|(_, colours)| colours.len()).sum();
        assert_eq!(count, 51);
        assert_eq!(NMD_CLASSES.len(), count);
        for &(class, colours) in NMD {
            for &colour in colours {
                let [_, r, g, b] = colour.to_be_bytes();
                assert_eq!(nmd_classes(&[r, g, b, 128]), [class], "#{colour:06X}");
            }
        }
    }

    #[test]
    fn worldcovers_eleven_codes_and_nothing_else() {
        let known: Vec<(u8, u8)> = (0..=255).map(|code| (code, world_cover_class(code))).filter(|&(_, c)| c != lc::NONE).collect();
        assert_eq!(
            known,
            [
                (10, lc::FOREST),
                (20, lc::SHRUB),
                (30, lc::GRASS),
                (40, lc::ARABLE),
                (50, lc::BUILT),
                (60, lc::BARE),
                (70, lc::SNOW),
                (80, lc::WATER),
                (90, lc::WETLAND),
                (95, lc::BROADLEAVES),
                (100, lc::HEATH)
            ]
        );
        // only the red channel is the code
        assert_eq!(world_cover_classes(&[10, 0, 0, 200, 10, 0, 0, 127]), [lc::FOREST, lc::NONE]);
    }

    #[test]
    fn woods_are_the_four_tree_classes() {
        let woods: Vec<u8> = (0..=lc::SNOW).filter(|&c| wooded(c)).collect();
        assert_eq!(woods, [lc::CONIFERS, lc::MIXED, lc::BROADLEAVES, lc::FOREST]);
    }

    #[test]
    fn a_class_off_the_tile_is_its_edges_and_a_nan_place_is_none() {
        let g = LandGrid { size: 2, classes: vec![1, 2, 3, 4] };
        assert_eq!(class_at(&g, -3.0, 0.2), 1);
        assert_eq!(class_at(&g, 7.0, 7.0), 4);
        assert_eq!(class_at(&g, f64::NAN, 0.2), lc::NONE);
        assert_eq!(class_at(&LandGrid::default(), 0.5, 0.5), lc::NONE);
    }

    #[test]
    fn a_tiles_box_in_web_mercator_metres() {
        assert_eq!(bbox(0, 0, 0), [-HALF_WORLD, -HALF_WORLD, HALF_WORLD, HALF_WORLD]);
        let [w, s, e, n] = bbox(1, 0, 1);
        assert_eq!([w, s, e, n], [0.0, 0.0, HALF_WORLD, HALF_WORLD]);
    }

    #[test]
    fn nmds_url_names_the_tiles_box_to_the_millimetre() {
        // Lund's zoom-14 tile and a quarter of the world, as the TypeScript writes them
        assert_eq!(
            nmd_url(8792, 5125, 14),
            "https://geodata.naturvardsverket.se/geoserver/wms?service=WMS&version=1.3.0&request=GetMap\
             &layers=lc-nmd:LC.LandCoverRaster.Bas.2023.v2.x&styles=&crs=EPSG:3857\
             &bbox=1467590.943,7499389.719,1470036.928,7501835.704&width=256&height=256&format=image/png&transparent=true"
        );
        assert!(nmd_url(0, 1, 1).contains("&bbox=-20037508.343,-20037508.343,0.000,0.000&"));
        assert!(nmd_url(3, 1, 2).contains("&bbox=10018754.171,0.000,20037508.343,10018754.171&"));
    }

    #[test]
    fn sweden_by_its_tiles_middles() {
        // Lund and Kiruna in; Berlin south of the box, Bergen west of it
        assert!(in_sweden(8792, 5125, 14));
        assert!(in_sweden(569, 246, 10));
        assert!(!in_sweden(8801, 5374, 14));
        assert!(!in_sweden(527, 295, 10));
    }

    #[test]
    fn worldcovers_mosaic_its_id_and_tiles_from_zoom_6() {
        let search: serde_json::Value = serde_json::from_str(WORLD_COVER_SEARCH).unwrap();
        assert_eq!(search["collections"][0], "esa-worldcover");
        assert_eq!(search["filter"]["args"][1]["interval"][1], "2021-12-31T23:59:59Z");
        assert_eq!(mosaic_id(&serde_json::json!({"searchid": "abc"})), Some("abc"));
        assert_eq!(mosaic_id(&serde_json::json!({"searchid": ""})), None);
        assert_eq!(mosaic_id(&serde_json::json!(null)), None);
        assert_eq!(world_cover_url("abc", 1, 2, 5), None);
        assert_eq!(
            world_cover_url("abc", 35, 20, 6).as_deref(),
            Some(
                "https://planetarycomputer.microsoft.com/api/data/v1/mosaic/abc/tiles/WebMercatorQuad/6/35/20@1x\
                 ?assets=map&collection=esa-worldcover&format=png"
            )
        );
    }

    #[test]
    fn nmd_first_worldcover_where_it_is_clear() {
        assert!(wants_world_cover(None));
        assert!(wants_world_cover(Some(&[1, 0, 2])));
        assert!(!wants_world_cover(Some(&[1, 3, 2])));
        assert_eq!(combine(None, None), None);
        assert_eq!(combine(None, Some(vec![4, 4])), Some(LandGrid { size: SIZE, classes: vec![4, 4] }));
        assert_eq!(combine(Some(vec![0, 1]), None), Some(LandGrid { size: SIZE, classes: vec![0, 1] }));
        assert_eq!(combine(Some(vec![0, 1, 0]), Some(vec![13, 4])).unwrap().classes, [13, 1, 0]);
    }
}
