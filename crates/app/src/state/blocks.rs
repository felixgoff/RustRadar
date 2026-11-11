//! Signal blocks (item T5; doc 02 §6.1–6.2; `refreshBlocks`, `Globe.svelte:2218-2244`): the block
//! set fetched round the view while the rail map shows the blocks, and the block layers' part of
//! each scene refresh ([`Blocks::build`], with [`rustradar_scene::blocks::BlockBuilder`]).
//!
//! The blocks are on when the rail map is on and shows the blocks ([`blocks_on`]; today's
//! `trainBlocks` prop, `settings.railMap && settings.railShows === "blocks" && isTauri()`), and
//! are asked for only close in: at zoom [`BLOCKS_MIN_ZOOM`] or closer, and never while a request is
//! out. With the view's bounds ([`view_bounds`]):
//! - the view inside the area last kept: nothing, unless that set is incomplete and
//!   [`LOADING_MS`] have passed since the last ask;
//! - the view outside it (or nothing kept yet): asked for once [`ASK_AGAIN_MS`] have passed since
//!   the last ask;
//! - each ask is for the view padded by [`PAD`] of its width and height on each side.
//!
//! An answer replaces the set (and the taken blocks are worked out again at the next scene
//! refresh), unless it is incomplete with fewer blocks than the set on show: then the set stays
//! and is marked incomplete, so the map does not flick between the blocks and the bare lines while
//! the track or signals load (`Globe.svelte:2235`; the plan's "a shorter answer while loading does
//! not replace a fuller set"). A failed request is dropped. A set stays when the blocks are turned
//! off or the camera zooms out, as today; it is only not drawn.
//!
//! Today's page checks all this at every frame its loop draws; here [`Blocks::poll`] does it once
//! a frame and asks the frame loop to come back when the next ask is due ([`super::Wants`]).
//!
//! **Sources**: the `--fixture` folder ([`FIXTURE_BLOCKS`]: a `TrainBlocks` answer, read again at
//! each ask), else the backend (`trains::blocks`), else none (nothing is asked);
//! [`Blocks::with_feed`] sets one instead (tests). `--demo` asks the backend as today's `#demo`
//! does (the demo trains take blocks wherever their made-up places lie on fetched track).
//!
//! Deviations from today's app:
//! - An answer is taken at the next poll after it arrives (a fixture's in the poll that asked).
//! - The view's bounds are the controller's view on its canvas at the poll; today's are read at
//!   each frame of the loop.

use std::path::PathBuf;

use rustradar_backend::trains as backend_trains;
use rustradar_backend::Backend;
use rustradar_core::geo::Area;
use rustradar_core::model::{Structure, Train, TrainBlock, TrainBlocks};
use rustradar_core::settings::{RailShows, Settings};
use rustradar_map::SceneInput;
use rustradar_scene::blocks::{BlockBuilder, DrawnTrain, SetRef, Taken};
use rustradar_scene::camera::ViewState;
use rustradar_scene::trains::Drawn;

pub use rustradar_scene::blocks::{BLOCKS_MIN_ZOOM, view_bounds};

use super::aircraft::Answer;
use super::{Cx, Startup};
use crate::requests::Requests;

/// The `--fixture` folder's blocks: a `TrainBlocks` answer as `train_blocks` sends it.
pub const FIXTURE_BLOCKS: &str = "train_blocks.json";
/// Each ask is for the view padded by this much of its width and height on each side.
pub const PAD: f64 = 0.15;
/// A view outside the area kept is asked for at most this often, ms.
pub const ASK_AGAIN_MS: f64 = 700.0;
/// An incomplete set is asked for again this long after the last ask, while the view stays in
/// its area, ms.
pub const LOADING_MS: f64 = 2_500.0;

/// Whether the map shows the signal blocks (today's `trainBlocks`): the rail map on, showing the
/// blocks. The rail map shows the speed limits only when it does not (`railSpeeds`, item T4).
pub fn blocks_on(settings: &Settings) -> bool {
    settings.rail_map && settings.rail_shows == RailShows::Blocks
}

/// Where the blocks come from: the backend, the `--fixture` folder, or a test's.
pub trait BlockFeed: Send {
    /// `trainBlocks(area)`.
    fn train_blocks(&mut self, area: Area) -> Answer<Result<TrainBlocks, String>>;
}

/// The backend's `trains::blocks`, on its runtime.
struct BackendFeed {
    requests: Requests,
}

impl BlockFeed for BackendFeed {
    fn train_blocks(&mut self, area: Area) -> Answer<Result<TrainBlocks, String>> {
        self.requests.request(backend_trains::blocks(area.west, area.south, area.east, area.north)).into()
    }
}

/// The `--fixture` folder's [`FIXTURE_BLOCKS`], read at each ask.
struct FixtureFeed {
    dir: PathBuf,
}

impl BlockFeed for FixtureFeed {
    fn train_blocks(&mut self, _area: Area) -> Answer<Result<TrainBlocks, String>> {
        let path = self.dir.join(FIXTURE_BLOCKS);
        Answer::ready(
            std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| serde_json::from_slice::<TrainBlocks>(&bytes).map_err(|e| e.to_string()))
                .map_err(|e| format!("fixture {}: {e}", path.display())),
        )
    }
}

/// Which blocks the state asks for.
enum Source {
    /// Chosen at the first poll: the fixture folder, else the backend, else none.
    Auto,
    Off,
    On(Box<dyn BlockFeed>),
}

/// The blocks fetched (`blockSet`): the area asked for (padded round the view), the blocks
/// starting in it with the bridges and tunnels round them, and whether all of it was there.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockSet {
    pub area: Area,
    pub list: Vec<TrainBlock>,
    pub structures: Vec<Structure>,
    pub complete: bool,
}

impl BlockSet {
    /// Whether the view's `bounds` (`[west, south, east, north]`) lie within the area.
    pub fn holds(&self, [west, south, east, north]: [f64; 4]) -> bool {
        west >= self.area.west && east <= self.area.east && south >= self.area.south && north <= self.area.north
    }
}

/// The area asked for with the view's `bounds`: padded by [`PAD`] of its width and height on each
/// side.
pub fn padded([west, south, east, north]: [f64; 4]) -> Area {
    let (dx, dy) = ((east - west) * PAD, (north - south) * PAD);
    Area { west: west - dx, south: south - dy, east: east + dx, north: north + dy }
}

/// Signal blocks: the set fetched round the view and the block layers' scene.
pub struct Blocks {
    source: Source,
    set: Option<BlockSet>,
    /// Bumped with each set kept (the scene's geometry is made again).
    generation: u64,
    /// The request out (`blocksAsking`), and the area it asked for.
    asking: Option<(Answer<Result<TrainBlocks, String>>, Area)>,
    /// `blocksAskedAt`: when the last ask went out (local ms); 0 before the first.
    asked_at: f64,
    builder: BlockBuilder,
}

impl Default for Blocks {
    /// The `--fixture` folder's blocks, else the backend's, chosen at the first poll.
    fn default() -> Self {
        Blocks::with_source(Source::Auto)
    }
}

impl std::fmt::Debug for Blocks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Blocks")
            .field("blocks", &self.set.as_ref().map(|s| s.list.len()))
            .field("complete", &self.set.as_ref().map(|s| s.complete))
            .field("asking", &self.asking.is_some())
            .field("asked_at", &self.asked_at)
            .finish_non_exhaustive()
    }
}

impl Blocks {
    fn with_source(source: Source) -> Self {
        Blocks { source, set: None, generation: 0, asking: None, asked_at: 0.0, builder: BlockBuilder::new() }
    }

    /// The blocks of `feed` instead of the fixture folder's or the backend's; `None`: none at all.
    pub fn with_feed(feed: Option<Box<dyn BlockFeed>>) -> Self {
        Blocks::with_source(match feed {
            Some(feed) => Source::On(feed),
            None => Source::Off,
        })
    }

    fn resolve(&mut self, startup: &Startup, backend: Option<&Backend>, requests: Option<&Requests>) {
        if !matches!(self.source, Source::Auto) {
            return;
        }
        self.source = if let Some(dir) = &startup.fixture {
            Source::On(Box::new(FixtureFeed { dir: dir.clone() }))
        } else if let (Some(_), Some(requests)) = (backend, requests) {
            Source::On(Box::new(BackendFeed { requests: requests.clone() }))
        } else {
            Source::Off
        };
    }

    /// The set kept, if any.
    pub fn set(&self) -> Option<&BlockSet> {
        self.set.as_ref()
    }

    /// Whether a request is out.
    pub fn asking(&self) -> bool {
        self.asking.is_some()
    }

    /// The taken blocks as last worked out (by block and by train).
    pub fn taken(&self) -> &Taken {
        self.builder.taken()
    }

    /// When the next ask is due with the camera at `view` on a `size` canvas, and for which
    /// area: none while off, out beyond zoom [`BLOCKS_MIN_ZOOM`] (or on the globe), asking, or
    /// inside a complete set.
    fn next_ask(&self, on: bool, view: &ViewState, size: [f64; 2]) -> Option<(f64, Area)> {
        if !on || matches!(self.source, Source::Off) || self.asking.is_some() || view.zoom < BLOCKS_MIN_ZOOM {
            return None;
        }
        let bounds = view_bounds(view, size)?;
        let at = match &self.set {
            Some(set) if set.holds(bounds) => {
                if set.complete {
                    return None;
                }
                self.asked_at + LOADING_MS
            }
            _ => self.asked_at + ASK_AGAIN_MS,
        };
        Some((at, padded(bounds)))
    }

    /// An answer for `area` (`fetchTrainBlocks(area).then(…)`): kept unless it is a short one
    /// while loading. Whether the set changed.
    fn take(&mut self, answer: TrainBlocks, area: Area) -> bool {
        if let Some(set) = &mut self.set
            && !answer.complete
            && answer.blocks.len() < set.list.len()
        {
            set.complete = false;
            return false;
        }
        self.set = Some(BlockSet { area, list: answer.blocks, structures: answer.structures, complete: answer.complete });
        self.generation += 1;
        true
    }

    /// This frame's step (see [`super::AppState::poll`] for the order): the ask whose time has
    /// come, then the answer if it is in, then when to come back for the next ask.
    pub fn poll(&mut self, cx: &mut Cx<'_>) {
        self.resolve(cx.startup, cx.backend, cx.requests);
        let now = cx.clocks.local_ms;
        let on = blocks_on(&cx.persisted.settings);
        let view = cx.controller.view();
        let size = cx.controller.size();
        if let Some((at, area)) = self.next_ask(on, &view, size)
            && now >= at
            && let Source::On(feed) = &mut self.source
        {
            self.asking = Some((feed.train_blocks(area), area));
            self.asked_at = now;
        }
        if let Some((answer, area)) = &self.asking
            && let Some(result) = answer.poll()
        {
            let area = *area;
            self.asking = None;
            match result {
                Ok(blocks) => cx.wants.scene |= self.take(blocks, area),
                // errors are dropped: the view asks again
                Err(e) => log::debug!("signal blocks: {e}"),
            }
        }
        if let Some((at, _)) = self.next_ask(on, &view, size) {
            cx.wants.wake_at(at);
        }
    }

    /// Fills the block layers' input of `scene` (`blockLayers`): the set kept while the blocks are
    /// on in `settings`, with the trains drawn this refresh, `list` in the order `drawn` holds
    /// them (the trains' scene builder runs first, as `trainLayers` is computed before
    /// `blockLayers`, G:2733).
    pub fn build(&mut self, settings: &Settings, list: &[&Train], drawn: &Drawn, scene: &mut SceneInput) {
        let set = if blocks_on(settings) {
            self.set.as_ref().map(|s| SetRef { generation: self.generation, list: &s.list, structures: &s.structures })
        } else {
            None
        };
        let trains: Vec<DrawnTrain<'_>> =
            list.iter().zip(&drawn.positions).map(|(t, p)| DrawnTrain { id: &t.id, name: &t.name, at: [p[0], p[1]] }).collect();
        scene.blocks.scene = self.builder.build(set, &trains, &scene.view, scene.size, scene.local_now_ms);
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};

    use rustradar_core::geo::Basemap;

    use super::*;
    use crate::state::{AppState, Wants};

    /// 2026-06-21 10:00 UTC.
    const T0: f64 = 1_782_036_000_000.0;
    const LAT: f64 = 59.6735;
    const LON: f64 = 17.91;
    const SIZE: [f64; 2] = [1440.0, 900.0];

    /// One ask: its area, and the way to answer it.
    type Ask = (Area, Sender<Result<TrainBlocks, String>>);

    /// The asks made.
    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<Vec<Ask>>>);

    impl BlockFeed for Fake {
        fn train_blocks(&mut self, area: Area) -> Answer<Result<TrainBlocks, String>> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().push((area, tx));
            rx.into()
        }
    }

    impl Fake {
        fn asks(&self) -> usize {
            self.0.lock().unwrap().len()
        }

        fn area(&self, k: usize) -> Area {
            self.0.lock().unwrap()[k].0
        }

        fn answer(&self, k: usize, answer: Result<TrainBlocks, String>) {
            self.0.lock().unwrap()[k].1.send(answer).unwrap();
        }
    }

    fn spot(east: f64, north: f64) -> [f64; 2] {
        [LON + east / (111_320.0 * (LAT * PI / 180.0).cos()), LAT + north / 111_320.0]
    }

    fn view(east: f64, zoom: f64) -> ViewState {
        let [longitude, latitude] = spot(east, 0.0);
        ViewState { longitude, latitude, zoom, pitch: 0.0, bearing: 0.0, position: None }
    }

    /// `n` blocks of 100 m running east along the view's middle.
    fn answer(n: usize, complete: bool) -> TrainBlocks {
        let blocks = (0..n)
            .map(|k| {
                let from = -300.0 + 100.0 * k as f64;
                TrainBlock { id: format!("b{k}"), signal: Some(format!("{k}")), lines: vec![vec![spot(from, 0.0), spot(from + 100.0, 0.0)]] }
            })
            .collect();
        TrainBlocks { blocks, structures: Vec::new(), complete }
    }

    /// The state at `v` with the blocks on and asked of `fake`.
    fn state(v: ViewState, fake: &Fake) -> AppState {
        let startup = Startup { basemap: Some(Basemap::Dark), fixture: None, demo: false };
        let mut s = AppState::new(startup, None, None, None, v, SIZE);
        s.persisted.settings.rail_map = true;
        s.persisted.settings.rail_shows = RailShows::Blocks;
        s.blocks = Blocks::with_feed(Some(Box::new(fake.clone())));
        s
    }

    #[test]
    fn the_blocks_are_on_with_the_rail_map_showing_them() {
        let mut settings = Settings::default();
        assert!(!blocks_on(&settings));
        settings.rail_shows = RailShows::Blocks;
        assert!(!blocks_on(&settings), "the rail map is off");
        settings.rail_map = true;
        assert!(blocks_on(&settings));
        settings.rail_shows = RailShows::Speeds;
        assert!(!blocks_on(&settings));
    }

    #[test]
    fn asked_close_in_for_the_view_padded_by_fifteen_percent() {
        let fake = Fake::default();
        let mut s = state(view(0.0, 15.0), &fake);
        s.poll(T0);
        assert_eq!(fake.asks(), 1, "at once");
        let [w, so, e, n] = view_bounds(&view(0.0, 15.0), SIZE).unwrap();
        let area = fake.area(0);
        let (dx, dy) = ((e - w) * 0.15, (n - so) * 0.15);
        assert_eq!(area, Area { west: w - dx, south: so - dy, east: e + dx, north: n + dy });
        // never twice while one is out
        s.poll(T0 + 5_000.0);
        assert_eq!(fake.asks(), 1);
        assert!(s.blocks.asking());
    }

    #[test]
    fn nothing_is_asked_off_far_out_or_without_a_source() {
        let fake = Fake::default();
        let mut s = state(view(0.0, 12.99), &fake);
        s.poll(T0);
        assert_eq!(fake.asks(), 0, "out beyond zoom 13");
        s.controller.set_view(view(0.0, 13.0));
        s.persisted.settings.rail_shows = RailShows::Speeds;
        s.poll(T0 + 1_000.0);
        assert_eq!(fake.asks(), 0, "the rail map shows speeds");
        s.persisted.settings.rail_shows = RailShows::Blocks;
        s.persisted.settings.rail_map = false;
        s.poll(T0 + 2_000.0);
        assert_eq!(fake.asks(), 0, "the rail map is off");
        s.persisted.settings.rail_map = true;
        s.poll(T0 + 3_000.0);
        assert_eq!(fake.asks(), 1);
        // no backend, no fixture: none at all
        let startup = Startup { basemap: Some(Basemap::Dark), fixture: None, demo: false };
        let mut bare = AppState::new(startup, None, None, None, view(0.0, 15.0), SIZE);
        bare.persisted.settings.rail_map = true;
        bare.persisted.settings.rail_shows = RailShows::Blocks;
        assert_eq!(bare.poll(T0).wake_at, None);
        assert!(!bare.blocks.asking() && bare.blocks.set().is_none());
    }

    #[test]
    fn a_complete_set_round_the_view_is_kept_and_asked_for_again_only_when_the_view_leaves_it() {
        let fake = Fake::default();
        let mut s = state(view(0.0, 15.0), &fake);
        s.poll(T0);
        fake.answer(0, Ok(answer(3, true)));
        let wants = s.poll(T0 + 300.0);
        assert!(wants.scene, "a new set: the scene again");
        assert_eq!(wants.wake_at, None, "inside a complete set: nothing to come back for");
        let set = s.blocks.set().unwrap();
        assert_eq!((set.list.len(), set.complete, set.area), (3, true, fake.area(0)));
        s.poll(T0 + 60_000.0);
        assert_eq!(fake.asks(), 1, "nothing more while the view stays inside");
        // a small move stays inside the padded area
        s.controller.set_view(view(30.0, 15.0));
        s.poll(T0 + 61_000.0);
        assert_eq!(fake.asks(), 1);
        // far enough to leave it: asked at once (700 ms since the last ask are long gone)
        s.controller.set_view(view(400.0, 15.0));
        s.poll(T0 + 62_000.0);
        assert_eq!(fake.asks(), 2);
    }

    #[test]
    fn a_view_outside_is_asked_for_at_most_every_700_ms() {
        let fake = Fake::default();
        let mut s = state(view(0.0, 15.0), &fake);
        s.poll(T0);
        fake.answer(0, Err("Trafikverket is out of reach".into()));
        let wants = s.poll(T0 + 100.0);
        assert!(!wants.scene && s.blocks.set().is_none(), "an error is dropped");
        assert_eq!(wants.wake_at, Some(T0 + 700.0), "come back when the next ask is due");
        s.poll(T0 + 699.0);
        assert_eq!(fake.asks(), 1);
        s.poll(T0 + 700.0);
        assert_eq!(fake.asks(), 2);
        // answered, then the view moves out of it at once: still 700 ms after the last ask
        fake.answer(1, Ok(answer(3, true)));
        s.poll(T0 + 800.0);
        s.controller.set_view(view(1_000.0, 15.0));
        let wants = s.poll(T0 + 900.0);
        assert_eq!(fake.asks(), 2);
        assert_eq!(wants.wake_at, Some(T0 + 1_400.0));
        s.poll(T0 + 1_400.0);
        assert_eq!(fake.asks(), 3);
    }

    #[test]
    fn an_incomplete_set_is_asked_for_again_every_two_and_a_half_seconds() {
        let fake = Fake::default();
        let mut s = state(view(0.0, 15.0), &fake);
        s.poll(T0);
        fake.answer(0, Ok(answer(2, false)));
        let wants = s.poll(T0 + 200.0);
        assert!(wants.scene);
        assert_eq!(wants.wake_at, Some(T0 + 2_500.0));
        s.poll(T0 + 2_499.0);
        assert_eq!(fake.asks(), 1);
        s.poll(T0 + 2_500.0);
        assert_eq!(fake.asks(), 2);
        // still loading and still fewer: the fuller set on show stays, marked incomplete
        fake.answer(1, Ok(answer(4, false)));
        assert!(s.poll(T0 + 2_600.0).scene, "more blocks while loading replace the set");
        s.poll(T0 + 5_000.0);
        assert_eq!(fake.asks(), 3);
        fake.answer(2, Ok(answer(1, false)));
        let wants = s.poll(T0 + 5_100.0);
        assert!(!wants.scene, "a shorter answer while loading does not replace a fuller set");
        let set = s.blocks.set().unwrap();
        assert_eq!((set.list.len(), set.complete), (4, false));
        assert_eq!(wants.wake_at, Some(T0 + 7_500.0));
        // a complete answer replaces it whatever its length
        s.poll(T0 + 7_500.0);
        fake.answer(3, Ok(answer(1, true)));
        assert!(s.poll(T0 + 7_600.0).scene);
        let set = s.blocks.set().unwrap();
        assert_eq!((set.list.len(), set.complete), (1, true));
        assert_eq!(s.poll(T0 + 20_000.0), Wants::default(), "done");
    }

    #[test]
    fn an_incomplete_answer_as_long_as_the_set_replaces_it() {
        let fake = Fake::default();
        let mut s = state(view(0.0, 15.0), &fake);
        s.poll(T0);
        fake.answer(0, Ok(answer(2, true)));
        s.poll(T0 + 100.0);
        // the view leaves; the answer for the new place is incomplete but as long: kept, with its
        // area
        s.controller.set_view(view(2_000.0, 15.0));
        s.poll(T0 + 1_000.0);
        fake.answer(1, Ok(answer(2, false)));
        s.poll(T0 + 1_100.0);
        let set = s.blocks.set().unwrap();
        assert_eq!((set.complete, set.area), (false, fake.area(1)));
    }

    #[test]
    fn the_scene_has_the_blocks_only_while_on_and_close_in_with_the_trains_as_drawn() {
        let fake = Fake::default();
        let mut s = state(view(0.0, 15.0), &fake);
        s.poll(T0);
        fake.answer(0, Ok(answer(4, true)));
        s.poll(T0 + 100.0);
        let train = Train {
            id: "trafikverket:2026-06-21:523".into(),
            source: rustradar_core::model::TrainSource::Trafikverket,
            name: "SJ Snabbtåg 523".into(),
            kind: rustradar_core::model::TrainKind::HighSpeed,
            lat: LAT,
            lon: spot(-150.0, 0.0)[0],
            heading: Some(90.0),
            speed_kmh: None,
            destination: None,
            operator: Some("SJ".into()),
            delay_minutes: None,
            position_unix: (T0 / 1000.0) as i64,
            frames: Vec::new(),
            reports: Vec::new(),
            predicted: Vec::new(),
            cars: None,
            tail: Vec::new(),
            structures: Vec::new(),
            ended_unix: None,
            shape: None,
            model: None,
        };
        let mut motion = rustradar_scene::trains::TrainMotion::new();
        motion.set_list(&[&train], T0 as i64);
        let settings = s.persisted.settings.clone();
        let mut scene = SceneInput::new(view(0.0, 15.0), SIZE, 1.0, Basemap::Dark);
        scene.local_now_ms = T0 + 200.0;
        s.blocks.build(&settings, &[&train], motion.drawn(), &mut scene);
        let built = &scene.blocks.scene;
        assert_eq!(built.runs.signals.len(), 4);
        assert!(built.is_taken("b1") && !built.is_taken("b0"));
        assert_eq!(built.labels[0].text, "SJ Snabbtåg 523");
        assert_eq!(s.blocks.taken().of["trafikverket:2026-06-21:523"], "b1");
        // the blocks off: nothing drawn, the set kept
        let mut off = settings.clone();
        off.rail_shows = RailShows::Speeds;
        s.blocks.build(&off, &[&train], motion.drawn(), &mut scene);
        assert!(scene.blocks.scene.is_empty());
        assert!(s.blocks.set().is_some());
        // out beyond zoom 13: nothing drawn
        let mut far = SceneInput::new(view(0.0, 12.5), SIZE, 1.0, Basemap::Dark);
        s.blocks.build(&settings, &[&train], motion.drawn(), &mut far);
        assert!(far.blocks.scene.is_empty());
    }

    #[test]
    fn the_fixture_folder_answers_from_its_file() {
        let dir = std::env::temp_dir().join(format!("rustradar-t5-blocks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FIXTURE_BLOCKS), serde_json::to_vec(&answer(3, true)).unwrap()).unwrap();
        let startup = Startup { basemap: Some(Basemap::Dark), fixture: Some(dir.clone()), demo: false };
        let mut s = AppState::new(startup, None, None, None, view(0.0, 15.0), SIZE);
        s.persisted.settings.rail_map = true;
        s.persisted.settings.rail_shows = RailShows::Blocks;
        let wants = s.poll(T0);
        assert!(wants.scene, "asked and answered in the first poll");
        assert_eq!(s.blocks.set().map(|set| set.list.len()), Some(3));
        // a folder without the file: an error, dropped, asked again 700 ms on
        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let startup = Startup { basemap: Some(Basemap::Dark), fixture: Some(empty), demo: false };
        let mut s = AppState::new(startup, None, None, None, view(0.0, 15.0), SIZE);
        s.persisted.settings.rail_map = true;
        s.persisted.settings.rail_shows = RailShows::Blocks;
        assert_eq!(s.poll(T0).wake_at, Some(T0 + 700.0));
        assert!(s.blocks.set().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
