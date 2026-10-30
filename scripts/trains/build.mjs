// Turns Lardman's train models (3D Warehouse, used with his permission; the
// app credits him) into the cars RustRadar draws close in. Run with
// `bun run build:trains <folder>`, the folder holding each model's SketchUp
// Collada export as `<name>/model.dae`; the generated files go to
// `static/trains/`.
//
// Each model is a whole trainset lying along +y. It is cut into cars at
// `cuts`, and three are kept: the leading car, one from the middle and the
// last, which is all a train on the map needs (train-models.ts). Each car is
// turned to face +y (the way the map's cars run), reduced by vertex
// clustering as the aircraft are (scripts/aircraft/build.mjs), and scaled
// into the unit box the map's cars are drawn in: x across by CAR_WIDTH_M, y
// along by its own length, z up by CAR_HEIGHT_M.
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { readCollada } from "./collada.mjs";

const OUT = new URL("../../static/trains/", import.meta.url);
/** As in src/lib/train-models.ts. */
const CAR_WIDTH_M = 3.1;
const CAR_HEIGHT_M = 4.1;
/** Triangles per car. */
const TRIANGLES = 1500;

/**
 * The models RustRadar draws (which train takes which is in
 * src-tauri/src/train_class.rs): the export's folder, and where its cars
 * meet along y, metres, read off the bogies and the gaps between bodies.
 * Models of two units keep only the first.
 */
const MODELS = {
  contessa: { dir: "Contessa", cuts: [-26.4, -0.2, 26.4, 52.7] },
  x40: { dir: "Coradia+Duplex+X40", cuts: [9.7, 36.9, 63.3, 91.1] },
  x60: { dir: "Coradia+Lirex+X60", cuts: [0.4, 20.75, 37.15, 53.55, 69.95, 86.35, 107.1] },
  "x61-skane": { dir: "Coradia+Nordic+X61", cuts: [2.6, 22.6, 39.6, 56.0, 76.7] },
  "x61-vasttrafik": { dir: "Coradia+Nordic+X61+VT", cuts: [2.6, 22.6, 39.6, 56.0, 76.7] },
  "sj-intercity": { dir: "SJ+IC_black", cuts: [0.3, 15.4, 42.1, 68.6, 95.2, 121.7, 148.2, 174.9] },
  x55: { dir: "SJ+X55", cuts: [0.3, 26.9, 53.5, 80.1, 107.3] },
  x2000: { dir: "X2000_upgrade", cuts: [-0.2, 17.5, 42.5, 67.5, 92.5, 117.5, 142.5, 165.2] },
};

const source = process.argv[2];
if (!source) {
  console.error("usage: bun run build:trains <folder with the models' Collada exports>");
  process.exit(1);
}

/** The triangles of one car, `[y0, y1]` along the model, turned to face +y and scaled into the unit box. */
function carOf({ positions, colors }, [y0, y1], xMid, zLow) {
  const length = y1 - y0;
  const mid = (y0 + y1) / 2;
  const out = [];
  const outColors = [];
  for (let t = 0; t < positions.length / 9; t++) {
    const o = t * 9;
    const y = (positions[o + 1] + positions[o + 4] + positions[o + 7]) / 3;
    if (y < y0 || y >= y1) continue;
    for (let v = 0; v < 3; v++) {
      // half a turn about z: the model's low-y end leads
      out.push(-(positions[o + v * 3] - xMid) / CAR_WIDTH_M, -(positions[o + v * 3 + 1] - mid) / length, (positions[o + v * 3 + 2] - zLow) / CAR_HEIGHT_M);
    }
    outColors.push(colors[t * 3], colors[t * 3 + 1], colors[t * 3 + 2]);
  }
  return { length, positions: Float32Array.from(out), colors: Float32Array.from(outColors) };
}

/** Vertices snapped to a grid of `cell` (unit-box units, along y scaled by `stretch`) and merged; each triangle keeps its colour. */
function cluster(positions, colors, cell, stretch) {
  const cells = new Map();
  const key = (i) =>
    `${Math.floor(positions[i] / cell)},${Math.floor((positions[i + 1] * stretch) / cell)},${Math.floor(positions[i + 2] / cell)}`;
  for (let i = 0; i < positions.length; i += 3) {
    const k = key(i);
    const sum = cells.get(k) ?? [0, 0, 0, 0];
    for (let c = 0; c < 3; c++) sum[c] += positions[i + c];
    sum[3]++;
    cells.set(k, sum);
  }
  const faces = [];
  const seen = new Set();
  for (let t = 0; t < positions.length / 9; t++) {
    const k = [0, 1, 2].map((v) => key(t * 9 + v * 3));
    if (k[0] === k[1] || k[1] === k[2] || k[0] === k[2]) continue;
    const id = [...k].sort().join("|");
    if (seen.has(id)) continue;
    seen.add(id);
    faces.push({ points: k.map((c) => cells.get(c)).map((s) => [s[0] / s[3], s[1] / s[3], s[2] / s[3]]), color: [colors[t * 3], colors[t * 3 + 1], colors[t * 3 + 2]] });
  }
  return faces;
}

/** The cell size that leaves about `TRIANGLES`, searched for (how many survive depends on the shape). */
function decimate(car) {
  // cells cubic in metres: y is the car's length to the unit box's 1, x and z about 3 and 4
  const stretch = car.length / CAR_WIDTH_M;
  let best = null;
  let [low, high] = [0.002, 0.5];
  for (let step = 0; step < 20; step++) {
    const cell = Math.sqrt(low * high);
    const faces = cluster(car.positions, car.colors, cell, stretch);
    if (!best || Math.abs(faces.length - TRIANGLES) < Math.abs(best.length - TRIANGLES)) best = faces;
    if (faces.length > TRIANGLES) low = cell;
    else high = cell;
    if (Math.abs(faces.length - TRIANGLES) <= TRIANGLES * 0.05) break;
  }
  return best;
}

/**
 * Flat-shaded and unindexed: positions as floats, normals and colours as
 * bytes. A normal is divided by the scale the map draws the car at
 * (`[width, length, height]`), which the layer multiplies back in, so the
 * light falls as it would on the car in metres.
 */
function packCar(faces, length) {
  const positions = new Float32Array(faces.length * 9);
  const normals = new Int8Array(faces.length * 9);
  const colors = new Uint8Array(faces.length * 9);
  const scale = [CAR_WIDTH_M, length, CAR_HEIGHT_M];
  faces.forEach(({ points, color }, f) => {
    // the normal in metres
    const m = points.map((p) => p.map((v, c) => v * scale[c]));
    const u = [0, 1, 2].map((c) => m[1][c] - m[0][c]);
    const w = [0, 1, 2].map((c) => m[2][c] - m[0][c]);
    const n = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]].map((v, c) => v / scale[c]);
    const len = Math.hypot(...n) || 1;
    for (let v = 0; v < 3; v++) {
      for (let c = 0; c < 3; c++) {
        positions[f * 9 + v * 3 + c] = points[v][c];
        normals[f * 9 + v * 3 + c] = Math.round((n[c] / len) * 127);
        colors[f * 9 + v * 3 + c] = Math.round(Math.min(1, Math.max(0, color[c])) * 255);
      }
    }
  });
  return { positions, normals, colors };
}

const sections = [];
let at = 0;
/** Appends a section to `meshes.bin`, 4-byte aligned, and returns its offset. */
function append(array) {
  const offset = at;
  const bytes = Buffer.from(array.buffer, array.byteOffset, array.byteLength);
  const pad = Buffer.alloc((4 - (bytes.length % 4)) % 4);
  sections.push(bytes, pad);
  at += bytes.length + pad.length;
  return offset;
}

const index = { models: {} };
for (const [name, { dir, cuts }] of Object.entries(MODELS)) {
  const model = readCollada(join(source, dir, "model.dae"));
  let [xLo, xHi, zLo] = [Infinity, -Infinity, Infinity];
  for (let i = 0; i < model.positions.length; i += 3) {
    xLo = Math.min(xLo, model.positions[i]);
    xHi = Math.max(xHi, model.positions[i]);
    zLo = Math.min(zLo, model.positions[i + 2]);
  }
  const all = cuts.slice(1).map((y1, i) => [cuts[i], y1]);
  const kept = [all[0], all[Math.min(1, all.length - 1)], all[all.length - 1]];
  index.models[name] = {
    cars: kept.map((span) => {
      const car = carOf(model, span, (xLo + xHi) / 2, zLo);
      const packed = packCar(decimate(car), car.length);
      return {
        length: Math.round(car.length * 100) / 100,
        vertices: packed.positions.length / 3,
        positions: append(packed.positions),
        normals: append(packed.normals),
        colors: append(packed.colors),
      };
    }),
  };
  console.log(`${name.padEnd(16)} ${all.length} cars, kept ${index.models[name].cars.map((c) => `${c.length} m/${c.vertices / 3}`).join(", ")}`);
}

mkdirSync(OUT, { recursive: true });
writeFileSync(new URL("meshes.bin", OUT), Buffer.concat(sections));
writeFileSync(new URL("index.json", OUT), JSON.stringify(index));
console.log(`static/trains: ${(at / 1e6).toFixed(2)} MB`);
