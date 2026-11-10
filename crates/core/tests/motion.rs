//! The golden poses of S1.17 (`tests/data/golden/motion-*.json`, written by
//! `scripts/port/golden.ts` from the TypeScript `Motion`) replayed through the public API of
//! `rustradar_core::motion::aircraft::Motion`, as the script describes: reports, measured data
//! and taxiway networks applied at their times, then every aircraft posed each frame. Every
//! column matches, gear included: positions to 1e-9 degrees, the drawn time to 1e-3 ms (its
//! values are ~1.8e12 ms, whose doubles are 2.4e-4 ms apart) and the rest to 1e-6.
//!
//! The ported `motion.test.ts` cases are unit tests beside the code they exercise
//! (`crates/core/src/motion/aircraft/*.rs`), since they share helpers there.

use std::collections::HashMap;
use std::sync::Arc;

use rustradar_core::model::{Airport, LiveFlight, TaxiLine};
use rustradar_core::motion::aircraft::{Measured, Motion};
use serde_json::Value;

const CASES: [(&str, &str); 12] = [
    ("motion-aee604", include_str!("data/golden/motion-aee604.json")),
    ("motion-baw9173", include_str!("data/golden/motion-baw9173.json")),
    ("motion-dal1424", include_str!("data/golden/motion-dal1424.json")),
    ("motion-dal2049", include_str!("data/golden/motion-dal2049.json")),
    ("motion-klc20d", include_str!("data/golden/motion-klc20d.json")),
    ("motion-klm76f", include_str!("data/golden/motion-klm76f.json")),
    ("motion-skw446w", include_str!("data/golden/motion-skw446w.json")),
    ("motion-swa1571", include_str!("data/golden/motion-swa1571.json")),
    ("motion-swa2713", include_str!("data/golden/motion-swa2713.json")),
    ("motion-synthetic-arrival-departure", include_str!("data/golden/motion-synthetic-arrival-departure.json")),
    ("motion-synthetic-taxi", include_str!("data/golden/motion-synthetic-taxi.json")),
    ("motion-synthetic-turn-measured", include_str!("data/golden/motion-synthetic-turn-measured.json")),
];

/// A `Measured` of the TypeScript (camelCase, absent values left out); `null` is none. Unknown
/// keys fail, so that nothing the TypeScript was given is dropped here.
fn measured(v: &Value) -> Option<Measured> {
    if v.is_null() {
        return None;
    }
    let mut m = Measured::default();
    for (key, value) in v.as_object().expect("a measured state is an object") {
        let number = || Some(value.as_f64().unwrap_or_else(|| panic!("{key}: {value} is not a number")));
        match key.as_str() {
            "positionMs" => m.position_ms = number(),
            "lat" => m.lat = number(),
            "lon" => m.lon = number(),
            "mlat" => m.mlat = value.as_bool().expect("mlat"),
            "seenMs" => m.seen_ms = number().unwrap(),
            "onGround" => m.on_ground = value.as_bool().expect("onGround"),
            "altBaro" => m.alt_baro = number(),
            "navQnh" => m.nav_qnh = number(),
            "gs" => m.gs = number(),
            "track" => m.track = number(),
            "trueHeading" => m.true_heading = number(),
            "magHeading" => m.mag_heading = number(),
            "roll" => m.roll = number(),
            "trackRate" => m.track_rate = number(),
            "baroRate" => m.baro_rate = number(),
            "geomRate" => m.geom_rate = number(),
            "tas" => m.tas = number(),
            "navAltitudeMcp" => m.nav_altitude_mcp = number(),
            "navAltitudeFms" => m.nav_altitude_fms = number(),
            "navHeading" => m.nav_heading = number(),
            "navModes" => m.nav_modes = Some(serde_json::from_value(value.clone()).expect("navModes")),
            _ => panic!("unknown measured field {key}"),
        }
    }
    Some(m)
}

/// The signed difference of two angles, degrees.
fn signed(deg: f64) -> f64 {
    ((deg + 540.0) % 360.0) - 180.0
}

#[test]
fn the_golden_poses_match_the_typescript() {
    for (name, text) in CASES {
        let golden: Value = serde_json::from_str(text).unwrap();
        let about = golden["about"].as_str().unwrap();
        let airports: Vec<Arc<Airport>> =
            serde_json::from_value::<Vec<Airport>>(golden["airports"].clone()).unwrap().into_iter().map(Arc::new).collect();
        let ids: Vec<u32> = serde_json::from_value(golden["ids"].clone()).unwrap();
        let phases: Vec<String> = serde_json::from_value(golden["phases"].clone()).unwrap();
        let columns: Vec<String> = serde_json::from_value(golden["columns"].clone()).unwrap();
        assert_eq!(
            columns,
            ["frame", "id", "lon", "lat", "altFt", "heading", "track", "pitch", "bank", "gear", "phase", "drawnMs"],
            "{name}"
        );
        let events = golden["events"].as_array().unwrap();
        let poses = golden["poses"].as_array().unwrap();
        let step = golden["stepMs"].as_f64().unwrap();
        let start = golden["startMs"].as_f64().unwrap();
        let frames = golden["frames"].as_u64().unwrap();

        let mut m = Motion::new();
        m.set_airports(&airports);
        let mut current: HashMap<u32, LiveFlight> = HashMap::new();
        let mut next = 0;
        let mut row = 0;
        let mut kinds: HashMap<String, usize> = HashMap::new();
        for frame in 0..frames {
            let now = start + frame as f64 * step;
            while next < events.len() && events[next]["atMs"].as_f64().unwrap() <= now {
                let e = &events[next];
                let kind = e["type"].as_str().unwrap();
                *kinds.entry(kind.to_string()).or_default() += 1;
                match kind {
                    "report" => {
                        let f: LiveFlight = serde_json::from_value(e["flight"].clone()).unwrap();
                        current.insert(f.id, f);
                    }
                    "measure" => {
                        let id = e["id"].as_u64().unwrap() as u32;
                        m.measure(id, measured(&e["measured"]).as_ref(), now);
                    }
                    "taxiways" => {
                        let lines: Option<Vec<TaxiLine>> = serde_json::from_value(e["lines"].clone()).unwrap();
                        m.set_taxiways(e["icao"].as_str().unwrap(), lines.as_deref());
                    }
                    other => panic!("{name}: unknown event {other}"),
                }
                next += 1;
            }
            for id in &ids {
                let Some(f) = current.get(id) else {
                    continue;
                };
                let p = m.pose(f, now);
                let drawn = m.drawn_time(*id, now);
                let want: Vec<f64> = poses[row].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
                row += 1;
                let at = format!("{name} frame {frame} id {id}");
                assert_eq!(want[0], frame as f64, "{at}");
                assert_eq!(want[1], *id as f64, "{at}");
                assert!((p.lon - want[2]).abs() <= 1e-9, "{at}: lon {} against {}", p.lon, want[2]);
                assert!((p.lat - want[3]).abs() <= 1e-9, "{at}: lat {} against {}", p.lat, want[3]);
                assert!((p.alt_ft - want[4]).abs() <= 1e-6, "{at}: altFt {} against {}", p.alt_ft, want[4]);
                assert!(signed(p.heading - want[5]).abs() <= 1e-6, "{at}: heading {} against {}", p.heading, want[5]);
                assert!(signed(p.track - want[6]).abs() <= 1e-6, "{at}: track {} against {}", p.track, want[6]);
                assert!((p.pitch - want[7]).abs() <= 1e-6, "{at}: pitch {} against {}", p.pitch, want[7]);
                assert!((p.bank - want[8]).abs() <= 1e-6, "{at}: bank {} against {}", p.bank, want[8]);
                assert!((p.gear - want[9]).abs() <= 1e-6, "{at}: gear {} against {}", p.gear, want[9]);
                assert_eq!(p.phase.as_str(), phases[want[10] as usize], "{at}");
                assert!((drawn - want[11]).abs() <= 1e-3, "{at}: drawn {drawn} against {}", want[11]);
            }
        }
        assert_eq!(next, events.len(), "{name}: events after the last frame");
        assert_eq!(row, poses.len(), "{name}: {about}");
        // the synthetic cases exercise what the recorded flights do not
        match name {
            "motion-synthetic-taxi" => assert_eq!(kinds.get("taxiways"), Some(&3), "{name}"),
            "motion-synthetic-turn-measured" => assert!(kinds.get("measure").is_some_and(|&n| n > 80), "{name}"),
            _ => {}
        }
    }
}

/// The gear in the golden poses moves: down on the ground, up in the climb, eased in between.
#[test]
fn the_golden_gear_goes_up_and_down() {
    let golden: Value = serde_json::from_str(CASES[9].1).unwrap();
    let poses = golden["poses"].as_array().unwrap();
    let gears: Vec<f64> = poses.iter().map(|r| r[9].as_f64().unwrap()).collect();
    assert!(gears.contains(&0.0) && gears.contains(&1.0));
    assert!(gears.iter().any(|&g| g > 0.0 && g < 1.0));
}
