// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { hazeFor, nightness, skyBackground, skyColours, wallColour } from "./realistic";
import { areaPaint, lineOf } from "./realistic-ground";

describe("realistic map: buildings and light", () => {
  test("a building takes its mapped colour, toned down, or one of a city's by its seed", () => {
    expect(wallColour(1, "#ffffff")).toEqual([241, 241, 241]);
    expect(wallColour(1, "white")).toEqual(wallColour(2, "white"));
    expect(wallColour(1, "#b04a3a")).toEqual(wallColour(9, "red"));
    // unmapped: the same seed the same colour, and a city's worth of them over many
    expect(wallColour(12345)).toEqual(wallColour(12345));
    const colours = new Set(Array.from({ length: 200 }, (_, i) => wallColour(i).join(",")));
    expect(colours.size).toBeGreaterThan(20);
    for (let i = 0; i < 200; i++) for (const c of wallColour(i * 7919)) expect(c >= 0 && c <= 255).toBe(true);
    // not a colour at all: one of the city's
    expect(wallColour(5, "brick-ish")).toEqual(wallColour(5));
  });

  test("night falls through dusk", () => {
    expect(nightness(0.5)).toBe(0);
    expect(nightness(0.05)).toBe(0);
    expect(nightness(-0.03)).toBeGreaterThan(0);
    expect(nightness(-0.03)).toBeLessThan(1);
    expect(nightness(-0.3)).toBe(1);
  });

  test("the haze leaves the point looked at clear and thickens beyond", () => {
    const { start, density } = hazeFor(2000);
    expect(start).toBeGreaterThan(2000);
    const at = (d: number) => 1 - Math.exp(-Math.max(d - start, 0) * density);
    expect(at(2000)).toBe(0);
    // the top of a view tilted 60°, about two and a half times as far
    expect(at(5000)).toBeGreaterThan(0.3);
    expect(at(5000)).toBeLessThan(0.7);
  });

  test("the sky is blue by day and dark at night, its horizon where the view puts it", () => {
    const day = skyColours(0.6);
    expect(day.top[2]).toBeGreaterThan(day.top[0]);
    expect(day.horizon[0]).toBeGreaterThan(day.top[0]);
    const night = skyColours(-0.4);
    expect(night.top[0] + night.top[1] + night.top[2]).toBeLessThan(80);
    expect(skyBackground(0.6, 0.3, 0)).toContain(" 30%");
    // a horizon above the screen is clamped to its top
    expect(skyBackground(0.6, -2, 0)).toContain(" 0%");
  });
});

describe("realistic map: the ground", () => {
  const f = (properties: Record<string, unknown>) => ({ properties });
  // the material is carried in the alpha: 255 less its code
  const material = (rgba: number[]) => 255 - rgba[3];

  test("each kind of area has its own surface; names of places none", () => {
    expect(material(areaPaint(f({ layerName: "water", class: "lake" })))).toBe(5);
    expect(material(areaPaint(f({ layerName: "landcover", class: "wood", subclass: "forest" })))).toBe(2);
    expect(material(areaPaint(f({ layerName: "landcover", class: "farmland", subclass: "farmland" })))).toBe(3);
    expect(material(areaPaint(f({ layerName: "landuse", class: "railway" })))).toBe(8);
    expect(areaPaint(f({ layerName: "landuse", class: "suburb" }))[3]).toBe(0);
    expect(areaPaint(f({ layerName: "building" }))[3]).toBe(0);
  });

  test("roads at their widths with pavements, railways on ballast, tunnels not at all", () => {
    const motorway = lineOf(f({ layerName: "transportation", class: "motorway" }));
    const minor = lineOf(f({ layerName: "transportation", class: "minor" }));
    expect(motorway.width).toBeGreaterThan(minor.width);
    expect(minor.edge).toBeGreaterThan(0);
    const rail = lineOf(f({ layerName: "transportation", class: "rail", subclass: "rail" }));
    expect(material(rail.paint)).toBe(8);
    expect(rail.edge).toBeGreaterThan(0);
    expect(lineOf(f({ layerName: "transportation", class: "rail", brunnel: "tunnel" })).width).toBe(0);
    expect(lineOf(f({ layerName: "transportation", class: "ferry" })).width).toBe(0);
    expect(lineOf(f({ layerName: "transportation", class: "path", subclass: "footway" })).width).toBe(2);
    expect(material(lineOf(f({ layerName: "waterway", class: "river" })).paint)).toBe(5);
  });
});
