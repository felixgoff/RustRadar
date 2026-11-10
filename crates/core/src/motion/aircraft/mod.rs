//! Port of `motion.ts` (S1.18 to S1.20; doc 05 §10): where to draw each aircraft between feed
//! reports, and how.
//!
//! Flightradar24 sends a report every 8-15 s with a buffer of the aircraft's positions over the
//! ~10 s before it. Between reports the model interpolates inside the known samples (a cubic
//! smoothing spline, evaluated as Hermite pieces), extrapolates along an arc past them, predicts
//! touchdowns and lift-offs, derives pitch, bank and gear, and blends every correction in over a
//! few seconds so that nothing ever jumps or freezes. By design each aircraft is drawn 2-12 s in
//! the past (the delay), so that it is almost always between known positions.
//!
//! # Layout
//!
//! | file | holds | item |
//! |---|---|---|
//! | `mod.rs` | [`Measured`], [`Phase`], [`Pose`], [`Motion`] and its [`World`]; clock, airports, taxiway networks, `prune`, `draw_altitude`, field selection; the small functions of doc 05 §10.3 | S1.18 |
//! | [`consts`] | every constant, the aircraft class, the per-phase and per-class tables | S1.18 |
//! | [`track`] | [`Track`], [`State`](track::State), [`Raw`](track::Raw), [`Blend`](track::Blend), the speed profile | S1.18 |
//! | [`samples`] | report points, merging (with jump detection), measured samples, the local frame, buffer estimators | S1.18 |
//! | [`spline`] | the cubic smoothing spline | S1.18 |
//! | [`fit`] | the horizontal fit around a reference path, Hermite evaluation, turn nodes | S1.18 |
//! | [`altitude`] | pressure altitude and its QNH correction, the flare curve, vertical weights, altitude samples and their fit, field removal | S1.18 |
//! | `delay`, `phase`, `qnh`, `predict`, `evaluate`, `blend` | doc 05 §10.8-10.11 | S1.19 |
//! | `measured`, `taxi_snap`, `gear` | doc 05 §10.12-10.13 | S1.20 |
//!
//! The TypeScript's `Motion` methods that read only per-track data are free functions taking
//! `&Track` / `&mut Track` here, and those that also read the model's shared tables (airports,
//! learned QNHs, taxiway networks) are methods of [`World`]. [`Motion`] holds the tracks and the
//! world side by side, so that a method can borrow one track mutably together with the world.
//!
//! # Conventions
//!
//! - Times are `f64` ms (server-synced, as `motion.now()`), with the TypeScript's sentinels:
//!   `-inf` for "never", NaN for "unknown"; every comparison is written as there, negations
//!   included (`!(x > 0)` is true for NaN). The public API takes `f64` ms too: the TypeScript's
//!   callers pass fractional times (the drawn time is a cubic ease), and so do its tests.
//! - Nothing reads a clock: [`Motion::now`] and [`Motion::sync_clock`] take the local clock.
//! - JavaScript built-ins whose semantics differ go through `core::num` (`wrap180`, `wrap360`,
//!   `wrap_lon`, `smoothstep`, `sign0`) and `js_min`/`js_max` (NaN-propagating); [`clamp`] is
//!   the TypeScript's own `clamp`, which keeps NaN and never panics.
//! - `LiveFlight`'s `f32` latitude and longitude are read with `model::f32_as_js`, as the
//!   TypeScript read the JSON's shortest decimal.
//! - The module-level scratch of the TypeScript (`hx`, `profS`, `springV`, `reportFromMs`, …)
//!   becomes return values, or buffers in [`Scratch`] owned by the [`World`] (no allocation per
//!   report once they have grown).
//! - Airports are `Arc<Airport>` compared by identity ([`same_airport`], [`AirportKey`]), as the
//!   TypeScript compared `Airport` objects; taxiway networks are `Arc<TaxiNetwork>` compared with
//!   `Arc::ptr_eq`, and kept in insertion order (the first that covers a point wins).

// The TypeScript's negated comparisons (`!(x > 0)`, true for NaN) are kept as written (doc 05 §14).
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod altitude;
pub mod blend;
pub mod consts;
pub mod delay;
pub mod evaluate;
pub mod fit;
pub mod gear;
pub mod measured;
pub mod phase;
pub mod predict;
pub mod qnh;
pub mod samples;
pub mod spline;
pub mod taxi_snap;
pub mod track;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use indexmap::IndexMap;

pub use crate::num::{sign0, smoothstep, wrap_lon, wrap180, wrap360};
use crate::model::{Airport, LiveFlight, TaxiLine};
use crate::taxinet::TaxiNetwork;
use crate::trains::{js_max, js_min};
use altitude::{corrected, field_removal};
use consts::*;
use track::Track;

/// Measured state of one aircraft from a second source (adsb.lol's readsb fields). Times are Unix
/// ms; absent values are `None` (left out in the TypeScript).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Measured {
    /// When `lat`/`lon` were measured.
    pub position_ms: Option<f64>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// The position is multilaterated: less precise than Flightradar24's.
    pub mlat: bool,
    /// When the other values were last reported.
    pub seen_ms: f64,
    pub on_ground: bool,
    /// Barometric (pressure) altitude, feet: referenced to 1013.25 hPa, like Flightradar24's.
    pub alt_baro: Option<f64>,
    /// The altimeter setting on board, hPa: below the transition altitude, the local QNH.
    pub nav_qnh: Option<f64>,
    /// Ground speed, kt; track over the ground and nose heading, degrees true.
    pub gs: Option<f64>,
    pub track: Option<f64>,
    pub true_heading: Option<f64>,
    /// Nose heading, degrees magnetic: with `true_heading`, gives the variation for `nav_heading`.
    pub mag_heading: Option<f64>,
    /// Bank, degrees, right wing down positive.
    pub roll: Option<f64>,
    /// Degrees per second, clockwise positive.
    pub track_rate: Option<f64>,
    /// Feet per minute.
    pub baro_rate: Option<f64>,
    pub geom_rate: Option<f64>,
    /// True airspeed, kt.
    pub tas: Option<f64>,
    /// Altitudes selected on the autopilot (MCP/FCU) and by the FMS, feet.
    pub nav_altitude_mcp: Option<f64>,
    pub nav_altitude_fms: Option<f64>,
    /// Heading selected on the autopilot, degrees (magnetic, in practice).
    pub nav_heading: Option<f64>,
    pub nav_modes: Option<Vec<String>>,
}

/// Flight phase, as drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    Parked,
    Taxi,
    TakeoffRoll,
    InitialClimb,
    Climb,
    Cruise,
    Descent,
    Approach,
    Flare,
    LandingRoll,
}

impl Phase {
    /// The TypeScript's string for the phase (`"takeoffRoll"`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Parked => "parked",
            Phase::Taxi => "taxi",
            Phase::TakeoffRoll => "takeoffRoll",
            Phase::InitialClimb => "initialClimb",
            Phase::Climb => "climb",
            Phase::Cruise => "cruise",
            Phase::Descent => "descent",
            Phase::Approach => "approach",
            Phase::Flare => "flare",
            Phase::LandingRoll => "landingRoll",
        }
    }

    /// The phase a TypeScript string names.
    pub fn from_name(name: &str) -> Option<Phase> {
        Some(match name {
            "parked" => Phase::Parked,
            "taxi" => Phase::Taxi,
            "takeoffRoll" => Phase::TakeoffRoll,
            "initialClimb" => Phase::InitialClimb,
            "climb" => Phase::Climb,
            "cruise" => Phase::Cruise,
            "descent" => Phase::Descent,
            "approach" => Phase::Approach,
            "flare" => Phase::Flare,
            "landingRoll" => Phase::LandingRoll,
            _ => return None,
        })
    }

    /// `GROUND`: parked, taxi, takeoff roll, landing roll.
    pub fn is_ground(self) -> bool {
        matches!(self, Phase::Parked | Phase::Taxi | Phase::TakeoffRoll | Phase::LandingRoll)
    }

    /// `FROM_TAXI`: parked, taxi, takeoff roll.
    pub fn from_taxi(self) -> bool {
        matches!(self, Phase::Parked | Phase::Taxi | Phase::TakeoffRoll)
    }

    /// `FROM_LANDING`: approach, flare, landing roll, descent.
    pub fn from_landing(self) -> bool {
        matches!(self, Phase::Approach | Phase::Flare | Phase::LandingRoll | Phase::Descent)
    }
}

/// Where and how to draw an aircraft.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub lon: f64,
    pub lat: f64,
    /// Height to draw at, feet: altitude with the relevant airport's field elevation removed near
    /// airports (faded back to plain reported altitude by ~30-60 km out), exactly 0 on the ground.
    /// Near a field the reported pressure altitude is corrected with its QNH, and the last feet
    /// above the runway are shaped into a flare.
    pub alt_ft: f64,
    /// Degrees clockwise from north: where the nose points. Without measured data this is the
    /// direction of motion (`track`); with a measured true heading it differs from it by the crab
    /// angle into a crosswind.
    pub heading: f64,
    /// Degrees clockwise from north: direction of motion along the drawn path (reported track
    /// when parked).
    pub track: f64,
    /// Degrees, nose up positive.
    pub pitch: f64,
    /// Degrees, right wing down positive (a right turn banks right).
    pub bank: f64,
    /// 0 = retracted .. 1 = fully down, animated.
    pub gear: f64,
    pub phase: Phase,
}

/// An airport as a map key by identity (the TypeScript's `Map<Airport, …>`): two keys are equal
/// when they hold the same `Arc`. The key keeps its airport alive, so an address is never reused
/// while it is in a map.
#[derive(Debug, Clone)]
pub struct AirportKey(pub Arc<Airport>);

impl PartialEq for AirportKey {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for AirportKey {}

impl Hash for AirportKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

/// `a === b` for the TypeScript's `Airport | null`: the same airport object, or both none.
pub fn same_airport(a: Option<&Arc<Airport>>, b: Option<&Arc<Airport>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// A field's QNH as learned from the traffic (hPa) and when (`qnhs` values).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QnhReading {
    pub hpa: f64,
    pub ms: f64,
}

/// A field's QNH interpolated from those around, cached until anything is learned (`version`)
/// or a minute passes (`qnhAround` values).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QnhAround {
    pub hpa: f64,
    pub ms: f64,
    pub version: u64,
}

/// Buffers the TypeScript kept at module level, reused report after report.
#[derive(Debug, Default)]
pub struct Scratch {
    pub samples: samples::Scratch,
    pub fit: fit::Scratch,
}

/// What every track shares: the clock, the frame's ingest budget, the airports, the QNHs learned
/// from the traffic, the taxiway networks and scratch space. (The TypeScript's `Motion` fields
/// other than `tracks`.)
#[derive(Debug)]
pub struct World {
    pub clock_offset: f64,
    pub clock_synced: bool,
    pub last_prune: f64,
    /// Reports still to be taken in this frame (`budget_ms`).
    pub budget: i32,
    pub budget_ms: f64,
    pub by_iata: HashMap<String, Arc<Airport>>,
    /// Airports by 1-degree cell ([`World::cell`]), in the order given.
    pub grid: HashMap<i64, Vec<Arc<Airport>>>,
    /// Each field's QNH as learned from the traffic (hPa) and when, in the order first learned.
    pub qnhs: IndexMap<AirportKey, QnhReading>,
    /// Fields' QNHs interpolated from those around, until anything is learned (`qnh_version`) or
    /// a minute passes.
    pub qnh_around: HashMap<AirportKey, QnhAround>,
    pub qnh_version: u64,
    /// Taxiway networks by airport (ICAO code), as loaded: insertion order, as a JavaScript `Map`
    /// (remove with `shift_remove`, which keeps the others' order).
    pub taxiways: IndexMap<String, Arc<TaxiNetwork>>,
    pub scratch: Scratch,
}

impl Default for World {
    fn default() -> Self {
        World {
            clock_offset: 0.0,
            clock_synced: false,
            last_prune: f64::NEG_INFINITY,
            budget: 0,
            budget_ms: f64::NAN,
            by_iata: HashMap::new(),
            grid: HashMap::new(),
            qnhs: IndexMap::new(),
            qnh_around: HashMap::new(),
            qnh_version: 0,
            taxiways: IndexMap::new(),
            scratch: Scratch::default(),
        }
    }
}

impl World {
    /// The airport grid's key for a 1-degree cell (longitude cells wrap).
    pub fn cell(lat_cell: f64, lon_cell: f64) -> i64 {
        let lon_wrapped = ((lon_cell % 360.0) + 360.0) % 360.0;
        ((lat_cell + 90.0) * 360.0 + lon_wrapped) as i64
    }

    /// Nearest airport within `max_km` (small ranges only: searches the 3×3 grid cells around).
    pub fn nearest_airport(&self, lat: f64, lon: f64, max_km: f64) -> Option<Arc<Airport>> {
        let lat_cell = (lat / GRID_DEG).floor();
        let lon_cell = (lon / GRID_DEG).floor();
        let mut best: Option<&Arc<Airport>> = None;
        let mut best_km = max_km;
        for i in -1..=1 {
            for j in -1..=1 {
                let Some(list) = self.grid.get(&Self::cell(lat_cell + i as f64, lon_cell + j as f64)) else {
                    continue;
                };
                for ap in list {
                    let km = distance_km(lat, lon, ap.lat, ap.lon);
                    if km < best_km {
                        best_km = km;
                        best = Some(ap);
                    }
                }
            }
        }
        best.cloned()
    }

    /// `selectField`: destination within 40 km, else origin within 40 km, else the nearest within
    /// 15 km, else the previous one while within 60 km. A report (`f`) also sets the track's
    /// origin and destination airports.
    pub fn select_field(&self, tr: &mut Track, f: Option<&LiveFlight>) {
        if let Some(f) = f {
            let lookup = |code: &str| if code.is_empty() { None } else { self.by_iata.get(code).cloned() };
            tr.origin_ap = lookup(&f.origin);
            tr.dest_ap = lookup(&f.destination);
        }
        let lat = tr.lat0;
        let lon = tr.lon0;
        let within = |ap: &Option<Arc<Airport>>, range: f64| {
            ap.as_ref().filter(|ap| distance_km(lat, lon, ap.lat, ap.lon) <= range).cloned()
        };
        let field = within(&tr.dest_ap, FIELD_RANGE_KM)
            .or_else(|| within(&tr.origin_ap, FIELD_RANGE_KM))
            .or_else(|| self.nearest_airport(lat, lon, NEAREST_FIELD_KM))
            .or_else(|| within(&tr.field, FADE_ZERO_KM));
        tr.field_km = match &field {
            Some(ap) => distance_km(lat, lon, ap.lat, ap.lon),
            None => f64::INFINITY,
        };
        tr.field_elev = match &field {
            Some(ap) => ap.alt.unwrap_or(0) as f64,
            None => 0.0,
        };
        tr.field = field;
    }
}

/// The aircraft motion model: one per app, fed every snapshot and asked for every visible
/// aircraft's pose every frame.
#[derive(Debug, Default)]
pub struct Motion {
    tracks: HashMap<u32, Track>,
    world: World,
}

impl Motion {
    pub fn new() -> Self {
        Self::default()
    }

    /// Server-synced clock, ms, given the local clock: use this everywhere instead of the local
    /// time.
    pub fn now(&self, local_now_ms: f64) -> f64 {
        local_now_ms + self.world.clock_offset
    }

    /// Called with each snapshot's `server_time_ms` on arrival (and the local clock then): keeps a
    /// smoothed offset to the local clock, ignoring absurd values. The first sample, or a real
    /// clock change, is taken as is; otherwise network jitter is smoothed out.
    pub fn sync_clock(&mut self, server_time_ms: f64, local_now_ms: f64) {
        if !server_time_ms.is_finite() || server_time_ms < MIN_SANE_TIME_MS {
            return;
        }
        let w = &mut self.world;
        let sample = server_time_ms - local_now_ms;
        if !w.clock_synced || (sample - w.clock_offset).abs() > CLOCK_RESYNC_MS {
            w.clock_offset = sample;
        } else {
            w.clock_offset += CLOCK_SMOOTHING * (sample - w.clock_offset);
        }
        w.clock_synced = true;
    }

    /// The airports fields are chosen from (by IATA code for origins and destinations, by grid
    /// cell for the nearest). Airports without a finite position are left out.
    pub fn set_airports(&mut self, airports: &[Arc<Airport>]) {
        let w = &mut self.world;
        w.by_iata.clear();
        w.grid.clear();
        for ap in airports {
            if !ap.lat.is_finite() || !ap.lon.is_finite() {
                continue;
            }
            if !ap.iata.is_empty() {
                w.by_iata.insert(ap.iata.clone(), Arc::clone(ap));
            }
            let key = World::cell((ap.lat / GRID_DEG).floor(), (ap.lon / GRID_DEG).floor());
            w.grid.entry(key).or_default().push(Arc::clone(ap));
        }
    }

    /// An airport's taxiway, taxilane and runway centrelines (OpenStreetMap), by its ICAO code:
    /// slow aircraft on the ground near them are drawn on them. `None` (or no lines) forgets the
    /// airport's. Tracks drawn on the old network let go of it.
    pub fn set_taxiways(&mut self, icao: &str, lines: Option<&[TaxiLine]>) {
        let taxiways = &mut self.world.taxiways;
        let old = taxiways.get(icao).cloned();
        match lines {
            // a JavaScript Map keeps a replaced key where it was, as IndexMap::insert does
            Some(lines) if !lines.is_empty() => {
                taxiways.insert(icao.to_string(), Arc::new(TaxiNetwork::new(lines)));
            }
            _ => {
                taxiways.shift_remove(icao);
            }
        }
        if let Some(old) = old {
            for tr in self.tracks.values_mut() {
                if tr.snap_net.as_ref().is_some_and(|net| Arc::ptr_eq(net, &old)) {
                    tr.snap_net = None;
                }
            }
        }
    }

    pub fn has_taxiways(&self, icao: &str) -> bool {
        self.world.taxiways.contains_key(icao)
    }

    /// The airport an aircraft on the ground is at (its relevant field), or none when it is
    /// airborne or unknown.
    pub fn ground_field(&self, id: u32) -> Option<&Arc<Airport>> {
        let tr = self.tracks.get(&id)?;
        if tr.phase.is_ground() { tr.field.as_ref() } else { None }
    }

    /// Same airport-relative drawing height for points of the aircraft's trail (so the trail
    /// meets the model): `alt_ft` is a reported (pressure) altitude, corrected with the
    /// aircraft's QNH near its field like its own.
    pub fn draw_altitude(&self, id: u32, alt_ft: f64, lat: f64, lon: f64) -> f64 {
        if !(alt_ft > 0.0) {
            return 0.0;
        }
        let Some(tr) = self.tracks.get(&id) else {
            return alt_ft;
        };
        let w = match &tr.field {
            Some(field) => field_fade(distance_km(lat, lon, field.lat, field.lon)),
            None => 0.0,
        };
        js_max(0.0, corrected(tr, alt_ft, w) - field_removal(tr, lat, lon))
    }

    /// Drops tracks not touched for 15 min.
    pub fn prune(&mut self, now_ms: f64) {
        self.world.last_prune = now_ms;
        self.tracks.retain(|_, tr| !(now_ms - tr.touched > TRACK_TTL_MS));
    }
}

// --- Small functions (doc 05 §10.3) ------------------------------------------------------

/// The TypeScript's `clamp`: `lo` below it, `hi` above it, else `x` (NaN stays NaN). Unlike
/// `f64::clamp` it never panics, whatever the bounds.
pub fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// Equirectangular distance, km: exact enough for the < 100 km ranges it is used for.
pub fn distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let x = wrap180(lon2 - lon1) * (((lat1 + lat2) / 2.0) * DEG).cos();
    x.hypot(lat2 - lat1) * KM_PER_DEG
}

/// Field elevation removal factor at `km` from the field.
pub fn field_fade(km: f64) -> f64 {
    clamp((FADE_ZERO_KM - km) / (FADE_ZERO_KM - FADE_FULL_KM), 0.0, 1.0)
}

/// Weight of a track's measured values at time `t`: 1 while fresh, easing to 0 (the estimates)
/// once stale.
pub fn meas_weight(tr: &Track, t: f64) -> f64 {
    if tr.meas_ms == f64::NEG_INFINITY {
        return 0.0;
    }
    1.0 - smoothstep((t - tr.meas_ms - MEAS_FRESH_MS) / MEAS_FADE_MS)
}

/// Whether measured values reported at `meas_ms` still count at time `t`.
pub fn meas_fresh(tr: &Track, t: f64) -> bool {
    tr.meas.is_some() && t - tr.meas_ms <= MEAS_FRESH_MS
}

/// Pitch for flight at a vertical and ground speed in a phase; `agl` (ft) shapes the flare.
pub fn air_pitch(vs_fpm: f64, speed_ms: f64, phase: Phase, agl: f64) -> f64 {
    let gamma = (vs_fpm * FPM_TO_MS).atan2(js_max(speed_ms, 10.0)) / DEG;
    let mut offset = pitch_offset(phase);
    // ease from the approach attitude into the flare's as the ground comes up
    if matches!(phase, Phase::Approach | Phase::Flare) && agl < FLARE_MAX_AGL_FT {
        let (approach, flare) = (pitch_offset(Phase::Approach), pitch_offset(Phase::Flare));
        offset = approach + (flare - approach) * (1.0 - js_max(0.0, agl) / FLARE_MAX_AGL_FT);
    }
    clamp(gamma + offset, MIN_PITCH, MAX_PITCH)
}

/// A value at `x0` moving at `v0` /s, `dt` s later, following `target` through a critically
/// damped spring at `SNAP_SPRING_RATE`: the value and its new rate (the TypeScript's `springV`).
pub fn spring(x0: f64, v0: f64, target: f64, dt: f64) -> (f64, f64) {
    let k = SNAP_SPRING_RATE;
    let decay = (-k * dt).exp();
    let d = x0 - target;
    let b = v0 + k * d;
    let v = (v0 - k * b * dt) * decay;
    (target + (d + b * dt) * decay, v)
}

/// A time series (measured attitude) takes a newer value; the last half minute is kept.
pub fn push_series(ts: &mut Vec<f64>, vs: &mut Vec<f64>, t: f64, v: f64) {
    if let Some(&last) = ts.last()
        && t <= last
    {
        if t == last {
            *vs.last_mut().expect("series of equal length") = v;
        }
        return;
    }
    ts.push(t);
    vs.push(v);
    let mut drop = 0;
    while ts.len() - drop > 1 && ts[drop] < t - MEAS_HISTORY_MS {
        drop += 1;
    }
    ts.drain(..drop);
    vs.drain(..drop);
}

/// Slope (per ms) of a series at sample `j` for a monotone C1 cubic: none at a peak, a dip or an
/// end, never overshooting.
pub fn series_slope(ts: &[f64], vs: &[f64], j: usize) -> f64 {
    if j == 0 || j + 1 >= ts.len() {
        return 0.0;
    }
    let d0 = (vs[j] - vs[j - 1]) / (ts[j] - ts[j - 1]);
    let d1 = (vs[j + 1] - vs[j]) / (ts[j + 1] - ts[j]);
    if d0 * d1 <= 0.0 {
        return 0.0;
    }
    let m = (vs[j + 1] - vs[j - 1]) / (ts[j + 1] - ts[j - 1]);
    let max = 3.0 * js_min(d0.abs(), d1.abs());
    if m.abs() > max { sign0(m) * max } else { m }
}

/// A time series at `t`: a monotone C1 cubic between its values, held flat beyond its ends;
/// `fallback` if empty.
pub fn series_at(ts: &[f64], vs: &[f64], t: f64, fallback: f64) -> f64 {
    let n = ts.len();
    if n == 0 {
        return fallback;
    }
    if t <= ts[0] {
        return vs[0];
    }
    if t >= ts[n - 1] {
        return vs[n - 1];
    }
    let mut i = n - 2;
    while i > 0 && t < ts[i] {
        i -= 1;
    }
    let h = ts[i + 1] - ts[i];
    let u = (t - ts[i]) / h;
    let m0 = series_slope(ts, vs, i) * h;
    let m1 = series_slope(ts, vs, i + 1) * h;
    let u2 = u * u;
    let u3 = u2 * u;
    (2.0 * u3 - 3.0 * u2 + 1.0) * vs[i] + (u3 - 2.0 * u2 + u) * m0 + (3.0 * u2 - 2.0 * u3) * vs[i + 1] + (u3 - u2) * m1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;

    fn airport(iata: &str, lat: f64, lon: f64, alt: i64) -> Arc<Airport> {
        Arc::new(Airport {
            id: 1,
            name: "Test".into(),
            iata: iata.into(),
            icao: format!("X{iata}"),
            city: String::new(),
            country: String::new(),
            lat,
            lon,
            alt: Some(alt),
            size: 0,
            timezone: "UTC".into(),
        })
    }

    fn flight(origin: &str, destination: &str) -> LiveFlight {
        LiveFlight { origin: origin.into(), destination: destination.into(), ..LiveFlight::default() }
    }

    #[test]
    fn phases_name_and_group_as_the_typescript() {
        for p in [
            Phase::Parked,
            Phase::Taxi,
            Phase::TakeoffRoll,
            Phase::InitialClimb,
            Phase::Climb,
            Phase::Cruise,
            Phase::Descent,
            Phase::Approach,
            Phase::Flare,
            Phase::LandingRoll,
        ] {
            assert_eq!(Phase::from_name(p.as_str()), Some(p));
        }
        assert_eq!(Phase::TakeoffRoll.as_str(), "takeoffRoll");
        assert_eq!(Phase::from_name("TakeoffRoll"), None);
        assert!(Phase::LandingRoll.is_ground() && !Phase::Flare.is_ground());
        assert!(Phase::TakeoffRoll.from_taxi() && !Phase::LandingRoll.from_taxi());
        assert!(Phase::Descent.from_landing() && !Phase::Taxi.from_landing());
    }

    #[test]
    fn clamp_keeps_nan_and_never_panics() {
        assert_eq!(clamp(5.0, 0.0, 1.0), 1.0);
        assert_eq!(clamp(-5.0, 0.0, 1.0), 0.0);
        assert!(clamp(f64::NAN, 0.0, 1.0).is_nan());
        // inverted bounds: lo wins below it, as in the TypeScript
        assert_eq!(clamp(0.5, 1.0, 0.0), 1.0);
        assert_eq!(clamp(0.5, f64::NAN, 1.0), 0.5);
    }

    #[test]
    fn small_functions_match_the_typescript() {
        // bun: distanceKm(51.47, -0.4543, 51.5, -0.2), distanceKm(0, 179.9, 0.1, -179.9)
        assert_eq!(distance_km(51.47, -0.4543, 51.5, -0.2), 17.921752310589362);
        assert_eq!(distance_km(0.0, 179.9, 0.1, -179.9), 24.863933899043538);
        assert_eq!(field_fade(10.0), 1.0);
        assert_eq!(field_fade(45.0), 0.5);
        assert_eq!(field_fade(70.0), 0.0);
        // bun: airPitch(-700, 72, "approach", 30), airPitch(1800, 80, "initialClimb", NaN),
        //      airPitch(0, 0, "cruise", NaN), airPitch(-3000, 5, "descent", NaN)
        assert_eq!(air_pitch(-700.0, 72.0, Phase::Approach, 30.0), 1.1725226046934303);
        assert_eq!(air_pitch(1800.0, 80.0, Phase::InitialClimb, f64::NAN), 12.52060970856714);
        assert_eq!(air_pitch(0.0, 0.0, Phase::Cruise, f64::NAN), 2.0);
        assert_eq!(air_pitch(-3000.0, 5.0, Phase::Descent, f64::NAN), -10.0);
        // bun: spring(3, 1, 0, 0.1), spring(0, 0, 10, 2)
        assert_eq!(spring(3.0, 1.0, 0.0, 0.1), (2.7900410346781137, -4.245714617988434));
        assert_eq!(spring(0.0, 0.0, 10.0, 2.0), (9.995006007726127, 0.022699964881242426));
    }

    #[test]
    fn measured_weights_fade_after_fifteen_seconds() {
        let mut tr = Track::new(1);
        assert_eq!(meas_weight(&tr, 0.0), 0.0);
        assert!(!meas_fresh(&tr, 0.0));
        tr.meas = Some(Measured::default());
        tr.meas_ms = 10_000.0;
        assert_eq!(meas_weight(&tr, 20_000.0), 1.0);
        assert_eq!(meas_weight(&tr, 25_000.0), 1.0);
        assert_eq!(meas_weight(&tr, 27_500.0), 0.5);
        assert_eq!(meas_weight(&tr, 30_000.0), 0.0);
        assert!(meas_fresh(&tr, 25_000.0));
        assert!(!meas_fresh(&tr, 25_001.0));
    }

    #[test]
    fn series_are_monotone_cubics_held_at_their_ends() {
        let mut ts = Vec::new();
        let mut vs = Vec::new();
        assert_eq!(series_at(&ts, &vs, 5.0, 7.0), 7.0);
        push_series(&mut ts, &mut vs, 1_000.0, 0.0);
        push_series(&mut ts, &mut vs, 2_000.0, 10.0);
        push_series(&mut ts, &mut vs, 3_000.0, 12.0);
        push_series(&mut ts, &mut vs, 4_000.0, 0.0);
        // older is ignored, the same time overwrites
        push_series(&mut ts, &mut vs, 3_500.0, 99.0);
        push_series(&mut ts, &mut vs, 4_000.0, 1.0);
        assert_eq!((ts.clone(), vs.clone()), (vec![1_000.0, 2_000.0, 3_000.0, 4_000.0], vec![0.0, 10.0, 12.0, 1.0]));
        assert_eq!(series_at(&ts, &vs, 0.0, 7.0), 0.0);
        assert_eq!(series_at(&ts, &vs, 9_000.0, 7.0), 1.0);
        assert_eq!(series_at(&ts, &vs, 2_000.0, 7.0), 10.0);
        // a peak has no slope; ends have none; never above the peak between its neighbours
        assert_eq!(series_slope(&ts, &vs, 2), 0.0);
        assert_eq!(series_slope(&ts, &vs, 0), 0.0);
        assert_eq!(series_slope(&ts, &vs, 3), 0.0);
        for k in 0..=100 {
            let v = series_at(&ts, &vs, 2_000.0 + 10.0 * k as f64, 7.0);
            assert!((10.0..=12.0).contains(&v), "{v}");
        }
        // bun: seriesAt([1000,2000,3000,4000],[0,10,12,1],1500,7), (…,2500,7), (…,3700,7)
        assert_eq!(series_at(&ts, &vs, 1_500.0, 7.0), 4.25);
        assert_eq!(series_at(&ts, &vs, 2_500.0, 7.0), 11.75);
        assert_close!(series_at(&ts, &vs, 3_700.0, 7.0), 3.376000000000001, 12);
        // the slope is limited to 3 times the smaller neighbouring difference
        assert_eq!(series_slope(&[0.0, 1.0, 2.0], &[0.0, 0.1, 10.0], 1), 0.30000000000000004);
        // half a minute is kept
        push_series(&mut ts, &mut vs, 33_500.0, 2.0);
        assert_eq!(ts, vec![4_000.0, 33_500.0]);
        assert_eq!(vs, vec![1.0, 2.0]);
        push_series(&mut ts, &mut vs, 100_000.0, 3.0);
        assert_eq!((ts, vs), (vec![100_000.0], vec![3.0]));
    }

    #[test]
    fn the_clock_is_offset_and_smoothed() {
        let mut m = Motion::new();
        assert_eq!(m.now(1_000.0), 1_000.0);
        // garbage is ignored
        m.sync_clock(5.0e11, 1_000.0);
        m.sync_clock(f64::NAN, 1_000.0);
        assert_eq!(m.now(1_000.0), 1_000.0);
        // the first sample is taken as is
        m.sync_clock(1.7e12 + 1_000.0, 1.7e12);
        assert_eq!(m.now(1.7e12), 1.7e12 + 1_000.0);
        // jitter is smoothed (0.1), a jump past 5 s re-syncs
        m.sync_clock(1.7e12 + 2_000.0, 1.7e12);
        assert_eq!(m.now(0.0), 1_100.0);
        m.sync_clock(1.7e12 + 10_000.0, 1.7e12);
        assert_eq!(m.now(0.0), 10_000.0);
    }

    /// `motion.test.ts` "syncClock offsets now()", with the local clock passed in where the
    /// TypeScript read `Date.now()` (here it moves on a few ms between calls, as there).
    #[test]
    fn sync_clock_offsets_now() {
        let mut local = 1_760_000_000_000.0;
        let mut date_now = || {
            local += 3.0;
            local
        };
        let mut m = Motion::new();
        let t = date_now();
        assert!((m.now(t) - t).abs() < 50.0);
        let t = date_now();
        m.sync_clock(t + 60_000.0, t);
        let t = date_now();
        assert!((m.now(t) - t - 60_000.0).abs() < 50.0);
        m.sync_clock(f64::NAN, date_now());
        m.sync_clock(5.0, date_now());
        let t = date_now();
        assert!((m.now(t) - t - 60_000.0).abs() < 50.0);
        // small differences are smoothed, not taken outright
        let t = date_now();
        m.sync_clock(t + 61_000.0, t);
        let t = date_now();
        let offset = m.now(t) - t;
        assert!(offset > 60_000.0 + 50.0);
        assert!(offset < 61_000.0 - 50.0);
    }

    #[test]
    fn airport_cells_wrap_and_skip_bad_positions() {
        assert_eq!(World::cell(0.0, 0.0), 90 * 360);
        assert_eq!(World::cell(0.0, -1.0), 90 * 360 + 359);
        assert_eq!(World::cell(0.0, 360.0), World::cell(0.0, 0.0));
        let mut m = Motion::new();
        let good = airport("AAA", 45.0, 10.0, 100);
        let bad = airport("BBB", f64::NAN, 10.0, 0);
        let anonymous = airport("", 45.95, 10.95, 0);
        m.set_airports(&[good.clone(), bad, anonymous.clone()]);
        assert_eq!(m.world.by_iata.len(), 1);
        assert_eq!(m.world.grid.values().map(Vec::len).sum::<usize>(), 2);
        // the nearest reaches into the neighbouring cells, across the antimeridian too
        assert!(Arc::ptr_eq(&m.world.nearest_airport(46.02, 11.02, 15.0).unwrap(), &anonymous));
        assert!(Arc::ptr_eq(&m.world.nearest_airport(44.99, 9.99, 15.0).unwrap(), &good));
        assert!(m.world.nearest_airport(46.5, 10.0, 15.0).is_none());
        let east = airport("EEE", 0.0, 179.95, 0);
        m.set_airports(std::slice::from_ref(&east));
        assert!(Arc::ptr_eq(&m.world.nearest_airport(0.0, -179.95, 15.0).unwrap(), &east));
        // a new list replaces the old
        assert!(!m.world.by_iata.contains_key("AAA"));
    }

    #[test]
    fn the_field_is_the_destination_then_the_origin_then_the_nearest_then_the_last() {
        let mut m = Motion::new();
        let dest = airport("DST", 45.0, 10.0, 1_000);
        let orig = airport("ORG", 45.2, 10.0, 200);
        let near = airport("NRB", 45.6, 10.0, 50);
        m.set_airports(&[dest.clone(), orig.clone(), near.clone()]);
        let mut tr = Track::new(1);
        let f = flight("ORG", "DST");
        // 22 km from the destination: it is the field
        tr.lat0 = 45.2;
        tr.lon0 = 10.0;
        m.world.select_field(&mut tr, Some(&f));
        assert!(same_airport(tr.field.as_ref(), Some(&dest)));
        assert_eq!(tr.field_elev, 1_000.0);
        assert_eq!(tr.field_km, 22.238985328912065);
        // 44 km out: the origin, 22 km away (the nearest airport is nearer, but comes after)
        tr.lat0 = 45.4;
        m.world.select_field(&mut tr, None);
        assert!(same_airport(tr.field.as_ref(), Some(&orig)));
        // neither within 40 km: the nearest within 15 km
        tr.lat0 = 45.62;
        m.world.select_field(&mut tr, None);
        assert!(same_airport(tr.field.as_ref(), Some(&near)));
        // none of those (22 km from it): the previous one, while within 60 km
        tr.lat0 = 45.8;
        m.world.select_field(&mut tr, None);
        assert!(same_airport(tr.field.as_ref(), Some(&near)));
        assert_eq!(tr.field_elev, 50.0);
        tr.lat0 = 46.2;
        m.world.select_field(&mut tr, None);
        assert!(tr.field.is_none());
        assert_eq!((tr.field_km, tr.field_elev), (f64::INFINITY, 0.0));
        // unknown or empty codes are no airport
        m.world.select_field(&mut tr, Some(&flight("", "XXX")));
        assert!(tr.origin_ap.is_none() && tr.dest_ap.is_none());
    }

    #[test]
    fn airports_compare_by_identity() {
        let a = airport("AAA", 1.0, 2.0, 3);
        let twin = airport("AAA", 1.0, 2.0, 3);
        assert!(same_airport(Some(&a), Some(&a.clone())));
        assert!(!same_airport(Some(&a), Some(&twin)));
        assert!(same_airport(None, None));
        assert!(!same_airport(Some(&a), None));
        assert_eq!(AirportKey(a.clone()), AirportKey(a.clone()));
        assert_ne!(AirportKey(a), AirportKey(twin));
    }

    #[test]
    fn taxiway_networks_are_kept_in_order_and_let_go_of() {
        use crate::model::TaxiKind;
        let line = |lat: f64| TaxiLine { kind: TaxiKind::Taxiway, r#ref: None, points: vec![[10.0, lat], [10.01, lat]] };
        let mut m = Motion::new();
        m.set_taxiways("AAAA", Some(&[line(45.0)]));
        m.set_taxiways("BBBB", Some(&[line(46.0)]));
        assert!(m.has_taxiways("AAAA") && m.has_taxiways("BBBB"));
        let mut tr = Track::new(1);
        tr.snap_net = m.world.taxiways.get("AAAA").cloned();
        m.tracks.insert(1, tr);
        // replacing keeps the place; the old network is let go of
        m.set_taxiways("AAAA", Some(&[line(45.1)]));
        assert_eq!(m.world.taxiways.keys().collect::<Vec<_>>(), ["AAAA", "BBBB"]);
        assert!(m.tracks[&1].snap_net.is_none());
        // forgetting keeps the others' order
        m.set_taxiways("CCCC", Some(&[line(47.0)]));
        m.set_taxiways("AAAA", Some(&[]));
        assert_eq!(m.world.taxiways.keys().collect::<Vec<_>>(), ["BBBB", "CCCC"]);
        m.set_taxiways("BBBB", None);
        assert!(!m.has_taxiways("BBBB"));
    }

    #[test]
    fn prune_drops_tracks_untouched_for_fifteen_minutes() {
        let mut m = Motion::new();
        let mut old = Track::new(1);
        old.touched = 0.0;
        let mut fresh = Track::new(2);
        fresh.touched = 100_000.0;
        m.tracks.insert(1, old);
        m.tracks.insert(2, fresh);
        m.prune(900_000.0);
        assert_eq!(m.tracks.len(), 2);
        m.prune(900_001.0);
        assert!(!m.tracks.contains_key(&1) && m.tracks.contains_key(&2));
        assert_eq!(m.world.last_prune, 900_001.0);
    }

    #[test]
    fn ground_field_and_trail_heights() {
        let mut m = Motion::new();
        let field = airport("FLD", 45.0, 10.0, 1_000);
        let mut tr = Track::new(1);
        tr.field = Some(field.clone());
        tr.field_elev = 1_000.0;
        tr.phase = Phase::Taxi;
        m.tracks.insert(1, tr);
        assert!(Arc::ptr_eq(m.ground_field(1).unwrap(), &field));
        assert!(m.ground_field(2).is_none());
        // trail points: none at or below 0, an unknown aircraft's as given, else less the field
        assert_eq!(m.draw_altitude(1, 0.0, 45.0, 10.0), 0.0);
        assert_eq!(m.draw_altitude(2, 3_000.0, 45.0, 10.0), 3_000.0);
        assert_eq!(m.draw_altitude(1, 3_000.0, 45.0, 10.0), 2_000.0);
        assert_eq!(m.draw_altitude(1, 500.0, 45.0, 10.0), 0.0);
        // corrected with the QNH near the field: 1003.25 hPa reads ~280 ft high there
        m.tracks.get_mut(&1).unwrap().qnh = 1003.25;
        // bun: Math.max(0, correctAlt(3000, 1, 1003.25) - 1000)
        assert_eq!(m.draw_altitude(1, 3_000.0, 45.0, 10.0), 1730.946863930527);
        m.tracks.get_mut(&1).unwrap().phase = Phase::Approach;
        assert!(m.ground_field(1).is_none());
    }
}
