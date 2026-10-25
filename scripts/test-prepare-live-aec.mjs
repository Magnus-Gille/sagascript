import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import test from "node:test";

import { prepareLiveAec } from "./prepare-live-aec.mjs";

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function executable(path, body) {
  writeFileSync(path, `#!/bin/sh\n${body}\n`, { mode: 0o755 });
  chmodSync(path, 0o755);
}

function nodeExecutable(path, body) {
  writeFileSync(path, `#!${process.execPath}\n${body}\n`, { mode: 0o755 });
  chmodSync(path, 0o755);
}

function fixture() {
  const root = mkdtempSync(join(tmpdir(), "sagascript-live-aec-test-"));
  const venv = join(root, "venv");
  mkdirSync(join(venv, "bin"), { recursive: true });
  writeFileSync(join(venv, "pyvenv.cfg"), "home = fixture\n");
  const pythonTarget = join(root, "python-target");
  const python = join(root, "python");
  const meson = join(venv, "bin", "meson");
  const ninja = join(venv, "bin", "ninja");
  executable(pythonTarget, 'echo "Python 3.13.0"');
  symlinkSync(pythonTarget, python);
  executable(meson, 'if [ "$1" = "--version" ]; then echo "1.12.1"; else printf "%s\\n" "$*" > "$SAGASCRIPT_TEST_MESON_ARGS"; fi');
  executable(ninja, 'if [ "$1" = "--version" ]; then echo "1.13.2"; else echo "$*" > "$SAGASCRIPT_TEST_NINJA_ARGS"; fi');
  const rustSysroot = join(root, "rust-sysroot");
  mkdirSync(join(rustSysroot, "bin"), { recursive: true });
  mkdirSync(join(rustSysroot, "lib"), { recursive: true });
  const rustc = join(root, "rustc");
  executable(rustc, `if [ "$1" = "--print" ] && [ "$2" = "sysroot" ]; then echo "${rustSysroot}"; else echo "rustc 1.99.0"; fi`);
  const cargo = join(rustSysroot, "bin", "cargo");
  nodeExecutable(
    cargo,
    'import { writeFileSync } from "node:fs"; writeFileSync(process.env.SAGASCRIPT_TEST_CARGO_ARGS, [process.env.DYLD_LIBRARY_PATH ?? "", process.env.MACOSX_DEPLOYMENT_TARGET ?? "", process.argv.slice(2).join(" ")].join("|") + "\\n");',
  );
  const requestedSysroot = join(root, "requested-rust-sysroot");
  mkdirSync(join(requestedSysroot, "bin"), { recursive: true });
  mkdirSync(join(requestedSysroot, "lib"), { recursive: true });
  const requestedRustc = join(requestedSysroot, "bin", "rustc");
  executable(requestedRustc, 'echo "requested rustc must not be invoked" >&2; exit 1');
  const requestedCargo = join(requestedSysroot, "bin", "cargo");
  nodeExecutable(
    requestedCargo,
    'import { writeFileSync } from "node:fs"; writeFileSync(process.env.SAGASCRIPT_TEST_REQUESTED_CARGO_ARGS, [process.env.DYLD_LIBRARY_PATH ?? "", process.env.RUSTC ?? "", process.argv.slice(2).join(" ")].join("|") + "\\n");',
  );
  const rustup = join(root, "rustup");
  executable(
    rustup,
    `printf "%s" "$*" > "$SAGASCRIPT_TEST_RUSTUP_ARGS"
if [ "$1" = "run" ] && [ "$2" = "requested-toolchain" ] && [ "$3" = "rustc" ] && [ "$4" = "--print" ] && [ "$5" = "sysroot" ]; then
  echo "${requestedSysroot}"
else
  echo "requested toolchain unavailable" >&2
  exit 1
fi`,
  );
  const sourceBytes = Buffer.from("fixture abseil source\n");
  const patchBytes = Buffer.from("fixture wrapdb patch\n");
  const sourceArchive = join(root, "source.tar.gz");
  const patchArchive = join(root, "patch.zip");
  writeFileSync(sourceArchive, sourceBytes);
  writeFileSync(patchArchive, patchBytes);
  return {
    root,
    venv,
    python,
    meson,
    ninja,
    rustc,
    cargo,
    rustup,
    requestedSysroot,
    requestedRustc,
    requestedCargo,
    sourceArchive,
    patchArchive,
    manifest: {
      mesonVersion: "1.12.1",
      ninjaVersion: "1.13.2",
      abseilDirectory: "abseil-cpp-fixture",
      sourceFilename: "abseil-cpp-fixture.tar.gz",
      sourceSha256: sha256(sourceBytes),
      patchFilename: "abseil-cpp-fixture_patch.zip",
      patchUrl: "https://wrapdb.example.invalid/fixture",
      patchSha256: sha256(patchBytes),
    },
  };
}

function prepare(fixtureData, extra = {}) {
  return prepareLiveAec({
    platform: "darwin",
    pythonPath: fixtureData.python,
    venvDir: fixtureData.venv,
    sourceArchive: fixtureData.sourceArchive,
    patchArchive: fixtureData.patchArchive,
    cacheDir: join(fixtureData.root, "cache"),
    wrapperDir: join(fixtureData.root, "bin"),
    rustcPath: fixtureData.rustc,
    manifest: fixtureData.manifest,
    basePath: "/usr/bin",
    ...extra,
  });
}

test("prepares verified local artifacts and wrappers without installing or downloading", () => {
  const data = fixture();
  try {
    const mesonArgs = join(data.root, "meson-args");
    const ninjaArgs = join(data.root, "ninja-args");
    const result = prepare(data, {
      basePath: "/usr/bin",
    });
    assert.equal(result.env.PKG_CONFIG_PATH, result.env.PKG_CONFIG_LIBDIR);
    assert.match(result.env.DYLD_LIBRARY_PATH, /rust-sysroot[\\/]lib/);
    assert.match(result.env.PATH, /bin[\\/]?:\/usr\/bin$/);
    assert.equal(readFileSync(result.artifacts.source, "utf8"), "fixture abseil source\n");
    assert.match(readFileSync(result.wrappers.mesonWrapper, "utf8"), /forcefallback/);
    assert.equal(result.env.DYLD_LIBRARY_PATH, join(data.root, "rust-sysroot", "lib"));
    assert.equal(result.env.MACOSX_DEPLOYMENT_TARGET, "13.0");
    const cargoWrapper = readFileSync(result.wrappers.cargoWrapper, "utf8");
    assert.match(cargoWrapper, /rust_library='[^']+rust-sysroot[^']+lib'/);
    assert.match(cargoWrapper, /rust_cargo='[^']+rust-sysroot[^']+cargo'/);

    const versionRun = spawnSync(result.wrappers.mesonWrapper, ["--version"], { encoding: "utf8" });
    assert.equal(versionRun.status, 0, versionRun.stderr);
    assert.equal(versionRun.stdout.trim(), "1.12.1");

    const mesonRun = spawnSync(result.wrappers.mesonWrapper, ["setup", "build"], {
      env: { ...process.env, SAGASCRIPT_TEST_MESON_ARGS: mesonArgs },
      encoding: "utf8",
    });
    assert.equal(mesonRun.status, 0, mesonRun.stderr);
    assert.match(readFileSync(mesonArgs, "utf8"), /--wrap-mode=forcefallback/);

    const ninjaRun = spawnSync(result.wrappers.ninjaWrapper, ["-C", "build"], {
      env: { ...process.env, SAGASCRIPT_TEST_NINJA_ARGS: ninjaArgs },
      encoding: "utf8",
    });
    assert.equal(ninjaRun.status, 0, ninjaRun.stderr);
    assert.equal(readFileSync(ninjaArgs, "utf8").trim(), "-C build");

    const cargoArgs = join(data.root, "cargo-args");
    const cargoRun = spawnSync(result.wrappers.cargoWrapper, ["build", "--target", "aarch64-apple-darwin"], {
      env: { ...process.env, DYLD_LIBRARY_PATH: "", MACOSX_DEPLOYMENT_TARGET: "27.0", SAGASCRIPT_TEST_CARGO_ARGS: cargoArgs },
      encoding: "utf8",
    });
    assert.equal(cargoRun.status, 0, cargoRun.stderr);
    assert.equal(
      readFileSync(cargoArgs, "utf8").trim(),
      `${join(data.root, "rust-sysroot", "lib")}|13.0|build --target aarch64-apple-darwin`,
    );
  } finally {
    rmSync(data.root, { recursive: true, force: true });
  }
});

test("honors RUSTUP_TOOLCHAIN through rustup without falling back to PATH rustc", () => {
  const data = fixture();
  const previousPath = process.env.PATH;
  const previousToolchain = process.env.RUSTUP_TOOLCHAIN;
  try {
    // Preparation is deliberately performed without a runtime override. The
    // wrapper must resolve a later RUSTUP_TOOLCHAIN itself.
    delete process.env.RUSTUP_TOOLCHAIN;
    const result = prepare(data, { rustcPath: data.rustc });
    assert.equal(result.env.SAGASCRIPT_LIVE_AEC_RUST_SYSROOT, join(data.root, "rust-sysroot"));
    assert.equal(result.env.SAGASCRIPT_LIVE_AEC_RUSTC, data.rustc);
    assert.equal(result.env.DYLD_LIBRARY_PATH, join(data.root, "rust-sysroot", "lib"));

    const rustupArgs = join(data.root, "rustup-args");
    const requestedCargoArgs = join(data.root, "requested-cargo-args");
    const defaultCargoArgs = join(data.root, "default-cargo-args");
    const cargoRun = spawnSync(result.wrappers.cargoWrapper, ["build", "--offline"], {
      env: {
        ...process.env,
        PATH: `${data.root}${delimiter}${previousPath || ""}`,
        RUSTUP_TOOLCHAIN: "requested-toolchain",
        SAGASCRIPT_TEST_RUSTUP_ARGS: rustupArgs,
        SAGASCRIPT_TEST_REQUESTED_CARGO_ARGS: requestedCargoArgs,
        SAGASCRIPT_TEST_CARGO_ARGS: defaultCargoArgs,
      },
      encoding: "utf8",
    });
    assert.equal(cargoRun.status, 0, cargoRun.stderr);
    assert.equal(readFileSync(rustupArgs, "utf8"), "run requested-toolchain rustc --print sysroot");
    assert.equal(
      readFileSync(requestedCargoArgs, "utf8").trim(),
      `${join(data.requestedSysroot, "lib")}|${data.requestedRustc}|build --offline`,
    );
    assert.equal(existsSync(defaultCargoArgs), false, "runtime override must not invoke prepared cargo");
  } finally {
    if (previousPath === undefined) delete process.env.PATH;
    else process.env.PATH = previousPath;
    if (previousToolchain === undefined) delete process.env.RUSTUP_TOOLCHAIN;
    else process.env.RUSTUP_TOOLCHAIN = previousToolchain;
    rmSync(data.root, { recursive: true, force: true });
  }
});

test("fails closed when the requested Rust toolchain is unavailable", () => {
  const data = fixture();
  const previousPath = process.env.PATH;
  const previousToolchain = process.env.RUSTUP_TOOLCHAIN;
  try {
    delete process.env.RUSTUP_TOOLCHAIN;
    const result = prepare(data, { rustcPath: data.rustc });
    const defaultCargoArgs = join(data.root, "default-cargo-args");
    const cargoRun = spawnSync(result.wrappers.cargoWrapper, ["build"], {
      env: {
        ...process.env,
        PATH: `${data.root}${delimiter}${previousPath || ""}`,
        RUSTUP_TOOLCHAIN: "missing-toolchain",
        SAGASCRIPT_TEST_CARGO_ARGS: defaultCargoArgs,
      },
      encoding: "utf8",
    });
    assert.notEqual(cargoRun.status, 0);
    assert.match(cargoRun.stderr, /requested Rust toolchain is unavailable/);
    assert.equal(existsSync(defaultCargoArgs), false, "unavailable override must not invoke prepared cargo");
  } finally {
    if (previousPath === undefined) delete process.env.PATH;
    else process.env.PATH = previousPath;
    if (previousToolchain === undefined) delete process.env.RUSTUP_TOOLCHAIN;
    else process.env.RUSTUP_TOOLCHAIN = previousToolchain;
    rmSync(data.root, { recursive: true, force: true });
  }
});

test("rejects wrong artifact hashes and never replaces a non-owned wrapper", () => {
  const data = fixture();
  try {
    assert.throws(() => prepare(data, { manifest: { ...data.manifest, sourceSha256: "0".repeat(64) } }), /source archive hash mismatch/);
    const result = prepare(data);
    writeFileSync(result.wrappers.mesonWrapper, "user-owned wrapper\n");
    assert.throws(() => prepare(data), /non-owned wrapper/);
    assert.equal(readFileSync(result.wrappers.mesonWrapper, "utf8"), "user-owned wrapper\n");
  } finally {
    rmSync(data.root, { recursive: true, force: true });
  }
});

test("fails closed for non-macOS and missing pinned tools", () => {
  const data = fixture();
  try {
    assert.throws(() => prepareLiveAec({ ...data, platform: "linux" }), /macOS-only/);
    assert.throws(() => prepare(data, { manifest: { ...data.manifest, ninjaVersion: "9.9.9" } }), /Ninja must report version 9.9.9/);
  } finally {
    rmSync(data.root, { recursive: true, force: true });
  }
});

test("rejects a non-empty pkg-config directory", () => {
  const data = fixture();
  try {
    const result = prepare(data);
    writeFileSync(join(result.env.PKG_CONFIG_PATH, "host.pc"), "host artifact");
    assert.throws(() => prepare(data), /pkg-config directory must remain empty/);
  } finally {
    rmSync(data.root, { recursive: true, force: true });
  }
});
