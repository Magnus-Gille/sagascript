// Workflow hardening (#354): every workflow starts from a read-only token,
// every job that runs steps has a timeout below the 360-minute default, every
// action is pinned to a commit SHA with the release it resolves to, write
// scopes exist only in reviewed publishing jobs, and installed tools are
// pinned to exact versions.
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";

const directory = fileURLToPath(new URL("../.github/workflows/", import.meta.url));
const workflows = Object.fromEntries(
  readdirSync(directory)
    .filter((name) => /\.ya?ml$/.test(name))
    .sort()
    .map((name) => [name, readFileSync(`${directory}${name}`, "utf8").replace(/\r\n?/g, "\n")]),
);

// Every job allowed to hold a write scope, as "workflow:job:scope". A new
// writer must be added here, which puts it in front of a reviewer.
const WRITERS = new Map([
  ["download-metrics.yml:snapshot:contents", "pushes the daily snapshot to the metrics/downloads branch"],
  ["release.yml:promote-release:contents", "creates the draft releases from a verified prebuild"],
  ["release.yml:publish-release:contents", "creates the draft release on the fallback path"],
  ["sbom.yml:attach:contents", "uploads the SBOMs to a published release"],
]);

// Actions that publish no release tags, so no `# vX` comment can name one.
// They must still be SHA-pinned and carry an explanatory comment.
const UNVERSIONED_ACTIONS = new Set(["dtolnay/rust-toolchain"]);

const DEFAULT_TIMEOUT_MINUTES = 360;

// Parses a `permissions:` value: an inline value (`read-all`, `write-all`,
// `{}`) or the indented `scope: level` lines that follow it.
function parsePermissions(lines, index, indent) {
  const inline = new RegExp(`^${indent}permissions:\\s*([^#\\s][^#]*?)?\\s*(?:#.*)?$`).exec(lines[index]);
  const scopes = new Map();
  if (inline?.[1]) {
    scopes.set("*", inline[1].trim());
    return scopes;
  }
  for (const line of lines.slice(index + 1)) {
    if (/^\s*(#.*)?$/.test(line)) continue;
    const scope = new RegExp(`^${indent}  ([a-z-]+):\\s*(read|write|none)\\s*(?:#.*)?$`).exec(line);
    if (!scope) break;
    scopes.set(scope[1], scope[2]);
  }
  return scopes;
}

function topLevelPermissions(text) {
  const lines = text.split("\n");
  const index = lines.findIndex((line) => /^permissions:/.test(line));
  return index < 0 ? null : parsePermissions(lines, index, "");
}

// Splits the `jobs:` mapping into { id, lines } blocks.
function jobs(text) {
  const lines = text.split("\n");
  const start = lines.indexOf("jobs:");
  assert.ok(start >= 0, "workflow must define jobs");
  const end = lines.findIndex((line, index) => index > start && /^[A-Za-z]/.test(line));
  const body = lines.slice(start + 1, end < 0 ? lines.length : end);
  const result = [];
  for (const line of body) {
    const key = /^  ([A-Za-z0-9_-]+):\s*$/.exec(line);
    if (key) result.push({ id: key[1], lines: [] });
    else if (result.length > 0) result.at(-1).lines.push(line);
  }
  assert.ok(result.length > 0, "workflow must define at least one job");
  return result;
}

function jobPermissions(job) {
  const index = job.lines.findIndex((line) => /^    permissions:/.test(line));
  return index < 0 ? null : parsePermissions(job.lines, index, "    ");
}

function writeScopes(scopes) {
  if (!scopes) return [];
  return [...scopes].flatMap(([scope, level]) => {
    if (scope === "*") return /write/.test(level) ? ["*"] : [];
    return level === "write" ? [scope] : [];
  });
}

for (const [name, text] of Object.entries(workflows)) {
  test(`${name} declares a least-privilege top-level permissions block`, () => {
    const scopes = topLevelPermissions(text);
    assert.ok(scopes, `${name} must declare top-level permissions`);
    assert.equal(scopes.get("contents"), "read", `${name} must default to contents: read`);
    assert.deepEqual(writeScopes(scopes), [], `${name} grants writes at the top level; grant them per job instead`);
  });

  test(`${name} bounds every job that runs steps with timeout-minutes`, () => {
    for (const job of jobs(text)) {
      const runsSteps = job.lines.some((line) => /^    steps:\s*$/.test(line));
      const callsWorkflow = job.lines.some((line) => /^    uses:\s*\S/.test(line));
      const timeout = job.lines.map((line) => /^    timeout-minutes:\s*(\d+)\s*(?:#.*)?$/.exec(line)).find(Boolean);
      if (callsWorkflow) {
        // GitHub rejects timeout-minutes on a reusable-workflow call; the
        // callee's own jobs carry the timeout.
        assert.equal(timeout, undefined, `${name}:${job.id} calls a reusable workflow and cannot take timeout-minutes`);
        continue;
      }
      assert.ok(runsSteps, `${name}:${job.id} must run steps or call a reusable workflow`);
      assert.ok(timeout, `${name}:${job.id} must set timeout-minutes`);
      const minutes = Number(timeout[1]);
      assert.ok(minutes > 0 && minutes < DEFAULT_TIMEOUT_MINUTES, `${name}:${job.id} timeout ${minutes} must be below the ${DEFAULT_TIMEOUT_MINUTES}-minute default`);
    }
  });

  test(`${name} pins every action to a commit SHA with its release in a comment`, () => {
    let actions = 0;
    for (const [line, ref, comment] of text.matchAll(/^\s*(?:-\s+)?uses:\s*(\S+)(.*)$/gm)) {
      if (ref.startsWith("./")) continue;
      actions += 1;
      const pinned = /^([^@\s]+)@([0-9a-f]{40})$/.exec(ref);
      assert.ok(pinned, `${name}: ${line.trim()} must be pinned to a full commit SHA`);
      if (UNVERSIONED_ACTIONS.has(pinned[1])) {
        assert.match(comment, /^\s+#\s*\S/, `${name}: ${pinned[1]} must explain its pin in a comment`);
      } else {
        assert.match(comment, /^\s+# v\d+(?:\.\d+)*\b/, `${name}: ${line.trim()} must name the release it resolves to (# vX.Y.Z)`);
      }
    }
    assert.ok(actions > 0, `${name} uses no actions; the pin check would be vacuous`);
  });

  test(`${name} grants write scopes only to reviewed publishing jobs`, () => {
    for (const job of jobs(text)) {
      for (const scope of writeScopes(jobPermissions(job))) {
        const key = `${name}:${job.id}:${scope}`;
        assert.ok(WRITERS.has(key), `${key} is not a reviewed writer; add it to WRITERS only if the job publishes`);
      }
    }
  });

  test(`${name} installs command-line tools at exact versions`, () => {
    for (const [command] of text.matchAll(/^.*\bcargo install\b.*$/gm)) {
      assert.match(command, /--locked\b/, `${name}: ${command.trim()} must use --locked`);
      assert.match(command, /--version \d+\.\d+\.\d+\b/, `${name}: ${command.trim()} must pin --version`);
    }
    for (const [command, args] of text.matchAll(/^.*\bnpm (?:install|i) (?:-g|--global)\b(.*)$/gm)) {
      const packages = args.trim().split(/\s+/).filter((arg) => arg && !arg.startsWith("-"));
      assert.ok(packages.length > 0, `${name}: ${command.trim()} installs nothing`);
      for (const spec of packages) {
        assert.match(spec, /^(?:@[^@/\s]+\/)?[^@\s]+@\d+\.\d+\.\d+$/, `${name}: ${spec} must be pinned to an exact version`);
      }
    }
  });
}

test("every reviewed writer still exists and still needs its scope", () => {
  for (const key of WRITERS.keys()) {
    const [name, id, scope] = key.split(":");
    assert.ok(workflows[name], `${key}: workflow is gone; drop it from WRITERS`);
    const job = jobs(workflows[name]).find((candidate) => candidate.id === id);
    assert.ok(job, `${key}: job is gone; drop it from WRITERS`);
    assert.ok(writeScopes(jobPermissions(job)).includes(scope), `${key}: job no longer writes; drop it from WRITERS`);
  }
});

test("SBOM generation runs read-only and only the release attach step writes", () => {
  const text = workflows["sbom.yml"];
  const byId = Object.fromEntries(jobs(text).map((job) => [job.id, job]));
  const sbom = byId.sbom?.lines.join("\n") ?? "";
  const attach = byId.attach?.lines.join("\n") ?? "";
  assert.deepEqual(writeScopes(jobPermissions(byId.sbom)), [], "the job that installs SBOM tools must not hold a write token");
  assert.equal(jobPermissions(byId.sbom)?.get("contents"), "read");
  assert.match(sbom, /cargo install --locked cargo-sbom --version \d+\.\d+\.\d+$/m);
  assert.match(sbom, /npm install -g @cyclonedx\/cdxgen@\d+\.\d+\.\d+$/m);
  assert.match(sbom, /name: sbom\s*\n\s+path: \|/);
  assert.doesNotMatch(sbom, /gh release/);

  assert.match(attach, /^    needs: sbom$/m);
  assert.match(attach, /^    if: github\.event_name == 'release'$/m);
  assert.deepEqual(writeScopes(jobPermissions(byId.attach)), ["contents"]);
  assert.match(attach, /uses: actions\/download-artifact@[0-9a-f]{40} # v\S+\s*\n\s+with:\s*\n\s+name: sbom/);
  assert.match(attach, /run: gh release upload "\$TAG_NAME" sbom-rust\.spdx\.json sbom-rust\.cdx\.json sbom-full\.cdx\.json$/m);
  assert.doesNotMatch(attach, /cargo |npm |npx |cdxgen|actions\/checkout/, "the writer must not install or run project tooling");
});
