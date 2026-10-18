// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { blockOf, coverOf, leafPhase, plantTile, tilesAround } from "./tree-layer";
import { cardArrays } from "./tree-models";

describe("trees", () => {
  test("woods, parks and gardens grow trees; roads, buildings and water keep them off", () => {
    expect(coverOf({ layerName: "landcover", class: "wood" })).toBeGreaterThan(0);
    expect(coverOf({ layerName: "landcover", class: "grass", subclass: "park" })).toBeGreaterThan(0);
    expect(coverOf({ layerName: "landuse", class: "industrial" })).toBe(0);
    // woods win over the parks and streets they lie in
    expect(coverOf({ layerName: "landcover", class: "wood" })).toBeGreaterThan(coverOf({ layerName: "landuse", class: "residential" }));
    expect(blockOf({ layerName: "building" }, "Polygon")).toBe(true);
    expect(blockOf({ layerName: "water", class: "lake" }, "MultiPolygon")).toBe(true);
    expect(blockOf({ layerName: "transportation", class: "rail" }, "LineString")).toBeGreaterThan(2);
    expect(blockOf({ layerName: "transportation", class: "rail", brunnel: "tunnel" }, "LineString")).toBeGreaterThan(0);
    expect(blockOf({ layerName: "transportation", class: "ferry" }, "LineString")).toBe(0);
  });

  // a tile near Malmö: a square of wood with a railway straight through it, and a building in it
  const tile = { x: 8783, y: 5133, z: 14 };
  const wood = { geometry: { type: "Polygon", coordinates: [[[0.2, 0.2], [0.6, 0.2], [0.6, 0.6], [0.2, 0.6], [0.2, 0.2]]] }, properties: { layerName: "landcover", class: "wood" } };
  const rail = { geometry: { type: "LineString", coordinates: [[0, 0.4], [1, 0.4]] }, properties: { layerName: "transportation", class: "rail" } };
  const building = { geometry: { type: "Polygon", coordinates: [[[0.25, 0.25], [0.3, 0.25], [0.3, 0.3], [0.25, 0.3], [0.25, 0.25]]] }, properties: { layerName: "building" } };
  const date = new Date(Date.UTC(2026, 6, 1));
  const planted = plantTile([wood, rail, building], tile, date);
  const n = 2 ** 14;
  const all = [planted.conifers, planted.broadleaves].flatMap((t) =>
    Array.from({ length: t.length }, (_, k) => {
      // back to the tile's own coordinates
      const [lon, lat] = [t.positions[k * 3], t.positions[k * 3 + 1]];
      const x = ((lon + 180) / 360) * n - tile.x;
      const r = (lat * Math.PI) / 180;
      const y = ((1 - Math.log(Math.tan(r) + 1 / Math.cos(r)) / Math.PI) / 2) * n - tile.y;
      return [x, y, t.matrices[k * 12 + 8]];
    }),
  );

  test("a wood is planted thick, inside it only, and off its railway and building", () => {
    // about 550 m square at 6 m apart: thousands of trees
    expect(all.length).toBeGreaterThan(3000);
    for (const [x, y] of all) {
      expect(x > 0.199 && x < 0.601 && y > 0.199 && y < 0.601).toBe(true);
      expect(Math.abs(y - 0.4)).toBeGreaterThan(0.001);
      expect(x > 0.25 && x < 0.3 && y > 0.25 && y < 0.3).toBe(false);
    }
    // as tall as a wood's trees (their cards up to Streets GL's margin taller)
    for (const [, , height] of all) expect(height >= 13 && height <= 24 * 1.43).toBe(true);
  });

  test("the same trees every time", () => {
    const again = plantTile([wood, rail, building], tile, date);
    expect(Array.from(again.conifers.positions)).toEqual(Array.from(planted.conifers.positions));
    expect(Array.from(again.broadleaves.colors)).toEqual(Array.from(planted.broadleaves.colors));
  });

  test("broadleaves green in summer, turning in autumn, bare in winter; the tropics always green", () => {
    expect(leafPhase(new Date(Date.UTC(2026, 6, 10)), 56).summer).toBe(1);
    const october = leafPhase(new Date(Date.UTC(2026, 9, 16)), 56);
    expect(october.autumn).toBeGreaterThan(0.5);
    expect(leafPhase(new Date(Date.UTC(2026, 0, 15)), 56).bare).toBe(1);
    // the southern hemisphere's January is summer
    expect(leafPhase(new Date(Date.UTC(2026, 0, 15)), -45).summer).toBe(1);
    expect(leafPhase(new Date(Date.UTC(2026, 0, 15)), 5).summer).toBe(1);
  });

  test("the tiles around a point, nearest first", () => {
    const tiles = tilesAround(13.0, 55.605, 1500);
    expect(tiles[0].tile).toEqual({ x: 8783, y: 5133, z: 14 });
    expect(tiles[0].distance).toBe(0);
    for (let i = 1; i < tiles.length; i++) expect(tiles[i].distance).toBeGreaterThanOrEqual(tiles[i - 1].distance);
    expect(tiles.length).toBeGreaterThan(3);
  });

  test("the cards: two crossed quads a unit wide and tall, on the ground, the picture's top at the top", () => {
    const a = cardArrays();
    expect(a.positions.length).toBe(2 * 6 * 3);
    expect(a.normals.length).toBe(a.positions.length);
    expect(a.texCoords.length).toBe((a.positions.length / 3) * 2);
    const zs = a.positions.filter((_, i) => i % 3 === 2);
    expect([Math.min(...zs), Math.max(...zs)]).toEqual([0, 1]);
    for (let k = 0; k < 12; k++) expect(a.texCoords[k * 2 + 1]).toBe(1 - zs[k]);
    // the second card is the first turned a quarter
    expect(a.normals.slice(0, 3)).toEqual([0, -1, 0]);
    expect(a.normals.slice(18, 21)).toEqual([1, 0, 0]);
  });
});
