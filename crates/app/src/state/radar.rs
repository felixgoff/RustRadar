//! The weather radar (item W5; doc 02 §3.6, §14.2; G:1111-1151, G:2680-2690): which RainViewer
//! frame the map draws and which measured volumes (NOAA MRMS, DWD) it draws over it, asked for as
//! today's `refreshRadar` and `refreshMeasured` ask for them.
//!
//! Today's page asks from inside `buildLayers`, at each render while the weather is on. Here
//! [`Radar::poll`] asks once a frame on the same conditions and the same clock (the local one,
//! `Date.now()`), and hands the frame loop the time of the next ask as a
//! [`Wants::wake_at`](super::Wants). A poll, in this order:
//!
//! 1. the first poll only: the source (below);
//! 2. while the weather is on (`settings.weather`): RainViewer's frame list is asked for once the
//!    last ask is more than ten minutes old (`rainviewer::is_due`, `Date.now() - radarCheckedAt >
//!    600_000`, the time taken as the ask starts, success or not); the measured volumes for the
//!    view ([`view_area`] of the camera and the canvas, `None` with most of the globe in sight)
//!    once the last ask *ended* more than [`MEASURED_REFRESH_MS`] ago and none is out
//!    (`measuredLoading`; `measuredCheckedAt` is set as the ask starts and again as it ends);
//! 3. the answers, whether or not the weather is still on (today's promises land all the same):
//!    - **a frame**: RainViewer's tile template is set in the map's tile sources
//!      ([`Radar::tile_sources`], `TileSources::set_template(io::RAINVIEWER, …)`) *before* the frame
//!      is handed over ([`Radar::url`]), so no tile of the new frame is asked under the old one's
//!      URL. A list without a frame, or a failed ask, leaves no frame (`radarUrl = null`), so no
//!      RainViewer is drawn until the next ask finds one;
//!    - **volumes**: a volume already held with the same source and scan time is kept as it is;
//!      when every one in the answer is held and there are as many as before, nothing changes
//!      (the set and its meshes stay). Otherwise the answer's volumes, in its order, become the set
//!      ([`Radar::measured`]). A failed ask keeps the set (logged, as `console.warn`).
//!
//! **What the scene gets** (`scene_builder::radar`): while the weather is on, the frame
//! ([`Radar::url`], `None` draws no RainViewer) and the volumes; with the weather off neither, so
//! the map lets go of RainViewer's tiles, the lifted layers' arenas (`freeArenas`, item W3: freed
//! whenever the scene carries no frame) and the measured meshes. RainViewer is blanked under the
//! coverage of the volumes the map draws (`setRadarCoverage` once a set is in: the map's radar
//! layer hands the drawn set's coverages to the tiles, W4), not here.
//!
//! **Sources**: the `--fixture` folder ([`FixtureFeeds`]: RainViewer's frame list from
//! `<tiles>/rainviewer/weather-maps.json` ([`FIXTURE_FRAMES`], the list the committed tiles were
//! fetched from), the volumes recorded under `<fixture>/radar/<source>/` read with
//! `backend::radar::read`), else the network (RainViewer through `backend::rainviewer::frame`, on
//! the runtime of the [`Requests`]; the volumes through `Backend::radar_volumes`, only with a
//! backend: today's `isTauri()`), else none (no runtime: nothing is asked).
//! [`Radar::with_feeds`] sets them instead (tests, shots without a fixture folder).
//!
//! Deviations from today's app:
//! - New volumes are turned into the map's polygons ([`map_volume`]: `band_polygon` over the
//!   backend's [`radar::Polygon`]s, as `parseVolumePolygons` read the blobs) off the main thread,
//!   in the ask itself; a volume already held is not converted again. Today's page loads each new
//!   volume's blobs with `radar_image` and parses them in a worker.
//! - With the weather off the state keeps the frame and the volumes (as today's `radarUrl` and
//!   `measured`), but the map drops their meshes: turned on again, the volumes are meshed again
//!   (today only their GPU buffers are rebuilt from the meshes kept), and RainViewer is blanked
//!   again once they are drawn (today the coverage stays set while the weather is off).
//! - A failed frame ask asks for a new scene at once; today the frame is dropped silently and the
//!   next render shows it.
//! - The fixture folder's volumes are held as the backend holds them: a recorded volume whose
//!   region is in the area asked for is "fetched" and held from then on, and every one held is in
//!   each answer (`radar_frame` answers every volume it holds), in the order of their folders'
//!   names.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustradar_backend::{Backend, radar, rainviewer};
use rustradar_core::geo::{self, Area, view_area};
use rustradar_core::model::RadarBounds;
use rustradar_map::io;
use rustradar_map::layers::radar::measured::{Volume as MapVolume, VolumeKey, band_polygon};

use super::aircraft::Answer;
use super::{Cx, Startup};
use crate::requests::Requests;
use crate::tile_fetcher::{TileSources, fixture_tiles};

/// The measured volumes are asked for again once the last ask ended this long ago, ms
/// (G:2686: `Date.now() - measuredCheckedAt > 120_000`).
pub const MEASURED_REFRESH_MS: f64 = 120_000.0;

/// The `--fixture` folder's RainViewer frame list, in its tile folder: `weather-maps.json` as
/// RainViewer sent it when the committed tiles were fetched.
pub const FIXTURE_FRAMES: &str = "rainviewer/weather-maps.json";

/// The `--fixture` folder's measured volumes: one folder per source, as `backend::radar::read`
/// reads it (`volume.json` and the blobs).
pub const FIXTURE_VOLUMES: &str = "radar";

/// What an answer to [`RadarFeeds::volumes`] brings for each volume, in the backend's order: one
/// the state holds already (by its source and scan time), or a new one made for the map.
#[derive(Debug, Clone)]
pub enum Arrived {
    Held(VolumeKey),
    New(Arc<MapVolume>),
}

/// Where the radar's answers come from: the network and the backend, the `--fixture` folder, or a
/// test's.
pub trait RadarFeeds: Send {
    /// `refreshRadar`'s ask: the newest RainViewer frame (`None` when the list has none).
    fn frame(&mut self) -> Answer<Result<Option<rainviewer::Frame>, String>>;
    /// Whether the measured volumes can be asked for at all (today's `isTauri()`: a backend).
    fn measures(&self) -> bool;
    /// `refreshMeasured(area)`'s ask: the volumes for `area` (`None`: everywhere), each one whose
    /// key is in `held` answered as [`Arrived::Held`], the others made for the map.
    fn volumes(&mut self, area: Option<Area>, held: Vec<VolumeKey>) -> Answer<Result<Vec<Arrived>, String>>;
}

/// A backend volume as the map's measured layer takes it (W4's `Volume`): what the backend says of
/// it (`info()`), the ground's and each drawn height's polygons through `band_polygon` (which drops
/// what `parseVolumePolygons` drops), the coverage mask.
pub fn map_volume(volume: &radar::Volume) -> MapVolume {
    let polygons = |list: &[radar::Polygon]| list.iter().filter_map(|p| band_polygon(p.band, &p.rings)).collect::<Vec<_>>();
    MapVolume {
        frame: volume.info(),
        ground: polygons(&volume.ground),
        levels: volume.drawn_levels().map(|(_, shapes)| polygons(shapes)).collect(),
        coverage: volume.coverage.clone(),
    }
}

/// `volumes` as an answer for a state holding `held`: the volumes held by their keys, the others
/// made for the map ([`map_volume`]).
fn arrivals(volumes: &[Arc<radar::Volume>], held: &[VolumeKey]) -> Vec<Arrived> {
    volumes
        .iter()
        .map(|volume| {
            let key = (volume.source.to_owned(), volume.time);
            if held.contains(&key) { Arrived::Held(key) } else { Arrived::New(Arc::new(map_volume(volume))) }
        })
        .collect()
}

/// The area as `radar_volumes` takes it (the page's numbers become `f32` on the way to Rust).
fn bounding_box(area: Area) -> fr24::grpc::BoundingBox {
    fr24::grpc::BoundingBox { south: area.south as f32, north: area.north as f32, west: area.west as f32, east: area.east as f32 }
}

/// Whether a source's `bounds` are in `area` (`None`: everywhere), as the backend decides which
/// sources to fetch (`in_view` of `radar_frame`): an area across the antimeridian is two spans.
fn in_view(bounds: &RadarBounds, area: Option<&Area>) -> bool {
    let Some(area) = area else { return true };
    // (compared as the backend compares them: the area in `f32`)
    let (south, north, west, east) = (f64::from(area.south as f32), f64::from(area.north as f32), f64::from(area.west as f32), f64::from(area.east as f32));
    if south > bounds.north || north < bounds.south {
        return false;
    }
    if west > east { bounds.east >= west || bounds.west <= east } else { bounds.west <= east && bounds.east >= west }
}

/// The network's answers: RainViewer on the runtime of `requests`, the volumes from `backend`.
struct BackendFeeds {
    requests: Requests,
    backend: Option<Backend>,
}

impl RadarFeeds for BackendFeeds {
    fn frame(&mut self) -> Answer<Result<Option<rainviewer::Frame>, String>> {
        self.requests.request(rainviewer::frame()).into()
    }

    fn measures(&self) -> bool {
        self.backend.is_some()
    }

    fn volumes(&mut self, area: Option<Area>, held: Vec<VolumeKey>) -> Answer<Result<Vec<Arrived>, String>> {
        let Some(backend) = self.backend.clone() else { return Answer::ready(Err(super::aircraft::NO_BACKEND.to_owned())) };
        self.requests
            .request(async move {
                let volumes = backend.radar_volumes(area.map(bounding_box)).await?;
                // the new volumes' polygons made for the map off the runtime's own threads: a
                // scan is megabytes of them
                tokio::task::spawn_blocking(move || arrivals(&volumes, &held)).await.map_err(|e| format!("measured radar: {e}"))
            })
            .into()
    }
}

/// The `--fixture` folder's answers (see the module documentation), read when asked for.
pub struct FixtureFeeds {
    dir: PathBuf,
    /// The recorded volumes, read at the first ask: each source's folder name and its volume.
    recorded: Option<Vec<(String, Arc<radar::Volume>)>>,
    /// The sources "fetched" so far (in view of an ask), held from then on.
    fetched: Vec<String>,
}

impl FixtureFeeds {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        FixtureFeeds { dir: dir.into(), recorded: None, fetched: Vec::new() }
    }

    /// Every folder of `dir` with a `volume.json`, read, in the order of the folders' names; a
    /// folder that does not read (DWD's raw files) is left out and logged.
    fn read_recorded(dir: &Path) -> Vec<(String, Arc<radar::Volume>)> {
        let mut folders: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|entries| entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.join("volume.json").is_file()).collect())
            .unwrap_or_default();
        folders.sort();
        folders
            .into_iter()
            .filter_map(|folder| match radar::read(&folder) {
                Ok(volume) => Some((volume.source.to_owned(), Arc::new(volume))),
                Err(e) => {
                    log::warn!("fixture radar volume {}: {e}", folder.display());
                    None
                }
            })
            .collect()
    }
}

impl RadarFeeds for FixtureFeeds {
    fn frame(&mut self) -> Answer<Result<Option<rainviewer::Frame>, String>> {
        let path = fixture_tiles(&self.dir).join(FIXTURE_FRAMES);
        Answer::ready(std::fs::read(&path).map_err(|e| format!("fixture {}: {e}", path.display())).and_then(|bytes| rainviewer::parse(&bytes)))
    }

    fn measures(&self) -> bool {
        true
    }

    fn volumes(&mut self, area: Option<Area>, held: Vec<VolumeKey>) -> Answer<Result<Vec<Arrived>, String>> {
        let dir = self.dir.join(FIXTURE_VOLUMES);
        let recorded = self.recorded.get_or_insert_with(|| Self::read_recorded(&dir));
        for (source, volume) in recorded.iter() {
            if !self.fetched.contains(source) && in_view(&volume.bounds, area.as_ref()) {
                self.fetched.push(source.clone());
            }
        }
        let volumes: Vec<Arc<radar::Volume>> =
            recorded.iter().filter(|(source, _)| self.fetched.contains(source)).map(|(_, volume)| Arc::clone(volume)).collect();
        Answer::ready(Ok(arrivals(&volumes, &held)))
    }
}

/// Which answers the radar takes.
enum Source {
    /// Chosen at the first poll: the fixture folder, else the network, else none.
    Auto,
    /// None: nothing is asked.
    Off,
    On(Box<dyn RadarFeeds>),
}

/// The weather radar: RainViewer's frame and the measured volumes, and when they are asked for.
pub struct Radar {
    source: Source,
    /// The map's tile sources, shared with the window's tile fetcher (`tile_fetcher::for_state`):
    /// RainViewer's template is set in it as each frame comes.
    sources: TileSources,

    // RainViewer
    /// `radarUrl`: the frame's tile URL template.
    url: Option<String>,
    /// When that frame was taken, Unix seconds.
    frame_time: Option<i64>,
    /// `radarCheckedAt`: when the frame list was last asked for (local ms; 0 before the first).
    checked_at: f64,
    frame_request: Option<Answer<Result<Option<rainviewer::Frame>, String>>>,

    // the measured volumes
    /// `measured`, in the backend's order.
    measured: Vec<Arc<MapVolume>>,
    /// `measuredCheckedAt`: when the last ask started, then when it ended.
    measured_checked_at: f64,
    /// `measuredLoading`: the ask out.
    volumes_request: Option<Answer<Result<Vec<Arrived>, String>>>,

    /// Moves on whenever the frame or the set of volumes changes.
    generation: u64,
}

impl Default for Radar {
    /// The radar of the `--fixture` folder, else the network, chosen at the first poll.
    fn default() -> Self {
        Radar::with_source(Source::Auto)
    }
}

impl std::fmt::Debug for Radar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let measured: Vec<VolumeKey> = self.measured.iter().map(|v| v.key()).collect();
        f.debug_struct("Radar")
            .field("url", &self.url)
            .field("frame_time", &self.frame_time)
            .field("checked_at", &self.checked_at)
            .field("asking_frame", &self.frame_request.is_some())
            .field("measured", &measured)
            .field("measured_checked_at", &self.measured_checked_at)
            .field("loading_measured", &self.volumes_request.is_some())
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl Radar {
    fn with_source(source: Source) -> Self {
        Radar {
            source,
            sources: TileSources::app(),
            url: None,
            frame_time: None,
            checked_at: 0.0,
            frame_request: None,
            measured: Vec::new(),
            measured_checked_at: 0.0,
            volumes_request: None,
            generation: 0,
        }
    }

    /// The radar on `feeds` instead of the fixture folder or the network; `None`: none at all.
    pub fn with_feeds(feeds: Option<Box<dyn RadarFeeds>>) -> Self {
        Radar::with_source(match feeds {
            Some(feeds) => Source::On(feeds),
            None => Source::Off,
        })
    }

    // ---------------------------------------------------------------- what the others read

    /// The RainViewer frame's tile URL template (`radarUrl`); `None`: no RainViewer to draw.
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// When the frame drawn was taken, Unix seconds (none without a frame, or when the list gave
    /// no time).
    pub fn frame_time(&self) -> Option<i64> {
        self.url.as_ref().and(self.frame_time)
    }

    /// The measured volumes (`measured`), in the backend's order.
    pub fn measured(&self) -> &[Arc<MapVolume>] {
        &self.measured
    }

    /// Whether an ask for the measured volumes is out (`measuredLoading`).
    pub fn loading_measured(&self) -> bool {
        self.volumes_request.is_some()
    }

    /// The map's tile sources, RainViewer's template among them once a frame has come. The
    /// window's tile fetcher reads tiles through this table (`tile_fetcher::for_state`).
    pub fn tile_sources(&self) -> &TileSources {
        &self.sources
    }

    /// Moves on whenever [`Self::url`] or [`Self::measured`] changes.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    // ---------------------------------------------------------------- the frame's step

    /// The source, once: the fixture folder, else the network, else none.
    fn resolve(&mut self, startup: &Startup, backend: Option<&Backend>, requests: Option<&Requests>) {
        if !matches!(self.source, Source::Auto) {
            return;
        }
        self.source = if let Some(dir) = &startup.fixture {
            Source::On(Box::new(FixtureFeeds::new(dir.clone())))
        } else if let Some(requests) = requests {
            Source::On(Box::new(BackendFeeds { requests: requests.clone(), backend: backend.cloned() }))
        } else {
            Source::Off
        };
    }

    /// This frame's step (see [`super::AppState::poll`] for the order, and the module
    /// documentation).
    pub fn poll(&mut self, cx: &mut Cx<'_>) {
        self.resolve(cx.startup, cx.backend, cx.requests);
        let now = cx.clocks.local_ms;
        let weather = cx.persisted.settings.weather;
        let before = self.generation;
        let Source::On(feeds) = &mut self.source else { return };
        let measures = feeds.measures();
        if weather {
            if rainviewer::is_due(self.checked_at, now) {
                self.checked_at = now;
                // an ask still out is dropped: the newer one's answer is the one kept
                self.frame_request = Some(feeds.frame());
            }
            // only the sources on screen are worth fetching; each is tens of megabytes
            if measures && self.volumes_request.is_none() && now - self.measured_checked_at > MEASURED_REFRESH_MS {
                self.measured_checked_at = now;
                let view = cx.controller.view();
                let [width, height] = cx.controller.size();
                let area = view_area(geo::View { longitude: view.longitude, latitude: view.latitude, zoom: view.zoom, pitch: view.pitch }, width, height);
                let held = self.measured.iter().map(|v| v.key()).collect();
                self.volumes_request = Some(feeds.volumes(area, held));
            }
        }
        self.take_frame();
        self.take_volumes(now);
        if self.generation != before {
            cx.wants.scene = true;
        }
        if weather {
            // the next asks, strictly after their intervals
            cx.wants.wake_at(self.checked_at + rainviewer::REFRESH_MS + 1.0);
            if measures && self.volumes_request.is_none() {
                cx.wants.wake_at(self.measured_checked_at + MEASURED_REFRESH_MS + 1.0);
            }
        }
    }

    /// `refreshRadar`'s answer, if it has come: the template set, then the frame handed over.
    fn take_frame(&mut self) {
        let Some(answer) = self.frame_request.as_ref().and_then(Answer::poll) else { return };
        self.frame_request = None;
        let frame = match answer {
            Ok(frame) => frame,
            Err(e) => {
                log::warn!("RainViewer unavailable: {e}");
                None
            }
        };
        let url = frame.as_ref().map(|f| f.url.clone());
        self.frame_time = frame.and_then(|f| f.time);
        if url == self.url {
            return;
        }
        if let Some(url) = &url {
            // before the scene carries it: no tile of the new frame is asked under the old URL
            self.sources.set_template(io::RAINVIEWER, url);
        }
        self.url = url;
        self.generation += 1;
    }

    /// `refreshMeasured`'s answer, if it has come: the volumes held kept, the set replaced unless
    /// it is the one held; the ask's end noted (`finally`).
    fn take_volumes(&mut self, now: f64) {
        let Some(answer) = self.volumes_request.as_ref().and_then(Answer::poll) else { return };
        self.volumes_request = None;
        self.measured_checked_at = now;
        let arrived = match answer {
            Ok(arrived) => arrived,
            Err(e) => {
                log::warn!("measured radar unavailable: {e}");
                return;
            }
        };
        // a scan already in hand is kept: the sources publish on their own schedules
        if arrived.len() == self.measured.len() && arrived.iter().all(|a| matches!(a, Arrived::Held(_))) {
            return;
        }
        let held = std::mem::take(&mut self.measured);
        self.measured = arrived
            .into_iter()
            .filter_map(|a| match a {
                Arrived::Held(key) => held.iter().find(|v| v.key() == key).cloned(),
                Arrived::New(volume) => Some(volume),
            })
            .collect();
        self.generation += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};

    use rustradar_backend::tiles::Tiles;
    use rustradar_core::model::{RadarBounds, RadarVolume};
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::persist::tests::TempDir;
    use crate::requests;
    use crate::state::AppState;

    /// The local clock at the first frame.
    const T0: f64 = 1_782_036_000_000.0;
    const SIZE: [f64; 2] = [1440.0, 900.0];
    const FRAME_URL: &str = "https://tilecache.rainviewer.com/v2/radar/47622fe604ca/256/{z}/{x}/{y}/2/1_1.png";
    const NEXT_URL: &str = "https://tilecache.rainviewer.com/v2/radar/5b1c0ffee000/256/{z}/{x}/{y}/2/1_1.png";

    type Framed = Result<Option<rainviewer::Frame>, String>;
    type Volumes = Result<Vec<Arrived>, String>;

    /// The asks the radar made, with the senders that answer them.
    #[derive(Default)]
    struct Log {
        frames: Vec<Sender<Framed>>,
        volumes: Vec<(Option<Area>, Vec<VolumeKey>, Sender<Volumes>)>,
        measures: bool,
    }

    /// Canned feeds: every ask waits until the test answers it.
    #[derive(Clone)]
    struct Canned(Arc<Mutex<Log>>);

    impl Canned {
        fn new(measures: bool) -> Self {
            Canned(Arc::new(Mutex::new(Log { measures, ..Default::default() })))
        }

        fn log(&self) -> std::sync::MutexGuard<'_, Log> {
            self.0.lock().unwrap()
        }

        fn frames_asked(&self) -> usize {
            self.log().frames.len()
        }

        fn volumes_asked(&self) -> usize {
            self.log().volumes.len()
        }

        fn answer_frame(&self, answer: Framed) {
            self.log().frames.last().expect("a frame ask").send(answer).unwrap();
        }

        fn answer_volumes(&self, answer: Volumes) {
            self.log().volumes.last().expect("a volumes ask").2.send(answer).unwrap();
        }

        /// The latest volumes ask: its area and the keys it said were held.
        fn last_volumes_ask(&self) -> (Option<Area>, Vec<VolumeKey>) {
            let log = self.log();
            let (area, held, _) = log.volumes.last().expect("a volumes ask");
            (*area, held.clone())
        }
    }

    impl RadarFeeds for Canned {
        fn frame(&mut self) -> Answer<Framed> {
            let (tx, rx) = mpsc::channel();
            self.log().frames.push(tx);
            rx.into()
        }

        fn measures(&self) -> bool {
            self.log().measures
        }

        fn volumes(&mut self, area: Option<Area>, held: Vec<VolumeKey>) -> Answer<Volumes> {
            let (tx, rx) = mpsc::channel();
            self.log().volumes.push((area, held, tx));
            rx.into()
        }
    }

    fn frame(url: &str, time: i64) -> Framed {
        Ok(Some(rainviewer::Frame { time: Some(time), url: url.into() }))
    }

    /// A small measured volume of `source` scanned at `time`.
    fn volume(source: &str, time: i64) -> Arc<MapVolume> {
        Arc::new(MapVolume {
            frame: RadarVolume {
                source: source.into(),
                time,
                bounds: RadarBounds { west: -130.0, south: 20.0, east: -60.0, north: 55.0 },
                levels: vec![],
                coverage_width: 2,
                coverage_height: 1,
            },
            coverage: vec![1, 0],
            ..Default::default()
        })
    }

    fn view(latitude: f64, longitude: f64, zoom: f64) -> ViewState {
        ViewState { longitude, latitude, zoom, bearing: 0.0, pitch: 0.0, position: None }
    }

    /// The whole app's state on canned radar feeds at `view`, the weather `on` or off.
    fn app(canned: &Canned, view: ViewState, on: bool) -> AppState {
        let mut s = AppState::new(Startup::default(), None, None, None, view, SIZE);
        s.persisted.settings.weather = on;
        s.radar = Radar::with_feeds(Some(Box::new(canned.clone())));
        s
    }

    #[test]
    fn nothing_is_asked_while_the_weather_is_off() {
        let canned = Canned::new(true);
        let mut s = app(&canned, view(52.0, 10.0, 4.5), false);
        for i in 0..5 {
            let wants = s.poll(T0 + f64::from(i) * 700_000.0);
            assert_eq!(wants.wake_at, None, "no ask to wake for");
        }
        assert_eq!((canned.frames_asked(), canned.volumes_asked()), (0, 0));
        assert_eq!(s.radar.url(), None);
    }

    #[test]
    fn the_frame_list_is_asked_at_once_then_after_ten_minutes_from_the_ask() {
        let canned = Canned::new(false);
        let mut s = app(&canned, view(52.0, 10.0, 4.5), true);
        let wants = s.poll(T0);
        assert_eq!(canned.frames_asked(), 1, "never asked: due at once");
        assert_eq!(s.radar.generation(), 0, "nothing has changed yet");
        assert_eq!(wants.wake_at, Some(T0 + 600_001.0), "the next ask, strictly after ten minutes");
        assert_eq!(s.radar.url(), None, "no frame before the answer");
        // the answer, 2 s later: the frame drawn and a new scene asked for
        canned.answer_frame(frame(FRAME_URL, 1_794_328_200));
        let wants = s.poll(T0 + 2_000.0);
        assert!(wants.scene);
        assert_eq!(s.radar.url(), Some(FRAME_URL));
        assert_eq!(s.radar.frame_time(), Some(1_794_328_200));
        let generation = s.radar.generation();
        // the ten minutes count from the ask's start, not its answer
        s.poll(T0 + 600_000.0);
        assert_eq!(canned.frames_asked(), 1, "exactly ten minutes is not more than ten");
        s.poll(T0 + 600_001.0);
        assert_eq!(canned.frames_asked(), 2);
        // the same frame again changes nothing
        canned.answer_frame(frame(FRAME_URL, 1_794_328_200));
        let wants = s.poll(T0 + 600_100.0);
        assert!(!wants.scene);
        assert_eq!(s.radar.generation(), generation);
        // a newer frame replaces it
        s.poll(T0 + 1_200_002.0);
        assert_eq!(canned.frames_asked(), 3);
        canned.answer_frame(frame(NEXT_URL, 1_794_328_800));
        assert!(s.poll(T0 + 1_200_100.0).scene);
        assert_eq!(s.radar.url(), Some(NEXT_URL));
        assert!(s.radar.generation() > generation);
    }

    #[test]
    fn a_failed_ask_or_a_list_without_a_frame_draws_no_rainviewer_until_the_next() {
        let canned = Canned::new(false);
        let mut s = app(&canned, view(52.0, 10.0, 4.5), true);
        s.poll(T0);
        canned.answer_frame(frame(FRAME_URL, 1));
        s.poll(T0 + 1.0);
        assert_eq!(s.radar.url(), Some(FRAME_URL));
        // `catch { radarUrl = null }`
        s.poll(T0 + 600_002.0);
        canned.answer_frame(Err("can't reach RainViewer".into()));
        assert!(s.poll(T0 + 600_003.0).scene);
        assert_eq!((s.radar.url(), s.radar.frame_time()), (None, None));
        // not asked again before ten minutes have passed since that ask
        s.poll(T0 + 900_000.0);
        assert_eq!(canned.frames_asked(), 2);
        s.poll(T0 + 1_200_003.0);
        assert_eq!(canned.frames_asked(), 3);
        canned.answer_frame(frame(FRAME_URL, 2));
        s.poll(T0 + 1_200_004.0);
        assert_eq!(s.radar.url(), Some(FRAME_URL));
        // a list with no past frame: no frame
        s.poll(T0 + 1_800_005.0);
        canned.answer_frame(Ok(None));
        s.poll(T0 + 1_800_006.0);
        assert_eq!(s.radar.url(), None);
    }

    #[test]
    fn an_answer_is_taken_with_the_weather_turned_off_meanwhile_and_kept_for_its_return() {
        let canned = Canned::new(false);
        let mut s = app(&canned, view(52.0, 10.0, 4.5), true);
        s.poll(T0);
        s.persisted.settings.weather = false;
        canned.answer_frame(frame(FRAME_URL, 1));
        let wants = s.poll(T0 + 500.0);
        assert_eq!(s.radar.url(), Some(FRAME_URL), "today's promise lands all the same");
        assert_eq!(wants.wake_at, None, "with the weather off nothing is asked");
        // on again within ten minutes: the frame held is drawn, no new ask
        s.persisted.settings.weather = true;
        s.poll(T0 + 60_000.0);
        assert_eq!(canned.frames_asked(), 1);
        assert_eq!(s.radar.url(), Some(FRAME_URL));
    }

    #[test]
    fn the_template_is_set_before_the_frame_is_handed_over() {
        let canned = Canned::new(false);
        let mut s = app(&canned, view(52.0, 10.0, 4.5), true);
        assert!(!s.radar.tile_sources().contains(io::RAINVIEWER), "no RainViewer source before a frame");
        s.poll(T0);
        canned.answer_frame(frame(FRAME_URL, 1));
        s.poll(T0 + 1.0);
        let dir = TempDir::new("radar-sources");
        let tiles = Tiles::new(&dir.0).expect("a tile cache");
        let rt = requests::runtime().expect("runtime");
        let source = rt.block_on(s.radar.tile_sources().resolve(&tiles, io::RAINVIEWER)).expect("the source");
        assert_eq!(source.template, FRAME_URL);
        assert_eq!(source.url(6, 35, 22), "https://tilecache.rainviewer.com/v2/radar/47622fe604ca/256/6/35/22/2/1_1.png");
        // a new frame: the new template by the time the scene can carry it
        s.poll(T0 + 600_002.0);
        canned.answer_frame(frame(NEXT_URL, 2));
        s.poll(T0 + 600_003.0);
        assert_eq!(s.radar.url(), Some(NEXT_URL));
        assert_eq!(rt.block_on(s.radar.tile_sources().resolve(&tiles, io::RAINVIEWER)).unwrap().template, NEXT_URL);
        // the app's other sources are in the same table
        assert!(s.radar.tile_sources().contains(io::ESRI) && s.radar.tile_sources().contains(io::OPENFREEMAP));
    }

    #[test]
    fn the_volumes_are_asked_for_the_view_two_minutes_after_the_last_ask_ended() {
        let canned = Canned::new(true);
        let ohio = view(40.0, -84.0, 5.0);
        let mut s = app(&canned, ohio, true);
        let wants = s.poll(T0);
        assert_eq!(canned.volumes_asked(), 1, "never asked: due at once");
        let expected = view_area(geo::View { longitude: -84.0, latitude: 40.0, zoom: 5.0, pitch: 0.0 }, SIZE[0], SIZE[1]);
        assert!(expected.is_some());
        assert_eq!(canned.last_volumes_ask(), (expected, vec![]), "the view's area, nothing held");
        // the ask out: only RainViewer's next ask to wake for
        assert_eq!(wants.wake_at, Some(T0 + 600_001.0));
        // a slow load must not race the next poll
        s.poll(T0 + 200_000.0);
        assert_eq!(canned.volumes_asked(), 1);
        assert!(s.radar.loading_measured());
        canned.answer_volumes(Ok(vec![Arrived::New(volume("mrms", 100))]));
        let wants = s.poll(T0 + 210_000.0);
        assert!(wants.scene);
        assert!(!s.radar.loading_measured());
        assert_eq!(s.radar.measured().iter().map(|v| v.key()).collect::<Vec<_>>(), vec![("mrms".to_owned(), 100)]);
        // two minutes from the answer, not from the ask
        assert_eq!(wants.wake_at, Some(T0 + 210_000.0 + 120_001.0));
        s.poll(T0 + 330_000.0);
        assert_eq!(canned.volumes_asked(), 1, "exactly two minutes is not more than two");
        // the camera has moved: the new view's area, and the volume held named
        s.controller.set_view(view(35.0, -97.0, 4.0));
        s.poll(T0 + 330_001.0);
        assert_eq!(canned.volumes_asked(), 2);
        let expected = view_area(geo::View { longitude: -97.0, latitude: 35.0, zoom: 4.0, pitch: 0.0 }, SIZE[0], SIZE[1]);
        assert_eq!(canned.last_volumes_ask(), (expected, vec![("mrms".to_owned(), 100)]));
        // most of the globe in sight: everywhere
        canned.answer_volumes(Ok(vec![Arrived::Held(("mrms".into(), 100))]));
        s.poll(T0 + 330_002.0);
        s.controller.set_view(view(35.0, -97.0, 1.5));
        s.poll(T0 + 450_003.0);
        assert_eq!(canned.last_volumes_ask().0, None);
    }

    #[test]
    fn a_held_scan_is_kept_and_an_unchanged_set_changes_nothing() {
        let canned = Canned::new(true);
        let mut s = app(&canned, view(40.0, -84.0, 5.0), true);
        s.poll(T0);
        let (mrms, dwd) = (volume("mrms", 100), volume("dwd", 200));
        canned.answer_volumes(Ok(vec![Arrived::New(Arc::clone(&mrms)), Arrived::New(Arc::clone(&dwd))]));
        s.poll(T0 + 1.0);
        let generation = s.radar.generation();
        // the same scans in another order: nothing changes (`held.every(Boolean)`)
        s.poll(T0 + 120_002.0);
        assert_eq!(canned.last_volumes_ask().1, vec![("mrms".to_owned(), 100), ("dwd".to_owned(), 200)]);
        canned.answer_volumes(Ok(vec![Arrived::Held(("dwd".into(), 200)), Arrived::Held(("mrms".into(), 100))]));
        let wants = s.poll(T0 + 120_003.0);
        assert!(!wants.scene);
        assert_eq!(s.radar.generation(), generation);
        assert!(Arc::ptr_eq(&s.radar.measured()[0], &mrms), "the set as it was, in its order");
        // a new MRMS scan: the DWD volume held is the same one, the new one comes in, the
        // answer's order kept
        s.poll(T0 + 240_004.0);
        let next = volume("mrms", 340);
        canned.answer_volumes(Ok(vec![Arrived::Held(("dwd".into(), 200)), Arrived::New(Arc::clone(&next))]));
        assert!(s.poll(T0 + 240_005.0).scene);
        let measured = s.radar.measured();
        assert_eq!(measured.len(), 2);
        assert!(Arc::ptr_eq(&measured[0], &dwd) && Arc::ptr_eq(&measured[1], &next));
        // a source gone from the answer: the set is the answer's
        s.poll(T0 + 360_006.0);
        canned.answer_volumes(Ok(vec![Arrived::Held(("mrms".into(), 340))]));
        s.poll(T0 + 360_007.0);
        assert_eq!(s.radar.measured().len(), 1);
        assert!(Arc::ptr_eq(&s.radar.measured()[0], &next));
        // none at all: the set empties
        s.poll(T0 + 480_008.0);
        canned.answer_volumes(Ok(vec![]));
        assert!(s.poll(T0 + 480_009.0).scene);
        assert!(s.radar.measured().is_empty());
    }

    #[test]
    fn a_failed_ask_keeps_the_volumes_and_asks_again_two_minutes_after_it() {
        let canned = Canned::new(true);
        let mut s = app(&canned, view(40.0, -84.0, 5.0), true);
        s.poll(T0);
        canned.answer_volumes(Ok(vec![Arrived::New(volume("mrms", 100))]));
        s.poll(T0 + 1.0);
        s.poll(T0 + 120_002.0);
        canned.answer_volumes(Err("no radar volume could be loaded".into()));
        let wants = s.poll(T0 + 125_000.0);
        assert!(!wants.scene);
        assert_eq!(s.radar.measured().len(), 1, "the old set is kept");
        assert_eq!(wants.wake_at, Some(T0 + 125_000.0 + 120_001.0), "`finally`: from the failure's end");
        s.poll(T0 + 245_001.0);
        assert_eq!(canned.volumes_asked(), 3);
    }

    #[test]
    fn without_a_backend_no_volumes_are_asked_for() {
        let canned = Canned::new(false);
        let mut s = app(&canned, view(40.0, -84.0, 5.0), true);
        let wants = s.poll(T0);
        assert_eq!((canned.frames_asked(), canned.volumes_asked()), (1, 0));
        assert_eq!(wants.wake_at, Some(T0 + 600_001.0), "only RainViewer's next ask");
        s.poll(T0 + 10_000_000.0);
        assert_eq!(canned.volumes_asked(), 0);
        // and with no source at all (no runtime): nothing
        let mut off = AppState::new(Startup::default(), None, None, None, view(40.0, -84.0, 5.0), SIZE);
        off.persisted.settings.weather = true;
        assert_eq!(off.poll(T0).wake_at, None);
        assert_eq!((off.radar.url(), off.radar.measured().len()), (None, 0));
    }

    /// The committed fixtures' folder (`crates/map/tests/data`).
    fn committed() -> PathBuf {
        io::fixture_tiles_dir().parent().expect("the data folder").to_owned()
    }

    #[test]
    fn the_fixture_folder_gives_the_committed_frame_and_the_recorded_volume_once_in_view() {
        let startup = Startup { fixture: Some(committed()), ..Default::default() };
        // Germany close in: RainViewer's frame at the first frame, no measured volume in view (at
        // V2's zoom most of the globe is in sight, and every source is fetched)
        let mut s = AppState::new(startup.clone(), None, None, None, view(52.0, 10.0, 7.0), SIZE);
        s.persisted.settings.weather = true;
        assert!(s.poll(T0).scene);
        assert_eq!(s.radar.url(), Some(FRAME_URL), "the frame the committed tiles are of (their NOTICE)");
        assert_eq!(s.radar.frame_time(), Some(1_794_328_200));
        assert!(s.radar.measured().is_empty(), "MRMS is not in view over Europe");
        assert!(!s.radar.loading_measured());
        // over the Ohio Valley: the recorded MRMS scan, as the backend held it
        let mut s = AppState::new(startup, None, None, None, view(39.0, -84.0, 5.3), SIZE);
        s.persisted.settings.weather = true;
        s.poll(T0);
        let measured = s.radar.measured();
        assert_eq!(measured.iter().map(|v| v.key()).collect::<Vec<_>>(), vec![("mrms".to_owned(), 1_794_446_561)]);
        let mrms = Arc::clone(&measured[0]);
        assert_eq!(mrms.frame.levels.len(), 12);
        assert_eq!(mrms.levels.len(), mrms.frame.levels.len(), "one polygon list per height named");
        assert_eq!(mrms.coverage.len(), 700 * 350);
        assert!(!mrms.ground.is_empty());
        // held from then on: the next ask, even over Europe, names it and keeps it
        s.controller.set_view(view(52.0, 10.0, 7.0));
        s.poll(T0 + 120_001.0);
        assert_eq!(s.radar.measured().len(), 1);
        assert!(Arc::ptr_eq(&s.radar.measured()[0], &mrms));
    }

    #[test]
    fn a_backend_volume_is_made_for_the_map_as_its_blobs_were_parsed() {
        let dir = committed().join("radar/mrms");
        let volume = radar::read(&dir).expect("the recorded MRMS volume");
        let made = map_volume(&volume);
        assert_eq!(made.frame, volume.info());
        assert_eq!(made.coverage, volume.coverage);
        // as `parseVolumePolygons` reads the blob `radar_image` serves for each layer
        let parsed = |name: &str| rustradar_scene::radar::mesh::parse_volume_polygons(&volume.image(name).expect("a blob")).expect("a blob that reads");
        assert_eq!(made.ground, parsed("ground"));
        assert_eq!(made.levels.len(), volume.info().levels.len());
        for (km, polygons) in volume.info().levels.iter().zip(&made.levels) {
            assert_eq!(polygons, &parsed(&rustradar_core::num::js_string(f64::from(*km))), "{km} km");
        }
        // a held volume is not made again
        let volumes = [Arc::new(volume)];
        let key = ("mrms".to_owned(), volumes[0].time);
        assert!(matches!(&arrivals(&volumes, std::slice::from_ref(&key))[..], [Arrived::Held(k)] if *k == key));
        assert!(matches!(&arrivals(&volumes, &[("mrms".to_owned(), 1)])[..], [Arrived::New(v)] if v.key() == key));
    }

    #[test]
    fn a_source_is_in_view_as_the_backend_decides() {
        let mrms = RadarBounds { west: -130.0, south: 20.0, east: -60.0, north: 55.0 };
        let area = |west, south, east, north| Area { west, south, east, north };
        assert!(in_view(&mrms, None));
        assert!(in_view(&mrms, Some(&area(-90.0, 35.0, -80.0, 45.0))));
        assert!(!in_view(&mrms, Some(&area(5.0, 45.0, 15.0, 55.0))), "Europe");
        assert!(!in_view(&mrms, Some(&area(-90.0, 56.0, -80.0, 60.0))), "north of it");
        assert!(in_view(&mrms, Some(&area(-60.0, 20.0, -50.0, 30.0))), "an edge shared counts");
        // across the antimeridian: two spans
        assert!(in_view(&mrms, Some(&area(170.0, 30.0, -120.0, 40.0))));
        assert!(!in_view(&mrms, Some(&area(170.0, 30.0, -140.0, 40.0))));
    }
}
