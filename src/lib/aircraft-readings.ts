// What the aircraft's own transponder reports (via adsb.lol), as the flight
// panel lists it: the few values worth a glance always, the rest on request.
import type { AdsbAircraft } from "./api";

export interface Reading {
  label: string;
  /** Formatted; null when this aircraft doesn't report it. */
  value: string | null;
  /** What the value is, where the label alone doesn't say. */
  title?: string;
  /** Takes a whole row of the grid. */
  wide?: boolean;
  /** Words rather than a measurement: set in the UI face, not the data face. */
  words?: boolean;
}

export interface Readings {
  /** Always on show, in fixed places, with a dash for what isn't reported. */
  first: Reading[];
  /** Behind "More from the aircraft": only what this aircraft reports. */
  more: Reading[];
  /** The rest of the "more" values, which this aircraft doesn't report, for a sentence. */
  unreported: string[];
}

/** `007°`: three digits, as headings and tracks are written. */
export const degrees = (v: number) => `${String(((Math.round(v) % 360) + 360) % 360).padStart(3, "0")}°`;

const number = (v: number | undefined, unit = "", digits = 0) =>
  v === undefined || !Number.isFinite(v)
    ? null
    : `${v.toLocaleString(undefined, { maximumFractionDigits: digits, minimumFractionDigits: digits })}${unit}`;

const MODES: Record<string, string> = {
  autopilot: "Autopilot",
  vnav: "VNAV",
  althold: "Altitude hold",
  approach: "Approach",
  lnav: "LNAV",
  tcas: "TCAS",
};

export function aircraftReadings(m: AdsbAircraft): Readings {
  const first: Reading[] = [
    { label: "Heading", value: m.trueHeading === undefined ? null : degrees(m.trueHeading) },
    { label: "IAS", value: number(m.ias, " kt") },
    { label: "Mach", value: number(m.mach, "", 3) },
    {
      label: "Wind",
      value: m.windDir === undefined || m.windSpeed === undefined ? null : `${degrees(m.windDir)} at ${m.windSpeed} kt`,
      title: "Derived from the aircraft's airspeed, heading and ground track",
      wide: true,
    },
  ];
  // [reading, how a sentence names it]
  const rest: [Reading, string][] = [
    [
      {
        label: "Bank",
        value:
          m.roll === undefined
            ? null
            : `${Math.abs(m.roll).toFixed(0)}° ${m.roll > 0.5 ? "R" : m.roll < -0.5 ? "L" : ""}`.trim(),
      },
      "bank",
    ],
    [{ label: "GNSS altitude", value: number(m.altGeom, " ft") }, "GNSS altitude"],
    [{ label: "TAS", value: number(m.tas, " kt") }, "TAS"],
    [
      {
        label: "Selected alt.",
        value: number(m.navAltitudeMcp ?? m.navAltitudeFms, " ft"),
        title: "Set on the autopilot (or by the flight management system)",
      },
      "selected altitude",
    ],
    [
      {
        label: "Selected hdg",
        value: m.navHeading === undefined ? null : degrees(m.navHeading),
        title: "Set on the autopilot; magnetic",
      },
      "selected heading",
    ],
    [{ label: "OAT", value: number(m.oat, " °C") }, "OAT"],
    [
      {
        label: "Autopilot modes",
        value: m.navModes?.length ? m.navModes.map((mode) => MODES[mode] ?? mode.toUpperCase()).join(" · ") : null,
        wide: true,
        words: true,
      },
      "autopilot modes",
    ],
  ];
  return {
    first,
    more: rest.filter(([r]) => r.value !== null).map(([r]) => r),
    unreported: rest.filter(([r]) => r.value === null).map(([, name]) => name),
  };
}

/** "bank, OAT and autopilot modes". */
export function listing(names: string[]): string {
  return names.length < 2 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
}
