# Installing Sagascript

## macOS

Download **Sagascript.dmg** from the [latest release](https://github.com/Magnus-Gille/sagascript/releases/latest).
The v1 artifact is an Apple Silicon binary. Intel Macs are not supported by the
v1 binary release.

Open the DMG, drag Sagascript to Applications, and launch the copy in
`/Applications`. Official releases are signed with Developer ID and notarized by
Apple; do not bypass Gatekeeper with `xattr` or “Open Anyway”. If an official
artifact is blocked, do not run it—report the release version and download URL.

## Windows

Use the [Windows beta prerelease](https://github.com/Magnus-Gille/sagascript/releases/tag/windows-beta-20260905).
It is an unsigned preview for Windows 11 on x64 and ARM64, not a signed or
stable release. Choose `Sagascript-Windows-x64-Setup.exe` for Intel/AMD PCs or
`Sagascript-Windows-arm64-Setup.exe` for native ARM64 PCs. Verify the matching
`SHA256SUMS-Windows-<architecture>` file before running an installer, and do
not bypass SmartScreen.

The prerelease also includes MSI, portable desktop, and CLI files for each
architecture. The CLI is a separate console executable; it will not be
automatically added to `PATH` by this beta.

The NSIS installer and MSI also install the console CLI as `sagascript-cli.exe` in
the install directory, next to `sagascript.exe` (and, on ARM64, next to `engine-host\`,
so Pianissimo works). The installers do not add the directory to `PATH`; call the CLI by
full path, for example `& "$env:LOCALAPPDATA\Sagascript\sagascript-cli.exe" engine doctor`
(adjust for the directory you chose; MSI defaults differ). On ARM64,
`Sagascript-Windows-arm64-Portable.zip` holds `sagascript.exe`, `sagascript-cli.exe` and
`engine-host\` for use without installing; extract it and run `.\sagascript-cli.exe`. The
standalone `-CLI.exe` and `-Portable.exe` files are single files without `engine-host\`
(Whisper models only).

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

- **macOS:** 13.0 (Ventura) or later on Apple Silicon
- **Windows beta:** Windows 11 on x64 or ARM64
