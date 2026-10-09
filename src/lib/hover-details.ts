// What the hover card adds to the feed's own data: the aircraft's photo, the
// airline's logo and the flight's progress, from `flight_details`. The source
// is unofficial and rate-limited, so a flight is only fetched once the pointer
// has rested on it, one request at a time, and the answers are cached.
import type { AircraftImage, FlightDetails } from "./api";

/** The few fields of a flight's details the hover card shows. */
export interface HoverInfo {
  images: AircraftImage[];
  /** Flightradar24 airline id for {@link airlineLogoUrl}, or 0. */
  logoId: number;
  /** 0–100, as Flightradar24 reports it; null when it doesn't. */
  progressPct: number | null;
}

/** Keeps the cache small: a flight's details carry its whole trail. */
export function hoverInfo(details: FlightDetails): HoverInfo {
  const schedule = details.schedule;
  const progress = details.progress;
  return {
    images: details.aircraft?.images ?? [],
    logoId: schedule?.painted_as_id || schedule?.operated_by_id || 0,
    // an empty progress record (no route known) reports 0 with no distances
    progressPct:
      progress && (progress.traversed_distance > 0 || progress.remaining_distance > 0) ? progress.progress_pct : null,
  };
}

/**
 * `loading`: asked for or about to be; `ready`: in hand; `unavailable`: the
 * request failed or the source is rate-limiting, so the card goes without.
 */
export type HoverStatus = "loading" | "ready" | "unavailable";

type Entry = { at: number; info: HoverInfo | null };

export interface HoverLoaderOptions {
  fetch: (flightId: number) => Promise<FlightDetails>;
  /** True for an error that means "slow down". */
  isRateLimited: (message: string) => boolean;
  /** Called when a request settles, so the card can re-read. */
  onchange: () => void;
  /** How long the pointer must rest on a flight before it is fetched. */
  dwellMs?: number;
  capacity?: number;
  ttlMs?: number;
  /** A failed flight is not asked for again for this long. */
  failureTtlMs?: number;
  /** After a rate-limit answer, nothing is asked for this long. */
  cooldownMs?: number;
  now?: () => number;
}

export class HoverLoader {
  private readonly opts: Required<HoverLoaderOptions>;
  /** Insertion order is recency order: least recently used first. */
  private readonly cache = new Map<number, Entry>();
  private hovered: number | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private inFlight: number | null = null;
  /** The one flight waiting for the request in flight to finish. */
  private pending: number | null = null;
  private coolUntil = 0;

  constructor(options: HoverLoaderOptions) {
    this.opts = {
      dwellMs: 300,
      capacity: 100,
      ttlMs: 10 * 60_000,
      failureTtlMs: 2 * 60_000,
      cooldownMs: 60_000,
      now: () => Date.now(),
      ...options,
    };
  }

  /** The pointer is now on `flightId`, or on no flight. */
  hover(flightId: number | null) {
    if (flightId === this.hovered) return;
    this.hovered = flightId;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    if (flightId === null || this.entry(flightId) || this.coolingDown()) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      if (this.hovered === flightId) this.request(flightId);
    }, this.opts.dwellMs);
  }

  /** Cached details for a flight, if fresh. */
  info(flightId: number): HoverInfo | null {
    return this.entry(flightId)?.info ?? null;
  }

  status(flightId: number): HoverStatus {
    const entry = this.entry(flightId);
    if (entry) return entry.info ? "ready" : "unavailable";
    if (this.inFlight === flightId || this.pending === flightId || this.timer !== null) return "loading";
    return this.coolingDown() ? "unavailable" : "loading";
  }

  /** Stop any waiting request, e.g. when the globe goes away. */
  dispose() {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.hovered = null;
    this.pending = null;
  }

  private coolingDown() {
    return this.opts.now() < this.coolUntil;
  }

  private entry(flightId: number): Entry | undefined {
    const entry = this.cache.get(flightId);
    if (!entry) return undefined;
    const ttl = entry.info ? this.opts.ttlMs : this.opts.failureTtlMs;
    this.cache.delete(flightId);
    if (this.opts.now() - entry.at > ttl) return undefined;
    this.cache.set(flightId, entry); // most recently used
    return entry;
  }

  private store(flightId: number, info: HoverInfo | null) {
    this.cache.delete(flightId);
    this.cache.set(flightId, { at: this.opts.now(), info });
    while (this.cache.size > this.opts.capacity) this.cache.delete(this.cache.keys().next().value!);
  }

  private request(flightId: number) {
    if (this.inFlight !== null) {
      this.pending = flightId; // replaces any earlier waiting flight
      return;
    }
    this.inFlight = flightId;
    this.opts
      .fetch(flightId)
      .then((details) => this.store(flightId, hoverInfo(details)))
      .catch((e) => {
        if (this.opts.isRateLimited(String(e))) this.coolUntil = this.opts.now() + this.opts.cooldownMs;
        else this.store(flightId, null);
      })
      .finally(() => {
        this.inFlight = null;
        const next = this.pending;
        this.pending = null;
        // only if the pointer is still there: anything else is stale
        if (next !== null && next === this.hovered && !this.entry(next) && !this.coolingDown()) this.request(next);
        this.opts.onchange();
      });
  }
}

/**
 * How far along its route a flight is, 0–1, from great-circle distances:
 * flown from the origin over flown plus remaining to the destination. An
 * estimate; null when either end is unknown or they are the same place.
 */
export function routeProgress(
  position: { lat: number; lon: number },
  origin: { lat: number; lon: number } | undefined,
  destination: { lat: number; lon: number } | undefined,
): number | null {
  if (!origin || !destination) return null;
  const angle = (a: { lat: number; lon: number }, b: { lat: number; lon: number }) => {
    const r = Math.PI / 180;
    const c =
      Math.sin(a.lat * r) * Math.sin(b.lat * r) + Math.cos(a.lat * r) * Math.cos(b.lat * r) * Math.cos((a.lon - b.lon) * r);
    return Math.acos(Math.min(1, Math.max(-1, c)));
  };
  if (angle(origin, destination) < 1e-4) return null; // under ~600 m: no route to speak of
  const flown = angle(origin, position);
  const remaining = angle(position, destination);
  return flown / (flown + remaining);
}
