// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { EMPTY_KEYS, keysFile, readKeysFile } from "./keys";

const ALL = {
  fr24Email: "me@example.com",
  fr24Password: "pw",
  trafiklabGtfs: "gtfs",
  trafiklabOxyfi: "",
  trafikverket: "tv",
};

describe("keys file", () => {
  test("leaves the login out unless asked", () => {
    const file = JSON.parse(keysFile(ALL, false));
    expect(file.keys).toEqual({ trafiklabGtfs: "gtfs", trafikverket: "tv" });
    expect(JSON.parse(keysFile(ALL, true)).keys.fr24Password).toBe("pw");
  });

  test("round-trips over what's there, keeping what the file lacks", () => {
    const current = { ...EMPTY_KEYS, fr24Email: "keep@example.com", trafikverket: "old" };
    const { keys, count } = readKeysFile(keysFile(ALL, false), current);
    expect(count).toBe(2);
    expect(keys).toEqual({ ...current, trafiklabGtfs: "gtfs", trafikverket: "tv" });
  });

  test("rejects other files", () => {
    expect(() => readKeysFile("not json", EMPTY_KEYS)).toThrow("isn't a RustRadar keys file");
    expect(() => readKeysFile('{"keys":{}}', EMPTY_KEYS)).toThrow("isn't a RustRadar keys file");
    expect(() => readKeysFile('{"format":"rustradar-keys","keys":{"x":"1"}}', EMPTY_KEYS)).toThrow("no keys");
  });
});
