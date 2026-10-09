// Typed wrappers around the Tauri commands in `src-tauri/src/lib.rs`.
import { Channel, invoke } from "@tauri-apps/api/core";

export interface LiveFlight {
  id: number;
  lat: number;
  lon: number;
  /** degrees clockwise from north */
  track: number;
  /** feet */
  alt: number;
  /** knots */
  speed: number;
  onGround: boolean;
  timestampMs: number;
  callsign: string;
  flight: string;
  reg: string;
  typecode: string;
  origin: string;
  destination: string;
  /** Flightradar24's icon class, e.g. `A320`, `B747`, `EC` (helicopter). */
  icon: string;
  /**
   * Positions after `lat`/`lon`: `[Δlat, Δlon]` in 1e-5° and ms after
   * `timestampMs`, oldest first. Up to ~10 s of look-ahead for smooth playback.
   */
  positions?: [number, number, number][];
  /** Feet per minute; only when signed in. */
  vspeed?: number;
}

export interface LiveSnapshot {
  flights: LiveFlight[];
  serverTimeMs: number;
  requests: number;
  errors: string[];
  elapsedMs: number;
  authenticated: boolean;
}

export interface TrailPoint {
  timestamp: number;
  latitude: number;
  longitude: number;
  altitude: number;
  ground_speed: number;
  track: number;
  vertical_speed: number;
  source: number;
}

export interface AircraftImage {
  url: string;
  copyright: string;
  thumbnail: string;
  medium: string;
  large: string;
}

/** A filed flight plan; only sent to Flightradar24 subscribers. */
export interface FlightPlan {
  departure: string;
  destination: string;
  /** ICAO field 15 route string. */
  route: string;
  length: number;
  /** Fixed-point [lat, lon] pairs of undocumented scale. */
  waypoints: [number, number][];
  alternates: string[];
}

export interface FlightDetails {
  aircraft: {
    icao_address: number;
    reg: string;
    typecode: string;
    full_description: string;
    service: string;
    registered_owners: string;
    msn: string | null;
    birth_date: string | null;
    age: number | null;
    images: AircraftImage[];
  } | null;
  schedule: {
    flight_number: string;
    operated_by_id: number;
    painted_as_id: number;
    origin_id: number;
    destination_id: number;
    diverted_id: number;
    scheduled_departure: number;
    scheduled_arrival: number;
    actual_departure: number;
    actual_arrival: number;
    arr_terminal: string;
    arr_gate: string;
  } | null;
  progress: {
    traversed_distance: number;
    remaining_distance: number;
    elapsed_time: number;
    remaining_time: number;
    eta: number;
    great_circle_distance: number;
    mean_flight_time: number;
    flight_stage: string;
    delay_status: string;
    progress_pct: number;
  } | null;
  flight: {
    timestamp_ms: number;
    flightid: number;
    latitude: number;
    longitude: number;
    track: number;
    altitude: number;
    ground_speed: number;
    vertical_speed: number;
    on_ground: boolean;
    callsign: string;
    squawk: number;
    source: string;
    airspace: string;
  } | null;
  flight_plan: FlightPlan | null;
  trail: TrailPoint[];
}

export type FollowEvent =
  | { event: "details"; data: FlightDetails }
  | { event: "error"; data: string }
  | { event: "ended"; data: string };

export interface TopFlight {
  flight_id: number;
  live_clicks: number;
  total_clicks: number;
  flight_number: string;
  callsign: string;
  from_iata: string;
  from_city: string;
  to_iata: string;
  to_city: string;
  type: string;
  full_description: string;
}

interface FindBase {
  id: string;
  label: string;
  name: string | null;
  match: string | null;
}

export type FindEntry = FindBase &
  (
    | { type: "airport"; detail: { lat: number; lon: number; size: number | null } }
    | { type: "live"; detail: { lat: number; lon: number; callsign: string | null; route: string | null; ac_type: string | null } }
    | { type: "operator" | "schedule" | "aircraft" | "unknown"; detail: unknown }
  );

export interface Airline {
  name: string;
  iata: string;
  icao: string;
}

export interface Airport {
  /** Flightradar24's airport id, as in a flight's schedule. */
  id: number;
  name: string;
  iata: string;
  icao: string;
  city: string;
  country: string;
  lat: number;
  lon: number;
  /** Field elevation, feet, where known. */
  alt?: number | null;
  size: number;
  /** IANA timezone name. */
  timezone: string;
}

export interface ReferenceData {
  fetchedAt: number;
  airlines: Airline[];
  airports: Airport[];
}

export type BoardMode = "arrivals" | "departures";

/** One flight on an airport board; times are Unix seconds at this airport. */
export interface BoardRow {
  flightId: number | null;
  number: string | null;
  callsign: string | null;
  airline: string | null;
  airlineIcao: string | null;
  logo: string | null;
  airportIata: string | null;
  airportName: string | null;
  airportCity: string | null;
  scheduled: number | null;
  estimated: number | null;
  actual: number | null;
  status: string | null;
  statusColor: string | null;
  live: boolean;
  aircraft: string | null;
  registration: string | null;
  terminal: string | null;
  gate: string | null;
}

export interface Board {
  rows: BoardRow[];
  total: number | null;
  page: number;
  pages: number | null;
}

/** One past or scheduled flight of an aircraft; times are Unix seconds. */
export interface HistoryRow {
  flight_id: number | null;
  number: string | null;
  callsign: string | null;
  registration: string | null;
  typecode: string | null;
  origin: string | null;
  origin_iata: string | null;
  destination: string | null;
  destination_iata: string | null;
  status: string | null;
  status_color: string | null;
  live: boolean;
  stod: number | null;
  etod: number | null;
  atod: number | null;
  stoa: number | null;
  etoa: number | null;
  atoa: number | null;
}

export interface LogoColors {
  primary: [number, number, number];
  secondary: [number, number, number] | null;
}

export interface SessionInfo {
  authenticated: boolean;
  expires: number | null;
}

export const isTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export interface BoundingBox {
  south: number;
  north: number;
  west: number;
  east: number;
}

/**
 * Live flights within `area` (west > east wraps the antimeridian), or the
 * whole world; `services` limits the categories (`null` means all).
 */
export const liveFlights = (area: BoundingBox | null, services: number[] | null) =>
  invoke<LiveSnapshot>("live_flights", { area, services });

/** Flightradar24 answers gRPC status 8 (resource exhausted) when polled too hard. */
export const isRateLimited = (message: string) => /status 8|too many requests/i.test(message);

export const flightDetails = (flightId: number) =>
  invoke<FlightDetails>("flight_details", { flightId });

export const topFlights = () => invoke<TopFlight[]>("top_flights");

export const search = (query: string) => invoke<FindEntry[]>("search", { query });

export const referenceData = () => invoke<ReferenceData>("reference_data");

export const airportBoard = (code: string, mode: BoardMode, page = 1) =>
  invoke<Board>("airport_board", { code, mode, page });

export const aircraftHistory = (registration: string) =>
  invoke<HistoryRow[]>("aircraft_history", { registration });

export const airlineColors = (iatas: string[]) =>
  invoke<Record<string, LogoColors | null>>("airline_colors", { iatas });

/** One source's current 3D radar scan: NOAA MRMS, or DWD over central Europe. */
export interface RadarVolume {
  /** The name images are requested by, e.g. `mrms`. */
  source: string;
  /** Unix seconds of the scan. */
  time: number;
  bounds: { west: number; south: number; east: number; north: number };
  /** Heights (km) with polygons. */
  levels: number[];
  /** Size of the coverage mask, one byte per cell, north row first. */
  coverageWidth: number;
  coverageHeight: number;
}

/** `area` limits the fetch to the sources on screen; `null` means everywhere. */
export const radarFrame = (area: BoundingBox | null) => invoke<RadarVolume[]>("radar_frame", { area });

/**
 * `ground` or a height such as `3`: filled reflectivity polygons in the binary
 * format `parseVolumePolygons` reads (radar-mesh.ts); `coverage`: the raw mask.
 */
export const radarImage = (source: string, name: string) =>
  invoke<ArrayBuffer>("radar_image", { source, name });

/** One position of a flight tracked by the OpenSky Network. */
export interface OpenSkyPoint {
  /** Unix seconds */
  timestamp: number;
  latitude: number;
  longitude: number;
  /** Feet */
  altitude: number;
  track: number;
  on_ground: boolean;
}

/** The flight an aircraft is on now, by Mode S address (six hex digits). */
export const openskyTrack = (icao24: string) => invoke<OpenSkyPoint[]>("opensky_track", { icao24 });

export const sessionInfo = () => invoke<SessionInfo>("session_info");

export const signIn = (email: string, password: string) =>
  invoke<SessionInfo>("sign_in", { email, password });

export const signOut = () => invoke<SessionInfo>("sign_out");

/** Streams updates for `flightId` until {@link unfollowFlight} or another flight is followed. */
export function followFlight(flightId: number, onEvent: (e: FollowEvent) => void) {
  const channel = new Channel<FollowEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("follow_flight", { flightId, onEvent: channel });
}

export const unfollowFlight = () => invoke<void>("unfollow_flight");

/** Flightradar24's logo for an airline id, such as a schedule's `painted_as_id`. */
export const airlineLogoUrl = (airlineId: number) =>
  `https://cdn.flightradar24.com/assets/airlines/logotypes/${airlineId}.png`;
