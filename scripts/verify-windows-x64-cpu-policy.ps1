#!/usr/bin/env pwsh

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$TargetRoot,

    [Parameter(Mandatory = $true)]
    [string]$PolicyFile
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Normalize-PolicyPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    try {
        $fullPath = [System.IO.Path]::GetFullPath($Path)
    } catch {
        throw "CMAKE_PROJECT_INCLUDE is not a valid path: $Path"
    }
    $normalized = $fullPath.Replace('\', '/')
    $_isWindowsHost = [System.Environment]::OSVersion.Platform -eq [System.PlatformID]::Win32NT
    if ($_isWindowsHost) {
        return $normalized.TrimEnd('/').ToLowerInvariant()
    }
    return $normalized.TrimEnd('/')
}

function Read-CMakeCache {
    param(
        [Parameter(Mandatory = $true)]
        [System.IO.FileInfo]$CacheFile
    )

    $entries = @{}
    $lineNumber = 0
    foreach ($line in [System.IO.File]::ReadAllLines($CacheFile.FullName)) {
        $lineNumber++
        if ([string]::IsNullOrEmpty($line) -or $line.StartsWith('//') -or $line.StartsWith('#')) {
            continue
        }

        $match = [regex]::Match($line, '^(?<name>[^:=\r\n]+):(?<type>[^=\r\n]+)=(?<value>.*)$')
        if (-not $match.Success) {
            throw "Malformed CMake cache entry in '$($CacheFile.FullName)' at line $lineNumber"
        }

        $name = $match.Groups['name'].Value
        if ($entries.ContainsKey($name)) {
            throw "Duplicate CMake cache entry '$name' in '$($CacheFile.FullName)'"
        }
        $entries[$name] = [pscustomobject]@{
            Type = $match.Groups['type'].Value
            Value = $match.Groups['value'].Value
        }
    }
    return $entries
}

function Require-CacheEntry {
    param(
        [Parameter(Mandatory = $true)]
        [hashtable]$Entries,

        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $true)]
        [string]$CachePath
    )

    if (-not $Entries.ContainsKey($Name)) {
        throw "Missing required CMake cache entry '$Name' in '$CachePath'"
    }
    return $Entries[$Name]
}

function Assert-CacheValue {
    param(
        [Parameter(Mandatory = $true)]
        [hashtable]$Entries,

        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $true)]
        [string]$Expected,

        [Parameter(Mandatory = $true)]
        [string]$CachePath
    )

    $entry = Require-CacheEntry -Entries $Entries -Name $Name -CachePath $CachePath
    if ($entry.Value -cne $Expected) {
        throw "CMake cache '$CachePath' has $Name='$($entry.Value)', expected '$Expected'"
    }
}

$targetItem = Get-Item -LiteralPath $TargetRoot -ErrorAction Stop
if (-not $targetItem.PSIsContainer) {
    throw "TargetRoot is not a directory: $TargetRoot"
}
$policyItem = Get-Item -LiteralPath $PolicyFile -ErrorAction Stop
if ($policyItem.PSIsContainer) {
    throw "PolicyFile is not a file: $PolicyFile"
}

$targetRootPath = [System.IO.Path]::GetFullPath($targetItem.FullName)
$expectedPolicyPath = Normalize-PolicyPath -Path $policyItem.FullName
$cacheFiles = @(Get-ChildItem -LiteralPath $targetRootPath -Filter 'CMakeCache.txt' -File -Recurse -ErrorAction Stop)
$selected = @()

foreach ($cacheFile in $cacheFiles) {
    $hasWhisperProject = $false
    foreach ($line in [System.IO.File]::ReadAllLines($cacheFile.FullName)) {
        if ($line -ceq 'CMAKE_PROJECT_NAME:STATIC=whisper.cpp') {
            $hasWhisperProject = $true
            break
        }
    }
    if (-not $hasWhisperProject) {
        continue
    }

    $entries = Read-CMakeCache -CacheFile $cacheFile
    if (-not $entries.ContainsKey('CMAKE_PROJECT_NAME')) {
        throw "Selected cache is missing CMAKE_PROJECT_NAME: '$($cacheFile.FullName)'"
    }
    $project = $entries['CMAKE_PROJECT_NAME']
    if ($project.Type -cne 'STATIC' -or $project.Value -cne 'whisper.cpp') {
        continue
    }
    $selected += [pscustomobject]@{
        Path = $cacheFile.FullName
        Entries = $entries
    }
}

if ($selected.Count -eq 0) {
    throw "No CMakeCache.txt with CMAKE_PROJECT_NAME:STATIC=whisper.cpp found under '$targetRootPath'"
}

$requiredOff = @(
    'GGML_NATIVE',
    'GGML_AVX_VNNI',
    'GGML_AVX512',
    'GGML_AVX512_VBMI',
    'GGML_AVX512_VNNI',
    'GGML_AVX512_BF16',
    'GGML_AMX_TILE',
    'GGML_AMX_INT8',
    'GGML_AMX_BF16',
    'GGML_CPU_ALL_VARIANTS',
    'GGML_BACKEND_DL',
    'GGML_LLAMAFILE'
)
$requiredOn = @('GGML_AVX', 'GGML_SSE42', 'GGML_AVX2', 'GGML_BMI2', 'GGML_FMA', 'GGML_F16C')

foreach ($candidate in $selected) {
    $entries = $candidate.Entries
    $include = Require-CacheEntry -Entries $entries -Name 'CMAKE_PROJECT_INCLUDE' -CachePath $candidate.Path
    if ((Normalize-PolicyPath -Path $include.Value) -cne $expectedPolicyPath) {
        throw "CMake cache '$($candidate.Path)' has CMAKE_PROJECT_INCLUDE='$($include.Value)', expected '$($policyItem.FullName)'"
    }
    foreach ($name in $requiredOff) {
        Assert-CacheValue -Entries $entries -Name $name -Expected 'OFF' -CachePath $candidate.Path
    }
    foreach ($name in $requiredOn) {
        Assert-CacheValue -Entries $entries -Name $name -Expected 'ON' -CachePath $candidate.Path
    }
    Write-Host "Policy pass: $($candidate.Path)"
}

Write-Host "Verified $($selected.Count) whisper.cpp CMake cache(s) under '$targetRootPath'"
