// How old the live feed's data is, and when that age is worth saying out loud.

/** A missed refresh or a slow round is ordinary: below this age the status stays quiet. */
export const STALE_MIN_MS = 30_000;

/** The age past which the feed's data counts as stale: about two refresh intervals, never less than the minimum. */
export function staleAfter(intervalMs: number): number {
  return Math.max(STALE_MIN_MS, 2 * intervalMs);
}

/** `45 s`, `3 min`, `2 h 05 min`: seconds under a minute, then whole minutes. */
export function ageLabel(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  return m % 60 ? `${h} h ${String(m % 60).padStart(2, "0")} min` : `${h} h`;
}

export interface Freshness {
  stale: boolean;
  label: string;
}

/** What the sidebar says about data `ageMs` old, when the feed refreshes every `intervalMs`. */
export function feedFreshness(ageMs: number, intervalMs: number): Freshness {
  const age = ageLabel(ageMs / 1000);
  return ageMs > staleAfter(intervalMs) ? { stale: true, label: `Data ${age} old` } : { stale: false, label: `Updated ${age} ago` };
}

/** The snapshot on show: when it arrived, and the refresh schedule it was fetched on (ms). */
export interface FeedClock {
  at: number;
  interval: number;
}

/**
 * After a refresh round. A snapshot that arrived restarts the clock, on the
 * schedule set for the round after it (backoff included). A round that failed
 * changes neither: a backoff raised by failures must not stretch the
 * threshold for data fetched on the shorter schedule, or old data would turn
 * quiet again with nothing new arriving.
 */
export function afterRound(shown: FeedClock | null, round: { arrived: boolean; at: number; interval: number }): FeedClock | null {
  return round.arrived ? { at: round.at, interval: round.interval } : shown;
}
