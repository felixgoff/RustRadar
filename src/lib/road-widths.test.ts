// @ts-ignore bun's types are not a dependency; the runner provides the module
import { expect, test } from "bun:test";
import { addRoadPiecesForTest, cellOf, stretchAt, withRoadWidths } from "./road-widths";

// a road running east along 55.7°N near Lund, 9 m wide, and a gravel one north–south across it
addRoadPiecesForTest([
  { line: [[13.18, 55.7], [13.2, 55.7]], width: 9, paved: true },
  { line: [[13.19, 55.69], [13.19, 55.71]], width: 4.5, paved: false },
]);

test("a road takes the stretch it is on and runs along", () => {
  expect(stretchAt([13.185, 55.70003], 90)?.width).toBe(9);
  // either way along it
  expect(stretchAt([13.185, 55.70003], 270)?.width).toBe(9);
  // across it, the crossing road's
  expect(stretchAt([13.19, 55.6999], 0)?.width).toBe(4.5);
  // too far off, or the wrong way
  expect(stretchAt([13.185, 55.7003], 90)).toBeNull();
  expect(stretchAt([13.185, 55.70003], 45)).toBeNull();
});

test("a tile's roads carry their stretches' widths and surfaces; its paths don't", () => {
  // the zoom-16 tile round 13.185, 55.7, and a line along 55.7° through it, and one of a path
  const z = 16;
  const n = 2 ** z;
  const tx = ((13.185 + 180) / 360) * n;
  const r = (55.7 * Math.PI) / 180;
  const ty = ((1 - Math.log(Math.tan(r) + 1 / Math.cos(r)) / Math.PI) / 2) * n;
  const tile = { x: Math.floor(tx), y: Math.floor(ty), z };
  const [x, y] = [tx - tile.x, ty - tile.y];
  const lines = {
    type: "LineString",
    positions: { value: new Float64Array([x - 0.2, y, x + 0.2, y, x - 0.2, y, x + 0.2, y]), size: 2 },
    pathIndices: { value: new Uint32Array([0, 2, 4]), size: 1 },
    featureIds: { value: new Uint32Array([0, 0, 1, 1]), size: 1 },
    globalFeatureIds: { value: new Uint32Array([0, 0, 1, 1]), size: 1 },
    numericProps: {},
    properties: [{ road: true }, { road: false }],
  };
  const out = withRoadWidths(lines, tile, (p) => p.road === true);
  expect(Array.from(out.numericProps.nvdbWidth.value)).toEqual([9, 9, 0, 0]);
  expect(Array.from(out.numericProps.nvdbGravel.value)).toEqual([0, 0, 0, 0]);
  // the same object again, while no more have come
  expect(withRoadWidths(lines, tile, (p) => p.road === true)).toBe(out);
});

test("the backend's squares, the same as roads.rs's", () => {
  const { key, centre } = cellOf(13.19, 55.705);
  expect(cellOf(centre[0], centre[1]).key).toBe(key);
});
