//! Port of `rail-overview.ts` (doc 02 §8.3): the railway network zoomed out.
//!
//! The map's vector tiles carry railways only from zoom 8, so a whole country (Sweden, say) showed
//! no rail at all. Below that the network comes from Natural Earth's 1:10m railroads (public
//! domain), simplified to about a kilometre and shipped with the app as
//! `static/rail/railroads.bin`; see `scripts/build-rail-overview.py` for the format:
//!
//! ```text
//! "RRO1"                                  4 bytes magic
//! u32 paths, u32 points                   little-endian
//! per path:  u8 category, u16 point_count (3 bytes each, packed)
//! per point: i16 lon*100, i16 lat*100     (4 bytes each)
//! ```
//!
//! The lazy loading of `railOverview()` (fetch once, drawn without it on failure) belongs to the
//! app; this holds the parser and what `railOverviewLayer` draws each path with.

use std::fmt;

/// An 8-bit colour with alpha (`rail-overview.ts` `RGBA`).
pub type Rgba = [u8; 4];

/// The parsed file (`RailOverview`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RailOverview {
    /// The number of paths.
    pub length: usize,
    /// Where each path starts in `positions` (in points), and finally the point count.
    pub start_indices: Vec<u32>,
    /// lon, lat pairs.
    pub positions: Vec<f32>,
    /// Natural Earth's category per path: 1 main lines, 2 and 3 lesser ones.
    pub categories: Vec<u8>,
}

/// Why a buffer is not a rail overview (the TypeScript throws these messages).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailOverviewError {
    /// "not a rail overview file"
    NotRailOverview,
    /// "rail overview file is truncated" (also for a buffer too short for its header, where the
    /// TypeScript's `DataView` throws a `RangeError`)
    Truncated,
    /// "rail overview point count disagrees"
    PointCountDisagrees,
}

impl fmt::Display for RailOverviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RailOverviewError::NotRailOverview => "not a rail overview file",
            RailOverviewError::Truncated => "rail overview file is truncated",
            RailOverviewError::PointCountDisagrees => "rail overview point count disagrees",
        })
    }
}

impl std::error::Error for RailOverviewError {}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Parses the packed file (`parseRailOverview`); refuses anything that isn't one.
pub fn parse_rail_overview(bytes: &[u8]) -> Result<RailOverview, RailOverviewError> {
    if bytes.get(0..4) != Some(b"RRO1".as_slice()) {
        return Err(RailOverviewError::NotRailOverview);
    }
    if bytes.len() < 12 {
        return Err(RailOverviewError::Truncated);
    }
    let paths = u32_at(bytes, 4);
    let points = u32_at(bytes, 8);
    let header_end = 12 + u64::from(paths) * 3;
    if bytes.len() as u64 != header_end + u64::from(points) * 4 {
        return Err(RailOverviewError::Truncated);
    }
    let (paths, points, header_end) = (paths as usize, points as usize, header_end as usize);
    let mut start_indices = vec![0_u32; paths + 1];
    let mut categories = vec![0_u8; paths];
    let mut at: u64 = 0;
    for i in 0..paths {
        categories[i] = bytes[12 + i * 3];
        start_indices[i] = at as u32;
        at += u64::from(u16_at(bytes, 12 + i * 3 + 1));
    }
    start_indices[paths] = at as u32;
    if at != points as u64 {
        return Err(RailOverviewError::PointCountDisagrees);
    }
    let positions = bytes[header_end..]
        .chunks_exact(2)
        .map(|v| (f64::from(i16::from_le_bytes([v[0], v[1]])) / 100.0) as f32)
        .collect();
    Ok(RailOverview { length: paths, start_indices, positions, categories })
}

/// The two colours the overview is drawn in, by category (`railOverviewLayer`'s `colors`):
/// `rail::RAIL_MAIN` and `rail::RAIL_MINOR` on the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RailOverviewColors {
    pub main: Rgba,
    pub minor: Rgba,
}

impl RailOverview {
    /// Path `i`'s lon, lat pairs.
    pub fn path(&self, i: usize) -> &[f32] {
        let (from, to) = (self.start_indices[i] as usize, self.start_indices[i + 1] as usize);
        &self.positions[from * 2..to * 2]
    }

    /// Path `i`'s colour: main lines (category 1) in `colors.main`, the rest in `colors.minor`.
    pub fn color(&self, i: usize, colors: &RailOverviewColors) -> Rgba {
        if self.categories[i] == 1 { colors.main } else { colors.minor }
    }

    /// Path `i`'s width in screen pixels: 1.5 for a main line, else 1.
    pub fn width(&self, i: usize) -> f64 {
        if self.categories[i] == 1 { 1.5 } else { 1.0 }
    }
}

/// The overview layer's id in each projection. It is drawn as open paths, widths in pixels, with
/// no culling and no depth write (`SURFACE_NO_CULL`).
pub fn rail_overview_layer_id(flat: bool) -> &'static str {
    if flat { "rail-overview-flat" } else { "rail-overview-globe" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file() -> Vec<u8> {
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../static/rail/railroads.bin")).expect("static/rail/railroads.bin")
    }

    #[test]
    fn the_shipped_overview_parses_and_covers_sweden() {
        let rail = parse_rail_overview(&file()).unwrap();
        assert!(rail.length > 20_000);
        assert_eq!(rail.start_indices[rail.length] as usize, rail.positions.len() / 2);
        let mut in_sweden = 0;
        for p in rail.positions.chunks(2) {
            let (lon, lat) = (p[0], p[1]);
            if lon > 12.0 && lon < 22.0 && lat > 56.0 && lat < 67.0 {
                in_sweden += 1;
            }
        }
        assert!(in_sweden > 1_000);
    }

    #[test]
    fn anything_that_isnt_the_file_is_refused() {
        let buffer = file();
        assert!(parse_rail_overview(&[0; 16]).is_err());
        assert!(parse_rail_overview(&buffer[..buffer.len() - 4]).is_err());
    }

    // new tests

    #[test]
    fn the_shipped_overview_reads_as_the_typescript_reads_it() {
        // parseRailOverview's answers for the same file, printed by bun
        let rail = parse_rail_overview(&file()).unwrap();
        assert_eq!((rail.length, rail.positions.len() / 2), (25_225, 124_464));
        assert_eq!(rail.categories.iter().filter(|&&c| c == 1).count(), 7_106);
        assert_eq!((rail.categories[0], rail.start_indices[1]), (1, 2));
        assert_eq!(&rail.positions[..2], [30.780000686645508_f64 as f32, 69.45999908447266_f64 as f32]);
        assert_eq!(rail.path(0), &rail.positions[..4]);
        let colors = RailOverviewColors { main: [240, 112, 96, 245], minor: [214, 128, 112, 225] };
        assert_eq!((rail.color(0, &colors), rail.width(0)), (colors.main, 1.5));
        let minor = rail.categories.iter().position(|&c| c != 1).unwrap();
        assert_eq!((rail.color(minor, &colors), rail.width(minor)), (colors.minor, 1.0));
        assert_eq!((rail_overview_layer_id(false), rail_overview_layer_id(true)), ("rail-overview-globe", "rail-overview-flat"));
    }

    #[test]
    fn each_way_of_not_being_the_file_says_so() {
        // two paths of 2 and 1 points: (1.5, -2.25), (180, -90), (-0.01, 0)
        let mut good = b"RRO1".to_vec();
        good.extend(2_u32.to_le_bytes());
        good.extend(3_u32.to_le_bytes());
        good.extend([1, 2, 0, 3, 1, 0]);
        for v in [150_i16, -225, 18000, -9000, -1, 0] {
            good.extend(v.to_le_bytes());
        }
        let rail = parse_rail_overview(&good).unwrap();
        assert_eq!(rail.start_indices, [0, 2, 3]);
        assert_eq!(rail.categories, [1, 3]);
        assert_eq!(rail.positions, [1.5, -2.25, 180.0, -90.0, -0.01, 0.0]);
        assert_eq!(rail.path(1), [-0.01, 0.0]);

        assert_eq!(parse_rail_overview(b"RRO"), Err(RailOverviewError::NotRailOverview));
        assert_eq!(parse_rail_overview(b"RRO2\0\0\0\0\0\0\0\0"), Err(RailOverviewError::NotRailOverview));
        assert_eq!(parse_rail_overview(&good[..10]), Err(RailOverviewError::Truncated));
        let mut longer = good.clone();
        longer.push(0);
        assert_eq!(parse_rail_overview(&longer), Err(RailOverviewError::Truncated));
        let mut miscounted = good.clone();
        miscounted[13] = 1; // the first path says 1 point: 2 in all, the header says 3
        assert_eq!(parse_rail_overview(&miscounted), Err(RailOverviewError::PointCountDisagrees));
        assert_eq!(RailOverviewError::Truncated.to_string(), "rail overview file is truncated");
    }
}
