// Terrain: shaded relief under the map, and a true 3D surface the map stands
// on (the terrain setting, on the flat map).
//
// Elevation comes from the keyless AWS Open Data "Terrain Tiles" (Mapzen /
// Tilezen Joerd), terrarium PNG encoding, z0-15:
//   height (m) = R * 256 + G + B / 256 - 32768
//
// The default is a hillshade raster at every zoom, in both projections: each
// terrarium tile is decoded and shaded on the CPU (about a millisecond for a
// 256 px tile) into a transparent image that darkens slopes facing away from
// the light and faintly lightens those facing it. Flat ground comes out fully
// transparent, so plains, the sea and tile edges leave the map untouched.
//
// The 3D surface (`terrainSurfaceLayer`) is a deck.gl TerrainLayer drawn as
// deck.gl's terrain: the map's other layers stand on it with deck.gl's
// TerrainExtension (Globe.svelte's `onTerrain`), the flat ones draped over
// it, the rest raised by the ground's height under them.

import { COORDINATE_SYSTEM, type Layer, LayerExtension } from "@deck.gl/core";
import { TerrainLayer, TileLayer } from "@deck.gl/geo-layers";
import { BitmapLayer } from "@deck.gl/layers";

/** Terrarium-encoded elevation tiles, keyless (AWS Open Data, Mapzen Terrain Tiles). */
export const TERRARIUM_URL = "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png";
/** The deepest zoom the terrarium tiles exist at; deeper tiles are overzoomed. */
export const TERRARIUM_MAX_ZOOM = 15;
/** deck.gl `elevationDecoder` for terrarium PNGs. */
export const TERRARIUM_DECODER = { rScaler: 256, gScaler: 1, bScaler: 1 / 256, offset: -32768 } as const;

/**
 * The attribution the Terrain Tiles licence asks for (tilezen/joerd
 * docs/attribution.md, "when using Mapzen's hosted service"), shortened to
 * one line for the map's attribution strip. The full list is in
 * `TERRAIN_ATTRIBUTION_FULL`, for an about/credits view.
 */
export const TERRAIN_ATTRIBUTION =
  "Terrain: Mapzen Terrain Tiles (AWS Open Data): USGS 3DEP, SRTM, GMTED2010; NOAA ETOPO1; Copernicus EU-DEM; and others";
export const TERRAIN_ATTRIBUTION_FULL = [
  "Mapzen",
  "ArcticDEM terrain data DEM(s) were created from DigitalGlobe, Inc., imagery and funded under National Science Foundation awards 1043681, 1559691, and 1542736",
  "Australia terrain data © Commonwealth of Australia (Geoscience Australia) 2017",
  "Austria terrain data © offene Daten Österreichs – Digitales Geländemodell (DGM) Österreich",
  "Canada terrain data contains information licensed under the Open Government Licence – Canada",
  "Europe terrain data produced using Copernicus data and information funded by the European Union - EU-DEM layers",
  "Global ETOPO1 terrain data U.S. National Oceanic and Atmospheric Administration",
  "Mexico terrain data source: INEGI, Continental relief, 2016",
  "New Zealand terrain data Copyright 2011 Crown copyright (c) Land Information New Zealand and the New Zealand Government (All rights reserved)",
  "Norway terrain data © Kartverket",
  "United Kingdom terrain data © Environment Agency copyright and/or database right 2015. All rights reserved",
  "United States 3DEP (formerly NED) and global GMTED2010 and SRTM terrain data courtesy of the U.S. Geological Survey",
] as const;

const TILE_SIZE = 256;
/** Web Mercator's equator, metres. */
const EARTH_CIRCUMFERENCE = 2 * Math.PI * 6_378_137;
const DEG = Math.PI / 180;

/** Heights in metres from terrarium RGBA pixels; `out` is reused when given. */
export function decodeTerrarium(rgba: ArrayLike<number>, out?: Float32Array): Float32Array {
  const n = rgba.length >> 2;
  const heights = out && out.length >= n ? out : new Float32Array(n);
  for (let i = 0, j = 0; i < n; i++, j += 4) {
    heights[i] = rgba[j] * 256 + rgba[j + 1] + rgba[j + 2] / 256 - 32768;
  }
  return heights;
}

/** Ground metres per pixel of tile row `row` (of `size`) in Web Mercator tile `z`/`y`. */
export function tileRowMetersPerPixel(z: number, y: number, row: number, size = TILE_SIZE): number {
  const n = 2 ** z;
  const mercY = (y + (row + 0.5) / size) / n; // 0 at the top (north), 1 at the bottom
  const lat = Math.atan(Math.sinh(Math.PI * (1 - 2 * mercY)));
  return (EARTH_CIRCUMFERENCE * Math.cos(lat)) / (size * n);
}

/**
 * Vertical exaggeration for the shading at a tile zoom. Zoomed out, a pixel
 * spans kilometres and averages the slopes away; exaggerating them keeps the
 * relief legible on the globe without shouting once zoomed in.
 */
export function reliefExaggeration(z: number): number {
  return Math.min(8, Math.max(1, 2 ** ((11 - z) / 3)));
}

export type HillshadeStyle = {
  /** Direction the light comes from, degrees clockwise from north (cartographic default: 315, the north-west). */
  azimuth: number;
  /** Height of the light above the horizon, degrees. */
  altitude: number;
  /** Vertical exaggeration. */
  zFactor: number;
  /** Colour of the shadows, and how dark the darkest gets (0-255 alpha). */
  shadow: [number, number, number];
  shadowAlpha: number;
  /** Colour of the lit slopes, and their strongest alpha. */
  highlight: [number, number, number];
  highlightAlpha: number;
  /** Shade difference from flat ground (0-1) that reaches the full alpha. */
  gain: number;
};

/** Tuned for the dark map: deep navy shadows, a faint warm light on the sunny side. */
export const HILLSHADE_STYLE: HillshadeStyle = {
  azimuth: 315,
  altitude: 45,
  zFactor: 1,
  shadow: [4, 8, 16],
  shadowAlpha: 135,
  highlight: [255, 244, 225],
  highlightAlpha: 40,
  gain: 0.55,
};

/**
 * A hillshade of a `width` x `height` grid of heights (metres, row 0 north),
 * as RGBA: shadow colour where a slope faces away from the light, highlight
 * colour where it faces it, alpha proportional to how much it differs from
 * flat ground (flat ground is fully transparent). Below sea level counts as
 * sea level, so the sea floor isn't shaded. `metersPerPixel(row)` is the
 * ground size of a pixel in that row. Horn's 3x3 gradient; at the edges it
 * uses the neighbours there are, so tiles meet without a seam.
 */
/** Scratch for `hillshade`: the heights with the sea floor raised to sea level. */
let seaLevel: Float32Array | null = null;

export function hillshade(
  heights: ArrayLike<number>,
  width: number,
  height: number,
  metersPerPixel: (row: number) => number,
  style: Partial<HillshadeStyle> = {},
  out?: Uint8ClampedArray,
): Uint8ClampedArray {
  const s = { ...HILLSHADE_STYLE, ...style };
  const rgba = out && out.length >= width * height * 4 ? out : new Uint8ClampedArray(width * height * 4);
  const az = s.azimuth * DEG;
  const alt = s.altitude * DEG;
  // towards the light, in east, north, up
  const lx = Math.sin(az) * Math.cos(alt);
  const ly = Math.cos(az) * Math.cos(alt);
  const lz = Math.sin(alt);
  const flat = lz;
  // below sea level counts as sea level
  const n = width * height;
  if (!seaLevel || seaLevel.length < n) seaLevel = new Float32Array(n);
  const z = seaLevel;
  for (let i = 0; i < n; i++) z[i] = Math.max(0, heights[i]);
  const [sr, sg, sb] = s.shadow;
  const [hr, hg, hb] = s.highlight;
  for (let y = 0; y < height; y++) {
    const rn = (y > 0 ? y - 1 : y) * width;
    const rc = y * width;
    const rs = (y < height - 1 ? y + 1 : y) * width;
    const ySpan = (y > 0 ? 1 : 0) + (y < height - 1 ? 1 : 0);
    const mpp = metersPerPixel(y) / s.zFactor;
    const yScale = 1 / (4 * ySpan * mpp);
    for (let x = 0; x < width; x++) {
      const xw = x > 0 ? x - 1 : x;
      const xe = x < width - 1 ? x + 1 : x;
      // Horn: weighted differences across the 3x3 neighbourhood
      const dzdx = (z[rn + xe] + 2 * z[rc + xe] + z[rs + xe] - z[rn + xw] - 2 * z[rc + xw] - z[rs + xw]) / (4 * (xe - xw) * mpp);
      // rows run south: the gradient towards the north is the negative of the row's
      const dzdy = (z[rn + xw] + 2 * z[rn + x] + z[rn + xe] - z[rs + xw] - 2 * z[rs + x] - z[rs + xe]) * yScale;
      // the surface normal (-dz/dx, -dz/dy, 1), normalised, against the light
      const shade = (-dzdx * lx - dzdy * ly + lz) / Math.sqrt(dzdx * dzdx + dzdy * dzdy + 1);
      const d = (shade - flat) / s.gain;
      const i = (y * width + x) * 4;
      if (d < 0) {
        rgba[i] = sr;
        rgba[i + 1] = sg;
        rgba[i + 2] = sb;
        rgba[i + 3] = Math.min(1, -d) * s.shadowAlpha;
      } else {
        rgba[i] = hr;
        rgba[i + 1] = hg;
        rgba[i + 2] = hb;
        rgba[i + 3] = Math.min(1, d) * s.highlightAlpha;
      }
    }
  }
  return rgba;
}

type TileIndex = { x: number; y: number; z: number };
type TileRequest = { index: TileIndex; url?: string | null; signal?: AbortSignal };

/** Reused for every tile: one canvas to read pixels from, one scratch grid of heights. */
let pixelCanvas: OffscreenCanvas | null = null;
let pixelContext: OffscreenCanvasRenderingContext2D | null = null;
let scratchHeights: Float32Array | null = null;

/** Exact pixel values of an image (no colour management or premultiplication, which would corrupt the heights). */
async function pixels(blob: Blob): Promise<ImageData> {
  const bitmap = await createImageBitmap(blob, { colorSpaceConversion: "none", premultiplyAlpha: "none" });
  const { width, height } = bitmap;
  if (!pixelCanvas || pixelCanvas.width !== width || pixelCanvas.height !== height) {
    pixelCanvas = new OffscreenCanvas(width, height);
    pixelContext = pixelCanvas.getContext("2d", { willReadFrequently: true });
  }
  pixelContext!.clearRect(0, 0, width, height);
  pixelContext!.drawImage(bitmap, 0, 0);
  bitmap.close();
  return pixelContext!.getImageData(0, 0, width, height, { colorSpace: "srgb" });
}

/** A terrarium tile shaded into a transparent hillshade image; null where there is no tile. */
async function hillshadeTile({ index, url, signal }: TileRequest): Promise<ImageBitmap | null> {
  if (!url) return null;
  const response = await fetch(url, { signal });
  if (!response.ok) return null;
  const image = await pixels(await response.blob());
  if (signal?.aborted) return null;
  const { width, height } = image;
  const heights = (scratchHeights = decodeTerrarium(image.data, scratchHeights ?? undefined));
  const shaded = hillshade(heights, width, height, (row) => tileRowMetersPerPixel(index.z, index.y, row, height), {
    zFactor: reliefExaggeration(index.z),
  });
  return createImageBitmap(new ImageData(shaded as Uint8ClampedArray<ArrayBuffer>, width, height), {
    premultiplyAlpha: "none",
  });
}

/** Ground layers paint in layer order and don't write depth (Globe.svelte's SURFACE). */
const SURFACE = { depthWriteEnabled: false } as const;

const renderHillshadeTile = (props: any) => {
  if (!props.data) return null;
  const [[west, south], [east, north]] = props.tile.boundingBox;
  return new BitmapLayer(props, {
    data: undefined,
    image: props.data,
    bounds: [west, south, east, north],
    // Web Mercator images, reprojected onto the globe (as the imagery is)
    _imageCoordinateSystem: COORDINATE_SYSTEM.CARTESIAN,
    parameters: SURFACE,
  });
};

const unloadHillshadeTile = (tile: { content?: ImageBitmap | null }) => tile.content?.close?.();

/** Hillshade strength per basemap: the imagery has its own sunlit relief and needs less; the realistic ground's light colours need less to show it. */
const HILLSHADE_OPACITY = { dark: 1, satellite: 0.45, realistic: 0.6 } as const;

export type TerrainBasemap = keyof typeof HILLSHADE_OPACITY;

export type TerrainOptions = {
  /** GlobeView's flat Web Mercator projection is in use (zoom above 12). */
  flat: boolean;
  /** The view's zoom. */
  zoom: number;
  basemap: TerrainBasemap;
  /** Hillshade strength, 0-1, on top of the basemap's own. */
  opacity?: number;
  /** Layer extensions for the hillshade (the realistic map's haze). */
  extensions?: unknown[];
};

/**
 * The hillshade's deepest tiles. Most of the source data is ~30 m (SRTM,
 * EU-DEM), about a z12 pixel at mid latitudes: deeper tiles are resampled and
 * shade into terraces and stripes, while a z12 tile stretched further stays
 * smooth.
 */
export const HILLSHADE_MAX_ZOOM = 12;

/**
 * Hillshade opacity at a view zoom: full out to zoom 11, easing to 0.6 by 14,
 * where a single slope fills the screen and would otherwise darken all of it.
 */
export function hillshadeFade(zoom: number): number {
  return 1 - 0.4 * Math.min(1, Math.max(0, (zoom - 11) / 3));
}

const NO_EXTENSIONS: never[] = [];

/** The shaded relief raster: one layer and tile cache for both projections. */
export const hillshadeLayer = (opacity: number, extensions: unknown[] = NO_EXTENSIONS) =>
  new TileLayer<ImageBitmap | null>({
    id: "terrain-hillshade",
    data: TERRARIUM_URL,
    minZoom: 0,
    maxZoom: HILLSHADE_MAX_ZOOM,
    tileSize: TILE_SIZE,
    maxRequests: 8,
    // keep the shaded tiles: refetching is the expensive part
    maxCacheSize: 300,
    getTileData: hillshadeTile,
    onTileUnload: unloadHillshadeTile,
    renderSubLayers: renderHillshadeTile,
    opacity,
    parameters: SURFACE,
    extensions: extensions as never,
  });

const meshLoadOptions = new Map<string, object>();
function meshLoadOptionsFor(workerUrl: string) {
  const key = workerUrl;
  let options = meshLoadOptions.get(key);
  if (!options) {
    options = { terrain: { workerUrl } };
    meshLoadOptions.set(key, options);
  }
  return options;
}

/** The dark map's land, which the untextured mesh is painted in. */
const MESH_COLOR: [number, number, number] = [34, 39, 48];

/**
 * The 3D terrain no lower than the sea: the tiles have the sea floor, which
 * the sea's surface (the globe, the map's water) would otherwise hide.
 * (Its mesh's vertices are longitude, latitude and metres up, as they pass
 * the size filter.)
 */
class SeaLevelExtension extends LayerExtension {
  static extensionName = "SeaLevelExtension";

  getShaders() {
    return { inject: { "vs:DECKGL_FILTER_SIZE": "size.z = max(size.z, 0.0);" } };
  }
}
const SEA_LEVEL = new SeaLevelExtension();

/**
 * The 3D surface's deepest tiles: about 5 m a pixel, finer than most of the
 * data (~30 m), and each a texture of what is draped on it (deck.gl's, up to
 * 2048 px: well under a metre a pixel). Deeper would only make more tiles to
 * mesh and drape.
 */
const SURFACE_MAX_ZOOM = 14;

/**
 * The ground in 3D: the elevation as a surface, which the map's other layers
 * stand on with deck.gl's TerrainExtension (the flat ones draped over it,
 * the rest raised by the height under them). Drawn with what is draped on
 * it; its own colour only shows where nothing is.
 */
export const terrainSurfaceLayer = (workerUrl: string, extensions: unknown[] = NO_EXTENSIONS) =>
  new TerrainLayer({
    id: "terrain-surface",
    elevationData: TERRARIUM_URL,
    elevationDecoder: TERRARIUM_DECODER,
    // out to the horizon, where coarser tiles are asked for
    minZoom: 0,
    maxZoom: SURFACE_MAX_ZOOM,
    tileSize: TILE_SIZE,
    meshMaxError: 4,
    color: MESH_COLOR,
    material: false,
    operation: "terrain+draw",
    loadOptions: meshLoadOptionsFor(workerUrl),
    maxRequests: 8,
    extensions: [SEA_LEVEL, ...extensions] as never,
  });

/**
 * The terrain layers for the current view, in drawing order: the hillshade.
 *
 * Insert them right after the basemap's ground (`landLayer()` on the dark map,
 * `imageryLayer()` on the satellite one) and before `vectorMapLayer`, so
 * water, borders, roads and runways stay crisp on top of the shading.
 */
export function terrainLayers({ zoom, basemap, opacity = 1, extensions }: TerrainOptions): Layer[] {
  // opacity in steps, so the layer's props don't change with every frame of a zoom
  const fade = Math.round(hillshadeFade(zoom) * 20) / 20;
  return [hillshadeLayer(HILLSHADE_OPACITY[basemap] * opacity * fade, extensions)];
}
