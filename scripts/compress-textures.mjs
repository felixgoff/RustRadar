#!/usr/bin/env node
// The map's images as GPU textures ready to use: Basis Universal in KTX2
// files (static/realistic/*.ktx2, static/aircraft/*.ktx2), each with its mip
// levels made, which the app turns into whatever compressed format the GPU
// takes (BC7 on desktops, ASTC on phones and Apple's) in a worker:
// src/lib/ktx2.ts. A quarter of the memory of the images' own pixels, and no
// mipmaps to make as they load.
//
//   node scripts/compress-textures.mjs
//
// From the images in textures/realistic/ (scripts/realistic-textures.py,
// earth-texture.py and night-texture.py make them) and the aircraft's icon
// atlases in textures/aircraft/ (scripts/aircraft/build.mjs). Each texture
// array is one file, its layers in the order the shaders index them
// (src/lib/realistic-textures.ts's lists, read from there). Colours are
// UASTC (near lossless) in sRGB; normal maps, masks and the icons UASTC, as
// data; the two pictures of the whole Earth ETC1S (a fifth of the size, as
// good as their own detail). Uses the Basis Universal encoder loaders.gl
// ships (node_modules/@loaders.gl/textures/dist/libs), and Pillow (python3)
// to read the images.

import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { cpus } from "node:os";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { isMainThread, parentPort, Worker, workerData } from "node:worker_threads";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const LIBS = `${ROOT}node_modules/@loaders.gl/textures/dist/libs/`;

/** An image's pixels, RGBA, through Pillow. */
function pixels(path) {
  const code = "import sys; from PIL import Image; im = Image.open(sys.argv[1]).convert('RGBA'); sys.stdout.write(f'{im.width} {im.height}\\n'); sys.stdout.flush(); sys.stdout.buffer.write(im.tobytes())";
  const out = spawnSync("python3", ["-I", "-c", code, path], { maxBuffer: 1 << 30 });
  if (out.status !== 0) throw new Error(`${path}: ${out.stderr}`);
  const nl = out.stdout.indexOf(10);
  const [width, height] = out.stdout.subarray(0, nl).toString().split(" ").map(Number);
  return { width, height, data: new Uint8Array(out.stdout.subarray(nl + 1)) };
}

/** The encoder's settings for each kind of image. */
const KINDS = {
  color: { uastc: true, srgb: true },
  normal: { uastc: true, normal: true },
  data: { uastc: true },
  earth: { uastc: false, srgb: true },
};

async function encoder() {
  // (a script, not a module: its package says its .js files are modules)
  const source = readFileSync(`${LIBS}basis_encoder.js`, "utf8");
  const BASIS = new Function("require", "__dirname", `${source}\nreturn BASIS;`)(createRequire(import.meta.url), LIBS);
  const module = await BASIS({ wasmBinary: readFileSync(`${LIBS}basis_encoder.wasm`) });
  module.initializeBasis();
  return module;
}

/** Equally sized images as one KTX2 file (an array when more than one), with all its mip levels. */
function encode(module, images, kind) {
  const o = KINDS[kind];
  const e = new module.BasisEncoder();
  try {
    e.setCreateKTX2File(true);
    if (images.length > 1) e.setTexType(module.basis_texture_type.cBASISTexType2DArray.value);
    images.forEach((image, i) => e.setSliceSourceImage(i, image.data, image.width, image.height, false));
    e.setUASTC(o.uastc);
    if (o.normal) {
      e.setNormalMap();
      e.setMipRenormalize(true);
    } else {
      e.setPerceptual(Boolean(o.srgb));
      e.setMipSRGB(Boolean(o.srgb));
    }
    e.setKTX2SRGBTransferFunc(Boolean(o.srgb));
    e.setMipGen(true);
    e.setMipSmallestDimension(1);
    if (o.uastc) {
      e.setPackUASTCFlags(2);
      e.setKTX2UASTCSupercompression(true);
    } else {
      e.setQualityLevel(255);
      e.setCompressionLevel(2);
    }
    const { width, height } = images[0];
    const out = new Uint8Array(images.length * width * height * 4 * 2 + (1 << 20));
    const n = e.encode(out);
    if (!n) throw new Error("the encoder made nothing");
    return out.slice(0, n);
  } finally {
    e.delete();
  }
}

/** The names a TypeScript `export const NAME = [...] as const` lists, in order. */
function listIn(source, name) {
  const m = source.match(new RegExp(`export const ${name} = \\[([^\\]]*)\\]`));
  if (!m) throw new Error(`no ${name} in realistic-textures.ts`);
  return [...m[1].matchAll(/"([^"]+)"/g)].map((x) => x[1]);
}

/** Every file to make: its path, its images and their kind. */
function jobs() {
  const ts = readFileSync(`${ROOT}src/lib/realistic-textures.ts`, "utf8");
  const src = (name) => `${ROOT}textures/realistic/${name}`;
  const out = (name) => `${ROOT}static/realistic/${name}.ktx2`;
  const set = (list, kind, as) => ({ out: out(`${list.toLowerCase().replace("_textures", "")}.${kind}`), images: listIn(ts, list).map((n) => src(`${n}.${kind}.webp`)), kind: as });
  return [
    set("BUILDING_TEXTURES", "color", "color"),
    set("BUILDING_TEXTURES", "normal", "normal"),
    set("BUILDING_TEXTURES", "mask", "data"),
    set("BUILDING_TEXTURES", "glow", "data"),
    set("GROUND_TEXTURES", "color", "color"),
    set("GROUND_TEXTURES", "normal", "normal"),
    set("TREE_TEXTURES", "color", "color"),
    set("TREE_TEXTURES", "normal", "normal"),
    set("TREE_TEXTURES", "mask", "data"),
    { out: out("water.normal"), images: [src("water.normal.webp")], kind: "normal" },
    { out: out("tree-volume.normal"), images: [src("tree-volume.normal.webp")], kind: "normal" },
    { out: out("earth.color"), images: [src("earth.color.webp")], kind: "earth" },
    { out: out("earth.night"), images: [src("earth.night.webp")], kind: "earth" },
    { out: `${ROOT}static/aircraft/icons.ktx2`, images: [`${ROOT}textures/aircraft/icons.png`], kind: "data" },
    { out: `${ROOT}static/aircraft/icons-outline.ktx2`, images: [`${ROOT}textures/aircraft/icons-outline.png`], kind: "data" },
  ];
}

if (isMainThread) {
  const todo = jobs();
  const threads = Math.max(1, Math.min(todo.length, cpus().length));
  let next = 0;
  let failed = false;
  await Promise.all(
    Array.from({ length: threads }, async () => {
      while (next < todo.length) {
        const job = todo[next++];
        const started = performance.now();
        const result = await new Promise((resolve, reject) => {
          const worker = new Worker(fileURLToPath(import.meta.url), { workerData: job });
          worker.once("message", resolve);
          worker.once("error", reject);
        }).catch((error) => ({ error: String(error) }));
        if (result.error) {
          failed = true;
          console.error(`${job.out}: ${result.error}`);
          continue;
        }
        console.log(`${job.out.slice(ROOT.length)}: ${job.images.length} image(s), ${(result.bytes / 1024).toFixed(0)} KiB, ${((performance.now() - started) / 1000).toFixed(1)} s`);
      }
    }),
  );
  if (failed) process.exit(1);
} else {
  const module = await encoder();
  const images = workerData.images.map(pixels);
  const sizes = new Set(images.map((i) => `${i.width}x${i.height}`));
  if (sizes.size > 1) throw new Error(`images of different sizes: ${[...sizes].join(", ")}`);
  const bytes = encode(module, images, workerData.kind);
  writeFileSync(workerData.out, bytes);
  parentPort.postMessage({ bytes: bytes.length });
}
