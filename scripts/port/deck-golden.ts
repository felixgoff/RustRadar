// Golden values from deck.gl itself for the Rust port of the camera and of tile selection (plan item
// S1.32; docs/port/01-globe-camera-basemap.md §2; consumed by items E1 `scene::camera` and E3 `scene::tiles`).
// Run from the repository root:
//
//   bun scripts/port/deck-golden.ts
//
// It writes crates/scene/tests/data/deck/{views,tiles}.json. The output depends on nothing but the deck.gl and
// math.gl packages in node_modules (no clock, no random numbers), so two runs are byte-identical. Once written
// the files are plain data and outlive node_modules.
//
// The files were written with bun (JavaScriptCore). Run under Node (V8, `node --experimental-strip-types`) the
// last bits of Math.sin and friends differ: matrices by up to 3e-15 of their largest element, projected pixels
// by up to 1.3e-7 px (at zoom 20), tile lists not at all. That is the floor for any tolerance; compare matrix
// elements against the matrix's largest element, since some are rounding noise around 0.
//
// Every viewport is made the way the app makes it (src/lib/Globe.svelte, createDeck): a
// `new GlobeView({ resolution: 5 })` given the view state with the free camera's LIMITS spread in, and deck's
// `View.makeViewport`, which picks `GlobeViewport` for zoom <= 12 and `WebMercatorViewport` above.
//
// Conventions of both files
//
//   - Numbers are JavaScript doubles written by JSON.stringify (shortest decimal that reads back as the same
//     double). Every number is finite (the script throws otherwise); -0 is written as 0.
//   - Matrices are 16 numbers in math.gl's order: column-major, so element (row r, column c) is m[c * 4 + r]
//     and the translation is m[12], m[13], m[14]. They act on column vectors: clip = M · [x, y, z, 1].
//   - "Common space" is deck's world space: on the globe a sphere of radius 256 units (lon 0 / lat 0 at
//     (0, -256, 0), north pole at (0, 0, 256)); on the flat map 512 units round the world with y growing north.
//   - Pixels are deck's: CSS pixels, origin at the top left, y down (`topLeft: true`).
//   - A view state is { longitude, latitude, zoom, pitch, bearing } in degrees, plus `position` ([0, 0, metres]:
//     the camera's target raised above the ground) where the view has one. `width` and `height` are the canvas
//     size in CSS pixels.
//
// ---------------------------------------------------------------------------------------------------------
// views.json: one record per view
// ---------------------------------------------------------------------------------------------------------
//
//   name, about         the view's id and what it is for
//   viewState, width, height
//   viewport            "globe" (GlobeViewport) or "mercator" (WebMercatorViewport)
//   zoom, scale         viewport.zoom; viewport.scale: on the globe 2^(zoom - log2(π·cos lat)), on the flat map
//                       2^zoom
//   fovy                vertical field of view in degrees; focalDistance: deck's `altitude` (1.5)
//   metersPerPixel      viewport.metersPerPixel (metersPerUnit[2] / scale)
//   distanceScales      unitsPerMeter, metersPerUnit, unitsPerDegree, degreesPerUnit (each [x, y, z]) as the
//                       viewport holds them: on the flat map those of the view centre's latitude
//   center              the view centre in common space (viewport.center, `position` included)
//   cameraPosition      the camera in common space (translation of the inverse view matrix)
//   viewMatrixUncentered, viewMatrix, projectionMatrix, viewProjectionMatrix, pixelProjectionMatrix,
//   pixelUnprojectionMatrix
//                       the viewport's matrices. viewMatrix = viewMatrixUncentered · translate(-center);
//                       viewProjectionMatrix = projectionMatrix · viewMatrix; pixelProjectionMatrix =
//                       scale(width/2, -height/2, 1) · translate(1, -1, 0) · viewProjectionMatrix (pixels with
//                       y down; z stays NDC depth)
//   frustumPlanes       viewport.getFrustumPlanes(): { left, right, bottom, top, near, far }, each
//                       [nx, ny, nz, distance] exactly as deck returns them (deck's tile traversal negates the
//                       normal before it culls: `new Plane(normal.negate(), distance)`)
//   bounds              viewport.getBounds(): [minLng, minLat, maxLng, maxLat]
//   project[]           { in: [lng, lat, metres], out: viewport.project(in) = [x, y, depth],
//                         back: viewport.unproject(out) = [lng, lat, metres] (the path with a depth) }.
//                       `project([lng, lat])` gives the same x and y as with metres 0. The inputs are the view
//                       centre, the ground points under the `unproject` grid, both also at `liftM` metres, and
//                       on the globe a few far points (antipode, poles, a quarter turn east). On the flat map a
//                       point is kept only when its depth is within [-1, 1] (in front of the camera and inside
//                       the frustum): points far beyond the horizon project to millions of pixels and say
//                       nothing a tolerance in pixels could check.
//   liftM               the height used above: 5 % of the camera's distance from its target, in whole metres
//   unproject[]         { in: [x, y], targetZ, out: viewport.unproject(in, { targetZ }) = [lng, lat] }, for a
//                       5 × 5 grid over the canvas (edges included) and two odd pixels, each at targetZ 0 and
//                       at `liftM`. On the globe a pixel off the limb gives the lon/lat of the ray's closest
//                       approach to the globe's centre (deck clamps the ray/sphere discriminant at 0), i.e. the
//                       nearest point of the limb; on the flat map a pixel above the horizon gives the plane's
//                       intersection behind the camera, as deck computes it. On the globe the targetZ 0
//                       records also carry `anchor`: viewport.getZoomAnchorStrength(in) (1 on the visible
//                       globe, easing to 0 off its edge; GlobeController's wheel-zoom anchoring).
//
// Two traps of the flat map that the records pin: `project` turns metres into units with unitsPerMeter at the
// POINT's latitude, while `unproject`'s targetZ (and `position`) use distanceScales.unitsPerMeter[2] at the view
// centre's latitude. And its `zoom` is the raw view zoom, while the globe's `scale` carries 1/(π·cos lat).
//
// ---------------------------------------------------------------------------------------------------------
// tiles.json: deck's getTileIndices for the same views
// ---------------------------------------------------------------------------------------------------------
//
//   configs[]   { name, tileSize, minZoom, maxZoom, zRange, layers }: tileset options. null means the option
//               is not given (no clamp, no elevation range). `layers` names the app's layers that use it.
//   views[]     { name, viewState, width, height, viewport, tiles: { <config name>: [[x, y, z], ...] } }
//
// The indices are those `Tileset2D.getTileIndices` returns (what TileLayer and MVTLayer select before their
// refinement strategy), in deck's own order: a depth-first walk from tile 0/0/0 with children in the order
// (2x, 2y), (2x, 2y+1), (2x+1, 2y), (2x+1, 2y+1). y counts from the north (OSM / XYZ scheme). The tileset takes
// floor(maxZoom) and ceil(minZoom); the zoom rule is z = Math.round(zoom + log2(512 / tileSize)), then z above
// maxZoom becomes maxZoom (z below minZoom would give no tiles at all, as no config has an extent; no view here
// is that far out). On the globe, and on the flat map above 60° pitch, the walk takes tiles far from the camera
// at a coarser zoom: a tile of level t is taken once t + floor(log2(its bounding volume's distance to the camera
// · scale / height)) reaches the rule's z; otherwise every tile is at the rule's z. GlobeView never repeats the
// world, so nothing is selected across the antimeridian on the flat map.

import { _GlobeView as GlobeView, _GlobeViewport as GlobeViewport } from "@deck.gl/core";
import { _Tileset2D as Tileset2D } from "@deck.gl/geo-layers";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const OUT = join(ROOT, "crates", "scene", "tests", "data", "deck");

// the free camera's limits, spread into the view state as Globe.svelte does (`{ ...view, ...LIMITS }`)
const LIMITS = { minZoom: 0.5, maxZoom: 20, maxPitch: 60 };
// geo.ts: metersPerPixel(zoom, lat) and the camera distance (G:3111)
const EARTH_CIRCUMFERENCE_M = 40_075_016.686;
const CAMERA_HEIGHTS = 1.5;

type ViewState = {
  longitude: number;
  latitude: number;
  zoom: number;
  pitch: number;
  bearing: number;
  position?: [number, number, number];
};
type View = { name: string; about: string; viewState: ViewState; width: number; height: number };

const DESK = { width: 1440, height: 900 };
const PHONE = { width: 390, height: 844 };

function view(
  name: string,
  about: string,
  [longitude, latitude, zoom, pitch, bearing]: number[],
  size: { width: number; height: number },
  position?: [number, number, number],
): View {
  const viewState: ViewState = { longitude, latitude, zoom, pitch, bearing };
  if (position) viewState.position = position;
  return { name, about, viewState, ...size };
}

// 24 views: zoom 0.5 to 20 (and the locked camera's 23), latitudes 0, ±45, ±60, 80, pitch 0 to 85,
// bearings all round. Twelve on each side of the projection swap at zoom 12.
const VIEWS: View[] = [
  view("globe-z0.5-equator", "the whole globe, corners off the limb", [0, 0, 0.5, 0, 0], DESK),
  view("globe-z0.5-lat45-tilted", "the whole globe from the north-west, tilted and turned", [-100, 45, 0.5, 30, 20], DESK),
  view("globe-z1.5-lat60-phone", "the globe on a phone, enlarged by 1/cos 60°", [18, 60, 1.5, 0, 0], PHONE),
  view("globe-z2-lat80", "near the pole, pitched, bearing negative", [150, 80, 2, 45, -30], DESK),
  view("globe-z2.5-south45", "a half zoom (round half up) south of the equator", [170, -45, 2.5, 0, 0], DESK),
  view("globe-z3-equator-pitch60", "the free camera's steepest pitch, bearing east", [120, 0, 3, 60, 90], DESK),
  view("globe-z4.5-antimeridian", "centred just west of the antimeridian, bearing 350", [179.9, 45, 4.5, 20, 350], DESK),
  view("globe-z5-south60", "south, looking south", [-150, -60, 5, 55, 180], DESK),
  view("globe-z6.3-lat60", "a fractional zoom", [-40, 60, 6.3, 10, 45], DESK),
  view("globe-z7.7-equator-phone", "a phone at the equator, north up", [-60, 0, 7.7, 0, 0], PHONE),
  view("globe-z9.5-lat45-pitch60", "a half zoom, tilted, bearing negative", [2.35, 45, 9.5, 60, -120], DESK),
  view("globe-z12-lat80", "the last globe zoom, at 80°N", [15.6, 80, 12, 60, 200], DESK),
  view("mercator-z12.01-lat45", "just past the swap to the flat map", [7.4, 45, 12.01, 0, 0], DESK),
  view("mercator-z13-equator-pitch60", "flat map at the equator, the free camera's steepest pitch", [-78.5, 0, 13, 60, 30], DESK),
  view("mercator-z13.5-south45-phone", "a half zoom on a phone, south", [-70.6, -45, 13.5, 0, 0], PHONE),
  view("mercator-z14-lat60-pitch75", "the realistic map's steepest pitch (coarser tiles far off)", [10.7, 60, 14, 75, -15], DESK),
  view("mercator-z15-lat80", "flat map at 80°N", [15.6, 80, 15, 45, 100], DESK),
  view("mercator-z16-lat45-phone", "a phone, north up", [-73.98, 45, 16, 0, 0], PHONE),
  view("mercator-z17-lat60-raised", "locked on an aircraft: the target raised 1500 m", [18.06, 60, 17, 70, 250], DESK, [0, 0, 1500]),
  view("mercator-z18-equator", "close in at the equator, bearing negative", [103.8, 0, 18, 30, -60], DESK),
  view("mercator-z19-lat45-pitch85", "locked at the steepest pitch, target raised 300 m", [2.29, 45, 19, 85, 10], DESK, [0, 0, 300]),
  view("mercator-z20-lat80", "the free camera's closest zoom at 80°N", [-68.7, 80, 20, 60, 315], DESK),
  view("mercator-z20-antimeridian-south60", "the flat map centred just west of the antimeridian", [179.999, -60, 20, 50, 90], DESK),
  view("mercator-z23-origin-phone", "the locked camera's closest zoom, on lon 0 / lat 0 (tile edges)", [0, 0, 23, 85, 135], PHONE, [0, 0, 12]),
];

type Config = { name: string; tileSize: number; minZoom: number | null; maxZoom: number | null; zRange: [number, number] | null; layers: string };
const CONFIGS: Config[] = [
  { name: "vector", tileSize: 512, minZoom: 0, maxZoom: 14, zRange: null, layers: "map-globe, map-flat, aeroway-true-*, label-tiles (OpenFreeMap)" },
  { name: "imagery", tileSize: 256, minZoom: 0, maxZoom: 19, zRange: null, layers: "imagery (satellite basemap)" },
  { name: "radar", tileSize: 256, minZoom: 0, maxZoom: 7, zRange: null, layers: "radar-* (weather radar)" },
  { name: "free512", tileSize: 512, minZoom: null, maxZoom: null, zRange: null, layers: "none: the zoom rule unclamped" },
  { name: "free256", tileSize: 256, minZoom: null, maxZoom: null, zRange: null, layers: "none: the zoom rule unclamped" },
  { name: "elevated512", tileSize: 512, minZoom: 0, maxZoom: 14, zRange: [-100, 3000], layers: "none: a tileset given an elevation range (as deck's TerrainLayer gives one)" },
];

// --- helpers -------------------------------------------------------------------------------------------------

const RAD = Math.PI / 180;

function metersPerPixel(zoom: number, lat: number): number {
  return (EARTH_CIRCUMFERENCE_M * Math.cos(lat * RAD)) / (512 * 2 ** zoom);
}

function wrap180(lon: number): number {
  return lon > 180 ? lon - 360 : lon < -180 ? lon + 360 : lon;
}

/** A plain array of finite numbers (throws on NaN or infinity: JSON has neither). */
function nums(values: ArrayLike<number>, what: string): number[] {
  const out = Array.from(values, Number);
  for (const v of out) if (!Number.isFinite(v)) throw new Error(`${what}: not finite: ${out.join(", ")}`);
  return out.map((v) => (Object.is(v, -0) ? 0 : v));
}

function num(v: number, what: string): number {
  return nums([v], what)[0];
}

type Viewport = {
  width: number;
  height: number;
  zoom: number;
  scale: number;
  fovy: number;
  focalDistance: number;
  metersPerPixel: number;
  center: ArrayLike<number>;
  cameraPosition: ArrayLike<number>;
  distanceScales: Record<string, ArrayLike<number>>;
  viewMatrixUncentered: ArrayLike<number>;
  viewMatrix: ArrayLike<number>;
  projectionMatrix: ArrayLike<number>;
  viewProjectionMatrix: ArrayLike<number>;
  pixelProjectionMatrix: ArrayLike<number>;
  pixelUnprojectionMatrix: ArrayLike<number>;
  getFrustumPlanes(): Record<string, { normal: ArrayLike<number>; distance: number }>;
  getBounds(): number[];
  project(xyz: number[]): number[];
  unproject(xyz: number[], opts?: { targetZ?: number }): number[];
  getZoomAnchorStrength?(xy: number[]): number;
};

const globeView = new GlobeView({ resolution: 5 });

function makeViewport(v: View): Viewport {
  const viewport = globeView.makeViewport({ width: v.width, height: v.height, viewState: { ...v.viewState, ...LIMITS } });
  if (!viewport) throw new Error(`${v.name}: no viewport`);
  return viewport as unknown as Viewport;
}

function isGlobe(viewport: Viewport): boolean {
  return viewport instanceof GlobeViewport;
}

/** The unproject grid: 5 × 5 over the canvas, edges included, and two odd pixels. */
function grid(width: number, height: number): [number, number][] {
  const out: [number, number][] = [];
  for (let j = 0; j <= 4; j++) for (let i = 0; i <= 4; i++) out.push([(width * i) / 4, (height * j) / 4]);
  out.push([123.25, 456.75], [width - 0.5, 0.5]);
  return out;
}

// --- views.json ----------------------------------------------------------------------------------------------

function viewRecord(v: View): string {
  const viewport = makeViewport(v);
  const globe = isGlobe(viewport);
  const { longitude, latitude, zoom } = v.viewState;
  const liftM = Math.max(1, Math.round(0.05 * CAMERA_HEIGHTS * v.height * metersPerPixel(zoom, latitude)));
  const what = (s: string) => `${v.name}: ${s}`;

  const unproject: string[] = [];
  const ground: [number, number][] = [];
  for (const targetZ of [0, liftM]) {
    for (const xy of grid(v.width, v.height)) {
      const out = nums(viewport.unproject(xy, { targetZ }).slice(0, 2), what(`unproject ${xy}`));
      const record: Record<string, unknown> = { in: xy, targetZ, out };
      if (globe && targetZ === 0) record.anchor = num(viewport.getZoomAnchorStrength!(xy), what(`anchor ${xy}`));
      if (targetZ === 0) ground.push(out as [number, number]);
      unproject.push(JSON.stringify(record));
    }
  }

  const inputs: number[][] = [
    [longitude, latitude, 0],
    [longitude, latitude, liftM],
  ];
  for (const [lng, lat] of ground) inputs.push([lng, lat, 0], [lng, lat, liftM]);
  if (globe) {
    inputs.push(
      [wrap180(longitude + 180), -latitude, 0],
      [0, 90, 0],
      [0, -90, 0],
      [wrap180(longitude + 90), 0, 0],
      [longitude, latitude, 2_000_000],
    );
  }
  const project: string[] = [];
  for (const input of inputs) {
    const out = nums(viewport.project(input), what(`project ${input}`));
    if (!globe && !(out[2] >= -1 && out[2] <= 1)) continue;
    const back = nums(viewport.unproject(out), what(`unproject back ${out}`));
    project.push(JSON.stringify({ in: input, out, back }));
  }

  const planes = viewport.getFrustumPlanes();
  const frustumPlanes: Record<string, number[]> = {};
  for (const side of ["left", "right", "bottom", "top", "near", "far"]) {
    const { normal, distance } = planes[side];
    frustumPlanes[side] = nums([...Array.from(normal), distance], what(`plane ${side}`));
  }
  const ds = viewport.distanceScales;
  const distanceScales: Record<string, number[]> = {};
  for (const key of ["unitsPerMeter", "metersPerUnit", "unitsPerDegree", "degreesPerUnit"]) {
    distanceScales[key] = nums(ds[key], what(key));
  }
  const matrix = (name: keyof Viewport) => JSON.stringify(nums(viewport[name] as ArrayLike<number>, what(String(name))));

  return [
    "{",
    `"name": ${JSON.stringify(v.name)},`,
    `"about": ${JSON.stringify(v.about)},`,
    `"viewState": ${JSON.stringify(v.viewState)}, "width": ${v.width}, "height": ${v.height},`,
    `"viewport": ${JSON.stringify(globe ? "globe" : "mercator")},`,
    `"zoom": ${num(viewport.zoom, what("zoom"))}, "scale": ${num(viewport.scale, what("scale"))}, ` +
      `"fovy": ${num(viewport.fovy, what("fovy"))}, "focalDistance": ${num(viewport.focalDistance, what("focalDistance"))}, ` +
      `"metersPerPixel": ${num(viewport.metersPerPixel, what("metersPerPixel"))},`,
    `"distanceScales": ${JSON.stringify(distanceScales)},`,
    `"center": ${JSON.stringify(nums(viewport.center, what("center")))},`,
    `"cameraPosition": ${JSON.stringify(nums(viewport.cameraPosition, what("cameraPosition")))},`,
    `"viewMatrixUncentered": ${matrix("viewMatrixUncentered")},`,
    `"viewMatrix": ${matrix("viewMatrix")},`,
    `"projectionMatrix": ${matrix("projectionMatrix")},`,
    `"viewProjectionMatrix": ${matrix("viewProjectionMatrix")},`,
    `"pixelProjectionMatrix": ${matrix("pixelProjectionMatrix")},`,
    `"pixelUnprojectionMatrix": ${matrix("pixelUnprojectionMatrix")},`,
    `"frustumPlanes": ${JSON.stringify(frustumPlanes)},`,
    `"bounds": ${JSON.stringify(nums(viewport.getBounds(), what("bounds")))},`,
    `"liftM": ${liftM},`,
    `"project": [\n${project.join(",\n")}\n],`,
    `"unproject": [\n${unproject.join(",\n")}\n]`,
    "}",
  ].join("\n");
}

// --- tiles.json ----------------------------------------------------------------------------------------------

type Index = { x: number; y: number; z: number };
type TilesetLike = {
  _maxZoom?: number;
  _minZoom?: number;
  getTileIndices(args: {
    viewport: unknown;
    maxZoom?: number;
    minZoom?: number;
    zRange: [number, number] | null;
    modelMatrix: null;
    modelMatrixInverse: null;
  }): Index[];
};

function tileIndices(viewport: Viewport, c: Config): number[][] {
  const opts: Record<string, unknown> = { tileSize: c.tileSize, getTileData: () => null };
  if (c.minZoom !== null) opts.minZoom = c.minZoom;
  if (c.maxZoom !== null) opts.maxZoom = c.maxZoom;
  const tileset = new Tileset2D(opts as never) as unknown as TilesetLike;
  // what Tileset2D.update hands it (its own floor(maxZoom) and ceil(minZoom), the layer's zRange)
  const indices = tileset.getTileIndices({
    viewport,
    maxZoom: tileset._maxZoom,
    minZoom: tileset._minZoom,
    zRange: c.zRange,
    modelMatrix: null,
    modelMatrixInverse: null,
  });
  return indices.map(({ x, y, z }) => nums([x, y, z], `tile of ${c.name}`));
}

function tilesRecord(v: View): string {
  const viewport = makeViewport(v);
  const lines = CONFIGS.map((c) => `${JSON.stringify(c.name)}: ${JSON.stringify(tileIndices(viewport, c))}`);
  return [
    "{",
    `"name": ${JSON.stringify(v.name)},`,
    `"viewState": ${JSON.stringify(v.viewState)}, "width": ${v.width}, "height": ${v.height},`,
    `"viewport": ${JSON.stringify(isGlobe(viewport) ? "globe" : "mercator")},`,
    `"tiles": {\n${lines.join(",\n")}\n}`,
    "}",
  ].join("\n");
}

// --- write ---------------------------------------------------------------------------------------------------

function version(pkg: string): string {
  return JSON.parse(readFileSync(join(ROOT, "node_modules", pkg, "package.json"), "utf8")).version;
}
const versions = {
  "@deck.gl/core": version("@deck.gl/core"),
  "@deck.gl/geo-layers": version("@deck.gl/geo-layers"),
  "@math.gl/web-mercator": version("@math.gl/web-mercator"),
  "@math.gl/culling": version("@math.gl/culling"),
};

if (VIEWS.length !== 24) throw new Error(`${VIEWS.length} views, the plan asks for 24`);
if (new Set(VIEWS.map((v) => v.name)).size !== VIEWS.length) throw new Error("view names repeat");

mkdirSync(OUT, { recursive: true });
let total = 0;
function write(name: string, text: string): void {
  writeFileSync(join(OUT, name), text);
  total += Buffer.byteLength(text);
  console.log(`${name}: ${Buffer.byteLength(text)} bytes`);
}

write(
  "views.json",
  [
    "{",
    `"about": ${JSON.stringify(
      "deck.gl's GlobeView viewports (GlobeViewport for zoom <= 12, WebMercatorViewport above) for 24 views: " +
        "matrices, project, unproject. Written by scripts/port/deck-golden.ts; its header documents every field.",
    )},`,
    `"versions": ${JSON.stringify(versions)},`,
    `"views": [\n${VIEWS.map(viewRecord).join(",\n")}\n]`,
    "}",
    "",
  ].join("\n"),
);

write(
  "tiles.json",
  [
    "{",
    `"about": ${JSON.stringify(
      "deck.gl's Tileset2D.getTileIndices ([x, y, z], OSM scheme, deck's traversal order) for the 24 views of " +
        "views.json and each tileset config. Written by scripts/port/deck-golden.ts; its header documents every field.",
    )},`,
    `"versions": ${JSON.stringify(versions)},`,
    `"configs": [\n${CONFIGS.map((c) => JSON.stringify(c)).join(",\n")}\n],`,
    `"views": [\n${VIEWS.map(tilesRecord).join(",\n")}\n]`,
    "}",
    "",
  ].join("\n"),
);

console.log(`total ${total} bytes`);
