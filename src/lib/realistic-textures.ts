// The realistic map's textures: Streets GL's (github.com/StrandedKitty/streets-gl,
// MIT licence, © StrandedKitty; static/realistic/LICENSE-streets-gl.txt), built
// into static/realistic/ by scripts/realistic-textures.py. Each set is three
// texture arrays, a layer per surface: its colour (sRGB, and in its alpha the
// glow of lit windows), its normal map, and its mask (roughness, metalness, and
// where the building's own colour tints it).

import type { Device, Texture } from "@luma.gl/core";

/** The buildings' layers, in the order the shaders index them. */
export const BUILDING_TEXTURES = [
  "plaster-wall",
  "plaster-window",
  "brick-wall",
  "brick-window",
  "block-wall",
  "block-window",
  "wood-wall",
  "wood-window",
  "glass",
  "roof-tar",
  "roof-concrete",
  "roof-metal",
  "roof-tiles",
  "roof-eternit",
  "roof-generic1",
  "roof-generic3",
] as const;

/** The ground's layers, in the order the shaders index them. */
export const GROUND_TEXTURES = [
  "grass",
  "park",
  "forest",
  "farmland0",
  "farmland1",
  "farmland2",
  "garden",
  "sand",
  "rock",
  "soil",
  "pavement",
  "asphalt",
  "road",
  "road-unmarked",
  "gravel",
  "railway",
  "cobblestone",
  "dirt",
] as const;

export const textureLayer = {
  building: (name: (typeof BUILDING_TEXTURES)[number]) => BUILDING_TEXTURES.indexOf(name),
  ground: (name: (typeof GROUND_TEXTURES)[number]) => GROUND_TEXTURES.indexOf(name),
};

export type TextureArrays = { color: Texture; normal: Texture; mask: Texture };
export type RealisticTextures = { buildings: TextureArrays; ground: TextureArrays; water: Texture; ready: boolean };

const BASE = "/realistic/";
const SIZE = 512;
const MIP_LEVELS = 10;
const SAMPLER = {
  addressModeU: "repeat",
  addressModeV: "repeat",
  magFilter: "linear",
  minFilter: "linear",
  mipmapFilter: "linear",
  maxAnisotropy: 8,
} as const;

/** A texture array of `layers` images (or a 1×1 stand-in of `fill` until they load). */
function textureArray(device: Device, images: ImageBitmap[] | null, srgb: boolean, fill: [number, number, number, number]): Texture {
  const format = srgb ? "rgba8unorm-srgb" : "rgba8unorm";
  if (!images) {
    return device.createTexture({
      dimension: "2d-array",
      format,
      width: 1,
      height: 1,
      depth: 1,
      sampler: SAMPLER,
      data: new Uint8Array(fill),
    } as never);
  }
  const texture = device.createTexture({
    dimension: "2d-array",
    format,
    width: SIZE,
    height: SIZE,
    depth: images.length,
    mipLevels: MIP_LEVELS,
    sampler: SAMPLER,
  } as never);
  images.forEach((image, z) => texture.copyExternalImage({ image, z, width: SIZE, height: SIZE, depth: 1 }));
  (texture as unknown as { generateMipmapsWebGL(): void }).generateMipmapsWebGL();
  return texture;
}

/** An image the right way up for the shaders (v up the image), its pixels exactly as stored. */
async function bitmap(url: string): Promise<ImageBitmap> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: ${response.status}`);
  return createImageBitmap(await response.blob(), {
    imageOrientation: "flipY",
    premultiplyAlpha: "none",
    colorSpaceConversion: "none",
  });
}

const STAND_IN = {
  color: [255, 255, 255, 0] as [number, number, number, number],
  normal: [128, 128, 255, 255] as [number, number, number, number],
  // rough, not metal, tinted all over
  mask: [235, 0, 255, 255] as [number, number, number, number],
};

function standIns(device: Device): TextureArrays {
  return {
    color: textureArray(device, null, true, STAND_IN.color),
    normal: textureArray(device, null, false, STAND_IN.normal),
    mask: textureArray(device, null, false, STAND_IN.mask),
  };
}

async function loadSet(device: Device, names: readonly string[]): Promise<TextureArrays> {
  const [color, normal, mask] = await Promise.all(
    (["color", "normal", "mask"] as const).map((kind) => Promise.all(names.map((n) => bitmap(`${BASE}${n}.${kind}.webp`)))),
  );
  const set = {
    color: textureArray(device, color, true, STAND_IN.color),
    normal: textureArray(device, normal, false, STAND_IN.normal),
    mask: textureArray(device, mask, false, STAND_IN.mask),
  };
  for (const list of [color, normal, mask]) for (const image of list) image.close();
  return set;
}

let state: { device: Device; textures: RealisticTextures } | null = null;
const listeners = new Set<() => void>();

/** Asked for when the textures have loaded (the map redraws). */
export function onTexturesReady(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/**
 * The textures for `device`: stand-ins at first (plain, flat), the real ones
 * once loaded. Loading starts the first time they are asked for.
 */
export function realisticTextures(device: Device): RealisticTextures {
  if (state?.device === device) return state.textures;
  const waterStandIn = device.createTexture({
    format: "rgba8unorm",
    width: 1,
    height: 1,
    sampler: SAMPLER,
    data: new Uint8Array(STAND_IN.normal),
  } as never);
  const textures: RealisticTextures = { buildings: standIns(device), ground: standIns(device), water: waterStandIn, ready: false };
  state = { device, textures };
  const mine = state;
  void (async () => {
    try {
      const [buildings, ground, water] = await Promise.all([
        loadSet(device, BUILDING_TEXTURES),
        loadSet(device, GROUND_TEXTURES),
        bitmap(`${BASE}water.normal.webp`),
      ]);
      if (state !== mine) return;
      const waterTexture = device.createTexture({
        format: "rgba8unorm",
        width: water.width,
        height: water.height,
        mipLevels: Math.floor(Math.log2(Math.max(water.width, water.height))) + 1,
        sampler: SAMPLER,
      } as never);
      waterTexture.copyExternalImage({ image: water, width: water.width, height: water.height });
      (waterTexture as unknown as { generateMipmapsWebGL(): void }).generateMipmapsWebGL();
      water.close();
      for (const old of [textures.buildings, textures.ground]) for (const t of Object.values(old)) t.destroy();
      textures.water.destroy();
      Object.assign(textures, { buildings, ground, water: waterTexture, ready: true });
      for (const listener of listeners) listener();
    } catch (error) {
      // the stand-ins stay: plain surfaces, lit as ever
      console.warn("realistic textures:", error);
    }
  })();
  return textures;
}
