#!/usr/bin/env pwsh
<#
.SYNOPSIS
Checks Pianissimo (Swedish, ONNX int8) on a Windows-on-ARM (Snapdragon) laptop.

.DESCRIPTION
Run after installing the unsigned arm64 Sagascript build. Needs no admin rights
and changes nothing except the app's own model download (about 660 MB).
Writes windows-arm-pianissimo-result.json and .txt to -OutputDir.

.EXAMPLE
./windows-arm-pianissimo-test.ps1
./windows-arm-pianissimo-test.ps1 -Audio C:\audio\swedish.wav -LongAudio C:\audio\meeting.mp3 -Yes
#>
[CmdletBinding()]
param(
    [string]$Audio,
    [string]$LongAudio,
    [string]$Cli,
    [string]$Ref = 'main',
    [string]$OutputDir = (Join-Path $env:USERPROFILE 'sagascript-pianissimo-test'),
    [switch]$Yes
)

$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$summary = [ordered]@{ started = (Get-Date).ToString('o'); steps = [ordered]@{} }
$textLines = New-Object System.Collections.Generic.List[string]

function Add-Line([string]$text) { Write-Host $text; $textLines.Add($text) }
function Stop-Test([string]$message) {
    $summary.failure = $message
    Save-Result
    Write-Host "FAILED: $message" -ForegroundColor Red
    exit 1
}
function Save-Result {
    $summary.finished = (Get-Date).ToString('o')
    $summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $OutputDir 'windows-arm-pianissimo-result.json') -Encoding UTF8
    $textLines | Set-Content -LiteralPath (Join-Path $OutputDir 'windows-arm-pianissimo-result.txt') -Encoding UTF8
}
function Invoke-Step([string]$name, [string[]]$Arguments, [switch]$Capture) {
    Add-Line "== $name : sagascript $($Arguments -join ' ')"
    $outFile = Join-Path $OutputDir ("$name.stdout.txt")
    $errFile = Join-Path $OutputDir ("$name.stderr.txt")
    $elapsed = Measure-Command {
        & $script:CliPath @Arguments > $outFile 2> $errFile
        $script:LastExit = $LASTEXITCODE
    }
    $stdout = if (Test-Path $outFile) { Get-Content -LiteralPath $outFile -Raw } else { '' }
    $stderr = if (Test-Path $errFile) { Get-Content -LiteralPath $errFile -Raw } else { '' }
    $summary.steps[$name] = [ordered]@{ exit_code = $script:LastExit; seconds = [math]::Round($elapsed.TotalSeconds, 2) }
    if ($stdout) { Add-Line $stdout.TrimEnd() }
    if ($script:LastExit -ne 0) {
        if ($stderr) { Add-Line $stderr.TrimEnd() }
        Stop-Test "$name exited with code $script:LastExit. See $errFile"
    }
    return $stdout
}

# 1. Machine info
$os = Get-CimInstance Win32_OperatingSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$ramGb = [math]::Round($os.TotalVisibleMemorySize / 1MB, 1)
$osArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
$summary.machine = [ordered]@{ os = $os.Caption; build = $os.BuildNumber; os_arch = $osArch; cpu = $cpu.Name; cores = $cpu.NumberOfCores; logical = $cpu.NumberOfLogicalProcessors; ram_gb = $ramGb }
Add-Line "OS: $($os.Caption) build $($os.BuildNumber) ($osArch)"
Add-Line "CPU: $($cpu.Name), $($cpu.NumberOfCores) cores / $($cpu.NumberOfLogicalProcessors) threads"
Add-Line "RAM: $ramGb GB"
if ($osArch -ne 'Arm64') { Stop-Test "This test is for Windows on ARM64; this machine reports $osArch. Install the arm64 build on a Snapdragon laptop." }

# 2. Locate the installed CLI
function Test-HasHost([string]$cliPath) {
    Test-Path -LiteralPath (Join-Path (Split-Path $cliPath -Parent) 'engine-host\sagascript-engine-host-ort.exe') -PathType Leaf
}
$script:CliPath = $null
if ($Cli) {
    if (-not (Test-Path -LiteralPath $Cli -PathType Leaf)) { Stop-Test "-Cli path not found: $Cli" }
    $script:CliPath = $Cli
    Add-Line "CLI chosen because -Cli was given."
} else {
    # Known install locations first; PATH may hold an older or x64 sagascript.exe.
    $known = @()
    foreach ($root in @($env:LOCALAPPDATA, $env:ProgramFiles)) {
        if ($root) {
            # The installer ships the console CLI as sagascript-cli.exe; sagascript.exe is the GUI app
            # (older 1.4.x installs have only that, with a hand-copied CLI).
            foreach ($dir in @((Join-Path $root 'Sagascript'), (Join-Path $root 'Programs\Sagascript'))) {
                $preferred = @('sagascript-cli.exe', 'sagascript.exe') | ForEach-Object { Join-Path $dir $_ } |
                    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
                if ($preferred) { $known += $preferred }
            }
        }
    }
    $found = @($known | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) -and (Test-HasHost $_) } | Select-Object -Unique)
    if ($found.Count -gt 1) {
        Stop-Test ("Several installs with an engine host found: " + ($found -join '; ') + ". Pass -Cli <path to the sagascript.exe to test>.")
    }
    if ($found.Count -eq 1) {
        $script:CliPath = $found[0]
        Add-Line "CLI chosen from a known install location with a sibling engine-host."
    } else {
        $onPath = @(Get-Command sagascript-cli -All -ErrorAction SilentlyContinue | ForEach-Object { $_.Source } | Select-Object -Unique)
        if ($onPath.Count -eq 0) { $onPath = @(Get-Command sagascript -All -ErrorAction SilentlyContinue | ForEach-Object { $_.Source } | Select-Object -Unique) }
        $withHost = @($onPath | Where-Object { Test-HasHost $_ })
        if ($withHost.Count -gt 1) {
            Stop-Test ("Several sagascript.exe on PATH have an engine host: " + ($withHost -join '; ') + ". Pass -Cli <path>.")
        }
        if ($withHost.Count -eq 1) {
            $script:CliPath = $withHost[0]
            Add-Line "CLI chosen from PATH (no known install location had an engine-host)."
        } elseif ($onPath.Count -ge 1) {
            $script:CliPath = $onPath[0]
            Add-Line "WARNING: CLI taken from PATH but it has no engine-host beside it; Pianissimo will likely be unavailable."
        }
    }
}
if (-not $script:CliPath) { Stop-Test "Could not find sagascript-cli.exe. Install the arm64 build, or pass -Cli <path to sagascript-cli.exe>." }
Add-Line "CLI: $script:CliPath"
$summary.cli = $script:CliPath
$hostExe = Join-Path (Split-Path $script:CliPath -Parent) 'engine-host\sagascript-engine-host-ort.exe'
$ortDll = Join-Path (Split-Path $script:CliPath -Parent) 'engine-host\onnxruntime.dll'
$ortProvidersDll = Join-Path (Split-Path $script:CliPath -Parent) 'engine-host\onnxruntime_providers_shared.dll'
$summary.engine_host_present = (Test-Path -LiteralPath $hostExe) -and (Test-Path -LiteralPath $ortDll) -and (Test-Path -LiteralPath $ortProvidersDll)
Add-Line "Engine host beside CLI: $($summary.engine_host_present)"

# 3. Version and engine status
Invoke-Step 'version' @('--version') | Out-Null
Invoke-Step 'engine-status' @('engine', 'status', '--json') | Out-Null

# 4. Model download (about 660 MB)
if (-not $Yes) {
    $answer = Read-Host 'Download the Pianissimo Swedish model (about 660 MB) into the Sagascript models folder? [y/N]'
    if ($answer -notmatch '^(y|yes)$') { Stop-Test 'Model download declined; nothing else can be tested without it.' }
}
Invoke-Step 'download-model' @('download-model', 'pianissimo-sv') | Out-Null

# 5. Doctor
Invoke-Step 'engine-doctor' @('engine', 'doctor', '--json') | Out-Null

# 6. Audio to transcribe
if (-not $Audio) {
    $Audio = Join-Path $OutputDir 'swedish-fleurs-hongkong.wav'
    if (-not (Test-Path -LiteralPath $Audio)) {
        $url = "https://raw.githubusercontent.com/Magnus-Gille/sagascript/$Ref/test-audio/swedish-fleurs-hongkong.wav"
        Add-Line "Fetching test audio: $url"
        try { Invoke-WebRequest -Uri $url -OutFile $Audio } catch { Stop-Test "Could not fetch the test WAV from $url ($($_.Exception.Message)). Pass -Audio <path> or a different -Ref." }
    }
}
if (-not (Test-Path -LiteralPath $Audio)) { Stop-Test "Audio file not found: $Audio" }

function Invoke-Transcription([string]$name, [string]$file) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $json = Invoke-Step $name @('transcribe', '--model', 'pianissimo-sv', '--language', 'sv', '--json', $file)
    $sw.Stop()
    $result = $null
    try { $result = $json | ConvertFrom-Json } catch { Add-Line "Note: output was not valid JSON ($($_.Exception.Message))" }
    $text = if ($result -and $result.text) { [string]$result.text } else { '' }
    if (-not $text) { Stop-Test "$name produced no transcript text" }
    $summary.steps[$name].file = $file
    $summary.steps[$name].file_bytes = (Get-Item -LiteralPath $file).Length
    $summary.steps[$name].transcript_chars = $text.Length
    $summary.steps[$name].transcript_preview = $text.Substring(0, [math]::Min(200, $text.Length))
    Add-Line "$name : $([math]::Round($sw.Elapsed.TotalSeconds, 2)) s wall, transcript: $($summary.steps[$name].transcript_preview)"
}

# 7. Transcription (the first run includes model load; the second shows the warm speed)
Invoke-Transcription 'transcribe-short-cold' $Audio
Invoke-Transcription 'transcribe-short-warm' $Audio
if ($LongAudio) {
    if (-not (Test-Path -LiteralPath $LongAudio)) { Stop-Test "Long audio file not found: $LongAudio" }
    Invoke-Transcription 'transcribe-long' $LongAudio
}

$summary.result = 'pass'
Add-Line 'RESULT: pass'
Save-Result
Write-Host ''
Write-Host "Send back both files from ${OutputDir}: windows-arm-pianissimo-result.json and windows-arm-pianissimo-result.txt"
