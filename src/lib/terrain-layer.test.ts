// @ts-ignore bun's types are not a dependency; the runner provides the module
import { expect, test } from "bun:test";
import {
  decodeTerrarium,
  hillshade,
  hillshadeFade,
  HILLSHADE_STYLE,
  reliefExaggeration,
  terrainLayers,
  terrainNeedsGroundElevation,
  tileRowMetersPerPixel,
} from "./terrain-layer";

/** Terrarium RGB for a height, as the tiles encode it. */
function encode(h: number): [number, number, number] {
  const v = h + 32768;
  const r = Math.floor(v / 256);
  const g = Math.floor(v - r * 256);
  const b = Math.round((v - Math.floor(v)) * 256);
  return [r, g, b];
}

test("terrarium pixels decode to metres", () => {
  const heights = [0, 4807.5, -10994, 1.25];
  const rgba = new Uint8ClampedArray(heights.flatMap((h) => [...encode(h), 255]));
  const out = decodeTerrarium(rgba);
  heights.forEach((h, i) => expect(out[i]).toBeCloseTo(h, 2));
  // sea level is 128, 0, 0
  expect(encode(0)).toEqual([128, 0, 0]);
});

test("decoding reuses a buffer that is big enough", () => {
  const buffer = new Float32Array(8);
  expect(decodeTerrarium(new Uint8ClampedArray(16), buffer)).toBe(buffer);
  expect(decodeTerrarium(new Uint8ClampedArray(64), buffer)).not.toBe(buffer);
});

test("a tile's pixels are about 611 m at the equator at zoom 8 and shrink towards the poles", () => {
  // the row just above the equator in a zoom-8 tile
  expect(tileRowMetersPerPixel(8, 127, 255)).toBeCloseTo(611.5, 0);
  const z8Alps = tileRowMetersPerPixel(8, 90, 128); // about 46.5 N
  expect(z8Alps).toBeGreaterThan(400);
  expect(z8Alps).toBeLessThan(430);
  expect(tileRowMetersPerPixel(8, 90, 0)).toBeLessThan(tileRowMetersPerPixel(8, 90, 255));
});

/** A W x H grid sloping up towards `bearing` (degrees from north) with `grade` (rise per metre). */
function slope(w: number, h: number, grade: number, bearing: number, mpp = 10): Float32Array {
  const ex = Math.sin((bearing * Math.PI) / 180);
  const ny = Math.cos((bearing * Math.PI) / 180);
  const out = new Float32Array(w * h);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) out[y * w + x] = 1000 + grade * mpp * (x * ex - y * ny);
  return out;
}
const alphaAt = (rgba: Uint8ClampedArray, w: number, x: number, y: number) => rgba[(y * w + x) * 4 + 3];
const isShadow = (rgba: Uint8ClampedArray, w: number, x: number, y: number) =>
  rgba[(y * w + x) * 4] === HILLSHADE_STYLE.shadow[0] && rgba[(y * w + x) * 4 + 1] === HILLSHADE_STYLE.shadow[1];

test("flat ground is fully transparent", () => {
  const rgba = hillshade(new Float32Array(16).fill(812), 4, 4, () => 10);
  for (let i = 3; i < rgba.length; i += 4) expect(rgba[i]).toBe(0);
});

test("a slope facing the north-west light is lit, one facing away is shaded", () => {
  const w = 5;
  // rising towards the south-east: the face looks north-west, into the light
  const lit = hillshade(slope(w, w, 0.5, 135), w, w, () => 10);
  expect(isShadow(lit, w, 2, 2)).toBe(false);
  expect(alphaAt(lit, w, 2, 2)).toBeGreaterThan(0);
  // rising towards the north-west: facing away
  const dark = hillshade(slope(w, w, 0.5, 315), w, w, () => 10);
  expect(isShadow(dark, w, 2, 2)).toBe(true);
  expect(alphaAt(dark, w, 2, 2)).toBeGreaterThan(alphaAt(lit, w, 2, 2));
});

test("steeper slopes shade darker, up to the cap", () => {
  const w = 5;
  const gentle = alphaAt(hillshade(slope(w, w, 0.1, 315), w, w, () => 10), w, 2, 2);
  const steep = alphaAt(hillshade(slope(w, w, 0.4, 315), w, w, () => 10), w, 2, 2);
  const cliff = alphaAt(hillshade(slope(w, w, 20, 315), w, w, () => 10), w, 2, 2);
  expect(steep).toBeGreaterThan(gentle);
  expect(cliff).toBeLessThanOrEqual(HILLSHADE_STYLE.shadowAlpha);
});

test("edge pixels shade like the middle, so tiles meet without a seam", () => {
  const w = 6;
  const rgba = hillshade(slope(w, w, 0.3, 300), w, w, () => 10);
  const middle = alphaAt(rgba, w, 2, 2);
  for (const [x, y] of [[0, 0], [w - 1, 0], [0, w - 1], [w - 1, w - 1], [0, 3], [3, 0]]) {
    expect(Math.abs(alphaAt(rgba, w, x, y) - middle)).toBeLessThanOrEqual(1);
  }
});

test("the sea floor isn't shaded", () => {
  const w = 5;
  const seabed = slope(w, w, 0.5, 315).map((h) => h - 5000);
  const rgba = hillshade(seabed, w, w, () => 10);
  for (let i = 3; i < rgba.length; i += 4) expect(rgba[i]).toBe(0);
});

test("zoomed out, relief is exaggerated more", () => {
  expect(reliefExaggeration(3)).toBeGreaterThan(reliefExaggeration(8));
  expect(reliefExaggeration(8)).toBeGreaterThan(reliefExaggeration(14));
  expect(reliefExaggeration(0)).toBeLessThanOrEqual(8);
  expect(reliefExaggeration(15)).toBe(1);
});

test("the hillshade at every zoom, the mesh only when asked for and flat", () => {
  const ids = (o: Parameters<typeof terrainLayers>[0]) => terrainLayers(o).map((l) => l.id);
  expect(ids({ flat: false, zoom: 3, basemap: "dark" })).toEqual(["terrain-hillshade"]);
  expect(ids({ flat: true, zoom: 14, basemap: "dark" })).toEqual(["terrain-hillshade"]);
  expect(ids({ flat: false, zoom: 10, basemap: "dark", mesh: true })).toEqual(["terrain-hillshade"]);
  const meshWorkerUrl = "/terrain-worker.js";
  // the mesh can only be parsed in the bundled worker
  expect(ids({ flat: true, zoom: 13, basemap: "dark", mesh: true })).toEqual(["terrain-hillshade"]);
  expect(ids({ flat: true, zoom: 13, basemap: "dark", mesh: true, meshWorkerUrl })).toEqual(["terrain-mesh"]);
  expect(ids({ flat: true, zoom: 13, basemap: "satellite", mesh: true, meshWorkerUrl, imageryUrl: "https://x/{z}/{y}/{x}" })).toEqual([
    "terrain-mesh-imagery",
  ]);
  expect(terrainNeedsGroundElevation({ flat: true, zoom: 13, mesh: true })).toBe(true);
  expect(terrainNeedsGroundElevation({ flat: true, zoom: 13 })).toBe(false);
});

test("the hillshade is fainter over the imagery", () => {
  const [dark] = terrainLayers({ flat: false, zoom: 8, basemap: "dark" });
  const [sat] = terrainLayers({ flat: false, zoom: 8, basemap: "satellite" });
  expect(sat.props.opacity).toBeLessThan(dark.props.opacity);
});

test("close in, the hillshade fades back but never out", () => {
  expect(hillshadeFade(5)).toBe(1);
  expect(hillshadeFade(11)).toBe(1);
  expect(hillshadeFade(12.5)).toBeLessThan(1);
  expect(hillshadeFade(18)).toBeCloseTo(0.6);
});
