// Flight path export: the recorded track of a flight as a file other tools
// open. KML plays back in Google Earth (a timed, extruded 3D track); GPX goes
// into mapping and GPS software; CSV into a spreadsheet; GeoJSON into GIS
// tools and web maps. Built here, saved by the backend (`save_export`).
//
// Altitudes are what Flightradar24 reports: pressure altitude in feet (not
// corrected for the day's QNH), written in metres where a format expects them.

import { invoke } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { isTauri, type TrailPoint } from "./api";

export type ExportFormat = "kml" | "gpx" | "csv" | "geojson";

export const EXPORT_FORMATS: { format: ExportFormat; label: string; hint: string }[] = [
  { format: "kml", label: "KML", hint: "Google Earth, with a 3D track that plays back in time" },
  { format: "gpx", label: "GPX", hint: "Mapping and GPS apps" },
  { format: "csv", label: "CSV", hint: "Spreadsheets: time, position, altitude, speed, track" },
  { format: "geojson", label: "GeoJSON", hint: "GIS tools and web maps" },
];

/** What the file says about the flight. Empty strings are left out. */
export interface ExportMeta {
  callsign: string;
  flightNumber: string;
  registration: string;
  aircraftType: string;
  origin: string;
  destination: string;
}

const FT_TO_M = 0.3048;

/** The trail in time order, without points that carry no time or position. */
export function exportPoints(trail: readonly TrailPoint[]): TrailPoint[] {
  return trail
    .filter((p) => p.timestamp > 0 && Number.isFinite(p.latitude) && Number.isFinite(p.longitude))
    .slice()
    .sort((a, b) => a.timestamp - b.timestamp);
}

const iso = (unix: number) => new Date(unix * 1000).toISOString().replace(".000Z", "Z");
const metres = (feet: number) => Math.round(Math.max(0, feet) * FT_TO_M * 10) / 10;
const coord = (x: number) => Number(x.toFixed(6));

function xml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&apos;");
}

/** "BAW117 (BA117) LHR–JFK" from whatever is known. */
export function exportTitle(meta: ExportMeta): string {
  const id = meta.callsign || meta.flightNumber || meta.registration || "Flight";
  const number = meta.flightNumber && meta.flightNumber !== id ? ` (${meta.flightNumber})` : "";
  const route = meta.origin || meta.destination ? ` ${meta.origin || "?"}–${meta.destination || "?"}` : "";
  return `${id}${number}${route}`;
}

function description(meta: ExportMeta, points: readonly TrailPoint[]): string {
  const parts = [
    meta.aircraftType && `Aircraft ${meta.aircraftType}`,
    meta.registration && `registration ${meta.registration}`,
    points.length ? `${points.length} positions, ${iso(points[0].timestamp)} to ${iso(points[points.length - 1].timestamp)}` : "",
    "Altitude is pressure altitude as reported. Exported from RustRadar; data from Flightradar24.",
  ].filter(Boolean);
  return parts.join(". ");
}

/** File name for the export: "BAW117 LHR-JFK 2026-10-12.kml". */
export function exportFileName(meta: ExportMeta, points: readonly TrailPoint[], format: ExportFormat): string {
  const id = meta.callsign || meta.flightNumber || meta.registration || "flight";
  const route = meta.origin && meta.destination ? ` ${meta.origin}-${meta.destination}` : "";
  const day = points.length ? ` ${iso(points[0].timestamp).slice(0, 10)}` : "";
  return `${id}${route}${day}.${format}`;
}

function kml(points: readonly TrailPoint[], meta: ExportMeta): string {
  const title = xml(exportTitle(meta));
  const when = points.map((p) => `        <when>${iso(p.timestamp)}</when>`).join("\n");
  const coords = points
    .map((p) => `        <gx:coord>${coord(p.longitude)} ${coord(p.latitude)} ${metres(p.altitude)}</gx:coord>`)
    .join("\n");
  return `<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://www.opengis.net/kml/2.2" xmlns:gx="http://www.google.com/kml/ext/2.2">
  <Document>
    <name>${title}</name>
    <description>${xml(description(meta, points))}</description>
    <Style id="track">
      <LineStyle><color>ff34a6f0</color><width>3</width></LineStyle>
      <PolyStyle><color>4034a6f0</color></PolyStyle>
      <IconStyle><scale>0.8</scale><Icon><href>http://maps.google.com/mapfiles/kml/shapes/airports.png</href></Icon></IconStyle>
    </Style>
    <Placemark>
      <name>${title}</name>
      <styleUrl>#track</styleUrl>
      <gx:Track>
        <extrude>1</extrude>
        <altitudeMode>absolute</altitudeMode>
${when}
${coords}
      </gx:Track>
    </Placemark>
  </Document>
</kml>
`;
}

function gpx(points: readonly TrailPoint[], meta: ExportMeta): string {
  const title = xml(exportTitle(meta));
  const pts = points
    .map(
      (p) =>
        `      <trkpt lat="${coord(p.latitude)}" lon="${coord(p.longitude)}"><ele>${metres(p.altitude)}</ele><time>${iso(p.timestamp)}</time></trkpt>`,
    )
    .join("\n");
  return `<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="RustRadar" xmlns="http://www.topografix.com/GPX/1/1">
  <metadata>
    <name>${title}</name>
    <desc>${xml(description(meta, points))}</desc>
${points.length ? `    <time>${iso(points[0].timestamp)}</time>\n` : ""}  </metadata>
  <trk>
    <name>${title}</name>
    <trkseg>
${pts}
    </trkseg>
  </trk>
</gpx>
`;
}

function csvField(s: string): string {
  return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

function csv(points: readonly TrailPoint[], meta: ExportMeta): string {
  const header = [
    "time_utc",
    "unix_time",
    "latitude",
    "longitude",
    "altitude_ft",
    "ground_speed_kt",
    "track_deg",
    "vertical_speed_fpm",
    "callsign",
  ].join(",");
  const rows = points.map((p) =>
    [
      iso(p.timestamp),
      p.timestamp,
      coord(p.latitude),
      coord(p.longitude),
      Math.round(p.altitude),
      Math.round(p.ground_speed),
      Math.round(p.track),
      Math.round(p.vertical_speed),
      csvField(meta.callsign),
    ].join(","),
  );
  return [header, ...rows].join("\n") + "\n";
}

function geojson(points: readonly TrailPoint[], meta: ExportMeta): string {
  const feature = {
    type: "Feature",
    geometry: {
      type: "LineString",
      coordinates: points.map((p) => [coord(p.longitude), coord(p.latitude), metres(p.altitude)]),
    },
    properties: {
      name: exportTitle(meta),
      callsign: meta.callsign || null,
      flightNumber: meta.flightNumber || null,
      registration: meta.registration || null,
      aircraftType: meta.aircraftType || null,
      origin: meta.origin || null,
      destination: meta.destination || null,
      altitude: "pressure altitude in metres, as reported",
      times: points.map((p) => iso(p.timestamp)),
      groundSpeedKt: points.map((p) => Math.round(p.ground_speed)),
      source: "Flightradar24, via RustRadar",
    },
  };
  return JSON.stringify({ type: "FeatureCollection", features: [feature] }, null, 2) + "\n";
}

/** The file's contents in `format`. */
export function exportPath(trail: readonly TrailPoint[], meta: ExportMeta, format: ExportFormat): string {
  const points = exportPoints(trail);
  switch (format) {
    case "kml":
      return kml(points, meta);
    case "gpx":
      return gpx(points, meta);
    case "csv":
      return csv(points, meta);
    case "geojson":
      return geojson(points, meta);
  }
}

/**
 * Saves the export: to the Downloads folder in the app (returning where, after
 * showing it in the file manager), or as a browser download outside it.
 */
export async function saveExport(fileName: string, contents: string, format: ExportFormat): Promise<string> {
  if (isTauri()) {
    const path = await invoke<string>("save_export", { fileName, contents });
    // showing it is a convenience; the file is saved either way
    revealItemInDir(path).catch(() => {});
    return path;
  }
  const type = { kml: "application/vnd.google-earth.kml+xml", gpx: "application/gpx+xml", csv: "text/csv", geojson: "application/geo+json" }[format];
  const url = URL.createObjectURL(new Blob([contents], { type }));
  const a = Object.assign(document.createElement("a"), { href: url, download: fileName });
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
  return fileName;
}
