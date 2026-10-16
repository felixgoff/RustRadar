// Trains on the map: colours and names per kind, motion between refreshes,
// and the credits each source asks for.
import type { Train, TrainKind, TrainSource, TrainStop } from "./api";
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

/**
 * Operators with a colour of their own (Sweden's, as Trafikverket names them),
 * so a train is recognised by who runs it. Others take their kind's colour.
 */
const OPERATORS: { match: RegExp; color: RGB }[] = [
  { match: /^sj\b/i, color: [232, 232, 236] }, // silver
  { match: /^sl\b|storstockholms/i, color: [70, 146, 236] }, // blue
  { match: /mälartåg/i, color: [236, 84, 84] }, // red
  { match: /västtrafik|västtåg/i, color: [62, 200, 236] }, // sky
  { match: /skånetrafiken|pågatåg/i, color: [196, 92, 222] }, // purple
  { match: /öresundståg/i, color: [150, 132, 246] }, // lavender
  { match: /arlanda express/i, color: [250, 212, 64] }, // yellow
  { match: /snälltåget/i, color: [120, 214, 92] }, // green
  { match: /norrtåg/i, color: [58, 204, 176] }, // teal
  { match: /^vy\b/i, color: [240, 92, 150] }, // pink
  { match: /bergslagen/i, color: [244, 152, 56] }, // orange
  { match: /upptåget|^ul\b/i, color: [250, 120, 40] }, // tangerine
  { match: /östgöta/i, color: [214, 70, 130] }, // raspberry
  { match: /x-trafik|x-tåget/i, color: [150, 220, 60] }, // lime
  { match: /krösatåg|kronoberg|jönköping|kalmar|blekinge/i, color: [236, 120, 110] }, // coral
  { match: /värmland/i, color: [110, 180, 255] }, // cornflower
];

const operatorIndex = (operator: string | null | undefined) =>
  operator ? OPERATORS.findIndex((o) => o.match.test(operator.trim())) : -1;

/** The train's colour: its operator's, else its kind's. */
export function trainColorOf(train: Pick<Train, "kind" | "operator">): RGB {
  const i = operatorIndex(train.operator);
  return i >= 0 ? OPERATORS[i].color : trainColor(train.kind);
}

/** The attribution each source asks for, and where it links. */
export const TRAIN_CREDITS: Record<TrainSource, { text: string; url: string }> = {
  trafikverket: {
    text: "Trains in Sweden: Trafikverket, CC0",
    url: "https://data.trafikverket.se/",
  },
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
  trafikverket: "Trafikverket",
  digitraffic: "Digitraffic (Fintraffic)",
  amtrak: "Amtraker",
  entur: "Entur",
  irishrail: "Irish Rail",
  vbb: "VBB (transport.rest)",
};

// --- On the map ---------------------------------------------------------------

/** A train with a direction is a carriage with a nose; one without, a rounded square. Coloured by operator when it has a colour. */
export function trainIcon(kind: TrainKind, heading: number | null, operator?: string | null): string {
  return `${heading === null ? "box" : "car"}-${colourKey(kind, operator)}`;
}

/** The atlas row a train's colour is in: its operator's, else its kind's. */
function colourKey(kind: TrainKind, operator?: string | null): string {
  const i = operatorIndex(operator);
  return i >= 0 ? `op${i}` : kind in KINDS ? kind : "other";
}

/** One car of a train seen from above, lying on the ground: the leading one with a nose. */
export function trainCarIcon(kind: TrainKind, operator: string | null | undefined, lead: boolean): string {
  return `${lead ? "cab" : "coach"}-${colourKey(kind, operator)}`;
}

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
/**
 * A car as the map draws it further out, where it is a few pixels long: a
 * short, fat dash, so a train reads as its cars. Its cell's width, its own
 * width and length, and the room round it.
 */
const CAR_CELL_W = 18;
const CAR_W = 14;
const CAR_LEN = 36;
const CAR_PAD = 2;
/**
 * A car's icon's height in its atlas cell to the car's own length: the
 * cell has a little room round the car (IconLayer sizes the cell).
 */
export const CAR_ICON_CELL = (CAR_LEN + 2 * CAR_PAD) / CAR_LEN;
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
  // the carriage and the square, then a car and a leading car
  canvas.width = CELL * 2 + CAR_CELL_W * 2;
  // a row per kind's colour, then one per operator's
  const rows: [string, RGB][] = [
    ...TRAIN_KINDS.map((kind): [string, RGB] => [kind, trainColor(kind)]),
    ...OPERATORS.map((o, i): [string, RGB] => [`op${i}`, o.color]),
  ];
  canvas.height = CELL * rows.length;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const mapping: Record<string, object> = {};
  ctx.lineJoin = "round";
  rows.forEach(([kind, [r, g, b]], row) => {
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
    // a car: a short, fat dash with a dark edge, the leading one round at its front
    for (const lead of [false, true]) {
      const x = CELL * 2 + (lead ? CAR_CELL_W : 0);
      const [l, r, t, b] = [x + (CAR_CELL_W - CAR_W) / 2, x + (CAR_CELL_W + CAR_W) / 2, y + CAR_PAD, y + CAR_PAD + CAR_LEN];
      ctx.beginPath();
      if (lead) {
        ctx.moveTo(l, b);
        ctx.lineTo(l, t + CAR_W / 2);
        ctx.arc((l + r) / 2, t + CAR_W / 2, CAR_W / 2, Math.PI, 0);
        ctx.lineTo(r, b);
        ctx.closePath();
      } else {
        ctx.roundRect(l, t, CAR_W, CAR_LEN, 2);
      }
      ctx.lineWidth = 3;
      ctx.stroke();
      ctx.fill();
      mapping[`${lead ? "cab" : "coach"}-${kind}`] = { x, y, width: CAR_CELL_W, height: CAR_LEN + 2 * CAR_PAD, mask: false };
    }
  });
  return { atlas: canvas, mapping };
}

/** The delay's tone: a few minutes is ordinary for a train, past 5 worth noting, past 30 bad. */
export function delayTone(minutes: number | null): "good" | "warn" | "bad" | null {
  if (minutes === null || !Number.isFinite(minutes)) return null;
  return minutes >= 30 ? "bad" : minutes >= 5 ? "warn" : "good";
}

// --- Stops ---------------------------------------------------------------------

/** What a stop shows: its time (departure, or arrival at the last), as it now stands, and whether the train is past it. */
export interface StopView {
  /** Unix seconds: the actual time, else the estimate, else the plan. */
  time: number | null;
  planned: number | null;
  /** Minutes off the plan, when there's something to compare. */
  late: number | null;
  passed: boolean;
}

export function stopView(stop: Pick<TrainStop, "arrival" | "departure">): StopView {
  const t = stop.departure ?? stop.arrival;
  const planned = t?.planned ?? null;
  const time = t?.actual ?? t?.estimated ?? planned;
  const late = planned !== null && time !== null && time !== planned ? Math.round((time - planned) / 60) : null;
  const passed = Boolean(stop.departure ? stop.departure.actual : stop.arrival?.actual);
  return { time, planned, late, passed };
}

/** The first stop not yet passed, or -1 when the run is over (or nothing has happened yet). */
export function nextStop(stops: readonly Pick<TrainStop, "arrival" | "departure">[]): number {
  const views = stops.map(stopView);
  const lastPassed = views.findLastIndex((v) => v.passed);
  return lastPassed + 1 < stops.length ? lastPassed + 1 : -1;
}

/** The product a train runs as ("Öresundståg"), when its name carries one: Trafikverket's are "<product> <number>". */
export function trainProduct(train: Pick<Train, "source" | "name">): string | null {
  if (train.source !== "trafikverket" || train.name.startsWith("Train ")) return null;
  const product = train.name.replace(/\s+\S+$/, "").trim();
  return product && product !== train.name ? product : null;
}
