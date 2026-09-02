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
- Use compact native macOS text in the menu bar: **S** while idle, **●** while
  recording, **…** while loading or transcribing, and **!** when the hotkey is
  unavailable. Do not use the former parchment bitmap or depend on an
  image-backed status item that macOS may register but paint blank.
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
- Idle, recording, loading/transcribing, and hotkey-error states render the
  native markers S, ●, …, and ! respectively.

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

## Historical signed baseline — v1.1.0

The signed `v1.1.0` baseline is preserved as release evidence. Annotated tag
`v1.1.0` points at exact release commit
`6782e7cdfa32af9a7bd73a4cd7aed18c62cc6493`; GitHub Actions run
`33524699785` passed the quality gate, real transcription and diarization,
Developer ID signing, app and DMG notarization/stapling, Gatekeeper, simulated
upgrade, and installed-CLI checks. The GitHub release remained an unpublished
draft.

The independently verified draft artifacts were:

- `Sagascript.dmg` SHA-256:
  `552cd99e0372d7d9f1370bdfdbd55e11f2177a4b7233792a667b2bec0e02d788`
- `Sagascript.app.tar.gz` SHA-256:
  `5814d1c6eb9de9e8dfba4b6961f2a3c62f6379cf43912e2d8dd84b36a235c8c2`
- Signing Team ID: `7C6WF6GFZ4`

This evidence applies only to that exact historical revision. It does not
prove or authorize publication of a later candidate.

## Current implementation checkpoint — 2026-09-02

The signed `v1.1.1` candidate is installed and remains the stable daily-use
build. Its exact release revision is
`57b91dcdab4729b2a70d1717319c9d9760da7d8c`; GitHub Actions run
`33551011841` passed the quality gate, signing, app and DMG notarization,
stapling, Gatekeeper, upgrade, CLI, and transcription checks. The verified DMG
SHA-256 is
`03c859fd4ac0edabc8ced784fc4aaf526b9c6a13dcfdfd0c3081ef78f66b7199`.
The GitHub release is still an unpublished draft.

Clean-machine onboarding confirmed that transcription and the signed bundle's
Accessibility identity work. The permission page now keeps an explicit reopen
button and reopens the correct System Settings pane without creating another
macOS prompt.

The next patch is prepared in an isolated `1.1.2` worktree and does not modify
the installed app:

- The per-profile download action has a stable layout slot, preventing the
  stale `DownDownloading 100%` WebKit repaint overlap.
- Dictate follows backend recording, model-loading, transcribing, and idle
  events, so it no longer offers a conflicting start while hotkey work is busy.
- Completed-onboarding background launches create no Settings window. Settings
  can be revealed from the menu, macOS Reopen, or the new `sagascript open`
  recovery command; onboarding still opens when incomplete.
- The unreliable image-backed tray marker is replaced with compact native
  state text: S, ●, …, and !.

Verification completed for the isolated patch:

- Frontend tests, Svelte diagnostics, production frontend build, release
  metadata, Rust workspace check, Clippy with warnings denied, the lean CLI
  build, and all 578 Rust tests pass. The AppKit pasteboard test requires the
  unsandboxed macOS test context and passes there.
- A local unsigned app bundle built successfully during an earlier diagnostic.
  That diagnostic exercised a superseded all-bare-launches-hidden rule and does
  not verify the final login-item background marker or deliberate Finder launch
  behavior. Those paths, the onboarding-window hide, and the native status
  marker still need a final isolated visual pass.

Open release blockers:

- Keep the installed `v1.1.1` build running until daily-use work can be paused;
  do not start another GUI candidate because the singleton lock and shared TCC
  identity would interfere with it.
- Visually verify S → ● → … → S and the no-popup behavior using the final signed
  `1.1.2` candidate when an isolated acceptance window is available.
- Resolve the final follow-up from the independent cross-model review before
  release readiness.
- `npm run licenses:check` needs a worktree-local `npm ci`; the current shared
  `node_modules` contains older package versions than `package-lock.json`.
  Dependency installation requires a separate explicit approval.
- The product page scaffold's dependency audit must be green before deployment.
- Tagging, pushing, signing/notarizing, publishing the GitHub release, deploying
  the site, and linking it from gille.ai are separate publication/production
  mutations and require explicit approval with an exact revision.
