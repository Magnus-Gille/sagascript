// Where a bare `import "name"` from `fromDirectory` would find its package:
// the nearest node_modules/<name> walking up, as ESM resolution does. Unlike
// require.resolve this ignores NODE_PATH and global folders, which ESM
// imports never consult either (#350).
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";

export function findPackageDirectory(fromDirectory, name) {
  let directory = fromDirectory;
  for (;;) {
    const candidate = join(directory, "node_modules", name);
    if (existsSync(join(candidate, "package.json"))) return candidate;
    const parent = dirname(directory);
    if (parent === directory) return null;
    directory = parent;
  }
}
