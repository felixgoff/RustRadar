import type { LiveFlight } from "./api";

export type RGB = [number, number, number];

export type Basemap = "dark" | "satellite";

const DEG = Math.PI / 180;
const EARTH_RADIUS_M = 6_371_000;
const EARTH_CIRCUMFERENCE_M = 40_075_016.686;
const KNOTS_TO_MS = 0.514444;
const FEET_TO_M = 0.3048;
/** Don't extrapolate positions further than this past the last update. */
const MAX_EXTRAPOLATION_S = 90;

/** Every aircraft is white... */
export const AIRCRAFT_COLOR: RGB = [255, 255, 255];
/** ...except the selected one, in the selection amber (`--amber`). */
export const SELECTED_COLOR: RGB = [252, 180, 66];

/** Altitude (feet) → colour stops for the selected flight's trail, low to high. */
export const ALTITUDE_STOPS: [number, RGB][] = [
  [0, [255, 98, 0]],
  [4_000, [255, 196, 0]],
  [8_000, [160, 230, 40]],
  [15_000, [40, 220, 130]],
  [25_000, [0, 200, 230]],
  [35_000, [70, 125, 255]],
  [45_000, [205, 85, 255]],
];

export function altitudeColor(altFt: number): RGB {
  const stops = ALTITUDE_STOPS;
  if (altFt <= stops[0][0]) return stops[0][1];
  for (let i = 1; i < stops.length; i++) {
    const [a1, c1] = stops[i];
    if (altFt <= a1) {
      const [a0, c0] = stops[i - 1];
      const t = (altFt - a0) / (a1 - a0);
      return [0, 1, 2].map((k) => Math.round(c0[k] + (c1[k] - c0[k]) * t)) as RGB;
    }
  }
  return stops[stops.length - 1][1];
}

/** Elevation in metres for rendering, exaggerated so altitude reads on a globe. */
export const elevation = (altFt: number, exaggeration: number) =>
  Math.max(0, altFt) * FEET_TO_M * exaggeration;

/**
 * Exaggeration fades to true scale between zoom 6 and 11: at street level
 * aircraft must sit at their real height next to buildings.
 */
export function effectiveExaggeration(exaggeration: number, zoom: number): number {
  const t = Math.min(1, Math.max(0, (11 - zoom) / 5));
  return 1 + (exaggeration - 1) * t;
}

/** Ground resolution at the view centre. GlobeView matches Web Mercator's scale there. */
export const metersPerPixel = (zoom: number, latitude: number) =>
  (EARTH_CIRCUMFERENCE_M * Math.cos(latitude * DEG)) / (512 * 2 ** zoom);

/**
 * Dead-reckons a flight forward from its last report using ground speed and
 * track, so aircraft move smoothly between feed refreshes.
 */
export function extrapolate(f: LiveFlight, nowMs: number): [number, number] {
  if (f.onGround || f.speed < 40) return [f.lon, f.lat];
  const dt = Math.min(Math.max((nowMs - f.timestampMs) / 1000, 0), MAX_EXTRAPOLATION_S);
  const d = (f.speed * KNOTS_TO_MS * dt) / EARTH_RADIUS_M; // angular distance
  const lat1 = f.lat * DEG;
  const lon1 = f.lon * DEG;
  const brg = f.track * DEG;
  const lat2 = Math.asin(Math.sin(lat1) * Math.cos(d) + Math.cos(lat1) * Math.sin(d) * Math.cos(brg));
  const lon2 =
    lon1 + Math.atan2(Math.sin(brg) * Math.sin(d) * Math.cos(lat1), Math.cos(d) - Math.sin(lat1) * Math.sin(lat2));
  return [((lon2 / DEG + 540) % 360) - 180, lat2 / DEG];
}

/** Cosine of the great-circle angle between two points. */
export function cosAngle(lon1: number, lat1: number, lon2: number, lat2: number): number {
  const a = lat1 * DEG;
  const b = lat2 * DEG;
  return Math.sin(a) * Math.sin(b) + Math.cos(a) * Math.cos(b) * Math.cos((lon1 - lon2) * DEG);
}

/**
 * Angular radius (degrees) around the view centre that covers a screen of the
 * given size, with some margin. At zoom z the globe is 512·2^z px around;
 * tilting the camera pulls the horizon much further out.
 */
export function visibleRadius(zoom: number, pitch: number, widthPx: number, heightPx: number): number {
  const halfDiagonal = Math.hypot(widthPx, heightPx) / 2;
  const degreesPerPixel = 360 / (512 * 2 ** zoom);
  const tilt = 1 + 4 * Math.sin(pitch * DEG);
  // slightly past the horizon so elevated aircraft on the limb stay visible
  return Math.min(100, 1.3 * halfDiagonal * degreesPerPixel * tilt);
}

/**
 * The area worth fetching for a view, or `null` when most of the globe is in
 * sight and the whole world should be fetched.
 */
export function viewArea(
  view: { longitude: number; latitude: number; zoom: number; pitch: number },
  widthPx: number,
  heightPx: number,
): { south: number; north: number; west: number; east: number } | null {
  const r = visibleRadius(view.zoom, view.pitch, widthPx, heightPx);
  if (r >= 50) return null;
  const south = Math.max(-90, view.latitude - r);
  const north = Math.min(90, view.latitude + r);
  const widest = Math.max(Math.abs(south), Math.abs(north));
  const dLon = r / Math.max(0.05, Math.cos(widest * DEG));
  if (dLon >= 180) return { south, north, west: -180, east: 180 };
  const wrap = (lon: number) => ((lon + 540) % 360) - 180;
  return { south, north, west: wrap(view.longitude - dLon), east: wrap(view.longitude + dLon) };
}

/** Shortest-path longitude for animating from `from` to `to`. */
export function unwrapLongitude(from: number, to: number): number {
  let lon = to;
  while (lon - from > 180) lon -= 360;
  while (lon - from < -180) lon += 360;
  return lon;
}
