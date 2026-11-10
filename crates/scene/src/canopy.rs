//! Pure part of `canopy.ts` (S1.23; doc 04 §8.6): how tall the trees are, metre by metre, the
//! world over. Meta and the World Resources Institute's canopy height map (v2, CC BY 4.0), as
//! Taylor Geospatial Engine Labs mirror it on Source Cooperative. Each file is a zoom-10 web
//! mercator tile (named by its quadkey), 32768 pixels across (1.2 m each, the height in whole
//! metres, 0 where there are no trees or no data), with smaller copies of itself; the parts
//! wanted are read out of it with HTTP range requests.
//!
//! The reading (a cloud-optimised GeoTIFF reader, [`FILES_CACHED`] files and [`MAX_CACHED`]
//! tiles kept) is the caller's. For a map tile of zoom [`FILE_ZOOM`] or more, `canopy` does:
//! [`place_in_file`]; the file at [`file_url`] of its key, without its masks ([`is_mask`]),
//! largest image first (none: no canopy); [`pick_image`]; then the [`window`] of that image,
//! its first sample, as the [`CanopyGrid`].

use crate::landcover::{grid_value, js_max, js_min};
use rustradar_core::num::js_round;

/// Where the files are, each `<quadkey>.tif`.
pub const CHM: &str = "https://data.source.coop/tge-labs/meta-chm-v2/chm/";
pub const CANOPY_ATTRIBUTION: &str = "Canopy height: Meta & WRI (CC BY 4.0), via TGE Labs";
/// The files' zoom; a tile under it has no canopy.
pub const FILE_ZOOM: u32 = 10;
/// Pixels across a tile read, at least (the nearest of the file's sizes at or over it).
pub const WANT: f64 = 512.0;
/// Files kept open by the caller.
pub const FILES_CACHED: usize = 24;
/// Tiles kept by the caller, the most recently used last.
pub const MAX_CACHED: usize = 96;

/// A tile's tree heights, metres, `size` × `size` from its north-west corner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CanopyGrid {
    pub size: usize,
    pub heights: Vec<u8>,
}

/// The height at a tile's `x, y` (0 to 1, y down); 0 where the TypeScript reads `undefined`
/// (a NaN place).
pub fn height_at(g: &CanopyGrid, x: f64, y: f64) -> u8 {
    grid_value(g.size, &g.heights, x, y)
}

/// The tallest within `r` pixels of a tile's `x, y` (a box of `2r + 1` pixels a side, cut by the
/// tile's edges); 0 for a NaN place, as the TypeScript's loops then never run.
pub fn tallest_near(g: &CanopyGrid, x: f64, y: f64, r: u32) -> u8 {
    let (s, r) = (g.size as f64, f64::from(r));
    let (cx, cy) = ((x * s).floor(), (y * s).floor());
    let mut best = 0;
    let mut j = js_max(0.0, cy - r);
    while j <= js_min(s - 1.0, cy + r) {
        let mut i = js_max(0.0, cx - r);
        while i <= js_min(s - 1.0, cx + r) {
            best = best.max(g.heights.get((j * s + i) as usize).copied().unwrap_or(0));
            i += 1.0;
        }
        j += 1.0;
    }
    best
}

/// A tile's quadkey (Bing's: a digit a zoom, x's bit and twice y's).
pub fn quadkey(x: u32, y: u32, z: u32) -> String {
    (1..=z)
        .rev()
        .map(|i| {
            // (`1 << n` as JavaScript shifts: the count taken modulo 32)
            let bit = 1u32.wrapping_shl(i - 1);
            char::from(b'0' + u8::from(x & bit != 0) + 2 * u8::from(y & bit != 0))
        })
        .collect()
}

/// Which zoom-10 file a tile is in ([`quadkey`]), and where in it: its north-west corner and its
/// size, as fractions of the file.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaceInFile {
    pub key: String,
    pub x: f64,
    pub y: f64,
    pub span: f64,
}

/// Where a tile (zoom [`FILE_ZOOM`] or more) is in its file. Under that zoom it is as wrong as
/// the TypeScript's (`>>` takes a negative count modulo 32); `canopy` never asks.
pub fn place_in_file(x: u32, y: u32, z: u32) -> PlaceInFile {
    let k = z as i32 - FILE_ZOOM as i32;
    // `t.x >> k`: a 32-bit shift, its count modulo 32
    let (fx, fy) = ((x as i32).wrapping_shr(k as u32), (y as i32).wrapping_shr(k as u32));
    let scale = 2f64.powi(k);
    let span = 1.0 / scale;
    PlaceInFile {
        key: quadkey(fx as u32, fy as u32, FILE_ZOOM),
        x: (f64::from(x) - f64::from(fx) * scale) * span,
        y: (f64::from(y) - f64::from(fy) * scale) * span,
        span,
    }
}

/// A file's URL by its key.
pub fn file_url(key: &str) -> String {
    format!("{CHM}{key}.tif")
}

/// Whether one of a file's images is a mask, by its `NewSubfileType` (a mask's has the 4 bit
/// set; no tag is 0). Masks are not read.
pub fn is_mask(new_subfile_type: Option<u32>) -> bool {
    new_subfile_type.unwrap_or(0) & 4 != 0
}

/// Which of a file's images (their widths, largest first) to read a tile out of: the smallest
/// with at least [`WANT`] pixels across the tile (`span` of the file), else the largest there
/// is; none when there are none.
pub fn pick_image(widths: &[u32], span: f64) -> Option<usize> {
    if widths.is_empty() {
        return None;
    }
    Some(widths.iter().rposition(|&w| f64::from(w) * span >= WANT).unwrap_or(0))
}

/// The pixels of an image read for a tile: from `x0, y0`, `size` across and down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub x0: usize,
    pub y0: usize,
    pub size: usize,
}

/// The window of an image `width` pixels across that a tile at `place` takes (at least a
/// pixel).
pub fn window(width: u32, place: &PlaceInFile) -> Window {
    let w = f64::from(width);
    let size = js_max(1.0, js_round(w * place.span));
    Window { x0: js_round(place.x * w) as usize, y0: js_round(place.y * w) as usize, size: size as usize }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;

    // `landcover.test.ts`
    #[test]
    fn the_canopys_files_by_quadkey_lunds() {
        assert_eq!(quadkey(549, 320, 10), "1202100101");
        let p = place_in_file(549 * 64 + 3, 320 * 64 + 5, 16);
        assert_eq!(p.key, "1202100101");
        assert_eq!(p.span, 1.0 / 64.0);
        assert_close!(p.x, 3.0 / 64.0, 12);
        assert_close!(p.y, 5.0 / 64.0, 12);
    }

    // New tests (no TypeScript twin).

    #[test]
    fn a_quadkey_a_digit_a_zoom() {
        assert_eq!(quadkey(0, 0, 0), "");
        assert_eq!(quadkey(1, 0, 1), "1");
        assert_eq!(quadkey(0, 1, 1), "2");
        // Bing's own example
        assert_eq!(quadkey(3, 5, 3), "213");
        assert_eq!(quadkey(35210, 21493, 16), "1202102332221212");
    }

    #[test]
    fn a_file_tile_is_its_whole_file() {
        let p = place_in_file(549, 320, 10);
        assert_eq!(p, PlaceInFile { key: "1202100101".into(), x: 0.0, y: 0.0, span: 1.0 });
        assert_eq!(file_url(&p.key), "https://data.source.coop/tge-labs/meta-chm-v2/chm/1202100101.tif");
    }

    #[test]
    fn heights_at_a_place_and_the_tallest_near_it() {
        // 4 × 4, a 9 m tree in the second row
        let mut heights = vec![0; 16];
        heights[4 + 2] = 9;
        heights[15] = 3;
        let g = CanopyGrid { size: 4, heights };
        assert_eq!(height_at(&g, 0.6, 0.3), 9);
        assert_eq!(height_at(&g, 0.1, 0.1), 0);
        assert_eq!(tallest_near(&g, 0.1, 0.1, 0), 0);
        assert_eq!(tallest_near(&g, 0.1, 0.1, 1), 0);
        assert_eq!(tallest_near(&g, 0.3, 0.1, 1), 9);
        // cut by the tile's edges
        assert_eq!(tallest_near(&g, 0.99, 0.99, 1), 3);
        assert_eq!(tallest_near(&g, 0.99, 0.99, 3), 9);
        assert_eq!(tallest_near(&g, f64::NAN, 0.5, 3), 0);
        assert_eq!(height_at(&g, f64::NAN, 0.5), 0);
    }

    #[test]
    fn the_smallest_copy_with_enough_pixels_else_the_largest() {
        let widths = [32768, 16384, 8192, 4096, 2048, 1024, 512];
        // a zoom-16 tile is a 64th of its file: 512 pixels across in the 32768 one
        assert_eq!(pick_image(&widths, 1.0 / 64.0), Some(0));
        // a zoom-13 tile: an eighth, enough in the 4096 one
        assert_eq!(pick_image(&widths, 1.0 / 8.0), Some(3));
        // a zoom-18 tile: never enough, the largest
        assert_eq!(pick_image(&widths, 1.0 / 256.0), Some(0));
        assert_eq!(pick_image(&widths, 1.0), Some(6));
        assert_eq!(pick_image(&[], 1.0), None);
        assert!(is_mask(Some(4)) && is_mask(Some(5)) && !is_mask(Some(1)) && !is_mask(None));
    }

    #[test]
    fn a_tiles_window_in_its_image() {
        let p = place_in_file(549 * 8 + 3, 320 * 8 + 5, 13);
        assert_eq!(window(4096, &p), Window { x0: 1536, y0: 2560, size: 512 });
        // at least a pixel
        let p = place_in_file(549 * 1024 + 1023, 320 * 1024, 20);
        assert_eq!(window(512, &p), Window { x0: 512, y0: 0, size: 1 });
    }
}
