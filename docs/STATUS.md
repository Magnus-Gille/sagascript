# Status — FlowDictate

This file is the agent's running "project board".

## Current State
**COMPLETE** — All Definition of Done criteria met + performance optimization + Swedish language support + expert review fixes + launch at login (2026-01-24)

## Summary

FlowDictate is a complete macOS dictation app with:

1. **Menu bar application** using SwiftUI MenuBarExtra
2. **Global hotkey** (Control+Shift+Space default, configurable) using HotKey package
3. **Audio capture** via AVAudioEngine at 16kHz mono
4. **Local transcription** using WhisperKit (Apple Silicon optimized, **performance optimized**)
5. **Remote transcription** using OpenAI Whisper API
6. **Visual indicator** with floating NSPanel overlay
7. **Text paste** via clipboard + simulated Cmd+V
8. **Settings UI** with language, backend, hotkey, and **model selection**
9. **Secure API key storage** in macOS Keychain
10. **Structured logging** with JSONL file output for debugging + **RTF metrics**
11. **Swedish-optimized transcription** via KB-Whisper models (4x better WER)
12. **Launch at login** via SMAppService (macOS 13+)

## Expert Review Fixes (2026-01-23)

Addressed security and stability issues identified by senior reviewers:

### Security Fixes
- **Transcript logging gated** - `#if DEBUG` guards prevent transcripts from appearing in release build console logs
- **Log file permissions** - Logs now created with 0o600 (owner-only read/write), directory with 0o700
- **OpenAI backend hardening** - Ephemeral URLSession (no caching), 60s request timeout, 25MB file size check

### Stability Fixes
- **Async mic permission** - Permission dialog no longer blocks main thread (uses `await AVCaptureDevice.requestAccess`)
- **Audio buffer size cap** - 15-minute maximum prevents unbounded memory growth
- **Data loss prevention** - Audio retained in `lastCapturedAudio` for retry on transcription failure

### User Experience Fixes
- **Clipboard save/restore** - Previous clipboard contents restored after paste (~100ms delay)
- **Retry transcription** - `retryLastTranscription()` method allows re-processing failed audio

### Performance Fixes
- **WAV encoding optimized** - Uses `withUnsafeBufferPointer` for O(1) memory copy instead of per-sample loop
- **WhisperKit worker scaling** - Dynamic worker count based on CPU cores (min 2, max 16, typically cores/2)

## Swedish Language Support (2026-01-23)

Added KB-Whisper models for significantly improved Swedish transcription:
- **KB-Whisper models** - Fine-tuned on 50,000+ hours of Swedish speech data
- **whisper.cpp backend** - SwiftWhisper used for KB-Whisper GGML models
- **Auto-download** - Models downloaded automatically from HuggingFace on first use
- **Auto-model selection** - Automatically uses kb-whisper-base for Swedish language
- **Model variants** - kb-whisper-tiny (13% WER), kb-whisper-base (9% WER), kb-whisper-small (7% WER)

Swedish WER improvements vs OpenAI base Whisper:
| Model | Swedish WER | Improvement | Size |
|-------|-------------|-------------|------|
| OpenAI base | 39.6% | baseline | - |
| kb-whisper-tiny | 13.2% | 3x better | ~40MB |
| kb-whisper-base | 9.1% | 4x better | ~60MB |
| kb-whisper-small | 7.3% | 5x better | ~190MB |

**No manual setup required** - select Swedish language and the model downloads automatically.

## Performance Optimization (2026-01-23)

Applied WhisperKit performance optimizations:
- **Model prewarming** - CoreML models specialized for ANE at load time
- **Full compute options** - GPU for mel, ANE for encoder/decoder, CPU for prefill
- **Greedy decoding** - Deterministic, no sampling overhead
- **Quality thresholds disabled** - Skip compression/logprob checks for speed
- **User-selectable models** - tinyEn, tiny, baseEn, base + KB-Whisper Swedish models
- **RTF logging** - Real-Time Factor tracked in logs for performance monitoring

Expected improvement: Faster transcription due to optimized decoding options.
Note: First load with prewarm takes ~4-6s, subsequent loads faster.

## Potential Next Steps

Ideas for future work (not committed to):
- [ ] Text formatting/cleanup via LLM (like Wispr Flow)
- [ ] Streaming transcription for real-time feedback
- [ ] Multiple language quick-switch
- [ ] Custom vocabulary/context prompts
- [ ] App notarization for distribution

## Repository

- GitHub: https://github.com/Magnus-Gille/flowdictate
- CI: GitHub Actions (macOS 14, build + test)
- Dependencies: WhisperKit, SwiftWhisper, HotKey (via SPM)

## Files Created

### Source Code
- `Sources/FlowDictate/FlowDictateApp.swift` — App entry point
- `Sources/FlowDictate/Models/` — Language, AppState, AnyCodable, LogEvents
- `Sources/FlowDictate/Services/` — All core services including LoggingService, LaunchAtLoginService
- `Sources/FlowDictate/Views/` — SwiftUI views

### Tests
- `Tests/FlowDictateTests/` — 29 unit tests

### Documentation
- `docs/PRD.md` — Product requirements
- `docs/ARCHITECTURE.md` — Architecture + diagrams
- `docs/NFRS.md` — Performance requirements
- `docs/SECURITY_PRIVACY.md` — Security + privacy
- `docs/TEST_PLAN.md` — Testing strategy
- `docs/DECISIONS.md` — Design decisions
- `docs/BENCHMARKS.md` — Performance benchmarks
- `docs/GITHUB_SETUP.md` — GitHub setup guide

### App Bundle
- `AppBundle/Info.plist` — macOS app bundle configuration
- `scripts/build-app.sh` — Build script for creating .app bundle

### CI/CD
- `.github/workflows/ci.yml` — Build + test workflow
- `.github/dependabot.yml` — Dependency updates

## Definition of Done

All items complete:
- [x] Product functionality (7/7)
- [x] Backends (3/3)
- [x] Performance (2/2)
- [x] Quality (3/3)
- [x] Safety (2/2)

## How to Use

1. Clone the repository
2. Run `swift test` to run tests
3. Run `./scripts/build-app.sh` to build the app bundle
4. Run `open .build/release/FlowDictate.app` to launch the app
5. Grant Microphone and Accessibility permissions when prompted
6. Press Control+Shift+Space to start dictating (configurable in Settings)

**Note:** The app runs as a menu bar application (no Dock icon). Look for the waveform icon in the menu bar.
