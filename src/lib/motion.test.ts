// Unit tests for the aircraft motion model. Run with `bun test src`.
// @ts-ignore -- bun's types aren't a dependency; svelte-check still type-checks this file
import * as bunTest from "bun:test";
import type { Airport, LiveFlight } from "./api";
import { Motion, type Measured, type Pose } from "./motion";
import landings from "./testdata/landings.json";

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

const DEG = Math.PI / 180;
const M_PER_DEG = 6_371_000 * DEG;
const KT = 0.514444;
const T0 = 1_760_000_000_000;

/** A point `east`/`north` metres from lat/lon (flat earth, as the model uses). */
function offset(lat: number, lon: number, east: number, north: number): [number, number] {
  return [lat + north / M_PER_DEG, lon + east / (M_PER_DEG * Math.cos(lat * DEG))];
}

/** Metres between two poses or points. */
function metres(a: { lat: number; lon: number }, b: { lat: number; lon: number }): number {
  const x = (b.lon - a.lon) * M_PER_DEG * Math.cos(((a.lat + b.lat) / 2) * DEG);
  return Math.hypot(x, (b.lat - a.lat) * M_PER_DEG);
}

function flight(over: Partial<LiveFlight>): LiveFlight {
  return {
    id: 1,
    lat: 45,
    lon: 10,
    track: 0,
    alt: 35_000,
    speed: 450,
    onGround: false,
    timestampMs: T0,
    callsign: "TEST1",
    flight: "TE1",
    reg: "N1",
    typecode: "A320",
    origin: "",
    destination: "",
    icon: "A320",
    ...over,
  };
}

/** A report at time `t0` whose look-ahead buffer follows `path(seconds) → [east, north]` metres. */
function pathFlight(
  path: (s: number) => [number, number],
  over: Partial<LiveFlight>,
  t0 = T0,
  bufferS = [2, 4, 6, 8],
): LiveFlight {
  const [lat, lon] = offset(45, 10, ...path(0));
  const positions = bufferS.map((s): [number, number, number] => {
    const [la, lo] = offset(45, 10, ...path(s));
    return [Math.round((la - lat) / 1e-5), Math.round((lo - lon) / 1e-5), s * 1000];
  });
  return flight({ lat, lon, timestampMs: t0, positions, ...over });
}

/** Right turn at `rateDeg` deg/s (negative: left) from heading north at `speedKt`. */
function circle(speedKt: number, rateDeg: number) {
  const v = speedKt * KT;
  const w = rateDeg * DEG;
  const r = v / w;
  return (s: number): [number, number] => [r * (1 - Math.cos(w * s)), r * Math.sin(w * s)];
}

/**
 * The pose `f` is drawn with at the moment its drawn time is `T`. The map
 * shows aircraft a few seconds in the past, so tests compare with the truth
 * at the drawn time; the delay changes slowly, so a few fixed-point steps
 * find that moment. `f` must have been taken in already (posed once).
 */
function poseAt(m: Motion, f: LiveFlight, T: number): Pose {
  let now = T;
  for (let i = 0; i < 8; i++) now += T - m.drawnTime(f.id, now);
  return m.pose(f, now);
}

/** Truth `path(seconds)` → [east, north] at the drawn time of `id` at `now`, as a point. */
function truthAt(m: Motion, id: number, now: number, path: (s: number) => [number, number], t0 = T0) {
  const [lat, lon] = offset(45, 10, ...path((m.drawnTime(id, now) - t0) / 1000));
  return { lat, lon };
}

function airport(over: Partial<Airport>): Airport {
  return { id: 1, name: "Test", iata: "TST", icao: "XTST", city: "", country: "", lat: 45, lon: 10, alt: 0, size: 0, timezone: "UTC", ...over };
}

test("straight flight matches dead reckoning, a few seconds in the past", () => {
  const m = new Motion();
  const f = flight({ track: 90, speed: 250 });
  m.pose(f, T0); // taken in as it happens
  // drawn behind real time, by 2-12 s
  const lag = T0 + 30_000 - m.drawnTime(1, T0 + 30_000);
  expect(lag).toBeGreaterThanOrEqual(2_000);
  expect(lag).toBeLessThanOrEqual(12_000);
  const p = poseAt(m, f, T0 + 30_000);
  const [lat, lon] = offset(45, 10, 250 * KT * 30, 0);
  expect(metres(p, { lat, lon })).toBeLessThan(5);
  expect(p.heading).toBeCloseTo(90, 1);
  expect(p.bank).toBeCloseTo(0, 3);

  // and with a buffer along the same line
  const line = (s: number): [number, number] => [0, 250 * KT * s];
  const g = pathFlight(line, { id: 2, track: 0, speed: 250 });
  m.pose(g, T0);
  const q = poseAt(m, g, T0 + 38_000);
  const [lat2, lon2] = offset(45, 10, ...line(38));
  expect(metres(q, { lat: lat2, lon: lon2 })).toBeLessThan(5);
});

test("a constant-rate turn continues along its circle", () => {
  const m = new Motion();
  const path = circle(160, 3);
  const f = pathFlight(path, { speed: 160, alt: 10_000 });
  m.pose(f, T0);
  for (const s of [5, 10, 20, 30, 50]) {
    const p = poseAt(m, f, T0 + s * 1000);
    const [lat, lon] = offset(45, 10, ...path(s));
    expect(metres(p, { lat, lon })).toBeLessThan(25);
  }
  // heading follows the circle: 3 deg/s from north
  expect(poseAt(m, f, T0 + 20_000).heading).toBeCloseTo(60, 0);
});

test("the smoothed path stays on exact samples", () => {
  const m = new Motion();
  const path = circle(220, 2);
  const f = pathFlight(path, { speed: 220, alt: 12_000 });
  m.pose(f, T0);
  // smoothing takes out jitter, not a steady turn: exact (1 m quantised) samples are kept to within a metre or so
  for (const [dLat, dLon, dMs] of [[0, 0, 0] as [number, number, number], ...f.positions!]) {
    const p = poseAt(m, f, T0 + dMs);
    expect(metres(p, { lat: f.lat + dLat * 1e-5, lon: f.lon + dLon * 1e-5 })).toBeLessThan(1.5);
  }
  // and runs smoothly between them, close to the true path
  const p = poseAt(m, f, T0 + 5_000);
  const [lat, lon] = offset(45, 10, ...path(5));
  expect(metres(p, { lat, lon })).toBeLessThan(3);
});

test("a new report blends in without a jump and converges", () => {
  const m = new Motion();
  const speed = 250;
  const v = speed * KT;
  const line = (s: number): [number, number] => [v * s, 0];
  const a = pathFlight(line, { track: 90, speed });
  const frame = 16;
  let prev: Pose = m.pose(a, T0 + 2_000);
  let t = T0 + 2_000;
  for (; t <= T0 + 12_000; t += frame) {
    const p = m.pose(a, t);
    expect(metres(prev, p)).toBeLessThan(v * 0.016 + 0.5);
    prev = p;
  }
  // a report from 10 s, 300 m north of the line
  const off = (s: number): [number, number] => [v * (s + 10), 300];
  const b = pathFlight(off, { track: 90, speed, timestampMs: T0 + 10_000 }, T0 + 10_000);
  const now = t;
  for (; t <= now + 5_000; t += frame) {
    const p = m.pose(b, t);
    // the blend adds at most ~0.8·k·300 m/s (k = 2.2/s) on top of the motion itself
    expect(metres(prev, p)).toBeLessThan((v + 250) * 0.016);
    expect(Math.abs(((p.heading - prev.heading + 540) % 360) - 180)).toBeLessThan(5);
    prev = p;
  }
  // a few seconds later it is on the new data (at the drawn time)
  for (; t <= now + 8_000; t += frame) m.pose(b, t);
  expect(metres(m.pose(b, t), truthAt(m, 1, t, (s) => off(s - 10)))).toBeLessThan(2);
});

test("an old report still moves", () => {
  const m = new Motion();
  const now = T0 + 300_000;
  const f = flight({ track: 45, speed: 400, alt: 30_000 });
  const p = m.pose(f, now);
  const q = m.pose(f, now + 1_000);
  expect(metres(p, q)).toBeGreaterThan(400 * KT * 0.9);
  expect(metres(p, q)).toBeLessThan(400 * KT * 1.1);
  // and holds after ten minutes without data
  const r = poseAt(m, f, T0 + 700_000);
  const s = poseAt(m, f, T0 + 701_000);
  expect(metres(r, s)).toBeLessThan(0.01);
});

test("taxiing aircraft move; parked ones don't", () => {
  const m = new Motion();
  const taxi = flight({ onGround: true, speed: 15, alt: 0, track: 270 });
  const p = m.pose(taxi, T0 + 10_000);
  const q = m.pose(taxi, T0 + 11_000);
  expect(p.phase).toBe("taxi");
  expect(metres(p, q)).toBeGreaterThan(15 * KT * 0.9);
  expect(p.altFt).toBe(0);
  expect(p.heading).toBeCloseTo(270, 1);

  const parked = flight({ id: 2, onGround: true, speed: 2, alt: 0, track: 123 });
  const r = m.pose(parked, T0 + 10_000);
  const s = m.pose(parked, T0 + 60_000);
  expect(r.phase).toBe("parked");
  expect(metres(r, s)).toBeLessThan(0.01);
  expect(r.heading).toBeCloseTo(123, 3);
  expect(r.gear).toBe(1);
});

test("an approach to a high field touches down and rolls out", () => {
  const m = new Motion();
  const den = airport({ iata: "DEN", lat: 39.8617, lon: -104.6731, alt: 5_434 });
  m.setAirports([den, airport({ iata: "ORD", lat: 41.97, lon: -87.9, alt: 672 })]);
  const [lat, lon] = [den.lat - 14 / 111.2, den.lon];
  const f = flight({ lat, lon, track: 0, speed: 140, alt: 5_434 + 800, vspeed: -700, origin: "ORD", destination: "DEN" });
  m.pose(f, T0);
  const at = (T: number) => poseAt(m, f, T);
  expect(at(T0).phase).toBe("approach");
  expect(at(T0).gear).toBe(1);
  expect(at(T0).altFt).toBeCloseTo(800, 0);
  let landed = NaN;
  let last = at(T0);
  for (let T = T0; T <= T0 + 120_000; T += 250) {
    const p = at(T);
    expect(p.altFt).toBeGreaterThanOrEqual(0);
    expect(p.altFt).toBeLessThanOrEqual(last.altFt + 1e-9); // never climbs back up
    if (p.altFt < 200) expect(p.gear).toBe(1);
    if (p.altFt === 0 && Number.isNaN(landed)) landed = T;
    last = p;
  }
  // touchdown predicted ~70 s out (the descent levels off past 60 s)
  expect(landed).toBeGreaterThan(T0 + 60_000);
  expect(landed).toBeLessThan(T0 + 80_000);
  const roll = at(landed + 10_000);
  expect(roll.phase).toBe("landingRoll");
  expect(roll.bank).toBe(0);
  expect(at(landed + 6_000).pitch).toBeCloseTo(0, 3);
  // decelerating: less ground covered each second
  const d1 = metres(at(landed + 2_000), at(landed + 3_000));
  const d2 = metres(at(landed + 20_000), at(landed + 21_000));
  expect(d2).toBeLessThan(d1 - 10);
  // a report from the runway confirms it without lifting the aircraft off the ground
  const onRunway = flight({ ...f, lat: den.lat - 4 / 111.2, alt: 0, speed: 110, onGround: true, vspeed: 0, timestampMs: landed + 5_000 });
  m.pose(onRunway, landed + 6_000);
  const p = poseAt(m, onRunway, landed + 7_000);
  expect(p.phase).toBe("landingRoll");
  expect(p.altFt).toBe(0);
});

test("phases through a takeoff", () => {
  const m = new Motion();
  m.setAirports([airport({ iata: "AAA", alt: 100 }), airport({ iata: "BBB", lat: 50, lon: 20 })]);
  const base = { origin: "AAA", destination: "BBB", track: 0 };
  let t = T0;
  let north = 0;
  // each report taken in as it happens, then looked at once the drawn time reaches it
  const report = (over: Partial<LiveFlight>, dt = 10_000) => {
    t += dt;
    north += ((over.speed ?? 0) * KT * dt) / 1000;
    const [lat, lon] = offset(45, 10, 0, north);
    const f = flight({ ...base, lat, lon, timestampMs: t, ...over });
    m.pose(f, t);
    return poseAt(m, f, t);
  };
  expect(report({ onGround: true, speed: 0, alt: 0 }).phase).toBe("parked");
  expect(report({ onGround: true, speed: 15, alt: 0 }).phase).toBe("taxi");
  expect(report({ onGround: true, speed: 60, alt: 0 }).phase).toBe("takeoffRoll");
  expect(report({ onGround: true, speed: 120, alt: 0 }).phase).toBe("takeoffRoll");
  const climbing = report({ speed: 150, alt: 500 });
  expect(climbing.phase).toBe("initialClimb");
  // drawn above the field, not MSL
  const later = poseAt(m, flight({ ...base, timestampMs: t }), t + 5_000).altFt;
  expect(later).toBeGreaterThan(400);
  expect(later).toBeLessThan(800);
  expect(report({ speed: 250, alt: 5_100 }, 120_000).phase).toBe("climb");
  expect(report({ speed: 450, alt: 35_000, vspeed: 0 }, 600_000).phase).toBe("cruise");
});

test("phases follow the drawn time, not the newest report", () => {
  const m = new Motion();
  m.setAirports([airport({ iata: "AAA", alt: 100 })]);
  const roll = flight({ origin: "AAA", onGround: true, speed: 60, alt: 0, track: 0 });
  m.pose(roll, T0);
  // airborne at T0 + 10 s, taken in at once: the drawn aircraft, seconds behind, is still rolling
  const [lat, lon] = offset(45, 10, 0, 1_000);
  const air = flight({ origin: "AAA", lat, lon, onGround: false, speed: 150, alt: 400, track: 0, timestampMs: T0 + 10_000 });
  m.pose(air, T0 + 10_000);
  expect(m.drawnTime(1, T0 + 10_000)).toBeLessThan(T0 + 10_000);
  const before = poseAt(m, air, T0 + 9_000);
  expect(before.phase).toBe("takeoffRoll");
  expect(before.altFt).toBe(0);
  expect(before.gear).toBe(1);
  const after = poseAt(m, air, T0 + 11_000);
  expect(after.phase).toBe("initialClimb");
  expect(after.altFt).toBeGreaterThan(250);
});

test("a takeoff roll lifts off by itself", () => {
  const m = new Motion();
  m.setAirports([airport({ iata: "AAA", alt: 100 })]);
  const f = flight({ origin: "AAA", onGround: true, speed: 120, alt: 0 });
  m.pose(f, T0);
  const at = (T: number) => poseAt(m, f, T);
  expect(at(T0).phase).toBe("takeoffRoll");
  // 25 kt to go at 2.5 kt/s: rotation starts 3 s before lift-off at 10 s
  expect(at(T0 + 6_000).pitch).toBe(0);
  const rotating = at(T0 + 9_000).pitch;
  expect(rotating).toBeGreaterThan(0);
  expect(at(T0 + 9_900).altFt).toBe(0);
  const airborne = at(T0 + 20_000);
  expect(airborne.phase).toBe("initialClimb");
  // 10 s at 1800 fpm, less the ~33 ft the lift-off's curve (the flare's, mirrored) takes
  expect(airborne.altFt).toBeCloseTo(300 - 100 / 3, 0);
  expect(airborne.pitch).toBeGreaterThan(rotating);
  expect(airborne.gear).toBe(1); // still below 400 ft
});

test("phases through a landing", () => {
  const m = new Motion();
  m.setAirports([airport({ iata: "AAA", lat: 50, lon: 20 }), airport({ iata: "BBB", alt: 300 })]);
  const base = { origin: "AAA", destination: "BBB", track: 180 };
  let t = T0;
  const report = (over: Partial<LiveFlight>, km: number) => {
    t += 10_000;
    const [lat, lon] = offset(45, 10, 0, km * 1000);
    const f = flight({ ...base, lat, lon, timestampMs: t, ...over });
    m.pose(f, t);
    return poseAt(m, f, t);
  };
  expect(report({ speed: 450, alt: 36_000, vspeed: 0 }, 300).phase).toBe("cruise");
  expect(report({ speed: 300, alt: 20_000, vspeed: -1_500 }, 120).phase).toBe("descent");
  const approach = report({ speed: 160, alt: 2_300, vspeed: -800 }, 15);
  expect(approach.phase).toBe("approach");
  expect(approach.altFt).toBeCloseTo(2_000, 0);
  expect(report({ speed: 140, alt: 340, vspeed: -600 }, 1).phase).toBe("flare");
  expect(report({ speed: 120, alt: 0, onGround: true, vspeed: 0 }, 0).phase).toBe("landingRoll");
  expect(report({ speed: 20, alt: 0, onGround: true, vspeed: 0 }, -1).phase).toBe("taxi");
  expect(report({ speed: 0, alt: 0, onGround: true, vspeed: 0 }, -1.5).phase).toBe("parked");
});

test("bank follows the turn; pitch follows the climb", () => {
  const m = new Motion();
  const r = pathFlight(circle(250, 2), { id: 1, speed: 250, alt: 20_000 });
  const l = pathFlight(circle(250, -2), { id: 2, speed: 250, alt: 20_000 });
  m.pose(r, T0);
  m.pose(l, T0);
  const right = poseAt(m, r, T0 + 12_000);
  const left = poseAt(m, l, T0 + 12_000);
  // rate-two turn at 250 kt: atan(v ω / g) ≈ 24°
  expect(right.bank).toBeGreaterThan(20);
  expect(right.bank).toBeLessThan(28);
  expect(left.bank).toBeLessThan(-20);
  expect(left.bank).toBeGreaterThan(-28);
  // inside the samples too, from the smoothed path's own turn
  expect(poseAt(m, r, T0 + 4_000).bank).toBeGreaterThan(20);
  // bank fades out as the predicted turn ends
  expect(poseAt(m, r, T0 + 75_000).bank).toBe(0);

  const pitch = (vspeed: number, id: number) => {
    const f = flight({ id, alt: 20_000, speed: 300, vspeed });
    m.pose(f, T0);
    return poseAt(m, f, T0 + 5_000);
  };
  const climb = pitch(2_000, 10);
  const cruise = pitch(0, 11);
  const descent = pitch(-2_000, 12);
  expect(climb.phase).toBe("climb");
  expect(cruise.phase).toBe("cruise");
  expect(descent.phase).toBe("descent");
  expect(climb.pitch).toBeGreaterThan(cruise.pitch);
  expect(cruise.pitch).toBeGreaterThan(descent.pitch);
});

test("gear animates when the target changes", () => {
  const m = new Motion();
  const field = airport({ iata: "BBB", alt: 0 });
  m.setAirports([field]);
  const [lat, lon] = offset(45, 10, 0, -20_000);
  // 2500 ft on approach at 1000 fpm: gear goes down passing 2000 ft, 30 s on (drawn time)
  const f = flight({ lat, lon, destination: "BBB", speed: 150, alt: 2_500, vspeed: -1_000 });
  m.pose(f, T0);
  expect(poseAt(m, f, T0).gear).toBe(0);
  for (let T = T0; T <= T0 + 31_000; T += 1_000) poseAt(m, f, T);
  const moving = poseAt(m, f, T0 + 32_000).gear;
  expect(moving).toBeGreaterThan(0);
  expect(moving).toBeLessThan(1);
  expect(poseAt(m, f, T0 + 40_000).gear).toBe(1);
});

test("syncClock offsets now()", () => {
  const m = new Motion();
  expect(Math.abs(m.now() - Date.now())).toBeLessThan(50);
  m.syncClock(Date.now() + 60_000);
  expect(Math.abs(m.now() - Date.now() - 60_000)).toBeLessThan(50);
  m.syncClock(Number.NaN);
  m.syncClock(5);
  expect(Math.abs(m.now() - Date.now() - 60_000)).toBeLessThan(50);
  // small differences are smoothed, not taken outright
  m.syncClock(Date.now() + 61_000);
  const offset = m.now() - Date.now();
  expect(offset).toBeGreaterThan(60_000 + 50);
  expect(offset).toBeLessThan(61_000 - 50);
});

test("trail heights match the model's", () => {
  const m = new Motion();
  m.setAirports([airport({ iata: "DEN", alt: 5_000 })]);
  const f = flight({ destination: "DEN", alt: 7_000, speed: 160, vspeed: -800, track: 0 });
  m.pose(f, T0);
  const p = poseAt(m, f, T0);
  expect(m.drawAltitude(1, 7_000, f.lat, f.lon)).toBeCloseTo(p.altFt, 3);
  expect(m.drawAltitude(1, 0, f.lat, f.lon)).toBe(0);
  // far away, plain MSL
  const [lat, lon] = offset(45, 10, 0, 80_000);
  expect(m.drawAltitude(1, 7_000, lat, lon)).toBe(7_000);
  expect(m.drawAltitude(999, 7_000, lat, lon)).toBe(7_000);
});

test("prune drops stale tracks", () => {
  const m = new Motion();
  const f = flight({});
  m.pose(f, T0);
  m.prune(T0 + 16 * 60_000);
  // a re-created track starts from scratch: no blending from the old one
  const g = flight({ lat: 46, timestampMs: T0 + 16 * 60_000 });
  m.pose(g, T0 + 16 * 60_000);
  expect(poseAt(m, g, T0 + 16 * 60_000).lat).toBeCloseTo(46, 6);
});

// --- Measured data (adsb.lol) for the selected aircraft -------------------------------------

/** A measured state at `t` for a flight following `path`, east/north metres from 45N 10E. */
function measuredAt(path: (s: number) => [number, number], t: number, over: Partial<Measured> = {}): Measured {
  const [lat, lon] = offset(45, 10, ...path((t - T0) / 1000));
  return { positionMs: t, seenMs: t, lat, lon, onGround: false, ...over };
}

/** Largest per-frame change of `get` between `from` and `to`, at 16 ms frames, measuring each frame. */
function maxStep(m: Motion, f: LiveFlight, from: number, to: number, get: (p: Pose) => number): number {
  let prev = get(m.pose(f, from));
  let worst = 0;
  for (let t = from + 16; t <= to; t += 16) {
    const v = get(m.pose(f, t));
    worst = Math.max(worst, Math.abs(((v - prev + 540) % 360) - 180));
    prev = v;
  }
  return worst;
}

const north = (speedKt: number) => (s: number): [number, number] => [0, speedKt * KT * s];
const east = (speedKt: number) => (s: number): [number, number] => [speedKt * KT * s, 0];

test("measured roll drives the bank smoothly", () => {
  const m = new Motion();
  const path = north(250);
  const f = pathFlight(path, { speed: 250, alt: 10_000 });
  m.pose(f, T0 + 1_000);
  const t = T0 + 3_000;
  expect(Math.abs(m.pose(f, t).bank)).toBeLessThan(0.5);
  m.measure(1, measuredAt(path, t, { gs: 250, track: 0, trueHeading: 0, roll: 25, trackRate: 2 }), t);
  // no jump on arrival, and only a gentle change per frame while it blends in
  // (a 25° step eases in at up to ~20°/s, 0.33° a frame)
  expect(maxStep(m, f, t, t + 6_000, (p) => p.bank)).toBeLessThan(0.4);
  expect(maxStep(m, f, t, t + 6_000, (p) => p.heading)).toBeLessThan(0.3);
  const p = poseAt(m, f, t + 1_000);
  expect(p.bank).toBeGreaterThan(22);
  expect(p.bank).toBeLessThan(28);
  // and past the data the path turns right, as the measured track rate says
  expect(poseAt(m, f, T0 + 15_000).track).toBeGreaterThan(5);
});

test("a measured true heading off the track shows the crab", () => {
  const m = new Motion();
  const path = east(300);
  const f = pathFlight(path, { speed: 300, alt: 20_000, track: 90 });
  const t = T0 + 9_000;
  m.pose(f, t);
  m.measure(1, measuredAt(path, t, { gs: 300, track: 90, trueHeading: 100, roll: 0, trackRate: 0 }), t);
  const p = m.pose(f, t + 8_000);
  expect(p.heading).toBeCloseTo(100, 0); // the nose points into the wind
  expect(p.track).toBeCloseTo(90, 0); // the path still follows the track
  expect(p.bank).toBeCloseTo(0, 1);
  expect(metres(p, truthAt(m, 1, t + 8_000, path))).toBeLessThan(10);
});

test("a climb levels off at the selected altitude", () => {
  const path = north(280);
  const run = (navAltitudeMcp?: number) => {
    const m = new Motion();
    const f = pathFlight(path, { speed: 280, alt: 30_000 });
    const t = T0 + 8_000;
    m.pose(f, t);
    m.measure(1, measuredAt(path, t, { gs: 280, track: 0, altBaro: 30_000, baroRate: 2_000, navAltitudeMcp }), t);
    return { m, f, t };
  };
  const free = run();
  expect(poseAt(free.m, free.f, free.t + 90_000).altFt).toBeGreaterThan(32_300);

  const { m, f, t } = run(32_000);
  let prev = poseAt(m, f, t).altFt;
  let prevRate = 0;
  for (let s = t + 1_000; s <= t + 120_000; s += 1_000) {
    const alt = poseAt(m, f, s).altFt;
    expect(alt).toBeLessThanOrEqual(32_000.5);
    expect(alt).toBeGreaterThanOrEqual(prev - 0.5); // never sinks back
    const rate = alt - prev;
    if (s > t + 8_000) expect(Math.abs(rate - prevRate)).toBeLessThan(6); // the rate eases off, no kink
    prev = alt;
    prevRate = rate;
  }
  expect(poseAt(m, f, t + 90_000).altFt).toBeCloseTo(32_000, 0);
  // at the selected level the attitude is level flight's, not the climb's
  expect(poseAt(m, f, t + 90_000).pitch).toBeLessThan(4);
});

test("a turn on the autopilot stops at the selected heading", () => {
  const m = new Motion();
  const path = circle(160, 3);
  const f = pathFlight(path, { speed: 160, alt: 10_000 });
  const t = T0 + 8_000;
  m.pose(f, t);
  const bankNow = Math.atan2(160 * KT * 3 * DEG, 9.80665) / DEG;
  // selected 088 magnetic with 2° east variation: 090 true
  m.measure(
    1,
    measuredAt(path, t, { gs: 160, track: 24, trueHeading: 24, magHeading: 22, roll: bankNow, trackRate: 3, navHeading: 88 }),
    t,
  );
  expect(maxStep(m, f, t, t + 60_000, (p) => p.bank)).toBeLessThan(0.3);
  const end = poseAt(m, f, t + 40_000);
  expect(end.track).toBeCloseTo(90, 0);
  expect(end.heading).toBeCloseTo(90, 0);
  expect(Math.abs(end.bank)).toBeLessThan(0.5);
  expect(poseAt(m, f, t + 60_000).track).toBeCloseTo(90, 0);

  // with LNAV engaged the selected heading doesn't steer: the turn carries on
  const lnav = new Motion();
  lnav.pose(f, t);
  lnav.measure(
    1,
    measuredAt(path, t, { gs: 160, track: 24, trueHeading: 24, magHeading: 22, trackRate: 3, navHeading: 88, navModes: ["lnav"] }),
    t,
  );
  expect(poseAt(lnav, f, t + 40_000).track).toBeGreaterThan(130);
});

test("a stale measured source falls back to the estimates smoothly", () => {
  const m = new Motion();
  const path = east(300);
  const f = pathFlight(path, { speed: 300, alt: 20_000, track: 90 });
  const t = T0 + 9_000;
  m.pose(f, t);
  m.measure(1, measuredAt(path, t, { gs: 300, track: 90, trueHeading: 98, roll: 12, trackRate: 0 }), t);
  expect(poseAt(m, f, t + 10_000).heading).toBeCloseTo(98, 0);
  expect(poseAt(m, f, t + 10_000).bank).toBeCloseTo(12, 0);
  // nothing more arrives: between 15 and 20 s (drawn time) the measured values ease out
  expect(maxStep(m, f, t + 10_000, t + 40_000, (p) => p.heading)).toBeLessThan(0.06);
  expect(maxStep(m, f, t + 10_000, t + 40_000, (p) => p.bank)).toBeLessThan(0.1);
  const late = poseAt(m, f, t + 25_000);
  expect(late.heading).toBeCloseTo(late.track, 3);
  expect(late.bank).toBeCloseTo(0, 3);

  // dropping the source (deselected) eases out too
  const n = new Motion();
  n.pose(f, t);
  n.measure(1, measuredAt(path, t, { gs: 300, track: 90, trueHeading: 98, roll: 12, trackRate: 0 }), t);
  n.pose(f, t + 5_000);
  n.measure(1, null, t + 5_000);
  expect(maxStep(n, f, t + 5_000, t + 12_000, (p) => p.heading)).toBeLessThan(0.3);
  expect(n.pose(f, t + 12_000).heading).toBeCloseTo(90, 1);
});

test("measured attitude is shown at the drawn time, not on arrival", () => {
  const m = new Motion();
  const path = north(250);
  // no look-ahead: drawn ~10 s behind at first
  const f = flight({ speed: 250, track: 0, alt: 10_000 });
  m.pose(f, T0);
  // adsb.lol every 3 s (T0, T0 + 3 s, ...), half a second old: wings level and nose on
  // track, then from T0 + 12 s a 20° bank (a slip: no turn) with the nose 5° right
  const state = (t: number) =>
    measuredAt(path, t, {
      gs: 250,
      track: 0,
      trackRate: 0,
      roll: t >= T0 + 10_000 ? 20 : 0,
      trueHeading: t >= T0 + 10_000 ? 5 : 0,
    });
  let a = 0;
  let early = 0;
  let lateChecks = 0;
  for (let t = T0; t <= T0 + 40_000; t += 16) {
    if (t >= T0 + 500 + a * 3_000) m.measure(1, state(T0 + a++ * 3_000), t);
    const p = m.pose(f, t);
    const T = m.drawnTime(1, t);
    const crab = ((p.heading - p.track + 540) % 360) - 180;
    if (T <= T0 + 9_000 && t > T0 + 1_000) {
      // already in hand (from T0 + 12.5 s) but not drawn yet: between the T0 + 9 s and
      // T0 + 12 s samples it eases over, never before
      if (t > T0 + 12_500) early++;
      expect(Math.abs(p.bank)).toBeLessThan(0.5);
      expect(Math.abs(crab)).toBeLessThan(0.1);
    }
    if (T >= T0 + 12_000) {
      lateChecks++;
      expect(Math.abs(p.bank - 20)).toBeLessThan(1);
      expect(Math.abs(crab - 5)).toBeLessThan(0.3);
    }
  }
  expect(early).toBeGreaterThan(100);
  expect(lateChecks).toBeGreaterThan(100);
});

test("interleaved Flightradar24 and measured samples stay continuous", () => {
  const m = new Motion();
  const v = 450 * KT;
  const path = east(450);
  // Flightradar24: a report every 8 s with 8 s of look-ahead, arriving 12 s after its time
  const fr24At = (k: number) =>
    pathFlight((s) => path(s + k * 8), { speed: 450, alt: 36_000, track: 90 }, T0 + k * 8_000);
  // adsb.lol: every 3 s, 0.5 s old on arrival, a few metres of noise and a little clock offset
  const noise = (k: number) => ((k * 7919) % 21) - 10;
  let report = fr24At(0);
  let k = 0;
  let a = 0;
  const start = T0 + 12_000;
  let prev = m.pose(report, start);
  let worstStep = 0;
  let worstError = 0;
  for (let t = start + 16; t <= start + 90_000; t += 16) {
    if (t >= T0 + (k + 1) * 8_000 + 12_000) report = fr24At(++k);
    if (t >= start + a * 3_000) {
      const at = start + a * 3_000 - 500 + 150;
      const [e, n] = path((at - 150 - T0) / 1000);
      const [lat, lon] = offset(45, 10, e + noise(a), n + noise(a + 3));
      m.measure(1, { positionMs: at, seenMs: at, lat, lon, onGround: false, gs: 450, track: 90, trueHeading: 90, roll: 0, trackRate: 0, altBaro: 36_000, baroRate: 0 }, t);
      a++;
    }
    const p = m.pose(report, t);
    const step = metres(prev, p);
    // moving forward each frame, at about the right speed (the drawn time runs at 90-110% while the delay eases)
    expect(p.lon).toBeGreaterThan(prev.lon);
    worstStep = Math.max(worstStep, Math.abs(step - v * 0.016));
    worstError = Math.max(worstError, metres(p, truthAt(m, 1, t, path)));
    prev = p;
  }
  expect(worstStep).toBeLessThan(1.5);
  expect(worstError).toBeLessThan(80);
});

// --- Delayed, smoothed playback ------------------------------------------------------------

/** A small deterministic generator, uniform in [-1, 1]. */
function noiseSource(seed: number) {
  let s = seed;
  return () => ((s = (s * 16807) % 2147483647) / 2147483647) * 2 - 1;
}

interface Flown {
  /** RMS and largest deviation of the drawn acceleration's size from the truth's, m/s². */
  rmsAccel: number;
  maxAccel: number;
  /** Largest change of the drawn velocity from one 16 ms frame to the next, beyond the truth's, m/s. */
  maxVelocityStep: number;
  /** Largest turn of the drawn heading, deg/s. */
  maxHeadingRate: number;
  /** RMS and largest distance from the noise-free truth at the drawn time, m. */
  rmsError: number;
  maxError: number;
  /** Largest height error at the drawn time, ft, and largest change of vertical speed per frame, ft/s. */
  maxAltError: number;
  maxClimbStep: number;
}

/**
 * Flies Flightradar24-like reports through 16 ms frames for two minutes and
 * measures the drawn path: a report every 8 s with 8 s of look-ahead (2 s
 * apart), each arriving 4 s after its time, at 250 kt turning right at
 * `turnDeg` deg/s and climbing 600 fpm; positions off by up to ±`noise` m
 * and altitudes by ±`altNoise` ft, in 25 ft steps.
 */
function fly(turnDeg: number, noise: number, altNoise: number): Flown {
  const m = new Motion();
  const rnd = noiseSource(7);
  const v = 250 * KT;
  const w = turnDeg * DEG;
  const truth = (s: number): [number, number] =>
    w === 0 ? [v * s, 0] : [(v / w) * Math.sin(w * s), -(v / w) * (1 - Math.cos(w * s))];
  const accelOf = (s: number): [number, number] => [-v * w * Math.sin(w * s), -v * w * Math.cos(w * s)];
  const altAt = (s: number) => 20_000 + 10 * s;
  const report = (k: number): LiveFlight => {
    const s0 = k * 8;
    const [e0, n0] = truth(s0);
    const [lat, lon] = offset(45, 10, e0 + noise * rnd(), n0 + noise * rnd());
    const positions = [2, 4, 6, 8].map((d): [number, number, number] => {
      const [e, n] = truth(s0 + d);
      const [la, lo] = offset(45, 10, e + noise * rnd(), n + noise * rnd());
      return [Math.round((la - lat) / 1e-5), Math.round((lo - lon) / 1e-5), d * 1000];
    });
    const alt = Math.round((altAt(s0) + altNoise * rnd()) / 25) * 25;
    return flight({ lat, lon, track: 90 + turnDeg * s0, alt, speed: 250, timestampMs: T0 + s0 * 1000, positions });
  };
  const dt = 0.016;
  let k = 0;
  let f = report(0);
  const take = (t: number) => {
    while (T0 + (k + 1) * 8_000 + 4_000 <= t) f = report(++k);
  };
  const start = T0 + 24_000;
  for (let t = T0 + 4_000; t < start; t += 16) {
    take(t);
    m.pose(f, t);
  }
  const toEN = (p: { lat: number; lon: number }): [number, number] => [
    (p.lon - 10) * M_PER_DEG * Math.cos(45 * DEG),
    (p.lat - 45) * M_PER_DEG,
  ];
  let prev: [number, number] | null = null;
  let prevV: [number, number] | null = null;
  let prevHeading = NaN;
  let prevAlt = NaN;
  let prevClimb = NaN;
  const out: Flown = { rmsAccel: 0, maxAccel: 0, maxVelocityStep: 0, maxHeadingRate: 0, rmsError: 0, maxError: 0, maxAltError: 0, maxClimbStep: 0 };
  let n = 0;
  let sumA = 0;
  let sumE = 0;
  for (let t = start; t < start + 120_000; t += 16) {
    take(t);
    const p = m.pose(f, t);
    const s = (m.drawnTime(1, t) - T0) / 1000;
    const pos = toEN(p);
    const [te, tn] = truth(s);
    const err = Math.hypot(pos[0] - te, pos[1] - tn);
    out.maxError = Math.max(out.maxError, err);
    sumE += err * err;
    out.maxAltError = Math.max(out.maxAltError, Math.abs(p.altFt - altAt(s)));
    if (prev) {
      const vel: [number, number] = [(pos[0] - prev[0]) / dt, (pos[1] - prev[1]) / dt];
      if (prevV) {
        const [ax, ay] = accelOf(s);
        const dvx = vel[0] - prevV[0] - ax * dt;
        const dvy = vel[1] - prevV[1] - ay * dt;
        out.maxVelocityStep = Math.max(out.maxVelocityStep, Math.hypot(dvx, dvy));
        const dev = Math.abs(Math.hypot(vel[0] - prevV[0], vel[1] - prevV[1]) / dt - v * Math.abs(w));
        out.maxAccel = Math.max(out.maxAccel, dev);
        sumA += dev * dev;
        n++;
      }
      prevV = vel;
    }
    prev = pos;
    if (!Number.isNaN(prevHeading))
      out.maxHeadingRate = Math.max(out.maxHeadingRate, Math.abs(((p.heading - prevHeading + 540) % 360) - 180) / dt);
    prevHeading = p.heading;
    if (!Number.isNaN(prevAlt)) {
      const climb = (p.altFt - prevAlt) / dt;
      if (!Number.isNaN(prevClimb)) out.maxClimbStep = Math.max(out.maxClimbStep, Math.abs(climb - prevClimb));
      prevClimb = climb;
    }
    prevAlt = p.altFt;
  }
  out.rmsAccel = Math.sqrt(sumA / n);
  out.rmsError = Math.sqrt(sumE / n);
  return out;
}

test("noisy positions are smoothed: no sample-rate wiggle, close to the truth", () => {
  // Drawn at "now" through each sample, the same data gave RMS accelerations of
  // ~58 m/s² (peaks ~1600), heading rates of 23-30°/s and errors up to 110 m.
  for (const turn of [0, 3]) {
    const r = fly(turn, 30, 25);
    expect(r.rmsAccel).toBeLessThan(5);
    expect(r.maxAccel).toBeLessThan(30);
    expect(r.maxHeadingRate).toBeLessThan(turn + 4);
    expect(r.rmsError).toBeLessThan(15);
    expect(r.maxError).toBeLessThan(35);
    expect(r.maxAltError).toBeLessThan(40);
    expect(r.maxClimbStep).toBeLessThan(1);
  }
  // and exact data is followed exactly
  for (const turn of [0, 3]) {
    const r = fly(turn, 0, 0);
    expect(r.maxError).toBeLessThan(3);
    expect(r.rmsAccel).toBeLessThan(0.5);
    expect(r.maxAccel).toBeLessThan(3);
    expect(r.maxHeadingRate).toBeLessThan(turn + 0.5);
  }
});

test("a new sample causes no step in position or velocity", () => {
  // Flightradar24 and adsb.lol together, the sources a few metres and a little clock apart
  const m = new Motion();
  const v = 300 * KT;
  const path = east(300);
  const fr24At = (k: number) => pathFlight((s) => path(s + k * 8), { speed: 300, alt: 30_000, track: 90 }, T0 + k * 8_000);
  const noise = (k: number) => ((k * 7919) % 21) - 10;
  let report = fr24At(0);
  let k = 0;
  let a = 0;
  const start = T0 + 6_000;
  const toE = (p: Pose) => (p.lon - 10) * M_PER_DEG * Math.cos(45 * DEG);
  const toN = (p: Pose) => (p.lat - 45) * M_PER_DEG;
  let prev = m.pose(report, start);
  let prevVE = NaN;
  let prevVN = NaN;
  let worstPosition = 0;
  let worstVelocity = 0;
  let arrivals = 0;
  for (let t = start + 16; t <= start + 60_000; t += 16) {
    let arrived = false;
    if (t >= T0 + (k + 1) * 8_000 + 6_000) {
      report = fr24At(++k);
      arrived = true;
    }
    if (t >= start + a * 3_000) {
      const at = start + a * 3_000 - 800 + 100;
      const [e, n] = path((at - 100 - T0) / 1000);
      const [lat, lon] = offset(45, 10, e + noise(a), n + noise(a + 5));
      m.measure(1, { positionMs: at, seenMs: at, lat, lon, onGround: false, gs: 300, track: 90, altBaro: 30_000 }, t);
      a++;
      arrived = true;
    }
    const p = m.pose(report, t);
    const vE = (toE(p) - toE(prev)) / 0.016;
    const vN = (toN(p) - toN(prev)) / 0.016;
    // the step from the frame before: the speed's worth, give or take the playback rate
    worstPosition = Math.max(worstPosition, Math.abs(Math.hypot(vE, vN) * 0.016 - v * 0.016));
    if (!Number.isNaN(prevVE)) {
      const dv = Math.hypot(vE - prevVE, vN - prevVN);
      worstVelocity = Math.max(worstVelocity, dv);
      if (arrived) arrivals++;
    }
    prevVE = vE;
    prevVN = vN;
    prev = p;
  }
  expect(arrivals).toBeGreaterThan(20);
  // within 10% of the speed per frame (the drawn time runs at 90-110% while the delay eases)
  expect(worstPosition).toBeLessThan(v * 0.016 * 0.12);
  // velocity changes by under 0.1 m/s a frame (6 m/s²), arrivals included; drawn through
  // each sample at "now", the same data gave 16 m/s in a frame (~1000 m/s²)
  expect(worstVelocity).toBeLessThan(0.1);
});

test("the delay follows the data's cadence, within 2-12 s, and eases", () => {
  const m = new Motion();
  const path = east(250);
  // first: reports every 8 s with 8 s of look-ahead, arriving 4 s late
  // (the look-ahead reaches 4 s past now: 8 + 2 - 4 = 6 s behind)
  const dense = (k: number) => pathFlight((s) => path(s + k * 8), { speed: 250, track: 90 }, T0 + k * 8_000);
  // then: a bare report every 20 s, arriving 2 s late (20 + 3 + 2: the 12 s cap)
  const sparse = (t: number) => {
    const [lat, lon] = offset(45, 10, ...path((t - T0) / 1000));
    return flight({ lat, lon, speed: 250, track: 90, timestampMs: t });
  };
  let f = dense(0);
  let next = T0 + 4_000;
  let k = 0;
  let prevT = -Infinity;
  let prevDelay = NaN;
  let worstRate = 0;
  let denseDelay = NaN;
  for (let t = T0 + 4_000; t <= T0 + 240_000; t += 16) {
    if (t >= next) {
      if (t < T0 + 100_000) {
        f = dense(k++);
        next = T0 + k * 8_000 + 4_000;
      } else {
        const at = Math.floor((t - T0) / 20_000) * 20_000 + T0;
        f = sparse(at);
        next = at + 22_000;
      }
    }
    m.pose(f, t);
    const T = m.drawnTime(1, t);
    const delay = t - T;
    expect(delay).toBeGreaterThanOrEqual(2_000 - 1e-6);
    expect(delay).toBeLessThanOrEqual(12_000 + 1e-6);
    expect(T).toBeGreaterThan(prevT); // never runs backwards
    if (!Number.isNaN(prevDelay)) worstRate = Math.max(worstRate, Math.abs(delay - prevDelay) / 16);
    prevDelay = delay;
    prevT = T;
    if (t <= T0 + 100_000) denseDelay = delay;
  }
  expect(denseDelay).toBeGreaterThan(5_000);
  expect(denseDelay).toBeLessThan(7_000);
  expect(prevDelay).toBeGreaterThan(11_000);
  // eased: never faster than ~0.15 s per second (the drawn time runs at 85-115% at most)
  expect(worstRate).toBeLessThan(0.16);
});

test("stale data never freezes, and fresh data is caught up smoothly", () => {
  const m = new Motion();
  const v = 250 * KT;
  // east at 250 kt, and from 30 s on drifting north at 2 m/s, which the extrapolation can't know
  const truth = (s: number): [number, number] => [v * s, s > 30 ? 2 * (s - 30) : 0];
  const at = (k: number) => pathFlight((s) => truth(s + k * 8), { speed: 250, track: 90 }, T0 + k * 8_000);
  let f = at(0);
  let k = 0;
  let prev = m.pose(f, T0 + 4_000);
  let prevStep = NaN;
  let worstGap = Infinity;
  let worstGapStep = 0;
  let worstResume = 0;
  let worstStepChange = 0;
  for (let t = T0 + 4_016; t <= T0 + 150_000; t += 16) {
    // reports up to 30 s, then nothing until 90 s
    if (t >= T0 + (k + 1) * 8_000 + 4_000 && !(t > T0 + 30_000 && t < T0 + 90_000)) f = at(++k);
    if (t > T0 + 30_000 && t < T0 + 90_000) k = Math.floor((t - T0 - 4_000) / 8_000);
    const p = m.pose(f, t);
    const step = metres(prev, p);
    if (t > T0 + 40_000 && t < T0 + 90_000) {
      worstGap = Math.min(worstGap, step);
      worstGapStep = Math.max(worstGapStep, step);
    }
    if (t >= T0 + 90_000 && t < T0 + 100_000) worstResume = Math.max(worstResume, step);
    if (!Number.isNaN(prevStep)) worstStepChange = Math.max(worstStepChange, Math.abs(step - prevStep));
    prevStep = step;
    prev = p;
  }
  // moving all through the gap, at its speed
  expect(worstGap).toBeGreaterThan(v * 0.016 * 0.85);
  expect(worstGapStep).toBeLessThan(v * 0.016 * 1.15);
  // ~120 m of drift to catch up: a smooth slide, no jump
  expect(worstResume).toBeLessThan(v * 0.016 + 3);
  expect(worstStepChange).toBeLessThan(0.1);
  // and back on the data
  expect(metres(prev, truthAt(m, 1, T0 + 150_000, truth))).toBeLessThan(2);
});

// --- Touchdowns and lift-offs ----------------------------------------------------------------------

/** What the drawn aircraft did, frame by frame (16 ms), while reports arrived `latency` ms after their time. */
interface Frame {
  drawn: number;
  altFt: number;
  gear: number;
  phase: string;
}

function play(m: Motion, reports: LiveFlight[], latency = 3_000, after = 10_000): Frame[] {
  const frames: Frame[] = [];
  let k = 0;
  const end = reports[reports.length - 1].timestampMs + latency + after;
  for (let now = reports[0].timestampMs + latency; now <= end; now += 16) {
    while (k + 1 < reports.length && reports[k + 1].timestampMs + latency <= now) k++;
    const p = m.pose(reports[k], now);
    frames.push({ drawn: m.drawnTime(reports[k].id, now), altFt: p.altFt, gear: p.gear, phase: p.phase });
  }
  return frames;
}

/** Largest change of the drawn height between frames, ft. */
const worstStep = (frames: Frame[]) => frames.reduce((w, f, i) => (i ? Math.max(w, Math.abs(f.altFt - frames[i - 1].altFt)) : w), 0);

/** Drawn time at which the aircraft first comes within a foot of the ground after `from` (landing), or last leaves it (take-off). */
const firstDown = (frames: Frame[], from = -Infinity) => frames.find((f) => f.drawn > from && f.altFt < 1)?.drawn ?? NaN;
const lastUp = (frames: Frame[]) => {
  for (let i = frames.length - 1; i > 0; i--) if (frames[i - 1].altFt < 1 && frames[i].altFt >= 1) return frames[i].drawn;
  return NaN;
};

/** 4000 fpm at 60 fps: the most the drawn height may move in a frame (a firm flare's worth, not a jump). */
const MAX_FRAME_FT = (4_000 / 60) * 0.016;

// Real reports (Flightradar24, 2026-10-09): see testdata/landings.json. `alt` is pressure altitude:
// that day Schiphol's QNH was 1004 hPa (runways read ~225 ft for -11 ft), Heathrow's 1008 (175 ft
// for 83), Atlanta's 1020 (750-775 ft for 1026), and Flightradar24's ground flag came 10-30 s
// after touchdown, or (take-offs) went airborne 10-20 s before lift-off.
type FixtureFlight = (typeof landings.flights)[number];
const fixtureAirports = landings.airports.map((a, i) => airport({ id: i + 1, ...a }));
function fixtureReports(f: FixtureFlight): LiveFlight[] {
  return f.reports.map((r) => {
    const [timestampMs, lat, lon, alt, speed, track, ground, positions] = r as [
      number, number, number, number, number, number, number, [number, number, number][],
    ];
    return flight({
      id: f.id, lat, lon, alt, speed, track, onGround: ground === 1, timestampMs, positions,
      callsign: f.callsign, typecode: f.typecode, icon: f.typecode, origin: f.origin, destination: f.destination,
    });
  });
}
const fixture = (callsign: string) => landings.flights.find((f) => f.callsign === callsign)!;

test("real touchdowns and lift-offs are smooth: no jump at the ground flag, gear down, never below ground", () => {
  for (const f of landings.flights) {
    const m = new Motion();
    m.setAirports(fixtureAirports);
    const frames = play(m, fixtureReports(f));
    // before this change: up to 5.7 ft a frame (21,000 fpm) at Schiphol, 13 ft at Denver
    expect(worstStep(frames)).toBeLessThan(MAX_FRAME_FT);
    for (const fr of frames) {
      expect(fr.altFt).toBeGreaterThanOrEqual(0);
      if (fr.altFt < 30 && Math.abs(fr.drawn - f.eventMs) < 60_000) expect(fr.gear).toBe(1);
    }
    if (f.event === "land") {
      // down by the ground flag at the latest (it lags), and not long before the runway
      const down = firstDown(frames, f.eventMs - 120_000);
      expect(down).toBeGreaterThan(f.eventMs - 45_000);
      expect(down).toBeLessThan(f.eventMs + 8_000);
      expect(frames.filter((fr) => fr.drawn > down + 3_000).every((fr) => fr.altFt < 1)).toBe(true);
    } else {
      // the airborne flag comes during the roll: off the ground once it climbs, not before
      const up = lastUp(frames);
      expect(up).toBeGreaterThan(f.eventMs);
      expect(up).toBeLessThan(f.eventMs + 30_000);
    }
  }
});

test("each runway tells its field's QNH, which corrects the next arrival", () => {
  const learn = (callsign: string) => {
    const m = new Motion();
    m.setAirports(fixtureAirports);
    const f = fixture(callsign);
    play(m, fixtureReports(f));
    return { m, at: fixtureAirports.find((a) => a.iata === f.airport)!, t: f.eventMs };
  };
  // METAR: EHAM Q1004, EGLL Q1008, KORD A3014 (1020.7); to within what a runway (not the
  // field's highest point, in 25 ft steps) and a METAR (whole hPa, rounded down) can tell
  const ams = learn("KLM76F");
  expect(Math.abs(ams.m.fieldQnh(ams.at, ams.t) - 1004)).toBeLessThan(2.5);
  const lhr = learn("AEE604");
  expect(Math.abs(lhr.m.fieldQnh(lhr.at, lhr.t) - 1008)).toBeLessThan(2.5);
  const ord = learn("SKW446W");
  expect(Math.abs(ord.m.fieldQnh(ord.at, ord.t) - 1020.7)).toBeLessThan(2.5);

  // the next arrival at Schiphol is drawn at its true height: down when its runway roll begins
  // (the level run at 225 ft from 30 s before the ground flag), not 236 ft up until the flag
  const f = fixture("KLC20D");
  const frames = play(ams.m, fixtureReports(f));
  expect(worstStep(frames)).toBeLessThan(MAX_FRAME_FT);
  const down = firstDown(frames, f.eventMs - 120_000);
  expect(down).toBeGreaterThan(f.eventMs - 36_000);
  expect(down).toBeLessThan(f.eventMs - 22_000);
  // and the one in Atlanta (reading 250 ft low) no longer lands 15 s short: down with its braking
  const atl = learn("DAL1424");
  const g = fixture("SWA2713");
  const atlFrames = play(atl.m, fixtureReports(g));
  const atlDown = firstDown(atlFrames, g.eventMs - 120_000);
  expect(atlDown).toBeGreaterThan(g.eventMs - 20_000);
  expect(atlDown).toBeLessThan(g.eventMs - 8_000);
  // the flare: the last 30 ft take seconds, touching down at a few hundred fpm at most
  const i = atlFrames.findIndex((fr) => fr.drawn >= atlDown);
  const j = atlFrames.findIndex((fr) => fr.altFt < 30 && fr.drawn > g.eventMs - 60_000);
  expect(atlFrames[i].drawn - atlFrames[j].drawn).toBeGreaterThan(3_000);
  const sink = ((atlFrames[i - 31].altFt - atlFrames[i].altFt) / ((atlFrames[i].drawn - atlFrames[i - 31].drawn) / 1000)) * 60;
  expect(sink).toBeLessThan(300);
});

/** The synthetic field, 1000 ft up (so that its runway reads above 0 even 400 ft low). */
const FIELD_FT = 1_000;
const fld = () => airport({ iata: "FLD", lat: 45, lon: 10, alt: FIELD_FT });

/**
 * A synthetic arrival at a field `elev` ft up: 3° down at 140 kt, touching down at T0 + 120 s, then
 * braking at 2.5 kt/s. `alt` reads `biasFt` above the true height (pressure altitude, in 25 ft
 * steps), the ground flag comes `lateS` after touchdown and meanwhile the runway reads level,
 * as in the real data. Reports every `everyS` s, with a look-ahead buffer.
 */
function arrival(biasFt: number, everyS = 5, lateS = 12, id = 1, lastAirS = Infinity, elev = FIELD_FT): LiveFlight[] {
  const field = { lat: 45, lon: 10, elev };
  const tdS = 120;
  const v0 = 140 * KT;
  const north = (s: number) => (s <= tdS ? v0 * (s - tdS) : v0 * (s - tdS) - (2.5 * KT * (s - tdS) ** 2) / 2);
  const speed = (s: number) => (s <= tdS ? 140 : Math.max(20, 140 - 2.5 * (s - tdS)));
  const agl = (s: number) => Math.max(0, ((tdS - s) * v0 * Math.tan(3 * DEG)) / 0.3048);
  const reports: LiveFlight[] = [];
  for (let s = 0; s <= tdS + 40; s += everyS) {
    if (s < tdS && s > lastAirS) continue; // nothing heard on short final
    const [lat, lon] = offset(field.lat, field.lon, 0, north(s));
    const positions = [2, 4, 6, 8].map((d): [number, number, number] => {
      const [la, lo] = offset(field.lat, field.lon, 0, north(s + d));
      return [Math.round((la - lat) / 1e-5), Math.round((lo - lon) / 1e-5), d * 1000];
    });
    const onGround = s >= tdS + lateS;
    const alt = onGround ? 0 : Math.max(0, Math.round((field.elev + agl(s) + biasFt) / 25) * 25);
    reports.push(flight({ id, lat, lon, alt, speed: speed(s), track: 0, onGround, timestampMs: T0 + s * 1000, positions, destination: "FLD", origin: "XXX" }));
  }
  return reports;
}

/** A departure from the same field (`FIELD_FT` up): rolls from T0 at 2.5 kt/s, airborne-flagged from 90 kt, lifts off at 150 kt, climbs at 2000 fpm. */
function departure(biasFt: number, id = 2): LiveFlight[] {
  const loS = 60;
  const reports: LiveFlight[] = [];
  const north = (s: number) => (s <= loS ? (2.5 * KT * s * s) / 2 : (2.5 * KT * loS * loS) / 2 + 150 * KT * (s - loS));
  for (let s = 0; s <= 110; s += 5) {
    const speed = s <= loS ? 2.5 * s : 150;
    const [lat, lon] = offset(45, 10, 0, north(s));
    const positions = [2, 4, 6, 8].map((d): [number, number, number] => {
      const [la, lo] = offset(45, 10, 0, north(s + d));
      return [Math.round((la - lat) / 1e-5), Math.round((lo - lon) / 1e-5), d * 1000];
    });
    const climb = Math.max(0, ((s - loS) * 2000) / 60);
    const onGround = speed < 90;
    const alt = onGround ? 0 : Math.max(0, Math.round((FIELD_FT + climb + biasFt) / 25) * 25);
    reports.push(flight({ id, lat, lon, alt, speed, track: 0, onGround, timestampMs: T0 + s * 1000, positions, origin: "FLD", destination: "XXX" }));
  }
  return reports;
}


test("a pressure altitude 400 ft off either way: down on time once the field's QNH is known", () => {
  for (const bias of [400, -400]) {
    const m = new Motion();
    m.setAirports([fld(), airport({ id: 2, iata: "XXX", lat: 50, lon: 20 })]);
    // an earlier departure's take-off roll tells the field's QNH
    play(m, departure(bias, 2).map((f) => ({ ...f, timestampMs: f.timestampMs - 600_000 })));
    const qnh = m.fieldQnh(fld(), T0);
    expect(Math.abs(qnh - (1013.25 - bias / 27.7))).toBeLessThan(1);
    const frames = play(m, arrival(bias));
    expect(worstStep(frames)).toBeLessThan(MAX_FRAME_FT);
    expect(Math.min(...frames.map((f) => f.altFt))).toBeGreaterThanOrEqual(0);
    // at the true height on the approach (to within a 25 ft step and the smoothing's lag)
    const mid = frames.find((f) => f.drawn >= T0 + 60_000)!;
    expect(Math.abs(mid.altFt - (60 * 140 * KT * Math.tan(3 * DEG)) / 0.3048)).toBeLessThan(60);
    // down within a few seconds of the true touchdown (the flare stretches it a little)
    const down = firstDown(frames, T0 + 60_000);
    expect(Math.abs(down - (T0 + 120_000))).toBeLessThan(5_000);
    expect(frames.find((f) => f.drawn >= down)!.gear).toBe(1);
  }
});

test("a pressure altitude 400 ft off at a field whose QNH isn't known yet: no jump, down by the ground flag", () => {
  for (const bias of [400, -400]) {
    const m = new Motion();
    m.setAirports([fld(), airport({ id: 2, iata: "XXX", lat: 50, lon: 20 })]);
    const frames = play(m, arrival(bias));
    // the runway's level run tells it 5-10 s after touchdown: eased down, not dropped
    expect(worstStep(frames)).toBeLessThan(2 * MAX_FRAME_FT);
    expect(Math.min(...frames.map((f) => f.altFt))).toBeGreaterThanOrEqual(0);
    // (it can't be told before then: the drawn time is only seconds behind the data)
    const down = firstDown(frames, T0 + 60_000);
    expect(down).toBeLessThan(T0 + 120_000 + 15_000);
    // never lifted back up once down
    expect(frames.filter((f) => f.drawn > down).every((f) => f.altFt < 1)).toBe(true);
    // and it told the field's QNH for the next one
    expect(Math.abs(m.fieldQnh(fld(), T0 + 200_000) - (1013.25 - bias / 27.7))).toBeLessThan(1);
  }
});

test("sparse reports on short final: the descent carries on into a flare, not a drop at the ground report", () => {
  const m = new Motion();
  m.setAirports([fld(), airport({ id: 2, iata: "XXX", lat: 50, lon: 20 })]);
  // a report every 15 s, the last airborne one 30 s (~370 ft) out
  const frames = play(m, arrival(0, 15, 12, 1, 90));
  expect(worstStep(frames)).toBeLessThan(MAX_FRAME_FT);
  expect(Math.min(...frames.map((f) => f.altFt))).toBeGreaterThanOrEqual(0);
  const down = firstDown(frames, T0 + 60_000);
  expect(Math.abs(down - (T0 + 120_000))).toBeLessThan(6_000);
});

test("a take-off: on the ground through the airborne-flagged roll, then off it smoothly from 0", () => {
  for (const bias of [300, -300]) {
    const m = new Motion();
    m.setAirports([fld(), airport({ id: 2, iata: "XXX", lat: 50, lon: 20 })]);
    const frames = play(m, departure(bias));
    expect(worstStep(frames)).toBeLessThan(MAX_FRAME_FT);
    // flagged airborne at 90 kt (36 s) at the biased runway altitude: still rolling
    for (const f of frames) if (f.drawn < T0 + 58_000) expect(f.altFt).toBe(0);
    const up = lastUp(frames);
    expect(Math.abs(up - (T0 + 60_000))).toBeLessThan(5_000);
    // climbing at its true height above the field, not the biased one
    const later = frames.find((f) => f.drawn >= T0 + 90_000)!;
    expect(Math.abs(later.altFt - 1_000)).toBeLessThan(120);
    expect(frames.find((f) => f.drawn >= up)!.gear).toBe(1);
  }
});

test("no altitude on short final (a negative pressure altitude, sent as 0): the descent carries on", () => {
  const m = new Motion();
  m.setAirports([airport({ iata: "FLD", lat: 45, lon: 10, alt: 0 }), airport({ id: 2, iata: "XXX", lat: 50, lon: 20 })]);
  // QNH ~1022 at a field at sea level: the last ~250 ft read below 0, which Flightradar24 reports as 0
  const reports = arrival(-250, 5, 12, 1, Infinity, 0);
  const frames = play(m, reports);
  expect(worstStep(frames)).toBeLessThan(MAX_FRAME_FT);
  const down = firstDown(frames, T0 + 60_000);
  // it reads 250 ft low and the QNH isn't known: it gets down early, but gets down, gently
  expect(down).toBeLessThan(T0 + 120_000);
  expect(down).toBeGreaterThan(T0 + 85_000);
  expect(frames.filter((f) => f.drawn > down).every((f) => f.altFt < 1)).toBe(true);
});

test("the selected aircraft's altimeter setting (adsb.lol nav_qnh) corrects it and its field", () => {
  const m = new Motion();
  m.setAirports([fld(), airport({ id: 2, iata: "XXX", lat: 50, lon: 20 })]);
  // 1000 hPa: pressure altitude reads ~365 ft high
  const reports = arrival(365);
  const f = reports[10]; // 50 s in, ~1300 ft above the field
  m.pose(reports[9], reports[9].timestampMs + 3_000);
  m.pose(f, f.timestampMs + 3_000);
  m.measure(1, { positionMs: f.timestampMs, seenMs: f.timestampMs, lat: f.lat, lon: f.lon, onGround: false, altBaro: f.alt, navQnh: 1000 }, f.timestampMs + 3_000);
  expect(m.fieldQnh(fld(), f.timestampMs)).toBeCloseTo(1000, 3);
  // eased over to the true height
  const truth = ((120 - 50) * 140 * KT * Math.tan(3 * DEG)) / 0.3048;
  const p = poseAt(m, f, f.timestampMs + 1_000);
  expect(Math.abs(p.altFt - truth)).toBeLessThan(150);
  const q = poseAt(m, f, f.timestampMs + 8_000);
  expect(Math.abs(q.altFt - (truth - 8 * 140 * KT * Math.tan(3 * DEG) / 0.3048))).toBeLessThan(60);
});
