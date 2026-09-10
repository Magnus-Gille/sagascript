import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const dist = join(root, "dist");
const vite = join(root, "node_modules", "vite", "bin", "vite.js");

export function addTree(hash, path, relative = "") {
  for (const name of readdirSync(path).sort()) {
    const absolute = join(path, name);
    const child = join(relative, name);
    const stat = statSync(absolute);
    if (stat.isDirectory()) {
      addTree(hash, absolute, child);
    } else {
      hash.update(child);
      hash.update(readFileSync(absolute));
    }
  }
}

export function hashFrontendTree(distPath) {
  const hash = createHash("sha256");
  addTree(hash, distPath);
  return hash.digest("hex");
}

export function validateFrontendOutput(distPath) {
  const indexPath = join(distPath, "index.html");
  if (!existsSync(indexPath)) {
    throw new Error("Vite output is missing dist/index.html");
  }
  if (!readFileSync(indexPath, "utf8").includes("<title>Sagascript Settings</title>")) {
    throw new Error("dist/index.html does not contain the current Sagascript title");
  }
}

export function validateFrontend({ distPath, metadataPath }) {
  validateFrontendOutput(distPath);

  if (!existsSync(metadataPath)) {
    throw new Error("Frontend metadata is missing src-tauri/build-meta.env");
  }
  const metadata = readFileSync(metadataPath, "utf8");
  const match = /^SAGASCRIPT_FRONTEND_HASH=([0-9a-f]{64})\n?$/.exec(metadata);
  if (!match) {
    throw new Error("Frontend metadata contains an invalid SAGASCRIPT_FRONTEND_HASH");
  }

  const actualHash = hashFrontendTree(distPath);
  if (actualHash !== match[1]) {
    throw new Error(
      `Frontend output hash ${actualHash} does not match build metadata ${match[1]}`,
    );
  }
  return actualHash;
}

function buildFrontend() {
  // Tauri embeds dist at Rust compile time. Removing it first prevents obsolete
  // hashed assets from surviving an incremental build.
  rmSync(dist, { recursive: true, force: true });
  execFileSync(process.execPath, [vite, "build"], { cwd: root, stdio: "inherit" });

  validateFrontendOutput(dist);
  const frontendHash = hashFrontendTree(dist);
  // build.rs watches this ignored file. A changed frontend content hash forces
  // Cargo/Tauri to regenerate the embedded asset context even with target/ cached.
  writeFileSync(
    join(root, "src-tauri", "build-meta.env"),
    `SAGASCRIPT_FRONTEND_HASH=${frontendHash}\n`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  buildFrontend();
}
