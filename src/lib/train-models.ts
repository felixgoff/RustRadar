/**
 * Trains close in: each car a 3D body, a unit box along +y (the way it
 * runs) scaled to the car by SimpleMeshLayer: underframe, body, a dark band
 * of windows and a lighter roof; the leading car with a sloped, glazed nose.
 * The vertex colours are multiplied by the train's colour (SimpleMeshLayer),
 * so the body (1) takes it, the windows and underframe stay dark.
 */
import { Geometry } from "@luma.gl/engine";

/** A car's size as drawn, metres. */
export const CAR_WIDTH_M = 3.1;
export const CAR_HEIGHT_M = 4.1;

type Arrays = { positions: number[]; normals: number[]; colors: number[] };

/** An axis-aligned box `[x0, x1] × [y0, y1] × [z0, z1]`, each face a shade (sides, ends, top and bottom). */
function box(
  a: Arrays,
  [x0, x1]: [number, number],
  [y0, y1]: [number, number],
  [z0, z1]: [number, number],
  shade: { side: number; end?: number; top?: number },
) {
  const face = (corners: number[][], normal: number[], s: number) => {
    for (const i of [0, 1, 2, 0, 2, 3]) {
      a.positions.push(...corners[i]);
      a.normals.push(...normal);
      a.colors.push(s, s, s);
    }
  };
  const end = shade.end ?? shade.side;
  const top = shade.top ?? shade.side;
  face([[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]], [1, 0, 0], shade.side);
  face([[x0, y1, z0], [x0, y0, z0], [x0, y0, z1], [x0, y1, z1]], [-1, 0, 0], shade.side);
  face([[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]].reverse(), [0, 1, 0], end);
  face([[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]], [0, -1, 0], end);
  face([[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]], [0, 0, 1], top);
  face([[x0, y1, z0], [x1, y1, z0], [x1, y0, z0], [x0, y0, z0]], [0, 0, -1], 0.2);
}

/** A car's triangles, unit sized (x across, y along with the front at +0.5, z up from 0 to 1). */
export function carArrays(lead: boolean): Arrays {
  const a: Arrays = { positions: [], normals: [], colors: [] };
  const front = lead ? 0.42 : 0.5;
  // bogies and underframe, set in a little
  box(a, [-0.4, 0.4], [-0.47, 0.47], [0, 0.2], { side: 0.22 });
  // the body: below the windows, the windows, above them
  box(a, [-0.5, 0.5], [-0.5, front], [0.2, 0.5], { side: 1, end: 0.85 });
  box(a, [-0.5, 0.5], [-0.5, front], [0.5, 0.72], { side: 0.16, end: 0.3 });
  box(a, [-0.5, 0.5], [-0.5, front], [0.72, 0.88], { side: 1, end: 0.85, top: 0.9 });
  // the roof, a little narrower and lighter
  box(a, [-0.42, 0.42], [-0.48, front - 0.02], [0.88, 1], { side: 0.8, top: 0.75 });
  if (lead) {
    // the nose: down from the roof line to the buffer beam, glazed above
    const [y0, y1] = [front, 0.5];
    const slope = (z0: number, z1: number, s: number) => {
      const yAt = (z: number) => y1 - (y1 - y0) * ((z - 0.2) / 0.68);
      const n = [0, 0.68, y1 - y0];
      const len = Math.hypot(n[1], n[2]);
      const corners = [
        [-0.5, yAt(z0), z0],
        [0.5, yAt(z0), z0],
        [0.5, yAt(z1), z1],
        [-0.5, yAt(z1), z1],
      ];
      for (const i of [0, 1, 2, 0, 2, 3]) {
        a.positions.push(...corners[i]);
        a.normals.push(0, n[1] / len, n[2] / len);
        a.colors.push(s, s, s);
      }
    };
    slope(0.2, 0.5, 1);
    slope(0.5, 0.8, 0.12);
    slope(0.8, 0.88, 1);
    // its sides, under the slope: closed with a box as long as the nose's foot
    box(a, [-0.5, 0.5], [y0, y1], [0.2, 0.3], { side: 1, end: 1 });
  }
  return a;
}

let meshes: { coach: Geometry; cab: Geometry } | null = null;
/** The two cars' meshes, built once. */
export function carMeshes(): { coach: Geometry; cab: Geometry } {
  if (meshes) return meshes;
  const geometry = (a: Arrays) =>
    new Geometry({
      topology: "triangle-list",
      attributes: {
        positions: { size: 3, value: new Float32Array(a.positions) },
        normals: { size: 3, value: new Float32Array(a.normals) },
        colors: { size: 3, value: new Float32Array(a.colors) },
      },
    });
  return (meshes = { coach: geometry(carArrays(false)), cab: geometry(carArrays(true)) });
}

/** A car laid on the map: its middle `[lon, lat]`, the way it points (degrees clockwise from north), and its length. */
export type CarPlace = { at: [number, number]; heading: number; length: number };

/**
 * Where each car of `lines` (`[lon, lat]` lines, back to front, as
 * `carLines` makes them) lies: halfway between its ends, pointing from its
 * back to its front.
 */
export function carPlaces(lines: [number, number][][]): CarPlace[] {
  return lines.flatMap((line) => {
    if (line.length < 2) return [];
    const [back, front] = [line[0], line[line.length - 1]];
    const k = Math.cos((((back[1] + front[1]) / 2) * Math.PI) / 180);
    const [east, north] = [(front[0] - back[0]) * k * 111_320, (front[1] - back[1]) * 111_320];
    const length = Math.hypot(east, north);
    if (length < 0.5) return [];
    const heading = ((Math.atan2(east, north) * 180) / Math.PI + 360) % 360;
    return [{ at: [(back[0] + front[0]) / 2, (back[1] + front[1]) / 2] as [number, number], heading, length }];
  });
}
