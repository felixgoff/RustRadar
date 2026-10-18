// The trains sidebar: which loaded trains match the search and filters, and
// in what order. Pure, so the map and the list agree and it can be tested.
import type { Train, TrainKind } from "./api";
import { cosAngle } from "./geo";
import { TRAIN_KINDS } from "./trains";

export type TrainSort = "delay" | "speed" | "name" | "distance";

/** What the coverage note says: where live trains exist at all. */
export const TRAIN_COVERAGE = "Live trains are shown in Sweden, from Trafikverket.";

/** A train this many minutes late or more counts as delayed (as `delayTone` starts to warn). */
export const DELAYED_MINUTES = 5;

export interface TrainFilters {
  /** Kinds left out; empty shows every kind. Stored as left out, so a kind added later shows. */
  hiddenKinds: TrainKind[];
  /** Operators left out, as trains name them ("" for trains whose operator isn't known); not streamed live either. */
  hiddenOperators: string[];
  delayedOnly: boolean;
  sort: TrainSort;
}

export const DEFAULT_TRAIN_FILTERS: TrainFilters = {
  hiddenKinds: [],
  hiddenOperators: [],
  delayedOnly: false,
  sort: "delay",
};

const SORTS: TrainSort[] = ["delay", "speed", "name", "distance"];

/** Stored train filters over the defaults; anything unreadable falls back. */
export function mergeTrainFilters(saved: unknown): TrainFilters {
  const f: TrainFilters = structuredClone(DEFAULT_TRAIN_FILTERS);
  if (!saved || typeof saved !== "object") return f;
  const s = saved as Record<string, unknown>;
  const kinds = new Set<string>(TRAIN_KINDS);
  if (Array.isArray(s.hiddenKinds)) f.hiddenKinds = s.hiddenKinds.filter((k) => kinds.has(k)) as TrainKind[];
  if (Array.isArray(s.hiddenOperators)) f.hiddenOperators = s.hiddenOperators.filter((o) => typeof o === "string");
  if (typeof s.delayedOnly === "boolean") f.delayedOnly = s.delayedOnly;
  if (SORTS.includes(s.sort as TrainSort)) f.sort = s.sort as TrainSort;
  return f;
}

/** How many filter groups narrow the list, for the badge (the sort is not a filter). */
export function trainFilterCount(f: TrainFilters): number {
  return [f.hiddenKinds.length > 0, f.hiddenOperators.length > 0, f.delayedOnly].filter(Boolean).length;
}

/** Lower case without accents, so "helsinki" finds "Helsinki" and "oslo s" finds "Oslo S". */
export const fold = (s: string) =>
  s
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .toLowerCase();

/** Whether a train matches the search: every word in its name (and number), destination or operator. */
export function trainMatches(train: Train, query: string): boolean {
  const words = fold(query).split(/\s+/).filter(Boolean);
  if (!words.length) return true;
  const hay = fold([train.name, train.destination ?? "", train.operator ?? ""].join(" "));
  return words.every((w) => hay.includes(w));
}

export const isDelayed = (t: Pick<Train, "delayMinutes">) =>
  t.delayMinutes !== null && Number.isFinite(t.delayMinutes) && t.delayMinutes >= DELAYED_MINUTES;

/** A predicate for the filters alone: what the map draws. */
export function trainFilter(f: TrainFilters): (t: Train) => boolean {
  const kinds = new Set(f.hiddenKinds);
  const operators = new Set(f.hiddenOperators);
  return (t) =>
    !kinds.has(t.kind) && !operators.has(t.operator ?? "") && (!f.delayedOnly || isDelayed(t));
}

const byName = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** Unknown values sort last whichever way the rest go. */
function nullsLast(a: number | null, b: number | null, descending: boolean): number {
  const ka = a === null || !Number.isFinite(a);
  const kb = b === null || !Number.isFinite(b);
  if (ka || kb) return Number(ka) - Number(kb);
  return descending ? b! - a! : a! - b!;
}

/** A new array in the chosen order; ties fall back to the name. */
export function sortTrains(list: readonly Train[], sort: TrainSort, centre: { lat: number; lon: number }): Train[] {
  const name = (a: Train, b: Train) => byName.compare(a.name, b.name) || a.id.localeCompare(b.id);
  if (sort === "distance") {
    // nearest first: the larger the cosine of the angle, the closer
    const keyed = list.map((t) => ({ t, c: cosAngle(t.lon, t.lat, centre.lon, centre.lat) }));
    return keyed.sort((a, b) => b.c - a.c || name(a.t, b.t)).map((k) => k.t);
  }
  const compare =
    sort === "delay"
      ? (a: Train, b: Train) => nullsLast(a.delayMinutes, b.delayMinutes, true)
      : sort === "speed"
        ? (a: Train, b: Train) => nullsLast(a.speedKmh, b.speedKmh, true)
        : () => 0;
  return [...list].sort((a, b) => compare(a, b) || name(a, b));
}

/** Distance along the ground in kilometres, for a row when sorted by distance. */
export function distanceKm(lat: number, lon: number, centre: { lat: number; lon: number }): number {
  const c = Math.min(1, Math.max(-1, cosAngle(lon, lat, centre.lon, centre.lat)));
  return Math.acos(c) * 6371;
}

/** "850 m", "12 km", "1,240 km". */
export function distanceLabel(km: number): string {
  if (km < 1) return `${Math.round(km * 100) * 10} m`;
  return `${Math.round(km).toLocaleString("en-GB")} km`;
}

/** The operators of these trains, most trains first ("" for those whose operator isn't known, last). */
export function trainOperators(trains: readonly Pick<Train, "operator">[]): { name: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const t of trains) counts.set(t.operator ?? "", (counts.get(t.operator ?? "") ?? 0) + 1);
  return [...counts]
    .map(([name, count]) => ({ name, count }))
    .sort((a, b) => Number(a.name === "") - Number(b.name === "") || b.count - a.count || a.name.localeCompare(b.name));
}
