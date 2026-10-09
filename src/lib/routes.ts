// Routes for the selected flight: the filed flight plan when Flightradar24
// shares it, otherwise an estimate shaped by `flightpath.ts`; and AIRAC cycle
// arithmetic.
import type { Airport, FlightDetails, FlightPlan, LiveFlight, OpenSkyPoint, TrailPoint } from "./api";
import { distanceKm, estimateRoute, type LonLat, type PathPoint, type RunwayGuess } from "./flightpath";
import { reference } from "./reference.svelte";

export { distanceKm, greatCircle, trimToPosition, type LonLat, type PathPoint, type RunwayGuess } from "./flightpath";

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

/**
 * The flown track, with OpenSky's earlier part in front of Flightradar24's
 * when OpenSky saw more of the flight (Flightradar24's own points win where
 * both have them: they are newer and carry speeds).
 */
export function mergeTrail(
  trail: TrailPoint[],
  opensky: OpenSkyPoint[],
): { trail: TrailPoint[]; added: number } {
  const first = trail.find((p) => p.timestamp > 0)?.timestamp ?? Infinity;
  const earlier = opensky.filter((p) => p.timestamp < first - 30);
  if (earlier.length < 2) return { trail, added: 0 };
  const points: TrailPoint[] = earlier.map((p, i) => {
    // OpenSky has no speed: take it from the move to the next point
    const next = earlier[i + 1] ?? trail[0];
    let speed = 0;
    if (next && next.timestamp > p.timestamp) {
      const km = distanceKm([p.longitude, p.latitude], [next.longitude, next.latitude]);
      speed = Math.min(700, Math.round((km / 1.852 / (next.timestamp - p.timestamp)) * 3600));
    }
    return {
      timestamp: p.timestamp,
      latitude: p.latitude,
      longitude: p.longitude,
      altitude: p.altitude,
      ground_speed: speed,
      track: p.track,
      vertical_speed: 0,
      source: -1,
    };
  });
  return { trail: [...points, ...trail], added: points.length };
}

export interface Route {
  origin?: Airport;
  destination?: Airport;
  /** `filed`: the subscriber flight plan; `estimated`: the app's own guess. */
  kind: "filed" | "estimated" | "none";
  /** The filed route, origin to destination. */
  filed: LonLat[];
  /** Estimated: from the take-off roll to where the recorded track begins, with heights (ft MSL). */
  before: PathPoint[];
  /** Estimated: from the aircraft to touchdown, with heights (ft MSL). */
  ahead: PathPoint[];
  /** The runway `before` departs from, when runways are known. */
  departure: RunwayGuess | null;
  /** The runway `ahead` lands on, when runways are known. */
  arrival: RunwayGuess | null;
}

/**
 * The selected flight's route. `traffic` (the other flights on the globe) is
 * read for which runways are in use at the two airports.
 */
export function routeFor(flight: LiveFlight, details: FlightDetails | null, traffic: readonly LiveFlight[] = []): Route {
  const schedule = details?.schedule;
  const plan = details?.flight_plan;
  const origin =
    reference.airportById(schedule?.origin_id) ?? reference.airport(flight.origin) ?? reference.airport(plan?.departure);
  const destination =
    reference.airportById(schedule?.diverted_id || schedule?.destination_id) ??
    reference.airport(flight.destination) ??
    reference.airport(plan?.destination);

  const filed = plan ? decodeWaypoints(plan, origin) : [];
  if (filed.length > 1) {
    return { origin, destination, kind: "filed", filed, before: [], ahead: [], departure: null, arrival: null };
  }
  const trailStart = details?.trail.find((p) => p.latitude || p.longitude);
  return {
    origin,
    destination,
    filed: [],
    ...estimateRoute({
      flight,
      trailStart,
      origin,
      destination,
      originRunways: origin ? (reference.runways(origin) ?? []) : [],
      destinationRunways: destination ? (reference.runways(destination) ?? []) : [],
      traffic,
    }),
  };
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
