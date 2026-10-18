// The structural guard behind #436 (a follow-up to #358): a `run:` block with several commands
// on a Windows runner must not hide a failing command.
//
// Windows steps run in PowerShell (`pwsh`) unless they name another shell. GitHub appends
// `exit $LASTEXITCODE` to a pwsh script, so a step reports only the exit code of the LAST native
// command: `npm run build` failing in front of a passing `npm run check` went green. Under
// `shell: bash` GitHub runs `bash --noprofile --norc -eo pipefail`, which stops at the first failure.
//
// A step that may run on a Windows runner passes when it
//   - is a single command (one logical, non-comment line), or
//   - names `shell: bash`, or
//   - is a `pwsh` script that STARTS with the two preference lines in PWSH_PREFERENCES (PowerShell
//     7.4 and later throw on any native command exiting non-zero) and does not switch either off later, or
//   - matches an exception the caller pinned, with its reason: an audited script whose every native command
//     is checked by hand and for which the preference would hide a diagnostic, or the `powershell` 5.1 step,
//     which has no such preference.
//
// Nothing here runs a script: workflows are read through scripts/lib/ci-workflow.mjs (parsed YAML,
// CRLF-safe). The scripts of a step are classified by line; there is no PowerShell parser.
import { createHash } from "node:crypto";

/** The first two command lines of a multi-command pwsh script, in either order. Both are needed: the native-exit preference only acts under the error preference. */
export const PWSH_PREFERENCES = [
  { name: "$ErrorActionPreference = 'Stop'", pattern: /^\$ErrorActionPreference\s*=\s*(['"])Stop\1\s*;?$/i },
  { name: "$PSNativeCommandUseErrorActionPreference = $true", pattern: /^\$PSNativeCommandUseErrorActionPreference\s*=\s*\$true\s*;?$/i },
];

// A line after the preference lines that may change either preference: any assignment to either variable (in any scope,
// with any value, braced or not) other than the passing line itself, or any variable cmdlet, alias or `variable:` drive
// path that names one. Reading a preference, or naming it in text, is no change.
const PREFERENCE_NAME = String.raw`(?:PSNativeCommandUseErrorActionPreference|ErrorActionPreference)`;
const PREFERENCE_ASSIGNMENT = new RegExp(String.raw`\$\{?(?:\w+:)?${PREFERENCE_NAME}\}?\s*(?:[-+*/%]|\?\?)?=(?!=)`, "i");
const PREFERENCE_VARIABLE_ACCESS = new RegExp(
  String.raw`(?:\b(?:Set|Remove|Clear|New)-Variable\b|\b(?:sv|rv|clv|si|ri)\b|\bvariable:)[^|]*\b${PREFERENCE_NAME}\b`,
  "i",
);
const changesPreference = (line) =>
  !PWSH_PREFERENCES.some((preference) => preference.pattern.test(line)) &&
  (PREFERENCE_ASSIGNMENT.test(line) || PREFERENCE_VARIABLE_ACCESS.test(line));

const POWERSHELL = new Set(["pwsh", "powershell"]);
const flat = (value) => (value === undefined || value === null ? [] : Array.isArray(value) ? value.flatMap(flat) : [value]);

/** The values `matrix.<key>` takes in a job, or null when the matrix cannot be read statically. */
function matrixValues(matrix, key) {
  if (matrix === null || typeof matrix !== "object") return null;
  const values = [...flat(matrix[key]), ...flat(matrix.include).map((entry) => entry?.[key])].filter((value) => value !== undefined);
  return values.length > 0 ? values : null;
}

/**
 * Every runner label a job may run on: `{ labels, resolved }`. `resolved` is false when `runs-on` holds an
 * expression that is not a plain `matrix.<key>` reference (an input, `fromJSON`, a ternary), so the labels
 * are incomplete.
 */
export function runnerLabels(job) {
  const labels = [];
  let resolved = true;
  const candidates =
    job.runsOn?.labels !== undefined || job.runsOn?.group !== undefined ? [job.runsOn.labels, job.runsOn.group] : [job.runsOn];
  for (const value of flat(candidates)) {
    if (typeof value !== "string") {
      resolved = false;
      continue;
    }
    if (!value.includes("${{")) {
      labels.push(value);
      continue;
    }
    const reference = /^\$\{\{\s*matrix\.([\w-]+)\s*\}\}$/.exec(value.trim());
    const values = reference ? matrixValues(job.strategy?.matrix, reference[1]) : null;
    if (values === null) resolved = false;
    for (const label of values ?? []) {
      if (typeof label === "string" && !label.includes("${{")) labels.push(label);
      else resolved = false;
    }
  }
  return { labels, resolved };
}

/** True when a job may run on a Windows runner. A `runs-on` that cannot be resolved counts as Windows: the guard fails closed. */
export function mayRunOnWindows(job) {
  const { labels, resolved } = runnerLabels(job);
  return !resolved || labels.some((label) => /windows/i.test(label));
}

/** `bash`, `pwsh`, `powershell`, `cmd`, ... or a custom template (kept verbatim). GitHub's default shell on Windows is pwsh. */
export const effectiveShell = (step, job, workflow) => String(step.shell ?? job?.defaultShell ?? workflow?.defaultShell ?? "pwsh").trim();

/**
 * The logical command lines of a script: continuation lines joined (a trailing backtick in PowerShell, a
 * backslash elsewhere), blank and comment lines dropped, `<# #>` blocks removed in PowerShell.
 */
export function scriptLines(run, shell) {
  const powershell = POWERSHELL.has(shell);
  let text = run;
  if (powershell) text = text.replace(/<#[\s\S]*?#>/g, "");
  const joined = text.replace(powershell ? /`[ \t]*\n[ \t]*/g : /\\\n[ \t]*/g, " ");
  const comment = shell === "cmd" ? /^(?:::|rem(?:\s|$))/i : /^#/;
  return joined
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "" && !comment.test(line));
}

// The line with every quoted string emptied, so a `;` inside a string does not separate commands. A backtick
// escapes the next character in a PowerShell double-quoted string; a doubled quote ends and restarts the string.
function withoutStrings(line) {
  let out = "";
  let quote = null;
  for (let index = 0; index < line.length; index++) {
    const char = line[index];
    if (quote === null) {
      if (char === "'" || char === '"') quote = char;
      else out += char;
    } else if (quote === '"' && char === "`") index++;
    else if (char === quote) quote = null;
  }
  return out;
}

/** How many commands a logical line holds: PowerShell and cmd lines are split on `;` and `&`, outside strings. */
function commandsOnLine(line, shell) {
  const separator = shell === "cmd" ? /&+|\|\|/ : POWERSHELL.has(shell) ? /;/ : null;
  if (separator === null) return 1;
  return Math.max(
    1,
    withoutStrings(line)
      .split(separator)
      .filter((part) => part.trim() !== "").length,
  );
}

/** The SHA-256 of a script as the guard sees it (its logical command lines joined by "\n"): what an exception can pin. */
export const scriptHash = (lines) => createHash("sha256").update(lines.join("\n")).digest("hex");

// A line shape matches one command per line, however loose its pattern: `audited.ps1 -A x; cargo test` is not the
// audited line. (A pipeline appended to the line is for the pattern to refuse: anchor it with `$`.)
const matchesException = (exception, shell, lines) =>
  exception.shell === shell &&
  (exception.sha256 === scriptHash(lines) ||
    (exception.lines !== undefined &&
      exception.lines.length === lines.length &&
      exception.lines.every((pattern, index) => pattern.test(lines[index]) && commandsOnLine(lines[index], shell) === 1)));

/**
 * True for a shell that stops at the first failing command: `bash` (GitHub runs `bash --noprofile --norc -eo pipefail
 * {0}`), `sh` (`sh -e {0}`), or a bash or sh template that turns errexit on (`-e`, `-eo pipefail`, `-o errexit`).
 */
export function stopsAtFirstFailure(shell) {
  if (shell === "bash" || shell === "sh") return true;
  const template = /^(?:bash|sh)\s+(.*)\{0\}\s*$/.exec(shell);
  if (template === null) return false;
  const words = template[1].trim().split(/\s+/);
  return words.some((word, index) => /^-[A-Za-z]*e[A-Za-z]*$/.test(word) || (word === "-o" && words[index + 1] === "errexit"));
}

// Interpreters whose scripts are code, not commands; the guard cannot judge how their statements fail.
const INTERPRETER = /^(?:python[\d.]*|py|node|perl|ruby)(?:\s|$)/i;

const EXCEPTION_HINT = "pin the audited script as an exception (WINDOWS_SHELL_EXCEPTIONS in scripts/test-ci-parallel-gates.mjs)";

/** True when a script's first two command lines are the two PWSH_PREFERENCES, in either order. */
const carriesPreferences = (lines) =>
  PWSH_PREFERENCES.every((preference) => lines.slice(0, PWSH_PREFERENCES.length).some((line) => preference.pattern.test(line)));

/**
 * Whether a step is acceptable. Returns `{ verdict, lines, commands, problem?, exception? }`: `verdict` names why
 * it passes ("single command", "bash", "pwsh preferences", "exception: <reason>") or is null with a `problem`.
 * `exceptions`: `{ shell, reason, lines?: [RegExp...], sha256? }`. `lines` matches the script line by line;
 * `sha256` (see scriptHash) pins the whole script, so any edit of an audited script needs a new audit.
 */
export function judgeStep(step, shell, exceptions = []) {
  const lines = scriptLines(step.run, shell);
  const commands = lines.reduce((count, line) => count + commandsOnLine(line, shell), 0);
  const judged = { lines, commands };
  if (commands <= 1) return { ...judged, verdict: "single command" };
  // The preference lines are PowerShell. A step that carries them must say `shell: pwsh` itself: left to the default,
  // a changed `defaults.run.shell` would hand them to another shell, and this guard would judge the step by that one.
  if (carriesPreferences(lines) && String(step.shell ?? "").trim() !== "pwsh") {
    return {
      ...judged,
      verdict: null,
      problem:
        "carries the PowerShell preference lines but does not name shell: pwsh itself, so a changed defaults.run.shell would move it to another shell",
    };
  }
  if (stopsAtFirstFailure(shell)) return { ...judged, verdict: "bash" };
  let problem;
  if (shell === "pwsh") {
    const missing = !carriesPreferences(lines);
    const weakened = lines.slice(2).find(changesPreference);
    if (!missing && weakened === undefined) return { ...judged, verdict: "pwsh preferences" };
    const why =
      weakened !== undefined
        ? `it changes a preference after the first two lines: ${weakened}`
        : `its first two command lines must be ${PWSH_PREFERENCES.map((preference) => preference.name).join(" and ")}`;
    problem = `runs ${commands} commands in pwsh, where only the last native command's exit code counts; ${why}, or use shell: bash`;
  } else {
    problem = INTERPRETER.test(shell)
      ? `runs ${commands} lines in ${shell}, which is an interpreter, not a shell the guard can judge: ${EXCEPTION_HINT}`
      : `runs ${commands} commands in ${shell}, which does not stop at the first failing command; use shell: bash, or ${EXCEPTION_HINT}`;
  }
  const exception = exceptions.find((candidate) => matchesException(candidate, shell, lines));
  if (exception) return { ...judged, verdict: `exception: ${exception.reason}`, exception };
  return { ...judged, verdict: null, problem: `${problem} (script sha256 ${scriptHash(lines)})` };
}

/**
 * Judges every `run:` step that may run on Windows: the steps of every job of every workflow whose runner
 * is or may be Windows, and every step of every local composite action (an action runs on its caller's
 * runner, so it counts as Windows). `workflows`: `[{ file, workflow }]` from parseWorkflow;
 * `actions`: `[{ name, action }]` from loadAction. Returns `{ rows, problems }`.
 */
export function windowsShellAudit({ workflows, actions = [], exceptions = [] }) {
  const rows = [];
  const used = new Set();
  const record = (where, step, shell, judged) => {
    if (judged.exception) used.add(judged.exception);
    rows.push({
      where,
      step: step.name ?? `step ${step.index + 1}`,
      shell,
      commands: judged.commands,
      verdict: judged.verdict,
      problem: judged.problem,
    });
  };
  for (const { file, workflow } of workflows) {
    for (const job of workflow.jobs) {
      if (!mayRunOnWindows(job)) continue;
      for (const step of job.steps.filter((candidate) => candidate.run !== "")) {
        const shell = effectiveShell(step, job, workflow);
        record(`${file} > ${job.id}`, step, shell, judgeStep(step, shell, exceptions));
      }
    }
  }
  for (const { name, action } of actions) {
    for (const step of action.steps.filter((candidate) => candidate.run !== "")) {
      if (step.shell === undefined) {
        rows.push({
          where: `action ${name}`,
          step: step.name ?? `step ${step.index + 1}`,
          shell: "(none)",
          commands: scriptLines(step.run, "bash").length,
          verdict: null,
          problem: "a composite action's run step must name its shell",
        });
        continue;
      }
      const shell = String(step.shell).trim();
      record(`action ${name}`, step, shell, judgeStep(step, shell, exceptions));
    }
  }
  const problems = rows.filter((row) => row.verdict === null).map((row) => `${row.where}: "${row.step}" ${row.problem}`);
  for (const exception of exceptions) {
    if (!used.has(exception))
      problems.push(
        `the exception for "${exception.reason}" matches no step any more: re-audit the step, then pin its new script (see the sha256 above) or remove the exception`,
      );
  }
  return { rows, problems };
}
