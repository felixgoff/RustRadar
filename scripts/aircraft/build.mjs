// Turns the Flightradar24 3D models into the two things RustRadar draws:
// low-poly meshes at two levels of detail, and top-down silhouettes for the
// flat map view. Run with `bun run build:aircraft`; the generated files go to
// `static/aircraft/`.
//
// The published models are far too detailed to instance thousands of times
// (a 737 is 25k triangles), so each is reduced by vertex clustering: vertices
// are snapped to a grid, merged, and the surviving triangles re-normalled.
// Shape and silhouette survive that; surface detail does not, which is the
// right trade for aircraft a few dozen pixels tall.
import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { deflateSync } from "node:zlib";
import { readTriangles } from "./gltf1.mjs";

const SOURCE = "https://raw.githubusercontent.com/Flightradar24/fr24-3d-models/master/models";
const CACHE = new URL("../../.cache/fr24-models/", import.meta.url);
const OUT = new URL("../../static/aircraft/", import.meta.url);

/** Triangle budget per level of detail. */
const LODS = [220, 2600];
/** Pixels per silhouette cell in the atlas. */
const ICON = 128;
const PARTS = ["top", "belly", "wings", "tail", "engines"];

/**
 * Which published model stands in for each aircraft type. Keys are ICAO type
 * designators as Flightradar24 reports them, plus its icon classes as a
 * fallback for the long tail of types with no model of their own.
 */
const MODELS = {
  // Airbus
  A318: "a318", A319: "a319", A19N: "a319", A320: "a320", A20N: "a320",
  A321: "a321", A21N: "a321", A332: "a332", A333: "a333", A338: "a333",
  A339: "a333", A342: "a343", A343: "a343", A345: "a346", A346: "a346",
  A359: "a359", A35K: "a359", A388: "a380", A3ST: "beluga", A337: "beluga",
  // Boeing
  B736: "b736", B737: "b737", B38M: "b738", B738: "b738", B739: "b739",
  B39M: "b739", B37M: "b737", B744: "b744", B748: "b748", B741: "b744",
  B742: "b744", B743: "b744", B74S: "b744", B752: "b752", B753: "b753",
  B762: "b762", B763: "b763", B764: "b764", B772: "b772", B77L: "b772",
  B773: "b773", B77W: "b773", B778: "b773", B779: "b773", B788: "b788",
  B789: "b789", B78X: "b789",
  // regional and turboprop
  BA46: "bae146", B461: "bae146", B462: "bae146", B463: "bae146",
  RJ1H: "bae146", RJ85: "bae146", RJ70: "bae146",
  CRJ7: "crj700", CRJ9: "crj900", CRJX: "crj900", CRJ2: "crj700", CRJ1: "crj700",
  BCS1: "cs100", BCS3: "cs300", E170: "e170", E75L: "e170", E75S: "e170",
  E190: "e190", E195: "e190", E290: "e190", E295: "e190",
  DH8D: "q400", DH8A: "q400", DH8B: "q400", DH8C: "q400",
  AT43: "atr42", AT45: "atr42", AT46: "atr42", AT72: "atr42", AT75: "atr42", AT76: "atr42",
  // light, business and other
  C25A: "citation", C25B: "citation", C25C: "citation", C500: "citation",
  C510: "citation", C525: "citation", C550: "citation", C560: "citation",
  C56X: "citation", C650: "citation", C680: "citation", C68A: "citation",
  C700: "citation", C750: "citation", LJ60: "citation", LJ45: "citation",
  P28A: "pa28", P28B: "pa28", PA28: "pa28", C172: "pa28", C152: "pa28",
  C182: "pa28", C206: "pa28", SR20: "pa28", SR22: "pa28", DA40: "pa28",
  A225: "an225", AN22: "an225",
  // rear-engined T-tails: the CRJ is the nearest published shape
  B712: "crj900", MD82: "crj900", MD83: "crj900", MD87: "crj900", MD88: "crj900",
  MD90: "crj900", E135: "crj700", E145: "crj700", E45X: "crj700", E170: "e170",
  // four-engined types with no model of their own
  IL76: "a343", IL96: "a343", K35R: "a343", E3TF: "b744", E6: "a343", C135: "a343",
  GL5T: "citation", GL7T: "citation", GLEX: "citation", GLF4: "citation",
  GLF5: "citation", GLF6: "citation", CL60: "citation", CL35: "citation",
  FA7X: "citation", FA8X: "citation", F2TH: "citation", H25B: "citation",
  PC12: "pa28", TBM9: "pa28", BE20: "atr42", B350: "atr42", C208: "pa28",
};

/** Icon class (Flightradar24's own grouping) to model, used when the type is unknown. */
const ICON_MODELS = {
  A320: "a320", B738: "b738", B736: "b736", B757: "b752", B777: "b772",
  B747: "b744", A330: "a332", A340: "a343", A343: "a343", A380: "a380",
  A3ST: "beluga", A225: "an225", MD11: "b763", Q300: "q400", LJ60: "citation",
  C206: "pa28", C303: "pa28", EC: "heli",
  // the live feed spells a few of these out in full
  F100: "bae146", FOKKER100: "bae146", RJ85: "bae146",
  AS20: "ask21", ASW20: "ask21", GLID: "ask21",
};

const ALL = [...new Set([...Object.values(MODELS), ...Object.values(ICON_MODELS)])].sort();

async function source(name) {
  mkdirSync(CACHE, { recursive: true });
  const ext = name === "an225" ? "gltf" : "glb";
  const file = new URL(`${name}.${ext}`, CACHE);
  if (!existsSync(file)) {
    const response = await fetch(`${SOURCE}/${name}.${ext}`);
    if (!response.ok) throw new Error(`${name}: ${response.status}`);
    writeFileSync(file, new Uint8Array(await response.arrayBuffer()));
  }
  return new Uint8Array(readFileSync(file));
}

const bounds = (p, stride = 3) => {
  const lo = [Infinity, Infinity, Infinity];
  const hi = [-Infinity, -Infinity, -Infinity];
  for (let i = 0; i < p.length; i += stride) {
    for (let c = 0; c < 3; c++) {
      if (p[i + c] < lo[c]) lo[c] = p[i + c];
      if (p[i + c] > hi[c]) hi[c] = p[i + c];
    }
  }
  return { lo, hi, size: hi.map((v, c) => v - lo[c]) };
};

/**
 * Rotates a model into RustRadar's frame: nose along +y, right wing +x, up +z,
 * centred on its bounding box. The published models share a convention but do
 * not promise it, so the axes are found from the shape: aircraft are flat, so
 * "up" is the shallowest axis, and the tallest point sits on the centreline at
 * the tail, which names the other two and which way the nose points.
 */
function orient(positions, sourceNormals) {
  const { lo, hi, size } = bounds(positions);
  const up = size.indexOf(Math.min(...size));
  const [a, b] = [0, 1, 2].filter((c) => c !== up);
  let peak = -Infinity;
  let at = null;
  for (let i = 0; i < positions.length; i += 3) {
    if (positions[i + up] > peak) {
      peak = positions[i + up];
      at = [positions[i], positions[i + 1], positions[i + 2]];
    }
  }
  // the fin is central across the span and far along the length
  const offset = (c) => Math.abs((at[c] - (lo[c] + hi[c]) / 2) / (size[c] || 1));
  const [length, span] = offset(a) >= offset(b) ? [a, b] : [b, a];
  // the tall point is the tail, so the nose is the other way
  const nose = at[length] > (lo[length] + hi[length]) / 2 ? -1 : 1;
  // keep the frame right-handed: flipping the nose flips the span with it
  const out = new Float32Array(positions.length);
  const normals = new Float32Array(positions.length);
  for (let i = 0; i < positions.length; i += 3) {
    out[i] = (positions[i + span] - (lo[span] + hi[span]) / 2) * nose;
    out[i + 1] = (positions[i + length] - (lo[length] + hi[length]) / 2) * nose;
    out[i + 2] = positions[i + up] - (lo[up] + hi[up]) / 2;
    // directions take the same permutation, without the centring
    normals[i] = sourceNormals[i + span] * nose;
    normals[i + 1] = sourceNormals[i + length] * nose;
    normals[i + 2] = sourceNormals[i + up];
  }
  return { positions: out, normals };
}

/**
 * Where the fuselage sits vertically, and how thick it is. The bounding box
 * is no guide: the fin makes an airliner look twice as tall as its body, which
 * would drop the whole cabin below centre. Measuring ahead of the wing root
 * and near the centreline sees the barrel alone.
 */
function waistline(triangles, span, length) {
  let lo = Infinity;
  let hi = -Infinity;
  for (let i = 0; i < triangles.length; i += 3) {
    if (Math.abs(triangles[i]) < 0.08 * span && triangles[i + 1] > 0.15 * length) {
      lo = Math.min(lo, triangles[i + 2]);
      hi = Math.max(hi, triangles[i + 2]);
    }
  }
  return Number.isFinite(lo) ? { centre: (lo + hi) / 2, radius: Math.max((hi - lo) / 2, 1e-3) } : { centre: 0, radius: 1 };
}

/** Which livery part a triangle belongs to, from where it sits on the airframe. */
function partOf(cx, cy, cz, span, length, waist) {
  const above = cz - waist.centre;
  // anything well off the centreline is wing, stabiliser or engine
  if (Math.abs(cx) > 0.1 * span) return "wings";
  // the fin: on the centreline, aft, and clear of the top of the fuselage
  if (cy < -0.1 * length && above > 0.8 * waist.radius) return "tail";
  return above < 0 ? "belly" : "top";
}

/**
 * Vertex clustering down to roughly `target` triangles: snap to a grid, merge
 * each cell to one vertex, drop the triangles that collapse. The cell size is
 * searched for, because how many triangles survive depends on the shape.
 */
function decimate(triangles, normals, parts, target) {
  const { lo, size } = bounds(triangles);
  const diagonal = Math.hypot(...size);
  let best = null;
  let low = diagonal / 400;
  let high = diagonal / 3;
  for (let step = 0; step < 18; step++) {
    const cell = Math.sqrt(low * high);
    const result = cluster(triangles, normals, parts, lo, cell);
    if (!best || Math.abs(result.count - target) < Math.abs(best.count - target)) best = result;
    if (result.count > target) low = cell;
    else high = cell;
    if (Math.abs(result.count - target) <= target * 0.06) break;
  }
  return best;
}

function cluster(triangles, normals, parts, lo, cell) {
  const cells = new Map();
  const key = (x, y, z) =>
    `${Math.floor((x - lo[0]) / cell)},${Math.floor((y - lo[1]) / cell)},${Math.floor((z - lo[2]) / cell)}`;
  // average the vertices that land in each cell, so the hull keeps its shape,
  // and their authored normals with them
  for (let i = 0; i < triangles.length; i += 3) {
    const k = key(triangles[i], triangles[i + 1], triangles[i + 2]);
    const sum = cells.get(k) ?? [0, 0, 0, 0, 0, 0, 0];
    for (let c = 0; c < 3; c++) {
      sum[c] += triangles[i + c];
      sum[3 + c] += normals[i + c];
    }
    sum[6]++;
    cells.set(k, sum);
  }
  const index = new Map();
  const points = [];
  const merged = [];
  for (const [k, sum] of cells) {
    index.set(k, points.length / 3);
    points.push(sum[0] / sum[6], sum[1] / sum[6], sum[2] / sum[6]);
    const len = Math.hypot(sum[3], sum[4], sum[5]) || 1;
    merged.push(sum[3] / len, sum[4] / len, sum[5] / len);
  }
  const faces = [];
  const seen = new Set();
  for (let t = 0; t < triangles.length / 9; t++) {
    const v = [0, 1, 2].map((c) => index.get(key(triangles[t * 9 + c * 3], triangles[t * 9 + c * 3 + 1], triangles[t * 9 + c * 3 + 2])));
    if (v[0] === v[1] || v[1] === v[2] || v[0] === v[2]) continue;
    const id = [...v].sort((p, q) => p - q).join(",");
    if (seen.has(id)) continue;
    seen.add(id);
    faces.push({ v, part: parts[t] });
  }
  return { points: Float32Array.from(points), normals: Float32Array.from(merged), faces, count: faces.length };
}

/** Packs one level of detail: vertices shared, triangles grouped by livery part. */
function pack(points, vertexNormals, faces) {
  const order = PARTS.filter((p) => faces.some((f) => f.part === p));
  const indices = [];
  const ranges = {};
  for (const part of order) {
    const start = indices.length / 3;
    for (const face of faces) if (face.part === part) indices.push(...face.v);
    ranges[part] = [start, indices.length / 3 - start];
  }
  const bytes = new Int8Array(points.length);
  for (let i = 0; i < vertexNormals.length; i += 3) {
    const len = Math.hypot(vertexNormals[i], vertexNormals[i + 1], vertexNormals[i + 2]) || 1;
    for (let c = 0; c < 3; c++) bytes[i + c] = Math.max(-127, Math.round((vertexNormals[i + c] / len) * 127));
  }
  return { points, normals: bytes, indices: Uint16Array.from(indices), ranges };
}

/** Fills the top-down outline of a model into an alpha mask. */
function silhouette(triangles, size, cellPx) {
  const scale = (cellPx * 0.94) / size;
  const supersample = 3;
  const n = cellPx * supersample;
  const mask = new Uint8Array(n * n);
  const toPixel = (x, y) => [n / 2 + x * scale * supersample, n / 2 - y * scale * supersample];
  for (let t = 0; t < triangles.length; t += 9) {
    const p = [0, 1, 2].map((c) => toPixel(triangles[t + c * 3], triangles[t + c * 3 + 1]));
    const minX = Math.max(0, Math.floor(Math.min(...p.map((v) => v[0]))));
    const maxX = Math.min(n - 1, Math.ceil(Math.max(...p.map((v) => v[0]))));
    const minY = Math.max(0, Math.floor(Math.min(...p.map((v) => v[1]))));
    const maxY = Math.min(n - 1, Math.ceil(Math.max(...p.map((v) => v[1]))));
    const area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
    if (Math.abs(area) < 1e-9) continue;
    for (let y = minY; y <= maxY; y++) {
      for (let x = minX; x <= maxX; x++) {
        const px = x + 0.5, py = y + 0.5;
        const w0 = ((p[1][0] - p[0][0]) * (py - p[0][1]) - (px - p[0][0]) * (p[1][1] - p[0][1])) / area;
        const w1 = ((px - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (py - p[0][1])) / area;
        if (w0 >= 0 && w1 >= 0 && w0 + w1 <= 1) mask[y * n + x] = 1;
      }
    }
  }
  const out = new Uint8Array(cellPx * cellPx);
  for (let y = 0; y < cellPx; y++) {
    for (let x = 0; x < cellPx; x++) {
      let hit = 0;
      for (let dy = 0; dy < supersample; dy++) {
        for (let dx = 0; dx < supersample; dx++) hit += mask[(y * supersample + dy) * n + x * supersample + dx];
      }
      out[y * cellPx + x] = Math.round((hit / (supersample * supersample)) * 255);
    }
  }
  return out;
}

const crcTable = Array.from({ length: 256 }, (_, i) => {
  let c = i;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = (buf) => {
  let c = 0xffffffff;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};

/** Minimal RGBA PNG writer: white pixels carrying the mask as alpha. */
function png(width, height, alpha) {
  const raw = Buffer.alloc(height * (width * 4 + 1));
  for (let y = 0; y < height; y++) {
    const row = y * (width * 4 + 1);
    for (let x = 0; x < width; x++) {
      const at = row + 1 + x * 4;
      raw[at] = raw[at + 1] = raw[at + 2] = 255;
      raw[at + 3] = alpha[y * width + x];
    }
  }
  const chunk = (type, data) => {
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(body));
    return Buffer.concat([length, body, crc]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = 6; // RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

const meshes = [];
const index = { models: {}, types: MODELS, icons: ICON_MODELS, lods: LODS, icon: { cell: ICON, names: [] } };
const icons = [];

for (const name of ALL) {
  let triangles;
  let normals;
  try {
    const read = readTriangles(await source(name));
    ({ positions: triangles, normals } = orient(read.positions, read.normals));
  } catch (e) {
    console.warn(`skipped ${name}: ${e.message}`);
    continue;
  }
  const { size } = bounds(triangles);
  const [span, length] = size;
  const waist = waistline(triangles, span, length);
  const parts = [];
  for (let t = 0; t < triangles.length; t += 9) {
    let cx = 0, cy = 0, cz = 0;
    for (let c = 0; c < 3; c++) {
      cx += triangles[t + c * 3] / 3;
      cy += triangles[t + c * 3 + 1] / 3;
      cz += triangles[t + c * 3 + 2] / 3;
    }
    parts.push(partOf(cx, cy, cz, span, length, waist));
  }

  const tally = parts.reduce((n, p) => ({ ...n, [p]: (n[p] ?? 0) + 1 }), {});
  const model = { size: Number(Math.max(span, length).toFixed(2)), span: Number(span.toFixed(2)), length: Number(length.toFixed(2)), lods: [] };
  for (const target of LODS) {
    const { points, normals: merged, faces } = decimate(triangles, normals, parts, target);
    const packed = pack(points, merged, faces);
    // every section starts 4-byte aligned, so the frontend can map typed
    // arrays straight onto the file without copying
    const pad = (n) => Buffer.alloc((4 - (n % 4)) % 4);
    const raw = (a) => Buffer.from(a.buffer, a.byteOffset, a.byteLength);
    const base = meshes.reduce((n, b) => n + b.length, 0);
    const normalsAt = base + packed.points.byteLength;
    const indicesAt = normalsAt + packed.normals.byteLength + pad(packed.normals.byteLength).length;
    meshes.push(
      Buffer.concat([
        raw(packed.points),
        raw(packed.normals),
        pad(packed.normals.byteLength),
        raw(packed.indices),
        pad(packed.indices.byteLength),
      ]),
    );
    model.lods.push({
      positions: base,
      normals: normalsAt,
      indices: indicesAt,
      vertices: packed.points.length / 3,
      triangles: packed.indices.length / 3,
      parts: packed.ranges,
    });
  }
  index.models[name] = model;
  index.icon.names.push(name);
  icons.push(silhouette(triangles, Math.max(span, length), ICON));
  console.log(
    `${name.padEnd(10)} ${String(triangles.length / 9).padStart(6)} tris  ->  ` +
      model.lods.map((l) => String(l.triangles).padStart(5)).join(" / ") +
      `   ${span.toFixed(1)} x ${length.toFixed(1)} m   ` +
      PARTS.filter((p) => tally[p]).map((p) => `${p}:${Math.round((tally[p] / parts.length) * 100)}%`).join(" "),
  );
}

const columns = Math.ceil(Math.sqrt(icons.length));
const rows = Math.ceil(icons.length / columns);
const atlas = new Uint8Array(columns * ICON * rows * ICON);
icons.forEach((mask, i) => {
  const [cx, cy] = [(i % columns) * ICON, Math.floor(i / columns) * ICON];
  for (let y = 0; y < ICON; y++) {
    atlas.set(mask.subarray(y * ICON, y * ICON + ICON), (cy + y) * columns * ICON + cx);
  }
});
index.icon.columns = columns;

mkdirSync(OUT, { recursive: true });
writeFileSync(new URL("meshes.bin", OUT), Buffer.concat(meshes));
writeFileSync(new URL("icons.png", OUT), png(columns * ICON, rows * ICON, atlas));
writeFileSync(new URL("index.json", OUT), JSON.stringify(index));
console.log(
  `\n${index.icon.names.length} models · meshes.bin ${(Buffer.concat(meshes).length / 1024).toFixed(0)} KB · ` +
    `icons.png ${(png(columns * ICON, rows * ICON, atlas).length / 1024).toFixed(0)} KB (${columns}x${rows})`,
);
