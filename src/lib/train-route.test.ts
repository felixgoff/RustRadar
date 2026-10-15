// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import type { TrainRoute } from "./api";
import { type LonLat, type RouteAhead, routeAhead as ahead } from "./train-route";

/** About 11 m of longitude at 60°N: a straight line east along 60°N, one point every `step` of it. */
const east = (from: number, to: number, step = 100): LonLat[] => {
  const out: LonLat[] = [];
  for (let i = from; i <= to; i += step) out.push([18 + i * 0.000018, 60]);
  return out;
};
/** `m` metres east along the line. */
const at = (m: number, north = 0): LonLat => [18 + m * 0.000018, 60 + north / 111_320];
const route = (...segments: LonLat[][]): TrainRoute => ({ segments, stops: [], complete: true });
const metres = (a: LonLat, b: LonLat) => Math.hypot((b[0] - a[0]) * 0.5, b[1] - a[1]) * 111_320;
/** As drawn: the head on into the rest, then the stretches after. */
const routeAhead = (...args: Parameters<typeof ahead>): RouteAhead & { paths: LonLat[][] } => {
  const a = ahead(...args);
  const [first, ...later] = a.rest;
  const paths = a.head.length ? [[...a.head, ...(first ?? []).slice(1)], ...later] : [];
  return { ...a, paths };
};

describe("routeAhead", () => {
  test("starts exactly at the train and goes on from where it is along the route", () => {
    const r = route(east(0, 2000));
    const a = routeAhead(r, at(450), null);
    expect(a.on).toBe(true);
    expect(a.paths[0][0]).toEqual(at(450));
    // the next point is where it is on the route, then the route on from there
    expect(metres(a.paths[0][1], at(450))).toBeLessThan(0.5);
    expect(metres(a.paths[0][2], at(500))).toBeLessThan(0.5);
    expect(metres(a.paths[0][a.paths[0].length - 1], at(2000))).toBeLessThan(0.5);
  });

  test("the part behind the train stays trimmed off as it goes on", () => {
    const r = route(east(0, 2000));
    let cursor = null;
    for (const m of [100, 400, 900, 1500]) {
      const a = routeAhead(r, at(m), cursor);
      cursor = a.cursor;
      expect(a.paths[0][0]).toEqual(at(m));
      expect(a.paths.flat().every((p) => p[0] >= at(m)[0] - 1e-9)).toBe(true);
    }
  });

  test("a route that comes back past the train: it is on the first pass", () => {
    // out east 1 km and back along a track 10 m north of it
    const back = east(0, 1000).reverse().map(([lon]) => [lon, 60 + 10 / 111_320] as LonLat);
    const r = route([...east(0, 1000), ...back]);
    const a = routeAhead(r, at(300), null);
    expect(a.cursor.leg).toBeLessThan(10);
    expect(a.paths[0].length).toBeGreaterThan(15);
  });

  test("a train on into the next stretch, past a gap, starts from there", () => {
    const r = route(east(0, 1000), east(1500, 3000));
    const a = routeAhead(r, at(2000), null);
    expect(a.on).toBe(true);
    expect(a.paths.length).toBe(1);
    expect(a.paths[0][0]).toEqual(at(2000));
  });

  test("short of the route's start, it goes there along its own way ahead", () => {
    const r = route(east(500, 2000));
    // where the train is drawn going next: along the track, past the start
    const ahead = [at(200), at(300), at(400), at(495), at(600)];
    const a = routeAhead(r, at(100), null, ahead);
    expect(a.on).toBe(true);
    expect(a.paths[0].slice(0, 4)).toEqual([at(100), at(200), at(300), at(400)]);
    expect(metres(a.paths[0][a.paths[0].length - 1], at(2000))).toBeLessThan(0.5);
  });

  test("short of the start with no way of its own there: none drawn, ask again", () => {
    const r = route(east(500, 2000));
    const a = routeAhead(r, at(100), null, [at(150)]);
    expect(a.on).toBe(false);
    expect(a.paths).toEqual([]);
  });

  test("off the route, or past its end: none drawn, ask again", () => {
    const r = route(east(0, 1000));
    expect(routeAhead(r, at(500, 800), null)).toMatchObject({ on: false, paths: [], rest: [] });
    expect(routeAhead(r, at(1200), null)).toMatchObject({ on: false, paths: [], rest: [] });
  });

  test("beside the route, on another track: joined to it, but asked again", () => {
    const r = route(east(0, 1000));
    const a = routeAhead(r, at(500, 80), null);
    expect(a.on).toBe(false);
    expect(a.paths[0][0]).toEqual(at(500, 80));
  });

  test("the route on from the train stays the same lines while it is on the same leg", () => {
    const r = route(east(0, 2000));
    const a = ahead(r, at(410), null);
    const b = ahead(r, at(450), a.cursor);
    expect(b.rest).toBe(a.rest);
    expect(b.head[0]).toEqual(at(450));
    const c = ahead(r, at(520), b.cursor);
    expect(c.rest).not.toBe(b.rest);
  });

  test("a new route starts the search again", () => {
    const r1 = route(east(0, 2000));
    const a = routeAhead(r1, at(1500), null);
    const r2 = route(east(1400, 4000));
    const b = routeAhead(r2, at(1500), a.cursor);
    expect(b.cursor.route).toBe(r2);
    expect(b.on).toBe(true);
    expect(b.paths[0][0]).toEqual(at(1500));
  });
});
