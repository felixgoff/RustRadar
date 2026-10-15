//! Trafikverket's track speed limits (STH), per train category and
//! direction, from `data/sth.json` (made by `scripts/sth.py` from BIS data,
//! CC0): for a piece of the track graph, the limits that apply going one
//! way along it.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::motion::{course, distance};

/// A track piece this far from the data's line is that line, metres.
const SNAP_M: f64 = 6.0;
/// The data's line and the track run within this of each other, degrees.
const PARALLEL: f64 = 30.0;
/// Grid cells of the index, degrees.
const CELL: f64 = 0.002;

/// Limits in km/h, per train category: A, B, C, S.
pub type Limits = [u16; 4];

#[derive(Deserialize)]
struct File {
    pieces: Vec<(u8, Limits, Vec<(f64, f64)>)>,
}

struct Piece {
    /// 0 both ways, 1 along the line as drawn, 2 against it.
    direction: u8,
    limits: Limits,
    /// (lat, lon).
    points: Vec<(f64, f64)>,
}

struct Index {
    pieces: Vec<Piece>,
    /// Segments (piece, first point) by the cells they cross.
    cells: HashMap<(i32, i32), Vec<(u32, u32)>>,
    /// The data's extent: south, west, north, east.
    bounds: (f64, f64, f64, f64),
}

fn cell(p: (f64, f64)) -> (i32, i32) {
    ((p.0 / CELL).floor() as i32, (p.1 / CELL).floor() as i32)
}

fn index() -> &'static Index {
    static I: OnceLock<Index> = OnceLock::new();
    I.get_or_init(|| build(include_str!("../data/sth.json")))
}

fn build(json: &str) -> Index {
    let file: File = serde_json::from_str(json).unwrap_or(File { pieces: Vec::new() });
    let mut cells: HashMap<(i32, i32), Vec<(u32, u32)>> = HashMap::new();
    let mut bounds = (90.0f64, 180.0f64, -90.0f64, -180.0f64);
    let pieces: Vec<Piece> = file
        .pieces
        .into_iter()
        .map(|(direction, limits, coords)| Piece {
            direction,
            limits,
            points: coords.into_iter().map(|(lon, lat)| (lat, lon)).collect(),
        })
        .collect();
    for (i, piece) in pieces.iter().enumerate() {
        for (j, w) in piece.points.windows(2).enumerate() {
            let (a, b) = (cell(w[0]), cell(w[1]));
            for x in a.0.min(b.0)..=a.0.max(b.0) {
                for y in a.1.min(b.1)..=a.1.max(b.1) {
                    cells.entry((x, y)).or_default().push((i as u32, j as u32));
                }
            }
            for p in w {
                bounds = (bounds.0.min(p.0), bounds.1.min(p.1), bounds.2.max(p.0), bounds.3.max(p.1));
            }
        }
    }
    Index { pieces, cells, bounds }
}

/// Whether there are limits anywhere near this box (south, west, north, east).
pub fn covers(south: f64, west: f64, north: f64, east: f64) -> bool {
    let b = index().bounds;
    !(north < b.0 || south > b.2 || east < b.1 || west > b.3)
}

fn turn(a: f64, b: f64) -> f64 {
    let d = (b - a).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// The nearest point to `p` on the segment `a`-`b`.
fn nearest(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let k = p.0.to_radians().cos();
    let (dx, dy) = ((b.1 - a.1) * k, b.0 - a.0);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { ((((p.1 - a.1) * k) * dx + (p.0 - a.0) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// The limits at `p` for travel on a course (degrees), if the data has the
/// track there: the nearest line running that way, with its direction.
fn at(ix: &Index, p: (f64, f64), heading: f64) -> Option<Limits> {
    let (cx, cy) = cell(p);
    let mut best: Option<(f64, Limits)> = None;
    for x in cx - 1..=cx + 1 {
        for y in cy - 1..=cy + 1 {
            for &(i, j) in ix.cells.get(&(x, y)).into_iter().flatten() {
                let piece = &ix.pieces[i as usize];
                let (a, b) = (piece.points[j as usize], piece.points[j as usize + 1]);
                let d = distance(p, nearest(p, a, b));
                if d > SNAP_M || best.is_some_and(|(bd, _)| bd <= d) {
                    continue;
                }
                let off = turn(heading, course(a, b));
                let along = off <= PARALLEL;
                let against = off >= 180.0 - PARALLEL;
                let applies = match piece.direction {
                    1 => along,
                    2 => against,
                    _ => along || against,
                };
                if applies {
                    best = Some((d, piece.limits));
                }
            }
        }
    }
    best.map(|b| b.1)
}

/// The limits going from `a` to `b` along a piece of track, where the data
/// has it: the lowest found a quarter, half and three quarters along.
pub fn along(a: (f64, f64), b: (f64, f64)) -> Option<Limits> {
    let ix = index();
    if distance(a, b) < 0.5 {
        return None;
    }
    let heading = course(a, b);
    let at_k = |k: f64| at(ix, (a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k), heading);
    [0.25, 0.5, 0.75]
        .into_iter()
        .filter_map(at_k)
        .reduce(|x, y| [x[0].min(y[0]), x[1].min(y[1]), x[2].min(y[2]), x[3].min(y[3])])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::ahead;

    #[test]
    fn limits_follow_the_direction_of_travel() {
        // a line drawn eastwards: 80 both ways, then 120 going east only and 100 going west only
        let a = (63.0, 18.0);
        let b = ahead(a, 90.0, 1_000.0);
        let c = ahead(b, 90.0, 1_000.0);
        let pt = |p: (f64, f64)| format!("[{},{}]", p.1, p.0);
        let json = format!(
            r#"{{"categories":["A","B","C","S"],"pieces":[
                [0,[80,80,90,95],[{},{}]],
                [1,[120,130,140,160],[{},{}]],
                [2,[100,100,100,100],[{},{}]]]}}"#,
            pt(a),
            pt(b),
            pt(b),
            pt(c),
            pt(b),
            pt(c)
        );
        let ix = build(&json);
        let east = |p| at(&ix, p, 90.0);
        let west = |p| at(&ix, p, 270.0);
        let first = ahead(a, 90.0, 500.0);
        let second = ahead(b, 90.0, 500.0);
        assert_eq!(east(first), Some([80, 80, 90, 95]));
        assert_eq!(west(first), Some([80, 80, 90, 95]));
        assert_eq!(east(second), Some([120, 130, 140, 160]));
        assert_eq!(west(second), Some([100, 100, 100, 100]));
        // across the line, or off it, nothing
        assert_eq!(at(&ix, second, 0.0), None);
        assert_eq!(at(&ix, ahead(second, 0.0, 20.0), 90.0), None);
    }

    #[test]
    fn the_shipped_data_loads() {
        let ix = index();
        assert!(ix.pieces.len() > 1000, "{}", ix.pieces.len());
        assert!(covers(64.0, 20.0, 64.1, 20.1));
        assert!(!covers(55.5, 12.9, 55.7, 13.2), "not Skåne");
    }
}
