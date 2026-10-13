// Rail map: the railway network over the dark map.
//
// The default is drawn from the OpenFreeMap vector tiles the map already uses
// (OpenMapTiles schema, `transportation` layer): class `rail` (subclass rail,
// narrow_gauge, preserved, funicular) and `transit` (light_rail, subway, tram,
// monorail), with `service` (yard, siding, spur, crossover) and `brunnel`
// (bridge, tunnel). The tiles carry rail from zoom 8. Lines are styled in the
// map's own muted palette: mainlines a light steel grey, sidings and yards
// dimmer and thinner, urban transit a muted violet, tunnels dashed and faint.
// Stations (`poi` class railway) can be marked from zoom 12.
//
// Optionally, close in, the OpenRailwayMap "standard" raster goes on top for
// detail (track usage colour-coded: high speed, main, branch...). Its terms
// ask for a unique User-Agent (or, for websites, a valid Referer), light use
// only, a stop on HTTP 429 and ~5 s timeouts; a desktop webview can't set its
// User-Agent, so it is off by default and takes a `fetchImage` that can route
// the requests through the app's backend.

import { COORDINATE_SYSTEM, type Layer } from "@deck.gl/core";
import { PathStyleExtension } from "@deck.gl/extensions";
import { MVTLayer, TileLayer } from "@deck.gl/geo-layers";
import { BitmapLayer } from "@deck.gl/layers";
import { railOverview, railOverviewLayer } from "./rail-overview";

type RGBA = [number, number, number, number];
type Feature = { properties: Record<string, unknown> };

const NONE: RGBA = [0, 0, 0, 0];
// Coral, the colour railway maps reach for: it stands apart from the grey
// roads and borders, the amber runways and aircraft, and the blue water.
/** Mainline track. */
export const RAIL_MAIN: RGBA = [240, 112, 96, 245];
/** Narrow gauge, heritage lines and funiculars. */
export const RAIL_MINOR: RGBA = [214, 128, 112, 225];
/** Yards, sidings, spurs and crossovers. */
export const RAIL_SERVICE: RGBA = [168, 102, 94, 200];
/** Light rail, trams, subways and monorails: violet, apart from the railways. */
export const RAIL_TRANSIT: RGBA = [178, 146, 240, 225];
/** Tunnels are drawn at this fraction of their colour's alpha (and dashed). */
const TUNNEL_ALPHA = 0.55;

/** The tiles carry rail lines from this zoom. */
export const RAIL_MIN_ZOOM = 8;
/** Stations are marked from this zoom. */
export const STATION_MIN_ZOOM = 12;
/** OpenRailwayMap's raster, when enabled, from this zoom. */
export const OPENRAILWAYMAP_MIN_ZOOM = 10;

export type RailKind = "main" | "minor" | "service" | "transit" | null;

/** What kind of rail a `transportation` feature is, or null for anything else (roads, paths, ferries). */
export function railKind(p: Record<string, unknown>): RailKind {
  if (p.layerName !== undefined && p.layerName !== "transportation") return null;
  if (p.class !== "rail" && p.class !== "transit") return null;
  if (p.service) return "service";
  if (p.class === "transit") return "transit";
  // a rail line without a subclass is taken for a mainline
  return p.subclass === undefined || p.subclass === "rail" ? "main" : "minor";
}

const KIND_COLOR: Record<Exclude<RailKind, null>, RGBA> = {
  main: RAIL_MAIN,
  minor: RAIL_MINOR,
  service: RAIL_SERVICE,
  transit: RAIL_TRANSIT,
};
const KIND_WIDTH: Record<Exclude<RailKind, null>, number> = { main: 1.9, minor: 1.3, service: 0.8, transit: 1.2 };
const tunnelColors = new Map<RGBA, RGBA>();
const tunnelOf = (c: RGBA): RGBA => {
  let t = tunnelColors.get(c);
  if (!t) tunnelColors.set(c, (t = [c[0], c[1], c[2], Math.round(c[3] * TUNNEL_ALPHA)]));
  return t;
};

/** Line colour of a feature: its rail kind's, fainter in tunnels; transparent for anything but rail. */
export function railColor({ properties: p }: Feature): RGBA {
  const kind = railKind(p);
  if (!kind) return NONE;
  return p.brunnel === "tunnel" ? tunnelOf(KIND_COLOR[kind]) : KIND_COLOR[kind];
}

/** Line width (pixels, before the zoom scale) of a feature; 0 for anything but rail. */
export function railWidth({ properties: p }: Feature): number {
  const kind = railKind(p);
  return kind ? KIND_WIDTH[kind] : 0;
}

const SOLID: [number, number] = [0, 0];
const TUNNEL_DASH: [number, number] = [3, 2.5];
/** Dash pattern (in line widths): tunnels dashed, the rest solid. */
export const railDash = ({ properties: p }: Feature): [number, number] => (p.brunnel === "tunnel" ? TUNNEL_DASH : SOLID);

/** Lines thicken a little as the view closes in: x0.8 at zoom 8 to x1.8 at 15. */
export function railWidthScale(zoom: number): number {
  const t = Math.min(1, Math.max(0, (zoom - 8) / 7));
  return Math.round((0.8 + t) * 10) / 10;
}

const STATION_SUBCLASSES = new Set(["station", "halt"]);
/** Whether a `poi` feature is a railway station or halt (tram and subway stops are left to the base map). */
export const isStation = (p: Record<string, unknown>) =>
  p.class === "railway" && STATION_SUBCLASSES.has(String(p.subclass)) && (p.layerName === undefined || p.layerName === "poi");

const STATION_FILL: RGBA = [214, 220, 232, 255];
const STATION_RING: RGBA = [8, 12, 20, 230];

// Ground layers: painted in layer order, no depth writes; lines and points
// are camera-facing quads, which GlobeView would otherwise cull.
const SURFACE_NO_CULL = { cullMode: "none", depthWriteEnabled: false } as const;
const dashed = new PathStyleExtension({ dash: true });
const RAIL_EXTENSIONS = [dashed];

/** Load options are compared by identity: one object per worker URL. */
const loadOptionsCache = new Map<string, object>();
function loadOptions(layers: string[], workerUrl: string | undefined) {
  const key = `${layers.join(",")}|${workerUrl ?? ""}`;
  let options = loadOptionsCache.get(key);
  if (!options) {
    options = { mvt: { layers, ...(workerUrl ? { workerUrl } : {}) }, ...(workerUrl ? {} : { worker: false }) };
    loadOptionsCache.set(key, options);
  }
  return options;
}

/** MVTLayer decodes tiles per projection, so each projection gets its own layer and cache (as the map does). */
export const railNetworkLayer = (url: string, flat: boolean, zoom: number, workerUrl?: string) =>
  new MVTLayer<Record<string, unknown>, { getDashArray: unknown; dashJustified: boolean }>({
    id: flat ? "rail-flat" : "rail-globe",
    data: url,
    minZoom: RAIL_MIN_ZOOM,
    maxZoom: 14,
    loadOptions: loadOptions(["transportation"], workerUrl),
    filled: false,
    stroked: false,
    getLineColor: railColor,
    getLineWidth: railWidth,
    lineWidthUnits: "pixels",
    lineWidthScale: railWidthScale(zoom),
    lineCapRounded: true,
    getPointRadius: 0,
    extensions: RAIL_EXTENSIONS,
    getDashArray: railDash,
    dashJustified: true,
    parameters: SURFACE_NO_CULL,
  } as never);

/** Railway stations and halts as small rings. */
export const railStationLayer = (url: string, flat: boolean, workerUrl?: string) =>
  new MVTLayer({
    id: flat ? "rail-stations-flat" : "rail-stations-globe",
    data: url,
    minZoom: STATION_MIN_ZOOM,
    maxZoom: 14,
    loadOptions: loadOptions(["poi"], workerUrl),
    filled: true,
    stroked: true,
    pointType: "circle",
    pointRadiusUnits: "pixels",
    getPointRadius: ({ properties: p }: Feature) => (isStation(p) ? (p.subclass === "station" ? 3.5 : 2.5) : 0),
    getFillColor: ({ properties: p }: Feature) => (isStation(p) ? STATION_FILL : NONE),
    getLineColor: ({ properties: p }: Feature) => (isStation(p) ? STATION_RING : NONE),
    getLineWidth: 1.5,
    lineWidthUnits: "pixels",
    parameters: SURFACE_NO_CULL,
  });

// --- OpenRailwayMap ---------------------------------------------------------

/** OpenRailwayMap's "standard" style (track usage). Tiles are 512 px images for 256 px tiles. */
export const OPENRAILWAYMAP_URL = "https://tiles.openrailwaymap.org/standard/{z}/{x}/{y}.png";
/** The attribution OpenRailwayMap's terms require (with the OpenStreetMap credit the map already gives). */
export const OPENRAILWAYMAP_ATTRIBUTION = "Rail overlay: © OpenStreetMap contributors, style CC-BY-SA 2.0 OpenRailwayMap";
/** Requests time out after this long, as its terms ask (ms). */
const ORM_TIMEOUT_MS = 5_000;
/** After an HTTP 429, or several failures in a row, no requests for this long (ms). */
const ORM_BACKOFF_MS = 15 * 60_000;
const ORM_FAILURES_BEFORE_BACKOFF = 4;

/** Fetches a tile image; resolves to null when there is none. */
export type FetchImage = (url: string, signal: AbortSignal) => Promise<Blob | null>;

const fetchBlob: FetchImage = async (url, signal) => {
  const response = await fetch(url, { signal, referrerPolicy: "strict-origin-when-cross-origin" });
  if (response.status === 429) throw new RateLimited();
  if (!response.ok) return null;
  return response.blob();
};

/** Thrown by a `FetchImage` on HTTP 429. */
export class RateLimited extends Error {
  constructor() {
    super("rate limited");
  }
}

/**
 * Keeps OpenRailwayMap's terms: a request in flight at most `ORM_TIMEOUT_MS`;
 * after a 429 or a run of failures, a long pause with no requests at all.
 */
export class Backoff {
  private until = 0;
  private failures = 0;
  constructor(private now: () => number = Date.now) {}
  get paused(): boolean {
    return this.now() < this.until;
  }
  succeeded() {
    this.failures = 0;
  }
  failed(rateLimited: boolean) {
    this.failures++;
    if (rateLimited || this.failures >= ORM_FAILURES_BEFORE_BACKOFF) {
      this.until = this.now() + ORM_BACKOFF_MS;
      this.failures = 0;
    }
  }
}
const ormBackoff = new Backoff();

function ormTileFetcher(fetchImage: FetchImage) {
  return async ({ url, signal }: { url?: string | null; signal?: AbortSignal }): Promise<ImageBitmap | null> => {
    if (!url || ormBackoff.paused) return null;
    const timeout = AbortSignal.timeout(ORM_TIMEOUT_MS);
    const both = signal ? AbortSignal.any([signal, timeout]) : timeout;
    try {
      const blob = await fetchImage(url, both);
      ormBackoff.succeeded();
      return blob ? await createImageBitmap(blob) : null;
    } catch (error) {
      // the view moving on cancels a request: that's no failure of theirs
      if (!signal?.aborted) ormBackoff.failed(error instanceof RateLimited);
      return null;
    }
  };
}
const ormFetchers = new WeakMap<FetchImage, ReturnType<typeof ormTileFetcher>>();
const ormFetcherFor = (fetchImage: FetchImage) => {
  let f = ormFetchers.get(fetchImage);
  if (!f) ormFetchers.set(fetchImage, (f = ormTileFetcher(fetchImage)));
  return f;
};

const renderOrmTile = (props: any) => {
  if (!props.data) return null;
  const [[west, south], [east, north]] = props.tile.boundingBox;
  return new BitmapLayer(props, {
    data: undefined,
    image: props.data,
    bounds: [west, south, east, north],
    _imageCoordinateSystem: COORDINATE_SYSTEM.CARTESIAN,
    parameters: SURFACE_NO_CULL,
  });
};
const unloadOrmTile = (tile: { content?: ImageBitmap | null }) => tile.content?.close?.();

/** OpenRailwayMap's raster, slightly translucent so it sits in the dark map. One layer for both projections. */
export const openRailwayMapLayer = (fetchImage: FetchImage = fetchBlob, url = OPENRAILWAYMAP_URL, opacity = 0.85) =>
  new TileLayer<ImageBitmap | null>({
    id: "rail-openrailwaymap",
    data: url,
    minZoom: OPENRAILWAYMAP_MIN_ZOOM,
    maxZoom: 18,
    tileSize: 256,
    // light use: few at a time, and keep what's been fetched
    maxRequests: 4,
    maxCacheSize: 200,
    getTileData: ormFetcherFor(fetchImage),
    onTileUnload: unloadOrmTile,
    renderSubLayers: renderOrmTile,
    opacity,
    parameters: SURFACE_NO_CULL,
  });

export type RailOptions = {
  /** OpenFreeMap's vector tile URL template (Globe.svelte's `tileUrl`, from the TileJSON); null while it loads. */
  tileUrl: string | null;
  /** GlobeView's flat Web Mercator projection is in use (zoom above 12). */
  flat: boolean;
  zoom: number;
  /** The bundled MVT worker (`@loaders.gl/mvt/mvt-worker.js?url`); without it tiles are decoded on the main thread. */
  workerUrl?: string;
  /** Mark stations and halts from zoom 12. Default true. */
  stations?: boolean;
  /**
   * Add OpenRailwayMap's raster from zoom 10. Default false: its terms ask for
   * a unique User-Agent, which only a backend fetch (`fetchImage`) can send.
   */
  openRailwayMap?: boolean;
  /** How OpenRailwayMap tiles are fetched; throw `RateLimited` on HTTP 429. Default: the webview's fetch. */
  fetchImage?: FetchImage;
  /** OpenRailwayMap URL template (e.g. a backend proxy or cache); default `OPENRAILWAYMAP_URL`. */
  openRailwayMapUrl?: string;
};

/**
 * The rail layers for the current view, in drawing order. Insert them after
 * the map (`vectorMapLayer` and `aerowayLayer`) and before the buildings,
 * ships and labels: like the map they are ground layers drawn in layer order.
 * With OpenRailwayMap on, the vector network stays underneath it: the raster
 * covers the same lines in its own colours and the vector lines fill in while
 * its tiles load.
 */
export function railLayers({
  tileUrl,
  flat,
  zoom,
  workerUrl,
  stations = true,
  openRailwayMap = false,
  fetchImage,
  openRailwayMapUrl,
}: RailOptions): Layer[] {
  const layers: Layer[] = [];
  // zoomed out, where the tiles have no rail yet, the shipped overview
  if (zoom < RAIL_MIN_ZOOM + 0.5) {
    const overview = railOverview();
    if (overview) layers.push(railOverviewLayer(overview, { main: RAIL_MAIN, minor: RAIL_MINOR }, flat));
  }
  if (tileUrl && zoom >= RAIL_MIN_ZOOM - 0.5) layers.push(railNetworkLayer(tileUrl, flat, zoom, workerUrl));
  if (openRailwayMap && zoom >= OPENRAILWAYMAP_MIN_ZOOM) layers.push(openRailwayMapLayer(fetchImage, openRailwayMapUrl));
  if (tileUrl && stations && zoom >= STATION_MIN_ZOOM) layers.push(railStationLayer(tileUrl, flat, workerUrl));
  return layers;
}
