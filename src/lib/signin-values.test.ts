// @ts-ignore bun's types are not a dependency; the runner provides the module
import { describe, expect, test } from "bun:test";
import { missingWithoutSignIn, signInNote } from "./signin-values";

const none = { verticalSpeed: false, squawk: false, airspace: false };

describe("missingWithoutSignIn", () => {
  test("lists all three when signed out with no other source", () => {
    expect(missingWithoutSignIn(false, none)).toEqual(["verticalSpeed", "squawk", "airspace"]);
  });

  test("lists nothing when signed in, even with values absent", () => {
    expect(missingWithoutSignIn(true, none)).toEqual([]);
  });

  test("leaves out values another source supplies", () => {
    expect(missingWithoutSignIn(false, { verticalSpeed: true, squawk: true, airspace: false })).toEqual(["airspace"]);
    expect(missingWithoutSignIn(false, { verticalSpeed: true, squawk: false, airspace: false })).toEqual([
      "squawk",
      "airspace",
    ]);
  });

  test("lists nothing when every value has a source", () => {
    expect(missingWithoutSignIn(false, { verticalSpeed: true, squawk: true, airspace: true })).toEqual([]);
  });
});

describe("signInNote", () => {
  test("counts all three", () => {
    expect(signInNote(["verticalSpeed", "squawk", "airspace"])).toBe("3 values need a Flightradar24 sign-in");
  });

  test("names one or two, so the reader knows which", () => {
    expect(signInNote(["airspace"])).toBe("Airspace needs a Flightradar24 sign-in");
    expect(signInNote(["squawk", "airspace"])).toBe("Squawk and airspace need a Flightradar24 sign-in");
    expect(signInNote(["verticalSpeed", "airspace"])).toBe("Vertical speed and airspace need a Flightradar24 sign-in");
  });

  test("says nothing when no value needs it", () => {
    expect(signInNote([])).toBe("");
  });
});
