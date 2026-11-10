//! Port of `filters.ts`, settings part (S1.12; doc 05 §6.5, doc 06 §23): the app's settings,
//! how stored settings are read back (`mergeSettings`, with the legacy keys), which kinds of
//! traffic are on the map (`shownKinds`), and the one persisted object of settings and filters.
//!
//! The TypeScript kept the persisted object in the web view's localStorage under `rustradar.v1`.
//! The native app keeps the same JSON in a file (`app::persist`, `settings.json` in the config
//! folder); this module reads it ([`load_persisted`]) and writes it ([`persisted_json`]) without
//! touching the disk. Stored values are read through `serde_json::Value` by the merge functions,
//! never with a plain `Deserialize`, so that every loading rule of doc 06 §23 holds.

use std::io;

use serde::Serialize;
use serde_json::ser::Formatter;
use serde_json::{Serializer, Value};

use crate::filters::{Filters, merge_filters};
use crate::geo::Basemap;
use crate::num::js_string;
use crate::ship_list::{ShipFilters, merge_ship_filters};
use crate::train_list::{TrainFilters, merge_train_filters};
use crate::weather::Sky;

/// The kind of traffic the sidebar is for; the globe shows it and any others in `also_show`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Aircraft,
    Trains,
    Ships,
}

/// Every mode, in the TypeScript's `MODES` order (the order `also_show` is kept in).
pub const MODES: [Mode; 3] = [Mode::Aircraft, Mode::Trains, Mode::Ships];

impl Mode {
    /// The name in the settings and in the TypeScript.
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Aircraft => "aircraft",
            Mode::Trains => "trains",
            Mode::Ships => "ships",
        }
    }

    /// The mode a name stands for (`MODES.includes(name)`); `None` for any other text.
    pub fn from_name(name: &str) -> Option<Mode> {
        MODES.into_iter().find(|m| m.as_str() == name)
    }
}

/// What the rail map shows: the track's speed limits, or its signal blocks (close in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RailShows {
    #[default]
    Speeds,
    Blocks,
}

/// Every choice, in the TypeScript's `RAIL_SHOWS` order.
pub const RAIL_SHOWS: [RailShows; 2] = [RailShows::Speeds, RailShows::Blocks];

impl RailShows {
    /// The name in the settings and in the TypeScript.
    pub const fn as_str(self) -> &'static str {
        match self {
            RailShows::Speeds => "speeds",
            RailShows::Blocks => "blocks",
        }
    }

    /// The choice a name stands for (`RAIL_SHOWS.includes(name)`); `None` for any other text.
    pub fn from_name(name: &str) -> Option<RailShows> {
        RAIL_SHOWS.into_iter().find(|r| r.as_str() == name)
    }
}

/// The app's settings, stored as the `settings` object of the settings file (doc 06 §23).
/// Read stored settings with [`merge_settings`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// The focus: aircraft, trains or ships. Its kind is always on the map, and the sidebar is for it.
    pub mode: Mode,
    /// Other kinds also fetched and drawn on the map; the focus is shown whether listed or not.
    pub also_show: Vec<Mode>,
    pub basemap: Basemap,
    /// Place names on the map.
    pub labels: bool,
    /// Place-name size on the map, as a multiple of the default (0.85, 1, 1.25 or 1.5).
    pub label_size: f64,
    pub buildings: bool,
    /// Callsigns written next to the aircraft on the map.
    pub aircraft_labels: bool,
    /// Day and night shading.
    pub daylight: bool,
    /// The weather radar.
    pub weather: bool,
    /// Measured data for the selected aircraft from adsb.lol.
    pub adsb: bool,
    /// Railway lines from OpenRailwayMap.
    pub rail_map: bool,
    /// What the rail map shows: speed limits, or signal blocks close in (free, or taken by the
    /// train drawn in them).
    pub rail_shows: RailShows,
    /// The realistic map's weather: live where the map is looking, or one chosen.
    pub sky: Sky,
    /// The realistic map's top and bottom out of focus, as through a tilt-shift lens.
    pub tilt_shift: bool,
    /// Relief from the AWS Terrain Tiles (Mapzen).
    pub terrain: bool,
    /// Vertical exaggeration of altitudes when zoomed out, 1 = true scale (1 to 40).
    pub exaggeration: f64,
}

pub const DEFAULT_SETTINGS: Settings = Settings {
    mode: Mode::Aircraft,
    also_show: Vec::new(),
    basemap: Basemap::Dark,
    labels: true,
    label_size: 1.0,
    buildings: true,
    aircraft_labels: false,
    daylight: true,
    weather: false,
    adsb: true,
    rail_map: false,
    rail_shows: RailShows::Speeds,
    sky: Sky::Live,
    tilt_shift: false,
    terrain: false,
    exaggeration: 10.0,
};

impl Default for Settings {
    fn default() -> Self {
        DEFAULT_SETTINGS
    }
}

/// Stored settings over the defaults. `saved` is the stored `settings` value (`None` where
/// nothing is stored). A setting added since they were saved takes its default, and one stored
/// with the wrong JSON type (an older format, a hand edit) falls back to it too; keys that are
/// not settings are dropped.
///
/// Settings from before the modes had an on/off switch each for `aircraft`, `ships` and `trains`:
/// those are dropped, and the mode starts on aircraft unless the aircraft were switched off, then
/// on whichever of ships or trains was still on (a missing switch counts as on). Signal blocks
/// were once a switch of their own: `trainBlocks: true` with no `railShows` stored turns on the
/// rail map showing the blocks.
///
/// One deviation: the TypeScript kept any string as the basemap (and then drew an unknown one as
/// the dark map); here an unknown basemap is the default, the dark map.
pub fn merge_settings(saved: Option<&Value>) -> Settings {
    let mut merged = DEFAULT_SETTINGS;
    let Some(old) = saved.and_then(Value::as_object) else {
        // nothing, a primitive, or an array (whose entries are no settings): the defaults
        return merged;
    };
    // copy each stored key that is a setting and has the default's JSON type
    let flag = |key: &str, field: &mut bool| {
        if let Some(b) = old.get(key).and_then(Value::as_bool) {
            *field = b;
        }
    };
    flag("labels", &mut merged.labels);
    flag("buildings", &mut merged.buildings);
    flag("aircraftLabels", &mut merged.aircraft_labels);
    flag("daylight", &mut merged.daylight);
    flag("weather", &mut merged.weather);
    flag("adsb", &mut merged.adsb);
    flag("railMap", &mut merged.rail_map);
    flag("tiltShift", &mut merged.tilt_shift);
    flag("terrain", &mut merged.terrain);
    let number = |key: &str, field: &mut f64| {
        if let Some(x) = old.get(key).and_then(Value::as_f64) {
            *field = x;
        }
    };
    number("labelSize", &mut merged.label_size);
    number("exaggeration", &mut merged.exaggeration);
    let name = |key: &str| old.get(key).and_then(Value::as_str);
    if let Some(basemap) = name("basemap").and_then(basemap_named) {
        merged.basemap = basemap;
    }
    // an array is an object to typeof: anything but an array of known kinds is none
    merged.also_show = match old.get("alsoShow").and_then(Value::as_array) {
        Some(kinds) => MODES.into_iter().filter(|m| kinds.iter().any(|k| k.as_str() == Some(m.as_str()))).collect(),
        None => Vec::new(),
    };
    // an unknown sky or rail choice falls back to the default
    merged.sky = name("sky").and_then(Sky::from_name).unwrap_or(DEFAULT_SETTINGS.sky);
    merged.rail_shows = name("railShows").and_then(RailShows::from_name).unwrap_or(DEFAULT_SETTINGS.rail_shows);
    // signal blocks were a switch of their own, drawn with or without the rail map
    if old.get("trainBlocks") == Some(&Value::Bool(true)) && !old.contains_key("railShows") {
        merged.rail_map = true;
        merged.rail_shows = RailShows::Blocks;
    }
    merged.mode = match name("mode").and_then(Mode::from_name) {
        Some(mode) => mode,
        // the old switches defaulted to on: missing counts as on
        None => {
            let off = |key: &str| old.get(key) == Some(&Value::Bool(false));
            if !off("aircraft") {
                Mode::Aircraft
            } else if !off("ships") {
                Mode::Ships
            } else if !off("trains") {
                Mode::Trains
            } else {
                DEFAULT_SETTINGS.mode
            }
        }
    };
    merged
}

/// The basemap a stored name stands for.
fn basemap_named(name: &str) -> Option<Basemap> {
    [Basemap::Dark, Basemap::Satellite, Basemap::Realistic].into_iter().find(|b| b.as_str() == name)
}

/// Which kinds are on the map (`shownKinds`'s `Record<Mode, boolean>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShownKinds {
    pub aircraft: bool,
    pub trains: bool,
    pub ships: bool,
}

impl ShownKinds {
    /// Whether this kind is on the map.
    pub const fn shows(self, mode: Mode) -> bool {
        match mode {
            Mode::Aircraft => self.aircraft,
            Mode::Trains => self.trains,
            Mode::Ships => self.ships,
        }
    }
}

/// Which kinds are on the map: the focus, and whatever else is shown with it.
pub fn shown_kinds(mode: Mode, also_show: &[Mode]) -> ShownKinds {
    let on = |m: Mode| m == mode || also_show.contains(&m);
    ShownKinds { aircraft: on(Mode::Aircraft), trains: on(Mode::Trains), ships: on(Mode::Ships) }
}

/// Everything persisted: the settings file's one JSON object (doc 06 §23).
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Persisted {
    pub settings: Settings,
    pub filters: Filters,
    pub train_filters: TrainFilters,
    pub ship_filters: ShipFilters,
}

/// The persisted object read back from the stored text (`loadPersisted`): `None` where nothing
/// is stored. Text that is not JSON, or JSON that is not an object, gives the defaults; each part
/// is then read by its merge function ([`merge_settings`], [`merge_filters`],
/// [`merge_train_filters`], [`merge_ship_filters`]), so a part missing or unreadable on its own
/// takes its defaults.
pub fn load_persisted(stored: Option<&str>) -> Persisted {
    // unreadable: the defaults (`null`, a number or a string has no parts either)
    let saved = stored.and_then(|text| serde_json::from_str::<Value>(text).ok()).unwrap_or(Value::Null);
    Persisted {
        settings: merge_settings(saved.get("settings")),
        filters: merge_filters(saved.get("filters")),
        train_filters: merge_train_filters(saved.get("trainFilters")),
        ship_filters: merge_ship_filters(saved.get("shipFilters")),
    }
}

/// The text to store (`JSON.stringify(state)`): compact, keys in the order above, numbers as
/// JavaScript writes them (`"labelSize":1`, where serde_json writes `1.0`).
pub fn persisted_json(state: &Persisted) -> String {
    let mut out = Vec::new();
    let mut serializer = Serializer::with_formatter(&mut out, JsNumbers);
    state.serialize(&mut serializer).expect("plain data always serialises");
    String::from_utf8(out).expect("serde_json writes UTF-8")
}

/// serde_json's compact layout with JavaScript's numbers.
struct JsNumbers;

impl Formatter for JsNumbers {
    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        // serde_json writes a NaN or an infinity as `null` before it gets here, as JSON.stringify does
        writer.write_all(js_string(value).as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::model::TrainKind;
    use crate::ship_list::{DEFAULT_SHIP_FILTERS, ShipMotion, ShipSort};
    use crate::train_list::{DEFAULT_TRAIN_FILTERS, TrainSort};

    fn merge(saved: Value) -> Settings {
        merge_settings(Some(&saved))
    }

    /// The keys a merged result has when written.
    fn keys(s: &Settings) -> Vec<String> {
        serde_json::to_value(s).unwrap().as_object().unwrap().keys().cloned().collect()
    }

    // `filters.test.ts`: `mergeSettings`

    #[test]
    fn settings_saved_before_rail_map_and_terrain_existed_take_their_defaults() {
        let s = merge(json!({ "basemap": "satellite", "labels": false, "exaggeration": 4 }));
        assert_eq!(s.basemap, Basemap::Satellite);
        assert!(!s.labels);
        assert_eq!(s.exaggeration, 4.0);
        assert!(!s.rail_map);
        assert!(!s.terrain);
        assert_eq!(s.mode, Mode::Aircraft);
    }

    #[test]
    fn stored_choices_for_the_newer_settings_are_kept() {
        let s = merge(json!({ "mode": "trains", "railMap": true, "terrain": true }));
        assert_eq!((s.mode, s.rail_map, s.terrain), (Mode::Trains, true, true));
    }

    #[test]
    fn nothing_saved_or_something_unreadable_gives_the_defaults() {
        assert_eq!(merge_settings(None), DEFAULT_SETTINGS);
        assert_eq!(merge(json!("nonsense")), DEFAULT_SETTINGS);
    }

    #[test]
    fn unknown_keys_and_values_of_the_wrong_type_are_dropped() {
        let s = merge(json!({ "railMap": "yes", "exaggeration": null, "legacy": 1 }));
        assert!(!s.rail_map);
        assert_eq!(s.exaggeration, DEFAULT_SETTINGS.exaggeration);
        assert!(!keys(&s).contains(&"legacy".to_owned()));
    }

    #[test]
    fn the_old_signal_blocks_switch_becomes_the_rail_map_showing_the_blocks() {
        let s = merge(json!({ "trainBlocks": true }));
        assert_eq!((s.rail_map, s.rail_shows), (true, RailShows::Blocks));
        assert_eq!(merge(json!({ "trainBlocks": false })).rail_shows, RailShows::Speeds);
        assert_eq!(merge(json!({ "railShows": "tunnels" })).rail_shows, RailShows::Speeds);
    }

    #[test]
    fn a_setting_since_removed_airline_liveries_now_always_on_is_dropped() {
        let s = merge(json!({ "liveries": false, "labels": false }));
        assert!(!keys(&s).contains(&"liveries".to_owned()));
        assert!(!s.labels);
        assert!(!s.aircraft_labels);
    }

    #[test]
    fn a_mode_that_isnt_one_of_the_three_falls_back_to_aircraft() {
        assert_eq!(merge(json!({ "mode": "boats" })).mode, Mode::Aircraft);
    }

    #[test]
    fn the_defaults_are_not_shared_with_the_result() {
        let mut s = merge(json!({}));
        s.mode = Mode::Ships;
        assert_eq!(DEFAULT_SETTINGS.mode, Mode::Aircraft);
    }

    // `filters.test.ts`: the old aircraft, ships and trains switches become the mode

    #[test]
    fn they_are_dropped_from_the_settings() {
        let s = merge(json!({ "aircraft": true, "ships": false, "trains": true }));
        let keys = keys(&s);
        assert!(!(keys.contains(&"aircraft".to_owned()) || keys.contains(&"ships".to_owned()) || keys.contains(&"trains".to_owned())));
    }

    #[test]
    fn aircraft_on_or_never_saved_starts_in_aircraft_mode() {
        assert_eq!(merge(json!({ "aircraft": true, "ships": true, "trains": true })).mode, Mode::Aircraft);
        assert_eq!(merge(json!({ "labels": true })).mode, Mode::Aircraft);
    }

    #[test]
    fn aircraft_off_starts_on_ships_or_on_trains_with_the_ships_off_too() {
        assert_eq!(merge(json!({ "aircraft": false, "ships": true, "trains": true })).mode, Mode::Ships);
        assert_eq!(merge(json!({ "aircraft": false })).mode, Mode::Ships);
        assert_eq!(merge(json!({ "aircraft": false, "ships": false, "trains": true })).mode, Mode::Trains);
    }

    #[test]
    fn everything_off_starts_on_aircraft() {
        assert_eq!(merge(json!({ "aircraft": false, "ships": false, "trains": false })).mode, Mode::Aircraft);
    }

    #[test]
    fn a_stored_mode_wins_over_the_old_switches() {
        assert_eq!(merge(json!({ "mode": "trains", "aircraft": false, "ships": true })).mode, Mode::Trains);
    }

    // `filters.test.ts`: `alsoShow`

    #[test]
    fn defaults_to_nothing_also_for_saves_from_before_it_or_before_the_modes() {
        assert_eq!(merge(json!({})).also_show, []);
        assert_eq!(merge(json!({ "aircraft": true, "ships": true, "trains": true })).also_show, []);
    }

    #[test]
    fn a_stored_list_of_kinds_is_kept() {
        assert_eq!(merge(json!({ "mode": "aircraft", "alsoShow": ["ships"] })).also_show, [Mode::Ships]);
    }

    #[test]
    fn unknown_kinds_and_duplicates_are_dropped() {
        let mut also = merge(json!({ "alsoShow": ["ships", "boats", "ships", 3, "trains"] })).also_show;
        also.sort_by_key(|m| m.as_str());
        assert_eq!(also, [Mode::Ships, Mode::Trains]);
    }

    #[test]
    fn anything_but_an_array_becomes_none() {
        for also_show in [json!({ "ships": true }), json!("ships"), json!(null), json!(1), json!(true)] {
            assert_eq!(merge(json!({ "alsoShow": also_show })).also_show, [], "{also_show}");
        }
    }

    #[test]
    fn the_result_doesnt_share_the_default_list() {
        merge(json!({})).also_show.push(Mode::Ships);
        assert_eq!(DEFAULT_SETTINGS.also_show, []);
    }

    #[test]
    fn an_unknown_mode_with_every_old_switch_off_still_lands_on_a_valid_mode() {
        assert_eq!(merge(json!({ "mode": "boats", "aircraft": false, "ships": false, "trains": false })).mode, Mode::Aircraft);
    }

    // `filters.test.ts`: `shownKinds`

    #[test]
    fn the_focus_alone() {
        assert_eq!(shown_kinds(Mode::Trains, &[]), ShownKinds { aircraft: false, trains: true, ships: false });
    }

    #[test]
    fn the_focus_and_the_kinds_shown_with_it() {
        assert_eq!(shown_kinds(Mode::Aircraft, &[Mode::Ships]), ShownKinds { aircraft: true, trains: false, ships: true });
    }

    #[test]
    fn the_focus_listed_in_also_show_too_changes_nothing() {
        assert_eq!(shown_kinds(Mode::Ships, &[Mode::Ships]), ShownKinds { aircraft: false, trains: false, ships: true });
    }

    // New tests: every loading rule of doc 06 §23

    #[test]
    fn the_defaults_are_those_of_the_settings_file_schema() {
        let text = persisted_json(&Persisted::default());
        let want = json!({
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
        });
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), want);
    }

    #[test]
    fn the_stored_text_is_what_json_stringify_writes() {
        // keys in the TypeScript's order, compact, and whole numbers without `.0`: the text below
        // is what `persist(loadPersisted())` stored, taken from the TypeScript under bun
        let settings = |label_size: &str, exaggeration: &str| {
            format!(
                r#"{{"settings":{{"mode":"aircraft","alsoShow":[],"basemap":"dark","labels":true,"labelSize":{label_size},"buildings":true,"aircraftLabels":false,"daylight":true,"weather":false,"adsb":true,"railMap":false,"railShows":"speeds","sky":"live","tiltShift":false,"terrain":false,"exaggeration":{exaggeration}}},"filters":{{"categories":[0,1,2,3,4,5,6,7,8,9,10,11],"altitude":[0,50000],"airborne":true,"onGround":true,"airlines":[],"types":[],"airports":[],"text":""}},"trainFilters":{{"hiddenKinds":[],"hiddenOperators":[],"delayedOnly":false,"sort":"delay"}},"shipFilters":{{"hiddenKinds":[],"motion":"all","hiddenSizes":[],"hideOld":false,"sort":"size"}}}}"#
            )
        };
        assert_eq!(persisted_json(&Persisted::default()), settings("1", "10"));
        let mut p = Persisted::default();
        p.settings.label_size = 0.85;
        p.settings.exaggeration = 25.0;
        assert_eq!(persisted_json(&p), settings("0.85", "25"));
    }

    #[test]
    fn what_is_stored_reads_back_the_same() {
        let p = Persisted {
            settings: Settings {
                mode: Mode::Ships,
                also_show: vec![Mode::Aircraft, Mode::Trains],
                basemap: Basemap::Realistic,
                labels: false,
                label_size: 1.25,
                buildings: false,
                aircraft_labels: true,
                daylight: false,
                weather: true,
                adsb: false,
                rail_map: true,
                rail_shows: RailShows::Blocks,
                sky: Sky::Snow,
                tilt_shift: true,
                terrain: true,
                exaggeration: 3.0,
            },
            filters: Filters {
                categories: vec![0, 3],
                altitude: (1_000, 20_000),
                airborne: false,
                on_ground: true,
                airlines: vec!["SAS".into()],
                types: vec!["A3".into()],
                airports: vec!["ARN".into()],
                text: "sk".into(),
            },
            train_filters: TrainFilters {
                hidden_kinds: vec![TrainKind::HighSpeed],
                hidden_operators: vec!["SJ".into(), "".into()],
                delayed_only: true,
                sort: TrainSort::Distance,
            },
            ship_filters: ShipFilters {
                hidden_kinds: vec![3, 7],
                motion: ShipMotion::Moored,
                hidden_sizes: vec![2],
                hide_old: true,
                sort: ShipSort::Name,
            },
        };
        assert_eq!(load_persisted(Some(&persisted_json(&p))), p);
    }

    #[test]
    fn unreadable_json_gives_all_the_defaults() {
        for stored in [None, Some(""), Some("{"), Some("not json"), Some("{\"settings\": {\"labels\": false}"), Some("null")] {
            assert_eq!(load_persisted(stored), Persisted::default(), "{stored:?}");
        }
        // JSON that is not an object has no parts
        for stored in ["5", "\"settings\"", "true", "[{\"settings\": {\"labels\": false}}]"] {
            assert_eq!(load_persisted(Some(stored)), Persisted::default(), "{stored}");
        }
    }

    #[test]
    fn each_part_missing_or_unreadable_on_its_own_takes_its_defaults() {
        let p = load_persisted(Some(r#"{"settings": {"labels": false}, "filters": 3, "trainFilters": null}"#));
        assert!(!p.settings.labels);
        assert_eq!(p.filters, Filters::default());
        assert_eq!(p.train_filters, DEFAULT_TRAIN_FILTERS);
        assert_eq!(p.ship_filters, DEFAULT_SHIP_FILTERS);
        let q = load_persisted(Some(r#"{"settings": [], "shipFilters": {"sort": "name"}}"#));
        assert_eq!(q.settings, DEFAULT_SETTINGS);
        assert_eq!(q.ship_filters.sort, ShipSort::Name);
    }

    #[test]
    fn settings_copy_only_values_of_the_defaults_json_type() {
        // booleans take only booleans
        for (key, field) in [
            ("labels", (|s: &Settings| s.labels) as fn(&Settings) -> bool),
            ("buildings", |s| s.buildings),
            ("aircraftLabels", |s| s.aircraft_labels),
            ("daylight", |s| s.daylight),
            ("weather", |s| s.weather),
            ("adsb", |s| s.adsb),
            ("railMap", |s| s.rail_map),
            ("tiltShift", |s| s.tilt_shift),
            ("terrain", |s| s.terrain),
        ] {
            let default = field(&DEFAULT_SETTINGS);
            assert_eq!(field(&merge(json!({ key: !default }))), !default, "{key}");
            for wrong in [json!(0), json!(1), json!("true"), json!(null), json!([true]), json!({})] {
                assert_eq!(field(&merge(json!({ key: wrong }))), default, "{key}: {wrong}");
            }
        }
        // numbers take any number, whole or not, and nothing else
        let s = merge(json!({ "labelSize": 1.5, "exaggeration": 40 }));
        assert_eq!((s.label_size, s.exaggeration), (1.5, 40.0));
        assert_eq!(merge(json!({ "labelSize": 0.85, "exaggeration": 2.5 })).label_size, 0.85);
        assert_eq!(merge(json!({ "exaggeration": 2.5 })).exaggeration, 2.5);
        for wrong in [json!("1.5"), json!(true), json!(null), json!([2])] {
            let s = merge(json!({ "labelSize": wrong, "exaggeration": wrong }));
            assert_eq!((s.label_size, s.exaggeration), (1.0, 10.0), "{wrong}");
        }
        // the basemap takes the three names
        for (name, basemap) in [("dark", Basemap::Dark), ("satellite", Basemap::Satellite), ("realistic", Basemap::Realistic)] {
            assert_eq!(merge(json!({ "basemap": name })).basemap, basemap);
        }
        for wrong in [json!("Satellite"), json!("watercolor"), json!(1), json!(null)] {
            assert_eq!(merge(json!({ "basemap": wrong })).basemap, Basemap::Dark, "{wrong}");
        }
    }

    #[test]
    fn a_known_sky_or_rail_choice_is_kept_and_any_other_falls_back() {
        for name in ["live", "sunny", "cloudy", "rain", "fog", "snow"] {
            assert_eq!(merge(json!({ "sky": name })).sky.as_str(), name);
        }
        for wrong in [json!("hail"), json!("Rain"), json!(3), json!(null), json!(["rain"])] {
            assert_eq!(merge(json!({ "sky": wrong })).sky, Sky::Live, "{wrong}");
        }
        assert_eq!(merge(json!({ "railShows": "blocks" })).rail_shows, RailShows::Blocks);
        assert_eq!(merge(json!({ "railShows": "speeds" })).rail_shows, RailShows::Speeds);
        for wrong in [json!("Blocks"), json!(true), json!(null)] {
            assert_eq!(merge(json!({ "railShows": wrong })).rail_shows, RailShows::Speeds, "{wrong}");
        }
    }

    #[test]
    fn also_show_keeps_the_modes_order_and_may_list_the_focus() {
        let s = merge(json!({ "mode": "ships", "alsoShow": ["ships", "trains", "aircraft"] }));
        assert_eq!(s.also_show, [Mode::Aircraft, Mode::Trains, Mode::Ships]);
        assert_eq!(merge(json!({ "alsoShow": ["Ships", "SHIPS", " ships", null, ["ships"]] })).also_show, []);
        assert_eq!(merge(json!({ "alsoShow": [] })).also_show, []);
    }

    #[test]
    fn the_signal_blocks_switch_counts_only_as_true_and_without_a_stored_rail_choice() {
        // a stored rail choice wins, even one that is not known or not text
        for rail_shows in [json!("speeds"), json!("tunnels"), json!(null)] {
            let s = merge(json!({ "trainBlocks": true, "railShows": rail_shows }));
            assert_eq!((s.rail_map, s.rail_shows), (false, RailShows::Speeds), "{rail_shows}");
        }
        assert!(!merge(json!({ "trainBlocks": true, "railShows": "blocks", "railMap": false })).rail_map);
        // only `true` itself
        for blocks in [json!("true"), json!(1), json!(null)] {
            let s = merge(json!({ "trainBlocks": blocks }));
            assert_eq!((s.rail_map, s.rail_shows), (false, RailShows::Speeds), "{blocks}");
        }
        // it turns the rail map on even when the rail map was stored off
        let s = merge(json!({ "trainBlocks": true, "railMap": false }));
        assert_eq!((s.rail_map, s.rail_shows), (true, RailShows::Blocks));
        assert!(!keys(&s).contains(&"trainBlocks".to_owned()));
    }

    #[test]
    fn only_a_stored_false_switches_a_kind_off() {
        // the old switches were booleans that defaulted to on: anything else counts as on
        for on in [json!(null), json!(0), json!("false"), json!(true)] {
            assert_eq!(merge(json!({ "aircraft": on })).mode, Mode::Aircraft, "{on}");
            assert_eq!(merge(json!({ "aircraft": false, "ships": on })).mode, Mode::Ships, "{on}");
            assert_eq!(merge(json!({ "aircraft": false, "ships": false, "trains": on })).mode, Mode::Trains, "{on}");
        }
        // a stored mode of the wrong type or case is no mode
        for mode in [json!(1), json!(null), json!("Trains"), json!(["trains"])] {
            assert_eq!(merge(json!({ "mode": mode, "aircraft": false })).mode, Mode::Ships, "{mode}");
        }
        for name in ["aircraft", "trains", "ships"] {
            assert_eq!(merge(json!({ "mode": name, "aircraft": false, "ships": false })).mode.as_str(), name);
        }
    }

    #[test]
    fn settings_that_are_not_an_object_give_the_defaults() {
        for saved in [json!(null), json!(0), json!(true), json!(["labels", false]), json!([{ "labels": false }])] {
            assert_eq!(merge(saved.clone()), DEFAULT_SETTINGS, "{saved}");
        }
    }

    #[test]
    fn train_and_ship_filters_are_checked_field_by_field() {
        let p = load_persisted(Some(
            r#"{
                "trainFilters": {"hiddenKinds": ["highSpeed", "tram", 3], "hiddenOperators": ["SJ", 4, ""], "delayedOnly": "yes", "sort": "colour"},
                "shipFilters": {"hiddenKinds": [3, 1, 9, "4"], "motion": "sailing", "hiddenSizes": [0, 5], "hideOld": true, "sort": "distance"}
            }"#,
        ));
        assert_eq!(p.train_filters.hidden_kinds, [TrainKind::HighSpeed]);
        assert_eq!(p.train_filters.hidden_operators, ["SJ", ""]);
        assert!(!p.train_filters.delayed_only);
        assert_eq!(p.train_filters.sort, TrainSort::Delay);
        assert_eq!(p.ship_filters.hidden_kinds, [3]);
        assert_eq!(p.ship_filters.motion, ShipMotion::All);
        assert_eq!(p.ship_filters.hidden_sizes, [0]);
        assert!(p.ship_filters.hide_old);
        assert_eq!(p.ship_filters.sort, ShipSort::Distance);
    }

    #[test]
    fn aircraft_filters_are_laid_over_the_defaults() {
        let p = load_persisted(Some(r#"{"filters": {"text": "SAS", "airborne": false, "unknown": 1}}"#));
        assert_eq!(p.filters, Filters { text: "SAS".into(), airborne: false, ..Filters::default() });
    }

    #[test]
    fn a_settings_file_from_before_the_modes_carries_over() {
        // the shape of a store written by an older release: switches per kind, signal blocks
        let p = load_persisted(Some(
            r#"{"settings":{"aircraft":false,"ships":false,"trains":true,"trainBlocks":true,"basemap":"satellite","labels":true,"liveries":false,"labelSize":1.25,"exaggeration":10}}"#,
        ));
        let want = Settings {
            mode: Mode::Trains,
            basemap: Basemap::Satellite,
            label_size: 1.25,
            rail_map: true,
            rail_shows: RailShows::Blocks,
            ..DEFAULT_SETTINGS
        };
        assert_eq!(p.settings, want);
        assert_eq!(p.filters, Filters::default());
    }

    #[test]
    fn modes_and_rail_choices_by_name() {
        assert_eq!(MODES.map(Mode::as_str), ["aircraft", "trains", "ships"]);
        assert_eq!(RAIL_SHOWS.map(RailShows::as_str), ["speeds", "blocks"]);
        for m in MODES {
            assert_eq!(Mode::from_name(m.as_str()), Some(m));
            assert_eq!(serde_json::to_value(m).unwrap(), json!(m.as_str()));
        }
        for r in RAIL_SHOWS {
            assert_eq!(RailShows::from_name(r.as_str()), Some(r));
            assert_eq!(serde_json::to_value(r).unwrap(), json!(r.as_str()));
        }
        assert_eq!(Mode::from_name("boats"), None);
        assert_eq!(RailShows::from_name("tunnels"), None);
        assert_eq!(Settings::default(), DEFAULT_SETTINGS);
    }

    #[test]
    fn shown_kinds_reads_by_mode() {
        let shown = shown_kinds(Mode::Aircraft, &[Mode::Trains]);
        assert!(shown.shows(Mode::Aircraft) && shown.shows(Mode::Trains) && !shown.shows(Mode::Ships));
    }
}
