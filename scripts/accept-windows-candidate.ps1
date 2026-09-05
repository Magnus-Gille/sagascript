#!/usr/bin/env pwsh

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$CliExe,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedVersion,

    [string]$Fixture = (Join-Path $PSScriptRoot "norwegian-short-3s.mp3"),

    [string]$OutputPath = (Join-Path (Get-Location).Path "windows-acceptance.json")
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

function Invoke-Sagascript {
    param([string]$Executable, [string[]]$Arguments)

    $output = & $Executable @Arguments 2>&1
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "Sagascript $($Arguments -join ' ') failed with exit code $exitCode`n$($output -join "`n")"
    }
    return ($output -join "`n")
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

$transcriptTemp = Join-Path ([System.IO.Path]::GetTempPath()) ("sagascript-acceptance-{0}.json" -f [guid]::NewGuid())
try {
    $transcriptionTimer = [System.Diagnostics.Stopwatch]::StartNew()
    & $cliExePath transcribe `
        --language no `
        --model nb-whisper-tiny `
        --beam 0 `
        --json `
        $fixturePath > $transcriptTemp
    $transcriptionExit = $LASTEXITCODE
    $transcriptionTimer.Stop()
    if ($transcriptionExit -ne 0) {
        throw "Installed-file transcription failed with exit code $transcriptionExit"
    }

    $result = Get-Content -LiteralPath $transcriptTemp -Raw | ConvertFrom-Json
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
} finally {
    Remove-Item -LiteralPath $transcriptTemp -Force -ErrorAction SilentlyContinue
}

Write-Host "Automated installed-CLI acceptance passed"
Write-Host "Evidence: $outputFullPath"
Write-Host "Continue with the manual tray, microphone, hotkey, paste, focus, and uninstall checklist in README-Windows-Candidate.md"
