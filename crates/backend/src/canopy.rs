//! How tall the trees are, metre by metre, the world over (item R7; doc 04 §8.6; `canopy.ts`):
//! Meta and the World Resources Institute's canopy height map (v2, CC BY 4.0), as Taylor
//! Geospatial Engine Labs mirror it on Source Cooperative ([`CHM`]). Each file is a cloud-optimised
//! GeoTIFF of one zoom-[`FILE_ZOOM`] web mercator tile, named by its quadkey, 32768 pixels across
//! (1.2 m each, the height in whole metres, 0 where there are no trees or no data), with smaller
//! copies of itself; the parts a map tile wants are read out of it with HTTP range requests, as
//! today's app does through geotiff.js.
//!
//! What today's `canopy(tile)` does, and so this module ([`Canopy::tile`]):
//! - a tile under zoom [`FILE_ZOOM`] has no canopy;
//! - the tile's file and its place in it ([`place_in_file`]); the file's images (itself and its
//!   smaller copies, not their masks: [`is_mask`]), largest first, read once from its first bytes
//!   (its IFDs) and kept for the [`FILES_CACHED`] files asked last (first in, first out), a file
//!   that can't be read kept as none;
//! - the smallest image with at least [`WANT`] pixels across the tile, else the largest
//!   ([`pick_image`]); the tile's [`window`] of it; the internal tiles under the window read
//!   (each one range) and inflated, their first sample put together as the [`Grid`];
//! - the [`MAX_CACHED`] tiles asked last kept (a hit moves to the back), a tile that failed kept
//!   as none until it is forgotten, as `canopy`'s promise of `null` is.
//!
//! The map's trees (`map::layers::trees`) ask for a tile through the app's tile fetcher as the
//! source [`NAME`]; the fetcher hands those asks to [`tile`] (the network through [`global`]), and
//! the answer's bytes are the grid's heights, row by row (`size²` bytes, `size` their square root).
//! A `--fixture` folder's canopy is the recording `cargo xtask fixtures` keeps ([`FIXTURE`]: each
//! file's `<quadkey>.json` and the byte ranges it holds as `<quadkey>/<start>-<end>.bin`), which
//! [`Canopy::recorded`] reads as this module reads the network, and `map::io::DirTiles` reads for
//! itself (the map cannot see the backend).
//!
//! `scene::canopy` has the same constants and rules ([`quadkey`], [`place_in_file`],
//! [`pick_image`], [`window`]), ported with `canopy.ts` in stage 1; this module cannot see it (the
//! backend depends on `core` only), so the two are kept equal by hand (the app's tests compare
//! them).
//!
//! The TIFF reading is what geotiff.js does for these files: classic or big TIFF of either byte
//! order; tiled images of one 8-bit unsigned sample, none or Deflate compression (`Compression` 1,
//! 8 or 32946), with or without horizontal differencing (`Predictor` 2); an internal tile without
//! bytes reads as 0 (GDAL's sparse tiles). Anything else is a file that can't be read.
//!
//! Deviations, invisible: the first bytes are read in blocks of [`HEAD_BYTES`] until every IFD is
//! in (geotiff.js reads blocks of the same size, but as its reads ask, and keeps them); each
//! internal tile is one request (geotiff.js joins neighbouring ones); every ask gives up after
//! [`TIMEOUT`].

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use indexmap::IndexMap;
use tokio::sync::OnceCell;

use crate::CmdResult;

/// The source's name: the map's `layers::trees::CANOPY`, as the app's tile fetcher is asked it.
pub const NAME: &str = "canopy";
/// Where the files are, each `<quadkey>.tif` (`CHM`).
pub const CHM: &str = "https://data.source.coop/tge-labs/meta-chm-v2/chm/";
/// Their credit (`CANOPY_ATTRIBUTION`).
pub const ATTRIBUTION: &str = "Canopy height: Meta & WRI (CC BY 4.0), via TGE Labs";
/// The files' zoom; a tile under it has no canopy (`FILE_ZOOM`).
pub const FILE_ZOOM: u32 = 10;
/// Pixels across a tile read, at least (the nearest of the file's sizes at or over it; `WANT`).
pub const WANT: f64 = 512.0;
/// Files kept, first in first out (`files`' 24).
pub const FILES_CACHED: usize = 24;
/// Tiles kept, the most recently used last (`MAX_CACHED`).
pub const MAX_CACHED: usize = 96;
/// A `--fixture` folder's recording of the files, beside its `tiles` folder (`cargo xtask
/// fixtures`' `canopy/`).
pub const FIXTURE: &str = "canopy";
/// A file's first bytes are read this many at a time until every IFD is in.
pub const HEAD_BYTES: u64 = 65_536;
/// The most of a file's first bytes read to find its IFDs.
pub const MAX_HEAD_BYTES: u64 = 4 << 20;
/// How long one read may take.
pub const TIMEOUT: Duration = Duration::from_secs(30);

// --- the rules (`scene::canopy`'s twins) -----------------------------------------------------------

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

/// Which zoom-[`FILE_ZOOM`] file a tile is in, and where in it: its north-west corner and its
/// size, as fractions of the file (`placeInFile`).
#[derive(Debug, Clone, PartialEq)]
pub struct PlaceInFile {
    pub key: String,
    pub x: f64,
    pub y: f64,
    pub span: f64,
}

/// Where a tile (zoom [`FILE_ZOOM`] or more) is in its file.
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
/// set; no tag is 0).
pub fn is_mask(new_subfile_type: Option<u32>) -> bool {
    new_subfile_type.unwrap_or(0) & 4 != 0
}

/// Which of a file's images (their widths, largest first) to read a tile out of: the smallest
/// with at least [`WANT`] pixels across the tile (`span` of the file), else the largest; none when
/// there are none.
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

/// The window of an image `width` pixels across that a tile at `place` takes (at least a pixel):
/// `round` as JavaScript's `Math.round`.
pub fn window(width: u32, place: &PlaceInFile) -> Window {
    let w = f64::from(width);
    let round = |v: f64| (v + 0.5).floor();
    let size = round(w * place.span).max(1.0);
    Window { x0: round(place.x * w) as usize, y0: round(place.y * w) as usize, size: size as usize }
}

// --- TIFF -------------------------------------------------------------------------------------------

/// One image of a TIFF: what reading its pixels takes.
#[derive(Debug, Clone, PartialEq)]
pub struct TiffImage {
    /// `NewSubfileType` (254); a mask has the 4 bit set.
    pub new_subfile_type: Option<u32>,
    pub width: u32,
    pub height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    /// `BitsPerSample`, `SamplesPerPixel`, `PlanarConfiguration`, `SampleFormat` (their first
    /// values; absent ones as TIFF's defaults: 1, 1, 1, 1).
    pub bits: u32,
    pub samples: u32,
    pub planar: u32,
    pub sample_format: u32,
    /// `Compression` (1 none) and `Predictor` (1 none).
    pub compression: u32,
    pub predictor: u32,
    /// Where each internal tile's bytes start and how many there are, row by row.
    pub offsets: Vec<u64>,
    pub byte_counts: Vec<u64>,
}

/// Why a TIFF's IFDs could not be read from the bytes at hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TiffError {
    /// The IFDs go on past the bytes given: at least this many are needed.
    Short(u64),
    Bad(String),
}

/// The images of a TIFF or BigTIFF from its first bytes (a COG keeps every IFD there).
pub fn parse_tiff(bytes: &[u8]) -> Result<Vec<TiffImage>, TiffError> {
    let need = |end: u64| -> Result<(), TiffError> { if end > bytes.len() as u64 { Err(TiffError::Short(end)) } else { Ok(()) } };
    need(8)?;
    let little = match &bytes[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Err(TiffError::Bad("not a TIFF".into())),
    };
    let uint = |at: u64, size: u64| -> Result<u64, TiffError> {
        need(at + size)?;
        let b = &bytes[at as usize..(at + size) as usize];
        Ok(b.iter().enumerate().fold(0u64, |v, (i, &byte)| {
            let shift = if little { 8 * i } else { 8 * (size as usize - 1 - i) };
            v | (u64::from(byte) << shift)
        }))
    };
    let big = match uint(2, 2)? {
        42 => false,
        43 => true,
        v => return Err(TiffError::Bad(format!("TIFF version {v}"))),
    };
    let (count_size, entry_size, offset_size) = if big { (8, 20, 8) } else { (2, 12, 4) };
    let mut next = if big { uint(8, 8)? } else { uint(4, 4)? };
    let mut images = Vec::new();
    let mut seen = Vec::new();
    while next != 0 {
        if seen.contains(&next) || images.len() > 64 {
            return Err(TiffError::Bad("the IFDs loop".into()));
        }
        seen.push(next);
        let count = uint(next, count_size)?;
        let entries = next + count_size;
        let ifd_end = entries + count * entry_size + offset_size;
        need(ifd_end)?;
        let mut tags: Vec<(u16, Vec<u64>)> = Vec::new();
        for i in 0..count {
            let at = entries + i * entry_size;
            let tag = uint(at, 2)? as u16;
            let kind = uint(at + 2, 2)?;
            let n = uint(at + 4, if big { 8 } else { 4 })?;
            let value_at = at + if big { 12 } else { 8 };
            let item = match kind {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 | 16 | 17 | 18 => 8,
                _ => continue,
            };
            // only the integer tags reading the pixels takes
            if ![254, 256, 257, 258, 259, 277, 284, 317, 322, 323, 324, 325, 339].contains(&tag) || !matches!(kind, 1 | 3 | 4 | 16) {
                continue;
            }
            let size = n * item;
            let start = if size <= offset_size { value_at } else { uint(value_at, offset_size)? };
            need(start + size)?;
            let values = (0..n).map(|k| uint(start + k * item, item)).collect::<Result<Vec<_>, _>>()?;
            tags.push((tag, values));
        }
        next = uint(ifd_end - offset_size, offset_size)?;
        let all = |tag: u16| tags.iter().find(|(t, _)| *t == tag).map(|(_, v)| v.clone());
        let one = |tag: u16| all(tag).and_then(|v| v.first().copied()).map(|v| v as u32);
        let missing = |what: &str| TiffError::Bad(format!("an image without {what}"));
        let image = TiffImage {
            new_subfile_type: one(254),
            width: one(256).ok_or_else(|| missing("ImageWidth"))?,
            height: one(257).ok_or_else(|| missing("ImageLength"))?,
            tile_width: one(322).ok_or_else(|| missing("tiles"))?,
            tile_height: one(323).ok_or_else(|| missing("tiles"))?,
            bits: one(258).unwrap_or(1),
            samples: one(277).unwrap_or(1),
            planar: one(284).unwrap_or(1),
            sample_format: one(339).unwrap_or(1),
            compression: one(259).unwrap_or(1),
            predictor: one(317).unwrap_or(1),
            offsets: all(324).unwrap_or_default(),
            byte_counts: all(325).unwrap_or_default(),
        };
        if image.tile_width == 0 || image.tile_height == 0 || image.offsets.len() != image.byte_counts.len() {
            return Err(TiffError::Bad("an image's tiles do not add up".into()));
        }
        images.push(image);
    }
    Ok(images)
}

/// One internal tile's bytes as stored to its pixels' first samples (`tile_width × tile_height`,
/// row by row): inflated, the differencing undone.
pub fn decode_tile(image: &TiffImage, stored: &[u8]) -> Result<Vec<u8>, String> {
    if image.bits != 8 || image.sample_format != 1 {
        return Err(format!("{}-bit samples of format {}", image.bits, image.sample_format));
    }
    let samples = image.samples.max(1) as usize;
    let (w, h) = (image.tile_width as usize, image.tile_height as usize);
    // (planar images keep each sample's tile apart: the first sample's are the first tiles)
    let per_pixel = if image.planar == 2 { 1 } else { samples };
    let want = w * h * per_pixel;
    let mut raw = match image.compression {
        1 => stored.to_vec(),
        8 | 32946 => {
            let mut out = Vec::with_capacity(want);
            flate2::read::ZlibDecoder::new(stored).read_to_end(&mut out).map_err(|e| format!("a tile that does not inflate: {e}"))?;
            out
        }
        c => return Err(format!("compression {c}")),
    };
    if raw.len() < want {
        return Err(format!("a tile of {} bytes where {want} are wanted", raw.len()));
    }
    match image.predictor {
        1 => {}
        2 => {
            for row in raw[..want].chunks_exact_mut(w * per_pixel) {
                for i in per_pixel..row.len() {
                    row[i] = row[i].wrapping_add(row[i - per_pixel]);
                }
            }
        }
        p => return Err(format!("predictor {p}")),
    }
    if per_pixel == 1 {
        raw.truncate(want);
        return Ok(raw);
    }
    Ok(raw[..want].chunks_exact(per_pixel).map(|px| px[0]).collect())
}

/// The internal tiles a window of `image` touches: each one's index among the image's tiles, and
/// its column and row.
pub fn tiles_under(image: &TiffImage, win: Window) -> Vec<(usize, usize, usize)> {
    let (tw, th) = (image.tile_width as usize, image.tile_height as usize);
    let across = (image.width as usize).div_ceil(tw);
    let down = (image.height as usize).div_ceil(th);
    let (x1, y1) = ((win.x0 + win.size).min(image.width as usize), (win.y0 + win.size).min(image.height as usize));
    if win.x0 >= x1 || win.y0 >= y1 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for row in win.y0 / th..=((y1 - 1) / th).min(down - 1) {
        for col in win.x0 / tw..=((x1 - 1) / tw).min(across - 1) {
            out.push((row * across + col, col, row));
        }
    }
    out
}

/// A tile's tree heights, metres, `size` × `size` from its north-west corner (`CanopyGrid`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grid {
    pub size: usize,
    pub heights: Vec<u8>,
}

/// The window `win` of `image` from its internal tiles' pixels (`(col, row, pixels)`, the first
/// samples as [`decode_tile`] gives them; a tile not given reads as 0), as geotiff.js's
/// `readRasters({ window, samples: [0], interleave: true })` puts them together.
pub fn assemble(image: &TiffImage, win: Window, tiles: &[(usize, usize, Vec<u8>)]) -> Grid {
    let (tw, th) = (image.tile_width as usize, image.tile_height as usize);
    let mut heights = vec![0u8; win.size * win.size];
    for (col, row, pixels) in tiles {
        let (tx0, ty0) = (col * tw, row * th);
        for y in ty0.max(win.y0)..(ty0 + th).min(win.y0 + win.size).min(image.height as usize) {
            let (from, to) = (tx0.max(win.x0), (tx0 + tw).min(win.x0 + win.size).min(image.width as usize));
            if from >= to {
                continue;
            }
            let src = (y - ty0) * tw + (from - tx0);
            let dst = (y - win.y0) * win.size + (from - win.x0);
            heights[dst..dst + (to - from)].copy_from_slice(&pixels[src..src + (to - from)]);
        }
    }
    Grid { size: win.size, heights }
}

// --- where the bytes come from ----------------------------------------------------------------------

/// Where a canopy's files are read from.
enum Source {
    /// The network: `{base}{key}.tif` with range requests.
    Http { http: reqwest::Client, base: String },
    /// A recording of the files' ranges (a `--fixture` folder's [`FIXTURE`] folder).
    Recorded { dir: PathBuf },
}

impl Source {
    /// The bytes `[start, end)` of file `key`: fewer where the file (or the recorded range) ends
    /// first, `None` where there is no such file.
    async fn read(&self, key: &str, start: u64, end: u64) -> CmdResult<Option<Vec<u8>>> {
        match self {
            Source::Http { http, base } => {
                let url = format!("{base}{key}.tif");
                let ask = http.get(&url).header(reqwest::header::RANGE, format!("bytes={start}-{}", end - 1)).send();
                let response = tokio::time::timeout(TIMEOUT, ask).await.map_err(|_| format!("{url}: no answer"))?.map_err(|e| format!("{url}: {e}"))?;
                let status = response.status();
                if status == reqwest::StatusCode::NOT_FOUND {
                    return Ok(None);
                }
                // (a server that ignores the range would send the whole file: geotiff.js's
                // `allowFullFile: false` takes none of it)
                if status != reqwest::StatusCode::PARTIAL_CONTENT {
                    return Err(format!("{url}: {status} for a range"));
                }
                let body = tokio::time::timeout(TIMEOUT, response.bytes()).await.map_err(|_| format!("{url}: no answer"))?.map_err(|e| format!("{url}: {e}"))?;
                Ok(Some(body.into_iter().take((end - start) as usize).collect()))
            }
            Source::Recorded { dir } => read_recorded(dir, key, start, end),
        }
    }
}

/// The recording of file `key` in `dir` (`<key>.json`'s `ranges`, `[start, end)` each): the
/// bytes `[start, end)` from the range that holds `start` (cut at its end, as a server cuts a read
/// at the file's), `None` where `dir` has no recording of the file.
pub fn read_recorded(dir: &Path, key: &str, start: u64, end: u64) -> CmdResult<Option<Vec<u8>>> {
    let manifest = dir.join(format!("{key}.json"));
    let Ok(text) = std::fs::read(&manifest) else { return Ok(None) };
    let json: serde_json::Value = serde_json::from_slice(&text).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let ranges = json["ranges"].as_array().ok_or_else(|| format!("{}: no ranges", manifest.display()))?;
    for range in ranges {
        let (Some(from), Some(to)) = (range[0].as_u64(), range[1].as_u64()) else { continue };
        if from <= start && start < to {
            let path = dir.join(key).join(format!("{from}-{to}.bin"));
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let (a, b) = ((start - from) as usize, (end.min(to) - from) as usize);
            return bytes.get(a..b).map(|b| Some(b.to_vec())).ok_or_else(|| format!("{}: shorter than its name", path.display()));
        }
    }
    Err(format!("canopy {key}: bytes {start}-{end} were not recorded"))
}

// --- the canopy -------------------------------------------------------------------------------------

/// A file's images (not its masks), largest first; none where the file can't be had.
type Images = Option<Arc<Vec<TiffImage>>>;
/// The tiles kept, by `(z, x, y)`, the most recently used last.
type Tiles = IndexMap<(u32, u32, u32), Arc<OnceCell<Option<Arc<Grid>>>>>;

/// One canopy height map (see the module docs): where its files are, and what has been read.
pub struct Canopy {
    source: Source,
    files: Mutex<IndexMap<String, Arc<OnceCell<Images>>>>,
    tiles: Mutex<Tiles>,
}

impl std::fmt::Debug for Canopy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.source {
            Source::Http { base, .. } => f.debug_struct("Canopy").field("base", base).finish_non_exhaustive(),
            Source::Recorded { dir } => f.debug_struct("Canopy").field("dir", dir).finish_non_exhaustive(),
        }
    }
}

impl Canopy {
    fn new(source: Source) -> Self {
        Canopy { source, files: Mutex::new(IndexMap::new()), tiles: Mutex::new(IndexMap::new()) }
    }

    /// The files at `base` (`{base}{key}.tif`; [`CHM`]), asked with `http`.
    pub fn network(http: reqwest::Client, base: &str) -> Self {
        Self::new(Source::Http { http, base: base.to_owned() })
    }

    /// The recording in the folder `dir` (`<key>.json`, `<key>/<start>-<end>.bin`).
    pub fn recorded(dir: impl Into<PathBuf>) -> Self {
        Self::new(Source::Recorded { dir: dir.into() })
    }

    /// A `--fixture` folder's recording ([`FIXTURE`] in `dir`, or beside `dir` when `dir` is the
    /// folder's `tiles`), where it has one.
    pub fn fixture(dir: &Path) -> Option<Self> {
        [dir.join(FIXTURE), dir.join("..").join(FIXTURE)].into_iter().find(|p| p.is_dir()).map(Self::recorded)
    }

    /// How many files and tiles are kept now.
    pub fn kept(&self) -> (usize, usize) {
        let files = self.files.lock().unwrap_or_else(PoisonError::into_inner).len();
        (files, self.tiles.lock().unwrap_or_else(PoisonError::into_inner).len())
    }

    /// A map tile's tree heights (a tile of zoom [`FILE_ZOOM`] or more), or none where there are
    /// none to be had (`canopy(t)`).
    pub async fn tile(&self, x: u32, y: u32, z: u32) -> Option<Arc<Grid>> {
        if z < FILE_ZOOM {
            return None;
        }
        let cell = {
            let mut tiles = self.tiles.lock().unwrap_or_else(PoisonError::into_inner);
            // (a hit moves to the back: the most recently used last)
            let cell = tiles.shift_remove(&(z, x, y)).unwrap_or_default();
            tiles.insert((z, x, y), Arc::clone(&cell));
            while tiles.len() > MAX_CACHED {
                tiles.shift_remove_index(0);
            }
            cell
        };
        cell.get_or_init(|| async {
            match self.read_tile(x, y, z).await {
                Ok(grid) => grid.map(Arc::new),
                Err(e) => {
                    log::warn!("canopy {z}/{x}/{y}: {e}");
                    None
                }
            }
        })
        .await
        .clone()
    }

    async fn read_tile(&self, x: u32, y: u32, z: u32) -> CmdResult<Option<Grid>> {
        let place = place_in_file(x, y, z);
        let Some(images) = self.images(&place.key).await else { return Ok(None) };
        let widths: Vec<u32> = images.iter().map(|im| im.width).collect();
        let Some(i) = pick_image(&widths, place.span) else { return Ok(None) };
        let image = images[i].clone();
        let win = window(image.width, &place);
        let mut stored = Vec::new();
        for (k, col, row) in tiles_under(&image, win) {
            let (Some(&offset), Some(&count)) = (image.offsets.get(k), image.byte_counts.get(k)) else { continue };
            // (a tile without bytes: GDAL's sparse tile, read as 0)
            if count == 0 {
                continue;
            }
            let bytes = self.source.read(&place.key, offset, offset + count).await?.ok_or_else(|| format!("canopy {}: the file went", place.key))?;
            if bytes.len() as u64 != count {
                return Err(format!("canopy {}: {} bytes of an internal tile of {count}", place.key, bytes.len()));
            }
            stored.push((col, row, bytes));
        }
        // (inflating is work for a thread of its own, not the runtime's)
        let grid = tokio::task::spawn_blocking(move || -> CmdResult<Grid> {
            let tiles = stored.into_iter().map(|(col, row, bytes)| Ok((col, row, decode_tile(&image, &bytes)?))).collect::<CmdResult<Vec<_>>>()?;
            Ok(assemble(&image, win, &tiles))
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(Some(grid))
    }

    /// File `key`'s images, read once from its first bytes; none where it can't be read (kept so
    /// until it is forgotten, as `imagesOf`'s `null`).
    async fn images(&self, key: &str) -> Images {
        let cell = {
            let mut files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
            match files.get(key) {
                Some(cell) => Arc::clone(cell),
                None => {
                    let cell = Arc::new(OnceCell::new());
                    files.insert(key.to_owned(), Arc::clone(&cell));
                    while files.len() > FILES_CACHED {
                        files.shift_remove_index(0);
                    }
                    cell
                }
            }
        };
        cell.get_or_init(|| async {
            match self.read_images(key).await {
                Ok(images) => images.map(Arc::new),
                Err(e) => {
                    log::warn!("canopy {key}: {e}");
                    None
                }
            }
        })
        .await
        .clone()
    }

    /// File `key`'s images (masks left out), largest first, from its first bytes.
    async fn read_images(&self, key: &str) -> CmdResult<Option<Vec<TiffImage>>> {
        let mut head: Vec<u8> = Vec::new();
        let images = loop {
            let have = head.len() as u64;
            let Some(more) = self.source.read(key, have, have + HEAD_BYTES).await? else { return Ok(None) };
            let got = more.len() as u64;
            head.extend(more);
            match parse_tiff(&head) {
                Ok(images) => break images,
                Err(TiffError::Short(need)) if got > 0 && need <= MAX_HEAD_BYTES => {
                    // (the file, or what is held of it, ended before the IFDs did)
                    if got < HEAD_BYTES && need > head.len() as u64 {
                        return Err(format!("canopy {key}: its IFDs run past its end ({need} bytes)"));
                    }
                }
                Err(e) => return Err(format!("canopy {key}: {e:?}")),
            }
        };
        let mut readable: Vec<TiffImage> = images.into_iter().filter(|im| !is_mask(im.new_subfile_type)).collect();
        // (JavaScript's sort is stable, as `sort_by_key` is)
        readable.sort_by_key(|im| std::cmp::Reverse(im.width));
        Ok(Some(readable))
    }
}

/// The app's canopy: Source Cooperative's files ([`CHM`]) through the app's HTTP client
/// (`fr24::client_builder()`, doc 07 §2.3), made at the first ask.
pub fn global() -> Option<&'static Canopy> {
    static CANOPY: OnceLock<Option<Canopy>> = OnceLock::new();
    CANOPY
        .get_or_init(|| {
            let http = fr24::client_builder()
                .user_agent(concat!("RustRadar/", env!("CARGO_PKG_VERSION"), " (+https://github.com/felixgoff/RustRadar)"))
                .timeout(TIMEOUT)
                .build()
                .ok()?;
            Some(Canopy::network(http, CHM))
        })
        .as_ref()
}

/// The answer to the tile fetcher's ask for tile `z/x/y` of [`NAME`]: the grid's heights, row by
/// row (`size²` bytes), or none where there are none to be had.
pub async fn tile_of(canopy: &Canopy, z: u32, x: u32, y: u32) -> CmdResult<Option<Vec<u8>>> {
    Ok(canopy.tile(x, y, z).await.map(|grid| grid.heights.clone()))
}

/// Tile `z/x/y` of the app's canopy ([`global`]): `Ok(None)` at once under zoom [`FILE_ZOOM`]
/// (nothing is asked).
pub async fn tile(z: u32, x: u32, y: u32) -> CmdResult<Option<Vec<u8>>> {
    if z < FILE_ZOOM {
        return Ok(None);
    }
    let canopy = global().ok_or("no HTTP client for the canopy")?;
    tile_of(canopy, z, x, y).await
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    /// The committed recording (plan, "Fixtures for stages 8-10"): file 1200312213, the head and
    /// the internal tiles under 25 zoom-16 tiles round V5, Humlegården and Stockholm C.
    fn recording() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../map/tests/data").join(FIXTURE)
    }

    const KEY: &str = "1200312213";

    /// The zoom-16 tiles the recording holds (its JSON's `tiles`).
    fn recorded_tiles() -> Vec<(u32, u32, u32)> {
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(recording().join(format!("{KEY}.json"))).unwrap()).unwrap();
        json["tiles"].as_array().unwrap().iter().map(|t| (t[0].as_u64().unwrap() as u32, t[1].as_u64().unwrap() as u32, t[2].as_u64().unwrap() as u32)).collect()
    }

    #[test]
    fn the_rules_are_canopy_ts_s() {
        // `landcover.test.ts`'s Lund
        assert_eq!(quadkey(549, 320, 10), "1202100101");
        let p = place_in_file(549 * 64 + 3, 320 * 64 + 5, 16);
        assert_eq!((p.key.as_str(), p.span, p.x, p.y), ("1202100101", 1.0 / 64.0, 3.0 / 64.0, 5.0 / 64.0));
        assert_eq!(quadkey(3, 5, 3), "213");
        assert_eq!(file_url("1202100101"), "https://data.source.coop/tge-labs/meta-chm-v2/chm/1202100101.tif");
        let widths = [32768, 16384, 8192, 4096, 2048, 1024, 512];
        assert_eq!(pick_image(&widths, 1.0 / 64.0), Some(0));
        assert_eq!(pick_image(&widths, 1.0 / 8.0), Some(3));
        assert_eq!(pick_image(&widths, 1.0 / 256.0), Some(0));
        assert_eq!(pick_image(&[], 1.0), None);
        assert!(is_mask(Some(4)) && is_mask(Some(5)) && !is_mask(Some(1)) && !is_mask(None));
        let p = place_in_file(549 * 8 + 3, 320 * 8 + 5, 13);
        assert_eq!(window(4096, &p), Window { x0: 1536, y0: 2560, size: 512 });
        let p = place_in_file(549 * 1024 + 1023, 320 * 1024, 20);
        assert_eq!(window(512, &p), Window { x0: 512, y0: 0, size: 1 });
        // V5's tiles are in Stockholm's file
        assert_eq!(place_in_file(36057, 19268, 16).key, KEY);
    }

    /// The recording's file as geotiff.js meets it: 32768 px across with four smaller copies, each
    /// with a mask; 512 px internal tiles of one byte, Deflate, no differencing.
    #[test]
    fn the_recorded_file_s_images_are_read_from_its_first_bytes() {
        let head = read_recorded(&recording(), KEY, 0, HEAD_BYTES).unwrap().unwrap();
        assert_eq!(head.len() as u64, HEAD_BYTES);
        assert!(matches!(parse_tiff(&head), Err(TiffError::Short(_))), "the IFDs run past the first block");
        // the whole head as recorded (a read past its end is cut there)
        let all = read_recorded(&recording(), KEY, 0, 4 * HEAD_BYTES).unwrap().unwrap();
        let images = parse_tiff(&all).unwrap();
        let readable: Vec<&TiffImage> = images.iter().filter(|im| !is_mask(im.new_subfile_type)).collect();
        assert_eq!(readable[0].width, 32768);
        assert!(readable.windows(2).all(|w| w[0].width > w[1].width), "largest first");
        assert!(images.iter().any(|im| is_mask(im.new_subfile_type)));
        for im in &readable {
            assert_eq!((im.tile_width, im.tile_height, im.bits, im.samples, im.compression, im.predictor), (512, 512, 8, 1, 8, 1));
            let across = im.width.div_ceil(512) as usize;
            assert_eq!(im.offsets.len(), across * im.height.div_ceil(512) as usize);
        }
        // nothing outside the recording, nothing of another file
        assert!(read_recorded(&recording(), KEY, 200_000, 200_010).is_err());
        assert_eq!(read_recorded(&recording(), "0000000000", 0, 10), Ok(None));
    }

    #[test]
    fn deflate_and_differencing_come_undone_and_windows_are_put_together() {
        let image = TiffImage {
            new_subfile_type: None,
            width: 6,
            height: 5,
            tile_width: 4,
            tile_height: 4,
            bits: 8,
            samples: 1,
            planar: 1,
            sample_format: 1,
            compression: 8,
            predictor: 2,
            offsets: vec![0; 4],
            byte_counts: vec![1; 4],
        };
        // a tile whose rows climb by one: differenced, every byte after the first is 1
        let pixels: Vec<u8> = (0..16).map(|i| (i / 4 * 10 + i % 4) as u8).collect();
        let mut diffed = pixels.clone();
        for row in diffed.chunks_exact_mut(4) {
            for i in (1..4).rev() {
                row[i] = row[i].wrapping_sub(row[i - 1]);
            }
        }
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut z, &diffed).unwrap();
        let stored = z.finish().unwrap();
        assert_eq!(decode_tile(&image, &stored).unwrap(), pixels);
        assert!(decode_tile(&image, &stored[..4]).is_err());
        assert!(decode_tile(&TiffImage { compression: 5, ..image.clone() }, &stored).is_err());
        assert!(decode_tile(&TiffImage { bits: 16, ..image.clone() }, &stored).is_err());
        // a 3-px window over the four tiles' corner, one tile missing (0)
        let win = Window { x0: 2, y0: 2, size: 3 };
        assert_eq!(tiles_under(&image, win), vec![(0, 0, 0), (1, 1, 0), (2, 0, 1), (3, 1, 1)]);
        let tile = |v: u8| vec![v; 16];
        let grid = assemble(&image, win, &[(0, 0, pixels.clone()), (1, 0, tile(7)), (0, 1, tile(9))]);
        assert_eq!(grid, Grid { size: 3, heights: vec![22, 23, 7, 32, 33, 7, 9, 9, 0] });
    }

    /// Every tile the recording was made for reads from it: 512 px across (a 64th of the 32768 px
    /// image), the heights of Stockholm's parks and streets; a tile under zoom 10 or out of the
    /// recording's file has none.
    #[tokio::test]
    async fn the_recorded_tiles_read_as_grids() {
        let canopy = Canopy::fixture(&recording().join("../tiles")).expect("the committed recording");
        let mut tall = 0;
        for (z, x, y) in recorded_tiles() {
            let grid = canopy.tile(x, y, z).await.unwrap_or_else(|| panic!("{z}/{x}/{y}"));
            assert_eq!((grid.size, grid.heights.len()), (512, 512 * 512));
            let trees = grid.heights.iter().filter(|&&h| h >= 3).count();
            let top = *grid.heights.iter().max().unwrap();
            assert!(top < 60, "{z}/{x}/{y}: {top} m");
            tall += trees;
        }
        eprintln!("{tall} pixels of trees 3 m or taller over {} tiles", recorded_tiles().len());
        assert!(tall > 100_000);
        assert_eq!(canopy.tile(549, 320, 9).await, None);
        assert_eq!(canopy.kept(), (1, recorded_tiles().len()));
        // a file not recorded: none, and kept so
        assert_eq!(canopy.tile(549 * 64, 320 * 64, 16).await, None);
        assert_eq!(canopy.kept().0, 2);
        assert_eq!(tile_of(&canopy, 9, 0, 0).await, Ok(None));
    }

    /// An HTTP server on 127.0.0.1 answering range requests for the recording's file from the
    /// recording (206, the bytes asked for; 404 for any other path), counting the asks.
    async fn server() -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/chm/", listener.local_addr().unwrap());
        let asked = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&asked);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let count = Arc::clone(&count);
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    count.fetch_add(1, Ordering::SeqCst);
                    let text = String::from_utf8_lossy(&head).to_string();
                    let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    let range = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_owned));
                    let answer = match (path == format!("/chm/{KEY}.tif"), range) {
                        (true, Some(range)) => {
                            let (a, b) = range.trim().split_once('-').unwrap();
                            let (a, b): (u64, u64) = (a.parse().unwrap(), b.parse().unwrap());
                            read_recorded(&recording(), KEY, a, b + 1).ok().flatten().map(|bytes| ("206 Partial Content", bytes))
                        }
                        _ => None,
                    };
                    let (status, body) = answer.unwrap_or(("404 Not Found", Vec::new()));
                    let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        (base, asked)
    }

    /// Over HTTP with range requests, as today's `fromUrl(…).readRasters`: the same grids as the
    /// recording's, the file's images read once, a tile asked again kept; a file the server lacks
    /// is none.
    #[tokio::test]
    async fn the_network_reads_the_same_grids_with_range_requests() {
        let (base, asked) = server().await;
        let http = fr24::client_builder().no_proxy().timeout(Duration::from_secs(10)).build().unwrap();
        let network = Canopy::network(http, &base);
        let local = Canopy::recorded(recording());
        let tiles = recorded_tiles();
        for &(z, x, y) in &tiles[..6] {
            assert_eq!(network.tile(x, y, z).await, local.tile(x, y, z).await, "{z}/{x}/{y}");
        }
        let after = asked.load(Ordering::SeqCst);
        let (z, x, y) = tiles[0];
        assert!(network.tile(x, y, z).await.is_some());
        assert_eq!(asked.load(Ordering::SeqCst), after, "a kept tile is not asked again");
        assert_eq!(network.tile(549 * 64, 320 * 64, 16).await, None);
    }

    /// The tiles kept: the 96 asked last, a hit moving to the back; the files: the 24 asked first
    /// to go first.
    #[tokio::test]
    async fn ninety_six_tiles_and_twenty_four_files_are_kept() {
        let canopy = Canopy::recorded(std::env::temp_dir().join("rustradar-no-canopy-here"));
        for i in 0..100 {
            assert_eq!(canopy.tile(i, 0, 10).await, None);
        }
        let (files, tiles) = canopy.kept();
        assert_eq!((files, tiles), (FILES_CACHED, MAX_CACHED));
        let kept: Vec<(u32, u32, u32)> = canopy.tiles.lock().unwrap().keys().copied().collect();
        assert_eq!(kept.first(), Some(&(10, 4, 0)));
        canopy.tile(4, 0, 10).await;
        let kept: Vec<(u32, u32, u32)> = canopy.tiles.lock().unwrap().keys().copied().collect();
        assert_eq!((kept.first(), kept.last()), (Some(&(10, 5, 0)), Some(&(10, 4, 0))));
        let files: Vec<String> = canopy.files.lock().unwrap().keys().cloned().collect();
        assert_eq!(files.last(), Some(&quadkey(99, 0, 10)));
        assert_eq!(files.first(), Some(&quadkey(76, 0, 10)));
    }
}
