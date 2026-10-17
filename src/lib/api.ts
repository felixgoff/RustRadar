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
   * The aircraft's recent history (Flightradar24's position buffer):
   * `[Δlat, Δlon, Δms]` back from the report, each point at
   * `lat - Δlat·1e-5`, `lon - Δlon·1e-5` and `timestampMs - Δms`, newest first
   * (Δms increasing). Up to ~10 s, overlapping the previous reports' positions.
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

/** A coarse category of a place, for its icon and how close to fly. */
export type PlaceKind =
  | "address"
  | "street"
  | "poi"
  | "airport"
  | "district"
  | "city"
  | "town"
  | "postcode"
  | "region"
  | "country";

/** A place or address from Photon (© OpenStreetMap contributors, ODbL). */
export interface Place {
  /** Its name, or the street and number, e.g. `Damrak 1`. */
  name: string;
  /** A readable address line, e.g. `Damrak 1, 1012 LG Amsterdam, Netherlands`. */
  label: string;
  kind: PlaceKind;
  /** What a named place is, e.g. `Hotel`, `Canal` (points of interest only). */
  category?: string;
  lat: number;
  lon: number;
  /** `[west, south, east, north]`, degrees, when the place has an extent. */
  bbox?: [number, number, number, number];
}

/**
 * Places and addresses matching `query` (at least 3 characters), ranked toward
 * `near` when given. Rate-limited in Rust; rejects while the service rests.
 */
export const geocode = (query: string, near?: { lat: number; lon: number } | null, lang?: string) =>
  invoke<Place[]>("geocode", { query, lat: near?.lat ?? null, lon: near?.lon ?? null, lang: lang ?? null });

export const referenceData = () => invoke<ReferenceData>("reference_data");

/** One end of a runway, from OurAirports. */
export interface RunwayEnd {
  /** e.g. `22L` */
  ident: string;
  /** The physical end of the runway; the landing threshold may be displaced from it. */
  lat: number;
  lon: number;
  elevationFt?: number | null;
  /** Feet from the physical end to the landing threshold. */
  displacedFt: number;
}

export interface Runway {
  lengthFt?: number | null;
  widthFt?: number | null;
  surface: string;
  ends: [RunwayEnd, RunwayEnd];
}

/** Runways of airports by ICAO code (OurAirports, cached weekly); unknown airports are left out. */
export const airportRunways = (airports: string[]) =>
  invoke<Record<string, Runway[]>>("airport_runways", { airports });

/** A taxiway, taxilane or runway centreline from OpenStreetMap (one OSM way). */
export interface TaxiLine {
  kind: "taxiway" | "taxilane" | "runway";
  /** Designator where tagged, e.g. `A3` or `04L/22R`. */
  ref?: string;
  /** `[lon, lat]` pairs. */
  points: [number, number][];
}

/**
 * The taxiway, taxilane and runway centrelines within 4 km of an airport (ICAO
 * code and reference point), from OpenStreetMap via Overpass; cached for a
 * month. Empty when they can't be had.
 */
export const airportTaxiways = (icao: string, lat: number, lon: number) =>
  invoke<TaxiLine[]>("airport_taxiways", { icao, lat, lon });

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

/**
 * A ship as VesselFinder's public map shows it (AIS). The server thins ships
 * out by zoom: zoomed out only some of them come back.
 */
export interface Ship {
  mmsi: number;
  lat: number;
  lon: number;
  /** Its name, or its MMSI when it hasn't broadcast one. */
  name: string;
  /**
   * VesselFinder's type colour: 2 tug/special, 3 passenger, 4 cargo,
   * 5 fishing, 6 tanker, 7 SAR/law enforcement, 8 pleasure/sailing, 0 unknown;
   * anything else is "other".
   */
  kind: number;
  /** 0 small, 1 medium, 2 large (always medium when zoomed out). */
  size: 0 | 1 | 2;
  /** Degrees clockwise from north; null when moored or not reported. */
  heading: number | null;
  /** The position is old. */
  old: boolean;
  /** Metres from the AIS reference point to bow, stern, port and starboard; close in only. */
  dims: [number, number, number, number] | null;
  /**
   * Where it is expected over the next couple of minutes as `[lat, lon,
   * unixMs]`, worked out from how it was seen to move; empty when it isn't
   * moving. See `positionAt` in trains.ts.
   */
  frames: [number, number, number][];
}

/**
 * Ships in an area (`west > east` crosses the antimeridian) at a deck.gl zoom.
 * Throttled in Rust (one request at a time, an area fetched in the last 10 s
 * comes from memory); rejects while VesselFinder rests after a failure.
 */
export const shipsInView = (area: BoundingBox, zoom: number) =>
  invoke<Ship[]>("ships_in_view", { ...area, zoom });

/** An AIS navigational status: the standard code (0..14) and its wording. */
export interface NavStatus {
  code: number;
  label: string;
}

/**
 * One ship's particulars and last report, from VesselFinder's ship card.
 * Values it leaves out, or gives as 0 meaning "not known", are null.
 */
export interface ShipDetails {
  name: string | null;
  imo: number | null;
  mmsi: number;
  /** VesselFinder's own type wording, e.g. "Container Ship". */
  typeLabel: string | null;
  /** The flag state's name. */
  flag: string | null;
  /** ISO 3166 code, upper case. */
  flagCode: string | null;
  lengthM: number | null;
  beamM: number | null;
  /** As last entered by the crew. */
  draughtM: number | null;
  maxDraughtM: number | null;
  grossTonnage: number | null;
  deadweight: number | null;
  yearBuilt: number | null;
  /** As entered by the crew: often a UN/LOCODE such as `UKSOU`, sometimes free text. */
  destination: string | null;
  /** As entered by the crew, Unix seconds. */
  etaUnix: number | null;
  /** Speed over ground, knots. */
  speedKn: number | null;
  /** Course over ground, degrees. */
  courseDeg: number | null;
  navStatus: NavStatus | null;
  /** When the position was reported, Unix seconds. */
  positionUnix: number | null;
  /** The position came by satellite AIS. */
  satellite: boolean;
  photoId: string | null;
  /** A search-and-rescue aircraft. */
  sar: boolean;
}

/** Throttled with the map feed, and answered from memory for a minute per ship. */
export const shipDetails = (mmsi: number) => invoke<ShipDetails>("ship_details", { mmsi });

/** VesselFinder's photo of a ship: 768x512, or the 180x120 thumbnail. */
export const shipPhotoUrl = (photoId: string, thumbnail = false) =>
  `https://static.vesselfinder.net/ship-photo/${encodeURIComponent(photoId)}/${thumbnail ? 0 : 1}`;

/** The ship's page on VesselFinder. */
export const shipPageUrl = (mmsi: number) => `https://www.vesselfinder.com/vessels/details/${mmsi}`;

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

/**
 * One aircraft as adsb.lol reports it: what its transponder broadcasts,
 * including what Flightradar24's anonymous feed leaves out. ODbL: credit
 * "adsb.lol contributors" wherever it is shown. Absent values are left out.
 */
export interface AdsbAircraft {
  /** ICAO 24-bit address, lowercase hex. */
  hex: string;
  callsign: string;
  reg: string;
  typecode: string;
  /** How it was received: `adsb_icao`, `mlat`, `tisb_icao`, ... */
  source: string;
  /** The position is multilaterated. */
  mlat: boolean;
  lat?: number;
  lon?: number;
  /** Unix ms of the position. */
  positionMs?: number;
  /** Unix ms of the latest message of any kind. */
  seenMs: number;
  onGround: boolean;
  /** Feet. */
  altBaro?: number;
  altGeom?: number;
  /** Knots; degrees true. */
  gs?: number;
  track?: number;
  trueHeading?: number;
  magHeading?: number;
  /** Bank, degrees, right wing down positive. */
  roll?: number;
  /** Degrees per second, clockwise positive. */
  trackRate?: number;
  /** Feet per minute. */
  baroRate?: number;
  geomRate?: number;
  /** Knots, and Mach. */
  ias?: number;
  tas?: number;
  mach?: number;
  /** Where the wind blows from, degrees true, and its speed, knots. */
  windDir?: number;
  windSpeed?: number;
  /** °C */
  oat?: number;
  tat?: number;
  /** Selected altitudes (autopilot panel, FMS), feet; selected heading, degrees. */
  navAltitudeMcp?: number;
  navAltitudeFms?: number;
  navHeading?: number;
  /** hPa */
  navQnh?: number;
  navModes?: string[];
  squawk?: string;
  emergency?: string;
  category?: string;
}

export interface AdsbSnapshot {
  /** adsb.lol's clock when it answered, Unix ms. */
  nowMs: number;
  aircraft: AdsbAircraft[];
}

/** One aircraft from adsb.lol by Mode S address (six hex digits); polite spacing and backoff are applied by the backend. */
export const adsbHex = (hex: string) => invoke<AdsbSnapshot>("adsb_hex", { hex });

/** Aircraft from adsb.lol using a callsign, for when the Mode S address isn't known yet. */
export const adsbCallsign = (callsign: string) => invoke<AdsbSnapshot>("adsb_callsign", { callsign });

export const ADSB_LOL_URL = "https://adsb.lol";

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

/** Where a train comes from: one regional public feed each. */
export type TrainSource = "trafikverket";

export type TrainKind = "highSpeed" | "intercity" | "regional" | "suburban" | "other";

/** A train on the move, merged from regional public feeds (no global one exists). */
/** A position report as the dev map marks it. */
export interface Sighting {
  lat: number;
  lon: number;
  unixMs: number;
  speedKmh: number | null;
  /** The newest report; else the one before it, the train is drawn from. */
  latest: boolean;
}

export interface Train {
  /** Source-prefixed and stable while the train runs, e.g. `trafikverket:2026-10-13:523`. */
  id: string;
  source: TrainSource;
  /** e.g. "SJ Snabbtåg 523", "Pendeltåg 2245". */
  name: string;
  kind: TrainKind;
  lat: number;
  lon: number;
  /** Degrees clockwise from north; null when the source has none (or it stands still). */
  heading: number | null;
  speedKmh: number | null;
  destination: string | null;
  operator: string | null;
  /** Minutes late at the last or next stop (negative: early). */
  delayMinutes: number | null;
  /** When the position was reported, Unix seconds. */
  positionUnix: number;
  /**
   * Where it is drawn as `[lat, lon, unixMs]`, in time order, from about now
   * on (along the track, for trains on one); empty when there is nothing to
   * move it by. See `positionAt` in trains.ts.
   */
  frames: [number, number, number][];
  /** Dev builds: its latest report and the one before. */
  reports?: Sighting[];
  /** Dev builds: where the real train most likely is, as `frames`. */
  predicted?: [number, number, number][];
  /** How many cars (and locomotives) it runs with, where its operator says. */
  cars?: number | null;
  /** The track it has come along, `[lat, lon]` back from the start of `frames`, for its cars. */
  tail?: [number, number][];
}

/**
 * Trains in Sweden in an area (`west > east` crosses the antimeridian) at a
 * deck.gl zoom, from Trafikverket with the key from the keys page. Cached
 * 10–30 s in Rust (the live streams patch it between), with backoff.
 */
export const trainsInView = (area: BoundingBox, zoom: number) =>
  invoke<Train[]>("trains_in_view", { ...area, zoom });

/** A time at a stop, Unix seconds: as planned, as now expected, as it happened. */
export interface StopTime {
  planned: number | null;
  estimated: number | null;
  actual: number | null;
}

/** One stop of a train's run. */
export interface TrainStop {
  name: string;
  lat: number | null;
  lon: number | null;
  /** None at the first stop. */
  arrival: StopTime | null;
  /** None at the last stop. */
  departure: StopTime | null;
  track: string | null;
  canceled: boolean;
  /** Notices at the stop: delay causes, replacement buses. */
  notes: string[];
}

/** A photo of a train, from the Wikipedia article on its product or operator. */
export interface TrainPhoto {
  url: string;
  /** The article it comes from. */
  title: string;
  pageUrl: string;
}

/** Pictures for the train panel, from Wikimedia: a photo of the train and its operator's logo, when found. */
export interface TrainMedia {
  photo: TrainPhoto | null;
  logo: string | null;
}

export const trainMedia = (source: string, product: string | null, operator: string | null) =>
  invoke<TrainMedia>("train_media", { source, product, operator });

/** Live news of the followed train: a new position (with the way ahead), or its stops as they now stand. */
export type TrainEvent = { kind: "train"; train: Train } | { kind: "stops"; stops: TrainStop[] };

/** Streams live updates of a train until {@link unfollowTrain} or another is followed (Trafikverket's trains). */
export function followTrain(id: string, onEvent: (e: TrainEvent) => void) {
  const channel = new Channel<TrainEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("follow_train", { id, onEvent: channel });
}

export const unfollowTrain = () => invoke<void>("unfollow_train");

/** A place on a train's way ahead: a station it stops at, or one it passes; at its platform where the track is known. */
export type RouteStop = { name: string; lat: number; lon: number; track: string | null; stops: boolean };
/** A train's way ahead along the track, from where it is to its last station. */
export type TrainRoute = {
  /** Stretches of track, `[lon, lat]`; the first starts at the train. Gaps where the track isn't known yet, never straight lines. */
  segments: [number, number][][];
  /** The places still ahead. */
  stops: RouteStop[];
  /** False while some track is still loading: ask again shortly. */
  complete: boolean;
};
export const trainRoute = (id: string) => invoke<TrainRoute>("train_route", { id });
/** A signal block: from a main signal to the next its trains meet, `[lon, lat]` stretches (branching where the track does). */
export type TrainBlock = { id: string; signal: string | null; lines: [number, number][][] };
/** The blocks starting in an area, close in; while `complete` is false the track or signals are still loading. */
export const trainBlocks = (area: { west: number; south: number; east: number; north: number }) =>
  invoke<{ blocks: TrainBlock[]; complete: boolean }>("train_blocks", area);
/** The trains the map shows close up, streamed live like a followed one (an empty list stops it). */
export const watchTrains = (ids: string[]) => invoke<void>("watch_trains", { ids });

/** The operators left out on the map ("" for trains whose operator isn't known): their trains aren't streamed live. */
export const hideTrainOperators = (names: string[]) => invoke<void>("hide_train_operators", { names });

/** Every stop of a train, in order, for sources with its timetable (Trafikverket); empty for the others. */
export const trainStops = (id: string) => invoke<TrainStop[]>("train_stops", { id });

/** Whether the saved Trafikverket key works: "none" saved, "invalid" (refused) or "ok"; rejects when Trafikverket can't be reached. */
export const trafikverketKeyStatus = () => invoke<"none" | "invalid" | "ok">("trafikverket_key_status");
