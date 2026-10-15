//! The railway network, so trains run along the track rather than straight
//! across country. It comes from OpenFreeMap's vector tiles, the ones the map
//! draws its rail map from: the `transportation` layer's `rail` lines at
//! zoom 12, where they are main lines, not sidings.
//!
//! Tiles are fetched in the background and kept; a route is only made from
//! tiles already here, so asking never waits on the network. A train whose
//! tiles haven't arrived yet goes straight until the next refresh.

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::motion::{ahead, course, distance};

const TILEJSON: &str = "https://tiles.openfreemap.org/planet";
const ZOOM: u32 = 12;
/// Tiles kept in memory (each is a few kilobytes of lines).
const MAX_TILES: usize = 6000;
/// A route needing more tiles than this is too long to make here.
const MAX_ROUTE_TILES: usize = 48;
/// Fetches at once.
const MAX_LOADING: usize = 8;
/// A tile that failed is asked again after this.
const RETRY: Duration = Duration::from_secs(120);
/// A train further than this from any track isn't on one we know.
const SNAP_M: f64 = 150.0;
/// A station further than this from the track isn't on it.
const STATION_SNAP_M: f64 = 800.0;
/// Track ends this close are joined (tile seams, small gaps in the data).
const STITCH_M: f64 = 25.0;
/// Going on along the track it takes the straightest way at a junction, but
/// never one turning back on itself (a stitched-on spur).
const MAX_TURN: f64 = 120.0;

type Line = Vec<(f64, f64)>;
type Key = (u32, u32);

struct Tiles {
    template: Option<String>,
    lines: HashMap<Key, (Instant, Arc<Vec<Line>>)>,
    loading: HashSet<Key>,
    failed: HashMap<Key, Instant>,
}

fn tiles() -> &'static Mutex<Tiles> {
    static T: OnceLock<Mutex<Tiles>> = OnceLock::new();
    T.get_or_init(|| {
        Mutex::new(Tiles { template: None, lines: HashMap::new(), loading: HashSet::new(), failed: HashMap::new() })
    })
}

fn client() -> Option<&'static reqwest::Client> {
    static C: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    C.get_or_init(|| {
        fr24::client_builder()
            .user_agent(concat!("RustRadar/", env!("CARGO_PKG_VERSION"), " (+https://github.com/felixgoff/RustRadar)"))
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

/// A point in tile `(x, y)`'s own units (`0..extent`, y down) to degrees.
fn to_lat_lon(x: u32, y: u32, extent: f64, px: f64, py: f64) -> (f64, f64) {
    let n = f64::from(1u32 << ZOOM);
    let lon = (f64::from(x) + px / extent) / n * 360.0 - 180.0;
    let m = std::f64::consts::PI * (1.0 - 2.0 * (f64::from(y) + py / extent) / n);
    (m.sinh().atan().to_degrees(), lon)
}

/// Protocol buffer reading, just enough for a vector tile.
struct Pb<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Pb<'a> {
    fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.b.get(self.i)?;
            self.i += 1;
            v |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return Some(v);
            }
        }
        None
    }

    /// The next field: its number, and its value (a varint, or the bytes of a
    /// length-delimited one); fixed-size fields are skipped over.
    fn field(&mut self) -> Option<(u64, Result<u64, &'a [u8]>)> {
        let key = self.varint()?;
        let value = match key & 7 {
            0 => Ok(self.varint()?),
            2 => {
                let len = usize::try_from(self.varint()?).ok()?;
                let end = self.i.checked_add(len)?;
                let bytes = self.b.get(self.i..end)?;
                self.i = end;
                Err(bytes)
            }
            1 => {
                self.i += 8;
                Ok(0)
            }
            5 => {
                self.i += 4;
                Ok(0)
            }
            _ => return None,
        };
        Some((key >> 3, value))
    }
}

fn fields(b: &[u8]) -> impl Iterator<Item = (u64, Result<u64, &[u8]>)> {
    let mut pb = Pb { b, i: 0 };
    std::iter::from_fn(move || if pb.i < pb.b.len() { pb.field() } else { None })
}

fn packed(b: &[u8]) -> Vec<u64> {
    let mut pb = Pb { b, i: 0 };
    std::iter::from_fn(|| if pb.i < pb.b.len() { pb.varint() } else { None }).collect()
}

/// The rail lines in a vector tile, in degrees.
pub fn decode(pbf: &[u8], x: u32, y: u32) -> Vec<Line> {
    let mut out = Vec::new();
    for (f, layer) in fields(pbf) {
        let (3, Err(layer)) = (f, layer) else { continue };
        let mut name = "";
        let mut keys: Vec<&str> = Vec::new();
        let mut values: Vec<Option<&str>> = Vec::new();
        let mut features: Vec<&[u8]> = Vec::new();
        let mut extent = 4096.0;
        for (lf, v) in fields(layer) {
            match (lf, v) {
                (1, Err(b)) => name = std::str::from_utf8(b).unwrap_or_default(),
                (2, Err(b)) => features.push(b),
                (3, Err(b)) => keys.push(std::str::from_utf8(b).unwrap_or_default()),
                (4, Err(b)) => values.push(fields(b).find_map(|(vf, vv)| match (vf, vv) {
                    (1, Err(s)) => std::str::from_utf8(s).ok(),
                    _ => None,
                })),
                (5, Ok(e)) => extent = e as f64,
                _ => {}
            }
        }
        if name != "transportation" {
            continue;
        }
        for feature in features {
            let mut rail = false;
            let mut geometry: &[u8] = &[];
            let mut kind = 0;
            for (ff, v) in fields(feature) {
                match (ff, v) {
                    (2, Err(tags)) => {
                        let t = packed(tags);
                        rail = t.chunks(2).any(|kv| {
                            kv.len() == 2
                                && keys.get(kv[0] as usize) == Some(&"class")
                                && values.get(kv[1] as usize).copied().flatten() == Some("rail")
                        });
                    }
                    (3, Ok(k)) => kind = k,
                    (4, Err(g)) => geometry = g,
                    _ => {}
                }
            }
            if !rail || kind != 2 {
                continue;
            }
            // MoveTo starts a line, LineTo goes on; coordinates are zigzag deltas
            let commands = packed(geometry);
            let (mut cx, mut cy) = (0i64, 0i64);
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
                        out.push(std::mem::take(&mut line));
                    } else if id == 1 {
                        line.clear();
                    }
                    line.push(to_lat_lon(x, y, extent, cx as f64, cy as f64));
                }
            }
            if line.len() > 1 {
                out.push(line);
            }
        }
    }
    out
}

async fn template() -> Option<String> {
    if let Some(t) = tiles().lock().unwrap().template.clone() {
        return Some(t);
    }
    let body = client()?.get(TILEJSON).send().await.ok()?.text().await.ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let t = json["tiles"][0].as_str()?.to_owned();
    tiles().lock().unwrap().template = Some(t.clone());
    Some(t)
}

async fn fetch(key: Key) {
    let lines = async {
        let url = template()
            .await?
            .replace("{z}", &ZOOM.to_string())
            .replace("{x}", &key.0.to_string())
            .replace("{y}", &key.1.to_string());
        let response = client()?.get(url).send().await.ok()?;
        if response.status().as_u16() == 204 || response.status().as_u16() == 404 {
            return Some(Vec::new()); // nothing there: no rail
        }
        if !response.status().is_success() {
            return None;
        }
        Some(decode(&response.bytes().await.ok()?, key.0, key.1))
    }
    .await;
    let mut t = tiles().lock().unwrap();
    t.loading.remove(&key);
    match lines {
        Some(lines) => {
            if t.lines.len() >= MAX_TILES
                && let Some(oldest) = t.lines.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| *k)
            {
                t.lines.remove(&oldest);
            }
            t.lines.insert(key, (Instant::now(), Arc::new(lines)));
        }
        None => {
            t.failed.insert(key, Instant::now());
        }
    }
}

/// The lines of these tiles if all are here; the missing ones are sent for.
fn lines_of(keys: &[Key]) -> Option<Vec<Arc<Vec<Line>>>> {
    let mut t = tiles().lock().unwrap();
    let mut out = Vec::with_capacity(keys.len());
    let mut missing = Vec::new();
    for key in keys {
        match t.lines.get_mut(key) {
            Some((at, lines)) => {
                *at = Instant::now();
                out.push(lines.clone());
            }
            None => missing.push(*key),
        }
    }
    // tests never go to the network
    if cfg!(test) {
        return missing.is_empty().then_some(out);
    }
    for key in &missing {
        let recently_failed = t.failed.get(key).is_some_and(|at| at.elapsed() < RETRY);
        if t.loading.len() < MAX_LOADING && !recently_failed && t.loading.insert(*key) {
            tauri::async_runtime::spawn(fetch(*key));
        }
    }
    missing.is_empty().then_some(out)
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

struct Graph {
    nodes: Vec<(f64, f64)>,
    adj: Vec<Vec<(usize, f64)>>,
}

impl Graph {
    fn build(sets: &[Arc<Vec<Line>>]) -> Graph {
        let mut g = Graph { nodes: Vec::new(), adj: Vec::new() };
        let mut index: HashMap<(i64, i64), usize> = HashMap::new();
        let mut node = |g: &mut Graph, p: (f64, f64)| -> usize {
            let k = ((p.0 * 1e6).round() as i64, (p.1 * 1e6).round() as i64);
            *index.entry(k).or_insert_with(|| {
                g.nodes.push(p);
                g.adj.push(Vec::new());
                g.nodes.len() - 1
            })
        };
        for line in sets.iter().flat_map(|s| s.iter()) {
            let mut prev: Option<usize> = None;
            for &p in line {
                let n = node(&mut g, p);
                if let Some(a) = prev.filter(|a| *a != n) {
                    g.link(a, n);
                }
                prev = Some(n);
            }
        }
        g.stitch();
        g
    }

    fn link(&mut self, a: usize, b: usize) {
        let d = distance(self.nodes[a], self.nodes[b]);
        if !self.adj[a].iter().any(|(n, _)| *n == b) {
            self.adj[a].push((b, d));
            self.adj[b].push((a, d));
        }
    }

    /// Joins each loose end to the nearest other track within `STITCH_M`:
    /// tiles cut lines at their edges, and the pieces don't quite meet.
    fn stitch(&mut self) {
        const CELL: f64 = 0.0005;
        let cell = |p: (f64, f64)| ((p.0 / CELL).floor() as i64, (p.1 / CELL).floor() as i64);
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, p) in self.nodes.iter().enumerate() {
            grid.entry(cell(*p)).or_default().push(i);
        }
        let ends: Vec<usize> = (0..self.nodes.len()).filter(|i| self.adj[*i].len() == 1).collect();
        for a in ends {
            let (cx, cy) = cell(self.nodes[a]);
            let neighbour = self.adj[a][0].0;
            let best = (cx - 1..=cx + 1)
                .flat_map(|x| (cy - 1..=cy + 1).map(move |y| (x, y)))
                .filter_map(|c| grid.get(&c))
                .flatten()
                .filter(|&&b| b != a && b != neighbour)
                .map(|&b| (b, distance(self.nodes[a], self.nodes[b])))
                .filter(|(_, d)| *d <= STITCH_M)
                .min_by(|x, y| x.1.total_cmp(&y.1));
            if let Some((b, _)) = best {
                self.link(a, b);
            }
        }
    }

    /// The nearest point on the track: the segment's ends, how far along, and how far off.
    fn snap(&self, p: (f64, f64), within: f64) -> Option<(usize, usize, (f64, f64), f64)> {
        let mut best: Option<(usize, usize, (f64, f64), f64)> = None;
        let k = p.0.to_radians().cos();
        for a in 0..self.nodes.len() {
            for &(b, _) in &self.adj[a] {
                if b < a {
                    continue;
                }
                let (pa, pb) = (self.nodes[a], self.nodes[b]);
                // flat projection: plenty for a segment of a few hundred metres
                let (ax, ay, bx, by, px, py) = (pa.1 * k, pa.0, pb.1 * k, pb.0, p.1 * k, p.0);
                let (dx, dy) = (bx - ax, by - ay);
                let len2 = dx * dx + dy * dy;
                let t = if len2 > 0.0 { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                let q = (pa.0 + (pb.0 - pa.0) * t, pa.1 + (pb.1 - pa.1) * t);
                let d = distance(p, q);
                if d <= within && best.is_none_or(|bst| d < bst.3) {
                    best = Some((a, b, q, d));
                }
            }
        }
        best
    }

    fn nearest_node(&self, p: (f64, f64), within: f64) -> Option<usize> {
        (0..self.nodes.len())
            .filter(|i| !self.adj[*i].is_empty())
            .map(|i| (i, distance(p, self.nodes[i])))
            .filter(|(_, d)| *d <= within)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// The shortest way along the track from `from` to `to`, not leaving
    /// `from` towards `not_via` (the way it came).
    fn path(&self, from: usize, to: usize, not_via: Option<usize>) -> Option<Vec<usize>> {
        #[derive(PartialEq)]
        struct Item(f64, usize);
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
        let mut dist = vec![f64::INFINITY; self.nodes.len()];
        let mut prev = vec![usize::MAX; self.nodes.len()];
        let mut heap = BinaryHeap::new();
        dist[from] = 0.0;
        heap.push(Item(0.0, from));
        while let Some(Item(d, n)) = heap.pop() {
            if n == to {
                break;
            }
            if d > dist[n] {
                continue;
            }
            for &(m, w) in &self.adj[n] {
                if n == from && Some(m) == not_via {
                    continue;
                }
                let nd = d + w;
                if nd < dist[m] {
                    dist[m] = nd;
                    prev[m] = n;
                    heap.push(Item(nd, m));
                }
            }
        }
        if !dist[to].is_finite() {
            return None;
        }
        let mut out = vec![to];
        while *out.last()? != from {
            out.push(prev[*out.last()?]);
        }
        out.reverse();
        Some(out)
    }

    /// Onwards from `at`, having come from `came`, as straight as the track
    /// allows, for `metres`.
    fn onwards(&self, mut came: usize, mut at: usize, metres: f64) -> Vec<usize> {
        let mut out = Vec::new();
        let mut left = metres;
        let mut seen = HashSet::from([came, at]);
        while left > 0.0 {
            let heading = course(self.nodes[came], self.nodes[at]);
            let next = self.adj[at]
                .iter()
                .filter(|(m, _)| !seen.contains(m))
                .map(|&(m, w)| (m, w, turn(heading, course(self.nodes[at], self.nodes[m]))))
                .filter(|(_, _, t)| *t <= MAX_TURN)
                .min_by(|a, b| a.2.total_cmp(&b.2));
            let Some((m, w, _)) = next else { break };
            out.push(m);
            seen.insert(m);
            left -= w;
            (came, at) = (at, m);
        }
        out
    }
}

fn turn(a: f64, b: f64) -> f64 {
    let d = (b - a).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// A way along the track: its points, and how far along it each target was reached.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub points: Vec<(f64, f64)>,
    pub reached: Vec<f64>,
}

/// A way along the known track from `start` (heading roughly `heading`)
/// through as many of `targets` as it can reach in order, then on along the
/// track for `onwards_m`. None while the track there isn't loaded (it is sent
/// for), or when `start` isn't on a track.
pub fn route(start: (f64, f64), heading: Option<f64>, targets: &[(f64, f64)], onwards_m: f64) -> Option<Route> {
    let mut targets = targets.to_vec();
    let mut keys = tiles_for(&[start, ahead(start, heading.unwrap_or(0.0), onwards_m.min(5_000.0))], 0.01);
    if !targets.is_empty() {
        let mut points = vec![start];
        points.extend(&targets);
        let all = tiles_for(&points, 0.02);
        if all.len() <= MAX_ROUTE_TILES {
            keys = all;
        } else {
            targets.clear();
        }
    }
    if keys.len() > MAX_ROUTE_TILES {
        return None;
    }
    let sets = lines_of(&keys)?;
    let g = Graph::build(&sets);
    route_in(&g, start, heading, &targets, onwards_m)
}

/// `route` in the shape the motion module takes (`motion::Router`).
pub fn router(
    start: (f64, f64),
    heading: Option<f64>,
    targets: &[(f64, f64)],
    onwards_m: f64,
) -> Option<crate::motion::Routed> {
    route(start, heading, targets, onwards_m).map(|r| (r.points, r.reached))
}

fn route_in(
    g: &Graph,
    start: (f64, f64),
    heading: Option<f64>,
    targets: &[(f64, f64)],
    onwards_m: f64,
) -> Option<Route> {
    let (a, b, on, _) = g.snap(start, SNAP_M)?;
    // which way along the segment it is going: the end nearer its heading,
    // else towards the first target
    let toward_b = match (heading, targets.first()) {
        (Some(h), _) => turn(h, course(on, g.nodes[b])) <= turn(h, course(on, g.nodes[a])),
        (None, Some(t)) => distance(g.nodes[b], *t) <= distance(g.nodes[a], *t),
        (None, None) => true,
    };
    let (behind, front) = if toward_b { (a, b) } else { (b, a) };
    let mut nodes = vec![front];
    let mut reached = Vec::new();
    let mut points = vec![on, g.nodes[front]];
    let mut length = distance(on, g.nodes[front]);
    let mut came = behind;
    for target in targets {
        let Some(to) = g.nearest_node(*target, STATION_SNAP_M) else { break };
        let here = *nodes.last()?;
        let Some(path) = g.path(here, to, Some(came)) else { break };
        for w in path.windows(2) {
            length += distance(g.nodes[w[0]], g.nodes[w[1]]);
            points.push(g.nodes[w[1]]);
        }
        if path.len() >= 2 {
            came = path[path.len() - 2];
        }
        nodes.push(to);
        reached.push(length);
    }
    let last = *nodes.last()?;
    for n in g.onwards(came, last, onwards_m) {
        points.push(g.nodes[n]);
    }
    points.dedup();
    (points.len() >= 2).then_some(Route { points, reached })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(lines: Vec<Line>) -> Graph {
        Graph::build(&[Arc::new(lines)])
    }

    /// A line of points every 100 m from `from` on a course.
    fn track(from: (f64, f64), course: f64, km: f64) -> Line {
        (0..=(km * 10.0) as usize).map(|i| ahead(from, course, i as f64 * 100.0)).collect()
    }

    #[test]
    fn it_follows_the_bend_rather_than_going_straight() {
        // east for 2 km, then north for 3 km
        let corner = ahead((57.0, 12.0), 90.0, 2_000.0);
        let mut line = track((57.0, 12.0), 90.0, 2.0);
        line.extend(track(corner, 0.0, 3.0).into_iter().skip(1));
        let g = graph(vec![line]);
        let start = ahead((57.0, 12.0), 90.0, 500.0);
        let r = route_in(&g, (start.0 + 0.0002, start.1), Some(90.0), &[], 3_000.0).unwrap();
        let end = *r.points.last().unwrap();
        assert!(end.0 > corner.0 + 0.005, "went round the corner northwards: {end:?}");
        assert!((end.1 - corner.1).abs() < 0.001, "stayed on the northbound track");
    }

    #[test]
    fn it_finds_the_way_to_a_station_through_a_junction() {
        // a junction: one branch north-east, one south-east; the station is on the southern one
        let junction = ahead((57.0, 12.0), 90.0, 1_000.0);
        let main = track((57.0, 12.0), 90.0, 1.0);
        let north = track(junction, 45.0, 4.0);
        let south = track(junction, 135.0, 4.0);
        let station = ahead(junction, 135.0, 3_000.0);
        let g = graph(vec![main, north, south]);
        let r = route_in(&g, (57.0, 12.0005), Some(90.0), &[station], 500.0).unwrap();
        assert_eq!(r.reached.len(), 1);
        assert!((r.reached[0] - 4_000.0).abs() < 150.0, "about 4 km along: {}", r.reached[0]);
        assert!(r.points.iter().all(|p| p.0 <= 57.0 + 1e-4), "never on the northern branch");
    }

    #[test]
    fn tile_seams_are_stitched() {
        // the same line cut in two with a 10 m gap
        let a = track((57.0, 12.0), 90.0, 1.0);
        let gap_start = ahead(*a.last().unwrap(), 90.0, 10.0);
        let b = track(gap_start, 90.0, 1.0);
        let g = graph(vec![a, b]);
        let r = route_in(&g, (57.0, 12.0001), Some(90.0), &[], 1_800.0).unwrap();
        assert!(distance(r.points[0], *r.points.last().unwrap()) > 1_500.0);
    }

    #[test]
    fn tiles_and_geometry_decode() {
        assert_eq!(tile_of(55.70, 13.19), (2198, 1281));
        // one layer "transportation", extent 4096, keys ["class"], values ["rail"],
        // a line feature from (0,0) to (4096,4096)
        let value = [0x0a, 0x04, b'r', b'a', b'i', b'l'];
        let geometry = [9, 0, 0, 10, 0x80, 0x40, 0x80, 0x40];
        let mut feature = vec![0x12, 2, 0, 0, 0x18, 2, 0x22, geometry.len() as u8];
        feature.extend(geometry);
        let mut layer = vec![0x0a, 14];
        layer.extend(b"transportation");
        layer.extend([0x12, feature.len() as u8]);
        layer.extend(&feature);
        layer.extend([0x1a, 5]);
        layer.extend(b"class");
        layer.extend([0x22, value.len() as u8]);
        layer.extend(value);
        layer.extend([0x28, 0x80, 0x20]);
        let mut tile = vec![0x1a, layer.len() as u8];
        tile.extend(&layer);
        let lines = decode(&tile, 2198, 1281);
        assert_eq!(lines.len(), 1);
        let (nw, se) = (lines[0][0], lines[0][1]);
        assert_eq!(tile_of(nw.0 - 1e-9, nw.1 + 1e-9), (2198, 1281));
        assert!(se.0 < nw.0 && se.1 > nw.1, "south-east of the corner");
    }
}
