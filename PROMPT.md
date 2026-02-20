# MASTER PROMPT — Sagascript Autonomous Build (Claude Code + Ralph Loop)

You are running inside a **Ralph loop** (continuous iteration). Your job is to complete the entire project **without asking the user questions**.

## Absolute rules

1. **Do not ask the user questions.** If something is ambiguous, pick the best default, document it in `docs/DECISIONS.md`, and proceed.
2. **Stay sandboxed.** Only modify files inside this repository.
3. **Be honest.** If something requires credentials (GitHub, API keys, Apple signing) and you don’t have them, implement a secure placeholder and document how to enable it later.
4. **Performance matters.** Latency and responsiveness are top priorities.
5. **Privacy matters.** Local-first transcription is the default. Remote transcription is opt-in.
6. **Ralph loop completion promise:** You may output the exact phrase **SAGASCRIPT_COMPLETE** *only when everything in the “Definition of Done” section is true.*

---

## What you’re building

A **Wispr Flow–style dictation app** for **macOS** (Apple Silicon; target: MacBook Air **M4**, 32GB RAM).

### Core user experience (MVP)

- Runs as a **menu bar** app (minimal UI).
- User configures a **global hotkey/button** (default can be something safe like `⌥ Space`).
- When the hotkey is pressed:
  - Start dictation immediately.
  - Provide a clear visual indicator: *“dictation is active now”*.
    - Swedish requirement: *“Och så bör transkriptionen ha en visuell indikation som visar att man nu använder applikationen.”*
- When dictation stops (key released or toggled off):
  - Transcribe the utterance quickly (low latency).
  - Paste the transcribed text into the **currently active app/window** (like Wispr Flow).
- Support **English and Swedish** (and only those are required).
- The app should be very fast and feel “instant”.

### Model options

Provide selectable transcription backends:

1. **Local (preferred default)**:
   - Primary: **WhisperKit** (Swift/Core ML on Apple Silicon) for low latency streaming & on-device inference.
   - Secondary/fallback: **whisper.cpp** (Metal acceleration) OR Apple Speech framework (optional) as alternative.
2. **Remote (optional)**:
   - OpenAI Audio Transcriptions API (`gpt-4o-transcribe`, `gpt-4o-mini-transcribe`, and/or `whisper-1`) behind a provider interface.
   - API keys must be stored securely (macOS Keychain) and never logged.

### Non-functional requirements (NFRs)

**Latency & performance**
- Target “push-to-talk responsiveness”:
  - mic capture starts within ~100ms of hotkey press
  - time-to-first-partial (if streaming UI) within ~500ms
  - time-to-final text paste within ~1–2s for typical short utterances (local small model)
- Minimal CPU spikes; avoid draining battery.
- Preload/warm the model at app launch or first use.
- Avoid jank in UI; main thread must stay responsive.

**Security & privacy**
- Local-first.
- Never persist audio by default.
- If remote transcription is enabled:
  - clear toggle + disclosure
  - API key stored in Keychain
  - TLS only
  - don’t send audio unless user explicitly enabled remote backend
- Follow least-privilege permissions:
  - microphone permission
  - accessibility permission only if needed for paste/typing (document why)

**Code quality**
- Clear module boundaries.
- Comments for non-obvious parts.
- Concurrency safe (Swift Concurrency where appropriate).
- Comprehensive tests (unit tests + some integration tests where feasible).
- CI runs tests on macOS.

---

## Autonomy workflow (repeat every loop iteration)

You must follow this workflow **every iteration**:

1. **Re-read**:
   - `PROMPT.md`
   - `CLAUDE.md`
   - `docs/10_DEFINITION_OF_DONE.md`
   - The current project status in `docs/STATUS.md`
2. **Plan** the next smallest verifiable step:
   - Update `docs/STATUS.md` with:
     - what you’re doing now
     - what changed since last iteration
     - what remains
3. **Implement**:
   - Prefer TDD: write tests first when possible.
   - Keep changes small and incremental.
4. **Verify**:
   - Run relevant tests/lint/build locally if possible.
   - If macOS build isn’t available in this environment, ensure CI will cover it.
5. **Commit**:
   - Commit meaningful increments with a clear message.
6. **If stuck**:
   - Create a `docs/BLOCKERS.md` entry with:
     - exact error messages
     - what you tried
     - next approach
   - Then proceed with another approach (fallback implementation, simplify scope).

---

## Git + GitHub requirements

You must set up and use git properly:

- Initialize git (if not already).
- Use frequent commits.
- Add GitHub automation:
  - GitHub Actions CI on macOS: build + run tests.
  - Dependabot for Swift Package Manager (if applicable) and GitHub Actions.
  - (Optional) CodeQL for Swift if practical; otherwise document why not.

**GitHub integration (autonomous)**
- If GitHub credentials are available (e.g., `gh auth status` works or `GITHUB_TOKEN` / `GH_TOKEN` exists), create a GitHub repo and push.
- If not available, keep everything local and document a one-command push flow in `docs/GITHUB_SETUP.md`.

---

## Deliverables you must create/update

Create these files early (then keep improving them):

- `docs/PRD.md` — full product requirements document (user stories, acceptance criteria, UX flows)
- `docs/ARCHITECTURE.md` — architecture + diagrams (Mermaid ok)
- `docs/NFRS.md` — performance, latency budgets, reliability, etc
- `docs/SECURITY_PRIVACY.md` — threat model + data handling
- `docs/TEST_PLAN.md` — detailed tests and how to run them
- `docs/STATUS.md` — running status log / progress tracker
- `docs/DECISIONS.md` — assumptions and architectural decisions

---

## Technical direction (use this unless you have a better proven plan)

Default stack (recommended):

- **Swift + SwiftUI** menu-bar app (AppKit bridging where needed)
- **AVAudioEngine** for mic capture
- **WhisperKit** as the default local transcription engine
- A provider interface for remote transcription using OpenAI Audio Transcriptions API
- Global hotkey via Carbon or a maintained Swift package (evaluate trade-offs)
- Text insertion into active app via:
  - safest approach: copy to clipboard + simulate ⌘V
  - require Accessibility permission; handle gracefully if not granted
- Visual indicator:
  - menu bar icon state + small HUD overlay or floating panel when recording

---

## Definition of Done

You may only output **SAGASCRIPT_COMPLETE** when all are true:

### Product
- [ ] Push-to-talk or toggle hotkey works globally.
- [ ] Recording indicator is visible and unambiguous (menu icon + overlay).
- [ ] English + Swedish transcription supported (user selectable OR auto with constraints).
- [ ] Transcribed text is pasted into the active application reliably.
- [ ] Runs in background as a menu bar app.
- [ ] Settings UI is minimal and understandable.

### Backends
- [ ] Local backend works (WhisperKit).
- [ ] Remote backend exists (OpenAI) and is behind a clean interface.
- [ ] API keys stored in Keychain and never logged.

### Quality
- [ ] Unit tests exist for key logic (audio pipeline pieces that can be tested; model selection; paste mechanism abstraction; settings persistence).
- [ ] CI is set up and passing on macOS.
- [ ] Docs: PRD, Architecture, NFRs, Security/Privacy, Test Plan, Status, Decisions are complete.

### Safety
- [ ] No sensitive data is committed.
- [ ] Permissions are requested only when needed and documented.

When complete, print:
SAGASCRIPT_COMPLETE
