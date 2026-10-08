// Weather radar in 3D. Where NOAA MRMS measures reflectivity aloft, over North
// America, the layers are the real thing; over central Europe DWD's composite
// is finer than the tiles, though its heights are estimated too. Both are
// loaded by the backend (`src-tauri/src/radar.rs`). Everywhere else, RainViewer tiles are split into stacked layers by estimated echo-top
// height, so strong cells still rise higher than drizzle.
//
// All of it is drawn as smooth filled polygons, one per 5 dBZ band in the NWS
// palette (radar-mesh.ts), so the whole globe looks alike.
//
// The public API only serves the "Universal Blue" palette (the raw-dBZ scheme
// returns the same colours), so reflectivity is read back from the colour:
// translucent beige for drizzle, light to dark blue for light to moderate
// rain, yellow to red for heavy rain, pink and white for hail.
//
// This module has no DOM in it (only what workers have too): it runs in the radar workers
// (radar-worker.ts), which radar-pool.ts hands the tiles and volumes to.

import {
  BAND_COUNT,
  bandDbz,
  blur,
  buildMesh,
  buildWalls,
  contourBands,
  parseVolumePolygons,
  polygonWalls,
  type PolygonData,
  type RadarMesh,
  type WallLine,
} from "./radar-mesh";

/** Heights (km) of the stacked layers above the ground layer. */
export const RADAR_LEVELS_KM = [1.5, 3, 4.5, 6, 8, 10, 12];

/** Approximate reflectivity (dBZ) of a Universal Blue pixel. */
export function reflectivity(r: number, g: number, b: number, a: number): number {
  if (a === 0) return -Infinity;
  if (a < 250) return (a / 255) * 10; // drizzle: beige, more opaque as it strengthens
  if (b > g && r < 160) return 10 + (25 * (595 - (r + g + b))) / 420; // blues: light 10 → dark 35
  if (r > 240 && b < 100) return 40 + (15 * (224 - g)) / 224; // yellow 40 → red 55
  return 60; // pinks and white
}

/**
 * Echo top (km) for a reflectivity: a rough climatological fit, from about
 * 2 km in drizzle to 12 km in severe storms.
 */
export function echoTopKm(dbz: number): number {
  return dbz === -Infinity ? 0 : Math.min(12, Math.max(1, 2 + (dbz - 10) * 0.22));
}

/** The weakest band that still reaches `km`: higher layers keep only the stronger echoes. */
const lowestBandAt = (km: number) => {
  let band = 0;
  while (band < BAND_COUNT && echoTopKm(bandDbz(band)) < km) band++;
  return band;
};

export interface RadarLevel {
  km: number;
  /** Binary SolidPolygonLayer data, in longitude and latitude; drawn lifted to `km`. */
  data: PolygonData;
  /**
   * Walls around the layer's outlines, z from 0 to 1, stretched from `km`
   * down to the layer drawn below it, so the stack reads as one volume.
   */
  walls: PolygonData | null;
}

/** A RainViewer tile as a worker hands it over: plain typed arrays, transferred, not copied. */
export interface TileMesh {
  /** `z/x/y`: stable between frames, for layer ids. */
  id: string;
  /** West, south, east, north in degrees. */
  bounds: [number, number, number, number];
  /** Every band, flat; drawn on the ground and, from each level's band up, lifted. */
  slices: RadarMesh;
  /** The bands' outlines as walls of unit height (`buildWalls`); null when there are none. */
  walls: RadarMesh | null;
  /**
   * Lifted layers, each with only the bands whose echo tops reach it: those
   * from `fromBand` up, of `slices` and `walls`. Contiguous from the lowest
   * height, so each one's walls run down to the one before.
   */
  levels: { km: number; fromBand: number }[];
}

/** A tile on the main thread, with its ground layer's data made once so the layer keeps it. */
export interface RadarTile extends TileMesh {
  ground: PolygonData | null;
}

/**
 * Where a measured volume (MRMS, DWD) has data: RainViewer is hidden there so
 * the two don't overlap. One byte per cell, north row first.
 */
export interface Coverage {
  /** Changes only when the mask or its bounds do: a new scan with the same coverage keeps it. */
  key: string;
  bounds: [number, number, number, number];
  width: number;
  height: number;
  mask: Uint8Array;
}

/** Where MRMS or DWD has data. */
export type Covered = (lon: number, lat: number) => boolean;

export function coveredBy(coverages: Coverage[]): Covered | undefined {
  if (!coverages.length) return undefined;
  const tests = coverages.map(({ bounds: [west, south, east, north], width: w, height: h, mask }): Covered => {
    return (lon, lat) => {
      if (lon < west || lon > east || lat < south || lat > north) return false;
      // clamped, so a point exactly on the east or south edge still lands in the
      // last cell rather than falling through and leaving a seam
      const x = Math.min(w - 1, Math.floor(((lon - west) / (east - west)) * w));
      const y = Math.min(h - 1, Math.floor(((north - lat) / (north - south)) * h));
      return mask[y * w + x] === 1;
    };
  });
  return (lon, lat) => tests.some((covered) => covered(lon, lat));
}

const intersects = (a: [number, number, number, number], b: [number, number, number, number]) =>
  a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3];

/** The coverages that reach into a tile: only they change what it shows. */
export const coveragesOver = (bounds: [number, number, number, number], coverages: Coverage[]) =>
  coverages.filter((c) => intersects(c.bounds, bounds));

const tileLatitude = (y: number, z: number) => (Math.atan(Math.sinh(Math.PI * (1 - (2 * y) / 2 ** z))) * 180) / Math.PI;

/** West, south, east, north of a Web Mercator tile, in degrees. */
export function tileBounds({ x, y, z }: { x: number; y: number; z: number }): [number, number, number, number] {
  return [(x / 2 ** z) * 360 - 180, tileLatitude(y + 1, z), ((x + 1) / 2 ** z) * 360 - 180, tileLatitude(y, z)];
}

/** The tile's RGBA pixels, or null if it can't be had (cancelled, offline, not found). */
export async function fetchPixels(url: string, signal?: AbortSignal) {
  try {
    const response = await fetch(url, { signal });
    if (!response.ok) return null;
    const image = await createImageBitmap(await response.blob());
    const { width, height } = image;
    const canvas = new OffscreenCanvas(width, height);
    const context = canvas.getContext("2d", { willReadFrequently: true })!;
    context.drawImage(image, 0, 0);
    image.close();
    return { width, height, pixels: context.getImageData(0, 0, width, height).data };
  } catch {
    return null; // cancelled as the view moved on, or offline: the tile just stays empty
  }
}

/**
 * The tile's pixels (RGBA, Universal Blue) as meshes; null when it has no echo.
 * `covered` blanks out what the measured volumes draw themselves.
 */
export function processTile(
  pixels: Uint8ClampedArray | Uint8Array,
  width: number,
  height: number,
  index: { x: number; y: number; z: number },
  covered?: Covered,
): TileMesh | null {
  const bounds = tileBounds(index);
  const [west, , east] = bounds;

  // the colours back to reflectivity, with no echo as 0 dBZ (below the first
  // band) so the blur fades edges out rather than pulling them in hard
  const dbz = new Float32Array(width * height);
  let echo = false;
  for (let r = 0; r < height; r++) {
    // rows are Web Mercator: even steps in y, not in latitude
    const lat = covered ? tileLatitude(index.y + (r + 0.5) / height, index.z) : 0;
    for (let c = 0; c < width; c++) {
      const i = (r * width + c) * 4;
      if (!pixels[i + 3]) continue;
      // blank out what MRMS and DWD cover; they draw it themselves
      if (covered?.(west + ((c + 0.5) / width) * (east - west), lat)) continue;
      const v = Math.max(0, reflectivity(pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]));
      dbz[r * width + c] = v;
      if (v > 0) echo = true;
    }
  }
  // most of the world is dry: nothing to contour
  if (!echo) return null;

  // contoured in the tile's own (Mercator) grid, then placed on the globe
  const lines: WallLine[] = [];
  const polygons = contourBands(blur(dbz, width, height), width, height, lines);
  if (!polygons.length) return null;
  for (const { positions: p } of [...polygons, ...lines]) {
    for (let i = 0; i < p.length; i += 2) {
      p[i] = west + p[i] * (east - west);
      p[i + 1] = tileLatitude(index.y + p[i + 1], index.z);
    }
  }
  const slices = buildMesh(polygons);
  if (!slices.indices.length) return null;
  const walls = lines.length ? buildWalls(lines) : null;
  const levels: TileMesh["levels"] = [];
  for (const km of RADAR_LEVELS_KM) {
    const fromBand = lowestBandAt(km);
    if (slices.bandStarts[fromBand] >= slices.indices.length) break; // higher ones are emptier still
    levels.push({ km, fromBand });
  }
  return { id: `${index.z}/${index.x}/${index.y}`, bounds, slices, walls: walls?.indices.length ? walls : null, levels };
}

/** The bounds and heights of a measured volume, as the backend describes it (`api.RadarVolume`). */
export interface VolumeFrame {
  source: string;
  time: number;
  bounds: { west: number; south: number; east: number; north: number };
  levels: number[];
  coverageWidth: number;
  coverageHeight: number;
}

/** A measured volume as a worker hands it over. */
export interface VolumeMesh {
  ground: RadarMesh | null;
  levels: { km: number; slices: RadarMesh; walls: RadarMesh | null }[];
  coverage: Coverage;
}

/**
 * Triangulates a measured volume from the backend's blobs: `ground`, the
 * coverage mask and one per height in `frame.levels`, in that order.
 */
export function processVolume(frame: VolumeFrame, ground: ArrayBuffer, coverage: ArrayBuffer, levels: ArrayBuffer[]): VolumeMesh {
  const nonEmpty = (mesh: RadarMesh) => (mesh.indices.length ? mesh : null);
  const meshes: VolumeMesh["levels"] = [];
  frame.levels.forEach((km, i) => {
    const polygons = parseVolumePolygons(levels[i]);
    const slices = buildMesh(polygons);
    if (!slices.indices.length) return;
    meshes.push({ km, slices, walls: nonEmpty(buildWalls(polygonWalls(polygons))) });
  });
  const { west, south, east, north } = frame.bounds;
  const mask = new Uint8Array(coverage);
  return {
    ground: nonEmpty(buildMesh(parseVolumePolygons(ground))),
    levels: meshes,
    coverage: {
      key: `${frame.source}:${west},${south},${east},${north}:${frame.coverageWidth}x${frame.coverageHeight}:${hash(mask)}`,
      bounds: [west, south, east, north],
      width: frame.coverageWidth,
      height: frame.coverageHeight,
      mask,
    },
  };
}

/** FNV-1a, enough to tell one coverage mask from the next. */
function hash(bytes: Uint8Array): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < bytes.length; i++) h = Math.imul(h ^ bytes[i], 0x01000193);
  return (h >>> 0).toString(36);
}

/** The typed arrays' buffers, once each, for `postMessage`'s transfer list. */
export function transferables(value: TileMesh | VolumeMesh | null): ArrayBuffer[] {
  const buffers = new Set<ArrayBuffer>();
  const add = (mesh: RadarMesh | null | undefined) => {
    if (!mesh) return;
    for (const a of [mesh.positions, mesh.colors, mesh.indices, mesh.bandStarts, mesh.bandVertexStarts]) {
      buffers.add(a.buffer as ArrayBuffer);
    }
  };
  if (value && "slices" in value) {
    add(value.slices);
    add(value.walls);
  } else if (value) {
    add(value.ground);
    for (const level of value.levels) {
      add(level.slices);
      add(level.walls);
    }
    buffers.add(value.coverage.mask.buffer as ArrayBuffer);
  }
  return [...buffers];
}
