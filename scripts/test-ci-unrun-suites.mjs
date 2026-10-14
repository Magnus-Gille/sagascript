import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// #357: suites that used to run in no PR lane (or only in the release gate)
// are now part of PR CI. These assertions keep them there.

const read = async (path) => (await readFile(new URL(`../${path}`, import.meta.url), "utf8")).replace(/\r\n?/g, "\n");

const [workflow, devOverrides, workspaceManifest, ortManifest] = await Promise.all([
  read(".github/workflows/ci.yml"),
  read("src-tauri/crates/sagascript-core/src/transcription/dev_overrides.rs"),
  read("src-tauri/Cargo.toml"),
  read("src-tauri/engine-host/ort/Cargo.toml"),
]);

function jobBlock(jobId) {
  const start = workflow.indexOf(`\n  ${jobId}:\n`);
  assert.notEqual(start, -1, `workflow must define ${jobId}`);
  const body = workflow.slice(start + 1);
  const next = body.slice(1).search(/^  [a-z][a-z0-9-]*:$/m);
  return next === -1 ? body : body.slice(0, next + 1);
}

// Each step as { name, raw, run, workingDirectory }, in workflow order.
function steps(jobId) {
  return jobBlock(jobId)
    .split(/\n(?=      - name: )/)
    .slice(1)
    .map((raw) => {
      const name = /^      - name: (.*)$/m.exec(raw)[1];
      const inline = /^        run: (?![|>])(.*)$/m.exec(raw)?.[1];
      const block = /^        run: [|>]-?\n((?:          .*\n?|\n)*)/m
        .exec(raw)?.[1]
        ?.split("\n")
        .map((line) => line.replace(/^          /, ""))
        .join("\n");
      const workingDirectory = /^        working-directory: (.*)$/m.exec(raw)?.[1];
      return { name, raw, run: (inline ?? block ?? "").trim(), workingDirectory };
    });
}

function stepRunning(jobId, command, workingDirectory) {
  const matches = steps(jobId).filter((step) => step.run === command);
  assert.equal(matches.length, 1, `${jobId} must run \`${command}\` exactly once as its own step`);
  const [step] = matches;
  assert.equal(step.workingDirectory, workingDirectory, `\`${command}\` must run in ${workingDirectory}`);
  assert.doesNotMatch(step.raw, /^        (if|continue-on-error):/m, `\`${command}\` must be unconditional and blocking`);
  return step;
}

test("test-linux runs the engine-protocol wire-format tests", () => {
  stepRunning("test-linux", "cargo test -p sagascript-engine-protocol", "src-tauri");
});

test("test-linux tests and lints the standalone ORT engine host workspace", () => {
  assert.match(ortManifest, /^\[workspace\]$/m, "the ORT host is its own Cargo workspace");
  stepRunning("test-linux", "cargo test --locked", "src-tauri/engine-host/ort");
  stepRunning("test-linux", "cargo clippy --all-targets --locked -- -D warnings", "src-tauri/engine-host/ort");
  const fmt = steps("test-linux").find((step) => step.name === "Check Rust formatting");
  assert.ok(fmt, "test-linux must keep its rustfmt gate");
  assert.match(fmt.run, /^cargo fmt --manifest-path engine-host\/ort\/Cargo\.toml --all -- --check$/m);

  const cache = steps("test-linux").find((step) => step.name === "Cache Rust dependencies");
  assert.ok(cache, "test-linux must keep its Rust cache");
  assert.match(cache.raw, /^          workspaces: \|\n            src-tauri\n            src-tauri\/engine-host\/ort\n/m);
});

test("test-linux runs the release-only dev-override guard in the release profile", () => {
  // The test exists only without debug assertions and without the feature...
  assert.match(
    devOverrides,
    /#\[cfg\(not\(any\(debug_assertions, feature = "dev-overrides"\)\)\)\]\n    #\[test\]\n    fn release_build_ignores_the_real_environment\(\)/,
  );
  // ...and its doc comment names the command CI must run.
  const command = "cargo test --release -p sagascript-core --lib dev_overrides";
  assert.ok(devOverrides.includes(`(\`${command}\`)`), "the test's doc comment must name the CI command");
  const guard = steps("test-linux").filter((step) => step.run.split("\n").some((line) => line.startsWith(command)));
  assert.equal(guard.length, 1, "test-linux must run the release guard exactly once");
  assert.equal(guard[0].workingDirectory, "src-tauri");
  assert.doesNotMatch(guard[0].raw, /^        (if|continue-on-error):/m, "the release guard must be unconditional and blocking");
  // cargo test exits 0 when the filter matches nothing; the step must prove
  // the guard test itself ran, and must not lose cargo's exit code in a pipe.
  assert.match(guard[0].run, /^set -o pipefail$/m);
  assert.match(guard[0].run, /^grep -q 'release_build_ignores_the_real_environment \.\.\. ok' /m);

  // A release profile with debug assertions would compile the guard out and
  // leave the step green with zero tests.
  const release = /^\[profile\.release\]\n([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(workspaceManifest)?.[1];
  assert.ok(release !== undefined, "src-tauri/Cargo.toml must define [profile.release]");
  assert.doesNotMatch(release, /^debug-assertions\s*=\s*true/m);
  assert.doesNotMatch(workflow, /CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS/);
});

test("test-windows runs the full frontend suite after npm ci", () => {
  const names = steps("test-windows").map((step) => step.name);
  const suite = stepRunning("test-windows", "npm run test:frontend", undefined);
  const index = names.indexOf(suite.name);
  assert.ok(names.indexOf("Install npm dependencies") >= 0, "test-windows must install npm dependencies");
  assert.ok(index > names.indexOf("Install npm dependencies"), "the suite needs node_modules");
  assert.ok(index < names.indexOf("Cargo check"), "the suite must fail before Cargo work");
  const identity = names.indexOf("Test Windows build identity gates");
  assert.ok(identity >= 0 && identity < names.indexOf("Install npm dependencies"), "the early build identity gates stay ahead of npm ci");
});

test("test-macos and test-windows run the Vitest unit tests and the Prettier check after npm ci", () => {
  for (const job of ["test-macos", "test-windows"]) {
    const names = steps(job).map((step) => step.name);
    const install = names.indexOf("Install npm dependencies");
    assert.ok(install >= 0, `${job} must install npm dependencies`);
    for (const command of ["npm run test:unit", "npm run format:check"]) {
      const step = stepRunning(job, command, undefined);
      const index = names.indexOf(step.name);
      assert.ok(index > install, `${job}: \`${command}\` needs node_modules`);
      assert.ok(index < names.indexOf("Cargo check"), `${job}: \`${command}\` must fail before Cargo work`);
    }
  }
});
