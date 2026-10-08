//! DWD's WN composite: reflectivity measured at 25 heights, 500 m apart from
//! the surface to 12 km, on a 1 km grid over central Europe, every 5 minutes.
//! Germany's radar network plus its neighbours' — the European counterpart to
//! MRMS, and the reason the globe does not have to guess storm heights from
//! the colour of a RainViewer tile anywhere near Germany.
//!
//! Published free by Deutscher Wetterdienst at opendata.dwd.de. Each run is a
//! bzip2'd tar of one file per height in DWD's composite format: an ASCII
//! header terminated by ETX, then one little-endian 16-bit value per cell on
//! the RADOLAN polar stereographic grid, holding reflectivity in tenths of an
//! RVP6 count.

use std::io::Read;

use crate::radar::{self, Bounds, Error, GROUND_MIN_DBZ, LIFTED_MIN_DBZ, Volume};

const DIRECTORY: &str = "https://opendata.dwd.de/weather/radar/composite/wn/";

/// Heights to draw, km. The product carries every 500 m to 12 km; above 6 km
/// the extra detail is not worth another image.
const LEVELS_KM: [f32; 18] = [
    0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0, 5.5, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
];
/// Output resolution in degrees: the ground composite, then each height.
const GROUND_STEP: f64 = 0.01;
const LEVEL_STEP: f64 = 0.02;
const COVERAGE_STEP: f64 = 0.1;

/// DWD's Fehlkennung bit: the cell holds no measurement. Testing the flag
/// rather than the whole word matters because the clutter (`0x8000`) and
/// secondary (`0x1000`) bits may ride alongside it; such a word would then
/// unmask to 0x9C4 — 92.5 dBZ, the top of the palette — and paint a violet
/// column at every height. No live level has carried the combination so far,
/// so this is insurance rather than a cure.
const NO_DATA_FLAG: u16 = 0x2000;

/// Reflectivity from a stored cell. The header's `PR E-01` is tenths of an
/// RVP6 count, and RVP6 is twice the dBZ above -32.5 — reading the value as
/// tenths of a dBZ instead puts thunderstorms past 140 dBZ.
fn dbz(raw: u16) -> f32 {
    f32::from(raw & 0x0FFF) / 20.0 - 32.5
}

// The RADOLAN grid: a polar stereographic sphere, true at 60°N, aligned to
// 10°E. `de1200` places the reference point 9°E/51°N 470 km from the left
// edge and 600 km from the bottom. Checked against the published corner of
// the 900x900 grid, which these constants reproduce to four decimals.
const EARTH_KM: f64 = 6370.04;
const TRUE_LAT: f64 = 60.0;
const MERIDIAN: f64 = 10.0;
const REF_LON: f64 = 9.0;
const REF_LAT: f64 = 51.0;
const J0: f64 = 470.0;
const I0: f64 = 600.0;

/// Longitude and latitude to RADOLAN grid kilometres.
fn project(lon: f64, lat: f64) -> (f64, f64) {
    let (lon, lat) = (lon.to_radians(), lat.to_radians());
    let m = (1.0 + TRUE_LAT.to_radians().sin()) / (1.0 + lat.sin());
    let d = lon - MERIDIAN.to_radians();
    (
        EARTH_KM * m * lat.cos() * d.sin(),
        -EARTH_KM * m * lat.cos() * d.cos(),
    )
}

/// The inverse, for working out what the grid covers.
fn unproject(x: f64, y: f64) -> (f64, f64) {
    let lon = (-x / y).atan().to_degrees() + MERIDIAN;
    let k = EARTH_KM * (1.0 + TRUE_LAT.to_radians().sin());
    let r2 = x * x + y * y;
    let lat = ((k * k - r2) / (k * k + r2)).asin().to_degrees();
    (lon, lat)
}

pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    /// dBZ, row 0 southernmost; [`f32::NEG_INFINITY`] where no radar sees.
    pub dbz: Vec<f32>,
}

impl Grid {
    /// The grid's south-west corner in RADOLAN kilometres.
    fn origin(&self) -> (f64, f64) {
        let (x0, y0) = project(REF_LON, REF_LAT);
        (x0 - J0, y0 - I0)
    }

    /// Nearest cell to a position, or `None` outside the grid.
    fn at(&self, lon: f64, lat: f64) -> Option<f32> {
        let (ox, oy) = self.origin();
        let (x, y) = project(lon, lat);
        let (j, i) = (x - ox, y - oy);
        if j < 0.0 || i < 0.0 || j >= self.cols as f64 || i >= self.rows as f64 {
            return None;
        }
        Some(self.dbz[i as usize * self.cols + j as usize])
    }

    /// Latitude and longitude limits of the whole grid.
    ///
    /// The corners alone are not enough. Latitude here depends only on
    /// `r² = x² + y²`, so along the northern edge it peaks where `|x|` is
    /// smallest — on `de1200` that is `x = 0`, well inside the grid, and the
    /// corners miss some 0.36° (40 km) of the top. Longitude grows with `x/|y|`
    /// and latitude falls with `r²`, both monotonic along an edge, so west,
    /// east and south really are corner values.
    fn bounds(&self) -> Bounds {
        let (ox, oy) = self.origin();
        let (w, h) = (self.cols as f64, self.rows as f64);
        let mut points = vec![(ox, oy), (ox + w, oy), (ox, oy + h), (ox + w, oy + h)];
        // the meridian crossing, when the grid straddles it
        if (ox..=ox + w).contains(&0.0) {
            points.push((0.0, oy + h));
        }
        let edges: Vec<_> = points.into_iter().map(|(x, y)| unproject(x, y)).collect();
        Bounds {
            west: edges.iter().map(|c| c.0).fold(f64::MAX, f64::min),
            east: edges.iter().map(|c| c.0).fold(f64::MIN, f64::max),
            south: edges.iter().map(|c| c.1).fold(f64::MAX, f64::min),
            north: edges.iter().map(|c| c.1).fold(f64::MIN, f64::max),
        }
    }
}

/// Splits a tar archive into its regular files.
fn untar(data: &[u8]) -> Vec<(String, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 512 <= data.len() {
        let header = &data[at..at + 512];
        if header.iter().all(|&b| b == 0) {
            break; // the two empty blocks that end an archive
        }
        let name = String::from_utf8_lossy(&header[..100])
            .trim_end_matches('\0')
            .to_string();
        let size = String::from_utf8_lossy(&header[124..136]);
        let size = usize::from_str_radix(size.trim_end_matches(['\0', ' ']).trim(), 8).unwrap_or(0);
        at += 512;
        // '0' and NUL both mean a regular file
        if matches!(header[156], b'0' | 0) && at + size <= data.len() {
            out.push((name, &data[at..at + size]));
        }
        at += size.div_ceil(512) * 512;
    }
    out
}

/// Reads one composite file: the ASCII header gives the grid, then the cells.
pub fn decode(file: &[u8]) -> Result<Grid, Error> {
    let etx = file
        .iter()
        .position(|&b| b == 0x03)
        .ok_or("no end of header")?;
    // Searched as bytes: a lossy conversion can turn a stray byte into a
    // multi-byte U+FFFD, and slicing across one panics. The station list at
    // the tail is free text in angle brackets and could hold anything, so only
    // the fixed fields ahead of it are considered.
    let header = &file[..etx];
    let fields = match header.iter().position(|&b| b == b'<') {
        Some(at) => &header[..at],
        None => header,
    };
    let gp = fields
        .windows(2)
        .position(|w| w == b"GP")
        .ok_or("header has no grid size")?;
    let size = fields
        .get(gp + 2..gp + 11)
        .ok_or("header ends inside its grid size")?;
    let size = std::str::from_utf8(size).map_err(|_| "grid size is not text")?;
    let (rows, cols) = size.split_once('x').ok_or("malformed grid size")?;
    let rows: usize = rows.trim().parse().map_err(|_| "unreadable row count")?;
    let cols: usize = cols.trim().parse().map_err(|_| "unreadable column count")?;
    let cells = rows
        .checked_mul(cols)
        .and_then(|n| n.checked_mul(2))
        .ok_or("grid size is implausible")?;
    let body = &file[etx + 1..];
    if body.len() < cells {
        return Err("composite is shorter than its grid".into());
    }
    let dbz = body
        .as_chunks::<2>()
        .0
        .iter()
        .take(rows * cols)
        .map(|c| {
            let raw = u16::from_le_bytes(*c);
            if raw & NO_DATA_FLAG != 0 {
                f32::NEG_INFINITY
            } else {
                dbz(raw)
            }
        })
        .collect();
    Ok(Grid { cols, rows, dbz })
}

/// Height in km from a member name like `WN2610081840_030`.
fn level_of(name: &str) -> Option<f32> {
    let suffix = name.rsplit('_').next()?;
    suffix.parse::<f32>().ok().map(|v| v / 10.0)
}

/// Unix seconds from a run name's `WN<YYMMDDhhmm>`, as in
/// `WN2610081930.tar.bz2`; `now` stands in for a name we cannot read.
fn run_time(name: &str, now: i64) -> i64 {
    let digits: String = name
        .trim_start_matches("WN")
        .chars()
        .take(10)
        .filter(|c| c.is_ascii_digit())
        .collect();
    let number = |at: usize| digits.get(at..at + 2).and_then(|s| s.parse::<i64>().ok());
    match (number(0), number(6), number(8)) {
        // YYMMDDhhmm
        (Some(_), Some(hh), Some(mm)) if digits.len() >= 10 => {
            let days = crate::mrms::days_from_civil(
                2000 + digits[0..2].parse::<i64>().unwrap_or(0),
                digits[2..4].parse().unwrap_or(1),
                digits[4..6].parse().unwrap_or(1),
            );
            days * 86_400 + hh * 3600 + mm * 60
        }
        _ => now,
    }
}

/// Resamples a grid onto plain latitude and longitude at `step` degrees.
fn resample(grid: &Grid, bounds: Bounds, step: f64) -> (usize, usize, Vec<f32>) {
    let w = ((bounds.east - bounds.west) / step).round() as usize;
    let h = ((bounds.north - bounds.south) / step).round() as usize;
    let mut out = vec![f32::NEG_INFINITY; w * h];
    for row in 0..h {
        // north row first, the order the frontend's images expect
        let lat = bounds.north - (row as f64 + 0.5) * step;
        for col in 0..w {
            let lon = bounds.west + (col as f64 + 0.5) * step;
            if let Some(v) = grid.at(lon, lat) {
                out[row * w + col] = v;
            }
        }
    }
    (w, h, out)
}

/// Downloads and reduces the newest run.
pub async fn fetch(http: &reqwest::Client) -> Result<Volume, Error> {
    let listing = http
        .get(DIRECTORY)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let newest = listing
        .split("href=\"")
        .skip(1)
        .filter_map(|s| s.split('"').next())
        .filter(|name| name.starts_with("WN") && name.ends_with(".tar.bz2"))
        .max()
        .ok_or("no WN composite published")?
        .to_owned();
    let archive = http
        .get(format!("{DIRECTORY}{newest}"))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let time = run_time(&newest, crate::mrms::time_now());

    tokio::task::spawn_blocking(move || {
        let mut tar = Vec::new();
        bzip2_rs::DecoderReader::new(&archive[..]).read_to_end(&mut tar)?;

        let members = untar(&tar);
        let wanted: Vec<_> = members
            .iter()
            .filter_map(|(name, body)| {
                let km = level_of(name)?;
                LEVELS_KM.contains(&km).then_some((km, *body))
            })
            .collect();
        if wanted.is_empty() {
            return Err("the run carried none of the heights we draw".into());
        }

        // One short or corrupt member must not cost the whole continent, so a
        // height that fails is reported and dropped, as MRMS does.
        let mut frame: Option<(Bounds, usize, usize)> = None;
        let mut ground: Vec<f32> = Vec::new();
        let mut seen: Option<(usize, usize, Vec<u8>)> = None;
        let mut levels = Vec::with_capacity(LEVELS_KM.len());

        for &km in &LEVELS_KM {
            let Some((_, body)) = wanted.iter().find(|(k, _)| *k == km) else {
                levels.push((km, None));
                continue;
            };
            let grid = match decode(body) {
                Ok(grid) => grid,
                Err(e) => {
                    eprintln!("DWD {km} km: {e}");
                    levels.push((km, None));
                    continue;
                }
            };
            // every height shares one grid, so the first to decode fixes the frame
            let (bounds, _, _) = *frame.get_or_insert_with(|| {
                let bounds = grid.bounds();
                let gw = ((bounds.east - bounds.west) / GROUND_STEP).round() as usize;
                let gh = ((bounds.north - bounds.south) / GROUND_STEP).round() as usize;
                ground = vec![f32::NEG_INFINITY; gw * gh];
                (bounds, gw, gh)
            });
            let (_, _, g) = resample(&grid, bounds, GROUND_STEP);
            for (cell, v) in ground.iter_mut().zip(g) {
                *cell = cell.max(v);
            }
            let (cw, ch, c) = resample(&grid, bounds, COVERAGE_STEP);
            let mask: Vec<u8> = c.iter().map(|v| u8::from(v.is_finite())).collect();
            match &mut seen {
                Some((_, _, all)) => all.iter_mut().zip(mask).for_each(|(a, s)| *a |= s),
                None => seen = Some((cw, ch, mask)),
            }
            let (lw, lh, l) = resample(&grid, bounds, LEVEL_STEP);
            // the height still counts towards the ground and the mask above
            let png = radar::encode(lw, lh, &l, LIFTED_MIN_DBZ).unwrap_or_else(|e| {
                eprintln!("DWD {km} km: {e}");
                None
            });
            levels.push((km, png));
        }

        let (bounds, gw, gh) = frame.ok_or("no DWD height could be decoded")?;
        let (cw, ch, coverage) = seen.unwrap_or_default();
        Ok(Volume {
            source: "dwd",
            time,
            fetched: std::time::Instant::now(),
            bounds,
            ground: radar::encode(gw, gh, &ground, GROUND_MIN_DBZ)?.unwrap_or_default(),
            levels,
            coverage,
            coverage_size: (cw as u32, ch as u32),
        })
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published corner of the 900x900 grid, which shares these constants.
    #[test]
    fn projection_matches_the_published_grid() {
        let (x, y) = project(REF_LON, REF_LAT);
        assert!((x - 450.0 - -523.4622).abs() < 0.001, "x was {x}");
        assert!((y - 450.0 - -4658.6447).abs() < 0.001, "y was {y}");
    }

    #[test]
    fn projection_round_trips() {
        for (lon, lat) in [(9.0, 51.0), (3.6, 46.0), (15.0, 54.0)] {
            let (x, y) = project(lon, lat);
            let (back_lon, back_lat) = unproject(x, y);
            assert!((back_lon - lon).abs() < 1e-6, "{lon} -> {back_lon}");
            assert!((back_lat - lat).abs() < 1e-6, "{lat} -> {back_lat}");
        }
    }

    #[test]
    fn reflectivity_stays_physical() {
        assert!(
            (dbz(0) - -32.5).abs() < 1e-3,
            "an empty cell is the noise floor"
        );
        // the strongest cell of a real run decodes to a storm core, not 140 dBZ
        assert!((dbz(1771) - 56.05).abs() < 0.01, "{}", dbz(1771));
        assert!((dbz(1249) - 29.95).abs() < 0.01, "{}", dbz(1249));
    }

    #[test]
    fn reads_level_heights() {
        assert_eq!(level_of("WN2610081840_030"), Some(3.0));
        assert_eq!(level_of("WN2610081840_005"), Some(0.5));
        assert_eq!(level_of("WN2610081840_120"), Some(12.0));
    }

    /// A header shaped like the real thing, followed by an all-zero grid.
    fn composite(header: &str, rows: usize, cols: usize) -> Vec<u8> {
        let mut file = header.as_bytes().to_vec();
        file.push(0x03);
        file.extend(std::iter::repeat_n(0u8, rows * cols * 2));
        file
    }

    const HEADER: &str = "WN081840100001026BY   2640195VS 5SW  P42001HPR E-01INT   5GP0003x0004VV 030MF 00000008MS103<deasb,deboo>";

    #[test]
    fn reads_the_grid_size_past_the_station_list() {
        let grid = decode(&composite(HEADER, 3, 4)).unwrap();
        assert_eq!((grid.rows, grid.cols), (3, 4));
        assert_eq!(grid.dbz.len(), 12);
    }

    /// Short and malformed headers used to panic on a fixed nine-byte slice.
    #[test]
    fn rejects_broken_grid_sizes_without_panicking() {
        for header in ["WN0818401GP12", "WN0818401GP", "WN0818401", "GPabcdxefg"] {
            assert!(decode(&composite(header, 0, 0)).is_err(), "{header}");
        }
        // a lossy conversion would widen this byte into a multi-byte U+FFFD
        let mut raw = b"WN0818401GP\xff200x1100VV".to_vec();
        raw.push(0x03);
        assert!(decode(&raw).is_err());
        // "GP" inside the station list is not a grid size
        let tail = "WN081840100001026VV 030MS103<deasb,GP9999x9999>";
        assert!(decode(&composite(tail, 0, 0)).is_err());
    }

    /// The Fehlkennung flag, not one exact word, marks a cell as unmeasured.
    #[test]
    fn no_data_survives_the_other_flags() {
        for raw in [0x29C4u16, 0xA9C4, 0x39C4, 0x2000] {
            let mut file = composite(HEADER, 0, 0);
            file.pop();
            file.extend_from_slice("GP0001x0001VV 030".as_bytes());
            file.push(0x03);
            file.extend_from_slice(&raw.to_le_bytes());
            let grid = decode(&file).unwrap();
            assert_eq!(grid.dbz[0], f32::NEG_INFINITY, "{raw:#06X}");
        }
        // a clutter-flagged real echo still decodes
        let mut file = composite(HEADER, 0, 0);
        file.pop();
        file.extend_from_slice("GP0001x0001VV 030".as_bytes());
        file.push(0x03);
        file.extend_from_slice(&(0x8000u16 | 1249).to_le_bytes());
        let grid = decode(&file).unwrap();
        assert!((grid.dbz[0] - 29.95).abs() < 0.01, "{}", grid.dbz[0]);
    }

    /// Latitude peaks at `x = 0`, inside the grid; the corners stop at 55.87°.
    #[test]
    fn bounds_reach_the_true_northern_edge() {
        let grid = Grid {
            cols: 1100,
            rows: 1200,
            dbz: Vec::new(),
        };
        let b = grid.bounds();
        assert!((b.north - 56.2247).abs() < 1e-3, "north was {}", b.north);
        assert!((b.south - 45.6836).abs() < 1e-3, "south was {}", b.south);
        assert!((b.west - 1.4356).abs() < 1e-3, "west was {}", b.west);
        assert!((b.east - 18.7673).abs() < 1e-3, "east was {}", b.east);
        // every cell of the grid must fall inside the reported box
        for (x, y) in [(0.0, 1200.0), (1100.0, 1200.0), (0.0, 0.0), (1100.0, 0.0)] {
            let (ox, oy) = grid.origin();
            let (lon, lat) = unproject(ox + x, oy + y);
            assert!(lat <= b.north + 1e-9 && lat >= b.south - 1e-9, "{lat}");
            assert!(lon <= b.east + 1e-9 && lon >= b.west - 1e-9, "{lon}");
        }
    }

    /// A grid that does not straddle the meridian keeps its corner extremes.
    #[test]
    fn bounds_fall_back_to_corners_off_the_meridian() {
        let grid = Grid {
            cols: 100,
            rows: 100,
            dbz: Vec::new(),
        };
        let (ox, oy) = grid.origin();
        assert!(ox + 100.0 < 0.0, "this grid should sit west of 10°E");
        let b = grid.bounds();
        let north = unproject(ox + 100.0, oy + 100.0).1;
        assert!((b.north - north).abs() < 1e-9, "{} vs {north}", b.north);
    }

    #[test]
    fn reads_run_times() {
        // WN2610081930: 2026-10-08 19:30 UTC
        assert_eq!(run_time("WN2610081930.tar.bz2", 0), 1_791_487_800);
        assert_eq!(run_time("nonsense", 42), 42);
    }

    #[test]
    fn untars_a_small_archive() {
        let mut tar = vec![0u8; 1024];
        tar[..5].copy_from_slice(b"a.txt");
        tar[124..135].copy_from_slice(b"00000000004");
        tar[156] = b'0';
        tar[512..516].copy_from_slice(b"data");
        let files = untar(&tar);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "a.txt");
        assert_eq!(files[0].1, b"data");
    }

    /// Fetches the live run: `cargo test -p rustradar dwd -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_run() {
        let started = std::time::Instant::now();
        let volume = fetch(&reqwest::Client::new()).await.unwrap();
        let info = volume.info();
        println!(
            "time {} heights {:?}\nbounds {:.2}..{:.2}E {:.2}..{:.2}N  ground {} KB  coverage {}x{}  in {:?}",
            info.time,
            info.levels,
            info.bounds.west,
            info.bounds.east,
            info.bounds.south,
            info.bounds.north,
            volume.ground.len() / 1024,
            info.coverage_width,
            info.coverage_height,
            started.elapsed()
        );
        // the mask should look like the German radar network, not the whole box
        let lit = volume.coverage.iter().filter(|&&c| c == 1).count();
        let total = volume.coverage.len();
        println!(
            "coverage {:.0}% of the box",
            lit as f64 / total as f64 * 100.0
        );
        let w = info.coverage_width as usize;
        for row in volume.coverage.chunks(w).step_by(3) {
            let line: String = row
                .iter()
                .step_by(2)
                .map(|&c| if c == 1 { '#' } else { '.' })
                .collect();
            println!("{line}");
        }
        assert!(lit > total / 20, "almost nothing is covered");
    }
}
