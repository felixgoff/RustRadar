//! Port of `clouds.ts` (S1.28; doc 04 §9): the clouds as they are now, the world over, for the
//! realistic globe.
//!
//! EUMETSAT's world cloud map (EUMETView), its geostationary satellites' infrared at 10.8 µm
//! stitched round the Earth every three hours: cold cloud tops bright, the warm ground dark (the
//! clouds' shader turns the one into cloud and leaves the other clear). Asked for again every hour.
//! Fetching and decoding the picture is the backend's and cropping it the map's; here are where it
//! is laid ([`CLOUD_BOUNDS`], [`CLOUD_HEIGHT_M`]), when to ask for it ([`picture_due`]), how much of
//! it shows ([`cloud_opacity`]) and, on the flat map, its strips ([`cloud_strips`]).

use crate::geom::{js_max, js_min};

pub const CLOUDS_ATTRIBUTION: &str = "Clouds: © EUMETSAT";
/// The picture's edges, `[west, south, east, north]`: as far north and south as the flat map goes.
pub const CLOUD_BOUNDS: [f64; 4] = [-180.0, -85.0, 180.0, 85.0];
/// The picture (WMS 1.3.0 in EPSG:4326, so its bounding box is latitude first), 4096 × 1934.
pub const CLOUD_URL: &str = "https://view.eumetsat.int/geoserver/wms?service=WMS&version=1.3.0&request=GetMap&layers=mumi:worldcloudmap_ir108&styles=&crs=EPSG:4326&bbox=-85,-180,85,180&width=4096&height=1934&format=image/jpeg";
/// A newer picture is asked for this long after the last.
pub const REFRESH_MS: i64 = 60 * 60_000;
/// After a failure (no picture yet), tried again this soon.
pub const RETRY_MS: i64 = 5 * 60_000;

/// The clouds' height, metres: they are drawn there, faded out as the camera comes near them; the
/// weather is the sun's above them.
pub const CLOUD_HEIGHT_M: f64 = 10_000.0;
/// The weather's own look under the clouds, the sun's above them: from here up to them, metres
/// (`Globe.svelte`'s `WEATHER_UP_TO_M`).
pub const WEATHER_UP_TO_M: f64 = CLOUD_HEIGHT_M * 0.3;
/// The clouds fade away as the view zooms in from here, gone by there (`Globe.svelte`).
pub const CLOUDS_FULL_ZOOM: f64 = 5.5;
pub const CLOUDS_GONE_ZOOM: f64 = 6.5;

/// Whether to ask for the picture now (`cloudPicture`): not while a request is out (`loading`),
/// else [`REFRESH_MS`] after the last ask once there is a picture, [`RETRY_MS`] while there is
/// none. `asked_at_ms` is none before the first ask (the TypeScript's `-Infinity`).
pub fn picture_due(now_ms: i64, asked_at_ms: Option<i64>, have_picture: bool, loading: bool) -> bool {
    let wait = if have_picture { REFRESH_MS } else { RETRY_MS };
    !loading && asked_at_ms.is_none_or(|asked| now_ms.saturating_sub(asked) > wait)
}

/// How much of the clouds shows at `zoom`: all out to [`CLOUDS_FULL_ZOOM`], none from
/// [`CLOUDS_GONE_ZOOM`] (`1 − smoothstep`).
pub fn cloud_opacity(zoom: f64) -> f64 {
    let t = rustradar_core::num::clamp01((zoom - CLOUDS_FULL_ZOOM) / (CLOUDS_GONE_ZOOM - CLOUDS_FULL_ZOOM));
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// A quad's corners at the clouds' height, as deck.gl's `BitmapLayer` takes them: lower left,
/// upper left, upper right, lower right, each `[longitude, latitude, metres]`.
pub type Corners = [[f64; 3]; 4];

/// On the globe the picture is one quad over [`CLOUD_BOUNDS`] (the globe's mesh is fine enough
/// for its degrees).
pub fn globe_corners() -> Corners {
    let [west, south, east, north] = CLOUD_BOUNDS;
    let h = CLOUD_HEIGHT_M;
    [[west, south, h], [west, north, h], [east, north, h], [east, south, h]]
}

/// A strip of the picture a degree of latitude high: its rows `y0..y1` (the whole width), laid
/// at `corners`, which are where those rows are (a little past the degree).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strip {
    /// The degree it is for: from `degree` to `degree + 1` north (clipped to the picture's edges).
    pub degree: i32,
    pub y0: u32,
    pub y1: u32,
    pub corners: Corners,
}

/// The strip for the degree from `k` north, of a picture `height` pixels high; none past the
/// picture's edges. The same for the same degree each time, so it can be cut out once.
pub fn cloud_strip(height: u32, k: i32) -> Option<Strip> {
    let [_, south, _, north] = CLOUD_BOUNDS;
    let rows = f64::from(height) / (north - south);
    let (a, b) = (js_max(south, f64::from(k)), js_min(north, f64::from(k) + 1.0));
    if b <= a {
        return None;
    }
    let (y0, y1) = (((north - b) * rows).floor(), ((north - a) * rows).ceil());
    // (its edges where its rows are: a little past the degree)
    let (top, bottom) = (north - y0 / rows, north - y1 / rows);
    let h = CLOUD_HEIGHT_M;
    Some(Strip {
        degree: k,
        y0: y0 as u32,
        y1: y1 as u32,
        corners: [[-180.0, bottom, h], [-180.0, top, h], [180.0, top, h], [180.0, bottom, h]],
    })
}

/// How many degrees either side of the view's the flat map's strips reach (`cloudStrips`' default).
pub const STRIP_REACH: i32 = 3;

/// The picture (`height` pixels high) in strips a degree of latitude high, round `lat`, at the
/// clouds' height: on the flat map a picture is laid on as one quad and web mercator would bend it
/// out of place, a strip at a time only a little. None for a latitude that isn't a number (where
/// the TypeScript would loop for ever on an infinite one).
pub fn cloud_strips(height: u32, lat: f64, reach: i32) -> Vec<Strip> {
    if !lat.is_finite() {
        return Vec::new();
    }
    let at = lat.floor() as i32;
    (at.saturating_sub(reach)..=at.saturating_add(reach)).filter_map(|k| cloud_strip(height, k)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PICTURE_HEIGHT: u32 = 1934;

    #[test]
    fn the_url_asks_for_the_bounds_latitude_first() {
        let [west, south, east, north] = CLOUD_BOUNDS;
        assert!(CLOUD_URL.contains(&format!("&bbox={south},{west},{north},{east}&")));
        assert!(CLOUD_URL.ends_with("&width=4096&height=1934&format=image/jpeg"));
    }

    #[test]
    fn asked_for_hourly_and_again_soon_after_a_failure() {
        assert!(picture_due(0, None, false, false));
        assert!(!picture_due(0, None, false, true));
        assert!(!picture_due(1000 + RETRY_MS, Some(1000), false, false));
        assert!(picture_due(1001 + RETRY_MS, Some(1000), false, false));
        assert!(!picture_due(1001 + RETRY_MS, Some(1000), true, false));
        assert!(picture_due(1001 + REFRESH_MS, Some(1000), true, false));
        assert!(!picture_due(1001 + REFRESH_MS, Some(1000), true, true));
    }

    #[test]
    fn the_clouds_fade_as_the_view_zooms_in() {
        assert_eq!(cloud_opacity(3.0), 1.0);
        assert_eq!(cloud_opacity(5.5), 1.0);
        assert_eq!(cloud_opacity(6.0), 0.5);
        assert_eq!(cloud_opacity(6.5), 0.0);
        assert_eq!(cloud_opacity(12.0), 0.0);
        assert_eq!(globe_corners()[2], [180.0, 85.0, CLOUD_HEIGHT_M]);
    }

    /// The TypeScript's strips (bun, a picture 1934 rows high).
    #[test]
    fn strips_round_the_view_as_the_typescript_cut_them() {
        let strips = cloud_strips(PICTURE_HEIGHT, 55.6, STRIP_REACH);
        let expected: [(f64, f64, u32); 7] = [
            (51.9493278179938, 53.00413650465357, 12),
            (52.916235780765255, 54.05894519131334, 13),
            (53.97104446742503, 55.0258531540848, 12),
            (54.93795243019649, 56.08066184074457, 13),
            (55.99276111685626, 57.047569803516026, 12),
            (56.959669079627716, 58.014477766287484, 12),
            (57.926577042399174, 59.06928645294726, 13),
        ];
        assert_eq!(strips.len(), expected.len());
        for (i, (s, (bottom, top, rows))) in strips.iter().zip(expected).enumerate() {
            assert_eq!(s.degree, 52 + i as i32);
            assert_eq!(s.y1 - s.y0, rows);
            let h = CLOUD_HEIGHT_M;
            assert_eq!(s.corners, [[-180.0, bottom, h], [-180.0, top, h], [180.0, top, h], [180.0, bottom, h]]);
        }
        // at the picture's edges, only what it has
        let south = cloud_strips(PICTURE_HEIGHT, -84.2, 3);
        assert_eq!(south.len(), 4);
        assert_eq!(south[0].corners[0][1], -85.0);
        assert_eq!(south[0].corners[1][1], -83.94519131334022);
        assert_eq!(south[0].y1, PICTURE_HEIGHT);
        let north = cloud_strips(PICTURE_HEIGHT, 84.9, 2);
        assert_eq!(north.len(), 3);
        assert_eq!((north[2].y0, north[2].corners[1][1], north[2].corners[0][1]), (0, 85.0, 83.94519131334023));
        // rows that fall between pixels: the strip takes in both
        let odd = cloud_strips(1000, -0.5, 1);
        assert_eq!(odd.iter().map(|s| s.y1 - s.y0).collect::<Vec<_>>(), [7, 6, 7]);
        assert_eq!(odd[0].corners[0][1], -2.039999999999992);
        assert_eq!(odd[2].corners[1][1], 1.0200000000000102);
        // one row a degree: the degree exactly
        assert_eq!(cloud_strips(170, 0.0, 0)[0].corners[1][1], 1.0);
        assert!(cloud_strips(PICTURE_HEIGHT, f64::NAN, 3).is_empty());
        assert!(cloud_strips(PICTURE_HEIGHT, f64::INFINITY, 3).is_empty());
    }
}
