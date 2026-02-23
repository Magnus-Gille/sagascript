# Windows-Specific Notes

## Differences from macOS

| Feature | macOS | Windows |
|---|---|---|
| Transcription backend | Metal + Core ML (GPU) | CPU only (CUDA planned) |
| Permissions required | Microphone, Accessibility, Input Monitoring | Microphone only |
| Tray behavior | Menu bar icon | System tray icon |
| Default hotkey | Ctrl+Shift+Space | Ctrl+Shift+Space |
| Paste shortcut | Cmd+V | Ctrl+V |
| Settings path | `~/Library/Application Support/com.sagascript.app/` | `%APPDATA%\com.sagascript.app\` |
| Log path | `~/Library/Logs/Sagascript/` | `%LOCALAPPDATA%\Sagascript\Logs\` |
| Model path | `~/.sagascript/models/` | `%USERPROFILE%\.sagascript\models\` |
| Installer format | `.dmg` | `.exe` (NSIS) / `.msi` |

## Known limitations

- **CPU-only transcription.** GPU acceleration (Metal/Core ML) is not available on Windows. Large models (`large`, `large-v3`) will be significantly slower than on macOS with Metal. We recommend using `base` or `small` models on Windows.
- **No auto-updater.** The Tauri auto-updater is not yet configured for Windows. Check the [Releases page](https://github.com/Magnus-Gille/sagascript/releases) for updates.
- **ARM64 not tested.** Snapdragon / Copilot+ PCs (ARM64) have not been tested yet. The app is currently x86_64 only.

## Troubleshooting

### "Windows protected your PC" (SmartScreen warning)

This appears because the application is not yet code-signed. To proceed:

1. Click **"More info"**
2. Click **"Run anyway"**

This warning will be removed once the app has a code signing certificate.

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
