//! `api.ts` ADS-B DTOs (S1.5): adsb.lol aircraft and OpenSky track points, as `RS:adsblol.rs`
//! and `RS:opensky.rs` send them.

use serde::{Deserialize, Serialize};

/// Where the adsb.lol credit links to (`api.ts:578`).
pub const ADSB_LOL_URL: &str = "https://adsb.lol";

/// One aircraft as adsb.lol reports it: what its transponder broadcasts, including what
/// Flightradar24's anonymous feed leaves out (`api.ts:512`). ODbL: credit "adsb.lol
/// contributors" wherever it is shown. Absent values are left out of the JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdsbAircraft {
    /// ICAO 24-bit address, lowercase hex (`~`-prefixed for non-ICAO addresses).
    pub hex: String,
    /// Callsign, trimmed.
    pub callsign: String,
    pub reg: String,
    pub typecode: String,
    /// How it was received: `adsb_icao`, `mlat`, `tisb_icao`, ...
    pub source: String,
    /// The position is multilaterated.
    pub mlat: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    /// Unix ms of the position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_ms: Option<u64>,
    /// Unix ms of the latest message of any kind.
    pub seen_ms: u64,
    pub on_ground: bool,
    /// Barometric altitude, feet (absent on the ground).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_baro: Option<i32>,
    /// Geometric (GNSS) altitude, feet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_geom: Option<i32>,
    /// Ground speed, knots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gs: Option<f32>,
    /// Direction of motion over the ground, degrees true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<f32>,
    /// Where the nose points, degrees true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub true_heading: Option<f32>,
    /// Where the nose points, degrees magnetic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mag_heading: Option<f32>,
    /// Bank, degrees, right wing down positive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roll: Option<f32>,
    /// Degrees per second, clockwise positive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_rate: Option<f32>,
    /// Feet per minute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baro_rate: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geom_rate: Option<i32>,
    /// Indicated and true airspeed, knots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ias: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tas: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mach: Option<f32>,
    /// Where the wind blows from, degrees true, and its speed, knots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_dir: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_speed: Option<i32>,
    /// Outside and total air temperature, °C.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oat: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tat: Option<i32>,
    /// Altitude selected on the autopilot's control panel (MCP/FCU), feet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav_altitude_mcp: Option<i32>,
    /// Altitude selected by the flight management system, feet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav_altitude_fms: Option<i32>,
    /// Heading selected on the autopilot, degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav_heading: Option<f32>,
    /// Altimeter setting, hPa.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav_qnh: Option<f32>,
    /// Engaged autopilot modes, e.g. `autopilot`, `vnav`, `lnav`, `tcas`; left out when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nav_modes: Vec<String>,
    /// Left out when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub squawk: String,
    /// `none` is left out.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub emergency: String,
    /// ADS-B emitter category, e.g. `A3`; left out when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
}

/// One answer of adsb.lol (`api.ts:566`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdsbSnapshot {
    /// adsb.lol's clock when it answered, Unix ms.
    pub now_ms: u64,
    pub aircraft: Vec<AdsbAircraft>,
}

/// One position of a flight tracked by the OpenSky Network (`api.ts:493`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenSkyPoint {
    /// Unix seconds
    pub timestamp: i64,
    pub latitude: f32,
    pub longitude: f32,
    /// Barometric altitude, feet
    pub altitude: i32,
    /// Degrees clockwise from north
    pub track: u32,
    pub on_ground: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn adsb_snapshot_round_trips() {
        // the first aircraft of `adsblol-frankfurt.json` as the backend passes it on
        let wire = json!({
            "nowMs": 1_791_000_000_500u64,
            "aircraft": [{
                "hex": "89617e", "callsign": "UAE1CL", "reg": "A6-EGF", "typecode": "B77W", "source": "adsb_icao",
                "mlat": false, "lat": 50.25612, "lon": 7.594299, "positionMs": 1_791_000_000_500u64,
                "seenMs": 1_791_000_000_500u64, "onGround": false, "altBaro": 35000, "altGeom": 35875,
                "gs": 548.0, "track": 108.0, "trueHeading": 110.5, "magHeading": 106.75, "roll": 0.0,
                "baroRate": 0, "geomRate": -32, "ias": 290, "tas": 490, "mach": 0.75, "windDir": 270,
                "windSpeed": 61, "oat": -53, "tat": -22, "navAltitudeMcp": 35008, "navQnh": 1012.75,
                "navModes": ["autopilot", "vnav"], "squawk": "4612", "category": "A5",
            }],
        });
        let snapshot: AdsbSnapshot = serde_json::from_value(wire.clone()).unwrap();
        let a = &snapshot.aircraft[0];
        assert_eq!((a.alt_baro, a.nav_altitude_mcp, a.nav_altitude_fms), (Some(35000), Some(35008), None));
        assert_eq!(a.emergency, "");
        assert_eq!(a.track_rate, None);
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), wire);
    }

    #[test]
    fn adsb_aircraft_leaves_out_what_is_absent() {
        let wire = json!({
            "hex": "~2a1b3c", "callsign": "", "reg": "", "typecode": "", "source": "mlat", "mlat": true,
            "seenMs": 5, "onGround": true,
        });
        let aircraft: AdsbAircraft = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(
            aircraft,
            AdsbAircraft { hex: "~2a1b3c".to_owned(), source: "mlat".to_owned(), mlat: true, seen_ms: 5, on_ground: true, ..Default::default() }
        );
        assert_eq!(serde_json::to_value(&aircraft).unwrap(), wire);
    }

    #[test]
    fn open_sky_point_round_trips() {
        let wire = json!({"timestamp": 1_700_000_000, "latitude": 50.5, "longitude": 8.25, "altitude": 11_000, "track": 93, "on_ground": false});
        let point: OpenSkyPoint = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(point.track, 93);
        assert_eq!(serde_json::to_value(point).unwrap(), wire);
    }

    #[test]
    fn adsb_lol_url() {
        assert_eq!(ADSB_LOL_URL, "https://adsb.lol");
    }
}
