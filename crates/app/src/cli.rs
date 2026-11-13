//! `--set KEY=VALUE` (item F3; plan "Stages 8-10 status", gap 10): a setting for this run only,
//! so that a shot of the sky, a weather preset, the tilt-shift, the terrain, the buildings, the
//! radar or the labels is a command line rather than a test's `headless::render_on_with`.
//!
//! **Keys and values** are the settings file's (`settings.json`, doc 06 §23; today's
//! `filters.ts` `Settings` under `rustradar.v1`), spelt as it spells them ([`KEYS`]):
//! - `labels`, `buildings`, `aircraftLabels`, `daylight`, `weather` (the weather radar), `adsb`,
//!   `railMap`, `tiltShift`, `terrain`: `true` or `false`;
//! - `mode`: `aircraft`, `trains` or `ships`; `alsoShow`: a comma-separated list of them, or
//!   nothing (`alsoShow=`); `basemap`: `dark`, `satellite` or `realistic`; `railShows`: `speeds`
//!   or `blocks`; `sky` (the realistic map's weather): `live`, `sunny`, `cloudy`, `rain`, `fog` or
//!   `snow`;
//! - `labelSize`: one of the settings menu's sizes, 0.85, 1, 1.25 or 1.5
//!   ([`menus::LABEL_SIZES`]); `exaggeration`: a whole number from 1 to 40 (the menu's slider,
//!   [`menus::EXAGGERATION`]).
//!
//! A value is read strictly: only what the app itself can store. An unknown key is an error that
//! lists the keys; a bad value one that lists the values the key takes ([`Set::parse`]). `--set`
//! may be given any number of times; the last value of a key wins ([`crate::Options::parse`]).
//!
//! **What it does.** [`seed`] writes the values into the state's settings before the first frame,
//! as a saved settings file would have given them, and the run then saves no settings: its store
//! is one without a file ([`Store::default`], as without a backend), so neither the values set nor
//! anything changed in the window is written, as for a canned session (`--fixture`, `--demo`;
//! `state::settings`). The headless shot and the window seed the same way. Two keys have flags of
//! their own and meet them so: `basemap=` is `--basemap` (the one given last wins; the camera's
//! limits follow it from the start, `AppState::new`), and a shot's `--mode` is applied after the
//! `--set`s, so it wins over `mode=`.
//!
//! **A shot is repeatable** with `--now`: a `sky` preset lights the realistic map by that preset
//! and the live weather is not asked at all (`state::weather::weather_live`); the live sky (`live`,
//! the default) of a shot never asks MET Norway: it reads the `--fixture` folder's recorded
//! forecast (`state::weather::FIXTURE_FORECAST`), and is a fine day without one or without a
//! folder (`headless`). The radar (`weather=true`) reads the folder's RainViewer frame and
//! volumes, the terrain the folder's surface tiles.
//!
//! Deviation: today's app has no command line (its dev address `#@lat,lon,zoom,pitch,bearing`
//! sets only the view); `--set` is the port's, for shots and reviews.

use rustradar_core::geo::Basemap;
use rustradar_core::settings::{MODES, Mode, RAIL_SHOWS, RailShows, Settings};
use rustradar_core::weather::{SKIES, Sky};

use crate::state::AppState;
use crate::state::settings::Store;
use crate::ui::menus;

/// The keys `--set` takes: every setting, named as the settings file names it, in its order.
pub const KEYS: [&str; 16] = [
    "mode",
    "alsoShow",
    "basemap",
    "labels",
    "labelSize",
    "buildings",
    "aircraftLabels",
    "daylight",
    "weather",
    "adsb",
    "railMap",
    "railShows",
    "sky",
    "tiltShift",
    "terrain",
    "exaggeration",
];

/// The basemaps by their names in the settings.
const BASEMAPS: [Basemap; 3] = [Basemap::Dark, Basemap::Satellite, Basemap::Realistic];

/// One `--set KEY=VALUE`: a setting's value for this run.
#[derive(Debug, Clone, PartialEq)]
pub enum Set {
    Mode(Mode),
    AlsoShow(Vec<Mode>),
    Basemap(Basemap),
    Labels(bool),
    LabelSize(f64),
    Buildings(bool),
    AircraftLabels(bool),
    Daylight(bool),
    /// The weather radar.
    Weather(bool),
    Adsb(bool),
    RailMap(bool),
    RailShows(RailShows),
    /// The realistic map's weather: live, or a preset.
    Sky(Sky),
    TiltShift(bool),
    Terrain(bool),
    Exaggeration(f64),
}

impl Set {
    /// Reads `KEY=VALUE` (what follows `--set`). The error says what was wrong: the keys there are
    /// for an unknown one, the values the key takes for a bad value.
    pub fn parse(text: &str) -> Result<Set, String> {
        let Some((key, value)) = text.split_once('=') else {
            return Err(format!("--set wants KEY=VALUE, e.g. terrain=true, not {text:?}; the keys: {}", KEYS.join(", ")));
        };
        let wants = |what: &str| format!("--set {key} wants {what}, not {value:?}");
        let flag = || match value {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(wants("true or false")),
        };
        Ok(match key {
            "mode" => Set::Mode(named(value, &MODES, Mode::as_str).ok_or_else(|| wants(&one_of(&MODES.map(Mode::as_str))))?),
            "alsoShow" => {
                let list = || wants(&format!("a comma-separated list of {} (or nothing)", all_of(&MODES.map(Mode::as_str))));
                let kinds = if value.is_empty() {
                    Vec::new()
                } else {
                    value.split(',').map(|name| named(name, &MODES, Mode::as_str)).collect::<Option<Vec<_>>>().ok_or_else(list)?
                };
                // in the settings' order, each once (as `mergeSettings` keeps them)
                Set::AlsoShow(MODES.into_iter().filter(|m| kinds.contains(m)).collect())
            }
            "basemap" => Set::Basemap(named(value, &BASEMAPS, Basemap::as_str).ok_or_else(|| wants(&one_of(&BASEMAPS.map(Basemap::as_str))))?),
            "labels" => Set::Labels(flag()?),
            "labelSize" => {
                let sizes = menus::LABEL_SIZES.map(|(size, _)| size);
                let size = value.parse::<f64>().ok().filter(|v| sizes.contains(v));
                Set::LabelSize(size.ok_or_else(|| wants(&one_of(&sizes.map(|s| s.to_string()))))?)
            }
            "buildings" => Set::Buildings(flag()?),
            "aircraftLabels" => Set::AircraftLabels(flag()?),
            "daylight" => Set::Daylight(flag()?),
            "weather" => Set::Weather(flag()?),
            "adsb" => Set::Adsb(flag()?),
            "railMap" => Set::RailMap(flag()?),
            "railShows" => Set::RailShows(named(value, &RAIL_SHOWS, RailShows::as_str).ok_or_else(|| wants(&one_of(&RAIL_SHOWS.map(RailShows::as_str))))?),
            "sky" => Set::Sky(named(value, &SKIES, Sky::as_str).ok_or_else(|| wants(&one_of(&SKIES.map(Sky::as_str))))?),
            "tiltShift" => Set::TiltShift(flag()?),
            "terrain" => Set::Terrain(flag()?),
            "exaggeration" => {
                let (min, max, step) = menus::EXAGGERATION;
                let v = value.parse::<f64>().ok().filter(|v| (min..=max).contains(v) && (v - min) % step == 0.0);
                Set::Exaggeration(v.ok_or_else(|| wants(&format!("a whole number from {min} to {max}")))?)
            }
            _ => return Err(format!("--set: there is no setting {key:?}; the settings: {}", KEYS.join(", "))),
        })
    }

    /// The key, as the settings file names it.
    pub fn key(&self) -> &'static str {
        match self {
            Set::Mode(_) => "mode",
            Set::AlsoShow(_) => "alsoShow",
            Set::Basemap(_) => "basemap",
            Set::Labels(_) => "labels",
            Set::LabelSize(_) => "labelSize",
            Set::Buildings(_) => "buildings",
            Set::AircraftLabels(_) => "aircraftLabels",
            Set::Daylight(_) => "daylight",
            Set::Weather(_) => "weather",
            Set::Adsb(_) => "adsb",
            Set::RailMap(_) => "railMap",
            Set::RailShows(_) => "railShows",
            Set::Sky(_) => "sky",
            Set::TiltShift(_) => "tiltShift",
            Set::Terrain(_) => "terrain",
            Set::Exaggeration(_) => "exaggeration",
        }
    }

    /// Writes the value into `settings`.
    pub fn apply(&self, settings: &mut Settings) {
        match self {
            Set::Mode(v) => settings.mode = *v,
            Set::AlsoShow(v) => settings.also_show = v.clone(),
            Set::Basemap(v) => settings.basemap = *v,
            Set::Labels(v) => settings.labels = *v,
            Set::LabelSize(v) => settings.label_size = *v,
            Set::Buildings(v) => settings.buildings = *v,
            Set::AircraftLabels(v) => settings.aircraft_labels = *v,
            Set::Daylight(v) => settings.daylight = *v,
            Set::Weather(v) => settings.weather = *v,
            Set::Adsb(v) => settings.adsb = *v,
            Set::RailMap(v) => settings.rail_map = *v,
            Set::RailShows(v) => settings.rail_shows = *v,
            Set::Sky(v) => settings.sky = *v,
            Set::TiltShift(v) => settings.tilt_shift = *v,
            Set::Terrain(v) => settings.terrain = *v,
            Set::Exaggeration(v) => settings.exaggeration = *v,
        }
    }
}

/// Seeds `state` with the command line's `--set`s before its first frame: their values in its
/// settings, and a store that writes nothing, so they hold for this run only (see the module
/// documentation). Without any, `state` is left as it is. A basemap is the command line's
/// `--basemap` ([`crate::Options::parse`] puts it there), read when the state is made.
pub fn seed(state: &mut AppState, set: &[Set]) {
    if set.is_empty() {
        return;
    }
    for s in set {
        s.apply(&mut state.persisted.settings);
    }
    state.settings = Store::default();
}

/// The choice named `name`, if any.
fn named<T: Copy>(name: &str, choices: &[T], name_of: fn(T) -> &'static str) -> Option<T> {
    choices.iter().copied().find(|&c| name_of(c) == name)
}

/// `a, b or c`.
fn one_of<S: AsRef<str>>(names: &[S]) -> String {
    joined(names, "or")
}

/// `a, b and c`.
fn all_of<S: AsRef<str>>(names: &[S]) -> String {
    joined(names, "and")
}

fn joined<S: AsRef<str>>(names: &[S], last: &str) -> String {
    let names: Vec<&str> = names.iter().map(AsRef::as_ref).collect();
    match names.split_last() {
        Some((tail, [])) => (*tail).to_owned(),
        Some((tail, rest)) => format!("{} {last} {tail}", rest.join(", ")),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::settings::DEFAULT_SETTINGS;
    use rustradar_scene::camera::ViewState;
    use serde_json::Value;

    use super::*;
    use crate::state::Startup;
    use crate::state::weather::weather_live;
    use crate::{ArgsError, Options, USAGE};

    fn parse(args: &[&str]) -> Result<Options, ArgsError> {
        Options::parse(args.iter().map(|a| a.to_string()))
    }

    fn bad(args: &[&str]) -> String {
        match parse(args) {
            Err(ArgsError::Bad(why)) => why,
            other => panic!("{args:?} gave {other:?}"),
        }
    }

    /// The settings `--set`s give over the defaults.
    fn seeded(args: &[&str]) -> Settings {
        let options = parse(args).expect("parsed");
        let mut settings = DEFAULT_SETTINGS;
        for s in &options.set {
            s.apply(&mut settings);
        }
        settings
    }

    #[test]
    fn every_setting_has_its_key_as_the_settings_file_names_it() {
        let file = serde_json::to_value(DEFAULT_SETTINGS).expect("the settings as JSON");
        let mut names: Vec<&str> = file.as_object().expect("an object").keys().map(String::as_str).collect();
        let mut keys = KEYS.to_vec();
        names.sort_unstable();
        keys.sort_unstable();
        assert_eq!(names, keys, "the file's keys");
        // in the file's order
        let text = rustradar_core::settings::persisted_json(&Default::default());
        let at: Vec<usize> = KEYS.iter().map(|k| text.find(&format!("\"{k}\":")).expect("in the file")).collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]), "{text}");
        // and each key reads back as itself, its default value written as the file writes it
        for key in KEYS {
            let value = match &file[key] {
                Value::String(s) => s.clone(),
                Value::Array(a) => a.iter().map(|v| v.as_str().unwrap().to_owned()).collect::<Vec<_>>().join(","),
                other => other.to_string(),
            };
            let set = Set::parse(&format!("{key}={value}")).unwrap_or_else(|e| panic!("{key}={value}: {e}"));
            assert_eq!(set.key(), key);
            let mut settings = DEFAULT_SETTINGS;
            set.apply(&mut settings);
            assert_eq!(settings, DEFAULT_SETTINGS, "{key}={value} is the default");
            assert!(USAGE.contains(key), "--help names {key}");
        }
    }

    #[test]
    fn good_values_are_read() {
        let s = seeded(&[
            "--set", "terrain=true", "--set", "sky=rain", "--set", "tiltShift=true", "--set", "weather=true", "--set", "labels=false",
            "--set", "labelSize=1.25", "--set", "buildings=false", "--set", "aircraftLabels=true", "--set", "daylight=false",
            "--set", "adsb=false", "--set", "railMap=true", "--set", "railShows=blocks", "--set", "exaggeration=3",
            "--set", "mode=ships", "--set", "alsoShow=ships,aircraft",
        ]);
        let want = Settings {
            terrain: true,
            sky: Sky::Rain,
            tilt_shift: true,
            weather: true,
            labels: false,
            label_size: 1.25,
            buildings: false,
            aircraft_labels: true,
            daylight: false,
            adsb: false,
            rail_map: true,
            rail_shows: RailShows::Blocks,
            exaggeration: 3.0,
            mode: Mode::Ships,
            also_show: vec![Mode::Aircraft, Mode::Ships],
            ..DEFAULT_SETTINGS
        };
        assert_eq!(s, want);
        // every sky, the four sizes, the slider's ends, no other kinds
        for sky in SKIES {
            assert_eq!(seeded(&["--set", &format!("sky={}", sky.as_str())]).sky, sky);
        }
        for (size, _) in menus::LABEL_SIZES {
            assert_eq!(seeded(&["--set", &format!("labelSize={size}")]).label_size, size);
        }
        assert_eq!(seeded(&["--set", "labelSize=1.0"]).label_size, 1.0);
        assert_eq!(seeded(&["--set", "exaggeration=1"]).exaggeration, 1.0);
        assert_eq!(seeded(&["--set", "exaggeration=40"]).exaggeration, 40.0);
        assert_eq!(seeded(&["--set", "alsoShow=trains", "--set", "alsoShow="]).also_show, Vec::<Mode>::new());
        // a basemap is --basemap's
        let o = parse(&["--set", "basemap=realistic"]).expect("parsed");
        assert_eq!((o.basemap, o.set.as_slice()), (Some(Basemap::Realistic), &[][..]));
        assert_eq!(o.startup(), Startup { basemap: Some(Basemap::Realistic), ..Default::default() });
        // for the window too, and with a shot
        assert!(parse(&["--set", "terrain=true"]).is_ok());
        assert!(parse(&["--screenshot", "a.png", "--set", "terrain=true"]).is_ok());
    }

    #[test]
    fn an_unknown_key_lists_the_keys() {
        for wrong in ["tilt=true", "tilt_shift=true", "TiltShift=true", "radar=true", "=true"] {
            let why = bad(&["--set", wrong]);
            assert!(why.starts_with("--set: there is no setting"), "{wrong}: {why}");
            assert!(why.contains(&KEYS.join(", ")), "{wrong}: {why}");
        }
        let why = bad(&["--set", "terrain"]);
        assert!(why.contains("KEY=VALUE") && why.contains(&KEYS.join(", ")), "{why}");
        assert!(bad(&["--set"]).contains("needs a value"));
    }

    #[test]
    fn a_bad_value_lists_the_values() {
        let cases = [
            ("terrain=yes", "--set terrain wants true or false, not \"yes\""),
            ("terrain=", "--set terrain wants true or false, not \"\""),
            ("labels=True", "--set labels wants true or false, not \"True\""),
            ("sky=storm", "--set sky wants live, sunny, cloudy, rain, fog or snow, not \"storm\""),
            ("sky=Rain", "--set sky wants live, sunny, cloudy, rain, fog or snow, not \"Rain\""),
            ("mode=boats", "--set mode wants aircraft, trains or ships, not \"boats\""),
            ("basemap=light", "--set basemap wants dark, satellite or realistic, not \"light\""),
            ("railShows=signals", "--set railShows wants speeds or blocks, not \"signals\""),
            ("labelSize=2", "--set labelSize wants 0.85, 1, 1.25 or 1.5, not \"2\""),
            ("labelSize=big", "--set labelSize wants 0.85, 1, 1.25 or 1.5, not \"big\""),
            ("exaggeration=41", "--set exaggeration wants a whole number from 1 to 40, not \"41\""),
            ("exaggeration=0", "--set exaggeration wants a whole number from 1 to 40, not \"0\""),
            ("exaggeration=2.5", "--set exaggeration wants a whole number from 1 to 40, not \"2.5\""),
            ("exaggeration=NaN", "--set exaggeration wants a whole number from 1 to 40, not \"NaN\""),
            ("alsoShow=trains,boats", "--set alsoShow wants a comma-separated list of aircraft, trains and ships (or nothing), not \"trains,boats\""),
            ("alsoShow=trains,", "--set alsoShow wants a comma-separated list of aircraft, trains and ships (or nothing), not \"trains,\""),
        ];
        for (arg, want) in cases {
            assert_eq!(bad(&["--set", arg]), want);
        }
        // and the whole message ends with the usage
        let e = parse(&["--set", "sky=storm"]).unwrap_err();
        assert!(e.to_string().ends_with(USAGE));
    }

    #[test]
    fn a_key_set_again_takes_its_last_value() {
        let o = parse(&["--set", "terrain=true", "--set", "sky=rain", "--set", "terrain=false"]).expect("parsed");
        assert_eq!(o.set, [Set::Sky(Sky::Rain), Set::Terrain(false)], "each key once, the last value");
        assert!(!seeded(&["--set", "terrain=true", "--set", "terrain=false"]).terrain);
        assert_eq!(seeded(&["--set", "sky=rain", "--set", "sky=snow"]).sky, Sky::Snow);
        // between --basemap and basemap=, the last one given
        assert_eq!(parse(&["--basemap", "dark", "--set", "basemap=realistic"]).unwrap().basemap, Some(Basemap::Realistic));
        assert_eq!(parse(&["--set", "basemap=realistic", "--basemap", "satellite"]).unwrap().basemap, Some(Basemap::Satellite));
    }

    /// The state seeded by `--set`: the values in its settings before the first frame, the live
    /// weather off with a preset sky, and nothing saved.
    #[test]
    fn a_seeded_state_holds_the_values_and_saves_nothing() {
        let options = parse(&["--basemap", "realistic", "--set", "sky=rain", "--set", "terrain=true"]).expect("parsed");
        let mut state = AppState::new(options.startup(), None, None, None, ViewState::INITIAL, [1100.0, 720.0]);
        seed(&mut state, &options.set);
        let s = &state.persisted.settings;
        assert_eq!((s.basemap, s.sky, s.terrain), (Basemap::Realistic, Sky::Rain, true));
        assert!(!weather_live(s.basemap, s.sky), "a preset: the live weather is not asked");
        assert!(!state.settings.writes() && state.settings.path().is_none(), "nothing is written");
        // no --set: the state as it was made
        let plain = parse(&["--basemap", "realistic"]).expect("parsed");
        let mut state = AppState::new(plain.startup(), None, None, None, ViewState::INITIAL, [1100.0, 720.0]);
        let before = state.persisted.clone();
        seed(&mut state, &plain.set);
        assert_eq!(state.persisted, before);
    }

    #[test]
    fn lists_are_written_as_english_lists() {
        assert_eq!(one_of(&["a"]), "a");
        assert_eq!(one_of(&["a", "b"]), "a or b");
        assert_eq!(all_of(&["a", "b", "c"]), "a, b and c");
    }
}
