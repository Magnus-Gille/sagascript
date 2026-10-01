import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { createManifest, verifyManifest } from "./release-prebuild-manifest.mjs";

const read = (path) => readFileSync(fileURLToPath(new URL(`../${path}`, import.meta.url)), "utf8").replace(/\r\n/g, "\n");
const workflows = Object.fromEntries(
  ["release", "prebuild-release", "release-build-macos", "release-quality-gate", "windows-package"].map((name) => [
    name,
    read(`.github/workflows/${name}.yml`),
  ]),
);
const SHA = "a".repeat(40);
const TREE = "b".repeat(40);

function fixture() {
  const dir = mkdtempSync(join(tmpdir(), "prebuild-"));
  mkdirSync(join(dir, "macos-bundle"));
  mkdirSync(join(dir, "windows-arm64-unsigned-candidate"));
  writeFileSync(join(dir, "macos-bundle", "Sagascript.dmg"), "dmg");
  writeFileSync(join(dir, "windows-arm64-unsigned-candidate", "setup.exe"), "exe");
  const manifest = createManifest({ dir, version: "1.2.3", sha: SHA, tree: TREE });
  return { dir, manifest, expect: { dir, manifest, version: "1.2.3", sha: SHA, tree: TREE } };
}

test("manifest records version, commit, tree and per-file hashes", () => {
  const { manifest } = fixture();
  assert.equal(manifest.version, "1.2.3");
  assert.equal(manifest.sha, SHA);
  assert.equal(manifest.tree_sha, TREE);
  assert.deepEqual(manifest.files.map((f) => f.name), ["macos-bundle/Sagascript.dmg", "windows-arm64-unsigned-candidate/setup.exe"]);
  assert.match(manifest.files[0].sha256, /^[0-9a-f]{64}$/);
  assert.equal(manifest.files[0].size, 3);
});

test("verification accepts an untouched prebuild", () => {
  const { expect } = fixture();
  assert.deepEqual(verifyManifest(expect), []);
});

test("verification rejects tampering, missing or extra files, and identity mismatches", () => {
  const { dir, expect } = fixture();
  assert.match(verifyManifest({ ...expect, sha: "c".repeat(40) }).join("\n"), /sha is/);
  assert.match(verifyManifest({ ...expect, tree: "c".repeat(40) }).join("\n"), /tree_sha is/);
  assert.match(verifyManifest({ ...expect, version: "1.2.4" }).join("\n"), /version is/);
  writeFileSync(join(dir, "extra.bin"), "x");
  assert.match(verifyManifest(expect).join("\n"), /extra\.bin: present but not in the manifest/);
  writeFileSync(join(dir, "macos-bundle", "Sagascript.dmg"), "DMG");
  assert.match(verifyManifest(expect).join("\n"), /Sagascript\.dmg: sha256/);
  writeFileSync(join(dir, "macos-bundle", "gone.txt"), "");
  const missing = { ...expect, manifest: { ...expect.manifest, files: [...expect.manifest.files, { name: "nope.exe", sha256: "0".repeat(64), size: 1 }] } };
  assert.match(verifyManifest(missing).join("\n"), /nope\.exe: ENOENT/);
});

test("manifest creation rejects non-final versions and malformed identities", () => {
  const { dir } = fixture();
  assert.throws(() => createManifest({ dir, version: "1.2.3-rc1", sha: SHA, tree: TREE }));
  assert.throws(() => createManifest({ dir, version: "1.2.3", sha: "abc", tree: TREE }));
});

test("manifest covers the arm64 portable zip and the 5-line checksum file", () => {
  const dir = mkdtempSync(join(tmpdir(), "prebuild-zip-"));
  const sub = join(dir, "windows-arm64-unsigned-candidate");
  mkdirSync(sub);
  const names = [
    "Sagascript-Windows-arm64-CLI.exe",
    "Sagascript-Windows-arm64-Portable.exe",
    "Sagascript-Windows-arm64-Portable.zip",
    "Sagascript-Windows-arm64-Setup.exe",
    "Sagascript-Windows-arm64.msi",
    "SHA256SUMS-Windows-arm64",
  ];
  for (const name of names) writeFileSync(join(sub, name), name);
  const manifest = createManifest({ dir, version: "1.2.3", sha: SHA, tree: TREE });
  assert.ok(manifest.files.some((f) => f.name.endsWith("-Portable.zip")));
  assert.deepEqual(verifyManifest({ dir, manifest, version: "1.2.3", sha: SHA, tree: TREE }), []);
  const w = workflows.release;
  assert.match(w, /files\+=\("Sagascript-Windows-\$arch-Portable\.zip"\)\s*\n\s+expected=5/);
  assert.match(w, /expected=4/);
  const v = read("scripts/verify-release-draft.sh");
  assert.match(v, /names\+=\("Sagascript-Windows-\$arch-Portable\.zip"\)/);
});

test("windows-package keeps the console CLI, installed-CLI and portable zip steps unconditional by event", () => {
  const w = workflows["windows-package"];
  assert.match(w, /workflow_call:/);
  assert.match(w, /build\\cli\\sagascript-cli\.exe/);
  assert.match(w, /Verify silent per-user install runs the installed ARM64 CLI/);
  assert.match(w, /Build and verify ARM64 portable zip/);
  assert.match(w, /Verify installers carry the console CLI/);
  assert.doesNotMatch(w, /if:[^\n]*github\.event_name/);
});

test("prebuild runs on main pushes only, never on pull requests, and never cancels", () => {
  const w = workflows["prebuild-release"];
  assert.match(w, /on:\s*\n\s+push:\s*\n\s+branches: \[main\]/);
  assert.doesNotMatch(w, /pull_request/);
  assert.match(w, /cancel-in-progress: false/);
  assert.match(w, /permissions:\s*\n\s+contents: read/);
  assert.match(w, /git ls-remote --exit-code --tags origin/);
  assert.match(w, /uses: \.\/\.github\/workflows\/windows-package\.yml/);
  assert.match(w, /release-prebuild-manifest\.mjs create/);
});

test("prebuild builds only when the push changes the version, or on a main-only dispatch", () => {
  const w = workflows["prebuild-release"];
  assert.match(w, /\n  workflow_dispatch:/);
  assert.match(w, /if \[\[ "\$REF" != "refs\/heads\/main" \]\]/);
  assert.match(w, /BEFORE: \$\{\{ github\.event\.before \}\}/);
  assert.match(w, /\^0\+\$/);
  assert.match(w, /git fetch --no-tags --depth=1 origin "\$BEFORE"/);
  assert.match(w, /for file in package\.json src-tauri\/tauri\.conf\.json/);
  assert.match(w, /git show "\$BEFORE:\$file"/);
  assert.match(w, /did not change the release version/);
  assert.match(w, /group: prebuild-release-\$\{\{ github\.sha \}\}/);
  assert.match(w, /cancel-in-progress: false/);
});

test("no workflow uses secrets: inherit; the signing callee declares exactly the secrets it reads", () => {
  for (const [name, text] of Object.entries(workflows)) {
    assert.doesNotMatch(text, /^\s*secrets:\s*inherit/m, `${name} must pass secrets explicitly`);
  }
  const callee = workflows["release-build-macos"];
  const used = [...new Set([...callee.matchAll(/\$\{\{\s*secrets\.([A-Z0-9_]+)\s*\}\}/g)].map((m) => m[1]))].sort();
  assert.ok(used.length >= 9);
  const header = callee.slice(callee.indexOf("workflow_call:"), callee.indexOf("\npermissions:"));
  const declared = [...header.matchAll(/^ {6}([A-Z0-9_]+):\s*\n\s+required: true/gm)].map((m) => m[1]).sort();
  assert.deepEqual(declared, used);
  for (const name of ["prebuild-release", "release"]) {
    const text = workflows[name];
    const block = text.slice(text.indexOf("release-build-macos.yml"));
    const passed = [...block.matchAll(/^ {6}([A-Z0-9_]+): \$\{\{ secrets\.\1 \}\}/gm)].map((m) => m[1]).sort();
    assert.deepEqual(passed, used, `${name} passes exactly the callee's secrets`);
  }
});

test("rust-toolchain is pinned by full SHA with an explicit toolchain in the release workflows", () => {
  for (const name of ["release-build-macos", "release-quality-gate"]) {
    assert.match(workflows[name], /uses: dtolnay\/rust-toolchain@[0-9a-f]{40} # .*\n\s+with:\n\s+toolchain: stable/, name);
    assert.doesNotMatch(workflows[name], /rust-toolchain@stable/);
  }
});

test("promote-release checkout does not persist credentials", () => {
  const w = workflows.release;
  const promote = w.slice(w.indexOf("  promote-release:"), w.indexOf("  verify-draft:"));
  assert.match(promote, /actions\/checkout@v6\s*\n\s+with:\n\s+persist-credentials: false/);
});

test("locate-prebuild requires unexpired artifacts and logs why it falls back", () => {
  const w = workflows.release;
  assert.match(w, /actions\/runs\/\$candidate\/artifacts/);
  assert.match(w, /\.expired/);
  for (const name of ["macos-bundle", "windows-x64-unsigned-candidate", "windows-arm64-unsigned-candidate", "release-manifest"]) {
    assert.ok(w.includes(name), `locate-prebuild requires ${name}`);
  }
  assert.match(w, /lacks unexpired artifacts/);
  assert.match(w, /for event in push workflow_dispatch/);
});

test("verify-draft requires the Windows assets on the promote path", () => {
  assert.match(workflows.release, /verify-release-draft\.sh --assets-dir draft-assets[\s\S]*--require-windows/);
  const v = read("scripts/verify-release-draft.sh");
  assert.match(v, /--require-windows\) require_windows=1/);
  assert.match(v, /missing_windows "Windows \$arch: no SHA256SUMS/);
  assert.match(v, /GITHUB_ACTIONS:-\} == true/);
  assert.match(v, /missing_windows "windows-beta-\$version release is missing"/);
});

test("manifest verification rejects absolute, traversing and separator-abusing entry names", () => {
  const { expect } = fixture();
  for (const name of ["/etc/passwd", "../x", "macos-bundle/../../x", "a\\b", "./a", "a//b", "C:/x", ""]) {
    const manifest = { ...expect.manifest, files: [...expect.manifest.files, { name, sha256: "0".repeat(64), size: 1 }] };
    assert.match(verifyManifest({ ...expect, manifest }).join("\n"), /unsafe manifest entry name/, JSON.stringify(name));
  }
});

test("signing lives in exactly one reusable workflow behind the updater-signing environment", () => {
  const signing = workflows["release-build-macos"];
  assert.match(signing, /workflow_call:/);
  assert.match(signing, /environment: updater-signing/);
  for (const name of ["release", "prebuild-release", "windows-package", "release-quality-gate"]) {
    // Callers may only forward secrets as `secrets:` inputs to the signing workflow; never consume them.
    const consumed = workflows[name].replace(/^ {6}([A-Z0-9_]+): \$\{\{ secrets\.\1 \}\}\n/gm, "");
    assert.doesNotMatch(consumed, /APPLE_CERTIFICATE|secrets\.TAURI_SIGNING|environment: updater-signing/, `${name} must not hold signing material`);
  }
});

test("tag path promotes a verified prebuild and keeps the full build as fallback", () => {
  const w = workflows.release;
  assert.match(w, /--workflow prebuild-release\.yml --branch main/);
  assert.match(w, /--event "\$event" --commit "\$TAG_SHA" --status success/);
  assert.match(w, /gh run download "\$PREBUILD_RUN_ID"/);
  assert.match(w, /release-prebuild-manifest\.mjs verify[\s\S]*--sha "\$GITHUB_SHA"[\s\S]*HEAD\^\{tree\}/);
  assert.match(w, /promote-release:[\s\S]*if: needs\.locate-prebuild\.outputs\.found == 'true'/);
  assert.match(w, /quality-gate:[\s\S]*if: needs\.locate-prebuild\.outputs\.found != 'true'[\s\S]*release-quality-gate\.yml/);
  assert.match(w, /build-macos:[\s\S]*release-build-macos\.yml[\s\S]*secrets:\s*\n\s+APPLE_CERTIFICATE:/);
  assert.match(w, /publish-release:\s*\n\s+needs: \[build-macos\]/);
  assert.match(w, /--prerelease --target "\$GITHUB_SHA"/);
  assert.match(w, /verify-release-draft\.sh --assets-dir draft-assets/);
  // Verification must not hold a write token; only promote/publish write.
  const verify = w.slice(w.indexOf("  verify-draft:"), w.indexOf("  quality-gate:"));
  assert.doesNotMatch(verify, /contents: write/);
  const writers = [...w.matchAll(/contents: write/g)].length;
  assert.equal(writers, 2, "promote-release and the fallback publish-release are the only writers");
});

test("new workflows pin third-party actions and avoid expression interpolation in run blocks", () => {
  for (const [name, text] of Object.entries(workflows)) {
    for (const [, action, ref] of text.matchAll(/^\s*(?:- )?uses: ([^\s@./][^\s@]*)@(\S+)/gm)) {
      if (action.startsWith("actions/")) continue;
      if (name === "windows-package" && action.startsWith("dtolnay/")) continue; // repo-wide @stable, out of scope here
      assert.match(ref, /^[0-9a-f]{40}$/, `${name}: ${action} must be pinned by commit SHA`);
    }
    const lines = text.split("\n");
    for (let i = 0; i < lines.length; i++) {
      const block = lines[i].match(/^(\s*)run: [|>]/);
      if (!block) continue;
      for (let j = i + 1; j < lines.length && (lines[j].trim() === "" || lines[j].startsWith(`${block[1]}  `)); j++) {
        if (name === "windows-package") continue; // pre-existing matrix-only interpolation, covered by its own tests
        assert.doesNotMatch(lines[j], /\$\{\{/, `${name}:${j + 1} interpolates an expression inside run:`);
      }
    }
  }
});

test("draft verification script covers the release checklist", () => {
  const s = read("scripts/verify-release-draft.sh");
  for (const needle of [
    "shasum -a 256 -c",
    "codesign --verify --deep --strict",
    "7C6WF6GFZ4",
    "Notarized Developer ID",
    "stapler validate",
    "verify-macos-release.sh",
    "--version",
    "latest.json",
    "smoke-pianissimo-installed.sh",
    "swedish-fleurs-hongkong.wav",
    "hongkong",
    "windows-acceptance-ps51.json",
    "GITHUB_STEP_SUMMARY",
  ]) assert.ok(s.includes(needle), `verify-release-draft.sh covers ${needle}`);
});
