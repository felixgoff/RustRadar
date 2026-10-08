//! NOAA MRMS (Multi-Radar Multi-Sensor): radar reflectivity measured at many
//! heights, merged from every weather radar over the contiguous US and
//! southern Canada. A true 3D picture of precipitation at 0.01° (about 1 km)
//! every two minutes, from NOAA's public bucket.
//!
//! Each height is a GRIB2 file holding one 7000×3500 grid, packed as a 16-bit
//! PNG (data representation template 5.41). Grids are reduced here to
//! contoured isobands of the composite for the ground and of each height,
//! and a coverage mask (where the radars see), ready for the globe.

use std::io::Read;
use std::time::Instant;

use futures_util::StreamExt;
use image::ImageFormat;

use crate::radar::{self, Bounds, Error, GROUND_MIN_DBZ, LIFTED_MIN_DBZ, Volume};

const BUCKET: &str = "https://noaa-mrms-pds.s3.amazonaws.com";
/// Heights (km above sea level) drawn as layers; MRMS has 33 between 0.5 and
/// 19 km, but each one is a megabyte on the wire, so take a sparser ladder.
pub const LEVELS_KM: [f32; 12] = [
    1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 12.0, 14.0,
];
/// Source cells per contoured grid cell: 0.02° for the ground and per height.
const GROUND_STEP: usize = 2;
const LEVEL_STEP: usize = 2;
/// Coverage mask cells: 0.1°.
const COVERAGE_STEP: usize = 10;
/// The CONUS grid's outer edges, degrees.
pub const BOUNDS: Bounds = Bounds {
    west: -130.0,
    south: 20.0,
    east: -60.0,
    north: 55.0,
};

/// A height reduced to the handful of small outputs the globe needs. The
/// 98 MB grid it came from is already gone by the time this exists, which is
/// what keeps a whole volume from sitting in memory at once.
struct Reduced {
    time: i64,
    /// dBZ at ground resolution, folded into the composite.
    ground: (usize, usize, Vec<f32>),
    /// 1 where this height has data, for the coverage mask.
    seen: (usize, usize, Vec<u8>),
    /// The layer's isobands, or `None` where nothing reached [`LIFTED_MIN_DBZ`].
    shapes: Option<Vec<u8>>,
}

/// A reduced height, or why it couldn't be loaded.
type LevelResult = Result<Reduced, Error>;

fn product(km: f32) -> String {
    format!("MergedReflectivityQC_{km:05.2}")
}

/// The newest file of a product: today's listing, or yesterday's just after midnight.
async fn latest_key(http: &reqwest::Client, product: &str) -> Result<String, Error> {
    let now = time_now();
    for days_back in 0..2 {
        let (date, start_after) = if days_back == 0 {
            // skip most of the day's ~700 files
            let earlier = now - 20 * 60;
            let date = ymd(earlier);
            (
                date.clone(),
                format!(
                    "CONUS/{product}/{date}/MRMS_{product}_{date}-{}",
                    hms(earlier)
                ),
            )
        } else {
            (ymd(now - 86_400), String::new())
        };
        let url = format!(
            "{BUCKET}/?list-type=2&prefix=CONUS/{product}/{date}/&start-after={start_after}"
        );
        let listing = http
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        if let Some(key) = keys(&listing).last() {
            return Ok((*key).to_owned());
        }
    }
    Err(format!("no recent {product} files").into())
}

fn keys(listing: &str) -> Vec<&str> {
    listing
        .split("<Key>")
        .skip(1)
        .filter_map(|s| s.split("</Key>").next())
        .filter(|k| k.ends_with(".grib2.gz"))
        .collect()
}

/// Unix seconds from a key's `YYYYMMDD-HHMMSS`.
fn key_time(key: &str) -> Option<i64> {
    let stamp = key.rsplit('_').next()?.strip_suffix(".grib2.gz")?;
    let (date, time) = stamp.split_once('-')?;
    let n = |s: &str| s.parse::<i64>().ok();
    let (y, m, d) = (n(&date[0..4])?, n(&date[4..6])?, n(&date[6..8])?);
    let (hh, mm, ss) = (n(&time[0..2])?, n(&time[2..4])?, n(&time[4..6])?);
    Some(days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss)
}

pub(crate) fn time_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// civil date <-> days since 1970-01-01 (Howard Hinnant's algorithms)
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn ymd(t: i64) -> String {
    let (y, m, d) = civil_from_days(t.div_euclid(86_400));
    format!("{y:04}{m:02}{d:02}")
}

fn hms(t: i64) -> String {
    let s = t.rem_euclid(86_400);
    format!("{:02}{:02}{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// A decoded grid: reflectivity in dBZ, north row first; -99 where a radar
/// sees but finds no echo, -999 where no radar sees.
pub struct Grid {
    pub width: usize,
    pub height: usize,
    pub dbz: Vec<f32>,
}

/// GRIB2 stores signed scale factors as sign and magnitude, not two's complement.
fn grib_i16(b: [u8; 2]) -> i32 {
    let v = i32::from(u16::from_be_bytes(b) & 0x7fff);
    if b[0] & 0x80 != 0 { -v } else { v }
}

/// Decodes a single-message GRIB2 file with a regular grid and PNG packing.
pub fn decode_grib(grib: &[u8]) -> Result<Grid, Error> {
    if grib.get(..4) != Some(b"GRIB") || grib.get(7) != Some(&2) {
        return Err("not a GRIB2 file".into());
    }
    let (mut width, mut height) = (0usize, 0usize);
    let mut scale: Option<(f32, i32, i32)> = None;
    let mut i = 16;
    while i + 5 <= grib.len() && &grib[i..i + 4] != b"7777" {
        let len = u32::from_be_bytes(grib[i..i + 4].try_into()?) as usize;
        // a declared length under five bytes still slices fine but has no
        // section number to read, and zero would spin here forever
        let section = grib
            .get(i..i + len)
            .filter(|s| s.len() >= 5)
            .ok_or("bad GRIB2 section length")?;
        match section[4] {
            3 => {
                let dims = section
                    .get(30..38)
                    .ok_or("truncated GRIB2 grid definition")?;
                width = u32::from_be_bytes(dims[0..4].try_into()?) as usize;
                height = u32::from_be_bytes(dims[4..8].try_into()?) as usize;
            }
            5 => {
                let packing = section
                    .get(9..19)
                    .ok_or("truncated GRIB2 data representation")?;
                let template = u16::from_be_bytes(packing[0..2].try_into()?);
                if template != 41 {
                    return Err(format!("unsupported GRIB2 packing template 5.{template}").into());
                }
                let reference = f32::from_be_bytes(packing[2..6].try_into()?);
                let binary = grib_i16([packing[6], packing[7]]);
                let decimal = grib_i16([packing[8], packing[9]]);
                scale = Some((reference, binary, decimal));
            }
            7 => {
                let (reference, binary, decimal) = scale.ok_or("data before its representation")?;
                let png = image::load_from_memory_with_format(&section[5..], ImageFormat::Png)?;
                let raw = png.to_luma16();
                if raw.width() as usize != width || raw.height() as usize != height {
                    return Err("grid and image sizes differ".into());
                }
                let (b, d) = (2f32.powi(binary), 10f32.powi(decimal));
                let dbz = raw
                    .into_raw()
                    .into_iter()
                    .map(|x| (reference + f32::from(x) * b) / d)
                    .collect();
                return Ok(Grid { width, height, dbz });
            }
            _ => {}
        }
        i += len;
    }
    Err("no data section".into())
}

/// Downloads one height and reduces it, so the grid never leaves this task.
async fn fetch_level(http: &reqwest::Client, km: f32) -> Result<Reduced, Error> {
    let key = latest_key(http, &product(km)).await?;
    let gz = http
        .get(format!("{BUCKET}/{key}"))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let time = key_time(&key).unwrap_or(0);
    // decoding, downsampling and contouring are all CPU-bound, so they belong
    // off the async executor; doing them together also means the grid is
    // freed here rather than being handed back to the caller
    tokio::task::spawn_blocking(move || -> Result<Reduced, Error> {
        let mut grib = Vec::new();
        flate2::read::GzDecoder::new(&gz[..]).read_to_end(&mut grib)?;
        let grid = decode_grib(&grib)?;
        drop(grib);
        let (gw, gh, g) = block_max(&grid, GROUND_STEP);
        let (cw, ch, c) = block_max(&grid, COVERAGE_STEP);
        let (lw, lh, l) = block_max(&grid, LEVEL_STEP);
        drop(grid);
        let shapes = radar::contour(lw, lh, &l, BOUNDS, LIFTED_MIN_DBZ);
        // a radar sees a cell if it reported anything at all there
        let seen = c.iter().map(|&v| u8::from(v > -998.0)).collect();
        Ok(Reduced {
            time,
            ground: (gw, gh, g),
            seen: (cw, ch, seen),
            shapes,
        })
    })
    .await?
}

/// Strongest value in each `step`×`step` block: storm cores survive downsampling.
fn block_max(grid: &Grid, step: usize) -> (usize, usize, Vec<f32>) {
    let (w, h) = (grid.width / step, grid.height / step);
    let mut out = vec![f32::NEG_INFINITY; w * h];
    for y in 0..h * step {
        let row = &grid.dbz[y * grid.width..y * grid.width + w * step];
        let o = (y / step) * w;
        for (x, &v) in row.iter().enumerate() {
            let cell = &mut out[o + x / step];
            if v > *cell {
                *cell = v;
            }
        }
    }
    (w, h, out)
}

/// Downloads and reduces the newest volume. Only the heights actually in
/// flight hold a full grid: what comes back here is already small, so peak
/// memory is set by the concurrency limit and not by the number of heights.
pub async fn fetch(http: &reqwest::Client) -> Result<Volume, Error> {
    let mut results: Vec<(usize, LevelResult)> =
        futures_util::stream::iter(LEVELS_KM.into_iter().enumerate().map(|(i, km)| {
            let http = http.clone();
            async move { (i, fetch_level(&http, km).await) }
        }))
        .buffer_unordered(4)
        .collect()
        .await;
    results.sort_by_key(|(i, _)| *i);

    let mut time = 0;
    let mut ground: Option<(usize, usize, Vec<f32>)> = None;
    let mut coverage: Option<(usize, usize, Vec<u8>)> = None;
    let mut levels = Vec::with_capacity(LEVELS_KM.len());
    for ((_, result), &km) in results.into_iter().zip(LEVELS_KM.iter()) {
        let level = match result {
            Ok(level) => level,
            Err(e) => {
                eprintln!("MRMS {km} km: {e}");
                levels.push((km, None));
                continue;
            }
        };
        time = time.max(level.time);
        let (gw, gh, g) = level.ground;
        // the ground shows the strongest echo at any height
        match &mut ground {
            Some((_, _, all)) => all.iter_mut().zip(g).for_each(|(a, v)| *a = a.max(v)),
            None => ground = Some((gw, gh, g)),
        }
        let (cw, ch, s) = level.seen;
        // a radar sees a cell if any height has data there
        match &mut coverage {
            Some((_, _, all)) => all.iter_mut().zip(s).for_each(|(a, s)| *a |= s),
            None => coverage = Some((cw, ch, s)),
        }
        levels.push((km, level.shapes));
    }
    let (gw, gh, g) = ground.ok_or("no MRMS heights could be loaded")?;
    let ground =
        tokio::task::spawn_blocking(move || radar::contour(gw, gh, &g, BOUNDS, GROUND_MIN_DBZ))
            .await?;
    let (cw, ch, coverage) = coverage.unwrap_or_default();
    Ok(Volume {
        source: "mrms",
        bounds: BOUNDS,
        time,
        fetched: Instant::now(),
        ground: ground.unwrap_or_else(radar::no_shapes),
        levels,
        coverage,
        coverage_size: (cw as u32, ch as u32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip() {
        let t =
            key_time("CONUS/X/20261007/MRMS_MergedReflectivityQC_03.00_20261007-175439.grib2.gz")
                .unwrap();
        assert_eq!(t, 1_791_395_679);
        assert_eq!(ymd(t), "20261007");
        assert_eq!(hms(t), "175439");
    }

    #[test]
    fn sign_magnitude() {
        assert_eq!(grib_i16([0x00, 0x01]), 1);
        assert_eq!(grib_i16([0x80, 0x03]), -3);
    }

    /// Fetches the live volume: `cargo test -p rustradar mrms -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_volume() {
        let started = Instant::now();
        let frame = fetch(&reqwest::Client::new()).await.unwrap();
        let info = frame.info();
        println!(
            "time {} heights {:?} ground {} KB coverage {}x{} in {:?}",
            info.time,
            info.levels,
            frame.ground.len() / 1024,
            info.coverage_width,
            info.coverage_height,
            started.elapsed()
        );
        frame.print_layers();
        assert!(frame.coverage.contains(&1));
    }
}
