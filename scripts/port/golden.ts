// Golden fixtures for the Rust port of `motion.ts` and `flightpath.ts` (plan item S1.17,
// docs/port/05-pure-logic-and-tests.md §15). Run from the repository root:
//
//   bun scripts/port/golden.ts
//
// It writes crates/core/tests/data/golden/*.json. The output depends on nothing but the
// TypeScript and src/lib/testdata/landings.json (no clock, no random numbers), so two runs
// are byte-identical. Once written the files are plain data and outlive the TypeScript.
//
// ---------------------------------------------------------------------------------------
// motion-*.json: one `Motion` driven through frames, every `Pose` written down
// ---------------------------------------------------------------------------------------
//
//   about      what the case is
//   stepMs     frame step (100)
//   startMs    `nowMs` of frame 0; frame i is at startMs + i * stepMs
//   frames     number of frames
//   airports   `Airport[]` (the wire form of api.ts), given to `setAirports` before frame 0
//   ids        aircraft ids, in the order they are posed within a frame
//   events     in the order they apply; each has `atMs` and `type`:
//                "report"    { flight: LiveFlight }           the newest report of flight.id
//                "measure"   { id, measured: Measured|null }  motion.measure(id, measured, now)
//                "taxiways"  { icao, lines: TaxiLine[]|null } motion.setTaxiways(icao, lines)
//   phases     the names `phase` indexes
//   columns    the meaning of a row of `poses`
//   poses      one row per aircraft per frame
//
// To replay: make a new Motion, `setAirports(airports)`, then for each frame, with
// now = startMs + frame * stepMs:
//   1. apply every event not yet applied with atMs <= now, in array order. A "report" only
//      replaces the LiveFlight held for its aircraft (nothing is called); "measure" and
//      "taxiways" call the model with `now`.
//   2. for each id of `ids`, in order, that has a report by now:
//      p = motion.pose(report, now), then drawnMs = motion.drawnTime(id, now); that is one row.
//
// Every LiveFlight here is one the Rust `model::LiveFlight` can hold and the webview would
// have seen: `lat` and `lon` are the shortest decimals of an f32 (as the backend's JSON
// carries them; the TypeScript computes with that decimal read as an f64, NOT with the f32
// widened), `track`, `alt` and `speed` are integers. `Measured` values are plain f64.
//
// ---------------------------------------------------------------------------------------
// routes.json: `estimateRoute` on fixed inputs
// ---------------------------------------------------------------------------------------
//
//   cases[]    { name, input: EstimateInput, output: { kind, before, ahead, departure, arrival } }
//
// `input.flight` and `input.trailStart` follow the same f32 rule; `before` and `ahead` are
// `[lon, lat, ft MSL][]`.

import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Airport, LiveFlight, Runway, TaxiLine, TrailPoint } from "../../src/lib/api";
import { destination, estimateRoute, type EstimateInput, type LonLat, type Traffic } from "../../src/lib/flightpath";
import { Motion, type Measured, type Phase } from "../../src/lib/motion";
import landings from "../../src/lib/testdata/landings.json";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const OUT = join(ROOT, "crates", "core", "tests", "data", "golden");

const STEP_MS = 100;
const DEG = Math.PI / 180;
const M_PER_DEG = 6_371_000 * DEG;
const KT = 0.514444;
const T0 = 1_760_000_000_000;

const PHASES: Phase[] = [
  "parked", "taxi", "takeoffRoll", "initialClimb", "climb", "cruise", "descent", "approach", "flare", "landingRoll",
];
const COLUMNS = ["frame", "id", "lon", "lat", "altFt", "heading", "track", "pitch", "bank", "gear", "phase", "drawnMs"];

// --- inputs the Rust model can hold --------------------------------------------------------

/** The shortest decimal that reads back as the f32 nearest `x`: what the backend's JSON carries. */
function f32Decimal(x: number): number {
  const f = Math.fround(x);
  for (let digits = 1; digits <= 9; digits++) {
    const d = Number(f.toPrecision(digits));
    if (Math.fround(d) === f) return d;
  }
  throw new Error(`no f32 decimal for ${x}`);
}

function assertF32(x: number, what: string): void {
  if (!Number.isFinite(x) || f32Decimal(x) !== x) throw new Error(`${what}: ${x} is not the decimal of an f32`);
}

function assertInt(x: number, what: string): void {
  if (!Number.isInteger(x)) throw new Error(`${what}: ${x} is not an integer`);
}

function checkFlight(f: LiveFlight): LiveFlight {
  assertF32(f.lat, "lat");
  assertF32(f.lon, "lon");
  for (const key of ["id", "track", "alt", "speed", "timestampMs"] as const) assertInt(f[key], key);
  if (f.id < 0 || f.timestampMs < 0) throw new Error("negative id or time");
  for (const p of f.positions ?? []) for (const v of p) assertInt(v, "positions");
  return f;
}

/** Latitude or longitude as Flightradar24 gives it: five decimals (which an f32 keeps below 128 degrees). */
const q5 = (deg: number) => Number(deg.toFixed(5));

/** A point `east`/`north` metres from lat/lon (flat earth, as the model uses). */
function offset(lat: number, lon: number, east: number, north: number): [number, number] {
  return [lat + north / M_PER_DEG, lon + east / (M_PER_DEG * Math.cos(lat * DEG))];
}

function flight(over: Partial<LiveFlight>): LiveFlight {
  return checkFlight({
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
  });
}

/** Flightradar24's position buffer for a report at `lat`/`lon`: where it was `ds` seconds before, newest first. */
function history(lat: number, lon: number, past: (d: number) => [number, number], ds = [2, 4, 6, 8]): [number, number, number][] {
  return ds.map((d): [number, number, number] => {
    const [la, lo] = past(d);
    return [Math.round((lat - la) / 1e-5), Math.round((lon - lo) / 1e-5), d * 1000];
  });
}

/** A report at `s` seconds on `truth(seconds) -> [lat, lon]`, quantised as Flightradar24 does. */
function reportAt(truth: (s: number) => [number, number], s: number, over: Partial<LiveFlight>, ds?: number[]): LiveFlight {
  const [la, lo] = truth(s);
  const lat = q5(la);
  const lon = q5(lo);
  return flight({ lat, lon, timestampMs: T0 + Math.round(s * 1000), positions: history(lat, lon, (d) => truth(s - d), ds), ...over });
}

function airport(over: Partial<Airport>): Airport {
  return { id: 1, name: "Test", iata: "TST", icao: "XTST", city: "", country: "", lat: 45, lon: 10, alt: 0, size: 0, timezone: "UTC", ...over };
}

// --- a motion case -------------------------------------------------------------------------

type MotionEvent =
  | { atMs: number; type: "report"; flight: LiveFlight }
  | { atMs: number; type: "measure"; id: number; measured: Measured | null }
  | { atMs: number; type: "taxiways"; icao: string; lines: TaxiLine[] | null };

interface MotionCase {
  name: string;
  about: string;
  airports: Airport[];
  ids: number[];
  events: MotionEvent[];
  startMs: number;
  endMs: number;
}

/** Reports as events, each arriving `latency` after its time (as `play` in motion.test.ts). */
const arriving = (reports: LiveFlight[], latency = 3_000): MotionEvent[] =>
  reports.map((f) => ({ atMs: f.timestampMs + latency, type: "report", flight: f }));

/** Events in the order they apply: by time, ties in the order given. */
function ordered(events: MotionEvent[]): MotionEvent[] {
  return events
    .map((e, i) => ({ e, i }))
    .sort((a, b) => a.e.atMs - b.e.atMs || a.i - b.i)
    .map(({ e }) => e);
}

function runMotion(c: MotionCase): string {
  const events = ordered(c.events);
  const frames = Math.floor((c.endMs - c.startMs) / STEP_MS) + 1;
  const m = new Motion();
  m.setAirports(c.airports);
  const current = new Map<number, LiveFlight>();
  const rows: string[] = [];
  let next = 0;
  for (let frame = 0; frame < frames; frame++) {
    const now = c.startMs + frame * STEP_MS;
    for (; next < events.length && events[next].atMs <= now; next++) {
      const e = events[next];
      if (e.type === "report") current.set(e.flight.id, e.flight);
      else if (e.type === "measure") m.measure(e.id, e.measured, now);
      else m.setTaxiways(e.icao, e.lines);
    }
    for (const id of c.ids) {
      const f = current.get(id);
      if (!f) continue;
      const p = m.pose(f, now);
      const drawnMs = m.drawnTime(id, now);
      const phase = PHASES.indexOf(p.phase);
      if (phase < 0) throw new Error(`${c.name}: unknown phase ${p.phase}`);
      const row = [frame, id, p.lon, p.lat, p.altFt, p.heading, p.track, p.pitch, p.bank, p.gear, phase, drawnMs];
      if (!row.every(Number.isFinite)) throw new Error(`${c.name}: frame ${frame} has a value JSON cannot hold: ${row}`);
      rows.push(JSON.stringify(row));
    }
  }
  if (next !== events.length) throw new Error(`${c.name}: ${events.length - next} events after the last frame`);
  for (const id of c.ids) if (!current.has(id)) throw new Error(`${c.name}: no report for ${id}`);
  return [
    "{",
    `"about": ${JSON.stringify(c.about)},`,
    `"stepMs": ${STEP_MS},`,
    `"startMs": ${c.startMs},`,
    `"frames": ${frames},`,
    `"airports": ${JSON.stringify(c.airports)},`,
    `"ids": ${JSON.stringify(c.ids)},`,
    `"events": [\n${events.map((e) => JSON.stringify(e)).join(",\n")}\n],`,
    `"phases": ${JSON.stringify(PHASES)},`,
    `"columns": ${JSON.stringify(COLUMNS)},`,
    `"poses": [\n${rows.join(",\n")}\n]`,
    "}",
    "",
  ].join("\n");
}

// --- the recorded flights ------------------------------------------------------------------

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

function recorded(f: FixtureFlight): MotionCase {
  const reports = fixtureReports(f);
  const latency = 3_000;
  return {
    name: `motion-${f.callsign.toLowerCase()}`,
    about:
      `${f.callsign} of landings.json (${f.event === "land" ? "landing at" : "take-off from"} ${f.airport}), ` +
      "played as motion.test.ts plays it: each report arrives 3 s after its time, frames run to 10 s after the last arrives.",
    airports: fixtureAirports,
    ids: [f.id],
    events: arriving(reports, latency),
    startMs: reports[0].timestampMs + latency,
    endMs: reports[reports.length - 1].timestampMs + latency + 10_000,
  };
}

// --- synthetic 1: a climbing turn with measured data ---------------------------------------

/**
 * 250 kt north from 45N 10E, a right turn of 60 degrees at 2 deg/s between 40 and 70 s, then
 * straight; climbing at 1500 fpm from 12,000 ft to level at 14,000 ft (at 80 s). Flightradar24
 * reports every 5 s, 3 s late. From 20 s the aircraft is "selected": a measured state every
 * second, 1 s late, with a 4 degree crab, the roll, the rates and the selected altitude; the
 * position is in every other one. At 100 s the source goes quiet and at 115 s it is dropped.
 */
function turnMeasured(): MotionCase {
  const v = 250 * KT;
  const w = 2 * DEG;
  const r = v / w;
  const turnFrom = 40;
  const turnTo = 70;
  const east = (s: number): [number, number] => {
    if (s <= turnFrom) return [0, v * s];
    const a = w * (Math.min(s, turnTo) - turnFrom);
    const x = r * (1 - Math.cos(a));
    const y = v * turnFrom + r * Math.sin(a);
    const after = Math.max(0, s - turnTo);
    return [x + v * after * Math.sin(a), y + v * after * Math.cos(a)];
  };
  const truth = (s: number) => offset(45, 10, ...east(s));
  const course = (s: number) => 2 * (Math.min(Math.max(s, turnFrom), turnTo) - turnFrom);
  const turning = (s: number) => s > turnFrom && s < turnTo;
  const levelAt = 80;
  const alt = (s: number) => 12_000 + (1_500 / 60) * Math.min(Math.max(s, 0), levelAt);
  const climbing = (s: number) => s < levelAt;
  const bank = Math.atan2(v * w, 9.80665) / DEG;

  const events: MotionEvent[] = [];
  for (let s = 0; s <= 125; s += 5) {
    const f = reportAt(truth, s, { speed: 250, track: Math.round(course(s)), alt: Math.round(alt(s) / 25) * 25 });
    events.push({ atMs: f.timestampMs + 3_000, type: "report", flight: f });
  }
  for (let s = 20; s <= 100; s++) {
    const t = T0 + s * 1000;
    const [lat, lon] = truth(s);
    const measured: Measured = {
      seenMs: t,
      onGround: false,
      altBaro: Math.round(alt(s) / 25) * 25,
      gs: 250,
      track: course(s),
      trueHeading: course(s) + 4,
      magHeading: course(s) + 1,
      roll: turning(s) ? bank : 0,
      trackRate: turning(s) ? 2 : 0,
      baroRate: climbing(s) ? 1_500 : 0,
      geomRate: climbing(s) ? 1_472 : 0,
      tas: 262,
      navAltitudeMcp: 14_000,
      navQnh: 1013.2,
      navModes: ["autopilot", "vnav"],
    };
    if (s % 2 === 0) Object.assign(measured, { positionMs: t, lat, lon });
    events.push({ atMs: t + 1_000, type: "measure", id: 1, measured });
  }
  events.push({ atMs: T0 + 115_000, type: "measure", id: 1, measured: null });
  return {
    name: "motion-synthetic-turn-measured",
    about:
      "Synthetic: 250 kt north from 45N 10E, a 60 degree right turn at 2 deg/s from 40 to 70 s, climbing 1500 fpm " +
      "from 12,000 to 14,000 ft; Flightradar24 reports every 5 s (3 s late); measured states every second from 20 to " +
      "100 s (1 s late, a position in every other one), then silence, dropped at 115 s.",
    airports: [],
    ids: [1],
    events,
    startMs: T0 + 3_000,
    endMs: T0 + 135_000,
  };
}

// --- synthetic 2: taxiing an L on a taxiway network ----------------------------------------

/**
 * `taxiL` of motion.test.ts: north along x = 0 from 400 m south of the corner, a 30 m radius
 * turn right, then east, at 15 kt, positions off by up to 8 m; reports every 6 s, 3 s late.
 * The taxiway network is there from the start, forgotten at 60 s and given again at 75 s.
 */
function taxi(): MotionCase {
  const v = 15 * KT;
  const R = 30;
  const leg1 = 370;
  const arc = (Math.PI / 2) * R;
  const noise = 8;
  const truth = (s: number): [number, number] => {
    const d = v * s;
    if (d <= leg1) return [0, -400 + d];
    if (d <= leg1 + arc) {
      const a = (d - leg1) / R;
      return [R - R * Math.cos(a), -R + R * Math.sin(a)];
    }
    return [R + (d - leg1 - arc), 0];
  };
  const jitter = (s: number, axis: number) => {
    const x = Math.sin((s * 3 + axis) * 12.9898) * 43758.5453;
    return noise * ((x - Math.floor(x)) * 2 - 1);
  };
  const measured = (s: number) => {
    const [e, n] = truth(s);
    return offset(45, 10, e + jitter(s, 0), n + jitter(s, 1));
  };
  const totalS = (leg1 + arc + 300) / v;
  const reports: LiveFlight[] = [];
  for (let k = 0; k * 6 < totalS; k++) {
    const s0 = k * 6;
    const [e0, n0] = truth(s0);
    const [e1, n1] = truth(s0 + 0.5);
    const track = Math.round((Math.atan2(e1 - e0, n1 - n0) / DEG + 360) % 360) % 360;
    reports.push(reportAt(measured, s0, { alt: 0, onGround: true, speed: 15, track, origin: "TST" }, [2, 4, 6]));
  }
  const line = (points: [number, number][], kind: TaxiLine["kind"] = "taxiway"): TaxiLine => ({
    kind,
    points: points.map(([e, n]) => {
      const [lat, lon] = offset(45, 10, e, n);
      return [lon, lat];
    }),
  });
  const lines = [
    line([[0, -500], [0, -200], [0, 0]]),
    line([[0, 0], [250, 0], [500, 0]]),
    line([[0, 0], [-300, 0]], "taxilane"),
    line([[400, -300], [400, 300]], "runway"),
  ];
  return {
    name: "motion-synthetic-taxi",
    about:
      "Synthetic: taxiing an L at 15 kt around 45N 10E (north, a 30 m radius right turn, east), positions up to 8 m " +
      "off, reports every 6 s (3 s late), on a taxiway network that is forgotten at 60 s and given again at 75 s.",
    airports: [airport({})],
    ids: [1],
    events: [
      { atMs: T0, type: "taxiways", icao: "XTST", lines },
      ...arriving(reports),
      { atMs: T0 + 60_000, type: "taxiways", icao: "XTST", lines: null },
      { atMs: T0 + 75_000, type: "taxiways", icao: "XTST", lines },
    ],
    startMs: T0 + 3_000,
    endMs: reports[reports.length - 1].timestampMs + 3_000 + 10_000,
  };
}

// --- synthetic 3: an arrival and a departure at one field ----------------------------------

/**
 * `arrival` and `departure` of motion.test.ts at a field 1000 ft up, in one Motion, so that
 * the QNH the arrival's runway tells corrects the departure. The arrival (id 1) comes 3
 * degrees down at 140 kt, reading 400 ft high, nothing heard for the last 20 s of the
 * approach, touching down at 120 s and flagged on the ground 12 s later; the departure
 * (id 2) starts its roll at 150 s, 2 km east of the arrival's runway, reading 400 ft high too.
 */
function arrivalDeparture(): MotionCase {
  const fieldFt = 1_000;
  const biasFt = 400;
  const field = airport({ iata: "FLD", icao: "XFLD", lat: 45, lon: 10, alt: fieldFt });

  const tdS = 120;
  const lateS = 12;
  const lastAirS = 100;
  const v0 = 140 * KT;
  const northA = (s: number) => (s <= tdS ? v0 * (s - tdS) : v0 * (s - tdS) - (2.5 * KT * (s - tdS) ** 2) / 2);
  const agl = (s: number) => Math.max(0, ((tdS - s) * v0 * Math.tan(3 * DEG)) / 0.3048);
  const truthA = (s: number) => offset(45, 10, 0, northA(s));
  const arrivals: LiveFlight[] = [];
  for (let s = 0; s <= tdS + 40; s += 5) {
    if (s < tdS && s > lastAirS) continue; // nothing heard on short final
    const onGround = s >= tdS + lateS;
    const alt = onGround ? 0 : Math.max(0, Math.round((fieldFt + agl(s) + biasFt) / 25) * 25);
    const speed = Math.round(s <= tdS ? 140 : Math.max(20, 140 - 2.5 * (s - tdS)));
    arrivals.push(reportAt(truthA, s, { id: 1, alt, speed, track: 0, onGround, destination: "FLD", origin: "XXX" }));
  }

  const startS = 150;
  const loS = 60;
  const northD = (u: number) =>
    u <= 0 ? 0 : u <= loS ? (2.5 * KT * u * u) / 2 : (2.5 * KT * loS * loS) / 2 + 150 * KT * (u - loS);
  const truthD = (s: number) => offset(45, 10, 2_000, northD(s - startS));
  const departures: LiveFlight[] = [];
  for (let u = 0; u <= 110; u += 5) {
    const speed = u <= loS ? 2.5 * u : 150;
    const climb = Math.max(0, ((u - loS) * 2000) / 60);
    const onGround = speed < 90;
    const alt = onGround ? 0 : Math.max(0, Math.round((fieldFt + climb + biasFt) / 25) * 25);
    departures.push(
      reportAt(truthD, startS + u, {
        id: 2, alt, speed: Math.round(speed), track: 0, onGround, origin: "FLD", destination: "XXX",
        callsign: "TEST2", flight: "TE2", reg: "N2", typecode: "B738", icon: "B738",
      }),
    );
  }
  return {
    name: "motion-synthetic-arrival-departure",
    about:
      "Synthetic: at a field 1000 ft up (45N 10E), an arrival (id 1: 3 degrees down at 140 kt, pressure altitude " +
      "400 ft high, silent for the last 20 s, down at 120 s, ground flag 12 s late) and then a departure (id 2: rolls " +
      "from 150 s, lifts off 60 s later, 400 ft high too) in one Motion; reports every 5 s, 3 s late.",
    airports: [field],
    ids: [1, 2],
    events: [...arriving(arrivals), ...arriving(departures)],
    startMs: T0 + 3_000,
    endMs: T0 + (startS + 110) * 1000 + 3_000 + 10_000,
  };
}

// --- estimateRoute -------------------------------------------------------------------------

/** Copenhagen Kastrup's runways, as OurAirports lists them (as in routes.test.ts). */
const EKCH: Runway[] = [
  {
    lengthFt: 11811, widthFt: 148, surface: "ASP",
    ends: [
      { ident: "04L", lat: 55.592201, lon: 12.603536, elevationFt: 13, displacedFt: 0 },
      { ident: "22R", lat: 55.616543, lon: 12.641174, elevationFt: 14, displacedFt: 1967 },
    ],
  },
  {
    lengthFt: 10827, widthFt: 148, surface: "ASP",
    ends: [
      { ident: "04R", lat: 55.6031, lon: 12.633, elevationFt: 12, displacedFt: 0 },
      { ident: "22L", lat: 55.6254, lon: 12.6676, elevationFt: 8, displacedFt: 0 },
    ],
  },
  {
    lengthFt: 9186, widthFt: 148, surface: "ASPH",
    ends: [
      { ident: "12", lat: 55.626293, lon: 12.633331, elevationFt: 13, displacedFt: 2313 },
      { ident: "30", lat: 55.612522, lon: 12.670532, elevationFt: 8, displacedFt: 886 },
    ],
  },
];
/** Stockholm Arlanda, without elevations or lengths (older caches lack them). */
const ESSA: Runway[] = [
  {
    lengthFt: null, widthFt: null, surface: "",
    ends: [
      { ident: "01L", lat: 59.6372, lon: 17.9133, elevationFt: null, displacedFt: 0 },
      { ident: "19R", lat: 59.6661, lon: 17.9237, elevationFt: null, displacedFt: 0 },
    ],
  },
  {
    lengthFt: 8202, widthFt: 148, surface: "ASP",
    ends: [
      { ident: "08", lat: 59.6604, lon: 17.9367, elevationFt: 115, displacedFt: 0 },
      { ident: "26", lat: 59.6634, lon: 17.9805, elevationFt: 137, displacedFt: 0 },
    ],
  },
];
/** Nadi-like: a single runway next to the antimeridian. */
const DATELINE: Runway[] = [
  {
    lengthFt: 10000, widthFt: 150, surface: "ASP",
    ends: [
      { ident: "09", lat: -17.0, lon: 179.95, elevationFt: 20, displacedFt: 0 },
      { ident: "27", lat: -17.0, lon: 179.98, elevationFt: 30, displacedFt: 0 },
    ],
  },
];

const CPH = airport({ id: 1, name: "Copenhagen", iata: "CPH", icao: "EKCH", lat: 55.618, lon: 12.656, alt: 17, size: 1, timezone: "" });
const ARN = airport({ id: 2, name: "Stockholm Arlanda", iata: "ARN", icao: "ESSA", lat: 59.6519, lon: 17.9186, alt: 137, size: 1, timezone: "" });
const DTL = airport({ id: 3, name: "Dateline", iata: "DTL", icao: "XDTL", lat: -17.0, lon: 179.965, alt: null, size: 1, timezone: "" });

type RouteFlight = EstimateInput["flight"];

function routeFlight(lat: number, lon: number, track: number, alt: number, speed: number, onGround = false, id = 1): RouteFlight {
  assertF32(lat, "flight.lat");
  assertF32(lon, "flight.lon");
  for (const v of [id, track, alt, speed]) assertInt(v, "flight");
  return { id, lat, lon, track, alt, speed, onGround };
}

function trailStart(lat: number, lon: number, track: number, altitude: number, groundSpeed: number): TrailPoint {
  assertF32(lat, "trailStart.latitude");
  assertF32(lon, "trailStart.longitude");
  for (const v of [track, altitude, groundSpeed]) assertInt(v, "trailStart");
  return { timestamp: 1_760_000_000, latitude: lat, longitude: lon, altitude, ground_speed: groundSpeed, track, vertical_speed: 0, source: 0 };
}

/** Traffic at a point given as [lon, lat], rounded as Flightradar24's positions are. */
function trafficAt(id: number, at: LonLat, track: number, alt: number, speed: number, onGround = false): Traffic {
  return { id, lat: q5(at[1]), lon: q5(at[0]), track, alt, speed, onGround };
}

function routeCases(): { name: string; input: EstimateInput }[] {
  const none = { originRunways: [], destinationRunways: [], traffic: [] };
  // traffic landing on the 04s at Copenhagen and departing on 22R
  const final04L = destination([12.603536, 55.592201], 40.9 + 180, 6 * 1852);
  const final04R = destination([12.633, 55.6031], 41.2 + 180, 3 * 1852);
  const climb22R = destination([12.603536, 55.592201], 221, 4 * 1852);
  const landing04: Traffic[] = [
    trafficAt(11, final04L, 41, 1_900, 150),
    trafficAt(12, final04R, 41, 1_000, 140),
    trafficAt(13, [12.2, 56.4], 180, 30_000, 440), // unrelated
  ];
  const departing22: Traffic[] = [trafficAt(21, climb22R, 221, 2_500, 180), trafficAt(1, final04L, 41, 1_900, 150)];
  return [
    {
      name: "arrival, runway guessed",
      input: { flight: routeFlight(55.618, 12.813, 22, 4_250, 220), destination: CPH, ...none, destinationRunways: EKCH },
    },
    {
      name: "arrival, no runways known",
      input: { flight: routeFlight(55.618, 12.813, 22, 4_250, 220), destination: CPH, ...none },
    },
    {
      name: "arrival, the runway other traffic lands on",
      input: { flight: routeFlight(55.618, 12.813, 22, 4_250, 220), destination: CPH, ...none, destinationRunways: EKCH, traffic: landing04 },
    },
    {
      name: "arrival, lined up on final",
      input: { flight: routeFlight(55.69687, 12.77897, 221, 1_900, 160), destination: CPH, ...none, destinationRunways: EKCH },
    },
    {
      name: "arrival, over the runway in the flare",
      input: { flight: routeFlight(55.62183, 12.66204, 221, 40, 140), destination: CPH, ...none, destinationRunways: EKCH },
    },
    {
      name: "arrival from cruise, far out",
      input: { flight: routeFlight(48.35, 2.55, 30, 36_000, 450), destination: CPH, ...none, destinationRunways: EKCH },
    },
    {
      name: "arrival from behind the airport, descending",
      input: { flight: routeFlight(56.1, 13.4, 300, 20_000, 400), destination: CPH, ...none, destinationRunways: EKCH },
    },
    {
      name: "departure and arrival, both with runways",
      input: {
        flight: routeFlight(58.9, 16.9, 25, 15_000, 320),
        trailStart: trailStart(55.4, 12.2, 250, 9_000, 300),
        origin: CPH,
        destination: ARN,
        originRunways: EKCH,
        destinationRunways: ESSA,
        traffic: departing22,
      },
    },
    {
      name: "departure, no runways known, speed unknown",
      input: {
        flight: routeFlight(57.2, 14.6, 30, 34_000, 430),
        trailStart: trailStart(55.9, 13.1, 35, 11_000, 0),
        origin: CPH,
        ...none,
      },
    },
    {
      name: "departure, the trail begins at the airport",
      input: {
        flight: routeFlight(57.2, 14.6, 30, 34_000, 430),
        trailStart: trailStart(55.62, 12.65, 220, 0, 20),
        origin: CPH,
        ...none,
        originRunways: EKCH,
      },
    },
    {
      name: "on the ground",
      input: { flight: routeFlight(55.61, 12.64, 40, 0, 12, true), destination: ARN, ...none, destinationRunways: ESSA },
    },
    {
      name: "no airports",
      input: { flight: routeFlight(55.618, 12.813, 22, 4_250, 220), ...none },
    },
    {
      name: "arrival across the antimeridian",
      input: { flight: routeFlight(-16.5, -179.5, 250, 9_000, 280), destination: DTL, ...none, destinationRunways: DATELINE },
    },
  ];
}

function runRoutes(): string {
  const cases = routeCases().map(({ name, input }) => {
    const output = estimateRoute(input);
    for (const p of [...output.before, ...output.ahead])
      if (p.length !== 3 || !p.every(Number.isFinite)) throw new Error(`routes "${name}": a point JSON cannot hold: ${p}`);
    return (
      `{"name": ${JSON.stringify(name)},\n` +
      `"input": ${JSON.stringify(input)},\n` +
      `"output": {"kind": ${JSON.stringify(output.kind)}, "departure": ${JSON.stringify(output.departure)}, ` +
      `"arrival": ${JSON.stringify(output.arrival)},\n` +
      `"before": ${JSON.stringify(output.before)},\n` +
      `"ahead": ${JSON.stringify(output.ahead)}}}`
    );
  });
  const about =
    "estimateRoute (src/lib/flightpath.ts) on fixed inputs. Each case: input is an EstimateInput, output is what " +
    "it returned; before and ahead are [lon, lat, ft MSL] points. Written by scripts/port/golden.ts.";
  return `{\n"about": ${JSON.stringify(about)},\n"cases": [\n${cases.join(",\n")}\n]\n}\n`;
}

// --- write ---------------------------------------------------------------------------------

mkdirSync(OUT, { recursive: true });
let total = 0;
function write(name: string, text: string): void {
  writeFileSync(join(OUT, name), text);
  total += Buffer.byteLength(text);
  console.log(`${name}: ${Buffer.byteLength(text)} bytes`);
}

const motionCases = [...landings.flights.map(recorded), turnMeasured(), taxi(), arrivalDeparture()];
for (const c of motionCases) write(`${c.name}.json`, runMotion(c));
write("routes.json", runRoutes());
console.log(`total: ${total} bytes`);
if (total >= 5_000_000) throw new Error("the golden files must stay under 5 MB in total");
