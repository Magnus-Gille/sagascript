#!/usr/bin/env node

import { createHash } from "node:crypto";
import {
  appendFileSync,
  chmodSync,
  closeSync,
  copyFileSync,
  constants as fsConstants,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { spawnSync } from "node:child_process";
import { delimiter, dirname, isAbsolute, join, resolve } from "node:path";

export const LIVE_AEC_MANIFEST = Object.freeze({
  mesonVersion: "1.12.1",
  ninjaVersion: "1.13.2",
  abseilDirectory: "abseil-cpp-20240722.0",
  sourceFilename: "abseil-cpp-20240722.0.tar.gz",
  sourceSha256: "f50e5ac311a81382da7fa75b97310e4b9006474f9560ac46f54a9967f07d4ae3",
  patchFilename: "abseil-cpp_20240722.0-3_patch.zip",
  patchUrl: "https://wrapdb.mesonbuild.com/v2/abseil-cpp_20240722.0-3/get_patch",
  patchSha256: "12dd8df1488a314c53e3751abd2750cf233b830651d168b6a9f15e7d0cf71f7b",
});

const WRAPPER_MARKER = "# sagascript-live-aec-wrapper-v1";

const HELP = `Usage: node scripts/prepare-live-aec.mjs --python PATH --venv PATH [options]

Prepare a task-local, offline WebRTC AEC build environment on macOS.
The Python environment and pinned Meson/Ninja executables must already exist;
this helper never installs packages or downloads artifacts.

Required:
  --python PATH             Explicit Python executable for the prepared venv
  --venv PATH               Existing venv containing Meson ${LIVE_AEC_MANIFEST.mesonVersion} and Ninja ${LIVE_AEC_MANIFEST.ninjaVersion}

Optional:
  --cache-dir PATH          Meson package cache (default: .sagascript-live-aec/package-cache)
  --source-archive PATH     Already-downloaded Abseil source archive to seed the cache
  --patch-archive PATH      Already-downloaded WrapDB patch archive to seed the cache
  --meson PATH              Meson executable (default: VENV/bin/meson)
  --ninja PATH              Ninja executable (default: VENV/bin/ninja)
  --rustc PATH              Rust compiler used to derive the toolchain library path (default: rustc on PATH)
  --compiler PATH           C compiler selected for native objects (default: CC or cc on PATH)
  --wrapper-dir PATH        Task-local PATH wrappers (default: .sagascript-live-aec/bin)
  --github-env PATH         Append prepared variables to this GitHub environment file
  --github-path PATH        Append the wrapper directory to this GitHub PATH file
  --help                    Show this help

Without GitHub output paths, the prepared environment is emitted as JSON.
The emitted environment points both pkg-config search variables at a verified
empty directory and derives DYLD_LIBRARY_PATH from the explicit rustc
--print sysroot result. At wrapper runtime, RUSTUP_TOOLCHAIN is resolved with
\`rustup run TOOLCHAIN rustc --print sysroot\` without installing anything.
The PATH cargo wrapper forwards normal cargo arguments and does not perform
rustup toolchain selection (+toolchain) when the variable is unset.
`;

function fail(message) {
  throw new Error(message);
}

function requirePath(value, name) {
  if (!value) fail(`${name} is required`);
  return resolve(value);
}

function assertRegularFile(path, label, { allowSymlink = false } = {}) {
  let info;
  try {
    info = lstatSync(path);
  } catch {
    fail(`${label} is missing: ${path}`);
  }
  if (info.isSymbolicLink() && allowSymlink) {
    try {
      if (!statSync(path).isFile()) fail(`${label} symlink target must be a file: ${path}`);
    } catch {
      fail(`${label} symlink target is unavailable: ${path}`);
    }
    return;
  }
  if (!info.isFile()) fail(`${label} must be a regular file: ${path}`);
}

function assertExecutable(path, label, options = {}) {
  assertRegularFile(path, label, { allowSymlink: true, ...options });
  if ((statSync(path).mode & 0o111) === 0) fail(`${label} is not executable: ${path}`);
}

function commandVersion(path, label) {
  // The caller owns the process environment. CI supplies a sanitized one when
  // required; this helper does not pretend to provide process isolation.
  const result = spawnSync(path, ["--version"], { encoding: "utf8", timeout: 10_000, env: process.env });
  if (result.error) fail(`could not run ${label} at ${path}: ${result.error.message}`);
  if (result.status !== 0) fail(`${label} at ${path} failed --version: ${(result.stderr || result.stdout || "").trim()}`);
  return `${result.stdout || ""}\n${result.stderr || ""}`.trim();
}

function assertPinnedVersion(path, label, expected) {
  const output = commandVersion(path, label);
  const versions = output.match(/\b\d+\.\d+(?:\.\d+)?\b/g) || [];
  if (!versions.includes(expected)) fail(`${label} must report version ${expected}; got: ${output}`);
  return output;
}

function executableFromPath(value, label, searchPath = process.env.PATH || "") {
  if (value.includes("/")) {
    const path = resolve(value);
    assertExecutable(path, label);
    return path;
  }
  for (const directory of searchPath.split(delimiter).filter(Boolean)) {
    const path = join(directory, value);
    if (existsSync(path)) {
      assertExecutable(path, label);
      return path;
    }
  }
  fail(`${label} was not found on PATH: ${value}`);
}

function rustFromSysroot(sysroot, rustc, label) {
  if (!existsSync(sysroot) || !lstatSync(sysroot).isDirectory()) {
    fail(`${label} returned a missing sysroot: ${sysroot}`);
  }
  const library = join(sysroot, "lib");
  if (!existsSync(library) || !lstatSync(library).isDirectory()) {
    fail(`${label} has no library directory: ${library}`);
  }
  assertExecutable(rustc, "Rust compiler");
  const cargo = join(sysroot, "bin", "cargo");
  assertExecutable(cargo, "Rust cargo");
  return { rustc, sysroot, library, cargo };
}

function rustToolchain(passedPath) {
  const rustc = executableFromPath(passedPath || "rustc", "rustc");
  const result = spawnSync(rustc, ["--print", "sysroot"], {
    encoding: "utf8",
    timeout: 10_000,
    env: process.env,
  });
  if (result.error || result.status !== 0) {
    fail(`could not derive Rust sysroot from ${rustc}: ${(result.stderr || result.error?.message || "unknown error").trim()}`);
  }
  const rawSysroot = (result.stdout || "").trim();
  if (!rawSysroot) fail(`rustc returned an empty sysroot: ${rustc}`);
  const sysroot = resolve(rawSysroot);
  return rustFromSysroot(sysroot, rustc, `rustc ${rustc}`);
}

function compilerRuntimeArchive(compilerPath, searchPath) {
  const compiler = executableFromPath(compilerPath || process.env.CC || "cc", "C compiler", searchPath);
  const result = spawnSync(compiler, ["-print-file-name=libclang_rt.osx.a"], {
    encoding: "utf8",
    timeout: 10_000,
    env: process.env,
  });
  if (result.error) fail(`could not resolve the C compiler runtime archive with ${compiler}: ${result.error.message}`);
  if (result.status !== 0) {
    fail(`C compiler at ${compiler} failed to resolve libclang_rt.osx.a: ${(result.stderr || result.stdout || "").trim()}`);
  }
  const archive = (result.stdout || "").replace(/\r?\n$/, "");
  if (!archive || /[\r\n]/.test(archive) || !isAbsolute(archive)) {
    fail(`C compiler at ${compiler} did not resolve libclang_rt.osx.a to an absolute path`);
  }
  assertRegularFile(archive, "C compiler runtime archive");
  return { compiler, archive };
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function verifyHash(path, expected, label) {
  const actual = sha256(path);
  if (actual !== expected) fail(`${label} hash mismatch: expected ${expected}, got ${actual}`);
}

function ensureDirectory(path, label) {
  mkdirSync(path, { recursive: true });
  if (!lstatSync(path).isDirectory()) fail(`${label} is not a directory: ${path}`);
}

function ensureEmptyDirectory(path, label) {
  ensureDirectory(path, label);
  if (readdirSync(path).length !== 0) fail(`${label} must remain empty: ${path}`);
}

function seedArtifact({ cacheDir, sourcePath, filename, expectedHash, label }) {
  const destination = join(cacheDir, filename);
  if (sourcePath) {
    const source = resolve(sourcePath);
    assertRegularFile(source, `${label} source`);
    verifyHash(source, expectedHash, label);
    if (!existsSync(destination)) {
      try {
        copyFileSync(source, destination, fsConstants.COPYFILE_EXCL);
      } catch (error) {
        if (error?.code !== "EEXIST") throw error;
      }
    } else {
      assertRegularFile(destination, label);
    }
  }
  assertRegularFile(destination, label);
  verifyHash(destination, expectedHash, label);
  return destination;
}

function shellQuote(value) {
  return `'${value.replaceAll("'", `'"'"'`)}'`;
}

function writeOwnedWrapper(path, contents) {
  if (existsSync(path)) {
    assertRegularFile(path, "existing wrapper");
    if (readFileSync(path, "utf8") !== contents) {
      fail(`refusing to overwrite existing non-owned wrapper: ${path}`);
    }
    return;
  }
  let descriptor;
  try {
    descriptor = openSync(path, fsConstants.O_WRONLY | fsConstants.O_CREAT | fsConstants.O_EXCL, 0o755);
    writeFileSync(descriptor, contents);
  } catch (error) {
    if (error?.code === "EEXIST") {
      assertRegularFile(path, "existing wrapper");
      if (readFileSync(path, "utf8") !== contents) {
        fail(`refusing to overwrite existing non-owned wrapper: ${path}`);
      }
      return;
    }
    throw error;
  } finally {
    if (descriptor !== undefined) {
      try {
        closeSync(descriptor);
      } catch {
        // The original write error is more useful than a close failure.
      }
    }
  }
  chmodSync(path, 0o755);
}

function prepareWrappers(wrapperDir, mesonPath, ninjaPath, cargoPath, rustLibrary, compilerRuntime) {
  ensureDirectory(wrapperDir, "wrapper directory");
  const mesonWrapper = join(wrapperDir, "meson");
  const ninjaWrapper = join(wrapperDir, "ninja");
  const cargoWrapper = join(wrapperDir, "cargo");
  // Clippy recognizes Cargo wrapper mode by the compiler basename "rustc".
  // Keep the shim off PATH so direct rustc discovery still finds the toolchain.
  const compilerShimDir = join(wrapperDir, "compiler");
  ensureDirectory(compilerShimDir, "compiler shim directory");
  const rustcWrapper = join(compilerShimDir, "rustc");
  writeOwnedWrapper(
    mesonWrapper,
    `#!/bin/sh\n${WRAPPER_MARKER}\nif [ "\${1-}" = "setup" ]; then\n  shift\n  exec ${shellQuote(mesonPath)} setup --wrap-mode=forcefallback "$@"\nfi\nexec ${shellQuote(mesonPath)} "$@"\n`,
  );
  writeOwnedWrapper(ninjaWrapper, `#!/bin/sh\n${WRAPPER_MARKER}\nexec ${shellQuote(ninjaPath)} "$@"\n`);
  writeOwnedWrapper(
    rustcWrapper,
    `#!/bin/sh
${WRAPPER_MARKER}
rustc=\${SAGASCRIPT_LIVE_AEC_REAL_RUSTC-}
case "\${rustc}" in
  /*) ;;
  *)
    echo "live AEC rustc: selected compiler path is missing or not absolute" >&2
    exit 1
    ;;
esac
if [ ! -x "\${rustc}" ]; then
  echo "live AEC rustc: selected compiler is missing or not executable" >&2
  exit 1
fi
exec "\${rustc}" "$@" -C ${shellQuote(`link-arg=${compilerRuntime}`)}
`,
  );
  writeOwnedWrapper(
    cargoWrapper,
    `#!/bin/sh
${WRAPPER_MARKER}
rust_library=${shellQuote(rustLibrary)}
rust_cargo=${shellQuote(cargoPath)}
rustc_wrapper=${shellQuote(rustcWrapper)}
if [ "\${RUSTUP_TOOLCHAIN+x}" = x ]; then
  if [ -z "\${RUSTUP_TOOLCHAIN}" ]; then
    echo "live AEC cargo: RUSTUP_TOOLCHAIN is empty" >&2
    exit 1
  fi
  newline='
'
  case "\${RUSTUP_TOOLCHAIN}" in
    *"\${newline}"*)
      echo "live AEC cargo: RUSTUP_TOOLCHAIN contains a newline" >&2
      exit 1
      ;;
  esac
  rustup_path=$(command -v rustup 2>/dev/null)
  case "\${rustup_path}" in
    /*) ;;
    *)
      echo "live AEC cargo: rustup is unavailable on PATH" >&2
      exit 1
      ;;
  esac
  if [ ! -x "\${rustup_path}" ]; then
    echo "live AEC cargo: rustup is not executable" >&2
    exit 1
  fi
  runtime_sysroot=$("\${rustup_path}" run "\${RUSTUP_TOOLCHAIN}" rustc --print sysroot 2>/dev/null)
  if [ "$?" -ne 0 ] || [ -z "\${runtime_sysroot}" ]; then
    echo "live AEC cargo: requested Rust toolchain is unavailable" >&2
    exit 1
  fi
  case "\${runtime_sysroot}" in
    /*) ;;
    *)
      echo "live AEC cargo: rustup returned a non-absolute sysroot" >&2
      exit 1
      ;;
  esac
  case "\${runtime_sysroot}" in
    *"\${newline}"*)
      echo "live AEC cargo: rustup returned a sysroot containing a newline" >&2
      exit 1
      ;;
  esac
  rust_library="\${runtime_sysroot}/lib"
  rust_cargo="\${runtime_sysroot}/bin/cargo"
fi
if [ ! -d "\${rust_library}" ]; then
  echo "live AEC cargo: Rust library directory is missing" >&2
  exit 1
fi
if [ ! -x "\${rust_cargo}" ]; then
  echo "live AEC cargo: Rust cargo is missing or not executable" >&2
  exit 1
fi
# Keep Cargo's existing RUSTC_WRAPPER, RUSTC_WORKSPACE_WRAPPER, and
# build.rustc-wrapper configuration intact. Route the selected compiler through
# a transparent rustc shim instead of replacing Cargo's wrapper chain.
export SAGASCRIPT_LIVE_AEC_REAL_RUSTC="\${rust_cargo%/cargo}/rustc"
export RUSTC="\${rustc_wrapper}"
export RUSTDOC="\${rust_cargo%/cargo}/rustdoc"
export DYLD_LIBRARY_PATH="\${rust_library}"
export MACOSX_DEPLOYMENT_TARGET=13.0
exec "\${rust_cargo}" "$@"
`,
  );
  return { mesonWrapper, ninjaWrapper, cargoWrapper, rustcWrapper };
}

function appendGitHubEnv(path, env) {
  const lines = Object.entries(env).map(([key, value]) => {
    if (/[\r\n]/.test(value)) fail(`cannot write multiline GitHub environment value: ${key}`);
    return `${key}=${value}`;
  });
  appendFileSync(path, `${lines.join("\n")}\n`, { encoding: "utf8" });
}

function appendGitHubPath(path, wrapperDir) {
  appendFileSync(path, `${wrapperDir}\n`, { encoding: "utf8" });
}

export function prepareLiveAec({
  pythonPath,
  venvDir,
  cacheDir = join(process.cwd(), ".sagascript-live-aec", "package-cache"),
  wrapperDir = join(process.cwd(), ".sagascript-live-aec", "bin"),
  mesonPath,
  ninjaPath,
  rustcPath,
  compilerPath,
  sourceArchive,
  patchArchive,
  pkgConfigDir,
  githubEnv,
  githubPath,
  platform = process.platform,
  basePath = process.env.PATH || "",
  manifest = LIVE_AEC_MANIFEST,
} = {}) {
  if (platform !== "darwin") fail(`live AEC preparation is macOS-only (current platform: ${platform})`);
  const python = requirePath(pythonPath, "--python");
  const venv = requirePath(venvDir, "--venv");
  assertExecutable(python, "Python");
  commandVersion(python, "Python");
  assertRegularFile(join(venv, "pyvenv.cfg"), "venv marker");

  const meson = resolve(mesonPath || join(venv, "bin", "meson"));
  const ninja = resolve(ninjaPath || join(venv, "bin", "ninja"));
  assertExecutable(meson, "Meson");
  assertExecutable(ninja, "Ninja");
  const mesonVersion = assertPinnedVersion(meson, "Meson", manifest.mesonVersion);
  const ninjaVersion = assertPinnedVersion(ninja, "Ninja", manifest.ninjaVersion);
  const rust = rustToolchain(rustcPath);
  const compiler = compilerRuntimeArchive(compilerPath, basePath);

  const cache = resolve(cacheDir);
  ensureDirectory(cache, "Meson package cache");
  const pkgConfig = resolve(pkgConfigDir || join(dirname(cache), "pkg-config-empty"));
  ensureEmptyDirectory(pkgConfig, "pkg-config directory");
  const source = seedArtifact({
    cacheDir: cache,
    sourcePath: sourceArchive,
    filename: manifest.sourceFilename,
    expectedHash: manifest.sourceSha256,
    label: "Abseil source archive",
  });
  const patch = seedArtifact({
    cacheDir: cache,
    sourcePath: patchArchive,
    filename: manifest.patchFilename,
    expectedHash: manifest.patchSha256,
    label: "Abseil WrapDB patch archive",
  });

  const wrappers = prepareWrappers(resolve(wrapperDir), meson, ninja, rust.cargo, rust.library, compiler.archive);
  const preparedPath = `${resolve(wrapperDir)}${delimiter}${basePath}`;
  const env = {
    PATH: preparedPath,
    VIRTUAL_ENV: venv,
    MESON_PACKAGE_CACHE_DIR: cache,
    PKG_CONFIG_PATH: pkgConfig,
    PKG_CONFIG_LIBDIR: pkgConfig,
    // The cargo wrapper restores this exact sysroot path for nested /usr/bin/env
    // invocations, where the caller's DYLD_LIBRARY_PATH may be stripped.
    DYLD_LIBRARY_PATH: rust.library,
    MACOSX_DEPLOYMENT_TARGET: "13.0",
    SAGASCRIPT_LIVE_AEC_PYTHON: python,
    SAGASCRIPT_LIVE_AEC_MESON: meson,
    SAGASCRIPT_LIVE_AEC_NINJA: ninja,
    SAGASCRIPT_LIVE_AEC_CARGO: rust.cargo,
    SAGASCRIPT_LIVE_AEC_RUSTC: rust.rustc,
    SAGASCRIPT_LIVE_AEC_RUST_SYSROOT: rust.sysroot,
    SAGASCRIPT_LIVE_AEC_COMPILER: compiler.compiler,
    SAGASCRIPT_LIVE_AEC_COMPILER_RUNTIME: compiler.archive,
  };
  if (githubEnv) appendGitHubEnv(resolve(githubEnv), env);
  if (githubPath) appendGitHubPath(resolve(githubPath), resolve(wrapperDir));

  return {
    env,
    wrappers,
    artifacts: { source, patch },
    versions: { meson: mesonVersion, ninja: ninjaVersion },
    patchUrl: manifest.patchUrl,
  };
}

function parseArgs(argv) {
  const options = {};
  const takesValue = new Set([
    "python",
    "venv",
    "cache-dir",
    "source-archive",
    "patch-archive",
    "meson",
    "ninja",
    "rustc",
    "compiler",
    "wrapper-dir",
    "github-env",
    "github-path",
  ]);
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--help" || arg === "-h") return { help: true };
    if (!arg.startsWith("--")) fail(`unknown argument: ${arg}`);
    const [rawName, inlineValue] = arg.slice(2).split("=", 2);
    if (!takesValue.has(rawName)) fail(`unknown option: --${rawName}`);
    const value = inlineValue ?? argv[++index];
    if (!value || value.startsWith("--")) fail(`--${rawName} requires a value`);
    options[rawName.replaceAll("-", "")] = value;
  }
  return options;
}

function main() {
  const parsed = parseArgs(process.argv.slice(2));
  if (parsed.help) {
    process.stdout.write(HELP);
    return;
  }
  const result = prepareLiveAec({
    pythonPath: parsed.python,
    venvDir: parsed.venv,
    cacheDir: parsed.cachedir,
    sourceArchive: parsed.sourcearchive,
    patchArchive: parsed.patcharchive,
    mesonPath: parsed.meson,
    ninjaPath: parsed.ninja,
    rustcPath: parsed.rustc,
    compilerPath: parsed.compiler,
    wrapperDir: parsed.wrapperdir,
    githubEnv: parsed.githubenv,
    githubPath: parsed.githubpath,
  });
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}

if (process.argv[1] && import.meta.url === new URL(`file://${resolve(process.argv[1])}`).href) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`prepare-live-aec: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}
