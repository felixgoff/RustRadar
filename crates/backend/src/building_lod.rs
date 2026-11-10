//! Buildings far out, quantized: the map draws each building only from zoom
//! 13, where OpenFreeMap's tiles have them. Further out this makes its own
//! tiles for zooms 9 to 12, in the same form (OpenMapTiles' `building` layer,
//! `render_height`), so the map's building layer draws them as it does the
//! rest, only coarser.
//!
//! - Zoom 12: the footprints of its four zoom-13 tiles (OpenFreeMap's, all
//!   of a tile's buildings in one outline) filled into a grid of 256 by 256
//!   cells (about 10 m each in Sweden). Zoom 13 says nothing of heights, so
//!   each cell is as tall as its neighbourhood is built up: 5 m where houses
//!   stand apart, up to 20 m where the town is solid.
//! - Zooms 11 to 9: the four grids under each, halved: a cell is built
//!   where two of its four are, as tall as the tallest of them.
//! - Each grid back to outlines: its cells in bands of height (low, middling,
//!   tall), each band softened and traced (marching squares) and simplified,
//!   so the outlines follow the town's own lines and angles, the taller bands
//!   standing inside the lower.
//!
//! Grids are kept in memory and on disk (a month), so a place is worked out
//! once; served to the map as `buildingtiles://localhost/{z}/{x}/{y}`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use rustradar_core::mvt;

/// The zoom OpenFreeMap's footprints are read at, and the zooms made from them.
const SOURCE_ZOOM: u32 = 13;
const MIN_ZOOM: u32 = 9;
const MAX_ZOOM: u32 = 12;
/// Cells across a grid, and a vector tile's units across a cell.
const CELLS: usize = 256;
const EXTENT: u32 = 4096;
const UNITS: u32 = EXTENT / CELLS as u32;
/// How far round a cell its neighbourhood reaches, cells; and the heights it gives, metres.
const NEAR: usize = 5;
const LOW_M: f64 = 5.0;
const HIGH_M: f64 = 20.0;
/// Heights are kept in steps of this many metres.
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

/// A tile's `building` layers as rings (their units, y down, open) and their extent (the last
/// one's; the default without one).
pub fn footprints(pbf: &[u8]) -> (Vec<Vec<(f64, f64)>>, f64) {
    let (mut rings, mut extent) = (Vec::new(), mvt::EXTENT_DEFAULT);
    for layer in mvt::layers_called(pbf, "building") {
        rings.extend(layer.features().filter(|f| f.kind == mvt::GeomType::Polygon).flat_map(|f| f.rings()));
        extent = layer.extent;
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

/// The heights the outlines are drawn in steps of: each band the cells at least as tall as
/// its first number, drawn as tall as its second, the taller inside the lower.
const BANDS: [(u8, u8); 3] = [(1, 7), (10, 12), (15, 18)];
/// Outlines smaller than this many cells are left out; they are simplified to within this many.
const MIN_RING_CELLS: f64 = 1.2;
/// Where the softened cells are traced: under a half, so houses a cell or two apart join into
/// one outline (a street of them, from far out) and a house alone keeps a little more than its cell.
const TRACE_AT: f32 = 0.3;
const SIMPLIFY_CELLS: f64 = 0.45;

/// Twice a ring's signed area, positive clockwise with y down (a vector tile's outer rings).
fn signed_area2(ring: &[(f64, f64)]) -> f64 {
    (0..ring.len()).map(|i| {
        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
        a.0 * b.1 - b.0 * a.1
    }).sum()
}

/// A line simplified to within `tol` (Douglas–Peucker), its ends kept.
fn simplify(points: &[(f64, f64)], tol: f64) -> Vec<(f64, f64)> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0, points.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (p, q) = (points[a], points[b]);
        let (dx, dy) = (q.0 - p.0, q.1 - p.1);
        let len = dx.hypot(dy).max(1e-9);
        let (mut far, mut at) = (0.0, a);
        for (i, r) in points.iter().enumerate().take(b).skip(a + 1) {
            let d = ((r.0 - p.0) * dy - (r.1 - p.1) * dx).abs() / len;
            if d > far {
                (far, at) = (d, i);
            }
        }
        if far > tol {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    points.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

/// A closed ring simplified: split at its farthest point from its first, each half simplified.
fn simplify_ring(ring: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let first = ring[0];
    let far = (1..ring.len())
        .max_by(|&a, &b| {
            let d = |p: (f64, f64)| (p.0 - first.0).hypot(p.1 - first.1);
            d(ring[a]).total_cmp(&d(ring[b]))
        })
        .unwrap_or(0);
    if far == 0 {
        return ring.to_vec();
    }
    let mut a = simplify(&ring[..=far], SIMPLIFY_CELLS);
    let mut tail: Vec<(f64, f64)> = ring[far..].to_vec();
    tail.push(first);
    let b = simplify(&tail, SIMPLIFY_CELLS);
    a.pop();
    a.extend_from_slice(&b[..b.len() - 1]);
    a
}

/// A traced ring cleaned up: its repeats and closing point gone, simplified; none if too small to keep.
fn clean_ring(mut points: Vec<(f64, f64)>) -> Option<Vec<(f64, f64)>> {
    points.dedup();
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    if points.len() < 3 || signed_area2(&points).abs() / 2.0 < MIN_RING_CELLS {
        return None;
    }
    let points = simplify_ring(&points);
    (points.len() >= 3 && signed_area2(&points).abs() / 2.0 >= MIN_RING_CELLS).then_some(points)
}

/// A building's outline from the grid: its outer ring then its courtyards (cells across, y down,
/// the outer clockwise), and how tall it is drawn.
pub type Outline = (Vec<Vec<(f64, f64)>>, u8);

/// A grid's buildings as outlines: each height band's cells softened and traced (marching
/// squares), so they follow the town's own lines and angles rather than the cells' edges.
pub fn outlines(grid: &Grid) -> Vec<Outline> {
    let mut out = Vec::new();
    for &(from, height) in &BANDS {
        let mask: Vec<f32> = grid.iter().map(|&h| f32::from(u8::from(h >= from))).collect();
        if !mask.iter().any(|&v| v > 0.0) {
            continue;
        }
        // softened over a cell's neighbours, so corners round off and steps become slopes
        let mut field = vec![0f32; CELLS * CELLS];
        for j in 0..CELLS {
            for i in 0..CELLS {
                let (mut sum, mut n) = (0.0, 0.0);
                for jj in j.saturating_sub(1)..(j + 2).min(CELLS) {
                    for ii in i.saturating_sub(1)..(i + 2).min(CELLS) {
                        sum += mask[jj * CELLS + ii];
                        n += 1.0;
                    }
                }
                field[j * CELLS + i] = (sum / n + mask[j * CELLS + i]) / 2.0;
            }
        }
        let Ok(contours) = contour::ContourBuilder::new(CELLS, CELLS, true).contours(&field, &[TRACE_AT]) else { continue };
        for c in contours {
            for polygon in &c.geometry().0 {
                let exterior: Vec<(f64, f64)> = polygon.exterior().0.iter().map(|p| (f64::from(p.x), f64::from(p.y))).collect();
                let Some(mut outer) = clean_ring(exterior) else { continue };
                if signed_area2(&outer) < 0.0 {
                    outer.reverse();
                }
                let mut rings = vec![outer];
                for hole in polygon.interiors() {
                    let points: Vec<(f64, f64)> = hole.0.iter().map(|p| (f64::from(p.x), f64::from(p.y))).collect();
                    if let Some(mut h) = clean_ring(points) {
                        if signed_area2(&h) > 0.0 {
                            h.reverse();
                        }
                        rings.push(h);
                    }
                }
                out.push((rings, height));
            }
        }
    }
    out
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

/// A grid as a vector tile: one `building` layer of outlines with `render_height` and `render_min_height`.
pub fn encode(grid: &Grid) -> Vec<u8> {
    let shapes = outlines(grid);
    if shapes.is_empty() {
        return Vec::new();
    }
    let mut heights: Vec<u8> = shapes.iter().map(|s| s.1).collect();
    heights.sort_unstable();
    heights.dedup();
    let mut layer = Vec::new();
    tag(&mut layer, 15, 0);
    varint(&mut layer, 2);
    bytes_field(&mut layer, 1, b"building");
    let unit = f64::from(UNITS);
    for (n, (rings, h)) in shapes.iter().enumerate() {
        let mut feature = Vec::new();
        tag(&mut feature, 1, 0);
        varint(&mut feature, n as u64 + 1);
        // render_height: its value; render_min_height: the first value after the heights (0)
        let value = heights.binary_search(h).unwrap_or(0) as u64;
        packed_field(&mut feature, 2, &[0, value, 1, heights.len() as u64]);
        tag(&mut feature, 3, 0);
        varint(&mut feature, 3);
        // each ring: MoveTo its first point, LineTo the rest, ClosePath (zigzag deltas, the cursor carried on)
        let (mut cx, mut cy) = (0i64, 0i64);
        let mut commands = Vec::new();
        for ring in rings {
            let points: Vec<(i64, i64)> = ring.iter().map(|&(x, y)| ((x * unit).round() as i64, (y * unit).round() as i64)).collect();
            for (k, &(x, y)) in points.iter().enumerate() {
                if k == 0 {
                    commands.push((1 << 3) | 1);
                } else if k == 1 {
                    commands.push(((points.len() as u64 - 1) << 3) | 2);
                }
                commands.push(zigzag(x - cx));
                commands.push(zigzag(y - cy));
                (cx, cy) = (x, y);
            }
            commands.push((1 << 3) | 7);
        }
        packed_field(&mut feature, 4, &commands);
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

/// One tile of far-off buildings for the map to draw, as a vector tile. None
/// where there is nothing: a zoom they aren't made at, a tile off the map, or
/// an empty one.
pub async fn tile(z: u32, x: u32, y: u32) -> Result<Option<Vec<u8>>, String> {
    if !(MIN_ZOOM..=MAX_ZOOM).contains(&z) || x >= 1 << z || y >= 1 << z {
        return Ok(None);
    }
    match grid((z, x, y)).await {
        Ok(g) => match encode(&g) {
            b if b.is_empty() => Ok(None),
            b => Ok(Some(b)),
        },
        Err(e) => {
            eprintln!("building tile {z}/{x}/{y}: {e}");
            Err(e)
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
    fn outlines_follow_the_blocks_at_their_own_angles() {
        let mut g = vec![0u8; CELLS * CELLS];
        // a square block 20 cells across, and a diagonal one
        for j in 10..30 {
            for i in 10..30 {
                g[j * CELLS + i] = 8;
            }
        }
        for k in 0..40 {
            for w in 0..6 {
                g[(100 + k) * CELLS + 100 + k + w] = 8;
            }
        }
        let shapes = outlines(&g);
        assert_eq!(shapes.len(), 2);
        let area = |r: &[(f64, f64)]| signed_area2(r) / 2.0;
        // the square: about its 400 cells (a little more: traced outside its middle), its corners rounded off
        let square = shapes.iter().find(|(r, _)| r[0].iter().all(|p| p.0 < 50.0)).unwrap();
        assert!((area(&square.0[0]) - 420.0).abs() < 40.0, "{}", area(&square.0[0]));
        // the diagonal: its long sides run at 45°, not along the cells
        let diagonal = shapes.iter().find(|(r, _)| r[0].iter().all(|p| p.0 > 50.0)).unwrap();
        let ring = &diagonal.0[0];
        let slanted = (0..ring.len()).any(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            dx.hypot(dy) > 10.0 && (dx.abs() - dy.abs()).abs() < 0.3 * dx.hypot(dy)
        });
        assert!(slanted, "{ring:?}");
        assert!(ring.len() < 30, "simplified: {}", ring.len());
    }

    #[test]
    fn tall_parts_stand_inside_the_low() {
        let mut g = vec![0u8; CELLS * CELLS];
        for j in 10..40 {
            for i in 10..40 {
                g[j * CELLS + i] = if (20..30).contains(&i) && (20..30).contains(&j) { 18 } else { 6 };
            }
        }
        let mut heights: Vec<u8> = outlines(&g).iter().map(|s| s.1).collect();
        heights.sort_unstable();
        assert_eq!(heights, vec![7, 12, 18]);
    }

    #[test]
    fn a_courtyard_is_a_hole() {
        let mut g = vec![0u8; CELLS * CELLS];
        for j in 10..40 {
            for i in 10..40 {
                if !((20..30).contains(&i) && (20..30).contains(&j)) {
                    g[j * CELLS + i] = 6;
                }
            }
        }
        let shapes = outlines(&g);
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].0.len(), 2);
        assert!(signed_area2(&shapes[0].0[0]) > 0.0 && signed_area2(&shapes[0].0[1]) < 0.0);
    }

    #[test]
    fn the_tile_reads_back_as_its_outlines() {
        let mut g = vec![0u8; CELLS * CELLS];
        // (low enough to be one band's: one outline)
        for j in 10..20 {
            for i in 5..15 {
                g[j * CELLS + i] = 8;
            }
        }
        let (rings, extent) = footprints(&encode(&g));
        assert_eq!(extent, 4096.0);
        assert_eq!(rings.len(), 1);
        // about the block's 10 by 10 cells, 16 units each
        let (xs, ys): (Vec<f64>, Vec<f64>) = rings[0].iter().copied().unzip();
        let (x0, x1) = (xs.iter().copied().fold(f64::MAX, f64::min), xs.iter().copied().fold(f64::MIN, f64::max));
        let (y0, y1) = (ys.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MIN, f64::max));
        assert!((x0 - 80.0).abs() < 24.0 && (x1 - 240.0).abs() < 24.0, "{x0} {x1}");
        assert!((y0 - 160.0).abs() < 24.0 && (y1 - 320.0).abs() < 24.0, "{y0} {y1}");
        assert!(encode(&vec![0u8; CELLS * CELLS]).is_empty());
    }

    /// OpenFreeMap's zoom-13 tile 4396/2562 (central Lund; its build of 2026-11-01; © OpenMapTiles,
    /// © OpenStreetMap contributors, ODbL), as fetched.
    const LUND_Z13: &[u8] = include_bytes!("../tests/data/openfreemap-13-4396-2562.pbf");

    /// What loaders.gl 4.5.2's `VectorTile` (the TypeScript app's decoder) read in [`LUND_Z13`],
    /// layer by layer: its name, version, extent and features; its features by geometry type
    /// (unknown, point, line, polygon) and the sum of their ids; the parts and points of their
    /// geometry as its `loadGeometry` has them (each MoveTo a part, a ring's first point again at its
    /// ClosePath) and the sums of the points' x and y; and their tags: how many, the strings'
    /// length in UTF-16 units, how many numbers and their sum, how many true and false.
    #[allow(clippy::type_complexity)]
    const LUND_Z13_AS_LOADERS_GL_READ_IT: [(&str, u64, f64, usize, [usize; 4], u64, usize, usize, f64, f64, usize, usize, usize, f64, usize, usize); 13] = [
        ("aeroway", 2, 4096.0, 1, [0, 0, 0, 1], 5261401532, 1, 11, 15582.0, 27924.0, 1, 7, 0, 0.0, 0, 0),
        ("boundary", 2, 4096.0, 2, [0, 0, 2, 0], 2061018810, 12, 167, 321099.0, 415369.0, 6, 0, 6, 19.0, 0, 0),
        ("building", 2, 4096.0, 1, [0, 0, 0, 1], 2356842430, 680, 10921, 21769780.0, 26204183.0, 0, 0, 0, 0.0, 0, 0),
        ("landcover", 2, 4096.0, 51, [0, 0, 0, 51], 131281382359, 71, 1874, 4344601.0, 2839047.0, 102, 543, 0, 0.0, 0, 0),
        ("landuse", 2, 4096.0, 107, [0, 0, 0, 107], 181884550085, 352, 3731, 7153691.0, 8993090.0, 107, 911, 0, 0.0, 0, 0),
        ("mountain_peak", 2, 4096.0, 3, [0, 3, 0, 0], 114501495943, 3, 3, 11937.0, -243.0, 12, 12, 9, 1265.0, 0, 0),
        ("park", 2, 4096.0, 1, [0, 1, 0, 0], 1029245572, 1, 1, 7980.0, 1402.0, 18, 266, 1, 1.0, 0, 0),
        ("place", 2, 4096.0, 10, [0, 10, 0, 0], 283359567650, 10, 10, 8008.0, -1989.0, 139, 897, 10, 155.0, 0, 0),
        ("poi", 2, 4096.0, 1, [0, 1, 0, 0], 52485407021, 1, 1, 260.0, 3373.0, 21, 331, 1, 1.0, 0, 0),
        ("transportation", 2, 4096.0, 427, [0, 0, 389, 38], 1767609250397, 1032, 4631, 9400266.0, 10564498.0, 1431, 6028, 354, 331.0, 0, 0),
        ("transportation_name", 2, 4096.0, 16, [0, 2, 14, 0], 11727108304, 21, 284, 632948.0, 565408.0, 113, 1165, 6, 21.0, 0, 0),
        ("water", 2, 4096.0, 24, [0, 0, 0, 24], 71602291012, 25, 278, 366128.0, 507930.0, 72, 105, 48, 7160229096.0, 0, 0),
        ("waterway", 2, 4096.0, 10, [0, 0, 10, 0], 35084875060, 10, 84, 93678.0, 80134.0, 20, 58, 10, 1.0, 0, 0),
    ];

    /// loaders.gl's `loadGeometry` from a feature's commands: each MoveTo starts a part, a
    /// ClosePath repeats the part's first point.
    fn load_geometry(f: &mvt::Feature) -> Vec<Vec<(f64, f64)>> {
        let mut parts: Vec<mvt::Line> = Vec::new();
        let mut part: Option<mvt::Line> = None;
        for c in f.commands() {
            match c {
                mvt::Command::MoveTo(p) => {
                    parts.extend(part.take());
                    part = Some(vec![p]);
                }
                mvt::Command::LineTo(p) => part.iter_mut().for_each(|l| l.push(p)),
                mvt::Command::ClosePath => part.iter_mut().for_each(|l| l.push(l[0])),
            }
        }
        parts.extend(part);
        parts
    }

    #[test]
    fn a_real_tile_reads_as_loaders_gl_read_it() {
        let layers: Vec<mvt::Layer> = mvt::layers(LUND_Z13).collect();
        assert_eq!(layers.len(), LUND_Z13_AS_LOADERS_GL_READ_IT.len());
        for (layer, want) in layers.iter().zip(LUND_Z13_AS_LOADERS_GL_READ_IT) {
            let (name, version, extent, features, types, ids, parts, points, sx, sy, tags, strings, nums, numsum, trues, falses) = want;
            assert_eq!((layer.name, layer.version, layer.extent, layer.len()), (name, version, extent, features));
            let mut got = ([0usize; 4], 0u64, 0usize, 0usize, 0.0f64, 0.0f64, 0usize, 0usize, 0usize, 0.0f64, 0usize, 0usize);
            for f in layer.features() {
                got.0[f.kind as usize] += 1;
                got.1 += f.id.unwrap_or(0);
                let geometry = load_geometry(&f);
                got.2 += geometry.len();
                for &(x, y) in geometry.iter().flatten() {
                    got.3 += 1;
                    got.4 += x;
                    got.5 += y;
                }
                for (_, v) in &f.tags {
                    got.6 += 1;
                    match *v {
                        mvt::Value::Str(s) => got.7 += s.encode_utf16().count(),
                        mvt::Value::Num(n) => {
                            got.8 += 1;
                            got.9 += n;
                        }
                        mvt::Value::Bool(true) => got.10 += 1,
                        mvt::Value::Bool(false) => got.11 += 1,
                    }
                }
                // the readings the backend uses, from the same commands
                let closed_by_ring = geometry.iter().filter(|p| p.len() >= 4).map(|p| p[..p.len() - 1].to_vec()).collect::<Vec<_>>();
                match f.kind {
                    mvt::GeomType::Polygon => assert_eq!(f.rings(), closed_by_ring, "{name}"),
                    mvt::GeomType::LineString => {
                        assert_eq!(f.lines(), geometry.iter().filter(|p| p.len() > 1).cloned().collect::<Vec<_>>(), "{name}")
                    }
                    _ => assert_eq!(f.points().collect::<Vec<_>>(), geometry.concat(), "{name}"),
                }
            }
            assert_eq!((got.0, got.1, got.2, got.3, got.4, got.5), (types, ids, parts, points, sx, sy), "{name}: geometry");
            assert_eq!((got.6, got.7, got.8, got.10, got.11), (tags, strings, nums, trues, falses), "{name}: tags");
            assert!((got.9 - numsum).abs() <= 1e-9 * numsum.abs(), "{name}: {} against {numsum}", got.9);
        }
        // the layers picked by name come in the order asked
        let names = ["water", "building", "nothing", "aeroway"];
        assert_eq!(mvt::layers_named(LUND_Z13, &names).iter().map(|l| l.name).collect::<Vec<_>>(), ["water", "building", "aeroway"]);
        // a tag as loaders.gl had it
        let poi = mvt::layer(LUND_Z13, "poi").unwrap().feature(0).unwrap();
        assert_eq!(poi.text("name"), Some("Lund centralstation"));
        assert_eq!((poi.text("class"), poi.text("subclass"), poi.num("rank")), (Some("railway"), Some("station"), Some(1.0)));
        assert_eq!(poi.points().collect::<Vec<_>>(), [(260.0, 3373.0)]);
    }

    #[test]
    fn footprints_of_a_real_tile() {
        let (rings, extent) = footprints(LUND_Z13);
        assert_eq!(extent, 4096.0);
        // every part of the one building feature, closed (each ring's first point not repeated)
        assert_eq!(rings.len(), 680);
        assert_eq!(rings.iter().map(Vec::len).sum::<usize>(), 10921 - 680);
        let mut built = vec![false; CELLS * CELLS];
        fill(&rings, extent, &mut built, 0, 0, CELLS);
        let count = built.iter().filter(|&&b| b).count();
        assert!(count > 3000, "{count}");
        // no buildings in a tile without the layer
        assert_eq!(footprints(&[]), (Vec::new(), 4096.0));
    }

    /// OpenFreeMap itself: Lund's zoom-12 tile.
    #[tokio::test]
    #[ignore]
    async fn real_buildings() {
        let g = make((12, 2198, 1281)).await.unwrap();
        let built = g.iter().filter(|&&h| h > 0).count();
        let shapes = outlines(&g).len();
        println!("{built} cells built, {shapes} outlines, {} bytes", encode(&g).len());
        assert!(built > 1000);
    }
}
