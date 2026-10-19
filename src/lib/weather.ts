// The weather the realistic map is drawn in: live, where the map is looking
// (MET Norway's forecast for now, through the app's backend), or one chosen.

import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "./api";

/** The weather now at a place (MET Norway, via `weather_at`). */
export type Weather = {
  /** How much of the sky is cloud, and of it low cloud, 0 to 1. */
  cloud: number;
  cloudLow: number;
  /** Fog over the place, 0 to 1. */
  fog: number;
  /** Rain or snow expected in the next hour, mm. */
  precipitation: number;
  /** MET's symbol for the next hour: `clearsky_day`, `partlycloudy_night`, `rain`, `snow`, `fog`, … */
  symbol: string;
  /** °C. */
  temperature: number;
};

/** The realistic map's weather setting: live, or one chosen. */
export type Sky = "live" | "sunny" | "cloudy" | "rain" | "fog" | "snow";
export const SKIES: readonly Sky[] = ["live", "sunny", "cloudy", "rain", "fog", "snow"];

/** What the weather does to the light, each 0 to 1 but the haze. */
export type Conditions = {
  /** How much of the sun gets through. */
  sun: number;
  /** How grey the sky is. */
  cloud: number;
  /** Extinction of the air, per metre (aerial perspective): a clear day's is about 2e-5. */
  haze: number;
  /** Wet surfaces: darker, and reflecting the sky. */
  wet: number;
  /** Snow lying on the ground and the roofs. */
  snow: number;
  /** Shadows of clouds drifting over the ground, in broken cloud. */
  cloudShadows: number;
  /** Rain falling, 0 to 1: a mist in the distance, and drops in front of the camera. */
  rain: number;
};

export const PRESETS: Record<Exclude<Sky, "live">, Conditions> = {
  sunny: { sun: 1, cloud: 0.05, haze: 2e-5, wet: 0, snow: 0, cloudShadows: 0, rain: 0 },
  cloudy: { sun: 0, cloud: 1, haze: 7e-5, wet: 0, snow: 0, cloudShadows: 0, rain: 0 },
  rain: { sun: 0, cloud: 1, haze: 1.8e-4, wet: 1, snow: 0, cloudShadows: 0, rain: 1 },
  fog: { sun: 0.04, cloud: 1, haze: 4.5e-3, wet: 0.35, snow: 0, cloudShadows: 0, rain: 0 },
  snow: { sun: 0, cloud: 1, haze: 1.4e-4, wet: 0, snow: 1, cloudShadows: 0, rain: 0 },
};

const clamp01 = (x: number) => Math.min(1, Math.max(0, x));

/** Between two weathers: `a` at 0, `b` at 1. */
export function mixConditions(a: Conditions, b: Conditions, t: number): Conditions {
  const m = (k: keyof Conditions) => a[k] + (b[k] - a[k]) * clamp01(t);
  return { sun: m("sun"), cloud: m("cloud"), haze: m("haze"), wet: m("wet"), snow: m("snow"), cloudShadows: m("cloudShadows"), rain: m("rain") };
}

/** The light's conditions in some weather. */
export function conditionsOf(w: Weather): Conditions {
  const symbol = w.symbol;
  const snowing = /snow|sleet/.test(symbol) || (/rain/.test(symbol) && w.temperature <= 0);
  const raining = !snowing && (/rain|thunder/.test(symbol) || w.precipitation >= 0.1);
  const foggy = Math.max(w.fog, /fog/.test(symbol) ? 0.8 : 0);
  // the sun comes through broken cloud, not through low overcast
  const cover = Math.max(w.cloud * 0.85, w.cloudLow);
  const sun = clamp01((0.92 - cover) / 0.6) * (1 - foggy);
  const rain = clamp01(w.precipitation / 2);
  // clear air about 40 km visibility; cloud, rain and snow thicken it; fog to a few hundred metres
  let haze = 2e-5 + 5e-5 * w.cloud + (raining ? 6e-5 + 1.2e-4 * rain : 0) + (snowing ? 1.2e-4 : 0);
  haze = Math.max(haze, foggy * 4.5e-3);
  return {
    sun,
    cloud: clamp01(Math.max(cover, foggy)),
    haze,
    wet: raining ? 0.55 + 0.45 * rain : foggy * 0.35,
    snow: snowing ? 1 : 0,
    // most in broken cloud, none under a clear or an overcast sky
    cloudShadows: clamp01(4 * sun * (1 - sun)) * clamp01(w.cloud * 2),
    // a drizzle shows, a downpour more
    rain: raining ? 0.35 + 0.65 * rain : 0,
  };
}

/** The weather now at `lat`, `lon`: through the app's backend, or straight from MET Norway in a plain browser. */
export async function fetchWeather(lat: number, lon: number): Promise<Weather> {
  if (isTauri()) return invoke<Weather>("weather_at", { lat, lon });
  const url = `https://api.met.no/weatherapi/locationforecast/2.0/complete?lat=${lat.toFixed(2)}&lon=${lon.toFixed(2)}`;
  const response = await fetch(url);
  if (!response.ok) throw new Error(`MET Norway answered ${response.status}`);
  const body = await response.json();
  const step = body?.properties?.timeseries?.[0]?.data;
  const now = step?.instant?.details ?? {};
  return {
    cloud: clamp01((now.cloud_area_fraction ?? 0) / 100),
    cloudLow: clamp01((now.cloud_area_fraction_low ?? 0) / 100),
    fog: clamp01((now.fog_area_fraction ?? 0) / 100),
    precipitation: Math.max(0, step?.next_1_hours?.details?.precipitation_amount ?? 0),
    symbol: step?.next_1_hours?.summary?.symbol_code ?? "",
    temperature: now.air_temperature ?? 10,
  };
}
