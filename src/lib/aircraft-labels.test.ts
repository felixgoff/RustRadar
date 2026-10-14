// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { aircraftLabel, altitudeLabel, labelPriority, SELECTED_LABEL_PRIORITY } from "./aircraft-labels";

describe("aircraftLabel", () => {
  test("prefers the callsign, then the flight number, then the registration", () => {
    expect(aircraftLabel({ callsign: "DLH4AB", flight: "LH400", reg: "D-AIXA" })).toBe("DLH4AB");
    expect(aircraftLabel({ callsign: "", flight: "LH400", reg: "D-AIXA" })).toBe("LH400");
    expect(aircraftLabel({ callsign: " ", flight: "", reg: "D-AIXA" })).toBe("D-AIXA");
  });
  test("is empty when there is nothing to name it by", () => {
    expect(aircraftLabel({ callsign: "", flight: "", reg: "" })).toBe("");
  });
});

describe("altitudeLabel", () => {
  test("flight levels from 18,000 ft", () => {
    expect(altitudeLabel(37_000, false)).toBe("FL370");
    expect(altitudeLabel(18_000, false)).toBe("FL180");
    expect(altitudeLabel(37_049, false)).toBe("FL370");
  });
  test("feet below that, with thousands separators", () => {
    expect(altitudeLabel(4_500, false)).toBe((4_500).toLocaleString() + " ft");
    expect(altitudeLabel(4_500, false)).toMatch(/^4\D?500 ft$/);
    expect(altitudeLabel(-20, false)).toBe("0 ft");
  });
  test("GND on the ground", () => {
    expect(altitudeLabel(0, true)).toBe("GND");
  });
});

describe("labelPriority", () => {
  test("larger aircraft win over smaller ones at the same height", () => {
    expect(labelPriority(73, 35_000, false)).toBeGreaterThan(labelPriority(38, 35_000, false));
  });
  test("lower aircraft win over higher ones of the same size", () => {
    expect(labelPriority(38, 3_000, false)).toBeGreaterThan(labelPriority(38, 38_000, false));
  });
  test("stays in the collision filter's range, under the selected aircraft", () => {
    for (const p of [labelPriority(0, 60_000, false), labelPriority(500, 0, true)]) {
      expect(p).toBeGreaterThanOrEqual(-1000);
      expect(p).toBeLessThan(SELECTED_LABEL_PRIORITY);
    }
  });
});
