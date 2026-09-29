# Testing Pianissimo on Windows on ARM (Snapdragon)

Pianissimo (Swedish file transcription) runs on Windows only on ARM64, through a separate
ONNX Runtime engine host (`engine-host\sagascript-engine-host-ort.exe` plus `onnxruntime.dll`
in the app install directory). The x64 package does not contain them. The model (about
660 MB) is not bundled; the app downloads it from Hugging Face on first use.

## Get the unsigned arm64 build

1. Open the pull request, then Actions, then the "Windows Package Candidate" run.
2. Download the artifact `windows-arm64-unsigned-candidate` (needs GitHub login; kept 14 days).
3. Unzip it. Use `Sagascript-Windows-arm64-Setup.exe` (or the `.msi`). The artifact also holds
   `windows-arm-pianissimo-test.ps1` and `SHA256SUMS-Windows-arm64`.

The build is unsigned. SmartScreen shows "Windows protected your PC": choose More info, then
Run anyway. Defender may scan the engine host on its first start, so the first run can be slow.
If Defender quarantines a file, restore it from Protection history rather than disabling
protection. Verify the download first: `Get-FileHash .\Sagascript-Windows-arm64-Setup.exe`
must match `SHA256SUMS-Windows-arm64`.

## Run the test

No admin rights needed. From PowerShell:

```powershell
Unblock-File .\windows-arm-pianissimo-test.ps1   # only if it was downloaded via a browser
powershell -ExecutionPolicy Bypass -File .\windows-arm-pianissimo-test.ps1
```

Options: `-Audio <wav>` (default: fetches `test-audio/swedish-fleurs-hongkong.wav` from GitHub
at `-Ref`, default `main`; use the PR branch name if the file is not on `main`),
`-LongAudio <file>` for a longer Swedish recording, `-Cli <path>` if `sagascript.exe` is
not found, `-Yes` to skip the download prompt.

The script prints OS, CPU and RAM, runs `sagascript --version`, `engine status --json`,
`download-model pianissimo-sv` (asks first), `engine doctor --json`, and transcribes the audio
twice (cold, then warm) with timings.

## What to send back

Both files from `%USERPROFILE%\sagascript-pianissimo-test\`:
`windows-arm-pianissimo-result.json` and `windows-arm-pianissimo-result.txt`. If a step
fails, also send the matching `*.stderr.txt` in that folder. The result contains machine
model, timings and a 200 character transcript preview of the test audio, but no credentials.

## CI pin

The arm64 workflow downloads ONNX Runtime 1.28.2 (`onnxruntime-win-arm64-1.28.2.zip`, from
the official microsoft/onnxruntime GitHub release) and verifies SHA-256
`a3ab2265e52d157ef1c4f4f82f66fc582ce12a780510aac240690ce194b58510`. It ships
`onnxruntime.dll` and `onnxruntime_providers_shared.dll` only.
