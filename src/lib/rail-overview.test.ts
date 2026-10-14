// Unit tests for the zoomed-out rail network. Run with `bun test src`.
// @ts-ignore -- bun's types aren't a dependency; svelte-check still type-checks this file
import * as bunTest from "bun:test";
// @ts-ignore -- node's types aren't a dependency either
import { readFileSync } from "node:fs";
import { parseRailOverview } from "./rail-overview";

interface Matchers {
  toBe(value: unknown): void;
  toBeGreaterThan(value: number): void;
  toThrow(): void;
}
const { test, expect } = bunTest as unknown as {
  test(name: string, fn: () => void): void;
  expect(value: unknown): Matchers;
};

const file = readFileSync(new URL("../../static/rail/railroads.bin", import.meta.url));
const buffer = file.buffer.slice(file.byteOffset, file.byteOffset + file.byteLength) as ArrayBuffer;

test("the shipped overview parses, and covers Sweden", () => {
  const rail = parseRailOverview(buffer);
  expect(rail.length).toBeGreaterThan(20_000);
  expect(rail.startIndices[rail.length]).toBe(rail.positions.length / 2);
  let inSweden = 0;
  for (let i = 0; i < rail.positions.length; i += 2) {
    const [lon, lat] = [rail.positions[i], rail.positions[i + 1]];
    if (lon > 12 && lon < 22 && lat > 56 && lat < 67) inSweden++;
  }
  expect(inSweden).toBeGreaterThan(1_000);
});

test("anything that isn't the file is refused", () => {
  expect(() => parseRailOverview(new ArrayBuffer(16))).toThrow();
  expect(() => parseRailOverview(buffer.slice(0, buffer.byteLength - 4))).toThrow();
});
