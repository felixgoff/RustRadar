//! Port of `train-cars.ts` (S1.14, doc 05 §12.6): a train's cars, laid along the track behind
//! where it is drawn.
//!
//! Flat metres: 111 320 per degree, longitude scaled by the cosine of the first point's latitude.

use std::f64::consts::PI;

use crate::trains::{Frame, js_min};

/// How long a car is drawn, metres (a coach with its coupling); as `CAR_M` in the backend.
pub const CAR_M: f64 = 26.0;
/// The gap drawn between cars, metres: buffer to buffer, as coupled cars stand.
pub const GAP_M: f64 = 0.6;
/// A train's cars where its operator doesn't say how many.
pub const DEFAULT_CARS: u32 = 4;

/// `[lat, lon]`.
type LatLon = (f64, f64);

fn metres(a: LatLon, b: LatLon) -> f64 {
    let k = (a.0 * PI / 180.0).cos();
    ((b.0 - a.0) * 111_320.0).hypot((b.1 - a.1) * 111_320.0 * k)
}

/// Each car of a train as a `[lon, lat]` line, front first: back from where it is drawn at
/// `now_ms` along its frames (`(lat, lon, ms)`), then along `tail` (the track behind where its
/// frames start, `(lat, lon)`), and straight on back where that runs out. Every car a coach
/// ([`CAR_M`] less the gap) with [`GAP_M`] between.
pub fn car_lines(frames: &[Frame], tail: &[(f64, f64)], cars: usize, now_ms: i64) -> Vec<Vec<[f64; 2]>> {
    car_lines_with(frames, tail, cars, now_ms, |_| CAR_M - GAP_M, GAP_M)
}

/// [`car_lines`] with each car's length, metres (`length_of(car)`, front car 0), and the gap
/// between cars (none for models: their lengths run to halfway across it).
pub fn car_lines_with(
    frames: &[Frame],
    tail: &[(f64, f64)],
    cars: usize,
    now_ms: i64,
    length_of: impl Fn(usize) -> f64,
    gap: f64,
) -> Vec<Vec<[f64; 2]>> {
    let Some(first) = frames.first() else {
        return Vec::new();
    };
    // the way it has come and goes: the tail reversed, up to the frames' start, then the frames
    let mut path: Vec<LatLon> = tail.iter().skip(1).rev().copied().collect();
    path.push((first.0, first.1));
    let lead = path.len() - 1;
    for f in &frames[1..] {
        let last = path[path.len() - 1];
        if f.0 != last.0 || f.1 != last.1 {
            path.push((f.0, f.1));
        }
    }
    let mut at: Vec<f64> = vec![0.0];
    for i in 1..path.len() {
        at.push(at[i - 1] + metres(path[i - 1], path[i]));
    }
    // how far along the head is now
    let mut head = at[lead];
    for pair in frames.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if now_ms <= a.2 {
            break;
        }
        let k = if b.2 > a.2 { js_min(1.0, (now_ms - a.2) as f64 / (b.2 - a.2) as f64) } else { 1.0 };
        head += metres((a.0, a.1), (b.0, b.1)) * k;
        if now_ms < b.2 {
            break;
        }
    }
    // the point `s` metres along (before the start: straight on back along its first stretch)
    let point = |s: f64| -> LatLon {
        if path.len() < 2 {
            return path[0];
        }
        if s <= 0.0 {
            let (p, q) = (path[0], path[1]);
            let k = if at[1] > 0.0 { s / at[1] } else { 0.0 };
            return (p.0 + (q.0 - p.0) * k, p.1 + (q.1 - p.1) * k);
        }
        let mut i = 1;
        while i < path.len() - 1 && at[i] < s {
            i += 1;
        }
        let span = at[i] - at[i - 1];
        let k = if span > 0.0 { js_min(1.0, (s - at[i - 1]) / span) } else { 1.0 };
        (path[i - 1].0 + (path[i].0 - path[i - 1].0) * k, path[i - 1].1 + (path[i].1 - path[i - 1].1) * k)
    };
    let mut out: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut front = head;
    for c in 0..cars.max(1) {
        let back = front - length_of(c);
        let mut line: Vec<LatLon> = vec![point(back)];
        line.extend(path.iter().zip(&at).filter(|(_, d)| **d > back && **d < front).map(|(p, _)| *p));
        line.push(point(front));
        out.push(line.into_iter().map(|(lat, lon)| [lon, lat]).collect());
        front = back - gap;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;

    // east along 60°N: a degree of longitude is about 55.66 km
    const DEG_PER_M: f64 = 1.0 / 55_660.0;

    fn at(m: f64) -> (f64, f64) {
        (60.0, 18.0 + m * DEG_PER_M)
    }

    fn lon_m(lon: f64) -> f64 {
        (lon - 18.0) / DEG_PER_M
    }

    /// At 0 m when frames start, 20 m/s east.
    fn frames() -> Vec<Frame> {
        let (a, b) = (at(0.0), at(200.0));
        vec![(a.0, a.1, 0), (b.0, b.1, 10_000)]
    }

    /// The track behind it, given back 200 m.
    fn tail() -> Vec<(f64, f64)> {
        vec![at(0.0), at(-100.0), at(-200.0)]
    }

    #[test]
    fn its_cars_back_from_where_it_is_drawn_now_front_first() {
        let cars = car_lines(&frames(), &tail(), 3, 5_000);
        assert_eq!(cars.len(), 3);
        // the head is 100 m on; the first car from there back, the last ending three cars (less a gap) back
        assert_close!(lon_m(cars[0][cars[0].len() - 1][0]), 100.0, 0);
        assert_close!(lon_m(cars[2][0][0]), 100.0 - 3.0 * CAR_M + GAP_M, 0);
    }

    #[test]
    fn standing_where_its_frames_start_its_cars_lie_along_the_track_behind() {
        let cars = car_lines(&frames(), &tail(), 4, 0);
        assert_close!(lon_m(cars[0][cars[0].len() - 1][0]), 0.0, 0);
        assert_close!(lon_m(cars[3][0][0]), -4.0 * CAR_M + GAP_M, 0);
    }

    #[test]
    fn without_the_track_behind_straight_on_back() {
        let cars = car_lines(&frames(), &[], 2, 0);
        assert_close!(lon_m(cars[1][0][0]), -2.0 * CAR_M + GAP_M, 0);
    }

    #[test]
    fn cars_of_their_own_lengths_the_gap_between_each() {
        // a 15 m locomotive and a 25 m coach, standing at 0 m
        let cars = car_lines_with(&frames(), &tail(), 2, 0, |k| if k == 0 { 15.0 } else { 25.0 }, GAP_M);
        let (loco, coach) = (&cars[0], &cars[1]);
        assert_close!(lon_m(loco[0][0]), -15.0, 0);
        assert_close!(lon_m(coach[coach.len() - 1][0]), -15.0 - GAP_M, 0);
        assert_close!(lon_m(coach[0][0]), -40.0 - GAP_M, 0);
    }

    #[test]
    fn models_cars_end_to_end_their_lengths_take_in_the_gap() {
        let cars = car_lines_with(&frames(), &tail(), 2, 0, |_| 25.0, 0.0);
        let (first, second) = (&cars[0], &cars[1]);
        assert_close!(lon_m(second[second.len() - 1][0]), lon_m(first[0][0]), 1);
    }

    #[test]
    fn no_frames_no_cars() {
        assert!(car_lines(&[], &tail(), 3, 0).is_empty());
    }

    #[test]
    fn at_least_one_car_and_interior_points_kept() {
        // no cars asked for: still one. 10 m north, then 10 m east: a car over the bend keeps the
        // path's points, and its back runs on straight past the start
        let north = 60.0 + 10.0 / 111_320.0;
        let east = 18.0 + 10.0 * DEG_PER_M;
        let bend = vec![(60.0, 18.0, 0), (north, 18.0, 1_000), (north, east, 2_000)];
        let cars = car_lines(&bend, &[], 0, 2_000);
        assert_eq!(cars.len(), 1);
        assert_eq!(cars[0][1..], [[18.0, 60.0], [18.0, north], [east, north]]);
        assert_eq!(cars[0][0][0], 18.0);
        assert_close!((cars[0][0][1] - 60.0) * 111_320.0, -(CAR_M - GAP_M - 20.0), 3);
    }
}
