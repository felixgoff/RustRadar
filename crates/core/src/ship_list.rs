//! Port of `ship-list.ts` (S1.13, doc 05 §12.2): which loaded ships match the search and
//! filters, and in what order. Pure, so the map and the list agree.
//!
//! Names fold with `train_list`'s [`fold`] and sort with [`natural_cmp`] (the stand-in for
//! `Intl.Collator(undefined, { numeric: true, sensitivity: "base" })`), as the trains list does.

use std::cmp::Ordering;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::geo::cos_angle;
use crate::model::Ship;
use crate::ships::{SHIP_COLOR_KEYS, ship_kind_key, ship_kind_label};
use crate::text::natural_cmp;
use crate::train_list::fold;
use crate::trains::{LatLon, is_js_space};

/// The order of the ships list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShipSort {
    #[default]
    Size,
    Name,
    Distance,
}

impl ShipSort {
    /// The name it is stored by.
    pub fn as_str(self) -> &'static str {
        match self {
            ShipSort::Size => "size",
            ShipSort::Name => "name",
            ShipSort::Distance => "distance",
        }
    }
}

/// Under way is a ship with a heading; moored covers those without one too (AIS gives no
/// heading at rest).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShipMotion {
    #[default]
    All,
    Moving,
    Moored,
}

impl ShipMotion {
    /// The name it is stored by.
    pub fn as_str(self) -> &'static str {
        match self {
            ShipMotion::All => "all",
            ShipMotion::Moving => "moving",
            ShipMotion::Moored => "moored",
        }
    }
}

/// A size class, as `Ship::size` holds it: 0 small, 1 medium, 2 large.
pub type ShipSize = u8;

/// VesselFinder's size classes, as the list names them: `(size, label)`.
pub const SHIP_SIZES: [(ShipSize, &str); 3] = [(0, "Small"), (1, "Medium"), (2, "Large")];

/// Shorter type names for chips and list rows; the ship's panel keeps VesselFinder's full
/// wording.
pub fn ship_kind_short(kind: u8) -> &'static str {
    match ship_kind_key(kind) {
        2 => "Tug or special",
        7 => "Rescue or law",
        8 => "Pleasure or sailing",
        _ => ship_kind_label(kind),
    }
}

/// What the coverage note says: the feed thins ships by zoom.
pub const SHIP_COVERAGE: &str = "Ships in view from VesselFinder; zoom in to see more.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShipFilters {
    /// Type groups ([`ship_kind_key`]) left out; empty shows them all.
    pub hidden_kinds: Vec<u8>,
    pub motion: ShipMotion,
    /// Size classes left out.
    pub hidden_sizes: Vec<ShipSize>,
    /// Leave out ships whose position is old.
    pub hide_old: bool,
    pub sort: ShipSort,
}

pub const DEFAULT_SHIP_FILTERS: ShipFilters = ShipFilters {
    hidden_kinds: Vec::new(),
    motion: ShipMotion::All,
    hidden_sizes: Vec::new(),
    hide_old: false,
    sort: ShipSort::Size,
};

impl Default for ShipFilters {
    fn default() -> Self {
        DEFAULT_SHIP_FILTERS
    }
}

const SORTS: [ShipSort; 3] = [ShipSort::Size, ShipSort::Name, ShipSort::Distance];
const MOTIONS: [ShipMotion; 3] = [ShipMotion::All, ShipMotion::Moving, ShipMotion::Moored];

/// The numbers of a stored array that are one of `keys` (`array.filter((k) => keys.includes(k))`:
/// numbers only, `5.0` as `5`, duplicates kept).
fn numbers_in(values: &[Value], keys: &[u8]) -> Vec<u8> {
    values
        .iter()
        .filter_map(Value::as_f64)
        .filter_map(|v| keys.iter().copied().find(|k| f64::from(*k) == v))
        .collect()
}

/// Stored ship filters over the defaults, field by field; anything unreadable falls back.
/// `saved` is the stored value (`None` where nothing is stored).
pub fn merge_ship_filters(saved: Option<&Value>) -> ShipFilters {
    let mut f = DEFAULT_SHIP_FILTERS;
    let Some(s) = saved.and_then(Value::as_object) else {
        return f;
    };
    if let Some(kinds) = s.get("hiddenKinds").and_then(Value::as_array) {
        f.hidden_kinds = numbers_in(kinds, &SHIP_COLOR_KEYS);
    }
    if let Some(sizes) = s.get("hiddenSizes").and_then(Value::as_array) {
        f.hidden_sizes = numbers_in(sizes, &[0, 1, 2]);
    }
    if let Some(motion) = s.get("motion").and_then(Value::as_str).and_then(|n| MOTIONS.into_iter().find(|m| m.as_str() == n)) {
        f.motion = motion;
    }
    if let Some(hide_old) = s.get("hideOld").and_then(Value::as_bool) {
        f.hide_old = hide_old;
    }
    if let Some(sort) = s.get("sort").and_then(Value::as_str).and_then(|n| SORTS.into_iter().find(|s| s.as_str() == n)) {
        f.sort = sort;
    }
    f
}

/// How many filter groups narrow the list, for the badge (the sort is not a filter).
pub fn ship_filter_count(f: &ShipFilters) -> usize {
    [!f.hidden_kinds.is_empty(), f.motion != ShipMotion::All, !f.hidden_sizes.is_empty(), f.hide_old]
        .into_iter()
        .filter(|on| *on)
        .count()
}

/// A name worth showing: VesselFinder sends the MMSI as the name when the ship never broadcast
/// one, and then this is "".
pub fn ship_name(s: &Ship) -> &str {
    if !s.name.is_empty() && s.name != s.mmsi.to_string() { &s.name } else { "" }
}

/// Whether a ship matches the search: its name (every word), or its MMSI from the start.
pub fn ship_matches(ship: &Ship, query: &str) -> bool {
    let q = query.trim_matches(is_js_space);
    if q.is_empty() {
        return true;
    }
    // `/^\d+$/`: ASCII digits
    if q.bytes().all(|b| b.is_ascii_digit()) {
        return ship.mmsi.to_string().starts_with(q) || fold(&ship.name).contains(q);
    }
    let hay = fold(ship_name(ship));
    // `split(/\s+/)`: the empty words a run of spaces leaves here are in every name
    fold(q).split(is_js_space).all(|w| hay.contains(w))
}

/// A predicate for the filters alone: what the map draws.
pub fn ship_filter(f: &ShipFilters) -> impl Fn(&Ship) -> bool + use<> {
    let kinds: HashSet<u8> = f.hidden_kinds.iter().copied().collect();
    let sizes: HashSet<ShipSize> = f.hidden_sizes.iter().copied().collect();
    let (hide_old, motion) = (f.hide_old, f.motion);
    move |s: &Ship| {
        if kinds.contains(&ship_kind_key(s.kind)) || sizes.contains(&s.size) {
            return false;
        }
        if hide_old && s.old {
            return false;
        }
        if motion == ShipMotion::Moving && s.heading.is_none() {
            return false;
        }
        if motion == ShipMotion::Moored && s.heading.is_some() {
            return false;
        }
        true
    }
}

/// Length overall in metres from the AIS dimensions (bow plus stern), when known.
pub fn ship_length(s: &Ship) -> Option<u32> {
    s.dims.map(|d| u32::from(d[0]) + u32::from(d[1]))
}

/// The list in the chosen order (references into it; the list is left as it was). Size: the
/// size class first, then the measured length within it (known lengths before unknown), largest
/// first. Unnamed ships sort after named ones by name; ties fall back to the MMSI.
pub fn sort_ships<'a>(list: impl IntoIterator<Item = &'a Ship>, sort: ShipSort, centre: LatLon) -> Vec<&'a Ship> {
    let name = |a: &Ship, b: &Ship| {
        let (na, nb) = (ship_name(a), ship_name(b));
        // `Number(!na) - Number(!nb) || byName.compare(na, nb) || a.mmsi - b.mmsi`
        na.is_empty().cmp(&nb.is_empty()).then_with(|| natural_cmp(na, nb)).then(a.mmsi.cmp(&b.mmsi))
    };
    if sort == ShipSort::Distance {
        // nearest first: the larger the cosine of the angle, the closer
        let mut keyed: Vec<(&Ship, f64)> =
            list.into_iter().map(|s| (s, cos_angle(s.lon, s.lat, centre.lon, centre.lat))).collect();
        keyed.sort_by(|a, b| (b.1 - a.1).partial_cmp(&0.0).unwrap_or(Ordering::Equal).then_with(|| name(a.0, b.0)));
        return keyed.into_iter().map(|(s, _)| s).collect();
    }
    let mut out: Vec<&Ship> = list.into_iter().collect();
    if sort == ShipSort::Name {
        out.sort_by(|a, b| name(a, b));
        return out;
    }
    out.sort_by(|a, b| {
        if a.size != b.size {
            return b.size.cmp(&a.size);
        }
        match (ship_length(a), ship_length(b)) {
            (Some(la), Some(lb)) => lb.cmp(&la).then_with(|| name(a, b)),
            // `Number(la === null) - Number(lb === null) || name(a, b)`
            (la, lb) => la.is_none().cmp(&lb.is_none()).then_with(|| name(a, b)),
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `ship-list.test.ts`'s `ship(over)`: the Viking Grace at Helsinki, changed by `over`.
    fn ship(over: impl FnOnce(&mut Ship)) -> Ship {
        let mut s = Ship {
            mmsi: 230_000_001,
            lat: 60.1,
            lon: 24.9,
            name: "VIKING GRACE".into(),
            kind: 3,
            size: 1,
            heading: Some(90.0),
            old: false,
            dims: None,
            frames: vec![],
        };
        over(&mut s);
        s
    }

    const CENTRE: LatLon = LatLon { lat: 60.1, lon: 24.9 };

    // --- shipMatches

    fn serenade() -> Ship {
        ship(|s| {
            s.name = "SILJA SERENADE".into();
            s.mmsi = 230_180_000;
        })
    }

    #[test]
    fn by_name_any_case_every_word() {
        let s = serenade();
        assert!(ship_matches(&s, "serenade"));
        assert!(ship_matches(&s, "silja seren"));
        assert!(!ship_matches(&s, "silja grace"));
    }

    #[test]
    fn by_mmsi_from_its_start() {
        let s = serenade();
        assert!(ship_matches(&s, "230180"));
        assert!(!ship_matches(&s, "180000"));
    }

    #[test]
    fn a_ship_with_no_name_of_its_own_is_found_by_mmsi_not_by_its_mmsi_as_name_in_a_word_search() {
        let unnamed = ship(|s| {
            s.name = "273456789".into();
            s.mmsi = 273_456_789;
        });
        assert_eq!(ship_name(&unnamed), "");
        assert!(ship_matches(&unnamed, "2734"));
        assert!(!ship_matches(&unnamed, "grace"));
    }

    #[test]
    fn an_empty_search_matches_everything() {
        assert!(ship_matches(&serenade(), ""));
    }

    // --- shipFilter

    fn filter_list() -> Vec<Ship> {
        let at = |mmsi: u32, name: &str, kind: u8, size: u8, heading: Option<f32>, old: bool| {
            ship(|s| {
                (s.mmsi, s.name, s.kind, s.size, s.heading, s.old) = (mmsi, name.into(), kind, size, heading, old);
            })
        };
        vec![
            at(1, "A", 4, 2, Some(10.0), false),
            at(2, "B", 6, 1, None, false),
            at(3, "C", 8, 0, Some(200.0), true),
            // an unlisted type goes with the unknown
            at(4, "D", 9, 1, None, false),
        ]
    }

    fn names(f: &ShipFilters) -> Vec<String> {
        let keep = ship_filter(f);
        filter_list().into_iter().filter(|s| keep(s)).map(|s| s.name).collect()
    }

    #[test]
    fn the_defaults_show_every_ship() {
        assert_eq!(names(&DEFAULT_SHIP_FILTERS), ["A", "B", "C", "D"]);
        assert_eq!(ship_filter_count(&DEFAULT_SHIP_FILTERS), 0);
    }

    #[test]
    fn hidden_types_the_unknown_group_taking_unlisted_types() {
        assert_eq!(names(&ShipFilters { hidden_kinds: vec![6, 0], ..DEFAULT_SHIP_FILTERS }), ["A", "C"]);
    }

    #[test]
    fn moving_has_a_heading_moored_is_the_rest() {
        assert_eq!(names(&ShipFilters { motion: ShipMotion::Moving, ..DEFAULT_SHIP_FILTERS }), ["A", "C"]);
        assert_eq!(names(&ShipFilters { motion: ShipMotion::Moored, ..DEFAULT_SHIP_FILTERS }), ["B", "D"]);
    }

    #[test]
    fn sizes_and_old_positions() {
        assert_eq!(names(&ShipFilters { hidden_sizes: vec![0, 1], ..DEFAULT_SHIP_FILTERS }), ["A"]);
        assert_eq!(names(&ShipFilters { hide_old: true, ..DEFAULT_SHIP_FILTERS }), ["A", "B", "D"]);
    }

    #[test]
    fn each_group_counts_once_for_the_badge() {
        let f = ShipFilters { hidden_kinds: vec![2, 3], motion: ShipMotion::Moored, hide_old: true, ..DEFAULT_SHIP_FILTERS };
        assert_eq!(ship_filter_count(&f), 3);
    }

    // --- sortShips

    fn sort_list() -> Vec<Ship> {
        let at = |mmsi: u32, name: &str, size: u8, dims: Option<[u16; 4]>, lat: f64| {
            ship(|s| {
                (s.mmsi, s.name, s.size, s.dims, s.lat) = (mmsi, name.into(), size, dims, lat);
            })
        };
        vec![
            at(1, "BRAVO", 1, Some([80, 20, 8, 8]), 60.5),
            at(2, "ALPHA", 2, None, 59.0),
            at(3, "CHARLIE", 2, Some([200, 94, 16, 16]), 60.11),
            at(4, "4", 0, None, 60.2),
            at(5, "DELTA", 1, None, 61.0),
        ]
    }

    fn order(sort: ShipSort) -> Vec<u32> {
        sort_ships(&sort_list(), sort, CENTRE).into_iter().map(|s| s.mmsi).collect()
    }

    #[test]
    fn by_size_class_first_then_measured_length_known_before_unknown() {
        assert_eq!(order(ShipSort::Size), [3, 2, 1, 5, 4]);
        assert_eq!(ship_length(&sort_list()[2]), Some(294));
    }

    #[test]
    fn by_name_unnamed_ships_last() {
        assert_eq!(order(ShipSort::Name), [2, 1, 3, 5, 4]);
    }

    #[test]
    fn by_distance_from_the_maps_centre() {
        assert_eq!(order(ShipSort::Distance), [3, 4, 1, 5, 2]);
    }

    // --- mergeShipFilters

    #[test]
    fn nothing_stored_gives_the_defaults() {
        assert_eq!(merge_ship_filters(Some(&Value::Null)), DEFAULT_SHIP_FILTERS);
        assert_eq!(merge_ship_filters(None), DEFAULT_SHIP_FILTERS);
    }

    #[test]
    fn stored_choices_are_kept_unknown_values_dropped() {
        let f = merge_ship_filters(Some(&json!({
            "hiddenKinds": [5, 42], "hiddenSizes": [0, 7], "motion": "moored", "hideOld": true, "sort": "name",
        })));
        assert_eq!(
            f,
            ShipFilters {
                hidden_kinds: vec![5],
                hidden_sizes: vec![0],
                motion: ShipMotion::Moored,
                hide_old: true,
                sort: ShipSort::Name,
            }
        );
        assert_eq!(merge_ship_filters(Some(&json!({ "motion": "drifting", "sort": "speed" }))), DEFAULT_SHIP_FILTERS);
    }

    // --- new tests

    #[test]
    fn stored_values_are_read_as_javascript_reads_them() {
        // numbers only (`includes` is strict): "5" and true are dropped, 5.0 is 5; duplicates
        // stay; a boolean that isn't one, or a stored array, falls back
        let f = merge_ship_filters(Some(&json!({
            "hiddenKinds": ["5", 5.0, 0, 0, 1, true, null], "hiddenSizes": [2.0, 1.5, -1], "hideOld": "yes",
        })));
        assert_eq!((f.hidden_kinds, f.hidden_sizes, f.hide_old), (vec![5, 0, 0], vec![2], false));
        assert_eq!(merge_ship_filters(Some(&json!([{ "sort": "name" }]))), DEFAULT_SHIP_FILTERS);
        assert_eq!(merge_ship_filters(Some(&json!("size"))), DEFAULT_SHIP_FILTERS);
        // the defaults are not shared
        let mut g = merge_ship_filters(None);
        g.hidden_kinds.push(3);
        assert!(DEFAULT_SHIP_FILTERS.hidden_kinds.is_empty());
    }

    #[test]
    fn filters_are_stored_under_the_typescripts_names() {
        let f = ShipFilters { hidden_kinds: vec![4], motion: ShipMotion::Moving, ..DEFAULT_SHIP_FILTERS };
        let stored = serde_json::to_value(&f).unwrap();
        assert_eq!(
            stored,
            json!({ "hiddenKinds": [4], "motion": "moving", "hiddenSizes": [], "hideOld": false, "sort": "size" })
        );
        assert_eq!(merge_ship_filters(Some(&stored)), f);
    }

    #[test]
    fn short_type_names_sizes_and_the_coverage_note() {
        assert_eq!(ship_kind_short(2), "Tug or special");
        assert_eq!(ship_kind_short(7), "Rescue or law");
        assert_eq!(ship_kind_short(8), "Pleasure or sailing");
        assert_eq!(ship_kind_short(4), "Cargo");
        assert_eq!(ship_kind_short(0), "Unknown type");
        assert_eq!(ship_kind_short(13), "Other");
        assert_eq!(SHIP_SIZES.map(|(_, label)| label), ["Small", "Medium", "Large"]);
        assert_eq!(SHIP_COVERAGE, "Ships in view from VesselFinder; zoom in to see more.");
    }

    #[test]
    fn digit_searches_also_look_in_the_name_and_words_fold() {
        let tug = ship(|s| {
            s.name = "BUGSER 12 ÅLESUND".into();
            s.mmsi = 257_000_123;
        });
        assert!(ship_matches(&tug, "12")); // in the name, not the MMSI's start
        assert!(ship_matches(&tug, " 2570 "));
        assert!(ship_matches(&tug, "alesund  bugser"));
        assert!(ship_matches(&tug, "Ålesund"));
        assert!(!ship_matches(&tug, "12 oslo"));
        assert_eq!(ship_name(&ship(|s| s.name = String::new())), "");
    }

    #[test]
    fn sorts_leave_the_list_as_it_was_and_tie_on_the_mmsi() {
        let list = [
            ship(|s| (s.mmsi, s.name) = (9, "Ferry 10".into())),
            ship(|s| (s.mmsi, s.name) = (7, "FERRY 2".into())),
            ship(|s| (s.mmsi, s.name) = (8, "ferry 2".into())),
            ship(|s| (s.mmsi, s.name) = (6, "6".into())),
            ship(|s| (s.mmsi, s.name) = (5, String::new())),
        ];
        let order = |sort| sort_ships(&list, sort, CENTRE).into_iter().map(|s| s.mmsi).collect::<Vec<_>>();
        // numbers in natural order, case ignored, then the MMSI; unnamed last by MMSI
        assert_eq!(order(ShipSort::Name), [7, 8, 9, 5, 6]);
        // the same place and size: down to the name
        assert_eq!(order(ShipSort::Distance), [7, 8, 9, 5, 6]);
        assert_eq!(order(ShipSort::Size), [7, 8, 9, 5, 6]);
        assert_eq!(list[0].mmsi, 9);
    }
}
