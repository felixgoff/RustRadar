// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import {
  MAX_SUPERSTRUCTURE_M,
  offsetEastNorth,
  SHIP_CLASSES,
  shipClassOf,
  shipModelArrays,
  shipPlacement,
  TYPICAL_LENGTH,
} from "./ship-models";

describe("ship models", () => {
  for (const cls of SHIP_CLASSES) {
    test(`${cls}: whole triangles, unit normals, inside the unit box`, () => {
      const m = shipModelArrays(cls);
      const vertices = m.positions.length / 3;
      expect(vertices).toBeGreaterThan(0);
      expect(vertices % 3).toBe(0);
      // low-poly: a few hundred triangles to a couple of thousand
      expect(vertices / 3).toBeLessThan(3000);
      expect(m.normals.length).toBe(m.positions.length);
      expect(m.colors.length).toBe(m.positions.length);
      let [minZ, maxZ, minY, maxY, maxX] = [Infinity, -Infinity, Infinity, -Infinity, 0];
      for (let i = 0; i < m.positions.length; i += 3) {
        const [x, y, z] = [m.positions[i], m.positions[i + 1], m.positions[i + 2]];
        expect(Number.isFinite(x + y + z)).toBe(true);
        expect(Math.hypot(m.normals[i], m.normals[i + 1], m.normals[i + 2])).toBeCloseTo(1, 5);
        for (let k = 0; k < 3; k++) {
          expect(m.colors[i + k]).toBeGreaterThanOrEqual(0);
          expect(m.colors[i + k]).toBeLessThanOrEqual(1);
        }
        maxX = Math.max(maxX, Math.abs(x));
        [minY, maxY, minZ, maxZ] = [Math.min(minY, y), Math.max(maxY, y), Math.min(minZ, z), Math.max(maxZ, z)];
      }
      // one unit long, bow to +y, the beam its own share of that
      expect(minY).toBeCloseTo(-0.5, 3);
      expect(maxY).toBeCloseTo(0.5, 3);
      // (fenders and window bands may stand a hair proud of the hull)
      expect(maxX).toBeLessThanOrEqual((m.beam / 2) * 1.02);
      expect(maxX).toBeGreaterThanOrEqual(m.beam / 2 - 1e-3);
      expect(m.beam).toBeLessThanOrEqual(0.5);
      // sits on the waterline with only a little below it
      expect(minZ).toBeLessThan(0);
      expect(minZ).toBeGreaterThan(-0.1);
      expect(maxZ).toBeLessThanOrEqual(1);
      expect(m.top).toBeGreaterThan(0);
      expect(m.top).toBeLessThanOrEqual(maxZ + 1e-6);
    });
  }

  test("the hull's normals face outward: the bow's sides lean forward, the deck up", () => {
    const m = shipModelArrays("generic");
    let forward = 0;
    let up = 0;
    for (let i = 0; i < m.positions.length; i += 3) {
      if (m.positions[i + 1] > 0.4 && Math.abs(m.normals[i]) > 0.3) forward += m.normals[i + 1] > 0 ? 1 : -1;
      if (m.normals[i + 2] > 0.99) up++;
    }
    expect(forward).toBeGreaterThan(0);
    expect(up).toBeGreaterThan(0);
  });
});

describe("shipClassOf", () => {
  const ship = (kind: number, length: number | null, mmsi = 1, size: 0 | 1 | 2 = 1) => ({
    kind,
    size,
    mmsi,
    dims: length === null ? null : ([length / 2, length / 2, 5, 5] as [number, number, number, number]),
  });
  test("by type colour and length", () => {
    expect(shipClassOf(ship(6, 200))).toBe("tanker");
    expect(shipClassOf(ship(3, 150))).toBe("passenger");
    expect(shipClassOf(ship(2, 30))).toBe("tug");
    expect(shipClassOf(ship(5, 20))).toBe("fishing");
    expect(shipClassOf(ship(8, 12))).toBe("pleasure");
    expect(shipClassOf(ship(7, 18))).toBe("sar");
    expect(shipClassOf(ship(0, 80))).toBe("generic");
    expect(shipClassOf(ship(9, 80))).toBe("generic");
    expect(shipClassOf(ship(4, 90))).toBe("cargo");
    expect(shipClassOf(ship(4, 300, 1))).toBe("container");
    expect(shipClassOf(ship(4, 300, 3))).toBe("bulk");
    // without dimensions the size bucket decides
    expect(shipClassOf(ship(4, null, 1, 2))).toBe("container");
    expect(shipClassOf(ship(4, null, 1, 0))).toBe("cargo");
  });
});

describe("shipPlacement", () => {
  test("true length and beam, centred between bow and stern, port and starboard", () => {
    const p = shipPlacement({ kind: 6, size: 2, mmsi: 1, dims: [200, 50, 10, 30] });
    expect(p.cls).toBe("tanker");
    expect(p.length).toBe(250);
    expect(p.beam).toBe(40);
    expect(p.ahead).toBe(75);
    expect(p.right).toBe(10);
    const m = shipModelArrays("tanker");
    expect(p.scale[0] * m.beam).toBeCloseTo(40, 6);
    expect(p.scale[1]).toBe(250);
    // the bridge is squashed to the cap
    expect(p.scale[2] * m.top).toBeLessThanOrEqual(MAX_SUPERSTRUCTURE_M + 1e-6);
  });

  test("a small boat keeps its proportions", () => {
    const p = shipPlacement({ kind: 8, size: 0, mmsi: 1, dims: [6, 6, 2, 2] });
    expect(p.scale[2]).toBe(12);
  });

  test("without dimensions: a typical size, centred", () => {
    const p = shipPlacement({ kind: 2, size: 1, mmsi: 1, dims: null });
    expect(p.length).toBe(TYPICAL_LENGTH.tug[1]);
    expect(p.right).toBe(0);
    expect(p.ahead).toBe(0);
    expect(p.scale[0]).toBeCloseTo(p.scale[1], 6);
    // nonsense dimensions are ignored
    expect(shipPlacement({ kind: 2, size: 1, mmsi: 1, dims: [0, 0, 0, 0] }).length).toBe(TYPICAL_LENGTH.tug[1]);
  });

  test("offsets turn with the heading", () => {
    const [e, n] = offsetEastNorth(0, 100, 90);
    expect(e).toBeCloseTo(100, 9);
    expect(n).toBeCloseTo(0, 9);
    const [e2, n2] = offsetEastNorth(10, 0, 0);
    expect(e2).toBeCloseTo(10, 9); // starboard is east heading north
    expect(n2).toBeCloseTo(0, 9);
  });
});
