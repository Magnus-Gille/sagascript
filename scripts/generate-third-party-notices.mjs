import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { appPackageReferences, npmLicenseError, shippedNpmClosure } from "./lib/npm-license-policy.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outputPath = join(root, "THIRD_PARTY_NOTICES.md");
const mode = process.argv[2];

function compareCodeUnits(a, b) {
  return a < b ? -1 : a > b ? 1 : 0;
}

function normalizeNewlines(text) {
  return text.replace(/\r\n?/g, "\n");
}

function npmInstallationError(pkg, installedVersion, directoryExists) {
  const path = relative(root, pkg.directory);
  if (!directoryExists) {
    return pkg.optional ? null : `npm dependency directory is missing: ${path} (run npm ci)`;
  }
  if (typeof installedVersion !== "string") {
    return `npm dependency manifest has no version: ${path}/package.json (run npm ci)`;
  }
  if (installedVersion !== pkg.version) {
    return `npm dependency version mismatch for ${pkg.name}: lockfile requires ${pkg.version}, installed ${installedVersion} in ${path} (run npm ci)`;
  }
  return null;
}

/** The inventory record for one `packages` entry of package-lock.json (v3). */
function npmPackageFromLock(path, entry, shipped) {
  const name = path.slice(path.lastIndexOf("node_modules/") + "node_modules/".length);
  return {
    ecosystem: "npm",
    name,
    version: entry.version,
    license: entry.license,
    source: componentSource({ ...entry, name }, "npm"),
    directory: join(root, path),
    optional: entry.optional === true,
    devOnly: entry.dev === true,
    // In the closure of what the app bundle can contain (see
    // scripts/lib/npm-license-policy.mjs: "dev" does not mean "not shipped").
    shipped: shipped.has(path),
  };
}

if (mode === "--test-sort") {
  const fixture = ["z", "ä", "a", "å", "A"].sort(compareCodeUnits);
  const expected = ["A", "a", "z", "ä", "å"];
  if (fixture.join("\0") !== expected.join("\0")) {
    throw new Error(`Unexpected deterministic sort order: ${fixture.join(", ")}`);
  }
  console.log(createHash("sha256").update(fixture.join("\n")).digest("hex"));
  process.exit(0);
}

if (mode === "--test-npm-installation") {
  const pkg = {
    name: "example",
    version: "2.0.0",
    optional: false,
    directory: join(root, "node_modules/example"),
  };
  const cases = [
    [npmInstallationError(pkg, "2.0.0", true), null],
    [
      npmInstallationError(pkg, "1.0.0", true),
      "npm dependency version mismatch for example: lockfile requires 2.0.0, installed 1.0.0 in node_modules/example (run npm ci)",
    ],
    [npmInstallationError(pkg, undefined, false), "npm dependency directory is missing: node_modules/example (run npm ci)"],
    [npmInstallationError({ ...pkg, optional: true }, undefined, false), null],
    [npmInstallationError(pkg, undefined, true), "npm dependency manifest has no version: node_modules/example/package.json (run npm ci)"],
  ];
  for (const [actual, expected] of cases) {
    if (actual !== expected) {
      throw new Error(`Unexpected npm installation validation result: ${actual}`);
    }
  }
  console.log("npm installation validation passed");
  process.exit(0);
}

if (mode === "--test-newline-normalization") {
  const lf = "first\nsecond\n";
  for (const candidate of [lf, "first\r\nsecond\r\n", "first\rsecond\r"]) {
    if (normalizeNewlines(candidate) !== lf) {
      throw new Error("Generated notice comparison is not newline-stable");
    }
  }
  console.log("newline normalization passed");
  process.exit(0);
}

if (mode === "--test-dev-only-licenses") {
  // A synthetic package-lock.json (v3). The project depends on runtime-dep,
  // which needs nested (installed under it) and hoisted; app code under src/
  // imports entities, a dev dependency (as svelte is); tooling is a dev
  // dependency nothing imports; opt is devOptional; vite injects its
  // module-preload polyfill but its own dependencies stay build-time only.
  const packages = {
    "": { dependencies: { "runtime-dep": "^1.0.0" }, devDependencies: { entities: "^7.0.0", tooling: "^1.0.0", vite: "^6.0.0" } },
    "node_modules/runtime-dep": { version: "1.0.0", license: "MIT", dependencies: { nested: "^2.0.0", hoisted: "^1.0.0" } },
    "node_modules/runtime-dep/node_modules/nested": { version: "2.0.0", license: "BlueOak-1.0.0" },
    "node_modules/nested": { version: "1.0.0", license: "BSD-2-Clause", dev: true },
    "node_modules/hoisted": { version: "1.0.0", license: "BSD-2-Clause" },
    "node_modules/entities": { version: "7.0.1", license: "BSD-2-Clause", dev: true },
    "node_modules/tooling": { version: "1.0.0", license: "BSD-2-Clause", dev: true, dependencies: { "tooling-dep": "^1.0.0" } },
    "node_modules/tooling-dep": { version: "1.0.0", license: "BlueOak-1.0.0", dev: true },
    "node_modules/opt": { version: "1.0.0", license: "BSD-2-Clause", devOptional: true, optional: true },
    "node_modules/vite": { version: "6.4.3", license: "MIT", dev: true, dependencies: { rollup: "^4.0.0" } },
    "node_modules/rollup": { version: "4.0.0", license: "BlueOak-1.0.0", dev: true },
    "node_modules/gpl-tool": { version: "1.0.0", license: "GPL-3.0-only", dev: true },
    "node_modules/compound": { version: "1.0.0", license: "BSD-2-Clause OR MIT", dev: true },
    "node_modules/unlicensed": { version: "1.0.0", dev: true },
  };
  const shipped = shippedNpmClosure(packages, ["entities"]);
  const expectedShipped = [
    "node_modules/entities",
    "node_modules/hoisted",
    "node_modules/runtime-dep",
    "node_modules/runtime-dep/node_modules/nested",
    "node_modules/vite",
  ];
  if ([...shipped].sort().join("\n") !== expectedShipped.join("\n")) {
    throw new Error(`Unexpected shipped closure: ${[...shipped].sort().join(", ")}`);
  }
  const shippedError = (id, license) =>
    `npm ${id} has license ${license}, which is reviewed for dev-only tooling only; the app bundle can contain this package (src/ or a runtime dependency reaches it)`;
  const expected = {
    "node_modules/runtime-dep": null,
    "node_modules/runtime-dep/node_modules/nested": shippedError("nested@2.0.0", "BlueOak-1.0.0"),
    "node_modules/nested": null,
    "node_modules/hoisted": shippedError("hoisted@1.0.0", "BSD-2-Clause"),
    // Imported by app code: shipped, although "dev": true.
    "node_modules/entities": shippedError("entities@7.0.1", "BSD-2-Clause"),
    "node_modules/tooling": null,
    "node_modules/tooling-dep": null,
    "node_modules/opt":
      "npm opt@1.0.0 has license BSD-2-Clause, which is reviewed for dev-only tooling only; this package is not a dev dependency",
    "node_modules/vite": null,
    "node_modules/rollup": null,
    "node_modules/gpl-tool": "npm gpl-tool@1.0.0 has unreviewed license: GPL-3.0-only",
    "node_modules/compound": "npm compound@1.0.0 has unreviewed license: BSD-2-Clause OR MIT",
    "node_modules/unlicensed": "npm unlicensed@1.0.0 has no SPDX license",
  };
  for (const [path, error] of Object.entries(expected)) {
    const actual = npmLicenseError(npmPackageFromLock(path, packages[path], shipped));
    if (actual !== error) throw new Error(`Unexpected npm license result for ${path}: ${actual}`);
  }
  let unresolved = null;
  try {
    shippedNpmClosure(packages, [{ file: "src/main.ts", package: "not-installed", specifier: "not-installed/x.css" }]);
  } catch (error) {
    unresolved = error.message;
  }
  if (!unresolved?.startsWith('src/main.ts references npm package not-installed ("not-installed/x.css"), which package-lock.json')) {
    throw new Error(`An app reference missing from the lockfile must fail and name its file: ${unresolved}`);
  }
  console.log("dev-only npm license policy passed");
  process.exit(0);
}

if (!new Set(["--check", "--write", "--test-live-aec-licenses"]).has(mode)) {
  console.error(
    "Usage: node scripts/generate-third-party-notices.mjs --check|--write|--test-sort|--test-npm-installation|--test-newline-normalization|--test-dev-only-licenses|--test-live-aec-licenses",
  );
  process.exit(2);
}

// This is an intentionally reviewed allowlist, not a permissive pattern. A new
// expression stops the release until somebody examines the dependency and adds
// the exact SPDX expression here.
const reviewedRustLicenses = new Set([
  "(Apache-2.0 OR MIT) AND BSD-3-Clause",
  "(MIT OR Apache-2.0) AND Unicode-3.0",
  "0BSD OR MIT OR Apache-2.0",
  "Apache-2.0",
  "Apache-2.0 / MIT",
  "Apache-2.0 AND ISC",
  "Apache-2.0 AND MIT",
  "Apache-2.0 OR BSL-1.0",
  "Apache-2.0 OR ISC OR MIT",
  "Apache-2.0 OR MIT",
  "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT",
  "Apache-2.0/MIT",
  "BSD-2-Clause OR Apache-2.0 OR MIT",
  "BSD-2-Clause OR MIT OR Apache-2.0",
  "BSD-3-Clause",
  "BSD-3-Clause AND MIT",
  "BSD-3-Clause OR Apache-2.0",
  "BSD-3-Clause/MIT",
  "CC0-1.0",
  "CC0-1.0 OR MIT-0 OR Apache-2.0",
  "CDLA-Permissive-2.0",
  "ISC",
  "MIT",
  "MIT / Apache-2.0",
  "MIT OR Apache-2.0",
  "MIT OR Apache-2.0 OR Zlib",
  "MIT OR Zlib OR Apache-2.0",
  "MIT/Apache-2.0",
  "MPL-2.0",
  "Unicode-3.0",
  "Unlicense",
  "Unlicense OR MIT",
  "Unlicense/MIT",
  "Zlib",
  "Zlib OR Apache-2.0 OR MIT",
]);

function cargoMetadata(target, manifestPath = "src-tauri/Cargo.toml") {
  return JSON.parse(
    execFileSync("cargo", ["metadata", "--format-version", "1", "--locked", "--filter-platform", target, "--manifest-path", manifestPath], {
      cwd: root,
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      stdio: ["ignore", "pipe", "inherit"],
    }),
  );
}

function componentSource(pkg, ecosystem) {
  if (pkg.repository) return pkg.repository;
  if (pkg.homepage) return pkg.homepage;
  if (ecosystem === "Rust") {
    return `https://crates.io/crates/${pkg.name}/${pkg.version}`;
  }
  return `https://www.npmjs.com/package/${pkg.name}/v/${pkg.version}`;
}

function rootLicenseFiles(directory) {
  if (!existsSync(directory)) return [];
  return readdirSync(directory)
    .filter((name) => /^(licen[sc]e|copying|notice)([._-].*)?$/i.test(name))
    .map((name) => join(directory, name))
    .filter((path) => statSync(path).isFile())
    .sort(compareCodeUnits);
}

const liveAecLicenseOverrides = new Map([
  ["webrtc-audio-processing", "6db9d4840c6ff0fb3c37ace8e32cfd3eb6c6e759c67b4ecbee969cd8d75154a9"],
  ["webrtc-audio-processing-config", "9b79539028e216e813e152d45f5c1ed5fdd0554426ad50270fb03134e7082dac"],
  ["webrtc-audio-processing-sys", "9b79539028e216e813e152d45f5c1ed5fdd0554426ad50270fb03134e7082dac"],
]);

function rustLicenseOverride(pkg) {
  const expectedHash = liveAecLicenseOverrides.get(pkg.name);
  if (!expectedHash) return { license: pkg.license };
  if (pkg.version !== "2.1.0") {
    return { error: `Rust ${pkg.name}@${pkg.version} has unexpected version for reviewed license_file=COPYING override` };
  }
  if (pkg.license && pkg.license !== "BSD-3-Clause") {
    return { error: `Rust ${pkg.name}@${pkg.version} has unexpected SPDX license for reviewed license_file=COPYING override` };
  }
  const copying = join(pkg.directory, "COPYING");
  if (!existsSync(copying)) {
    return { error: `Rust ${pkg.name}@${pkg.version} is missing reviewed license_file=COPYING` };
  }
  const digest = createHash("sha256").update(readFileSync(copying)).digest("hex");
  if (digest !== expectedHash) {
    return { error: `Rust ${pkg.name}@${pkg.version} COPYING hash mismatch for reviewed license_file override` };
  }
  return { license: "BSD-3-Clause" };
}

function normalizeLicenseContent(path) {
  return readFileSync(path, "utf8")
    .replace(/\r\n?/g, "\n")
    .split("\n")
    .map((line) => line.trimEnd())
    .join("\n")
    .trim();
}

function readLiveAecLicenseTexts() {
  const directory = join(root, "third-party/live-aec");
  const manifestPath = join(directory, "manifest.json");
  if (!existsSync(manifestPath)) throw new Error("live AEC legal-text manifest is missing");
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  if (manifest.schema_version !== 1 || !Array.isArray(manifest.files) || manifest.files.length !== 9) {
    throw new Error("live AEC legal-text manifest has an unexpected schema or file count");
  }
  const seen = new Set();
  return manifest.files.map((record) => {
    if (
      typeof record.filename !== "string" ||
      typeof record.sha256 !== "string" ||
      seen.has(record.filename) ||
      !/^[a-f0-9]{64}$/.test(record.sha256)
    ) {
      throw new Error("live AEC legal-text manifest has an invalid or duplicate file record");
    }
    seen.add(record.filename);
    const path = resolve(directory, record.filename);
    const relativePath = relative(directory, path);
    if (isAbsolute(relativePath) || relativePath === ".." || relativePath.startsWith(`..${pathSeparator()}`)) {
      throw new Error("live AEC legal-text manifest escapes its directory");
    }
    if (!existsSync(path) || !statSync(path).isFile()) {
      throw new Error(`live AEC legal text is missing: ${record.filename}`);
    }
    const digest = createHash("sha256").update(readFileSync(path)).digest("hex");
    if (digest !== record.sha256) {
      throw new Error(`live AEC legal text hash mismatch: ${record.filename}`);
    }
    return {
      filename: record.filename,
      content: normalizeLicenseContent(path),
      component: record.filename.startsWith("abseil/") ? "Native Abseil C++ 20240722.0-3 source" : "Native WebRTC AEC3 2.1.0 source",
    };
  });
}

function pathSeparator() {
  return process.platform === "win32" ? "\\" : "/";
}

if (mode === "--test-live-aec-licenses") {
  const nativeTexts = readLiveAecLicenseTexts();
  if (nativeTexts.length !== 9 || nativeTexts.some(({ content }) => !content)) {
    throw new Error("live AEC manifest did not produce all nine non-empty legal texts");
  }
  const fixture = mkdtempSync(join(tmpdir(), "sagascript-live-aec-license-test-"));
  try {
    writeFileSync(join(fixture, "COPYING"), "fixture does not match the reviewed text\n");
    const mismatch = rustLicenseOverride({
      name: "webrtc-audio-processing",
      version: "2.1.0",
      directory: fixture,
      license: undefined,
    });
    if (!mismatch.error?.includes("COPYING hash mismatch")) {
      throw new Error(`Unexpected live AEC mismatch result: ${mismatch.error}`);
    }
    const unexpectedLicense = rustLicenseOverride({
      name: "webrtc-audio-processing",
      version: "2.1.0",
      directory: fixture,
      license: "MIT",
    });
    if (!unexpectedLicense.error?.includes("unexpected SPDX license")) {
      throw new Error(`Unexpected live AEC SPDX result: ${unexpectedLicense.error}`);
    }
    const unexpectedVersion = rustLicenseOverride({
      name: "webrtc-audio-processing",
      version: "2.1.1",
      directory: fixture,
      license: undefined,
    });
    if (!unexpectedVersion.error?.includes("unexpected version")) {
      throw new Error(`Unexpected live AEC version result: ${unexpectedVersion.error}`);
    }
    const unrelated = rustLicenseOverride({
      name: "webrtc-audio-processing-extra",
      version: "2.1.0",
      directory: fixture,
      license: undefined,
    });
    if (unrelated.license !== undefined || unrelated.error !== undefined) {
      throw new Error("An unlisted package unexpectedly received a live AEC override");
    }
    console.log("live AEC license override validation passed");
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
  process.exit(0);
}

const rustById = new Map();
// The Windows-on-ARM Pianissimo engine host is a separate Cargo workspace with
// its own lockfile; include it for both platforms it can be built for.
const rustInventories = [
  { manifest: "src-tauri/Cargo.toml", target: "aarch64-apple-darwin" },
  { manifest: "src-tauri/engine-host/ort/Cargo.toml", target: "aarch64-apple-darwin" },
  { manifest: "src-tauri/engine-host/ort/Cargo.toml", target: "aarch64-pc-windows-msvc" },
];
for (const { manifest, target } of rustInventories) {
  const metadata = cargoMetadata(target, manifest);
  const nodes = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
  const resolvedIds = new Set();
  const queue = [...metadata.workspace_members];
  while (queue.length) {
    const id = queue.pop();
    if (resolvedIds.has(id)) continue;
    resolvedIds.add(id);
    for (const dep of nodes.get(id)?.deps ?? []) {
      if (dep.dep_kinds.some(({ kind }) => kind !== "dev")) queue.push(dep.pkg);
    }
  }
  for (const pkg of metadata.packages) {
    if (pkg.source && resolvedIds.has(pkg.id)) rustById.set(pkg.id, pkg);
  }
}

const rustPackages = [...rustById.values()]
  .map((pkg) => ({
    ecosystem: "Rust",
    name: pkg.name,
    version: pkg.version,
    license: pkg.license,
    source: componentSource(pkg, "Rust"),
    directory: dirname(pkg.manifest_path),
  }))
  .sort((a, b) => compareCodeUnits(a.name, b.name) || compareCodeUnits(a.version, b.version));

const lock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8"));
const errors = [];
let shippedNpm;
try {
  shippedNpm = shippedNpmClosure(lock.packages, appPackageReferences(root));
} catch (error) {
  // Reported with the other errors; until fixed, nothing counts as shipped.
  errors.push(error.message);
  shippedNpm = new Set();
}
const lockedNpmPackages = Object.entries(lock.packages)
  .filter(([path]) => path.includes("node_modules/"))
  .map(([path, entry]) => npmPackageFromLock(path, entry, shippedNpm))
  .sort((a, b) => compareCodeUnits(a.name, b.name) || compareCodeUnits(a.version, b.version));
// Keep every locked npm package in the inventory so output is identical on
// Apple Silicon and Intel CI. Platform-specific optional packages are not
// installed on the other architecture, so their SPDX metadata is recorded but
// their local license file is deliberately excluded from the text appendix.
const npmPackages = lockedNpmPackages;

for (const pkg of rustPackages) {
  const override = rustLicenseOverride(pkg);
  if (override.error) {
    errors.push(override.error);
    continue;
  }
  if (override.license !== undefined) pkg.license = override.license;
  if (!pkg.license) errors.push(`Rust ${pkg.name}@${pkg.version} has no SPDX license`);
  else if (!reviewedRustLicenses.has(pkg.license)) {
    errors.push(`Rust ${pkg.name}@${pkg.version} has unreviewed license: ${pkg.license}`);
  }
}
for (const pkg of lockedNpmPackages) {
  const licenseError = npmLicenseError(pkg);
  if (licenseError) errors.push(licenseError);
  const directoryExists = existsSync(pkg.directory);
  let installedVersion;
  if (directoryExists) {
    const manifestPath = join(pkg.directory, "package.json");
    if (existsSync(manifestPath)) {
      installedVersion = JSON.parse(readFileSync(manifestPath, "utf8")).version;
    }
  }
  const installationError = npmInstallationError(pkg, installedVersion, directoryExists);
  if (installationError) {
    errors.push(installationError);
  }
}

const modelSources = [
  "https://huggingface.co/ggerganov/whisper.cpp/resolve/",
  "https://huggingface.co/KBLab/kb-whisper-",
  "https://huggingface.co/NbAiLab/nb-whisper-",
  "https://huggingface.co/ggml-org/whisper-vad/resolve/",
  "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/",
  "https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM/resolve/",
];
const modelSourceCode = [
  readFileSync(join(root, "src-tauri/crates/sagascript-core/src/models/sources.rs"), "utf8"),
  readFileSync(join(root, "src-tauri/crates/sagascript-core/src/transcription/model.rs"), "utf8"),
  readFileSync(join(root, "src-tauri/crates/sagascript-core/src/diarization/model.rs"), "utf8"),
].join("\n");
for (const source of modelSources) {
  if (!modelSourceCode.includes(source)) errors.push(`Reviewed model source is no longer referenced: ${source}`);
}

let liveAecLicenseTexts = [];
try {
  liveAecLicenseTexts = readLiveAecLicenseTexts();
} catch (error) {
  errors.push(error instanceof Error ? error.message : String(error));
}

if (errors.length) {
  console.error(errors.map((error) => `- ${error}`).join("\n"));
  process.exit(1);
}

const licenseTexts = new Map();
for (const pkg of [...rustPackages, ...npmPackages.filter((pkg) => !pkg.optional)]) {
  for (const path of rootLicenseFiles(pkg.directory)) {
    // Preserve license wording while normalizing platform line endings and
    // insignificant trailing whitespace so the generated Markdown is stable
    // and passes the repository's whitespace gate.
    const content = readFileSync(path, "utf8")
      .replace(/\r\n?/g, "\n")
      .split("\n")
      .map((line) => line.trimEnd())
      .join("\n")
      .trim();
    if (!content) continue;
    const digest = createHash("sha256").update(content).digest("hex");
    const entry = licenseTexts.get(digest) ?? { content, components: [], filenames: new Set() };
    entry.components.push(`${pkg.ecosystem} ${pkg.name}@${pkg.version}`);
    entry.filenames.add(path.slice(pkg.directory.length + 1));
    licenseTexts.set(digest, entry);
  }
}
for (const record of liveAecLicenseTexts) {
  if (!record.content) continue;
  const digest = createHash("sha256").update(record.content).digest("hex");
  const entry = licenseTexts.get(digest) ?? { content: record.content, components: [], filenames: new Set() };
  entry.components.push(record.component);
  entry.filenames.add(`third-party/live-aec/${record.filename}`);
  licenseTexts.set(digest, entry);
}

function table(packages) {
  return [
    "| Component | Version | Declared license | Source |",
    "|---|---:|---|---|",
    ...packages.map((pkg) => `| ${pkg.name.replaceAll("|", "\\|")} | ${pkg.version} | ${pkg.license} | [upstream](${pkg.source}) |`),
  ].join("\n");
}

const generated = `# Third-party notices

This notice covers the runtime and build-time dependencies used to produce the
official Apple Silicon macOS build, plus the separately downloaded models
Sagascript can use. It is generated from the locked Rust and npm dependency
graphs; do not edit the generated inventories by hand. Sagascript itself is
licensed under the MIT License in \`LICENSE\`.

Generate this file with \`npm run licenses:generate\`. The release gate runs
\`npm run licenses:check\` and fails if this file is stale, a dependency omits
license metadata, or a new license expression has not been reviewed. When a
release is published, the separate SBOM workflow attaches SPDX and CycloneDX
dependency records to that release.

## Downloadable models

Models are **not bundled in the installer**. Sagascript downloads a model only
after the user chooses one; inference, audio, and transcripts remain local.
The license shown is the license published by the linked upstream repository,
reviewed on 2026-07-10 unless a later review date is shown below. This shipped
notice supplies the source and attribution
link; the upstream repository is authoritative for its license terms.

| Model family | What Sagascript downloads | License | Upstream / attribution |
|---|---|---|---|
| OpenAI Whisper GGML + Core ML encoders | Tiny, Base, Small, Medium, Large v3 Turbo variants | MIT | [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) |
| KB-Whisper | Tiny, Base, Small, Medium, Large Swedish models | Apache-2.0 | [KBLab, National Library of Sweden](https://huggingface.co/KBLab) |
| Klang Pianissimo (ONNX, Windows on ARM) | \`KlangAI/pianissimo-sv-onnx\` @ \`32118e6c01a88e3c4b4b735971195303a88d21ce\`: \`encoder-model.int8.onnx\`, \`decoder_joint-model.int8.onnx\`, \`nemo128.onnx\`, \`vocab.txt\`, \`config.json\`; downloaded on Windows on ARM only | CC-BY-4.0 | [Klang AI AB, model card and attribution](https://huggingface.co/KlangAI/pianissimo-sv-onnx/tree/32118e6c01a88e3c4b4b735971195303a88d21ce), reviewed 2026-09-29 |
| Klang Pianissimo (Core ML conversion) | Core ML archive converted from the original NeMo checkpoint (encoder int8, decoder and joint fp16); optional Swedish model | CC-BY-4.0 | [Klang AI model card and attribution](https://huggingface.co/KlangAI/pianissimo-sv), reviewed 2026-09-25 |
| NB-Whisper | Tiny, Base, Small, Medium, Large Norwegian models | Apache-2.0 | [NbAiLab, National Library of Norway](https://huggingface.co/NbAiLab) |
| Finnish-NLP Whisper Tiny | Unmodified \`ggml-model-fi-tiny.bin\`, optional Finnish specialist | Apache-2.0 | [Finnish-NLP pinned GGML repository](https://huggingface.co/Finnish-NLP/Finnish-finetuned-whisper-models-ggml-format/tree/c58924b6deb4438756b3d38ecd67d65bdf20298d), reviewed 2026-09-06 |
| Silero VAD (GGML conversion) | \`ggml-silero-v5.1.2.bin\` | MIT | [ggml-org/whisper-vad](https://huggingface.co/ggml-org/whisper-vad) |
| Pyannote Segmentation 3.0 (ONNX conversion) | \`model.onnx\` | MIT, copyright CNRS | [csukuangfj conversion repository and LICENSE](https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/blob/main/LICENSE) |
| WeSpeaker ResNet34-LM | \`voxceleb_resnet34_LM.onnx\` | CC-BY-4.0 | [WeSpeaker project/model card](https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM) |

Finnish recommends the existing multilingual OpenAI Whisper Base model (MIT),
not the English-only Base.en model. Optional Finnish-NLP Tiny is downloaded
unchanged from its publisher under the declared [Apache-2.0 terms](https://www.apache.org/licenses/LICENSE-2.0).
Its [source-model card](https://huggingface.co/Finnish-NLP/whisper-tiny-finnish/tree/bc5193ed50c052c426230644d17a12c3a8f86df6)
identifies OpenAI Whisper Tiny as the base; retain the OpenAI Whisper MIT
attribution as well. No matching CoreML encoder is supplied for the fine-tune.

## Live audio native sources

The optional live audio path uses WebRTC AEC3 from \`webrtc-audio-processing\` 2.1.0
and Abseil C++ 20240722.0-3. Their vendored legal texts are verified from the
repository's manifest and included below. Upstream sources are [WebRTC audio
processing](https://github.com/tonarino/webrtc-audio-processing) and
[Abseil C++](https://abseil.io/).

## Rust dependencies in the macOS application, build, and Windows engine host

${table(rustPackages)}

## npm dependencies used by the frontend and build

${table(npmPackages)}

## Pianissimo engine host

The Apple Silicon app bundles \`Resources/EngineHost/sagascript-engine-host\`, a Swift
Core ML host (\`src-tauri/engine-host/coreml\`) that runs the Pianissimo model on the
Apple Neural Engine. Its TDT greedy decoding structure is adapted from
[FluidAudio v0.17.4](https://github.com/FluidInference/FluidAudio) (Copyright 2024
FluidInference / FluidAudio contributors), licensed under the
[Apache-2.0 License](https://www.apache.org/licenses/LICENSE-2.0).

The Windows on ARM installer bundles \`engine-host\\sagascript-engine-host-ort.exe\`
(\`src-tauri/engine-host/ort\`, a separate Cargo workspace whose crates are listed in
the Rust table above) together with **ONNX Runtime 1.28.2** (\`onnxruntime.dll\`,
\`onnxruntime_providers_shared.dll\`) from the official
[Microsoft ONNX Runtime release](https://github.com/microsoft/onnxruntime/releases/tag/v1.28.2)
(\`onnxruntime-win-arm64-1.28.2.zip\`), Copyright (c) Microsoft Corporation, licensed
under the [MIT License](https://github.com/microsoft/onnxruntime/blob/v1.28.2/LICENSE).
ONNX Runtime's own third-party notices are published in
[ThirdPartyNotices.txt](https://github.com/microsoft/onnxruntime/blob/v1.28.2/ThirdPartyNotices.txt)
of that release and apply to the bundled binaries. The Pianissimo ONNX model
(CC BY 4.0, Klang AI AB) is downloaded on demand and is not bundled.

The downloadable Pianissimo Core ML model is converted from Klang AI AB's
[Pianissimo](https://huggingface.co/KlangAI/pianissimo-sv) (CC BY 4.0, converted, not
retrained) with scripts in \`scripts/pianissimo-coreml\` that are derived from
[FluidInference/mobius](https://github.com/FluidInference/mobius) (commit
864ef8050f2f281d0761de26e3a03108f9f1ce73), licensed under the
[Apache-2.0 License](https://www.apache.org/licenses/LICENSE-2.0). Attribution and
conversion details are kept in \`scripts/pianissimo-coreml/LICENSE-and-attribution.txt\`.

## Ported source code

Sagascript's engine-agnostic long-audio window planning and token merging
(\`src-tauri/crates/sagascript-core/src/transcription/chunk_merge.rs\`) is a Rust port of the
alignment and merge logic in [parakeet-mlx](https://github.com/senstella/parakeet-mlx)
(\`parakeet_mlx/alignment.py\`), copyright its contributors, licensed under the
[Apache-2.0 License](https://www.apache.org/licenses/LICENSE-2.0).

## License and notice texts shipped by dependencies

The following verbatim texts are collected from root-level \`LICENSE\`,
\`COPYING\`, and \`NOTICE\` files in the exact locked source packages. Identical
texts are deduplicated while retaining the associated component list.

${[...licenseTexts.entries()]
  .sort(([a], [b]) => compareCodeUnits(a, b))
  .map(
    ([digest, entry]) => `### ${digest.slice(0, 12)}

Components: ${entry.components.sort(compareCodeUnits).join(", ")}

Source filenames: ${[...entry.filenames].sort(compareCodeUnits).join(", ")}

~~~~text
${entry.content}
~~~~`,
  )
  .join("\n\n")}
`;

if (mode === "--write") {
  writeFileSync(outputPath, generated);
  console.log(`Wrote ${relative(root, outputPath)} (${rustPackages.length} Rust, ${npmPackages.length} npm packages)`);
} else {
  if (!existsSync(outputPath) || normalizeNewlines(readFileSync(outputPath, "utf8")) !== generated) {
    console.error("THIRD_PARTY_NOTICES.md is stale; run npm run licenses:generate and review the diff");
    process.exit(1);
  }
  console.log(`Third-party notices current (${rustPackages.length} Rust, ${npmPackages.length} npm packages)`);
}
