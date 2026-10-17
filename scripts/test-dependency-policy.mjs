// Dependency policy (#355): cargo-deny and npm audit stay blocking CI gates,
// and src-tauri/deny.toml stays in step with what it summarises: the license
// families scripts/generate-third-party-notices.mjs reviews and the git
// sources the Cargo manifests actually use.
import assert from "node:assert/strict";
import { readdirSync } from "node:fs";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { commandOf, gatesFor } from "./ci/cargo-gates.mjs";
import { commandLines, loadWorkflow } from "./lib/ci-workflow.mjs";

const read = async (path) => (await readFile(new URL(`../${path}`, import.meta.url), "utf8")).replace(/\r\n?/g, "\n");

const [deny, notices, ...manifests] = await Promise.all([
  read("src-tauri/deny.toml"),
  read("scripts/generate-third-party-notices.mjs"),
  ...[
    "src-tauri/Cargo.toml",
    "src-tauri/engine-host/ort/Cargo.toml",
    ...readdirSync(new URL("../src-tauri/crates/", import.meta.url)).map((crate) => `src-tauri/crates/${crate}/Cargo.toml`),
  ].map(read),
]);

const workflow = loadWorkflow();
const stepIndex = (jobId, predicate) => workflow.job(jobId).steps.findIndex(predicate);
const runs = (command) => (step) => commandLines(step).includes(command);
// A step that gates unconditionally: no `if`, no `continue-on-error`.
function assertBlocking(step, what) {
  assert.equal(step.condition, undefined, `${what} must be unconditional`);
  assert.equal(step.continueOnError, undefined, `${what} must be blocking`);
}

// The string items of a TOML array `key = [ ... ]` inside `[section]`.
function tomlStrings(text, section, key) {
  const body = new RegExp(`^\\[${section}\\]\\n([\\s\\S]*?)(?=^\\[|(?![\\s\\S]))`, "m").exec(text)?.[1];
  assert.ok(body !== undefined, `deny.toml must have a [${section}] section`);
  const array = new RegExp(`^${key} = \\[([^\\]]*)\\]`, "m").exec(body)?.[1];
  assert.ok(array !== undefined, `[${section}] must set ${key}`);
  return [...array.replace(/#.*$/gm, "").matchAll(/"([^"]+)"/g)].map((match) => match[1]);
}

test("test-linux gates both Cargo workspaces on cargo-deny before compiling", () => {
  const INSTALL = "cargo install --locked cargo-deny --version 0.20.2";
  const linux = gatesFor({ os: "linux" });
  // The ORT host run may only silence the "policy entry not used here" notes,
  // not skip a check or allow a finding.
  assert.deepEqual(
    linux.slice(0, 2).map((gate) => [gate.cwd, commandOf(gate)]),
    [
      ["src-tauri", "cargo deny --locked --workspace check"],
      [
        "src-tauri",
        "cargo deny --locked --manifest-path engine-host/ort/Cargo.toml --workspace check -A license-not-encountered -A license-exception-not-encountered -A unmatched-source",
      ],
    ],
    "the cargo-deny gates must check both workspaces, every check kind, against the committed lockfiles, ahead of every gate that compiles",
  );
  assert.ok(
    linux.slice(2).every((gate) => gate.args[0] !== "deny"),
    "no other gate runs cargo-deny",
  );
  const steps = workflow.job("test-linux").steps;
  const setup = stepIndex("test-linux", (step) => step.uses === "./.github/actions/setup");
  const install = stepIndex("test-linux", runs(INSTALL));
  const gates = stepIndex("test-linux", (step) => commandLines(step).includes("node scripts/ci/cargo-gates.mjs --os linux"));
  assert.ok(setup >= 0 && install >= 0 && gates >= 0);
  assertBlocking(steps[install], "the cargo-deny install");
  assertBlocking(steps[gates], "the cargo gates");
  assert.ok(setup < install, "the cached cargo-deny binary must be restored first");
  assert.ok(install < gates, "cargo-deny must be installed before it runs");
});

test("npm run verify runs the same cargo-deny release CI installs", async () => {
  const { CARGO_DENY_VERSION } = await import("./verify.mjs");
  assert.ok(
    workflow.job("test-linux").steps.some(runs(`cargo install --locked cargo-deny --version ${CARGO_DENY_VERSION}`)),
    `verify's preflight asks for cargo-deny ${CARGO_DENY_VERSION}; test-linux must install the same release`,
  );
});

test("test-macos gates the npm lockfile on npm audit", () => {
  const audit = stepIndex("test-macos", runs("npm audit --audit-level=high"));
  assert.ok(audit >= 0, "test-macos must run npm audit");
  assertBlocking(workflow.job("test-macos").steps[audit], "`npm audit --audit-level=high`");
  assert.ok(audit > stepIndex("test-macos", (step) => step.uses === "./.github/actions/setup"), "audit after npm ci (the setup action)");
  assert.ok(audit < stepIndex("test-macos", runs("npm run build")), "audit before the bundle is built");
});

// SPDX expression satisfiability: AND binds tighter than OR, the legacy "/"
// means OR, and "X WITH Y" is one license that must be allowed as written.
function satisfiable(expression, allowed) {
  const tokens = expression.match(/\(|\)|\/|[^\s()/]+/g);
  let position = 0;
  const peek = () => tokens[position];
  function atom() {
    if (peek() === "(") {
      position += 1;
      const value = or();
      assert.equal(tokens[position++], ")", `unbalanced parentheses in ${expression}`);
      return value;
    }
    let license = tokens[position++];
    if (peek() === "WITH") {
      license = `${license} WITH ${tokens[position + 1]}`;
      position += 2;
    }
    return allowed.has(license);
  }
  function and() {
    let value = atom();
    while (peek() === "AND") {
      position += 1;
      value = atom() && value;
    }
    return value;
  }
  function or() {
    let value = and();
    while (peek() === "OR" || peek() === "/") {
      position += 1;
      value = and() || value;
    }
    return value;
  }
  const value = or();
  assert.equal(position, tokens.length, `could not parse ${expression}`);
  return value;
}

test("deny.toml allows exactly the license families the notices generator needs", () => {
  const start = notices.indexOf("const reviewedRustLicenses = new Set([");
  assert.notEqual(start, -1, "generate-third-party-notices.mjs must keep its reviewed Rust license list");
  const reviewed = notices.slice(start, notices.indexOf("]);", start));
  const expressions = [...reviewed.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
  const allowed = new Set(tomlStrings(deny, "licenses", "allow"));
  // Complete: every reviewed expression can be satisfied from the allow list.
  for (const expression of expressions) {
    assert.ok(satisfiable(expression, allowed), `deny.toml cannot satisfy the reviewed expression ${expression}`);
  }
  // Minimal: every allowed license is needed by some reviewed expression, so
  // nothing is allowed on its own that the notices only accept as an
  // alternative (0BSD, BSD-2-Clause, BSL-1.0, MIT-0 ...) or do not review at all.
  for (const license of allowed) {
    const without = new Set([...allowed].filter((other) => other !== license));
    assert.ok(
      expressions.some((expression) => !satisfiable(expression, without)),
      `deny.toml allows ${license}, which no reviewed expression needs`,
    );
  }
  const identifiers = new Set(expressions.flatMap((expression) => expression.match(/[^\s()/]+/g)));
  const exceptions = /^exceptions = \[([\s\S]*?)^\]/m.exec(deny)?.[1] ?? "";
  for (const [, license] of exceptions.matchAll(/allow = \["([^"]+)"\]/g)) {
    for (const term of license.split(" WITH ")) {
      assert.ok(identifiers.has(term), `deny.toml excepts ${license}, which no reviewed expression contains`);
    }
  }
});

test("deny.toml judges every shipped platform, feature and advisory kind", () => {
  assert.deepEqual(
    new Set(tomlStrings(deny, "graph", "targets")),
    new Set(["aarch64-apple-darwin", "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc", "x86_64-unknown-linux-gnu"]),
  );
  assert.match(deny, /^all-features = true$/m);
  for (const kind of ["unsound", "unmaintained"]) {
    assert.match(deny, new RegExp(`^${kind} = "(workspace|all)"$`, "m"), `${kind} advisories must still fail direct dependencies`);
  }
  assert.doesNotMatch(deny, /^yanked = "allow"$/m);
  // An ignore is an inline table with the advisory id and a reason.
  const ignore = /^ignore = \[([\s\S]*?)\]$/m.exec(deny)?.[1];
  assert.ok(ignore !== undefined, "[advisories] must set ignore");
  const entries = ignore
    .replace(/#.*$/gm, "")
    .split(/\}\s*,?/)
    .map((entry) => entry.trim())
    .filter(Boolean);
  for (const entry of entries) {
    assert.match(entry, /^\{\s*id = "RUSTSEC-\d{4}-\d{4}",\s*reason = "[^"]{10,}"$/, `ignore entry needs an id and a reason: ${entry}`);
  }
});

test("deny.toml allows exactly the git sources the Cargo manifests use", () => {
  const used = new Set(
    manifests.flatMap((manifest) => [...manifest.replace(/#.*$/gm, "").matchAll(/\bgit = "([^"]+)"/g)].map((match) => match[1])),
  );
  assert.deepEqual(new Set(tomlStrings(deny, "sources", "allow-git")), used);
  assert.match(deny, /^unknown-git = "deny"$/m);
  assert.match(deny, /^unknown-registry = "deny"$/m);
  assert.match(deny, /^required-git-spec = "rev"$/m);
});

test("deny.toml keeps a single reqwest in the graph", () => {
  assert.match(deny, /\{ crate = "reqwest", deny-multiple-versions = true\b/);
  assert.match(deny, /^multiple-versions = "warn"$/m);
});
