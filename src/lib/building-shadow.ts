// Building shadows on the ground, cast by the real sun.
//
// A building is its footprint extruded straight up. Its shadow on the flat
// ground is the footprint swept along the sun's ground direction by
// `height / tan(sun altitude)`: the Minkowski sum of the footprint with that
// segment. That is the footprint itself plus, for every wall that faces away
// from the sun, the parallelogram the wall's foot sweeps out. The footprint
// is under the building and never seen, so only the parallelograms are drawn:
// two triangles per wall, no tessellation, exact for any footprint, concave
// or holed. They overlap each other (and other buildings' shadows) where
// walls meet; the layer that draws them lets only one through per pixel.

/** At a low sun a shadow is at most this many times the building's height. */
export const SHADOW_LENGTH_CAP = 10;
/** Building shadows in full sun: a little lighter than an aircraft's on the ground. */
export const BUILDING_SHADOW_ALPHA = 0.35;

/**
 * Where the shadow of a point one metre up lands, as [east, north] metres
 * from the point below it, with `toSun` the direction toward the sun as
 * [east, north, up] (`localSun`). Null with the sun on or below the horizon.
 * Long shadows are capped at {@link SHADOW_LENGTH_CAP}.
 */
export function shadowStep(toSun: readonly number[], cap = SHADOW_LENGTH_CAP): [number, number] | null {
  const [east, north, up] = toSun;
  if (!(up > 0)) return null;
  const across = Math.hypot(east, north);
  if (across === 0) return [0, 0];
  // cot(altitude), the length of the shadow per metre of height
  const length = Math.min(across / up, cap);
  return [(-east / across) * length, (-north / across) * length];
}

/**
 * Footprints in the layout of loaders.gl's binary polygons: flat positions
 * (`size` numbers per vertex), and the vertex at which each polygon and each
 * ring starts, each list ending with the vertex count. A polygon's first ring
 * is its outline, the rest are holes; rings may or may not repeat their first
 * vertex at the end.
 */
export interface Footprints {
  positions: ArrayLike<number>;
  size: number;
  polygonStarts: ArrayLike<number>;
  ringStarts: ArrayLike<number>;
}

/** Triangles on the ground: flat `[x, y, …]` positions and their indices. */
export interface ShadowMesh {
  positions: Float32Array;
  indices: Uint32Array;
}

/**
 * The shadows of extruded footprints, in the footprints' own coordinates.
 * `heights[p]` is polygon `p`'s height in metres (0 or less casts nothing)
 * and `step` is where one metre of height lands, in the same units as the
 * positions (from {@link shadowStep}, scaled to them). Null when nothing
 * casts a shadow.
 */
export function buildingShadows(footprints: Footprints, heights: ArrayLike<number>, step: readonly number[]): ShadowMesh | null {
  const { positions, size, polygonStarts, ringStarts } = footprints;
  const [sx, sy] = step;
  if (sx === 0 && sy === 0) return null;
  const vertexCount = Math.floor(positions.length / size);
  // at most one wall per vertex, four corners and six indices each
  const out = new Float32Array(vertexCount * 8);
  const indices = new Uint32Array(vertexCount * 6);
  let v = 0;
  let k = 0;
  let polygon = -1;
  const rings = ringStarts.length - 1;
  for (let r = 0; r < rings; r++) {
    const start = ringStarts[r];
    const end = ringStarts[r + 1];
    // which polygon this ring belongs to, and whether it is the outline
    while (polygon + 1 < polygonStarts.length - 1 && polygonStarts[polygon + 1] <= start) polygon++;
    const outline = polygon >= 0 && polygonStarts[polygon] === start;
    if (end - start < 3) continue;
    const height = polygon >= 0 ? heights[polygon] : 0;
    // twice the signed area: positive when the ring runs counter-clockwise (x right, y up)
    let area = 0;
    for (let i = start, j = end - 1; i < end; j = i++) {
      area += positions[j * size] * positions[i * size + 1] - positions[i * size] * positions[j * size + 1];
    }
    if (!(height > 0) || area === 0) continue;
    // A wall's outward normal points away from the building's material: to
    // the right of a counter-clockwise outline, into a hole.
    const sign = outline ? Math.sign(area) : -Math.sign(area);
    const dx = sx * height;
    const dy = sy * height;
    for (let i = start; i < end; i++) {
      const next = i + 1 < end ? i + 1 : start;
      const ax = positions[i * size];
      const ay = positions[i * size + 1];
      const bx = positions[next * size];
      const by = positions[next * size + 1];
      const ex = bx - ax;
      const ey = by - ay;
      // the outward normal is sign * (ey, -ex); only walls facing away from the sun cast
      if (sign * (ey * dx - ex * dy) <= 0) continue;
      const base = v / 2;
      out[v++] = ax;
      out[v++] = ay;
      out[v++] = bx;
      out[v++] = by;
      out[v++] = bx + dx;
      out[v++] = by + dy;
      out[v++] = ax + dx;
      out[v++] = ay + dy;
      indices[k++] = base;
      indices[k++] = base + 1;
      indices[k++] = base + 2;
      indices[k++] = base;
      indices[k++] = base + 2;
      indices[k++] = base + 3;
    }
  }
  if (k === 0) return null;
  return { positions: out.slice(0, v), indices: indices.slice(0, k) };
}
