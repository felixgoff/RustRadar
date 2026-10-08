// Radar as filled, smooth-edged polygons instead of pixels. Every source ends
// up as reflectivity bands of 5 dBZ in the NWS palette (the same one the
// backend uses, `src-tauri/src/radar.rs`), triangulated once on load and
// handed to deck.gl's SolidPolygonLayer as binary attributes, so nothing is
// re-tessellated as the camera moves.

import { cutPolygonByGrid, earcut } from "@math.gl/polygon";

/** NWS reflectivity palette: band `i` covers [5 + 5i, 10 + 5i) dBZ, the last one everything above. */
const PALETTE: [number, number, number][] = [
  [0x04, 0xe9, 0xe7],
  [0x01, 0x9f, 0xf4],
  [0x03, 0x00, 0xf4],
  [0x02, 0xfd, 0x02],
  [0x01, 0xc5, 0x01],
  [0x00, 0x8e, 0x00],
  [0xfd, 0xf8, 0x02],
  [0xe5, 0xbc, 0x00],
  [0xfd, 0x95, 0x00],
  [0xfd, 0x00, 0x00],
  [0xd4, 0x00, 0x00],
  [0xbc, 0x00, 0x00],
  [0xf8, 0x00, 0xfd],
  [0x98, 0x54, 0xc6],
];
export const BAND_COUNT = PALETTE.length;
/** The lowest reflectivity (dBZ) in a band. */
export const bandDbz = (band: number) => 5 + 5 * band;
/** As `color()` in radar.rs: light echoes are translucent, so the map shows through drizzle. */
export function bandColor(band: number): [number, number, number, number] {
  const alpha = Math.min(235, Math.max(150, 150 + (bandDbz(band) - 5) * 4));
  return [...PALETTE[band], alpha];
}

/**
 * One filled polygon of a single band: flat `[x0, y0, x1, y1, …]` positions,
 * the outer ring first, then the holes, which start at the vertex indices in
 * `holeIndices` (earcut's convention).
 */
export interface BandPolygon {
  band: number;
  positions: Float64Array | number[];
  holeIndices: number[];
}

/**
 * Parses a measured volume level from the backend. Little-endian:
 * `u32 polygon_count`, then per polygon `u8 band, u8 + u16 reserved, u32 ring_count`
 * and per ring `u32 point_count` followed by that many `f32 lon, f32 lat`.
 * Ring 0 is the outer ring, the rest are holes; rings are not closed. An
 * empty blob means no echo.
 */
export function parseVolumePolygons(buffer: ArrayBuffer): BandPolygon[] {
  if (buffer.byteLength === 0) return [];
  const view = new DataView(buffer);
  const need = (offset: number, bytes: number) => {
    if (offset + bytes > buffer.byteLength) {
      throw new Error(`radar polygons truncated at byte ${offset} (of ${buffer.byteLength})`);
    }
  };
  need(0, 4);
  const count = view.getUint32(0, true);
  let offset = 4;
  const polygons: BandPolygon[] = [];
  for (let p = 0; p < count; p++) {
    need(offset, 8);
    const band = view.getUint8(offset);
    const ringCount = view.getUint32(offset + 4, true);
    offset += 8;
    const rings: Float64Array[] = [];
    for (let r = 0; r < ringCount; r++) {
      need(offset, 4);
      const points = view.getUint32(offset, true);
      offset += 4;
      need(offset, points * 8);
      const ring = new Float64Array(points * 2);
      for (let i = 0; i < points * 2; i++, offset += 4) ring[i] = view.getFloat32(offset, true);
      rings.push(ring);
    }
    // a hole too small to enclose anything is dropped; so is a polygon whose outer ring is
    if (band >= BAND_COUNT || !rings.length || rings[0].length < 6) continue;
    const kept = [rings[0], ...rings.slice(1).filter((r) => r.length >= 6)];
    const positions = new Float64Array(kept.reduce((n, r) => n + r.length, 0));
    const holeIndices: number[] = [];
    let at = 0;
    for (const [i, ring] of kept.entries()) {
      if (i) holeIndices.push(at / 2);
      positions.set(ring, at);
      at += ring.length;
    }
    polygons.push({ band, positions, holeIndices });
  }
  return polygons;
}

/**
 * Triangles of all bands, sorted by band so a band and everything above it is
 * one run of `indices`, and its vertices one run at the end of `positions`.
 */
export interface RadarMesh {
  /** Values per vertex in `positions`: 2 for the flat slices, 3 for walls. */
  size: 2 | 3;
  /** Longitude, latitude (and for walls, 0 at the foot or 1 at the top). */
  positions: Float32Array;
  /** RGBA per vertex. */
  colors: Uint8Array;
  indices: Uint32Array;
  /** Where each band's triangles start in `indices`; `BAND_COUNT + 1` entries. */
  bandStarts: Uint32Array;
  /** Where each band's vertices start (in vertices, not values); `BAND_COUNT + 1` entries. */
  bandVertexStarts: Uint32Array;
}

/**
 * Edges are drawn straight through the globe, not along it: anything wider
 * than this (degrees) is cut up first, as deck.gl does for its own polygons,
 * so the radar doesn't sink under the curve of the earth. Cutting also keeps
 * earcut fast on continent-wide polygons with thousands of holes.
 */
const GRID_DEG = 4;

export function buildMesh(polygons: BandPolygon[], gridDeg = GRID_DEG): RadarMesh {
  const sorted = [...polygons].sort((a, b) => a.band - b.band);
  // triangulated first and copied out once, into arrays of the right size
  const pieces: { band: number; positions: ArrayLike<number>; triangles: number[] }[] = [];
  let values = 0;
  let triangleCount = 0;
  for (const polygon of sorted) {
    const cut =
      needsCut(polygon.positions, gridDeg)
        ? cutPolygonByGrid(polygon.positions, polygon.holeIndices.map((i) => i * 2), { size: 2, gridResolution: gridDeg })
        : null;
    if (!cut) {
      const triangles = earcut(polygon.positions as number[], polygon.holeIndices.length ? polygon.holeIndices : undefined, 2);
      if (!triangles.length) continue;
      pieces.push({ band: polygon.band, positions: polygon.positions, triangles });
      values += polygon.positions.length;
      triangleCount += triangles.length;
      continue;
    }
    for (const piece of cut) {
      const flat = piece.positions as ArrayLike<number> as number[];
      const holes = piece.holeIndices ? Array.from(piece.holeIndices, (i) => i / 2) : undefined;
      const triangles = earcut(flat, holes, 2);
      if (!triangles.length) continue;
      pieces.push({ band: polygon.band, positions: flat, triangles });
      values += flat.length;
      triangleCount += triangles.length;
    }
  }
  const positions = new Float32Array(values);
  const colors = new Uint8Array(values * 2);
  const indices = new Uint32Array(triangleCount);
  const bandStarts = new Uint32Array(BAND_COUNT + 1);
  const bandVertexStarts = new Uint32Array(BAND_COUNT + 1);
  let band = 0;
  let vertex = 0;
  let index = 0;
  for (const piece of pieces) {
    while (band < piece.band) {
      bandStarts[++band] = index;
      bandVertexStarts[band] = vertex;
    }
    const [r, g, b, a] = bandColor(piece.band);
    const flat = piece.positions;
    positions.set(flat, vertex * 2);
    const count = flat.length / 2;
    for (let i = 0; i < count; i++) {
      const c = (vertex + i) * 4;
      colors[c] = r;
      colors[c + 1] = g;
      colors[c + 2] = b;
      colors[c + 3] = a;
    }
    for (const t of piece.triangles) indices[index++] = vertex + t;
    vertex += count;
  }
  while (band < BAND_COUNT) {
    bandStarts[++band] = index;
    bandVertexStarts[band] = vertex;
  }
  return { size: 2, positions, colors, indices, bandStarts, bandVertexStarts };
}

function needsCut(positions: ArrayLike<number>, grid: number): boolean {
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (let i = 0; i < positions.length; i += 2) {
    x0 = Math.min(x0, positions[i]);
    x1 = Math.max(x1, positions[i]);
    y0 = Math.min(y0, positions[i + 1]);
    y1 = Math.max(y1, positions[i + 1]);
  }
  return Math.floor(x0 / grid) !== Math.floor(x1 / grid) || Math.floor(y0 / grid) !== Math.floor(y1 / grid);
}

/**
 * deck.gl binary data for a SolidPolygonLayer with `_normalize: false`: the
 * triangulation is supplied as `indices`, so holes need no further work and
 * the winding doesn't matter (the layers draw both faces). The whole mesh is
 * one "polygon" to deck.gl; nothing is picked, so that is all it needs.
 */
export interface PolygonData {
  length: number;
  startIndices: Uint32Array;
  attributes: {
    getPolygon: { value: Float32Array; size: 2 | 3 };
    getFillColor: { value: Uint8Array; size: 4; normalized: true };
    indices: Uint32Array;
  };
}

/**
 * The mesh, or only its bands from `fromBand` up, ready for deck.gl; null when
 * that is nothing. Only the vertices those bands use are kept, so a layer of
 * the strong echoes alone doesn't upload the whole mesh. Each call returns a
 * new object (deck.gl writes into `attributes`, so layers must not share one);
 * keep it to keep the layer.
 */
export function polygonData(mesh: RadarMesh, fromBand = 0): PolygonData | null {
  return mergeMeshes([{ mesh, fromBand }]);
}

/**
 * Several meshes (of the same `size`), each from its own `fromBand` up, as one
 * deck.gl layer's data: one draw call however many tiles there are. Null when
 * there is nothing.
 */
export function mergeMeshes(parts: { mesh: RadarMesh; fromBand: number }[]): PolygonData | null {
  let size: 2 | 3 = 2;
  let vertices = 0;
  let indexCount = 0;
  const used: { mesh: RadarMesh; vertex: number; index: number }[] = [];
  for (const { mesh, fromBand } of parts) {
    const band = Math.min(Math.max(0, fromBand), BAND_COUNT);
    const index = mesh.bandStarts[band];
    if (index >= mesh.indices.length) continue;
    const vertex = mesh.bandVertexStarts[band];
    size = mesh.size;
    used.push({ mesh, vertex, index });
    vertices += mesh.positions.length / mesh.size - vertex;
    indexCount += mesh.indices.length - index;
  }
  if (!used.length) return null;
  let positions: Float32Array;
  let colors: Uint8Array;
  let indices: Uint32Array;
  if (used.length === 1 && used[0].vertex === 0) {
    // the whole of one mesh: shared, not copied
    const { mesh, index } = used[0];
    positions = mesh.positions;
    colors = mesh.colors;
    indices = mesh.indices.subarray(index);
  } else {
    positions = new Float32Array(vertices * size);
    colors = new Uint8Array(vertices * 4);
    indices = new Uint32Array(indexCount);
    let v = 0;
    let i = 0;
    for (const { mesh, vertex, index } of used) {
      positions.set(mesh.positions.subarray(vertex * size), v * size);
      colors.set(mesh.colors.subarray(vertex * 4), v * 4);
      const shift = v - vertex;
      const source = mesh.indices;
      for (let k = index; k < source.length; k++) indices[i++] = source[k] + shift;
      v += mesh.positions.length / size - vertex;
    }
  }
  return {
    length: 1,
    startIndices: new Uint32Array([0, positions.length / size]),
    attributes: {
      getPolygon: { value: positions, size },
      getFillColor: { value: colors, size: 4, normalized: true },
      indices,
    },
  };
}

/**
 * The outline of one band where it borders weaker echo (or none), as an open
 * polyline `[x0, y0, x1, y1, …]`; a whole ring repeats its first point at the
 * end. Each boundary between two bands is one line, in the stronger band.
 */
export interface WallLine {
  band: number;
  positions: Float64Array | number[];
}

/** Walls are a little fainter than the slices they hang from: seen edge-on, they stack up. */
const WALL_ALPHA = 0.75;

/**
 * Vertical walls along the lines, from the ground (z = 0) to unit height
 * (z = 1): a layer stretches them between two heights with its model matrix.
 * Two triangles per segment, no tessellation; sorted by band like `buildMesh`,
 * so `polygonData(walls, fromBand)` gives the walls of a layer that only keeps
 * the stronger bands.
 */
export function buildWalls(lines: WallLine[]): RadarMesh {
  const sorted = [...lines].sort((a, b) => a.band - b.band);
  let points = 0;
  for (const line of sorted) points += line.positions.length / 2;
  const positions = new Float32Array(points * 6);
  const colors = new Uint8Array(points * 8);
  const indices = new Uint32Array(points * 6);
  const bandStarts = new Uint32Array(BAND_COUNT + 1);
  const bandVertexStarts = new Uint32Array(BAND_COUNT + 1);
  let band = 0;
  let vertex = 0;
  let index = 0;
  for (const line of sorted) {
    while (band < line.band) {
      bandStarts[++band] = index;
      bandVertexStarts[band] = vertex;
    }
    const [r, g, b, a] = bandColor(line.band);
    const alpha = Math.round(a * WALL_ALPHA);
    const p = line.positions;
    let previous = -1;
    for (let i = 0; i < p.length; i += 2) {
      const x = p[i];
      const y = p[i + 1];
      // a repeated point would only add a wall of no width
      if (previous >= 0 && x === p[i - 2] && y === p[i - 1]) continue;
      const o = vertex * 3;
      positions[o] = positions[o + 3] = x;
      positions[o + 1] = positions[o + 4] = y;
      positions[o + 2] = 0;
      positions[o + 5] = 1;
      const c = vertex * 4;
      colors[c] = colors[c + 4] = r;
      colors[c + 1] = colors[c + 5] = g;
      colors[c + 2] = colors[c + 6] = b;
      colors[c + 3] = colors[c + 7] = alpha;
      if (previous >= 0) {
        indices[index++] = previous;
        indices[index++] = previous + 1;
        indices[index++] = vertex;
        indices[index++] = previous + 1;
        indices[index++] = vertex + 1;
        indices[index++] = vertex;
      }
      previous = vertex;
      vertex += 2;
    }
  }
  while (band < BAND_COUNT) {
    bandStarts[++band] = index;
    bandVertexStarts[band] = vertex;
  }
  return {
    size: 3,
    positions: positions.slice(0, vertex * 3),
    colors: colors.slice(0, vertex * 4),
    indices: indices.slice(0, index),
    bandStarts,
    bandVertexStarts,
  };
}

/**
 * The wall lines of a measured level (`parseVolumePolygons`), from the rings
 * as they come, before any grid cutting, so no wall runs along a cut. Every
 * outer ring is a wall. Bands that touch share their boundary: the backend
 * gives it to both, as one band's outer ring and the other's hole, the same
 * points reversed. Such a hole is left out and its outer ring takes the
 * stronger of the two bands; only a hole with nothing filling it is a wall of
 * its own.
 */
export function polygonWalls(polygons: BandPolygon[]): WallLine[] {
  const lines: WallLine[] = [];
  const outers = new Map<string, WallLine>();
  for (const { band, positions, holeIndices } of polygons) {
    const end = polygonRingEnds(positions.length, holeIndices)[0];
    const key = ringKey(positions, 0, end);
    const known = outers.get(key);
    if (known) {
      known.band = Math.max(known.band, band);
      continue;
    }
    const line = { band, positions: closedRing(positions, 0, end) };
    outers.set(key, line);
    lines.push(line);
  }
  const holes = new Map<string, WallLine>();
  for (const { band, positions, holeIndices } of polygons) {
    const ends = polygonRingEnds(positions.length, holeIndices);
    for (let h = 0; h < holeIndices.length; h++) {
      const [start, end] = [holeIndices[h] * 2, ends[h + 1]];
      const key = ringKey(positions, start, end);
      const known = outers.get(key) ?? holes.get(key);
      if (known) {
        known.band = Math.max(known.band, band);
        continue;
      }
      const line = { band, positions: closedRing(positions, start, end) };
      holes.set(key, line);
      lines.push(line);
    }
  }
  return lines;
}

/** Where each ring of a polygon ends in its flat positions: the outer ring, then each hole. */
function polygonRingEnds(length: number, holeIndices: number[]): number[] {
  return [...holeIndices.map((i) => i * 2), length];
}

function closedRing(positions: ArrayLike<number>, start: number, end: number): Float64Array {
  const ring = new Float64Array(end - start + 2);
  for (let i = start; i < end; i++) ring[i - start] = positions[i];
  ring[end - start] = positions[start];
  ring[end - start + 1] = positions[start + 1];
  return ring;
}

/**
 * Identifies a ring whichever way round and from whichever point it starts:
 * its point count, its lowest point (west first, then south) and its centroid,
 * rounded well below the data's resolution.
 */
function ringKey(positions: ArrayLike<number>, start: number, end: number): string {
  const q = (v: number) => Math.round(v * 1e5);
  let [mx, my] = [Infinity, Infinity];
  let [sx, sy] = [0, 0];
  for (let i = start; i < end; i += 2) {
    const [x, y] = [q(positions[i]), q(positions[i + 1])];
    sx += x;
    sy += y;
    if (x < mx || (x === mx && y < my)) [mx, my] = [x, y];
  }
  const n = (end - start) / 2;
  return `${n}:${mx},${my}:${Math.round(sx / n)},${Math.round(sy / n)}`;
}

/**
 * Clips a closed ring to the unit square as wall lines. Stretches running
 * along the square's edge are left out: that is where the grid ends, not the
 * echo, and the next tile carries on from there.
 */
function clipRingToUnit(ring: Float64Array, band: number, out: WallLine[]) {
  const n = ring.length / 2;
  let line: number[] | null = null;
  for (let i = 0; i < n; i++) {
    const j = (i + 1) % n;
    const [ax, ay, bx, by] = [ring[i * 2], ring[i * 2 + 1], ring[j * 2], ring[j * 2 + 1]];
    const clipped = clipSegment(ax, ay, bx, by);
    if (!clipped || alongEdge(clipped)) {
      line = null;
      continue;
    }
    const [px, py, qx, qy] = clipped;
    // carries on from the last segment unless that one was cut short
    if (line && line[line.length - 2] === px && line[line.length - 1] === py) {
      line.push(qx, qy);
    } else {
      line = [px, py, qx, qy];
      out.push({ band, positions: line });
    }
  }
}

/** Liang–Barsky against the unit square: what of the segment lies inside, or null. */
function clipSegment(ax: number, ay: number, bx: number, by: number): [number, number, number, number] | null {
  const [dx, dy] = [bx - ax, by - ay];
  let [t0, t1] = [0, 1];
  for (const [p, q] of [
    [-dx, ax],
    [dx, 1 - ax],
    [-dy, ay],
    [dy, 1 - ay],
  ]) {
    if (p === 0) {
      if (q < 0) return null;
    } else {
      const t = q / p;
      if (p < 0) t0 = Math.max(t0, t);
      else t1 = Math.min(t1, t);
    }
  }
  if (t0 >= t1) return null;
  // ends that aren't clipped keep their exact values, so lines join up
  return [
    t0 === 0 ? ax : ax + t0 * dx,
    t0 === 0 ? ay : ay + t0 * dy,
    t1 === 1 ? bx : ax + t1 * dx,
    t1 === 1 ? by : ay + t1 * dy,
  ];
}

const EDGE = 1e-9;
function alongEdge([px, py, qx, qy]: [number, number, number, number]): boolean {
  const on = (a: number, b: number, edge: number) => Math.abs(a - edge) < EDGE && Math.abs(b - edge) < EDGE;
  return on(px, qx, 0) || on(px, qx, 1) || on(py, qy, 0) || on(py, qy, 1);
}

/**
 * Contours a reflectivity grid (dBZ, row 0 at the top; anything below 5 is no
 * echo) into one set of polygons per 5 dBZ band, in unit coordinates: x from
 * 0 (left edge of the grid) to 1 (right edge), y from 0 (top) to 1 (bottom).
 * The polygons fill exactly to the grid's edges, so neighbouring grids join
 * without a seam.
 *
 * With `walls`, the outlines of the bands are added to it as well, clipped to
 * the grid and in the same coordinates, for `buildWalls`. They come from the
 * contours themselves, before the polygons are cut into pieces, so they run
 * only where the reflectivity changes band: each threshold's rings once, in
 * the band that starts there, and nothing along the grid's edges or cuts.
 * They are simplified to within a tenth of a cell of the contours.
 */
export function contourBands(
  values: Float32Array,
  width: number,
  height: number,
  walls?: WallLine[],
): BandPolygon[] {
  let max = -Infinity;
  for (let i = 0; i < values.length; i++) if (values[i] > max) max = values[i];
  if (!(max >= bandDbz(0))) return [];

  // Two cells of padding: the edge values repeated, then nothing. Contours
  // then run exactly as they would if the grid went on, out to the repeated
  // cells, and close in the outer ring, clear of the grid. Without it, the
  // rings would close along the grid edge itself, where the rings of
  // neighbouring thresholds would overlap.
  const PAD = 2;
  const w = width + 2 * PAD;
  const h = height + 2 * PAD;
  const padded = new Float32Array(w * h);
  for (let y = 1; y < h - 1; y++) {
    const sy = Math.min(height - 1, Math.max(0, y - PAD));
    for (let x = 1; x < w - 1; x++) {
      const sx = Math.min(width - 1, Math.max(0, x - PAD));
      padded[y * w + x] = values[sy * width + sx];
    }
  }

  const thresholds: number[] = [];
  for (let b = 0; b < BAND_COUNT && bandDbz(b) <= max; b++) thresholds.push(bandDbz(b));
  const tracer = new IsoRings(w, h);
  // rings of everything at or above each threshold, in unit coordinates
  const above = thresholds.map((t) => {
    const rings: Float64Array[] = [];
    tracer.trace(padded, t, (ring) => {
      const flat = new Float64Array(ring.length);
      for (let i = 0; i < ring.length; i += 2) {
        flat[i] = (ring[i] - PAD) / width;
        flat[i + 1] = (ring[i + 1] - PAD) / height;
      }
      // specks a fraction of a cell across are noise; dropped from the
      // threshold, they vanish from both bands that share the ring
      if (Math.abs(ringArea(flat)) * width * height >= 0.3) rings.push(flat);
    });
    return rings;
  });
  if (walls) {
    const from = walls.length;
    above.forEach((rings, band) => rings.forEach((ring) => clipRingToUnit(ring, band, walls)));
    // a tenth of a cell is far below what shows, and drops nearly half the points
    const tolerance = WALL_TOLERANCE / Math.max(width, height);
    for (let i = from; i < walls.length; i++) walls[i].positions = simplifyLine(walls[i].positions, tolerance);
  }

  const out: BandPolygon[] = [];
  for (let band = 0; band < thresholds.length; band++) {
    // a band is what is above its threshold but not above the next: with
    // rings that never cross, that is the even-odd fill of both sets
    for (const polygon of evenOdd([...above[band], ...(above[band + 1] ?? [])])) {
      // cut on a grid that has the grid's edges among its lines: that clips
      // to the grid, and smaller pieces with fewer holes each keep earcut fast
      // on speckled tiles
      const pieces = cutPolygonByGrid(polygon.positions, polygon.holeIndices.map((i) => i * 2), {
        size: 2,
        gridResolution: 1 / 8,
      });
      for (const piece of pieces) {
        // keep what lies within the grid; the padding falls away
        const [cx, cy] = centre(piece.positions, piece.holeIndices?.[0] ?? piece.positions.length);
        if (cx <= 0 || cx >= 1 || cy <= 0 || cy >= 1) continue;
        out.push({
          band,
          positions: piece.positions as number[],
          holeIndices: piece.holeIndices ? Array.from(piece.holeIndices, (i) => i / 2) : [],
        });
      }
    }
  }
  return out;
}

/**
 * Marching squares with the isolines stitched into closed rings and placed by
 * linear interpolation: d3-contour's `isorings` and `smoothLinear` (ISC
 * licence, Mike Bostock), ported to flat arrays. The rings come out exactly as
 * d3's do; what is left out is d3's matching of holes to their polygons, which
 * compares every hole with every polygon and dominated the time on speckled
 * tiles, and which `evenOdd` does again anyway.
 */
class IsoRings {
  private readonly byStart: Int32Array;
  private readonly byEnd: Int32Array;
  private readonly span: number;
  private readonly first: Int32Array;
  private readonly last: Int32Array;
  private fragments: { start: number; end: number; ring: number[] }[] = [];

  constructor(
    private readonly dx: number,
    private readonly dy: number,
  ) {
    // points sit on half cells from -0.5 to d + 0.5: one slot each
    this.span = 2 * dx + 3;
    this.byStart = new Int32Array(this.span * (2 * dy + 3));
    this.byEnd = new Int32Array(this.span * (2 * dy + 3));
    this.first = new Int32Array(dy);
    this.last = new Int32Array(dy);
  }

  /** Calls `emit` with each closed ring (last point repeating the first) of `values >= value`. */
  trace(values: Float32Array, value: number, emit: (ring: number[]) => void) {
    const { dx, dy } = this;
    this.byStart.fill(0);
    this.byEnd.fill(0);
    this.fragments = [];
    const done = (ring: number[]) => {
      smoothLinear(ring, values, dx, dy, value);
      emit(ring);
    };
    // Only the cells around values at or above the threshold can hold a
    // line: each row's span of those is found first, and the rest of the
    // grid (most of a tile, usually) is skipped. The cells visited are the
    // same, in the same order, as d3's full sweep that skips nothing.
    const { first, last } = this;
    for (let y = 0; y < dy; y++) {
      const row = y * dx;
      let lo = dx;
      let hi = -1;
      for (let x = 0; x < dx; x++) {
        if (values[row + x] >= value) {
          if (lo === dx) lo = x;
          hi = x;
        }
      }
      first[y] = lo;
      last[y] = hi;
    }
    const at = (x: number, y: number) => (x >= 0 && x < dx && y >= 0 && y < dy && values[y * dx + x] >= value ? 1 : 0);
    // cell (x, y) has the values (x, y) to (x + 1, y + 1) at its corners; the
    // grid is ringed by cells half outside it, from -1 to d - 1
    for (let y = -1; y < dy; y++) {
      let lo = dx;
      let hi = -1;
      if (y >= 0) [lo, hi] = [first[y], last[y]];
      if (y + 1 < dy) [lo, hi] = [Math.min(lo, first[y + 1]), Math.max(hi, last[y + 1])];
      for (let x = lo - 1; x <= hi; x++) {
        const c = at(x, y + 1) | (at(x + 1, y + 1) << 1) | (at(x + 1, y) << 2) | (at(x, y) << 3);
        if (c && c !== 15) this.stitch(c, x, y, done);
      }
    }
  }

  private key(x: number, y: number) {
    return x * 2 + 1 + (y * 2 + 1) * this.span;
  }

  private stitch(c: number, x: number, y: number, done: (ring: number[]) => void) {
    const segments = CASES[c];
    for (let k = 0; k < segments.length; k += 4) {
      const sx = segments[k] + x;
      const sy = segments[k + 1] + y;
      const ex = segments[k + 2] + x;
      const ey = segments[k + 3] + y;
      const si = this.key(sx, sy);
      const ei = this.key(ex, ey);
      const { byStart, byEnd, fragments } = this;
      let fi = byEnd[si];
      if (fi) {
        const f = fragments[fi - 1];
        const gi = byStart[ei];
        if (gi) {
          const g = fragments[gi - 1];
          byEnd[f.end] = 0;
          byStart[g.start] = 0;
          if (f === g) {
            f.ring.push(ex, ey);
            done(f.ring);
          } else {
            fragments.push({ start: f.start, end: g.end, ring: f.ring.concat(g.ring) });
            byStart[f.start] = byEnd[g.end] = fragments.length;
          }
        } else {
          byEnd[f.end] = 0;
          f.ring.push(ex, ey);
          byEnd[(f.end = ei)] = fi;
        }
      } else if ((fi = byStart[ei])) {
        // nothing ends where this segment starts (that was the case above)
        const f = fragments[fi - 1];
        byStart[f.start] = 0;
        f.ring.unshift(sx, sy);
        byStart[(f.start = si)] = fi;
      } else {
        fragments.push({ start: si, end: ei, ring: [sx, sy, ex, ey] });
        byStart[si] = byEnd[ei] = fragments.length;
      }
    }
  }
}

/** Marching squares segments per case, `[x0, y0, x1, y1, …]` within the cell (d3-contour's table). */
const CASES: number[][] = [
  [],
  [1.0, 1.5, 0.5, 1.0],
  [1.5, 1.0, 1.0, 1.5],
  [1.5, 1.0, 0.5, 1.0],
  [1.0, 0.5, 1.5, 1.0],
  [1.0, 1.5, 0.5, 1.0, 1.0, 0.5, 1.5, 1.0],
  [1.0, 0.5, 1.0, 1.5],
  [1.0, 0.5, 0.5, 1.0],
  [0.5, 1.0, 1.0, 0.5],
  [1.0, 1.5, 1.0, 0.5],
  [0.5, 1.0, 1.0, 0.5, 1.5, 1.0, 1.0, 1.5],
  [1.5, 1.0, 1.0, 0.5],
  [0.5, 1.0, 1.5, 1.0],
  [1.0, 1.5, 1.5, 1.0],
  [0.5, 1.0, 1.0, 1.5],
  [],
];

/** Moves each point along its cell edge to where the values cross `value`. */
function smoothLinear(ring: number[], values: Float32Array, dx: number, dy: number, value: number) {
  const at = (i: number) => (i >= 0 && i < values.length ? values[i] : -Infinity);
  for (let i = 0; i < ring.length; i += 2) {
    const x = ring[i];
    const y = ring[i + 1];
    const xt = x | 0;
    const yt = y | 0;
    const v1 = at(yt * dx + xt);
    if (x > 0 && x < dx && xt === x) ring[i] = smooth1(x, at(yt * dx + xt - 1), v1, value);
    if (y > 0 && y < dy && yt === y) ring[i + 1] = smooth1(y, at((yt - 1) * dx + xt), v1, value);
  }
}

function smooth1(x: number, v0: number, v1: number, value: number) {
  const a = value - v0;
  const b = v1 - v0;
  const d = isFinite(a) || isFinite(b) ? a / b : Math.sign(a) / Math.sign(b);
  return isNaN(d) ? x : x + d - 0.5;
}

/** How far (in cells) a wall may stray from its contour: walls are simplified, the slices are not. */
const WALL_TOLERANCE = 0.1;

/**
 * Douglas–Peucker: the points of a flat polyline that keep it within
 * `tolerance` of the original. The ends are always kept, so lines still meet
 * where they did, and a closed ring stays closed.
 */
export function simplifyLine(p: ArrayLike<number>, tolerance: number): number[] {
  const n = p.length / 2;
  if (n < 3) return Array.from(p);
  const keep = new Uint8Array(n);
  keep[0] = keep[n - 1] = 1;
  const stack = [0, n - 1];
  while (stack.length) {
    const b = stack.pop()!;
    const a = stack.pop()!;
    const [ax, ay] = [p[a * 2], p[a * 2 + 1]];
    const [dx, dy] = [p[b * 2] - ax, p[b * 2 + 1] - ay];
    const length = Math.hypot(dx, dy);
    let worst = tolerance;
    let at = -1;
    for (let i = a + 1; i < b; i++) {
      const [x, y] = [p[i * 2] - ax, p[i * 2 + 1] - ay];
      // distance from the chord, or from its start where the chord is a point (a closed ring)
      const d = length ? Math.abs(x * dy - y * dx) / length : Math.hypot(x, y);
      if (d > worst) [worst, at] = [d, i];
    }
    if (at >= 0) {
      keep[at] = 1;
      stack.push(a, at, at, b);
    }
  }
  const out: number[] = [];
  for (let i = 0; i < n; i++) if (keep[i]) out.push(p[i * 2], p[i * 2 + 1]);
  return out;
}

/** Shoelace area of a flat ring. */
function ringArea(ring: ArrayLike<number>): number {
  let sum = 0;
  const n = ring.length;
  for (let i = 0, j = n - 2; i < n; j = i, i += 2) sum += (ring[j] - ring[i]) * (ring[j + 1] + ring[i + 1]);
  return sum / 2;
}

function centre(positions: ArrayLike<number>, end: number): [number, number] {
  let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (let i = 0; i < end; i += 2) {
    x0 = Math.min(x0, positions[i]);
    x1 = Math.max(x1, positions[i]);
    y0 = Math.min(y0, positions[i + 1]);
    y1 = Math.max(y1, positions[i + 1]);
  }
  return [(x0 + x1) / 2, (y0 + y1) / 2];
}

function insideRing(ring: ArrayLike<number>, x: number, y: number): boolean {
  let inside = false;
  const n = ring.length;
  for (let i = 0, j = n - 2; i < n; j = i, i += 2) {
    const [xi, yi, xj, yj] = [ring[i], ring[i + 1], ring[j], ring[j + 1]];
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

/**
 * Polygons (outer ring plus holes) filling a set of non-crossing rings by the
 * even-odd rule: rings nested an even number deep are outer rings, and the
 * rings directly inside them are their holes.
 */
export function evenOdd(rings: Float64Array[]): { positions: Float64Array; holeIndices: number[] }[] {
  const info = rings
    .map((ring) => {
      let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
      for (let i = 0; i < ring.length; i += 2) {
        x0 = Math.min(x0, ring[i]);
        x1 = Math.max(x1, ring[i]);
        y0 = Math.min(y0, ring[i + 1]);
        y1 = Math.max(y1, ring[i + 1]);
      }
      return { ring, area: Math.abs(ringArea(ring)), box: [x0, y0, x1, y1], depth: 0, holes: [] as Float64Array[] };
    })
    .sort((a, b) => b.area - a.area);
  // rings are filed in a coarse grid by their boxes, so a speckled field with
  // thousands of rings doesn't compare every pair
  const N = 32;
  let [gx0, gy0, gx1, gy1] = [Infinity, Infinity, -Infinity, -Infinity];
  for (const { box } of info) {
    gx0 = Math.min(gx0, box[0]);
    gy0 = Math.min(gy0, box[1]);
    gx1 = Math.max(gx1, box[2]);
    gy1 = Math.max(gy1, box[3]);
  }
  const cellX = (x: number) => Math.min(N - 1, Math.max(0, Math.floor(((x - gx0) / (gx1 - gx0 || 1)) * N)));
  const cellY = (y: number) => Math.min(N - 1, Math.max(0, Math.floor(((y - gy0) / (gy1 - gy0 || 1)) * N)));
  const cells: number[][] = Array.from({ length: N * N }, () => []);
  for (let i = 0; i < info.length; i++) {
    const r = info[i];
    const [x, y] = [r.ring[0], r.ring[1]];
    // the smallest ring around this one is its parent: filed in order of
    // falling area, the last match in the cell is the one
    const candidates = cells[cellY(y) * N + cellX(x)];
    for (let k = candidates.length - 1; k >= 0; k--) {
      const p = info[candidates[k]];
      if (r.box[0] < p.box[0] || r.box[1] < p.box[1] || r.box[2] > p.box[2] || r.box[3] > p.box[3]) continue;
      if (!insideRing(p.ring, x, y)) continue;
      r.depth = p.depth + 1;
      if (r.depth % 2) p.holes.push(r.ring);
      break;
    }
    for (let cy = cellY(r.box[1]); cy <= cellY(r.box[3]); cy++) {
      for (let cx = cellX(r.box[0]); cx <= cellX(r.box[2]); cx++) cells[cy * N + cx].push(i);
    }
  }
  return info
    .filter((r) => r.depth % 2 === 0)
    .map(({ ring, holes }) => {
      const positions = new Float64Array(ring.length + holes.reduce((n, h) => n + h.length, 0));
      positions.set(ring);
      const holeIndices: number[] = [];
      let at = ring.length;
      for (const hole of holes) {
        holeIndices.push(at / 2);
        positions.set(hole, at);
        at += hole.length;
      }
      return { positions, holeIndices };
    });
}

/** Light 3×3 binomial blur, edges clamped: takes the stair-steps out of the contours. */
export function blur(values: Float32Array, width: number, height: number): Float32Array {
  const tmp = new Float32Array(values.length);
  const out = new Float32Array(values.length);
  for (let y = 0; y < height; y++) {
    const row = y * width;
    for (let x = 0; x < width; x++) {
      const l = values[row + Math.max(0, x - 1)];
      const r = values[row + Math.min(width - 1, x + 1)];
      tmp[row + x] = (l + 2 * values[row + x] + r) / 4;
    }
  }
  for (let y = 0; y < height; y++) {
    const up = Math.max(0, y - 1) * width;
    const down = Math.min(height - 1, y + 1) * width;
    for (let x = 0; x < width; x++) out[y * width + x] = (tmp[up + x] + 2 * tmp[y * width + x] + tmp[down + x]) / 4;
  }
  return out;
}
