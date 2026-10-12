// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { dimensions, draught, isLocode, latLon, navTone, positionAge } from "./ship-details";

describe("positionAge", () => {
  const now = 1_791_837_457_000;
  test("recent reports read as an age", () => {
    expect(positionAge(1_791_837_457 - 120, now)).toEqual({ stale: false, label: "Position 2 min ago" });
    expect(positionAge(1_791_837_457 + 5, now)).toEqual({ stale: false, label: "Position 0 s ago" });
  });
  test("past a quarter of an hour, old", () => {
    expect(positionAge(1_791_837_457 - 15 * 60, now).stale).toBe(false);
    expect(positionAge(1_791_837_457 - 47 * 60, now)).toEqual({ stale: true, label: "Position 47 min old" });
  });
});

test("dimensions and draught leave out what isn't known", () => {
  expect(dimensions(366, 48)).toBe("366 × 48 m");
  expect(dimensions(249, null)).toBe("249 m");
  expect(dimensions(null, null)).toBeNull();
  expect(draught(9.3, 15.5)).toBe("9.3 / 15.5 m");
  expect(draught(null, 15)).toBe("max 15.0 m");
  expect(draught(null, null)).toBeNull();
});

test("positions, statuses and destinations", () => {
  expect(latLon(51.92134, -4.4812)).toBe("51.9213° N, 4.4812° W");
  expect(navTone({ code: 6, label: "Aground" })).toBe("warn");
  expect(navTone({ code: 5, label: "Moored" })).toBe("neutral");
  expect(isLocode("UKSOU")).toBe(true);
  expect(isLocode("CAPTAIN OF")).toBe(false);
});
