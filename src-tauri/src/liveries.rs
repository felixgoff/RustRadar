//! Livery colours for airlines without a hand-made livery: the dominant
//! colours of the airline's logo, which usually match its tail.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogoColors {
    pub primary: [u8; 3],
    pub secondary: Option<[u8; 3]>,
}

/// Cached results per IATA code; `None` means the airline has no usable logo.
pub type ColorCache = HashMap<String, Option<LogoColors>>;

pub fn is_iata(code: &str) -> bool {
    code.len() == 2
        && code
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

pub fn read_cache(path: &Path) -> ColorCache {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn write_cache(path: &Path, cache: &ColorCache) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(bytes) = serde_json::to_vec(cache) {
        let _ = std::fs::write(path, bytes);
    }
}

/// Square airline logos keyed by IATA code.
pub async fn fetch(http: &reqwest::Client, iata: &str) -> Option<LogoColors> {
    let url = format!("https://pics.avs.io/120/120/{iata}.png");
    let response = http.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    dominant_colors(&response.bytes().await.ok()?)
}

/// The two most prominent distinct colours of a logo, ignoring transparency
/// and the white or light grey backgrounds logos sit on. Saturated pixels
/// count for more, so a brand colour beats a thin dark outline.
pub fn dominant_colors(png: &[u8]) -> Option<LogoColors> {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    let mut buckets: HashMap<u16, (f32, [f32; 3])> = HashMap::new();
    for pixel in image.pixels() {
        let [r, g, b, a] = pixel.0;
        if a < 200 {
            continue;
        }
        let max = f32::from(r.max(g).max(b));
        let min = f32::from(r.min(g).min(b));
        let saturation = if max > 0.0 { (max - min) / max } else { 0.0 };
        if min > 215.0 || (saturation < 0.15 && max > 120.0) {
            continue;
        }
        let weight = 0.4 + saturation;
        // 3 bits per channel: gradients and anti-aliasing land in one bucket
        let key = (u16::from(r >> 5) << 6) | (u16::from(g >> 5) << 3) | u16::from(b >> 5);
        let entry = buckets.entry(key).or_insert((0.0, [0.0; 3]));
        entry.0 += weight;
        entry.1[0] += f32::from(r) * weight;
        entry.1[1] += f32::from(g) * weight;
        entry.1[2] += f32::from(b) * weight;
    }
    let mut ranked: Vec<(f32, [f32; 3])> = buckets
        .into_values()
        .map(|(weight, sum)| (weight, sum.map(|c| c / weight)))
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    let &(primary_weight, primary) = ranked.first()?;
    let distance = |a: [f32; 3], b: [f32; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };
    let secondary = ranked
        .iter()
        .skip(1)
        .find(|(weight, color)| *weight > primary_weight * 0.12 && distance(*color, primary) > 90.0)
        .map(|(_, color)| *color);
    let to_u8 = |c: [f32; 3]| c.map(|v| v.round().clamp(0.0, 255.0) as u8);
    Some(LogoColors {
        primary: to_u8(primary),
        secondary: secondary.map(to_u8),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(pixels: &[[u8; 4]], width: u32) -> Vec<u8> {
        let height = pixels.len() as u32 / width;
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba(pixels[(y * width + x) as usize])
        });
        let mut out = std::io::Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn ignores_background_and_finds_two_colours() {
        let white = [255, 255, 255, 255];
        let clear = [0, 0, 0, 0];
        let orange = [255, 102, 0, 255];
        let navy = [5, 22, 77, 255];
        let mut pixels = vec![white; 40];
        pixels.extend(vec![clear; 20]);
        pixels.extend(vec![orange; 25]);
        pixels.extend(vec![navy; 15]);
        let colors = dominant_colors(&png(&pixels, 10)).unwrap();
        assert_eq!(colors.primary, [255, 102, 0]);
        assert_eq!(colors.secondary, Some([5, 22, 77]));
    }

    #[test]
    fn iata_codes() {
        assert!(is_iata("LH"));
        assert!(is_iata("U2"));
        assert!(!is_iata("lh"));
        assert!(!is_iata("DLH"));
    }

    /// Network: `cargo test -p rustradar -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn real_logos() {
        let http = reqwest::Client::new();
        for iata in [
            "LH", "FR", "U2", "EK", "QR", "BA", "AF", "KL", "TK", "W6", "DL", "UA",
        ] {
            println!("{iata}: {:?}", fetch(&http, iata).await);
        }
    }
}
