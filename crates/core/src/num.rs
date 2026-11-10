//! JavaScript-semantics number helpers (doc 05 §14): use these wherever the TypeScript used the
//! JavaScript built-in, so that a ported test passes for the right reason.
//!
//! | JavaScript | here |
//! |---|---|
//! | `Math.round(x)` | [`js_round`] |
//! | `Math.sign(x)` | [`sign0`] |
//! | `x.toFixed(n)` | [`to_fixed`] |
//! | `Number(x.toFixed(n))` | [`fixed_number`] |
//! | `String(x)`, `JSON.stringify(x)` of a number | [`js_string`] |
//! | `x \| 0` | [`to_int32`] |
//! | `Uint8ClampedArray[i] = x` | [`clamp_u8`] |
//! | `motion.ts` `wrap180`, `wrap360`, `wrapLon`, `smoothstep`; `weather.ts` `clamp01` | same names |

/// `Math.round`: the nearest integer, a half rounding towards +infinity (`-2.5 → -2`, `2.5 → 3`).
/// `f64::round` rounds a half away from zero instead.
///
/// Exact for every input (unlike `(x + 0.5).floor()`, which is wrong for 0.49999999999999994 and
/// above 2^52); keeps the sign of zero as JavaScript does (`-0.2 → -0`); NaN and infinities pass.
pub fn js_round(x: f64) -> f64 {
    let floor = x.floor();
    // `x - floor` is exact: both are within a factor of two, or the difference is x itself
    if x - floor >= 0.5 {
        let up = floor + 1.0;
        // Math.round(-0.5) and Math.round(-0.2) are -0
        if up == 0.0 && x.is_sign_negative() { -0.0 } else { up }
    } else {
        floor
    }
}

/// `Math.sign`: -1, 0 or 1 (the zero keeps its sign), NaN for NaN.
/// `f64::signum(0.0)` is 1.0 instead.
pub fn sign0(x: f64) -> f64 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        // +0, -0 and NaN come back as they are
        x
    }
}

/// The digits of a non-negative finite `a` with exactly `digits` decimals, a tie rounding up
/// (`2.5 → "3"`, `0.125 → "0.13"`). Only an exact tie differs from `format!("{:.n}")`, which
/// rounds it to even: 1.005 is below the tie as a double and gives "1.00" either way.
fn fixed_abs(a: f64, digits: usize) -> String {
    let one_more = format!("{:.*}", digits + 1, a);
    if one_more.ends_with('5') {
        // A tie has exactly `digits + 1` decimals; no double has more than 1074.
        let exact = format!("{a:.1075}");
        let tie = exact.starts_with(&one_more) && exact[one_more.len()..].bytes().all(|b| b == b'0');
        if tie {
            let mut out = one_more.into_bytes();
            out.pop();
            if out.last() == Some(&b'.') {
                out.pop();
            }
            let mut carry = true;
            for b in out.iter_mut().rev() {
                if *b == b'.' {
                    continue;
                }
                if *b == b'9' {
                    *b = b'0';
                } else {
                    *b += 1;
                    carry = false;
                    break;
                }
            }
            if carry {
                out.insert(0, b'1');
            }
            return String::from_utf8(out).expect("ASCII digits");
        }
    }
    format!("{a:.digits$}")
}

/// `x.toFixed(digits)`: ties round up in magnitude (`2.5 → "3"`, `0.5 → "1"`, `-2.5 → "-3"`),
/// where `format!("{:.n}")` rounds them to even. `-0` prints as `"0"`, a negative value that
/// rounds to zero keeps its sign (`"-0.00"`), NaN is `"NaN"`, and from 1e21 the result is
/// [`js_string`]'s, as in JavaScript.
pub fn to_fixed(x: f64, digits: usize) -> String {
    if !x.is_finite() || x.abs() >= 1e21 {
        return js_string(x);
    }
    let body = fixed_abs(x.abs(), digits);
    if x < 0.0 { format!("-{body}") } else { body }
}

/// `Number(x.toFixed(digits))`: rounded to `digits` decimals, and `-0` becomes `0`.
/// Print it with [`js_string`] (or `{}`) for the shortest form without trailing zeros.
pub fn fixed_number(x: f64, digits: usize) -> f64 {
    let v: f64 = match to_fixed(x, digits).as_str() {
        "Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        s => s.parse().unwrap_or(f64::NAN),
    };
    if v == 0.0 { 0.0 } else { v }
}

/// `String(x)` for a number, which is also how `JSON.stringify` writes a finite one: the
/// shortest digits that read back the same, a whole value without `.0` (`10668`, where Rust's
/// `{:?}` and serde_json write `10668.0`), `-0` as `0`, exponent form below 1e-6 and from 1e21
/// (`1e+21`, `1.5e-7`), `NaN`, `Infinity`, `-Infinity`.
pub fn js_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    if x == 0.0 {
        return "0".to_owned();
    }
    let a = x.abs();
    if (1e-6..1e21).contains(&a) {
        return format!("{x}");
    }
    let s = format!("{x:e}");
    match s.split_once('e') {
        Some((mantissa, exp)) if !exp.starts_with('-') => format!("{mantissa}e+{exp}"),
        _ => s,
    }
}

/// `x | 0` (ToInt32): truncated, then wrapped to 32 bits; NaN and infinities give 0.
/// `x as i32` saturates instead.
pub fn to_int32(x: f64) -> i32 {
    if !x.is_finite() {
        return 0;
    }
    (x.trunc().rem_euclid(4_294_967_296.0) as u32) as i32
}

/// A store into a `Uint8ClampedArray`: clamped to 0..=255, ties rounding to even, NaN as 0.
/// `x as u8` truncates instead.
pub fn clamp_u8(x: f64) -> u8 {
    // NaN stays NaN through clamp and round, and `NaN as u8` is 0
    x.clamp(0.0, 255.0).round_ties_even() as u8
}

/// A longitude brought into [-180, 180) (`motion.ts` `wrapLon`).
pub fn wrap_lon(lon: f64) -> f64 {
    ((((lon + 180.0) % 360.0) + 360.0) % 360.0) - 180.0
}

/// An angle difference brought into [-180, 180) (`motion.ts`, `flightpath.ts` `wrap180`).
pub fn wrap180(deg: f64) -> f64 {
    ((((deg + 180.0) % 360.0) + 360.0) % 360.0) - 180.0
}

/// An angle brought into [0, 360) (`motion.ts` `wrap360`).
pub fn wrap360(deg: f64) -> f64 {
    ((deg % 360.0) + 360.0) % 360.0
}

/// `Math.min(1, Math.max(0, x))` (`weather.ts`, `realistic.ts` `clamp01`): NaN stays NaN, where
/// `x.max(0.0).min(1.0)` would turn it into 0.
pub fn clamp01(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    // written out so that -0 behaves as in JavaScript too: max(0, -0) is +0
    let lo = if x > 0.0 { x } else { 0.0 };
    if lo < 1.0 { lo } else { 1.0 }
}

/// 0 up to `t = 0`, 1 from `t = 1`, the cubic ease between (`motion.ts` `smoothstep`); NaN stays NaN.
pub fn smoothstep(t: f64) -> f64 {
    if t <= 0.0 {
        0.0
    } else if t >= 1.0 {
        1.0
    } else {
        t * t * (3.0 - 2.0 * t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bit-for-bit, so that -0 and +0 differ.
    fn same(a: f64, b: f64) -> bool {
        a.to_bits() == b.to_bits()
    }

    #[test]
    fn js_round_rounds_a_half_towards_plus_infinity() {
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-3.5), -3.0);
        assert_eq!(js_round(-2.6), -3.0);
        assert_eq!(js_round(-2.4), -2.0);
        assert_eq!(js_round(0.5), 1.0);
        assert_eq!(js_round(1.4999), 1.0);
        assert_eq!(js_round(7.0), 7.0);
        // where f64::round goes the other way
        assert_eq!((-2.5f64).round(), -3.0);
    }

    #[test]
    fn js_round_is_exact_at_the_edges() {
        // the largest double below 0.5: `(x + 0.5).floor()` would say 1
        assert_eq!(js_round(0.499_999_999_999_999_94), 0.0);
        // above 2^52 every double is whole: `x + 0.5` would round to the next even one
        assert_eq!(js_round(4_503_599_627_370_497.0), 4_503_599_627_370_497.0);
        assert_eq!(js_round(-4_503_599_627_370_497.0), -4_503_599_627_370_497.0);
        assert!(same(js_round(-0.5), -0.0));
        assert!(same(js_round(-0.2), -0.0));
        assert!(same(js_round(-0.0), -0.0));
        assert!(same(js_round(0.2), 0.0));
        assert!(js_round(f64::NAN).is_nan());
        assert_eq!(js_round(f64::INFINITY), f64::INFINITY);
        assert_eq!(js_round(f64::NEG_INFINITY), f64::NEG_INFINITY);
    }

    #[test]
    fn sign0_is_zero_for_zero() {
        assert_eq!(sign0(0.0), 0.0);
        assert!(same(sign0(0.0), 0.0));
        assert!(same(sign0(-0.0), -0.0));
        assert_eq!(sign0(3.2), 1.0);
        assert_eq!(sign0(-1e-300), -1.0);
        assert_eq!(sign0(f64::INFINITY), 1.0);
        assert_eq!(sign0(f64::NEG_INFINITY), -1.0);
        assert!(sign0(f64::NAN).is_nan());
        // where f64::signum goes the other way
        assert_eq!(0.0f64.signum(), 1.0);
    }

    #[test]
    fn to_fixed_rounds_ties_up_in_magnitude() {
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(0.5, 0), "1");
        assert_eq!(to_fixed(1.5, 0), "2");
        assert_eq!(to_fixed(-2.5, 0), "-3");
        assert_eq!(to_fixed(-0.5, 0), "-1");
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(0.375, 2), "0.38");
        assert_eq!(to_fixed(2.25, 1), "2.3");
        assert_eq!(to_fixed(9.5, 0), "10");
        assert_eq!(to_fixed(99.5, 0), "100");
        assert_eq!(to_fixed(0.96875, 4), "0.9688");
        assert_eq!(to_fixed(9.96875, 4), "9.9688");
        assert_eq!(to_fixed(0.999_511_718_75, 10), "0.9995117188");
        // where Rust's formatting goes the other way
        assert_eq!(format!("{:.0}", 2.5), "2");
        assert_eq!(format!("{:.0}", 0.5), "0");
    }

    #[test]
    fn to_fixed_leaves_what_is_not_a_tie_alone() {
        // these are not ties as doubles: 1.005 is a little below, 1.45 a little above
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(1.45, 1), "1.4");
        assert_eq!(to_fixed(8.345, 2), "8.35");
        assert_eq!(to_fixed(0.15, 1), "0.1");
        assert_eq!(to_fixed(1.0, 0), "1");
        assert_eq!(to_fixed(1.0, 2), "1.00");
        assert_eq!(to_fixed(12.3456, 2), "12.35");
        assert_eq!(to_fixed(-12.3456, 1), "-12.3");
        assert_eq!(to_fixed(59.913_868_4, 6), "59.913868");
        assert_eq!(to_fixed(123.456, 0), "123");
        assert_eq!(to_fixed(0.000_001, 7), "0.0000010");
        assert_eq!(to_fixed(1e20, 2), "100000000000000000000.00");
    }

    #[test]
    fn to_fixed_zero_sign_and_the_values_that_are_not_numbers() {
        assert_eq!(to_fixed(0.0, 2), "0.00");
        assert_eq!(to_fixed(-0.0, 2), "0.00");
        assert_eq!(to_fixed(-0.0, 0), "0");
        // a negative value keeps its sign even when it rounds to zero
        assert_eq!(to_fixed(-0.001, 2), "-0.00");
        assert_eq!(to_fixed(-0.4, 0), "-0");
        assert_eq!(to_fixed(f64::NAN, 2), "NaN");
        assert_eq!(to_fixed(f64::INFINITY, 2), "Infinity");
        assert_eq!(to_fixed(f64::NEG_INFINITY, 2), "-Infinity");
        assert_eq!(to_fixed(1e21, 2), "1e+21");
        assert_eq!(to_fixed(-1.5e22, 2), "-1.5e+22");
    }

    #[test]
    fn fixed_number_prints_in_the_shortest_form_and_minus_zero_as_zero() {
        // export.ts `coord`: Number(x.toFixed(6))
        assert_eq!(js_string(fixed_number(24.9, 6)), "24.9");
        assert_eq!(js_string(fixed_number(60.0, 6)), "60");
        assert_eq!(js_string(fixed_number(60.317_222_22, 6)), "60.317222");
        assert_eq!(js_string(fixed_number(-0.000_000_1, 6)), "0");
        assert!(same(fixed_number(-0.000_000_1, 6), 0.0));
        assert!(same(fixed_number(-0.0, 6), 0.0));
        assert_eq!(fixed_number(2.5, 0), 3.0);
        assert_eq!(fixed_number(-24.123_456_7, 6), -24.123_457);
        assert!(fixed_number(f64::NAN, 6).is_nan());
        assert_eq!(fixed_number(f64::INFINITY, 6), f64::INFINITY);
        assert_eq!(fixed_number(f64::NEG_INFINITY, 6), f64::NEG_INFINITY);
        // where Rust's formatting keeps the zeros
        assert_eq!(format!("{:.6}", 24.9), "24.900000");
    }

    #[test]
    fn js_string_writes_a_whole_float_without_a_fraction() {
        assert_eq!(js_string(10668.0), "10668");
        assert_eq!(js_string(-3.0), "-3");
        assert_eq!(js_string(0.1), "0.1");
        assert_eq!(js_string(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(js_string(-0.0), "0");
        assert_eq!(js_string(1234.5), "1234.5");
        assert_eq!(js_string(1e20), "100000000000000000000");
        assert_eq!(js_string(0.000_001), "0.000001");
        // where Rust's debug form and serde_json go the other way
        assert_eq!(format!("{:?}", 10668.0), "10668.0");
        assert_eq!(serde_json::to_string(&10668.0).unwrap(), "10668.0");
    }

    #[test]
    fn js_string_exponent_form_and_the_values_that_are_not_numbers() {
        assert_eq!(js_string(1e21), "1e+21");
        assert_eq!(js_string(1.5e21), "1.5e+21");
        assert_eq!(js_string(-1e21), "-1e+21");
        assert_eq!(js_string(1e-7), "1e-7");
        assert_eq!(js_string(1.5e-7), "1.5e-7");
        assert_eq!(js_string(f64::MAX), "1.7976931348623157e+308");
        assert_eq!(js_string(5e-324), "5e-324");
        assert_eq!(js_string(f64::NAN), "NaN");
        assert_eq!(js_string(f64::INFINITY), "Infinity");
        assert_eq!(js_string(f64::NEG_INFINITY), "-Infinity");
    }

    #[test]
    fn remainder_keeps_the_sign_of_the_dividend_and_the_wraps_bring_it_back() {
        // `%` is the same in both languages: the wraps need both halves of `((x % n) + n) % n`
        assert_eq!(-10.0f64 % 360.0, -10.0);
        assert_eq!(wrap360(-10.0), 350.0);
        assert_eq!(wrap360(370.0), 10.0);
        assert_eq!(wrap360(360.0), 0.0);
        assert_eq!(wrap360(0.0), 0.0);
        assert_eq!(wrap360(-720.0), 0.0);
        assert_eq!(wrap360(725.5), 5.5);
        assert!(wrap360(f64::NAN).is_nan());
        assert!(wrap360(f64::INFINITY).is_nan());
    }

    #[test]
    fn wrap180_is_half_open_at_plus_180() {
        assert_eq!(wrap180(180.0), -180.0);
        assert_eq!(wrap180(-180.0), -180.0);
        assert_eq!(wrap180(179.0), 179.0);
        assert_eq!(wrap180(190.0), -170.0);
        assert_eq!(wrap180(-190.0), 170.0);
        assert_eq!(wrap180(350.0 - 10.0), -20.0);
        assert_eq!(wrap180(10.0 - 350.0), 20.0);
        assert_eq!(wrap180(540.0), -180.0);
        assert_eq!(wrap180(0.0), 0.0);
        assert_eq!(wrap180(-900.0), -180.0);
        assert!(wrap180(f64::NAN).is_nan());
    }

    #[test]
    fn wrap_lon_brings_a_longitude_into_range() {
        assert_eq!(wrap_lon(181.0), -179.0);
        assert_eq!(wrap_lon(-181.0), 179.0);
        assert_eq!(wrap_lon(180.0), -180.0);
        assert_eq!(wrap_lon(24.5), 24.5);
        // the detour through +180 costs the last bits, as it does in JavaScript
        assert_eq!(wrap_lon(24.95), 24.950000000000045);
        assert_eq!(wrap_lon(360.0), 0.0);
        assert_eq!(wrap_lon(-360.0), 0.0);
        assert_eq!(wrap_lon(540.5), -179.5);
    }

    #[test]
    fn to_int32_wraps_where_a_cast_saturates() {
        assert_eq!(to_int32(4_294_967_296.0), 0);
        assert_eq!(to_int32(4_294_967_297.0), 1);
        assert_eq!(to_int32(2_147_483_648.0), -2_147_483_648);
        assert_eq!(to_int32(2_147_483_647.0), 2_147_483_647);
        assert_eq!(to_int32(-2_147_483_649.0), 2_147_483_647);
        assert_eq!(to_int32(3_000_000_000.0), -1_294_967_296);
        assert_eq!(to_int32(3.7), 3);
        assert_eq!(to_int32(-3.7), -3);
        assert_eq!(to_int32(-0.5), 0);
        assert_eq!(to_int32(-1.0), -1);
        assert_eq!(to_int32(1e21), -559_939_584);
        assert_eq!(to_int32(f64::NAN), 0);
        assert_eq!(to_int32(f64::INFINITY), 0);
        assert_eq!(to_int32(f64::NEG_INFINITY), 0);
        // where the cast goes the other way
        assert_eq!(3_000_000_000.0f64 as i32, i32::MAX);
        // Math.imul is wrapping_mul on the wrapped operands; >>> is a shift on u32
        assert_eq!(to_int32(2_654_435_761.0).wrapping_mul(to_int32(40_503.0)), 990_274_311);
        assert_eq!((to_int32(-1.0) as u32) >> 28, 15);
    }

    #[test]
    fn clamp_u8_clamps_and_rounds_ties_to_even() {
        assert_eq!(clamp_u8(-5.0), 0);
        assert_eq!(clamp_u8(300.0), 255);
        assert_eq!(clamp_u8(0.5), 0);
        assert_eq!(clamp_u8(1.5), 2);
        assert_eq!(clamp_u8(2.5), 2);
        assert_eq!(clamp_u8(254.5), 254);
        assert_eq!(clamp_u8(254.6), 255);
        assert_eq!(clamp_u8(127.4), 127);
        assert_eq!(clamp_u8(f64::NAN), 0);
        assert_eq!(clamp_u8(f64::INFINITY), 255);
        // where the cast goes the other way
        assert_eq!(1.5f64 as u8, 1);
    }

    #[test]
    #[allow(clippy::manual_clamp)]
    fn clamp01_clamps_and_keeps_nan() {
        assert_eq!(clamp01(-0.3), 0.0);
        assert_eq!(clamp01(0.0), 0.0);
        assert_eq!(clamp01(0.25), 0.25);
        assert_eq!(clamp01(1.0), 1.0);
        assert_eq!(clamp01(1.7), 1.0);
        assert_eq!(clamp01(f64::INFINITY), 1.0);
        assert_eq!(clamp01(f64::NEG_INFINITY), 0.0);
        assert!(same(clamp01(-0.0), 0.0));
        // Math.min(1, Math.max(0, NaN)) is NaN
        assert!(clamp01(f64::NAN).is_nan());
        // where the std chain goes the other way
        assert_eq!(f64::NAN.max(0.0).min(1.0), 0.0);
    }

    #[test]
    fn smoothstep_eases_between_zero_and_one() {
        assert_eq!(smoothstep(-1.0), 0.0);
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(0.25), 0.15625);
        assert_eq!(smoothstep(0.5), 0.5);
        assert_eq!(smoothstep(0.75), 0.84375);
        assert_eq!(smoothstep(1.0), 1.0);
        assert_eq!(smoothstep(2.0), 1.0);
        assert_eq!(smoothstep(f64::INFINITY), 1.0);
        assert_eq!(smoothstep(f64::NEG_INFINITY), 0.0);
        // both comparisons are false for NaN, so the polynomial carries it through
        assert!(smoothstep(f64::NAN).is_nan());
    }

    #[test]
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    fn comparisons_with_nan_are_false_so_negated_forms_must_be_kept() {
        let x = f64::NAN;
        // `!(x > 0)` is true for NaN where `x <= 0` is false: keep the form the TypeScript has
        assert!(!(x > 0.0));
        assert!(!(x <= 0.0));
        // `x === x` is the NaN test
        #[allow(clippy::eq_op)]
        {
            assert!(x != x);
        }
        // Math.min(...[]) is Infinity: the fold over an empty list must start there
        assert_eq!(std::iter::empty::<f64>().fold(f64::INFINITY, f64::min), f64::INFINITY);
    }
}
