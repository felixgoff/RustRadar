// The land's colour as it really is, between the zoomed-out picture of the
// Earth and the drawn ground close in: EOX's Sentinel-2 cloudless mosaic of
// 2017 (CC BY 4.0), its tiles round the view gathered into one picture that
// the realistic ground (realistic-ground.ts) takes its colour from at the
// middle zooms, lit as the rest of it is.

import type { Device, Texture } from "@luma.gl/core";

export const IMAGERY_ATTRIBUTION = "Sentinel-2 cloudless 2017 by EOX IT Services GmbH (contains modified Copernicus Sentinel data 2017)";
const url = (z: number, x: number, y: number) => `https://tiles.maps.eox.at/wmts/1.0.0/s2cloudless-2017_3857/default/g/${z}/${y}/${x}.jpg`;
/** Tiles across the picture, and pixels a tile. */
const ACROSS = 8;
const TILE = 256;
const SIZE = ACROSS * TILE;
/** The mosaic's 10 m pixels are past use beyond this zoom; below the other, the Earth's own picture does. */
const MAX_ZOOM = 13;
const MIN_ZOOM = 5;
const MAX_LOADING = 6;

/** Where the picture lies, in deck.gl's web mercator units (512 round the world, y north): its west and north edges, and its size. */
export type ImageryRect = { west: number; north: number; size: number };

type Window = { z: number; x0: number; y0: number; rect: ImageryRect };
let window: Window | null = null;
let canvas: OffscreenCanvas | HTMLCanvasElement | null = null;
let dirty = false;
let loaded = 0;
const queue: { z: number; x: number; y: number }[] = [];
let loading = 0;
let onLoaded: () => void = () => {};

/** The window of tiles at `z` round a point: its corner tile, and where it lies. */
export function windowAt(lon: number, lat: number, z: number): Window {
  const n = 2 ** z;
  const r = (lat * Math.PI) / 180;
  const tx = ((lon + 180) / 360) * n;
  const ty = ((1 - Math.log(Math.tan(r) + 1 / Math.cos(r)) / Math.PI) / 2) * n;
  const x0 = Math.floor(tx - ACROSS / 2 + 0.5);
  const y0 = Math.max(0, Math.min(n - ACROSS, Math.floor(ty - ACROSS / 2 + 0.5)));
  return { z, x0, y0, rect: { west: (x0 / n) * 512, north: 512 - (y0 / n) * 512, size: (ACROSS / n) * 512 } };
}

function pump() {
  while (loading < MAX_LOADING && queue.length) {
    const t = queue.shift()!;
    const forWindow = window;
    loading++;
    fetch(url(t.z, ((t.x % 2 ** t.z) + 2 ** t.z) % 2 ** t.z, t.y))
      .then((r) => (r.ok ? r.blob() : null))
      .then((b) => (b ? createImageBitmap(b) : null))
      .then((image) => {
        if (!image) return;
        // still the picture it was asked for
        if (window === forWindow && canvas && window) {
          const context = canvas.getContext("2d") as CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D | null;
          context?.drawImage(image, (t.x - window.x0) * TILE, (t.y - window.y0) * TILE, TILE, TILE);
          dirty = true;
          loaded++;
        }
        image.close();
      })
      .catch(() => {})
      .finally(() => {
        loading--;
        if (dirty) onLoaded();
        pump();
      });
  }
}

/**
 * Gathers the picture round the view (at the view's zoom: twice a screen
 * across), anew when the view has moved off it; `onLoad` as its tiles come in.
 */
export function wantImagery(lon: number, lat: number, viewZoom: number, onLoad: () => void) {
  onLoaded = onLoad;
  const z = Math.min(MAX_ZOOM, Math.floor(viewZoom));
  if (z < MIN_ZOOM) return;
  const next = windowAt(lon, lat, z);
  // kept while the view is within its middle half
  if (window && window.z === z && Math.abs(next.x0 - window.x0) <= ACROSS / 4 && Math.abs(next.y0 - window.y0) <= ACROSS / 4) return;
  window = next;
  canvas ??= typeof OffscreenCanvas !== "undefined" ? new OffscreenCanvas(SIZE, SIZE) : Object.assign(document.createElement("canvas"), { width: SIZE, height: SIZE });
  canvas.getContext("2d")?.clearRect(0, 0, SIZE, SIZE);
  loaded = 0;
  dirty = true;
  queue.length = 0;
  // nearest the middle first
  const tiles: { z: number; x: number; y: number; d: number }[] = [];
  for (let j = 0; j < ACROSS; j++) for (let i = 0; i < ACROSS; i++) tiles.push({ z, x: next.x0 + i, y: next.y0 + j, d: Math.hypot(i - ACROSS / 2 + 0.5, j - ACROSS / 2 + 0.5) });
  queue.push(...tiles.sort((a, b) => a.d - b.d));
  pump();
}

const textures = new WeakMap<Device, Texture>();

/** The picture as a texture (brought up to date), and where it lies; null before any of it has come. */
export function imageryTexture(device: Device): { texture: Texture; rect: ImageryRect } | null {
  if (!window || !canvas || !loaded) return null;
  let texture = textures.get(device);
  if (!texture) {
    texture = device.createTexture({
      format: "rgba8unorm-srgb",
      width: SIZE,
      height: SIZE,
      mipLevels: Math.log2(SIZE) + 1,
      sampler: { addressModeU: "clamp-to-edge", addressModeV: "clamp-to-edge", magFilter: "linear", minFilter: "linear", mipmapFilter: "linear", maxAnisotropy: 8 },
    } as never);
    textures.set(device, texture);
    dirty = true;
  }
  if (dirty) {
    texture.copyExternalImage({ image: canvas as never, width: SIZE, height: SIZE });
    (texture as unknown as { generateMipmapsWebGL(): void }).generateMipmapsWebGL();
    dirty = false;
  }
  return { texture, rect: window.rect };
}
