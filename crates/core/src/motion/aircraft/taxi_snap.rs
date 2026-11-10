//! Taxiway snapping (`motion.ts` `taxiSnap`, `:2034-2216`; doc 05 §10.13).
//!
//! Reported positions wander metres either side of the taxiway centrelines and cut the
//! corners. A slow aircraft on the ground within `SNAP_FAR_M` of a line and moving along it
//! (either way, within `SNAP_WIDE_DEG`, now or `SNAP_LOOKAHEAD_MS` ahead: the data is there,
//! since aircraft are drawn in the past) is pulled onto it and pointed along it, fully within
//! `SNAP_NEAR_M` and `SNAP_ALIGNED_DEG`.
//!
//! Nothing jumps. Each line within `SNAP_FAR_M` gets a share by nearness (`SNAP_SOFT_M`) and by
//! how well it matches the motion now and a moment ahead, and the shares blend the lines'
//! nearest points and directions; the blend pulls as far as it is near and along the motion.
//! Through a junction the shares hand over from one line to the next as the aircraft turns,
//! which rounds the corner. The offset follows that pull through a critically damped spring
//! (the drift across the line fed forward, so along a line it stays exact), and the whole fades
//! in and out with the ground speed and phase. A parked aircraft keeps the offset it stopped
//! with (holding short on a taxiway), but is never pulled onto a line.

use std::sync::Arc;

use super::consts::*;
use super::evaluate::evaluate;
use super::track::{Raw, Track};
use super::{Phase, World, clamp, smoothstep, spring, wrap_lon, wrap180, wrap360};
use crate::taxinet::{TaxiNetwork, project};
use crate::trains::js_max;

impl World {
    /// Draws a slow aircraft on the ground on the airport's taxiway centrelines: moves `raw`'s
    /// position, heading and track (drawn at `t` at `now_ms`). Called every frame for every
    /// aircraft while a network is loaded (or the snap is still fading out); the network is the
    /// track's own while it covers the aircraft, else the first loaded that does.
    pub fn taxi_snap(&self, tr: &mut Track, raw: &mut Raw, now_ms: f64, t: f64) {
        let gap = now_ms - tr.snap_ms;
        tr.snap_ms = now_ms;
        let recent = gap <= CONTINUITY_GAP_MS;
        // the network the aircraft is in, and where in it
        let mut net: Option<Arc<TaxiNetwork>> = None;
        let mut x = 0.0;
        let mut y = 0.0;
        if raw.ground {
            let mut own_covers = false;
            if let Some(own) = &tr.snap_net
                && !self.taxiways.is_empty()
            {
                x = own.x(raw.lon);
                y = own.y(raw.lat);
                own_covers = own.covers(x, y);
            }
            if own_covers {
                net = tr.snap_net.clone();
            } else {
                for n in self.taxiways.values() {
                    x = n.x(raw.lon);
                    y = n.y(raw.lat);
                    if n.covers(x, y) {
                        net = Some(Arc::clone(n));
                        break;
                    }
                }
            }
        }
        let own_net = match (&net, &tr.snap_net) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        let holding = raw.phase == Phase::Parked && own_net && tr.snap_gate > 0.0;
        let target = if net.is_none() || (raw.phase == Phase::Parked && !holding) {
            0.0
        } else {
            1.0 - smoothstep((raw.speed_ms / KNOTS_TO_MS - SNAP_SLOW_KT) / (SNAP_FAST_KT - SNAP_SLOW_KT))
        };
        if !recent {
            tr.snap_gate = target; // not drawn a moment ago: nothing to ease from
        } else {
            let step = (SNAP_GATE_RATE * gap) / 1000.0;
            tr.snap_gate = clamp(target, tr.snap_gate - step, tr.snap_gate + step);
        }
        tr.snap_net.clone_from(&net);
        if tr.snap_gate <= 0.0 && target <= 0.0 {
            tr.snap_e = 0.0;
            tr.snap_n = 0.0;
            tr.snap_h = 0.0;
            tr.snap_ve = 0.0;
            tr.snap_vn = 0.0;
            tr.snap_vh = 0.0;
            return;
        }

        let mut e = tr.snap_e;
        let mut n = tr.snap_n;
        let mut h = tr.snap_h;
        if holding {
            tr.snap_ve = 0.0;
            tr.snap_vn = 0.0;
            tr.snap_vh = 0.0;
        } else if let Some(net) = &net {
            // the direction of motion, now and a moment ahead
            let (dx, dy) = if raw.speed_ms > SNAP_MIN_SPEED_MS {
                (raw.v_e / raw.speed_ms, raw.v_n / raw.speed_ms)
            } else {
                ((raw.track * DEG).sin(), (raw.track * DEG).cos())
            };
            let mut fdx = dx;
            let mut fdy = dy;
            let mut sum_a = 0.0;
            let mut sum_x = 0.0;
            let mut sum_y = 0.0;
            let mut sum_hx = 0.0;
            let mut sum_hy = 0.0;
            if let Some(list) = net.near(x, y) {
                let mut looked = false;
                for &s in list {
                    let (proj_x, proj_y, proj_dist) = project(net, s, x, y);
                    if proj_dist >= SNAP_FAR_M {
                        continue;
                    }
                    if !looked {
                        looked = true;
                        let ahead = evaluate(tr, t + SNAP_LOOKAHEAD_MS, None);
                        if ahead.speed_ms > SNAP_MIN_SPEED_MS {
                            fdx = ahead.v_e / ahead.speed_ms;
                            fdy = ahead.v_n / ahead.speed_ms;
                        }
                        // the model ahead changes as reports come in (the drawn pose blends that
                        // out; this doesn't): followed with a lag, so the lines it picks don't either
                        if recent && tr.snap_ahead_ms == tr.snap_ms - gap {
                            let k = 1.0 - (-gap / SNAP_AHEAD_LAG_MS).exp();
                            fdx = tr.snap_ax + (fdx - tr.snap_ax) * k;
                            fdy = tr.snap_ay + (fdy - tr.snap_ay) * k;
                            let l = fdx.hypot(fdy);
                            if l > 1e-6 {
                                fdx /= l;
                                fdy /= l;
                            } else {
                                fdx = dx;
                                fdy = dy;
                            }
                        }
                        tr.snap_ax = fdx;
                        tr.snap_ay = fdy;
                        tr.snap_ahead_ms = now_ms;
                    }
                    let ux = net.ux()[s];
                    let uy = net.uy()[s];
                    let now = ux * dx + uy * dy;
                    let later = ux * fdx + uy * fdy;
                    // the line's share: by nearness and how well it matches the motion, which
                    // shifts over from line to line as the aircraft turns through a junction
                    let near = smoothstep((SNAP_FAR_M - proj_dist) / (SNAP_FAR_M - SNAP_NEAR_M));
                    let align = SNAP_NOW_SHARE * now * now + (1.0 - SNAP_NOW_SHARE) * later * later;
                    let a = near * (-proj_dist / SNAP_SOFT_M).exp() * align;
                    if !(a > 0.0) {
                        continue;
                    }
                    // pointing along the line the way the aircraft goes
                    let sign = if (if now.abs() >= later.abs() { now } else { later }) >= 0.0 { a } else { -a };
                    sum_a += a;
                    sum_x += a * proj_x;
                    sum_y += a * proj_y;
                    sum_hx += sign * ux;
                    sum_hy += sign * uy;
                }
            }
            // the lines' blended point and direction pull as far as they are near and along the
            // motion (now or a moment ahead): through a junction the blend turns with the aircraft
            let hl = sum_hx.hypot(sum_hy);
            e = 0.0;
            n = 0.0;
            h = 0.0;
            // how fast the offset moves with the aircraft itself (its drift across the line), m/s
            let mut ff_e = 0.0;
            let mut ff_n = 0.0;
            if sum_a > 0.0 && hl > 1e-9 * sum_a {
                let px = sum_x / sum_a - x;
                let py = sum_y / sum_a - y;
                let hx = sum_hx / hl;
                let hy = sum_hy / hl;
                let c = js_max((hx * dx + hy * dy).abs(), (hx * fdx + hy * fdy).abs());
                let w = smoothstep((SNAP_FAR_M - px.hypot(py)) / (SNAP_FAR_M - SNAP_NEAR_M))
                    * smoothstep((c - cos_snap_wide()) / (cos_snap_aligned() - cos_snap_wide()));
                if w > 0.0 {
                    e = px * w;
                    n = py * w;
                    h = wrap180(hx.atan2(hy) / DEG - raw.track) * w;
                    let along = raw.v_e * hx + raw.v_n * hy;
                    ff_e = -(raw.v_e - along * hx) * w;
                    ff_n = -(raw.v_n - along * hy) * w;
                }
            }
            // followed through critically damped springs, so that what's left of sharp corners,
            // and a held offset let go, are smoothed out (C1): the offset with its drift fed
            // forward, and the heading as drawn, which stays put along a line
            if recent {
                let dt = gap / 1000.0;
                let (followed_e, ve) = spring(tr.snap_e + ff_e * dt, tr.snap_ve - ff_e, e, dt);
                e = followed_e;
                tr.snap_ve = ve + ff_e;
                let (followed_n, vn) = spring(tr.snap_n + ff_n * dt, tr.snap_vn - ff_n, n, dt);
                n = followed_n;
                tr.snap_vn = vn + ff_n;
                let drawn = raw.track + h;
                let (heading, vh) = spring(tr.snap_hd, tr.snap_vh, tr.snap_hd + wrap180(drawn - tr.snap_hd), dt);
                tr.snap_vh = vh;
                h = wrap180(heading - raw.track);
            } else {
                tr.snap_ve = 0.0;
                tr.snap_vn = 0.0;
                tr.snap_vh = 0.0;
            }
        }
        tr.snap_e = e;
        tr.snap_n = n;
        tr.snap_h = h;
        tr.snap_hd = wrap360(raw.track + h);
        let g = smoothstep(tr.snap_gate);
        if g <= 0.0 {
            return;
        }
        raw.lat = clamp(raw.lat + (g * n) / M_PER_DEG, -90.0, 90.0);
        raw.lon = wrap_lon(raw.lon + (g * e) / (M_PER_DEG * js_max((raw.lat * DEG).cos(), 1e-3)));
        raw.heading = wrap360(raw.heading + g * h);
        raw.track = wrap360(raw.track + g * h);
    }
}

#[cfg(test)]
mod tests {
    use super::super::gear::testing::*;
    use super::super::{Motion, Pose};
    use super::*;
    use crate::assert_close;
    use crate::model::{LiveFlight, TaxiKind, TaxiLine};
    use crate::trains::js_min;

    /// `taxiLine`: a taxiway line through local [east, north] metre points around 45°N 10°E, as
    /// OpenStreetMap gives it ([lon, lat]).
    fn taxi_line(points: &[(f64, f64)], kind: TaxiKind) -> TaxiLine {
        let points = points
            .iter()
            .map(|&(e, n)| {
                let (lat, lon) = offset(45.0, 10.0, e, n);
                [lon, lat]
            })
            .collect();
        TaxiLine { kind, r#ref: None, points }
    }

    fn to_local(p: &Pose) -> (f64, f64) {
        ((p.lon - 10.0) * M_PER_DEG * (45.0 * DEG).cos(), (p.lat - 45.0) * M_PER_DEG)
    }

    /// Metres from a local point to a polyline.
    fn off_line(p: (f64, f64), line: &[(f64, f64)]) -> f64 {
        let mut best = f64::INFINITY;
        for seg in line.windows(2) {
            let (ax, ay) = seg[0];
            let (bx, by) = seg[1];
            let l2 = (bx - ax).powi(2) + (by - ay).powi(2);
            let t = js_max(0.0, js_min(1.0, ((p.0 - ax) * (bx - ax) + (p.1 - ay) * (by - ay)) / l2));
            best = js_min(best, (p.0 - ax - t * (bx - ax)).hypot(p.1 - ay - t * (by - ay)));
        }
        best
    }

    fn angle_diff(a: f64, b: f64) -> f64 {
        signed(a - b).abs()
    }

    /// What `taxiL` measured.
    #[derive(Debug, Default)]
    struct Taxied {
        /// Largest distance from the centreline where the truth is on a straight leg (> 60 m
        /// from the corner), m.
        leg_off: f64,
        /// Largest heading error from the leg's direction there, deg.
        leg_heading: f64,
        /// Largest distance from the centreline anywhere, m.
        max_off: f64,
        /// Largest movement in a 16 ms frame, m, and heading change, deg.
        max_step: f64,
        max_turn: f64,
    }

    /// `taxiL`: taxies an L: north along x = 0 from 400 m south of the corner, a 30 m radius turn
    /// right, then east, at 15 kt; Flightradar24-like reports every 6 s (the 6 s before in the
    /// buffer, 2 s apart), each arriving 3 s after its time, positions off by up to ±`noise` m.
    /// Drawn through 16 ms frames.
    fn taxi_l(lines: Option<&[TaxiLine]>, noise: f64) -> Taxied {
        let mut m = Motion::new();
        if let Some(lines) = lines {
            m.set_taxiways("XTST", Some(lines));
        }
        let v = 15.0 * KT;
        let r = 30.0;
        let leg1 = 370.0;
        let arc = (std::f64::consts::PI / 2.0) * r;
        let truth = move |s: f64| {
            let d = v * s;
            if d <= leg1 {
                return (0.0, -400.0 + d);
            }
            if d <= leg1 + arc {
                let a = (d - leg1) / r;
                return (r - r * a.cos(), -r + r * a.sin());
            }
            (r + (d - leg1 - arc), 0.0)
        };
        let jitter = move |s: f64, axis: f64| {
            let x = ((s * 3.0 + axis) * 12.9898).sin() * 43758.5453;
            noise * ((x - x.floor()) * 2.0 - 1.0)
        };
        let measured = move |s: f64| {
            let (e, n) = truth(s);
            offset(45.0, 10.0, e + jitter(s, 0.0), n + jitter(s, 1.0))
        };
        let report = |k: i64| {
            let s0 = (k * 6) as f64;
            let (lat, lon) = measured(s0);
            let (e0, n0) = truth(s0);
            let (e1, n1) = truth(s0 + 0.5);
            let track = ((e1 - e0).atan2(n1 - n0) / DEG + 360.0) % 360.0;
            let f = flight_with_history(lat, lon, |d| measured(s0 - d), &[2.0, 4.0, 6.0]);
            LiveFlight {
                alt: 0,
                on_ground: true,
                speed: 15,
                track: crate::num::js_round(track) as i32,
                timestamp_ms: (T0 + s0 * 1000.0) as u64,
                ..f
            }
        };
        let centre = [(0.0, -500.0), (0.0, 0.0), (500.0, 0.0)];
        let mut out = Taxied::default();
        let mut k = 0;
        let mut f = report(0);
        let mut prev: Option<Pose> = None;
        let total_s = (leg1 + arc + 300.0) / v;
        let mut t = T0 + 3_000.0;
        while t < T0 + total_s * 1000.0 {
            while T0 + ((k + 1) * 6_000) as f64 + 3_000.0 <= t {
                k += 1;
                f = report(k);
            }
            let p = m.pose(&f, t);
            if t < T0 + 20_000.0 {
                prev = Some(p);
                t += 16.0;
                continue; // settling in
            }
            let s = (m.drawn_time(1, t) - T0) / 1000.0;
            let (te, tn) = truth(s);
            let off = off_line(to_local(&p), &centre);
            out.max_off = js_max(out.max_off, off);
            if tn < -60.0 {
                out.leg_off = js_max(out.leg_off, off);
                out.leg_heading = js_max(out.leg_heading, angle_diff(p.heading, 0.0));
            } else if te > 60.0 {
                out.leg_off = js_max(out.leg_off, off);
                out.leg_heading = js_max(out.leg_heading, angle_diff(p.heading, 90.0));
            }
            if let Some(prev) = &prev {
                out.max_step = js_max(out.max_step, metres(prev, &p));
                out.max_turn = js_max(out.max_turn, angle_diff(p.heading, prev.heading));
            }
            prev = Some(p);
            t += 16.0;
        }
        out
    }

    #[test]
    fn a_taxiing_aircraft_is_drawn_on_the_taxiway_centreline_turning_with_it() {
        // drawn from the reports alone, the noise shows: metres off the line, the nose wandering
        let free = taxi_l(None, 8.0);
        assert!(free.leg_off > 2.0, "{free:?}");
        // on the network: on the line along the legs, pointing along them, and smooth through the corner
        let lines = [
            taxi_line(&[(0.0, -500.0), (0.0, -200.0), (0.0, 0.0)], TaxiKind::Taxiway),
            taxi_line(&[(0.0, 0.0), (250.0, 0.0), (500.0, 0.0)], TaxiKind::Taxiway),
            taxi_line(&[(0.0, 0.0), (-300.0, 0.0)], TaxiKind::Taxiway),
        ];
        let snapped = taxi_l(Some(&lines), 8.0);
        assert!(snapped.leg_off < 0.5, "{snapped:?}");
        assert!(snapped.leg_heading < 1.0, "{snapped:?}");
        assert!(snapped.max_off < 8.0, "{snapped:?}");
        // 15 kt is 0.12 m a frame: through the corner a little more, but no jumps, and turns of at
        // most ~45 deg/s
        assert!(snapped.max_step < 0.25, "{snapped:?}");
        assert!(snapped.max_turn < 0.75, "{snapped:?}");
    }

    #[test]
    fn taxiway_snapping_leaves_alone_what_isnt_taxiing_along_a_line() {
        let lines = [
            taxi_line(&[(0.0, -1000.0), (0.0, 1000.0)], TaxiKind::Taxiway),
            taxi_line(&[(-1000.0, 0.0), (1000.0, 0.0)], TaxiKind::Runway),
        ];
        let at = |id: u32, speed: i32, track: i32, e: f64, n: f64| {
            let (lat, lon) = offset(45.0, 10.0, e, n);
            LiveFlight { id, on_ground: true, speed, alt: 0, track, timestamp_ms: T0 as u64, ..flight_at(lat, lon) }
        };
        let cases = [
            // parked on an apron 80 m from the taxiway, pointing along it
            at(1, 0, 0, 80.0, 300.0),
            // taxiing 80 m from it, along it
            at(2, 12, 0, 80.0, -300.0),
            // taxiing across it, 10 m away
            at(3, 12, 90, -10.0, 500.0),
            // a takeoff roll at 100 kt, 10 m off the runway's centreline
            at(4, 100, 90, -500.0, 10.0),
        ];
        for f in &cases {
            let mut plain = Motion::new();
            let mut net = Motion::new();
            net.set_taxiways("XTST", Some(&lines));
            let mut t = T0;
            while t <= T0 + 8_000.0 {
                let a = plain.pose(f, t);
                let b = net.pose(f, t);
                assert!(metres(&a, &b) < 0.001, "{}: {}", f.id, metres(&a, &b));
                assert!(angle_diff(a.heading, b.heading) < 0.001, "{}", f.id);
                t += 16.0;
            }
        }
        // the speeding roll is a takeoff roll; the first two are parked and taxiing
        let mut roll = Motion::new();
        assert_eq!(roll.pose(&cases[3], T0 + 4_000.0).phase, Phase::TakeoffRoll);
    }

    #[test]
    fn a_taxiing_aircraft_near_a_taxiway_is_pulled_onto_it_smoothly_and_forgetting_the_network_lets_it_go_smoothly() {
        let lines = [taxi_line(&[(0.0, -2000.0), (0.0, 2000.0)], TaxiKind::Taxiway)];
        let mut m = Motion::new();
        m.set_taxiways("XTST", Some(&lines));
        // 8 m east of the line, taxiing north at 12 kt
        let v = 12.0 * KT;
        let f = LiveFlight { on_ground: true, speed: 12, alt: 0, track: 0, ..path_flight(move |s| (8.0, v * s), T0, &BUFFER) };
        let mut prev = m.pose(&f, T0);
        let mut off = 0.0;
        let mut t = T0 + 16.0;
        while t <= T0 + 6_000.0 {
            let p = m.pose(&f, t);
            assert!(metres(&prev, &p) < v * 0.016 + 0.15, "{}: {}", t - T0, metres(&prev, &p));
            off = to_local(&p).0.abs();
            prev = p;
            t += 16.0;
        }
        assert!(off < 0.1, "{off}");
        assert_close!(prev.heading, 0.0, 1);
        assert!(m.has_taxiways("XTST"));
        m.set_taxiways("XTST", None);
        assert!(!m.has_taxiways("XTST"));
        let mut t = T0 + 6_016.0;
        while t <= T0 + 10_000.0 {
            let p = m.pose(&f, t);
            assert!(metres(&prev, &p) < v * 0.016 + 0.15, "{}: {}", t - T0, metres(&prev, &p));
            prev = p;
            t += 16.0;
        }
        assert!((to_local(&prev).0 - 8.0).abs() < 0.5, "{}", to_local(&prev).0);
    }

    #[test]
    fn the_gate_eases_with_the_speed_and_a_parked_aircraft_holds_its_offset() {
        let line = [taxi_line(&[(0.0, -500.0), (0.0, 500.0)], TaxiKind::Taxiway)];
        // a track taxiing north at 10 kt, 6 m east of the line (no network loaded yet: the
        // calls below are the snap on its own)
        let (lat, lon) = offset(45.0, 10.0, 6.0, 0.0);
        let mut m = Motion::new();
        let f = LiveFlight { on_ground: true, speed: 10, alt: 0, track: 0, ..flight_at(lat, lon) };
        m.pose(&f, T0);
        let base = m.tracks.remove(&1).expect("the track posed");
        let net = Arc::new(TaxiNetwork::new(&line));
        let mut w = World::default();
        w.taxiways.insert("XTST".into(), Arc::clone(&net));
        let (lat, lon) = (seen(lat), seen(lon));
        let local_x = |lon: f64| (lon - 10.0) * M_PER_DEG * (45.0 * DEG).cos();
        let raw_at = |phase: Phase, speed_kt: f64| Raw {
            lat,
            lon,
            heading: 3.0,
            track: 3.0,
            phase,
            speed_ms: speed_kt * KNOTS_TO_MS,
            v_n: speed_kt * KNOTS_TO_MS,
            ground: true,
            ..Raw::default()
        };
        // not drawn a moment ago: the gate is set outright, the offset taken without a spring
        let mut tr = base.clone();
        let mut raw = raw_at(Phase::Taxi, 10.0);
        w.taxi_snap(&mut tr, &mut raw, T0, T0);
        assert_eq!(tr.snap_gate, 1.0);
        assert!(Arc::ptr_eq(tr.snap_net.as_ref().unwrap(), &net));
        assert!((tr.snap_e + local_x(lon)).abs() < 1e-6 && tr.snap_n.abs() < 1e-6, "{} {}", tr.snap_e, tr.snap_n);
        assert!((tr.snap_h + 3.0).abs() < 1e-9, "{}", tr.snap_h);
        assert!(local_x(raw.lon).abs() < 1e-6, "{}", local_x(raw.lon));
        assert!(signed(raw.heading).abs() < 1e-9 && signed(raw.track).abs() < 1e-9, "{}", raw.heading);
        // at 30 kt the gate's target is half open, eased at 0.8 a second
        let mut raw = raw_at(Phase::Taxi, 30.0);
        w.taxi_snap(&mut tr, &mut raw, T0 + 100.0, T0);
        assert_eq!(tr.snap_gate, 1.0 - 0.08);
        // parked on the same network with the gate open: held where it stopped, no pull
        let mut tr = base.clone();
        let mut raw = raw_at(Phase::Taxi, 10.0);
        w.taxi_snap(&mut tr, &mut raw, T0, T0);
        let held = (tr.snap_e, tr.snap_n, tr.snap_h);
        let mut raw = raw_at(Phase::Parked, 0.0);
        w.taxi_snap(&mut tr, &mut raw, T0 + 16.0, T0);
        assert_eq!((tr.snap_e, tr.snap_n, tr.snap_h), held);
        assert_eq!((tr.snap_ve, tr.snap_vn, tr.snap_vh, tr.snap_gate), (0.0, 0.0, 0.0, 1.0));
        assert!(local_x(raw.lon).abs() < 1e-6);
        // a parked aircraft that never snapped is left alone; in the air nothing is snapped
        let mut tr = base.clone();
        let mut raw = raw_at(Phase::Parked, 0.0);
        w.taxi_snap(&mut tr, &mut raw, T0, T0);
        assert_eq!((tr.snap_gate, raw.lat, raw.lon), (0.0, lat, lon));
        let mut raw = Raw { ground: false, ..raw_at(Phase::Taxi, 10.0) };
        w.taxi_snap(&mut tr, &mut raw, T0 + 16.0, T0);
        assert!(tr.snap_net.is_none() && tr.snap_gate == 0.0 && raw.lon == lon);
    }
}
