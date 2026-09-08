import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { hashFrontendTree, validateFrontend } from "./build-frontend.mjs";

async function fixture() {
  const root = await mkdtemp(join(tmpdir(), "sagascript-frontend-reuse-"));
  const distPath = join(root, "dist");
  const metadataPath = join(root, "build-meta.env");
  const indexPath = join(distPath, "index.html");
  await mkdir(distPath);
  await writeFile(indexPath, "<title>Sagascript Settings</title>\n");
  const hash = hashFrontendTree(distPath);
  await writeFile(metadataPath, `SAGASCRIPT_FRONTEND_HASH=${hash}\n`);
  return { distPath, metadataPath, hash };
}

test("verified frontend output is reusable without rewriting metadata", async () => {
  const { distPath, metadataPath, hash } = await fixture();
  const before = await readFile(metadataPath, "utf8");

  assert.equal(validateFrontend({ distPath, metadataPath }), hash);
  assert.equal(await readFile(metadataPath, "utf8"), before);
});

test("changed frontend output is rejected", async () => {
  const { distPath, metadataPath } = await fixture();
  await writeFile(join(distPath, "app.js"), "changed");

  assert.throws(
    () => validateFrontend({ distPath, metadataPath }),
    /does not match build metadata/,
  );
});

test("missing frontend output is rejected", async () => {
  const { distPath, metadataPath } = await fixture();
  await rm(distPath, { recursive: true });

  assert.throws(
    () => validateFrontend({ distPath, metadataPath }),
    /missing dist\/index\.html/,
  );
});

test("missing frontend metadata is rejected", async () => {
  const { distPath, metadataPath } = await fixture();
  await writeFile(metadataPath, "");

  assert.throws(
    () => validateFrontend({ distPath, metadataPath }),
    /invalid SAGASCRIPT_FRONTEND_HASH/,
  );
});
