//! Port of `places.ts` (doc 05 §11.5): where to fly for a place found by search, and how the
//! search list describes it.
//!
//! `Math.min`/`Math.max` are the private `js_min`/`js_max`, which keep NaN as JavaScript does.
//! `KIND_ZOOM[kind] ?? KIND_ZOOM.poi` needs no fallback here: [`PlaceKind`] is closed, so a kind
//! the TypeScript did not know never deserialises.

use crate::model::{Place, PlaceKind};

/// How close to fly to each kind of place when it has no extent to fit (`KIND_ZOOM`).
pub fn kind_zoom(kind: PlaceKind) -> f64 {
    match kind {
        PlaceKind::Address => 17.5,
        PlaceKind::Poi => 16.5,
        PlaceKind::Street => 16.0,
        PlaceKind::District => 14.0,
        PlaceKind::Postcode => 13.0,
        PlaceKind::Airport => 13.0,
        PlaceKind::Town => 12.0,
        PlaceKind::City => 11.0,
        PlaceKind::Region => 7.0,
        PlaceKind::Country => 5.0,
    }
}

/// Never closer than the globe's street level allows, nor further than the whole planet.
const MIN_ZOOM: f64 = 2.0;
const MAX_ZOOM: f64 = 17.5;
/// An extent may move the zoom this far from its kind's (an overseas territory widens a country).
const BELOW_KIND: f64 = 2.0;
const ABOVE_KIND: f64 = 1.5;
/// The share of the view a fitted extent fills.
const FILL: f64 = 0.7;
/// deck.gl draws the world 512 px wide at zoom 0.
const WORLD_PX: f64 = 512.0;

/// `Math.min` of two: NaN if either is.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}

/// `Math.max` of two: NaN if either is.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}

fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    js_min(hi, js_max(lo, x))
}

/// The zoom that shows `bbox` (`[west, south, east, north]`, degrees) within a `width`×`height`
/// view. deck.gl's globe keeps Web Mercator's local scale (a degree of longitude is
/// 512·2^zoom/360 px at any latitude, a degree of latitude 1/cos(lat) times that), so this holds
/// on the globe and in the flat view above zoom 12 alike.
pub fn fit_zoom(bbox: [f64; 4], width: f64, height: f64) -> f64 {
    let [west, south, east, north] = bbox;
    // across the antimeridian
    let lon_span = js_max(if east >= west { east - west } else { east + 360.0 - west }, 1e-6);
    let mid_lat = js_min(((south + north) / 2.0).abs(), 85.0) * (std::f64::consts::PI / 180.0);
    let lat_span = js_max(north - south, 1e-6) / mid_lat.cos();
    let scale = js_min((width * FILL * 360.0) / (WORLD_PX * lon_span), (height * FILL * 360.0) / (WORLD_PX * lat_span));
    scale.log2()
}

/// Where to fly for a place ([`place_view`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaceView {
    pub lon: f64,
    pub lat: f64,
    pub zoom: f64,
}

/// Where to fly for a place: its point, at a zoom fitted to its extent or set by its kind.
pub fn place_view(place: &Place, width: f64, height: f64) -> PlaceView {
    let kind_zoom = kind_zoom(place.kind);
    let zoom = match place.bbox {
        Some(bbox) => clamp(fit_zoom(bbox, width, height), kind_zoom - BELOW_KIND, kind_zoom + ABOVE_KIND),
        None => kind_zoom,
    };
    PlaceView { lon: place.lon, lat: place.lat, zoom: clamp(zoom, MIN_ZOOM, MAX_ZOOM) }
}

/// What a kind of place is called in the search list (`KIND_WORD`).
pub fn kind_word(kind: PlaceKind) -> &'static str {
    match kind {
        PlaceKind::Address => "Address",
        PlaceKind::Street => "Street",
        PlaceKind::Poi => "Place",
        PlaceKind::Airport => "Airport",
        PlaceKind::District => "District",
        PlaceKind::City => "City",
        PlaceKind::Town => "Town",
        PlaceKind::Postcode => "Postcode",
        PlaceKind::Region => "Region",
        PlaceKind::Country => "Country",
    }
}

/// The muted second line: what it is, then where (the label without the name).
pub fn place_detail(place: &Place) -> String {
    // `category ?? KIND_WORD[kind]`: an empty category stays empty
    let what = place.category.as_deref().unwrap_or(kind_word(place.kind));
    let prefix = format!("{}, ", place.name);
    let place_where = match place.label.strip_prefix(&prefix) {
        Some(rest) => rest,
        None if place.label == place.name => "",
        None => &place.label,
    };
    if place_where.is_empty() { what.to_owned() } else { format!("{what} · {place_where}") }
}

/// The interface language (a BCP 47 tag such as `de-AT`), if Photon has names in it; otherwise
/// `None`, for local names. The TypeScript defaulted to `navigator.language`: the caller passes
/// the system's language.
pub fn place_language(language: &str) -> Option<&'static str> {
    // `language.slice(0, 2).toLowerCase()`
    let primary = language.chars().take(2).collect::<String>().to_lowercase();
    ["en", "de", "fr", "it"].into_iter().find(|l| *l == primary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(kind: PlaceKind) -> Place {
        Place { name: "X".to_owned(), label: "X".to_owned(), kind, category: None, lat: 0.0, lon: 0.0, bbox: None }
    }

    /// `new GlobeViewport({ width, height, longitude, latitude, zoom }).project([lon, lat, 0])`, x and y,
    /// for pitch and bearing 0: deck.gl 9.4's `globe-viewport.js` and `viewport.js` (doc 01 §2.2)
    /// written out. Checked against the 128 ground points of the north-up globe views in
    /// `crates/scene/tests/data/deck/views.json` (all within 1e-6 px).
    fn globe_project(view: (f64, f64, f64), width: f64, height: f64, lon: f64, lat: f64) -> (f64, f64) {
        const GLOBE_RADIUS: f64 = 256.0;
        const MAX_LATITUDE: f64 = 85.051129;
        const ALTITUDE: f64 = 1.5;
        let (longitude, latitude, zoom) = view;
        let rad = std::f64::consts::PI / 180.0;
        // projectPosition at 0 m
        let world = |lng: f64, lat: f64| {
            let (l, p) = (lng * rad, lat * rad);
            [l.sin() * p.cos() * GLOBE_RADIUS, -l.cos() * p.cos() * GLOBE_RADIUS, p.sin() * GLOBE_RADIUS]
        };
        let scale_latitude = latitude.clamp(-MAX_LATITUDE, MAX_LATITUDE);
        let scale = 2f64.powf(zoom - (std::f64::consts::PI * (scale_latitude * rad).cos()).log2());
        // viewMatrix = lookAt · Rx(lat) · Rz(-lng) · S(scale / height) · T(-center), right to left
        let (center, point) = (world(longitude, latitude), world(lon, lat));
        let q: Vec<f64> = (0..3).map(|i| (point[i] - center[i]) * (scale / height)).collect();
        let (sz, cz) = (-longitude * rad).sin_cos();
        let (x, y, z) = (q[0] * cz - q[1] * sz, q[0] * sz + q[1] * cz, q[2]);
        let (sx, cx) = (latitude * rad).sin_cos();
        let (y, z) = (y * cx - z * sx, y * sx + z * cx);
        // lookAt(eye [0, -1.5, 0], up [0, 0, 1]): camera x = x, camera y = z, depth = y + 1.5
        let depth = y + ALTITUDE;
        // perspective with tan(fovy / 2) = 0.5 / altitude, then the pixel matrix (y down)
        let focal = ALTITUDE / 0.5;
        let (ndc_x, ndc_y) = (focal * (height / width) * x / depth, focal * z / depth);
        ((ndc_x + 1.0) * width / 2.0, (1.0 - ndc_y) * height / 2.0)
    }

    #[test]
    fn globe_project_reproduces_deck_gl() {
        // records of `crates/scene/tests/data/deck/views.json`: (view, width, height, lon, lat, x, y)
        let records = [
            ((0.0, 0.0, 0.5), 1440.0, 900.0, -53.43690413140591, 26.65668928552827, 640.4522407440313, 400.28265046501963),
            ((18.0, 60.0, 1.5), 390.0, 844.0, -127.15625260370403, 46.43425509430156, 48.45704836816906, 104.86602262239657),
            ((170.0, -45.0, 2.5), 1440.0, 900.0, 133.814911522432, 13.309653719948754, 430.40078786938426, 88.00098483672997),
            ((-60.0, 0.0, 7.7), 390.0, 844.0, -60.33125036102683, 1.43341239733763, 97.50000000000134, -2.414666060164699e-12),
        ];
        for (view, width, height, lon, lat, x, y) in records {
            let (px, py) = globe_project(view, width, height, lon, lat);
            assert!((px - x).abs() < 1e-6 && (py - y).abs() < 1e-6, "{view:?} [{lon}, {lat}]: ({px}, {py})");
        }
    }

    // placeView

    #[test]
    fn uses_the_kinds_zoom_without_an_extent() {
        assert_eq!(place_view(&place(PlaceKind::Address), 1400.0, 900.0).zoom, 17.5);
        assert_eq!(place_view(&place(PlaceKind::Street), 1400.0, 900.0).zoom, 16.0);
        assert_eq!(place_view(&place(PlaceKind::Town), 1400.0, 900.0).zoom, 12.0);
        assert_eq!(place_view(&place(PlaceKind::Country), 1400.0, 900.0).zoom, 5.0);
    }

    #[test]
    fn fits_an_extent_within_reach_of_the_kinds_zoom() {
        // Paris (the city), from Photon
        let paris = Place {
            lat: 48.8535,
            lon: 2.3484,
            bbox: Some([2.224122, 48.8155755, 2.4697602, 48.902156]),
            ..place(PlaceKind::City)
        };
        let PlaceView { zoom, lat, lon } = place_view(&paris, 1400.0, 900.0);
        assert!(zoom > 11.0);
        assert!(zoom < 12.5);
        // centred on the place's point, not its extent's middle
        assert_eq!([lat, lon], [48.8535, 2.3484]);

        // a country with far-flung territory doesn't zoom out to the whole planet
        let france = Place { lat: 46.6, lon: 2.2, bbox: Some([-178.0, -50.0, 172.0, 51.1]), ..place(PlaceKind::Country) };
        assert_eq!(place_view(&france, 1400.0, 900.0).zoom, kind_zoom(PlaceKind::Country) - 2.0);
        // a few metres of canal don't fly closer than street level
        let canal = Place { bbox: Some([4.8985, 52.3767, 4.8989, 52.377]), ..place(PlaceKind::Poi) };
        assert_eq!(place_view(&canal, 1400.0, 900.0).zoom, 17.5);
    }

    #[test]
    fn a_fitted_extent_is_on_screen_on_the_globe() {
        let bbox = [5.8, 47.3, 15.0, 55.1]; // Germany
        let zoom = fit_zoom(bbox, 1200.0, 800.0);
        for corner in [[bbox[0], bbox[1]], [bbox[2], bbox[3]], [bbox[0], bbox[3]], [bbox[2], bbox[1]]] {
            let (x, y) = globe_project((10.4, 51.2, zoom), 1200.0, 800.0, corner[0], corner[1]);
            assert!(x > 0.0, "{corner:?}: x {x}");
            assert!(x < 1200.0, "{corner:?}: x {x}");
            assert!(y > 0.0, "{corner:?}: y {y}");
            assert!(y < 800.0, "{corner:?}: y {y}");
        }
        // and fills a good part of it: one zoom level closer would not fit
        let (_, top) = globe_project((10.4, 51.2, zoom + 1.0), 1200.0, 800.0, 10.4, bbox[3]);
        assert!(top < 0.0, "top {top}");
    }

    #[test]
    fn handles_extents_across_the_antimeridian() {
        let fiji = fit_zoom([177.0, -19.2, -179.8, -16.1], 1200.0, 800.0);
        assert!(fiji > 5.0);
        assert!(fiji < 8.0);
    }

    // placeDetail

    #[test]
    fn says_what_it_is_then_where() {
        let damrak = Place {
            name: "Damrak 1".to_owned(),
            label: "Damrak 1, 1012 LG Amsterdam, Netherlands".to_owned(),
            ..place(PlaceKind::Address)
        };
        assert_eq!(place_detail(&damrak), "Address · 1012 LG Amsterdam, Netherlands");
        let eiffel = Place {
            name: "Eiffel Tower".to_owned(),
            label: "Eiffel Tower, 5 Avenue Anatole France, 75007 Paris, France".to_owned(),
            category: Some("Tower".to_owned()),
            ..place(PlaceKind::Poi)
        };
        assert_eq!(place_detail(&eiffel), "Tower · 5 Avenue Anatole France, 75007 Paris, France");
        let netherlands =
            Place { name: "Netherlands".to_owned(), label: "Netherlands".to_owned(), ..place(PlaceKind::Country) };
        assert_eq!(place_detail(&netherlands), "Country");
    }

    // placeLanguage

    #[test]
    fn asks_for_names_photon_has_else_local_ones() {
        assert_eq!(place_language("de-AT"), Some("de"));
        assert_eq!(place_language("en"), Some("en"));
        assert_eq!(place_language("nl-NL"), None);
        assert_eq!(place_language(""), None);
    }

    // Beyond the TypeScript tests.

    #[test]
    fn every_kind_has_its_zoom_and_word_and_odd_labels_read_as_in_the_typescript() {
        let kinds = [
            (PlaceKind::Address, 17.5, "Address"),
            (PlaceKind::Poi, 16.5, "Place"),
            (PlaceKind::Street, 16.0, "Street"),
            (PlaceKind::District, 14.0, "District"),
            (PlaceKind::Postcode, 13.0, "Postcode"),
            (PlaceKind::Airport, 13.0, "Airport"),
            (PlaceKind::Town, 12.0, "Town"),
            (PlaceKind::City, 11.0, "City"),
            (PlaceKind::Region, 7.0, "Region"),
            (PlaceKind::Country, 5.0, "Country"),
        ];
        for (kind, zoom, word) in kinds {
            assert_eq!((kind_zoom(kind), kind_word(kind)), (zoom, word));
        }
        // a label that doesn't start with the name is shown whole; an empty category stays empty
        let other = Place { name: "Gare".to_owned(), label: "Paris, France".to_owned(), ..place(PlaceKind::Poi) };
        assert_eq!(place_detail(&other), "Place · Paris, France");
        let blank = Place { category: Some(String::new()), ..other };
        assert_eq!(place_detail(&blank), " · Paris, France");
        assert_eq!(place_language("FR-ca"), Some("fr"));
        assert_eq!(place_language("i"), None);
        // a degenerate extent is still a number, clamped to the kind's reach
        let point = Place { bbox: Some([1.0, 1.0, 1.0, 1.0]), ..place(PlaceKind::Town) };
        assert_eq!(place_view(&point, 1400.0, 900.0).zoom, 13.5);
    }
}
