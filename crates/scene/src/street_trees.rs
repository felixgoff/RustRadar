//! Port of `street-trees.ts` (doc 04 §8.7): the trees towns keep registers of, in their streets
//! and parks: Malmö's and Umeå's (both CC0), each where it stands, of its kind, Malmö's with their
//! crowns' widths. `static/street-trees/*.bin`; see `scripts/street-trees.py` for the format:
//!
//! ```text
//! "RST1"            4 bytes magic
//! u32 n             little-endian
//! n records of 10 bytes, sorted by latitude:
//!   i32 lon * 1e6, i32 lat * 1e6, u8 kind, u8 crown diameter in half metres (0: not known)
//! ```
//!
//! The fetching of `streetTrees` (each town's file once, only when a tile meets its bounds)
//! belongs to the app: [`towns_for_tile`] says which files a tile needs, [`parse_town`] reads one
//! and [`trees_in_tile`] picks a tile's trees from it.

/// A registered tree in a tile (`RegisteredTree`): where (0 to 1 across it, y down), its kind
/// (Streets GL's tree, the format's: [`register_kinds`]), its crown's diameter (metres, 0 not
/// known).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegisteredTree {
    pub x: f64,
    pub y: f64,
    pub kind: u8,
    pub crown: f64,
}

/// The kinds, as the format has them (`REGISTER_KINDS`).
pub mod register_kinds {
    pub const BROADLEAF: u8 = 0;
    pub const CONIFER: u8 = 1;
    pub const BEECH: u8 = 2;
    pub const LINDEN: u8 = 3;
    pub const OAK: u8 = 4;
}

pub const STREET_TREES_ATTRIBUTION: &str = "Street trees: Malmö stad, Umeå kommun (CC0)";

/// A town with a register: its file (`static/street-trees/{file}.bin`) and round where its trees
/// are (west, south, east, north).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TownFile {
    pub file: &'static str,
    pub bounds: [f64; 4],
}

/// The towns (`TOWNS`).
pub const TOWNS: [TownFile; 2] = [
    TownFile { file: "malmo", bounds: [12.8, 55.45, 13.3, 55.7] },
    TownFile { file: "umea", bounds: [19.6, 63.5, 20.9, 64.2] },
];

/// A town's file read, tree by tree (sorted by latitude).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Town {
    /// Longitudes, millionths of a degree.
    pub lon: Vec<i32>,
    /// Latitudes, millionths of a degree, ascending.
    pub lat: Vec<i32>,
    pub kind: Vec<u8>,
    /// Crown diameters in half metres (0: not known).
    pub crown: Vec<u8>,
}

/// A town's file read (`parseTown`); `None` for anything that isn't one, or is cut short.
pub fn parse_town(bytes: &[u8]) -> Option<Town> {
    if bytes.len() < 8 || &bytes[0..4] != b"RST1" {
        return None;
    }
    let n = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if (bytes.len() as u64) < 8 + n as u64 * 10 {
        return None;
    }
    let mut town = Town { lon: vec![0; n], lat: vec![0; n], kind: vec![0; n], crown: vec![0; n] };
    for (i, record) in bytes[8..8 + n * 10].chunks_exact(10).enumerate() {
        town.lon[i] = i32::from_le_bytes([record[0], record[1], record[2], record[3]]);
        town.lat[i] = i32::from_le_bytes([record[4], record[5], record[6], record[7]]);
        town.kind[i] = record[8];
        town.crown[i] = record[9];
    }
    Some(town)
}

/// The first index at or past `v` in a sorted list.
fn lower_bound(list: &[i32], v: f64) -> usize {
    let (mut lo, mut hi) = (0, list.len());
    while lo < hi {
        let mid = (lo + hi) >> 1;
        if f64::from(list[mid]) < v {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// A tile's latitude at tile row `y` (of `n`), degrees.
fn lat_of(y: f64, n: f64) -> f64 {
    ((std::f64::consts::PI * (1.0 - (2.0 * y) / n)).sinh().atan() * 180.0) / std::f64::consts::PI
}

/// Web Mercator tile `z`/`x`/`y`'s `[west, south, east, north]`, degrees, as `street-trees.ts`
/// works them out.
fn tile_bounds(x: u32, y: u32, z: u32) -> [f64; 4] {
    let n = 2f64.powi(z as i32);
    let (x, y) = (f64::from(x), f64::from(y));
    let (north, south) = (lat_of(y, n), lat_of(y + 1.0, n));
    let (west, east) = ((x / n) * 360.0 - 180.0, ((x + 1.0) / n) * 360.0 - 180.0);
    [west, south, east, north]
}

/// A town's trees in tile `z`/`x`/`y` (`treesInTile`).
pub fn trees_in_tile(town: &Town, x: u32, y: u32, z: u32) -> Vec<RegisteredTree> {
    let n = 2f64.powi(z as i32);
    let [west, south, east, north] = tile_bounds(x, y, z);
    let (tx, ty) = (f64::from(x), f64::from(y));
    let mut out = Vec::new();
    let mut i = lower_bound(&town.lat, (south * 1e6).floor());
    while i < town.lat.len() && f64::from(town.lat[i]) <= north * 1e6 {
        let lon = f64::from(town.lon[i]) / 1e6;
        if !(lon < west || lon >= east) {
            let r = ((f64::from(town.lat[i]) / 1e6) * std::f64::consts::PI) / 180.0;
            let y = ((1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0) * n - ty;
            out.push(RegisteredTree { x: ((lon + 180.0) / 360.0) * n - tx, y, kind: town.kind[i], crown: f64::from(town.crown[i]) / 2.0 });
        }
        i += 1;
    }
    out
}

/// The towns whose registers tile `z`/`x`/`y` meets: the files `streetTrees` loads for it (none
/// where no town keeps a register). The tile's trees are [`trees_in_tile`] of each, in this order.
pub fn towns_for_tile(x: u32, y: u32, z: u32) -> impl Iterator<Item = &'static TownFile> {
    let [west, south, east, north] = tile_bounds(x, y, z);
    TOWNS.iter().filter(move |town| {
        let [w, s, e, nn] = town.bounds;
        !(east < w || west > e || north < s || south > nn)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;

    fn town(file: &str) -> Town {
        let path = format!("{}/../../static/street-trees/{file}.bin", env!("CARGO_MANIFEST_DIR"));
        parse_town(&std::fs::read(&path).expect("a street-trees file")).expect("a town")
    }

    /// The zoom-`z` tile under a place, as the TypeScript's tests would find it.
    fn tile_at(lon: f64, lat: f64, z: u32) -> (u32, u32) {
        let n = 2f64.powi(z as i32);
        let r = lat * std::f64::consts::PI / 180.0;
        let x = (((lon + 180.0) / 360.0) * n).floor();
        let y = (((1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0) * n).floor();
        (x as u32, y as u32)
    }

    #[test]
    fn the_shipped_registers_parse_sorted_by_latitude_within_their_towns() {
        for (file, count) in [("malmo", 93_046), ("umea", 31_760)] {
            let t = town(file);
            assert_eq!(t.lat.len(), count);
            assert!(t.lat.windows(2).all(|w| w[0] <= w[1]), "{file} is sorted by latitude");
            assert!(t.kind.iter().all(|&k| k <= register_kinds::OAK));
            let [w, s, e, n] = TOWNS.iter().find(|town| town.file == file).unwrap().bounds;
            for i in 0..count {
                let (lon, lat) = (f64::from(t.lon[i]) / 1e6, f64::from(t.lat[i]) / 1e6);
                assert!(lon >= w && lon <= e && lat >= s && lat <= n, "{file} tree {i} at {lon}, {lat}");
            }
        }
        // only Malmö's register has the crowns
        assert!(town("malmo").crown.iter().any(|&c| c > 0));
        assert!(town("umea").crown.iter().all(|&c| c == 0));
    }

    /// A tree as the TypeScript placed it: `x` is plain arithmetic and exact; `y` goes through
    /// `tan`, `cos` and `log`, where V8's and the system's libm may differ in the last bit.
    fn same(tree: RegisteredTree, (x, y, kind, crown): (f64, f64, u8, f64)) {
        assert_eq!(tree.x, x);
        assert_close!(tree.y, y, 12);
        assert_eq!((tree.kind, tree.crown), (kind, crown));
    }

    #[test]
    fn a_tiles_trees_are_the_typescripts() {
        // treesInTile's answers for the same files and tiles, printed by bun
        let malmo = town("malmo");
        assert_eq!(tile_at(13.0, 55.6, 15), (17_567, 10_268));
        let trees = trees_in_tile(&malmo, 17_567, 10_268, 15);
        assert_eq!(trees.len(), 707);
        same(trees[0], (0.4246030222202535, 0.9958546160596597, 0, 16.0));
        same(trees[1], (0.42260053333302494, 0.9898939740742208, 0, 12.5));
        same(trees[2], (0.6686336000020674, 0.9898939740742208, 4, 18.5));
        same(trees[706], (0.924770133333368, 0.0024440899014734896, 0, 0.0));
        let mut kinds = [0; 5];
        for t in &trees {
            kinds[t.kind as usize] += 1;
        }
        assert_eq!(kinds, [479, 84, 5, 88, 51]);
        assert_eq!(trees.iter().map(|t| t.crown).sum::<f64>(), 3624.0);

        let umea = town("umea");
        assert_eq!(tile_at(20.26, 63.825, 15), (18_228, 8_775));
        let trees = trees_in_tile(&umea, 18_228, 8_775, 15);
        assert_eq!(trees.len(), 647);
        same(trees[0], (0.22199751111111254, 0.9974525001271104, 0, 0.0));
        same(trees[646], (0.254037333335873, 0.0008462521182082128, 0, 0.0));

        assert_eq!(trees_in_tile(&malmo, 2195, 1283, 12).len(), 22_055);
        // none in a tile of the other town
        assert!(trees_in_tile(&malmo, 18_228, 8_775, 15).is_empty());
    }

    #[test]
    fn a_tile_loads_only_the_towns_it_meets() {
        let files = |(x, y): (u32, u32), z| towns_for_tile(x, y, z).map(|t| t.file).collect::<Vec<_>>();
        assert_eq!(files(tile_at(13.0, 55.6, 15), 15), ["malmo"]);
        assert_eq!(files(tile_at(20.26, 63.825, 15), 15), ["umea"]);
        assert!(files(tile_at(18.07, 59.33, 15), 15).is_empty()); // Stockholm keeps none
        // zoomed out, one tile meets both
        assert_eq!(files((0, 0), 0), ["malmo", "umea"]);
    }

    #[test]
    fn anything_that_isnt_a_register_is_refused() {
        assert_eq!(parse_town(b"RST1\x01\0\0"), None);
        assert_eq!(parse_town(b"RST2\0\0\0\0"), None);
        // one record promised, nine bytes given
        assert_eq!(parse_town(b"RST1\x01\0\0\x00123456789"), None);
        assert_eq!(parse_town(b"RST1\0\0\0\0"), Some(Town::default()));
        // one tree at 13.000001 E, 55.5 N, a linden of 7.5 m; trailing bytes are ignored
        let mut bytes = b"RST1\x01\0\0\0".to_vec();
        bytes.extend(13_000_001_i32.to_le_bytes());
        bytes.extend(55_500_000_i32.to_le_bytes());
        bytes.extend([register_kinds::LINDEN, 15, 99]);
        let t = parse_town(&bytes).unwrap();
        assert_eq!(t, Town { lon: vec![13_000_001], lat: vec![55_500_000], kind: vec![3], crown: vec![15] });
        let (x, y) = tile_at(13.000001, 55.5, 16);
        let trees = trees_in_tile(&t, x, y, 16);
        assert_eq!(trees.len(), 1);
        assert_eq!((trees[0].kind, trees[0].crown), (register_kinds::LINDEN, 7.5));
    }
}
