import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const scripts = fileURLToPath(new URL(".", import.meta.url));
const python = process.env.PYTHON ?? "python3";
const posix = process.platform !== "win32";

test(
  "engine-host conformance reader pipe regressions",
  { skip: posix ? false : "select cannot monitor subprocess pipes on Windows" },
  () => {
    const result = spawnSync(python, [join(scripts, "test-engine-host-conformance-reader.py")], {
      cwd: join(scripts, ".."),
      env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" },
      encoding: "utf8",
      timeout: 15_000,
    });
    assert.equal(
      result.status,
      0,
      `Python reader tests failed (status=${result.status}, signal=${result.signal}):\n${result.stdout}\n${result.stderr}`,
    );
  },
);
