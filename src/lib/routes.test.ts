// Unit tests for the estimated route's geometry (`flightpath.ts`, which
// `routes.ts` builds on). Run with `bun test src`.
// @ts-ignore -- bun's types aren't a dependency; svelte-check still type-checks this file
import * as bunTest from "bun:test";
import type { Runway } from "./api";
import {
  AIM_M,
  arrivalPath,
  bearing,
  centrelineOffset,
  chooseArrivalRunway,
  departurePath,
  destination,
  distanceM,
  dubins,
  estimateRoute,
  greatCircle,
  runwayEnds,
  trimToPosition,
  type Aircraft,
  type LonLat,
  type PathPoint,
  type RunwayEndInfo,
  type Traffic,
} from "./flightpath";

interface Matchers {
  toBe(value: unknown): void;
  toBeCloseTo(value: number, digits?: number): void;
  toBeGreaterThan(value: number): void;
  toBeGreaterThanOrEqual(value: number): void;
  toBeLessThan(value: number): void;
  toBeLessThanOrEqual(value: number): void;
}
const { test, expect } = bunTest as unknown as {
  test(name: string, fn: () => void): void;
  expect(value: unknown): Matchers;
};

const NM = 1852;
const FT = 0.3048;

/** Copenhagen Kastrup's runways, as OurAirports lists them. */
const EKCH: Runway[] = [
  {
    lengthFt: 11811,
    widthFt: 148,
    surface: "ASP",
    ends: [
      { ident: "04L", lat: 55.592201, lon: 12.603536, elevationFt: 13, displacedFt: 0 },
      { ident: "22R", lat: 55.616543, lon: 12.641174, elevationFt: 14, displacedFt: 1967 },
    ],
  },
  {
    lengthFt: 10827,
    widthFt: 148,
    surface: "ASP",
    ends: [
      { ident: "04R", lat: 55.6031, lon: 12.633, elevationFt: 12, displacedFt: 0 },
      { ident: "22L", lat: 55.6254, lon: 12.6676, elevationFt: 8, displacedFt: 0 },
    ],
  },
  {
    lengthFt: 9186,
    widthFt: 148,
    surface: "ASPH",
    ends: [
      { ident: "12", lat: 55.626293, lon: 12.633331, elevationFt: 13, displacedFt: 2313 },
      { ident: "30", lat: 55.612522, lon: 12.670532, elevationFt: 8, displacedFt: 886 },
    ],
  },
];
const ENDS = runwayEnds(EKCH, 17);
const end = (ident: string) => ENDS.find((e) => e.ident === ident)!;
const CPH: LonLat = [12.656, 55.618];

const ll = (p: PathPoint | LonLat): LonLat => [p[0], p[1]];
const wrap180 = (d: number) => ((((d + 180) % 360) + 360) % 360) - 180;

/** The largest change of direction (degrees) between consecutive legs longer than `minM`. */
function maxKink(path: readonly (PathPoint | LonLat)[], minM = 1): number {
  let worst = 0;
  let previous: number | null = null;
  for (let i = 1; i < path.length; i++) {
    if (distanceM(ll(path[i - 1]), ll(path[i])) < minM) continue;
    const course = bearing(ll(path[i - 1]), ll(path[i]));
    if (previous !== null) worst = Math.max(worst, Math.abs(wrap180(course - previous)));
    previous = course;
  }
  return worst;
}

/** A point on an end's extended centreline, `nm` before the threshold and `offsetM` to the right. */
function onApproach(e: RunwayEndInfo, nm: number, offsetM = 0): LonLat {
  const p = destination(e.threshold, e.heading + 180, nm * NM);
  return offsetM ? destination(p, e.heading + 90, offsetM) : p;
}

const aircraft = (at: LonLat, track: number, altFt: number, speedKt = 220): Aircraft => ({
  lon: at[0],
  lat: at[1],
  track,
  altFt,
  speedKt,
});

test("runway ends: both directions, displaced thresholds, true headings", () => {
  expect(ENDS.length).toBe(6);
  expect(Math.abs(wrap180(end("22L").heading - 221))).toBeLessThan(1);
  expect(Math.abs(wrap180(end("04R").heading - 41))).toBeLessThan(1);
  // 22R's threshold is displaced 1,967 ft along the runway
  expect(distanceM(end("22R").start, end("22R").threshold)).toBeCloseTo(1967 * FT, 0);
  expect(end("22L").elevationFt).toBe(8);
});

test("runway choice: the one the aircraft is lined up with", () => {
  const ac = aircraft(onApproach(end("22L"), 6, 150), 221, 1_900, 160);
  const choice = chooseArrivalRunway(ENDS, ac);
  expect(choice?.end.ident).toBe("22L");
  expect(choice?.reason).toBe("aligned");
  // and its parallel when that is the one it is on
  expect(chooseArrivalRunway(ENDS, aircraft(onApproach(end("22R"), 6), 221, 1_900, 160))?.end.ident).toBe("22R");
});

test("runway choice: the direction other traffic is landing", () => {
  // east of the field heading north: no runway lines up with that
  const ac = aircraft([12.813, 55.618], 22, 4_250);
  const landing22: Traffic = { id: 9, ...latLon(onApproach(end("22L"), 5, 50)), track: 221, alt: 1_600, speed: 150, onGround: false };
  const choice = chooseArrivalRunway(ENDS, ac, [landing22]);
  expect(choice?.end.ident).toBe("22L");
  expect(choice?.reason).toBe("traffic");
  // a departure climbing out on 04L says the flow is 04
  const past04L = destination(end("04L").start, end("04L").heading, end("04L").lengthM + 3_000);
  const departing04: Traffic = { id: 8, ...latLon(past04L), track: 41, alt: 1_500, speed: 180, onGround: false };
  const flow04 = chooseArrivalRunway(ENDS, ac, [departing04]);
  expect(Math.abs(wrap180(flow04!.end.heading - 41))).toBeLessThan(2);
  expect(flow04?.reason).toBe("traffic");
  // the aircraft itself and unrelated traffic count for nothing
  const self: Traffic = { ...landing22, id: 1 };
  expect(chooseArrivalRunway(ENDS, ac, [self], 1)?.reason).toBe("guessed");
  const high: Traffic = { ...landing22, alt: 9_000 };
  expect(chooseArrivalRunway(ENDS, ac, [high])?.reason).toBe("guessed");
});

function latLon(p: LonLat) {
  return { lon: p[0], lat: p[1] };
}

test("runway choice without evidence: a guess, the end reached most directly", () => {
  // straight in from the north-east, but still far out and not yet aligned
  const ac = aircraft(destination(CPH, 41, 60_000), 230, 12_000, 280);
  const choice = chooseArrivalRunway(ENDS, ac);
  expect(choice?.reason).toBe("guessed");
  expect(Math.abs(wrap180(choice!.end.heading - 221))).toBeLessThan(2);
});

test("final approach: on the extended centreline, then the threshold and the aiming point", () => {
  const e = end("22L");
  const path = arrivalPath(aircraft([12.813, 55.618], 22, 4_250), e);
  const last = path[path.length - 1];
  expect(distanceM(ll(last), destination(e.threshold, e.heading, AIM_M))).toBeLessThan(1);
  expect(path.some((p) => distanceM(ll(p), e.threshold) < 1)).toBe(true);
  // the last 5 NM before the threshold lie on the centreline
  // (the downwind leg runs alongside, kilometres off to the side)
  const final = path.filter((p) => {
    const { along, cross } = centrelineOffset(e, ll(p));
    return along > 0 && along <= 5 * NM && Math.abs(cross) < 1_000;
  });
  expect(final.length).toBeGreaterThan(2);
  for (const p of final) expect(Math.abs(centrelineOffset(e, ll(p)).cross)).toBeLessThan(5);
});

test("final approach: heights follow the 3° glide path to the touchdown zone", () => {
  const e = end("22L");
  const path = arrivalPath(aircraft([12.813, 55.618], 22, 4_250), e);
  const ftPerNm = Math.tan((3 * Math.PI) / 180) * (NM / FT); // ≈ 318
  for (const p of path) {
    const { along } = centrelineOffset(e, ll(p));
    if (along < 0 || along > 8 * NM || Math.abs(centrelineOffset(e, ll(p)).cross) > 5) continue;
    const expected = e.elevationFt + ((along + AIM_M) / NM) * ftPerNm;
    expect(Math.abs(p[2] - expected)).toBeLessThan(5);
  }
  // 50 ft over the threshold, on the ground at the aiming point
  const atThreshold = path.find((p) => distanceM(ll(p), e.threshold) < 1)!;
  expect(atThreshold[2] - e.elevationFt).toBeCloseTo(50, 0);
  expect(path[path.length - 1][2]).toBeCloseTo(e.elevationFt, 3);
  // never climbs, and starts at the aircraft's altitude
  expect(path[0][2]).toBe(4_250);
  for (let i = 1; i < path.length; i++) expect(path[i][2]).toBeLessThanOrEqual(path[i - 1][2] + 1e-9);
});

test("an aircraft below the glide path flies level until it meets it", () => {
  const e = end("22L");
  const path = arrivalPath(aircraft(onApproach(e, 14, 3_000), 260, 2_000, 200), e);
  const levelUntil = path.findIndex((p) => p[2] < 2_000 - 1);
  expect(levelUntil).toBeGreaterThan(0);
  // where it starts down it is on the glide path, about 6 NM out
  const { along } = centrelineOffset(e, ll(path[levelUntil]));
  expect(along / NM).toBeGreaterThan(5);
  expect(along / NM).toBeLessThan(7);
});

test("the path leaves along the aircraft's track and turns smoothly", () => {
  const e = end("22L");
  for (const [at, track, alt] of [
    [[12.813, 55.618], 22, 4_250], // downwind, as in the recording
    [[12.893, 55.715], 306, 3_650], // base
    [[12.4, 55.5], 90, 5_000], // from the south-west, passing the field
    [[12.9, 55.55], 200, 6_000], // heading away from the final
  ] as [LonLat, number, number][]) {
    const path = arrivalPath(aircraft(at, track, alt), e);
    expect(Math.abs(wrap180(bearing(ll(path[0]), ll(path[1])) - track))).toBeLessThan(3);
    expect(maxKink(path)).toBeLessThan(6);
  }
});

test("Dubins paths arrive at the pose asked for without kinks", () => {
  let seed = 7;
  const random = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
  for (let i = 0; i < 500; i++) {
    const [x0, y0, h0, h1] = [(random() - 0.5) * 40_000, (random() - 0.5) * 40_000, random() * 360, random() * 360];
    const r = 1_000 + random() * 5_000;
    const points = dubins(x0, y0, h0, 0, 0, h1, r);
    const n = points.length;
    expect(Math.hypot(points[n - 1][0], points[n - 1][1])).toBeLessThan(1e-6);
    const course = (a: number[], b: number[]) => (Math.atan2(b[0] - a[0], b[1] - a[1]) * 180) / Math.PI;
    expect(Math.abs(wrap180(course(points[0], points[1]) - h0))).toBeLessThan(2);
    expect(Math.abs(wrap180(course(points[n - 2], points[n - 1]) - h1))).toBeLessThan(2);
    for (let k = 2; k < n; k++) {
      if (Math.hypot(points[k][0] - points[k - 1][0], points[k][1] - points[k - 1][1]) < 1) continue;
      expect(Math.abs(wrap180(course(points[k - 1], points[k]) - course(points[k - 2], points[k - 1])))).toBeLessThan(3.01);
    }
  }
});

test("far away: the en-route part is the great circle, only the ends are shaped", () => {
  const e = end("22L");
  // over Bergen, heading roughly for Copenhagen
  const from: LonLat = [5.2, 60.3];
  const course = bearing(from, CPH);
  const path = arrivalPath(aircraft(from, course, 36_000, 450), e);
  expect(maxKink(path)).toBeLessThan(6);
  const total = distanceM(from, CPH);
  // the points more than 100 km from both ends lie on one great circle...
  const middle = path.filter((p) => distanceM(from, ll(p)) > 100_000 && distanceM(ll(p), CPH) > 100_000);
  expect(middle.length).toBeGreaterThan(5);
  const circle = greatCircle(ll(middle[0]), ll(middle[middle.length - 1]), 2);
  const offCircle = (p: LonLat) => Math.min(...circle.map((q) => distanceM(p, q)));
  for (const p of middle) expect(offCircle(ll(p))).toBeLessThan(1_100);
  // ...the one from the aircraft, give or take the airport's size
  expect(Math.abs(wrap180(bearing(ll(middle[0]), ll(middle[middle.length - 1])) - bearing(ll(middle[0]), CPH)))).toBeLessThan(2);
  // cruise until the descent: level for the first half
  for (const p of path) if (distanceM(from, ll(p)) < total / 2) expect(p[2]).toBe(36_000);
});

test("far away and turning: joins the great circle smoothly from the current track", () => {
  const from: LonLat = [5.2, 60.3];
  const path = arrivalPath(aircraft(from, 300, 20_000, 400), end("22L"));
  expect(Math.abs(wrap180(bearing(ll(path[0]), ll(path[1])) - 300))).toBeLessThan(3);
  expect(maxKink(path)).toBeLessThan(6);
});

test("departure: rolls on the runway, climbs out straight ahead, then turns onto the first tracked track", () => {
  const e = end("22R");
  const to = aircraft([12.2, 55.4], 250, 9_000, 300);
  const path = departurePath(e, to);
  expect(distanceM(ll(path[0]), e.start)).toBeLessThan(1);
  expect(path[0][2]).toBe(e.elevationFt);
  expect(distanceM(ll(path[path.length - 1]), [to.lon, to.lat])).toBeLessThan(1);
  expect(path[path.length - 1][2]).toBe(9_000);
  // straight ahead on the centreline for the first few kilometres
  for (const p of path) {
    const { along, cross } = centrelineOffset(e, ll(p));
    if (-along < 6_000) expect(Math.abs(cross)).toBeLessThan(5);
  }
  const n = path.length;
  expect(Math.abs(wrap180(bearing(ll(path[n - 2]), ll(path[n - 1])) - 250))).toBeLessThan(3);
  expect(maxKink(path)).toBeLessThan(6);
  for (let i = 1; i < n; i++) expect(path[i][2]).toBeGreaterThanOrEqual(path[i - 1][2]);
});

test("trimming starts the path at the aircraft as it moves along it", () => {
  const path = arrivalPath(aircraft([12.813, 55.618], 22, 4_250), end("22L"));
  const moved = destination(ll(path[0]), 22, 900);
  const trimmed = trimToPosition(path, moved);
  expect(trimmed[0][0]).toBe(moved[0]);
  expect(distanceM(ll(trimmed[1]), ll(path[0]))).toBeGreaterThan(900);
  expect(trimmed.length).toBeLessThan(path.length + 1);
  expect(maxKink(trimmed)).toBeLessThan(6);
});

test("a whole estimate names its runways and stays an estimate", () => {
  const flight = { id: 1, lat: 55.618, lon: 12.813, track: 22, alt: 4_250, speed: 220, onGround: false };
  const estimate = estimateRoute({
    flight,
    destination: { id: 1, name: "Copenhagen", iata: "CPH", icao: "EKCH", city: "", country: "", lat: CPH[1], lon: CPH[0], alt: 17, size: 1, timezone: "" },
    originRunways: [],
    destinationRunways: EKCH,
    traffic: [],
  });
  expect(estimate.kind).toBe("estimated");
  expect(estimate.arrival?.reason).toBe("guessed");
  expect(estimate.ahead.length).toBeGreaterThan(10);
  // without runways it still ends at the airport, without a kink at the aircraft
  const bare = estimateRoute({
    flight,
    destination: { id: 1, name: "Copenhagen", iata: "CPH", icao: "EKCH", city: "", country: "", lat: CPH[1], lon: CPH[0], alt: 17, size: 1, timezone: "" },
    originRunways: [],
    destinationRunways: [],
    traffic: [],
  });
  expect(bare.arrival).toBe(null);
  expect(distanceM(ll(bare.ahead[bare.ahead.length - 1]), CPH)).toBeLessThan(1);
  expect(maxKink(bare.ahead)).toBeLessThan(6);
});
