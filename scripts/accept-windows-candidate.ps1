#!/usr/bin/env pwsh

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$CliExe,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedVersion,

    [string]$Fixture,

    [string]$OutputPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-NonemptyFile {
    param([string]$Path, [string]$Label)

    $item = Get-Item -LiteralPath $Path -ErrorAction Stop
    if ($item.PSIsContainer -or $item.Length -le 0) {
        throw "$Label must be a non-empty file: $Path"
    }
    return $item.FullName
}

function ConvertTo-WindowsArgument {
    param([AllowNull()][string]$Argument)

    # ProcessStartInfo does not invoke a shell, but the Windows process API
    # still receives one command-line string. Quote every argument according
    # to CommandLineToArgvW rules so spaces, quotes, and trailing backslashes
    # survive the native argument parser unchanged.
    if ($null -eq $Argument) {
        return '""'
    }

    $builder = New-Object System.Text.StringBuilder
    [void]$builder.Append('"')
    $backslashes = 0
    foreach ($character in $Argument.ToCharArray()) {
        if ($character -eq '\') {
            $backslashes++
            continue
        }

        if ($character -eq '"') {
            for ($index = 0; $index -lt (2 * $backslashes + 1); $index++) {
                [void]$builder.Append('\')
            }
            [void]$builder.Append('"')
            $backslashes = 0
            continue
        }

        for ($index = 0; $index -lt $backslashes; $index++) {
            [void]$builder.Append('\')
        }
        [void]$builder.Append($character)
        $backslashes = 0
    }

    for ($index = 0; $index -lt (2 * $backslashes); $index++) {
        [void]$builder.Append('\')
    }
    [void]$builder.Append('"')
    return $builder.ToString()
}

function Invoke-Sagascript {
    param([string]$Executable, [string[]]$Arguments)

    $startInfo = New-Object System.Diagnostics.ProcessStartInfo
    $startInfo.FileName = $Executable
    $startInfo.Arguments = (($Arguments | ForEach-Object {
        ConvertTo-WindowsArgument -Argument $_
    }) -join ' ')
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    $startInfo.StandardOutputEncoding = $utf8
    $startInfo.StandardErrorEncoding = $utf8

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $startInfo
    try {
        if (-not $process.Start()) {
            throw "Could not start Sagascript executable: $Executable"
        }

        # Start both asynchronous reads before waiting for the process. A
        # native command can fill either redirected pipe independently.
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        $stdout = $stdoutTask.GetAwaiter().GetResult()
        $stderr = $stderrTask.GetAwaiter().GetResult()
        $exitCode = $process.ExitCode
    } finally {
        $process.Dispose()
    }

    if ($exitCode -ne 0) {
        throw "Sagascript $($Arguments -join ' ') failed with exit code $exitCode`nstdout:`n$stdout`nstderr:`n$stderr"
    }

    # Diagnostics on stderr are allowed for successful commands. Keep the
    # return value strictly to native stdout so JSON callers remain parseable.
    return $stdout
}

if ([string]::IsNullOrWhiteSpace($Fixture)) {
    $Fixture = Join-Path $PSScriptRoot "norwegian-short-3s.mp3"
}
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path (Get-Location).Path "windows-acceptance.json"
}

if ($ExpectedVersion -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$') {
    throw "ExpectedVersion is not a semantic version: $ExpectedVersion"
}

$cliExePath = Resolve-NonemptyFile -Path $CliExe -Label "CliExe"
$fixturePath = Resolve-NonemptyFile -Path $Fixture -Label "Fixture"
$outputFullPath = [System.IO.Path]::GetFullPath($OutputPath)
$outputParent = [System.IO.Path]::GetDirectoryName($outputFullPath)
if (-not [System.IO.Directory]::Exists($outputParent)) {
    throw "Acceptance output directory does not exist: $outputParent"
}

$versionOutput = Invoke-Sagascript -Executable $cliExePath -Arguments @("--version")
$versionPattern = "(?<![0-9.])$([regex]::Escape($ExpectedVersion))(?![0-9.])"
if ($versionOutput -notmatch $versionPattern) {
    throw "CLI executable did not report expected version $ExpectedVersion`: $versionOutput"
}
[void](Invoke-Sagascript -Executable $cliExePath -Arguments @("--help"))
[void](Invoke-Sagascript -Executable $cliExePath -Arguments @("list-models"))
[void](Invoke-Sagascript -Executable $cliExePath -Arguments @("config", "path"))

$downloadTimer = [System.Diagnostics.Stopwatch]::StartNew()
[void](Invoke-Sagascript -Executable $cliExePath -Arguments @("download-model", "nb-whisper-tiny"))
$downloadTimer.Stop()
$verificationTimer = [System.Diagnostics.Stopwatch]::StartNew()
[void](Invoke-Sagascript -Executable $cliExePath -Arguments @("download-model", "nb-whisper-tiny"))
$verificationTimer.Stop()

$transcriptionTimer = [System.Diagnostics.Stopwatch]::StartNew()
$transcriptionOutput = Invoke-Sagascript -Executable $cliExePath -Arguments @(
    "transcribe",
    "--language", "no",
    "--model", "nb-whisper-tiny",
    "--beam", "0",
    "--json",
    $fixturePath
)
$transcriptionTimer.Stop()

# ProcessStartInfo decodes stdout as UTF-8 before PowerShell can reinterpret it;
# trim only framing whitespace and a possible UTF-8 BOM before parsing JSON.
$transcriptionJson = $transcriptionOutput.Trim().TrimStart([char]0xFEFF)
$result = $transcriptionJson | ConvertFrom-Json
if ($result.language -ne "no") {
    throw "Expected Norwegian transcription result, got '$($result.language)'"
}
if ($result.text -notmatch "(?i)storting") {
    throw "Expected 'storting' in the public-fixture transcript"
}
if (-not $result.segments) {
    throw "Expected at least one transcription segment"
}

$os = Get-CimInstance Win32_OperatingSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$computer = Get-CimInstance Win32_ComputerSystem
$report = [ordered]@{
    schema_version = 1
    accepted_at_utc = [DateTime]::UtcNow.ToString("o")
    expected_version = $ExpectedVersion
    reported_version = $versionOutput.Trim()
    source_fixture_sha256 = (Get-FileHash -LiteralPath $fixturePath -Algorithm SHA256).Hash.ToLowerInvariant()
    download_seconds = [Math]::Round($downloadTimer.Elapsed.TotalSeconds, 3)
    verification_seconds = [Math]::Round($verificationTimer.Elapsed.TotalSeconds, 3)
    transcription_seconds = [Math]::Round($transcriptionTimer.Elapsed.TotalSeconds, 3)
    detected_language = $result.language
    segment_count = @($result.segments).Count
    windows = [ordered]@{
        caption = $os.Caption
        version = $os.Version
        build_number = $os.BuildNumber
        architecture = $os.OSArchitecture
    }
    hardware = [ordered]@{
        processor = $cpu.Name.Trim()
        logical_processors = $cpu.NumberOfLogicalProcessors
        memory_gib = [Math]::Round($computer.TotalPhysicalMemory / 1GB, 2)
    }
    automated_cli_acceptance = "pass"
    manual_gui_acceptance = "required"
}

$utf8WithoutBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText(
    $outputFullPath,
    (($report | ConvertTo-Json -Depth 5) + "`n"),
    $utf8WithoutBom
)

Write-Host "Automated installed-CLI acceptance passed"
Write-Host "Evidence: $outputFullPath"
Write-Host "Continue with the manual tray, microphone, hotkey, paste, focus, and uninstall checklist in README-Windows-Candidate.md"
