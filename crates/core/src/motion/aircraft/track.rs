//! Per-aircraft state (`motion.ts` `Track`, `:735-920`; doc 05 §10.4), the report `State`
//! (`:719-732`), the un-blended evaluation result `Raw` (`:923-969`), the critically damped
//! `Blend` with `blend` and `gentleRate` (`:618-688`), and the speed profile (`profile`, `ramp`,
//! `:978-1009`).
//!
//! Faithful to the TypeScript's sentinels: times are `f64` ms where `-inf` is "never" and NaN is
//! "unknown", so every comparison reads as there (doc 05 §14). The fields keep their TypeScript
//! names in snake case and their grouping; they are public so that the three parts of the port
//! (and their tests) can reach them, as the TypeScript's free functions did.
//!
//! Airports are shared `Arc<Airport>`s compared by identity (`Arc::ptr_eq`, see
//! [`super::same_airport`]), as the TypeScript compared `Airport` objects; taxiway networks
//! likewise (`net === tr.snapNet`).

use std::sync::Arc;

use super::consts::*;
use super::{Measured, Phase, clamp};
use crate::model::Airport;
use crate::taxinet::TaxiNetwork;
use crate::trains::{js_max, js_min};

/// What a report said and predicted, kept so that it takes over only when the drawn time reaches
/// it: the drawn aircraft is a few seconds in the past, and its phase, gear and height have to be
/// those of that moment.
#[derive(Debug, Clone)]
pub struct State {
    pub report_ms: f64,
    pub phase: Phase,
    pub track_deg: f64,
    /// Reported altitude (ft) and vertical rate (fpm).
    pub alt: f64,
    pub vrate: f64,
    pub field: Option<Arc<Airport>>,
    pub field_elev: f64,
    pub liftoff_ms: f64,
    pub touchdown_ms: f64,
    pub touchdown_pitch: f64,
    pub cap_alt: f64,
}

/// Scratch result of evaluating a track's model at a time (no blending): `motion.ts` `Raw`.
/// `Copy`, so the TypeScript's three module-level scratch values become locals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Raw {
    pub lat: f64,
    pub lon: f64,
    pub alt_msl: f64,
    pub draw_alt: f64,
    pub heading: f64,
    pub track: f64,
    pub pitch: f64,
    pub bank: f64,
    pub phase: Phase,
    pub speed_ms: f64,
    pub vs_fpm: f64,
    /// The altitude (ft) and vertical rate (fpm) the data gives, before the flare shapes them
    /// near the ground.
    pub alt_raw: f64,
    pub vs_raw: f64,
    pub agl: f64,
    pub ground: bool,
    /// Velocity east/north, m/s of drawn time (of real time, once `drawn` scaled it).
    pub v_e: f64,
    pub v_n: f64,
    /// Bank before the measured roll's bias.
    pub model_bank: f64,
    /// The state's field is known (gear logic).
    pub has_field: bool,
}

impl Default for Raw {
    /// `newRaw()`: zeros, cruise, and an unknown height above ground.
    fn default() -> Self {
        Raw {
            lat: 0.0,
            lon: 0.0,
            alt_msl: 0.0,
            draw_alt: 0.0,
            heading: 0.0,
            track: 0.0,
            pitch: 0.0,
            bank: 0.0,
            phase: Phase::Cruise,
            speed_ms: 0.0,
            vs_fpm: 0.0,
            alt_raw: 0.0,
            vs_raw: 0.0,
            agl: f64::NAN,
            ground: false,
            v_e: 0.0,
            v_n: 0.0,
            model_bank: 0.0,
            has_field: false,
        }
    }
}

/// Critically damped decay (rate k) of a correction applied `t` seconds ago: 1 → 0 with zero slope at 0.
pub fn blend(t: f64, k: f64) -> f64 {
    if t <= 0.0 {
        return 1.0;
    }
    let kt = k * t;
    if kt >= BLEND_END_KT {
        return 0.0;
    }
    (1.0 + kt) * (-kt).exp()
}

/// Blend rate for a small correction of `x0` m with a rate difference of `v0` m/s: the slowest
/// that keeps its peak acceleration (≈ 2 k v0 + k² x0) under GENTLE_ACCEL, within
/// MIN_BLEND_RATE..BLEND_RATE.
pub fn gentle_rate(x0: f64, v0: f64) -> f64 {
    let k = if x0 > 1e-6 {
        (-v0 + (v0 * v0 + x0 * GENTLE_ACCEL).sqrt()) / x0
    } else if v0 > 1e-6 {
        GENTLE_ACCEL / (2.0 * v0)
    } else {
        BLEND_RATE
    };
    clamp(k, MIN_BLEND_RATE, BLEND_RATE)
}

/// A correction being blended out, critically damped from an offset and its rate:
/// x(t) = (x0 + (v0 + k x0) t) e^{-kt}, so the drawn position and its velocity are both continuous
/// (C1). Restarting one from its current offset and rate at the same k continues the same curve,
/// so corrections simply add.
///
/// As in the TypeScript, [`Blend::at`] leaves its result in `px`/`py`/`pvx`/`pvy`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blend {
    pub ms: f64,
    pub k: f64,
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    // results of `at`
    pub px: f64,
    pub py: f64,
    pub pvx: f64,
    pub pvy: f64,
    /// Converts the units to metres for the gentle rate.
    pub scale: f64,
}

impl Blend {
    pub fn new(scale: f64) -> Self {
        Blend {
            ms: f64::NEG_INFINITY,
            k: BLEND_RATE,
            x: 0.0,
            y: 0.0,
            vx: 0.0,
            vy: 0.0,
            px: 0.0,
            py: 0.0,
            pvx: 0.0,
            pvy: 0.0,
            scale,
        }
    }

    /// The offset and its rate at `now_ms`, into px/py/pvx/pvy (all 0 once it is over).
    pub fn at(&mut self, now_ms: f64) {
        let t = js_max(0.0, (now_ms - self.ms) / 1000.0);
        let k = self.k;
        if !(k * t < BLEND_END_KT) {
            self.px = 0.0;
            self.py = 0.0;
            self.pvx = 0.0;
            self.pvy = 0.0;
            return;
        }
        let e = (-k * t).exp();
        let cx = self.vx + k * self.x;
        let cy = self.vy + k * self.y;
        self.px = (self.x + cx * t) * e;
        self.py = (self.y + cy * t) * e;
        self.pvx = (self.vx - k * cx * t) * e;
        self.pvy = (self.vy - k * cy * t) * e;
    }

    /// Adds a correction and its rate at `now_ms` to what is left of this one; on at rate `k`, or
    /// (`None`) the gentle rate for the sum.
    pub fn add(&mut self, now_ms: f64, dx: f64, dy: f64, dvx: f64, dvy: f64, k: Option<f64>) {
        self.at(now_ms);
        self.x = self.px + dx;
        self.y = self.py + dy;
        self.vx = self.pvx + dvx;
        self.vy = self.pvy + dvy;
        self.ms = now_ms;
        self.k = match k {
            Some(k) => k,
            None => gentle_rate(self.x.hypot(self.y) * self.scale, self.vx.hypot(self.vy) * self.scale),
        };
    }

    pub fn clear(&mut self) {
        self.ms = f64::NEG_INFINITY;
    }
}

/// Per-aircraft state: samples, latest report, estimates, prediction and blending.
#[derive(Debug, Clone)]
pub struct Track {
    pub id: u32,
    pub touched: f64,
    pub cls: AircraftClass,
    pub icon: String,
    pub typecode: String,

    // latest report (all describe `report_ms`), from either source
    pub report_ms: f64,
    /// Time of the newest Flightradar24 report taken in.
    pub fr24_ms: f64,
    pub lat0: f64,
    pub lon0: f64,
    pub alt: f64,
    pub speed_kt: f64,
    pub track_deg: f64,
    pub on_ground: bool,
    pub origin_ap: Option<Arc<Airport>>,
    pub dest_ap: Option<Arc<Airport>>,
    /// The relevant field for AGL and drawing height, or none.
    pub field: Option<Arc<Airport>>,
    pub field_elev: f64,
    pub field_km: f64,

    // estimates
    /// kt/s
    pub accel: f64,
    /// Ground-speed change since the previous report (kt/s), NaN if too close or too far apart to tell.
    pub report_accel: f64,
    /// deg/s, clockwise positive
    pub turn_rate: f64,
    /// fpm
    pub vrate: f64,
    pub vr_ref_ms: f64,
    /// The vertical rate's reference: a pressure altitude and its correction weight (re-corrected
    /// as the QNH changes).
    pub vr_ref_raw: f64,
    pub vr_ref_w: f64,
    pub vr_ref_ground: bool,
    pub phase: Phase,

    // pressure altitude
    /// QNH (hPa) the altitudes are corrected with, NaN when unknown (no correction).
    pub qnh: f64,
    /// QNH read off this aircraft's own runway (at `qnh_own_field`), NaN if none.
    pub qnh_own: f64,
    pub qnh_own_field: Option<Arc<Airport>>,
    /// The latest reported pressure altitude (ft; 0 is no altitude), its time and its correction weight.
    pub raw_alt: f64,
    pub raw_alt_ms: f64,
    pub raw_w: f64,
    /// When the aircraft was last on the ground: older altitude samples belong to an earlier flight segment.
    pub ground_ms: f64,
    /// Altitude corrections are blended out at this rate on the next change (a QNH learned), else NaN.
    pub alt_blend_rate: f64,

    // horizontal samples, oldest first; xs/ys (raw) and fx/fy/mx/my (smoothed values and slopes,
    // m and m/s) are in the local frame around the newest
    pub n: usize,
    pub ts: Vec<f64>,
    pub lats: Vec<f64>,
    pub lons: Vec<f64>,
    pub xs: Vec<f64>,
    pub ys: Vec<f64>,
    pub fx: Vec<f64>,
    pub fy: Vec<f64>,
    pub mx: Vec<f64>,
    pub my: Vec<f64>,
    /// Turn rate of the smoothed path at each sample, rad/s, clockwise positive.
    pub om: Vec<f64>,
    /// The last fit's reference turn rate (rad/s) and acceleration (m/s²) at its sample times,
    /// which seed the next.
    pub r_ts: Vec<f64>,
    pub r_om: Vec<f64>,
    pub r_ac: Vec<f64>,

    // altitude samples (airborne only), oldest first: reported pressure altitudes (ft) and their
    // correction weights; the corrected values (ft), and the smoothed values (ft) and slopes (ft/s)
    pub ats: Vec<f64>,
    pub aps: Vec<f64>,
    pub aws: Vec<f64>,
    pub avs: Vec<f64>,
    pub afv: Vec<f64>,
    pub afd: Vec<f64>,

    /// Each report's state, oldest first: the one in effect at the drawn time is used.
    pub states: Vec<State>,
    /// The state last drawn (its report_ms).
    pub active_ms: f64,
    /// When pose() was last called.
    pub last_pose_ms: f64,

    // measured attitude history: roll (and its bias to the model's bank) and crab, by time
    pub roll_ts: Vec<f64>,
    pub rolls: Vec<f64>,
    pub roll_biases: Vec<f64>,
    pub crab_ts: Vec<f64>,
    pub crabs: Vec<f64>,

    // drawing delay: eases from `d_from` (slope `d_slope`) at `d_ms` to `d_to` over `d_dur`, ms
    pub d_from: f64,
    pub d_to: f64,
    pub d_slope: f64,
    pub d_ms: f64,
    pub d_dur: f64,
    /// Smoothed data cadence (ms between reports) and delay target, ms.
    pub cadence_ms: f64,
    pub delay_target: f64,
    /// Newest sample time when data last arrived.
    pub newest_ms: f64,
    /// Data time of the last arrival that extended the samples.
    pub arrival_data_ms: f64,
    pub ref_lat: f64,
    pub ref_lon: f64,
    /// Per sample source (SRC_*), kept only once measured samples are mixed in.
    pub src: Vec<u8>,
    pub mixed: bool,

    // measured data (selected aircraft only)
    pub meas: Option<Measured>,
    /// When the measured values were reported; -inf without any.
    pub meas_ms: f64,
    /// Newest measured position taken in.
    pub meas_pos_ms: f64,
    /// Nose heading minus track, degrees.
    pub crab: f64,
    /// True airspeed, m/s (0: unknown).
    pub tas_ms: f64,
    /// Track (deg) the autopilot is turning onto, NaN if none.
    pub nav_track: f64,
    /// Altitude (ft) the climb or descent levels off at, NaN if none.
    pub cap_alt: f64,
    /// Seconds past the newest sample when the predicted turn ends.
    pub turn_end_s: f64,

    // extrapolation past the newest sample
    /// m/s
    pub v0: f64,
    /// rad
    pub h0: f64,
    /// 1/m
    pub kappa: f64,
    /// m
    pub s_turn_max: f64,
    pub seg_a: Vec<f64>,
    pub seg_dur: Vec<f64>,
    pub prof_end_v: f64,
    pub prof_end_t: f64,

    // predicted touchdown / lift-off, server ms (NaN when none)
    pub touchdown_ms: f64,
    pub touchdown_pitch: f64,
    pub liftoff_ms: f64,

    // the angles' correction being blended out
    pub off_ms: f64,
    /// Position (east/north, m) and height (ft) corrections: a large one (a jump in the data) at
    /// the full rate, small ones (the smoothed curve moving as samples come in) gently, so their
    /// acceleration stays small.
    pub fast: Blend,
    pub gentle: Blend,
    pub alt_fast: Blend,
    pub alt_gentle: Blend,
    pub off_hdg: f64,
    pub off_trk: f64,
    pub off_pitch: f64,
    pub off_bank: f64,

    // taxiway snapping (see `Motion.taxiSnap`)
    /// The network the aircraft is in (compared with `Arc::ptr_eq`), and the gate (0..1, eased)
    /// snapping is let through.
    pub snap_net: Option<Arc<TaxiNetwork>>,
    pub snap_gate: f64,
    pub snap_ms: f64,
    /// The direction of motion `SNAP_LOOKAHEAD_MS` ahead (unit east, north), as last followed, and when.
    pub snap_ax: f64,
    pub snap_ay: f64,
    pub snap_ahead_ms: f64,
    /// The offset (east, north m; heading deg) last applied before the gate, and its rates (/s;
    /// the heading's of `snap_hd`).
    pub snap_e: f64,
    pub snap_n: f64,
    pub snap_h: f64,
    pub snap_ve: f64,
    pub snap_vn: f64,
    pub snap_vh: f64,
    /// The heading the offset draws (deg), which the heading's spring follows.
    pub snap_hd: f64,

    // gear animation
    /// -1 until the first pose sets it.
    pub gear_target: f64,
    pub gear_from: f64,
    pub gear_ms: f64,
}

impl Track {
    /// A track not yet fed: every field as `new Track(id)` starts it.
    pub fn new(id: u32) -> Self {
        let ninf = f64::NEG_INFINITY;
        Track {
            id,
            touched: 0.0,
            cls: AircraftClass::Jet,
            icon: String::new(),
            typecode: String::new(),

            report_ms: ninf,
            fr24_ms: ninf,
            lat0: 0.0,
            lon0: 0.0,
            alt: 0.0,
            speed_kt: 0.0,
            track_deg: 0.0,
            on_ground: false,
            origin_ap: None,
            dest_ap: None,
            field: None,
            field_elev: 0.0,
            field_km: f64::INFINITY,

            accel: 0.0,
            report_accel: f64::NAN,
            turn_rate: 0.0,
            vrate: 0.0,
            vr_ref_ms: f64::NAN,
            vr_ref_raw: 0.0,
            vr_ref_w: 0.0,
            vr_ref_ground: false,
            phase: Phase::Cruise,

            qnh: f64::NAN,
            qnh_own: f64::NAN,
            qnh_own_field: None,
            raw_alt: 0.0,
            raw_alt_ms: ninf,
            raw_w: 0.0,
            ground_ms: ninf,
            alt_blend_rate: f64::NAN,

            n: 0,
            ts: Vec::new(),
            lats: Vec::new(),
            lons: Vec::new(),
            xs: Vec::new(),
            ys: Vec::new(),
            fx: Vec::new(),
            fy: Vec::new(),
            mx: Vec::new(),
            my: Vec::new(),
            om: Vec::new(),
            r_ts: Vec::new(),
            r_om: Vec::new(),
            r_ac: Vec::new(),

            ats: Vec::new(),
            aps: Vec::new(),
            aws: Vec::new(),
            avs: Vec::new(),
            afv: Vec::new(),
            afd: Vec::new(),

            states: Vec::new(),
            active_ms: f64::NAN,
            last_pose_ms: ninf,

            roll_ts: Vec::new(),
            rolls: Vec::new(),
            roll_biases: Vec::new(),
            crab_ts: Vec::new(),
            crabs: Vec::new(),

            d_from: MIN_DELAY_MS,
            d_to: MIN_DELAY_MS,
            d_slope: 0.0,
            d_ms: ninf,
            d_dur: 0.0,
            cadence_ms: DEFAULT_CADENCE_MS,
            delay_target: f64::NAN,
            newest_ms: ninf,
            arrival_data_ms: ninf,
            ref_lat: 0.0,
            ref_lon: 0.0,
            src: Vec::new(),
            mixed: false,

            meas: None,
            meas_ms: ninf,
            meas_pos_ms: ninf,
            crab: 0.0,
            tas_ms: 0.0,
            nav_track: f64::NAN,
            cap_alt: f64::NAN,
            turn_end_s: TURN_HORIZON_S,

            v0: 0.0,
            h0: 0.0,
            kappa: 0.0,
            s_turn_max: 0.0,
            seg_a: Vec::new(),
            seg_dur: Vec::new(),
            prof_end_v: 0.0,
            prof_end_t: 0.0,

            touchdown_ms: f64::NAN,
            touchdown_pitch: 0.0,
            liftoff_ms: f64::NAN,

            off_ms: ninf,
            fast: Blend::new(1.0),
            gentle: Blend::new(1.0),
            alt_fast: Blend::new(FEET_TO_M),
            alt_gentle: Blend::new(FEET_TO_M),
            off_hdg: 0.0,
            off_trk: 0.0,
            off_pitch: 0.0,
            off_bank: 0.0,

            snap_net: None,
            snap_gate: 0.0,
            snap_ms: ninf,
            snap_ax: 0.0,
            snap_ay: 1.0,
            snap_ahead_ms: ninf,
            snap_e: 0.0,
            snap_n: 0.0,
            snap_h: 0.0,
            snap_ve: 0.0,
            snap_vn: 0.0,
            snap_vh: 0.0,
            snap_hd: 0.0,

            gear_target: -1.0,
            gear_from: 0.0,
            gear_ms: ninf,
        }
    }

    /// `vrReference`: the latest report (on the ground, or with an altitude) becomes the vertical
    /// rate's reference.
    pub fn vr_reference(&mut self, t: f64) {
        if !self.on_ground && !(self.raw_alt > 0.0) {
            return;
        }
        self.vr_ref_ms = t;
        self.vr_ref_raw = self.raw_alt;
        self.vr_ref_w = self.raw_w;
        self.vr_ref_ground = self.on_ground;
    }
}

/// Distance (m) and speed (m/s) `dt` seconds along the track's speed profile (`profile`, which
/// wrote them into the module's `profS`/`profV`).
pub fn profile(tr: &Track, dt: f64) -> (f64, f64) {
    let mut s = 0.0;
    let mut t = 0.0;
    let mut v = tr.v0;
    for i in 0..tr.seg_a.len() {
        let d = tr.seg_dur[i];
        let a = tr.seg_a[i];
        if dt <= t + d {
            let u = dt - t;
            return (s + v * u + 0.5 * a * u * u, v + a * u);
        }
        s += v * d + 0.5 * a * d * d;
        v += a * d;
        t += d;
    }
    (s + v * (dt - t), v)
}

/// Appends a constant-acceleration piece (m/s²) to the speed profile, until `target` m/s or
/// `max_dur` s (none when that duration is not positive and finite).
pub fn ramp(tr: &mut Track, a: f64, target: f64, max_dur: f64) {
    let mut dur = max_dur;
    if a != 0.0 {
        dur = js_min(dur, js_max(0.0, (target - tr.prof_end_v) / a));
    }
    if !(dur > 0.0) || !dur.is_finite() {
        return;
    }
    tr.seg_a.push(a);
    tr.seg_dur.push(dur);
    tr.prof_end_v += a * dur;
    tr.prof_end_t += dur;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;

    #[test]
    fn a_new_track_starts_as_the_typescript_does() {
        let tr = Track::new(7);
        assert_eq!(tr.id, 7);
        assert_eq!(tr.report_ms, f64::NEG_INFINITY);
        assert!(tr.report_accel.is_nan() && tr.vr_ref_ms.is_nan() && tr.qnh.is_nan());
        assert!(tr.active_ms.is_nan() && tr.delay_target.is_nan() && tr.nav_track.is_nan());
        assert!(tr.cap_alt.is_nan() && tr.touchdown_ms.is_nan() && tr.liftoff_ms.is_nan());
        assert_eq!(tr.field_km, f64::INFINITY);
        assert_eq!(tr.phase, Phase::Cruise);
        assert_eq!((tr.d_from, tr.d_to, tr.cadence_ms), (2_000.0, 2_000.0, 8_000.0));
        assert_eq!(tr.turn_end_s, 60.0);
        assert_eq!((tr.snap_ax, tr.snap_ay), (0.0, 1.0));
        assert_eq!(tr.gear_target, -1.0);
        assert_eq!(tr.fast.scale, 1.0);
        assert_eq!(tr.alt_fast.scale, 0.3048);
        assert_eq!(tr.n, 0);
        assert!(Raw::default().agl.is_nan());
    }

    #[test]
    fn blend_decays_critically_damped_and_ends() {
        assert_eq!(blend(-1.0, 2.2), 1.0);
        assert_eq!(blend(0.0, 2.2), 1.0);
        // (1 + kt) e^-kt
        assert_eq!(blend(1.0, 2.2), 3.2 * (-2.2f64).exp());
        assert_close!(blend(3.0, 2.2), 0.0103, 4);
        // gone once kt reaches 13.2
        assert_eq!(blend(6.0, 2.2), 0.0);
        assert!(blend(5.99, 2.2) > 0.0);
    }

    #[test]
    fn gentle_rate_keeps_the_peak_acceleration_down() {
        // bun: gentleRate(10, 0), gentleRate(10, 2), gentleRate(0, 1), gentleRate(0, 0), gentleRate(1000, 0)
        assert_eq!(gentle_rate(10.0, 0.0), 0.5477225575051661);
        assert_eq!(gentle_rate(10.0, 2.0), 0.5);
        assert_eq!(gentle_rate(0.0, 1.0), 1.5);
        assert_eq!(gentle_rate(0.0, 0.0), 2.2);
        assert_eq!(gentle_rate(1000.0, 0.0), 0.5);
        assert_eq!(gentle_rate(0.1, 0.0), 2.2);
    }

    #[test]
    fn blends_add_up_and_stay_c1() {
        let mut b = Blend::new(1.0);
        // nothing under way
        b.at(5_000.0);
        assert_eq!((b.px, b.py, b.pvx, b.pvy), (0.0, 0.0, 0.0, 0.0));
        b.add(1_000.0, 10.0, -4.0, 0.0, 1.0, Some(2.2));
        b.at(1_000.0);
        assert_eq!((b.px, b.py, b.pvx, b.pvy), (10.0, -4.0, 0.0, 1.0));
        // the rate is the derivative of the offset
        b.at(1_500.0);
        let (x0, vx0) = (b.px, b.pvx);
        b.at(1_501.0);
        assert_close!((b.px - x0) / 0.001, vx0, 1);
        // a second correction adds to what is left, and continuing at the same k changes nothing
        b.at(2_000.0);
        let (x, y, vx, vy) = (b.px, b.py, b.pvx, b.pvy);
        let mut c = b;
        c.add(2_000.0, 0.0, 0.0, 0.0, 0.0, Some(2.2));
        for t in [2_000.0, 2_700.0, 4_000.0] {
            b.at(t);
            c.at(t);
            assert_close!(b.px, c.px, 9);
            assert_close!(b.pvy, c.pvy, 9);
        }
        c.add(2_000.0, 1.0, 1.0, 0.0, 0.0, Some(2.2));
        c.at(2_000.0);
        assert_close!(c.px, x + 1.0, 12);
        assert_close!(c.py, y + 1.0, 12);
        assert_close!(c.pvx, vx, 12);
        assert_close!(c.pvy, vy, 12);
        // the gentle rate for the summed size, in metres
        let mut g = Blend::new(0.3048);
        g.add(0.0, 100.0, 0.0, 0.0, 0.0, None);
        assert_eq!(g.k, gentle_rate(30.48, 0.0));
        // cleared: over
        g.clear();
        g.at(10.0);
        assert_eq!(g.px, 0.0);
    }

    #[test]
    fn blend_matches_the_typescript() {
        // bun: b = new Blend(); b.add(1000, 30, -20, 4, 1, null); b.at(2500)
        let mut b = Blend::new(1.0);
        b.add(1_000.0, 30.0, -20.0, 4.0, 1.0, None);
        assert_eq!(b.k, 0.5);
        b.at(2_500.0);
        assert_close!(b.px, 27.63344333534936, 12);
        assert_close!(b.py, -15.824279516823992, 12);
        assert_close!(b.pvx, -4.841757165595401, 12);
        assert_close!(b.pvy, 3.660840783742864, 12);
    }

    #[test]
    fn the_speed_profile_integrates_its_pieces() {
        let mut tr = Track::new(1);
        tr.v0 = 10.0;
        tr.prof_end_v = 10.0;
        // 2 m/s² until 20 m/s (5 s), then hold
        ramp(&mut tr, 2.0, 20.0, f64::INFINITY);
        assert_eq!((tr.seg_a.clone(), tr.seg_dur.clone()), (vec![2.0], vec![5.0]));
        assert_eq!((tr.prof_end_v, tr.prof_end_t), (20.0, 5.0));
        assert_eq!(profile(&tr, 0.0), (0.0, 10.0));
        assert_eq!(profile(&tr, 2.0), (24.0, 14.0));
        assert_eq!(profile(&tr, 5.0), (75.0, 20.0));
        assert_eq!(profile(&tr, 10.0), (175.0, 20.0));
        // a target already passed, a zero or NaN acceleration with an endless duration: no piece
        ramp(&mut tr, 2.0, 15.0, 10.0);
        ramp(&mut tr, 0.0, 0.0, f64::INFINITY);
        ramp(&mut tr, f64::NAN, 30.0, 10.0);
        assert_eq!(tr.seg_a.len(), 1);
        // a level piece for a set time
        ramp(&mut tr, 0.0, 0.0, 3.0);
        assert_eq!((tr.prof_end_v, tr.prof_end_t), (20.0, 8.0));
        // braking to a stop and beyond it: held at the end speed
        ramp(&mut tr, -4.0, 0.0, f64::INFINITY);
        assert_eq!(tr.seg_dur.last(), Some(&5.0));
        assert_eq!(profile(&tr, 13.0), (75.0 + 60.0 + 50.0, 0.0));
        assert_eq!(profile(&tr, 20.0), (185.0, 0.0));
    }

    #[test]
    fn the_vertical_rate_reference_needs_an_altitude_or_the_ground() {
        let mut tr = Track::new(1);
        tr.raw_alt = 0.0;
        tr.vr_reference(1_000.0);
        assert!(tr.vr_ref_ms.is_nan());
        tr.raw_alt = 3_000.0;
        tr.raw_w = 0.5;
        tr.vr_reference(2_000.0);
        assert_eq!((tr.vr_ref_ms, tr.vr_ref_raw, tr.vr_ref_w, tr.vr_ref_ground), (2_000.0, 3_000.0, 0.5, false));
        tr.raw_alt = 0.0;
        tr.on_ground = true;
        tr.vr_reference(3_000.0);
        assert_eq!((tr.vr_ref_ms, tr.vr_ref_raw, tr.vr_ref_ground), (3_000.0, 0.0, true));
    }
}
