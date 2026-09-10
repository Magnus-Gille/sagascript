# Releasing Sagascript

Sagascript's stable v1 binary release is macOS-only. Windows has a separate
clearly labelled unsigned beta distributed from a GitHub prerelease; it must not
be described as signed, stable, or fully accepted. The automated Windows
candidate workflow remains non-publishing, and the macOS `v*` release workflow
must not be used for the Windows beta.

Production macOS releases must be signed with a **Developer ID Application**
certificate, use hardened runtime, and be notarized and stapled. The release
workflow refuses to publish an unsigned or unverifiable macOS artifact.
The production signing Team ID is **`7C6WF6GFZ4`**; the verifier rejects a
different Team ID even if the certificate is otherwise valid.

## Windows beta publication

The Windows beta tag is [`windows-beta-20260905`](https://github.com/Magnus-Gille/sagascript/releases/tag/windows-beta-20260905).
Publish it as a GitHub prerelease so it does not trigger the macOS `v*` release
workflow. It should contain the exact x64 and ARM64 artifacts from [Actions run
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

Windows beta publication is a separate owner action: attach only the exact
artifacts from the accepted candidate run to the prerelease tag above, mark it
as a prerelease, and verify the release page and checksums before linking it
from the product website. Do not change the candidate workflow into an
automatic publisher.

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
  and `SHA256SUMS`; the unsigned Windows beta remains a separate prerelease.
  Verify every downloaded artifact against its published checksum before
  testing.
- Review `THIRD_PARTY_NOTICES.md`. Run `npm run licenses:generate` and inspect
  any diff whenever either lockfile or a model source changes.

## macOS permission identity migration

Pre-launch builds used `com.sagascript.app`; production uses
`ai.gille.sagascript` (a reverse-DNS identity under the project owner's domain).
Settings are copied automatically on first launch, but
macOS permissions intentionally are not transferable between bundle identities.
This requires a one-time re-approval. Follow the reset procedure in
`CONTRIBUTING.md`; do not repeatedly run differently signed copies from build
directories and `/Applications` during the same permission test.
