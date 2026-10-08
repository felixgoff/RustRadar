//! Shared ground for the 3D weather radar.
//!
//! Each source turns its native product into a [`Volume`]: a flat composite
//! for the ground, one coloured image per height, and a mask of where its
//! radars actually see. The frontend stacks the images so storms stand up.
//! Sources that measure reflectivity aloft (NOAA MRMS over North America,
//! DWD over central Europe) give real heights; everywhere else the globe
//! falls back to RainViewer with heights estimated from intensity.

use std::io::Cursor;
use std::time::{Duration, Instant};

use image::{ImageFormat, RgbaImage};
use serde::Serialize;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// MRMS publishes every two minutes and DWD every five, but a volume is tens
/// of megabytes and nothing on a flight tracker turns on eight-minute-old
/// rain, so trade freshness for bandwidth.
pub const MAX_AGE: Duration = Duration::from_secs(480);

/// Weakest echo drawn on the ground, and up in the air: aloft only moderate
/// rain and stronger, so widespread light rain stays a flat sheet and storm
/// cores stand out as columns.
pub const GROUND_MIN_DBZ: f32 = 5.0;
pub const LIFTED_MIN_DBZ: f32 = 25.0;

#[derive(Clone, Copy, Serialize)]
pub struct Bounds {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

/// One source's current scan, reduced to what the globe draws.
pub struct Volume {
    /// Short name the frontend asks for images by, e.g. `mrms`.
    pub source: &'static str,
    /// Unix seconds of the newest height's scan.
    pub time: i64,
    pub fetched: Instant,
    pub bounds: Bounds,
    /// PNG: the strongest echo at any height, for the ground.
    pub ground: Vec<u8>,
    /// PNG per height (km), lowest first; `None` where nothing reaches it.
    pub levels: Vec<(f32, Option<Vec<u8>>)>,
    /// One byte per coverage cell, north row first: 1 where a radar sees.
    pub coverage: Vec<u8>,
    pub coverage_size: (u32, u32),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeInfo {
    pub source: &'static str,
    pub time: i64,
    pub bounds: Bounds,
    /// Heights that have an image, km.
    pub levels: Vec<f32>,
    pub coverage_width: u32,
    pub coverage_height: u32,
}

impl Volume {
    pub fn info(&self) -> VolumeInfo {
        VolumeInfo {
            source: self.source,
            time: self.time,
            bounds: self.bounds,
            levels: self
                .levels
                .iter()
                .filter(|(_, png)| png.is_some())
                .map(|(km, _)| *km)
                .collect(),
            coverage_width: self.coverage_size.0,
            coverage_height: self.coverage_size.1,
        }
    }

    /// `ground`, `coverage`, or a height in km such as `3`.
    pub fn image(&self, name: &str) -> Option<Vec<u8>> {
        match name {
            "ground" => Some(self.ground.clone()),
            "coverage" => Some(self.coverage.clone()),
            km => {
                let km: f32 = km.parse().ok()?;
                self.levels
                    .iter()
                    .find(|(k, _)| (*k - km).abs() < 0.01)
                    .and_then(|(_, png)| png.clone())
            }
        }
    }
}

/// The usual NWS reflectivity colours, in 5 dBZ steps from 5 dBZ.
const PALETTE: [[u8; 3]; 14] = [
    [0x04, 0xe9, 0xe7],
    [0x01, 0x9f, 0xf4],
    [0x03, 0x00, 0xf4],
    [0x02, 0xfd, 0x02],
    [0x01, 0xc5, 0x01],
    [0x00, 0x8e, 0x00],
    [0xfd, 0xf8, 0x02],
    [0xe5, 0xbc, 0x00],
    [0xfd, 0x95, 0x00],
    [0xfd, 0x00, 0x00],
    [0xd4, 0x00, 0x00],
    [0xbc, 0x00, 0x00],
    [0xf8, 0x00, 0xfd],
    [0x98, 0x54, 0xc6],
];

pub fn color(dbz: f32) -> [u8; 4] {
    let i = (((dbz - 5.0) / 5.0).floor() as usize).min(PALETTE.len() - 1);
    let [r, g, b] = PALETTE[i];
    // light echoes are translucent, so the map shows through drizzle
    let alpha = (150.0 + (dbz - 5.0) * 4.0).clamp(150.0, 235.0) as u8;
    [r, g, b, alpha]
}

/// Colours a grid of dBZ into a PNG, or `None` when nothing reaches `min_dbz`.
pub fn encode(w: usize, h: usize, values: &[f32], min_dbz: f32) -> Result<Option<Vec<u8>>, Error> {
    if !values.iter().any(|&v| v >= min_dbz) {
        return Ok(None);
    }
    let mut img = RgbaImage::new(w as u32, h as u32);
    for (px, &v) in img.pixels_mut().zip(values) {
        if v >= min_dbz {
            px.0 = color(v);
        }
    }
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
    Ok(Some(png))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_steps() {
        assert_eq!(color(5.0)[..3], PALETTE[0]);
        assert_eq!(color(52.0)[..3], PALETTE[9]);
        assert_eq!(color(90.0)[..3], PALETTE[13]);
    }

    #[test]
    fn empty_levels_encode_to_nothing() {
        let quiet = vec![-99.0f32; 16];
        assert!(encode(4, 4, &quiet, LIFTED_MIN_DBZ).unwrap().is_none());
    }
}
