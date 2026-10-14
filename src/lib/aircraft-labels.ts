// What the map writes next to each aircraft when aircraft labels are on.
import type { LiveFlight } from "./api";

/** The aircraft's name on the map: its callsign, else its flight number, else its registration. */
export function aircraftLabel(flight: Pick<LiveFlight, "callsign" | "flight" | "reg">): string {
  return flight.callsign.trim() || flight.flight.trim() || flight.reg.trim();
}

/** Aviation's own notation: flight levels above the (US) transition altitude, feet below it. */
export function altitudeLabel(altitudeFt: number, onGround: boolean): string {
  if (onGround) return "GND";
  const ft = Math.max(0, Math.round(altitudeFt));
  return ft >= 18_000 ? `FL${String(Math.round(ft / 100)).padStart(3, "0")}` : `${ft.toLocaleString()} ft`;
}

/** The size (metres) at which an aircraft counts as large as they come, for {@link labelPriority}. */
const LARGEST_M = 80;
/** Above this, altitude no longer makes a label less important. */
const HIGHEST_FT = 45_000;

/**
 * Which label wins where two would overlap, in the collision filter's range
 * (-1000 to 1000, higher wins): larger aircraft first, then lower ones, which
 * are the ones near airports and about to land or just off. The selected
 * aircraft takes {@link SELECTED_LABEL_PRIORITY}, above all of these.
 */
export function labelPriority(sizeM: number, altitudeFt: number, onGround: boolean): number {
  const size = Math.min(1, Math.max(0, sizeM / LARGEST_M));
  const low = onGround ? 1 : 1 - Math.min(1, Math.max(0, altitudeFt / HIGHEST_FT));
  return Math.round(600 * size + 300 * low) - 450;
}

export const SELECTED_LABEL_PRIORITY = 1000;
