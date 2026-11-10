//! The pure part of `terrain-layer.ts` (doc 04 §14, doc 05 §13.5): terrarium heights, the
//! hillshade raster drawn under the map on every basemap, what its tile layer and the 3D surface
//! are made of, and the ground's height under the camera.
//!
//! Elevation comes from the keyless AWS Open Data "Terrain Tiles" (Mapzen / Tilezen Joerd),
//! terrarium PNG encoding, z0-15: `height (m) = R * 256 + G + B / 256 - 32768`.
//!
//! The hillshade is a raster at every zoom, in both projections: each terrarium tile is decoded
//! and shaded on the CPU (about a millisecond for a 256 px tile) into a transparent image that
//! darkens slopes facing away from the light and faintly lightens those facing it. Flat ground
//! comes out fully transparent, so plains, the sea and tile edges leave the map untouched.
//!
//! The 3D surface is drawn as deck.gl's terrain was: the map's other layers stand on it, the flat
//! ones draped over it, the rest raised by the ground's height under them (doc 04 §14.3, §14.5).
//! What deck.gl's layers were configured with is kept here as plain descriptions
//! ([`HillshadeLayer`], [`TerrainSurface`]) for the map crate to draw from.
//!
//! Transliterated as written: heights are `f32` where the TypeScript kept them in a
//! `Float32Array`, the RGBA output is stored as a `Uint8ClampedArray` stores it
//! ([`clamp_u8`]: clamped, ties to even), `Math.round` is [`js_round`], and `Math.min`/`Math.max`
//! are the private `js_min`/`js_max`, which keep NaN as JavaScript does. A read past the end of
//! an input reads NaN, where JavaScript reads `undefined`. The TypeScript's module-level scratch
//! buffer (`seaLevel`) is a local here.

use indexmap::IndexMap;
use rustradar_core::geo::{Basemap, Rgb};
use rustradar_core::num::{clamp01, clamp_u8, js_round};

/// Terrarium-encoded elevation tiles, keyless (AWS Open Data, Mapzen Terrain Tiles).
pub const TERRARIUM_URL: &str = "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png";
/// The deepest zoom the terrarium tiles exist at; deeper tiles are overzoomed.
pub const TERRARIUM_MAX_ZOOM: u32 = 15;
/// The 3D surface's heights: Mapterhorn's (mapterhorn.com), terrarium in 512 px WebP: Copernicus
/// GLO-30 (30 m) everywhere to zoom 12, and deeper the national lidar models it has (Sweden's:
/// Lantmäteriet's 1 m, to zoom 16; Norway's, Denmark's, Finland's, much of Europe's). Where it
/// has none, its deeper tiles are missing (404) and Mapzen's stand in
/// ([`TerrainSurface::fallback_url`]).
pub const MAPTERHORN_URL: &str = "https://tiles.mapterhorn.com/{z}/{x}/{y}.webp";

/// deck.gl's `elevationDecoder`: `height = r * r_scaler + g * g_scaler + b * b_scaler + offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElevationDecoder {
    pub r_scaler: f64,
    pub g_scaler: f64,
    pub b_scaler: f64,
    pub offset: f64,
}

/// The `elevationDecoder` for terrarium tiles.
pub const TERRARIUM_DECODER: ElevationDecoder = ElevationDecoder { r_scaler: 256.0, g_scaler: 1.0, b_scaler: 1.0 / 256.0, offset: -32768.0 };

/// The attribution the Terrain Tiles licence asks for (tilezen/joerd docs/attribution.md, "when
/// using Mapzen's hosted service"), shortened to one line for the map's attribution strip. The
/// full list is in [`TERRAIN_ATTRIBUTION_FULL`], for an about/credits view.
pub const TERRAIN_ATTRIBUTION: &str = "Terrain: © Mapterhorn (Lantmäteriet, Kartverket, Copernicus GLO-30 and others); Mapzen Terrain Tiles (AWS Open Data): USGS 3DEP, SRTM, GMTED2010; NOAA ETOPO1; Copernicus EU-DEM; and others";
/// The Terrain Tiles' full attribution, line by line.
pub const TERRAIN_ATTRIBUTION_FULL: [&str; 12] = [
    "Mapzen",
    "ArcticDEM terrain data DEM(s) were created from DigitalGlobe, Inc., imagery and funded under National Science Foundation awards 1043681, 1559691, and 1542736",
    "Australia terrain data © Commonwealth of Australia (Geoscience Australia) 2017",
    "Austria terrain data © offene Daten Österreichs – Digitales Geländemodell (DGM) Österreich",
    "Canada terrain data contains information licensed under the Open Government Licence – Canada",
    "Europe terrain data produced using Copernicus data and information funded by the European Union - EU-DEM layers",
    "Global ETOPO1 terrain data U.S. National Oceanic and Atmospheric Administration",
    "Mexico terrain data source: INEGI, Continental relief, 2016",
    "New Zealand terrain data Copyright 2011 Crown copyright (c) Land Information New Zealand and the New Zealand Government (All rights reserved)",
    "Norway terrain data © Kartverket",
    "United Kingdom terrain data © Environment Agency copyright and/or database right 2015. All rights reserved",
    "United States 3DEP (formerly NED) and global GMTED2010 and SRTM terrain data courtesy of the U.S. Geological Survey",
];

/// The hillshade's tile size in pixels, and the default row count of
/// [`tile_row_meters_per_pixel`].
pub const TILE_SIZE: usize = 256;
/// Web Mercator's equator, metres.
const EARTH_CIRCUMFERENCE: f64 = 2.0 * std::f64::consts::PI * 6_378_137.0;
/// `Math.PI / 180`.
const DEG: f64 = std::f64::consts::PI / 180.0;

/// `Math.min(a, b)`: NaN when either is NaN, and -0 below +0.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b || (a == b && a.is_sign_negative()) {
        a
    } else {
        b
    }
}

/// `Math.max(a, b)`: NaN when either is NaN, and +0 above -0.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b || (a == b && a.is_sign_positive()) {
        a
    } else {
        b
    }
}

/// Heights in metres from terrarium RGBA pixels (`decodeTerrarium`). `out` is reused when it
/// holds at least one height per pixel (and comes back as long as it was); else a new buffer.
pub fn decode_terrarium(rgba: &[u8], out: Option<Vec<f32>>) -> Vec<f32> {
    let n = rgba.len() >> 2;
    let mut heights = match out {
        Some(out) if out.len() >= n => out,
        _ => vec![0.0; n],
    };
    for (i, px) in rgba.chunks_exact(4).enumerate() {
        heights[i] =
            (f64::from(px[0]) * 256.0 + f64::from(px[1]) + f64::from(px[2]) / 256.0 - 32768.0) as f32;
    }
    heights
}

/// Ground metres per pixel of tile row `row` (of `size`, [`TILE_SIZE`] in the TypeScript's
/// default) in Web Mercator tile `z`/`y`.
pub fn tile_row_meters_per_pixel(z: u32, y: u32, row: usize, size: usize) -> f64 {
    let n = 2f64.powi(z as i32);
    let size = size as f64;
    let merc_y = (f64::from(y) + (row as f64 + 0.5) / size) / n; // 0 at the top (north), 1 at the bottom
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * merc_y)).sinh().atan();
    (EARTH_CIRCUMFERENCE * lat.cos()) / (size * n)
}

/// Vertical exaggeration for the shading at a tile zoom. Zoomed out, a pixel spans kilometres and
/// averages the slopes away; exaggerating them keeps the relief legible on the globe without
/// shouting once zoomed in.
pub fn relief_exaggeration(z: f64) -> f64 {
    js_min(8.0, js_max(1.0, 2f64.powf((11.0 - z) / 3.0)))
}

/// How the hillshade is drawn (`HillshadeStyle`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HillshadeStyle {
    /// Direction the light comes from, degrees clockwise from north (cartographic default: 315,
    /// the north-west).
    pub azimuth: f64,
    /// Height of the light above the horizon, degrees.
    pub altitude: f64,
    /// Vertical exaggeration.
    pub z_factor: f64,
    /// Colour of the shadows, and how dark the darkest gets (0-255 alpha).
    pub shadow: Rgb,
    pub shadow_alpha: f64,
    /// Colour of the lit slopes, and their strongest alpha.
    pub highlight: Rgb,
    pub highlight_alpha: f64,
    /// Shade difference from flat ground (0-1) that reaches the full alpha.
    pub gain: f64,
}

/// Tuned for the dark map: deep navy shadows, a faint warm light on the sunny side. The
/// TypeScript's `Partial<HillshadeStyle>` overrides are `HillshadeStyle { z_factor, ..HILLSHADE_STYLE }`.
pub const HILLSHADE_STYLE: HillshadeStyle = HillshadeStyle {
    azimuth: 315.0,
    altitude: 45.0,
    z_factor: 1.0,
    shadow: [4, 8, 16],
    shadow_alpha: 135.0,
    highlight: [255, 244, 225],
    highlight_alpha: 40.0,
    gain: 0.55,
};

/// A hillshade of a `width` x `height` grid of heights (metres, row 0 north), as RGBA: shadow
/// colour where a slope faces away from the light, highlight colour where it faces it, alpha
/// proportional to how much it differs from flat ground (flat ground is fully transparent). Below
/// sea level counts as sea level, so the sea floor isn't shaded. `meters_per_pixel(row)` is the
/// ground size of a pixel in that row. Horn's 3x3 gradient; at the edges it uses the neighbours
/// there are, so tiles meet without a seam.
///
/// `out` is reused when it holds at least `width * height * 4` bytes (and comes back as long as it
/// was); else a new buffer. The output is not premultiplied.
pub fn hillshade(
    heights: &[f32],
    width: usize,
    height: usize,
    meters_per_pixel: impl Fn(usize) -> f64,
    style: &HillshadeStyle,
    out: Option<Vec<u8>>,
) -> Vec<u8> {
    let s = style;
    let mut rgba = match out {
        Some(out) if out.len() >= width * height * 4 => out,
        _ => vec![0; width * height * 4],
    };
    let az = s.azimuth * DEG;
    let alt = s.altitude * DEG;
    // towards the light, in east, north, up
    let lx = az.sin() * alt.cos();
    let ly = az.cos() * alt.cos();
    let lz = alt.sin();
    let flat = lz;
    // below sea level counts as sea level
    let n = width * height;
    let z: Vec<f32> =
        (0..n).map(|i| js_max(0.0, heights.get(i).map_or(f64::NAN, |&h| f64::from(h))) as f32).collect();
    let z = |i: usize| f64::from(z[i]);
    let [sr, sg, sb] = s.shadow;
    let [hr, hg, hb] = s.highlight;
    for y in 0..height {
        let rn = (if y > 0 { y - 1 } else { y }) * width;
        let rc = y * width;
        let rs = (if y < height - 1 { y + 1 } else { y }) * width;
        let y_span = f64::from(u8::from(y > 0) + u8::from(y < height - 1));
        let mpp = meters_per_pixel(y) / s.z_factor;
        let y_scale = 1.0 / (4.0 * y_span * mpp);
        for x in 0..width {
            let xw = if x > 0 { x - 1 } else { x };
            let xe = if x < width - 1 { x + 1 } else { x };
            // Horn: weighted differences across the 3x3 neighbourhood
            let dzdx = (z(rn + xe) + 2.0 * z(rc + xe) + z(rs + xe) - z(rn + xw) - 2.0 * z(rc + xw) - z(rs + xw))
                / (4.0 * (xe - xw) as f64 * mpp);
            // rows run south: the gradient towards the north is the negative of the row's
            let dzdy = (z(rn + xw) + 2.0 * z(rn + x) + z(rn + xe) - z(rs + xw) - 2.0 * z(rs + x) - z(rs + xe)) * y_scale;
            // the surface normal (-dz/dx, -dz/dy, 1), normalised, against the light
            let shade = (-dzdx * lx - dzdy * ly + lz) / (dzdx * dzdx + dzdy * dzdy + 1.0).sqrt();
            let d = (shade - flat) / s.gain;
            let i = (y * width + x) * 4;
            if d < 0.0 {
                rgba[i] = sr;
                rgba[i + 1] = sg;
                rgba[i + 2] = sb;
                rgba[i + 3] = clamp_u8(js_min(1.0, -d) * s.shadow_alpha);
            } else {
                rgba[i] = hr;
                rgba[i + 1] = hg;
                rgba[i + 2] = hb;
                rgba[i + 3] = clamp_u8(js_min(1.0, d) * s.highlight_alpha);
            }
        }
    }
    rgba
}

/// The zoom the ground's height under the camera is read at (GLO-30's: 30 m, plenty for where
/// to aim).
pub const GROUND_ZOOM: u32 = 12;
/// How many of [`GROUND_ZOOM`]'s tiles [`GroundHeights`] keeps: a handful is all a camera needs.
pub const GROUND_TILES_KEPT: usize = 16;

/// A height tile [`GroundHeights`] wants: Mapterhorn's [`MAPTERHORN_URL`] at `z`/`x`/`y`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroundTile {
    pub z: u32,
    pub x: i64,
    pub y: i64,
}

/// The ground's height at a place, from Mapterhorn's zoom-12 tiles (`groundHeight`, doc 04
/// §14.4). The TypeScript's module-level `groundTiles` map, without the fetching: the caller
/// fetches the tiles [`GroundHeights::height`] asks for, decodes them with [`decode_terrarium`]
/// and hands them back with [`GroundHeights::loaded`] (then redraws, the TypeScript's `onLoad`).
/// A tile that fails is never handed back and stays wanted-but-missing until it is evicted.
#[derive(Debug, Clone, Default)]
pub struct GroundHeights {
    /// By `(x, y)` in insertion order (`None`: none there, or loading); the oldest goes first.
    tiles: IndexMap<(i64, i64), Option<Vec<f32>>>,
}

impl GroundHeights {
    pub fn new() -> Self {
        Self::default()
    }

    /// The ground's height at a place, metres, never below 0; `None` until its tile has loaded.
    /// The first time a tile is needed (or again after it was evicted), `request` is called with
    /// it, and the oldest tile beyond [`GROUND_TILES_KEPT`] is dropped.
    pub fn height(&mut self, lon: f64, lat: f64, request: impl FnOnce(GroundTile)) -> Option<f64> {
        let n = 2f64.powi(GROUND_ZOOM as i32);
        let r = (js_max(-85.0, js_min(85.0, lat)) * std::f64::consts::PI) / 180.0;
        let x = (((lon + 180.0) / 360.0) % 1.0) * n;
        let y = ((1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0) * n;
        let key = (x.floor() as i64, y.floor() as i64);
        if !self.tiles.contains_key(&key) {
            self.tiles.insert(key, None);
            // a handful is all a camera needs
            if self.tiles.len() > GROUND_TILES_KEPT {
                self.tiles.shift_remove_index(0);
            }
            request(GroundTile { z: GROUND_ZOOM, x: key.0, y: key.1 });
        }
        let heights = self.tiles.get(&key)?.as_ref()?;
        let size = js_round((heights.len() as f64).sqrt());
        let px = js_min(size - 1.0, ((x % 1.0) * size).floor());
        let py = js_min(size - 1.0, ((y % 1.0) * size).floor());
        let at = py * size + px;
        let h = if at >= 0.0 { heights.get(at as usize).map_or(f64::NAN, |&h| f64::from(h)) } else { f64::NAN };
        Some(js_max(0.0, h))
    }

    /// A tile's heights, fetched and decoded. Kept even when the tile was evicted meanwhile (it
    /// then counts as the newest), as the TypeScript's `Map.set` does.
    pub fn loaded(&mut self, tile: GroundTile, heights: Vec<f32>) {
        self.tiles.insert((tile.x, tile.y), Some(heights));
    }

    /// How many tiles are kept, loaded or not.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

/// What a layer's look adds on top of its own: the deck.gl layer extensions the TypeScript passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Extension {
    /// `SeaLevelExtension`: the 3D surface no lower than the sea (`size.z = max(size.z, 0.0)` on
    /// its vertices). The tiles have the sea floor, which the sea's surface (the globe, the map's
    /// water) would otherwise hide.
    SeaLevel,
    /// The realistic map's haze over a ground layer (Globe.svelte's `HAZE_FADE`): its alpha times
    /// the air's transmittance.
    HazeFade,
    /// The realistic map's air over the 3D surface (Globe.svelte's `TERRAIN_AIR`).
    TerrainAir,
}

/// The hillshade's deepest tiles. Most of the source data is ~30 m (SRTM, EU-DEM), about a z12
/// pixel at mid latitudes: deeper tiles are resampled and shade into terraces and stripes, while
/// a z12 tile stretched further stays smooth.
pub const HILLSHADE_MAX_ZOOM: u32 = 12;

/// Hillshade opacity at a view zoom: full out to zoom 11, easing to 0.6 by 14, where a single
/// slope fills the screen and would otherwise darken all of it.
pub fn hillshade_fade(zoom: f64) -> f64 {
    1.0 - 0.4 * clamp01((zoom - 11.0) / 3.0)
}

/// Hillshade strength per basemap (`HILLSHADE_OPACITY`): the imagery has its own sunlit relief and
/// needs less; the realistic ground's light colours need less to show it.
pub fn hillshade_opacity(basemap: Basemap) -> f64 {
    match basemap {
        Basemap::Dark => 1.0,
        Basemap::Satellite => 0.45,
        Basemap::Realistic => 0.6,
    }
}

/// The shaded relief raster (`hillshadeLayer`): one layer and tile cache for both projections.
/// Each tile is [`TERRARIUM_URL`]'s, shaded by [`hillshade`] with
/// `z_factor: relief_exaggeration(z)` and [`tile_row_meters_per_pixel`], and drawn as a bitmap
/// over the tile's bounds (a Web Mercator image, reprojected onto the globe as the imagery is).
/// Ground layers paint in layer order and don't write depth.
#[derive(Debug, Clone, PartialEq)]
pub struct HillshadeLayer {
    pub id: &'static str,
    pub url: &'static str,
    pub min_zoom: u32,
    pub max_zoom: u32,
    pub tile_size: usize,
    pub max_requests: usize,
    /// Shaded tiles kept: refetching is the expensive part.
    pub max_cache_size: usize,
    pub opacity: f64,
    pub extensions: Vec<Extension>,
}

/// The hillshade layer at an opacity (`hillshadeLayer`).
pub fn hillshade_layer(opacity: f64, extensions: &[Extension]) -> HillshadeLayer {
    HillshadeLayer {
        id: "terrain-hillshade",
        url: TERRARIUM_URL,
        min_zoom: 0,
        max_zoom: HILLSHADE_MAX_ZOOM,
        tile_size: TILE_SIZE,
        max_requests: 8,
        max_cache_size: 300,
        opacity,
        extensions: extensions.to_vec(),
    }
}

/// What the terrain layers are for (`TerrainOptions`).
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainOptions {
    /// GlobeView's flat Web Mercator projection is in use (zoom above 12).
    pub flat: bool,
    /// The view's zoom.
    pub zoom: f64,
    pub basemap: Basemap,
    /// Hillshade strength, 0-1, on top of the basemap's own (1 in the TypeScript's default).
    pub opacity: f64,
    /// Layer extensions for the hillshade (the realistic map's haze).
    pub extensions: Vec<Extension>,
}

impl Default for TerrainOptions {
    fn default() -> Self {
        TerrainOptions { flat: false, zoom: 0.0, basemap: Basemap::Dark, opacity: 1.0, extensions: Vec::new() }
    }
}

/// The terrain layers for the current view, in drawing order: the hillshade.
///
/// Insert them right after the basemap's ground (the land on the dark map, the imagery on the
/// satellite one) and before the vector map, so water, borders, roads and runways stay crisp on
/// top of the shading.
pub fn terrain_layers(options: &TerrainOptions) -> Vec<HillshadeLayer> {
    let TerrainOptions { zoom, basemap, opacity, ref extensions, .. } = *options;
    // opacity in steps, so the layer's props don't change with every frame of a zoom
    let fade = js_round(hillshade_fade(zoom) * 20.0) / 20.0;
    vec![hillshade_layer(hillshade_opacity(basemap) * opacity * fade, extensions)]
}

/// The dark map's land, which the untextured 3D surface is painted in.
pub const MESH_COLOR: Rgb = [34, 39, 48];

/// How the 3D surface takes part in drawing (deck.gl's `operation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerrainOperation {
    /// `"terrain+draw"`: drawn, and the terrain the map's other layers stand on.
    TerrainAndDraw,
}

/// The ground in 3D (`terrainSurfaceLayer`, doc 04 §14.3): the elevation as a surface, which the
/// map's other layers stand on (the flat ones draped over it, the rest raised by the height under
/// them). Drawn with what is draped on it; its own colour only shows where nothing is.
///
/// Its tiles are picked by their distance from the camera (`lod`, levels `0..=max_zoom`, bias
/// [`TerrainSurface::lod_bias`]), every zoom to Mapterhorn's deepest (Sweden's 1 m model: about
/// 0.7 m a pixel there), each a texture of what is draped on it: fine close by, coarse to the
/// horizon. The heights its tiles span widen as each loads; until then the tiles are taken to lie
/// at sea level.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainSurface {
    pub id: &'static str,
    pub url: &'static str,
    /// Mapzen's tiles of the same `z`/`x`/`y`, where Mapterhorn has none this deep (it answers
    /// 404): a quarter of the pixels.
    pub fallback_url: &'static str,
    pub decoder: ElevationDecoder,
    pub min_zoom: u32,
    pub max_zoom: u32,
    pub tile_size: usize,
    /// The RTIN mesh's error, metres: the 1 m model's detail, not its noise.
    pub mesh_max_error: f64,
    pub color: Rgb,
    /// Lit by nothing (deck.gl's `material: false`).
    pub material: bool,
    pub operation: TerrainOperation,
    pub max_requests: usize,
    pub lod_bias: f64,
    pub extensions: Vec<Extension>,
}

/// The 3D surface, with [`Extension::SeaLevel`] first and then `extensions` (the realistic map's
/// air).
pub fn terrain_surface(extensions: &[Extension]) -> TerrainSurface {
    let mut all = vec![Extension::SeaLevel];
    all.extend_from_slice(extensions);
    TerrainSurface {
        id: "terrain-surface",
        url: MAPTERHORN_URL,
        fallback_url: TERRARIUM_URL,
        decoder: TERRARIUM_DECODER,
        min_zoom: 0,
        // `SURFACE_LOD.levels.length - 1`: levels 0 to 16
        max_zoom: 16,
        tile_size: 512,
        mesh_max_error: 2.0,
        color: MESH_COLOR,
        material: false,
        operation: TerrainOperation::TerrainAndDraw,
        max_requests: 8,
        lod_bias: 0.5,
        extensions: all,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;

    /// Terrarium RGB for a height, as the tiles encode it.
    fn encode(h: f64) -> [f64; 3] {
        let v = h + 32768.0;
        let r = (v / 256.0).floor();
        let g = (v - r * 256.0).floor();
        let b = js_round((v - v.floor()) * 256.0);
        [r, g, b]
    }

    /// A W x H grid sloping up towards `bearing` (degrees from north) with `grade` (rise per metre).
    fn slope(w: usize, h: usize, grade: f64, bearing: f64) -> Vec<f32> {
        let mpp = 10.0;
        let ex = (bearing * std::f64::consts::PI / 180.0).sin();
        let ny = (bearing * std::f64::consts::PI / 180.0).cos();
        let mut out = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                out[y * w + x] = (1000.0 + grade * mpp * (x as f64 * ex - y as f64 * ny)) as f32;
            }
        }
        out
    }

    fn alpha_at(rgba: &[u8], w: usize, x: usize, y: usize) -> u8 {
        rgba[(y * w + x) * 4 + 3]
    }

    fn is_shadow(rgba: &[u8], w: usize, x: usize, y: usize) -> bool {
        rgba[(y * w + x) * 4] == HILLSHADE_STYLE.shadow[0] && rgba[(y * w + x) * 4 + 1] == HILLSHADE_STYLE.shadow[1]
    }

    fn shade(heights: &[f32], w: usize) -> Vec<u8> {
        hillshade(heights, w, w, |_| 10.0, &HILLSHADE_STYLE, None)
    }

    #[test]
    fn terrarium_pixels_decode_to_metres() {
        let heights = [0.0, 4807.5, -10994.0, 1.25];
        let rgba: Vec<u8> = heights
            .iter()
            .flat_map(|&h| {
                let [r, g, b] = encode(h);
                [clamp_u8(r), clamp_u8(g), clamp_u8(b), 255]
            })
            .collect();
        let out = decode_terrarium(&rgba, None);
        for (i, &h) in heights.iter().enumerate() {
            assert_close!(out[i], h, 2);
        }
        // sea level is 128, 0, 0
        assert_eq!(encode(0.0), [128.0, 0.0, 0.0]);
    }

    #[test]
    fn decoding_reuses_a_buffer_that_is_big_enough() {
        let buffer = vec![0.0_f32; 8];
        let ptr = buffer.as_ptr();
        let back = decode_terrarium(&[0; 16], Some(buffer));
        assert_eq!(back.as_ptr(), ptr);
        assert_eq!(back.len(), 8);
        let other = decode_terrarium(&[0; 64], Some(back));
        assert_ne!(other.as_ptr(), ptr);
        assert_eq!(other.len(), 16);
    }

    #[test]
    fn a_tiles_pixels_are_about_611_m_at_the_equator_at_zoom_8_and_shrink_towards_the_poles() {
        // the row just above the equator in a zoom-8 tile
        assert_close!(tile_row_meters_per_pixel(8, 127, 255, TILE_SIZE), 611.5, 0);
        let z8_alps = tile_row_meters_per_pixel(8, 90, 128, TILE_SIZE); // about 46.5 N
        assert!(z8_alps > 400.0);
        assert!(z8_alps < 430.0);
        assert!(tile_row_meters_per_pixel(8, 90, 0, TILE_SIZE) < tile_row_meters_per_pixel(8, 90, 255, TILE_SIZE));
    }

    #[test]
    fn flat_ground_is_fully_transparent() {
        let rgba = hillshade(&[812.0; 16], 4, 4, |_| 10.0, &HILLSHADE_STYLE, None);
        for a in rgba.iter().skip(3).step_by(4) {
            assert_eq!(*a, 0);
        }
    }

    #[test]
    fn a_slope_facing_the_north_west_light_is_lit_one_facing_away_is_shaded() {
        let w = 5;
        // rising towards the south-east: the face looks north-west, into the light
        let lit = shade(&slope(w, w, 0.5, 135.0), w);
        assert!(!is_shadow(&lit, w, 2, 2));
        assert!(alpha_at(&lit, w, 2, 2) > 0);
        // rising towards the north-west: facing away
        let dark = shade(&slope(w, w, 0.5, 315.0), w);
        assert!(is_shadow(&dark, w, 2, 2));
        assert!(alpha_at(&dark, w, 2, 2) > alpha_at(&lit, w, 2, 2));
    }

    #[test]
    fn steeper_slopes_shade_darker_up_to_the_cap() {
        let w = 5;
        let gentle = alpha_at(&shade(&slope(w, w, 0.1, 315.0), w), w, 2, 2);
        let steep = alpha_at(&shade(&slope(w, w, 0.4, 315.0), w), w, 2, 2);
        let cliff = alpha_at(&shade(&slope(w, w, 20.0, 315.0), w), w, 2, 2);
        assert!(steep > gentle);
        assert!(f64::from(cliff) <= HILLSHADE_STYLE.shadow_alpha);
    }

    #[test]
    fn edge_pixels_shade_like_the_middle_so_tiles_meet_without_a_seam() {
        let w = 6;
        let rgba = shade(&slope(w, w, 0.3, 300.0), w);
        let middle = i32::from(alpha_at(&rgba, w, 2, 2));
        for (x, y) in [(0, 0), (w - 1, 0), (0, w - 1), (w - 1, w - 1), (0, 3), (3, 0)] {
            assert!((i32::from(alpha_at(&rgba, w, x, y)) - middle).abs() <= 1);
        }
    }

    #[test]
    fn the_sea_floor_isnt_shaded() {
        let w = 5;
        let seabed: Vec<f32> = slope(w, w, 0.5, 315.0).iter().map(|&h| (f64::from(h) - 5000.0) as f32).collect();
        let rgba = shade(&seabed, w);
        for a in rgba.iter().skip(3).step_by(4) {
            assert_eq!(*a, 0);
        }
    }

    #[test]
    fn zoomed_out_relief_is_exaggerated_more() {
        assert!(relief_exaggeration(3.0) > relief_exaggeration(8.0));
        assert!(relief_exaggeration(8.0) > relief_exaggeration(14.0));
        assert!(relief_exaggeration(0.0) <= 8.0);
        assert_eq!(relief_exaggeration(15.0), 1.0);
    }

    #[test]
    fn the_hillshade_at_every_zoom() {
        let ids = |o: TerrainOptions| terrain_layers(&o).iter().map(|l| l.id).collect::<Vec<_>>();
        assert_eq!(ids(TerrainOptions { flat: false, zoom: 3.0, basemap: Basemap::Dark, ..TerrainOptions::default() }), ["terrain-hillshade"]);
        assert_eq!(ids(TerrainOptions { flat: true, zoom: 14.0, basemap: Basemap::Dark, ..TerrainOptions::default() }), ["terrain-hillshade"]);
    }

    #[test]
    fn the_3d_surface_is_deck_gls_terrain_never_below_the_sea() {
        let surface = terrain_surface(&[]);
        assert_eq!(surface.operation, TerrainOperation::TerrainAndDraw);
        assert_eq!(surface.extensions, [Extension::SeaLevel]);
    }

    #[test]
    fn the_hillshade_is_fainter_over_the_imagery() {
        let dark = &terrain_layers(&TerrainOptions { flat: false, zoom: 8.0, basemap: Basemap::Dark, ..TerrainOptions::default() })[0];
        let sat = &terrain_layers(&TerrainOptions { flat: false, zoom: 8.0, basemap: Basemap::Satellite, ..TerrainOptions::default() })[0];
        assert!(sat.opacity < dark.opacity);
    }

    #[test]
    fn close_in_the_hillshade_fades_back_but_never_out() {
        assert_eq!(hillshade_fade(5.0), 1.0);
        assert_eq!(hillshade_fade(11.0), 1.0);
        assert!(hillshade_fade(12.5) < 1.0);
        assert_close!(hillshade_fade(18.0), 0.6);
    }

    // new tests

    /// A 7 x 5 grid with valleys below sea level, as bun printed it.
    const GRID: [f64; 35] = [
        -5.0, 61.568851470947266, 38.037723541259766, -18.451702117919922, -36.068172454833984, -1.798871397972107,
        21.50689125061035, -13.352444648742676, 36.65078353881836, 3.877642869949341, -20.7073917388916, 10.4844970703125,
        60.12181854248047, 59.32289505004883, 4.161468505859375, 4.32100248336792, -47.442054748535156, -51.04050827026367,
        -1.9605642557144165, 32.24512481689453, 10.235544204711914, 40.02484130859375, 23.465316772460938,
        -11.78018569946289, 8.772823333740234, 44.20774841308594, 29.855905532836914, -25.949743270874023,
        -15.582690238952637, -22.36858558654785, -10.629083633422852, 45.40277862548828, 75.30926513671875,
        43.80780029296875, 1.0932402610778809,
    ];

    fn grid() -> Vec<f32> {
        GRID.iter().map(|&h| h as f32).collect()
    }

    /// The colour of each pixel by its alpha: shadow (s) or highlight (h).
    fn expand(pixels: &[(char, u8)]) -> Vec<u8> {
        pixels
            .iter()
            .flat_map(|&(kind, a)| {
                let [r, g, b] = if kind == 's' { HILLSHADE_STYLE.shadow } else { HILLSHADE_STYLE.highlight };
                [r, g, b, a]
            })
            .collect()
    }

    #[test]
    fn the_pixels_are_the_typescripts() {
        // hillshade(GRID, 7, 5, row => tileRowMetersPerPixel(9, 170, row * 50, 256), { zFactor: reliefExaggeration(9) })
        let rgba = hillshade(
            &grid(),
            7,
            5,
            |row| tile_row_meters_per_pixel(9, 170, row * 50, 256),
            &HillshadeStyle { z_factor: relief_exaggeration(9.0), ..HILLSHADE_STYLE },
            None,
        );
        #[rustfmt::skip]
        let expected = expand(&[
            ('h', 9), ('s', 10), ('s', 58), ('s', 21), ('h', 7), ('h', 13), ('h', 13),
            ('h', 6), ('s', 16), ('s', 38), ('s', 8), ('h', 6), ('h', 6), ('s', 1),
            ('h', 5), ('s', 5), ('s', 10), ('h', 4), ('h', 7), ('s', 9), ('s', 50),
            ('s', 11), ('s', 13), ('h', 2), ('h', 11), ('h', 9), ('s', 11), ('s', 38),
            ('s', 46), ('s', 30), ('h', 5), ('h', 14), ('h', 8), ('s', 25), ('s', 43),
        ]);
        assert_eq!(rgba, expected);
        // another light, gain and ground size: a shadow can round to an alpha of 0
        let rgba = hillshade(&grid(), 7, 5, |_| 7.25, &HillshadeStyle { azimuth: 200.0, altitude: 30.0, gain: 0.3, ..HILLSHADE_STYLE }, None);
        #[rustfmt::skip]
        let expected = expand(&[
            ('s', 24), ('h', 40), ('s', 49), ('s', 102), ('s', 135), ('s', 135), ('s', 135),
            ('h', 3), ('h', 40), ('s', 10), ('h', 29), ('s', 95), ('s', 135), ('s', 0),
            ('s', 135), ('s', 135), ('s', 112), ('s', 135), ('s', 129), ('h', 40), ('h', 12),
            ('s', 108), ('s', 135), ('s', 135), ('s', 135), ('s', 135), ('s', 135), ('s', 135),
            ('h', 40), ('h', 40), ('s', 82), ('s', 135), ('s', 135), ('s', 135), ('s', 135),
        ]);
        assert_eq!(rgba, expected);
    }

    #[test]
    fn the_output_buffer_is_reused_when_big_enough() {
        let buffer = vec![7_u8; 200];
        let ptr = buffer.as_ptr();
        let rgba = hillshade(&grid(), 7, 5, |_| 10.0, &HILLSHADE_STYLE, Some(buffer));
        assert_eq!(rgba.as_ptr(), ptr);
        assert_eq!(rgba.len(), 200);
        // past the image, the buffer is left as it was
        assert!(rgba[140..].iter().all(|&b| b == 7));
        assert_eq!(rgba[..140], hillshade(&grid(), 7, 5, |_| 10.0, &HILLSHADE_STYLE, None)[..]);
        let small = hillshade(&grid(), 7, 5, |_| 10.0, &HILLSHADE_STYLE, Some(vec![0; 139]));
        assert_eq!(small.len(), 140);
    }

    #[test]
    fn decoding_and_ground_sizes_are_the_typescripts() {
        let heights = decode_terrarium(&[0, 0, 0, 0, 255, 255, 255, 255, 128, 0, 1, 0, 77, 13, 201, 9], None);
        let heights: Vec<f64> = heights.into_iter().map(f64::from).collect();
        assert_eq!(heights, [-32768.0, 32767.99609375, 0.00390625, -13042.21484375]);
        // a trailing partial pixel is not a height
        assert_eq!(decode_terrarium(&[128, 0, 0, 255, 1, 2], None), [0.0]);
        assert_close!(tile_row_meters_per_pixel(9, 170, 50, 256), 190.19978196989513, 9);
        assert_close!(tile_row_meters_per_pixel(3, 1, 3, 512), 1847.8532426123793, 9);
        assert_close!(relief_exaggeration(9.0), 1.5874010519681994, 12);
        assert_close!(relief_exaggeration(4.5), 4.489848193237491, 12);
        assert_eq!(relief_exaggeration(-10.0), 8.0);
        assert!(relief_exaggeration(f64::NAN).is_nan());
    }

    #[test]
    fn opacity_by_basemap_strength_and_zoom_in_twentieths() {
        let opacity = |basemap, zoom, opacity| {
            terrain_layers(&TerrainOptions { flat: zoom > 12.0, zoom, basemap, opacity, extensions: vec![Extension::HazeFade] })[0].opacity
        };
        assert_eq!(opacity(Basemap::Dark, 8.0, 1.0), 1.0);
        assert_close!(opacity(Basemap::Satellite, 8.0, 1.0), 0.45, 12);
        assert_close!(opacity(Basemap::Realistic, 8.0, 1.4), 0.84, 12);
        // hillshadeFade(12.4) = 0.81333…, in steps of 0.05: 0.8
        assert_close!(opacity(Basemap::Dark, 12.4, 1.0), 0.8, 12);
        assert_close!(opacity(Basemap::Dark, 20.0, 1.0), 0.6, 12);
        let layer = &terrain_layers(&TerrainOptions { extensions: vec![Extension::HazeFade], ..TerrainOptions::default() })[0];
        assert_eq!(layer.extensions, [Extension::HazeFade]);
        assert_eq!((layer.min_zoom, layer.max_zoom, layer.tile_size, layer.max_cache_size), (0, 12, 256, 300));
        assert_eq!(terrain_surface(&[Extension::TerrainAir]).extensions, [Extension::SeaLevel, Extension::TerrainAir]);
    }

    #[test]
    fn ground_height_waits_for_its_tile_then_reads_the_nearest_pixel() {
        let mut ground = GroundHeights::new();
        let mut asked = Vec::new();
        // Malmö: tile 2195/1283 at zoom 12, pixel (466, 288) of a 512 px tile
        assert_eq!(ground.height(13.0, 55.6, |t| asked.push(t)), None);
        assert_eq!(asked, [GroundTile { z: 12, x: 2195, y: 1283 }]);
        // asked once only
        assert_eq!(ground.height(13.0, 55.6, |t| asked.push(t)), None);
        assert_eq!(asked.len(), 1);
        let mut heights = vec![0.0_f32; 512 * 512];
        heights[288 * 512 + 466] = 21.5;
        heights[288 * 512 + 467] = -3.0;
        ground.loaded(asked[0], heights);
        assert_eq!(ground.height(13.0, 55.6, |t| asked.push(t)), Some(21.5));
        // never below the sea
        assert_eq!(ground.height(13.0 + 360.0 / 4096.0 / 512.0, 55.6, |t| asked.push(t)), Some(0.0));
        assert_eq!(asked.len(), 1);
        // San Francisco, the far south (clamped to 85 S) and the null island, as the TypeScript keys them
        ground.height(-122.42, 37.77, |t| asked.push(t));
        ground.height(179.99, -89.0, |t| asked.push(t));
        ground.height(0.0, 0.0, |t| asked.push(t));
        let keys: Vec<_> = asked[1..].iter().map(|t| (t.x, t.y)).collect();
        assert_eq!(keys, [(655, 1583), (4095, 4089), (2048, 2048)]);
    }

    #[test]
    fn ground_height_keeps_the_last_sixteen_tiles() {
        let mut ground = GroundHeights::new();
        let mut asked = 0;
        let tile_lon = |i: usize| -180.0 + (i as f64 + 0.5) * 360.0 / 4096.0;
        for i in 0..17 {
            ground.height(tile_lon(i), 0.0, |_| asked += 1);
        }
        assert_eq!((asked, ground.len()), (17, 16));
        // the first was dropped and is asked for again; the second is still kept
        ground.height(tile_lon(1), 0.0, |_| asked += 1);
        assert_eq!(asked, 17);
        ground.height(tile_lon(0), 0.0, |_| asked += 1);
        assert_eq!((asked, ground.len()), (18, 16));
        // a tile loaded after it was dropped is kept anyway
        ground.loaded(GroundTile { z: 12, x: 1, y: 2048 }, vec![5.0; 4]);
        assert_eq!(ground.len(), 17);
        assert_eq!(ground.height(tile_lon(1), 0.0, |_| asked += 1), Some(5.0));
    }
}
