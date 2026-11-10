//! Port of `filters.ts`, aircraft part (S1.12; doc 05 §6.5, doc 06 §23): the aircraft
//! categories, the aircraft filters, how they are read back from the settings file, and the
//! predicate that applies them.
//!
//! The settings half of `filters.ts` (`Settings`, `mergeSettings`, `shownKinds`, the persisted
//! object) is [`crate::settings`].

use std::collections::HashSet;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::model::LiveFlight;
use crate::reference::airline_icao;
use crate::trains::is_js_space;

/// One of Flightradar24's aircraft categories (`Service`), filtered server-side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Category {
    pub id: u8,
    pub label: &'static str,
}

/// Every category, by id.
pub const CATEGORIES: [Category; 12] = [
    Category { id: 0, label: "Passenger" },
    Category { id: 1, label: "Cargo" },
    Category { id: 2, label: "Military and government" },
    Category { id: 3, label: "Business jets" },
    Category { id: 4, label: "General aviation" },
    Category { id: 5, label: "Helicopters" },
    Category { id: 6, label: "Lighter than air" },
    Category { id: 7, label: "Gliders" },
    Category { id: 8, label: "Drones" },
    Category { id: 9, label: "Ground vehicles" },
    Category { id: 10, label: "Other" },
    Category { id: 11, label: "Uncategorised" },
];

/// The altitude slider's top, in feet; at this value there is no upper limit.
pub const MAX_ALTITUDE: i32 = 50_000;

/// The aircraft filters, stored as the `filters` object of the settings file (doc 06 §23).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Filters {
    /// Category ids to show; all of them means no filter.
    pub categories: Vec<u8>,
    /// Feet, inclusive; the top value means "and above".
    pub altitude: (i32, i32),
    pub airborne: bool,
    pub on_ground: bool,
    /// Airline ICAO designators.
    pub airlines: Vec<String>,
    /// Type designator prefixes, e.g. `A38` or `B77W`.
    pub types: Vec<String>,
    /// Airport IATA codes; a flight matches if it departs from or flies to one.
    pub airports: Vec<String>,
    /// Matches callsign, flight number or registration.
    pub text: String,
}

/// `DEFAULT_FILTERS`: every category, any altitude, in the air and on the ground.
impl Default for Filters {
    fn default() -> Self {
        Filters {
            categories: CATEGORIES.iter().map(|c| c.id).collect(),
            altitude: (0, MAX_ALTITUDE),
            airborne: true,
            on_ground: true,
            airlines: Vec::new(),
            types: Vec::new(),
            airports: Vec::new(),
            text: String::new(),
        }
    }
}

/// Stored aircraft filters over the defaults (`{ ...DEFAULT_FILTERS, ...saved.filters }`): each
/// stored field replaces the default's. `saved` is the stored value (`None` where nothing is
/// stored); anything but an object leaves the defaults.
///
/// The TypeScript did no validation and kept a stored value of the wrong type (which then broke
/// the filter); here such a field keeps its default, so does an altitude that is not a pair of
/// whole numbers, or a category id that is not a whole number from 0 to 255.
pub fn merge_filters(saved: Option<&Value>) -> Filters {
    let mut f = Filters::default();
    let Some(s) = saved.and_then(Value::as_object) else {
        return f;
    };
    overlay(s, "categories", &mut f.categories);
    overlay(s, "altitude", &mut f.altitude);
    overlay(s, "airborne", &mut f.airborne);
    overlay(s, "onGround", &mut f.on_ground);
    overlay(s, "airlines", &mut f.airlines);
    overlay(s, "types", &mut f.types);
    overlay(s, "airports", &mut f.airports);
    overlay(s, "text", &mut f.text);
    f
}

/// `field = saved[key]` where it is stored and reads as the field's type.
fn overlay<T: DeserializeOwned>(saved: &Map<String, Value>, key: &str, field: &mut T) {
    if let Some(value) = saved.get(key).and_then(|v| T::deserialize(v).ok()) {
        *field = value;
    }
}

/// Categories to request from the server, or `None` for all of them (with none selected nothing
/// is shown, and the request just keeps the count fresh).
pub fn server_categories(filters: &Filters) -> Option<Vec<u8>> {
    let n = filters.categories.len();
    if n == CATEGORIES.len() || n == 0 { None } else { Some(filters.categories.clone()) }
}

/// How many filter groups narrow the view, for the badge.
pub fn active_count(f: &Filters) -> usize {
    [
        f.categories.len() != CATEGORIES.len(),
        f.altitude.0 > 0 || f.altitude.1 < MAX_ALTITUDE,
        !f.airborne || !f.on_ground,
        !f.airlines.is_empty(),
        !f.types.is_empty(),
        !f.airports.is_empty(),
        !f.text.trim_matches(is_js_space).is_empty(),
    ]
    .into_iter()
    .filter(|on| *on)
    .count()
}

/// A predicate for the client-side filters (categories are applied by the server, but with none
/// selected nothing matches).
pub fn matcher(f: &Filters) -> impl Fn(&LiveFlight) -> bool + use<> {
    let (low, high) = f.altitude;
    let unbounded = high >= MAX_ALTITUDE;
    let airlines: HashSet<String> = f.airlines.iter().cloned().collect();
    let airports: HashSet<String> = f.airports.iter().cloned().collect();
    let text = f.text.trim_matches(is_js_space).to_uppercase();
    let none = f.categories.is_empty();
    let (airborne, on_ground) = (f.airborne, f.on_ground);
    let types = f.types.clone();
    move |flight: &LiveFlight| {
        if none {
            return false;
        }
        if if flight.on_ground { !on_ground } else { !airborne } {
            return false;
        }
        // aircraft on the ground are at 0 ft: a minimum altitude leaves them out
        if if flight.on_ground { low > 0 } else { flight.alt < low || (!unbounded && flight.alt > high) } {
            return false;
        }
        if !airlines.is_empty() {
            match airline_icao(&flight.callsign) {
                Some(icao) if airlines.contains(icao) => {}
                _ => return false,
            }
        }
        if !types.is_empty() && !types.iter().any(|t| flight.typecode.starts_with(t.as_str())) {
            return false;
        }
        if !airports.is_empty() && !airports.contains(&flight.origin) && !airports.contains(&flight.destination) {
            return false;
        }
        if !text.is_empty() && ![&flight.callsign, &flight.flight, &flight.reg].iter().any(|v| v.to_uppercase().contains(&text)) {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn flight(callsign: &str, alt: i32, on_ground: bool) -> LiveFlight {
        LiveFlight {
            callsign: callsign.into(),
            flight: "LH2416".into(),
            reg: "D-AIUA".into(),
            typecode: "A320".into(),
            origin: "ARN".into(),
            destination: "MUC".into(),
            alt,
            on_ground,
            ..LiveFlight::default()
        }
    }

    #[test]
    fn the_twelve_categories_by_id() {
        assert_eq!(CATEGORIES.map(|c| c.id), [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
        assert_eq!(CATEGORIES[2].label, "Military and government");
        assert_eq!(CATEGORIES[11].label, "Uncategorised");
    }

    #[test]
    fn the_defaults_show_everything() {
        let f = Filters::default();
        assert_eq!(f.categories, (0..12).collect::<Vec<u8>>());
        assert_eq!(f.altitude, (0, 50_000));
        assert!(f.airborne && f.on_ground);
        assert!(f.airlines.is_empty() && f.types.is_empty() && f.airports.is_empty() && f.text.is_empty());
        assert_eq!(active_count(&f), 0);
        assert_eq!(server_categories(&f), None);
        let all = matcher(&f);
        assert!(all(&flight("DLH4XK", 36_000, false)) && all(&flight("SEABC", 0, true)));
    }

    #[test]
    fn server_categories_none_for_all_or_none() {
        let mut f = Filters::default();
        assert_eq!(server_categories(&f), None);
        f.categories = vec![];
        assert_eq!(server_categories(&f), None);
        f.categories = vec![0, 5];
        assert_eq!(server_categories(&f), Some(vec![0, 5]));
    }

    #[test]
    fn active_count_counts_each_group_once() {
        let mut f = Filters::default();
        f.categories.pop();
        assert_eq!(active_count(&f), 1);
        f.altitude = (1_000, MAX_ALTITUDE);
        assert_eq!(active_count(&f), 2);
        f.altitude = (0, 49_000);
        assert_eq!(active_count(&f), 2);
        f.airborne = false;
        f.on_ground = false;
        assert_eq!(active_count(&f), 3);
        f.airlines = vec!["DLH".into()];
        f.types = vec!["A3".into()];
        f.airports = vec!["ARN".into()];
        assert_eq!(active_count(&f), 6);
        // blank text is no filter: JavaScript's trim
        f.text = " \t\u{a0}\u{feff}\n".into();
        assert_eq!(active_count(&f), 6);
        f.text = " sas ".into();
        assert_eq!(active_count(&f), 7);
    }

    #[test]
    fn no_categories_matches_nothing() {
        let f = Filters { categories: vec![], ..Filters::default() };
        assert!(!matcher(&f)(&flight("DLH4XK", 36_000, false)));
    }

    #[test]
    fn the_ground_and_air_switches() {
        let air = flight("DLH4XK", 36_000, false);
        let ground = flight("DLH4XK", 0, true);
        let only_air = matcher(&Filters { on_ground: false, ..Filters::default() });
        assert!(only_air(&air) && !only_air(&ground));
        let only_ground = matcher(&Filters { airborne: false, ..Filters::default() });
        assert!(!only_ground(&air) && only_ground(&ground));
    }

    #[test]
    fn altitude_bounds_are_inclusive_and_the_top_is_open() {
        let m = matcher(&Filters { altitude: (10_000, 30_000), ..Filters::default() });
        assert!(!m(&flight("A", 9_999, false)));
        assert!(m(&flight("A", 10_000, false)) && m(&flight("A", 30_000, false)));
        assert!(!m(&flight("A", 30_001, false)));
        let above = matcher(&Filters { altitude: (10_000, MAX_ALTITUDE), ..Filters::default() });
        assert!(above(&flight("A", 60_000, false)));
        // the reported altitude of an aircraft on the ground is not used...
        let below = matcher(&Filters { altitude: (0, 5_000), ..Filters::default() });
        assert!(below(&flight("A", 9_000, true)));
        // ...but any minimum leaves the aircraft on the ground out
        let min = matcher(&Filters { altitude: (1_000, MAX_ALTITUDE), ..Filters::default() });
        assert!(!min(&flight("A", 0, true)));
    }

    #[test]
    fn airlines_by_the_callsign_designator() {
        let m = matcher(&Filters { airlines: vec!["DLH".into(), "SAS".into()], ..Filters::default() });
        assert!(m(&flight("DLH4XK", 30_000, false)));
        assert!(m(&flight("SAS1234", 30_000, false)));
        assert!(!m(&flight("BAW12", 30_000, false)));
        // a callsign without the airline pattern matches no airline
        assert!(!m(&flight("DLHXK", 30_000, false)));
        assert!(!m(&flight("", 30_000, false)));
    }

    #[test]
    fn types_by_prefix() {
        let m = matcher(&Filters { types: vec!["B77".into(), "A32".into()], ..Filters::default() });
        assert!(m(&flight("A", 30_000, false)));
        let mut b = flight("A", 30_000, false);
        b.typecode = "B77W".into();
        assert!(m(&b));
        b.typecode = "B788".into();
        assert!(!m(&b));
        // prefixes are not upper-cased
        let lower = matcher(&Filters { types: vec!["a32".into()], ..Filters::default() });
        assert!(!lower(&flight("A", 30_000, false)));
    }

    #[test]
    fn airports_by_origin_or_destination() {
        let from = matcher(&Filters { airports: vec!["ARN".into()], ..Filters::default() });
        let to = matcher(&Filters { airports: vec!["MUC".into()], ..Filters::default() });
        let other = matcher(&Filters { airports: vec!["LHR".into()], ..Filters::default() });
        let f = flight("A", 30_000, false);
        assert!(from(&f) && to(&f) && !other(&f));
    }

    #[test]
    fn text_in_the_callsign_flight_number_or_registration_in_any_case() {
        let f = flight("DLH4XK", 30_000, false);
        for text in ["dlh4", " 4xk ", "lh2416", "d-aiu", "AIUA"] {
            assert!(matcher(&Filters { text: text.into(), ..Filters::default() })(&f), "{text}");
        }
        for text in ["A320", "ARN", "dlh 4"] {
            assert!(!matcher(&Filters { text: text.into(), ..Filters::default() })(&f), "{text}");
        }
        // JavaScript's toUpperCase: ß is SS
        let mut g = f.clone();
        g.reg = "STRASSE".into();
        assert!(matcher(&Filters { text: "straße".into(), ..Filters::default() })(&g));
    }

    #[test]
    fn every_filter_must_match() {
        let f = Filters {
            altitude: (20_000, MAX_ALTITUDE),
            airlines: vec!["DLH".into()],
            types: vec!["A3".into()],
            airports: vec!["MUC".into()],
            text: "lh".into(),
            ..Filters::default()
        };
        let m = matcher(&f);
        assert!(m(&flight("DLH4XK", 36_000, false)));
        assert!(!m(&flight("DLH4XK", 16_000, false)));
        assert!(!m(&flight("DLH4XK", 0, true)));
        let mut g = flight("DLH4XK", 36_000, false);
        g.destination = "FRA".into();
        g.origin = "MUC".into();
        assert!(m(&g));
        g.origin = "HAM".into();
        assert!(!m(&g));
    }

    #[test]
    fn stored_filters_are_laid_over_the_defaults() {
        let f = merge_filters(Some(&json!({ "categories": [0, 1], "text": "SAS", "airlines": ["SAS"] })));
        assert_eq!(f.categories, [0, 1]);
        assert_eq!(f.text, "SAS");
        assert_eq!(f.airlines, ["SAS"]);
        // the rest are the defaults
        assert_eq!(f, Filters { categories: vec![0, 1], text: "SAS".into(), airlines: vec!["SAS".into()], ..Filters::default() });
        let all = merge_filters(Some(&json!({
            "categories": [], "altitude": [2000, 30000], "airborne": false, "onGround": true,
            "airlines": ["DLH"], "types": ["B77"], "airports": ["ARN", "LHR"], "text": "x", "legacy": 1
        })));
        let want = Filters {
            categories: vec![],
            altitude: (2_000, 30_000),
            airborne: false,
            on_ground: true,
            airlines: vec!["DLH".into()],
            types: vec!["B77".into()],
            airports: vec!["ARN".into(), "LHR".into()],
            text: "x".into(),
        };
        assert_eq!(all, want);
    }

    #[test]
    fn stored_filters_that_are_not_an_object_give_the_defaults() {
        for saved in [json!(null), json!("filters"), json!([1, 2]), json!(3), json!(true), json!({})] {
            assert_eq!(merge_filters(Some(&saved)), Filters::default(), "{saved}");
        }
        assert_eq!(merge_filters(None), Filters::default());
    }

    #[test]
    fn a_stored_filter_of_the_wrong_type_keeps_its_default() {
        let f = merge_filters(Some(&json!({
            "categories": [0, "1"], "altitude": [0, 1000, 2000], "airborne": "no", "onGround": null,
            "airlines": "DLH", "types": [3], "airports": {}, "text": 5
        })));
        assert_eq!(f, Filters::default());
        assert_eq!(merge_filters(Some(&json!({ "categories": [300] }))).categories, Filters::default().categories);
        assert_eq!(merge_filters(Some(&json!({ "altitude": [0.5, 1000] }))).altitude, (0, MAX_ALTITUDE));
        // one field of the wrong type leaves the others as stored
        let g = merge_filters(Some(&json!({ "text": null, "types": ["A3"] })));
        assert_eq!((g.text.as_str(), g.types), ("", vec!["A3".to_owned()]));
    }

    #[test]
    fn filters_are_written_with_the_stored_names() {
        let v = serde_json::to_value(Filters::default()).unwrap();
        assert_eq!(
            v,
            json!({
                "categories": [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
                "altitude": [0, 50000],
                "airborne": true,
                "onGround": true,
                "airlines": [],
                "types": [],
                "airports": [],
                "text": ""
            })
        );
        assert_eq!(merge_filters(Some(&v)), Filters::default());
    }
}
