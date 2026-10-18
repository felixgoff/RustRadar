// The realistic map's ground, as Streets GL draws it: from the map's own data,
// not photographs, in Streets GL's textures (github.com/StrandedKitty/streets-gl,
// MIT licence). Grass where nothing else is mapped; woods, fields, parks, sand,
// rock, the paving of towns, water; roads in asphalt with their markings at
// their real widths between pavements, railways' tracks on their ballast,
// paths and runways. Lit as the rest of the realistic map is (realistic.ts):
// wet in rain, white under snow, water rippling and reflecting the sky.

import { type Layer, LayerExtension } from "@deck.gl/core";
import { MVTLayer } from "@deck.gl/geo-layers";
import { GeoJsonLayer, PathLayer, SolidPolygonLayer } from "@deck.gl/layers";
import type { Device } from "@luma.gl/core";
import { bridgeParts, omtDeck, type Lines, type TileIndex } from "./bridges";
import { LC } from "./landcover";
import { SGL_FROM_ZOOM, SGL_TILES, SGL_ZOOM, sglFetch, sglPath } from "./streets-gl";
import { realisticLight } from "./realistic";
import { withRoadWidths } from "./road-widths";
import { type GROUND_TEXTURES, realisticTextures, textureLayer } from "./realistic-textures";

type RGB = [number, number, number];
type RGBA = [number, number, number, number];

/**
 * A kind of ground: its texture (or water), how many metres a repeat of it
 * covers (0: laid along a line, across its whole width), how rough it is,
 * whether snow lies on it (1) or turns to slush (0.5), and whether rain makes
 * it shine.
 */
type Surface = { texture: (typeof GROUND_TEXTURES)[number] | "water"; metres: number; rough: number; snow: number; paved?: boolean };

/** The surfaces, by the id a feature's colour carries in its alpha (255 less the id); each texture's metres as Streets GL lays it. */
const SURFACES: Record<number, Surface> = {
  // where nothing is mapped: Streets GL's terrain (its detail repeats every 76 m)
  0: { texture: "terrain", metres: 76, rough: 0.9, snow: 1 },
  1: { texture: "park", metres: 20, rough: 0.9, snow: 1 },
  2: { texture: "forest", metres: 15, rough: 0.95, snow: 1 },
  3: { texture: "farmland0", metres: 50, rough: 0.95, snow: 1 },
  4: { texture: "grass", metres: 25, rough: 0.95, snow: 1 },
  5: { texture: "garden", metres: 16, rough: 0.92, snow: 1 },
  6: { texture: "sand", metres: 12, rough: 0.9, snow: 1 },
  7: { texture: "rock", metres: 32, rough: 0.85, snow: 1 },
  8: { texture: "soil", metres: 25, rough: 0.95, snow: 1 },
  9: { texture: "pavement", metres: 10, rough: 0.8, snow: 0.5, paved: true },
  10: { texture: "asphalt", metres: 20, rough: 0.85, snow: 0.5, paved: true },
  11: { texture: "water", metres: 0, rough: 0.05, snow: 0 },
  12: { texture: "gravel", metres: 8, rough: 0.9, snow: 1 },
  13: { texture: "cobblestone", metres: 6, rough: 0.75, snow: 0.5, paved: true },
  // glaciers and ice caps: snow, whatever the weather
  14: { texture: "sand", metres: 10, rough: 0.6, snow: 1 },
  // a pitch (one for every game: the map doesn't say which), and wooden planks
  15: { texture: "pitch", metres: 20, rough: 0.9, snow: 1 },
  16: { texture: "wood", metres: 4, rough: 0.8, snow: 1 },
  // a field as mapped (Streets GL's tiles): its crop and the lie of its rows in its colour
  17: { texture: "farmland0", metres: 50, rough: 0.95, snow: 1 },
  // lines: laid along them
  32: { texture: "road", metres: 0, rough: 0.8, snow: 0.5, paved: true },
  33: { texture: "road-unmarked", metres: 0, rough: 0.8, snow: 0.5, paved: true },
  34: { texture: "dirt", metres: 0, rough: 0.95, snow: 1 },
  35: { texture: "railway", metres: 0, rough: 0.8, snow: 0.5 },
  39: { texture: "tram", metres: 0, rough: 0.8, snow: 0.5, paved: true },
  40: { texture: "wood", metres: 0, rough: 0.8, snow: 1 },
  // lines with a texture of the world's
  36: { texture: "pavement", metres: 10, rough: 0.8, snow: 0.5, paved: true },
  37: { texture: "asphalt", metres: 20, rough: 0.8, snow: 0.5, paved: true },
  38: { texture: "gravel", metres: 8, rough: 0.9, snow: 1 },
  41: { texture: "cycleway", metres: 8, rough: 0.8, snow: 0.5, paved: true },
};
const FARMLAND = 3;
const FIELD = 17;
const WATER_ID = 11;
const ICE = 14;

const paint = (id: number, [r, g, b]: RGB = [255, 255, 255]): RGBA => [r, g, b, 255 - id];
const NONE: RGBA = [0, 0, 0, 0];

/** The ground where nothing else is mapped, as in Streets GL: its terrain in its biome's colour (temperate). */
export const LAND = paint(0, [58, 81, 29]);
/** The globe itself, showing only past the map's edge at the poles: ice. */
export const POLES: RGB = [226, 234, 240];

// tints multiply a texture's own colours: white leaves them as they are
const AREAS: Record<string, RGBA> = {
  ocean: paint(WATER_ID, [200, 214, 222]),
  water: paint(WATER_ID),
  pool: paint(WATER_ID, [150, 255, 255]),
  wood: paint(2),
  scrub: paint(2, [230, 236, 214]),
  grass: paint(4),
  park: paint(1),
  meadow: paint(4, [255, 246, 206]),
  golf: paint(1, [236, 255, 228]),
  heath: paint(4, [236, 220, 196]),
  farmland: paint(FARMLAND),
  orchard: paint(5, [232, 240, 214]),
  allotments: paint(8, [226, 236, 206]),
  wetland: paint(4, [196, 214, 200]),
  sand: paint(6),
  rock: paint(7),
  ice: paint(14),
  residential: paint(5),
  institutional: paint(5, [236, 236, 226]),
  commercial: paint(9),
  industrial: paint(10, [236, 236, 236]),
  railway: paint(12),
  cemetery: paint(1, [228, 236, 222]),
  pitch: paint(15),
  athletics: paint(8, [255, 150, 120]),
  playground: paint(6),
  quarry: paint(8, [236, 228, 214]),
  aerodrome: paint(4, [236, 244, 222]),
  apron: paint(10, [255, 255, 255]),
  runway: paint(10, [180, 180, 186]),
  taxiway: paint(10, [214, 214, 218]),
  square: paint(13),
  platform: paint(9),
  pier: paint(16),
  bridge: paint(9),
};

/**
 * The paint of land cover's classes (landcover.ts) where the map's data has
 * nothing: woods, felled woods, mires, fields, grass, scrub, heath, bare
 * rock, ice. Towns and water are left to the map's own data.
 */
export const LAND_COVER_PAINTS: Partial<Record<number, RGBA>> = {
  [LC.CONIFERS]: AREAS.wood,
  [LC.MIXED]: AREAS.wood,
  [LC.BROADLEAVES]: AREAS.wood,
  [LC.FOREST]: AREAS.wood,
  [LC.CLEARCUT]: AREAS.heath,
  [LC.WETLAND]: AREAS.wetland,
  [LC.ARABLE]: AREAS.farmland,
  [LC.GRASS]: AREAS.meadow,
  [LC.SHRUB]: AREAS.scrub,
  [LC.HEATH]: AREAS.heath,
  [LC.BARE]: AREAS.rock,
  [LC.SNOW]: AREAS.ice,
};

type Properties = Record<string, unknown>;
type Feature = { properties: Properties };
/** An area's paint, or none (left out). */
export function areaPaint({ properties: p }: Feature): RGBA {
  const [cls, sub] = [String(p.class ?? ""), String(p.subclass ?? "")];
  switch (p.layerName) {
    case "water":
      return cls === "ocean" ? AREAS.ocean : cls === "swimming_pool" ? AREAS.pool : AREAS.water;
    case "landcover":
      switch (cls) {
        case "wood":
          return AREAS.wood;
        case "farmland":
          return sub === "orchard" || sub === "vineyard" || sub === "plant_nursery" ? AREAS.orchard : AREAS.farmland;
        case "grass":
          if (sub === "scrub" || sub === "shrubbery") return AREAS.scrub;
          if (sub === "heath" || sub === "fell") return AREAS.heath;
          if (sub === "golf_course") return AREAS.golf;
          if (sub === "meadow" || sub === "grassland") return AREAS.meadow;
          if (sub === "allotments") return AREAS.allotments;
          if (sub === "park" || sub === "garden" || sub === "village_green" || sub === "recreation_ground") return AREAS.park;
          return AREAS.grass;
        case "wetland":
          return AREAS.wetland;
        case "sand":
          return AREAS.sand;
        case "rock":
          return AREAS.rock;
        case "ice":
          return AREAS.ice;
        default:
          return NONE;
      }
    case "landuse":
      switch (cls) {
        case "residential":
          return AREAS.residential;
        case "school":
        case "university":
        case "college":
        case "kindergarten":
        case "hospital":
        case "library":
        case "stadium":
          return AREAS.institutional;
        case "commercial":
        case "retail":
        case "garages":
        case "bus_station":
        case "dam":
          return AREAS.commercial;
        case "industrial":
          return AREAS.industrial;
        case "railway":
          return AREAS.railway;
        case "cemetery":
          return AREAS.cemetery;
        case "pitch":
          return AREAS.pitch;
        case "track":
          return AREAS.athletics;
        case "playground":
          return AREAS.playground;
        case "quarry":
          return AREAS.quarry;
        case "theme_park":
        case "zoo":
          return AREAS.park;
        default:
          // suburbs, quarters and neighbourhoods are names, not ground
          return NONE;
      }
    case "aeroway":
      switch (cls) {
        case "aerodrome":
          return AREAS.aerodrome;
        case "apron":
          return AREAS.apron;
        case "runway":
          return AREAS.runway;
        case "taxiway":
          return AREAS.taxiway;
        default:
          return NONE;
      }
    case "transportation":
      if (cls === "pier") return AREAS.pier;
      if (cls === "bridge") return AREAS.bridge;
      if (sub === "platform") return AREAS.platform;
      return AREAS.square;
    // Streets GL's tiles (streets-gl.ts), by `type`, as Streets GL draws them
    case "common":
    case "natural":
      return p.type === "farmland" ? fieldPaint(p) : (SGL_AREAS[String(p.type)] ?? NONE);
    case "highways":
      return p.pathType === "pedestrian" || p.pathType === "footway" || p.pathType === "platform" ? AREAS.platform : AREAS.apron;
    default:
      return NONE;
  }
}

/**
 * A mapped field's paint: in its colour, the lie of its rows (along its
 * longer side, from the oriented box Streets GL's tiles give it: red, 0 to
 * 180°), its crop (green, one of three) and its shade (blue), the last two
 * picked by its OpenStreetMap id.
 */
function fieldPaint(p: Properties): RGBA {
  const at = (k: string) => Number(p[k]) || 0;
  const [ax, ay] = [at("@ombb10") - at("@ombb00"), at("@ombb11") - at("@ombb01")];
  const [bx, by] = [at("@ombb20") - at("@ombb10"), at("@ombb21") - at("@ombb11")];
  const [dx, dy] = Math.hypot(ax, ay) >= Math.hypot(bx, by) ? [ax, ay] : [bx, by];
  const angle = (((Math.atan2(dy, dx) % Math.PI) + Math.PI) % Math.PI) / Math.PI;
  const id = Number(p.osmId) || 0;
  const h = (salt: number) => {
    const x = Math.sin(id * 12.9898 + salt * 78.233) * 43758.5453;
    return x - Math.floor(x);
  };
  return [Math.round(angle * 255), Math.floor(h(1) * 3) * 100, Math.round(h(2) * 255), 255 - FIELD];
}

const SGL_AREAS: Record<string, RGBA> = {
  forest: AREAS.wood,
  scrub: AREAS.scrub,
  grass: AREAS.grass,
  garden: AREAS.residential,
  farmland: AREAS.farmland,
  fairway: AREAS.golf,
  pitch: AREAS.pitch,
  playground: AREAS.pitch,
  dogPark: AREAS.pitch,
  sand: AREAS.sand,
  rock: AREAS.rock,
  construction: AREAS.quarry,
  brownfield: AREAS.quarry,
  parking: AREAS.apron,
  apron: AREAS.apron,
  helipad: AREAS.apron,
  bridge: AREAS.bridge,
  pier: AREAS.pier,
};

/** A line's surface: its full width in metres and paint, and its edging either side (pavements, ballast) and paint. */
type Line = { width: number; paint: RGBA; edge: number; edgePaint: RGBA };
const PAVEMENT = paint(36);
const SHOULDER = paint(37, [210, 210, 210]);
const BALLAST = paint(38);
const road = (width: number, marked: boolean, edge = 0, edgePaint = PAVEMENT): Line => ({
  width,
  paint: paint(marked ? 32 : 33),
  edge,
  edgePaint,
});
const ROADS: Record<string, Line> = {
  motorway: road(11, true, 1.5, SHOULDER),
  trunk: road(10, true, 1.5, SHOULDER),
  primary: road(9, true, 2),
  secondary: road(8, true, 2),
  tertiary: road(7, false, 1.8),
  minor: road(6, false, 1.6),
  busway: road(7, false, 1.8),
  service: road(4, false),
  raceway: road(10, true),
  track: { width: 3, paint: paint(34), edge: 0, edgePaint: NONE },
  // the track itself, rails on sleepers, on its ballast
  rail: { width: 2.6, paint: paint(35), edge: 1, edgePaint: BALLAST },
  transit: { width: 2.2, paint: paint(35), edge: 0.6, edgePaint: BALLAST },
  pier: { width: 3, paint: paint(40), edge: 0, edgePaint: NONE },
};
const PATHS: Record<string, Line> = {
  footway: { width: 2, paint: paint(36), edge: 0, edgePaint: NONE },
  cycleway: { width: 2.5, paint: paint(41), edge: 0, edgePaint: NONE },
  pedestrian: { width: 5, paint: paint(36), edge: 0, edgePaint: NONE },
  path: { width: 1.6, paint: paint(38, [236, 226, 206]), edge: 0, edgePaint: NONE },
};
const WATERWAYS: Record<string, number> = { river: 18, canal: 12, stream: 3, drain: 1.5, ditch: 1.5 };
const AEROWAYS: Record<string, Line> = {
  runway: { width: 45, paint: paint(37, [180, 180, 186]), edge: 0, edgePaint: NONE },
  taxiway: { width: 18, paint: paint(37, [214, 214, 218]), edge: 0, edgePaint: NONE },
};
const TRAM: Line = { width: 2.2, paint: paint(39), edge: 0, edgePaint: NONE };
const NO_LINE: Line = { width: 0, paint: NONE, edge: 0, edgePaint: NONE };

/** How a line is drawn on the ground: a road, railway, path, river or runway (tunnels and ferries not at all). */
export function lineOf({ properties: p }: Feature): Line {
  const [cls, sub] = [String(p.class ?? ""), String(p.subclass ?? "")];
  switch (p.layerName) {
    case "transportation":
      if (p.brunnel === "tunnel" || cls === "ferry" || cls.endsWith("_construction")) return NO_LINE;
      if (cls === "path") return PATHS[sub] ?? PATHS[sub === "steps" || sub === "platform" ? "footway" : "path"];
      // a tram's rails are set in the street; light rail runs on its own ballast, as a railway
      if (cls === "transit" && sub === "tram") return TRAM;
      return asSurveyed(ROADS[cls] ?? NO_LINE, p, false);
    case "waterway": {
      if (p.brunnel === "tunnel") return NO_LINE;
      return { width: WATERWAYS[cls] ?? 2, paint: AREAS.water, edge: 0, edgePaint: NONE };
    }
    case "aeroway":
      return AEROWAYS[cls] ?? NO_LINE;
    // Streets GL's tiles: roads as wide as their lanes, of their own surface; railways; waterways
    case "highways": {
      if (p.type === "railway") return p.railwayType === "tram" ? TRAM : ROADS.rail;
      const path = sglPath(p);
      if (!path) return NO_LINE;
      const own = AEROWAYS[path.kind] ?? PATHS[path.kind] ?? ROADS[path.kind] ?? NO_LINE;
      const surface = SGL_SURFACES[path.surface];
      const mappedWidth = Number(p.width) > 0;
      return asSurveyed({
        ...own,
        width: path.width ?? own.width,
        // a road's own surface where it isn't asphalt; a path's as mapped
        paint: surface && (path.surface !== "asphalt" || PATHS[path.kind]) ? surface : own.paint,
        edge: p.sidewalkSide ? Math.max(own.edge, 2) : own.edge,
        edgePaint: p.sidewalkSide ? PAVEMENT : own.edgePaint,
      }, p, mappedWidth || typeof p.surface === "string");
    }
    case "water":
      if (p.type !== "waterway") return NO_LINE;
      return { width: Number(p.width) || WATERWAYS[String(p.waterwayType)] || 2, paint: AREAS.water, edge: 0, edgePaint: NONE };
    default:
      return NO_LINE;
  }
}

/**
 * A road as Trafikverket's survey has it (road-widths.ts), where it has it:
 * as wide as it is (unless `mapped` already says), and gravel where it is.
 */
function asSurveyed(line: Line, p: Properties, mapped: boolean): Line {
  const width = Number(p.nvdbWidth) || 0;
  if (!width || mapped) return line;
  return { ...line, width, paint: Number(p.nvdbGravel) > 0 ? paint(34) : line.paint };
}

/** Whether a tile's line is a road the survey may have (not a railway, path, runway or pier). */
const isCarRoad = (p: Properties) => {
  if (p.layerName === "transportation") return String(p.class) in ROADS && !["rail", "transit", "pier", "track"].includes(String(p.class));
  if (p.layerName !== "highways" || p.type === "railway") return false;
  const path = sglPath(p);
  return Boolean(path && path.kind in ROADS && path.kind !== "track");
};
/** A tile's lines with the survey's roads, kept the same object while they are (for the layers' data to compare equal). */
const surveyed = new WeakMap<object, Binary>();
function withSurvey(lines: Binary, tile: TileIndex): Binary {
  const own = lines.lines as unknown as Lines;
  const roads = withRoadWidths(own, tile, isCarRoad);
  if (roads === own) return lines;
  let out = surveyed.get(roads);
  if (!out) surveyed.set(roads, (out = { ...lines, lines: roads as never }));
  return out;
}

/** Streets GL's road surfaces (streets-gl.ts), as the ground's line paints: asphalt the road's own. */
const SGL_SURFACES: Record<string, RGBA | undefined> = {
  asphalt: paint(37),
  concrete: paint(37, [226, 226, 222]),
  cobblestone: paint(13),
  dirt: paint(34),
  sand: paint(34, [255, 236, 200]),
  wood: paint(40),
};

const linePaint = (f: Feature) => lineOf(f).paint;
const onRails = ({ properties: p }: Feature) => p.class === "rail" || p.class === "transit";
const lineWidth = (f: Feature) => lineOf(f).width;
const edgePaint = (f: Feature) => lineOf(f).edgePaint;
const edgeWidth = (f: Feature) => {
  const line = lineOf(f);
  return line.edge ? line.width + 2 * line.edge : 0;
};

// --------------------------------------------------------------- shading

const SURFACE_COUNT = Math.max(...Object.keys(SURFACES).map(Number)) + 1;
/** A GLSL table of one of the surfaces' properties, by id (unused ids: grass's). */
const table = (name: string, value: (s: Surface) => number) =>
  `const float ${name}[${SURFACE_COUNT}] = float[${SURFACE_COUNT}](${Array.from({ length: SURFACE_COUNT }, (_, i) =>
    value(SURFACES[i] ?? SURFACES[0]).toFixed(3),
  ).join(", ")});`;

const groundFs = /* glsl */ `\
uniform mediump sampler2DArray rground_color;
uniform mediump sampler2DArray rground_normal;
uniform sampler2D rground_water;
uniform sampler2D rground_earth;
uniform sampler2D rground_night;
in vec2 rg_e;
${table("GS_LAYER", (s) => (s.texture === "water" ? -1 : textureLayer.ground(s.texture)))}
${table("GS_METRES", (s) => s.metres)}
${table("GS_ROUGH", (s) => s.rough)}
${table("GS_SNOW", (s) => s.snow)}
${table("GS_PAVED", (s) => (s.paved ? 1 : 0))}
`;

const groundInject = {
  "vs:DECKGL_FILTER_GL_POSITION": /* glsl */ `
  {
    // where it is on the picture of the Earth: on the globe, its own place; on the flat map (close in) the view's centre is near enough
    vec2 rg_ll = project.projectionMode == PROJECTION_MODE_GLOBE ? geometry.worldPosition.xy : rlight.anchor;
    rg_e = vec2((rg_ll.x + 180.0) / 360.0, (90.0 - rg_ll.y) / 180.0);
  }
  `,
  "fs:DECKGL_FILTER_COLOR": /* glsl */ `
  {
    float id = floor((1.0 - color.a) * 255.0 + 0.5);
    if (id >= ${SURFACE_COUNT}.0) {
      color.a = 0.0;
    } else {
      int i = int(id);
      vec3 tint = rl_linear(color.rgb);
      float layer = GS_LAYER[i];
      vec3 v = normalize(rl_eye);
      vec3 n = vec3(0.0, 0.0, 1.0);
      vec3 albedo;
      float rough = GS_ROUGH[i];
      // texture coordinates: fixed to the world, whole repeats to the period; or along a line, across its width
      vec2 uv = GS_METRES[i] > 0.0
        ? rl_t * max(1.0, floor(rlight.periodMetres / GS_METRES[i] + 0.5))
        : vec2(geometry.uv.x * 0.5 + 0.5, geometry.uv.y * 0.5);
      if (layer < 0.0) {
        // water: Streets GL's ripples, two of them drifting across each other, under a mirror of the sky
        vec2 w = rl_t * max(1.0, floor(rlight.periodMetres / 18.0 + 0.5));
        vec3 n1 = texture(rground_water, w + vec2(rlight.time * 0.011, rlight.time * 0.007)).xyz * 2.0 - 1.0;
        vec3 n2 = texture(rground_water, w * 1.7 - vec2(rlight.time * 0.008, -rlight.time * 0.012)).xyz * 2.0 - 1.0;
        n = normalize(vec3(n1.xy + n2.xy, n1.z * n2.z) * vec3(0.6, 0.6, 1.0));
        albedo = vec3(0.012, 0.025, 0.035) * tint * 2.0;
        // ripples too small to see from up here still break up the sun's reflection: a soft sheen, not a mirror's spot
        float unseen = smoothstep(0.04, 0.5, rl_cells(w));
        rough = mix(rough, 0.25, unseen);
        rl_glint = 1.0 - 0.8 * unseen;
      } else {
        // Fields: the land round points scattered over it (40 to the clouds' period, about 250 m, numbered
        // the world over so the pattern repeats only every 11 km or so), so each field has straight edges
        // and its own shape, size and lie; to each its crop (one of Streets GL's three) and its rows turned
        // its own way. The crops' colours are drawn closer together than in the pictures, the more so the
        // smaller the fields look (side by side, far off, the soil's brown turns purple in the air), and
        // fields too small to see take the crops' mean, not a flicker.
        // A field mapped as one (Streets GL's tiles) has its crop, rows and shade in its colour.
        vec3 c3 = vec3(0.0);
        float crops = 0.0, even = 0.0, shade = 1.0;
        bool field = i == ${FARMLAND} || i == ${FIELD};
        if (field) {
          vec2 p = rl_c * 40.0;
          float seed, a;
          if (i == ${FIELD}) {
            seed = color.b;
            layer += floor(color.g * 2.999);
            a = 3.14159 * color.r;
            tint = vec3(1.0);
          } else {
            vec2 cell = floor(p);
            float best = 1e9;
            vec2 near = cell;
            for (int y = -1; y <= 1; y++) {
              for (int x = -1; x <= 1; x++) {
                vec2 o = cell + vec2(float(x), float(y));
                vec2 q = o + 0.1 + 0.8 * vec2(rl_hash(o, 40.0), rl_hash(o + 17.0, 40.0));
                float d = dot(p - q, p - q);
                if (d < best) {
                  best = d;
                  near = o;
                }
              }
            }
            seed = rl_hash(near + 29.0, 40.0);
            layer += floor(seed * 2.999);
            a = 6.2832 * fract(seed * 7.31);
          }
          uv = mat2(cos(a), sin(a), -sin(a), cos(a)) * uv;
          float first = GS_LAYER[${FARMLAND}];
          c3 = (textureLod(rground_color, vec3(0.5, 0.5, first), 16.0).rgb
            + textureLod(rground_color, vec3(0.5, 0.5, first + 1.0), 16.0).rgb
            + textureLod(rground_color, vec3(0.5, 0.5, first + 2.0), 16.0).rgb) / 3.0;
          even = mix(0.5, 0.85, smoothstep(0.004, 0.04, rl_cells(p)));
          shade = 0.93 + 0.14 * fract(seed * 3.7);
          crops = smoothstep(0.35, 0.9, rl_cells(rl_t * 3.0));
        }
        // (the images are stored top row first: v runs up them)
        vec3 at = vec3(uv.x, -uv.y, layer);
        vec4 c = texture(rground_color, at);
        if (field) {
          vec3 own = textureLod(rground_color, vec3(0.5, 0.5, layer), 16.0).rgb;
          c.rgb *= mix(vec3(1.0), c3 / max(own, vec3(1e-3)), even) * shade;
          c.rgb = mix(c.rgb, c3, crops);
        }
        vec3 nm = texture(rground_normal, at).xyz * 2.0 - 1.0;
        n = normalize(vec3(nm.xy, max(nm.z, 0.3)));
        // a little unevenness over tens of metres, so the repeats don't show (gone where it would only speckle)
        float grain = 1.0 - smoothstep(0.25, 1.0, rl_cells(rl_t * 32.0));
        albedo = c.rgb * tint * (1.0 + 0.28 * grain * (rl_noise(rl_t, 32.0) - 0.5));
        // Zoomed out the map's data hardly says what the land is (desert, savanna and tundra would all be
        // grass): there its colour is the Earth's own, from the picture of it, taking over as a pixel grows
        // past 100 m or so. Closer in, land nothing is mapped on keeps the Earth's colour where that is
        // dry (sand, rock), in the terrain's detail; where it is green, Streets GL's biome green.
        vec4 earth = texture(rground_earth, rg_e);
        float far = earth.a * smoothstep(120.0, 1200.0, rl_cells(rl_t) * rlight.periodMetres);
        if (i == 0) {
          const vec3 LUM = vec3(0.2126, 0.7152, 0.0722);
          // (the detail's own unevenness, around its tinted mean, on the Earth's colour)
          vec3 mean = textureLod(rground_color, vec3(0.5, 0.5, layer), 16.0).rgb * tint;
          float dry = earth.a * smoothstep(0.03, 0.15, (earth.r - earth.g) / (earth.r + earth.g + 1e-4));
          albedo = mix(albedo, earth.rgb * dot(albedo, LUM) / max(dot(mean, LUM), 1e-4), dry);
        }
        if (i != ${ICE}) albedo = mix(albedo, earth.rgb, far);
        // snow lies on the land, slush on the roads; rain darkens what is paved and makes it shine
        // (the weather where the camera is: not the whole world's, zoomed out)
        float snow = i == ${ICE} ? 1.0 : rlight.snow * GS_SNOW[i] * (1.0 - far);
        albedo = mix(albedo, vec3(0.82, 0.84, 0.88) * mix(0.75, 1.0, GS_SNOW[i]), snow);
        float wet = rlight.wet * mix(0.35, 1.0, GS_PAVED[i]) * (1.0 - far);
        albedo *= 1.0 - 0.45 * wet;
        rough = mix(rough, 0.12, wet * GS_PAVED[i] * (1.0 - rlight.snow));
        n = normalize(mix(n, vec3(0.0, 0.0, 1.0), wet * 0.6));
      }
      vec3 lit = rl_shade(albedo, n, v, rough, 0.0, 1.0, rl_sunVisible());
    #ifndef RL_LINE
      // from far over the globe, its towns' lights where the sun has set (NASA's Black Marble)
      if (rlight.globe > 0.0) {
        float dark = 1.0 - smoothstep(-0.12, 0.03, rl_sun.z);
        lit += rl_linear(texture(rground_night, rg_e).rgb) * 0.55 * rlight.globe * dark;
      }
    #endif
      color = vec4(rl_display(rl_aerial(lit)), 1.0);
    }
  }
  `,
};

const groundModule = {
  name: "rground",
  vs: "out vec2 rg_e;\n",
  fs: groundFs,
} as const;

type ExtLayer = Layer & { context: { device: Device }; setShaderModuleProps(props: object): void };

/** The ground's surfaces, by the id their colour carries (see `paint`), lit as the realistic map is. */
export class GroundExtension extends LayerExtension {
  static extensionName = "RealisticGroundExtension";

  getShaders(this: Layer) {
    // a line's sides lie a way out from its middle, which the textures fixed to the world need on the globe
    const defines = this instanceof PathLayer ? { RL_LINE: 1 } : {};
    return { modules: [realisticLight.module, groundModule], inject: groundInject, defines };
  }

  draw(this: ExtLayer, params: Parameters<typeof realisticLight.props>[0]) {
    const textures = realisticTextures(this.context.device);
    this.setShaderModuleProps({
      rlight: realisticLight.props(params),
      rground: {
        rground_color: textures.ground.color,
        rground_normal: textures.ground.normal,
        rground_water: textures.water,
        rground_earth: textures.earth,
        rground_night: textures.night,
      },
    });
  }
}

export const GROUND_SURFACES = new GroundExtension();

// --------------------------------------------------------------- layers

// Each tile is drawn three times over: its areas, then the edging of its
// lines, then the lines themselves, so no road's pavement covers another's
// asphalt where they meet. The loader's binary tile is split once into areas
// and lines (an empty part in each), so each pass only builds what it draws.

type Binary = Record<"points" | "lines" | "polygons", Record<string, unknown>>;
const ids = () => ({ value: new Uint32Array(0), size: 1 });
const noGeometry = (size: number) => ({
  positions: { value: new Float64Array(0), size },
  featureIds: ids(),
  globalFeatureIds: ids(),
  numericProps: {},
  properties: [],
  fields: [],
});
const parts = new WeakMap<object, { areas: Binary; lines: Binary }>();

type GeoFeature = { geometry?: { type?: string } };
const featureParts = new WeakMap<object, { areas: GeoFeature[]; lines: GeoFeature[] }>();
/** A tile's GeoJSON features, as areas and lines. */
function splitFeatures(features: GeoFeature[]): { areas: GeoFeature[]; lines: GeoFeature[] } {
  let p = featureParts.get(features);
  if (!p) {
    const type = (f: GeoFeature) => f.geometry?.type ?? "";
    p = { areas: features.filter((f) => type(f).endsWith("Polygon")), lines: features.filter((f) => type(f).endsWith("LineString")) };
    featureParts.set(features, p);
  }
  return p;
}
function split(tile: Binary): { areas: Binary; lines: Binary } {
  let p = parts.get(tile);
  if (!p) {
    const size = Number((tile.polygons.positions as { size?: number } | undefined)?.size ?? 2);
    const points = { ...noGeometry(size), type: "Point" };
    const lines = { ...noGeometry(size), type: "LineString", pathIndices: { value: new Uint32Array([0]), size: 1 } };
    const polygons = {
      ...noGeometry(size),
      type: "Polygon",
      polygonIndices: { value: new Uint32Array([0]), size: 1 },
      primitivePolygonIndices: { value: new Uint32Array([0]), size: 1 },
      triangles: { value: new Uint32Array(0), size: 1 },
    };
    p = { areas: { ...tile, points, lines }, lines: { ...tile, points, polygons } };
    parts.set(tile, p);
  }
  return p;
}

/** The shade a bridge throws on what it crosses (not lit: a shadow). */
const BRIDGE_SHADE: RGBA = [16, 20, 14, 70];

/**
 * One tile of ground: a GeoJsonLayer to MVTLayer (which expects one), drawn
 * by the three passes rather than by GeoJsonLayer's own state; close in, its
 * bridges raised to their decks over their shade.
 */
class GroundTile extends GeoJsonLayer {
  static layerName = "GroundTile";

  updateState() {
    // nothing of its own: see renderLayers
  }

  renderLayers() {
    // the props passed in (a spread misses `data`, which deck.gl keeps behind a getter as it may load)
    const own = { ...this.props } as Record<string, unknown>;
    const id = this.props.id;
    const data = this.props.data as unknown as Binary | GeoFeature[] | null;
    if (!data) return [];
    // binary in the flat projection; on the globe, GeoJSON features
    const { areas, lines: tileLines } = Array.isArray(data) ? splitFeatures(data) : split(data);
    const tile = (this.props as { tile?: { index: TileIndex } }).tile?.index;
    // the survey's roads at their widths (binary tiles, the flat map's)
    const lines = !Array.isArray(data) && tile ? withSurvey(tileLines as Binary, tile) : tileLines;
    const bridges = !Array.isArray(data) && tile ? bridgeParts((lines as Binary).lines as unknown as Lines, tile, omtDeck) : null;
    const onGround = bridges ? { ...(lines as Binary), lines: bridges.ground } : lines;
    const layers = [
      new GeoJsonLayer({ ...own, id: `${id}-areas`, data: areas, filled: true, stroked: false } as never),
      new GeoJsonLayer({ ...own, id: `${id}-edges`, data: onGround, filled: false, getLineColor: edgePaint, getLineWidth: edgeWidth } as never),
      new GeoJsonLayer({ ...own, id: `${id}-lines`, data: onGround, filled: false } as never),
    ];
    if (bridges) {
      const raised = { ...(lines as Binary), lines: bridges.raised };
      // (the shade plain: not one of the ground's surfaces, but kept to the tile)
      const unlit = (own.extensions as unknown[]).filter((e) => !(e instanceof GroundExtension));
      layers.push(
        new GeoJsonLayer({
          ...own,
          id: `${id}-bridge-shade`,
          data: { ...(lines as Binary), lines: bridges.under },
          filled: false,
          getLineColor: BRIDGE_SHADE,
          getLineWidth: (f: Feature) => Math.max(lineWidth(f), edgeWidth(f)) + 2,
          extensions: unlit,
        } as never),
        new GeoJsonLayer({ ...own, id: `${id}-bridge-edges`, data: raised, filled: false, getLineColor: edgePaint, getLineWidth: edgeWidth } as never),
        new GeoJsonLayer({ ...own, id: `${id}-bridge-lines`, data: raised, filled: false } as never),
        // and in the depth buffer, for what is drawn after to pass under them: the
        // roads' bridges only (the railways' carry the rail map, signal blocks and
        // trains, drawn after on the same deck, which would fight with it)
        new GeoJsonLayer({
          ...own,
          id: `${id}-bridge-decks`,
          data: raised,
          filled: false,
          getLineColor: NONE,
          getLineWidth: (f: Feature) => (onRails(f) ? 0 : Math.max(lineWidth(f), edgeWidth(f))),
          extensions: unlit,
          parameters: { ...(own.parameters as object), depthWriteEnabled: true },
        } as never),
      );
    }
    return layers;
  }
}

const renderGround = (props: object) => new GroundTile(props as never);

const GROUND_LAYERS = ["landuse", "landcover", "aeroway", "water", "waterway", "transportation"];
const loadOptions = new Map<string, object>();

/** The ground's vector tiles; MVTLayer decodes per projection, so each gets its own layer (and cache). */
export const groundLayer = (url: string, flat: boolean, workerUrl: string, extensions: unknown[], parameters: object, roadsVersion = 0) => {
  let options = loadOptions.get(workerUrl);
  if (!options) loadOptions.set(workerUrl, (options = { mvt: { layers: GROUND_LAYERS, workerUrl } }));
  return new MVTLayer({
    id: flat ? "ground-flat" : "ground-globe",
    data: url,
    // (its tiles drawn again as the survey's roads come in)
    roadsVersion,
    minZoom: 0,
    maxZoom: 14,
    loadOptions: options,
    renderSubLayers: renderGround as never,
    getFillColor: areaPaint as never,
    getLineColor: linePaint as never,
    getLineWidth: lineWidth as never,
    lineWidthUnits: "meters",
    // far out, the main roads and rivers stay a hairline
    lineWidthMinPixels: 0.6,
    lineJointRounded: true,
    lineCapRounded: true,
    getPointRadius: 0,
    extensions: extensions as never,
    parameters,
    pickable: false,
  });
};

const SGL_GROUND_LAYERS = ["common", "natural", "water", "highways"];
const sglOptions = new Map<string, object>();

/** The ground close in from Streets GL's tiles (streets-gl.ts): each field, garden and road as mapped. Flat projection only. */
export const sglGroundLayer = (workerUrl: string, extensions: unknown[], parameters: object, roadsVersion = 0) => {
  let options = sglOptions.get(workerUrl);
  if (!options) sglOptions.set(workerUrl, (options = { mvt: { layers: SGL_GROUND_LAYERS, workerUrl } }));
  return new MVTLayer({
    id: "ground-sgl",
    data: SGL_TILES,
    roadsVersion,
    minZoom: SGL_FROM_ZOOM,
    maxZoom: SGL_ZOOM,
    loadOptions: options,
    fetch: sglFetch as never,
    renderSubLayers: renderGround as never,
    getFillColor: areaPaint as never,
    getLineColor: linePaint as never,
    getLineWidth: lineWidth as never,
    lineWidthUnits: "meters",
    lineJointRounded: true,
    lineCapRounded: true,
    getPointRadius: 0,
    extensions: extensions as never,
    parameters,
    pickable: false,
  });
};

const LAND_DATA = [
  [
    [
      [-180, -85.06],
      [180, -85.06],
      [180, 85.06],
      [-180, 85.06],
    ],
  ],
];
/** The land under the ground's tiles: grass all over, the sea and everything else drawn over it. */
export const landLayer = (extensions: unknown[], parameters: object) =>
  new SolidPolygonLayer({
    id: "ground-land",
    data: LAND_DATA,
    getPolygon: (d: unknown) => d as never,
    getFillColor: LAND,
    extensions: extensions as never,
    parameters,
  });
