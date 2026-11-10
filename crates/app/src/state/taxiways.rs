//! Taxiway networks for the motion model (A11; doc 03 §16; `taxiways.ts`): which airports'
//! OpenStreetMap centrelines are loaded, at most [`MAX_LOADED`], the least recently wanted let go
//! of, and an airport that gave none asked again after [`RETRY_MS`].
//!
//! Nothing here is drawn (the map's own aeroway lines are): a loaded network goes to the motion
//! model ([`Motion::set_taxiways`]), which keeps taxiing aircraft on the lines.
//!
//! Who asks: the aircraft scene builder. At zoom ≥ 12
//! ([`TAXIWAY_ZOOM`](rustradar_scene::aircraft::TAXIWAY_ZOOM)) each scene refresh lists the drawn
//! flights with their pose's phase ([`AircraftScene::taxi_wanted`], `taxiwaysFor(f.id,
//! pose.phase)` in the per-flight loop) and hands the list to [`Taxiways::taxiways_for`] with the
//! motion model and the local clock: an aircraft parked, taxiing or on its take-off or landing
//! roll at an airport ([`Motion::ground_field`]) wants that airport's network
//! ([`Taxiways::want`], `wantTaxiways`):
//! - a 4-character ICAO code (trimmed and upper-cased as JavaScript does) and a source are needed;
//! - a loaded network's last-wanted time is set to now;
//! - otherwise, unless a request for it is out or one was started less than [`RETRY_MS`] ago
//!   (`triedAt`, set when the request starts, whatever it answers), the source is asked for it
//!   (`airportTaxiways(icao, lat, lon)`).
//!
//! Each [`Taxiways::poll`]:
//! 1. the first poll only: where the networks come from ([`TaxiwaySource`]): the `--fixture`
//!    folder's [`FIXTURE_DIR`]`/{ICAO}.json` (the `airport_taxiways` answer, a list of
//!    [`TaxiLine`]s, or the backend's cache file `{fetchedAt, lines}`; read at each request; a
//!    missing file answers as a failed call), else the backend
//!    ([`rustradar_backend::taxiways::airport`] with the backend's cache folder), else none: then
//!    nothing is ever asked, as without Tauri (`isTauri()`). [`Taxiways::with_source`] sets the
//!    source instead (tests, and shots without a fixture folder, which must not reach the
//!    network);
//! 2. the answers that have arrived, in the order asked: a non-empty network goes to the motion
//!    model, is marked loaded at now, and the least recently wanted networks beyond
//!    [`MAX_LOADED`] are let go of (`motion.setTaxiways(code, null)`; ties go to the one loaded
//!    first, as a JavaScript `Map` iterates); an empty or failed answer leaves nothing behind but
//!    its `triedAt`, so it is asked again once [`RETRY_MS`] have passed since it was asked;
//! 3. a network loaded or let go of asks for a scene refresh.
//!
//! Clock: the local clock (`Date.now()`), the scene refresh's for a want, the poll's for an answer.
//! No timer runs here: a retry waits for the next want after [`RETRY_MS`].
//!
//! Deviations, within a frame:
//! - answers are taken at the next poll, in the order they were asked, where the page takes each
//!   as its promise settles; two that arrive between the same two frames are loaded at the same
//!   clock reading, in the order asked;
//! - a network loaded or let go of forces a scene refresh, where today's page waits for the next
//!   paced one (at most 50 ms away at zoom ≥ 12);
//! - the `--fixture` folder is a source of its own (new; today a page without Tauri asks nothing).
//!
//! [`AircraftScene::taxi_wanted`]: rustradar_scene::aircraft::AircraftScene::taxi_wanted

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustradar_backend::taxiways;
use rustradar_core::model::{Airport, TaxiLine};
use rustradar_core::motion::aircraft::{Motion, Phase};

use super::Cx;
use super::aircraft::Answer;
use crate::requests::Requests;

/// Networks kept at once (`MAX_LOADED`).
pub const MAX_LOADED: usize = 6;

/// An airport whose network failed to load, or has none, is asked again after this long, ms
/// (`RETRY_MS`), counted from when it was asked.
pub const RETRY_MS: f64 = 5.0 * 60_000.0;

/// The `--fixture` folder's networks: `DIR/taxiways/{ICAO}.json`, as the backend's cache folder
/// names them.
pub const FIXTURE_DIR: &str = "taxiways";

/// `airport_taxiways`' answer: the airport's centrelines, empty when it has none or they cannot
/// be had.
pub type LinesAnswer = Result<Arc<Vec<TaxiLine>>, String>;

/// Where the networks come from: the backend, the `--fixture` folder, or a test's.
pub trait TaxiwaySource: Send {
    /// `airportTaxiways(icao, lat, lon)`: `icao` is trimmed and upper-cased, four characters.
    fn airport_taxiways(&mut self, icao: &str, lat: f64, lon: f64) -> Answer<LinesAnswer>;
}

/// The backend's `airport_taxiways` (Overpass, cached in memory and under the cache folder), on
/// its runtime.
struct BackendTaxiways {
    /// The backend's cache folder ([`rustradar_backend::Dirs::cache`]).
    cache: PathBuf,
    requests: Requests,
}

impl TaxiwaySource for BackendTaxiways {
    fn airport_taxiways(&mut self, icao: &str, lat: f64, lon: f64) -> Answer<LinesAnswer> {
        let (icao, cache) = (icao.to_owned(), self.cache.clone());
        self.requests.request(async move { taxiways::airport(&icao, lat, lon, &cache).await }).into()
    }
}

/// The `--fixture` folder's [`FIXTURE_DIR`], read when asked (on the main thread: a shot wants the
/// answer at the next frame).
struct FixtureTaxiways {
    dir: PathBuf,
}

/// The fixture file's network for `code`, or why there is none.
fn read_fixture(dir: &Path, code: &str) -> LinesAnswer {
    let path = dir.join(FIXTURE_DIR).join(format!("{code}.json"));
    let error = |e: &dyn std::fmt::Display| format!("fixture {}: {e}", path.display());
    let bytes = std::fs::read(&path).map_err(|e| error(&e))?;
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| error(&e))?;
    // the backend's cache file holds the lines with the time they were fetched
    if let Some(lines) = value.get_mut("lines") {
        value = lines.take();
    }
    serde_json::from_value::<Vec<TaxiLine>>(value).map(Arc::new).map_err(|e| error(&e))
}

impl TaxiwaySource for FixtureTaxiways {
    fn airport_taxiways(&mut self, icao: &str, _lat: f64, _lon: f64) -> Answer<LinesAnswer> {
        Answer::ready(read_fixture(&self.dir, icao))
    }
}

/// Which source the networks come from.
enum Source {
    /// Chosen at the first poll: the fixture folder, else the backend, else none.
    Auto,
    /// None: nothing is asked (the page without Tauri).
    Off,
    On(Box<dyn TaxiwaySource>),
}

/// `String.prototype.trim`: JavaScript's white space and line terminators, which take in U+FEFF
/// and leave out U+0085 (Rust's `str::trim` does the opposite for both).
fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c == '\u{feff}' || (c != '\u{85}' && c.is_whitespace()))
}

/// `airport.icao?.trim().toUpperCase()`, when it is 4 long (`length`: UTF-16 units).
fn code_of(airport: &Airport) -> Option<String> {
    let code = js_trim(&airport.icao).to_uppercase();
    (code.encode_utf16().count() == 4).then_some(code)
}

/// The airports' taxiway networks the motion model holds, the requests out and when each airport
/// was last asked for.
pub struct Taxiways {
    source: Source,
    /// The loaded networks' ICAO codes, in the order they loaded, each with when it was last
    /// wanted (local ms): `loaded`, a JavaScript `Map` (a code wanted again keeps its place).
    loaded: Vec<(String, f64)>,
    /// The requests out, in the order asked, with their answers (`pending`).
    pending: Vec<(String, Answer<LinesAnswer>)>,
    /// When each airport was last asked for (local ms, `triedAt`).
    tried_at: HashMap<String, f64>,
}

impl Default for Taxiways {
    /// The networks from the `--fixture` folder, else the backend, chosen at the first poll.
    fn default() -> Self {
        Taxiways::with(Source::Auto)
    }
}

impl std::fmt::Debug for Taxiways {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let source = match self.source {
            Source::Auto => "auto",
            Source::Off => "off",
            Source::On(_) => "on",
        };
        f.debug_struct("Taxiways")
            .field("source", &source)
            .field("loaded", &self.loaded)
            .field("pending", &self.pending.iter().map(|(code, _)| code).collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Taxiways {
    fn with(source: Source) -> Self {
        Taxiways { source, loaded: Vec::new(), pending: Vec::new(), tried_at: HashMap::new() }
    }

    /// The networks from `source` instead of the fixture folder or the backend; `None`: from
    /// nowhere (nothing is ever asked: a shot without a fixture folder, which must not reach the
    /// network).
    pub fn with_source(source: Option<Box<dyn TaxiwaySource>>) -> Self {
        Taxiways::with(source.map_or(Source::Off, Source::On))
    }

    /// `taxiwaysFor(id, phase)` for each flight a scene refresh drew at zoom ≥ 12, in order
    /// ([`AircraftScene::taxi_wanted`](rustradar_scene::aircraft::AircraftScene::taxi_wanted)), at
    /// the local time `now_ms`: one on the ground (parked, taxiing, rolling) wants the network of
    /// the airport the motion model has it at.
    pub fn taxiways_for(&mut self, wanted: &[(u32, Phase)], motion: &Motion, now_ms: f64) {
        for &(id, phase) in wanted {
            if !phase.is_ground() {
                continue;
            }
            if let Some(airport) = motion.ground_field(id) {
                self.want(airport, now_ms);
            }
        }
    }

    /// `wantTaxiways(airport)` at the local time `now_ms`: loads the airport's network unless it
    /// is loaded (then it was wanted now), asked for, or was asked for less than [`RETRY_MS`] ago.
    /// Cheap to repeat every frame. Before the first poll (no source chosen yet) it does nothing.
    pub fn want(&mut self, airport: &Airport, now_ms: f64) {
        let Some(code) = code_of(airport) else { return };
        let Source::On(source) = &mut self.source else { return };
        if let Some((_, wanted)) = self.loaded.iter_mut().find(|(c, _)| *c == code) {
            *wanted = now_ms;
            return;
        }
        let tried = self.tried_at.get(&code).copied().unwrap_or(f64::NEG_INFINITY);
        if self.pending.iter().any(|(c, _)| *c == code) || now_ms - tried < RETRY_MS {
            return;
        }
        self.tried_at.insert(code.clone(), now_ms);
        let answer = source.airport_taxiways(&code, airport.lat, airport.lon);
        self.pending.push((code, answer));
    }

    /// This frame's step (see the module's documentation, and [`super::AppState::poll`] for the
    /// order).
    pub fn poll(&mut self, cx: &mut Cx<'_>) {
        if matches!(self.source, Source::Auto) {
            self.source = if let Some(dir) = &cx.startup.fixture {
                Source::On(Box::new(FixtureTaxiways { dir: dir.clone() }))
            } else if let (Some(backend), Some(requests)) = (cx.backend, cx.requests) {
                Source::On(Box::new(BackendTaxiways { cache: backend.dirs().cache.clone(), requests: requests.clone() }))
            } else {
                Source::Off
            };
        }
        let now = cx.clocks.local_ms;
        let mut changed = false;
        let mut i = 0;
        while i < self.pending.len() {
            let Some(result) = self.pending[i].1.poll() else {
                i += 1;
                continue;
            };
            let (code, _) = self.pending.remove(i);
            match result {
                Ok(lines) if !lines.is_empty() => {
                    cx.motion.set_taxiways(&code, Some(&lines));
                    self.set_loaded(code, now);
                    self.evict(cx.motion);
                    changed = true;
                }
                // none (offline, Overpass busy, or none mapped): asked again later
                Ok(_) => log::debug!("no taxiways for {code}"),
                Err(e) => log::debug!("taxiways for {code}: {e}"),
            }
        }
        if changed {
            cx.wants.scene = true;
        }
    }

    /// `loaded.set(code, ms)`: a new code goes last.
    fn set_loaded(&mut self, code: String, ms: f64) {
        match self.loaded.iter_mut().find(|(c, _)| *c == code) {
            Some((_, wanted)) => *wanted = ms,
            None => self.loaded.push((code, ms)),
        }
    }

    /// `evict()`: lets go of the least recently wanted networks beyond [`MAX_LOADED`].
    fn evict(&mut self, motion: &mut Motion) {
        while self.loaded.len() > MAX_LOADED {
            let mut oldest: Option<usize> = None;
            let mut oldest_ms = f64::INFINITY;
            for (i, (_, ms)) in self.loaded.iter().enumerate() {
                if *ms < oldest_ms {
                    oldest_ms = *ms;
                    oldest = Some(i);
                }
            }
            let Some(i) = oldest else { return };
            let (code, _) = self.loaded.remove(i);
            motion.set_taxiways(&code, None);
        }
    }

    /// The loaded networks' ICAO codes, in the order they loaded.
    pub fn loaded(&self) -> impl Iterator<Item = &str> + '_ {
        self.loaded.iter().map(|(code, _)| code.as_str())
    }

    /// Whether the network of `code` (trimmed, upper case) is loaded.
    pub fn is_loaded(&self, code: &str) -> bool {
        self.loaded.iter().any(|(c, _)| c == code)
    }

    /// When the loaded network of `code` was last wanted (local ms).
    pub fn last_wanted(&self, code: &str) -> Option<f64> {
        self.loaded.iter().find(|(c, _)| c == code).map(|&(_, ms)| ms)
    }

    /// Whether a request for `code` is out.
    pub fn is_pending(&self, code: &str) -> bool {
        self.pending.iter().any(|(c, _)| c == code)
    }

    /// Requests out and not yet answered.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// When `code` was last asked for (local ms).
    pub fn tried_at(&self, code: &str) -> Option<f64> {
        self.tried_at.get(code).copied()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use rustradar_core::model::{BoundingBox, LiveFlight, LiveSnapshot, SessionInfo, TaxiKind, TopFlight};
    use rustradar_core::settings::Persisted;
    use rustradar_scene::camera::ViewState;
    use rustradar_scene::controller::Controller;

    use super::*;
    use crate::requests::{self, Requests};
    use crate::scene_builder::SceneBuilder;
    use crate::state::aircraft::{Feeds, LoadedReference};
    use crate::state::{AppState, Clocks, Startup, Wants, aircraft};

    const T0: f64 = 1.7e12;

    /// The calls a [`Canned`] source was asked, each with where its answer goes.
    type Calls = Arc<Mutex<Vec<(String, [f64; 2], Sender<LinesAnswer>)>>>;

    /// A fake backend: every call waits until the test answers it.
    #[derive(Clone, Default)]
    struct Canned(Calls);

    impl TaxiwaySource for Canned {
        fn airport_taxiways(&mut self, icao: &str, lat: f64, lon: f64) -> Answer<LinesAnswer> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().push((icao.to_owned(), [lat, lon], tx));
            rx.into()
        }
    }

    impl Canned {
        fn taxiways(&self) -> Taxiways {
            Taxiways::with_source(Some(Box::new(self.clone())))
        }

        fn asked(&self) -> Vec<String> {
            self.0.lock().unwrap().iter().map(|(code, _, _)| code.clone()).collect()
        }

        fn answer(&self, i: usize, answer: LinesAnswer) {
            self.0.lock().unwrap()[i].2.send(answer).unwrap();
        }
    }

    /// What a poll reads besides the taxiways.
    struct World {
        controller: Controller,
        motion: Motion,
        persisted: Persisted,
        startup: Startup,
    }

    impl World {
        fn new(startup: Startup) -> Self {
            World {
                controller: Controller::new(ViewState::INITIAL, [1440.0, 900.0]),
                motion: Motion::new(),
                persisted: Persisted::default(),
                startup,
            }
        }

        fn poll(&mut self, taxiways: &mut Taxiways, now: f64) -> Wants {
            let mut wants = Wants::default();
            let mut cx = Cx {
                clocks: Clocks { local_ms: now, feed_ms: now },
                controller: &mut self.controller,
                motion: &mut self.motion,
                persisted: &mut self.persisted,
                backend: None,
                requests: None,
                startup: &self.startup,
                events: &[],
                wants: &mut wants,
            };
            taxiways.poll(&mut cx);
            wants
        }
    }

    fn airport(icao: &str, lat: f64, lon: f64) -> Airport {
        Airport { icao: icao.into(), lat, lon, ..Airport::default() }
    }

    /// Seven airports with their reference points.
    const AIRPORTS: [(&str, f64, f64); 7] = [
        ("ESSA", 59.6519, 17.9186),
        ("EKCH", 55.6181, 12.6561),
        ("ENGM", 60.1939, 11.1004),
        ("EFHK", 60.3172, 24.9633),
        ("EGLL", 51.4700, -0.4543),
        ("EDDF", 50.0379, 8.5622),
        ("LFPG", 49.0097, 2.5479),
    ];

    fn nth(i: usize) -> Airport {
        let (code, lat, lon) = AIRPORTS[i];
        airport(code, lat, lon)
    }

    /// A one-segment taxiway at `[lat, lon]`.
    fn lines([lat, lon]: [f64; 2]) -> Arc<Vec<TaxiLine>> {
        Arc::new(vec![TaxiLine { kind: TaxiKind::Taxiway, r#ref: Some("A".into()), points: vec![[lon, lat], [lon + 0.002, lat]] }])
    }

    /// Wants airport `i` at `now`, answers it with a network and takes the answer.
    fn load(taxiways: &mut Taxiways, canned: &Canned, world: &mut World, i: usize, now: f64) -> Wants {
        let calls = canned.asked().len();
        taxiways.want(&nth(i), now);
        assert_eq!(canned.asked().len(), calls + 1, "{} is asked for", AIRPORTS[i].0);
        canned.answer(calls, Ok(lines([AIRPORTS[i].1, AIRPORTS[i].2])));
        world.poll(taxiways, now + 40.0)
    }

    fn codes(range: std::ops::Range<usize>) -> Vec<&'static str> {
        AIRPORTS[range].iter().map(|a| a.0).collect()
    }

    #[test]
    fn the_seventh_airport_evicts_the_first() {
        let canned = Canned::default();
        let mut taxiways = canned.taxiways();
        let mut world = World::new(Startup::default());
        for i in 0..6 {
            let wants = load(&mut taxiways, &canned, &mut world, i, T0 + i as f64 * 1000.0);
            assert!(wants.scene, "a network arrived: the scene is refreshed");
        }
        assert_eq!(taxiways.loaded().collect::<Vec<_>>(), codes(0..6));
        assert!(AIRPORTS[..6].iter().all(|a| world.motion.has_taxiways(a.0)), "six networks fit");
        assert_eq!(canned.asked(), codes(0..6));
        // the answer was taken at the poll after the want: loaded then
        assert_eq!(taxiways.last_wanted("ESSA"), Some(T0 + 40.0));
        load(&mut taxiways, &canned, &mut world, 6, T0 + 6000.0);
        assert_eq!(taxiways.loaded().collect::<Vec<_>>(), codes(1..7));
        assert!(!taxiways.is_loaded("ESSA"));
        assert!(!world.motion.has_taxiways("ESSA"), "the motion model let go of the first network");
        assert!(AIRPORTS[1..].iter().all(|a| world.motion.has_taxiways(a.0)));
        assert_eq!(taxiways.pending_len(), 0);
    }

    #[test]
    fn the_least_recently_wanted_airport_is_let_go_of() {
        let canned = Canned::default();
        let mut taxiways = canned.taxiways();
        let mut world = World::new(Startup::default());
        for i in 0..6 {
            load(&mut taxiways, &canned, &mut world, i, T0 + i as f64 * 1000.0);
        }
        // Arlanda's traffic is watched again: wanted now, and not asked for again
        taxiways.want(&nth(0), T0 + 5500.0);
        assert_eq!(canned.asked().len(), 6, "a loaded network is not asked for");
        assert_eq!(taxiways.last_wanted("ESSA"), Some(T0 + 5500.0));
        let wants = world.poll(&mut taxiways, T0 + 5516.0);
        assert!(!wants.scene, "nothing arrived, nothing to refresh");
        load(&mut taxiways, &canned, &mut world, 6, T0 + 6000.0);
        // Copenhagen was wanted longest ago; Arlanda keeps its place in the load order
        assert_eq!(taxiways.loaded().collect::<Vec<_>>(), ["ESSA", "ENGM", "EFHK", "EGLL", "EDDF", "LFPG"]);
        assert!(world.motion.has_taxiways("ESSA") && !world.motion.has_taxiways("EKCH"));
        // let go of, Copenhagen waits out its retry from when it was asked: 5 min after T0 + 1000
        taxiways.want(&nth(1), T0 + 7000.0);
        assert_eq!(canned.asked().len(), 7);
        taxiways.want(&nth(1), T0 + 1000.0 + RETRY_MS);
        assert_eq!(canned.asked().len(), 8);
        assert_eq!(canned.asked()[7], "EKCH");
    }

    #[test]
    fn ties_go_to_the_network_loaded_first() {
        let canned = Canned::default();
        let mut taxiways = canned.taxiways();
        let mut world = World::new(Startup::default());
        // all seven asked in one scene refresh, all answered before the next frame
        for i in 0..7 {
            taxiways.want(&nth(i), T0);
        }
        assert_eq!(canned.asked(), codes(0..7));
        // answered out of order: still loaded in the order asked, at one clock reading
        for i in (0..7).rev() {
            canned.answer(i, Ok(lines([AIRPORTS[i].1, AIRPORTS[i].2])));
        }
        world.poll(&mut taxiways, T0 + 30.0);
        assert_eq!(taxiways.loaded().collect::<Vec<_>>(), codes(1..7));
        assert!(!world.motion.has_taxiways("ESSA"));
    }

    #[test]
    fn a_failed_load_is_retried_after_5_min() {
        let canned = Canned::default();
        let mut taxiways = canned.taxiways();
        let mut world = World::new(Startup::default());
        taxiways.want(&nth(0), T0);
        assert_eq!(canned.asked(), ["ESSA"]);
        assert!(taxiways.is_pending("ESSA"));
        assert_eq!(taxiways.tried_at("ESSA"), Some(T0));
        // the arguments: the code and the airport's reference point
        assert_eq!(canned.0.lock().unwrap()[0].1, [59.6519, 17.9186]);
        canned.answer(0, Err("couldn't download taxiways: Overpass busy".into()));
        let wants = world.poll(&mut taxiways, T0 + 2000.0);
        assert!(!wants.scene);
        assert!(!taxiways.is_loaded("ESSA") && !taxiways.is_pending("ESSA"));
        assert!(!world.motion.has_taxiways("ESSA"));
        // not again until 5 min after it was asked
        for t in [T0 + 2016.0, T0 + 60_000.0, T0 + RETRY_MS - 1.0] {
            taxiways.want(&nth(0), t);
        }
        assert_eq!(canned.asked().len(), 1);
        taxiways.want(&nth(0), T0 + RETRY_MS);
        assert_eq!(canned.asked(), ["ESSA", "ESSA"]);
        assert_eq!(taxiways.tried_at("ESSA"), Some(T0 + RETRY_MS));
        canned.answer(1, Ok(lines([59.65, 17.92])));
        assert!(world.poll(&mut taxiways, T0 + RETRY_MS + 16.0).scene);
        assert!(taxiways.is_loaded("ESSA") && world.motion.has_taxiways("ESSA"));
    }

    #[test]
    fn an_airport_with_no_network_is_asked_again_after_5_min() {
        let canned = Canned::default();
        let mut taxiways = canned.taxiways();
        let mut world = World::new(Startup::default());
        // the backend answers a failure as an empty network
        taxiways.want(&nth(1), T0);
        canned.answer(0, Ok(Arc::new(Vec::new())));
        assert!(!world.poll(&mut taxiways, T0 + 16.0).scene);
        assert!(!taxiways.is_loaded("EKCH") && !world.motion.has_taxiways("EKCH"));
        taxiways.want(&nth(1), T0 + RETRY_MS - 0.5);
        assert_eq!(canned.asked().len(), 1);
        taxiways.want(&nth(1), T0 + RETRY_MS);
        assert_eq!(canned.asked(), ["EKCH", "EKCH"]);
    }

    #[test]
    fn an_airport_is_asked_for_once_while_its_request_is_out() {
        let canned = Canned::default();
        let mut taxiways = canned.taxiways();
        let mut world = World::new(Startup::default());
        for t in [T0, T0 + 16.0, T0 + RETRY_MS + 1.0] {
            taxiways.want(&nth(0), t);
            world.poll(&mut taxiways, t + 1.0);
        }
        assert_eq!(canned.asked(), ["ESSA"], "pending: not asked again, even past the retry");
        canned.answer(0, Ok(lines([59.65, 17.92])));
        world.poll(&mut taxiways, T0 + RETRY_MS + 20.0);
        assert!(taxiways.is_loaded("ESSA"));
        taxiways.want(&nth(0), T0 + 2.0 * RETRY_MS);
        assert_eq!(canned.asked().len(), 1, "loaded: not asked again");
        assert_eq!(taxiways.last_wanted("ESSA"), Some(T0 + 2.0 * RETRY_MS));
    }

    #[test]
    fn a_code_needs_four_characters_and_a_source() {
        let canned = Canned::default();
        let mut taxiways = canned.taxiways();
        for icao in ["", "ESS", "ESSAX", "   "] {
            taxiways.want(&airport(icao, 59.65, 17.92), T0);
        }
        assert!(canned.asked().is_empty());
        assert!(taxiways.tried_at("ESS").is_none());
        // trimmed as JavaScript trims (the byte-order mark too) and upper-cased
        taxiways.want(&airport(" essa\n", 59.65, 17.92), T0);
        taxiways.want(&airport("\u{feff}ekch", 55.62, 12.66), T0);
        assert_eq!(canned.asked(), ["ESSA", "EKCH"]);
        // U+0085 is no white space to JavaScript: five characters long, never asked
        taxiways.want(&airport("ENGM\u{85}", 60.19, 11.10), T0);
        assert_eq!(canned.asked().len(), 2);

        // no source (the page without Tauri): nothing is asked, nothing remembered
        let mut off = Taxiways::with_source(None);
        off.want(&nth(0), T0);
        assert!(off.tried_at("ESSA").is_none() && off.pending_len() == 0);
        let mut world = World::new(Startup::default());
        world.poll(&mut off, T0);
        off.want(&nth(0), T0);
        assert!(off.tried_at("ESSA").is_none());
        // the default chooses at its first poll: without a fixture folder or a backend, none
        let mut auto = Taxiways::default();
        auto.want(&nth(0), T0);
        assert!(auto.tried_at("ESSA").is_none(), "no source before the first poll");
        world.poll(&mut auto, T0);
        auto.want(&nth(0), T0 + 16.0);
        assert!(auto.tried_at("ESSA").is_none() && auto.pending_len() == 0);
    }

    /// A report of flight `id` at `[lat, lon]`, `alt` ft and `speed` kt, two seconds before `now`.
    fn report(id: u32, [lat, lon]: [f64; 2], alt: i32, speed: i32, on_ground: bool, now: f64) -> LiveFlight {
        LiveFlight {
            id,
            lat: lat as f32,
            lon: lon as f32,
            track: 10,
            alt,
            speed,
            on_ground,
            timestamp_ms: (now - 2000.0) as u64,
            ..LiveFlight::default()
        }
    }

    #[test]
    fn aircraft_on_the_ground_want_their_airports_network() {
        let canned = Canned::default();
        let mut taxiways = canned.taxiways();
        let mut motion = Motion::new();
        let essa = Arc::new(Airport { iata: "ARN".into(), ..nth(0) });
        let ekch = Arc::new(Airport { iata: "CPH".into(), ..nth(1) });
        motion.set_airports(&[essa, ekch]);
        let parked = report(1, [59.6495, 17.925], 0, 0, true, T0);
        let taxiing = report(2, [59.6505, 17.930], 0, 12, true, T0);
        let cruising = report(3, [59.70, 17.95], 35_000, 450, false, T0);
        let kastrup = report(4, [55.6200, 12.6500], 0, 0, true, T0);
        let wanted: Vec<(u32, Phase)> =
            [&parked, &taxiing, &cruising, &kastrup].into_iter().map(|f| (f.id, motion.pose(f, T0).phase)).collect();
        assert!(wanted[0].1.is_ground() && wanted[1].1.is_ground() && wanted[3].1.is_ground(), "{wanted:?}");
        assert!(!wanted[2].1.is_ground(), "{wanted:?}");
        assert_eq!(motion.ground_field(1).map(|a| a.icao.as_str()), Some("ESSA"));
        taxiways.taxiways_for(&wanted, &motion, T0);
        // Arlanda once for its two, then Kastrup; nothing for the airliner overhead
        assert_eq!(canned.asked(), ["ESSA", "EKCH"]);
        // a ground phase for a flight the motion model does not know: no airport, nothing asked
        taxiways.taxiways_for(&[(99, Phase::Taxi)], &motion, T0 + 16.0);
        // a flight at its field but drawn airborne asks nothing
        taxiways.taxiways_for(&[(1, Phase::Approach)], &motion, T0 + 32.0);
        assert_eq!(canned.asked().len(), 2);
    }

    /// A snapshot, answered at once (as `scene_builder::aircraft`'s tests feed theirs).
    struct Synthetic(LiveSnapshot);

    impl Feeds for Synthetic {
        fn live_flights(&mut self, _area: Option<BoundingBox>, _categories: Option<Vec<u8>>) -> Answer<Result<LiveSnapshot, String>> {
            Answer::ready(Ok(self.0.clone()))
        }

        fn top_flights(&mut self) -> Answer<Result<Vec<TopFlight>, String>> {
            Answer::ready(Ok(Vec::new()))
        }

        fn reference_data(&mut self) -> Answer<Result<LoadedReference, String>> {
            Answer::ready(Err("no reference data in this test".into()))
        }

        fn session_info(&mut self) -> Option<SessionInfo> {
            None
        }
    }

    #[test]
    fn the_scene_refresh_at_arlanda_loads_its_network_into_the_motion_model() {
        // V4: Arlanda, flat map, pitched; four aircraft on its aprons (parked or taxiing, as the
        // stage-4 synthetic snapshot has them) and one climbing out
        let view = ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 13.5, bearing: 30.0, pitch: 45.0, position: None };
        let now = 1_782_036_000_000.0;
        let flights = vec![
            report(1, [59.6500, 17.915], 0, 0, true, now),
            report(2, [59.6512, 17.921], 0, 11, true, now),
            report(3, [59.6531, 17.918], 0, 0, true, now),
            report(4, [59.6548, 17.926], 0, 9, true, now),
            report(5, [59.6600, 17.925], 1_800, 160, false, now),
        ];
        let snapshot = LiveSnapshot {
            flights,
            server_time_ms: now as u64,
            requests: 1,
            errors: Vec::new(),
            elapsed_ms: 120,
            authenticated: false,
        };
        let mut state = AppState::new(Startup::default(), None, None, None, view, [1100.0, 720.0]);
        state.aircraft = aircraft::Aircraft::with_feeds(Some(Box::new(Synthetic(snapshot))));
        let canned = Canned::default();
        state.taxiways = canned.taxiways();
        // the reference data's Arlanda, as the aircraft feed hands it to the motion model
        state.motion.set_airports(&[Arc::new(Airport { iata: "ARN".into(), ..nth(0) })]);
        state.poll(now);
        let clocks = state.clocks(now);
        let mut builder = SceneBuilder::default();
        builder.build(&mut state, clocks, 1.0);
        let wanted = std::mem::take(&mut builder.aircraft.latest.taxi_wanted);
        assert_eq!(wanted.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [1, 2, 3, 4, 5], "every aircraft drawn");
        let on_ground: Vec<u32> = wanted.iter().filter(|(_, phase)| phase.is_ground()).map(|(id, _)| *id).collect();
        assert_eq!(on_ground, [1, 2, 3, 4], "{wanted:?}");
        state.taxiways.taxiways_for(&wanted, &state.motion, now);
        assert_eq!(canned.asked(), ["ESSA"], "one request for the four");
        canned.answer(0, Ok(lines([59.6495, 17.92])));
        let wants = state.poll(now + 16.0);
        assert!(wants.scene);
        assert!(state.motion.has_taxiways("ESSA"));
        assert_eq!(state.taxiways.loaded().collect::<Vec<_>>(), ["ESSA"]);
    }

    /// A scratch folder for one test, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("rustradar-a11-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join(FIXTURE_DIR)).unwrap();
            Scratch(dir)
        }

        fn write(&self, code: &str, json: &serde_json::Value) {
            std::fs::write(self.0.join(FIXTURE_DIR).join(format!("{code}.json")), serde_json::to_vec(json).unwrap()).unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_fixture_folder_answers_by_icao_code() {
        let scratch = Scratch::new("fixture");
        let line = serde_json::json!({"kind": "taxiway", "ref": "Y", "points": [[17.92, 59.65], [17.93, 59.65]]});
        // the answer's form, and the backend's cache file's
        scratch.write("ESSA", &serde_json::json!([line]));
        scratch.write("EKCH", &serde_json::json!({"fetchedAt": 1_782_000_000, "lines": [line, line]}));
        scratch.write("ENGM", &serde_json::json!([]));
        let mut world = World::new(Startup { fixture: Some(scratch.0.clone()), ..Default::default() });
        let mut taxiways = Taxiways::default();
        world.poll(&mut taxiways, T0);
        for i in 0..4 {
            taxiways.want(&nth(i), T0 + 10.0);
        }
        // read when asked, taken at the next poll
        assert_eq!(taxiways.pending_len(), 4);
        assert!(world.poll(&mut taxiways, T0 + 26.0).scene);
        assert_eq!(taxiways.loaded().collect::<Vec<_>>(), ["ESSA", "EKCH"]);
        assert!(world.motion.has_taxiways("ESSA") && world.motion.has_taxiways("EKCH"));
        // an empty file and a missing one (Helsinki) load nothing, and wait out the retry
        assert!(!world.motion.has_taxiways("ENGM") && !world.motion.has_taxiways("EFHK"));
        assert_eq!(taxiways.pending_len(), 0);
        scratch.write("EFHK", &serde_json::json!([line]));
        taxiways.want(&nth(3), T0 + 60_000.0);
        assert_eq!(taxiways.pending_len(), 0);
        taxiways.want(&nth(3), T0 + 10.0 + RETRY_MS);
        world.poll(&mut taxiways, T0 + 26.0 + RETRY_MS);
        assert!(world.motion.has_taxiways("EFHK"));
        assert!(read_fixture(&scratch.0, "EGLL").unwrap_err().contains("EGLL.json"));
    }

    #[test]
    fn the_backend_answers_from_its_cache_folder() {
        // a fresh copy in the cache folder: answered without the network
        let scratch = Scratch::new("backend");
        let fetched_at = (crate::state::system_now_ms() / 1000.0) as i64;
        // a code no other test asks the backend for (its answers are kept for the process)
        scratch.write(
            "ZQTX",
            &serde_json::json!({"fetchedAt": fetched_at, "lines": [{"kind": "taxilane", "points": [[10.0, 45.0], [10.01, 45.0]]}]}),
        );
        let rt = requests::runtime().expect("runtime");
        let mut source = BackendTaxiways { cache: scratch.0.clone(), requests: Requests::new(rt.handle().clone(), requests::no_wake()) };
        let answer = source.airport_taxiways("ZQTX", 45.0, 10.0);
        let started = Instant::now();
        let lines = loop {
            if let Some(lines) = answer.poll() {
                break lines;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "no answer from the backend");
            std::thread::sleep(Duration::from_millis(1));
        };
        let lines = lines.expect("the cached network");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].kind, TaxiKind::Taxilane);
        assert_eq!(lines[0].points, [[10.0, 45.0], [10.01, 45.0]]);
    }
}
