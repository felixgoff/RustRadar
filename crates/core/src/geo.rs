//! Port of `geo.ts` (doc 05 §6.2; docs 01 §3.13, 02 §1.1): the basemap choice, the aircraft and
//! altitude colours, rendered elevation and its exaggeration, ground resolution, and the
//! visibility helpers that decide what is drawn and fetched around the view centre.
//!
//! [`sphere`] holds the great-circle helpers of `flightpath.ts:76-133`.
//!
//! Transliterated as written: degrees become radians with `* DEG` and come back with `/ DEG`, the
//! `%` wraps are copied verbatim (JavaScript's `%` is Rust's), `Math.round` is
//! [`js_round`], and `Math.min`/`Math.max` are the private `js_min`/`js_max`, which keep NaN as
//! JavaScript does (`f64::min`/`f64::max` return the other argument instead).

pub mod sphere;

use serde::{Deserialize, Serialize};

use crate::num::{clamp01, js_round};

/// An 8-bit colour (`geo.ts` `RGB`).
pub type Rgb = [u8; 3];

/// The dark map, the satellite imagery (darkened, so the traffic stands out), or a realistic 3D
/// world on the imagery. Stored in the settings as `"dark"`, `"satellite"`, `"realistic"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Basemap {
    /// The default (`filters.ts` `DEFAULT_SETTINGS.basemap`).
    #[default]
    Dark,
    Satellite,
    Realistic,
}

impl Basemap {
    /// The name in the settings and in the TypeScript.
    pub fn as_str(self) -> &'static str {
        match self {
            Basemap::Dark => "dark",
            Basemap::Satellite => "satellite",
            Basemap::Realistic => "realistic",
        }
    }
}

/// `Math.PI / 180`.
const DEG: f64 = std::f64::consts::PI / 180.0;
/// The sphere of every great-circle formula here and in [`sphere`].
pub const EARTH_RADIUS_M: f64 = 6_371_000.0;
/// The Web Mercator equator, for the ground resolution.
pub const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.686;
pub const KNOTS_TO_MS: f64 = 0.514444;
pub const FEET_TO_M: f64 = 0.3048;
/// Don't extrapolate positions further than this past the last update.
#[cfg(test)]
const MAX_EXTRAPOLATION_S: f64 = 90.0;

/// Every aircraft is white...
pub const AIRCRAFT_COLOR: Rgb = [255, 255, 255];
/// ...except the selected one, in the selection colour (`--amber` in the CSS, which is in fact the
/// blue `#378ef0`).
pub const SELECTED_COLOR: Rgb = [55, 142, 240];

/// Altitude (feet) → colour stops for the selected flight's trail, low to high.
pub const ALTITUDE_STOPS: [(f64, Rgb); 7] = [
    (0.0, [255, 98, 0]),
    (4_000.0, [255, 196, 0]),
    (8_000.0, [160, 230, 40]),
    (15_000.0, [40, 220, 130]),
    (25_000.0, [0, 200, 230]),
    (35_000.0, [70, 125, 255]),
    (45_000.0, [205, 85, 255]),
];

/// `Math.min(a, b)`: NaN when either is NaN, and -0 below +0.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b || (a == b && a.is_sign_negative()) {
        a
    } else {
        b
    }
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

/// The trail colour at `alt_ft`: linear between [`ALTITUDE_STOPS`], each channel rounded as
/// `Math.round` does, held at the first stop below it and at the last above it (and for NaN).
pub fn altitude_color(alt_ft: f64) -> Rgb {
    let stops = &ALTITUDE_STOPS;
    if alt_ft <= stops[0].0 {
        return stops[0].1;
    }
    for pair in stops.windows(2) {
        let (a1, c1) = pair[1];
        if alt_ft <= a1 {
            let (a0, c0) = pair[0];
            let t = (alt_ft - a0) / (a1 - a0);
            return [0, 1, 2].map(|k| {
                let (from, to) = (f64::from(c0[k]), f64::from(c1[k]));
                // between two channel values, so within 0..=255
                js_round(from + (to - from) * t) as u8
            });
        }
    }
    stops[stops.len() - 1].1
}

/// Elevation in metres for rendering, exaggerated so altitude reads on a globe; nothing below
/// the ground.
pub fn elevation(alt_ft: f64, exaggeration: f64) -> f64 {
    js_max(0.0, alt_ft) * FEET_TO_M * exaggeration
}

/// The exaggeration fades to true scale between zoom 6 and 11: at street level aircraft must sit
/// at their real height next to buildings.
pub fn effective_exaggeration(exaggeration: f64, zoom: f64) -> f64 {
    let t = clamp01((11.0 - zoom) / 5.0);
    1.0 + (exaggeration - 1.0) * t
}

/// Ground resolution at the view centre, metres per pixel of a 512 px world at zoom 0. The globe
/// view matches Web Mercator's scale there.
pub fn meters_per_pixel(zoom: f64, latitude: f64) -> f64 {
    (EARTH_CIRCUMFERENCE_M * (latitude * DEG).cos()) / (512.0 * 2f64.powf(zoom))
}

/// Dead-reckons a flight forward from its last report using ground speed and track, so aircraft
/// move between feed refreshes: `[lon, lat]`. Unmoved on the ground or below 40 kt; at most
/// [`MAX_EXTRAPOLATION_S`] past the report, never back before it.
///
/// Nothing in the app calls `geo.ts` `extrapolate` (aircraft move with `motion.ts`), so it is
/// ported as a test helper only (doc 05 §16.9).
#[cfg(test)]
pub(crate) fn extrapolate(f: &crate::model::LiveFlight, now_ms: i64) -> [f64; 2] {
    use crate::model::f32_as_js;
    let (lat, lon) = (f32_as_js(f.lat), f32_as_js(f.lon));
    let speed = f64::from(f.speed);
    if f.on_ground || speed < 40.0 {
        return [lon, lat];
    }
    let dt = js_min(js_max((now_ms as f64 - f.timestamp_ms as f64) / 1000.0, 0.0), MAX_EXTRAPOLATION_S);
    let d = (speed * KNOTS_TO_MS * dt) / EARTH_RADIUS_M; // angular distance
    let lat1 = lat * DEG;
    let lon1 = lon * DEG;
    let brg = f64::from(f.track) * DEG;
    let lat2 = (lat1.sin() * d.cos() + lat1.cos() * d.sin() * brg.cos()).asin();
    let lon2 = lon1 + (brg.sin() * d.sin() * lat1.cos()).atan2(d.cos() - lat1.sin() * lat2.sin());
    [((lon2 / DEG + 540.0) % 360.0) - 180.0, lat2 / DEG]
}

/// Cosine of the great-circle angle between two points (degrees): 1 at the same point, 0 a
/// quarter of the way round, -1 at the antipode.
pub fn cos_angle(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> f64 {
    let a = lat1 * DEG;
    let b = lat2 * DEG;
    a.sin() * b.sin() + a.cos() * b.cos() * ((lon1 - lon2) * DEG).cos()
}

/// Angular radius (degrees, at most 100) around the view centre that covers a screen of the given
/// size, with some margin. At zoom z the globe is 512·2^z px around; tilting the camera pulls the
/// horizon much further out.
pub fn visible_radius(zoom: f64, pitch: f64, width_px: f64, height_px: f64) -> f64 {
    let half_diagonal = width_px.hypot(height_px) / 2.0;
    let degrees_per_pixel = 360.0 / (512.0 * 2f64.powf(zoom));
    let tilt = 1.0 + 4.0 * (pitch * DEG).sin();
    // slightly past the horizon so elevated aircraft on the limb stay visible
    js_min(100.0, 1.3 * half_diagonal * degrees_per_pixel * tilt)
}

/// The parts of the view state [`view_area`] reads (degrees, deck.gl zoom).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct View {
    pub longitude: f64,
    pub latitude: f64,
    pub zoom: f64,
    pub pitch: f64,
}

/// A latitude and longitude box in degrees. `west > east` when it crosses the antimeridian.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Area {
    pub south: f64,
    pub north: f64,
    pub west: f64,
    pub east: f64,
}

/// The area worth fetching for a view, or `None` when most of the globe is in sight and the whole
/// world should be fetched.
pub fn view_area(view: View, width_px: f64, height_px: f64) -> Option<Area> {
    let r = visible_radius(view.zoom, view.pitch, width_px, height_px);
    if r >= 50.0 {
        return None;
    }
    let south = js_max(-90.0, view.latitude - r);
    let north = js_min(90.0, view.latitude + r);
    let widest = js_max(south.abs(), north.abs());
    let d_lon = r / js_max(0.05, (widest * DEG).cos());
    if d_lon >= 180.0 {
        return Some(Area { south, north, west: -180.0, east: 180.0 });
    }
    // not `num::wrap_lon`: this form is the TypeScript's; it rounds differently in the last bits,
    // and below -540 it does not reach [-180, 180)
    let wrap = |lon: f64| ((lon + 540.0) % 360.0) - 180.0;
    Some(Area { south, north, west: wrap(view.longitude - d_lon), east: wrap(view.longitude + d_lon) })
}

/// Shortest-path longitude for animating from `from` to `to`: `to` moved by whole turns to within
/// 180° of `from`.
pub fn unwrap_longitude(from: f64, to: f64) -> f64 {
    let mut lon = to;
    while lon - from > 180.0 {
        lon -= 360.0;
    }
    while lon - from < -180.0 {
        lon += 360.0;
    }
    lon
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;
    use crate::model::LiveFlight;

    /// Bit-for-bit, so that -0 and +0 differ.
    fn same(a: f64, b: f64) -> bool {
        a.to_bits() == b.to_bits()
    }

    // Expected values below are the TypeScript's answers for the same inputs (bun 1.4).

    #[test]
    fn altitude_color_matches_the_typescript() {
        let cases: [(f64, Rgb); 17] = [
            (-500.0, [255, 98, 0]),
            (0.0, [255, 98, 0]),
            (1000.0, [255, 123, 0]),
            (2000.0, [255, 147, 0]),
            (4000.0, [255, 196, 0]),
            (6000.0, [208, 213, 20]),
            (8000.0, [160, 230, 40]),
            (11_500.0, [100, 225, 85]),
            (15_000.0, [40, 220, 130]),
            (16_250.0, [35, 218, 143]),
            (20_000.0, [20, 210, 180]),
            (25_000.0, [0, 200, 230]),
            (30_000.0, [35, 163, 243]),
            (35_000.0, [70, 125, 255]),
            (40_000.0, [138, 105, 255]),
            (45_000.0, [205, 85, 255]),
            (60_000.0, [205, 85, 255]),
        ];
        for (alt, rgb) in cases {
            assert_eq!(altitude_color(alt), rgb, "{alt} ft");
        }
    }

    #[test]
    fn altitude_color_rounds_halves_up() {
        // 16 250 ft is an eighth of the way from 15 000 to 25 000: 217.5 and 142.5 round up
        assert_eq!(altitude_color(16_250.0), [35, 218, 143]);
        // 6 000 ft is half way from 4 000 to 8 000: 207.5 → 208, 213 and 20 exact
        assert_eq!(altitude_color(6000.0), [208, 213, 20]);
    }

    #[test]
    fn altitude_color_is_held_at_both_ends() {
        assert_eq!(altitude_color(f64::NEG_INFINITY), ALTITUDE_STOPS[0].1);
        assert_eq!(altitude_color(-0.0), ALTITUDE_STOPS[0].1);
        assert_eq!(altitude_color(f64::INFINITY), ALTITUDE_STOPS[6].1);
        // every comparison with NaN is false: the last stop
        assert_eq!(altitude_color(f64::NAN), [205, 85, 255]);
        // each stop is its own colour
        for (alt, rgb) in ALTITUDE_STOPS {
            assert_eq!(altitude_color(alt), rgb);
        }
    }

    #[test]
    fn elevation_is_metres_times_the_exaggeration() {
        assert_eq!(elevation(35_000.0, 1.0), 10_668.0);
        assert_eq!(elevation(35_000.0, 3.0), 32_004.0);
        assert_eq!(elevation(1000.0, 2.5), 762.0);
        assert_eq!(elevation(0.0, 4.0), 0.0);
    }

    #[test]
    fn elevation_is_never_below_the_ground() {
        assert_eq!(elevation(-200.0, 5.0), 0.0);
        // Math.max(0, -0) is +0
        assert!(same(elevation(-0.0, 1.0), 0.0));
        assert!(same(elevation(-1e-9, 1.0), 0.0));
        // Math.max keeps NaN
        assert!(elevation(f64::NAN, 1.0).is_nan());
    }

    #[test]
    fn effective_exaggeration_fades_between_zoom_6_and_11() {
        assert_eq!(effective_exaggeration(3.0, 0.0), 3.0);
        assert_eq!(effective_exaggeration(3.0, 6.0), 3.0);
        assert_eq!(effective_exaggeration(3.0, 8.5), 2.0);
        assert_eq!(effective_exaggeration(3.0, 11.0), 1.0);
        assert_eq!(effective_exaggeration(3.0, 15.0), 1.0);
        assert_eq!(effective_exaggeration(1.0, 8.0), 1.0);
        assert_eq!(effective_exaggeration(5.0, 10.0), 1.8);
        // linear in between
        assert_close!(effective_exaggeration(3.0, 7.0), 2.6, 12);
    }

    #[test]
    fn meters_per_pixel_matches_the_typescript() {
        assert_eq!(meters_per_pixel(0.0, 0.0), 78_271.516_964_843_74);
        assert_eq!(meters_per_pixel(0.0, 0.0), EARTH_CIRCUMFERENCE_M / 512.0);
        assert_close!(meters_per_pixel(10.0, 60.0), 38.218_514_142_990_12, 12);
        assert_close!(meters_per_pixel(15.5, 59.33), 0.861_564_641_208_120_2, 12);
        assert_close!(meters_per_pixel(3.25, -33.9), 6_828.743_268_757_912, 9);
    }

    #[test]
    fn meters_per_pixel_halves_with_each_zoom_level() {
        for zoom in [0.0, 3.0, 7.5, 14.0] {
            let r = meters_per_pixel(zoom, 47.0) / meters_per_pixel(zoom + 1.0, 47.0);
            assert_close!(r, 2.0, 12);
        }
        // the same north and south
        assert_eq!(meters_per_pixel(9.0, 35.0), meters_per_pixel(9.0, -35.0));
    }

    #[test]
    fn visible_radius_matches_the_typescript() {
        assert_eq!(visible_radius(2.0, 0.0, 1440.0, 900.0), 100.0);
        assert_close!(visible_radius(5.0, 0.0, 1440.0, 900.0), 24.252_886_064_588_88, 12);
        assert_close!(visible_radius(8.0, 0.0, 1440.0, 900.0), 3.031_610_758_073_61, 12);
        assert_close!(visible_radius(8.0, 60.0, 1440.0, 900.0), 13.533_418_481_585_393, 12);
        assert_close!(visible_radius(12.5, 45.0, 390.0, 844.0), 0.280_839_219_735_461_55, 12);
        assert_close!(visible_radius(0.0, 0.0, 100.0, 100.0), 64.633_979_217_832_86, 12);
    }

    #[test]
    fn visible_radius_grows_with_tilt_and_is_capped() {
        let flat = visible_radius(8.0, 0.0, 1440.0, 900.0);
        // sin 30° = 1/2: three times as far
        assert_close!(visible_radius(8.0, 30.0, 1440.0, 900.0), 3.0 * flat, 12);
        // one zoom level in halves it
        assert_close!(visible_radius(9.0, 0.0, 1440.0, 900.0), flat / 2.0, 12);
        assert_eq!(visible_radius(0.0, 60.0, 4000.0, 3000.0), 100.0);
        assert!(visible_radius(f64::NAN, 0.0, 100.0, 100.0).is_nan());
    }

    fn view(longitude: f64, latitude: f64, zoom: f64, pitch: f64) -> View {
        View { longitude, latitude, zoom, pitch }
    }

    fn assert_area(actual: Option<Area>, expected: Area) {
        let a = actual.expect("an area");
        assert_close!(a.south, expected.south, 9);
        assert_close!(a.north, expected.north, 9);
        assert_close!(a.west, expected.west, 9);
        assert_close!(a.east, expected.east, 9);
    }

    #[test]
    fn view_area_is_none_with_most_of_the_globe_in_sight() {
        assert_eq!(view_area(view(18.07, 59.33, 2.0, 0.0), 1440.0, 900.0), None);
        assert_eq!(view_area(view(0.0, 0.0, 0.0, 0.0), 100.0, 100.0), None);
    }

    #[test]
    fn view_area_matches_the_typescript() {
        let area = |south, north, west, east| Area { south, north, west, east };
        assert_area(
            view_area(view(18.07, 59.33, 8.0, 0.0), 1440.0, 900.0),
            area(56.298_389_241_926_39, 62.361_610_758_073_61, 11.534_802_663_722_417, 24.605_197_336_277_57),
        );
        assert_area(
            view_area(view(18.07, 59.33, 8.0, 50.0), 1440.0, 900.0),
            area(47.008_994_942_238_39, 71.651_005_057_761_61, -21.068_627_771_481_488, 57.208_627_771_481_474),
        );
    }

    #[test]
    fn view_area_may_cross_the_antimeridian() {
        let a = view_area(view(179.5, -16.5, 8.0, 0.0), 1440.0, 900.0);
        assert_area(a, Area {
            south: -19.531_610_758_073_61,
            north: -13.468_389_241_926_39,
            west: 176.283_290_727_134_5,
            east: -177.283_290_727_134_5,
        });
        let a = view_area(view(-179.8, 10.0, 9.0, 30.0), 1440.0, 900.0);
        assert_area(a, Area {
            south: 5.452_583_862_889_585,
            north: 14.547_416_137_110_414,
            west: 175.501_965_353_522_1,
            east: -175.101_965_353_522_13,
        });
        let a = a.expect("an area");
        assert!(a.west > a.east);
    }

    #[test]
    fn view_area_near_a_pole_takes_every_longitude() {
        let a = view_area(view(10.0, 89.0, 5.0, 0.0), 1440.0, 900.0).expect("an area");
        assert_close!(a.south, 64.747_113_935_411_12, 9);
        assert_eq!((a.north, a.west, a.east), (90.0, -180.0, 180.0));
        let a = view_area(view(0.0, -80.0, 6.0, 20.0), 1440.0, 900.0).expect("an area");
        assert_close!(a.north, -51.283_605_831_961_964, 9);
        assert_eq!((a.south, a.west, a.east), (-90.0, -180.0, 180.0));
    }

    #[test]
    fn cos_angle_matches_the_typescript() {
        assert_eq!(cos_angle(0.0, 0.0, 0.0, 0.0), 1.0);
        assert_eq!(cos_angle(0.0, 0.0, 180.0, 0.0), -1.0);
        assert_close!(cos_angle(0.0, 0.0, 90.0, 0.0), 6.123_233_995_736_766e-17, 15);
        assert_close!(cos_angle(18.07, 59.33, 2.35, 48.86), 0.970_798_175_452_562_3, 14);
        assert_close!(cos_angle(-0.46, 51.47, 139.78, 35.55), 0.065_223_974_745_694_3, 14);
        assert_close!(cos_angle(10.0, 90.0, -170.0, 89.0), 0.999_847_695_156_391_3, 14);
    }

    #[test]
    fn cos_angle_is_symmetric_and_agrees_with_the_distance() {
        let (a, b) = ([18.07, 59.33], [2.35, 48.86]);
        assert_eq!(cos_angle(a[0], a[1], b[0], b[1]), cos_angle(b[0], b[1], a[0], a[1]));
        let angle = sphere::distance_m(a, b) / EARTH_RADIUS_M;
        assert_close!(cos_angle(a[0], a[1], b[0], b[1]), angle.cos(), 12);
    }

    #[test]
    fn unwrap_longitude_takes_the_short_way() {
        assert_eq!(unwrap_longitude(170.0, -170.0), 190.0);
        assert_eq!(unwrap_longitude(-170.0, 170.0), -190.0);
        assert_eq!(unwrap_longitude(10.0, 20.0), 20.0);
        assert_eq!(unwrap_longitude(0.0, 540.0), 180.0);
        assert_eq!(unwrap_longitude(0.0, -541.0), 179.0);
        assert_eq!(unwrap_longitude(179.0, 179.0), 179.0);
        // exactly half a turn stays
        assert_eq!(unwrap_longitude(0.0, -180.0), -180.0);
    }

    #[test]
    fn basemap_reads_and_writes_the_settings_names() {
        for (b, name) in [(Basemap::Dark, "dark"), (Basemap::Satellite, "satellite"), (Basemap::Realistic, "realistic")] {
            assert_eq!(b.as_str(), name);
            assert_eq!(serde_json::to_string(&b).unwrap(), format!("\"{name}\""));
            assert_eq!(serde_json::from_str::<Basemap>(&format!("\"{name}\"")).unwrap(), b);
        }
        assert_eq!(Basemap::default(), Basemap::Dark);
        assert!(serde_json::from_str::<Basemap>("\"Dark\"").is_err());
    }

    #[test]
    fn js_min_and_max_follow_javascript() {
        assert!(js_min(f64::NAN, 1.0).is_nan());
        assert!(js_min(1.0, f64::NAN).is_nan());
        assert!(js_max(f64::NAN, 1.0).is_nan());
        assert!(js_max(1.0, f64::NAN).is_nan());
        assert!(same(js_min(0.0, -0.0), -0.0));
        assert!(same(js_min(-0.0, 0.0), -0.0));
        assert!(same(js_max(0.0, -0.0), 0.0));
        assert!(same(js_max(-0.0, 0.0), 0.0));
        assert_eq!(js_min(2.0, 3.0), 2.0);
        assert_eq!(js_max(2.0, 3.0), 3.0);
    }

    fn flight(lat: f32, lon: f32, track: i32, speed: i32, on_ground: bool) -> LiveFlight {
        LiveFlight { lat, lon, track, speed, on_ground, timestamp_ms: 1_700_000_000_000, ..LiveFlight::default() }
    }

    #[test]
    fn extrapolate_dead_reckons_along_the_track() {
        let f = flight(59.65, 17.92, 45, 450, false);
        let p = extrapolate(&f, 1_700_000_030_000);
        assert_close!(p[0], 18.007_520_738_280_39, 9);
        assert_close!(p[1], 59.694_135_243_458_43, 9);
        // 450 kt for 30 s is 6 945 m
        assert_close!(sphere::distance_m([17.92, 59.65], p), 450.0 * KNOTS_TO_MS * 30.0, 3);
        assert_close!(sphere::bearing([17.92, 59.65], p), 45.0, 9);
        let f = flight(51.4775, -0.4614, 270, 160, false);
        let p = extrapolate(&f, 1_700_000_010_500);
        assert_close!(p[0], -0.473_879_538_451_683_4, 9);
        assert_close!(p[1], 51.477_499_337_757_614, 9);
    }

    #[test]
    fn extrapolate_stops_90_s_after_the_report() {
        let f = flight(59.65, 17.92, 45, 450, false);
        let p = extrapolate(&f, 1_700_000_300_000);
        assert_close!(p[0], 18.183_255_446_157_773, 9);
        assert_close!(p[1], 59.782_230_282_648_5, 9);
        assert_eq!(p, extrapolate(&f, 1_700_000_090_000));
    }

    #[test]
    fn extrapolate_never_goes_back_before_the_report() {
        // dt is held at 0; the round trip through radians moves the point by an ulp
        let f = flight(59.65, 17.92, 45, 450, false);
        let p = extrapolate(&f, 1_699_999_990_000);
        assert_close!(p[0], 17.919_999_999_999_96, 12);
        assert_close!(p[1], 59.650_000_000_000_006, 12);
    }

    #[test]
    fn extrapolate_leaves_slow_and_grounded_aircraft_where_they_are() {
        // the reported position as JavaScript reads it from the JSON, not the f32 widened
        assert_eq!(extrapolate(&flight(59.65, 17.92, 45, 39, false), 1_700_000_030_000), [17.92, 59.65]);
        assert_eq!(extrapolate(&flight(59.65, 17.92, 45, 450, true), 1_700_000_030_000), [17.92, 59.65]);
    }

    #[test]
    fn extrapolate_wraps_across_the_antimeridian() {
        let p = extrapolate(&flight(59.65, 179.9, 90, 450, false), 1_700_000_060_000);
        assert_close!(p[0], -179.852_780_900_230_07, 9);
        assert_close!(p[1], 59.649_767_440_150_015, 9);
    }
}
