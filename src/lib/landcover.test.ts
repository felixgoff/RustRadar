// @ts-ignore bun's types are not a dependency; the runner provides the module
import { expect, test } from "bun:test";
import { classAt, LC, nmdClasses, worldCoverClasses } from "./landcover";
import { placeInFile, quadkey } from "./canopy";

const px = (...colours: [number, number, number, number][]) => new Uint8ClampedArray(colours.flat());

test("NMD's colours back to their classes: pine, birch, a mire, a field; clear and unknown are none", () => {
  const c = nmdClasses(px([0x6e, 0x8c, 0x05, 255], [0x4c, 0xe6, 0x00, 255], [0xc2, 0x9e, 0xd7, 255], [0xff, 0xff, 0xbe, 255], [0x6e, 0x8c, 0x05, 0], [1, 2, 3, 255]));
  expect(Array.from(c)).toEqual([LC.CONIFERS, LC.BROADLEAVES, LC.WETLAND, LC.ARABLE, LC.NONE, LC.NONE]);
});

test("WorldCover's codes, its grey", () => {
  const c = worldCoverClasses(px([10, 10, 10, 255], [40, 40, 40, 255], [80, 80, 80, 255], [50, 50, 50, 0]));
  expect(Array.from(c)).toEqual([LC.FOREST, LC.ARABLE, LC.WATER, LC.NONE]);
});

test("a class at a place in its tile", () => {
  const g = { size: 2, classes: new Uint8Array([1, 2, 3, 4]) };
  expect(classAt(g, 0.1, 0.1)).toBe(1);
  expect(classAt(g, 0.9, 0.9)).toBe(4);
  expect(classAt(g, 1, 0)).toBe(2);
});

test("the canopy's files by quadkey: Lund's", () => {
  expect(quadkey(549, 320, 10)).toBe("1202100101");
  const p = placeInFile({ x: 549 * 64 + 3, y: 320 * 64 + 5, z: 16 });
  expect(p.key).toBe("1202100101");
  expect(p.span).toBe(1 / 64);
  expect(p.x).toBeCloseTo(3 / 64, 12);
  expect(p.y).toBeCloseTo(5 / 64, 12);
});

test("a town's register file, back to its trees in a tile", async () => {
  const { parseTown, treesInTile } = await import("./street-trees");
  const buffer = new ArrayBuffer(8 + 20);
  const v = new DataView(buffer);
  new Uint8Array(buffer).set([82, 83, 84, 49]);
  v.setUint32(4, 2, true);
  // sorted by latitude: one in the tile at zoom 14 round Malmö, one north of it
  v.setInt32(8, 13_000_000, true);
  v.setInt32(12, 55_605_000, true);
  v.setUint8(16, 4);
  v.setUint8(17, 12);
  v.setInt32(18, 13_000_000, true);
  v.setInt32(22, 59_000_000, true);
  const town = parseTown(buffer)!;
  const found = treesInTile(town, { x: 8783, y: 5133, z: 14 });
  expect(found).toHaveLength(1);
  expect(found[0].kind).toBe(4);
  expect(found[0].crown).toBe(6);
  expect(found[0].x).toBeGreaterThan(0);
  expect(found[0].x).toBeLessThan(1);
});

test("land cover traced into rounded outlines: a wood and a field, towns and water left out", async () => {
  const { coverOutlines } = await import("./landcover-ground");
  const size = 32;
  const classes = new Uint8Array(size * size);
  for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) classes[y * size + x] = x < 12 ? LC.CONIFERS : x < 24 ? LC.ARABLE : LC.BUILT;
  // a scrap of heath too small to keep
  classes[5 * size + 5] = LC.HEATH;
  const areas = coverOutlines({ size, classes });
  expect(areas).toHaveLength(2);
  const xs = areas.map((a) => a.polygon[0].map(([x]) => x));
  // the wood's outline about its 12 columns, the field's from there to 24
  expect(Math.max(...xs[0])).toBeGreaterThan(10);
  expect(Math.max(...xs[0])).toBeLessThan(14);
  expect(Math.min(...xs[1])).toBeGreaterThan(10);
  expect(Math.max(...xs[1])).toBeLessThan(26);
});
