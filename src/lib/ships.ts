// Ships on the map: type colours and names, the true-scale hull outline, and
// the icon atlas, drawn at runtime so no image files are needed.
import type { Ship } from "./api";

export type RGB = [number, number, number];
type LonLat = [number, number];

/**
 * VesselFinder's type colours, in calm tones that read on the dark map and
 * stay clear of the amber reserved for aircraft and selection.
 */
const KINDS: Record<number, { label: string; color: RGB }> = {
  0: { label: "Unknown type", color: [140, 152, 168] },
  2: { label: "Tug or special craft", color: [72, 198, 194] },
  3: { label: "Passenger", color: [98, 152, 240] },
  4: { label: "Cargo", color: [116, 188, 112] },
  5: { label: "Fishing", color: [236, 128, 156] },
  6: { label: "Tanker", color: [218, 92, 92] },
  7: { label: "Search and rescue or law enforcement", color: [228, 234, 242] },
  8: { label: "Pleasure craft or sailing", color: [192, 136, 236] },
};
const OTHER = { label: "Other", color: KINDS[0].color };

/** The colour keys the icon atlas has a cell for. */
export const SHIP_COLOR_KEYS = Object.keys(KINDS).map(Number);
export const shipKindLabel = (kind: number) => (KINDS[kind] ?? OTHER).label;
export const shipColor = (kind: number): RGB => (KINDS[kind] ?? OTHER).color;
/** The atlas's colour key for a type: unlisted types share the unknown's grey. */
const colorKey = (kind: number) => (kind in KINDS ? kind : 0);

/** A pointed hull for a ship under way (heading known), a dot for one without. */
export const shipIcon = (ship: Pick<Ship, "kind" | "heading">) =>
  `${ship.heading === null ? "dot" : "hull"}-${colorKey(ship.kind)}`;

/** Icon size in pixels: modestly larger for larger ships, smaller for a heading-less dot. */
export function shipIconSize(ship: Pick<Ship, "size" | "heading">, zoom: number): number {
  const base = [15, 18, 22][ship.size] ?? 18;
  return base * shipIconScale(zoom) * (ship.heading === null ? 0.62 : 1);
}

/** Smaller zoomed out, where a couple of thousand ships share the screen; in steps, so sizes update rarely. */
export const shipIconScale = (zoom: number) => (zoom < 3 ? 0.7 : zoom < 5 ? 0.85 : zoom < 7 ? 0.92 : 1);

/** How far into a hull's length the bow's taper starts, from the bow. */
const BOW_TAPER = 0.2;
const DEG = Math.PI / 180;
const METRES_PER_DEGREE = 111_320;

/**
 * The hull's outline at true scale, `[lon, lat]` going round: the AIS
 * dimensions are metres from the reference point to bow, stern, port and
 * starboard, turned to `heading`. A pointed bow, square stern.
 */
export function hullPolygon(lon: number, lat: number, dims: readonly number[], heading: number): LonLat[] {
  const [bow, stern, port, stbd] = dims;
  const length = bow + stern;
  const h = heading * DEG;
  // forward and to starboard, as east and north
  const [fe, fn] = [Math.sin(h), Math.cos(h)];
  const [re, rn] = [Math.cos(h), -Math.sin(h)];
  const kx = 1 / (METRES_PER_DEGREE * Math.max(0.01, Math.cos(lat * DEG)));
  const ky = 1 / METRES_PER_DEGREE;
  const point = (right: number, ahead: number): LonLat => [
    lon + (right * re + ahead * fe) * kx,
    lat + (right * rn + ahead * fn) * ky,
  ];
  const shoulder = bow - length * BOW_TAPER;
  return [
    point((stbd - port) / 2, bow),
    point(stbd, shoulder),
    point(stbd, -stern),
    point(-port, -stern),
    point(-port, shoulder),
  ];
}

/** Cell size of the atlas, pixels; glyphs are drawn at twice their usual size for sharpness. */
const CELL = 64;
/** The app's navy ink, as the glyphs' border. */
const INK = "rgba(8, 12, 20, 0.9)";

/**
 * An atlas with a hull and a dot per type colour, each with a dark border,
 * coloured in (IconLayer `mask: false`; its colour's alpha still fades them).
 */
export function shipAtlas(): { atlas: HTMLCanvasElement; mapping: Record<string, object> } | null {
  if (typeof document === "undefined") return null;
  const keys = SHIP_COLOR_KEYS;
  const canvas = document.createElement("canvas");
  canvas.width = CELL * 2;
  canvas.height = CELL * keys.length;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const mapping: Record<string, object> = {};
  ctx.lineJoin = "round";
  keys.forEach((key, row) => {
    const [r, g, b] = shipColor(key);
    const fill = `rgb(${r}, ${g}, ${b})`;
    const y = row * CELL;
    // the hull points up (north); IconLayer turns it to the heading
    ctx.beginPath();
    ctx.moveTo(32, y + 4);
    ctx.lineTo(47, y + 24);
    ctx.lineTo(46, y + 59);
    ctx.lineTo(18, y + 59);
    ctx.lineTo(17, y + 24);
    ctx.closePath();
    ctx.fillStyle = fill;
    ctx.strokeStyle = INK;
    ctx.lineWidth = 5;
    ctx.stroke();
    ctx.fill();
    mapping[`hull-${key}`] = { x: 0, y, width: CELL, height: CELL, mask: false };
    ctx.beginPath();
    ctx.arc(CELL + 32, y + 32, 20, 0, Math.PI * 2);
    ctx.lineWidth = 8;
    ctx.stroke();
    ctx.fill();
    mapping[`dot-${key}`] = { x: CELL, y, width: CELL, height: CELL, mask: false };
  });
  return { atlas: canvas, mapping };
}
