//! Which way trains run on each track of double (and more) track, from
//! Trafikverket's traffic flow directions (`data/flow.json`, made by
//! `scripts/flow.py` from the INSPIRE layer, CC0): so a train is put on, and
//! routed along, the track running its way rather than the one beside it.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::lines::Lines;
use crate::motion::{course, distance};

/// A track piece this close to a one-way line is that line, metres: under
/// half the 4.5 m between tracks, so never the track beside it.
const SNAP_M: f64 = 2.0;

type RawPiece = (u8, Vec<(f64, f64)>);

#[derive(Deserialize)]
struct File {
    pieces: Vec<RawPiece>,
}

fn index() -> &'static Lines<()> {
    static I: OnceLock<Lines<()>> = OnceLock::new();
    I.get_or_init(|| build(include_str!("../data/flow.json")))
}

fn build(json: &str) -> Lines<()> {
    let pieces = serde_json::from_str::<File>(json).map(|f| f.pieces).unwrap_or_default();
    Lines::new(pieces.into_iter().map(|(direction, points)| (direction, (), points)).collect())
}

/// Whether there are one-way tracks anywhere near this box (south, west, north, east).
pub fn covers(south: f64, west: f64, north: f64, east: f64) -> bool {
    index().covers(south, west, north, east)
}

fn with_flow_in(ix: &Lines<()>, a: (f64, f64), b: (f64, f64)) -> Option<bool> {
    if distance(a, b) < 0.5 {
        return None;
    }
    let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let (piece, along) = ix.nearest(mid, course(a, b), SNAP_M, |_, _| true)?;
    Some(along == (piece.direction == 1))
}

/// Going from `a` to `b` along a piece of track: with the way trains run on
/// it (true), against it (false), or either way / not known (none).
pub fn with_flow(a: (f64, f64), b: (f64, f64)) -> Option<bool> {
    with_flow_in(index(), a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::ahead;

    #[test]
    fn double_track_runs_each_way_on_its_own_track() {
        // two tracks 4.5 m apart, both drawn eastwards: the northern one runs
        // east ("in direction"), the southern one west ("in opposite direction")
        let a = (59.0, 18.0);
        let north_a = ahead(a, 0.0, 4.5);
        let pt = |p: (f64, f64)| format!("[{},{}]", p.1, p.0);
        let json = format!(
            r#"{{"pieces":[[1,[{},{}]],[2,[{},{}]]]}}"#,
            pt(north_a),
            pt(ahead(north_a, 90.0, 2_000.0)),
            pt(a),
            pt(ahead(a, 90.0, 2_000.0))
        );
        let ix = build(&json);
        let east = |from: (f64, f64)| with_flow_in(&ix, from, ahead(from, 90.0, 100.0));
        let west = |from: (f64, f64)| with_flow_in(&ix, ahead(from, 90.0, 100.0), from);
        let (n, s) = (ahead(north_a, 90.0, 500.0), ahead(a, 90.0, 500.0));
        assert_eq!((east(n), west(n)), (Some(true), Some(false)), "north track runs east");
        assert_eq!((east(s), west(s)), (Some(false), Some(true)), "south track runs west");
        assert_eq!(east(ahead(s, 180.0, 20.0)), None, "away from the tracks");
    }

    #[test]
    fn the_shipped_data_loads() {
        assert!(index().len() > 3000);
        assert!(covers(55.5, 12.9, 55.7, 13.2), "Skåne");
    }
}

