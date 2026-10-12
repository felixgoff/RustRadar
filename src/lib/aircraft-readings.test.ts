// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import type { AdsbAircraft } from "./api";
import { aircraftReadings, degrees, listing } from "./aircraft-readings";

const base: AdsbAircraft = {
  hex: "406f2e",
  callsign: "BAW548",
  reg: "G-TTNA",
  typecode: "A20N",
  source: "adsb_icao",
  mlat: false,
  seenMs: 0,
  onGround: false,
};

const full: AdsbAircraft = {
  ...base,
  trueHeading: 138.6,
  roll: -12.4,
  altGeom: 37625,
  ias: 262,
  tas: 452,
  mach: 0.78,
  windDir: 291,
  windSpeed: 38,
  oat: -52,
  navAltitudeMcp: 37008,
  navHeading: 136.4,
  navModes: ["autopilot", "vnav", "lnav", "tcas"],
};

const labels = (rs: { label: string }[]) => rs.map((r) => r.label);

describe("aircraftReadings", () => {
  test("puts heading, IAS, Mach and wind first, always, in that order", () => {
    expect(labels(aircraftReadings(full).first)).toEqual(["Heading", "IAS", "Mach", "Wind"]);
    expect(labels(aircraftReadings(base).first)).toEqual(["Heading", "IAS", "Mach", "Wind"]);
  });

  test("formats the first values in aviation notation", () => {
    const values = aircraftReadings(full).first.map((r) => r.value);
    expect(values).toEqual(["139°", "262 kt", "0.780", "291° at 38 kt"]);
  });

  test("marks a first value the aircraft doesn't report as null, keeping its place", () => {
    const { first } = aircraftReadings({ ...full, mach: undefined, windSpeed: undefined });
    expect(first.map((r) => r.value)).toEqual(["139°", "262 kt", null, null]);
  });

  test("puts every other reported value behind More, nothing unreported", () => {
    const r = aircraftReadings(full);
    expect(labels(r.more)).toEqual(["Bank", "GNSS altitude", "TAS", "Selected alt.", "Selected hdg", "OAT", "Autopilot modes"]);
    expect(r.unreported).toEqual([]);
    expect(r.more.find((x) => x.label === "Bank")?.value).toBe("12° L");
    expect(r.more.find((x) => x.label === "Autopilot modes")?.value).toBe("Autopilot · VNAV · LNAV · TCAS");
  });

  test("leaves unreported values out of More and names them instead", () => {
    const r = aircraftReadings({ ...full, roll: undefined, oat: undefined, navModes: [] });
    expect(labels(r.more)).toEqual(["GNSS altitude", "TAS", "Selected alt.", "Selected hdg"]);
    expect(r.unreported).toEqual(["bank", "OAT", "autopilot modes"]);
  });

  test("falls back to the FMS altitude when the panel's isn't reported", () => {
    const r = aircraftReadings({ ...base, navAltitudeFms: 24000 });
    expect(r.more.map((x) => x.value)).toEqual(["24,000 ft"]);
  });

  test("with nothing beyond the basics, More is empty", () => {
    expect(aircraftReadings(base).more).toEqual([]);
  });
});

describe("degrees", () => {
  test("pads to three digits and wraps at 360", () => {
    expect(degrees(7)).toBe("007°");
    expect(degrees(359.6)).toBe("000°");
    expect(degrees(-10)).toBe("350°");
  });
});

describe("listing", () => {
  test("joins with commas and a final and", () => {
    expect(listing([])).toBe("");
    expect(listing(["OAT"])).toBe("OAT");
    expect(listing(["bank", "OAT"])).toBe("bank and OAT");
    expect(listing(["bank", "OAT", "autopilot modes"])).toBe("bank, OAT and autopilot modes");
  });
});
