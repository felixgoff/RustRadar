//! Port of `screen.ts` (doc 01 §3.14, doc 05 §11.6): directions on screen, for things drawn flat
//! against it.
//!
//! Transliterated as written: degrees become radians with `* DEG` and come back with `/ DEG`, and
//! `Math.max` is the private `js_max`, which keeps NaN as JavaScript does.

/// `Math.PI / 180`.
const DEG: f64 = std::f64::consts::PI / 180.0;
/// Metres per degree of latitude (and of longitude at the equator), as `screen.ts` takes it.
const METRES_PER_DEGREE: f64 = 111_320.0;

/// The part of a viewport this needs: geographic position to screen pixels (`screen.ts`
/// `Projector`, deck.gl's `viewport.project`).
pub trait Projector {
    /// `[lon, lat, z]` (degrees, degrees, metres up) to `[x, y]` screen pixels, y growing
    /// downwards from the top left.
    fn project(&self, position: [f64; 3]) -> [f64; 2];
}

/// `Math.max(a, b)`: NaN when either is NaN, and +0 above -0.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b || (a == b && a.is_sign_positive()) {
        a
    } else {
        b
    }
}

/// The angle, anticlockwise in degrees (deck.gl IconLayer's `getAngle`), that turns a
/// north-pointing icon to face `heading` at `[lon, lat, z]` as the camera sees it.
///
/// `bearing - heading` is only right where north points straight up the screen: the centre of a
/// flat, untilted view. On the globe, meridians lean and converge away from the centre, and
/// tilting foreshortens directions, so the heading is projected instead: the aircraft and a point
/// `step_m` metres ahead of it, and the icon points from one to the other.
///
/// `None` when the two project less than 1e-3 px apart or to non-finite values (behind the
/// camera): the caller falls back to `bearing - heading`.
pub fn screen_angle<P: Projector + ?Sized>(viewport: &P, [lon, lat, z]: [f64; 3], heading: f64, step_m: f64) -> Option<f64> {
    let h = heading * DEG;
    let ahead = [
        lon + (step_m * h.sin()) / (METRES_PER_DEGREE * js_max(0.01, (lat * DEG).cos())),
        lat + (step_m * h.cos()) / METRES_PER_DEGREE,
        z,
    ];
    let [x0, y0] = viewport.project([lon, lat, z]);
    let [x1, y1] = viewport.project(ahead);
    let dx = x1 - x0;
    let dy = y1 - y0;
    // too short to have a direction (or behind the camera): let the caller fall back
    if !dx.is_finite() || !dy.is_finite() || dx.hypot(dy) < 1e-3 {
        return None;
    }
    // clockwise from screen-up, where screen y grows downwards; IconLayer turns the other way
    Some(-dx.atan2(-dy) / DEG)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;

    /// deck.gl 9.4's `Viewport.project` for one fixed viewport: the position into deck's common
    /// space, then its `pixelProjectionMatrix` (column-major, as gl-matrix keeps it), divided by w.
    /// The matrices below are deck.gl's own for the views of `screen.test.ts`, printed by bun.
    struct DeckViewport {
        globe: bool,
        pixel_projection: [f64; 16],
    }

    impl Projector for DeckViewport {
        fn project(&self, [lng, lat, z]: [f64; 3]) -> [f64; 2] {
            let pi = std::f64::consts::PI;
            let common = if self.globe {
                // `GlobeViewport.projectPosition`
                let (lambda, phi) = (lng * DEG, lat * DEG);
                let d = (z / 6_370_972.0 + 1.0) * 256.0;
                [lambda.sin() * phi.cos() * d, -lambda.cos() * phi.cos() * d, phi.sin() * d]
            } else {
                // `WebMercatorViewport.projectPosition`: math.gl's `lngLatToWorld`, the shader's
                // latitude clamp, and `unitsPerMeter(lat)` for the height
                let (lambda, phi) = (lng * DEG, lat * DEG);
                let x = (512.0 * (lambda + pi)) / (2.0 * pi);
                let y = (512.0 * (pi + (pi / 4.0 + phi * 0.5).tan().ln())) / (2.0 * pi);
                [x, y.clamp(-318.0, 830.0), z * (512.0 / 40.03e6 / (lat * DEG).cos())]
            };
            let m = &self.pixel_projection;
            let v = [common[0], common[1], common[2], 1.0];
            let row = |r: usize| m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2] + m[12 + r] * v[3];
            let w = row(3);
            [row(0) / w, row(1) / w]
        }
    }

    /// `new WebMercatorViewport({ width: 800, height: 600, longitude: 8, latitude: 50, zoom: 8, bearing, pitch: 0 })`.
    fn mercator(bearing: i32) -> DeckViewport {
        let pixel_projection = match bearing {
            0 => [
                384.00000000000006, 0.0, 0.0, 0.0, 0.0, -384.0, 0.0, 0.0, -170.66666666666669, -128.0,
                -0.48697290930506487, -0.4266666666666667, -102073.06666666668, 130379.42415127152, 1.497879858657244, 1.5,
            ],
            35 => [
                314.55438500697284, -220.25335155880168, 0.0, 0.0, -220.2533515588017, -314.55438500697284, 0.0, 0.0,
                -170.66666666666669, -128.0, -0.48697290930506487, -0.4266666666666667, -8980.396371590105,
                165772.8050947056, 1.497879858657244, 1.5,
            ],
            -120 => [
                -191.99999999999994, 332.5537550532245, 0.0, 0.0, 332.5537550532245, 191.99999999999994, 0.0, 0.0,
                -170.66666666666669, -128.0, -0.48697290930506487, -0.4266666666666667, -60585.648680751205,
                -153432.19609342233, 1.497879858657244, 1.5,
            ],
            _ => unreachable!("no matrix for bearing {bearing}"),
        };
        DeckViewport { globe: false, pixel_projection }
    }

    /// `new GlobeViewport({ width: 800, height: 600, longitude: 0, latitude, zoom: 2 })`.
    // deck.gl's numbers as bun printed them, one of which happens to be 2/pi
    #[allow(clippy::approx_constant)]
    fn globe(latitude: i32) -> DeckViewport {
        let pixel_projection = match latitude {
            0 => [
                1.9098593171027443, 0.0, 0.0, 0.0, 0.8488263631567752, 0.6366197723675814, 0.0031391127334345083,
                0.002122065907891938, 0.0, -1.9098593171027443, 0.0, 0.0, 817.2995489681344, 612.9746617261009,
                1.782884885081438, 2.043248872420336,
            ],
            30 => [
                2.205315581687168, 0.0, 0.0, 0.0, 0.8488263631567751, -0.46603801847600246, 0.0030632905924921623,
                0.0021220659078919376, -0.490070129263815, -2.277411914050605, -0.0017685916481813976,
                -0.0012251753231595375, 850.9159061830733, 638.186929637305, 1.849060607046491, 2.1272897654576832,
            ],
            _ => unreachable!("no matrix for latitude {latitude}"),
        };
        DeckViewport { globe: true, pixel_projection }
    }

    /// Compare angles the short way round.
    fn close(a: Option<f64>, b: f64, tolerance: f64) {
        let a = a.expect("an angle");
        let d = ((((a - b) % 360.0) + 540.0) % 360.0) - 180.0;
        assert!(d.abs() < tolerance, "{a} is not within {tolerance} of {b}");
    }

    #[test]
    fn the_test_projector_is_deck_gls_project() {
        // `viewport.project` as bun printed it for the same viewports
        let [x, y] = mercator(0).project([8.01, 50.02, 0.0]);
        assert_close!(x, 403.6408888888933, 8);
        assert_close!(y, 288.6692076081332, 8);
        let [x, y] = globe(0).project([-50.0, 50.0, 1000.0]);
        assert_close!(x, 267.60961059883806, 8);
        assert_close!(y, 94.03711676128417, 8);
        let [x, y] = globe(30).project([-50.0, 50.0, 1000.0]);
        assert_close!(x, 232.75991992670865, 8);
        assert_close!(y, 144.84359440529042, 8);
    }

    #[test]
    fn matches_bearing_heading_at_the_centre_of_a_flat_untilted_view() {
        for bearing in [0, 35, -120] {
            let viewport = mercator(bearing);
            for heading in [0.0, 90.0, 200.0, 315.0] {
                close(screen_angle(&viewport, [8.0, 50.0, 0.0], heading, 500.0), f64::from(bearing) - heading, 0.5);
            }
        }
    }

    #[test]
    fn east_points_right_and_north_points_up_at_the_globes_centre() {
        let viewport = globe(0);
        close(screen_angle(&viewport, [0.0, 0.0, 0.0], 90.0, 10_000.0), -90.0, 0.5);
        close(screen_angle(&viewport, [0.0, 0.0, 0.0], 0.0, 10_000.0), 0.0, 0.5);
    }

    #[test]
    fn follows_the_meridians_lean_away_from_the_globes_centre() {
        let viewport = globe(30);
        // north-bound well west of centre: the meridian leans, so the icon must too,
        // where `bearing - heading` would keep it upright
        let angle = screen_angle(&viewport, [-50.0, 50.0, 0.0], 0.0, 10_000.0).unwrap();
        assert!(angle.abs() > 10.0);
        // and an east-bound aircraft there isn't simply turned a right angle from it
        let east = screen_angle(&viewport, [-50.0, 50.0, 0.0], 90.0, 10_000.0).unwrap();
        assert!((east - (angle - 90.0)).abs() > 1.0);
    }

    #[test]
    fn has_no_direction_for_a_zero_step() {
        let viewport = globe(0);
        assert_eq!(screen_angle(&viewport, [0.0, 0.0, 0.0], 90.0, 0.0), None);
    }

    #[test]
    fn the_angles_are_the_typescripts() {
        // screenAngle's own answers for the views above, printed by bun
        let m = mercator(35);
        let expected = [34.999999999808715, -55.00000000014486, -165.00080827466243, 79.99905370404626];
        for (heading, want) in [0.0, 90.0, 200.0, 315.0].into_iter().zip(expected) {
            assert_close!(screen_angle(&m, [8.0, 50.0, 0.0], heading, 500.0).unwrap(), want, 6);
        }
        let g = globe(30);
        assert_close!(screen_angle(&g, [0.0, 0.0, 0.0], 90.0, 10_000.0).unwrap(), -89.96984029275103, 6);
        assert_close!(screen_angle(&g, [-50.0, 50.0, 0.0], 0.0, 10_000.0).unwrap(), -37.7297102275341, 6);
        assert_close!(screen_angle(&g, [-50.0, 50.0, 0.0], 90.0, 10_000.0).unwrap(), -117.27399910575356, 6);
        let g = globe(0);
        assert_close!(screen_angle(&g, [-50.0, 50.0, 0.0], 0.0, 10_000.0).unwrap(), -51.250001176228416, 6);
        assert_close!(screen_angle(&g, [-50.0, 50.0, 0.0], 90.0, 10_000.0).unwrap(), -71.75553731317387, 6);
    }

    #[test]
    fn a_non_finite_projection_has_no_direction() {
        struct Behind;
        impl Projector for Behind {
            fn project(&self, [lon, _, _]: [f64; 3]) -> [f64; 2] {
                if lon == 0.0 { [0.0, 0.0] } else { [f64::NAN, f64::INFINITY] }
            }
        }
        assert_eq!(screen_angle(&Behind, [0.0, 0.0, 0.0], 90.0, 100.0), None);
    }
}
