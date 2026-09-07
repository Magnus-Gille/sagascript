import assert from "node:assert/strict";
import { chmodSync, existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const verifierPath = join(scriptDirectory, "verify-windows-release.ps1");
const expectedVersion = "1.1.3";
const expectedGitHash = "0123456789abcdef0123456789abcdef01234567";

function findPowerShell(probeCwd) {
  const candidates = [
    process.env.SAGASCRIPT_PWSH,
    join(homedir(), ".dotnet", "tools", "pwsh"),
    "pwsh",
  ].filter(Boolean);

  for (const candidate of candidates) {
    const probe = spawnSync(
      candidate,
      ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "$PSVersionTable.PSVersion.ToString()"],
      { cwd: probeCwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
    );
    if (probe.status === 0) {
      return candidate;
    }
  }
  return null;
}

function writePowerShellFixtures(root) {
  const cliPath = join(root, "stub-cli.ps1");
  const runnerPath = join(root, "run-verifier.ps1");

  writeFileSync(
    cliPath,
    [
      "#!/usr/bin/env pwsh",
      "param([string]$Argument)",
      "if ($Argument -eq \"--version\") {",
      "    Write-Output $env:SAGASCRIPT_TEST_VERSION_OUTPUT",
      "    exit 0",
      "}",
      "if ($Argument -eq \"--help\") {",
      "    Write-Output \"help\"",
      "    exit 0",
      "}",
      "exit 17",
      "",
    ].join("\n"),
  );
  chmodSync(cliPath, 0o755);

  writeFileSync(
    runnerPath,
    [
      "param(",
      "    [Parameter(Mandatory = $true)][string]$VerifierPath,",
      "    [Parameter(Mandatory = $true)][string]$CliPath,",
      "    [Parameter(Mandatory = $true)][string]$ChecksumPath,",
      "    [Parameter(Mandatory = $true)][string]$ExpectedVersion,",
      "    [string]$ExpectedGitHash,",
      "    [switch]$OmitExpectedGitHash",
      ")",
      "$ErrorActionPreference = \"Stop\"",
      "# Test-only mock: this does not make any Authenticode claim.",
      "function Get-AuthenticodeSignature {",
      "    param([string]$FilePath)",
      "    [pscustomobject]@{ Status = \"NotSigned\" }",
      "}",
      "if ($OmitExpectedGitHash) {",
      "    & $VerifierPath -CliExe $CliPath -ExpectedVersion $ExpectedVersion -Artifacts $CliPath -ChecksumOutput $ChecksumPath",
      "} else {",
      "    & $VerifierPath -CliExe $CliPath -ExpectedVersion $ExpectedVersion -ExpectedGitHash $ExpectedGitHash -Artifacts $CliPath -ChecksumOutput $ChecksumPath",
      "}",
      "if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }",
      "",
    ].join("\n"),
  );

  return { cliPath, runnerPath };
}

function runVerifier({ powershell, runnerPath, cliPath, checksumPath, output, expectedGitHash, omitExpectedGitHash }) {
  const args = [
    "-NoLogo",
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    runnerPath,
    "-VerifierPath",
    verifierPath,
    "-CliPath",
    cliPath,
    "-ChecksumPath",
    checksumPath,
    "-ExpectedVersion",
    expectedVersion,
  ];
  if (omitExpectedGitHash) {
    args.push("-OmitExpectedGitHash");
  } else {
    args.push("-ExpectedGitHash", expectedGitHash);
  }

  return spawnSync(powershell, args, {
    cwd: dirname(cliPath),
    encoding: "utf8",
    env: { ...process.env, SAGASCRIPT_TEST_VERSION_OUTPUT: output },
    stdio: ["ignore", "pipe", "pipe"],
  });
}

test("Windows release verifier enforces clean full build identity", (t) => {
  const root = mkdtempSync(join(tmpdir(), "sagascript-windows-release-identity-"));
  try {
    const powershell = findPowerShell(root);
    if (!powershell) {
      if (process.platform === "win32") {
        assert.fail("pwsh is required for the Windows release identity test on native Windows");
      }
      t.skip("pwsh unavailable outside native Windows");
      return;
    }

    const { cliPath, runnerPath } = writePowerShellFixtures(root);
    const run = (output, expected = expectedGitHash, omitExpectedGitHash = false) => {
      const checksumPath = join(root, `SHA256SUMS-${Math.random().toString(16).slice(2)}`);
      const result = runVerifier({
        powershell,
        runnerPath,
        cliPath,
        checksumPath,
        output,
        expectedGitHash: expected,
        omitExpectedGitHash,
      });
      return { result, checksumPath };
    };
    const validOutput = `sagascript ${expectedVersion} (git ${expectedGitHash}, built 2026-09-06)`;

    const backwardsCompatible = run(
      "sagascript 1.1.3 (git 0949345, built 2026-09-06)",
      "",
      true,
    );
    assert.equal(backwardsCompatible.result.status, 0, `${backwardsCompatible.result.stderr}\nstdout=${backwardsCompatible.result.stdout}`);
    assert.ok(existsSync(backwardsCompatible.checksumPath));

    const valid = run(validOutput);
    assert.equal(valid.result.status, 0, `${valid.result.stderr}\nstdout=${valid.result.stdout}`);
    assert.ok(existsSync(valid.checksumPath));

    const rejectedOutputs = [
      `sagascript ${expectedVersion} (git ${expectedGitHash}-dirty, built 2026-09-06)`,
      `sagascript ${expectedVersion} (git unknown, built 2026-09-06)`,
      `sagascript ${expectedVersion} (git ${"0".repeat(40)}, built 2026-09-06)`,
      `sagascript ${expectedVersion} (git ${expectedGitHash}, built 2026-09-06) trailing`,
      `sagascript ${expectedVersion} (git ${expectedGitHash}, built 2026-02-30)`,
      `sagascript ${expectedVersion} (git ${expectedGitHash}deadbeef, built 2026-09-06)`,
    ];
    for (const output of rejectedOutputs) {
      const rejected = run(output);
      assert.notEqual(rejected.result.status, 0, `unexpectedly accepted: ${output}`);
    }

    const malformedExpectedHashes = [
      "",
      expectedGitHash.toUpperCase(),
      expectedGitHash.slice(0, 39),
      `${expectedGitHash}0`,
    ];
    for (const malformedExpectedHash of malformedExpectedHashes) {
      const rejected = run(validOutput, malformedExpectedHash);
      assert.notEqual(
        rejected.result.status,
        0,
        `unexpectedly accepted expected hash: ${malformedExpectedHash}`,
      );
    }

    const mismatchedExpectedHash = run(validOutput, "1".repeat(40));
    assert.notEqual(mismatchedExpectedHash.result.status, 0);

    const malformedVersion = run(`sagascript ${expectedVersion}.0 (git ${expectedGitHash}, built 2026-09-06)`);
    assert.notEqual(malformedVersion.result.status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
