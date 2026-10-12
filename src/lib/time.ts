// Clock times at an airport, delays and durations. Clocks are 24-hour with the
// zone's abbreviation, the way boards and planespotters write them: `13:25 BST`.

const formatters = new Map<string, Intl.DateTimeFormat>();

/** Cached: these run for every board row and chart tick. `locale` "" is the viewer's own. */
function formatter(options: Intl.DateTimeFormatOptions, timeZone?: string, locale = ""): Intl.DateTimeFormat {
  const key = `${locale}|${timeZone ?? ""}|${JSON.stringify(options)}`;
  let f = formatters.get(key);
  if (!f) {
    const locales = locale ? [locale] : [];
    try {
      f = new Intl.DateTimeFormat(locales, { ...options, timeZone: timeZone || undefined });
    } catch {
      // an unknown zone name: the viewer's own zone beats no time at all
      f = new Intl.DateTimeFormat(locales, options);
    }
    formatters.set(key, f);
  }
  return f;
}

// en-GB so every viewer reads `13:25` in Latin digits, whatever their own locale
const CLOCK: Intl.DateTimeFormatOptions = { hour: "2-digit", minute: "2-digit", hourCycle: "h23" };
const clock = (timeZone?: string) => formatter(CLOCK, timeZone, "en-GB");

function part(parts: Intl.DateTimeFormatPart[], type: Intl.DateTimeFormatPartTypes): number {
  return Number(parts.find((p) => p.type === type)?.value ?? 0);
}

/** `13:25`, 24-hour, in an IANA zone (the viewer's own without one), or a dash. */
export function clockAt(unix: number | null | undefined, timeZone?: string): string {
  return unix ? clock(timeZone).format(unix * 1000) : "—";
}

/** `13:25 BST`: the clock time with its zone's name at that moment, or a dash. */
export function clockWithZone(unix: number | null | undefined, timeZone?: string): string {
  return unix ? `${clockAt(unix, timeZone)} ${zoneName(unix, timeZone)}` : "—";
}

/** Minutes past midnight in an IANA zone (the viewer's own without one); agrees with `clockAt`. */
export function minutesOfDay(unix: number, timeZone?: string): number {
  const parts = clock(timeZone).formatToParts(unix * 1000);
  return (part(parts, "hour") * 60 + part(parts, "minute")) % 1440;
}

const WALL: Intl.DateTimeFormatOptions = {
  year: "numeric",
  month: "numeric",
  day: "numeric",
  hour: "numeric",
  minute: "numeric",
  second: "numeric",
  hourCycle: "h23",
};

/** The zone's offset from UTC at that moment, in minutes east. */
function utcOffset(unix: number, timeZone?: string): number {
  const ms = unix * 1000;
  const parts = formatter(WALL, timeZone, "en-US").formatToParts(ms);
  const wall = Date.UTC(
    part(parts, "year"),
    part(parts, "month") - 1,
    part(parts, "day"),
    part(parts, "hour") % 24,
    part(parts, "minute"),
    part(parts, "second"),
  );
  return Math.round((wall - ms) / 60_000);
}

/** `UTC+3`, `UTC+5:45`, `UTC−4`, or `UTC` at zero. */
function utcLabel(minutes: number): string {
  if (!minutes) return "UTC";
  const abs = Math.abs(minutes);
  const m = abs % 60;
  return `UTC${minutes < 0 ? "−" : "+"}${Math.floor(abs / 60)}${m ? `:${String(m).padStart(2, "0")}` : ""}`;
}

/**
 * Each locale abbreviates only the zones its readers know (en-US: EDT, PDT;
 * en-GB: BST, CEST; ja-JP: JST) and writes `GMT+1` for the rest, so the first
 * alphabetic answer down this list is the zone's own name. Locales that would
 * answer in another language (de-DE: MESZ) are left out.
 */
const NAMING_LOCALES = ["en-US", "en-GB", "en-CA", "en-AU", "en-NZ", "en-IE", "en-IN", "en-ZA", "en-SG", "en-HK", "ja-JP", "pt-BR", "es-AR"];
const SHORT: Intl.DateTimeFormatOptions = { hour: "numeric", timeZoneName: "short" };
const ABBREVIATION = /^[A-Z]{2,6}$/;
const zoneNames = new Map<string, string>();

/**
 * Zones that keep another zone's clock and should carry its name. Locale data
 * gives a name such as BST to Europe/London alone, so Jersey in summer would
 * otherwise read `UTC+1` beside London's `BST` for the same clock. Old IANA
 * names, which airport data still uses, point at their current ones.
 */
const SAME_CLOCK: Record<string, string> = {
  "Europe/Jersey": "Europe/London",
  "Europe/Guernsey": "Europe/London",
  "Europe/Isle_of_Man": "Europe/London",
  "Europe/Belfast": "Europe/London",
  "Asia/Saigon": "Asia/Ho_Chi_Minh",
  "Asia/Rangoon": "Asia/Yangon",
  "Asia/Katmandu": "Asia/Kathmandu",
  "Asia/Istanbul": "Europe/Istanbul",
};

/** Names some locale gives to another zone too, kept for the one zone travellers know by it (Chromium also calls Dhaka `BST`). */
const OWNED: Record<string, string> = { BST: "Europe/London" };

/**
 * Names in common English use that no locale above supplies, by zone and
 * offset (minutes east), so summer time keeps its own name. Left out where
 * the name would be ambiguous: China's and Taiwan's CST, the Gulf's AST,
 * Israel's IST; those keep their UTC offset.
 */
const COMMON_NAMES: Record<string, Record<number, string>> = {
  "Asia/Seoul": { 540: "KST" },
  "Europe/Moscow": { 180: "MSK" },
  "Europe/Istanbul": { 180: "TRT" },
  "Europe/Samara": { 240: "SAMT" },
  "Asia/Manila": { 480: "PHT" },
  "Asia/Bangkok": { 420: "ICT" },
  "Asia/Ho_Chi_Minh": { 420: "ICT" },
  "Asia/Jakarta": { 420: "WIB" },
  "Asia/Karachi": { 300: "PKT" },
  "Asia/Tashkent": { 300: "UZT" },
  "Asia/Kathmandu": { 345: "NPT" },
  "Asia/Yangon": { 390: "MMT" },
  "Asia/Tehran": { 210: "IRST" },
  "Asia/Baku": { 240: "AZT" },
  "Asia/Tbilisi": { 240: "GET" },
  "America/Bogota": { [-300]: "COT" },
  "America/Lima": { [-300]: "PET" },
  "America/Guayaquil": { [-300]: "ECT" },
  "America/Caracas": { [-240]: "VET" },
  "America/La_Paz": { [-240]: "BOT" },
  "America/Santiago": { [-240]: "CLT", [-180]: "CLST" },
  "America/Montevideo": { [-180]: "UYT" },
  "America/Asuncion": { [-180]: "PYT" },
  "Pacific/Fiji": { 720: "FJT" },
};

let viewerZone: string | undefined;
/** The viewer's own IANA zone, for naming times given without one. */
function ownZone(): string {
  viewerZone ??= new Intl.DateTimeFormat().resolvedOptions().timeZone ?? "";
  return viewerZone;
}

const zones = new Map<string, boolean>();
/** Whether this engine knows the zone; an unknown one is read as the viewer's own, as `formatter` does. */
function known(timeZone: string): boolean {
  let ok = zones.get(timeZone);
  if (ok === undefined) {
    try {
      new Intl.DateTimeFormat("en", { timeZone });
      ok = true;
    } catch {
      ok = false;
    }
    zones.set(timeZone, ok);
  }
  return ok;
}

function abbreviation(unix: number, timeZone: string | undefined, zone: string): string | null {
  let gmt = false;
  for (const locale of NAMING_LOCALES) {
    const parts = formatter(SHORT, timeZone, locale).formatToParts(unix * 1000);
    const name = parts.find((p) => p.type === "timeZoneName")?.value ?? "";
    if (!ABBREVIATION.test(name)) continue;
    if (OWNED[name] && OWNED[name] !== zone) continue;
    // a locale also prints plain `GMT` for any zero-offset zone it has no name
    // for (Lisbon in winter, which en-GB calls WET), so a real name wins
    if (name === "GMT") gmt = true;
    else return name;
  }
  return gmt ? "GMT" : null;
}

/**
 * The zone's abbreviation at that moment, e.g. `BST`, `CEST`, `EDT`, `JST`,
 * `KST`; `UTC+3` where it has none in common, unambiguous use.
 */
export function zoneName(unix: number, timeZone?: string): string {
  const offset = utcOffset(unix, timeZone);
  const key = `${timeZone ?? ""}|${offset}`;
  let name = zoneNames.get(key);
  if (name === undefined) {
    const own = timeZone && known(timeZone) ? timeZone : ownZone();
    const zone = SAME_CLOCK[own] ?? own;
    // named by the zone it shares a clock with, read at the same offset
    const named = zone === own ? timeZone : zone;
    name = abbreviation(unix, named, zone) ?? COMMON_NAMES[zone]?.[offset] ?? utcLabel(offset);
    zoneNames.set(key, name);
  }
  return name;
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
