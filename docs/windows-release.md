# Windows release track

The selected zero-cost option is a clearly labelled **unsigned Windows beta**
distributed from the GitHub prerelease
[`windows-beta-20260905`](https://github.com/Magnus-Gille/sagascript/releases/tag/windows-beta-20260905).
It contains the exact binaries produced by the accepted candidate run. This
is a public preview for Windows 11 on x64 and ARM64, not a signed or stable
release. Do not tell users to bypass SmartScreen.

## Zero-cost distribution decision

The selected zero-cost path is a GitHub prerelease linked from the product
website after publication readback. The beta keeps the unsigned status explicit
and provides architecture-qualified NSIS, MSI, portable, CLI, and checksum
artifacts. The Microsoft Store and MSIX remain optional future work for a
trusted installation and update path.

Future signing investigations are tracked separately in
[#190: SignPath Foundation](https://github.com/Magnus-Gille/sagascript/issues/190)
and [#191: Microsoft Store/MSIX](https://github.com/Magnus-Gille/sagascript/issues/191).
They do not block this unsigned beta.

References:

- [Choose a Windows distribution path](https://learn.microsoft.com/windows/apps/package-and-deploy/choose-distribution-path)
- [Publish a first Windows app](https://learn.microsoft.com/windows/apps/package-and-deploy/publish-first-app)
- [SmartScreen reputation for developers](https://learn.microsoft.com/windows/apps/package-and-deploy/smartscreen-reputation)

Direct distribution is limited to this clearly labelled beta. It must not be
described as an official, signed, stable, or fully accepted Windows release.

## Candidate workflow

`.github/workflows/windows-package.yml` runs the release metadata, license,
frontend, Rust, and real CPU-transcription gates on native `windows-latest`
(x64) and `windows-11-arm` (ARM64) runners. Each runner then
builds unsigned NSIS and MSI installers, a portable desktop executable, and a
separate console CLI executable. It uploads them as one short-lived GitHub
Actions artifact and never creates or modifies a GitHub Release. Its artifacts
are to be attached manually from [Actions run 33963645741](https://github.com/Magnus-Gille/sagascript/actions/runs/33963645741)
to the prerelease tag above. Windows GUI executables do not reliably expose
redirected console output, so the candidate keeps the desktop and CLI launch
surfaces explicit instead of pretending one file behaves identically in both
contexts.

The artifact names are architecture-qualified:
`Sagascript-Windows-<architecture>-Portable.exe`,
`Sagascript-Windows-<architecture>-CLI.exe`,
`Sagascript-Windows-<architecture>-Setup.exe`, and
`Sagascript-Windows-<architecture>.msi`, where `<architecture>` is `x64` or
`arm64`. The matching `SHA256SUMS-Windows-<architecture>` file covers exactly
those four files.

`scripts/verify-windows-release.ps1` validates the executable surface,
signature state, and deterministic checksums. `Internal` signature policy
allows only a valid signature or a genuinely unsigned artifact. The stable
public pipeline must use `Release`, which requires every executable artifact to
have a valid Authenticode signature; the beta does not satisfy that gate.

After installing an internally accepted candidate on the test PC, run the
bundled CLI acceptance helper from the extracted workflow artifact directory.
Pass the bundled public fixture and keep the JSON evidence beside the artifact
files:

```powershell
.\accept-windows-candidate.ps1 `
  -CliExe ".\Sagascript-Windows-<architecture>-CLI.exe" `
  -ExpectedVersion "1.1.3" `
  -Fixture ".\norwegian-short-3s.mp3" `
  -OutputPath ".\windows-acceptance.json"
```

The helper downloads the recommended Norwegian test engine, transcribes only
the bundled public fixture, and writes `windows-acceptance.json`. It records
machine and timing evidence but never records a private microphone sample or
stores transcript text. The installed path remains an acceptance finding until
it has been observed on Windows; do not add it to documentation or `PATH`
promises merely because an installer default was expected.

## x64 candidate CPU baseline and cache portability

New x64 candidates require AVX2, FMA, F16C and BMI2. ARM64 is unchanged.
The x64 job records processor identity and required CPU features, then injects
`scripts/cmake/windows-x64-portable.cmake` through `CMAKE_PROJECT_INCLUDE`.
This is a supported forwarding path in the pinned `whisper-rs-sys`; standalone
`GGML_*` environment variables are not forwarded, and the bundled deprecated
`WHISPER_NATIVE=OFF` alias does not disable native probing.

The hook forces the baseline and disables host-native, AVX-512, AVX-VNNI and
AMX instructions, all-CPU-variant builds, dynamic backends and LLAMAFILE.
The Rust cache namespace includes the workflow, policy and
verifier content hashes, preventing reuse of the older host-native cache.
`verify-windows-x64-cpu-policy.ps1` checks actual whisper.cpp CMake caches after
debug compilation, before release CLI inference, and after installer builds;
missing or mismatched configuration fails the job. The packaged core checks
the required CPU features before native model loading and gives an actionable
error on unsupported processors. Source builds without the package-policy
marker retain their own compiler configuration.

This correction follows candidate run `33999334679`, whose x64 CLI exited with
`0xC000001D` (illegal instruction) during Norwegian inference after restoring
a native cache. The preceding freshly built candidate passed. The old logs do
not identify the CPU features or resolved CMake configuration, so a particular
unsupported instruction or cache-origin cause is not proven. Require real
inference on both cold and reused-cache candidates before accepting this
correction; a static configuration check alone is not runtime proof. The
historical beta below has not been rebuilt by this source change.

## Beta verification record

The candidate uses app version `1.1.3` from full source revision
`56cf3420f7d81ac2c423bcfee6c8961de03fcfaf`. Both native job checkout logs for
run `33963645741` identify that pull-request merge ref; the run API's
`9d75ce806041641e7635f1ef01f1cb044fb8a5f0` is its PR-head parent, not the
checked-out merge revision. The published beta tag points to the merge
revision. The ARM64 app was installed and
uninstalled on a user Windows machine and Swedish and English dictation were
tested. The x64 candidate passed the automated CI gates, but GUI acceptance was
not performed on an x64 machine. The install test retained existing models and
settings, so it is not a clean-state acceptance.

## Initial support boundary

- Windows 11 on x64 and ARM64.
- CPU transcription using the recommended small language-specific engine.
- One installed desktop app with system-tray UI plus a beta CLI executable
  exposing the canonical commands. Installer/PATH integration remains a public-
  release decision.
- No GPU-acceleration promise in the first release.
- Windows 10 may work but is not supported by this beta.

## Remaining acceptance work

Record the Windows edition/build, CPU, RAM, microphone, and exact beta artifact
checksums. A clean-machine run is still required before calling the Windows
build stable. The existing install test retained models and settings and does
not replace that run.

### Installation and identity

- SmartScreen identifies the beta as unsigned; verify the architecture-specific
  `SHA256SUMS-Windows-<architecture>` file from the GitHub prerelease before
  deciding whether to run it.
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

## Stable-release gates

Before removing the candidate warning:

1. All automated Windows candidate checks pass without `continue-on-error`.
2. Clean-machine acceptance is recorded against the exact commit and hashes.
3. Every executable in the stable release has a valid Authenticode signature;
   use the `Release` verification policy.
4. The signed installer passes clean-machine installation, upgrade, uninstall,
   dictation, CLI, and recovery checks.
5. If the Microsoft Store path is chosen, the accepted installer is converted to
   MSIX, passes Store flighting/certification, and the website link is updated
   only after the listing is read back.

Creating the Store product identity is an owner action. Do not store Microsoft
passwords, session material, recovery codes, or signing credentials in this
repository. The MSIX manifest's Store-assigned identity values must be copied
from the exact Partner Center product record when that record exists; do not
guess them in advance.
