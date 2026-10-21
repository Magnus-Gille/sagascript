import { statSync, statfsSync } from "node:fs";
import { dirname, resolve } from "node:path";

export const BLOCKED_LINUX_FILESYSTEMS = new Map([
  [0x01021994, "tmpfs"],
  [0x858458f6, "ramfs"],
  [0x958458f6, "hugetlbfs"],
]);

function existingParent(path, stat = statSync) {
  let candidate = resolve(path);
  for (;;) {
    try {
      stat(candidate);
      return candidate;
    } catch (error) {
      if (!error || !["ENOENT", "ENOTDIR"].includes(error.code)) throw error;
      if (candidate === dirname(candidate)) return null;
      candidate = dirname(candidate);
    }
  }
}

function failure(path, location, detail) {
  return {
    path,
    location,
    message: `${path} resolves through ${location}: ${detail}`,
    fix: `Use a disk-backed location for this build (for example set CARGO_TARGET_DIR=/path/to/disk/target) and run verify again.`,
  };
}

/**
 * Check explicit build paths on Linux. statfsSync follows symlinks, so an
 * absent target below a symlink is checked on the symlink's mounted parent.
 * Other platforms deliberately skip this host-specific check.
 */
export function probeBuildStorage(paths, { platform = process.platform, stat = statSync, statfs = statfsSync } = {}) {
  if (platform !== "linux") return { ok: true, checked: [], failures: [] };

  const checked = [];
  const failures = [];
  for (const input of [...new Set(paths.map((path) => resolve(path)))]) {
    let location;
    try {
      location = existingParent(input, stat);
    } catch (error) {
      failures.push(failure(input, input, `path probe failed: ${error?.message ?? error}`));
      continue;
    }
    if (!location) {
      failures.push(failure(input, input, "no existing parent could be inspected"));
      continue;
    }
    try {
      const filesystem = statfs(location);
      if (filesystem?.type === undefined || filesystem.type === null) throw new Error("filesystem probe returned no type");
      const type = Number(filesystem.type);
      if (!Number.isFinite(type)) throw new Error("filesystem probe returned an invalid type");
      checked.push({ path: input, location, type });
      const name = BLOCKED_LINUX_FILESYSTEMS.get(type);
      if (name) failures.push(failure(input, location, `filesystem ${name} is memory-backed`));
    } catch (error) {
      failures.push(failure(input, location, `filesystem probe failed: ${error?.message ?? error}`));
    }
  }
  return { ok: failures.length === 0, checked, failures };
}
