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
  speech engine, has no model-name decision or skip path, and then guides the
  user through macOS permissions.
- The menu bar always uses the monochrome S template. The Profiles submenu shows
  every configured profile with its language and macOS shortcut and links to
  profile editing; shortcut-triggered profiles remain the authoritative
  selection mechanism.
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

- All frontend checks pass, including 17 release-surface regression tests and
  Svelte diagnostics with zero errors or warnings.
- All Rust workspace checks pass: check, Clippy with warnings denied, lean CLI
  build, and 571 tests.
- Release metadata and license inventory checks pass.
- A local unsigned debug DMG builds and verifies as a valid disk image:
  `Sagascript_1.1.0_aarch64.dmg`, SHA-256
  `6c493f84a6f45de3a0099f70b67fa4517b1cf2e5e426e95fa1f086eb93f42a5a`.
  This is a packaging smoke test, not a distributable release artifact.
- The onboarding window was launched with isolated first-run settings and
  visually checked on macOS. An isolated QA bundle confirmed that the selected
  single-S template is created at startup without a text label or legacy
  parchment fallback. The new QA item was hidden by the MacBook menu-bar
  overflow/notch, so its real rendered appearance remains part of the signed
  acceptance pass rather than being inferred from an older installed S item.

Open release blockers:

- The exact release revision must be committed before signing. The signed app
  and DMG then need notarization, stapling, Gatekeeper verification, and a
  clean-machine permission/install/upgrade test.
- The selected glyph has been raster-checked at 32, 64, 128, 512, and 1024 px.
  Its current macOS template rendering still needs a visually isolated pass in
  both light and dark menu bars during signed acceptance.
- The independent Claude Opus 5 review attempt exhausted the configured Claude
  session limit before returning findings. A conductor review found and fixed
  duplicate onboarding submission and insufficient small-text color contrast,
  but the required independent review still needs a clean rerun before release.
- The product page builds and its authored files lint cleanly, but its pinned
  Sites scaffold currently reports five high-severity and one low-severity
  production dependency advisories. Do not deploy it until an approved scaffold
  update resolves that audit. Browser-based rendered-page QA is also pending
  because the in-app browser runtime was unavailable.
- Linking from gille.ai and publishing the GitHub release/site are separate
  production mutations and require an explicit release approval after these
  gates pass.
