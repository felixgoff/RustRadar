// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, mock, test } from "bun:test";

// the airline lookup is Svelte state, which needs the compiler: not used here
mock.module("./reference.svelte", () => ({ airlineIcao: () => null }));
const { DEFAULT_SETTINGS, mergeSettings } = await import("./filters");

describe("mergeSettings", () => {
  test("settings saved before rail map and terrain existed take their defaults", () => {
    const saved = { basemap: "satellite", labels: false, exaggeration: 4 };
    const s = mergeSettings(saved);
    expect(s.basemap).toBe("satellite");
    expect(s.labels).toBe(false);
    expect(s.exaggeration).toBe(4);
    expect(s.railMap).toBe(false);
    expect(s.terrain).toBe(false);
    expect(s.mode).toBe("aircraft");
  });

  test("stored choices for the newer settings are kept", () => {
    const s = mergeSettings({ mode: "trains", railMap: true, terrain: true });
    expect([s.mode, s.railMap, s.terrain]).toEqual(["trains", true, true]);
  });

  test("nothing saved, or something unreadable, gives the defaults", () => {
    expect(mergeSettings(undefined)).toEqual(DEFAULT_SETTINGS);
    expect(mergeSettings("nonsense")).toEqual(DEFAULT_SETTINGS);
  });

  test("unknown keys and values of the wrong type are dropped", () => {
    const s = mergeSettings({ railMap: "yes", exaggeration: null, legacy: 1 });
    expect(s.railMap).toBe(false);
    expect(s.exaggeration).toBe(DEFAULT_SETTINGS.exaggeration);
    expect("legacy" in s).toBe(false);
  });

  test("a setting since removed (airline liveries, now always on) is dropped", () => {
    const s = mergeSettings({ liveries: false, labels: false });
    expect("liveries" in s).toBe(false);
    expect(s.labels).toBe(false);
    expect(s.aircraftLabels).toBe(false);
  });

  test("a mode that isn't one of the three falls back to aircraft", () => {
    expect(mergeSettings({ mode: "boats" }).mode).toBe("aircraft");
  });

  test("the defaults are not shared with the result", () => {
    mergeSettings({}).mode = "ships";
    expect(DEFAULT_SETTINGS.mode).toBe("aircraft");
  });
});

describe("the old aircraft, ships and trains switches become the mode", () => {
  test("they are dropped from the settings", () => {
    const s = mergeSettings({ aircraft: true, ships: false, trains: true });
    expect("aircraft" in s || "ships" in s || "trains" in s).toBe(false);
  });

  test("aircraft on (or never saved) starts in aircraft mode", () => {
    expect(mergeSettings({ aircraft: true, ships: true, trains: true }).mode).toBe("aircraft");
    expect(mergeSettings({ labels: true }).mode).toBe("aircraft");
  });

  test("aircraft off starts on ships, or on trains with the ships off too", () => {
    expect(mergeSettings({ aircraft: false, ships: true, trains: true }).mode).toBe("ships");
    expect(mergeSettings({ aircraft: false }).mode).toBe("ships");
    expect(mergeSettings({ aircraft: false, ships: false, trains: true }).mode).toBe("trains");
  });

  test("everything off starts on aircraft", () => {
    expect(mergeSettings({ aircraft: false, ships: false, trains: false }).mode).toBe("aircraft");
  });

  test("a stored mode wins over the old switches", () => {
    expect(mergeSettings({ mode: "trains", aircraft: false, ships: true }).mode).toBe("trains");
  });
});
