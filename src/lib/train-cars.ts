/** A train's cars, laid along the track behind where it is drawn. */

/** How long a car is drawn, metres (a coach with its coupling); as `CAR_M` in the backend. */
export const CAR_M = 26;
/** The gap drawn between cars, metres. */
const GAP_M = 2.5;
/** A train's cars where its operator doesn't say how many. */
export const DEFAULT_CARS = 4;

type LatLon = [number, number];

function metres(a: LatLon, b: LatLon): number {
  const k = Math.cos((a[0] * Math.PI) / 180);
  return Math.hypot((b[0] - a[0]) * 111_320, (b[1] - a[1]) * 111_320 * k);
}

/**
 * Each car of a train as a `[lon, lat]` line, front first: back from where
 * it is drawn at `nowMs` along its frames (`[lat, lon, ms]`), then along
 * `tail` (the track behind where its frames start, `[lat, lon]`), and
 * straight on back where that runs out.
 */
export function carLines(
  frames: [number, number, number][],
  tail: LatLon[],
  cars: number,
  nowMs: number,
): [number, number][][] {
  if (!frames.length) return [];
  // the way it has come and goes: the tail reversed, up to the frames' start, then the frames
  const path: LatLon[] = [...tail.slice(1).reverse(), [frames[0][0], frames[0][1]]];
  const lead = path.length - 1;
  for (const f of frames.slice(1)) {
    const last = path[path.length - 1];
    if (f[0] !== last[0] || f[1] !== last[1]) path.push([f[0], f[1]]);
  }
  const at: number[] = [0];
  for (let i = 1; i < path.length; i++) at.push(at[i - 1] + metres(path[i - 1], path[i]));
  // how far along the head is now
  let head = at[lead];
  for (let i = 1; i < frames.length; i++) {
    const [a, b] = [frames[i - 1], frames[i]];
    if (nowMs <= a[2]) break;
    const k = b[2] > a[2] ? Math.min(1, (nowMs - a[2]) / (b[2] - a[2])) : 1;
    head += metres([a[0], a[1]], [b[0], b[1]]) * k;
    if (nowMs < b[2]) break;
  }
  // the point `s` metres along (before the start: straight on back along its first stretch)
  const point = (s: number): LatLon => {
    if (path.length < 2) return path[0];
    if (s <= 0) {
      const [p, q] = [path[0], path[1]];
      const k = at[1] > 0 ? s / at[1] : 0;
      return [p[0] + (q[0] - p[0]) * k, p[1] + (q[1] - p[1]) * k];
    }
    let i = 1;
    while (i < path.length - 1 && at[i] < s) i++;
    const span = at[i] - at[i - 1];
    const k = span > 0 ? Math.min(1, (s - at[i - 1]) / span) : 1;
    return [path[i - 1][0] + (path[i][0] - path[i - 1][0]) * k, path[i - 1][1] + (path[i][1] - path[i - 1][1]) * k];
  };
  const out: [number, number][][] = [];
  for (let c = 0; c < Math.max(1, cars); c++) {
    const [front, back] = [head - c * CAR_M, head - (c + 1) * CAR_M + GAP_M];
    const line: LatLon[] = [point(back)];
    for (let i = 0; i < path.length; i++) if (at[i] > back && at[i] < front) line.push(path[i]);
    line.push(point(front));
    out.push(line.map(([lat, lon]) => [lon, lat]));
  }
  return out;
}
