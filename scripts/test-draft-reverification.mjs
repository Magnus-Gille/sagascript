import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { parse } from "yaml";
import { draftMetadataFromManifest, expectedFromEnvironment, validateSource, verifyAssets } from "./prepare-draft-reverification.mjs";

const workflowPath = new URL("../.github/workflows/verify-release-draft.yml", import.meta.url);
const helperPath = new URL("./prepare-draft-reverification.mjs", import.meta.url);
const workflowText = readFileSync(workflowPath, "utf8").replace(/\r\n?/g, "\n");
const helperText = readFileSync(helperPath, "utf8");
const workflow = parse(workflowText);

const SHA = "a".repeat(40);
const EXPECTED = { repo: "Magnus-Gille/sagascript", version: "1.4.5", sha: SHA, runId: "39526373954" };
const DIGEST = (text) => "sha256:" + createHash("sha256").update(text).digest("hex");

function asset(id, name, content = name) {
  return { id, name, digest: DIGEST(content), size: Buffer.byteLength(content), content };
}

function sourceFixture() {
  const release = asset(101, "Sagascript.dmg", "stable");
  const windows = asset(201, "Sagascript-Windows-x64.msi", "windows");
  return {
    run: {
      id: Number(EXPECTED.runId),
      repository: { full_name: EXPECTED.repo },
      path: ".github/workflows/release.yml",
      event: "push",
      head_branch: "v1.4.5",
      head_sha: SHA,
      status: "completed",
    },
    jobs: [{ name: "promote-release", status: "completed", conclusion: "success" }],
    tagCommit: { sha: SHA },
    release: {
      id: 1,
      tag_name: "v1.4.5",
      draft: true,
      prerelease: false,
      assets: [{ ...release }],
    },
    windowsRelease: {
      id: 2,
      tag_name: "windows-beta-1.4.5",
      draft: true,
      prerelease: true,
      assets: [{ ...windows }],
    },
  };
}

function manifestFixture() {
  const source = sourceFixture();
  return {
    schema_version: 1,
    repo: EXPECTED.repo,
    version: EXPECTED.version,
    sha: EXPECTED.sha,
    capturedAt: "2026-10-21T12:34:56.000Z",
    release: source.release,
    windowsRelease: source.windowsRelease,
  };
}

function assertRejected(mutator, message) {
  const source = structuredClone(sourceFixture());
  mutator(source);
  assert.throws(() => validateSource(source, EXPECTED), message);
}

test("validateSource accepts the exact unpublished promoted release fixture", () => {
  assert.doesNotThrow(() => validateSource(sourceFixture(), EXPECTED));
});

test("validateSource rejects identity, workflow, event, tag, SHA, active-run, and promotion mismatches", () => {
  const cases = [
    [(source) => (source.run.repository.full_name = "other/repo"), /source repository mismatch/],
    [(source) => (source.run.id = 99), /source run ID mismatch/],
    [(source) => (source.run.path = ".github/workflows/other.yml"), /release workflow/],
    [(source) => (source.run.event = "workflow_dispatch"), /tag push/],
    [(source) => (source.run.head_branch = "main"), /source tag mismatch/],
    [(source) => (source.run.head_sha = "b".repeat(40)), /source release SHA mismatch/],
    [(source) => (source.run.status = "in_progress"), /still active/],
    [(source) => (source.tagCommit.sha = "b".repeat(40)), /tag no longer points/],
    [(source) => (source.jobs = [{ name: "promote-release", status: "completed", conclusion: "failure" }]), /successfully promoted/],
  ];
  for (const [mutator, message] of cases) assertRejected(mutator, message);

  const wrongExpected = { ...EXPECTED, repo: "other/repo" };
  assert.throws(() => validateSource(sourceFixture(), wrongExpected), /source repository mismatch/);
  assert.throws(() => validateSource(sourceFixture(), { ...EXPECTED, runId: "99" }), /source run ID mismatch/);
});

test("validateSource rejects published drafts, wrong prerelease flags, empty assets, and unsafe metadata", () => {
  assertRejected((source) => (source.release.draft = false), /unpublished draft/);
  assertRejected((source) => (source.windowsRelease.draft = false), /unpublished draft/);
  assertRejected((source) => (source.release.prerelease = true), /stable draft/);
  assertRejected((source) => (source.windowsRelease.prerelease = false), /Windows draft/);
  assertRejected((source) => (source.release.assets = []), /draft has no assets/);

  for (const name of ["", "/absolute", "../traversal", "a/b", "a\\b", ".hidden"]) {
    assertRejected((source) => (source.release.assets[0].name = name), /unsafe asset name/);
  }
  assertRejected((source) => source.release.assets.push({ ...source.release.assets[0], id: 102 }), /duplicate asset name/);
});

test("validateSource rejects missing or invalid asset digests and sizes", () => {
  assertRejected((source) => delete source.release.assets[0].digest, /missing SHA-256 asset digest/);
  assertRejected((source) => (source.release.assets[0].digest = "sha256:bad"), /missing SHA-256 asset digest/);
  assertRejected((source) => (source.release.assets[0].size = 0), /invalid asset size/);
  assertRejected((source) => (source.release.assets[0].size = -1), /invalid asset size/);
  assertRejected((source) => (source.release.assets[0].size = Number.MAX_SAFE_INTEGER + 1), /invalid asset size/);
  assertRejected((source) => (source.release.assets[0].size = "4"), /invalid asset size/);
});

test("draftMetadataFromManifest accepts the captured stable and Windows draft snapshot", () => {
  const manifest = manifestFixture();
  assert.deepEqual(draftMetadataFromManifest(manifest, EXPECTED), {
    release: manifest.release,
    windowsRelease: manifest.windowsRelease,
    snapshotCapturedAt: manifest.capturedAt,
  });
});

test("draftMetadataFromManifest rejects invalid schema, identity, timestamp, and release IDs", () => {
  const cases = [
    [(manifest) => (manifest.schema_version = 2), /unsupported draft manifest schema/],
    [(manifest) => (manifest.repo = "other/repo"), /repository mismatch/],
    [(manifest) => (manifest.version = "1.4.6"), /version mismatch/],
    [(manifest) => (manifest.sha = "b".repeat(40)), /release SHA mismatch/],
    [(manifest) => (manifest.capturedAt = "not-a-timestamp"), /capture timestamp/],
    [(manifest) => (manifest.release.id = 0), /release IDs/],
    [(manifest) => delete manifest.windowsRelease.id, /release IDs/],
  ];
  for (const [mutator, message] of cases) {
    const manifest = manifestFixture();
    mutator(manifest);
    assert.throws(() => draftMetadataFromManifest(manifest, EXPECTED), message);
  }
});

test("expectedFromEnvironment accepts reviewed main dispatch inputs", () => {
  assert.deepEqual(
    expectedFromEnvironment({
      GITHUB_REPOSITORY: EXPECTED.repo,
      RELEASE_VERSION: EXPECTED.version,
      RELEASE_SHA: EXPECTED.sha,
      RELEASE_RUN_ID: EXPECTED.runId,
      GITHUB_REF: "refs/heads/main",
    }),
    EXPECTED,
  );
});

test("expectedFromEnvironment rejects missing, malformed, or non-main dispatch inputs", () => {
  const valid = {
    GITHUB_REPOSITORY: EXPECTED.repo,
    RELEASE_VERSION: EXPECTED.version,
    RELEASE_SHA: EXPECTED.sha,
    RELEASE_RUN_ID: EXPECTED.runId,
    GITHUB_REF: "refs/heads/main",
  };
  for (const field of ["GITHUB_REPOSITORY", "RELEASE_VERSION", "RELEASE_SHA", "RELEASE_RUN_ID"]) {
    assert.throws(() => expectedFromEnvironment({ ...valid, [field]: "" }), /The input did not match the regular expression/);
  }
  for (const [field, value] of [
    ["GITHUB_REPOSITORY", "Magnus-Gille/sagascript;echo"],
    ["RELEASE_VERSION", "1.4.5$(echo pwned)"],
    ["RELEASE_SHA", SHA.slice(0, 39) + ";"],
    ["RELEASE_RUN_ID", "39526373954;echo"],
  ]) {
    assert.throws(() => expectedFromEnvironment({ ...valid, [field]: value }), /The input did not match the regular expression/);
  }
  assert.throws(() => expectedFromEnvironment({ ...valid, GITHUB_REF: "refs/heads/release" }), /reviewed main workflow/);
  const missingRef = { ...valid };
  delete missingRef.GITHUB_REF;
  assert.throws(() => expectedFromEnvironment(missingRef), /reviewed main workflow/);
});

function assetSnapshot() {
  const directory = mkdtempSync(join(tmpdir(), "draft-reverification-assets-"));
  const release = asset(301, "Sagascript.dmg", "stable-bytes");
  const windows = asset(401, "Sagascript-Windows-x64.msi", "windows-bytes");
  const source = sourceFixture();
  source.release.assets = [{ ...release }];
  source.windowsRelease.assets = [{ ...windows }];
  mkdirSync(join(directory, "release"));
  mkdirSync(join(directory, "windows-beta"));
  writeFileSync(join(directory, "release", release.name), release.content);
  writeFileSync(join(directory, "windows-beta", windows.name), windows.content);
  return { directory, source, release, windows, cleanup: () => rmSync(directory, { recursive: true, force: true }) };
}

test("verifyAssets accepts exact inventories and returns release and asset bindings", (t) => {
  const { directory, source, release, windows, cleanup } = assetSnapshot();
  t.after(cleanup);
  assert.deepEqual(verifyAssets(directory, source), [
    { releaseId: 1, assetId: release.id, name: release.name, size: release.size, digest: release.digest },
    { releaseId: 2, assetId: windows.id, name: windows.name, size: windows.size, digest: windows.digest },
  ]);
});

test("verifyAssets rejects missing, extra, size-mismatched, and same-length hash-mismatched files", (t) => {
  const fixtures = [];
  const useFixture = () => {
    const fixture = assetSnapshot();
    fixtures.push(fixture);
    return fixture;
  };
  t.after(() => fixtures.forEach(({ cleanup }) => cleanup()));

  {
    const fixture = useFixture();
    writeFileSync(join(fixture.directory, "release", "extra.txt"), "extra");
    assert.throws(() => verifyAssets(fixture.directory, fixture.source), /inventories differ/);
  }
  {
    const fixture = useFixture();
    rmSync(join(fixture.directory, "release", fixture.release.name));
    assert.throws(() => verifyAssets(fixture.directory, fixture.source), /inventories differ/);
  }
  {
    const fixture = useFixture();
    writeFileSync(join(fixture.directory, "release", fixture.release.name), "alteredbytes");
    assert.throws(() => verifyAssets(fixture.directory, fixture.source), /asset hash differs/);
  }
  {
    const fixture = useFixture();
    fixture.source.release.assets[0].size += 1;
    assert.throws(() => verifyAssets(fixture.directory, fixture.source), /asset size differs/);
  }
});

test("verifyAssets rejects symlinked asset files", { skip: process.platform === "win32" }, (t) => {
  const fixture = assetSnapshot();
  t.after(fixture.cleanup);
  const target = join(fixture.directory, "real-target");
  writeFileSync(target, fixture.release.content);
  rmSync(join(fixture.directory, "release", fixture.release.name));
  symlinkSync(target, join(fixture.directory, "release", fixture.release.name));
  assert.throws(() => verifyAssets(fixture.directory, fixture.source), /not a regular file/);
});

test("verifyAssets rejects symlinked asset directories", { skip: process.platform === "win32" }, (t) => {
  const fixture = assetSnapshot();
  t.after(fixture.cleanup);
  const moved = join(fixture.directory, "windows-beta-target");
  rmSync(join(fixture.directory, "windows-beta"), { recursive: true, force: true });
  mkdirSync(moved);
  writeFileSync(join(moved, fixture.windows.name), fixture.windows.content);
  symlinkSync(moved, join(fixture.directory, "windows-beta"));
  assert.throws(() => verifyAssets(fixture.directory, fixture.source), /not a real directory/);
});

test("verify-release-draft workflow is manual, read-only, pinned, and reuses the guarded verifier", () => {
  assert.equal(workflow.on.workflow_dispatch.inputs.release_run_id.required, true);
  assert.equal(workflow.on.workflow_dispatch.inputs.version.required, true);
  assert.equal(workflow.on.workflow_dispatch.inputs.release_sha.required, true);
  assert.equal(workflow.on.workflow_dispatch.inputs.draft_manifest.required, true);
  assert.deepEqual(Object.keys(workflow.on), ["workflow_dispatch"]);
  assert.deepEqual(workflow.permissions, { contents: "read", actions: "read" });
  assert.deepEqual(Object.keys(workflow.jobs), ["verify-drafts"]);

  const job = workflow.jobs["verify-drafts"];
  assert.equal(job["runs-on"], "macos-14");
  assert.equal(job["timeout-minutes"], 60);
  assert.deepEqual(job.env, {
    RELEASE_RUN_ID: "${{ inputs.release_run_id }}",
    RELEASE_VERSION: "${{ inputs.version }}",
    RELEASE_SHA: "${{ inputs.release_sha }}",
    DRAFT_MANIFEST: "${{ inputs.draft_manifest }}",
  });

  const actionSteps = job.steps.filter((step) => step.uses);
  assert.ok(actionSteps.length > 0);
  for (const step of actionSteps) {
    assert.match(step.uses, /^[^@]+@[0-9a-f]{40}$/);
  }
  const runSteps = job.steps.filter((step) => step.run);
  assert.ok(
    runSteps.every((step) => !step.run.includes("${{")),
    "run blocks must not interpolate GitHub expressions",
  );
  assert.ok(runSteps.some((step) => /prepare-draft-reverification\.mjs preflight/.test(step.run)));
  assert.ok(runSteps.some((step) => /prepare-draft-reverification\.mjs verify-assets/.test(step.run)));
  assert.match(job.steps.find((step) => step.name === "Verify unchanged draft release").run, /--require-windows/);
  assert.equal(job.steps.filter((step) => step.if).length, 1);
  assert.equal(job.steps.find((step) => step.if)?.if, "always()");
  assert.doesNotMatch(workflowText, /contents:\s*write|secrets\.|environment\s*:/);
  assert.doesNotMatch(workflowText, /gh\s+release|publish-release|cargo\s+(build|install)|tauri\s+build/);
  assert.doesNotMatch(workflowText, /if:\s*.*(success\(\)|failure\(\)|cancelled\(\))/);
  assert.doesNotMatch(helperText, /\/releases\/tags\//);
});
