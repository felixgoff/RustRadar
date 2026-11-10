//! Port of `aircraft-readings.ts` (doc 05 §11.3): what the aircraft's own transponder reports
//! (via adsb.lol), as the flight panel lists it: the few values worth a glance always, the rest
//! on request.
//!
//! The model keeps several of these values as `f32`; the TypeScript read them as the JSON's
//! shortest decimal, so they come in through [`f32_as_js`]. `toLocaleString(undefined, …)` is
//! [`group_thousands_fixed`] (English formatting everywhere, plan §7), `toFixed` is [`to_fixed`]
//! and `String(n)` is [`js_string`].

use crate::model::{AdsbAircraft, f32_as_js};
use crate::num::{js_round, js_string, to_fixed};
use crate::text::group_thousands_fixed;

/// One value of the panel (`aircraft-readings.ts` `Reading`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reading {
    pub label: &'static str,
    /// Formatted; `None` when this aircraft doesn't report it.
    pub value: Option<String>,
    /// What the value is, where the label alone doesn't say.
    pub title: Option<&'static str>,
    /// Takes a whole row of the grid.
    pub wide: bool,
    /// Words rather than a measurement: set in the UI face, not the data face.
    pub words: bool,
}

/// The panel's values (`aircraft-readings.ts` `Readings`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Readings {
    /// Always on show, in fixed places, with a dash for what isn't reported.
    pub first: Vec<Reading>,
    /// Behind "More from the aircraft": only what this aircraft reports.
    pub more: Vec<Reading>,
    /// The rest of the "more" values, which this aircraft doesn't report, for a sentence.
    pub unreported: Vec<&'static str>,
}

/// `007°`: three digits, as headings and tracks are written.
pub fn degrees(v: f64) -> String {
    format!("{:0>3}°", js_string(((js_round(v) % 360.0) + 360.0) % 360.0))
}

/// `number(v, unit, digits)`: `None` when absent or not finite, else grouped with exactly
/// `digits` decimals, then the unit.
fn number(v: Option<f64>, unit: &str, digits: usize) -> Option<String> {
    v.filter(|v| v.is_finite()).map(|v| format!("{}{unit}", group_thousands_fixed(v, digits)))
}

fn int(v: Option<i32>) -> Option<f64> {
    v.map(f64::from)
}

fn float(v: Option<f32>) -> Option<f64> {
    v.map(f32_as_js)
}

/// `MODES[mode] ?? mode.toUpperCase()`.
fn mode_name(mode: &str) -> String {
    match mode {
        "autopilot" => "Autopilot".to_owned(),
        "vnav" => "VNAV".to_owned(),
        "althold" => "Altitude hold".to_owned(),
        "approach" => "Approach".to_owned(),
        "lnav" => "LNAV".to_owned(),
        "tcas" => "TCAS".to_owned(),
        other => other.to_uppercase(),
    }
}

fn reading(label: &'static str, value: Option<String>) -> Reading {
    Reading { label, value, ..Default::default() }
}

/// The flight panel's readings of what `m` reports: heading, IAS, Mach and wind always, in that
/// order; the rest behind "More" when reported, else named in `unreported`.
pub fn aircraft_readings(m: &AdsbAircraft) -> Readings {
    let first = vec![
        reading("Heading", float(m.true_heading).map(degrees)),
        reading("IAS", number(int(m.ias), " kt", 0)),
        reading("Mach", number(float(m.mach), "", 3)),
        Reading {
            label: "Wind",
            value: match (m.wind_dir, m.wind_speed) {
                (Some(dir), Some(speed)) => Some(format!("{} at {} kt", degrees(f64::from(dir)), js_string(f64::from(speed)))),
                _ => None,
            },
            title: Some("Derived from the aircraft's airspeed, heading and ground track"),
            wide: true,
            words: false,
        },
    ];
    // [reading, how a sentence names it]
    let rest: [(Reading, &'static str); 7] = [
        (
            reading(
                "Bank",
                float(m.roll).map(|roll| {
                    let side = if roll > 0.5 {
                        "R"
                    } else if roll < -0.5 {
                        "L"
                    } else {
                        ""
                    };
                    // the template literal, then `.trim()` (only its own trailing space can go)
                    format!("{}° {side}", to_fixed(roll.abs(), 0)).trim_end().to_owned()
                }),
            ),
            "bank",
        ),
        (reading("GNSS altitude", number(int(m.alt_geom), " ft", 0)), "GNSS altitude"),
        (reading("TAS", number(int(m.tas), " kt", 0)), "TAS"),
        (
            Reading {
                title: Some("Set on the autopilot (or by the flight management system)"),
                ..reading("Selected alt.", number(int(m.nav_altitude_mcp.or(m.nav_altitude_fms)), " ft", 0))
            },
            "selected altitude",
        ),
        (
            Reading { title: Some("Set on the autopilot; magnetic"), ..reading("Selected hdg", float(m.nav_heading).map(degrees)) },
            "selected heading",
        ),
        (reading("OAT", number(int(m.oat), " °C", 0)), "OAT"),
        (
            Reading {
                label: "Autopilot modes",
                value: if m.nav_modes.is_empty() {
                    None
                } else {
                    Some(m.nav_modes.iter().map(|mode| mode_name(mode)).collect::<Vec<_>>().join(" · "))
                },
                title: None,
                wide: true,
                words: true,
            },
            "autopilot modes",
        ),
    ];
    let mut more = Vec::new();
    let mut unreported = Vec::new();
    for (r, name) in rest {
        if r.value.is_some() {
            more.push(r);
        } else {
            unreported.push(name);
        }
    }
    Readings { first, more, unreported }
}

/// "bank, OAT and autopilot modes".
pub fn listing(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> AdsbAircraft {
        AdsbAircraft {
            hex: "406f2e".to_owned(),
            callsign: "BAW548".to_owned(),
            reg: "G-TTNA".to_owned(),
            typecode: "A20N".to_owned(),
            source: "adsb_icao".to_owned(),
            mlat: false,
            seen_ms: 0,
            on_ground: false,
            ..Default::default()
        }
    }

    fn full() -> AdsbAircraft {
        AdsbAircraft {
            true_heading: Some(138.6),
            roll: Some(-12.4),
            alt_geom: Some(37625),
            ias: Some(262),
            tas: Some(452),
            mach: Some(0.78),
            wind_dir: Some(291),
            wind_speed: Some(38),
            oat: Some(-52),
            nav_altitude_mcp: Some(37008),
            nav_heading: Some(136.4),
            nav_modes: ["autopilot", "vnav", "lnav", "tcas"].map(str::to_owned).to_vec(),
            ..base()
        }
    }

    fn labels(rs: &[Reading]) -> Vec<&'static str> {
        rs.iter().map(|r| r.label).collect()
    }

    fn values(rs: &[Reading]) -> Vec<Option<&str>> {
        rs.iter().map(|r| r.value.as_deref()).collect()
    }

    fn value_of<'a>(rs: &'a [Reading], label: &str) -> Option<&'a str> {
        rs.iter().find(|r| r.label == label).and_then(|r| r.value.as_deref())
    }

    // aircraftReadings

    #[test]
    fn puts_heading_ias_mach_and_wind_first_always_in_that_order() {
        assert_eq!(labels(&aircraft_readings(&full()).first), ["Heading", "IAS", "Mach", "Wind"]);
        assert_eq!(labels(&aircraft_readings(&base()).first), ["Heading", "IAS", "Mach", "Wind"]);
    }

    #[test]
    fn formats_the_first_values_in_aviation_notation() {
        let r = aircraft_readings(&full());
        assert_eq!(values(&r.first), [Some("139°"), Some("262 kt"), Some("0.780"), Some("291° at 38 kt")]);
    }

    #[test]
    fn marks_a_first_value_the_aircraft_doesnt_report_as_null_keeping_its_place() {
        let r = aircraft_readings(&AdsbAircraft { mach: None, wind_speed: None, ..full() });
        assert_eq!(values(&r.first), [Some("139°"), Some("262 kt"), None, None]);
    }

    #[test]
    fn puts_every_other_reported_value_behind_more_nothing_unreported() {
        let r = aircraft_readings(&full());
        assert_eq!(
            labels(&r.more),
            ["Bank", "GNSS altitude", "TAS", "Selected alt.", "Selected hdg", "OAT", "Autopilot modes"]
        );
        assert_eq!(r.unreported, Vec::<&str>::new());
        assert_eq!(value_of(&r.more, "Bank"), Some("12° L"));
        assert_eq!(value_of(&r.more, "Autopilot modes"), Some("Autopilot · VNAV · LNAV · TCAS"));
    }

    #[test]
    fn leaves_unreported_values_out_of_more_and_names_them_instead() {
        let r = aircraft_readings(&AdsbAircraft { roll: None, oat: None, nav_modes: Vec::new(), ..full() });
        assert_eq!(labels(&r.more), ["GNSS altitude", "TAS", "Selected alt.", "Selected hdg"]);
        assert_eq!(r.unreported, ["bank", "OAT", "autopilot modes"]);
    }

    #[test]
    fn falls_back_to_the_fms_altitude_when_the_panels_isnt_reported() {
        let r = aircraft_readings(&AdsbAircraft { nav_altitude_fms: Some(24000), ..base() });
        assert_eq!(values(&r.more), [Some("24,000 ft")]);
    }

    #[test]
    fn with_nothing_beyond_the_basics_more_is_empty() {
        assert_eq!(aircraft_readings(&base()).more, []);
    }

    // degrees

    #[test]
    fn pads_to_three_digits_and_wraps_at_360() {
        assert_eq!(degrees(7.0), "007°");
        assert_eq!(degrees(359.6), "000°");
        assert_eq!(degrees(-10.0), "350°");
    }

    // listing

    #[test]
    fn joins_with_commas_and_a_final_and() {
        assert_eq!(listing(&[]), "");
        assert_eq!(listing(&["OAT"]), "OAT");
        assert_eq!(listing(&["bank", "OAT"]), "bank and OAT");
        assert_eq!(listing(&["bank", "OAT", "autopilot modes"]), "bank, OAT and autopilot modes");
    }

    // Beyond the TypeScript tests: the rest of each reading's shape and formatting.

    #[test]
    fn titles_widths_and_faces_are_as_in_the_typescript() {
        let r = aircraft_readings(&full());
        let shape = |rs: &[Reading]| rs.iter().map(|r| (r.label, r.title.is_some(), r.wide, r.words)).collect::<Vec<_>>();
        assert_eq!(
            shape(&r.first),
            [("Heading", false, false, false), ("IAS", false, false, false), ("Mach", false, false, false), ("Wind", true, true, false)]
        );
        assert_eq!(
            shape(&r.more),
            [
                ("Bank", false, false, false),
                ("GNSS altitude", false, false, false),
                ("TAS", false, false, false),
                ("Selected alt.", true, false, false),
                ("Selected hdg", true, false, false),
                ("OAT", false, false, false),
                ("Autopilot modes", false, true, true),
            ]
        );
        assert_eq!(
            values(&r.more),
            [
                Some("12° L"),
                Some("37,625 ft"),
                Some("452 kt"),
                Some("37,008 ft"),
                Some("136°"),
                Some("-52 °C"),
                Some("Autopilot · VNAV · LNAV · TCAS"),
            ]
        );
    }

    #[test]
    fn bank_names_a_side_only_beyond_half_a_degree_and_unknown_modes_shout() {
        let bank = |roll: f32| value_of(&aircraft_readings(&AdsbAircraft { roll: Some(roll), ..base() }).more, "Bank").map(str::to_owned);
        assert_eq!(bank(0.5).as_deref(), Some("1°"));
        assert_eq!(bank(0.75).as_deref(), Some("1° R"));
        assert_eq!(bank(-0.25).as_deref(), Some("0°"));
        assert_eq!(bank(-2.5).as_deref(), Some("3° L"));
        let modes = AdsbAircraft { nav_modes: ["althold", "approach", "loc"].map(str::to_owned).to_vec(), ..base() };
        assert_eq!(value_of(&aircraft_readings(&modes).more, "Autopilot modes"), Some("Altitude hold · Approach · LOC"));
        assert_eq!(degrees(-0.4), "000°");
        assert_eq!(degrees(719.5), "000°");
    }
}
