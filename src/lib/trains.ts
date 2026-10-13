// Trains on the map: colours and names per kind, motion between refreshes,
// and the credits each source asks for.
import type { Train, TrainKind, TrainSource } from "./api";
import { ageLabel } from "./freshness";

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

/** A train's position counts as stale past this age (seconds): the feeds report every few seconds to a minute. */
export const TRAIN_STALE_S = 5 * 60;

/** `Position 40 s ago`, or past {@link TRAIN_STALE_S} `Position 12 min old`, in the warning tone. */
export function trainPositionAge(positionUnix: number, nowMs: number): { stale: boolean; label: string } {
  const age = Math.max(0, nowMs / 1000 - positionUnix);
  return age > TRAIN_STALE_S
    ? { stale: true, label: `Position ${ageLabel(age)} old` }
    : { stale: false, label: `Position ${ageLabel(age)} ago` };
}

/** What a source is called in the panel. */
export const TRAIN_SOURCE_NAMES: Record<TrainSource, string> = {
  digitraffic: "Digitraffic (Fintraffic)",
  amtrak: "Amtraker",
  entur: "Entur",
  irishrail: "Irish Rail",
  vbb: "VBB (transport.rest)",
};

// --- On the map ---------------------------------------------------------------

/** A train with a direction is a carriage with a nose; one without, a rounded square. */
export const trainIcon = (kind: TrainKind, heading: number | null) =>
  `${heading === null ? "box" : "car"}-${kind in KINDS ? kind : "other"}`;

/** Base icon length in pixels per kind: long-distance trains a little longer, metros shorter. */
const ICON_PX: Record<TrainKind, number> = {
  highSpeed: 24,
  intercity: 23,
  regional: 21,
  suburban: 20,
  metro: 18,
  freight: 22,
  other: 19,
};

/** Smaller zoomed out; in steps, so sizes update rarely. */
export const trainIconScale = (zoom: number) => (zoom < 4 ? 0.7 : zoom < 6 ? 0.82 : zoom < 9 ? 0.92 : 1);

/** Icon size in pixels (IconLayer `getSize`: the cell's height). */
export function trainIconSize(kind: TrainKind, heading: number | null, zoom: number): number {
  return (ICON_PX[kind] ?? ICON_PX.other) * trainIconScale(zoom) * (heading === null ? 0.8 : 1);
}

/** Cell size of the atlas, pixels; glyphs are drawn at about three times their usual size for sharpness. */
const CELL = 64;
/** The app's navy ink, as the glyphs' border and the carriage joints. */
const INK = "rgba(8, 12, 20, 0.9)";

/**
 * An atlas with a carriage and a square per kind colour, each with a dark
 * border (IconLayer `mask: false`; its colour's alpha still fades them). The
 * carriage points up (north): a rounded nose, a square tail, and two joints
 * across it so it reads as a train and not as a ship's hull.
 */
export function trainAtlas(): { atlas: HTMLCanvasElement; mapping: Record<string, object> } | null {
  if (typeof document === "undefined") return null;
  const canvas = document.createElement("canvas");
  canvas.width = CELL * 2;
  canvas.height = CELL * TRAIN_KINDS.length;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const mapping: Record<string, object> = {};
  ctx.lineJoin = "round";
  TRAIN_KINDS.forEach((kind, row) => {
    const [r, g, b] = trainColor(kind);
    const y = row * CELL;
    ctx.fillStyle = `rgb(${r}, ${g}, ${b})`;
    ctx.strokeStyle = INK;
    // the carriage: 26 wide, 56 long, a pointed, rounded nose
    const [left, right, top, bottom] = [19, 45, y + 4, y + 60];
    ctx.beginPath();
    ctx.moveTo(32, top);
    ctx.bezierCurveTo(40, top + 2, right, top + 9, right, top + 18);
    ctx.lineTo(right, bottom - 3);
    ctx.quadraticCurveTo(right, bottom, right - 3, bottom);
    ctx.lineTo(left + 3, bottom);
    ctx.quadraticCurveTo(left, bottom, left, bottom - 3);
    ctx.lineTo(left, top + 18);
    ctx.bezierCurveTo(left, top + 9, 24, top + 2, 32, top);
    ctx.closePath();
    ctx.lineWidth = 5;
    ctx.stroke();
    ctx.fill();
    // joints between the cars
    ctx.lineWidth = 2.5;
    ctx.beginPath();
    for (const j of [top + 25, top + 41]) {
      ctx.moveTo(left + 1, j);
      ctx.lineTo(right - 1, j);
    }
    ctx.stroke();
    mapping[`car-${kind}`] = { x: 0, y, width: CELL, height: CELL, mask: false };
    // no heading: a rounded square
    ctx.beginPath();
    ctx.roundRect(CELL + 14, y + 14, 36, 36, 9);
    ctx.lineWidth = 6;
    ctx.stroke();
    ctx.fill();
    mapping[`box-${kind}`] = { x: CELL, y, width: CELL, height: CELL, mask: false };
  });
  return { atlas: canvas, mapping };
}

/** The delay's tone: a few minutes is ordinary for a train, past 5 worth noting, past 30 bad. */
export function delayTone(minutes: number | null): "good" | "warn" | "bad" | null {
  if (minutes === null || !Number.isFinite(minutes)) return null;
  return minutes >= 30 ? "bad" : minutes >= 5 ? "warn" : "good";
}
