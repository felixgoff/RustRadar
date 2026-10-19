// The realistic map's textures: Streets GL's (github.com/StrandedKitty/streets-gl,
// MIT licence, © StrandedKitty; static/realistic/LICENSE-streets-gl.txt), built
// by scripts/realistic-textures.py and compress-textures.mjs into
// static/realistic/, as GPU-compressed KTX2 files (ktx2.ts). Each set is texture
// arrays, a layer per surface: its colour (sRGB) and its normal map; the
// buildings' also their mask (roughness, metalness, and where the building's
// own colour tints it) and where their windows light up at night; the trees'
// (a picture of each whole tree) also their shape, as a mask, and the one
// normal map of a crown's volume they share.
//
// Every image is opaque. The images are uploaded the way they are stored, top
// row first; the shaders turn them the right way up.

import type { Device, Texture } from "@luma.gl/core";
import { loadKtx2 } from "./ktx2";

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
  "terrain",
] as const;

/** The trees' layers, in the order the shaders index them: Streets GL's. */
export const TREE_TEXTURES = ["tree-beech", "tree-fir", "tree-linden0", "tree-linden1", "tree-oak"] as const;

export const textureLayer = {
  building: (name: (typeof BUILDING_TEXTURES)[number]) => BUILDING_TEXTURES.indexOf(name),
  ground: (name: (typeof GROUND_TEXTURES)[number]) => GROUND_TEXTURES.indexOf(name),
  tree: (name: (typeof TREE_TEXTURES)[number]) => TREE_TEXTURES.indexOf(name),
};

export type BuildingTextures = { color: Texture; normal: Texture; mask: Texture; glow: Texture };
export type GroundTextures = { color: Texture; normal: Texture };
export type TreeTextures = { color: Texture; normal: Texture; mask: Texture; volume: Texture };
/**
 * `earth`: the whole Earth, cloud-free, for the land's colour zoomed out (its alpha is 0 until it loads: not used);
 * `night`: its lights at night, for the globe's night side (black until it loads).
 */
export type RealisticTextures = {
  buildings: BuildingTextures;
  ground: GroundTextures;
  trees: TreeTextures;
  water: Texture;
  earth: Texture;
  night: Texture;
  ready: boolean;
};

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

/** Until the textures load, a 1×1 stand-in for a kind. */
function standIn(device: Device, kind: Kind): Texture {
  // the colour is sRGB, filtered in linear light; the rest are data
  const format = kind === "color" ? "rgba8unorm-srgb" : "rgba8unorm";
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

/** A 1×1 array of nothing: no tree's shape. */
const blank = (device: Device) =>
  device.createTexture({ dimension: "2d-array", format: "rgba8unorm", width: 1, height: 1, depth: 1, data: new Uint8Array(4) } as never);
/** A 1×1 flat normal map. */
const flatNormal = (device: Device) =>
  device.createTexture({ format: "rgba8unorm", width: 1, height: 1, sampler: SAMPLER, data: new Uint8Array(STAND_IN.normal) } as never);

const STAND_IN: Record<Kind, [number, number, number, number]> = {
  color: [255, 255, 255, 255],
  normal: [128, 128, 255, 255],
  // rough, not metal, tinted all over
  mask: [235, 0, 255, 255],
  // no windows lit
  glow: [0, 0, 0, 255],
};

/** A set's textures: an array of a kind's layers each, from static/realistic/<set>.<kind>.ktx2. */
async function loadSet<K extends Kind>(device: Device, set: string, kinds: readonly K[]): Promise<Record<K, Texture>> {
  const textures = await Promise.all(kinds.map((kind) => loadKtx2(device, `${BASE}${set}.${kind}.ktx2`, { srgb: kind === "color", array: true, sampler: SAMPLER })));
  return Object.fromEntries(kinds.map((kind, i) => [kind, textures[i]])) as Record<K, Texture>;
}

const BUILDING_KINDS = ["color", "normal", "mask", "glow"] as const;
const GROUND_KINDS = ["color", "normal"] as const;
const TREE_KINDS = ["color", "normal", "mask"] as const;
const standIns = <K extends Kind>(device: Device, kinds: readonly K[]) =>
  Object.fromEntries(kinds.map((kind) => [kind, standIn(device, kind)])) as Record<K, Texture>;

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
    // (no tree shows until they load: a mask of none)
    trees: { ...standIns(device, ["color", "normal"] as const), mask: blank(device), volume: flatNormal(device) },
    water: waterStandIn,
    earth: device.createTexture({ format: "rgba8unorm", width: 1, height: 1, data: new Uint8Array(4) } as never),
    night: device.createTexture({ format: "rgba8unorm", width: 1, height: 1, data: new Uint8Array([0, 0, 0, 255]) } as never),
    ready: false,
  };
  state = { device, textures };
  const mine = state;
  void (async () => {
    try {
      // round the world east to west; not over the poles
      const globe = { ...SAMPLER, addressModeV: "clamp-to-edge" };
      const plain = (name: string, srgb: boolean, sampler: object = SAMPLER) => loadKtx2(device, `${BASE}${name}.ktx2`, { srgb, array: false, sampler });
      const [buildings, ground, trees, waterTexture, earthTexture, volumeTexture, nightTexture] = await Promise.all([
        loadSet(device, "building", BUILDING_KINDS),
        loadSet(device, "ground", GROUND_KINDS),
        loadSet(device, "tree", TREE_KINDS),
        plain("water.normal", false),
        plain("earth.color", true, globe),
        plain("tree-volume.normal", false, { ...SAMPLER, addressModeU: "clamp-to-edge", addressModeV: "clamp-to-edge" }),
        plain("earth.night", true, globe),
      ]);
      if (state !== mine) {
        for (const set of [buildings, ground, trees]) for (const t of Object.values(set)) t.destroy();
        for (const t of [waterTexture, earthTexture, volumeTexture, nightTexture]) t.destroy();
        return;
      }
      for (const old of [textures.buildings, textures.ground, textures.trees]) for (const t of Object.values(old)) t.destroy();
      textures.water.destroy();
      textures.earth.destroy();
      textures.night.destroy();
      Object.assign(textures, { buildings, ground, trees: { ...trees, volume: volumeTexture }, water: waterTexture, earth: earthTexture, night: nightTexture, ready: true });
      for (const listener of listeners) listener();
    } catch (error) {
      // the stand-ins stay: plain surfaces, lit as ever
      console.warn("realistic textures:", error);
    }
  })();
  return textures;
}
