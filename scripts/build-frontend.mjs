import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const dist = join(root, "dist");
const vite = join(root, "node_modules", "vite", "bin", "vite.js");

// Tauri embeds dist at Rust compile time. Removing it first prevents obsolete
// hashed assets from surviving an incremental build.
rmSync(dist, { recursive: true, force: true });
execFileSync(process.execPath, [vite, "build"], { cwd: root, stdio: "inherit" });

const indexPath = join(dist, "index.html");
if (!existsSync(indexPath)) {
  throw new Error("Vite completed without producing dist/index.html");
}
if (!readFileSync(indexPath, "utf8").includes("<title>Sagascript Settings</title>")) {
  throw new Error("dist/index.html does not contain the current Sagascript title");
}

function addTree(hash, path, relative = "") {
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

const hash = createHash("sha256");
addTree(hash, dist);
const frontendHash = hash.digest("hex");
// build.rs watches this ignored file. A changed frontend content hash forces
// Cargo/Tauri to regenerate the embedded asset context even with target/ cached.
writeFileSync(
  join(root, "src-tauri", "build-meta.env"),
  `SAGASCRIPT_FRONTEND_HASH=${frontendHash}\n`,
);
