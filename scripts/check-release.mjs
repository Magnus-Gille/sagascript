import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const json = (root, path) => JSON.parse(readFileSync(join(root, path), "utf8"));
const read = (root, path) => readFileSync(join(root, path), "utf8");

function lockPackageVersion(lock, name) {
  const escapedName = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return lock.match(new RegExp(`\\[\\[package\\]\\]\\s*\\nname = "${escapedName}"\\s*\\nversion = "([^"]+)"`))?.[1];
}

export function checkRelease(root = projectRoot) {
  const packageJson = json(root, "package.json");
  const packageLock = json(root, "package-lock.json");
  const tauri = json(root, "src-tauri/tauri.conf.json");
  const cargo = read(root, "src-tauri/Cargo.toml");
  const cargoVersion = cargo.match(/\[workspace\.package\][\s\S]*?\nversion\s*=\s*"([^"]+)"/)?.[1];

  const ortManifest = read(root, "src-tauri/engine-host/ort/Cargo.toml");
  const ortLock = read(root, "src-tauri/engine-host/ort/Cargo.lock");
  const ortManifestVersion = ortManifest.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1];
  const ortVersions = new Map([
    ["engine-host/ort/Cargo.toml", ortManifestVersion],
    ["engine-host/ort/Cargo.lock host package", lockPackageVersion(ortLock, "sagascript-engine-host-ort")],
    ["engine-host/ort/Cargo.lock protocol package", lockPackageVersion(ortLock, "sagascript-engine-protocol")],
  ]);

  const versions = new Map([
    ["package.json", packageJson.version],
    ["package-lock.json", packageLock.version],
    ["package-lock root package", packageLock.packages?.[""]?.version],
    ["tauri.conf.json", tauri.version],
    ["Cargo workspace", cargoVersion],
    ...ortVersions,
  ]);
  const expected = packageJson.version;
  const mismatches = [...versions].filter(([, version]) => version !== expected);
  if (mismatches.length) {
    throw new Error(`Release versions differ from ${expected}: ${mismatches.map(([file, version]) => `${file}=${version}`).join(", ")}`);
  }

  const tag = process.env.RELEASE_TAG;
  if (tag && tag !== `v${expected}`) {
    throw new Error(`Tag ${tag} does not match application version v${expected}`);
  }
  if (tauri.identifier.endsWith(".app")) {
    throw new Error(`Bundle identifier must not end in .app: ${tauri.identifier}`);
  }

  const store = read(root, "src-tauri/crates/sagascript-core/src/settings/store.rs");
  if (!store.includes(`const APP_IDENTIFIER: &str = "${tauri.identifier}";`)) {
    throw new Error("Tauri and settings-store application identifiers differ");
  }
  if (!read(root, "index.html").includes("<title>Sagascript Settings</title>")) {
    throw new Error("index.html still carries a stale application title");
  }

  return `Release metadata is consistent: Sagascript ${expected} (${tauri.identifier})`;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  console.log(checkRelease());
}
