//! Dubins paths (`flightpath.ts:157-338`): the shortest path between two poses for a vehicle that
//! can't turn tighter than radius r, two arcs joined by a straight line or three arcs; and one
//! turn then a straight line, for an end without a heading.
//!
//! Angles here are mathematical (counter-clockwise from east, radians) inside, compass degrees in
//! the arguments; L turns left. Coordinates are a local frame in metres (x east, y north).

use super::{ARC_STEP, LINE_STEP_M, js_max, mod2pi, to_math};
use std::f64::consts::PI;

/// One piece of a path: a left arc, a right arc, or a straight line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Piece {
    L,
    R,
    S,
}

use Piece::{L, R, S};

/// A Dubins word and the normalised lengths of its three pieces: radians for arcs, radii for the
/// straight.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    word: [Piece; 3],
    lengths: [f64; 3],
}

/// The standard closed forms for the six words, in the order LSL, RSR, LSR, RSL, RLR, LRL; a word
/// with no solution is left out.
fn dubins_candidates(alpha: f64, beta: f64, d: f64) -> [Option<Candidate>; 6] {
    let (sa, sb, ca, cb) = (alpha.sin(), beta.sin(), alpha.cos(), beta.cos());
    let cab = (alpha - beta).cos();
    let mut out = [None; 6];
    let mut p2 = 2.0 + d * d - 2.0 * cab + 2.0 * d * (sa - sb);
    if p2 >= 0.0 {
        let tmp = (cb - ca).atan2(d + sa - sb);
        out[0] = Some(Candidate { word: [L, S, L], lengths: [mod2pi(-alpha + tmp), p2.sqrt(), mod2pi(beta - tmp)] });
    }
    p2 = 2.0 + d * d - 2.0 * cab + 2.0 * d * (sb - sa);
    if p2 >= 0.0 {
        let tmp = (ca - cb).atan2(d - sa + sb);
        out[1] = Some(Candidate { word: [R, S, R], lengths: [mod2pi(alpha - tmp), p2.sqrt(), mod2pi(-beta + tmp)] });
    }
    p2 = -2.0 + d * d + 2.0 * cab + 2.0 * d * (sa + sb);
    if p2 >= 0.0 {
        let p = p2.sqrt();
        let tmp = (-ca - cb).atan2(d + sa + sb) - (-2.0_f64).atan2(p);
        out[2] = Some(Candidate { word: [L, S, R], lengths: [mod2pi(-alpha + tmp), p, mod2pi(-mod2pi(beta) + tmp)] });
    }
    p2 = -2.0 + d * d + 2.0 * cab - 2.0 * d * (sa + sb);
    if p2 >= 0.0 {
        let p = p2.sqrt();
        let tmp = (ca + cb).atan2(d - sa - sb) - 2.0_f64.atan2(p);
        out[3] = Some(Candidate { word: [R, S, L], lengths: [mod2pi(alpha - tmp), p, mod2pi(beta - tmp)] });
    }
    let mut c = (6.0 - d * d + 2.0 * cab + 2.0 * d * (sa - sb)) / 8.0;
    if c.abs() <= 1.0 {
        let p = mod2pi(2.0 * PI - c.acos());
        let t = mod2pi(alpha - (ca - cb).atan2(d - sa + sb) + p / 2.0);
        out[4] = Some(Candidate { word: [R, L, R], lengths: [t, p, mod2pi(alpha - beta - t + p)] });
    }
    c = (6.0 - d * d + 2.0 * cab + 2.0 * d * (sb - sa)) / 8.0;
    if c.abs() <= 1.0 {
        let p = mod2pi(2.0 * PI - c.acos());
        let t = mod2pi(-alpha - (ca - cb).atan2(d + sa - sb) + p / 2.0);
        out[5] = Some(Candidate { word: [L, R, L], lengths: [t, p, mod2pi(mod2pi(beta) - alpha - t + p)] });
    }
    out
}

/// A position and a mathematical angle (radians).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Pose2 {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) a: f64,
}

/// Moves along one piece, `len` normalised as in [`Candidate`].
fn advance(p: Pose2, kind: Piece, len: f64, r: f64) -> Pose2 {
    let Pose2 { x, y, a } = p;
    match kind {
        L => Pose2 { x: x + r * ((a + len).sin() - a.sin()), y: y + r * (a.cos() - (a + len).cos()), a: a + len },
        R => Pose2 { x: x + r * (a.sin() - (a - len).sin()), y: y + r * ((a - len).cos() - a.cos()), a: a - len },
        S => Pose2 { x: x + r * len * a.cos(), y: y + r * len * a.sin(), a },
    }
}

/// Points along a sequence of pieces, the start included: arcs every [`ARC_STEP`], straights
/// every [`LINE_STEP_M`].
fn sample_pieces(start: Pose2, pieces: &[(Piece, f64)], r: f64) -> Vec<[f64; 2]> {
    let mut out = vec![[start.x, start.y]];
    let mut p = start;
    for &(kind, len) in pieces {
        if len <= 1e-9 {
            continue;
        }
        let steps = if kind == S { (len * r) / LINE_STEP_M } else { len / ARC_STEP };
        let n = js_max(1.0, steps.ceil());
        // a NaN count samples nothing (`i <= NaN` is false); `as` would give 0 too
        let n_int = if n.is_nan() { 0 } else { n as usize };
        for i in 1..=n_int {
            let q = advance(p, kind, (len * i as f64) / n, r);
            out.push([q.x, q.y]);
        }
        p = advance(p, kind, len, r);
    }
    out
}

/// A planned local path: where it starts, its pieces and turn radius, where it ends and how long
/// it is (metres).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Plan {
    pub(super) start: Pose2,
    pub(super) pieces: Vec<(Piece, f64)>,
    pub(super) r: f64,
    pub(super) to: [f64; 2],
    pub(super) length: f64,
}

/// Points along a plan, both ends included exactly.
pub(super) fn sample_plan(plan: &Plan) -> Vec<[f64; 2]> {
    let mut points = sample_pieces(plan.start, &plan.pieces, plan.r);
    let last = points.len() - 1;
    points[last] = plan.to;
    points
}

/// A straight line, for when no turning path exists (the target is inside the turn).
fn direct_plan(x0: f64, y0: f64, x1: f64, y1: f64) -> Plan {
    let length = (x1 - x0).hypot(y1 - y0);
    Plan {
        start: Pose2 { x: x0, y: y0, a: (y1 - y0).atan2(x1 - x0) },
        pieces: vec![(S, 1.0)],
        r: length,
        to: [x1, y1],
        length,
    }
}

/// The shortest smooth path from (x0, y0) heading `h0` to (x1, y1) heading `h1` (compass
/// degrees) with turns of radius `r`. Only candidates that really arrive are kept (within
/// `max(1, r·1e-4)` m and 1e-3 rad, which guards the closed forms' edge cases); with none, the
/// straight line.
pub(super) fn dubins_plan(x0: f64, y0: f64, h0: f64, x1: f64, y1: f64, h1: f64, r: f64) -> Plan {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let theta = dy.atan2(dx);
    let d = dx.hypot(dy) / r;
    let alpha = mod2pi(to_math(h0) - theta);
    let beta = mod2pi(to_math(h1) - theta);
    let start = Pose2 { x: x0, y: y0, a: to_math(h0) };
    let mut best: Option<Vec<(Piece, f64)>> = None;
    let mut best_length = f64::INFINITY;
    for Candidate { word, lengths } in dubins_candidates(alpha, beta, d).into_iter().flatten() {
        let total = lengths[0] + lengths[1] + lengths[2];
        if total >= best_length {
            continue;
        }
        // keep only solutions that really arrive (guards the closed forms' edge cases)
        let pieces: Vec<(Piece, f64)> = word.into_iter().zip(lengths).collect();
        let mut end = start;
        for &(k, len) in &pieces {
            end = advance(end, k, len, r);
        }
        let heading_error = (end.a - to_math(h1)).sin().atan2((end.a - to_math(h1)).cos()).abs();
        if (end.x - x1).hypot(end.y - y1) > js_max(1.0, r * 1e-4) || heading_error > 1e-3 {
            continue;
        }
        best = Some(pieces);
        best_length = total;
    }
    match best {
        None => direct_plan(x0, y0, x1, y1),
        Some(pieces) => Plan { start, pieces, r, to: [x1, y1], length: best_length * r },
    }
}

/// The shortest smooth path from (x0, y0) heading `h0` to (x1, y1) heading `h1` (compass
/// degrees) with turns of radius `r`, as local points (`flightpath.ts` `dubins`, used only by
/// the tests).
#[cfg(test)]
pub(crate) fn dubins(x0: f64, y0: f64, h0: f64, x1: f64, y1: f64, h1: f64, r: f64) -> Vec<[f64; 2]> {
    sample_plan(&dubins_plan(x0, y0, h0, x1, y1, h1, r))
}

/// The shortest smooth path from (x0, y0) heading `h0` to the point (x1, y1), arriving at
/// whatever heading: one turn (left or right, whichever is shorter and whose circle does not
/// hold the target), then straight; with neither, the straight line.
pub(super) fn turn_then_straight_plan(x0: f64, y0: f64, h0: f64, x1: f64, y1: f64, r: f64) -> Plan {
    let a = to_math(h0);
    let mut best: Option<Vec<(Piece, f64)>> = None;
    let mut best_length = f64::INFINITY;
    for side in [1.0, -1.0] {
        // the turn circle's centre: left of the nose for a left turn
        let cx = x0 - side * r * a.sin();
        let cy = y0 + side * r * a.cos();
        let vx = x1 - cx;
        let vy = y1 - cy;
        let dc = vx.hypot(vy);
        if dc < r {
            continue;
        }
        let straight = (dc * dc - r * r).sqrt();
        let tangent = vy.atan2(vx) - side * straight.atan2(r);
        let turn = if side > 0.0 { mod2pi(tangent - (a - PI / 2.0)) } else { mod2pi(a + PI / 2.0 - tangent) };
        let length = turn * r + straight;
        if length < best_length {
            best_length = length;
            best = Some(vec![(if side > 0.0 { L } else { R }, turn), (S, straight / r)]);
        }
    }
    match best {
        None => direct_plan(x0, y0, x1, y1),
        Some(pieces) => Plan { start: Pose2 { x: x0, y: y0, a }, pieces, r, to: [x1, y1], length: best_length },
    }
}

/// [`turn_then_straight_plan`] as local points (`flightpath.ts` `turnThenStraight`, used only by
/// the tests).
#[cfg(test)]
pub(crate) fn turn_then_straight(x0: f64, y0: f64, h0: f64, x1: f64, y1: f64, r: f64) -> Vec<[f64; 2]> {
    sample_plan(&turn_then_straight_plan(x0, y0, h0, x1, y1, r))
}
