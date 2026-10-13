// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import type { Train, TrainKind } from "./api";
import { shipColor } from "./ships";
import {
  delayText,
  delayTone,
  headingAt,
  positionAt,
  TRAIN_CREDITS,
  TRAIN_KINDS,
  TRAIN_SOURCE_NAMES,
  trainColor,
  trainCredits,
  trainIcon,
  trainIconSize,
  trainLabel,
  trainPositionAge,
} from "./trains";

const train = (over: Partial<Train> = {}): Train => ({
  id: "vbb:1",
  source: "vbb",
  name: "RE1",
  kind: "regional",
  lat: 52.51,
  lon: 13.435,
  heading: null,
  speedKmh: null,
  destination: null,
  operator: null,
  delayMinutes: null,
  positionUnix: 1_000,
  frames: [],
  ...over,
});

describe("colours and labels", () => {
  test("every kind has a distinct colour and a label", () => {
    const colors = TRAIN_KINDS.map((k) => trainColor(k).join());
    expect(new Set(colors).size).toBe(TRAIN_KINDS.length);
    expect(TRAIN_KINDS.map(trainLabel)).toEqual([
      "High-speed",
      "Intercity",
      "Regional",
      "Suburban",
      "Metro",
      "Freight",
      "Other",
    ]);
  });

  test("unknown kinds fall back to other", () => {
    expect(trainColor("tram" as TrainKind)).toEqual(trainColor("other"));
    expect(trainLabel("tram" as TrainKind)).toBe("Other");
  });

  test("clear of the ship palette and of the selection amber", () => {
    const ships = [0, 2, 3, 4, 5, 6, 7, 8].map(shipColor);
    const amber: [number, number, number] = [240, 180, 60];
    const dist = (a: number[], b: number[]) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    for (const kind of TRAIN_KINDS) {
      const c = trainColor(kind);
      for (const s of ships) expect(dist(c, s)).toBeGreaterThan(25);
      expect(dist(c, amber)).toBeGreaterThan(60);
    }
  });
});

describe("positionAt", () => {
  const frames: Train["frames"] = [
    [52.51, 13.435, 10_000],
    [52.5085, 13.45, 20_000],
    [52.503, 13.469, 40_000],
  ];

  test("without frames: the reported position", () => {
    expect(positionAt(train(), 15_000)).toEqual({ lat: 52.51, lon: 13.435 });
  });

  test("interpolates between frames", () => {
    const t = train({ frames });
    expect(positionAt(t, 10_000)).toEqual({ lat: 52.51, lon: 13.435 });
    const mid = positionAt(t, 15_000);
    expect(mid.lat).toBeCloseTo(52.50925, 6);
    expect(mid.lon).toBeCloseTo(13.4425, 6);
    const later = positionAt(t, 30_000);
    expect(later.lat).toBeCloseTo(52.50575, 6);
    expect(later.lon).toBeCloseTo(13.4595, 6);
    expect(positionAt(t, 20_000)).toEqual({ lat: 52.5085, lon: 13.45 });
  });

  test("holds at the ends rather than guessing", () => {
    const t = train({ frames });
    expect(positionAt(t, 0)).toEqual({ lat: 52.51, lon: 13.435 });
    expect(positionAt(t, 99_000)).toEqual({ lat: 52.503, lon: 13.469 });
    expect(positionAt(train({ frames: [[1, 2, 5]] }), 100)).toEqual({ lat: 1, lon: 2 });
  });

  test("crosses the antimeridian the short way", () => {
    const t = train({
      frames: [
        [10, 179.5, 0],
        [10, -179.5, 1_000],
      ],
    });
    expect(positionAt(t, 250).lon).toBeCloseTo(179.75, 6);
    expect(positionAt(t, 750).lon).toBeCloseTo(-179.75, 6);
  });
});

describe("headingAt", () => {
  test("along the frames, else the reported heading", () => {
    const east = train({
      heading: 10,
      frames: [
        [0, 0, 0],
        [0, 1, 1_000],
      ],
    });
    expect(headingAt(east, 500)).toBeCloseTo(90, 3);
    expect(headingAt(east, 5_000)).toBeCloseTo(90, 3);
    expect(headingAt(train({ heading: 10 }), 0)).toBe(10);
    expect(headingAt(train(), 0)).toBeNull();
  });
});

describe("credits and wording", () => {
  test("one credit per source in view", () => {
    const credits = trainCredits([train(), train({ source: "amtrak" }), train()]);
    expect(credits).toEqual([TRAIN_CREDITS.vbb, TRAIN_CREDITS.amtrak]);
    expect(TRAIN_CREDITS.digitraffic.text).toContain("CC BY 4.0");
    expect(TRAIN_CREDITS.amtrak.url).toBe("https://amtraker.com");
  });

  test("delays", () => {
    expect(delayText(5)).toBe("5 min late");
    expect(delayText(-2)).toBe("2 min early");
    expect(delayText(0)).toBe("on time");
    expect(delayText(null)).toBe("");
  });
});

describe("on the map", () => {
  test("a carriage with a heading, a square without; unknown kinds as other", () => {
    expect(trainIcon("intercity", 90)).toBe("car-intercity");
    expect(trainIcon("metro", null)).toBe("box-metro");
    expect(trainIcon("tram" as TrainKind, 0)).toBe("car-other");
  });

  test("sizes stay modest and grow with zoom", () => {
    for (const kind of TRAIN_KINDS) {
      const close = trainIconSize(kind, 0, 12);
      expect(close).toBeGreaterThan(12);
      expect(close).toBeLessThanOrEqual(24);
      expect(trainIconSize(kind, 0, 2)).toBeLessThan(close);
      expect(trainIconSize(kind, null, 12)).toBeLessThan(close);
    }
    expect(trainIconSize("highSpeed", 0, 12)).toBeGreaterThan(trainIconSize("metro", 0, 12));
  });
});

describe("panel", () => {
  test("positions go stale after five minutes", () => {
    expect(trainPositionAge(1_000, 1_040_000)).toEqual({ stale: false, label: "Position 40 s ago" });
    expect(trainPositionAge(1_000, (1_000 + 300) * 1000).stale).toBe(false);
    expect(trainPositionAge(1_000, (1_000 + 720) * 1000)).toEqual({ stale: true, label: "Position 12 min old" });
    // a report from a clock a little ahead reads as just now
    expect(trainPositionAge(1_000, 990_000).label).toBe("Position 0 s ago");
  });

  test("delay tones", () => {
    expect(delayTone(null)).toBeNull();
    expect(delayTone(-2)).toBe("good");
    expect(delayTone(4)).toBe("good");
    expect(delayTone(5)).toBe("warn");
    expect(delayTone(30)).toBe("bad");
  });

  test("every source has a name and a credit", () => {
    for (const source of Object.keys(TRAIN_CREDITS)) {
      expect(TRAIN_SOURCE_NAMES[source as keyof typeof TRAIN_SOURCE_NAMES]).toBeTruthy();
    }
    expect(TRAIN_CREDITS.amtrak.url).toBe("https://amtraker.com");
    expect(TRAIN_CREDITS.amtrak.text).toContain("Amtraker");
  });
});
