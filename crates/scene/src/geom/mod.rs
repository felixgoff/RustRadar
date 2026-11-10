//! Geometry kernel (S1.24, stage 2): math.gl's grid cutting, earcut and deck.gl's globe
//! subdivision, plus the JavaScript-semantics helpers the geometry ports share.

pub mod densify;
pub mod earcut;
pub mod grid_cut;
pub mod path;

/// `Math.min(a, b)`: NaN when either is NaN, and -0 below +0 (`f64::min` drops a NaN instead).
pub fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b || (a == b && a.is_sign_negative()) {
        a
    } else {
        b
    }
}

/// `Math.max(a, b)`: NaN when either is NaN, and +0 above -0 (`f64::max` drops a NaN instead).
pub fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b || (a == b && a.is_sign_positive()) {
        a
    } else {
        b
    }
}

/// `Math.hypot(...values)` as JavaScriptCore computes it (bun, which made every golden value of
/// the port, and WebKit, the webview on macOS and Linux): two values through the C library's
/// `hypot` ([`js_hypot2`]); any other count scaled by the largest and summed with Kahan's
/// compensation, as both JavaScriptCore and V8 do. An infinity wins over NaN, then NaN. V8 (the
/// Windows webview) also takes the Kahan path for two values, which can differ in the last bit.
pub fn js_hypot(values: &[f64]) -> f64 {
    if let &[x, y] = values {
        return js_hypot2(x, y);
    }
    let mut max = 0.0_f64;
    let mut nan = false;
    for &v in values {
        if v.is_nan() {
            nan = true;
        } else if v.abs() > max {
            max = v.abs();
        }
    }
    if max == f64::INFINITY {
        return f64::INFINITY;
    }
    if nan {
        return f64::NAN;
    }
    if max == 0.0 {
        return 0.0;
    }
    let (mut sum, mut compensation) = (0.0_f64, 0.0_f64);
    for &v in values {
        let n = v.abs() / max;
        let summand = n * n - compensation;
        let preliminary = sum + summand;
        compensation = (preliminary - sum) - summand;
        sum = preliminary;
    }
    sum.sqrt() * max
}

/// `Math.hypot(x, y)` as JavaScriptCore computes it: `f64::hypot`, the C library's (see
/// [`js_hypot`]). An infinity wins over NaN, as in JavaScript.
pub fn js_hypot2(x: f64, y: f64) -> f64 {
    x.hypot(y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(a: f64, b: f64) -> bool {
        a.to_bits() == b.to_bits()
    }

    #[test]
    fn js_min_and_max_follow_javascript() {
        assert!(js_min(f64::NAN, 1.0).is_nan());
        assert!(js_max(1.0, f64::NAN).is_nan());
        assert!(same(js_min(0.0, -0.0), -0.0));
        assert!(same(js_max(-0.0, 0.0), 0.0));
        assert_eq!(js_min(2.0, 3.0), 2.0);
        assert_eq!(js_max(2.0, 3.0), 3.0);
    }

    #[test]
    fn js_hypot_follows_the_engines() {
        assert_eq!(js_hypot(&[]), 0.0);
        assert_eq!(js_hypot2(3.0, 4.0), 5.0);
        assert_eq!(js_hypot2(-3.0, 4.0), 5.0);
        assert_eq!(js_hypot(&[0.0, -0.0]), 0.0);
        assert!(js_hypot2(f64::NAN, 1.0).is_nan());
        assert_eq!(js_hypot2(f64::NAN, f64::NEG_INFINITY), f64::INFINITY);
        // no overflow for large values, no underflow for small ones
        assert_eq!(js_hypot(&[1e300, 1e300, 0.0]), 1e300 * 2f64.sqrt());
        assert!((js_hypot(&[1e-300, 1e-300, 0.0]) / 1e-300 - 2f64.sqrt()).abs() < 1e-15);
        // values bun prints for Math.hypot
        assert_eq!(js_hypot2(0.1, 0.2), 0.223606797749979);
        assert_eq!(js_hypot(&[1.0, 2.0, 3.0]), 3.741657386773941);
        assert_eq!(js_hypot(&[-0.75, -0.25, 1.0]), 1.2747548783981961);
        assert_eq!(js_hypot(&[3.0]), 3.0);
        assert!(js_hypot(&[1.0, f64::NAN, 2.0]).is_nan());
        assert_eq!(js_hypot(&[1.0, f64::NAN, f64::INFINITY]), f64::INFINITY);
        // where bun (JavaScriptCore) and node (V8) part: bun's, 12.013013776733962 in node
        assert_eq!(js_hypot2(9.848920823762272, 6.878318007133856), 12.013013776733963);
        assert_eq!(js_hypot(&[9.848920823762272, 6.878318007133856]), 12.013013776733963);
    }
}
