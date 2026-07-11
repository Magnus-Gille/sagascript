# Windows-Specific Notes

Windows support is a build-from-source preview. Sagascript v1 publishes signed
and notarized macOS artifacts only; the project does not publish an unsigned
Windows installer. CI coverage is useful development evidence, not a promise
of production support.

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
| Installer format | Official signed `.dmg` | Local source build: `.exe` (NSIS) / `.msi` |

## Known limitations

- **CPU-only transcription.** GPU acceleration (Metal/Core ML) is not available on Windows. Large models (`large`, `large-v3`) will be significantly slower than on macOS with Metal. We recommend using `base` or `small` models on Windows.
- **No official binary or auto-updater.** Build the current preview from source.
- **ARM64 not tested.** Snapdragon / Copilot+ PCs (ARM64) have not been tested yet. The app is currently x86_64 only.

## Troubleshooting

### "Windows protected your PC" (SmartScreen warning)

Do not bypass SmartScreen for a Sagascript installer downloaded from another
party. The project does not publish Windows binaries for v1. Inspect the source
and build the preview locally instead.

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
