# UI/UX guide (minimal and fast)

## UI components (MVP)

### 1) Menu bar icon
States:
- Idle
- Listening (recording)
- Transcribing
- Error (missing permission / backend failure)

Menu items:
- Start/Stop dictation (optional)
- Settings…
- Quit

### 2) Settings window (minimal)
Sections:
- Hotkey:
  - “Press new hotkey…” capture control
  - mode: push-to-talk vs toggle
- Language:
  - English
  - Swedish
  - Auto (optional)
- Backend:
  - Local: WhisperKit
  - Remote: OpenAI (optional)
- Remote API key:
  - text field + “Save to Keychain”
  - “Remove key”
- Permissions status:
  - Microphone: granted/denied
  - Accessibility: granted/denied
  - Buttons to open System Settings panes

### 3) Visual indicator (important)
Requirement (sv): “visuell indikation…”
MVP options:
- A small floating HUD panel that appears while listening:
  - “Listening…” + waveform or dot
  - changes to “Transcribing…” briefly after stop
- Keep it small and non-intrusive.

## Interaction model recommendation

Default:
- Push-to-talk: hold hotkey to record, release to stop and transcribe.

Rationale:
- Minimizes accidental open microphone
- Predictable for latency and privacy

## Error handling UX

If mic permission missing:
- show actionable UI, do not crash
If accessibility missing:
- show that dictation works but paste is disabled until enabled

