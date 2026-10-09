// Aircraft motion between feed reports: where to draw each aircraft, and how.
//
// Flightradar24 sends a report every 8-15 s (minutes, for some sources) with a
// buffer of the aircraft's positions over the ~10 s before it: its recent
// history, so each report brings the path since the previous one. Between
// reports we interpolate inside the known samples, extrapolate along an arc
// past them, predict touchdowns and lift-offs, derive pitch, bank and gear,
// and blend every correction in over a few seconds so that nothing on the
// globe ever jumps or freezes.
//
// BY DESIGN THE MAP SHOWS EACH AIRCRAFT A FEW SECONDS IN THE PAST (2-12 s; see
// `drawnTime`). Drawn at "now", an aircraft is usually past its newest data
// and extrapolating, then corrected when the next report lands: that is what
// made aircraft jerk about. Drawn slightly in the past it is almost always
// between known positions, so it can follow a smoothed path through them:
//
// - Positions (and altitudes) are fitted on arrival with a cubic smoothing
//   spline (`smoothSpline`), which approximates the samples instead of passing
//   through each noisy one; its stiffness averages over about the latest three
//   samples. Track, heading, speed, turn rate (bank) and vertical rate come
//   from the fitted curve, which is C2 and evaluated per frame as a Hermite
//   piece, as cheaply as before.
// - The delay is chosen per aircraft from its data cadence and how old its
//   newest sample (the latest report) is on arrival (`noteArrival`), so that
//   the drawn time stays inside the data, and eases between values at no
//   more than ~10% of real time (`retarget`), so playback never jumps.
// - Each report's state (phase, gear, field, measured attitude) takes over
//   when the drawn time reaches the report's time, not on arrival, so what is
//   drawn is consistent; past the newest data the extrapolation carries on as
//   before, from the end of the smoothed curve.
// - Any change to the curve at the drawn time (a new sample, a state taking
//   over) is blended out matching position AND velocity (C1).
//
// Only the drawing is delayed: the panel's numbers come from the feed itself.
//
// Heights near airports are true heights above the field: the reported
// pressure altitude corrected with the field's QNH, learned from the traffic
// (see "Pressure altitude" below), and on the runway is on the ground even
// while Flightradar24's air/ground flag lags, so touchdowns and lift-offs
// happen where and when they did, through a flare.
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
  /** Barometric (pressure) altitude, feet: referenced to 1013.25 hPa, like Flightradar24's. */
  altBaro?: number;
  /** The altimeter setting on board, hPa: below the transition altitude, the local QNH. */
  navQnh?: number;
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
  /**
   * Height to draw at, feet: altitude with the relevant airport's field elevation removed near
   * airports (faded back to plain reported altitude by ~30-60 km out), exactly 0 on the ground.
   * Near a field the reported pressure altitude is corrected with its QNH, and the last feet
   * above the runway are shaped into a flare.
   */
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

/**
 * How much horizontal history a track keeps (and fits): the drawn time is
 * never more than 12 s behind the newest sample (which is never in the
 * future), so older samples only steady the fit's start.
 */
const SAMPLE_WINDOW_MS = 30_000;
/**
 * A report (or a point of its buffer) this far (m) off the smoothed path of
 * the samples held is a jump in the data: the old samples are dropped rather
 * than smoothed into it.
 */
const JUMP_M = 150;
/** ...checked for points inside those samples or up to this long (ms) past them. */
const JUMP_LOOKAHEAD_MS = 4_000;
/** Samples closer together than this are duplicates (and would make the spline's tangents explode). */
const MIN_SAMPLE_GAP_MS = 200;
/** How far before the first sample we run the track backwards (clock skew), seconds. */
const BACKWARD_LIMIT_S = 30;
/** FR24 buffer offsets are in 1e-5 degrees, back from the report: a point is at `lat/lon - d * this`. */
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
/** After k·t this large the correction is gone (< 0.01%): 6 s at the full rate. */
const BLEND_END_KT = 13.2;
/**
 * Small corrections (the smoothed curve moving by metres as a new sample
 * comes in) are blended out more slowly, so that their acceleration stays
 * under this (m/s², a gentle turn's worth)...
 */
const GENTLE_ACCEL = 3;
/** ...but no slower than this rate, 1/s (~1% after 13 s)... */
const MIN_BLEND_RATE = 0.5;
/** ...and only below this size (m), back to the full rate by twice it (a real jump in the data is resolved quickly). */
const GENTLE_MAX_M = 40;
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

// --- Delayed, smoothed playback --------------------------------------------------------

/** The drawn time trails the server clock by this much at least and at most. */
const MIN_DELAY_MS = 2_000;
const MAX_DELAY_MS = 12_000;
/**
 * The delay changes at no more than about this rate (seconds per second):
 * while it adjusts, playback runs at 90-110% of real time, which nobody sees.
 */
const DELAY_RATE = 0.1;
/**
 * ...and its rate changes by at most this much per second, so the playback
 * speeds up or slows down by under 1% a second (2.5 m/s² at 250 m/s).
 */
const DELAY_ACCEL = 0.01;
/** Delay targets this close to the current one are ignored, so the delay isn't forever on the move. */
const DELAY_DEADBAND_MS = 500;
/** Weight of a new cadence or lead measurement in the delay target. */
const DELAY_SMOOTHING = 0.3;
/** Data cadence assumed until two reports are seen (Flightradar24's refresh). */
const DEFAULT_CADENCE_MS = 8_000;
const MIN_CADENCE_MS = 1_000;
const MAX_CADENCE_MS = 60_000;
/** The drawn time stays this far (one sample spacing, within these bounds) behind the newest sample. */
const MIN_MARGIN_MS = 1_000;
const MAX_MARGIN_MS = 3_000;
/**
 * Smoothing-spline stiffness for positions, s³ (per unit sample weight). The
 * fitted curve averages over about (λ × spacing)^¼ seconds either side: 2.5 s
 * for Flightradar24's buffer (2 s apart), 2.8 s for adsb.lol's 3 s polls. About
 * three samples, that is, which takes out the jitter with little lag.
 */
const SMOOTH_LAMBDA = 20;
/** The same for altitude (25 ft steps, a report every 3-15 s): about ±5-7 s. */
const ALT_SMOOTH_LAMBDA = 200;
/** Sample weights by source (inverse variance relative to ~10 m). */
const WEIGHT_FR24 = 1;
const WEIGHT_ADSB = 1;
const WEIGHT_MLAT = 0.04;
/** Weight of a reported velocity as the slope at the newest sample, when that sample is the report itself. */
const REPORTED_SLOPE_WEIGHT = 10;
/** The smoothing's reference path turns (and changes speed) as the data does for this long before the newest sample, s... */
const REF_FULL_S = 12;
/** ...fading to straight and steady by this long before it. */
const REF_ZERO_S = 24;
/** Fitting passes: around the report's estimated turn and speed change, then twice around those read off the previous pass. */
const REF_PASSES = 3;
/** Those are read off two chords of this length (ms) ending at the newest sample... */
const REF_CHORD_MS = 6_000;
/** ...or shorter ones, down to this (s). */
const REF_MIN_CHORD_S = 2;
/** The drawn turn rate (for the bank) is the smoothed heading's change over ±this, ms. */
const TURN_WINDOW_MS = 2_000;
/** Past the newest sample the turn rate eases from the path's into the prediction's over this long, s. */
const TURN_HANDOVER_S = 3;
/** Past the newest altitude the vertical rate eases from the smoothed curve's into the estimate's with this time constant, s. */
const VRATE_HANDOVER_S = 3;
/** Altitude differences up to this (ft) are never a jump in the data, whatever the rate (quantisation). */
const ALT_STEP_FT = 300;
/** Altitude history kept, ms. */
const ALT_WINDOW_MS = 60_000;
/** Report states kept (each takes over when the drawn time reaches it). */
const MAX_STATES = 16;
/** Measured attitude history kept, ms. */
const MEAS_HISTORY_MS = 30_000;
/** A pose further than this from the previous one (in time) is a fresh look: a state taking over isn't blended. */
const CONTINUITY_GAP_MS = 1_000;

// --- Pressure altitude ---------------------------------------------------------------
//
// Reported altitudes (Flightradar24's `alt`, ADS-B's `alt_baro`) are pressure
// altitudes: what an altimeter set to the standard 1013.25 hPa reads. On a day
// whose QNH (sea-level pressure) is off standard they are off true altitude by
// ~27-30 ft per hPa, the same for every aircraft around: 1004 hPa at Amsterdam
// reads ~240 ft high, 1020 hPa in Atlanta ~200 ft low. Measured at touchdown
// (`testdata/landings.json`) a runway read 175-225 ft at Heathrow and Schiphol
// and 200-280 ft below the field in Atlanta, Chicago and Dallas, while on the
// ground Flightradar24 says 0: drawn as is, an aircraft hovered a few hundred
// feet over the runway and then dropped, or sank into it 30 s early.
//
// So each field's QNH is learned from the traffic itself: a runway's pressure
// altitude, read off an aircraft rolling on it (see `onRunway`), is the field
// elevation at that field's QNH. It is shared by every aircraft around (and,
// interpolated, nearby fields), and the selected aircraft's own altimeter
// setting (adsb.lol's `nav_qnh`) is better still. Altitudes near a field are
// corrected with it, faded out with the field elevation itself.

const STD_HPA = 1013.25;
/** ISA: pressure altitude h (ft) at pressure p is H0 (1 - (p / 1013.25)^EXP). */
const ISA_H0_FT = 145_442.16;
const ISA_EXP = 0.190263;
const MIN_QNH_HPA = 940;
const MAX_QNH_HPA = 1_060;
/** A field's learned QNH counts for this long, ms (it moves ~1 hPa an hour)... */
const QNH_TTL_MS = 3 * 3_600_000;
/** ...and stands in for fields without one this far around, km. */
const QNH_NEIGHBOUR_KM = 300;
/** Weight of a new runway reading in a field's QNH; a reading this far (hPa) off it counts less. */
const QNH_SMOOTHING = 0.3;
const QNH_OUTLIER_HPA = 5;
const QNH_OUTLIER_SMOOTHING = 0.05;
/** The on-board altimeter setting is the local QNH below this height above the field, ft. */
const MEAS_QNH_MAX_AGL_FT = 3_000;
/** Runways are read only this close to the field's reference point, km. */
const RUNWAY_FIELD_KM = 8;
/** A runway's pressure altitude is this close to the field (ft), its QNH known or not. */
const RUNWAY_TOL_KNOWN_FT = 150;
const RUNWAY_TOL_UNKNOWN_FT = 600;
/** One altitude step: reports this close are level. */
const LEVEL_FT = 25;
/** Braking on the runway (kt/s), not slowing down on the approach. */
const RUNWAY_DECEL_KTS = 1;
/** Below these ground speeds (kt) nothing flies, whatever the air/ground flag says. */
const MIN_FLYING_KT = { jet: 60, turboprop: 50, light: 30 } as const;
/** Older reports than this (s) don't tell a level run on the runway. */
const LEVEL_MAX_DT_S = 20;

/** True altitude (ft) of a pressure altitude at a QNH (hPa). */
function trueAltitude(pressureAltFt: number, qnh: number): number {
  return ISA_H0_FT - (ISA_H0_FT - pressureAltFt) * Math.pow(STD_HPA / qnh, ISA_EXP);
}

/** The QNH (hPa) at which a pressure altitude reads a true altitude (ft). */
function qnhFrom(pressureAltFt: number, trueAltFt: number): number {
  const ratio = (ISA_H0_FT - trueAltFt) / (ISA_H0_FT - pressureAltFt);
  return STD_HPA / Math.pow(ratio, 1 / ISA_EXP);
}

const validQnh = (q: number | undefined): q is number =>
  typeof q === "number" && Number.isFinite(q) && q >= MIN_QNH_HPA && q <= MAX_QNH_HPA;

// --- Touchdown and lift-off ------------------------------------------------------------
//
// The last feet above the runway are drawn through a soft floor (`flare`):
// the height above the field follows the data down to FLARE_HEIGHT_FT, then
// eases into the ground, meeting it at TOUCHDOWN_SINK of the descent rate
// (~100-200 fpm off a 3° approach) rather than diving in or levelling off.
// Lift-offs leave the ground the same way.

/** The flare begins this high above the field, ft. */
const FLARE_HEIGHT_FT = 50;
/** Touchdown (and lift-off) at this fraction of the approach's (climb's) vertical rate. */
const TOUCHDOWN_SINK = 0.2;
/** The flare's length in raw height (ft): touchdown is where the data reaches FLARE_HEIGHT_FT - this. */
const FLARE_SPAN_FT = (2 * FLARE_HEIGHT_FT) / (1 + TOUCHDOWN_SINK);
/** Raw height above the field (ft, negative) at which the flared height reaches the ground. */
const FLARE_FLOOR_FT = FLARE_HEIGHT_FT - FLARE_SPAN_FT;
const FLARE_A = (1 - TOUCHDOWN_SINK) / (2 * FLARE_SPAN_FT);
/** A correction of a whole altitude reference (a QNH learned) is blended out at this rate, 1/s, not snapped... */
const REFERENCE_BLEND_RATE = 0.8;
/**
 * ...or more slowly, so that it moves the drawn height by no more than this
 * (ft/s, ~1800 fpm) at its peak: a QNH learned on the runway can move an
 * aircraft still drawn on short final by a few hundred feet.
 */
const REFERENCE_MAX_FT_S = 30;
/** Height left above the flare's floor at a predicted touchdown is eased out over this long before it, ms. */
const TOUCHDOWN_FIT_MS = 2_000;

/** Height to draw (ft above the field) for a raw height `x`: x above the flare, easing to 0 at FLARE_FLOOR_FT. */
function flare(x: number): number {
  if (x >= FLARE_HEIGHT_FT) return x;
  const u = x - FLARE_FLOOR_FT;
  return u <= 0 ? 0 : FLARE_A * u * u + TOUCHDOWN_SINK * u;
}

/** Slope of `flare` at `x`. */
function flareSlope(x: number): number {
  if (x >= FLARE_HEIGHT_FT) return 1;
  const u = x - FLARE_FLOOR_FT;
  return u <= 0 ? 0 : 2 * FLARE_A * u + TOUCHDOWN_SINK;
}

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
/** Newer reports taken in per frame at most (~10 µs each); new aircraft always are. */
const INGEST_BUDGET = 600;
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

/** Critically damped decay (rate k) of a correction applied `t` seconds ago: 1 → 0 with zero slope at 0. */
function blend(t: number, k: number): number {
  if (t <= 0) return 1;
  const kt = k * t;
  if (kt >= BLEND_END_KT) return 0;
  return (1 + kt) * Math.exp(-kt);
}

/**
 * Blend rate for a small correction of `x0` m with a rate difference of `v0`
 * m/s: the slowest that keeps its peak acceleration (≈ 2 k v0 + k² x0) under
 * GENTLE_ACCEL, within MIN_BLEND_RATE..BLEND_RATE.
 */
function gentleRate(x0: number, v0: number): number {
  const k = x0 > 1e-6 ? (-v0 + Math.sqrt(v0 * v0 + x0 * GENTLE_ACCEL)) / x0 : v0 > 1e-6 ? GENTLE_ACCEL / (2 * v0) : BLEND_RATE;
  return clamp(k, MIN_BLEND_RATE, BLEND_RATE);
}

/**
 * A correction being blended out, critically damped from an offset and its
 * rate: x(t) = (x0 + (v0 + k x0) t) e^{-kt}, so the drawn position and its
 * velocity are both continuous (C1). Restarting one from its current offset
 * and rate at the same k continues the same curve, so corrections simply add.
 */
class Blend {
  ms = -Infinity;
  k = BLEND_RATE;
  x = 0;
  y = 0;
  vx = 0;
  vy = 0;
  // results of `at`
  px = 0;
  py = 0;
  pvx = 0;
  pvy = 0;
  /** `scale` converts the units to metres for the gentle rate. */
  constructor(readonly scale = 1) {}

  /** The offset and its rate at `nowMs`, into px/py/pvx/pvy (all 0 once it is over). */
  at(nowMs: number): void {
    const t = Math.max(0, (nowMs - this.ms) / 1000);
    const k = this.k;
    if (!(k * t < BLEND_END_KT)) {
      this.px = this.py = this.pvx = this.pvy = 0;
      return;
    }
    const e = Math.exp(-k * t);
    const cx = this.vx + k * this.x;
    const cy = this.vy + k * this.y;
    this.px = (this.x + cx * t) * e;
    this.py = (this.y + cy * t) * e;
    this.pvx = (this.vx - k * cx * t) * e;
    this.pvy = (this.vy - k * cy * t) * e;
  }

  /** Adds a correction and its rate at `nowMs` to what is left of this one; on at rate `k`, or the gentle rate for the sum. */
  add(nowMs: number, dx: number, dy: number, dvx: number, dvy: number, k: number | null): void {
    this.at(nowMs);
    this.x = this.px + dx;
    this.y = this.py + dy;
    this.vx = this.pvx + dvx;
    this.vy = this.pvy + dvy;
    this.ms = nowMs;
    this.k = k ?? gentleRate(Math.hypot(this.x, this.y) * this.scale, Math.hypot(this.vx, this.vy) * this.scale);
  }

  clear(): void {
    this.ms = -Infinity;
  }
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

/**
 * What a report said and predicted, kept so that it takes over only when the
 * drawn time reaches it: the drawn aircraft is a few seconds in the past, and
 * its phase, gear and height have to be those of that moment.
 */
interface State {
  reportMs: number;
  phase: Phase;
  trackDeg: number;
  /** Reported altitude (ft) and vertical rate (fpm). */
  alt: number;
  vrate: number;
  field: Airport | null;
  fieldElev: number;
  liftoffMs: number;
  touchdownMs: number;
  touchdownPitch: number;
  capAlt: number;
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
  /** Ground-speed change since the previous report (kt/s), NaN if too close or too far apart to tell. */
  reportAccel = NaN;
  turnRate = 0; // deg/s, clockwise positive
  vrate = 0; // fpm
  vrRefMs = NaN;
  /** The vertical rate's reference: a pressure altitude and its correction weight (re-corrected as the QNH changes). */
  vrRefRaw = 0;
  vrRefW = 0;
  vrRefGround = false;
  phase: Phase = "cruise";

  // pressure altitude
  /** QNH (hPa) the altitudes are corrected with, NaN when unknown (no correction). */
  qnh = NaN;
  /** QNH read off this aircraft's own runway (at `qnhOwnField`), NaN if none. */
  qnhOwn = NaN;
  qnhOwnField: Airport | null = null;
  /** The latest reported pressure altitude (ft; 0 is no altitude), its time and its correction weight. */
  rawAlt = 0;
  rawAltMs = -Infinity;
  rawW = 0;
  /** When the aircraft was last on the ground: older altitude samples belong to an earlier flight segment. */
  groundMs = -Infinity;
  /** Altitude corrections are blended out at this rate on the next change (a QNH learned), else NaN. */
  altBlendRate = NaN;

  // horizontal samples, oldest first; x/y (raw) and fx/fy/mx/my (smoothed
  // values and slopes, m and m/s) are in the local frame around the newest
  n = 0;
  ts: number[] = [];
  lats: number[] = [];
  lons: number[] = [];
  xs: number[] = [];
  ys: number[] = [];
  fx: number[] = [];
  fy: number[] = [];
  mx: number[] = [];
  my: number[] = [];
  /** Turn rate of the smoothed path at each sample, rad/s, clockwise positive. */
  om: number[] = [];

  // altitude samples (airborne only), oldest first: reported pressure altitudes (ft) and their
  // correction weights; the corrected values (ft), and the smoothed values (ft) and slopes (ft/s)
  ats: number[] = [];
  aps: number[] = [];
  aws: number[] = [];
  avs: number[] = [];
  afv: number[] = [];
  afd: number[] = [];

  /** Each report's state, oldest first: the one in effect at the drawn time is used. */
  states: State[] = [];
  /** The state last drawn (its reportMs). */
  activeMs = NaN;
  /** When pose() was last called. */
  lastPoseMs = -Infinity;

  // measured attitude history: roll (and its bias to the model's bank) and crab, by time
  rollTs: number[] = [];
  rolls: number[] = [];
  rollBiases: number[] = [];
  crabTs: number[] = [];
  crabs: number[] = [];

  // drawing delay: eases from `dFrom` (slope `dSlope`) at `dMs` to `dTo` over `dDur`, ms
  dFrom = MIN_DELAY_MS;
  dTo = MIN_DELAY_MS;
  dSlope = 0;
  dMs = -Infinity;
  dDur = 0;
  /** Smoothed data cadence (ms between reports) and delay target, ms. */
  cadenceMs = DEFAULT_CADENCE_MS;
  delayTarget = NaN;
  /** Newest sample time when data last arrived. */
  newestMs = -Infinity;
  /** Data time of the last arrival that extended the samples. */
  arrivalDataMs = -Infinity;
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

  // the angles' correction being blended out
  offMs = -Infinity;
  /**
   * Position (east/north, m) and height (ft) corrections: a large one (a jump
   * in the data) at the full rate, small ones (the smoothed curve moving as
   * samples come in) gently, so their acceleration stays small.
   */
  fast = new Blend();
  gentle = new Blend();
  altFast = new Blend(FEET_TO_M);
  altGentle = new Blend(FEET_TO_M);
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
  /** The altitude (ft) and vertical rate (fpm) the data gives, before the flare shapes them near the ground. */
  altRaw: number;
  vsRaw: number;
  agl: number;
  ground: boolean;
  /** Velocity east/north, m/s of drawn time (of real time, once `drawn` scaled it). */
  vE: number;
  vN: number;
  /** Bank before the measured roll's bias. */
  modelBank: number;
  /** The state's field is known (gear logic). */
  hasField: boolean;
}

const newRaw = (): Raw => ({
  vE: 0,
  vN: 0,
  modelBank: 0,
  hasField: false,
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
  altRaw: 0,
  vsRaw: 0,
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
  /** Reports still to be taken in this frame (`budgetMs`). */
  private budget = 0;
  private budgetMs = NaN;
  private byIata = new Map<string, Airport>();
  private grid = new Map<number, Airport[]>();
  /** Each field's QNH as learned from the traffic (hPa) and when. */
  private qnhs = new Map<Airport, { hpa: number; ms: number }>();
  /** Fields' QNHs interpolated from those around, until anything is learned (`qnhVersion`) or a minute passes. */
  private qnhAround = new Map<Airport, { hpa: number; ms: number; version: number }>();
  private qnhVersion = 0;

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
    if (tr.n === 0) this.ingest(tr, f, nowMs);
    else if (f.timestampMs > tr.fr24Ms) {
      // a snapshot brings every aircraft's report at once: take in at most so many a frame
      // (one frame being one `nowMs`); the rest are drawn as they were for a frame or two,
      // which changes nothing on screen, since the drawn time is seconds behind anyway
      if (nowMs !== this.budgetMs) {
        this.budgetMs = nowMs;
        this.budget = INGEST_BUDGET;
      }
      if (this.budget > 0) {
        this.budget--;
        this.ingest(tr, f, nowMs);
      }
    }
    if (nowMs - this.lastPrune > PRUNE_INTERVAL_MS) this.prune(nowMs);

    // drawn a few seconds in the past, by design (see the top of this file)
    const T = drawTime(tr, nowMs);
    const state = stateAt(tr, T);
    if (state.reportMs !== tr.activeMs) this.takeOver(tr, nowMs, T, state);
    tr.lastPoseMs = nowMs;
    const raw = this.drawn(tr, nowMs, T, state, scratchA, true);
    return {
      lon: raw.lon,
      lat: raw.lat,
      altFt: raw.drawAlt,
      heading: raw.heading,
      track: raw.track,
      pitch: clamp(raw.pitch, MIN_PITCH, MAX_PITCH),
      bank: clamp(raw.bank, -MAX_BANK, MAX_BANK),
      gear: this.gear(tr, raw, nowMs),
      phase: raw.phase,
    };
  }

  /**
   * The (server) time an aircraft is drawn at: `nowMs` less its delay, which
   * is 2-12 s and changes smoothly. Trails and anything else that shows where
   * the aircraft has been should stop at this time, or they run ahead of it.
   * `nowMs` itself for an aircraft not seen yet.
   */
  drawnTime(id: number, nowMs: number = this.now()): number {
    const tr = this.tracks.get(id);
    return tr && tr.n > 0 ? drawTime(tr, nowMs) : nowMs;
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
        tr.tasMs = 0;
        tr.navTrack = NaN;
        tr.rollTs.length = tr.rolls.length = tr.rollBiases.length = 0;
        tr.crabTs.length = tr.crabs.length = 0;
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
    this.blendAround(tr, nowMs, () => this.applyMeasured(tr, m, newPosition, nowMs));
  }

  /**
   * Same airport-relative drawing height for points of the aircraft's trail
   * (so the trail meets the model): `altFt` is a reported (pressure) altitude,
   * corrected with the aircraft's QNH near its field like its own.
   */
  drawAltitude(id: number, altFt: number, lat: number, lon: number): number {
    if (!(altFt > 0)) return 0;
    const tr = this.tracks.get(id);
    if (!tr) return altFt;
    const w = tr.field ? fieldFade(distanceKm(lat, lon, tr.field.lat, tr.field.lon)) : 0;
    return Math.max(0, corrected(tr, altFt, w) - fieldRemoval(tr, lat, lon));
  }

  /**
   * The QNH (hPa) learned for a field from the traffic at `nowMs`: its own,
   * else interpolated from fields around; NaN if none is known.
   */
  fieldQnh(ap: Airport, nowMs: number = this.now()): number {
    const own = this.qnhs.get(ap);
    if (own && nowMs - own.ms <= QNH_TTL_MS) return own.hpa;
    if (this.qnhs.size === 0) return NaN;
    const cached = this.qnhAround.get(ap);
    if (cached && cached.version === this.qnhVersion && Math.abs(nowMs - cached.ms) < 60_000) return cached.hpa;
    let sum = 0;
    let weights = 0;
    for (const [other, q] of this.qnhs) {
      if (nowMs - q.ms > QNH_TTL_MS) continue;
      const km = distanceKm(ap.lat, ap.lon, other.lat, other.lon);
      if (km > QNH_NEIGHBOUR_KM) continue;
      const w = 1 / ((km + 20) * (km + 20));
      sum += w * q.hpa;
      weights += w;
    }
    const hpa = weights > 0 ? sum / weights : NaN;
    this.qnhAround.set(ap, { hpa, ms: nowMs, version: this.qnhVersion });
    return hpa;
  }

  /** A reading of a field's QNH (hPa) at `ms`. */
  private learnQnh(ap: Airport, hpa: number, ms: number): void {
    if (!validQnh(hpa)) return;
    this.qnhVersion++;
    const q = this.qnhs.get(ap);
    if (!q || ms - q.ms > QNH_TTL_MS) {
      this.qnhs.set(ap, { hpa, ms });
      return;
    }
    const k = Math.abs(hpa - q.hpa) > QNH_OUTLIER_HPA ? QNH_OUTLIER_SMOOTHING : QNH_SMOOTHING;
    q.hpa += k * (hpa - q.hpa);
    q.ms = Math.max(q.ms, ms);
  }

  /** The QNH to correct a track's altitudes with at `t`: its own altimeter's, its own runway's, else its field's. */
  private trackQnh(tr: Track, t: number): number {
    if (!tr.field) return NaN;
    const m = tr.meas;
    if (m && validQnh(m.navQnh) && measFresh(tr, t) && tr.alt - tr.fieldElev < MEAS_QNH_MAX_AGL_FT) return m.navQnh;
    if (tr.qnhOwnField === tr.field && validQnh(tr.qnhOwn)) return tr.qnhOwn;
    return this.fieldQnh(tr.field, t);
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
    if (first) this.update(tr, f, t, true, nowMs);
    else this.blendAround(tr, nowMs, () => this.update(tr, f, t, false, nowMs));
  }

  /**
   * The model at drawn time `T` as it moves in real time at `nowMs`: its
   * velocities scaled by the playback rate (the delay may be easing), and with
   * the correction being blended out if `withOffsets`.
   */
  private drawn(tr: Track, nowMs: number, T: number, state: State, out: Raw, withOffsets: boolean): Raw {
    this.evaluate(tr, T, out, state);
    const rate = 1 - delaySlope(tr, nowMs);
    out.vE *= rate;
    out.vN *= rate;
    out.vsFpm *= rate;
    if (!withOffsets) return out;
    // position and height: the corrections being blended out (C1)
    const { fast, gentle, altFast, altGentle } = tr;
    fast.at(nowMs);
    gentle.at(nowMs);
    const dE = fast.px + gentle.px;
    const dN = fast.py + gentle.py;
    if (dE !== 0 || dN !== 0) {
      out.lat += dN / M_PER_DEG;
      out.lon = wrapLon(out.lon + dE / (M_PER_DEG * Math.max(Math.cos(out.lat * DEG), 1e-3)));
      out.vE += fast.pvx + gentle.pvx;
      out.vN += fast.pvy + gentle.pvy;
    }
    altFast.at(nowMs);
    altGentle.at(nowMs);
    const dA = altFast.px + altGentle.px;
    if (dA !== 0) {
      out.drawAlt = Math.max(0, out.drawAlt + dA);
      out.vsFpm += (altFast.pvx + altGentle.pvx) * 60;
    }
    const age = (nowMs - tr.offMs) / 1000;
    // the angles: from the offset with zero rate, at the full rate as before
    const w = blend(age, BLEND_RATE);
    if (w === 0) return out;
    out.heading = wrap360(out.heading + tr.offHdg * w);
    out.track = wrap360(out.track + tr.offTrk * w);
    out.pitch += tr.offPitch * w;
    out.bank += tr.offBank * w;
    return out;
  }

  /** Starts blending out the difference between what was drawn (`before`) and the model now (`after`). */
  private setOffsets(tr: Track, nowMs: number, before: Raw, after: Raw, snapBeforeMs: number): void {
    const { fast, gentle, altFast, altGentle } = tr;
    const cosLat = Math.max(Math.cos(after.lat * DEG), 1e-3);
    const offE = wrap180(before.lon - after.lon) * M_PER_DEG * cosLat;
    const offN = (before.lat - after.lat) * M_PER_DEG;
    if (Math.hypot(offE, offN) > SNAP_DISTANCE_M || nowMs - snapBeforeMs > SNAP_AGE_MS) {
      fast.clear();
      gentle.clear();
      altFast.clear();
      altGentle.clear();
      tr.offMs = -Infinity;
      tr.altBlendRate = NaN;
      return;
    }
    // what changed now, beyond the corrections already under way (which carry on as they were),
    // goes to the fast blend if large and the gentle one if small
    fast.at(nowMs);
    gentle.at(nowMs);
    const dE = offE - fast.px - gentle.px;
    const dN = offN - fast.py - gentle.py;
    const dVE = before.vE - after.vE - fast.pvx - gentle.pvx;
    const dVN = before.vN - after.vN - fast.pvy - gentle.pvy;
    const w = smoothstep((Math.hypot(dE, dN) - GENTLE_MAX_M) / GENTLE_MAX_M);
    fast.add(nowMs, w * dE, w * dN, w * dVE, w * dVN, BLEND_RATE);
    gentle.add(nowMs, (1 - w) * dE, (1 - w) * dN, (1 - w) * dVE, (1 - w) * dVN, null);
    altFast.at(nowMs);
    altGentle.at(nowMs);
    const dA = before.drawAlt - after.drawAlt - altFast.px - altGentle.px;
    const dVA = (before.vsFpm - after.vsFpm) / 60 - altFast.pvx - altGentle.pvx;
    const wa = smoothstep((Math.abs(dA) * FEET_TO_M - GENTLE_MAX_M) / GENTLE_MAX_M);
    // a new altitude reference (a QNH learned) moves the whole vertical profile: eased in, not snapped
    // (and one still under way carries on at its own pace, sped up no more than that allows;
    // a critically damped blend of x from rest moves at most k |x| / e)
    const capped = (REFERENCE_MAX_FT_S * Math.E) / Math.max(1, Math.abs(altFast.px + wa * dA));
    const altRate = Number.isFinite(tr.altBlendRate)
      ? Math.min(tr.altBlendRate, capped)
      : altFast.px !== 0 && altFast.k < BLEND_RATE
        ? Math.min(BLEND_RATE, Math.max(altFast.k, capped))
        : BLEND_RATE;
    tr.altBlendRate = NaN;
    altFast.add(nowMs, wa * dA, 0, wa * dVA, 0, altRate);
    altGentle.add(nowMs, (1 - wa) * dA, 0, (1 - wa) * dVA, 0, null);
    // the angles, from the offset with zero rate as before
    tr.offMs = nowMs;
    tr.offHdg = wrap180(before.heading - after.heading);
    tr.offTrk = wrap180(before.track - after.track);
    tr.offPitch = before.pitch - after.pitch;
    tr.offBank = before.bank - after.bank;
  }

  /** The state drawn last frame, if the previous pose was a moment ago; else the one in effect at `T`. */
  private shownState(tr: Track, nowMs: number, T: number): State {
    if (Math.abs(nowMs - tr.lastPoseMs) <= CONTINUITY_GAP_MS) {
      for (const s of tr.states) if (s.reportMs === tr.activeMs) return s;
    }
    return stateAt(tr, T);
  }

  /** The drawn time has reached a newer (or, going back, older) report's state: blend over to it. */
  private takeOver(tr: Track, nowMs: number, T: number, state: State): void {
    if (Math.abs(nowMs - tr.lastPoseMs) <= CONTINUITY_GAP_MS) {
      const prev = tr.states.find((s) => s.reportMs === tr.activeMs);
      if (prev) {
        const before = this.drawn(tr, nowMs, T, prev, scratchA, true);
        const after = this.drawn(tr, nowMs, T, state, scratchB, false);
        this.setOffsets(tr, nowMs, before, after, nowMs);
      }
    }
    tr.activeMs = state.reportMs;
  }

  /** Applies `change` to an existing track, blending from the pose drawn before it to the one after. */
  private blendAround(tr: Track, nowMs: number, change: () => void): void {
    const previousMs = tr.reportMs;
    // what is on screen right now, correction included
    const T = drawTime(tr, nowMs);
    const before = this.drawn(tr, nowMs, T, this.shownState(tr, nowMs, T), scratchA, true);

    change();

    // the delay eases from where it was, so the drawn time is still T
    const state = stateAt(tr, T);
    const after = this.drawn(tr, nowMs, T, state, scratchB, false);
    tr.activeMs = state.reportMs;
    this.setOffsets(tr, nowMs, before, after, previousMs);
  }

  /** Updates samples, estimates, phase and predictions from a report. */
  private update(tr: Track, f: LiveFlight, t: number, first: boolean, nowMs: number): void {
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
      const lat0 = Number.isFinite(f.lat) ? clamp(f.lat, -90, 90) : 0;
      const lon0 = Number.isFinite(f.lon) ? wrapLon(f.lon) : 0;
      if (mergeReport(tr, f, t, lat0, lon0, true) !== null) {
        this.refresh(tr);
        noteArrival(tr, nowMs);
      }
      return;
    }
    // Flightradar24 reports no altitude as 0, and a negative pressure altitude (a field near sea
    // level on a high-pressure day) as 0 too: an airborne 0 is no altitude, carried on from the
    // model instead (before this report changes it)
    const rawAlt = Number.isFinite(f.alt) ? f.alt : 0;
    const altKnown = rawAlt > 0;
    let estAlt = NaN;
    let estVrate = NaN;
    let estGround = false;
    if (!f.onGround && !altKnown && !first) {
      const est = this.evaluate(tr, t, scratchC);
      estGround = est.ground;
      estAlt = est.altRaw;
      estVrate = est.vsRaw;
    }
    const prevOnGround = tr.onGround;
    const prevRaw = tr.rawAlt;
    const levelDt = (t - tr.rawAltMs) / 1000;

    tr.reportMs = t;
    tr.speedKt = Number.isFinite(f.speed) ? Math.max(0, f.speed) : 0;
    tr.trackDeg = Number.isFinite(f.track) ? wrap360(f.track) : 0;
    tr.lat0 = Number.isFinite(f.lat) ? clamp(f.lat, -90, 90) : 0;
    tr.lon0 = Number.isFinite(f.lon) ? wrapLon(f.lon) : 0;

    this.selectField(tr, f);
    const reportedOnly = mergeReport(tr, f, t, tr.lat0, tr.lon0, false)!;
    // this report's own samples: its buffer (the ~10 s before it) up to the report itself
    const ownFrom = reportFromMs;

    // --- acceleration (kt/s): across reports and along the buffer
    const reportAccel =
      dtReport >= MIN_ACCEL_DT_S && dtReport <= MAX_ACCEL_DT_S ? (tr.speedKt - prevSpeed) / dtReport : NaN;
    const bufferAccel = bufferAcceleration(tr, ownFrom);
    let accel =
      Number.isFinite(reportAccel) && Number.isFinite(bufferAccel)
        ? (reportAccel + bufferAccel) / 2
        : Number.isFinite(reportAccel)
          ? reportAccel
          : bufferAccel;
    if (!Number.isFinite(accel)) accel = tr.accel * SMOOTHING; // no news: let it relax
    tr.accel = clamp(first ? accel : tr.accel + SMOOTHING * (accel - tr.accel), -MAX_ACCEL_KTS, MAX_ACCEL_KTS);
    tr.reportAccel = dtReport >= 2 && dtReport <= MAX_ACCEL_DT_S ? (tr.speedKt - prevSpeed) / dtReport : NaN;

    // --- on the ground or in the air: Flightradar24's flag lags, so an aircraft rolling on the
    // runway, which reads the runway's pressure altitude, is on the ground (and tells its QNH)
    const oldQnh = tr.qnh;
    if (tr.field !== tr.qnhOwnField) tr.qnhOwn = NaN;
    tr.qnh = this.trackQnh(tr, t);
    tr.rawW = tr.field ? fieldFade(tr.fieldKm) : 0;
    let onGround = !!f.onGround;
    if (!onGround && !first)
      onGround = this.onRunway(tr, rawAlt, prevOnGround, prevPhase, prevRaw, levelDt, estGround);
    else if (onGround && !prevOnGround && !first) this.touchedDown(tr, prevRaw, levelDt, prevPhase);
    tr.onGround = onGround;
    this.setQnh(tr, t, oldQnh);
    tr.rawAlt = onGround || altKnown ? rawAlt : 0;
    tr.rawAltMs = t;
    let altEstimated = false;
    if (onGround) {
      tr.alt = 0;
      tr.groundMs = t;
    } else if (altKnown) {
      tr.alt = corrected(tr, rawAlt, tr.rawW);
      addAltitude(tr, t, rawAlt, tr.rawW);
    } else {
      // no altitude: carried on from the model (or the field, for a first sighting)
      altEstimated = true;
      tr.alt = Number.isFinite(estAlt) ? estAlt : tr.field ? tr.fieldElev : 0;
    }

    // --- turn rate (deg/s): buffer chord headings when fast enough, else reported tracks
    const reportTurn =
      dtReport >= MIN_TURN_DT_S && dtReport <= MAX_TURN_DT_S ? wrap180(tr.trackDeg - prevTrack) / dtReport : NaN;
    const bufferTurn = tr.onGround ? NaN : bufferTurnRate(tr, ownFrom);
    let turn = Number.isFinite(bufferTurn) ? bufferTurn : Number.isFinite(reportTurn) ? reportTurn : 0;
    if (tr.onGround && tr.speedKt < PARKED_MAX_KT) turn = 0;
    tr.turnRate = clamp(first ? turn : tr.turnRate + SMOOTHING * (turn - tr.turnRate), -MAX_TURN_DEG_S, MAX_TURN_DEG_S);

    // --- vertical rate (fpm): reported when signed in, else altitude differences between reports
    if (typeof f.vspeed === "number" && Number.isFinite(f.vspeed) && !(altEstimated && f.vspeed === 0)) {
      tr.vrate = clamp(onGround ? 0 : f.vspeed, -MAX_VRATE_FPM, MAX_VRATE_FPM);
      this.vrReference(tr, t);
    } else if (tr.onGround) {
      tr.vrate = 0;
      this.vrReference(tr, t);
    } else if (altEstimated) {
      // no altitude, no news: the model's own rate carries on (the reference stays the last real altitude)
      if (Number.isFinite(estVrate)) tr.vrate = clamp(estVrate, -MAX_VRATE_FPM, MAX_VRATE_FPM);
    } else {
      const dt = (t - tr.vrRefMs) / 1000;
      if (!(dt >= MIN_VRATE_DT_S)) {
        if (!Number.isFinite(tr.vrRefMs)) this.vrReference(tr, t);
      } else {
        // FR24 reports 0 ft on the ground: a lift-off climbs from the field (both re-corrected
        // with the QNH in use now, so a QNH learned in between is no vertical rate)
        const refAlt = tr.vrRefGround ? (tr.field ? tr.fieldElev : NaN) : corrected(tr, tr.vrRefRaw, tr.vrRefW);
        if (dt <= MAX_VRATE_DT_S && Number.isFinite(refAlt)) {
          const vr = ((tr.alt - refAlt) / dt) * 60;
          const fresh = prevPhase === null || tr.vrRefGround;
          tr.vrate = clamp(fresh ? vr : tr.vrate + SMOOTHING * (vr - tr.vrate), -MAX_VRATE_FPM, MAX_VRATE_FPM);
        } else if (dt > MAX_VRATE_DT_S) tr.vrate = 0;
        this.vrReference(tr, t);
      }
    }

    this.measuredRates(tr, t);
    tr.phase = decidePhase(tr, prevPhase);
    this.refit(tr, reportedOnly);
    noteArrival(tr, nowMs);
  }

  /**
   * Whether an airborne report is really of an aircraft on the ground.
   * Flightradar24's air/ground flag lags: it flags take-off rolls airborne
   * from ~80-110 kt, and landings on the ground 10-15 s after touchdown, and
   * meanwhile reports the runway's pressure altitude, level. So: below flying
   * speed; rolling on after a ground report (below rotation speed, or level);
   * level and braking after an approach; or, with no altitude, where the
   * model has it on the ground already. Each runway read this way gives the
   * field's QNH (a landing's only if it brings the aircraft down: a touchdown
   * never lifts it back up).
   */
  private onRunway(
    tr: Track,
    raw: number,
    prevOnGround: boolean,
    prevPhase: Phase | null,
    prevRaw: number,
    levelDt: number,
    estGround: boolean,
  ): boolean {
    const field = tr.field;
    if (!field || tr.cls === "heli" || tr.fieldKm > RUNWAY_FIELD_KM) return false;
    if (!(raw > 0)) return estGround;
    const agl = corrected(tr, raw, tr.rawW) - tr.fieldElev;
    if (!(Math.abs(agl) < (validQnh(tr.qnh) ? RUNWAY_TOL_KNOWN_FT : RUNWAY_TOL_UNKNOWN_FT))) return false;
    const speed = tr.speedKt;
    const level = prevRaw > 0 && Math.abs(raw - prevRaw) <= LEVEL_FT && levelDt <= LEVEL_MAX_DT_S;
    let runway: boolean;
    if (speed < MIN_FLYING_KT[tr.cls]) runway = true;
    else if (prevOnGround) runway = speed < ROTATION_KT[tr.cls] || level;
    else {
      runway =
        level &&
        prevPhase !== null &&
        FROM_LANDING.has(prevPhase) &&
        (tr.reportAccel <= -RUNWAY_DECEL_KTS || tr.accel <= -RUNWAY_DECEL_KTS);
    }
    if (runway) {
      const q = qnhFrom(raw, tr.fieldElev);
      if (validQnh(q)) {
        this.learnQnh(field, q, tr.reportMs);
        if (prevOnGround || agl >= 0) {
          tr.qnhOwn = q;
          tr.qnhOwnField = field;
        }
      }
    }
    return runway;
  }

  /** A ground report after an approach: its last altitude, moments ago, was the runway's (if it was near the field). */
  private touchedDown(tr: Track, prevRaw: number, levelDt: number, prevPhase: Phase | null): void {
    const field = tr.field;
    if (!field || tr.cls === "heli" || tr.fieldKm > RUNWAY_FIELD_KM || !(prevRaw > 0) || levelDt > LEVEL_MAX_DT_S) return;
    if (prevPhase !== "approach" && prevPhase !== "flare") return;
    const agl = corrected(tr, prevRaw, tr.rawW) - tr.fieldElev;
    if (Math.abs(agl) < (validQnh(tr.qnh) ? RUNWAY_TOL_KNOWN_FT : RUNWAY_TOL_UNKNOWN_FT))
      this.learnQnh(field, qnhFrom(prevRaw, tr.fieldElev), tr.reportMs);
  }

  /** The track's QNH at `t`; a change from `oldQnh` that moves its altitudes is eased in rather than snapped (see `setOffsets`). */
  private setQnh(tr: Track, t: number, oldQnh: number): void {
    tr.qnh = this.trackQnh(tr, t);
    if (!tr.onGround && tr.rawAlt > 0) tr.alt = corrected(tr, tr.rawAlt, tr.rawW);
    const na = tr.ats.length;
    if (na === 0) return;
    const raw = tr.aps[na - 1];
    const w = tr.aws[na - 1];
    if (Math.abs(correctAlt(raw, w, tr.qnh) - correctAlt(raw, w, oldQnh)) > 2 * LEVEL_FT) tr.altBlendRate = REFERENCE_BLEND_RATE;
  }

  /** The latest report (on the ground, or with an altitude) becomes the vertical rate's reference. */
  private vrReference(tr: Track, t: number): void {
    if (!tr.onGround && !(tr.rawAlt > 0)) return;
    tr.vrRefMs = t;
    tr.vrRefRaw = tr.rawAlt;
    tr.vrRefW = tr.rawW;
    tr.vrRefGround = tr.onGround;
  }

  /** Re-derives the extrapolation after samples or estimates changed without a newer report. */
  private refresh(tr: Track): void {
    this.refit(tr, tr.ts[tr.n - 1] === tr.reportMs);
  }

  /** Smoothed curves, extrapolation and predictions, the latest state's snapshot, and the measured roll's bias. */
  private refit(tr: Track, reportedOnly: boolean): void {
    fitTrack(tr, reportedOnly);
    fitAltitude(tr);
    this.predict(tr);
    turnNodes(tr);
    snapshot(tr);
    this.measuredAttitude(tr);
  }

  /** Takes in a measured state; a newer position also becomes the track's latest report. */
  private applyMeasured(tr: Track, m: Measured, newPosition: boolean, nowMs: number): void {
    tr.meas = m;
    tr.measMs = Math.max(tr.measMs, m.seenMs);
    const prevPhase = tr.phase;
    const oldQnh = tr.qnh;
    this.measuredQnh(tr, m);
    // the attitude's history, so that it is shown at the drawn time
    if (!m.onGround) {
      if (Number.isFinite(m.roll)) pushSeries(tr.rollTs, tr.rolls, m.seenMs, clamp(m.roll!, -MAX_BANK, MAX_BANK));
      const track = Number.isFinite(m.track) ? m.track! : tr.trackDeg;
      if (Number.isFinite(m.trueHeading))
        pushSeries(tr.crabTs, tr.crabs, m.seenMs, clamp(wrap180(m.trueHeading! - track), -MAX_CRAB, MAX_CRAB));
    }
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
        this.selectField(tr, null);
        if (tr.field !== tr.qnhOwnField) tr.qnhOwn = NaN;
        tr.qnh = this.trackQnh(tr, t);
        tr.onGround = !!m.onGround;
        tr.rawW = tr.field ? fieldFade(tr.fieldKm) : 0;
        tr.rawAltMs = t;
        // Flightradar24 reports 0 ft on the ground too
        if (tr.onGround) {
          tr.alt = 0;
          tr.rawAlt = 0;
          tr.groundMs = t;
        } else if (Number.isFinite(m.altBaro)) {
          tr.rawAlt = m.altBaro!;
          tr.alt = corrected(tr, tr.rawAlt, tr.rawW);
          addAltitude(tr, t, tr.rawAlt, tr.rawW);
        }
        if (Number.isFinite(m.gs)) tr.speedKt = Math.max(0, m.gs!);
        if (Number.isFinite(m.track)) tr.trackDeg = wrap360(m.track!);
        if (dtReport >= MIN_ACCEL_DT_S && dtReport <= MAX_ACCEL_DT_S) {
          const accel = (tr.speedKt - prevSpeed) / dtReport;
          tr.reportAccel = accel;
          tr.accel = clamp(tr.accel + SMOOTHING * (accel - tr.accel), -MAX_ACCEL_KTS, MAX_ACCEL_KTS);
        }
      }
    }
    this.setQnh(tr, tr.reportMs, oldQnh);
    this.measuredRates(tr, tr.reportMs);
    tr.phase = decidePhase(tr, prevPhase);
    this.refresh(tr);
    noteArrival(tr, nowMs);
  }

  /** The on-board altimeter setting near the field is its QNH: learned for it. Returns whether it was. */
  private measuredQnh(tr: Track, m: Measured): boolean {
    if (!tr.field || !validQnh(m.navQnh) || m.onGround || !(tr.alt - tr.fieldElev < MEAS_QNH_MAX_AGL_FT)) return false;
    this.learnQnh(tr.field, m.navQnh, m.seenMs);
    return true;
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
      this.vrReference(tr, t);
    }
    // the selected heading steers only when the autopilot isn't following a route (LNAV);
    // it is magnetic, so it needs the variation the two reported headings give
    const lnav = m.navModes?.some((mode) => mode.toLowerCase() === "lnav") ?? false;
    if (has(m.navHeading) && has(m.trueHeading) && has(m.magHeading) && !lnav) {
      const variation = wrap180(m.trueHeading - m.magHeading);
      tr.navTrack = wrap360(m.navHeading + variation - tr.crab);
    }
  }

  /** Each measured bank as an offset to the model's own at the time it was reported (interpolated at the drawn time). */
  private measuredAttitude(tr: Track): void {
    const n = tr.rollTs.length;
    tr.rollBiases.length = n;
    tr.rollBiases.fill(0); // the model's own bank (`modelBank`) is what is compared
    for (let k = 0; k < n; k++) {
      const raw = this.evaluate(tr, tr.rollTs[k], scratchC);
      tr.rollBiases[k] = raw.ground ? 0 : tr.rolls[k] - raw.modelBank;
    }
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
          // from the smoothed altitude, as drawn, where the newest altitude is this report's
          const na = tr.ats.length;
          const alt0 =
            na > 0 && tr.ats[na - 1] >= tr.reportMs - COINCIDENT_MS
              ? tr.afv[na - 1] + ((clamp(tr.afd[na - 1] * 60, -MAX_VRATE_FPM, MAX_VRATE_FPM) - tr.vrate) / 60) * VRATE_HANDOVER_S
              : tr.alt;
          const needed = Math.max(0, (alt0 - tr.fieldElev - FLARE_FLOOR_FT) / (-tr.vrate / 60));
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
          const vs = TOUCHDOWN_SINK * tr.vrate * verticalWeight((tr.touchdownMs - tr.reportMs) / 1000);
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

  /**
   * The model at drawn time T (server ms), without blending, into `out`: the
   * smoothed path inside the samples, the extrapolation past them, and the
   * vertical profile, phase and attitude of `state` (by default the report
   * state in effect at T).
   */
  private evaluate(tr: Track, T: number, out: Raw, state?: State): Raw {
    const S = state ?? stateAt(tr, T);
    // --- horizontal
    const n = tr.n;
    const last = n - 1;
    const tLast = tr.ts[last];
    let x: number;
    let y: number;
    let vx: number;
    let vy: number;
    let omega: number; // turn rate, rad/s
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
      // from the end of the smoothed path, along its direction
      x = tr.fx[last];
      y = tr.fy[last];
      if (Math.abs(k * sTurn) < 1e-9) {
        x += sTurn * Math.sin(h0);
        y += sTurn * Math.cos(h0);
      } else {
        // constant-curvature arc: d(east)/ds = sin h, d(north)/ds = cos h, h = h0 + k s
        x += (Math.cos(h0) - Math.cos(h)) / k;
        y += (Math.sin(h) - Math.sin(h0)) / k;
      }
      x += (s - sTurn) * Math.sin(h);
      y += (s - sTurn) * Math.cos(h);
      vx = v * Math.sin(h);
      vy = v * Math.cos(h);
      pathHeading = h;
      let turnRad: number;
      if (tr.turnEndS < TURN_HORIZON_S) {
        // rolling out onto the selected heading
        turnRad = dt < tr.turnEndS ? k * v : 0;
        bankFade = smoothstep((tr.turnEndS - dt) / ROLL_OUT_S);
      } else {
        turnRad = dt < TURN_HORIZON_S ? k * v : 0;
        bankFade = dt <= BANK_FADE_START_S ? 1 : Math.max(0, (TURN_HORIZON_S - dt) / (TURN_HORIZON_S - BANK_FADE_START_S));
      }
      // the smoothed path's own turn rate hands over to the prediction's
      omega = n > 1 ? tr.om[last] + (turnRad - tr.om[last]) * smoothstep(dt / TURN_HANDOVER_S) : turnRad;
    } else if (T <= tr.ts[0]) {
      const dt = Math.max((T - tr.ts[0]) / 1000, -BACKWARD_LIMIT_S);
      x = tr.fx[0] + tr.mx[0] * dt;
      y = tr.fy[0] + tr.my[0] * dt;
      vx = tr.mx[0];
      vy = tr.my[0];
      omega = tr.om[0];
    } else {
      // the smoothed path: a cubic Hermite piece between samples i and i+1
      hermite(tr, T);
      x = hx;
      y = hy;
      vx = hvx;
      vy = hvy;
      omega = tr.om[hi] + (tr.om[hi + 1] - tr.om[hi]) * hu;
    }
    const lat = clamp(tr.refLat + y / M_PER_DEG, -90, 90);
    const cosMid = Math.max(Math.cos(((tr.refLat + lat) / 2) * DEG), 1e-3);
    out.lat = lat;
    out.lon = wrapLon(tr.refLon + x / (M_PER_DEG * cosMid));
    out.vE = vx;
    out.vN = vy;
    const speed = Math.hypot(vx, vy);
    out.speedMs = speed;
    out.track =
      S.phase === "parked"
        ? S.trackDeg
        : wrap360((speed > HEADING_MIN_SPEED_MS ? Math.atan2(vx, vy) : pathHeading) / DEG);
    // measured values count while fresh (the nose's crab, the bank, the airspeed), then ease out;
    // the crab and bank as they were at T
    const mw = measWeight(tr, T);
    out.heading = mw > 0 ? wrap360(out.track + seriesAt(tr.crabTs, tr.crabs, T, tr.crab) * mw) : out.track;

    // --- vertical, phase and attitude
    let phase = S.phase;
    const field = S.field;
    const fieldElev = S.fieldElev;
    const floor = field ? fieldElev : Math.min(0, S.alt);
    out.hasField = field !== null;
    out.bank = 0;
    out.modelBank = 0;
    out.vsFpm = 0;
    out.agl = NaN;
    if (GROUND.has(phase)) {
      out.ground = true;
      out.altMsl = field ? fieldElev : 0;
      out.altRaw = out.altMsl;
      out.vsRaw = 0;
      out.pitch = 0;
      if (phase === "landingRoll" && speed < TAXI_MAX_KT * KNOTS_TO_MS) phase = "taxi";
      const lo = S.liftoffMs;
      if (phase === "takeoffRoll" && Number.isFinite(lo)) {
        if (T < lo) out.pitch = ROTATION_PITCH * smoothstep(1 - (lo - T) / (ROTATION_S * 1000));
        else {
          // predicted lift-off and initial climb, leaving the ground through the flare's curve
          const u = (T - lo) / 1000;
          const raw = FLARE_FLOOR_FT + (TAKEOFF_CLIMB_FPM * verticalIntegral(u)) / 60;
          const vs = TAKEOFF_CLIMB_FPM * verticalWeight(u);
          out.ground = false;
          out.agl = flare(raw);
          out.altRaw = out.altMsl + raw;
          out.vsRaw = vs;
          out.altMsl = Math.min(out.altMsl + out.agl, MAX_ALT_FT);
          out.vsFpm = vs * flareSlope(raw);
          phase = out.agl < APPROACH_MAX_AGL_FT ? "initialClimb" : "climb";
          const settled = airPitch(vs, speed, phase, out.agl);
          out.pitch = ROTATION_PITCH + (settled - ROTATION_PITCH) * smoothstep(u / ROTATION_S);
        }
      }
    } else {
      const td = S.touchdownMs;
      if (Number.isFinite(td) && T >= td) {
        // predicted landing roll
        out.ground = true;
        out.altMsl = fieldElev;
        out.altRaw = fieldElev + FLARE_FLOOR_FT;
        out.vsRaw = 0;
        out.agl = 0;
        phase = speed < TAXI_MAX_KT * KNOTS_TO_MS ? "taxi" : "landingRoll";
        out.pitch = S.touchdownPitch * (1 - smoothstep((T - td) / (TOUCHDOWN_EASE_S * 1000)));
      } else {
        airAltitude(tr, S, T);
        let alt = amAlt;
        let vs = amVs;
        if (field && Number.isFinite(td) && T > td - TOUCHDOWN_FIT_MS) {
          // the last moments before the predicted touchdown: height still left above the
          // flare's floor then (the data moved on since it was predicted: a QNH re-read, say)
          // eases out, so the wheels meet the runway when the landing roll begins
          airAltitude(tr, S, td);
          const excess = amAlt - (fieldElev + FLARE_FLOOR_FT);
          if (excess > 0) {
            const u = 1 - (td - T) / TOUCHDOWN_FIT_MS;
            alt -= excess * smoothstep(u);
            vs -= ((excess * 6 * u * (1 - u)) / TOUCHDOWN_FIT_MS) * 60_000;
          }
        }
        out.ground = false;
        out.altRaw = alt;
        out.vsRaw = vs;
        if (field) {
          // near the ground, the flare: into the runway at a believable sink rate, never through it
          const raw = Math.min(alt, MAX_ALT_FT) - fieldElev;
          out.agl = flare(raw);
          out.altMsl = fieldElev + out.agl;
          out.vsFpm = vs * flareSlope(raw);
          if (phase === "approach" && out.agl < FLARE_MAX_AGL_FT) phase = "flare";
        } else {
          out.altMsl = clamp(alt, floor, MAX_ALT_FT);
          out.vsFpm = alt === out.altMsl ? vs : 0;
        }
        // the flight-path angle is through the air: true airspeed when measured
        const airSpeed = mw > 0 && tr.tasMs > 0 ? speed + (tr.tasMs - speed) * mw : speed;
        out.pitch = airPitch(out.vsFpm, airSpeed, phase, out.agl);
        const bank = Math.atan2(speed * omega, GRAVITY_MS2) / DEG;
        out.modelBank = clamp(bank * bankFade, -MAX_BANK, MAX_BANK);
        const bias = mw > 0 ? seriesAt(tr.rollTs, tr.rollBiases, T, 0) * mw : 0;
        out.bank = clamp(out.modelBank + bias, -MAX_BANK, MAX_BANK);
      }
    }
    out.phase = phase;
    if (out.ground) out.drawAlt = 0;
    else if (Number.isFinite(S.liftoffMs) && !field) out.drawAlt = out.agl; // predicted climb off an unknown field
    else out.drawAlt = Math.max(0, out.altMsl - fieldRemoval(tr, out.lat, out.lon));
    return out;
  }

  /** Gear fraction, animating towards the target for this pose. */
  private gear(tr: Track, raw: Raw, nowMs: number): number {
    let down: boolean;
    const phase = raw.phase;
    if (raw.ground || GROUND.has(phase)) down = true;
    else if (raw.hasField) {
      down =
        ((phase === "approach" || phase === "flare") && raw.agl < GEAR_APPROACH_AGL_FT) ||
        (phase === "initialClimb" && raw.agl < GEAR_CLIMB_AGL_FT) ||
        // whatever the phase (no altitude to tell one): just above the field and slow
        (raw.agl < GEAR_CLIMB_AGL_FT && raw.speedMs < GEAR_FALLBACK_KT * KNOTS_TO_MS);
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

// Results of `airAltitude`.
let amAlt = 0;
let amVs = 0;

/**
 * The model's airborne altitude (ft, corrected) and vertical rate (fpm) at T
 * for a state, before the flare shapes them near the ground, into amAlt/amVs:
 * the smoothed altitude curve inside its samples, and past them on from it at
 * the state's estimated rate (levelling off at a selected altitude).
 */
function airAltitude(tr: Track, S: State, T: number): void {
  // the altitude samples count for a state that knew of them: one from before the first
  // (its altitude estimated, say) carries on its own way, so that the state taking over
  // when the drawn time reaches that first sample's report blends the difference out
  const na = S.reportMs >= tr.ats[0] ? tr.ats.length : 0;
  let alt: number;
  let vs: number;
  if (na > 1 && T >= tr.ats[0] && T <= tr.ats[na - 1]) {
    // inside the altitude samples: the smoothed curve and its slope
    altitudeAt(tr, T);
    alt = hx;
    vs = hvx * 60;
  } else if (na > 0 && T < tr.ats[0]) {
    // before them (a new track is drawn from its buffer's history): back along the
    // smoothed curve's start, as the horizontal path is, so it meets the curve
    vs = clamp(tr.afd[0] * 60, -MAX_VRATE_FPM, MAX_VRATE_FPM);
    alt = tr.afv[0] + (vs / 60) * Math.max((T - tr.ats[0]) / 1000, -BACKWARD_LIMIT_S);
  } else {
    // past them: on from the smoothed newest altitude (or the report) at the
    // estimated rate, the curve's own rate easing into it (C1)
    let baseMs = S.reportMs;
    let base = S.alt;
    let baseRate = S.vrate * verticalWeight(0);
    if (na > 0 && T >= tr.ats[0]) {
      baseMs = tr.ats[na - 1];
      base = tr.afv[na - 1];
      baseRate = clamp(tr.afd[na - 1] * 60, -MAX_VRATE_FPM, MAX_VRATE_FPM);
    }
    const dtR = (T - S.reportMs) / 1000;
    const dtB = (baseMs - S.reportMs) / 1000;
    const extra = baseRate - S.vrate * verticalWeight(dtB);
    const ease = Math.exp(-Math.max(0, (T - baseMs) / 1000) / VRATE_HANDOVER_S);
    let climb =
      (S.vrate * (verticalIntegral(dtR) - verticalIntegral(dtB))) / 60 + (extra / 60) * VRATE_HANDOVER_S * (1 - ease);
    let vsScale = 1;
    let vsWeight = verticalWeight(dtR);
    const cap = S.capAlt;
    if (cap === cap) {
      // altitude capture: ease onto the selected altitude (C1: the rate goes smoothly to 0)
      const d = cap - base;
      const band = clamp(Math.abs(S.vrate) * CAPTURE_FT_PER_FPM, MIN_CAPTURE_FT, MAX_CAPTURE_FT);
      const kk = Math.min(1, band / Math.abs(d));
      const xx = climb / d;
      if (xx >= 1 + kk) {
        climb = d;
        vsScale = 0;
      } else if (xx > 1 - kk) {
        const u = xx - (1 - kk);
        climb = d * (xx - (u * u) / (4 * kk));
        vsScale = 1 - u / (2 * kk);
      }
    }
    alt = base + climb;
    vs = (S.vrate * vsWeight + extra * ease) * vsScale;
  }
  amAlt = alt;
  amVs = vs;
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

// Scratch for `mergeReport`: a report's own samples, oldest first, and the merged result
// (reports are merged for every aircraft in every snapshot).
const newT: number[] = [];
const newLat: number[] = [];
const newLon: number[] = [];
const outT: number[] = [];
const outLat: number[] = [];
const outLon: number[] = [];
const outSrc: number[] = [];
/** Time of the oldest sample the last report merged brought: its buffer's oldest point, or its own time. */
let reportFromMs = 0;

/**
 * A report's own samples into newT/newLat/newLon, oldest first: its position
 * buffer, which is the aircraft's recent history (each point `dMs` before the
 * report, at `lat/lon - d·1e-5`; Flightradar24 sends the newest first), then
 * the report point itself, the newest. Of two points closer together than
 * MIN_SAMPLE_GAP_MS the later one in the buffer is dropped.
 */
function reportPoints(f: LiveFlight, t: number, lat0: number, lon0: number): void {
  newT.length = newLat.length = newLon.length = 0;
  const positions = f.positions;
  if (positions) {
    for (let k = 0; k < positions.length; k++) {
      const [dLat, dLon, dMs] = positions[k];
      const lat = lat0 - dLat * BUFFER_SCALE;
      const lon = lon0 - dLon * BUFFER_SCALE;
      if (!(dMs >= MIN_SAMPLE_GAP_MS && dMs <= SAMPLE_WINDOW_MS) || !(Math.abs(lat) <= 90) || !Number.isFinite(lon)) continue;
      const time = t - dMs;
      let j = newT.length;
      while (j > 0 && newT[j - 1] > time) j--;
      if ((j > 0 && time - newT[j - 1] < MIN_SAMPLE_GAP_MS) || (j < newT.length && newT[j] - time < MIN_SAMPLE_GAP_MS)) continue;
      newT.splice(j, 0, time);
      newLat.splice(j, 0, lat);
      newLon.splice(j, 0, wrapLon(lon));
    }
  }
  newT.push(t);
  newLat.push(lat0);
  newLon.push(lon0);
  reportFromMs = newT[0];
}

/**
 * Whether one of the report's points (in newT/newLat/newLon) is well off the
 * path the held samples smoothed to, inside them or just past them: the old
 * samples were wrong (or this is a jump in the data), and would poison the
 * spline.
 */
function offPath(tr: Track): boolean {
  const n = tr.n;
  if (n < 2 || tr.fx.length !== n || tr.ts.length !== n) return false;
  const { ts } = tr;
  const last = n - 1;
  for (let j = 0; j < newT.length; j++) {
    const t = newT[j];
    if (t < ts[0] || t > ts[last] + JUMP_LOOKAHEAD_MS) continue;
    let x: number;
    let y: number;
    if (t <= ts[last]) {
      hermite(tr, t);
      x = hx;
      y = hy;
    } else {
      // just past them: straight on is good to well inside JUMP_M over a few seconds
      const dt = (t - ts[last]) / 1000;
      x = tr.fx[last] + tr.mx[last] * dt;
      y = tr.fy[last] + tr.my[last] * dt;
    }
    const lat = tr.refLat + y / M_PER_DEG;
    const lon = tr.refLon + x / (M_PER_DEG * Math.max(Math.cos(((tr.refLat + lat) / 2) * DEG), 1e-3));
    if (distanceKm(lat, lon, newLat[j], newLon[j]) * 1000 > JUMP_M) return true;
  }
  return false;
}

/**
 * Merges a Flightradar24 report at `t` (position `lat0`/`lon0`) into the
 * track's samples: the report point, the newest, and its buffer, the ~10 s
 * before it. The buffer overlaps what earlier reports brought (it holds their
 * positions, to a metre or so), so samples within COINCIDENT_MS of each other
 * are the same moment: a held sample of a better source (ADS-B) is kept,
 * otherwise the newer arrival replaces it. Buffer points older than every
 * held sample add nothing the track's record needs and are left out, and only
 * the last SAMPLE_WINDOW_MS is kept.
 *
 * A report that jumps away from the held samples (implausibly far from the
 * nearest, or a point of it well off their smoothed path) starts them over,
 * unless it is `older` than the measured state (which then wins): it is
 * dropped, and the result is null. Otherwise returns whether the newest sample
 * is the report point itself, whose velocity is then the reported one.
 */
function mergeReport(tr: Track, f: LiveFlight, t: number, lat0: number, lon0: number, older: boolean): boolean | null {
  reportPoints(f, t, lat0, lon0);
  const { ts, lats, lons, src } = tr;
  if (ts.length > 0) {
    const near = nearestSample(tr, t);
    const far =
      distanceKm(lats[near], lons[near], lat0, lon0) > Math.min(plausibleKm(tr, (t - ts[near]) / 1000), SNAP_DISTANCE_M / 1000);
    if (far || (!tr.mixed && offPath(tr))) {
      if (older) return null;
      clearSamples(tr);
    }
  }
  const mixed = tr.mixed;
  const held = ts.length;
  // which of the report's points go in: none older than the record, none that a better source has
  const from = held > 0 ? ts[0] : -Infinity;
  const last = newT.length - 1;
  let m = 0;
  for (let j = 0; j <= last; j++) {
    const time = newT[j];
    if (time < from && j < last) continue;
    let outranked = false;
    if (mixed) {
      for (let i = 0; i < held; i++) {
        if (Math.abs(ts[i] - time) < COINCIDENT_MS && src[i] > SRC_FR24) {
          outranked = true;
          break;
        }
      }
    }
    if (outranked) continue;
    newT[m] = time;
    newLat[m] = newLat[j];
    newLon[m] = newLon[j];
    m++;
  }
  // both in time order, a held sample giving way to a new point of the same moment
  outT.length = outLat.length = outLon.length = outSrc.length = 0;
  let i = 0;
  let j = 0;
  while (i < held || j < m) {
    if (j >= m || (i < held && ts[i] < newT[j])) {
      const ti = ts[i];
      if (!((j > 0 && ti - newT[j - 1] < COINCIDENT_MS) || (j < m && newT[j] - ti < COINCIDENT_MS))) {
        outT.push(ti);
        outLat.push(lats[i]);
        outLon.push(lons[i]);
        if (mixed) outSrc.push(src[i]);
      }
      i++;
    } else {
      outT.push(newT[j]);
      outLat.push(newLat[j]);
      outLon.push(newLon[j]);
      if (mixed) outSrc.push(SRC_FR24);
      j++;
    }
  }
  const n = outT.length;
  ts.length = lats.length = lons.length = n;
  for (let k = 0; k < n; k++) {
    ts[k] = outT[k];
    lats[k] = outLat[k];
    lons[k] = outLon[k];
  }
  if (mixed) {
    src.length = n;
    for (let k = 0; k < n; k++) src[k] = outSrc[k];
  }
  finishSamples(tr);
  return tr.ts[tr.n - 1] === t;
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

/** Window trim, sample count, source bookkeeping and the local frame (around the newest sample), after a merge. */
function finishSamples(tr: Track): void {
  const { ts, lats, lons } = tr;
  const newest = ts[ts.length - 1];
  let start = 0;
  while (start < ts.length - 1 && ts[start] < newest - SAMPLE_WINDOW_MS) start++;
  if (start) {
    ts.splice(0, start);
    lats.splice(0, start);
    lons.splice(0, start);
    if (tr.mixed) tr.src.splice(0, start);
  }
  const n = (tr.n = ts.length);
  // still mixed while measured samples are left; per-sample sources are kept only then
  tr.mixed = tr.mixed && tr.src.some((s) => s !== SRC_FR24);
  if (!tr.mixed) tr.src.length = 0;
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

// --- Smoothing -------------------------------------------------------------------------

/** Solver scratch, grown as needed: fits run for every aircraft in every snapshot. */
let splineBuf = new Float64Array(0);
const weightBuf: number[] = [];

/**
 * Cubic smoothing spline. Finds the piecewise cubic f, in Hermite form (node
 * values `f` and slopes `d` at the sample times `ts`, ms), that minimises
 *
 *     Σ w_i (f(t_i) - y_i)²  +  λ ∫ f''(t)² dt                  (t in seconds)
 *     [ + endSlopeW (f'(t_last) - endSlope)² ]
 *
 * i.e. the smoothest curve that stays close to the samples: it approximates
 * them instead of passing through each noisy one. The optimum is the natural
 * smoothing spline, which is C2. λ sets how far it averages: about
 * (λ / (w · samples per second))^¼ seconds either side, so sparse samples
 * are followed closely and dense noisy ones averaged. The normal equations are
 * block tridiagonal (2×2 blocks: value and slope per sample), solved in O(n);
 * series `a` and `b` (east and north) share the system.
 */
function smoothSpline(
  n: number,
  ts: number[],
  ws: number[] | null,
  lambda: number,
  ya: number[],
  fa: number[],
  da: number[],
  yb: number[] | null,
  fb: number[] | null,
  db: number[] | null,
  endSlopeW: number,
  endSlopeA: number,
  endSlopeB: number,
): void {
  const S = 11;
  if (splineBuf.length < n * S) splineBuf = new Float64Array(Math.max(n * S, 2 * splineBuf.length));
  const B = splineBuf;
  // assembly and forward elimination
  for (let i = 0; i < n; i++) {
    const w = ws ? ws[i] : 1;
    let a = w;
    let b = 0;
    let c = 0;
    let u00 = 0;
    let u01 = 0;
    let u10 = 0;
    let u11 = 0;
    let raf = w * ya[i];
    let rad = 0;
    let rbf = yb ? w * yb[i] : 0;
    let rbd = 0;
    // ∫f''² over a piece of length h is (4/h³)[3Δ² - 3hΔ(d0+d1) + h²(d0² + d0 d1 + d1²)], Δ = f1 - f0
    if (i > 0) {
      const h = (ts[i] - ts[i - 1]) / 1000;
      const cc = (lambda * 4) / (h * h * h);
      a += 3 * cc;
      b -= 1.5 * h * cc;
      c += h * h * cc;
    }
    if (i < n - 1) {
      const h = (ts[i + 1] - ts[i]) / 1000;
      const cc = (lambda * 4) / (h * h * h);
      a += 3 * cc;
      b += 1.5 * h * cc;
      c += h * h * cc;
      u00 = -3 * cc;
      u01 = 1.5 * h * cc;
      u10 = -1.5 * h * cc;
      u11 = 0.5 * h * h * cc;
    }
    if (i === n - 1 && endSlopeW > 0) {
      c += endSlopeW;
      rad += endSlopeW * endSlopeA;
      rbd += endSlopeW * endSlopeB;
    }
    if (i > 0) {
      // eliminate the previous block: L = Uᵀ D⁻¹, D -= L U, r -= L r'
      const o = (i - 1) * S;
      const det = B[o] * B[o + 2] - B[o + 1] * B[o + 1];
      const i00 = B[o + 2] / det;
      const i01 = -B[o + 1] / det;
      const i11 = B[o] / det;
      const q00 = B[o + 3];
      const q01 = B[o + 4];
      const q10 = B[o + 5];
      const q11 = B[o + 6];
      const l00 = q00 * i00 + q10 * i01;
      const l01 = q00 * i01 + q10 * i11;
      const l10 = q01 * i00 + q11 * i01;
      const l11 = q01 * i01 + q11 * i11;
      a -= l00 * q00 + l01 * q10;
      b -= l00 * q01 + l01 * q11;
      c -= l10 * q01 + l11 * q11;
      raf -= l00 * B[o + 7] + l01 * B[o + 8];
      rad -= l10 * B[o + 7] + l11 * B[o + 8];
      rbf -= l00 * B[o + 9] + l01 * B[o + 10];
      rbd -= l10 * B[o + 9] + l11 * B[o + 10];
    }
    const o = i * S;
    B[o] = a;
    B[o + 1] = b;
    B[o + 2] = c;
    B[o + 3] = u00;
    B[o + 4] = u01;
    B[o + 5] = u10;
    B[o + 6] = u11;
    B[o + 7] = raf;
    B[o + 8] = rad;
    B[o + 9] = rbf;
    B[o + 10] = rbd;
  }
  // back substitution
  let zaf = 0;
  let zad = 0;
  let zbf = 0;
  let zbd = 0;
  for (let i = n - 1; i >= 0; i--) {
    const o = i * S;
    let raf = B[o + 7];
    let rad = B[o + 8];
    let rbf = B[o + 9];
    let rbd = B[o + 10];
    if (i < n - 1) {
      raf -= B[o + 3] * zaf + B[o + 4] * zad;
      rad -= B[o + 5] * zaf + B[o + 6] * zad;
      rbf -= B[o + 3] * zbf + B[o + 4] * zbd;
      rbd -= B[o + 5] * zbf + B[o + 6] * zbd;
    }
    const det = B[o] * B[o + 2] - B[o + 1] * B[o + 1];
    const i00 = B[o + 2] / det;
    const i01 = -B[o + 1] / det;
    const i11 = B[o] / det;
    zaf = i00 * raf + i01 * rad;
    zad = i01 * raf + i11 * rad;
    zbf = i00 * rbf + i01 * rbd;
    zbd = i01 * rbf + i11 * rbd;
    fa[i] = zaf;
    da[i] = zad;
    if (fb && db) {
      fb[i] = zbf;
      db[i] = zbd;
    }
  }
}

// Scratch for `fitTrack`: the reference path and the residuals' fit.
const refX: number[] = [];
const refY: number[] = [];
const refVX: number[] = [];
const refVY: number[] = [];
const resX: number[] = [];
const resY: number[] = [];
const resFX: number[] = [];
const resFY: number[] = [];
const resDX: number[] = [];
const resDY: number[] = [];

/** Seconds' worth of the reference turn (and speed change) `back` seconds before the newest sample: full, then fading out. */
function referenceSpan(back: number): number {
  if (back <= REF_FULL_S) return back;
  const span = REF_ZERO_S - REF_FULL_S;
  const u = Math.min(back, REF_ZERO_S) - REF_FULL_S;
  return REF_FULL_S + u - (u * u) / (2 * span);
}

/**
 * The smoothed horizontal path through the samples (values fx/fy, slopes
 * mx/my). A smoothing spline penalises acceleration, so on its own it would
 * straighten a steady turn (a constant sideways acceleration) by tens of
 * metres. Instead it smooths the samples' difference from a reference path
 * that turns, and changes speed, as the data itself does over the last
 * ~12 s, then adds the reference back: a steady turn costs nothing and only
 * the jitter is taken out. The first pass's reference takes the report's
 * estimates; the next two read the turn and acceleration off the pass before
 * (each pass cuts the dependence on the estimates ~6-fold), so the result
 * comes from the data and changes as little as it does from report to report. A newest
 * sample that is the report itself (as it is unless a measured one is newer)
 * takes the reported velocity as its slope, softly.
 */
function fitTrack(tr: Track, reportedOnly: boolean): void {
  const n = tr.n;
  tr.fx.length = tr.fy.length = tr.mx.length = tr.my.length = n;
  const v = tr.phase === "parked" ? 0 : tr.speedKt * KNOTS_TO_MS;
  const rx = v * Math.sin(tr.trackDeg * DEG);
  const ry = v * Math.cos(tr.trackDeg * DEG);
  if (n === 1) {
    tr.fx[0] = tr.xs[0];
    tr.fy[0] = tr.ys[0];
    tr.mx[0] = rx;
    tr.my[0] = ry;
    return;
  }
  let ws: number[] | null = null;
  if (tr.mixed) {
    weightBuf.length = n;
    for (let i = 0; i < n; i++)
      weightBuf[i] = tr.src[i] === SRC_MLAT ? WEIGHT_MLAT : tr.src[i] === SRC_ADSB ? WEIGHT_ADSB : WEIGHT_FR24;
    ws = weightBuf;
  }
  // seeded with the report's estimates, then refined from the data itself
  const moving = tr.phase !== "parked" && v > HEADING_MIN_SPEED_MS;
  let omega = moving ? tr.turnRate * DEG : 0;
  let accel = moving ? tr.accel * KNOTS_TO_MS : 0;
  for (let pass = 0; pass < REF_PASSES; pass++) {
    fitAround(tr, ws, reportedOnly, rx, ry, omega, accel, pass === 0);
    if (pass === REF_PASSES - 1 || tr.phase === "parked") break;
    // the turn and speed change of this pass's path: two chords over the last ~12 s
    const tEnd = tr.ts[n - 1];
    const t0 = Math.max(tr.ts[0], tEnd - 2 * REF_CHORD_MS);
    const w = (tEnd - t0) / 2000;
    if (w < REF_MIN_CHORD_S) break;
    hermite(tr, t0);
    const x0 = hx;
    const y0 = hy;
    hermite(tr, t0 + w * 1000);
    const x1 = hx;
    const y1 = hy;
    const x2 = tr.fx[n - 1];
    const y2 = tr.fy[n - 1];
    const s1 = Math.hypot(x1 - x0, y1 - y0) / w;
    const s2 = Math.hypot(x2 - x1, y2 - y1) / w;
    const turn = s1 > HEADING_MIN_SPEED_MS && s2 > HEADING_MIN_SPEED_MS
      ? (wrap180((Math.atan2(x2 - x1, y2 - y1) - Math.atan2(x1 - x0, y1 - y0)) / DEG) * DEG) / w
      : 0;
    const nextOmega = clamp(turn, -MAX_TURN_DEG_S * DEG, MAX_TURN_DEG_S * DEG);
    const nextAccel = clamp((s2 - s1) / w, -MAX_ACCEL_KTS * KNOTS_TO_MS, MAX_ACCEL_KTS * KNOTS_TO_MS);
    // straight and steady, as assumed: the fit stands
    if (Math.abs(nextOmega - omega) < 0.02 * DEG && Math.abs(nextAccel - accel) < 0.02) break;
    omega = nextOmega;
    accel = nextAccel;
  }
}

/** One pass of `fitTrack`: the smoothed residual from a reference turning at `omega` (rad/s) and accelerating at `accel` (m/s²). */
function fitAround(
  tr: Track,
  ws: number[] | null,
  reportedOnly: boolean,
  rx: number,
  ry: number,
  omega: number,
  accel: number,
  first: boolean,
): void {
  const n = tr.n;
  // the reference, integrated back from the newest sample (from the report's velocity, or the previous pass's):
  // heading h(a) = hEnd - ω F(a), speed v(a) = vEnd - accel F(a), `a` seconds back, F = referenceSpan
  const plain = omega === 0 && accel === 0;
  const tEnd = tr.ts[n - 1];
  let vEnd = 0;
  let hEnd = 0;
  if (!plain && first) {
    // the reported velocity, carried on to the newest sample
    const since = clamp((tEnd - tr.reportMs) / 1000, 0, TURN_HORIZON_S);
    vEnd = Math.max(0, Math.hypot(rx, ry) + accel * Math.min(since, ACCEL_HORIZON_S));
    hEnd = tr.trackDeg * DEG + omega * since;
  } else if (!plain) {
    vEnd = Math.hypot(tr.mx[n - 1], tr.my[n - 1]);
    hEnd = Math.atan2(tr.mx[n - 1], tr.my[n - 1]);
  }
  refX.length = refY.length = refVX.length = refVY.length = resX.length = resY.length = n;
  let x = 0;
  let y = 0;
  refX[n - 1] = 0;
  refY[n - 1] = 0;
  refVX[n - 1] = vEnd * Math.sin(hEnd);
  refVY[n - 1] = vEnd * Math.cos(hEnd);
  for (let i = n - 2; i >= 0; i--) {
    if (plain) {
      refX[i] = refY[i] = refVX[i] = refVY[i] = 0;
      continue;
    }
    const a0 = (tEnd - tr.ts[i + 1]) / 1000;
    const a1 = (tEnd - tr.ts[i]) / 1000;
    const steps = Math.max(1, Math.ceil(a1 - a0));
    const da = (a1 - a0) / steps;
    for (let k = 0; k < steps; k++) {
      const F = referenceSpan(a0 + (k + 0.5) * da);
      const h = hEnd - omega * F;
      const speed = Math.max(0, vEnd - accel * F);
      x -= speed * Math.sin(h) * da;
      y -= speed * Math.cos(h) * da;
    }
    const F = referenceSpan(a1);
    const h = hEnd - omega * F;
    const speed = Math.max(0, vEnd - accel * F);
    refX[i] = x;
    refY[i] = y;
    refVX[i] = speed * Math.sin(h);
    refVY[i] = speed * Math.cos(h);
  }
  for (let i = 0; i < n; i++) {
    resX[i] = tr.xs[i] - refX[i];
    resY[i] = tr.ys[i] - refY[i];
  }
  resFX.length = resFY.length = resDX.length = resDY.length = n;
  smoothSpline(
    n,
    tr.ts,
    ws,
    SMOOTH_LAMBDA,
    resX,
    resFX,
    resDX,
    resY,
    resFY,
    resDY,
    reportedOnly ? REPORTED_SLOPE_WEIGHT : 0,
    rx - refVX[n - 1],
    ry - refVY[n - 1],
  );
  for (let i = 0; i < n; i++) {
    tr.fx[i] = resFX[i] + refX[i];
    tr.fy[i] = resFY[i] + refY[i];
    tr.mx[i] = resDX[i] + refVX[i];
    tr.my[i] = resDY[i] + refVY[i];
  }
}

/** The smoothed altitude through the airborne altitude samples, corrected with the track's QNH. */
function fitAltitude(tr: Track): void {
  const n = tr.ats.length;
  tr.avs.length = tr.afv.length = tr.afd.length = n;
  if (n === 0) return;
  for (let i = 0; i < n; i++) tr.avs[i] = corrected(tr, tr.aps[i], tr.aws[i]);
  if (n === 1) {
    tr.afv[0] = tr.avs[0];
    tr.afd[0] = tr.vrate / 60;
    return;
  }
  smoothSpline(n, tr.ats, null, ALT_SMOOTH_LAMBDA, tr.avs, tr.afv, tr.afd, null, null, null, 0, 0, 0);
}

/**
 * An airborne pressure altitude (ft) at `t`, corrected with weight `w` (the
 * field fade); a sample within a second of the newest replaces it, an older
 * one is dropped. The first one after the aircraft was on the ground starts
 * a new flight segment: the samples of the approach before it are kept until
 * then, so the drawn descent carries on into the touchdown rather than
 * restarting from the latest report when the ground report comes in.
 */
function addAltitude(tr: Track, t: number, raw: number, w: number): void {
  const { ats, aps, aws } = tr;
  let n = ats.length;
  if (n && ats[n - 1] < tr.groundMs) ats.length = aps.length = aws.length = n = 0;
  if (n && t < ats[n - 1] - COINCIDENT_MS) return;
  // a change no aircraft can fly (beyond 25 ft steps) is a jump in the data: start over
  if (n && Math.abs(raw - aps[n - 1]) > ALT_STEP_FT) {
    const dtMin = Math.max(t - ats[n - 1], COINCIDENT_MS) / 60_000;
    if (Math.abs(raw - aps[n - 1]) / dtMin > 1.25 * MAX_VRATE_FPM) ats.length = aps.length = aws.length = n = 0;
  }
  if (n && Math.abs(t - ats[n - 1]) < COINCIDENT_MS) {
    ats[n - 1] = Math.max(t, ats[n - 1]);
    aps[n - 1] = raw;
    aws[n - 1] = w;
  } else {
    ats.push(t);
    aps.push(raw);
    aws.push(w);
  }
  while (ats.length > 1 && ats[0] < t - ALT_WINDOW_MS) {
    ats.shift();
    aps.shift();
    aws.shift();
  }
}

/** A pressure altitude (ft) corrected to true altitude at `qnh` with weight `w` (none without a QNH). */
function correctAlt(raw: number, w: number, qnh: number): number {
  return w > 0 && validQnh(qnh) ? raw + w * (trueAltitude(raw, qnh) - raw) : raw;
}

/** A pressure altitude (ft) corrected with the track's QNH and weight `w`. */
const corrected = (tr: Track, raw: number, w: number) => correctAlt(raw, w, tr.qnh);

// Scratch results of `hermite`/`altitudeAt`: value(s), slope(s), segment and fraction.
let hx = 0;
let hy = 0;
let hvx = 0;
let hvy = 0;
let hi = 0;
let hu = 0;

/** The smoothed horizontal path at T (inside the samples) into hx/hy (m) and hvx/hvy (m/s), segment hi at fraction hu. */
function hermite(tr: Track, T: number): void {
  const ts = tr.ts;
  let i = tr.n - 2;
  while (i > 0 && T < ts[i]) i--;
  const h = (ts[i + 1] - ts[i]) / 1000;
  const u = (T - ts[i]) / 1000 / h;
  const u2 = u * u;
  const u3 = u2 * u;
  const h00 = 2 * u3 - 3 * u2 + 1;
  const h10 = u3 - 2 * u2 + u;
  const h01 = 3 * u2 - 2 * u3;
  const h11 = u3 - u2;
  const d00 = 6 * u2 - 6 * u;
  const d10 = 3 * u2 - 4 * u + 1;
  const d11 = 3 * u2 - 2 * u;
  const { fx, fy, mx, my } = tr;
  hx = h00 * fx[i] + h10 * h * mx[i] + h01 * fx[i + 1] + h11 * h * mx[i + 1];
  hy = h00 * fy[i] + h10 * h * my[i] + h01 * fy[i + 1] + h11 * h * my[i + 1];
  hvx = (d00 * (fx[i] - fx[i + 1])) / h + d10 * mx[i] + d11 * mx[i + 1];
  hvy = (d00 * (fy[i] - fy[i + 1])) / h + d10 * my[i] + d11 * my[i + 1];
  hi = i;
  hu = u;
}

/** The smoothed altitude at T (inside its samples) into hx (ft) and hvx (ft/s). */
function altitudeAt(tr: Track, T: number): void {
  const ts = tr.ats;
  let i = ts.length - 2;
  while (i > 0 && T < ts[i]) i--;
  const h = (ts[i + 1] - ts[i]) / 1000;
  const u = (T - ts[i]) / 1000 / h;
  const u2 = u * u;
  const u3 = u2 * u;
  const f = tr.afv;
  const d = tr.afd;
  hx = (2 * u3 - 3 * u2 + 1) * f[i] + (u3 - 2 * u2 + u) * h * d[i] + (3 * u2 - 2 * u3) * f[i + 1] + (u3 - u2) * h * d[i + 1];
  hvx = ((6 * u2 - 6 * u) * (f[i] - f[i + 1])) / h + (3 * u2 - 4 * u + 1) * d[i] + (3 * u2 - 2 * u) * d[i + 1];
}

/**
 * The smoothed path's turn rate at each sample (rad/s, for the bank): its
 * heading's change over ±2 s, which averages out what jitter is left.
 */
function turnNodes(tr: Track): void {
  const n = tr.n;
  tr.om.length = n;
  const first = tr.ts[0];
  const newest = tr.ts[n - 1];
  const max = MAX_TURN_DEG_S * DEG;
  for (let i = 0; i < n; i++) {
    const a = Math.max(first, tr.ts[i] - TURN_WINDOW_MS);
    const b = Math.min(newest, tr.ts[i] + TURN_WINDOW_MS);
    let w = 0;
    if (b - a >= 500) {
      hermite(tr, a);
      const ax = hvx;
      const ay = hvy;
      hermite(tr, b);
      if (Math.hypot(ax, ay) > HEADING_MIN_SPEED_MS && Math.hypot(hvx, hvy) > HEADING_MIN_SPEED_MS)
        w = (wrap180((Math.atan2(hvx, hvy) - Math.atan2(ax, ay)) / DEG) * DEG) / ((b - a) / 1000);
    }
    tr.om[i] = clamp(w, -max, max);
  }
}

/** A time series (measured attitude) takes a newer value; the last half minute is kept. */
function pushSeries(ts: number[], vs: number[], t: number, v: number): void {
  const n = ts.length;
  if (n && t <= ts[n - 1]) {
    if (t === ts[n - 1]) vs[n - 1] = v;
    return;
  }
  ts.push(t);
  vs.push(v);
  while (ts.length > 1 && ts[0] < t - MEAS_HISTORY_MS) {
    ts.shift();
    vs.shift();
  }
}

/** Slope (per ms) of a series at sample j for a monotone C1 cubic: none at a peak, a dip or an end, never overshooting. */
function seriesSlope(ts: number[], vs: number[], j: number): number {
  if (j <= 0 || j >= ts.length - 1) return 0;
  const d0 = (vs[j] - vs[j - 1]) / (ts[j] - ts[j - 1]);
  const d1 = (vs[j + 1] - vs[j]) / (ts[j + 1] - ts[j]);
  if (d0 * d1 <= 0) return 0;
  const m = (vs[j + 1] - vs[j - 1]) / (ts[j + 1] - ts[j - 1]);
  const max = 3 * Math.min(Math.abs(d0), Math.abs(d1));
  return Math.abs(m) > max ? Math.sign(m) * max : m;
}

/** A time series at T: a monotone C1 cubic between its values, held flat beyond its ends; `fallback` if empty. */
function seriesAt(ts: number[], vs: number[], T: number, fallback: number): number {
  const n = ts.length;
  if (n === 0) return fallback;
  if (T <= ts[0]) return vs[0];
  if (T >= ts[n - 1]) return vs[n - 1];
  let i = n - 2;
  while (i > 0 && T < ts[i]) i--;
  const h = ts[i + 1] - ts[i];
  const u = (T - ts[i]) / h;
  const m0 = seriesSlope(ts, vs, i) * h;
  const m1 = seriesSlope(ts, vs, i + 1) * h;
  const u2 = u * u;
  const u3 = u2 * u;
  return (2 * u3 - 3 * u2 + 1) * vs[i] + (u3 - 2 * u2 + u) * m0 + (3 * u2 - 2 * u3) * vs[i + 1] + (u3 - u2) * m1;
}

// --- Report states and the drawing delay ---------------------------------------------------

/** Keeps the latest report's state (replacing an earlier snapshot of the same report). */
function snapshot(tr: Track): void {
  const s = tr.states;
  while (s.length && s[s.length - 1].reportMs >= tr.reportMs) s.pop();
  s.push({
    reportMs: tr.reportMs,
    phase: tr.phase,
    trackDeg: tr.trackDeg,
    alt: tr.alt,
    vrate: tr.vrate,
    field: tr.field,
    fieldElev: tr.fieldElev,
    liftoffMs: tr.liftoffMs,
    touchdownMs: tr.touchdownMs,
    touchdownPitch: tr.touchdownPitch,
    capAlt: tr.capAlt,
  });
  if (s.length > MAX_STATES) s.shift();
}

/** The report state in effect at T: the newest one not after it (the oldest kept, before them all). */
function stateAt(tr: Track, T: number): State {
  const s = tr.states;
  let i = s.length - 1;
  while (i > 0 && s[i].reportMs > T) i--;
  return s[i];
}

/** The drawing delay at `nowMs`, ms: a C1 ease (cubic Hermite) from `dFrom` with slope `dSlope` to `dTo`. */
function delayAt(tr: Track, nowMs: number): number {
  if (!(tr.dDur > 0)) return tr.dTo;
  const u = (nowMs - tr.dMs) / tr.dDur;
  if (u >= 1) return tr.dTo;
  if (u <= 0) return tr.dFrom;
  const u2 = u * u;
  const u3 = u2 * u;
  return (2 * u3 - 3 * u2 + 1) * tr.dFrom + (u3 - 2 * u2 + u) * tr.dDur * tr.dSlope + (3 * u2 - 2 * u3) * tr.dTo;
}

/** The delay's rate of change at `nowMs` (ms per ms): the drawn time runs at 1 minus this. */
function delaySlope(tr: Track, nowMs: number): number {
  if (!(tr.dDur > 0)) return 0;
  const u = (nowMs - tr.dMs) / tr.dDur;
  if (u >= 1) return 0;
  if (u <= 0) return tr.dSlope;
  return ((6 * u * u - 6 * u) * (tr.dFrom - tr.dTo)) / tr.dDur + (3 * u * u - 4 * u + 1) * tr.dSlope;
}

/** The time a track is drawn at. */
const drawTime = (tr: Track, nowMs: number) => nowMs - delayAt(tr, nowMs);

/** Eases the delay towards `target` from where it is now, at most ~DELAY_RATE; `first` sets it outright. */
function retarget(tr: Track, nowMs: number, target: number, first: boolean): void {
  if (first) {
    tr.dFrom = tr.dTo = target;
    tr.dSlope = 0;
    tr.dDur = 0;
    tr.dMs = nowMs;
    return;
  }
  if (Math.abs(target - tr.dTo) < DELAY_DEADBAND_MS) return;
  const from = delayAt(tr, nowMs);
  tr.dSlope = delaySlope(tr, nowMs);
  tr.dFrom = from;
  tr.dTo = target;
  tr.dMs = nowMs;
  // long enough that the rate stays under DELAY_RATE (a cubic ease peaks at 1.5x its
  // average) and changes gently: |D''| ≤ (6|Δ|/L + 4|s|)/L ≤ DELAY_ACCEL
  const delta = Math.abs(target - from);
  const s = Math.abs(tr.dSlope);
  const j = DELAY_ACCEL / 1000; // per ms
  tr.dDur = Math.max(1_000, (1.5 * delta) / DELAY_RATE, (4 * s + Math.sqrt(16 * s * s + 24 * j * delta)) / (2 * j));
}

/**
 * Newer data arrived: adapt the delay. The newest sample is the latest report
 * (a buffer only holds the past), already `age` old on arrival. The drawn time
 * should still be a margin (one sample spacing) short of it when the next data
 * is due, one cadence from now, so the delay is
 *
 *     cadence + margin + age        (age = now - newest sample)
 *
 * within 2-12 s: right after an arrival the drawn time is a cadence and a
 * margin behind the newest data, and it closes up to the margin as the next
 * comes due. Flightradar24 alone, every ~8 s (the area refresh) with its
 * buffer 2 s apart and reports 0-5 s old, gives 10-15 s: 10-12 s, mostly the
 * cap, where the drawn time can reach the newest report just before the next
 * (and run up to ~1 s past it with 5 s old reports); every 15 s (the world
 * view), the cap. With adsb.lol for the selected aircraft (every 3 s,
 * ~0.5-1 s old, samples 1-3 s apart): ~5-6 s. The target is smoothed over
 * reports and the delay eases towards it.
 */
function noteArrival(tr: Track, nowMs: number): void {
  const n = tr.n;
  if (n === 0) return;
  const newest = tr.ts[n - 1];
  if (!(newest > tr.newestMs)) return;
  const first = tr.newestMs === -Infinity;
  tr.newestMs = newest;
  const dataMs = tr.reportMs;
  if (Number.isFinite(tr.arrivalDataMs) && dataMs > tr.arrivalDataMs) {
    const gap = clamp(dataMs - tr.arrivalDataMs, MIN_CADENCE_MS, MAX_CADENCE_MS);
    tr.cadenceMs += DELAY_SMOOTHING * (gap - tr.cadenceMs);
  }
  tr.arrivalDataMs = Math.max(tr.arrivalDataMs, dataMs);
  const k = Math.min(3, n - 1);
  const margin = clamp(k > 0 ? (newest - tr.ts[n - 1 - k]) / k : MAX_MARGIN_MS, MIN_MARGIN_MS, MAX_MARGIN_MS);
  const target = clamp(tr.cadenceMs + margin - (newest - nowMs), MIN_DELAY_MS, MAX_DELAY_MS);
  tr.delayTarget = first || !Number.isFinite(tr.delayTarget) ? target : tr.delayTarget + DELAY_SMOOTHING * (target - tr.delayTarget);
  retarget(tr, nowMs, tr.delayTarget, first);
}

/**
 * Average turn rate (deg/s) over this report's own samples (those from
 * `from`, its buffer's oldest point, on: the last ~10 s up to the report),
 * from chord headings in time order; NaN if they are too short to tell.
 */
function bufferTurnRate(tr: Track, from: number): number {
  const n = tr.n;
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

/**
 * Acceleration (kt/s) from the chord speeds of this report's own samples
 * (from `from` on, in time order: the later chord minus the earlier), NaN if
 * they span too little.
 */
function bufferAcceleration(tr: Track, from: number): number {
  let firstSpeed = NaN;
  let firstMid = 0;
  let lastSpeed = NaN;
  let lastMid = 0;
  for (let i = 0; i + 1 < tr.n; i++) {
    if (tr.ts[i] < from) continue;
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
    // the speed change between reports, else the estimate (on a first sighting, the buffer's:
    // the speed change over the history it holds)
    const accel = Number.isFinite(tr.reportAccel) ? tr.reportAccel : tr.accel;
    // a roll goes on as it began: a landing becomes a take-off (touch and go) only on clear
    // acceleration, a take-off a landing (rejected) on clear braking
    if (prev && FROM_LANDING.has(prev)) return accel > 2 * ROLL_ACCEL_KTS ? "takeoffRoll" : "landingRoll";
    if (prev && FROM_TAXI.has(prev)) return accel < -2 * ROLL_ACCEL_KTS ? "landingRoll" : "takeoffRoll";
    if (accel > ROLL_ACCEL_KTS) return "takeoffRoll";
    if (accel < -ROLL_ACCEL_KTS) return "landingRoll";
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
