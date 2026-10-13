// The npm license policy (#361): BSD-2-Clause and BlueOak-1.0.0 are accepted
// only for dev tooling that the app bundle cannot contain (owner decision,
// https://github.com/Magnus-Gille/sagascript/issues/361#issuecomment-6153321272).
// "dev": true in package-lock.json is not enough: Vite bundles whatever src/
// imports, devDependencies included (svelte, esm-env). These tests pin the
// shipped closure scripts/lib/npm-license-policy.mjs computes, check it
// against the modules of a real `vite build`, and hold every shipped package
// to the runtime license list on every PR (licenses:check runs only in the
// release gate).
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { build } from "vite";
import {
  appImportedPackages,
  importedPackages,
  isAppSource,
  npmLicenseError,
  packageNameOfSpecifier,
  resolveLockPath,
  reviewedDevOnlyNpmLicenses,
  reviewedNpmLicenses,
  shippedNpmClosure,
} from "./lib/npm-license-policy.mjs";

const root = fileURLToPath(new URL("..", import.meta.url));
const lock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8"));
const closure = shippedNpmClosure(lock.packages, appImportedPackages(root));
const packageName = (path) => path.slice(path.lastIndexOf("node_modules/") + "node_modules/".length);
const closureNames = new Set([...closure].map(packageName));
// Quotes a specifier in fixture source, so that scripts/test-frontend-test-groups.mjs,
// which scans this file for the packages it imports, does not count the fixtures.
const q = (specifier) => `"${specifier}"`;

test("the generator's synthetic policy cases pass (also run by the release gate)", () => {
  const output = execFileSync(
    process.execPath,
    [fileURLToPath(new URL("./generate-third-party-notices.mjs", import.meta.url)), "--test-dev-only-licenses"],
    { encoding: "utf8" },
  );
  assert.match(output, /dev-only npm license policy passed/);
});

test("the two dev-only license families are not in the runtime list", () => {
  for (const license of reviewedDevOnlyNpmLicenses) {
    assert.ok(!reviewedNpmLicenses.has(license), `${license} is reviewed for dev-only tooling only`);
  }
  assert.deepEqual([...reviewedDevOnlyNpmLicenses].sort(), ["BSD-2-Clause", "BlueOak-1.0.0"]);
});

test("bare imports name packages; app sources exclude tests and declarations", () => {
  assert.equal(packageNameOfSpecifier("@tauri-apps/api/core"), "@tauri-apps/api");
  assert.equal(packageNameOfSpecifier("svelte/internal/client"), "svelte");
  assert.deepEqual(
    importedPackages(`
      import { invoke } from ${q("@tauri-apps/api/core")};
      import type { Snippet } from ${q("svelte")};
      import ${q("entities")};
      export { x } from ${q("esm-env")};
      const later = await import(${q("lazy-pkg/sub")});
      import { local } from ${q("./local")};
      import { readFile } from ${q("node:fs")};
    `),
    ["@tauri-apps/api", "svelte", "entities", "esm-env", "lazy-pkg"],
  );
  assert.ok(isAppSource("src/lib/api.ts"));
  assert.ok(isAppSource("src/lib/Settings.svelte"));
  assert.ok(isAppSource("src/main.ts"));
  assert.ok(!isAppSource("src/lib/api.test.ts"));
  assert.ok(!isAppSource("src/test/ipc.ts"));
  assert.ok(!isAppSource("src/vite-env.d.ts"));
  assert.ok(isAppSource("src/app.css"));
  assert.deepEqual(importedPackages(`@import ${q("modern-normalize/modern-normalize.css")};\n@import ${q("./local.css")};`), [
    "modern-normalize",
  ]);
});

test("lockfile lookups follow Node: nearest node_modules first, then up", () => {
  const packages = {
    "": {},
    "node_modules/a": {},
    "node_modules/a/node_modules/b": {},
    "node_modules/b": {},
    "node_modules/@scope/c": {},
  };
  assert.equal(resolveLockPath(packages, "node_modules/a", "b"), "node_modules/a/node_modules/b");
  assert.equal(resolveLockPath(packages, "node_modules/a/node_modules/b", "@scope/c"), "node_modules/@scope/c");
  assert.equal(resolveLockPath(packages, "", "b"), "node_modules/b");
  assert.equal(resolveLockPath(packages, "node_modules/a", "missing"), null);
});

test("the shipped closure holds the runtime dependencies, svelte and what they need, not the test and lint tooling", () => {
  for (const name of ["@tauri-apps/api", "@tauri-apps/plugin-dialog", "@tauri-apps/plugin-autostart", "svelte", "esm-env", "vite"]) {
    assert.ok(closureNames.has(name), `${name} is shipped`);
  }
  for (const name of [
    "happy-dom",
    "entities",
    "eslint",
    "espree",
    "minimatch",
    "vitest",
    "@testing-library/svelte",
    "typescript",
    "rollup",
  ]) {
    assert.ok(!closureNames.has(name), `${name} is build or test tooling`);
  }
  // svelte and esm-env are devDependencies: "dev" alone would miss them.
  assert.equal(lock.packages["node_modules/svelte"].dev, true);
  assert.equal(lock.packages["node_modules/esm-env"].dev, true);
});

test("every package the app can ship has a license from the runtime list", () => {
  const failures = [...closure].flatMap((path) => {
    const entry = lock.packages[path];
    const error = npmLicenseError({
      name: packageName(path),
      version: entry.version,
      license: entry.license,
      devOnly: entry.dev === true,
      shipped: true,
    });
    return error === null ? [] : [error];
  });
  assert.deepEqual(failures, []);
});

test("app code importing a BSD-2-Clause dev package (entities, from happy-dom) fails the policy", () => {
  const project = mkdtempSync(join(tmpdir(), "sagascript-license-"));
  try {
    mkdirSync(join(project, "src/lib"), { recursive: true });
    writeFileSync(join(project, "src/lib/decode.ts"), `import { decodeHTML } from ${q("entities")};\nexport const decode = decodeHTML;\n`);
    writeFileSync(join(project, "src/lib/decode.test.ts"), `import { test } from ${q("vitest")};\ntest("x", () => undefined);\n`);
    assert.deepEqual(appImportedPackages(project), ["entities"]);
    const shipped = shippedNpmClosure(lock.packages, appImportedPackages(project));
    const entry = lock.packages["node_modules/entities"];
    assert.equal(entry.dev, true);
    assert.equal(entry.license, "BSD-2-Clause");
    assert.match(
      npmLicenseError({
        name: "entities",
        version: entry.version,
        license: entry.license,
        devOnly: true,
        shipped: shipped.has("node_modules/entities"),
      }),
      /^npm entities@.* has license BSD-2-Clause, which is reviewed for dev-only tooling only; the app bundle can contain this package/,
    );
  } finally {
    rmSync(project, { recursive: true, force: true });
  }
});

// The lockfile key of the package a bundled module belongs to:
// node_modules/a/node_modules/@b/c/x.js → node_modules/a/node_modules/@b/c.
function lockPathOfModule(relativePath) {
  const segments = relativePath.split("/");
  const key = [];
  let index = 0;
  while (segments[index] === "node_modules") {
    const length = segments[index + 1].startsWith("@") ? 2 : 1;
    key.push("node_modules", ...segments.slice(index + 1, index + 1 + length));
    index += 1 + length;
  }
  return key.join("/");
}

test("a real vite build bundles nothing outside app sources and the shipped closure", async () => {
  // The production build as `npm run build` makes it (vite.config.ts), kept
  // in memory; about two seconds.
  const result = await build({ root, configLoader: "runner", logLevel: "silent", build: { write: false } });
  const chunks = (Array.isArray(result) ? result : [result]).flatMap((output) => output.output).filter((item) => item.type === "chunk");
  const posixRoot = root.replaceAll("\\", "/").replace(/\/$/, "");
  const bundled = new Set();
  const unexplained = [];
  for (const id of chunks.flatMap((chunk) => Object.keys(chunk.modules))) {
    const path = id.replaceAll("\\", "/").split("?")[0];
    if (path.startsWith("\0")) {
      // Virtual modules: only Vite's own injected helpers are expected.
      if (/^\0vite\/(modulepreload-polyfill|preload-helper)\.js$/.test(path)) bundled.add("node_modules/vite");
      else unexplained.push(id);
      continue;
    }
    // Windows ids are C:/... like the root, but compare the drive letter loosely.
    const underRoot =
      process.platform === "win32" ? path.toLowerCase().startsWith(`${posixRoot.toLowerCase()}/`) : path.startsWith(`${posixRoot}/`);
    const relativePath = underRoot ? path.slice(posixRoot.length + 1) : null;
    if (relativePath?.startsWith("node_modules/")) bundled.add(lockPathOfModule(relativePath));
    else if (relativePath === "index.html" || (relativePath?.startsWith("src/") && isAppSource(relativePath))) continue;
    else unexplained.push(id);
  }
  assert.deepEqual(unexplained, [], "every bundled module is app code, a Vite helper or an npm package");
  assert.deepEqual(
    [...bundled].filter((path) => !closure.has(path)),
    [],
    "every bundled package is in the shipped closure the license policy applies",
  );
  for (const path of ["node_modules/svelte", "node_modules/esm-env", "node_modules/@tauri-apps/api", "node_modules/vite"]) {
    assert.ok(bundled.has(path), `the bundle contains ${path}`);
  }
});
