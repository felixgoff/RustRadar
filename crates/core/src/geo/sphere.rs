//! Great-circle helpers on a sphere of 6 371 000 m: `flightpath.ts:76-133` (`distanceM`,
//! `distanceKm`, `bearing`, `destination`, `intermediate`, `greatCircle`, `unwrap`).
//!
//! Points are `[lon, lat]` in degrees, as in the TypeScript. `backend::rail_motion`'s `distance`,
//! `course` and `ahead` are the same formulas with `(lat, lon)` tuples (doc 05 §4.1): check the
//! argument order when switching it over to these.
//!
//! Transliterated as written: degrees become radians with `* DEG` and come back with `/ DEG`
//! (not `to_radians`/`to_degrees`, which round differently), `%` is JavaScript's (the sign of the
//! dividend, as Rust's), and `Math.min`/`Math.max` keep NaN as they do in JavaScript.

use super::{DEG, EARTH_RADIUS_M, js_max, js_min};
use crate::num::wrap180;

/// Longitude, latitude in degrees (`flightpath.ts` `LonLat`).
pub type LonLat = [f64; 2];
/// Longitude, latitude in degrees, altitude in feet above mean sea level (`flightpath.ts`
/// `PathPoint`).
pub type PathPoint = [f64; 3];

/// `greatCircle`'s default `stepKm`.
pub const DEFAULT_STEP_KM: f64 = 80.0;

/// Great-circle distance in metres (haversine).
pub fn distance_m(a: LonLat, b: LonLat) -> f64 {
    let (lon1, lat1, lon2, lat2) = (a[0] * DEG, a[1] * DEG, b[0] * DEG, b[1] * DEG);
    // `x ** 2` is `x * x` in JavaScript (fdlibm's pow special-cases an exponent of 2)
    let half_lat = ((lat2 - lat1) / 2.0).sin();
    let half_lon = ((lon2 - lon1) / 2.0).sin();
    let h = half_lat * half_lat + lat1.cos() * lat2.cos() * half_lon * half_lon;
    2.0 * EARTH_RADIUS_M * js_min(1.0, h.sqrt()).asin()
}

/// Great-circle distance in km.
pub fn distance_km(a: LonLat, b: LonLat) -> f64 {
    distance_m(a, b) / 1000.0
}

/// Initial course from `a` to `b`, degrees clockwise from true north, in [0, 360).
pub fn bearing(a: LonLat, b: LonLat) -> f64 {
    let (lon1, lat1, lon2, lat2) = (a[0] * DEG, a[1] * DEG, b[0] * DEG, b[1] * DEG);
    let y = (lon2 - lon1).sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * (lon2 - lon1).cos();
    (y.atan2(x) / DEG + 360.0) % 360.0
}

/// The point `m` metres from `a` on initial course `course` (degrees), its longitude in
/// [-180, 180).
pub fn destination(a: LonLat, course: f64, m: f64) -> LonLat {
    let d = m / EARTH_RADIUS_M;
    let (lon1, lat1, c) = (a[0] * DEG, a[1] * DEG, course * DEG);
    let lat2 = (lat1.sin() * d.cos() + lat1.cos() * d.sin() * c.cos()).asin();
    let lon2 = lon1 + (c.sin() * d.sin() * lat1.cos()).atan2(d.cos() - lat1.sin() * lat2.sin());
    [wrap180(lon2 / DEG), lat2 / DEG]
}

/// The point a fraction `f` of the way along the great circle from `a` to `b`; `a` itself when
/// the two (nearly) coincide. The longitude comes back in [-180, 180].
pub fn intermediate(a: LonLat, b: LonLat, f: f64) -> LonLat {
    let d = distance_m(a, b) / EARTH_RADIUS_M;
    if d < 1e-9 {
        return a;
    }
    let (lon1, lat1, lon2, lat2) = (a[0] * DEG, a[1] * DEG, b[0] * DEG, b[1] * DEG);
    let wa = ((1.0 - f) * d).sin() / d.sin();
    let wb = (f * d).sin() / d.sin();
    let x = wa * lat1.cos() * lon1.cos() + wb * lat2.cos() * lon2.cos();
    let y = wa * lat1.cos() * lon1.sin() + wb * lat2.cos() * lon2.sin();
    let z = wa * lat1.sin() + wb * lat2.sin();
    [y.atan2(x) / DEG, z.atan2(x.hypot(y)) / DEG]
}

/// Points along the great circle from `a` to `b`, about every `step_km` ([`DEFAULT_STEP_KM`] in
/// the TypeScript when left out): `a`, then between 1 and 255 points in between, then `b`, with
/// the longitudes made continuous by [`unwrap`]. Just `[a, b]` when the two coincide.
pub fn great_circle(a: LonLat, b: LonLat, step_km: f64) -> Vec<LonLat> {
    let km = distance_km(a, b);
    if km < 1e-6 {
        return vec![a, b];
    }
    let n = js_min(256.0, js_max(2.0, (km / step_km).ceil()));
    if n.is_nan() {
        // a NaN coordinate or step: `Array.from({ length: NaN })` is empty
        return Vec::new();
    }
    let n = n as usize;
    let points: Vec<LonLat> = (0..=n)
        .map(|i| {
            if i == 0 {
                a
            } else if i == n {
                b
            } else {
                intermediate(a, b, i as f64 / n as f64)
            }
        })
        .collect();
    unwrap(&points)
}

/// Keeps a path's longitudes continuous across the antimeridian: each point's longitude moves by
/// whole turns until it is within 180° of the previous point's (already moved) longitude. The
/// first point stays as it is; the other coordinates (latitude, altitude) are kept. Works on
/// [`LonLat`] and [`PathPoint`] alike (`N` must be at least 1).
pub fn unwrap<const N: usize>(points: &[[f64; N]]) -> Vec<[f64; N]> {
    let mut previous = points.first().map_or(0.0, |p| p[0]);
    points
        .iter()
        .map(|p| {
            let mut lon = p[0];
            while lon - previous > 180.0 {
                lon -= 360.0;
            }
            while lon - previous < -180.0 {
                lon += 360.0;
            }
            previous = lon;
            let mut q = *p;
            q[0] = lon;
            q
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;

    const LHR: LonLat = [-0.4614, 51.4775];
    const CDG: LonLat = [2.55, 49.0097];
    const HND: LonLat = [139.7798, 35.5494];
    const SFO: LonLat = [-122.375, 37.619];
    const ARN: LonLat = [17.9186, 59.6519];

    /// Degrees to 1e-9, the golden-route tolerance (the platforms' `sin`/`atan2` may differ in
    /// the last bit from V8's).
    fn assert_lon_lat(actual: LonLat, expected: LonLat) {
        assert_close!(actual[0], expected[0], 9);
        assert_close!(actual[1], expected[1], 9);
    }

    // Expected values below are the TypeScript's answers for the same inputs (bun 1.4).

    #[test]
    fn distance_m_matches_the_typescript() {
        let cases: [(LonLat, LonLat, f64); 9] = [
            (LHR, CDG, 348_014.003_718_310_5),
            (LHR, HND, 9_591_099.174_415_115),
            (HND, SFO, 8_286_074.624_814_691),
            (ARN, LHR, 1_461_964.049_454_837_8),
            ([0.0, 0.0], [180.0, 0.0], 20_015_086.796_020_57),
            ([10.0, 20.0], [10.0, 21.0], 111_194.926_644_558_89),
            ([0.0, 0.0], [1.0, 0.0], 111_194.926_644_558_74),
            ([0.0, 0.0], [-1.0, 0.0], 111_194.926_644_558_74),
            ([0.0, 0.0], [0.0, -1.0], 111_194.926_644_558_74),
        ];
        for (a, b, m) in cases {
            assert_close!(distance_m(a, b), m, 6);
            // symmetric
            assert_close!(distance_m(b, a), m, 6);
        }
        assert_eq!(distance_m([0.0, 0.0], [0.0, 0.0]), 0.0);
        assert_eq!(distance_m(LHR, LHR), 0.0);
    }

    #[test]
    fn distance_m_is_half_the_circumference_at_most() {
        // antipodes: h may round above 1, and `Math.min(1, ·)` keeps asin defined
        let d = distance_m([0.0, 0.0], [180.0, 0.0]);
        assert_close!(d, std::f64::consts::PI * EARTH_RADIUS_M, 6);
        let d = distance_m([30.0, 45.0], [-150.0, -45.0]);
        assert!(d.is_finite());
        assert_close!(d, std::f64::consts::PI * EARTH_RADIUS_M, 6);
    }

    #[test]
    fn distance_m_keeps_nan_as_math_min_does() {
        // `f64::min(NaN, 1.0)` would be 1 and give half the circumference
        assert!(distance_m([f64::NAN, 0.0], [1.0, 0.0]).is_nan());
        assert!(distance_m([0.0, 0.0], [1.0, f64::NAN]).is_nan());
    }

    #[test]
    fn distance_km_is_distance_m_in_km() {
        assert_close!(distance_km(LHR, CDG), 348.014_003_718_310_5, 9);
        assert_close!(distance_km(LHR, HND), 9_591.099_174_415_116, 9);
        assert_eq!(distance_km(ARN, LHR), distance_m(ARN, LHR) / 1000.0);
        assert_eq!(distance_km(CDG, CDG), 0.0);
    }

    #[test]
    fn bearing_matches_the_typescript() {
        assert_close!(bearing(LHR, CDG), 140.865_146_317_257_09, 9);
        assert_close!(bearing(LHR, HND), 31.430_677_396_790_372, 9);
        assert_close!(bearing(HND, SFO), 54.509_946_770_591_455, 9);
        assert_close!(bearing(ARN, LHR), 239.698_114_692_290_1, 9);
    }

    #[test]
    fn bearing_of_the_four_directions() {
        assert_eq!(bearing([10.0, 20.0], [10.0, 21.0]), 0.0);
        assert_eq!(bearing([0.0, 0.0], [1.0, 0.0]), 90.0);
        assert_eq!(bearing([0.0, 0.0], [0.0, -1.0]), 180.0);
        assert_eq!(bearing([0.0, 0.0], [-1.0, 0.0]), 270.0);
        assert_eq!(bearing([0.0, 0.0], [180.0, 0.0]), 90.0);
        // no course between a point and itself: atan2(0, 0) is 0
        assert_eq!(bearing([0.0, 0.0], [0.0, 0.0]), 0.0);
        assert_eq!(bearing(LHR, LHR), 0.0);
    }

    #[test]
    fn bearing_is_in_0_to_360() {
        for &(a, b) in &[(LHR, CDG), (CDG, LHR), (HND, SFO), (SFO, HND), (ARN, LHR), (LHR, ARN)] {
            let c = bearing(a, b);
            assert!((0.0..360.0).contains(&c), "{c}");
        }
    }

    #[test]
    fn destination_matches_the_typescript() {
        assert_lon_lat(destination(LHR, 90.0, 100_000.0), [0.982_359_563_538_921_1, 51.468_635_156_376_386]);
        assert_lon_lat(destination(ARN, 225.0, 1_234_567.0), [5.400_749_856_140_01, 51.079_770_715_821_71]);
        assert_eq!(destination([0.0, 0.0], 0.0, 0.0), [0.0, 0.0]);
    }

    #[test]
    fn destination_wraps_across_the_antimeridian() {
        let east = destination([179.9, 0.0], 90.0, 50_000.0);
        assert_lon_lat(east, [-179.650_339_197_040_58, 2.753_350_050_939_909e-17]);
        let west = destination([-179.9, 0.0], 270.0, 50_000.0);
        assert_lon_lat(west, [179.650_339_197_040_58, -8.260_050_152_819_726e-17]);
        // over the pole: the far side's meridian, 180 wrapped to -180
        let over = destination([0.0, 89.9], 0.0, 50_000.0);
        assert_lon_lat(over, [-180.0, 89.650_339_197_040_57]);
    }

    #[test]
    fn destination_inverts_distance_and_bearing() {
        for &(a, b) in &[(LHR, CDG), (LHR, HND), (HND, SFO), (ARN, LHR)] {
            let p = destination(a, bearing(a, b), distance_m(a, b));
            assert_close!(distance_m(p, b), 0.0, 3);
        }
    }

    #[test]
    fn intermediate_runs_along_the_great_circle() {
        // the midpoint of two points on the equator
        assert_lon_lat(intermediate([0.0, 0.0], [10.0, 0.0], 0.5), [5.0, 0.0]);
        // the ends
        assert_lon_lat(intermediate(LHR, HND, 0.0), LHR);
        assert_lon_lat(intermediate(LHR, HND, 1.0), HND);
        // a quarter of the way is a quarter of the distance, on the same course
        let total = distance_m(LHR, HND);
        let q = intermediate(LHR, HND, 0.25);
        assert_close!(distance_m(LHR, q), total / 4.0, 3);
        assert_close!(distance_m(q, HND), 3.0 * total / 4.0, 3);
        assert_close!(bearing(LHR, q), bearing(LHR, HND), 9);
    }

    #[test]
    fn intermediate_of_coincident_points_is_the_first() {
        assert_eq!(intermediate(LHR, LHR, 0.5), LHR);
        assert_eq!(intermediate([1.0, 2.0], [1.0, 2.0 + 1e-12], 0.5), [1.0, 2.0]);
    }

    #[test]
    fn great_circle_matches_the_typescript() {
        // (a, b, step, number of points, [first, second, middle, second to last, last])
        let cases: [(LonLat, LonLat, f64, usize, [LonLat; 5]); 4] = [
            (
                LHR,
                CDG,
                80.0,
                6,
                [
                    LHR,
                    [0.166_239_247_988_926_4, 50.990_282_512_277_26],
                    [1.382_676_051_732_110_6, 50.006_093_267_114_79],
                    [1.972_282_149_412_709_9, 49.509_364_551_657_96],
                    CDG,
                ],
            ),
            (
                HND,
                SFO,
                1000.0,
                10,
                [
                    HND,
                    [148.589_948_883_625_5, 40.043_349_086_969_705],
                    [194.066_932_202_890_11, 48.459_867_838_890_62],
                    [228.326_954_480_829_02, 41.803_430_369_449_54],
                    [237.625, 37.619],
                ],
            ),
            (
                LHR,
                HND,
                25.0,
                257,
                [
                    LHR,
                    [-0.177_507_730_832_149_32, 51.764_654_326_682_425],
                    [89.829_183_056_821_48, 69.115_138_913_395_8],
                    [139.613_850_196_794_9, 35.858_210_635_981_2],
                    HND,
                ],
            ),
            (
                CDG,
                ARN,
                2.0,
                257,
                [
                    CDG,
                    [2.596_179_526_269_757, 49.054_510_402_885_19],
                    [9.231_240_613_824_342, 54.571_133_605_825_12],
                    [17.840_925_687_205_758, 59.614_714_253_470_05],
                    ARN,
                ],
            ),
        ];
        for (a, b, step, len, picks) in cases {
            let g = great_circle(a, b, step);
            assert_eq!(g.len(), len);
            let got = [g[0], g[1], g[len / 2], g[len - 2], g[len - 1]];
            for (p, e) in got.into_iter().zip(picks) {
                assert_lon_lat(p, e);
            }
        }
    }

    #[test]
    fn great_circle_keeps_its_ends_exactly() {
        let g = great_circle(LHR, CDG, DEFAULT_STEP_KM);
        assert_eq!(g[0], LHR);
        assert_eq!(g[g.len() - 1], CDG);
        // across the antimeridian the last point is unwrapped, not moved
        let g = great_circle(HND, SFO, 1000.0);
        assert_eq!(g[g.len() - 1], [SFO[0] + 360.0, SFO[1]]);
    }

    #[test]
    fn great_circle_takes_at_least_two_and_at_most_256_segments() {
        // 348 km at 1000 km steps: still two segments
        assert_eq!(great_circle(LHR, CDG, 1000.0).len(), 3);
        // 348 km at 80 km: ceil(4.35) = 5
        assert_eq!(great_circle(LHR, CDG, 80.0).len(), 6);
        assert_eq!(great_circle(LHR, HND, 1.0).len(), 257);
        // a zero step is an infinite count, held at 256
        assert_eq!(great_circle(LHR, HND, 0.0).len(), 257);
    }

    #[test]
    fn great_circle_of_coincident_points_is_both() {
        assert_eq!(great_circle(LHR, LHR, 80.0), vec![LHR, LHR]);
        assert_eq!(great_circle([5.0, 5.0], [5.0, 5.0 + 1e-12], 80.0), vec![[5.0, 5.0], [5.0, 5.0 + 1e-12]]);
    }

    #[test]
    fn great_circle_of_nan_is_empty() {
        assert!(great_circle([f64::NAN, 0.0], CDG, 80.0).is_empty());
        assert!(great_circle(LHR, CDG, f64::NAN).is_empty());
    }

    #[test]
    fn great_circle_longitudes_are_continuous() {
        for &(a, b) in &[(HND, SFO), (SFO, HND), (LHR, HND)] {
            let g = great_circle(a, b, 25.0);
            for w in g.windows(2) {
                assert!((w[1][0] - w[0][0]).abs() <= 180.0, "{w:?}");
            }
        }
    }

    #[test]
    fn unwrap_keeps_longitudes_within_180_of_the_previous() {
        let path: Vec<LonLat> = vec![[170.0, 1.0], [179.0, 2.0], [-179.0, 3.0], [-170.0, 4.0], [175.0, 5.0]];
        assert_eq!(
            unwrap(&path),
            vec![[170.0, 1.0], [179.0, 2.0], [181.0, 3.0], [190.0, 4.0], [175.0, 5.0]]
        );
        let back: Vec<LonLat> = vec![[-170.0, 0.0], [175.0, 0.0], [-185.0, 0.0], [900.0, 0.0]];
        assert_eq!(unwrap(&back), vec![[-170.0, 0.0], [-185.0, 0.0], [-185.0, 0.0], [-180.0, 0.0]]);
    }

    #[test]
    fn unwrap_keeps_the_first_point_and_the_altitudes() {
        let path: Vec<PathPoint> = vec![[400.0, 10.0, 0.0], [-179.0, 11.0, 5000.0], [179.0, 12.0, 35_000.0]];
        assert_eq!(unwrap(&path), vec![[400.0, 10.0, 0.0], [541.0, 11.0, 5000.0], [539.0, 12.0, 35_000.0]]);
        // exactly 180 apart stays
        let half: Vec<LonLat> = vec![[0.0, 0.0], [180.0, 0.0], [0.0, 0.0], [-180.0, 0.0]];
        assert_eq!(unwrap(&half), half);
    }

    #[test]
    fn unwrap_of_nothing_is_nothing() {
        let empty: [LonLat; 0] = [];
        assert!(unwrap(&empty).is_empty());
    }
}
