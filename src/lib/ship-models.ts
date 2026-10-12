// Low-poly ship meshes, built procedurally like the aircraft (models.ts), and
// how each ship is sized and placed from its AIS report.
//
// Every model is one unit long: the bow points along +y and y runs from -0.5
// (stern) to 0.5 (bow), +x is starboard, +z is up and z = 0 is the waterline.
// The beam is the class's typical share of the length (`ShipModel.beam`) and
// is stretched per ship to its real beam; heights are in the same units.
import { Geometry } from "@luma.gl/engine";
import type { Ship } from "./api";
import { shipColor, type RGB } from "./ships";

type V3 = [number, number, number];

export type ShipClass =
  | "container"
  | "tanker"
  | "bulk"
  | "cargo"
  | "passenger"
  | "tug"
  | "fishing"
  | "pleasure"
  | "sar"
  | "generic";

export const SHIP_CLASSES: ShipClass[] = [
  "container",
  "tanker",
  "bulk",
  "cargo",
  "passenger",
  "tug",
  "fishing",
  "pleasure",
  "sar",
  "generic",
];

/** One model's triangles, flat shaded: three vertices per triangle. */
export interface ShipModelArrays {
  positions: Float32Array;
  normals: Float32Array;
  /** Per vertex, 0..1. */
  colors: Float32Array;
  /** Beam as a share of the length, the model's full width. */
  beam: number;
  /** Height of the superstructure's top above the waterline (masts not counted). */
  top: number;
}

const sub = (a: V3, b: V3): V3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const cross = (a: V3, b: V3): V3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const dot = (a: V3, b: V3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
const shade = (c: RGB, k: number): RGB => [Math.min(255, c[0] * k), Math.min(255, c[1] * k), Math.min(255, c[2] * k)];

// shared paints
const WHITE: RGB = [236, 238, 240];
const OFF_WHITE: RGB = [214, 216, 212];
const WINDOW: RGB = [34, 44, 62];
const ANTIFOULING: RGB = [112, 34, 30];
const BOOT_TOP: RGB = [26, 26, 30];
const STEEL_DECK: RGB = [104, 108, 106];
const RUST_DECK: RGB = [128, 70, 56];
const TEAK: RGB = [176, 138, 96];
const FUNNEL: RGB = [44, 46, 52];
const CRANE: RGB = [218, 176, 60];
const MAST: RGB = [200, 202, 204];
const ORANGE: RGB = [236, 112, 36];
const CONTAINERS: RGB[] = [
  [178, 58, 48],
  [46, 92, 150],
  [64, 130, 84],
  [214, 140, 52],
  [150, 150, 146],
  [222, 222, 214],
  [92, 70, 120],
  [40, 120, 150],
  [190, 160, 60],
];

class ShipBuilder {
  private positions: number[] = [];
  private normals: number[] = [];
  private colors: number[] = [];

  /** A flat-shaded triangle, wound and lit so its normal faces away from `inside`. */
  tri(a: V3, b: V3, c: V3, inside: V3, color: RGB) {
    let n = cross(sub(b, a), sub(c, a));
    const len = Math.hypot(n[0], n[1], n[2]);
    if (len < 1e-12) return;
    n = [n[0] / len, n[1] / len, n[2] / len];
    const centroid: V3 = [(a[0] + b[0] + c[0]) / 3, (a[1] + b[1] + c[1]) / 3, (a[2] + b[2] + c[2]) / 3];
    if (dot(n, sub(centroid, inside)) < 0) {
      [b, c] = [c, b];
      n = [-n[0], -n[1], -n[2]];
    }
    for (const v of [a, b, c]) {
      this.positions.push(v[0], v[1], v[2]);
      this.normals.push(n[0], n[1], n[2]);
      this.colors.push(color[0] / 255, color[1] / 255, color[2] / 255);
    }
  }

  quad(a: V3, b: V3, c: V3, d: V3, inside: V3, color: RGB) {
    this.tri(a, b, c, inside, color);
    this.tri(a, c, d, inside, color);
  }

  /**
   * A convex six-sided solid from its eight corners: the bottom four, then
   * the top four above them, each going round the same way.
   */
  solid(corners: V3[], color: RGB, top: RGB = color) {
    const inside = corners.reduce<V3>((m, p) => [m[0] + p[0] / 8, m[1] + p[1] / 8, m[2] + p[2] / 8], [0, 0, 0]);
    const [b0, b1, b2, b3, t0, t1, t2, t3] = corners;
    this.quad(b0, b1, b2, b3, inside, color);
    this.quad(t0, t1, t2, t3, inside, top);
    this.quad(b0, b1, t1, t0, inside, color);
    this.quad(b1, b2, t2, t1, inside, color);
    this.quad(b2, b3, t3, t2, inside, color);
    this.quad(b3, b0, t0, t3, inside, color);
  }

  /** An axis-aligned box from its minimum and maximum corners, optionally a different colour on top. */
  box([x0, y0, z0]: V3, [x1, y1, z1]: V3, color: RGB, top: RGB = color) {
    this.solid(
      [
        [x0, y0, z0],
        [x1, y0, z0],
        [x1, y1, z0],
        [x0, y1, z0],
        [x0, y0, z1],
        [x1, y0, z1],
        [x1, y1, z1],
        [x0, y1, z1],
      ],
      color,
      top,
    );
  }

  /** A box centred on the centreline. */
  block(halfWidth: number, y0: number, y1: number, z0: number, z1: number, color: RGB, top: RGB = color) {
    this.box([-halfWidth, y0, z0], [halfWidth, y1, z1], color, top);
  }

  /** A thin square rod from `a` to `b`, `r` its half-thickness. */
  rod(a: V3, b: V3, r: number, color: RGB) {
    const d = sub(b, a);
    const len = Math.hypot(d[0], d[1], d[2]);
    if (len < 1e-9) return;
    const axis: V3 = [d[0] / len, d[1] / len, d[2] / len];
    // any direction across the rod, then a second one across both
    const helper: V3 = Math.abs(axis[2]) < 0.9 ? [0, 0, 1] : [1, 0, 0];
    let u = cross(axis, helper);
    const ul = Math.hypot(u[0], u[1], u[2]);
    u = [(u[0] / ul) * r, (u[1] / ul) * r, (u[2] / ul) * r];
    const v = cross(axis, u);
    const ring = (p: V3): V3[] => [
      [p[0] - u[0] - v[0], p[1] - u[1] - v[1], p[2] - u[2] - v[2]],
      [p[0] + u[0] - v[0], p[1] + u[1] - v[1], p[2] + u[2] - v[2]],
      [p[0] + u[0] + v[0], p[1] + u[1] + v[1], p[2] + u[2] + v[2]],
      [p[0] - u[0] + v[0], p[1] - u[1] + v[1], p[2] - u[2] + v[2]],
    ];
    this.solid([...ring(a), ...ring(b)], color);
  }

  /**
   * The hull: stations lofted from the stern (y = -0.5) to a pointed bow
   * (y = 0.5), with sheer, flare above the waterline, a black boot-top at
   * it and red antifouling below, and a flat deck on top.
   */
  hull(h: HullSpec): (y: number) => number {
    const {
      beam,
      freeboard,
      draft,
      sheerBow = freeboard * 0.5,
      sheerStern = freeboard * 0.15,
      bowLength = 0.22,
      transom = 0.85,
      rake = 0.03,
      overhang = 0.02,
      topsides,
      deck,
      bottom = 0.75,
      rail: railColor = topsides,
      railScale = 1,
    } = h;
    const half = beam / 2;
    const boot = Math.min(freeboard * 0.25, 0.006);
    const railHeight = Math.min(freeboard * 0.18, 0.006) * railScale;
    const railWidth = beam * 0.04 * railScale;
    const steps = 16;
    const stations: number[] = [];
    for (let i = 0; i <= steps; i++) stations.push(i / steps);
    // the bow's taper gets extra stations so its curve stays smooth
    for (let i = 1; i < 6; i++) stations.push(1 - bowLength + (bowLength * i) / 6);
    stations.sort((a, b) => a - b);
    const width = (s: number) => {
      const fromBow = (s - (1 - bowLength)) / bowLength;
      if (fromBow > 0) return Math.max(0, 1 - fromBow ** 1.7) ** 0.75;
      if (s < 0.14) return lerp(transom, 1, Math.sqrt(s / 0.14));
      return 1;
    };
    const deckZ = (s: number) =>
      freeboard + sheerBow * Math.max(0, (s - 0.55) / 0.45) ** 2 + sheerStern * Math.max(0, (0.15 - s) / 0.15) ** 2;
    type Station = { deck: V3; boot: V3; water: V3; keel: V3 };
    const at = (s: number): Station => {
      const w = width(s) * half;
      const dz = deckZ(s);
      // the waterline and keel are shorter than the deck: a raked stem and a stern overhang
      const yWater = lerp(-0.5 + overhang, 0.5 - rake, s);
      const yKeel = lerp(-0.5 + overhang * 2, 0.5 - rake * 1.6, s);
      const yDeck = -0.5 + s;
      const yBoot = lerp(yWater, yDeck, boot / dz);
      return {
        deck: [w, yDeck, dz],
        boot: [w * 0.97, yBoot, boot],
        water: [w * 0.94, yWater, 0],
        keel: [w * bottom, yKeel, -draft],
      };
    };
    const mirror = (p: V3): V3 => [-p[0], p[1], p[2]];
    const ring = stations.map(at);
    for (let k = 0; k + 1 < ring.length; k++) {
      const [a, b] = [ring[k], ring[k + 1]];
      const inside: V3 = [0, (a.deck[1] + b.deck[1]) / 2, (freeboard - draft) / 2];
      for (const side of [1, -1]) {
        const p = side > 0 ? (v: V3) => v : mirror;
        const out: V3 = [inside[0] - side, inside[1], inside[2]];
        this.quad(p(a.deck), p(b.deck), p(b.boot), p(a.boot), out, topsides);
        this.quad(p(a.boot), p(b.boot), p(b.water), p(a.water), out, BOOT_TOP);
        this.quad(p(a.water), p(b.water), p(b.keel), p(a.keel), out, ANTIFOULING);
      }
      this.quad(a.deck, b.deck, mirror(b.deck), mirror(a.deck), [0, inside[1], -1], deck);
      // a bulwark round the deck edge in the hull's colour, so the type shows from above too
      const rail = (p: V3, dw: number, dz: number): V3 => [Math.max(0, p[0] - dw), p[1], p[2] + dz];
      for (const side of [1, -1]) {
        const p = side > 0 ? (v: V3) => v : mirror;
        const [ao, bo] = [rail(a.deck, 0, railHeight), rail(b.deck, 0, railHeight)];
        const [ai, bi] = [rail(a.deck, railWidth, railHeight), rail(b.deck, railWidth, railHeight)];
        const [af, bf] = [rail(a.deck, railWidth, 0), rail(b.deck, railWidth, 0)];
        const mid: V3 = [side * Math.max(0, a.deck[0] - railWidth / 2), inside[1], a.deck[2] + railHeight / 2];
        // outside, top and inside faces, each lit away from the rail's middle
        this.quad(p(a.deck), p(b.deck), p(bo), p(ao), [mid[0] - side, mid[1], mid[2]], railColor);
        this.quad(p(ao), p(bo), p(bi), p(ai), [mid[0], mid[1], mid[2] - 1], railColor);
        this.quad(p(ai), p(bi), p(bf), p(af), [mid[0] + side, mid[1], mid[2]], shade(railColor, 0.8));
      }
      this.quad(a.keel, b.keel, mirror(b.keel), mirror(a.keel), [0, inside[1], 1], ANTIFOULING);
    }
    // the transom
    const t = ring[0];
    const aft: V3 = [0, 1, 0];
    this.quad(t.deck, mirror(t.deck), mirror(t.boot), t.boot, aft, topsides);
    this.quad(t.boot, mirror(t.boot), mirror(t.water), t.water, aft, BOOT_TOP);
    this.quad(t.water, mirror(t.water), mirror(t.keel), t.keel, aft, ANTIFOULING);
    return (y: number) => deckZ(y + 0.5);
  }

  build(beam: number, top: number): ShipModelArrays {
    return {
      positions: new Float32Array(this.positions),
      normals: new Float32Array(this.normals),
      colors: new Float32Array(this.colors),
      beam,
      top,
    };
  }
}

interface HullSpec {
  beam: number;
  freeboard: number;
  draft: number;
  sheerBow?: number;
  sheerStern?: number;
  /** Share of the length over which the bow narrows to its point. */
  bowLength?: number;
  /** The transom's width as a share of the beam. */
  transom?: number;
  rake?: number;
  overhang?: number;
  topsides: RGB;
  deck: RGB;
  /** The keel's width as a share of the beam. */
  bottom?: number;
  /** The bulwark's colour (default the topsides') and how much heavier than usual it is. */
  rail?: RGB;
  railScale?: number;
}

/** A small deterministic sequence, so the container stacks look the same every time. */
function seeded(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 2 ** 32;
  };
}

/** The type colour, a touch darker, for the hull's topsides. */
const topsidesOf = (kind: number) => shade(shipColor(kind), 0.82);

/** An aft bridge: a deckhouse with a window band, bridge wings and a funnel behind it. */
function aftBridge(
  m: ShipBuilder,
  { beam, y0, y1, z0, height, funnel = true }: { beam: number; y0: number; y1: number; z0: number; height: number; funnel?: boolean },
) {
  const hw = beam * 0.4;
  const top = z0 + height;
  m.block(hw, y0, y1, z0, top - height * 0.12, WHITE);
  // the wheelhouse on top, a dark band of windows across its front, and wings to the ship's sides
  m.block(hw * 0.92, y0 + (y1 - y0) * 0.25, y1, top - height * 0.12, top, WHITE);
  m.box([-hw * 0.93, y1 - 0.002, top - height * 0.1], [hw * 0.93, y1 + 0.001, top - height * 0.03], WINDOW);
  m.box([-beam * 0.5, y1 - (y1 - y0) * 0.35, top - height * 0.13], [beam * 0.5, y1, top - height * 0.1], OFF_WHITE);
  if (funnel) {
    const fy = y0 - (y1 - y0) * 0.05;
    m.block(beam * 0.1, fy, fy + (y1 - y0) * 0.4, top - height * 0.3, top + height * 0.12, FUNNEL);
  }
}

function container(): ShipModelArrays {
  const m = new ShipBuilder();
  const beam = 0.14;
  const freeboard = 0.04;
  m.hull({ beam, freeboard, draft: 0.02, topsides: topsidesOf(4), deck: STEEL_DECK, bowLength: 0.25, sheerBow: 0.02 });
  // a forecastle and a breakwater ahead of the first bay
  m.block(beam * 0.32, 0.36, 0.43, freeboard, freeboard + 0.022, OFF_WHITE);
  const rand = seeded(7);
  const rows = 5;
  const bayLength = 0.036;
  const gap = 0.005;
  const stack = (y0: number, y1: number, narrow: number) => {
    for (let r = 0; r < rows; r++) {
      const x0 = -beam * 0.46 * narrow + ((beam * 0.92 * narrow) / rows) * r;
      const x1 = x0 + (beam * 0.92 * narrow) / rows - 0.0012;
      const tiers = 3 + Math.floor(rand() * 4);
      const color = CONTAINERS[Math.floor(rand() * CONTAINERS.length)];
      m.box([x0, y0, freeboard], [x1, y1, freeboard + tiers * 0.0085], color);
    }
  };
  // bays ahead of the bridge, then two behind it
  for (let y = 0.34; y - bayLength > -0.24; y -= bayLength + gap) {
    stack(y - bayLength, y, y > 0.22 ? 0.7 : 1);
  }
  stack(-0.42, -0.42 + bayLength, 0.85);
  aftBridge(m, { beam, y0: -0.36, y1: -0.28, z0: freeboard, height: 0.085 });
  return m.build(beam, freeboard + 0.085 * 1.12);
}

function tanker(): ShipModelArrays {
  const m = new ShipBuilder();
  const beam = 0.17;
  const freeboard = 0.032;
  m.hull({ beam, freeboard, draft: 0.022, topsides: topsidesOf(6), deck: RUST_DECK, bowLength: 0.2, transom: 0.9 });
  m.block(beam * 0.3, 0.38, 0.45, freeboard, freeboard + 0.018, OFF_WHITE);
  // the pipe run along the centreline, the manifold across midships, and a catwalk over them
  m.block(beam * 0.06, -0.3, 0.38, freeboard, freeboard + 0.006, OFF_WHITE);
  m.box([-beam * 0.42, -0.02, freeboard], [beam * 0.42, 0.02, freeboard + 0.008], OFF_WHITE);
  m.box([-beam * 0.1, 0.06, freeboard], [beam * 0.1, 0.075, freeboard + 0.01], CRANE);
  for (let y = -0.25; y < 0.36; y += 0.08) m.block(beam * 0.02, y, y + 0.008, freeboard, freeboard + 0.012, OFF_WHITE);
  aftBridge(m, { beam, y0: -0.42, y1: -0.33, z0: freeboard, height: 0.08 });
  return m.build(beam, freeboard + 0.08 * 1.12);
}

/** Bulk carriers (large hatch covers) and general cargo ships (smaller hatches, deck cranes). */
function bulk(cranes: boolean): ShipModelArrays {
  const m = new ShipBuilder();
  const beam = 0.16;
  const freeboard = cranes ? 0.055 : 0.045;
  m.hull({ beam, freeboard, draft: 0.022, topsides: topsidesOf(4), deck: cranes ? STEEL_DECK : RUST_DECK, bowLength: 0.2 });
  m.block(beam * 0.34, 0.38, 0.45, freeboard, freeboard + 0.02, OFF_WHITE);
  const holds = cranes ? 4 : 7;
  const span = 0.62;
  const start = -0.27;
  const each = span / holds;
  for (let i = 0; i < holds; i++) {
    const y0 = start + i * each + each * 0.08;
    const y1 = start + (i + 1) * each - each * 0.08;
    m.block(beam * 0.32, y0, y1, freeboard, freeboard + 0.012, cranes ? [64, 92, 120] : [150, 104, 70], cranes ? [84, 116, 148] : [170, 120, 80]);
    if (cranes && i < holds - 1) {
      // a crane post between holds, its jib reaching forward over the next one
      const y = y1 + each * 0.08;
      const x = (i % 2 ? -1 : 1) * beam * 0.18;
      m.rod([x, y, freeboard], [x, y, freeboard + 0.07], 0.005, CRANE);
      m.rod([x, y, freeboard + 0.055], [x * 0.3, y + each * 0.85, freeboard + 0.1], 0.0028, CRANE);
    }
  }
  aftBridge(m, { beam, y0: -0.42, y1: -0.33, z0: freeboard, height: 0.075 });
  return m.build(beam, freeboard + 0.075 * 1.12);
}

function passenger(): ShipModelArrays {
  const m = new ShipBuilder();
  const beam = 0.15;
  const freeboard = 0.045;
  m.hull({ beam, freeboard, draft: 0.02, topsides: topsidesOf(3), deck: TEAK, bowLength: 0.26, sheerBow: 0.015, transom: 0.9 });
  // tiers of decks, each stepping back at the front, with a band of windows along both sides
  const tiers = 6;
  const tier = 0.021;
  let z = freeboard;
  for (let i = 0; i < tiers; i++) {
    const front = 0.3 - i * 0.022;
    const back = -0.43 + i * 0.012;
    const hw = beam * (0.49 - i * 0.012);
    m.block(hw, back, front, z, z + tier, WHITE);
    m.box([-hw - 0.0012, back + 0.01, z + tier * 0.35], [hw + 0.0012, front - 0.008, z + tier * 0.7], WINDOW);
    m.box([-hw * 0.9, front - 0.002, z + tier * 0.35], [hw * 0.9, front + 0.0012, z + tier * 0.7], WINDOW);
    z += tier;
  }
  // the funnel, aft of midships
  m.block(beam * 0.14, -0.27, -0.17, z, z + 0.03, topsidesOf(3), FUNNEL);
  return m.build(beam, z + 0.03);
}

function tug(): ShipModelArrays {
  const m = new ShipBuilder();
  const beam = 0.34;
  const freeboard = 0.07;
  m.hull({ beam, freeboard, draft: 0.05, sheerBow: 0.07, sheerStern: 0.01, bowLength: 0.32, transom: 0.8, bottom: 0.7, topsides: topsidesOf(2), deck: STEEL_DECK, rake: 0.02, overhang: 0.03, rail: BOOT_TOP, railScale: 2.5 });
  // (the black bulwark is its fender belt)
  // the deckhouse, with the wheelhouse on top and windows all round
  m.block(beam * 0.32, -0.08, 0.2, freeboard, freeboard + 0.1, WHITE);
  m.block(beam * 0.27, -0.02, 0.18, freeboard + 0.1, freeboard + 0.19, WHITE, topsidesOf(2));
  m.box([-beam * 0.275, -0.025, freeboard + 0.13], [beam * 0.275, 0.185, freeboard + 0.17], WINDOW);
  // twin stacks behind it and the tow hook aft
  m.box([-beam * 0.12, -0.17, freeboard], [-beam * 0.02, -0.11, freeboard + 0.17], FUNNEL);
  m.box([beam * 0.02, -0.17, freeboard], [beam * 0.12, -0.11, freeboard + 0.17], FUNNEL);
  m.block(beam * 0.12, -0.36, -0.28, freeboard, freeboard + 0.04, CRANE);
  m.rod([0, 0.08, freeboard + 0.19], [0, 0.08, freeboard + 0.3], 0.006, MAST);
  return m.build(beam, freeboard + 0.19);
}

function fishing(): ShipModelArrays {
  const m = new ShipBuilder();
  const beam = 0.28;
  const freeboard = 0.065;
  m.hull({ beam, freeboard, draft: 0.045, sheerBow: 0.06, sheerStern: 0.01, bowLength: 0.3, transom: 0.75, bottom: 0.6, topsides: topsidesOf(5), deck: STEEL_DECK });
  // a wheelhouse forward
  m.block(beam * 0.34, 0.08, 0.3, freeboard, freeboard + 0.08, WHITE);
  m.block(beam * 0.3, 0.12, 0.28, freeboard + 0.08, freeboard + 0.15, WHITE);
  m.box([-beam * 0.305, 0.2, freeboard + 0.1], [beam * 0.305, 0.282, freeboard + 0.135], WINDOW);
  // a mast behind it, and the A-frame gantry over the stern
  m.rod([0, 0.06, freeboard], [0, 0.06, freeboard + 0.32], 0.006, MAST);
  const gz = freeboard + 0.22;
  for (const x of [-1, 1]) m.rod([x * beam * 0.4, -0.45, freeboard], [x * beam * 0.36, -0.4, gz], 0.008, ORANGE);
  m.rod([-beam * 0.37, -0.4, gz], [beam * 0.37, -0.4, gz], 0.008, ORANGE);
  // a winch drum
  m.block(beam * 0.22, -0.2, -0.12, freeboard, freeboard + 0.04, CRANE);
  return m.build(beam, freeboard + 0.15);
}

function pleasure(): ShipModelArrays {
  const m = new ShipBuilder();
  const beam = 0.3;
  const freeboard = 0.07;
  m.hull({ beam, freeboard, draft: 0.04, sheerBow: 0.03, bowLength: 0.48, transom: 0.8, bottom: 0.35, rake: 0.05, overhang: 0.04, topsides: topsidesOf(8), deck: TEAK });
  // a low coachroof with a dark window strip, the cockpit aft
  m.block(beam * 0.28, -0.12, 0.2, freeboard, freeboard + 0.04, WHITE);
  m.box([-beam * 0.285, -0.08, freeboard + 0.015], [beam * 0.285, 0.16, freeboard + 0.03], WINDOW);
  m.box([-beam * 0.3, -0.42, freeboard], [beam * 0.3, -0.2, freeboard + 0.005], topsidesOf(8));
  // the mast and boom
  m.rod([0, 0.12, freeboard + 0.04], [0, 0.12, 0.95], 0.007, MAST);
  m.rod([0, 0.12, freeboard + 0.1], [0, -0.3, freeboard + 0.1], 0.006, MAST);
  return m.build(beam, freeboard + 0.04);
}

function sar(): ShipModelArrays {
  const m = new ShipBuilder();
  const beam = 0.3;
  const freeboard = 0.06;
  m.hull({ beam, freeboard, draft: 0.035, sheerBow: 0.035, bowLength: 0.4, transom: 0.92, bottom: 0.3, rake: 0.04, topsides: [44, 62, 104], deck: STEEL_DECK });
  // a bright orange cabin with a wraparound screen, and a radar mast
  m.block(beam * 0.36, -0.18, 0.2, freeboard, freeboard + 0.1, ORANGE);
  m.box([-beam * 0.365, -0.05, freeboard + 0.055], [beam * 0.365, 0.205, freeboard + 0.085], WINDOW);
  m.block(beam * 0.28, -0.12, 0.08, freeboard + 0.1, freeboard + 0.14, ORANGE);
  m.rod([0, -0.05, freeboard + 0.14], [0, -0.05, freeboard + 0.25], 0.006, MAST);
  m.box([-beam * 0.2, -0.065, freeboard + 0.23], [beam * 0.2, -0.035, freeboard + 0.24], MAST);
  return m.build(beam, freeboard + 0.14);
}

function generic(): ShipModelArrays {
  const m = new ShipBuilder();
  const beam = 0.18;
  const freeboard = 0.06;
  m.hull({ beam, freeboard, draft: 0.025, bowLength: 0.25, topsides: topsidesOf(0), deck: STEEL_DECK });
  m.block(beam * 0.3, 0.36, 0.44, freeboard, freeboard + 0.025, OFF_WHITE);
  aftBridge(m, { beam, y0: -0.38, y1: -0.22, z0: freeboard, height: 0.09 });
  m.block(beam * 0.3, -0.1, 0.25, freeboard, freeboard + 0.015, [120, 124, 120]);
  return m.build(beam, freeboard + 0.09 * 1.12);
}

const BUILDERS: Record<ShipClass, () => ShipModelArrays> = {
  container,
  tanker,
  bulk: () => bulk(false),
  cargo: () => bulk(true),
  passenger,
  tug,
  fishing,
  pleasure,
  sar,
  generic,
};

/** One class's model as triangle arrays (for tests and the meshes). */
export const shipModelArrays = (cls: ShipClass) => BUILDERS[cls]();

export interface ShipMesh {
  geometry: Geometry;
  beam: number;
  top: number;
}

let meshes: Record<ShipClass, ShipMesh> | null = null;
/** Every class's mesh, built once on first use. */
export function shipMeshes(): Record<ShipClass, ShipMesh> {
  if (meshes) return meshes;
  const built = {} as Record<ShipClass, ShipMesh>;
  for (const cls of SHIP_CLASSES) {
    const a = shipModelArrays(cls);
    built[cls] = {
      geometry: new Geometry({
        topology: "triangle-list",
        attributes: {
          positions: { size: 3, value: a.positions },
          normals: { size: 3, value: a.normals },
          colors: { size: 3, value: a.colors },
        },
      }),
      beam: a.beam,
      top: a.top,
    };
  }
  return (meshes = built);
}

/** Typical lengths in metres by class and size bucket (small, medium, large), for ships without dimensions. */
export const TYPICAL_LENGTH: Record<ShipClass, [number, number, number]> = {
  container: [140, 220, 330],
  tanker: [90, 180, 270],
  bulk: [130, 190, 250],
  cargo: [60, 95, 130],
  passenger: [40, 110, 230],
  tug: [20, 28, 36],
  fishing: [14, 24, 45],
  pleasure: [8, 12, 18],
  sar: [10, 16, 24],
  generic: [25, 60, 120],
};

/** Usable AIS dimensions: a believable length and beam. */
export function usableDims(dims: Ship["dims"]): Ship["dims"] {
  if (!dims) return null;
  const [bow, stern, port, stbd] = dims;
  const length = bow + stern;
  const beam = port + stbd;
  if (!(length >= 3 && length <= 500 && beam >= 1 && beam <= length * 0.7)) return null;
  return dims;
}

/** The model a ship is drawn with, from its type colour and size. */
export function shipClassOf(ship: Pick<Ship, "kind" | "size" | "mmsi" | "dims">): ShipClass {
  const dims = usableDims(ship.dims);
  const length = dims ? dims[0] + dims[1] : null;
  const long = (metres: number) => (length ?? [0, 0, Infinity][ship.size] ?? 0) >= metres;
  switch (ship.kind) {
    case 2:
      return long(55) ? "generic" : "tug";
    case 3:
      return "passenger";
    case 4:
      // the feed doesn't say which cargo ship: big ones are container ships
      // or bulk carriers (fixed per ship), smaller ones general cargo
      if (!long(150)) return "cargo";
      return ship.mmsi % 3 === 0 ? "bulk" : "container";
    case 5:
      return long(70) ? "generic" : "fishing";
    case 6:
      return "tanker";
    case 7:
      return long(45) ? "generic" : "sar";
    case 8:
      return long(30) ? "passenger" : "pleasure";
    default:
      return "generic";
  }
}

/** The tallest a superstructure is drawn, metres; taller models are squashed to it. */
export const MAX_SUPERSTRUCTURE_M = 40;

export interface ShipPlacement {
  cls: ShipClass;
  length: number;
  beam: number;
  /** Mesh scale: metres per model unit across, along and up. */
  scale: [number, number, number];
  /** The hull's centre from the AIS reference point, metres to starboard and ahead. */
  right: number;
  ahead: number;
}

/**
 * How big a ship is drawn and where its hull sits around the reported
 * position: from its AIS dimensions (reference point to bow, stern, port and
 * starboard) or, without them, a typical size for its class, centred.
 */
export function shipPlacement(ship: Pick<Ship, "kind" | "size" | "mmsi" | "dims">, cls = shipClassOf(ship)): ShipPlacement {
  const model = shipModelShape(cls);
  const dims = usableDims(ship.dims);
  let length: number;
  let beam: number;
  let right = 0;
  let ahead = 0;
  if (dims) {
    const [bow, stern, port, stbd] = dims;
    length = bow + stern;
    beam = port + stbd;
    ahead = (bow - stern) / 2;
    right = (stbd - port) / 2;
  } else {
    length = TYPICAL_LENGTH[cls][ship.size] ?? TYPICAL_LENGTH[cls][1];
    beam = length * model.beam;
  }
  // heights follow the length, but no superstructure towers over 40 m
  const up = Math.min(length, MAX_SUPERSTRUCTURE_M / model.top);
  return { cls, length, beam, scale: [beam / model.beam, length, up], right, ahead };
}

/** Beam and top of a class's model, without building its geometry. */
const shapes = new Map<ShipClass, { beam: number; top: number }>();
function shipModelShape(cls: ShipClass) {
  let s = shapes.get(cls);
  if (!s) {
    const { beam, top } = shipModelArrays(cls);
    shapes.set(cls, (s = { beam, top }));
  }
  return s;
}

/** A starboard/ahead offset in metres turned to `heading` (degrees from north), as east and north. */
export function offsetEastNorth(right: number, ahead: number, heading: number): [number, number] {
  const h = (heading * Math.PI) / 180;
  const [s, c] = [Math.sin(h), Math.cos(h)];
  return [right * c + ahead * s, -right * s + ahead * c];
}
