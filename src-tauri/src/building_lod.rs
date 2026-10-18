//! Buildings far out, quantized: the map draws each building only from zoom
//! 13, where OpenFreeMap's tiles have them. Further out this makes its own
//! tiles for zooms 10 to 12, in the same form (OpenMapTiles' `building` layer,
//! `render_height`), so the map's building layer draws them as it does the
//! rest, only coarser.
//!
//! - Zoom 12: the footprints of its four zoom-13 tiles (OpenFreeMap's, all
//!   of a tile's buildings in one outline) filled into a grid of 256 by 256
//!   cells (about 10 m each in Sweden). Zoom 13 says nothing of heights, so
//!   each cell is as tall as its neighbourhood is built up: 5 m where houses
//!   stand apart, up to 20 m where the town is solid.
//! - Zooms 11 and 10: the four grids under each, halved: a cell is built
//!   where two of its four are, as tall as the tallest of them.
//! - Each grid back to outlines: runs of cells of a height along each row,
//!   joined with the same runs in the rows below, as rectangles.
//!
//! Grids are kept in memory and on disk (a month), so a place is worked out
//! once; served to the map as `buildingtiles://localhost/{z}/{x}/{y}`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use crate::rail::{fields, packed};

/// The zoom OpenFreeMap's footprints are read at, and the zooms made from them.
const SOURCE_ZOOM: u32 = 13;
const MIN_ZOOM: u32 = 10;
const MAX_ZOOM: u32 = 12;
/// Cells across a grid, and a vector tile's units across a cell.
const CELLS: usize = 256;
const EXTENT: u32 = 4096;
const UNITS: u32 = EXTENT / CELLS as u32;
/// How far round a cell its neighbourhood reaches, cells; and the heights it gives, metres.
const NEAR: usize = 5;
const LOW_M: f64 = 5.0;
const HIGH_M: f64 = 20.0;
/// Heights are kept in steps of this many metres (fewer, larger rectangles).
const STEP_M: u8 = 2;
const TILEJSON: &str = "https://tiles.openfreemap.org/planet";
const TILEJSON_TTL: Duration = Duration::from_secs(24 * 3600);
const DISK_FRESH: Duration = Duration::from_secs(30 * 24 * 3600);
/// Grids kept in memory, and source tiles fetched at once.
const MAX_GRIDS: usize = 512;
const FETCHES: usize = 8;

type Key = (u32, u32, u32);
/// Heights a cell, metres (0: no building), rows from the north.
type Grid = Vec<u8>;

// ------------------------------------------------------------------ the source

fn template() -> &'static Mutex<Option<(Instant, String)>> {
    static T: OnceLock<Mutex<Option<(Instant, String)>>> = OnceLock::new();
    T.get_or_init(Default::default)
}

/// OpenFreeMap's tile URL (its TileJSON names the current build), kept a day.
async fn source_url(http: &reqwest::Client) -> Result<String, String> {
    if let Some((at, url)) = template().lock().unwrap().clone()
        && at.elapsed() < TILEJSON_TTL
    {
        return Ok(url);
    }
    let body = http.get(TILEJSON).send().await.map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let url = json["tiles"][0].as_str().ok_or("OpenFreeMap's TileJSON names no tiles")?.to_owned();
    *template().lock().unwrap() = Some((Instant::now(), url.clone()));
    Ok(url)
}

fn fetches() -> &'static tokio::sync::Semaphore {
    static S: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    S.get_or_init(|| tokio::sync::Semaphore::new(FETCHES))
}

/// A zoom-13 tile of OpenFreeMap's (gzipped or not as it comes); empty where there is nothing.
async fn source_tile(x: u32, y: u32) -> Result<Vec<u8>, String> {
    let http = crate::rail::http().ok_or("no HTTP client")?;
    let url = source_url(http).await?.replace("{z}", &SOURCE_ZOOM.to_string()).replace("{x}", &x.to_string()).replace("{y}", &y.to_string());
    let _turn = fetches().acquire().await.map_err(|e| e.to_string())?;
    let response = http.get(&url).send().await.map_err(|e| e.to_string())?;
    let bytes = match response.status().as_u16() {
        204 | 404 => return Ok(Vec::new()),
        s if (200..300).contains(&s) => response.bytes().await.map_err(|e| e.to_string())?.to_vec(),
        s => return Err(format!("OpenFreeMap answered {s}")),
    };
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&bytes[..]).read_to_end(&mut out).map_err(|e| e.to_string())?;
        return Ok(out);
    }
    Ok(bytes)
}

/// A tile's `building` layer as rings (its units, y down) and its extent.
pub fn footprints(pbf: &[u8]) -> (Vec<Vec<(f64, f64)>>, f64) {
    let mut rings = Vec::new();
    let mut extent = 4096.0;
    for (f, layer) in fields(pbf) {
        let (3, Err(layer)) = (f, layer) else { continue };
        let mut name = "";
        let mut features: Vec<&[u8]> = Vec::new();
        for (lf, v) in fields(layer) {
            match (lf, v) {
                (1, Err(b)) => name = std::str::from_utf8(b).unwrap_or_default(),
                (2, Err(b)) => features.push(b),
                (5, Ok(e)) => extent = e as f64,
                _ => {}
            }
        }
        if name != "building" {
            continue;
        }
        for feature in features {
            let (mut kind, mut geometry): (u64, &[u8]) = (0, &[]);
            for (ff, v) in fields(feature) {
                match (ff, v) {
                    (3, Ok(k)) => kind = k,
                    (4, Err(g)) => geometry = g,
                    _ => {}
                }
            }
            if kind != 3 {
                continue;
            }
            // MoveTo starts a ring, LineTo goes on, ClosePath ends it; coordinates are zigzag deltas
            let commands = packed(geometry);
            let (mut cx, mut cy) = (0i64, 0i64);
            let mut ring: Vec<(f64, f64)> = Vec::new();
            let mut i = 0;
            while i < commands.len() {
                let (id, count) = (commands[i] & 7, (commands[i] >> 3) as usize);
                i += 1;
                if id == 7 {
                    if ring.len() >= 3 {
                        rings.push(std::mem::take(&mut ring));
                    }
                    ring.clear();
                    continue;
                }
                for _ in 0..count {
                    let (Some(&dx), Some(&dy)) = (commands.get(i), commands.get(i + 1)) else { break };
                    i += 2;
                    cx += ((dx >> 1) as i64) ^ -((dx & 1) as i64);
                    cy += ((dy >> 1) as i64) ^ -((dy & 1) as i64);
                    ring.push((cx as f64, cy as f64));
                }
            }
        }
    }
    (rings, extent)
}

/// Fills `rings` (even–odd, so courtyards stay open) into `cells` × `cells` of
/// `grid` from `(ox, oy)`, at each cell's middle, the rings in `extent` units across the part.
fn fill(rings: &[Vec<(f64, f64)>], extent: f64, grid: &mut [bool], ox: usize, oy: usize, cells: usize) {
    let scale = extent / cells as f64;
    let mut crossings: Vec<f64> = Vec::new();
    for j in 0..cells {
        let y = (j as f64 + 0.5) * scale;
        crossings.clear();
        for ring in rings {
            for k in 0..ring.len() {
                let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
                if (a.1 > y) != (b.1 > y) {
                    crossings.push(a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0));
                }
            }
        }
        crossings.sort_by(f64::total_cmp);
        for pair in crossings.chunks(2) {
            let [x0, x1] = pair else { continue };
            // the cells whose middles lie between
            let i0 = ((x0 / scale) - 0.5).ceil().max(0.0) as usize;
            let i1 = ((x1 / scale) - 0.5).floor().min(cells as f64 - 1.0);
            if i1 < 0.0 {
                continue;
            }
            for i in i0..=(i1 as usize) {
                grid[(oy + j) * CELLS + ox + i] = true;
            }
        }
    }
}

/// Built cells to heights: each as tall as its neighbourhood is built up.
pub fn heights_of(built: &[bool]) -> Grid {
    // a summed-area table, for each neighbourhood's count
    let n = CELLS + 1;
    let mut sum = vec![0u32; n * n];
    for j in 0..CELLS {
        for i in 0..CELLS {
            sum[(j + 1) * n + i + 1] = u32::from(built[j * CELLS + i]) + sum[j * n + i + 1] + sum[(j + 1) * n + i] - sum[j * n + i];
        }
    }
    let mut out = vec![0u8; CELLS * CELLS];
    for j in 0..CELLS {
        for i in 0..CELLS {
            if !built[j * CELLS + i] {
                continue;
            }
            let (i0, i1) = (i.saturating_sub(NEAR), (i + NEAR + 1).min(CELLS));
            let (j0, j1) = (j.saturating_sub(NEAR), (j + NEAR + 1).min(CELLS));
            let count = sum[j1 * n + i1] + sum[j0 * n + i0] - sum[j0 * n + i1] - sum[j1 * n + i0];
            let share = f64::from(count) / ((i1 - i0) * (j1 - j0)) as f64;
            let metres = LOW_M + (HIGH_M - LOW_M) * share.powf(1.5);
            out[j * CELLS + i] = quantize(metres);
        }
    }
    out
}

fn quantize(metres: f64) -> u8 {
    let step = f64::from(STEP_M);
    ((metres / step).round() * step).clamp(step, 254.0) as u8
}

/// Four grids (north-west, north-east, south-west, south-east) as the one over them, halved.
pub fn halve(children: [&Grid; 4]) -> Grid {
    let mut out = vec![0u8; CELLS * CELLS];
    let half = CELLS / 2;
    for (q, child) in children.iter().enumerate() {
        let (ox, oy) = ((q % 2) * half, (q / 2) * half);
        for j in 0..half {
            for i in 0..half {
                let cells = [
                    child[(2 * j) * CELLS + 2 * i],
                    child[(2 * j) * CELLS + 2 * i + 1],
                    child[(2 * j + 1) * CELLS + 2 * i],
                    child[(2 * j + 1) * CELLS + 2 * i + 1],
                ];
                if cells.iter().filter(|&&h| h > 0).count() >= 2 {
                    out[(oy + j) * CELLS + ox + i] = *cells.iter().max().unwrap();
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------ grids, kept

fn cache_dir() -> &'static OnceLock<PathBuf> {
    static D: OnceLock<PathBuf> = OnceLock::new();
    &D
}

/// Where grids are kept between runs (the app's cache directory).
pub fn set_cache_dir(dir: PathBuf) {
    let _ = cache_dir().set(dir.join("building-lod"));
}

fn disk_path((z, x, y): Key) -> Option<PathBuf> {
    Some(cache_dir().get()?.join(format!("{z}/{x}/{y}.grid")))
}

fn from_disk(key: Key) -> Option<Grid> {
    let path = disk_path(key)?;
    let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
    if SystemTime::now().duration_since(modified).map_or(true, |age| age > DISK_FRESH) {
        return None;
    }
    let mut out = Vec::with_capacity(CELLS * CELLS);
    flate2::read::DeflateDecoder::new(std::fs::File::open(path).ok()?).read_to_end(&mut out).ok()?;
    (out.len() == CELLS * CELLS).then_some(out)
}

fn to_disk(key: Key, grid: &Grid) {
    let Some(path) = disk_path(key) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(file) = std::fs::File::create(path) {
        let mut writer = flate2::write::DeflateEncoder::new(file, flate2::Compression::fast());
        let _ = writer.write_all(grid);
        let _ = writer.finish();
    }
}

type Cell = Arc<tokio::sync::OnceCell<Arc<Grid>>>;

fn grids() -> &'static Mutex<HashMap<Key, (Instant, Cell)>> {
    static G: OnceLock<Mutex<HashMap<Key, (Instant, Cell)>>> = OnceLock::new();
    G.get_or_init(Default::default)
}

/// A tile's grid: from memory, disk, or made (once, however many ask at a time).
fn grid(key: Key) -> std::pin::Pin<Box<dyn Future<Output = Result<Arc<Grid>, String>> + Send>> {
    Box::pin(async move {
        let cell = {
            let mut map = grids().lock().unwrap();
            if map.len() >= MAX_GRIDS
                && let Some(oldest) = map.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| *k)
            {
                map.remove(&oldest);
            }
            let entry = map.entry(key).or_insert_with(|| (Instant::now(), Arc::default()));
            entry.0 = Instant::now();
            entry.1.clone()
        };
        cell.get_or_try_init(|| async {
            if let Some(g) = from_disk(key) {
                return Ok(Arc::new(g));
            }
            let made = make(key).await?;
            to_disk(key, &made);
            Ok(Arc::new(made))
        })
        .await
        .cloned()
    })
}

async fn make((z, x, y): Key) -> Result<Grid, String> {
    let children = [(2 * x, 2 * y), (2 * x + 1, 2 * y), (2 * x, 2 * y + 1), (2 * x + 1, 2 * y + 1)];
    if z + 1 == SOURCE_ZOOM {
        let tiles = futures_util::future::join_all(children.iter().map(|&(cx, cy)| source_tile(cx, cy))).await;
        let mut built = vec![false; CELLS * CELLS];
        let half = CELLS / 2;
        for (q, tile) in tiles.into_iter().enumerate() {
            let (rings, extent) = footprints(&tile?);
            fill(&rings, extent, &mut built, (q % 2) * half, (q / 2) * half, half);
        }
        return Ok(heights_of(&built));
    }
    let below = futures_util::future::join_all(children.iter().map(|&(cx, cy)| grid((z + 1, cx, cy)))).await;
    let below: Vec<Arc<Grid>> = below.into_iter().collect::<Result<_, _>>()?;
    Ok(halve([&below[0], &below[1], &below[2], &below[3]]))
}

// ------------------------------------------------------------------ back to outlines

/// A grid's buildings as rectangles: cells from `(i0, j0)` to `(i1, j1)` (not included), and their height.
pub fn rectangles(grid: &Grid) -> Vec<(usize, usize, usize, usize, u8)> {
    let mut done = Vec::new();
    // the rectangles still growing down: by their run's start, end and height
    let mut open: HashMap<(usize, usize, u8), usize> = HashMap::new();
    for j in 0..=CELLS {
        let mut runs = Vec::new();
        if j < CELLS {
            let row = &grid[j * CELLS..(j + 1) * CELLS];
            let mut i = 0;
            while i < CELLS {
                let h = row[i];
                let start = i;
                while i < CELLS && row[i] == h {
                    i += 1;
                }
                if h > 0 {
                    runs.push((start, i, h));
                }
            }
        }
        let mut next = HashMap::with_capacity(runs.len());
        for run in runs {
            next.insert(run, open.remove(&run).unwrap_or(j));
        }
        for ((i0, i1, h), j0) in open.drain() {
            done.push((i0, j0, i1, j, h));
        }
        open = next;
    }
    done
}

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// A field's key: its number and wire type (0 a varint, 2 bytes).
fn tag(out: &mut Vec<u8>, field: u64, wire: u64) {
    varint(out, (field << 3) | wire);
}

fn bytes_field(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    tag(out, field, 2);
    varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn packed_field(out: &mut Vec<u8>, field: u64, values: &[u64]) {
    let mut body = Vec::new();
    for &v in values {
        varint(&mut body, v);
    }
    bytes_field(out, field, &body);
}

const fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

/// A grid as a vector tile: one `building` layer of rectangles with `render_height` and `render_min_height`.
pub fn encode(grid: &Grid) -> Vec<u8> {
    let rects = rectangles(grid);
    if rects.is_empty() {
        return Vec::new();
    }
    let mut heights: Vec<u8> = rects.iter().map(|r| r.4).collect();
    heights.sort_unstable();
    heights.dedup();
    let mut layer = Vec::new();
    tag(&mut layer, 15, 0);
    varint(&mut layer, 2);
    bytes_field(&mut layer, 1, b"building");
    for (n, &(i0, j0, i1, j1, h)) in rects.iter().enumerate() {
        let (x0, y0, x1, y1) = ((i0 as u32 * UNITS) as i64, (j0 as u32 * UNITS) as i64, (i1 as u32 * UNITS) as i64, (j1 as u32 * UNITS) as i64);
        let mut feature = Vec::new();
        tag(&mut feature, 1, 0);
        varint(&mut feature, n as u64 + 1);
        // render_height: its value; render_min_height: the first value after the heights (0)
        let value = heights.binary_search(&h).unwrap_or(0) as u64;
        packed_field(&mut feature, 2, &[0, value, 1, heights.len() as u64]);
        tag(&mut feature, 3, 0);
        varint(&mut feature, 3);
        // clockwise with y down: along the top, down the east side, back along the bottom
        packed_field(
            &mut feature,
            4,
            &[
                (1 << 3) | 1,
                zigzag(x0),
                zigzag(y0),
                (3 << 3) | 2,
                zigzag(x1 - x0),
                0,
                0,
                zigzag(y1 - y0),
                zigzag(x0 - x1),
                0,
                (1 << 3) | 7,
            ],
        );
        bytes_field(&mut layer, 2, &feature);
    }
    bytes_field(&mut layer, 3, b"render_height");
    bytes_field(&mut layer, 3, b"render_min_height");
    for &h in heights.iter().chain(std::iter::once(&0u8)) {
        let mut value = Vec::new();
        tag(&mut value, 5, 0);
        varint(&mut value, u64::from(h));
        bytes_field(&mut layer, 4, &value);
    }
    tag(&mut layer, 5, 0);
    varint(&mut layer, u64::from(EXTENT));
    let mut tile = Vec::new();
    bytes_field(&mut tile, 3, &layer);
    tile
}

/// The `buildingtiles:` protocol the map draws far-off buildings from: `/{z}/{x}/{y}`.
pub async fn serve(path: &str) -> tauri::http::Response<Vec<u8>> {
    let respond = |status: u16, body: Vec<u8>| {
        tauri::http::Response::builder()
            .status(status)
            .header("Content-Type", "application/x-protobuf")
            .header("Access-Control-Allow-Origin", "*")
            .body(body)
            .unwrap_or_default()
    };
    let parts: Vec<u32> = path.trim_matches('/').split('/').filter_map(|p| p.parse().ok()).collect();
    let [z, x, y] = parts[..] else { return respond(400, Vec::new()) };
    if !(MIN_ZOOM..=MAX_ZOOM).contains(&z) || x >= 1 << z || y >= 1 << z {
        return respond(204, Vec::new());
    }
    match grid((z, x, y)).await {
        Ok(g) => match encode(&g) {
            b if b.is_empty() => respond(204, Vec::new()),
            b => respond(200, b),
        },
        Err(e) => {
            eprintln!("building tile {z}/{x}/{y}: {e}");
            respond(502, Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, s: f64) -> Vec<(f64, f64)> {
        vec![(x, y), (x + s, y), (x + s, y + s), (x, y + s)]
    }

    #[test]
    fn footprints_fill_their_cells_and_courtyards_stay_open() {
        let mut built = vec![false; CELLS * CELLS];
        // a block 1024 units square with a 512 courtyard, in a 4096 tile filled across the whole grid
        let rings = vec![square(1024.0, 1024.0, 1024.0), square(1280.0, 1280.0, 512.0)];
        fill(&rings, 4096.0, &mut built, 0, 0, CELLS);
        let count = built.iter().filter(|&&b| b).count();
        // 64² cells less the 32² courtyard
        assert_eq!(count, 64 * 64 - 32 * 32);
        assert!(!built[100 * CELLS + 100]);
        assert!(built[66 * CELLS + 66]);
    }

    #[test]
    fn solid_town_is_taller_than_scattered_houses() {
        let mut built = vec![false; CELLS * CELLS];
        for j in 0..40 {
            for i in 0..40 {
                built[j * CELLS + i] = true;
            }
        }
        built[200 * CELLS + 200] = true;
        let h = heights_of(&built);
        assert!(h[20 * CELLS + 20] >= 18, "{}", h[20 * CELLS + 20]);
        assert!(h[200 * CELLS + 200] <= 6 && h[200 * CELLS + 200] > 0);
        assert_eq!(h[100 * CELLS + 100], 0);
    }

    #[test]
    fn halving_keeps_what_is_mostly_built() {
        let mut a = vec![0u8; CELLS * CELLS];
        // two of a 2×2 built: kept, as tall as the taller; one alone: gone
        a[0] = 8;
        a[1] = 12;
        a[2 * CELLS + 4] = 6;
        let empty = vec![0u8; CELLS * CELLS];
        let out = halve([&a, &empty, &empty, &empty]);
        assert_eq!(out[0], 12);
        assert_eq!(out[CELLS + 2], 0);
    }

    #[test]
    fn rectangles_cover_the_grid_exactly() {
        let mut g = vec![0u8; CELLS * CELLS];
        for j in 10..20 {
            for i in 5..15 {
                g[j * CELLS + i] = 10;
            }
        }
        for i in 30..40 {
            g[12 * CELLS + i] = 6;
        }
        let rects = rectangles(&g);
        assert_eq!(rects.len(), 2);
        assert!(rects.contains(&(5, 10, 15, 20, 10)));
        assert!(rects.contains(&(30, 12, 40, 13, 6)));
    }

    #[test]
    fn the_tile_reads_back_as_its_rectangles() {
        let mut g = vec![0u8; CELLS * CELLS];
        for j in 10..20 {
            for i in 5..15 {
                g[j * CELLS + i] = 10;
            }
        }
        let (rings, extent) = footprints(&encode(&g));
        assert_eq!(extent, 4096.0);
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0], vec![(80.0, 160.0), (240.0, 160.0), (240.0, 320.0), (80.0, 320.0)]);
        assert!(encode(&vec![0u8; CELLS * CELLS]).is_empty());
    }

    /// OpenFreeMap itself: Lund's zoom-12 tile.
    #[tokio::test]
    #[ignore]
    async fn real_buildings() {
        let g = make((12, 2198, 1281)).await.unwrap();
        let built = g.iter().filter(|&&h| h > 0).count();
        let rects = rectangles(&g).len();
        println!("{built} cells built, {rects} rectangles, {} bytes", encode(&g).len());
        assert!(built > 1000);
    }
}
