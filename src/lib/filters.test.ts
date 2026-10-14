// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, mock, test } from "bun:test";

// the airline lookup is Svelte state, which needs the compiler: not used here
mock.module("./reference.svelte", () => ({ airlineIcao: () => null }));
const { DEFAULT_SETTINGS, mergeSettings } = await import("./filters");

describe("mergeSettings", () => {
  test("settings saved before trains, rail map and terrain existed take their defaults", () => {
    const saved = { basemap: "satellite", labels: false, ships: false, exaggeration: 4 };
    const s = mergeSettings(saved);
    expect(s.basemap).toBe("satellite");
    expect(s.labels).toBe(false);
    expect(s.ships).toBe(false);
    expect(s.exaggeration).toBe(4);
    expect(s.trains).toBe(true);
    expect(s.railMap).toBe(false);
    expect(s.terrain).toBe(false);
  });

  test("stored choices for the new settings are kept", () => {
    const s = mergeSettings({ trains: false, railMap: true, terrain: true });
    expect([s.trains, s.railMap, s.terrain]).toEqual([false, true, true]);
  });

  test("nothing saved, or something unreadable, gives the defaults", () => {
    expect(mergeSettings(undefined)).toEqual(DEFAULT_SETTINGS);
    expect(mergeSettings("nonsense")).toEqual(DEFAULT_SETTINGS);
  });

  test("unknown keys and values of the wrong type are dropped", () => {
    const s = mergeSettings({ trains: "yes", exaggeration: null, legacy: 1 });
    expect(s.trains).toBe(true);
    expect(s.exaggeration).toBe(DEFAULT_SETTINGS.exaggeration);
    expect("legacy" in s).toBe(false);
  });

  test("a setting since removed (airline liveries, now always on) is dropped", () => {
    const s = mergeSettings({ liveries: false, labels: false });
    expect("liveries" in s).toBe(false);
    expect(s.labels).toBe(false);
    expect(s.aircraft).toBe(true);
    expect(s.aircraftLabels).toBe(false);
  });

  test("the defaults are not shared with the result", () => {
    mergeSettings({}).trains = false;
    expect(DEFAULT_SETTINGS.trains).toBe(true);
  });
});
