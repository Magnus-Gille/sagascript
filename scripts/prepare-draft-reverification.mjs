import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { lstatSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Reverification never builds, signs, uploads, publishes, or changes a release.
// The workflow revision supplies the verifier; the release SHA identifies the
// immutable app being checked, which can predate that verifier correction.
export function validateSource(source, expected) {
  const { repo, version, sha, runId } = expected;
  assert.match(repo, /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/);
  assert.match(version, /^\d+\.\d+\.\d+$/);
  assert.match(sha, /^[0-9a-f]{40}$/);
  assert.match(runId, /^[1-9]\d*$/);
  const { run, jobs, tagCommit, release, windowsRelease } = source;
  assert.equal(String(run.id), runId, "source run ID mismatch");
  assert.equal(run.repository.full_name, repo, "source repository mismatch");
  assert.equal(run.path, ".github/workflows/release.yml", "source must be the release workflow");
  assert.equal(run.event, "push", "source must be a tag push");
  assert.equal(run.head_branch, "v" + version, "source tag mismatch");
  assert.equal(run.head_sha, sha, "source release SHA mismatch");
  assert.equal(run.status, "completed", "source release run is still active");
  assert.equal(tagCommit.sha, sha, "tag no longer points to the accepted release SHA");
  assert.ok(
    jobs.some((job) => job.name === "promote-release" && job.status === "completed" && job.conclusion === "success"),
    "source must have successfully promoted prebuilt artifacts",
  );
  for (const [metadata, tag] of [
    [release, "v" + version],
    [windowsRelease, "windows-beta-" + version],
  ]) {
    assert.equal(metadata.tag_name, tag, "release tag mismatch");
    assert.equal(metadata.draft, true, "reverification requires an unpublished draft");
    assert.ok(metadata.assets.length > 0, "draft has no assets");
    const names = new Set();
    for (const asset of metadata.assets) {
      assert.match(asset.name, /^[A-Za-z0-9][A-Za-z0-9._-]*$/, "unsafe asset name");
      assert.ok(!names.has(asset.name), "duplicate asset name");
      names.add(asset.name);
      assert.match(asset.digest, /^sha256:[0-9a-f]{64}$/, "missing SHA-256 asset digest");
      assert.ok(Number.isSafeInteger(asset.size) && asset.size > 0, "invalid asset size");
    }
  }
  assert.equal(release.prerelease, false, "stable draft must not be a prerelease");
  assert.equal(windowsRelease.prerelease, true, "Windows draft must remain a beta");
}

export function verifyAssets(directory, source) {
  const verified = [];
  for (const [folder, metadata] of [
    ["release", source.release],
    ["windows-beta", source.windowsRelease],
  ]) {
    const path = join(directory, folder);
    assert.ok(lstatSync(path).isDirectory() && !lstatSync(path).isSymbolicLink(), "asset directory is not a real directory");
    assert.deepEqual(
      readdirSync(path).sort(),
      metadata.assets.map((asset) => asset.name).sort(),
      "downloaded snapshot and supplied draft snapshot asset inventories differ",
    );
    for (const asset of metadata.assets) {
      const file = join(path, asset.name);
      const stat = lstatSync(file);
      assert.ok(stat.isFile() && !stat.isSymbolicLink(), "asset is not a regular file");
      assert.equal(stat.size, asset.size, "asset size differs from supplied draft snapshot: " + asset.name);
      const digest = "sha256:" + createHash("sha256").update(readFileSync(file)).digest("hex");
      assert.equal(digest, asset.digest, "asset hash differs from supplied draft snapshot: " + asset.name);
      verified.push({ releaseId: metadata.id, assetId: asset.id, name: asset.name, size: stat.size, digest });
    }
  }
  return verified;
}

export function expectedFromEnvironment(env = process.env) {
  const expected = {
    repo: env.GITHUB_REPOSITORY ?? "",
    version: env.RELEASE_VERSION ?? "",
    sha: env.RELEASE_SHA ?? "",
    runId: env.RELEASE_RUN_ID ?? "",
  };
  // Validate all inputs before they become API path segments or action inputs.
  assert.match(expected.repo, /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/);
  assert.match(expected.version, /^\d+\.\d+\.\d+$/);
  assert.match(expected.sha, /^[0-9a-f]{40}$/);
  assert.match(expected.runId, /^[1-9]\d*$/);
  assert.equal(env.GITHUB_REF, "refs/heads/main", "dispatch must use the reviewed main workflow");
  return expected;
}

// Draft discovery requires privileges the read-only job deliberately lacks.
// The authorized operator supplies a fresh provider snapshot and checks live
// state again after verification; CI binds that snapshot to the source artifact.
export function draftMetadataFromManifest(manifest, expected) {
  assert.equal(manifest.schema_version, 1, "unsupported draft manifest schema");
  assert.equal(manifest.repo, expected.repo, "draft manifest repository mismatch");
  assert.equal(manifest.version, expected.version, "draft manifest version mismatch");
  assert.equal(manifest.sha, expected.sha, "draft manifest release SHA mismatch");
  assert.ok(
    typeof manifest.capturedAt === "string" && Number.isFinite(Date.parse(manifest.capturedAt)),
    "draft manifest needs its capture timestamp",
  );
  for (const metadata of [manifest.release, manifest.windowsRelease]) {
    assert.ok(Number.isSafeInteger(metadata?.id) && metadata.id > 0, "draft manifest needs release IDs");
  }
  return { release: manifest.release, windowsRelease: manifest.windowsRelease, snapshotCapturedAt: manifest.capturedAt };
}

function fetchSource(expected) {
  const api = (path) => JSON.parse(execFileSync("gh", ["api", path], { encoding: "utf8", timeout: 30_000 }));
  const base = "repos/" + expected.repo;
  const metadata = draftMetadataFromManifest(JSON.parse(process.env.DRAFT_MANIFEST ?? ""), expected);
  return {
    run: api(base + "/actions/runs/" + expected.runId),
    jobs: api(base + "/actions/runs/" + expected.runId + "/jobs?per_page=100").jobs,
    tagCommit: api(base + "/commits/v" + expected.version),
    ...metadata,
  };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const mode = process.argv[2];
  assert.ok(mode === "preflight" || mode === "verify-assets", "expected preflight or verify-assets");
  const expected = expectedFromEnvironment();
  const source = fetchSource(expected);
  validateSource(source, expected);
  const receipt = {
    checkedAt: new Date().toISOString(),
    verifierSha: process.env.GITHUB_SHA,
    snapshotCapturedAt: source.snapshotCapturedAt,
    ...expected,
    releaseId: source.release.id,
    windowsReleaseId: source.windowsRelease.id,
  };
  if (mode === "verify-assets") receipt.assets = verifyAssets("draft-assets", source);
  writeFileSync("draft-reverification-receipt.json", JSON.stringify(receipt, null, 2) + "\n");
  console.log(JSON.stringify(receipt));
}
