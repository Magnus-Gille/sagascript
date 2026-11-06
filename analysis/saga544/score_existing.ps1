[CmdletBinding()]
param(
    [string]$LabRoot = 'C:\Users\magnu\AppData\Local\Temp\sagascript-accelerator-lab',
    [string]$RawHostJson = 'cpu-current-paired.json',
    [string]$MergedJsonl = 'matrix-results-final\cpu-long.stdout.jsonl',
    [switch]$ShowTexts
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$manifest = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'frozen-split.json') -Raw | ConvertFrom-Json
$fixtures = Get-Content -LiteralPath (Join-Path $LabRoot 'matrix-fixtures-final\fixtures.json') -Raw | ConvertFrom-Json
$raw = Get-Content -LiteralPath (Join-Path $LabRoot $RawHostJson) -Raw | ConvertFrom-Json
$mergedRows = @(Get-Content -LiteralPath (Join-Path $LabRoot $MergedJsonl) | ForEach-Object { $_ | ConvertFrom-Json })
$goodRows = @($mergedRows | Where-Object { $_.status -eq 'ok' })
if ($goodRows.Count -eq 0 -or -not $goodRows[0].result.segments) { throw 'No successful long transcript result with segments was found.' }
$full = $goodRows[0].result
$devIds = @($manifest.development | ForEach-Object { [string]$_.id })
$devMap = @{}
foreach ($clip in $manifest.development) { $devMap[[string]$clip.id] = $clip }

function Get-Words([string]$Text) {
    $normalized = $Text.Normalize([Text.NormalizationForm]::FormC).ToLowerInvariant()
    $normalized = $normalized.Normalize([Text.NormalizationForm]::FormC)
    $matches = [regex]::Matches(
        $normalized,
        "[\p{L}\p{M}\p{N}]+(?:['’][\p{L}\p{M}\p{N}]+)*"
    )
    @($matches | ForEach-Object { $_.Value })
}

function Get-EditStats([string]$Reference, [string]$Hypothesis) {
    $referenceWords = @(Get-Words $Reference)
    $hypothesisWords = @(Get-Words $Hypothesis)
    $n = $referenceWords.Count
    $m = $hypothesisWords.Count
    $dp = [int[,]]::new($n + 1, $m + 1)
    for ($i = 1; $i -le $n; $i++) { $dp[$i, 0] = $i }
    for ($j = 1; $j -le $m; $j++) { $dp[0, $j] = $j }
    for ($i = 1; $i -le $n; $i++) {
        for ($j = 1; $j -le $m; $j++) {
            $substitution = $dp[($i - 1), ($j - 1)] + [int]($referenceWords[$i - 1] -ne $hypothesisWords[$j - 1])
            $deletion = $dp[($i - 1), $j] + 1
            $insertion = $dp[$i, ($j - 1)] + 1
            $dp[$i, $j] = [math]::Min($substitution, [math]::Min($deletion, $insertion))
        }
    }
    $substitutions = 0
    $deletions = 0
    $insertions = 0
    $i = $n
    $j = $m
    while ($i -gt 0 -and $j -gt 0) {
        $same = $referenceWords[$i - 1] -ceq $hypothesisWords[$j - 1]
        $diagonal = $dp[($i - 1), ($j - 1)] + [int](-not $same)
        if ($dp[$i, $j] -eq $diagonal) {
            if (-not $same) { $substitutions++ }
            $i--; $j--
        } elseif ($dp[$i, $j] -eq ($dp[($i - 1), $j] + 1)) {
            $deletions++; $i--
        } else {
            $insertions++; $j--
        }
    }
    $deletions += $i
    $insertions += $j
    [pscustomobject]@{
        reference_words = $n
        hypothesis_words = $m
        substitutions = $substitutions
        deletions = $deletions
        insertions = $insertions
        edits = ($substitutions + $deletions + $insertions)
    }
}

$clipBounds = [System.Collections.Generic.List[object]]::new()
$offset = 0.0
foreach ($clip in $fixtures.fixtures.long.composition) {
    $end = $offset + [double]$clip.duration_seconds
    $clipBounds.Add([pscustomobject]@{ id = [string]$clip.id; start = $offset; end = $end })
    $offset = $end
}

$rawTotal = 0
$mergedTotal = 0
$refTotal = 0
$rows = foreach ($bounds in $clipBounds) {
    if (-not $devMap.ContainsKey($bounds.id)) { continue }
    $clip = $devMap[$bounds.id]
    $rawRuns = @($raw.runs | Where-Object { [string]$_.id -eq $bounds.id })
    if ($rawRuns.Count -eq 0) {
        $rawStats = $null
        $uniqueRuns = 0
    } else {
        $rawStats = Get-EditStats $clip.reference $rawRuns[0].text
        $uniqueRuns = @($rawRuns | Select-Object -ExpandProperty text -Unique).Count
    }
    $selectedSegments = @($full.segments | Where-Object {
        $_.start -ge $bounds.start -and $_.start -lt $bounds.end
    })
    $mergedText = ($selectedSegments | ForEach-Object { $_.text }) -join ' '
    $mergedStats = Get-EditStats $clip.reference $mergedText
    if ($rawStats) { $rawTotal += $rawStats.edits }
    $mergedTotal += $mergedStats.edits
    $refTotal += $mergedStats.reference_words
    [pscustomobject]@{
        id = $bounds.id
        raw_runs = $rawRuns.Count
        raw_unique_transcripts = $uniqueRuns
        raw_edits = if ($rawStats) { $rawStats.edits } else { $null }
        merged_edits = $mergedStats.edits
        reference_words = $mergedStats.reference_words
        merged_words = $mergedStats.hypothesis_words
        segment_count = $selectedSegments.Count
        raw_text = if ($rawRuns.Count) { $rawRuns[0].text } else { '' }
        merged_text = $mergedText
    }
}
Write-Output ('Run configuration: per-clip host window_s={0}; long CLI first window audio_seconds={1}. Compare host and encoder identities before attributing any difference to merge behavior.' -f $raw.load.window_s, $full.performance.windows[0].audio_seconds)
Write-Output 'Metric note: merged results are sliced by segment start time within each fixture clip interval; they retain the long CLI acoustic context and merge behavior.'
$rows | Format-Table id,raw_runs,raw_unique_transcripts,raw_edits,merged_edits,reference_words,merged_words,segment_count -AutoSize
if ($ShowTexts) { $rows | Format-List id,raw_text,merged_text }
Write-Output ("DEV_TOTAL raw_host_edits="+$rawTotal+" merged_cli_edits="+$mergedTotal+" reference_words="+$refTotal)
