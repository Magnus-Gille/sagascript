# Installing Sagascript

## macOS

Download **Sagascript.dmg** from the [latest release](https://github.com/Magnus-Gille/sagascript/releases/latest). This is a universal binary that runs natively on both Apple Silicon and Intel Macs.

Open the DMG, drag Sagascript to Applications, and launch the copy in
`/Applications`. Official releases are signed with Developer ID and notarized by
Apple; do not bypass Gatekeeper with `xattr` or “Open Anyway”. If an official
artifact is blocked, do not run it—report the release version and download URL.

## Windows

Download **Sagascript-Setup.exe** from the [latest release](https://github.com/Magnus-Gille/sagascript/releases/latest).

### SmartScreen warning

Sagascript is not yet signed with a Windows code-signing certificate. Windows Defender SmartScreen will show a warning on first run.

**To proceed:**

1. Run the installer — SmartScreen shows *"Windows protected your PC"*
2. Click **More info**
3. Click **Run anyway**

The warning only appears once per downloaded file.

## First launch

On first launch, Sagascript will walk you through setup:

1. **Language** — pick your primary dictation language (English, Swedish, or Norwegian)
2. **Speech engine download** — downloads the recommended Whisper model for your language (55-142 MB)
3. **Microphone permission** (macOS) — required for live dictation
4. **Accessibility permission** (macOS) — allows auto-paste into any app after dictation

Each permission should be requested once. The global hotkey itself does not
require an additional TCC grant. If an older pre-release build keeps reappearing in
Privacy & Security, remove the old rows and reinstall one fresh copy in
`/Applications`; see the repository's troubleshooting instructions.

All speech processing happens locally on your device. No audio is sent to any server.

## System requirements

- **macOS:** 13.0 (Ventura) or later, Apple Silicon or Intel
- **Windows:** 10 or later, WebView2 runtime (installed automatically)
