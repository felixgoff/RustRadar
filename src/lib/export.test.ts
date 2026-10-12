// Unit tests for flight path export. Run with `bun test src`.
// @ts-ignore -- bun's types aren't a dependency; svelte-check still type-checks this file
import * as bunTest from "bun:test";
import type { TrailPoint } from "./api";
import { exportFileName, exportPath, exportPoints, exportTitle, type ExportMeta } from "./export";

interface Matchers {
  toBe(value: unknown): void;
  toContain(value: unknown): void;
  toEqual(value: unknown): void;
}
const { test, expect } = bunTest as unknown as {
  test(name: string, fn: () => void): void;
  expect(value: unknown): Matchers & { not: Matchers };
};

const point = (timestamp: number, latitude: number, longitude: number, altitude: number): TrailPoint => ({
  timestamp,
  latitude,
  longitude,
  altitude,
  ground_speed: 450,
  track: 270,
  vertical_speed: 0,
  source: 0,
});

// 2026-10-12 16:00:00Z onwards, deliberately out of order with a bad point
const TRAIL = [
  point(1_791_820_860, 51.5, -0.6, 3_000),
  point(1_791_820_800, 51.47, -0.45, 0),
  point(0, 0, 0, 0),
  point(1_791_820_920, 51.52, -0.8, 10_000),
];
const META: ExportMeta = {
  callsign: "BAW117",
  flightNumber: "BA117",
  registration: "G-VIIL",
  aircraftType: "B772",
  origin: "LHR",
  destination: "JFK",
};

test("points are in time order and timeless points are dropped", () => {
  expect(exportPoints(TRAIL).map((p) => p.timestamp)).toEqual([1_791_820_800, 1_791_820_860, 1_791_820_920]);
});

test("titles and file names say which flight it is", () => {
  expect(exportTitle(META)).toBe("BAW117 (BA117) LHR–JFK");
  expect(exportTitle({ ...META, callsign: "", flightNumber: "", origin: "", destination: "" })).toBe("G-VIIL");
  expect(exportFileName(META, exportPoints(TRAIL), "kml")).toBe("BAW117 LHR-JFK 2026-10-12.kml");
  expect(exportFileName({ ...META, callsign: "" }, [], "csv")).toBe("BA117 LHR-JFK.csv");
});

test("KML is a timed, extruded track in metres", () => {
  const kml = exportPath(TRAIL, META, "kml");
  expect(kml).toContain("<gx:Track>");
  expect(kml).toContain("<altitudeMode>absolute</altitudeMode>");
  expect(kml).toContain("<when>2026-10-12T16:00:00Z</when>");
  // 10,000 ft
  expect(kml).toContain("<gx:coord>-0.8 51.52 3048</gx:coord>");
  expect(kml).not.toContain("<when>1970");
});

test("GPX has one track point per position, with height and time", () => {
  const gpx = exportPath(TRAIL, META, "gpx");
  expect(gpx.match(/<trkpt /g)?.length).toBe(3);
  expect(gpx).toContain('<trkpt lat="51.47" lon="-0.45"><ele>0</ele><time>2026-10-12T16:00:00Z</time></trkpt>');
});

test("CSV has a header and a row per position", () => {
  const lines = exportPath(TRAIL, META, "csv").trim().split("\n");
  expect(lines.length).toBe(4);
  expect(lines[0]).toBe("time_utc,unix_time,latitude,longitude,altitude_ft,ground_speed_kt,track_deg,vertical_speed_fpm,callsign");
  expect(lines[3]).toBe("2026-10-12T16:02:00Z,1791820920,51.52,-0.8,10000,450,270,0,BAW117");
});

test("GeoJSON is a line with a time per vertex", () => {
  const json = JSON.parse(exportPath(TRAIL, META, "geojson"));
  const feature = json.features[0];
  expect(feature.geometry.type).toBe("LineString");
  expect(feature.geometry.coordinates[1]).toEqual([-0.6, 51.5, 914.4]);
  expect(feature.properties.times.length).toBe(3);
});

test("names with markup are escaped in XML", () => {
  const kml = exportPath(TRAIL, { ...META, callsign: "A&B<1>" }, "kml");
  expect(kml).toContain("A&amp;B&lt;1&gt;");
  expect(kml).not.toContain("A&B<1>");
});
