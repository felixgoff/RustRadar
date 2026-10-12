// Airports' taxiway networks (OpenStreetMap centrelines, via the backend),
// loaded only where ground traffic is watched up close and handed to the
// motion model, which keeps taxiing aircraft on the lines. A few airports at
// most are kept; the least recently wanted is let go.
import { airportTaxiways, isTauri, type Airport } from "./api";
import { motion, type Phase } from "./motion";

/** From this zoom the view is close enough for the taxiways to show. */
export const TAXIWAY_ZOOM = 12;
/** Networks kept at once. */
const MAX_LOADED = 6;
/** An airport whose network failed to load, or has none, is asked again after this long, ms. */
const RETRY_MS = 5 * 60_000;

const GROUND: ReadonlySet<Phase> = new Set<Phase>(["parked", "taxi", "takeoffRoll", "landingRoll"]);

/** Loaded networks by ICAO code, and when each was last wanted (ms). */
const loaded = new Map<string, number>();
const pending = new Set<string>();
const triedAt = new Map<string, number>();

/** Loads the airport's network if it isn't yet (once; the call is cheap to repeat every frame). */
export function wantTaxiways(airport: Airport): void {
  const code = airport.icao?.trim().toUpperCase();
  if (!code || code.length !== 4 || !isTauri()) return;
  const now = Date.now();
  if (loaded.has(code)) {
    loaded.set(code, now);
    return;
  }
  if (pending.has(code) || now - (triedAt.get(code) ?? -Infinity) < RETRY_MS) return;
  pending.add(code);
  triedAt.set(code, now);
  airportTaxiways(code, airport.lat, airport.lon)
    .then((lines) => {
      // none (offline, Overpass busy, or none mapped): asked again later
      if (lines.length === 0) return;
      motion.setTaxiways(code, lines);
      loaded.set(code, Date.now());
      evict();
    })
    .catch(() => {})
    .finally(() => pending.delete(code));
}

/** Loads the network of the airport an aircraft is on the ground at; call per drawn aircraft while zoomed in. */
export function taxiwaysFor(id: number, phase: Phase): void {
  if (!GROUND.has(phase)) return;
  const airport = motion.groundField(id);
  if (airport) wantTaxiways(airport);
}

function evict(): void {
  while (loaded.size > MAX_LOADED) {
    let oldest: string | null = null;
    let oldestMs = Infinity;
    for (const [code, ms] of loaded)
      if (ms < oldestMs) {
        oldestMs = ms;
        oldest = code;
      }
    if (oldest === null) return;
    loaded.delete(oldest);
    motion.setTaxiways(oldest, null);
  }
}
