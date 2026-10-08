// A radar worker: fetches, decodes and contours RainViewer tiles, and
// triangulates the measured volumes, off the main thread. radar-pool.ts runs a
// few of these and speaks the protocol below.

import { coveredBy, fetchPixels, processTile, processVolume, transferables, type Coverage, type Covered, type VolumeFrame } from "./radar";

export type WorkerRequest =
  | { type: "coverage"; coverages: Coverage[] }
  | { type: "tile"; id: number; url: string; index: { x: number; y: number; z: number } }
  | { type: "volume"; id: number; frame: VolumeFrame; ground: ArrayBuffer; coverage: ArrayBuffer; levels: ArrayBuffer[] }
  | { type: "cancel"; id: number };

export type WorkerResponse = { id: number; result: unknown } | { id: number; error: string };

let covered: Covered | undefined;
const running = new Map<number, AbortController>();

const post = (message: WorkerResponse, transfer: ArrayBuffer[] = []) =>
  (self as unknown as Worker).postMessage(message, transfer);

self.onmessage = async ({ data }: MessageEvent<WorkerRequest>) => {
  switch (data.type) {
    case "coverage":
      covered = coveredBy(data.coverages);
      return;
    case "cancel":
      running.get(data.id)?.abort();
      return;
    case "tile": {
      const controller = new AbortController();
      running.set(data.id, controller);
      // the coverage as the job was sent: a newer one may arrive while it loads
      const mask = covered;
      try {
        const image = await fetchPixels(data.url, controller.signal);
        // a tile the view has left is not worth contouring
        const result =
          image && !controller.signal.aborted ? processTile(image.pixels, image.width, image.height, data.index, mask) : null;
        post({ id: data.id, result }, transferables(result));
      } catch (e) {
        post({ id: data.id, error: String(e) });
      } finally {
        running.delete(data.id);
      }
      return;
    }
    case "volume":
      try {
        const result = processVolume(data.frame, data.ground, data.coverage, data.levels);
        post({ id: data.id, result }, transferables(result));
      } catch (e) {
        post({ id: data.id, error: String(e) });
      }
      return;
  }
};
