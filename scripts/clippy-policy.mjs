// Clippy policy lints with a measured, non-increasing baseline (#356).
//
// CI's clippy gates run with `-D warnings`, so these lints cannot simply be
// switched on as `warn` in [workspace.lints]: the existing findings would fail
// every build and flood every editor. Instead this script runs a separate
// clippy pass with only the policy lints enabled, counts the findings per lint
// and crate, and fails when any count rises above the checked-in baseline.
//
//   node scripts/clippy-policy.mjs                           check (CI)
//   node scripts/clippy-policy.mjs --update                  lower the baseline after a cleanup
//   node scripts/clippy-policy.mjs --update --allow-increase raise it deliberately (say why in the PR)
//
// What counts: production code, i.e. the lib and bin targets of every member
// of the src-tauri workspace. Two configurations are measured: the whole
// workspace (features unified as in the GUI build) and the batch-only CLI
// (`-p sagascript-cli --no-default-features`), which compiles the
// `cfg(not(feature = ...))` paths the workspace build leaves out. A finding is
// one primary source span per lint and crate, so a span reported by both runs
// counts once. Tests, examples, benches and build scripts are not counted, nor
// is test tooling that cargo builds as a bin from tests/ (sagascript-core's
// fake engine host). `--all-targets` is deliberately not passed, because
// cargo's JSON does not tell a lib compiled for its unit tests from the lib
// itself. Only the platform the script runs on is measured (Linux in CI);
// code behind `cfg(target_os = "macos")` or `cfg(target_os = "windows")` is
// not.
import { spawnSync } from "node:child_process";
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = fileURLToPath(new URL("..", import.meta.url));
export const WORKSPACE_DIR = resolve(REPO_ROOT, "src-tauri");
export const DEFAULT_BASELINE = resolve(WORKSPACE_DIR, "clippy-policy-baseline.json");

// The policy. print_stdout/print_stderr will be allowed on the CLI output
// layer (#364); until then the CLI's user-facing output is counted too.
export const POLICY_LINTS = [
  "unwrap_used",
  "expect_used",
  "panic",
  "await_holding_lock",
  "too_many_lines",
  "cognitive_complexity",
  "dbg_macro",
  "print_stdout",
  "print_stderr",
];

export const RUNS = [
  { name: "workspace", args: ["--workspace"] },
  { name: "batch-only CLI", args: ["-p", "sagascript-cli", "--no-default-features"] },
];

// `-A clippy::all` silences the default clippy groups so only policy lints
// are reported; rustc's own warnings are ignored by the parser below.
export function clippyArgs(run) {
  return [
    "clippy",
    ...run.args,
    "--message-format=json",
    "--",
    "-A",
    "clippy::all",
    ...POLICY_LINTS.flatMap((lint) => ["-W", `clippy::${lint}`]),
  ];
}

const POLICY_CODES = new Map(POLICY_LINTS.map((lint) => [`clippy::${lint}`, lint]));
const PRODUCTION_KINDS = new Set(["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro", "bin"]);

// The package name in a cargo package ID: `path+file:///…/dir#name@1.0`,
// `path+file:///…/name#1.0` (name omitted when it equals the directory) or
// the pre-1.77 `name 1.0 (path+file:///…)`.
export function packageName(packageId) {
  const legacy = /^(\S+) \S+ \(/.exec(packageId);
  if (legacy) return legacy[1];
  const hash = packageId.lastIndexOf("#");
  if (hash < 0) throw new Error(`unrecognised package id: ${packageId}`);
  const fragment = packageId.slice(hash + 1);
  const at = fragment.lastIndexOf("@");
  if (at > 0) return fragment.slice(0, at);
  const path = new URL(packageId.slice(0, hash).replace(/^[a-z]+\+/, "")).pathname;
  return decodeURIComponent(path.split("/").filter(Boolean).at(-1));
}

const isLocalPackage = (packageId) => /^path\+file:\/\//.test(packageId) || / \(path\+file:\/\//.test(packageId);

// A lib or bin whose source lives under the package's tests/, examples/ or
// benches/ directory is test tooling, not shipped code.
const TOOLING_DIRS = new Set(["tests", "examples", "benches"]);
function isTestTooling(message) {
  const source = message.target?.src_path;
  if (!source || !message.manifest_path) return false;
  return TOOLING_DIRS.has(relative(dirname(message.manifest_path), source).split(/[\\/]/)[0]);
}

// Parses `cargo --message-format=json` output into policy findings and the
// names of the local (workspace) packages that were compiled.
const LINT_NAME_CODES = new Set(["renamed_and_removed_lints", "unknown_lints"]);

export function parseCargoMessages(text) {
  const findings = [];
  const crates = new Set();
  // rustc's notes on the `-W clippy::<lint>` flags themselves. A policy lint
  // that a newer clippy renamed or removed would otherwise fire under a code
  // this script does not count and read as a drop to zero.
  const lintNameProblems = new Set();
  for (const line of text.split("\n")) {
    if (!line.startsWith("{")) continue;
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      continue;
    }
    if (!message.package_id || !isLocalPackage(message.package_id)) continue;
    const crate = packageName(message.package_id);
    if (
      message.reason === "compiler-message" &&
      LINT_NAME_CODES.has(message.message?.code?.code) &&
      /clippy::/.test(message.message?.message ?? "")
    ) {
      lintNameProblems.add(message.message.message);
    }
    const kinds = message.target?.kind ?? [];
    if (!kinds.some((kind) => PRODUCTION_KINDS.has(kind)) || isTestTooling(message)) continue;
    if (message.reason === "compiler-artifact") crates.add(crate);
    if (message.reason !== "compiler-message") continue;
    const diagnostic = message.message ?? {};
    const lint = POLICY_CODES.get(diagnostic.code?.code);
    if (!lint || !["warning", "error"].includes(diagnostic.level)) continue;
    const span = (diagnostic.spans ?? []).find((candidate) => candidate.is_primary) ?? diagnostic.spans?.[0];
    const file = span?.file_name ?? "<unknown>";
    const location = span ? `${span.line_start}:${span.column_start}-${span.line_end}:${span.column_end}` : "?";
    crates.add(crate);
    findings.push({
      lint,
      crate,
      file,
      line: span?.line_start ?? 0,
      column: span?.column_start ?? 0,
      key: [crate, lint, file, location].join("\u0000"),
    });
  }
  return { findings, crates, lintNameProblems: [...lintNameProblems] };
}

// One entry per distinct (crate, lint, span), in a stable order.
export function dedupe(findings) {
  const unique = new Map();
  for (const finding of findings) if (!unique.has(finding.key)) unique.set(finding.key, finding);
  return [...unique.values()].sort(
    (a, b) => a.file.localeCompare(b.file) || a.line - b.line || a.column - b.column || a.lint.localeCompare(b.lint),
  );
}

// { lint: { crate: count } } with every policy lint and every given crate.
export function countFindings(findings, crates = []) {
  const counts = Object.fromEntries(POLICY_LINTS.map((lint) => [lint, {}]));
  const allCrates = new Set(crates);
  for (const finding of findings) allCrates.add(finding.crate);
  for (const lint of POLICY_LINTS) for (const crate of allCrates) counts[lint][crate] = 0;
  for (const finding of dedupe(findings)) counts[finding.lint][finding.crate] += 1;
  return sortCounts(counts);
}

function sortCounts(counts) {
  return Object.fromEntries(
    Object.entries(counts).map(([lint, perCrate]) => [
      lint,
      Object.fromEntries(Object.entries(perCrate).sort(([a], [b]) => a.localeCompare(b))),
    ]),
  );
}

// Every (lint, crate) cell present in either side; a missing cell is 0.
export function compareCounts(baseline, current) {
  const lints = [...new Set([...POLICY_LINTS, ...Object.keys(baseline), ...Object.keys(current)])];
  const rows = [];
  for (const lint of lints) {
    const crates = [...new Set([...Object.keys(baseline[lint] ?? {}), ...Object.keys(current[lint] ?? {})])].sort();
    for (const crate of crates) {
      const before = baseline[lint]?.[crate] ?? 0;
      const now = current[lint]?.[crate] ?? 0;
      rows.push({ lint, crate, baseline: before, now, delta: now - before });
    }
  }
  return {
    rows,
    increased: rows.filter((row) => row.delta > 0),
    decreased: rows.filter((row) => row.delta < 0),
    reachedZero: rows.filter((row) => row.baseline > 0 && row.now === 0),
  };
}

export function readBaseline(path) {
  const data = JSON.parse(readFileSync(path, "utf8"));
  if (!data || typeof data.counts !== "object") throw new Error(`${path}: missing "counts"`);
  for (const [lint, perCrate] of Object.entries(data.counts)) {
    for (const [crate, count] of Object.entries(perCrate)) {
      if (!Number.isInteger(count) || count < 0) throw new Error(`${path}: ${lint}/${crate} must be a count`);
    }
  }
  return data.counts;
}

export function serializeBaseline(counts) {
  const baseline = {
    description:
      "Clippy policy-lint findings in production code, per lint and crate (#356). " +
      "CI fails when a count rises. Regenerate with `node scripts/clippy-policy.mjs --update` after lowering a count; " +
      "see CONTRIBUTING.md.",
    counts: sortCounts(Object.fromEntries(POLICY_LINTS.map((lint) => [lint, counts[lint] ?? {}]))),
  };
  return `${JSON.stringify(baseline, null, 2)}\n`;
}

// Baseline crates this measurement did not compile. Their counts would read
// as zero, so a broken run could pass the check or wipe the baseline.
export function unmeasuredCrates(baseline, crates) {
  const known = new Set(Object.values(baseline).flatMap((perCrate) => Object.keys(perCrate)));
  return [...known].filter((crate) => !crates.has(crate)).sort();
}

// --update: the new baseline is the current counts, but a count may only go
// up with an explicit --allow-increase.
export function planUpdate(baseline, current, { allowIncrease = false } = {}) {
  const { increased } = compareCounts(baseline, current);
  if (increased.length > 0 && !allowIncrease) return { ok: false, increased };
  return { ok: true, increased, text: serializeBaseline(current) };
}

function pad(value, width, right = false) {
  const text = String(value);
  return right ? text.padStart(width) : text.padEnd(width);
}

export function renderTable(rows) {
  const shown = rows.filter((row) => row.baseline > 0 || row.now > 0);
  const total = shown.reduce((sum, row) => ({ baseline: sum.baseline + row.baseline, now: sum.now + row.now }), { baseline: 0, now: 0 });
  const header = ["lint", "crate", "baseline", "now", "delta"];
  const body = shown.map((row) => [row.lint, row.crate, row.baseline, row.now, formatDelta(row.delta)]);
  body.push(["total", "", total.baseline, total.now, formatDelta(total.now - total.baseline)]);
  const widths = header.map((title, index) => Math.max(title.length, ...body.map((cells) => String(cells[index]).length)));
  const line = (cells) =>
    cells
      .map((cell, index) => pad(cell, widths[index], index >= 2))
      .join("  ")
      .trimEnd();
  return [line(header), line(widths.map((width) => "-".repeat(width))), ...body.map(line)].join("\n");
}

export function renderMarkdown(rows) {
  const crates = [...new Set(rows.map((row) => row.crate))].sort();
  const lints = [...new Set(rows.map((row) => row.lint))];
  const cell = (lint, crate) => {
    const row = rows.find((candidate) => candidate.lint === lint && candidate.crate === crate);
    if (!row) return "0";
    return row.delta === 0 ? String(row.now) : `${row.now} (${formatDelta(row.delta)})`;
  };
  return [
    `| lint | ${crates.join(" | ")} |`,
    `|---|${crates.map(() => "---:").join("|")}|`,
    ...lints.map((lint) => `| \`${lint}\` | ${crates.map((crate) => cell(lint, crate)).join(" | ")} |`),
  ].join("\n");
}

const formatDelta = (delta) => (delta > 0 ? `+${delta}` : String(delta));

export function parseOptions(args) {
  const options = { update: false, allowIncrease: false, baseline: DEFAULT_BASELINE, inputs: [], help: false };
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    const value = () => {
      const next = args[index + 1];
      if (next === undefined || next.startsWith("--")) throw new Error(`${arg} needs a value`);
      index += 1;
      return next;
    };
    if (arg === "--update") options.update = true;
    else if (arg === "--allow-increase") options.allowIncrease = true;
    else if (arg === "--baseline") options.baseline = resolve(value());
    else if (arg === "--input") options.inputs.push(resolve(value()));
    else if (arg === "--help" || arg === "-h") options.help = true;
    else throw new Error(`unknown argument: ${arg}`);
  }
  if (options.allowIncrease && !options.update) throw new Error("--allow-increase only applies with --update");
  return options;
}

const USAGE = `Usage: node scripts/clippy-policy.mjs [--update [--allow-increase]] [--baseline FILE] [--input FILE ...]

Runs clippy with only the policy lints (${POLICY_LINTS.join(", ")}),
counts findings per lint and crate, and fails if a count is above the baseline.

  --update          rewrite the baseline with the current counts (refuses increases)
  --allow-increase  with --update: also accept higher counts (justify it in the PR)
  --baseline FILE   baseline to compare against (default: src-tauri/clippy-policy-baseline.json)
  --input FILE      read cargo --message-format=json output from FILE instead of running cargo`;

function runClippy({ log, error }) {
  let text = "";
  for (const run of RUNS) {
    const args = clippyArgs(run);
    log(`$ (cd src-tauri && cargo ${args.join(" ")})`);
    const result = spawnSync("cargo", args, {
      cwd: WORKSPACE_DIR,
      encoding: "utf8",
      maxBuffer: 1 << 30,
      stdio: ["ignore", "pipe", "inherit"],
    });
    if (result.error) throw result.error;
    if (result.status !== 0) {
      for (const line of result.stdout.split("\n")) {
        if (!line.startsWith("{")) continue;
        try {
          const message = JSON.parse(line);
          if (message.reason === "compiler-message" && message.message?.level === "error") {
            error(message.message.rendered ?? message.message.message);
          }
        } catch {
          // not a JSON message
        }
      }
      const failure = new Error(`cargo clippy (${run.name}) failed with exit status ${result.status}`);
      failure.exitCode = 2;
      throw failure;
    }
    text += result.stdout;
  }
  return text;
}

export function main(args = process.argv.slice(2), { log = console.log, error = console.error } = {}) {
  let options;
  try {
    options = parseOptions(args);
  } catch (problem) {
    error(`clippy-policy: ${problem.message}\n\n${USAGE}`);
    return 2;
  }
  if (options.help) {
    log(USAGE);
    return 0;
  }

  let text;
  try {
    text = options.inputs.length > 0 ? options.inputs.map((input) => readFileSync(input, "utf8")).join("\n") : runClippy({ log, error });
  } catch (problem) {
    error(`clippy-policy: ${problem.message}`);
    return problem.exitCode ?? 2;
  }
  const { findings, crates, lintNameProblems } = parseCargoMessages(text);
  if (lintNameProblems.length > 0) {
    error(
      "clippy-policy: clippy does not know a policy lint by this name; update POLICY_LINTS, the baseline " +
        "and engine-protocol's deny list together instead of reading it as a drop to zero:\n" +
        lintNameProblems.map((problem) => `  ${problem}`).join("\n"),
    );
    return 2;
  }
  if (crates.size === 0) {
    error("clippy-policy: no workspace crate was compiled; refusing to report an empty measurement");
    return 2;
  }
  const current = countFindings(findings, crates);
  const baselinePath = relative(process.cwd(), options.baseline) || options.baseline;

  let baseline = {};
  if (existsSync(options.baseline)) {
    try {
      baseline = readBaseline(options.baseline);
    } catch (problem) {
      error(`clippy-policy: ${problem.message}`);
      return 2;
    }
  } else if (!options.update) {
    error(`clippy-policy: ${baselinePath} does not exist; create it with --update --allow-increase`);
    return 2;
  }
  const unmeasured = unmeasuredCrates(baseline, crates);
  if (unmeasured.length > 0) {
    error(
      `clippy-policy: ${baselinePath} lists crates this measurement did not compile: ${unmeasured.join(", ")}. ` +
        "If a crate was removed or renamed, delete its entries from the baseline by hand.",
    );
    return 2;
  }

  const comparison = compareCounts(baseline, current);
  const source = options.inputs.length > 0 ? "--input" : RUNS.map((run) => run.name).join(" + ");
  log(`\nClippy policy lints in production code (${source}), vs ${baselinePath}:\n`);
  log(renderTable(comparison.rows));
  const clean = [...crates].filter((crate) => POLICY_LINTS.every((lint) => current[lint][crate] === 0)).sort();
  if (clean.length > 0) log(`\nAt zero for every policy lint: ${clean.join(", ")}`);
  if (process.env.GITHUB_STEP_SUMMARY && !options.update) {
    appendFileSync(
      process.env.GITHUB_STEP_SUMMARY,
      `### Clippy policy lints (production code)\n\nNow (change vs baseline):\n\n${renderMarkdown(comparison.rows)}\n\n`,
    );
  }

  if (options.update) {
    const plan = planUpdate(baseline, current, { allowIncrease: options.allowIncrease });
    if (!plan.ok) {
      error(`\nRefusing to raise the baseline without --allow-increase:`);
      for (const row of plan.increased) error(`  ${row.lint} in ${row.crate}: ${row.baseline} -> ${row.now}`);
      return 1;
    }
    writeFileSync(options.baseline, plan.text);
    log(`\nWrote ${baselinePath}.`);
    if (plan.increased.length > 0) log("It raises counts: say why in the pull request.");
    return 0;
  }

  if (comparison.increased.length > 0) {
    error("\nClippy policy counts went up. New code must not add these findings:");
    const unique = dedupe(findings);
    for (const row of comparison.increased) {
      error(`\n  ${row.lint} in ${row.crate}: ${row.baseline} -> ${row.now} (+${row.delta}). Current locations:`);
      const located = unique.filter((finding) => finding.crate === row.crate && finding.lint === row.lint);
      for (const finding of located.slice(0, 50)) error(`    src-tauri/${finding.file}:${finding.line}:${finding.column}`);
      if (located.length > 50) error(`    … and ${located.length - 50} more`);
    }
    error(
      '\nFix the new findings (CONTRIBUTING.md, "Clippy policy lints"). If an increase is truly intended, ' +
        "run `node scripts/clippy-policy.mjs --update --allow-increase` and justify it in the pull request.",
    );
    return 1;
  }

  if (comparison.decreased.length > 0) {
    log("\nCounts went down. Lock in the improvement: node scripts/clippy-policy.mjs --update, then commit the baseline.");
    for (const row of comparison.reachedZero) {
      log(
        `  ${row.crate} is now at zero for ${row.lint}: consider #![deny(clippy::${row.lint})] in its crate roots ` +
          "(only Linux is measured here; the macOS and Windows clippy gates confirm it).",
      );
    }
  } else {
    log("\nNo policy count went up.");
  }
  return 0;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = main();
}
