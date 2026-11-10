//! Test helpers shared by this crate and the crates above it.
//!
//! `assert_close!` is Jest's `toBeCloseTo`: `assert_close!(a, v, digits)` passes when
//! `|a − v| < 0.5·10^−digits`. Two digits when the third argument is left out, as in Jest.

/// A number an assertion can compare as `f64`.
pub trait AsF64: Copy {
    fn as_f64(self) -> f64;
}

impl AsF64 for f64 {
    fn as_f64(self) -> f64 {
        self
    }
}

macro_rules! as_f64 {
    ($($t:ty),*) => {$(
        impl AsF64 for $t {
            fn as_f64(self) -> f64 {
                self as f64
            }
        }
    )*};
}
as_f64!(f32, i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

/// Jest's `toBeCloseTo` rule. Equal infinities are close; NaN is close to nothing.
pub fn is_close(actual: impl AsF64, expected: impl AsF64, digits: i32) -> bool {
    let (a, v) = (actual.as_f64(), expected.as_f64());
    if a.is_infinite() && v.is_infinite() {
        return a == v;
    }
    (v - a).abs() < 10f64.powi(-digits) / 2.0
}

/// `assert_close!(actual, expected)` or `assert_close!(actual, expected, digits)`:
/// passes when `|actual − expected| < 0.5·10^−digits` (two digits by default).
#[macro_export]
macro_rules! assert_close {
    ($actual:expr, $expected:expr $(,)?) => {
        $crate::assert_close!($actual, $expected, 2)
    };
    ($actual:expr, $expected:expr, $digits:expr $(,)?) => {{
        let (actual, expected, digits) = ($actual, $expected, $digits);
        assert!(
            $crate::testing::is_close(actual, expected, digits),
            "assert_close!({}, {}, {}): got {:?}, expected {:?} to {} digits",
            stringify!($actual),
            stringify!($expected),
            stringify!($digits),
            actual,
            expected,
            digits,
        );
    }};
}

#[cfg(test)]
mod tests {
    use super::is_close;

    #[test]
    fn close_means_under_half_a_unit_of_the_last_digit() {
        assert_close!(1.004, 1.0);
        assert_close!(1.0049, 1.0, 2);
        assert!(!is_close(1.006, 1.0, 2));
        assert!(!is_close(0.994, 1.0, 2));
        assert_close!(0.3, 0.1 + 0.2, 15);
        assert!(!is_close(0.3, 0.1 + 0.2, 17));
    }

    #[test]
    fn digits_may_be_zero_or_negative() {
        assert_close!(10.4, 10, 0);
        assert!(!is_close(10.5, 10, 0));
        assert_close!(1040, 1000, -2);
        assert!(!is_close(1050, 1000, -2));
    }

    #[test]
    fn infinities_and_nan() {
        assert_close!(f64::INFINITY, f64::INFINITY);
        assert_close!(f64::NEG_INFINITY, f64::NEG_INFINITY, 5);
        assert!(!is_close(f64::INFINITY, f64::NEG_INFINITY, 2));
        assert!(!is_close(f64::INFINITY, 1.0, 2));
        assert!(!is_close(f64::NAN, f64::NAN, 2));
        assert!(!is_close(f64::NAN, 1.0, 2));
    }

    #[test]
    fn mixed_number_types_compare() {
        assert_close!(3_i32, 3.001_f32);
        assert_close!(7_usize, 7.0, 9);
    }

    #[test]
    #[should_panic(expected = "assert_close!(1.5, 1.0, 1)")]
    fn failure_names_the_expressions() {
        assert_close!(1.5, 1.0, 1);
    }
}
