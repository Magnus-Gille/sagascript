import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import test from "node:test";

const workflow = (await readFile(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8"))
  .replace(/\r\n/g, "\n");

function jobBlock(jobId) {
  const start = workflow.indexOf(`  ${jobId}:`);
  assert.notEqual(start, -1, `workflow must define ${jobId}`);
  const remainder = workflow.slice(start);
  const body = remainder.slice(remainder.indexOf("\n") + 1);
  const nextJob = body.search(/^  [a-z][a-z0-9-]*:/m);
  return remainder.slice(0, nextJob === -1 ? remainder.length : remainder.indexOf("\n") + 1 + nextJob);
}

function scopedAggregator(jobId, expectedNeeds, laneResultNames, stepName) {
  const block = jobBlock(jobId);
  const needs = /^    needs: \[([^\]]+)\]$/m.exec(block)?.[1]
    .split(",")
    .map((value) => value.trim());
  assert.deepEqual(needs, expectedNeeds, `${jobId} must wait for scope and its applicable lane(s)`);
  assert.match(block, /^    if: \$\{\{ always\(\) \}\}$/m);
  assert.ok(block.includes("SCOPE_RESULT: ${{ needs.scope.result }}"));
  assert.ok(block.includes("SCOPE_TRUSTED: ${{ needs.scope.outputs.scope_trusted }}"));
  assert.ok(block.includes("DOCS_ONLY: ${{ needs.scope.outputs.docs_only }}"));
  for (const [variable, lane] of Object.entries(laneResultNames)) {
    assert.ok(block.includes(`${variable}: \${{ needs.${lane}.result }}`));
  }
  const run = parseSteps(block).get(stepName);
  assert.match(run, /test "\$SCOPE_RESULT" = success/);
  assert.match(run, /test "\$SCOPE_TRUSTED" = true/);
  assert.match(run, /if \[ "\$DOCS_ONLY" = true \]/);
  return run;
}

function runScopedAggregator(run, values) {
  execFileSync("bash", ["-euo", "pipefail", "-c", run], {
    env: { PATH: process.env.PATH, ...values },
    stdio: "pipe",
  });
}

for (const [jobId, needs, laneResultNames, stepName] of [
  ["check-macos", ["scope", "test-macos", "build-macos"], { TEST_RESULT: "test-macos", BUILD_RESULT: "build-macos" }, "Assert macOS CI lanes passed"],
  ["check-windows", ["scope", "test-windows", "build-windows"], { TEST_RESULT: "test-windows", BUILD_RESULT: "build-windows" }, "Assert Windows CI lanes passed"],
  ["check-linux", ["scope", "test-linux"], { LINUX_RESULT: "test-linux" }, "Assert Linux CI lane passed"],
]) {
  test(`${jobId} accepts only trusted docs-only skips or successful full lanes`, () => {
    const run = scopedAggregator(jobId, needs, laneResultNames, stepName);
    const laneVariables = Object.keys(laneResultNames);
    const correctDocsOnly = Object.fromEntries(laneVariables.map((variable) => [variable, "skipped"]));
    const correctFull = Object.fromEntries(laneVariables.map((variable) => [variable, "success"]));

    assert.doesNotThrow(() => runScopedAggregator(run, {
      SCOPE_RESULT: "success", SCOPE_TRUSTED: "true", DOCS_ONLY: "true", ...correctDocsOnly,
    }));
    assert.doesNotThrow(() => runScopedAggregator(run, {
      SCOPE_RESULT: "success", SCOPE_TRUSTED: "true", DOCS_ONLY: "false", ...correctFull,
    }));

    for (const scopeResult of ["failure", "cancelled", "skipped"]) {
      for (const docsOnly of ["true", "false"]) {
        assert.throws(() => runScopedAggregator(run, {
          SCOPE_RESULT: scopeResult, SCOPE_TRUSTED: "true", DOCS_ONLY: docsOnly,
          ...(docsOnly === "true" ? correctDocsOnly : correctFull),
        }));
      }
    }
    for (const trusted of ["false", "", "untrusted"]) {
      assert.throws(() => runScopedAggregator(run, {
        SCOPE_RESULT: "success", SCOPE_TRUSTED: trusted, DOCS_ONLY: "true", ...correctDocsOnly,
      }));
    }
    for (const docsOnly of ["", "unknown"]) {
      assert.throws(() => runScopedAggregator(run, {
        SCOPE_RESULT: "success", SCOPE_TRUSTED: "true", DOCS_ONLY: docsOnly, ...correctFull,
      }));
    }
    for (const variable of laneVariables) {
      for (const result of ["failure", "cancelled", "skipped"]) {
        assert.throws(() => runScopedAggregator(run, {
          SCOPE_RESULT: "success", SCOPE_TRUSTED: "true", DOCS_ONLY: "false",
          ...correctFull, [variable]: result,
        }));
      }
    }
  });
}

function parseSteps(job) {
  const lines = job.split("\n");
  const starts = lines
    .map((line, index) => (/^      - name: (.*)$/.exec(line) ? index : -1))
    .filter((index) => index >= 0);
  const result = new Map();

  for (let i = 0; i < starts.length; i += 1) {
    const start = starts[i];
    const end = starts[i + 1] ?? lines.length;
    const segment = lines.slice(start, end);
    const name = /^      - name: (.*)$/.exec(segment[0])[1];
    const runIndex = segment.findIndex((line) => /^        run:\s*/.test(line));
    if (runIndex < 0) continue;

    const runValue = segment[runIndex].replace(/^        run:\s*/, "");
    let command;
    if (runValue === "|" || runValue === ">" || runValue === ">-") {
      command = segment
        .slice(runIndex + 1)
        .filter((line) => line.length === 0 || line.startsWith("          "))
        .map((line) => line.replace(/^          /, ""))
        .join("\n");
    } else {
      command = runValue;
    }
    result.set(name, command.trim());
  }
  return result;
}

function normalizeCommand(command) {
  return command.replace(/\r\n/g, "\n").trim();
}

// These are the Cargo/native/smoke run bodies in the pre-split check-macos and
// check-windows jobs at base 1ab1128. Keeping the fixture explicit makes a
// dropped invocation fail this test even if the replacement workflow remains valid YAML.
const BASELINE_GATES = {
  macos: {
    test: [
      ["Cargo check", "cargo check"],
      ["Cargo test", "cargo test"],
      ["Cargo clippy", "cargo clippy -- -D warnings"],
      ["Cargo test (sagascript-core, no features)", "cargo test -p sagascript-core"],
      ["Cargo test (sagascript-cli)", "cargo test -p sagascript-cli"],
      ["Cargo clippy (sagascript-core, no features)", "cargo clippy -p sagascript-core --all-targets -- -D warnings"],
      ["Cargo clippy (sagascript-cli)", "cargo clippy -p sagascript-cli --all-targets -- -D warnings"],
      ["Cargo check (sagascript-core, diarization)", "cargo check -p sagascript-core --features diarization"],
      ["Cargo test (sagascript-core, diarization)", "cargo test -p sagascript-core --features diarization"],
      ["Cargo clippy (sagascript-core, diarization)", "cargo clippy -p sagascript-core --features diarization --all-targets -- -D warnings"],
    ],
    build: [
      ["Build native release binary (no installer)", "npx tauri build --config scripts/tauri-ci-prebuilt.json --no-bundle"],
      ["Verify native release binary", "BINARY=src-tauri/target/release/sagascript\n[[ -x \"$BINARY\" ]]\nfile \"$BINARY\""],
      ["Smoke test — help and version", "BINARY=src-tauri/target/release/sagascript\n$BINARY --help\n$BINARY transcribe --help\n$BINARY record --help"],
      ["Smoke test — config management", "BINARY=src-tauri/target/release/sagascript\n$BINARY config list\n$BINARY config get language\n$BINARY config set language sv\n$BINARY config get language | grep -q sv\n$BINARY config reset language\n$BINARY config get language | grep -q en\n$BINARY config path"],
      ["Smoke test — model listing and formats", "BINARY=src-tauri/target/release/sagascript\n$BINARY list-models\n$BINARY formats"],
      ["Smoke test — shell completions", "BINARY=src-tauri/target/release/sagascript\n$BINARY completions zsh > /dev/null\n$BINARY completions bash > /dev/null"],
      ["Smoke test — download model and transcribe", "BINARY=src-tauri/target/release/sagascript\n$BINARY download-model nb-whisper-tiny\nRESULT=$($BINARY transcribe --language no --model nb-whisper-tiny test-audio/norwegian-short-3s.mp3)\necho \"Transcription: $RESULT\"\necho \"$RESULT\" | grep -iq \"stortinget\""],
      ["Smoke test — transcribe with JSON output", "BINARY=src-tauri/target/release/sagascript\nSTDOUT=\"$RUNNER_TEMP/transcription-smoke.json\"\nSTDERR=\"$RUNNER_TEMP/transcription-smoke.stderr\"\n$BINARY transcribe --json --language no --model nb-whisper-tiny \\\n  test-audio/norwegian-short-3s.mp3 > \"$STDOUT\" 2> \"$STDERR\"\npython3 scripts/verify-json-cli-streams.py \"$STDOUT\" \"$STDERR\""],
    ],
  },
  windows: {
    test: [
      ["Cargo check", "cargo check"],
      ["Cargo test", "cargo test"],
      ["Cargo clippy", "cargo clippy -- -D warnings"],
      ["Cargo test (sagascript-core, no features)", "cargo test -p sagascript-core"],
      ["Cargo test (sagascript-cli)", "cargo test -p sagascript-cli"],
      ["Cargo clippy (sagascript-core, no features)", "cargo clippy -p sagascript-core --all-targets -- -D warnings"],
      ["Cargo clippy (sagascript-cli)", "cargo clippy -p sagascript-cli --all-targets -- -D warnings"],
    ],
    build: [
      ["Build native release binary (no installer)", "npx tauri build --config scripts/tauri-ci-prebuilt.json --no-bundle"],
      ["Verify native release binary", "$binary = \"src-tauri\\target\\release\\sagascript.exe\"\nif (-not (Test-Path -PathType Leaf $binary)) {\n  throw \"Missing native release binary: $binary\"\n}"],
      ["Smoke test — help and version", "$env:BINARY = \"src-tauri\\target\\release\\sagascript.exe\"\n& $env:BINARY --help\n& $env:BINARY transcribe --help\n& $env:BINARY record --help"],
      ["Smoke test — config management", "$env:BINARY = \"src-tauri\\target\\release\\sagascript.exe\"\n& $env:BINARY config list\n& $env:BINARY config get language\n& $env:BINARY config set language sv\n$result = & $env:BINARY config get language\nif ($result -notmatch \"sv\") { throw \"Expected 'sv'\" }\n& $env:BINARY config reset language\n$result = & $env:BINARY config get language\nif ($result -notmatch \"en\") { throw \"Expected 'en'\" }\n& $env:BINARY config path"],
      ["Smoke test — model listing and formats", "$env:BINARY = \"src-tauri\\target\\release\\sagascript.exe\"\n& $env:BINARY list-models\n& $env:BINARY formats"],
      ["Smoke test — shell completions", "$env:BINARY = \"src-tauri\\target\\release\\sagascript.exe\"\n& $env:BINARY completions powershell | Out-Null"],
      ["Smoke test — download model and transcribe", "$env:BINARY = \"src-tauri\\target\\release\\sagascript.exe\"\n& $env:BINARY download-model nb-whisper-tiny\n$result = & $env:BINARY transcribe --language no --model nb-whisper-tiny test-audio/norwegian-short-3s.mp3\nWrite-Host \"Transcription: $result\"\nif ($result -notmatch \"(?i)stortinget\") { throw \"Expected 'stortinget' in output\" }"],
      ["Smoke test — transcribe with JSON output", "set BINARY=src-tauri\\target\\release\\sagascript.exe\n%BINARY% transcribe --json --language no --model nb-whisper-tiny test-audio/norwegian-short-3s.mp3 > json_output.txt 2>nul\ntype json_output.txt\nfindstr /C:\"text\" json_output.txt\nfindstr /C:\"duration_seconds\" json_output.txt\nfindstr /C:\"language\" json_output.txt"],
    ],
  },
};

for (const [platform, jobIds] of Object.entries({
  macos: { test: "test-macos", build: "build-macos" },
  windows: { test: "test-windows", build: "build-windows" },
})) {
  for (const lane of ["test", "build"]) {
    test(`${platform} ${lane} lane retains every baseline gate`, () => {
      const block = jobBlock(jobIds[lane]);
      assert.match(block, /^    needs: scope$/m, "each lane must wait only for the scope gate");
      assert.match(
        block,
        /^    if: \$\{\{ needs\.scope\.result == 'success' && needs\.scope\.outputs\.docs_only != 'true' \}\}$/m,
      );
      const commands = parseSteps(block);
      for (const [name, expected] of BASELINE_GATES[platform][lane]) {
        assert.equal(
          commands.get(name),
          normalizeCommand(expected),
          `${jobIds[lane]} must retain the baseline ${name} command`,
        );
      }
    });
  }
}

test("parallel lanes preserve blocking checks and existing Windows smoke exceptions", () => {
  for (const jobId of ["test-macos", "test-windows", "build-macos"]) {
    assert.doesNotMatch(jobBlock(jobId), /continue-on-error:/);
  }
  const windows = jobBlock("build-windows");
  assert.equal((windows.match(/continue-on-error: true/g) ?? []).length, 2);
  for (const name of ["Smoke test — download model and transcribe", "Smoke test — transcribe with JSON output"]) {
    const step = windows.slice(windows.indexOf(`      - name: ${name}`)).split(/\n      - name:/)[0];
    assert.match(step, /continue-on-error: true/);
  }
});

test("model caches are isolated to native lanes", () => {
  assert.doesNotMatch(jobBlock("test-macos"), /name: Cache downloaded models/);
  assert.doesNotMatch(jobBlock("test-windows"), /name: Cache downloaded models/);
  assert.match(jobBlock("build-macos"), /name: Cache downloaded models/);
  assert.match(jobBlock("build-windows"), /name: Cache downloaded models/);
  assert.match(jobBlock("test-macos"), /shared-key: sagascript-ci-checks/);
  assert.match(jobBlock("test-windows"), /shared-key: sagascript-ci-checks/);
  assert.match(jobBlock("build-macos"), /shared-key: sagascript-ci-native/);
  assert.match(jobBlock("build-windows"), /shared-key: sagascript-ci-native/);
});

function aggregator(jobId, expectedNeeds, testResultName, buildResultName) {
  const block = jobBlock(jobId);
  const needs = /^    needs: \[([^\]]+)\]$/m.exec(block)?.[1]
    .split(",")
    .map((value) => value.trim());
  assert.deepEqual(needs, expectedNeeds, `${jobId} must wait for scope and its applicable lane(s)`);
  assert.match(block, /^    if: \$\{\{ always\(\) \}\}$/m);
  assert.match(block, new RegExp(`TEST_RESULT: \\\$\\{\\{ needs\\.${testResultName}\\.result \\\}\\}`));
  assert.match(block, new RegExp(`BUILD_RESULT: \\\$\\{\\{ needs\\.${buildResultName}\\.result \\\}\\}`));
  const run = parseSteps(block).get(jobId === "check-macos" ? "Assert macOS CI lanes passed" : "Assert Windows CI lanes passed");
  assert.match(run, /test "\$TEST_RESULT" = success/);
  assert.match(run, /test "\$BUILD_RESULT" = success/);
  return run;
}

function runAggregator(run, testResult, buildResult) {
  execFileSync("bash", ["-euo", "pipefail", "-c", run], {
    env: {
      PATH: process.env.PATH,
      SCOPE_RESULT: "success",
      SCOPE_TRUSTED: "true",
      DOCS_ONLY: "false",
      TEST_RESULT: testResult,
      BUILD_RESULT: buildResult,
    },
    stdio: "pipe",
  });
}

for (const [jobId, needs] of [
  ["check-macos", ["scope", "test-macos", "build-macos"]],
  ["check-windows", ["scope", "test-windows", "build-windows"]],
]) {
  test(`${jobId} fails closed for every non-success lane result`, () => {
    const [, testLane, buildLane] = needs;
    const run = aggregator(jobId, needs, testLane, buildLane);
    for (const testResult of ["success", "failure", "cancelled", "skipped"]) {
      for (const buildResult of ["success", "failure", "cancelled", "skipped"]) {
        const invoke = () => runAggregator(run, testResult, buildResult);
        if (testResult === "success" && buildResult === "success") {
          assert.doesNotThrow(invoke);
        } else {
          assert.throws(invoke, `${jobId} must reject ${testResult}/${buildResult}`);
        }
      }
    }
  });
}
