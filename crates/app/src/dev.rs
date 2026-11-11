//! Developer-only helpers, behind command-line flags rather than in the app's own flow.
//!
//! [`demo_trains`]: the made-up trains of `--demo` (today's `#demo` address hash, a port of
//! `src/lib/demo-trains.ts`; doc 02 §13.2): six trains running straight through the middle of the
//! view, one per car shape, to see how trains are drawn without a Trafikverket key. The train state
//! (`state::trains`) shows them instead of the backend's trains when the app runs with `--demo`.

use rustradar_core::model::{Train, TrainKind, TrainShape, TrainSource};

/// Metres in a degree of latitude, as `demo-trains.ts` takes it (`M_PER_DEG`).
pub const M_PER_DEG: f64 = 111_320.0;

/// How far the frames of a demo train reach, seconds; one frame every [`DEMO_FRAME_S`].
pub const DEMO_RUN_S: i64 = 120;
/// Seconds between two frames of a demo train.
pub const DEMO_FRAME_S: i64 = 2;
/// How far back each demo train's tail reaches, metres, nearest first.
pub const DEMO_TAIL_M: [f64; 5] = [0.0, 100.0, 200.0, 300.0, 400.0];

/// One demo train's run (`runs` in `demoTrains`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DemoRun {
    /// Degrees clockwise from north.
    pub heading: f64,
    pub cars: u32,
    pub kind: TrainKind,
    pub operator: &'static str,
    /// Metres to the right of the view centre (at `heading + 90`), negative to the left.
    pub offset: f64,
    /// Metres per second.
    pub speed: f64,
    pub shape: TrainShape,
}

/// The six runs, one of each shape (`demo-trains.ts`, in its order).
pub const DEMO_RUNS: [DemoRun; 6] = [
    DemoRun { heading: 70.0, cars: 3, kind: TrainKind::Regional, operator: "Skånetrafiken", offset: 0.0, speed: 14.0, shape: TrainShape::Regional },
    DemoRun { heading: 250.0, cars: 6, kind: TrainKind::HighSpeed, operator: "SJ", offset: 40.0, speed: 22.0, shape: TrainShape::HighSpeed },
    DemoRun { heading: 160.0, cars: 12, kind: TrainKind::Intercity, operator: "Öresundståg", offset: -60.0, speed: 0.0, shape: TrainShape::Regional },
    DemoRun { heading: 340.0, cars: 4, kind: TrainKind::Regional, operator: "SJ", offset: 90.0, speed: 0.0, shape: TrainShape::DoubleDeck },
    DemoRun { heading: 20.0, cars: 8, kind: TrainKind::Intercity, operator: "Snälltåget", offset: -110.0, speed: 0.0, shape: TrainShape::Loco },
    DemoRun { heading: 110.0, cars: 1, kind: TrainKind::Regional, operator: "Inlandsbanan", offset: 140.0, speed: 0.0, shape: TrainShape::Railcar },
];

/// `(lat, lon)` `m` metres from `at` going `heading` degrees (`ahead`): a flat step with the
/// longitude scaled by the cosine of the start's latitude.
pub fn ahead(at: (f64, f64), heading: f64, m: f64) -> (f64, f64) {
    let h = heading * std::f64::consts::PI / 180.0;
    let k = (at.0 * std::f64::consts::PI / 180.0).cos();
    (at.0 + h.cos() * m / M_PER_DEG, at.1 + h.sin() * m / (M_PER_DEG * k))
}

/// The demo trains through `(latitude, longitude)` at `now_ms` (Unix ms, `Date.now()`;
/// `demoTrains(centre, nowMs)`): each starts `offset` metres to the side of the centre and 40 m
/// back, has frames every 2 s for 120 s at its speed (the moving ones run 14 and 22 m/s, the rest
/// stand), a tail at 0 to 400 m behind its start, the id `trafikverket:demo:{n}` and the name
/// `Demo {operator} {100 + n}`.
pub fn demo_trains(latitude: f64, longitude: f64, now_ms: i64) -> Vec<Train> {
    DEMO_RUNS
        .iter()
        .enumerate()
        .map(|(n, r)| {
            let start = ahead(ahead((latitude, longitude), r.heading + 90.0, r.offset), r.heading, -40.0);
            let frames = (0..=DEMO_RUN_S)
                .step_by(DEMO_FRAME_S as usize)
                .map(|s| {
                    let (lat, lon) = ahead(start, r.heading, r.speed * s as f64);
                    (lat, lon, now_ms + s * 1000)
                })
                .collect();
            let tail = DEMO_TAIL_M.iter().map(|&m| ahead(start, r.heading, -m)).collect();
            Train {
                id: format!("trafikverket:demo:{n}"),
                source: TrainSource::Trafikverket,
                name: format!("Demo {} {}", r.operator, 100 + n),
                kind: r.kind,
                lat: start.0,
                lon: start.1,
                heading: Some(r.heading as f32),
                speed_kmh: Some((r.speed * 3.6) as f32),
                destination: None,
                operator: Some(r.operator.to_owned()),
                delay_minutes: Some(0),
                position_unix: now_ms.div_euclid(1000),
                frames,
                reports: Vec::new(),
                predicted: Vec::new(),
                cars: Some(r.cars),
                tail,
                structures: Vec::new(),
                ended_unix: None,
                shape: Some(r.shape),
                model: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use rustradar_core::trains::{heading_at, position_at, train_color_of, train_running};

    use super::*;

    /// The local clock of the tests: 2026-06-21 10:00 UTC, the reference pictures' morning.
    const NOW: i64 = 1_782_036_000_123;

    /// Metres between two points the way `ahead` steps (flat, longitude scaled at `a`'s latitude).
    fn flat_m(a: (f64, f64), b: (f64, f64)) -> f64 {
        let k = (a.0 * std::f64::consts::PI / 180.0).cos();
        ((b.0 - a.0) * M_PER_DEG).hypot((b.1 - a.1) * M_PER_DEG * k)
    }

    #[test]
    fn ahead_steps_north_and_east_as_demo_trains_ts() {
        // north: latitude only
        let (lat, lon) = ahead((59.0, 18.0), 0.0, 111_320.0);
        assert!((lat - 60.0).abs() < 1e-12 && (lon - 18.0).abs() < 1e-12, "{lat} {lon}");
        // east at 60° N: a degree of longitude is half as long
        let (lat, lon) = ahead((60.0, 18.0), 90.0, 55_660.0);
        assert!((lat - 60.0).abs() < 1e-12, "{lat}");
        assert!((lon - 19.0).abs() < 1e-9, "{lon}");
        // back is the opposite way
        let (lat, _) = ahead((59.0, 18.0), 0.0, -1113.2);
        assert!((lat - 58.99).abs() < 1e-12, "{lat}");
        // the formula as the TypeScript writes it, term by term
        let at = (59.33, 18.07);
        let h = 250.0 * std::f64::consts::PI / 180.0;
        let k = (59.33 * std::f64::consts::PI / 180.0).cos();
        assert_eq!(ahead(at, 250.0, 40.0), (59.33 + h.cos() * 40.0 / M_PER_DEG, 18.07 + h.sin() * 40.0 / (M_PER_DEG * k)));
    }

    #[test]
    fn six_trains_one_of_each_shape_through_the_view_centre() {
        let trains = demo_trains(59.33, 18.07, NOW);
        assert_eq!(trains.len(), 6);
        let names: Vec<&str> = trains.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            ["Demo Skånetrafiken 100", "Demo SJ 101", "Demo Öresundståg 102", "Demo SJ 103", "Demo Snälltåget 104", "Demo Inlandsbanan 105"]
        );
        let ids: Vec<&str> = trains.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["trafikverket:demo:0", "trafikverket:demo:1", "trafikverket:demo:2", "trafikverket:demo:3", "trafikverket:demo:4", "trafikverket:demo:5"]);
        let shapes: Vec<TrainShape> = trains.iter().filter_map(|t| t.shape).collect();
        assert_eq!(
            shapes,
            [TrainShape::Regional, TrainShape::HighSpeed, TrainShape::Regional, TrainShape::DoubleDeck, TrainShape::Loco, TrainShape::Railcar]
        );
        let cars: Vec<Option<u32>> = trains.iter().map(|t| t.cars).collect();
        assert_eq!(cars, [Some(3), Some(6), Some(12), Some(4), Some(8), Some(1)]);
        for (t, r) in trains.iter().zip(DEMO_RUNS) {
            assert_eq!(t.source, TrainSource::Trafikverket);
            assert_eq!(t.kind, r.kind);
            assert_eq!(t.heading, Some(r.heading as f32));
            assert_eq!(t.speed_kmh, Some((r.speed * 3.6) as f32));
            assert_eq!((t.destination.as_deref(), t.operator.as_deref(), t.delay_minutes), (None, Some(r.operator), Some(0)));
            assert_eq!(t.position_unix, 1_782_036_000);
            assert!(t.ended_unix.is_none() && t.model.is_none() && t.structures.is_empty() && t.reports.is_empty());
            assert!(train_running(t, NOW + 3_600_000), "a demo train never ends its run");
            // `offset` to the side of the centre, then 40 m back along its heading
            let side = ahead((59.33, 18.07), r.heading + 90.0, r.offset);
            assert!((flat_m((59.33, 18.07), side) - r.offset.abs()).abs() < 0.01, "{}", t.name);
            assert_eq!((t.lat, t.lon), ahead(side, r.heading, -40.0));
        }
        // the operators' colours (SJ white, Skånetrafiken purple), so the cars show who runs them
        assert_eq!(train_color_of(&trains[1]), [232, 232, 236]);
        assert_eq!(train_color_of(&trains[0]), [196, 92, 222]);
    }

    #[test]
    fn frames_every_two_seconds_for_two_minutes_and_a_tail_behind() {
        let trains = demo_trains(59.33, 18.07, NOW);
        for (t, r) in trains.iter().zip(DEMO_RUNS) {
            assert_eq!(t.frames.len(), 61, "{}", t.name);
            for (i, &(lat, lon, at)) in t.frames.iter().enumerate() {
                let s = 2 * i as i64;
                assert_eq!(at, NOW + s * 1000);
                assert_eq!((lat, lon), ahead((t.lat, t.lon), r.heading, r.speed * s as f64));
            }
            // the first frame is where it is now; the moving ones go `speed` m/s
            assert_eq!((t.frames[0].0, t.frames[0].1), (t.lat, t.lon));
            let last = t.frames[60];
            assert!((flat_m((t.lat, t.lon), (last.0, last.1)) - r.speed * 120.0).abs() < 0.05, "{}", t.name);
            // the tail: its start, then 100 m steps back
            assert_eq!(t.tail.len(), 5);
            assert_eq!(t.tail[0], (t.lat, t.lon));
            for (p, m) in t.tail.iter().zip(DEMO_TAIL_M) {
                assert_eq!(*p, ahead((t.lat, t.lon), r.heading, -m));
                assert!((flat_m((t.lat, t.lon), *p) - m).abs() < 0.01);
            }
        }
        // the two moving trains are drawn along their heading; the standing ones stay put
        let mid = NOW + 60_000;
        let moved = |t: &Train| {
            let p = position_at(t, mid);
            flat_m((t.lat, t.lon), (p.lat, p.lon))
        };
        assert!((moved(&trains[0]) - 14.0 * 60.0).abs() < 0.5, "{}", moved(&trains[0]));
        assert!((moved(&trains[1]) - 22.0 * 60.0).abs() < 0.5, "{}", moved(&trains[1]));
        for t in &trains[2..] {
            assert!(moved(t) < 1e-6, "{}", t.name);
        }
        let heading = heading_at(&trains[0], mid).expect("a heading");
        assert!((heading - 70.0).abs() < 0.01, "{heading}");
    }

    #[test]
    fn the_whole_seconds_of_the_report_round_down() {
        let trains = demo_trains(0.0, 0.0, 1999);
        assert_eq!(trains[0].position_unix, 1);
        assert_eq!(trains[0].frames[1].2, 3999);
    }
}
