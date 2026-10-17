// Bridges in the air and tunnels underground.
//
// A vector tile's lines split into those on the ground and those on a bridge,
// the bridges raised to their deck (rising to it from each end, so they meet
// the ground where their ways do), and a copy of them flat, for their shadow
// on what they cross. For deck.gl's tiles in the flat projection, whose
// coordinates are the tile's own (0 to 1 across it) and whose heights are
// deck.gl's common units.
//
// And the app's own bridges and tunnels on the track (`Structure`, from the
// backend's tiles), for what is drawn on it: the signal blocks and the trains.

const EARTH_CIRCUMFERENCE = 40_075_016.686;
/** Up to its deck over this much of each end, metres (less on a short bridge). */
const RAMP_M = 40;
/** A raised line is cut into pieces no longer than this, so it can rise along them, metres. */
const PIECE_M = 6;

/** A railway bridge's deck, metres (the map's data doesn't say how high). */
export const RAIL_DECK_M = 6;
/** A bridge's deck, metres: a level more for each `layer` above the first (a bridge over a bridge). */
const DECK_M = 6;

/** OpenMapTiles' bridges (`brunnel`): railways at a railway's deck, the rest a deck a `layer`. */
export const omtDeck: DeckHeight = (p, numeric) =>
  p.brunnel !== "bridge"
    ? 0
    : p.class === "rail" || p.class === "transit"
      ? RAIL_DECK_M
      : DECK_M * Math.max(1, Math.min(4, numeric("layer") ?? Number(p.layer ?? 1)));
/** OpenRailwayMap's bridges (`bridge`). */
export const ormDeck: DeckHeight = (p) => (p.bridge === true ? RAIL_DECK_M : 0);

/**
 * How high a bridge is `d` metres along it, of `total`: up to its deck over
 * a ramp at each end, but not at an end where it goes on (`on`).
 */
function rise(d: number, total: number, deck: number, on: readonly [boolean, boolean]): number {
  const ramp = Math.min(RAMP_M, total * 0.3);
  if (ramp <= 0) return deck;
  const k = Math.max(0, Math.min(1, on[0] ? 1 : d / ramp, on[1] ? 1 : (total - d) / ramp));
  return deck * k * k * (3 - 2 * k);
}

type Column = { value: ArrayLike<number>; size: number };
/** A binary tile's lines (loaders.gl's binary GeoJSON), as far as they are read here. */
export type Lines = {
  type: string;
  positions: Column;
  pathIndices: Column;
  featureIds: Column;
  globalFeatureIds: Column;
  numericProps: Record<string, Column>;
  properties: Record<string, unknown>[];
  fields?: unknown[];
};
export type TileIndex = { x: number; y: number; z: number };
/** How high a line's deck is, metres; 0 on the ground. `numeric` reads a numeric property at one of its vertices. */
export type DeckHeight = (properties: Record<string, unknown>, numeric: (name: string) => number | undefined) => number;

/** Lines being gathered: their columns as plain arrays. */
class Builder {
  positions: number[] = [];
  pathIndices: number[] = [0];
  featureIds: number[] = [];
  globalFeatureIds: number[] = [];
  numeric: Record<string, number[]>;
  constructor(
    private from: Lines,
    readonly size: number,
  ) {
    this.numeric = Object.fromEntries(Object.keys(from.numericProps).map((k) => [k, []]));
  }
  /** One vertex, its attributes those of vertex `i` of the source. */
  push(i: number, x: number, y: number, z?: number) {
    this.positions.push(x, y);
    if (this.size === 3) this.positions.push(z ?? 0);
    this.featureIds.push(this.from.featureIds.value[i]);
    this.globalFeatureIds.push(this.from.globalFeatureIds.value[i]);
    for (const k in this.numeric) this.numeric[k].push(this.from.numericProps[k].value[i]);
  }
  end() {
    if (this.positions.length / this.size > this.pathIndices[this.pathIndices.length - 1]) this.pathIndices.push(this.positions.length / this.size);
  }
  build(): Lines {
    return {
      ...this.from,
      positions: { value: new Float64Array(this.positions), size: this.size },
      pathIndices: { value: new Uint32Array(this.pathIndices), size: 1 },
      featureIds: { value: new Uint32Array(this.featureIds), size: 1 },
      globalFeatureIds: { value: new Uint32Array(this.globalFeatureIds), size: 1 },
      numericProps: Object.fromEntries(Object.entries(this.numeric).map(([k, v]) => [k, { value: new Float64Array(v), size: 1 }])),
    };
  }
}

const cache = new WeakMap<object, { ground: Lines; raised: Lines; under: Lines } | null>();

/**
 * `lines` split by `height`: those on the ground, the bridges raised (or null
 * when there are none, for the layers to be left as they were).
 */
export function bridgeParts(lines: Lines, tile: TileIndex, height: DeckHeight): { ground: Lines; raised: Lines; under: Lines } | null {
  if (cache.has(lines)) return cache.get(lines)!;
  const size = lines.positions.size;
  const pos = lines.positions.value;
  const paths = lines.pathIndices.value;
  // metres across the tile, at its middle's latitude; and common units a metre there
  const n = 2 ** tile.z;
  const lat = Math.atan(Math.sinh(Math.PI * (1 - (2 * (tile.y + 0.5)) / n)));
  const tileMetres = (EARTH_CIRCUMFERENCE * Math.cos(lat)) / n;
  const commonPerMetre = 512 / (EARTH_CIRCUMFERENCE * Math.cos(lat));
  const ground = new Builder(lines, size);
  const raised = new Builder(lines, 3);
  const under = new Builder(lines, size);
  let any = false;
  for (let p = 0; p + 1 < paths.length; p++) {
    const [start, end] = [paths[p], paths[p + 1]];
    if (end - start < 2) continue;
    const props = lines.properties[lines.featureIds.value[start]] ?? {};
    const deck = height(props, (name) => lines.numericProps[name]?.value[start]);
    if (deck <= 0) {
      for (let i = start; i < end; i++) ground.push(i, pos[i * size], pos[i * size + 1], size === 3 ? pos[i * size + 2] : undefined);
      ground.end();
      continue;
    }
    any = true;
    // its length, and how far along each vertex is
    const along = [0];
    for (let i = start + 1; i < end; i++) {
      const [dx, dy] = [pos[i * size] - pos[(i - 1) * size], pos[i * size + 1] - pos[(i - 1) * size + 1]];
      along.push(along[along.length - 1] + Math.hypot(dx, dy) * tileMetres);
    }
    const total = along[along.length - 1];
    // an end where the tile cuts the bridge stays up: the rest of it is the next tile's
    const cut = (i: number) => {
      const [x, y] = [pos[i * size], pos[i * size + 1]];
      return x <= 0.001 || x >= 0.999 || y <= 0.001 || y >= 0.999;
    };
    const on = [cut(start), cut(end - 1)] as const;
    const up = (d: number) => rise(d, total, deck, on) * commonPerMetre;
    for (let i = start; i < end; i++) {
      const [x, y] = [pos[i * size], pos[i * size + 1]];
      under.push(i, x, y, size === 3 ? pos[i * size + 2] : undefined);
      if (i > start) {
        // the way there in pieces, rising as it goes
        const [px, py] = [pos[(i - 1) * size], pos[(i - 1) * size + 1]];
        const [d0, d1] = [along[i - 1 - start], along[i - start]];
        const pieces = Math.ceil((d1 - d0) / PIECE_M);
        for (let k = 1; k < pieces; k++) {
          const t = k / pieces;
          raised.push(i - 1, px + (x - px) * t, py + (y - py) * t, up(d0 + (d1 - d0) * t));
        }
      }
      raised.push(i, x, y, up(along[i - start]));
    }
    raised.end();
    under.end();
  }
  const out = any ? { ground: ground.build(), raised: raised.build(), under: under.build() } : null;
  cache.set(lines, out);
  return out;
}

type Coord = number[];
type LineFeature = { type?: string; geometry: { type: string; coordinates: unknown }; properties: Record<string, unknown> };
const featureCache = new WeakMap<object, { ground: LineFeature[]; raised: LineFeature[] } | null>();

/**
 * The same for a tile's GeoJSON features (in its own coordinates): those on
 * the ground, and the bridges' raised to their decks (or null when there are
 * none).
 */
export function raiseFeatures(features: LineFeature[], tile: TileIndex, height: DeckHeight): { ground: LineFeature[]; raised: LineFeature[] } | null {
  if (featureCache.has(features)) return featureCache.get(features)!;
  const n = 2 ** tile.z;
  const lat = Math.atan(Math.sinh(Math.PI * (1 - (2 * (tile.y + 0.5)) / n)));
  const tileMetres = (EARTH_CIRCUMFERENCE * Math.cos(lat)) / n;
  const commonPerMetre = 512 / (EARTH_CIRCUMFERENCE * Math.cos(lat));
  const cut = (p: Coord) => p[0] <= 0.001 || p[0] >= 0.999 || p[1] <= 0.001 || p[1] >= 0.999;
  const raise = (line: Coord[], deck: number): Coord[] => {
    const along = [0];
    for (let i = 1; i < line.length; i++) along.push(along[i - 1] + Math.hypot(line[i][0] - line[i - 1][0], line[i][1] - line[i - 1][1]) * tileMetres);
    const total = along[along.length - 1];
    const on = [cut(line[0]), cut(line[line.length - 1])] as const;
    const up = (d: number) => rise(d, total, deck, on) * commonPerMetre;
    const out: Coord[] = [[line[0][0], line[0][1], up(0)]];
    for (let i = 1; i < line.length; i++) {
      const pieces = Math.max(1, Math.ceil((along[i] - along[i - 1]) / PIECE_M));
      for (let k = 1; k <= pieces; k++) {
        const t = k / pieces;
        out.push([line[i - 1][0] + (line[i][0] - line[i - 1][0]) * t, line[i - 1][1] + (line[i][1] - line[i - 1][1]) * t, up(along[i - 1] + (along[i] - along[i - 1]) * t)]);
      }
    }
    return out;
  };
  const [ground, raised]: LineFeature[][] = [[], []];
  for (const f of features) {
    const deck = height(f.properties ?? {}, (name) => {
      const v = f.properties?.[name];
      return typeof v === "number" ? v : undefined;
    });
    const g = f.geometry;
    if (deck <= 0 || (g.type !== "LineString" && g.type !== "MultiLineString")) {
      ground.push(f);
      continue;
    }
    const coordinates = g.type === "LineString" ? raise(g.coordinates as Coord[], deck) : (g.coordinates as Coord[][]).map((l) => raise(l, deck));
    raised.push({ ...f, geometry: { type: g.type, coordinates } });
  }
  const result = raised.length ? { ground, raised } : null;
  featureCache.set(features, result);
  return result;
}

// --- on the track ------------------------------------------------------------

type LonLat = [number, number];
/** A bridge or a tunnel on the track, as the backend sends it: its line, and whether it goes on as one past each end. */
export type Structure = { tunnel: boolean; line: LonLat[]; joined: [boolean, boolean] };
/** Where a place on the track is: in a tunnel, or this high (metres); on the ground, neither. */
export type Level = { tunnel: boolean; height: number };
export const GROUND: Level = { tunnel: false, height: 0 };

const M_PER_DEGREE = 111_320;
/** Turned more than this from a structure's line, a way crosses it rather than runs along it, degrees. */
const ALONG_DEG = 30;

type Measured = { box: [number, number, number, number]; along: number[] };
const measured = new WeakMap<Structure, Measured>();
function measure(s: Structure): Measured {
  let m = measured.get(s);
  if (m) return m;
  const box: Measured["box"] = [Infinity, Infinity, -Infinity, -Infinity];
  const along = [0];
  s.line.forEach(([lon, lat], i) => {
    box[0] = Math.min(box[0], lon);
    box[1] = Math.min(box[1], lat);
    box[2] = Math.max(box[2], lon);
    box[3] = Math.max(box[3], lat);
    if (i > 0) {
      const [plon, plat] = s.line[i - 1];
      along.push(along[i - 1] + Math.hypot((lon - plon) * Math.cos((lat * Math.PI) / 180), lat - plat) * M_PER_DEGREE);
    }
  });
  measured.set(s, (m = { box, along }));
  return m;
}

/**
 * The level at `p` going `heading` (degrees; null: either way): that of the
 * bridge or tunnel whose line is nearest, within `within` metres and running
 * the same way (not one it passes under or over); else the ground.
 */
export function levelAt(p: LonLat, heading: number | null, structures: readonly Structure[], within = 4): Level {
  if (!structures.length) return GROUND;
  const k = Math.cos((p[1] * Math.PI) / 180);
  const pad = within / M_PER_DEGREE;
  let best = Infinity;
  let level = GROUND;
  for (const s of structures) {
    const { box, along } = measure(s);
    if (p[0] < box[0] - pad / k || p[0] > box[2] + pad / k || p[1] < box[1] - pad || p[1] > box[3] + pad) continue;
    for (let i = 1; i < s.line.length; i++) {
      const [ax, ay] = [(s.line[i - 1][0] - p[0]) * k * M_PER_DEGREE, (s.line[i - 1][1] - p[1]) * M_PER_DEGREE];
      const [dx, dy] = [(s.line[i][0] - s.line[i - 1][0]) * k * M_PER_DEGREE, (s.line[i][1] - s.line[i - 1][1]) * M_PER_DEGREE];
      const len2 = dx * dx + dy * dy;
      const t = len2 > 0 ? Math.max(0, Math.min(1, -(ax * dx + ay * dy) / len2)) : 0;
      const d = Math.hypot(ax + dx * t, ay + dy * t);
      if (d > within || d >= best) continue;
      if (heading !== null && len2 > 1) {
        const turn = Math.abs(((heading - (Math.atan2(dx, dy) * 180) / Math.PI) % 180) + 180) % 180;
        if (Math.min(turn, 180 - turn) > ALONG_DEG) continue;
      }
      best = d;
      level = s.tunnel ? { tunnel: true, height: 0 } : { tunnel: false, height: rise(along[i - 1] + Math.sqrt(len2) * t, along[along.length - 1], RAIL_DECK_M, s.joined) };
    }
  }
  return level;
}

/** The course from one place to another, degrees from north (flat: fine over a few km). */
function courseOf(a: LonLat, b: LonLat): number {
  return (Math.atan2((b[0] - a[0]) * Math.cos((a[1] * Math.PI) / 180), b[1] - a[1]) * 180) / Math.PI;
}

/**
 * A line on the track as it lies: cut where it goes into or out of a tunnel,
 * and up on the bridges along it (`[lon, lat, metres]`).
 */
export function onStructures(line: readonly LonLat[], structures: readonly Structure[]): { tunnel: boolean; path: [number, number, number][] }[] {
  if (!structures.length || line.length < 2) return [{ tunnel: false, path: line.map(([lon, lat]) => [lon, lat, 0]) }];
  const out: { tunnel: boolean; path: [number, number, number][] }[] = [];
  const bridges = structures.filter((s) => !s.tunnel).map((s) => measure(s).box);
  const pad = 10 / M_PER_DEGREE;
  for (let i = 1; i < line.length; i++) {
    const [a, b] = [line[i - 1], line[i]];
    const course = courseOf(a, b);
    // in pieces where it may be on a bridge, so it can rise along it
    const nearBridge = bridges.some(
      (box) => Math.max(a[0], b[0]) >= box[0] - pad * 2 && Math.min(a[0], b[0]) <= box[2] + pad * 2 && Math.max(a[1], b[1]) >= box[1] - pad && Math.min(a[1], b[1]) <= box[3] + pad,
    );
    const metres = Math.hypot((b[0] - a[0]) * Math.cos((a[1] * Math.PI) / 180), b[1] - a[1]) * M_PER_DEGREE;
    const pieces = nearBridge ? Math.max(1, Math.ceil(metres / PIECE_M)) : 1;
    for (let k = 0; k < pieces; k++) {
      const at = (t: number): LonLat => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
      const [from, to] = [at(k / pieces), at((k + 1) / pieces)];
      const tunnel = levelAt(at((k + 0.5) / pieces), course, structures).tunnel;
      const point = (q: LonLat): [number, number, number] => [q[0], q[1], levelAt(q, course, structures).height];
      const last = out[out.length - 1];
      if (last?.tunnel === tunnel) last.path.push(point(to));
      else out.push({ tunnel, path: [point(from), point(to)] });
    }
  }
  return out;
}
