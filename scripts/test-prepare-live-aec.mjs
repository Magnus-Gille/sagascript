import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import test from "node:test";

import { prepareLiveAec } from "./prepare-live-aec.mjs";

// The helper generates POSIX shell scripts for macOS; Windows cannot execute
// those fixtures. Its platform guard is tested separately on every host.
const posixFixture = { skip: process.platform === "win32" && "POSIX helper fixtures require a Unix host" };

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

function shellQuote(value) {
  return `'${value.replaceAll("'", `\u0027"\u0027"\u0027`)}'`;
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
    'import { writeFileSync } from "node:fs"; writeFileSync(process.env.SAGASCRIPT_TEST_REQUESTED_CARGO_ARGS, [process.env.DYLD_LIBRARY_PATH ?? "", process.env.RUSTC ?? "", process.env.SAGASCRIPT_LIVE_AEC_REAL_RUSTC ?? "", process.argv.slice(2).join(" ")].join("|") + "\\n");',
  );
  const compilerDirectory = join(root, "compiler toolchain", "bin");
  mkdirSync(compilerDirectory, { recursive: true });
  const compilerPath = join(compilerDirectory, "clang");
  const compilerRuntimeDirectory = join(root, `compiler runtime ' $(touch ${join(root, "compiler-runtime-shell-injected")})`);
  const compilerRuntime = join(compilerRuntimeDirectory, "libclang_rt.osx.a");
  mkdirSync(compilerRuntimeDirectory, { recursive: true });
  writeFileSync(compilerRuntime, "fixture compiler runtime archive\n");
  executable(
    compilerPath,
    `if [ "$1" = "-print-file-name=libclang_rt.osx.a" ]; then printf "%s\\n" ${shellQuote(compilerRuntime)}; else echo "unexpected compiler args: $*" >&2; exit 1; fi`,
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
    compilerPath,
    compilerRuntime,
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
    compilerPath: fixtureData.compilerPath,
    manifest: fixtureData.manifest,
    basePath: "/usr/bin",
    ...extra,
  });
}

function captureCargoEnvironment(data, result, overrides = {}, unset = []) {
  const output = join(data.root, "cargo-environment.json");
  nodeExecutable(
    data.cargo,
    'import { writeFileSync } from "node:fs"; writeFileSync(process.env.SAGASCRIPT_TEST_CARGO_ENV, JSON.stringify({ rustflags: process.env.RUSTFLAGS ?? null, encoded: process.env.CARGO_ENCODED_RUSTFLAGS ?? null, rustc: process.env.RUSTC ?? null, realRustc: process.env.SAGASCRIPT_LIVE_AEC_REAL_RUSTC ?? null, rustcWrapper: process.env.RUSTC_WRAPPER ?? null, workspaceWrapper: process.env.RUSTC_WORKSPACE_WRAPPER ?? null, deploymentTarget: process.env.MACOSX_DEPLOYMENT_TARGET ?? null }) + "\\n");',
  );
  const env = { ...process.env, ...result.env, SAGASCRIPT_TEST_CARGO_ENV: output, ...overrides };
  if (!Object.hasOwn(overrides, "RUSTUP_TOOLCHAIN")) delete env.RUSTUP_TOOLCHAIN;
  for (const name of unset) delete env[name];
  const run = spawnSync(result.wrappers.cargoWrapper, ["check", "--locked"], { encoding: "utf8", env });
  assert.equal(run.status, 0, run.stderr);
  return JSON.parse(readFileSync(output, "utf8"));
}

function writeRuntimeArchive(path) {
  rmSync(path, { force: true });
  const object = `${path}.o`;
  const compile = spawnSync("cc", ["-x", "c", "-c", "-o", object, "-"], {
    input: "int sagascript_runtime_archive_fixture(void) { return 1; }\n",
    encoding: "utf8",
  });
  assert.equal(compile.status, 0, compile.stderr);
  const archive = spawnSync("ar", ["crs", path, object], { encoding: "utf8" });
  assert.equal(archive.status, 0, archive.stderr);
}

function parseCapturedRustcArgs(path) {
  return readFileSync(path, "utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));
}

function runCargoLinkSmoke(data) {
  if (process.platform === "win32") return;
  const rustcPath = process.env.RUSTC || "rustc";
  const result = prepare(data, { rustcPath, basePath: process.env.PATH || "", wrapperDir: join(data.root, "cargo-smoke-bin") });
  const hostOutput = spawnSync(result.env.SAGASCRIPT_LIVE_AEC_RUSTC, ["-vV"], { encoding: "utf8" });
  assert.equal(hostOutput.status, 0, hostOutput.stderr);
  const host = hostOutput.stdout.match(/^host: (.+)$/m)?.[1];
  assert.ok(host, "rustc -vV must report a host triple");

  assert.ok(readFileSync(result.wrappers.rustcWrapper, "utf8").includes(`-C ${shellQuote(`link-arg=${data.compilerRuntime}`)}`));

  const project = join(data.root, "cargo smoke project");
  mkdirSync(join(project, ".cargo"), { recursive: true });
  mkdirSync(join(project, "src"), { recursive: true });
  writeFileSync(join(project, "Cargo.toml"), '[package]\nname = "runtime-arg-smoke"\nversion = "0.1.0"\nedition = "2021"\n');
  writeFileSync(join(project, "Cargo.lock"), 'version = 4\n\n[[package]]\nname = "runtime-arg-smoke"\nversion = "0.1.0"\n');
  writeFileSync(
    join(project, "src", "main.rs"),
    'extern "C" { fn sagascript_runtime_archive_fixture() -> i32; }\nfn main() { unsafe { let _ = sagascript_runtime_archive_fixture(); } }\n',
  );

  const configWrapper = join(data.root, "config-rustc-wrapper.mjs");
  nodeExecutable(
    configWrapper,
    'import { appendFileSync } from "node:fs"; import { spawnSync } from "node:child_process"; const args = process.argv.slice(2); appendFileSync(process.env.SAGASCRIPT_TEST_CONFIG_RUSTC_ARGS, JSON.stringify(args) + "\\n"); const run = spawnSync(args[0], args.slice(1), { stdio: "inherit", env: process.env }); process.exit(run.status ?? 1);',
  );
  writeFileSync(
    join(project, ".cargo", "config.toml"),
    `[build]\nrustc-wrapper = ${JSON.stringify(configWrapper)}\n[target.${host}]\nrustflags = ["--cfg", "from_cargo_config"]\n`,
  );

  const workspaceWrapper = join(data.root, "workspace-rustc-wrapper.mjs");
  const workspaceArgs = join(data.root, "workspace-rustc-argv.jsonl");
  nodeExecutable(
    workspaceWrapper,
    'import { appendFileSync } from "node:fs"; import { spawnSync } from "node:child_process"; const args = process.argv.slice(2); appendFileSync(process.env.SAGASCRIPT_TEST_WORKSPACE_RUSTC_ARGS, JSON.stringify(args) + "\\n"); const run = spawnSync(args[0], args.slice(1), { stdio: "inherit", env: process.env }); process.exit(run.status ?? 1);',
  );

  const cases = [
    { name: "config", overrides: {}, expectedCfg: "from_cargo_config" },
    { name: "rustflags", overrides: { RUSTFLAGS: "--cfg from_rustflags" }, expectedCfg: "from_rustflags" },
    {
      name: "encoded",
      overrides: { RUSTFLAGS: "--cfg from_rustflags", CARGO_ENCODED_RUSTFLAGS: "--cfg\x1ffrom_encoded" },
      expectedCfg: "from_encoded",
    },
  ];
  for (const testCase of cases) {
    const argsLog = join(data.root, `rustc-argv-${testCase.name}.jsonl`);
    const env = {
      ...process.env,
      ...result.env,
      SAGASCRIPT_TEST_CONFIG_RUSTC_ARGS: argsLog,
      SAGASCRIPT_TEST_WORKSPACE_RUSTC_ARGS: workspaceArgs,
      RUSTC_WORKSPACE_WRAPPER: workspaceWrapper,
      CARGO_NET_OFFLINE: "true",
      CARGO_TARGET_DIR: join(data.root, `target-${testCase.name}`),
    };
    delete env.RUSTUP_TOOLCHAIN;
    delete env.RUSTC_WRAPPER;
    delete env.RUSTFLAGS;
    delete env.CARGO_ENCODED_RUSTFLAGS;
    Object.assign(env, testCase.overrides);
    const run = spawnSync(result.wrappers.cargoWrapper, ["build", "--locked", "--offline"], { cwd: project, encoding: "utf8", env });
    assert.equal(run.status, 0, `${testCase.name} cargo smoke failed: ${run.stderr}`);
    const configInvocations = parseCapturedRustcArgs(argsLog);
    assert.ok(
      configInvocations.some((args) => args.includes("--crate-name") && args.includes("runtime_arg_smoke")),
      `${testCase.name} build.rustc-wrapper received the crate invocation`,
    );
    const workspaceInvocations = parseCapturedRustcArgs(workspaceArgs);
    assert.ok(
      workspaceInvocations.some((args) => args.includes("--crate-name") && args.includes("runtime_arg_smoke")),
      `${testCase.name} RUSTC_WORKSPACE_WRAPPER received the crate invocation`,
    );
    const configCompileArgs = configInvocations.find((args) => args.includes("--crate-name") && args.includes("runtime_arg_smoke"));
    assert.ok(
      configCompileArgs.includes(testCase.expectedCfg),
      `${testCase.name} retained expected Cargo flags: ${configCompileArgs.join(" ")}`,
    );
  }
}

test("prepares verified local artifacts and wrappers without installing or downloading", posixFixture, () => {
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
    assert.equal(result.env.SAGASCRIPT_LIVE_AEC_COMPILER, data.compilerPath);
    assert.equal(result.env.SAGASCRIPT_LIVE_AEC_COMPILER_RUNTIME, data.compilerRuntime);
    const cargoWrapper = readFileSync(result.wrappers.cargoWrapper, "utf8");
    assert.match(cargoWrapper, /rust_library='[^']+rust-sysroot[^']+lib'/);
    assert.match(cargoWrapper, /rust_cargo='[^']+rust-sysroot[^']+cargo'/);
    assert.match(cargoWrapper, /export RUSTC=/);
    assert.match(readFileSync(result.wrappers.rustcWrapper, "utf8"), /link-arg=/);

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

test("uses the selected CC compiler and resolves its absolute runtime archive", posixFixture, () => {
  const data = fixture();
  const previousCompiler = process.env.CC;
  try {
    process.env.CC = data.compilerPath;
    const result = prepare(data, { compilerPath: undefined });
    assert.equal(result.env.SAGASCRIPT_LIVE_AEC_COMPILER, data.compilerPath);
    assert.equal(result.env.SAGASCRIPT_LIVE_AEC_COMPILER_RUNTIME, data.compilerRuntime);
  } finally {
    if (previousCompiler === undefined) delete process.env.CC;
    else process.env.CC = previousCompiler;
    rmSync(data.root, { recursive: true, force: true });
  }
});

test("fails closed when the selected compiler cannot resolve its runtime archive", posixFixture, () => {
  const data = fixture();
  const missingCompiler = join(data.root, "missing-runtime-clang");
  const missingArchive = join(data.root, "missing toolchain", "libclang_rt.osx.a");
  executable(missingCompiler, 'if [ "$1" = "-print-file-name=libclang_rt.osx.a" ]; then echo "libclang_rt.osx.a"; else exit 1; fi');
  try {
    assert.throws(() => prepare(data, { compilerPath: missingCompiler }), /did not resolve libclang_rt\.osx\.a to an absolute path/);
    executable(
      missingCompiler,
      `if [ "$1" = "-print-file-name=libclang_rt.osx.a" ]; then printf "%s\\n" ${shellQuote(missingArchive)}; else exit 1; fi`,
    );
    assert.throws(() => prepare(data, { compilerPath: missingCompiler }), /C compiler runtime archive is missing/);
  } finally {
    rmSync(data.root, { recursive: true, force: true });
  }
});

test("preserves Cargo flags and wrapper configuration while routing rustc through the compiler shim", posixFixture, () => {
  const data = fixture();
  try {
    const result = prepare(data);
    const previousWrapper = join(data.root, "prior-rustc-wrapper");
    executable(previousWrapper, "exit 0");
    const originalRustflags = "--cfg from_rustflags";
    const originalEncoded = "--cfg\x1ffrom_encoded";
    const observed = captureCargoEnvironment(data, result, {
      RUSTFLAGS: originalRustflags,
      CARGO_ENCODED_RUSTFLAGS: originalEncoded,
      RUSTC_WRAPPER: previousWrapper,
    });
    assert.equal(observed.rustflags, originalRustflags);
    assert.equal(observed.encoded, originalEncoded);
    assert.equal(observed.rustc, result.wrappers.rustcWrapper);
    assert.equal(observed.realRustc, join(data.root, "rust-sysroot", "bin", "rustc"));
    assert.equal(observed.rustcWrapper, previousWrapper);
    assert.equal(observed.deploymentTarget, "13.0");
    assert.equal(existsSync(join(data.root, "compiler-runtime-shell-injected")), false);
  } finally {
    rmSync(data.root, { recursive: true, force: true });
  }
});

test(
  "links through Cargo config/workspace wrappers with a single compiler archive argument",
  {
    skip: process.platform !== "darwin" && "native Cargo linker smoke requires macOS",
  },
  () => {
    const data = fixture();
    try {
      writeRuntimeArchive(data.compilerRuntime);
      runCargoLinkSmoke(data);
      assert.equal(existsSync(join(data.root, "compiler-runtime-shell-injected")), false);
    } finally {
      rmSync(data.root, { recursive: true, force: true });
    }
  },
);

test("honors RUSTUP_TOOLCHAIN through rustup without falling back to PATH rustc", posixFixture, () => {
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
      `${join(data.requestedSysroot, "lib")}|${join(data.root, "bin", "rustc-with-compiler-runtime")}|${data.requestedRustc}|build --offline`,
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

test("fails closed when the requested Rust toolchain is unavailable", posixFixture, () => {
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

test("rejects wrong artifact hashes and never replaces a non-owned wrapper", posixFixture, () => {
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

test("fails closed for non-macOS and missing pinned tools", posixFixture, () => {
  const data = fixture();
  try {
    assert.throws(() => prepareLiveAec({ ...data, platform: "linux" }), /macOS-only/);
    assert.throws(() => prepare(data, { manifest: { ...data.manifest, ninjaVersion: "9.9.9" } }), /Ninja must report version 9.9.9/);
  } finally {
    rmSync(data.root, { recursive: true, force: true });
  }
});

test("rejects a non-empty pkg-config directory", posixFixture, () => {
  const data = fixture();
  try {
    const result = prepare(data);
    writeFileSync(join(result.env.PKG_CONFIG_PATH, "host.pc"), "host artifact");
    assert.throws(() => prepare(data), /pkg-config directory must remain empty/);
  } finally {
    rmSync(data.root, { recursive: true, force: true });
  }
});

test("rejects non-macOS before accessing build dependencies", () => {
  for (const platform of ["win32", "linux"]) {
    assert.throws(() => prepareLiveAec({ platform }), /macOS-only/);
  }
});
