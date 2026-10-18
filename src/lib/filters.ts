// Aircraft filters and persisted settings.
import type { LiveFlight } from "./api";
import type { Basemap } from "./geo";
import { airlineIcao } from "./reference.svelte";
import { mergeShipFilters, type ShipFilters } from "./ship-list";
import { mergeTrainFilters, type TrainFilters } from "./train-list";
import { SKIES, type Sky } from "./weather";

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

/** The kind of traffic the sidebar is for; the globe shows it and any others in `alsoShow`. */
export type Mode = "aircraft" | "trains" | "ships";
export const MODES: readonly Mode[] = ["aircraft", "trains", "ships"];

/** What the rail map shows: the track's speed limits, or its signal blocks (close in). */
export type RailShows = "speeds" | "blocks";
export const RAIL_SHOWS: readonly RailShows[] = ["speeds", "blocks"];

export interface Settings {
  /** The focus: aircraft, trains or ships. Its kind is always on the map, and the sidebar is for it. */
  mode: Mode;
  /** Other kinds also fetched and drawn on the map; the focus is shown whether listed or not. */
  alsoShow: Mode[];
  basemap: Basemap;
  labels: boolean;
  /** Place-name size on the map, as a multiple of the default. */
  labelSize: number;
  buildings: boolean;
  /** Callsigns written next to the aircraft on the map. */
  aircraftLabels: boolean;
  daylight: boolean;
  weather: boolean;
  /** Measured data for the selected aircraft from adsb.lol. */
  adsb: boolean;
  /** Railway lines from OpenRailwayMap. */
  railMap: boolean;
  /** What the rail map shows: speed limits, or signal blocks close in (free, or taken by the train drawn in them). */
  railShows: RailShows;
  /** The realistic map's weather: live where the map is looking, or one chosen. */
  sky: Sky;
  /** The realistic map's top and bottom out of focus, as through a tilt-shift lens. */
  tiltShift: boolean;
  /** Relief from the AWS Terrain Tiles (Mapzen). */
  terrain: boolean;
  exaggeration: number;
}

export const DEFAULT_SETTINGS: Settings = {
  mode: "aircraft",
  alsoShow: [],
  basemap: "dark",
  labels: true,
  labelSize: 1,
  buildings: true,
  aircraftLabels: false,
  daylight: true,
  weather: false,
  adsb: true,
  railMap: false,
  railShows: "speeds",
  sky: "live",
  tiltShift: false,
  terrain: false,
  exaggeration: 10,
};

/**
 * Stored settings over the defaults. A setting added since they were saved
 * takes its default, and one stored with the wrong type (an older format,
 * a hand edit) falls back to it too.
 *
 * Settings from before the modes had an on/off switch each for aircraft,
 * ships and trains: those are dropped, and the mode starts on aircraft
 * unless the aircraft were switched off, then on whichever of ships or
 * trains was still on. (`alsoShow` has a new name so that those switches
 * can't come back through the copy by type below.)
 */
export function mergeSettings(saved: unknown): Settings {
  const merged: Settings = { ...DEFAULT_SETTINGS, alsoShow: [] };
  if (!saved || typeof saved !== "object") return merged;
  const values = merged as unknown as Record<string, unknown>;
  for (const [key, value] of Object.entries(saved)) {
    if (key in DEFAULT_SETTINGS && typeof value === typeof values[key]) values[key] = value;
  }
  const old = saved as Record<string, unknown>;
  // an array is an object to typeof: anything but an array of known kinds is none
  merged.alsoShow = Array.isArray(old.alsoShow)
    ? MODES.filter((m) => (old.alsoShow as unknown[]).includes(m))
    : [];
  if (!SKIES.includes(merged.sky)) merged.sky = DEFAULT_SETTINGS.sky;
  if (!RAIL_SHOWS.includes(merged.railShows)) merged.railShows = DEFAULT_SETTINGS.railShows;
  // signal blocks were a switch of their own, drawn with or without the rail map
  if (old.trainBlocks === true && !("railShows" in old)) Object.assign(merged, { railMap: true, railShows: "blocks" });
  if (!MODES.includes(old.mode as Mode)) {
    merged.mode = DEFAULT_SETTINGS.mode;
    // the old switches defaulted to on: missing counts as on
    if (old.aircraft !== false) merged.mode = "aircraft";
    else if (old.ships !== false) merged.mode = "ships";
    else if (old.trains !== false) merged.mode = "trains";
  }
  return merged;
}

/** Which kinds are on the map: the focus, and whatever else is shown with it. */
export function shownKinds(settings: Pick<Settings, "mode" | "alsoShow">): Record<Mode, boolean> {
  const on = (m: Mode) => m === settings.mode || settings.alsoShow.includes(m);
  return { aircraft: on("aircraft"), trains: on("trains"), ships: on("ships") };
}

const STORAGE_KEY = "rustradar.v1";

export interface Persisted {
  settings: Settings;
  filters: Filters;
  trainFilters: TrainFilters;
  shipFilters: ShipFilters;
}

export function loadPersisted(): Persisted {
  let saved: Record<string, unknown> = {};
  try {
    saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}") ?? {};
  } catch {
    // unreadable: the defaults
  }
  return {
    settings: mergeSettings(saved.settings),
    filters: { ...DEFAULT_FILTERS, ...(saved.filters as Partial<Filters> | undefined) },
    trainFilters: mergeTrainFilters(saved.trainFilters),
    shipFilters: mergeShipFilters(saved.shipFilters),
  };
}

export function persist(state: Persisted) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
  } catch {
    // storage full or unavailable: settings just won't survive a restart
  }
}
