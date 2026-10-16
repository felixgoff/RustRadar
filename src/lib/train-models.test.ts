// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { carArrays, carPlaces } from "./train-models";

describe("train models", () => {
  test("a car is a closed unit box of triangles, its body taking the train's colour", () => {
    for (const lead of [false, true]) {
      const a = carArrays(lead);
      expect(a.positions.length % 9).toBe(0);
      expect(a.normals.length).toBe(a.positions.length);
      expect(a.colors.length).toBe(a.positions.length);
      const xs = a.positions.filter((_, i) => i % 3 === 0);
      const ys = a.positions.filter((_, i) => i % 3 === 1);
      const zs = a.positions.filter((_, i) => i % 3 === 2);
      expect(Math.min(...xs)).toBe(-0.5);
      expect(Math.max(...xs)).toBe(0.5);
      expect(Math.max(...ys)).toBe(0.5);
      expect(Math.min(...zs)).toBe(0);
      expect(Math.max(...zs)).toBe(1);
      // some of it full colour (the body), some dark (windows, underframe)
      expect(a.colors.some((c) => c === 1)).toBe(true);
      expect(a.colors.some((c) => c < 0.25)).toBe(true);
    }
  });

  test("each car lies halfway along it, pointing from its back to its front", () => {
    // 24 m east along 60°N, then one north
    const east: [number, number][] = [
      [18, 60],
      [18 + 24 / 55_660, 60],
    ];
    const north: [number, number][] = [
      [18, 60],
      [18, 60 + 24 / 111_320],
    ];
    const [a, b] = carPlaces([east, north]);
    expect(a.heading).toBeCloseTo(90, 0);
    expect(a.length).toBeCloseTo(24, 0);
    expect(a.at[0]).toBeCloseTo(18 + 12 / 55_660, 6);
    expect(b.heading).toBeCloseTo(0, 0);
    expect(carPlaces([[[18, 60]]])).toEqual([]);
  });
});
