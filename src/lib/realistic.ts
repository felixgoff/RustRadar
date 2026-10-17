// The realistic map's light and materials, after Streets GL
// (github.com/StrandedKitty/streets-gl, MIT licence): physically based
// shading in linear light — a sun reddened by the air it comes through, the
// sky's grey light from every side, GGX highlights and reflections of the
// sky — then aerial perspective, the distance fading into the sky's colour
// (the clear air's few percent a kilometre, thick only in fog), and the
// screen's gamma with no tone curve, so the colours stay as Streets GL shows
// them. The weather sets it all: how much sun gets through, how grey the sky
// is, how thick the air, wet or snowy.
//
// The shading is shader code injected into deck.gl's layers by extensions:
// buildings (Streets GL's façade and roof textures: storeys and bays of
// windows, normal maps, glass that reflects the sky, lit at night), trees, and
// the ground's (realistic-ground.ts). What changes from frame to frame (the
// sun, the haze) is read from `look`, which the map sets before each frame:
// as a prop it would rebuild every tile.

import { type Layer, LayerExtension, project } from "@deck.gl/core";
import type { Device } from "@luma.gl/core";
import { realisticTextures } from "./realistic-textures";
import type { Conditions } from "./weather";

type RGB = [number, number, number];
const DEG = Math.PI / 180;
const clamp01 = (x: number) => Math.min(1, Math.max(0, x));
const smooth = (a: number, b: number, x: number) => {
  const t = clamp01((x - a) / (b - a));
  return t * t * (3 - 2 * t);
};
const mix = (a: readonly number[], b: readonly number[], t: number) => a.map((v, i) => v + (b[i] - v) * t) as RGB;
const scale = (a: readonly number[], k: number) => a.map((v) => v * k) as RGB;

// --------------------------------------------------------------- the light

/** The light the realistic map is drawn in this frame (colours linear). */
export type Look = {
  /** Towards the sun: east, north and up at the middle of the view. */
  sunDir: RGB;
  /** The sine of the sun's altitude. */
  sunUp: number;
  /** The sun's light on a surface facing it (0 when clouds or the night hide it). */
  sunColor: RGB;
  /** The sky overhead and at the horizon, and the light thrown up by the ground. */
  skyZenith: RGB;
  skyHorizon: RGB;
  groundColor: RGB;
  /** How strong the sky's light is on surfaces. */
  ambient: number;
  /** The air's colour in the distance, its extinction per metre, and from how far out it starts (fog). */
  hazeColor: RGB;
  hazeDensity: number;
  hazeStart: number;
  wet: number;
  snow: number;
  /** Shadows of drifting clouds, 0 to 1. */
  cloudShadow: number;
  /** 0 by day to 1 at night: windows light up. */
  night: number;
  /** Brightening before the screen's gamma: under cloud a little, at night more. */
  exposure: number;
};

/** The sun's light above the air, as the shading's units go. */
const SUN = 3.2;
/** Optical depth of a clear atmosphere straight up (Rayleigh and aerosol), red, green, blue. */
const TAU: RGB = [0.13, 0.19, 0.33];
const DAY_ZENITH: RGB = [0.07, 0.17, 0.48];
const DAY_HORIZON: RGB = [0.42, 0.55, 0.75];
const DUSK_HORIZON: RGB = [0.75, 0.38, 0.16];
const NIGHT_ZENITH: RGB = [0.0048, 0.008, 0.019];
const NIGHT_HORIZON: RGB = [0.014, 0.018, 0.032];
const OVERCAST_ZENITH: RGB = [0.5, 0.52, 0.56];
const OVERCAST_HORIZON: RGB = [0.62, 0.63, 0.66];
/** Under cloud at night, the towns' light comes back off it. */
const CITY_GLOW: RGB = [0.012, 0.008, 0.005];

/** The share of each colour of the sun's light that comes through the air at `altDeg`, by Kasten and Young's air mass. */
export function transmittance(altDeg: number, haze: number): RGB {
  const alt = Math.max(altDeg, -1);
  const airMass = 1 / (Math.sin(Math.max(alt, 0) * DEG) + 0.50572 * (alt + 6.07995) ** -1.6364);
  // hazier air dims and reddens it more
  const aerosol = Math.min(1.2, haze * 2500);
  return TAU.map((tau) => Math.exp(-(tau + aerosol * 0.4) * (airMass - 1) * 0.9)) as RGB;
}

/** How bright a colour looks (Rec. 709's weights). */
const luminance = (c: readonly number[]) => 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
/** The daylight of a clear day with the sun 40° up, which the exposure is set for (Streets GL's: no adjusting); dimmer scenes are taken brighter. */
const CLEAR_DAYLIGHT = 1.84;
/** The clear air's own haze, per metre: Streets GL's atmosphere at the ground (about 2 % a kilometre). */
const CLEAR_AIR = 2.2e-5;

/**
 * The light in some weather with the sun towards `sun` (east, north, up),
 * the camera `cameraMetres` from what it looks at. As Streets GL has it, the
 * air is clear (a couple of percent a kilometre) whatever the clouds; only
 * fog thickens it, and never so much that the map gets hard to read: at most
 * about a fifth of the way to the middle of the view, more in fog, which
 * only begins half way there.
 */
export function lookFor(c: Conditions, sun: readonly number[], cameraMetres = 1000): Look {
  const sunUp = Math.max(-1, Math.min(1, sun[2]));
  const altDeg = Math.asin(sunUp) / DEG;
  // daylight from 6° below the horizon (civil twilight) to full from 15° up
  const day = smooth(-6, 15, altDeg);
  const sunColor = scale(transmittance(altDeg, c.haze), SUN * c.sun * smooth(-0.8, 3, altDeg));
  // the clear sky, warming at its horizon around sunset
  const dusk = clamp01(1 - Math.abs(altDeg - 1) / 9) * (1 - c.cloud);
  const clearZenith = mix(NIGHT_ZENITH, DAY_ZENITH, day);
  const clearHorizon = mix(mix(NIGHT_HORIZON, DAY_HORIZON, day), scale(DUSK_HORIZON, Math.max(day, 0.15)), dusk * 0.75);
  // an overcast sky: grey, darker in rain, as bright as the day; at night lit a little by the towns below
  const dim = (0.12 + 0.88 * day) * (1 - 0.3 * c.wet) * day;
  const glow = scale(CITY_GLOW, 1 - day);
  const greyZenith = scale(OVERCAST_ZENITH, dim).map((v, i) => v + glow[i] * 0.5) as RGB;
  const greyHorizon = scale(OVERCAST_HORIZON, dim).map((v, i) => v + glow[i]) as RGB;
  const skyZenith = mix(clearZenith, greyZenith, c.cloud);
  const skyHorizon = mix(clearHorizon, greyHorizon, c.cloud);
  const skyLight = mix(skyHorizon, skyZenith, 0.5);
  const night = 1 - smooth(-7, 1, altDeg);
  // Streets GL's light from the sky: grey and the same from every side, a fifth of the sun's
  // on a clear day; under cloud the sky gives what the sun doesn't; at night a little, so the
  // streets are dim but there to see, not black
  const overcast = luminance(mix(OVERCAST_ZENITH, OVERCAST_HORIZON, 0.5)) * (1 - 0.3 * c.wet);
  const ambient = (0.2 + (overcast * 1.1 - 0.2) * c.cloud) * (0.15 + 0.85 * day) + 0.08 * night;
  // the ground throws back about a fifth of what falls on it
  const groundColor = skyLight.map((s, i) => 0.2 * (s + sunColor[i] * Math.max(sunUp, 0))) as RGB;
  // fog is the air itself lit: whiter than a hazy sky
  const fog = clamp01((c.haze - 3e-4) / 2e-3);
  const fogColour = scale([0.7, 0.71, 0.72], Math.max(day, 0.02));
  const hazeColor = mix(skyHorizon, mix(skyLight, fogColour, 0.5), fog);
  const camera = Math.max(cameraMetres, 50);
  // a clear day as Streets GL shows it; a grey one taken a little brighter, as a camera would (more so at night)
  const daylight = luminance(sunColor) * Math.max(sunUp, 0) + ambient;
  const adapt = Math.min(1.8, Math.max(1, Math.sqrt(CLEAR_DAYLIGHT / Math.max(daylight, 1e-3))));
  return {
    sunDir: [sun[0], sun[1], sunUp],
    sunUp,
    sunColor,
    skyZenith,
    skyHorizon,
    groundColor,
    ambient,
    hazeColor,
    hazeDensity: Math.min(Math.min(c.haze, CLEAR_AIR) * (1 - fog) + c.haze * fog, (0.22 + 0.64 * fog) / camera),
    hazeStart: fog * 0.5 * camera,
    wet: c.wet,
    snow: c.snow,
    cloudShadow: c.cloudShadows * 0.85,
    night,
    exposure: adapt * (1 + 2.2 * (1 - day)),
  };
}

/** The light this frame: the map sets it before each frame. */
export const look: Look = lookFor({ sun: 1, cloud: 0.05, haze: 2e-5, wet: 0, snow: 0, cloudShadows: 0 }, [0.4, -0.55, 0.73]);

export function setLook(next: Look) {
  Object.assign(look, next);
}

/** A linear colour as the screen shows it, 0–255, through the same exposure and gamma as the shaders (Streets GL's: no tone curve). */
export function displayRgb(linear: readonly number[], exposure = look.exposure): RGB {
  return linear.map((v) => Math.round(255 * clamp01(v * exposure) ** (1 / 2.2))) as RGB;
}

/**
 * The sky behind the map as a CSS background: `top` the elevation (degrees
 * above the horizon) at the top of the screen, `horizon` where the horizon
 * is down it (0 top, 1 bottom), the sun's glow round `sunAt` (pixels) when
 * it is in view; far out, `space`, the globe hangs in space instead.
 */
export function skyCss(l: Look, top: number, horizon: number, sunAt: [number, number] | null, space: number): string {
  const elevationColour = (deg: number) => mix(l.skyHorizon, l.skyZenith, Math.sqrt(clamp01(Math.sin(deg * DEG))));
  const css = (c: readonly number[]) => `rgb(${c.join(" ")})`;
  const atTop = mix(displayRgb(elevationColour(Math.max(top, 0)), l.exposure), [4, 7, 16], space).map(Math.round);
  const atHorizon = mix(displayRgb(l.hazeColor, l.exposure), [12, 18, 34], space).map(Math.round);
  const at = Math.round(clamp01(horizon) * 100);
  const layers = [`linear-gradient(to bottom, ${css(atTop)} 0%, ${css(atHorizon)} ${at}%, ${css(atHorizon)} 100%)`];
  const sun = Math.max(...l.sunColor);
  if (sunAt && sun > 0.05 && space < 1) {
    const glow = displayRgb(l.sunColor.map((v, i) => v * 0.5 + l.skyHorizon[i]), l.exposure);
    const alpha = Math.min(0.85, sun / SUN) * (1 - space);
    const [x, y] = [Math.round(sunAt[0]), Math.round(sunAt[1])];
    layers.unshift(
      `radial-gradient(circle at ${x}px ${y}px, rgb(255 252 240 / ${alpha.toFixed(2)}) 0px, rgb(255 250 235 / ${alpha.toFixed(2)}) 14px, ` +
        `rgb(${glow.join(" ")} / ${(alpha * 0.55).toFixed(2)}) 40px, rgb(${glow.join(" ")} / 0) 320px)`,
    );
  }
  return layers.join(", ");
}

/**
 * The sun the realistic map is lit by, as [east, north, up] at the view: the
 * real one with day and night on; otherwise always a fine afternoon, the sun
 * in the south-west 35° up.
 */
export function sceneSun(daylight: boolean, realSun: readonly number[]): RGB {
  if (daylight) return [realSun[0], realSun[1], realSun[2]];
  const [az, alt] = [215 * DEG, 35 * DEG];
  return [Math.sin(az) * Math.cos(alt), Math.cos(az) * Math.cos(alt), Math.sin(alt)];
}

/** `local` (east, north, up at `lat`, `lon`) as a direction from the Earth's centre, as `sunDirection` gives the sun's. */
export function fromLocal(local: readonly number[], lat: number, lon: number): RGB {
  const [sinLat, cosLat, sinLon, cosLon] = [Math.sin(lat * DEG), Math.cos(lat * DEG), Math.sin(lon * DEG), Math.cos(lon * DEG)];
  const [e, n, u] = local;
  return [
    -sinLon * e - sinLat * cosLon * n + cosLat * cosLon * u,
    cosLon * e - sinLat * sinLon * n + cosLat * sinLon * u,
    cosLat * n + sinLat * u,
  ];
}

// --------------------------------------------------------------- shading

/** Textures repeat every this many common units (1/64 of the 512-unit world: about 600 m at 60°N)... */
const PERIOD = 1 / 64;
/** ...and the clouds' shadows every this many. */
const CLOUD_PERIOD = 1 / 4;

const rlightBlock = /* glsl */ `\
layout(std140) uniform rlightUniforms {
  vec3 sunDir;
  float sunUp;
  vec3 sunColor;
  float ambient;
  vec3 skyZenith;
  float exposure;
  vec3 skyHorizon;
  float hazeDensity;
  vec3 hazeColor;
  float wet;
  vec3 groundColor;
  float snow;
  vec3 sunGlobe;
  float hazeStart;
  vec2 origin;
  float period;
  float periodMetres;
  vec2 cloudOrigin;
  float cloudPeriod;
  float cloudShadow;
  vec2 anchor;
  float night;
  float time;
} rlight;
`;

const rlightVaryings = (dir: "in" | "out") => /* glsl */ `\
${dir} vec3 rl_eye;
${dir} vec3 rl_sun;
${dir} vec2 rl_t;
${dir} vec2 rl_c;
`;

const rlightVs = /* glsl */ `\
${rlightBlock}
${rlightVaryings("out")}
// east, north and up at a point of deck.gl's globe (its common space there);
// the flat map's common space is east, north and up already
mat3 rl_frame(vec3 p, bool globe) {
  if (!globe) return mat3(1.0);
  vec3 up = normalize(p);
  vec3 east = normalize(vec3(-up.y, up.x, 0.0) + vec3(1e-9, 0.0, 0.0));
  return mat3(east, cross(up, east), up);
}
`;

const rlightFs = /* glsl */ `\
${rlightBlock}
${rlightVaryings("in")}

#define RL_PI 3.14159265

vec3 rl_linear(vec3 c) {
  return pow(max(c, vec3(0.0)), vec3(2.2));
}
// the screen's gamma, as Streets GL shows its colours: no tone curve to dull them
vec3 rl_display(vec3 c) {
  return pow(clamp(c * rlight.exposure, 0.0, 1.0), vec3(1.0 / 2.2));
}
// value noise on a lattice \`n\` cells to the period, repeating with it
float rl_hash(vec2 cell, float n) {
  cell = mod(cell, n);
  return fract(sin(dot(cell, vec2(127.1, 311.7))) * 43758.5453);
}
float rl_noise(vec2 t, float n) {
  vec2 p = t * n;
  vec2 i = floor(p);
  vec2 f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(rl_hash(i, n), rl_hash(i + vec2(1.0, 0.0), n), f.x),
             mix(rl_hash(i + vec2(0.0, 1.0), n), rl_hash(i + vec2(1.0, 1.0), n), f.x), f.y);
}
// how many cells of a lattice in texture coordinates \`t\` one pixel spans (more than one: too fine to show)
float rl_cells(vec2 t) {
  vec2 d = fwidth(t);
  return max(d.x, d.y);
}
// towards the sun from here
vec3 rl_toSun() {
  return normalize(rl_sun);
}
// the sun's light here: none where the globe has turned away from it
vec3 rl_sunLight() {
  return rlight.sunColor * smoothstep(-0.02, 0.02, rl_sun.z);
}
// the sky's light from a direction, with the sun's glow round it; below the horizon, the ground's
vec3 rl_sky(vec3 dir) {
  float up = clamp(dir.z, -1.0, 1.0);
  vec3 sky = mix(rlight.skyHorizon, rlight.skyZenith, sqrt(max(up, 0.0)));
  float toSun = max(dot(dir, rl_toSun()), 0.0);
  sky += rl_sunLight() * (0.04 * pow(toSun, 6.0) + 0.5 * pow(toSun, 900.0));
  return mix(sky, rlight.groundColor, smoothstep(0.0, -0.2, up));
}
// all the light a surface facing \`n\` gets from the sky: grey, as Streets GL has it (a little less facing down)
vec3 rl_irradiance(vec3 n) {
  return vec3(rlight.ambient) * mix(0.6, 1.0, clamp(0.5 + 0.5 * n.z, 0.0, 1.0));
}
// the split-sum environment BRDF (Karis's fit for mobile, as Streets GL has it)
vec2 rl_envBRDF(float NdotV, float roughness) {
  const vec4 c0 = vec4(-1.0, -0.0275, -0.572, 0.022);
  const vec4 c1 = vec4(1.0, 0.0425, 1.04, -0.04);
  vec4 r = roughness * c0 + c1;
  float a004 = min(r.x * r.x, exp2(-9.28 * NdotV)) * r.x + r.y;
  return vec2(-1.04, 1.04) * a004 + r.zw;
}
// how much of the sun's own reflection a surface shows (less where it is broken up too finely to draw)
float rl_glint = 1.0;
// a surface lit by the sun (where \`sunVisible\`) and the sky: GGX, Smith-Schlick, Schlick's Fresnel
vec3 rl_shade(vec3 albedo, vec3 n, vec3 v, float roughness, float metalness, float ao, float sunVisible) {
  vec3 l = rl_toSun();
  roughness = clamp(roughness, 0.04, 1.0);
  float NdotL = max(dot(n, l), 0.0);
  float NdotV = max(dot(n, v), 1e-3);
  vec3 h = normalize(l + v);
  float NdotH = max(dot(n, h), 0.0);
  float VdotH = max(dot(v, h), 0.0);
  vec3 f0 = mix(vec3(0.04), albedo, metalness);
  vec3 F = f0 + (1.0 - f0) * pow(1.0 - VdotH, 5.0);
  float a = roughness * roughness;
  float a2 = a * a;
  float d = NdotH * NdotH * (a2 - 1.0) + 1.0;
  float D = a2 / (RL_PI * d * d);
  float k = a * 0.5;
  float vis = 0.25 / max((NdotL * (1.0 - k) + k) * (NdotV * (1.0 - k) + k), 1e-4);
  vec3 diffuse = albedo * (1.0 - metalness);
  vec3 direct = ((1.0 - F) * diffuse + F * min(D * vis * RL_PI, 60.0) * rl_glint) * rl_sunLight() * NdotL * sunVisible;
  vec2 brdf = rl_envBRDF(NdotV, roughness);
  vec3 r = reflect(-v, n);
  vec3 env = mix(rl_sky(r), rl_irradiance(r) / max(rlight.ambient, 1e-3), roughness);
  vec3 specular = env * (f0 * brdf.x + brdf.y) * (1.0 - 0.5 * roughness);
  return direct + (diffuse * rl_irradiance(n) + specular) * ao;
}
// how much of what is here gets through the air to the camera (fog only from some way out, so the map stays readable)
float rl_clear() {
  return exp(-max(length(rl_eye) - rlight.hazeStart, 0.0) * rlight.hazeDensity);
}
// the air between here and the camera: dimming and adding its own light (brighter towards the sun)
vec3 rl_aerial(vec3 c) {
  float t = rl_clear();
  vec3 toHere = -rl_eye / max(length(rl_eye), 1e-3);
  float forward = pow(max(dot(toHere, rl_toSun()), 0.0), 8.0);
  vec3 air = rlight.hazeColor + rl_sunLight() * 0.06 * forward;
  return c * t + air * (1.0 - t);
}
// how much of the sun the clouds let through here, drifting with the wind
float rl_sunVisible() {
  if (rlight.cloudShadow <= 0.0) return 1.0;
  vec2 c = rl_c + vec2(rlight.time * 0.0006, rlight.time * 0.00025);
  float n = 0.55 * rl_noise(c, 4.0) + 0.3 * rl_noise(c, 9.0) + 0.15 * rl_noise(c, 23.0);
  return 1.0 - rlight.cloudShadow * smoothstep(0.46, 0.6, n);
}
`;

/** Web mercator's units round the world (deck.gl's), per degree of longitude. */
const MERCATOR_PER_DEGREE = 512 / 360;
/** ...and per metre at the equator. */
const MERCATOR_PER_METRE = 512 / 40_075_016.686;

const rlightModule = {
  name: "rlight",
  vs: rlightVs,
  fs: rlightFs,
  inject: {
    "vs:DECKGL_FILTER_GL_POSITION": /* glsl */ `
  {
    bool rl_globe = project.projectionMode == PROJECTION_MODE_GLOBE;
    mat3 rl_enu = rl_frame(geometry.position.xyz, rl_globe);
    // to the camera and to the sun from here, as metres east, north and up
    rl_eye = ((project.cameraPosition - geometry.position.xyz) / project.commonUnitsPerMeter.x) * rl_enu;
    rl_sun = rl_globe ? rlight.sunGlobe * rl_enu : rlight.sunDir;
    // where it is on the flat map, for what lies fixed to the world (textures, the clouds' shadows):
    // on the globe, web mercator's units from the view's centre, so they fit the flat map's
    vec2 rl_m = geometry.position.xy;
    if (rl_globe) {
      vec2 rl_deg = geometry.worldPosition.xy - rlight.anchor;
      float rl_cos = max(cos(radians(rlight.anchor.y + 0.5 * rl_deg.y)), 0.01);
      rl_m = vec2(rl_deg.x, rl_deg.y / rl_cos) * ${MERCATOR_PER_DEGREE.toFixed(10)};
    #ifdef RL_LINE
      // out across a line's width from its middle
      vec3 rl_side = ((geometry.position.xyz - project_position(geometry.worldPosition.xyz)) / project.commonUnitsPerMeter.x) * rl_enu;
      rl_m += rl_side.xy * (${MERCATOR_PER_METRE.toExponential(10)} / rl_cos);
    #endif
    }
    rl_t = (rlight.origin + rl_m) / rlight.period;
    rl_c = (rlight.cloudOrigin + rl_m) / rlight.cloudPeriod;
  }
    `,
  },
  uniformTypes: {
    sunDir: "vec3<f32>",
    sunUp: "f32",
    sunColor: "vec3<f32>",
    ambient: "f32",
    skyZenith: "vec3<f32>",
    exposure: "f32",
    skyHorizon: "vec3<f32>",
    hazeDensity: "f32",
    hazeColor: "vec3<f32>",
    wet: "f32",
    groundColor: "vec3<f32>",
    snow: "f32",
    sunGlobe: "vec3<f32>",
    hazeStart: "f32",
    origin: "vec2<f32>",
    period: "f32",
    periodMetres: "f32",
    cloudOrigin: "vec2<f32>",
    cloudPeriod: "f32",
    cloudShadow: "f32",
    anchor: "vec2<f32>",
    night: "f32",
    time: "f32",
  },
} as const;

type DrawParams = {
  shaderModuleProps: { project: { viewport: { resolution?: number; longitude?: number; latitude?: number } } };
};
const mod = (v: number, p: number) => ((v % p) + p) % p;

/** A place on web mercator's map, as deck.gl's flat map has it (512 units round the world). */
function mercator(lng: number, lat: number): [number, number] {
  const phi = Math.max(-89.9, Math.min(89.9, lat)) * DEG;
  return [(lng + 180) * MERCATOR_PER_DEGREE, (Math.PI + Math.log(Math.tan(Math.PI / 4 + phi / 2))) * (512 / (2 * Math.PI))];
}

/**
 * The light's props for a layer: the look, and where the layer is, for what
 * lies fixed to the world (cut to the textures' and the clouds' periods, so
 * the numbers stay small enough for the GPU's precision).
 */
function rlightProps(params: DrawParams) {
  const projectProps = params.shaderModuleProps.project;
  const viewport = projectProps.viewport;
  const [lng, lat] = [Math.fround(viewport.longitude ?? 0), Math.fround(viewport.latitude ?? 0)];
  let at: [number, number];
  if (viewport.resolution) {
    // the globe: from the view's centre (the shader measures from it)
    at = mercator(lng, lat);
  } else {
    // the flat map: from deck.gl's origin for the layer
    const { commonOrigin } = project.getUniforms(projectProps as never) as { commonOrigin: number[] };
    at = [commonOrigin[0], commonOrigin[1]];
  }
  // towards the sun in the globe's common space (x, y and z are the Earth's y, −x and z)
  const sun = fromLocal(look.sunDir, lat, lng);
  return {
    sunDir: look.sunDir,
    sunUp: look.sunUp,
    sunColor: look.sunColor,
    ambient: look.ambient,
    skyZenith: look.skyZenith,
    exposure: look.exposure,
    skyHorizon: look.skyHorizon,
    hazeDensity: look.hazeDensity,
    hazeColor: look.hazeColor,
    wet: look.wet,
    groundColor: look.groundColor,
    snow: look.snow,
    sunGlobe: [sun[1], -sun[0], sun[2]],
    hazeStart: look.hazeStart,
    origin: [mod(at[0], PERIOD), mod(at[1], PERIOD)],
    period: PERIOD,
    periodMetres: (PERIOD * 40_075_016.686 * Math.cos(lat * DEG)) / 512,
    cloudOrigin: [mod(at[0], CLOUD_PERIOD), mod(at[1], CLOUD_PERIOD)],
    cloudPeriod: CLOUD_PERIOD,
    cloudShadow: look.cloudShadow,
    anchor: [lng, lat],
    night: look.night,
    time: (performance.now() / 1000) % 100_000,
  };
}

/** The shared light, for the ground's extension (realistic-ground.ts). */
export const realisticLight = { module: rlightModule, props: rlightProps };

// --------------------------------------------------------------- buildings

const rbuildVaryings = (dir: "in" | "out") => /* glsl */ `\
${dir} vec2 rb_uv;
${dir} vec3 rb_n;
${dir} vec3 rb_tangent;
${dir} float rb_z;
flat ${dir} vec3 rb_tint;
flat ${dir} vec4 rb_info;
`;

const rbuildModule = {
  name: "rbuild",
  vs: rbuildVaryings("out"),
  fs: /* glsl */ `\
uniform mediump sampler2DArray rbuild_color;
uniform mediump sampler2DArray rbuild_normal;
uniform mediump sampler2DArray rbuild_mask;
uniform mediump sampler2DArray rbuild_glow;
${rbuildVaryings("in")}`,
} as const;

/** A window bay, metres (Streets GL's), and a storey. */
const BAY_M = 4;
const STOREY_M = 3.3;

/** Metres to a degree of latitude, or of longitude at the equator (deck.gl's globe's). */
const METRES_PER_DEGREE = (6_370_972 * Math.PI) / 180;

const rbuildInject = {
  // at the end of main, where the layer's attributes and main's own variables
  // can be read: the wall's length and height, how far along and up it this vertex is
  "vs:#main-end": /* glsl */ `
    rb_tint = fillColors.rgb;
    float rb_code = floor((1.0 - fillColors.a) * 255.0 + 0.5);
    bool rb_globe = project.projectionMode == PROJECTION_MODE_GLOBE;
  #ifdef IS_SIDE_VERTEX
    {
      // the wall from this corner to the next, metres east and north, and facing out (deck.gl's side)
      vec2 rb_along;
      if (rb_globe) {
        rb_along = vec2((nextPos.x - pos.x) * cos(radians(0.5 * (pos.y + nextPos.y))), nextPos.y - pos.y) * ${METRES_PER_DEGREE.toFixed(3)};
        rb_n = normalize(vec3(-rb_along.y, rb_along.x, 0.0) + vec3(1e-9, 0.0, 0.0));
      } else {
        vec4 rb_a = project_position(vec4(pos, 1.0), pos64Low);
        vec4 rb_b = project_position(vec4(nextPos, 1.0), nextPos64Low);
        rb_along = (rb_b.xy - rb_a.xy) / project.commonUnitsPerMeter.xy;
        rb_n = geometry.normal;
      }
      float rb_length = length(rb_along);
      float rb_height = elevations * solidPolygon.elevationScale;
      // whole bays of windows to the wall, whole storeys to the building (Streets GL's way)
      float rb_bays = floor(rb_length / ${BAY_M.toFixed(1)} + 0.5);
      float rb_levels = max(1.0, floor(rb_height / ${STOREY_M.toFixed(1)} + 0.5));
      rb_uv = vec2(positions.x * (rb_bays > 0.5 ? rb_bays : rb_length / ${BAY_M.toFixed(1)}), positions.y * rb_levels);
      rb_tangent = vec3(rb_along / max(rb_length, 1e-6), 0.0);
      rb_z = positions.y * rb_height;
      rb_info = vec4(rb_code, rb_bays, rb_levels, 1.0);
    }
  #else
    // a roof: its textures lie fixed to the world (rl_t), east along them
    rb_n = rb_globe ? vec3(0.0, 0.0, 1.0) : geometry.normal;
    rb_uv = vec2(0.0);
    rb_tangent = vec3(1.0, 0.0, 0.0);
    rb_z = elevations * solidPolygon.elevationScale;
    rb_info = vec4(rb_code, 0.0, 0.0, 0.0);
  #endif
  `,
  "fs:DECKGL_FILTER_COLOR": /* glsl */ `
  {
    float code = rb_info.x;
    float facade = mod(code, 5.0);
    float roof = floor(code / 5.0 + 0.01);
    bool wall = rb_info.w > 0.5;
    // the building's own choices from its colour: the same all round it
    float which = fract(dot(rb_tint, vec3(71.23, 43.17, 36.19)));
    vec3 tint = rl_linear(rb_tint);
    float layer;
    vec2 uv;
    if (wall) {
      // plaster, brick, block, wood: the wall alone, or a window to each bay; glass towers all glass
      layer = facade > 3.5 ? 8.0 : facade * 2.0 + step(0.5, rb_info.y);
      uv = rb_uv;
    } else {
      // tar, concrete, metal, tiles, eternit and two of Streets GL's flat roofs, each its metres a tile
      // (whole tiles to the textures' period, so they meet where it repeats)
      layer = 9.0 + roof;
      float metres = roof < 0.5 ? 4.0 : roof < 1.5 ? 10.0 : roof < 2.5 ? 4.0 : roof < 3.5 ? 3.0 : roof < 4.5 ? 5.0 : 18.0;
      uv = rl_t * max(1.0, floor(rlight.periodMetres / metres + 0.5));
      // Streets GL's roof colours: tiles terracotta (or slate), metal pale grey-blue (or copper),
      // concrete grey; its flat roofs a little under their textures' white, so the sun doesn't burn them out;
      // tar and eternit darker
      tint = roof > 2.5 && roof < 3.5 ? (which < 0.75 ? vec3(0.597, 0.205, 0.127) : vec3(0.262, 0.262, 0.309))
        : roof > 1.5 && roof < 2.5 ? (which < 0.8 ? vec3(0.546, 0.644, 0.723) : vec3(0.366, 0.59, 0.509))
        : roof > 0.5 && roof < 1.5 ? vec3(0.497)
        : roof > 4.5 ? vec3(0.8)
        : vec3(0.55);
    }
    // (the images are stored top row first: v runs up them)
    vec3 at = vec3(uv.x, -uv.y, layer);
    vec3 c = texture(rbuild_color, at).rgb;
    vec3 nm = texture(rbuild_normal, at).xyz * 2.0 - 1.0;
    vec3 m = texture(rbuild_mask, at).rgb;
    float glow = texture(rbuild_glow, at).r;
    vec3 albedo = c * mix(vec3(1.0), tint, m.b);
    if (!wall) albedo = mix(albedo, vec3(0.8), rlight.snow * 0.92);
    // the texture's u runs along the wall (or east), its v up the wall (or north)
    vec3 N = normalize(rb_n);
    vec3 T = normalize(rb_tangent - N * dot(rb_tangent, N) + vec3(1e-6));
    vec3 B = wall ? vec3(0.0, 0.0, 1.0) : normalize(cross(N, T));
    vec3 n = normalize(T * nm.x + B * nm.y + N * max(nm.z, 0.2));
    vec3 v = normalize(rl_eye);
    // the ground shades the foot of a wall; rain makes everything shinier
    float ao = wall ? mix(0.55, 1.0, smoothstep(0.0, 2.5, rb_z)) : 1.0;
    float roughness = mix(m.r, 0.18, rlight.wet * 0.5);
    vec3 lit = rl_shade(albedo, n, v, roughness, m.g, ao, rl_sunVisible());
    // at night about half the windows are lit, warm, some brighter
    float pick = rl_hash(floor(uv) + rb_tint.xy * 41.0, 1021.0);
    lit += glow * step(0.48, pick) * rlight.night * mix(vec3(1.0, 0.62, 0.3), vec3(1.0, 0.84, 0.62), step(0.85, pick)) * 0.28;
    color = vec4(rl_display(rl_aerial(lit)), 1.0);
  }
  `,
};

type ExtLayer = Layer & { context: { device: Device }; setShaderModuleProps(props: object): void };

/** Buildings with Streets GL's façades and roofs, lit as the realistic map is: for their SolidPolygonLayer. */
export class BuildingExtension extends LayerExtension {
  static extensionName = "RealisticBuildingExtension";

  isEnabled(layer: { getAttributeManager(): { attributes: object } | null; props: { extruded?: boolean } }): boolean {
    return Boolean(layer.props.extruded) && "fillColors" in (layer.getAttributeManager()?.attributes ?? {});
  }

  getShaders(this: never, extension: BuildingExtension) {
    if (!extension.isEnabled(this)) return null;
    return { modules: [rlightModule, rbuildModule], inject: rbuildInject };
  }

  draw(this: ExtLayer, params: DrawParams, extension: BuildingExtension) {
    if (!extension.isEnabled(this as never)) return;
    const textures = realisticTextures(this.context.device).buildings;
    this.setShaderModuleProps({
      rlight: rlightProps(params),
      rbuild: { rbuild_color: textures.color, rbuild_normal: textures.normal, rbuild_mask: textures.mask, rbuild_glow: textures.glow },
    });
  }
}

// --------------------------------------------------------------- trees

const rtreeModule = {
  name: "rtree",
  vs: "out vec3 rt_n;\n",
  fs: "in vec3 rt_n;\n",
  inject: {
    "vs:DECKGL_FILTER_GL_POSITION": "    rt_n = geometry.normal * rl_frame(geometry.position.xyz, project.projectionMode == PROJECTION_MODE_GLOBE);\n",
  },
} as const;

const rtreeInject = {
  "fs:DECKGL_FILTER_COLOR": /* glsl */ `
  {
    vec3 albedo = rl_linear(color.rgb);
    vec3 n = normalize(rt_n);
    vec3 v = normalize(rl_eye);
    float sun = rl_sunVisible();
    // snow on the tops of the crowns
    albedo = mix(albedo, vec3(0.75), rlight.snow * smoothstep(0.2, 0.7, n.z) * 0.8);
    vec3 lit = rl_shade(albedo, n, v, 0.85, 0.0, 1.0, sun);
    // the sun through the leaves, seen from the shaded side
    lit += albedo * rl_sunLight() * 0.3 * pow(max(dot(-v, rl_toSun()), 0.0), 3.0) * sun;
    color = vec4(rl_display(rl_aerial(lit)), color.a);
  }
  `,
};

/** Trees lit as the realistic map is (their material plain, `UNLIT`, so deck.gl's own light leaves them be). */
export class TreeExtension extends LayerExtension {
  static extensionName = "RealisticTreeExtension";

  getShaders() {
    return { modules: [rlightModule, rtreeModule], inject: rtreeInject };
  }

  draw(this: ExtLayer, params: DrawParams) {
    this.setShaderModuleProps({ rlight: rlightProps(params) });
  }
}

/** A material that leaves the colour as it is, for layers lit by these extensions instead. */
export const UNLIT = { ambient: 1, diffuse: 0, shininess: 1, specularColor: [0, 0, 0] as RGB };

// --------------------------------------------------------------- haze for overlays

/** What lies over the ground (shading, shadows) fades away into the distance's air rather than taking its colour. */
export class HazeFadeExtension extends LayerExtension {
  static extensionName = "RealisticHazeFadeExtension";

  getShaders() {
    return {
      modules: [rlightModule],
      inject: {
        "fs:DECKGL_FILTER_COLOR": /* glsl */ `
    color.a *= rl_clear();
  `,
      },
    };
  }

  draw(this: ExtLayer, params: DrawParams) {
    this.setShaderModuleProps({ rlight: rlightProps(params) });
  }
}

/**
 * A band of shade along a line, darkest on it and fading to nothing at its
 * edges (and into the distance's air): where walls meet the ground.
 */
export class FootShadeExtension extends LayerExtension {
  static extensionName = "RealisticFootShadeExtension";

  getShaders() {
    return {
      modules: [rlightModule],
      inject: {
        "fs:DECKGL_FILTER_COLOR": /* glsl */ `
    float across = 1.0 - min(abs(geometry.uv.x), 1.0);
    color.a *= across * rl_clear();
  `,
      },
    };
  }

  draw(this: ExtLayer, params: DrawParams) {
    this.setShaderModuleProps({ rlight: rlightProps(params) });
  }
}

/** One of each, shared by every layer that uses them: the same extension compares equal, so no shader rebuilds. */
export const BUILDINGS = new BuildingExtension();
export const TREES = new TreeExtension();
export const HAZE_FADE = new HazeFadeExtension();
export const FOOT_SHADE = new FootShadeExtension();

// --------------------------------------------------------------- buildings' looks

/** Façades: plaster, brick, concrete block, wood, glass (the order the shader has them). */
const FACADES = 5;
const [PLASTER, BRICK, BLOCK, WOOD, GLASS] = [0, 1, 2, 3, 4];
/** Roofs: tar, concrete, metal, tiles, eternit, and two flat roofs with their plant (the order the shader has them). */
const [TAR, CONCRETE, METAL, TILES, ETERNIT, FLAT1, FLAT3] = [0, 1, 2, 3, 4, 5, 6];

/** Each façade's colours (sRGB): its texture is near white where tinted, so these are the walls'. */
const WALL_COLOURS: Record<number, RGB[]> = {
  [PLASTER]: [
    [238, 234, 222],
    [232, 220, 196],
    [226, 208, 172],
    [214, 214, 208],
    [236, 214, 196],
    [206, 220, 214],
    [240, 228, 180],
  ],
  [BRICK]: [
    [176, 92, 70],
    [150, 78, 60],
    [190, 130, 96],
    [128, 72, 58],
    [206, 176, 136],
  ],
  [BLOCK]: [
    [212, 210, 204],
    [196, 194, 188],
    [222, 216, 204],
  ],
  // Falu red, ochre, white and grey: wooden houses as the north has them
  [WOOD]: [
    [150, 44, 32],
    [150, 44, 32],
    [214, 172, 92],
    [236, 234, 226],
    [168, 170, 166],
  ],
  [GLASS]: [
    [200, 214, 222],
    [190, 206, 200],
    [214, 206, 190],
  ],
};

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

/** A hash of `seed`, 0 to 1, a different one for each `salt`. */
function hash(seed: number, salt: number): number {
  let h = Math.imul(seed ^ Math.imul(salt + 1, 0x9e3779b9), 0x85ebca6b) >>> 0;
  h = Math.imul(h ^ (h >>> 13), 0xc2b2ae35) >>> 0;
  return ((h ^ (h >>> 16)) >>> 0) / 4294967296;
}

const pickWeighted = (r: number, weights: [number, number][]) => {
  const total = weights.reduce((s, [, w]) => s + w, 0);
  let left = r * total;
  for (const [value, w] of weights) {
    if (left < w) return value;
    left -= w;
  }
  return weights[weights.length - 1][0];
};

/**
 * A building's look, picked by `seed` (the same each time it is loaded) and
 * its height: its wall colour (the one OpenStreetMap maps for it, toned down,
 * where it has one), and in the alpha its façade and roof (255 less façade +
 * 5 × roof), which the shader reads.
 */
export function buildingStyle(seed: number, height: number, colour?: unknown): [number, number, number, number] {
  const facade =
    height > 30 && hash(seed, 1) < 0.55
      ? GLASS
      : pickWeighted(hash(seed, 2), [
          [PLASTER, 0.42],
          [BRICK, 0.28],
          [BLOCK, height > 12 ? 0.25 : 0.1],
          [WOOD, height < 9 ? 0.22 : 0.02],
        ]);
  const roof =
    height < 10
      ? pickWeighted(hash(seed, 3), [
          [TILES, 0.55],
          [METAL, 0.2],
          [ETERNIT, 0.15],
          [TAR, 0.1],
        ])
      : pickWeighted(hash(seed, 3), [
          [TAR, 0.35],
          [CONCRETE, 0.25],
          [FLAT1, 0.2],
          [FLAT3, 0.15],
          [METAL, 0.05],
        ]);
  const hex = typeof colour === "string" ? (NAMED[colour.toLowerCase()] ?? colour) : null;
  let rgb: RGB;
  if (hex && /^#[0-9a-f]{6}$/i.test(hex)) {
    const v = parseInt(hex.slice(1), 16);
    const soften = (c: number) => Math.round(c * 0.75 + 200 * 0.25);
    rgb = [soften(v >> 16), soften((v >> 8) & 255), soften(v & 255)];
  } else {
    const list = WALL_COLOURS[facade];
    const base = list[Math.floor(hash(seed, 4) * list.length)];
    // a little variety within each
    const j = Math.round((hash(seed, 5) - 0.5) * 14);
    rgb = base.map((c) => Math.min(255, Math.max(0, c + j))) as RGB;
  }
  return [rgb[0], rgb[1], rgb[2], 255 - (facade + FACADES * roof)];
}
