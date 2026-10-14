// The ships sidebar: which loaded ships match the search and filters, and in
// what order. Pure, so the map and the list agree and it can be tested.
import type { Ship } from "./api";
import { cosAngle } from "./geo";
import { SHIP_COLOR_KEYS, shipKindKey, shipKindLabel } from "./ships";
import { fold } from "./train-list";

export type ShipSort = "size" | "name" | "distance";
/** Under way is a ship with a heading; moored covers those without one too (AIS gives no heading at rest). */
export type ShipMotion = "all" | "moving" | "moored";
export type ShipSize = Ship["size"];

/** VesselFinder's size classes, as the list names them. */
export const SHIP_SIZES: { size: ShipSize; label: string }[] = [
  { size: 0, label: "Small" },
  { size: 1, label: "Medium" },
  { size: 2, label: "Large" },
];

/** Shorter type names for chips and list rows; the ship's panel keeps VesselFinder's full wording. */
const SHORT: Record<number, string> = { 2: "Tug or special", 7: "Rescue or law", 8: "Pleasure or sailing" };
export const shipKindShort = (kind: number) => SHORT[shipKindKey(kind)] ?? shipKindLabel(kind);

/** What the coverage note says: the feed thins ships by zoom. */
export const SHIP_COVERAGE = "Ships in view from VesselFinder; zoom in to see more.";

export interface ShipFilters {
  /** Type groups (`shipKindKey`) left out; empty shows them all. */
  hiddenKinds: number[];
  motion: ShipMotion;
  /** Size classes left out. */
  hiddenSizes: ShipSize[];
  /** Leave out ships whose position is old. */
  hideOld: boolean;
  sort: ShipSort;
}

export const DEFAULT_SHIP_FILTERS: ShipFilters = {
  hiddenKinds: [],
  motion: "all",
  hiddenSizes: [],
  hideOld: false,
  sort: "size",
};

const SORTS: ShipSort[] = ["size", "name", "distance"];
const MOTIONS: ShipMotion[] = ["all", "moving", "moored"];

/** Stored ship filters over the defaults; anything unreadable falls back. */
export function mergeShipFilters(saved: unknown): ShipFilters {
  const f: ShipFilters = structuredClone(DEFAULT_SHIP_FILTERS);
  if (!saved || typeof saved !== "object") return f;
  const s = saved as Record<string, unknown>;
  if (Array.isArray(s.hiddenKinds)) f.hiddenKinds = s.hiddenKinds.filter((k) => SHIP_COLOR_KEYS.includes(k));
  if (Array.isArray(s.hiddenSizes)) f.hiddenSizes = s.hiddenSizes.filter((k) => k === 0 || k === 1 || k === 2);
  if (MOTIONS.includes(s.motion as ShipMotion)) f.motion = s.motion as ShipMotion;
  if (typeof s.hideOld === "boolean") f.hideOld = s.hideOld;
  if (SORTS.includes(s.sort as ShipSort)) f.sort = s.sort as ShipSort;
  return f;
}

/** How many filter groups narrow the list, for the badge (the sort is not a filter). */
export function shipFilterCount(f: ShipFilters): number {
  return [f.hiddenKinds.length > 0, f.motion !== "all", f.hiddenSizes.length > 0, f.hideOld].filter(Boolean).length;
}

/** A name worth showing: VesselFinder sends the MMSI as the name when the ship never broadcast one. */
export const shipName = (s: Pick<Ship, "name" | "mmsi">) => (s.name && s.name !== String(s.mmsi) ? s.name : "");

/** Whether a ship matches the search: its name (every word), or its MMSI from the start. */
export function shipMatches(ship: Ship, query: string): boolean {
  const q = query.trim();
  if (!q) return true;
  if (/^\d+$/.test(q)) return String(ship.mmsi).startsWith(q) || fold(ship.name).includes(q);
  const hay = fold(shipName(ship));
  return fold(q)
    .split(/\s+/)
    .every((w) => hay.includes(w));
}

/** A predicate for the filters alone: what the map draws. */
export function shipFilter(f: ShipFilters): (s: Ship) => boolean {
  const kinds = new Set(f.hiddenKinds);
  const sizes = new Set(f.hiddenSizes);
  return (s) => {
    if (kinds.has(shipKindKey(s.kind)) || sizes.has(s.size)) return false;
    if (f.hideOld && s.old) return false;
    if (f.motion === "moving" && s.heading === null) return false;
    if (f.motion === "moored" && s.heading !== null) return false;
    return true;
  };
}

/** Length overall in metres from the AIS dimensions, when known. */
export const shipLength = (s: Pick<Ship, "dims">) => (s.dims ? s.dims[0] + s.dims[1] : null);

const byName = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/**
 * A new array in the chosen order. Size: the size class first, then the
 * measured length within it (known lengths before unknown), largest first.
 * Unnamed ships sort after named ones by name; ties fall back to the MMSI.
 */
export function sortShips(list: readonly Ship[], sort: ShipSort, centre: { lat: number; lon: number }): Ship[] {
  const name = (a: Ship, b: Ship) => {
    const [na, nb] = [shipName(a), shipName(b)];
    return Number(!na) - Number(!nb) || byName.compare(na, nb) || a.mmsi - b.mmsi;
  };
  if (sort === "distance") {
    const keyed = list.map((s) => ({ s, c: cosAngle(s.lon, s.lat, centre.lon, centre.lat) }));
    return keyed.sort((a, b) => b.c - a.c || name(a.s, b.s)).map((k) => k.s);
  }
  if (sort === "name") return [...list].sort(name);
  return [...list].sort((a, b) => {
    if (a.size !== b.size) return b.size - a.size;
    const [la, lb] = [shipLength(a), shipLength(b)];
    if (la === null || lb === null) return Number(la === null) - Number(lb === null) || name(a, b);
    return lb - la || name(a, b);
  });
}
