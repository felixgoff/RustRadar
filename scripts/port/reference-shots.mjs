#!/usr/bin/env node
// Reference pictures of today's web app for the native port (plan item S1.33,
// docs/port/00-architecture-and-plan.md). Opens the Svelte app in Playwright's
// Chromium at 1440×900 and 390×844 for the twelve views listed in VIEWS and
// saves PNGs to docs/port/reference/. The view list and what the pictures show
// are in docs/port/reference/README.md.
//
//   node scripts/port/reference-shots.mjs [--url http://127.0.0.1:1421]
//        [--only dark-v1,realistic-v5] [--sizes desktop,phone] [--out <dir>]
//        [--time 2026-06-21T10:00:00Z]
//
// Without --url it starts `bunx vite dev --port 1421 --strictPort` and stops it
// at the end: the `#@lat,lon,zoom,pitch,bearing` and `#demo` hashes work only
// in dev builds (`import.meta.env.DEV`), so a production build would not do.
// Without the Rust backend there is no live data: these are references for the
// basemaps and the shell. Needs the Playwright module (global or local) and its
// Chromium, and ImageMagick to shrink the large pictures. GPU work runs on
// SwiftShader, so a view takes from 20 s to 4 min, the whole set half an hour.

import { spawn, execSync } from "node:child_process";
import { createRequire } from "node:module";
import { existsSync, mkdirSync, renameSync, statSync, unlinkSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

/** The named views of the plan's section 8 (`lat, lon, zoom, pitch, bearing`). */
const NAMED = {
  v1: [35, 10, 1.9, 0, 0],
  v2: [52, 10, 4.5, 0, 0],
  v3: [59.33, 18.07, 9, 0, 0],
  v4: [59.6519, 17.9186, 13.5, 45, 30],
  v5: [59.3293, 18.0686, 15.5, 60, 20],
  v6: [57.7, 11.93, 13, 0, 0],
  v7: [78.2, 15.6, 6, 0, 0],
  v8: [0, 179.5, 3, 0, 0],
};

/**
 * The twelve views: a named view on a basemap, the mode (the sidebar's focus)
 * stored in the settings, and whether `demo` is added to the hash.
 */
const VIEWS = [
  { basemap: "dark", view: "v1", mode: "aircraft" },
  { basemap: "dark", view: "v2", mode: "aircraft" },
  { basemap: "dark", view: "v3", mode: "trains", demo: true },
  { basemap: "dark", view: "v4", mode: "aircraft" },
  { basemap: "dark", view: "v8", mode: "aircraft" },
  { basemap: "satellite", view: "v2", mode: "aircraft" },
  { basemap: "satellite", view: "v6", mode: "ships" },
  { basemap: "satellite", view: "v7", mode: "aircraft" },
  { basemap: "realistic", view: "v1", mode: "aircraft" },
  { basemap: "realistic", view: "v3", mode: "aircraft" },
  { basemap: "realistic", view: "v4", mode: "aircraft" },
  { basemap: "realistic", view: "v5", mode: "trains", demo: true },
];

const SIZES = {
  desktop: { width: 1440, height: 900, phone: false },
  phone: { width: 390, height: 844, phone: true },
};

/** Midsummer, noon in Stockholm: the sun's place drives day and night and the realistic map's light. */
const DEFAULT_TIME = "2026-06-21T10:00:00Z";
/** A capture larger than this is stored as a 256-colour dithered palette PNG. */
const MAX_EXACT_BYTES = 800_000;
/** Settling: at least this long after loading, the network quiet this long, then this many more frames. */
const MIN_WAIT_MS = 15_000;
const QUIET_MS = 5_000;
const FRAMES_AFTER_QUIET = 4;
/** Quiet this long without new frames also counts as settled (nothing is being redrawn). */
const STILL_MS = 20_000;
const MAX_WAIT_MS = 150_000;
const SHOT_TIMEOUT_MS = 300_000;

const hashOf = ({ view, demo }) => `#@${NAMED[view].join(",")}${demo ? "&demo" : ""}`;
const fileOf = ({ basemap, view }, { width, height }) => `${basemap}-${view}-${width}x${height}.png`;

function args() {
  const out = { url: null, only: null, sizes: Object.keys(SIZES), dir: join(ROOT, "docs/port/reference"), time: DEFAULT_TIME };
  const argv = process.argv.slice(2);
  for (let i = 0; i < argv.length; i++) {
    const value = () => {
      if (i + 1 >= argv.length) throw new Error(`${argv[i]} needs a value`);
      return argv[++i];
    };
    switch (argv[i]) {
      case "--url": out.url = value().replace(/\/+$/, ""); break;
      case "--only": out.only = value().split(","); break;
      case "--sizes": out.sizes = value().split(","); break;
      case "--out": out.dir = resolve(value()); break;
      case "--time": out.time = value(); break;
      default: throw new Error(`unknown argument ${argv[i]}`);
    }
  }
  for (const s of out.sizes) if (!SIZES[s]) throw new Error(`unknown size ${s} (desktop, phone)`);
  if (Number.isNaN(Date.parse(out.time))) throw new Error(`unreadable --time ${out.time}`);
  return out;
}

async function loadPlaywright() {
  try {
    return await import("playwright");
  } catch {
    const require = createRequire(import.meta.url);
    return require(join(execSync("npm root -g").toString().trim(), "playwright"));
  }
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** The page holds no map: an error page from the dev server. Worth another try. */
class NotMounted extends Error {
  constructor() {
    super("the app did not mount (no map canvas in <main>)");
  }
}

async function waitForHttp(url, ms) {
  const until = Date.now() + ms;
  while (Date.now() < until) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      // not up yet
    }
    await sleep(1000);
  }
  throw new Error(`${url} did not answer within ${ms / 1000} s`);
}

/** `bunx vite dev` in its own process group, so that stopping it stops vite and its helpers and nothing else. */
async function startDevServer() {
  const url = "http://127.0.0.1:1421";
  const child = spawn("bunx", ["vite", "dev", "--port", "1421", "--strictPort"], {
    cwd: ROOT,
    detached: true,
    stdio: ["ignore", "ignore", "inherit"],
  });
  const stop = () => {
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      // already gone
    }
  };
  try {
    await waitForHttp(url + "/", 120_000);
  } catch (e) {
    stop();
    throw e;
  }
  return { url, stop };
}

/** Runs in the page before its own scripts. */
function prepare({ settings, startMs }) {
  // the settings the app loads (filters.ts, `rustradar.v1`)
  try {
    if (location.protocol.startsWith("http")) localStorage.setItem("rustradar.v1", JSON.stringify({ settings }));
  } catch {
    // about:blank has no storage
  }
  // a clock that runs from `startMs`: the same sun on every run
  const Real = Date;
  const delta = startMs - Real.now();
  function Shifted(...a) {
    if (!new.target) return new Real(Real.now() + delta).toString();
    return a.length === 0 ? new Real(Real.now() + delta) : new Real(...a);
  }
  Shifted.prototype = Real.prototype;
  Shifted.now = () => Real.now() + delta;
  Shifted.parse = Real.parse;
  Shifted.UTC = Real.UTC;
  globalThis.Date = Shifted;
  // frames counted, and parked once `__freeze` is set: SwiftShader draws a frame
  // in one to three seconds, and a capture waits behind every queued frame
  const raf = window.requestAnimationFrame.bind(window);
  const caf = window.cancelAnimationFrame.bind(window);
  window.__frames = 0;
  window.__freeze = false;
  window.requestAnimationFrame = (cb) =>
    window.__freeze
      ? -1
      : raf((t) => {
          window.__frames++;
          cb(t);
        });
  window.cancelAnimationFrame = (id) => {
    if (id !== -1) caf(id);
  };
}

async function shoot(browser, base, shot, size, opt) {
  const context = await browser.newContext({
    viewport: { width: size.width, height: size.height },
    deviceScaleFactor: 1,
    isMobile: size.phone,
    hasTouch: size.phone,
    locale: "en-US",
    timezoneId: "Europe/Stockholm",
  });
  try {
    // the EUMETSAT cloud picture would hide the map
    await context.route("https://view.eumetsat.int/**", (r) => r.abort());
    // `sky: sunny`: the realistic map's weather from a preset, not from today's forecast
    const settings = { mode: shot.mode, basemap: shot.basemap, sky: "sunny" };
    await context.addInitScript(prepare, { settings, startMs: Date.parse(opt.time) });

    const inflight = new Set();
    let activity = Date.now();
    const begin = (r) => {
      inflight.add(r);
      activity = Date.now();
    };
    const end = (r) => {
      inflight.delete(r);
      activity = Date.now();
    };
    context.on("request", begin);
    context.on("requestfinished", end);
    context.on("requestfailed", end);

    const page = await context.newPage();
    const frames = () => page.evaluate(() => window.__frames ?? 0);
    const start = Date.now();
    await page.goto(base + "/" + hashOf(shot), { waitUntil: "load", timeout: 120_000 });
    // the dev server sometimes fails a module on first load and serves its error page instead of the app
    const mounted = () => page.evaluate(() => !!document.querySelector("main canvas"));
    try {
      await page.waitForSelector("main canvas", { timeout: 60_000 });
    } catch {
      throw new NotMounted();
    }

    let settled = false;
    let quietFrom = null;
    let quietFrames = 0;
    while (Date.now() - start < MAX_WAIT_MS) {
      await sleep(1000);
      const now = Date.now();
      const quiet = now - start >= MIN_WAIT_MS && inflight.size === 0 && now - activity >= QUIET_MS;
      if (!quiet) {
        quietFrom = null;
        continue;
      }
      const f = await frames();
      if (quietFrom === null) [quietFrom, quietFrames] = [now, f];
      if (f - quietFrames >= FRAMES_AFTER_QUIET || now - quietFrom >= STILL_MS) {
        settled = true;
        break;
      }
    }
    const loadS = (Date.now() - start) / 1000;

    // stop drawing, then let the frames already queued finish
    await page.evaluate(() => (window.__freeze = true));
    let last = -1;
    for (let i = 0; i < 120; i++) {
      const f = await frames();
      if (f === last) break;
      last = f;
      await sleep(500);
    }
    await sleep(2000);
    if (!(await mounted())) throw new NotMounted();
    const path = join(opt.dir, fileOf(shot, size));
    await page.screenshot({ path, animations: "disabled", timeout: SHOT_TIMEOUT_MS });
    const exact = statSync(path).size <= MAX_EXACT_BYTES || !shrink(path);
    console.log(
      `${fileOf(shot, size)}  ${hashOf(shot)}  ${settled ? "settled" : "NOT SETTLED"} after ${loadS.toFixed(0)} s, ` +
        `${((Date.now() - start) / 1000).toFixed(0)} s in all, ${(statSync(path).size / 1000).toFixed(0)} kB` +
        (exact ? "" : " (256 colours)"),
    );
  } finally {
    await context.close();
  }
}

/** A 256-colour dithered palette PNG in place of `path`, with ImageMagick. False when that failed. */
function shrink(path) {
  const tmp = path + ".tmp.png";
  for (const tool of ["magick", "convert"]) {
    try {
      execSync(`${tool} "${path}" -dither FloydSteinberg -colors 256 "PNG8:${tmp}"`, { stdio: "ignore" });
      renameSync(tmp, path);
      return true;
    } catch {
      if (existsSync(tmp)) unlinkSync(tmp);
    }
  }
  console.warn(`  kept ${path} in full colour: neither magick nor convert worked`);
  return false;
}

async function main() {
  const opt = args();
  const shots = VIEWS.filter((v) => !opt.only || opt.only.includes(`${v.basemap}-${v.view}`));
  if (!shots.length) throw new Error("--only matched no view");
  mkdirSync(opt.dir, { recursive: true });

  const server = opt.url ? { url: opt.url, stop() {} } : await startDevServer();
  const stopOnSignal = () => {
    server.stop();
    process.exit(130);
  };
  process.on("SIGINT", stopOnSignal);
  process.on("SIGTERM", stopOnSignal);
  let browser;
  try {
    const { chromium } = await loadPlaywright();
    const proxy = process.env.HTTPS_PROXY || process.env.https_proxy;
    browser = await chromium.launch({
      args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"],
      ...(proxy ? { proxy: { server: proxy, bypass: "localhost,127.0.0.1" } } : {}),
    });
    for (const shot of shots) {
      for (const s of opt.sizes) {
        for (let attempt = 1; ; attempt++) {
          try {
            await shoot(browser, server.url, shot, SIZES[s], opt);
            break;
          } catch (e) {
            if (!(e instanceof NotMounted) || attempt === 3) throw e;
            console.warn(`  ${fileOf(shot, SIZES[s])}: the app did not mount, again (${attempt + 1} of 3)`);
          }
        }
      }
    }
  } finally {
    await browser?.close();
    server.stop();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
