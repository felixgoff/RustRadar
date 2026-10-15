import type { TrainRoute } from "./api";

/** `[lon, lat]`. */
export type LonLat = [number, number];

/** Within this of its route (metres), a drawn train is on the track it runs along. */
export const ON_TRACK_M = 40;
/** Further than this from every part of its route (metres), a drawn train isn't on it: the route is out of date. */
export const OFF_ROUTE_M = 300;
/** It is looked for this far along its route on from where it was last (metres): it can't have gone further meanwhile. */
const SEARCH_M = 20_000;

/**
 * Where along its route the train was last found: a stretch, and a leg of
 * it (it only ever moves on), and the route on from the end of that leg.
 */
export type RouteCursor = { route: TrainRoute; seg: number; leg: number; rest: LonLat[][] };

export type RouteAhead = {
  /**
   * What to draw: from exactly where the train is drawn onto its route
   * (changing as it goes), then the route on from there (the same lines
   * until it is on a new leg of it). Both empty when there is none to draw.
   */
  head: LonLat[];
  rest: LonLat[][];
  cursor: RouteCursor;
  /** False when the train isn't on its route: the route is out of date, ask again. */
  on: boolean;
};

const M_PER_DEG = 111_320;

/** The point on the leg `a`-`b` nearest `p`, how far along it (0..1), and how far from `p`, metres (flat: fine over a few km). */
function nearestOn(p: LonLat, a: LonLat, b: LonLat): { q: LonLat; t: number; d: number } {
  const k = Math.cos((p[1] * Math.PI) / 180);
  const [ax, ay] = [(a[0] - p[0]) * k, a[1] - p[1]];
  const [dx, dy] = [(b[0] - a[0]) * k, b[1] - a[1]];
  const len2 = dx * dx + dy * dy;
  const t = len2 > 0 ? Math.max(0, Math.min(1, -(ax * dx + ay * dy) / len2)) : 0;
  const [x, y] = [ax + dx * t, ay + dy * t];
  return { q: [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t], t, d: Math.hypot(x, y) * M_PER_DEG };
}

/** Metres between two points (flat). */
function metres(a: LonLat, b: LonLat): number {
  const k = Math.cos((a[1] * Math.PI) / 180);
  return Math.hypot((b[0] - a[0]) * k, b[1] - a[1]) * M_PER_DEG;
}

type Found = { seg: number; leg: number; q: LonLat; t: number; d: number };

/**
 * The first place along the route from `from` where `p` is on it: the
 * nearest point of the first run of legs within `ON_TRACK_M` of it (a line
 * that later comes back past it is later on); else, failing that, the
 * nearest point at all (within `SEARCH_M` on).
 */
function locate(route: TrainRoute, p: LonLat, from: { seg: number; leg: number }): Found | null {
  let first: Found | null = null;
  let nearest: Found | null = null;
  let searched = 0;
  for (let s = from.seg; s < route.segments.length && searched < SEARCH_M; s++) {
    const line = route.segments[s];
    for (let j = s === from.seg ? from.leg : 0; j < line.length - 1 && searched < SEARCH_M; j++) {
      searched += metres(line[j], line[j + 1]);
      const n = nearestOn(p, line[j], line[j + 1]);
      const here = { seg: s, leg: j, ...n };
      if (!nearest || n.d < nearest.d) nearest = here;
      if (n.d <= ON_TRACK_M) {
        if (!first || n.d < first.d) first = here;
      } else if (first) {
        return first;
      }
    }
  }
  return first ?? nearest;
}

/**
 * The selected train's way ahead from exactly where it is drawn (`drawn`):
 * from the train to where it is on its route, then on along the route; the
 * part behind it trimmed off. A train short of the route's start (drawn a
 * little behind where the route was worked out from) goes there along its
 * own way `ahead` (where it is drawn next, on the track). A train beside
 * the route (on another track) is joined to it, but the route is out of
 * date; one off it, or past its end, has none drawn.
 */
export function routeAhead(route: TrainRoute, drawn: LonLat, cursor: RouteCursor | null, ahead: LonLat[] = []): RouteAhead {
  const from = cursor?.route === route ? cursor : { route, seg: 0, leg: 0, rest: [] };
  const off: RouteAhead = { head: [], rest: [], cursor: from, on: false };
  const found = locate(route, drawn, from);
  if (!found) return off;
  const line = route.segments[found.seg];
  // the route on from the end of the leg it is on (kept while it is on that leg)
  const onFrom = (leg: number): RouteCursor =>
    from.seg === found.seg && from.leg === leg && from.rest.length
      ? from
      : {
          route,
          seg: found.seg,
          leg,
          rest: [line.slice(leg + 1), ...route.segments.slice(found.seg + 1)].filter((l) => l.length >= 2),
        };
  const on = found.d <= ON_TRACK_M;
  if (!on && found.seg === 0 && found.leg === 0 && found.t === 0) {
    // short of its start: along its own way there
    const at = onFrom(0);
    const head: LonLat[] = [drawn];
    for (const p of ahead) {
      if (metres(p, line[0]) <= ON_TRACK_M) return { head: [...head, line[0], line[1]], rest: at.rest, cursor: at, on: true };
      const onto = nearestOn(p, line[0], line[1]);
      if (onto.t > 0 && onto.d <= ON_TRACK_M) return { head: [...head, onto.q, line[1]], rest: at.rest, cursor: at, on: true };
      head.push(p);
    }
    return off;
  }
  if (found.d > OFF_ROUTE_M || (!on && found.t === 1 && found.leg === line.length - 2)) return off;
  const at = onFrom(found.leg);
  return { head: [drawn, found.q, line[found.leg + 1]], rest: at.rest, cursor: at, on };
}

/**
 * The way from where the train is drawn (`drawn`) to the next place its
 * frames (`[lat, lon, unix ms]`) have it stand still, from `nowMs`: empty when
 * it doesn't stop within them, or stands there already.
 */
export function wayToStop(drawn: LonLat, frames: [number, number, number][], nowMs: number): LonLat[] {
  const way: LonLat[] = [drawn];
  for (let i = 1; i < frames.length; i++) {
    const [a, b] = [frames[i - 1], frames[i]];
    if (b[2] <= nowMs) continue;
    // a frame where it was a moment before: it stands there
    if (a[0] === b[0] && a[1] === b[1]) return a[2] <= nowMs ? [] : way;
    way.push([b[1], b[0]]);
  }
  return [];
}
