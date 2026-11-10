//! Port of `time.ts` (doc 05 §6.3): clock times at an airport, delays and durations. Clocks are
//! 24-hour with the zone's abbreviation, the way boards and planespotters write them: `13:25 BST`.
//!
//! Times are Unix seconds as JavaScript numbers (`f64`; see [`UnixTime`] for the optional ones).
//! Offsets and calendar fields come from jiff's bundled tz database. The abbreviations come from
//! [`zone_names`], a table generated once from today's TypeScript in Node
//! (`scripts/port/zone-names.mjs`), and then from [`COMMON_NAMES`] and [`utc_label`] exactly as
//! `zoneName` falls through today.
//!
//! The TypeScript reads "no zone" (and a zone the engine does not know) as the viewer's own zone.
//! This crate reads no system state, so that zone is held by a [`Clocks`], built once by the app
//! from the platform's zone name. Formatting is fixed to English (plan section 7, "Locale"): where
//! the TypeScript used the viewer's locale (`dayAt`) this port writes what `en-GB` writes.
//!
//! Instants outside jiff's years −9999 to 9999 are read clamped to them, and `NaN` as the epoch,
//! where JavaScript would throw (`NaN`, beyond ±8.64e15 ms) or show the far years. The
//! TypeScript's `Date.UTC` reads the years 0–99 as 1900–1999; that quirk of `utcOffset` is not
//! reproduced.

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;

use crate::num::{js_round, js_string};

pub mod zone_names;

use zone_names::ZONE_NAMES;

/// What a missing time reads as.
pub const DASH: &str = "—";

/// A Unix time in seconds, as the TypeScript's `number | null | undefined`: `0`, `NaN` and a
/// missing value all mean "no time" (they are falsy). Implemented for the integer types the model
/// stores times in, for `f64` (`now / 1000`), and for `Option` of any of them.
pub trait UnixTime: Copy {
    /// The time in seconds, or `None` when the TypeScript's `unix ? … : …` would take the else arm.
    fn unix(self) -> Option<f64>;
}

impl UnixTime for f64 {
    fn unix(self) -> Option<f64> {
        (self != 0.0 && !self.is_nan()).then_some(self)
    }
}

macro_rules! unix_int {
    ($($t:ty),*) => {$(
        impl UnixTime for $t {
            fn unix(self) -> Option<f64> {
                (self != 0).then_some(self as f64)
            }
        }
    )*};
}
unix_int!(i32, i64, u32, u64);

impl<T: UnixTime> UnixTime for Option<T> {
    fn unix(self) -> Option<f64> {
        self.and_then(T::unix)
    }
}

/// Milliseconds as `Intl.DateTimeFormat.format(ms)` reads them: TimeClip truncates toward zero.
/// Returns the milliseconds the TypeScript computed with (clamped to jiff's range) and the
/// instant the formatter shows.
fn instant(ms: f64) -> (f64, Timestamp) {
    let (min, max) = (Timestamp::MIN.as_millisecond() as f64, Timestamp::MAX.as_millisecond() as f64);
    let ms = if ms.is_nan() { 0.0 } else { ms.clamp(min, max) };
    let shown = Timestamp::from_millisecond(ms.trunc() as i64).unwrap_or(Timestamp::UNIX_EPOCH);
    (ms, shown)
}

/// The clock and calendar fields an `Intl.DateTimeFormat` in `tz` shows for `ms`.
fn wall(tz: &TimeZone, ms: f64) -> DateTime {
    tz.to_datetime(instant(ms).1)
}

/// `UTC+3`, `UTC+5:45`, `UTC−4`, or `UTC` at zero (`utcLabel`; the minus sign is U+2212).
pub fn utc_label(minutes: i32) -> String {
    if minutes == 0 {
        return "UTC".to_string();
    }
    let abs = minutes.unsigned_abs();
    let m = abs % 60;
    let sign = if minutes < 0 { "−" } else { "+" };
    let rest = if m != 0 { format!(":{m:02}") } else { String::new() };
    format!("UTC{sign}{}{rest}", abs / 60)
}

/// Each locale abbreviates only the zones its readers know (en-US: EDT, PDT; en-GB: BST, CEST;
/// ja-JP: JST) and writes `GMT+1` for the rest, so the first alphabetic answer down this list is
/// the zone's own name. Locales that would answer in another language (de-DE: MESZ) are left out.
/// Asked by `scripts/port/zone-names.mjs`; its answers are [`zone_names::ZONE_NAMES`].
pub const NAMING_LOCALES: [&str; 13] =
    ["en-US", "en-GB", "en-CA", "en-AU", "en-NZ", "en-IE", "en-IN", "en-ZA", "en-SG", "en-HK", "ja-JP", "pt-BR", "es-AR"];

/// Zones that keep another zone's clock and should carry its name. Locale data gives a name such
/// as BST to Europe/London alone, so Jersey in summer would otherwise read `UTC+1` beside London's
/// `BST` for the same clock. Old IANA names, which airport data still uses, point at their
/// current ones.
pub const SAME_CLOCK: &[(&str, &str)] = &[
    ("Europe/Jersey", "Europe/London"),
    ("Europe/Guernsey", "Europe/London"),
    ("Europe/Isle_of_Man", "Europe/London"),
    ("Europe/Belfast", "Europe/London"),
    ("Asia/Saigon", "Asia/Ho_Chi_Minh"),
    ("Asia/Rangoon", "Asia/Yangon"),
    ("Asia/Katmandu", "Asia/Kathmandu"),
    ("Asia/Istanbul", "Europe/Istanbul"),
];

/// Names some locale gives to another zone too, kept for the one zone travellers know by it
/// (Chromium also calls Dhaka `BST`). Applied when the table is generated (its test checks it).
pub const OWNED: &[(&str, &str)] = &[("BST", "Europe/London")];

/// Names in common English use that no locale above supplies, by zone and offset (minutes east),
/// so summer time keeps its own name. Left out where the name would be ambiguous: China's and
/// Taiwan's CST, the Gulf's AST, Israel's IST; those keep their UTC offset.
pub const COMMON_NAMES: &[(&str, i32, &str)] = &[
    ("Asia/Seoul", 540, "KST"),
    ("Europe/Moscow", 180, "MSK"),
    ("Europe/Istanbul", 180, "TRT"),
    ("Europe/Samara", 240, "SAMT"),
    ("Asia/Manila", 480, "PHT"),
    ("Asia/Bangkok", 420, "ICT"),
    ("Asia/Ho_Chi_Minh", 420, "ICT"),
    ("Asia/Jakarta", 420, "WIB"),
    ("Asia/Karachi", 300, "PKT"),
    ("Asia/Tashkent", 300, "UZT"),
    ("Asia/Kathmandu", 345, "NPT"),
    ("Asia/Yangon", 390, "MMT"),
    ("Asia/Tehran", 210, "IRST"),
    ("Asia/Baku", 240, "AZT"),
    ("Asia/Tbilisi", 240, "GET"),
    ("America/Bogota", -300, "COT"),
    ("America/Lima", -300, "PET"),
    ("America/Guayaquil", -300, "ECT"),
    ("America/Caracas", -240, "VET"),
    ("America/La_Paz", -240, "BOT"),
    ("America/Santiago", -240, "CLT"),
    ("America/Santiago", -180, "CLST"),
    ("America/Montevideo", -180, "UYT"),
    ("America/Asuncion", -180, "PYT"),
    ("Pacific/Fiji", 720, "FJT"),
];

fn same_clock(zone: &str) -> Option<&'static str> {
    SAME_CLOCK.iter().find(|&&(from, _)| from == zone).map(|&(_, to)| to)
}

fn common_name(zone: &str, offset: i32) -> Option<&'static str> {
    COMMON_NAMES.iter().find(|&&(z, o, _)| z == zone && o == offset).map(|&(_, _, name)| name)
}

/// What `abbreviation` answered for `zone` at `offset`: the generated table's row, if any.
fn abbreviation(zone: &str, offset: i32) -> Option<&'static str> {
    let offset = i16::try_from(offset).ok()?;
    let row = ZONE_NAMES.binary_search_by(|&(z, o, _)| z.cmp(zone).then(o.cmp(&offset))).ok()?;
    Some(ZONE_NAMES[row].2)
}

/// Whether the tz database knows the zone; an unknown one is read as the viewer's own (`known`).
fn known(time_zone: &str) -> bool {
    TimeZone::get(time_zone).is_ok()
}

/// The clocks of `time.ts`, read for one viewer: a time given without a zone (`None` or `""`), or
/// with a zone the tz database does not know, is read in the viewer's own zone, as the
/// TypeScript's formatters fall back to it. `Default` is a viewer in UTC.
#[derive(Debug, Clone)]
pub struct Clocks {
    own: TimeZone,
    own_name: String,
}

impl Default for Clocks {
    fn default() -> Self {
        Clocks::new("UTC")
    }
}

impl Clocks {
    /// Clocks for a viewer in the IANA zone `own_zone` (what the platform reports, e.g.
    /// `Europe/Stockholm`). A name the tz database does not know gives a viewer in UTC.
    pub fn new(own_zone: &str) -> Self {
        match TimeZone::get(own_zone) {
            Ok(own) => {
                let own_name = own.iana_name().unwrap_or(own_zone).to_string();
                Clocks { own, own_name }
            }
            Err(_) => Clocks { own: TimeZone::UTC, own_name: "UTC".to_string() },
        }
    }

    /// The viewer's own IANA zone (`ownZone`).
    pub fn own_zone(&self) -> &str {
        &self.own_name
    }

    /// The zone a formatter for `time_zone` shows: that zone, or the viewer's own without one or
    /// for one the database does not know (`formatter`'s `catch`).
    fn zone(&self, time_zone: Option<&str>) -> TimeZone {
        match time_zone {
            Some(name) if !name.is_empty() => TimeZone::get(name).unwrap_or_else(|_| self.own.clone()),
            _ => self.own.clone(),
        }
    }

    /// `clock(timeZone).format(unix * 1000)`: `en-GB`, two-digit hour (`h23`) and minute.
    fn clock(&self, unix: f64, time_zone: Option<&str>) -> String {
        let at = wall(&self.zone(time_zone), unix * 1000.0);
        format!("{:02}:{:02}", at.hour(), at.minute())
    }

    /// `13:25`, 24-hour, in an IANA zone (the viewer's own without one), or a dash (`clockAt`).
    pub fn clock_at(&self, unix: impl UnixTime, time_zone: Option<&str>) -> String {
        match unix.unix() {
            Some(unix) => self.clock(unix, time_zone),
            None => DASH.to_string(),
        }
    }

    /// `13:25 BST`: the clock time with its zone's name at that moment, or a dash
    /// (`clockWithZone`).
    pub fn clock_with_zone(&self, unix: impl UnixTime, time_zone: Option<&str>) -> String {
        match unix.unix() {
            Some(unix) => format!("{} {}", self.clock(unix, time_zone), self.zone_name(unix, time_zone)),
            None => DASH.to_string(),
        }
    }

    /// Minutes past midnight in an IANA zone (the viewer's own without one); agrees with
    /// [`Clocks::clock_at`] (`minutesOfDay`).
    pub fn minutes_of_day(&self, unix: f64, time_zone: Option<&str>) -> i32 {
        let at = wall(&self.zone(time_zone), unix * 1000.0);
        (i32::from(at.hour()) * 60 + i32::from(at.minute())) % 1440
    }

    /// The zone's offset from UTC at that moment, in minutes east (`utcOffset`): the wall clock's
    /// fields to the second, read as UTC, less the instant, rounded to minutes.
    fn utc_offset(&self, unix: f64, time_zone: Option<&str>) -> i32 {
        let (ms, shown) = instant(unix * 1000.0);
        let offset_ms = i64::from(self.zone(time_zone).to_offset(shown).seconds()) * 1000;
        // the fields floor to the second; the offset is whole seconds, so this is `Date.UTC(…)`
        let wall = shown.as_millisecond().div_euclid(1000) * 1000 + offset_ms;
        js_round((wall as f64 - ms) / 60_000.0) as i32
    }

    /// The zone's abbreviation at that moment, e.g. `BST`, `CEST`, `EDT`, `JST`, `KST`; `UTC+3`
    /// where it has none in common, unambiguous use (`zoneName`).
    pub fn zone_name(&self, unix: f64, time_zone: Option<&str>) -> String {
        let offset = self.utc_offset(unix, time_zone);
        let own = match time_zone {
            Some(name) if !name.is_empty() && known(name) => name,
            _ => self.own_name.as_str(),
        };
        // named by the zone it shares a clock with, read at the same offset
        let zone = same_clock(own).unwrap_or(own);
        match abbreviation(zone, offset).or_else(|| common_name(zone, offset)) {
            Some(name) => name.to_string(),
            None => utc_label(offset),
        }
    }

    /// `Today`, `Yesterday`, `Tomorrow` or `Mon 6 Oct`, in an IANA zone (`dayAt`). Calendar days
    /// are compared at `now_ms` and 24 hours either side of it; the fallback is written as
    /// `en-GB` writes it (`Sept` for September).
    pub fn day_at(&self, unix: f64, time_zone: Option<&str>, now_ms: i64) -> String {
        let tz = self.zone(time_zone);
        let target = wall(&tz, unix * 1000.0);
        let offsets = [(0, "Today"), (-1, "Yesterday"), (1, "Tomorrow")];
        for (offset, label) in offsets {
            if wall(&tz, now_ms as f64 + f64::from(offset) * 86_400_000.0).date() == target.date() {
                return label.to_string();
            }
        }
        const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sept", "Oct", "Nov", "Dec"];
        let weekday = WEEKDAYS[target.weekday().to_monday_zero_offset() as usize];
        format!("{weekday} {} {}", target.day(), MONTHS[target.month() as usize - 1])
    }
}

/// `5 h 10 min`, `45 min` (`duration`). Minutes round, so `3599` reads `60 min`, as today.
pub fn duration(seconds: f64) -> String {
    let h = (seconds / 3600.0).floor();
    let m = js_round((seconds % 3600.0) / 60.0);
    if h != 0.0 && !h.is_nan() {
        format!("{} h {:0>2} min", js_string(h), js_string(m))
    } else {
        format!("{} min", js_string(m))
    }
}

/// How a status reads: `good`, `warn` or `bad` (`Tone`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    Good,
    Warn,
    Bad,
}

impl Tone {
    /// The TypeScript's string for it, as the CSS classes use it.
    pub fn as_str(self) -> &'static str {
        match self {
            Tone::Good => "good",
            Tone::Warn => "warn",
            Tone::Bad => "bad",
        }
    }
}

/// Minutes late (negative: early) against the schedule, when both times are known
/// (`delayMinutes`).
pub fn delay_minutes(scheduled: impl UnixTime, actual: impl UnixTime) -> Option<i64> {
    let (scheduled, actual) = (scheduled.unix()?, actual.unix()?);
    Some(js_round((actual - scheduled) / 60.0) as i64)
}

/// A punctuality verdict: its words and its tone (`punctuality`'s `{ label, tone }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Punctuality {
    pub label: String,
    pub tone: Tone,
}

/// Departure-board wording: on time within a quarter of an hour, as airlines count it
/// (`punctuality`).
pub fn punctuality(minutes: Option<i64>) -> Option<Punctuality> {
    let minutes = minutes?;
    let (label, tone) = if minutes >= 45 {
        (format!("{} late", duration((minutes * 60) as f64)), Tone::Bad)
    } else if minutes >= 15 {
        (format!("{minutes} min late"), Tone::Warn)
    } else if minutes <= -5 {
        (format!("{} min early", -minutes), Tone::Good)
    } else {
        ("On time".to_string(), Tone::Good)
    };
    Some(Punctuality { label, tone })
}

/// Flightradar24's board status colours, as tones (`toneOf`).
pub fn tone_of(color: Option<&str>) -> Option<Tone> {
    match color {
        Some("green") => Some(Tone::Good),
        Some("yellow") | Some("orange") => Some(Tone::Warn),
        Some("red") => Some(Tone::Bad),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zone_names::INSTANTS_MS;

    fn at(iso: &str) -> f64 {
        iso.parse::<Timestamp>().unwrap().as_millisecond() as f64 / 1000.0
    }

    fn summer() -> f64 {
        at("2026-07-15T12:25:00Z")
    }

    fn winter() -> f64 {
        at("2026-01-15T12:25:00Z")
    }

    /// The TypeScript tests run in the test machine's zone; these in one with summer time.
    fn clocks() -> Clocks {
        Clocks::new("Europe/Stockholm")
    }

    fn zone_name(unix: f64, zone: &str) -> String {
        clocks().zone_name(unix, Some(zone))
    }

    fn clock_at(unix: f64, zone: &str) -> String {
        clocks().clock_at(unix, Some(zone))
    }

    #[test]
    fn clocks_are_24_hour() {
        assert_eq!(clock_at(summer(), "Europe/London"), "13:25");
        assert_eq!(clock_at(winter(), "Europe/London"), "12:25");
        assert_eq!(clock_at(summer(), "America/New_York"), "08:25");
        assert_eq!(clock_at(summer(), "Asia/Tokyo"), "21:25");
        assert_eq!(clock_at(at("2026-07-14T23:05:00Z"), "Europe/London"), "00:05");
    }

    #[test]
    fn a_missing_time_is_a_dash() {
        let c = clocks();
        assert_eq!(c.clock_at(None::<f64>, Some("Europe/London")), "—");
        assert_eq!(c.clock_at(None::<f64>, None), "—");
        assert_eq!(c.clock_with_zone(0.0, Some("Europe/London")), "—");
        // the model's integer times: 0 is falsy there too
        assert_eq!(c.clock_with_zone(0_u32, Some("Europe/London")), "—");
        assert_eq!(c.clock_at(f64::NAN, Some("Europe/London")), "—");
    }

    #[test]
    fn minutes_of_the_day_agree_with_the_clock() {
        let c = clocks();
        assert_eq!(c.minutes_of_day(summer(), Some("Europe/London")), 13 * 60 + 25);
        assert_eq!(c.minutes_of_day(at("2026-07-14T23:05:00Z"), Some("Europe/London")), 5);
    }

    #[test]
    fn zones_are_named_by_their_abbreviation_not_an_offset() {
        assert_eq!(zone_name(summer(), "Europe/London"), "BST");
        assert_eq!(zone_name(winter(), "Europe/London"), "GMT");
        assert_eq!(zone_name(summer(), "Europe/Paris"), "CEST");
        assert_eq!(zone_name(winter(), "Europe/Paris"), "CET");
        assert_eq!(zone_name(summer(), "America/New_York"), "EDT");
        assert_eq!(zone_name(winter(), "America/New_York"), "EST");
        assert_eq!(zone_name(summer(), "America/Los_Angeles"), "PDT");
        assert_eq!(zone_name(summer(), "Asia/Tokyo"), "JST");
        assert_eq!(zone_name(winter(), "Asia/Tokyo"), "JST");
        assert_eq!(zone_name(winter(), "Australia/Sydney"), "AEDT");
    }

    #[test]
    fn a_real_name_beats_a_locales_plain_gmt_at_zero_offset() {
        assert_eq!(zone_name(winter(), "Europe/Lisbon"), "WET");
    }

    #[test]
    fn utc_stays_utc() {
        assert_eq!(zone_name(summer(), "UTC"), "UTC");
        assert_eq!(zone_name(summer(), "Etc/UTC"), "UTC");
    }

    #[test]
    fn a_zone_without_an_abbreviation_in_common_use_falls_back_to_its_utc_offset() {
        assert_eq!(zone_name(summer(), "Etc/GMT-14"), "UTC+14");
        assert_eq!(zone_name(summer(), "Etc/GMT+5"), "UTC−5");
        // CST and AST would each name two different clocks
        assert_eq!(zone_name(summer(), "Asia/Shanghai"), "UTC+8");
        assert_eq!(zone_name(summer(), "Asia/Qatar"), "UTC+3");
    }

    #[test]
    fn hubs_that_no_locale_names_get_their_common_english_name() {
        assert_eq!(zone_name(summer(), "Asia/Seoul"), "KST");
        assert_eq!(zone_name(summer(), "Europe/Istanbul"), "TRT");
        assert_eq!(zone_name(winter(), "Europe/Moscow"), "MSK");
        assert_eq!(zone_name(summer(), "Asia/Manila"), "PHT");
        assert_eq!(zone_name(summer(), "America/Bogota"), "COT");
        assert_eq!(zone_name(summer(), "America/Lima"), "PET");
        assert_eq!(zone_name(summer(), "Asia/Kathmandu"), "NPT");
        // Chile keeps summer time in the southern summer
        assert_eq!(zone_name(summer(), "America/Santiago"), "CLT");
        assert_eq!(zone_name(winter(), "America/Santiago"), "CLST");
    }

    #[test]
    fn the_crown_dependencies_keep_londons_names() {
        assert_eq!(zone_name(summer(), "Europe/Jersey"), "BST");
        assert_eq!(zone_name(summer(), "Europe/Guernsey"), "BST");
        assert_eq!(zone_name(summer(), "Europe/Isle_of_Man"), "BST");
        assert_eq!(zone_name(winter(), "Europe/Jersey"), "GMT");
        assert_eq!(clock_at(summer(), "Europe/Jersey"), "13:25");
    }

    #[test]
    fn bst_is_londons_alone_dhaka_keeps_its_offset() {
        assert_eq!(zone_name(summer(), "Asia/Dhaka"), "UTC+6");
        assert_eq!(zone_name(winter(), "Asia/Dhaka"), "UTC+6");
    }

    #[test]
    fn old_zone_names_are_named_like_their_current_ones() {
        assert_eq!(zone_name(summer(), "Asia/Saigon"), "ICT");
        assert_eq!(zone_name(summer(), "Asia/Istanbul"), "TRT");
    }

    #[test]
    fn clock_with_zone() {
        let c = clocks();
        assert_eq!(c.clock_with_zone(summer(), Some("Europe/London")), "13:25 BST");
        assert_eq!(c.clock_with_zone(winter(), Some("America/New_York")), "07:25 EST");
        assert_eq!(c.clock_with_zone(summer(), Some("Europe/Istanbul")), "15:25 TRT");
        assert_eq!(c.clock_with_zone(summer(), Some("Asia/Shanghai")), "20:25 UTC+8");
    }

    #[test]
    fn an_unknown_zone_falls_back_to_the_viewers_own_rather_than_failing() {
        let c = clocks();
        assert_eq!(c.clock_at(summer(), Some("Mars/Olympus_Mons")), c.clock_at(summer(), None));
        assert_eq!(c.zone_name(summer(), Some("Mars/Olympus_Mons")), c.zone_name(summer(), None));
        // and the viewer's own zone is the one the clocks were made for
        assert_eq!(c.clock_with_zone(summer(), Some("Mars/Olympus_Mons")), "14:25 CEST");
        assert_eq!(c.clock_with_zone(summer(), Some("")), "14:25 CEST");
        assert_eq!(Clocks::default().clock_with_zone(summer(), None), "12:25 UTC");
        assert_eq!(Clocks::new("Mars/Olympus_Mons").own_zone(), "UTC");
        // a viewer in a zone that keeps another's clock is named by that one
        assert_eq!(Clocks::new("Europe/Jersey").clock_with_zone(summer(), None), "13:25 BST");
    }

    // ---- new tests (no TypeScript twin) ----

    /// The acceptance check of S1.11: every row of the generated table is a name for an offset
    /// that jiff's tz database gives its zone at one of the January and July instants it was
    /// read at, and the Rust path finds that row there.
    #[test]
    fn the_generated_table_agrees_with_jiffs_offsets() {
        assert!(ZONE_NAMES.len() > 400, "{} rows", ZONE_NAMES.len());
        for &(zone, offset, name) in ZONE_NAMES {
            let tz = TimeZone::get(zone).unwrap_or_else(|e| panic!("{zone}: {e}"));
            let offsets = INSTANTS_MS.map(|ms| tz.to_offset(Timestamp::from_millisecond(ms).unwrap()).seconds() / 60);
            let Some(i) = offsets.iter().position(|&o| o == i32::from(offset)) else {
                panic!("{zone} {offset} {name}: jiff gives {offsets:?}");
            };
            let unix = INSTANTS_MS[i] as f64 / 1000.0;
            assert_eq!(Clocks::default().zone_name(unix, Some(zone)), name, "{zone} at {offset}");
        }
    }

    #[test]
    fn the_generated_table_is_sorted_and_holds_only_locale_names() {
        for pair in ZONE_NAMES.windows(2) {
            let (a, b) = ((pair[0].0, pair[0].1), (pair[1].0, pair[1].1));
            assert!(a < b, "{a:?} before {b:?}");
        }
        for &(zone, offset, name) in ZONE_NAMES {
            // `ABBREVIATION = /^[A-Z]{2,6}$/`
            assert!((2..=6).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_uppercase()), "{zone}: {name}");
            // OWNED: a name kept for one zone is nobody else's
            if let Some(&(_, owner)) = OWNED.iter().find(|&&(n, _)| n == name) {
                assert_eq!(zone, owner, "{name}");
            }
            // zones that keep another's clock are read through it, and fallbacks are not repeated
            assert!(same_clock(zone).is_none(), "{zone}");
            assert_ne!(Some(name), common_name(zone, i32::from(offset)), "{zone}");
            assert_ne!(name, utc_label(i32::from(offset)), "{zone}");
        }
    }

    #[test]
    fn utc_labels() {
        assert_eq!(utc_label(0), "UTC");
        assert_eq!(utc_label(180), "UTC+3");
        assert_eq!(utc_label(345), "UTC+5:45");
        assert_eq!(utc_label(-240), "UTC−4");
        assert_eq!(utc_label(-570), "UTC−9:30");
    }

    #[test]
    fn zones_named_by_the_table_beyond_the_ported_cases() {
        assert_eq!(zone_name(summer(), "Asia/Kolkata"), "IST");
        assert_eq!(zone_name(summer(), "Asia/Calcutta"), "IST");
        assert_eq!(zone_name(summer(), "America/St_Johns"), "NDT");
        assert_eq!(zone_name(summer(), "Australia/Adelaide"), "ACST");
        assert_eq!(zone_name(summer(), "Pacific/Chatham"), "CHAST");
        assert_eq!(zone_name(summer(), "Australia/Eucla"), "ACWST");
        assert_eq!(zone_name(summer(), "Europe/Belfast"), "BST");
        assert_eq!(zone_name(summer(), "Asia/Tehran"), "IRST");
        assert_eq!(zone_name(winter(), "America/Caracas"), "VET");
        assert_eq!(zone_name(summer(), "America/Asuncion"), "PYT");
        assert_eq!(zone_name(winter(), "America/Asuncion"), "PYT");
        // offsets that only the second year shows: Morocco on UTC+0 from October 2026
        assert_eq!(zone_name(summer(), "Africa/Casablanca"), "UTC+1");
        assert_eq!(zone_name(at("2027-01-15T12:00:00Z"), "Africa/Casablanca"), "WET");
        // and the first year alone: British Columbia keeps summer time from November 2026
        assert_eq!(zone_name(winter(), "America/Vancouver"), "PST");
        assert_eq!(zone_name(at("2027-01-15T12:00:00Z"), "America/Vancouver"), "PDT");
        // the table is case-sensitive, as `SAME_CLOCK` and `OWNED` were: London's BST needs its name
        assert_eq!(zone_name(summer(), "europe/london"), "UTC+1");
        // an offset in seconds (Madras time, +5:21:10) rounds to minutes
        assert_eq!(zone_name(at("1900-01-01T00:00:00Z"), "Asia/Kolkata"), "UTC+5:21");
    }

    #[test]
    fn clocks_read_the_instant_as_intl_does() {
        let c = Clocks::default();
        assert_eq!(c.minutes_of_day(at("2026-07-15T23:59:59.999Z"), Some("UTC")), 1439);
        assert_eq!(c.clock_at(at("2026-07-15T12:25:59.999Z"), Some("UTC")), "12:25");
        // before the epoch the milliseconds truncate toward zero, the fields floor
        assert_eq!(c.clock_at(-1.5, Some("UTC")), "23:59");
        assert_eq!(c.clock_at(0.0005, Some("UTC")), "00:00");
        assert_eq!(c.clock_at(Some(1_784_118_300_i64), Some("Europe/London")), "13:25");
    }

    #[test]
    fn days_are_today_yesterday_tomorrow_or_a_date() {
        let c = clocks();
        let now = (summer() * 1000.0) as i64;
        let day = 86_400.0;
        assert_eq!(c.day_at(summer(), Some("UTC"), now), "Today");
        assert_eq!(c.day_at(summer() - day, Some("UTC"), now), "Yesterday");
        assert_eq!(c.day_at(summer() + day, Some("UTC"), now), "Tomorrow");
        assert_eq!(c.day_at(summer() + 2.0 * day, Some("UTC"), now), "Fri 17 Jul");
        assert_eq!(c.day_at(at("2026-09-06T10:00:00Z"), Some("UTC"), now), "Sun 6 Sept");
        // the calendar day is the zone's
        assert_eq!(c.day_at(at("2026-07-15T23:30:00Z"), Some("Europe/London"), now), "Tomorrow");
        assert_eq!(c.day_at(at("2026-07-15T23:30:00Z"), Some("UTC"), now), "Today");
        assert_eq!(c.day_at(at("2026-07-15T23:30:00Z"), Some("Mars/x"), now), "Tomorrow");
        assert_eq!(c.day_at(at("2026-07-15T23:30:00Z"), None, now), "Tomorrow");
    }

    #[test]
    fn durations_round_to_minutes() {
        let cases = [
            (3599.0, "60 min"),
            (18_600.0, "5 h 10 min"),
            (2700.0, "45 min"),
            (7199.0, "1 h 60 min"),
            (90.0, "2 min"),
            (-90.0, "-1 h -1 min"),
            (0.0, "0 min"),
            (59.4, "1 min"),
            (29.0, "0 min"),
        ];
        for (seconds, label) in cases {
            assert_eq!(duration(seconds), label, "{seconds}");
        }
    }

    #[test]
    fn delays_and_punctuality() {
        assert_eq!(delay_minutes(1000_i64, 1090_i64), Some(2));
        assert_eq!(delay_minutes(1090_i64, 1000_i64), Some(-1));
        assert_eq!(delay_minutes(0_u32, 5_u32), None);
        assert_eq!(delay_minutes(Some(5_i64), None::<i64>), None);
        let p = |m: i64| punctuality(Some(m)).map(|p| (p.label, p.tone.as_str()));
        assert_eq!(p(45), Some(("45 min late".to_string(), "bad")));
        assert_eq!(p(60), Some(("1 h 00 min late".to_string(), "bad")));
        assert_eq!(p(125), Some(("2 h 05 min late".to_string(), "bad")));
        assert_eq!(p(15), Some(("15 min late".to_string(), "warn")));
        assert_eq!(p(14), Some(("On time".to_string(), "good")));
        assert_eq!(p(-4), Some(("On time".to_string(), "good")));
        assert_eq!(p(-5), Some(("5 min early".to_string(), "good")));
        assert_eq!(punctuality(None), None);
    }

    #[test]
    fn board_colours_are_tones() {
        assert_eq!(tone_of(Some("green")), Some(Tone::Good));
        assert_eq!(tone_of(Some("yellow")), Some(Tone::Warn));
        assert_eq!(tone_of(Some("orange")), Some(Tone::Warn));
        assert_eq!(tone_of(Some("red")), Some(Tone::Bad));
        assert_eq!(tone_of(Some("grey")), None);
        assert_eq!(tone_of(None), None);
    }
}
