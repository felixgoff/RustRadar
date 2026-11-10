//! The smoothed horizontal path of `motion.ts` (doc 05 §10.7): the fit around a reference path
//! (`fitTrack`, `localRates`, `fitAround`, `referenceWeight`, `:2790-3018`), its Hermite pieces
//! (`hermite`, `:3076-3107`) and the turn rate at each sample for the bank (`turnNodes`,
//! `:3124-3148`).
//!
//! The TypeScript's module-level scratch (`splineBuf`, `weightBuf`, `refX`, …, `resDY`) lives in
//! [`Scratch`]; `hermite`'s results (`hx`, `hy`, `hvx`, `hvy`, `hi`, `hu`) are its return value,
//! [`Hermite`].

use super::consts::*;
use super::spline::{EndSlope, Series, smooth_spline};
use super::track::Track;
use super::{Phase, clamp, wrap180};
use crate::trains::{js_max, js_min};

/// Scratch for the fits, grown as needed and reused: they run for every aircraft in every
/// snapshot.
#[derive(Debug, Default)]
pub struct Scratch {
    /// The smoothing spline's solver buffer (`splineBuf`), shared with
    /// [`fit_altitude`](super::altitude::fit_altitude).
    pub spline: Vec<f64>,
    /// Per-sample weights of a mixed track (`weightBuf`).
    pub weights: Vec<f64>,
    /// The reference path (m), its velocity (m/s), and its turn rate (rad/s) and acceleration
    /// (m/s²) per sample, with the next pass's rates.
    pub ref_x: Vec<f64>,
    pub ref_y: Vec<f64>,
    pub ref_vx: Vec<f64>,
    pub ref_vy: Vec<f64>,
    pub ref_om: Vec<f64>,
    pub ref_ac: Vec<f64>,
    pub next_om: Vec<f64>,
    pub next_ac: Vec<f64>,
    /// The samples' residuals from the reference, and their fit (values, slopes).
    pub res_x: Vec<f64>,
    pub res_y: Vec<f64>,
    pub res_fx: Vec<f64>,
    pub res_fy: Vec<f64>,
    pub res_dx: Vec<f64>,
    pub res_dy: Vec<f64>,
}

/// The smoothed horizontal path at a time inside the samples: position (m, local frame) and
/// velocity (m/s), on piece `i` (between samples `i` and `i + 1`) at fraction `u` (the
/// TypeScript's `hx`, `hy`, `hvx`, `hvy`, `hi`, `hu`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hermite {
    pub x: f64,
    pub y: f64,
    pub vx: f64,
    pub vy: f64,
    pub i: usize,
    pub u: f64,
}

/// The smoothed horizontal path at `t` (inside the samples): the cubic Hermite piece of the
/// segment holding `t`, searched from the newest backwards. Needs two samples and their fit; with
/// fewer (which the TypeScript's callers never ask for: it would read `undefined`) every number
/// is NaN.
pub fn hermite(tr: &Track, t: f64) -> Hermite {
    if tr.n < 2 {
        return Hermite { x: f64::NAN, y: f64::NAN, vx: f64::NAN, vy: f64::NAN, i: 0, u: f64::NAN };
    }
    let ts = &tr.ts;
    let mut i = tr.n - 2;
    while i > 0 && t < ts[i] {
        i -= 1;
    }
    let h = (ts[i + 1] - ts[i]) / 1000.0;
    let u = (t - ts[i]) / 1000.0 / h;
    let u2 = u * u;
    let u3 = u2 * u;
    let h00 = 2.0 * u3 - 3.0 * u2 + 1.0;
    let h10 = u3 - 2.0 * u2 + u;
    let h01 = 3.0 * u2 - 2.0 * u3;
    let h11 = u3 - u2;
    let d00 = 6.0 * u2 - 6.0 * u;
    let d10 = 3.0 * u2 - 4.0 * u + 1.0;
    let d11 = 3.0 * u2 - 2.0 * u;
    let (fx, fy, mx, my) = (&tr.fx, &tr.fy, &tr.mx, &tr.my);
    Hermite {
        x: h00 * fx[i] + h10 * h * mx[i] + h01 * fx[i + 1] + h11 * h * mx[i + 1],
        y: h00 * fy[i] + h10 * h * my[i] + h01 * fy[i + 1] + h11 * h * my[i + 1],
        vx: (d00 * (fx[i] - fx[i + 1])) / h + d10 * mx[i] + d11 * mx[i + 1],
        vy: (d00 * (fy[i] - fy[i + 1])) / h + d10 * my[i] + d11 * my[i + 1],
        i,
        u,
    }
}

/// How much of the first pass's (constant) reference turn applies `back` seconds before the
/// newest sample: full, then fading out.
pub fn reference_weight(back: f64) -> f64 {
    if back <= REF_FULL_S {
        return 1.0;
    }
    js_max(0.0, (REF_ZERO_S - back) / (REF_ZERO_S - REF_FULL_S))
}

/// The smoothed horizontal path through the samples (values `fx`/`fy`, slopes `mx`/`my`). A
/// smoothing spline penalises acceleration, so on its own it would straighten a steady turn (a
/// constant sideways acceleration) by tens of metres. Instead it smooths the samples' difference
/// from a reference path that turns, and changes speed, as the data itself does, then adds the
/// reference back: a turn costs nothing and only the jitter is taken out.
///
/// The reference turns and changes speed at each moment's own rates, given per sample. One
/// constant turn across a straight leg and the turn after it would bend the straight part (the
/// spline only smooths the difference), so the aircraft would be drawn turning, and banked,
/// seconds before it did; a reference that is straight where the data is and turns where it does
/// leaves nothing to bend. The first pass takes the last fit's rates where it covered the same
/// moments (it had the data either side of them) and the report's estimates, fading out with age,
/// for the rest; the next two read them off the pass before, over ±REF_LOCAL_MS. Averaged over
/// seconds (several samples), they leave the jitter mostly to the spline. A newest sample that is
/// the report itself (`reported_only`, as it is unless a measured one is newer) takes the
/// reported velocity as its slope, softly.
///
/// The rates the fit settled on are kept in `r_ts`/`r_om`/`r_ac` to seed the next one.
pub fn fit_track(tr: &mut Track, sc: &mut Scratch, reported_only: bool) {
    let n = tr.n;
    tr.fx.resize(n, 0.0);
    tr.fy.resize(n, 0.0);
    tr.mx.resize(n, 0.0);
    tr.my.resize(n, 0.0);
    if n == 0 {
        // what the TypeScript's loops leave without samples: no curve and no rates to seed from
        tr.r_ts.clear();
        tr.r_om.clear();
        tr.r_ac.clear();
        return;
    }
    let v = if tr.phase == Phase::Parked { 0.0 } else { tr.speed_kt * KNOTS_TO_MS };
    let rx = v * (tr.track_deg * DEG).sin();
    let ry = v * (tr.track_deg * DEG).cos();
    if n == 1 {
        tr.fx[0] = tr.xs[0];
        tr.fy[0] = tr.ys[0];
        tr.mx[0] = rx;
        tr.my[0] = ry;
        return;
    }
    let weighted = tr.mixed;
    if weighted {
        sc.weights.clear();
        sc.weights.extend(tr.src[..n].iter().map(|&s| {
            if s == SRC_MLAT {
                WEIGHT_MLAT
            } else if s == SRC_ADSB {
                WEIGHT_ADSB
            } else {
                WEIGHT_FR24
            }
        }));
    }
    // seeded with the report's estimates (fading out with age), then each moment's own, from the data
    let moving = tr.phase != Phase::Parked && v > HEADING_MIN_SPEED_MS;
    let omega = if moving { tr.turn_rate * DEG } else { 0.0 };
    let accel = if moving { tr.accel * KNOTS_TO_MS } else { 0.0 };
    sc.ref_om.resize(n, 0.0);
    sc.ref_ac.resize(n, 0.0);
    let t_end = tr.ts[n - 1];
    for i in 0..n {
        let w = reference_weight((t_end - tr.ts[i]) / 1000.0);
        sc.ref_om[i] = omega * w;
        sc.ref_ac[i] = accel * w;
    }
    // where the last fit covered the same moments, its rates are the better start (it had the data either side)
    if moving {
        let (r_ts, r_om, r_ac) = (&tr.r_ts, &tr.r_om, &tr.r_ac);
        let m = r_ts.len();
        let mut j = 0;
        let mut i = 0;
        while i < n && m > 1 {
            let t = tr.ts[i];
            if t < r_ts[0] || t > r_ts[m - 1] {
                i += 1;
                continue;
            }
            while j < m - 2 && r_ts[j + 1] < t {
                j += 1;
            }
            let u = (t - r_ts[j]) / (r_ts[j + 1] - r_ts[j]);
            sc.ref_om[i] = r_om[j] + (r_om[j + 1] - r_om[j]) * u;
            sc.ref_ac[i] = r_ac[j] + (r_ac[j + 1] - r_ac[j]) * u;
            i += 1;
        }
    }
    for pass in 0..REF_PASSES {
        fit_around(tr, sc, weighted, reported_only, rx, ry, pass == 0);
        if pass == REF_PASSES - 1 || !moving {
            break;
        }
        // each sample's turn and speed change on this pass's path
        local_rates(tr, &mut sc.next_om, &mut sc.next_ac, &sc.ref_om, &sc.ref_ac);
        let mut change = 0.0;
        for i in 0..n {
            change = js_max(
                js_max(change, (sc.next_om[i] - sc.ref_om[i]).abs() / (0.02 * DEG)),
                (sc.next_ac[i] - sc.ref_ac[i]).abs() / 0.02,
            );
            sc.ref_om[i] = sc.next_om[i];
            sc.ref_ac[i] = sc.next_ac[i];
        }
        // as assumed: the fit stands
        if change < 1.0 {
            break;
        }
    }
    tr.r_ts.resize(n, 0.0);
    tr.r_om.resize(n, 0.0);
    tr.r_ac.resize(n, 0.0);
    for i in 0..n {
        tr.r_ts[i] = tr.ts[i];
        tr.r_om[i] = if moving { sc.ref_om[i] } else { 0.0 };
        tr.r_ac[i] = if moving { sc.ref_ac[i] } else { 0.0 };
    }
}

/// Turn rate (rad/s) and acceleration (m/s²) of the smoothed path at each sample, from its
/// heading's and speed's change over ±REF_LOCAL_MS (as much of that as the samples cover), into
/// `om`/`ac`; where that is shorter than REF_MIN_CHORD_S, the sample keeps `om0`/`ac0`'s.
pub fn local_rates(tr: &Track, om: &mut Vec<f64>, ac: &mut Vec<f64>, om0: &[f64], ac0: &[f64]) {
    let n = tr.n;
    om.resize(n, 0.0);
    ac.resize(n, 0.0);
    if n == 0 {
        return;
    }
    let first = tr.ts[0];
    let newest = tr.ts[n - 1];
    let max_turn = MAX_TURN_DEG_S * DEG;
    let max_accel = MAX_ACCEL_KTS * KNOTS_TO_MS;
    for i in 0..n {
        let a = js_max(first, tr.ts[i] - REF_LOCAL_MS);
        let b = js_min(newest, tr.ts[i] + REF_LOCAL_MS);
        let mut w = om0[i];
        let mut acc = ac0[i];
        if b - a >= REF_MIN_CHORD_S * 1000.0 {
            w = 0.0;
            let ha = hermite(tr, a);
            let (ax, ay) = (ha.vx, ha.vy);
            let hb = hermite(tr, b);
            let s0 = ax.hypot(ay);
            let s1 = hb.vx.hypot(hb.vy);
            let dt = (b - a) / 1000.0;
            if s0 > HEADING_MIN_SPEED_MS && s1 > HEADING_MIN_SPEED_MS {
                w = (wrap180((hb.vx.atan2(hb.vy) - ax.atan2(ay)) / DEG) * DEG) / dt;
            }
            acc = (s1 - s0) / dt;
        }
        om[i] = clamp(w, -max_turn, max_turn);
        ac[i] = clamp(acc, -max_accel, max_accel);
    }
}

/// One pass of [`fit_track`]: the smoothed residual from a reference turning at `ref_om` (rad/s)
/// and accelerating at `ref_ac` (m/s²), each given per sample and linear between them, integrated
/// back from the newest sample (from the report's velocity `rx`/`ry` on the `first` pass, or the
/// previous pass's). `weighted`: the samples take `sc.weights`.
pub fn fit_around(tr: &mut Track, sc: &mut Scratch, weighted: bool, reported_only: bool, rx: f64, ry: f64, first: bool) {
    let n = tr.n;
    if n == 0 {
        return;
    }
    let Scratch {
        spline,
        weights,
        ref_x,
        ref_y,
        ref_vx,
        ref_vy,
        ref_om,
        ref_ac,
        res_x,
        res_y,
        res_fx,
        res_fy,
        res_dx,
        res_dy,
        ..
    } = sc;
    let mut plain = true;
    let mut i = 0;
    while i < n && plain {
        plain = ref_om[i] == 0.0 && ref_ac[i] == 0.0;
        i += 1;
    }
    let t_end = tr.ts[n - 1];
    let mut v_end = 0.0;
    let mut h_end = 0.0;
    if !plain && first {
        // the reported velocity, carried on to the newest sample
        let since = clamp((t_end - tr.report_ms) / 1000.0, 0.0, TURN_HORIZON_S);
        v_end = js_max(0.0, rx.hypot(ry) + ref_ac[n - 1] * js_min(since, ACCEL_HORIZON_S));
        h_end = tr.track_deg * DEG + ref_om[n - 1] * since;
    } else if !plain {
        v_end = tr.mx[n - 1].hypot(tr.my[n - 1]);
        h_end = tr.mx[n - 1].atan2(tr.my[n - 1]);
    }
    for buf in [&mut *ref_x, &mut *ref_y, &mut *ref_vx, &mut *ref_vy, &mut *res_x, &mut *res_y] {
        buf.resize(n, 0.0);
    }
    let mut x = 0.0;
    let mut y = 0.0;
    let mut h = h_end;
    let mut speed = v_end;
    ref_x[n - 1] = 0.0;
    ref_y[n - 1] = 0.0;
    ref_vx[n - 1] = v_end * h_end.sin();
    ref_vy[n - 1] = v_end * h_end.cos();
    for i in (0..n - 1).rev() {
        if plain {
            ref_x[i] = 0.0;
            ref_y[i] = 0.0;
            ref_vx[i] = 0.0;
            ref_vy[i] = 0.0;
            continue;
        }
        // back from sample i+1 to sample i in steps of at most a second, at the rates between them
        let span = (tr.ts[i + 1] - tr.ts[i]) / 1000.0;
        let steps = js_max(1.0, span.ceil());
        let da = span / steps;
        let mut k = 0.0;
        while k < steps {
            let u = 1.0 - (k + 0.5) / steps; // from sample i (0) to i+1 (1)
            let w = ref_om[i] + (ref_om[i + 1] - ref_om[i]) * u;
            let acc = ref_ac[i] + (ref_ac[i + 1] - ref_ac[i]) * u;
            let h_mid = h - (w * da) / 2.0;
            let v_mid = js_max(0.0, speed - (acc * da) / 2.0);
            x -= v_mid * h_mid.sin() * da;
            y -= v_mid * h_mid.cos() * da;
            h -= w * da;
            speed = js_max(0.0, speed - acc * da);
            k += 1.0;
        }
        ref_x[i] = x;
        ref_y[i] = y;
        ref_vx[i] = speed * h.sin();
        ref_vy[i] = speed * h.cos();
    }
    for i in 0..n {
        res_x[i] = tr.xs[i] - ref_x[i];
        res_y[i] = tr.ys[i] - ref_y[i];
    }
    for buf in [&mut *res_fx, &mut *res_fy, &mut *res_dx, &mut *res_dy] {
        buf.resize(n, 0.0);
    }
    smooth_spline(
        spline,
        &tr.ts[..n],
        if weighted { Some(&weights[..n]) } else { None },
        SMOOTH_LAMBDA,
        Series { y: &res_x[..], f: &mut res_fx[..], d: &mut res_dx[..] },
        Some(Series { y: &res_y[..], f: &mut res_fy[..], d: &mut res_dy[..] }),
        EndSlope {
            w: if reported_only { REPORTED_SLOPE_WEIGHT } else { 0.0 },
            a: rx - ref_vx[n - 1],
            b: ry - ref_vy[n - 1],
        },
    );
    for i in 0..n {
        tr.fx[i] = res_fx[i] + ref_x[i];
        tr.fy[i] = res_fy[i] + ref_y[i];
        tr.mx[i] = res_dx[i] + ref_vx[i];
        tr.my[i] = res_dy[i] + ref_vy[i];
    }
}

/// The smoothed path's turn rate at each sample (`om`, rad/s, for the bank): its heading's change
/// over ±TURN_WINDOW_MS, which averages out what jitter is left.
pub fn turn_nodes(tr: &mut Track) {
    let n = tr.n;
    tr.om.resize(n, 0.0);
    if n == 0 {
        return;
    }
    let first = tr.ts[0];
    let newest = tr.ts[n - 1];
    let max = MAX_TURN_DEG_S * DEG;
    for i in 0..n {
        let a = js_max(first, tr.ts[i] - TURN_WINDOW_MS);
        let b = js_min(newest, tr.ts[i] + TURN_WINDOW_MS);
        let mut w = 0.0;
        if b - a >= 500.0 {
            let ha = hermite(tr, a);
            let hb = hermite(tr, b);
            if ha.vx.hypot(ha.vy) > HEADING_MIN_SPEED_MS && hb.vx.hypot(hb.vy) > HEADING_MIN_SPEED_MS {
                w = (wrap180((hb.vx.atan2(hb.vy) - ha.vx.atan2(ha.vy)) / DEG) * DEG) / ((b - a) / 1000.0);
            }
        }
        tr.om[i] = clamp(w, -max, max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;
    use crate::motion::aircraft::samples::{self, merge_report};

    const T0: f64 = 1_760_000_000_000.0;

    fn close_all(actual: &[f64], expected: &[f64], digits: i32) {
        assert_eq!(actual.len(), expected.len(), "{actual:?} against {expected:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert_close!(*a, *e, digits);
        }
    }

    fn close_hermite(h: Hermite, expected: [f64; 6]) {
        assert_close!(h.x, expected[0], 9);
        assert_close!(h.y, expected[1], 9);
        assert_close!(h.vx, expected[2], 9);
        assert_close!(h.vy, expected[3], 9);
        assert_eq!(h.i as f64, expected[4]);
        assert_close!(h.u, expected[5], 12);
    }

    /// A 2 deg/s right turn at 220 kt from heading north at 45N 10E (the TypeScript tests'
    /// `circle(220, 2)`), its first report at T0 with the buffer 2-8 s back, merged and fitted.
    fn turning(sc: &mut samples::Scratch, fs: &mut Scratch) -> Track {
        let mut tr = Track::new(1);
        tr.phase = Phase::Cruise;
        tr.speed_kt = 220.0;
        tr.track_deg = 0.0;
        tr.turn_rate = 2.0;
        tr.report_ms = T0;
        let positions = [[203, -10, 2_000], [406, -40, 4_000], [606, -90, 6_000], [804, -160, 8_000]];
        let own = merge_report(&mut tr, sc, &positions, T0, 45.0, 10.0, false);
        assert_eq!(own, Some(true));
        fit_track(&mut tr, fs, true);
        turn_nodes(&mut tr);
        tr
    }

    #[test]
    fn the_reference_turn_fades_out_between_twelve_and_twenty_four_seconds_back() {
        assert_eq!(reference_weight(0.0), 1.0);
        assert_eq!(reference_weight(12.0), 1.0);
        assert_eq!(reference_weight(18.0), 0.5);
        assert_eq!(reference_weight(24.0), 0.0);
        assert_eq!(reference_weight(30.0), 0.0);
    }

    #[test]
    fn a_turn_is_fitted_around_a_turning_reference_as_the_typescript_does() {
        // bun: the same samples through mergeReport, fitTrack(tr, true), turnNodes, hermite
        let mut sc = samples::Scratch::default();
        let mut fs = Scratch::default();
        let mut tr = turning(&mut sc, &mut fs);
        close_all(&tr.fx, &[125.70075178790175, 70.86550308369172, 31.51603572205746, 7.84640935769247, -0.04863995625778843], 9);
        close_all(&tr.fy, &[-893.8533906446868, -674.1322734751933, -451.168908202325, -226.0325110246439, 0.16151439326509565], 9);
        close_all(&tr.mx, &[-31.24103027973613, -23.5658019508122, -15.765692269294586, -7.895287639668812, 0.0022261096489915], 9);
        close_all(&tr.my, &[108.85996079050639, 110.75568636219104, 112.11040873945527, 112.91829721749794, 113.17776045918058], 9);
        close_all(&tr.om, &[0.034913204818672584, 0.03494062837980136, 0.034959921017569444, 0.03493249893880152, 0.03491320778339528], 12);
        assert_eq!(tr.r_ts, tr.ts);
        close_all(&tr.r_om, &[0.03494062837980136, 0.03494434895127016, 0.034936563659301444, 0.03494434993951106, 0.03493249893880152], 12);
        close_all(&tr.r_ac, &[-0.010147405965572176, -0.010021150882482269, -0.009543471528203895, -0.009542759155702166, -0.008939537090835614], 12);
        close_hermite(hermite(&tr, T0 - 3_000.0), [17.71362138246852, -338.8026817329951, -11.836974796032894, 112.59512139402251, 2.0, 0.5]);
        // 2 deg/s, as flown, at every sample
        for &om in &tr.om {
            assert_close!(om / DEG, 2.0, 1);
        }

        // the next report, 8 s on: the previous fit's rates seed this one where it covered the same moments
        tr.track_deg = 16.0;
        tr.report_ms = T0 + 8_000.0;
        let positions = [[197, 70, 2_000], [398, 120, 4_000], [600, 150, 6_000], [804, 160, 8_000], [1_007, 150, 10_000]];
        let own = merge_report(&mut tr, &mut sc, &positions, T0 + 8_000.0, 45.00803723240071, 10.00159743815269, false);
        assert_eq!(own, Some(true));
        assert_eq!(sc.report_from_ms, T0 - 2_000.0);
        assert_eq!(tr.n, 9);
        fit_track(&mut tr, &mut fs, true);
        turn_nodes(&mut tr);
        let fx = [
            0.15070899061818338,
            -54.75849436459397,
            -94.16805405875358,
            -117.89400625751192,
            -125.82543709635398,
            -117.93652218310257,
            -94.26915010628282,
            -54.932433096522544,
            -0.1339219659114176,
        ];
        let fy = [
            -1787.5706581981558,
            -1567.8737354606915,
            -1344.9547815340595,
            -1119.8586704294235,
            -893.6622958811428,
            -667.4424138689749,
            -442.32491788138407,
            -219.41136802674396,
            0.18107681320200916,
        ];
        let mx = [
            -31.285173541169165,
            -23.597645354667634,
            -15.795314829954766,
            -7.919725320136102,
            -0.009546937372511116,
            7.89466942221631,
            15.763034292325068,
            23.554545117013944,
            31.21448370368936,
        ];
        let my = [
            108.85425064110468,
            110.73745597452064,
            112.0859661015618,
            112.90706607603015,
            113.19009609409966,
            112.9213454427235,
            112.09512265691305,
            110.71070755465651,
            108.7874304815233,
        ];
        let om = [
            0.034953174926292366,
            0.03496545710096021,
            0.03498149311273697,
            0.03497880038968734,
            0.0349571179923737,
            0.03494755596386999,
            0.03495810906034118,
            0.0349295707496918,
            0.034896153151222624,
        ];
        let r_om = [
            0.03496545710096021,
            0.034972053717255105,
            0.03497212874532378,
            0.034969305552555334,
            0.034963178176778666,
            0.03495761352635744,
            0.034938563356780895,
            0.034937457090635,
            0.0349295707496918,
        ];
        let r_ac = [
            -0.016840265419098444,
            -0.012720031327826101,
            -0.008838572230713382,
            -0.0033549038920064334,
            0.0005709171255432466,
            0.0005244348847739388,
            -0.0016283949929949415,
            -0.0033182824719399187,
            -0.005235503279404696,
        ];
        close_all(&tr.fx, &fx, 9);
        close_all(&tr.fy, &fy, 9);
        close_all(&tr.mx, &mx, 9);
        close_all(&tr.my, &my, 9);
        close_all(&tr.om, &om, 12);
        assert_eq!(tr.r_ts, tr.ts);
        close_all(&tr.r_om, &r_om, 12);
        close_all(&tr.r_ac, &r_ac, 12);
        close_hermite(hermite(&tr, T0 + 1_234.0), [-122.82562015797208, -754.0151235360003, 4.870043134142086, 113.10115397370812, 4.0, 0.617]);
        // the rates on the final path: covering 2 s or more everywhere, so `om0`/`ac0` are not used
        let (mut om, mut ac) = (Vec::new(), Vec::new());
        local_rates(&tr, &mut om, &mut ac, &[0.01; 9], &[0.2; 9]);
        close_all(&om, &r_om, 12);
        close_all(&ac, &r_ac, 12);
    }

    #[test]
    fn hermite_pieces_meet_the_nodes_and_follow_the_slopes() {
        let mut sc = samples::Scratch::default();
        let mut fs = Scratch::default();
        let tr = turning(&mut sc, &mut fs);
        for i in 0..tr.n {
            let h = hermite(&tr, tr.ts[i]);
            assert_close!(h.x, tr.fx[i], 9);
            assert_close!(h.y, tr.fy[i], 9);
            assert_close!(h.vx, tr.mx[i], 9);
            assert_close!(h.vy, tr.my[i], 9);
        }
        // the derivative is the velocity
        let (a, b) = (hermite(&tr, T0 - 5_000.0), hermite(&tr, T0 - 4_999.0));
        assert_close!((b.x - a.x) / 0.001, a.vx, 2);
        assert_close!((b.y - a.y) / 0.001, a.vy, 2);
        // pieces are searched from the newest back; before the first, the first piece goes on
        assert_eq!((hermite(&tr, T0).i, hermite(&tr, T0 - 2_000.0).i, hermite(&tr, T0 - 2_001.0).i), (3, 3, 2));
        assert_eq!(hermite(&tr, T0 - 9_000.0).i, 0);
        // fewer than two samples: nothing to evaluate
        let mut one = Track::new(2);
        one.n = 1;
        one.ts = vec![T0];
        assert!(hermite(&one, T0).x.is_nan());
    }

    #[test]
    fn one_sample_takes_the_reported_velocity_and_none_clears_the_curve() {
        let mut fs = Scratch::default();
        let mut tr = Track::new(1);
        tr.n = 1;
        tr.ts = vec![T0];
        tr.xs = vec![3.0];
        tr.ys = vec![-4.0];
        tr.speed_kt = 100.0;
        tr.track_deg = 90.0;
        tr.r_ts = vec![1.0, 2.0];
        tr.r_om = vec![0.0, 0.0];
        tr.r_ac = vec![0.0, 0.0];
        fit_track(&mut tr, &mut fs, true);
        assert_eq!((tr.fx.clone(), tr.fy.clone()), (vec![3.0], vec![-4.0]));
        assert_close!(tr.mx[0], 100.0 * KNOTS_TO_MS, 12);
        assert_close!(tr.my[0], 0.0, 12);
        // the last fit's rates are left as they were
        assert_eq!(tr.r_ts.len(), 2);
        turn_nodes(&mut tr);
        assert_eq!(tr.om, [0.0]);
        // parked: no velocity
        tr.phase = Phase::Parked;
        fit_track(&mut tr, &mut fs, true);
        assert_eq!((tr.mx[0], tr.my[0]), (0.0, 0.0));
        // no samples: no curve, no rates
        tr.n = 0;
        tr.ts.clear();
        fit_track(&mut tr, &mut fs, true);
        turn_nodes(&mut tr);
        assert!(tr.fx.is_empty() && tr.my.is_empty() && tr.om.is_empty());
        assert!(tr.r_ts.is_empty() && tr.r_om.is_empty() && tr.r_ac.is_empty());
    }

    #[test]
    fn a_parked_aircraft_gets_a_plain_spline_and_no_rates() {
        // bun: a parked track's samples through fitTrack(tr, false) and turnNodes
        let mut fs = Scratch::default();
        let mut tr = Track::new(1);
        tr.phase = Phase::Parked;
        tr.speed_kt = 2.0;
        tr.turn_rate = 3.0;
        tr.accel = 1.0;
        tr.n = 4;
        tr.ts = vec![T0, T0 + 2_000.0, T0 + 4_000.0, T0 + 6_000.0];
        tr.xs = vec![0.4, -0.3, 0.2, 0.0];
        tr.ys = vec![-0.2, 0.5, 0.1, 0.0];
        fit_track(&mut tr, &mut fs, false);
        turn_nodes(&mut tr);
        close_all(&tr.fx, &FX_PARKED, 12);
        close_all(&tr.fy, &FY_PARKED, 12);
        close_all(&tr.mx, &MX_PARKED, 12);
        close_all(&tr.my, &MY_PARKED, 12);
        assert_eq!(tr.r_ts, tr.ts);
        assert_eq!((tr.r_om.clone(), tr.r_ac.clone()), (vec![0.0; 4], vec![0.0; 4]));
        // creeping at centimetres a second: no heading to turn
        assert_eq!(tr.om, [0.0; 4]);
    }

    /// Every report of the recorded flights in `landings.json` merged and fitted in turn, with
    /// the estimates the fit reads set simply from the report and its buffer (the model's own
    /// `update` is S1.19's): sums over the reports of the newest node's value and slope, a middle
    /// sample's turn rate and the path halfway through the samples, against the TypeScript's
    /// (bun: the same loop over `mergeReport`, `bufferTurnRate`, `bufferAcceleration`,
    /// `fitTrack`, `turnNodes`, `hermite`).
    #[test]
    fn the_recorded_flights_fit_as_in_the_typescript() {
        type Report = (f64, f64, f64, f64, f64, f64, u8, Vec<[i32; 3]>);
        #[derive(serde::Deserialize)]
        struct Flight {
            id: u32,
            reports: Vec<Report>,
        }
        #[derive(serde::Deserialize)]
        struct Landings {
            flights: Vec<Flight>,
        }
        let landings: Landings = serde_json::from_str(include_str!("../../../tests/data/landings.json")).unwrap();
        let expected: [[f64; 9]; 9] = [
            [538.0, -24.395696095789834, -35.17236822437758, -2426.8090987709443, -148.31574126824944, -0.6913997396616709, 34549.10902899625, -138.80308629488573, 0.0],
            [690.0, 9.07525422559965, -23.468229017084827, -91.14266937328803, -2556.0170225718603, -103.8744132004166, 1537.9991017462028, -2710.7430394541766, 0.0],
            [237.0, 19.721548010998923, 5.6793519019946785, 895.5807702045191, 17.88856574750607, -17.651413003971573, -12058.85471196273, 5.9883604871272516, 0.0],
            [678.0, 10.075432009838337, 13.993233573571871, 3151.1917483704124, 0.8390401384444797, 15.900693643936876, -45236.594589878165, 5.063742098160438, 0.0],
            [269.0, -7.93674527579986, -11.026067953934797, 1272.3439233683, 212.99101864852935, -59.56048526277095, -16640.56259040891, 61.80580691913441, 0.0],
            [249.0, -26.488202728548124, 9.75415715886063, 1497.3553845821086, 68.25707367786362, -30.1295014472045, -19774.701024448426, 3.1126260086052557, 0.0],
            [168.0, -6.295914024769923, -1.9848687654780055, -456.1791095559015, 264.18653345033687, 2.4515819941525985, 6188.600225479606, 343.23563997337595, 0.0],
            [551.0, -15.39214691972877, -8.299463418149273, -2276.2053647405437, -17.003989510488886, 0.0937201592136725, 31934.22957757129, -10.342762378821279, 0.0],
            [312.0, -29.227619297230298, -5.784399488471814, -1231.000720453863, -7.506745648826065, -0.6490365172846493, 16934.359355546443, -7.382602658184719, 0.0],
        ];
        assert_eq!(landings.flights.len(), expected.len());
        let mut sc = samples::Scratch::default();
        let mut fs = Scratch::default();
        for (f, want) in landings.flights.iter().zip(&expected) {
            let mut tr = Track::new(f.id);
            let mut d = [0.0; 9];
            for (t, lat, lon, _alt, speed, track, on_ground, positions) in &f.reports {
                tr.speed_kt = js_max(0.0, *speed);
                tr.track_deg = crate::num::wrap360(*track);
                tr.on_ground = *on_ground == 1;
                tr.phase = if !tr.on_ground {
                    Phase::Cruise
                } else if *speed < 3.0 {
                    Phase::Parked
                } else {
                    Phase::Taxi
                };
                tr.report_ms = *t;
                let Some(own) = merge_report(&mut tr, &mut sc, positions, *t, *lat, *lon, false) else {
                    d[8] += 1.0;
                    continue;
                };
                let turn = samples::buffer_turn_rate(&tr, sc.report_from_ms);
                let acc = samples::buffer_acceleration(&tr, sc.report_from_ms);
                tr.turn_rate = if turn.is_finite() { clamp(turn, -4.0, 4.0) } else { 0.0 };
                tr.accel = if acc.is_finite() { clamp(acc, -4.0, 4.0) } else { 0.0 };
                fit_track(&mut tr, &mut fs, own);
                turn_nodes(&mut tr);
                let n = tr.n;
                d[0] += n as f64;
                d[1] += tr.fx[n - 1];
                d[2] += tr.fy[n - 1];
                d[3] += tr.mx[n - 1];
                d[4] += tr.my[n - 1];
                d[5] += tr.om[n / 2] * 1000.0;
                if n >= 2 {
                    let h = hermite(&tr, (tr.ts[0] + tr.ts[n - 1]) / 2.0);
                    d[6] += h.x;
                    d[7] += h.vy;
                }
            }
            assert_eq!(d[0], want[0], "samples of flight {}", f.id);
            assert_eq!(d[8], want[8], "dropped reports of flight {}", f.id);
            for k in 1..8 {
                assert_close!(d[k], want[k], 6);
            }
        }
    }

    const FX_PARKED: [f64; 4] = [0.19971988795518042, 0.08655462184873826, 0.027731092436974247, -0.014005602240896198];
    const FY_PARKED: [f64; 4] = [0.04005602240896222, 0.12268907563025097, 0.1344537815126043, 0.10280112044817903];
    const MX_PARKED: [f64; 4] = [-0.0632586367880485, -0.04323062558356644, -0.02180205415499497, -0.02040149393090535];
    const MY_PARKED: [f64; 4] = [0.04931839402427643, 0.025312791783380195, -0.00897292250233403, -0.01925303454715195];
}
