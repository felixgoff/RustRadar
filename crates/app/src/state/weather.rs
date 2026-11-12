//! Live weather for the realistic map's sky (item R11; doc 06 §3.10, doc 04 §1–2,
//! `+page.svelte:206-243`): the weather where the map is looking, asked for again every ten
//! minutes or after a move of 15 km, and after a failure not for another two minutes.
//!
//! While the sky follows the day's weather (`weatherLive`: the realistic basemap with the *sky*
//! setting on *live*) today's page calls `refreshWeather(lat, lon)` at the view's centre on every
//! change of the view, and every 60 s from a timer started as the weather goes live. That call
//! asks only when no ask is out (`weatherBusy`) and either nothing was asked yet, or the last ask is
//! older than [`WEATHER_REFRESH_MS`] ([`WEATHER_RETRY_MS`] after a failure), or it succeeded and
//! the view has moved at least [`WEATHER_MOVE_KM`] from where it was asked (great-circle,
//! `flightpath.ts`'s `distanceKm`). An ask is dated when its answer comes (`Date.now()` after the
//! `await`), with the place it was asked for. [`Weather::poll`] does the same once a frame: a
//! changed camera is the view's change, the timer's ticks are kept on the local clock (each 60 s
//! from when the weather went live, as `setInterval` ticks), and the frame loop is woken for the
//! next tick ([`super::Wants::wake_at`]).
//!
//! An answer is taken whether or not the weather is still live (today's promise lands all the
//! same): a weather replaces the one held (`liveWeather`) and asks for a new scene, so the light
//! changes at once; a failure keeps it. What the others read:
//! - [`Weather::conditions`]: the live weather as light (`conditionsOf(liveWeather)`, `None` until
//!   one has come), which `scene_builder::realistic` hands the map's sky for *live*
//!   (`skyConditions`, `+page.svelte:243`);
//! - [`Weather::loaded`]: one has come (`liveWeather`), for the attribution's "Weather: MET
//!   Norway" while the sky is live.
//!
//! **Sources** (chosen at the first poll): the `--fixture` folder ([`FIXTURE_FORECAST`]: MET
//! Norway's Locationforecast answer as it sends it, read through `backend::weather::parse`; a
//! folder without one answers each ask with a failure, so a shot stays on a fine day), else MET
//! Norway through `backend::weather::at` on the runtime of the [`Requests`] (today's `weather_at`
//! command, with its ten-minute cache per 0.1° cell), else none (no runtime: nothing is asked).
//! [`Weather::with_feed`] sets one instead (tests, shots without a fixture folder).
//!
//! Deviations: none in what is asked and when. Today's plain-browser fallback (`fetchWeather`
//! straight from MET Norway without the backend) is not needed: the app always has the backend's
//! code.

use std::path::PathBuf;

use rustradar_core::geo::Basemap;
use rustradar_core::geo::sphere::distance_km;
use rustradar_core::model::Weather as Forecast;
use rustradar_core::weather::{Conditions, Sky, conditions_of};
use rustradar_scene::camera::ViewState;

use super::aircraft::Answer;
use super::{Cx, Startup};
use crate::requests::Requests;

/// The live weather is asked for again after this long, ms (`WEATHER_REFRESH_MS`, `+page:210`).
pub const WEATHER_REFRESH_MS: f64 = 10.0 * 60_000.0;
/// After a failure, not before this long, ms (`WEATHER_RETRY_MS`, `+page:211`).
pub const WEATHER_RETRY_MS: f64 = 2.0 * 60_000.0;
/// Or after the view has moved this far from where it was asked, km (`WEATHER_MOVE_KM`).
pub const WEATHER_MOVE_KM: f64 = 15.0;
/// The timer that looks again while the weather is live, ms (`setInterval(…, 60_000)`).
pub const WEATHER_TICK_MS: f64 = 60_000.0;

/// The `--fixture` folder's weather: MET Norway's Locationforecast `complete` answer, as it sends
/// it.
pub const FIXTURE_FORECAST: &str = "weather/locationforecast.json";

/// Where the weather comes from: the network, the `--fixture` folder, or a test's.
pub trait WeatherFeed: Send {
    /// `fetchWeather(lat, lon)`: the weather now at that place.
    fn weather_at(&mut self, lat: f64, lon: f64) -> Answer<Result<Forecast, String>>;
}

/// MET Norway through the backend, on the runtime of `requests`.
struct Network(Requests);

impl WeatherFeed for Network {
    fn weather_at(&mut self, lat: f64, lon: f64) -> Answer<Result<Forecast, String>> {
        self.0.request(rustradar_backend::weather::at(lat, lon)).into()
    }
}

/// The `--fixture` folder's recorded forecast, read at each ask.
pub struct FixtureFeed {
    dir: PathBuf,
}

impl FixtureFeed {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        FixtureFeed { dir: dir.into() }
    }
}

impl WeatherFeed for FixtureFeed {
    fn weather_at(&mut self, _lat: f64, _lon: f64) -> Answer<Result<Forecast, String>> {
        let path = self.dir.join(FIXTURE_FORECAST);
        let answer = std::fs::read(&path)
            .map_err(|e| format!("fixture {}: {e}", path.display()))
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| format!("fixture {}: {e}", path.display())))
            .and_then(|body| rustradar_backend::weather::parse(&body).ok_or_else(|| format!("fixture {}: no weather in it", path.display())));
        Answer::ready(answer)
    }
}

/// Which feed the weather takes.
enum Source {
    /// Chosen at the first poll: the fixture folder, else the network, else none.
    Auto,
    /// None: nothing is asked.
    Off,
    On(Box<dyn WeatherFeed>),
}

/// The last ask (`weatherAsked`): where, when its answer came (local ms), and whether it was one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Asked {
    pub lat: f64,
    pub lon: f64,
    pub at: f64,
    pub ok: bool,
}

/// The ask out (`weatherBusy`): where it was asked for, and its answer on the way.
struct Out {
    lat: f64,
    lon: f64,
    answer: Answer<Result<Forecast, String>>,
}

/// Live weather for the realistic map's sky.
pub struct Weather {
    source: Source,
    /// `liveWeather`: the last weather that came.
    live: Option<Forecast>,
    asked: Option<Asked>,
    out: Option<Out>,
    /// The camera at the last poll while live (`None`: not live), to see the view change.
    seen: Option<ViewState>,
    /// The live timer's next tick (local ms), while live.
    next_tick: Option<f64>,
}

impl Default for Weather {
    /// The weather of the `--fixture` folder, else the network, chosen at the first poll.
    fn default() -> Self {
        Weather::with_source(Source::Auto)
    }
}

impl std::fmt::Debug for Weather {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Weather")
            .field("live", &self.live)
            .field("asked", &self.asked)
            .field("busy", &self.out.is_some())
            .field("next_tick", &self.next_tick)
            .finish_non_exhaustive()
    }
}

/// Whether the sky follows the day's weather (`weatherLive`): the realistic basemap with the sky on
/// *live*.
pub fn weather_live(basemap: Basemap, sky: Sky) -> bool {
    basemap == Basemap::Realistic && sky == Sky::Live
}

impl Weather {
    fn with_source(source: Source) -> Self {
        Weather { source, live: None, asked: None, out: None, seen: None, next_tick: None }
    }

    /// The weather on `feed` instead of the fixture folder or the network; `None`: none at all.
    pub fn with_feed(feed: Option<Box<dyn WeatherFeed>>) -> Self {
        Weather::with_source(match feed {
            Some(feed) => Source::On(feed),
            None => Source::Off,
        })
    }

    // ---------------------------------------------------------------- what the others read

    /// The live weather (`liveWeather`), once one has come.
    pub fn live(&self) -> Option<&Forecast> {
        self.live.as_ref()
    }

    /// Whether a live weather has come (`liveWeather` is set).
    pub fn loaded(&self) -> bool {
        self.live.is_some()
    }

    /// The live weather as light (`conditionsOf(liveWeather)`), `None` until one has come.
    pub fn conditions(&self) -> Option<Conditions> {
        self.live.as_ref().map(conditions_of)
    }

    /// The last ask, once its answer has come (`weatherAsked`).
    pub fn asked(&self) -> Option<Asked> {
        self.asked
    }

    /// Whether an ask is out (`weatherBusy`).
    pub fn busy(&self) -> bool {
        self.out.is_some()
    }

    // ---------------------------------------------------------------- the frame's step

    /// The source, once: the fixture folder, else the network, else none.
    fn resolve(&mut self, startup: &Startup, requests: Option<&Requests>) {
        if !matches!(self.source, Source::Auto) {
            return;
        }
        self.source = if let Some(dir) = &startup.fixture {
            Source::On(Box::new(FixtureFeed::new(dir.clone())))
        } else if let Some(requests) = requests {
            Source::On(Box::new(Network(requests.clone())))
        } else {
            Source::Off
        };
    }

    /// This frame's step (see [`super::AppState::poll`] for the order, and the module
    /// documentation): the answer, if it has come; then, while the weather is live, an ask at the
    /// view's centre as the view changes, as the weather goes live and at each tick of its timer.
    pub fn poll(&mut self, cx: &mut Cx<'_>) {
        self.resolve(cx.startup, cx.requests);
        let now = cx.clocks.local_ms;
        if self.take_answer(now) {
            cx.wants.scene = true;
        }
        let settings = &cx.persisted.settings;
        if !weather_live(settings.basemap, settings.sky) {
            // the effects end: no timer, and going live again starts afresh
            self.seen = None;
            self.next_tick = None;
            return;
        }
        let view = cx.controller.view();
        // the view's effect runs as the weather goes live and at each change of the view
        let mut refresh = self.seen != Some(view);
        self.seen = Some(view);
        match self.next_tick {
            // the timer starts with the effect
            None => self.next_tick = Some(now + WEATHER_TICK_MS),
            Some(tick) if now >= tick => {
                refresh = true;
                // (a late tick is one tick, as the browser's are)
                let mut next = tick + WEATHER_TICK_MS;
                while next <= now {
                    next += WEATHER_TICK_MS;
                }
                self.next_tick = Some(next);
            }
            Some(_) => {}
        }
        if refresh {
            self.refresh(view.latitude, view.longitude, now);
        }
        if let Some(tick) = self.next_tick {
            cx.wants.wake_at(tick);
        }
    }

    /// `refreshWeather(lat, lon)` at the local clock `now`: an ask unless one is out, or the last
    /// is recent (ten minutes after a weather, two after a failure) and either failed or was asked
    /// within 15 km of here.
    fn refresh(&mut self, lat: f64, lon: f64, now: f64) {
        let Source::On(feed) = &mut self.source else { return };
        if self.out.is_some() {
            return;
        }
        if let Some(last) = self.asked {
            let age = now - last.at;
            let moved = distance_km([last.lon, last.lat], [lon, lat]);
            let wait = if last.ok { WEATHER_REFRESH_MS } else { WEATHER_RETRY_MS };
            if age < wait && (moved < WEATHER_MOVE_KM || !last.ok) {
                return;
            }
        }
        self.out = Some(Out { lat, lon, answer: feed.weather_at(lat, lon) });
    }

    /// The answer to the ask out, if it has come: a weather is kept (and the ask dated now, a
    /// success), a failure keeps the one held. Whether the weather changed.
    fn take_answer(&mut self, now: f64) -> bool {
        let Some(answer) = self.out.as_ref().and_then(|out| out.answer.poll()) else { return false };
        let Out { lat, lon, .. } = self.out.take().expect("an ask is out");
        match answer {
            Ok(weather) => {
                self.asked = Some(Asked { lat, lon, at: now, ok: true });
                let changed = self.live.as_ref() != Some(&weather);
                self.live = Some(weather);
                changed
            }
            Err(e) => {
                log::debug!("live weather: {e}");
                self.asked = Some(Asked { lat, lon, at: now, ok: false });
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};

    use rustradar_core::weather::PRESETS;

    use super::*;
    use crate::persist::tests::TempDir;
    use crate::state::AppState;

    /// The local clock at the first frame.
    const T0: f64 = 1_782_036_000_000.0;
    const SIZE: [f64; 2] = [1440.0, 900.0];

    type Reply = Result<Forecast, String>;
    /// An ask: where, and the sender that answers it.
    type Ask = (f64, f64, Sender<Reply>);

    /// The asks made, in order.
    #[derive(Clone, Default)]
    struct Canned(Arc<Mutex<Vec<Ask>>>);

    impl Canned {
        fn asks(&self) -> Vec<(f64, f64)> {
            self.0.lock().unwrap().iter().map(|(lat, lon, _)| (*lat, *lon)).collect()
        }

        fn answer(&self, answer: Reply) {
            self.0.lock().unwrap().last().expect("an ask").2.send(answer).unwrap();
        }
    }

    impl WeatherFeed for Canned {
        fn weather_at(&mut self, lat: f64, lon: f64) -> Answer<Reply> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().push((lat, lon, tx));
            rx.into()
        }
    }

    fn rain() -> Forecast {
        Forecast { cloud: 1.0, cloud_low: 0.9, fog: 0.0, precipitation: 1.4, symbol: "rain".into(), temperature: 8.0 }
    }

    fn clear() -> Forecast {
        Forecast { cloud: 0.1, cloud_low: 0.0, fog: 0.0, precipitation: 0.0, symbol: "clearsky_day".into(), temperature: 21.0 }
    }

    fn view(latitude: f64, longitude: f64) -> ViewState {
        ViewState { longitude, latitude, zoom: 13.0, bearing: 0.0, pitch: 0.0, position: None }
    }

    /// The whole app's state on canned weather at Stockholm, realistic, the sky live (the default).
    fn app(canned: &Canned) -> AppState {
        let mut s = AppState::new(Startup { basemap: Some(Basemap::Realistic), ..Default::default() }, None, None, None, view(59.33, 18.07), SIZE);
        assert_eq!(s.persisted.settings.sky, Sky::Live, "today's default");
        s.weather = Weather::with_feed(Some(Box::new(canned.clone())));
        s
    }

    /// The point `km` east of `view` along its parallel (near enough for these distances).
    fn east(v: ViewState, km: f64) -> ViewState {
        let dlon = km / (111.195 * v.latitude.to_radians().cos());
        ViewState { longitude: v.longitude + dlon, ..v }
    }

    #[test]
    fn the_weather_is_asked_where_the_map_looks_as_it_goes_live_and_lights_the_sky() {
        let canned = Canned::default();
        let mut s = app(&canned);
        let wants = s.poll(T0);
        assert_eq!(canned.asks(), [(59.33, 18.07)]);
        assert!(s.weather.busy());
        assert_eq!(wants.wake_at, Some(T0 + WEATHER_TICK_MS), "the timer's first tick");
        assert_eq!(s.weather.conditions(), None, "none until it has come: a fine day");
        // nothing more while the ask is out, however the view moves
        s.controller.set_view(east(view(59.33, 18.07), 40.0));
        s.poll(T0 + 500.0);
        assert_eq!(canned.asks().len(), 1);
        // the answer: the light takes it at once
        canned.answer(Ok(rain()));
        let wants = s.poll(T0 + 1_000.0);
        assert!(wants.scene);
        assert!(s.weather.loaded());
        assert_eq!(s.weather.conditions(), Some(conditions_of(&rain())));
        assert_eq!(s.weather.asked(), Some(Asked { lat: 59.33, lon: 18.07, at: T0 + 1_000.0, ok: true }), "dated when it came");
        // the same answer again changes nothing to draw
        s.controller.set_view(view(59.33, 18.07));
        s.weather.asked = None;
        s.poll(T0 + 2_000.0);
        canned.answer(Ok(rain()));
        assert!(!s.poll(T0 + 2_500.0).scene);
    }

    #[test]
    fn it_is_asked_again_after_ten_minutes_or_fifteen_km() {
        let canned = Canned::default();
        let mut s = app(&canned);
        s.poll(T0);
        canned.answer(Ok(clear()));
        s.poll(T0 + 1_000.0);
        let start = view(59.33, 18.07);
        // moving 14 km within ten minutes: not asked
        s.controller.set_view(east(start, 14.0));
        s.poll(T0 + 60_000.0);
        assert_eq!(canned.asks().len(), 1);
        // 15 km: asked at once, where the map looks now
        let moved = east(start, 15.1);
        s.controller.set_view(moved);
        s.poll(T0 + 61_000.0);
        assert_eq!(canned.asks().len(), 2);
        assert!((canned.asks()[1].1 - moved.longitude).abs() < 1e-12);
        canned.answer(Ok(rain()));
        s.poll(T0 + 62_000.0);
        // standing still: the timer's ticks ask once ten minutes have passed since that answer
        let mut asked_at = None;
        let mut t = T0 + 62_000.0;
        while t < T0 + 62_000.0 + 12.0 * 60_000.0 {
            t += 1_000.0;
            s.poll(t);
            if canned.asks().len() == 3 && asked_at.is_none() {
                asked_at = Some(t);
            }
        }
        // the ticks fall every 60 s from the first poll: 600 000 ms after the answer at +62 s is
        // +662 s, so the tick at +720 s asks
        assert_eq!(asked_at, Some(T0 + 720_000.0));
    }

    #[test]
    fn after_a_failure_it_waits_two_minutes_wherever_the_view_goes() {
        let canned = Canned::default();
        let mut s = app(&canned);
        let held = Some(conditions_of(&clear()));
        s.poll(T0);
        canned.answer(Ok(clear()));
        s.poll(T0 + 1.0);
        // at exactly ten minutes from the answer it is not yet due: the tick after asks
        s.poll(T0 + 600_000.0);
        assert_eq!(canned.asks().len(), 1);
        s.poll(T0 + 660_000.0);
        assert_eq!(canned.asks().len(), 2);
        // it fails
        canned.answer(Err("can't reach MET Norway".into()));
        assert!(!s.poll(T0 + 661_000.0).scene);
        assert_eq!(s.weather.conditions(), held, "the weather held is kept");
        assert_eq!(s.weather.asked(), Some(Asked { lat: 59.33, lon: 18.07, at: T0 + 661_000.0, ok: false }));
        // far away within the two minutes, and the ticks in them: not asked
        s.controller.set_view(view(48.85, 2.35));
        for t in [700_000.0, 720_000.0, 780_000.0] {
            s.poll(T0 + t);
            assert_eq!(canned.asks().len(), 2, "+{t} ms");
        }
        // two minutes after the failure came, the next change of view asks, where the map looks
        s.controller.set_view(view(48.86, 2.35));
        s.poll(T0 + 781_000.0);
        assert_eq!(canned.asks(), [(59.33, 18.07), (59.33, 18.07), (48.86, 2.35)]);
    }

    #[test]
    fn nothing_is_asked_unless_the_realistic_sky_is_live() {
        for (basemap, sky) in [(Basemap::Dark, Sky::Live), (Basemap::Satellite, Sky::Live), (Basemap::Realistic, Sky::Rain)] {
            let canned = Canned::default();
            let mut s = app(&canned);
            s.persisted.settings.basemap = basemap;
            s.persisted.settings.sky = sky;
            for i in 0..4 {
                s.controller.set_view(east(view(59.33, 18.07), f64::from(i) * 20.0));
                s.poll(T0 + f64::from(i) * 700_000.0);
                assert_eq!(s.weather.next_tick, None, "no timer: {basemap:?} {sky:?}");
            }
            assert!(canned.asks().is_empty(), "{basemap:?} {sky:?}");
            assert!(!weather_live(basemap, sky));
        }
        // an answer that comes after the sky stopped being live is kept all the same
        let canned = Canned::default();
        let mut s = app(&canned);
        s.poll(T0);
        s.persisted.settings.sky = Sky::Sunny;
        canned.answer(Ok(rain()));
        assert!(s.poll(T0 + 1_000.0).scene);
        assert!(s.weather.loaded());
        assert_eq!(s.weather.next_tick, None, "the timer stops");
        // going live again starts the effects afresh: `refreshWeather` at once, which lets the
        // last ask be (recent, and here), and the timer from now
        s.persisted.settings.sky = Sky::Live;
        let wants = s.poll(T0 + 2_000.0);
        assert_eq!(canned.asks().len(), 1);
        assert_eq!(s.weather.next_tick, Some(T0 + 2_000.0 + WEATHER_TICK_MS));
        assert!(wants.wake_at.is_some_and(|t| t <= T0 + 2_000.0 + WEATHER_TICK_MS));
    }

    /// The scene lights the realistic map with the live weather's conditions for *live*, a preset
    /// otherwise; the attribution credits MET Norway once its weather has come.
    #[test]
    fn the_scene_and_the_credits_follow_the_live_weather() {
        use crate::scene_builder::SceneBuilder;
        use crate::state::Clocks;
        use crate::ui::attribution::{self, WEATHER};
        let canned = Canned::default();
        let mut s = app(&canned);
        let clocks = Clocks { local_ms: T0, feed_ms: T0 };
        s.poll(T0);
        assert_eq!(SceneBuilder::default().build(&mut s, clocks, 1.0).sky.weather, None);
        assert!(!attribution::credits(&attribution::Conditions::of(&s, T0)).iter().any(|c| c == WEATHER));
        canned.answer(Ok(rain()));
        s.poll(T0 + 1.0);
        assert_eq!(SceneBuilder::default().build(&mut s, clocks, 1.0).sky.weather, Some(conditions_of(&rain())));
        assert!(attribution::credits(&attribution::Conditions::of(&s, T0)).iter().any(|c| c == WEATHER));
        // a chosen sky: its preset, and no credit
        s.persisted.settings.sky = Sky::Snow;
        assert_eq!(SceneBuilder::default().build(&mut s, clocks, 1.0).sky.weather, Some(PRESETS.snow));
        assert!(!attribution::credits(&attribution::Conditions::of(&s, T0)).iter().any(|c| c == WEATHER));
    }

    /// The `--fixture` folder's forecast is MET Norway's answer, read as the backend reads it; a
    /// folder without one fails the ask (a fine day).
    #[test]
    fn the_fixture_folder_answers_with_its_recorded_forecast() {
        let dir = TempDir::new("weather-fixture");
        let mut feed = FixtureFeed::new(dir.0.as_path());
        assert!(feed.weather_at(59.33, 18.07).poll().unwrap().is_err());
        std::fs::create_dir_all(dir.0.as_path().join("weather")).unwrap();
        let body = serde_json::json!({"properties": {"timeseries": [{"time": "2026-11-12T12:00:00Z", "data": {
            "instant": {"details": {"cloud_area_fraction": 100.0, "cloud_area_fraction_low": 87.5, "fog_area_fraction": 0.0, "air_temperature": 4.2}},
            "next_1_hours": {"summary": {"symbol_code": "rain"}, "details": {"precipitation_amount": 1.4}}
        }}]}});
        std::fs::write(dir.0.as_path().join(FIXTURE_FORECAST), body.to_string()).unwrap();
        let weather = feed.weather_at(59.33, 18.07).poll().unwrap().unwrap();
        assert_eq!(weather, Forecast { cloud: 1.0, cloud_low: 0.875, fog: 0.0, precipitation: 1.4, symbol: "rain".into(), temperature: 4.2 });
        // through the app with --fixture: the sky rains
        let startup = Startup { basemap: Some(Basemap::Realistic), fixture: Some(dir.0.as_path().to_path_buf()), ..Default::default() };
        let mut s = AppState::new(startup, None, None, None, view(59.33, 18.07), SIZE);
        s.poll(T0);
        s.poll(T0 + 1.0);
        let conditions = s.weather.conditions().expect("the fixture's weather");
        assert_eq!(conditions.rain, conditions_of(&weather).rain);
        assert!(conditions.rain > 0.0);
    }
}
