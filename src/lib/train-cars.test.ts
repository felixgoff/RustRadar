// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { CAR_M, carLines } from "./train-cars";

// east along 60°N: a degree of longitude is about 55.66 km
const DEG_PER_M = 1 / 55_660;
const at = (m: number): [number, number] => [60, 18 + m * DEG_PER_M];
const lonM = (lon: number) => (lon - 18) / DEG_PER_M;

describe("carLines", () => {
  // at 0 m when frames start, 20 m/s east; the track behind it given back 200 m
  const frames: [number, number, number][] = [
    [...at(0), 0],
    [...at(200), 10_000],
  ];
  const tail: [number, number][] = [at(0), at(-100), at(-200)];

  test("its cars back from where it is drawn now, front first", () => {
    const cars = carLines(frames, tail, 3, 5_000);
    expect(cars.length).toBe(3);
    // the head is 100 m on; the first car from there back, the last ending 78 m back
    expect(lonM(cars[0][cars[0].length - 1][0])).toBeCloseTo(100, 0);
    expect(lonM(cars[2][0][0])).toBeCloseTo(100 - 3 * CAR_M + 2.5, 0);
  });

  test("standing where its frames start, its cars lie along the track behind", () => {
    const cars = carLines(frames, tail, 4, 0);
    expect(lonM(cars[0][cars[0].length - 1][0])).toBeCloseTo(0, 0);
    expect(lonM(cars[3][0][0])).toBeCloseTo(-4 * CAR_M + 2.5, 0);
  });

  test("without the track behind, straight on back", () => {
    const cars = carLines(frames, [], 2, 0);
    expect(lonM(cars[1][0][0])).toBeCloseTo(-2 * CAR_M + 2.5, 0);
  });

  test("drawn larger than life, the cars still follow the track behind, then straight on back", () => {
    const cars = carLines(frames, tail, 4, 0, 80);
    expect(cars.length).toBe(4);
    // each car 80 m of the way, less a gap in proportion
    const gap = 2.5 * (80 / CAR_M);
    expect(lonM(cars[1][cars[1].length - 1][0])).toBeCloseTo(-80, 0);
    expect(lonM(cars[1][0][0])).toBeCloseTo(-160 + gap, 0);
    // past the 200 m of track known behind it, on in the same line
    expect(lonM(cars[3][0][0])).toBeCloseTo(-320 + gap, 0);
    expect(cars[3][0][1]).toBeCloseTo(60, 9);
  });

  test("no frames, no cars", () => {
    expect(carLines([], tail, 3, 0)).toEqual([]);
  });
});
