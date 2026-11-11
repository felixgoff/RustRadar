//! The data types of `api.ts`, one definition for logic, backend and scene (S1.5).
//!
//! Each type keeps the field names it has on the wire today and the number types the backend
//! sends, so replacing a backend struct with its twin here leaves the JSON unchanged. Every
//! type is re-exported from this module: `rustradar_core::model::LiveFlight`.
//!
//! Numbers: several fields are `f32` because that is what the upstream feeds carry
//! (`LiveFlight::lat`, `TrailPoint::latitude`, `Ship::heading`, …). The TypeScript never saw
//! the `f32`: it read the JSON's shortest decimal as a double. A port that must reproduce the
//! TypeScript's arithmetic converts with [`f32_as_js`], not with `as f64`.

pub mod adsb;
pub mod airport;
pub mod flight;
pub mod keys;
pub mod place;
pub mod radar;
pub mod ship;
pub mod train;
pub mod weather;

pub use adsb::*;
pub use airport::*;
pub use flight::*;
pub use keys::*;
pub use place::*;
pub use radar::*;
pub use ship::*;
pub use train::*;
pub use weather::*;

/// The double JavaScript read for an `f32` the backend serialised: the nearest double to the
/// value's shortest decimal form (`51.4775_f32` → `51.4775`, where `as f64` gives
/// `51.477500915527344`). NaN and the infinities pass through.
pub fn f32_as_js(value: f32) -> f64 {
    if !value.is_finite() {
        return value as f64;
    }
    // `Display` writes the shortest decimal that reads back as the same `f32`, as serde_json does;
    // written on the stack (the scene builders ask this for every aircraft every frame, P1): the
    // longest, a subnormal's `-0.000…1`, is 48 bytes
    let mut text = StackText { bytes: [0; 48], len: 0 };
    match std::fmt::write(&mut text, format_args!("{value}")) {
        Ok(()) => std::str::from_utf8(&text.bytes[..text.len]).ok().and_then(|t| t.parse().ok()).unwrap_or(value as f64),
        Err(_) => value.to_string().parse().unwrap_or(value as f64),
    }
}

/// A short text written on the stack ([`f32_as_js`]); writing past its end fails.
struct StackText {
    bytes: [u8; 48],
    len: usize,
}

impl std::fmt::Write for StackText {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self.len + s.len();
        self.bytes.get_mut(self.len..end).ok_or(std::fmt::Error)?.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stack text (P1) reads every `f32` as its `to_string()` does: the extremes, subnormals,
    /// signed zeros, and a sweep of bit patterns.
    #[test]
    fn f32_as_js_reads_as_the_heap_text_does() {
        let heap = |v: f32| -> f64 { if v.is_finite() { v.to_string().parse().unwrap() } else { v as f64 } };
        let mut values = vec![0.0, -0.0, f32::MIN_POSITIVE, -f32::MIN_POSITIVE, f32::MAX, f32::MIN, f32::EPSILON, 1e-45, -1e-45, 1.0e38, 59.6519, 17.9186, -179.99];
        values.extend((0..200_000u32).map(|i| f32::from_bits(i.wrapping_mul(0x9E37_79B9) ^ (i << 7))));
        for v in values {
            let (ours, theirs) = (f32_as_js(v), heap(v));
            assert!(ours.to_bits() == theirs.to_bits() || (ours.is_nan() && theirs.is_nan()), "{v:e}: {ours} against {theirs}");
        }
        assert!(f32_as_js(f32::NAN).is_nan());
        assert_eq!(f32_as_js(f32::NEG_INFINITY), f64::NEG_INFINITY);
    }

    #[test]
    fn f32_as_js_reads_the_shortest_decimal() {
        assert_eq!(f32_as_js(51.4775), 51.4775);
        assert_ne!(51.4775_f32 as f64, 51.4775);
        assert_eq!(f32_as_js(-0.461941), -0.461941);
        assert_eq!(f32_as_js(4.95295), 4.95295);
        assert_eq!(f32_as_js(0.0), 0.0);
        assert!(f32_as_js(-0.0).is_sign_negative());
        assert_eq!(f32_as_js(1.0e-7), 1.0e-7);
        assert_eq!(f32_as_js(3.0e20), 3.0e20);
        assert_eq!(f32_as_js(f32::MAX), 3.4028235e38);
        assert_eq!(f32_as_js(f32::INFINITY), f64::INFINITY);
        assert!(f32_as_js(f32::NAN).is_nan());
    }

    #[test]
    fn f32_as_js_agrees_with_the_json_the_backend_writes() {
        for value in [52.3236_f32, 4.720_43, -84.4281, 271.3, 0.848, 1012.8, 108.07, 1.0e-10, 16_777_216.0] {
            let json = serde_json::to_string(&value).unwrap();
            let read: f64 = serde_json::from_str(&json).unwrap();
            assert_eq!(f32_as_js(value), read, "{json}");
        }
    }

    #[test]
    fn every_type_is_reachable_from_the_module() {
        let _ = (LiveFlight::default(), Airport::default(), Ship::default(), AdsbAircraft::default());
        let _ = (RadarVolume::default(), Weather::default(), Keys::default(), TrainRoute::default());
        let _ = (PlaceKind::City, TrainKind::Other, TaxiKind::Runway, BoardMode::Arrivals);
    }
}
