// The realistic map's textures: Streets GL's (github.com/StrandedKitty/streets-gl,
// MIT licence, © StrandedKitty; static/realistic/LICENSE-streets-gl.txt), built
// into static/realistic/ by scripts/realistic-textures.py. Each set is texture
// arrays, a layer per surface: its colour (sRGB) and its normal map; the
// buildings' also their mask (roughness, metalness, and where the building's
// own colour tints it) and where their windows light up at night.
//
// Every image is opaque, and decoded exactly as stored: a browser may
// premultiply an image's colour by its alpha as it decodes it (Safari does),
// which loses the colour wherever the alpha is 0. The images are uploaded the
// way they are stored, top row first; the shaders turn them the right way up.

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
  "pitch",
  "cycleway",
  "wood",
  "tram",
] as const;

export const textureLayer = {
  building: (name: (typeof BUILDING_TEXTURES)[number]) => BUILDING_TEXTURES.indexOf(name),
  ground: (name: (typeof GROUND_TEXTURES)[number]) => GROUND_TEXTURES.indexOf(name),
};

export type BuildingTextures = { color: Texture; normal: Texture; mask: Texture; glow: Texture };
export type GroundTextures = { color: Texture; normal: Texture };
/** `earth`: the whole Earth, cloud-free, for the land's colour zoomed out (its alpha is 0 until it loads: not used). */
export type RealisticTextures = { buildings: BuildingTextures; ground: GroundTextures; water: Texture; earth: Texture; ready: boolean };

const BASE = "/realistic/";
const SAMPLER = {
  addressModeU: "repeat",
  addressModeV: "repeat",
  magFilter: "linear",
  minFilter: "linear",
  mipmapFilter: "linear",
  maxAnisotropy: 8,
} as const;

type Kind = "color" | "normal" | "mask" | "glow";

/** A texture array of `images`, all one size, mipmapped; or until they load, a 1×1 stand-in for their kind. */
function textureArray(device: Device, images: ImageBitmap[] | null, kind: Kind): Texture {
  // the colour is sRGB, filtered in linear light; the rest are data
  const format = kind === "color" ? "rgba8unorm-srgb" : "rgba8unorm";
  if (!images) {
    return device.createTexture({
      dimension: "2d-array",
      format,
      width: 1,
      height: 1,
      depth: 1,
      sampler: SAMPLER,
      data: new Uint8Array(STAND_IN[kind]),
    } as never);
  }
  const size = images[0].width;
  const texture = device.createTexture({
    dimension: "2d-array",
    format,
    width: size,
    height: size,
    depth: images.length,
    mipLevels: Math.floor(Math.log2(size)) + 1,
    sampler: SAMPLER,
  } as never);
  images.forEach((image, z) => texture.copyExternalImage({ image, z, width: size, height: size, depth: 1 }));
  (texture as unknown as { generateMipmapsWebGL(): void }).generateMipmapsWebGL();
  return texture;
}

/** A plain 2D texture of `image`, mipmapped. */
function texture2d(device: Device, image: ImageBitmap, format: string, sampler: object = SAMPLER): Texture {
  const texture = device.createTexture({
    format,
    width: image.width,
    height: image.height,
    mipLevels: Math.floor(Math.log2(Math.max(image.width, image.height))) + 1,
    sampler,
  } as never);
  texture.copyExternalImage({ image, width: image.width, height: image.height });
  (texture as unknown as { generateMipmapsWebGL(): void }).generateMipmapsWebGL();
  image.close();
  return texture;
}

/** An image's pixels exactly as stored (no premultiplying, no colour conversion: normal maps are data). */
async function bitmap(url: string): Promise<ImageBitmap> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: ${response.status}`);
  return createImageBitmap(await response.blob(), { premultiplyAlpha: "none", colorSpaceConversion: "none" });
}

const STAND_IN: Record<Kind, [number, number, number, number]> = {
  color: [255, 255, 255, 255],
  normal: [128, 128, 255, 255],
  // rough, not metal, tinted all over
  mask: [235, 0, 255, 255],
  // no windows lit
  glow: [0, 0, 0, 255],
};

async function loadSet<K extends Kind>(device: Device, names: readonly string[], kinds: readonly K[]): Promise<Record<K, Texture>> {
  const images = await Promise.all(kinds.map((kind) => Promise.all(names.map((n) => bitmap(`${BASE}${n}.${kind}.webp`)))));
  const set = {} as Record<K, Texture>;
  kinds.forEach((kind, i) => {
    set[kind] = textureArray(device, images[i], kind);
    for (const image of images[i]) image.close();
  });
  return set;
}

const BUILDING_KINDS = ["color", "normal", "mask", "glow"] as const;
const GROUND_KINDS = ["color", "normal"] as const;
const standIns = <K extends Kind>(device: Device, kinds: readonly K[]) =>
  Object.fromEntries(kinds.map((kind) => [kind, textureArray(device, null, kind)])) as Record<K, Texture>;

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
  const textures: RealisticTextures = {
    buildings: standIns(device, BUILDING_KINDS),
    ground: standIns(device, GROUND_KINDS),
    water: waterStandIn,
    earth: device.createTexture({ format: "rgba8unorm", width: 1, height: 1, data: new Uint8Array(4) } as never),
    ready: false,
  };
  state = { device, textures };
  const mine = state;
  void (async () => {
    try {
      const [buildings, ground, water, earth] = await Promise.all([
        loadSet(device, BUILDING_TEXTURES, BUILDING_KINDS),
        loadSet(device, GROUND_TEXTURES, GROUND_KINDS),
        bitmap(`${BASE}water.normal.webp`),
        bitmap(`${BASE}earth.color.webp`),
      ]);
      if (state !== mine) return;
      const waterTexture = texture2d(device, water, "rgba8unorm");
      // round the world east to west; not over the poles
      const earthTexture = texture2d(device, earth, "rgba8unorm-srgb", { ...SAMPLER, addressModeV: "clamp-to-edge" });
      for (const old of [textures.buildings, textures.ground]) for (const t of Object.values(old)) t.destroy();
      textures.water.destroy();
      textures.earth.destroy();
      Object.assign(textures, { buildings, ground, water: waterTexture, earth: earthTexture, ready: true });
      for (const listener of listeners) listener();
    } catch (error) {
      // the stand-ins stay: plain surfaces, lit as ever
      console.warn("realistic textures:", error);
    }
  })();
  return textures;
}
