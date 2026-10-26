// Toolchain pins (#353): one source of truth per tool, and nothing that drifts
// silently. rust-toolchain.toml pins the development/CI Rust release, `.nvmrc`
// the Node.js major, and `rust-version` the minimum supported Rust version.
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { commandOf, gatesFor } from "./ci/cargo-gates.mjs";
import { commandLines, loadAction, loadWorkflow } from "./lib/ci-workflow.mjs";

const read = (path) => readFileSync(fileURLToPath(new URL(`../${path}`, import.meta.url)), "utf8").replace(/\r\n?/g, "\n");

const toolchainFile = read("rust-toolchain.toml");
const channel = /^channel = "([^"]+)"\s*(?:#.*)?$/m.exec(toolchainFile)?.[1];
const nodeMajor = read(".nvmrc").trim();
// Every workflow, and the local composite actions (#358): ci.yml installs its toolchains through
// .github/actions/setup, so the pins live in the action file.
const workflows = Object.fromEntries([
  ...readdirSync(fileURLToPath(new URL("../.github/workflows/", import.meta.url)))
    .filter((name) => name.endsWith(".yml"))
    .map((name) => [name, read(`.github/workflows/${name}`)]),
  ...readdirSync(fileURLToPath(new URL("../.github/actions/", import.meta.url))).map((name) => [
    `actions/${name}/action.yml`,
    read(`.github/actions/${name}/action.yml`),
  ]),
]);
const rustVersion = (manifest) => /^rust-version = "([^"]+)"$/m.exec(read(manifest))?.[1];
const msrv = rustVersion("src-tauri/Cargo.toml");

// The `with:` block that follows a `uses:` line, as { key: value }.
function withInputs(text, usesIndex) {
  const lines = text.slice(usesIndex).split("\n").slice(1);
  const indent = /^(\s*)with:$/.exec(lines[0])?.[1];
  if (indent === undefined) return {};
  const inputs = {};
  for (const line of lines.slice(1)) {
    const match = new RegExp(`^${indent}  ([a-z-]+): (.*)$`).exec(line);
    if (!match) break;
    inputs[match[1]] = match[2].trim();
  }
  return inputs;
}

test("rust-toolchain.toml pins an exact release with the gate components", () => {
  assert.match(channel ?? "", /^\d+\.\d+\.\d+$/, "channel must be an exact x.y.z release, not stable/nightly");
  assert.match(toolchainFile, /^components = \[[^\]]*"clippy"[^\]]*\]$/m);
  assert.match(toolchainFile, /^components = \[[^\]]*"rustfmt"[^\]]*\]$/m);
});

test("every workflow Rust install is SHA-pinned and uses the rust-toolchain.toml channel", () => {
  let installs = 0;
  for (const [name, text] of Object.entries(workflows)) {
    assert.doesNotMatch(text, /dtolnay\/rust-toolchain@(stable|beta|nightly|master)\b/, name);
    for (const match of text.matchAll(/uses: dtolnay\/rust-toolchain@(\S+)/g)) {
      installs += 1;
      assert.match(match[1], /^[0-9a-f]{40}$/, `${name}: dtolnay/rust-toolchain must be pinned by commit SHA`);
      assert.equal(withInputs(text, match.index).toolchain, channel, `${name}: toolchain must equal rust-toolchain.toml`);
    }
  }
  assert.ok(installs >= 6, `expected the setup action and the release, packaging and gate installs, found ${installs}`);
});

test("a workflow that installs a different Rust release overrides the toolchain file explicitly", () => {
  // rust-toolchain.toml outranks `rustup default`, so a job that deliberately
  // measures with another release (code-health's 1.93.1 cohort) must export
  // RUSTUP_TOOLCHAIN or its unprefixed rustc/cargo calls silently switch.
  for (const [name, text] of Object.entries(workflows)) {
    for (const match of text.matchAll(/rustup toolchain install ([0-9]+(?:\.[0-9]+)*)\b/g)) {
      if (match[1] === channel) continue;
      // Exported for later steps too, not only for the installing step's shell.
      assert.ok(
        text.includes(`RUSTUP_TOOLCHAIN=${match[1]}" >> "$GITHUB_ENV"`),
        `${name}: installs Rust ${match[1]} but never writes RUSTUP_TOOLCHAIN=${match[1]} to $GITHUB_ENV`,
      );
    }
  }
});

test("the MSRV is declared once, inherited by every crate and checked in CI", () => {
  assert.match(msrv ?? "", /^\d+\.\d+(\.\d+)?$/);
  assert.equal(rustVersion("src-tauri/engine-host/ort/Cargo.toml"), msrv, "ORT host must declare the same MSRV");
  // Every workspace member: the app package and each crate under crates/ (the
  // workspace's `members = [".", "crates/*"]`), including test-only ones.
  const members = readdirSync(fileURLToPath(new URL("../src-tauri/crates/", import.meta.url)), { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => `src-tauri/crates/${entry.name}`);
  assert.ok(members.includes("src-tauri/crates/sagascript-core"), "crates/ must list the member crates");
  for (const crate of ["src-tauri", ...members]) {
    assert.match(read(`${crate}/Cargo.toml`), /^rust-version\.workspace = true$/m, `${crate} must inherit rust-version`);
  }
  // The MSRV lane installs the declared `rust-version` through the setup action, which exports
  // RUSTUP_TOOLCHAIN, and compiles with the gates' msrv profile.
  const workflow = loadWorkflow();
  const msrvLane = workflow.job("msrv-linux");
  const setup = msrvLane.steps.find((step) => step.uses === "./.github/actions/setup");
  assert.equal(setup?.with.rust, "msrv", "msrv-linux must install the declared MSRV, not a pinned release");
  const macosMsrvLane = workflow.job("msrv-macos");
  const macosSetup = macosMsrvLane.steps.find((step) => step.uses === "./.github/actions/setup");
  assert.equal(macosSetup?.with.rust, "msrv", "msrv-macos must install the declared MSRV, not a pinned release");
  assert.ok(
    macosMsrvLane.steps.some((step) => commandLines(step).includes("node scripts/ci/cargo-gates.mjs --os macos --profile msrv-live")),
    "msrv-macos must run the macOS record gate through the canonical cargo-gates script",
  );
  const install = loadAction("setup").steps.find((step) => step.name === "Install the declared MSRV toolchain");
  const lines = commandLines(install);
  assert.ok(
    lines.some((line) => line.includes("rust-version = ")),
    "the action must read the declared rust-version, not hardcode it",
  );
  assert.ok(lines.includes('echo "RUSTUP_TOOLCHAIN=$msrv" >> "$GITHUB_ENV"'));
  assert.ok(msrvLane.steps.some((step) => commandLines(step).includes("node scripts/ci/cargo-gates.mjs --profile msrv")));
  assert.ok(gatesFor({ os: "linux", profile: "msrv" }).some((gate) => commandOf(gate) === "cargo check --workspace --locked"));
  assert.ok(workflow.job("check-linux").needs.includes("msrv-linux"), "check-linux must gate on msrv-linux");
  assert.ok(workflow.job("check-macos").needs.includes("msrv-macos"), "check-macos must gate on msrv-macos");
});

test("Node.js is pinned to one major everywhere", () => {
  assert.match(nodeMajor, /^\d+$/);
  const nodeVersions = Object.entries(workflows).flatMap(([name, text]) =>
    [...text.matchAll(/node-version: (\S+)/g)].map((match) => [name, match[1]]),
  );
  assert.ok(nodeVersions.length > 0);
  for (const [name, version] of nodeVersions) assert.equal(version, nodeMajor, name);
  assert.equal(JSON.parse(read("package.json")).engines?.node, `>=${nodeMajor}`);
});

test("README, CONTRIBUTING and the installation guide state the same versions as the pin files", () => {
  for (const doc of ["README.md", "CONTRIBUTING.md", "docs/installation.md"]) {
    const text = read(doc);
    assert.ok(text.includes(`Rust ${msrv}+`), `${doc} must state Rust ${msrv}+`);
    assert.ok(text.includes(`Node.js ${nodeMajor}+`), `${doc} must state Node.js ${nodeMajor}+`);
    const escapedMsrv = msrv.replaceAll(".", "\\.");
    assert.doesNotMatch(text, new RegExp(`Rust (?!${escapedMsrv}\\+)1\\.\\d+\\+`), `${doc} names another Rust minimum`);
    assert.doesNotMatch(text, new RegExp(`Node\\.js (?!${nodeMajor}\\+)\\d+\\+`), `${doc} names another Node.js minimum`);
  }
});
