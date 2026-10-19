// What the app uses of basis_transcoder.mjs.

type KTX2File = {
  startTranscoding(): number;
  getWidth(): number;
  getHeight(): number;
  getLevels(): number;
  getLayers(): number;
  getImageLevelInfo(level: number, layer: number, face: number): { origWidth: number; origHeight: number };
  getImageTranscodedSizeInBytes(level: number, layer: number, face: number, format: number): number;
  transcodeImage(dst: Uint8Array, level: number, layer: number, face: number, format: number, getAlphaForOpaque: number, channel0: number, channel1: number): number;
  close(): void;
  delete(): void;
};

type BasisModule = {
  initializeBasis(): void;
  KTX2File: new (bytes: Uint8Array) => KTX2File;
};

declare function BASIS(options: { wasmBinary: ArrayBuffer }): Promise<BasisModule>;
export default BASIS;
