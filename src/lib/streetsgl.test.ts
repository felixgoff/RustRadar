// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { streetsGlUrl } from "./streetsgl";

describe("streetsGlUrl", () => {
  test("the map's view as Streets GL's camera", () => {
    const url = streetsGlUrl({ longitude: 13.00059, latitude: 55.60596, zoom: 15, bearing: -30, pitch: 45 }, 800);
    const [lat, lon, pitch, yaw, distance] = url.split("#")[1].split(",").map(Number);
    expect(url.startsWith("https://streets.gl/#")).toBe(true);
    expect([lat, lon]).toEqual([55.60596, 13.00059]);
    expect(pitch).toBe(45);
    expect(yaw).toBe(330);
    // about 1.35 m a pixel at zoom 15 there: 800 px is about 1.1 km, seen from about 1.5 km
    expect(distance).toBeGreaterThan(1_300);
    expect(distance).toBeLessThan(1_700);
  });

  test("looking straight down stays within its camera's pitch", () => {
    const url = streetsGlUrl({ longitude: 0, latitude: 0, zoom: 10, bearing: 0, pitch: 0 }, 800);
    expect(Number(url.split("#")[1].split(",")[2])).toBe(89.99);
  });
});
