#!/usr/bin/env pwsh

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$LogPath,

    [Parameter(Mandatory = $true)]
    [string]$OutputPath,

    [string]$Since
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-Field {
    param(
        [AllowNull()][object]$Object,
        [Parameter(Mandatory = $true)][string]$Name
    )

    if ($null -eq $Object) {
        return $null
    }
    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) {
        return $null
    }
    return $property.Value
}

function ConvertTo-Identifier {
    param([AllowNull()][object]$Value)

    if ($null -eq $Value) {
        return "unknown"
    }
    $text = ([string]$Value).Trim()
    if ($text -match '^[A-Za-z0-9][A-Za-z0-9_.-]*$') {
        return $text
    }
    return "unknown"
}

function ConvertTo-HardwareLabel {
    param([AllowNull()][object]$Value)

    if ($null -eq $Value) {
        return "unknown"
    }
    $text = ([string]$Value) -replace '[\r\n\t]+', ' '
    $text = $text.Trim()
    if ($text.Length -eq 0 -or $text.Length -gt 200) {
        return "unknown"
    }
    return $text
}

function ConvertTo-Number {
    param([AllowNull()][object]$Value)

    if ($null -eq $Value) {
        return $null
    }
    $number = 0.0
    $parsed = [double]::TryParse(
        [string]$Value,
        [Globalization.NumberStyles]::Float,
        [Globalization.CultureInfo]::InvariantCulture,
        [ref]$number
    )
    if ($parsed -and $number -ge 0 -and -not [double]::IsNaN($number) -and -not [double]::IsInfinity($number)) {
        return $number
    }
    return $null
}

function Get-Distribution {
    param([AllowNull()][object[]]$Values)

    if ($null -eq $Values -or $Values.Count -eq 0) {
        return "unavailable"
    }

    $sorted = @($Values | Sort-Object)
    $lastIndex = $sorted.Count - 1
    $result = [ordered]@{}
    foreach ($label in @("p50", "p95")) {
        $fraction = if ($label -eq "p50") { 0.50 } else { 0.95 }
        $position = $lastIndex * $fraction
        $lower = [int][Math]::Floor($position)
        $upper = [int][Math]::Ceiling($position)
        if ($lower -eq $upper) {
            $percentile = [double]$sorted[$lower]
        } else {
            $weight = $position - $lower
            $percentile = ([double]$sorted[$lower]) + (([double]$sorted[$upper] - [double]$sorted[$lower]) * $weight)
        }
        $result[$label] = [Math]::Round($percentile, 3)
    }
    return $result
}

function Get-HardwareMetadata {
    $hardware = [ordered]@{}
    try {
        $os = Get-CimInstance Win32_OperatingSystem
        if ($null -ne $os) {
            $hardware.os = [ordered]@{
                caption = ConvertTo-HardwareLabel (Get-Field $os "Caption")
                version = ConvertTo-Identifier (Get-Field $os "Version")
                build_number = ConvertTo-Identifier (Get-Field $os "BuildNumber")
                architecture = ConvertTo-Identifier (Get-Field $os "OSArchitecture")
            }
        }
    } catch {
        # Hardware access can be restricted by a test runner; omit only the
        # unavailable section and never expose the exception text.
    }

    try {
        $cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
        if ($null -ne $cpu) {
            $logicalProcessors = ConvertTo-Number (Get-Field $cpu "NumberOfLogicalProcessors")
            $hardware.cpu = [ordered]@{
                model = ConvertTo-HardwareLabel (Get-Field $cpu "Name")
                logical_processors = if ($null -eq $logicalProcessors) { "unavailable" } else { [int]$logicalProcessors }
            }
        }
    } catch {
    }

    try {
        $computer = Get-CimInstance Win32_ComputerSystem
        if ($null -ne $computer) {
            $memoryBytes = ConvertTo-Number (Get-Field $computer "TotalPhysicalMemory")
            $hardware.computer = [ordered]@{
                model = ConvertTo-HardwareLabel (Get-Field $computer "Model")
                memory_gib = if ($null -eq $memoryBytes) { "unavailable" } else { [Math]::Round($memoryBytes / 1GB, 3) }
            }
        }
    } catch {
    }

    if ($hardware.Count -eq 0) {
        $hardware.status = "unavailable"
    }
    return $hardware
}

function Get-GroupSummary {
    param([AllowNull()][object[]]$Samples)

    $successfulSamples = @($Samples | Where-Object { $_.outcome -eq "success" })
    $totalValues = @($successfulSamples | ForEach-Object { if ($null -ne $_.total_ms) { $_.total_ms } })
    $phaseValues = @{}
    $allowedPhases = @(
        "recording_finalization",
        "conversion",
        "model_acquisition",
        "inference",
        "postprocessing",
        "clipboard_focus_paste"
    )
    foreach ($sample in $successfulSamples) {
        foreach ($phase in $sample.phases.PSObject.Properties) {
            if ($allowedPhases -notcontains $phase.Name) {
                continue
            }
            $value = ConvertTo-Number $phase.Value
            if ($null -ne $value) {
                if (-not $phaseValues.ContainsKey($phase.Name)) {
                    $phaseValues[$phase.Name] = New-Object System.Collections.ArrayList
                }
                [void]$phaseValues[$phase.Name].Add($value)
            }
        }
    }

    $phaseSummary = [ordered]@{}
    foreach ($phaseName in @($phaseValues.Keys | Sort-Object)) {
        $phaseSummary[$phaseName] = Get-Distribution -Values @($phaseValues[$phaseName])
    }
    $phases = if ($phaseSummary.Count -eq 0) { "unavailable" } else { $phaseSummary }
    return [ordered]@{
        sample_count = @($Samples).Count
        measured_success_count = $successfulSamples.Count
        total_ms = Get-Distribution -Values $totalValues
        phases_ms = $phases
    }
}

$resolvedLogPath = (Get-Item -LiteralPath $LogPath -ErrorAction Stop).FullName
if ((Get-Item -LiteralPath $resolvedLogPath).PSIsContainer) {
    throw "LogPath must be a file"
}
$outputFullPath = [IO.Path]::GetFullPath($OutputPath)
$outputParent = [IO.Path]::GetDirectoryName($outputFullPath)
if (-not [IO.Directory]::Exists($outputParent)) {
    throw "Output directory does not exist"
}

$sinceTime = $null
if (-not [string]::IsNullOrWhiteSpace($Since)) {
    try {
        $sinceTime = [DateTimeOffset]::Parse(
            $Since,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::AssumeUniversal
        )
    } catch {
        throw "Since must be a valid timestamp"
    }
}

$samples = New-Object System.Collections.ArrayList
# Stream one line at a time so summarizing a rotated log does not duplicate a
# full private log in memory or carry any unrecognized fields into the report.
Get-Content -LiteralPath $resolvedLogPath -ReadCount 1 | ForEach-Object {
    $line = ([string]$_).TrimStart([char]0xFEFF)
    if ([string]::IsNullOrWhiteSpace($line)) {
        return
    }

    try {
        $entry = $line | ConvertFrom-Json
    } catch {
        return
    }
    if ((Get-Field $entry "event") -ne "dictation_session_finished") {
        return
    }

    if ($null -ne $sinceTime) {
        try {
            $entryTime = [DateTimeOffset]::Parse(
                [string](Get-Field $entry "ts"),
                [Globalization.CultureInfo]::InvariantCulture,
                [Globalization.DateTimeStyles]::AssumeUniversal
            )
        } catch {
            return
        }
        if ($entryTime -lt $sinceTime) {
            return
        }
    }

    $data = Get-Field $entry "data"
    if ($null -eq $data) {
        return
    }
    $cache = Get-Field $data "model_cached"
    $cacheState = if ($cache -is [bool]) {
        if ($cache) { "warm" } else { "cold" }
    } else {
        "unknown"
    }
    $phases = Get-Field $data "phases_ms"
    if ($null -eq $phases) {
        $phases = [pscustomobject]@{}
    }
    $outcomeValue = Get-Field $data "outcome"
    $outcome = if ($null -eq $outcomeValue) {
        "unknown"
    } else {
        (ConvertTo-Identifier $outcomeValue).ToLowerInvariant()
    }

    [void]$samples.Add([pscustomobject]@{
        language = ConvertTo-Identifier (Get-Field $data "language")
        model = ConvertTo-Identifier (Get-Field $data "model")
        context_profile = ConvertTo-Identifier (Get-Field $data "context_profile")
        version = ConvertTo-Identifier (Get-Field $data "version")
        git_hash = ConvertTo-Identifier (Get-Field $data "git_hash")
        model_cached = $cacheState
        outcome = $outcome
        audio_ms = ConvertTo-Number (Get-Field $data "audio_ms")
        total_ms = ConvertTo-Number (Get-Field $data "key_up_to_completion_ms")
        phases = $phases
    })
}

$hardware = Get-HardwareMetadata
if ($samples.Count -eq 0) {
    $report = [ordered]@{
        schema_version = 1
        status = "unavailable"
        reason = "no_terminal_entries"
        hardware = $hardware
    }
} else {
    $outcomes = [ordered]@{
        success = 0
        error = 0
        cancelled = 0
        other = 0
    }
    $durationBuckets = [ordered]@{
        short = 0
        medium = 0
        long = 0
    }
    foreach ($sample in $samples) {
        if ($outcomes.Contains($sample.outcome)) {
            $outcomes[$sample.outcome]++
        } else {
            $outcomes.other++
        }
        if ($null -ne $sample.audio_ms) {
            if ($sample.audio_ms -le 5000) {
                $durationBuckets.short++
            } elseif ($sample.audio_ms -le 15000) {
                $durationBuckets.medium++
            } else {
                $durationBuckets.long++
            }
        }
    }

    $overall = Get-GroupSummary -Samples @($samples)
    $buildMap = @{}
    foreach ($sample in $samples) {
        if ($sample.version -eq "unknown" -and $sample.git_hash -eq "unknown") {
            continue
        }
        $buildKey = "$($sample.version)|$($sample.git_hash)"
        if (-not $buildMap.ContainsKey($buildKey)) {
            $buildMap[$buildKey] = [ordered]@{
                version = $sample.version
                git_hash = $sample.git_hash
            }
        }
    }
    $builds = @($buildMap.Values | Sort-Object version, git_hash)
    $groups = @{}
    foreach ($sample in $samples) {
        $key = "$($sample.language)|$($sample.model_cached)"
        if (-not $groups.ContainsKey($key)) {
            $groups[$key] = New-Object System.Collections.ArrayList
        }
        [void]$groups[$key].Add($sample)
    }

    $perLanguage = [ordered]@{}
    foreach ($language in @($groups.Keys | ForEach-Object { ($_ -split '\|', 2)[0] } | Sort-Object -Unique)) {
        $languageGroups = @()
        foreach ($cacheState in @("cold", "warm", "unknown")) {
            $key = "$language|$cacheState"
            if ($groups.ContainsKey($key)) {
                $groupSamples = @($groups[$key])
                $summary = Get-GroupSummary -Samples $groupSamples
                $languageGroups += [ordered]@{
                    model_cached = $cacheState
                    sample_count = $summary.sample_count
                    total_ms = $summary.total_ms
                    phases_ms = $summary.phases_ms
                    models = @($groupSamples | ForEach-Object { $_.model } | Sort-Object -Unique)
                    context_profiles = @($groupSamples | ForEach-Object { $_.context_profile } | Sort-Object -Unique)
                    builds = @($groupSamples | Where-Object { $_.version -ne "unknown" -or $_.git_hash -ne "unknown" } | ForEach-Object { "$($_.version)|$($_.git_hash)" } | Sort-Object -Unique)
                }
            }
        }
        $perLanguage[$language] = $languageGroups
    }

    $report = [ordered]@{
        schema_version = 1
        status = "available"
        sample_count = $samples.Count
        builds = $builds
        outcomes = $outcomes
        duration_buckets = $durationBuckets
        latency_ms = [ordered]@{
            measured_success_count = $overall.measured_success_count
            total = $overall.total_ms
            phases = $overall.phases_ms
        }
        per_language = $perLanguage
        hardware = $hardware
    }
}

$utf8WithoutBom = New-Object System.Text.UTF8Encoding($false)
[IO.File]::WriteAllText(
    $outputFullPath,
    (($report | ConvertTo-Json -Depth 8) + "`n"),
    $utf8WithoutBom
)

Write-Host "Windows dictation summary written"
Write-Host "Evidence: $outputFullPath"
