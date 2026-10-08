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

/** Latitudes on a meridian where the sun is below `altitude`, or null. */
function belowOn(lon: number, altitude: number, sun: { lat: number; lon: number }): [number, number] | null {
  // along a meridian the sun's altitude falls steadily toward one end, so
  // the night part is a single interval
  let lo: number | null = null;
  let hi = 0;
  for (let lat = -POLE; lat <= POLE; lat += 1) {
    if (sunAltitude(lat, lon, sun) < altitude) {
      lo ??= lat;
      hi = lat;
    }
  }
  return lo === null ? null : [lo, hi];
}

/**
 * The region where the sun is below `altitude` degrees, as one quad per
 * 2° meridian strip. (Walking from a pole fails when the pole itself is in
 * shallower twilight, as it is near the equinoxes.)
 */
function nightBelow(altitude: number, sun: { lat: number; lon: number }): LonLat[][] {
  const STEP = 2;
  const quads: LonLat[][] = [];
  let previous = belowOn(-180, altitude, sun);
  for (let lon = -180; lon < 180; lon += STEP) {
    const next = belowOn(lon + STEP, altitude, sun);
    if (previous || next) {
      // where the band ends between two meridians, taper it to a point
      const a = previous ?? [(next![0] + next![1]) / 2, (next![0] + next![1]) / 2];
      const b = next ?? [(a[0] + a[1]) / 2, (a[0] + a[1]) / 2];
      quads.push([
        [lon, a[0]],
        [lon + STEP, b[0]],
        [lon + STEP, b[1]],
        [lon, a[1]],
      ]);
    }
    previous = next;
  }
  return quads;
}

/** Sunset, then civil, nautical and astronomical twilight. */
export const TWILIGHT_STEPS = [0, -6, -12, -18];

/** Quads for every band, overlapping so the night deepens toward its middle. */
export function nightPolygons(time: number): LonLat[][] {
  const sun = subsolarPoint(time);
  return TWILIGHT_STEPS.flatMap((altitude) => nightBelow(altitude, sun));
}
