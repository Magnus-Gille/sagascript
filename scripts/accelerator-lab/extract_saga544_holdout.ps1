[CmdletBinding()]
param(
    [string]$ArchivePath = (Join-Path $env:TEMP 'sagascript-accelerator-lab\fleurs-sv-dev.tar.gz')
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$manifestPath = Join-Path $repoRoot 'analysis\saga544\frozen-split.json'
$outputRoot = Join-Path $repoRoot 'analysis\saga544\audio'
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$clips = @($manifest.holdout)

if (-not (Test-Path -LiteralPath $ArchivePath -PathType Leaf)) {
    throw "FLEURS archive was not found."
}
if ($clips.Count -eq 0) {
    throw 'The frozen holdout manifest has no clips.'
}

$members = [System.Collections.Generic.List[string]]::new()
foreach ($clip in $clips) {
    if ($clip.filename -notmatch '^\d+\.wav$' -or
        $clip.archive_member -ne ('dev/' + $clip.filename)) {
        throw 'The frozen holdout manifest contains an invalid archive member.'
    }
    $members.Add([string]$clip.archive_member)
}

New-Item -ItemType Directory -Path $outputRoot -Force | Out-Null
$targets = @($clips | ForEach-Object {
    Join-Path $outputRoot ($_.archive_member -replace '/', '\')
})
$existing = @($targets | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf })
if ($existing.Count -ne 0 -and $existing.Count -ne $targets.Count) {
    throw 'Some holdout clips already exist; refusing a partial overwrite.'
}

if ($existing.Count -eq 0) {
    $arguments = @('-xzf', $ArchivePath, '-C', $outputRoot, '--') + @($members)
    & tar @arguments
    if ($LASTEXITCODE -ne 0) {
        throw 'tar failed while extracting the selected public FLEURS clips.'
    }
}

foreach ($clip in $clips) {
    $path = Join-Path $outputRoot ($clip.archive_member -replace '/', '\')
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw 'A selected holdout clip is missing after extraction.'
    }
    $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($clip.sha256 -and $clip.sha256 -ne $hash) {
        throw 'An extracted holdout clip does not match its frozen SHA-256.'
    }
    $clip.sha256 = $hash
    [pscustomobject]@{
        id = $clip.id
        file = $path
        duration_seconds = $clip.duration_seconds
        sha256 = $hash
    }
}

Set-Content -LiteralPath $manifestPath -Value ($manifest | ConvertTo-Json -Depth 8) -Encoding utf8NoBOM
