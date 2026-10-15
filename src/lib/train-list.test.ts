// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import type { Train } from "./api";
import {
  DEFAULT_TRAIN_FILTERS,
  distanceKm,
  distanceLabel,
  isDelayed,
  mergeTrainFilters,
  sortTrains,
  trainFilter,
  trainFilterCount,
  trainMatches,
  trainOperators,
} from "./train-list";

const train = (over: Partial<Train>): Train => ({
  id: over.name ?? "t",
  source: "digitraffic",
  name: "IC 1",
  kind: "intercity",
  lat: 60.17,
  lon: 24.94,
  heading: null,
  speedKmh: null,
  destination: null,
  operator: null,
  delayMinutes: null,
  positionUnix: 0,
  frames: [],
  ...over,
});

const helsinki = { lat: 60.17, lon: 24.94 };

describe("trainMatches", () => {
  const t = train({ name: "F4 Oslo S-Bergen", destination: "Bergen", operator: "Vy", source: "entur" });

  test("an empty search matches everything", () => {
    expect(trainMatches(t, "  ")).toBe(true);
  });

  test("by name or number, destination and operator, any case", () => {
    expect(trainMatches(t, "f4")).toBe(true);
    expect(trainMatches(t, "bergen")).toBe(true);
    expect(trainMatches(t, "VY")).toBe(true);
    expect(trainMatches(t, "tampere")).toBe(false);
  });

  test("every word has to match, in any field", () => {
    expect(trainMatches(t, "vy bergen")).toBe(true);
    expect(trainMatches(t, "vy tampere")).toBe(false);
  });

  test("accents don't matter", () => {
    const j = train({ name: "IC 27", destination: "Jyväskylä" });
    expect(trainMatches(j, "jyvaskyla")).toBe(true);
    expect(trainMatches(j, "Jyväs")).toBe(true);
  });

  test("a train without destination or operator still matches by name", () => {
    expect(trainMatches(train({ name: "HDM 123" }), "hdm")).toBe(true);
  });
});

describe("trainFilter", () => {
  const list = [
    train({ name: "a", kind: "intercity", source: "digitraffic", delayMinutes: 0 }),
    train({ name: "b", kind: "freight", source: "digitraffic", delayMinutes: 12 }),
    train({ name: "c", kind: "regional", source: "amtrak", delayMinutes: 5 }),
    train({ name: "d", kind: "regional", source: "entur", delayMinutes: null }),
  ];
  const names = (f: typeof DEFAULT_TRAIN_FILTERS) => list.filter(trainFilter(f)).map((t) => t.name);

  test("the defaults show every train", () => {
    expect(names(DEFAULT_TRAIN_FILTERS)).toEqual(["a", "b", "c", "d"]);
    expect(trainFilterCount(DEFAULT_TRAIN_FILTERS)).toBe(0);
  });

  test("hidden kinds and regions are left out", () => {
    expect(names({ ...DEFAULT_TRAIN_FILTERS, hiddenKinds: ["freight"] })).toEqual(["a", "c", "d"]);
    expect(names({ ...DEFAULT_TRAIN_FILTERS, hiddenSources: ["digitraffic", "entur"] })).toEqual(["c"]);
  });

  test("delayed only keeps trains 5 min or more late, not unknown delays", () => {
    const f = { ...DEFAULT_TRAIN_FILTERS, delayedOnly: true };
    expect(names(f)).toEqual(["b", "c"]);
    expect(trainFilterCount(f)).toBe(1);
    expect(isDelayed({ delayMinutes: 4 })).toBe(false);
    expect(isDelayed({ delayMinutes: -3 })).toBe(false);
  });

  test("hidden operators are left out, those not known as \"\"", () => {
    const ops = [
      train({ name: "sj", operator: "SJ" }),
      train({ name: "sl", operator: "SL" }),
      train({ name: "none", operator: null }),
    ];
    const shown = (hiddenOperators: string[]) =>
      ops.filter(trainFilter({ ...DEFAULT_TRAIN_FILTERS, hiddenOperators })).map((t) => t.name);
    expect(shown(["SJ"])).toEqual(["sl", "none"]);
    expect(shown([""])).toEqual(["sj", "sl"]);
    expect(trainFilterCount({ ...DEFAULT_TRAIN_FILTERS, hiddenOperators: ["SJ"] })).toBe(1);
  });

  test("operators are listed most trains first, unknown last", () => {
    const ops = [null, "SL", "SJ", "SL", null, "Mälartåg", "SL", "SJ"].map((operator) => train({ operator }));
    expect(trainOperators(ops)).toEqual([
      { name: "SL", count: 3 },
      { name: "SJ", count: 2 },
      { name: "Mälartåg", count: 1 },
      { name: "", count: 2 },
    ]);
  });

  test("each group counts once for the badge", () => {
    expect(trainFilterCount({ ...DEFAULT_TRAIN_FILTERS, hiddenKinds: ["metro", "other"], hiddenSources: ["vbb"] })).toBe(2);
  });
});

describe("sortTrains", () => {
  const list = [
    train({ name: "IC 10", delayMinutes: 3, speedKmh: 140, lat: 61.5, lon: 23.8 }),
    train({ name: "IC 2", delayMinutes: null, speedKmh: 190, lat: 60.2, lon: 24.9 }),
    train({ name: "P 50", delayMinutes: 25, speedKmh: null, lat: 65.0, lon: 25.5 }),
    train({ name: "H 9", delayMinutes: -2, speedKmh: 0, lat: 60.5, lon: 25.0 }),
  ];
  const order = (sort: Parameters<typeof sortTrains>[1]) => sortTrains(list, sort, helsinki).map((t) => t.name);

  test("by delay: most late first, early after on time, unknown last", () => {
    expect(order("delay")).toEqual(["P 50", "IC 10", "H 9", "IC 2"]);
  });

  test("by speed: fastest first, unknown last", () => {
    expect(order("speed")).toEqual(["IC 2", "IC 10", "H 9", "P 50"]);
  });

  test("by name: numbers in natural order", () => {
    expect(order("name")).toEqual(["H 9", "IC 2", "IC 10", "P 50"]);
  });

  test("by distance from the map's centre: nearest first", () => {
    expect(order("distance")).toEqual(["IC 2", "H 9", "IC 10", "P 50"]);
  });

  test("the list passed in is left as it was", () => {
    sortTrains(list, "name", helsinki);
    expect(list[0].name).toBe("IC 10");
  });
});

describe("distances", () => {
  test("Helsinki to Tampere is about 160 km", () => {
    const km = distanceKm(61.4978, 23.761, helsinki);
    expect(km).toBeGreaterThan(155);
    expect(km).toBeLessThan(165);
  });

  test("labels", () => {
    expect(distanceLabel(0.43)).toBe("430 m");
    expect(distanceLabel(12.4)).toBe("12 km");
    expect(distanceLabel(1240.2)).toBe("1,240 km");
  });
});

describe("mergeTrainFilters", () => {
  test("nothing stored gives the defaults, not shared", () => {
    const f = mergeTrainFilters(undefined);
    expect(f).toEqual(DEFAULT_TRAIN_FILTERS);
    f.hiddenKinds.push("metro");
    expect(DEFAULT_TRAIN_FILTERS.hiddenKinds).toEqual([]);
  });

  test("stored choices are kept; unknown kinds, sources and sorts are dropped", () => {
    const f = mergeTrainFilters({
      hiddenKinds: ["metro", "hovercraft"],
      hiddenSources: ["vbb", "mars"],
      hiddenOperators: ["SJ", 7],
      delayedOnly: true,
      sort: "speed",
    });
    expect(f).toEqual({
      hiddenKinds: ["metro"],
      hiddenSources: ["vbb"],
      hiddenOperators: ["SJ"],
      delayedOnly: true,
      sort: "speed",
    });
    expect(mergeTrainFilters({ sort: "colour", delayedOnly: "yes" })).toEqual(DEFAULT_TRAIN_FILTERS);
  });
});
