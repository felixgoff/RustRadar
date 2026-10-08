// Weather radar in 3D. Where a weather service measures reflectivity aloft —
// NOAA MRMS over North America, DWD over central Europe — the layers are the
// real thing, loaded by the backend (`src-tauri/src/radar.rs`). Everywhere
// else, RainViewer tiles are split into stacked layers by estimated echo-top
// height, so strong cells still rise higher than drizzle.
//
// The public API only serves the "Universal Blue" palette (the raw-dBZ scheme
// returns the same colours), so reflectivity is read back from the colour:
// translucent beige for drizzle, light to dark blue for light to moderate
// rain, yellow to red for heavy rain, pink and white for hail.

/** Heights (km) of the stacked layers above the ground layer. */
export const RADAR_LEVELS_KM = [1.5, 3, 4.5, 6, 8, 10, 12];
/** Resolution of the lifted layers; they are soft anyway. */
const LEVEL_SIZE = 128;

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

export interface RadarLevel {
  km: number;
  image: ImageBitmap;
}

export interface RadarTile {
  /** `z/x/y`: stable between frames, for layer ids. */
  id: string;
  /** West, south, east, north in degrees. */
  bounds: [number, number, number, number];
  /** The tile as served, Web Mercator. */
  image: ImageBitmap;
  /** Lifted layers, resampled to plain latitude/longitude so they can carry a height. */
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
  const pixels = context.getImageData(0, 0, width, height);
  const source = pixels.data;

  const north = tileLatitude(index.y, index.z);
  const south = tileLatitude(index.y + 1, index.z);
  const west = (index.x / 2 ** index.z) * 360 - 180;
  const east = ((index.x + 1) / 2 ** index.z) * 360 - 180;

  // blank out what MRMS covers (it applies to the lifted layers too, which read `source`)
  if (covered) {
    let masked = false;
    for (let r = 0; r < height; r++) {
      const lat = tileLatitude(index.y + (r + 0.5) / height, index.z);
      for (let c = 0; c < width; c++) {
        const i = (r * width + c) * 4;
        if (source[i + 3] && covered(west + ((c + 0.5) / width) * (east - west), lat)) {
          source[i + 3] = 0;
          masked = true;
        }
      }
    }
    if (masked) {
      context.putImageData(pixels, 0, 0);
      image = await createImageBitmap(canvas);
    }
  }

  // for each output row, the Mercator source row at the same latitude
  const mercY = (lat: number) => Math.log(Math.tan(Math.PI / 4 + (lat * Math.PI) / 360));
  const [top, bottom] = [mercY(north), mercY(south)];
  const rows = Array.from({ length: LEVEL_SIZE }, (_, r) => {
    const lat = north - ((r + 0.5) / LEVEL_SIZE) * (north - south);
    return Math.min(height - 1, Math.floor(((top - mercY(lat)) / (top - bottom)) * height));
  });

  const tops = new Float32Array(LEVEL_SIZE * LEVEL_SIZE);
  let highest = 0;
  for (let r = 0; r < LEVEL_SIZE; r++) {
    for (let c = 0; c < LEVEL_SIZE; c++) {
      const i = (rows[r] * width + Math.floor(((c + 0.5) / LEVEL_SIZE) * width)) * 4;
      const km = echoTopKm(reflectivity(source[i], source[i + 1], source[i + 2], source[i + 3]));
      tops[r * LEVEL_SIZE + c] = km;
      highest = Math.max(highest, km);
    }
  }

  const levels: RadarLevel[] = [];
  for (const km of RADAR_LEVELS_KM) {
    if (km > highest) break;
    const out = new ImageData(LEVEL_SIZE, LEVEL_SIZE);
    for (let r = 0; r < LEVEL_SIZE; r++) {
      for (let c = 0; c < LEVEL_SIZE; c++) {
        const o = r * LEVEL_SIZE + c;
        if (tops[o] < km) continue;
        const i = (rows[r] * width + Math.floor(((c + 0.5) / LEVEL_SIZE) * width)) * 4;
        out.data.set([source[i], source[i + 1], source[i + 2], Math.max(source[i + 3], 160)], o * 4);
      }
    }
    levels.push({ km, image: await createImageBitmap(out) });
  }
  return { id: `${index.z}/${index.x}/${index.y}`, bounds: [west, south, east, north], image, levels };
}

export interface MeasuredVolume {
  source: string;
  time: number;
  bounds: [number, number, number, number];
  ground: ImageBitmap | null;
  levels: RadarLevel[];
  covered: Covered;
}

/** Loads one measured volume from the backend as GPU-ready images. */
export async function loadVolume(
  frame: import("./api").RadarVolume,
  image: (source: string, name: string) => Promise<ArrayBuffer>,
): Promise<MeasuredVolume> {
  const bitmap = async (name: string) => {
    const bytes = await image(frame.source, name);
    return bytes.byteLength ? createImageBitmap(new Blob([bytes], { type: "image/png" })) : null;
  };
  const [ground, coverage, ...levels] = await Promise.all([
    bitmap("ground"),
    image(frame.source, "coverage").then((b) => new Uint8Array(b)),
    ...frame.levels.map(async (km) => ({ km, image: await bitmap(String(km)) })),
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
    levels: levels.filter((l): l is RadarLevel => l.image !== null),
    covered,
  };
}
