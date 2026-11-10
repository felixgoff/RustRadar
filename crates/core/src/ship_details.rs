//! Port of `ship-details.ts` (S1.13, doc 05 §12.3): how a ship's card reads, the age of its last
//! report and its numbers in words.
//!
//! The card's values are the model's `Option`s (`ShipDetails::length_m`, …). Where the
//! TypeScript tested a number for truthiness, `None`, 0 and NaN all count as "not known".

use std::sync::LazyLock;

use regex::Regex;

use crate::freshness::{Freshness, age_label};
use crate::model::NavStatus;
use crate::num::{js_round, to_fixed};
use crate::text::group_thousands;
use crate::trains::js_max;

/// AIS reports come every few seconds to a few minutes; past this (seconds) a position is worth
/// flagging as old.
pub const SHIP_STALE_S: i64 = 15 * 60;

/// `Position 2 min ago`, or past [`SHIP_STALE_S`] `Position 47 min old`, in the warning tone.
/// `position_unix` is in seconds (`ShipDetails::position_unix`), `now_ms` in milliseconds.
pub fn position_age(position_unix: i64, now_ms: i64) -> Freshness {
    let age = js_max(0.0, now_ms as f64 / 1000.0 - position_unix as f64);
    if age > SHIP_STALE_S as f64 {
        Freshness { stale: true, label: format!("Position {} old", age_label(age)) }
    } else {
        Freshness { stale: false, label: format!("Position {} ago", age_label(age)) }
    }
}

/// The tone a navigational status is shown in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NavTone {
    Neutral,
    /// Something is wrong: shown in the warning tone, always with its words.
    Warn,
}

impl NavTone {
    /// The TypeScript's name for it (a badge class).
    pub fn as_str(self) -> &'static str {
        match self {
            NavTone::Neutral => "neutral",
            NavTone::Warn => "warn",
        }
    }
}

/// Statuses that say something is wrong: not under command, aground, AIS-SART active.
const ALARMING: [u8; 3] = [2, 6, 14];

pub fn nav_tone(status: &NavStatus) -> NavTone {
    if ALARMING.contains(&status.code) { NavTone::Warn } else { NavTone::Neutral }
}

/// A number JavaScript would find truthy: known, not 0 and not NaN.
fn truthy(v: Option<f64>) -> Option<f64> {
    v.filter(|v| *v != 0.0 && !v.is_nan())
}

/// `366 × 48 m`, `366 m` with no beam, `48 m beam` with no length, or none.
pub fn dimensions(length: Option<f64>, beam: Option<f64>) -> Option<String> {
    let m = |v: f64| group_thousands(js_round(v));
    match (truthy(length), truthy(beam)) {
        (Some(length), Some(beam)) => Some(format!("{} × {} m", m(length), m(beam))),
        (Some(length), None) => Some(format!("{} m", m(length))),
        (None, beam) => beam.map(|beam| format!("{} m beam", m(beam))),
    }
}

/// `9.3 / 15.5 m`, `9.3 m` or `max 15.5 m`, or none.
pub fn draught(current: Option<f64>, max: Option<f64>) -> Option<String> {
    let m = |v: f64| to_fixed(v, 1);
    match (truthy(current), truthy(max)) {
        (Some(current), Some(max)) => Some(format!("{} / {} m", m(current), m(max))),
        (Some(current), None) => Some(format!("{} m", m(current))),
        (None, max) => max.map(|max| format!("max {} m", m(max))),
    }
}

/// `51.9213° N, 4.4812° E`.
pub fn lat_lon(lat: f64, lon: f64) -> String {
    let f = |v: f64, pos: &str, neg: &str| format!("{}° {}", to_fixed(v.abs(), 4), if v < 0.0 { neg } else { pos });
    format!("{}, {}", f(lat, "N", "S"), f(lon, "E", "W"))
}

/// `/^[A-Z]{2} ?[A-Z2-9]{3}$/` (without the `m` flag, `$` is the end of the text in both).
static LOCODE: LazyLock<Regex> = LazyLock::new(|| Regex::new("^[A-Z]{2} ?[A-Z2-9]{3}$").expect("locode pattern"));

/// A UN/LOCODE (`UKSOU`): two letters of country, three of place. Anything else is the crew's
/// free text.
pub fn is_locode(destination: &str) -> bool {
    LOCODE.is_match(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_791_837_457_000;

    fn status(code: u8, label: &str) -> NavStatus {
        NavStatus { code, label: label.into() }
    }

    fn fresh(stale: bool, label: &str) -> Freshness {
        Freshness { stale, label: label.into() }
    }

    // --- positionAge

    #[test]
    fn recent_reports_read_as_an_age() {
        assert_eq!(position_age(1_791_837_457 - 120, NOW), fresh(false, "Position 2 min ago"));
        assert_eq!(position_age(1_791_837_457 + 5, NOW), fresh(false, "Position 0 s ago"));
    }

    #[test]
    fn past_a_quarter_of_an_hour_old() {
        assert!(!position_age(1_791_837_457 - 15 * 60, NOW).stale);
        assert_eq!(position_age(1_791_837_457 - 47 * 60, NOW), fresh(true, "Position 47 min old"));
    }

    // ---

    #[test]
    fn dimensions_and_draught_leave_out_what_isnt_known() {
        assert_eq!(dimensions(Some(366.0), Some(48.0)).as_deref(), Some("366 × 48 m"));
        assert_eq!(dimensions(Some(249.0), None).as_deref(), Some("249 m"));
        assert_eq!(dimensions(None, None), None);
        assert_eq!(draught(Some(9.3), Some(15.5)).as_deref(), Some("9.3 / 15.5 m"));
        assert_eq!(draught(None, Some(15.0)).as_deref(), Some("max 15.0 m"));
        assert_eq!(draught(None, None), None);
    }

    #[test]
    fn positions_statuses_and_destinations() {
        assert_eq!(lat_lon(51.92134, -4.4812), "51.9213° N, 4.4812° W");
        assert_eq!(nav_tone(&status(6, "Aground")), NavTone::Warn);
        assert_eq!(nav_tone(&status(5, "Moored")), NavTone::Neutral);
        assert!(is_locode("UKSOU"));
        assert!(!is_locode("CAPTAIN OF"));
    }

    #[test]
    fn beam_alone_rounding_thousands_and_falsy_zeros() {
        // new: the branches the TypeScript tests leave out, with the TypeScript's answers
        assert_eq!(dimensions(None, Some(48.4)).as_deref(), Some("48 m beam"));
        assert_eq!(dimensions(Some(1240.5), Some(0.0)).as_deref(), Some("1,241 m"));
        assert_eq!(dimensions(Some(0.0), Some(f64::NAN)), None);
        assert_eq!(draught(Some(9.25), None).as_deref(), Some("9.3 m")); // toFixed's tie (9.25 is exact)
        assert_eq!(draught(Some(0.0), Some(0.0)), None);
        assert_eq!(nav_tone(&status(14, "AIS-SART active")).as_str(), "warn");
        assert_eq!(nav_tone(&status(2, "Not under command")).as_str(), "warn");
        assert_eq!(NavTone::Neutral.as_str(), "neutral");
    }

    #[test]
    fn south_west_signs_and_locode_shapes() {
        // new
        assert_eq!(lat_lon(-33.85, 151.2093), "33.8500° S, 151.2093° E");
        assert_eq!(lat_lon(-0.0, 0.0), "0.0000° N, 0.0000° E");
        assert!(is_locode("US NYC"));
        assert!(is_locode("SE2AB"));
        assert!(!is_locode("SE1AB")); // place codes use 2-9, not 0 or 1
        assert!(!is_locode("uksou"));
        assert!(!is_locode("UKSOU\n"));
        assert!(!is_locode("GB  SOU"));
        // exactly at the threshold the position still counts as fresh; a second past, old
        assert_eq!(position_age(1_791_837_457 - 15 * 60 - 1, NOW), fresh(true, "Position 15 min old"));
    }
}
