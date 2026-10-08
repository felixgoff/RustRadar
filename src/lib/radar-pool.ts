// The main thread's side of the radar workers (radar-worker.ts): a few of them,
// each handed whole jobs (a tile: fetch, decode, contour, triangulate; a
// measured volume: parse and triangulate), with results coming back as
// transferred typed arrays. Contouring a tile takes tens of milliseconds and
// dozens load at once as the view moves; on the main thread that was the lag.

import {
  coveragesOver,
  coveredBy,
  fetchPixels,
  processTile,
  processVolume,
  tileBounds,
  type Coverage,
  type RadarLevel,
  type RadarTile,
  type TileMesh,
  type VolumeFrame,
  type VolumeMesh,
} from "./radar";
import { polygonData, type PolygonData } from "./radar-mesh";
import type { WorkerRequest, WorkerResponse } from "./radar-worker";

type Slot = { worker: Worker; busy: number; dead: boolean };
type Pending = { resolve: (value: unknown) => void; reject: (error: Error) => void; slot: Slot };

let slots: Slot[] | null = null;
let nextId = 1;
const pending = new Map<number, Pending>();
let coverages: Coverage[] = [];

/** Two to four workers, leaving a core for the main thread; none if workers can't start. */
function pool(): Slot[] {
  if (slots) return slots.filter((s) => !s.dead);
  slots = [];
  const count = Math.max(2, Math.min(4, (navigator.hardwareConcurrency || 4) - 1));
  try {
    for (let i = 0; i < count; i++) {
      const worker = new Worker(new URL("./radar-worker.ts", import.meta.url), { type: "module", name: `radar-${i}` });
      const slot: Slot = { worker, busy: 0, dead: false };
      worker.onmessage = ({ data }: MessageEvent<WorkerResponse>) => {
        const job = pending.get(data.id);
        if (!job) return; // cancelled: the result is dropped
        pending.delete(data.id);
        slot.busy--;
        if ("error" in data) job.reject(new Error(data.error));
        else job.resolve(data.result);
      };
      // a worker that fails (to load, say) is dropped, and its jobs with it;
      // with none left, the work falls back to the main thread
      worker.onerror = (e) => {
        console.warn("radar worker failed:", e.message);
        slot.dead = true;
        worker.terminate();
        for (const [id, job] of pending) {
          if (job.slot !== slot) continue;
          pending.delete(id);
          job.reject(new Error("radar worker failed"));
        }
      };
      if (coverages.length) worker.postMessage({ type: "coverage", coverages } satisfies WorkerRequest);
      slots.push(slot);
    }
  } catch (e) {
    console.warn("radar workers unavailable, contouring on the main thread:", e);
    for (const { worker } of slots) worker.terminate();
    slots = [];
  }
  return slots.filter((s) => !s.dead);
}

/** Runs a job on the least busy worker; a cancelled one resolves to null at once. */
function run<T>(message: WorkerRequest & { id: number }, transfer: Transferable[], signal?: AbortSignal): Promise<T | null> {
  const slot = pool().reduce((a, b) => (b.busy < a.busy ? b : a));
  return new Promise<T | null>((resolve, reject) => {
    if (signal?.aborted) return resolve(null);
    pending.set(message.id, { resolve: resolve as (value: unknown) => void, reject, slot });
    slot.busy++;
    slot.worker.postMessage(message, transfer);
    signal?.addEventListener(
      "abort",
      () => {
        if (!pending.delete(message.id)) return;
        slot.busy--;
        slot.worker.postMessage({ type: "cancel", id: message.id } satisfies WorkerRequest);
        resolve(null);
      },
      { once: true },
    );
  });
}

/**
 * Where the measured volumes are: RainViewer tiles are blanked out there. A
 * tile is only contoured again when the coverage over it changes.
 */
export function setRadarCoverage(next: Coverage[]) {
  const key = (list: Coverage[]) => list.map((c) => c.key).join("|");
  if (key(next) === key(coverages)) return;
  coverages = next;
  for (const { worker } of pool()) worker.postMessage({ type: "coverage", coverages } satisfies WorkerRequest);
}

/**
 * A key for everything the tiles depend on besides their address: the
 * coverage. The tile layer reloads when it changes; tiles it doesn't reach
 * come straight from the cache.
 */
export const radarCoverageKey = () => coverages.map((c) => c.key).sort().join("-") || "full";

/** Finished tiles, so a reload for a change of coverage elsewhere costs nothing. */
const cache = new Map<string, RadarTile | null>();
const CACHE_SIZE = 96;
let cachedUrl = "";

function withGround(mesh: TileMesh | null): RadarTile | null {
  return mesh && { ...mesh, ground: polygonData(mesh.slices) };
}

export async function loadRadarTile(
  url: string,
  index: { x: number; y: number; z: number },
  signal?: AbortSignal,
): Promise<RadarTile | null> {
  if (url !== cachedUrl) {
    cache.clear(); // a new frame: nothing of the old one is wanted again
    cachedUrl = url;
  }
  const over = coveragesOver(tileBounds(index), coverages);
  const key = `${index.z}/${index.x}/${index.y}|${over.map((c) => c.key).join("|")}`;
  if (cache.has(key)) {
    const tile = cache.get(key)!;
    cache.delete(key); // most recently used goes last
    cache.set(key, tile);
    return tile;
  }
  const id = nextId++;
  let tile: RadarTile | null;
  if (pool().length) {
    const mesh = await run<TileMesh>({ type: "tile", id, url, index }, [], signal);
    if (signal?.aborted) return null;
    tile = withGround(mesh);
  } else {
    const image = await fetchPixels(url, signal);
    if (!image || signal?.aborted) return null;
    tile = withGround(processTile(image.pixels, image.width, image.height, index, coveredBy(over)));
  }
  cache.set(key, tile);
  if (cache.size > CACHE_SIZE) cache.delete(cache.keys().next().value!);
  return tile;
}

export interface MeasuredVolume {
  source: string;
  time: number;
  bounds: [number, number, number, number];
  ground: PolygonData | null;
  levels: RadarLevel[];
  coverage: Coverage;
}

/** Loads one measured volume from the backend, triangulated (in a worker) and ready for the GPU. */
export async function loadVolume(
  frame: VolumeFrame,
  image: (source: string, name: string) => Promise<ArrayBuffer>,
): Promise<MeasuredVolume> {
  const [ground, coverage, ...levels] = await Promise.all([
    image(frame.source, "ground"),
    image(frame.source, "coverage"),
    ...frame.levels.map((km) => image(frame.source, String(km))),
  ]);
  // plain data only: the frame may carry more than a worker can be sent
  const plain: VolumeFrame = {
    source: frame.source,
    time: frame.time,
    bounds: { ...frame.bounds },
    levels: [...frame.levels],
    coverageWidth: frame.coverageWidth,
    coverageHeight: frame.coverageHeight,
  };
  const mesh = pool().length
    ? await run<VolumeMesh>(
        { type: "volume", id: nextId++, frame: plain, ground, coverage, levels },
        [...new Set([ground, coverage, ...levels])],
      )
    : processVolume(plain, ground, coverage, levels);
  if (!mesh) throw new Error("radar volume job dropped");
  const { west, south, east, north } = frame.bounds;
  return {
    source: frame.source,
    time: frame.time,
    bounds: [west, south, east, north],
    ground: mesh.ground && polygonData(mesh.ground),
    levels: mesh.levels.map(({ km, slices, walls }) => ({ km, data: polygonData(slices)!, walls: walls && polygonData(walls) })),
    coverage: mesh.coverage,
  };
}
