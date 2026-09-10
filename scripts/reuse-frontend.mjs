import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { validateFrontend } from "./build-frontend.mjs";

const root = dirname(dirname(fileURLToPath(import.meta.url)));

try {
  const frontendHash = validateFrontend({
    distPath: join(root, "dist"),
    metadataPath: join(root, "src-tauri", "build-meta.env"),
  });
  console.log(`Reusing verified frontend output (${frontendHash})`);
} catch (error) {
  console.error(`Cannot reuse frontend output: ${error.message}`);
  process.exitCode = 1;
}
