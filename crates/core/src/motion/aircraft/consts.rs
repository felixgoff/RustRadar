//! Every constant of `motion.ts` (`:137-571`, doc 05 §10.2), with the aircraft class
//! (`aircraftClass`, `:569-580`) and the per-class and per-phase tables.
//!
//! Units: ms unless named. Values and derivations are those of the TypeScript, so the derived ones
//! (`FLARE_SPAN_FT`, `FLARE_A`, `MAX_SPEED_MS`, …) round exactly as there. `KNOTS_TO_MS`,
//! `FEET_TO_M` and `EARTH_RADIUS_M` are `core::geo`'s (one definition, the same values).

use std::sync::LazyLock;

use regex::Regex;

use super::Phase;

pub use crate::geo::{EARTH_RADIUS_M, FEET_TO_M, KNOTS_TO_MS};

pub const DEG: f64 = std::f64::consts::PI / 180.0;
pub const M_PER_DEG: f64 = EARTH_RADIUS_M * DEG;
pub const KM_PER_DEG: f64 = M_PER_DEG / 1000.0;
pub const FPM_TO_MS: f64 = FEET_TO_M / 60.0;
pub const GRAVITY_MS2: f64 = 9.80665;

// --- Samples -----------------------------------------------------------------

/// How much horizontal history a track keeps (and fits): the drawn time is never more than 12 s
/// behind the newest sample (which is never in the future), so older samples only steady the
/// fit's start.
pub const SAMPLE_WINDOW_MS: f64 = 30_000.0;
/// A report (or a point of its buffer) this far (m) off the smoothed path of the samples held is
/// a jump in the data: the old samples are dropped rather than smoothed into it.
pub const JUMP_M: f64 = 150.0;
/// ...checked for points inside those samples or up to this long (ms) past them.
pub const JUMP_LOOKAHEAD_MS: f64 = 4_000.0;
/// Samples closer together than this are duplicates (and would make the spline's tangents explode).
pub const MIN_SAMPLE_GAP_MS: f64 = 200.0;
/// How far before the first sample we run the track backwards (clock skew), seconds.
pub const BACKWARD_LIMIT_S: f64 = 30.0;
/// FR24 buffer offsets are in 1e-5 degrees, back from the report: a point is at `lat/lon - d * this`.
pub const BUFFER_SCALE: f64 = 1e-5;

// --- Estimators ----------------------------------------------------------------

/// Weight of a fresh estimate against the previous one ("lightly smoothed").
pub const SMOOTHING: f64 = 0.5;
/// Ground-speed change limit, kt/s: airliners manage ~3-4 kt/s on a takeoff roll.
pub const MAX_ACCEL_KTS: f64 = 4.0;
/// Turn rate limit, deg/s: a rate-one turn is 3 deg/s; anything faster is noise.
pub const MAX_TURN_DEG_S: f64 = 4.0;
pub const MAX_VRATE_FPM: f64 = 8_000.0;
/// Altitude comes in ~25 ft steps, so vertical rates need a few seconds between reports.
pub const MIN_VRATE_DT_S: f64 = 4.0;
pub const MAX_VRATE_DT_S: f64 = 300.0;
/// Speed differences between reports further apart than this say nothing about acceleration now.
pub const MIN_ACCEL_DT_S: f64 = 4.0;
pub const MAX_ACCEL_DT_S: f64 = 120.0;
/// Reported-track differences between reports further apart than this may hide whole turns.
pub const MIN_TURN_DT_S: f64 = 2.0;
pub const MAX_TURN_DT_S: f64 = 60.0;
/// Buffer chords shorter than this are too short for their heading to mean anything (positions
/// are quantised to ~1 m), so slow aircraft take their turn rate from reported tracks instead.
pub const MIN_TURN_CHORD_M: f64 = 100.0;
pub const MIN_SPEED_CHORD_M: f64 = 10.0;
/// Sanity cap on speeds read off the buffer.
pub const MAX_SPEED_MS: f64 = 1_200.0 * KNOTS_TO_MS;

// --- Extrapolation -------------------------------------------------------------

/// Keep moving this long past the last data, then hold.
pub const MAX_EXTRAPOLATION_S: f64 = 600.0;
/// A turn is continued this long past the data, then the path goes straight.
pub const TURN_HORIZON_S: f64 = 60.0;
/// Bank fades out over the last part of the turn horizon.
pub const BANK_FADE_START_S: f64 = 45.0;
/// Acceleration is applied this long past the data.
pub const ACCEL_HORIZON_S: f64 = 20.0;
/// Airborne speed changes are capped at this many knots either way.
pub const MAX_AIR_SPEED_CHANGE_KT: f64 = 30.0;
/// Taxi speed changes are gentler and stay below this.
pub const MAX_TAXI_ACCEL_KTS: f64 = 1.0;
pub const MAX_TAXI_SPEED_KT: f64 = 30.0;
/// Vertical rate is applied fully for this long past the report...
pub const VERTICAL_FADE_START_S: f64 = 60.0;
/// ...then fades linearly to level flight by this time.
pub const VERTICAL_HORIZON_S: f64 = 90.0;
pub const MAX_ALT_FT: f64 = 60_000.0;
/// Tightest turn radius we draw (1 / metres): taxiway corners.
pub const MAX_CURVATURE: f64 = 1.0 / 30.0;
/// Below this speed (m/s) a velocity's direction is noise; keep the last heading.
pub const HEADING_MIN_SPEED_MS: f64 = 1.0;

// --- Blending corrections ---------------------------------------------------------

/// Critically damped decay rate, 1/s: (1 + k t) e^{-k t} is ~1% after 3 s.
pub const BLEND_RATE: f64 = 2.2;
/// After k·t this large the correction is gone (< 0.01%): 6 s at the full rate.
pub const BLEND_END_KT: f64 = 13.2;
/// Small corrections (the smoothed curve moving by metres as a new sample comes in) are blended
/// out more slowly, so that their acceleration stays under this (m/s², a gentle turn's worth)...
pub const GENTLE_ACCEL: f64 = 3.0;
/// ...but no slower than this rate, 1/s (~1% after 13 s)...
pub const MIN_BLEND_RATE: f64 = 0.5;
/// ...and only below this size (m), back to the full rate by twice it (a real jump in the data is
/// resolved quickly).
pub const GENTLE_MAX_M: f64 = 40.0;
/// Larger corrections than this are a different aircraft position altogether: snap.
pub const SNAP_DISTANCE_M: f64 = 5_000.0;
/// If the previous data was older than this, its extrapolation means little: snap.
pub const SNAP_AGE_MS: f64 = 120_000.0;

// --- Measured data (selected aircraft) ----------------------------------------------

/// Measured values are used fully for this long after they were reported...
pub const MEAS_FRESH_MS: f64 = 15_000.0;
/// ...then eased back to the estimates over this long.
pub const MEAS_FADE_MS: f64 = 5_000.0;
/// Samples from the two sources closer in time than this are the same moment: the better one is kept.
pub const COINCIDENT_MS: f64 = 1_000.0;
/// The crab is kicked out over the last this many feet above the runway.
pub const DECRAB_AGL_FT: f64 = 60.0;
/// Largest crab angle believed, degrees.
pub const MAX_CRAB: f64 = 30.0;
/// A turn onto a selected heading rolls out over this long.
pub const ROLL_OUT_S: f64 = 4.0;
/// Altitude capture: the level-off is spread over this fraction of the vertical rate (ft per fpm)...
pub const CAPTURE_FT_PER_FPM: f64 = 0.15;
/// ...within these bounds, feet either side of the selected altitude.
pub const MIN_CAPTURE_FT: f64 = 100.0;
pub const MAX_CAPTURE_FT: f64 = 800.0;
/// Sample sources, also their rank when two coincide (higher wins).
pub const SRC_FR24: u8 = 1;
pub const SRC_ADSB: u8 = 2;
pub const SRC_MLAT: u8 = 0;

// --- Delayed, smoothed playback --------------------------------------------------------

/// The drawn time trails the server clock by this much at least and at most.
pub const MIN_DELAY_MS: f64 = 2_000.0;
pub const MAX_DELAY_MS: f64 = 12_000.0;
/// The delay changes at no more than about this rate (seconds per second): while it adjusts,
/// playback runs at 90-110% of real time, which nobody sees.
pub const DELAY_RATE: f64 = 0.1;
/// ...and its rate changes by at most this much per second, so the playback speeds up or slows
/// down by under 1% a second (2.5 m/s² at 250 m/s).
pub const DELAY_ACCEL: f64 = 0.01;
/// Delay targets this close to the current one are ignored, so the delay isn't forever on the move.
pub const DELAY_DEADBAND_MS: f64 = 500.0;
/// Weight of a new cadence or lead measurement in the delay target.
pub const DELAY_SMOOTHING: f64 = 0.3;
/// Data cadence assumed until two reports are seen (Flightradar24's refresh).
pub const DEFAULT_CADENCE_MS: f64 = 8_000.0;
pub const MIN_CADENCE_MS: f64 = 1_000.0;
pub const MAX_CADENCE_MS: f64 = 60_000.0;
/// The drawn time stays this far (one sample spacing, within these bounds) behind the newest sample.
pub const MIN_MARGIN_MS: f64 = 1_000.0;
pub const MAX_MARGIN_MS: f64 = 3_000.0;
/// Smoothing-spline stiffness for positions, s³ (per unit sample weight). The fitted curve
/// averages over about (λ × spacing)^¼ seconds either side: 2.5 s for Flightradar24's buffer (2 s
/// apart), 2.8 s for adsb.lol's 3 s polls. About three samples, that is, which takes out the
/// jitter with little lag.
pub const SMOOTH_LAMBDA: f64 = 20.0;
/// The same for altitude (25 ft steps, a report every 3-15 s): about ±5-7 s.
pub const ALT_SMOOTH_LAMBDA: f64 = 200.0;
/// Sample weights by source (inverse variance relative to ~10 m).
pub const WEIGHT_FR24: f64 = 1.0;
pub const WEIGHT_ADSB: f64 = 1.0;
pub const WEIGHT_MLAT: f64 = 0.04;
/// Weight of a reported velocity as the slope at the newest sample, when that sample is the report itself.
pub const REPORTED_SLOPE_WEIGHT: f64 = 10.0;
/// The first fitting pass's reference path turns (and changes speed) at the report's estimates
/// for this long before the newest sample, s...
pub const REF_FULL_S: f64 = 12.0;
/// ...fading to straight and steady by this long before it.
pub const REF_ZERO_S: f64 = 24.0;
/// Fitting passes: around the report's estimated turn and speed change, then twice around each
/// moment's own, read off the previous pass.
pub const REF_PASSES: usize = 3;
/// Those are each sample's heading and speed change over ±this (ms) on the previous pass's path...
pub const REF_LOCAL_MS: f64 = 4_000.0;
/// ...or as much of it as the samples cover, down to this (s).
pub const REF_MIN_CHORD_S: f64 = 2.0;
/// The drawn turn rate (for the bank) is the smoothed heading's change over ±this, ms.
pub const TURN_WINDOW_MS: f64 = 2_000.0;
/// The bank is read off the path this far ahead of the drawn time, ms (see `motion.ts:320-329`):
/// the map shows aircraft seconds in the past, so the wings roll in before the heading swings.
pub const BANK_LEAD_MS: f64 = 1_000.0;
/// Past the newest sample the turn rate eases from the path's into the prediction's over this long, s.
pub const TURN_HANDOVER_S: f64 = 3.0;
/// Past the newest altitude the vertical rate eases from the smoothed curve's into the estimate's
/// with this time constant, s.
pub const VRATE_HANDOVER_S: f64 = 3.0;
/// Altitude differences up to this (ft) are never a jump in the data, whatever the rate (quantisation).
pub const ALT_STEP_FT: f64 = 300.0;
/// Altitude history kept, ms.
pub const ALT_WINDOW_MS: f64 = 60_000.0;
/// Report states kept (each takes over when the drawn time reaches it).
pub const MAX_STATES: usize = 16;
/// Measured attitude history kept, ms.
pub const MEAS_HISTORY_MS: f64 = 30_000.0;
/// A pose further than this from the previous one (in time) is a fresh look: a state taking over
/// isn't blended.
pub const CONTINUITY_GAP_MS: f64 = 1_000.0;

// --- Pressure altitude ---------------------------------------------------------------
// (see `motion.ts:345-362`: each field's QNH is learned from the traffic itself)

pub const STD_HPA: f64 = 1013.25;
/// ISA: pressure altitude h (ft) at pressure p is H0 (1 - (p / 1013.25)^EXP).
pub const ISA_H0_FT: f64 = 145_442.16;
pub const ISA_EXP: f64 = 0.190263;
pub const MIN_QNH_HPA: f64 = 940.0;
pub const MAX_QNH_HPA: f64 = 1_060.0;
/// A field's learned QNH counts for this long, ms (it moves ~1 hPa an hour)...
pub const QNH_TTL_MS: f64 = 3.0 * 3_600_000.0;
/// ...and stands in for fields without one this far around, km.
pub const QNH_NEIGHBOUR_KM: f64 = 300.0;
/// Weight of a new runway reading in a field's QNH; a reading this far (hPa) off it counts less.
pub const QNH_SMOOTHING: f64 = 0.3;
pub const QNH_OUTLIER_HPA: f64 = 5.0;
pub const QNH_OUTLIER_SMOOTHING: f64 = 0.05;
/// The on-board altimeter setting is the local QNH below this height above the field, ft.
pub const MEAS_QNH_MAX_AGL_FT: f64 = 3_000.0;
/// Runways are read only this close to the field's reference point, km.
pub const RUNWAY_FIELD_KM: f64 = 8.0;
/// A runway's pressure altitude is this close to the field (ft), its QNH known or not.
pub const RUNWAY_TOL_KNOWN_FT: f64 = 150.0;
pub const RUNWAY_TOL_UNKNOWN_FT: f64 = 600.0;
/// One altitude step: reports this close are level.
pub const LEVEL_FT: f64 = 25.0;
/// Braking on the runway (kt/s), not slowing down on the approach.
pub const RUNWAY_DECEL_KTS: f64 = 1.0;
/// Older reports than this (s) don't tell a level run on the runway.
pub const LEVEL_MAX_DT_S: f64 = 20.0;

// --- Touchdown and lift-off ------------------------------------------------------------

/// The flare begins this high above the field, ft.
pub const FLARE_HEIGHT_FT: f64 = 50.0;
/// Touchdown (and lift-off) at this fraction of the approach's (climb's) vertical rate.
pub const TOUCHDOWN_SINK: f64 = 0.2;
/// The flare's length in raw height (ft): touchdown is where the data reaches FLARE_HEIGHT_FT - this.
pub const FLARE_SPAN_FT: f64 = (2.0 * FLARE_HEIGHT_FT) / (1.0 + TOUCHDOWN_SINK);
/// Raw height above the field (ft, negative) at which the flared height reaches the ground.
pub const FLARE_FLOOR_FT: f64 = FLARE_HEIGHT_FT - FLARE_SPAN_FT;
pub const FLARE_A: f64 = (1.0 - TOUCHDOWN_SINK) / (2.0 * FLARE_SPAN_FT);
/// A correction of a whole altitude reference (a QNH learned) is blended out at this rate, 1/s,
/// not snapped...
pub const REFERENCE_BLEND_RATE: f64 = 0.8;
/// ...or more slowly, so that it moves the drawn height by no more than this (ft/s, ~1800 fpm) at
/// its peak: a QNH learned on the runway can move an aircraft still drawn on short final by a few
/// hundred feet.
pub const REFERENCE_MAX_FT_S: f64 = 30.0;
/// Height left above the flare's floor at a predicted touchdown is eased out over this long
/// before it, ms.
pub const TOUCHDOWN_FIT_MS: f64 = 2_000.0;

// --- Airports ----------------------------------------------------------------------

/// Destination or origin within this range is the relevant field.
pub const FIELD_RANGE_KM: f64 = 40.0;
/// Otherwise the nearest airport within this range.
pub const NEAREST_FIELD_KM: f64 = 15.0;
/// Field elevation is removed fully within this range of the field...
pub const FADE_FULL_KM: f64 = 30.0;
/// ...fading to plain MSL here; a chosen field is also kept until this range.
pub const FADE_ZERO_KM: f64 = 60.0;
/// Airport grid cell size, degrees.
pub const GRID_DEG: f64 = 1.0;

// --- Phases ------------------------------------------------------------------------

pub const PARKED_MAX_KT: f64 = 3.0;
pub const TAXI_MAX_KT: f64 = 40.0;
/// Acceleration (kt/s) that decides between a takeoff and a landing roll.
pub const ROLL_ACCEL_KTS: f64 = 0.5;
pub const APPROACH_MAX_AGL_FT: f64 = 3_000.0;
pub const APPROACH_MAX_VRATE_FPM: f64 = -200.0;
pub const APPROACH_RANGE_KM: f64 = 25.0;
pub const FLARE_MAX_AGL_FT: f64 = 60.0;
pub const INITIAL_CLIMB_MIN_VRATE_FPM: f64 = 200.0;
pub const CLIMB_VRATE_FPM: f64 = 500.0;
/// Hysteresis: an established climb or descent ends below this rate.
pub const CLIMB_HOLD_VRATE_FPM: f64 = 250.0;

// --- Takeoff and landing prediction ---------------------------------------------------

pub const LANDING_DECEL_KTS: f64 = 2.0;
/// A predicted landing roll slows to this and then taxis on.
pub const ROLLOUT_KT: f64 = 20.0;
pub const TAKEOFF_ACCEL_KTS: f64 = 2.5;
/// After rotation the aircraft keeps accelerating up to this much past rotation speed.
pub const TAKEOFF_OVERSPEED_KT: f64 = 20.0;
pub const TAKEOFF_CLIMB_FPM: f64 = 1_800.0;
/// Rotation raises the nose over this long before lift-off...
pub const ROTATION_S: f64 = 3.0;
/// ...to this pitch, degrees.
pub const ROTATION_PITCH: f64 = 8.0;
/// After touchdown the nose comes down over this long.
pub const TOUCHDOWN_EASE_S: f64 = 4.0;

// --- Attitude ------------------------------------------------------------------------

pub const MIN_PITCH: f64 = -10.0;
pub const MAX_PITCH: f64 = 20.0;
pub const MAX_BANK: f64 = 30.0;

// --- Gear ----------------------------------------------------------------------------

pub const GEAR_TRANSIT_MS: f64 = 8_000.0;
pub const GEAR_APPROACH_AGL_FT: f64 = 2_000.0;
pub const GEAR_CLIMB_AGL_FT: f64 = 400.0;
/// Without a known field: low, slow and descending means gear down.
pub const GEAR_FALLBACK_ALT_FT: f64 = 1_500.0;
pub const GEAR_FALLBACK_KT: f64 = 180.0;

// --- Housekeeping ----------------------------------------------------------------------

pub const TRACK_TTL_MS: f64 = 15.0 * 60_000.0;
/// Newer reports taken in per frame at most (~10 µs each); new aircraft always are.
pub const INGEST_BUDGET: i32 = 600;
pub const PRUNE_INTERVAL_MS: f64 = 60_000.0;
/// Clock offsets: a sample this far from the current offset is a re-sync, not jitter.
pub const CLOCK_RESYNC_MS: f64 = 5_000.0;
pub const CLOCK_SMOOTHING: f64 = 0.1;
/// Server times before this (2001) are garbage.
pub const MIN_SANE_TIME_MS: f64 = 1e12;

// --- Taxiway snapping (`Motion.taxiSnap`) ------------------------------------------------
// A slow aircraft on the ground is drawn on the airport's taxiway (or runway) centreline when it
// is near one and moving along it.

/// Full snap within this distance of a line, m, fading out to none at `SNAP_FAR_M`.
pub const SNAP_NEAR_M: f64 = 15.0;
pub const SNAP_FAR_M: f64 = 30.0;
/// Full snap with the motion within this angle of the line (either way), fading out to none at
/// `SNAP_WIDE_DEG`.
pub const SNAP_ALIGNED_DEG: f64 = 20.0;
pub const SNAP_WIDE_DEG: f64 = 35.0;
/// Snapping fades out between these ground speeds, kt: never on a takeoff or landing roll at speed.
pub const SNAP_SLOW_KT: f64 = 25.0;
pub const SNAP_FAST_KT: f64 = 35.0;
/// The direction of motion this far ahead (drawn time) also counts: at a junction, the line it turns onto.
pub const SNAP_LOOKAHEAD_MS: f64 = 3_000.0;
/// Nearby lines' pulls are averaged with weights falling off e-fold over this distance, m: wide
/// enough to round a corner.
pub const SNAP_SOFT_M: f64 = 15.0;
/// A line's share of the pull goes by its alignment with the motion now, this much, and a moment
/// ahead, the rest.
pub const SNAP_NOW_SHARE: f64 = 0.7;
/// The direction ahead follows the model's with this time constant, ms: new reports move it.
pub const SNAP_AHEAD_LAG_MS: f64 = 500.0;
/// Snapping turns on or off (as the gate opens or closes) at this rate, /s.
pub const SNAP_GATE_RATE: f64 = 0.8;
/// The offset follows its target through a critically damped spring of this rate, /s.
pub const SNAP_SPRING_RATE: f64 = 5.0;
/// Below this speed (m/s) the direction of motion is the drawn track.
pub const SNAP_MIN_SPEED_MS: f64 = 0.5;

/// `COS_SNAP_ALIGNED = Math.cos(SNAP_ALIGNED_DEG * (Math.PI / 180))` (a function: `cos` is not
/// `const` in Rust).
pub fn cos_snap_aligned() -> f64 {
    (SNAP_ALIGNED_DEG * (std::f64::consts::PI / 180.0)).cos()
}

/// `COS_SNAP_WIDE = Math.cos(SNAP_WIDE_DEG * (Math.PI / 180))`.
pub fn cos_snap_wide() -> f64 {
    (SNAP_WIDE_DEG * (std::f64::consts::PI / 180.0)).cos()
}

// --- Per phase and per class ---------------------------------------------------------

/// `PITCH_OFFSET`: pitch above the flight-path angle, degrees: roughly the angle of attack per phase.
pub fn pitch_offset(phase: Phase) -> f64 {
    match phase {
        Phase::Parked | Phase::Taxi | Phase::TakeoffRoll | Phase::LandingRoll => 0.0,
        Phase::InitialClimb => 6.0,
        Phase::Climb => 3.0,
        Phase::Cruise => 2.0,
        Phase::Descent => 1.0,
        Phase::Approach => 3.0,
        Phase::Flare => 5.0,
    }
}

/// What an aircraft is, for the speeds it flies at and whether it rolls (`AircraftClass`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AircraftClass {
    #[default]
    Jet,
    Turboprop,
    Light,
    Heli,
}

impl AircraftClass {
    pub fn as_str(self) -> &'static str {
        match self {
            AircraftClass::Jet => "jet",
            AircraftClass::Turboprop => "turboprop",
            AircraftClass::Light => "light",
            AircraftClass::Heli => "heli",
        }
    }
}

/// `MIN_FLYING_KT[cls]`: below these ground speeds (kt) nothing flies, whatever the air/ground flag
/// says. The table has no helicopter (callers return before asking); NaN stands for its
/// `undefined`, so that every comparison with it is false as there.
pub fn min_flying_kt(cls: AircraftClass) -> f64 {
    match cls {
        AircraftClass::Jet => 60.0,
        AircraftClass::Turboprop => 50.0,
        AircraftClass::Light => 30.0,
        AircraftClass::Heli => f64::NAN,
    }
}

/// `ROTATION_KT[cls]`: rotation speeds, kt, by aircraft class (helicopters don't roll: NaN, as
/// for [`min_flying_kt`]).
pub fn rotation_kt(cls: AircraftClass) -> f64 {
    match cls {
        AircraftClass::Jet => 145.0,
        AircraftClass::Turboprop => 110.0,
        AircraftClass::Light => 60.0,
        AircraftClass::Heli => f64::NAN,
    }
}

// JavaScript's `\d` is ASCII, so the patterns spell out `[0-9]`; `$` is the end of the text.
static HELI_TYPES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "^(EC[0-9]|H[0-9][0-9]|AS3|AS5|AS6|R22|R44|R66|B06|B47|B407|B412|B429|A109|A119|A139|A169|A189|AW1|S76|S92|MI8|UH|H60|BK17|EH10|NH90|EXPL|CH47|GAZL|ALO)",
    )
    .expect("helicopter types")
});
static TURBOPROP_TYPES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "^(AT4|AT7|DH8|DHC|SF34|E120|JS3|JS4|B190|C208|PC12|PC6|D228|D328|SW4|F50|BE20|BE9|BE30|AN2[46]|AN3|L410|C130|A400|P180|TBM|SB20|Y12|C212|CN35|C295|BN2)",
    )
    .expect("turboprop types")
});
static LIGHT_TYPES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "^(C1[0-9][0-9]|C2[0-9]|PA[0-9]|P28|SR2|DA[24]|BE3|BE[0-9]{2}$|M20|RV|GLID|AS2|ASW|DG|LS[0-9]|ULAC|TOBA|AT3|C42|P2002)",
    )
    .expect("light types")
});

/// The class of an aircraft from Flightradar24's icon and its type code (upper-cased, matched at
/// the start); helicopter, then turboprop, then light, else jet: the order matters.
pub fn aircraft_class(icon: &str, typecode: &str) -> AircraftClass {
    let code = typecode.to_uppercase();
    if icon == "EC" || HELI_TYPES.is_match(&code) {
        return AircraftClass::Heli;
    }
    if icon == "Q300" || TURBOPROP_TYPES.is_match(&code) {
        return AircraftClass::Turboprop;
    }
    if matches!(icon, "C206" | "C303" | "GLID" | "AS20" | "ASW20") || LIGHT_TYPES.is_match(&code) {
        return AircraftClass::Light;
    }
    AircraftClass::Jet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_constants_match_the_typescript() {
        // the values bun prints for motion.ts's expressions
        assert_eq!(FLARE_SPAN_FT, 83.33333333333334);
        assert_eq!(FLARE_FLOOR_FT, -33.33333333333334);
        assert_eq!(FLARE_A, 0.0048);
        assert_eq!(MAX_SPEED_MS, 617.3328);
        assert_eq!(M_PER_DEG, 111194.92664455874);
        assert_eq!(KM_PER_DEG, 111.19492664455875);
        assert_eq!(FPM_TO_MS, 0.00508);
        assert_eq!(QNH_TTL_MS, 10_800_000.0);
        assert_eq!(TRACK_TTL_MS, 900_000.0);
        assert_eq!(MAX_CURVATURE, 1.0 / 30.0);
        assert_eq!(cos_snap_aligned(), 0.9396926207859084);
        assert_eq!(cos_snap_wide(), 0.8191520442889918);
        // sources rank MLAT < Flightradar24 < ADS-B
        const { assert!(SRC_MLAT < SRC_FR24 && SRC_FR24 < SRC_ADSB) };
    }

    #[test]
    fn pitch_offsets_by_phase() {
        assert_eq!(pitch_offset(Phase::Parked), 0.0);
        assert_eq!(pitch_offset(Phase::LandingRoll), 0.0);
        assert_eq!(pitch_offset(Phase::InitialClimb), 6.0);
        assert_eq!(pitch_offset(Phase::Climb), 3.0);
        assert_eq!(pitch_offset(Phase::Cruise), 2.0);
        assert_eq!(pitch_offset(Phase::Descent), 1.0);
        assert_eq!(pitch_offset(Phase::Approach), 3.0);
        assert_eq!(pitch_offset(Phase::Flare), 5.0);
    }

    #[test]
    fn class_tables_have_no_helicopter() {
        assert_eq!(min_flying_kt(AircraftClass::Jet), 60.0);
        assert_eq!(min_flying_kt(AircraftClass::Light), 30.0);
        assert_eq!(rotation_kt(AircraftClass::Turboprop), 110.0);
        // `undefined` in the TypeScript: no comparison holds
        assert!(!(20.0 < min_flying_kt(AircraftClass::Heli)));
        assert!(!(20.0 >= rotation_kt(AircraftClass::Heli)));
    }

    #[test]
    fn aircraft_classes_by_icon_and_type_code() {
        use AircraftClass::*;
        assert_eq!(aircraft_class("A320", "A320"), Jet);
        assert_eq!(aircraft_class("B738", "b738"), Jet);
        assert_eq!(aircraft_class("", ""), Jet);
        // icons decide on their own
        assert_eq!(aircraft_class("EC", "A320"), Heli);
        assert_eq!(aircraft_class("Q300", "A320"), Turboprop);
        assert_eq!(aircraft_class("ASW20", ""), Light);
        // type codes, upper-cased and anchored at the start
        assert_eq!(aircraft_class("", "ec35"), Heli);
        assert_eq!(aircraft_class("", "H125"), Heli);
        assert_eq!(aircraft_class("", "AT76"), Turboprop);
        assert_eq!(aircraft_class("", "DH8D"), Turboprop);
        assert_eq!(aircraft_class("", "AN24"), Turboprop);
        assert_eq!(aircraft_class("", "AN22"), Jet);
        assert_eq!(aircraft_class("", "C172"), Light);
        assert_eq!(aircraft_class("", "PA28"), Light);
        assert_eq!(aircraft_class("", "XC172"), Jet);
        // `BE[0-9]{2}$` is a whole code: BE36 is light (BE3), BE400 isn't; BE20 and BE300 are
        // turboprops first
        assert_eq!(aircraft_class("", "BE36"), Light);
        assert_eq!(aircraft_class("", "BE20"), Turboprop);
        assert_eq!(aircraft_class("", "BE300"), Turboprop);
        assert_eq!(aircraft_class("", "BE400"), Jet);
        // order: a helicopter pattern wins over a light one (AS3 vs AS2), turboprop over light
        assert_eq!(aircraft_class("", "AS350"), Heli);
        assert_eq!(aircraft_class("", "AS20"), Light);
        assert_eq!(aircraft_class("C206", "AT45"), Turboprop);
        // JavaScript's \d is ASCII only
        assert_eq!(aircraft_class("", "EC٣"), Jet);
    }
}
