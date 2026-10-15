//! Lines from Trafikverket's data (speed limits, traffic flow directions)
//! with something about each, indexed so a piece of the track graph can find
//! the line it lies on and how it runs along it.

use std::collections::HashMap;

use crate::motion::{course, distance};

/// Grid cells of the index, degrees.
const CELL: f64 = 0.002;
/// The data's line and the track run within this of each other, degrees.
const PARALLEL: f64 = 30.0;

pub struct Piece<T> {
    /// 0 both ways, 1 along the line as drawn, 2 against it.
    pub direction: u8,
    pub value: T,
    /// (lat, lon).
    points: Vec<(f64, f64)>,
}

/// A piece as the data files have it: direction, value, `[lon, lat]` points.
pub type Raw<T> = (u8, T, Vec<(f64, f64)>);

/// Segments (piece, first point) by the grid cells they cross.
type Cells = HashMap<(i32, i32), Vec<(u32, u32)>>;

pub struct Lines<T> {
    pieces: Vec<Piece<T>>,
    cells: Cells,
    /// The extent: south, west, north, east.
    bounds: (f64, f64, f64, f64),
}

fn cell(p: (f64, f64)) -> (i32, i32) {
    ((p.0 / CELL).floor() as i32, (p.1 / CELL).floor() as i32)
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

impl<T> Lines<T> {
    /// From pieces as the files have them: direction, value, `[lon, lat]` points.
    pub fn new(raw: Vec<Raw<T>>) -> Lines<T> {
        let pieces: Vec<Piece<T>> = raw
            .into_iter()
            .map(|(direction, value, coords)| Piece {
                direction,
                value,
                points: coords.into_iter().map(|(lon, lat)| (lat, lon)).collect(),
            })
            .collect();
        let mut cells: Cells = HashMap::new();
        let mut bounds = (90.0f64, 180.0f64, -90.0f64, -180.0f64);
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
        Lines { pieces, cells, bounds }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    /// Whether there are lines anywhere near this box (south, west, north, east).
    pub fn covers(&self, south: f64, west: f64, north: f64, east: f64) -> bool {
        let b = self.bounds;
        !(north < b.0 || south > b.2 || east < b.1 || west > b.3)
    }

    /// The nearest line within `within` metres of `p` that runs parallel to
    /// travel on `heading` and that `keep` accepts, with whether the travel
    /// goes the way the line is drawn.
    pub fn nearest(
        &self,
        p: (f64, f64),
        heading: f64,
        within: f64,
        keep: impl Fn(&Piece<T>, bool) -> bool,
    ) -> Option<(&Piece<T>, bool)> {
        let (cx, cy) = cell(p);
        let mut best: Option<(f64, &Piece<T>, bool)> = None;
        for x in cx - 1..=cx + 1 {
            for y in cy - 1..=cy + 1 {
                for &(i, j) in self.cells.get(&(x, y)).into_iter().flatten() {
                    let piece = &self.pieces[i as usize];
                    let (a, b) = (piece.points[j as usize], piece.points[j as usize + 1]);
                    let d = distance(p, nearest(p, a, b));
                    if d > within || best.is_some_and(|(bd, _, _)| bd <= d) {
                        continue;
                    }
                    let off = turn(heading, course(a, b));
                    let along = off <= PARALLEL;
                    if (along || off >= 180.0 - PARALLEL) && keep(piece, along) {
                        best = Some((d, piece, along));
                    }
                }
            }
        }
        best.map(|(_, piece, along)| (piece, along))
    }
}
