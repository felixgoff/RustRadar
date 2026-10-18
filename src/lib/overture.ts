// How tall buildings are where the map's own data doesn't say: Overture Maps'
// buildings (© OpenStreetMap, © Overture Maps Foundation, ODbL), which add
// machine-learned heights (Microsoft's) to OpenStreetMap's. Read from
// Overture's own PMTiles file of them, a zoom-14 tile at a time, with range
// requests; the release is the latest its catalogue names.

import { parse } from "@loaders.gl/core";
import { MVTLoader } from "@loaders.gl/mvt";
import { PMTiles } from "pmtiles";
import { sglFetch } from "./streets-gl";

export const OVERTURE_ATTRIBUTION = "Building heights: © Overture Maps Foundation, © OpenStreetMap (ODbL)";
const CATALOG = "https://stac.overturemaps.org/catalog.json";
/** The release asked for when the catalogue can't be read. */
const KNOWN_RELEASE = "2026-09-23.1";
const ZOOM = 14;

/** An Overture building: its outline (longitude, latitude), its box, and its height and floors where it has them. */
type Building = { ring: [number, number][]; box: [number, number, number, number]; height: number; floors: number };
/** Tall enough to say: what a building is given, metres or floors. */
export type Height = { height: number; floors: number };

let archive: Promise<PMTiles | null> | null = null;
function buildings(): Promise<PMTiles | null> {
  archive ??= fetch(CATALOG)
    .then((r) => (r.ok ? r.json() : null))
    .then((c: { latest?: string } | null) => c?.latest ?? KNOWN_RELEASE)
    .catch(() => KNOWN_RELEASE)
    .then((release) => new PMTiles(`https://tiles.overturemaps.org/${release}/buildings.pmtiles`));
  return archive;
}

/** Whether a point is inside a ring (even-odd). */
export function inside(p: [number, number], ring: [number, number][]): boolean {
  let yes = false;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const [[xi, yi], [xj, yj]] = [ring[i], ring[j]];
    if (yi > p[1] !== yj > p[1] && p[0] < ((xj - xi) * (p[1] - yi)) / (yj - yi) + xi) yes = !yes;
  }
  return yes;
}

type Feature = { geometry: { type: string; coordinates: unknown }; properties: Record<string, unknown> };

/** A tile's buildings with a height or floors, from its decoded features (longitude and latitude). */
export function tallBuildings(features: Feature[]): Building[] {
  const out: Building[] = [];
  for (const f of features) {
    const height = Number(f.properties.height) || 0;
    const floors = Number(f.properties.num_floors) || 0;
    if (!(height > 0 || floors > 0)) continue;
    const polygons = (f.geometry.type === "Polygon" ? [f.geometry.coordinates] : f.geometry.type === "MultiPolygon" ? f.geometry.coordinates : []) as [number, number][][][];
    for (const polygon of polygons) {
      const ring = polygon[0];
      if (!ring || ring.length < 3) continue;
      const xs = ring.map((p) => p[0]);
      const ys = ring.map((p) => p[1]);
      out.push({ ring, box: [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)], height, floors });
    }
  }
  return out;
}

const tiles = new Map<string, Promise<Building[]>>();

/** The buildings with heights in a zoom-14 tile (none where it can't be had). */
function tileBuildings(x: number, y: number): Promise<Building[]> {
  const key = `${x}/${y}`;
  let hit = tiles.get(key);
  if (!hit) {
    hit = buildings()
      .then((a) => a?.getZxy(ZOOM, x, y))
      .then(async (r) => {
        if (!r?.data) return [];
        const features = (await parse(r.data, MVTLoader, {
          worker: false,
          mvt: { shape: "geojson", coordinates: "wgs84", tileIndex: { x, y, z: ZOOM }, layers: ["building", "building_part"] },
        })) as unknown as Feature[];
        return tallBuildings(features);
      })
      .catch(() => []);
    tiles.set(key, hit);
    while (tiles.size > 48) tiles.delete(tiles.keys().next().value!);
  }
  return hit;
}

/** The zoom-14 tile a point is in. */
function tileOf(lon: number, lat: number): [number, number] {
  const n = 2 ** ZOOM;
  const r = (lat * Math.PI) / 180;
  return [Math.floor(((lon + 180) / 360) * n), Math.floor(((1 - Math.log(Math.tan(r) + 1 / Math.cos(r)) / Math.PI) / 2) * n)];
}

/** The tallest Overture building (or part) at each point, as its height and floors; null where none has them. */
export async function heightsAt(points: [number, number][]): Promise<(Height | null)[]> {
  const byTile = new Map<string, Promise<Building[]>>();
  for (const p of points) {
    const [x, y] = tileOf(p[0], p[1]);
    if (!byTile.has(`${x}/${y}`)) byTile.set(`${x}/${y}`, tileBuildings(x, y));
  }
  const loaded = new Map<string, Building[]>();
  for (const [k, v] of byTile) loaded.set(k, await v);
  return points.map((p) => {
    const [x, y] = tileOf(p[0], p[1]);
    let best: Height | null = null;
    for (const b of loaded.get(`${x}/${y}`) ?? []) {
      if (p[0] < b.box[0] || p[0] > b.box[2] || p[1] < b.box[1] || p[1] > b.box[3] || !inside(p, b.ring)) continue;
      if (!best || b.height > best.height || (!b.height && !best.height && b.floors > best.floors)) best = { height: b.height, floors: b.floors };
    }
    return best;
  });
}

type Column = { value: ArrayLike<number> & { length: number }; size: number };
/** A binary tile's polygons (loaders.gl's), as far as they are read here: in the tile's own coordinates, y south. */
type Polygons = { positions: Column; featureIds: Column; numericProps: Record<string, Column> };

/**
 * A Streets GL tile's buildings (binary, in the tile's own coordinates) with
 * Overture's heights given to those it has none for (neither a height nor
 * levels mapped): `height`, else `levels`, set on each of their vertices.
 */
export async function withOvertureHeights(polygons: Polygons, tile: { x: number; y: number; z: number }): Promise<void> {
  const { positions, featureIds, numericProps } = polygons;
  const count = featureIds.value.length;
  if (!count) return;
  const size = positions.size;
  const own = (name: string) => numericProps[name]?.value;
  const [heights, levels] = [own("height"), own("levels")];
  // each feature's vertices: where they start and end, and their middle
  const n = 2 ** tile.z;
  const spans: { start: number; end: number; at: [number, number] }[] = [];
  for (let start = 0; start < count; ) {
    let end = start;
    let [sx, sy] = [0, 0];
    while (end < count && featureIds.value[end] === featureIds.value[start]) {
      sx += positions.value[end * size];
      sy += positions.value[end * size + 1];
      end++;
    }
    if (!(heights?.[start] || levels?.[start])) {
      const [x, y] = [sx / (end - start), sy / (end - start)];
      spans.push({ start, end, at: [((tile.x + x) / n) * 360 - 180, (Math.atan(Math.sinh(Math.PI * (1 - (2 * (tile.y + y)) / n))) * 180) / Math.PI] });
    }
    start = end;
  }
  if (!spans.length) return;
  const found = await heightsAt(spans.map((s) => s.at));
  if (!found.some(Boolean)) return;
  const column = (name: string) => {
    const existing = numericProps[name];
    if (existing && existing.value instanceof Float32Array) return existing.value;
    const fresh = new Float32Array(count);
    if (existing) for (let i = 0; i < count; i++) fresh[i] = existing.value[i];
    numericProps[name] = { value: fresh, size: 1 };
    return fresh;
  };
  const [h, l] = [column("height"), column("levels")];
  spans.forEach((s, k) => {
    const f = found[k];
    if (!f) return;
    if (f.height > 0) h.fill(f.height, s.start, s.end);
    else l.fill(f.floors, s.start, s.end);
  });
}

/** The Streets GL buildings' tiles (streets-gl.ts's `sglFetch`), with Overture's heights where they have none. */
export async function sglBuildingFetch(url: string, options: { loadOptions?: object }) {
  const content = (await sglFetch(url, options as never)) as { polygons?: Polygons } | null;
  const m = url.match(/\/(\d+)\/(\d+)\/(\d+)(?:\?|$)/);
  if (content?.polygons && m) await withOvertureHeights(content.polygons, { z: Number(m[1]), x: Number(m[2]), y: Number(m[3]) }).catch(() => {});
  return content;
}
