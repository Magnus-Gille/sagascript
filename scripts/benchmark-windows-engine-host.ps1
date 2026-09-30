#!/usr/bin/env pwsh
# Warm-host latency probe for the ONNX engine host. Speaks the engine-host protocol directly:
# loads the model once, then transcribes a ~5 s and a full-length clip twice each, printing
# per-stage host timings (preprocess/encode/decode) and the client-observed round trip.
# The first pass of each clip includes ONNX Runtime first-run costs; the second is the warm number.
# Repeats for several ONNX Runtime tuning configurations (SAGASCRIPT_ORT_* variables, see
# src-tauri/engine-host/ort/src/ort_engine.rs) so one run compares them on the same machine.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$HostDir,
    [Parameter(Mandatory = $true)][string]$ModelDir,
    [string]$Wav = 'test-audio/swedish-fleurs-hongkong.wav',
    [string]$ModelId = 'pianissimo-sv-onnx-bench'
)
$ErrorActionPreference = 'Stop'

function Read-WavAsFloat([string]$path) {
    $b = [System.IO.File]::ReadAllBytes($path)
    if ([System.Text.Encoding]::ASCII.GetString($b, 0, 4) -ne 'RIFF') { throw "$path is not a RIFF WAV" }
    $pos = 12; $rate = 0; $ch = 0; $bits = 0
    while ($pos + 8 -le $b.Length) {
        $id = [System.Text.Encoding]::ASCII.GetString($b, $pos, 4)
        $len = [BitConverter]::ToUInt32($b, $pos + 4)
        if ($id -eq 'fmt ') {
            $ch = [BitConverter]::ToUInt16($b, $pos + 10); $rate = [BitConverter]::ToUInt32($b, $pos + 12)
            $bits = [BitConverter]::ToUInt16($b, $pos + 22)
        } elseif ($id -eq 'data') {
            if ($rate -ne 16000 -or $ch -ne 1 -or $bits -ne 16) { throw "Need 16 kHz mono 16-bit WAV, got $rate Hz $ch ch $bits bit" }
            $n = [int][Math]::Min($len, $b.Length - $pos - 8) / 2
            $out = New-Object byte[] ($n * 4)
            for ($i = 0; $i -lt $n; $i++) {
                $f = [BitConverter]::ToInt16($b, $pos + 8 + 2 * $i) / 32768.0
                [BitConverter]::GetBytes([single]$f).CopyTo($out, 4 * $i)
            }
            return , $out
        }
        $pos += 8 + $len + ($len % 2)
    }
    throw "No data chunk in $path"
}

$hostExe = Join-Path $HostDir 'sagascript-engine-host-ort.exe'
$pcm = Join-Path ([System.IO.Path]::GetTempPath()) "engine-bench-$PID.f32"
$bytes = Read-WavAsFloat $Wav
[System.IO.File]::WriteAllBytes($pcm, $bytes)
$fullSamples = [int64]($bytes.Length / 4)
$clips = @(@{ name = '5s'; samples = [int64][Math]::Min(80000, $fullSamples) }, @{ name = ('{0:N1}s' -f ($fullSamples / 16000.0)); samples = $fullSamples })
$configs = @(
    @{ name = 'default'; env = @{} },
    @{ name = 'dec-threads=1'; env = @{ SAGASCRIPT_ORT_DEC_THREADS = '1' } },
    @{ name = 'arena=1'; env = @{ SAGASCRIPT_ORT_ARENA = '1' } },
    @{ name = 'threads=4'; env = @{ SAGASCRIPT_ORT_THREADS = '4' } },
    @{ name = 'threads=all'; env = @{ SAGASCRIPT_ORT_THREADS = [string][Environment]::ProcessorCount } }
)

function Invoke-Config($config) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $hostExe; $psi.Arguments = '--protocol 1'; $psi.WorkingDirectory = $HostDir
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $psi.EnvironmentVariables['SAGASCRIPT_ORT_TRACE'] = '1'
    foreach ($k in $config.env.Keys) { $psi.EnvironmentVariables[$k] = $config.env[$k] }
    $p = [System.Diagnostics.Process]::Start($psi)
    $err = $p.StandardError.ReadToEndAsync()
    $script:nextId = 0
    function Send($op, $fields) {
        $script:nextId++
        $msg = [ordered]@{ v = 1; id = $script:nextId; op = $op } + $fields
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $p.StandardInput.WriteLine(($msg | ConvertTo-Json -Compress -Depth 5)); $p.StandardInput.Flush()
        while ($true) {
            $t = $p.StandardOutput.ReadLineAsync()
            if (-not $t.Wait(180000)) { throw "Timed out waiting for $op" }
            if ($null -eq $t.Result) { throw "Host exited during $op" }
            if ([string]::IsNullOrWhiteSpace($t.Result)) { continue }
            $m = $t.Result | ConvertFrom-Json
            if ($m.id -eq $script:nextId -and $null -ne $m.ok) {
                if (-not $m.ok) { throw "$op failed: $($t.Result)" }
                return @{ msg = $m; ms = $sw.Elapsed.TotalMilliseconds }
            }
        }
    }
    try {
        [void](Send 'hello' @{ client = @{ name = 'engine-bench'; version = '0'; git_sha = ('0' * 40) } })
        $load = Send 'load' @{ model_dir = (Resolve-Path $ModelDir).Path; model_id = $ModelId; compute_units = 'cpu' }
        Write-Output ("[{0}] load: {1:N0} ms (host load_ms={2})" -f $config.name, $load.ms, $load.msg.load_ms)
        foreach ($clip in $clips) {
            foreach ($pass in 1, 2) {
                $r = Send 'transcribe_window' @{ pcm_path = $pcm; offset_samples = 0; num_samples = $clip.samples; sample_rate = 16000; format = 'f32le'; priority = 'interactive' }
                $t = $r.msg.timings
                Write-Output ("[{0}] clip={1} pass={2} round_trip={3:N0} ms  pre={4} enc={5} dec={6}  tokens={7}" -f `
                    $config.name, $clip.name, $pass, $r.ms, $t.preprocess_ms, $t.encode_ms, $t.decode_ms, @($r.msg.tokens).Count)
            }
        }
        [void](Send 'shutdown' @{})
        [void]$p.WaitForExit(15000)
    } finally {
        if (-not $p.HasExited) { $p.Kill() }
        if ($err.IsCompleted -and $err.Result) { $err.Result.TrimEnd().Split("`n") | ForEach-Object { Write-Output "    host: $_" } }
    }
}

try {
    Write-Output ("CPU logical processors: {0}; host: {1}" -f [Environment]::ProcessorCount, (& $hostExe --version))
    foreach ($config in $configs) { Invoke-Config $config }
} finally {
    Remove-Item -LiteralPath $pcm -Force -ErrorAction SilentlyContinue
}
