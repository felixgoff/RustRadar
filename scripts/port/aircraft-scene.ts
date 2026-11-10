// The aircraft part of today's `buildLayers()` run on the stage-4 synthetic snapshot, for the Rust
// port's golden counts (plan item A3, `crates/app/src/scene_builder/aircraft.rs`). Run from the
// repository root:
//
//   bun scripts/port/aircraft-scene.ts
//
// It writes crates/app/tests/data/aircraft-scene.json: one record per named view (V1, V2, V3, V4
// and two more), each with the icons (id, model, size, outline size, screen angle), the mesh groups
// (id, flights, raised heights, flights with gear drawn), the labelled flights (id, height, size)
// and the flights taxiways are asked for. No clock, no random numbers: two runs write the same
// bytes. Numbers are JSON.stringify's (the shortest decimal that reads back as the same double).
//
// The snapshot is `crates/app/tests/common/fixtures.rs` `synthetic_snapshot`, generated here the
// same way (the same integer hash; degrees to 4 decimals, so the f32 the wire carries reads back
// as that decimal). The loop is G:2520-2636 (and the CPU parts of G:2870-3025) copied as written,
// with today's modules: geo.ts, models.ts, motion.ts, screen.ts, deck's GlobeViewport, and the
// published models' lookup and sizes from static/aircraft/index.json and meshes.bin (what
// aircraft.ts reads; its `forFlight` is copied). Liveries do not change counts and are left out;
// no flight is selected or hovered, and nothing is followed; aircraft labels are on and the
// altitude exaggeration is the default 10. `yawOffset` is left out: it changes no count and no
// height.

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { _GlobeViewport as GlobeViewport } from "@deck.gl/core";
import type { LiveFlight } from "../../src/lib/api";
import { cosAngle, effectiveExaggeration, elevation, metersPerPixel, visibleRadius } from "../../src/lib/geo";
import type { Pose } from "../../src/lib/motion";
import { screenAngle } from "../../src/lib/screen";

// models.ts imports liveries.ts, which imports a Svelte module that calls the `$state` rune on load:
// outside Svelte's compiler it is plain data
const rune = Object.assign(<T>(v: T) => v, { raw: <T>(v: T) => v });
(globalThis as unknown as { $state: typeof rune }).$state = rune;
const { MODEL_SIZE, modelFor, models, sizeOf } = await import("../../src/lib/models");
const { Motion } = await import("../../src/lib/motion");

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const NOW_MS = 1_782_036_000_000;

// ------------------------------------------------------------------------------------------------
// The snapshot (fixtures.rs)
// ------------------------------------------------------------------------------------------------

const TYPES: [string, string][] = [
  ["A20N", "A320"], ["B738", "B738"], ["A359", "A330"], ["B77W", "B777"], ["E190", "E190"], ["CRJ9", "CRJ9"],
  ["DH8D", "Q300"], ["C172", "C206"], ["EC35", "EC"], ["BALL", "BALL"], ["", "GRND"], ["CONC", "CONC"],
  ["A388", "A380"], ["B744", "B747"], ["ZZZZ", ""], ["GLID", "GLID"],
];

function unit(n: number, salt: number): number {
  let x = (Math.imul(n, 0x9e3779b9) ^ Math.imul(salt, 0x85ebca6b)) >>> 0;
  x = (x ^ (x >>> 16)) >>> 0;
  x = Math.imul(x, 0x7feb352d) >>> 0;
  x = (x ^ (x >>> 15)) >>> 0;
  x = Math.imul(x, 0x846ca68b) >>> 0;
  x = (x ^ (x >>> 16)) >>> 0;
  return x / 4_294_967_296;
}

const deg4 = (x: number) => Math.round(x * 1e4) / 1e4;

function flight(n: number, lat: number, lon: number, alt: number, speed: number, track: number, onGround: boolean): LiveFlight {
  const [typecode, icon] = TYPES[n % TYPES.length];
  const named = n % 10 !== 9;
  const name = (prefix: string) => (named ? `${prefix}${String(n).padStart(3, "0")}` : "");
  return {
    id: 1000 + n, lat: deg4(lat), lon: deg4(lon), track, alt, speed, onGround, timestampMs: NOW_MS - 2000,
    callsign: name("SYN"), flight: name("SY"), reg: name("SE-"), typecode, origin: "", destination: "", icon, positions: [],
  } as LiveFlight;
}

function snapshot(): LiveFlight[] {
  const flights: LiveFlight[] = [];
  let n = 0;
  for (let i = 0; i < 200; i++, n++) {
    flights.push(flight(n, -55 + 125 * unit(n, 1), -180 + 360 * unit(n, 2), 1000 + Math.trunc(40_000 * unit(n, 3)),
      150 + Math.trunc(350 * unit(n, 4)), Math.trunc(360 * unit(n, 5)), false));
  }
  for (let i = 0; i < 30; i++, n++) {
    flights.push(flight(n, 59.33 + 1.2 * (unit(n, 1) - 0.5), 18.07 + 2.4 * (unit(n, 2) - 0.5), 1500 + Math.trunc(36_500 * unit(n, 3)),
      160 + Math.trunc(300 * unit(n, 4)), Math.trunc(360 * unit(n, 5)), false));
  }
  const tracks = [8, 188, 79, 259, 8, 188];
  for (let i = 0; i < 6; i++, n++) {
    const speed = i % 2 === 0 ? 0 : 8 + Math.trunc(6 * unit(n, 4));
    flights.push(flight(n, 59.6495 + 0.006 * unit(n, 1), 17.912 + 0.016 * unit(n, 2), 0, speed, tracks[i], true));
  }
  return flights;
}

// ------------------------------------------------------------------------------------------------
// The published models: aircraft.ts's lookup and sizes, and each mesh's positions for lowestZ
// ------------------------------------------------------------------------------------------------

type PackedMesh = { positions: number; vertices: number };
type Packed = { size: number; lods: PackedMesh[]; gear?: PackedMesh & { attachZ: number } };
const index = JSON.parse(readFileSync(join(ROOT, "static/aircraft/index.json"), "utf8")) as {
  models: Record<string, Packed>; types: Record<string, string>; icons: Record<string, string>;
  icon: { cell: number; outline?: { cell: number } };
};
const bin = readFileSync(join(ROOT, "static/aircraft/meshes.bin"));
const buffer = bin.buffer.slice(bin.byteOffset, bin.byteOffset + bin.byteLength);
const positionsOf = (m: PackedMesh) => new Float32Array(buffer, m.positions, m.vertices * 3);

type Real = { name: string; size: number; lods: { positions: Float32Array }[]; gear: { positions: Float32Array; attachZ: number } | null };
const real = new Map<string, Real>();
for (const [name, p] of Object.entries(index.models)) {
  real.set(name, {
    name, size: p.size, lods: p.lods.map((l) => ({ positions: positionsOf(l) })),
    gear: p.gear ? { positions: positionsOf(p.gear), attachZ: p.gear.attachZ } : null,
  });
}
function forFlight(typecode: string, icon: string): Real | null {
  const name = index.types[typecode.toUpperCase()] ?? index.icons[icon];
  return name ? (real.get(name) ?? null) : null;
}
const outlineScale = index.icon.outline ? index.icon.outline.cell / index.icon.cell : 1;

function lowestZ(positions: ArrayLike<number>): number {
  let z = 0;
  for (let i = 2; i < positions.length; i += 3) z = Math.min(z, positions[i]);
  return z;
}

// ------------------------------------------------------------------------------------------------
// buildLayers(): the aircraft (G:2520-2636, 2870-3025)
// ------------------------------------------------------------------------------------------------

const clamp01 = (x: number) => Math.min(1, Math.max(0, x));
const MIN_PX_GLOBE = 8;
const MIN_PX_CITY = 24;
const minPxAt = (zoom: number) => MIN_PX_GLOBE + (MIN_PX_CITY - MIN_PX_GLOBE) * clamp01((zoom - 3) / 7);
const depthAt = (zoom: number) => Math.round(clamp01((zoom - 6) / 3) * 10) / 10;
const FLAT_PITCH = 5;
const CLOSE_ZOOM = 11;
const GEAR_DEPTH = 0.8;
const FLAT_ABOVE_ZOOM = 12;
const TAXIWAY_ZOOM = 12;
const AIRCRAFT_LABEL_MIN_ZOOM = 4;
const ICON_HEADING_STEP_PX = 20;
const labelTextOf = (f: LiveFlight) => f.callsign.trim() || f.flight.trim() || f.reg.trim();

type View = { latitude: number; longitude: number; zoom: number; pitch: number; bearing: number };

function build(view: View, width: number, height: number, exaggeration: number) {
  const flights = snapshot();
  const motion = new Motion();
  const now = NOW_MS;
  const { longitude: cLon, latitude: cLat, zoom, bearing, pitch } = view;
  const flat = zoom > FLAT_ABOVE_ZOOM;
  const ex = effectiveExaggeration(exaggeration, zoom);
  const mpp = metersPerPixel(zoom, cLat);
  const minPx = minPxAt(zoom);
  const screenPx = (meters: number) => {
    const floor = minPx * Math.min(2, Math.max(0.6, (meters / MODEL_SIZE.narrow) ** 0.8));
    return Math.max(floor, meters / mpp);
  };
  const depth = depthAt(zoom);
  const radius = visibleRadius(zoom, pitch, width, height);
  const minCos = Math.cos((radius * Math.PI) / 180);
  const useIcons = pitch <= FLAT_PITCH && zoom < CLOSE_ZOOM;
  const detail = zoom >= CLOSE_ZOOM ? 1 : 0;

  type Group = { real: Real | null; kind: string; flights: LiveFlight[]; poses: Pose[]; positions: [number, number, number][] };
  const lockedOn = undefined;
  const selected = undefined as LiveFlight | undefined;
  const groups = new Map<string, Group>();
  const icons: LiveFlight[] = [];
  const iconPoses: Pose[] = [];
  const iconPositions: [number, number, number][] = [];
  const iconNames: string[] = [];
  const iconSizes: number[] = [];
  const taxiways = zoom >= TAXIWAY_ZOOM;
  const taxi: number[] = [];
  const labelling = true;
  const labelAll = labelling && zoom >= AIRCRAFT_LABEL_MIN_ZOOM;
  const labelled: LiveFlight[] = [];
  const labelledAt: [number, number, number][] = [];
  const labelledSizes: number[] = [];
  for (const f of flights) {
    const facing = cosAngle(f.lon, f.lat, cLon, cLat);
    if (f.id !== lockedOn && facing <= minCos) continue;
    const real = forFlight(f.typecode, f.icon);
    const pose = motion.pose(f, now);
    if (taxiways) taxi.push(f.id);
    const position: [number, number, number] = [pose.lon, pose.lat, elevation(pose.altFt, ex)];
    if (
      labelling &&
      (labelAll || f.id === selected?.id) &&
      (zoom > 6 || facing > 0.42 || f.id === lockedOn) &&
      labelTextOf(f)
    ) {
      labelled.push(f);
      labelledAt.push(position);
      labelledSizes.push(real?.size ?? sizeOf(f.icon));
    }
    if (useIcons && real) {
      icons.push(f);
      iconPoses.push(pose);
      iconPositions.push(position);
      iconNames.push(real.name);
      iconSizes.push(real.size);
      continue;
    }
    const [kind] = modelFor(f.icon);
    const id = real ? `t-${real.name}` : `k-${kind}`;
    let group = groups.get(id);
    if (!group) groups.set(id, (group = { real, kind, flights: [], poses: [], positions: [] }));
    group.flights.push(f);
    group.poses.push(pose);
    group.positions.push(position);
  }
  for (const group of groups.values()) {
    const real = group.real;
    const mesh = real?.gear?.positions ?? real?.lods[Math.min(detail, real.lods.length - 1)].positions ?? models()[group.kind as keyof typeof MODEL_SIZE].combined.attributes.positions!.value;
    const wheels = -lowestZ(mesh as ArrayLike<number>);
    const divisor = real?.size ?? MODEL_SIZE[group.kind as keyof typeof MODEL_SIZE];
    group.positions.forEach((position, i) => {
      const s = (screenPx(real?.size ?? sizeOf(group.flights[i].icon)) * mpp) / divisor;
      position[2] += wheels * s * Math.max(0.04, depth);
    });
  }
  const viewport = new GlobeViewport({ ...view, width, height });
  const step = ICON_HEADING_STEP_PX * mpp;
  const iconAngles = iconPoses.map((pose, i) => screenAngle(viewport, iconPositions[i], pose.heading, step) ?? bearing - pose.heading);
  return {
    view, flat, depth, detail, useIcons,
    icons: icons.map((f, i) => ({ id: f.id, name: iconNames[i], size: screenPx(iconSizes[i]) * 1.06, outline: screenPx(iconSizes[i]) * 1.06 * outlineScale, angle: iconAngles[i] })),
    groups: [...groups].map(([id, g]) => ({
      id,
      ids: g.flights.map((f) => f.id),
      z: g.positions.map((p) => p[2]),
      gear: g.real?.gear && depth >= GEAR_DEPTH ? g.flights.filter((_, i) => g.poses[i].gear > 0).map((f) => f.id) : [],
    })),
    labelled: labelled.map((f, i) => ({ id: f.id, z: labelledAt[i][2], size: labelledSizes[i] })),
    taxi,
  };
}

const VIEWS: Record<string, [number, number, number, number, number]> = {
  V1: [35, 10, 1.9, 0, 0],
  V2: [52, 10, 4.5, 0, 0],
  V3: [59.33, 18.07, 9, 0, 0],
  V4: [59.6519, 17.9186, 13.5, 45, 30],
  "V3-pitched": [59.33, 18.07, 9, 30, 0],
  "V4-11.9": [59.6519, 17.9186, 11.9, 45, 30],
};
const records = Object.entries(VIEWS).map(([name, [latitude, longitude, zoom, pitch, bearing]]) =>
  JSON.stringify({ name, width: 1100, height: 720, exaggeration: 10, ...build({ latitude, longitude, zoom, pitch, bearing }, 1100, 720, 10) }),
);
const OUT = join(ROOT, "crates", "app", "tests", "data");
mkdirSync(OUT, { recursive: true });
writeFileSync(join(OUT, "aircraft-scene.json"), `[\n${records.join(",\n")}\n]\n`);
