// Unit tests for clock times and zone names. Run with `bun test src`.
// @ts-ignore -- bun's types aren't a dependency; svelte-check still type-checks this file
import * as bunTest from "bun:test";
import { clockAt, clockWithZone, minutesOfDay, zoneName } from "./time";

interface Matchers {
  toBe(value: unknown): void;
  toContain(value: unknown): void;
}
const { test, expect } = bunTest as unknown as {
  test(name: string, fn: () => void): void;
  expect(value: unknown): Matchers;
};

const at = (iso: string) => Date.parse(iso) / 1000;
const SUMMER = at("2026-07-15T12:25:00Z");
const WINTER = at("2026-01-15T12:25:00Z");

test("clocks are 24-hour", () => {
  expect(clockAt(SUMMER, "Europe/London")).toBe("13:25");
  expect(clockAt(WINTER, "Europe/London")).toBe("12:25");
  expect(clockAt(SUMMER, "America/New_York")).toBe("08:25");
  expect(clockAt(SUMMER, "Asia/Tokyo")).toBe("21:25");
  expect(clockAt(at("2026-07-14T23:05:00Z"), "Europe/London")).toBe("00:05");
});

test("a missing time is a dash", () => {
  expect(clockAt(null, "Europe/London")).toBe("—");
  expect(clockAt(undefined)).toBe("—");
  expect(clockWithZone(0, "Europe/London")).toBe("—");
});

test("minutes of the day agree with the clock", () => {
  expect(minutesOfDay(SUMMER, "Europe/London")).toBe(13 * 60 + 25);
  expect(minutesOfDay(at("2026-07-14T23:05:00Z"), "Europe/London")).toBe(5);
});

test("zones are named by their abbreviation, not an offset", () => {
  expect(zoneName(SUMMER, "Europe/London")).toBe("BST");
  expect(zoneName(WINTER, "Europe/London")).toBe("GMT");
  expect(zoneName(SUMMER, "Europe/Paris")).toBe("CEST");
  expect(zoneName(WINTER, "Europe/Paris")).toBe("CET");
  expect(zoneName(SUMMER, "America/New_York")).toBe("EDT");
  expect(zoneName(WINTER, "America/New_York")).toBe("EST");
  expect(zoneName(SUMMER, "America/Los_Angeles")).toBe("PDT");
  expect(zoneName(SUMMER, "Asia/Tokyo")).toBe("JST");
  expect(zoneName(WINTER, "Asia/Tokyo")).toBe("JST");
  expect(zoneName(WINTER, "Australia/Sydney")).toBe("AEDT");
});

test("a real name beats a locale's plain GMT at zero offset", () => {
  expect(zoneName(WINTER, "Europe/Lisbon")).toBe("WET");
});

test("UTC stays UTC", () => {
  expect(zoneName(SUMMER, "UTC")).toBe("UTC");
  expect(zoneName(SUMMER, "Etc/UTC")).toBe("UTC");
});

test("a zone without an abbreviation in common use falls back to its UTC offset", () => {
  expect(zoneName(SUMMER, "Etc/GMT-14")).toBe("UTC+14");
  expect(zoneName(SUMMER, "Etc/GMT+5")).toBe("UTC−5");
  // CST and AST would each name two different clocks
  expect(zoneName(SUMMER, "Asia/Shanghai")).toBe("UTC+8");
  expect(zoneName(SUMMER, "Asia/Qatar")).toBe("UTC+3");
});

test("hubs that no locale names get their common English name", () => {
  expect(zoneName(SUMMER, "Asia/Seoul")).toBe("KST");
  expect(zoneName(SUMMER, "Europe/Istanbul")).toBe("TRT");
  expect(zoneName(WINTER, "Europe/Moscow")).toBe("MSK");
  expect(zoneName(SUMMER, "Asia/Manila")).toBe("PHT");
  expect(zoneName(SUMMER, "America/Bogota")).toBe("COT");
  expect(zoneName(SUMMER, "America/Lima")).toBe("PET");
  expect(zoneName(SUMMER, "Asia/Kathmandu")).toBe("NPT");
  // Chile keeps summer time in the southern summer
  expect(zoneName(SUMMER, "America/Santiago")).toBe("CLT");
  expect(zoneName(WINTER, "America/Santiago")).toBe("CLST");
});

test("the Crown Dependencies keep London's names", () => {
  expect(zoneName(SUMMER, "Europe/Jersey")).toBe("BST");
  expect(zoneName(SUMMER, "Europe/Guernsey")).toBe("BST");
  expect(zoneName(SUMMER, "Europe/Isle_of_Man")).toBe("BST");
  expect(zoneName(WINTER, "Europe/Jersey")).toBe("GMT");
  expect(clockAt(SUMMER, "Europe/Jersey")).toBe("13:25");
});

test("BST is London's alone: Dhaka keeps its offset", () => {
  expect(zoneName(SUMMER, "Asia/Dhaka")).toBe("UTC+6");
  expect(zoneName(WINTER, "Asia/Dhaka")).toBe("UTC+6");
});

test("old zone names are named like their current ones", () => {
  expect(zoneName(SUMMER, "Asia/Saigon")).toBe("ICT");
  expect(zoneName(SUMMER, "Asia/Istanbul")).toBe("TRT");
});

test("clock with zone", () => {
  expect(clockWithZone(SUMMER, "Europe/London")).toBe("13:25 BST");
  expect(clockWithZone(WINTER, "America/New_York")).toBe("07:25 EST");
  expect(clockWithZone(SUMMER, "Europe/Istanbul")).toBe("15:25 TRT");
  expect(clockWithZone(SUMMER, "Asia/Shanghai")).toBe("20:25 UTC+8");
});

test("an unknown zone falls back to the viewer's own rather than failing", () => {
  expect(clockAt(SUMMER, "Mars/Olympus_Mons")).toBe(clockAt(SUMMER));
  expect(zoneName(SUMMER, "Mars/Olympus_Mons")).toBe(zoneName(SUMMER));
});
