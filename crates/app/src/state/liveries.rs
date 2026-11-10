//! Airline livery colours (A2; doc 03 §17; `liveries.ts`): the curated schemes, the air forces'
//! grey and, for every other airline, colours from its logo, asked of the backend's
//! `airline_colors` in batches.
//!
//! The rules are `core::liveries` ([`rustradar_core::liveries::Liveries`]); this part runs its
//! timer and its requests. The scene builder asks [`Liveries::for_flight`] for each aircraft drawn
//! with airline colours (zoom above 6) and reads [`Liveries::version`] to know when colours it
//! drew from the default have arrived (deck's `updateTriggers: [liveryAmount, liveries.version]`).
//! [`Liveries::for_flight`] takes `&self` (the builders get `&AppState`): an airline seen for the
//! first time is queued through a `RefCell`, on the main thread.
//!
//! Each [`Liveries::poll`]:
//! 1. the first poll only: where the colours come from ([`ColorSource`]): the `--fixture`
//!    folder's [`FIXTURE_COLORS`] (`airline_colors`' wire JSON, read at each request; a missing
//!    file answers as a failed call), else the backend ([`Backend::airline_colors`]), else
//!    nothing: then every batch fails at once, as `invoke` does without Tauri, and nothing is
//!    cached, so a later sighting asks again ([`Liveries::with_source`] sets the source instead:
//!    tests, and shots without a fixture folder, which must not reach the network);
//! 2. the batch timer (`setTimeout(flush, 400)`): when due, up to 40 queued IATA codes, the
//!    earliest first, go out in one call;
//! 3. the answers that have arrived: colours (or none) cached per code and the version bumped,
//!    which asks for a scene refresh; a failed call caches nothing;
//! 4. the timer's deadline as the frame loop's [`Wants::wake_at`](super::Wants).
//!
//! Clock: the local clock (`setTimeout`). A code queued by the scene builder starts its timer
//! there; the next poll reports the deadline. Deviations, within a frame: the timer fires at the
//! first poll after it is due (each frame; the scene refreshes every 50–150 ms where liveries
//! show), not on its own; a version bump forces a scene refresh where today's page waits for
//! the next paced one.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rustradar_backend::Backend;
use rustradar_core::liveries::{self as core_liveries, Livery};
use rustradar_core::model::{LiveFlight, LogoColors};
use rustradar_core::reference::Reference;

use super::Cx;
use super::aircraft::Answer;
use crate::requests::Requests;

/// The `--fixture` folder's logo colours: `airline_colors`' answer, IATA code to
/// `{primary, secondary}` or `null`.
pub const FIXTURE_COLORS: &str = "airline_colors.json";

/// `airline_colors`' answer: logo colours (or none) per IATA code asked for.
pub type ColorsAnswer = Result<HashMap<String, Option<LogoColors>>, String>;

/// Where the logo colours come from: the backend, the `--fixture` folder, or a test's.
pub trait ColorSource: Send {
    /// `airlineColors(iatas)`.
    fn airline_colors(&mut self, iatas: Vec<String>) -> Answer<ColorsAnswer>;
}

/// The backend's `airline_colors`, on its runtime.
struct BackendColors {
    backend: Backend,
    requests: Requests,
}

impl ColorSource for BackendColors {
    fn airline_colors(&mut self, iatas: Vec<String>) -> Answer<ColorsAnswer> {
        let backend = self.backend.clone();
        self.requests.request(async move { backend.airline_colors(iatas).await }).into()
    }
}

/// The `--fixture` folder's [`FIXTURE_COLORS`], read when asked (on the main thread: a shot wants
/// the answer in the frame that asked).
struct FixtureColors {
    dir: PathBuf,
}

/// The fixture file's answer, or why there is none.
fn read_fixture(dir: &Path) -> ColorsAnswer {
    let path = dir.join(FIXTURE_COLORS);
    let bytes = std::fs::read(&path).map_err(|e| format!("fixture {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("fixture {}: {e}", path.display()))
}

impl ColorSource for FixtureColors {
    fn airline_colors(&mut self, _iatas: Vec<String>) -> Answer<ColorsAnswer> {
        Answer::ready(read_fixture(&self.dir))
    }
}

/// Which source the liveries use.
enum Source {
    /// Chosen at the first poll: the fixture folder, else the backend, else none.
    Auto,
    /// None: every call fails (the page without Tauri).
    Off,
    On(Box<dyn ColorSource>),
}

/// The airlines' liveries: `core`'s rules, the batch timer and the calls out.
pub struct Liveries {
    /// `core`'s state, queued into by [`Self::for_flight`] through `&self`.
    inner: RefCell<core_liveries::Liveries>,
    source: Source,
    /// The calls out: each batch and its answer.
    out: Vec<(Vec<String>, Answer<ColorsAnswer>)>,
    /// The version the last poll saw (a change asks for a scene refresh).
    seen_version: u64,
}

impl Default for Liveries {
    fn default() -> Self {
        Liveries::with(Source::Auto)
    }
}

impl std::fmt::Debug for Liveries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let source = match self.source {
            Source::Auto => "auto",
            Source::Off => "off",
            Source::On(_) => "on",
        };
        f.debug_struct("Liveries")
            .field("source", &source)
            .field("calls_out", &self.out.len())
            .field("version", &self.version())
            .finish_non_exhaustive()
    }
}

/// `core::liveries` counts whole milliseconds.
fn ms(local_ms: f64) -> i64 {
    local_ms.floor() as i64
}

impl Liveries {
    fn with(source: Source) -> Self {
        Liveries { inner: RefCell::new(core_liveries::Liveries::new()), source, out: Vec::new(), seen_version: 0 }
    }

    /// Logo colours from `source` instead of the fixture folder or the backend; `None`: from
    /// nowhere (every batch fails at once; a shot without a fixture folder, which must not reach
    /// the network).
    pub fn with_source(source: Option<Box<dyn ColorSource>>) -> Self {
        Liveries::with(source.map_or(Source::Off, Source::On))
    }

    /// The livery `f` is drawn in now (`liveries.forFlight(f)`), at the local time `now_ms`:
    /// curated, military, from its airline's logo, or the default while that is not known yet
    /// (then its IATA code is queued for the next batch). `reference` is the reference data
    /// (`state::aircraft`'s), where the airline's IATA code is looked up by its ICAO designator;
    /// without it the code comes from the flight number.
    pub fn for_flight(&self, f: &LiveFlight, reference: Option<&Reference>, now_ms: f64) -> Livery {
        let iata = |icao: &str| reference.and_then(|r| r.airline(Some(icao))).map(|a| a.iata.as_str());
        self.inner.borrow_mut().for_flight(f, iata, ms(now_ms))
    }

    /// Bumped whenever logo colours arrive (`liveries.version`): colours drawn from the default
    /// before it changed may be stale.
    pub fn version(&self) -> u64 {
        self.inner.borrow().version()
    }

    /// When the batch timer fires (local Unix ms), if it runs.
    pub fn deadline(&self) -> Option<f64> {
        self.inner.borrow().deadline().map(|due| due as f64)
    }

    /// Batches out and not yet answered.
    pub fn calls_out(&self) -> usize {
        self.out.len()
    }

    /// This frame's step (see the module's documentation, and [`super::AppState::poll`] for the
    /// order).
    pub fn poll(&mut self, cx: &mut Cx<'_>) {
        let now = ms(cx.clocks.local_ms);
        if matches!(self.source, Source::Auto) {
            self.source = if let Some(dir) = &cx.startup.fixture {
                Source::On(Box::new(FixtureColors { dir: dir.clone() }))
            } else if let (Some(backend), Some(requests)) = (cx.backend, cx.requests) {
                Source::On(Box::new(BackendColors { backend: backend.clone(), requests: requests.clone() }))
            } else {
                Source::Off
            };
        }
        let inner = self.inner.get_mut();
        if let Some(batch) = inner.tick(now) {
            match &mut self.source {
                Source::On(source) => {
                    let answer = source.airline_colors(batch.clone());
                    self.out.push((batch, answer));
                }
                // `invoke` without Tauri rejects at once: nothing cached, asked again later
                Source::Auto | Source::Off => inner.settled(&batch, None, now),
            }
        }
        self.out.retain(|(batch, answer)| {
            let Some(result) = answer.poll() else { return true };
            match result {
                Ok(colors) => inner.settled(batch, Some(&colors), now),
                Err(e) => {
                    // try again on a later sighting
                    log::debug!("airline colours for {}: {e}", batch.join(" "));
                    inner.settled(batch, None, now);
                }
            }
            false
        });
        if inner.version() != self.seen_version {
            self.seen_version = inner.version();
            cx.wants.scene = true;
        }
        if let Some(due) = inner.deadline() {
            cx.wants.wake_at(due as f64);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};

    use rustradar_core::liveries::DEFAULT_LIVERY;
    use rustradar_core::model::{Airline, ReferenceData};
    use rustradar_core::motion::aircraft::Motion;
    use rustradar_core::settings::Persisted;
    use rustradar_scene::camera::ViewState;
    use rustradar_scene::controller::Controller;

    use super::*;
    use crate::state::{Clocks, Startup, Wants};

    const T0: f64 = 1.7e12;

    /// The calls a [`Canned`] source was asked, each with where its answer goes.
    type Calls = Arc<Mutex<Vec<(Vec<String>, Sender<ColorsAnswer>)>>>;

    /// Canned colours: every call waits until the test answers it.
    #[derive(Clone, Default)]
    struct Canned(Calls);

    impl ColorSource for Canned {
        fn airline_colors(&mut self, iatas: Vec<String>) -> Answer<ColorsAnswer> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().push((iatas, tx));
            rx.into()
        }
    }

    impl Canned {
        fn liveries(&self) -> Liveries {
            Liveries::with_source(Some(Box::new(self.clone())))
        }

        fn asked(&self) -> Vec<Vec<String>> {
            self.0.lock().unwrap().iter().map(|(codes, _)| codes.clone()).collect()
        }

        fn answer(&self, i: usize, answer: ColorsAnswer) {
            self.0.lock().unwrap()[i].1.send(answer).unwrap();
        }
    }

    /// What a poll reads besides the liveries.
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

        fn poll(&mut self, liveries: &mut Liveries, now: f64) -> Wants {
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
            liveries.poll(&mut cx);
            wants
        }
    }

    fn flight(callsign: &str, number: &str) -> LiveFlight {
        LiveFlight { callsign: callsign.into(), flight: number.into(), ..LiveFlight::default() }
    }

    /// Scandinavian (SAS) flies as SK; an airline the reference knows without an IATA code.
    fn reference() -> Reference {
        let airline = |name: &str, iata: &str, icao: &str| Airline { name: name.into(), iata: iata.into(), icao: icao.into() };
        Reference::new(ReferenceData {
            fetched_at: 0,
            airlines: vec![airline("Scandinavian Airlines", "SK", "SAS"), airline("Nobody", "", "NOI")],
            airports: Vec::new(),
        })
    }

    fn colors(primary: [u8; 3], secondary: Option<[u8; 3]>) -> Option<LogoColors> {
        Some(LogoColors { primary, secondary })
    }

    #[test]
    fn logo_colours_are_asked_for_400_ms_after_a_first_sighting() {
        let canned = Canned::default();
        let mut liveries = canned.liveries();
        let mut world = World::new(Startup::default());
        let reference = reference();
        // the reference's code, not the flight number's
        let sas = flight("SAS1415", "XX1415");
        assert_eq!(liveries.for_flight(&sas, Some(&reference), T0), DEFAULT_LIVERY);
        assert_eq!(liveries.deadline(), Some(T0 + 400.0));
        // seen again: still once in the queue
        assert_eq!(liveries.for_flight(&sas, Some(&reference), T0 + 100.0), DEFAULT_LIVERY);
        let wants = world.poll(&mut liveries, T0 + 100.0);
        assert_eq!(wants, Wants { scene: false, wake_at: Some(T0 + 400.0) });
        assert!(canned.asked().is_empty());
        world.poll(&mut liveries, T0 + 399.0);
        assert!(canned.asked().is_empty());
        let wants = world.poll(&mut liveries, T0 + 400.0);
        assert_eq!(canned.asked(), [["SK"]]);
        assert_eq!(wants, Wants::default(), "nothing more to wait for");
        assert_eq!(liveries.calls_out(), 1);
        // in flight: not asked again
        liveries.for_flight(&sas, Some(&reference), T0 + 500.0);
        assert_eq!(liveries.deadline(), None);
        assert_eq!(world.poll(&mut liveries, T0 + 600.0), Wants::default());
        canned.answer(0, Ok(HashMap::from([("SK".to_owned(), colors([0, 0, 255], Some([200, 0, 0])))])));
        let wants = world.poll(&mut liveries, T0 + 700.0);
        assert!(wants.scene, "colours arrived: the scene is drawn again");
        assert_eq!(liveries.version(), 1);
        assert_eq!(liveries.calls_out(), 0);
        let livery = liveries.for_flight(&sas, Some(&reference), T0 + 800.0);
        assert_eq!(livery, Livery { tail: [0, 0, 255], engines: [200, 0, 0], ..DEFAULT_LIVERY });
        assert!(!world.poll(&mut liveries, T0 + 900.0).scene, "once");
        // without reference data the flight number names the airline
        assert_eq!(liveries.for_flight(&flight("SAS7", "SK7"), None, T0 + 1000.0), livery);
        assert_eq!(liveries.for_flight(&flight("SAS7", "XX7"), None, T0 + 1000.0), DEFAULT_LIVERY);
        assert_eq!(liveries.deadline(), Some(T0 + 1400.0), "XX queued");
    }

    #[test]
    fn curated_and_military_liveries_ask_nothing() {
        let canned = Canned::default();
        let mut liveries = canned.liveries();
        let mut world = World::new(Startup::default());
        let ryanair = liveries.for_flight(&flight("RYR12AB", "FR1234"), None, T0);
        assert_eq!(ryanair.tail, [7, 53, 144]);
        assert_ne!(liveries.for_flight(&flight("RCH871", "RC871"), None, T0), DEFAULT_LIVERY);
        assert_eq!(liveries.for_flight(&flight("N123AB", ""), None, T0), DEFAULT_LIVERY);
        assert_eq!(world.poll(&mut liveries, T0 + 10_000.0), Wants::default());
        assert!(canned.asked().is_empty());
    }

    #[test]
    fn a_failed_call_caches_nothing_and_a_later_sighting_asks_again() {
        let canned = Canned::default();
        let mut liveries = canned.liveries();
        let mut world = World::new(Startup::default());
        let reference = reference();
        let sas = flight("SAS1", "SK1");
        liveries.for_flight(&sas, Some(&reference), T0);
        world.poll(&mut liveries, T0 + 400.0);
        canned.answer(0, Err("offline".into()));
        let wants = world.poll(&mut liveries, T0 + 450.0);
        assert_eq!(wants, Wants::default(), "no colours, no refresh, no timer");
        assert_eq!((liveries.version(), liveries.calls_out()), (0, 0));
        assert_eq!(liveries.for_flight(&sas, Some(&reference), T0 + 500.0), DEFAULT_LIVERY);
        assert_eq!(world.poll(&mut liveries, T0 + 500.0).wake_at, Some(T0 + 900.0));
        world.poll(&mut liveries, T0 + 900.0);
        assert_eq!(canned.asked(), [["SK"], ["SK"]]);
        // a logo without colours is remembered as none: drawn in the default, not asked again
        canned.answer(1, Ok(HashMap::from([("SK".to_owned(), None)])));
        assert!(world.poll(&mut liveries, T0 + 950.0).scene);
        assert_eq!(liveries.for_flight(&sas, Some(&reference), T0 + 1000.0), DEFAULT_LIVERY);
        assert_eq!(liveries.deadline(), None);
    }

    #[test]
    fn batches_of_forty_and_the_rest_after_the_first_settles() {
        let canned = Canned::default();
        let mut liveries = canned.liveries();
        let mut world = World::new(Startup::default());
        let codes: Vec<String> = (0..45).map(|i| format!("{}{}", (b'A' + i / 10) as char, i % 10)).collect();
        for (i, code) in codes.iter().enumerate() {
            liveries.for_flight(&flight(&format!("ZZZ{i}"), &format!("{code}1")), None, T0 + i as f64);
        }
        world.poll(&mut liveries, T0 + 400.0);
        assert_eq!(canned.asked(), [codes[..40].to_vec()]);
        // the other five wait for the first batch
        assert_eq!(world.poll(&mut liveries, T0 + 5_000.0), Wants::default());
        canned.answer(0, Ok(HashMap::new()));
        let wants = world.poll(&mut liveries, T0 + 5_100.0);
        assert_eq!(wants, Wants { scene: true, wake_at: Some(T0 + 5_500.0) });
        world.poll(&mut liveries, T0 + 5_500.0);
        assert_eq!(canned.asked()[1], codes[40..]);
    }

    #[test]
    fn without_a_backend_every_batch_fails_at_once() {
        // no fixture, no backend: the page without Tauri
        let mut liveries = Liveries::default();
        let mut world = World::new(Startup::default());
        let sas = flight("SAS1", "SK1");
        liveries.for_flight(&sas, None, T0);
        assert_eq!(world.poll(&mut liveries, T0).wake_at, Some(T0 + 400.0));
        assert_eq!(world.poll(&mut liveries, T0 + 400.0), Wants::default());
        assert_eq!((liveries.version(), liveries.calls_out()), (0, 0));
        // asked again on a later sighting, and fails again
        liveries.for_flight(&sas, None, T0 + 1_000.0);
        assert_eq!(liveries.deadline(), Some(T0 + 1_400.0));
        // a source of none, set up front, is the same
        let mut off = Liveries::with_source(None);
        off.for_flight(&sas, None, T0);
        assert_eq!(world.poll(&mut off, T0 + 400.0), Wants::default());
        assert_eq!(format!("{off:?}"), "Liveries { source: \"off\", calls_out: 0, version: 0, .. }");
    }

    /// A folder of its own under the system's temporary folder, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("rustradar-liveries-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_fixture_folder_answers_from_its_file_in_the_same_frame() {
        let dir = TempDir::new("fixture");
        std::fs::write(dir.0.join(FIXTURE_COLORS), r#"{"SK": {"primary": [0, 32, 96], "secondary": null}, "AB": null}"#).unwrap();
        let mut liveries = Liveries::default();
        let mut world = World::new(Startup { fixture: Some(dir.0.clone()), ..Startup::default() });
        let reference = reference();
        let (sas, other) = (flight("SAS1", "SK1"), flight("ABC1", "AB1"));
        liveries.for_flight(&sas, Some(&reference), T0);
        liveries.for_flight(&other, Some(&reference), T0);
        let wants = world.poll(&mut liveries, T0 + 400.0);
        assert!(wants.scene && liveries.version() == 1 && liveries.calls_out() == 0);
        assert_eq!(liveries.for_flight(&sas, Some(&reference), T0 + 500.0), Livery { tail: [0, 32, 96], ..DEFAULT_LIVERY });
        assert_eq!(liveries.for_flight(&other, Some(&reference), T0 + 500.0), DEFAULT_LIVERY);
        assert_eq!(liveries.deadline(), None, "both known");
        // a folder without the file: a failed call, nothing cached
        let empty = TempDir::new("empty");
        let mut liveries = Liveries::default();
        let mut world = World::new(Startup { fixture: Some(empty.0.clone()), ..Startup::default() });
        liveries.for_flight(&sas, Some(&reference), T0);
        assert_eq!(world.poll(&mut liveries, T0 + 400.0), Wants::default());
        assert_eq!(liveries.version(), 0);
        assert!(read_fixture(&empty.0).unwrap_err().contains(FIXTURE_COLORS));
    }
}
