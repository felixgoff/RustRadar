// Aircraft filters and persisted settings.
import type { LiveFlight } from "./api";
import type { Basemap } from "./geo";
import { airlineIcao } from "./reference.svelte";

/** Flightradar24's aircraft categories (`Service`), filtered server-side. */
export const CATEGORIES: { id: number; label: string }[] = [
  { id: 0, label: "Passenger" },
  { id: 1, label: "Cargo" },
  { id: 2, label: "Military and government" },
  { id: 3, label: "Business jets" },
  { id: 4, label: "General aviation" },
  { id: 5, label: "Helicopters" },
  { id: 6, label: "Lighter than air" },
  { id: 7, label: "Gliders" },
  { id: 8, label: "Drones" },
  { id: 9, label: "Ground vehicles" },
  { id: 10, label: "Other" },
  { id: 11, label: "Uncategorised" },
];

/** The altitude slider's top; at this value there is no upper limit. */
export const MAX_ALTITUDE = 50_000;

export interface Filters {
  /** Category ids to show; all of them means no filter. */
  categories: number[];
  /** Feet, inclusive; the top value means "and above". */
  altitude: [number, number];
  airborne: boolean;
  onGround: boolean;
  /** Airline ICAO designators. */
  airlines: string[];
  /** Type designator prefixes, e.g. `A38` or `B77W`. */
  types: string[];
  /** Airport IATA codes; a flight matches if it departs from or flies to one. */
  airports: string[];
  /** Matches callsign, flight number or registration. */
  text: string;
}

export const DEFAULT_FILTERS: Filters = {
  categories: CATEGORIES.map((c) => c.id),
  altitude: [0, MAX_ALTITUDE],
  airborne: true,
  onGround: true,
  airlines: [],
  types: [],
  airports: [],
  text: "",
};

/**
 * Categories to request from the server, or `null` for all of them (with
 * none selected nothing is shown, and the request just keeps the count fresh).
 */
export function serverCategories(filters: Filters): number[] | null {
  const n = filters.categories.length;
  return n === CATEGORIES.length || n === 0 ? null : [...filters.categories];
}

/** How many filter groups narrow the view, for the badge. */
export function activeCount(f: Filters): number {
  return [
    f.categories.length !== CATEGORIES.length,
    f.altitude[0] > 0 || f.altitude[1] < MAX_ALTITUDE,
    !f.airborne || !f.onGround,
    f.airlines.length > 0,
    f.types.length > 0,
    f.airports.length > 0,
    f.text.trim().length > 0,
  ].filter(Boolean).length;
}

/** A predicate for the client-side filters (categories are applied by the server). */
export function matcher(f: Filters): (flight: LiveFlight) => boolean {
  const [low, high] = f.altitude;
  const unbounded = high >= MAX_ALTITUDE;
  const airlines = new Set(f.airlines);
  const airports = new Set(f.airports);
  const text = f.text.trim().toUpperCase();
  if (!f.categories.length) return () => false;
  return (flight) => {
    if (flight.onGround ? !f.onGround : !f.airborne) return false;
    // aircraft on the ground are at 0 ft: a minimum altitude leaves them out
    if (flight.onGround ? low > 0 : flight.alt < low || (!unbounded && flight.alt > high)) return false;
    if (airlines.size) {
      const icao = airlineIcao(flight.callsign);
      if (!icao || !airlines.has(icao)) return false;
    }
    if (f.types.length && !f.types.some((t) => flight.typecode.startsWith(t))) return false;
    if (airports.size && !airports.has(flight.origin) && !airports.has(flight.destination)) return false;
    if (text && ![flight.callsign, flight.flight, flight.reg].some((v) => v.toUpperCase().includes(text))) return false;
    return true;
  };
}

export interface Settings {
  basemap: Basemap;
  labels: boolean;
  buildings: boolean;
  liveries: boolean;
  daylight: boolean;
  weather: boolean;
  exaggeration: number;
}

export const DEFAULT_SETTINGS: Settings = {
  basemap: "dark",
  labels: true,
  buildings: true,
  liveries: true,
  daylight: true,
  weather: false,
  exaggeration: 10,
};

const STORAGE_KEY = "rustradar.v1";

export function loadPersisted(): { settings: Settings; filters: Filters } {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}");
    return {
      settings: { ...DEFAULT_SETTINGS, ...saved.settings },
      filters: { ...DEFAULT_FILTERS, ...saved.filters },
    };
  } catch {
    return { settings: { ...DEFAULT_SETTINGS }, filters: { ...DEFAULT_FILTERS } };
  }
}

export function persist(settings: Settings, filters: Filters) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ settings, filters }));
  } catch {
    // storage full or unavailable: settings just won't survive a restart
  }
}
