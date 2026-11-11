//! The settings and filters kept between runs, and the modes (item U5; doc 06 §3.2–3.3, §23;
//! `+page.svelte:146-190`, `:256-263`, `:1220-1226`, `:1418-1421`; `filters.ts:200-230`).
//!
//! **The file.** [`Store::open`] reads `settings.json` from the config folder ([`crate::persist`])
//! when the state is made, before the first frame: a missing or unreadable file gives the
//! defaults, a readable one is read with the rules of doc 06 §23. `AppState::persisted` holds what
//! it gave; every part reads and changes it there, and nothing else needs telling. Each poll
//! [`Store::poll`] compares it with what the last poll saw: a change is written
//! [`SAVE_DELAY_MS`] after the last one (the deadline goes to the frame loop as
//! [`Wants::wake_at`](super::Wants)), atomically and off the frame loop's thread
//! ([`persist::Writer`]); a change of the settings (not of the filters, whose lists the parts
//! compare themselves) also refreshes the scene (plan 1.4.9). Like the page's persist effect,
//! which runs once on mount, what was loaded is written back once at start (500 ms in), in
//! today's form: a missing file is made, an older release's keys are dropped. A text equal to the
//! file's is not written again. [`Store::flush`] writes a pending change at once and waits for the
//! disk: the window calls it when the app ends or goes to the background (a phone may end it
//! there).
//!
//! **Modes** (doc 06 §3.3): `settings.mode` is the focus, always on the map; `settings.also_show`
//! lists the other kinds also on it ([`rustradar_core::settings::shown_kinds`]).
//! [`AppState::set_mode`] (the mode bar, keys P, I, S) and [`AppState::toggle_shown`] (the mode
//! bar's eye, Shift+P, I, S; [`toggle_shown`]: a no-op for the focus) are what
//! [`Action::SetMode`](super::Action::SetMode) and [`Action::ToggleShown`](super::Action::ToggleShown)
//! do. [`AppState::offered_modes`] is the mode bar's `offered`.
//!
//! **The Trafikverket key gate**: the trains are offered once Trafikverket has accepted the key
//! ([`super::trains::Trains::offered`]; the check, its 30 s retry, and the trains mode and the
//! trains on the map turned off by "none" or "invalid" are `state::trains`'). Here: the trains
//! mode and the trains' toggle are refused while trains are not offered (`i: () => trainsOffered
//! && (settings.mode = "trains")`, `I: () => trainsOffered && toggleShown("trains")`), and the mode
//! bar offers the trains while they are offered or are the focus (a saved trains mode stays while
//! the key's answer is unknown).
//!
//! **A kind leaving the map** is each part's own effect, run at its next poll after the change:
//! the aircraft stop their feed and drop the snapshot and the freshness clock, and come back as
//! soon as the gap between requests allows (`state::aircraft`), the open flight closes
//! (`state::flight`; the airport board with it is U13's); the ships stop their timers, drop their
//! answers and let go of the selected ship (`state::ships`); the trains too, watching none
//! (`state::trains`). The tests below run them through the actions.
//!
//! Deviations from today's app:
//! - The file is written 500 ms after the last change (plan 2.3), not at every change as
//!   `localStorage` was; a change not yet written when the app ends is written by [`Store::flush`].
//! - `--basemap` shows its basemap over the saved one without saving it: the file keeps the saved
//!   basemap until another one is picked.
//! - A canned session (`--fixture`, `--demo`) reads the file and never writes it: the demo puts the
//!   trains on the map, and a fixture folder without trains answers "no key", which turns a saved
//!   trains mode off; neither is the viewer's choice.
//! - The Tauri build's localStorage is not read: the first native launch starts from the defaults
//!   (plan section 6, item 8). A corrupt file is replaced by the defaults at the first save, as
//!   today's storage was.
//!
//! Left to the items that own them: the mode bar (U7) and the keys (U6) send the two actions; the
//! lists (U14, U15) drop their hover when the focus changes (`hoveredTrainId`,
//! `hoveredShipMmsi`); the menus (U16) change `persisted.settings` directly or through their own
//! actions (saved by the poll either way); the keys dialog (U17) asks for a new key check when it
//! closes ([`super::trains::Trains::check_key`]).

use std::path::PathBuf;

use rustradar_backend::Dirs;
use rustradar_core::geo::Basemap;
use rustradar_core::settings::{Mode, Persisted, Settings, ShownKinds, persisted_json};

use super::{AppState, Cx, Startup};
use crate::persist::{self, SAVE_DELAY_MS, Writer};

/// The settings file: read when the state is made, written [`SAVE_DELAY_MS`] after the last change.
#[derive(Debug, Default)]
pub struct Store {
    /// `settings.json` in the config folder; `None` without one (no backend, unit tests): nothing
    /// is read or written.
    path: Option<PathBuf>,
    /// A canned session (`--fixture`, `--demo`): the file is read, never written.
    read_only: bool,
    /// The file's text as read or last written; `None`: no file, or one that could not be read.
    saved: Option<String>,
    /// The settings and filters as the last poll saw them.
    seen: Persisted,
    /// `--basemap`: the basemap shown over the saved one, and the saved one, written in its place
    /// while the shown one stays.
    shown_over: Option<(Basemap, Basemap)>,
    /// When the change seen last is to be written (local Unix ms).
    save_at: Option<f64>,
    /// The first poll has run (the page's mount).
    started: bool,
    /// Made at the first write.
    writer: Option<Writer>,
}

impl Store {
    /// Reads the settings file in `dirs`' config folder (none without folders), with the command
    /// line's basemap over the saved one. Returns the store and what the state starts with.
    pub fn open(dirs: Option<&Dirs>, startup: &Startup) -> (Store, Persisted) {
        let path = dirs.map(persist::path);
        let loaded = path.as_deref().map(persist::load).unwrap_or_default();
        let mut persisted = loaded.persisted;
        let mut shown_over = None;
        if let Some(basemap) = startup.basemap {
            if basemap != persisted.settings.basemap {
                shown_over = Some((basemap, persisted.settings.basemap));
            }
            persisted.settings.basemap = basemap;
        }
        let store = Store {
            path,
            read_only: startup.fixture.is_some() || startup.demo,
            saved: loaded.text,
            seen: persisted.clone(),
            shown_over,
            ..Store::default()
        };
        (store, persisted)
    }

    /// Where the settings are kept; `None` when they are not.
    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    /// Whether changes are written.
    pub fn writes(&self) -> bool {
        self.path.is_some() && !self.read_only
    }

    /// Whether a change waits to be written.
    pub fn pending(&self) -> bool {
        self.save_at.is_some()
    }

    /// This frame's step (see [`super::AppState::poll`] for the order): a change since the last
    /// poll is written [`SAVE_DELAY_MS`] after it (and a change of the settings refreshes the
    /// scene); the write that is due is sent.
    pub fn poll(&mut self, cx: &mut Cx<'_>) {
        let now = cx.clocks.local_ms;
        if !self.started {
            self.started = true;
            // the page's persist effect runs on mount: what was loaded goes back in today's form
            if self.writes() {
                self.save_at = Some(now + SAVE_DELAY_MS);
            }
        }
        if self.observe(cx.persisted, now) {
            cx.wants.scene = true;
        }
        match self.save_at {
            Some(at) if now >= at => {
                self.save_at = None;
                self.save();
            }
            Some(at) => cx.wants.wake_at(at),
            None => {}
        }
    }

    /// Writes a change not yet written now and waits until it is on the disk: before the app ends,
    /// or goes to the background where a phone may end it.
    pub fn flush(&mut self, persisted: &Persisted) {
        if !self.writes() {
            return;
        }
        self.observe(persisted, 0.0);
        self.save_at = None;
        self.save();
        if let Some(writer) = &self.writer {
            writer.flush();
        }
    }

    /// Takes `persisted` in as seen; a change arms the write. Returns whether the settings changed.
    fn observe(&mut self, persisted: &Persisted, now: f64) -> bool {
        if *persisted == self.seen {
            return false;
        }
        let settings_changed = persisted.settings != self.seen.settings;
        // another basemap picked: from now on the one shown is the one saved
        if self.shown_over.is_some_and(|(shown, _)| persisted.settings.basemap != shown) {
            self.shown_over = None;
        }
        self.seen = persisted.clone();
        if self.writes() {
            self.save_at = Some(now + SAVE_DELAY_MS);
        }
        settings_changed
    }

    /// Sends the text of what was seen last to the writer, unless the file already holds it.
    fn save(&mut self) {
        let Some(path) = &self.path else { return };
        if self.read_only {
            return;
        }
        let text = self.text(&self.seen);
        if self.saved.as_ref() == Some(&text) {
            return;
        }
        self.writer.get_or_insert_with(|| Writer::new(path.clone())).save(text.clone());
        self.saved = Some(text);
    }

    /// The file's text for `persisted` (`JSON.stringify`), with the saved basemap in place of the
    /// command line's while that is shown.
    fn text(&self, persisted: &Persisted) -> String {
        match self.shown_over {
            Some((shown, saved)) if persisted.settings.basemap == shown => {
                let mut kept = persisted.clone();
                kept.settings.basemap = saved;
                persisted_json(&kept)
            }
            _ => persisted_json(persisted),
        }
    }
}

/// `toggleShown(kind)`: another kind on or off the map. The focus is always on: a no-op for it.
/// Returns whether anything changed.
pub fn toggle_shown(settings: &mut Settings, kind: Mode) -> bool {
    if kind == settings.mode {
        return false;
    }
    if settings.also_show.contains(&kind) {
        settings.also_show.retain(|k| *k != kind);
    } else {
        settings.also_show.push(kind);
    }
    true
}

/// The kinds the mode bar offers (`offered`): aircraft and ships always, the trains while
/// Trafikverket has accepted the key or while they are the focus.
pub fn offered(mode: Mode, trains_offered: bool) -> ShownKinds {
    ShownKinds { aircraft: true, trains: trains_offered || mode == Mode::Trains, ships: true }
}

impl AppState {
    /// `settings.mode = mode` (the mode bar, keys P, I, S): the focus. The trains only while they
    /// are offered. Returns whether the focus changed.
    pub fn set_mode(&mut self, mode: Mode) -> bool {
        let settings = &mut self.persisted.settings;
        if settings.mode == mode || (mode == Mode::Trains && !self.trains.offered()) {
            return false;
        }
        settings.mode = mode;
        true
    }

    /// [`toggle_shown`] (the mode bar's eye, Shift+P, I, S): the trains only while they are
    /// offered. Returns whether anything changed.
    pub fn toggle_shown(&mut self, kind: Mode) -> bool {
        if kind == Mode::Trains && !self.trains.offered() {
            return false;
        }
        toggle_shown(&mut self.persisted.settings, kind)
    }

    /// [`offered`] now: the mode bar's options.
    pub fn offered_modes(&self) -> ShownKinds {
        offered(self.persisted.settings.mode, self.trains.offered())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rustradar_core::model::{LiveFlight, LiveSnapshot, Ship, Train, TrainKind, TrainSource};
    use rustradar_core::motion::aircraft::Motion;
    use rustradar_core::settings::{DEFAULT_SETTINGS, MODES, RailShows, load_persisted, shown_kinds};
    use rustradar_core::weather::Sky;
    use rustradar_scene::camera::ViewState;
    use rustradar_scene::controller::Controller;
    use serde_json::{Value, json};

    use super::super::flight::SelectOptions;
    use super::super::{Action, Clocks, Wants, aircraft, ships, trains};
    use super::*;
    use crate::persist::tests::TempDir;

    const T0: f64 = 1.782e12;

    fn dirs(dir: &TempDir) -> Dirs {
        Dirs { cache: dir.0.join("cache"), config: dir.0.join("config"), downloads: None }
    }

    fn file(dir: &TempDir) -> PathBuf {
        dir.0.join("config").join(persist::FILE)
    }

    /// The store's step at local time `t`, alone.
    fn step(store: &mut Store, persisted: &mut Persisted, t: f64) -> Wants {
        let mut controller = Controller::new(ViewState::INITIAL, [800.0, 600.0]);
        let mut motion = Motion::new();
        let startup = Startup::default();
        let mut wants = Wants::default();
        let mut cx = Cx {
            clocks: Clocks { local_ms: t, feed_ms: t },
            controller: &mut controller,
            motion: &mut motion,
            persisted,
            backend: None,
            requests: None,
            startup: &startup,
            events: &[],
            wants: &mut wants,
        };
        store.poll(&mut cx);
        wants
    }

    /// Waits for the writes sent so far, then reads the file's text.
    fn on_disk(store: &Store, dir: &TempDir) -> Option<String> {
        if let Some(writer) = &store.writer {
            writer.flush();
        }
        fs::read_to_string(file(dir)).ok()
    }

    /// Settings and filters with nothing left at its default.
    fn everything_changed() -> Persisted {
        let mut p = load_persisted(Some(
            r#"{
                "settings": {"mode": "ships", "alsoShow": ["aircraft", "trains"], "basemap": "realistic", "labels": false,
                    "labelSize": 1.25, "buildings": false, "aircraftLabels": true, "daylight": false, "weather": true,
                    "adsb": false, "railMap": true, "railShows": "blocks", "sky": "snow", "tiltShift": true,
                    "terrain": true, "exaggeration": 3},
                "filters": {"categories": [0, 3], "altitude": [1000, 20000], "airborne": false, "onGround": true,
                    "airlines": ["SAS"], "types": ["A3"], "airports": ["ARN"], "text": "sk"},
                "trainFilters": {"hiddenKinds": ["highSpeed"], "hiddenOperators": ["SJ", ""], "delayedOnly": true, "sort": "distance"},
                "shipFilters": {"hiddenKinds": [3, 7], "motion": "moored", "hiddenSizes": [2], "hideOld": true, "sort": "name"}
            }"#,
        ));
        p.settings.exaggeration = 2.5;
        p
    }

    /// Doc 06 §23's schema, checked on a value read from disk: every part and key as documented,
    /// nothing else, each of its type and among its values.
    fn check_schema(v: &Value) {
        fn keys(v: &Value, want: &[&str], part: &str) {
            let mut got: Vec<&str> = v.as_object().unwrap_or_else(|| panic!("{part} is an object")).keys().map(String::as_str).collect();
            let mut want = want.to_vec();
            got.sort_unstable();
            want.sort_unstable();
            assert_eq!(got, want, "{part}'s keys");
        }
        fn one_of(v: &Value, values: &[&str], what: &str) {
            let s = v.as_str().unwrap_or_else(|| panic!("{what} is text: {v}"));
            assert!(values.contains(&s), "{what}: {s}");
        }
        fn strings(v: &Value, what: &str) -> Vec<String> {
            v.as_array()
                .unwrap_or_else(|| panic!("{what} is an array"))
                .iter()
                .map(|s| s.as_str().unwrap_or_else(|| panic!("{what} holds text")).to_owned())
                .collect()
        }
        fn whole(v: &Value, what: &str) -> i64 {
            v.as_i64().unwrap_or_else(|| panic!("{what} is a whole number: {v}"))
        }
        let boolean = |v: &Value, what: &str| assert!(v.is_boolean(), "{what} is true or false: {v}");
        keys(v, &["settings", "filters", "trainFilters", "shipFilters"], "the file");

        let s = &v["settings"];
        keys(
            s,
            &[
                "mode", "alsoShow", "basemap", "labels", "labelSize", "buildings", "aircraftLabels", "daylight", "weather", "adsb",
                "railMap", "railShows", "sky", "tiltShift", "terrain", "exaggeration",
            ],
            "settings",
        );
        let modes = ["aircraft", "trains", "ships"];
        one_of(&s["mode"], &modes, "mode");
        for kind in strings(&s["alsoShow"], "alsoShow") {
            assert!(modes.contains(&kind.as_str()), "alsoShow: {kind}");
        }
        one_of(&s["basemap"], &["dark", "satellite", "realistic"], "basemap");
        for key in ["labels", "buildings", "aircraftLabels", "daylight", "weather", "adsb", "railMap", "tiltShift", "terrain"] {
            boolean(&s[key], key);
        }
        let size = s["labelSize"].as_f64().expect("labelSize is a number");
        assert!([0.85, 1.0, 1.25, 1.5].contains(&size), "labelSize: {size}");
        one_of(&s["railShows"], &["speeds", "blocks"], "railShows");
        one_of(&s["sky"], &["live", "sunny", "cloudy", "rain", "fog", "snow"], "sky");
        let exaggeration = s["exaggeration"].as_f64().expect("exaggeration is a number");
        assert!((1.0..=40.0).contains(&exaggeration), "exaggeration: {exaggeration}");

        let f = &v["filters"];
        keys(f, &["categories", "altitude", "airborne", "onGround", "airlines", "types", "airports", "text"], "filters");
        for c in f["categories"].as_array().expect("categories is an array") {
            assert!((0..=11).contains(&whole(c, "a category")), "category {c}");
        }
        let altitude = f["altitude"].as_array().expect("altitude is an array");
        assert_eq!(altitude.len(), 2, "altitude is [low, high]");
        let (low, high) = (whole(&altitude[0], "altitude"), whole(&altitude[1], "altitude"));
        assert!(0 <= low && low <= high && high <= 50_000, "altitude {low}..{high} ft");
        boolean(&f["airborne"], "airborne");
        boolean(&f["onGround"], "onGround");
        for key in ["airlines", "types", "airports"] {
            strings(&f[key], key);
        }
        assert!(f["text"].is_string(), "text is text");

        let t = &v["trainFilters"];
        keys(t, &["hiddenKinds", "hiddenOperators", "delayedOnly", "sort"], "trainFilters");
        for kind in strings(&t["hiddenKinds"], "hiddenKinds") {
            assert!(["highSpeed", "intercity", "regional", "suburban", "other"].contains(&kind.as_str()), "train kind {kind}");
        }
        strings(&t["hiddenOperators"], "hiddenOperators");
        boolean(&t["delayedOnly"], "delayedOnly");
        one_of(&t["sort"], &["delay", "speed", "name", "distance"], "train sort");

        let h = &v["shipFilters"];
        keys(h, &["hiddenKinds", "motion", "hiddenSizes", "hideOld", "sort"], "shipFilters");
        for kind in h["hiddenKinds"].as_array().expect("hiddenKinds is an array") {
            assert!([0, 2, 3, 4, 5, 6, 7, 8].contains(&whole(kind, "a ship colour key")), "ship kind {kind}");
        }
        one_of(&h["motion"], &["all", "moving", "moored"], "motion");
        for size in h["hiddenSizes"].as_array().expect("hiddenSizes is an array") {
            assert!((0..=2).contains(&whole(size, "a ship size")), "ship size {size}");
        }
        boolean(&h["hideOld"], "hideOld");
        one_of(&h["sort"], &["size", "name", "distance"], "ship sort");
    }

    // ------------------------------------------------------------------- the file

    #[test]
    fn a_missing_file_gives_the_defaults_and_is_made_half_a_second_in() {
        let dir = TempDir::new("settings-missing");
        let (mut store, mut p) = Store::open(Some(&dirs(&dir)), &Startup::default());
        assert_eq!(p, Persisted::default());
        assert_eq!(store.path(), Some(file(&dir).as_path()));
        assert!(store.writes());
        // the mount's write, due 500 ms in: the frame loop is told to come back then
        let wants = step(&mut store, &mut p, T0);
        assert_eq!(wants, Wants { scene: false, wake_at: Some(T0 + SAVE_DELAY_MS) });
        assert_eq!(on_disk(&store, &dir), None);
        step(&mut store, &mut p, T0 + 499.0);
        assert_eq!(on_disk(&store, &dir), None);
        assert_eq!(step(&mut store, &mut p, T0 + 500.0), Wants::default());
        assert_eq!(on_disk(&store, &dir), Some(persisted_json(&Persisted::default())));
        assert!(!store.pending());
    }

    #[test]
    fn a_corrupt_file_gives_the_defaults_and_is_replaced_at_the_first_save() {
        for text in ["", "{\"settings\": {\"labels\": fal", "[]", "null", "{\"settings\": 7, \"filters\": \"x\"}"] {
            let dir = TempDir::new("settings-corrupt");
            fs::create_dir_all(dir.0.join("config")).unwrap();
            fs::write(file(&dir), text).unwrap();
            let (mut store, mut p) = Store::open(Some(&dirs(&dir)), &Startup::default());
            assert_eq!(p, Persisted::default(), "{text:?}");
            step(&mut store, &mut p, T0);
            step(&mut store, &mut p, T0 + 500.0);
            assert_eq!(on_disk(&store, &dir), Some(persisted_json(&Persisted::default())), "{text:?}");
        }
        // a file that cannot be read as text, and a folder in its place
        let dir = TempDir::new("settings-unreadable");
        fs::create_dir_all(dir.0.join("config")).unwrap();
        fs::write(file(&dir), [0xc3, 0x28, 0xa0]).unwrap();
        assert_eq!(Store::open(Some(&dirs(&dir)), &Startup::default()).1, Persisted::default());
        fs::remove_file(file(&dir)).unwrap();
        fs::create_dir(file(&dir)).unwrap();
        let (mut store, mut p) = Store::open(Some(&dirs(&dir)), &Startup::default());
        assert_eq!(p, Persisted::default());
        // its write fails (logged) and the app carries on
        step(&mut store, &mut p, T0);
        step(&mut store, &mut p, T0 + 500.0);
        assert_eq!(on_disk(&store, &dir), None);
        assert!(file(&dir).is_dir());
    }

    #[test]
    fn what_is_saved_is_read_back_the_same() {
        let dir = TempDir::new("settings-round-trip");
        let (mut store, mut p) = Store::open(Some(&dirs(&dir)), &Startup::default());
        step(&mut store, &mut p, T0);
        p = everything_changed();
        step(&mut store, &mut p, T0 + 100.0);
        step(&mut store, &mut p, T0 + 600.0);
        assert_eq!(on_disk(&store, &dir), Some(persisted_json(&everything_changed())));
        drop(store);
        let (store, again) = Store::open(Some(&dirs(&dir)), &Startup::default());
        assert_eq!(again, everything_changed());
        // the defaults round-trip too
        let mut store = store;
        let mut p = again;
        step(&mut store, &mut p, T0);
        p = Persisted::default();
        step(&mut store, &mut p, T0 + 10.0);
        store.flush(&p);
        assert_eq!(Store::open(Some(&dirs(&dir)), &Startup::default()).1, Persisted::default());
    }

    #[test]
    fn a_file_written_by_this_code_parses_as_the_documented_schema() {
        for p in [Persisted::default(), everything_changed()] {
            let dir = TempDir::new("settings-schema");
            let (mut store, _) = Store::open(Some(&dirs(&dir)), &Startup::default());
            store.flush(&p);
            let text = fs::read_to_string(file(&dir)).expect("written");
            let value: Value = serde_json::from_str(&text).expect("the file is JSON");
            check_schema(&value);
            // and the documented defaults, value for value (doc 06 §23)
            if p == Persisted::default() {
                assert_eq!(
                    value,
                    json!({
                        "settings": {
                            "mode": "aircraft", "alsoShow": [], "basemap": "dark", "labels": true, "labelSize": 1,
                            "buildings": true, "aircraftLabels": false, "daylight": true, "weather": false, "adsb": true,
                            "railMap": false, "railShows": "speeds", "sky": "live", "tiltShift": false, "terrain": false,
                            "exaggeration": 10
                        },
                        "filters": {
                            "categories": [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11], "altitude": [0, 50000], "airborne": true,
                            "onGround": true, "airlines": [], "types": [], "airports": [], "text": ""
                        },
                        "trainFilters": { "hiddenKinds": [], "hiddenOperators": [], "delayedOnly": false, "sort": "delay" },
                        "shipFilters": { "hiddenKinds": [], "motion": "all", "hiddenSizes": [], "hideOld": false, "sort": "size" }
                    })
                );
            }
        }
    }

    #[test]
    fn the_file_is_written_half_a_second_after_the_last_change() {
        let dir = TempDir::new("settings-debounce");
        let (mut store, mut p) = Store::open(Some(&dirs(&dir)), &Startup::default());
        step(&mut store, &mut p, T0);
        step(&mut store, &mut p, T0 + 500.0);
        let defaults = on_disk(&store, &dir);
        // an exaggeration slider dragged: a change every frame
        for (i, x) in [12.0, 14.0, 17.0].into_iter().enumerate() {
            p.settings.exaggeration = x;
            let t = T0 + 1000.0 + 16.0 * i as f64;
            assert_eq!(step(&mut store, &mut p, t).wake_at, Some(t + SAVE_DELAY_MS), "each change moves the write");
        }
        let last = T0 + 1032.0;
        step(&mut store, &mut p, last + 499.0);
        assert_eq!(on_disk(&store, &dir), defaults, "not before 500 ms after the last change");
        step(&mut store, &mut p, last + 500.0);
        assert_eq!(load_persisted(on_disk(&store, &dir).as_deref()).settings.exaggeration, 17.0);
        // a change undone before it was written writes nothing
        p.settings.labels = false;
        step(&mut store, &mut p, T0 + 3000.0);
        p.settings.labels = true;
        step(&mut store, &mut p, T0 + 3100.0);
        let before = fs::metadata(file(&dir)).unwrap().modified().unwrap();
        let saved = store.saved.clone();
        step(&mut store, &mut p, T0 + 3600.0);
        assert_eq!(store.saved, saved);
        assert_eq!(fs::metadata(file(&dir)).unwrap().modified().unwrap(), before);
        assert_eq!(dir.files(), ["config"]);
        let config: Vec<String> = fs::read_dir(dir.0.join("config")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(config, [persist::FILE], "no temporary file left");
    }

    #[test]
    fn a_saved_file_in_today_s_form_is_not_written_again() {
        let dir = TempDir::new("settings-unchanged");
        fs::create_dir_all(dir.0.join("config")).unwrap();
        fs::write(file(&dir), persisted_json(&everything_changed())).unwrap();
        let (mut store, mut p) = Store::open(Some(&dirs(&dir)), &Startup::default());
        assert_eq!(p, everything_changed());
        step(&mut store, &mut p, T0);
        step(&mut store, &mut p, T0 + 500.0);
        store.flush(&p);
        assert!(store.writer.is_none(), "nothing to write: the file holds this text");
    }

    #[test]
    fn a_file_of_an_older_release_is_read_by_today_s_rules_and_written_back_in_today_s_form() {
        let dir = TempDir::new("settings-legacy");
        fs::create_dir_all(dir.0.join("config")).unwrap();
        let old = r#"{"settings":{"aircraft":false,"ships":false,"trains":true,"trainBlocks":true,"basemap":"satellite","liveries":false,"labelSize":1.25},"filters":{"text":"SAS"}}"#;
        fs::write(file(&dir), old).unwrap();
        let (mut store, mut p) = Store::open(Some(&dirs(&dir)), &Startup::default());
        let want = Settings {
            mode: Mode::Trains,
            basemap: Basemap::Satellite,
            label_size: 1.25,
            rail_map: true,
            rail_shows: RailShows::Blocks,
            ..DEFAULT_SETTINGS
        };
        assert_eq!(p.settings, want);
        assert_eq!(p.filters.text, "SAS");
        step(&mut store, &mut p, T0);
        step(&mut store, &mut p, T0 + 500.0);
        let text = on_disk(&store, &dir).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        check_schema(&value);
        assert_eq!(load_persisted(Some(&text)), p);
        for gone in ["aircraft", "ships", "trains", "trainBlocks", "liveries"] {
            assert!(value["settings"].get(gone).is_none(), "{gone} is dropped");
        }
    }

    #[test]
    fn a_change_of_the_settings_refreshes_the_scene_and_one_of_the_filters_does_not() {
        let (mut store, mut p) = Store::open(None, &Startup::default());
        assert!(!store.writes());
        assert_eq!(step(&mut store, &mut p, T0), Wants::default());
        p.settings.daylight = false;
        assert!(step(&mut store, &mut p, T0 + 10.0).scene);
        assert!(!step(&mut store, &mut p, T0 + 20.0).scene, "once");
        p.filters.text = "SAS".into();
        p.ship_filters.hide_old = true;
        // the parts compare their own lists; without a folder nothing is written, nothing waits
        assert_eq!(step(&mut store, &mut p, T0 + 30.0), Wants::default());
        assert!(!store.pending());
    }

    #[test]
    fn without_a_config_folder_nothing_is_read_or_written() {
        let (mut store, mut p) = Store::open(None, &Startup { basemap: Some(Basemap::Realistic), ..Default::default() });
        assert_eq!(store.path(), None);
        assert_eq!(p.settings.basemap, Basemap::Realistic);
        p.settings.labels = false;
        step(&mut store, &mut p, T0);
        step(&mut store, &mut p, T0 + 1000.0);
        store.flush(&p);
        assert!(store.writer.is_none());
        // the state's own store: the unit tests' states have none either
        let s = AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, [800.0, 600.0]);
        assert_eq!(s.settings.path(), None);
        assert_eq!(s.persisted, Persisted::default());
    }

    #[test]
    fn the_command_line_basemap_is_shown_and_not_saved_until_another_is_picked() {
        let dir = TempDir::new("settings-basemap");
        fs::create_dir_all(dir.0.join("config")).unwrap();
        let mut saved = Persisted::default();
        saved.settings.basemap = Basemap::Satellite;
        fs::write(file(&dir), persisted_json(&saved)).unwrap();
        let startup = Startup { basemap: Some(Basemap::Realistic), ..Default::default() };
        let (mut store, mut p) = Store::open(Some(&dirs(&dir)), &startup);
        assert_eq!(p.settings.basemap, Basemap::Realistic, "shown over the saved one");
        // another change is saved with the saved basemap
        step(&mut store, &mut p, T0);
        p.settings.labels = false;
        step(&mut store, &mut p, T0 + 100.0);
        step(&mut store, &mut p, T0 + 600.0);
        let on = load_persisted(on_disk(&store, &dir).as_deref());
        assert_eq!((on.settings.basemap, on.settings.labels), (Basemap::Satellite, false));
        // a basemap picked is saved, and from then on the one shown
        p.settings.basemap = Basemap::Dark;
        step(&mut store, &mut p, T0 + 1000.0);
        step(&mut store, &mut p, T0 + 1500.0);
        assert_eq!(load_persisted(on_disk(&store, &dir).as_deref()).settings.basemap, Basemap::Dark);
        p.settings.basemap = Basemap::Realistic;
        store.flush(&p);
        assert_eq!(load_persisted(on_disk(&store, &dir).as_deref()).settings.basemap, Basemap::Realistic);
        // the same basemap as the saved one is no override at all
        let (store, p) = Store::open(Some(&dirs(&dir)), &startup);
        assert_eq!((store.shown_over, p.settings.basemap), (None, Basemap::Realistic));
    }

    #[test]
    fn a_canned_session_reads_the_file_and_never_writes_it() {
        for startup in [Startup { demo: true, ..Default::default() }, Startup { fixture: Some("/nowhere".into()), ..Default::default() }] {
            let dir = TempDir::new("settings-canned");
            fs::create_dir_all(dir.0.join("config")).unwrap();
            let text = persisted_json(&everything_changed());
            fs::write(file(&dir), &text).unwrap();
            let (mut store, mut p) = Store::open(Some(&dirs(&dir)), &startup);
            assert_eq!(p, everything_changed(), "{startup:?}");
            assert!(!store.writes());
            step(&mut store, &mut p, T0);
            p.settings.also_show.clear();
            p.settings.mode = Mode::Aircraft;
            let wants = step(&mut store, &mut p, T0 + 100.0);
            assert_eq!(wants, Wants { scene: true, wake_at: None });
            step(&mut store, &mut p, T0 + 1000.0);
            store.flush(&p);
            assert!(store.writer.is_none());
            assert_eq!(fs::read_to_string(file(&dir)).unwrap(), text);
        }
    }

    #[test]
    fn flush_writes_a_change_at_once_and_waits_for_the_disk() {
        let dir = TempDir::new("settings-flush");
        let (mut store, mut p) = Store::open(Some(&dirs(&dir)), &Startup::default());
        step(&mut store, &mut p, T0);
        p.settings.sky = Sky::Fog;
        // changed after the last poll (an action of the UI pass), then the window closes
        store.flush(&p);
        assert!(!store.pending());
        assert_eq!(fs::read_to_string(file(&dir)).ok().as_deref().map(|t| load_persisted(Some(t)).settings.sky), Some(Sky::Fog));
        // nothing more to write afterwards
        let saved = store.saved.clone();
        step(&mut store, &mut p, T0 + 1000.0);
        assert_eq!(store.saved, saved);
    }

    // ------------------------------------------------------------------- the modes

    #[test]
    fn toggle_shown_flips_another_kind_and_leaves_the_focus() {
        let mut s = DEFAULT_SETTINGS;
        assert!(!toggle_shown(&mut s, Mode::Aircraft), "the focus is always on");
        assert_eq!(s.also_show, []);
        assert!(toggle_shown(&mut s, Mode::Ships));
        assert!(toggle_shown(&mut s, Mode::Trains));
        assert_eq!(s.also_show, [Mode::Ships, Mode::Trains], "added at the end, as the page does");
        assert!(toggle_shown(&mut s, Mode::Ships));
        assert_eq!(s.also_show, [Mode::Trains]);
        // a list that names a kind twice (hand-edited): every copy goes
        s.also_show = vec![Mode::Ships, Mode::Trains, Mode::Ships];
        assert!(toggle_shown(&mut s, Mode::Ships));
        assert_eq!(s.also_show, [Mode::Trains]);
        // the focus listed too stays listed (shown either way)
        s.mode = Mode::Trains;
        assert!(!toggle_shown(&mut s, Mode::Trains));
        assert_eq!(s.also_show, [Mode::Trains]);
        assert_eq!(shown_kinds(s.mode, &s.also_show), ShownKinds { aircraft: false, trains: true, ships: false });
    }

    #[test]
    fn the_mode_bar_offers_the_trains_with_a_key_or_as_the_focus() {
        assert_eq!(offered(Mode::Aircraft, false), ShownKinds { aircraft: true, trains: false, ships: true });
        assert_eq!(offered(Mode::Aircraft, true), ShownKinds { aircraft: true, trains: true, ships: true });
        assert_eq!(offered(Mode::Trains, false), ShownKinds { aircraft: true, trains: true, ships: true });
        for mode in MODES {
            let o = offered(mode, false);
            assert!(o.aircraft && o.ships, "{mode:?}");
        }
    }

    fn ship(mmsi: u32) -> Ship {
        Ship { mmsi, lat: 59.33, lon: 18.07, name: format!("SHIP {mmsi}"), kind: 3, size: 1, ..Default::default() }
    }

    fn train(n: u32) -> Train {
        Train {
            id: format!("trafikverket:2026-06-21:{n}"),
            source: TrainSource::Trafikverket,
            name: format!("Pendeltåg {n}"),
            kind: TrainKind::Suburban,
            lat: 59.33,
            lon: 18.07,
            heading: Some(90.0),
            speed_kmh: Some(60.0),
            destination: None,
            operator: Some("SL".into()),
            delay_minutes: Some(0),
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
        }
    }

    /// A fixture folder with aircraft, ships and (when `trains`) trains, and the state on it at
    /// Stockholm.
    fn canned(dir: &TempDir, with_trains: bool) -> AppState {
        let flights = vec![LiveFlight { id: 7, callsign: "SAS7".into(), lat: 59.4, lon: 18.0, alt: 12_000, ..Default::default() }];
        let snapshot = LiveSnapshot { flights, server_time_ms: T0 as u64, ..Default::default() };
        fs::write(dir.0.join(aircraft::FIXTURE_LIVE), serde_json::to_vec(&snapshot).unwrap()).unwrap();
        fs::write(dir.0.join(ships::FIXTURE_SHIPS), serde_json::to_vec(&vec![ship(1), ship(2)]).unwrap()).unwrap();
        if with_trains {
            fs::write(dir.0.join(trains::FIXTURE_TRAINS), serde_json::to_vec(&vec![train(1)]).unwrap()).unwrap();
        }
        let startup = Startup { fixture: Some(dir.0.clone()), ..Default::default() };
        let view = ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, pitch: 0.0, bearing: 0.0, position: None };
        AppState::new(startup, None, None, None, view, [1440.0, 900.0])
    }

    #[test]
    fn the_trains_mode_and_toggle_wait_for_trafikverket_to_accept_the_key() {
        let dir = TempDir::new("settings-key");
        let mut s = canned(&dir, false);
        assert_eq!(s.trains.key_status(), None);
        assert_eq!(s.offered_modes(), ShownKinds { aircraft: true, trains: false, ships: true });
        s.poll(T0);
        // no trains in the folder: the key check answers "none"
        assert!(!s.trains.offered());
        assert!(!s.apply(Action::SetMode(Mode::Trains), T0));
        assert!(!s.apply(Action::ToggleShown(Mode::Trains), T0));
        assert_eq!((s.persisted.settings.mode, &s.persisted.settings.also_show[..]), (Mode::Aircraft, &[][..]));
        // the other kinds need no key
        assert!(s.apply(Action::SetMode(Mode::Ships), T0));
        assert!(!s.apply(Action::SetMode(Mode::Ships), T0), "already the focus");
        assert!(s.apply(Action::ToggleShown(Mode::Aircraft), T0));
        assert_eq!(s.persisted.settings.also_show, [Mode::Aircraft]);
        assert!(!s.apply(Action::ToggleShown(Mode::Ships), T0), "the focus");

        // with trains in the folder the key is accepted: both are offered
        let dir = TempDir::new("settings-key-ok");
        let mut s = canned(&dir, true);
        s.poll(T0);
        assert!(s.trains.offered());
        assert_eq!(s.offered_modes(), ShownKinds { aircraft: true, trains: true, ships: true });
        assert!(s.apply(Action::ToggleShown(Mode::Trains), T0));
        assert_eq!(s.persisted.settings.also_show, [Mode::Trains]);
        assert!(s.apply(Action::SetMode(Mode::Trains), T0));
        assert_eq!(s.persisted.settings.mode, Mode::Trains);
    }

    #[test]
    fn a_kind_leaving_the_map_stops_and_lets_go_of_what_it_had() {
        let dir = TempDir::new("settings-kinds");
        let mut s = canned(&dir, true);
        s.persisted.settings.also_show = vec![Mode::Ships, Mode::Trains];
        let mut t = T0;
        s.poll(t);
        assert_eq!(s.aircraft.flights().len(), 1, "the canned answers come at the first frame");
        assert_eq!(s.ships.ships().len(), 2);
        assert_eq!(s.trains.trains().len(), 1);
        s.select_flight(Some(7), SelectOptions::default(), t);
        assert_eq!(s.flight.selected_id(), Some(7));

        // the aircraft off the map (the focus moves to the ships): the flight closes, the feed
        // stops and its snapshot and freshness clock go
        t += 1000.0;
        assert!(s.apply(Action::SetMode(Mode::Ships), t));
        s.poll(t);
        assert_eq!(s.flight.selected_id(), None);
        assert!(s.aircraft.snapshot().is_none());
        assert!(s.aircraft.shown_feed().is_none());
        assert_eq!(s.aircraft.next_refresh_at(), 0.0);
        // back on as a kind also shown: asked again as soon as the gap between requests allows
        t += 10_000.0;
        assert!(s.apply(Action::ToggleShown(Mode::Aircraft), t));
        s.poll(t);
        s.poll(t + 16.0);
        assert_eq!(s.aircraft.flights().len(), 1, "a fresh snapshot, at once");

        // the ships off (the focus back on the aircraft, the ships not listed): their answers and
        // the selected ship go
        s.select_ship(Some(ship(1)), t);
        assert_eq!(s.ships.selected_mmsi(), Some(1));
        t += 1000.0;
        assert!(s.apply(Action::SetMode(Mode::Aircraft), t));
        s.poll(t);
        assert_eq!(s.ships.selected_mmsi(), Some(1), "still listed: still on the map");
        assert!(s.apply(Action::ToggleShown(Mode::Ships), t));
        assert_eq!(s.persisted.settings.also_show, [Mode::Trains, Mode::Aircraft]);
        s.poll(t);
        assert!(!shown_kinds(s.persisted.settings.mode, &s.persisted.settings.also_show).ships);
        assert_eq!(s.ships.selected_mmsi(), None);
        assert!(s.ships.ships().is_empty());

        // the trains off: theirs too
        s.select_train(Some(train(1)), t);
        assert!(s.trains.selected_id().is_some());
        t += 1000.0;
        assert!(s.apply(Action::ToggleShown(Mode::Trains), t));
        s.poll(t);
        assert_eq!(s.trains.selected_id(), None);
        assert!(s.trains.trains().is_empty());
        assert!(s.trains.watched().is_empty());
    }

    #[test]
    fn the_state_starts_from_the_file_in_its_backend_s_config_folder() {
        // as the window starts it: the backend on the folders, the state on the backend
        let dir = TempDir::new("settings-state");
        fs::create_dir_all(dir.0.join("config")).unwrap();
        fs::write(file(&dir), persisted_json(&everything_changed())).unwrap();
        // the process's one runtime (`requests::runtime`), as the window's
        let rt = crate::requests::runtime().expect("runtime");
        let (events, _rx) = std::sync::mpsc::channel();
        let backend = crate::app::start_backend(Some(dirs(&dir)), rt.handle(), events, crate::requests::no_wake());
        assert!(backend.is_some(), "the backend starts on the test's folders");
        let s = AppState::new(Startup::default(), backend, None, None, ViewState::INITIAL, [800.0, 600.0]);
        assert_eq!(s.persisted, everything_changed());
        assert_eq!(s.settings.path(), Some(file(&dir).as_path()));
        assert!(s.settings.writes());
        // the camera starts with the saved basemap's limits
        assert_eq!(s.controller.limits(), rustradar_scene::camera::REALISTIC_LIMITS);
        // and `--basemap` is shown over it
        let (events, _rx) = std::sync::mpsc::channel();
        let backend = crate::app::start_backend(Some(dirs(&dir)), rt.handle(), events, crate::requests::no_wake());
        let startup = Startup { basemap: Some(Basemap::Dark), ..Default::default() };
        let s = AppState::new(startup, backend, None, None, ViewState::INITIAL, [800.0, 600.0]);
        assert_eq!(s.basemap(), Basemap::Dark);
        assert_eq!(s.controller.limits(), rustradar_scene::camera::LIMITS);
        assert_eq!(s.persisted.settings.labels, everything_changed().settings.labels);
    }
}
