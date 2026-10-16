/**
 * Trees for the realistic map, as meshes for SimpleMeshLayer: a broadleaf (a
 * trunk under a lumpy, rounded crown) and a conifer (a trunk in stacked
 * cones), each unit tall, in two levels of detail. The crowns' vertex colours
 * are a shade (darker underneath, where the crown shades itself) that the
 * tree's own colour multiplies.
 */
import { Geometry } from "@luma.gl/engine";

type Arrays = { positions: number[]; normals: number[]; colors: number[] };
type Vec3 = [number, number, number];

const normalize = (v: Vec3): Vec3 => {
  const l = Math.hypot(v[0], v[1], v[2]) || 1;
  return [v[0] / l, v[1] / l, v[2] / l];
};

function push(a: Arrays, p: Vec3, n: Vec3, shade: number) {
  a.positions.push(...p);
  a.normals.push(...n);
  a.colors.push(shade, shade, shade);
}

/** An icosahedron's corners and faces (counter-clockwise seen from outside). */
function icosahedron(): { vertices: Vec3[]; faces: [number, number, number][] } {
  const t = (1 + Math.sqrt(5)) / 2;
  const vertices = (
    [
      [-1, t, 0], [1, t, 0], [-1, -t, 0], [1, -t, 0],
      [0, -1, t], [0, 1, t], [0, -1, -t], [0, 1, -t],
      [t, 0, -1], [t, 0, 1], [-t, 0, -1], [-t, 0, 1],
    ] as Vec3[]
  ).map(normalize);
  const faces: [number, number, number][] = [
    [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11],
    [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
    [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9],
    [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
  ];
  return { vertices, faces };
}

/** The icosahedron with each face split in four, its new corners pushed out onto the sphere. */
function icosphere(subdivisions: number) {
  let { vertices, faces } = icosahedron();
  for (let s = 0; s < subdivisions; s++) {
    const middles = new Map<string, number>();
    const middle = (a: number, b: number) => {
      const key = a < b ? `${a},${b}` : `${b},${a}`;
      let i = middles.get(key);
      if (i === undefined) {
        const [p, q] = [vertices[a], vertices[b]];
        i = vertices.push(normalize([(p[0] + q[0]) / 2, (p[1] + q[1]) / 2, (p[2] + q[2]) / 2])) - 1;
        middles.set(key, i);
      }
      return i;
    };
    faces = faces.flatMap(([a, b, c]) => {
      const [ab, bc, ca] = [middle(a, b), middle(b, c), middle(c, a)];
      return [
        [a, ab, ca],
        [b, bc, ab],
        [c, ca, bc],
        [ab, bc, ca],
      ] as [number, number, number][];
    });
  }
  return { vertices, faces };
}

/** A fixed bumpiness for each direction, so a crown is lumpy rather than a ball, the same every time. */
function lump(v: Vec3): number {
  const s = Math.sin(v[0] * 12.9898 + v[1] * 78.233 + v[2] * 37.719) * 43758.5453;
  return s - Math.floor(s);
}

/**
 * A crown: an ellipsoid centred at height `z` with half-width `a` and
 * half-height `c`, lumpy, shaded darker underneath.
 */
function crown(out: Arrays, subdivisions: number, z: number, a: number, c: number, bumps: number) {
  const { vertices, faces } = icosphere(subdivisions);
  const placed = vertices.map((v) => {
    const k = 1 + bumps * (lump(v) - 0.5);
    return [v[0] * a * k, v[1] * a * k, z + v[2] * c * k] as Vec3;
  });
  // smooth normals of the ellipsoid (from the sphere's: scaled by the inverse of its axes)
  const normals = vertices.map((v) => normalize([v[0] / a, v[1] / a, v[2] / c]));
  const shade = (p: Vec3) => 0.62 + 0.38 * Math.min(1, Math.max(0, (p[2] - (z - c)) / (2 * c)));
  for (const face of faces) for (const i of face) push(out, placed[i], normals[i], shade(placed[i]));
}

/** A cone (or a cylinder, with `r1`): `sides` around, from height `z0` with radius `r0` to `z1` with `r1`, open at both ends. */
function cone(out: Arrays, sides: number, z0: number, z1: number, r0: number, r1: number, shade: [number, number]) {
  const h = z1 - z0;
  for (let i = 0; i < sides; i++) {
    const [a0, a1] = [(i / sides) * 2 * Math.PI, ((i + 1) / sides) * 2 * Math.PI];
    const ring = (a: number, r: number, z: number): Vec3 => [Math.cos(a) * r, Math.sin(a) * r, z];
    // the side's normal: out, and up by how much the cone narrows
    const normal = (a: number): Vec3 => normalize([Math.cos(a) * h, Math.sin(a) * h, r0 - r1]);
    const [p00, p10, p01, p11] = [ring(a0, r0, z0), ring(a1, r0, z0), ring(a0, r1, z1), ring(a1, r1, z1)];
    const [n0, n1] = [normal(a0), normal(a1)];
    push(out, p00, n0, shade[0]);
    push(out, p10, n1, shade[0]);
    push(out, p11, n1, shade[1]);
    if (r1 > 0) {
      push(out, p00, n0, shade[0]);
      push(out, p11, n1, shade[1]);
      push(out, p01, n0, shade[1]);
    }
  }
}

const TRUNK: [number, number] = [0.45, 0.5];

/** A broadleaf, 1 tall and about 0.6 across. `detail` 1: a rounded crown on a trunk; 0: a faceted one alone. */
export function broadleafArrays(detail: 0 | 1): Arrays {
  const a: Arrays = { positions: [], normals: [], colors: [] };
  if (detail) cone(a, 6, 0, 0.42, 0.035, 0.028, TRUNK);
  crown(a, detail, 0.64, 0.3, 0.36, detail ? 0.22 : 0.1);
  return a;
}

/** A conifer, 1 tall and about 0.46 across at its foot: three stacked cones (one, with `detail` 0). */
export function coniferArrays(detail: 0 | 1): Arrays {
  const a: Arrays = { positions: [], normals: [], colors: [] };
  if (detail) {
    cone(a, 5, 0, 0.3, 0.025, 0.02, TRUNK);
    cone(a, 8, 0.14, 0.62, 0.23, 0, [0.6, 0.95]);
    cone(a, 8, 0.38, 0.84, 0.17, 0, [0.62, 0.97]);
    cone(a, 8, 0.6, 1, 0.11, 0, [0.66, 1]);
  } else {
    cone(a, 6, 0.12, 1, 0.22, 0, [0.62, 1]);
  }
  return a;
}

const geometry = (a: Arrays) =>
  new Geometry({
    topology: "triangle-list",
    attributes: {
      positions: { size: 3, value: new Float32Array(a.positions) },
      normals: { size: 3, value: new Float32Array(a.normals) },
      colors: { size: 3, value: new Float32Array(a.colors) },
    },
  });

type TreeMeshes = { broadleaf: [Geometry, Geometry]; conifer: [Geometry, Geometry] };
let meshes: TreeMeshes | null = null;
/** The trees' meshes, built once: `[low detail, high detail]` of each. */
export function treeMeshes(): TreeMeshes {
  return (meshes ??= {
    broadleaf: [geometry(broadleafArrays(0)), geometry(broadleafArrays(1))],
    conifer: [geometry(coniferArrays(0)), geometry(coniferArrays(1))],
  });
}
