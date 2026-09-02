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

The accepted daily-use build is Sagascript `1.1.2` from exact source revision
`6008bd191a9c33281fc420ca9492a0edd3fb8f48`. The installed Apple Silicon app
is Developer ID signed by Team ID `7C6WF6GFZ4`, has hardened runtime and the
audio-input entitlement, and reports
`sagascript 1.1.2 (git 6008bd1, built 2026-09-02)`. It remains running while
release preparation happens in a separate worktree.

Manual acceptance has confirmed that the idle **S** is visible in the menu
bar, transcription works, and version/build information is visible in the app.
The implemented release surface also includes:

- Native menu states **S**, **●**, **…**, and **!**, plus direct profile
  selection and a clear update action.
- A stable download-progress layout and backend-owned Dictate state, avoiding
  the repaint overlap and conflicting recording action found during testing.
- A completed-onboarding background path that does not create a Settings
  window. Settings opens only through a deliberate menu, Finder/Reopen, or
  `sagascript open` action.
- Accessibility recovery that deep-links back to the correct pane without
  issuing repeated permission prompts.
- Version, source revision, and build date in both the app and
  `sagascript --version`.
- A short responsive product page with the shared S identity, local-processing
  explanation, install flow, CLI examples, and canonical GitHub release link.
  Its production dependency audit currently reports zero known
  vulnerabilities.

The product-page copy review was delegated to the local M5 inference host using
`qwen3-30b-instruct`. Grounded findings were adopted: the privacy boundary now
names update checks and user-selected engine downloads, the profile copy no
longer implies a two-language limit, and the source link names GitHub.
An independent release-diff review with M5's `qwen3-coder-next-80b` found no
release-blocking issues after the deterministic checks passed. The final
product page passed visual desktop and mobile review at 1440 px and 390 px;
navigation, responsive stacking, readable terminal text, and browser console
output were checked, and an observed missing-favicon request was fixed.

Open release gates:

- A local Apple Silicon build from exact revision
  `e42d4cd4e91211a55310d0d6c4b03cf336160b7e` produced the expected app and
  DMG metadata, but signing from the Codex process omitted a usable certificate
  chain: strict `codesign` verification rejected both artifacts. Those
  artifacts are quarantined from installation and publication. Produce the
  authoritative Developer ID signed and notarized artifacts through the
  release workflow's ephemeral keychain, or rebuild from the owner's Terminal,
  then verify the newly embedded exact revision.
- Run the signed/notarized clean-install acceptance: DMG drag-and-drop,
  Gatekeeper, Microphone, Accessibility with an unrelated Settings pane open,
  model download, dictation, auto-paste, file transcription, relaunch, update
  check, and bundled CLI.
- Tagging, pushing, notarization through release CI, publishing the GitHub
  release, deploying the site, and asking the gille.ai repository to add the
  link remain explicit production/publication actions. Each must name the
  exact 40-character release SHA, target, verification, and rollback before
  execution.
