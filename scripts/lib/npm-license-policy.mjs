// The npm license policy of scripts/generate-third-party-notices.mjs (#361),
// shared with its tests: which license expressions are reviewed, and which
// npm packages can end up in the shipped frontend bundle.
//
// Vite bundles whatever src/ imports, whether package.json lists it under
// dependencies or devDependencies (svelte and its esm-env are devDependencies
// and ship in every build). "dev": true in package-lock.json therefore does
// not mean "never shipped"; the shipped set is computed from the imports.
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

// Reviewed npm license expressions, as exact SPDX strings (an allowlist, as for
// the Rust crates in the generator). Every npm package must use one of these,
// and every package in the shipped closure (shippedNpmClosure) must.
export const reviewedNpmLicenses = new Set(["0BSD", "Apache-2.0", "Apache-2.0 OR MIT", "BSD-3-Clause", "ISC", "MIT", "MIT OR Apache-2.0"]);

// Reviewed only for build and test tooling: packages that are both outside the
// shipped closure and marked `"dev": true` in package-lock.json. A package the
// app bundle can contain (imported by src/, a runtime dependency, or their
// dependencies) still fails with one of these and needs a new owner decision.
// Decision: https://github.com/Magnus-Gille/sagascript/issues/361#issuecomment-6153321272
export const reviewedDevOnlyNpmLicenses = new Set(["BSD-2-Clause", "BlueOak-1.0.0"]);

// Code Vite writes into the bundle itself rather than from an import: the
// module-preload polyfill (\0vite/modulepreload-polyfill.js, in every build)
// and the preload helper (\0vite/preload-helper.js, with dynamic imports).
// Both are self-contained snippets of the vite package (MIT) that import
// nothing, so vite counts as shipped without its own dependency tree (rollup,
// esbuild, postcss, ...), which only runs at build time.
export const VITE_INJECTED_PACKAGES = ["vite"];

/** The package a bare specifier names: "@tauri-apps/api/core" → "@tauri-apps/api", "svelte/internal" → "svelte". */
export function packageNameOfSpecifier(specifier) {
  const parts = specifier.split("/");
  return specifier.startsWith("@") ? parts.slice(0, 2).join("/") : parts[0];
}

// Static (`from "x"`), side-effect (`import "x"`, CSS `@import "x"`) and
// dynamic (`import("x")`) imports and re-exports of a bare package specifier.
const BARE_IMPORT = /(?:\bfrom\s*|\bimport\s*\(?\s*)["']([@a-z][\w@./:-]*)["']/g;

/** The packages a JavaScript, TypeScript, Svelte or CSS source imports by bare specifier. */
export function importedPackages(source) {
  return [...source.matchAll(BARE_IMPORT)]
    .map((match) => match[1])
    .filter((specifier) => !specifier.startsWith("node:"))
    .map(packageNameOfSpecifier);
}

/**
 * Is a file under src/ part of the app, as opposed to tests, test support or
 * declarations? Stylesheets count: a CSS `@import "pkg"` would bundle pkg.
 */
export function isAppSource(path) {
  return /\.(ts|js|svelte|css)$/.test(path) && !/\.test\.ts$/.test(path) && !path.endsWith(".d.ts") && !path.startsWith("src/test/");
}

function filesUnder(root, directory) {
  return readdirSync(join(root, directory), { withFileTypes: true }).flatMap((entry) => {
    const path = `${directory}/${entry.name}`;
    return entry.isDirectory() ? filesUnder(root, path) : [path];
  });
}

/**
 * The packages the app's own code (`isAppSource` files under src/) imports.
 * A compiled .svelte component imports Svelte's runtime (svelte/internal/...)
 * even when its source imports nothing, so any component adds svelte.
 */
export function appImportedPackages(root) {
  const packages = new Set();
  for (const path of filesUnder(root, "src").filter(isAppSource)) {
    for (const name of importedPackages(readFileSync(join(root, path), "utf8"))) packages.add(name);
    if (path.endsWith(".svelte")) packages.add("svelte");
  }
  return [...packages].sort();
}

/**
 * The package-lock.json `packages` key that `name`, required by the package
 * at `fromPath` ("" for the project), resolves to, as Node looks it up: the
 * nearest node_modules/<name> walking up from `fromPath`. Null if absent.
 */
export function resolveLockPath(packages, fromPath, name) {
  let base = fromPath;
  for (;;) {
    const candidate = `${base ? `${base}/` : ""}node_modules/${name}`;
    if (Object.hasOwn(packages, candidate)) return candidate;
    if (!base) return null;
    const parent = base.lastIndexOf("/node_modules/");
    base = parent === -1 ? "" : base.slice(0, parent);
  }
}

/**
 * The lockfile keys of every package the shipped bundle can contain: the
 * project's runtime `dependencies`, the packages app code imports
 * (`appPackages`), and everything they depend on (`dependencies`, plus
 * `optionalDependencies` and `peerDependencies` that are installed), followed
 * transitively; plus the packages Vite injects code from, without their
 * dependencies. An over-approximation: svelte's compiler dependencies are in
 * it although only its runtime is bundled. Throws if an app import is not in
 * the lockfile.
 */
export function shippedNpmClosure(packages, appPackages) {
  const shipped = new Set();
  const visit = (fromPath, name, required) => {
    const path = resolveLockPath(packages, fromPath, name);
    if (path === null) {
      if (required) throw new Error(`${fromPath || "src/"} needs npm package ${name}, which package-lock.json does not contain`);
      return;
    }
    if (shipped.has(path)) return;
    shipped.add(path);
    const entry = packages[path];
    for (const dependency of Object.keys(entry.dependencies ?? {})) visit(path, dependency, true);
    for (const dependency of Object.keys({ ...entry.optionalDependencies, ...entry.peerDependencies })) visit(path, dependency, false);
  };
  for (const name of Object.keys(packages[""]?.dependencies ?? {})) visit("", name, true);
  for (const name of appPackages) visit("", name, true);
  for (const name of VITE_INJECTED_PACKAGES) {
    const path = resolveLockPath(packages, "", name);
    if (path === null) throw new Error(`package-lock.json does not contain ${name}, which injects code into the bundle`);
    shipped.add(path);
  }
  return shipped;
}

/** The shipped closure of this checkout: package-lock.json and the imports under src/. */
export function projectShippedNpmClosure(root) {
  const lock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8"));
  return shippedNpmClosure(lock.packages, appImportedPackages(root));
}

/**
 * Why a package's license is not acceptable, or null. `pkg`: { name, version,
 * license, devOnly ("dev": true in the lockfile), shipped (in the closure) }.
 */
export function npmLicenseError(pkg) {
  const id = `npm ${pkg.name}@${pkg.version}`;
  if (!pkg.license) return `${id} has no SPDX license`;
  if (reviewedNpmLicenses.has(pkg.license)) return null;
  if (reviewedDevOnlyNpmLicenses.has(pkg.license)) {
    if (pkg.devOnly && !pkg.shipped) return null;
    return `${id} has license ${pkg.license}, which is reviewed for dev-only tooling only; ${
      pkg.shipped
        ? "the app bundle can contain this package (src/ or a runtime dependency reaches it)"
        : "this package is not a dev dependency"
    }`;
  }
  return `${id} has unreviewed license: ${pkg.license}`;
}
