# Windows release track

Sagascript's Windows build is a release candidate, not a public release. The
candidate workflow intentionally produces **unsigned** x64 and ARM64 artifacts for
testing on an owner-controlled Windows PC. Do not publish those artifacts or
tell users to bypass SmartScreen.

## Zero-cost distribution decision

The intended public path is a Microsoft Store MSIX submission. Microsoft signs
Store-distributed MSIX packages and provides the trusted installation and
update path without a separate code-signing subscription. Tauri currently
produces NSIS and MSI installers for Sagascript; MSIX packaging is a separate
follow-up after the native Windows behavior passes acceptance.

References:

- [Choose a Windows distribution path](https://learn.microsoft.com/windows/apps/package-and-deploy/choose-distribution-path)
- [Publish a first Windows app](https://learn.microsoft.com/windows/apps/package-and-deploy/publish-first-app)
- [SmartScreen reputation for developers](https://learn.microsoft.com/windows/apps/package-and-deploy/smartscreen-reputation)

Direct website/GitHub distribution remains blocked for a normal public release
while the installers are unsigned. An unsigned build may be used only as a
clearly labelled internal candidate on a known test machine.

## Candidate workflow

`.github/workflows/windows-package.yml` runs the release metadata, license,
frontend, Rust, and real CPU-transcription gates on native `windows-latest`
(x64) and `windows-11-arm` (ARM64) runners. Each runner then
builds unsigned NSIS and MSI installers, a portable desktop executable, and a
separate console CLI executable. It uploads them as one short-lived GitHub
Actions artifact and never creates or modifies a GitHub Release. Windows GUI
executables do not reliably expose redirected console output, so the candidate
keeps the desktop and CLI launch surfaces explicit instead of pretending one
file behaves identically in both contexts.

The artifact names are architecture-qualified:
`Sagascript-Windows-<architecture>-Portable.exe`,
`Sagascript-Windows-<architecture>-CLI.exe`,
`Sagascript-Windows-<architecture>-Setup.exe`, and
`Sagascript-Windows-<architecture>.msi`, where `<architecture>` is `x64` or
`arm64`. The matching `SHA256SUMS-Windows-<architecture>` file covers exactly
those four files.

`scripts/verify-windows-release.ps1` validates the executable surface,
signature state, and deterministic checksums. `Internal` signature policy
allows only a valid signature or a genuinely unsigned artifact. The future
public pipeline must use `Release`, which requires every executable artifact to
have a valid Authenticode signature.

After installing an internally accepted candidate on the test PC, run the
bundled CLI acceptance helper against the CLI executable from the same workflow
artifact:

```powershell
.\accept-windows-candidate.ps1 `
  -CliExe ".\Sagascript-Windows-<architecture>-CLI.exe" `
  -ExpectedVersion "1.1.3"
```

The helper downloads the recommended Norwegian test engine, transcribes only
the bundled public fixture, and writes `windows-acceptance.json`. It records
machine and timing evidence but never records a private microphone sample or
stores transcript text. The installed path remains an acceptance finding until
it has been observed on Windows; do not add it to documentation or `PATH`
promises merely because an installer default was expected.

## Initial support boundary

- Windows 11 on x64 and ARM64.
- CPU transcription using the recommended small language-specific engine.
- One installed desktop app with system-tray UI plus a candidate CLI executable
  exposing the canonical commands. Installer/PATH integration remains a public-
  release decision.
- No GPU-acceleration promise in the first release.
- Windows 10 may work but is not release-supported until separately accepted.

## Clean-machine acceptance

Record the Windows edition/build, CPU, RAM, microphone, and exact candidate
artifact checksums. Start from a machine with no prior Sagascript installation,
settings, model directory, or running process.

### Installation and identity

- SmartScreen identifies the candidate as unsigned; test only after independently
  matching the architecture-specific `SHA256SUMS-Windows-<architecture>` file
  from the workflow artifact.
- NSIS installation completes without development tools.
- MSI installation completes without development tools.
- Only one installer format is selected for the eventual Store/MSIX conversion.
- Start-menu entry, application icon, publisher placeholder, version, install
  location, and uninstall entry are coherent.
- Uninstall removes program files and shortcuts. Record whether user settings
  and downloaded models are retained.

### First run and daily dictation

- First launch explains local processing and downloads exactly one recommended
  speech engine for English, Swedish, or Norwegian.
- Download progress, checksum failure, retry, cancellation, and low-disk/network
  failure states are recoverable.
- The S icon is visible in the Windows system tray in idle state.
- `Control+Shift+Space` starts/stops push-to-talk without opening or focusing the
  settings window.
- Recording, loading, transcribing, and error states remain understandable in
  the tray/indicator.
- Microphone denial or the Windows desktop-microphone privacy switch produces an
  actionable recovery message.
- Auto-paste works in Notepad, Word, and a browser text field. Clipboard fallback
  preserves the transcript when simulated paste is blocked.
- A normal, non-elevated Sagascript process must not claim it can paste into an
  elevated target process.
- Two language profiles and two shortcuts can be used without restart.
- Closing settings leaves the tray process running; Quit exits every process.
- Sagascript never opens its main window merely because the first dictation ran.

### Accuracy, latency, and resilience

- Complete at least ten short utterances in each of English, Swedish, and
  Norwegian, including two consecutive transcriptions.
- Record cold and warm key-release-to-paste latency for each language.
- A 60-second recording finishes without UI lockup or a permanently busy state.
- Interruptions, no default microphone, sleep/wake, audio-device changes, and a
  second launch fail safely.
- CPU usage, memory, fan noise, and battery impact are acceptable with the
  recommended model; larger models remain Advanced/unsupported if they are not.

### CLI, updates, and upgrade

- `Sagascript-Windows-<architecture>-CLI.exe --version` reports the candidate version and
  source revision.
- `--help`, `list-models`, `config`, file transcription, microphone recording,
  JSON output, and PowerShell completions work from the candidate CLI binary.
- Decide and document how the CLI is installed and whether it is added to
  `PATH`; do not
  imply a bare `sagascript` command works until this is verified.
- Check for Updates reports checking/current/available/error and opens the exact
  stable release page without claiming it installed anything.
- An upgrade candidate preserves settings, profiles, glossary, and downloaded
  engines and does not create duplicate startup/tray entries.

## Public-release gates

Before removing the candidate warning:

1. All automated Windows candidate checks pass without `continue-on-error`.
2. Clean-machine acceptance is recorded against the exact commit and hashes.
3. A Microsoft Store account and product identity are ready at no recurring
   signing cost.
4. The accepted installer is converted to MSIX and tested through Store flighting.
5. Store certification passes and the Store-delivered package is installed on a
   clean machine.
6. The website Windows button points to the Store listing only after readback.

Creating the Store product identity is an owner action. Do not store Microsoft
passwords, session material, recovery codes, or signing credentials in this
repository. The MSIX manifest's Store-assigned identity values must be copied
from the exact Partner Center product record when that record exists; do not
guess them in advance.
