//! Port of `ships.ts` without the canvas atlas (S1.13, doc 05 §12.1): type colours and names,
//! the icon keys and sizes, and the true-scale hull outline.
//!
//! `shipAtlas` (Canvas 2D) is drawn by `scene::atlas::ships` (doc 02 §4.2): a row per colour key
//! in [`SHIP_COLOR_KEYS`] order, a `hull-` and a `dot-` cell each, as [`ship_icon`] names them.
//!
//! Kinds are the model's `u8` (`Ship::kind`); the TypeScript's `KINDS[kind] ?? OTHER` is a
//! lookup in [`KINDS`] that falls back to "Other" in the unknown grey.

use crate::geo::Rgb;
use crate::geo::sphere::LonLat;
use crate::model::Ship;
use crate::trains::js_max;

/// VesselFinder's type colours, in calm tones that read on the dark map and stay clear of the
/// amber reserved for aircraft and selection: `(kind, label, colour)`, in the TypeScript
/// object's key order (ascending integers).
const KINDS: [(u8, &str, Rgb); 8] = [
    (0, "Unknown type", [140, 152, 168]),
    (2, "Tug or special craft", [72, 198, 194]),
    (3, "Passenger", [98, 152, 240]),
    (4, "Cargo", [116, 188, 112]),
    (5, "Fishing", [236, 128, 156]),
    (6, "Tanker", [218, 92, 92]),
    (7, "Search and rescue or law enforcement", [228, 234, 242]),
    (8, "Pleasure craft or sailing", [192, 136, 236]),
];

/// Any type not in [`KINDS`]: "Other", in the unknown's grey.
const OTHER: (&str, Rgb) = ("Other", KINDS[0].2);

/// The colour keys the icon atlas has a cell for, in its row order.
pub const SHIP_COLOR_KEYS: [u8; 8] = [0, 2, 3, 4, 5, 6, 7, 8];

/// `KINDS[kind] ?? OTHER`.
fn kind_entry(kind: u8) -> (&'static str, Rgb) {
    KINDS.iter().find(|(k, _, _)| *k == kind).map_or(OTHER, |(_, label, color)| (*label, *color))
}

pub fn ship_kind_label(kind: u8) -> &'static str {
    kind_entry(kind).0
}

pub fn ship_color(kind: u8) -> Rgb {
    kind_entry(kind).1
}

/// The atlas's colour key for a type, which is also the type group a ship falls in as the map
/// colours it: unlisted types share the unknown's grey (key 0).
pub fn ship_kind_key(kind: u8) -> u8 {
    if KINDS.iter().any(|(k, _, _)| *k == kind) { kind } else { 0 }
}

/// A pointed hull for a ship under way (heading known), a dot for one without: `hull-3`, `dot-0`.
pub fn ship_icon(ship: &Ship) -> String {
    format!("{}-{}", if ship.heading.is_none() { "dot" } else { "hull" }, ship_kind_key(ship.kind))
}

/// Icon size in pixels: modestly larger for larger ships, smaller for a heading-less dot.
pub fn ship_icon_size(ship: &Ship, zoom: f64) -> f64 {
    // `[15, 18, 22][ship.size] ?? 18`
    let base = [15.0, 18.0, 22.0].get(usize::from(ship.size)).copied().unwrap_or(18.0);
    base * ship_icon_scale(zoom) * if ship.heading.is_none() { 0.62 } else { 1.0 }
}

/// Smaller zoomed out, where a couple of thousand ships share the screen; in steps, so sizes
/// update rarely.
pub fn ship_icon_scale(zoom: f64) -> f64 {
    if zoom < 3.0 {
        0.7
    } else if zoom < 5.0 {
        0.85
    } else if zoom < 7.0 {
        0.92
    } else {
        1.0
    }
}

/// How far into a hull's length the bow's taper starts, from the bow.
const BOW_TAPER: f64 = 0.2;
/// `Math.PI / 180`.
const DEG: f64 = std::f64::consts::PI / 180.0;
const METRES_PER_DEGREE: f64 = 111_320.0;

/// The hull's outline at true scale, `[lon, lat]` going round: the AIS dimensions `dims` are
/// metres from the reference point to bow, stern, port and starboard (`Ship::dims` as `f64`),
/// turned to `heading` (degrees clockwise from north). A pointed bow, square stern.
pub fn hull_polygon(lon: f64, lat: f64, dims: [f64; 4], heading: f64) -> [LonLat; 5] {
    let [bow, stern, port, stbd] = dims;
    let length = bow + stern;
    let h = heading * DEG;
    // forward and to starboard, as east and north
    let (fe, fn_) = (h.sin(), h.cos());
    let (re, rn) = (h.cos(), -h.sin());
    let kx = 1.0 / (METRES_PER_DEGREE * js_max(0.01, (lat * DEG).cos()));
    let ky = 1.0 / METRES_PER_DEGREE;
    let point = |right: f64, ahead: f64| -> LonLat {
        [lon + (right * re + ahead * fe) * kx, lat + (right * rn + ahead * fn_) * ky]
    };
    let shoulder = bow - length * BOW_TAPER;
    [
        point((stbd - port) / 2.0, bow),
        point(stbd, shoulder),
        point(stbd, -stern),
        point(-port, -stern),
        point(-port, shoulder),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;

    /// A ship with the given type, size and heading; the rest as `Ship::default()`.
    fn ship(kind: u8, size: u8, heading: Option<f32>) -> Ship {
        Ship { kind, size, heading, ..Ship::default() }
    }

    // --- hullPolygon

    #[test]
    fn points_the_bow_along_the_heading_at_true_length_and_beam() {
        // 200 m long, 30 m wide, reference point 150 m from the bow, centred
        let north = hull_polygon(0.0, 0.0, [150.0, 50.0, 15.0, 15.0], 0.0);
        let [bow, _, stern_stbd, stern_port, _] = north;
        assert_close!(bow[0], 0, 9);
        assert_close!(bow[1] * METRES_PER_DEGREE, 150, 6);
        assert_close!(stern_stbd[0] * METRES_PER_DEGREE, 15, 6); // starboard is east heading north
        assert_close!(stern_port[0] * METRES_PER_DEGREE, -15, 6);
        assert_close!(stern_stbd[1] * METRES_PER_DEGREE, -50, 6);

        let east = hull_polygon(0.0, 0.0, [150.0, 50.0, 15.0, 15.0], 90.0);
        assert_close!(east[0][0] * METRES_PER_DEGREE, 150, 6);
        assert_close!(east[0][1], 0, 9);
        assert_close!(east[2][1] * METRES_PER_DEGREE, -15, 6); // starboard is south heading east
    }

    #[test]
    fn longitude_stretches_with_latitude() {
        let [bow, ..] = hull_polygon(10.0, 60.0, [100.0, 0.0, 5.0, 5.0], 90.0);
        assert_close!((bow[0] - 10.0) * METRES_PER_DEGREE * (60.0 * std::f64::consts::PI / 180.0).cos(), 100, 6);
    }

    #[test]
    fn hull_shoulders_and_an_off_centre_reference_point() {
        // new: the shoulders sit a fifth of the length behind the bow; the bow tip is half way
        // between the sides when the reference point is off the centre line
        let hull = hull_polygon(0.0, 0.0, [80.0, 20.0, 4.0, 10.0], 0.0);
        let m = |p: LonLat| [p[0] * METRES_PER_DEGREE, p[1] * METRES_PER_DEGREE];
        let expected = [[3.0, 80.0], [10.0, 60.0], [10.0, -20.0], [-4.0, -20.0], [-4.0, 60.0]];
        for (p, e) in hull.into_iter().map(m).zip(expected) {
            assert_close!(p[0], e[0], 9);
            assert_close!(p[1], e[1], 9);
        }
        // at the pole the longitude scale stops at cos = 0.01
        let polar = hull_polygon(0.0, 90.0, [0.0, 0.0, 0.0, 1.0], 0.0);
        assert_close!(polar[1][0] * METRES_PER_DEGREE * 0.01, 1, 9);
    }

    // --- type colours and icons

    #[test]
    fn known_and_unknown_types() {
        assert_eq!(ship_kind_label(4), "Cargo");
        assert_eq!(ship_kind_label(6), "Tanker");
        assert_eq!(ship_kind_label(13), "Other");
        assert_eq!(ship_color(13), ship_color(0));
        assert_eq!(ship_icon(&ship(3, 1, Some(90.0))), "hull-3");
        assert_eq!(ship_icon(&ship(11, 1, None)), "dot-0");
    }

    #[test]
    fn bigger_ships_get_bigger_icons_moored_ones_a_small_dot() {
        let at = |size: u8, heading: Option<f32>| ship_icon_size(&ship(0, size, heading), 10.0);
        assert!(at(0, Some(0.0)) < at(1, Some(0.0)));
        assert!(at(1, Some(0.0)) < at(2, Some(0.0)));
        assert!(at(1, None) < at(1, Some(0.0)));
    }

    #[test]
    fn the_palette_keys_and_sizes_as_in_the_typescript() {
        // new: doc 05 §12.1 and doc 02 §4.1, §4.3
        assert_eq!(SHIP_COLOR_KEYS, KINDS.map(|(k, _, _)| k));
        assert_eq!(ship_color(7), [228, 234, 242]);
        assert_eq!(ship_kind_label(1), "Other");
        assert_eq!((ship_kind_key(8), ship_kind_key(9), ship_kind_key(1)), (8, 0, 0));
        let icons: Vec<String> = SHIP_COLOR_KEYS.iter().map(|k| ship_icon(&ship(*k, 1, Some(0.0)))).collect();
        assert_eq!(icons, ["hull-0", "hull-2", "hull-3", "hull-4", "hull-5", "hull-6", "hull-7", "hull-8"]);
        assert_eq!(ship_icon_size(&ship(2, 2, Some(0.0)), 12.0), 22.0);
        assert_eq!(ship_icon_size(&ship(2, 7, Some(0.0)), 12.0), 18.0); // an unknown size reads as 18
        assert_close!(ship_icon_size(&ship(2, 0, None), 2.0), 15.0 * 0.7 * 0.62, 12);
        let scales: Vec<f64> = [0.0, 2.99, 3.0, 4.99, 5.0, 6.99, 7.0, 20.0].map(ship_icon_scale).to_vec();
        assert_eq!(scales, [0.7, 0.7, 0.85, 0.85, 0.92, 0.92, 1.0, 1.0]);
        assert_eq!(ship_icon_scale(f64::NAN), 1.0);
    }
}
