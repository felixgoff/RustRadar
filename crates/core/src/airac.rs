//! AIRAC cycle arithmetic of `routes.ts` (S1.16; doc 05 §8.2, doc 03 §15.1): aeronautical data
//! changes every 28 days, and cycle 2401 became effective on 2024-01-25 at 00:00 UTC.
//!
//! The TypeScript took `now = Date.now()` and returned `Date`s; here the time is passed in and
//! comes back as Unix milliseconds. The UTC calendar arithmetic is done by hand (the proleptic
//! Gregorian calendar, as JavaScript's `Date`), for every `now_ms` a `Date` can hold
//! (±8.64e15 ms).

/// `Date.UTC(2024, 0, 25)`: when cycle 2401 became effective, Unix milliseconds.
pub const AIRAC_EPOCH_MS: i64 = 1_706_140_800_000;
/// A cycle lasts 28 days.
pub const AIRAC_PERIOD_MS: i64 = 28 * 86_400_000;

const DAY_MS: i64 = 86_400_000;

/// One AIRAC cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiracCycle {
    /// e.g. `2610`: the year the cycle starts in (two digits) and the cycle's number within it.
    pub ident: String,
    /// When the cycle became effective, Unix milliseconds.
    pub effective_ms: i64,
    /// When the next cycle becomes effective, Unix milliseconds.
    pub expires_ms: i64,
}

/// The cycle in effect at `now_ms` (Unix milliseconds).
pub fn airac_cycle(now_ms: i64) -> AiracCycle {
    // `Math.floor` and `Math.ceil` of the quotients: exact here, as both sides are whole numbers
    // of milliseconds well inside a double's integers
    let n = (now_ms - AIRAC_EPOCH_MS).div_euclid(AIRAC_PERIOD_MS);
    let start = AIRAC_EPOCH_MS + n * AIRAC_PERIOD_MS;
    let year = utc_year(start);
    let first_of_year = ceil_div(utc_ms(year, 1, 1) - AIRAC_EPOCH_MS, AIRAC_PERIOD_MS);
    // `String(x).padStart(2, "0")`
    let ident = format!("{:0>2}{:0>2}", (year % 100).to_string(), (n - first_of_year + 1).to_string());
    AiracCycle { ident, effective_ms: start, expires_ms: start + AIRAC_PERIOD_MS }
}

/// `Math.ceil(a / b)` for `b > 0`.
fn ceil_div(a: i64, b: i64) -> i64 {
    -(-a).div_euclid(b)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (`month` 1-12).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    // Howard Hinnant's `days_from_civil`, for years counted from March
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The proleptic Gregorian year of a day counted from 1970-01-01 (Hinnant's `civil_from_days`).
fn year_from_days(days: i64) -> i64 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400;
    if month <= 2 { year + 1 } else { year }
}

/// `new Date(ms).getUTCFullYear()`.
fn utc_year(ms: i64) -> i64 {
    year_from_days(ms.div_euclid(DAY_MS))
}

/// `Date.UTC(year, month - 1, day)`.
fn utc_ms(year: i64, month: i64, day: i64) -> i64 {
    days_from_civil(year, month, day) * DAY_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Date.UTC(y, m - 1, d, h, min)`.
    fn utc(y: i64, m: i64, d: i64, h: i64, min: i64) -> i64 {
        utc_ms(y, m, d) + (h * 60 + min) * 60_000
    }

    #[test]
    fn the_calendar_matches_javascripts_date() {
        // Date.UTC values printed by the TypeScript's runtime
        assert_eq!(utc_ms(1970, 1, 1), 0);
        assert_eq!(utc_ms(2024, 1, 25), AIRAC_EPOCH_MS);
        assert_eq!(utc_ms(2000, 3, 1), 951_868_800_000);
        assert_eq!(utc_ms(2024, 2, 29), 1_709_164_800_000);
        assert_eq!(utc_ms(1969, 12, 31), -86_400_000);
        assert_eq!(utc_ms(1900, 1, 1), -2_208_988_800_000);
        for (ms, year) in [
            (0, 1970),
            (-1, 1969),
            (1_704_067_199_999, 2023),
            (1_704_067_200_000, 2024),
            (1_735_689_599_999, 2024),
            (1_735_689_600_000, 2025),
            (951_868_800_000, 2000),
            (-2_208_988_800_000, 1900),
            (253_402_300_800_000, 10_000),
            (-62_135_596_800_001, 0),
        ] {
            assert_eq!(utc_year(ms), year, "{ms}");
        }
        // every day for 1600 years round-trips
        let mut days = days_from_civil(1600, 1, 1);
        for year in 1600..3200 {
            assert_eq!(days, days_from_civil(year, 1, 1));
            assert_eq!(year_from_days(days), year);
            assert_eq!(year_from_days(days - 1), year - 1);
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            days += if leap { 366 } else { 365 };
        }
    }

    #[test]
    fn cycle_2401_starts_at_the_epoch() {
        let c = airac_cycle(AIRAC_EPOCH_MS);
        assert_eq!(c.ident, "2401");
        assert_eq!(c.effective_ms, AIRAC_EPOCH_MS);
        assert_eq!(c.expires_ms, AIRAC_EPOCH_MS + AIRAC_PERIOD_MS);
        // the last millisecond of 2401, and the first of 2402
        assert_eq!(airac_cycle(AIRAC_EPOCH_MS + AIRAC_PERIOD_MS - 1).ident, "2401");
        assert_eq!(airac_cycle(AIRAC_EPOCH_MS + AIRAC_PERIOD_MS).ident, "2402");
        assert_eq!(airac_cycle(AIRAC_EPOCH_MS + AIRAC_PERIOD_MS).effective_ms, utc(2024, 2, 22, 0, 0));
    }

    #[test]
    fn airac_cycle_matches_the_typescript() {
        // (now, ident, effective, expires): the TypeScript's `airacCycle(now)` for the same instants
        let cases = [
            (utc(2026, 11, 10, 12, 0), "2611", utc(2026, 10, 29, 0, 0), utc(2026, 11, 26, 0, 0)),
            (utc(2024, 12, 26, 0, 0), "2413", utc(2024, 12, 26, 0, 0), utc(2025, 1, 23, 0, 0)),
            (utc(2025, 1, 22, 23, 59), "2413", utc(2024, 12, 26, 0, 0), utc(2025, 1, 23, 0, 0)),
            (utc(2025, 1, 23, 0, 0), "2501", utc(2025, 1, 23, 0, 0), utc(2025, 2, 20, 0, 0)),
            (utc(2026, 1, 1, 0, 0), "2513", utc(2025, 12, 25, 0, 0), utc(2026, 1, 22, 0, 0)),
            (utc(2026, 1, 22, 0, 0), "2601", utc(2026, 1, 22, 0, 0), utc(2026, 2, 19, 0, 0)),
            (utc(2020, 12, 31, 0, 0), "2014", utc(2020, 12, 31, 0, 0), utc(2021, 1, 28, 0, 0)),
            (utc(2024, 1, 1, 0, 0), "2313", utc(2023, 12, 28, 0, 0), utc(2024, 1, 25, 0, 0)),
            (utc(2019, 6, 15, 8, 30), "1906", utc(2019, 5, 23, 0, 0), utc(2019, 6, 20, 0, 0)),
            (utc(2031, 3, 3, 3, 3), "3102", utc(2031, 2, 13, 0, 0), utc(2031, 3, 13, 0, 0)),
        ];
        for (now, ident, effective, expires) in cases {
            let c = airac_cycle(now);
            assert_eq!(c, AiracCycle { ident: ident.into(), effective_ms: effective, expires_ms: expires }, "{now}");
        }
    }

    #[test]
    fn a_cycle_always_holds_now() {
        let mut now = utc(2015, 1, 1, 0, 0);
        while now < utc(2035, 1, 1, 0, 0) {
            let c = airac_cycle(now);
            assert!(c.effective_ms <= now && now < c.expires_ms);
            assert_eq!(c.ident.len(), 4);
            let number: i64 = c.ident[2..].parse().unwrap();
            assert!((1..=14).contains(&number), "{}", c.ident);
            now += 86_400_000 * 3 + 12_345;
        }
    }
}
