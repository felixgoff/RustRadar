// Unit tests for the aircraft motion model. Run with `bun test src`.
// @ts-ignore -- bun's types aren't a dependency; svelte-check still type-checks this file
import * as bunTest from "bun:test";
import type { Airport, LiveFlight } from "./api";
import { Motion, type Measured, type Pose } from "./motion";

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

function airport(over: Partial<Airport>): Airport {
  return { id: 1, name: "Test", iata: "TST", icao: "XTST", city: "", country: "", lat: 45, lon: 10, alt: 0, size: 0, timezone: "UTC", ...over };
}

test("straight flight matches dead reckoning", () => {
  const m = new Motion();
  const f = flight({ track: 90, speed: 250 });
  const p = m.pose(f, T0 + 30_000);
  const [lat, lon] = offset(45, 10, 250 * KT * 30, 0);
  expect(metres(p, { lat, lon })).toBeLessThan(5);
  expect(p.heading).toBeCloseTo(90, 1);
  expect(p.bank).toBeCloseTo(0, 3);

  // and with a buffer along the same line
  const line = (s: number): [number, number] => [0, 250 * KT * s];
  const g = pathFlight(line, { id: 2, track: 0, speed: 250 });
  const q = m.pose(g, T0 + 38_000);
  const [lat2, lon2] = offset(45, 10, ...line(38));
  expect(metres(q, { lat: lat2, lon: lon2 })).toBeLessThan(5);
});

test("a constant-rate turn continues along its circle", () => {
  const m = new Motion();
  const path = circle(160, 3);
  const f = pathFlight(path, { speed: 160, alt: 10_000 });
  for (const s of [10, 20, 30, 50]) {
    const p = m.pose(f, T0 + s * 1000);
    const [lat, lon] = offset(45, 10, ...path(s));
    expect(metres(p, { lat, lon })).toBeLessThan(25);
  }
  // heading follows the circle: 3 deg/s from north
  expect(m.pose(f, T0 + 20_000).heading).toBeCloseTo(60, 0);
});

test("interpolation passes through the buffer samples", () => {
  const m = new Motion();
  const path = circle(220, 2);
  const f = pathFlight(path, { speed: 220, alt: 12_000 });
  for (const [dLat, dLon, dMs] of [[0, 0, 0] as [number, number, number], ...f.positions!]) {
    const p = m.pose(f, T0 + dMs);
    expect(metres(p, { lat: f.lat + dLat * 1e-5, lon: f.lon + dLon * 1e-5 })).toBeLessThan(0.5);
  }
  // and runs smoothly between them, close to the true path
  const p = m.pose(f, T0 + 5_000);
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
  // a few seconds later it is on the new data
  const fresh = new Motion().pose(b, t);
  expect(metres(m.pose(b, t), fresh)).toBeLessThan(2);
  const [lat, lon] = offset(45, 10, ...off((t - T0) / 1000 - 10));
  expect(metres(m.pose(b, t), { lat, lon })).toBeLessThan(2);
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
  const r = m.pose(f, T0 + 700_000);
  const s = m.pose(f, T0 + 701_000);
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
  expect(m.pose(f, T0).phase).toBe("approach");
  expect(m.pose(f, T0).gear).toBe(1);
  expect(m.pose(f, T0).altFt).toBeCloseTo(800, 0);
  let landed = NaN;
  let last = m.pose(f, T0);
  for (let t = T0; t <= T0 + 120_000; t += 250) {
    const p = m.pose(f, t);
    expect(p.altFt).toBeGreaterThanOrEqual(0);
    expect(p.altFt).toBeLessThanOrEqual(last.altFt + 1e-9); // never climbs back up
    if (p.altFt < 200) expect(p.gear).toBe(1);
    if (p.altFt === 0 && Number.isNaN(landed)) landed = t;
    last = p;
  }
  // touchdown predicted ~70 s out (the descent levels off past 60 s)
  expect(landed).toBeGreaterThan(T0 + 60_000);
  expect(landed).toBeLessThan(T0 + 80_000);
  const roll = m.pose(f, landed + 10_000);
  expect(roll.phase).toBe("landingRoll");
  expect(roll.bank).toBe(0);
  expect(m.pose(f, landed + 6_000).pitch).toBeCloseTo(0, 3);
  // decelerating: less ground covered each second
  const d1 = metres(m.pose(f, landed + 2_000), m.pose(f, landed + 3_000));
  const d2 = metres(m.pose(f, landed + 20_000), m.pose(f, landed + 21_000));
  expect(d2).toBeLessThan(d1 - 10);
  // a report from the runway confirms it without lifting the aircraft off the ground
  const onRunway = flight({ ...f, lat: den.lat - 4 / 111.2, alt: 0, speed: 110, onGround: true, vspeed: 0, timestampMs: landed + 5_000 });
  const p = m.pose(onRunway, landed + 7_000);
  expect(p.phase).toBe("landingRoll");
  expect(p.altFt).toBe(0);
});

test("phases through a takeoff", () => {
  const m = new Motion();
  m.setAirports([airport({ iata: "AAA", alt: 100 }), airport({ iata: "BBB", lat: 50, lon: 20 })]);
  const base = { origin: "AAA", destination: "BBB", track: 0 };
  let t = T0;
  let north = 0;
  const report = (over: Partial<LiveFlight>, dt = 10_000) => {
    t += dt;
    north += ((over.speed ?? 0) * KT * dt) / 1000;
    const [lat, lon] = offset(45, 10, 0, north);
    return m.pose(flight({ ...base, lat, lon, timestampMs: t, ...over }), t);
  };
  expect(report({ onGround: true, speed: 0, alt: 0 }).phase).toBe("parked");
  expect(report({ onGround: true, speed: 15, alt: 0 }).phase).toBe("taxi");
  expect(report({ onGround: true, speed: 60, alt: 0 }).phase).toBe("takeoffRoll");
  expect(report({ onGround: true, speed: 120, alt: 0 }).phase).toBe("takeoffRoll");
  const climbing = report({ speed: 150, alt: 500 });
  expect(climbing.phase).toBe("initialClimb");
  // drawn above the field, not MSL (the predicted lift-off blends into the report over a few seconds)
  const later = m.pose(flight({ ...base, timestampMs: t }), t + 5_000).altFt;
  expect(later).toBeGreaterThan(400);
  expect(later).toBeLessThan(800);
  expect(report({ speed: 250, alt: 5_100 }, 120_000).phase).toBe("climb");
  expect(report({ speed: 450, alt: 35_000, vspeed: 0 }, 600_000).phase).toBe("cruise");
});

test("a takeoff roll lifts off by itself", () => {
  const m = new Motion();
  m.setAirports([airport({ iata: "AAA", alt: 100 })]);
  const f = flight({ origin: "AAA", onGround: true, speed: 120, alt: 0 });
  expect(m.pose(f, T0).phase).toBe("takeoffRoll");
  // 25 kt to go at 2.5 kt/s: rotation starts 3 s before lift-off at 10 s
  expect(m.pose(f, T0 + 6_000).pitch).toBe(0);
  const rotating = m.pose(f, T0 + 9_000).pitch;
  expect(rotating).toBeGreaterThan(0);
  expect(m.pose(f, T0 + 9_900).altFt).toBe(0);
  const airborne = m.pose(f, T0 + 20_000);
  expect(airborne.phase).toBe("initialClimb");
  expect(airborne.altFt).toBeCloseTo(300, 0); // 10 s at 1800 fpm
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
    return m.pose(flight({ ...base, lat, lon, timestampMs: t, ...over }), t);
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
  const right = m.pose(pathFlight(circle(250, 2), { id: 1, speed: 250, alt: 20_000 }), T0 + 12_000);
  const left = m.pose(pathFlight(circle(250, -2), { id: 2, speed: 250, alt: 20_000 }), T0 + 12_000);
  // rate-two turn at 250 kt: atan(v ω / g) ≈ 24°
  expect(right.bank).toBeGreaterThan(20);
  expect(right.bank).toBeLessThan(28);
  expect(left.bank).toBeLessThan(-20);
  expect(left.bank).toBeGreaterThan(-28);
  // bank fades out as the predicted turn ends
  expect(m.pose(pathFlight(circle(250, 2), { id: 1, speed: 250, alt: 20_000 }), T0 + 75_000).bank).toBe(0);

  const pitch = (vspeed: number, id: number) => m.pose(flight({ id, alt: 20_000, speed: 300, vspeed }), T0 + 5_000);
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
  // 2500 ft on approach at 1000 fpm: gear goes down passing 2000 ft, 30 s on
  const f = flight({ lat, lon, destination: "BBB", speed: 150, alt: 2_500, vspeed: -1_000 });
  expect(m.pose(f, T0).gear).toBe(0);
  for (let t = T0; t <= T0 + 31_000; t += 1_000) m.pose(f, t);
  const moving = m.pose(f, T0 + 32_000).gear;
  expect(moving).toBeGreaterThan(0);
  expect(moving).toBeLessThan(1);
  expect(m.pose(f, T0 + 40_000).gear).toBe(1);
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
  const p = m.pose(f, T0);
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
  expect(m.pose(g, T0 + 16 * 60_000).lat).toBeCloseTo(46, 6);
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
  const t = T0 + 3_000;
  expect(Math.abs(m.pose(f, t).bank)).toBeLessThan(0.5);
  m.measure(1, measuredAt(path, t, { gs: 250, track: 0, trueHeading: 0, roll: 25, trackRate: 2 }), t);
  // no jump on arrival, and only a gentle change per frame while it blends in
  // (a 25° step eases in at up to ~20°/s, 0.33° a frame)
  expect(maxStep(m, f, t, t + 6_000, (p) => p.bank)).toBeLessThan(0.4);
  expect(maxStep(m, f, t, t + 6_000, (p) => p.heading)).toBeLessThan(0.3);
  const p = m.pose(f, t + 6_000);
  expect(p.bank).toBeGreaterThan(22);
  expect(p.bank).toBeLessThan(28);
  // and the path now turns right, as the measured track rate says
  expect(p.track).toBeGreaterThan(5);
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
  const [lat, lon] = offset(45, 10, ...path(17));
  expect(metres(p, { lat, lon })).toBeLessThan(10);
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
  expect(free.m.pose(free.f, free.t + 90_000).altFt).toBeGreaterThan(32_300);

  const { m, f, t } = run(32_000);
  let prev = m.pose(f, t).altFt;
  let prevRate = 0;
  for (let s = t + 1_000; s <= t + 120_000; s += 1_000) {
    const alt = m.pose(f, s).altFt;
    expect(alt).toBeLessThanOrEqual(32_000.5);
    expect(alt).toBeGreaterThanOrEqual(prev - 0.5); // never sinks back
    const rate = alt - prev;
    if (s > t + 8_000) expect(Math.abs(rate - prevRate)).toBeLessThan(6); // the rate eases off, no kink
    prev = alt;
    prevRate = rate;
  }
  expect(m.pose(f, t + 90_000).altFt).toBeCloseTo(32_000, 0);
  // at the selected level the attitude is level flight's, not the climb's
  expect(m.pose(f, t + 90_000).pitch).toBeLessThan(4);
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
  const end = m.pose(f, t + 40_000);
  expect(end.track).toBeCloseTo(90, 0);
  expect(end.heading).toBeCloseTo(90, 0);
  expect(Math.abs(end.bank)).toBeLessThan(0.5);
  expect(m.pose(f, t + 60_000).track).toBeCloseTo(90, 0);

  // with LNAV engaged the selected heading doesn't steer: the turn carries on
  const lnav = new Motion();
  lnav.pose(f, t);
  lnav.measure(
    1,
    measuredAt(path, t, { gs: 160, track: 24, trueHeading: 24, magHeading: 22, trackRate: 3, navHeading: 88, navModes: ["lnav"] }),
    t,
  );
  expect(lnav.pose(f, t + 40_000).track).toBeGreaterThan(130);
});

test("a stale measured source falls back to the estimates smoothly", () => {
  const m = new Motion();
  const path = east(300);
  const f = pathFlight(path, { speed: 300, alt: 20_000, track: 90 });
  const t = T0 + 9_000;
  m.pose(f, t);
  m.measure(1, measuredAt(path, t, { gs: 300, track: 90, trueHeading: 98, roll: 12, trackRate: 0 }), t);
  expect(m.pose(f, t + 10_000).heading).toBeCloseTo(98, 0);
  expect(m.pose(f, t + 10_000).bank).toBeCloseTo(12, 0);
  // nothing more arrives: between 15 and 20 s the measured values ease out
  expect(maxStep(m, f, t + 10_000, t + 30_000, (p) => p.heading)).toBeLessThan(0.06);
  expect(maxStep(m, f, t + 10_000, t + 30_000, (p) => p.bank)).toBeLessThan(0.1);
  const late = m.pose(f, t + 25_000);
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
    // moving forward each frame, at about the right speed
    if (!(p.lon > prev.lon) || Math.abs(step - v * 0.016) > 1.5) console.log("BAD", t - start, step, k, a);
    worstStep = Math.max(worstStep, Math.abs(step - v * 0.016));
    const [lat, lon] = offset(45, 10, ...path((t - T0) / 1000));
    worstError = Math.max(worstError, metres(p, { lat, lon }));
    prev = p;
  }
  expect(worstStep).toBeLessThan(1.5);
  expect(worstError).toBeLessThan(80);
});
