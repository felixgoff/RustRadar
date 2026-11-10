//! Port of `taxinet.ts` (S1.15, doc 05 §9.1, doc 03 §16): an airport's taxiway network
//! (OpenStreetMap centrelines) indexed for quick "which lines pass near this point" lookups. The
//! motion model asks it for every taxiing aircraft every frame, so the lines are cut into straight
//! segments in a local east/north metre frame and bucketed in a grid.
//!
//! Differences from the TypeScript, none of which changes an answer for an airport's lines:
//! - the grid is keyed by `(cx, cy)` rather than `(cx + 32768) * 65536 + (cy + 32768)`, which
//!   only told cells apart within 1 638 km of the origin;
//! - [`project`] returns the nearest point and its distance instead of writing the module's
//!   `projX`, `projY` and `projDist`;
//! - the segment arrays are read through methods (`ax()`, …), so that they stay as built.
//!
//! `Math.min`/`Math.max` keep NaN as in JavaScript (`js_min`/`js_max`).

use std::collections::HashMap;

use crate::model::TaxiLine;
use crate::trains::{js_max, js_min};

const DEG: f64 = std::f64::consts::PI / 180.0;
const M_PER_DEG: f64 = 6_371_000.0 * DEG;
/// Grid cell size, m. Each segment is listed in the cells within one cell of it, so lookups reach
/// at least this far.
const CELL_M: f64 = 50.0;
/// Points further than this from the airport's lines are outside the network, m.
const MARGIN_M: f64 = 100.0;

/// The grid cell of a local point; `None` for a point that is not a number (`Math.floor(NaN)`
/// makes a key no cell has in the TypeScript).
fn cell_of(x: f64, y: f64) -> Option<(i32, i32)> {
    let (cx, cy) = ((x / CELL_M).floor(), (y / CELL_M).floor());
    // `as` saturates the infinities onto cells no segment reaches
    (!cx.is_nan() && !cy.is_nan()).then_some((cx as i32, cy as i32))
}

/// An airport's taxiway, taxilane and runway centrelines as straight segments in a local frame,
/// with a grid of the segments that pass near each cell.
#[derive(Debug, Clone, PartialEq)]
pub struct TaxiNetwork {
    /// The local frame's origin, degrees: the mean of all the lines' points.
    lat0: f64,
    lon0: f64,
    /// The local frame's scale, m per degree of longitude.
    kx: f64,
    /// Segments: start (ax, ay), unit direction (ux, uy) and length, m.
    ax: Vec<f64>,
    ay: Vec<f64>,
    ux: Vec<f64>,
    uy: Vec<f64>,
    len: Vec<f64>,
    cells: HashMap<(i32, i32), Vec<usize>>,
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

impl TaxiNetwork {
    pub fn new(lines: &[TaxiLine]) -> Self {
        let mut s_lat = 0.0;
        let mut s_lon = 0.0;
        let mut n = 0_usize;
        for line in lines {
            for &[lon, lat] in &line.points {
                s_lat += lat;
                s_lon += lon;
                n += 1;
            }
        }
        let lat0 = if n > 0 { s_lat / n as f64 } else { 0.0 };
        let lon0 = if n > 0 { s_lon / n as f64 } else { 0.0 };
        let mut net = TaxiNetwork {
            lat0,
            lon0,
            kx: M_PER_DEG * js_max((lat0 * DEG).cos(), 1e-3),
            ax: Vec::new(),
            ay: Vec::new(),
            ux: Vec::new(),
            uy: Vec::new(),
            len: Vec::new(),
            cells: HashMap::new(),
            min_x: f64::INFINITY,
            min_y: f64::INFINITY,
            max_x: f64::NEG_INFINITY,
            max_y: f64::NEG_INFINITY,
        };
        for line in lines {
            for i in 1..line.points.len() {
                let [lon_a, lat_a] = line.points[i - 1];
                let [lon_b, lat_b] = line.points[i];
                let xa = net.x(lon_a);
                let ya = (lat_a - net.lat0) * M_PER_DEG;
                let xb = net.x(lon_b);
                let yb = (lat_b - net.lat0) * M_PER_DEG;
                let l = (xb - xa).hypot(yb - ya);
                // `!(l > 0.1) || !Number.isFinite(l)`: NaN is not finite
                if l <= 0.1 || !l.is_finite() {
                    continue;
                }
                let seg = net.ax.len();
                net.ax.push(xa);
                net.ay.push(ya);
                net.ux.push((xb - xa) / l);
                net.uy.push((yb - ya) / l);
                net.len.push(l);
                net.min_x = js_min(js_min(net.min_x, xa), xb);
                net.max_x = js_max(js_max(net.max_x, xa), xb);
                net.min_y = js_min(js_min(net.min_y, ya), yb);
                net.max_y = js_max(js_max(net.max_y, ya), yb);
                // listed in every cell within one of the cells it passes through
                let steps = (l / (CELL_M / 2.0)).ceil();
                for k in 0..=(steps as u64) {
                    let t = k as f64 / steps;
                    let cx = ((xa + (xb - xa) * t) / CELL_M).floor();
                    let cy = ((ya + (yb - ya) * t) / CELL_M).floor();
                    for i2 in -1..=1 {
                        for j2 in -1..=1 {
                            // the sum in floats, as the TypeScript's key: no overflow far out
                            let key = ((cx + f64::from(i2)) as i32, (cy + f64::from(j2)) as i32);
                            let list = net.cells.entry(key).or_default();
                            if list.last() != Some(&seg) {
                                list.push(seg);
                            }
                        }
                    }
                }
            }
        }
        net
    }

    /// The local frame's origin, degrees.
    pub fn lat0(&self) -> f64 {
        self.lat0
    }

    pub fn lon0(&self) -> f64 {
        self.lon0
    }

    /// Segment starts, east of the origin, m.
    pub fn ax(&self) -> &[f64] {
        &self.ax
    }

    /// Segment starts, north of the origin, m.
    pub fn ay(&self) -> &[f64] {
        &self.ay
    }

    /// Segment directions, east part of the unit vector.
    pub fn ux(&self) -> &[f64] {
        &self.ux
    }

    /// Segment directions, north part of the unit vector.
    pub fn uy(&self) -> &[f64] {
        &self.uy
    }

    /// Segment lengths, m (the TypeScript's `len`).
    pub fn lens(&self) -> &[f64] {
        &self.len
    }

    /// The number of segments.
    pub fn count(&self) -> usize {
        self.ax.len()
    }

    /// East of the origin, m, for a longitude (across the antimeridian too).
    pub fn x(&self, lon: f64) -> f64 {
        ((((lon - self.lon0 + 540.0) % 360.0) + 360.0) % 360.0 - 180.0) * self.kx
    }

    /// North of the origin, m, for a latitude.
    pub fn y(&self, lat: f64) -> f64 {
        (lat - self.lat0) * M_PER_DEG
    }

    /// Whether a local point is near enough to the lines to look them up.
    pub fn covers(&self, x: f64, y: f64) -> bool {
        x >= self.min_x - MARGIN_M
            && x <= self.max_x + MARGIN_M
            && y >= self.min_y - MARGIN_M
            && y <= self.max_y + MARGIN_M
    }

    /// The segments that may pass within `CELL_M` of a local point (and some further), or `None`.
    pub fn near(&self, x: f64, y: f64) -> Option<&[usize]> {
        self.cells.get(&cell_of(x, y)?).map(Vec::as_slice)
    }

    /// Metres per degree of longitude in the local frame.
    pub fn metres_per_deg_lon(&self) -> f64 {
        self.kx
    }
}

/// The point of segment `s` nearest to (x, y), and its distance: `(projX, projY, projDist)`.
pub fn project(net: &TaxiNetwork, s: usize, x: f64, y: f64) -> (f64, f64, f64) {
    let ax = net.ax[s];
    let ay = net.ay[s];
    let ux = net.ux[s];
    let uy = net.uy[s];
    let mut t = (x - ax) * ux + (y - ay) * uy;
    let l = net.len[s];
    t = if t < 0.0 {
        0.0
    } else if t > l {
        l
    } else {
        t
    };
    let proj_x = ax + ux * t;
    let proj_y = ay + uy * t;
    (proj_x, proj_y, (x - proj_x).hypot(y - proj_y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;
    use crate::model::TaxiKind;

    fn line(points: &[[f64; 2]]) -> TaxiLine {
        TaxiLine { kind: TaxiKind::Taxiway, r#ref: None, points: points.to_vec() }
    }

    /// `[lon, lat]` of a point `e` m east and `n` m north of 45°N 10°E, in the frame of a
    /// network whose mean point is there.
    fn at(e: f64, n: f64) -> [f64; 2] {
        [10.0 + e / (M_PER_DEG * (45.0 * DEG).cos()), 45.0 + n / M_PER_DEG]
    }

    /// An L: 400 m north along x = 0, then 400 m east, balanced by its mirror image so that the
    /// mean of the points is the origin.
    fn el() -> Vec<TaxiLine> {
        vec![
            line(&[at(0.0, -400.0), at(0.0, 0.0), at(400.0, 0.0)]),
            line(&[at(0.0, 400.0), at(0.0, 0.0), at(-400.0, 0.0)]),
        ]
    }

    /// Distance from a local point to segment `s`, by brute force over its length.
    fn distance_to(net: &TaxiNetwork, s: usize, x: f64, y: f64) -> f64 {
        let (ax, ay, ux, uy, l) = (net.ax()[s], net.ay()[s], net.ux()[s], net.uy()[s], net.lens()[s]);
        (0..=1000)
            .map(|i| {
                let t = l * i as f64 / 1000.0;
                (x - ax - ux * t).hypot(y - ay - uy * t)
            })
            .fold(f64::INFINITY, f64::min)
    }

    #[test]
    fn the_local_frame_is_centred_on_the_mean_point() {
        let net = TaxiNetwork::new(&el());
        assert_close!(net.lat0(), 45.0, 12);
        assert_close!(net.lon0(), 10.0, 12);
        assert_eq!(net.metres_per_deg_lon(), M_PER_DEG * (net.lat0() * DEG).cos());
        let [lon, lat] = at(123.0, -45.0);
        assert_close!(net.x(lon), 123.0, 6);
        assert_close!(net.y(lat), -45.0, 6);
        // the same place a turn of the globe away
        assert_close!(net.x(lon + 360.0), 123.0, 6);
        assert_close!(net.x(lon - 720.0), 123.0, 6);
    }

    #[test]
    fn lines_become_unit_segments_and_points_under_ten_centimetres_apart_are_skipped() {
        let mut lines = el();
        lines.push(line(&[at(1000.0, 1000.0), at(1000.05, 1000.0), at(1000.0, 1000.0)]));
        lines.push(line(&[at(2000.0, 0.0)]));
        let net = TaxiNetwork::new(&lines);
        assert_eq!(net.count(), 4);
        for s in 0..net.count() {
            assert_close!(net.ux()[s].hypot(net.uy()[s]), 1.0, 12);
            assert_close!(net.lens()[s], 400.0, 1);
        }
        let net_mean = (net.lat0(), net.lon0());
        // the short lines' points still count towards the origin
        assert!(net_mean.0 > 45.0 && net_mean.1 > 10.0);
        // north along x = 0, from 400 m south
        assert_close!(net.uy()[0], 1.0, 9);
        assert_close!(net.ux()[0], 0.0, 9);
    }

    #[test]
    fn the_grid_lists_every_segment_near_a_point() {
        let net = TaxiNetwork::new(&el());
        // a cheap deterministic scatter over the L and around it
        let mut seed = 12345_u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut checked = 0;
        for _ in 0..3000 {
            let x = next() * 1000.0 - 500.0;
            let y = next() * 1000.0 - 500.0;
            let list = net.near(x, y).unwrap_or(&[]);
            for s in 0..net.count() {
                // samples every 25 m, listed a cell around: anything within 37.5 m is found
                if distance_to(&net, s, x, y) <= CELL_M * 0.75 {
                    assert!(list.contains(&s), "segment {s} missing near ({x}, {y})");
                    checked += 1;
                }
            }
            // and nothing listed is far away: the 3 x 3 cells around a sample reach under 2.5 cells
            for &s in list {
                assert!(distance_to(&net, s, x, y) < CELL_M * 2.5 * std::f64::consts::SQRT_2);
            }
        }
        assert!(checked > 300, "{checked}");
    }

    #[test]
    fn a_cell_lists_a_segment_once() {
        let net = TaxiNetwork::new(&el());
        for list in net.cells.values() {
            let mut sorted = list.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), list.len());
        }
        // where the four legs meet, all four are listed
        let mut corner = net.near(0.0, 0.0).unwrap().to_vec();
        corner.sort_unstable();
        assert_eq!(corner, [0, 1, 2, 3]);
        // far from every line: no cell at all
        assert_eq!(net.near(300.0, 300.0), None);
        assert_eq!(net.near(f64::NAN, 0.0), None);
        assert_eq!(net.near(f64::INFINITY, 0.0), None);
    }

    #[test]
    fn covers_is_the_bounding_box_with_a_margin() {
        let net = TaxiNetwork::new(&el());
        assert!(net.covers(0.0, 0.0));
        assert!(net.covers(499.0, 499.0));
        assert!(net.covers(-499.0, -499.0));
        assert!(!net.covers(501.0, 0.0));
        assert!(!net.covers(0.0, -501.0));
        assert!(!net.covers(f64::NAN, 0.0));
    }

    #[test]
    fn an_empty_network_covers_nothing() {
        let net = TaxiNetwork::new(&[]);
        assert_eq!((net.lat0(), net.lon0(), net.count()), (0.0, 0.0, 0));
        assert_eq!(net.metres_per_deg_lon(), M_PER_DEG);
        assert!(!net.covers(0.0, 0.0));
        assert_eq!(net.near(0.0, 0.0), None);
        let single = TaxiNetwork::new(&[line(&[[10.0, 45.0]])]);
        assert_eq!(single.count(), 0);
        assert!(!single.covers(0.0, 0.0));
    }

    #[test]
    fn a_network_across_the_antimeridian_is_continuous() {
        let net = TaxiNetwork::new(&[line(&[[179.999, -17.0], [179.9995, -17.0], [-179.999, -17.0]])]);
        // the origin is the plain mean of the longitudes, far from the lines, but `x` wraps the
        // difference, so the lines stay together
        assert_close!(net.lon0(), (179.999 + 179.9995 - 179.999) / 3.0, 12);
        assert_eq!(net.count(), 2);
        // both legs point east, each well under a kilometre
        for s in 0..net.count() {
            assert_close!(net.ux()[s], 1.0, 9);
            assert!(net.lens()[s] < 200.0);
        }
        assert_close!(net.x(-179.999) - net.x(179.999), 0.002 * net.metres_per_deg_lon(), 6);
        // and near the poles the scale stays positive
        let polar = TaxiNetwork::new(&[line(&[[0.0, 90.0], [180.0, 90.0]])]);
        assert_eq!(polar.metres_per_deg_lon(), M_PER_DEG * 1e-3);
    }

    #[test]
    fn project_clamps_to_the_segment() {
        let net = TaxiNetwork::new(&[line(&[at(-100.0, 0.0), at(100.0, 0.0)])]);
        assert_eq!(net.count(), 1);
        let ay = net.ay()[0];
        // beside the middle: the foot of the perpendicular
        let (px, py, d) = project(&net, 0, 30.0, ay + 12.0);
        assert_close!(px, 30.0, 9);
        assert_close!(py, ay, 9);
        assert_close!(d, 12.0, 9);
        // beyond either end: the end
        let (px, py, d) = project(&net, 0, 130.0, ay - 40.0);
        assert_close!(px, net.ax()[0] + net.lens()[0], 9);
        assert_close!(py, ay, 9);
        assert_close!(d, 50.0, 6);
        let (px, _, d) = project(&net, 0, -103.0, ay + 4.0);
        assert_eq!(px, net.ax()[0]);
        assert_close!(d, 5.0, 6);
    }
}
