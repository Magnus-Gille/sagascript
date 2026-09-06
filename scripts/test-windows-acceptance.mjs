import assert from "node:assert/strict";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import test from "node:test";

const acceptanceScript = await readFile(
  new URL("./accept-windows-candidate.ps1", import.meta.url),
  "utf8",
);

test("Windows acceptance uses separate asynchronous UTF-8 native streams", () => {
  assert.match(acceptanceScript, /System\.Diagnostics\.ProcessStartInfo/);
  assert.match(acceptanceScript, /UseShellExecute = \$false/);
  assert.match(acceptanceScript, /StandardOutputEncoding/);
  assert.match(acceptanceScript, /StandardErrorEncoding/);
  assert.match(acceptanceScript, /ReadToEndAsync\(\)/g);
  assert.equal((acceptanceScript.match(/ReadToEndAsync\(\)/g) ?? []).length, 2);
  assert.doesNotMatch(acceptanceScript, /2>&1/);
  assert.doesNotMatch(acceptanceScript, />\s*\$transcriptTemp/);
});

test("Windows acceptance passes Unicode JSON with native stderr diagnostics", (t) => {
  if (process.platform !== "win32") {
    t.skip("Windows-only acceptance harness");
    return;
  }

  const compilerCandidates = [
    join(process.env.SystemRoot ?? "C:\\Windows", "Microsoft.NET", "Framework64", "v4.0.30319", "csc.exe"),
    join(process.env.SystemRoot ?? "C:\\Windows", "Microsoft.NET", "Framework", "v4.0.30319", "csc.exe"),
  ];
  const compiler = compilerCandidates.find((candidate) => existsSync(candidate));
  if (!compiler) {
    t.skip("Microsoft C# compiler is unavailable");
    return;
  }

  const tempRoot = mkdtempSync(join(tmpdir(), "sagascript acceptance "));
  try {
    const sourcePath = join(tempRoot, "acceptance stub.cs");
    const cliPath = join(tempRoot, "acceptance stub.exe");
    const fixturePath = join(tempRoot, "public fixture.mp3");
    const reportPath = join(tempRoot, "acceptance report.json");
    const runnerPath = join(tempRoot, "run acceptance.ps1");
    writeFileSync(
      sourcePath,
      String.raw`using System;
using System.Text;

class Program
{
    static int Main(string[] args)
    {
        Console.OutputEncoding = new UTF8Encoding(false);
        Console.Error.WriteLine("diagnostic: caf\u00e9");
        if (Environment.GetEnvironmentVariable("SAGASCRIPT_ACCEPTANCE_STUB_FAIL") == "1")
        {
            Console.Error.WriteLine("forced native failure");
            return 23;
        }
        if (args.Length == 0) return 9;
        if (args[0] == "--version")
        {
            Console.WriteLine("sagascript 1.1.3 \u2603");
            return 0;
        }
        if (args[0] == "transcribe")
        {
            Console.WriteLine("{\"language\":\"no\",\"text\":\"storting caf\u00e9\",\"segments\":[{\"text\":\"storting caf\u00e9\"}]}");
            return 0;
        }
        if (args[0] == "--help" || args[0] == "list-models" || args[0] == "config" || args[0] == "download-model")
        {
            Console.WriteLine("ok \u2603");
            return 0;
        }
        Console.Error.WriteLine("unsupported command");
        return 11;
    }
}`,
      "utf8",
    );
    const compile = spawnSync(
      compiler,
      ["/nologo", "/target:exe", `/out:${cliPath}`, sourcePath],
      { encoding: "utf8" },
    );
    assert.equal(compile.status, 0, compile.stderr || compile.stdout);
    writeFileSync(fixturePath, Buffer.from("public fixture bytes\n", "utf8"));
    writeFileSync(
      runnerPath,
      String.raw`param(
    [string]$ScriptPath,
    [string]$CliExe,
    [string]$Fixture,
    [string]$OutputPath
)
$ErrorActionPreference = "Stop"
function Get-CimInstance {
    param([string]$ClassName)
    if ($ClassName -eq "Win32_OperatingSystem") {
        return [pscustomobject]@{ Caption = "Windows test"; Version = "10.0"; BuildNumber = "test"; OSArchitecture = "x64" }
    }
    if ($ClassName -eq "Win32_Processor") {
        return [pscustomobject]@{ Name = "Acceptance test CPU"; NumberOfLogicalProcessors = 4 }
    }
    if ($ClassName -eq "Win32_ComputerSystem") {
        return [pscustomobject]@{ TotalPhysicalMemory = 8GB }
    }
    throw "Unexpected CIM class: $ClassName"
}
function Get-FileHash {
    param([string]$LiteralPath, [string]$Algorithm)
    return [pscustomobject]@{ Hash = ("0" * 64) }
}
try {
    & $ScriptPath -CliExe $CliExe -ExpectedVersion "1.1.3" -Fixture $Fixture -OutputPath $OutputPath
    exit 0
} catch {
    Write-Error $_
    exit 1
}`,
      "utf8",
    );

    const powershell = join(
      process.env.SystemRoot ?? "C:\\Windows",
      "System32",
      "WindowsPowerShell",
      "v1.0",
      "powershell.exe",
    );
    const run = (env = process.env) =>
      spawnSync(
        powershell,
        [
          "-NoLogo",
          "-NoProfile",
          "-NonInteractive",
          "-ExecutionPolicy",
          "Bypass",
          "-File",
          runnerPath,
          "-ScriptPath",
          fileURLToPath(new URL("./accept-windows-candidate.ps1", import.meta.url)),
          "-CliExe",
          cliPath,
          "-ExpectedVersion",
          "1.1.3",
          "-Fixture",
          fixturePath,
          "-OutputPath",
          reportPath,
        ],
        { encoding: "utf8", env },
      );

    const accepted = run();
    assert.equal(accepted.status, 0, accepted.stderr || accepted.stdout);
    const report = JSON.parse(readFileSync(reportPath, "utf8"));
    assert.equal(report.automated_cli_acceptance, "pass");
    assert.equal(report.detected_language, "no");
    assert.equal(report.segment_count, 1);

    const failed = run({ ...process.env, SAGASCRIPT_ACCEPTANCE_STUB_FAIL: "1" });
    assert.notEqual(failed.status, 0);
    assert.match(`${failed.stdout}\n${failed.stderr}`, /exit code 23/);
    assert.match(`${failed.stdout}\n${failed.stderr}`, /forced native failure/);
  } finally {
    rmSync(tempRoot, { recursive: true, force: true });
  }
});
