//! QNH learned from the traffic (`motion.ts` `fieldQnh`, `learnQnh`, `trackQnh`, `setQnh`,
//! `:1180-1226`, `:1594-1603`; doc 05 §10.5).
//!
//! Flightradar24 reports pressure altitude (referenced to 1013.25 hPa). Near a field it is
//! corrected to true height with the field's QNH, which the model learns from the traffic itself:
//! every aircraft rolling on a runway reads the runway's pressure altitude, which at the field's
//! known elevation gives its QNH. A field nobody has read yet takes the inverse-square-distance
//! mean of the fields around within 300 km.

use std::sync::Arc;

use super::altitude::{correct_alt, corrected, valid_qnh};
use super::consts::*;
use super::track::Track;
use super::{AirportKey, Motion, QnhAround, QnhReading, World, distance_km, meas_fresh, same_airport};
use crate::model::Airport;
use crate::trains::js_max;

impl World {
    /// The QNH (hPa) learned for a field from the traffic at `now_ms`: its own, else interpolated
    /// from fields around (cached until anything is learned or a minute passes); NaN if none is
    /// known.
    pub fn field_qnh(&mut self, ap: &Arc<Airport>, now_ms: f64) -> f64 {
        let key = AirportKey(Arc::clone(ap));
        if let Some(own) = self.qnhs.get(&key)
            && now_ms - own.ms <= QNH_TTL_MS
        {
            return own.hpa;
        }
        if self.qnhs.is_empty() {
            return f64::NAN;
        }
        if let Some(cached) = self.qnh_around.get(&key)
            && cached.version == self.qnh_version
            && (now_ms - cached.ms).abs() < 60_000.0
        {
            return cached.hpa;
        }
        let mut sum = 0.0;
        let mut weights = 0.0;
        for (other, q) in &self.qnhs {
            if now_ms - q.ms > QNH_TTL_MS {
                continue;
            }
            let km = distance_km(ap.lat, ap.lon, other.0.lat, other.0.lon);
            if km > QNH_NEIGHBOUR_KM {
                continue;
            }
            let w = 1.0 / ((km + 20.0) * (km + 20.0));
            sum += w * q.hpa;
            weights += w;
        }
        let hpa = if weights > 0.0 { sum / weights } else { f64::NAN };
        self.qnh_around.insert(key, QnhAround { hpa, ms: now_ms, version: self.qnh_version });
        hpa
    }

    /// A reading of a field's QNH (hPa) at `ms`: a new or expired one replaces what was known,
    /// otherwise it is smoothed in (slowly for an outlier).
    pub fn learn_qnh(&mut self, ap: &Arc<Airport>, hpa: f64, ms: f64) {
        if !valid_qnh(hpa) {
            return;
        }
        self.qnh_version += 1;
        let key = AirportKey(Arc::clone(ap));
        match self.qnhs.get_mut(&key) {
            Some(q) if !(ms - q.ms > QNH_TTL_MS) => {
                let k = if (hpa - q.hpa).abs() > QNH_OUTLIER_HPA { QNH_OUTLIER_SMOOTHING } else { QNH_SMOOTHING };
                q.hpa += k * (hpa - q.hpa);
                q.ms = js_max(q.ms, ms);
            }
            // a JavaScript Map keeps a replaced key where it was, as IndexMap::insert does
            _ => {
                self.qnhs.insert(key, QnhReading { hpa, ms });
            }
        }
    }

    /// The QNH to correct a track's altitudes with at `t`: its own altimeter's (measured, fresh,
    /// near the field), its own runway's, else its field's; NaN without a field.
    pub fn track_qnh(&mut self, tr: &Track, t: f64) -> f64 {
        let Some(field) = &tr.field else {
            return f64::NAN;
        };
        if let Some(m) = &tr.meas {
            let nav_qnh = m.nav_qnh.unwrap_or(f64::NAN);
            if valid_qnh(nav_qnh) && meas_fresh(tr, t) && tr.alt - tr.field_elev < MEAS_QNH_MAX_AGL_FT {
                return nav_qnh;
            }
        }
        if same_airport(tr.qnh_own_field.as_ref(), Some(field)) && valid_qnh(tr.qnh_own) {
            return tr.qnh_own;
        }
        self.field_qnh(field, t)
    }

    /// The track's QNH at `t`; a change from `old_qnh` that moves its altitudes is eased in rather
    /// than snapped (see [`set_offsets`](super::blend::set_offsets)).
    pub fn set_qnh(&mut self, tr: &mut Track, t: f64, old_qnh: f64) {
        tr.qnh = self.track_qnh(tr, t);
        if !tr.on_ground && tr.raw_alt > 0.0 {
            tr.alt = corrected(tr, tr.raw_alt, tr.raw_w);
        }
        let na = tr.ats.len();
        if na == 0 {
            return;
        }
        let raw = tr.aps[na - 1];
        let w = tr.aws[na - 1];
        if (correct_alt(raw, w, tr.qnh) - correct_alt(raw, w, old_qnh)).abs() > 2.0 * LEVEL_FT {
            tr.alt_blend_rate = REFERENCE_BLEND_RATE;
        }
    }
}

impl Motion {
    /// The QNH (hPa) learned for a field from the traffic at `now_ms`: its own, else interpolated
    /// from fields around; NaN if none is known. Airports are told apart by identity (the `Arc`
    /// given to [`Motion::set_airports`]): another `Arc` of the same field is interpolated to,
    /// as the TypeScript's `Map<Airport, …>` did.
    pub fn field_qnh(&mut self, ap: &Arc<Airport>, now_ms: f64) -> f64 {
        self.world.field_qnh(ap, now_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;
    use crate::motion::aircraft::Measured;

    fn airport(iata: &str, lat: f64, lon: f64, alt: i64) -> Arc<Airport> {
        Arc::new(Airport { iata: iata.into(), lat, lon, alt: Some(alt), ..Airport::default() })
    }

    use crate::motion::aircraft::blend::testing::*;

    #[test]
    fn each_runway_tells_its_fields_qnh_which_corrects_the_next_arrival() {
        let l = landings();
        let airports = fixture_airports(&l);
        let learn = |callsign: &str| {
            let mut m = Motion::new();
            m.set_airports(&airports);
            let f = fixture(&l, callsign);
            play(&mut m, &fixture_reports(f), 3_000.0, 10_000.0);
            let at = airports.iter().find(|a| a.iata == f.airport).unwrap().clone();
            (m, at, f.event_ms)
        };
        // METAR: EHAM Q1004, EGLL Q1008, KORD A3014 (1020.7); to within what a runway (not the
        // field's highest point, in 25 ft steps) and a METAR (whole hPa, rounded down) can tell
        let (mut ams, at, t) = learn("KLM76F");
        assert!((ams.field_qnh(&at, t) - 1_004.0).abs() < 2.5, "{}", ams.field_qnh(&at, t));
        let (mut lhr, at, t) = learn("AEE604");
        assert!((lhr.field_qnh(&at, t) - 1_008.0).abs() < 2.5, "{}", lhr.field_qnh(&at, t));
        let (mut ord, at, t) = learn("SKW446W");
        assert!((ord.field_qnh(&at, t) - 1_020.7).abs() < 2.5, "{}", ord.field_qnh(&at, t));

        // the next arrival at Schiphol is drawn at its true height: down when its runway roll begins
        // (the level run at 225 ft from 30 s before the ground flag), not 236 ft up until the flag
        let f = fixture(&l, "KLC20D");
        let frames = play(&mut ams, &fixture_reports(f), 3_000.0, 10_000.0);
        assert!(worst_step(&frames) < MAX_FRAME_FT, "{}", worst_step(&frames));
        let down = first_down(&frames, f.event_ms - 120_000.0);
        assert!(down > f.event_ms - 36_000.0 && down < f.event_ms - 22_000.0, "{}", down - f.event_ms);
        // and the one in Atlanta (reading 250 ft low) no longer lands 15 s short: down with its braking
        let (mut atl, _, _) = learn("DAL1424");
        let g = fixture(&l, "SWA2713");
        let atl_frames = play(&mut atl, &fixture_reports(g), 3_000.0, 10_000.0);
        let atl_down = first_down(&atl_frames, g.event_ms - 120_000.0);
        assert!(atl_down > g.event_ms - 20_000.0 && atl_down < g.event_ms - 8_000.0, "{}", atl_down - g.event_ms);
        // the flare: the last 30 ft take seconds, touching down at a few hundred fpm at most
        let i = atl_frames.iter().position(|fr| fr.drawn >= atl_down).unwrap();
        let j = atl_frames.iter().position(|fr| fr.alt_ft < 30.0 && fr.drawn > g.event_ms - 60_000.0).unwrap();
        assert!(atl_frames[i].drawn - atl_frames[j].drawn > 3_000.0);
        let sink = ((atl_frames[i - 31].alt_ft - atl_frames[i].alt_ft)
            / ((atl_frames[i].drawn - atl_frames[i - 31].drawn) / 1000.0))
            * 60.0;
        assert!(sink < 300.0, "{sink}");
    }

    #[test]
    fn a_pressure_altitude_400_ft_off_at_a_field_whose_qnh_isnt_known_yet_no_jump_down_by_the_ground_flag() {
        for bias in [400.0, -400.0] {
            let mut m = Motion::new();
            m.set_airports(&[fld(), xxx()]);
            let frames = play(&mut m, &arrival(bias, 5.0, 12.0, 1, f64::INFINITY, FIELD_FT), 3_000.0, 10_000.0);
            // the runway's level run tells it 5-10 s after touchdown: eased down, not dropped
            assert!(worst_step(&frames) < 2.0 * MAX_FRAME_FT, "{bias}: {}", worst_step(&frames));
            assert!(frames.iter().all(|f| f.alt_ft >= 0.0));
            // (it can't be told before then: the drawn time is only seconds behind the data)
            let down = first_down(&frames, T0 + 60_000.0);
            assert!(down < T0 + 120_000.0 + 15_000.0, "{bias}: {}", down - T0);
            // never lifted back up once down
            assert!(frames.iter().filter(|f| f.drawn > down).all(|f| f.alt_ft < 1.0));
            // and it told the field's QNH for the next one
            let qnh = m.field_qnh(&fld(), T0 + 200_000.0);
            assert!((qnh - (1013.25 - bias / 27.7)).abs() < 1.0, "{bias}: {qnh}");
        }
    }

    #[test]
    fn no_altitude_on_short_final_a_negative_pressure_altitude_sent_as_0_the_descent_carries_on() {
        let mut m = Motion::new();
        m.set_airports(&[airport_at("FLD", 45.0, 10.0, 0), xxx()]);
        // QNH ~1022 at a field at sea level: the last ~250 ft read below 0, which Flightradar24 reports as 0
        let reports = arrival(-250.0, 5.0, 12.0, 1, f64::INFINITY, 0.0);
        let frames = play(&mut m, &reports, 3_000.0, 10_000.0);
        assert!(worst_step(&frames) < MAX_FRAME_FT, "{}", worst_step(&frames));
        let down = first_down(&frames, T0 + 60_000.0);
        // it reads 250 ft low and the QNH isn't known: it gets down early, but gets down, gently
        assert!(down < T0 + 120_000.0 && down > T0 + 85_000.0, "{}", down - T0);
        assert!(frames.iter().filter(|f| f.drawn > down).all(|f| f.alt_ft < 1.0));
    }

    #[test]
    fn readings_replace_smooth_and_expire() {
        let mut w = World::default();
        let a = airport("AAA", 45.0, 10.0, 0);
        assert!(w.field_qnh(&a, 0.0).is_nan());
        // nonsense is ignored
        w.learn_qnh(&a, 900.0, 0.0);
        w.learn_qnh(&a, f64::NAN, 0.0);
        assert!(w.qnhs.is_empty());
        assert_eq!(w.qnh_version, 0);
        w.learn_qnh(&a, 1_000.0, 0.0);
        assert_eq!(w.field_qnh(&a, 1_000.0), 1_000.0);
        // smoothed 0.3, an outlier 0.05; the time only moves on
        w.learn_qnh(&a, 1_002.0, 5_000.0);
        assert_close!(w.field_qnh(&a, 5_000.0), 1_000.6, 9);
        w.learn_qnh(&a, 1_020.0, 1_000.0);
        assert_close!(w.qnhs[&AirportKey(a.clone())].hpa, 1_000.6 + 0.05 * 19.4, 9);
        assert_eq!(w.qnhs[&AirportKey(a.clone())].ms, 5_000.0);
        // three hours on, it is forgotten and the next reading replaces it
        assert!(w.field_qnh(&a, 5_000.0 + QNH_TTL_MS + 1.0).is_nan());
        w.learn_qnh(&a, 1_010.0, 5_000.0 + QNH_TTL_MS + 1.0);
        assert_eq!(w.field_qnh(&a, 5_000.0 + QNH_TTL_MS + 1.0), 1_010.0);
        assert_eq!(w.qnh_version, 4);
    }

    #[test]
    fn a_field_without_a_reading_takes_its_neighbours() {
        let mut w = World::default();
        let a = airport("AAA", 45.0, 10.0, 0);
        let b = airport("BBB", 45.0, 11.0, 0);
        let far = airport("FAR", 50.0, 20.0, 0);
        let mid = airport("MID", 45.0, 10.5, 0);
        w.learn_qnh(&a, 1_000.0, 0.0);
        w.learn_qnh(&b, 1_010.0, 0.0);
        w.learn_qnh(&far, 1_040.0, 0.0);
        // half way between: the plain mean (the far one is past 300 km)
        assert_close!(w.field_qnh(&mid, 0.0), 1_005.0, 9);
        // weighted by 1 / (km + 20)²
        let near_a = airport("NRA", 45.0, 10.1, 0);
        let (ka, kb) = (distance_km(45.0, 10.1, 45.0, 10.0), distance_km(45.0, 10.1, 45.0, 11.0));
        let (wa, wb) = (1.0 / ((ka + 20.0) * (ka + 20.0)), 1.0 / ((kb + 20.0) * (kb + 20.0)));
        assert_eq!(w.field_qnh(&near_a, 0.0), (wa * 1_000.0 + wb * 1_010.0) / (wa + wb));
        // cached until something is learned or a minute passes
        w.qnhs.get_mut(&AirportKey(a.clone())).unwrap().hpa = 990.0;
        assert_close!(w.field_qnh(&mid, 59_000.0), 1_005.0, 9);
        assert_close!(w.field_qnh(&mid, 60_000.0), 1_000.0, 9);
        w.learn_qnh(&b, 1_010.0, 60_000.0);
        assert_close!(w.field_qnh(&mid, 60_000.0), 1_000.0, 9);
        w.learn_qnh(&far, 1_040.0, 60_000.0);
        // another Arc of a field with a reading is not that field: it is interpolated to
        let twin = airport("AAA", 45.0, 10.0, 0);
        let kb = distance_km(45.0, 10.0, 45.0, 11.0);
        let wb = 1.0 / ((kb + 20.0) * (kb + 20.0));
        assert_close!(w.field_qnh(&twin, 60_000.0), (990.0 / 400.0 + 1_010.0 * wb) / (1.0 / 400.0 + wb), 9);
    }

    #[test]
    fn a_track_corrects_with_its_altimeter_then_its_runway_then_its_field() {
        let mut w = World::default();
        let field = airport("FLD", 45.0, 10.0, 1_000);
        let other = airport("OTH", 45.1, 10.0, 0);
        let mut tr = Track::new(1);
        assert!(w.track_qnh(&tr, 0.0).is_nan());
        tr.field = Some(field.clone());
        tr.field_elev = 1_000.0;
        tr.alt = 2_000.0;
        assert!(w.track_qnh(&tr, 0.0).is_nan());
        w.learn_qnh(&field, 1_000.0, 0.0);
        assert_eq!(w.track_qnh(&tr, 0.0), 1_000.0);
        // its own runway's reading, at this field only
        tr.qnh_own = 1_005.0;
        tr.qnh_own_field = Some(other);
        assert_eq!(w.track_qnh(&tr, 0.0), 1_000.0);
        tr.qnh_own_field = Some(field.clone());
        assert_eq!(w.track_qnh(&tr, 0.0), 1_005.0);
        // the altimeter setting on board, while fresh and below 3000 ft above the field
        tr.meas = Some(Measured { nav_qnh: Some(1_007.0), ..Measured::default() });
        tr.meas_ms = 0.0;
        assert_eq!(w.track_qnh(&tr, 10_000.0), 1_007.0);
        assert_eq!(w.track_qnh(&tr, 16_000.0), 1_005.0);
        tr.alt = 4_000.0;
        assert_eq!(w.track_qnh(&tr, 10_000.0), 1_005.0);
    }

    #[test]
    fn a_qnh_change_that_moves_the_altitude_is_eased() {
        let mut w = World::default();
        let field = airport("FLD", 45.0, 10.0, 0);
        let mut tr = Track::new(1);
        tr.field = Some(field.clone());
        tr.raw_alt = 2_000.0;
        tr.raw_w = 1.0;
        tr.alt = 2_000.0;
        tr.ats = vec![0.0];
        tr.aps = vec![2_000.0];
        tr.aws = vec![1.0];
        // nothing known: nothing changes
        w.set_qnh(&mut tr, 0.0, f64::NAN);
        assert!(tr.qnh.is_nan() && tr.alt_blend_rate.is_nan());
        assert_eq!(tr.alt, 2_000.0);
        // 1 hPa (~27 ft): within the step, not eased
        w.learn_qnh(&field, 1_014.25, 0.0);
        w.set_qnh(&mut tr, 0.0, f64::NAN);
        assert_eq!(tr.qnh, 1_014.25);
        assert_eq!(tr.alt, correct_alt(2_000.0, 1.0, 1_014.25));
        assert!(tr.alt_blend_rate.is_nan());
        // 10 hPa (~270 ft): eased at REFERENCE_BLEND_RATE
        w.qnhs.get_mut(&AirportKey(field.clone())).unwrap().hpa = 1_004.25;
        w.set_qnh(&mut tr, 0.0, 1_014.25);
        assert_eq!(tr.alt_blend_rate, REFERENCE_BLEND_RATE);
        // on the ground the altitude stays as it is
        tr.on_ground = true;
        tr.alt = 0.0;
        w.set_qnh(&mut tr, 0.0, 1_014.25);
        assert_eq!(tr.alt, 0.0);
    }
}
