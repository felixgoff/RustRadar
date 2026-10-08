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

import {
  BAND_COUNT,
  bandDbz,
  blur,
  buildMesh,
  buildWalls,
  contourBands,
  parseVolumePolygons,
  polygonData,
  polygonWalls,
  type PolygonData,
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

export interface RadarTile {
  /** `z/x/y`: stable between frames, for layer ids. */
  id: string;
  /** West, south, east, north in degrees. */
  bounds: [number, number, number, number];
  /** Every band, flat on the ground; null when the tile has no echo. */
  ground: PolygonData | null;
  /** Lifted layers, each with only the bands whose echo tops reach it. */
  levels: RadarLevel[];
}

/** Where MRMS has data: RainViewer is hidden there so the two don't overlap. */
export type Covered = (lon: number, lat: number) => boolean;

const tileLatitude = (y: number, z: number) => (Math.atan(Math.sinh(Math.PI * (1 - (2 * y) / 2 ** z))) * 180) / Math.PI;

export async function loadRadarTile(
  url: string,
  index: { x: number; y: number; z: number },
  signal?: AbortSignal,
  covered?: Covered,
): Promise<RadarTile | null> {
  let image: ImageBitmap;
  try {
    const response = await fetch(url, { signal });
    if (!response.ok) return null;
    image = await createImageBitmap(await response.blob());
  } catch {
    return null; // cancelled as the view moved on, or offline: the tile just stays empty
  }
  const { width, height } = image;
  const canvas = new OffscreenCanvas(width, height);
  const context = canvas.getContext("2d", { willReadFrequently: true })!;
  context.drawImage(image, 0, 0);
  image.close();
  const source = context.getImageData(0, 0, width, height).data;

  const north = tileLatitude(index.y, index.z);
  const south = tileLatitude(index.y + 1, index.z);
  const west = (index.x / 2 ** index.z) * 360 - 180;
  const east = ((index.x + 1) / 2 ** index.z) * 360 - 180;

  // the colours back to reflectivity, with no echo as 0 dBZ (below the first
  // band) so the blur fades edges out rather than pulling them in hard
  const dbz = new Float32Array(width * height);
  for (let r = 0; r < height; r++) {
    // rows are Web Mercator: even steps in y, not in latitude
    const lat = covered ? tileLatitude(index.y + (r + 0.5) / height, index.z) : 0;
    for (let c = 0; c < width; c++) {
      const i = (r * width + c) * 4;
      if (!source[i + 3]) continue;
      // blank out what MRMS and DWD cover; they draw it themselves
      if (covered?.(west + ((c + 0.5) / width) * (east - west), lat)) continue;
      dbz[r * width + c] = Math.max(0, reflectivity(source[i], source[i + 1], source[i + 2], source[i + 3]));
    }
  }

  // contoured in the tile's own (Mercator) grid, then placed on the globe
  const lines: WallLine[] = [];
  const polygons = contourBands(blur(dbz, width, height), width, height, lines);
  for (const { positions: p } of [...polygons, ...lines]) {
    for (let i = 0; i < p.length; i += 2) {
      p[i] = west + p[i] * (east - west);
      p[i + 1] = tileLatitude(index.y + p[i + 1], index.z);
    }
  }
  const mesh = buildMesh(polygons);
  const walls = buildWalls(lines);
  const levels: RadarLevel[] = [];
  for (const km of RADAR_LEVELS_KM) {
    const data = polygonData(mesh, lowestBandAt(km));
    if (!data) break; // higher ones are emptier still
    levels.push({ km, data, walls: polygonData(walls, lowestBandAt(km)) });
  }
  return { id: `${index.z}/${index.x}/${index.y}`, bounds: [west, south, east, north], ground: polygonData(mesh), levels };
}

export interface MeasuredVolume {
  source: string;
  time: number;
  bounds: [number, number, number, number];
  ground: PolygonData | null;
  levels: RadarLevel[];
  covered: Covered;
}

/** Loads one measured volume from the backend, triangulated and ready for the GPU. */
export async function loadVolume(
  frame: import("./api").RadarVolume,
  image: (source: string, name: string) => Promise<ArrayBuffer>,
): Promise<MeasuredVolume> {
  const [ground, coverage, ...levels] = await Promise.all([
    image(frame.source, "ground").then((b) => polygonData(buildMesh(parseVolumePolygons(b)))),
    image(frame.source, "coverage").then((b) => new Uint8Array(b)),
    ...frame.levels.map(async (km) => {
      const polygons = parseVolumePolygons(await image(frame.source, String(km)));
      return { km, data: polygonData(buildMesh(polygons)), walls: polygonData(buildWalls(polygonWalls(polygons))) };
    }),
  ]);
  const { west, south, east, north } = frame.bounds;
  const { coverageWidth: w, coverageHeight: h } = frame;
  const covered: Covered = (lon, lat) => {
    if (lon < west || lon > east || lat < south || lat > north) return false;
    // clamped, so a point exactly on the east or south edge still lands in the
    // last cell rather than falling through and leaving a seam
    const x = Math.min(w - 1, Math.floor(((lon - west) / (east - west)) * w));
    const y = Math.min(h - 1, Math.floor(((north - lat) / (north - south)) * h));
    return coverage[y * w + x] === 1;
  };
  return {
    source: frame.source,
    time: frame.time,
    bounds: [west, south, east, north],
    ground,
    levels: levels.filter((l): l is RadarLevel => l.data !== null),
    covered,
  };
}
