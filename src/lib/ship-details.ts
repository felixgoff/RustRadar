// How a ship's card reads: the age of its last report, and its numbers in words.
import { ageLabel } from "./freshness";
import type { NavStatus } from "./api";

/** AIS reports come every few seconds to a few minutes; past this a position is worth flagging as old. */
export const SHIP_STALE_S = 15 * 60;

/** `Position 2 min ago`, or past {@link SHIP_STALE_S} `Position 47 min old`, in the warning tone. */
export function positionAge(positionUnix: number, nowMs: number): { stale: boolean; label: string } {
  const age = Math.max(0, nowMs / 1000 - positionUnix);
  return age > SHIP_STALE_S
    ? { stale: true, label: `Position ${ageLabel(age)} old` }
    : { stale: false, label: `Position ${ageLabel(age)} ago` };
}

/** Statuses that say something is wrong: the warning tone, always with their words. */
const ALARMING = new Set([2, 6, 14]);
export const navTone = (status: NavStatus) => (ALARMING.has(status.code) ? "warn" : "neutral");

/** `366 × 48 m`, `366 m` with no beam, or null. */
export function dimensions(length: number | null, beam: number | null): string | null {
  const m = (v: number) => Math.round(v).toLocaleString("en-GB");
  if (length && beam) return `${m(length)} × ${m(beam)} m`;
  if (length) return `${m(length)} m`;
  return beam ? `${m(beam)} m beam` : null;
}

/** `9.3 / 15.5 m`, `9.3 m` or `max 15.5 m`, or null. */
export function draught(current: number | null, max: number | null): string | null {
  const m = (v: number) => v.toFixed(1);
  if (current && max) return `${m(current)} / ${m(max)} m`;
  if (current) return `${m(current)} m`;
  return max ? `max ${m(max)} m` : null;
}

/** `51.9213° N, 4.4812° E`. */
export function latLon(lat: number, lon: number): string {
  const f = (v: number, pos: string, neg: string) => `${Math.abs(v).toFixed(4)}° ${v < 0 ? neg : pos}`;
  return `${f(lat, "N", "S")}, ${f(lon, "E", "W")}`;
}

/** A UN/LOCODE (`UKSOU`): two letters of country, three of place. Anything else is the crew's free text. */
export const isLocode = (destination: string) => /^[A-Z]{2} ?[A-Z2-9]{3}$/.test(destination);
