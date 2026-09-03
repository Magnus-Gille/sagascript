import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

import { createSnapshot } from "./record-release-downloads.mjs";

const asset = (name, downloadCount, overrides = {}) => ({
  name,
  content_type: "application/octet-stream",
  size: 100,
  download_count: downloadCount,
  ...overrides,
});

const release = (tagName, publishedAt, assets = [], overrides = {}) => ({
  tag_name: tagName,
  published_at: publishedAt,
  draft: false,
  prerelease: false,
  assets,
  ...overrides,
});

test("normalizes public releases deterministically and totals download events", () => {
  const snapshot = createSnapshot(
    [
      release("v2.0.0-beta", "2026-09-02T00:00:00Z", [asset("preview.txt", 11)], {
        prerelease: true,
      }),
      release("v1.0.0", "2026-09-01T00:00:00Z", [
        asset("Sagascript.dmg", 13),
        asset("notes.txt", 7),
        asset("Sagascript.app.tar.gz", 3),
        asset("Sagascript-Setup.MSI", 2),
        asset("Sagascript-Setup.exe", 5),
      ]),
      release("draft", "2026-08-01T00:00:00Z", [asset("Sagascript.dmg", 999)], {
        draft: true,
      }),
    ],
    "Magnus-Gille/sagascript",
    "2026-09-03T23:45:00+02:00",
  );

  assert.equal(snapshot.captured_at, "2026-09-03T21:45:00.000Z");
  assert.equal(snapshot.snapshot_date, "2026-09-03");
  assert.deepEqual(snapshot.releases.map(({ tag_name }) => tag_name), [
    "v1.0.0",
    "v2.0.0-beta",
  ]);
  assert.equal(snapshot.releases[1].prerelease, true);
  assert.deepEqual(snapshot.releases[0].assets.map(({ name }) => name), [
    "Sagascript-Setup.MSI",
    "Sagascript-Setup.exe",
    "Sagascript.app.tar.gz",
    "Sagascript.dmg",
    "notes.txt",
  ]);
  assert.deepEqual(snapshot.totals, {
    all_assets: 41,
    app_downloads: 23,
    dmg_downloads: 13,
    updater_downloads: 3,
    windows_downloads: 7,
  });
});

test("sorts equal-time releases by tag name", () => {
  const snapshot = createSnapshot(
    [release("v2", "2026-09-01T00:00:00Z"), release("v1", "2026-09-01T00:00:00Z")],
    "owner/repo",
    "2026-09-03T00:00:00Z",
  );
  assert.deepEqual(snapshot.releases.map(({ tag_name }) => tag_name), ["v1", "v2"]);
});

test("rejects malformed timestamps and asset counters", () => {
  assert.throws(
    () => createSnapshot([], "owner/repo", "2026-09-03"),
    /captured-at.*ISO 8601/i,
  );
  assert.throws(
    () =>
      createSnapshot(
        [release("v1", "not-a-time")],
        "owner/repo",
        "2026-09-03T00:00:00Z",
      ),
    /published_at.*ISO 8601/i,
  );

  for (const [field, value] of [
    ["size", -1],
    ["size", 1.5],
    ["download_count", -1],
    ["download_count", "1"],
  ]) {
    assert.throws(
      () =>
        createSnapshot(
          [release("v1", "2026-09-01T00:00:00Z", [asset("bad", 1, { [field]: value })])],
          "owner/repo",
          "2026-09-03T00:00:00Z",
        ),
      new RegExp(`${field}.*non-negative integer`, "i"),
    );
  }
});

test("rejects malformed repository, release, and asset fields", () => {
  const capturedAt = "2026-09-03T00:00:00Z";
  assert.throws(() => createSnapshot([], "missing-slash", capturedAt), /owner\/repo/);

  for (const [input, expected] of [
    [[release("", "2026-09-01T00:00:00Z")], /tag_name.*non-empty string/],
    [
      [release("v1", "2026-09-01T00:00:00Z", [], { prerelease: "false" })],
      /prerelease.*boolean/,
    ],
    [[release("v1", "2026-09-01T00:00:00Z", {})], /assets.*array/],
    [
      [release("v1", "2026-09-01T00:00:00Z", [asset("", 1)])],
      /assets\[0\]\.name.*non-empty string/,
    ],
    [
      [
        release("v1", "2026-09-01T00:00:00Z", [
          asset("file", 1, { content_type: null }),
        ]),
      ],
      /content_type.*string/,
    ],
  ]) {
    assert.throws(() => createSnapshot(input, "owner/repo", capturedAt), expected);
  }
});

test("CLI rejects unknown, missing, and duplicate flags", () => {
  const run = (arguments_) =>
    spawnSync(process.execPath, ["scripts/record-release-downloads.mjs", ...arguments_], {
      cwd: new URL("..", import.meta.url),
      encoding: "utf8",
    });

  const unknown = run(["--unknown", "value"]);
  assert.notEqual(unknown.status, 0);
  assert.match(unknown.stderr, /unknown flag.*--unknown/i);

  const missing = run(["--input", "releases.json"]);
  assert.notEqual(missing.status, 0);
  assert.match(missing.stderr, /missing required flag.*--output/i);

  const duplicate = run([
    "--input",
    "one.json",
    "--input",
    "two.json",
    "--output",
    "snapshot.json",
    "--repository",
    "owner/repo",
  ]);
  assert.notEqual(duplicate.status, 0);
  assert.match(duplicate.stderr, /duplicate flag.*--input/i);
});

test("CLI rejects missing flag values", () => {
  const result = spawnSync(
    process.execPath,
    ["scripts/record-release-downloads.mjs", "--input", "--output"],
    { cwd: new URL("..", import.meta.url), encoding: "utf8" },
  );
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /missing value.*--input/i);
});

test("CLI writes pretty JSON with a trailing newline and creates parent directories", () => {
  const directory = mkdtempSync(join(tmpdir(), "sagascript-download-metrics-"));
  try {
    const input = join(directory, "releases.json");
    const output = join(directory, "nested", "snapshot.json");
    writeFileSync(input, JSON.stringify([release("v1", "2026-09-01T00:00:00Z")]), "utf8");

    const result = spawnSync(
      process.execPath,
      [
        "scripts/record-release-downloads.mjs",
        "--input",
        input,
        "--output",
        output,
        "--repository",
        "owner/repo",
        "--captured-at",
        "2026-09-03T00:00:00Z",
      ],
      { cwd: new URL("..", import.meta.url), encoding: "utf8" },
    );

    assert.equal(result.status, 0, result.stderr);
    const contents = readFileSync(output, "utf8");
    assert.match(contents, /^\{\n  "schema_version": 1,/);
    assert.ok(contents.endsWith("\n"));
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
