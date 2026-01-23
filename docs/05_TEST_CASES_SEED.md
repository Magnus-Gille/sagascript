# Test cases seed

The agent must turn this into `docs/TEST_PLAN.md` and runnable tests.

## Unit tests (fast)

### HotkeyService
- Registers default hotkey
- Hotkey triggers start/stop events correctly
- Push-to-talk:
  - keyDown -> start
  - keyUp -> stop
- Toggle:
  - press once -> start
  - press again -> stop
- Hotkey persistence: saved + restored

### AudioCaptureService
- Produces audio buffers in expected format (sample rate, channels)
- Handles start/stop idempotently
- Handles mic permission missing -> error state

### TranscriptionPipeline
- Backend selection works
- Language selection:
  - English -> config applied
  - Swedish -> config applied
  - Auto -> strategy documented/tested
- Chunking / buffering correctness (if implemented)
- Cancellation works (stop mid-transcribe)

### TextInsertionService
- Clipboard mode:
  - writes clipboard
  - restores previous clipboard after paste (optional but desirable)
- Accessibility:
  - detects permission missing
  - surfaces user-friendly error state

### Settings + Keychain
- Remote API key stored/retrieved from Keychain abstraction
- Never persisted in plaintext settings

## Integration tests (macOS runner)

- Full end-to-end (mock backend):
  - hotkey -> record -> “transcribe” -> paste into a test text field
- Backend smoke tests:
  - run WhisperKit on a small bundled sample audio
  - validate transcript is non-empty
- Permission flow:
  - app handles missing permissions gracefully (no crash)

## Performance tests

- Measure:
  - time from hotkey press to “recording started”
  - time to first partial transcript
  - time to final paste
- Record results in `docs/BENCHMARKS.md`

## Manual test checklist (release readiness)

- Works across apps: Notes, Mail, Safari, VS Code, Slack
- Swedish + English samples
- Long dictation (30–60s)
- Quick repeated snippets
- App resume after sleep
- Hotkey works after switching Spaces/desktops
