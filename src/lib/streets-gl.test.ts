// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { hexColour, sglBuilding, sglPath, sglTree } from "./streets-gl";

describe("sglBuilding", () => {
  test("levels of 4 m, a pitched roof a level more", () => {
    expect(sglBuilding({ levels: 3 })).toMatchObject({ height: 12, minHeight: 0, roofHeight: 0, roofType: "flat" });
    expect(sglBuilding({ levels: 2, roofType: "gabled" })).toMatchObject({ height: 12, roofHeight: 4, roofType: "gabled" });
    expect(sglBuilding({}).height).toBe(4);
    // as a tile decoded to binary has a building nothing is mapped for: one level, not none
    expect(sglBuilding({ height: 0, levels: 0, minLevel: 0 }).height).toBe(4);
    expect(sglBuilding({ minLevel: 2 })).toMatchObject({ height: 12, minHeight: 8 });
  });

  test("a mapped height holds its roof", () => {
    expect(sglBuilding({ height: 9, roofType: "hipped", roofHeight: 12 })).toMatchObject({ height: 9, roofHeight: 9 });
  });

  test("numbers a binary tile fills with 0 are not mapped", () => {
    expect(sglBuilding({ height: 0, levels: 0, roofHeight: 0, roofLevels: 0, color: 0 })).toMatchObject({ height: 4, roofHeight: 0, colour: undefined });
  });

  test("a roof on posts is only its roof", () => {
    const canopy = sglBuilding({ buildingType: "roof", height: 5 });
    expect(canopy.minHeight).toBeGreaterThanOrEqual(4);
  });

  test("materials and colours as mapped", () => {
    const b = sglBuilding({ material: "brick", roofMaterial: "roof_tiles", color: 0xffff00, roofColor: 0x123456 });
    expect(b).toMatchObject({ facade: "brick", roof: "tiles", colour: "yellow", roofColour: "#123456" });
  });
});

describe("sglPath", () => {
  test("roads as wide as their lanes", () => {
    expect(sglPath({ pathType: "residential" })).toEqual({ kind: "minor", width: 6, surface: "asphalt" });
    expect(sglPath({ pathType: "primary", lanes: 4 })).toEqual({ kind: "primary", width: 12, surface: "asphalt" });
    expect(sglPath({ pathType: "secondary", oneway: true })?.width).toBe(4);
  });

  test("paths, surfaces and mapped widths", () => {
    expect(sglPath({ pathType: "footway" })).toEqual({ kind: "footway", width: null, surface: "dirt" });
    expect(sglPath({ pathType: "service", surface: "sett", width: 5 })).toEqual({ kind: "service", width: 5, surface: "cobblestone" });
    expect(sglPath({ pathType: "runway" })?.width).toBe(45);
    expect(sglPath({ pathType: "construction" })).toBeNull();
  });
});

test("tree kinds", () => {
  expect(sglTree({ leafType: "needleleaved" })).toBe("fir");
  expect(sglTree({ genus: "Quercus" })).toBe("oak");
  expect(sglTree({})).toBe("broadleaf");
});

test("named colours back to their names", () => {
  expect(hexColour(0xffffff)).toBe("white");
  expect(hexColour(0x334455)).toBe("#334455");
  expect(hexColour(undefined)).toBeUndefined();
});
