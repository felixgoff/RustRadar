//! Radar meshing (S1.27; doc 02 §3): port of `radar.ts`, with `radar-mesh.ts` in [`mesh`] and
//! [`iso`], and `radar-arena.ts` in [`arena`].
//!
//! Where NOAA MRMS or DWD measure reflectivity aloft, the backend's volumes are the real thing;
//! everywhere else RainViewer tiles are split into stacked layers by estimated echo-top height.
//! All of it ends up as smooth filled polygons, one per 5 dBZ band in the NWS palette.
//!
//! RainViewer's public API serves only the "Universal Blue" palette, so reflectivity is read back
//! from the colour ([`reflectivity`]). [`process_tile`] turns a tile's RGBA pixels into meshes;
//! [`process_volume`] does the same for a measured volume. Both run on worker threads in the
//! native app; `fetchPixels` and the worker protocol (`radar-pool.ts`, `radar-worker.ts`) belong
//! to the map's tile runtime and the backend, not here.
//!
//! Deviations: [`process_volume`] reads a level whose blob is missing as no echo (the
//! TypeScript would throw), and returns [`mesh::Truncated`] where it would throw on a truncated
//! blob.

pub mod arena;
pub mod iso;
pub mod mesh;

use rustradar_core::model::{RadarVolume, f32_as_js};
use rustradar_core::num::js_string;

use crate::geom::{js_max, js_min};

pub use mesh::{BAND_COUNT, BandPolygon, PALETTE, RadarMesh, WallLine, band_color, band_dbz};

/// Heights (km) of the stacked layers above the ground layer.
pub const RADAR_LEVELS_KM: [f64; 7] = [1.5, 3.0, 4.5, 6.0, 8.0, 10.0, 12.0];

/// Approximate reflectivity (dBZ) of a Universal Blue pixel; `-inf` for a transparent one.
pub fn reflectivity(r: u8, g: u8, b: u8, a: u8) -> f64 {
    let (r, g, b, a) = (f64::from(r), f64::from(g), f64::from(b), f64::from(a));
    if a == 0.0 {
        return f64::NEG_INFINITY;
    }
    if a < 250.0 {
        return (a / 255.0) * 10.0; // drizzle: beige, more opaque as it strengthens
    }
    if b > g && r < 160.0 {
        return 10.0 + (25.0 * (595.0 - (r + g + b))) / 420.0; // blues: light 10 → dark 35
    }
    if r > 240.0 && b < 100.0 {
        return 40.0 + (15.0 * (224.0 - g)) / 224.0; // yellow 40 → red 55
    }
    60.0 // pinks and white
}

/// Echo top (km) for a reflectivity: a rough climatological fit, from about 2 km in drizzle to
/// 12 km in severe storms; 0 for no echo.
pub fn echo_top_km(dbz: f64) -> f64 {
    if dbz == f64::NEG_INFINITY { 0.0 } else { js_min(12.0, js_max(1.0, 2.0 + (dbz - 10.0) * 0.22)) }
}

/// The weakest band that still reaches `km` (`lowestBandAt`): higher layers keep only the
/// stronger echoes. [`BAND_COUNT`] when none does.
pub fn lowest_band_at(km: f64) -> usize {
    let mut band = 0;
    while band < BAND_COUNT && echo_top_km(band_dbz(band)) < km {
        band += 1;
    }
    band
}

/// One lifted layer of a tile: its height and the weakest band drawn there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileLevel {
    pub km: f64,
    pub from_band: usize,
}

/// A RainViewer tile's meshes (`TileMesh`).
#[derive(Debug, Clone, PartialEq)]
pub struct TileMesh {
    /// `z/x/y`: stable between frames.
    pub id: String,
    /// West, south, east, north in degrees.
    pub bounds: [f64; 4],
    /// Every band, flat; drawn on the ground and, from each level's band up, lifted.
    pub slices: RadarMesh,
    /// The bands' outlines as walls of unit height ([`mesh::build_walls`]); `None` when there
    /// are none.
    pub walls: Option<RadarMesh>,
    /// Lifted layers, each with only the bands whose echo tops reach it: those from `from_band`
    /// up, of `slices` and `walls`. Contiguous from the lowest height, so each one's walls run
    /// down to the one before.
    pub levels: Vec<TileLevel>,
}

/// Where a measured volume (MRMS, DWD) has data: RainViewer is hidden there so the two don't
/// overlap. One byte per cell, north row first, 1 = covered.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Coverage {
    /// Changes only when the mask or its bounds do: a new scan with the same coverage keeps it.
    pub key: String,
    /// West, south, east, north in degrees.
    pub bounds: [f64; 4],
    pub width: usize,
    pub height: usize,
    pub mask: Vec<u8>,
}

impl Coverage {
    /// Whether the cell under `(lon, lat)` is covered; false outside the bounds.
    pub fn covers(&self, lon: f64, lat: f64) -> bool {
        let [west, south, east, north] = self.bounds;
        if lon < west || lon > east || lat < south || lat > north {
            return false;
        }
        let (w, h) = (self.width as f64, self.height as f64);
        // clamped, so a point exactly on the east or south edge still lands in the last cell
        // rather than falling through and leaving a seam
        let x = js_min(w - 1.0, ((lon - west) / (east - west) * w).floor());
        let y = js_min(h - 1.0, ((north - lat) / (north - south) * h).floor());
        // NaN (bounds of no extent) or a cell off the mask reads as `undefined`: not covered
        let i = y * w + x;
        i >= 0.0 && self.mask.get(i as usize) == Some(&1)
    }
}

/// Where MRMS or DWD has data: any of the coverages (`Covered`).
#[derive(Debug, Clone, Copy)]
pub struct Covered<'a> {
    coverages: &'a [Coverage],
}

impl Covered<'_> {
    /// Whether any coverage covers `(lon, lat)`.
    pub fn covers(&self, lon: f64, lat: f64) -> bool {
        self.coverages.iter().any(|c| c.covers(lon, lat))
    }
}

/// The test for the coverages, or `None` when there are none (`coveredBy`).
pub fn covered_by(coverages: &[Coverage]) -> Option<Covered<'_>> {
    if coverages.is_empty() { None } else { Some(Covered { coverages }) }
}

/// Whether two west, south, east, north boxes touch or overlap.
fn intersects(a: &[f64; 4], b: &[f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/// The coverages that reach into a tile: only they change what it shows (`coveragesOver`).
pub fn coverages_over<'a>(bounds: &[f64; 4], coverages: &'a [Coverage]) -> Vec<&'a Coverage> {
    coverages.iter().filter(|c| intersects(&c.bounds, bounds)).collect()
}

/// A Web Mercator tile's index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileIndex {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

/// The latitude of the top of tile row `y` (fractional rows allowed) at zoom `z`.
pub fn tile_latitude(y: f64, z: u32) -> f64 {
    (std::f64::consts::PI * (1.0 - (2.0 * y) / 2f64.powi(z as i32))).sinh().atan() * 180.0 / std::f64::consts::PI
}

/// West, south, east, north of a Web Mercator tile, in degrees (`tileBounds`).
pub fn tile_bounds(index: TileIndex) -> [f64; 4] {
    let n = 2f64.powi(index.z as i32);
    let (x, y) = (f64::from(index.x), f64::from(index.y));
    [(x / n) * 360.0 - 180.0, tile_latitude(y + 1.0, index.z), ((x + 1.0) / n) * 360.0 - 180.0, tile_latitude(y, index.z)]
}

/// The tile's pixels (`width` × `height` RGBA, Universal Blue, row 0 at the top) as meshes;
/// `None` when it has no echo (`processTile`). `covered` blanks out what the measured volumes
/// draw themselves. Contoured in the tile's own (Mercator) grid, then placed on the globe: every
/// vertex lies within [`tile_bounds`], and neighbouring tiles' pieces meet on the tile edge.
pub fn process_tile(pixels: &[u8], width: usize, height: usize, index: TileIndex, covered: Option<Covered<'_>>) -> Option<TileMesh> {
    let bounds = tile_bounds(index);
    let [west, _, east, _] = bounds;

    // the colours back to reflectivity, with no echo as 0 dBZ (below the first band) so the
    // blur fades edges out rather than pulling them in hard
    let mut dbz = vec![0.0_f32; width * height];
    let mut echo = false;
    for r in 0..height {
        // rows are Web Mercator: even steps in y, not in latitude
        let lat = if covered.is_some() { tile_latitude(f64::from(index.y) + (r as f64 + 0.5) / height as f64, index.z) } else { 0.0 };
        for c in 0..width {
            let i = (r * width + c) * 4;
            if pixels[i + 3] == 0 {
                continue;
            }
            // blank out what MRMS and DWD cover; they draw it themselves
            if covered.is_some_and(|cv| cv.covers(west + ((c as f64 + 0.5) / width as f64) * (east - west), lat)) {
                continue;
            }
            let v = 0.0_f64.max(reflectivity(pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]));
            dbz[r * width + c] = v as f32;
            if v > 0.0 {
                echo = true;
            }
        }
    }
    // most of the world is dry: nothing to contour
    if !echo {
        return None;
    }

    // contoured in the tile's own (Mercator) grid, then placed on the globe
    let mut lines: Vec<WallLine> = Vec::new();
    let mut polygons = mesh::contour_bands(&mesh::blur(&dbz, width, height), width, height, Some(&mut lines));
    if polygons.is_empty() {
        return None;
    }
    let place = |p: &mut Vec<f64>| {
        for i in (0..p.len() / 2 * 2).step_by(2) {
            p[i] = west + p[i] * (east - west);
            p[i + 1] = tile_latitude(f64::from(index.y) + p[i + 1], index.z);
        }
    };
    polygons.iter_mut().for_each(|p| place(&mut p.positions));
    lines.iter_mut().for_each(|l| place(&mut l.positions));
    let slices = mesh::build_mesh(&polygons);
    if slices.indices.is_empty() {
        return None;
    }
    let walls = if lines.is_empty() { None } else { Some(mesh::build_walls(&lines)) };
    let mut levels = Vec::new();
    for km in RADAR_LEVELS_KM {
        let from_band = lowest_band_at(km);
        if slices.band_starts[from_band] as usize >= slices.indices.len() {
            break; // higher ones are emptier still
        }
        levels.push(TileLevel { km, from_band });
    }
    Some(TileMesh {
        id: format!("{}/{}/{}", index.z, index.x, index.y),
        bounds,
        slices,
        walls: walls.filter(|w| !w.indices.is_empty()),
        levels,
    })
}

/// One height of a measured volume.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeLevel {
    pub km: f64,
    pub slices: RadarMesh,
    pub walls: Option<RadarMesh>,
}

/// A measured volume's meshes (`VolumeMesh`).
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeMesh {
    pub ground: Option<RadarMesh>,
    pub levels: Vec<VolumeLevel>,
    pub coverage: Coverage,
}

/// Triangulates a measured volume from the backend's blobs (`processVolume`): `ground`, the
/// coverage mask and one per height in `frame.levels`, in that order. A missing level blob is no
/// echo.
pub fn process_volume(frame: &RadarVolume, ground: &[u8], coverage: Vec<u8>, levels: &[&[u8]]) -> Result<VolumeMesh, mesh::Truncated> {
    let mut parsed = Vec::with_capacity(frame.levels.len());
    for i in 0..frame.levels.len() {
        parsed.push(mesh::parse_volume_polygons(levels.get(i).copied().unwrap_or(&[]))?);
    }
    let ground = mesh::parse_volume_polygons(ground)?;
    Ok(process_volume_polygons(frame, &ground, coverage, &parsed))
}

/// [`process_volume`] for polygons already parsed (or handed over by the backend as they are):
/// one list per height in `frame.levels`; a missing one is no echo.
pub fn process_volume_polygons(frame: &RadarVolume, ground: &[BandPolygon], coverage: Vec<u8>, levels: &[Vec<BandPolygon>]) -> VolumeMesh {
    let non_empty = |mesh: RadarMesh| if mesh.indices.is_empty() { None } else { Some(mesh) };
    let mut meshes = Vec::new();
    for (i, &km) in frame.levels.iter().enumerate() {
        let polygons = levels.get(i).map(Vec::as_slice).unwrap_or(&[]);
        let slices = mesh::build_mesh(polygons);
        if slices.indices.is_empty() {
            continue;
        }
        let walls = non_empty(mesh::build_walls(&mesh::polygon_walls(polygons)));
        meshes.push(VolumeLevel { km: f32_as_js(km), slices, walls });
    }
    let b = frame.bounds;
    VolumeMesh {
        ground: non_empty(mesh::build_mesh(ground)),
        levels: meshes,
        coverage: Coverage {
            key: format!(
                "{}:{},{},{},{}:{}x{}:{}",
                frame.source,
                js_string(b.west),
                js_string(b.south),
                js_string(b.east),
                js_string(b.north),
                frame.coverage_width,
                frame.coverage_height,
                hash(&coverage)
            ),
            bounds: [b.west, b.south, b.east, b.north],
            width: frame.coverage_width as usize,
            height: frame.coverage_height as usize,
            mask: coverage,
        },
    }
}

/// FNV-1a in base 36, enough to tell one coverage mask from the next.
fn hash(bytes: &[u8]) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for &b in bytes {
        h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    radix36(h)
}

/// `n.toString(36)`.
fn radix36(mut n: u32) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;
    use rustradar_core::model::RadarBounds;

    /// RainViewer's "Universal Blue" rain colours (`/2/` in the tile URL), RGBA, for -10 to 74
    /// dBZ in steps of 1: the rain column of `rainviewer_api_colors_table.csv` (below -10 dBZ
    /// the table is transparent).
    const UNIVERSAL_BLUE: [u32; 85] = [
        0x63615914, 0x66635a19, 0x69665c1e, 0x6c685d24, 0x6f6b5f29, 0x726e612e, 0x75706234, 0x78736439, //
        0x7c75653e, 0x7f786744, 0x827b6949, 0x857d6a4e, 0x88806c54, 0x8b826d59, 0x8e856f5e, 0x92887164, //
        0x9e93756e, 0xaa9e7978, 0xb6a97e82, 0xc2b4828c, 0xcec08796, 0xd2c48ba0, 0xd6c88faa, 0xdacc93b4, //
        0xded097be, 0x88ddeeff, 0x6cd1ebff, 0x51c5e8ff, 0x36bae5ff, 0x1baee2ff, 0x00a3e0ff, 0x009ad5ff, //
        0x0091caff, 0x0088bfff, 0x007fb4ff, 0x0077aaff, 0x0070a3ff, 0x00699cff, 0x006295ff, 0x005b8eff, //
        0x005588ff, 0x005180ff, 0x004e78ff, 0x004a70ff, 0x004768ff, 0xffee00ff, 0xffe000ff, 0xffd200ff, //
        0xffc500ff, 0xffb700ff, 0xffaa00ff, 0xff9f00ff, 0xff9500ff, 0xff8b00ff, 0xff8100ff, 0xff4400ff, //
        0xf23600ff, 0xe62800ff, 0xd91b00ff, 0xcd0d00ff, 0xc10000ff, 0xa80000ff, 0x8f0000ff, 0x760000ff, //
        0x5d0000ff, 0xffaaffff, 0xff9fffff, 0xff95ffff, 0xff8bffff, 0xff81ffff, 0xff77ffff, 0xff6cffff, //
        0xff62ffff, 0xff58ffff, 0xff4effff, 0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, //
        0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff,
    ];

    fn rgba(c: u32) -> [u8; 4] {
        c.to_be_bytes()
    }

    fn reflectivity_of(c: u32) -> f64 {
        let [r, g, b, a] = rgba(c);
        reflectivity(r, g, b, a)
    }

    #[test]
    fn reflectivity_inverts_the_palette() {
        // nothing where the tile is transparent
        assert_eq!(reflectivity(0, 0, 0, 0), f64::NEG_INFINITY);
        assert_eq!(reflectivity(255, 255, 255, 0), f64::NEG_INFINITY);
        // stronger rain never reads weaker: the three ramps join up in order
        let read: Vec<f64> = UNIVERSAL_BLUE.iter().map(|&c| reflectivity_of(c)).collect();
        for (i, pair) in read.windows(2).enumerate() {
            assert!(pair[0] <= pair[1], "{} dBZ reads {}, {} dBZ reads {}", i as i32 - 10, pair[0], i as i32 - 9, pair[1]);
        }
        let at = |dbz: i32| read[(dbz + 10) as usize];
        // drizzle, by its opacity: below 10 and so mostly under the first band's 5 dBZ
        for dbz in -10..15 {
            assert!((0.0..10.0).contains(&at(dbz)), "{dbz}: {}", at(dbz));
        }
        // blues: light 10 to dark 35
        for dbz in 15..35 {
            assert!((10.0..=35.0).contains(&at(dbz)), "{dbz}: {}", at(dbz));
        }
        // yellow 40 to red 55; the darker reds, pinks and white read as hail
        for dbz in 35..47 {
            assert!((39.0..=55.0).contains(&at(dbz)), "{dbz}: {}", at(dbz));
        }
        for dbz in 47..75 {
            assert_eq!(at(dbz), 60.0, "{dbz}");
        }
        // the values the TypeScript gives, so a change to the inversion shows here
        for (dbz, expected) in [
            (-10, 0.7843137254901961),
            (0, 2.8627450980392153),
            (14, 7.450980392156863),
            (15, 10.0),
            (20, 22.38095238095238),
            (34, 35.0),
            (35, 39.0625),
            (40, 43.61607142857143),
            (46, 51.38392857142857),
            (47, 60.0),
            (55, 60.0),
            (65, 60.0),
        ] {
            assert_eq!(at(dbz), expected, "{dbz}");
        }
        // snow (shown in the tiles too): blue-white ramps the formula was not made for
        for (c, expected) in [
            (0xcfffff00, f64::NEG_INFINITY),
            (0xc7ffff7f, 4.980392156862745),
            (0xc3ffffbf, 7.490196078431373),
            (0xbfffffff, 60.0),
            (0x4f8fffff, 17.023809523809526),
            (0x0f4fffff, 24.642857142857142),
        ] {
            assert_eq!(reflectivity_of(c), expected, "{c:08x}");
        }
        // the 80 dBZ green of the rain table is hail too
        assert_eq!(reflectivity(0, 255, 0, 255), 60.0);
    }

    #[test]
    fn echo_tops_and_the_bands_each_level_keeps() {
        assert_eq!(echo_top_km(f64::NEG_INFINITY), 0.0);
        assert_eq!(echo_top_km(band_dbz(0)), 1.0); // 0.9, clamped
        assert_close!(echo_top_km(band_dbz(1)), 2.0, 12);
        assert_close!(echo_top_km(band_dbz(2)), 3.1, 12);
        assert_eq!(echo_top_km(70.0), 12.0); // 15.2, clamped
        // doc 02 §3.2: the weakest band drawn at each height
        let from: Vec<usize> = RADAR_LEVELS_KM.iter().map(|&km| lowest_band_at(km)).collect();
        assert_eq!(from, [1, 2, 4, 5, 7, 9, 11]);
        assert_eq!(lowest_band_at(13.0), BAND_COUNT);
        assert_eq!(lowest_band_at(0.0), 0);
    }

    #[test]
    fn tile_bounds_are_web_mercator() {
        let world = tile_bounds(TileIndex { x: 0, y: 0, z: 0 });
        assert_eq!(world, [-180.0, -85.05112877980659, 180.0, 85.05112877980659]);
        let a = tile_bounds(TileIndex { x: 10, y: 20, z: 6 });
        let b = tile_bounds(TileIndex { x: 11, y: 21, z: 6 });
        assert_eq!(a[0], -123.75);
        assert_eq!(a[2], b[0]);
        assert_eq!(a[1], b[3]);
        assert_close!(a[1], 52.48278022207821, 12);
        assert_close!(a[3], 55.77657301866768, 12);
        assert_close!(b[1], 48.92249926375824, 12);
    }

    fn coverage(bounds: [f64; 4], width: usize, height: usize, mask: Vec<u8>) -> Coverage {
        Coverage { key: "k".into(), bounds, width, height, mask }
    }

    #[test]
    fn coverage_looks_up_the_cell_under_a_point() {
        // 2 x 2 over [0, 10] x [0, 10], north row first: only the north-east cell covered
        let c = coverage([0.0, 0.0, 10.0, 10.0], 2, 2, vec![0, 1, 0, 0]);
        assert!(c.covers(7.0, 7.0));
        assert!(!c.covers(3.0, 7.0));
        assert!(!c.covers(7.0, 3.0));
        // the east and north edges land in the last cell, the outside in none
        assert!(c.covers(10.0, 10.0));
        assert!(c.covers(10.0, 6.0));
        // the middle is the south row's top
        assert!(!c.covers(10.0, 5.0));
        assert!(!c.covers(10.5, 7.0));
        assert!(!c.covers(7.0, -0.1));
        // a south-west cell covered: the south edge is in the last row
        let d = coverage([0.0, 0.0, 10.0, 10.0], 2, 2, vec![0, 0, 1, 0]);
        assert!(d.covers(0.0, 0.0));
        // bounds of no extent cover nothing
        assert!(!coverage([5.0, 5.0, 5.0, 5.0], 1, 1, vec![1]).covers(5.0, 5.0));

        assert!(covered_by(&[]).is_none());
        let both = [c.clone(), coverage([20.0, 0.0, 30.0, 10.0], 1, 1, vec![1])];
        let covered = covered_by(&both).unwrap();
        assert!(covered.covers(7.0, 7.0) && covered.covers(25.0, 1.0));
        assert!(!covered.covers(15.0, 5.0));
        // touching counts
        let over = coverages_over(&[10.0, 10.0, 12.0, 12.0], &both);
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].bounds, c.bounds);
        assert!(coverages_over(&[11.0, 0.0, 19.0, 9.0], &both).is_empty());
    }

    #[test]
    fn coverage_keys_hash_the_mask() {
        let frame = RadarVolume {
            source: "dwd".into(),
            time: 1,
            bounds: RadarBounds { west: 1.5, south: 45.25, east: 18.000001, north: 56.0 },
            levels: vec![1.0, 4.5],
            coverage_width: 3,
            coverage_height: 2,
        };
        // as the TypeScript writes them
        for (mask, key) in [
            (vec![], "dwd:1.5,45.25,18.000001,56:3x2:ztntfp"),
            (vec![1, 0, 1, 1, 0, 0], "dwd:1.5,45.25,18.000001,56:3x2:156cqju"),
            (vec![255, 255, 255, 255], "dwd:1.5,45.25,18.000001,56:3x2:1r0atgx"),
        ] {
            let volume = process_volume(&frame, &[], mask.clone(), &[&[], &[]]).unwrap();
            assert_eq!(volume.coverage.key, key);
            assert_eq!(volume.coverage.mask, mask);
            assert_eq!(volume.coverage.bounds, [1.5, 45.25, 18.000001, 56.0]);
            assert!(volume.ground.is_none() && volume.levels.is_empty());
        }
        assert_eq!(radix36(0), "0");
        assert_eq!(radix36(u32::MAX), "1z141z3");
    }

    #[test]
    fn a_volume_meshes_its_ground_and_levels() {
        // two bands of squares; the level at 4.5 km has nothing; a truncated blob is an error
        let blob = |polygons: &[(u8, &[f32])]| {
            let mut out = (polygons.len() as u32).to_le_bytes().to_vec();
            for (band, ring) in polygons {
                out.extend_from_slice(&[*band, 0, 0, 0]);
                out.extend_from_slice(&1u32.to_le_bytes());
                out.extend_from_slice(&((ring.len() / 2) as u32).to_le_bytes());
                ring.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
            }
            out
        };
        let square: &[f32] = &[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0];
        let inner: &[f32] = &[0.25, 0.25, 0.75, 0.25, 0.75, 0.75, 0.25, 0.75];
        let low = blob(&[(1, square), (4, inner)]);
        let frame = RadarVolume { source: "mrms".into(), levels: vec![1.5, 4.5, 3.0], coverage_width: 1, coverage_height: 1, ..Default::default() };
        let volume = process_volume(&frame, &low, vec![1], &[&low, &blob(&[])]).unwrap();
        assert_eq!(volume.ground.as_ref().map(|g| g.indices.len()), Some(12));
        // the 4.5 km blob is empty and the 3 km one missing: no echo
        assert_eq!(volume.levels.len(), 1);
        let level = &volume.levels[0];
        assert_eq!(level.km, 1.5);
        assert_eq!(level.slices.band_starts[2], 6);
        // two outer rings, so two closed walls of four segments each
        let walls = level.walls.as_ref().unwrap();
        assert_eq!(walls.vertex_count(), 2 * 2 * 5);
        assert_eq!(walls.indices.len(), 2 * 4 * 6);
        let mut cut = low.clone();
        cut.truncate(cut.len() - 3);
        assert_eq!(process_volume(&frame, &[], vec![], &[&cut]).unwrap_err(), mesh::Truncated { offset: 60, length: cut.len() });
    }

    /// The Universal Blue colour of `40 + 15·m/32` dBZ (`m` from 0 to 32): the yellow-to-red
    /// ramp, whose reflectivities are binary fractions, so blurring sums them exactly.
    fn ramp(m: i64) -> [u8; 4] {
        [255, (224 - 7 * m.clamp(0, 32)) as u8, 0, 255]
    }

    /// A storm round the corner where four tiles of `n` × `n` pixels meet, mirrored across both
    /// seams: global pixel (`X`, `Y`) of the 2n × 2n block.
    fn storm(n: usize, x: usize, y: usize) -> [u8; 4] {
        let (dx, dy) = (2 * x as i64 - (2 * n as i64 - 1), 2 * y as i64 - (2 * n as i64 - 1));
        let e = 2 * dx * dx + 3 * dy * dy;
        let s = (n * n) as i64;
        if e < s / 2 {
            [255, 255, 255, 255] // hail
        } else if e < 4 * s {
            ramp(32 - (e - s / 2) * 33 / (4 * s - s / 2))
        } else {
            [0, 0, 0, 0]
        }
    }

    /// The pixels of tile (`col`, `row`) of the 2 × 2 block.
    fn storm_tile(n: usize, col: usize, row: usize) -> Vec<u8> {
        let mut px = Vec::with_capacity(n * n * 4);
        for y in 0..n {
            for x in 0..n {
                px.extend_from_slice(&storm(n, col * n + x, row * n + y));
            }
        }
        px
    }

    /// Per band, the sorted bits of the other coordinate of the slices' vertices that lie on a
    /// line: longitude `at` (`lon` true) or latitude `at`.
    fn on_line(mesh: &RadarMesh, lon: bool, at: f32) -> Vec<Vec<u32>> {
        (0..BAND_COUNT)
            .map(|b| {
                let (from, to) = (mesh.band_vertex_starts[b] as usize, mesh.band_vertex_starts[b + 1] as usize);
                let mut out: Vec<u32> = (from..to)
                    .map(|v| (mesh.positions[v * 2], mesh.positions[v * 2 + 1]))
                    .filter(|&(x, y)| if lon { x == at } else { y == at })
                    .map(|(x, y)| if lon { y.to_bits() } else { x.to_bits() })
                    .collect();
                out.sort_unstable();
                out.dedup();
                out
            })
            .collect()
    }

    #[test]
    fn two_neighbouring_synthetic_tiles_contour_to_edges_that_meet_exactly() {
        let n = 32;
        let index = |col: u32, row: u32| TileIndex { x: 10 + col, y: 20 + row, z: 6 };
        let tile = |col: usize, row: usize| process_tile(&storm_tile(n, col, row), n, n, index(col as u32, row as u32), None).unwrap();
        let (a, b, c) = (tile(0, 0), tile(1, 0), tile(0, 1));
        assert_eq!(a.id, "6/10/20");
        // every vertex inside its own tile: the padding was clipped away
        for t in [&a, &b, &c] {
            let [w, s, e, n] = t.bounds.map(|v| v as f32);
            for p in t.slices.positions.chunks_exact(2) {
                assert!(p[0] >= w && p[0] <= e && p[1] >= s && p[1] <= n, "{p:?} outside {:?}", t.bounds);
            }
        }
        // the east edge of `a` and the west edge of `b`: the same meridian, the same points
        let seam = a.bounds[2] as f32;
        assert_eq!(seam, b.bounds[0] as f32);
        let (east, west) = (on_line(&a.slices, true, seam), on_line(&b.slices, true, seam));
        assert_eq!(east, west);
        assert!(east.iter().filter(|band| band.len() >= 2).count() >= 12, "{east:?}");
        // the south edge of `a` and the north edge of `c`
        let seam = a.bounds[1] as f32;
        assert_eq!(seam, c.bounds[3] as f32);
        let (south, north) = (on_line(&a.slices, false, seam), on_line(&c.slices, false, seam));
        assert_eq!(south, north);
        assert!(south.iter().filter(|band| band.len() >= 2).count() >= 12, "{south:?}");
        // and the three tiles carry the stack up to 12 km, where only the hail is left
        assert_eq!(a.levels.len(), RADAR_LEVELS_KM.len());
        assert_eq!(a.levels.last().map(|l| l.from_band), Some(11));
    }

    #[test]
    fn a_dry_or_covered_tile_has_no_meshes() {
        let index = TileIndex { x: 1, y: 1, z: 2 };
        assert!(process_tile(&[0; 16 * 16 * 4], 16, 16, index, None).is_none());
        // drizzle of 0.04 dBZ: echo, but under the first band
        let faint: Vec<u8> = [200, 190, 130, 1].repeat(16 * 16);
        assert!(process_tile(&faint, 16, 16, index, None).is_none());
        // hail everywhere, but all of it under a measured volume's coverage
        let hail = [255_u8; 16 * 16 * 4];
        let full = process_tile(&hail, 16, 16, index, None).unwrap();
        assert!(full.walls.is_none(), "a tile full of echo has no edges of its own");
        let bounds = tile_bounds(index);
        let cover = [coverage([bounds[0] - 1.0, bounds[1] - 1.0, bounds[2] + 1.0, bounds[3] + 1.0], 1, 1, vec![1])];
        assert!(process_tile(&hail, 16, 16, index, covered_by(&cover)).is_none());
    }

    /// `sum += a[i] * (k % 97 + 1)` over every value, as the TypeScript checksums were taken.
    fn checksum(values: &[f32]) -> f64 {
        let mut sum = 0.0;
        for (k, &v) in values.iter().enumerate() {
            sum += f64::from(v) * ((k % 97) + 1) as f64;
        }
        sum
    }

    #[test]
    fn a_tile_meshes_as_the_typescript_does() {
        // a yellow-to-red storm with white hail, a ring of blues and one of drizzle; the same
        // pixels run through `processTile` in bun gave the numbers below
        let (w, h) = (32, 32);
        let mut seed = 42_u32;
        let mut rnd = move || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            seed
        };
        let mut px = Vec::with_capacity(w * h * 4);
        for i in 0..w * h {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            let e = (2 * x - 40).pow(2) + 2 * (2 * y - 30).pow(2);
            px.extend_from_slice(&if e < 300 {
                [255, 255, 255, 255]
            } else if e < 2400 {
                [255, (224 - 7 * ((e - 300) / 64).min(32)) as u8, 0, 255]
            } else if e < 3600 {
                [0, (105 + rnd() % 100) as u8, 200, 255]
            } else if e < 4400 {
                [200, 190, 130, 150]
            } else {
                [0, 0, 0, 0]
            });
        }
        let t = process_tile(&px, w, h, TileIndex { x: 9, y: 5, z: 4 }, None).unwrap();
        assert_eq!(t.id, "4/9/5");
        let levels: Vec<(f64, usize)> = t.levels.iter().map(|l| (l.km, l.from_band)).collect();
        assert_eq!(levels, [(1.5, 1), (3.0, 2), (4.5, 4), (6.0, 5), (8.0, 7), (10.0, 9), (12.0, 11)]);
        // the vertices are the TypeScript's, value for value; the triangles are earcut's
        assert_eq!(t.slices.positions.len(), 7368);
        assert_eq!(checksum(&t.slices.positions), 14627129.456626892);
        assert_eq!(t.slices.band_vertex_starts, [0, 0, 11, 25, 109, 250, 413, 594, 1527, 2677, 3305, 3561, 3684, 3684, 3684]);
        assert_eq!(t.slices.colors[..4], band_color(1));
        // the walls come without earcut: all of them as the TypeScript has them
        let walls = t.walls.unwrap();
        assert_eq!(walls.positions.len(), 1836);
        assert_eq!(walls.indices.len(), 1662);
        assert_eq!(checksum(&walls.positions), 2400298.709848404);
        assert_eq!(walls.band_starts, [0, 0, 0, 12, 30, 150, 228, 318, 432, 882, 1356, 1530, 1662, 1662, 1662]);
    }
}
