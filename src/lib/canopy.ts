// How tall the trees are, metre by metre, the world over: Meta and the World
// Resources Institute's canopy height map (v2, CC BY 4.0), as Taylor
// Geospatial Engine Labs mirror it on Source Cooperative, where a browser may
// read it. Each file is a zoom-10 web mercator tile (named by its quadkey),
// 32768 pixels across (1.2 m each, the height in whole metres, 0 where there
// are no trees or no data), with smaller copies of itself; the parts wanted
// are read out of it with HTTP range requests (geotiff.js).

import { fromUrl, type GeoTIFF, type GeoTIFFImage } from "geotiff";

const CHM = "https://data.source.coop/tge-labs/meta-chm-v2/chm/";
export const CANOPY_ATTRIBUTION = "Canopy height: Meta & WRI (CC BY 4.0), via TGE Labs";
const FILE_ZOOM = 10;
/** Pixels across a tile read, at least (the nearest of the file's sizes at or over it). */
const WANT = 512;

/** A tile's tree heights, metres, `size` × `size` from its north-west corner. */
export type CanopyGrid = { size: number; heights: Uint8Array };

/** The height at a tile's `x, y` (0 to 1, y down). */
export const heightAt = (g: CanopyGrid, x: number, y: number): number =>
  g.heights[Math.min(g.size - 1, Math.max(0, Math.floor(y * g.size))) * g.size + Math.min(g.size - 1, Math.max(0, Math.floor(x * g.size)))];

/** The tallest within `r` pixels of a tile's `x, y`. */
export function tallestNear(g: CanopyGrid, x: number, y: number, r: number): number {
  const [cx, cy] = [Math.floor(x * g.size), Math.floor(y * g.size)];
  let best = 0;
  for (let j = Math.max(0, cy - r); j <= Math.min(g.size - 1, cy + r); j++) {
    for (let i = Math.max(0, cx - r); i <= Math.min(g.size - 1, cx + r); i++) best = Math.max(best, g.heights[j * g.size + i]);
  }
  return best;
}

/** A tile's quadkey (Bing's: a digit a zoom, x's bit and twice y's). */
export function quadkey(x: number, y: number, z: number): string {
  let key = "";
  for (let i = z; i > 0; i--) {
    const bit = 1 << (i - 1);
    key += String((x & bit ? 1 : 0) + (y & bit ? 2 : 0));
  }
  return key;
}

/** Which zoom-10 file a tile is in, and where in it: its corner and size, as fractions of the file. */
export function placeInFile(t: { x: number; y: number; z: number }): { key: string; x: number; y: number; span: number } {
  const k = t.z - FILE_ZOOM;
  const [fx, fy] = [t.x >> k, t.y >> k];
  const span = 1 / 2 ** k;
  return { key: quadkey(fx, fy, FILE_ZOOM), x: (t.x - fx * 2 ** k) * span, y: (t.y - fy * 2 ** k) * span, span };
}

/** A file's images (itself and its smaller copies, not their masks), largest first; null where there is no file. */
const files = new Map<string, Promise<GeoTIFFImage[] | null>>();
function imagesOf(key: string): Promise<GeoTIFFImage[] | null> {
  let hit = files.get(key);
  if (!hit) {
    hit = fromUrl(`${CHM}${key}.tif`, { allowFullFile: false })
      .then(async (tiff: GeoTIFF) => {
        const images: GeoTIFFImage[] = [];
        for (let i = 0; i < (await tiff.getImageCount()); i++) {
          const image = await tiff.getImage(i);
          // (a mask's subfile type has bit 4 set)
          if (!((Number(image.fileDirectory.NewSubfileType) || 0) & 4)) images.push(image);
        }
        return images.sort((a, b) => b.getWidth() - a.getWidth());
      })
      .catch(() => null);
    files.set(key, hit);
    while (files.size > 24) files.delete(files.keys().next().value!);
  }
  return hit;
}

const cache = new Map<string, Promise<CanopyGrid | null>>();
const MAX_CACHED = 96;

/** A map tile's tree heights (a tile of zoom 10 or more), or null where there are none to be had. */
export function canopy(t: { x: number; y: number; z: number }): Promise<CanopyGrid | null> {
  if (t.z < FILE_ZOOM) return Promise.resolve(null);
  const id = `${t.z}/${t.x}/${t.y}`;
  let hit = cache.get(id);
  if (hit) {
    cache.delete(id);
    cache.set(id, hit);
    return hit;
  }
  hit = (async (): Promise<CanopyGrid | null> => {
    const place = placeInFile(t);
    const images = await imagesOf(place.key);
    if (!images?.length) return null;
    // the smallest copy with at least WANT pixels across the tile (else the largest there is)
    const image = [...images].reverse().find((im) => im.getWidth() * place.span >= WANT) ?? images[0];
    const w = image.getWidth();
    const size = Math.max(1, Math.round(w * place.span));
    const [x0, y0] = [Math.round(place.x * w), Math.round(place.y * w)];
    const raster = (await image.readRasters({ window: [x0, y0, x0 + size, y0 + size], samples: [0], interleave: true })) as unknown as ArrayLike<number>;
    const heights = raster instanceof Uint8Array ? raster : Uint8Array.from(raster);
    return { size, heights };
  })().catch(() => null);
  cache.set(id, hit);
  while (cache.size > MAX_CACHED) cache.delete(cache.keys().next().value!);
  return hit;
}
