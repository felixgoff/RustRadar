// A KTX2 worker: transcodes textures off the main thread for ktx2.ts.

import { transcode, transcoder, type Target } from "./ktx2-transcode";

/** `wasm`, the transcoder's, comes with a worker's first job. */
export type Ktx2Request = { id: number; wasm?: ArrayBuffer; bytes: ArrayBuffer; target: Target };
export type Ktx2Response = { id: number; result: ReturnType<typeof transcode> } | { id: number; error: string };

let wasm: ArrayBuffer | undefined;

self.onmessage = async ({ data }: MessageEvent<Ktx2Request>) => {
  const post = (message: Ktx2Response, transfer: ArrayBuffer[] = []) => (self as unknown as Worker).postMessage(message, transfer);
  wasm ??= data.wasm;
  try {
    const result = transcode(await transcoder(wasm!), new Uint8Array(data.bytes), data.target);
    post({ id: data.id, result }, result.levels.map((l) => l.data.buffer as ArrayBuffer));
  } catch (e) {
    post({ id: data.id, error: String(e) });
  }
};
