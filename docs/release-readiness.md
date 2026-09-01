# Sagascript release readiness

This document defines the product boundary and acceptance gates for the next
public macOS release. The target experience is: install, grant two permissions,
choose a language, and dictate.

## Product decisions

- Keep local transcription as the default and make every remote behavior opt-in.
- Show one recommended speech model per explicit language. Download it when a
  profile first needs it. Keep manual model selection in **Advanced**.
- Make dictation profiles the primary language control. Each profile has a name,
  language, and global shortcut; users may add more than two profiles.
- Keep **Dictate**, **Transcribe**, and **Settings** as the visible app surfaces.
- Remove **Teach** from the release UI. Keep the reviewed glossary and CLI
  capabilities intact so the feature can return later without a data migration.
- Keep ordinary settings short. Manual model choice, decoder strategy,
  temperature fallback, and VAD belong in a collapsed **Advanced** section.
- Keep the menu-bar menu task-oriented: current state, profiles, transcribe a
  file, updates, settings, and quit.
- Use one monochrome **S** template as the macOS menu-bar marker in every state.
  Do not combine it with the former parchment-style bitmap icon or changing
  text markers; status belongs in the tooltip and first menu row.
- Establish one custom S silhouette as the product identity. Derive the macOS
  app icon, DMG/Finder icon, menu-bar template glyph, website favicon, social
  mark, and future platform assets from one vector master; do not maintain a
  separate parchment/document icon family.

## Work plan

### 1. Simplify the app surface

- Remove the Teach tab and its navigation paths.
- Remove model details from the Dictate and Transcribe summaries.
- Put expert transcription controls behind a collapsed Advanced disclosure.
- Tighten typography, spacing, borders, and hierarchy without introducing a
  custom design system or non-native interaction patterns.

Acceptance:

- A new user can identify the shortcut, language profiles, auto-paste behavior,
  and test action without learning what a Whisper model or decoder is.
- Existing settings remain readable and no user preference is discarded.

### 2. Make profiles first-class

- Keep name, language, and shortcut editable in the Dictate view.
- Add a **Profiles** menu in the macOS menu-bar menu, including each profile's
  language and shortcut.
- Make the selected profile unambiguous and keep shortcut-triggered profile
  selection authoritative.

Acceptance:

- Two or more language profiles can be created, changed, removed, and used
  without restarting the app.
- Invalid or conflicting shortcuts fail closed and explain how to recover.
- Equivalent profile management remains available through
  `sagascript config profiles`.
- Idle, recording, loading, transcribing, and hotkey-error states all retain
  the same visible S marker in the macOS menu bar.

### 3. Streamline onboarding and permissions

- Keep a single language choice and automatically use its recommended model.
- Explain model setup as a local speech-engine download, not a model decision.
- Request Microphone only for live dictation and Accessibility only for
  auto-paste. File-only/manual-paste paths must remain usable.
- End with the actual configured shortcut and one clear practice action.

Acceptance on a clean Apple Silicon Mac running macOS 13 or later:

- The notarized DMG installs by drag-and-drop and passes Gatekeeper without a
  bypass.
- Microphone and Accessibility are requested once, deep-link to the correct
  System Settings panes, and persist across quit/relaunch and upgrade.
- Model download has progress, retry, integrity verification, and a recoverable
  failure path.
- Live dictation, auto-paste, file transcription, quit/relaunch, and the bundled
  CLI all pass from the installed app.

### 4. Make updates obvious

- Keep update checks explicit and privacy-preserving.
- Show checking, up-to-date, available, and error states in the menu.
- When a release is available, provide a clear action that opens the exact
  stable GitHub release page. Do not claim an in-app update was installed.

Acceptance:

- Version comparison covers newer, equal, older, malformed, draft, and
  prerelease responses.
- The menu always returns from the temporary checking state.
- The available-version action points at a stable release and the user can
  verify the installed version afterward with `sagascript --version`.

### 5. Publish a minimal product page

- Build a one-page Sagascript site using gille.ai's editorial vocabulary:
  white canvas, black type, thin rules, red accents, generous spacing, and a
  compact grid.
- First viewport: what Sagascript does, local/private positioning, macOS system
  requirement, and one download action.
- Keep the rest to three sections: how it works, CLI usage, and privacy/system
  requirements. Link to GitHub for source and detailed documentation.
- Do not claim Windows availability or Intel support for the v1 binary.

Acceptance:

- The page is responsive, keyboard accessible, visually verified, and links to
  the immutable public release artifact or canonical latest-release page.
- The page states that speech stays local and that network access is used for
  downloads the user initiates.
- gille.ai links to the final product URL only after the hosted page and release
  artifact have both been verified.

### 6. Release gate

Run the repository checks plus a signed, notarized clean-machine acceptance.
Record the exact 40-character release SHA, tag, artifact checksums, signing Team
ID, notarization/Gatekeeper results, installed CLI version, and rollback
artifact. Publishing remains a separate, explicit production action after all
gates pass.

Required deterministic checks:

```text
npm run test:frontend
npm run check
npm run build
npm run release:check
npm run licenses:check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p sagascript-cli --no-default-features
```

## Deferred from this release

- Teach/training UI.
- Cloud transcription or accounts.
- Automatic background update installation.
- Intel macOS binaries and public Windows installers.
- Additional model tuning in the normal settings surface.

## Current implementation checkpoint — 2026-09-01

Completed locally on branch `codex/release-readiness`:

- The normal UI is reduced to Dictate, Transcribe, and Settings. Model choice
  and decoder controls are hidden under Advanced, and Teach is absent from the
  first-release UI while its CLI/core implementation remains intact.
- Onboarding starts with language, immediately prepares the recommended local
  speech engine, has no model-name decision, and then guides the user through
  macOS permissions. A failed download has an error-only escape so an offline
  first run cannot lock the user out of the application.
- The menu bar always uses the monochrome S template. The Profiles submenu shows
  every configured profile with its language and macOS shortcut, marks the
  active or most recently used profile, and links to profile editing;
  shortcut-triggered profiles remain the authoritative selection mechanism.
- Each Dictate profile reports whether its recommended speech engine is ready
  and offers a direct download when it is missing. Manual model selection stays
  hidden under Advanced.
- Update checks expose checking, current, available, and retry states. An
  available release turns the action into a link to that exact stable GitHub
  release tag.
- A minimal product page exists in `site/`, including responsive layout, the
  app screenshot, CLI usage, download links, metadata, favicon, and social
  preview image.
- Four candidate S directions are recorded in
  `docs/design/sagascript-icon-concepts-v1.png`. The selected lower-right soft
  direction is rebuilt in `assets/brand/` as a deterministic vector master.
  Continuous curves, rounded inward-facing terminals, and a single-glyph-only
  rule keep it visually separate from paired lightning-bolt or historical rune
  symbols.
  App, platform, menu-bar, favicon, wordmark, and social-card assets now derive
  from that identity.

Verification completed:

- All frontend checks pass, including 21 release-surface regression tests and
  Svelte diagnostics with zero errors or warnings.
- All Rust workspace checks pass: check, Clippy with warnings denied, lean CLI
  build, and 573 tests.
- Release metadata and license inventory checks pass.
- A local unsigned debug DMG builds and verifies as a valid disk image:
  `Sagascript_1.1.0_aarch64.dmg`, SHA-256
  `14c8b4ed21ec524ca06f65efd7483af270e9b60650957ea189decdefb21fd195`.
  This is a packaging smoke test, not a distributable release artifact.
- Annotated tag `v1.1.0` points at exact release commit
  `6782e7cdfa32af9a7bd73a4cd7aed18c62cc6493`. GitHub Actions run
  `33524699785` passed the quality gate, real transcription, real two-speaker
  diarization, Developer ID signing, app and DMG notarization/stapling,
  Gatekeeper verification, simulated upgrade, and transcription through the
  installed CLI. The generated GitHub release remains an unpublished draft.
- The downloaded draft artifacts were verified independently against both
  `SHA256SUMS` and GitHub's stored digests:
  `Sagascript.dmg` is
  `552cd99e0372d7d9f1370bdfdbd55e11f2177a4b7233792a667b2bec0e02d788`,
  and `Sagascript.app.tar.gz` is
  `5814d1c6eb9de9e8dfba4b6961f2a3c62f6379cf43912e2d8dd84b36a235c8c2`.
  Both app copies pass strict code-signature verification, hardened-runtime and
  audio-input-entitlement checks, stapler validation, and Gatekeeper with Team
  ID `7C6WF6GFZ4`.
- The signing, notarization, Gatekeeper, upgrade, CLI, hash, and artifact
  evidence above applies only to the historical `v1.1.0` baseline at
  `6782e7cdfa32af9a7bd73a4cd7aed18c62cc6493`. It does not cover the 1.1.1
  Accessibility fixes or authorize publishing 1.1.1.
- Claude Opus 5 independently reviewed commit
  `40bda7906eff25a5014dd94c313be3f7db91f11e`. Its grounded findings are fixed:
  recoverable onboarding, visible per-profile engine readiness, re-checkable
  update actions, selected-profile indication, shortcut alias formatting,
  Retina tray source, site contrast/mobile navigation, and dead site scaffold.
- The onboarding window was launched with isolated first-run settings and
  visually checked on macOS. An isolated QA bundle confirmed that the selected
  single-S template is created at startup without a text label or legacy
  parchment fallback. The new QA item was hidden by the MacBook menu-bar
  overflow/notch, so its real rendered appearance remains part of the signed
  acceptance pass rather than being inferred from an older installed S item.

Open release blockers:

- The exact 1.1.1 release revision has not been tagged, pushed, signed,
  notarized, stapled, or checked by Gatekeeper. The 1.1.1 CI run must repeat the
  automated quality gate, simulated upgrade, installed CLI acceptance, artifact
  hashes, and signature verification before its output can be installed.
- After that CI run, a manual first-launch pass on a clean Apple Silicon Mac
  still needs to verify real TCC prompts and persistence for Microphone and
  Accessibility, live dictation, auto-paste, launch at login, profile switching,
  and the update link.
- The selected glyph has been raster-checked at 32, 64, 128, 512, and 1024 px.
  Its current macOS template rendering still needs a visually isolated pass in
  both light and dark menu bars during signed acceptance.
- Claude Opus 5 completed the historical base review. A separate Opus 5 review
  of the full 1.1.0-to-1.1.1 candidate correctly reopened stale release-evidence
  claims and one missing wiring assertion. Those findings were fixed locally; the
  complete corrected diff still needs an independent pass before the release
  SHA is approved. M5 remained unavailable for this pass because its connector
  returned a network failure and the local profile doctor reported a missing
  credential; no credentials were changed.
- The product page builds and its authored files lint cleanly, but its pinned
  Sites scaffold currently reports five high-severity and one low-severity
  production dependency advisories. Do not deploy it until an approved scaffold
  update resolves that audit. Browser-based rendered-page QA is also pending:
  the in-app browser runtime had no connected browser instance during the
  post-review validation pass.
- Linking from gille.ai and publishing the GitHub release/site are separate
  production mutations and require an explicit release approval after these
  gates pass.
