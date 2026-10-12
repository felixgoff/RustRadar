// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { afterRound, ageLabel, feedFreshness, STALE_MIN_MS, staleAfter, type FeedClock } from "./freshness";

describe("staleAfter", () => {
  test("is two refresh intervals", () => {
    expect(staleAfter(15_000)).toBe(30_000);
    expect(staleAfter(45_000)).toBe(90_000);
  });

  test("never drops below the minimum", () => {
    expect(staleAfter(8_000)).toBe(STALE_MIN_MS);
    expect(staleAfter(0)).toBe(STALE_MIN_MS);
  });
});

describe("ageLabel", () => {
  test("counts seconds under a minute", () => {
    expect(ageLabel(0)).toBe("0 s");
    expect(ageLabel(12.7)).toBe("12 s");
    expect(ageLabel(59.9)).toBe("59 s");
  });

  test("counts whole minutes past a minute", () => {
    expect(ageLabel(60)).toBe("1 min");
    expect(ageLabel(3 * 60 + 59)).toBe("3 min");
    expect(ageLabel(59 * 60 + 59)).toBe("59 min");
  });

  test("counts hours and minutes past an hour", () => {
    expect(ageLabel(3600)).toBe("1 h");
    expect(ageLabel(2 * 3600 + 5 * 60)).toBe("2 h 05 min");
  });

  test("never goes negative", () => {
    expect(ageLabel(-3)).toBe("0 s");
  });
});

describe("feedFreshness", () => {
  test("stays quiet within two refresh intervals", () => {
    expect(feedFreshness(12_000, 15_000)).toEqual({ stale: false, label: "Updated 12 s ago" });
    expect(feedFreshness(30_000, 15_000)).toEqual({ stale: false, label: "Updated 30 s ago" });
  });

  test("says how old the data is once past them", () => {
    expect(feedFreshness(31_000, 15_000)).toEqual({ stale: true, label: "Data 31 s old" });
    expect(feedFreshness(3 * 60_000 + 20_000, 15_000)).toEqual({ stale: true, label: "Data 3 min old" });
  });

  test("measures against the real interval, backoff included", () => {
    // rate limited: refreshes every 60 s, so a 90 s old snapshot is on schedule
    expect(feedFreshness(90_000, 60_000).stale).toBe(false);
    expect(feedFreshness(121_000, 60_000)).toEqual({ stale: true, label: "Data 2 min old" });
  });

  test("a regional feed still gets the minimum's grace", () => {
    expect(feedFreshness(20_000, 8_000).stale).toBe(false);
    expect(feedFreshness(31_000, 8_000).stale).toBe(true);
  });
});

describe("afterRound", () => {
  const WORLD = 15_000;
  const arrived = (at: number, interval: number) => ({ arrived: true, at, interval });
  const failed = (at: number, interval: number) => ({ arrived: false, at, interval });

  test("a snapshot that arrives restarts the clock on its own schedule", () => {
    expect(afterRound(null, arrived(3_000, WORLD))).toEqual({ at: 3_000, interval: WORLD });
    // a partial snapshot that was rate limited is still news, refreshed on the longer schedule
    expect(afterRound({ at: 3_000, interval: WORLD }, arrived(18_000, 2 * WORLD))).toEqual({ at: 18_000, interval: 30_000 });
  });

  test("a failed round changes nothing", () => {
    expect(afterRound(null, failed(3_000, WORLD))).toBeNull();
    expect(afterRound({ at: 3_000, interval: WORLD }, failed(18_000, 2 * WORLD))).toEqual({ at: 3_000, interval: WORLD });
  });

  test("failures after a success never turn stale data quiet again", () => {
    // one success at 3 s, then a network error and rate limits, the backoff doubling each time
    const rounds = [arrived(3_000, WORLD), failed(18_000, 2 * WORLD), failed(51_000, 4 * WORLD), failed(114_000, 6 * WORLD)];
    let shown: FeedClock | null = null;
    let wasStale = false;
    let firstStale = 0;
    for (let t = 3_000; t <= 200_000; t += 1_000) {
      for (const r of rounds) if (r.at === t) shown = afterRound(shown, r);
      if (!shown) continue;
      const { stale } = feedFreshness(t - shown.at, shown.interval);
      expect(wasStale && !stale).toBe(false);
      if (stale && !wasStale) firstStale = t;
      wasStale = stale;
    }
    // stale two world intervals after the only data, not once the backoff has run its course
    expect(firstStale).toBe(34_000);
    expect(feedFreshness(184_000 - 3_000, WORLD)).toEqual({ stale: true, label: "Data 3 min old" });
  });
});
