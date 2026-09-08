import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const [cliManifest, coreManifest, ciWorkflow] = await Promise.all([
  readFile(new URL("../src-tauri/crates/sagascript-cli/Cargo.toml", import.meta.url), "utf8"),
  readFile(new URL("../src-tauri/crates/sagascript-core/Cargo.toml", import.meta.url), "utf8"),
  readFile(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8"),
]);

function macosWorkflowFor(workflow) {
  const normalized = workflow.replace(/\r\n/g, "\n");
  return normalized.slice(
    normalized.indexOf("  check-macos:"),
    normalized.indexOf("  check-linux:"),
  );
}

function assertMacosDiarizationCoverage(workflow) {
  const macosWorkflow = macosWorkflowFor(workflow);
  assert.match(
    cliManifest,
    /default\s*=\s*\[\s*"record"\s*,\s*"diarization"\s*\]/,
    "CLI default features must include diarization",
  );
  assert.match(coreManifest, /default\s*=\s*\[\s*\]/, "core must retain its no-feature default");

  assert.match(macosWorkflow, /run: cargo test -p sagascript-cli\n/);
  assert.match(macosWorkflow, /run: cargo clippy -p sagascript-cli --all-targets -- -D warnings\n/);
  assert.doesNotMatch(macosWorkflow, /-p sagascript-cli --features diarization/);

  assert.equal(
    (macosWorkflow.match(/-p sagascript-core --features diarization/g) ?? []).length,
    3,
    "core check, test, and all-target clippy gates must remain separate",
  );
}

for (const [label, lineEnding] of [
  ["LF", "\n"],
  ["CRLF", "\r\n"],
]) {
  test(`macOS CI keeps CLI diarization in defaults and gates core explicitly (${label})`, () => {
    assertMacosDiarizationCoverage(ciWorkflow.replace(/\r?\n/g, lineEnding));
  });
}
