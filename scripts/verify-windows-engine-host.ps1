#!/usr/bin/env pwsh
# Verifies a Windows ARM64 ONNX engine-host directory: required files exist,
# `--version` matches the expected source revision, and the host answers the
# protocol `hello` and `ping` with onnxruntime.dll beside it.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$HostDir,
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-fA-F]{40}$')]
    [string]$ExpectedGitSha
)

$ErrorActionPreference = 'Stop'
$ExpectedGitSha = $ExpectedGitSha.ToLowerInvariant()

$hostExe = Join-Path $HostDir 'sagascript-engine-host-ort.exe'
foreach ($name in @('sagascript-engine-host-ort.exe', 'onnxruntime.dll')) {
    $path = Join-Path $HostDir $name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Missing $name in $HostDir"
    }
}

$versionOutput = (& $hostExe --version 2>&1) -join "`n"
if ($LASTEXITCODE -ne 0) { throw "Host --version failed with exit code ${LASTEXITCODE}: $versionOutput" }
Write-Output $versionOutput
if ($versionOutput -notmatch [regex]::Escape($ExpectedGitSha)) {
    throw "Host --version does not contain expected revision $ExpectedGitSha"
}

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $hostExe
$psi.Arguments = '--protocol 1'
$psi.WorkingDirectory = $HostDir
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$process = [System.Diagnostics.Process]::Start($psi)
try {
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $process.StandardInput.WriteLine('{"v":1,"id":1,"op":"hello","client":{"name":"windows-package-check","version":"0","git_sha":"' + $ExpectedGitSha + '"}}')
    $process.StandardInput.WriteLine('{"v":1,"id":2,"op":"ping"}')
    $process.StandardInput.WriteLine('{"v":1,"id":3,"op":"shutdown"}')
    $process.StandardInput.Flush()

    $responses = @{}
    while ($responses.Count -lt 3) {
        $readTask = $process.StandardOutput.ReadLineAsync()
        if (-not $readTask.Wait(30000)) { throw 'Timed out waiting for host response' }
        $line = $readTask.Result
        if ($null -eq $line) { break }
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        $message = $line | ConvertFrom-Json
        if ($null -ne $message.id -and $null -ne $message.ok) { $responses[[int]$message.id] = $message }
    }

    $hello = $responses[1]
    if ($null -eq $hello -or -not $hello.ok) { throw "hello failed: $($hello | ConvertTo-Json -Compress -Depth 5)" }
    if ($hello.host.git_sha -ne $ExpectedGitSha) {
        throw "hello git_sha $($hello.host.git_sha) does not match $ExpectedGitSha"
    }
    if ($hello.host.engine -ne 'onnx') { throw "Expected engine onnx, got $($hello.host.engine)" }
    $ping = $responses[2]
    if ($null -eq $ping -or -not $ping.ok) { throw "ping failed: $($ping | ConvertTo-Json -Compress -Depth 5)" }

    if (-not $process.WaitForExit(15000)) { throw 'Host did not exit after shutdown' }
    Write-Output "Engine host OK: engine=$($hello.host.engine) git_sha=$($hello.host.git_sha) dir=$HostDir"
}
finally {
    if (-not $process.HasExited) { $process.Kill() }
    if ($stderrTask -and $stderrTask.IsCompleted -and $stderrTask.Result) {
        Write-Output "Host stderr: $($stderrTask.Result)"
    }
}
