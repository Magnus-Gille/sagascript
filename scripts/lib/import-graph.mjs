// The npm packages a module needs, found by following its relative imports (#358). A test that runs
// before `npm ci` must not reach one even through a repository module it imports, and a test group's
// GROUP_NPM_PACKAGES (scripts/node-test-groups.mjs) must name every package its files reach.
//
// A static scan, not a resolver: it reads `from "x"`, `import "x"` and `import("x")` specifiers, after
// dropping block comments and full-line comments (a comment that mentions `import "name"` is no import).
import { readFileSync } from "node:fs";
import { join, posix } from "node:path";

const BARE_IMPORT = /\bfrom\s*["']([@a-z][\w@./:-]*)["']|\bimport\s*\(?\s*["']([@a-z][\w@./:-]*)["']/g;
const RELATIVE_IMPORT = /\b(?:from|import)\s*\(?\s*["'](\.{1,2}\/[^"']+\.m?js)["']/g;

/** The package a bare specifier names: `@scope/name` or `name`; null for a Node builtin (`node:fs`). */
export function packageOf(specifier) {
  if (specifier.startsWith("node:")) return null;
  return specifier.startsWith("@") ? specifier.split("/").slice(0, 2).join("/") : specifier.split("/")[0];
}

/**
 * `{ packages, files }` reached from `file` (repository relative, forward slashes) under `root`:
 * the bare-import packages of every file on the way, and the files themselves, `file` included.
 */
export function importGraph(file, root) {
  const packages = new Set();
  const files = new Set();
  const visit = (current) => {
    if (files.has(current)) return;
    files.add(current);
    const source = readFileSync(join(root, current), "utf8").replace(/\/\*[\s\S]*?\*\/|^\s*\/\/.*$/gm, "");
    for (const match of source.matchAll(BARE_IMPORT)) {
      const name = packageOf(match[1] ?? match[2]);
      if (name !== null) packages.add(name);
    }
    for (const match of source.matchAll(RELATIVE_IMPORT)) visit(posix.normalize(posix.join(posix.dirname(current), match[1])));
  };
  visit(file);
  return { packages, files };
}
