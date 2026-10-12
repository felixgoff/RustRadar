//! Live ship positions (AIS) from VesselFinder's public map: the same binary
//! feed its own web map reads, asked for the area in view at the zoom in view.
//!
//! The server thins ships out by zoom, so a world view brings a couple of
//! thousand and a harbour close in brings every one there. It is a free map
//! with no key, used politely: one request at a time, at least `GAP` apart,
//! an area already fetched within `REUSE` is answered from memory, and a
//! refusal or failure backs off. Ships are credited to VesselFinder where they
//! are shown.

use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::CmdResult;

const URL: &str = "https://www.vesselfinder.com/api/pub/mp2";
/// The feed answers browsers; a plain client is turned away.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
const REFERER: &str = "https://www.vesselfinder.com/";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
/// An area within the last one fetched, at the same zoom, is answered from it for this long.
const REUSE: Duration = Duration::from_secs(10);
/// Minimum gap between any two requests.
const GAP: Duration = Duration::from_secs(2);
const BACKOFF_BASE: Duration = Duration::from_secs(10);
const BACKOFF_MAX: Duration = Duration::from_secs(600);
/// Coordinates in the feed are degrees times this.
const SCALE: f64 = 600_000.0;
/// The zooms asked for (OpenLayers zooms, 256 px tiles). The feed answers
/// with no ships at all from 17 up, while 16 already has every ship in view
/// with its dimensions, so closer views ask at 16.
const MIN_ZOOM: u8 = 2;
const MAX_ZOOM: u8 = 16;
/// At and below this zoom the site asks for the whole width of the world.
const WORLD_ZOOM: u8 = 3;
/// Above this zoom each ship carries its dimensions and true heading.
const DIMS_ZOOM: u8 = 13;
/// The map's own latitude limits.
const SOUTH_LIMIT: f64 = -85.0;
const NORTH_LIMIT: f64 = 89.0;
/// The site's full-width longitude span, ±179.99°.
const WORLD_LON: i64 = 107_994_000;
/// Each side of the view is widened by this share, so small pans reuse the answer.
const PAD: f64 = 0.1;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ship {
    pub mmsi: u32,
    pub lat: f64,
    pub lon: f64,
    /// The ship's name, or its MMSI when it hasn't broadcast one.
    pub name: String,
    /// VesselFinder's type colour: 2 tug/special, 3 passenger, 4 cargo,
    /// 5 fishing, 6 tanker, 7 SAR/law enforcement, 8 pleasure/sailing,
    /// 0 unknown; others are "other".
    pub kind: u8,
    /// 0 small, 1 medium, 2 large (always medium at zoom 6 and below).
    pub size: u8,
    /// Degrees clockwise from north; none when moored or not reported.
    pub heading: Option<f32>,
    /// The position is old.
    pub old: bool,
    /// Metres from the AIS reference point to bow, stern, port and starboard
    /// (only close in, and only when plausible).
    pub dims: Option<[u16; 4]>,
}

/// Big-endian reads that stop at the end of the buffer instead of panicking.
struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let end = self.at.checked_add(N)?;
        let bytes = self.buf.get(self.at..end)?.try_into().ok()?;
        self.at = end;
        Some(bytes)
    }
    fn i8(&mut self) -> Option<i8> {
        self.take::<1>().map(i8::from_be_bytes)
    }
    fn u16(&mut self) -> Option<u16> {
        self.take::<2>().map(u16::from_be_bytes)
    }
    fn i16(&mut self) -> Option<i16> {
        self.take::<2>().map(i16::from_be_bytes)
    }
    fn i32(&mut self) -> Option<i32> {
        self.take::<4>().map(i32::from_be_bytes)
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        let end = self.at.checked_add(n)?;
        (end <= self.buf.len()).then(|| self.at = end)
    }
    fn done(&self) -> bool {
        self.at >= self.buf.len()
    }
}

/// One record; `None` when the buffer ends inside it.
fn record(r: &mut Reader, zoom: u8, selected: i32) -> Option<Option<Ship>> {
    let w = r.u16()?;
    let mmsi = r.i32()?;
    let lat = f64::from(r.i32()?) / SCALE;
    let lon = f64::from(r.i32()?) / SCALE;
    let is_selected = selected != 0 && mmsi == selected;
    if is_selected {
        r.skip(6)?; // course, speed and a spare
    }
    r.skip(1)?; // td
    let name_len = usize::try_from(r.i8()?).ok()?;
    let end = r.at.checked_add(name_len)?;
    let name_bytes = r.buf.get(r.at..end)?;
    r.at = end;
    if is_selected {
        r.skip(4)?; // timestamp
    }
    let kind = ((w & 0x00F0) >> 4) as u8;
    let icon = ((w & 0x3F00) >> 8) as u8;
    let sar = w & 2 != 0;
    let mut heading = (icon < 32).then(|| f32::from(icon) * 11.25);
    let mut dims = None;
    if zoom > DIMS_ZOOM {
        let [bow, stern, port, stbd, hdg] = [r.i16()?, r.i16()?, r.i16()?, r.i16()?, r.i16()?];
        let (b, s, p, t) = (i32::from(bow), i32::from(stern), i32::from(port), i32::from(stbd));
        if b >= 0 && s >= 0 && p >= 0 && t >= 0 && b + s > 0 && p + t > 0 && b + s <= 500 && p < 63 && t < 63 {
            dims = Some([bow as u16, stern as u16, port as u16, stbd as u16]);
        }
        if (0..=360).contains(&hdg) {
            heading = Some(f32::from(hdg) % 360.0);
        }
    } else if sar {
        r.skip(2)?; // altitude
    }
    if !(lat.abs() <= 90.0 && lon.abs() <= 180.0) || mmsi <= 0 {
        return Some(None);
    }
    let name: String = name_bytes.iter().map(|&b| b as char).collect();
    let name = name.trim();
    Some(Some(Ship {
        mmsi: mmsi as u32,
        lat,
        lon,
        name: if name.is_empty() { mmsi.to_string() } else { name.to_owned() },
        kind,
        size: if zoom > 6 {
            match w & 0xC000 {
                0xC000 => 2,
                0x8000 => 0,
                _ => 1,
            }
        } else {
            1
        },
        heading,
        old: w & 1 != 0,
        dims,
    }))
}

/// Decodes a response asked for at `zoom` (the feed's zoom, which decides
/// what each record carries). Stops at a truncated record.
pub fn parse(buf: &[u8], zoom: u8) -> Vec<Ship> {
    // a header shorter than its own minimum carries no extra fields
    let extra = if buf.len() >= 12 {
        usize::from(u16::from_be_bytes([buf[1], buf[2]]))
    } else {
        0
    };
    let start = 4 + extra;
    let Some(selected) = buf.get(start - 4..start).map(|b| i32::from_be_bytes(b.try_into().unwrap_or_default())) else {
        return Vec::new();
    };
    let mut r = Reader { buf, at: start };
    let mut ships = Vec::new();
    while !r.done() {
        match record(&mut r, zoom, selected) {
            Some(Some(ship)) => ships.push(ship),
            Some(None) => {}
            None => break,
        }
    }
    ships
}

/// The feed's zoom for a deck.gl zoom: deck.gl's tiles are 512 px, the map's 256.
pub fn feed_zoom(deck_zoom: f64) -> u8 {
    if !deck_zoom.is_finite() {
        return MIN_ZOOM;
    }
    (deck_zoom + 1.0).floor().clamp(f64::from(MIN_ZOOM), f64::from(MAX_ZOOM)) as u8
}

/// One request's area, in the feed's integer units: `[west, south, east, north]`.
type Bbox = [i64; 4];

fn units(deg: f64) -> i64 {
    (deg * SCALE).round() as i64
}

/// The areas to ask for: one, or two when the view crosses the antimeridian;
/// the whole width of the world zoomed out.
pub fn areas(west: f64, south: f64, east: f64, north: f64, zoom: u8) -> Result<Vec<Bbox>, String> {
    if ![west, south, east, north].iter().all(|v| v.is_finite()) {
        return Err("not an area".into());
    }
    let (mut south, mut north) = (south.min(north), south.max(north));
    let mut width = if east >= west { east - west } else { east + 360.0 - west };
    if zoom > WORLD_ZOOM {
        let pad_lat = (north - south) * PAD;
        south -= pad_lat;
        north += pad_lat;
        width += 2.0 * width * PAD;
    }
    let south = south.clamp(SOUTH_LIMIT, NORTH_LIMIT);
    let north = north.clamp(SOUTH_LIMIT, NORTH_LIMIT);
    if north <= south {
        return Err("the area is off the map".into());
    }
    let (s, n) = (units(south), units(north));
    if zoom <= WORLD_ZOOM || width >= 359.0 {
        return Ok(vec![[-WORLD_LON, s, WORLD_LON, n]]);
    }
    let centre = west + (if east >= west { east - west } else { east + 360.0 - west }) / 2.0;
    let wrap = |lon: f64| (lon + 540.0).rem_euclid(360.0) - 180.0;
    let (w, e) = (wrap(centre - width / 2.0), wrap(centre + width / 2.0));
    Ok(if w <= e {
        vec![[units(w), s, units(e), n]]
    } else {
        vec![[units(w), s, units(180.0) - 1, n], [units(-180.0), s, units(e), n]]
    })
}

fn url(bbox: &Bbox, zoom: u8, nonce: u32) -> String {
    let [w, s, e, n] = bbox;
    if zoom <= WORLD_ZOOM {
        format!("{URL}?bbox={w},{s},{e},{n}&zoom={zoom}&mmsi=0&mcbe=1")
    } else {
        format!("{URL}?bbox={w},{s},{e},{n}&zoom={zoom}&mmsi=0&ref={}&mcbe=1", nonce % 100_000)
    }
}

/// What was fetched last, and the politeness bookkeeping.
#[derive(Default)]
struct Cache {
    last: Option<Fetched>,
    /// No request before this moment.
    next: Option<Instant>,
    /// No requests before this: the last ones failed or were refused.
    blocked_until: Option<Instant>,
    failures: u32,
}

struct Fetched {
    at: Instant,
    zoom: u8,
    areas: Vec<Bbox>,
    ships: Vec<Ship>,
}

impl Fetched {
    /// Whether this answer already covers `areas` at `zoom`.
    fn covers(&self, zoom: u8, areas: &[Bbox]) -> bool {
        zoom == self.zoom
            && areas.iter().all(|[w, s, e, n]| {
                self.areas
                    .iter()
                    .any(|[fw, fs, fe, fnorth]| fw <= w && fs <= s && fe >= e && fnorth >= n)
            })
    }
}

impl Cache {
    fn fresh(&self, zoom: u8, areas: &[Bbox]) -> Option<Vec<Ship>> {
        let last = self.last.as_ref()?;
        (last.at.elapsed() < REUSE && last.covers(zoom, areas)).then(|| last.ships.clone())
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

    /// Reserves the next request slot; returns how long to wait for it.
    fn reserve(&mut self) -> Duration {
        let now = Instant::now();
        let at = self.next.filter(|t| *t > now).unwrap_or(now);
        self.next = Some(at + GAP);
        at - now
    }
}

fn state() -> &'static tokio::sync::Mutex<Cache> {
    static CACHE: OnceLock<tokio::sync::Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Held for the whole of a fetch: one at a time.
fn in_flight() -> &'static tokio::sync::Mutex<()> {
    static GATE: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    GATE.get_or_init(Default::default)
}

fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c);
    }
    let c = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .gzip(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(CLIENT.get_or_init(|| c))
}

fn nonce() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() ^ d.as_secs() as u32)
        .unwrap_or_default()
}

async fn request(url: &str) -> Result<Vec<u8>, (String, Option<Duration>)> {
    let response = client()
        .map_err(|e| (e, None))?
        .get(url)
        .header("accept", "*/*")
        .header("referer", REFERER)
        .send()
        .await
        .map_err(|e| (format!("can't reach VesselFinder: {e}"), None))?;
    let status = response.status();
    if !status.is_success() {
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let message = if status.as_u16() == 429 {
            "VesselFinder asked us to slow down".to_owned()
        } else {
            format!("VesselFinder answered {status}")
        };
        return Err((message, retry_after));
    }
    let body = response.bytes().await.map_err(|e| (e.to_string(), None))?;
    if body.len() < 4 {
        return Err(("VesselFinder sent an empty answer".into(), None));
    }
    Ok(body.to_vec())
}

/// Ships in an area (degrees; `west > east` crosses the antimeridian) at a
/// deck.gl zoom, as many as VesselFinder shows at that zoom.
#[tauri::command]
pub async fn ships_in_view(west: f64, south: f64, east: f64, north: f64, zoom: f64) -> CmdResult<Vec<Ship>> {
    let zoom = feed_zoom(zoom);
    let areas = areas(west, south, east, north, zoom)?;
    let _gate = in_flight().lock().await;
    let mut ships: Vec<Ship> = Vec::new();
    {
        let cache = state().lock().await;
        if let Some(hit) = cache.fresh(zoom, &areas) {
            return Ok(hit);
        }
        if let Some(until) = cache.blocked_until {
            let left = until.saturating_duration_since(Instant::now());
            if !left.is_zero() {
                return Err(format!("VesselFinder is resting; retrying in {} s", left.as_secs().max(1)));
            }
        }
    }
    for area in &areas {
        let wait = state().lock().await.reserve();
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        match request(&url(area, zoom, nonce())).await {
            Ok(body) => ships.extend(parse(&body, zoom)),
            Err((e, retry_after)) => {
                state().lock().await.failed(retry_after);
                return Err(e);
            }
        }
    }
    // two halves of a view across the antimeridian can both carry a ship on it
    if areas.len() > 1 {
        let mut seen = std::collections::HashSet::new();
        ships.retain(|s| seen.insert(s.mmsi));
    }
    let mut cache = state().lock().await;
    cache.succeeded();
    cache.last = Some(Fetched { at: Instant::now(), zoom, areas, ships: ships.clone() });
    Ok(ships)
}

#[cfg(test)]
mod tests {
    use super::*;

    const Z2: &[u8] = include_bytes!("../tests/data/vesselfinder-z2.bin");
    const Z3: &[u8] = include_bytes!("../tests/data/vesselfinder-z3.bin");
    const Z5: &[u8] = include_bytes!("../tests/data/vesselfinder-z5.bin");
    const Z8: &[u8] = include_bytes!("../tests/data/vesselfinder-z8.bin");
    const Z9: &[u8] = include_bytes!("../tests/data/vesselfinder-z9.bin");
    const Z14: &[u8] = include_bytes!("../tests/data/vesselfinder-z14.bin");

    /// Parses like `parse` but also says where it stopped.
    fn parse_to_end(buf: &[u8], zoom: u8) -> (usize, usize) {
        let start = 4 + usize::from(u16::from_be_bytes([buf[1], buf[2]]));
        let mut r = Reader { buf, at: start };
        let mut n = 0;
        while !r.done() {
            record(&mut r, zoom, 0).expect("record cut short");
            n += 1;
        }
        (n, r.at)
    }

    #[test]
    fn decodes_the_samples_to_the_end() {
        for (buf, zoom, count) in [(Z2, 2, 1295), (Z3, 3, 2139), (Z5, 5, 979), (Z8, 8, 491), (Z9, 9, 347), (Z14, 14, 190)] {
            let (n, end) = parse_to_end(buf, zoom);
            assert_eq!((n, end), (count, buf.len()), "zoom {zoom}");
            let ships = parse(buf, zoom);
            assert!(ships.len() <= count && ships.len() > count * 9 / 10, "zoom {zoom}: {}", ships.len());
            for s in &ships {
                assert!(s.lat.abs() <= 90.0 && s.lon.abs() <= 180.0);
                assert!(!s.name.is_empty());
                assert!(s.heading.is_none_or(|h| (0.0..360.0).contains(&h)));
                assert!(s.size <= 2 && s.kind < 16);
            }
        }
    }

    #[test]
    fn sizes_and_headings() {
        // zoomed out every ship is medium; close in there are all three
        assert!(parse(Z5, 5).iter().all(|s| s.size == 1));
        let z8 = parse(Z8, 8);
        for size in 0..=2 {
            assert!(z8.iter().any(|s| s.size == size), "size {size}");
        }
        let world = parse(Z3, 3);
        let moving = world.iter().filter(|s| s.heading.is_some()).count();
        assert!(moving > 1000 && moving < world.len(), "{moving}");
        // the common types are all there
        for kind in [3, 4, 6, 8] {
            assert!(world.iter().any(|s| s.kind == kind), "kind {kind}");
        }
        assert!(world.iter().all(|s| s.dims.is_none()));
    }

    #[test]
    fn close_in_ships_have_dimensions() {
        let harbour = parse(Z14, 14);
        let sized: Vec<_> = harbour.iter().filter_map(|s| s.dims).collect();
        assert!(sized.len() > harbour.len() / 2, "{} of {}", sized.len(), harbour.len());
        for [bow, stern, port, stbd] in sized {
            assert!(bow + stern > 0 && bow + stern <= 500 && port < 63 && stbd < 63);
        }
        // moored ships have no icon heading, but many report a true heading
        assert!(harbour.iter().filter(|s| s.heading.is_some()).count() > 3);
    }

    #[test]
    fn survives_truncation_and_junk() {
        for cut in [0, 1, 3, 4, 11, 12, 16, 17, 30, Z14.len() / 2, Z14.len() - 1] {
            let ships = parse(&Z14[..cut], 14);
            assert!(ships.len() <= 190);
        }
        // the zoom decides the record layout: the wrong one must not panic
        let _ = parse(Z2, 14);
        let _ = parse(Z14, 2);
        let junk: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let _ = parse(&junk, 9);
        let _ = parse(&junk, 15);
        assert!(parse(&[0x43, 0, 0, 0], 3).is_empty());
    }

    #[test]
    fn zooms_and_areas() {
        assert_eq!(feed_zoom(0.5), 2);
        assert_eq!(feed_zoom(2.9), 3);
        assert_eq!(feed_zoom(13.2), 14);
        assert_eq!(feed_zoom(25.0), 16);
        // the feed is empty from 17 up
        assert_eq!(feed_zoom(16.4), 16);
        assert_eq!(feed_zoom(f64::NAN), 2);

        // zoomed out: the site's full-width form, no nonce
        let world = areas(-30.0, -70.0, 60.0, 80.0, 3).unwrap();
        assert_eq!(world, vec![[-WORLD_LON, units(-70.0), WORLD_LON, units(80.0)]]);
        assert!(!url(&world[0], 3, 7).contains("ref="));
        assert!(url(&world[0], 5, 123_456).contains("&ref=23456&"));

        // a regional view, padded and clamped to the map
        let [[w, s, e, n]] = areas(4.0, 51.0, 5.0, 52.0, 9).unwrap()[..] else { panic!() };
        assert_eq!((w, s, e, n), (units(3.9), units(50.9), units(5.1), units(52.1)));
        let polar = areas(-10.0, 80.0, 10.0, 90.0, 6).unwrap();
        assert_eq!(polar[0][3], units(NORTH_LIMIT));

        // across the antimeridian: two requests, either side of it
        let pacific = areas(170.0, -20.0, -170.0, 0.0, 6).unwrap();
        assert_eq!(pacific.len(), 2);
        assert!(pacific[0][0] < units(170.0) && pacific[0][2] < units(180.0));
        assert!(pacific[1][0] == units(-180.0) && pacific[1][2] > units(-170.0));

        assert!(areas(f64::NAN, 0.0, 1.0, 1.0, 9).is_err());
        assert!(areas(0.0, 89.5, 1.0, 89.9, 9).is_err());
    }

    #[test]
    fn reuses_a_covering_answer() {
        let fetched = Fetched {
            at: Instant::now(),
            zoom: 9,
            areas: areas(4.0, 51.0, 5.0, 52.0, 9).unwrap(),
            ships: Vec::new(),
        };
        assert!(fetched.covers(9, &areas(4.05, 51.05, 4.95, 51.95, 9).unwrap()));
        assert!(!fetched.covers(10, &areas(4.05, 51.05, 4.95, 51.95, 10).unwrap()));
        assert!(!fetched.covers(9, &areas(4.5, 51.0, 5.5, 52.0, 9).unwrap()));
    }

    #[test]
    fn backs_off_after_failures() {
        let mut cache = Cache::default();
        assert!(cache.reserve().is_zero());
        assert!(cache.reserve() >= GAP - Duration::from_millis(50));
        cache.failed(None);
        let first = cache.blocked_until.unwrap();
        cache.failed(None);
        assert!(cache.blocked_until.unwrap() > first);
        cache.failed(Some(Duration::from_secs(5_000)));
        assert!(cache.blocked_until.unwrap() <= Instant::now() + BACKOFF_MAX);
        cache.succeeded();
        assert!(cache.blocked_until.is_none());
    }

    /// One live call; run by hand with `cargo test live_vesselfinder -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_vesselfinder() {
        let ships = ships_in_view(3.9, 51.8, 4.6, 52.1, 10.0).await.unwrap();
        assert!(!ships.is_empty());
    }
}
