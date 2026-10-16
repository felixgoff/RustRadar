// The realistic basemap's looks, in the spirit of Streets GL: buildings with
// façades (storeys and bays of windows fitted to each wall, shopfronts, glass
// towers, lit windows at night) and roofs of their own, the distance fading
// into a haze the colour of the sky, and the sky itself behind the map.
//
// The façades and the haze are deck.gl layer extensions: shader code injected
// into the layers' own shaders. Their settings change with the view and the
// sun every frame, so they are read from `look` (set by the map before each
// frame) rather than passed as props: changing a prop would rebuild every tile.

import { LayerExtension } from "@deck.gl/core";

/** What the extensions draw with this frame: the map sets it before drawing. */
export const look = {
  /** The haze's colour (0–1), where it starts (metres from the camera) and how fast it thickens beyond (per metre). */
  haze: [0.84, 0.88, 0.92] as [number, number, number],
  hazeStart: 1500,
  hazeDensity: 1 / 9000,
  /** The sky the windows reflect (0–1). */
  sky: [0.55, 0.68, 0.82] as [number, number, number],
  /** 0 by day to 1 at night: walls darken and some windows light up. */
  night: 0,
  /** 0 by day to 1 at night for the ground: only close in, where the view is all one place's night. */
  groundNight: 0,
  /** Towards the sun, in the flat projection's east, north and up (for the water's glitter). */
  sun: [0.4, -0.55, 0.73] as [number, number, number],
};

// --------------------------------------------------------------- façades

const facadeBlock = /* glsl */ `\
layout(std140) uniform facadeUniforms {
  vec3 sky;
  float night;
} facade;
`;

const facadeVaryings = (dir: "in" | "out") => /* glsl */ `\
${dir} vec3 facade_normal;
${dir} vec3 facade_view;
${dir} vec3 facade_light;
${dir} vec2 facade_uv;
flat ${dir} vec3 facade_wall;
flat ${dir} vec3 facade_size;
`;

const facadeVs = `${facadeBlock}${facadeVaryings("out")}`;

const facadeFs = /* glsl */ `\
${facadeBlock}${facadeVaryings("in")}
float facade_hash(vec2 p) {
  return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}
float facade_noise(vec2 p) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(facade_hash(i), facade_hash(i + vec2(1.0, 0.0)), f.x),
             mix(facade_hash(i + vec2(0.0, 1.0)), facade_hash(i + vec2(1.0, 1.0)), f.x), f.y);
}
// 1 inside [lo, hi], 0 outside, the edges \`w\` wide (a pixel's worth)
float facade_box(float x, float lo, float hi, float w) {
  return smoothstep(lo - w, lo + w, x) * (1.0 - smoothstep(hi - w, hi + w, x));
}
`;

const facadeInject = {
  // At the end of main, where the layer's attributes can be read (a hook
  // function can't see them): each wall's length and height, and how far
  // along and up it this vertex is, in metres.
  "vs:#main-end": /* glsl */ `
    facade_wall = fillColors.rgb;
    // how the light brightens or dims the building's own colour here, to light the façade's other colours alike
    facade_light = vColor.rgb / max(fillColors.rgb, vec3(0.02));
    facade_normal = geometry.normal;
    facade_view = project.cameraPosition - geometry.position.xyz;
  #ifdef IS_SIDE_VERTEX
    {
      vec4 facade_a = project_position(vec4(vertexPositions, 1.0), vertexPositions64Low);
      vec4 facade_b = project_position(vec4(nextVertexPositions, 1.0), nextVertexPositions64Low);
      float facade_length = length((facade_b.xy - facade_a.xy) / project.commonUnitsPerMeter.xy);
      float facade_height = elevations * solidPolygon.elevationScale;
      facade_uv = vec2(positions.x * facade_length, positions.y * facade_height);
      facade_size = vec3(facade_length, facade_height, 1.0);
    }
  #else
    // a roof: metres across the tile (vector tiles' own coordinates, scaled by the tile's model matrix)
    facade_uv = geometry.worldPosition.xy * vec2(project.modelMatrix[0][0], project.modelMatrix[1][1]) / project.commonUnitsPerMeter.xy;
    facade_size = vec3(0.0, elevations * solidPolygon.elevationScale, 0.0);
  #endif
  `,
  "fs:DECKGL_FILTER_COLOR": /* glsl */ `
  {
    vec3 n = normalize(facade_normal);
    vec3 toEye = normalize(facade_view);
    // the building's own choices, the same all round it: from its colour
    float which = fract(dot(facade_wall, vec3(71.23, 43.17, 36.19)));
    float night = facade.night;
    float wallLength = facade_size.x;
    float height = facade_size.y;
    if (facade_size.z < 0.5) {
      // a roof: bitumen, red tiles, slate or gravel, with a little grain
      vec3 roof = which < 0.34 ? vec3(0.34, 0.34, 0.35)
        : which < 0.58 ? vec3(0.50, 0.26, 0.19)
        : which < 0.8 ? vec3(0.23, 0.24, 0.27)
        : vec3(0.53, 0.51, 0.47);
      float grain = 0.9 + 0.1 * facade_noise(facade_uv * 0.7) + 0.06 * (facade_hash(floor(facade_uv * 1.7)) - 0.5);
      color.rgb = roof * grain * facade_light * mix(1.0, 0.35, night);
    } else {
      float u = facade_uv.x;
      float z = facade_uv.y;
      // towers in glass; the rest walls with windows punched in
      float glassy = step(30.0, height) * step(0.45, which);
      // whole storeys fitted to the building, under a parapet
      float parapet = 0.6;
      float floors = max(1.0, floor((height - parapet) / mix(3.1, 3.6, glassy) + 0.5));
      float storey = (height - parapet) / floors;
      // whole bays fitted to the wall, so its windows sit square on it
      float bays = floor(wallLength / mix(3.2, 1.6, glassy) + 0.5);
      float fu = u / max(wallLength, 0.01) * max(bays, 1.0);
      float fz = z / storey;
      float su = fract(fu);
      float sv = fract(fz);
      vec2 cell = vec2(floor(fu), floor(fz));
      // derivatives in uniform control flow: a pixel's worth of bay and of storey
      float wu = max(fwidth(fu), 1e-4);
      float wv = max(fwidth(fz), 1e-4);
      // walls long and tall enough for windows, below the parapet
      float windowed = step(0.5, bays) * step(3.6, height) * step(z, height - parapet);
      // punched windows, a little narrower or wider by building; shopfronts on the ground floor of taller ones
      float side = 0.2 + 0.12 * fract(which * 7.3);
      float punched = facade_box(su, side, 1.0 - side, wu) * facade_box(sv, 0.3, 0.8, wv);
      float frame = facade_box(su, side - 0.05, 1.05 - side, wu) * facade_box(sv, 0.25, 0.85, wv) - punched;
      float shops = step(cell.y, 0.5) * step(9.0, height) * step(0.4, which) * (1.0 - glassy);
      float shopfront = facade_box(su, 0.05, 0.95, wu) * facade_box(sv, 0.12, 0.84, wv);
      float curtain = facade_box(su, 0.04, 0.96, wu) * facade_box(sv, 0.26, 0.97, wv);
      float glass = mix(mix(punched, shopfront, shops), curtain, glassy) * windowed;
      frame *= (1.0 - shops) * (1.0 - glassy) * windowed;
      // a line at each floor, and the parapet's coping
      float ledge = 1.0 - 0.1 * facade_box(sv, -0.02, 0.03, wv) * windowed;
      float coping = 1.0 - 0.12 * step(height - 0.25, z);
      // glass: dark, with the sky in it (more of it at a glancing angle); at night some lit, warm
      float glance = pow(1.0 - abs(dot(n, toEye)), 3.0);
      vec3 pane = vec3(0.07, 0.09, 0.12) * facade_light + facade.sky * (0.16 + 0.5 * glance) * (1.0 - 0.85 * night);
      pane = mix(pane, pane * vec3(0.8, 1.0, 1.05) + vec3(0.0, 0.03, 0.04), glassy * step(0.7, which));
      float lit = step(0.6, facade_hash(cell * vec2(1.37, 2.71) + facade_wall.xy * 31.0)) * night;
      vec3 lamp = mix(vec3(1.0, 0.76, 0.46), vec3(0.86, 0.92, 1.0), step(0.86, facade_hash(cell + facade_wall.yz * 17.0)));
      vec3 wall = color.rgb * mix(1.0, 0.3, night) * coping;
      vec3 near = mix(mix(wall * ledge, wall * 1.14 + 0.03, frame), mix(pane, lamp, lit), glass);
      // finer than a pixel or two, the pattern averages out: no shimmer
      vec3 paneAverage = mix(pane, lamp, 0.36 * night);
      vec3 far = mix(wall, paneAverage, windowed * mix(mix(0.24, 0.6, shops), 0.72, glassy));
      float detail = 1.0 - smoothstep(0.22, 0.55, max(wu, wv));
      color.rgb = mix(far, near, detail);
      // the ground shades the foot of the wall
      color.rgb *= mix(0.7, 1.0, smoothstep(0.0, 2.5, z));
    }
  }
  `,
};

const facadeModule = {
  name: "facade",
  vs: facadeVs,
  fs: facadeFs,
  uniformTypes: { sky: "vec3<f32>", night: "f32" },
} as const;

/** Façades and roofs for extruded vector-tile polygons (buildings): an extension for their SolidPolygonLayer. */
export class FacadeExtension extends LayerExtension {
  static extensionName = "FacadeExtension";

  isEnabled(layer: { getAttributeManager(): { attributes: object } | null; props: { extruded?: boolean } }): boolean {
    return Boolean(layer.props.extruded) && "fillColors" in (layer.getAttributeManager()?.attributes ?? {});
  }

  getShaders(this: never, extension: FacadeExtension) {
    if (!extension.isEnabled(this)) return null;
    return { modules: [facadeModule], inject: facadeInject };
  }

  draw(this: { setShaderModuleProps(props: object): void }, _params: unknown, extension: FacadeExtension) {
    if (!extension.isEnabled(this as never)) return;
    this.setShaderModuleProps({ facade: { sky: look.sky, night: look.night } });
  }
}

// --------------------------------------------------------------- haze

const hazeBlock = /* glsl */ `\
layout(std140) uniform hazeUniforms {
  vec3 color;
  float start;
  float density;
} haze;
`;

// The way to the camera, in metres, is what is interpolated (it is linear
// across a triangle); its length is taken for each pixel. A distance worked
// out at the corners would be far too great inside the big triangles of the
// ground, which would vanish into the haze.
const hazeModule = {
  name: "haze",
  vs: `${hazeBlock}out vec3 haze_offset;\n`,
  fs: `${hazeBlock}in vec3 haze_offset;\nfloat haze_amount() {\n  return 1.0 - exp(-max(length(haze_offset) - haze.start, 0.0) * haze.density);\n}\n`,
  uniformTypes: { color: "vec3<f32>", start: "f32", density: "f32" },
} as const;

const hazeInject = {
  "vs:DECKGL_FILTER_GL_POSITION": /* glsl */ `
    haze_offset = (geometry.position.xyz - project.cameraPosition) / project.commonUnitsPerMeter.x;
  `,
  "fs:DECKGL_FILTER_COLOR": /* glsl */ `
    color.rgb = mix(color.rgb, haze.color, haze_amount());
  `,
};

/** The distance fading into the sky's haze, as `look` says. */
export class HazeExtension extends LayerExtension {
  static extensionName = "HazeExtension";

  getShaders() {
    return { modules: [hazeModule], inject: hazeInject };
  }

  draw(this: { setShaderModuleProps(props: object): void }) {
    this.setShaderModuleProps({ haze: { color: look.haze, start: look.hazeStart, density: look.hazeDensity } });
  }
}

const hazeFadeInject = {
  "vs:DECKGL_FILTER_GL_POSITION": hazeInject["vs:DECKGL_FILTER_GL_POSITION"],
  "fs:DECKGL_FILTER_COLOR": /* glsl */ `
    color.a *= 1.0 - haze_amount();
  `,
};

/** The haze for what lies over the ground (shading, shadows): fading away into it rather than taking its colour. */
export class HazeFadeExtension extends HazeExtension {
  static extensionName = "HazeFadeExtension";

  getShaders() {
    return { modules: [hazeModule], inject: hazeFadeInject };
  }
}

// --------------------------------------------------------------- night

const nightModule = {
  name: "night",
  fs: /* glsl */ `\
layout(std140) uniform nightUniforms {
  float amount;
} night;
`,
  uniformTypes: { amount: "f32" },
} as const;

/** Darkened at night close in, as the ground is: for what has no night of its own (the trees). */
export class NightExtension extends LayerExtension {
  static extensionName = "NightExtension";

  getShaders() {
    return {
      modules: [nightModule],
      inject: {
        "fs:DECKGL_FILTER_COLOR": /* glsl */ `
    color.rgb *= mix(vec3(1.0), vec3(0.16, 0.19, 0.27), night.amount);
  `,
      },
    };
  }

  draw(this: { setShaderModuleProps(props: object): void }) {
    this.setShaderModuleProps({ night: { amount: look.groundNight } });
  }
}

/** One of each, shared by every layer that uses them: the same extension compares equal, so no shader rebuilds. */
export const FACADE = new FacadeExtension();
export const HAZE = new HazeExtension();
export const HAZE_FADE = new HazeFadeExtension();
export const NIGHT = new NightExtension();

/**
 * The haze for a view: clear out to a little past the point the camera looks
 * at, then thickening, so looking down there is none and tilted towards the
 * horizon the far distance fades away. `distance` is the camera's from that
 * point, metres. (Far out, the globe's rim hazes over the same way.)
 */
export function hazeFor(distance: number): { start: number; density: number } {
  return { start: 1.15 * distance, density: 0.55 / distance };
}

// --------------------------------------------------------------- colours

/** Walls, as a city's are: whites and creams most, then greys, ochre, brick and a few pastels. */
const WALLS: [number, number, number, number][] = [
  // colour, weight
  [236, 232, 220, 6],
  [224, 212, 188, 5],
  [210, 208, 202, 4],
  [196, 190, 180, 3],
  [228, 202, 150, 2],
  [176, 94, 70, 2],
  [148, 108, 86, 1],
  [226, 192, 176, 1],
  [190, 202, 198, 1],
];
const WALL_WEIGHT = WALLS.reduce((sum, w) => sum + w[3], 0);

/** The colours OpenStreetMap's `building:colour` most often names, as #rrggbb. */
const NAMED: Record<string, string> = {
  white: "#f2f0ea",
  black: "#3a3a3a",
  grey: "#9a9a9a",
  gray: "#9a9a9a",
  lightgrey: "#c8c8c8",
  lightgray: "#c8c8c8",
  darkgrey: "#6a6a6a",
  darkgray: "#6a6a6a",
  silver: "#c0c0c0",
  red: "#b04a3a",
  maroon: "#7a3028",
  brown: "#8a5a40",
  tan: "#d2b48c",
  beige: "#e6dcc0",
  cream: "#f0e6c8",
  yellow: "#e8d27a",
  gold: "#d8b860",
  orange: "#d88a50",
  pink: "#e0b0b0",
  green: "#8aa080",
  blue: "#8098b0",
};

/**
 * A building's wall colour: the one its OpenStreetMap `colour` gives (toned
 * down: mapped colours are often loud), else one of a city's, picked by `seed`.
 */
export function wallColour(seed: number, colour?: unknown): [number, number, number] {
  const hex = typeof colour === "string" ? (NAMED[colour.toLowerCase()] ?? colour) : null;
  if (hex && /^#[0-9a-f]{6}$/i.test(hex)) {
    const v = parseInt(hex.slice(1), 16);
    const soften = (c: number) => Math.round(c * 0.75 + 200 * 0.25);
    return [soften(v >> 16), soften((v >> 8) & 255), soften(v & 255)];
  }
  let h = Math.imul(seed ^ 0x9e3779b9, 0x85ebca6b) >>> 0;
  h = Math.imul(h ^ (h >>> 13), 0xc2b2ae35) >>> 0;
  h = (h ^ (h >>> 16)) >>> 0;
  let pick = h % WALL_WEIGHT;
  for (const [r, g, b, w] of WALLS) {
    if (pick < w) {
      // a little variety within each
      const j = ((h >>> 8) % 17) - 8;
      return [r + j, g + j, b + j];
    }
    pick -= w;
  }
  return [WALLS[0][0], WALLS[0][1], WALLS[0][2]];
}

/** 0 by day to 1 at night, from the sun's height above the horizon (its sine): windows light up through dusk. */
export function nightness(sunUp: number): number {
  return Math.min(1, Math.max(0, (0.05 - sunUp) / 0.15));
}

const mixRgb = (a: readonly number[], b: readonly number[], t: number) =>
  a.map((v, i) => Math.round(v + (b[i] - v) * t)) as [number, number, number];
/** How much daylight there is, 0 at night to 1 by day, from the sun's height (its sine). */
const daylight = (sunUp: number) => Math.min(1, Math.max(0, (sunUp + 0.08) / 0.28));

/** The sky's colours, 0–255: overhead, and at the horizon (where the haze meets it). */
export function skyColours(sunUp: number): { top: [number, number, number]; horizon: [number, number, number] } {
  const day = daylight(sunUp);
  // at dusk the horizon warms before it darkens
  const dusk = Math.max(0, 1 - Math.abs(sunUp - 0.0) / 0.12) * 0.6;
  const horizon = mixRgb(mixRgb([30, 36, 56], [210, 222, 234], day), [236, 170, 120], dusk * (1 - day * 0.4));
  return { top: mixRgb([6, 10, 24], [72, 128, 196], day), horizon };
}

/**
 * The sky behind the map, as a CSS background: blue overhead, paling to
 * haze at the horizon, which is `horizon` (0 at the top of the screen, 1 at
 * the bottom; above the top when the view looks down steeply). Far out, the
 * globe hangs in space instead: `space`, 0 to 1.
 */
export function skyBackground(sunUp: number, horizon: number, space: number): string {
  const { top, horizon: low } = skyColours(sunUp);
  const night: [number, number, number] = [4, 7, 16];
  const t = mixRgb(top, night, space);
  const h = mixRgb(low, [12, 18, 34], space);
  const at = Math.round(Math.min(1, Math.max(0, horizon)) * 100);
  return `linear-gradient(to bottom, rgb(${t.join(" ")}) 0%, rgb(${h.join(" ")}) ${at}%, rgb(${h.join(" ")}) 100%)`;
}
