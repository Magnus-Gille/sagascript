import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const provenance = JSON.parse(readFileSync(join(repoRoot, "docs/code-health-contract-provenance.json"), "utf8"));
assert.equal(provenance.source.frozen_commit, "7df005ce952a52816597d9888da977d689a631fd");
assert.equal(provenance.source.merged_commit, "58428191034d9ac5e0c908346840c0219446599f");
assert.equal(provenance.files.length, 9);

for (const file of provenance.files) {
  const contents = readFileSync(join(repoRoot, file.path));
  const digest = createHash("sha256").update(contents).digest("hex");
  assert.equal(digest, file.sha256, `${file.path} differs from the frozen contract source`);
  assert.match(file.source_git_blob, /^[a-f0-9]{40}$/);
}

process.stdout.write(`verified ${provenance.files.length} frozen contract files\n`);
