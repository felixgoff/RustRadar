/** Flight values Flightradar24 sends only to signed-in users, in the order the flight panel lists them. */
export const SIGN_IN_VALUES = ["verticalSpeed", "squawk", "airspace"] as const;
export type SignInValue = (typeof SIGN_IN_VALUES)[number];

/**
 * The sign-in-only values the panel has nothing to show for: none when signed
 * in, and none that another source already supplies (the aircraft's own reports
 * via adsb.lol, or Flightradar24 sending one anyway).
 */
export function missingWithoutSignIn(authenticated: boolean, have: Record<SignInValue, boolean>): SignInValue[] {
  return authenticated ? [] : SIGN_IN_VALUES.filter((v) => !have[v]);
}

const NAMES: Record<SignInValue, string> = { verticalSpeed: "vertical speed", squawk: "squawk", airspace: "airspace" };

/**
 * "Airspace needs a Flightradar24 sign-in", "Squawk and airspace need …":
 * one or two values by name, so nobody has to go looking for them; all three
 * counted, "3 values need a Flightradar24 sign-in". Empty when none do.
 */
export function signInNote(missing: readonly SignInValue[]): string {
  if (!missing.length) return "";
  if (missing.length >= SIGN_IN_VALUES.length) return `${missing.length} values need a Flightradar24 sign-in`;
  const names = missing.map((v) => NAMES[v]).join(" and ");
  return `${names.charAt(0).toUpperCase()}${names.slice(1)} ${missing.length === 1 ? "needs" : "need"} a Flightradar24 sign-in`;
}
