# Releasing Sagascript

For 1.2.0, macOS Apple silicon and Windows ARM64 (Snapdragon) may ship as
stable. Magnus confirmed the tested Windows machine is Snapdragon on 2026-09-08.
Windows x64 (Intel/AMD) stays on a clearly labelled beta prerelease until it has
its own manual acceptance. Record the architecture,
exact source revision, candidate run, and test results in the release evidence;
one architecture's acceptance does not cover the other.

Windows installers remain unsigned. Stable status does not imply code signing;
retain the SmartScreen notice. The Windows candidate workflow remains
non-publishing. The macOS `v*` workflow creates a draft; attach only verified
Windows artifacts built from that same approved release revision.

Production macOS releases must be signed with a **Developer ID Application**
certificate, use hardened runtime, and be notarized and stapled. The release
workflow refuses to publish an unsigned or unverifiable macOS artifact.
The production signing Team ID is **`7C6WF6GFZ4`**; the verifier rejects a
different Team ID even if the certificate is otherwise valid.

## Historical Windows beta (1.1.3)

The Windows beta tag is [`windows-beta-20260905`](https://github.com/Magnus-Gille/sagascript/releases/tag/windows-beta-20260905).
This historical candidate used a GitHub prerelease, separate from the macOS
`v*` workflow, with the x64 and ARM64 artifacts from [Actions run
33963645741](https://github.com/Magnus-Gille/sagascript/actions/runs/33963645741),
built from full source revision
`56cf3420f7d81ac2c423bcfee6c8961de03fcfaf` and reporting app version `1.1.3`.
The release page must retain the architecture-specific installers, portable and
CLI executables, and matching SHA256 files.

The ARM64 app was installed and uninstalled on a user Windows machine, with
Swedish and English dictation tested. The x64 candidate passed automated CI,
but GUI acceptance was not performed on an x64 machine. The installation test
retained existing models and settings, so it was not a clean-state test. Keep
these limitations visible in the prerelease notes and do not promote this
artifact to the stable release channel.

## One-time Apple setup (repository owner)

1. Join the paid Apple Developer Program. Only the Account Holder can create a
   Developer ID Application certificate.
2. Create and export a **Developer ID Application** certificate plus its private
   key as a password-protected `.p12` file.
3. In App Store Connect, create an API key with Developer access and download its
   `.p8` private key. Apple only allows this download once.
4. Add these GitHub Actions secrets (never commit their values):

   - `APPLE_CERTIFICATE`: base64-encoded `.p12`
   - `APPLE_CERTIFICATE_PASSWORD`: export password for the `.p12`
   - `APPLE_SIGNING_IDENTITY`: full
     `Developer ID Application: … (7C6WF6GFZ4)` name. Release verification is
     intentionally pinned to this production team so macOS TCC permissions
     survive upgrades.
   - `KEYCHAIN_PASSWORD`: random password for the ephemeral CI keychain
   - `APPLE_API_ISSUER`: App Store Connect issuer UUID
   - `APPLE_API_KEY`: App Store Connect key ID
   - `APPLE_API_PRIVATE_KEY_BASE64`: base64-encoded `.p8`

To test signing locally, install the certificate and private key in the login
keychain and set `APPLE_SIGNING_IDENTITY`. Set `APPLE_API_ISSUER`,
`APPLE_API_KEY`, and `APPLE_API_KEY_PATH` to notarize. Do not put credentials in
`.env` for CI or print them in build logs.

## Release procedure

1. Update the version in `package.json`, `package-lock.json`,
   `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, and
   `src-tauri/tauri.conf.json`.
2. Run `npm run release:check`, `npm run licenses:check`, `npm run check`,
   `cargo test --workspace`, and
   `cargo clippy --workspace --all-targets -- -D warnings`.
3. Merge the release commit to `main`, then create and push exactly `vVERSION`.
4. The Release workflow gates the macOS build on tests, checks version/tag
   consistency, imports the Apple certificate into an ephemeral keychain, and
   lets Tauri sign, notarize, and staple the Apple Silicon macOS build.
5. The workflow independently verifies the Developer ID authority, Team ID,
   hardened-runtime flag, audio-input entitlement, notarization tickets,
   Gatekeeper acceptance, and bundle metadata before creating a draft release.
6. Download the draft artifacts and perform the clean-machine checklist below.
   Publish the draft only after it passes.

For 1.2.0, use [the release notes and publication checklist](docs/releases/1.2.0.md).
Build fresh Windows candidates from the approved 1.2.0 revision; do not relabel
1.1.3 test binaries. Attaching the accepted architecture to the stable draft is
an explicit owner publication action. Keep the unaccepted architecture on a
separate beta prerelease named `windows-beta-1.2.0` with matching checksums and
build identity. Do not reuse the historical 1.1.3 tag. Attach the Windows
workflow’s `SHA256SUMS-Windows-<architecture>` alongside its artifacts; the
macOS `SHA256SUMS` covers only the DMG and app archive. The
candidate workflow must not become an automatic publisher.

The new meeting review workflow is off in default release builds. The optional
`meeting-mode` Cargo feature is for future development and must not be enabled
for 1.2.0. Do not build release artifacts with `--all-features`. The desktop frontend retains ordinary file transcription/diarization;
its file-import branch into meeting review is disabled. Meeting document CLI commands and starting
new GUI meeting jobs are unavailable in default builds.

Publish GitHub notes from `docs/releases/1.2.0.md` after resolving and removing
the internal publication checklist. Deploy the matching website copy only after
the release and architecture-specific downloads are public and verified.

The macOS build job also simulates replacing an obsolete
`/Applications/Sagascript.app`, then runs a real Norwegian file transcription
through `/usr/local/bin/sagascript`. This protects the supported app-bundle CLI
link from silently continuing to execute a stale binary after an upgrade.

## Clean-machine acceptance checklist

- Download the DMG through a browser on a Mac that has never run Sagascript.
- Install to `/Applications`; confirm Gatekeeper opens it without “Open Anyway”.
- Confirm onboarding, model download, microphone, Accessibility, global hotkey,
  dictation, auto-paste, and quit/relaunch behavior.
- Confirm the app does not request the same permission again after relaunch.
- Confirm `sagascript --version` reports the release Git revision, and run one
  file transcription through `/usr/local/bin/sagascript` after upgrading an
  existing installation.
- Test the signed artifact on Apple Silicon. Do not claim Intel support for v1;
  the diarization runtime does not provide the required Intel macOS binary.
- Confirm the macOS draft contains `Sagascript.dmg`, `Sagascript.app.tar.gz`,
  and `SHA256SUMS`, plus the accepted Windows architecture and its checksums;
  the other Windows architecture remains a separate beta prerelease.
  Verify every downloaded artifact against its published checksum before
  testing.
- Review `THIRD_PARTY_NOTICES.md`. Run `npm run licenses:generate` and inspect
  any diff whenever either lockfile or a model source changes.

## Windows acceptance checklist

Record the physical machine's x64/ARM64 architecture, Windows version, source
revision and Actions run. Acceptance applies only to that architecture.

- Verify every installer against `SHA256SUMS-Windows-<architecture>`; in
  PowerShell use `Get-FileHash .\Sagascript-Windows-<architecture>-Setup.exe -Algorithm SHA256`.
- Record the unsigned installer / SmartScreen experience; follow device policy.
- Check fresh installation and upgrade, preserving existing profiles and models.
- Check microphone access, Swedish/English dictation, push-to-talk and toggle,
  auto-paste, file transcription with speaker diarization enabled, and relaunch.
- Check the displayed version and the separate CLI's version/build identity.
- Repeat a short smoke test on the final rebuilt 1.2.0 artifacts. Prior candidate
  acceptance does not establish that newly packaged installers work.

## macOS permission identity migration

Pre-launch builds used `com.sagascript.app`; production uses
`ai.gille.sagascript` (a reverse-DNS identity under the project owner's domain).
Settings are copied automatically on first launch, but
macOS permissions intentionally are not transferable between bundle identities.
This requires a one-time re-approval. Follow the reset procedure in
`CONTRIBUTING.md`; do not repeatedly run differently signed copies from build
directories and `/Applications` during the same permission test.
