// API keys and logins from the keys page, and the file they are shared in.
// The app keeps them in its config folder (keys.rs); in a plain browser, in
// localStorage.

import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "./api";

export interface Keys {
  fr24Email: string;
  fr24Password: string;
  trafiklabGtfs: string;
  trafiklabOxyfi: string;
  trafikverket: string;
}

export const EMPTY_KEYS: Keys = { fr24Email: "", fr24Password: "", trafiklabGtfs: "", trafiklabOxyfi: "", trafikverket: "" };

const FIELDS = Object.keys(EMPTY_KEYS) as (keyof Keys)[];
const FORMAT = "rustradar-keys";
const STORAGE = "rustradar.keys";

/** The shareable file. Without `withLogin`, the Flightradar24 login stays out of it. */
export function keysFile(keys: Keys, withLogin: boolean): string {
  const out: Partial<Keys> = {};
  for (const f of FIELDS) {
    if (!withLogin && (f === "fr24Email" || f === "fr24Password")) continue;
    if (keys[f]) out[f] = keys[f];
  }
  return JSON.stringify({ format: FORMAT, version: 1, keys: out }, null, 2) + "\n";
}

/**
 * Reads a keys file over `current`: what the file has replaces what's there,
 * what it leaves out stays. Throws a readable message for anything else.
 */
export function readKeysFile(text: string, current: Keys): { keys: Keys; count: number } {
  let json: unknown;
  try {
    json = JSON.parse(text);
  } catch {
    throw new Error("That file isn't a RustRadar keys file.");
  }
  const file = json as { format?: unknown; keys?: unknown };
  if (file?.format !== FORMAT || typeof file.keys !== "object" || file.keys === null) {
    throw new Error("That file isn't a RustRadar keys file.");
  }
  const given = file.keys as Record<string, unknown>;
  const keys = { ...current };
  let count = 0;
  for (const f of FIELDS) {
    const v = given[f];
    if (typeof v === "string" && v.trim()) {
      keys[f] = v;
      count++;
    }
  }
  if (count === 0) throw new Error("That file has no keys in it.");
  return { keys, count };
}

export async function loadKeys(): Promise<Keys> {
  if (isTauri()) return { ...EMPTY_KEYS, ...(await invoke<Keys>("get_keys")) };
  try {
    return { ...EMPTY_KEYS, ...JSON.parse(localStorage.getItem(STORAGE) ?? "{}") };
  } catch {
    return { ...EMPTY_KEYS };
  }
}

/** Saves the keys and returns them as stored (trimmed). */
export async function saveKeys(keys: Keys): Promise<Keys> {
  if (isTauri()) return invoke<Keys>("save_keys", { keys });
  try {
    localStorage.setItem(STORAGE, JSON.stringify(keys));
  } catch {
    // kept for this session only
  }
  return keys;
}
