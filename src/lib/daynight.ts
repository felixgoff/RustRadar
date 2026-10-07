// Night shading: where the sun is below the horizon (and below the civil,
// nautical and astronomical twilight altitudes) at a given moment.
import type { LonLat } from "./routes";

const DEG = Math.PI / 180;
/** Web Mercator's latitude limit; beyond it the flat map has no tiles. */
const POLE = 85;

/** Subsolar point (latitude = declination), low-precision solar almanac. */
export function subsolarPoint(time: number): { lat: number; lon: number } {
  const d = time / 86_400_000 - 10_957.5; // days since J2000.0
  const g = (357.529 + 0.98560028 * d) * DEG;
  const q = 280.459 + 0.98564736 * d;
  const L = (q + 1.915 * Math.sin(g) + 0.02 * Math.sin(2 * g)) * DEG;
  const e = (23.439 - 0.00000036 * d) * DEG;
  const rightAscension = Math.atan2(Math.cos(e) * Math.sin(L), Math.cos(L)) / DEG;
  const declination = Math.asin(Math.sin(e) * Math.sin(L)) / DEG;
  // equation of time, in degrees of rotation
  const eqTime = ((((q - rightAscension) % 360) + 540) % 360) - 180;
  const utcHours = (time / 3_600_000) % 24;
  const lon = ((((-15 * (utcHours - 12) - eqTime) % 360) + 540) % 360) - 180;
  return { lat: declination, lon };
}

/** Sun altitude (degrees) at a point. */
function sunAltitude(lat: number, lon: number, sun: { lat: number; lon: number }): number {
  const cosZenith =
    Math.sin(lat * DEG) * Math.sin(sun.lat * DEG) +
    Math.cos(lat * DEG) * Math.cos(sun.lat * DEG) * Math.cos((lon - sun.lon) * DEG);
  return Math.asin(Math.max(-1, Math.min(1, cosZenith))) / DEG;
}

/**
 * The region where the sun is below `altitude` degrees, as a polygon from
 * the night-side pole to the boundary on each meridian.
 */
function nightBelow(altitude: number, sun: { lat: number; lon: number }): LonLat[] {
  const nightPole = sun.lat >= 0 ? -POLE : POLE;
  const step = nightPole < 0 ? 0.5 : -0.5;
  const boundary: LonLat[] = [];
  for (let lon = -180; lon <= 180; lon += 2) {
    let lat = nightPole;
    // walk away from the night pole until the sun rises above `altitude`
    while (Math.abs(lat) <= POLE && sunAltitude(lat, lon, sun) < altitude) lat += step;
    boundary.push([lon, Math.max(-POLE, Math.min(POLE, lat))]);
  }
  return [...boundary, [180, nightPole], [-180, nightPole]];
}

/** Sunset, then civil, nautical and astronomical twilight. */
export const TWILIGHT_STEPS = [0, -6, -12, -18];

export function nightPolygons(time: number): LonLat[][] {
  const sun = subsolarPoint(time);
  return TWILIGHT_STEPS.map((altitude) => nightBelow(altitude, sun));
}
