//! The railway network, so trains run along the track rather than straight
//! across country, stop at the platform they are announced at, and keep to
//! each section's speed limit.
//!
//! It comes from OpenRailwayMap's vector tiles (`railway_line_high`): every
//! track as mapped in OpenStreetMap, with its `maxspeed` and its `track_ref`
//! (the platform track's number). The same tiles are what the map draws the
//! rail network from (through the `railtiles:` protocol, see `serve`), at the
//! zoom routing uses, so a train on the track is on the line drawn.
//!
//! Tiles are fetched in the background and kept, in memory and on disk; a
//! route is only made from tiles already here, so asking never waits on the
//! network. A train whose tiles haven't arrived yet goes straight until the
//! next refresh.
//!
//! OpenRailwayMap asks for an identifying User-Agent and Referer and light
//! use: tiles are kept on disk for two weeks and asked for a few at a time.

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use crate::motion::{ahead, course, distance};

#[path = "blocks.rs"]
pub mod blocks;

const TILES: &str = "https://openrailwaymap.app/railway_line_high,railway_text_km";
const REFERER: &str = "https://github.com/felixgoff/RustRadar";
/// The zoom routing uses, and the deepest the map draws (it scales the
/// tiles up beyond): their lines are within a metre of the full detail.
pub const ZOOM: u32 = 12;
/// The shallowest zoom the tiles are served at.
pub const MIN_ZOOM: u32 = 7;
/// Tiles' tracks kept in memory for routing.
const MAX_TILES: usize = 3000;
/// Tiles' bytes kept in memory for the map.
const MAX_BYTES: usize = 400;
/// Graphs kept built, by the tiles they cover (each train's motion and its way ahead use several).
const MAX_GRAPHS: usize = 32;
/// A tile on disk is used without asking again for this long.
const DISK_FRESH: Duration = Duration::from_secs(14 * 24 * 3600);
/// A route near a train needing more tiles than this is too long to make here.
const MAX_ROUTE_TILES: usize = 64;
/// One leg of a whole run, station to station.
const MAX_LEG_TILES: usize = 160;
/// A train on its way has passed at most this many places it can't reach going on.
const MAX_PASSED: usize = 3;
/// Fetches at once in the background.
const MAX_LOADING: usize = 16;
/// A tile that failed is asked again after this.
const RETRY: Duration = Duration::from_secs(120);
/// A train further than this from any track isn't on one we know.
const SNAP_M: f64 = 150.0;
/// Tracks this much further than the nearest are also considered for a train
/// (its reported position is a few metres out; parallel tracks are 4.5 m apart).
const SNAP_SPREAD_M: f64 = 6.0;
/// A station further than this from the track isn't on it.
const STATION_SNAP_M: f64 = 800.0;
/// A platform track this far from the station's point still belongs to it.
const PLATFORM_M: f64 = 1500.0;
/// Pieces of a track cut at a tile's edge meeting within this are joined.
const SEAM_M: f64 = 3.0;
/// A track ending this close beside another runs into it (a switch whose
/// node the tile's simplification moved). Under the 4.5 m between tracks.
const JOIN_M: f64 = 2.0;
/// The sharpest a train turns at a node, degrees: never back on itself.
const MAX_TURN: f64 = 120.0;
/// Segments shorter than this have no reliable direction.
const SHORT_M: f64 = 3.0;
/// Sidings, yards and spurs cost this much more than running lines.
const SERVICE_COST: f64 = 3.0;
/// Running against a track's usual direction (the left-hand track of double
/// track, in Sweden) costs this much more: it happens, but seldom.
const WRONG_WAY_COST: f64 = 4.0;
/// Known for this much of a stretch between switches (or a third of it), the
/// way trains run there holds for all of it, metres.
const SPREAD_FLOW_M: f64 = 100.0;
/// The extent of a tile's own units when it doesn't say.
const EXTENT_DEFAULT: f64 = 4096.0;

type Line = Vec<(f64, f64)>;
type Key = (u32, u32);
/// A tile's bytes by `(z, x, y)`, with when they were last used.
type Bytes = HashMap<(u32, u32, u32), (Instant, Arc<Vec<u8>>)>;

/// One track as the tile has it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub points: Line,
    /// km/h.
    pub maxspeed: Option<f64>,
    /// The track's number at its station ("4").
    pub track_ref: Option<String>,
    /// A siding, yard track, spur or crossover.
    pub service: bool,
    /// A yard track or a spur: never where a passenger train is guessed to stop.
    pub yard: bool,
    pub bridge: bool,
    pub tunnel: bool,
}

struct Store {
    tracks: HashMap<Key, (Instant, Arc<Vec<Track>>)>,
    bytes: Bytes,
    loading: HashSet<Key>,
    failed: HashMap<Key, Instant>,
    graphs: Vec<(Vec<Key>, Arc<Graph>)>,
}

fn store() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Store {
            tracks: HashMap::new(),
            bytes: HashMap::new(),
            loading: HashSet::new(),
            failed: HashMap::new(),
            graphs: Vec::new(),
        })
    })
}

fn cache_dir() -> &'static OnceLock<PathBuf> {
    static D: OnceLock<PathBuf> = OnceLock::new();
    &D
}

/// Where tiles are kept between runs (the app's cache directory).
pub fn set_cache_dir(dir: PathBuf) {
    let _ = cache_dir().set(dir.join("rail-tiles"));
}

/// The HTTP client the track's data is fetched with.
pub(crate) fn http() -> Option<&'static reqwest::Client> {
    client()
}

fn client() -> Option<&'static reqwest::Client> {
    static C: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    C.get_or_init(|| {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::REFERER, reqwest::header::HeaderValue::from_static(REFERER));
        fr24::client_builder()
            .user_agent(concat!("RustRadar/", env!("CARGO_PKG_VERSION"), " (+https://github.com/felixgoff/RustRadar)"))
            .default_headers(headers)
            .timeout(Duration::from_secs(20))
            .build()
            .ok()
    })
    .as_ref()
}

// ------------------------------------------------------------------ tiles

pub fn tile_of(lat: f64, lon: f64) -> Key {
    let n = f64::from(1u32 << ZOOM);
    let x = ((lon + 180.0) / 360.0 * n).floor().clamp(0.0, n - 1.0);
    let lat = lat.clamp(-85.05, 85.05).to_radians();
    let y = ((1.0 - lat.tan().asinh() / std::f64::consts::PI) / 2.0 * n).floor().clamp(0.0, n - 1.0);
    (x as u32, y as u32)
}

/// A point in tile `(z, x, y)`'s own units (`0..extent`, y down) to degrees.
fn to_lat_lon(z: u32, x: u32, y: u32, extent: f64, px: f64, py: f64) -> (f64, f64) {
    let n = f64::from(1u32 << z);
    let lon = (f64::from(x) + px / extent) / n * 360.0 - 180.0;
    let m = std::f64::consts::PI * (1.0 - 2.0 * (f64::from(y) + py / extent) / n);
    (m.sinh().atan().to_degrees(), lon)
}

/// Whether a point lies on the edge of a routing tile (where tiles cut tracks).
fn on_tile_edge(p: (f64, f64)) -> bool {
    let n = f64::from(1u32 << ZOOM);
    let fx = (p.1 + 180.0) / 360.0 * n;
    let lat = p.0.clamp(-85.05, 85.05).to_radians();
    let fy = (1.0 - lat.tan().asinh() / std::f64::consts::PI) / 2.0 * n;
    // within about half a metre
    (fx - fx.round()).abs() < 1e-4 || (fy - fy.round()).abs() < 1e-4
}

struct Pb<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Pb<'a> {
    fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let byte = *self.b.get(self.i)?;
            self.i += 1;
            v |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return Some(v);
            }
            shift += 7;
            if shift > 63 {
                return None;
            }
        }
    }

    /// The next field: its number, and its value (a varint, or the bytes of
    /// a length-delimited or fixed-width one).
    fn field(&mut self) -> Option<(u64, Result<u64, &'a [u8]>)> {
        let key = self.varint()?;
        let value = match key & 7 {
            0 => Ok(self.varint()?),
            1 => {
                let b = self.b.get(self.i..self.i + 8)?;
                self.i += 8;
                Err(b)
            }
            2 => {
                let len = self.varint()? as usize;
                let b = self.b.get(self.i..self.i.checked_add(len)?)?;
                self.i += len;
                Err(b)
            }
            5 => {
                let b = self.b.get(self.i..self.i + 4)?;
                self.i += 4;
                Err(b)
            }
            _ => return None,
        };
        Some((key >> 3, value))
    }
}

pub(crate) fn fields(b: &[u8]) -> impl Iterator<Item = (u64, Result<u64, &[u8]>)> {
    let mut pb = Pb { b, i: 0 };
    std::iter::from_fn(move || pb.field())
}

pub(crate) fn packed(b: &[u8]) -> Vec<u64> {
    let mut pb = Pb { b, i: 0 };
    std::iter::from_fn(|| if pb.i < pb.b.len() { pb.varint() } else { None }).collect()
}

/// A tag value: the vector tile spec's string, float, double, int, uint, sint or bool.
#[derive(Clone, Debug, PartialEq)]
enum Val<'a> {
    Str(&'a str),
    Num(f64),
    Bool(bool),
}

fn value(b: &[u8]) -> Option<Val<'_>> {
    fields(b).find_map(|(f, v)| match (f, v) {
        (1, Err(s)) => std::str::from_utf8(s).ok().map(Val::Str),
        (2, Err(b)) => Some(Val::Num(f64::from(f32::from_le_bytes(b.try_into().ok()?)))),
        (3, Err(b)) => Some(Val::Num(f64::from_le_bytes(b.try_into().ok()?))),
        (4, Ok(n)) => Some(Val::Num(n as i64 as f64)),
        (5, Ok(n)) => Some(Val::Num(n as f64)),
        (6, Ok(n)) => Some(Val::Num(((n >> 1) as i64 ^ -((n & 1) as i64)) as f64)),
        (7, Ok(n)) => Some(Val::Bool(n != 0)),
        _ => None,
    })
}

/// A segment cut to the square `0..e` (Liang-Barsky); none if it lies outside.
fn clip_segment(p: (f64, f64), q: (f64, f64), e: f64) -> Option<((f64, f64), (f64, f64))> {
    let (dx, dy) = (q.0 - p.0, q.1 - p.1);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (pp, qq) in [(-dx, p.0), (dx, e - p.0), (-dy, p.1), (dy, e - p.1)] {
        if pp == 0.0 {
            if qq < 0.0 {
                return None;
            }
        } else {
            let r = qq / pp;
            if pp < 0.0 {
                if r > t1 {
                    return None;
                }
                t0 = t0.max(r);
            } else {
                if r < t0 {
                    return None;
                }
                t1 = t1.min(r);
            }
        }
    }
    Some(((p.0 + t0 * dx, p.1 + t0 * dy), (p.0 + t1 * dx, p.1 + t1 * dy)))
}

/// A line cut to the tile: tiles overlap their neighbours a little, and cut
/// at the very edge the pieces of neighbouring tiles meet end to end.
fn clip(points: &[(f64, f64)], e: f64) -> Vec<Line> {
    let mut out = Vec::new();
    let mut cur: Line = Vec::new();
    for w in points.windows(2) {
        let Some((a, b)) = clip_segment(w[0], w[1], e) else {
            if cur.len() > 1 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
            continue;
        };
        if cur.last() != Some(&a) {
            if cur.len() > 1 {
                out.push(std::mem::take(&mut cur));
            }
            cur = vec![a];
        }
        if b != a {
            cur.push(b);
        }
        if b != w[1] {
            // left the tile
            if cur.len() > 1 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
        }
    }
    if cur.len() > 1 {
        out.push(cur);
    }
    out
}

/// The railway tracks in an OpenRailwayMap vector tile, in degrees.
pub fn decode(pbf: &[u8], z: u32, x: u32, y: u32) -> Vec<Track> {
    let mut out = Vec::new();
    for (f, layer) in fields(pbf) {
        let (3, Err(layer)) = (f, layer) else { continue };
        let mut name = "";
        let mut keys: Vec<&str> = Vec::new();
        let mut values: Vec<Option<Val>> = Vec::new();
        let mut features: Vec<&[u8]> = Vec::new();
        let mut extent = EXTENT_DEFAULT;
        for (lf, v) in fields(layer) {
            match (lf, v) {
                (1, Err(b)) => name = std::str::from_utf8(b).unwrap_or_default(),
                (2, Err(b)) => features.push(b),
                (3, Err(b)) => keys.push(std::str::from_utf8(b).unwrap_or_default()),
                (4, Err(b)) => values.push(value(b)),
                (5, Ok(e)) => extent = e as f64,
                _ => {}
            }
        }
        if name != "railway_line_high" {
            continue;
        }
        for feature in features {
            let mut tags: HashMap<&str, &Val> = HashMap::new();
            let mut geometry: &[u8] = &[];
            let mut kind = 0;
            for (ff, v) in fields(feature) {
                match (ff, v) {
                    (2, Err(t)) => {
                        for kv in packed(t).chunks(2) {
                            if let [k, v] = kv
                                && let (Some(k), Some(Some(v))) = (keys.get(*k as usize), values.get(*v as usize))
                            {
                                tags.insert(k, v);
                            }
                        }
                    }
                    (3, Ok(k)) => kind = k,
                    (4, Err(g)) => geometry = g,
                    _ => {}
                }
            }
            let text = |k: &str| match tags.get(k) {
                Some(Val::Str(s)) => Some(*s),
                _ => None,
            };
            // only railways in use: no trams or metros, nor lines abandoned, razed or being built
            if kind != 2 || text("feature") != Some("rail") || text("state").is_some_and(|s| s != "present") {
                continue;
            }
            let maxspeed = match tags.get("maxspeed") {
                Some(Val::Num(n)) if *n > 0.0 => Some(*n),
                _ => None,
            };
            let track_ref = text("track_ref").map(str::to_owned);
            let service = text("service").is_some();
            let yard = matches!(text("service"), Some("yard" | "spur"));
            let flag = |k: &str| matches!(tags.get(k), Some(Val::Bool(true)));

            // MoveTo starts a line, LineTo goes on; coordinates are zigzag deltas
            let commands = packed(geometry);
            let (mut cx, mut cy) = (0i64, 0i64);
            let mut lines: Vec<Line> = Vec::new();
            let mut line: Line = Vec::new();
            let mut i = 0;
            while i < commands.len() {
                let (id, count) = (commands[i] & 7, (commands[i] >> 3) as usize);
                i += 1;
                if id == 7 {
                    continue;
                }
                for _ in 0..count {
                    let (Some(&dx), Some(&dy)) = (commands.get(i), commands.get(i + 1)) else { break };
                    i += 2;
                    cx += ((dx >> 1) as i64) ^ -((dx & 1) as i64);
                    cy += ((dy >> 1) as i64) ^ -((dy & 1) as i64);
                    if id == 1 && line.len() > 1 {
                        lines.push(std::mem::take(&mut line));
                    } else if id == 1 {
                        line.clear();
                    }
                    line.push((cx as f64, cy as f64));
                }
            }
            if line.len() > 1 {
                lines.push(line);
            }
            for piece in lines.iter().flat_map(|l| clip(l, extent)) {
                out.push(Track {
                    points: piece.iter().map(|&(px, py)| to_lat_lon(z, x, y, extent, px, py)).collect(),
                    maxspeed,
                    track_ref: track_ref.clone(),
                    service,
                    yard,
                    bridge: flag("bridge"),
                    tunnel: flag("tunnel"),
                });
            }
        }
    }
    out
}

fn disk_path(z: u32, x: u32, y: u32) -> Option<PathBuf> {
    Some(cache_dir().get()?.join(format!("{z}/{x}/{y}.pbf")))
}

/// A tile from disk, and whether it is still fresh. An empty file: no rail there.
fn from_disk(z: u32, x: u32, y: u32) -> Option<(Vec<u8>, bool)> {
    let path = disk_path(z, x, y)?;
    let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
    let fresh = SystemTime::now().duration_since(modified).is_ok_and(|age| age < DISK_FRESH);
    Some((std::fs::read(&path).ok()?, fresh))
}

fn to_disk(z: u32, x: u32, y: u32, bytes: &[u8]) {
    if let Some(path) = disk_path(z, x, y) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, bytes);
    }
}

/// A tile's bytes: from memory, else disk, else OpenRailwayMap. Empty where
/// there is no railway.
pub async fn tile_bytes(z: u32, x: u32, y: u32) -> Result<Arc<Vec<u8>>, String> {
    if let Some((at, b)) = store().lock().unwrap().bytes.get_mut(&(z, x, y)) {
        *at = Instant::now();
        return Ok(b.clone());
    }
    let stale = match from_disk(z, x, y) {
        Some((bytes, true)) => return Ok(remember(z, x, y, bytes)),
        Some((bytes, false)) => Some(bytes),
        None => None,
    };
    let url = format!("{TILES}/{z}/{x}/{y}");
    let call = crate::devlog::start("OpenRailwayMap", "GET", &url, None);
    let fetched = async {
        let response = client().ok_or("no HTTP client")?.get(&url).send().await.map_err(|e| e.to_string())?;
        match response.status().as_u16() {
            204 | 404 => Ok(Vec::new()),
            s if (200..300).contains(&s) => Ok(response.bytes().await.map_err(|e| e.to_string())?.to_vec()),
            s => Err(format!("OpenRailwayMap answered {s}")),
        }
    }
    .await;
    if let Some(c) = call {
        match &fetched {
            Ok(b) => c.finish(if b.is_empty() { 204 } else { 200 }, b),
            Err(e) => c.fail(e),
        }
    }
    match (fetched, stale) {
        (Ok(bytes), _) => {
            to_disk(z, x, y, &bytes);
            Ok(remember(z, x, y, bytes))
        }
        // an old tile beats none
        (Err(_), Some(bytes)) => Ok(remember(z, x, y, bytes)),
        (Err(e), None) => Err(e),
    }
}

fn remember(z: u32, x: u32, y: u32, bytes: Vec<u8>) -> Arc<Vec<u8>> {
    let bytes = Arc::new(bytes);
    let mut s = store().lock().unwrap();
    if s.bytes.len() >= MAX_BYTES
        && let Some(oldest) = s.bytes.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| *k)
    {
        s.bytes.remove(&oldest);
    }
    s.bytes.insert((z, x, y), (Instant::now(), bytes.clone()));
    bytes
}

/// The `railtiles:` protocol the map draws the network from: `/{z}/{x}/{y}`.
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
    if !(MIN_ZOOM..=ZOOM).contains(&z) || x >= 1 << z || y >= 1 << z {
        return respond(204, Vec::new());
    }
    match tile_bytes(z, x, y).await {
        Ok(b) if b.is_empty() => respond(204, Vec::new()),
        Ok(b) => respond(200, b.to_vec()),
        Err(e) => {
            eprintln!("rail tile {z}/{x}/{y}: {e}");
            respond(502, Vec::new())
        }
    }
}

async fn fetch(key: Key) {
    let tracks = tile_bytes(ZOOM, key.0, key.1).await.map(|b| decode(&b, ZOOM, key.0, key.1));
    let mut s = store().lock().unwrap();
    s.loading.remove(&key);
    match tracks {
        Ok(tracks) => {
            if s.tracks.len() >= MAX_TILES
                && let Some(oldest) = s.tracks.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| *k)
            {
                s.tracks.remove(&oldest);
            }
            s.tracks.insert(key, (Instant::now(), Arc::new(tracks)));
        }
        Err(e) => {
            eprintln!("rail tile {key:?}: {e}");
            s.failed.insert(key, Instant::now());
        }
    }
}

/// The tracks of these tiles if all are here; the missing ones are sent for.
fn tracks_of(keys: &[Key]) -> Option<Vec<Arc<Vec<Track>>>> {
    let mut s = store().lock().unwrap();
    let mut out = Vec::with_capacity(keys.len());
    let mut missing = Vec::new();
    for key in keys {
        match s.tracks.get_mut(key) {
            Some((at, tracks)) => {
                *at = Instant::now();
                out.push(tracks.clone());
            }
            None => missing.push(*key),
        }
    }
    // tests never go to the network
    if cfg!(test) {
        return missing.is_empty().then_some(out);
    }
    for key in &missing {
        let recently_failed = s.failed.get(key).is_some_and(|at| at.elapsed() < RETRY);
        if s.loading.len() < MAX_LOADING && !recently_failed && s.loading.insert(*key) {
            tauri::async_runtime::spawn(fetch(*key));
        }
    }
    missing.is_empty().then_some(out)
}

/// The graph of these tiles' tracks, if they are all here.
fn graph_for(mut keys: Vec<Key>) -> Option<Arc<Graph>> {
    keys.sort_unstable();
    {
        let mut s = store().lock().unwrap();
        if let Some(i) = s.graphs.iter().position(|(k, _)| *k == keys) {
            let hit = s.graphs.remove(i);
            let g = hit.1.clone();
            s.graphs.push(hit);
            return Some(g);
        }
    }
    let sets = tracks_of(&keys)?;
    let g = Arc::new(Graph::build(&sets));
    let mut s = store().lock().unwrap();
    if s.graphs.len() >= MAX_GRAPHS {
        s.graphs.remove(0);
    }
    s.graphs.push((keys, g.clone()));
    Some(g)
}

/// The tiles covering the box round these points, padded by `pad` degrees.
fn tiles_for(points: &[(f64, f64)], pad: f64) -> Vec<Key> {
    let (mut s, mut w, mut n, mut e) = (90.0f64, 180.0f64, -90.0f64, -180.0f64);
    for &(lat, lon) in points {
        (s, n, w, e) = (s.min(lat), n.max(lat), w.min(lon), e.max(lon));
    }
    let (x0, y0) = tile_of(n + pad, w - pad);
    let (x1, y1) = tile_of(s - pad, e + pad);
    let mut keys = Vec::new();
    for x in x0..=x1 {
        for y in y0..=y1 {
            keys.push((x, y));
        }
    }
    keys
}

// ------------------------------------------------------------------ graph

#[derive(Clone, Copy, Debug)]
struct Edge {
    to: usize,
    len: f64,
    /// m/s.
    speed: Option<f32>,
    service: bool,
    yard: bool,
    /// Index into `Graph::refs`.
    track: Option<u32>,
    /// Trafikverket's limits going this way (km/h per train category), where known.
    sth: Option<crate::sth::Limits>,
    /// Going this way: with the way trains run on this track (true), against
    /// it (false), or either / not known.
    flow: Option<bool>,
}

/// The limit on a piece of track for a train: Trafikverket's for its
/// category where known, else OpenRailwayMap's; never above its top speed.
/// m/s.
fn speed_of(e: &Edge, class: Option<&crate::train_class::TrainClass>) -> Option<f32> {
    let track = match (e.sth, class) {
        (Some(limits), Some(c)) => Some(f32::from(limits[c.category as usize]) / 3.6),
        (Some(limits), None) => Some(f32::from(limits[0]) / 3.6),
        (None, _) => e.speed,
    };
    match class {
        Some(c) => track.map(|v| v.min(c.top() as f32)),
        None => track,
    }
}

struct Graph {
    nodes: Vec<(f64, f64)>,
    adj: Vec<Vec<Edge>>,
    refs: Vec<String>,
}

#[derive(Clone, Copy)]
struct Attr {
    speed: Option<f32>,
    service: bool,
    yard: bool,
    track: Option<u32>,
}

/// A track's number as compared: its first word, in lower case ("2a Kofta": "2a").
fn track_name(s: &str) -> String {
    s.split_whitespace().next().unwrap_or_default().to_lowercase()
}

/// How well a mapped track's number fits the announced one, best first:
/// the same ("4b", "4b"); the track of an announced sector ("4b", "4");
/// a sector of an announced track ("4", "4a"); else not at all. Different
/// sectors are different tracks ("2b" is not "2a").
fn track_fit(announced: &str, mapped: &str) -> Option<u8> {
    let (a, m) = (track_name(announced), track_name(mapped));
    if a.is_empty() || m.is_empty() {
        return None;
    }
    if a == m {
        return Some(0);
    }
    let split = |s: &str| {
        let d = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
        (s[..d].to_owned(), s[d..].to_owned())
    };
    let ((ad, asx), (md, msx)) = (split(&a), split(&m));
    if ad.is_empty() || ad != md {
        return None;
    }
    match (asx.is_empty(), msx.is_empty()) {
        (false, true) => Some(1),
        (true, false) => Some(2),
        _ => None,
    }
}

impl Graph {
    fn build(sets: &[Arc<Vec<Track>>]) -> Graph {
        let mut g = Graph { nodes: Vec::new(), adj: Vec::new(), refs: Vec::new() };
        let mut index: HashMap<(i64, i64), usize> = HashMap::new();
        let mut refs: HashMap<String, u32> = HashMap::new();
        for track in sets.iter().flat_map(|s| s.iter()) {
            let track_index = track.track_ref.as_deref().map(|r| {
                let r = track_name(r);
                *refs.entry(r.clone()).or_insert_with(|| {
                    g.refs.push(r);
                    (g.refs.len() - 1) as u32
                })
            });
            let attr = Attr {
                speed: track.maxspeed.map(|k| (k / 3.6) as f32),
                service: track.service,
                yard: track.yard,
                track: track_index,
            };
            let mut prev: Option<usize> = None;
            for &p in &track.points {
                let k = ((p.0 * 1e6).round() as i64, (p.1 * 1e6).round() as i64);
                let n = *index.entry(k).or_insert_with(|| {
                    g.nodes.push(p);
                    g.adj.push(Vec::new());
                    g.nodes.len() - 1
                });
                if let Some(a) = prev.filter(|a| *a != n) {
                    g.link(a, n, attr);
                }
                prev = Some(n);
            }
        }
        g.stitch();
        g.add_limits();
        g
    }

    /// Trafikverket's limits and traffic flow on each piece, each way, where it has them.
    fn add_limits(&mut self) {
        let (mut s, mut w, mut n, mut e) = (90.0f64, 180.0f64, -90.0f64, -180.0f64);
        for p in &self.nodes {
            (s, n, w, e) = (s.min(p.0), n.max(p.0), w.min(p.1), e.max(p.1));
        }
        if self.nodes.is_empty() {
            return;
        }
        let (limits, flow) = (crate::sth::covers(s, w, n, e), crate::flow::covers(s, w, n, e));
        if !limits && !flow {
            return;
        }
        for a in 0..self.nodes.len() {
            for i in 0..self.adj[a].len() {
                let (pa, pb) = (self.nodes[a], self.nodes[self.adj[a][i].to]);
                if limits {
                    self.adj[a][i].sth = crate::sth::along(pa, pb);
                }
                if flow {
                    self.adj[a][i].flow = crate::flow::with_flow(pa, pb);
                }
            }
        }
        if flow {
            self.spread_flow();
        }
    }

    /// Along each stretch of track between switches (or ends), the way
    /// trains run where the data says so for enough of it, and nowhere the
    /// other way: also where the map's line strays from Trafikverket's.
    fn spread_flow(&mut self) {
        for start in 0..self.nodes.len() {
            if self.adj[start].len() == 2 {
                continue;
            }
            for first in 0..self.adj[start].len() {
                // the stretch: on through nodes with only one way on
                let mut stretch = vec![(start, self.adj[start][first].to)];
                while let Some(&(a, b)) = stretch.last() {
                    if self.adj[b].len() != 2 || b == start || stretch.len() > 100_000 {
                        break;
                    }
                    let next = self.adj[b].iter().map(|e| e.to).find(|n| *n != a).unwrap_or(a);
                    stretch.push((b, next));
                }
                // what is known of it, either way along it, as going along the stretch
                let (mut with, mut against, mut total) = (0.0, 0.0, 0.0);
                for &(a, b) in &stretch {
                    let Some(e) = self.edge(a, b) else { continue };
                    total += e.len;
                    let back = self.edge(b, a).and_then(|e| e.flow).map(|f| !f);
                    for f in [e.flow, back].into_iter().flatten() {
                        if f {
                            with += e.len;
                        } else {
                            against += e.len;
                        }
                    }
                }
                let known = with + against;
                if known == 0.0 || (with > 0.0 && against > 0.0) || known < (0.3 * total).min(SPREAD_FLOW_M) {
                    continue;
                }
                let along = with > 0.0;
                for &(a, b) in &stretch {
                    for (x, y, f) in [(a, b, along), (b, a, !along)] {
                        if let Some(e) = self.adj[x].iter_mut().find(|e| e.to == y)
                            && e.flow.is_none()
                        {
                            e.flow = Some(f);
                        }
                    }
                }
            }
        }
    }

    fn link(&mut self, a: usize, b: usize, attr: Attr) {
        if a == b || self.adj[a].iter().any(|e| e.to == b) {
            return;
        }
        let len = distance(self.nodes[a], self.nodes[b]);
        let edge =
            |to| Edge { to, len, speed: attr.speed, service: attr.service, yard: attr.yard, track: attr.track, sth: None, flow: None };
        self.adj[a].push(edge(b));
        self.adj[b].push(edge(a));
    }

    fn attr(e: &Edge) -> Attr {
        Attr { speed: e.speed, service: e.service, yard: e.yard, track: e.track }
    }

    fn edge(&self, a: usize, b: usize) -> Option<&Edge> {
        self.adj[a].iter().find(|e| e.to == b)
    }

    fn unlink(&mut self, a: usize, b: usize) {
        self.adj[a].retain(|e| e.to != b);
        self.adj[b].retain(|e| e.to != a);
    }

    /// `from`'s tracks run into `into` instead; `from` is left alone.
    fn merge(&mut self, from: usize, into: usize) {
        for e in std::mem::take(&mut self.adj[from]) {
            self.adj[e.to].retain(|x| x.to != from);
            self.link(into, e.to, Self::attr(&e));
        }
    }

    /// A node at `q` on the segment `a`-`b`.
    fn split(&mut self, a: usize, b: usize, q: (f64, f64)) -> Option<usize> {
        let attr = Self::attr(self.edge(a, b)?);
        self.unlink(a, b);
        self.nodes.push(q);
        self.adj.push(Vec::new());
        let n = self.nodes.len() - 1;
        self.link(a, n, attr);
        self.link(n, b, attr);
        Some(n)
    }

    /// Joins tracks where the tiles don't quite: pieces cut at a tile's edge,
    /// and track ends running into another track between its nodes. Only
    /// within a couple of metres: parallel tracks are 4.5 m apart and must
    /// stay apart.
    fn stitch(&mut self) {
        const CELL: f64 = 0.0002;
        let cell = |p: (f64, f64)| ((p.0 / CELL).floor() as i64, (p.1 / CELL).floor() as i64);
        let around = |c: (i64, i64)| (c.0 - 1..=c.0 + 1).flat_map(move |x| (c.1 - 1..=c.1 + 1).map(move |y| (x, y)));

        // seams: ends on a tile's edge meet the end across it
        let seam_ends: Vec<usize> =
            (0..self.nodes.len()).filter(|i| self.adj[*i].len() == 1 && on_tile_edge(self.nodes[*i])).collect();
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for &i in &seam_ends {
            grid.entry(cell(self.nodes[i])).or_default().push(i);
        }
        for &a in &seam_ends {
            if self.adj[a].len() != 1 {
                continue;
            }
            let neighbour = self.adj[a][0].to;
            let best = around(cell(self.nodes[a]))
                .filter_map(|c| grid.get(&c))
                .flatten()
                .filter(|&&b| b != a && b != neighbour && self.adj[b].len() == 1)
                .map(|&b| (b, distance(self.nodes[a], self.nodes[b])))
                .filter(|(_, d)| *d <= SEAM_M)
                .min_by(|x, y| x.1.total_cmp(&y.1));
            if let Some((b, _)) = best {
                self.merge(a, b);
            }
        }

        // ends running into a track between its nodes
        let mut segments: HashMap<(i64, i64), Vec<(usize, usize)>> = HashMap::new();
        for a in 0..self.nodes.len() {
            for e in &self.adj[a] {
                if e.to < a {
                    continue;
                }
                let (c1, c2) = (cell(self.nodes[a]), cell(self.nodes[e.to]));
                for x in c1.0.min(c2.0)..=c1.0.max(c2.0) {
                    for y in c1.1.min(c2.1)..=c1.1.max(c2.1) {
                        segments.entry((x, y)).or_default().push((a, e.to));
                    }
                }
            }
        }
        let ends: Vec<usize> = (0..self.nodes.len()).filter(|i| self.adj[*i].len() == 1).collect();
        for a in ends {
            if self.adj[a].len() != 1 {
                continue;
            }
            let p = self.nodes[a];
            let neighbour = self.adj[a][0].to;
            let mut seen = HashSet::new();
            let best = around(cell(p))
                .filter_map(|c| segments.get(&c))
                .flatten()
                .filter(|s| seen.insert(**s))
                .filter(|(u, v)| ![*u, *v].contains(&a) && ![*u, *v].contains(&neighbour))
                .filter(|(u, v)| self.edge(*u, *v).is_some())
                .map(|&(u, v)| {
                    let (q, _) = project(p, self.nodes[u], self.nodes[v]);
                    (u, v, q, distance(p, q))
                })
                .filter(|s| s.3 <= JOIN_M)
                .min_by(|x, y| x.3.total_cmp(&y.3));
            if let Some((u, v, q, _)) = best {
                // onto a node it nearly meets, else a new one on the segment
                let into = if distance(q, self.nodes[u]) < 0.5 {
                    Some(u)
                } else if distance(q, self.nodes[v]) < 0.5 {
                    Some(v)
                } else {
                    self.split(u, v, q)
                };
                if let Some(into) = into {
                    self.merge(a, into);
                }
            }
        }
    }

    /// Every track segment within `within` of `p`, nearest first: its ends,
    /// the nearest point on it, and how far off that is.
    fn snaps(&self, p: (f64, f64), within: f64) -> Vec<(usize, usize, (f64, f64), f64)> {
        let mut out = Vec::new();
        for a in 0..self.nodes.len() {
            for e in &self.adj[a] {
                if e.to < a {
                    continue;
                }
                let (q, _) = project(p, self.nodes[a], self.nodes[e.to]);
                let d = distance(p, q);
                if d <= within {
                    out.push((a, e.to, q, d));
                }
            }
        }
        out.sort_by(|x, y| x.3.total_cmp(&y.3));
        out
    }

    fn snap(&self, p: (f64, f64), within: f64) -> Option<(usize, usize, (f64, f64), f64)> {
        self.snaps(p, within).into_iter().next()
    }

    /// Whether a train can go on from `from` through `at` to `to`.
    fn turnable(&self, from: usize, at: usize, to: usize) -> bool {
        let (f, a, t) = (self.nodes[from], self.nodes[at], self.nodes[to]);
        distance(f, a) < SHORT_M || distance(a, t) < SHORT_M || turn(course(f, a), course(a, t)) <= MAX_TURN
    }

    fn cost(e: &Edge) -> f64 {
        let mut c = if e.service { e.len * SERVICE_COST } else { e.len };
        if e.flow == Some(false) {
            c *= WRONG_WAY_COST;
        }
        c
    }

    /// The cheapest way on from standing at `at` having come from `came`, to
    /// the segment `ta`-`tb`, entering it at either end. The nodes from `at`
    /// to the end it enters by, and the cost.
    fn path(&self, at: usize, came: usize, ta: usize, tb: usize) -> Option<(Vec<usize>, f64)> {
        #[derive(PartialEq)]
        struct Item(f64, usize, usize);
        impl Eq for Item {}
        impl Ord for Item {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                o.0.total_cmp(&self.0)
            }
        }
        impl PartialOrd for Item {
            fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(o))
            }
        }
        // states: at a node, having come from another (trains can't turn back)
        let mut dist: HashMap<(usize, usize), f64> = HashMap::new();
        let mut prev: HashMap<(usize, usize), (usize, usize)> = HashMap::new();
        let mut heap = BinaryHeap::new();
        dist.insert((at, came), 0.0);
        heap.push(Item(0.0, at, came));
        let mut end = None;
        while let Some(Item(d, n, from)) = heap.pop() {
            if dist.get(&(n, from)).is_some_and(|best| d > *best) {
                continue;
            }
            // at an end of the target segment, able to run onto it
            if (n == ta || n == tb) && self.turnable(from, n, if n == ta { tb } else { ta }) {
                end = Some(((n, from), d));
                break;
            }
            for e in &self.adj[n] {
                if e.to == from || !self.turnable(from, n, e.to) {
                    continue;
                }
                let nd = d + Self::cost(e);
                if dist.get(&(e.to, n)).is_none_or(|best| nd < *best) {
                    dist.insert((e.to, n), nd);
                    prev.insert((e.to, n), (n, from));
                    heap.push(Item(nd, e.to, n));
                }
            }
        }
        let (mut state, cost) = end?;
        let mut out = vec![state.0];
        while state != (at, came) {
            state = *prev.get(&state)?;
            out.push(state.0);
        }
        out.reverse();
        Some((out, cost))
    }

    /// Onwards from `at`, having come from `came`, as straight as the track
    /// allows and keeping to running lines, for `metres`.
    fn onwards(&self, mut came: usize, mut at: usize, metres: f64) -> Vec<usize> {
        let mut out = Vec::new();
        let mut left = metres;
        let mut seen = HashSet::from([came, at]);
        while left > 0.0 {
            let heading = course(self.nodes[came], self.nodes[at]);
            let next = self.adj[at]
                .iter()
                .filter(|e| !seen.contains(&e.to))
                .map(|e| (e, turn(heading, course(self.nodes[at], self.nodes[e.to]))))
                .filter(|(e, t)| *t <= MAX_TURN || e.len < SHORT_M)
                .min_by(|a, b| {
                    let score = |(e, t): &(&Edge, f64)| {
                        t + if e.service { 30.0 } else { 0.0 } + if e.flow == Some(false) { 45.0 } else { 0.0 }
                    };
                    score(a).total_cmp(&score(b))
                });
            let Some((e, _)) = next else { break };
            out.push(e.to);
            seen.insert(e.to);
            left -= e.len;
            (came, at) = (at, e.to);
        }
        out
    }
}

/// The nearest point to `p` on the segment `a`-`b`, and how far along it (0 to 1).
fn project(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> ((f64, f64), f64) {
    // flat projection: plenty for a segment of a few hundred metres
    let k = p.0.to_radians().cos();
    let (ax, ay, bx, by, px, py) = (a.1 * k, a.0, b.1 * k, b.0, p.1 * k, p.0);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    ((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t), t)
}

fn turn(a: f64, b: f64) -> f64 {
    let d = (b - a).rem_euclid(360.0);
    d.min(360.0 - d)
}

// ------------------------------------------------------------------ routes

/// A way along the track: its points, how far along it each target was
/// reached, and each segment's speed limit (m/s).
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub points: Vec<(f64, f64)>,
    pub reached: Vec<f64>,
    pub speeds: Vec<Option<f64>>,
}

/// Where a train is on the track: on the segment `behind`-`front`, at `on`, heading for `front`.
#[derive(Clone, Copy)]
struct Place {
    on: (f64, f64),
    behind: usize,
    front: usize,
}

struct Builder<'a> {
    route: Route,
    length: f64,
    cost: f64,
    /// The train's class: which limits apply, and its top speed.
    class: Option<&'a crate::train_class::TrainClass>,
}

impl Builder<'_> {
    /// On to `p` along a piece of track (its limit for the segment).
    fn on(&mut self, p: (f64, f64), e: Option<&Edge>) {
        let speed = e.and_then(|e| speed_of(e, self.class));
        self.push(p, speed);
    }

    fn push(&mut self, p: (f64, f64), speed: Option<f32>) {
        let Some(&last) = self.route.points.last() else {
            self.route.points.push(p);
            return;
        };
        let d = distance(last, p);
        if d < 0.01 {
            return;
        }
        self.length += d;
        self.route.points.push(p);
        self.route.speeds.push(speed.map(f64::from));
    }
}

/// From `place` through the targets in order, then on for `onwards_m`;
/// as far as the track goes.
fn route_from<'a>(
    g: &Graph,
    place: Place,
    targets: &[(f64, f64)],
    onwards_m: f64,
    class: Option<&'a crate::train_class::TrainClass>,
) -> Option<Builder<'a>> {
    let mut b = Builder {
        route: Route { points: vec![place.on], reached: Vec::new(), speeds: Vec::new() },
        length: 0.0,
        cost: 0.0,
        class,
    };
    let mut at = place;
    for target in targets {
        let Some((ta, tb, tq, _)) = g.snap(*target, STATION_SNAP_M) else { break };
        let on_this = (at.behind, at.front) == (ta, tb) || (at.behind, at.front) == (tb, ta);
        if on_this && distance(g.nodes[at.behind], tq) >= distance(g.nodes[at.behind], at.on) - 0.01 {
            // further along the segment it is on
            b.cost += distance(at.on, tq);
            b.on(tq, g.edge(at.behind, at.front));
            at.on = tq;
        } else {
            let first = *g.edge(at.behind, at.front)?;
            let Some((nodes, cost)) = g.path(at.front, at.behind, ta, tb) else { break };
            b.cost += distance(at.on, g.nodes[at.front]) * Graph::cost(&first) / first.len.max(0.01);
            b.on(g.nodes[at.front], Some(&first));
            for w in nodes.windows(2) {
                b.on(g.nodes[w[1]], g.edge(w[0], w[1]));
            }
            b.cost += cost;
            let entered = *nodes.last()?;
            let other = if entered == ta { tb } else { ta };
            b.on(tq, g.edge(entered, other));
            b.cost += distance(g.nodes[entered], tq);
            at = Place { on: tq, behind: entered, front: other };
        }
        b.route.reached.push(b.length);
    }
    if onwards_m > 0.0 {
        b.on(g.nodes[at.front], g.edge(at.behind, at.front));
        let mut prev = at.front;
        for n in g.onwards(at.behind, at.front, onwards_m) {
            b.on(g.nodes[n], g.edge(prev, n));
            prev = n;
        }
    }
    Some(b)
}

/// The ways a train at `start` could be on the track: the nearest segment
/// and those just as likely (up to `spread` metres further), each way along
/// it the heading allows.
fn places(g: &Graph, start: (f64, f64), heading: Option<f64>, spread: f64) -> Vec<(Place, f64)> {
    let snaps = g.snaps(start, SNAP_M);
    let Some(nearest) = snaps.first().map(|s| s.3) else { return Vec::new() };
    let mut out = Vec::new();
    for (a, b, on, d) in snaps.into_iter().take_while(|s| s.3 <= nearest + spread).take(6) {
        out.push((Place { on, behind: a, front: b }, d));
        out.push((Place { on, behind: b, front: a }, d));
    }
    // with a heading, the way it points first
    if let Some(h) = heading {
        out.sort_by_key(|(p, _)| turn(h, course(g.nodes[p.behind], g.nodes[p.front])) > 90.0);
    }
    out
}

/// A way that goes against the train's heading costs this much more
/// (metres' worth): the heading tips the balance, the next station decides.
const AGAINST_HEADING: f64 = 400.0;

/// `route_in`, going only the way `heading` points (on from a station
/// without turning back there); with no heading, as `route_in`.
fn route_in_onwards(
    g: &Graph,
    start: (f64, f64),
    heading: Option<f64>,
    targets: &[(f64, f64)],
    onwards_m: f64,
    spread: f64,
) -> Option<Route> {
    route_in_dir(g, start, heading, targets, onwards_m, spread, None, true)
}

fn route_in(
    g: &Graph,
    start: (f64, f64),
    heading: Option<f64>,
    targets: &[(f64, f64)],
    onwards_m: f64,
    spread: f64,
) -> Option<Route> {
    route_in_dir(g, start, heading, targets, onwards_m, spread, None, false)
}

/// The way from `start`: towards the targets whichever way the heading
/// points, or (`only_onwards`, or no targets) only the way it points.
#[allow(clippy::too_many_arguments)]
fn route_in_dir(
    g: &Graph,
    start: (f64, f64),
    heading: Option<f64>,
    targets: &[(f64, f64)],
    onwards_m: f64,
    spread: f64,
    class: Option<&crate::train_class::TrainClass>,
    only_onwards: bool,
) -> Option<Route> {
    // the likeliest: reaching the most targets (it goes towards its next
    // station, whichever way it seemed to point), the cheapest way, near the report
    let candidates = places(g, start, heading, spread);
    let candidates = if heading.is_some() && (targets.is_empty() || only_onwards) {
        // nowhere it must go: the way it points
        let n = candidates.len().div_ceil(2);
        candidates.into_iter().take(n).collect()
    } else {
        candidates
    };
    let best = candidates
        .into_iter()
        .filter_map(|(place, off)| {
            let edge = g.edge(place.behind, place.front);
            let service = edge.is_some_and(|e| e.service);
            // on the track running the other way: possible, but unlikely
            let wrong_way = edge.is_some_and(|e| e.flow == Some(false));
            let way = course(g.nodes[place.behind], g.nodes[place.front]);
            let against_heading = heading.is_some_and(|h| turn(h, way) > 90.0);
            // reaching none of them: at least towards the first
            let away = targets.first().is_some_and(|t| turn(course(place.on, *t), way) > 90.0);
            let b = route_from(g, place, targets, onwards_m, class)?;
            let reached = b.route.reached.len();
            let score = b.cost
                + off * 20.0
                + if service && targets.is_empty() { 50.0 } else { 0.0 }
                + if wrong_way { 200.0 } else { 0.0 }
                + if against_heading { AGAINST_HEADING } else { 0.0 }
                + if reached == 0 && away { 2.0 * AGAINST_HEADING } else { 0.0 };
            Some((reached, score, b))
        })
        .min_by(|x, y| y.0.cmp(&x.0).then(x.1.total_cmp(&y.1)))?;
    let route = best.2.route;
    (route.points.len() >= 2).then_some(route)
}

/// How far on past its targets a way's track is loaded for, at most, metres.
const LOAD_ON_M: f64 = 8_000.0;

/// A way along the known track from `start` (heading roughly `heading`)
/// through as many of `targets` as it can reach in order, then on along the
/// track for `onwards_m`, on the limits for a train of this class; with
/// `only_onwards`, going only the way `heading` points (else whichever way
/// reaches the targets). None while the track there isn't loaded (it is sent
/// for), or when `start` isn't on a track.
pub fn route_for(
    start: (f64, f64),
    heading: Option<f64>,
    targets: &[(f64, f64)],
    onwards_m: f64,
    class: Option<&crate::train_class::TrainClass>,
    only_onwards: bool,
) -> Option<Route> {
    let mut targets = targets.to_vec();
    let on = onwards_m.clamp(0.0, LOAD_ON_M);
    // the track round the start and as far on as it goes
    let near = match heading {
        Some(h) => tiles_for(&[start, ahead(start, h, on)], 0.01),
        None => tiles_for(&[start], 0.01 + on / 111_000.0),
    };
    if near.len() > MAX_ROUTE_TILES {
        return None;
    }
    let mut graph = None;
    if !targets.is_empty() {
        let mut points = vec![start];
        points.extend(&targets);
        // and on past the last target, the way they lead, when they end close by
        let n = points.len();
        if on > 0.0 && distance(start, points[n - 1]) < on {
            points.push(ahead(points[n - 1], course(points[n - 2], points[n - 1]), on));
        }
        let all = tiles_for(&points, 0.02);
        if all.len() <= MAX_ROUTE_TILES {
            graph = graph_for(all);
        }
        // the track to far targets not here yet (it is sent for): those on the track that is
        if graph.is_none() {
            targets.retain(|t| near.contains(&tile_of(t.0, t.1)));
        }
    }
    let g = match graph {
        Some(g) => g,
        None => graph_for(near)?,
    };
    route_in_dir(&g, start, heading, &targets, onwards_m, SNAP_SPREAD_M, class, only_onwards)
}

/// Tests: every `{x}_{y}.pbf` tile in `dir` (zoom `ZOOM`) as if fetched.
#[cfg(test)]
pub fn load_tiles_for_tests(dir: &str) {
    let mut st = store().lock().unwrap();
    for e in std::fs::read_dir(dir).unwrap() {
        let name = e.unwrap().file_name().into_string().unwrap();
        let Some((x, y)) = name.strip_suffix(".pbf").and_then(|n| n.split_once('_')) else { continue };
        let (x, y) = (x.parse().unwrap(), y.parse().unwrap());
        let tracks = decode(&std::fs::read(format!("{dir}/{name}")).unwrap_or_default(), ZOOM, x, y);
        st.tracks.insert((x, y), (Instant::now(), Arc::new(tracks)));
    }
}

/// The track from `start` going `heading` (degrees) for `metres`, straight on
/// at switches whichever way traffic usually runs there: what lies behind a
/// train, given the way back. None while the track isn't loaded, or `start`
/// isn't on it.
pub fn track_from(start: (f64, f64), heading: f64, metres: f64) -> Option<Vec<(f64, f64)>> {
    // the tiles round it only (a train's length from it at most): the same for every train there, built once
    let g = graph_for(tiles_for(&[start], (metres / 111_000.0).min(0.003)))?;
    let (a, b, q, _) = g.snap(start, SNAP_SPREAD_M)?;
    let (mut came, mut at) = if turn(heading, course(g.nodes[a], g.nodes[b])) <= 90.0 { (a, b) } else { (b, a) };
    let mut out = vec![q, g.nodes[at]];
    let mut left = metres - distance(q, g.nodes[at]);
    let mut seen = HashSet::from([came, at]);
    while left > 0.0 {
        let way = course(g.nodes[came], g.nodes[at]);
        let next = g.adj[at]
            .iter()
            .filter(|e| !seen.contains(&e.to))
            .map(|e| (e, turn(way, course(g.nodes[at], g.nodes[e.to]))))
            .filter(|(e, t)| *t <= MAX_TURN || e.len < SHORT_M)
            .min_by(|x, y| (x.1 + if x.0.service { 30.0 } else { 0.0 }).total_cmp(&(y.1 + if y.0.service { 30.0 } else { 0.0 })));
        let Some((e, _)) = next else { break };
        let p = g.nodes[e.to];
        if e.len >= left {
            // as far as it reaches, part way along the last piece
            let k = left / e.len.max(0.01);
            let from = g.nodes[at];
            out.push((from.0 + (p.0 - from.0) * k, from.1 + (p.1 - from.1) * k));
            break;
        }
        out.push(p);
        seen.insert(e.to);
        left -= e.len;
        (came, at) = (at, e.to);
    }
    Some(out)
}

/// A bridge or a tunnel on the track: its line (`[lon, lat]`), and whether
/// it goes on as one past each end (cut by a tile's edge, or mapped in pieces).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Structure {
    pub tunnel: bool,
    pub line: Vec<[f64; 2]>,
    pub joined: [bool; 2],
}

/// The bridges and tunnels of these tiles' tracks that `keep` keeps.
fn structures_of(sets: &[Arc<Vec<Track>>], keep: impl Fn(&Track) -> bool) -> Vec<Structure> {
    let all: Vec<&Track> = sets.iter().flat_map(|s| s.iter()).filter(|t| (t.bridge || t.tunnel) && t.points.len() >= 2).collect();
    let ends = |t: &Track| [t.points[0], t.points[t.points.len() - 1]];
    all.iter()
        .filter(|t| keep(t))
        .map(|t| {
            let joined = |p| all.iter().any(|o| !std::ptr::eq(*o, *t) && o.tunnel == t.tunnel && ends(o).iter().any(|q| distance(*q, p) < SEAM_M));
            let [start, end] = ends(t);
            Structure { tunnel: t.tunnel, line: t.points.iter().map(|p| [p.1, p.0]).collect(), joined: [joined(start), joined(end)] }
        })
        .collect()
}

/// Whether a line's box and an area (south, west, north, east) overlap.
fn overlaps(line: &[(f64, f64)], (south, west, north, east): (f64, f64, f64, f64)) -> bool {
    let (mut s, mut w, mut n, mut e) = (90.0f64, 180.0f64, -90.0f64, -180.0f64);
    for &(lat, lon) in line {
        (s, n, w, e) = (s.min(lat), n.max(lat), w.min(lon), e.max(lon));
    }
    s <= north && n >= south && w <= east && e >= west
}

/// The bridges and tunnels on the track in an area (degrees); none while its tiles aren't all here.
pub fn structures_in(south: f64, west: f64, north: f64, east: f64) -> Vec<Structure> {
    let Some(sets) = tracks_of(&tiles_for(&[(south, west), (north, east)], 0.0)) else { return Vec::new() };
    structures_of(&sets, |t| overlaps(&t.points, (south, west, north, east)))
}

/// The bridges and tunnels the track has along a way (`(lat, lon)`, in
/// order: where a train is drawn), within `within` metres of it; none while
/// its tiles aren't all here.
pub fn structures_along(way: &[(f64, f64)], within: f64) -> Vec<Structure> {
    if way.len() < 2 {
        return Vec::new();
    }
    let Some(sets) = tracks_of(&tiles_for(way, 0.0)) else { return Vec::new() };
    structures_near(&sets, way, within)
}

fn structures_near(sets: &[Arc<Vec<Track>>], way: &[(f64, f64)], within: f64) -> Vec<Structure> {
    let near = |p: (f64, f64), line: &[(f64, f64)]| line.windows(2).any(|w| distance(p, project(p, w[0], w[1]).0) <= within);
    // (a box round the way first: most of the tiles' tracks are nowhere near it)
    let pad = within / 100_000.0 + 1e-4;
    let (mut s, mut w, mut n, mut e) = (90.0f64, 180.0f64, -90.0f64, -180.0f64);
    for &(lat, lon) in way {
        (s, n, w, e) = (s.min(lat), n.max(lat), w.min(lon), e.max(lon));
    }
    let k = s.abs().max(n.abs()).to_radians().cos().max(0.1);
    let area = (s - pad, w - pad / k, n + pad, e + pad / k);
    structures_of(sets, |t| {
        overlaps(&t.points, area)
            && (t.points.iter().any(|p| near(*p, way)) || way.iter().any(|p| near(*p, &t.points)))
    })
}

/// The nearest point on the track, if it is loaded and near.
pub fn snap(p: (f64, f64)) -> Option<(f64, f64)> {
    let g = graph_for(tiles_for(&[p], 0.005))?;
    g.snap(p, SNAP_M).map(|s| s.2)
}

/// Where a train stops at a station: on its announced track (`"4b"` is
/// track 4b, else track 4), at the point nearest the station's; else on the
/// running line that runs the way it goes there (`heading`, degrees), the
/// nearest. None while the track there isn't loaded, or when there is none.
pub fn platform(station: (f64, f64), track: Option<&str>, heading: Option<f64>) -> Option<(f64, f64)> {
    platforms(station, track, heading)?.into_iter().next()
}

/// The places a train might stop at a station, likeliest first: its
/// announced track, then the running lines near the station, those running
/// the way it goes (`heading`) first, nearest first (a station whose tracks
/// aren't numbered in the map has several). None while the track there
/// isn't loaded.
pub fn platforms(station: (f64, f64), track: Option<&str>, heading: Option<f64>) -> Option<Vec<(f64, f64)>> {
    // the platforms the map has there, with the tracks they serve (sent for the first time)
    let mapped = track.and(crate::osm_platforms::near(station));
    // the same for every train calling there the same way (worked out once: it takes the track round
    // it), and once more when the mapped platforms come
    let key = (
        (station.0 * 1e5).round() as i64,
        (station.1 * 1e5).round() as i64,
        track.map(str::to_owned),
        heading.map(|h| h.round() as i32),
        mapped.is_some(),
    );
    if let Some(found) = platform_cache().lock().unwrap().get(&key) {
        return Some(found.clone());
    }
    let g = graph_for(tiles_for(&[station], 0.015))?;
    let found = platforms_in(&g, station, track, heading, mapped.as_deref().map_or(&[], Vec::as_slice));
    let mut cache = platform_cache().lock().unwrap();
    if cache.len() >= MAX_PLATFORM_CACHE {
        cache.clear();
    }
    cache.insert(key, found.clone());
    Some(found)
}

/// Stations' platforms worked out, at most.
const MAX_PLATFORM_CACHE: usize = 50_000;

type PlatformKey = (i64, i64, Option<String>, Option<i32>, bool);
type PlatformCache = Mutex<HashMap<PlatformKey, Vec<(f64, f64)>>>;

/// The places found at each station (`platforms`), by where it is, the track and the way trains go.
fn platform_cache() -> &'static PlatformCache {
    static C: OnceLock<PlatformCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// How many places a station offers at most.
const MAX_PLATFORMS: usize = 6;
/// Two places nearer than this are the same track, metres.
const SAME_TRACK_M: f64 = 3.0;

/// A track segment near a point: its ends, the nearest point on it, and how far off that is.
type Snap = (usize, usize, (f64, f64), f64);

/// Tracks this near a platform's middle are beside it, metres (an island
/// platform is up to about 12 m wide).
const BESIDE_M: f64 = 15.0;

fn platforms_in(
    g: &Graph,
    station: (f64, f64),
    track: Option<&str>,
    heading: Option<f64>,
    mapped: &[crate::osm_platforms::Platform],
) -> Vec<(f64, f64)> {
    let near = g.snaps(station, PLATFORM_M);
    let mut out: Vec<(f64, f64)> = Vec::new();
    let add = |p: (f64, f64), out: &mut Vec<(f64, f64)>| {
        if out.len() < MAX_PLATFORMS && out.iter().all(|q| distance(*q, p) >= SAME_TRACK_M) {
            out.push(p);
        }
    };
    // which way a piece of track runs, for a train going `heading`: its way, either, the other
    let way = |s: &(usize, usize, (f64, f64), f64)| -> u8 {
        let Some(h) = heading else { return 1 };
        let forward = turn(h, course(g.nodes[s.0], g.nodes[s.1])) < 90.0;
        let e = if forward { g.edge(s.0, s.1) } else { g.edge(s.1, s.0) };
        match e.and_then(|e| e.flow) {
            Some(true) => 0,
            None => 1,
            Some(false) => 2,
        }
    };
    // the announced track: the best fit, nearest first
    if let Some(announced) = track {
        let mut fits: Vec<(u8, f64, (f64, f64))> = near
            .iter()
            .filter_map(|s| {
                let r = g.edge(s.0, s.1)?.track?;
                Some((track_fit(announced, &g.refs[r as usize])?, s.3, s.2))
            })
            .collect();
        fits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        if let Some(best) = fits.first().map(|f| f.0) {
            // that track only: one place on it, by the middle of a platform serving it, else nearest the station's point
            let fits_best = |s: &Snap| {
                g.edge(s.0, s.1).and_then(|e| e.track).and_then(|r| track_fit(announced, &g.refs[r as usize])) == Some(best)
            };
            let by_platform = platform_middles(announced, mapped)
                .into_iter()
                .find_map(|mid| g.snaps(mid, BESIDE_M).into_iter().find(|s| fits_best(s)).map(|s| s.2));
            if let Some(p) = by_platform.or_else(|| fits.iter().find(|f| f.0 == best).map(|f| f.2)) {
                add(p, &mut out);
            }
        } else {
            // not numbered in the map: beside the platforms that serve it
            for p in beside_platforms(g, announced, mapped, &way) {
                add(p, &mut out);
            }
        }
    }
    // then the running lines through the station, those running its way
    // first (on double track, never the one for trains the other way while
    // its own is there), nearest first; sidings; never a yard
    let mut lines: Vec<&(usize, usize, (f64, f64), f64)> = near.iter().filter(|s| s.3 <= STATION_SNAP_M).collect();
    lines.sort_by(|a, b| way(a).cmp(&way(b)).then(a.3.total_cmp(&b.3)));
    for running in [true, false] {
        for s in &lines {
            if g.edge(s.0, s.1).is_some_and(|e| !e.yard && e.service != running) {
                add(s.2, &mut out);
            }
        }
    }
    out
}

/// The platforms mapped as serving the announced track, with their middles.
fn serving<'a>(
    announced: &str,
    mapped: &'a [crate::osm_platforms::Platform],
) -> impl Iterator<Item = (&'a crate::osm_platforms::Platform, (f64, f64))> {
    mapped.iter().filter(|p| !p.line.is_empty() && p.tracks.iter().any(|t| track_fit(announced, t).is_some())).map(|p| {
        let n = p.line.len() as f64;
        (p, p.line.iter().fold((0.0, 0.0), |a, q| (a.0 + q.0 / n, a.1 + q.1 / n)))
    })
}

fn platform_middles(announced: &str, mapped: &[crate::osm_platforms::Platform]) -> Vec<(f64, f64)> {
    serving(announced, mapped).map(|(_, mid)| mid).collect()
}

/// Where the announced track is by the platforms mapped as serving it: the
/// track beside a platform serving it alone; of one serving it and others (an
/// island platform), the track on each side, one numbered otherwise in the
/// map left out, those running the train's way (`way`) first. At the
/// platform's middle.
fn beside_platforms(
    g: &Graph,
    announced: &str,
    mapped: &[crate::osm_platforms::Platform],
    way: &dyn Fn(&Snap) -> u8,
) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for (p, mid) in serving(announced, mapped) {
        let beside: Vec<_> =
            g.snaps(mid, BESIDE_M).into_iter().filter(|s| g.edge(s.0, s.1).is_some_and(|e| !e.yard)).collect();
        let Some(first) = beside.first() else {
            continue;
        };
        let mut sides = vec![*first];
        if p.tracks.len() > 1 {
            // the nearest across the platform from it too
            let off =
                |s: &(usize, usize, (f64, f64), f64)| ((s.2.0 - mid.0), (s.2.1 - mid.1) * mid.0.to_radians().cos());
            let o = off(first);
            if let Some(other) = beside.iter().find(|s| {
                let q = off(s);
                q.0 * o.0 + q.1 * o.1 < 0.0
            }) {
                sides.push(*other);
            }
        }
        // a side numbered as another track isn't this one
        sides.retain(|s| {
            g.edge(s.0, s.1).and_then(|e| e.track).is_none_or(|r| track_fit(announced, &g.refs[r as usize]).is_some())
        });
        sides.sort_by_key(|s| way(s));
        out.extend(sides.iter().map(|s| s.2));
    }
    out
}

/// Where a train standing near `platform` is: its reported place moved onto
/// the platform's track (it is at the platform, along it where it said).
/// None while the track isn't loaded.
pub fn hold(at: (f64, f64), platform: (f64, f64)) -> Option<(f64, f64)> {
    let g = graph_for(tiles_for(&[at, platform], 0.005))?;
    hold_in(&g, at, platform)
}

fn hold_in(g: &Graph, at: (f64, f64), platform: (f64, f64)) -> Option<(f64, f64)> {
    let (a, b, _, _) = g.snap(platform, 1.0)?;
    // the platform's track: from its segment on in both directions, as straight as it goes
    let mut chain: Vec<usize> = g.onwards(b, a, 800.0);
    chain.reverse();
    chain.extend([a, b]);
    chain.extend(g.onwards(a, b, 800.0));
    chain
        .windows(2)
        .map(|w| project(at, g.nodes[w[0]], g.nodes[w[1]]).0)
        .min_by(|p, q| distance(at, *p).total_cmp(&distance(at, *q)))
}

/// The way along one leg, from `from` (heading roughly `heading`) to `to`;
/// turning back at the station it leaves when it must (a terminus).
fn leg(g: &Graph, from: (f64, f64), heading: Option<f64>, to: (f64, f64)) -> Option<Route> {
    let reaches = |r: &Route| r.reached.len() == 1;
    route_in_onwards(g, from, heading, &[to], 0.0, 1.0)
        .filter(reaches)
        .or_else(|| heading.and_then(|_| route_in(g, from, None, &[to], 0.0, 1.0)).filter(reaches))
}

/// Whether a train can go on from `from` (arriving so) to `to` without turning back.
fn goes_on(g: &Graph, from: (f64, f64), heading: Option<f64>, to: (f64, f64)) -> bool {
    route_in_onwards(g, from, heading, &[to], 0.0, 1.0).is_some_and(|r| r.reached.len() == 1)
}

fn end_heading(points: &[(f64, f64)]) -> Option<f64> {
    (points.len() >= 2).then(|| course(points[points.len() - 2], points[points.len() - 1]))
}

/// The way along the track through a run's places (stations and the places
/// it passes; each with the spots it might stop at, likeliest first), leg by
/// leg, as stretches of track. At each place it takes the likeliest spot it
/// can both reach and go on from to the next place (a platform track that
/// ends there is no way through). Where the way between two places can't be
/// found there is a gap, never a line across country. It stops at the first
/// leg whose track isn't loaded yet (that and the next few are sent for):
/// then it is incomplete.
/// `heading`: the way it is going at the first place, when it is on its way
/// (it never turns back there): then a place next it can't reach going on
/// is one it has passed, and it goes on to the one after (up to
/// `MAX_PASSED` of them). Also how many it passed.
pub fn way_through(places: &[Vec<(f64, f64)>], heading: Option<f64>) -> (Vec<Line>, bool, usize) {
    let way = way(places, heading);
    (way.stretches, way.complete, way.passed)
}

/// `way_through`'s way, and the spot it takes at each place (none: passed, or not reached).
pub struct Way {
    pub stretches: Vec<Line>,
    pub complete: bool,
    pub passed: usize,
    /// By place (those with no spots left out).
    pub spots: Vec<Option<(f64, f64)>>,
}

/// `way_through`, with the spots it takes.
pub fn way(places: &[Vec<(f64, f64)>], heading: Option<f64>) -> Way {
    let places: Vec<&[(f64, f64)]> =
        places.iter().map(|p| &p[..p.len().min(MAX_PLATFORMS)]).filter(|p| !p.is_empty()).collect();
    let mut spots = vec![None; places.len()];
    let keys_between = |a: &[(f64, f64)], b: &[(f64, f64)]| {
        let mut points = a.to_vec();
        points.extend(b);
        tiles_for(&points, 0.03)
    };
    let mut stretches = Vec::new();
    let mut cur: Line = Vec::new();
    let mut complete = true;
    let mut passed = 0;
    let Some(first) = places.first() else { return Way { stretches, complete, passed, spots } };
    let mut at = first[0];
    spots[0] = Some(at);
    let mut i = 0;
    while i + 1 < places.len() {
        i += 1;
        // the first leg, from where it is
        let from_start = i == passed + 1;
        let keys = keys_between(&[at], places[i]);
        let found = if keys.len() > MAX_LEG_TILES {
            None
        } else {
            let Some(g) = graph_for(keys) else {
                complete = false;
                // the next legs' track is asked for meanwhile
                for j in i + 1..places.len().min(i + 4) {
                    let keys = keys_between(places[j - 1], places[j]);
                    if keys.len() <= MAX_LEG_TILES {
                        tracks_of(&keys);
                    }
                }
                break;
            };
            let mut first_reached = None;
            let mut through = None;
            for &spot in places[i] {
                // a train on its way never turns back to start with
                let r = match heading.filter(|_| from_start) {
                    Some(h) => route_in_onwards(&g, at, Some(h), &[spot], 0.0, 1.0).filter(|r| r.reached.len() == 1),
                    None => leg(&g, at, end_heading(&cur), spot),
                };
                let Some(r) = r else { continue };
                let Some(next) = places.get(i + 1) else {
                    through = Some((spot, r));
                    break;
                };
                // and on from there to the next place?
                let on_keys = keys_between(&[spot], next);
                let goes_on = on_keys.len() > MAX_LEG_TILES
                    || graph_for(on_keys)
                        .is_none_or(|g2| next.iter().any(|n| goes_on(&g2, spot, end_heading(&r.points), *n)));
                if goes_on {
                    through = Some((spot, r));
                    break;
                }
                first_reached.get_or_insert((spot, r));
            }
            through.or(first_reached)
        };
        if found.is_none() && from_start && heading.is_some() {
            if passed < MAX_PASSED && i + 1 < places.len() {
                passed += 1;
                continue;
            }
            // none it can reach: not passed, but no way there known (a gap)
            (i, passed, at) = (1, 0, places[1][0]);
            continue;
        }
        match found {
            Some((spot, r)) => {
                if cur.is_empty() {
                    cur = r.points;
                } else {
                    cur.extend(r.points.into_iter().skip(1));
                }
                at = spot;
                spots[i] = Some(spot);
            }
            None => {
                if cur.len() >= 2 {
                    stretches.push(std::mem::take(&mut cur));
                }
                cur.clear();
                at = places[i][0];
            }
        }
    }
    if cur.len() >= 2 {
        stretches.push(cur);
    }
    Way { stretches, complete, passed, spots }
}

/// The track, for the motion module, as a train of a class runs on it
/// (none: OpenRailwayMap's limits and Trafikverket's base ones).
pub struct Rail {
    pub class: Option<crate::train_class::TrainClass>,
}

impl crate::motion::Track for Rail {
    fn route(
        &self,
        start: (f64, f64),
        heading: Option<f64>,
        targets: &[(f64, f64)],
        onwards_m: f64,
        onwards_only: bool,
    ) -> Option<crate::motion::Routed> {
        route_for(start, heading, targets, onwards_m, self.class.as_ref(), onwards_only)
            .map(|r| (r.points, r.reached, r.speeds))
    }

    fn top(&self) -> Option<f64> {
        self.class.map(|c| c.top())
    }

    fn hold(&self, at: (f64, f64), platform: (f64, f64)) -> Option<(f64, f64)> {
        hold(at, platform)
    }
}

/// The track, with the way the train's route takes (`trafikverket::route_from`,
/// as its panel shows it): the motion goes along that way's tracks where it
/// leads (through points along it, `GUIDE_EVERY_M` apart), rather than
/// finding a way of its own between the stops (a parallel track, the other
/// side of a junction). Where the way can't be followed, as `Rail`.
pub struct Guided<'a> {
    pub rail: &'a Rail,
    /// From where the train is, as `(lat, lon)` points.
    pub way: &'a [(f64, f64)],
}

/// The points the motion is led through along a train's way are this far apart, metres.
const GUIDE_EVERY_M: f64 = 250.0;
/// ...as far as this on, metres (the motion's horizon, at most).
const GUIDE_FOR_M: f64 = 8_000.0;
/// A place this near the way is on it, metres.
const ON_WAY_M: f64 = 12.0;
/// A place this near where the way starts is led onto it, metres.
const SHORT_OF_WAY_M: f64 = 1_000.0;

/// How far along `line` the point nearest `p` is (from `after` on), if it is within `within`.
fn along_line(line: &[(f64, f64)], p: (f64, f64), within: f64, after: f64) -> Option<f64> {
    let mut s = 0.0;
    let mut best: Option<(f64, f64)> = None;
    for w in line.windows(2) {
        let len = distance(w[0], w[1]);
        let (q, t) = project(p, w[0], w[1]);
        let at = s + t * len;
        let off = distance(p, q);
        if at >= after - 0.5 && off <= within && best.is_none_or(|b| off < b.1 - 0.5) {
            best = Some((at, off));
        }
        s += len;
    }
    best.map(|b| b.0)
}

/// The point `s` metres along `line`.
fn point_along(line: &[(f64, f64)], s: f64) -> (f64, f64) {
    let mut left = s;
    for w in line.windows(2) {
        let len = distance(w[0], w[1]);
        if left <= len {
            let t = if len > 0.0 { left / len } else { 0.0 };
            return (w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t);
        }
        left -= len;
    }
    *line.last().unwrap_or(&(0.0, 0.0))
}

impl crate::motion::Track for Guided<'_> {
    fn route(
        &self,
        start: (f64, f64),
        heading: Option<f64>,
        targets: &[(f64, f64)],
        onwards_m: f64,
        onwards_only: bool,
    ) -> Option<crate::motion::Routed> {
        let plain = || self.rail.route(start, heading, targets, onwards_m, onwards_only);
        // where it is along the way: on it, or short of where it starts (the report behind the drawn train)
        let short = self.way.first().is_some_and(|w| distance(start, *w) <= SHORT_OF_WAY_M);
        let Some(s0) = along_line(self.way, start, ON_WAY_M, 0.0).or(short.then_some(0.0)) else { return plain() };
        let length: f64 = self.way.windows(2).map(|w| distance(w[0], w[1])).sum();
        // the targets on the way, in order, and where (those after the first off it: as they are)
        let mut on_way = Vec::new();
        let mut after = s0;
        for t in targets {
            let Some(s) = along_line(self.way, *t, ON_WAY_M, after) else { break };
            on_way.push(s);
            after = s;
        }
        // each stop on it where the way passes it (its platform, as the way takes it)
        let target = |i: usize| on_way.get(i).map_or(targets[i], |s| point_along(self.way, *s));
        // through the way's points between them, and on along it
        let end = length.min(s0 + GUIDE_FOR_M);
        let mut through: Vec<((f64, f64), Option<usize>)> = Vec::new();
        let mut s = s0 + GUIDE_EVERY_M;
        let mut next = 0;
        while s < end {
            while next < on_way.len() && on_way[next] <= s {
                through.push((target(next), Some(next)));
                next += 1;
            }
            if on_way.iter().all(|t| (t - s).abs() > GUIDE_EVERY_M / 4.0) {
                through.push((point_along(self.way, s), None));
            }
            s += GUIDE_EVERY_M;
        }
        through.extend((next..targets.len()).map(|i| (target(i), Some(i))));
        if through.iter().all(|t| t.1.is_some()) {
            return plain();
        }
        let points: Vec<(f64, f64)> = through.iter().map(|t| t.0).collect();
        let (pts, reached, speeds) = self.rail.route(start, heading, &points, onwards_m, onwards_only)?;
        let of_targets: Vec<f64> = reached.iter().zip(&through).filter(|(_, t)| t.1.is_some()).map(|(r, _)| *r).collect();
        // led astray (the way not followed to the end): the way of its own, if it reaches more of the stops
        if reached.len() < through.len()
            && let Some(own) = plain()
            && own.1.len() > of_targets.len()
        {
            return Some(own);
        }
        Some((pts, of_targets, speeds))
    }

    fn top(&self) -> Option<f64> {
        self.rail.top()
    }

    fn hold(&self, at: (f64, f64), platform: (f64, f64)) -> Option<(f64, f64)> {
        self.rail.hold(at, platform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(points: Line) -> Track {
        Track { points, ..Default::default() }
    }

    fn platform_in(g: &Graph, station: (f64, f64), track: Option<&str>) -> Option<(f64, f64)> {
        platforms_in(g, station, track, None, &[]).into_iter().next()
    }

    fn graph(tracks: Vec<Track>) -> Graph {
        Graph::build(&[Arc::new(tracks)])
    }

    /// A line of points every 100 m from `from` on a course.
    fn line(from: (f64, f64), course: f64, km: f64) -> Line {
        (0..=(km * 10.0) as usize).map(|i| ahead(from, course, i as f64 * 100.0)).collect()
    }

    #[test]
    fn it_follows_the_bend_rather_than_going_straight() {
        // east for 2 km, then north for 3 km
        let corner = ahead((57.0, 12.0), 90.0, 2_000.0);
        let mut l = line((57.0, 12.0), 90.0, 2.0);
        l.extend(line(corner, 0.0, 3.0).into_iter().skip(1));
        let g = graph(vec![track(l)]);
        let start = ahead((57.0, 12.0), 90.0, 500.0);
        let r = route_in(&g, (start.0 + 0.0002, start.1), Some(90.0), &[], 3_000.0, SNAP_SPREAD_M).unwrap();
        let end = *r.points.last().unwrap();
        assert!(end.0 > corner.0 + 0.005, "went round the corner northwards: {end:?}");
        assert!((end.1 - corner.1).abs() < 0.001, "stayed on the northbound track");
    }

    #[test]
    fn it_finds_the_way_to_a_station_through_a_junction() {
        // a junction: one branch north-east, one south-east; the station is on the southern one
        let junction = ahead((57.0, 12.0), 90.0, 1_000.0);
        let station = ahead(junction, 135.0, 3_000.0);
        let g = graph(vec![
            track(line((57.0, 12.0), 90.0, 1.0)),
            track(line(junction, 45.0, 4.0)),
            track(line(junction, 135.0, 4.0)),
        ]);
        let r = route_in(&g, (57.0, 12.0005), Some(90.0), &[station], 500.0, SNAP_SPREAD_M).unwrap();
        assert_eq!(r.reached.len(), 1);
        assert!((r.reached[0] - 4_000.0).abs() < 150.0, "about 4 km along: {}", r.reached[0]);
        assert!(r.points.iter().all(|p| p.0 <= 57.0 + 1e-4), "never on the northern branch");
    }

    #[test]
    fn it_never_turns_back_at_a_switch() {
        // a branch leaving the main line eastwards at 20°: a train on the main
        // line heading west can't take it without reversing
        let switch = ahead((57.0, 12.0), 90.0, 1_000.0);
        let branch_end = ahead(switch, 70.0, 2_000.0);
        let g = graph(vec![track(line((57.0, 12.0), 90.0, 3.0)), track(line(switch, 70.0, 2.0))]);
        let start = ahead((57.0, 12.0), 90.0, 1_500.0);
        let r = route_in(&g, start, Some(270.0), &[branch_end], 500.0, SNAP_SPREAD_M).unwrap();
        assert!(r.reached.is_empty(), "no way there going west");
    }

    #[test]
    fn parallel_tracks_stay_apart_and_seams_join() {
        // two tracks 4.5 m apart; the northern one cut at a tile edge, its pieces 1 m apart
        let edge_lon = (2198.0 / f64::from(1u32 << ZOOM)) * 360.0 - 180.0;
        let a = (55.70, edge_lon - 0.02);
        let south = line(a, 90.0, 2.5);
        let north_start = ahead(a, 0.0, 4.5);
        let north: Line = line(north_start, 90.0, 2.5);
        let west: Line = north.iter().copied().filter(|p| p.1 < edge_lon).chain([(north_start.0, edge_lon)]).collect();
        let east: Line = [(north_start.0 + 0.000009, edge_lon)]
            .into_iter()
            .chain(north.iter().copied().filter(|p| p.1 > edge_lon))
            .collect();
        let g = graph(vec![track(south), track(west), track(east)]);
        let end_north = *north.last().unwrap();
        let r = route_in(&g, ahead(north_start, 90.0, 50.0), Some(90.0), &[end_north], 0.0, 0.5).unwrap();
        assert_eq!(r.reached.len(), 1, "across the seam");
        assert!(r.points.iter().all(|p| p.0 > a.0 + 0.00002), "never on the southern track");
    }

    #[test]
    fn bridges_and_tunnels_along_a_way() {
        // east: 1 km on the ground, a bridge in two pieces (cut by a tile's edge), 1 km on; a tunnel on the track 4.5 m north
        let a = (59.0, 18.0);
        let at = |m: f64| ahead(a, 90.0, m);
        let beside = |m: f64| ahead(at(m), 0.0, 4.5);
        let tracks = vec![
            track(vec![at(0.0), at(1_000.0)]),
            Track { points: vec![at(1_000.0), at(1_060.0)], bridge: true, ..Default::default() },
            Track { points: vec![at(1_060.0), at(1_100.0)], bridge: true, ..Default::default() },
            track(vec![at(1_100.0), at(2_100.0)]),
            Track { points: vec![beside(1_500.0), beside(1_900.0)], tunnel: true, ..Default::default() },
        ];
        let sets = [Arc::new(tracks)];
        // a train's way: frames every 90 m, none on the bridge itself
        let way: Vec<(f64, f64)> = (0..12).map(|i| at(500.0 + 90.0 * i as f64 + 15.0)).collect();
        let found = structures_near(&sets, &way, 4.0);
        assert_eq!(found.len(), 2, "both pieces of the bridge, not the tunnel beside: {found:?}");
        assert!(found.iter().all(|s| !s.tunnel));
        let first = found.iter().find(|s| distance((s.line[0][1], s.line[0][0]), at(1_000.0)) < 1.0).unwrap();
        assert_eq!(first.joined, [false, true], "goes on as a bridge at its end only");
        assert_eq!(structures_near(&sets, &[at(0.0), at(400.0)], 4.0), vec![], "nothing on the way");
    }

    #[test]
    fn trains_stop_at_their_announced_track() {
        let station = (57.0, 12.0);
        let t1 = Track {
            points: line(ahead(station, 0.0, 20.0), 90.0, 0.4),
            track_ref: Some("1".into()),
            ..Default::default()
        };
        let t2 = Track {
            points: line(ahead(station, 0.0, 24.5), 90.0, 0.4),
            track_ref: Some("2".into()),
            service: true,
            ..Default::default()
        };
        let g = graph(vec![t1, t2]);
        let p2 = platform_in(&g, station, Some("2b")).unwrap();
        assert!((distance(station, p2) - 24.5).abs() < 0.5, "on track 2: {}", distance(station, p2));
        let p = platform_in(&g, station, Some("9")).unwrap();
        assert!((distance(station, p) - 20.0).abs() < 0.5, "an unknown track: the nearest running line");
        // a train reported between the tracks, standing at track 2, is put on track 2
        let reported = ahead(ahead(station, 0.0, 21.0), 90.0, 150.0);
        let held = hold_in(&g, reported, p2).unwrap();
        let on_track_2 = ahead(ahead(station, 0.0, 24.5), 90.0, 150.0);
        assert!(distance(held, on_track_2) < 1.0, "{} m off", distance(held, on_track_2));
    }

    #[test]
    fn routes_carry_the_speed_limits() {
        let a = Track { points: line((57.0, 12.0), 90.0, 1.0), maxspeed: Some(160.0), ..Default::default() };
        let b = Track {
            points: line(ahead((57.0, 12.0), 90.0, 1_000.0), 90.0, 1.0),
            maxspeed: Some(80.0),
            ..Default::default()
        };
        let g = graph(vec![a, b]);
        let r = route_in(&g, (57.0, 12.0), Some(90.0), &[], 1_800.0, SNAP_SPREAD_M).unwrap();
        assert_eq!(r.speeds.len(), r.points.len() - 1);
        assert!((r.speeds[0].unwrap() - 160.0 / 3.6).abs() < 0.01);
        assert!((r.speeds.last().unwrap().unwrap() - 80.0 / 3.6).abs() < 0.01);
    }

    #[test]
    fn a_run_is_drawn_along_the_track_with_gaps_never_straight_lines() {
        // track from A east to B; C is 3 km north of B, off any track
        let a = (61.0, 15.0);
        let b = ahead(a, 90.0, 3_000.0);
        let c = ahead(b, 0.0, 3_000.0);
        let mut l = line(a, 90.0, 3.0);
        *l.last_mut().unwrap() = b;
        let places = [vec![a], vec![b], vec![c]];
        {
            let mut st = store().lock().unwrap();
            for key in tiles_for(&[a, b, c], 0.05) {
                st.tracks.insert(key, (Instant::now(), Arc::new(Vec::new())));
            }
            st.tracks.insert(tile_of(a.0, a.1), (Instant::now(), Arc::new(vec![track(l)])));
        }
        let (stretches, complete, _) = way_through(&places, None);
        assert!(complete);
        assert_eq!(stretches.len(), 1, "A to B only: {stretches:?}");
        assert!(stretches[0].iter().all(|p| (p.0 - a.0).abs() < 1e-4), "along the track, not off to C");
        assert!(distance(*stretches[0].last().unwrap(), b) < 1.0);
    }

    #[test]
    fn the_way_through_takes_the_platform_it_can_go_on_from() {
        // the line east 3.6 km (in one tile, as its graph is made of the tiles round each leg); at
        // 1.5 km a bay track leaves it, 10 m north of it, ending at 2.1 km
        let n = f64::from(1u32 << ZOOM);
        let a = (62.5, ((16.5_f64 + 180.0) / 360.0 * n).floor() / n * 360.0 - 180.0 + 0.0005);
        let along = |m: f64| ahead(a, 90.0, m);
        assert_eq!(tile_of(a.0, a.1), tile_of(along(3_600.0).0, along(3_600.0).1));
        let mut bay = vec![along(1_500.0)];
        bay.extend((16..=21).map(|i| ahead(along(i as f64 * 100.0), 0.0, 10.0)));
        {
            let mut st = store().lock().unwrap();
            for key in tiles_for(&[a, along(3_600.0)], 0.05) {
                st.tracks.insert(key, (Instant::now(), Arc::new(Vec::new())));
            }
            st.tracks.insert(tile_of(a.0, a.1), (Instant::now(), Arc::new(vec![track(line(a, 90.0, 3.6)), track(bay)])));
        }
        // the station at 1.9 km: the bay's spot the likelier, then the line's
        let (bay_spot, line_spot) = (ahead(along(1_900.0), 0.0, 10.0), along(1_900.0));
        let through = way(&[vec![along(300.0)], vec![bay_spot, line_spot], vec![along(3_300.0)]], None);
        assert!(through.complete);
        assert_eq!(through.spots[1], Some(line_spot), "not the bay, which it would have to back out of");
        assert_eq!(through.spots[2], Some(along(3_300.0)));
        // the last place: the likeliest it reaches
        let ending = way(&[vec![along(300.0)], vec![bay_spot, line_spot]], None);
        assert_eq!(ending.spots[1], Some(bay_spot), "a train ending there may stop in the bay");
    }

    #[test]
    fn the_motion_follows_the_way_its_route_takes() {
        use crate::motion::Track as _;
        // two tracks east 3 km, 10 m apart, joined by crossovers at each end (in one tile)
        let n = f64::from(1u32 << ZOOM);
        let a = (61.5, ((15.5_f64 + 180.0) / 360.0 * n).floor() / n * 360.0 - 180.0 + 0.0005);
        let south = |m: f64| ahead(a, 90.0, m);
        let north = |m: f64| ahead(ahead(a, 0.0, 10.0), 90.0, m);
        {
            let mut st = store().lock().unwrap();
            for key in tiles_for(&[a, south(3_000.0)], 0.05) {
                st.tracks.insert(key, (Instant::now(), Arc::new(Vec::new())));
            }
            st.tracks.insert(
                tile_of(a.0, a.1),
                (
                    Instant::now(),
                    Arc::new(vec![
                        track(line(a, 90.0, 3.0)),
                        track(line(ahead(a, 0.0, 10.0), 90.0, 3.0)),
                        track(vec![south(200.0), north(300.0)]),
                        track(vec![north(2_700.0), south(2_800.0)]),
                    ]),
                ),
            );
        }
        let rail = Rail { class: None };
        let (start, target) = (south(100.0), south(2_900.0));
        let on_north = |points: &[(f64, f64)]| points.iter().any(|p| distance(*p, north(1_500.0)) < 2.0);
        let (own, ..) = rail.route(start, Some(90.0), &[target], 0.0, false).unwrap();
        assert!(!on_north(&own), "on its own it keeps to the track it is on");
        // its route crosses over and back
        let mut way = vec![start, south(200.0)];
        way.extend((3..=27).map(|i| north(i as f64 * 100.0)));
        way.extend([south(2_800.0), target]);
        let (led, reached, _) = Guided { rail: &rail, way: &way }.route(start, Some(90.0), &[target], 0.0, false).unwrap();
        assert!(on_north(&led), "along the way its route takes");
        assert_eq!(reached.len(), 1, "and still to its stop");
        assert!(distance(*led.last().unwrap(), target) < 1.0);
    }

    #[test]
    fn a_train_on_its_way_goes_on_from_where_it_is_not_from_the_station_it_left() {
        // track east 4 km; the train 800 m along, going east, 300 m past the station it left
        let a = (62.0, 16.0);
        let train = ahead(a, 90.0, 800.0);
        let (left, next) = (ahead(a, 90.0, 500.0), ahead(a, 90.0, 3_000.0));
        {
            let mut st = store().lock().unwrap();
            for key in tiles_for(&[a, ahead(a, 90.0, 4_000.0)], 0.05) {
                st.tracks.insert(key, (Instant::now(), Arc::new(Vec::new())));
            }
            st.tracks.insert(tile_of(a.0, a.1), (Instant::now(), Arc::new(vec![track(line(a, 90.0, 4.0))])));
        }
        let (stretches, complete, passed) = way_through(&[vec![train], vec![left], vec![next]], Some(90.0));
        assert!(complete);
        assert_eq!(passed, 1, "the station it left is behind it");
        assert_eq!(stretches.len(), 1);
        assert!(distance(stretches[0][0], train) < 1.0, "from the train: {:?}", stretches[0][0]);
        assert!(stretches[0].iter().all(|p| p.1 >= train.1 - 1e-6), "never back past the train");
        assert!(distance(*stretches[0].last().unwrap(), next) < 1.0);
    }

    #[test]
    fn an_unnumbered_track_is_found_beside_the_platform_mapped_as_serving_it() {
        use crate::osm_platforms::Platform;
        // three tracks east-west, 5 m apart (none numbered); the station's point on the southern one
        let a = (60.0, 15.0);
        let (south, middle, north) = (a, ahead(a, 0.0, 5.0), ahead(a, 0.0, 10.0));
        let g =
            graph(vec![track(line(south, 90.0, 1.0)), track(line(middle, 90.0, 1.0)), track(line(north, 90.0, 1.0))]);
        let station = ahead(south, 90.0, 500.0);
        let along = |p: (f64, f64), m: f64| ahead(p, 90.0, m);
        let lat_of = |p: (f64, f64)| p.0;
        // platform "3" north of the northern track; island "1;2" between the southern and middle ones
        let side = Platform {
            tracks: vec!["3".into()],
            line: vec![along(ahead(north, 0.0, 4.0), 400.0), along(ahead(north, 0.0, 4.0), 600.0)],
        };
        let island = Platform {
            tracks: vec!["1".into(), "2".into()],
            line: vec![along(ahead(south, 0.0, 2.5), 400.0), along(ahead(south, 0.0, 2.5), 600.0)],
        };
        let mapped = [side, island];
        let on = |p: (f64, f64), t: (f64, f64)| (lat_of(p) - lat_of(t)).abs() < 1e-6;
        let p3 = platforms_in(&g, station, Some("3"), None, &mapped)[0];
        assert!(on(p3, north), "track 3: beside its platform, not the nearest to the station");
        assert!(distance(p3, along(north, 500.0)) < 1.0, "at the platform's middle");
        // the island's two tracks both offered
        let p1 = platforms_in(&g, station, Some("1"), None, &mapped);
        assert!(on(p1[0], south) || on(p1[0], middle));
        assert!(p1.iter().take(2).any(|p| on(*p, south)) && p1.iter().take(2).any(|p| on(*p, middle)));
        // with no platform mapped, as before: the nearest
        assert!(on(platforms_in(&g, station, Some("3"), None, &[])[0], south));
    }

    #[test]
    fn the_track_behind_a_train_follows_its_own_line() {
        // a line east 3 km, with a parallel one 4.5 m north; behind a train going east at 2 km: west along its own
        let a = (63.5, 17.0);
        let north = ahead(a, 0.0, 4.5);
        {
            let mut st = store().lock().unwrap();
            for key in tiles_for(&[a, ahead(a, 90.0, 3_000.0)], 0.05) {
                st.tracks.insert(key, (Instant::now(), Arc::new(Vec::new())));
            }
            st.tracks.insert(tile_of(a.0, a.1), (Instant::now(), Arc::new(vec![track(line(a, 90.0, 3.0)), track(line(north, 90.0, 3.0))])));
        }
        let at = ahead(a, 90.0, 2_000.0);
        let back = track_from(at, 270.0, 100.0).unwrap();
        assert!(distance(back[0], at) < 1.0);
        let end = *back.last().unwrap();
        assert!((distance(end, ahead(a, 90.0, 1_900.0))) < 1.0, "100 m back along it: {end:?}");
        assert!(back.iter().all(|p| distance(*p, ahead(a, 90.0, distance(a, *p))) < 2.0), "on its own track, not the one beside it");
    }

    #[test]
    fn a_train_is_put_on_the_track_running_its_way() {
        // double track 4.5 m apart: the northern track runs east, the southern west
        let a = (59.0, 18.0);
        let north = ahead(a, 0.0, 4.5);
        let mut g = graph(vec![track(line(north, 90.0, 2.0)), track(line(a, 90.0, 2.0))]);
        for n in 0..g.nodes.len() {
            for i in 0..g.adj[n].len() {
                let (from, to) = (g.nodes[n], g.nodes[g.adj[n][i].to]);
                let east = to.1 > from.1;
                let on_north = from.0 > a.0 + 0.00002;
                g.adj[n][i].flow = Some(east == on_north);
            }
        }
        // reported right between them
        let between = ahead(ahead(a, 0.0, 2.25), 90.0, 500.0);
        let east = route_in(&g, between, Some(90.0), &[], 800.0, SNAP_SPREAD_M).unwrap();
        let west = route_in(&g, between, Some(270.0), &[], 400.0, SNAP_SPREAD_M).unwrap();
        assert!(east.points.iter().all(|p| p.0 > a.0 + 0.00002), "eastbound on the northern track");
        assert!(west.points.iter().all(|p| p.0 < a.0 + 0.00002), "westbound on the southern track");
    }

    #[test]
    fn the_way_trains_run_spreads_along_a_stretch_of_track() {
        // known for the first 300 m of a 2 km stretch, going east
        let a = (59.0, 18.0);
        let mut g = graph(vec![track(line(a, 90.0, 2.0))]);
        for n in 0..g.nodes.len() {
            for i in 0..g.adj[n].len() {
                let (from, to) = (g.nodes[n], g.nodes[g.adj[n][i].to]);
                g.adj[n][i].flow = (from.1.max(to.1) <= ahead(a, 90.0, 300.0).1 + 1e-9).then_some(to.1 > from.1);
            }
        }
        g.spread_flow();
        for n in 0..g.nodes.len() {
            for e in &g.adj[n] {
                assert_eq!(e.flow, Some(g.nodes[e.to].1 > g.nodes[n].1), "all of it, both ways");
            }
        }
        // said both ways on the same stretch: left as it is
        let mut g = graph(vec![track(line(a, 90.0, 2.0))]);
        let last = g.nodes.len() - 1;
        g.adj[0][0].flow = Some(true);
        g.adj[last][0].flow = Some(true);
        g.spread_flow();
        assert!(g.adj.iter().flatten().filter(|e| e.flow.is_some()).count() == 2);
    }

    #[test]
    fn a_station_offers_the_track_running_the_trains_way_first() {
        // double track 4.5 m apart, the northern running east; the station's point by the southern one
        let a = (59.0, 18.0);
        let north = ahead(a, 0.0, 4.5);
        let mut g = graph(vec![track(line(north, 90.0, 2.0)), track(line(a, 90.0, 2.0))]);
        for n in 0..g.nodes.len() {
            for i in 0..g.adj[n].len() {
                let (from, to) = (g.nodes[n], g.nodes[g.adj[n][i].to]);
                g.adj[n][i].flow = Some((to.1 > from.1) == (from.0 > a.0 + 0.00002));
            }
        }
        let station = ahead(ahead(a, 90.0, 1_000.0), 180.0, 20.0);
        let on_north = |p: (f64, f64)| p.0 > a.0 + 0.00002;
        assert!(on_north(platforms_in(&g, station, None, Some(90.0), &[])[0]), "eastbound: the northern track");
        assert!(!on_north(platforms_in(&g, station, None, Some(270.0), &[])[0]), "westbound: the southern track");
        assert!(!on_north(platforms_in(&g, station, None, None, &[])[0]), "not saying: the nearest");
    }

    #[test]
    fn it_heads_for_its_next_station_whatever_its_heading_says() {
        // a line east-west; the station is 3 km west, but the heading says east
        let g = graph(vec![track(line((57.0, 12.0), 90.0, 8.0))]);
        let here = ahead((57.0, 12.0), 90.0, 4_000.0);
        let station = ahead((57.0, 12.0), 90.0, 1_000.0);
        let r = route_in(&g, here, Some(90.0), &[station], 500.0, SNAP_SPREAD_M).unwrap();
        assert_eq!(r.reached.len(), 1, "reaches the station");
        assert!((r.reached[0] - 3_000.0).abs() < 50.0, "straight there: {}", r.reached[0]);
        assert!(r.points.iter().all(|p| p.1 <= here.1 + 1e-9), "never east of where it is");
        // with nowhere it must go, the heading is followed
        let r = route_in(&g, here, Some(90.0), &[], 500.0, SNAP_SPREAD_M).unwrap();
        assert!(r.points.last().unwrap().1 > here.1, "east, as it points");
    }

    #[test]
    fn lines_are_clipped_to_the_tile() {
        let pieces = clip(&[(-64.0, 100.0), (100.0, 100.0), (5000.0, 100.0), (5000.0, 200.0), (100.0, 200.0)], 4096.0);
        assert_eq!(
            pieces,
            vec![vec![(0.0, 100.0), (100.0, 100.0), (4096.0, 100.0)], vec![(4096.0, 200.0), (100.0, 200.0)]]
        );
    }

    #[test]
    fn tiles_and_tags_decode() {
        assert_eq!(tile_of(55.70, 13.19), (2198, 1281));
        fn bytes(field: u8, b: &[u8]) -> Vec<u8> {
            let mut v = vec![(field << 3) | 2, b.len() as u8];
            v.extend(b);
            v
        }
        // layer "railway_line_high", extent 4096, a line from (0,0) to (4096,4096)
        // tagged feature=rail, maxspeed=160 (a double), track_ref="3", tunnel=true
        let geometry = [9u8, 0, 0, 10, 0x80, 0x40, 0x80, 0x40];
        let mut feature = bytes(2, &[0, 0, 1, 1, 2, 2, 3, 3]);
        feature.extend([0x18, 2]);
        feature.extend(bytes(4, &geometry));
        let mut layer = bytes(1, b"railway_line_high");
        layer.extend(bytes(2, &feature));
        for k in ["feature", "maxspeed", "track_ref", "tunnel"] {
            layer.extend(bytes(3, k.as_bytes()));
        }
        layer.extend(bytes(4, &bytes(1, b"rail")));
        let mut double = vec![0x19];
        double.extend(160.0f64.to_le_bytes());
        layer.extend(bytes(4, &double));
        layer.extend(bytes(4, &bytes(1, b"3")));
        layer.extend(bytes(4, &[0x38, 1]));
        layer.extend([0x28, 0x80, 0x20]);
        let tile = bytes(3, &layer);
        let tracks = decode(&tile, ZOOM, 2198, 1281);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].maxspeed, Some(160.0));
        assert_eq!(tracks[0].track_ref.as_deref(), Some("3"));
        assert!(tracks[0].tunnel && !tracks[0].bridge);
        let (nw, se) = (tracks[0].points[0], tracks[0].points[1]);
        assert_eq!(tile_of(nw.0 - 1e-9, nw.1 + 1e-9), (2198, 1281));
        assert!(se.0 < nw.0 && se.1 > nw.1, "south-east of the corner");
    }

    /// A real tile (Lund, `RAIL_TILE` naming the file): tracks with speeds
    /// and numbers, and the station's platforms.
    #[test]
    #[ignore]
    fn a_real_tile() {
        let bytes = std::fs::read(std::env::var("RAIL_TILE").unwrap()).unwrap();
        let tracks = decode(&bytes, ZOOM, 2198, 1281);
        assert!(tracks.len() > 30, "{}", tracks.len());
        assert!(tracks.iter().any(|t| t.maxspeed.is_some()));
        let g = Graph::build(&[Arc::new(tracks)]);
        let lund = (55.7056, 13.1868);
        // each of its tracks has its own place, a few metres apart
        let platforms: Vec<(f64, f64)> =
            ["1", "2", "3", "4", "5", "6"].iter().map(|t| platform_in(&g, lund, Some(t)).unwrap()).collect();
        assert!(platforms.windows(2).all(|w| distance(w[0], w[1]) > 3.0), "{platforms:?}");
    }

    /// Real tiles from Lund to Malmö (`RAIL_TILES` naming a folder of
    /// `{x}_{y}.pbf`): the way between the two stations, across tile seams.
    #[test]
    #[ignore]
    fn lund_to_malmo() {
        let dir = std::env::var("RAIL_TILES").unwrap();
        let sets: Vec<Arc<Vec<Track>>> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| {
                let name = e.ok()?.file_name().into_string().ok()?;
                let (x, y) = name.strip_suffix(".pbf")?.split_once('_')?;
                let bytes = std::fs::read(format!("{dir}/{name}")).ok()?;
                Some(Arc::new(decode(&bytes, ZOOM, x.parse().ok()?, y.parse().ok()?)))
            })
            .collect();
        let g = Graph::build(&sets);
        let lund = platform_in(&g, (55.7056, 13.1868), Some("2")).unwrap();
        let malmo = platform_in(&g, (55.6094, 13.0006), Some("3")).unwrap();
        eprintln!("{} nodes, Malmö C track 3 at {malmo:?}", g.nodes.len());
        let r = route_in(&g, lund, None, &[malmo], 0.0, 1.0).unwrap();
        let known = r.speeds.iter().filter(|s| s.is_some()).count();
        eprintln!(
            "reached {:?}, {} points, {known} with a limit, top {:?} km/h",
            r.reached,
            r.points.len(),
            r.speeds.iter().flatten().fold(0.0f64, |a, b| a.max(*b * 3.6))
        );
        assert_eq!(r.reached.len(), 1);
        assert!((15_800.0..22_000.0).contains(&r.reached[0]), "{} m", r.reached[0]);
        // on the track running its way (Trafikverket's traffic flow), not the one beside it
        let (mut with, mut against) = (0.0, 0.0);
        for w in r.points.windows(2) {
            let mid = ((w[0].0 + w[1].0) / 2.0, (w[0].1 + w[1].1) / 2.0);
            let Some((a, b, _, _)) = g.snap(mid, 0.5) else { continue };
            let forward = turn(course(g.nodes[a], g.nodes[b]), course(w[0], w[1])) < 90.0;
            let e = if forward { g.edge(a, b) } else { g.edge(b, a) };
            match e.and_then(|e| e.flow) {
                Some(true) => with += distance(w[0], w[1]),
                Some(false) => against += distance(w[0], w[1]),
                None => {}
            }
        }
        eprintln!("with the traffic flow {with:.0} m, against it {against:.0} m");
        assert!(against < 0.05 * (with + against).max(1.0), "{against:.0} m against the flow");
    }

    /// Lund to Malmö C (announced at track 2b, which isn't numbered in the
    /// map: the City Tunnel's platforms) and on to Triangeln, on real tiles
    /// (`RAIL_TILES`): through the tunnel, not into the yard's track 2a.
    #[test]
    #[ignore]
    fn through_malmo_city_tunnel() {
        let dir = std::env::var("RAIL_TILES").unwrap();
        let mut sets = Vec::new();
        {
            let mut st = store().lock().unwrap();
            for e in std::fs::read_dir(&dir).unwrap() {
                let name = e.unwrap().file_name().into_string().unwrap();
                let Some((x, y)) = name.strip_suffix(".pbf").and_then(|n| n.split_once('_')) else { continue };
                let (x, y) = (x.parse().unwrap(), y.parse().unwrap());
                let tracks = Arc::new(decode(&std::fs::read(format!("{dir}/{name}")).unwrap(), ZOOM, x, y));
                sets.push(tracks.clone());
                st.tracks.insert((x, y), (Instant::now(), tracks));
            }
        }
        let g = Graph::build(&sets);
        let (lund, malmo, triangeln) = ((55.7056, 13.1868), (55.6094, 13.0006), (55.5937, 13.0003));
        let spots = |p, t| platforms_in(&g, p, Some(t), None, &[]);
        let malmo_spots = spots(malmo, "2b");
        let yard_2a = (55.6105, 13.0074);
        assert!(malmo_spots.iter().all(|p| distance(*p, yard_2a) > 150.0), "never the yard: {malmo_spots:?}");
        let (stretches, complete, _) = way_through(&[spots(lund, "2"), malmo_spots, spots(triangeln, "1")], None);
        assert!(complete);
        eprintln!("{} stretches, {:?} points", stretches.len(), stretches.iter().map(|s| s.len()).collect::<Vec<_>>());
        assert_eq!(stretches.len(), 1, "no gap");
        let end = *stretches[0].last().unwrap();
        assert!(distance(end, triangeln) < 300.0, "at Triangeln: {end:?}");
        // through the City Tunnel, west of the station, and never into the yard
        let tunnel = (55.6082, 12.9947);
        assert!(stretches[0].iter().any(|p| distance(*p, tunnel) < 30.0), "through the tunnel");
        let on_yard: Vec<(f64, f64)> = stretches[0]
            .windows(2)
            .map(|w| ((w[0].0 + w[1].0) / 2.0, (w[0].1 + w[1].1) / 2.0))
            .filter(|m| g.snap(*m, 0.5).is_some_and(|s| g.edge(s.0, s.1).is_some_and(|e| e.yard)))
            .collect();
        assert!(on_yard.is_empty(), "not on yard tracks: {on_yard:?}");
    }

    /// The track of these graphs, for the motion module, as a train of a class runs on it.
    struct OnGraph<'a> {
        g: &'a Graph,
        class: Option<crate::train_class::TrainClass>,
    }

    impl crate::motion::Track for OnGraph<'_> {
        fn route(
            &self,
            start: (f64, f64),
            heading: Option<f64>,
            targets: &[(f64, f64)],
            onwards_m: f64,
            onwards_only: bool,
        ) -> Option<crate::motion::Routed> {
            route_in_dir(self.g, start, heading, targets, onwards_m, SNAP_SPREAD_M, self.class.as_ref(), onwards_only)
                .map(|r| (r.points, r.reached, r.speeds))
        }

        fn hold(&self, at: (f64, f64), platform: (f64, f64)) -> Option<(f64, f64)> {
            hold_in(self.g, at, platform)
        }

        fn top(&self) -> Option<f64> {
            self.class.map(|c| c.top())
        }
    }

    #[test]
    fn a_train_whose_next_station_is_on_track_not_loaded_yet_still_moves() {
        use crate::motion::{self, Fix, Stop};
        // only the track round it is here: its station, 25 km on, is beyond it
        let a = (64.0, 18.0);
        let (station, start) = (ahead(a, 90.0, 25_000.0), ahead(a, 90.0, 1_000.0));
        {
            let mut st = store().lock().unwrap();
            for key in tiles_for(&[a, ahead(a, 90.0, 8_000.0)], 0.05) {
                st.tracks.insert(key, (Instant::now(), Arc::new(Vec::new())));
            }
            st.tracks.insert(tile_of(a.0, a.1), (Instant::now(), Arc::new(vec![track(line(a, 90.0, 8.0))])));
            assert!(!st.tracks.contains_key(&tile_of(station.0, station.1)));
        }
        const T0: i64 = 1_800_000_000_000;
        let report = Fix { lat: start.0, lon: start.1, t_ms: T0, speed: Some(40.0), heading: Some(90.0) };
        let stops =
            [Stop { lat: station.0, lon: station.1, arrive_ms: Some(T0 + 900_000), depart_ms: None, pass: false }];
        let sim = motion::simulate(None, &report, None, &stops, T0 + 5_000, Some(&Rail { class: None }));
        let (first, last) = (sim.frames.first().unwrap(), sim.frames.last().unwrap());
        let gone = distance((first.0, first.1), (last.0, last.1));
        assert!(gone > 1_000.0, "it goes on along the track it has: {gone:.0} m");
    }

    #[test]
    fn a_train_takes_the_branch_its_next_station_is_on_however_far_it_is() {
        use crate::motion::{self, Fix, Stop};
        // north 3 km to a junction: straight on north, or off north-east to its station 16 km on
        let a = (63.0, 17.0);
        let j = ahead(a, 0.0, 3_000.0);
        let mut trunk = line(a, 0.0, 3.0);
        *trunk.last_mut().unwrap() = j;
        let (mut straight, mut branch) = (line(j, 0.0, 20.0), line(j, 30.0, 20.0));
        (straight[0], branch[0]) = (j, j);
        let g = graph(vec![track(trunk), track(straight), track(branch)]);
        let track = OnGraph { g: &g, class: None };
        let station = ahead(j, 30.0, 16_000.0);
        const T0: i64 = 1_800_000_000_000;
        let at = ahead(a, 0.0, 1_000.0);
        let report = Fix { lat: at.0, lon: at.1, t_ms: T0, speed: Some(30.0), heading: Some(0.0) };
        let stops = [Stop {
            lat: station.0,
            lon: station.1,
            arrive_ms: Some(T0 + 600_000),
            depart_ms: Some(T0 + 660_000),
            pass: false,
        }];
        let sim = motion::simulate(None, &report, None, &stops, T0 + 5_000, Some(&track));
        // two minutes on it is well past the junction: on the branch, not straight on
        let end = sim.predicted.last().unwrap();
        let (end, j_on) = ((end.0, end.1), distance(j, (end.0, end.1)));
        assert!(j_on > 1_000.0, "past the junction: {j_on:.0} m");
        assert!(turn(course(j, end), 30.0) < 5.0, "on the branch to its station: {:.0}°", course(j, end));
    }

    /// An Öresundståg from Lund to Malmö C on real tiles (`RAIL_TILES`),
    /// reported every 5 to 30 s with a little GPS error, drawn as the map
    /// does (anew on each report and every 10 s): on the track all the way,
    /// never backwards, close to the real train, and in at Malmö.
    #[test]
    #[ignore]
    fn a_train_from_lund_to_malmo_is_drawn_on_the_track() {
        use crate::motion::{self, Fix, Frame, Stop};
        let dir = std::env::var("RAIL_TILES").unwrap();
        let sets: Vec<Arc<Vec<Track>>> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| {
                let name = e.ok()?.file_name().into_string().ok()?;
                let (x, y) = name.strip_suffix(".pbf")?.split_once('_')?;
                let bytes = std::fs::read(format!("{dir}/{name}")).ok()?;
                Some(Arc::new(decode(&bytes, ZOOM, x.parse().ok()?, y.parse().ok()?)))
            })
            .collect();
        let g = Graph::build(&sets);
        let class = crate::train_class::class_of(Some("PNA044"), None);
        let track = OnGraph { g: &g, class: Some(class) };
        let lund = platform_in(&g, (55.7056, 13.1868), Some("2")).unwrap();
        let malmo = platform_in(&g, (55.6094, 13.0006), Some("3")).unwrap();
        let way = route_in_dir(&g, lund, None, &[malmo], 0.0, 1.0, Some(&class), false).unwrap();
        let path: Vec<(f64, f64)> = way.points.clone();
        let mut at = vec![0.0];
        for w in path.windows(2) {
            at.push(at.last().unwrap() + distance(w[0], w[1]));
        }
        let length = *at.last().unwrap();
        const T0: i64 = 1_800_000_000_000;
        let point = |s: f64| {
            let i = at.partition_point(|d| *d <= s).clamp(1, path.len() - 1);
            let k = ((s - at[i - 1]) / (at[i] - at[i - 1]).max(1e-9)).clamp(0.0, 1.0);
            let (a, b) = (path[i - 1], path[i]);
            ((a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k), course(a, b))
        };
        // how far along the real way a place is, and how far off it
        let along = |p: (f64, f64)| {
            path.windows(2)
                .enumerate()
                .map(|(i, w)| {
                    let (q, _) = project(p, w[0], w[1]);
                    (distance(p, q), at[i] + distance(w[0], q))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .unwrap()
        };
        // the real run: the track's limits (90% of them), up to its top, braking into Malmö
        let limit = |s: f64| {
            let i = at.partition_point(|d| *d <= s).clamp(1, path.len() - 1);
            way.speeds.get(i - 1).copied().flatten().unwrap_or(30.0).min(class.top()) * 0.9
        };
        // calling at Hjärup on the way, for 45 s
        let hjarup = platform_in(&g, (55.6696, 13.1361), None).unwrap();
        let (hjarup_off, hjarup_at) = along(hjarup);
        assert!(hjarup_off < 10.0, "Hjärup's platform is on the way: {hjarup_off:.0} m off");
        let mut real: Vec<(f64, f64)> = Vec::new();
        let (mut s, mut v) = (0.0f64, 0.0f64);
        let (mut stood, mut left_hjarup) = (0, None);
        for i in 0..12_000 {
            real.push((s, v));
            let end = if stood < 450 { hjarup_at } else { length };
            if (end - s).abs() < 0.01 && end == hjarup_at {
                stood += 1;
                v = 0.0;
                if stood == 450 {
                    left_hjarup = Some(T0 + i * 100);
                }
                continue;
            }
            let mut cap = (2.0 * 0.5 * (end - s - 1.0).max(0.0)).sqrt().min(limit(s));
            let mut d = 0.0;
            while d < 2_500.0 {
                cap = cap.min((limit(s + d).powi(2) + 2.0 * 0.5 * d).sqrt());
                d += 50.0;
            }
            v = if v > cap { (v - 0.05).max(cap) } else { (v + 0.04).min(cap.max(0.3)) };
            s = (s + v * 0.1).min(end);
        }
        let real_at = |t: i64| real[(((t - T0) / 100) as usize).min(real.len() - 1)];
        let stops = [
            Stop { lat: lund.0, lon: lund.1, arrive_ms: None, depart_ms: Some(T0), pass: false },
            Stop {
                lat: hjarup.0,
                lon: hjarup.1,
                arrive_ms: Some(T0 + 300_000),
                depart_ms: Some(T0 + 360_000),
                pass: false,
            },
            Stop { lat: malmo.0, lon: malmo.1, arrive_ms: Some(T0 + 1_000_000), depart_ms: None, pass: false },
        ];
        let gaps = [12, 25, 8, 30, 15, 20, 5, 28, 18, 10];
        let mut times = vec![T0];
        while *times.last().unwrap() < T0 + 1_250_000 {
            times.push(times.last().unwrap() + gaps[times.len() % gaps.len()] * 1000);
        }
        let report = |t: i64, k: usize| {
            let (s, v) = real_at(t);
            let (p, heading) = point(s);
            // a few metres off, either side
            let p = ahead(p, heading + 90.0, [3.0, -4.0, 1.5, -2.0, 5.0][k % 5]);
            Fix { lat: p.0, lon: p.1, t_ms: t, speed: Some((v * 3.6).round() / 3.6), heading: Some(heading.round()) }
        };
        let (mut frames, mut latest, mut prev): (Option<Vec<Frame>>, Option<Fix>, Option<Fix>) = (None, None, None);
        let (mut heard, mut last_sim) = (0, i64::MIN);
        let mut drawn: Vec<(i64, f64, f64)> = Vec::new();
        let mut gaps_off: Vec<f64> = Vec::new();
        let mut moving_again = None;
        for now in (T0..T0 + 1_250_000).step_by(1000) {
            let mut fresh = false;
            while heard < times.len() && times[heard] + 2_000 <= now {
                let r = report(times[heard], heard);
                heard += 1;
                if left_hjarup.is_some_and(|t| r.t_ms > t) && moving_again.is_none() {
                    moving_again = Some(now);
                }
                if latest.is_some_and(|l| {
                    distance((l.lat, l.lon), (r.lat, r.lon)) < 2.0 && r.speed.is_some_and(|v| v >= 1.0)
                }) {
                    continue;
                }
                if latest.is_none_or(|l| distance((l.lat, l.lon), (r.lat, r.lon)) >= 2.0) {
                    prev = latest;
                }
                latest = Some(r);
                fresh = true;
            }
            let Some(l) = latest else { continue };
            if fresh || now - last_sim >= 10_000 {
                let next = motion::simulate(frames.as_deref(), &l, prev.as_ref(), &stops, now, Some(&track)).frames;
                assert!(!next.is_empty());
                if let Some(old) = &frames {
                    let a = motion::sample(old, now).unwrap().0;
                    let b = motion::sample(&next, now).map_or((next[0].0, next[0].1), |s| s.0);
                    assert!(distance(a, b) < 25.0, "a jump at {} s: {:.0} m", (now - T0) / 1000, distance(a, b));
                }
                frames = Some(next);
                last_sim = now;
            }
            let f = frames.as_ref().unwrap();
            let p = motion::sample(f, now).map_or((f[0].0, f[0].1), |s| s.0);
            let (off, s) = along(p);
            let on = g.snap(p, 50.0).map_or(f64::MAX, |s| s.3);
            drawn.push((now, s, off));
            gaps_off.push(on);
        }
        let back = drawn.windows(2).map(|w| w[0].1 - w[1].1).fold(0.0f64, f64::max);
        let errors: Vec<f64> = drawn.iter().map(|(t, s, _)| (s - real_at(*t).0).abs()).collect();
        let worst = errors.iter().fold(0.0f64, |a, b| a.max(*b));
        let mean = errors.iter().sum::<f64>() / errors.len() as f64;
        let off = drawn.iter().map(|d| d.2).fold(0.0f64, f64::max);
        let track_off = gaps_off.iter().fold(0.0f64, |a, b| a.max(*b));
        eprintln!("off the track: at most {track_off:.1} m");
        // (between frames it cuts across bends a little)
        assert!(track_off < 3.0, "on the track: {track_off:.1} m off it");
        // in at Hjärup, and out only once a report shows the real train moving off
        let at_hjarup: Vec<i64> = drawn
            .windows(2)
            .filter(|w| (w[0].1 - hjarup_at).abs() < 60.0 && (w[1].1 - w[0].1).abs() < 0.01)
            .map(|w| w[0].0)
            .collect();
        eprintln!("stood at Hjärup {} s", at_hjarup.len());
        assert!(at_hjarup.len() >= 30, "stood at Hjärup: {} s", at_hjarup.len());
        let set_off = *at_hjarup.last().unwrap();
        assert!(
            set_off + 1_000 >= moving_again.unwrap(),
            "left Hjärup at {} s, before a report said so at {} s",
            (set_off - T0) / 1000,
            (moving_again.unwrap() - T0) / 1000
        );
        eprintln!(
            "{length:.0} m: off by {worst:.0} m at worst, {mean:.0} on average; {off:.1} m from the real way at most; back {back:.1} m"
        );
        assert!(back < 6.0, "went back {back:.1} m (between tracks side by side at most)");
        assert!(worst < 600.0 && mean < 150.0, "off by {worst:.0} m at worst, {mean:.0} on average");
        let end = drawn.last().unwrap();
        assert!((end.1 - length).abs() < 60.0, "in at Malmö: {:.0} of {length:.0} m", end.1);
    }

    /// Real tiles (`RAIL_TILES`): Hjärup's tracks aren't numbered in the map;
    /// a train is stopped on the one running its way, each way.
    #[test]
    #[ignore]
    fn hjarup_offers_the_track_running_the_trains_way() {
        let dir = std::env::var("RAIL_TILES").unwrap();
        let sets: Vec<Arc<Vec<Track>>> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| {
                let name = e.ok()?.file_name().into_string().ok()?;
                let (x, y) = name.strip_suffix(".pbf")?.split_once('_')?;
                let bytes = std::fs::read(format!("{dir}/{name}")).ok()?;
                Some(Arc::new(decode(&bytes, ZOOM, x.parse().ok()?, y.parse().ok()?)))
            })
            .collect();
        let g = Graph::build(&sets);
        let hjarup = (55.6696, 13.1361);
        for heading in [225.0, 45.0] {
            let p = platforms_in(&g, hjarup, None, Some(heading), &[])[0];
            let (a, b, _, _) = g.snap(p, 1.0).unwrap();
            let forward = turn(course(g.nodes[a], g.nodes[b]), heading) < 90.0;
            let e = if forward { g.edge(a, b) } else { g.edge(b, a) };
            assert_eq!(e.and_then(|e| e.flow), Some(true), "going {heading}°: {p:?}");
        }
    }
}
