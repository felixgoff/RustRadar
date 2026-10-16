// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { buildingStyle, displayRgb, fromLocal, lookFor, sceneSun, skyCss, transmittance } from "./realistic";
import { areaPaint, lineOf } from "./realistic-ground";
import { localSun } from "./shadow";
import { conditionsOf, PRESETS } from "./weather";

const sunAt = (altDeg: number, azDeg = 180): [number, number, number] => {
  const [alt, az] = [(altDeg * Math.PI) / 180, (azDeg * Math.PI) / 180];
  return [Math.sin(az) * Math.cos(alt), Math.cos(az) * Math.cos(alt), Math.sin(alt)];
};
const sum = (c: readonly number[]) => c[0] + c[1] + c[2];

describe("realistic map: light", () => {
  test("the low sun comes through more air: dimmer and redder", () => {
    const high = transmittance(60, 2e-5);
    const low = transmittance(5, 2e-5);
    expect(sum(low)).toBeLessThan(sum(high));
    expect(low[0] / low[2]).toBeGreaterThan(high[0] / high[2]);
    expect(low[0]).toBeGreaterThan(low[2]);
  });

  test("sun by day in fine weather, none under cloud or at night", () => {
    const sunny = lookFor(PRESETS.sunny, sunAt(40));
    const cloudy = lookFor(PRESETS.cloudy, sunAt(40));
    const night = lookFor(PRESETS.sunny, sunAt(-20));
    expect(Math.max(...sunny.sunColor)).toBeGreaterThan(2);
    expect(Math.max(...cloudy.sunColor)).toBe(0);
    expect(Math.max(...night.sunColor)).toBe(0);
    // a blue sky overhead; a grey one under cloud; a dark one at night, its windows lit
    expect(sunny.skyZenith[2]).toBeGreaterThan(sunny.skyZenith[0] * 3);
    expect(cloudy.skyZenith[2] / cloudy.skyZenith[0]).toBeLessThan(1.2);
    expect(sum(night.skyZenith)).toBeLessThan(0.05);
    expect(night.night).toBe(1);
    expect(sunny.night).toBe(0);
  });

  test("clear air leaves the near distance clear; fog closes in beyond the middle of the view", () => {
    // what is left of something `metres` from the camera
    const seen = (l: { hazeDensity: number; hazeStart: number }, metres: number) =>
      Math.exp(-Math.max(metres - l.hazeStart, 0) * l.hazeDensity);
    // the camera 600 m out, as close in over a town
    const clear = lookFor(PRESETS.sunny, sunAt(40), 600);
    const fog = lookFor(PRESETS.fog, sunAt(40), 600);
    expect(seen(clear, 3000)).toBeGreaterThan(0.9);
    expect(seen(fog, 300)).toBe(1);
    expect(seen(fog, 600)).toBeGreaterThan(0.5);
    expect(seen(fog, 2400)).toBeLessThan(0.1);
    // far out, the map is still there to read, whatever the weather
    for (const preset of Object.values(PRESETS)) expect(seen(lookFor(preset, sunAt(40), 50_000), 50_000)).toBeGreaterThan(0.4);
  });

  test("a sunny day is taken darker than a grey one, so it isn't washed out; the night brighter", () => {
    const sunny = lookFor(PRESETS.sunny, sunAt(40)).exposure;
    const cloudy = lookFor(PRESETS.cloudy, sunAt(40)).exposure;
    expect(sunny).toBeLessThan(cloudy * 0.8);
    expect(sunny).toBeGreaterThanOrEqual(0.5);
    expect(lookFor(PRESETS.sunny, sunAt(-20)).exposure).toBeGreaterThan(cloudy * 2);
  });

  test("the screen's colours come through the same curve as the shaders'", () => {
    expect(displayRgb([0, 0, 0], 1)).toEqual([0, 0, 0]);
    const [dim, bright] = [displayRgb([0.2, 0.2, 0.2], 1)[0], displayRgb([2, 2, 2], 1)[0]];
    expect(bright).toBeGreaterThan(dim);
    expect(bright).toBeLessThanOrEqual(255);
  });

  test("the sky behind the map: its horizon where the tilt puts it, the sun's glow when in view", () => {
    const l = lookFor(PRESETS.sunny, sunAt(30));
    expect(skyCss(l, 3, 0.25, null, 0)).toContain(" 25%");
    expect(skyCss(l, 3, -0.5, null, 0)).toContain(" 0%");
    expect(skyCss(l, 3, 0.25, [400, 120], 0)).toContain("radial-gradient(circle at 400px 120px");
    // under cloud, no sun to glow
    expect(skyCss(lookFor(PRESETS.cloudy, sunAt(30)), 3, 0.25, [400, 120], 0)).not.toContain("radial-gradient");
  });

  test("without day and night, a fine afternoon; a local direction turns into the Earth's and back", () => {
    const sun = sceneSun(false, [0, 0, -1]);
    expect(sun[2]).toBeGreaterThan(0.5);
    expect(sceneSun(true, [0.1, 0.2, 0.3])).toEqual([0.1, 0.2, 0.3]);
    const back = localSun(fromLocal(sun, 55.6, 13), 55.6, 13);
    for (let i = 0; i < 3; i++) expect(back[i]).toBeCloseTo(sun[i], 9);
  });
});

describe("realistic map: buildings", () => {
  const facadeOf = (style: number[]) => (255 - style[3]) % 5;
  const roofOf = (style: number[]) => Math.floor((255 - style[3]) / 5);

  test("each building its own façade and roof, the same each time", () => {
    expect(buildingStyle(12345, 8)).toEqual(buildingStyle(12345, 8));
    const styles = Array.from({ length: 400 }, (_, i) => buildingStyle(i * 7919, 8));
    const facades = new Set(styles.map(facadeOf));
    expect(facades.size).toBeGreaterThan(3);
    for (const s of styles) {
      expect(facadeOf(s)).toBeLessThan(4);
      expect(roofOf(s)).toBeLessThan(7);
    }
    // low houses under pitched-roof materials; towers often glass
    const towers = Array.from({ length: 400 }, (_, i) => buildingStyle(i * 7919, 60));
    expect(towers.filter((s) => facadeOf(s) === 4).length).toBeGreaterThan(100);
  });

  test("a mapped colour is kept, toned down", () => {
    expect(buildingStyle(1, 8, "#ffffff").slice(0, 3)).toEqual([241, 241, 241]);
    expect(buildingStyle(1, 8, "white").slice(0, 3)).toEqual(buildingStyle(2, 8, "white").slice(0, 3));
    expect(buildingStyle(5, 8, "brick-ish")).toEqual(buildingStyle(5, 8));
  });
});

describe("realistic map: weather", () => {
  const weather = (over: Partial<Parameters<typeof conditionsOf>[0]>) =>
    conditionsOf({ cloud: 0, cloudLow: 0, fog: 0, precipitation: 0, symbol: "clearsky_day", temperature: 12, ...over });

  test("clear sky: sun; overcast: none; broken cloud: some, and the clouds' shadows", () => {
    expect(weather({}).sun).toBe(1);
    expect(weather({ cloud: 0.995, cloudLow: 0.95, symbol: "cloudy" }).sun).toBe(0);
    const broken = weather({ cloud: 0.55, cloudLow: 0.3, symbol: "partlycloudy_day" });
    expect(broken.sun).toBeGreaterThan(0.2);
    expect(broken.sun).toBeLessThan(0.9);
    expect(broken.cloudShadows).toBeGreaterThan(0.5);
  });

  test("rain wets, fog closes in, snow lies", () => {
    expect(weather({ cloud: 1, precipitation: 1.5, symbol: "rain" }).wet).toBeGreaterThan(0.7);
    expect(weather({ cloud: 1, fog: 0.9, symbol: "fog" }).haze).toBeGreaterThan(1e-3);
    expect(weather({ cloud: 1, precipitation: 0.5, symbol: "snow", temperature: -2 }).snow).toBe(1);
    // rain below freezing is snow
    expect(weather({ cloud: 1, precipitation: 0.5, symbol: "rain", temperature: -1 }).snow).toBe(1);
  });
});

describe("realistic map: the ground", () => {
  const f = (properties: Record<string, unknown>) => ({ properties });
  // the surface is carried in the alpha: 255 less its id
  const surface = (rgba: number[]) => 255 - rgba[3];

  test("each kind of area has its own surface; names of places none", () => {
    expect(surface(areaPaint(f({ layerName: "water", class: "lake" })))).toBe(11);
    expect(surface(areaPaint(f({ layerName: "landcover", class: "wood", subclass: "forest" })))).toBe(2);
    expect(surface(areaPaint(f({ layerName: "landcover", class: "farmland", subclass: "farmland" })))).toBe(3);
    expect(surface(areaPaint(f({ layerName: "landuse", class: "railway" })))).toBe(12);
    expect(areaPaint(f({ layerName: "landuse", class: "suburb" }))[3]).toBe(0);
    expect(areaPaint(f({ layerName: "building" }))[3]).toBe(0);
  });

  test("roads at their widths with pavements, railways on ballast, tunnels not at all", () => {
    const motorway = lineOf(f({ layerName: "transportation", class: "motorway" }));
    const minor = lineOf(f({ layerName: "transportation", class: "minor" }));
    expect(motorway.width).toBeGreaterThan(minor.width);
    expect(minor.edge).toBeGreaterThan(0);
    expect(surface(motorway.paint)).toBe(32);
    expect(surface(minor.paint)).toBe(33);
    const rail = lineOf(f({ layerName: "transportation", class: "rail", subclass: "rail" }));
    expect(surface(rail.paint)).toBe(35);
    expect(surface(rail.edgePaint)).toBe(38);
    expect(lineOf(f({ layerName: "transportation", class: "rail", brunnel: "tunnel" })).width).toBe(0);
    expect(lineOf(f({ layerName: "transportation", class: "ferry" })).width).toBe(0);
    expect(lineOf(f({ layerName: "transportation", class: "path", subclass: "footway" })).width).toBe(2);
    expect(surface(lineOf(f({ layerName: "waterway", class: "river" })).paint)).toBe(11);
  });
});
