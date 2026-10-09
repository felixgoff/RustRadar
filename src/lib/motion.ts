// Aircraft motion between feed reports: where to draw each aircraft, and how.
//
// Flightradar24 sends a report every 8-15 s (minutes, for some sources) with a
// short look-ahead buffer of positions. Between reports we interpolate inside
// the known samples, extrapolate along an arc past them, predict touchdowns
// and lift-offs, derive pitch, bank and gear, and blend every correction in
// over a few seconds so that nothing on the globe ever jumps or freezes.
//
// The selected aircraft can also get measured state from a second source
// (adsb.lol, see `Motion.measure`): its position samples interleave with
// Flightradar24's by time, and its transponder-reported heading, bank, turn
// and vertical rates and autopilot targets replace the estimates while they
// are fresh, eased in and out through the same blending. Every other aircraft
// is drawn from Flightradar24 alone.
//
// Times are server-synced milliseconds (`motion.now()`) throughout. Horizontal
// work happens in a local east/north metre frame around each track's newest
// sample: equirectangular, which is exact enough for the few hundred
// kilometres an aircraft can cover before its data runs out.
import type { Airport, LiveFlight } from "./api";

/**
 * Measured state of one aircraft from a second source (adsb.lol's readsb
 * fields; its `AdsbAircraft` fits). Times are Unix ms; absent values are
 * simply left out.
 */
export interface Measured {
  /** When `lat`/`lon` were measured. */
  positionMs?: number;
  lat?: number;
  lon?: number;
  /** The position is multilaterated: less precise than Flightradar24's. */
  mlat?: boolean;
  /** When the other values were last reported. */
  seenMs: number;
  onGround: boolean;
  /** Barometric altitude, feet. */
  altBaro?: number;
  /** Ground speed, kt; track over the ground and nose heading, degrees true. */
  gs?: number;
  track?: number;
  trueHeading?: number;
  /** Nose heading, degrees magnetic: with `trueHeading`, gives the variation for `navHeading`. */
  magHeading?: number;
  /** Bank, degrees, right wing down positive. */
  roll?: number;
  /** Degrees per second, clockwise positive. */
  trackRate?: number;
  /** Feet per minute. */
  baroRate?: number;
  geomRate?: number;
  /** True airspeed, kt. */
  tas?: number;
  /** Altitudes selected on the autopilot (MCP/FCU) and by the FMS, feet. */
  navAltitudeMcp?: number;
  navAltitudeFms?: number;
  /** Heading selected on the autopilot, degrees (magnetic, in practice). */
  navHeading?: number;
  navModes?: string[];
}

export type Phase =
  | "parked"
  | "taxi"
  | "takeoffRoll"
  | "initialClimb"
  | "climb"
  | "cruise"
  | "descent"
  | "approach"
  | "flare"
  | "landingRoll";

export interface Pose {
  lon: number;
  lat: number;
  /** Height to draw at, feet: MSL altitude with the relevant airport's field elevation removed near airports (faded back to plain MSL by ~30-60 km out), exactly 0 on the ground. */
  altFt: number;
  /**
   * Degrees clockwise from north: where the nose points. Without measured
   * data this is the direction of motion (`track`); with a measured true
   * heading it differs from it by the crab angle into a crosswind.
   */
  heading: number;
  /** Degrees clockwise from north: direction of motion along the drawn path (reported track when parked). */
  track: number;
  /** Degrees, nose up positive. */
  pitch: number;
  /** Degrees, right wing down positive (a right turn banks right). */
  bank: number;
  /** 0 = retracted .. 1 = fully down, animated. */
  gear: number;
  phase: Phase;
}

const DEG = Math.PI / 180;
const EARTH_RADIUS_M = 6_371_000;
const M_PER_DEG = EARTH_RADIUS_M * DEG;
const KM_PER_DEG = M_PER_DEG / 1000;
const KNOTS_TO_MS = 0.514444;
const FEET_TO_M = 0.3048;
const FPM_TO_MS = FEET_TO_M / 60;
const GRAVITY_MS2 = 9.80665;

// --- Samples -----------------------------------------------------------------

/** How much horizontal history a track keeps. */
const SAMPLE_WINDOW_MS = 60_000;
/** A newer report's samples replace old ones from this long before its own time on. */
const MERGE_EPSILON_MS = 500;
/** Samples closer together than this are duplicates (and would make the spline's tangents explode). */
const MIN_SAMPLE_GAP_MS = 200;
/** How far before the first sample we run the track backwards (clock skew), seconds. */
const BACKWARD_LIMIT_S = 30;
/** FR24 buffer offsets are in 1e-5 degrees. */
const BUFFER_SCALE = 1e-5;

// --- Estimators ----------------------------------------------------------------

/** Weight of a fresh estimate against the previous one ("lightly smoothed"). */
const SMOOTHING = 0.5;
/** Ground-speed change limit, kt/s: airliners manage ~3-4 kt/s on a takeoff roll. */
const MAX_ACCEL_KTS = 4;
/** Turn rate limit, deg/s: a rate-one turn is 3 deg/s; anything faster is noise. */
const MAX_TURN_DEG_S = 4;
const MAX_VRATE_FPM = 8_000;
/** Altitude comes in ~25 ft steps, so vertical rates need a few seconds between reports. */
const MIN_VRATE_DT_S = 4;
const MAX_VRATE_DT_S = 300;
/** Speed differences between reports further apart than this say nothing about acceleration now. */
const MIN_ACCEL_DT_S = 4;
const MAX_ACCEL_DT_S = 120;
/** Reported-track differences between reports further apart than this may hide whole turns. */
const MIN_TURN_DT_S = 2;
const MAX_TURN_DT_S = 60;
/**
 * Buffer chords shorter than this are too short for their heading to mean
 * anything (positions are quantised to ~1 m), so slow aircraft take their
 * turn rate from reported tracks instead.
 */
const MIN_TURN_CHORD_M = 100;
const MIN_SPEED_CHORD_M = 10;
/** Sanity cap on speeds read off the buffer. */
const MAX_SPEED_MS = 1_200 * KNOTS_TO_MS;

// --- Extrapolation -------------------------------------------------------------

/** Keep moving this long past the last data, then hold. */
const MAX_EXTRAPOLATION_S = 600;
/** A turn is continued this long past the data, then the path goes straight. */
const TURN_HORIZON_S = 60;
/** Bank fades out over the last part of the turn horizon. */
const BANK_FADE_START_S = 45;
/** Acceleration is applied this long past the data. */
const ACCEL_HORIZON_S = 20;
/** Airborne speed changes are capped at this many knots either way. */
const MAX_AIR_SPEED_CHANGE_KT = 30;
/** Taxi speed changes are gentler and stay below this. */
const MAX_TAXI_ACCEL_KTS = 1;
const MAX_TAXI_SPEED_KT = 30;
/** Vertical rate is applied fully for this long past the report... */
const VERTICAL_FADE_START_S = 60;
/** ...then fades linearly to level flight by this time. */
const VERTICAL_HORIZON_S = 90;
const MAX_ALT_FT = 60_000;
/** Tightest turn radius we draw (1 / metres): taxiway corners. */
const MAX_CURVATURE = 1 / 30;
/** Below this speed (m/s) a velocity's direction is noise; keep the last heading. */
const HEADING_MIN_SPEED_MS = 1;

// --- Blending corrections ---------------------------------------------------------

/** Critically damped decay rate, 1/s: (1 + k t) e^{-k t} is ~1% after 3 s. */
const BLEND_RATE = 2.2;
/** After this long the correction is gone (< 0.01%). */
const BLEND_END_S = 6;
/** Larger corrections than this are a different aircraft position altogether: snap. */
const SNAP_DISTANCE_M = 5_000;
/** If the previous data was older than this, its extrapolation means little: snap. */
const SNAP_AGE_MS = 120_000;

// --- Measured data (selected aircraft) ----------------------------------------------

/** Measured values are used fully for this long after they were reported... */
const MEAS_FRESH_MS = 15_000;
/** ...then eased back to the estimates over this long. */
const MEAS_FADE_MS = 5_000;
/** Samples from the two sources closer in time than this are the same moment: the better one is kept. */
const COINCIDENT_MS = 1_000;
/** Largest crab angle believed, degrees. */
const MAX_CRAB = 30;
/** A turn onto a selected heading rolls out over this long. */
const ROLL_OUT_S = 4;
/** Altitude capture: the level-off is spread over this fraction of the vertical rate (ft per fpm)... */
const CAPTURE_FT_PER_FPM = 0.15;
/** ...within these bounds, feet either side of the selected altitude. */
const MIN_CAPTURE_FT = 100;
const MAX_CAPTURE_FT = 800;
/** Sample sources, also their rank when two coincide (higher wins). */
const SRC_FR24 = 1;
const SRC_ADSB = 2;
const SRC_MLAT = 0;

// --- Airports ----------------------------------------------------------------------

/** Destination or origin within this range is the relevant field. */
const FIELD_RANGE_KM = 40;
/** Otherwise the nearest airport within this range. */
const NEAREST_FIELD_KM = 15;
/** Field elevation is removed fully within this range of the field... */
const FADE_FULL_KM = 30;
/** ...fading to plain MSL here; a chosen field is also kept until this range. */
const FADE_ZERO_KM = 60;
/** Airport grid cell size, degrees. */
const GRID_DEG = 1;

// --- Phases ------------------------------------------------------------------------

const PARKED_MAX_KT = 3;
const TAXI_MAX_KT = 40;
/** Acceleration (kt/s) that decides between a takeoff and a landing roll. */
const ROLL_ACCEL_KTS = 0.5;
const APPROACH_MAX_AGL_FT = 3_000;
const APPROACH_MAX_VRATE_FPM = -200;
const APPROACH_RANGE_KM = 25;
const FLARE_MAX_AGL_FT = 60;
const INITIAL_CLIMB_MIN_VRATE_FPM = 200;
const CLIMB_VRATE_FPM = 500;
/** Hysteresis: an established climb or descent ends below this rate. */
const CLIMB_HOLD_VRATE_FPM = 250;

// --- Takeoff and landing prediction ---------------------------------------------------

const LANDING_DECEL_KTS = 2;
/** A predicted landing roll slows to this and then taxis on. */
const ROLLOUT_KT = 20;
const TAKEOFF_ACCEL_KTS = 2.5;
/** After rotation the aircraft keeps accelerating up to this much past rotation speed. */
const TAKEOFF_OVERSPEED_KT = 20;
const TAKEOFF_CLIMB_FPM = 1_800;
/** Rotation raises the nose over this long before lift-off... */
const ROTATION_S = 3;
/** ...to this pitch, degrees. */
const ROTATION_PITCH = 8;
/** After touchdown the nose comes down over this long. */
const TOUCHDOWN_EASE_S = 4;
/** Rotation speeds, kt, by aircraft class (helicopters don't roll). */
const ROTATION_KT = { jet: 145, turboprop: 110, light: 60 } as const;

// --- Attitude ------------------------------------------------------------------------

/** Pitch above the flight-path angle, degrees: roughly the angle of attack per phase. */
const PITCH_OFFSET: Record<Phase, number> = {
  parked: 0,
  taxi: 0,
  takeoffRoll: 0,
  landingRoll: 0,
  initialClimb: 6,
  climb: 3,
  cruise: 2,
  descent: 1,
  approach: 3,
  flare: 5,
};
const MIN_PITCH = -10;
const MAX_PITCH = 20;
const MAX_BANK = 30;

// --- Gear ----------------------------------------------------------------------------

const GEAR_TRANSIT_MS = 8_000;
const GEAR_APPROACH_AGL_FT = 2_000;
const GEAR_CLIMB_AGL_FT = 400;
/** Without a known field: low, slow and descending means gear down. */
const GEAR_FALLBACK_ALT_FT = 1_500;
const GEAR_FALLBACK_KT = 180;

// --- Housekeeping ----------------------------------------------------------------------

const TRACK_TTL_MS = 15 * 60_000;
const PRUNE_INTERVAL_MS = 60_000;
/** Clock offsets: a sample this far from the current offset is a re-sync, not jitter. */
const CLOCK_RESYNC_MS = 5_000;
const CLOCK_SMOOTHING = 0.1;
/** Server times before this (2001) are garbage. */
const MIN_SANE_TIME_MS = 1e12;

type AircraftClass = "jet" | "turboprop" | "light" | "heli";

const GROUND: ReadonlySet<Phase> = new Set<Phase>(["parked", "taxi", "takeoffRoll", "landingRoll"]);
const FROM_TAXI: ReadonlySet<Phase> = new Set<Phase>(["parked", "taxi", "takeoffRoll"]);
const FROM_LANDING: ReadonlySet<Phase> = new Set<Phase>(["approach", "flare", "landingRoll", "descent"]);

const HELI_TYPES = /^(EC\d|H\d\d|AS3|AS5|AS6|R22|R44|R66|B06|B47|B407|B412|B429|A109|A119|A139|A169|A189|AW1|S76|S92|MI8|UH|H60|BK17|EH10|NH90|EXPL|CH47|GAZL|ALO)/;
const TURBOPROP_TYPES = /^(AT4|AT7|DH8|DHC|SF34|E120|JS3|JS4|B190|C208|PC12|PC6|D228|D328|SW4|F50|BE20|BE9|BE30|AN2[46]|AN3|L410|C130|A400|P180|TBM|SB20|Y12|C212|CN35|C295|BN2)/;
const LIGHT_TYPES = /^(C1\d\d|C2[0-9]|PA\d|P28|SR2|DA[24]|BE3|BE[0-9]{2}$|M20|RV|GLID|AS2|ASW|DG|LS\d|ULAC|TOBA|AT3|C42|P2002)/;

function aircraftClass(icon: string, typecode: string): AircraftClass {
  const type = (typecode || "").toUpperCase();
  if (icon === "EC" || HELI_TYPES.test(type)) return "heli";
  if (icon === "Q300" || TURBOPROP_TYPES.test(type)) return "turboprop";
  if (icon === "C206" || icon === "C303" || icon === "GLID" || icon === "AS20" || icon === "ASW20" || LIGHT_TYPES.test(type))
    return "light";
  return "jet";
}

const clamp = (x: number, lo: number, hi: number) => (x < lo ? lo : x > hi ? hi : x);
const wrapLon = (lon: number) => ((((lon + 180) % 360) + 360) % 360) - 180;
const wrap180 = (deg: number) => ((((deg + 180) % 360) + 360) % 360) - 180;
const wrap360 = (deg: number) => ((deg % 360) + 360) % 360;
const smoothstep = (t: number) => (t <= 0 ? 0 : t >= 1 ? 1 : t * t * (3 - 2 * t));

/** Equirectangular distance, km: exact enough for the < 100 km ranges it is used for. */
function distanceKm(lat1: number, lon1: number, lat2: number, lon2: number): number {
  const x = wrap180(lon2 - lon1) * Math.cos(((lat1 + lat2) / 2) * DEG);
  return Math.hypot(x, lat2 - lat1) * KM_PER_DEG;
}

/** Weight of the vertical rate t seconds past a report: 1, then fading to 0 (level off). */
function verticalWeight(t: number): number {
  if (t < -BACKWARD_LIMIT_S) return 0;
  if (t <= VERTICAL_FADE_START_S) return 1;
  return Math.max(0, 1 - (t - VERTICAL_FADE_START_S) / (VERTICAL_HORIZON_S - VERTICAL_FADE_START_S));
}

/** Integral of `verticalWeight` over [0, t]: seconds of full vertical rate applied. */
function verticalIntegral(t: number): number {
  if (t <= VERTICAL_FADE_START_S) return Math.max(t, -BACKWARD_LIMIT_S);
  const span = VERTICAL_HORIZON_S - VERTICAL_FADE_START_S;
  const u = Math.min(t, VERTICAL_HORIZON_S) - VERTICAL_FADE_START_S;
  return VERTICAL_FADE_START_S + u - (u * u) / (2 * span);
}

/** Inverse of `verticalIntegral`: when `applied` seconds' worth is reached, or NaN if never. */
function verticalIntegralInverse(applied: number): number {
  if (applied <= VERTICAL_FADE_START_S) return Math.max(0, applied);
  const span = VERTICAL_HORIZON_S - VERTICAL_FADE_START_S;
  const rest = applied - VERTICAL_FADE_START_S;
  const disc = span * span - 2 * span * rest;
  return disc < 0 ? NaN : VERTICAL_FADE_START_S + span - Math.sqrt(disc);
}

/** Critically damped decay of a correction applied `t` seconds ago: 1 → 0 with zero slope at 0. */
function blend(t: number): number {
  if (t <= 0) return 1;
  if (t >= BLEND_END_S) return 0;
  const kt = BLEND_RATE * t;
  return (1 + kt) * Math.exp(-kt);
}

/** Weight of a track's measured values at time T: 1 while fresh, easing to 0 (the estimates) once stale. */
function measWeight(tr: Track, T: number): number {
  if (tr.measMs === -Infinity) return 0;
  return 1 - smoothstep((T - tr.measMs - MEAS_FRESH_MS) / MEAS_FADE_MS);
}

/** Whether measured values reported at `measMs` still count at time `t`. */
const measFresh = (tr: Track, t: number) => tr.meas !== null && t - tr.measMs <= MEAS_FRESH_MS;

/** Field elevation removal factor at `km` from the field. */
function fieldFade(km: number): number {
  return clamp((FADE_ZERO_KM - km) / (FADE_ZERO_KM - FADE_FULL_KM), 0, 1);
}

/** Pitch for flight at a vertical and ground speed in a phase; `agl` (ft) shapes the flare. */
function airPitch(vsFpm: number, speedMs: number, phase: Phase, agl: number): number {
  const gamma = Math.atan2(vsFpm * FPM_TO_MS, Math.max(speedMs, 10)) / DEG;
  let offset = PITCH_OFFSET[phase];
  // ease from the approach attitude into the flare's as the ground comes up
  if ((phase === "approach" || phase === "flare") && agl < FLARE_MAX_AGL_FT)
    offset = PITCH_OFFSET.approach + (PITCH_OFFSET.flare - PITCH_OFFSET.approach) * (1 - Math.max(0, agl) / FLARE_MAX_AGL_FT);
  return clamp(gamma + offset, MIN_PITCH, MAX_PITCH);
}

/** Per-aircraft state: samples, latest report, estimates, prediction and blending. */
class Track {
  touched = 0;
  cls: AircraftClass = "jet";
  icon = "";
  typecode = "";

  // latest report (all describe `reportMs`), from either source
  reportMs = -Infinity;
  /** Time of the newest Flightradar24 report taken in. */
  fr24Ms = -Infinity;
  lat0 = 0;
  lon0 = 0;
  alt = 0;
  speedKt = 0;
  trackDeg = 0;
  onGround = false;
  originAp: Airport | null = null;
  destAp: Airport | null = null;
  /** The relevant field for AGL and drawing height, or null. */
  field: Airport | null = null;
  fieldElev = 0;
  fieldKm = Infinity;

  // estimates
  accel = 0; // kt/s
  turnRate = 0; // deg/s, clockwise positive
  vrate = 0; // fpm
  vrRefMs = NaN;
  vrRefAlt = 0;
  vrRefGround = false;
  phase: Phase = "cruise";

  // horizontal samples, oldest first; x/y/mx/my are in the local frame around the newest
  n = 0;
  ts: number[] = [];
  lats: number[] = [];
  lons: number[] = [];
  xs: number[] = [];
  ys: number[] = [];
  mx: number[] = [];
  my: number[] = [];
  refLat = 0;
  refLon = 0;
  /** Per sample source (SRC_*), kept only once measured samples are mixed in. */
  src: number[] = [];
  mixed = false;

  // measured data (selected aircraft only)
  meas: Measured | null = null;
  /** When the measured values were reported; -Infinity without any. */
  measMs = -Infinity;
  /** Newest measured position taken in. */
  measPosMs = -Infinity;
  /** Nose heading minus track, degrees. */
  crab = 0;
  /** Measured bank minus the model's bank at `measMs`, degrees. */
  rollBias = 0;
  /** True airspeed, m/s (0: unknown). */
  tasMs = 0;
  /** Track (deg) the autopilot is turning onto, NaN if none. */
  navTrack = NaN;
  /** Altitude (ft) the climb or descent levels off at, NaN if none. */
  capAlt = NaN;
  /** Seconds past the newest sample when the predicted turn ends. */
  turnEndS = TURN_HORIZON_S;

  // extrapolation past the newest sample
  v0 = 0; // m/s
  h0 = 0; // rad
  kappa = 0; // 1/m
  sTurnMax = 0; // m
  segA: number[] = [];
  segDur: number[] = [];
  profEndV = 0;
  profEndT = 0;

  // predicted touchdown / lift-off, server ms (NaN when none)
  touchdownMs = NaN;
  touchdownPitch = 0;
  liftoffMs = NaN;

  // correction being blended out
  offMs = -Infinity;
  offE = 0;
  offN = 0;
  offAlt = 0;
  offHdg = 0;
  offTrk = 0;
  offPitch = 0;
  offBank = 0;

  // gear animation
  gearTarget = -1;
  gearFrom = 0;
  gearMs = -Infinity;

  constructor(readonly id: number) {}
}

/** Scratch result of evaluating a track's model at a time (no blending). */
interface Raw {
  lat: number;
  lon: number;
  altMsl: number;
  drawAlt: number;
  heading: number;
  track: number;
  pitch: number;
  bank: number;
  phase: Phase;
  speedMs: number;
  vsFpm: number;
  agl: number;
  ground: boolean;
}

const newRaw = (): Raw => ({
  lat: 0,
  lon: 0,
  altMsl: 0,
  drawAlt: 0,
  heading: 0,
  track: 0,
  pitch: 0,
  bank: 0,
  phase: "cruise",
  speedMs: 0,
  vsFpm: 0,
  agl: NaN,
  ground: false,
});

// Reused scratch space: pose() is called thousands of times per frame.
const scratchA = newRaw();
const scratchB = newRaw();
const scratchC = newRaw();
let profS = 0;
let profV = 0;

/** Distance (m) and speed (m/s) `dt` seconds along the track's speed profile, into profS/profV. */
function profile(tr: Track, dt: number): void {
  let s = 0;
  let t = 0;
  let v = tr.v0;
  for (let i = 0; i < tr.segA.length; i++) {
    const d = tr.segDur[i];
    const a = tr.segA[i];
    if (dt <= t + d) {
      const u = dt - t;
      profS = s + v * u + 0.5 * a * u * u;
      profV = v + a * u;
      return;
    }
    s += v * d + 0.5 * a * d * d;
    v += a * d;
    t += d;
  }
  profS = s + v * (dt - t);
  profV = v;
}

/** Appends a constant-acceleration piece (m/s²) to the speed profile, until `target` m/s or `maxDur` s. */
function ramp(tr: Track, a: number, target: number, maxDur: number): void {
  let dur = maxDur;
  if (a !== 0) dur = Math.min(dur, Math.max(0, (target - tr.profEndV) / a));
  if (!(dur > 0) || !Number.isFinite(dur)) return;
  tr.segA.push(a);
  tr.segDur.push(dur);
  tr.profEndV += a * dur;
  tr.profEndT += dur;
}

export class Motion {
  private tracks = new Map<number, Track>();
  private clockOffset = 0;
  private clockSynced = false;
  private lastPrune = -Infinity;
  private byIata = new Map<string, Airport>();
  private grid = new Map<number, Airport[]>();

  /** Server-synced clock, ms. Use this everywhere instead of Date.now(). */
  now(): number {
    return Date.now() + this.clockOffset;
  }

  /** Called with each snapshot's serverTimeMs on arrival; keep a smoothed offset to the local clock (ignore absurd values). */
  syncClock(serverTimeMs: number): void {
    if (!Number.isFinite(serverTimeMs) || serverTimeMs < MIN_SANE_TIME_MS) return;
    const sample = serverTimeMs - Date.now();
    // the first sample, or a real clock change, is taken as is; otherwise smooth out network jitter
    if (!this.clockSynced || Math.abs(sample - this.clockOffset) > CLOCK_RESYNC_MS) this.clockOffset = sample;
    else this.clockOffset += CLOCK_SMOOTHING * (sample - this.clockOffset);
    this.clockSynced = true;
  }

  setAirports(airports: Airport[]): void {
    this.byIata.clear();
    this.grid.clear();
    for (const ap of airports) {
      if (!Number.isFinite(ap.lat) || !Number.isFinite(ap.lon)) continue;
      if (ap.iata) this.byIata.set(ap.iata, ap);
      const key = this.cell(Math.floor(ap.lat / GRID_DEG), Math.floor(ap.lon / GRID_DEG));
      const list = this.grid.get(key);
      if (list) list.push(ap);
      else this.grid.set(key, [ap]);
    }
  }

  /** The pose to draw this aircraft at. Lazily ingests `f` as a new report when it is newer than what the track holds (timestampMs larger), so callers just pass whatever LiveFlight they have. Must be cheap: it is called per visible aircraft per frame (thousands per frame at 20-60 Hz); avoid per-call allocations beyond the returned object. */
  pose(f: LiveFlight, nowMs: number = this.now()): Pose {
    let tr = this.tracks.get(f.id);
    if (!tr) {
      tr = new Track(f.id);
      this.tracks.set(f.id, tr);
    }
    tr.touched = nowMs;
    if (tr.n === 0 || f.timestampMs > tr.fr24Ms) this.ingest(tr, f, nowMs);
    if (nowMs - this.lastPrune > PRUNE_INTERVAL_MS) this.prune(nowMs);

    const raw = this.evaluate(tr, nowMs, scratchA);
    let { lat, lon, drawAlt: altFt, heading, track, pitch, bank } = raw;
    const w = blend((nowMs - tr.offMs) / 1000);
    if (w > 0) {
      lat += (tr.offN * w) / M_PER_DEG;
      lon = wrapLon(lon + (tr.offE * w) / (M_PER_DEG * Math.max(Math.cos(lat * DEG), 1e-3)));
      altFt = Math.max(0, altFt + tr.offAlt * w);
      heading = wrap360(heading + tr.offHdg * w);
      track = wrap360(track + tr.offTrk * w);
      pitch = clamp(pitch + tr.offPitch * w, MIN_PITCH, MAX_PITCH);
      bank = clamp(bank + tr.offBank * w, -MAX_BANK, MAX_BANK);
    }
    return { lon, lat, altFt, heading, track, pitch, bank, gear: this.gear(tr, raw, nowMs), phase: raw.phase };
  }

  /**
   * Measured state for an aircraft (the selected one), from a second source;
   * `null` drops it. Call on each fetch, not per frame. Its position joins
   * the track's samples in time order; its heading, bank, turn and vertical
   * rates, airspeed and autopilot targets replace the estimates while fresh,
   * and every change is blended in like a new report, so nothing jumps.
   */
  measure(id: number, m: Measured | null, nowMs: number = this.now()): void {
    const tr = this.tracks.get(id);
    if (!tr || tr.n === 0) return; // no Flightradar24 report yet: the next fetch will do
    if (!m) {
      if (tr.meas === null && tr.measMs === -Infinity) return;
      this.blendAround(tr, nowMs, () => {
        tr.meas = null;
        tr.measMs = -Infinity;
        tr.crab = 0;
        tr.rollBias = 0;
        tr.tasMs = 0;
        tr.navTrack = NaN;
        this.refresh(tr);
      });
      return;
    }
    const posMs = m.positionMs;
    const positioned =
      typeof posMs === "number" && Number.isFinite(posMs) && Number.isFinite(m.lat) && Number.isFinite(m.lon);
    const newPosition = positioned && posMs! > tr.measPosMs;
    if (!newPosition && !(m.seenMs > tr.measMs)) return; // nothing new
    tr.touched = nowMs;
    this.blendAround(tr, nowMs, () => this.applyMeasured(tr, m, newPosition));
  }

  /** Same airport-relative drawing height for points of the aircraft's trail (so the trail meets the model). */
  drawAltitude(id: number, altFt: number, lat: number, lon: number): number {
    if (!(altFt > 0)) return 0;
    const tr = this.tracks.get(id);
    return tr ? Math.max(0, altFt - fieldRemoval(tr, lat, lon)) : altFt;
  }

  /** Drop tracks not touched for 15 min; call opportunistically from pose/ingest yourself too. */
  prune(nowMs: number): void {
    this.lastPrune = nowMs;
    for (const [id, tr] of this.tracks) if (nowMs - tr.touched > TRACK_TTL_MS) this.tracks.delete(id);
  }

  private cell(latCell: number, lonCell: number): number {
    const lonWrapped = ((lonCell % 360) + 360) % 360;
    return (latCell + 90) * 360 + lonWrapped;
  }

  /** Nearest airport within `maxKm` (small ranges only: searches the 3×3 grid cells around). */
  private nearestAirport(lat: number, lon: number, maxKm: number): Airport | null {
    const latCell = Math.floor(lat / GRID_DEG);
    const lonCell = Math.floor(lon / GRID_DEG);
    let best: Airport | null = null;
    let bestKm = maxKm;
    for (let i = -1; i <= 1; i++) {
      for (let j = -1; j <= 1; j++) {
        const list = this.grid.get(this.cell(latCell + i, lonCell + j));
        if (!list) continue;
        for (const ap of list) {
          const km = distanceKm(lat, lon, ap.lat, ap.lon);
          if (km < bestKm) {
            bestKm = km;
            best = ap;
          }
        }
      }
    }
    return best;
  }

  /** Takes a newer report into the track and blends from the old drawn pose to the new one. */
  private ingest(tr: Track, f: LiveFlight, nowMs: number): void {
    const first = tr.n === 0;
    const t = Number.isFinite(f.timestampMs) ? f.timestampMs : nowMs;
    if (first) this.update(tr, f, t, true);
    else this.blendAround(tr, nowMs, () => this.update(tr, f, t, false));
  }

  /** Applies `change` to an existing track, blending from the pose drawn before it to the one after. */
  private blendAround(tr: Track, nowMs: number, change: () => void): void {
    const before = scratchA;
    const previousMs = tr.reportMs;
    {
      // what is on screen right now, correction included
      this.evaluate(tr, nowMs, before);
      const w = blend((nowMs - tr.offMs) / 1000);
      before.lat += (tr.offN * w) / M_PER_DEG;
      before.lon = wrapLon(before.lon + (tr.offE * w) / (M_PER_DEG * Math.max(Math.cos(before.lat * DEG), 1e-3)));
      before.drawAlt = Math.max(0, before.drawAlt + tr.offAlt * w);
      before.heading = wrap360(before.heading + tr.offHdg * w);
      before.track = wrap360(before.track + tr.offTrk * w);
      before.pitch += tr.offPitch * w;
      before.bank += tr.offBank * w;
    }

    change();

    const after = this.evaluate(tr, nowMs, scratchB);
    const cosLat = Math.max(Math.cos(after.lat * DEG), 1e-3);
    const offE = wrap180(before.lon - after.lon) * M_PER_DEG * cosLat;
    const offN = (before.lat - after.lat) * M_PER_DEG;
    if (Math.hypot(offE, offN) > SNAP_DISTANCE_M || nowMs - previousMs > SNAP_AGE_MS) {
      tr.offMs = -Infinity;
      return;
    }
    tr.offMs = nowMs;
    tr.offE = offE;
    tr.offN = offN;
    tr.offAlt = before.drawAlt - after.drawAlt;
    tr.offHdg = wrap180(before.heading - after.heading);
    tr.offTrk = wrap180(before.track - after.track);
    tr.offPitch = before.pitch - after.pitch;
    tr.offBank = before.bank - after.bank;
  }

  /** Updates samples, estimates, phase and predictions from a report. */
  private update(tr: Track, f: LiveFlight, t: number, first: boolean): void {
    const dtReport = first ? NaN : (t - tr.reportMs) / 1000;
    const prevSpeed = tr.speedKt;
    const prevTrack = tr.trackDeg;
    const prevPhase: Phase | null = first ? null : tr.phase;

    if (f.icon !== tr.icon || f.typecode !== tr.typecode) {
      tr.icon = f.icon;
      tr.typecode = f.typecode;
      tr.cls = aircraftClass(f.icon, f.typecode);
    }
    tr.fr24Ms = t;
    if (!first && t < tr.reportMs) {
      // older than measured state already taken in: only its samples and route are news
      this.selectField(tr, f);
      if (mergeMixed(tr, f, t, true) !== null) this.refresh(tr);
      return;
    }
    tr.reportMs = t;
    tr.alt = Number.isFinite(f.alt) ? f.alt : 0;
    tr.speedKt = Number.isFinite(f.speed) ? Math.max(0, f.speed) : 0;
    tr.trackDeg = Number.isFinite(f.track) ? wrap360(f.track) : 0;
    tr.onGround = !!f.onGround;
    tr.lat0 = Number.isFinite(f.lat) ? clamp(f.lat, -90, 90) : 0;
    tr.lon0 = Number.isFinite(f.lon) ? wrapLon(f.lon) : 0;

    this.selectField(tr, f);
    const reportedOnly = tr.mixed ? mergeMixed(tr, f, t, false)! : mergeSamples(tr, f);

    // --- acceleration (kt/s): across reports and across the buffer
    const reportAccel =
      dtReport >= MIN_ACCEL_DT_S && dtReport <= MAX_ACCEL_DT_S ? (tr.speedKt - prevSpeed) / dtReport : NaN;
    const bufferAccel = bufferAcceleration(tr, t);
    let accel =
      Number.isFinite(reportAccel) && Number.isFinite(bufferAccel)
        ? (reportAccel + bufferAccel) / 2
        : Number.isFinite(reportAccel)
          ? reportAccel
          : bufferAccel;
    if (!Number.isFinite(accel)) accel = tr.accel * SMOOTHING; // no news: let it relax
    tr.accel = clamp(first ? accel : tr.accel + SMOOTHING * (accel - tr.accel), -MAX_ACCEL_KTS, MAX_ACCEL_KTS);

    // --- turn rate (deg/s): buffer chord headings when fast enough, else reported tracks
    const reportTurn =
      dtReport >= MIN_TURN_DT_S && dtReport <= MAX_TURN_DT_S ? wrap180(tr.trackDeg - prevTrack) / dtReport : NaN;
    const bufferTurn = tr.onGround ? NaN : bufferTurnRate(tr, t);
    let turn = Number.isFinite(bufferTurn) ? bufferTurn : Number.isFinite(reportTurn) ? reportTurn : 0;
    if (tr.onGround && tr.speedKt < PARKED_MAX_KT) turn = 0;
    tr.turnRate = clamp(first ? turn : tr.turnRate + SMOOTHING * (turn - tr.turnRate), -MAX_TURN_DEG_S, MAX_TURN_DEG_S);

    // --- vertical rate (fpm): reported when signed in, else altitude differences between reports
    if (typeof f.vspeed === "number" && Number.isFinite(f.vspeed)) {
      tr.vrate = clamp(f.vspeed, -MAX_VRATE_FPM, MAX_VRATE_FPM);
      tr.vrRefMs = t;
      tr.vrRefAlt = tr.alt;
      tr.vrRefGround = tr.onGround;
    } else if (tr.onGround) {
      tr.vrate = 0;
      tr.vrRefMs = t;
      tr.vrRefAlt = tr.alt;
      tr.vrRefGround = true;
    } else {
      const dt = (t - tr.vrRefMs) / 1000;
      if (!(dt >= MIN_VRATE_DT_S)) {
        if (!Number.isFinite(tr.vrRefMs)) {
          tr.vrRefMs = t;
          tr.vrRefAlt = tr.alt;
          tr.vrRefGround = false;
        }
      } else {
        // FR24 reports 0 ft on the ground: a lift-off climbs from the field
        const refAlt = tr.vrRefGround ? (tr.field ? tr.fieldElev : NaN) : tr.vrRefAlt;
        if (dt <= MAX_VRATE_DT_S && Number.isFinite(refAlt)) {
          const vr = ((tr.alt - refAlt) / dt) * 60;
          const fresh = prevPhase === null || tr.vrRefGround;
          tr.vrate = clamp(fresh ? vr : tr.vrate + SMOOTHING * (vr - tr.vrate), -MAX_VRATE_FPM, MAX_VRATE_FPM);
        } else if (dt > MAX_VRATE_DT_S) tr.vrate = 0;
        tr.vrRefMs = t;
        tr.vrRefAlt = tr.alt;
        tr.vrRefGround = false;
      }
    }

    this.measuredRates(tr, t);
    tr.phase = decidePhase(tr, prevPhase);
    computeTangents(tr, reportedOnly);
    this.predict(tr);
    this.measuredAttitude(tr);
  }

  /** Re-derives the extrapolation after samples or estimates changed without a newer report. */
  private refresh(tr: Track): void {
    computeTangents(tr, tr.ts[tr.n - 1] === tr.reportMs);
    this.predict(tr);
    this.measuredAttitude(tr);
  }

  /** Takes in a measured state; a newer position also becomes the track's latest report. */
  private applyMeasured(tr: Track, m: Measured, newPosition: boolean): void {
    tr.meas = m;
    tr.measMs = Math.max(tr.measMs, m.seenMs);
    const prevPhase = tr.phase;
    if (newPosition) {
      const t = m.positionMs!;
      tr.measPosMs = t;
      const inserted = mergeMeasured(tr, t, m.lat!, m.lon!, m.mlat ? SRC_MLAT : SRC_ADSB);
      if (inserted && t > tr.reportMs) {
        const dtReport = (t - tr.reportMs) / 1000;
        const prevSpeed = tr.speedKt;
        tr.reportMs = t;
        tr.lat0 = clamp(m.lat!, -90, 90);
        tr.lon0 = wrapLon(m.lon!);
        tr.onGround = !!m.onGround;
        // Flightradar24 reports 0 ft on the ground too
        if (tr.onGround) tr.alt = 0;
        else if (Number.isFinite(m.altBaro)) tr.alt = m.altBaro!;
        if (Number.isFinite(m.gs)) tr.speedKt = Math.max(0, m.gs!);
        if (Number.isFinite(m.track)) tr.trackDeg = wrap360(m.track!);
        this.selectField(tr, null);
        if (dtReport >= MIN_ACCEL_DT_S && dtReport <= MAX_ACCEL_DT_S) {
          const accel = (tr.speedKt - prevSpeed) / dtReport;
          tr.accel = clamp(tr.accel + SMOOTHING * (accel - tr.accel), -MAX_ACCEL_KTS, MAX_ACCEL_KTS);
        }
      }
    }
    this.measuredRates(tr, tr.reportMs);
    tr.phase = decidePhase(tr, prevPhase);
    this.refresh(tr);
  }

  /** Measured turn and vertical rates, airspeed, crab and selected heading replace the estimates while fresh at `t`. */
  private measuredRates(tr: Track, t: number): void {
    const m = tr.meas;
    if (!m) return;
    const has = (x: number | undefined): x is number => typeof x === "number" && Number.isFinite(x);
    const track = has(m.track) ? m.track : tr.trackDeg;
    tr.crab = has(m.trueHeading) && !tr.onGround ? clamp(wrap180(m.trueHeading - track), -MAX_CRAB, MAX_CRAB) : 0;
    tr.tasMs = has(m.tas) && m.tas > 0 ? m.tas * KNOTS_TO_MS : 0;
    tr.navTrack = NaN;
    if (!measFresh(tr, t) || tr.onGround) return;
    let turn = NaN;
    if (has(m.trackRate)) turn = m.trackRate;
    else if (has(m.roll) && tr.speedKt > 60) {
      // a coordinated turn: rate = g tan(bank) / speed
      turn = (GRAVITY_MS2 * Math.tan(clamp(m.roll, -60, 60) * DEG)) / (tr.speedKt * KNOTS_TO_MS) / DEG;
    }
    if (Number.isFinite(turn)) tr.turnRate = clamp(turn, -MAX_TURN_DEG_S, MAX_TURN_DEG_S);
    const vr = has(m.baroRate) ? m.baroRate : m.geomRate;
    if (has(vr)) {
      tr.vrate = clamp(vr, -MAX_VRATE_FPM, MAX_VRATE_FPM);
      tr.vrRefMs = t;
      tr.vrRefAlt = tr.alt;
      tr.vrRefGround = false;
    }
    // the selected heading steers only when the autopilot isn't following a route (LNAV);
    // it is magnetic, so it needs the variation the two reported headings give
    const lnav = m.navModes?.some((mode) => mode.toLowerCase() === "lnav") ?? false;
    if (has(m.navHeading) && has(m.trueHeading) && has(m.magHeading) && !lnav) {
      const variation = wrap180(m.trueHeading - m.magHeading);
      tr.navTrack = wrap360(m.navHeading + variation - tr.crab);
    }
  }

  /** The measured bank as an offset to the model's own at the time it was reported. */
  private measuredAttitude(tr: Track): void {
    tr.rollBias = 0;
    const roll = tr.meas?.roll;
    if (typeof roll !== "number" || !Number.isFinite(roll) || tr.onGround) return;
    const raw = this.evaluate(tr, tr.measMs, scratchC);
    if (!raw.ground) tr.rollBias = clamp(roll, -MAX_BANK, MAX_BANK) - raw.bank;
  }

  /** Destination within 40 km, else origin within 40 km, else the nearest within 15 km, else the previous one while within 60 km. */
  private selectField(tr: Track, f: LiveFlight | null): void {
    if (f) {
      tr.originAp = (f.origin && this.byIata.get(f.origin)) || null;
      tr.destAp = (f.destination && this.byIata.get(f.destination)) || null;
    }
    const lat = tr.lat0;
    const lon = tr.lon0;
    const within = (ap: Airport | null, range: number) =>
      ap !== null && distanceKm(lat, lon, ap.lat, ap.lon) <= range ? ap : null;
    const field =
      within(tr.destAp, FIELD_RANGE_KM) ??
      within(tr.originAp, FIELD_RANGE_KM) ??
      this.nearestAirport(lat, lon, NEAREST_FIELD_KM) ??
      within(tr.field, FADE_ZERO_KM);
    tr.field = field;
    tr.fieldKm = field ? distanceKm(lat, lon, field.lat, field.lon) : Infinity;
    tr.fieldElev = field ? field.alt ?? 0 : 0;
  }

  /** Extrapolation start, speed profile and touchdown / lift-off predictions. */
  private predict(tr: Track): void {
    const last = tr.n - 1;
    const parked = tr.phase === "parked";
    const vx = tr.mx[last];
    const vy = tr.my[last];
    const speed = Math.min(Math.hypot(vx, vy), MAX_SPEED_MS);
    tr.h0 = speed > HEADING_MIN_SPEED_MS && !parked ? Math.atan2(vx, vy) : tr.trackDeg * DEG;
    tr.v0 = parked ? 0 : speed;
    tr.kappa = tr.v0 > HEADING_MIN_SPEED_MS ? clamp((tr.turnRate * DEG) / tr.v0, -MAX_CURVATURE, MAX_CURVATURE) : 0;

    tr.segA.length = 0;
    tr.segDur.length = 0;
    tr.profEndV = tr.v0;
    tr.profEndT = 0;
    tr.touchdownMs = NaN;
    tr.liftoffMs = NaN;
    const tLast = tr.ts[last];
    const accel = tr.accel * KNOTS_TO_MS;
    const v0 = tr.v0;

    switch (tr.phase) {
      case "parked":
        break;
      case "taxi": {
        const a = clamp(accel, -MAX_TAXI_ACCEL_KTS * KNOTS_TO_MS, MAX_TAXI_ACCEL_KTS * KNOTS_TO_MS);
        ramp(tr, a, a > 0 ? Math.max(v0, MAX_TAXI_SPEED_KT * KNOTS_TO_MS) : 0, ACCEL_HORIZON_S);
        break;
      }
      case "takeoffRoll": {
        if (tr.cls === "heli") break;
        const rotation = ROTATION_KT[tr.cls];
        ramp(tr, TAKEOFF_ACCEL_KTS * KNOTS_TO_MS, (rotation + TAKEOFF_OVERSPEED_KT) * KNOTS_TO_MS, Infinity);
        tr.liftoffMs = tr.reportMs + (Math.max(0, rotation - tr.speedKt) / TAKEOFF_ACCEL_KTS) * 1000;
        break;
      }
      case "landingRoll":
        ramp(tr, Math.min(accel, -LANDING_DECEL_KTS * KNOTS_TO_MS), ROLLOUT_KT * KNOTS_TO_MS, Infinity);
        break;
      default: {
        // airborne: does the descent reach the field within the vertical horizon?
        if ((tr.phase === "approach" || tr.phase === "flare") && tr.field && tr.vrate < 0) {
          const needed = Math.max(0, (tr.alt - tr.fieldElev) / (-tr.vrate / 60));
          const t = verticalIntegralInverse(needed);
          if (Number.isFinite(t)) tr.touchdownMs = tr.reportMs + t * 1000;
        }
        const change = MAX_AIR_SPEED_CHANGE_KT * KNOTS_TO_MS;
        const untilTouchdown = Number.isFinite(tr.touchdownMs) ? Math.max(0, (tr.touchdownMs - tLast) / 1000) : Infinity;
        ramp(tr, accel, accel > 0 ? v0 + change : Math.max(0, v0 - change), Math.min(ACCEL_HORIZON_S, untilTouchdown));
        if (Number.isFinite(untilTouchdown)) {
          ramp(tr, 0, 0, untilTouchdown - tr.profEndT);
          if (tr.profEndV > ROLLOUT_KT * KNOTS_TO_MS) ramp(tr, -LANDING_DECEL_KTS * KNOTS_TO_MS, ROLLOUT_KT * KNOTS_TO_MS, Infinity);
          // the attitude at touchdown, which the rollout eases down from
          profile(tr, untilTouchdown);
          const vs = tr.vrate * verticalWeight((tr.touchdownMs - tr.reportMs) / 1000);
          tr.touchdownPitch = Math.max(0, airPitch(vs, profV, "flare", 0));
        }
      }
    }
    profile(tr, TURN_HORIZON_S);
    tr.sTurnMax = profS;

    // measured autopilot targets: a turn onto the selected heading stops there,
    // and a climb or descent levels off at the selected altitude
    tr.turnEndS = TURN_HORIZON_S;
    if (Number.isFinite(tr.navTrack) && tr.kappa !== 0 && tr.v0 > HEADING_MIN_SPEED_MS && !GROUND.has(tr.phase)) {
      const togo = wrap180(tr.navTrack - tr.h0 / DEG);
      if (Math.sign(togo) === Math.sign(tr.kappa) || Math.abs(togo) < 1) {
        const s = Math.abs((togo * DEG) / tr.kappa);
        if (s < tr.sTurnMax) {
          tr.sTurnMax = s;
          tr.turnEndS = s / tr.v0;
        }
      }
    }
    tr.capAlt = captureAltitude(tr);
  }

  /** The model at time T (server ms), without blending, into `out`. */
  private evaluate(tr: Track, T: number, out: Raw): Raw {
    // --- horizontal
    const n = tr.n;
    const tLast = tr.ts[n - 1];
    let x: number;
    let y: number;
    let vx: number;
    let vy: number;
    let turnRad = tr.turnRate * DEG; // rad/s
    let bankFade = 1;
    let pathHeading = tr.h0; // heading to keep when the aircraft stops
    if (T >= tLast) {
      let dt = (T - tLast) / 1000;
      const held = dt >= MAX_EXTRAPOLATION_S;
      if (held) dt = MAX_EXTRAPOLATION_S;
      profile(tr, dt);
      const s = profS;
      const v = held ? 0 : Math.max(0, profV);
      const k = tr.kappa;
      const h0 = tr.h0;
      const sTurn = Math.min(s, tr.sTurnMax);
      const h = h0 + k * sTurn;
      if (Math.abs(k * sTurn) < 1e-9) {
        x = sTurn * Math.sin(h0);
        y = sTurn * Math.cos(h0);
      } else {
        // constant-curvature arc: d(east)/ds = sin h, d(north)/ds = cos h, h = h0 + k s
        x = (Math.cos(h0) - Math.cos(h)) / k;
        y = (Math.sin(h) - Math.sin(h0)) / k;
      }
      x += (s - sTurn) * Math.sin(h);
      y += (s - sTurn) * Math.cos(h);
      vx = v * Math.sin(h);
      vy = v * Math.cos(h);
      pathHeading = h;
      if (tr.turnEndS < TURN_HORIZON_S) {
        // rolling out onto the selected heading
        turnRad = dt < tr.turnEndS ? k * v : 0;
        bankFade = smoothstep((tr.turnEndS - dt) / ROLL_OUT_S);
      } else {
        turnRad = dt < TURN_HORIZON_S ? k * v : 0;
        bankFade = dt <= BANK_FADE_START_S ? 1 : Math.max(0, (TURN_HORIZON_S - dt) / (TURN_HORIZON_S - BANK_FADE_START_S));
      }
    } else if (T <= tr.ts[0]) {
      const dt = Math.max((T - tr.ts[0]) / 1000, -BACKWARD_LIMIT_S);
      x = tr.xs[0] + tr.mx[0] * dt;
      y = tr.ys[0] + tr.my[0] * dt;
      vx = tr.mx[0];
      vy = tr.my[0];
    } else {
      // cubic Hermite between samples i and i+1
      let i = n - 2;
      while (i > 0 && T < tr.ts[i]) i--;
      const h = (tr.ts[i + 1] - tr.ts[i]) / 1000;
      const u = (T - tr.ts[i]) / 1000 / h;
      const u2 = u * u;
      const u3 = u2 * u;
      const h00 = 2 * u3 - 3 * u2 + 1;
      const h10 = u3 - 2 * u2 + u;
      const h01 = 3 * u2 - 2 * u3;
      const h11 = u3 - u2;
      const d00 = 6 * u2 - 6 * u;
      const d10 = 3 * u2 - 4 * u + 1;
      const d11 = 3 * u2 - 2 * u;
      x = h00 * tr.xs[i] + h10 * h * tr.mx[i] + h01 * tr.xs[i + 1] + h11 * h * tr.mx[i + 1];
      y = h00 * tr.ys[i] + h10 * h * tr.my[i] + h01 * tr.ys[i + 1] + h11 * h * tr.my[i + 1];
      vx = (d00 * (tr.xs[i] - tr.xs[i + 1])) / h + d10 * tr.mx[i] + d11 * tr.mx[i + 1];
      vy = (d00 * (tr.ys[i] - tr.ys[i + 1])) / h + d10 * tr.my[i] + d11 * tr.my[i + 1];
    }
    const lat = clamp(tr.refLat + y / M_PER_DEG, -90, 90);
    const cosMid = Math.max(Math.cos(((tr.refLat + lat) / 2) * DEG), 1e-3);
    out.lat = lat;
    out.lon = wrapLon(tr.refLon + x / (M_PER_DEG * cosMid));
    const speed = Math.hypot(vx, vy);
    out.speedMs = speed;
    out.track =
      tr.phase === "parked"
        ? tr.trackDeg
        : wrap360((speed > HEADING_MIN_SPEED_MS ? Math.atan2(vx, vy) : pathHeading) / DEG);
    // measured values count while fresh (the nose's crab, the bank, the airspeed), then ease out
    const mw = measWeight(tr, T);
    out.heading = mw > 0 ? wrap360(out.track + tr.crab * mw) : out.track;

    // --- vertical, phase and attitude
    let phase = tr.phase;
    const field = tr.field;
    const floor = field ? tr.fieldElev : Math.min(0, tr.alt);
    out.bank = 0;
    out.vsFpm = 0;
    out.agl = NaN;
    if (GROUND.has(phase)) {
      out.ground = true;
      out.altMsl = field ? tr.fieldElev : 0;
      out.pitch = 0;
      if (phase === "landingRoll" && speed < TAXI_MAX_KT * KNOTS_TO_MS) phase = "taxi";
      const lo = tr.liftoffMs;
      if (phase === "takeoffRoll" && Number.isFinite(lo)) {
        if (T < lo) out.pitch = ROTATION_PITCH * smoothstep(1 - (lo - T) / (ROTATION_S * 1000));
        else {
          // predicted lift-off and initial climb
          const u = (T - lo) / 1000;
          const vs = TAKEOFF_CLIMB_FPM * verticalWeight(u);
          out.ground = false;
          out.agl = (TAKEOFF_CLIMB_FPM * verticalIntegral(u)) / 60;
          out.altMsl = Math.min(out.altMsl + out.agl, MAX_ALT_FT);
          out.vsFpm = vs;
          phase = out.agl < APPROACH_MAX_AGL_FT ? "initialClimb" : "climb";
          const settled = airPitch(vs, speed, phase, out.agl);
          out.pitch = ROTATION_PITCH + (settled - ROTATION_PITCH) * smoothstep(u / ROTATION_S);
        }
      }
    } else {
      const td = tr.touchdownMs;
      if (Number.isFinite(td) && T >= td) {
        // predicted landing roll
        out.ground = true;
        out.altMsl = tr.fieldElev;
        out.agl = 0;
        phase = speed < TAXI_MAX_KT * KNOTS_TO_MS ? "taxi" : "landingRoll";
        out.pitch = tr.touchdownPitch * (1 - smoothstep((T - td) / (TOUCHDOWN_EASE_S * 1000)));
      } else {
        const dtR = (T - tr.reportMs) / 1000;
        let climb = (tr.vrate * verticalIntegral(dtR)) / 60;
        let vsWeight = verticalWeight(dtR);
        const cap = tr.capAlt;
        if (cap === cap) {
          // altitude capture: ease onto the selected altitude (C1: the rate goes smoothly to 0)
          const d = cap - tr.alt;
          const band = clamp(Math.abs(tr.vrate) * CAPTURE_FT_PER_FPM, MIN_CAPTURE_FT, MAX_CAPTURE_FT);
          const kk = Math.min(1, band / Math.abs(d));
          const x = climb / d;
          if (x >= 1 + kk) {
            climb = d;
            vsWeight = 0;
          } else if (x > 1 - kk) {
            const u = x - (1 - kk);
            climb = d * (x - (u * u) / (4 * kk));
            vsWeight *= 1 - u / (2 * kk);
          }
        }
        const alt = tr.alt + climb;
        out.ground = false;
        out.altMsl = clamp(alt, floor, MAX_ALT_FT);
        out.vsFpm = alt === out.altMsl ? tr.vrate * vsWeight : 0;
        if (field) {
          out.agl = out.altMsl - tr.fieldElev;
          if (phase === "approach" && out.agl < FLARE_MAX_AGL_FT) phase = "flare";
        }
        // the flight-path angle is through the air: true airspeed when measured
        const airSpeed = mw > 0 && tr.tasMs > 0 ? speed + (tr.tasMs - speed) * mw : speed;
        out.pitch = airPitch(out.vsFpm, airSpeed, phase, out.agl);
        const bank = Math.atan2(speed * turnRad, GRAVITY_MS2) / DEG;
        out.bank = clamp(bank * bankFade + tr.rollBias * mw, -MAX_BANK, MAX_BANK);
      }
    }
    out.phase = phase;
    if (out.ground) out.drawAlt = 0;
    else if (Number.isFinite(tr.liftoffMs) && !field) out.drawAlt = out.agl; // predicted climb off an unknown field
    else out.drawAlt = Math.max(0, out.altMsl - fieldRemoval(tr, out.lat, out.lon));
    return out;
  }

  /** Gear fraction, animating towards the target for this pose. */
  private gear(tr: Track, raw: Raw, nowMs: number): number {
    let down: boolean;
    const phase = raw.phase;
    if (raw.ground || GROUND.has(phase)) down = true;
    else if (tr.field) {
      down =
        ((phase === "approach" || phase === "flare") && raw.agl < GEAR_APPROACH_AGL_FT) ||
        (phase === "initialClimb" && raw.agl < GEAR_CLIMB_AGL_FT);
    } else {
      down =
        raw.altMsl < GEAR_FALLBACK_ALT_FT &&
        raw.speedMs < GEAR_FALLBACK_KT * KNOTS_TO_MS &&
        raw.vsFpm < APPROACH_MAX_VRATE_FPM;
    }
    const target = down ? 1 : 0;
    if (tr.gearTarget < 0) {
      // a new track starts with its gear where it should be
      tr.gearTarget = target;
      tr.gearFrom = target;
      tr.gearMs = -Infinity;
      return target;
    }
    const current = gearAt(tr, nowMs);
    if (target !== tr.gearTarget) {
      tr.gearFrom = current;
      tr.gearTarget = target;
      tr.gearMs = nowMs;
    }
    return current;
  }
}

/** Gear fraction at a time, given the last target change. */
function gearAt(tr: Track, nowMs: number): number {
  const t = (nowMs - tr.gearMs) / GEAR_TRANSIT_MS;
  if (t >= 1) return tr.gearTarget;
  if (t <= 0) return tr.gearFrom;
  return tr.gearFrom + (tr.gearTarget - tr.gearFrom) * smoothstep(t);
}

/** The selected altitude a climb or descent is heading for, from fresh measured data; NaN if none. */
function captureAltitude(tr: Track): number {
  const m = tr.meas;
  if (!m || !measFresh(tr, tr.reportMs) || GROUND.has(tr.phase) || tr.phase === "approach" || tr.phase === "flare")
    return NaN;
  if (Math.abs(tr.vrate) < 100) return NaN;
  let best = NaN;
  for (const c of [m.navAltitudeMcp, m.navAltitudeFms]) {
    if (typeof c !== "number" || !Number.isFinite(c) || c <= 0) continue;
    const d = c - tr.alt;
    // only a target ahead of the climb or descent; one behind it is no level-off
    if (d * tr.vrate < 0 || Math.abs(d) < 1) continue;
    if (!(Math.abs(d) >= Math.abs(best - tr.alt))) best = c;
  }
  return best;
}

/** Feet of field elevation to remove at a point: the track's field, else the nearer of origin and destination. */
function fieldRemoval(tr: Track, lat: number, lon: number): number {
  if (tr.field) {
    const fade = fieldFade(distanceKm(lat, lon, tr.field.lat, tr.field.lon));
    if (fade > 0) return tr.fieldElev * fade;
  }
  let best = 0;
  let bestKm = FADE_ZERO_KM;
  for (let k = 0; k < 2; k++) {
    const ap = k === 0 ? tr.originAp : tr.destAp;
    if (!ap) continue;
    const km = distanceKm(lat, lon, ap.lat, ap.lon);
    if (km < bestKm) {
      bestKm = km;
      best = (ap.alt ?? 0) * fieldFade(km);
    }
  }
  return best;
}

/**
 * Merges the report point and its look-ahead buffer into the track's samples:
 * the new samples replace old ones from shortly before the report on, and only
 * the last minute is kept. Returns whether the newest sample is the report
 * point itself (no look-ahead), whose velocity is then the reported one.
 */
function mergeSamples(tr: Track, f: LiveFlight): boolean {
  const t = tr.reportMs;
  // in place: this runs for every aircraft in every snapshot
  const { ts, lats, lons } = tr;
  let n = 0;
  while (n < tr.n && ts[n] < t - MERGE_EPSILON_MS) n++;
  // an old sample far off this report's path (a jump in the data) poisons the spline: start over
  if (n) {
    const gap = (t - ts[n - 1]) / 1000;
    const km = distanceKm(lats[n - 1], lons[n - 1], tr.lat0, tr.lon0);
    const plausible = (Math.max(tr.speedKt, 250) * 2 * KNOTS_TO_MS * gap) / 1000 + 0.5;
    if (km > plausible || km * 1000 > SNAP_DISTANCE_M) n = 0;
  }
  ts.length = lats.length = lons.length = n;
  // old samples all end before t - MERGE_EPSILON_MS, so the report point always goes in
  ts.push(t);
  lats.push(tr.lat0);
  lons.push(tr.lon0);
  let buffered = false;
  const positions = f.positions;
  if (positions) {
    for (let i = 0; i < positions.length; i++) {
      const [dLat, dLon, dMs] = positions[i];
      const time = t + dMs;
      const lat = tr.lat0 + dLat * BUFFER_SCALE;
      const lon = tr.lon0 + dLon * BUFFER_SCALE;
      if (!(time - ts[ts.length - 1] >= MIN_SAMPLE_GAP_MS) || !Number.isFinite(lat) || !Number.isFinite(lon)) continue;
      ts.push(time);
      lats.push(lat);
      lons.push(lon);
      buffered = true;
    }
  }
  const newest = ts[ts.length - 1];
  let start = 0;
  while (start < ts.length - 1 && ts[start] < newest - SAMPLE_WINDOW_MS) start++;
  if (start) {
    ts.splice(0, start);
    lats.splice(0, start);
    lons.splice(0, start);
  }
  n = tr.n = ts.length;

  // local frame around the newest sample
  tr.refLat = lats[n - 1];
  tr.refLon = lons[n - 1];
  const cosRef = Math.max(Math.cos(tr.refLat * DEG), 1e-3);
  tr.xs.length = tr.ys.length = n;
  for (let i = 0; i < n; i++) {
    tr.xs[i] = wrap180(lons[i] - tr.refLon) * M_PER_DEG * cosRef;
    tr.ys[i] = (lats[i] - tr.refLat) * M_PER_DEG;
  }
  return !buffered;
}

/** Distance (km) a sample may plausibly be from another `gapS` seconds away, at the track's speed. */
const plausibleKm = (tr: Track, gapS: number) => (Math.max(tr.speedKt, 250) * 2 * KNOTS_TO_MS * Math.abs(gapS)) / 1000 + 0.5;

/** Starts the per-sample sources when measured samples first join a track: all so far are Flightradar24's. */
function startMixing(tr: Track): void {
  if (tr.mixed) return;
  tr.src.length = 0;
  for (let i = 0; i < tr.n; i++) tr.src.push(SRC_FR24);
  tr.mixed = true;
}

/**
 * Puts a sample into a mixed track in time order. Samples within a second of
 * it describe the same moment: the better source wins (ADS-B over
 * Flightradar24 over MLAT), and of equals the newer arrival. Returns whether
 * it went in.
 */
function insertSample(tr: Track, t: number, lat: number, lon: number, src: number): boolean {
  const { ts, lats, lons } = tr;
  for (let j = 0; j < ts.length; j++) if (Math.abs(ts[j] - t) < COINCIDENT_MS && tr.src[j] > src) return false;
  for (let j = ts.length - 1; j >= 0; j--) {
    if (Math.abs(ts[j] - t) < COINCIDENT_MS) {
      ts.splice(j, 1);
      lats.splice(j, 1);
      lons.splice(j, 1);
      tr.src.splice(j, 1);
    }
  }
  let i = ts.length;
  while (i > 0 && ts[i - 1] > t) i--;
  ts.splice(i, 0, t);
  lats.splice(i, 0, lat);
  lons.splice(i, 0, lon);
  tr.src.splice(i, 0, src);
  return true;
}

/** Index of the sample nearest in time to `t`, or -1. */
function nearestSample(tr: Track, t: number): number {
  let best = -1;
  for (let j = 0; j < tr.ts.length; j++) if (best < 0 || Math.abs(tr.ts[j] - t) < Math.abs(tr.ts[best] - t)) best = j;
  return best;
}

/** Drops all samples (a jump in the data: they would poison the spline). */
function clearSamples(tr: Track): void {
  tr.ts.length = tr.lats.length = tr.lons.length = tr.src.length = 0;
}

/** Window trim, sample count, source bookkeeping and the local frame, after a mixed merge. */
function finishSamples(tr: Track): void {
  const { ts, lats, lons } = tr;
  const newest = ts[ts.length - 1];
  let start = 0;
  while (start < ts.length - 1 && ts[start] < newest - SAMPLE_WINDOW_MS) start++;
  if (start) {
    ts.splice(0, start);
    lats.splice(0, start);
    lons.splice(0, start);
    tr.src.splice(0, start);
  }
  const n = (tr.n = ts.length);
  tr.mixed = tr.src.some((s) => s !== SRC_FR24);
  tr.refLat = lats[n - 1];
  tr.refLon = lons[n - 1];
  const cosRef = Math.max(Math.cos(tr.refLat * DEG), 1e-3);
  tr.xs.length = tr.ys.length = n;
  for (let i = 0; i < n; i++) {
    tr.xs[i] = wrap180(lons[i] - tr.refLon) * M_PER_DEG * cosRef;
    tr.ys[i] = (lats[i] - tr.refLat) * M_PER_DEG;
  }
}

/**
 * `mergeSamples` for a track that also has measured samples: this report's
 * point and buffer replace Flightradar24's own samples from shortly before it
 * on, and interleave with the measured ones by time. Returns whether the
 * newest sample is the report point itself. A report that jumps away from
 * the samples starts them over, unless it is `older` than the measured state
 * (which then wins): it is dropped, and the result is null.
 */
function mergeMixed(tr: Track, f: LiveFlight, t: number, older: boolean): boolean | null {
  startMixing(tr);
  const lat0 = Number.isFinite(f.lat) ? clamp(f.lat, -90, 90) : 0;
  const lon0 = Number.isFinite(f.lon) ? wrapLon(f.lon) : 0;
  const { ts, lats, lons, src } = tr;
  if (older) {
    const near = nearestSample(tr, t);
    if (near >= 0 && distanceKm(lats[near], lons[near], lat0, lon0) > Math.min(plausibleKm(tr, (t - ts[near]) / 1000), SNAP_DISTANCE_M / 1000))
      return null;
  }
  let kept = 0;
  for (let i = 0; i < ts.length; i++) {
    if (src[i] === SRC_FR24 && ts[i] >= t - MERGE_EPSILON_MS) continue;
    ts[kept] = ts[i];
    lats[kept] = lats[i];
    lons[kept] = lons[i];
    src[kept] = src[i];
    kept++;
  }
  ts.length = lats.length = lons.length = src.length = kept;
  const near = nearestSample(tr, t);
  if (near >= 0 && distanceKm(lats[near], lons[near], lat0, lon0) > Math.min(plausibleKm(tr, (t - ts[near]) / 1000), SNAP_DISTANCE_M / 1000))
    clearSamples(tr);
  insertSample(tr, t, lat0, lon0, SRC_FR24);
  let last = t;
  const positions = f.positions;
  if (positions) {
    for (let i = 0; i < positions.length; i++) {
      const [dLat, dLon, dMs] = positions[i];
      const time = t + dMs;
      const lat = lat0 + dLat * BUFFER_SCALE;
      const lon = lon0 + dLon * BUFFER_SCALE;
      if (!(time - last >= MIN_SAMPLE_GAP_MS) || !Number.isFinite(lat) || !Number.isFinite(lon)) continue;
      last = time;
      insertSample(tr, time, lat, lon, SRC_FR24);
    }
  }
  finishSamples(tr);
  return tr.ts[tr.n - 1] === t;
}

/**
 * A measured position joins the samples in time order. One that jumps away
 * from them starts them over if it is the newest data, and is dropped if not.
 * Returns whether it went in.
 */
function mergeMeasured(tr: Track, t: number, lat: number, lon: number, src: number): boolean {
  const near = nearestSample(tr, t);
  if (
    near >= 0 &&
    distanceKm(tr.lats[near], tr.lons[near], lat, lon) >
      Math.min(plausibleKm(tr, (t - tr.ts[near]) / 1000), SNAP_DISTANCE_M / 1000)
  ) {
    if (t <= tr.reportMs) return false;
    startMixing(tr);
    clearSamples(tr);
  }
  startMixing(tr);
  const inserted = insertSample(tr, t, clamp(lat, -90, 90), wrapLon(lon), src);
  finishSamples(tr);
  return inserted;
}

/**
 * Velocities at the samples for the Hermite spline: the three-point
 * derivative inside (exact for a parabola), and at the ends the chord turned
 * by half its heading change, which is the tangent of a circular arc. The
 * newest sample's chord spans this report's whole buffer (positions are
 * quantised to ~1 m, too coarse for one 2 s chord to give a speed that holds
 * for minutes of extrapolation); if it is the report point itself, it takes
 * the reported velocity.
 */
function computeTangents(tr: Track, reportedOnly: boolean): void {
  const n = tr.n;
  const parked = tr.phase === "parked";
  const omega = tr.turnRate * DEG;
  tr.mx.length = tr.my.length = n;
  const reportedV = parked ? 0 : tr.speedKt * KNOTS_TO_MS;
  const reportedX = reportedV * Math.sin(tr.trackDeg * DEG);
  const reportedY = reportedV * Math.cos(tr.trackDeg * DEG);
  if (n === 1) {
    tr.mx[0] = reportedX;
    tr.my[0] = reportedY;
    return;
  }
  for (let i = 0; i < n; i++) {
    if (i === 0 || i === n - 1) {
      if (i === n - 1 && reportedOnly) {
        tr.mx[i] = reportedX;
        tr.my[i] = reportedY;
        continue;
      }
      let a = 0;
      let b = 1;
      if (i === n - 1) {
        b = n - 1;
        a = b - 1;
        while (a > 0 && tr.ts[a - 1] >= tr.reportMs) a--;
      }
      const dt = (tr.ts[b] - tr.ts[a]) / 1000;
      // rotate the chord clockwise by ±ω·dt/2 onto the arc's end tangent, and
      // lengthen it from chord to arc length
      const th = ((i === 0 ? -1 : 1) * omega * dt) / 2;
      const arc = Math.abs(th) > 1e-6 ? Math.abs(th) / Math.sin(Math.abs(th)) : 1;
      const cx = ((tr.xs[b] - tr.xs[a]) / dt) * arc;
      const cy = ((tr.ys[b] - tr.ys[a]) / dt) * arc;
      const c = Math.cos(th);
      const s = Math.sin(th);
      tr.mx[i] = cx * c + cy * s;
      tr.my[i] = -cx * s + cy * c;
    } else {
      const d0 = (tr.ts[i] - tr.ts[i - 1]) / 1000;
      const d1 = (tr.ts[i + 1] - tr.ts[i]) / 1000;
      const c0x = (tr.xs[i] - tr.xs[i - 1]) / d0;
      const c0y = (tr.ys[i] - tr.ys[i - 1]) / d0;
      const c1x = (tr.xs[i + 1] - tr.xs[i]) / d1;
      const c1y = (tr.ys[i + 1] - tr.ys[i]) / d1;
      tr.mx[i] = (d1 * c0x + d0 * c1x) / (d0 + d1);
      tr.my[i] = (d1 * c0y + d0 * c1y) / (d0 + d1);
    }
  }
}

/**
 * Average turn rate (deg/s) over this report's own samples, from chord
 * headings; NaN if they are too short to tell. Older samples are left out:
 * where an old look-ahead meets a new report there is often a kink that is
 * not a turn.
 */
function bufferTurnRate(tr: Track, reportMs: number): number {
  const n = tr.n;
  const from = reportMs - MERGE_EPSILON_MS;
  let firstHeading = NaN;
  let firstMid = 0;
  let lastHeading = NaN;
  let lastMid = 0;
  let turned = 0;
  for (let i = 0; i + 1 < n; i++) {
    if (tr.ts[i] < from) continue;
    const dx = tr.xs[i + 1] - tr.xs[i];
    const dy = tr.ys[i + 1] - tr.ys[i];
    if (Math.hypot(dx, dy) < MIN_TURN_CHORD_M) {
      // a gap in usable chords: only count a contiguous run ending at the newest
      firstHeading = NaN;
      turned = 0;
      continue;
    }
    const heading = Math.atan2(dx, dy) / DEG;
    const mid = (tr.ts[i] + tr.ts[i + 1]) / 2;
    if (Number.isNaN(firstHeading)) {
      firstHeading = heading;
      firstMid = mid;
      turned = 0;
    } else turned += wrap180(heading - lastHeading);
    lastHeading = heading;
    lastMid = mid;
  }
  if (Number.isNaN(firstHeading) || lastMid - firstMid < 1000) return NaN;
  return turned / ((lastMid - firstMid) / 1000);
}

/** Acceleration (kt/s) from the chord speeds of this report's own samples, NaN if they span too little. */
function bufferAcceleration(tr: Track, reportMs: number): number {
  let firstSpeed = NaN;
  let firstMid = 0;
  let lastSpeed = NaN;
  let lastMid = 0;
  for (let i = 0; i + 1 < tr.n; i++) {
    if (tr.ts[i] < reportMs) continue;
    const dt = (tr.ts[i + 1] - tr.ts[i]) / 1000;
    const d = Math.hypot(tr.xs[i + 1] - tr.xs[i], tr.ys[i + 1] - tr.ys[i]);
    if (d < MIN_SPEED_CHORD_M) continue;
    const v = d / dt / KNOTS_TO_MS;
    const mid = (tr.ts[i] + tr.ts[i + 1]) / 2;
    if (Number.isNaN(firstSpeed)) {
      firstSpeed = v;
      firstMid = mid;
    }
    lastSpeed = v;
    lastMid = mid;
  }
  const span = (lastMid - firstMid) / 1000;
  return span >= MIN_ACCEL_DT_S ? (lastSpeed - firstSpeed) / span : NaN;
}

/** Flight phase for the latest report, with hysteresis from the previous one. */
function decidePhase(tr: Track, prev: Phase | null): Phase {
  const speed = tr.speedKt;
  if (tr.onGround) {
    if (speed < PARKED_MAX_KT) return "parked";
    if (speed < TAXI_MAX_KT || tr.cls === "heli") return "taxi";
    if (tr.accel > ROLL_ACCEL_KTS) return "takeoffRoll";
    if (tr.accel < -ROLL_ACCEL_KTS) return "landingRoll";
    if (prev && FROM_TAXI.has(prev)) return "takeoffRoll";
    if (prev && FROM_LANDING.has(prev)) return "landingRoll";
    // first sighting at speed on a runway: at the destination it is landing
    return tr.field && tr.field === tr.destAp ? "landingRoll" : "takeoffRoll";
  }
  const vr = tr.vrate;
  if (tr.field) {
    const agl = tr.alt - tr.fieldElev;
    const near = tr.fieldKm < APPROACH_RANGE_KM;
    if (agl < APPROACH_MAX_AGL_FT) {
      const wasApproach = prev === "approach" || prev === "flare";
      if (near && (vr < APPROACH_MAX_VRATE_FPM || (wasApproach && vr < -APPROACH_MAX_VRATE_FPM)))
        return agl < FLARE_MAX_AGL_FT ? "flare" : "approach";
      if (
        (near && vr > INITIAL_CLIMB_MIN_VRATE_FPM) ||
        prev === "takeoffRoll" ||
        (prev === "initialClimb" && vr > APPROACH_MAX_VRATE_FPM)
      )
        return "initialClimb";
    }
  }
  if (vr > CLIMB_VRATE_FPM || ((prev === "climb" || prev === "initialClimb") && vr > CLIMB_HOLD_VRATE_FPM)) return "climb";
  if (vr < -CLIMB_VRATE_FPM || ((prev === "descent" || prev === "approach") && vr < -CLIMB_HOLD_VRATE_FPM))
    return "descent";
  return "cruise";
}

export const motion = new Motion();
