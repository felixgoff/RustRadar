//! Port of `aircraft-labels.ts` (doc 05 §11.2): what the map writes next to each aircraft when
//! aircraft labels are on, and which label wins where two would overlap.
//!
//! `Math.round` is [`js_round`], `Math.min(1, Math.max(0, x))` is [`clamp01`], and
//! `toLocaleString()` is [`group_thousands`] (English formatting everywhere, plan §7).

use crate::model::LiveFlight;
use crate::num::{clamp01, js_round, js_string};
use crate::text::group_thousands;

/// `String.prototype.trim`: JavaScript's white space and line terminators, which include U+FEFF
/// and leave out U+0085 (Rust's `str::trim` does the opposite for both).
fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}')
}

/// The aircraft's name on the map: its callsign, else its flight number, else its registration.
pub fn aircraft_label(flight: &LiveFlight) -> String {
    let callsign = js_trim(&flight.callsign);
    let number = js_trim(&flight.flight);
    let name = if !callsign.is_empty() {
        callsign
    } else if !number.is_empty() {
        number
    } else {
        js_trim(&flight.reg)
    };
    name.to_owned()
}

/// Aviation's own notation: flight levels above the (US) transition altitude, feet below it.
pub fn altitude_label(altitude_ft: f64, on_ground: bool) -> String {
    if on_ground {
        return "GND".to_owned();
    }
    // Math.max(0, Math.round(altitudeFt)): NaN stays NaN, -0 becomes 0
    let rounded = js_round(altitude_ft);
    let ft = if rounded.is_nan() || rounded > 0.0 { rounded } else { 0.0 };
    if ft >= 18_000.0 {
        format!("FL{:0>3}", js_string(js_round(ft / 100.0)))
    } else {
        format!("{} ft", group_thousands(ft))
    }
}

/// The size (metres) at which an aircraft counts as large as they come, for [`label_priority`].
const LARGEST_M: f64 = 80.0;
/// Above this, altitude no longer makes a label less important.
const HIGHEST_FT: f64 = 45_000.0;

/// Which label wins where two would overlap, in the collision filter's range (-1000 to 1000,
/// higher wins): larger aircraft first, then lower ones, which are the ones near airports and
/// about to land or just off. The selected aircraft takes [`SELECTED_LABEL_PRIORITY`], above all
/// of these.
pub fn label_priority(size_m: f64, altitude_ft: f64, on_ground: bool) -> f64 {
    let size = clamp01(size_m / LARGEST_M);
    let low = if on_ground { 1.0 } else { 1.0 - clamp01(altitude_ft / HIGHEST_FT) };
    js_round(600.0 * size + 300.0 * low) - 450.0
}

pub const SELECTED_LABEL_PRIORITY: f64 = 1000.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn named(callsign: &str, flight: &str, reg: &str) -> LiveFlight {
        LiveFlight { callsign: callsign.to_owned(), flight: flight.to_owned(), reg: reg.to_owned(), ..Default::default() }
    }

    // aircraftLabel

    #[test]
    fn prefers_the_callsign_then_the_flight_number_then_the_registration() {
        assert_eq!(aircraft_label(&named("DLH4AB", "LH400", "D-AIXA")), "DLH4AB");
        assert_eq!(aircraft_label(&named("", "LH400", "D-AIXA")), "LH400");
        assert_eq!(aircraft_label(&named(" ", "", "D-AIXA")), "D-AIXA");
    }

    #[test]
    fn is_empty_when_there_is_nothing_to_name_it_by() {
        assert_eq!(aircraft_label(&named("", "", "")), "");
    }

    // altitudeLabel

    #[test]
    fn flight_levels_from_18_000_ft() {
        assert_eq!(altitude_label(37_000.0, false), "FL370");
        assert_eq!(altitude_label(18_000.0, false), "FL180");
        assert_eq!(altitude_label(37_049.0, false), "FL370");
    }

    #[test]
    fn feet_below_that_with_thousands_separators() {
        // `(4_500).toLocaleString() + " ft"` in an English locale
        assert_eq!(altitude_label(4_500.0, false), "4,500 ft");
        // toMatch(/^4\D?500 ft$/): at most one character, not a digit, between the 4 and the 500
        let label = altitude_label(4_500.0, false);
        let between = label.strip_prefix('4').and_then(|rest| rest.strip_suffix("500 ft")).expect(&label);
        assert!(between.chars().count() <= 1 && !between.chars().any(|c| c.is_ascii_digit()), "{label}");
        assert_eq!(altitude_label(-20.0, false), "0 ft");
    }

    #[test]
    fn gnd_on_the_ground() {
        assert_eq!(altitude_label(0.0, true), "GND");
    }

    // labelPriority

    #[test]
    fn larger_aircraft_win_over_smaller_ones_at_the_same_height() {
        assert!(label_priority(73.0, 35_000.0, false) > label_priority(38.0, 35_000.0, false));
    }

    #[test]
    fn lower_aircraft_win_over_higher_ones_of_the_same_size() {
        assert!(label_priority(38.0, 3_000.0, false) > label_priority(38.0, 38_000.0, false));
    }

    #[test]
    fn stays_in_the_collision_filters_range_under_the_selected_aircraft() {
        for p in [label_priority(0.0, 60_000.0, false), label_priority(500.0, 0.0, true)] {
            assert!(p >= -1000.0);
            assert!(p < SELECTED_LABEL_PRIORITY);
        }
    }

    // Beyond the TypeScript tests: the JavaScript built-ins' edges.

    #[test]
    fn rounding_and_trimming_follow_javascript() {
        // Math.round(-0.4) is -0, which Math.max(0, …) turns into 0 (Intl would print "-0")
        assert_eq!(altitude_label(-0.4, false), "0 ft");
        assert_eq!(altitude_label(17_999.5, false), "FL180");
        assert_eq!(altitude_label(17_999.4, false), "17,999 ft");
        assert_eq!(altitude_label(18_049.0, false), "FL180");
        assert_eq!(altitude_label(18_050.0, false), "FL181");
        assert_eq!(altitude_label(f64::NAN, false), "NaN ft");
        // the extremes of the range, exactly
        assert_eq!(label_priority(0.0, 60_000.0, false), -450.0);
        assert_eq!(label_priority(500.0, 0.0, true), 450.0);
        // 600·0.5 + 300·(1 − 0.5) = 450
        assert_eq!(label_priority(40.0, 22_500.0, false), 0.0);
        // a half rounds up: 600·(5 / 80) = 37.5 → 38
        assert_eq!(label_priority(5.0, 45_000.0, false), -412.0);
        assert_eq!(aircraft_label(&named("\u{feff}\tSAS1 \n", "", "")), "SAS1");
        assert_eq!(aircraft_label(&named("\u{85}", "", "SE-ROA")), "\u{85}");
    }
}
