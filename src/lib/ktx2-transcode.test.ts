// @ts-ignore bun's types are not a dependency; the runner provides the module
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { TARGET, transcode, transcoder } from "./ktx2-transcode";

const file = (path: string) => new Uint8Array(readFileSync(new URL(`../../static/${path}`, import.meta.url)));
const basis = () => transcoder(readFileSync(new URL("./vendor/basis_transcoder.wasm", import.meta.url)).buffer as ArrayBuffer);

test("a texture array: every layer, every mip level down to 1×1", async () => {
  const t = transcode(await basis(), file("realistic/tree.mask.ktx2"), TARGET.rgba);
  expect(t.layers).toBe(5);
  expect(t.levels.length).toBe(Math.log2(t.width) + 1);
  expect(t.levels.at(-1)!.width).toBe(1);
  for (const l of t.levels) expect(l.data.length).toBe(l.width * l.height * 4 * t.layers);
});

test("BC7 and ASTC: 16 bytes a 4×4 block", async () => {
  for (const target of [TARGET.bc7, TARGET.astc]) {
    const t = transcode(await basis(), file("realistic/water.normal.ktx2"), target);
    expect(t.layers).toBe(1);
    expect(t.levels[0].data.length).toBe((t.width / 4) * (t.height / 4) * 16);
  }
});

test("the aircraft icons keep their alpha", async () => {
  const t = transcode(await basis(), file("aircraft/icons.ktx2"), TARGET.rgba);
  const alpha = new Set<number>();
  for (let i = 3; i < t.levels[0].data.length; i += 4) alpha.add(t.levels[0].data[i]);
  expect(alpha.has(0) && alpha.has(255)).toBe(true);
});

test("not a KTX2 file (Tauri's page for a missing one)", async () => {
  expect(() => transcode(null as never, new TextEncoder().encode("<!doctype html><html>"), TARGET.rgba)).toThrow("not a KTX2 file");
});
