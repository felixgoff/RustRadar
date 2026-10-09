// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { _GlobeViewport as GlobeViewport, WebMercatorViewport } from "@deck.gl/core";
import { screenAngle } from "./screen";

const close = (a: number | null, b: number, tolerance = 0.5) => {
  expect(a).not.toBeNull();
  // compare angles the short way round
  const d = ((((a as number) - b) % 360) + 540) % 360 - 180;
  expect(Math.abs(d)).toBeLessThan(tolerance);
};

describe("screenAngle", () => {
  test("matches bearing - heading at the centre of a flat, untilted view", () => {
    for (const bearing of [0, 35, -120]) {
      const viewport = new WebMercatorViewport({ width: 800, height: 600, longitude: 8, latitude: 50, zoom: 8, bearing, pitch: 0 });
      for (const heading of [0, 90, 200, 315]) close(screenAngle(viewport, [8, 50, 0], heading, 500), bearing - heading);
    }
  });

  test("east points right and north points up at the globe's centre", () => {
    const viewport = new GlobeViewport({ width: 800, height: 600, longitude: 0, latitude: 0, zoom: 2 });
    close(screenAngle(viewport, [0, 0, 0], 90, 10_000), -90);
    close(screenAngle(viewport, [0, 0, 0], 0, 10_000), 0);
  });

  test("follows the meridians' lean away from the globe's centre", () => {
    const viewport = new GlobeViewport({ width: 800, height: 600, longitude: 0, latitude: 30, zoom: 2 });
    // north-bound well west of centre: the meridian leans, so the icon must too,
    // where `bearing - heading` would keep it upright
    const angle = screenAngle(viewport, [-50, 50, 0], 0, 10_000)!;
    expect(Math.abs(angle)).toBeGreaterThan(10);
    // and an east-bound aircraft there isn't simply turned a right angle from it
    const east = screenAngle(viewport, [-50, 50, 0], 90, 10_000)!;
    expect(Math.abs(east - (angle - 90))).toBeGreaterThan(1);
  });

  test("has no direction for a zero step", () => {
    const viewport = new GlobeViewport({ width: 800, height: 600, longitude: 0, latitude: 0, zoom: 2 });
    expect(screenAngle(viewport, [0, 0, 0], 90, 0)).toBeNull();
  });
});
