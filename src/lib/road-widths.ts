// Numbered Swedish roads as wide as they really are, and gravel where they
// are gravel: Trafikverket's road database (NVDB), which the backend asks
// for by square of the map with the Trafikverket key (roads.rs). Each of a
// ground tile's roads is matched to the nearest of its stretches that runs
// the same way, and carries its width (`nvdbWidth`, metres) and surface
// (`nvdbGravel`) as properties for the ground to draw it by.

import { isTauri, type RoadPiece, roadWidths } from "./api";
import type { Lines, TileIndex } from "./bridges";

/** The backend's squares, metres (roads.rs's CELL_M), and the Earth's radius as it has it. */
const CELL_M = 2000;
const EARTH_RADIUS = 6_371_008.8;
/** A road takes a stretch's width this near to it, metres, running within this many degrees of its way. */
const MATCH_M = 10;
const MATCH_DEG = 30;
/** The index's cells, degrees. */
const INDEX_DEG = 0.002;

/** The square a point is in, as roads.rs has it, and its middle. */
export function cellOf(lon: number, lat: number): { key: string; centre: [number, number] } {
  const stepLat = (CELL_M / EARTH_RADIUS) * (180 / Math.PI);
  const j = Math.floor(lat / stepLat);
  const midLat = (j + 0.5) * stepLat;
  const stepLon = stepLat / Math.max(0.05, Math.cos((midLat * Math.PI) / 180));
  const i = Math.floor(lon / stepLon);
  return { key: `${i},${j}`, centre: [(i + 0.5) * stepLon, midLat] };
}

type Segment = { a: [number, number]; b: [number, number]; piece: RoadPiece };
const index = new Map<string, Segment[]>();
const asked = new Set<string>();
let version = 0;

/** Bumped as stretches come in: the ground's tiles drawn again with them. */
export const roadWidthsVersion = () => version;

function add(piece: RoadPiece) {
  for (let k = 1; k < piece.line.length; k++) {
    const s: Segment = { a: piece.line[k - 1], b: piece.line[k], piece };
    const [x0, x1] = [Math.min(s.a[0], s.b[0]), Math.max(s.a[0], s.b[0])];
    const [y0, y1] = [Math.min(s.a[1], s.b[1]), Math.max(s.a[1], s.b[1])];
    for (let j = Math.floor(y0 / INDEX_DEG); j <= Math.floor(y1 / INDEX_DEG); j++) {
      for (let i = Math.floor(x0 / INDEX_DEG); i <= Math.floor(x1 / INDEX_DEG); i++) {
        const key = `${i},${j}`;
        const list = index.get(key);
        if (list) list.push(s);
        else index.set(key, [s]);
      }
    }
  }
}

/**
 * Asks (once a square) for the stretches around the view's centre, in the
 * app with a key, in Sweden; `onLoad` when new ones have come.
 */
export function wantRoadWidths(lon: number, lat: number, onLoad: () => void) {
  if (!isTauri() || lon < 10.5 || lon > 24.5 || lat < 55 || lat > 69.2) return;
  const here = cellOf(lon, lat);
  const stepLat = (CELL_M / EARTH_RADIUS) * (180 / Math.PI);
  const stepLon = stepLat / Math.max(0.05, Math.cos((lat * Math.PI) / 180));
  // the square here and those within a kilometre or so
  for (const [di, dj] of [[0, 0], [-1, 0], [1, 0], [0, -1], [0, 1]]) {
    const cell = cellOf(lon + di * stepLon * 0.6, lat + dj * stepLat * 0.6);
    if (asked.has(cell.key) || (cell.key !== here.key && Math.hypot((cell.centre[0] - lon) / stepLon, (cell.centre[1] - lat) / stepLat) > 1.1)) continue;
    asked.add(cell.key);
    roadWidths(cell.centre[0], cell.centre[1])
      .then((pieces) => {
        if (!pieces.length) return;
        pieces.forEach(add);
        version++;
        onLoad();
      })
      // (asked again when the view comes back)
      .catch(() => asked.delete(cell.key));
  }
}

/** Metres from a point to a segment (near enough over a few metres), as east and north metres at `cos`. */
function distance(p: [number, number], a: [number, number], b: [number, number], cos: number): number {
  const k = (EARTH_RADIUS * Math.PI) / 180;
  const [px, py] = [(p[0] - a[0]) * k * cos, (p[1] - a[1]) * k];
  const [bx, by] = [(b[0] - a[0]) * k * cos, (b[1] - a[1]) * k];
  const len2 = bx * bx + by * by;
  const t = len2 > 0 ? Math.max(0, Math.min(1, (px * bx + py * by) / len2)) : 0;
  return Math.hypot(px - bx * t, py - by * t);
}

/** The bearing from `a` to `b`, degrees. */
const bearing = (a: [number, number], b: [number, number], cos: number) => (Math.atan2((b[0] - a[0]) * cos, b[1] - a[1]) * 180) / Math.PI;

/** The stretch a road at `p` running `heading` (degrees, either way) is on, or null. */
export function stretchAt(p: [number, number], heading: number): RoadPiece | null {
  const cos = Math.cos((p[1] * Math.PI) / 180);
  let best: { d: number; piece: RoadPiece } | null = null;
  const [ci, cj] = [Math.floor(p[0] / INDEX_DEG), Math.floor(p[1] / INDEX_DEG)];
  for (let j = cj - 1; j <= cj + 1; j++) {
    for (let i = ci - 1; i <= ci + 1; i++) {
      for (const s of index.get(`${i},${j}`) ?? []) {
        const d = distance(p, s.a, s.b, cos);
        if (d > MATCH_M || (best && d >= best.d)) continue;
        const turn = Math.abs(((bearing(s.a, s.b, cos) - heading) % 180) + 180) % 180;
        if (Math.min(turn, 180 - turn) > MATCH_DEG) continue;
        best = { d, piece: s.piece };
      }
    }
  }
  return best?.piece ?? null;
}

const done = new WeakMap<object, { version: number; out: Lines }>();

/**
 * A tile's lines (binary, in the tile's own coordinates, y south) with each
 * road's stretch's width and surface as properties, where `isRoad` says it is
 * one and a stretch is found; the lines as they were while none have come.
 */
export function withRoadWidths(lines: Lines, tile: TileIndex, isRoad: (properties: Record<string, unknown>) => boolean): Lines {
  if (!version || !lines.pathIndices?.value.length) return lines;
  const hit = done.get(lines);
  if (hit?.version === version) return hit.out;
  const n = 2 ** tile.z;
  const size = lines.positions.size;
  const pos = lines.positions.value;
  const paths = lines.pathIndices.value;
  const count = pos.length / size;
  const width = new Float32Array(count);
  const gravel = new Float32Array(count);
  const at = (i: number): [number, number] => {
    const [x, y] = [pos[i * size], pos[i * size + 1]];
    return [((tile.x + x) / n) * 360 - 180, (Math.atan(Math.sinh(Math.PI * (1 - (2 * (tile.y + y)) / n))) * 180) / Math.PI];
  };
  for (let p = 0; p + 1 < paths.length; p++) {
    const [start, end] = [paths[p], paths[p + 1]];
    if (end - start < 2 || !isRoad(lines.properties[lines.featureIds.value[start]] ?? {})) continue;
    // at its middle, the way it runs there
    const mid = start + Math.floor((end - start - 1) / 2);
    const [a, b] = [at(mid), at(mid + 1)];
    const cos = Math.cos((a[1] * Math.PI) / 180);
    const piece = stretchAt([(a[0] + b[0]) / 2, (a[1] + b[1]) / 2], bearing(a, b, cos));
    if (!piece) continue;
    width.fill(piece.width, start, end);
    gravel.fill(piece.paved ? 0 : 1, start, end);
  }
  const out: Lines = { ...lines, numericProps: { ...lines.numericProps, nvdbWidth: { value: width, size: 1 }, nvdbGravel: { value: gravel, size: 1 } } };
  done.set(lines, { version, out });
  return out;
}

/** For tests: the stretches known, as if they had come. */
export function addRoadPiecesForTest(pieces: RoadPiece[]) {
  pieces.forEach(add);
  version++;
}
