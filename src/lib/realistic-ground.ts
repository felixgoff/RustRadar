// The realistic map's ground, as Streets GL draws it: from the map's own data,
// not photographs. Grass where nothing else is mapped; woods, fields, parks,
// sand, rock, snow, the paving of towns, water; roads at their real widths in
// asphalt between pavements, railways on their ballast, paths, runways. Each
// surface has a texture of its own (a shader extension: grass mottled, asphalt
// grainy, fields patchy, water rippling, reflecting the sky and catching the
// sun), fixed to the world, fading out where it would be finer than a pixel.

import { type Layer, LayerExtension, project } from "@deck.gl/core";
import { MVTLayer } from "@deck.gl/geo-layers";
import { GeoJsonLayer, SolidPolygonLayer } from "@deck.gl/layers";
import { look } from "./realistic";

type RGB = [number, number, number];
type RGBA = [number, number, number, number];

// What a surface is made of, carried in its colour's alpha (255 less the
// code): the shader textures it accordingly, and draws it opaque.
const PLAIN = 0;
const GRASS = 1;
const FOREST = 2;
const FIELD = 3;
const SAND = 4;
const WATER = 5;
const ASPHALT = 6;
const PAVING = 7;
const GRAVEL = 8;
const ROCK = 9;
const SNOW = 10;
const MARSH = 11;

const paint = ([r, g, b]: RGB, material: number): RGBA => [r, g, b, 255 - material];
const NONE: RGBA = [0, 0, 0, 0];

/** The ground where nothing else is mapped: grass, as in Streets GL. */
export const LAND = paint([134, 154, 98], GRASS);
/** The globe itself, showing only past the map's edge at the poles: ice. */
export const POLES: RGB = [226, 234, 240];

const WATER_PAINT = paint([52, 96, 132], WATER);
const AREAS: Record<string, RGBA> = {
  ocean: paint([42, 84, 120], WATER),
  water: WATER_PAINT,
  pool: paint([88, 164, 192], WATER),
  wood: paint([66, 92, 54], FOREST),
  scrub: paint([102, 120, 70], FOREST),
  grass: paint([120, 154, 82], GRASS),
  park: paint([112, 152, 78], GRASS),
  meadow: paint([140, 158, 90], GRASS),
  golf: paint([108, 162, 78], GRASS),
  heath: paint([126, 128, 86], GRASS),
  farmland: paint([184, 172, 122], FIELD),
  orchard: paint([126, 146, 82], GRASS),
  allotments: paint([128, 146, 90], GRASS),
  wetland: paint([100, 124, 92], MARSH),
  sand: paint([220, 204, 158], SAND),
  rock: paint([156, 152, 144], ROCK),
  ice: paint([234, 240, 246], SNOW),
  // towns: gardens and lawns between the houses, paving where there is trade and industry
  residential: paint([146, 156, 116], GRASS),
  institutional: paint([156, 160, 132], GRASS),
  commercial: paint([176, 172, 164], PAVING),
  industrial: paint([166, 162, 154], PAVING),
  railway: paint([148, 138, 124], GRAVEL),
  cemetery: paint([124, 150, 94], GRASS),
  pitch: paint([100, 154, 78], GRASS),
  athletics: paint([172, 96, 74], PAVING),
  playground: paint([198, 184, 144], SAND),
  quarry: paint([174, 164, 148], ROCK),
  // airports: grass around concrete
  aerodrome: paint([142, 160, 108], GRASS),
  apron: paint([158, 158, 156], PAVING),
  runway: paint([84, 84, 88], ASPHALT),
  taxiway: paint([102, 102, 104], ASPHALT),
  // squares, platforms, piers and bridges mapped as areas
  square: paint([194, 188, 176], PAVING),
  platform: paint([182, 178, 170], PAVING),
  pier: paint([154, 130, 102], PLAIN),
  bridge: paint([148, 148, 146], PAVING),
};

type Properties = Record<string, unknown>;
type Feature = { properties: Properties };

/** An area's paint, or none (left out). */
export function areaPaint({ properties: p }: Feature): RGBA {
  const [cls, sub] = [String(p.class ?? ""), String(p.subclass ?? "")];
  switch (p.layerName) {
    case "water":
      return cls === "ocean" ? AREAS.ocean : cls === "swimming_pool" ? AREAS.pool : WATER_PAINT;
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
    default:
      return NONE;
  }
}

/** A line's surface: its full width in metres and paint, and its edging either side (pavements, ballast) and paint. */
type Line = { width: number; paint: RGBA; edge: number; edgePaint: RGBA };
const asphalt = (shade: number): RGBA => paint([shade, shade + 2, shade + 6], ASPHALT);
const PAVEMENT = paint([174, 170, 162], PAVING);
const SHOULDER = paint([148, 148, 146], PAVING);
const BALLAST = paint([140, 130, 116], GRAVEL);
const road = (width: number, shade: number, edge = 0, edgePaint = PAVEMENT): Line => ({ width, paint: asphalt(shade), edge, edgePaint });
const ROADS: Record<string, Line> = {
  motorway: road(11, 76, 1.5, SHOULDER),
  trunk: road(10, 78, 1.5, SHOULDER),
  primary: road(9, 82, 2),
  secondary: road(8, 86, 2),
  tertiary: road(7, 90, 1.8),
  minor: road(6, 96, 1.6),
  busway: road(7, 88, 1.8),
  service: road(4, 106),
  raceway: road(10, 78),
  track: { width: 3, paint: paint([150, 134, 104], GRAVEL), edge: 0, edgePaint: NONE },
  // the track itself, dark with sleepers, on its ballast
  rail: { width: 2.4, paint: paint([92, 84, 76], GRAVEL), edge: 1, edgePaint: BALLAST },
  transit: { width: 2, paint: paint([98, 92, 86], GRAVEL), edge: 0.6, edgePaint: BALLAST },
  pier: { width: 3, paint: AREAS.pier, edge: 0, edgePaint: NONE },
};
const PATHS: Record<string, Line> = {
  footway: { width: 2, paint: paint([192, 186, 174], PAVING), edge: 0, edgePaint: NONE },
  cycleway: { width: 2.5, paint: asphalt(112), edge: 0, edgePaint: NONE },
  pedestrian: { width: 5, paint: paint([194, 188, 176], PAVING), edge: 0, edgePaint: NONE },
  path: { width: 1.6, paint: paint([176, 162, 132], GRAVEL), edge: 0, edgePaint: NONE },
};
const WATERWAYS: Record<string, number> = { river: 18, canal: 12, stream: 3, drain: 1.5, ditch: 1.5 };
const AEROWAYS: Record<string, Line> = {
  runway: { width: 45, paint: AREAS.runway, edge: 0, edgePaint: NONE },
  taxiway: { width: 18, paint: AREAS.taxiway, edge: 0, edgePaint: NONE },
};
const NO_LINE: Line = { width: 0, paint: NONE, edge: 0, edgePaint: NONE };

/** How a line is drawn on the ground: a road, railway, path, river or runway (tunnels and ferries not at all). */
export function lineOf({ properties: p }: Feature): Line {
  const [cls, sub] = [String(p.class ?? ""), String(p.subclass ?? "")];
  switch (p.layerName) {
    case "transportation":
      if (p.brunnel === "tunnel" || cls === "ferry" || cls.endsWith("_construction")) return NO_LINE;
      if (cls === "path") return PATHS[sub] ?? PATHS[sub === "steps" || sub === "platform" ? "footway" : "path"];
      return ROADS[cls] ?? NO_LINE;
    case "waterway": {
      if (p.brunnel === "tunnel") return NO_LINE;
      return { width: WATERWAYS[cls] ?? 2, paint: WATER_PAINT, edge: 0, edgePaint: NONE };
    }
    case "aeroway":
      return AEROWAYS[cls] ?? NO_LINE;
    default:
      return NO_LINE;
  }
}

const linePaint = (f: Feature) => lineOf(f).paint;
const lineWidth = (f: Feature) => lineOf(f).width;
const edgePaint = (f: Feature) => lineOf(f).edgePaint;
const edgeWidth = (f: Feature) => {
  const line = lineOf(f);
  return line.edge ? line.width + 2 * line.edge : 0;
};

// --------------------------------------------------------------- texture

/** The textures repeat every this many common units (1/64 of a 512-unit world: about 600 m at 60°N). */
const PERIOD = 1 / 64;

const groundBlock = /* glsl */ `\
layout(std140) uniform groundUniforms {
  vec2 origin;
  float period;
  float planar;
  vec3 sun;
  float time;
  vec3 sky;
  float night;
} ground;
`;

const groundVs = `${groundBlock}out vec2 ground_t;\nout vec3 ground_view;\n`;

const groundFs = /* glsl */ `\
${groundBlock}
in vec2 ground_t;
in vec3 ground_view;

// value noise on a lattice \`n\` cells to the period, repeating with it (so with the reduced origin)
float ground_hash(vec2 cell, float n) {
  cell = mod(cell, n);
  return fract(sin(dot(cell, vec2(127.1, 311.7))) * 43758.5453);
}
float ground_noise(vec2 t, float n) {
  vec2 p = t * n;
  vec2 i = floor(p);
  vec2 f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(ground_hash(i, n), ground_hash(i + vec2(1.0, 0.0), n), f.x),
             mix(ground_hash(i + vec2(0.0, 1.0), n), ground_hash(i + vec2(1.0, 1.0), n), f.x), f.y);
}
// how much of an octave of \`n\` cells to the period shows: none once a cell is under a couple of pixels
float ground_shows(float n) {
  vec2 w = fwidth(ground_t * n);
  return 1.0 - smoothstep(0.3, 0.7, max(w.x, w.y));
}
// noise at \`n\`, centred on 0, faded out where too fine
float ground_octave(float n) {
  return (ground_noise(ground_t, n) - 0.5) * ground_shows(n);
}
`;

const groundInject = {
  "vs:DECKGL_FILTER_GL_POSITION": /* glsl */ `
    ground_t = (ground.origin + geometry.position.xy) / ground.period;
    ground_view = project.cameraPosition - geometry.position.xyz;
  `,
  "fs:DECKGL_FILTER_COLOR": /* glsl */ `
  {
    float material = floor((1.0 - color.a) * 255.0 + 0.5);
    if (material < 100.0) {
      color.a = 1.0;
      vec3 c = color.rgb;
      // about 10 m, 2.5 m and 60 cm across, with the period about 600 m
      float big = ground_octave(64.0);
      float mid = ground_octave(256.0);
      float fine = ground_octave(1024.0);
      if (material == ${GRASS}.0) {
        // mottled, with drier patches
        c *= 1.0 + 0.22 * big + 0.14 * mid + 0.1 * fine;
        c = mix(c, c * vec3(1.12, 1.06, 0.78), smoothstep(0.1, 0.4, ground_octave(16.0)));
      } else if (material == ${FOREST}.0) {
        c *= 1.0 + 0.3 * big + 0.22 * mid;
      } else if (material == ${FIELD}.0) {
        // field to field the crop differs: greener, browner, paler
        float field = ground_octave(8.0);
        c = mix(c, c * vec3(0.78, 0.92, 0.7), smoothstep(-0.1, 0.25, field));
        c = mix(c, c * vec3(0.92, 0.8, 0.68), smoothstep(-0.1, -0.35, field));
        c *= 1.0 + 0.12 * mid + 0.1 * fine;
      } else if (material == ${SAND}.0 || material == ${SNOW}.0) {
        c *= 1.0 + 0.08 * big + 0.06 * fine;
      } else if (material == ${ASPHALT}.0) {
        c *= 1.0 + 0.1 * mid + 0.14 * fine + 0.12 * ground_octave(4096.0);
      } else if (material == ${PAVING}.0) {
        c *= 1.0 + 0.08 * big + 0.1 * fine + 0.1 * ground_octave(4096.0);
      } else if (material == ${GRAVEL}.0 || material == ${ROCK}.0) {
        c *= 1.0 + 0.16 * mid + 0.22 * fine + 0.18 * ground_octave(4096.0);
      } else if (material == ${MARSH}.0) {
        c *= 1.0 + 0.25 * big + 0.15 * mid;
        c = mix(c, vec3(0.2, 0.33, 0.42), 0.3 * smoothstep(0.15, 0.35, ground_octave(128.0)));
      } else if (material == ${WATER}.0) {
        vec3 v = normalize(ground_view);
        vec3 n = vec3(0.0, 0.0, 1.0);
        if (ground.planar > 0.5) {
          // ripples drifting with the wind: the slope of two octaves of noise
          vec2 drift = vec2(ground.time * 0.004, ground.time * 0.0025);
          float e = 0.002;
          vec2 t1 = ground_t + drift;
          vec2 t2 = ground_t * 1.7 - drift.yx;
          float h0 = ground_noise(t1, 512.0) + 0.5 * ground_noise(t2, 1024.0);
          float hx = ground_noise(t1 + vec2(e, 0.0), 512.0) + 0.5 * ground_noise(t2 + vec2(e * 1.7, 0.0), 1024.0);
          float hy = ground_noise(t1 + vec2(0.0, e), 512.0) + 0.5 * ground_noise(t2 + vec2(0.0, e * 1.7), 1024.0);
          float shows = ground_shows(512.0);
          n = normalize(vec3((h0 - hx) * 0.6 * shows, (h0 - hy) * 0.6 * shows, 1.0));
        }
        // the sky in it, more at a glancing angle, and the sun's glitter
        float fresnel = 0.04 + 0.96 * pow(1.0 - max(dot(n, v), 0.0), 5.0);
        // on the globe, "up" isn't the z axis: less of the sky, as the angle there is only roughly right
        c = mix(c, ground.sky, min(1.0, fresnel * 0.9) * mix(0.3, 1.0, ground.planar));
        float glint = pow(max(dot(reflect(-v, n), ground.sun), 0.0), 220.0) * smoothstep(0.0, 0.08, ground.sun.z) * ground.planar;
        c += vec3(1.0, 0.92, 0.78) * glint * 1.6;
      } else {
        c *= 1.0 + 0.08 * big + 0.06 * fine;
      }
      // at night, close in: dark, a little blue
      color.rgb = c * mix(vec3(1.0), vec3(0.16, 0.19, 0.27), ground.night);
    } else {
      color.a = 0.0;
    }
  }
  `,
};

const groundModule = {
  name: "ground",
  vs: groundVs,
  fs: groundFs,
  uniformTypes: { origin: "vec2<f32>", period: "f32", planar: "f32", sun: "vec3<f32>", time: "f32", sky: "vec3<f32>", night: "f32" },
} as const;

/** The surfaces' textures, by the material their colour carries (see `paint`). */
export class GroundExtension extends LayerExtension {
  static extensionName = "GroundExtension";

  getShaders() {
    return { modules: [groundModule], inject: groundInject };
  }

  draw(this: Layer, params: { shaderModuleProps: { project: object } }) {
    const projectProps = params.shaderModuleProps.project as { viewport: { resolution?: number } };
    // the layer's origin in common space, cut down to within one period in float64 (as deck.gl's fill patterns do)
    const { commonOrigin } = project.getUniforms(projectProps as never) as { commonOrigin: number[] };
    this.setShaderModuleProps({
      ground: {
        origin: [commonOrigin[0] % PERIOD, commonOrigin[1] % PERIOD],
        period: PERIOD,
        // the flat projection's common space is east, north and up, which the water's light needs
        planar: projectProps.viewport.resolution ? 0 : 1,
        sun: look.sun,
        time: (performance.now() / 1000) % 10_000,
        sky: look.sky,
        night: look.groundNight,
      },
    });
  }
}

export const GROUND = new GroundExtension();

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

/**
 * One tile of ground: a GeoJsonLayer to MVTLayer (which expects one), drawn
 * by the three passes rather than by GeoJsonLayer's own state.
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
    const { areas, lines } = Array.isArray(data) ? splitFeatures(data) : split(data);
    return [
      new GeoJsonLayer({ ...own, id: `${id}-areas`, data: areas, filled: true, stroked: false } as never),
      new GeoJsonLayer({ ...own, id: `${id}-edges`, data: lines, filled: false, getLineColor: edgePaint, getLineWidth: edgeWidth } as never),
      new GeoJsonLayer({ ...own, id: `${id}-lines`, data: lines, filled: false } as never),
    ];
  }
}

const renderGround = (props: object) => new GroundTile(props as never);

const GROUND_LAYERS = ["landuse", "landcover", "aeroway", "water", "waterway", "transportation"];
const loadOptions = new Map<string, object>();

/** The ground's vector tiles; MVTLayer decodes per projection, so each gets its own layer (and cache). */
export const groundLayer = (url: string, flat: boolean, workerUrl: string, extensions: unknown[], parameters: object) => {
  let options = loadOptions.get(workerUrl);
  if (!options) loadOptions.set(workerUrl, (options = { mvt: { layers: GROUND_LAYERS, workerUrl } }));
  return new MVTLayer({
    id: flat ? "ground-flat" : "ground-globe",
    data: url,
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
