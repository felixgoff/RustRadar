// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { buildingShadows, SHADOW_LENGTH_CAP, shadowStep, type Footprints } from "./building-shadow";

const DEG = Math.PI / 180;
type P2 = [number, number];

/** Toward a sun `altitude` degrees up, from the compass `azimuth` (degrees clockwise from north). */
const sunAt = (altitude: number, azimuth: number) => [
  Math.cos(altitude * DEG) * Math.sin(azimuth * DEG),
  Math.cos(altitude * DEG) * Math.cos(azimuth * DEG),
  Math.sin(altitude * DEG),
];

/** Polygons (each a list of rings, outline first) in the binary layout. */
function footprints(polygons: P2[][][], closed = false): Footprints {
  const positions: number[] = [];
  const polygonStarts: number[] = [];
  const ringStarts: number[] = [];
  for (const rings of polygons) {
    polygonStarts.push(positions.length / 2);
    for (const ring of rings) {
      ringStarts.push(positions.length / 2);
      for (const [x, y] of closed ? [...ring, ring[0]] : ring) positions.push(x, y);
    }
  }
  polygonStarts.push(positions.length / 2);
  ringStarts.push(positions.length / 2);
  return { positions, size: 2, polygonStarts, ringStarts };
}

const cross = (o: P2, a: P2, b: P2) => (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);

/** Even-odd: holes count as outside. */
function inside(p: P2, rings: P2[][]): boolean {
  let odd = false;
  for (const ring of rings) {
    for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
      const [xi, yi] = ring[i];
      const [xj, yj] = ring[j];
      if (yi > p[1] !== yj > p[1] && p[0] < ((xj - xi) * (p[1] - yi)) / (yj - yi) + xi) odd = !odd;
    }
  }
  return odd;
}

const segmentsCross = (a: P2, b: P2, c: P2, d: P2) =>
  Math.sign(cross(a, b, c)) !== Math.sign(cross(a, b, d)) && Math.sign(cross(c, d, a)) !== Math.sign(cross(c, d, b));

/** Ground point `p` is in the shadow when the ray from it toward the sun meets the building below its roof. */
function shadowedExactly(p: P2, rings: P2[][], d: P2): boolean {
  const q: P2 = [p[0] - d[0], p[1] - d[1]];
  if (inside(p, rings) || inside(q, rings)) return true;
  return rings.some((ring) => ring.some((a, i) => segmentsCross(p, q, a, ring[(i + 1) % ring.length])));
}

function inMesh(p: P2, mesh: { positions: Float32Array; indices: Uint32Array }): boolean {
  const at = (i: number): P2 => [mesh.positions[2 * i], mesh.positions[2 * i + 1]];
  for (let t = 0; t < mesh.indices.length; t += 3) {
    const [a, b, c] = [at(mesh.indices[t]), at(mesh.indices[t + 1]), at(mesh.indices[t + 2])];
    const s = [cross(a, b, p), cross(b, c, p), cross(c, a, p)];
    if (s.every((v) => v >= 0) || s.every((v) => v <= 0)) return true;
  }
  return false;
}

/** The drawn shadow plus the (hidden) footprint covers exactly the true shadow. */
function expectExact(rings: P2[][], height: number, toSun: number[], closed = false) {
  const step = shadowStep(toSun)!;
  const mesh = buildingShadows(footprints([rings], closed), [height], step);
  expect(mesh).not.toBeNull();
  const d: P2 = [step[0] * height, step[1] * height];
  let shadowed = 0;
  // a grid offset by irrational amounts, so no sample sits on an edge
  for (let x = -120 + Math.SQRT2 / 7; x < 120; x += 1.7) {
    for (let y = -120 + Math.PI / 11; y < 120; y += 1.7) {
      const p: P2 = [x, y];
      const expected = shadowedExactly(p, rings, d);
      expect(inMesh(p, mesh!) || inside(p, rings)).toBe(expected);
      if (expected && !inside(p, rings)) shadowed++;
    }
  }
  expect(shadowed).toBeGreaterThan(50);
}

describe("shadowStep", () => {
  test("is height / tan(altitude) away from the sun", () => {
    const [x, y] = shadowStep(sunAt(45, 90))!; // sun in the east, 45 degrees up
    expect(x).toBeCloseTo(-1, 12);
    expect(y).toBeCloseTo(0, 12);
    const [x2, y2] = shadowStep(sunAt(30, 180))!; // in the south: the shadow points north
    expect(x2).toBeCloseTo(0, 12);
    expect(y2).toBeCloseTo(1 / Math.tan(30 * DEG), 12);
  });

  test("none at night or with the sun on the horizon", () => {
    expect(shadowStep(sunAt(-5, 120))).toBeNull();
    expect(shadowStep([1, 0, 0])).toBeNull();
  });

  test("caps the length at a low sun", () => {
    const [x, y] = shadowStep(sunAt(2, 225))!;
    expect(Math.hypot(x, y)).toBeCloseTo(SHADOW_LENGTH_CAP, 9);
    // still away from the sun (south-west), so toward the north-east
    expect(x).toBeGreaterThan(0);
    expect(y).toBeCloseTo(x, 9);
    // a higher sun is under the cap and unchanged
    expect(Math.hypot(...shadowStep(sunAt(10, 225))!)).toBeCloseTo(1 / Math.tan(10 * DEG), 9);
  });

  test("straight down with the sun overhead", () => {
    expect(shadowStep([0, 0, 1])).toEqual([0, 0]);
  });
});

describe("buildingShadows", () => {
  const box: P2[] = [[0, 0], [20, 0], [20, 10], [0, 10]];
  const lShape: P2[] = [[0, 0], [40, 0], [40, 10], [10, 10], [10, 30], [0, 30]];

  test("a box's shadow, either winding, open or closed rings", () => {
    for (const sun of [sunAt(40, 135), sunAt(25, 300), sunAt(60, 10), sunAt(15, 90)]) {
      expectExact([box], 30, sun);
      expectExact([[...box].reverse()], 30, sun);
      expectExact([box], 30, sun, true);
    }
  });

  test("a box under a sun from the south-east: walls facing away only", () => {
    // sun 45 degrees up in the south-east: the west and north walls cast
    const mesh = buildingShadows(footprints([[box]]), [10], shadowStep(sunAt(45, 135))!)!;
    expect(mesh.indices.length).toBe(2 * 6);
    // each parallelogram has the wall's length times the shadow's reach across it
    const d = 10 * Math.SQRT1_2;
    let area = 0;
    for (let t = 0; t < mesh.indices.length; t += 3) {
      const at = (i: number): P2 => [mesh.positions[2 * mesh.indices[t + i]], mesh.positions[2 * mesh.indices[t + i] + 1]];
      area += Math.abs(cross(at(0), at(1), at(2))) / 2;
    }
    expect(area).toBeCloseTo(20 * d + 10 * d, 3);
  });

  test("an L-shape's shadow is exact, including the inner corner", () => {
    for (const sun of [sunAt(30, 45), sunAt(30, 225), sunAt(50, 160), sunAt(20, 280)]) {
      expectExact([lShape], 25, sun);
      expectExact([[...lShape].reverse()], 25, sun);
    }
  });

  test("a courtyard's walls shade the yard", () => {
    const outer: P2[] = [[0, 0], [60, 0], [60, 60], [0, 60]];
    const yard: P2[] = [[20, 20], [20, 40], [40, 40], [40, 20]]; // the opposite winding, as tiles have it
    expectExact([outer, yard], 15, sunAt(35, 200));
    expectExact([outer, [...yard].reverse()], 15, sunAt(35, 200));
  });

  test("buildings of different heights in one call", () => {
    const tall: P2[] = [[50, 50], [60, 50], [60, 60], [50, 60]];
    const step = shadowStep(sunAt(30, 180))!;
    const mesh = buildingShadows(footprints([[box], [tall]]), [10, 40], step)!;
    const far = Math.max(...Array.from(mesh.positions).filter((_, i) => i % 2 === 1));
    expect(far).toBeCloseTo(60 + 40 * step[1], 3);
  });

  test("nothing with no height, or with the sun overhead", () => {
    expect(buildingShadows(footprints([[box]]), [0], shadowStep(sunAt(30, 90))!)).toBeNull();
    expect(buildingShadows(footprints([[box]]), [20], shadowStep([0, 0, 1])!)).toBeNull();
  });
});
