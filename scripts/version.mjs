// The app's version lives in four files that have to agree (CI builds with
// --locked, and the updater compares tauri.conf.json's against the latest
// release). This reads and changes all of them at once.
//
//   bun run bump            show the version
//   bun run bump patch      0.1.22 -> 0.1.23   (or minor, major)
//   bun run bump 0.2.0      set it exactly
//   bun run bump --check    fail unless all four agree (used by CI)
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const path = (file) => join(root, file);

// Each place the version appears: where it is, how to read it, how to replace it.
const files = [
  {
    file: "package.json",
    pattern: /^(\s*"version":\s*")([^"]+)(")/m,
  },
  {
    file: "src-tauri/tauri.conf.json",
    pattern: /^(\s*"version":\s*")([^"]+)(")/m,
  },
  {
    file: "src-tauri/Cargo.toml",
    // the [package] table's version, the first one in the file
    pattern: /^(version\s*=\s*")([^"]+)(")/m,
  },
  {
    file: "Cargo.lock",
    // only this app's entry, not a dependency that happens to share a number
    pattern: /(name = "rustradar"\r?\nversion = ")([^"]+)(")/,
  },
];

const found = files.map((f) => {
  const text = readFileSync(path(f.file), "utf8");
  const match = text.match(f.pattern);
  if (!match) throw new Error(`no version found in ${f.file}`);
  return { ...f, text, version: match[2] };
});

const [current] = found.map((f) => f.version);
const agree = found.every((f) => f.version === current);
const arg = process.argv[2];

if (arg === "--check" || arg === undefined) {
  for (const f of found) console.log(`${f.version.padEnd(10)} ${f.file}`);
  if (!agree) {
    console.error("\nThe versions disagree. Run `bun run bump <version>` to make them match.");
    process.exit(1);
  }
  if (arg === undefined) console.log(`\nVersion ${current}. Run \`bun run bump patch\` to ship the next one.`);
  process.exit(0);
}

if (!agree) {
  console.error("The versions disagree; fix that first with an exact version: `bun run bump <version>`.");
  process.exit(1);
}

const next = (() => {
  if (/^\d+\.\d+\.\d+$/.test(arg)) return arg;
  const [major, minor, patch] = current.split(".").map(Number);
  if (arg === "major") return `${major + 1}.0.0`;
  if (arg === "minor") return `${major}.${minor + 1}.0`;
  if (arg === "patch") return `${major}.${minor}.${patch + 1}`;
  console.error(`Unknown version "${arg}". Use patch, minor, major or an exact x.y.z.`);
  process.exit(1);
})();

for (const f of found) {
  writeFileSync(path(f.file), f.text.replace(f.pattern, `$1${next}$3`));
}
console.log(`${current} -> ${next}`);
console.log("Updated package.json, tauri.conf.json, Cargo.toml and Cargo.lock.");
console.log(`Commit and merge to main to publish v${next}; installed apps update on their next launch.`);
