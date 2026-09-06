# Windows-Specific Notes

Windows support has an unsigned beta preview. Download the exact binaries from the
[`windows-beta-20260905` GitHub prerelease](https://github.com/Magnus-Gille/sagascript/releases/tag/windows-beta-20260905)
for Windows 11 on x64 or ARM64. The beta is not signed or stable, and it is
not a promise of production support; verify its architecture-specific checksums
before use.

The active zero-cost release strategy, candidate workflow, and clean-machine
acceptance checklist live in [Windows release track](windows-release.md).

## Differences from macOS

| Feature | macOS | Windows |
|---|---|---|
| Transcription backend | Metal + Core ML (GPU) | CPU only (CUDA planned) |
| Permissions required | Microphone, Accessibility | Microphone only |
| Tray behavior | Menu bar icon | System tray icon |
| Default hotkey | Ctrl+Shift+Space | Ctrl+Shift+Space |
| Paste shortcut | Cmd+V | Ctrl+V |
| Settings path | `~/Library/Application Support/ai.gille.sagascript/` | `%APPDATA%\ai.gille.sagascript\` |
| Log path | `~/Library/Logs/Sagascript/` | `%LOCALAPPDATA%\Sagascript\Logs\` |
| Model path | `~/.sagascript/models/` | `%USERPROFILE%\.sagascript\models\` |
| Installer format | Official signed `.dmg` | Unsigned beta: `.exe` (NSIS) / `.msi` |

## Known limitations

- **CPU-only transcription.** GPU acceleration (Metal/Core ML) is not available on Windows. Large models (`large`, `large-v3`) will be significantly slower than on macOS with Metal. We recommend using `base` or `small` models on Windows.
- **No signed stable binary or auto-updater.** The beta is a manually published
  preview; build from source if you need to inspect or reproduce it.
- **Architecture-specific builds.** Use the native ARM64 candidate on Snapdragon / Copilot+ PCs and the x64 candidate on Intel or AMD Windows PCs. CPU transcription is tested natively on both architectures in the candidate workflow.
- **New x64 candidates require AVX2, FMA, F16C and BMI2.** The candidate
  workflow explicitly disables build-host-native, AVX-512, AVX-VNNI and AMX
  instructions and verifies the actual native build cache. Unsupported CPUs
  receive an actionable error before model loading; use a source build
  configured for older CPUs. This policy does not change ARM64 or retroactively
  rebuild the linked historical beta.
- **Clipboard restoration is text-only.** Sagascript restores previous plain text
  only while its temporary clipboard generation is still current. New user
  copies or clipboard managers that add formats suppress restoration. Images
  and custom formats are not preserved on Windows. The generation check and
  restore share one native clipboard-open transaction, so a new copy cannot
  interleave between them.
- **Late paste does not move focus.** Modifier release waits at most one second,
  within the two-second paste-completion budget. A timeout can still finish
  later; check the editor before retrying. Dictate retains recognized text,
  but does not automatically open and steal focus while paste is uncertain.

## Troubleshooting

### "Windows protected your PC" (SmartScreen warning)

The beta is intentionally unsigned and may trigger SmartScreen. Verify the
matching `SHA256SUMS-Windows-<architecture>` file from the GitHub prerelease
before deciding whether to run it. Do not bypass SmartScreen. If you need to
avoid an unsigned installer, inspect the source and build the preview locally.

### Microphone not working

1. Open **Windows Settings** (Win+I)
2. Go to **Privacy & Security** > **Microphone**
3. Ensure **"Microphone access"** is turned on
4. Ensure Sagascript is listed and allowed

### Hotkey not registering

Some hotkey combinations may conflict with other applications or Windows system shortcuts. If `Ctrl+Shift+Space` doesn't work:

1. Check for conflicts with other apps (e.g., input method editors, screen capture tools)
2. Change the hotkey in Sagascript Settings to a different combination
3. Try restarting the application

### Slow transcription

Windows builds currently use CPU-only inference. If transcription is too slow:

- Use a smaller model (`base.en` or `small.en` for English)
- Close CPU-intensive background applications
- GPU acceleration (CUDA) is planned for a future release
