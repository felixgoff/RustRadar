// Clock times at an airport, delays and durations.

const formatters = new Map<string, Intl.DateTimeFormat>();

function formatter(options: Intl.DateTimeFormatOptions, timeZone?: string): Intl.DateTimeFormat {
  const key = JSON.stringify(options) + (timeZone ?? "");
  let f = formatters.get(key);
  if (!f) {
    try {
      f = new Intl.DateTimeFormat([], { ...options, timeZone: timeZone || undefined });
    } catch {
      // an unknown zone name: the viewer's own zone beats no time at all
      f = new Intl.DateTimeFormat([], options);
    }
    formatters.set(key, f);
  }
  return f;
}

/** `14:05` in an IANA zone (the viewer's own without one), or a dash. */
export function clockAt(unix: number | null | undefined, timeZone?: string): string {
  return unix ? formatter({ hour: "2-digit", minute: "2-digit" }, timeZone).format(unix * 1000) : "—";
}

/** Minutes past midnight in an IANA zone (the viewer's own without one). */
export function minutesOfDay(unix: number, timeZone?: string): number {
  const parts = formatter({ hour: "2-digit", minute: "2-digit", hourCycle: "h23" }, timeZone).formatToParts(unix * 1000);
  const get = (type: string) => Number(parts.find((p) => p.type === type)?.value ?? 0);
  return get("hour") * 60 + get("minute");
}

/** The zone's short name at that moment, e.g. `CEST` or `GMT+2`. */
export function zoneName(unix: number, timeZone?: string): string {
  const parts = formatter({ hour: "numeric", timeZoneName: "short" }, timeZone).formatToParts(unix * 1000);
  return parts.find((p) => p.type === "timeZoneName")?.value ?? "";
}

/** `Today`, `Yesterday`, `Tomorrow` or `Mon 6 Oct`, in an IANA zone. */
export function dayAt(unix: number, timeZone?: string, now = Date.now()): string {
  const day = formatter({ year: "numeric", month: "2-digit", day: "2-digit" }, timeZone);
  const offsets: [number, string][] = [
    [0, "Today"],
    [-1, "Yesterday"],
    [1, "Tomorrow"],
  ];
  const target = day.format(unix * 1000);
  for (const [offset, label] of offsets) {
    if (day.format(now + offset * 86_400_000) === target) return label;
  }
  return formatter({ weekday: "short", day: "numeric", month: "short" }, timeZone).format(unix * 1000);
}

/** `5 h 10 min`, `45 min`. */
export function duration(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.round((seconds % 3600) / 60);
  return h ? `${h} h ${String(m).padStart(2, "0")} min` : `${m} min`;
}

export type Tone = "good" | "warn" | "bad";

/** Minutes late (negative: early) against the schedule, when both times are known. */
export function delayMinutes(scheduled: number | null | undefined, actual: number | null | undefined): number | null {
  return scheduled && actual ? Math.round((actual - scheduled) / 60) : null;
}

/** Departure-board wording: on time within a quarter of an hour, as airlines count it. */
export function punctuality(minutes: number | null): { label: string; tone: Tone } | null {
  if (minutes === null) return null;
  if (minutes >= 45) return { label: `${duration(minutes * 60)} late`, tone: "bad" };
  if (minutes >= 15) return { label: `${minutes} min late`, tone: "warn" };
  if (minutes <= -5) return { label: `${-minutes} min early`, tone: "good" };
  return { label: "On time", tone: "good" };
}

/** Flightradar24's board status colours, as tones. */
export function toneOf(color: string | null | undefined): Tone | null {
  switch (color) {
    case "green":
      return "good";
    case "yellow":
    case "orange":
      return "warn";
    case "red":
      return "bad";
    default:
      return null;
  }
}
