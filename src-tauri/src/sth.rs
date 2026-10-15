//! Trafikverket's track speed limits (STH), per train category and
//! direction, from `data/sth.json` (made by `scripts/sth.py` from BIS data,
//! CC0): for a piece of the track graph, the limits that apply going one
//! way along it.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::lines::Lines;
use crate::motion::{course, distance};

/// A track piece this far from the data's line is that line, metres.
const SNAP_M: f64 = 6.0;

/// Limits in km/h, per train category: A, B, C, S.
pub type Limits = [u16; 4];

/// A piece as the file has it: direction, limits, `[lon, lat]` points.
type RawPiece = (u8, Limits, Vec<(f64, f64)>);

#[derive(Deserialize)]
struct File {
    pieces: Vec<RawPiece>,
}

fn index() -> &'static Lines<Limits> {
    static I: OnceLock<Lines<Limits>> = OnceLock::new();
    I.get_or_init(|| build(include_str!("../data/sth.json")))
}

fn build(json: &str) -> Lines<Limits> {
    Lines::new(serde_json::from_str::<File>(json).map(|f| f.pieces).unwrap_or_default())
}

/// Whether there are limits anywhere near this box (south, west, north, east).
pub fn covers(south: f64, west: f64, north: f64, east: f64) -> bool {
    index().covers(south, west, north, east)
}

/// The limits at `p` for travel on a course (degrees), if the data has the
/// track there: the nearest line whose limits are for that direction.
fn at(ix: &Lines<Limits>, p: (f64, f64), heading: f64) -> Option<Limits> {
    let applies = |piece: &crate::lines::Piece<Limits>, along: bool| match piece.direction {
        1 => along,
        2 => !along,
        _ => true,
    };
    ix.nearest(p, heading, SNAP_M, applies).map(|(piece, _)| piece.value)
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
        assert!(ix.len() > 1000, "{}", ix.len());
        assert!(covers(64.0, 20.0, 64.1, 20.1));
        assert!(!covers(55.5, 12.9, 55.7, 13.2), "not Skåne");
    }
}
