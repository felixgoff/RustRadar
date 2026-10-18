// @ts-ignore bun's types are not a dependency; the runner provides the module
import { expect, test } from "bun:test";
import { inside, tallBuildings } from "./overture";

const square = (x: number, y: number, s: number): [number, number][] => [
  [x, y],
  [x + s, y],
  [x + s, y + s],
  [x, y + s],
  [x, y],
];

test("a point in a building's outline", () => {
  expect(inside([0.5, 0.5], square(0, 0, 1))).toBe(true);
  expect(inside([1.5, 0.5], square(0, 0, 1))).toBe(false);
});

test("only the buildings with a height or floors are kept, each part of a multipolygon", () => {
  const kept = tallBuildings([
    { geometry: { type: "Polygon", coordinates: [square(0, 0, 1)] }, properties: { height: 12 } },
    { geometry: { type: "MultiPolygon", coordinates: [[square(2, 0, 1)], [square(4, 0, 1)]] }, properties: { num_floors: 3 } },
    { geometry: { type: "Polygon", coordinates: [square(6, 0, 1)] }, properties: {} },
  ]);
  expect(kept.map((b) => [b.height, b.floors])).toEqual([
    [12, 0],
    [0, 3],
    [0, 3],
  ]);
  expect(kept[1].box).toEqual([2, 0, 3, 1]);
});
