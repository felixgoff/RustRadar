// Trees for the realistic map: 3D trees wherever OpenFreeMap's vector tiles
// have woods, parks, gardens, cemeteries, orchards, scrub or the gardens of
// residential streets, kept off the roads, railways, buildings and water of
// the same tile. Conifers and broadleaves by latitude, each one of Streets
// GL's trees (fir; beech, linden or oak), coloured by the season (autumn's
// yellows and reds, bare in winter).
//
// Each tile is planted once: its polygons and lines are rasterised into two
// masks (what grows where, and where nothing may), then each kind of cover's
// own jittered grid of candidate trees is tested against them. The grid is
// laid in the world, not the tile, so trees don't line up along tile edges,
// and every tree's place, size and colour come from a hash of its grid cell:
// the same trees every time.

import type { Layer } from "@deck.gl/core";
import { SimpleMeshLayer } from "@deck.gl/mesh-layers";
import { load } from "@loaders.gl/core";
import { MVTLoader } from "@loaders.gl/mvt";
import { shadowStep } from "./building-shadow";
import { canopy, type CanopyGrid, tallestNear } from "./canopy";
import { classAt, landCover, type LandGrid, LC, wooded } from "./landcover";
import { SGL_ZOOM, sglPath, sglTree } from "./streets-gl";
import { REGISTER_KINDS, type RegisteredTree, streetTrees } from "./street-trees";
import { treeCard } from "./tree-models";

/** Tiles are planted from this zoom's data, OpenFreeMap's deepest. */
export const TREE_TILE_ZOOM = 14;
/** Masks, cells across a tile (about 2.7 m each at 56°N). */
const GRID = 512;
/** At most this many trees a tile; denser tiles are thinned evenly. */
const MAX_TREES = 24_000;
const EARTH_CIRCUMFERENCE = 40_075_016.686;
const DEG = Math.PI / 180;

/** How a kind of land is planted: trees `spacing` metres apart, each there by `chance`, `height` metres tall. */
type Cover = {
  spacing: number;
  chance: number;
  /** Share of conifers; null: by latitude, as in a wood. */
  conifers: number | null;
  height: [number, number];
  /** Crown width against the mesh's own, by tree. */
  width: [number, number];
};

// the codes in the cover mask, in drawing order: later ones win where they overlap
const COVERS: Cover[] = [
  // 0: nothing grows
  { spacing: 1, chance: 0, conifers: 0, height: [0, 0], width: [1, 1] },
  // 1: residential streets' gardens
  { spacing: 15, chance: 0.2, conifers: 0.25, height: [6, 13], width: [0.9, 1.25] },
  // 2: school, hospital and university grounds
  { spacing: 16, chance: 0.16, conifers: 0.2, height: [7, 14], width: [0.9, 1.3] },
  // 3: allotments
  { spacing: 10, chance: 0.18, conifers: 0.05, height: [3, 6], width: [1, 1.3] },
  // 4: gardens
  { spacing: 12, chance: 0.32, conifers: 0.2, height: [5, 11], width: [0.9, 1.3] },
  // 5: parks, recreation grounds, village greens
  { spacing: 15, chance: 0.42, conifers: 0.18, height: [9, 18], width: [0.95, 1.35] },
  // 6: cemeteries
  { spacing: 11, chance: 0.55, conifers: 0.3, height: [8, 16], width: [0.85, 1.2] },
  // 7: orchards: low trees in rows
  { spacing: 5, chance: 0.95, conifers: 0, height: [3.2, 4.8], width: [1.1, 1.3] },
  // 8: wetland: scattered birches and willows
  { spacing: 10, chance: 0.22, conifers: 0.1, height: [4, 10], width: [0.9, 1.2] },
  // 9: scrub, heath and shrubberies: bushes
  { spacing: 5.5, chance: 0.42, conifers: 0.2, height: [2.2, 4.5], width: [1.1, 1.5] },
  // 10: woods and forests
  { spacing: 6.2, chance: 0.92, conifers: null, height: [13, 24], width: [0.85, 1.15] },
  // 11, 12: woods mapped broadleaved, or needleleaved (Streets GL's tiles say)
  { spacing: 6.2, chance: 0.92, conifers: 0.05, height: [13, 24], width: [0.85, 1.15] },
  { spacing: 6.2, chance: 0.92, conifers: 0.97, height: [13, 24], width: [0.85, 1.15] },
];
const ORCHARD = 7;

type Properties = Record<string, unknown>;
type Feature = { geometry: { type: string; coordinates: unknown }; properties: Properties };

/** The cover a feature is, if trees grow in it (0 if not). */
export function coverOf(p: Properties): number {
  const [layer, cls, sub] = [p.layerName, p.class, p.subclass];
  // Streets GL's tiles (streets-gl.ts): woods by their leaves, scrub, gardens
  if (layer === "natural") {
    if (p.type === "forest") return p.leafType === "broadleaved" ? 11 : p.leafType === "needleleaved" ? 12 : 10;
    return p.type === "scrub" ? 9 : 0;
  }
  if (layer === "common") return p.type === "garden" ? 4 : 0;
  if (layer === "landcover") {
    if (cls === "wood") return 10;
    if (cls === "wetland") return 8;
    if (cls === "farmland") return sub === "orchard" ? ORCHARD : 0;
    if (cls === "grass") {
      if (sub === "scrub" || sub === "shrubbery" || sub === "heath") return 9;
      if (sub === "park" || sub === "recreation_ground" || sub === "village_green") return 5;
      if (sub === "garden") return 4;
      if (sub === "allotments") return 3;
    }
    return 0;
  }
  if (layer === "landuse") {
    if (cls === "cemetery") return 6;
    if (cls === "residential") return 1;
    if (cls === "school" || cls === "university" || cls === "college" || cls === "hospital") return 2;
  }
  return 0;
}

/** Metres to keep clear either side of a road, path or railway line. */
const CLEARANCE: Record<string, number> = {
  motorway: 12,
  trunk: 9,
  primary: 8,
  secondary: 7,
  tertiary: 6,
  minor: 5,
  service: 3.5,
  busway: 5,
  raceway: 6,
  track: 2.5,
  path: 1.5,
  rail: 3.5,
  transit: 3,
  pier: 2,
  bridge: 5,
};

/** Paved or kept clear, in Streets GL's tiles. */
const SGL_BARE = new Set(["parking", "pitch", "playground", "dogPark", "bridge", "construction", "brownfield", "apron", "helipad", "pier", "sand", "rock"]);

/** Where nothing grows: a polygon to fill (true), a line's clearance in metres, or nothing (0). */
export function blockOf(p: Properties, geometryType: string): number | true {
  const [layer, cls] = [p.layerName, String(p.class ?? "")];
  const polygon = geometryType.endsWith("Polygon");
  if (layer === "building" || layer === "buildings" || layer === "water") return polygon ? true : 0;
  // Streets GL's tiles: car parks, pitches and the like; roads and railways their own width and a little
  if (layer === "common") return polygon && SGL_BARE.has(String(p.type)) ? true : 0;
  if (layer === "highways") {
    if (polygon) return true;
    if (p.type === "railway") return CLEARANCE.rail;
    const path = sglPath(p);
    return path ? (path.width ?? 2) / 2 + 1.5 : 0;
  }
  if (layer === "landuse") return polygon && (cls === "railway" || cls === "pitch" || cls === "playground") ? true : 0;
  if (layer === "transportation") {
    if (cls.endsWith("_construction") || cls === "ferry") return 0;
    if (polygon) return true;
    return CLEARANCE[cls] ?? 3;
  }
  return 0;
}

// --------------------------------------------------------------- rasterising

type Ring = number[][];

/** Every cell of `mask` whose centre is inside the polygon (even-odd over all its rings), set to `value`. */
function fillPolygon(mask: Uint8Array, rings: Ring[], value: number) {
  type Edge = [number, number, number, number];
  const edges: Edge[] = [];
  let top = Infinity;
  let bottom = -Infinity;
  for (const ring of rings) {
    for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
      const [x0, y0] = [ring[j][0] * GRID, ring[j][1] * GRID];
      const [x1, y1] = [ring[i][0] * GRID, ring[i][1] * GRID];
      if (y0 === y1) continue;
      edges.push(y0 < y1 ? [x0, y0, x1, y1] : [x1, y1, x0, y0]);
      top = Math.min(top, y0, y1);
      bottom = Math.max(bottom, y0, y1);
    }
  }
  const from = Math.max(0, Math.floor(top - 0.5));
  const to = Math.min(GRID - 1, Math.ceil(bottom - 0.5));
  const crossings: number[] = [];
  for (let row = from; row <= to; row++) {
    const y = row + 0.5;
    crossings.length = 0;
    for (const [x0, y0, x1, y1] of edges) {
      if (y >= y0 && y < y1) crossings.push(x0 + ((y - y0) / (y1 - y0)) * (x1 - x0));
    }
    crossings.sort((a, b) => a - b);
    const line = row * GRID;
    for (let k = 0; k + 1 < crossings.length; k += 2) {
      const a = Math.max(0, Math.ceil(crossings[k] - 0.5));
      const b = Math.min(GRID - 1, Math.floor(crossings[k + 1] - 0.5));
      for (let col = a; col <= b; col++) mask[line + col] = value;
    }
  }
}

/** Every cell within `radius` cells of the line, set to `value`. */
function strokeLine(mask: Uint8Array, line: Ring, radius: number, value: number) {
  const r2 = radius * radius;
  for (let i = 1; i < line.length; i++) {
    const [ax, ay] = [line[i - 1][0] * GRID, line[i - 1][1] * GRID];
    const [bx, by] = [line[i][0] * GRID, line[i][1] * GRID];
    const [dx, dy] = [bx - ax, by - ay];
    const len2 = dx * dx + dy * dy || 1;
    const x0 = Math.max(0, Math.floor(Math.min(ax, bx) - radius));
    const x1 = Math.min(GRID - 1, Math.ceil(Math.max(ax, bx) + radius));
    const y0 = Math.max(0, Math.floor(Math.min(ay, by) - radius));
    const y1 = Math.min(GRID - 1, Math.ceil(Math.max(ay, by) + radius));
    for (let row = y0; row <= y1; row++) {
      const py = row + 0.5;
      for (let col = x0; col <= x1; col++) {
        const px = col + 0.5;
        const t = Math.min(1, Math.max(0, ((px - ax) * dx + (py - ay) * dy) / len2));
        const [ex, ey] = [ax + t * dx - px, ay + t * dy - py];
        if (ex * ex + ey * ey <= r2) mask[row * GRID + col] = value;
      }
    }
  }
}

/** A feature's polygons, each a list of rings. */
function polygonsOf(g: Feature["geometry"]): Ring[][] {
  if (g.type === "Polygon") return [g.coordinates as Ring[]];
  if (g.type === "MultiPolygon") return g.coordinates as Ring[][];
  return [];
}
function linesOf(g: Feature["geometry"]): Ring[] {
  if (g.type === "LineString") return [g.coordinates as Ring];
  if (g.type === "MultiLineString") return g.coordinates as Ring[];
  return [];
}

// --------------------------------------------------------------- planting

/** A hash of two integers (and a salt), 0 to 1. */
export function hash2(x: number, y: number, salt: number): number {
  let h = Math.imul(x | 0, 0x27d4eb2d) ^ Math.imul(y | 0, 0x165667b1) ^ Math.imul(salt | 0, 0x9e3779b1);
  h = Math.imul(h ^ (h >>> 15), 0x85ebca6b);
  h = Math.imul(h ^ (h >>> 13), 0xc2b2ae35);
  return ((h ^ (h >>> 16)) >>> 0) / 4294967296;
}

/** How far through the year the leaves are, for broadleaves at `lat` on `date`: shares of each look. */
export function leafPhase(date: Date, lat: number): { spring: number; summer: number; autumn: number; bare: number } {
  // the tropics are green all year
  if (Math.abs(lat) < 23) return { spring: 0, summer: 1, autumn: 0, bare: 0 };
  const start = Date.UTC(date.getUTCFullYear(), 0, 1);
  let day = (date.getTime() - start) / 86_400_000;
  // the southern hemisphere's seasons are half a year on
  if (lat < 0) day = (day + 182.5) % 365;
  // spring later and autumn earlier further from the equator: about four days a degree from 56°
  const shift = (Math.abs(lat) - 56) * 4;
  const leafOut = 118 + shift;
  const turn = 262 - shift;
  const fall = 300 - shift * 0.8;
  const ramp = (x: number, a: number, b: number) => Math.min(1, Math.max(0, (x - a) / (b - a)));
  const leaves = ramp(day, leafOut - 8, leafOut + 8) * (1 - ramp(day, fall, fall + 18));
  const autumn = ramp(day, turn - 10, turn + 14) * leaves;
  const spring = (1 - ramp(day, leafOut + 10, leafOut + 34)) * leaves * (1 - autumn);
  const summer = Math.max(0, leaves - autumn - spring);
  return { spring, summer, autumn, bare: 1 - leaves };
}

type RGB = [number, number, number];
/** Streets GL's trees, by their layer in the textures (realistic-textures.ts): fir, and the broadleaves. */
const FIR = 1;
const BROADLEAVES = [0, 2, 3, 4];
/** How much larger than the tree its card is, by layer (Streets GL's, for each picture's margin). */
const CARD_SCALE = [1.35, 1.06, 1.19, 1.02, 1.43];
/** A tree shown as its picture is: grey, which the brightness varies (220 is as it is). */
const SUMMER_AS_DRAWN: RGB = [220, 220, 220];
const SPRING: RGB[] = [
  [104, 146, 62],
  [118, 158, 70],
  [96, 136, 58],
];
const AUTUMN: RGB[] = [
  [196, 150, 46],
  [204, 118, 40],
  [170, 72, 38],
  [150, 138, 52],
  [104, 108, 46],
  [214, 168, 60],
];
const BARE: RGB[] = [
  [112, 98, 86],
  [98, 88, 80],
  [122, 108, 94],
];

/** One kind of tree's instances in a tile, shuffled so any first N of them are an even thinning. */
export type Instances = {
  length: number;
  /** `[lon, lat, 0]` each. */
  positions: Float64Array;
  /** The mesh's 3×3 transform and translation, 12 floats each (SimpleMeshLayer's own layout). */
  matrices: Float32Array;
  colors: Uint8Array;
};
export type TileTrees = { conifers: Instances; broadleaves: Instances };

type Tile = { x: number; y: number; z: number };

/**
 * What else is known of a tile's ground: its land cover (landcover.ts) and
 * how tall its trees really are (canopy.ts), each to the tile's own corners.
 */
export type Surroundings = { land?: LandGrid | null; canopy?: CanopyGrid | null; registered?: RegisteredTree[] };

/** Covers a canopy's heights place trees in, where they really are: residential streets to cemeteries, and woods. */
const PLACED_BY_CANOPY = new Set([1, 2, 3, 4, 5, 6, 10, 11, 12]);
/** Where the canopy is, the trees are sought this far apart, metres (closer than any cover plants them). */
const CANOPY_STEP_M = 7;
/** A tree is taken to stand where the canopy is this tall, metres (a wood's are left out lower: clearings). */
const CANOPY_TREE_M = 3;
const CANOPY_WOOD_M = 2;
/** Unmapped land with this tall a canopy grows trees, metres. */
const CANOPY_UNMAPPED_M = 4;
/** No other tree within this many metres of a town's registered one (it is that tree); a mapped one, this many. */
const REGISTERED_CLEAR_M = 4.5;
const REGISTERED_MAPPED_M = 3;
/** The salts of the trees the canopy or the land cover alone place, on unmapped land. */
const UNMAPPED_CANOPY = 20;
const UNMAPPED_WOOD = 21;

/** A tile's trees from its decoded features (MVTLoader, `coordinates: "local"`: 0 to 1 across, y down). */
export function plantTile(features: Feature[], tile: Tile, date: Date, around: Surroundings = {}): TileTrees {
  const cover = new Uint8Array(GRID * GRID);
  const blocked = new Uint8Array(GRID * GRID);
  const n = 2 ** tile.z;
  const latAt = (y: number) => Math.atan(Math.sinh(Math.PI * (1 - (2 * (tile.y + y)) / n))) / DEG;
  const midLat = latAt(0.5);
  const tileMetres = (EARTH_CIRCUMFERENCE * Math.cos(midLat * DEG)) / n;
  const cellMetres = tileMetres / GRID;

  // what grows where: lower codes first, so woods win over the parks and streets they lie in
  const covered = features
    .map((f) => ({ f, code: coverOf(f.properties) }))
    .filter((c) => c.code > 0)
    .sort((a, b) => a.code - b.code);
  const present = new Set<number>();
  for (const { f, code } of covered) {
    for (const polygon of polygonsOf(f.geometry)) fillPolygon(cover, polygon, code);
    present.add(code);
  }
  // mapped trees, one by one, and in rows (Streets GL's tiles)
  const mapped = features.filter((f) => (f.properties.layerName === "point" && f.properties.type === "tree") || f.properties.type === "treeRow");
  const { land, canopy: heights, registered = [] } = around;
  if (!present.size && !mapped.length && !heights && !land && !registered.length) return { conifers: empty(), broadleaves: empty() };
  // where nothing may
  for (const f of features) {
    const block = blockOf(f.properties, f.geometry.type);
    if (block === true) for (const polygon of polygonsOf(f.geometry)) fillPolygon(blocked, polygon, 1);
    else if (block > 0) for (const line of linesOf(f.geometry)) strokeLine(blocked, line, block / cellMetres, 1);
  }

  // conifers: a third in the south of Sweden, three quarters in the north; the same pattern elsewhere by latitude
  const woodConifers = Math.min(0.8, Math.max(0.15, 0.35 + (Math.abs(midLat) - 56) * 0.12));
  /** A wood's share of conifers at `x, y`: as its land cover says, else `fallback`. */
  const conifersAt = (x: number, y: number, fallback: number) => {
    const c = land ? classAt(land, x, y) : LC.NONE;
    return c === LC.CONIFERS ? 0.95 : c === LC.MIXED ? 0.5 : c === LC.BROADLEAVES ? 0.05 : fallback;
  };
  // the canopy's pixels, and how many of them make two metres (a crown's middle may miss the point)
  const near = heights ? Math.max(0, Math.round(2 / (tileMetres / heights.size))) : 0;
  const canopyAt = (x: number, y: number) => (heights ? tallestNear(heights, x, y, near) : 0);
  /** A tree's height from the canopy where it is (a little under it, as the canopy is its tallest), else its cover's. */
  const tallAt = (x: number, y: number, h: number, fallback: [number, number]): [number, number] => {
    const top = canopyAt(x, y);
    if (top < CANOPY_TREE_M) return fallback;
    const t = Math.min(45, top * (0.86 + 0.14 * h));
    return [t, t];
  };
  const leaves = leafPhase(date, midLat);
  type Tree = {
    x: number;
    y: number;
    rank: number;
    conifer: boolean;
    height: number;
    width: number;
    yaw: number;
    colour: RGB;
    species: number;
    /** Its colour is the season's in place of the picture's own (spring, autumn, bare). */
    retint: boolean;
  };
  const trees: Tree[] = [];
  /** A tree at `x, y` (the tile's 0 to 1) of these heights and widths, its own choices from `h`; a conifer, and which, if not `h`'s. */
  const grow = (x: number, y: number, h: (salt: number) => number, conifers: number, height: [number, number], wide: [number, number], rank: number, kind?: number) => {
    const conifer = kind === undefined ? h(3) < conifers : kind === FIR;
    let colour: RGB = SUMMER_AS_DRAWN;
    let width = wide[0] + (wide[1] - wide[0]) * h(5);
    const species = kind ?? (conifer ? FIR : BROADLEAVES[Math.floor(h(11) * BROADLEAVES.length)]);
    let retint = false;
    if (!conifer) {
      // each tree turns in its own time: pick its look by the season's shares
      const r = h(7);
      const pick = (list: RGB[]) => list[Math.floor(h(6) * list.length)];
      // (in summer, the picture's own green)
      retint = r < leaves.bare + leaves.autumn + leaves.spring;
      if (r < leaves.bare) {
        colour = pick(BARE);
        width *= 0.82;
      } else if (r < leaves.bare + leaves.autumn) colour = pick(AUTUMN);
      else if (retint) colour = pick(SPRING);
    }
    // a little variety in brightness, tree to tree
    const v = 0.88 + 0.24 * h(8);
    colour = colour.map((ch) => Math.min(255, Math.round(ch * v))) as RGB;
    const tall = height[0] + (height[1] - height[0]) * h(4);
    trees.push({ x, y, rank, conifer, height: tall, width, yaw: h(10) * 2 * Math.PI, colour, species, retint });
  };
  // the towns' registered trees, each where it stands, of its kind, its crown as wide as recorded
  // (kept in cells about 5 m across, to find them by)
  const taken = new Map<number, [number, number][]>();
  const cells = Math.max(1, Math.round(tileMetres / REGISTERED_CLEAR_M));
  const cellOf = (x: number, y: number) => Math.floor(y * cells) * cells + Math.floor(x * cells);
  for (const [k, t] of registered.entries()) {
    if (t.x < 0 || t.x >= 1 || t.y < 0 || t.y >= 1) continue;
    const h = (salt: number) => hash2(Math.round((tile.x + t.x) * 65536), Math.round((tile.y + t.y) * 65536), 1301 + salt);
    const which = [undefined, FIR, 0, h(12) < 0.5 ? 2 : 3, 4][t.kind] ?? undefined;
    // as tall as the canopy has it there, else as its crown suggests; as wide as its crown (the picture's crown about three quarters of it)
    const fromCrown = Math.min(28, Math.max(3, t.crown * 1.5 + 1));
    const tall = tallAt(t.x, t.y, h(4), t.crown > 0 ? [fromCrown, fromCrown] : [7, 14]);
    const across = Math.min(1.2, Math.max(0.75, t.crown / (((tall[0] + tall[1]) / 2) * 0.75)));
    const wide: [number, number] = t.crown > 0 ? [across, across] : [0.9, 1.2];
    grow(t.x, t.y, h, t.kind === REGISTER_KINDS.CONIFER ? 1 : 0, tall, wide, -1 - k * 1e-9, which);
    const cell = cellOf(t.x, t.y);
    const list = taken.get(cell);
    if (list) list.push([t.x, t.y]);
    else taken.set(cell, [[t.x, t.y]]);
  }
  /** Whether a registered tree stands within `m` metres. */
  const nearRegistered = (x: number, y: number, m: number) => {
    if (!taken.size) return false;
    const r = m / tileMetres;
    const [cx, cy] = [Math.floor(x * cells), Math.floor(y * cells)];
    for (let j = Math.max(0, cy - 1); j <= Math.min(cells - 1, cy + 1); j++) {
      for (let i = Math.max(0, cx - 1); i <= Math.min(cells - 1, cx + 1); i++) {
        for (const [tx, ty] of taken.get(j * cells + i) ?? []) if (Math.hypot(tx - x, ty - y) < r) return true;
      }
    }
    return false;
  };
  for (const code of present) {
    const c = COVERS[code];
    // where the canopy says, the trees where they really stand: sought closer together, each where it is tall enough
    const placed = heights !== undefined && heights !== null && PLACED_BY_CANOPY.has(code);
    const wood = code >= 10;
    const chance = placed && !wood ? 1 : c.chance;
    const step = (placed && !wood ? CANOPY_STEP_M : c.spacing) / tileMetres;
    // this cover's grid, in the world's tile units at this zoom: cells whose middle falls in the tile
    const [i0, i1] = [Math.floor(tile.x / step), Math.ceil((tile.x + 1) / step)];
    const [j0, j1] = [Math.floor(tile.y / step), Math.ceil((tile.y + 1) / step)];
    const jitter = code === ORCHARD ? 0.08 : 0.7;
    for (let j = j0; j < j1; j++) {
      for (let i = i0; i < i1; i++) {
        const h = (salt: number) => hash2(i, j, code * 16 + salt);
        if (h(0) >= chance) continue;
        const x = (i + 0.5 + jitter * (h(1) - 0.5)) * step - tile.x;
        const y = (j + 0.5 + jitter * (h(2) - 0.5)) * step - tile.y;
        if (x < 0 || x >= 1 || y < 0 || y >= 1) continue;
        const cell = Math.floor(y * GRID) * GRID + Math.floor(x * GRID);
        if (cover[cell] !== code || blocked[cell]) continue;
        if (placed && canopyAt(x, y) < (wood ? CANOPY_WOOD_M : CANOPY_TREE_M)) continue;
        if (nearRegistered(x, y, REGISTERED_CLEAR_M)) continue;
        const conifers = c.conifers ?? conifersAt(x, y, woodConifers);
        grow(x, y, h, conifers, placed ? tallAt(x, y, h(4), c.height) : c.height, c.width, h(9));
      }
    }
  }
  // unmapped land: trees where the canopy has them (else where the land cover has woods), of the land's kind
  if (heights || land) {
    const byCanopy = Boolean(heights);
    const salt = byCanopy ? UNMAPPED_CANOPY : UNMAPPED_WOOD;
    const wood = COVERS[10];
    const step = (byCanopy ? CANOPY_STEP_M : wood.spacing) / tileMetres;
    const [i0, i1] = [Math.floor(tile.x / step), Math.ceil((tile.x + 1) / step)];
    const [j0, j1] = [Math.floor(tile.y / step), Math.ceil((tile.y + 1) / step)];
    for (let j = j0; j < j1; j++) {
      for (let i = i0; i < i1; i++) {
        const h = (s: number) => hash2(i, j, salt * 16 + s);
        if (!byCanopy && h(0) >= wood.chance) continue;
        const x = (i + 0.5 + 0.7 * (h(1) - 0.5)) * step - tile.x;
        const y = (j + 0.5 + 0.7 * (h(2) - 0.5)) * step - tile.y;
        if (x < 0 || x >= 1 || y < 0 || y >= 1) continue;
        const cell = Math.floor(y * GRID) * GRID + Math.floor(x * GRID);
        if (cover[cell] || blocked[cell]) continue;
        const kind = land ? classAt(land, x, y) : LC.NONE;
        if (kind === LC.WATER || kind === LC.ARABLE) continue;
        if (nearRegistered(x, y, REGISTERED_CLEAR_M)) continue;
        if (byCanopy) {
          if (canopyAt(x, y) < CANOPY_UNMAPPED_M) continue;
          // in a wood as the land cover says; else a garden's or a street's tree
          const forest = wooded(kind);
          grow(x, y, h, conifersAt(x, y, forest ? woodConifers : 0.2), tallAt(x, y, h(4), wood.height), forest ? wood.width : [0.9, 1.25], h(9));
        } else if (wooded(kind)) grow(x, y, h, conifersAt(x, y, woodConifers), wood.height, wood.width, h(9));
      }
    }
  }
  // each mapped tree where it is, of its kind (any broadleaf where it doesn't say); a row's every 10 m or so
  const ROW_M = 10;
  for (const f of mapped) {
    const p = f.properties;
    const sort = sglTree(p);
    const mappedTall = Number(p.height) > 0 ? Number(p.height) : 0;
    const at = (x: number, y: number, k: number) => {
      if (x < 0 || x >= 1 || y < 0 || y >= 1) return;
      if (nearRegistered(x, y, REGISTERED_MAPPED_M)) return;
      const [gx, gy] = [Math.round((tile.x + x) * 65536), Math.round((tile.y + y) * 65536)];
      const h = (salt: number) => hash2(gx, gy, 977 + k * 31 + salt);
      // (a linden either of Streets GL's two; a broadleaf not said, any of them)
      const which = { fir: FIR, beech: 0, linden: h(12) < 0.5 ? 2 : 3, oak: 4, broadleaf: undefined }[sort];
      // as tall as mapped, else as the canopy has it there
      grow(x, y, h, 0, mappedTall ? [mappedTall, mappedTall] : tallAt(x, y, h(4), [7, 14]), [0.9, 1.2], -1, which);
    };
    if (f.geometry.type === "Point") {
      const [x, y] = f.geometry.coordinates as number[];
      at(x, y, 0);
    } else {
      for (const line of linesOf(f.geometry)) {
        let carry = 0;
        for (let i = 1; i < line.length; i++) {
          const [[x0, y0], [x1, y1]] = [line[i - 1], line[i]];
          const metres = Math.hypot(x1 - x0, y1 - y0) * tileMetres;
          for (let d = carry; d < metres; d += ROW_M) at(x0 + ((x1 - x0) * d) / metres, y0 + ((y1 - y0) * d) / metres, i);
          carry = (carry - metres) % ROW_M;
          if (carry < 0) carry += ROW_M;
        }
      }
    }
  }
  // shuffled (by a hash, so the same every time) and thinned to the most a tile may have
  trees.sort((a, b) => a.rank - b.rank);
  if (trees.length > MAX_TREES) trees.length = MAX_TREES;

  const lonAt = (x: number) => ((tile.x + x) / n) * 360 - 180;
  const build = (list: Tree[]): Instances => {
    const out = {
      length: list.length,
      positions: new Float64Array(list.length * 3),
      matrices: new Float32Array(list.length * 12),
      colors: new Uint8Array(list.length * 4),
    };
    list.forEach((t, k) => {
      out.positions[k * 3] = lonAt(t.x);
      out.positions[k * 3 + 1] = latAt(t.y);
      const [s, c] = [Math.sin(t.yaw), Math.cos(t.yaw)];
      // the card a little larger than the tree, as Streets GL has each picture's margin
      const tall = t.height * CARD_SCALE[t.species];
      const across = tall * t.width;
      // turned by its yaw, as wide as it is, as tall
      out.matrices.set([c * across, s * across, 0, -s * across, c * across, 0, 0, 0, tall, 0, 0, 0], k * 12);
      // the alpha says which picture, and whether the colour replaces its own (realistic.ts reads it)
      out.colors.set([...t.colour, 200 + t.species * 10 + (t.retint ? 5 : 0)], k * 4);
    });
    return out;
  };
  return { conifers: build(trees.filter((t) => t.conifer)), broadleaves: build(trees.filter((t) => !t.conifer)) };
}

function empty(): Instances {
  return { length: 0, positions: new Float64Array(0), matrices: new Float32Array(0), colors: new Uint8Array(0) };
}

// --------------------------------------------------------------- tiles

const TREE_LAYERS = ["landcover", "landuse", "water", "building", "transportation"];
const SGL_TREE_LAYERS = ["natural", "common", "water", "buildings", "highways", "point"];

/** A tile's trees as layer data, made once so the layers' data compares equal from frame to frame. */
type TileData = Record<keyof TileTrees, { length: number; attributes: object }>;
/** A tile's trees' shadows for one sun (`key`: its direction to half a degree). */
type TileShadows = { key: string; data: { length: number; attributes: object } };
type Entry = { trees: TileTrees | null; data: TileData | null; shadows: TileShadows | null; failed?: boolean; used: number };
/** Planted tiles, by `z/x/y`; the least recently used go once there are more than this. */
const cache = new Map<string, Entry>();
const MAX_CACHED = 256;
/** At most this many tiles loading at once. */
const MAX_LOADING = 4;
let loading = 0;

async function plant(url: string, layers: string[], tile: Tile, date: Date, entry: Entry, onLoad: () => void) {
  loading++;
  try {
    // the tile, and what is known of its ground (each may be missing: the trees are planted without it)
    const [features, land, heights, registered] = await Promise.all([
      load(url, MVTLoader, { worker: false, mvt: { shape: "geojson", coordinates: "local", layers } }) as unknown as Promise<Feature[]>,
      landCover(tile).catch(() => null),
      canopy(tile).catch(() => null),
      streetTrees(tile).catch(() => []),
    ]);
    const trees = plantTile(features, tile, date, { land, canopy: heights, registered });
    const data = (t: Instances) => ({
      length: t.length,
      attributes: {
        getPosition: { value: t.positions, size: 3 },
        instanceModelMatrix: { value: t.matrices, size: 12 },
        getColor: { value: t.colors, size: 4 },
      },
    });
    entry.trees = trees;
    entry.data = { conifers: data(trees.conifers), broadleaves: data(trees.broadleaves) };
  } catch {
    // tried again once it has been dropped from the cache
    entry.failed = true;
  } finally {
    loading--;
    onLoad();
  }
}

/** The tiles at `zoom` within `radius` metres of `[lon, lat]`, nearest first, with their distances. */
export function tilesAround(lon: number, lat: number, radius: number, zoom = TREE_TILE_ZOOM): { tile: Tile; distance: number }[] {
  const n = 2 ** zoom;
  const tx = ((lon + 180) / 360) * n;
  const ty = ((1 - Math.log(Math.tan(lat * DEG) + 1 / Math.cos(lat * DEG)) / Math.PI) / 2) * n;
  const tileMetres = (EARTH_CIRCUMFERENCE * Math.cos(lat * DEG)) / n;
  const span = radius / tileMetres;
  const out: { tile: Tile; distance: number }[] = [];
  for (let y = Math.floor(ty - span); y <= Math.floor(ty + span); y++) {
    for (let x = Math.floor(tx - span); x <= Math.floor(tx + span); x++) {
      if (y < 0 || y >= n) continue;
      // from the point to the nearest point of the tile
      const dx = Math.max(x - tx, 0, tx - (x + 1));
      const dy = Math.max(y - ty, 0, ty - (y + 1));
      const distance = Math.hypot(dx, dy) * tileMetres;
      if (distance <= radius) out.push({ tile: { x: ((x % n) + n) % n, y, z: zoom }, distance });
    }
  }
  return out.sort((a, b) => a.distance - b.distance);
}

/** A shadow's colour as it is given: lit by nothing. */
const SHADOW_MATERIAL = { ambient: 1, diffuse: 0, shininess: 1, specularColor: [0, 0, 0] as [number, number, number] };
const TREE_MATERIAL = { ambient: 0.55, diffuse: 0.55, shininess: 4, specularColor: [12, 14, 10] as [number, number, number] };

// --------------------------------------------------------------- shadows

/**
 * Each tree's shadow with the sun towards `sun` (east, north, up): its cards
 * laid flat on the ground away from the sun, each point as far along as its
 * height throws it (as a building's, `shadowStep`), just over the ground. The
 * layer cuts them to the pictures' shapes (realistic.ts's TreeShadowExtension),
 * so a tree's shadow is its own, from its trunk out. Null with the sun down.
 */
export function treeShadows(trees: TileTrees, sun: readonly number[]): Instances | null {
  const step = shadowStep(sun);
  if (!step) return null;
  const length = trees.conifers.length + trees.broadleaves.length;
  const out: Instances = { length, positions: new Float64Array(length * 3), matrices: new Float32Array(length * 12), colors: new Uint8Array(length * 4) };
  let k = 0;
  for (const kind of [trees.conifers, trees.broadleaves]) {
    out.positions.set(kind.positions, k * 3);
    out.colors.set(kind.colors, k * 4);
    for (let i = 0; i < kind.length; i++, k++) {
      const m = kind.matrices.subarray(i * 12, i * 12 + 12);
      // [x, y, z] to [x + z·east, y + z·north, 0] after the tree's own transform
      const flat = (c: number) => [m[c] + m[c + 2] * step[0], m[c + 1] + m[c + 2] * step[1], 0];
      out.matrices.set([...flat(0), ...flat(3), ...flat(6), m[9] + m[11] * step[0], m[10] + m[11] * step[1], 0.05], k * 12);
    }
  }
  return out;
}

export type TreeShadowOptions = {
  /** Towards the sun: east, north, up. */
  sun: readonly number[];
  color: [number, number, number, number];
  parameters: object;
  /** Extensions for the shadows' layers: realistic.ts's TREE_SHADOWS, which cuts them out and colours them. */
  extensions: unknown[];
};

export type TreeOptions = {
  /** The tiles' URL template, `{z}/{x}/{y}`: OpenFreeMap's, or Streets GL's (with `sgl`). */
  url: string;
  /** The tiles are Streets GL's (streets-gl.ts), at their zoom. */
  sgl?: boolean;
  /** Plant around here, `[lon, lat]`, out to `radius` metres. */
  centre: [number, number];
  radius: number;
  /** The day, for the leaves' colours. */
  date: Date;
  /** Extensions for the trees' layers (their light), and their material. */
  extensions: unknown[];
  material?: object;
  /** Their shadows, in sunshine. */
  shadow?: TreeShadowOptions | null;
  /** Asked for when a tile has been planted. */
  onLoad: () => void;
};

/** The trees around the view, one layer per tile and kind of tree. */
export function treeLayers({ url, sgl, centre, radius, date, extensions, material, shadow, onLoad }: TreeOptions): Layer[] {
  const layers: Layer[] = [];
  const now = performance.now();
  for (const { tile } of tilesAround(centre[0], centre[1], radius, sgl ? SGL_ZOOM : TREE_TILE_ZOOM)) {
    const key = `${tile.z}/${tile.x}/${tile.y}`;
    let entry = cache.get(key);
    if (!entry) {
      if (loading >= MAX_LOADING) continue;
      entry = { trees: null, data: null, shadows: null, used: now };
      cache.set(key, entry);
      const tileUrl = url.replace("{z}", String(tile.z)).replace("{x}", String(tile.x)).replace("{y}", String(tile.y));
      void plant(tileUrl, sgl ? SGL_TREE_LAYERS : TREE_LAYERS, tile, date, entry, onLoad);
    }
    entry.used = now;
    if (!entry.data || !entry.trees) continue;
    if (shadow && shadow.sun[2] > 0.02) {
      const az = Math.round((Math.atan2(shadow.sun[0], shadow.sun[1]) * 180) / Math.PI * 2);
      const alt = Math.round((Math.asin(Math.min(1, shadow.sun[2])) * 180) / Math.PI * 2);
      const sunKey = `${az},${alt}`;
      if (entry.shadows?.key !== sunKey) {
        const cast = treeShadows(entry.trees, shadow.sun);
        entry.shadows = {
          key: sunKey,
          data: {
            length: cast?.length ?? 0,
            attributes: cast
              ? {
                  getPosition: { value: cast.positions, size: 3 },
                  instanceModelMatrix: { value: cast.matrices, size: 12 },
                  // (the trees' own: which picture)
                  getColor: { value: cast.colors, size: 4 },
                }
              : {},
          },
        };
      }
      if (entry.shadows.data.length) {
        layers.push(
          new SimpleMeshLayer({
            id: `trees-${key}-shadows`,
            data: entry.shadows.data as never,
            mesh: treeCard(),
            material: SHADOW_MATERIAL,
            parameters: shadow.parameters,
            extensions: shadow.extensions as never,
            pickable: false,
            ...{ treeShadowColor: shadow.color },
          }),
        );
      }
    }
    for (const kind of ["conifers", "broadleaves"] as const) {
      const data = entry.data[kind];
      if (!data.length) continue;
      layers.push(
        new SimpleMeshLayer({
          id: `trees-${key}-${kind}`,
          data: data as never,
          mesh: treeCard(),
          material: material ?? TREE_MATERIAL,
          extensions: extensions as never,
          pickable: false,
        }),
      );
    }
  }
  // forget the tiles used longest ago
  if (cache.size > MAX_CACHED) {
    const old = [...cache.entries()].sort((a, b) => a[1].used - b[1].used).slice(0, cache.size - MAX_CACHED);
    for (const [key] of old) cache.delete(key);
  }
  return layers;
}
