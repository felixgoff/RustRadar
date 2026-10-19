// Tiles chosen by their distance from the camera: near ones fine, far ones
// coarse, in every view (deck.gl does so only on the globe and in flat views
// tilted past 60°; flat at 60° or less it takes the view's zoom everywhere).
//
// Its rule is deck.gl's own (tile-2d-traversal.js: a tile is fine enough when
// its zoom plus log2 of its distance, in view heights, reaches the view's),
// with the view's zoom taken out, so a tile's level follows only how far it
// is from the camera; a bias makes all of them finer, and only the zooms a
// source has are picked (one between goes to the finer). The traversal is
// deck.gl 9.4.0's (tile-2d-traversal.js, which it doesn't export), copied:
// @deck.gl/* are pinned for it.

import { _GlobeViewport as GlobeViewport, type DefaultProps, type Viewport } from "@deck.gl/core";
import { _getURLFromTemplate as urlOf, MVTLayer, type MVTLayerProps, _Tileset2D as Tileset2D } from "@deck.gl/geo-layers";
import { AxisAlignedBoundingBox, CullingVolume, makeOrientedBoundingBoxFromPoints, Plane } from "@math.gl/culling";

const TILE_SIZE = 512;
// where a tile on the globe is sampled for its box (tile-2d-traversal.js)
const REF_5 = [[0.5, 0.5], [0, 0], [0, 1], [1, 0], [1, 1]];
const REF_9 = REF_5.concat([[0, 0.5], [0.5, 0], [1, 0.5], [0.5, 1]]);
const REF_11 = REF_9.concat([[0.25, 0.5], [0.75, 0.5]]);

/** How a tile layer picks its tiles' zooms by distance. */
export type Lod = {
  /** The zooms with tiles, ascending. */
  levels: readonly number[];
  /** Zoom levels finer than deck.gl's rule, everywhere. */
  bias?: number;
  /** Nothing farther than this from the camera, metres. */
  maxDistance?: number;
};

type Index = { x: number; y: number; z: number };

function tileLngLat(x: number, y: number, z: number): [number, number] {
  const scale = 2 ** z;
  const n = Math.PI - (2 * Math.PI * y) / scale;
  return [(x / scale) * 360 - 180, (180 / Math.PI) * Math.atan(0.5 * (Math.exp(n) - Math.exp(-n)))];
}

function boundingVolume(x: number, y: number, z: number, elevation: [number, number], project: ((p: number[]) => number[]) | null) {
  if (project) {
    const points: number[][] = [];
    for (const [px, py] of z < 1 ? REF_11 : z < 2 ? REF_9 : REF_5) {
      const [lng, lat] = tileLngLat(x + px, y + py, z);
      points.push(project([lng, lat, elevation[0]]));
      if (elevation[0] !== elevation[1]) points.push(project([lng, lat, elevation[1]]));
    }
    return makeOrientedBoundingBoxFromPoints(points);
  }
  // web mercator's common space, y up
  const extent = TILE_SIZE / 2 ** z;
  const [ox, oy] = [x * extent, TILE_SIZE - (y + 1) * extent];
  return new AxisAlignedBoundingBox([ox, oy, elevation[0]], [ox + extent, oy + extent, elevation[1]]);
}

/** Whether a tile on the globe is all on the far side of it from the camera. */
function beyondHorizon(x: number, y: number, z: number, camera: number[], project: (p: number[]) => number[], elevation: number) {
  const [cx, cy, cz] = camera;
  const camLng = (Math.atan2(cx, -cy) * 180) / Math.PI;
  const camLat = (Math.asin(cz / Math.hypot(cx, cy, cz)) * 180) / Math.PI;
  const [west, north] = tileLngLat(x, y, z);
  const [east, south] = tileLngLat(x + 1, y + 1, z);
  const mid = (west + east) / 2;
  const lng = Math.max(west, Math.min(mid + (((camLng - mid + 540) % 360) - 180), east));
  const lat = Math.max(south, Math.min(camLat, north));
  const p = project([lng, lat, elevation]);
  return p[0] * cx + p[1] * cy + p[2] * cz <= p[0] * p[0] + p[1] * p[1] + p[2] * p[2];
}

/** The tiles in view, each at the zoom its distance from the camera calls for. */
export function distanceTileIndices(viewport: Viewport, lod: Lod, zRange: number[] | null = null, offset = 0): Index[] {
  const { levels, bias = 0, maxDistance = Infinity } = lod;
  const project = viewport instanceof GlobeViewport ? (p: number[]) => viewport.projectPosition(p) : null;
  const planes = Object.values(viewport.getFrustumPlanes()).map(({ normal, distance }) => new Plane(normal.clone().negate(), distance));
  const culling = new CullingVolume(planes);
  const upm = viewport.distanceScales.unitsPerMeter[2];
  const elevation: [number, number] = [zRange ? zRange[0] * upm : 0, zRange ? zRange[1] * upm : 0];
  const camera = viewport.cameraPosition;
  // deck.gl's distance is in view heights at the view's scale; without the zoom
  const k = viewport.scale / 2 ** viewport.zoom / viewport.height;
  const farthest = maxDistance * upm;
  const [lowest, highest] = [levels[0], levels[levels.length - 1]];
  const allowed = new Set(levels);
  const levelFor = (want: number) => (want >= highest ? highest : want <= lowest ? lowest : levels.find((l) => l >= want)!);
  const out: Index[] = [];
  const visit = (x: number, y: number, z: number) => {
    const volume = boundingVolume(x, y, z, elevation, project);
    if (culling.computeVisibility(volume) < 0) return;
    if (project && beyondHorizon(x, y, z, camera, project, elevation[1])) return;
    const distance = volume.distanceTo(camera);
    if (distance > farthest) return;
    if (allowed.has(z)) {
      const want = offset - Math.floor(Math.log2(Math.max(distance * k, 1e-12)) - bias);
      if (z >= levelFor(want)) {
        out.push({ x, y, z });
        return;
      }
    }
    if (z >= highest) return;
    for (const [dx, dy] of [[0, 0], [0, 1], [1, 0], [1, 1]]) visit(x * 2 + dx, y * 2 + dy, z + 1);
  };
  visit(0, 0, 0);
  return out;
}

type Opts = { lod?: Lod; tileSize: number; zoomOffset: number; minZoom?: number; extent?: unknown };

/** A tileset that picks its tiles by distance (`lod` in its options; without one, deck.gl's own way). */
export class LodTileset extends Tileset2D {
  getTileIndices(args: Parameters<Tileset2D["getTileIndices"]>[0]) {
    const { viewport, zRange } = args;
    const opts = (this as unknown as { opts: Opts }).opts;
    if (!opts.lod || !viewport.isGeospatial) return super.getTileIndices(args);
    // (the layer is hidden under its minZoom: nothing to load then)
    if (Number.isFinite(opts.minZoom) && !opts.extent && viewport.zoom < opts.minZoom!) return [];
    return distanceTileIndices(viewport, opts.lod, (zRange as number[] | null) ?? null, Math.log2(TILE_SIZE / opts.tileSize) + opts.zoomOffset);
  }
}

type RefineTile = {
  state: number;
  isSelected: boolean;
  isVisible: boolean;
  zoom: number;
  content: unknown;
  parent: RefineTile | null;
  children: RefineTile[] | null;
};
const VISITED = 1;
const VISIBLE = 2;
/** A tile with something to draw (one that failed, or isn't there, doesn't count: what is over it is drawn instead). */
const ready = (tile: RefineTile) => tile.content != null;

/**
 * deck.gl's 'no-overlap' refinement (a coarser tile is drawn in place of finer
 * ones until they are all in, never with them: 3D buildings would stand
 * twice), but a tile that came back empty or failed keeps the one over it.
 */
export function keepCoarserUntilIn(allTiles: RefineTile[]) {
  for (const tile of allTiles) tile.state = 0;
  for (const tile of allTiles) {
    if (!tile.isSelected) continue;
    for (let t: RefineTile | null = tile; t; t = t.parent) {
      if (ready(t)) {
        t.state |= VISIBLE;
        break;
      }
    }
  }
  const finer = (tile: RefineTile) => {
    for (const child of tile.children ?? []) {
      if (ready(child)) child.state |= VISIBLE;
      else finer(child);
    }
  };
  for (const tile of [...allTiles].sort((a, b) => a.zoom - b.zoom)) {
    tile.isVisible = Boolean(tile.state & VISIBLE);
    if (tile.children && (tile.isVisible || tile.state & VISITED)) {
      for (const child of tile.children) child.state = VISITED;
    } else if (tile.isSelected) {
      finer(tile);
    }
  }
}

/** Where a zoom's tiles come from, for a layer whose tiles are picked by distance. */
export type TileSource = {
  /** The zooms it has tiles at. */
  zooms: readonly number[];
  /** Its tiles, `{z}/{x}/{y}`. */
  url: string;
  /** The tiles' layers to read. */
  layers: string[];
  /** A fetch of its own. */
  fetch?: (url: string, options: { loadOptions?: object; signal?: AbortSignal }) => Promise<unknown>;
};

export type LodProps = {
  /** Each zoom's source (none: `data`, at the view's zoom, as any MVTLayer). */
  sources: readonly TileSource[];
  /** How the tiles' zooms are picked by their distance from the camera; none: by the view's zoom. */
  lod: Lod | null;
};

const lodDefaults: DefaultProps<LodProps> = {
  sources: { type: "array", value: [], compare: false },
  lod: { type: "object", value: null, compare: false },
};

/**
 * An MVTLayer whose tiles are picked by their distance from the camera
 * (`lod`), each zoom's from its own source (`sources`): a coarser tile stays
 * until the finer ones over it are in, never drawn with them.
 */
export class LodMVTLayer<D = Record<string, unknown>, P extends object = object> extends MVTLayer<D, P & LodProps> {
  static layerName = "LodMVTLayer";
  static defaultProps = { ...lodDefaults, TilesetClass: LodTileset } as never;

  _getTilesetOptions() {
    const options = super._getTilesetOptions();
    const { lod } = this.props as unknown as LodProps;
    return (lod ? { ...options, lod, refinementStrategy: keepCoarserUntilIn } : options) as typeof options;
  }

  getTileData(loadProps: Parameters<MVTLayer["getTileData"]>[0]) {
    const { sources } = this.props as unknown as LodProps;
    if (!sources.length) return super.getTileData(loadProps);
    const source = sources.find((s) => s.zooms.includes(loadProps.index.z));
    const url = source && urlOf(source.url, loadProps);
    if (!source || !url) return Promise.resolve(null) as never;
    // (as MVTLayer's own, with the source's layers)
    const base = (this.getLoadOptions() ?? {}) as { core?: object; mvt?: object };
    const loadOptions = {
      ...base,
      core: { ...base.core, mimeType: "application/x-protobuf" },
      mvt: {
        ...base.mvt,
        layers: source.layers,
        shape: this.state.binary ? "binary" : "geojson",
        coordinates: this.context.viewport.resolution ? "wgs84" : "local",
        tileIndex: loadProps.index,
      },
    };
    const fetch = (source.fetch ?? (this.props as MVTLayerProps).fetch) as (url: string, options: object) => Promise<never>;
    return fetch(url, { propName: "data", layer: this, loadOptions, signal: loadProps.signal });
  }
}
