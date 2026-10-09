// Unit tests for the hover card's details loader. Run with `bun test src`.
// @ts-ignore -- bun's types aren't a dependency; svelte-check still type-checks this file
import * as bunTest from "bun:test";
import type { FlightDetails } from "./api";
import { HoverLoader, routeProgress } from "./hover-details";

interface Matchers {
  toBe(value: unknown): void;
  toEqual(value: unknown): void;
  toBeCloseTo(value: number, digits?: number): void;
}
const { test, expect } = bunTest as unknown as {
  test(name: string, fn: () => void | Promise<void>): void;
  expect(value: unknown): Matchers;
};

const DWELL = 5;
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

function details(id: number): FlightDetails {
  return {
    aircraft: {
      icao_address: 0,
      reg: "",
      typecode: "",
      full_description: "",
      service: "",
      registered_owners: "",
      msn: null,
      birth_date: null,
      age: null,
      images: [{ url: "u", copyright: `p${id}`, thumbnail: "t", medium: "m", large: "l" }],
    },
    schedule: null,
    progress: null,
    flight: null,
    flight_plan: null,
    trail: [],
  };
}

/** A loader whose requests settle only when the test says so. */
function setup(now = () => Date.now()) {
  const calls: number[] = [];
  const settle = new Map<number, { ok: () => void; fail: (e: string) => void }>();
  const loader = new HoverLoader({
    fetch: (id) => {
      calls.push(id);
      return new Promise((resolve, reject) => settle.set(id, { ok: () => resolve(details(id)), fail: reject }));
    },
    isRateLimited: (m) => m.includes("status 8"),
    onchange: () => {},
    dwellMs: DWELL,
    now,
  });
  return { loader, calls, settle };
}

test("fetches only after the pointer rests", async () => {
  const { loader, calls } = setup();
  loader.hover(1);
  loader.hover(2);
  loader.hover(3);
  expect(calls).toEqual([]);
  await wait(DWELL * 3);
  expect(calls).toEqual([3]);
  expect(loader.status(3)).toBe("loading");
});

test("caches answers and never asks twice", async () => {
  const { loader, calls, settle } = setup();
  loader.hover(1);
  await wait(DWELL * 3);
  settle.get(1)!.ok();
  await wait(1);
  expect(loader.status(1)).toBe("ready");
  expect(loader.info(1)!.images[0].copyright).toBe("p1");
  loader.hover(null);
  loader.hover(1);
  await wait(DWELL * 3);
  expect(calls).toEqual([1]);
});

test("one request at a time, and stale waiting flights are dropped", async () => {
  const { loader, calls, settle } = setup();
  loader.hover(1);
  await wait(DWELL * 3);
  loader.hover(2);
  await wait(DWELL * 3);
  loader.hover(3);
  await wait(DWELL * 3);
  expect(calls).toEqual([1]); // 2 and 3 wait; 3 replaced 2
  loader.hover(null); // and the pointer left
  settle.get(1)!.ok();
  await wait(1);
  expect(calls).toEqual([1]);
});

test("the waiting flight is fetched next if still hovered", async () => {
  const { loader, calls, settle } = setup();
  loader.hover(1);
  await wait(DWELL * 3);
  loader.hover(2);
  await wait(DWELL * 3);
  settle.get(1)!.ok();
  await wait(1);
  expect(calls).toEqual([1, 2]);
});

test("rate limiting pauses all requests", async () => {
  let t = 1_000_000;
  const { loader, calls, settle } = setup(() => t);
  loader.hover(1);
  await wait(DWELL * 3);
  settle.get(1)!.fail("grpc status 8");
  await wait(1);
  expect(loader.status(1)).toBe("unavailable");
  loader.hover(2);
  await wait(DWELL * 3);
  expect(calls).toEqual([1]);
  expect(loader.status(2)).toBe("unavailable");
  t += 61_000;
  loader.hover(3);
  await wait(DWELL * 3);
  expect(calls).toEqual([1, 3]);
});

test("a failure is remembered for a while", async () => {
  let t = 1_000_000;
  const { loader, calls, settle } = setup(() => t);
  loader.hover(1);
  await wait(DWELL * 3);
  settle.get(1)!.fail("offline");
  await wait(1);
  loader.hover(null);
  loader.hover(1);
  await wait(DWELL * 3);
  expect(calls).toEqual([1]);
  expect(loader.status(1)).toBe("unavailable");
  t += 3 * 60_000;
  loader.hover(null);
  loader.hover(1);
  await wait(DWELL * 3);
  expect(calls).toEqual([1, 1]);
});

test("route progress from great-circle distances", () => {
  const vno = { lat: 54.634, lon: 25.285 };
  const waw = { lat: 52.166, lon: 20.967 };
  expect(routeProgress(vno, vno, waw)).toBeCloseTo(0, 5);
  expect(routeProgress(waw, vno, waw)).toBeCloseTo(1, 5);
  expect(routeProgress({ lat: (vno.lat + waw.lat) / 2, lon: (vno.lon + waw.lon) / 2 }, vno, waw)).toBeCloseTo(0.5, 1);
  expect(routeProgress(vno, undefined, waw)).toBe(null);
  expect(routeProgress(vno, vno, vno)).toBe(null);
});
