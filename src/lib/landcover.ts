// What covers the land, 10 m by 10 m, for the realistic map where the map's
// own data says nothing (and which trees a wood has, which it seldom says):
//
// - In Sweden, Naturvårdsverket's Nationella Marktäckedata (NMD2023, CC0):
//   pine, spruce and broadleaved forest apart, wetland, heath, fields. Its
//   WMS draws each class in its own exact colour, read back here to the class.
// - Elsewhere (and past Sweden's borders and waters, where NMD is clear),
//   ESA WorldCover 2021 (CC BY 4.0) through Microsoft's Planetary Computer,
//   whose tiles carry the class codes themselves.
//
// Each is read as a picture of a map tile, a class per pixel, kept a while.

/** The classes, the map's own: what grows where, as the trees and the ground want it. */
export const LC = {
  NONE: 0,
  CONIFERS: 1,
  MIXED: 2,
  BROADLEAVES: 3,
  /** Trees, kind not known (WorldCover). */
  FOREST: 4,
  /** Felled, growing back. */
  CLEARCUT: 5,
  WETLAND: 6,
  ARABLE: 7,
  GRASS: 8,
  SHRUB: 9,
  HEATH: 10,
  BARE: 11,
  BUILT: 12,
  WATER: 13,
  SNOW: 14,
} as const;
export type LandClass = (typeof LC)[keyof typeof LC];

/** Whether a class is woodland. */
export const wooded = (c: number) => c >= LC.CONIFERS && c <= LC.FOREST;

/** A tile's classes, `size` × `size` from its north-west corner. */
export type LandGrid = { size: number; classes: Uint8Array };

/** The class at a tile's `x, y` (0 to 1, y down). */
export const classAt = (g: LandGrid, x: number, y: number): number =>
  g.classes[Math.min(g.size - 1, Math.max(0, Math.floor(y * g.size))) * g.size + Math.min(g.size - 1, Math.max(0, Math.floor(x * g.size)))];

const rgb = (hex: string) => Number.parseInt(hex.slice(1), 16);

/** NMD2023 (v2) colours, as its legend gives them, to the map's classes. */
const NMD: Record<number, number> = {};
const nmd = (cls: number, ...colours: string[]) => colours.forEach((c) => (NMD[rgb(c)] = cls));
// forest on firm ground and on wetland: pine, spruce, mixed conifers; conifers with broadleaves; broadleaves; felled
nmd(LC.CONIFERS, "#6E8C05", "#2D5F00", "#4E7000", "#598C55", "#305E50", "#23735A");
nmd(LC.MIXED, "#38A800", "#438870");
nmd(LC.BROADLEAVES, "#4CE600", "#AAFF00", "#97E600", "#89CD9B", "#A5F578", "#ABCD78", "#00E6A9", "#55FF00");
nmd(LC.CLEARCUT, "#CDCD66", "#898944");
// open wetland, of every kind
nmd(LC.WETLAND, "#C29ED7", "#894465", "#CD6699", "#F57AB6", "#D69DBC", "#73004C", "#A80084", "#E600A9", "#FF00C5");
nmd(LC.WETLAND, "#704489", "#AA66CD", "#CA7AF5", "#4C0073", "#8400A8", "#A900E6", "#C500FF");
nmd(LC.ARABLE, "#FFFFBE");
nmd(LC.BARE, "#E1E1E1", "#800080");
nmd(LC.SHRUB, "#CCD79E", "#ABC9A6", "#9EB591");
nmd(LC.HEATH, "#D7C29E", "#CDAA66", "#897044");
nmd(LC.GRASS, "#FFEBAF", "#FFD37F", "#FFBF7F");
nmd(LC.BUILT, "#5A1414", "#E5464B", "#191919");
nmd(LC.WATER, "#6699CD", "#8ACCFA");

/** WorldCover's codes to the map's classes. */
const WORLDCOVER: Record<number, number> = {
  10: LC.FOREST,
  20: LC.SHRUB,
  30: LC.GRASS,
  40: LC.ARABLE,
  50: LC.BUILT,
  60: LC.BARE,
  70: LC.SNOW,
  80: LC.WATER,
  90: LC.WETLAND,
  95: LC.BROADLEAVES,
  100: LC.HEATH,
};

/** An NMD picture's pixels (RGBA) to classes: none where it is clear or not one of its colours. */
export function nmdClasses(rgba: Uint8ClampedArray | Uint8Array): Uint8Array {
  const out = new Uint8Array(rgba.length / 4);
  for (let i = 0; i < out.length; i++) {
    if (rgba[i * 4 + 3] < 128) continue;
    out[i] = NMD[(rgba[i * 4] << 16) | (rgba[i * 4 + 1] << 8) | rgba[i * 4 + 2]] ?? LC.NONE;
  }
  return out;
}

/** A WorldCover picture's pixels (its grey the code) to classes. */
export function worldCoverClasses(rgba: Uint8ClampedArray | Uint8Array): Uint8Array {
  const out = new Uint8Array(rgba.length / 4);
  for (let i = 0; i < out.length; i++) {
    if (rgba[i * 4 + 3] < 128) continue;
    out[i] = WORLDCOVER[rgba[i * 4]] ?? LC.NONE;
  }
  return out;
}

export const LANDCOVER_ATTRIBUTION = "Land cover: NMD2023 Naturvårdsverket · ESA WorldCover 2021";

const SIZE = 256;
const HALF_WORLD = 20_037_508.342789244;

/** A tile's corners in web mercator metres: west, south, east, north. */
function bbox(t: { x: number; y: number; z: number }): [number, number, number, number] {
  const span = (2 * HALF_WORLD) / 2 ** t.z;
  const west = -HALF_WORLD + t.x * span;
  const north = HALF_WORLD - t.y * span;
  return [west, north - span, west + span, north];
}

function inSweden(t: { x: number; y: number; z: number }) {
  const n = 2 ** t.z;
  const lon = ((t.x + 0.5) / n) * 360 - 180;
  const lat = (Math.atan(Math.sinh(Math.PI * (1 - (2 * (t.y + 0.5)) / n))) * 180) / Math.PI;
  return lon > 10.5 && lon < 24.5 && lat > 55 && lat < 69.2;
}

/** A picture's pixels, exactly as stored (no colour management, no premultiplying). */
async function pixels(url: string, init?: RequestInit): Promise<Uint8ClampedArray | null> {
  const response = await fetch(url, init);
  if (!response.ok) return null;
  const blob = await response.blob();
  const bitmap = await createImageBitmap(blob, { colorSpaceConversion: "none", premultiplyAlpha: "none" });
  const canvas = typeof OffscreenCanvas !== "undefined" ? new OffscreenCanvas(SIZE, SIZE) : Object.assign(document.createElement("canvas"), { width: SIZE, height: SIZE });
  const context = canvas.getContext("2d", { willReadFrequently: true }) as CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D | null;
  if (!context) return null;
  context.imageSmoothingEnabled = false;
  context.drawImage(bitmap, 0, 0, SIZE, SIZE);
  bitmap.close();
  return context.getImageData(0, 0, SIZE, SIZE).data;
}

const NMD_WMS = "https://geodata.naturvardsverket.se/geoserver/wms";
const nmdUrl = (t: { x: number; y: number; z: number }) =>
  `${NMD_WMS}?service=WMS&version=1.3.0&request=GetMap&layers=lc-nmd:LC.LandCoverRaster.Bas.2023.v2.x&styles=&crs=EPSG:3857` +
  `&bbox=${bbox(t).map((v) => v.toFixed(3)).join(",")}&width=${SIZE}&height=${SIZE}&format=image/png&transparent=true`;

const PC = "https://planetarycomputer.microsoft.com/api/data/v1";
let worldCoverSearch: Promise<string | null> | null = null;
/** The WorldCover 2021 mosaic's id, registered once (it is a hash of the query, the same each time). */
function worldCoverMosaic(): Promise<string | null> {
  worldCoverSearch ??= fetch(`${PC}/mosaic/register`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      collections: ["esa-worldcover"],
      "filter-lang": "cql2-json",
      filter: { op: "anyinteracts", args: [{ property: "datetime" }, { interval: ["2021-01-01T23:59:59Z", "2021-12-31T23:59:59Z"] }] },
    }),
  })
    .then((r) => (r.ok ? r.json() : null))
    .then((j: { searchid?: string } | null) => j?.searchid ?? null)
    .catch(() => {
      // asked again later
      worldCoverSearch = null;
      return null;
    });
  return worldCoverSearch;
}

async function worldCover(t: { x: number; y: number; z: number }): Promise<Uint8Array | null> {
  // (its mosaic is drawn from zoom 6)
  if (t.z < 6) return null;
  const id = await worldCoverMosaic();
  if (!id) return null;
  const px = await pixels(`${PC}/mosaic/${id}/tiles/WebMercatorQuad/${t.z}/${t.x}/${t.y}@1x?assets=map&collection=esa-worldcover&format=png`);
  return px && worldCoverClasses(px);
}

const cache = new Map<string, Promise<LandGrid | null>>();
const MAX_CACHED = 96;

/**
 * A map tile's land cover (a tile of zoom 10 or more; read at 256 pixels
 * across), or null where there is none to be had (or it can't be reached).
 */
export function landCover(t: { x: number; y: number; z: number }): Promise<LandGrid | null> {
  const key = `${t.z}/${t.x}/${t.y}`;
  let hit = cache.get(key);
  if (hit) {
    // the most recently used last
    cache.delete(key);
    cache.set(key, hit);
    return hit;
  }
  hit = (async (): Promise<LandGrid | null> => {
    let classes: Uint8Array | null = null;
    if (inSweden(t)) classes = await pixels(nmdUrl(t)).then((px) => px && nmdClasses(px)).catch(() => null);
    // past NMD's edge, WorldCover's
    if (!classes || classes.some((c) => c === LC.NONE)) {
      const world = await worldCover(t).catch(() => null);
      if (world) {
        if (!classes) classes = world;
        else for (let i = 0; i < classes.length; i++) if (!classes[i]) classes[i] = world[i];
      }
    }
    return classes ? { size: SIZE, classes } : null;
  })();
  cache.set(key, hit);
  // a failure is asked again once forgotten
  hit.then((g) => g ?? cache.delete(key));
  while (cache.size > MAX_CACHED) cache.delete(cache.keys().next().value!);
  return hit;
}
