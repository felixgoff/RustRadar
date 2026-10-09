// Directions on screen, for things drawn flat against it.

const DEG = Math.PI / 180;
const METRES_PER_DEGREE = 111_320;

/** The part of a deck.gl viewport this needs: geographic position to screen pixels. */
export interface Projector {
  project(xyz: number[]): number[];
}

/**
 * The angle, anticlockwise in degrees (IconLayer's `getAngle`), that turns a
 * north-pointing icon to face `heading` at `[lon, lat, z]` as the camera sees it.
 *
 * `bearing - heading` is only right where north points straight up the screen:
 * the centre of a flat, untilted view. On the globe, meridians lean and converge
 * away from the centre, and tilting foreshortens directions, so the heading is
 * projected instead: the aircraft and a point `stepM` metres ahead of it, and
 * the icon points from one to the other.
 */
export function screenAngle(viewport: Projector, [lon, lat, z]: number[], heading: number, stepM: number): number | null {
  const h = heading * DEG;
  const ahead = [
    lon + (stepM * Math.sin(h)) / (METRES_PER_DEGREE * Math.max(0.01, Math.cos(lat * DEG))),
    lat + (stepM * Math.cos(h)) / METRES_PER_DEGREE,
    z,
  ];
  const [x0, y0] = viewport.project([lon, lat, z]);
  const [x1, y1] = viewport.project(ahead);
  const dx = x1 - x0;
  const dy = y1 - y0;
  // too short to have a direction (or behind the camera): let the caller fall back
  if (!Number.isFinite(dx) || !Number.isFinite(dy) || Math.hypot(dx, dy) < 1e-3) return null;
  // clockwise from screen-up, where screen y grows downwards; IconLayer turns the other way
  return -Math.atan2(dx, -dy) / DEG;
}
