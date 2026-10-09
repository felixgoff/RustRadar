// Run with `bun test src`.
// @ts-ignore -- bun's types aren't a dependency; svelte-check still type-checks this file
import * as bunTest from "bun:test";
import type { AdsbAircraft } from "./api";
import { matchAdsb } from "./adsb";

const { test, expect } = bunTest as unknown as {
  test(name: string, fn: () => void): void;
  expect(value: unknown): { toBe(value: unknown): void };
};

const ac = (over: Partial<AdsbAircraft>): AdsbAircraft => ({
  hex: "89617e", callsign: "UAE1CL", reg: "A6-EGF", typecode: "B77W", source: "adsb_icao", mlat: false,
  lat: 50.25, lon: 7.59, seenMs: 0, onGround: false, ...over,
});

test("matches by address first", () => {
  const list = [ac({}), ac({ hex: "3c6444", callsign: "DLH1" })];
  expect(matchAdsb(list, { hex: "3C6444", callsign: "UAE1CL", reg: "", lat: 0, lon: 0 })?.hex).toBe("3c6444");
  expect(matchAdsb(list, { hex: "abcdef", callsign: "UAE1CL", reg: "", lat: 50, lon: 7 })).toBe(null);
});

test("without an address, by callsign or registration near the flight", () => {
  const key = { hex: null, callsign: "uae1cl ", reg: "", lat: 50.3, lon: 7.6 };
  expect(matchAdsb([ac({})], key)?.hex).toBe("89617e");
  // the same callsign far away is another flight
  expect(matchAdsb([ac({ lat: 25, lon: 55 })], key)).toBe(null);
  // a different registration under the callsign is another aircraft
  expect(matchAdsb([ac({})], { ...key, reg: "A6-EGG" })).toBe(null);
  // registration alone will do
  expect(matchAdsb([ac({ callsign: "" })], { ...key, callsign: "", reg: "A6EGF" })?.hex).toBe("89617e");
  // the nearer of two candidates
  const near = ac({ hex: "000001", lat: 50.31, lon: 7.6 });
  expect(matchAdsb([ac({}), near], key)?.hex).toBe("000001");
});
