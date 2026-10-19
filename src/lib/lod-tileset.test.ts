// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { WebMercatorViewport } from "@deck.gl/core";
import { distanceTileIndices, LodTileset } from "./lod-tileset";

describe("tiles by their distance from the camera", () => {
  const view = (zoom: number, pitch: number) => new WebMercatorViewport({ width: 1100, height: 720, longitude: 13.19, latitude: 55.705, zoom, pitch, bearing: 20 });
  const zooms = (tiles: { z: number }[]) => [...new Set(tiles.map((t) => t.z))].sort((a, b) => a - b);

  test("a tilted view: fine near the camera, coarser off, and only the zooms there are", () => {
    const tiles = distanceTileIndices(view(16, 75), { levels: [9, 10, 11, 12, 14, 16], bias: 0.5 });
    expect(zooms(tiles)).toContain(16);
    expect(zooms(tiles)).toContain(14);
    expect(tiles.every((t) => [9, 10, 11, 12, 14, 16].includes(t.z))).toBe(true);
    // tiles never overlap: none inside another
    const inside = (a: { x: number; y: number; z: number }, b: { x: number; y: number; z: number }) =>
      a.z > b.z && a.x >> (a.z - b.z) === b.x && a.y >> (a.z - b.z) === b.y;
    expect(tiles.some((a) => tiles.some((b) => inside(a, b)))).toBe(false);
  });

  test("straight down: the same everywhere; further up: coarser", () => {
    expect(zooms(distanceTileIndices(view(16, 0), { levels: [12, 14, 16], bias: 0.5 }))).toEqual([16]);
    expect(zooms(distanceTileIndices(view(13, 0), { levels: [12, 14, 16], bias: 0 }))).toEqual([14]);
  });

  test("nothing past the farthest", () => {
    const all = distanceTileIndices(view(15, 75), { levels: [9, 10, 11, 12, 14, 16], bias: 2 });
    const near = distanceTileIndices(view(15, 75), { levels: [9, 10, 11, 12, 14, 16], bias: 2, maxDistance: 10_000 });
    expect(near.length).toBeLessThan(all.length);
  });
});

describe("loading ahead while zooming", () => {
  const view = (zoom: number) => new WebMercatorViewport({ width: 1100, height: 720, longitude: 13.19, latitude: 55.705, zoom, pitch: 60, bearing: 20 });
  const lod = { levels: [13, 14, 16], bias: 0.5, maxDistance: 25_000 };
  const tileset = () => {
    const asked: string[] = [];
    const set = new LodTileset({ getTileData: ({ index }: { index: { x: number; y: number; z: number } }) => (asked.push(`${index.z}/${index.x}/${index.y}`), new Promise(() => {})), lod, tileSize: 512, maxRequests: 0 } as never);
    return { set, asked };
  };
  const inView = (zoom: number) => new Set(distanceTileIndices(view(zoom), lod).map((i) => `${i.z}/${i.x}/${i.y}`));

  test("zooming in asks for the tiles of a level further in, besides the view's", async () => {
    const { set, asked } = tileset();
    set.update(view(13.8));
    set.update(view(14));
    await new Promise((r) => setTimeout(r, 0));
    const now = inView(14);
    const extra = asked.filter((id) => !now.has(id) && !inView(13.8).has(id));
    expect(extra.length).toBeGreaterThan(0);
    expect(extra.every((id) => inView(15).has(id))).toBe(true);
  });

  test("standing still asks for nothing more", async () => {
    const { set, asked } = tileset();
    set.update(view(14));
    await new Promise((r) => setTimeout(r, 0));
    expect(new Set(asked)).toEqual(inView(14));
  });
});
