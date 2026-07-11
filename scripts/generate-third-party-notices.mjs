import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outputPath = join(root, "THIRD_PARTY_NOTICES.md");
const mode = process.argv[2];

function compareCodeUnits(a, b) {
  return a < b ? -1 : a > b ? 1 : 0;
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

if (!new Set(["--check", "--write"]).has(mode)) {
  console.error("Usage: node scripts/generate-third-party-notices.mjs --check|--write|--test-sort");
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
  "Zlib OR Apache-2.0 OR MIT",
]);

const reviewedNpmLicenses = new Set([
  "0BSD",
  "Apache-2.0",
  "Apache-2.0 OR MIT",
  "BSD-3-Clause",
  "ISC",
  "MIT",
  "MIT OR Apache-2.0",
]);

function cargoMetadata(target) {
  return JSON.parse(
    execFileSync(
      "cargo",
      [
        "metadata",
        "--format-version",
        "1",
        "--locked",
        "--filter-platform",
        target,
        "--manifest-path",
        "src-tauri/Cargo.toml",
      ],
      {
        cwd: root,
        encoding: "utf8",
        maxBuffer: 64 * 1024 * 1024,
        stdio: ["ignore", "pipe", "inherit"],
      },
    ),
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

const rustById = new Map();
for (const target of ["aarch64-apple-darwin"]) {
  const metadata = cargoMetadata(target);
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
const lockedNpmPackages = Object.entries(lock.packages)
  .filter(([path]) => path.includes("node_modules/"))
  .map(([path, pkg]) => {
    const name = path.slice(path.lastIndexOf("node_modules/") + "node_modules/".length);
    return {
      ecosystem: "npm",
      name,
      version: pkg.version,
      license: pkg.license,
      source: componentSource({ ...pkg, name }, "npm"),
      directory: join(root, path),
      optional: pkg.optional === true,
    };
  })
  .sort((a, b) => compareCodeUnits(a.name, b.name) || compareCodeUnits(a.version, b.version));
// Keep every locked npm package in the inventory so output is identical on
// Apple Silicon and Intel CI. Platform-specific optional packages are not
// installed on the other architecture, so their SPDX metadata is recorded but
// their local license file is deliberately excluded from the text appendix.
const npmPackages = lockedNpmPackages;

const errors = [];
for (const pkg of rustPackages) {
  if (!pkg.license) errors.push(`Rust ${pkg.name}@${pkg.version} has no SPDX license`);
  else if (!reviewedRustLicenses.has(pkg.license)) {
    errors.push(`Rust ${pkg.name}@${pkg.version} has unreviewed license: ${pkg.license}`);
  }
}
for (const pkg of lockedNpmPackages) {
  if (!pkg.license) errors.push(`npm ${pkg.name}@${pkg.version} has no SPDX license`);
  else if (!reviewedNpmLicenses.has(pkg.license)) {
    errors.push(`npm ${pkg.name}@${pkg.version} has unreviewed license: ${pkg.license}`);
  }
  if (!existsSync(pkg.directory) && !pkg.optional) {
    errors.push(`npm dependency directory is missing: ${relative(root, pkg.directory)} (run npm ci)`);
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
  readFileSync(join(root, "src-tauri/crates/sagascript-core/src/settings/manager.rs"), "utf8"),
  readFileSync(join(root, "src-tauri/crates/sagascript-core/src/transcription/model.rs"), "utf8"),
  readFileSync(join(root, "src-tauri/crates/sagascript-core/src/diarization/model.rs"), "utf8"),
].join("\n");
for (const source of modelSources) {
  if (!modelSourceCode.includes(source)) errors.push(`Reviewed model source is no longer referenced: ${source}`);
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

function table(packages) {
  return [
    "| Component | Version | Declared license | Source |",
    "|---|---:|---|---|",
    ...packages.map(
      (pkg) =>
        `| ${pkg.name.replaceAll("|", "\\|")} | ${pkg.version} | ${pkg.license} | [upstream](${pkg.source}) |`,
    ),
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
reviewed on 2026-07-10. This shipped notice supplies the source and attribution
link; the upstream repository is authoritative for its license terms.

| Model family | What Sagascript downloads | License | Upstream / attribution |
|---|---|---|---|
| OpenAI Whisper GGML + Core ML encoders | Tiny, Base, Small, Medium, Large v3 Turbo variants | MIT | [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) |
| KB-Whisper | Tiny, Base, Small, Medium, Large Swedish models | Apache-2.0 | [KBLab, National Library of Sweden](https://huggingface.co/KBLab) |
| NB-Whisper | Tiny, Base, Small, Medium, Large Norwegian models | Apache-2.0 | [NbAiLab, National Library of Norway](https://huggingface.co/NbAiLab) |
| Silero VAD (GGML conversion) | \`ggml-silero-v5.1.2.bin\` | MIT | [ggml-org/whisper-vad](https://huggingface.co/ggml-org/whisper-vad) |
| Pyannote Segmentation 3.0 (ONNX conversion) | \`model.onnx\` | MIT, copyright CNRS | [csukuangfj conversion repository and LICENSE](https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/blob/main/LICENSE) |
| WeSpeaker ResNet34-LM | \`voxceleb_resnet34_LM.onnx\` | CC-BY-4.0 | [WeSpeaker project/model card](https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM) |

## Rust dependencies in the macOS application and build

${table(rustPackages)}

## npm dependencies used by the frontend and build

${table(npmPackages)}

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
  if (!existsSync(outputPath) || readFileSync(outputPath, "utf8") !== generated) {
    console.error("THIRD_PARTY_NOTICES.md is stale; run npm run licenses:generate and review the diff");
    process.exit(1);
  }
  console.log(`Third-party notices current (${rustPackages.length} Rust, ${npmPackages.length} npm packages)`);
}
