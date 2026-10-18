// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { levelAt, onStructures, raiseFeatures, ormDeck, RAIL_DECK_M, type Structure } from "./bridges";

// east along 60°N: a degree of longitude is about 55.66 km
const DEG_PER_M = 1 / 55_660;
const at = (m: number, north = 0): [number, number] => [18 + m * DEG_PER_M, 60 + north / 111_320];

describe("levelAt", () => {
  // a 200 m bridge from 1000 m east, and a tunnel from 2000 m to 2500 m
  const bridge: Structure = { tunnel: false, line: [at(1000), at(1200)], joined: [false, false] };
  const tunnel: Structure = { tunnel: true, line: [at(2000), at(2500)], joined: [false, false] };
  const structures = [bridge, tunnel];

  test("on the ground off them", () => {
    expect(levelAt(at(500), 90, structures)).toEqual({ tunnel: false, height: 0 });
  });

  test("up on a bridge, rising from its ends", () => {
    expect(levelAt(at(1100), 90, structures).height).toBeCloseTo(RAIL_DECK_M, 5);
    expect(levelAt(at(1000), 90, structures).height).toBeCloseTo(0, 5);
    const ramp = levelAt(at(1020), 270, structures).height;
    expect(ramp).toBeGreaterThan(0);
    expect(ramp).toBeLessThan(RAIL_DECK_M);
  });

  test("an end where the bridge goes on stays up", () => {
    const cut: Structure = { ...bridge, joined: [true, false] };
    expect(levelAt(at(1000), 90, [cut]).height).toBeCloseTo(RAIL_DECK_M, 5);
  });

  test("in a tunnel", () => {
    expect(levelAt(at(2200), 90, structures).tunnel).toBe(true);
  });

  test("not a bridge it passes under, nor a track beside it", () => {
    expect(levelAt(at(1100), 0, structures).height).toBe(0);
    expect(levelAt(at(1100, 4.5), 90, structures).height).toBe(0);
  });
});

describe("onStructures", () => {
  const structures: Structure[] = [
    { tunnel: false, line: [at(100), at(200)], joined: [false, false] },
    { tunnel: true, line: [at(300), at(400)], joined: [false, false] },
  ];
  const runs = onStructures([at(0), at(500)], structures);

  test("cut where it goes into and out of a tunnel", () => {
    expect(runs.map((r) => r.tunnel)).toEqual([false, true, false]);
    const metres = (lon: number) => (lon - 18) / DEG_PER_M;
    expect(metres(runs[1].path[0][0])).toBeCloseTo(300, -1);
    expect(metres(runs[1].path[runs[1].path.length - 1][0])).toBeCloseTo(400, -1);
  });

  test("up along the bridge, in pieces", () => {
    const top = Math.max(...runs[0].path.map((p) => p[2]));
    expect(top).toBeCloseTo(RAIL_DECK_M, 5);
    expect(runs[0].path[0][2]).toBe(0);
  });

  test("nothing to cut without structures", () => {
    expect(onStructures([at(0), at(500)], [])).toEqual([{ tunnel: false, path: [[...at(0), 0], [...at(500), 0]] }]);
  });
});

describe("raiseFeatures", () => {
  const tile = { x: 2251, y: 1185, z: 12 };
  const line = (bridge: boolean) => ({
    geometry: { type: "LineString", coordinates: [[0.2, 0.5], [0.4, 0.5]] },
    properties: { bridge },
  });

  test("bridges up, the rest left on the ground", () => {
    const parts = raiseFeatures([line(false), line(true)], tile, ormDeck)!;
    expect(parts.ground).toHaveLength(1);
    expect(parts.raised).toHaveLength(1);
    const z = (parts.raised[0].geometry.coordinates as number[][]).map((c) => c[2]);
    expect(z[0]).toBe(0);
    expect(Math.max(...z)).toBeGreaterThan(0);
  });

  test("none without bridges", () => {
    expect(raiseFeatures([line(false)], tile, ormDeck)).toBeNull();
  });
});
