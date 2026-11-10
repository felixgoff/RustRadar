#!/usr/bin/env node
// Zone abbreviations for the Rust port of `time.ts` (plan item S1.11,
// docs/port/00-architecture-and-plan.md; doc 05 §6.3). Run from anywhere, with Node 22.13 or
// newer (it strips the TypeScript's types itself) and cargo on the PATH:
//
//   node scripts/port/zone-names.mjs
//
// It writes crates/core/src/time/zone_names.rs: today's `zoneName` run over every time zone of
// the tz database the Rust side bundles (jiff-tzdb, found through `cargo metadata`) for a
// January and a July instant, as rows `(zone, offset minutes east, name)`. Node's ICU stands in
// for the webview's (WebView2 and WebKit already disagree with each other today).
//
// The January and July of two years are read, 2026 and 2027: Morocco keeps UTC+0 from October
// 2026 on (WET), which neither instant of 2026 sees, and British Columbia and Alberta keep summer
// time from November 2026 on, so the winter offsets (PST, MST) are 2026's alone. Each instant
// gets a fresh copy of the module (`zoneName` caches a name per zone and offset), and a zone that
// reads two names at one offset stops the script: the table holds one.
//
// Rows the Rust module would answer the same way without the table are left out, so the table
// holds only what the locale data adds:
//   - zones `SAME_CLOCK` maps to another zone: Rust reads the other zone's rows, as `zoneName`
//     names them by it (checked here to give the same name at the same offset);
//   - names that `COMMON_NAMES[zone][offset] ?? utcLabel(offset)` gives anyway.
// Zones ICU does not know are left out too: `zoneName` would answer with the viewer's own zone.
//
// The output depends only on the TypeScript, Node's ICU and the zone list (no clock, no
// viewer zone), so two runs with the same Node are byte-identical.

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const OUT = join(ROOT, "crates/core/src/time/zone_names.rs");

/** The instants every zone is named at: mid-January and mid-July of two years, at noon UTC. */
const INSTANTS = ["2026-01-15T12:00:00Z", "2026-07-15T12:00:00Z", "2027-01-15T12:00:00Z", "2027-07-15T12:00:00Z"];

// today's time.ts, unchanged, with the internals the filter needs exported at the end; one
// module per instant, so no name cached at one instant answers for another
const source = readFileSync(join(ROOT, "src/lib/time.ts"), "utf8");
const js = stripTypeScriptTypes(`${source}\nexport { utcOffset, utcLabel, SAME_CLOCK, COMMON_NAMES };\n`);
const load = (iso) => import(`data:text/javascript;base64,${Buffer.from(`${js}\n// ${iso}\n`).toString("base64")}`);

// every zone name of the bundled database, links included (`Asia/Calcutta`, `Europe/Belfast`)
const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--format-version", "1", "--manifest-path", join(ROOT, "Cargo.toml")], {
    encoding: "utf8",
    maxBuffer: 256 << 20,
  }),
);
const tzdb = metadata.packages.find((p) => p.name === "jiff-tzdb");
if (!tzdb) throw new Error("jiff-tzdb is not in the workspace's dependency graph");
const tznames = readFileSync(join(dirname(tzdb.manifest_path), "tzname.rs"), "utf8");
const tzdbVersion = /VERSION: Option<&str> = Some\(r"([^"]+)"\)/.exec(tznames)?.[1] ?? "unknown";
const zones = [...tznames.matchAll(/\(r"([^"]+)", \d+\.\.\d+\)/g)].map((m) => m[1]);
if (zones.length < 400) throw new Error(`only ${zones.length} zone names found in ${tzdb.manifest_path}`);

function known(timeZone) {
  try {
    new Intl.DateTimeFormat("en", { timeZone });
    return true;
  } catch {
    return false;
  }
}

const byCodeUnit = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
const modules = await Promise.all(INSTANTS.map(async (iso) => [iso, await load(iso)]));
const rows = new Map(); // `${zone}|${offset}` -> [zone, offset, name]
const unknown = [];
for (const zone of zones) {
  if (!known(zone)) {
    unknown.push(zone);
    continue;
  }
  for (const [iso, time] of modules) {
    const unix = Date.parse(iso) / 1000;
    const offset = time.utcOffset(unix, zone);
    const name = time.zoneName(unix, zone);
    const same = time.SAME_CLOCK[zone];
    if (same) {
      const theirs = time.zoneName(unix, same);
      if (time.utcOffset(unix, same) !== offset || theirs !== name)
        throw new Error(`${zone} keeps ${same}'s clock but reads ${name} at ${offset} against ${theirs}`);
      continue;
    }
    if (name === (time.COMMON_NAMES[zone]?.[offset] ?? time.utcLabel(offset))) continue;
    const key = `${zone}|${offset}`;
    const held = rows.get(key);
    if (held && held[2] !== name) throw new Error(`${zone} at ${offset} reads both ${held[2]} and ${name}`);
    rows.set(key, [zone, offset, name]);
  }
}
const table = [...rows.values()].sort((a, b) => byCodeUnit(a[0], b[0]) || a[1] - b[1]);
const intlOnly = Intl.supportedValuesOf("timeZone").filter((z) => !zones.includes(z));

const rust = (s) => JSON.stringify(s); // zone names and abbreviations are plain ASCII
const lines = [
  "//! Zone abbreviations as today's `zoneName` (`src/lib/time.ts`) gives them, by zone and UTC offset.",
  "//!",
  "//! Generated by `node scripts/port/zone-names.mjs` (plan item S1.11): do not edit by hand.",
  `//! Node ${process.versions.node}, ICU ${process.versions.icu}, ICU time zone data ${process.versions.tz};`,
  `//! the zones of jiff-tzdb ${tzdb.version} (tz database ${tzdbVersion}), each read at the instants below.`,
  "//! Left out: zones that `SAME_CLOCK` maps to another (their clock's rows serve them), names the",
  "//! fallbacks give anyway (`COMMON_NAMES`, then `utc_label`), and zones ICU does not know",
  unknown.length ? `//! (${unknown.join(", ")}).` : "//! (none).",
  "",
  `/// The instants the table was read at (unix ms): ${INSTANTS.join(", ")}.`,
  `pub const INSTANTS_MS: [i64; ${INSTANTS.length}] = [${INSTANTS.map((iso) => Date.parse(iso)).join(", ")}];`,
  "",
  "/// `(zone, offset in minutes east of UTC, name)`, sorted by zone (byte order), then offset.",
  "pub static ZONE_NAMES: &[(&str, i16, &str)] = &[",
  ...table.map(([zone, offset, name]) => `    (${rust(zone)}, ${offset}, ${rust(name)}),`),
  "];",
  "",
];
writeFileSync(OUT, lines.join("\n"));

console.log(`${table.length} rows for ${new Set(table.map((r) => r[0])).size} zones -> ${OUT}`);
if (unknown.length) console.log(`zones of the tz database ICU does not know: ${unknown.join(", ")}`);
if (intlOnly.length) console.log(`zones ICU knows that the tz database lacks: ${intlOnly.join(", ")}`);
