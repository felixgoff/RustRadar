//! Port of `freshness.ts` (doc 05 §6.4): how old the live feed's data is, and when that age is
//! worth saying out loud.
//!
//! Times are whole milliseconds (`i64`), as everywhere in this crate; [`age_label`] takes seconds
//! as `f64` because the TypeScript passes it `ageMs / 1000`. Numbers print with
//! [`js_string`], so a label reads as the template literal did.

use crate::num::js_string;

/// A missed refresh or a slow round is ordinary: below this age the status stays quiet.
pub const STALE_MIN_MS: i64 = 30_000;

/// The age past which the feed's data counts as stale: about two refresh intervals, never less
/// than the minimum.
pub fn stale_after(interval_ms: i64) -> i64 {
    STALE_MIN_MS.max(2 * interval_ms)
}

/// `45 s`, `3 min`, `2 h 05 min`: seconds under a minute, then whole minutes.
pub fn age_label(seconds: f64) -> String {
    // Math.max(0, Math.floor(seconds)): NaN stays NaN, -0 becomes 0
    let floor = seconds.floor();
    let s = if floor.is_nan() || floor > 0.0 { floor } else { 0.0 };
    if s < 60.0 {
        return format!("{} s", js_string(s));
    }
    let m = (s / 60.0).floor();
    if m < 60.0 {
        return format!("{} min", js_string(m));
    }
    let h = (m / 60.0).floor();
    let rest = m % 60.0;
    // `m % 60 ? … : …`: 0 and NaN are falsy
    if rest != 0.0 && !rest.is_nan() {
        format!("{} h {:0>2} min", js_string(h), js_string(rest))
    } else {
        format!("{} h", js_string(h))
    }
}

/// What the sidebar says about the feed's data (`freshness.ts` `Freshness`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Freshness {
    pub stale: bool,
    pub label: String,
}

/// What the sidebar says about data `age_ms` old, when the feed refreshes every `interval_ms`.
pub fn feed_freshness(age_ms: i64, interval_ms: i64) -> Freshness {
    let age = age_label(age_ms as f64 / 1000.0);
    if age_ms > stale_after(interval_ms) {
        Freshness { stale: true, label: format!("Data {age} old") }
    } else {
        Freshness { stale: false, label: format!("Updated {age} ago") }
    }
}

/// The snapshot on show: when it arrived, and the refresh schedule it was fetched on (ms).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedClock {
    pub at: i64,
    pub interval: i64,
}

/// One refresh round as [`after_round`] sees it: whether a snapshot arrived, when the round
/// ended, and the schedule set for the round after it (ms, backoff included).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Round {
    pub arrived: bool,
    pub at: i64,
    pub interval: i64,
}

/// After a refresh round. A snapshot that arrived restarts the clock, on the schedule set for the
/// round after it (backoff included). A round that failed changes neither: a backoff raised by
/// failures must not stretch the threshold for data fetched on the shorter schedule, or old data
/// would turn quiet again with nothing new arriving.
pub fn after_round(shown: Option<FeedClock>, round: Round) -> Option<FeedClock> {
    if round.arrived { Some(FeedClock { at: round.at, interval: round.interval }) } else { shown }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet(label: &str) -> Freshness {
        Freshness { stale: false, label: label.to_owned() }
    }

    fn stale(label: &str) -> Freshness {
        Freshness { stale: true, label: label.to_owned() }
    }

    // staleAfter

    #[test]
    fn is_two_refresh_intervals() {
        assert_eq!(stale_after(15_000), 30_000);
        assert_eq!(stale_after(45_000), 90_000);
    }

    #[test]
    fn never_drops_below_the_minimum() {
        assert_eq!(stale_after(8_000), STALE_MIN_MS);
        assert_eq!(stale_after(0), STALE_MIN_MS);
    }

    // ageLabel

    #[test]
    fn counts_seconds_under_a_minute() {
        assert_eq!(age_label(0.0), "0 s");
        assert_eq!(age_label(12.7), "12 s");
        assert_eq!(age_label(59.9), "59 s");
    }

    #[test]
    fn counts_whole_minutes_past_a_minute() {
        assert_eq!(age_label(60.0), "1 min");
        assert_eq!(age_label((3 * 60 + 59) as f64), "3 min");
        assert_eq!(age_label((59 * 60 + 59) as f64), "59 min");
    }

    #[test]
    fn counts_hours_and_minutes_past_an_hour() {
        assert_eq!(age_label(3600.0), "1 h");
        assert_eq!(age_label((2 * 3600 + 5 * 60) as f64), "2 h 05 min");
    }

    #[test]
    fn never_goes_negative() {
        assert_eq!(age_label(-3.0), "0 s");
    }

    // feedFreshness

    #[test]
    fn stays_quiet_within_two_refresh_intervals() {
        assert_eq!(feed_freshness(12_000, 15_000), quiet("Updated 12 s ago"));
        assert_eq!(feed_freshness(30_000, 15_000), quiet("Updated 30 s ago"));
    }

    #[test]
    fn says_how_old_the_data_is_once_past_them() {
        assert_eq!(feed_freshness(31_000, 15_000), stale("Data 31 s old"));
        assert_eq!(feed_freshness(3 * 60_000 + 20_000, 15_000), stale("Data 3 min old"));
    }

    #[test]
    fn measures_against_the_real_interval_backoff_included() {
        // rate limited: refreshes every 60 s, so a 90 s old snapshot is on schedule
        assert!(!feed_freshness(90_000, 60_000).stale);
        assert_eq!(feed_freshness(121_000, 60_000), stale("Data 2 min old"));
    }

    #[test]
    fn a_regional_feed_still_gets_the_minimums_grace() {
        assert!(!feed_freshness(20_000, 8_000).stale);
        assert!(feed_freshness(31_000, 8_000).stale);
    }

    // afterRound

    const WORLD: i64 = 15_000;

    fn arrived(at: i64, interval: i64) -> Round {
        Round { arrived: true, at, interval }
    }

    fn failed(at: i64, interval: i64) -> Round {
        Round { arrived: false, at, interval }
    }

    #[test]
    fn a_snapshot_that_arrives_restarts_the_clock_on_its_own_schedule() {
        assert_eq!(after_round(None, arrived(3_000, WORLD)), Some(FeedClock { at: 3_000, interval: WORLD }));
        // a partial snapshot that was rate limited is still news, refreshed on the longer schedule
        assert_eq!(
            after_round(Some(FeedClock { at: 3_000, interval: WORLD }), arrived(18_000, 2 * WORLD)),
            Some(FeedClock { at: 18_000, interval: 30_000 })
        );
    }

    #[test]
    fn a_failed_round_changes_nothing() {
        assert_eq!(after_round(None, failed(3_000, WORLD)), None);
        assert_eq!(
            after_round(Some(FeedClock { at: 3_000, interval: WORLD }), failed(18_000, 2 * WORLD)),
            Some(FeedClock { at: 3_000, interval: WORLD })
        );
    }

    #[test]
    fn failures_after_a_success_never_turn_stale_data_quiet_again() {
        // one success at 3 s, then a network error and rate limits, the backoff doubling each time
        let rounds = [arrived(3_000, WORLD), failed(18_000, 2 * WORLD), failed(51_000, 4 * WORLD), failed(114_000, 6 * WORLD)];
        let mut shown: Option<FeedClock> = None;
        let mut was_stale = false;
        let mut first_stale = 0;
        for t in (3_000..=200_000).step_by(1_000) {
            for r in rounds {
                if r.at == t {
                    shown = after_round(shown, r);
                }
            }
            let Some(clock) = shown else { continue };
            let stale = feed_freshness(t - clock.at, clock.interval).stale;
            assert!(!(was_stale && !stale), "quiet again at {t}");
            if stale && !was_stale {
                first_stale = t;
            }
            was_stale = stale;
        }
        // stale two world intervals after the only data, not once the backoff has run its course
        assert_eq!(first_stale, 34_000);
        assert_eq!(feed_freshness(184_000 - 3_000, WORLD), stale("Data 3 min old"));
    }

    // Beyond the TypeScript tests: the edges the template literals show.

    #[test]
    fn hours_without_minutes_and_odd_inputs_read_as_in_javascript() {
        assert_eq!(age_label(-0.0), "0 s");
        assert_eq!(age_label(-0.5), "0 s");
        assert_eq!(age_label((25 * 3600 + 59 * 60 + 59) as f64), "25 h 59 min");
        // NaN fails every comparison, so it falls through to the hours
        assert_eq!(age_label(f64::NAN), "NaN h");
        assert_eq!(age_label(f64::INFINITY), "Infinity h");
    }
}
