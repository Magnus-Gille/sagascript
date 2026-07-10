# Releasing Sagascript

Production macOS releases must be signed with a **Developer ID Application**
certificate, use hardened runtime, and be notarized and stapled. The release
workflow refuses to publish an unsigned or unverifiable macOS artifact.

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
     `Developer ID Application: … (U7MYD3Z5CB)` name. Release verification is
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
   `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.
2. Run `npm run release:check`, `npm run check`,
   `cargo test --workspace`, and
   `cargo clippy --workspace --all-targets -- -D warnings`.
3. Merge the release commit to `main`, then create and push exactly `vVERSION`.
4. The Release workflow gates both platform builds on tests, checks version/tag
   consistency, imports the Apple certificate into an ephemeral keychain, and
   lets Tauri sign, notarize, and staple the universal macOS build.
5. The workflow independently verifies the Developer ID authority, Team ID,
   hardened-runtime flag, audio-input entitlement, notarization tickets,
   Gatekeeper acceptance, and bundle metadata before creating a draft release.
6. Download the draft artifacts and perform the clean-machine checklist below.
   Publish the draft only after it passes.

## Clean-machine acceptance checklist

- Download the DMG through a browser on a Mac that has never run Sagascript.
- Install to `/Applications`; confirm Gatekeeper opens it without “Open Anyway”.
- Confirm onboarding, model download, microphone, Accessibility, global hotkey,
  dictation, auto-paste, and quit/relaunch behavior.
- Confirm the app does not request the same permission again after relaunch.
- Test both Apple Silicon and Intel hardware before claiming universal support.
- Test the Windows installers on a clean Windows 10 and Windows 11 VM. Windows
  artifacts remain unsigned until a separate Windows signing identity is added.

## macOS permission identity migration

Pre-launch builds used `com.sagascript.app`; production uses
`ai.gille.sagascript` (a reverse-DNS identity under the project owner's domain).
Settings are copied automatically on first launch, but
macOS permissions intentionally are not transferable between bundle identities.
This requires a one-time re-approval. Follow the reset procedure in
`CONTRIBUTING.md`; do not repeatedly run differently signed copies from build
directories and `/Applications` during the same permission test.
