#!/usr/bin/env node
// Where deck.gl puts the map at the reference views: by its own viewport math on the CPU, and where Chromium on
// SwiftShader draws it (plan item F4, the "V3 placement" of the stages 8-10 status, T1's note that today's
// pictures stand about 4.5 px higher than deck's own viewport puts the map). Run from the repository root:
//
//   node scripts/port/deck-placement.mjs
//
// Needs bun (to bundle deck.gl from node_modules for the page), the Playwright module (global or local) and its
// Chromium headless shell, started as scripts/port/reference-shots.mjs starts it (`--use-angle=swiftshader
// --enable-unsafe-swiftshader`, device scale factor 1), which made the pictures in docs/port/reference/. It writes
// crates/scene/tests/data/deck/placement.json; nothing else is read or written, and no network is used.
//
// What it found. deck's globe places each vertex in its vertex shader (`project_globe_` in
// @deck.gl/core/dist/shaderlib/project/project.glsl.js): sin and cos of the vertex's longitude and latitude on the
// GPU, in f32, on a sphere of 256 units. GLSL ES leaves their precision to the driver; SwiftShader's are off by up to
// 1.9e-4 here. At zoom 9 a pixel is 0.003 units of that sphere, so at V3 the whole map is drawn 4.7 px higher and
// 0.7 px further right than deck's own `viewport.project` puts it (3.5 to 5.9 px across the canvas); at V2 (zoom
// 4.5) the same error is 22 times smaller in pixels, under 0.3 px; the flat map (zoom above 12) draws relative to
// the view centre and is not affected. The port projects in f64 on the CPU and keeps the GPU's own sin and cos out
// of its shaders (crates/map/src/shaders/lib/project.wgsl), so it puts the map where deck's math does; the
// reference pictures of the globe at V3 carry SwiftShader's error. A real GPU's sin and cos are typically far
// closer, so today's app on a desktop draws V3 where deck's math (and the port) does.
//
// placement.json
//
//   about
//   placement[]    deck's CPU viewport, made as the app makes it (src/lib/Globe.svelte, createDeck: a
//                  `new GlobeView({ resolution: 5 })` given the view state with the free camera's LIMITS spread in,
//                  and deck's `View.makeViewport`, GlobeViewport up to zoom 12 and WebMercatorViewport above), at
//                  V2, along the way from V2 to V3, at V3, and at V3's centre on to zoom 12.1, at 1440 × 900 and
//                  390 × 844 (the reference pictures' canvases, CSS pixels):
//                    { name, about, viewState, width, height, viewport: "globe" | "mercator", grid: [nx, ny],
//                      points: [{ in: [lng, lat], px: viewport.project(in) }] }
//                  The points are the pixels of an nx × ny grid over the canvas, edges included (row by row from
//                  the top left), unprojected to the ground: px gives the grid's pixel back to about 1e-9.
//   swiftshader    where Chromium on SwiftShader draws deck's globe at the reference views V2, V3 and V7:
//                    renderer   the WebGL renderer string
//                    views[]    { name, viewState, width, height, grid: [nx, ny], shift: [[dx, dy], ...],
//                                 mean: [dx, dy], min: [dx, dy], max: [dx, dy] }
//                               shift is drawn minus viewport.project in CSS pixels (x right, y down; rounded to
//                               0.001) at the grid's pixels, row by row from the top left: where deck's vertex
//                               shader puts a ground point at that pixel, read back with transform feedback through
//                               deck's `project_globe_` and `project_common_position_to_clipspace` (copied below)
//                               given the viewport's own viewProjectionMatrix as deck's uniforms give it (f32).
//                    dots       the same through deck itself: a ScatterplotLayer in a GlobeView at V3 1440 × 900
//                               and 390 × 844, each dot's centroid read off the canvas against the shift above
//                               (worst and mean difference in px); the script stops if they disagree by more than
//                               0.35 px.

import { execFileSync, execSync } from "node:child_process";
import { createRequire } from "node:module";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const OUT = join(ROOT, "crates/scene/tests/data/deck/placement.json");

const DESK = { width: 1440, height: 900 };
const PHONE = { width: 390, height: 844 };
const SIZES = [
  ["1440x900", DESK],
  ["390x844", PHONE],
];

const view = ([latitude, longitude, zoom, pitch = 0, bearing = 0]) => ({ longitude, latitude, zoom, pitch, bearing });
const V2 = [52, 10, 4.5];
const V3 = [59.33, 18.07, 9];
const V7 = [78.2, 15.6, 6];

/** The CPU records: V2, three steps from V2 to V3 (centre and zoom moved together), V3, then on towards the flat map. */
function placementViews() {
  const out = [];
  const add = (name, about, at) => {
    for (const [size, { width, height }] of SIZES) out.push({ name: `${name}-${size}`, about, viewState: view(at), width, height });
  };
  add("v2", "V2, Europe at zoom 4.5", V2);
  for (const t of [0.25, 0.5, 0.75]) {
    const at = V2.map((a, i) => a + (V3[i] - a) * t);
    add(`v2-v3-${t}`, `a ${t * 100} % step from V2 to V3 (centre and zoom)`, at);
  }
  add("v3", "V3, the Stockholm region at zoom 9", V3);
  for (const zoom of [10, 11, 11.9, 12, 12.1]) add(`v3-z${zoom}`, `V3's centre at zoom ${zoom}`, [V3[0], V3[1], zoom]);
  return out;
}

const SWIFTSHADER_VIEWS = [
  ["v2", V2],
  ["v3", V3],
  ["v7", V7],
];

// --- the page ------------------------------------------------------------------------------------------------

const PAGE = (root) => `
import { Deck, _GlobeView as GlobeView, _GlobeViewport as GlobeViewport } from "${root}/node_modules/@deck.gl/core/dist/index.js";
import { ScatterplotLayer } from "${root}/node_modules/@deck.gl/layers/dist/index.js";

// the free camera's limits, spread into the view state as Globe.svelte does ({ ...view, ...LIMITS })
const LIMITS = { minZoom: 0.5, maxZoom: 20, maxPitch: 60 };
const globeView = new GlobeView({ resolution: 5 });
const makeViewport = (viewState, width, height) => globeView.makeViewport({ width, height, viewState: { ...viewState, ...LIMITS } });

/**
 * An nx × ny grid of pixels over the canvas, edges included (or, inner, the middles of an nx × ny split of it),
 * unprojected to the ground, and deck's project of each.
 */
window.cpu = (viewState, width, height, nx, ny, inner = false) => {
  const viewport = makeViewport(viewState, width, height);
  const at = (i, n) => (inner ? (i + 0.5) / n : i / (n - 1));
  const points = [];
  for (let j = 0; j < ny; j++)
    for (let i = 0; i < nx; i++) {
      const [lng, lat] = viewport.unproject([at(i, nx) * width, at(j, ny) * height]);
      const [x, y] = viewport.project([lng, lat, 0]);
      points.push({ in: [lng, lat], px: [x, y] });
    }
  return { viewport: viewport instanceof GlobeViewport ? "globe" : "mercator", points };
};

// deck's own vertex shader functions (@deck.gl/core 9.4, shaderlib/project/project.glsl.js), the globe's path
const VS = \`#version 300 es
precision highp float;
in vec3 lnglatz;
uniform mat4 viewProjectionMatrix;
uniform vec4 center;
out vec4 clip;
const float EARTH_RADIUS = 6370972.0;
const float GLOBE_RADIUS = 256.0;
vec3 project_globe_(vec3 lnglatz) {
  float lambda = radians(lnglatz.x);
  float phi = radians(lnglatz.y);
  float cosPhi = cos(phi);
  float D = (lnglatz.z / EARTH_RADIUS + 1.0) * GLOBE_RADIUS;
  return vec3(sin(lambda) * cosPhi, -cos(lambda) * cosPhi, sin(phi)) * D;
}
vec4 project_common_position_to_clipspace(vec4 position, mat4 viewProjectionMatrix, vec4 center) {
  return viewProjectionMatrix * position + center;
}
void main() {
  clip = project_common_position_to_clipspace(vec4(project_globe_(lnglatz), 1.0), viewProjectionMatrix, center);
  gl_Position = vec4(0.0);
}\`;
const FS = \`#version 300 es
precision highp float;
out vec4 color;
void main() { color = vec4(1.0); }\`;

let gl = null;
let program = null;
let renderer = "";
function setUp() {
  if (gl) return;
  gl = document.createElement("canvas").getContext("webgl2");
  const shader = (type, source) => {
    const s = gl.createShader(type);
    gl.shaderSource(s, source);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s));
    return s;
  };
  program = gl.createProgram();
  gl.attachShader(program, shader(gl.VERTEX_SHADER, VS));
  gl.attachShader(program, shader(gl.FRAGMENT_SHADER, FS));
  gl.transformFeedbackVaryings(program, ["clip"], gl.INTERLEAVED_ATTRIBS);
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(program));
  const info = gl.getExtension("WEBGL_debug_renderer_info");
  renderer = gl.getParameter(info ? info.UNMASKED_RENDERER_WEBGL : gl.RENDERER);
}

/** Where deck's vertex shader puts each [lng, lat] (f32, as its position attributes hold them), in CSS pixels. */
function drawnAt(viewport, lngLats) {
  setUp();
  gl.useProgram(program);
  // deck's globe has no offset origin: the uniforms are the viewport's viewProjectionMatrix and a zero centre
  gl.uniformMatrix4fv(gl.getUniformLocation(program, "viewProjectionMatrix"), false, new Float32Array(viewport.viewProjectionMatrix));
  gl.uniform4f(gl.getUniformLocation(program, "center"), 0, 0, 0, 0);
  const n = lngLats.length;
  const input = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, input);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(lngLats.flatMap(([lng, lat]) => [lng, lat, 0])), gl.STATIC_DRAW);
  const at = gl.getAttribLocation(program, "lnglatz");
  gl.enableVertexAttribArray(at);
  gl.vertexAttribPointer(at, 3, gl.FLOAT, false, 0, 0);
  const output = gl.createBuffer();
  gl.bindBuffer(gl.TRANSFORM_FEEDBACK_BUFFER, output);
  gl.bufferData(gl.TRANSFORM_FEEDBACK_BUFFER, n * 16, gl.STATIC_READ);
  const feedback = gl.createTransformFeedback();
  gl.bindTransformFeedback(gl.TRANSFORM_FEEDBACK, feedback);
  gl.bindBufferBase(gl.TRANSFORM_FEEDBACK_BUFFER, 0, output);
  gl.enable(gl.RASTERIZER_DISCARD);
  gl.beginTransformFeedback(gl.POINTS);
  gl.drawArrays(gl.POINTS, 0, n);
  gl.endTransformFeedback();
  gl.disable(gl.RASTERIZER_DISCARD);
  gl.bindTransformFeedback(gl.TRANSFORM_FEEDBACK, null);
  const clip = new Float32Array(n * 4);
  gl.getBufferSubData(gl.TRANSFORM_FEEDBACK_BUFFER, 0, clip);
  const { width, height } = viewport;
  const out = [];
  for (let k = 0; k < n; k++) {
    const [x, y, , w] = clip.subarray(k * 4, k * 4 + 4);
    out.push([((x / w + 1) / 2) * width, ((1 - y / w) / 2) * height]);
  }
  return out;
}

/** The shift field: drawn minus deck's project at an nx × ny grid of pixels. */
window.shift = (viewState, width, height, nx, ny) => {
  const viewport = makeViewport(viewState, width, height);
  const { points } = window.cpu(viewState, width, height, nx, ny);
  const drawn = drawnAt(viewport, points.map((p) => p.in));
  return { renderer, shift: points.map((p, k) => [drawn[k][0] - p.px[0], drawn[k][1] - p.px[1]]) };
};

/** deck itself: white dots at the grid's ground points, drawn by a ScatterplotLayer in a GlobeView; their centroids. */
window.dots = (viewState, width, height, nx, ny) =>
  new Promise((resolve, reject) => {
    const viewport = makeViewport(viewState, width, height);
    const { points } = window.cpu(viewState, width, height, nx, ny, true);
    const parent = document.getElementById("map");
    Object.assign(parent.style, { width: width + "px", height: height + "px" });
    let deck = null;
    let done = false;
    const read = () => {
      if (done) return;
      done = true;
      const canvas = deck.getCanvas();
      const copy = document.createElement("canvas");
      [copy.width, copy.height] = [canvas.width, canvas.height];
      const ctx = copy.getContext("2d");
      ctx.drawImage(canvas, 0, 0);
      const { data } = ctx.getImageData(0, 0, copy.width, copy.height);
      const drawn = drawnAt(viewport, points.map((p) => p.in));
      const out = points.map((p, k) => {
        const [cx, cy] = [Math.round(p.px[0]), Math.round(p.px[1])];
        let [sum, sx, sy] = [0, 0, 0];
        for (let y = cy - 12; y <= cy + 12; y++)
          for (let x = cx - 12; x <= cx + 12; x++) {
            if (x < 0 || y < 0 || x >= copy.width || y >= copy.height) continue;
            const v = data[(y * copy.width + x) * 4];
            [sum, sx, sy] = [sum + v, sx + v * (x + 0.5), sy + v * (y + 0.5)];
          }
        return { centroid: sum > 0 ? [sx / sum, sy / sum] : null, drawn: drawn[k], whole: sum / 255 };
      });
      deck.finalize();
      resolve(out);
    };
    try {
      deck = new Deck({
        parent,
        views: globeView,
        initialViewState: { ...viewState, ...LIMITS },
        useDevicePixels: true,
        controller: false,
        deviceProps: { webgl: { preserveDrawingBuffer: true } },
        layers: [
          new ScatterplotLayer({
            id: "dots",
            data: points.map((p) => p.in),
            getPosition: (d) => d,
            getRadius: 6,
            radiusUnits: "pixels",
            getFillColor: [255, 255, 255, 255],
            antialiasing: false,
            // (GlobeView culls back faces; Globe.svelte's NO_CULL for camera-facing quads)
            parameters: { cullMode: "none", depthCompare: "always" },
          }),
        ],
        onAfterRender: () => setTimeout(read, 100),
        onError: reject,
      });
    } catch (e) {
      reject(e);
    }
  });
`;

const INDEX = `<!doctype html>
<html><head><meta charset="utf-8"><style>html, body { margin: 0; padding: 0; background: #000; overflow: hidden } #map { position: absolute; left: 0; top: 0 }</style></head>
<body><div id="map"></div><script src="page.bundle.js"></script></body></html>
`;

// --- helpers -------------------------------------------------------------------------------------------------

async function loadPlaywright() {
  try {
    return await import("playwright");
  } catch {
    const require = createRequire(import.meta.url);
    return require(join(execSync("npm root -g").toString().trim(), "playwright"));
  }
}

/** A plain array of finite numbers (throws on NaN or infinity: JSON has neither); -0 written as 0. */
function nums(values, what) {
  const out = Array.from(values, Number);
  for (const v of out) if (!Number.isFinite(v)) throw new Error(`${what}: not finite: ${out.join(", ")}`);
  return out.map((v) => (Object.is(v, -0) ? 0 : v));
}

const round3 = (v) => {
  const r = Math.round(v * 1000) / 1000;
  return Object.is(r, -0) ? 0 : r;
};

function stats(shift) {
  const mean = [0, 1].map((a) => shift.reduce((s, d) => s + d[a], 0) / shift.length);
  const min = [0, 1].map((a) => Math.min(...shift.map((d) => d[a])));
  const max = [0, 1].map((a) => Math.max(...shift.map((d) => d[a])));
  return { mean: mean.map(round3), min: min.map(round3), max: max.map(round3) };
}

// --- main ----------------------------------------------------------------------------------------------------

const work = mkdtempSync(join(tmpdir(), "deck-placement-"));
try {
  writeFileSync(join(work, "page.js"), PAGE(ROOT));
  writeFileSync(join(work, "index.html"), INDEX);
  execFileSync("bun", ["build", join(work, "page.js"), "--target=browser", `--outfile=${join(work, "page.bundle.js")}`], { stdio: "inherit" });

  const { chromium } = await loadPlaywright();
  const browser = await chromium.launch({ args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"] });
  const pages = {};
  const pageFor = async ({ width, height }) => {
    const key = `${width}x${height}`;
    if (!pages[key]) {
      const context = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: 1 });
      const page = await context.newPage();
      page.on("pageerror", (e) => console.error("page error:", e.message));
      await page.goto("file://" + join(work, "index.html"));
      pages[key] = page;
    }
    return pages[key];
  };

  const placement = [];
  for (const v of placementViews()) {
    const page = await pageFor(v);
    const { viewport, points } = await page.evaluate(([vs, w, h]) => window.cpu(vs, w, h, 9, 9), [v.viewState, v.width, v.height]);
    placement.push(
      JSON.stringify({
        name: v.name,
        about: v.about,
        viewState: v.viewState,
        width: v.width,
        height: v.height,
        viewport,
        grid: [9, 9],
        points: points.map((p, k) => ({ in: nums(p.in, `${v.name} ${k}`), px: nums(p.px, `${v.name} ${k}`) })),
      }),
    );
  }

  let renderer = "";
  const views = [];
  for (const [name, at] of SWIFTSHADER_VIEWS) {
    for (const [size, dims] of SIZES) {
      const grid = dims.width > 1000 ? [17, 11] : [7, 13];
      const page = await pageFor(dims);
      const r = await page.evaluate(([vs, w, h, [nx, ny]]) => window.shift(vs, w, h, nx, ny), [view(at), dims.width, dims.height, grid]);
      renderer = r.renderer;
      const shift = r.shift.map((d, k) => nums(d, `${name} ${size} ${k}`).map(round3));
      views.push({ name: `${name}-${size}`, viewState: view(at), width: dims.width, height: dims.height, grid, shift, ...stats(shift) });
      console.log(`${name} ${size}: drawn minus deck's project, mean ${stats(shift).mean.join(", ")} px`);
    }
  }

  // the transform feedback against deck itself
  const dots = [];
  for (const [size, dims] of SIZES) {
    const grid = dims.width > 1000 ? [9, 6] : [4, 8];
    // a new page: deck takes the canvas
    const context = await browser.newContext({ viewport: dims, deviceScaleFactor: 1 });
    const page = await context.newPage();
    await page.goto("file://" + join(work, "index.html"));
    const read = await page.evaluate(([vs, w, h, [nx, ny]]) => window.dots(vs, w, h, nx, ny), [view(V3), dims.width, dims.height, grid]);
    await context.close();
    const diffs = read.map((d, k) => {
      if (!d.centroid || d.whole < 50) throw new Error(`v3 ${size}: dot ${k} not drawn whole (${d.whole} px)`);
      return Math.hypot(d.centroid[0] - d.drawn[0], d.centroid[1] - d.drawn[1]);
    });
    const worst = Math.max(...diffs);
    const mean = diffs.reduce((a, b) => a + b, 0) / diffs.length;
    console.log(`v3 ${size}: deck's dots against the transform feedback: worst ${worst.toFixed(3)} px, mean ${mean.toFixed(3)} px`);
    if (worst > 0.35) throw new Error(`v3 ${size}: deck's dots are ${worst} px from the transform feedback`);
    dots.push({ name: `v3-${size}`, count: diffs.length, worst: round3(worst), mean: round3(mean) });
  }
  await browser.close();

  const text = [
    "{",
    `"about": ${JSON.stringify("deck.gl 9.4's placement of the map at the reference views: its CPU viewport (placement) and Chromium on SwiftShader's drawing of its globe (swiftshader); written by scripts/port/deck-placement.mjs")},`,
    `"placement": [\n${placement.join(",\n")}\n],`,
    `"swiftshader": {`,
    `"renderer": ${JSON.stringify(renderer)},`,
    `"views": [\n${views.map((v) => JSON.stringify(v)).join(",\n")}\n],`,
    `"dots": ${JSON.stringify(dots)}`,
    "}",
    "}",
    "",
  ].join("\n");
  writeFileSync(OUT, text);
  console.log(`wrote ${OUT}`);
} finally {
  rmSync(work, { recursive: true, force: true });
}
