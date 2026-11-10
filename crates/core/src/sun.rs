//! Port of `daynight.ts` and `shadow.ts` (S1.10; doc 05 §7.1–7.2, doc 01 §5.10): where the sun
//! stands, where it is night (below the horizon, and below the civil, nautical and astronomical
//! twilight altitudes), and the aircraft shadows it casts.
//!
//! Times are milliseconds as `f64`, the TypeScript's `number`: the clock they come from
//! (`motion.now()`) carries a smoothed server offset, so it is not always whole.
//!
//! Transliterated as written: degrees become radians with `* DEG` and come back with `/ DEG`, and
//! the `((x % 360) + 540) % 360 - 180` wraps are copied verbatim (JavaScript's `%` takes the sign
//! of the dividend, as Rust's does). They are not `num::wrap180`, which adds 180 first and so
//! rounds differently.

use std::f64::consts::PI;

use crate::geo::FEET_TO_M;
use crate::geo::sphere::LonLat;

const DEG: f64 = PI / 180.0;
/// Web Mercator's latitude limit; beyond it the flat map has no tiles.
const POLE: f64 = 85.0;

// ---- daynight.ts ----

/// A point on the globe in degrees: the subsolar point of [`subsolar_point`]
/// (`daynight.ts` `{ lat, lon }`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Subsolar {
    pub lat: f64,
    pub lon: f64,
}

/// Subsolar point (latitude = declination), low-precision solar almanac.
pub fn subsolar_point(time: f64) -> Subsolar {
    let d = time / 86_400_000.0 - 10_957.5; // days since J2000.0
    let g = (357.529 + 0.98560028 * d) * DEG;
    let q = 280.459 + 0.98564736 * d;
    let l = (q + 1.915 * g.sin() + 0.02 * (2.0 * g).sin()) * DEG;
    let e = (23.439 - 0.00000036 * d) * DEG;
    let right_ascension = (e.cos() * l.sin()).atan2(l.cos()) / DEG;
    let declination = (e.sin() * l.sin()).asin() / DEG;
    // equation of time, in degrees of rotation
    let eq_time = ((((q - right_ascension) % 360.0) + 540.0) % 360.0) - 180.0;
    let utc_hours = (time / 3_600_000.0) % 24.0;
    let lon = ((((-15.0 * (utc_hours - 12.0) - eq_time) % 360.0) + 540.0) % 360.0) - 180.0;
    Subsolar { lat: declination, lon }
}

/// Sun altitude (degrees) at a point.
fn sun_altitude(lat: f64, lon: f64, sun: Subsolar) -> f64 {
    let cos_zenith = (lat * DEG).sin() * (sun.lat * DEG).sin()
        + (lat * DEG).cos() * (sun.lat * DEG).cos() * ((lon - sun.lon) * DEG).cos();
    // Math.max(-1, Math.min(1, x)): NaN passes through, as it does through clamp
    cos_zenith.clamp(-1.0, 1.0).asin() / DEG
}

/// Latitudes `[first, last]` on a meridian where the sun is below `altitude`, or `None`.
fn below_on(lon: f64, altitude: f64, sun: Subsolar) -> Option<[f64; 2]> {
    // along a meridian the sun's altitude falls steadily toward one end, so
    // the night part is a single interval
    let mut lo: Option<f64> = None;
    let mut hi = 0.0;
    let mut lat = -POLE;
    while lat <= POLE {
        if sun_altitude(lat, lon, sun) < altitude {
            lo.get_or_insert(lat);
            hi = lat;
        }
        lat += 1.0;
    }
    lo.map(|lo| [lo, hi])
}

/// One 2° meridian strip of a night band, `[lon, a0], [lon + 2, b0], [lon + 2, b1], [lon, a1]`
/// (`[lon, lat]` in degrees), where `a` and `b` are the night's latitudes on the strip's west and
/// east meridians.
pub type NightQuad = [LonLat; 4];

/// The region where the sun is below `altitude` degrees, as one quad per
/// 2° meridian strip. (Walking from a pole fails when the pole itself is in
/// shallower twilight, as it is near the equinoxes.)
fn night_below(altitude: f64, sun: Subsolar) -> Vec<NightQuad> {
    const STEP: f64 = 2.0;
    let mut quads = Vec::new();
    let mut previous = below_on(-180.0, altitude, sun);
    let mut lon = -180.0;
    while lon < 180.0 {
        let next = below_on(lon + STEP, altitude, sun);
        // where the band ends between two meridians, taper it to a point
        let point = |[lo, hi]: [f64; 2]| [(lo + hi) / 2.0, (lo + hi) / 2.0];
        let sides = match (previous, next) {
            (None, None) => None,
            (Some(a), None) => Some((a, point(a))),
            (a, Some(b)) => Some((a.unwrap_or(point(b)), b)),
        };
        if let Some((a, b)) = sides {
            quads.push([[lon, a[0]], [lon + STEP, b[0]], [lon + STEP, b[1]], [lon, a[1]]]);
        }
        previous = next;
        lon += STEP;
    }
    quads
}

/// Sunset, then civil, nautical and astronomical twilight.
pub const TWILIGHT_STEPS: [f64; 4] = [0.0, -6.0, -12.0, -18.0];

/// Quads for every band, overlapping so the night deepens toward its middle: the quads of each of
/// [`TWILIGHT_STEPS`] in turn, each set from longitude -180 eastward (up to 180 per step).
pub fn night_polygons(time: f64) -> Vec<NightQuad> {
    let sun = subsolar_point(time);
    TWILIGHT_STEPS.iter().flat_map(|&altitude| night_below(altitude, sun)).collect()
}

// ---- shadow.ts ----
//
// Aircraft shadows on the ground, cast by the real sun.
//
// The map has no terrain: the ground is the plane z = 0 of each aircraft's
// local frame. A shadow is the aircraft's own mesh squashed onto that plane
// along the sun's rays (an oblique projection), so it takes the aircraft's
// heading, bank and pitch, and lands `height / tan(sun altitude)` away from
// the point straight below it.

/// The sun's direction from the Earth's centre (x through 0°E on the equator, z north), unit length.
pub fn sun_direction(time: f64) -> [f64; 3] {
    let Subsolar { lat, lon } = subsolar_point(time);
    let cos_lat = (lat * DEG).cos();
    [cos_lat * (lon * DEG).cos(), cos_lat * (lon * DEG).sin(), (lat * DEG).sin()]
}

/// The direction toward the sun at a point, as `[east, north, up]` (unit length;
/// `up` is the sine of the sun's altitude, negative at night). `sun` is [`sun_direction`]'s.
pub fn local_sun(sun: [f64; 3], lat: f64, lon: f64) -> [f64; 3] {
    let sin_lat = (lat * DEG).sin();
    let cos_lat = (lat * DEG).cos();
    let sin_lon = (lon * DEG).sin();
    let cos_lon = (lon * DEG).cos();
    let [x, y, z] = sun;
    [
        -sin_lon * x + cos_lon * y,
        -sin_lat * cos_lon * x - sin_lat * sin_lon * y + cos_lat * z,
        cos_lat * cos_lon * x + cos_lat * sin_lon * y + sin_lat * z,
    ]
}

/// Model matrix (column-major 4x4, `SimpleMeshLayer`'s `getTransformMatrix`)
/// that draws a mesh's shadow on the ground.
///
/// `orientation` and `scale` are what the aircraft itself is drawn with
/// (deck.gl's `[pitch, yaw, roll]` in degrees, and per-axis scale); `to_sun`
/// points toward the sun in the same local frame the mesh is drawn in, and
/// `height` is how far (metres, as drawn) the mesh's origin is above the
/// map. Each vertex `p` of the oriented mesh, at `(0, 0, height) + p`, moves
/// along the sun's rays to the ground plane `z = ground_z` (0 in the TypeScript's
/// default). The sun must be above the horizon.
pub fn shadow_matrix(
    orientation: [f64; 3],
    scale: [f64; 3],
    to_sun: [f64; 3],
    height: f64,
    ground_z: f64,
) -> [f64; 16] {
    let mut out = [0.0; 16];
    shadow_matrix_into(orientation, scale, to_sun, height, ground_z, &mut out);
    out
}

/// [`shadow_matrix`] written into `out` (every entry), which it returns: the TypeScript's `out`
/// argument, so a frame's worth of aircraft can share one buffer.
pub fn shadow_matrix_into(
    orientation: [f64; 3],
    scale: [f64; 3],
    to_sun: [f64; 3],
    height: f64,
    ground_z: f64,
    out: &mut [f64; 16],
) -> &mut [f64; 16] {
    // deck.gl's rotation (@deck.gl/mesh-layers utils/matrix.ts), columns scaled
    let sp = (orientation[0] * DEG).sin();
    let cp = (orientation[0] * DEG).cos();
    let sw = (orientation[1] * DEG).sin();
    let cw = (orientation[1] * DEG).cos();
    let sr = (orientation[2] * DEG).sin();
    let cr = (orientation[2] * DEG).cos();
    let [sx, sy, sz] = scale;
    let m00 = sx * cw * cp;
    let m10 = sx * sw * cp;
    let m20 = sx * -sp;
    let m01 = sy * (-sw * cr + cw * sp * sr);
    let m11 = sy * (cw * cr + sw * sp * sr);
    let m21 = sy * cp * sr;
    let m02 = sz * (sw * sr + cw * sp * cr);
    let m12 = sz * (-cw * sr + sw * sp * cr);
    let m22 = sz * cp * cr;
    // ground point reached from (x, y, z) along the ray: (x - kx (z - g), y - ky (z - g), g)
    let kx = to_sun[0] / to_sun[2];
    let ky = to_sun[1] / to_sun[2];
    out[0] = m00 - kx * m20;
    out[1] = m10 - ky * m20;
    out[2] = 0.0;
    out[3] = 0.0;
    out[4] = m01 - kx * m21;
    out[5] = m11 - ky * m21;
    out[6] = 0.0;
    out[7] = 0.0;
    out[8] = m02 - kx * m22;
    out[9] = m12 - ky * m22;
    out[10] = 0.0;
    out[11] = 0.0;
    out[12] = -kx * (height - ground_z);
    out[13] = -ky * (height - ground_z);
    out[14] = ground_z;
    out[15] = 1.0;
    out
}

/// Darkest a shadow gets: on the ground, under a high sun.
pub const SHADOW_MAX_ALPHA: f64 = 0.5;
/// Fainter than this, a shadow is not drawn at all.
pub const SHADOW_MIN_ALPHA: f64 = 0.05;
/// The sun's angular diameter, radians: it sets how fast a shadow blurs with distance.
const SUN_DIAMETER: f64 = 0.53 * DEG;
/// An airframe's typical member (fuselage, wing chord) against its largest dimension.
const MEMBER_FRACTION: f64 = 0.1;
/// Below this sun altitude (degrees) shadows fade: the light is dim, reddened and diffuse.
const LOW_SUN_DEG: f64 = 6.0;

/// How strongly the sun casts shadows (0..1), from its altitude sine: none at
/// night, rising from the horizon to full strength at `LOW_SUN_DEG` (6°).
/// Aircraft and buildings fade by the same measure, so their shadows agree.
pub fn sunlight(sin_sun: f64) -> f64 {
    // `!(sinSun > 0)`: NaN casts nothing too
    if sin_sun.is_nan() || sin_sun <= 0.0 {
        return 0.0;
    }
    (sin_sun.min(1.0).asin() / DEG / LOW_SUN_DEG).min(1.0)
}

/// Opacity (0..1) of the shadow of an aircraft `size_m` long at `alt_ft` above
/// the ground, with the sun's altitude sine `sin_sun`.
///
/// The sun is a disc, not a point: past `width / 0.53°` along the ray, an
/// object of that width casts no umbra, only a blur that pales as it spreads.
/// An airframe's fuselage and wing chord are about a tenth of its length, so
/// the shadow is crisp up to a slant distance of about 11 lengths (an A320:
/// ~400 m) and then loses contrast: in proportion to the distance for a long
/// thin shape, with its square for a compact one; an airframe is in between,
/// so the power is 1.5. Under a high sun an A320's shadow is faint by
/// 3,000 ft and gone by about 5,500 ft, a light aircraft's by about 2,000 ft,
/// a 777's by about 9,000 ft; a low sun, whose rays are longer, fades them
/// all sooner. None at night.
pub fn shadow_opacity(alt_ft: f64, size_m: f64, sin_sun: f64) -> f64 {
    let light = sunlight(sin_sun);
    if light == 0.0 {
        return 0.0;
    }
    // Math.max(0, altFt): NaN stays NaN (f64::max would give 0), -0 becomes 0
    let above = if alt_ft.is_nan() || alt_ft > 0.0 { alt_ft } else { 0.0 };
    let slant = (above * FEET_TO_M) / sin_sun;
    let crisp = (MEMBER_FRACTION * size_m) / SUN_DIAMETER;
    let contrast = if slant <= crisp { 1.0 } else { (crisp / slant).powf(1.5) };
    let alpha = SHADOW_MAX_ALPHA * contrast * light;
    if alpha < SHADOW_MIN_ALPHA { 0.0 } else { alpha }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;

    type V3 = [f64; 3];
    type M3 = [V3; 3]; // rows

    fn mul(a: M3, b: M3) -> M3 {
        std::array::from_fn(|i| {
            std::array::from_fn(|j| a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j])
        })
    }
    fn apply(m: M3, v: V3) -> V3 {
        std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
    }
    fn rot_x(a: f64) -> M3 {
        [[1.0, 0.0, 0.0], [0.0, a.cos(), -a.sin()], [0.0, a.sin(), a.cos()]]
    }
    fn rot_y(a: f64) -> M3 {
        [[a.cos(), 0.0, a.sin()], [0.0, 1.0, 0.0], [-a.sin(), 0.0, a.cos()]]
    }
    fn rot_z(a: f64) -> M3 {
        [[a.cos(), -a.sin(), 0.0], [a.sin(), a.cos(), 0.0], [0.0, 0.0, 1.0]]
    }

    /// deck.gl's `[pitch, yaw, roll]`: yaw about z, after pitch about y, after roll about x.
    fn deck_rotation([p, y, r]: V3) -> M3 {
        mul(rot_z(y * DEG), mul(rot_y(p * DEG), rot_x(r * DEG)))
    }

    /// Where a column-major 4x4 takes a point.
    fn transform(m: &[f64; 16], [x, y, z]: V3) -> V3 {
        [
            m[0] * x + m[4] * y + m[8] * z + m[12],
            m[1] * x + m[5] * y + m[9] * z + m[13],
            m[2] * x + m[6] * y + m[10] * z + m[14],
        ]
    }

    fn near(a: &[f64], b: &[f64], tolerance: f64) {
        for (i, v) in a.iter().enumerate() {
            assert!((v - b[i]).abs() < tolerance, "{a:?} against {b:?}: entry {i} off by {tolerance} or more");
        }
    }

    /// `Math.hypot(...v)`.
    fn hypot(v: V3) -> f64 {
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    }

    fn normalize(v: V3) -> V3 {
        let n = hypot(v);
        v.map(|c| c / n)
    }

    // ---- shadowMatrix ----

    #[test]
    fn projects_the_oriented_scaled_mesh_onto_the_ground_along_the_suns_rays() {
        let cases: [(V3, V3, V3, f64); 4] = [
            ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.0, 0.0, 1.0], 0.0),
            ([12.0, 180.0 - 75.0, 4.0], [2.0, 2.0, 2.0], normalize([0.3, -0.5, 0.8]), 150.0),
            ([-25.0, 33.0, -8.0], [0.5, 0.5, 0.02], normalize([-0.9, 0.1, 0.2]), 900.0),
            ([5.0, -140.0, 15.0], [3.0, 3.0, 3.0], normalize([0.01, 0.6, 0.05]), 40.0),
        ];
        let vertices: [V3; 6] = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [3.5, -2.0, 1.25],
            [-17.0, 4.0, -0.6],
        ];
        for (orientation, scale, sun, height) in cases {
            let m = shadow_matrix(orientation, scale, sun, height, 0.0);
            let rotation = deck_rotation(orientation);
            for v in vertices {
                // the vertex as the aircraft draws it, `height` above the ground
                let [x, y, z] = apply(rotation, [v[0] * scale[0], v[1] * scale[1], v[2] * scale[2]]);
                let lifted: V3 = [x, y, z + height];
                // follow the ray away from the sun down to z = 0
                let t = lifted[2] / sun[2];
                let ground: V3 = [lifted[0] - t * sun[0], lifted[1] - t * sun[1], 0.0];
                near(&transform(&m, v), &ground, 1e-6);
            }
        }
    }

    #[test]
    fn with_the_sun_overhead_the_shadow_is_the_plan_view_straight_below() {
        let m = shadow_matrix([10.0, 60.0, -20.0], [1.0, 1.0, 1.0], [0.0, 0.0, 1.0], 500.0, 0.0);
        // toBe(-0): Object.is tells -0 from 0
        assert!(m[12] == 0.0 && m[12].is_sign_negative(), "{}", m[12]);
        assert!(m[13] == 0.0 && m[13].is_sign_negative(), "{}", m[13]);
        let rotation = deck_rotation([10.0, 60.0, -20.0]);
        let [x, y, _] = apply(rotation, [0.0, 10.0, 0.0]);
        near(&transform(&m, [0.0, 10.0, 0.0]), &[x, y, 0.0], 1e-9);
    }

    #[test]
    fn a_point_at_the_origin_lands_height_tan_sun_altitude_away_from_the_sun() {
        // sun 30 degrees up, due east (+x)
        let sun: V3 = [(30.0 * DEG).cos(), 0.0, (30.0 * DEG).sin()];
        let m = shadow_matrix([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], sun, 100.0, 0.0);
        near(&transform(&m, [0.0, 0.0, 0.0]), &[-100.0 / (30.0 * DEG).tan(), 0.0, 0.0], 1e-9);
    }

    #[test]
    fn lands_on_a_ground_plane_below_the_map_meeting_the_wheels_there() {
        let sun = normalize([0.4, -0.2, 0.7]);
        // wheels 6 m below the origin, which stands on the map: they touch the plane z = -6
        let m = shadow_matrix([3.0, 20.0, -2.0], [1.0, 1.0, 1.0], sun, 0.0, -6.0);
        let wheel: V3 = apply(deck_rotation([3.0, 20.0, -2.0]), [0.0, 0.0, -6.0]);
        let shadow = transform(&m, [0.0, 0.0, -6.0]);
        assert_eq!(shadow[2], -6.0);
        // a point on the plane is its own shadow
        let on_plane = shadow_matrix([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], sun, 0.0, -6.0);
        near(&transform(&on_plane, [0.0, 0.0, -6.0]), &[0.0, 0.0, -6.0], 1e-9);
        // and in general it follows the ray down to the plane
        let t = (wheel[2] + 6.0) / sun[2];
        near(&shadow, &[wheel[0] - t * sun[0], wheel[1] - t * sun[1], -6.0], 1e-9);
    }

    #[test]
    fn writes_into_the_array_it_is_given() {
        let mut out = [7.0; 16];
        let returned: *const [f64; 16] =
            shadow_matrix_into([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.0, 0.0, 1.0], 0.0, 0.0, &mut out);
        assert!(std::ptr::eq(returned, &out));
        assert_eq!(out[15], 1.0);
        // every entry is written, none left as it was
        assert_eq!(out, shadow_matrix([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.0, 0.0, 1.0], 0.0, 0.0));
    }

    // ---- localSun ----

    /// `Date.UTC(2026, 5, 21, 15, 30)`.
    const TIME: f64 = 1_782_055_800_000.0;

    #[test]
    fn is_a_unit_vector() {
        let sun = sun_direction(TIME);
        for [lat, lon] in [[0.0, 0.0], [51.5, -0.1], [-33.9, 151.2], [64.0, -150.0]] {
            assert!((hypot(local_sun(sun, lat, lon)) - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn puts_the_sun_overhead_at_the_subsolar_point() {
        let sun = sun_direction(TIME);
        let subsolar = subsolar_point(TIME);
        near(&local_sun(sun, subsolar.lat, subsolar.lon), &[0.0, 0.0, 1.0], 1e-12);
    }

    #[test]
    fn puts_the_sun_on_the_horizon_a_quarter_turn_round_the_equator_and_below_it_opposite() {
        let sun = sun_direction(TIME);
        let subsolar = subsolar_point(TIME);
        assert!(local_sun(sun, 0.0, subsolar.lon + 90.0)[2].abs() < 1e-9);
        assert_close!(local_sun(sun, -subsolar.lat, subsolar.lon + 180.0)[2], -1.0, 9);
    }

    #[test]
    fn points_east_of_a_place_in_its_morning_and_north_of_it_beyond_the_tropic() {
        let sun = sun_direction(TIME);
        let subsolar = subsolar_point(TIME);
        // 45 degrees west of the subsolar point it is morning: the sun is in the east
        let [east, _, up] = local_sun(sun, subsolar.lat, subsolar.lon + -45.0);
        assert!(east > 0.6, "{east}");
        assert!(up > 0.6, "{up}");
        // in June the sun stands north of noon in the southern hemisphere
        let [e2, north, up2] = local_sun(sun, -40.0, subsolar.lon);
        assert!(e2.abs() < 1e-9, "{e2}");
        assert!(north > 0.0, "{north}");
        // and 40 + 23.4 degrees from the zenith
        assert_close!(up2.acos() / DEG, 40.0 + subsolar.lat, 6);
    }

    // ---- shadowOpacity ----

    const A320: f64 = 37.6;
    const C172: f64 = 11.0;

    fn high() -> f64 {
        (60.0 * DEG).sin()
    }

    #[test]
    fn is_darkest_on_the_ground() {
        assert_eq!(shadow_opacity(0.0, A320, high()), SHADOW_MAX_ALPHA);
        assert_eq!(shadow_opacity(200.0, A320, high()), SHADOW_MAX_ALPHA);
    }

    #[test]
    fn fades_with_height_and_is_gone_at_cruise() {
        let at = |ft: f64| shadow_opacity(ft, A320, high());
        assert!(at(2000.0) < at(500.0));
        assert!(at(3000.0) < 0.15, "{}", at(3000.0));
        assert_eq!(at(6000.0), 0.0);
        assert_eq!(at(36000.0), 0.0);
    }

    #[test]
    fn small_aircraft_lose_their_shadow_lower() {
        assert_eq!(shadow_opacity(2000.0, C172, high()), 0.0);
        assert!(shadow_opacity(2000.0, A320, high()) > 0.1);
    }

    #[test]
    fn none_at_night_faint_with_the_sun_on_the_horizon() {
        assert_eq!(shadow_opacity(0.0, A320, -0.1), 0.0);
        assert_eq!(shadow_opacity(0.0, A320, 0.0), 0.0);
        assert_close!(shadow_opacity(0.0, A320, (3.0 * DEG).sin()), SHADOW_MAX_ALPHA / 2.0, 6);
        // long rays: a low sun takes the shadow away sooner
        assert!(shadow_opacity(1000.0, A320, (10.0 * DEG).sin()) < shadow_opacity(1000.0, A320, high()));
    }

    // ---- new: subsolarPoint, nightPolygons (no TypeScript tests) ----

    // Times below are `Date.UTC(...)`; the expected values are the TypeScript's answers for the
    // same inputs (bun 1.4.4).
    /// `Date.UTC(2026, 5, 21, 12, 0)`: the June solstice.
    const JUNE: f64 = 1_782_043_200_000.0;
    /// `Date.UTC(2026, 2, 20, 12, 0)`: the March equinox.
    const MARCH: f64 = 1_774_008_000_000.0;
    /// `Date.UTC(2026, 8, 23, 3, 17)`: the September equinox, small hours in Europe.
    const SEPTEMBER: f64 = 1_790_133_420_000.0;
    /// `Date.UTC(2026, 11, 21, 6, 0)`: the December solstice.
    const DECEMBER: f64 = 1_797_832_800_000.0;
    /// Not a whole millisecond, as `motion.now()` can be.
    const FRACTIONAL: f64 = 1_781_234_567_890.25;

    #[test]
    fn subsolar_point_and_sun_direction_match_the_typescript() {
        // time, subsolar latitude and longitude, sun direction
        let cases: [(f64, f64, f64, V3); 7] = [
            (
                JUNE,
                23.435443652655245,
                0.4566753663439158,
                [0.9174796271506364, 0.007312915912208661, 0.3977155453642591],
            ),
            (
                MARCH,
                -0.043994469182766796,
                1.8660355394804355,
                [0.9994693998104309, 0.03256269724116503, -0.0007678482644537073],
            ),
            (
                DECEMBER,
                -23.433934769271726,
                89.48461810215849,
                [0.008253076173686811, 0.9174821256574318, -0.39769138264839443],
            ),
            (
                TIME,
                23.435221244550632,
                -52.035393107898926,
                [0.5644290253167381, -0.7233567953004924, 0.39771198381818806],
            ),
            // J2000.0 itself, Date.UTC(2000, 0, 1, 12, 0)
            (
                946_728_000_000.0,
                -23.033503799852458,
                0.8257934350725691,
                [0.9201806329543148, 0.013263311067075501, -0.3912693283600624],
            ),
            (
                FRACTIONAL,
                23.14374771204471,
                129.25486742275825,
                [-0.5818467463649808, 0.712021385498291, 0.39303932416181964],
            ),
            // before 1970: `%` keeps the dividend's sign, and the + 540 puts it right
            (
                -315_576_000_000.0,
                -23.06130693622578,
                0.8145358854835649,
                [0.9199932661435907, 0.013079811873592254, -0.39171585208132637],
            ),
        ];
        for (time, lat, lon, direction) in cases {
            let s = subsolar_point(time);
            near(&[s.lat, s.lon], &[lat, lon], 1e-11);
            near(&sun_direction(time), &direction, 1e-13);
            assert!((-180.0..180.0).contains(&s.lon), "{}", s.lon);
        }
    }

    /// `night_polygons`'s quads split by threshold, as `night_below` gives them.
    fn bands(time: f64) -> Vec<Vec<NightQuad>> {
        let sun = subsolar_point(time);
        TWILIGHT_STEPS.iter().map(|&altitude| night_below(altitude, sun)).collect()
    }

    #[test]
    fn night_polygons_quad_counts_per_threshold() {
        // at a solstice the winter pole is in astronomical night all day, so every strip of every
        // band has some night; at an equinox the night is a band that narrows as it deepens
        let cases: [(f64, [usize; 4]); 5] = [
            (JUNE, [180, 180, 180, 180]),
            (DECEMBER, [180, 180, 180, 180]),
            (FRACTIONAL, [180, 180, 180, 180]),
            (MARCH, [92, 85, 79, 73]),
            (SEPTEMBER, [91, 85, 79, 73]),
        ];
        for (time, counts) in cases {
            let bands = bands(time);
            assert_eq!(bands.iter().map(Vec::len).collect::<Vec<_>>(), counts, "at {time}");
            // night_polygons is the bands one after another, in TWILIGHT_STEPS order
            assert_eq!(night_polygons(time), bands.concat());
            for band in &bands {
                for (i, quad) in band.iter().enumerate() {
                    // 2° strips, from west to east, inside the Mercator limits
                    let lon = quad[0][0];
                    assert_eq!([quad[3][0], quad[1][0], quad[2][0]], [lon, lon + 2.0, lon + 2.0]);
                    assert!((-180.0..180.0).contains(&lon) && lon % 2.0 == 0.0, "{quad:?}");
                    if i > 0 {
                        assert!(lon > band[i - 1][0][0]);
                    }
                    for [_, lat] in quad {
                        assert!((-POLE..=POLE).contains(lat), "{quad:?}");
                    }
                    assert!(quad[0][1] <= quad[3][1] && quad[1][1] <= quad[2][1], "{quad:?}");
                }
            }
        }
        // June: from the south pole to the edge of the midnight sun, which recedes as night deepens
        let june = bands(JUNE);
        let first: Vec<NightQuad> = june.iter().map(|band| band[0]).collect();
        assert_eq!(
            first,
            [
                [[-180.0, -85.0], [-178.0, -85.0], [-178.0, 66.0], [-180.0, 66.0]],
                [[-180.0, -85.0], [-178.0, -85.0], [-178.0, 60.0], [-180.0, 60.0]],
                [[-180.0, -85.0], [-178.0, -85.0], [-178.0, 54.0], [-180.0, 54.0]],
                [[-180.0, -85.0], [-178.0, -85.0], [-178.0, 48.0], [-180.0, 48.0]],
            ]
        );
    }

    #[test]
    fn night_polygons_taper_to_a_point_where_the_night_ends() {
        // the strips with night on one meridian and none on the other
        let tapers = |time: f64| -> Vec<Vec<NightQuad>> {
            let sun = subsolar_point(time);
            TWILIGHT_STEPS
                .iter()
                .map(|&altitude| {
                    let dark = |lon: f64| below_on(lon, altitude, sun).is_some();
                    let quads = night_below(altitude, sun);
                    quads.into_iter().filter(|q| dark(q[0][0]) != dark(q[1][0])).collect()
                })
                .collect()
        };
        // there the band narrows to the midpoint of the night on the side that has it
        for time in [JUNE, MARCH, SEPTEMBER, DECEMBER, FRACTIONAL] {
            let sun = subsolar_point(time);
            for (altitude, band) in TWILIGHT_STEPS.iter().zip(tapers(time)) {
                for q in band {
                    let (point, side) = if below_on(q[0][0], *altitude, sun).is_none() {
                        ([q[0][1], q[3][1]], [q[1][1], q[2][1]])
                    } else {
                        ([q[1][1], q[2][1]], [q[0][1], q[3][1]])
                    };
                    let middle = (side[0] + side[1]) / 2.0;
                    assert_eq!(point, [middle, middle], "{q:?} at {time}");
                }
            }
        }
        assert_eq!(
            tapers(MARCH),
            [
                vec![
                    [[-88.0, 72.0], [-86.0, 78.5], [-86.0, 78.5], [-88.0, 85.0]],
                    [[90.0, 7.0], [92.0, -71.0], [92.0, 85.0], [90.0, 7.0]],
                ],
                vec![
                    [[-96.0, -39.0], [-94.0, 0.5], [-94.0, 0.5], [-96.0, 40.0]],
                    [[96.0, 0.5], [98.0, -11.0], [98.0, 12.0], [96.0, 0.5]],
                ],
                vec![
                    [[-102.0, -29.0], [-100.0, 0.5], [-100.0, 0.5], [-102.0, 30.0]],
                    [[102.0, 0.0], [104.0, -8.0], [104.0, 8.0], [102.0, 0.0]],
                ],
                vec![
                    [[-108.0, -24.0], [-106.0, 0.0], [-106.0, 0.0], [-108.0, 24.0]],
                    [[108.0, 0.0], [110.0, -6.0], [110.0, 6.0], [108.0, 0.0]],
                ],
            ]
        );
        // at 03:17 UTC in September the night lies clear of the antimeridian: each band starts and
        // ends with a taper, and has no other
        for (band, tapered) in bands(SEPTEMBER).into_iter().zip(tapers(SEPTEMBER)) {
            assert_eq!(tapered, [band[0], band[band.len() - 1]]);
        }
        assert_eq!(bands(SEPTEMBER)[0][0], [[-142.0, 0.0], [-140.0, -85.0], [-140.0, 85.0], [-142.0, 0.0]]);
        // at a solstice every meridian has some night, so nothing tapers
        for time in [JUNE, DECEMBER] {
            assert!(tapers(time).iter().all(Vec::is_empty), "at {time}");
        }
    }

    #[test]
    fn night_polygons_match_the_typescript() {
        // every coordinate is a whole or half degree, so this weighted sum is exact: it changes
        // with any vertex, and with the order of the vertices
        let checksum = |quads: &[NightQuad]| -> f64 {
            let mut sum = 0.0;
            for (k, [lon, lat]) in quads.iter().flatten().enumerate() {
                sum += (k + 1) as f64 * (lon + 1000.0 * lat);
            }
            sum
        };
        let cases: [(f64, usize, f64); 5] = [
            (JUNE, 720, -218_421_963_920.0),
            (MARCH, 329, 279_575_266.0),
            (SEPTEMBER, 328, 142_598_236.0),
            (DECEMBER, 720, 231_786_473_080.0),
            (FRACTIONAL, 720, -227_239_247_920.0),
        ];
        for (time, count, sum) in cases {
            let quads = night_polygons(time);
            assert_eq!(quads.len(), count, "at {time}");
            assert_eq!(checksum(&quads), sum, "at {time}");
        }
    }
}
