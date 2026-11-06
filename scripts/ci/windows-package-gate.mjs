// Decides whether a pull request needs the Windows Package Candidate (windows-package.yml, SAGA-551).
// The workflow's `decide` job runs this; workflow_call and workflow_dispatch never consult it.
//
// A pull request runs the package build (about 30 minutes on two Windows runners, not a required
// check) only when one of these holds:
//   1. it carries the label `windows-package`;
//   2. it changes the app version, like prebuild-release.yml's detection (a release PR);
//   3. it touches a file that only the Windows package build reads (WINDOWS_PACKAGE_PATHS).
// Anything this script cannot work out (no base commit, unreadable file) fails open: it runs.
import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";

export const LABEL = "windows-package";
export const VERSION_FILES = ["package.json", "src-tauri/tauri.conf.json"];

// Files that matter to Windows packaging, taken from what windows-package.yml runs and reads, plus
// the dependency manifests. A trailing "/" matches a directory. Application source and shared
// scripts are deliberately absent: ci.yml's test-windows and build-windows already compile and
// test them on Windows for every pull request. Dependency changes are not: they can change the
// bundler (the Tauri CLI from package-lock.json, tauri-build from Cargo.lock) or what is packaged,
// which only this workflow exercises before a release.
export const WINDOWS_PACKAGE_PATHS = [
  "package.json", // dependency requirements, overrides and the build scripts `tauri build` runs
  "package-lock.json", // @tauri-apps/cli is the NSIS/MSI bundler; frontend dependencies are packaged
  "src-tauri/Cargo.toml", // app crate features and dependency requirements
  "src-tauri/Cargo.lock", // tauri-build and every crate compiled into the packaged app
  ".gitattributes", // checkout normalisation (line endings) decides the packaged bytes
  ".github/workflows/windows-package.yml", // the workflow itself
  "src-tauri/tauri.conf.json", // bundler/installer (NSIS, MSI) configuration
  "src-tauri/engine-host/ort/", // the ONNX engine host the workflow builds and bundles for ARM64
  "scripts/tauri-windows-cli.json", // bundles the console CLI beside the app
  "scripts/tauri-windows-engine-host.json", // bundles the ARM64 engine host
  "scripts/tauri-ci-prebuilt.json", // reuses the prebuilt CLI/frontend in `tauri build`
  "scripts/cmake/", // x64 portable and ARM64 native CMake policies (also in the cache key)
  "scripts/ci-build-identity.mjs", // source identity pin/verify around the release build
  "scripts/report-windows-arm64-native.mjs", // ARM64 native-code report
  "scripts/verify-windows-x64-cpu-policy.ps1", // x64 CPU baseline check (also in the cache key)
  "scripts/verify-windows-engine-host.ps1", // engine host staging check
  "scripts/verify-windows-release.ps1", // installer/artifact verification
  "scripts/accept-windows-candidate.ps1", // silent-install acceptance run
  "scripts/benchmark-windows-engine-host.ps1", // engine host latency probe
  "scripts/verify-json-cli-streams.py", // stdout/stderr contract of the built Windows CLI
];

export const pathMatches = (file) => WINDOWS_PACKAGE_PATHS.some((entry) => (entry.endsWith("/") ? file.startsWith(entry) : file === entry));

/** Pure decision. `versions` is `[{ file, before, after }]`; null means "could not read". */
export function decide({ labels = [], changedFiles = null, versions = null }) {
  if (labels.includes(LABEL)) return { run: true, reason: `label ${LABEL}` };
  if (changedFiles === null || versions === null) return { run: true, reason: "change set unavailable; running to be safe" };
  const bumped = versions.find((v) => v.before !== v.after);
  if (bumped) return { run: true, reason: `version change in ${bumped.file}` };
  const hit = changedFiles.find(pathMatches);
  if (hit) return { run: true, reason: `Windows packaging path ${hit}` };
  return { run: false, reason: `no ${LABEL} label, version change or Windows packaging path` };
}

const git = (...args) => execFileSync("git", args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
const versionOf = (text) => {
  try {
    return JSON.parse(text).version ?? null;
  } catch {
    return null;
  }
};
const readAt = (rev, file) => {
  try {
    return git("show", `${rev}:${file}`);
  } catch {
    return "";
  }
};

function main() {
  const event = process.env.EVENT_NAME;
  const emit = ({ run, reason }) => {
    console.log(`run=${run}: ${reason}`);
    if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `run=${run}\n`);
  };
  // Releases (workflow_call from prebuild-release.yml) and manual runs always build.
  if (event !== "pull_request") return emit({ run: true, reason: `${event} always builds` });

  const labels = JSON.parse(process.env.PR_LABELS || "[]") ?? [];
  let changedFiles = null;
  let versions = null;
  try {
    // HEAD is the merge commit of the pull request; its first parent is the current base tip.
    changedFiles = git("diff", "--name-only", "HEAD^1", "HEAD").split("\n").filter(Boolean);
    versions = VERSION_FILES.map((file) => ({
      file,
      before: versionOf(readAt("HEAD^1", file)),
      after: versionOf(readFileSync(file, "utf8")),
    }));
  } catch (error) {
    console.log(`could not diff against the base: ${error.message}`);
  }
  emit(decide({ labels, changedFiles, versions }));
}

if (import.meta.url === `file://${process.argv[1]}`) main();
