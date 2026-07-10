import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const json = (path) => JSON.parse(readFileSync(join(root, path), "utf8"));

const packageJson = json("package.json");
const packageLock = json("package-lock.json");
const tauri = json("src-tauri/tauri.conf.json");
const cargo = readFileSync(join(root, "src-tauri", "Cargo.toml"), "utf8");
const cargoVersion = cargo.match(/\[workspace\.package\][\s\S]*?\nversion\s*=\s*"([^"]+)"/)?.[1];

const versions = new Map([
  ["package.json", packageJson.version],
  ["package-lock.json", packageLock.version],
  ["package-lock root package", packageLock.packages?.[""]?.version],
  ["tauri.conf.json", tauri.version],
  ["Cargo workspace", cargoVersion],
]);
const expected = packageJson.version;
const mismatches = [...versions].filter(([, version]) => version !== expected);
if (mismatches.length) {
  throw new Error(
    `Release versions differ from ${expected}: ${mismatches.map(([file, version]) => `${file}=${version}`).join(", ")}`,
  );
}

const tag = process.env.RELEASE_TAG;
if (tag && tag !== `v${expected}`) {
  throw new Error(`Tag ${tag} does not match application version v${expected}`);
}
if (tauri.identifier.endsWith(".app")) {
  throw new Error(`Bundle identifier must not end in .app: ${tauri.identifier}`);
}

const store = readFileSync(
  join(root, "src-tauri", "crates", "sagascript-core", "src", "settings", "store.rs"),
  "utf8",
);
if (!store.includes(`const APP_IDENTIFIER: &str = "${tauri.identifier}";`)) {
  throw new Error("Tauri and settings-store application identifiers differ");
}
if (!readFileSync(join(root, "index.html"), "utf8").includes("<title>Sagascript Settings</title>")) {
  throw new Error("index.html still carries a stale application title");
}

console.log(`Release metadata is consistent: Sagascript ${expected} (${tauri.identifier})`);
