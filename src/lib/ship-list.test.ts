// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import type { Ship } from "./api";
import {
  DEFAULT_SHIP_FILTERS,
  mergeShipFilters,
  shipFilter,
  shipFilterCount,
  shipLength,
  shipMatches,
  shipName,
  sortShips,
} from "./ship-list";

const ship = (over: Partial<Ship>): Ship => ({
  mmsi: 230000001,
  lat: 60.1,
  lon: 24.9,
  name: "VIKING GRACE",
  kind: 3,
  size: 1,
  heading: 90,
  old: false,
  dims: null,
  frames: [],
  ...over,
});

const centre = { lat: 60.1, lon: 24.9 };

describe("shipMatches", () => {
  const s = ship({ name: "SILJA SERENADE", mmsi: 230180000 });

  test("by name, any case, every word", () => {
    expect(shipMatches(s, "serenade")).toBe(true);
    expect(shipMatches(s, "silja seren")).toBe(true);
    expect(shipMatches(s, "silja grace")).toBe(false);
  });

  test("by MMSI, from its start", () => {
    expect(shipMatches(s, "230180")).toBe(true);
    expect(shipMatches(s, "180000")).toBe(false);
  });

  test("a ship with no name of its own is found by MMSI, not by its MMSI-as-name in a word search", () => {
    const unnamed = ship({ name: "273456789", mmsi: 273456789 });
    expect(shipName(unnamed)).toBe("");
    expect(shipMatches(unnamed, "2734")).toBe(true);
    expect(shipMatches(unnamed, "grace")).toBe(false);
  });

  test("an empty search matches everything", () => {
    expect(shipMatches(s, "")).toBe(true);
  });
});

describe("shipFilter", () => {
  const list = [
    ship({ mmsi: 1, name: "A", kind: 4, size: 2, heading: 10 }),
    ship({ mmsi: 2, name: "B", kind: 6, size: 1, heading: null }),
    ship({ mmsi: 3, name: "C", kind: 8, size: 0, heading: 200, old: true }),
    // an unlisted type goes with the unknown
    ship({ mmsi: 4, name: "D", kind: 9, size: 1, heading: null }),
  ];
  const names = (f: typeof DEFAULT_SHIP_FILTERS) => list.filter(shipFilter(f)).map((s) => s.name);

  test("the defaults show every ship", () => {
    expect(names(DEFAULT_SHIP_FILTERS)).toEqual(["A", "B", "C", "D"]);
    expect(shipFilterCount(DEFAULT_SHIP_FILTERS)).toBe(0);
  });

  test("hidden types, the unknown group taking unlisted types", () => {
    expect(names({ ...DEFAULT_SHIP_FILTERS, hiddenKinds: [6, 0] })).toEqual(["A", "C"]);
  });

  test("moving has a heading; moored is the rest", () => {
    expect(names({ ...DEFAULT_SHIP_FILTERS, motion: "moving" })).toEqual(["A", "C"]);
    expect(names({ ...DEFAULT_SHIP_FILTERS, motion: "moored" })).toEqual(["B", "D"]);
  });

  test("sizes and old positions", () => {
    expect(names({ ...DEFAULT_SHIP_FILTERS, hiddenSizes: [0, 1] })).toEqual(["A"]);
    expect(names({ ...DEFAULT_SHIP_FILTERS, hideOld: true })).toEqual(["A", "B", "D"]);
  });

  test("each group counts once for the badge", () => {
    expect(shipFilterCount({ ...DEFAULT_SHIP_FILTERS, hiddenKinds: [2, 3], motion: "moored", hideOld: true })).toBe(3);
  });
});

describe("sortShips", () => {
  const list = [
    ship({ mmsi: 1, name: "BRAVO", size: 1, dims: [80, 20, 8, 8], lat: 60.5 }),
    ship({ mmsi: 2, name: "ALPHA", size: 2, dims: null, lat: 59.0 }),
    ship({ mmsi: 3, name: "CHARLIE", size: 2, dims: [200, 94, 16, 16], lat: 60.11 }),
    ship({ mmsi: 4, name: "4", size: 0, dims: null, lat: 60.2 }),
    ship({ mmsi: 5, name: "DELTA", size: 1, dims: null, lat: 61 }),
  ];
  const order = (sort: Parameters<typeof sortShips>[1]) => sortShips(list, sort, centre).map((s) => s.mmsi);

  test("by size: class first, then measured length, known before unknown", () => {
    expect(order("size")).toEqual([3, 2, 1, 5, 4]);
    expect(shipLength(list[2])).toBe(294);
  });

  test("by name: unnamed ships last", () => {
    expect(order("name")).toEqual([2, 1, 3, 5, 4]);
  });

  test("by distance from the map's centre", () => {
    expect(order("distance")).toEqual([3, 4, 1, 5, 2]);
  });
});

describe("mergeShipFilters", () => {
  test("nothing stored gives the defaults", () => {
    expect(mergeShipFilters(null)).toEqual(DEFAULT_SHIP_FILTERS);
  });

  test("stored choices are kept; unknown values dropped", () => {
    const f = mergeShipFilters({ hiddenKinds: [5, 42], hiddenSizes: [0, 7], motion: "moored", hideOld: true, sort: "name" });
    expect(f).toEqual({ hiddenKinds: [5], hiddenSizes: [0], motion: "moored", hideOld: true, sort: "name" });
    expect(mergeShipFilters({ motion: "drifting", sort: "speed" })).toEqual(DEFAULT_SHIP_FILTERS);
  });
});
