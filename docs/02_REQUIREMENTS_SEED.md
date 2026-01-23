# Requirements seed (from the user)

This is the structured interpretation of the user’s request. The agent must refine this into `docs/PRD.md`.

## Product summary

Build a **macOS dictation app** similar to **Wispr Flow**:
- Triggered by a **configurable global hotkey/button**
- Captures microphone audio, transcribes it (English + Swedish), and **pastes** the result into the currently focused app
- Must be **extremely low latency** and feel instant
- Must have a **clean minimal UI**
- Must run in the background, likely as a **menu bar app**
- Must show a **visual indicator** while dictation is active *(sv: visuell indikation)*

## Personas

- Primary: power user who dictates many short snippets into any app (email, docs, chat)
- Secondary: bilingual user (English + Swedish)

## In-scope (MVP)

### Dictation flow
1. User presses global hotkey
2. App immediately starts capturing audio
3. UI shows dictation active indicator
4. User stops dictation (release key or press again)
5. App transcribes quickly
6. App inserts text into the active app

### Settings
- Configure global hotkey
- Choose transcription backend:
  - Local (default)
  - Remote (optional)
- Select language mode:
  - English
  - Swedish
  - Auto (optional; but consider latency)
- Basic status/debug (minimal)

## Out of scope (initially)
- Editing previous snippets
- Saved history/transcript log (privacy-sensitive)
- Cloud sync
- Multi-device
- Custom vocabulary (can be later)

## Acceptance criteria (MVP)
- Works on macOS Apple Silicon (target: MacBook Air M4)
- Reliable hotkey detection system-wide
- Responsive start/stop (no noticeable lag)
- Correctly pastes text into focused app
- Transcribes English and Swedish
- Clear visual indicator when active
- Reasonable CPU/battery usage

## Non-functional requirements
- Latency: prioritise time-to-text
- Privacy: local-first, no audio retention by default
- Security: API keys stored securely (Keychain)
- Code quality: tests, CI, good modular design

