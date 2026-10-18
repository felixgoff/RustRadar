// The realistic map's ground where the map's data says nothing: what the
// land cover (landcover.ts) has there, woods, fields, mires, heath, in the
// ground's own surfaces (realistic-ground.ts). Drawn over the grass that
// lies everywhere and under every mapped area, so it shows only where the
// map leaves the land unmapped.
//
// Each class of a tile's 10 m grid is traced into outlines (d3-contour), the
// grid softened first so they come out rounded rather than in steps.

import { TileLayer } from "@deck.gl/geo-layers";
import { SolidPolygonLayer } from "@deck.gl/layers";
import { contours } from "d3-contour";
import { landCover, type LandGrid } from "./landcover";
import { LAND_COVER_PAINTS } from "./realistic-ground";

type RGBA = [number, number, number, number];
type Ring = [number, number][];
/** An outline of one kind of ground, longitude and latitude, with its holes. */
export type CoverArea = { polygon: Ring[]; paint: RGBA };

/** Areas smaller than this many of the grid's cells are left out. */
const MIN_CELLS = 6;

/** A mask's values averaged with their neighbours', `passes` times over. */
function soften(mask: Float32Array<ArrayBuffer>, size: number, passes: number): Float32Array<ArrayBuffer> {
  let a = mask;
  let b = new Float32Array(a.length);
  for (let p = 0; p < passes; p++) {
    for (let y = 0; y < size; y++) {
      for (let x = 0; x < size; x++) {
        let sum = 0;
        let n = 0;
        for (let j = Math.max(0, y - 1); j <= Math.min(size - 1, y + 1); j++) {
          for (let i = Math.max(0, x - 1); i <= Math.min(size - 1, x + 1); i++) {
            sum += a[j * size + i];
            n++;
          }
        }
        b[y * size + x] = sum / n;
      }
    }
    [a, b] = [b, a];
  }
  return a;
}

const ringArea = (r: Ring) => Math.abs(r.reduce((s, [x, y], i) => s + x * r[(i + 1) % r.length][1] - r[(i + 1) % r.length][0] * y, 0)) / 2;

/** A tile's land cover as outlines of each kind of ground it paints, in the grid's own cells (0 to `size`, y down). */
export function coverOutlines(grid: LandGrid): { polygon: Ring[]; paint: RGBA }[] {
  const { size, classes } = grid;
  const out: { polygon: Ring[]; paint: RGBA }[] = [];
  const present = new Set(classes);
  // classes sharing a paint traced as one
  const byPaint = new Map<RGBA, Set<number>>();
  for (const c of present) {
    const paint = LAND_COVER_PAINTS[c];
    if (!paint) continue;
    if (!byPaint.has(paint)) byPaint.set(paint, new Set());
    byPaint.get(paint)!.add(c);
  }
  for (const [paint, group] of byPaint) {
    const mask = new Float32Array(size * size);
    let count = 0;
    for (let i = 0; i < mask.length; i++) {
      if (group.has(classes[i])) {
        mask[i] = 1;
        count++;
      }
    }
    if (count < MIN_CELLS) continue;
    const [shape] = contours().size([size, size]).thresholds([0.5])(Array.from(soften(mask, size, 2)));
    for (const polygon of shape?.coordinates ?? []) {
      if (ringArea(polygon[0] as Ring) < MIN_CELLS) continue;
      out.push({ polygon: polygon as Ring[], paint });
    }
  }
  return out;
}

/** The outlines in longitude and latitude, for a tile's grid. */
function placed(t: { x: number; y: number; z: number }, grid: LandGrid): CoverArea[] {
  const n = 2 ** t.z;
  const lonOf = (x: number) => ((t.x + x / grid.size) / n) * 360 - 180;
  const latOf = (y: number) => (Math.atan(Math.sinh(Math.PI * (1 - (2 * (t.y + y / grid.size)) / n))) * 180) / Math.PI;
  return coverOutlines(grid).map(({ polygon, paint }) => ({
    polygon: polygon.map((ring) => ring.map(([x, y]) => [lonOf(x), latOf(y)] as [number, number])),
    paint,
  }));
}

type TileProps = { id: string; data: CoverArea[] | null; tile: { index: { x: number; y: number; z: number } } };

/**
 * The land cover's ground, a tile's at zoom 10 to 14 (finer than 14 is finer
 * than the land cover itself: those tiles stretched). Flat projection.
 */
export const landCoverGroundLayer = (extensions: unknown[], parameters: object) =>
  new TileLayer<CoverArea[] | null>({
    id: "ground-land-cover",
    minZoom: 10,
    maxZoom: 14,
    tileSize: 256,
    maxRequests: 4,
    getTileData: async ({ index }: { index: { x: number; y: number; z: number } }) => {
      const grid = await landCover(index);
      return grid ? placed(index, grid) : null;
    },
    renderSubLayers: (props: TileProps) =>
      props.data?.length
        ? new SolidPolygonLayer<CoverArea>({
            id: `${props.id}-areas`,
            data: props.data,
            getPolygon: (d) => d.polygon as never,
            getFillColor: (d) => d.paint,
            extensions: extensions as never,
            parameters,
          })
        : null,
  } as never);
