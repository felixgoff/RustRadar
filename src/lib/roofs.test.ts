// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { boundingRectangle, roofParts } from "./roofs";

// a house 12 m east–west, 8 m north–south
const house: [number, number][] = [
  [0, 0],
  [12, 0],
  [12, 8],
  [0, 8],
];

/** The roof's triangles' horizontal area and the highest point, and the gable walls' area. */
function measure(parts: ReturnType<typeof roofParts>) {
  let [roof, wall, top] = [0, 0, -Infinity];
  for (let t = 0; t < parts.walls.length; t += 3) {
    const v = (k: number) => parts.positions.slice((t + k) * 3, (t + k) * 3 + 3);
    const [a, b, c] = [v(0), v(1), v(2)];
    top = Math.max(top, a[2], b[2], c[2]);
    const [ux, uy, uz] = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    const [wx, wy, wz] = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    const n = [uy * wz - uz * wy, uz * wx - ux * wz, ux * wy - uy * wx];
    if (parts.walls[t]) wall += Math.hypot(...n) / 2;
    else roof += Math.abs(n[2]) / 2;
  }
  return { roof, wall, top };
}

describe("roofParts", () => {
  test("a gabled roof: the ridge along the longer side, gables at the ends", () => {
    const parts = roofParts({ ring: house, base: 6, height: 3, shape: "gabled" });
    const m = measure(parts);
    expect(m.roof).toBeCloseTo(96, 3);
    expect(m.top).toBeCloseTo(9, 5);
    // two triangles 8 m wide, 3 m high
    expect(m.wall).toBeCloseTo(24, 3);
    // the ridge runs east–west: every top corner is on the north–south middle
    for (let i = 0; i < parts.positions.length; i += 3) {
      if (Math.abs(parts.positions[i + 2] - 9) < 1e-6) expect(parts.positions[i + 1]).toBeCloseTo(4, 5);
    }
  });

  test("mapped across: the ridge across the longer side", () => {
    const parts = roofParts({ ring: house, base: 6, height: 3, shape: "gabled", orientation: "across" });
    for (let i = 0; i < parts.positions.length; i += 3) {
      if (Math.abs(parts.positions[i + 2] - 9) < 1e-6) expect(parts.positions[i]).toBeCloseTo(6, 5);
    }
  });

  test("hipped and pyramidal: no gables, the whole footprint covered", () => {
    for (const shape of ["hipped", "pyramidal"]) {
      const m = measure(roofParts({ ring: house, base: 6, height: 3, shape }));
      expect(m.roof).toBeCloseTo(96, 3);
      expect(m.wall).toBeCloseTo(0, 5);
      expect(m.top).toBeCloseTo(9, 5);
    }
  });

  test("a skillion: down the way it faces, a wall under its high edge", () => {
    const parts = roofParts({ ring: house, base: 3, height: 2, shape: "skillion", direction: 180 });
    const m = measure(parts);
    expect(m.roof).toBeCloseTo(96, 3);
    // high along the north side (it faces south)
    for (let i = 0; i < parts.positions.length; i += 3) {
      if (!parts.walls[i / 3] && parts.positions[i + 1] > 7.99) expect(parts.positions[i + 2]).toBeCloseTo(5, 5);
    }
    expect(m.wall).toBeGreaterThan(0);
  });

  test("an L-shaped house: covered whole, gables where its roof stands over the eaves", () => {
    const l: [number, number][] = [
      [0, 0],
      [10, 0],
      [10, 4],
      [4, 4],
      [4, 10],
      [0, 10],
    ];
    const m = measure(roofParts({ ring: l, base: 3, height: 3, shape: "gabled" }));
    expect(m.roof).toBeCloseTo(64, 3);
    expect(m.wall).toBeGreaterThan(0);
  });

  test("round a courtyard: the yard left open, walls under the roof facing into it", () => {
    const yard: [number, number][] = [
      [4, 3],
      [8, 3],
      [8, 5],
      [4, 5],
    ];
    const m = measure(roofParts({ ring: house, holes: [yard], base: 6, height: 3, shape: "hipped" }));
    expect(m.roof).toBeCloseTo(96 - 8, 3);
    expect(m.wall).toBeGreaterThan(0);
  });

  test("flat roofs and degenerate rings: nothing", () => {
    expect(roofParts({ ring: house, base: 6, height: 3, shape: "flat" }).positions).toHaveLength(0);
    expect(roofParts({ ring: [[0, 0], [1, 0]], base: 6, height: 3, shape: "gabled" }).positions).toHaveLength(0);
  });
});

test("the least rectangle of a turned box", () => {
  const a = Math.PI / 6;
  const turn = ([x, y]: [number, number]): [number, number] => [x * Math.cos(a) - y * Math.sin(a), x * Math.sin(a) + y * Math.cos(a)];
  const r = boundingRectangle(house.map(turn));
  expect(r.half[0] * r.half[1] * 4).toBeCloseTo(96, 5);
});
