// Dependency policy (#355): cargo-deny and npm audit stay blocking CI gates,
// and src-tauri/deny.toml stays in step with what it summarises: the license
// families scripts/generate-third-party-notices.mjs reviews and the git
// sources the Cargo manifests actually use.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const read = async (path) =>
  (await readFile(new URL(`../${path}`, import.meta.url), "utf8")).replace(/\r\n?/g, "\n");

const [workflow, deny, notices, workspaceManifest, ortManifest] = await Promise.all([
  read(".github/workflows/ci.yml"),
  read("src-tauri/deny.toml"),
  read("scripts/generate-third-party-notices.mjs"),
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

// Each step as { name, raw, commands, workingDirectory }, in workflow order.
function steps(jobId) {
  return jobBlock(jobId)
    .split(/\n(?=      - name: )/)
    .slice(1)
    .map((raw) => {
      const name = /^      - name: (.*)$/m.exec(raw)[1];
      const inline = /^        run: (?![|>])(.*)$/m.exec(raw)?.[1];
      const block = /^        run: [|>]-?\n((?:          .*\n?|\n)*)/m.exec(raw)?.[1];
      const commands = (inline ?? block ?? "")
        .split("\n")
        .map((line) => line.trim())
        .filter(Boolean);
      const workingDirectory = /^        working-directory: (.*)$/m.exec(raw)?.[1];
      return { name, raw, commands, workingDirectory };
    });
}

function blockingStep(jobId, command, workingDirectory) {
  const all = steps(jobId);
  const matches = all.filter((step) => step.commands.includes(command));
  assert.equal(matches.length, 1, `${jobId} must run \`${command}\` exactly once`);
  const [step] = matches;
  assert.equal(step.workingDirectory, workingDirectory, `\`${command}\` must run in ${workingDirectory ?? "the repository root"}`);
  assert.doesNotMatch(step.raw, /^        (if|continue-on-error):/m, `\`${command}\` must be unconditional and blocking`);
  return all.indexOf(step);
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
  const names = steps("test-linux").map((step) => step.name);
  const install = blockingStep("test-linux", "cargo install --locked cargo-deny --version 0.20.2", undefined);
  const main = blockingStep("test-linux", "cargo deny --workspace check", "src-tauri");
  const ort = steps("test-linux")[main].commands.find((command) =>
    command.startsWith("cargo deny --manifest-path engine-host/ort/Cargo.toml --workspace check"),
  );
  assert.ok(ort, "the cargo-deny step must also check the standalone ORT engine host workspace");
  assert.ok(install < main, "cargo-deny must be installed before it runs");
  assert.ok(main > names.indexOf("Cache Rust dependencies"), "the cached cargo-deny binary must be restored first");
  assert.ok(main < names.indexOf("Cargo check (GUI app)"), "the policy must fail before any compilation");
});

test("test-macos gates the npm lockfile on npm audit", () => {
  const names = steps("test-macos").map((step) => step.name);
  const audit = blockingStep("test-macos", "npm audit --audit-level=high", undefined);
  assert.ok(audit > names.indexOf("Install npm dependencies"), "audit after npm ci");
  assert.ok(audit < names.indexOf("Build frontend"), "audit before the bundle is built");
});

test("deny.toml allows only license families the notices generator reviews", () => {
  const start = notices.indexOf("const reviewedRustLicenses = new Set([");
  assert.notEqual(start, -1, "generate-third-party-notices.mjs must keep its reviewed Rust license list");
  const reviewed = notices.slice(start, notices.indexOf("]);", start));
  const expressions = [...reviewed.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
  const identifiers = new Set(
    expressions.flatMap((expression) =>
      expression
        .replace(/[()]/g, " ")
        .split(/\s+(?:OR|AND)\s+|\s*\/\s*/)
        .map((term) => term.trim())
        .filter(Boolean),
    ),
  );
  for (const license of tomlStrings(deny, "licenses", "allow")) {
    assert.ok(identifiers.has(license), `deny.toml allows ${license}, which no reviewed expression contains`);
  }
  const exceptions = /^exceptions = \[([\s\S]*?)^\]/m.exec(deny)?.[1] ?? "";
  for (const [, license] of exceptions.matchAll(/allow = \["([^"]+)"\]/g)) {
    assert.ok(identifiers.has(license), `deny.toml excepts ${license}, which no reviewed expression contains`);
  }
});

test("deny.toml allows exactly the git sources the Cargo manifests use", () => {
  const used = new Set(
    [workspaceManifest, ortManifest].flatMap((manifest) =>
      [...manifest.replace(/#.*$/gm, "").matchAll(/\bgit = "([^"]+)"/g)].map((match) => match[1]),
    ),
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
