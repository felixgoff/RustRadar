// Low-poly aircraft meshes, built procedurally so there are no model files to
// ship. Units are metres; the nose points along +y, +x is the right wing, +z
// is up, and the lowest point of each model sits at z ≈ 0.
import { Geometry } from "@luma.gl/engine";
import { PARTS, type Part } from "./liveries";

type V3 = [number, number, number];

/** A model as one mesh (for flat, single-colour drawing) and per livery part. */
export interface ModelMeshes {
  combined: Geometry;
  parts: [Part, Geometry][];
}

const sub = (a: V3, b: V3): V3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const cross = (a: V3, b: V3): V3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const dot = (a: V3, b: V3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];

class MeshBuilder {
  /** The livery part new triangles belong to. */
  part: Part = "top";
  private buckets = new Map<Part, { positions: number[]; normals: number[] }>();

  /** Adds a flat-shaded triangle, wound so its normal faces away from `inside`. */
  tri(a: V3, b: V3, c: V3, inside: V3) {
    let n = cross(sub(b, a), sub(c, a));
    const len = Math.hypot(...n);
    if (len < 1e-9) return;
    n = [n[0] / len, n[1] / len, n[2] / len];
    const centroid: V3 = [(a[0] + b[0] + c[0]) / 3, (a[1] + b[1] + c[1]) / 3, (a[2] + b[2] + c[2]) / 3];
    if (dot(n, sub(centroid, inside)) < 0) {
      [b, c] = [c, b];
      n = [-n[0], -n[1], -n[2]];
    }
    let bucket = this.buckets.get(this.part);
    if (!bucket) this.buckets.set(this.part, (bucket = { positions: [], normals: [] }));
    for (const v of [a, b, c]) {
      bucket.positions.push(...v);
      bucket.normals.push(...n);
    }
  }

  quad(a: V3, b: V3, c: V3, d: V3, inside: V3) {
    this.tri(a, b, c, inside);
    this.tri(a, c, d, inside);
  }

  /**
   * A body of elliptical cross-sections lofted along y. With `belly`, the
   * underside goes to the belly part, for two-tone fuselages.
   */
  loft(sections: { y: number; w: number; h: number; z: number; x?: number }[], sides: number, belly = false) {
    const part = this.part;
    const rings = sections.map((s) =>
      Array.from({ length: sides }, (_, i): V3 => {
        const t = (i / sides) * Math.PI * 2;
        return [(s.x ?? 0) + s.w * Math.cos(t), s.y, s.z + s.h * Math.sin(t)];
      }),
    );
    for (let k = 0; k + 1 < rings.length; k++) {
      const [s0, s1] = [sections[k], sections[k + 1]];
      const inside: V3 = [((s0.x ?? 0) + (s1.x ?? 0)) / 2, (s0.y + s1.y) / 2, (s0.z + s1.z) / 2];
      for (let i = 0; i < sides; i++) {
        const j = (i + 1) % sides;
        const middle = ((i + 0.5) / sides) * Math.PI * 2;
        this.part = belly && Math.sin(middle) < -0.3 ? "belly" : part;
        this.quad(rings[k][i], rings[k][j], rings[k + 1][j], rings[k + 1][i], inside);
      }
    }
    this.part = part;
    // close blunt ends
    for (const [ring, s, dir] of [
      [rings[0], sections[0], -1],
      [rings[rings.length - 1], sections[sections.length - 1], 1],
    ] as const) {
      if (s.w < 1e-6) continue;
      const center: V3 = [s.x ?? 0, s.y, s.z];
      const inside: V3 = [s.x ?? 0, s.y - dir, s.z]; // a point inside the body
      for (let i = 0; i < sides; i++) this.tri(center, ring[i], ring[(i + 1) % sides], inside);
    }
  }

  /** A body of revolution around the vertical axis: `profile` is [radius, z] pairs. */
  lathe(profile: [number, number][], sides: number, at: V3 = [0, 0, 0]) {
    const rings = profile.map(([r, z]) =>
      Array.from({ length: sides }, (_, i): V3 => {
        const t = (i / sides) * Math.PI * 2;
        return [at[0] + r * Math.cos(t), at[1] + r * Math.sin(t), at[2] + z];
      }),
    );
    for (let k = 0; k + 1 < rings.length; k++) {
      const inside: V3 = [at[0], at[1], at[2] + (profile[k][1] + profile[k + 1][1]) / 2];
      for (let i = 0; i < sides; i++) {
        const j = (i + 1) % sides;
        this.quad(rings[k][i], rings[k][j], rings[k + 1][j], rings[k + 1][i], inside);
      }
    }
  }

  /**
   * A flat convex plate: `outline` is drawn in a local (u, v) plane and
   * `place(u, v, side)` maps it to 3D, with side = ±1 for its two faces.
   */
  slab(outline: [number, number][], place: (u: number, v: number, side: number) => V3) {
    const top = outline.map(([u, v]) => place(u, v, 1));
    const bottom = outline.map(([u, v]) => place(u, v, -1));
    const all = [...top, ...bottom];
    const inside = all.reduce<V3>((m, p) => [m[0] + p[0] / all.length, m[1] + p[1] / all.length, m[2] + p[2] / all.length], [0, 0, 0]);
    for (let i = 1; i + 1 < outline.length; i++) {
      this.tri(top[0], top[i], top[i + 1], inside);
      this.tri(bottom[0], bottom[i], bottom[i + 1], inside);
    }
    for (let i = 0; i < outline.length; i++) {
      const j = (i + 1) % outline.length;
      this.quad(top[i], top[j], bottom[j], bottom[i], inside);
    }
  }

  /** An axis-aligned box from its minimum and maximum corners. */
  box([x0, y0, z0]: V3, [x1, y1, z1]: V3) {
    this.slab(
      [
        [x0, y0],
        [x1, y0],
        [x1, y1],
        [x0, y1],
      ],
      (u, v, side) => [u, v, side > 0 ? z1 : z0],
    );
  }

  /** The same plate on both sides of the centreline. */
  mirrored(outline: [number, number][], place: (u: number, v: number, side: number) => V3) {
    this.slab(outline, place);
    this.slab(outline, (u, v, side) => {
      const [x, y, z] = place(u, v, side);
      return [-x, y, z];
    });
  }

  build(): ModelMeshes {
    const geometry = (positions: number[], normals: number[]) =>
      new Geometry({
        topology: "triangle-list",
        attributes: {
          positions: { size: 3, value: new Float32Array(positions) },
          normals: { size: 3, value: new Float32Array(normals) },
        },
      });
    const present = PARTS.filter((p) => this.buckets.has(p));
    return {
      combined: geometry(
        present.flatMap((p) => this.buckets.get(p)!.positions),
        present.flatMap((p) => this.buckets.get(p)!.normals),
      ),
      parts: present.map((p) => [p, geometry(this.buckets.get(p)!.positions, this.buckets.get(p)!.normals)]),
    };
  }
}

/** A swept-wing jet airliner with two or four engines under the wings. */
function airliner({ length: L, radius: R, span, engines }: { length: number; radius: number; span: number; engines: 2 | 4 }) {
  const m = new MeshBuilder();
  const cz = R + 0.6;
  m.loft(
    [
      { y: 0.5 * L, w: 0, h: 0, z: cz - 0.15 * R },
      { y: 0.475 * L, w: 0.45 * R, h: 0.42 * R, z: cz - 0.1 * R },
      { y: 0.44 * L, w: 0.82 * R, h: 0.8 * R, z: cz - 0.03 * R },
      { y: 0.4 * L, w: R, h: R, z: cz },
      { y: -0.29 * L, w: R, h: R, z: cz },
      { y: -0.41 * L, w: 0.62 * R, h: 0.68 * R, z: cz + 0.25 * R },
      { y: -0.49 * L, w: 0.16 * R, h: 0.2 * R, z: cz + 0.6 * R },
      { y: -0.5 * L, w: 0, h: 0, z: cz + 0.65 * R },
    ],
    12,
    true,
  );

  // wings: swept back about 27 degrees, tapering to a third of the root chord
  m.part = "wings";
  const wingZ = cz - 0.55 * R;
  const half = span / 2;
  const rootX = 0.9 * R;
  const rootLE = 0.09 * L;
  const rootTE = -0.1 * L;
  const sweep = 0.5;
  const leAt = (x: number) => rootLE - sweep * (x - rootX);
  const tipLE = leAt(half);
  const tipTE = tipLE - 0.32 * (rootLE - rootTE);
  const wingT = 0.12 * R;
  m.mirrored(
    [
      [rootX, rootLE],
      [half, tipLE],
      [half, tipTE],
      [rootX, rootTE],
    ],
    (u, v, side) => [u, v, wingZ + (side * wingT) / 2],
  );

  // engines hang slightly ahead of and below the wing
  m.part = "engines";
  const re = 0.48 * R;
  const engineZ = Math.max(re + 0.15, wingZ - 0.7 * re);
  const stations = engines === 2 ? [0.34] : [0.3, 0.56];
  for (const f of stations) {
    for (const sideX of [-1, 1]) {
      const x = sideX * (rootX + f * (half - rootX));
      const front = leAt(Math.abs(x)) + 1.3 * re;
      m.loft(
        [
          { x, y: front, w: 0.9 * re, h: 0.9 * re, z: engineZ },
          { x, y: front - 0.4 * re, w: re, h: re, z: engineZ },
          { x, y: front - 2.6 * re, w: 0.85 * re, h: 0.85 * re, z: engineZ },
          { x, y: front - 3.4 * re, w: 0.45 * re, h: 0.45 * re, z: engineZ },
        ],
        10,
      );
    }
  }

  // tailplane and fin
  m.part = "wings";
  const stabZ = cz + 0.35 * R;
  const stabT = 0.1 * R;
  m.mirrored(
    [
      [0.5 * R, -0.36 * L],
      [0.17 * L, -0.455 * L],
      [0.17 * L, -0.495 * L],
      [0.5 * R, -0.46 * L],
    ],
    (u, v, side) => [u, v, stabZ + (side * stabT) / 2],
  );
  const finBase = cz + 0.75 * R;
  const finTop = finBase + 0.19 * L;
  m.part = "tail";
  m.slab(
    [
      [-0.34 * L, finBase],
      [-0.45 * L, finTop],
      [-0.5 * L, finTop],
      [-0.495 * L, finBase],
    ],
    (u, v, side) => [(side * 0.13 * R) / 2, u, v],
  );
  return m.build();
}

/** A straight-winged propeller aircraft: high wing, single nose prop or twin nacelles. */
function propPlane({ length: L, span, twin, prop = true }: { length: number; span: number; twin: boolean; prop?: boolean }) {
  const m = new MeshBuilder();
  const R = 0.075 * L;
  const cz = 0.16 * L;
  m.loft(
    [
      { y: 0.5 * L, w: 0.25 * R, h: 0.25 * R, z: cz - 0.1 * R },
      { y: 0.46 * L, w: 0.8 * R, h: 0.85 * R, z: cz - 0.05 * R },
      { y: 0.3 * L, w: R, h: 1.2 * R, z: cz + 0.1 * R },
      { y: 0.02 * L, w: R, h: 1.2 * R, z: cz + 0.15 * R },
      { y: -0.25 * L, w: 0.55 * R, h: 0.65 * R, z: cz + 0.35 * R },
      { y: -0.5 * L, w: 0.12 * R, h: 0.18 * R, z: cz + 0.6 * R },
    ],
    10,
    true,
  );
  m.part = "wings";
  const wingZ = cz + 1.35 * R;
  const chord = 0.17 * L;
  const wingT = 0.25 * R;
  m.mirrored(
    [
      [0.5 * R, 0.2 * L],
      [span / 2, 0.19 * L],
      [span / 2, 0.19 * L - 0.75 * chord],
      [0.5 * R, 0.2 * L - chord],
    ],
    (u, v, side) => [u, v, wingZ + (side * wingT) / 2],
  );
  const propDisc = (x: number, y: number, z: number, r: number) =>
    m.slab(
      [
        [-r, -0.06 * r],
        [r, -0.06 * r],
        [r, 0.06 * r],
        [-r, 0.06 * r],
      ],
      (u, v, side) => [x + u, y + side * 0.03, z + v],
    );
  m.part = "engines";
  if (twin) {
    for (const sx of [-1, 1]) {
      const x = sx * 0.3 * (span / 2);
      const nz = wingZ - 0.6 * R;
      m.loft(
        [
          { x, y: 0.36 * L, w: 0.3 * R, h: 0.3 * R, z: nz },
          { x, y: 0.32 * L, w: 0.6 * R, h: 0.7 * R, z: nz },
          { x, y: 0.05 * L, w: 0.55 * R, h: 0.6 * R, z: nz },
          { x, y: -0.05 * L, w: 0.2 * R, h: 0.2 * R, z: nz },
        ],
        8,
      );
      if (prop) propDisc(x, 0.365 * L, nz, 0.17 * span / 2);
    }
  } else if (prop) {
    propDisc(0, 0.505 * L, cz - 0.1 * R, 0.11 * L);
  }
  m.part = "wings";
  m.mirrored(
    [
      [0.3 * R, -0.33 * L],
      [0.2 * L, -0.42 * L],
      [0.2 * L, -0.49 * L],
      [0.3 * R, -0.49 * L],
    ],
    (u, v, side) => [u, v, cz + 0.55 * R + side * 0.06 * R],
  );
  m.part = "tail";
  m.slab(
    [
      [-0.27 * L, cz + 0.6 * R],
      [-0.42 * L, cz + 0.6 * R + 0.17 * L],
      [-0.5 * L, cz + 0.6 * R + 0.17 * L],
      [-0.5 * L, cz + 0.6 * R],
    ],
    (u, v, side) => [side * 0.08 * R, u, v],
  );
  return m.build();
}

function helicopter() {
  const m = new MeshBuilder();
  m.loft(
    [
      { y: 2.6, w: 0.25, h: 0.3, z: 1.3 },
      { y: 2.1, w: 0.8, h: 0.9, z: 1.4 },
      { y: 0.5, w: 1.0, h: 1.1, z: 1.5 },
      { y: -1.0, w: 0.8, h: 0.95, z: 1.6 },
      { y: -1.8, w: 0.3, h: 0.35, z: 1.9 },
    ],
    10,
    true,
  );
  m.loft(
    [
      { y: -1.6, w: 0.22, h: 0.22, z: 2.0 },
      { y: -6.0, w: 0.12, h: 0.12, z: 2.2 },
      { y: -6.3, w: 0, h: 0, z: 2.2 },
    ],
    6,
  );
  m.part = "tail";
  m.slab(
    [
      [-5.6, 2.1],
      [-6.2, 3.3],
      [-6.6, 3.3],
      [-6.4, 2.1],
    ],
    (u, v, side) => [side * 0.05, u, v],
  );
  m.part = "engines";
  m.box([-0.12, -0.12, 2.5], [0.12, 0.12, 2.95]);
  const blade = (angle: number) => {
    const [c, s] = [Math.cos(angle), Math.sin(angle)];
    m.slab(
      [
        [-5.5, -0.16],
        [5.5, -0.16],
        [5.5, 0.16],
        [-5.5, 0.16],
      ],
      (u, v, side) => [u * c - v * s, u * s + v * c, 2.97 + side * 0.03],
    );
  };
  blade(Math.PI / 4);
  blade(-Math.PI / 4);
  for (const sx of [-1, 1]) m.box([sx * 0.9 - 0.06, -1.5, 0], [sx * 0.9 + 0.06, 1.8, 0.12]);
  return m.build();
}

function balloon() {
  const m = new MeshBuilder();
  m.part = "tail";
  m.lathe(
    [
      [0.6, 3.2],
      [3.2, 6.5],
      [5.6, 10],
      [6.6, 13.5],
      [6.2, 16.5],
      [4.4, 19],
      [0.01, 20.2],
    ],
    14,
  );
  m.part = "engines";
  m.box([-0.7, -0.7, 0], [0.7, 0.7, 1.1]);
  return m.build();
}

function groundVehicle() {
  const m = new MeshBuilder();
  m.box([-1.2, -3, 0.3], [1.2, 3, 1.6]);
  m.box([-1.1, 1.2, 1.6], [1.1, 2.8, 2.4]);
  return m.build();
}

export type ModelKind =
  | "narrow"
  | "wide2"
  | "wide4"
  | "turboprop"
  | "twin"
  | "light"
  | "glider"
  | "heli"
  | "balloon"
  | "ground";

let cache: Record<ModelKind, ModelMeshes> | null = null;

/** Built lazily: creating geometry needs no GPU, but there's no reason to do it at import. */
export function models(): Record<ModelKind, ModelMeshes> {
  cache ??= {
    narrow: airliner({ length: 38, radius: 2, span: 35, engines: 2 }),
    wide2: airliner({ length: 64, radius: 3.1, span: 61, engines: 2 }),
    wide4: airliner({ length: 70, radius: 3.3, span: 64, engines: 4 }),
    turboprop: propPlane({ length: 25, span: 27, twin: true }),
    twin: propPlane({ length: 9.5, span: 12, twin: true }),
    light: propPlane({ length: 8.6, span: 11, twin: false }),
    glider: propPlane({ length: 6.8, span: 15, twin: false, prop: false }),
    heli: helicopter(),
    balloon: balloon(),
    ground: groundVehicle(),
  };
  return cache;
}

export const MODEL_KINDS: ModelKind[] = [
  "narrow",
  "wide2",
  "wide4",
  "turboprop",
  "twin",
  "light",
  "glider",
  "heli",
  "balloon",
  "ground",
];

/** Flightradar24's icon class → model and a scale factor within that model. */
const ICON_MODELS: Record<string, [ModelKind, number]> = {
  B738: ["narrow", 1],
  A320: ["narrow", 1],
  B736: ["narrow", 0.85],
  B757: ["narrow", 1.25],
  RJ85: ["narrow", 0.75],
  FOKKER100: ["narrow", 0.8],
  LJ60: ["narrow", 0.45],
  FGTR: ["narrow", 0.45],
  CONC: ["narrow", 1.5],
  B767: ["wide2", 0.85],
  A330: ["wide2", 0.95],
  B777: ["wide2", 1.05],
  MD11: ["wide2", 0.9],
  A3ST: ["wide2", 0.85],
  A343: ["wide4", 0.95],
  A346: ["wide4", 1.05],
  B747: ["wide4", 1],
  A380: ["wide4", 1.1],
  A225: ["wide4", 1.2],
  SAT: ["wide4", 0.5],
  ISS: ["wide4", 1.4],
  Q300: ["turboprop", 1],
  C303: ["twin", 1],
  C206: ["light", 1],
  SLEI: ["light", 1],
  ASW20: ["glider", 1],
  SI2: ["glider", 1.6],
  EC: ["heli", 1],
  DRON: ["heli", 0.25],
  BALL: ["balloon", 1],
  GRND: ["ground", 1],
};

export function modelFor(icon: string): [ModelKind, number] {
  return ICON_MODELS[icon] ?? ["narrow", 1];
}

/** Largest dimension of each model (length, wingspan or rotor), in metres. */
export const MODEL_SIZE: Record<ModelKind, number> = {
  narrow: 38,
  wide2: 64,
  wide4: 70,
  turboprop: 27,
  twin: 12,
  light: 11,
  glider: 15,
  heli: 11,
  balloon: 13.2,
  ground: 6,
};

/** Real-world size (largest dimension, metres) of an aircraft of this icon class. */
export function sizeOf(icon: string): number {
  const [kind, factor] = modelFor(icon);
  return MODEL_SIZE[kind] * factor;
}
