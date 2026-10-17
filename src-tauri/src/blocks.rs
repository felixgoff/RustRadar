//! Signal blocks: the stretches of track from one main signal to the next a
//! train meets going the same way, worked out from the signals OpenRailwayMap
//! maps (its `railway_signals` tiles) on the track graph. Which train is in a
//! block is the map's business (where it draws them): the real interlocking's
//! state isn't published.

use super::*;
use serde::Serialize;

/// The zoom the signal tiles are fetched at (they are only served close in).
pub const SIGNAL_ZOOM: u32 = 14;
const SIGNALS: &str = "https://openrailwaymap.app/railway_signals";
/// A signal further than this from any track isn't on one we know, metres.
const SIGNAL_SNAP_M: f64 = 15.0;
/// A block is followed at most this far (a line without signals mapped), metres.
const MAX_BLOCK_M: f64 = 8_000.0;
/// Signal tiles fetched at once.
const MAX_SIGNAL_LOADING: usize = 8;
/// At most this many signal tiles for one view (close in only).
const MAX_SIGNAL_TILES: usize = 240;

/// A signal: where it is, the way the trains it is for go (degrees), and
/// whether it is a main signal (one a block starts at).
#[derive(Clone, Debug, PartialEq)]
pub struct Signal {
    pub at: (f64, f64),
    pub heading: Option<f64>,
    pub main: bool,
    pub name: Option<String>,
}

/// A block: the main signal it starts at, and its track (`[lon, lat]`
/// stretches; it branches where the track does).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub id: String,
    pub signal: Option<String>,
    pub lines: Vec<Vec<[f64; 2]>>,
}

/// The signals in a `railway_signals` tile.
pub fn decode_signals(pbf: &[u8], z: u32, x: u32, y: u32) -> Vec<Signal> {
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
        if name != "railway_signals" {
            continue;
        }
        for feature in features {
            let mut tags: HashMap<&str, &Val> = HashMap::new();
            let mut geometry: &[u8] = &[];
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
                    (4, Err(g)) => geometry = g,
                    _ => {}
                }
            }
            let text = |k: &str| match tags.get(k) {
                Some(Val::Str(s)) => Some(*s),
                _ => None,
            };
            let num = |k: &str| match tags.get(k) {
                Some(Val::Num(n)) => Some(*n),
                Some(Val::Bool(b)) => Some(f64::from(u8::from(*b))),
                _ => None,
            };
            // a point: MoveTo with one zigzag pair
            let commands = packed(geometry);
            let (Some(&dx), Some(&dy)) = (commands.get(1), commands.get(2)) else { continue };
            let px = (((dx >> 1) as i64) ^ -((dx & 1) as i64)) as f64;
            let py = (((dy >> 1) as i64) ^ -((dy & 1) as i64)) as f64;
            if !(0.0..=extent).contains(&px) || !(0.0..=extent).contains(&py) {
                continue;
            }
            // main if any of its parts is a main signal still in use
            let main = (0..12).any(|i| {
                text(&format!("category{i}")) == Some("main") && num(&format!("deactivated{i}")).is_none_or(|d| d == 0.0)
            });
            let both = num("direction_both").is_some_and(|b| b != 0.0);
            let heading = num("azimuth").filter(|_| !both).map(|a| a.rem_euclid(360.0));
            out.push(Signal { at: to_lat_lon(z, x, y, extent, px, py), heading, main, name: text("ref").map(str::to_owned) });
        }
    }
    out
}

// --------------------------------------------------------------- tiles

type SignalKey = (u32, u32);

#[derive(Default)]
struct SignalStore {
    tiles: HashMap<SignalKey, Arc<Vec<Signal>>>,
    loading: HashSet<SignalKey>,
    failed: HashMap<SignalKey, Instant>,
}

fn signal_store() -> &'static Mutex<SignalStore> {
    static S: OnceLock<Mutex<SignalStore>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn signal_path(key: SignalKey) -> Option<PathBuf> {
    Some(cache_dir().get()?.parent()?.join(format!("rail-signals/{SIGNAL_ZOOM}/{}/{}.pbf", key.0, key.1)))
}

async fn fetch_signals(key: SignalKey) {
    let (x, y) = key;
    let disk = signal_path(key).and_then(|p| {
        let fresh = std::fs::metadata(&p).ok()?.modified().ok()?.elapsed().is_ok_and(|age| age < DISK_FRESH);
        Some((std::fs::read(&p).ok()?, fresh))
    });
    let bytes = match disk {
        Some((b, true)) => Ok(b),
        stale => {
            let url = format!("{SIGNALS}/{SIGNAL_ZOOM}/{x}/{y}");
            let fetched = async {
                let response = client().ok_or("no HTTP client")?.get(&url).send().await.map_err(|e| e.to_string())?;
                match response.status().as_u16() {
                    204 | 404 => Ok(Vec::new()),
                    s if (200..300).contains(&s) => Ok(response.bytes().await.map_err(|e| e.to_string())?.to_vec()),
                    s => Err(format!("OpenRailwayMap answered {s}")),
                }
            }
            .await;
            match (fetched, stale) {
                (Ok(b), _) => {
                    if let Some(p) = signal_path(key) {
                        if let Some(dir) = p.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        let _ = std::fs::write(p, &b);
                    }
                    Ok(b)
                }
                (Err(_), Some((b, _))) => Ok(b),
                (Err(e), None) => Err(e),
            }
        }
    };
    let mut s = signal_store().lock().unwrap();
    s.loading.remove(&key);
    match bytes {
        Ok(b) => {
            s.tiles.insert(key, Arc::new(decode_signals(&b, SIGNAL_ZOOM, x, y)));
        }
        Err(e) => {
            eprintln!("signal tile {key:?}: {e}");
            s.failed.insert(key, Instant::now());
        }
    }
}

/// The signals of these tiles, if all are here; the missing ones are sent for.
fn signals_of(keys: &[SignalKey]) -> Option<Vec<Signal>> {
    let mut s = signal_store().lock().unwrap();
    let mut out = Vec::new();
    let mut missing = false;
    for key in keys {
        match s.tiles.get(key) {
            Some(t) => out.extend(t.iter().cloned()),
            None => {
                missing = true;
                let recent = s.failed.get(key).is_some_and(|at| at.elapsed() < RETRY);
                if !cfg!(test) && !recent && s.loading.len() < MAX_SIGNAL_LOADING && s.loading.insert(*key) {
                    tauri::async_runtime::spawn(fetch_signals(*key));
                }
            }
        }
    }
    (!missing).then_some(out)
}

/// Tests: every `{x}_{y}.pbf` signal tile in `dir` as if fetched.
#[cfg(test)]
pub fn load_signal_tiles_for_tests(dir: &str) {
    let mut s = signal_store().lock().unwrap();
    for e in std::fs::read_dir(dir).unwrap() {
        let name = e.unwrap().file_name().into_string().unwrap();
        let Some((x, y)) = name.strip_suffix(".pbf").and_then(|n| n.split_once('_')) else { continue };
        let (x, y) = (x.parse().unwrap(), y.parse().unwrap());
        let signals = decode_signals(&std::fs::read(format!("{dir}/{name}")).unwrap_or_default(), SIGNAL_ZOOM, x, y);
        s.tiles.insert((x, y), Arc::new(signals));
    }
}

fn signal_tiles(south: f64, west: f64, north: f64, east: f64) -> Vec<SignalKey> {
    let n = f64::from(1u32 << SIGNAL_ZOOM);
    let x = |lon: f64| (((lon + 180.0) / 360.0 * n).floor().max(0.0) as u32).min((n as u32) - 1);
    let y = |lat: f64| {
        let r = lat.to_radians();
        let v = (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
        (v.floor().max(0.0) as u32).min((n as u32) - 1)
    };
    let mut out = Vec::new();
    for tx in x(west)..=x(east) {
        for ty in y(north)..=y(south) {
            out.push((tx, ty));
        }
    }
    out
}

// --------------------------------------------------------------- blocks

/// The blocks starting at the main signals in an area (degrees), and whether
/// all its track and signals were here (else ask again shortly).
pub fn blocks_in(south: f64, west: f64, north: f64, east: f64) -> (Vec<Block>, bool) {
    let keys = signal_tiles(south, west, north, east);
    if keys.len() > MAX_SIGNAL_TILES {
        return (Vec::new(), true);
    }
    // the track a little round it, for blocks that run out of the view
    let pad = 0.03;
    let tracks = tiles_for(&[(south - pad, west - pad), (north + pad, east + pad)], 0.0);
    let (Some(signals), Some(g)) = (signals_of(&keys), graph_for(tracks)) else { return (Vec::new(), false) };
    (blocks_on(&g, &signals, (south, west, north, east)), true)
}

/// A signal on the track: the segment it is on (`from` → `to`, the way its
/// trains go), and how far along from `from`.
#[derive(Clone, Copy)]
struct Placed {
    from: usize,
    to: usize,
    along: f64,
    at: (f64, f64),
}

fn place(g: &Graph, s: &Signal) -> Option<Placed> {
    let h = s.heading?;
    let (a, b, q, _) = g.snap(s.at, SIGNAL_SNAP_M)?;
    let (from, to) = if turn(h, course(g.nodes[a], g.nodes[b])) <= 90.0 { (a, b) } else { (b, a) };
    Some(Placed { from, to, along: distance(g.nodes[from], q), at: q })
}

fn blocks_on(g: &Graph, signals: &[Signal], area: (f64, f64, f64, f64)) -> Vec<Block> {
    let (south, west, north, east) = area;
    let mains: Vec<(&Signal, Placed)> = signals.iter().filter(|s| s.main).filter_map(|s| Some((s, place(g, s)?))).collect();
    // the main signals on each segment, the way their trains go: how far along
    let mut on: HashMap<(usize, usize), Vec<f64>> = HashMap::new();
    for (_, p) in &mains {
        on.entry((p.from, p.to)).or_default().push(p.along);
    }
    let mut out = Vec::new();
    for (s, p) in mains.iter().filter(|(s, _)| (south..=north).contains(&s.at.0) && (west..=east).contains(&s.at.1)) {
        let mut lines: Vec<Line> = Vec::new();
        // on along its own segment first: another signal there ends it
        let len = distance(g.nodes[p.from], g.nodes[p.to]);
        let next_here = on[&(p.from, p.to)].iter().copied().filter(|a| *a > p.along + 1.0).fold(f64::INFINITY, f64::min);
        if next_here.is_finite() {
            let k = next_here / len.max(0.01);
            let (a, b) = (g.nodes[p.from], g.nodes[p.to]);
            lines.push(vec![p.at, (a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k)]);
        } else {
            // then on through the track, every way it can go, until the next main signal its way
            let mut stack: Vec<(usize, usize, Line, f64)> = vec![(p.from, p.to, vec![p.at, g.nodes[p.to]], len - p.along)];
            let mut seen: HashSet<(usize, usize)> = HashSet::from([(p.from, p.to)]);
            while let Some((came, at, mut line, gone)) = stack.pop() {
                let ways: Vec<&Edge> = g.adj[at]
                    .iter()
                    .filter(|e| e.to != came && !e.yard && g.turnable(came, at, e.to) && !seen.contains(&(at, e.to)))
                    .collect();
                if ways.is_empty() || gone > MAX_BLOCK_M {
                    lines.push(line);
                    continue;
                }
                // a branch starts lines of its own from here
                let branching = ways.len() > 1;
                if branching {
                    lines.push(std::mem::take(&mut line));
                }
                for e in ways {
                    seen.insert((at, e.to));
                    let mut l = if branching { vec![g.nodes[at]] } else { std::mem::take(&mut line) };
                    match on.get(&(at, e.to)).and_then(|s| s.iter().copied().reduce(f64::min)) {
                        Some(stop) => {
                            let k = stop / e.len.max(0.01);
                            let (a, b) = (g.nodes[at], g.nodes[e.to]);
                            l.push((a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k));
                            lines.push(l);
                        }
                        None => {
                            l.push(g.nodes[e.to]);
                            stack.push((at, e.to, l, gone + e.len));
                        }
                    }
                }
            }
        }
        let lines: Vec<Vec<[f64; 2]>> =
            lines.into_iter().filter(|l| l.len() >= 2).map(|l| l.into_iter().map(|p| [p.1, p.0]).collect()).collect();
        if !lines.is_empty() {
            out.push(Block { id: format!("{:.6},{:.6}", s.at.0, s.at.1), signal: s.name.clone(), lines });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(from: (f64, f64), course: f64, km: f64) -> Line {
        (0..=(km * 10.0) as usize).map(|i| ahead(from, course, i as f64 * 100.0)).collect()
    }

    fn signal(at: (f64, f64), heading: f64) -> Signal {
        Signal { at, heading: Some(heading), main: true, name: None }
    }

    #[test]
    fn a_block_runs_from_its_signal_to_the_next_one_its_trains_meet() {
        // a line east 5 km; main signals at 1 km and 3 km for eastbound trains, one at 2 km for westbound
        let a = (61.0, 15.0);
        let g = Graph::build(&[Arc::new(vec![Track { points: line(a, 90.0, 5.0), ..Default::default() }])]);
        let at = |m: f64| ahead(a, 90.0, m);
        let signals = [signal(at(1_000.0), 90.0), signal(at(3_000.0), 90.0), signal(at(2_050.0), 270.0)];
        let blocks = blocks_on(&g, &signals, (60.0, 14.0, 62.0, 16.0));
        assert_eq!(blocks.len(), 3);
        let length = |b: &Block| -> f64 {
            b.lines.iter().flat_map(|l| l.windows(2)).map(|w| distance((w[0][1], w[0][0]), (w[1][1], w[1][0]))).sum()
        };
        let from = |m: f64| blocks.iter().find(|b| distance((b.lines[0][0][1], b.lines[0][0][0]), at(m)) < 1.0).unwrap();
        assert!((length(from(1_000.0)) - 2_000.0).abs() < 5.0, "1 km to 3 km: {}", length(from(1_000.0)));
        // the last runs on to the end of the line; the westbound one back to the start
        assert!((length(from(3_000.0)) - 2_000.0).abs() < 5.0);
        assert!((length(from(2_050.0)) - 2_050.0).abs() < 5.0);
    }

    #[test]
    fn a_block_branches_where_the_track_does() {
        // east 2 km to a switch, then on east or off north-east; the signal at 500 m
        let a = (61.5, 15.0);
        let j = ahead(a, 90.0, 2_000.0);
        let mut trunk = line(a, 90.0, 2.0);
        *trunk.last_mut().unwrap() = j;
        let (mut on, mut off) = (line(j, 90.0, 2.0), line(j, 60.0, 2.0));
        (on[0], off[0]) = (j, j);
        let g = Graph::build(&[Arc::new(
            [trunk, on, off].into_iter().map(|points| Track { points, ..Default::default() }).collect(),
        )]);
        let blocks = blocks_on(&g, &[signal(ahead(a, 90.0, 500.0), 90.0)], (60.0, 14.0, 63.0, 16.0));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lines.len(), 3, "to the switch, then each way");
    }

    /// Real tiles (`RAIL_TILES`, `SIGNAL_TILES`): whether signals' headings agree with the way traffic runs.
    #[test]
    #[ignore]
    fn signal_headings_on_real_track() {
        load_tiles_for_tests(&std::env::var("RAIL_TILES").unwrap());
        load_signal_tiles_for_tests(&std::env::var("SIGNAL_TILES").unwrap());
        let (south, west, north, east) = (55.62, 13.0, 55.70, 13.18);
        let keys = signal_tiles(south, west, north, east);
        let signals = signals_of(&keys).unwrap();
        let g = graph_for(tiles_for(&[(south - 0.03, west - 0.03), (north + 0.03, east + 0.03)], 0.0)).unwrap();
        let (mut with, mut against, mut unknown, mut off) = (0, 0, 0, 0);
        for s in signals.iter().filter(|s| s.main) {
            match place(&g, s) {
                Some(p) => match g.edge(p.from, p.to).and_then(|e| e.flow) {
                    Some(true) => with += 1,
                    Some(false) => against += 1,
                    None => unknown += 1,
                },
                None => off += 1,
            }
        }
        let blocks = blocks_on(&g, &signals, (south, west, north, east));
        let len = |b: &Block| -> f64 {
            b.lines.iter().flat_map(|l| l.windows(2)).map(|w| distance((w[0][1], w[0][0]), (w[1][1], w[1][0]))).sum()
        };
        let mut lens: Vec<f64> = blocks.iter().map(len).collect();
        lens.sort_by(f64::total_cmp);
        eprintln!(
            "main signals: with traffic {with}, against {against}, flow unknown {unknown}, off the track {off}; {} blocks, median {:.0} m, longest {:.0} m",
            blocks.len(),
            lens.get(lens.len() / 2).copied().unwrap_or(0.0),
            lens.last().copied().unwrap_or(0.0)
        );
    }

    #[test]
    fn signals_are_read_from_their_tiles() {
        // a feature: a point at (2048, 2048), a main signal facing 90°
        let bytes = |n: u64, b: &[u8]| {
            let mut v = vec![(n << 3 | 2) as u8, b.len() as u8];
            v.extend_from_slice(b);
            v
        };
        let mut feature = vec![0x10, 0x01]; // id
        feature.extend(bytes(2, &[0, 0, 1, 1])); // tags: category0=main, azimuth=90
        feature.extend(bytes(4, &[9, 0x80, 0x20, 0x80, 0x20])); // MoveTo(2048, 2048)
        let mut layer = bytes(1, b"railway_signals");
        layer.extend(bytes(2, &feature));
        layer.extend(bytes(3, b"category0"));
        layer.extend(bytes(3, b"azimuth"));
        layer.extend(bytes(4, &bytes(1, b"main")));
        layer.extend(bytes(4, &[0x19, 0, 0, 0, 0, 0, 0x80, 0x56, 0x40])); // double 90
        layer.extend([0x28, 0x80, 0x20]); // extent 4096
        let tile = bytes(3, &layer);
        let s = decode_signals(&tile, SIGNAL_ZOOM, 8792, 5125);
        assert_eq!(s.len(), 1);
        assert!(s[0].main);
        assert_eq!(s[0].heading, Some(90.0));
    }
}
