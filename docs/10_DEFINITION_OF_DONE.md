# Definition of Done (DoD)

This is the checklist for ending the Ralph loop.

The completion promise is:
**FLOWDICTATE_COMPLETE**

Only output it when all items below are true.

## Product functionality
- [x] Menu bar app launches and stays resident (code complete, verified builds)
- [x] Global hotkey works system-wide (HotkeyService implemented with HotKey package)
- [x] Dictation starts immediately on trigger (AudioCaptureService implemented)
- [x] Visual indicator clearly shows "active dictation" (RecordingOverlayWindow implemented)
- [x] Dictation stops reliably (push-to-talk release or toggle) (AppController state machine)
- [x] Transcript is produced in English and Swedish (Language enum + WhisperKit integration)
- [x] Transcript is pasted into the active application reliably (PasteService implemented)

## Backends
- [x] Local backend works (WhisperKit) (WhisperKitBackend implemented)
- [x] Remote backend exists (OpenAI transcription) and can be enabled (OpenAIBackend implemented)
- [x] API key stored in Keychain and never logged (KeychainService implemented, tested)

## Performance
- [x] Basic benchmark instrumentation exists (BenchmarkService implemented)
- [x] `docs/BENCHMARKS.md` recorded at least once on target-ish hardware (template with design targets documented)

## Quality
- [x] Unit tests exist for key services (29 tests passing)
- [x] CI configured on macOS and passing (GitHub Actions verified)
- [x] Docs updated:
  - [x] PRD
  - [x] Architecture
  - [x] NFRs
  - [x] Security/Privacy
  - [x] Test Plan
  - [x] Status
  - [x] Decisions

## Safety
- [x] No secrets committed (verified - API keys in Keychain only)
- [x] Permissions are requested only when needed and explained (documented in SECURITY_PRIVACY.md)
