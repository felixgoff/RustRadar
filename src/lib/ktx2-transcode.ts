// A KTX2 file of Basis Universal data (scripts/compress-textures.mjs makes
// them) turned into one format the GPU takes, all its mip levels: the
// transcoder's half of ktx2.ts, which runs it in ktx2-worker.ts.

import BASIS from "./vendor/basis_transcoder.mjs";

/** The transcoder's formats (transcoder_texture_format) the app uploads. */
export const TARGET = { bc7: 6, astc: 10, rgba: 13 } as const;
export type Target = (typeof TARGET)[keyof typeof TARGET];

/** Its texels, all its layers in one, layer after layer, for each mip level (as `Texture.writeData` takes them). */
export type Transcoded = { width: number; height: number; layers: number; levels: { width: number; height: number; data: Uint8Array }[] };

// a KTX2 file's first bytes (Tauri answers a missing file with the page, as found)
const KTX2 = [0xab, 0x4b, 0x54, 0x58, 0x20, 0x32, 0x30, 0xbb, 0x0d, 0x0a, 0x1a, 0x0a];
export const isKtx2 = (bytes: Uint8Array) => KTX2.every((b, i) => bytes[i] === b);

type Module = Awaited<ReturnType<typeof BASIS>>;
let module: Promise<Module> | null = null;

/** The transcoder, from its wasm's bytes (fetched on the main thread: no fetching inside a worker). */
export function transcoder(wasm: ArrayBuffer): Promise<Module> {
  module ??= BASIS({ wasmBinary: wasm }).then((m) => {
    m.initializeBasis();
    return m;
  });
  return module;
}

export function transcode(basis: Module, bytes: Uint8Array, target: Target): Transcoded {
  if (!isKtx2(bytes)) throw new Error("not a KTX2 file");
  const file = new basis.KTX2File(bytes);
  try {
    if (!file.startTranscoding()) throw new Error("not a texture the transcoder reads");
    const layers = Math.max(1, file.getLayers());
    const levels = Array.from({ length: file.getLevels() }, (_, level) => {
      const { origWidth: width, origHeight: height } = file.getImageLevelInfo(level, 0, 0);
      const size = file.getImageTranscodedSizeInBytes(level, 0, 0, target);
      const data = new Uint8Array(size * layers);
      for (let layer = 0; layer < layers; layer++) {
        // (a truncated file reads, then fails here)
        if (!file.transcodeImage(data.subarray(layer * size, (layer + 1) * size), level, layer, 0, target, 0, -1, -1)) {
          throw new Error(`level ${level} of layer ${layer} didn't transcode`);
        }
      }
      return { width, height, data };
    });
    return { width: file.getWidth(), height: file.getHeight(), layers, levels };
  } finally {
    file.close();
    file.delete();
  }
}
