//! The aircraft fixtures of stage 4 (plan, "Stage 4"): a deterministic synthetic snapshot of 236
//! flights, and a `--fixture` folder holding it.
//!
//! - 200 flights worldwide, between 55° S and 70° N, from 1 000 to 41 000 ft;
//! - 30 around Stockholm (within about 0.6° of latitude and 1.2° of longitude of the city), from
//!   1 500 to 38 000 ft;
//! - 6 on the ground at Arlanda, parked or taxiing on its aprons.
//!
//! Types and icon classes cycle through [`TYPES`]: published models found by ICAO type, by icon
//! class only, and kinds with no published model (balloon, ground vehicle, Concorde, an unknown
//! type), so every path of the scene builder is taken. Every tenth flight has no callsign, flight
//! number or registration (no label). Nothing is random: the same call gives the same flights.
//!
//! Shared by the app's integration tests (`common::fixtures`) and the aircraft scene builder's unit
//! tests (`scene_builder/aircraft.rs` includes this file by path), so it uses nothing but the
//! workspace crates.
#![allow(dead_code)]

use std::path::Path;

use rustradar_core::model::{LiveFlight, LiveSnapshot};
use rustradar_core::num::js_round;

/// `(ICAO type, Flightradar24 icon class)`, cycled through by the flights.
pub const TYPES: [(&str, &str); 16] = [
    ("A20N", "A320"),
    ("B738", "B738"),
    ("A359", "A330"),
    ("B77W", "B777"),
    ("E190", "E190"),
    ("CRJ9", "CRJ9"),
    ("DH8D", "Q300"),
    ("C172", "C206"),
    // by icon class only: the helicopter
    ("EC35", "EC"),
    // no published model: built-in balloon, ground vehicle, narrow-body (Concorde's 1.5)
    ("BALL", "BALL"),
    ("", "GRND"),
    ("CONC", "CONC"),
    ("A388", "A380"),
    ("B744", "B747"),
    // unknown type and icon: the built-in narrow-body
    ("ZZZZ", ""),
    // a published model without gear
    ("GLID", "GLID"),
];

/// Flights worldwide, around Stockholm, on the ground at Arlanda.
pub const WORLD: usize = 200;
pub const STOCKHOLM: usize = 30;
pub const ARLANDA: usize = 6;

/// The first flight's id; the others follow in order.
pub const FIRST_ID: u32 = 1000;

/// A deterministic number in [0, 1) for `(n, salt)` (an integer hash, no state).
fn unit(n: u32, salt: u32) -> f64 {
    let mut x = n.wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    f64::from(x) / 4_294_967_296.0
}

/// Degrees to 4 decimals: as an `f32` its shortest decimal (what the wire JSON carries, and what
/// the webview reads) is that same decimal.
fn deg4(x: f64) -> f32 {
    (js_round(x * 1e4) / 1e4) as f32
}

/// One flight: the `n`-th of the snapshot, its type from [`TYPES`].
fn flight(n: u32, [lat, lon]: [f64; 2], alt: i32, speed: i32, track: i32, on_ground: bool, now_ms: f64) -> LiveFlight {
    let (typecode, icon) = TYPES[n as usize % TYPES.len()];
    let named = n % 10 != 9;
    let name = |prefix: &str| if named { format!("{prefix}{n:03}") } else { String::new() };
    LiveFlight {
        id: FIRST_ID + n,
        lat: deg4(lat),
        lon: deg4(lon),
        track,
        alt,
        speed,
        on_ground,
        // reported two seconds before the snapshot
        timestamp_ms: (now_ms - 2000.0) as u64,
        callsign: name("SYN"),
        flight: name("SY"),
        reg: name("SE-"),
        typecode: typecode.to_owned(),
        origin: String::new(),
        destination: String::new(),
        icon: icon.to_owned(),
        positions: Vec::new(),
        vspeed: None,
    }
}

/// The synthetic snapshot as the feed answers it at `now_ms` (its server time).
pub fn synthetic_snapshot(now_ms: f64) -> LiveSnapshot {
    let mut flights = Vec::with_capacity(WORLD + STOCKHOLM + ARLANDA);
    let mut n = 0u32;
    for _ in 0..WORLD {
        let lat = -55.0 + 125.0 * unit(n, 1);
        let lon = -180.0 + 360.0 * unit(n, 2);
        let alt = 1000 + (40_000.0 * unit(n, 3)) as i32;
        let speed = 150 + (350.0 * unit(n, 4)) as i32;
        let track = (360.0 * unit(n, 5)) as i32;
        flights.push(flight(n, [lat, lon], alt, speed, track, false, now_ms));
        n += 1;
    }
    for _ in 0..STOCKHOLM {
        let lat = 59.33 + 1.2 * (unit(n, 1) - 0.5);
        let lon = 18.07 + 2.4 * (unit(n, 2) - 0.5);
        let alt = 1500 + (36_500.0 * unit(n, 3)) as i32;
        let speed = 160 + (300.0 * unit(n, 4)) as i32;
        let track = (360.0 * unit(n, 5)) as i32;
        flights.push(flight(n, [lat, lon], alt, speed, track, false, now_ms));
        n += 1;
    }
    // Arlanda's aprons, around the terminals between the runways; parked or taxiing along the
    // runways' headings (01/19 and 08/26)
    for i in 0..ARLANDA as u32 {
        let lat = 59.6495 + 0.006 * unit(n, 1);
        let lon = 17.912 + 0.016 * unit(n, 2);
        let speed = if i % 2 == 0 { 0 } else { 8 + (6.0 * unit(n, 4)) as i32 };
        let track = [8, 188, 79, 259, 8, 188][i as usize];
        flights.push(flight(n, [lat, lon], 0, speed, track, true, now_ms));
        n += 1;
    }
    LiveSnapshot {
        flights,
        server_time_ms: now_ms as u64,
        requests: 1,
        errors: Vec::new(),
        elapsed_ms: 120,
        authenticated: false,
    }
}

/// Writes `snapshot` into `dir` as the `--fixture` folder's `live_flights.json` (the wire JSON of
/// `live_flights`), making the folder if needed.
pub fn write_fixture(dir: &Path, snapshot: &LiveSnapshot) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_vec(snapshot).map_err(std::io::Error::other)?;
    std::fs::write(dir.join("live_flights.json"), json)
}
