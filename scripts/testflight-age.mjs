#!/usr/bin/env node
// How many days ago the app's newest build reached App Store Connect (and so
// TestFlight, where a build expires after 90 days), from Apple's App Store
// Connect API with the same API key the iOS job uploads with:
//
//   APPLE_API_ISSUER=… APPLE_API_KEY_ID=… APPLE_API_KEY_PATH=AuthKey.p8 node scripts/testflight-age.mjs
//
// Prints the age in days (a very large number when there is no build yet) and,
// in GitHub Actions, sets the step's `days` and `version` outputs.
import { sign } from "node:crypto";
import { appendFileSync, readFileSync } from "node:fs";

const { APPLE_API_ISSUER: issuer, APPLE_API_KEY_ID: keyId, APPLE_API_KEY_PATH: keyPath, GITHUB_OUTPUT } = process.env;
if (!issuer || !keyId || !keyPath) {
  console.error("needs APPLE_API_ISSUER, APPLE_API_KEY_ID and APPLE_API_KEY_PATH");
  process.exit(2);
}
const bundleId = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8")).identifier;

/** A token for the API: ES256, good for 15 minutes. */
function token() {
  const b64 = (v) => Buffer.from(typeof v === "string" ? v : JSON.stringify(v)).toString("base64url");
  const now = Math.floor(Date.now() / 1000);
  const body = `${b64({ alg: "ES256", kid: keyId, typ: "JWT" })}.${b64({ iss: issuer, iat: now, exp: now + 900, aud: "appstoreconnect-v1" })}`;
  const signature = sign("sha256", Buffer.from(body), { key: readFileSync(keyPath, "utf8"), dsaEncoding: "ieee-p1363" });
  return `${body}.${signature.toString("base64url")}`;
}

const auth = { Authorization: `Bearer ${token()}` };
async function get(path) {
  const r = await fetch(`https://api.appstoreconnect.apple.com${path}`, { headers: auth });
  if (!r.ok) throw new Error(`App Store Connect answered ${r.status} for ${path}: ${await r.text()}`);
  return r.json();
}

const apps = await get(`/v1/apps?filter[bundleId]=${encodeURIComponent(bundleId)}&limit=1`);
const app = apps.data?.[0];
if (!app) throw new Error(`no app in App Store Connect with the bundle id ${bundleId}`);
const builds = await get(`/v1/builds?filter[app]=${app.id}&sort=-uploadedDate&limit=1&fields[builds]=uploadedDate,version`);
const newest = builds.data?.[0]?.attributes;
const days = newest?.uploadedDate ? (Date.now() - Date.parse(newest.uploadedDate)) / 86_400_000 : 1e6;
console.log(newest ? `newest build ${newest.version}, uploaded ${newest.uploadedDate}: ${days.toFixed(1)} days ago` : "no build uploaded yet");
if (GITHUB_OUTPUT) appendFileSync(GITHUB_OUTPUT, `days=${Math.floor(days)}\nversion=${newest?.version ?? ""}\n`);
