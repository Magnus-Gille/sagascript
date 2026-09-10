import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, stat, utimes, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { hashFrontendTree, validateFrontend } from "./build-frontend.mjs";

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), "sagascript-frontend-reuse-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const distPath = join(root, "dist");
  const metadataPath = join(root, "build-meta.env");
  const indexPath = join(distPath, "index.html");
  await mkdir(distPath);
  await writeFile(indexPath, "<title>Sagascript Settings</title>\n");
  const hash = hashFrontendTree(distPath);
  await writeFile(metadataPath, `SAGASCRIPT_FRONTEND_HASH=${hash}\n`);
  return { distPath, metadataPath, hash };
}

test("verified frontend output is reusable without rewriting metadata", async (t) => {
  const { distPath, metadataPath, hash } = await fixture(t);
  await utimes(metadataPath, 1000, 1000);
  const before = await readFile(metadataPath, "utf8");
  const beforeMtime = (await stat(metadataPath)).mtimeMs;

  assert.equal(validateFrontend({ distPath, metadataPath }), hash);
  assert.equal(await readFile(metadataPath, "utf8"), before);
  assert.equal((await stat(metadataPath)).mtimeMs, beforeMtime);
});

test("changed frontend output is rejected", async (t) => {
  const { distPath, metadataPath } = await fixture(t);
  await writeFile(join(distPath, "app.js"), "changed");

  assert.throws(
    () => validateFrontend({ distPath, metadataPath }),
    /does not match build metadata/,
  );
});

test("missing frontend output is rejected", async (t) => {
  const { distPath, metadataPath } = await fixture(t);
  await rm(distPath, { recursive: true });

  assert.throws(
    () => validateFrontend({ distPath, metadataPath }),
    /missing dist\/index\.html/,
  );
});

test("missing frontend metadata is rejected", async (t) => {
  const { distPath, metadataPath } = await fixture(t);
  await rm(metadataPath);

  assert.throws(
    () => validateFrontend({ distPath, metadataPath }),
    /Frontend metadata is missing/,
  );
});
