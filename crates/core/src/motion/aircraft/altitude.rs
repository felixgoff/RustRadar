//! Heights in `motion.ts`: pressure altitude and its QNH correction (`trueAltitude`, `qnhFrom`,
//! `validQnh`, `correctAlt`, `corrected`, `:394-406`, `:3068-3074`), the flare curve (`flare`,
//! `flareSlope`, `:436-448`), the vertical rate's weight past a report (`verticalWeight`,
//! `verticalIntegral`, `verticalIntegralInverse`, `:594-616`), the airborne altitude samples and
//! their smoothed curve (`addAltitude`, `fitAltitude`, `altitudeAt`, `:3020-3122`) and the field
//! elevation removed when drawing (`fieldRemoval`, `:2360-2378`). Doc 05 §10.3, §10.7.

use super::consts::*;
use super::spline::{EndSlope, Series, smooth_spline};
use super::track::Track;
use super::{distance_km, field_fade};
use crate::trains::{js_max, js_min};

/// True altitude (ft) of a pressure altitude at a QNH (hPa).
pub fn true_altitude(pressure_alt_ft: f64, qnh: f64) -> f64 {
    ISA_H0_FT - (ISA_H0_FT - pressure_alt_ft) * (STD_HPA / qnh).powf(ISA_EXP)
}

/// The QNH (hPa) at which a pressure altitude reads a true altitude (ft).
pub fn qnh_from(pressure_alt_ft: f64, true_alt_ft: f64) -> f64 {
    let ratio = (ISA_H0_FT - true_alt_ft) / (ISA_H0_FT - pressure_alt_ft);
    STD_HPA / ratio.powf(1.0 / ISA_EXP)
}

/// A believable QNH: finite and within 940..1060 hPa (`undefined` in the TypeScript is NaN here).
pub fn valid_qnh(q: f64) -> bool {
    q.is_finite() && (MIN_QNH_HPA..=MAX_QNH_HPA).contains(&q)
}

/// A pressure altitude (ft) corrected to true altitude at `qnh` with weight `w` (none without a QNH).
pub fn correct_alt(raw: f64, w: f64, qnh: f64) -> f64 {
    if w > 0.0 && valid_qnh(qnh) { raw + w * (true_altitude(raw, qnh) - raw) } else { raw }
}

/// A pressure altitude (ft) corrected with the track's QNH and weight `w`.
pub fn corrected(tr: &Track, raw: f64, w: f64) -> f64 {
    correct_alt(raw, w, tr.qnh)
}

/// Height to draw (ft above the field) for a raw height `x`: x above the flare, easing to 0 at
/// FLARE_FLOOR_FT.
pub fn flare(x: f64) -> f64 {
    if x >= FLARE_HEIGHT_FT {
        return x;
    }
    let u = x - FLARE_FLOOR_FT;
    if u <= 0.0 { 0.0 } else { FLARE_A * u * u + TOUCHDOWN_SINK * u }
}

/// Slope of [`flare`] at `x`.
pub fn flare_slope(x: f64) -> f64 {
    if x >= FLARE_HEIGHT_FT {
        return 1.0;
    }
    let u = x - FLARE_FLOOR_FT;
    if u <= 0.0 { 0.0 } else { 2.0 * FLARE_A * u + TOUCHDOWN_SINK }
}

/// Weight of the vertical rate `t` seconds past a report: 1, then fading to 0 (level off).
pub fn vertical_weight(t: f64) -> f64 {
    if t < -BACKWARD_LIMIT_S {
        return 0.0;
    }
    if t <= VERTICAL_FADE_START_S {
        return 1.0;
    }
    js_max(0.0, 1.0 - (t - VERTICAL_FADE_START_S) / (VERTICAL_HORIZON_S - VERTICAL_FADE_START_S))
}

/// Integral of [`vertical_weight`] over [0, t]: seconds of full vertical rate applied.
pub fn vertical_integral(t: f64) -> f64 {
    if t <= VERTICAL_FADE_START_S {
        return js_max(t, -BACKWARD_LIMIT_S);
    }
    let span = VERTICAL_HORIZON_S - VERTICAL_FADE_START_S;
    let u = js_min(t, VERTICAL_HORIZON_S) - VERTICAL_FADE_START_S;
    VERTICAL_FADE_START_S + u - (u * u) / (2.0 * span)
}

/// Inverse of [`vertical_integral`]: when `applied` seconds' worth is reached, or NaN if never.
pub fn vertical_integral_inverse(applied: f64) -> f64 {
    if applied <= VERTICAL_FADE_START_S {
        return js_max(0.0, applied);
    }
    let span = VERTICAL_HORIZON_S - VERTICAL_FADE_START_S;
    let rest = applied - VERTICAL_FADE_START_S;
    let disc = span * span - 2.0 * span * rest;
    if disc < 0.0 { f64::NAN } else { VERTICAL_FADE_START_S + span - disc.sqrt() }
}

/// An airborne pressure altitude (ft) at `t`, corrected with weight `w` (the field fade); a sample
/// within a second of the newest replaces it, an older one is dropped. The first one after the
/// aircraft was on the ground starts a new flight segment: the samples of the approach before it
/// are kept until then, so the drawn descent carries on into the touchdown rather than restarting
/// from the latest report when the ground report comes in.
pub fn add_altitude(tr: &mut Track, t: f64, raw: f64, w: f64) {
    let Track { ats, aps, aws, ground_ms, .. } = tr;
    let mut n = ats.len();
    if n > 0 && ats[n - 1] < *ground_ms {
        ats.clear();
        aps.clear();
        aws.clear();
        n = 0;
    }
    if n > 0 && t < ats[n - 1] - COINCIDENT_MS {
        return;
    }
    // a change no aircraft can fly (beyond 25 ft steps) is a jump in the data: start over
    if n > 0 && (raw - aps[n - 1]).abs() > ALT_STEP_FT {
        let dt_min = js_max(t - ats[n - 1], COINCIDENT_MS) / 60_000.0;
        if (raw - aps[n - 1]).abs() / dt_min > 1.25 * MAX_VRATE_FPM {
            ats.clear();
            aps.clear();
            aws.clear();
            n = 0;
        }
    }
    if n > 0 && (t - ats[n - 1]).abs() < COINCIDENT_MS {
        ats[n - 1] = js_max(t, ats[n - 1]);
        aps[n - 1] = raw;
        aws[n - 1] = w;
    } else {
        ats.push(t);
        aps.push(raw);
        aws.push(w);
    }
    let mut drop = 0;
    while ats.len() - drop > 1 && ats[drop] < t - ALT_WINDOW_MS {
        drop += 1;
    }
    ats.drain(..drop);
    aps.drain(..drop);
    aws.drain(..drop);
}

/// The smoothed altitude through the airborne altitude samples, corrected with the track's QNH
/// (`avs`, then the curve's values `afv` and slopes `afd`, ft/s). One sample takes the slope of
/// the vertical rate. `spline` is the solver's scratch buffer.
pub fn fit_altitude(tr: &mut Track, spline: &mut Vec<f64>) {
    let n = tr.ats.len();
    tr.avs.resize(n, 0.0);
    tr.afv.resize(n, 0.0);
    tr.afd.resize(n, 0.0);
    if n == 0 {
        return;
    }
    for i in 0..n {
        tr.avs[i] = correct_alt(tr.aps[i], tr.aws[i], tr.qnh);
    }
    if n == 1 {
        tr.afv[0] = tr.avs[0];
        tr.afd[0] = tr.vrate / 60.0;
        return;
    }
    let Track { ats, avs, afv, afd, .. } = tr;
    smooth_spline(
        spline,
        ats,
        None,
        ALT_SMOOTH_LAMBDA,
        Series { y: avs, f: afv, d: afd },
        None,
        EndSlope::NONE,
    );
}

/// The smoothed altitude at `t` (inside its samples, at least two of them): value (ft) and slope
/// (ft/s), the TypeScript's `hx` and `hvx`. NaN without two samples.
pub fn altitude_at(tr: &Track, t: f64) -> (f64, f64) {
    let ts = &tr.ats;
    if ts.len() < 2 {
        return (f64::NAN, f64::NAN);
    }
    let mut i = ts.len() - 2;
    while i > 0 && t < ts[i] {
        i -= 1;
    }
    let h = (ts[i + 1] - ts[i]) / 1000.0;
    let u = (t - ts[i]) / 1000.0 / h;
    let u2 = u * u;
    let u3 = u2 * u;
    let f = &tr.afv;
    let d = &tr.afd;
    let value = (2.0 * u3 - 3.0 * u2 + 1.0) * f[i]
        + (u3 - 2.0 * u2 + u) * h * d[i]
        + (3.0 * u2 - 2.0 * u3) * f[i + 1]
        + (u3 - u2) * h * d[i + 1];
    let slope = ((6.0 * u2 - 6.0 * u) * (f[i] - f[i + 1])) / h + (3.0 * u2 - 4.0 * u + 1.0) * d[i] + (3.0 * u2 - 2.0 * u) * d[i + 1];
    (value, slope)
}

/// Feet of field elevation to remove at a point: the track's field, else the nearer of origin and
/// destination.
pub fn field_removal(tr: &Track, lat: f64, lon: f64) -> f64 {
    if let Some(field) = &tr.field {
        let fade = field_fade(distance_km(lat, lon, field.lat, field.lon));
        if fade > 0.0 {
            return tr.field_elev * fade;
        }
    }
    let mut best = 0.0;
    let mut best_km = FADE_ZERO_KM;
    for ap in [&tr.origin_ap, &tr.dest_ap].into_iter().flatten() {
        let km = distance_km(lat, lon, ap.lat, ap.lon);
        if km < best_km {
            best_km = km;
            best = ap.alt.unwrap_or(0) as f64 * field_fade(km);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::assert_close;
    use crate::model::Airport;

    #[test]
    fn pressure_altitude_and_qnh_invert_each_other() {
        // bun: trueAltitude(3000, 1003.25), trueAltitude(3000, 1023.25), qnhFrom(-200, 0), qnhFrom(250, 0)
        assert_eq!(true_altitude(3_000.0, 1003.25), 2730.946863930527);
        assert_eq!(true_altitude(3_000.0, 1023.25), 3265.9109837632277);
        assert_eq!(qnh_from(-200.0, 0.0), 1020.5946787978459);
        assert_eq!(qnh_from(250.0, 0.0), 1004.1293980836144);
        assert_eq!(true_altitude(5_000.0, STD_HPA), 5_000.0);
        for (p, q) in [(0.0, 1000.0), (1_200.0, 1030.0), (-150.0, 960.0)] {
            assert_close!(qnh_from(p, true_altitude(p, q)), q, 9);
        }
        assert!(valid_qnh(940.0) && valid_qnh(1_060.0) && valid_qnh(1013.25));
        assert!(!valid_qnh(939.9) && !valid_qnh(1_060.1) && !valid_qnh(f64::NAN) && !valid_qnh(f64::INFINITY));
        // corrected only with a weight and a valid QNH
        assert_eq!(correct_alt(3_000.0, 0.0, 1003.25), 3_000.0);
        assert_eq!(correct_alt(3_000.0, 1.0, f64::NAN), 3_000.0);
        assert_eq!(correct_alt(3_000.0, 0.5, 1003.25), 3_000.0 + 0.5 * (2730.946863930527 - 3_000.0));
        let mut tr = Track::new(1);
        tr.qnh = 1023.25;
        assert_eq!(corrected(&tr, 3_000.0, 1.0), 3265.9109837632277);
    }

    #[test]
    fn the_flare_meets_the_ground_at_a_fifth_of_the_rate() {
        assert_eq!(flare(100.0), 100.0);
        assert_eq!(flare(50.0), 50.0);
        assert_eq!(flare(FLARE_FLOOR_FT), 0.0);
        assert_eq!(flare(-100.0), 0.0);
        assert_eq!(flare_slope(60.0), 1.0);
        assert_eq!(flare_slope(-40.0), 0.0);
        // continuous in value and slope at the top, a fifth of the slope at the floor
        assert_close!(flare(50.0 - 1e-9), 50.0, 6);
        assert_close!(flare_slope(50.0 - 1e-9), 1.0, 6);
        assert_close!(flare_slope(FLARE_FLOOR_FT + 1e-9), TOUCHDOWN_SINK, 6);
        // bun: flare(0), flareSlope(0), flare(20)
        assert_eq!(flare(0.0), 12.000000000000004);
        assert_eq!(flare_slope(0.0), 0.52);
        assert_eq!(flare(20.0), 24.320000000000007);
        // never through the ground, and rising with the raw height
        let mut last = 0.0;
        for k in -100..=200 {
            let v = flare(k as f64 * 0.5);
            assert!(v >= last);
            last = v;
        }
    }

    #[test]
    fn the_vertical_rate_fades_out_between_sixty_and_ninety_seconds() {
        assert_eq!(vertical_weight(-31.0), 0.0);
        assert_eq!(vertical_weight(-30.0), 1.0);
        assert_eq!(vertical_weight(60.0), 1.0);
        assert_eq!(vertical_weight(75.0), 0.5);
        assert_eq!(vertical_weight(100.0), 0.0);
        assert_eq!(vertical_integral(-40.0), -30.0);
        assert_eq!(vertical_integral(30.0), 30.0);
        assert_eq!(vertical_integral(75.0), 71.25);
        assert_eq!(vertical_integral(90.0), 75.0);
        assert_eq!(vertical_integral(500.0), 75.0);
        // the inverse, NaN past what is ever applied
        assert_eq!(vertical_integral_inverse(-5.0), 0.0);
        assert_eq!(vertical_integral_inverse(40.0), 40.0);
        assert_eq!(vertical_integral_inverse(71.25), 75.0);
        assert_eq!(vertical_integral_inverse(75.0), 90.0);
        assert!(vertical_integral_inverse(75.1).is_nan());
        for t in [0.0, 12.5, 61.0, 70.0, 89.0] {
            assert_close!(vertical_integral_inverse(vertical_integral(t)), t, 9);
        }
    }

    fn altitudes(tr: &Track) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
        (tr.ats.clone(), tr.aps.clone(), tr.aws.clone())
    }

    #[test]
    fn altitude_samples_keep_a_minute_of_one_flight_segment() {
        let mut tr = Track::new(1);
        add_altitude(&mut tr, 10_000.0, 3_000.0, 1.0);
        add_altitude(&mut tr, 15_000.0, 2_950.0, 0.9);
        // out of order by more than a second: ignored
        add_altitude(&mut tr, 13_000.0, 2_975.0, 1.0);
        // within a second of the newest: replaces it, keeping the later time
        add_altitude(&mut tr, 14_500.0, 2_940.0, 0.8);
        assert_eq!(altitudes(&tr), (vec![10_000.0, 15_000.0], vec![3_000.0, 2_940.0], vec![1.0, 0.8]));
        // 400 ft in 5 s is 4 800 fpm: a real change
        add_altitude(&mut tr, 20_000.0, 2_540.0, 1.0);
        assert_eq!(tr.ats.len(), 3);
        // 1 000 ft in a second is no aircraft: a jump, the samples start over
        add_altitude(&mut tr, 21_000.0, 3_540.0, 1.0);
        assert_eq!(altitudes(&tr), (vec![21_000.0], vec![3_540.0], vec![1.0]));
        // a minute is kept
        for k in 1..=13 {
            add_altitude(&mut tr, 21_000.0 + 5_000.0 * k as f64, 3_540.0, 1.0);
        }
        assert_eq!(tr.ats.first(), Some(&26_000.0));
        assert_eq!(tr.ats.len(), 13);
        // a ground report since the newest starts a new segment
        tr.ground_ms = 90_000.0;
        add_altitude(&mut tr, 95_000.0, 200.0, 1.0);
        assert_eq!(altitudes(&tr), (vec![95_000.0], vec![200.0], vec![1.0]));
    }

    #[test]
    fn the_altitude_curve_is_smoothed_and_corrected() {
        let mut buf = Vec::new();
        let mut tr = Track::new(1);
        fit_altitude(&mut tr, &mut buf);
        assert!(tr.afv.is_empty());
        // one sample: its value and the vertical rate
        tr.vrate = -600.0;
        tr.qnh = 1003.25;
        add_altitude(&mut tr, 0.0, 3_000.0, 1.0);
        fit_altitude(&mut tr, &mut buf);
        assert_eq!(tr.avs, vec![2730.946863930527]);
        assert_eq!((tr.afv.clone(), tr.afd.clone()), (vec![2730.946863930527], vec![-10.0]));
        // a steady descent stays as it is: a line costs the spline nothing
        tr.qnh = f64::NAN;
        tr.ats.clear();
        tr.aps.clear();
        tr.aws.clear();
        for k in 0..6 {
            add_altitude(&mut tr, 4_000.0 * k as f64, 3_000.0 - 40.0 * k as f64, 1.0);
        }
        fit_altitude(&mut tr, &mut buf);
        for k in 0..6 {
            assert_close!(tr.afv[k], 3_000.0 - 40.0 * k as f64, 9);
            assert_close!(tr.afd[k], -10.0, 9);
        }
        let (v, s) = altitude_at(&tr, 6_000.0);
        assert_close!(v, 2_940.0, 9);
        assert_close!(s, -10.0, 9);
        assert!(altitude_at(&Track::new(2), 0.0).0.is_nan());
    }

    #[test]
    fn the_altitude_curve_matches_the_typescript() {
        // bun: a Track with these samples through addAltitude, vrate 0, qnh NaN, then fitAltitude
        // and altitudeAt(tr, 9000)
        let mut buf = Vec::new();
        let mut tr = Track::new(1);
        for (t, a) in [(0.0, 5_000.0), (3_000.0, 4_975.0), (8_000.0, 4_975.0), (11_000.0, 4_900.0), (19_000.0, 4_825.0)] {
            add_altitude(&mut tr, t, a, 1.0);
        }
        fit_altitude(&mut tr, &mut buf);
        let afv = [5006.145453341516, 4983.3878857089785, 4942.331535334846, 4913.5479158107355, 4829.587209804095];
        let afd = [-7.539764977451341, -7.678037677635075, -9.047280368902648, -10.0057858717268, -10.73973944038164];
        for k in 0..5 {
            assert_close!(tr.afv[k], afv[k], 9);
            assert_close!(tr.afd[k], afd[k], 9);
        }
        let (v, s) = altitude_at(&tr, 9_000.0);
        assert_close!(v, 4933.071609932429, 9);
        assert_close!(s, -9.457457831251375, 9);
    }

    #[test]
    fn field_removal_fades_with_distance_from_the_field_or_the_route() {
        let ap = |lat: f64, alt: Option<i64>| {
            Arc::new(Airport {
                id: 1,
                name: String::new(),
                iata: String::new(),
                icao: String::new(),
                city: String::new(),
                country: String::new(),
                lat,
                lon: 10.0,
                alt,
                size: 0,
                timezone: String::new(),
            })
        };
        let mut tr = Track::new(1);
        assert_eq!(field_removal(&tr, 45.0, 10.0), 0.0);
        tr.field = Some(ap(45.0, Some(1_000)));
        tr.field_elev = 1_000.0;
        assert_eq!(field_removal(&tr, 45.0, 10.0), 1_000.0);
        // 45 km out: half; beyond 60 km the field is no longer removed
        let lat45 = 45.0 + 45.0 / KM_PER_DEG;
        assert_close!(field_removal(&tr, lat45, 10.0), 500.0, 9);
        assert_eq!(field_removal(&tr, 46.0, 10.0), 0.0);
        // then the nearer of origin and destination, faded likewise (an unknown elevation is 0)
        tr.origin_ap = Some(ap(46.1, Some(300)));
        tr.dest_ap = Some(ap(45.9, None));
        assert_eq!(field_removal(&tr, 46.05, 10.0), 300.0);
        assert_eq!(field_removal(&tr, 45.95, 10.0), 0.0);
    }
}
