//! Port of `adsb.ts` (doc 05 §11.1): joining adsb.lol's aircraft to Flightradar24's flights.
//! Flightradar24's anonymous feed carries no Mode S address, so the selected flight is matched by
//! the address from its details when known, else by callsign or registration, checked against its
//! position.

use crate::model::AdsbAircraft;

/// Further apart than this, a callsign or registration match is some other aircraft.
const MAX_MATCH_KM: f64 = 150.0;

/// `s.toUpperCase().replace(/[^A-Z0-9]/g, "")`.
fn normalise(s: &str) -> String {
    s.to_uppercase().chars().filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit()).collect()
}

/// Equirectangular distance, km.
fn km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let rad = std::f64::consts::PI / 180.0;
    let x = (((lon2 - lon1 + 540.0) % 360.0) - 180.0) * (((lat1 + lat2) / 2.0) * rad).cos();
    x.hypot(lat2 - lat1) * 111.195
}

/// What [`match_adsb`] knows of the flight (`adsb.ts` `FlightKey`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlightKey<'a> {
    /// Mode S address, six hex digits, if known.
    pub hex: Option<&'a str>,
    pub callsign: &'a str,
    pub reg: &'a str,
    pub lat: f64,
    pub lon: f64,
}

/// The aircraft in `list` that is this flight, or `None`.
pub fn match_adsb<'l>(list: &'l [AdsbAircraft], key: &FlightKey) -> Option<&'l AdsbAircraft> {
    // `key.hex?.toLowerCase()`, then `if (hex)`: an empty address is no address
    if let Some(hex) = key.hex.map(str::to_lowercase).filter(|hex| !hex.is_empty()) {
        return list.iter().find(|a| a.hex == hex);
    }
    let callsign = normalise(key.callsign);
    let reg = normalise(key.reg);
    let mut best = None;
    let mut best_km = MAX_MATCH_KM;
    for a in list {
        let same_callsign = !callsign.is_empty() && normalise(&a.callsign) == callsign;
        let other_reg = !reg.is_empty() && !a.reg.is_empty() && normalise(&a.reg) != reg;
        let same_reg = !reg.is_empty() && normalise(&a.reg) == reg;
        if !(same_callsign && !other_reg) && !same_reg {
            continue;
        }
        // a match must also be where the flight is
        let (Some(lat), Some(lon)) = (a.lat, a.lon) else { continue };
        if !key.lat.is_finite() || !key.lon.is_finite() {
            continue;
        }
        let d = km(key.lat, key.lon, lat, lon);
        if d < best_km {
            best_km = d;
            best = Some(a);
        }
    }
    best
}

/// Feet per minute: barometric when reported, else geometric.
pub fn vertical_rate(a: &AdsbAircraft) -> Option<i32> {
    a.baro_rate.or(a.geom_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ac(over: impl FnOnce(&mut AdsbAircraft)) -> AdsbAircraft {
        let mut a = AdsbAircraft {
            hex: "89617e".to_owned(),
            callsign: "UAE1CL".to_owned(),
            reg: "A6-EGF".to_owned(),
            typecode: "B77W".to_owned(),
            source: "adsb_icao".to_owned(),
            mlat: false,
            lat: Some(50.25),
            lon: Some(7.59),
            seen_ms: 0,
            on_ground: false,
            ..Default::default()
        };
        over(&mut a);
        a
    }

    fn hex_of(found: Option<&AdsbAircraft>) -> Option<&str> {
        found.map(|a| a.hex.as_str())
    }

    #[test]
    fn matches_by_address_first() {
        let list = [
            ac(|_| {}),
            ac(|a| {
                a.hex = "3c6444".to_owned();
                a.callsign = "DLH1".to_owned();
            }),
        ];
        let key = FlightKey { hex: Some("3C6444"), callsign: "UAE1CL", reg: "", lat: 0.0, lon: 0.0 };
        assert_eq!(hex_of(match_adsb(&list, &key)), Some("3c6444"));
        let key = FlightKey { hex: Some("abcdef"), callsign: "UAE1CL", reg: "", lat: 50.0, lon: 7.0 };
        assert_eq!(match_adsb(&list, &key), None);
    }

    #[test]
    fn without_an_address_by_callsign_or_registration_near_the_flight() {
        let key = FlightKey { hex: None, callsign: "uae1cl ", reg: "", lat: 50.3, lon: 7.6 };
        assert_eq!(hex_of(match_adsb(&[ac(|_| {})], &key)), Some("89617e"));
        // the same callsign far away is another flight
        let far = ac(|a| {
            a.lat = Some(25.0);
            a.lon = Some(55.0);
        });
        assert_eq!(match_adsb(&[far], &key), None);
        // a different registration under the callsign is another aircraft
        assert_eq!(match_adsb(&[ac(|_| {})], &FlightKey { reg: "A6-EGG", ..key }), None);
        // registration alone will do
        let no_callsign = ac(|a| a.callsign = String::new());
        assert_eq!(hex_of(match_adsb(&[no_callsign], &FlightKey { callsign: "", reg: "A6EGF", ..key })), Some("89617e"));
        // the nearer of two candidates
        let near = ac(|a| {
            a.hex = "000001".to_owned();
            a.lat = Some(50.31);
            a.lon = Some(7.6);
        });
        assert_eq!(hex_of(match_adsb(&[ac(|_| {}), near], &key)), Some("000001"));
    }

    // Beyond the TypeScript tests.

    #[test]
    fn an_empty_address_falls_back_to_callsign_and_positions_must_be_known() {
        let key = FlightKey { hex: Some(""), callsign: "UAE1CL", reg: "", lat: 50.3, lon: 7.6 };
        assert_eq!(hex_of(match_adsb(&[ac(|_| {})], &key)), Some("89617e"));
        let unplaced = ac(|a| a.lat = None);
        assert_eq!(match_adsb(&[unplaced], &key), None);
        assert_eq!(match_adsb(&[ac(|_| {})], &FlightKey { lat: f64::NAN, ..key }), None);
        // of two equally near, the first
        let twin = ac(|a| a.hex = "000002".to_owned());
        assert_eq!(hex_of(match_adsb(&[ac(|_| {}), twin], &key)), Some("89617e"));
        // across the antimeridian the distance is short
        let east = ac(|a| {
            a.lat = Some(0.0);
            a.lon = Some(179.9);
        });
        let west = FlightKey { hex: None, callsign: "UAE1CL", reg: "", lat: 0.0, lon: -179.9 };
        assert_eq!(hex_of(match_adsb(&[east], &west)), Some("89617e"));
    }

    #[test]
    fn vertical_rate_prefers_the_barometric() {
        assert_eq!(vertical_rate(&ac(|a| a.geom_rate = Some(-32))), Some(-32));
        assert_eq!(
            vertical_rate(&ac(|a| {
                a.baro_rate = Some(0);
                a.geom_rate = Some(-32);
            })),
            Some(0)
        );
        assert_eq!(vertical_rate(&ac(|_| {})), None);
    }
}
