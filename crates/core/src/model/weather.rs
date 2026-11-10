//! The weather DTO of `weather.ts` (S1.5), as `RS:weather.rs` sends it.

use serde::{Deserialize, Serialize};

/// The weather now at a place (MET Norway, via `weather_at`; `weather.ts:8`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Weather {
    /// How much of the sky is cloud, 0 to 1.
    pub cloud: f64,
    /// Of it, low cloud (the kind that hides the sun), 0 to 1.
    pub cloud_low: f64,
    /// Fog over the place, 0 to 1.
    pub fog: f64,
    /// Rain or snow expected in the next hour, mm.
    pub precipitation: f64,
    /// MET's symbol for the next hour: `clearsky_day`, `partlycloudy_night`, `rain`, `snow`, `fog`, …
    pub symbol: String,
    /// The air's temperature, °C.
    pub temperature: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn weather_round_trips() {
        let wire = json!({
            "cloud": 0.82, "cloudLow": 0.4, "fog": 0.0, "precipitation": 1.3,
            "symbol": "rain", "temperature": -2.5,
        });
        let weather: Weather = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(weather.cloud_low, 0.4);
        assert_eq!(weather.symbol, "rain");
        assert_eq!(serde_json::to_value(&weather).unwrap(), wire);
    }
}
