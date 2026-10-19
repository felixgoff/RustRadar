// Textures from KTX2 files (scripts/compress-textures.mjs): transcoded in a
// worker (ktx2-worker.ts) to a compressed format the GPU samples as is, a
// quarter of the memory of plain pixels, with the files' own mip levels.
// BC7 where there is BC (desktops; first, as some Linux drivers offer ASTC
// but decode it on the CPU), else ASTC (phones, Apple's); plain pixels when
// neither. Not ETC2: luma.gl 9.4.2 gives its formats the wrong GL numbers.

import type { Device, Texture } from "@luma.gl/core";
import wasmUrl from "./vendor/basis_transcoder.wasm?url";
import { isKtx2, TARGET, transcode, transcoder, type Target, type Transcoded } from "./ktx2-transcode";
import type { Ktx2Request, Ktx2Response } from "./ktx2-worker";

async function bytesOf(url: string): Promise<ArrayBuffer> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: ${response.status}`);
  return response.arrayBuffer();
}

let wasm: Promise<ArrayBuffer> | null = null;
const transcoderWasm = () => (wasm ??= bytesOf(wasmUrl));

type Slot = { worker: Worker; busy: number; started: boolean };
let slots: Slot[] | null = null;
let nextId = 1;
const pending = new Map<number, { resolve: (t: Transcoded) => void; reject: (e: Error) => void }>();

/** Two workers; none if they can't start (the main thread transcodes then). */
function pool(): Slot[] {
  if (slots) return slots;
  slots = [];
  try {
    for (let i = 0; i < 2; i++) {
      const worker = new Worker(new URL("./ktx2-worker.ts", import.meta.url), { type: "module", name: `ktx2-${i}` });
      const slot: Slot = { worker, busy: 0, started: false };
      worker.onmessage = ({ data }: MessageEvent<Ktx2Response>) => {
        const job = pending.get(data.id);
        pending.delete(data.id);
        slot.busy--;
        if ("error" in data) job?.reject(new Error(data.error));
        else job?.resolve(data.result);
      };
      slots.push(slot);
    }
  } catch (e) {
    console.warn("texture workers unavailable, transcoding on the main thread:", e);
    for (const { worker } of slots) worker.terminate();
    slots = [];
  }
  return slots;
}

async function run(bytes: ArrayBuffer, target: Target): Promise<Transcoded> {
  const wasm = await transcoderWasm();
  const free = pool();
  if (!free.length) return transcode(await transcoder(wasm), new Uint8Array(bytes), target);
  const slot = free.reduce((a, b) => (b.busy < a.busy ? b : a));
  const id = nextId++;
  const message: Ktx2Request = { id, bytes, target, wasm: slot.started ? undefined : wasm };
  slot.started = true;
  slot.busy++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    slot.worker.postMessage(message, [bytes]);
  });
}

/** What the textures become on `device`: the transcoder's target and the texture's format. */
function formatOn(device: Device, srgb: boolean): { target: Target; format: string } {
  const s = srgb ? "-srgb" : "";
  if (device.features.has("texture-compression-bc7-webgl")) return { target: TARGET.bc7, format: `bc7-rgba-unorm${s}` };
  if (device.features.has("texture-compression-astc")) return { target: TARGET.astc, format: `astc-4x4-unorm${s}` };
  return { target: TARGET.rgba, format: `rgba8unorm${s}` };
}

/**
 * The texture in a KTX2 file: a texture array of its layers (`array`), or a
 * plain 2D one; its colours sRGB (`srgb`) or data.
 */
export async function loadKtx2(device: Device, url: string, { srgb, array, sampler }: { srgb: boolean; array: boolean; sampler?: object }): Promise<Texture> {
  const bytes = await bytesOf(url);
  if (!isKtx2(new Uint8Array(bytes, 0, Math.min(12, bytes.byteLength)))) throw new Error(`${url}: not a KTX2 file`);
  const { target, format } = formatOn(device, srgb);
  const { width, height, layers, levels } = await run(bytes, target);
  const depth = array ? layers : 1;
  const texture = device.createTexture({ dimension: array ? "2d-array" : "2d", format, width, height, depth, mipLevels: levels.length, sampler } as never);
  levels.forEach((l, mipLevel) => texture.writeData(l.data, { mipLevel, width: l.width, height: l.height, depthOrArrayLayers: depth }));
  // (luma.gl doesn't say where the levels end: a texture missing one would sample black)
  const gl = (device as unknown as { gl: WebGL2RenderingContext }).gl;
  const glTarget = array ? gl.TEXTURE_2D_ARRAY : gl.TEXTURE_2D;
  gl.bindTexture(glTarget, texture.handle as WebGLTexture);
  gl.texParameteri(glTarget, gl.TEXTURE_MAX_LEVEL, levels.length - 1);
  gl.bindTexture(glTarget, null);
  return texture;
}
