// The npm license policy (#361): BSD-2-Clause and BlueOak-1.0.0 are accepted
// only for dev tooling that the app bundle cannot contain (owner decision,
// https://github.com/Magnus-Gille/sagascript/issues/361#issuecomment-6153321272).
// "dev": true in package-lock.json is not enough: Vite bundles whatever src/
// references, devDependencies included (svelte, esm-env). These tests pin the
// shipped closure scripts/lib/npm-license-policy.mjs computes, check it
// against everything a real `vite build` bundles, emits or reads, and hold
// every shipped package to the runtime license list on every PR
// (licenses:check runs only in the release gate).
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmdirSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, posix as posixPath } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { build } from "vite";
import {
  appImportedPackages,
  appPackageReferences,
  cssReferences,
  isAppSource,
  isBareSpecifier,
  npmLicenseError,
  packageNameOfSpecifier,
  resolveLockPath,
  reviewedDevOnlyNpmLicenses,
  reviewedNpmLicenses,
  scriptReferences,
  shippedNpmClosure,
  svelteBlocks,
} from "./lib/npm-license-policy.mjs";

// Canonical paths (symlinks resolved, Windows long names) as Vite sees them:
// a root that differs only in form makes Vite emit relative file names.
const root = realpathSync.native(fileURLToPath(new URL("..", import.meta.url)));
const lock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8"));
const packageName = (path) => path.slice(path.lastIndexOf("node_modules/") + "node_modules/".length);
// Computed on first use, so a bad reference under src/ fails only the tests
// that need the project's closure, with the scanner's message.
let projectClosure;
const closure = () => (projectClosure ??= shippedNpmClosure(lock.packages, appPackageReferences(root)));
// Quotes a specifier in fixture source, so that scripts/test-frontend-test-groups.mjs,
// which scans this file for the packages it imports, does not count the fixtures.
const q = (specifier) => `"${specifier}"`;
const sq = (specifier) => `'${specifier}'`;

/** A throwaway project: `files` maps paths to contents; node_modules is the repository's. */
function fixtureProject(files) {
  // macOS's temp dir is behind the /var -> /private/var symlink, Windows's may
  // use a short (RUNNER~1) name.
  const project = realpathSync.native(mkdtempSync(join(tmpdir(), "sagascript-license-")));
  for (const [path, content] of Object.entries(files)) {
    mkdirSync(dirname(join(project, path)), { recursive: true });
    writeFileSync(join(project, path), content);
  }
  // A junction needs no privileges on Windows; elsewhere it is a symlink.
  symlinkSync(join(root, "node_modules"), join(project, "node_modules"), "junction");
  return project;
}

function removeFixtureProject(project) {
  // Remove the link itself first, so no recursive delete can ever walk into
  // the repository's node_modules (rmdir removes a Windows junction, not its
  // target).
  const link = join(project, "node_modules");
  if (process.platform === "win32") rmdirSync(link);
  else unlinkSync(link);
  rmSync(project, { recursive: true, force: true });
}

test("the generator's synthetic policy cases pass (also run by the release gate)", () => {
  const output = execFileSync(
    process.execPath,
    [fileURLToPath(new URL("./generate-third-party-notices.mjs", import.meta.url)), "--test-dev-only-licenses"],
    { encoding: "utf8" },
  );
  assert.match(output, /dev-only npm license policy passed/);
});

test("the generator verifies the vendored live AEC legal texts and overrides", () => {
  const output = execFileSync(
    process.execPath,
    [fileURLToPath(new URL("./generate-third-party-notices.mjs", import.meta.url)), "--test-live-aec-licenses"],
    { encoding: "utf8" },
  );
  assert.match(output, /live AEC license override validation passed/);
});

test("the two dev-only license families are not in the runtime list", () => {
  for (const license of reviewedDevOnlyNpmLicenses) {
    assert.ok(!reviewedNpmLicenses.has(license), `${license} is reviewed for dev-only tooling only`);
  }
  assert.deepEqual([...reviewedDevOnlyNpmLicenses].sort(), ["BSD-2-Clause", "BlueOak-1.0.0"]);
});

test("scripts reference packages through imports, exports and new URL(), never through comments or strings", () => {
  const specifiers = (source) => scriptReferences(source).map((reference) => `${reference.kind} ${reference.specifier}`);
  assert.deepEqual(
    specifiers(`
      // adapted from ${sq("lodash")}
      /* import ${q("commented")}; */
      console.log("Recorded from ${sq("microphone")}");
      const label = \`import ${q("templated")}\`;
      import { invoke } from ${q("@tauri-apps/api/core")};
      import type { Snippet } from ${q("svelte")};
      import ${q("side-effect")};
      export { x } from ${q("esm-env")};
      export * from ${q("re-exported/sub")};
      import legacy = require(${q("required")});
      const later = await import(${q("lazy-pkg/sub")});
      const asset = new URL(${q("estraverse/estraverse.js")}, import.meta.url);
      const other = new URL(${q("not-an-asset")}, location.href);
      import worker from ${q("0x-worker/worker.js?worker")};
    `),
    [
      "import @tauri-apps/api/core",
      "import svelte",
      "import side-effect",
      "import esm-env",
      "import re-exported/sub",
      "import required",
      "import lazy-pkg/sub",
      "url estraverse/estraverse.js",
      "import 0x-worker/worker.js?worker",
    ],
  );
  assert.equal(packageNameOfSpecifier("@tauri-apps/api/core"), "@tauri-apps/api");
  assert.equal(packageNameOfSpecifier("svelte/internal/client"), "svelte");
  assert.equal(packageNameOfSpecifier("0x-worker/worker.js?worker"), "0x-worker");
  assert.equal(packageNameOfSpecifier("styles.css?url"), "styles.css");
  for (const specifier of ["svelte", "@scope/pkg", "0x-worker", "estraverse/estraverse.js"])
    assert.ok(isBareSpecifier(specifier), specifier);
  for (const specifier of ["./local", "../up", "/abs", "node:fs", "data:image/png;base64,AA", "https://example.com/x.css", "#hash"]) {
    assert.ok(!isBareSpecifier(specifier), specifier);
  }
});

test("stylesheets reference files through @import and url(), quoted or bare, not through comments", () => {
  assert.deepEqual(
    cssReferences(`
      /* @import ${q("commented.css")}; url(commented/x.png) */
      @import ${q("plain.css")};
      @import ${sq("single/quoted.css")};
      @import url("lz-string/tests/lib/jasmine-1.3.1/jasmine.css");
      @import url(bare/x.css) screen;
      .a { background: url("estraverse/estraverse.js"); }
      .b { background: url( 9pkg/x.png ); content: "/* not a comment */"; }
      .c { background: url(data:image/png;base64,AAAA); }
    `).map((reference) => reference.specifier),
    [
      "plain.css",
      "single/quoted.css",
      "lz-string/tests/lib/jasmine-1.3.1/jasmine.css",
      "bare/x.css",
      "estraverse/estraverse.js",
      "9pkg/x.png",
      "data:image/png;base64,AAAA",
    ],
  );
  const { scripts, styles } = svelteBlocks(`
    <!-- <script>import ${q("commented")};</script> -->
    <script lang="ts">import a from ${q("a")};</script>
    <p>Recorded from ${q("microphone")}</p>
    <style>.x { background: url(pkg/x.png); }</style>
  `);
  assert.deepEqual(scripts, [`import a from ${q("a")};`]);
  assert.deepEqual(styles, [".x { background: url(pkg/x.png); }"]);
});

test("app sources exclude tests, test support and declarations", () => {
  assert.ok(isAppSource("src/lib/api.ts"));
  assert.ok(isAppSource("src/lib/Settings.svelte"));
  assert.ok(isAppSource("src/main.ts"));
  assert.ok(isAppSource("src/app.css"));
  assert.ok(!isAppSource("src/lib/api.test.ts"));
  assert.ok(!isAppSource("src/test/ipc.ts"));
  assert.ok(!isAppSource("src/vite-env.d.ts"));
});

test("asset paths resolve as Vite does: a file next to the source first, then a package", () => {
  const project = fixtureProject({
    "src/main.ts": [
      `import "./app.css";`,
      `// adapted from ${sq("lodash")}`,
      `console.log("Recorded from ${sq("microphone")}");`,
      `export const local = new URL(${q("img/logo.png")}, import.meta.url);`,
      `export const shipped = new URL(${q("estraverse/estraverse.js")}, import.meta.url);`,
    ].join("\n"),
    "src/img/logo.png": "",
    "src/app.css": `@import url("lz-string/tests/lib/jasmine-1.3.1/jasmine.css");\n.a { background: url(img/logo.png); }\n.b { background: url(entities/package.json); }\n`,
    "src/lib/View.svelte": `<script>const u = new URL(${q("esrecurse/esrecurse.js")}, import.meta.url);</script>\n<style>.c { background: url("espree/package.json"); }</style>\n`,
  });
  try {
    assert.deepEqual(
      appPackageReferences(project).map((reference) => `${reference.file}: ${reference.specifier}`),
      [
        "src/app.css: lz-string/tests/lib/jasmine-1.3.1/jasmine.css",
        "src/app.css: entities/package.json",
        "src/lib/View.svelte: esrecurse/esrecurse.js",
        "src/lib/View.svelte: espree/package.json",
        "src/lib/View.svelte: svelte/internal (compiled component)",
        "src/main.ts: estraverse/estraverse.js",
      ],
    );
    assert.deepEqual(appImportedPackages(project), ["entities", "espree", "esrecurse", "estraverse", "lz-string", "svelte"]);
  } finally {
    removeFixtureProject(project);
  }
});

test("a reference the lockfile cannot resolve names its file and what matched", () => {
  const project = fixtureProject({ "src/main.ts": `import { debounce } from ${q("lodash-es")};\n` });
  try {
    assert.throws(
      () => shippedNpmClosure(lock.packages, appPackageReferences(project)),
      /^Error: src\/main\.ts references npm package lodash-es \("lodash-es"\), which package-lock\.json does not contain\./,
    );
  } finally {
    removeFixtureProject(project);
  }
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
  const names = new Set([...closure()].map(packageName));
  for (const name of ["@tauri-apps/api", "@tauri-apps/plugin-dialog", "@tauri-apps/plugin-autostart", "svelte", "esm-env", "vite"]) {
    assert.ok(names.has(name), `${name} is shipped`);
  }
  for (const name of [
    "happy-dom",
    "entities",
    "estraverse",
    "eslint",
    "espree",
    "minimatch",
    "vitest",
    "@testing-library/svelte",
    "typescript",
    "rollup",
  ]) {
    assert.ok(!names.has(name), `${name} is build or test tooling`);
  }
  // svelte and esm-env are devDependencies: "dev" alone would miss them.
  assert.equal(lock.packages["node_modules/svelte"].dev, true);
  assert.equal(lock.packages["node_modules/esm-env"].dev, true);
});

test("every package the app can ship has a license from the runtime list", () => {
  const failures = [...closure()].flatMap((path) => {
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

test("app code reaching a BSD-2-Clause dev package, by import or as an asset, fails the policy", () => {
  const project = fixtureProject({
    "src/lib/decode.ts": `import { decodeHTML } from ${q("entities")};\nexport const decode = decodeHTML;\n`,
    "src/lib/decode.test.ts": `import { test } from ${q("vitest")};\ntest("x", () => undefined);\n`,
    "src/main.ts": `export const asset = new URL(${q("estraverse/estraverse.js")}, import.meta.url);\n`,
    "src/app.css": `.a { background: url(esrecurse/esrecurse.js); }\n`,
  });
  try {
    const shipped = shippedNpmClosure(lock.packages, appPackageReferences(project));
    for (const name of ["entities", "estraverse", "esrecurse"]) {
      const entry = lock.packages[`node_modules/${name}`];
      assert.equal(entry.dev, true);
      assert.equal(entry.license, "BSD-2-Clause");
      assert.match(
        npmLicenseError({
          name,
          version: entry.version,
          license: entry.license,
          devOnly: true,
          shipped: shipped.has(`node_modules/${name}`),
        }),
        new RegExp(
          `^npm ${name}@.* has license BSD-2-Clause, which is reviewed for dev-only tooling only; the app bundle can contain this package`,
        ),
      );
    }
  } finally {
    removeFixtureProject(project);
  }
});

// The lockfile key of the package a bundled file belongs to:
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

const posix = (path) => path.replaceAll("\\", "/");

function relativeTo(path, base) {
  const prefix = `${posix(base).replace(/\/$/, "")}/`;
  // Windows paths are C:/... on both sides, but compare the drive letter loosely.
  const inside = process.platform === "win32" ? path.toLowerCase().startsWith(prefix.toLowerCase()) : path.startsWith(prefix);
  return inside ? path.slice(prefix.length) : null;
}

/**
 * What a module id, emitted asset's original file or file read by a build
 * is: { kind: "package", lockPath } for npm code, { kind: "app" } for the
 * project's own files, or { kind: "unexplained" }.
 */
function classifyBuildInput(id, { projectRoot, modulesRoot = projectRoot }) {
  let path = posix(id);
  const queryAt = path.indexOf("?");
  const query = queryAt === -1 ? "" : path.slice(queryAt);
  if (queryAt !== -1) path = path.slice(0, queryAt);
  if (path.startsWith("\0")) {
    const virtual = path.slice(1);
    // Vite's own helpers, and Rollup's CommonJS helpers, which vite bundles.
    if (/^vite\/(modulepreload-polyfill|preload-helper)\.js$/.test(virtual) || virtual === "commonjsHelpers.js") {
      return { kind: "package", lockPath: "node_modules/vite" };
    }
    // A CommonJS module's proxies (?commonjs-exports, ?commonjs-es-import, ...) stand for the file itself.
    if (!/[?&]commonjs-/.test(query)) return { kind: "unexplained" };
    path = virtual;
  }
  // Emitted assets name their original file relative to the project.
  // (Joined as POSIX paths: path.resolve would add the current drive on Windows.)
  if (!/^([a-z]:)?\//i.test(path)) path = posixPath.normalize(`${posix(projectRoot)}/${path}`);
  for (const base of [modulesRoot, projectRoot]) {
    const relativePath = relativeTo(path, base);
    if (relativePath?.startsWith("node_modules/")) return { kind: "package", lockPath: lockPathOfModule(relativePath) };
  }
  const relativePath = relativeTo(path, projectRoot);
  if (relativePath === null) return { kind: "unexplained" };
  if (relativePath === "index.html" || relativePath === "package.json" || relativePath.startsWith("public/")) return { kind: "app" };
  if (relativePath.startsWith("src/") && !relativePath.startsWith("src/test/") && !relativePath.endsWith(".test.ts"))
    return { kind: "app" };
  return { kind: "unexplained" };
}

/**
 * Everything a production build puts into the app or reads for it: module
 * ids, the original files of emitted assets (nothing inlined), and the files
 * the build read (CSS @import targets never become modules).
 */
async function buildInputs(options) {
  let read = [];
  const recorder = {
    name: "record-build-inputs",
    buildEnd() {
      read = this.getWatchFiles();
    },
  };
  const result = await build({
    ...options,
    logLevel: "silent",
    build: { write: false, assetsInlineLimit: 0 },
    plugins: [...(options.plugins ?? []), recorder],
  });
  const outputs = (Array.isArray(result) ? result : [result]).flatMap((output) => output.output);
  const modules = outputs.filter((item) => item.type === "chunk").flatMap((chunk) => Object.keys(chunk.modules));
  const assets = outputs.filter((item) => item.type === "asset").flatMap((asset) => asset.originalFileNames ?? []);
  return { modules, assets, read, all: [...modules, ...assets, ...read] };
}

/** The packages and unexplained files among a build's inputs. */
function classifyBuildInputs(inputs, roots) {
  const packages = new Set();
  const unexplained = [];
  for (const input of inputs) {
    const result = classifyBuildInput(input, roots);
    if (result.kind === "package") packages.add(result.lockPath);
    else if (result.kind === "unexplained") unexplained.push(input);
  }
  return { packages, unexplained };
}

test("build inputs are classified as npm packages, app files or unexplained", () => {
  const roots = { projectRoot: "/repo" };
  const lockPath = (id) => classifyBuildInput(id, roots).lockPath ?? classifyBuildInput(id, roots).kind;
  assert.equal(lockPath("/repo/node_modules/svelte/src/index-client.js"), "node_modules/svelte");
  assert.equal(lockPath("/repo/node_modules/a/node_modules/@b/c/x.js?v=1"), "node_modules/a/node_modules/@b/c");
  assert.equal(lockPath("node_modules/estraverse/estraverse.js"), "node_modules/estraverse");
  assert.equal(lockPath("\0vite/modulepreload-polyfill.js"), "node_modules/vite");
  assert.equal(lockPath("\0vite/preload-helper.js"), "node_modules/vite");
  assert.equal(lockPath("\0commonjsHelpers.js"), "node_modules/vite");
  assert.equal(lockPath("\0/repo/node_modules/cjs-pkg/index.js?commonjs-exports"), "node_modules/cjs-pkg");
  assert.equal(lockPath("\0/repo/node_modules/cjs-pkg/index.js?commonjs-es-import"), "node_modules/cjs-pkg");
  assert.equal(lockPath("/repo/src/lib/Settings.svelte"), "app");
  assert.equal(lockPath("src/img/logo.png"), "app");
  assert.equal(lockPath("index.html"), "app");
  assert.equal(lockPath("/repo/public/favicon.svg"), "app");
  assert.equal(lockPath("/repo/src/lib/api.test.ts"), "unexplained");
  assert.equal(lockPath("/repo/src/test/ipc.ts"), "unexplained");
  assert.equal(lockPath("\0some-plugin/virtual.js"), "unexplained");
  assert.equal(lockPath("/elsewhere/x.js"), "unexplained");
  assert.equal(
    classifyBuildInput("/real/node_modules/entities/lib/index.js", { projectRoot: "/tmp/x", modulesRoot: "/real" }).lockPath,
    "node_modules/entities",
  );
});

test("a real vite build bundles, emits and reads nothing outside app files and the shipped closure", async () => {
  // The production build as `npm run build` makes it (vite.config.ts), in
  // memory and without inlining assets; about two seconds.
  const inputs = await buildInputs({ root, configLoader: "runner" });
  const { packages, unexplained } = classifyBuildInputs(inputs.all, { projectRoot: root });
  assert.deepEqual(unexplained, [], "every build input is app code, a Vite helper or an npm package");
  assert.deepEqual(
    [...packages].filter((path) => !closure().has(path)),
    [],
    "every bundled package is in the shipped closure the license policy applies",
  );
  for (const path of ["node_modules/svelte", "node_modules/esm-env", "node_modules/@tauri-apps/api", "node_modules/vite"]) {
    assert.ok(packages.has(path), `the bundle contains ${path}`);
  }
});

test("the build check sees package files shipped as assets and CSS @import url(), which are no Rollup modules", async () => {
  const project = fixtureProject({
    "index.html": `<!doctype html><html><head><script type="module" src="/src/main.ts"></script></head><body></body></html>\n`,
    "src/main.ts": [
      `import "./app.css";`,
      `export const asset = new URL(${q("estraverse/estraverse.js")}, import.meta.url);`,
      // Small enough that Vite would inline it as a data: URI by default.
      `export const small = new URL(${q("esutils/package.json")}, import.meta.url);`,
    ].join("\n"),
    "src/app.css": `@import url("lz-string/tests/lib/jasmine-1.3.1/jasmine.css");\n.a { background: url("esrecurse/esrecurse.js"); }\n`,
  });
  try {
    const inputs = await buildInputs({ root: project, configFile: false });
    const roots = { projectRoot: project, modulesRoot: root };
    const { packages, unexplained } = classifyBuildInputs(inputs.all, roots);
    assert.deepEqual(unexplained, []);
    // Each source on its own: the emitted assets name the url() files (none
    // inlined), the read files include the inlined stylesheet; none of them
    // is a module.
    const lockPaths = (ids) => new Set(ids.map((id) => classifyBuildInput(id, roots).lockPath).filter(Boolean));
    assert.deepEqual([...lockPaths(inputs.assets)].sort(), ["node_modules/esrecurse", "node_modules/estraverse", "node_modules/esutils"]);
    assert.ok(lockPaths(inputs.read).has("node_modules/lz-string"));
    assert.deepEqual([...lockPaths(inputs.modules)].sort(), ["node_modules/vite"]);
    // Neither an emitted asset nor an inlined CSS @import is a Rollup module;
    // the check still sees all three packages...
    const reached = ["node_modules/estraverse", "node_modules/esutils", "node_modules/lz-string", "node_modules/esrecurse"];
    assert.deepEqual(
      reached.filter((path) => !packages.has(path)),
      [],
    );
    // ...and the scanner puts them into this project's closure.
    const fixtureClosure = shippedNpmClosure(lock.packages, appPackageReferences(project));
    assert.deepEqual(
      reached.filter((path) => !fixtureClosure.has(path)),
      [],
    );
  } finally {
    removeFixtureProject(project);
  }
});
