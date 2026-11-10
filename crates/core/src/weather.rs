//! Port of `weather.ts` (S1.9): the weather the realistic map is drawn in, as light conditions.
//!
//! The weather itself ([`Weather`], MET Norway's forecast for now) comes from
//! `backend::weather::weather_at`; the TypeScript's browser fallback `fetchWeather` is not ported.

use serde::{Deserialize, Serialize};

pub use crate::model::Weather;
use crate::num::clamp01;

/// The realistic map's weather setting: live where the map is looking, or one chosen
/// (`weather.ts:23`). The settings' default is [`Sky::Live`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sky {
    #[default]
    Live,
    Sunny,
    Cloudy,
    Rain,
    Fog,
    Snow,
}

/// Every setting, in the order of `weather.ts:24` `SKIES`.
pub const SKIES: [Sky; 6] = [Sky::Live, Sky::Sunny, Sky::Cloudy, Sky::Rain, Sky::Fog, Sky::Snow];

impl Sky {
    /// The name in settings and in the TypeScript: `"live"`, `"sunny"`, …
    pub const fn as_str(self) -> &'static str {
        match self {
            Sky::Live => "live",
            Sky::Sunny => "sunny",
            Sky::Cloudy => "cloudy",
            Sky::Rain => "rain",
            Sky::Fog => "fog",
            Sky::Snow => "snow",
        }
    }

    /// The setting a name stands for (`SKIES.includes(name)`); `None` for any other text.
    pub fn from_name(name: &str) -> Option<Sky> {
        SKIES.into_iter().find(|sky| sky.as_str() == name)
    }

    /// The chosen weather's conditions (`PRESETS[sky]`); `None` for [`Sky::Live`].
    pub const fn preset(self) -> Option<Conditions> {
        PRESETS.get(self)
    }
}

/// What the weather does to the light (`weather.ts:27`), each 0 to 1 but the haze.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Conditions {
    /// How much of the sun gets through.
    pub sun: f64,
    /// How grey the sky is.
    pub cloud: f64,
    /// Extinction of the air, per metre (aerial perspective): a clear day's is about 2e-5.
    pub haze: f64,
    /// Wet surfaces: darker, and reflecting the sky.
    pub wet: f64,
    /// Snow lying on the ground and the roofs.
    pub snow: f64,
    /// Shadows of clouds drifting over the ground, in broken cloud.
    pub cloud_shadows: f64,
    /// Rain falling, 0 to 1: a mist in the distance, and drops in front of the camera.
    pub rain: f64,
}

/// The chosen weathers (`weather.ts:44` `PRESETS`, a record keyed by every [`Sky`] but live).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Presets {
    pub sunny: Conditions,
    pub cloudy: Conditions,
    pub rain: Conditions,
    pub fog: Conditions,
    pub snow: Conditions,
}

impl Presets {
    /// `PRESETS[sky]`: `None` for [`Sky::Live`], which has no preset.
    pub const fn get(&self, sky: Sky) -> Option<Conditions> {
        match sky {
            Sky::Live => None,
            Sky::Sunny => Some(self.sunny),
            Sky::Cloudy => Some(self.cloudy),
            Sky::Rain => Some(self.rain),
            Sky::Fog => Some(self.fog),
            Sky::Snow => Some(self.snow),
        }
    }

    /// `Object.values(PRESETS)`: sunny, cloudy, rain, fog, snow.
    pub const fn values(&self) -> [Conditions; 5] {
        [self.sunny, self.cloudy, self.rain, self.fog, self.snow]
    }
}

const fn conditions(sun: f64, cloud: f64, haze: f64, wet: f64, snow: f64, cloud_shadows: f64, rain: f64) -> Conditions {
    Conditions { sun, cloud, haze, wet, snow, cloud_shadows, rain }
}

pub const PRESETS: Presets = Presets {
    sunny: conditions(1.0, 0.05, 2e-5, 0.0, 0.0, 0.0, 0.0),
    cloudy: conditions(0.0, 1.0, 7e-5, 0.0, 0.0, 0.0, 0.0),
    rain: conditions(0.0, 1.0, 1.8e-4, 1.0, 0.0, 0.0, 1.0),
    fog: conditions(0.04, 1.0, 4.5e-3, 0.35, 0.0, 0.0, 0.0),
    snow: conditions(0.0, 1.0, 1.4e-4, 0.0, 1.0, 0.0, 0.0),
};

/// `Math.max(a, b)`: NaN when either is, and +0 over -0, where `f64::max` drops a NaN.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b || (a == b && a.is_sign_positive()) {
        a
    } else {
        b
    }
}

/// Between two weathers: `a` at 0, `b` at 1, `t` clamped to that (`weather.ts:55`).
pub fn mix_conditions(a: Conditions, b: Conditions, t: f64) -> Conditions {
    let m = |a: f64, b: f64| a + (b - a) * clamp01(t);
    Conditions {
        sun: m(a.sun, b.sun),
        cloud: m(a.cloud, b.cloud),
        haze: m(a.haze, b.haze),
        wet: m(a.wet, b.wet),
        snow: m(a.snow, b.snow),
        cloud_shadows: m(a.cloud_shadows, b.cloud_shadows),
        rain: m(a.rain, b.rain),
    }
}

/// The light's conditions in some weather (`weather.ts:61`).
pub fn conditions_of(w: &Weather) -> Conditions {
    let symbol = w.symbol.as_str();
    // the TypeScript's `/snow|sleet/.test(symbol)` and friends: plain substring tests
    let snowing = symbol.contains("snow") || symbol.contains("sleet") || (symbol.contains("rain") && w.temperature <= 0.0);
    let raining = !snowing && (symbol.contains("rain") || symbol.contains("thunder") || w.precipitation >= 0.1);
    let foggy = js_max(w.fog, if symbol.contains("fog") { 0.8 } else { 0.0 });
    // the sun comes through broken cloud, not through low overcast
    let cover = js_max(w.cloud * 0.85, w.cloud_low);
    let sun = clamp01((0.92 - cover) / 0.6) * (1.0 - foggy);
    let rain = clamp01(w.precipitation / 2.0);
    // clear air about 40 km visibility; cloud, rain and snow thicken it; fog to a few hundred metres
    let mut haze = 2e-5
        + 5e-5 * w.cloud
        + (if raining { 6e-5 + 1.2e-4 * rain } else { 0.0 })
        + (if snowing { 1.2e-4 } else { 0.0 });
    haze = js_max(haze, foggy * 4.5e-3);
    Conditions {
        sun,
        cloud: clamp01(js_max(cover, foggy)),
        haze,
        wet: if raining { 0.55 + 0.45 * rain } else { foggy * 0.35 },
        snow: if snowing { 1.0 } else { 0.0 },
        // most in broken cloud, none under a clear or an overcast sky
        cloud_shadows: clamp01(4.0 * sun * (1.0 - sun)) * clamp01(w.cloud * 2.0),
        // a drizzle shows, a downpour more
        rain: if raining { 0.35 + 0.65 * rain } else { 0.0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;

    fn w(symbol: &str, cloud: f64, cloud_low: f64, fog: f64, precipitation: f64, temperature: f64) -> Weather {
        Weather { cloud, cloud_low, fog, precipitation, symbol: symbol.to_owned(), temperature }
    }

    fn fields(c: Conditions) -> [f64; 7] {
        [c.sun, c.cloud, c.haze, c.wet, c.snow, c.cloud_shadows, c.rain]
    }

    #[test]
    fn mix_conditions_goes_from_a_to_b_and_clamps_t() {
        let (sunny, rain) = (PRESETS.sunny, PRESETS.rain);
        assert_eq!(mix_conditions(sunny, rain, 0.0), sunny);
        assert_eq!(mix_conditions(sunny, rain, -1.0), sunny);
        assert_eq!(mix_conditions(sunny, rain, 2.0), mix_conditions(sunny, rain, 1.0));
        let end = mix_conditions(sunny, rain, 1.0);
        for (got, want) in fields(end).into_iter().zip(fields(rain)) {
            assert_close!(got, want, 12);
        }
        let half = mix_conditions(sunny, rain, 0.5);
        assert_eq!(half.sun, 0.5);
        assert_eq!(half.wet, 0.5);
        assert_close!(half.haze, (2e-5 + 1.8e-4) / 2.0, 15);
        // what weather.ts gives, to the bit
        let cases = [
            (mix_conditions(PRESETS.sunny, PRESETS.rain, 0.3), [0.7, 0.33499999999999996, 6.8e-05, 0.3, 0.0, 0.0, 0.3]),
            (mix_conditions(PRESETS.fog, PRESETS.snow, 0.77), [0.0092, 1.0, 0.0011428000000000002, 0.08050000000000002, 0.77, 0.0, 0.0]),
            (mix_conditions(PRESETS.cloudy, PRESETS.sunny, -1.0), [0.0, 1.0, 7e-05, 0.0, 0.0, 0.0, 0.0]),
            (mix_conditions(PRESETS.cloudy, PRESETS.sunny, 2.0), [1.0, 0.050000000000000044, 1.9999999999999998e-05, 0.0, 0.0, 0.0, 0.0]),
        ];
        for (got, want) in cases {
            assert_eq!(fields(got), want);
        }
    }

    #[test]
    fn conditions_of_matches_weather_ts() {
        // conditionsOf over the same inputs, printed by bun from weather.ts
        let cases = [
            (w("clearsky_day", 0.0, 0.0, 0.0, 0.0, 12.0), [1.0, 0.0, 2e-05, 0.0, 0.0, 0.0, 0.0]),
            (w("clearsky_day", 0.55, 0.3, 0.0, 0.05, 3.0), [0.7541666666666668, 0.4675, 4.75e-05, 0.0, 0.0, 0.741597222222222, 0.0]),
            (w("clearsky_day", 0.995, 0.95, 0.2, 0.1, 0.0), [0.0, 0.95, 0.0009, 0.5725, 0.0, 0.0, 0.38249999999999995]),
            (w("clearsky_day", 1.0, 0.1, 0.9, 1.5, -1.0), [0.011666666666666676, 0.9, 0.00405, 0.8875000000000001, 0.0, 0.04612222222222226, 0.8375]),
            (w("clearsky_day", 0.3, 0.7, 0.5, 3.0, 20.0), [0.1833333333333334, 0.7, 0.00225, 1.0, 0.0, 0.35933333333333345, 1.0]),
            (w("fog", 0.0, 0.0, 0.0, 0.0, 12.0), [0.19999999999999996, 0.8, 0.0036, 0.27999999999999997, 0.0, 0.0, 0.0]),
            (w("fog", 0.55, 0.3, 0.0, 0.05, 3.0), [0.15083333333333332, 0.8, 0.0036, 0.27999999999999997, 0.0, 0.5123305555555555, 0.0]),
            (w("fog", 0.3, 0.7, 0.5, 3.0, 20.0), [0.07333333333333335, 0.8, 0.0036, 1.0, 0.0, 0.16309333333333334, 1.0]),
            (w("lightrain", 0.55, 0.3, 0.0, 0.05, 3.0), [0.7541666666666668, 0.4675, 0.0001105, 0.56125, 0.0, 0.741597222222222, 0.36624999999999996]),
            (w("lightrain", 0.995, 0.95, 0.2, 0.1, 0.0), [0.0, 0.95, 0.0009, 0.06999999999999999, 1.0, 0.0, 0.0]),
            (w("lightrain", 1.0, 0.1, 0.9, 1.5, -1.0), [0.011666666666666676, 0.9, 0.00405, 0.315, 1.0, 0.04612222222222226, 0.0]),
            (w("sleet", 0.55, 0.3, 0.0, 0.05, 3.0), [0.7541666666666668, 0.4675, 0.0001675, 0.0, 1.0, 0.741597222222222, 0.0]),
            (w("snow", 0.3, 0.7, 0.5, 3.0, 20.0), [0.1833333333333334, 0.7, 0.00225, 0.175, 1.0, 0.35933333333333345, 0.0]),
            (w("rain", 0.995, 0.95, 0.2, 0.1, 0.0), [0.0, 0.95, 0.0009, 0.06999999999999999, 1.0, 0.0, 0.0]),
            (w("thunder", 0.995, 0.95, 0.2, 0.1, 0.0), [0.0, 0.95, 0.0009, 0.5725, 0.0, 0.0, 0.38249999999999995]),
            (w("thunder", 0.3, 0.7, 0.5, 3.0, 20.0), [0.1833333333333334, 0.7, 0.00225, 1.0, 0.0, 0.35933333333333345, 1.0]),
        ];
        for (weather, want) in cases {
            assert_eq!(fields(conditions_of(&weather)), want, "{weather:?}");
        }
    }

    #[test]
    fn a_missing_reading_stays_unknown_as_in_javascript() {
        // Math.max and Math.min keep a NaN; f64::max would have read it as no fog
        let c = conditions_of(&w("cloudy", 0.5, 0.2, f64::NAN, 0.0, 5.0));
        assert!(c.sun.is_nan() && c.cloud.is_nan() && c.haze.is_nan() && c.wet.is_nan());
        assert_eq!(c.snow, 0.0);
        assert_eq!(c.rain, 0.0);
    }

    #[test]
    fn presets_and_skies_are_the_tables_of_weather_ts() {
        assert_eq!(SKIES.map(Sky::as_str), ["live", "sunny", "cloudy", "rain", "fog", "snow"]);
        for sky in SKIES {
            assert_eq!(Sky::from_name(sky.as_str()), Some(sky));
            assert_eq!(serde_json::to_value(sky).unwrap(), sky.as_str());
            assert_eq!(serde_json::from_value::<Sky>(sky.as_str().into()).unwrap(), sky);
        }
        assert_eq!(Sky::from_name("Sunny"), None);
        assert_eq!(Sky::from_name("storm"), None);
        assert_eq!(Sky::default(), Sky::Live);

        assert_eq!(Sky::Live.preset(), None);
        assert_eq!(Sky::Fog.preset(), Some(PRESETS.fog));
        let values = PRESETS.values();
        let by_sky = [Sky::Sunny, Sky::Cloudy, Sky::Rain, Sky::Fog, Sky::Snow].map(|s| s.preset().unwrap());
        assert_eq!(values, by_sky);
        assert_eq!(fields(PRESETS.sunny), [1.0, 0.05, 2e-5, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(fields(PRESETS.cloudy), [0.0, 1.0, 7e-5, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(fields(PRESETS.rain), [0.0, 1.0, 1.8e-4, 1.0, 0.0, 0.0, 1.0]);
        assert_eq!(fields(PRESETS.fog), [0.04, 1.0, 4.5e-3, 0.35, 0.0, 0.0, 0.0]);
        assert_eq!(fields(PRESETS.snow), [0.0, 1.0, 1.4e-4, 0.0, 1.0, 0.0, 0.0]);
    }
}
