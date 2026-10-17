//! Live train positions in Sweden: every train on the national network,
//! from Trafikverket's open API with the key from the keys page (see
//! trafikverket.rs). CC0.
//!
//! A stale answer (up to `STALE_MAX`) is returned at once while a fresh one
//! is fetched in the background; calls keep a gap between them and back off
//! after failures or a 429. Between calls, the live streams patch the cached
//! answer (`patch`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::CmdResult;

const USER_AGENT: &str = concat!(
    "RustRadar/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/felixgoff/RustRadar)"
);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Minimum gap between two calls to the same source.
const GAP: Duration = Duration::from_secs(2);
/// A cached answer older than its source's TTL is refreshed; until this age
/// it is still shown meanwhile.
const STALE_MAX: Duration = Duration::from_secs(120);
const BACKOFF_BASE: Duration = Duration::from_secs(15);
const BACKOFF_MAX: Duration = Duration::from_secs(600);
/// Answers kept (one: the whole network).
const CACHE_LIMIT: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    HighSpeed,
    Intercity,
    Regional,
    Suburban,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Train {
    /// Source-prefixed and stable for the run, e.g. `trafikverket:2026-10-13:523`.
    pub id: String,
    pub source: &'static str,
    /// e.g. "SJ Snabbtåg 523", "Pendeltåg 2245".
    pub name: String,
    pub kind: Kind,
    pub lat: f64,
    pub lon: f64,
    /// Degrees clockwise from north.
    pub heading: Option<f32>,
    pub speed_kmh: Option<f32>,
    pub destination: Option<String>,
    pub operator: Option<String>,
    /// Minutes late (negative: early).
    pub delay_minutes: Option<i32>,
    /// When the position was reported, Unix seconds.
    pub position_unix: i64,
    /// Where it is drawn, `(lat, lon, unix_ms)` in time order, from about
    /// now on (from when it stopped, standing); empty when there is nothing
    /// to move it by.
    pub frames: Vec<(f64, f64, i64)>,
    /// Dev builds: its latest report and the one before, for the map to
    /// mark. Empty in release builds.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reports: Vec<Sighting>,
    /// Dev builds: where the real train most likely is, as `frames`, for the
    /// map to mark. Empty in release builds.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub predicted: Vec<(f64, f64, i64)>,
    /// How many cars (and locomotives) it runs with, where its operator says.
    pub cars: Option<u32>,
    /// The track it has come along, `(lat, lon)` back from the start of
    /// `frames`, for its cars to be drawn on; empty when not known.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tail: Vec<(f64, f64)>,
}

/// A position report as the dev map marks it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sighting {
    pub lat: f64,
    pub lon: f64,
    pub unix_ms: i64,
    pub speed_kmh: Option<f32>,
    /// The newest report; else the one before it, the train is drawn from.
    pub latest: bool,
}

/// Degrees; `west > east` crosses the antimeridian.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

impl Area {
    const WORLD: Area = Area { west: -180.0, south: -90.0, east: 180.0, north: 90.0 };

    /// From a view as the map reports it: longitudes wrapped, latitudes ordered.
    fn view(west: f64, south: f64, east: f64, north: f64) -> Option<Area> {
        if ![west, south, east, north].iter().all(|x| x.is_finite()) {
            return None;
        }
        let (south, north) = (south.min(north).max(-90.0), north.max(south).min(90.0));
        if east - west >= 360.0 {
            return Some(Area { south, north, ..Area::WORLD });
        }
        let wrap = |x: f64| (x + 180.0).rem_euclid(360.0) - 180.0;
        Some(Area { west: wrap(west), south, east: wrap(east), north })
    }

    fn lon_ranges(&self) -> Vec<(f64, f64)> {
        if self.west <= self.east {
            vec![(self.west, self.east)]
        } else {
            vec![(self.west, 180.0), (-180.0, self.east)]
        }
    }

    fn intersects(&self, other: &Area) -> bool {
        self.south <= other.north
            && other.south <= self.north
            && self
                .lon_ranges()
                .iter()
                .any(|(w, e)| other.lon_ranges().iter().any(|(ow, oe)| *w <= *oe && *ow <= *e))
    }

    fn contains(&self, lat: f64, lon: f64) -> bool {
        lat >= self.south
            && lat <= self.north
            && self.lon_ranges().iter().any(|(w, e)| lon >= *w && lon <= *e)
    }

    /// Widened by `share` of its size on every side (for small pans).
    fn padded(&self, share: f64) -> Area {
        let dlat = (self.north - self.south) * share;
        let width = (self.east - self.west).rem_euclid(360.0);
        if width + 2.0 * width * share >= 360.0 || (self.west == -180.0 && self.east == 180.0) {
            return Area { south: (self.south - dlat).max(-90.0), north: (self.north + dlat).min(90.0), ..Area::WORLD };
        }
        let wrap = |x: f64| (x + 180.0).rem_euclid(360.0) - 180.0;
        Area {
            west: wrap(self.west - width * share),
            south: (self.south - dlat).max(-90.0),
            east: wrap(self.east + width * share),
            north: (self.north + dlat).min(90.0),
        }
    }

}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Trafikverket,
}

pub const SOURCES: [Source; 1] = [Source::Trafikverket];

impl Source {
    /// Sweden, and its lines to Oslo, Copenhagen and Narvik (a little
    /// generous at the edges), as `(west, south, east, north)`.
    fn covers(self, view: &Area) -> bool {
        let (west, south, east, north) = (10.5, 55.0, 24.5, 69.5);
        Area { west, south, east, north }.intersects(view)
    }

    fn ttl(self) -> Duration {
        // the trains come live from the streams; while they bring nothing, asked for often
        Duration::from_secs(if crate::trafikverket::streams_live() { 30 } else { 10 })
    }

    /// Display name in errors.
    fn label(self) -> &'static str {
        "Trafikverket"
    }

    /// The cache key and area to ask for a view: the whole network, once for all.
    fn request(self, _view: &Area) -> (String, Area) {
        ("all".into(), Area::WORLD)
    }
}

// ---------------------------------------------------------------- helpers

pub(crate) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or_default()
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Unix ms of an RFC 3339 time: `2026-10-13T06:15:15.000Z`, `2026-10-12T23:11:39-07:00`.
pub fn unix_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let field = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (h, mi, sec) = (field(11..13)?, field(14..16)?, field(17..19)?);
    let mut rest = &s[19..];
    let mut ms = 0;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        let scaled: String = frac[..digits].chars().chain("000".chars()).take(3).collect();
        ms = scaled.parse::<i64>().ok()?;
        rest = &frac[digits..];
    }
    let offset_min = match rest {
        "Z" | "z" | "" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hh = rest.get(1..3)?.parse::<i64>().ok()?;
            let mm = rest.get(rest.len() - 2..)?.parse::<i64>().ok()?;
            sign * (hh * 60 + mm)
        }
    };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset_min * 60;
    Some(secs * 1000 + ms)
}

// ------------------------------------------------------------ fetching

/// Cached answers and politeness bookkeeping, per source.
#[derive(Default)]
struct SourceState {
    entries: HashMap<String, (Instant, Arc<Vec<Train>>)>,
    /// Keys with a background refresh under way.
    refreshing: HashSet<String>,
    next: Option<Instant>,
    blocked_until: Option<Instant>,
    failures: u32,
}

impl SourceState {
    /// A cached answer no older than `STALE_MAX`, and whether it's fresh.
    fn cached(&self, key: &str, ttl: Duration) -> Option<(bool, Arc<Vec<Train>>)> {
        let (at, trains) = self.entries.get(key)?;
        let age = at.elapsed();
        (age < STALE_MAX).then(|| (age < ttl, trains.clone()))
    }

    fn store(&mut self, key: String, trains: Arc<Vec<Train>>) {
        if self.entries.len() >= CACHE_LIMIT
            && !self.entries.contains_key(&key)
            && let Some(oldest) = self.entries.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(key, (Instant::now(), trains));
    }

    fn blocked(&self) -> Option<Duration> {
        let left = self.blocked_until?.saturating_duration_since(Instant::now());
        (!left.is_zero()).then_some(left)
    }

    /// Reserves the next call slot; returns how long to wait for it.
    fn reserve(&mut self) -> Duration {
        let now = Instant::now();
        let at = self.next.filter(|t| *t > now).unwrap_or(now);
        self.next = Some(at + GAP);
        at - now
    }

    fn failed(&mut self, retry_after: Option<Duration>) {
        self.failures = self.failures.saturating_add(1);
        let doubling = BACKOFF_BASE.saturating_mul(1 << self.failures.min(8).saturating_sub(1));
        let wait = retry_after.unwrap_or(doubling).min(BACKOFF_MAX);
        self.blocked_until = Some(Instant::now() + wait);
    }

    fn succeeded(&mut self) {
        self.failures = 0;
        self.blocked_until = None;
    }
}

fn states() -> &'static Mutex<HashMap<Source, SourceState>> {
    static STATES: OnceLock<Mutex<HashMap<Source, SourceState>>> = OnceLock::new();
    STATES.get_or_init(Default::default)
}

fn with_state<T>(source: Source, f: impl FnOnce(&mut SourceState) -> T) -> T {
    let mut states = states().lock().unwrap_or_else(|e| e.into_inner());
    f(states.entry(source).or_default())
}

/// Held for the whole of a fetch: one call per source at a time.
fn gate(source: Source) -> &'static tokio::sync::Mutex<()> {
    static GATES: OnceLock<[tokio::sync::Mutex<()>; SOURCES.len()]> = OnceLock::new();
    let gates = GATES.get_or_init(Default::default);
    &gates[SOURCES.iter().position(|s| *s == source).unwrap_or(0)]
}

pub(crate) fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c);
    }
    let c = fr24::client_builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .gzip(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(CLIENT.get_or_init(|| c))
}

type Failure = (String, Option<Duration>);

/// One call to the source.
pub async fn fetch(source: Source, _area: &Area) -> Result<Vec<Train>, Failure> {
    let http = client().map_err(|e| (e, None))?;
    match source {
        Source::Trafikverket => {
            let key = crate::keys::current().trafikverket;
            crate::trafikverket::trains(http, &key, now_ms()).await.map_err(|e| (e, None))
        }
    }
}

/// Fetches `key` within the source's politeness rules and caches it.
async fn refresh(source: Source, key: String, area: Area) -> Result<Arc<Vec<Train>>, String> {
    let _gate = gate(source).lock().await;
    let ready = with_state(source, |s| {
        if let Some((true, hit)) = s.cached(&key, source.ttl()) {
            return Ok(Ok(hit));
        }
        if let Some(left) = s.blocked() {
            return Ok(Err(format!("{} is resting; retrying in {} s", source.label(), left.as_secs().max(1))));
        }
        Err(s.reserve())
    });
    let wait = match ready {
        Ok(done) => {
            with_state(source, |s| s.refreshing.remove(&key));
            return done;
        }
        Err(wait) => wait,
    };
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
    let result = fetch(source, &area).await;
    with_state(source, |s| {
        s.refreshing.remove(&key);
        match result {
            Ok(mut trains) => {
                s.succeeded();
                // a train the stream moved on since keeps its newer report
                if let Some((_, had)) = s.entries.get(&key) {
                    let newer: HashMap<&str, &Train> = had.iter().map(|t| (t.id.as_str(), t)).collect();
                    for t in &mut trains {
                        if let Some(n) = newer.get(t.id.as_str()).filter(|n| n.position_unix > t.position_unix) {
                            *t = (*n).clone();
                        }
                    }
                }
                let trains = Arc::new(trains);
                s.store(key, trains.clone());
                Ok(trains)
            }
            Err((e, retry_after)) => {
                s.failed(retry_after);
                eprintln!("trains: {e}");
                Err(e)
            }
        }
    })
}

/// The source's trains in `view`: cached, stale while a refresh runs, or fetched now.
async fn from_source(source: Source, view: Area) -> Result<Vec<Train>, String> {
    let (key, area) = source.request(&view);
    let cached = with_state(source, |s| {
        let hit = s.cached(&key, source.ttl());
        if let Some((false, _)) = &hit
            && s.blocked().is_none()
            && s.refreshing.insert(key.clone())
        {
            tauri::async_runtime::spawn(refresh(source, key.clone(), area));
        }
        hit
    });
    let trains = match cached {
        Some((_, trains)) => trains,
        None => refresh(source, key, area).await?,
    };
    let within = view.padded(0.1);
    Ok(trains.iter().filter(|t| within.contains(t.lat, t.lon)).cloned().collect())
}

/// Puts trains a live stream moved into the source's cached answer, in
/// place of their older selves; trains not there yet are added. Whether
/// anything changed. The cache's age is left alone: the regular refresh
/// still comes, for the trains that stopped reporting.
pub fn patch(source: Source, moved: Vec<Train>) -> bool {
    let (key, _) = source.request(&Area::WORLD);
    with_state(source, |s| {
        let Some((at, had)) = s.entries.get(&key) else { return false };
        let at = *at;
        let mut trains: Vec<Train> = had.as_ref().clone();
        let mut index: HashMap<String, usize> = trains.iter().enumerate().map(|(i, t)| (t.id.clone(), i)).collect();
        let mut changed = false;
        for t in moved {
            match index.get(&t.id) {
                Some(&i) if trains[i].position_unix >= t.position_unix => {}
                Some(&i) => {
                    trains[i] = t;
                    changed = true;
                }
                None => {
                    index.insert(t.id.clone(), trains.len());
                    trains.push(t);
                    changed = true;
                }
            }
        }
        if changed {
            s.entries.insert(key, (at, Arc::new(trains)));
        }
        changed
    })
}

/// When the map last asked for Trafikverket's trains, Unix ms.
static TRAFIKVERKET_ASKED: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
/// The operators' live streams run while the map asked for them this recently.
const WANTED_FOR_MS: i64 = 120_000;

/// Whether the map shows Trafikverket's trains: their live stream is worth running.
pub fn trafikverket_wanted() -> bool {
    now_ms() - TRAFIKVERKET_ASKED.load(std::sync::atomic::Ordering::Relaxed) < WANTED_FOR_MS
}

/// The sources worth asking for a view at a deck.gl zoom.
pub fn sources_for(view: &Area, _zoom: f64) -> Vec<Source> {
    let keyed = !crate::keys::current().trafikverket.is_empty();
    SOURCES.into_iter().filter(|s| keyed && s.covers(view)).collect()
}

fn followed() -> &'static Mutex<Option<tauri::async_runtime::JoinHandle<()>>> {
    static F: OnceLock<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>> = OnceLock::new();
    F.get_or_init(Default::default)
}

/// Streams live updates of a train (its position and its stops) until
/// `unfollow_train` or another train is followed. Trafikverket's trains only:
/// the other sources have no stream.
#[tauri::command]
pub fn follow_train(id: String, on_event: tauri::ipc::Channel<crate::trafikverket::TrainEvent>) -> CmdResult<()> {
    let key = crate::keys::current().trafikverket;
    let task = (id.starts_with("trafikverket:") && !key.is_empty())
        .then(|| Ok::<_, String>(tauri::async_runtime::spawn(crate::trafikverket::follow(client()?.clone(), key, id, on_event))))
        .transpose()?;
    if let Some(previous) = std::mem::replace(&mut *followed().lock().unwrap_or_else(|e| e.into_inner()), task) {
        previous.abort();
    }
    Ok(())
}

#[tauri::command]
pub fn unfollow_train() {
    if let Some(task) = followed().lock().unwrap_or_else(|e| e.into_inner()).take() {
        task.abort();
    }
}

/// The whole way of a train along the track, for the sources that have its
/// timetable. Made from the track loaded so far: while `complete` is false,
/// ask again in a moment.
#[tauri::command]
pub async fn train_route(id: String) -> CmdResult<crate::trafikverket::TrainRoute> {
    let key = crate::keys::current().trafikverket;
    // a whole run is a few hundred kilometres of track: off the async threads
    tauri::async_runtime::spawn_blocking(move || crate::trafikverket::route(&id, &key))
        .await
        .map_err(|e| e.to_string())?
}

/// The signal blocks starting in an area (close in: the signals are only
/// mapped at street level), and whether its track and signals are all here.
#[tauri::command]
pub async fn train_blocks(west: f64, south: f64, east: f64, north: f64) -> CmdResult<TrainBlocks> {
    tauri::async_runtime::spawn_blocking(move || {
        let (blocks, complete) = crate::rail::blocks::blocks_in(south, west, north, east);
        TrainBlocks { blocks, complete }
    })
    .await
    .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
pub struct TrainBlocks {
    blocks: Vec<crate::rail::blocks::Block>,
    complete: bool,
}

/// The trains the map shows close up (by id): streamed live, every few
/// seconds, like a followed one. Trafikverket's only; an empty list stops it.
#[tauri::command]
pub fn watch_trains(ids: Vec<String>) {
    let ids: Vec<String> = ids.into_iter().filter(|id| id.starts_with("trafikverket:")).collect();
    crate::trafikverket::watch(&ids);
}

/// The operators the map leaves out (as trains name them; "" for trains
/// whose operator isn't known): their trains aren't streamed live.
#[tauri::command]
pub fn hide_train_operators(names: Vec<String>) {
    crate::trafikverket::hide_operators(names);
}

/// Every stop of a train, for the sources that have its timetable; none for the others.
#[tauri::command]
pub async fn train_stops(id: String) -> CmdResult<Vec<crate::trafikverket::Stop>> {
    if id.starts_with("trafikverket:") {
        let key = crate::keys::current().trafikverket;
        return crate::trafikverket::stops(client()?, &key, &id).await;
    }
    Ok(Vec::new())
}

/// Trains in an area (degrees; `west > east` crosses the antimeridian) at a
/// deck.gl zoom, merged from every regional source covering it. A source
/// that fails is left out (and logged); only when all fail is it an error.
#[tauri::command]
pub async fn trains_in_view(west: f64, south: f64, east: f64, north: f64, zoom: f64) -> CmdResult<Vec<Train>> {
    let view = Area::view(west, south, east, north).ok_or("not an area")?;
    let sources = sources_for(&view, if zoom.is_finite() { zoom } else { 0.0 });
    if sources.contains(&Source::Trafikverket) {
        TRAFIKVERKET_ASKED.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
    }
    let results = futures_util::future::join_all(sources.iter().map(|s| from_source(*s, view))).await;
    let mut trains = Vec::new();
    let mut errors = Vec::new();
    for result in results {
        match result {
            Ok(found) => trains.extend(found),
            Err(e) => errors.push(e),
        }
    }
    if trains.is_empty() && !errors.is_empty() && errors.len() == sources.len() {
        return Err(errors.join("; "));
    }
    // the rest glide on along the track ahead (or their heading) between reports
    let now = now_ms();
    let rail = crate::rail::Rail { class: None };
    for t in trains.iter_mut().filter(|t| t.frames.is_empty()) {
        let report = crate::motion::Fix {
            lat: t.lat,
            lon: t.lon,
            t_ms: t.position_unix * 1000,
            speed: t.speed_kmh.map(|s| f64::from(s) / 3.6),
            heading: t.heading.map(f64::from),
        };
        t.frames = crate::motion::simulate(None, &report, None, &[], now, Some(&rail)).frames;
    }
    Ok(trains)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_times() {
        assert_eq!(unix_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_ms("2026-10-13T06:15:00Z"), Some(1_791_872_100_000));
        assert_eq!(unix_ms("2026-10-13T08:15:00+02:00"), Some(1_791_872_100_000));
        assert_eq!(unix_ms("2026-10-12T23:15:00-07:00"), Some(1_791_872_100_000));
        assert_eq!(unix_ms("2026-10-13T06:15:00.250Z"), Some(1_791_872_100_250));
        assert_eq!(unix_ms("2026-10-13T06:15:00.585246Z"), Some(1_791_872_100_585));
        assert_eq!(unix_ms("2024-02-29T12:00:00Z"), Some(1_709_208_000_000));
        assert_eq!(unix_ms("13 Oct 2026"), None);
        assert_eq!(unix_ms("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn areas_and_coverage() {
        let stockholm = Area::view(17.8, 59.2, 18.3, 59.4).unwrap();
        assert!(Source::Trafikverket.covers(&stockholm));
        let berlin = Area::view(13.0, 52.3, 13.8, 52.7).unwrap();
        assert!(!Source::Trafikverket.covers(&berlin));
        let world = Area::view(-180.0, -85.0, 180.0, 85.0).unwrap();
        assert!(Source::Trafikverket.covers(&world));
        assert_eq!(Source::Trafikverket.request(&stockholm).0, "all");
        // unwrapped longitudes from the map
        let shifted = Area::view(384.5, 60.0, 385.5, 60.4).unwrap();
        assert!((shifted.west - 24.5).abs() < 1e-9);
        assert!(Area::view(f64::NAN, 0.0, 1.0, 1.0).is_none());
        assert!(Area::view(-200.0, -10.0, 200.0, 10.0).unwrap().contains(0.0, 179.0));
        let pacific = Area::view(140.0, 40.0, -140.0, 60.0).unwrap();
        assert!(pacific.contains(50.0, 179.5) && pacific.contains(50.0, -150.0) && !pacific.contains(50.0, 0.0));
    }

    #[test]
    fn streamed_trains_are_patched_in_newest_first() {
        let t = |id: &str, at: i64| Train {
            id: id.into(),
            source: "trafikverket",
            name: id.into(),
            kind: Kind::Regional,
            lat: 59.0,
            lon: 18.0,
            heading: None,
            speed_kmh: None,
            destination: None,
            operator: None,
            delay_minutes: None,
            position_unix: at,
            frames: Vec::new(),
            reports: Vec::new(),
            predicted: Vec::new(),
            cars: None,
            tail: Vec::new(),
        };
        let source = Source::Trafikverket;
        let (key, _) = source.request(&Area::WORLD);
        with_state(source, |s| s.entries.clear());
        assert!(!patch(source, vec![t("a", 10)]), "nothing to patch before the first answer");
        with_state(source, |s| s.store(key.clone(), Arc::new(vec![t("a", 10), t("b", 10)])));
        assert!(!patch(source, vec![t("a", 5)]), "an older report changes nothing");
        assert!(patch(source, vec![t("a", 20), t("c", 20)]));
        let now = with_state(source, |s| s.entries.get(&key).unwrap().1.clone());
        let at = |id: &str| now.iter().find(|x| x.id == id).map(|x| x.position_unix);
        assert_eq!((at("a"), at("b"), at("c")), (Some(20), Some(10), Some(20)));
        with_state(source, |s| s.entries.clear());
    }

    #[test]
    fn caches_and_backs_off() {
        let mut s = SourceState::default();
        let trains = Arc::new(vec![]);
        s.store("a".into(), trains);
        assert!(matches!(s.cached("a", Duration::from_secs(30)), Some((true, _))));
        assert!(matches!(s.cached("a", Duration::ZERO), Some((false, _))), "stale but usable");
        assert!(s.cached("b", Duration::from_secs(30)).is_none());
        for i in 0..40 {
            s.store(format!("k{i}"), Arc::new(vec![]));
        }
        assert!(s.entries.len() <= CACHE_LIMIT);

        assert!(s.reserve().is_zero());
        assert!(s.reserve() > GAP - Duration::from_millis(50));
        s.failed(None);
        let first = s.blocked_until.unwrap();
        s.failed(None);
        assert!(s.blocked_until.unwrap() > first, "backoff grows");
        for _ in 0..20 {
            s.failed(None);
        }
        assert!(s.blocked_until.unwrap() <= Instant::now() + BACKOFF_MAX);
        s.failed(Some(Duration::from_secs(5)));
        assert!(s.blocked().unwrap() <= Duration::from_secs(5));
        s.succeeded();
        assert!(s.blocked().is_none() && s.failures == 0);
    }

    #[test]
    fn serializes_camel_case() {
        let t = Train {
            id: "trafikverket:1".into(),
            source: "trafikverket",
            name: "SJ 1".into(),
            kind: Kind::HighSpeed,
            lat: 59.33,
            lon: 18.06,
            heading: Some(90.0),
            speed_kmh: Some(120.0),
            destination: Some("Göteborg C".into()),
            operator: Some("SJ".into()),
            delay_minutes: Some(3),
            position_unix: 1_791_872_100,
            frames: vec![(59.33, 18.06, 1_791_872_100_000)],
            reports: Vec::new(),
            predicted: Vec::new(),
            cars: Some(6),
            tail: Vec::new(),
        };
        let json = serde_json::to_value(&t).unwrap();
        for key in ["speedKmh", "delayMinutes", "positionUnix", "frames", "heading", "destination", "operator"] {
            assert!(json.get(key).is_some(), "{key}");
        }
        assert_eq!(json["kind"], "highSpeed");
        assert_eq!(json["frames"][0], serde_json::json!([59.33, 18.06, 1_791_872_100_000_i64]));
    }
}
