// An airport's taxiway network (OpenStreetMap centrelines) indexed for quick
// "which lines pass near this point" lookups: the motion model asks it for
// every taxiing aircraft every frame, so the lines are cut into straight
// segments in a local east/north metre frame and bucketed in a grid.
import type { TaxiLine } from "./api";

const DEG = Math.PI / 180;
const M_PER_DEG = 6_371_000 * DEG;
/** Grid cell size, m. Each segment is listed in the cells within one cell of it, so lookups reach at least this far. */
const CELL_M = 50;
/** Points further than this from the airport's lines are outside the network, m. */
const MARGIN_M = 100;

const cellKey = (cx: number, cy: number) => (cx + 32_768) * 65_536 + (cy + 32_768);

export class TaxiNetwork {
  /** The local frame's origin and scale (m per degree of longitude). */
  readonly lat0: number;
  readonly lon0: number;
  private readonly kx: number;
  /** Segments: start (ax, ay), unit direction (ux, uy) and length, m. */
  readonly ax: Float64Array;
  readonly ay: Float64Array;
  readonly ux: Float64Array;
  readonly uy: Float64Array;
  readonly len: Float64Array;
  readonly count: number;
  private readonly cells = new Map<number, number[]>();
  private minX = Infinity;
  private minY = Infinity;
  private maxX = -Infinity;
  private maxY = -Infinity;

  constructor(lines: readonly TaxiLine[]) {
    let sLat = 0;
    let sLon = 0;
    let n = 0;
    for (const line of lines)
      for (const [lon, lat] of line.points) {
        sLat += lat;
        sLon += lon;
        n++;
      }
    this.lat0 = n ? sLat / n : 0;
    this.lon0 = n ? sLon / n : 0;
    this.kx = M_PER_DEG * Math.max(Math.cos(this.lat0 * DEG), 1e-3);
    const ax: number[] = [];
    const ay: number[] = [];
    const ux: number[] = [];
    const uy: number[] = [];
    const len: number[] = [];
    for (const line of lines) {
      for (let i = 1; i < line.points.length; i++) {
        const [lonA, latA] = line.points[i - 1];
        const [lonB, latB] = line.points[i];
        const xa = this.x(lonA);
        const ya = (latA - this.lat0) * M_PER_DEG;
        const xb = this.x(lonB);
        const yb = (latB - this.lat0) * M_PER_DEG;
        const l = Math.hypot(xb - xa, yb - ya);
        if (!(l > 0.1) || !Number.isFinite(l)) continue;
        const seg = ax.length;
        ax.push(xa);
        ay.push(ya);
        ux.push((xb - xa) / l);
        uy.push((yb - ya) / l);
        len.push(l);
        this.minX = Math.min(this.minX, xa, xb);
        this.maxX = Math.max(this.maxX, xa, xb);
        this.minY = Math.min(this.minY, ya, yb);
        this.maxY = Math.max(this.maxY, ya, yb);
        // listed in every cell within one of the cells it passes through
        const steps = Math.ceil(l / (CELL_M / 2));
        for (let k = 0; k <= steps; k++) {
          const t = k / steps;
          const cx = Math.floor((xa + (xb - xa) * t) / CELL_M);
          const cy = Math.floor((ya + (yb - ya) * t) / CELL_M);
          for (let i2 = -1; i2 <= 1; i2++)
            for (let j2 = -1; j2 <= 1; j2++) {
              const key = cellKey(cx + i2, cy + j2);
              const list = this.cells.get(key);
              if (!list) this.cells.set(key, [seg]);
              else if (list[list.length - 1] !== seg) list.push(seg);
            }
        }
      }
    }
    this.ax = Float64Array.from(ax);
    this.ay = Float64Array.from(ay);
    this.ux = Float64Array.from(ux);
    this.uy = Float64Array.from(uy);
    this.len = Float64Array.from(len);
    this.count = ax.length;
  }

  /** East of the origin, m, for a longitude (across the antimeridian too). */
  x(lon: number): number {
    return ((((lon - this.lon0 + 540) % 360) + 360) % 360 - 180) * this.kx;
  }

  y(lat: number): number {
    return (lat - this.lat0) * M_PER_DEG;
  }

  /** Whether a local point is near enough to the lines to look them up. */
  covers(x: number, y: number): boolean {
    return x >= this.minX - MARGIN_M && x <= this.maxX + MARGIN_M && y >= this.minY - MARGIN_M && y <= this.maxY + MARGIN_M;
  }

  /** The segments that may pass within `CELL_M` of a local point (and some further), or undefined. */
  near(x: number, y: number): readonly number[] | undefined {
    return this.cells.get(cellKey(Math.floor(x / CELL_M), Math.floor(y / CELL_M)));
  }

  /** Metres per degree of longitude in the local frame. */
  get metresPerDegLon(): number {
    return this.kx;
  }
}

// Results of `project`.
export let projX = 0;
export let projY = 0;
export let projDist = 0;

/** The point of segment `s` nearest to (x, y), into projX/projY, and its distance, into projDist. */
export function project(net: TaxiNetwork, s: number, x: number, y: number): void {
  const ax = net.ax[s];
  const ay = net.ay[s];
  const ux = net.ux[s];
  const uy = net.uy[s];
  let t = (x - ax) * ux + (y - ay) * uy;
  const l = net.len[s];
  t = t < 0 ? 0 : t > l ? l : t;
  projX = ax + ux * t;
  projY = ay + uy * t;
  projDist = Math.hypot(x - projX, y - projY);
}
