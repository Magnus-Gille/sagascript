import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { checkRelease } from "./check-release.mjs";

const expectedVersion = "1.4.5";

async function createFixture() {
  const root = await mkdtemp(join(tmpdir(), "sagascript-release-check-"));
  await mkdir(join(root, "src-tauri", "crates", "sagascript-core", "src", "settings"), { recursive: true });
  await mkdir(join(root, "src-tauri", "engine-host", "ort"), { recursive: true });
  await writeFile(join(root, "package.json"), JSON.stringify({ version: expectedVersion }));
  await writeFile(
    join(root, "package-lock.json"),
    JSON.stringify({ version: expectedVersion, packages: { "": { version: expectedVersion } } }),
  );
  await writeFile(
    join(root, "src-tauri", "tauri.conf.json"),
    JSON.stringify({ version: expectedVersion, identifier: "ai.gille.sagascript" }),
  );
  await writeFile(join(root, "src-tauri", "Cargo.toml"), `[workspace.package]\nversion = "${expectedVersion}"\n`);
  await writeFile(
    join(root, "src-tauri", "engine-host", "ort", "Cargo.toml"),
    `[package]\nname = "sagascript-engine-host-ort"\nversion = "${expectedVersion}"\n`,
  );
  await writeFile(
    join(root, "src-tauri", "engine-host", "ort", "Cargo.lock"),
    `[[package]]\nname = "sagascript-engine-host-ort"\nversion = "${expectedVersion}"\n\n[[package]]\nname = "sagascript-engine-protocol"\nversion = "${expectedVersion}"\n`,
  );
  await writeFile(
    join(root, "src-tauri", "crates", "sagascript-core", "src", "settings", "store.rs"),
    'const APP_IDENTIFIER: &str = "ai.gille.sagascript";',
  );
  await writeFile(join(root, "index.html"), "<title>Sagascript Settings</title>");
  return root;
}

function checkFixture(root) {
  const inheritedTag = process.env.RELEASE_TAG;
  delete process.env.RELEASE_TAG;
  try {
    return checkRelease(root);
  } finally {
    if (inheritedTag === undefined) delete process.env.RELEASE_TAG;
    else process.env.RELEASE_TAG = inheritedTag;
  }
}

test("release check rejects stale or missing standalone ORT metadata", async (t) => {
  const root = await createFixture();
  t.after(() => rm(root, { recursive: true, force: true }));

  const ortManifest = join(root, "src-tauri", "engine-host", "ort", "Cargo.toml");
  const ortLock = join(root, "src-tauri", "engine-host", "ort", "Cargo.lock");

  await writeFile(ortManifest, '[package]\nname = "sagascript-engine-host-ort"\nversion = "1.4.4"\n');
  assert.throws(() => checkFixture(root), /engine-host\/ort\/Cargo\.toml=1\.4\.4/);

  await writeFile(ortManifest, `[package]\nname = "sagascript-engine-host-ort"\nversion = "${expectedVersion}"\n`);
  await writeFile(
    ortLock,
    '[[package]]\nname = "sagascript-engine-host-ort"\nversion = "1.4.4"\n\n[[package]]\nname = "sagascript-engine-protocol"\nversion = "1.4.5"\n',
  );
  assert.throws(() => checkFixture(root), /engine-host\/ort\/Cargo\.lock host package=1\.4\.4/);

  await writeFile(
    ortLock,
    '[[package]]\nname = "sagascript-engine-host-ort"\nversion = "1.4.5"\n\n[[package]]\nname = "sagascript-engine-protocol"\nversion = "1.4.4"\n',
  );
  assert.throws(() => checkFixture(root), /engine-host\/ort\/Cargo\.lock protocol package=1\.4.4/);

  await rm(ortLock);
  assert.throws(() => checkFixture(root), /Cargo\.lock/);
});

test("release check accepts aligned standalone ORT metadata", async (t) => {
  const root = await createFixture();
  t.after(() => rm(root, { recursive: true, force: true }));

  assert.doesNotThrow(() => checkFixture(root));
});
