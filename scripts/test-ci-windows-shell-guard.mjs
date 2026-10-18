import assert from "node:assert/strict";
import test from "node:test";
import { parseAction, parseWorkflow } from "./lib/ci-workflow.mjs";
import { effectiveShell, mayRunOnWindows, runnerLabels, scriptHash, scriptLines, windowsShellAudit } from "./lib/windows-shell-guard.mjs";

// The guard of #436 itself, on small synthetic workflows: which jobs count as Windows jobs, which shell a step
// runs in, and which scripts pass. The real workflows are checked by scripts/test-ci-parallel-gates.mjs.

const PREFERENCES = "$ErrorActionPreference = 'Stop'\n$PSNativeCommandUseErrorActionPreference = $true\n";
const block = (script) =>
  script
    .trimEnd()
    .split("\n")
    .map((line) => `          ${line}`)
    .join("\n");

/** A workflow with one job and one `run:` step. */
function workflowWith({ runsOn = "windows-latest", shell, script, jobExtra = "", top = "" }) {
  return parseWorkflow(`
name: Synthetic
on: workflow_call
${top}
jobs:
  probe:
    runs-on: ${runsOn}
${jobExtra}
    steps:
      - name: Probe step
${shell === undefined ? "" : `        shell: ${shell}\n`}        run: |
${block(script)}
`);
}

const problemsOf = (workflow, options = {}) => windowsShellAudit({ workflows: [{ file: "synthetic.yml", workflow }], ...options }).problems;

test("runner labels: literals, lists, groups, matrix references and what cannot be resolved", () => {
  const job = (yaml) => parseWorkflow(`on: push\njobs:\n  j:\n${yaml}\n    steps: []`).job("j");
  assert.deepEqual(runnerLabels(job("    runs-on: windows-11-arm")), { labels: ["windows-11-arm"], resolved: true });
  assert.deepEqual(runnerLabels(job("    runs-on: [self-hosted, Windows, ARM64]")).labels, ["self-hosted", "Windows", "ARM64"]);
  assert.deepEqual(runnerLabels(job("    runs-on:\n      group: big\n      labels: windows-latest-8-cores")).labels, [
    "windows-latest-8-cores",
    "big",
  ]);
  const included = job(
    "    runs-on: ${{ matrix.runner }}\n    strategy:\n      matrix:\n        include:\n          - runner: macos-14\n          - runner: windows-latest",
  );
  assert.deepEqual(runnerLabels(included), { labels: ["macos-14", "windows-latest"], resolved: true });
  const listed = job("    runs-on: ${{ matrix.os }}\n    strategy:\n      matrix:\n        os: [ubuntu-24.04, windows-2022]");
  assert.deepEqual(runnerLabels(listed).labels, ["ubuntu-24.04", "windows-2022"]);
  // Anything else is not resolved, and an unresolved runner counts as Windows: the guard fails closed.
  for (const expression of [
    "${{ inputs.runner }}",
    "${{ matrix.missing }}",
    "${{ fromJSON(inputs.runners) }}",
    "${{ matrix.os == 'a' && 'b' || 'c' }}",
  ]) {
    const unresolved = job(`    runs-on: ${expression}\n    strategy:\n      matrix:\n        os: [ubuntu-24.04]`);
    assert.equal(runnerLabels(unresolved).resolved, false, expression);
    assert.ok(mayRunOnWindows(unresolved), expression);
  }
  for (const runner of ["ubuntu-24.04", "macos-14", "[self-hosted, linux]"])
    assert.ok(!mayRunOnWindows(job(`    runs-on: ${runner}`)), runner);
  assert.ok(
    !mayRunOnWindows(parseWorkflow("on: push\njobs:\n  call:\n    uses: ./.github/workflows/x.yml").job("call")),
    "a job that calls a reusable workflow has no runner of its own",
  );
});

test("a step's shell: its own, else the job default, else the workflow default, else pwsh as on every Windows runner", () => {
  const step = (shell) => ({ shell });
  assert.equal(effectiveShell(step(undefined), {}, {}), "pwsh");
  assert.equal(effectiveShell(step("bash"), { defaultShell: "pwsh" }, {}), "bash");
  assert.equal(effectiveShell(step(undefined), { defaultShell: "bash" }, { defaultShell: "cmd" }), "bash");
  assert.equal(effectiveShell(step(undefined), {}, { defaultShell: "cmd" }), "cmd");
  const workflow = workflowWith({
    shell: undefined,
    script: "npm run a\nnpm run b",
    jobExtra: "    defaults:\n      run:\n        shell: bash",
    top: "defaults:\n  run:\n    shell: pwsh",
  });
  assert.deepEqual(problemsOf(workflow), [], "the job default bash covers the step");
  const inherited = workflowWith({ shell: undefined, script: "npm run a\nnpm run b", top: "defaults:\n  run:\n    shell: bash" });
  assert.deepEqual(problemsOf(inherited), [], "the workflow default bash covers the step");
});

test("a multi-command pwsh step without the preferences fails, and so does every shell that does not stop at the first failure", () => {
  for (const shell of [undefined, "pwsh"]) {
    const problems = problemsOf(workflowWith({ shell, script: "npm run build\nnpm run check" }));
    assert.equal(problems.length, 1, `shell ${shell}`);
    assert.match(problems[0], /synthetic\.yml > probe: "Probe step" runs 2 commands in pwsh/);
    assert.match(problems[0], /script sha256 [0-9a-f]{64}/);
  }
  const multi = "npm run build\nnpm run check";
  // Shells that stop at the first failing command: bash and sh as GitHub runs them (-e), and an explicit template with -e.
  for (const shell of [
    "bash",
    "sh",
    "sh -e {0}",
    "bash -e {0}",
    "bash -eo pipefail {0}",
    "bash --noprofile --norc -eo pipefail {0}",
    "bash --noprofile --norc -e -o pipefail {0}",
    "bash -o errexit {0}",
  ]) {
    assert.deepEqual(problemsOf(workflowWith({ shell, script: multi })), [], `shell ${shell}`);
  }
  // Shells that do not, or whose template does not say so. The message offers bash or a pinned exception.
  for (const shell of ["powershell", "cmd", "bash {0}", "sh {0}", "bash -x {0}", "bash -o pipefail {0}", "pwsh -command \". '{0}'\""]) {
    const problems = problemsOf(workflowWith({ shell, script: multi }));
    assert.equal(problems.length, 1, `shell ${shell}`);
    assert.match(
      problems[0],
      /does not stop at the first failing command; use shell: bash, or pin the audited script as an exception/,
      shell,
    );
  }
  // An interpreter is not a shell the guard can judge: its message names only the exception mechanism.
  for (const shell of ["python", "python {0}", "python3 {0}", "node {0}"]) {
    const problems = problemsOf(workflowWith({ shell, script: multi }));
    assert.equal(problems.length, 1, `shell ${shell}`);
    assert.match(
      problems[0],
      /is an interpreter, not a shell the guard can judge: pin the audited script as an exception \(WINDOWS_SHELL_EXCEPTIONS in scripts\/test-ci-parallel-gates\.mjs\)/,
      shell,
    );
    assert.doesNotMatch(problems[0], /use shell: bash/, shell);
  }
  // Single commands still pass in an interpreter.
  assert.deepEqual(problemsOf(workflowWith({ shell: "python", script: "print('hi')" })), []);
});

test("single commands pass in any shell: continuations, comments, blank lines and strings do not make a command", () => {
  const single = (shell, script) => problemsOf(workflowWith({ shell, script }));
  assert.deepEqual(single("pwsh", "npm run build"), []);
  assert.deepEqual(single("pwsh", "# why\n\nnpm run build -- `\n  --mode production"), [], "a backtick continues a PowerShell line");
  assert.deepEqual(
    single("pwsh", '<# a block\ncomment #>\nWrite-Host "a; b"'),
    [],
    "a ; inside a string or a block comment is no separator",
  );
  assert.deepEqual(single("pwsh", "Write-Host 'it''s; fine'"), []);
  assert.deepEqual(single("powershell", "./scripts/accept.ps1 -Fixture a.mp3"), []);
  assert.deepEqual(single("cmd", "REM first\nnpm run build"), []);
  // A backslash is a continuation in bash and a path separator in PowerShell: it must not join two PowerShell commands.
  assert.deepEqual(scriptLines("Get-Item build\\\nnpm run build", "bash"), ["Get-Item build npm run build"]);
  assert.deepEqual(scriptLines("Get-Item build\\\nnpm run build", "pwsh"), ["Get-Item build\\", "npm run build"]);
  assert.equal(problemsOf(workflowWith({ shell: "pwsh", script: "Get-Item build\\\nnpm run build" })).length, 1);
});

test("commands joined on one line count: `a; b` in PowerShell and `a & b` in cmd", () => {
  assert.equal(problemsOf(workflowWith({ shell: "pwsh", script: "npm run build; npm run check" })).length, 1);
  assert.equal(problemsOf(workflowWith({ shell: "cmd", script: "npm run build & npm run check" })).length, 1);
  assert.equal(problemsOf(workflowWith({ shell: "cmd", script: "npm run build && npm run check" })).length, 1);
  assert.deepEqual(problemsOf(workflowWith({ shell: "pwsh", script: `${PREFERENCES}npm run build; npm run check` })), []);
});

test("the pwsh preferences must be the first two command lines and must stay on", () => {
  const ok = (script) => problemsOf(workflowWith({ shell: "pwsh", script }));
  const body = "npm run build\nnpm run check";
  assert.deepEqual(ok(`${PREFERENCES}${body}`), []);
  assert.deepEqual(ok(`# why\n${PREFERENCES}# more\n${body}`), [], "comments around them are fine");
  assert.deepEqual(
    ok(`$PSNativeCommandUseErrorActionPreference = $true\n$ErrorActionPreference = "Stop"\n${body}`),
    [],
    "either order, either quotes",
  );
  assert.equal(ok(`$ErrorActionPreference = 'Stop'\n${body}`).length, 1, "the native-exit preference is needed");
  assert.equal(ok(`$PSNativeCommandUseErrorActionPreference = $true\n${body}`).length, 1, "so is the error preference it acts through");
  assert.equal(ok(`Write-Host start\n${PREFERENCES}${body}`).length, 1, "they must come first");
  assert.equal(ok(`$ErrorActionPreference = 'Continue'\n$PSNativeCommandUseErrorActionPreference = $true\n${body}`).length, 1);
  assert.equal(ok(`$ErrorActionPreference = 'Stop'\n$PSNativeCommandUseErrorActionPreference = $false\n${body}`).length, 1);
  assert.match(
    ok(`${PREFERENCES}npm run build\n$global:ErrorActionPreference = 'SilentlyContinue'\nnpm run check`)[0],
    /changes a preference after the first two lines/,
  );
  assert.match(
    ok(`${PREFERENCES}npm run build\n$PSNativeCommandUseErrorActionPreference = $false\nnpm run check`)[0],
    /changes a preference after the first two lines/,
  );
  // -ErrorAction on one cmdlet is not a preference.
  assert.deepEqual(ok(`${PREFERENCES}Remove-Item x -ErrorAction SilentlyContinue\nnpm run check`), []);
  // Any later change of either preference fails, whatever it assigns or how it reaches the variable.
  for (const change of [
    "$PSNativeCommandUseErrorActionPreference = $null",
    "$PSNativeCommandUseErrorActionPreference=0",
    "$ErrorActionPreference = $saved",
    "$ErrorActionPreference = [System.Management.Automation.ActionPreference]::Continue",
    "$ErrorActionPreference = [ActionPreference]::Continue",
    "$script:ErrorActionPreference = 'Ignore'",
    "${ErrorActionPreference} = 'Continue'",
    "$ErrorActionPreference += 'x'",
    "Set-Variable -Name ErrorActionPreference -Value Continue",
    "Set-Variable ErrorActionPreference Continue -Scope Global",
    "sv PSNativeCommandUseErrorActionPreference $false",
    "Remove-Variable PSNativeCommandUseErrorActionPreference",
    "Clear-Variable -Name ErrorActionPreference",
    "Remove-Item variable:ErrorActionPreference",
    "& { $ErrorActionPreference = 'Continue'; cargo test }",
    // An assignment inside a $( ) subexpression of a string still runs.
    "Write-Host \"$($ErrorActionPreference = 'Continue')\"",
    "Set-Variable -Name 'ErrorActionPreference' -Value Continue",
    'Set-Variable -Name "PSNativeCommandUseErrorActionPreference" -Value $false',
  ]) {
    const problems = ok(`${PREFERENCES}npm run build\n${change}\nnpm run check`);
    assert.equal(problems.length, 1, change);
    assert.match(problems[0], /changes a preference after the first two lines/, change);
  }
  // Re-asserting the passing values, reading them, or naming them in text is not a change.
  for (const harmless of [
    "$ErrorActionPreference = 'Stop'",
    "$PSNativeCommandUseErrorActionPreference = $true",
    "Write-Host $ErrorActionPreference",
    "if ($ErrorActionPreference -ne 'Stop') { throw 'preference lost' }",
    "Get-Variable PSNativeCommandUseErrorActionPreference",
    'Write-Host "ErrorActionPreference stays Stop"',
    // Text in a string is not an assignment, even when it reads like one.
    'Write-Host "x `$ErrorActionPreference=$ErrorActionPreference"',
    "Write-Host 'checking $ErrorActionPreference = Stop'",
    'Write-Host "checking $ErrorActionPreference = Stop"',
  ]) {
    assert.deepEqual(ok(`${PREFERENCES}npm run build\n${harmless}\nnpm run check`), [], harmless);
  }
});

test("a step with the preference lines names shell: pwsh itself, so a changed default cannot move it", () => {
  const body = `${PREFERENCES}npm run build\nnpm run check`;
  assert.deepEqual(problemsOf(workflowWith({ shell: "pwsh", script: body })), []);
  // The lines are PowerShell. Left to the default, a later `defaults.run.shell: bash` would hand them to Git Bash,
  // which the guard would then wave through as bash.
  for (const [what, workflow] of [
    ["no shell on a Windows runner", workflowWith({ shell: undefined, script: body })],
    ["a job default of bash", workflowWith({ shell: undefined, script: body, jobExtra: "    defaults:\n      run:\n        shell: bash" })],
    ["a workflow default of bash", workflowWith({ shell: undefined, script: body, top: "defaults:\n  run:\n    shell: bash" })],
    ["Windows PowerShell 5.1, which ignores the native-exit preference", workflowWith({ shell: "powershell", script: body })],
    ["a step that says bash", workflowWith({ shell: "bash", script: body })],
  ]) {
    assert.match(problemsOf(workflow).join("\n"), /carries the PowerShell preference lines but does not name shell: pwsh itself/, what);
  }
  // Composite actions always name their shell, so the same rule reads their step.
  const action = (shell) => ({
    name: "setup",
    action: parseAction(`name: setup\nruns:\n  using: composite\n  steps:\n    - shell: ${shell}\n      run: |\n${block(body)}`),
  });
  assert.deepEqual(windowsShellAudit({ workflows: [], actions: [action("pwsh")] }).problems, []);
  assert.equal(windowsShellAudit({ workflows: [], actions: [action("powershell")] }).problems.length, 1);
});

test("exceptions are pinned to the audited script, are reported when stale, and never excuse bash-less cmd", () => {
  const script = "./scripts/accept.ps1 -Fixture a.mp3\nif (-not (Test-Path 'out.json')) { throw 'missing' }";
  const workflow = workflowWith({ shell: "powershell", script });
  const byLines = {
    shell: "powershell",
    reason: "5.1 acceptance",
    lines: [/^\.\/scripts\/accept\.ps1 -Fixture \S+$/, /^if \(-not \(Test-Path '[^']+'\)\) \{ throw '[^']+' \}$/],
  };
  assert.deepEqual(problemsOf(workflow, { exceptions: [byLines] }), []);
  assert.deepEqual(
    problemsOf(workflow, { exceptions: [{ ...byLines, shell: "pwsh" }] }).length,
    2,
    "the wrong shell neither excuses the step nor counts as used",
  );
  // A third line, or another command, is no longer the audited script.
  const edited = workflowWith({ shell: "powershell", script: `${script}\nnpm run check` });
  assert.match(problemsOf(edited, { exceptions: [byLines] }).join("\n"), /runs 3 commands in powershell[\s\S]*matches no step any more/);
  // A line shape must match one command: a second command chained onto the audited line is not the audited script,
  // even when the shape has no end anchor, and an anchored shape rejects a pipeline appended to the line.
  const chained = workflowWith({ shell: "powershell", script: script.replace("a.mp3", "a.mp3; npm run check") });
  const unanchored = { ...byLines, lines: [/^\.\/scripts\/accept\.ps1 /, byLines.lines[1]] };
  assert.deepEqual(problemsOf(workflow, { exceptions: [unanchored] }), [], "the unanchored shape still matches the audited script");
  assert.match(
    problemsOf(chained, { exceptions: [unanchored] }).join("\n"),
    /runs 3 commands in powershell[\s\S]*matches no step any more/,
  );
  assert.match(problemsOf(chained, { exceptions: [byLines] }).join("\n"), /runs 3 commands in powershell/);
  const piped = workflowWith({ shell: "powershell", script: script.replace("a.mp3", "a.mp3 | Out-Null") });
  assert.match(problemsOf(piped, { exceptions: [byLines] }).join("\n"), /runs 2 commands in powershell/);
  // A pin by hash covers any shell, and any edit of the script breaks it.
  const pwsh = workflowWith({ shell: "pwsh", script: "npm run build\nif ($LASTEXITCODE -ne 0) { throw 'build' }\nnpm run check" });
  const hash = scriptHash(scriptLines("npm run build\nif ($LASTEXITCODE -ne 0) { throw 'build' }\nnpm run check", "pwsh"));
  assert.deepEqual(problemsOf(pwsh, { exceptions: [{ shell: "pwsh", sha256: hash, reason: "explicit checks" }] }), []);
  const reworded = workflowWith({ shell: "pwsh", script: "npm run build\nif ($LASTEXITCODE -ne 0) { throw 'broken' }\nnpm run check" });
  assert.equal(problemsOf(reworded, { exceptions: [{ shell: "pwsh", sha256: hash, reason: "explicit checks" }] }).length, 2);
  // Comments and indentation are not part of the pinned script.
  const commented = workflowWith({
    shell: "pwsh",
    script: "# audited\nnpm run build\n   if ($LASTEXITCODE -ne 0) { throw 'build' }\n\nnpm run check",
  });
  assert.deepEqual(problemsOf(commented, { exceptions: [{ shell: "pwsh", sha256: hash, reason: "explicit checks" }] }), []);
});

test("only Windows jobs are judged, in every workflow, reusable ones included", () => {
  const multi = "npm run build\nnpm run check";
  assert.deepEqual(problemsOf(workflowWith({ runsOn: "ubuntu-24.04", shell: "pwsh", script: multi })), []);
  assert.deepEqual(problemsOf(workflowWith({ runsOn: "macos-14", shell: undefined, script: multi })), []);
  assert.equal(problemsOf(workflowWith({ runsOn: "windows-2022", shell: undefined, script: multi })).length, 1);
  assert.equal(problemsOf(workflowWith({ runsOn: "windows-11-arm", shell: undefined, script: multi })).length, 1);
  // A reusable workflow (workflow_call) whose runner comes from its caller cannot be resolved, so it is judged as Windows.
  const reusable = workflowWith({ runsOn: "${{ inputs.runner }}", shell: undefined, script: multi });
  assert.equal(problemsOf(reusable).length, 1);
  // The same job on a matrix that includes Windows.
  const matrix = workflowWith({
    runsOn: "${{ matrix.runner }}",
    shell: undefined,
    script: multi,
    jobExtra: "    strategy:\n      matrix:\n        include:\n          - runner: ubuntu-24.04\n          - runner: windows-11-arm",
  });
  assert.equal(problemsOf(matrix).length, 1);
  // Several workflows are all judged, and each problem names its file.
  const { problems } = windowsShellAudit({
    workflows: [
      { file: "a.yml", workflow: workflowWith({ runsOn: "windows-latest", shell: "bash", script: multi }) },
      { file: "b.yml", workflow: workflowWith({ runsOn: "windows-latest", shell: undefined, script: multi }) },
    ],
  });
  assert.equal(problems.length, 1);
  assert.match(problems[0], /^b\.yml > probe: /);
});

test("local composite actions count as Windows steps and must name their shell", () => {
  const action = (steps) => ({ name: "setup", action: parseAction(`name: setup\nruns:\n  using: composite\n  steps:\n${steps}`) });
  const audit = (entry) => windowsShellAudit({ workflows: [], actions: [entry] }).problems;
  assert.deepEqual(audit(action("    - shell: bash\n      run: |\n        npm ci\n        npm run build")), []);
  assert.equal(audit(action("    - shell: pwsh\n      run: |\n        npm ci\n        npm run build")).length, 1);
  assert.match(audit(action("    - run: |\n        npm ci\n        npm run build"))[0], /action setup: .* must name its shell/);
  assert.deepEqual(audit(action("    - uses: actions/checkout@v4")), [], "a step without run is not judged");
});

test("CRLF checkouts judge the same as LF", () => {
  const lf =
    "name: x\non: push\njobs:\n  j:\n    runs-on: windows-latest\n    steps:\n      - run: |\n          npm run build\n          npm run check\n";
  assert.equal(
    windowsShellAudit({ workflows: [{ file: "x.yml", workflow: parseWorkflow(lf.replaceAll("\n", "\r\n")) }] }).problems.length,
    1,
  );
  const bash = lf.replace("      - run:", "      - shell: bash\n        run:");
  assert.deepEqual(
    windowsShellAudit({ workflows: [{ file: "x.yml", workflow: parseWorkflow(bash.replaceAll("\n", "\r\n")) }] }).problems,
    [],
  );
});
