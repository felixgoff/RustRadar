// Pitched roofs for Streets GL's buildings (streets-gl.ts), built in metres:
// the walls stop at the eaves, and over each footprint its roof's shape.
//
// A roof is the lowest of a few planes over the footprint's minimum-area
// rectangle: two slopes meeting at a ridge along its longer side (gabled),
// with the ends sloped in too (hipped), all four sides (pyramidal), or one
// slope (skillion). Each plane covers the part of the footprint where it is
// the lowest (the footprint clipped to it), and the walls are carried up
// under the roof wherever its edge stands above the eaves (the gables).
// Shapes Streets GL has and these don't come out as the nearest of them.

import { earcut } from "@math.gl/polygon";

type P = [number, number];

export type RoofInput = {
  /** The footprint, metres east and north of an origin (a closing repeat of the first point is ignored), and its courtyards. */
  ring: P[];
  holes?: P[][];
  /** The eaves, metres up, and the roof's height above them. */
  base: number;
  height: number;
  /** Streets GL's roof type. */
  shape: string;
  /** "across": the ridge across the longer side. */
  orientation?: string;
  /** Degrees from north the roof faces (a skillion's slope runs down that way). */
  direction?: number;
};

/** Triangles: their corners (east, north, up), each corner's normal and texture position, and which are walls (gables). */
export type RoofParts = { positions: number[]; normals: number[]; uvs: number[]; walls: boolean[] };

/** A plane over the ground: height in units of the roof's, `c + gx·x + gy·y`. */
type Plane = [number, number, number];

const SHAPES: Record<string, "gabled" | "hipped" | "pyramidal" | "skillion"> = {
  gabled: "gabled",
  gambrel: "gabled",
  saltbox: "gabled",
  round: "gabled",
  hipped: "hipped",
  mansard: "hipped",
  quadrupleSaltbox: "hipped",
  quadruple_saltbox: "hipped",
  pyramidal: "pyramidal",
  dome: "pyramidal",
  onion: "pyramidal",
  skillion: "skillion",
};

/** Whether a roof of this type is built (flat ones aren't). */
export const pitched = (shape: string) => shape in SHAPES;

const cross = (o: P, a: P, b: P) => (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);

/** The convex hull, anticlockwise (Andrew's monotone chain). */
function hull(points: P[]): P[] {
  const s = [...points].sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const half = (list: P[]) => {
    const out: P[] = [];
    for (const p of list) {
      while (out.length >= 2 && cross(out[out.length - 2], out[out.length - 1], p) <= 0) out.pop();
      out.push(p);
    }
    out.pop();
    return out;
  };
  return [...half(s), ...half(s.reverse())];
}

/** The rectangle of least area round the points: its middle, its axes (unit) and half-sizes along each. */
export function boundingRectangle(points: P[]): { centre: P; axes: [P, P]; half: [number, number] } {
  const h = hull(points);
  let best = { area: Infinity, centre: [0, 0] as P, axes: [[1, 0], [0, 1]] as [P, P], half: [0, 0] as [number, number] };
  for (let i = 0; i < h.length; i++) {
    const [a, b] = [h[i], h[(i + 1) % h.length]];
    const len = Math.hypot(b[0] - a[0], b[1] - a[1]);
    if (len < 1e-9) continue;
    const u: P = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
    const v: P = [-u[1], u[0]];
    let [u0, u1, v0, v1] = [Infinity, -Infinity, Infinity, -Infinity];
    for (const p of h) {
      const [pu, pv] = [p[0] * u[0] + p[1] * u[1], p[0] * v[0] + p[1] * v[1]];
      [u0, u1, v0, v1] = [Math.min(u0, pu), Math.max(u1, pu), Math.min(v0, pv), Math.max(v1, pv)];
    }
    const area = (u1 - u0) * (v1 - v0);
    if (area < best.area) {
      const [cu, cv] = [(u0 + u1) / 2, (v0 + v1) / 2];
      best = { area, centre: [u[0] * cu + v[0] * cv, u[1] * cu + v[1] * cv], axes: [u, v], half: [(u1 - u0) / 2, (v1 - v0) / 2] };
    }
  }
  return best;
}

/** The planes a roof is the lowest of, in the ground's own x and y. */
function planesOf(input: RoofInput, ring: P[]): Plane[] {
  const shape = SHAPES[input.shape];
  const r = boundingRectangle(ring);
  // the ridge along the longer side, unless mapped across it
  let [along, across] = r.axes;
  let [a, b] = r.half;
  if (a < b !== (input.orientation === "across")) {
    [along, across] = [across, along];
    [a, b] = [b, a];
  }
  if (b < 0.05) return [];
  const [cx, cy] = r.centre;
  /** `k + ku·u + kv·v`, with u and v measured along and across from the middle, in x and y. */
  const plane = (k: number, ku: number, kv: number): Plane => {
    const [gx, gy] = [ku * along[0] + kv * across[0], ku * along[1] + kv * across[1]];
    return [k - gx * cx - gy * cy, gx, gy];
  };
  switch (shape) {
    case "gabled":
      return [plane(1, 0, -1 / b), plane(1, 0, 1 / b)];
    case "hipped":
      // the ends as steep as the sides
      return [plane(1, 0, -1 / b), plane(1, 0, 1 / b), plane(a / b, -1 / b, 0), plane(a / b, 1 / b, 0)];
    case "pyramidal":
      return [plane(1, 0, -1 / b), plane(1, 0, 1 / b), plane(1, -1 / a, 0), plane(1, 1 / a, 0)];
    case "skillion": {
      // down the way the roof faces (else across), from the top at the far edge
      const deg = input.direction;
      const down: P = deg === undefined || !Number.isFinite(deg) ? across : [Math.sin((deg * Math.PI) / 180), Math.cos((deg * Math.PI) / 180)];
      let [lo, hi] = [Infinity, -Infinity];
      for (const p of ring) {
        const d = p[0] * down[0] + p[1] * down[1];
        [lo, hi] = [Math.min(lo, d), Math.max(hi, d)];
      }
      if (hi - lo < 0.05) return [];
      return [[hi / (hi - lo), -down[0] / (hi - lo), -down[1] / (hi - lo)]];
    }
    default:
      return [];
  }
}

const at = (p: Plane, q: P) => p[0] + p[1] * q[0] + p[2] * q[1];

/** The ring clipped to where `f` is at most 0 (Sutherland–Hodgman: one convex constraint at a time). */
function clip(ring: P[], f: (q: P) => number): P[] {
  const out: P[] = [];
  for (let i = 0; i < ring.length; i++) {
    const [p, q] = [ring[i], ring[(i + 1) % ring.length]];
    const [fp, fq] = [f(p), f(q)];
    if (fp <= 0) out.push(p);
    if (fp <= 0 !== fq <= 0) {
      const t = fp / (fp - fq);
      out.push([p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]);
    }
  }
  return out;
}

const open = (r: P[]) => (r.length > 1 && r[0][0] === r[r.length - 1][0] && r[0][1] === r[r.length - 1][1] ? r.slice(0, -1) : r);
const areaOf = (r: P[]) => r.reduce((a, p, i) => a + cross([0, 0], p, r[(i + 1) % r.length]), 0) / 2;

/** A roof's triangles; none when it has no shape to build (flat, too small, a ring that isn't one). */
export function roofParts(input: RoofInput): RoofParts {
  const out: RoofParts = { positions: [], normals: [], uvs: [], walls: [] };
  let ring = open(input.ring);
  if (ring.length < 3 || input.height <= 0.05 || !pitched(input.shape)) return out;
  // the outside anticlockwise and the courtyards clockwise, so a wall's outside is to the right of its way along
  const area = areaOf(ring);
  if (Math.abs(area) < 1) return out;
  if (area < 0) ring = [...ring].reverse();
  const holes = (input.holes ?? []).map(open).filter((h) => h.length >= 3).map((h) => (areaOf(h) > 0 ? [...h].reverse() : h));
  const planes = planesOf(input, ring);
  if (!planes.length) return out;
  const { base, height } = input;
  const lowest = (q: P) => Math.max(0, Math.min(...planes.map((p) => at(p, q))));
  const vertex = (x: number, y: number, z: number, n: number[], uv: P, wall: boolean) => {
    out.positions.push(x, y, z);
    out.normals.push(...n);
    out.uvs.push(...uv);
    out.walls.push(wall);
  };

  // the roof's faces: each plane where it is the lowest
  planes.forEach((plane, i) => {
    const lowestHere = (r: P[]) => {
      for (let j = 0; j < planes.length && r.length >= 3; j++) {
        if (j !== i) r = clip(r, (q) => at(plane, q) - at(planes[j], q));
      }
      return r;
    };
    const outer = lowestHere(ring);
    if (outer.length < 3) return;
    const inner = holes.map(lowestHere).filter((h) => h.length >= 3);
    const part = [...outer, ...inner.flat()];
    const holeStarts: number[] = [];
    let start = outer.length;
    for (const h of inner) {
      holeStarts.push(start);
      start += h.length;
    }
    // its normal; across it level, and up it, for its texture (in metres along its face)
    const [gx, gy] = [plane[1] * height, plane[2] * height];
    const n = [-gx, -gy, 1];
    const len = Math.hypot(n[0], n[1], n[2]);
    const normal = n.map((c) => c / len);
    const level: P = Math.hypot(gx, gy) > 1e-9 ? [-normal[1] / Math.hypot(normal[0], normal[1]), normal[0] / Math.hypot(normal[0], normal[1])] : [1, 0];
    // up the face: normal × level
    const up = [-normal[2] * level[1], normal[2] * level[0], normal[0] * level[1] - normal[1] * level[0]];
    const triangles = earcut(part.flat(), holeStarts.length ? holeStarts : undefined, 2);
    for (const k of triangles) {
      const q = part[k];
      const z = base + height * Math.max(0, at(plane, q));
      const uv: P = [q[0] * level[0] + q[1] * level[1], q[0] * up[0] + q[1] * up[1] + (z - base) * up[2]];
      vertex(q[0], q[1], z, normal, uv, false);
    }
  });

  // the walls up under it: along each side of the footprint, the roof's edge over the eaves (where it is one of the planes' creases, a corner)
  for (const r of [ring, ...holes]) for (let i = 0; i < r.length; i++) {
    const [p, q] = [r[i], r[(i + 1) % r.length]];
    const [dx, dy] = [q[0] - p[0], q[1] - p[1]];
    const len = Math.hypot(dx, dy);
    if (len < 1e-6) continue;
    const ts = [0, 1];
    for (let a = 0; a < planes.length; a++) {
      for (let b = a + 1; b < planes.length; b++) {
        const [fa, fb] = [at(planes[a], p) - at(planes[b], p), at(planes[a], q) - at(planes[b], q)];
        if (fa * fb < 0) ts.push(fa / (fa - fb));
      }
    }
    ts.sort((x, y) => x - y);
    const normal = [dy / len, -dx / len, 0];
    const point = (t: number): P => [p[0] + dx * t, p[1] + dy * t];
    for (let k = 1; k < ts.length; k++) {
      const [a, b] = [point(ts[k - 1]), point(ts[k])];
      const [ha, hb] = [height * lowest(a), height * lowest(b)];
      if (ha < 0.05 && hb < 0.05) continue;
      const [ua, ub] = [ts[k - 1] * len, ts[k] * len];
      // (in metres along the wall and up it)
      vertex(a[0], a[1], base, normal, [ua, 0], true);
      vertex(b[0], b[1], base, normal, [ub, 0], true);
      vertex(b[0], b[1], base + hb, normal, [ub, hb], true);
      vertex(a[0], a[1], base, normal, [ua, 0], true);
      vertex(b[0], b[1], base + hb, normal, [ub, hb], true);
      vertex(a[0], a[1], base + ha, normal, [ua, ha], true);
    }
  }
  return out;
}
