//! The cubic smoothing spline of `motion.ts` (`smoothSpline`, `:2645-2788`; doc 05 §10.7),
//! transliterated literally: the tests check it to centimetres, and the golden poses to 1e-6.

/// One series to smooth: the samples `y`, and where the fit's values `f` and slopes `d` (per
/// second) go. All three have one entry per sample time.
pub struct Series<'a> {
    pub y: &'a [f64],
    pub f: &'a mut [f64],
    pub d: &'a mut [f64],
}

/// The optional pull of the last slope towards a value: weight `w` (0 for none), the value for
/// series `a` and for series `b`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EndSlope {
    pub w: f64,
    pub a: f64,
    pub b: f64,
}

impl EndSlope {
    pub const NONE: EndSlope = EndSlope { w: 0.0, a: 0.0, b: 0.0 };
}

/// Numbers per sample in the solver's buffer: the 2×2 diagonal block (3, symmetric), the 2×2
/// block above it (4), and the two series' right-hand sides (2 each).
const S: usize = 11;

/// Cubic smoothing spline. Finds the piecewise cubic f, in Hermite form (node values `f` and
/// slopes `d` at the sample times `ts`, ms), that minimises
///
/// ```text
/// Σ w_i (f(t_i) - y_i)²  +  λ ∫ f''(t)² dt                  (t in seconds)
/// [ + end.w (f'(t_last) - end.a)² ]
/// ```
///
/// i.e. the smoothest curve that stays close to the samples: it approximates them instead of
/// passing through each noisy one. The optimum is the natural smoothing spline, which is C2. λ
/// sets how far it averages: about (λ / (w · samples per second))^¼ seconds either side, so sparse
/// samples are followed closely and dense noisy ones averaged. The normal equations are block
/// tridiagonal (2×2 blocks: value and slope per sample), solved in O(n); series `a` and `b`
/// (east and north) share the system.
///
/// `ws` are the samples' weights (1 each when `None`); `buf` is the solver's scratch, grown as
/// needed. The number of samples is `ts.len()`; the series hold at least as many.
pub fn smooth_spline(
    buf: &mut Vec<f64>,
    ts: &[f64],
    ws: Option<&[f64]>,
    lambda: f64,
    a: Series<'_>,
    mut b: Option<Series<'_>>,
    end: EndSlope,
) {
    let n = ts.len();
    if buf.len() < n * S {
        buf.resize((n * S).max(2 * buf.len()), 0.0);
    }
    let bb = &mut buf[..];
    let (ya, fa, da) = (a.y, a.f, a.d);
    // assembly and forward elimination
    for i in 0..n {
        let w = match ws {
            Some(ws) => ws[i],
            None => 1.0,
        };
        let mut ad = w;
        let mut bd = 0.0;
        let mut cd = 0.0;
        let mut u00 = 0.0;
        let mut u01 = 0.0;
        let mut u10 = 0.0;
        let mut u11 = 0.0;
        let mut raf = w * ya[i];
        let mut rad = 0.0;
        let mut rbf = match &b {
            Some(b) => w * b.y[i],
            None => 0.0,
        };
        let mut rbd = 0.0;
        // ∫f''² over a piece of length h is (4/h³)[3Δ² - 3hΔ(d0+d1) + h²(d0² + d0 d1 + d1²)], Δ = f1 - f0
        if i > 0 {
            let h = (ts[i] - ts[i - 1]) / 1000.0;
            let cc = (lambda * 4.0) / (h * h * h);
            ad += 3.0 * cc;
            bd -= 1.5 * h * cc;
            cd += h * h * cc;
        }
        if i < n - 1 {
            let h = (ts[i + 1] - ts[i]) / 1000.0;
            let cc = (lambda * 4.0) / (h * h * h);
            ad += 3.0 * cc;
            bd += 1.5 * h * cc;
            cd += h * h * cc;
            u00 = -3.0 * cc;
            u01 = 1.5 * h * cc;
            u10 = -1.5 * h * cc;
            u11 = 0.5 * h * h * cc;
        }
        if i == n - 1 && end.w > 0.0 {
            cd += end.w;
            rad += end.w * end.a;
            rbd += end.w * end.b;
        }
        if i > 0 {
            // eliminate the previous block: L = Uᵀ D⁻¹, D -= L U, r -= L r'
            let o = (i - 1) * S;
            let det = bb[o] * bb[o + 2] - bb[o + 1] * bb[o + 1];
            let i00 = bb[o + 2] / det;
            let i01 = -bb[o + 1] / det;
            let i11 = bb[o] / det;
            let q00 = bb[o + 3];
            let q01 = bb[o + 4];
            let q10 = bb[o + 5];
            let q11 = bb[o + 6];
            let l00 = q00 * i00 + q10 * i01;
            let l01 = q00 * i01 + q10 * i11;
            let l10 = q01 * i00 + q11 * i01;
            let l11 = q01 * i01 + q11 * i11;
            ad -= l00 * q00 + l01 * q10;
            bd -= l00 * q01 + l01 * q11;
            cd -= l10 * q01 + l11 * q11;
            raf -= l00 * bb[o + 7] + l01 * bb[o + 8];
            rad -= l10 * bb[o + 7] + l11 * bb[o + 8];
            rbf -= l00 * bb[o + 9] + l01 * bb[o + 10];
            rbd -= l10 * bb[o + 9] + l11 * bb[o + 10];
        }
        let o = i * S;
        bb[o] = ad;
        bb[o + 1] = bd;
        bb[o + 2] = cd;
        bb[o + 3] = u00;
        bb[o + 4] = u01;
        bb[o + 5] = u10;
        bb[o + 6] = u11;
        bb[o + 7] = raf;
        bb[o + 8] = rad;
        bb[o + 9] = rbf;
        bb[o + 10] = rbd;
    }
    // back substitution
    let mut zaf = 0.0;
    let mut zad = 0.0;
    let mut zbf = 0.0;
    let mut zbd = 0.0;
    for i in (0..n).rev() {
        let o = i * S;
        let mut raf = bb[o + 7];
        let mut rad = bb[o + 8];
        let mut rbf = bb[o + 9];
        let mut rbd = bb[o + 10];
        if i < n - 1 {
            raf -= bb[o + 3] * zaf + bb[o + 4] * zad;
            rad -= bb[o + 5] * zaf + bb[o + 6] * zad;
            rbf -= bb[o + 3] * zbf + bb[o + 4] * zbd;
            rbd -= bb[o + 5] * zbf + bb[o + 6] * zbd;
        }
        let det = bb[o] * bb[o + 2] - bb[o + 1] * bb[o + 1];
        let i00 = bb[o + 2] / det;
        let i01 = -bb[o + 1] / det;
        let i11 = bb[o] / det;
        zaf = i00 * raf + i01 * rad;
        zad = i01 * raf + i11 * rad;
        zbf = i00 * rbf + i01 * rbd;
        zbd = i01 * rbf + i11 * rbd;
        fa[i] = zaf;
        da[i] = zad;
        if let Some(b) = &mut b {
            b.f[i] = zbf;
            b.d[i] = zbd;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;

    /// The cost the spline minimises, for checking optimality: Σ w (f - y)² + λ ∫ f''² + end term.
    fn cost(ts: &[f64], ws: &[f64], lambda: f64, y: &[f64], f: &[f64], d: &[f64], end: (f64, f64)) -> f64 {
        let n = ts.len();
        let mut c = 0.0;
        for i in 0..n {
            c += ws[i] * (f[i] - y[i]) * (f[i] - y[i]);
        }
        for i in 1..n {
            let h = (ts[i] - ts[i - 1]) / 1000.0;
            let dl = f[i] - f[i - 1];
            let (d0, d1) = (d[i - 1], d[i]);
            c += lambda * (4.0 / (h * h * h)) * (3.0 * dl * dl - 3.0 * h * dl * (d0 + d1) + h * h * (d0 * d0 + d0 * d1 + d1 * d1));
        }
        c + end.0 * (d[n - 1] - end.1) * (d[n - 1] - end.1)
    }

    #[test]
    fn a_straight_line_is_left_as_it_is() {
        let ts = [0.0, 2_000.0, 4_000.0, 7_000.0, 8_000.0];
        let y: Vec<f64> = ts.iter().map(|t| 3.0 + 0.25 * t / 1000.0).collect();
        let y2: Vec<f64> = ts.iter().map(|t| -1.0 - 2.0 * t / 1000.0).collect();
        let (mut f, mut d, mut f2, mut d2) = (vec![0.0; 5], vec![0.0; 5], vec![0.0; 5], vec![0.0; 5]);
        let mut buf = Vec::new();
        smooth_spline(
            &mut buf,
            &ts,
            None,
            20.0,
            Series { y: &y, f: &mut f, d: &mut d },
            Some(Series { y: &y2, f: &mut f2, d: &mut d2 }),
            EndSlope::NONE,
        );
        for i in 0..5 {
            assert_close!(f[i], y[i], 9);
            assert_close!(d[i], 0.25, 9);
            assert_close!(f2[i], y2[i], 9);
            assert_close!(d2[i], -2.0, 9);
        }
        assert!(buf.len() >= 5 * S);
    }

    #[test]
    fn the_fit_minimises_its_cost() {
        let ts = [0.0, 1_500.0, 2_000.0, 4_000.0, 6_500.0, 8_000.0];
        let y = [0.0, 3.0, 1.0, 8.0, 2.0, 5.0];
        let ws = [1.0, 0.04, 1.0, 2.0, 1.0, 1.0];
        let end = EndSlope { w: 10.0, a: -1.0, b: 0.0 };
        let (mut f, mut d) = (vec![0.0; 6], vec![0.0; 6]);
        let mut buf = Vec::new();
        smooth_spline(&mut buf, &ts, Some(&ws), 20.0, Series { y: &y, f: &mut f, d: &mut d }, None, end);
        let best = cost(&ts, &ws, 20.0, &y, &f, &d, (end.w, end.a));
        // any small change to a node value or slope costs more: the gradient is zero there
        for k in 0..12 {
            for eps in [1e-4, -1e-4] {
                let (mut f2, mut d2) = (f.clone(), d.clone());
                if k < 6 { f2[k] += eps } else { d2[k - 6] += eps }
                assert!(cost(&ts, &ws, 20.0, &y, &f2, &d2, (end.w, end.a)) > best);
            }
        }
    }

    #[test]
    fn the_spline_matches_the_typescript() {
        // bun: smoothSpline(6, ts, ws, 20, y, fa, da, y2, fb, db, 10, -1, 0.5)
        let ts = [0.0, 1_500.0, 2_000.0, 4_000.0, 6_500.0, 8_000.0];
        let ws = [1.0, 0.04, 1.0, 2.0, 1.0, 1.0];
        let y = [0.0, 3.0, 1.0, 8.0, 2.0, 5.0];
        let y2 = [10.0, 9.0, 7.5, 7.0, 4.0, 1.0];
        let (mut fa, mut da, mut fb, mut db) = (vec![0.0; 6], vec![0.0; 6], vec![0.0; 6], vec![0.0; 6]);
        let mut buf = vec![7.0; 3];
        smooth_spline(
            &mut buf,
            &ts,
            Some(&ws),
            20.0,
            Series { y: &y, f: &mut fa, d: &mut da },
            Some(Series { y: &y2, f: &mut fb, d: &mut db }),
            EndSlope { w: 10.0, a: -1.0, b: 0.5 },
        );
        let efa = [0.8296393339808812, 1.6052149227109373, 1.9234426716617526, 4.084651860689102, 4.346839609698493, 3.656458521087094];
        let eda = [0.5147587002232958, 0.6152766554149914, 0.6510022880983208, 0.7810493004627356, -0.4061113418599584, -0.6024826003880014];
        let efb = [10.031418993022633, 8.893998839216018, 8.517102112040225, 6.833099011662219, 4.0981659542813745, 2.8040346931451035];
        let edb = [-0.7518458497052614, -0.7596052216591479, -0.7476549917407542, -0.950016585216327, -1.2098826637052535, -0.3826155649512282];
        for i in 0..6 {
            assert_close!(fa[i], efa[i], 12);
            assert_close!(da[i], eda[i], 12);
            assert_close!(fb[i], efb[i], 12);
            assert_close!(db[i], edb[i], 12);
        }
    }

    #[test]
    fn one_sample_is_left_to_the_callers_and_none_is_nothing() {
        let mut buf = Vec::new();
        let (mut f, mut d) = (vec![9.0], vec![9.0]);
        smooth_spline(&mut buf, &[5_000.0], None, 20.0, Series { y: &[4.0], f: &mut f, d: &mut d }, None, EndSlope::NONE);
        // nothing pins the slope of a single sample (0/0), as in the TypeScript: its callers
        // handle one sample themselves
        assert!(f[0].is_nan() && d[0].is_nan());
        let (mut f, mut d) = (Vec::<f64>::new(), Vec::<f64>::new());
        smooth_spline(&mut buf, &[], None, 20.0, Series { y: &[], f: &mut f, d: &mut d }, None, EndSlope::NONE);
    }
}
