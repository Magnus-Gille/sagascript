// The npm license policy of scripts/generate-third-party-notices.mjs (#361),
// shared with its tests: which license expressions are reviewed, and which
// npm packages can end up in the shipped frontend bundle.
//
// Vite bundles whatever src/ references (imports, and package files used as
// assets or stylesheets), whether package.json lists it under dependencies or
// devDependencies (svelte and its esm-env are devDependencies and ship in
// every build). "dev": true in package-lock.json therefore does not mean
// "never shipped"; the shipped set is computed from what src/ references.
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

// Loaded on first use: the generator's --test-* modes and the Windows CI step
// that runs one before `npm ci` import this module without node_modules.
let typescriptModule;
const ts = () => (typescriptModule ??= createRequire(import.meta.url)("typescript"));

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

/** The package a bare specifier names: "@tauri-apps/api/core" → "@tauri-apps/api", "svelte/internal?url" → "svelte". */
export function packageNameOfSpecifier(specifier) {
  const parts = specifier.split(/[?#]/)[0].split("/");
  return specifier.startsWith("@") ? parts.slice(0, 2).join("/") : parts[0];
}

/** Could `specifier` name an npm package (rather than a relative path, a URL or a Node builtin)? */
export function isBareSpecifier(specifier) {
  return /^[@a-z0-9]/i.test(specifier) && !/^[a-z][a-z0-9+.-]*:/i.test(specifier);
}

function isImportMetaUrl(node) {
  const { SyntaxKind, isMetaProperty, isPropertyAccessExpression } = ts();
  return (
    isPropertyAccessExpression(node) &&
    node.name.text === "url" &&
    isMetaProperty(node.expression) &&
    node.expression.keywordToken === SyntaxKind.ImportKeyword
  );
}

/**
 * The module specifiers a script really imports, from its syntax tree, so
 * comments and strings never count: import and export declarations (types
 * included), `import x = require("y")`, dynamic `import("x")` with a literal,
 * and `new URL("x", import.meta.url)` (an asset Vite emits).
 * `{ specifier, kind }`, kind "import" or "url".
 */
export function scriptReferences(source, fileName = "module.ts") {
  const typescript = ts();
  const kind = fileName.endsWith(".js") ? typescript.ScriptKind.JS : typescript.ScriptKind.TS;
  const sourceFile = typescript.createSourceFile(fileName, source, typescript.ScriptTarget.Latest, false, kind);
  const references = [];
  const visit = (node) => {
    if (
      (typescript.isImportDeclaration(node) || typescript.isExportDeclaration(node)) &&
      node.moduleSpecifier &&
      typescript.isStringLiteral(node.moduleSpecifier)
    ) {
      references.push({ specifier: node.moduleSpecifier.text, kind: "import" });
    } else if (typescript.isExternalModuleReference(node) && typescript.isStringLiteral(node.expression)) {
      references.push({ specifier: node.expression.text, kind: "import" });
    } else if (
      typescript.isCallExpression(node) &&
      node.expression.kind === typescript.SyntaxKind.ImportKeyword &&
      node.arguments[0] &&
      typescript.isStringLiteralLike(node.arguments[0])
    ) {
      references.push({ specifier: node.arguments[0].text, kind: "import" });
    } else if (
      typescript.isNewExpression(node) &&
      typescript.isIdentifier(node.expression) &&
      node.expression.text === "URL" &&
      node.arguments?.length === 2 &&
      typescript.isStringLiteralLike(node.arguments[0]) &&
      isImportMetaUrl(node.arguments[1])
    ) {
      references.push({ specifier: node.arguments[0].text, kind: "url" });
    }
    typescript.forEachChild(node, visit);
  };
  visit(sourceFile);
  return references;
}

/** CSS with its comments blanked out (line structure kept); strings stay intact. */
function withoutCssComments(css) {
  let result = "";
  let index = 0;
  while (index < css.length) {
    const char = css[index];
    if (char === "/" && css[index + 1] === "*") {
      const close = css.indexOf("*/", index + 2);
      const end = close === -1 ? css.length : close + 2;
      result += css.slice(index, end).replace(/[^\n]/g, " ");
      index = end;
    } else if (char === '"' || char === "'") {
      let end = index + 1;
      while (end < css.length && css[end] !== char && css[end] !== "\n") end += css[end] === "\\" ? 2 : 1;
      result += css.slice(index, end + 1);
      index = end + 1;
    } else {
      result += char;
      index += 1;
    }
  }
  return result;
}

// `@import "x"`, `@import 'x'`, `@import url(x)` and any `url(x)`, quoted or bare.
const CSS_IMPORT = /@import\s+(?:url\(\s*)?(["']?)([^"'()\s;]+)\1/g;
const CSS_URL = /\burl\(\s*(["']?)([^"'()\s]+)\1\s*\)/g;

/**
 * The files a stylesheet pulls into the bundle: `@import` (also as
 * `@import url(...)`, which postcss-import inlines without a Rollup module)
 * and `url(...)` (an asset Vite emits). Comments do not count.
 */
export function cssReferences(css) {
  const clean = withoutCssComments(css);
  const specifiers = [...clean.matchAll(CSS_IMPORT), ...clean.matchAll(CSS_URL)].map((match) => match[2]);
  return [...new Set(specifiers)].map((specifier) => ({ specifier, kind: "url" }));
}

/** The `<script>` and `<style>` blocks of a Svelte component, HTML comments removed. */
export function svelteBlocks(source) {
  const markup = source.replace(/<!--[\s\S]*?-->/g, (comment) => comment.replace(/[^\n]/g, " "));
  const blocks = (tag) => [...markup.matchAll(new RegExp(`<${tag}(?:\\s[^>]*)?>([\\s\\S]*?)</${tag}>`, "g"))].map((match) => match[1]);
  return { scripts: blocks("script"), styles: blocks("style") };
}

/**
 * The npm packages one app source file references, with what matched:
 * `{ file, package, specifier }`. Bare import specifiers always name a
 * package. A `url(...)`, CSS `@import` or `new URL(..., import.meta.url)`
 * path without ./ is resolved relative to the file first, as Vite does, and
 * names a package only when no such file exists.
 */
export function fileReferences(root, file) {
  const source = readFileSync(join(root, file), "utf8");
  let references;
  if (file.endsWith(".css")) references = cssReferences(source);
  else if (file.endsWith(".svelte")) {
    const { scripts, styles } = svelteBlocks(source);
    references = [...scripts.flatMap((script) => scriptReferences(script, `${file}.ts`)), ...styles.flatMap(cssReferences)];
  } else references = scriptReferences(source, file);
  const packages = references
    .filter(
      ({ specifier, kind }) =>
        isBareSpecifier(specifier) && (kind === "import" || !existsSync(join(root, dirname(file), specifier.split(/[?#]/)[0]))),
    )
    .map(({ specifier }) => ({ file, package: packageNameOfSpecifier(specifier), specifier }));
  // A compiled component imports Svelte's runtime (svelte/internal/...) even
  // when its source imports nothing.
  if (file.endsWith(".svelte")) packages.push({ file, package: "svelte", specifier: "svelte/internal (compiled component)" });
  return packages;
}

/**
 * Is a file under src/ part of the app, as opposed to tests, test support or
 * declarations? Stylesheets count: CSS can pull package files into the bundle.
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

/** Every npm package reference in the app's own code (`isAppSource` files under src/), per file. */
export function appPackageReferences(root) {
  return filesUnder(root, "src")
    .filter(isAppSource)
    .sort()
    .flatMap((file) => fileReferences(root, file));
}

/** The names of the packages the app's own code references. */
export function appImportedPackages(root) {
  return [...new Set(appPackageReferences(root).map((reference) => reference.package))].sort();
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
 * project's runtime `dependencies`, the packages app code references
 * (`appReferences`: names, or appPackageReferences() records), and
 * everything they depend on (`dependencies`, plus `optionalDependencies` and
 * `peerDependencies` that are installed), followed transitively; plus the
 * packages Vite injects code from, without their dependencies. An
 * over-approximation: svelte's compiler dependencies are in it although only
 * its runtime is bundled. Throws, naming the file and the matched text, if an
 * app reference is not in the lockfile.
 */
export function shippedNpmClosure(packages, appReferences) {
  const shipped = new Set();
  const visit = (fromPath, name, required, describe) => {
    const path = resolveLockPath(packages, fromPath, name);
    if (path === null) {
      if (required) throw new Error(describe());
      return;
    }
    if (shipped.has(path)) return;
    shipped.add(path);
    const entry = packages[path];
    for (const dependency of Object.keys(entry.dependencies ?? {})) {
      visit(path, dependency, true, () => `${path} depends on npm package ${dependency}, which package-lock.json does not contain`);
    }
    for (const dependency of Object.keys({ ...entry.optionalDependencies, ...entry.peerDependencies })) visit(path, dependency, false);
  };
  for (const name of Object.keys(packages[""]?.dependencies ?? {})) {
    visit("", name, true, () => `package.json depends on npm package ${name}, which package-lock.json does not contain`);
  }
  for (const reference of appReferences) {
    const {
      file,
      package: name,
      specifier,
    } = typeof reference === "string" ? { file: "src/", package: reference, specifier: reference } : reference;
    visit(
      "",
      name,
      true,
      () =>
        `${file} references npm package ${name} ("${specifier}"), which package-lock.json does not contain. ` +
        "Install it, or if that is a relative path, write it with ./ or fix it.",
    );
  }
  for (const name of VITE_INJECTED_PACKAGES) {
    const path = resolveLockPath(packages, "", name);
    if (path === null) throw new Error(`package-lock.json does not contain ${name}, which injects code into the bundle`);
    shipped.add(path);
  }
  return shipped;
}

/** The shipped closure of this checkout: package-lock.json and the references under src/. */
export function projectShippedNpmClosure(root) {
  const lock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8"));
  return shippedNpmClosure(lock.packages, appPackageReferences(root));
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
