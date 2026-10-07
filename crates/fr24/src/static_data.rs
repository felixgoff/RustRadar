//! Reference data: airlines, airports, aircraft families and countries.
//!
//! Port of upstream `fr24/static`. These lists change rarely; callers should
//! cache them rather than fetching on every start.

use serde::{Deserialize, Deserializer, Serialize};

/// A versioned list, as returned by the `/mobile/*` endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rows<T> {
    /// A number for airlines, a string for airports.
    #[serde(default)]
    pub version: serde_json::Value,
    pub rows: Vec<T>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Airline {
    #[serde(rename = "Name")]
    pub name: String,
    /// IATA code, often empty.
    #[serde(rename = "Code", default)]
    pub iata: String,
    #[serde(rename = "ICAO", default)]
    pub icao: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timezone {
    /// IANA name, e.g. `Europe/London`.
    pub name: String,
    /// UTC offset in seconds, at the time of the request.
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub abbr: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Airport {
    /// Flightradar24's airport id, as used by `ScheduleInfo::origin_id`.
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub iata: String,
    #[serde(default)]
    pub icao: String,
    #[serde(default)]
    pub city: String,
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub country: String,
    /// Elevation, feet. Upstream sends `"-1"` when unknown.
    #[serde(default, deserialize_with = "lenient_i64")]
    pub alt: Option<i64>,
    /// A relative size/importance score.
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub timezone: Option<Timezone>,
    #[serde(default, rename = "countryId")]
    pub country_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AircraftModel {
    #[serde(rename = "Name")]
    pub name: String,
    /// ICAO type designator, e.g. `A20N`.
    #[serde(rename = "Code")]
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AircraftFamily {
    pub description: String,
    pub models: Vec<AircraftModel>,
}

fn lenient_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(match value {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
    .filter(|v| *v >= 0))
}

pub(crate) const AIRLINES_URL: &str = "https://www.flightradar24.com/mobile/airlines";
pub(crate) const AIRPORTS_URL: &str = "https://www.flightradar24.com/mobile/airports/format/4";
pub(crate) const AIRCRAFT_FAMILY_URL: &str = "https://www.flightradar24.com/mobile/aircraft-family";
pub(crate) const COUNTRIES_URL: &str = "https://www.flightradar24.com/mobile/countries";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airports() {
        let json = r#"{"version":"1","rows":[
            {"id":1900,"name":"A Coruna Airport","iata":"LCG","icao":"LECO","city":"A Coruna","lat":43.302059,"lon":-8.37725,
             "country":"Spain","alt":326,"size":4387,"timezone":{"name":"Europe/Madrid","offset":7200,"offsetHours":"2:00","abbr":"CEST","abbrName":null,"isDst":true},
             "countryId":209,"videoStream":""},
            {"id":2,"name":"Nowhere","iata":"","icao":"XXXX","city":"","lat":0,"lon":0,"country":"","alt":"-1","size":0}
        ]}"#;
        let rows: Rows<Airport> = serde_json::from_str(json).unwrap();
        assert_eq!(rows.rows[0].iata, "LCG");
        assert_eq!(rows.rows[0].alt, Some(326));
        assert_eq!(
            rows.rows[0].timezone.as_ref().unwrap().name,
            "Europe/Madrid"
        );
        assert_eq!(rows.rows[1].alt, None);
        assert!(rows.rows[1].timezone.is_none());
    }

    #[test]
    fn airlines() {
        let json = r#"{"version":1791181722,"rows":[{"Name":"21 Air","Code":"2I","ICAO":"CSB"},{"Name":"X","Code":"","ICAO":"XXX"}]}"#;
        let rows: Rows<Airline> = serde_json::from_str(json).unwrap();
        assert_eq!(
            rows.rows[0],
            Airline {
                name: "21 Air".into(),
                iata: "2I".into(),
                icao: "CSB".into()
            }
        );
    }
}
