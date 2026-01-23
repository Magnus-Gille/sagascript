# Architecture seed

The agent must refine this into `docs/ARCHITECTURE.md`.

## High-level architecture

A native macOS menu bar app with these components:

1) **HotkeyService**
- Registers a global hotkey
- Supports push-to-talk (press & hold) OR toggle mode
- Publishes events: `dictationStart`, `dictationStop`

2) **AudioCaptureService**
- Uses `AVAudioEngine` (or `AVAudioSession` equivalents on macOS)
- Produces audio buffers (PCM) with consistent format
- Optional: voice activity detection (VAD) for auto-stop

3) **TranscriptionPipeline**
- Accepts audio buffers or a recorded segment
- Feeds audio to the active backend:
  - WhisperKitBackend (local, streaming)
  - OpenAIBackend (remote)
  - (Optional) WhisperCppBackend
- Emits partial + final transcript events

4) **TextInsertionService**
Two possible strategies:
- Clipboard + simulate ⌘V (simple, robust)
- AXUIElement direct insertion (more complex)

MVP recommendation:
- clipboard + ⌘V
- requires Accessibility permission

5) **UI**
- Menu bar item:
  - status (idle/recording/transcribing/error)
  - settings
  - quit
- Settings window:
  - hotkey config
  - backend selection
  - language selection
  - remote key management (Keychain)
- **Visual indicator** during dictation:
  - menu bar icon change
  - small HUD overlay / floating panel (“Listening…”)

6) **Telemetry / logs**
- Local logs only
- Never log audio or API keys
- Provide a simple debug mode toggle (off by default)

## Key architectural principles

- Keep heavy work off the main thread.
- Preload and warm the model to reduce “first use” latency.
- Make insertion robust and fail-safe.
- Treat permissions as first-class UX:
  - explain why we need microphone + accessibility
  - provide clear status if missing

## Suggested module boundaries (folders / targets)

- `FlowDictateApp/` (SwiftUI app + menu bar + settings)
- `Core/` (protocols + shared utilities)
- `Hotkey/`
- `Audio/`
- `Transcription/`
- `Insertion/`
- `Backends/`
  - `WhisperKitBackend/`
  - `OpenAIBackend/`

Each major service has:
- protocol
- implementation
- tests

