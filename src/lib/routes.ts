// Route geometry for the selected flight, and AIRAC cycle arithmetic.
import type { Airport, FlightDetails, FlightPlan, LiveFlight } from "./api";
import { reference } from "./reference.svelte";

export type LonLat = [number, number];

const DEG = Math.PI / 180;

/** Points along the great circle from `a` to `b`, about every `stepKm`. */
export function greatCircle(a: LonLat, b: LonLat, stepKm = 80): LonLat[] {
  const [lon1, lat1] = [a[0] * DEG, a[1] * DEG];
  const [lon2, lat2] = [b[0] * DEG, b[1] * DEG];
  const d =
    2 *
    Math.asin(
      Math.sqrt(
        Math.sin((lat2 - lat1) / 2) ** 2 + Math.cos(lat1) * Math.cos(lat2) * Math.sin((lon2 - lon1) / 2) ** 2,
      ),
    );
  if (d < 1e-9) return [a, b];
  const n = Math.min(256, Math.max(2, Math.ceil((d * 6371) / stepKm)));
  const points: LonLat[] = [];
  let previousLon = a[0];
  for (let i = 0; i <= n; i++) {
    const f = i / n;
    const A = Math.sin((1 - f) * d) / Math.sin(d);
    const B = Math.sin(f * d) / Math.sin(d);
    const x = A * Math.cos(lat1) * Math.cos(lon1) + B * Math.cos(lat2) * Math.cos(lon2);
    const y = A * Math.cos(lat1) * Math.sin(lon1) + B * Math.cos(lat2) * Math.sin(lon2);
    const z = A * Math.sin(lat1) + B * Math.sin(lat2);
    let lon = Math.atan2(y, x) / DEG;
    // keep the path continuous across the antimeridian
    while (lon - previousLon > 180) lon -= 360;
    while (lon - previousLon < -180) lon += 360;
    previousLon = lon;
    points.push([lon, Math.atan2(z, Math.hypot(x, y)) / DEG]);
  }
  return points;
}

/** Great-circle distance in km. */
export function distanceKm(a: LonLat, b: LonLat): number {
  const [lon1, lat1, lon2, lat2] = [a[0] * DEG, a[1] * DEG, b[0] * DEG, b[1] * DEG];
  const h = Math.sin((lat2 - lat1) / 2) ** 2 + Math.cos(lat1) * Math.cos(lat2) * Math.sin((lon2 - lon1) / 2) ** 2;
  return 2 * 6371 * Math.asin(Math.sqrt(h));
}

/**
 * Flight plan waypoints arrive as integers of undocumented scale (the proto
 * says degrees, which would be far too coarse for waypoints). Pick the scale
 * that puts the route's start nearest the departure airport. Without one,
 * take the smallest divisor that gives valid coordinates: dividing by too
 * much also gives valid ones, squashed toward 0°, 0°.
 */
export function decodeWaypoints(plan: FlightPlan, departure: Airport | undefined): LonLat[] {
  if (!plan.waypoints.length) return [];
  const candidates = [1, 1e4, 1e5, 1e6, 1e7]
    .map((scale) => plan.waypoints.map(([lat, lon]): LonLat => [lon / scale, lat / scale]))
    .filter((points) => points.every(([lon, lat]) => Math.abs(lat) <= 90 && Math.abs(lon) <= 180));
  if (!candidates.length) return [];
  if (!departure) return candidates[0];
  const home: LonLat = [departure.lon, departure.lat];
  return candidates.reduce((best, points) =>
    distanceKm(points[0], home) < distanceKm(best[0], home) ? points : best,
  );
}

export interface Route {
  origin?: Airport;
  destination?: Airport;
  /** `filed`: the subscriber flight plan; `estimated`: great circles only. */
  kind: "filed" | "estimated" | "none";
  /** The filed route, origin to destination. */
  filed: LonLat[];
  /** Great circle from the origin to where the recorded track begins. */
  before: LonLat[];
  /** Great circle from the aircraft to its destination. */
  ahead: LonLat[];
}

export function routeFor(flight: LiveFlight, details: FlightDetails | null): Route {
  const schedule = details?.schedule;
  const plan = details?.flight_plan;
  const origin =
    reference.airportById(schedule?.origin_id) ?? reference.airport(flight.origin) ?? reference.airport(plan?.departure);
  const destination =
    reference.airportById(schedule?.diverted_id || schedule?.destination_id) ??
    reference.airport(flight.destination) ??
    reference.airport(plan?.destination);
  const here: LonLat = [flight.lon, flight.lat];

  const filed = plan ? decodeWaypoints(plan, origin) : [];
  if (filed.length > 1) return { origin, destination, kind: "filed", filed, before: [], ahead: [] };

  const trailStart = details?.trail.find((p) => p.latitude || p.longitude);
  const start: LonLat = trailStart ? [trailStart.longitude, trailStart.latitude] : here;
  const before =
    origin && distanceKm([origin.lon, origin.lat], start) > 15 ? greatCircle([origin.lon, origin.lat], start) : [];
  const ahead = destination && !flight.onGround ? greatCircle(here, [destination.lon, destination.lat]) : [];
  const kind = origin || destination ? "estimated" : "none";
  return { origin, destination, kind, filed: [], before, ahead };
}

/** AIRAC cycles change every 28 days; cycle 2401 became effective on 2024-01-25. */
const AIRAC_EPOCH = Date.UTC(2024, 0, 25);
const AIRAC_PERIOD = 28 * 86_400_000;

export interface AiracCycle {
  /** e.g. `2610`: year and the cycle's number within it. */
  ident: string;
  effective: Date;
  expires: Date;
}

export function airacCycle(now = Date.now()): AiracCycle {
  const n = Math.floor((now - AIRAC_EPOCH) / AIRAC_PERIOD);
  const start = AIRAC_EPOCH + n * AIRAC_PERIOD;
  const year = new Date(start).getUTCFullYear();
  const firstOfYear = Math.ceil((Date.UTC(year, 0, 1) - AIRAC_EPOCH) / AIRAC_PERIOD);
  const ident = `${String(year % 100).padStart(2, "0")}${String(n - firstOfYear + 1).padStart(2, "0")}`;
  return { ident, effective: new Date(start), expires: new Date(start + AIRAC_PERIOD) };
}
