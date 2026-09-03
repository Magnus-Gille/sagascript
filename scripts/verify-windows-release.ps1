#!/usr/bin/env pwsh

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$AppExe,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedVersion,

    [Parameter(Mandatory = $true)]
    [string[]]$Artifacts,

    [ValidateSet("Internal", "Release")]
    [string]$SignaturePolicy = "Internal",

    [string]$ChecksumOutput = "SHA256SUMS-Windows"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-NonemptyFile {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [string]$Label
    )

    $item = Get-Item -LiteralPath $Path -ErrorAction Stop
    if ($item.PSIsContainer) {
        throw "$Label is not a file: $Path"
    }
    if ($item.Length -le 0) {
        throw "$Label is empty: $Path"
    }
    return $item.FullName
}

function Invoke-AppProbe {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Executable,

        [Parameter(Mandatory = $true)]
        [string]$Argument
    )

    $output = & $Executable $Argument 2>&1
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "'$Executable $Argument' exited with code $exitCode"
    }
    return ($output -join "`n")
}

if ($ExpectedVersion -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$') {
    throw "ExpectedVersion is not a semantic version: $ExpectedVersion"
}
if ($Artifacts.Count -eq 0) {
    throw "Artifacts must contain at least one file"
}
if (-not (Get-Command Get-AuthenticodeSignature -ErrorAction SilentlyContinue)) {
    throw "Get-AuthenticodeSignature is unavailable; run this verifier on Windows"
}

$appExePath = Resolve-NonemptyFile -Path $AppExe -Label "AppExe"
$pathSet = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
$nameSet = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
$artifactByName = @{}

foreach ($artifact in $Artifacts) {
    $resolved = Resolve-NonemptyFile -Path $artifact -Label "Artifact"
    if (-not $pathSet.Add($resolved)) {
        throw "Duplicate artifact path after resolution: $resolved"
    }

    $basename = [System.IO.Path]::GetFileName($resolved)
    if (-not $nameSet.Add($basename)) {
        throw "Duplicate artifact basename: $basename"
    }
    $artifactByName[$basename] = $resolved
}

if (-not $pathSet.Contains($appExePath)) {
    throw "AppExe must also appear in Artifacts: $appExePath"
}

$versionOutput = Invoke-AppProbe -Executable $appExePath -Argument "--version"
$versionPattern = "(?<![0-9.])$([regex]::Escape($ExpectedVersion))(?![0-9.])"
if ($versionOutput -notmatch $versionPattern) {
    throw "Version output does not contain exact version '$ExpectedVersion': $versionOutput"
}
[void](Invoke-AppProbe -Executable $appExePath -Argument "--help")
Write-Host "Executable probes passed for Sagascript $ExpectedVersion"

foreach ($basename in $artifactByName.Keys) {
    $artifactPath = $artifactByName[$basename]
    $signature = Get-AuthenticodeSignature -FilePath $artifactPath -ErrorAction Stop
    $status = $signature.Status.ToString()
    $valid = $status -eq "Valid"
    $allowedInternal = $valid -or $status -eq "NotSigned"

    if ($SignaturePolicy -eq "Release" -and -not $valid) {
        throw "Release artifact '$basename' does not have a valid Authenticode signature (status: $status)"
    }
    if ($SignaturePolicy -eq "Internal" -and -not $allowedInternal) {
        throw "Internal artifact '$basename' has an unsafe signature state: $status"
    }
}
Write-Host "Authenticode checks passed under $SignaturePolicy policy"

$sortedNames = [string[]]$artifactByName.Keys
[Array]::Sort($sortedNames, [System.StringComparer]::OrdinalIgnoreCase)
$checksumLines = foreach ($basename in $sortedNames) {
    $hash = (Get-FileHash -LiteralPath $artifactByName[$basename] -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $basename"
}

if ([System.IO.Path]::IsPathRooted($ChecksumOutput)) {
    $checksumPath = [System.IO.Path]::GetFullPath($ChecksumOutput)
} else {
    $checksumPath = [System.IO.Path]::GetFullPath((Join-Path (Get-Location).Path $ChecksumOutput))
}
$checksumParent = [System.IO.Path]::GetDirectoryName($checksumPath)
if (-not [System.IO.Directory]::Exists($checksumParent)) {
    throw "Checksum output directory does not exist: $checksumParent"
}

# Windows PowerShell 5.1's UTF8 encoding adds a BOM, so use .NET directly.
$utf8WithoutBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText(
    $checksumPath,
    (($checksumLines -join "`n") + "`n"),
    $utf8WithoutBom
)

Write-Host "Verified $($Artifacts.Count) Windows artifacts"
Write-Host "Checksums: $checksumPath"
