// Aircraft shadows on the ground, cast by the real sun.
//
// The map has no terrain: the ground is the plane z = 0 of each aircraft's
// local frame. A shadow is the aircraft's own mesh squashed onto that plane
// along the sun's rays (an oblique projection), so it takes the aircraft's
// heading, bank and pitch, and lands `height / tan(sun altitude)` away from
// the point straight below it.

import { subsolarPoint } from "./daynight";

const DEG = Math.PI / 180;
const FEET_TO_M = 0.3048;

/** The sun's direction from the Earth's centre (x through 0°E on the equator, z north), unit length. */
export function sunDirection(time: number): [number, number, number] {
  const { lat, lon } = subsolarPoint(time);
  const cosLat = Math.cos(lat * DEG);
  return [cosLat * Math.cos(lon * DEG), cosLat * Math.sin(lon * DEG), Math.sin(lat * DEG)];
}

/**
 * The direction toward the sun at a point, as [east, north, up] (unit length;
 * `up` is the sine of the sun's altitude, negative at night). Written into
 * `out` so a frame's worth of aircraft allocates nothing.
 */
export function localSun(
  sun: readonly number[],
  lat: number,
  lon: number,
  out: number[] = [0, 0, 0],
): number[] {
  const sinLat = Math.sin(lat * DEG);
  const cosLat = Math.cos(lat * DEG);
  const sinLon = Math.sin(lon * DEG);
  const cosLon = Math.cos(lon * DEG);
  const [x, y, z] = sun;
  out[0] = -sinLon * x + cosLon * y;
  out[1] = -sinLat * cosLon * x - sinLat * sinLon * y + cosLat * z;
  out[2] = cosLat * cosLon * x + cosLat * sinLon * y + sinLat * z;
  return out;
}

/**
 * Model matrix (column-major 4x4, `SimpleMeshLayer`'s `getTransformMatrix`)
 * that draws a mesh's shadow on the ground.
 *
 * `orientation` and `scale` are what the aircraft itself is drawn with
 * (deck.gl's `[pitch, yaw, roll]` in degrees, and per-axis scale); `toSun`
 * points toward the sun in the same local frame the mesh is drawn in, and
 * `height` is how far (metres, as drawn) the mesh's origin is above the
 * map. Each vertex `p` of the oriented mesh, at `(0, 0, height) + p`, moves
 * along the sun's rays to the ground plane `z = groundZ`. The sun must be
 * above the horizon.
 */
export function shadowMatrix(
  orientation: readonly number[],
  scale: readonly number[],
  toSun: readonly number[],
  height: number,
  out: number[] = new Array(16).fill(0),
  groundZ = 0,
): number[] {
  // deck.gl's rotation (@deck.gl/mesh-layers utils/matrix.ts), columns scaled
  const sp = Math.sin(orientation[0] * DEG);
  const cp = Math.cos(orientation[0] * DEG);
  const sw = Math.sin(orientation[1] * DEG);
  const cw = Math.cos(orientation[1] * DEG);
  const sr = Math.sin(orientation[2] * DEG);
  const cr = Math.cos(orientation[2] * DEG);
  const [sx, sy, sz] = scale;
  const m00 = sx * cw * cp;
  const m10 = sx * sw * cp;
  const m20 = sx * -sp;
  const m01 = sy * (-sw * cr + cw * sp * sr);
  const m11 = sy * (cw * cr + sw * sp * sr);
  const m21 = sy * cp * sr;
  const m02 = sz * (sw * sr + cw * sp * cr);
  const m12 = sz * (-cw * sr + sw * sp * cr);
  const m22 = sz * cp * cr;
  // ground point reached from (x, y, z) along the ray: (x - kx (z - g), y - ky (z - g), g)
  const kx = toSun[0] / toSun[2];
  const ky = toSun[1] / toSun[2];
  out[0] = m00 - kx * m20;
  out[1] = m10 - ky * m20;
  out[2] = 0;
  out[3] = 0;
  out[4] = m01 - kx * m21;
  out[5] = m11 - ky * m21;
  out[6] = 0;
  out[7] = 0;
  out[8] = m02 - kx * m22;
  out[9] = m12 - ky * m22;
  out[10] = 0;
  out[11] = 0;
  out[12] = -kx * (height - groundZ);
  out[13] = -ky * (height - groundZ);
  out[14] = groundZ;
  out[15] = 1;
  return out;
}

/** Darkest a shadow gets: on the ground, under a high sun. */
export const SHADOW_MAX_ALPHA = 0.5;
/** Fainter than this, a shadow is not drawn at all. */
export const SHADOW_MIN_ALPHA = 0.05;
/** The sun's angular diameter, radians: it sets how fast a shadow blurs with distance. */
const SUN_DIAMETER = 0.53 * DEG;
/** An airframe's typical member (fuselage, wing chord) against its largest dimension. */
const MEMBER_FRACTION = 0.1;
/** Below this sun altitude (degrees) shadows fade: the light is dim, reddened and diffuse. */
const LOW_SUN_DEG = 6;

/**
 * Opacity (0..1) of the shadow of an aircraft `sizeM` long at `altFt` above
 * the ground, with the sun's altitude sine `sinSun`.
 *
 * The sun is a disc, not a point: past `width / 0.53°` along the ray, an
 * object of that width casts no umbra, only a blur that pales as it spreads.
 * An airframe's fuselage and wing chord are about a tenth of its length, so
 * the shadow is crisp up to a slant distance of about 11 lengths (an A320:
 * ~400 m) and then loses contrast: in proportion to the distance for a long
 * thin shape, with its square for a compact one; an airframe is in between,
 * so the power is 1.5. Under a high sun an A320's shadow is faint by
 * 3,000 ft and gone by about 5,500 ft, a light aircraft's by about 2,000 ft,
 * a 777's by about 9,000 ft; a low sun, whose rays are longer, fades them
 * all sooner. None at night.
 */
export function shadowOpacity(altFt: number, sizeM: number, sinSun: number): number {
  if (!(sinSun > 0)) return 0;
  const sunDeg = Math.asin(Math.min(1, sinSun)) / DEG;
  const light = Math.min(1, sunDeg / LOW_SUN_DEG);
  const slant = (Math.max(0, altFt) * FEET_TO_M) / sinSun;
  const crisp = (MEMBER_FRACTION * sizeM) / SUN_DIAMETER;
  const contrast = slant <= crisp ? 1 : (crisp / slant) ** 1.5;
  const alpha = SHADOW_MAX_ALPHA * contrast * light;
  return alpha < SHADOW_MIN_ALPHA ? 0 : alpha;
}
