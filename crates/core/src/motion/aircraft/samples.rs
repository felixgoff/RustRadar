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
use crate::trains::{js_max, js_min};

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
        let lon = tr.ref_lon + x / (M_PER_DEG * js_max((((tr.ref_lat + lat) / 2.0) * DEG).cos(), 1e-3));
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
    (js_max(tr.speed_kt, 250.0) * 2.0 * KNOTS_TO_MS * gap_s.abs()) / 1000.0 + 0.5
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
    let cos_ref = js_max((tr.ref_lat * DEG).cos(), 1e-3);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;
    use crate::motion::aircraft::Phase;
    use crate::motion::aircraft::fit::{self, fit_track};

    const T0: f64 = 1_760_000_000_000.0;

    fn close_all(actual: &[f64], expected: &[f64], digits: i32) {
        assert_eq!(actual.len(), expected.len(), "{actual:?} against {expected:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert_close!(*a, *e, digits);
        }
    }

    /// Due north at 200 kt from 45N 10E, a report at T0 with its buffer 2-8 s back, merged and
    /// fitted (bun: the same through `mergeReport` and `fitTrack`).
    fn straight(sc: &mut Scratch, fs: &mut fit::Scratch) -> Track {
        let mut tr = Track::new(1);
        tr.phase = Phase::Cruise;
        tr.speed_kt = 200.0;
        tr.report_ms = T0;
        let own = merge_report(&mut tr, sc, &[[185, 0, 2_000], [370, 0, 4_000], [555, 0, 6_000], [740, 0, 8_000]], T0, 45.0, 10.0, false);
        assert_eq!(own, Some(true));
        fit_track(&mut tr, fs, true);
        tr
    }

    #[test]
    fn report_points_are_the_buffer_in_time_order_then_the_report() {
        // bun: reportPoints({ positions }, 1e6, 50, 179.9999)
        let mut sc = Scratch::default();
        let positions = [
            [0, 0, 100],
            [10, 10, 2_000],
            [11, -11, 2_100],
            [20, -20, 4_000],
            [5, 5, 31_000],
            [0, 0, -5],
            [30, -30, 1_700],
            [9_000_001, 0, 3_000],
        ];
        report_points(&mut sc, &positions, 1e6, 50.0, 179.9999);
        // outside 200 ms..30 s: left out; within 200 ms of a point already in: the later one in the
        // buffer is left out; the buffer's longitudes wrap, the report's is taken as given
        assert_eq!(sc.new_t, [996_000.0, 997_000.0, 998_000.0, 998_300.0, 1e6]);
        close_all(&sc.new_lat, &[49.9998, -40.00001, 49.9999, 49.9997, 50.0], 12);
        close_all(&sc.new_lon, &[-179.99990000000003, 179.99990000000003, 179.99980000000005, -179.9998, 179.9999], 12);
        assert_eq!(sc.report_from_ms, 996_000.0);
        // no buffer: the report alone
        report_points(&mut sc, &[], 5.0, 1.0, 2.0);
        assert_eq!((sc.new_t.clone(), sc.new_lat.clone(), sc.new_lon.clone()), (vec![5.0], vec![1.0], vec![2.0]));
        assert_eq!(sc.report_from_ms, 5.0);
    }

    #[test]
    fn a_report_and_its_buffer_become_samples_in_a_frame_around_the_newest() {
        let mut sc = Scratch::default();
        let mut fs = fit::Scratch::default();
        let tr = straight(&mut sc, &mut fs);
        assert_eq!(sc.report_from_ms, T0 - 8_000.0);
        assert_eq!(tr.n, 5);
        assert_eq!(tr.ts, [T0 - 8_000.0, T0 - 6_000.0, T0 - 4_000.0, T0 - 2_000.0, T0]);
        assert_eq!((tr.ref_lat, tr.ref_lon), (45.0, 10.0));
        assert!(!tr.mixed && tr.src.is_empty());
        close_all(&tr.xs, &[0.0; 5], 12);
        close_all(&tr.ys, &[-822.8424571693971, -617.1318428772454, -411.4212285850936, -205.71061429215177, 0.0], 9);
        // bun: the fit of the same
        close_all(&tr.fx, &[0.0; 5], 12);
        close_all(&tr.fy, &[-822.8482069221811, -617.140924986869, -411.4307376764659, -205.71168378055066, 0.025410442198761984], 9);
        close_all(&tr.mx, &[0.0; 5], 12);
        close_all(&tr.my, &[102.8534493092302, 102.85402428450823, 102.85665742130911, 102.86320783973284, 102.87473339775653], 9);
        assert_eq!(tr.r_ts, tr.ts);
        close_all(&tr.r_om, &[0.0; 5], 15);
        close_all(&tr.r_ac, &[0.0008020280197271745, 0.0016264217504395144, 0.002660511065791127, 0.0034515188747169154, 0.004518994111855079], 12);
    }

    #[test]
    fn a_report_off_the_smoothed_path_starts_the_samples_over_unless_it_is_older() {
        let mut sc = Scratch::default();
        let mut fs = fit::Scratch::default();
        let mut tr = straight(&mut sc, &mut fs);
        // 2 s later and 300 m east of the line: well inside the plausible distance, but off the path
        let lat = 45.0 + (2.0 * 102.888) / M_PER_DEG;
        let lon = 10.0 + 300.0 / (M_PER_DEG * (45.0 * DEG).cos());
        report_points(&mut sc, &[], T0 + 2_000.0, lat, lon);
        assert!(off_path(&tr, &sc));
        // older than the measured state: dropped, the samples kept
        assert_eq!(merge_report(&mut tr, &mut sc, &[], T0 + 2_000.0, lat, lon, true), None);
        assert_eq!(tr.n, 5);
        // newer: the samples (and the fit's rates) start over from it
        assert_eq!(merge_report(&mut tr, &mut sc, &[], T0 + 2_000.0, lat, lon, false), Some(true));
        assert_eq!(tr.ts, [T0 + 2_000.0]);
        close_all(&tr.lats, &[45.00185058802779], 12);
        close_all(&tr.lons, &[10.003815498436076], 12);
        assert!(tr.r_ts.is_empty() && tr.r_om.is_empty() && tr.r_ac.is_empty());
        // on the line it would have been fine
        let mut tr = straight(&mut sc, &mut fs);
        report_points(&mut sc, &[], T0 + 2_000.0, lat, 10.0);
        assert!(!off_path(&tr, &sc));
        // a single sample, or one not yet fitted, is never off a path
        tr.fx.clear();
        assert!(!off_path(&tr, &sc));
    }

    #[test]
    fn a_report_implausibly_far_from_the_nearest_sample_starts_them_over() {
        let mut sc = Scratch::default();
        let mut fs = fit::Scratch::default();
        let mut tr = straight(&mut sc, &mut fs);
        // 3 s at 250 kt (the floor) twice over, plus half a kilometre
        assert_close!(plausible_km(&tr, -3.0), 1.271666, 12);
        tr.speed_kt = 60.0;
        assert_close!(plausible_km(&tr, -3.0), 1.271666, 12);
        tr.speed_kt = f64::NAN;
        assert!(plausible_km(&tr, 1.0).is_nan());
        tr.speed_kt = 200.0;
        // 10 km north 4 s later: far, wherever the path goes
        assert_eq!(merge_report(&mut tr, &mut sc, &[], T0 + 4_000.0, 45.09, 10.0, false), Some(true));
        assert_eq!(tr.ts, [T0 + 4_000.0]);
        assert_eq!(nearest_sample(&tr, 0.0), Some(0));
        clear_samples(&mut tr);
        finish_samples(&mut tr);
        assert_eq!(nearest_sample(&tr, 0.0), None);
        assert_eq!(tr.n, 0);
        assert!(tr.ref_lat.is_nan() && tr.xs.is_empty());
    }

    #[test]
    fn measured_samples_join_by_time_and_source_rank() {
        let mut sc = Scratch::default();
        let mut fs = fit::Scratch::default();
        let mut tr = straight(&mut sc, &mut fs);
        start_mixing(&mut tr);
        assert_eq!(tr.src, [SRC_FR24; 5]);
        // ADS-B 400 ms after the report replaces it; MLAT 300 ms after a Flightradar24 sample is
        // outranked; MLAT between two samples goes in
        assert!(insert_sample(&mut tr, T0 + 400.0, 45.0004, 10.00001, SRC_ADSB));
        assert!(!insert_sample(&mut tr, T0 - 1_700.0, 44.998, 10.0, SRC_MLAT));
        assert!(insert_sample(&mut tr, T0 - 5_000.0, 44.9955, 10.00002, SRC_MLAT));
        finish_samples(&mut tr);
        // bun: the same through insertSample and finishSamples
        assert_eq!(tr.ts, [T0 - 8_000.0, T0 - 6_000.0, T0 - 5_000.0, T0 - 4_000.0, T0 - 2_000.0, T0 + 400.0]);
        close_all(&tr.lats, &[44.9926, 44.99445, 44.9955, 44.9963, 44.99815, 45.0004], 12);
        close_all(&tr.lons, &[10.0, 10.0, 10.00002, 10.0, 10.0, 10.00001], 12);
        assert_eq!(tr.src, [1, 1, 0, 1, 1, 2]);
        assert!(tr.mixed);
        close_all(&tr.xs, &[-0.7862613754565532, -0.7862613754565532, 0.7862613754565532, -0.7862613754565532, -0.7862613754565532, 0.0], 9);
        close_all(&tr.ys, &[-867.320427827117, -661.6098135349653, -544.8551405582531, -455.89919924281344, -250.1885849498716, 0.0], 9);
        // the weighted fit (MLAT counts for little), with the measured sample newest
        fit_track(&mut tr, &mut fs, false);
        close_all(&tr.fx, &[-0.794828189324932, -0.8220527256605591, -0.8034524833309878, -0.7586129227248226, -0.5684485090905784, -0.13751460067385204], 9);
        close_all(&tr.fy, &[-867.316593919666, -661.6658437239576, -558.7216739992173, -455.681630676544, -249.20347748377787, -0.5958184131833533], 9);
        close_all(&tr.mx, &[-0.03243596700264692, 0.00668781719014926, 0.030891690737854513, 0.059697137282811064, 0.13392439830561748, 0.21946999128210537], 9);
        close_all(&tr.my, &[102.75556787557511, 102.90044768135954, 102.98832553708202, 103.09681696893638, 103.39765710944003, 103.75148576921985], 9);
        close_all(&tr.r_om, &[0.00016313348278043166, 0.00020861896952645235, 0.0002224691798357844, 0.00022322948449482515, 0.0002519856469062246, 0.0002741879492979981], 12);
        // a Flightradar24 report whose buffer point coincides with the ADS-B sample: outranked
        let lat = 45.0 + (2.0 * 102.888) / M_PER_DEG;
        tr.report_ms = T0 + 2_000.0;
        assert_eq!(merge_report(&mut tr, &mut sc, &[[0, 0, 1_700], [180, 0, 2_000]], T0 + 2_000.0, lat, 10.0, false), Some(true));
        assert_eq!(tr.ts, [T0 - 8_000.0, T0 - 6_000.0, T0 - 5_000.0, T0 - 4_000.0, T0 - 2_000.0, T0 + 400.0, T0 + 2_000.0]);
        assert_eq!(tr.src, [1, 1, 0, 1, 1, 2, 1]);
        close_all(&tr.lats, &[44.9926, 44.99445, 44.9955, 44.9963, 44.99815, 45.0004, 45.00185058802779], 12);
        // a far measured position: older than the report, dropped; newer, the samples start over
        assert!(!merge_measured(&mut tr, T0 + 1_000.0, 46.0, 10.0, SRC_ADSB));
        assert_eq!(tr.n, 7);
        assert!(merge_measured(&mut tr, T0 + 3_000.0, 46.0, 10.0, SRC_ADSB));
        assert_eq!((tr.ts.clone(), tr.src.clone(), tr.mixed), (vec![T0 + 3_000.0], vec![SRC_ADSB], true));
        assert!(tr.r_ts.is_empty());
        // once only Flightradar24's samples are left, the track is no longer mixed
        tr.report_ms = T0 + 4_000.0;
        merge_report(&mut tr, &mut sc, &[], T0 + 4_000.0, 46.5, 10.0, false);
        assert!(!tr.mixed && tr.src.is_empty());
    }

    #[test]
    fn the_window_keeps_thirty_seconds_and_the_buffer_tells_the_acceleration() {
        // a braking roll: 80 kt slowing at 2 kt/s, a report every 2 s for 40 s (bun: the same)
        let mut sc = Scratch::default();
        let mut tr = Track::new(1);
        tr.phase = Phase::LandingRoll;
        tr.speed_kt = 60.0;
        tr.on_ground = true;
        for k in 0..=20 {
            let t = T0 + k as f64 * 2_000.0;
            let s = k as f64 * 2.0;
            let dist = 80.0 * KNOTS_TO_MS * s - 0.5 * 2.0 * KNOTS_TO_MS * s * s;
            tr.report_ms = t;
            merge_report(&mut tr, &mut sc, &[], t, 45.0 + dist / M_PER_DEG, 10.0, false);
        }
        assert_eq!((tr.n, tr.ts[0], tr.ts[tr.n - 1]), (16, T0 + 10_000.0, T0 + 40_000.0));
        assert_close!(buffer_acceleration(&tr, T0 + 24_000.0), -2.00000000004163, 9);
        // straight on: no chord turns, and too short a run of usable chords to tell anyway
        assert!(buffer_turn_rate(&tr, T0 + 24_000.0).is_nan());
        // too little left to tell an acceleration
        assert!(buffer_acceleration(&tr, T0 + 37_000.0).is_nan());
    }

    #[test]
    fn the_buffer_tells_the_turn_rate() {
        // the first report of a 2 deg/s right turn at 220 kt (bun: bufferTurnRate, bufferAcceleration)
        let mut sc = Scratch::default();
        let mut tr = Track::new(1);
        tr.speed_kt = 220.0;
        merge_report(&mut tr, &mut sc, &[[203, -10, 2_000], [406, -40, 4_000], [606, -90, 6_000], [804, -160, 8_000]], T0, 45.0, 10.0, false);
        assert_close!(buffer_turn_rate(&tr, sc.report_from_ms), 2.00676453322518, 9);
        assert_close!(buffer_acceleration(&tr, sc.report_from_ms), -0.17471799456329032, 9);
        // only the report's own samples count
        assert!(buffer_turn_rate(&tr, T0 - 1_000.0).is_nan());
    }
}
