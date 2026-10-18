// Streets GL's own vector tiles (tiles.streets.gl, its author's server; MIT
// code, © OpenStreetMap contributors data), which the realistic map reads
// close in: each building's height, colours and materials, each field, wood
// and tree, each road's lanes and surface. Only zoom 16 is served.
//
// Their schema, as Streets GL's tile processing reads it: layers `buildings`,
// `common` (gardens, grass, fields, pitches, car parks...), `natural` (woods,
// scrub, tree rows), `water` (water, waterways), `highways` (roads, paths,
// railways), `barriers` and `point` (single trees, benches, lamps), each
// feature's kind in `type`.

import { load } from "@loaders.gl/core";
import { MVTLoader } from "@loaders.gl/mvt";

/** The tiles; only `SGL_ZOOM` is served. */
export const SGL_TILES = "https://tiles.streets.gl/vector/{z}/{x}/{y}";
export const SGL_ZOOM = 16;
/** Their credit, with OpenStreetMap's. */
export const SGL_ATTRIBUTION = "Map data close in: Streets GL (tiles.streets.gl), © OpenStreetMap contributors";

type Properties = Record<string, unknown>;

/**
 * An MVTLayer's `fetch` for these tiles: only Streets GL's own zoom is asked
 * for (deck.gl wants coarser tiles towards a tilted view's horizon, which it
 * doesn't serve).
 */
export const sglFetch = (url: string, { loadOptions }: { loadOptions?: Parameters<typeof load>[2] }) =>
  url.includes(`/vector/${SGL_ZOOM}/`) ? load(url, MVTLoader, loadOptions) : Promise.resolve(null);

const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
// (a tile decoded to binary has a number the feature doesn't map as 0, so where 0 can't be meant it is none)
const some = (v: unknown): number | null => num(v) || null;

/** The CSS colours Streets GL turns OpenStreetMap's colour names into, back to their names (which the map tones down). */
const NAMES: Record<number, string> = {
  0xffffff: "white",
  0x808080: "grey",
  0xd3d3d3: "lightgrey",
  0xa9a9a9: "darkgrey",
  0xc0c0c0: "silver",
  0xff0000: "red",
  0x800000: "maroon",
  0xa52a2a: "brown",
  0xa0522d: "brown",
  0xd2b48c: "tan",
  0xf5f5dc: "beige",
  0xffff00: "yellow",
  0xffd700: "gold",
  0xffa500: "orange",
  0xffc0cb: "pink",
  0x008000: "green",
  0x0000ff: "blue",
};

/** A Streets GL colour (0xRRGGBB): a colour's name where it is a named one, else `#rrggbb` (black, 0, as none). */
export const hexColour = (v: unknown): string | undefined => {
  const n = some(v);
  return n === null ? undefined : (NAMES[n & 0xffffff] ?? `#${(n & 0xffffff).toString(16).padStart(6, "0")}`);
};

export type SglBuilding = {
  /** To the top of the roof, metres. */
  height: number;
  /** Where the walls start, metres up (a building over a passage, a roof on posts). */
  minHeight: number;
  /** The roof's own height, metres (0 when flat). */
  roofHeight: number;
  roofType: string;
  /** The walls' colour, `#rrggbb`, where mapped. */
  colour?: string;
  /** Mapped materials, as Streets GL names them (absent where not mapped). */
  facade?: "brick" | "cementBlock" | "wood" | "plaster" | "glass";
  roof?: "tiles" | "metal" | "concrete" | "eternit" | "thatch" | "grass" | "glass" | "tar";
  roofColour?: string;
};

const FACADES: Record<string, SglBuilding["facade"]> = {
  brick: "brick",
  cement_block: "cementBlock",
  block: "cementBlock",
  wood: "wood",
  plaster: "plaster",
  plastered: "plaster",
  concrete: "plaster",
  hard: "plaster",
  glass: "glass",
  mirror: "glass",
};
const ROOFS: Record<string, SglBuilding["roof"]> = {
  tile: "tiles",
  tiles: "tiles",
  roof_tiles: "tiles",
  slate: "tiles",
  metal: "metal",
  metal_sheet: "metal",
  "metal sheet": "metal",
  tin: "metal",
  copper: "metal",
  zinc: "metal",
  concrete: "concrete",
  asphalt: "concrete",
  eternit: "eternit",
  asbestos: "eternit",
  thatch: "thatch",
  grass: "grass",
  glass: "glass",
  tar_paper: "tar",
};
const ROOF_TYPES = new Set(["flat", "hipped", "gabled", "gambrel", "pyramidal", "onion", "dome", "round", "skillion", "mansard", "quadruple_saltbox", "saltbox"]);

/** A building's shape and looks from its tags, as Streets GL works them out (getBuildingParams): a level is 4 m, one when not mapped. */
export function sglBuilding(p: Properties): SglBuilding {
  const LEVEL = 4;
  const roofType = ROOF_TYPES.has(String(p.roofType)) ? String(p.roofType) : "flat";
  const levelsOfRoof = some(p.roofLevels) ?? (roofType === "flat" ? 0 : 1);
  let roofHeight = some(p.roofHeight) ?? levelsOfRoof * LEVEL;
  let height = some(p.height);
  let minHeight = num(p.minHeight);
  const levels = some(p.levels);
  const minLevel = num(p.minLevel);
  if (height !== null) roofHeight = Math.min(roofHeight, height - (minHeight ?? 0));
  if (height === null) height = (levels ?? minLevel ?? 1) * LEVEL + roofHeight;
  if (minHeight === null) minHeight = Math.min((minLevel ?? 0) * LEVEL, height);
  // a roof on posts: only its roof
  if (p.buildingType === "roof") minHeight = Math.max(minHeight, height - Math.max(roofHeight, 1));
  return {
    height,
    minHeight,
    roofHeight: Math.max(0, roofHeight),
    roofType,
    colour: hexColour(p.color),
    facade: FACADES[String(p.material)],
    roof: ROOFS[String(p.roofMaterial)],
    roofColour: hexColour(p.roofColor),
  };
}

/** How a Streets GL road is laid, as their tiles' `pathType` and tags give it: its kind (the realistic ground's), width and surface. */
export type SglPath = { kind: string; width: number | null; surface: "asphalt" | "dirt" | "cobblestone" | "concrete" | "sand" | "wood" };

const SURFACES: Record<string, SglPath["surface"]> = {
  asphalt: "asphalt",
  unpaved: "dirt",
  paved: "asphalt",
  ground: "dirt",
  concrete: "concrete",
  paving_stones: "cobblestone",
  gravel: "dirt",
  dirt: "dirt",
  grass: "dirt",
  compacted: "dirt",
  sand: "sand",
  sett: "cobblestone",
  fine_gravel: "dirt",
  wood: "wood",
  cobblestone: "cobblestone",
  earth: "dirt",
  pebblestone: "cobblestone",
};

/** Roads as the realistic ground has them (its `ROADS` and `PATHS`), and their lanes when not mapped. */
const KINDS: Record<string, { kind: string; lanes: number; width?: number }> = {
  motorway: { kind: "motorway", lanes: 2 },
  motorway_link: { kind: "motorway", lanes: 1 },
  trunk: { kind: "trunk", lanes: 2 },
  trunk_link: { kind: "trunk", lanes: 1 },
  primary: { kind: "primary", lanes: 2 },
  primary_link: { kind: "primary", lanes: 1 },
  secondary: { kind: "secondary", lanes: 2 },
  secondary_link: { kind: "secondary", lanes: 1 },
  tertiary: { kind: "tertiary", lanes: 2 },
  tertiary_link: { kind: "tertiary", lanes: 1 },
  residential: { kind: "minor", lanes: 2 },
  unclassified: { kind: "minor", lanes: 2 },
  living_street: { kind: "minor", lanes: 2 },
  service: { kind: "service", lanes: 2 },
  track: { kind: "track", lanes: 1 },
  busway: { kind: "busway", lanes: 1 },
  raceway: { kind: "raceway", lanes: 1, width: 6 },
  footway: { kind: "footway", lanes: 1 },
  path: { kind: "path", lanes: 1 },
  steps: { kind: "footway", lanes: 1 },
  platform: { kind: "pedestrian", lanes: 1 },
  pedestrian: { kind: "pedestrian", lanes: 1, width: 8 },
  cycleway: { kind: "cycleway", lanes: 1 },
  runway: { kind: "runway", lanes: 1, width: 45 },
  taxiway: { kind: "taxiway", lanes: 1, width: 20 },
};
const FOOT = new Set(["footway", "path", "pedestrian", "cycleway"]);

/** A road or path's kind, width (null: the kind's own) and surface; null for one Streets GL doesn't draw. */
export function sglPath(p: Properties): SglPath | null {
  const k = KINDS[String(p.pathType)];
  if (!k) return null;
  const width = some(p.width);
  const foot = FOOT.has(k.kind);
  const surface = SURFACES[String(p.surface)] ?? (foot || k.kind === "track" ? "dirt" : "asphalt");
  if (width !== null || k.width !== undefined || foot) return { kind: k.kind, width: width ?? k.width ?? null, surface };
  // a roadway as wide as its lanes, 3 m each (one alone is 4 m)
  const oneway = p.oneway === true;
  const lanes = some(p.lanes) ?? (oneway ? Math.max(1, Math.floor(k.lanes / 2)) : k.lanes);
  return { kind: k.kind, width: lanes === 1 ? 4 : lanes * 3, surface };
}

/** Streets GL's tree kinds from `leafType` and `genus`: a fir, a beech, linden or oak, or null (any broadleaf). */
export function sglTree(p: Properties): "fir" | "beech" | "linden" | "oak" | "broadleaf" {
  const genus = String(p.genus ?? "").toLowerCase();
  if (p.leafType === "needleleaved" || genus === "abies" || genus === "picea" || genus === "pinus") return "fir";
  if (genus === "fagus") return "beech";
  if (genus === "tilia") return "linden";
  if (genus === "quercus") return "oak";
  return "broadleaf";
}
