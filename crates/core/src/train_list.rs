//! Port of `train-list.ts` (S1.14, doc 05 §12.5): which loaded trains match the search and
//! filters, and in what order. Pure, so the map and the list agree.
//!
//! `fold`, `distance_km` and `distance_label` are shared with the ship list, as in the
//! TypeScript.

use std::cmp::Ordering;
use std::collections::HashSet;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::geo::cos_angle;
use crate::model::{Train, TrainKind};
use crate::num::{js_round, js_string};
use crate::text::{group_thousands, locale_cmp, natural_cmp};
use crate::trains::{LatLon, is_js_space, js_max, js_min, train_kind_named};

/// Lower case without accents, so "helsinki" finds "Helsinki" and "oslo s" finds "Oslo S".
pub use crate::text::fold;

/// The order of the trains list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrainSort {
    #[default]
    Delay,
    Speed,
    Name,
    Distance,
}

impl TrainSort {
    /// The name it is stored by.
    pub fn as_str(self) -> &'static str {
        match self {
            TrainSort::Delay => "delay",
            TrainSort::Speed => "speed",
            TrainSort::Name => "name",
            TrainSort::Distance => "distance",
        }
    }
}

const SORTS: [TrainSort; 4] = [TrainSort::Delay, TrainSort::Speed, TrainSort::Name, TrainSort::Distance];

/// What the coverage note says: where live trains exist at all.
pub const TRAIN_COVERAGE: &str = "Live trains are shown in Sweden, from Trafikverket.";

/// A train this many minutes late or more counts as delayed (as `delay_tone` starts to warn).
pub const DELAYED_MINUTES: i32 = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainFilters {
    /// Kinds left out; empty shows every kind. Stored as left out, so a kind added later shows.
    pub hidden_kinds: Vec<TrainKind>,
    /// Operators left out, as trains name them ("" for trains whose operator isn't known); not
    /// streamed live either.
    pub hidden_operators: Vec<String>,
    pub delayed_only: bool,
    pub sort: TrainSort,
}

pub const DEFAULT_TRAIN_FILTERS: TrainFilters =
    TrainFilters { hidden_kinds: Vec::new(), hidden_operators: Vec::new(), delayed_only: false, sort: TrainSort::Delay };

impl Default for TrainFilters {
    fn default() -> Self {
        DEFAULT_TRAIN_FILTERS
    }
}

/// Stored train filters over the defaults, field by field; anything unreadable falls back.
/// `saved` is the stored value (`None` where nothing is stored).
pub fn merge_train_filters(saved: Option<&Value>) -> TrainFilters {
    let mut f = DEFAULT_TRAIN_FILTERS;
    let Some(s) = saved.and_then(Value::as_object) else {
        return f;
    };
    if let Some(kinds) = s.get("hiddenKinds").and_then(Value::as_array) {
        f.hidden_kinds = kinds.iter().filter_map(|k| k.as_str().and_then(train_kind_named)).collect();
    }
    if let Some(operators) = s.get("hiddenOperators").and_then(Value::as_array) {
        f.hidden_operators = operators.iter().filter_map(|o| o.as_str().map(str::to_owned)).collect();
    }
    if let Some(delayed_only) = s.get("delayedOnly").and_then(Value::as_bool) {
        f.delayed_only = delayed_only;
    }
    if let Some(sort) = s.get("sort").and_then(Value::as_str).and_then(|n| SORTS.into_iter().find(|s| s.as_str() == n)) {
        f.sort = sort;
    }
    f
}

/// How many filter groups narrow the list, for the badge (the sort is not a filter).
pub fn train_filter_count(f: &TrainFilters) -> usize {
    [!f.hidden_kinds.is_empty(), !f.hidden_operators.is_empty(), f.delayed_only].into_iter().filter(|on| *on).count()
}

/// Whether a train matches the search: every word in its name (and number), destination or
/// operator.
pub fn train_matches(train: &Train, query: &str) -> bool {
    let query = fold(query);
    let words: Vec<&str> = query.split(is_js_space).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return true;
    }
    let hay = fold(&[train.name.as_str(), train.destination.as_deref().unwrap_or(""), train.operator.as_deref().unwrap_or("")].join(" "));
    words.iter().all(|w| hay.contains(w))
}

/// A finite delay of [`DELAYED_MINUTES`] or more.
pub fn is_delayed(train: &Train) -> bool {
    train.delay_minutes.is_some_and(|d| d >= DELAYED_MINUTES)
}

/// A predicate for the filters alone: what the map draws.
pub fn train_filter(f: &TrainFilters) -> impl Fn(&Train) -> bool + use<> {
    let kinds: HashSet<TrainKind> = f.hidden_kinds.iter().copied().collect();
    let operators: HashSet<String> = f.hidden_operators.iter().cloned().collect();
    let delayed_only = f.delayed_only;
    move |t: &Train| {
        !kinds.contains(&t.kind)
            && !operators.contains(t.operator.as_deref().unwrap_or(""))
            && (!delayed_only || is_delayed(t))
    }
}

/// Unknown values sort last whichever way the rest go.
fn nulls_last(a: Option<f64>, b: Option<f64>, descending: bool) -> Ordering {
    let ka = !a.is_some_and(f64::is_finite);
    let kb = !b.is_some_and(f64::is_finite);
    if ka || kb {
        // `Number(ka) - Number(kb)`
        return ka.cmp(&kb);
    }
    let (a, b) = (a.unwrap_or_default(), b.unwrap_or_default());
    let d = if descending { b - a } else { a - b };
    d.partial_cmp(&0.0).unwrap_or(Ordering::Equal)
}

/// The list in the chosen order (references into it; the list is left as it was); ties fall
/// back to the name, then the id.
pub fn sort_trains<'a>(list: impl IntoIterator<Item = &'a Train>, sort: TrainSort, centre: LatLon) -> Vec<&'a Train> {
    let name = |a: &Train, b: &Train| natural_cmp(&a.name, &b.name).then_with(|| locale_cmp(&a.id, &b.id));
    if sort == TrainSort::Distance {
        // nearest first: the larger the cosine of the angle, the closer
        let mut keyed: Vec<(&Train, f64)> =
            list.into_iter().map(|t| (t, cos_angle(t.lon, t.lat, centre.lon, centre.lat))).collect();
        keyed.sort_by(|a, b| (b.1 - a.1).partial_cmp(&0.0).unwrap_or(Ordering::Equal).then_with(|| name(a.0, b.0)));
        return keyed.into_iter().map(|(t, _)| t).collect();
    }
    // speeds are compared, never shown: `f64::from` keeps the order of the doubles JavaScript read
    let compare = |a: &Train, b: &Train| match sort {
        TrainSort::Delay => nulls_last(a.delay_minutes.map(f64::from), b.delay_minutes.map(f64::from), true),
        TrainSort::Speed => nulls_last(a.speed_kmh.map(f64::from), b.speed_kmh.map(f64::from), true),
        _ => Ordering::Equal,
    };
    let mut out: Vec<&Train> = list.into_iter().collect();
    out.sort_by(|a, b| compare(a, b).then_with(|| name(a, b)));
    out
}

/// Distance along the ground in kilometres, for a row when sorted by distance.
pub fn distance_km(lat: f64, lon: f64, centre: LatLon) -> f64 {
    let c = js_min(1.0, js_max(-1.0, cos_angle(lon, lat, centre.lon, centre.lat)));
    c.acos() * 6371.0
}

/// "850 m", "12 km", "1,240 km".
pub fn distance_label(km: f64) -> String {
    if km < 1.0 {
        return format!("{} m", js_string(js_round(km * 100.0) * 10.0));
    }
    format!("{} km", group_thousands(js_round(km)))
}

/// An operator of the trains in view and how many trains it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorCount {
    /// "" for trains whose operator isn't known.
    pub name: String,
    pub count: usize,
}

/// The operators of these trains, most trains first ("" for those whose operator isn't known,
/// last), then by name.
pub fn train_operators<'a>(trains: impl IntoIterator<Item = &'a Train>) -> Vec<OperatorCount> {
    let mut counts: IndexMap<&str, usize> = IndexMap::new();
    for t in trains {
        *counts.entry(t.operator.as_deref().unwrap_or("")).or_insert(0) += 1;
    }
    let mut out: Vec<OperatorCount> =
        counts.into_iter().map(|(name, count)| OperatorCount { name: name.to_owned(), count }).collect();
    out.sort_by(|a, b| {
        a.name.is_empty().cmp(&b.name.is_empty()).then(b.count.cmp(&a.count)).then_with(|| locale_cmp(&a.name, &b.name))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrainSource;
    use serde_json::json;

    /// A train named `name` (and with it as its id), else "IC 1" with the id "t".
    fn train(name: Option<&str>, over: impl FnOnce(&mut Train)) -> Train {
        let mut t = Train {
            id: name.unwrap_or("t").into(),
            source: TrainSource::Trafikverket,
            name: name.unwrap_or("IC 1").into(),
            kind: TrainKind::Intercity,
            lat: 60.17,
            lon: 24.94,
            heading: None,
            speed_kmh: None,
            destination: None,
            operator: None,
            delay_minutes: None,
            position_unix: 0,
            frames: vec![],
            reports: vec![],
            predicted: vec![],
            cars: None,
            tail: vec![],
            structures: vec![],
            ended_unix: None,
            shape: None,
            model: None,
        };
        over(&mut t);
        t
    }

    const HELSINKI: LatLon = LatLon { lat: 60.17, lon: 24.94 };

    // --- trainMatches

    fn bergen() -> Train {
        train(Some("F4 Oslo S-Bergen"), |t| {
            t.destination = Some("Bergen".into());
            t.operator = Some("Vy".into());
        })
    }

    #[test]
    fn an_empty_search_matches_everything() {
        assert!(train_matches(&bergen(), "  "));
    }

    #[test]
    fn by_name_or_number_destination_and_operator_any_case() {
        let t = bergen();
        assert!(train_matches(&t, "f4"));
        assert!(train_matches(&t, "bergen"));
        assert!(train_matches(&t, "VY"));
        assert!(!train_matches(&t, "tampere"));
    }

    #[test]
    fn every_word_has_to_match_in_any_field() {
        let t = bergen();
        assert!(train_matches(&t, "vy bergen"));
        assert!(!train_matches(&t, "vy tampere"));
    }

    #[test]
    fn accents_dont_matter() {
        let j = train(Some("IC 27"), |t| t.destination = Some("Jyväskylä".into()));
        assert!(train_matches(&j, "jyvaskyla"));
        assert!(train_matches(&j, "Jyväs"));
    }

    #[test]
    fn a_train_without_destination_or_operator_still_matches_by_name() {
        assert!(train_matches(&train(Some("HDM 123"), |_| {}), "hdm"));
    }

    // --- trainFilter

    fn filter_list() -> Vec<Train> {
        vec![
            train(Some("a"), |t| {
                t.kind = TrainKind::Intercity;
                t.delay_minutes = Some(0);
            }),
            train(Some("b"), |t| {
                t.kind = TrainKind::Suburban;
                t.delay_minutes = Some(12);
            }),
            train(Some("c"), |t| {
                t.kind = TrainKind::Regional;
                t.delay_minutes = Some(5);
            }),
            train(Some("d"), |t| {
                t.kind = TrainKind::Regional;
                t.delay_minutes = None;
            }),
        ]
    }

    fn names(list: &[Train], f: &TrainFilters) -> Vec<String> {
        let keep = train_filter(f);
        list.iter().filter(|t| keep(t)).map(|t| t.name.clone()).collect()
    }

    #[test]
    fn the_defaults_show_every_train() {
        assert_eq!(names(&filter_list(), &DEFAULT_TRAIN_FILTERS), ["a", "b", "c", "d"]);
        assert_eq!(train_filter_count(&DEFAULT_TRAIN_FILTERS), 0);
    }

    #[test]
    fn hidden_kinds_are_left_out() {
        let list = filter_list();
        let hidden = |hidden_kinds: Vec<TrainKind>| TrainFilters { hidden_kinds, ..DEFAULT_TRAIN_FILTERS };
        assert_eq!(names(&list, &hidden(vec![TrainKind::Suburban])), ["a", "c", "d"]);
        assert_eq!(names(&list, &hidden(vec![TrainKind::Regional, TrainKind::Intercity])), ["b"]);
    }

    #[test]
    fn delayed_only_keeps_trains_5_min_or_more_late_not_unknown_delays() {
        let f = TrainFilters { delayed_only: true, ..DEFAULT_TRAIN_FILTERS };
        assert_eq!(names(&filter_list(), &f), ["b", "c"]);
        assert_eq!(train_filter_count(&f), 1);
        assert!(!is_delayed(&train(None, |t| t.delay_minutes = Some(4))));
        assert!(!is_delayed(&train(None, |t| t.delay_minutes = Some(-3))));
    }

    #[test]
    fn hidden_operators_are_left_out_those_not_known_as() {
        // "hidden operators are left out, those not known as \"\""
        let ops = [
            train(Some("sj"), |t| t.operator = Some("SJ".into())),
            train(Some("sl"), |t| t.operator = Some("SL".into())),
            train(Some("none"), |t| t.operator = None),
        ];
        let shown = |hidden: &[&str]| {
            let f = TrainFilters {
                hidden_operators: hidden.iter().map(|o| o.to_string()).collect(),
                ..DEFAULT_TRAIN_FILTERS
            };
            names(&ops, &f)
        };
        assert_eq!(shown(&["SJ"]), ["sl", "none"]);
        assert_eq!(shown(&[""]), ["sj", "sl"]);
        assert_eq!(train_filter_count(&TrainFilters { hidden_operators: vec!["SJ".into()], ..DEFAULT_TRAIN_FILTERS }), 1);
    }

    #[test]
    fn operators_are_listed_most_trains_first_unknown_last() {
        let ops: Vec<Train> = [None, Some("SL"), Some("SJ"), Some("SL"), None, Some("Mälartåg"), Some("SL"), Some("SJ")]
            .into_iter()
            .map(|operator| train(None, |t| t.operator = operator.map(str::to_owned)))
            .collect();
        let count = |name: &str, count| OperatorCount { name: name.into(), count };
        assert_eq!(train_operators(&ops), [count("SL", 3), count("SJ", 2), count("Mälartåg", 1), count("", 2)]);
    }

    #[test]
    fn each_group_counts_once_for_the_badge() {
        let f = TrainFilters {
            hidden_kinds: vec![TrainKind::Suburban, TrainKind::Other],
            hidden_operators: vec!["SJ".into()],
            ..DEFAULT_TRAIN_FILTERS
        };
        assert_eq!(train_filter_count(&f), 2);
    }

    // --- sortTrains

    fn sort_list() -> Vec<Train> {
        let at = |name: &str, delay: Option<i32>, speed: Option<f32>, lat: f64, lon: f64| {
            train(Some(name), |t| {
                t.delay_minutes = delay;
                t.speed_kmh = speed;
                t.lat = lat;
                t.lon = lon;
            })
        };
        vec![
            at("IC 10", Some(3), Some(140.0), 61.5, 23.8),
            at("IC 2", None, Some(190.0), 60.2, 24.9),
            at("P 50", Some(25), None, 65.0, 25.5),
            at("H 9", Some(-2), Some(0.0), 60.5, 25.0),
        ]
    }

    fn order(sort: TrainSort) -> Vec<String> {
        sort_trains(&sort_list(), sort, HELSINKI).into_iter().map(|t| t.name.clone()).collect()
    }

    #[test]
    fn by_delay_most_late_first_early_after_on_time_unknown_last() {
        assert_eq!(order(TrainSort::Delay), ["P 50", "IC 10", "H 9", "IC 2"]);
    }

    #[test]
    fn by_speed_fastest_first_unknown_last() {
        assert_eq!(order(TrainSort::Speed), ["IC 2", "IC 10", "H 9", "P 50"]);
    }

    #[test]
    fn by_name_numbers_in_natural_order() {
        assert_eq!(order(TrainSort::Name), ["H 9", "IC 2", "IC 10", "P 50"]);
    }

    #[test]
    fn by_distance_from_the_maps_centre_nearest_first() {
        assert_eq!(order(TrainSort::Distance), ["IC 2", "H 9", "IC 10", "P 50"]);
    }

    #[test]
    fn the_list_passed_in_is_left_as_it_was() {
        let list = sort_list();
        sort_trains(&list, TrainSort::Name, HELSINKI);
        assert_eq!(list[0].name, "IC 10");
    }

    // --- distances

    #[test]
    fn helsinki_to_tampere_is_about_160_km() {
        let km = distance_km(61.4978, 23.761, HELSINKI);
        assert!(km > 155.0);
        assert!(km < 165.0);
    }

    #[test]
    fn labels() {
        assert_eq!(distance_label(0.43), "430 m");
        assert_eq!(distance_label(12.4), "12 km");
        assert_eq!(distance_label(1240.2), "1,240 km");
    }

    // --- mergeTrainFilters

    #[test]
    fn nothing_stored_gives_the_defaults_not_shared() {
        let mut f = merge_train_filters(None);
        assert_eq!(f, DEFAULT_TRAIN_FILTERS);
        f.hidden_kinds.push(TrainKind::Suburban);
        assert!(DEFAULT_TRAIN_FILTERS.hidden_kinds.is_empty());
        assert_eq!(merge_train_filters(Some(&Value::Null)), DEFAULT_TRAIN_FILTERS);
    }

    #[test]
    fn stored_choices_are_kept_unknown_kinds_and_sorts_and_the_old_regions_are_dropped() {
        let f = merge_train_filters(Some(&json!({
            "hiddenKinds": ["suburban", "hovercraft", "metro"],
            "hiddenSources": ["vbb", "mars"],
            "hiddenOperators": ["SJ", 7],
            "delayedOnly": true,
            "sort": "speed",
        })));
        assert_eq!(
            f,
            TrainFilters {
                hidden_kinds: vec![TrainKind::Suburban],
                hidden_operators: vec!["SJ".into()],
                delayed_only: true,
                sort: TrainSort::Speed,
            }
        );
        assert_eq!(merge_train_filters(Some(&json!({ "sort": "colour", "delayedOnly": "yes" }))), DEFAULT_TRAIN_FILTERS);
        // as stored: the same names on the way out
        assert_eq!(
            serde_json::to_value(&f).unwrap(),
            json!({ "hiddenKinds": ["suburban"], "hiddenOperators": ["SJ"], "delayedOnly": true, "sort": "speed" })
        );
    }

    #[test]
    fn fold_is_the_text_helper() {
        // ship_list folds through this module, as ship-list.ts imports train-list's
        assert_eq!(fold("Jyväskylä Oslo S"), "jyvaskyla oslo s");
    }
}
