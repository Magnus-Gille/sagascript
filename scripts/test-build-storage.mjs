import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, statSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { probeBuildStorage } from "./lib/build-storage.mjs";
import { CHECKS, preflight } from "./verify.mjs";

const TMPFS = 0x01021994;
const EXT4 = 0xef53;

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "sagascript-storage-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}

function metadataStub({ target, build, fail = false } = {}) {
  const calls = [];
  const runner = (command, args, options) => {
    calls.push({ command, args, ...options });
    if (fail) return { ok: false, stdout: "", stderr: "metadata unavailable", error: null };
    const metadata = {
      target_directory: target ?? resolve(options.cwd, options.env?.CARGO_TARGET_DIR ?? ".cargo-config-target"),
    };
    if (build !== undefined) metadata.build_directory = build;
    return { ok: true, stdout: JSON.stringify(metadata), stderr: "", error: null };
  };
  return { calls, runner };
}

test("rejects an absent target through a symlink mounted on tmpfs", (t) => {
  const root = fixture(t);
  const mounted = join(root, "mounted");
  mkdirSync(mounted);
  const link = join(root, "target-link");
  symlinkSync(mounted, link);
  const calls = [];
  const result = probeBuildStorage([join(link, "target")], {
    platform: "linux",
    stat: statSync,
    statfs: (path) => {
      calls.push(path);
      return { type: TMPFS };
    },
  });
  assert.equal(result.ok, false);
  assert.equal(calls[0], resolve(link));
  assert.match(result.failures[0].message, /tmpfs/);
  assert.match(result.failures[0].fix, /CARGO_TARGET_DIR/);
});

test("checks the nearest existing parent when the target directory is absent", (t) => {
  const root = fixture(t);
  const target = join(root, "missing", "target");
  const calls = [];
  const result = probeBuildStorage([target], {
    platform: "linux",
    stat: statSync,
    statfs: (path) => {
      calls.push(path);
      return { type: EXT4 };
    },
  });
  assert.equal(result.ok, true);
  assert.equal(calls[0], root);
});

test("accepts disk-backed storage and fails closed when statfs fails", (t) => {
  const root = fixture(t);
  assert.equal(probeBuildStorage([root], { platform: "linux", stat: statSync, statfs: () => ({ type: EXT4 }) }).ok, true);
  const failed = probeBuildStorage([root], {
    platform: "linux",
    stat: statSync,
    statfs: () => {
      throw new Error("permission denied");
    },
  });
  assert.equal(failed.ok, false);
  assert.match(failed.failures[0].message, /permission denied/);
});

test("fails closed for invalid filesystem types and non-ENOENT stat errors", (t) => {
  const root = fixture(t);
  const invalid = probeBuildStorage([root], { platform: "linux", stat: statSync, statfs: () => ({}) });
  assert.equal(invalid.ok, false);
  assert.match(invalid.failures[0].message, /no type/);
  const denied = probeBuildStorage([join(root, "missing")], {
    platform: "linux",
    stat: () => {
      const error = new Error("permission denied");
      error.code = "EACCES";
      throw error;
    },
    statfs: () => ({ type: EXT4 }),
  });
  assert.equal(denied.ok, false);
  assert.match(denied.failures[0].message, /permission denied/);
});

test("rejects ramfs and hugetlbfs", (t) => {
  const root = fixture(t);
  for (const [type, name] of [
    [0x858458f6, "ramfs"],
    [0x958458f6, "hugetlbfs"],
  ]) {
    const result = probeBuildStorage([root], { platform: "linux", stat: statSync, statfs: () => ({ type }) });
    assert.equal(result.ok, false);
    assert.match(result.failures[0].message, new RegExp(name));
  }
});

test("does not inspect storage on non-Linux platforms", () => {
  const result = probeBuildStorage(["/definitely/absent"], {
    platform: "darwin",
    statfs: () => {
      throw new Error("must not be called");
    },
  });
  assert.equal(result.ok, true);
});

test("skips metadata for fmt and frontend-only checks", async () => {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const prerequisites = { rust: { label: "Rust", probe: async () => ({ ok: true }) } };
  const values = {};
  const fmt = CHECKS.find((check) => check.id === "fmt");
  const fmtResult = await preflight([fmt], { root, platform: "linux", prerequisites, values });
  assert.deepEqual(fmtResult.missing, []);

  const frontendResult = await preflight([{ id: "frontend", steps: [{ run: "npm run test:unit" }], requires: [] }], {
    root: join(root, "does-not-exist"),
    platform: "linux",
    prerequisites,
    values: {},
  });
  assert.deepEqual(frontendResult.missing, []);
});

test("uses Cargo metadata output and rejects a configured build directory on tmpfs", async (t) => {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const target = fixture(t);
  const build = fixture(t);
  const metadata = metadataStub({ target, build });
  const result = await preflight([{ id: "cargo-config-target", steps: [{ run: "cargo check", cwd: "src-tauri" }], requires: [] }], {
    root,
    platform: "linux",
    prerequisites: {},
    values: {},
    metadataRunner: metadata.runner,
    storageProbe: (paths, options) => probeBuildStorage(paths, { ...options, statfs: (path) => ({ type: path === build ? TMPFS : EXT4 }) }),
  });
  assert.equal(result.missing.length, 1);
  assert.match(result.missing[0].missing, /tmpfs/);
  assert.match(result.missing[0].fix, /CARGO_TARGET_DIR/);
  assert.equal(metadata.calls[0].command, "cargo");
  assert.deepEqual(metadata.calls[0].args, ["metadata", "--no-deps", "--format-version", "1", "--offline", "--locked"]);
  assert.equal(metadata.calls[0].cwd, join(root, "src-tauri"));
});

test("per-step CARGO_TARGET_DIR overrides the process environment", async (t) => {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const processTarget = fixture(t);
  const stepTarget = fixture(t);
  const oldTarget = process.env.CARGO_TARGET_DIR;
  process.env.CARGO_TARGET_DIR = processTarget;
  const checked = [];
  const metadata = metadataStub();
  try {
    const result = await preflight(
      [
        {
          id: "cargo-step-target",
          steps: [{ run: "cargo check", cwd: "src-tauri", env: { CARGO_TARGET_DIR: stepTarget, CARGO_BUILD_BUILD_DIR: "../build-dir" } }],
          requires: [],
        },
      ],
      {
        root,
        platform: "linux",
        prerequisites: {},
        values: {},
        metadataRunner: metadata.runner,
        storageProbe: (paths, options) => {
          checked.push(...paths);
          return probeBuildStorage(paths, { ...options, statfs: () => ({ type: EXT4 }) });
        },
      },
    );
    assert.deepEqual(result.missing, []);
    assert.ok(checked.includes(stepTarget));
    assert.ok(!checked.includes(processTarget));
    assert.ok(checked.includes(resolve(root, "build-dir")));
    assert.equal(metadata.calls[0].env.CARGO_TARGET_DIR, stepTarget);
  } finally {
    if (oldTarget === undefined) delete process.env.CARGO_TARGET_DIR;
    else process.env.CARGO_TARGET_DIR = oldTarget;
  }
});

test("marks the clippy policy wrapper as a Cargo workspace check", async (t) => {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const target = fixture(t);
  const metadata = metadataStub({ target });
  const policy = CHECKS.find((check) => check.id === "clippy-policy");
  const result = await preflight([policy], {
    root,
    platform: "linux",
    prerequisites: {
      rust: { label: "Rust", probe: async () => ({ ok: true }) },
      "linux-libs": { label: "Linux libraries", probe: async () => ({ ok: true }) },
      "frontend-dist": { label: "dist", probe: async () => ({ ok: true }) },
    },
    values: {},
    metadataRunner: metadata.runner,
    storageProbe: (paths, options) =>
      probeBuildStorage(paths, { ...options, statfs: (path) => ({ type: path === target ? TMPFS : EXT4 }) }),
  });
  assert.equal(result.missing.length, 1);
  assert.match(result.missing[0].missing, /tmpfs/);
  assert.match(result.missing[0].neededBy.join(","), /clippy-policy/);
  assert.equal(metadata.calls[0].cwd, join(root, "src-tauri"));
});

test("skips metadata when Rust is already missing and reports metadata/probe failures", async (t) => {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const rustMetadata = metadataStub();
  const rustMissing = await preflight(
    [{ id: "cargo-rust-missing", steps: [{ run: "cargo check", cwd: "src-tauri" }], requires: ["rust"] }],
    {
      root,
      platform: "linux",
      prerequisites: { rust: { label: "Rust", probe: async () => ({ ok: false, missing: "rust missing" }) } },
      values: {},
      metadataRunner: rustMetadata.runner,
    },
  );
  assert.match(rustMissing.missing[0].missing, /rust missing/);
  assert.equal(rustMetadata.calls.length, 0);

  const metadataFailure = await preflight(
    [{ id: "cargo-metadata-failure", steps: [{ run: "cargo check", cwd: "src-tauri" }], requires: [] }],
    { root, platform: "linux", prerequisites: {}, values: {}, metadataRunner: metadataStub({ fail: true }).runner },
  );
  assert.match(metadataFailure.missing[0].missing, /metadata unavailable/);
  assert.match(metadataFailure.missing[0].fix, /cargo fetch --locked --manifest-path src-tauri\/Cargo\.toml/);

  const probeMetadata = metadataStub({ target: fixture(t) });
  const probeFailure = await preflight([{ id: "cargo-probe-failure", steps: [{ run: "cargo check", cwd: "src-tauri" }], requires: [] }], {
    root,
    platform: "linux",
    prerequisites: {},
    values: {},
    metadataRunner: probeMetadata.runner,
    storageProbe: (paths, options) =>
      probeBuildStorage(paths, {
        ...options,
        statfs: () => {
          throw new Error("statfs unavailable");
        },
      }),
  });
  assert.match(probeFailure.missing[0].missing, /statfs unavailable/);
});
