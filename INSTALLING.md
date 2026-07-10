# Installing Sagascript

## macOS

Download **Sagascript.dmg** from the [latest release](https://github.com/Magnus-Gille/sagascript/releases/latest).
The artifact is a universal binary. Apple Silicon is the tested launch
platform; the Intel slice remains pending clean-machine hardware acceptance.

Open the DMG, drag Sagascript to Applications, and launch the copy in
`/Applications`. Official releases are signed with Developer ID and notarized by
Apple; do not bypass Gatekeeper with `xattr` or “Open Anyway”. If an official
artifact is blocked, do not run it—report the release version and download URL.

## Windows

Sagascript v1 does not publish Windows installers. Windows remains a
build-from-source preview. Do not bypass SmartScreen for an unsigned installer
downloaded from another party; inspect the source and build it locally.

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

Speech processing happens locally on your device. Audio and transcripts are not
uploaded. Network access is used when you choose to download a speech,
diarization, or VAD model.

## System requirements

- **macOS:** 13.0 (Ventura) or later; Apple Silicon tested at launch
- **Windows preview:** 10 or later, built from source
