// Trains on the map: colours and names per kind, motion between refreshes,
// and the credits each source asks for.
import type { Train, TrainKind, TrainSource } from "./api";

export type RGB = [number, number, number];

/**
 * Calm tones that read on the dark map, on hues the ship palette (ships.ts)
 * leaves free, and clear of the amber reserved for aircraft and selection.
 */
const KINDS: Record<TrainKind, { label: string; color: RGB }> = {
  highSpeed: { label: "High-speed", color: [226, 92, 196] }, // magenta
  intercity: { label: "Intercity", color: [128, 132, 246] }, // periwinkle
  regional: { label: "Regional", color: [96, 212, 160] }, // mint
  suburban: { label: "Suburban", color: [112, 190, 232] }, // cerulean
  metro: { label: "Metro", color: [178, 212, 88] }, // lime
  freight: { label: "Freight", color: [176, 142, 116] }, // taupe
  other: { label: "Other", color: [158, 150, 140] }, // warm grey
};

/** The kinds in display order (fast to slow, passengers first). */
export const TRAIN_KINDS = Object.keys(KINDS) as TrainKind[];

export const trainColor = (kind: TrainKind): RGB => (KINDS[kind] ?? KINDS.other).color;
export const trainLabel = (kind: TrainKind): string => (KINDS[kind] ?? KINDS.other).label;

/** The attribution each source asks for, and where it links. */
export const TRAIN_CREDITS: Record<TrainSource, { text: string; url: string }> = {
  digitraffic: {
    text: "Trains in Finland: Fintraffic / digitraffic.fi, CC BY 4.0",
    url: "https://www.digitraffic.fi/en/railway-traffic/",
  },
  // Amtraker asks for prominent credit linking amtraker.com (not tucked away in settings)
  amtrak: {
    text: "Trains in the US and Canada: Amtraker (amtraker.com), ODC-By 1.0",
    url: "https://amtraker.com",
  },
  entur: {
    text: "Trains in Norway: Entur, NLOD",
    url: "https://developer.entur.no/",
  },
  irishrail: {
    text: "Trains in Ireland: Iarnród Éireann realtime data, CC BY",
    url: "https://api.irishrail.ie/realtime/",
  },
  vbb: {
    text: "Trains in Berlin–Brandenburg: VBB via transport.rest",
    url: "https://v6.vbb.transport.rest/",
  },
};

/** The credits for the sources of these trains, once each. */
export const trainCredits = (trains: readonly Pick<Train, "source">[]) =>
  [...new Set(trains.map((t) => t.source))].filter((s) => s in TRAIN_CREDITS).map((s) => TRAIN_CREDITS[s]);

type Frames = Train["frames"];

/** The frames' segment around `nowMs`: indexes and how far along (0..1); null without one. */
function segment(frames: Frames, nowMs: number): [number, number, number] | null {
  if (frames.length === 0) return null;
  if (frames.length === 1 || nowMs <= frames[0][2]) return [0, 0, 0];
  const last = frames.length - 1;
  if (nowMs >= frames[last][2]) return [last, last, 0];
  let i = 0;
  while (i < last - 1 && frames[i + 1][2] <= nowMs) i++;
  const [, , t0] = frames[i];
  const [, , t1] = frames[i + 1];
  return [i, i + 1, t1 > t0 ? (nowMs - t0) / (t1 - t0) : 1];
}

/**
 * Where the train is at `nowMs` (Unix ms): along its frames when it has
 * them (held at the first before it starts and at the last after it ends,
 * never extrapolated), else its reported position.
 */
export function positionAt(train: Pick<Train, "lat" | "lon" | "frames">, nowMs: number): { lat: number; lon: number } {
  const s = segment(train.frames ?? [], nowMs);
  if (!s) return { lat: train.lat, lon: train.lon };
  const [i, j, f] = s;
  const [lat0, lon0] = train.frames[i];
  const [lat1, lon1] = train.frames[j];
  // a leg across the antimeridian goes the short way round
  let dLon = lon1 - lon0;
  if (dLon > 180) dLon -= 360;
  else if (dLon < -180) dLon += 360;
  let lon = lon0 + dLon * f;
  if (lon > 180) lon -= 360;
  else if (lon < -180) lon += 360;
  return { lat: lat0 + (lat1 - lat0) * f, lon };
}

const DEG = Math.PI / 180;

/** The direction of travel at `nowMs`: along the current frame leg, else the reported heading. */
export function headingAt(train: Pick<Train, "heading" | "frames">, nowMs: number): number | null {
  const frames = train.frames ?? [];
  const s = segment(frames, nowMs);
  if (s && frames.length > 1) {
    // the leg being travelled, or the last one once past the end
    const i = Math.min(s[0], frames.length - 2);
    const [lat0, lon0] = frames[i];
    const [lat1, lon1] = frames[i + 1];
    if (lat0 !== lat1 || lon0 !== lon1) {
      const y = Math.sin((lon1 - lon0) * DEG) * Math.cos(lat1 * DEG);
      const x =
        Math.cos(lat0 * DEG) * Math.sin(lat1 * DEG) -
        Math.sin(lat0 * DEG) * Math.cos(lat1 * DEG) * Math.cos((lon1 - lon0) * DEG);
      return ((Math.atan2(y, x) / DEG) % 360 + 360) % 360;
    }
  }
  return train.heading;
}

/** "5 min late", "2 min early", "on time"; empty when unknown. */
export function delayText(minutes: number | null): string {
  if (minutes === null || !Number.isFinite(minutes)) return "";
  if (minutes === 0) return "on time";
  return minutes > 0 ? `${minutes} min late` : `${-minutes} min early`;
}
