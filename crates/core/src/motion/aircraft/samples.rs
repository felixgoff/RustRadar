//! A track's horizontal samples (`motion.ts` `:2380-2641`, doc 05 §10.6): a report's own points
//! (its buffer and itself), merging them into the track with jump detection, measured samples
//! joining by time and source rank, the window and the local east/north frame; and the two
//! estimators read off a report's own samples (`bufferTurnRate`, `bufferAcceleration`,
//! `:3307-3369`, doc 05 §10.5).
//!
//! The TypeScript's module-level `newT`/`newLat`/`newLon`, `outT`/… and `reportFromMs` live in
//! [`Scratch`].

use super::consts::*;
use super::fit::hermite;
use super::track::Track;
use super::{clamp, distance_km, wrap_lon, wrap180};
use crate::trains::js_min;

/// Scratch for merging: a report's own samples, oldest first, and the merged result (reports are
/// merged for every aircraft in every snapshot).
#[derive(Debug, Default)]
pub struct Scratch {
    pub new_t: Vec<f64>,
    pub new_lat: Vec<f64>,
    pub new_lon: Vec<f64>,
    pub out_t: Vec<f64>,
    pub out_lat: Vec<f64>,
    pub out_lon: Vec<f64>,
    pub out_src: Vec<u8>,
    /// Time of the oldest sample the last report merged brought: its buffer's oldest point, or
    /// its own time (`reportFromMs`).
    pub report_from_ms: f64,
}

/// A report's own samples into `new_t`/`new_lat`/`new_lon`, oldest first: its position buffer,
/// which is the aircraft's recent history (each point `dMs` before the report, at
/// `lat/lon - d·1e-5`; Flightradar24 sends the newest first), then the report point itself, the
/// newest. Of two points closer together than MIN_SAMPLE_GAP_MS the later one in the buffer is
/// dropped.
pub fn report_points(sc: &mut Scratch, positions: &[[i32; 3]], t: f64, lat0: f64, lon0: f64) {
    sc.new_t.clear();
    sc.new_lat.clear();
    sc.new_lon.clear();
    for &[d_lat, d_lon, d_ms] in positions {
        let (d_lat, d_lon, d_ms) = (d_lat as f64, d_lon as f64, d_ms as f64);
        let lat = lat0 - d_lat * BUFFER_SCALE;
        let lon = lon0 - d_lon * BUFFER_SCALE;
        if !(MIN_SAMPLE_GAP_MS..=SAMPLE_WINDOW_MS).contains(&d_ms) || !(lat.abs() <= 90.0) || !lon.is_finite() {
            continue;
        }
        let time = t - d_ms;
        let new_t = &sc.new_t;
        let mut j = new_t.len();
        while j > 0 && new_t[j - 1] > time {
            j -= 1;
        }
        if (j > 0 && time - new_t[j - 1] < MIN_SAMPLE_GAP_MS) || (j < new_t.len() && new_t[j] - time < MIN_SAMPLE_GAP_MS) {
            continue;
        }
        sc.new_t.insert(j, time);
        sc.new_lat.insert(j, lat);
        sc.new_lon.insert(j, wrap_lon(lon));
    }
    sc.new_t.push(t);
    sc.new_lat.push(lat0);
    sc.new_lon.push(lon0);
    sc.report_from_ms = sc.new_t[0];
}

/// Whether one of the report's points (in `new_t`/`new_lat`/`new_lon`) is well off the path the
/// held samples smoothed to, inside them or just past them: the old samples were wrong (or this
/// is a jump in the data), and would poison the spline.
pub fn off_path(tr: &Track, sc: &Scratch) -> bool {
    let n = tr.n;
    if n < 2 || tr.fx.len() != n || tr.ts.len() != n {
        return false;
    }
    let ts = &tr.ts;
    let last = n - 1;
    for j in 0..sc.new_t.len() {
        let t = sc.new_t[j];
        if t < ts[0] || t > ts[last] + JUMP_LOOKAHEAD_MS {
            continue;
        }
        let (x, y) = if t <= ts[last] {
            let h = hermite(tr, t);
            (h.x, h.y)
        } else {
            // just past them: straight on is good to well inside JUMP_M over a few seconds
            let dt = (t - ts[last]) / 1000.0;
            (tr.fx[last] + tr.mx[last] * dt, tr.fy[last] + tr.my[last] * dt)
        };
        let lat = tr.ref_lat + y / M_PER_DEG;
        let lon = tr.ref_lon + x / (M_PER_DEG * (((tr.ref_lat + lat) / 2.0) * DEG).cos().max(1e-3));
        if distance_km(lat, lon, sc.new_lat[j], sc.new_lon[j]) * 1000.0 > JUMP_M {
            return true;
        }
    }
    false
}

/// Merges a Flightradar24 report at `t` (position `lat0`/`lon0`, buffer `positions`) into the
/// track's samples: the report point, the newest, and its buffer, the ~10 s before it. The buffer
/// overlaps what earlier reports brought (it holds their positions, to a metre or so), so samples
/// within COINCIDENT_MS of each other are the same moment: a held sample of a better source
/// (ADS-B) is kept, otherwise the newer arrival replaces it. Buffer points older than every held
/// sample add nothing the track's record needs and are left out, and only the last
/// SAMPLE_WINDOW_MS is kept.
///
/// A report that jumps away from the held samples (implausibly far from the nearest, or a point
/// of it well off their smoothed path) starts them over, unless it is `older` than the measured
/// state (which then wins): it is dropped, and the result is `None`. Otherwise returns whether
/// the newest sample is the report point itself, whose velocity is then the reported one.
/// [`Scratch::report_from_ms`] is set either way.
pub fn merge_report(
    tr: &mut Track,
    sc: &mut Scratch,
    positions: &[[i32; 3]],
    t: f64,
    lat0: f64,
    lon0: f64,
    older: bool,
) -> Option<bool> {
    report_points(sc, positions, t, lat0, lon0);
    if let Some(near) = nearest_sample(tr, t) {
        let far = distance_km(tr.lats[near], tr.lons[near], lat0, lon0)
            > js_min(plausible_km(tr, (t - tr.ts[near]) / 1000.0), SNAP_DISTANCE_M / 1000.0);
        if far || (!tr.mixed && off_path(tr, sc)) {
            if older {
                return None;
            }
            clear_samples(tr);
        }
    }
    let mixed = tr.mixed;
    let held = tr.ts.len();
    // which of the report's points go in: none older than the record, none that a better source has
    let from = if held > 0 { tr.ts[0] } else { f64::NEG_INFINITY };
    let last = sc.new_t.len() - 1;
    let mut m = 0;
    for j in 0..=last {
        let time = sc.new_t[j];
        if time < from && j < last {
            continue;
        }
        let outranked = mixed && (0..held).any(|i| (tr.ts[i] - time).abs() < COINCIDENT_MS && tr.src[i] > SRC_FR24);
        if outranked {
            continue;
        }
        sc.new_t[m] = time;
        sc.new_lat[m] = sc.new_lat[j];
        sc.new_lon[m] = sc.new_lon[j];
        m += 1;
    }
    // both in time order, a held sample giving way to a new point of the same moment
    sc.out_t.clear();
    sc.out_lat.clear();
    sc.out_lon.clear();
    sc.out_src.clear();
    let (mut i, mut j) = (0, 0);
    while i < held || j < m {
        if j >= m || (i < held && tr.ts[i] < sc.new_t[j]) {
            let ti = tr.ts[i];
            if !((j > 0 && ti - sc.new_t[j - 1] < COINCIDENT_MS) || (j < m && sc.new_t[j] - ti < COINCIDENT_MS)) {
                sc.out_t.push(ti);
                sc.out_lat.push(tr.lats[i]);
                sc.out_lon.push(tr.lons[i]);
                if mixed {
                    sc.out_src.push(tr.src[i]);
                }
            }
            i += 1;
        } else {
            sc.out_t.push(sc.new_t[j]);
            sc.out_lat.push(sc.new_lat[j]);
            sc.out_lon.push(sc.new_lon[j]);
            if mixed {
                sc.out_src.push(SRC_FR24);
            }
            j += 1;
        }
    }
    tr.ts.clone_from(&sc.out_t);
    tr.lats.clone_from(&sc.out_lat);
    tr.lons.clone_from(&sc.out_lon);
    if mixed {
        tr.src.clone_from(&sc.out_src);
    }
    finish_samples(tr);
    Some(tr.n > 0 && tr.ts[tr.n - 1] == t)
}

/// Distance (km) a sample may plausibly be from another `gap_s` seconds away, at the track's speed.
pub fn plausible_km(tr: &Track, gap_s: f64) -> f64 {
    (tr.speed_kt.max(250.0) * 2.0 * KNOTS_TO_MS * gap_s.abs()) / 1000.0 + 0.5
}

/// Starts the per-sample sources when measured samples first join a track: all so far are
/// Flightradar24's.
pub fn start_mixing(tr: &mut Track) {
    if tr.mixed {
        return;
    }
    tr.src.clear();
    tr.src.resize(tr.n, SRC_FR24);
    tr.mixed = true;
}

/// Puts a sample into a mixed track in time order. Samples within a second of it describe the
/// same moment: the better source wins (ADS-B over Flightradar24 over MLAT), and of equals the
/// newer arrival. Returns whether it went in.
pub fn insert_sample(tr: &mut Track, t: f64, lat: f64, lon: f64, src: u8) -> bool {
    let len = tr.ts.len();
    if (0..len).any(|j| (tr.ts[j] - t).abs() < COINCIDENT_MS && tr.src[j] > src) {
        return false;
    }
    for j in (0..len).rev() {
        if (tr.ts[j] - t).abs() < COINCIDENT_MS {
            tr.ts.remove(j);
            tr.lats.remove(j);
            tr.lons.remove(j);
            tr.src.remove(j);
        }
    }
    let mut i = tr.ts.len();
    while i > 0 && tr.ts[i - 1] > t {
        i -= 1;
    }
    tr.ts.insert(i, t);
    tr.lats.insert(i, lat);
    tr.lons.insert(i, lon);
    tr.src.insert(i, src);
    true
}

/// Index of the sample nearest in time to `t` (the first of equals), or `None` (-1) without samples.
pub fn nearest_sample(tr: &Track, t: f64) -> Option<usize> {
    let mut best: Option<usize> = None;
    for j in 0..tr.ts.len() {
        if best.is_none_or(|b| (tr.ts[j] - t).abs() < (tr.ts[b] - t).abs()) {
            best = Some(j);
        }
    }
    best
}

/// Drops all samples (a jump in the data: they would poison the spline), and the last fit's
/// reference rates with them.
pub fn clear_samples(tr: &mut Track) {
    tr.ts.clear();
    tr.lats.clear();
    tr.lons.clear();
    tr.src.clear();
    tr.r_ts.clear();
    tr.r_om.clear();
    tr.r_ac.clear();
}

/// Window trim, sample count, source bookkeeping and the local frame (around the newest sample),
/// after a merge.
pub fn finish_samples(tr: &mut Track) {
    let len = tr.ts.len();
    if len > 0 {
        let newest = tr.ts[len - 1];
        let mut start = 0;
        while start < len - 1 && tr.ts[start] < newest - SAMPLE_WINDOW_MS {
            start += 1;
        }
        if start > 0 {
            tr.ts.drain(..start);
            tr.lats.drain(..start);
            tr.lons.drain(..start);
            if tr.mixed {
                tr.src.drain(..start);
            }
        }
    }
    let n = tr.ts.len();
    tr.n = n;
    // still mixed while measured samples are left; per-sample sources are kept only then
    tr.mixed = tr.mixed && tr.src.iter().any(|&s| s != SRC_FR24);
    if !tr.mixed {
        tr.src.clear();
    }
    // (the TypeScript reads `undefined`, a NaN in arithmetic, without samples)
    tr.ref_lat = tr.lats.last().copied().unwrap_or(f64::NAN);
    tr.ref_lon = tr.lons.last().copied().unwrap_or(f64::NAN);
    let cos_ref = (tr.ref_lat * DEG).cos().max(1e-3);
    tr.xs.clear();
    tr.ys.clear();
    for i in 0..n {
        tr.xs.push(wrap180(tr.lons[i] - tr.ref_lon) * M_PER_DEG * cos_ref);
        tr.ys.push((tr.lats[i] - tr.ref_lat) * M_PER_DEG);
    }
}

/// A measured position joins the samples in time order. One that jumps away from them starts
/// them over if it is the newest data, and is dropped if not. Returns whether it went in.
pub fn merge_measured(tr: &mut Track, t: f64, lat: f64, lon: f64, src: u8) -> bool {
    if let Some(near) = nearest_sample(tr, t)
        && distance_km(tr.lats[near], tr.lons[near], lat, lon)
            > js_min(plausible_km(tr, (t - tr.ts[near]) / 1000.0), SNAP_DISTANCE_M / 1000.0)
    {
        if t <= tr.report_ms {
            return false;
        }
        start_mixing(tr);
        clear_samples(tr);
    }
    start_mixing(tr);
    let inserted = insert_sample(tr, t, clamp(lat, -90.0, 90.0), wrap_lon(lon), src);
    finish_samples(tr);
    inserted
}

/// Average turn rate (deg/s) over this report's own samples (those from `from`, its buffer's
/// oldest point, on: the last ~10 s up to the report), from chord headings in time order; NaN if
/// they are too short to tell.
pub fn buffer_turn_rate(tr: &Track, from: f64) -> f64 {
    let n = tr.n;
    let mut first_heading = f64::NAN;
    let mut first_mid = 0.0;
    let mut last_heading = f64::NAN;
    let mut last_mid = 0.0;
    let mut turned = 0.0;
    let mut i = 0;
    while i + 1 < n {
        if tr.ts[i] < from {
            i += 1;
            continue;
        }
        let dx = tr.xs[i + 1] - tr.xs[i];
        let dy = tr.ys[i + 1] - tr.ys[i];
        if dx.hypot(dy) < MIN_TURN_CHORD_M {
            // a gap in usable chords: only count a contiguous run ending at the newest
            first_heading = f64::NAN;
            turned = 0.0;
            i += 1;
            continue;
        }
        let heading = dx.atan2(dy) / DEG;
        let mid = (tr.ts[i] + tr.ts[i + 1]) / 2.0;
        if first_heading.is_nan() {
            first_heading = heading;
            first_mid = mid;
            turned = 0.0;
        } else {
            turned += wrap180(heading - last_heading);
        }
        last_heading = heading;
        last_mid = mid;
        i += 1;
    }
    if first_heading.is_nan() || last_mid - first_mid < 1000.0 {
        return f64::NAN;
    }
    turned / ((last_mid - first_mid) / 1000.0)
}

/// Acceleration (kt/s) from the chord speeds of this report's own samples (from `from` on, in
/// time order: the later chord minus the earlier), NaN if they span too little.
pub fn buffer_acceleration(tr: &Track, from: f64) -> f64 {
    let mut first_speed = f64::NAN;
    let mut first_mid = 0.0;
    let mut last_speed = f64::NAN;
    let mut last_mid = 0.0;
    let mut i = 0;
    while i + 1 < tr.n {
        if tr.ts[i] < from {
            i += 1;
            continue;
        }
        let dt = (tr.ts[i + 1] - tr.ts[i]) / 1000.0;
        let d = (tr.xs[i + 1] - tr.xs[i]).hypot(tr.ys[i + 1] - tr.ys[i]);
        if d < MIN_SPEED_CHORD_M {
            i += 1;
            continue;
        }
        let v = d / dt / KNOTS_TO_MS;
        let mid = (tr.ts[i] + tr.ts[i + 1]) / 2.0;
        if first_speed.is_nan() {
            first_speed = v;
            first_mid = mid;
        }
        last_speed = v;
        last_mid = mid;
        i += 1;
    }
    let span = (last_mid - first_mid) / 1000.0;
    if span >= MIN_ACCEL_DT_S { (last_speed - first_speed) / span } else { f64::NAN }
}
