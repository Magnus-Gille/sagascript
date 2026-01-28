# Status — FlowDictate

This file is the agent's running "project board".

## Current State
**COMPLETE** — All Definition of Done criteria met + performance optimization + Swedish language support + expert review fixes + launch at login + UI cleanup + advanced hotkey support (2026-01-24)

## Summary

FlowDictate is a complete macOS dictation app with:

1. **Menu bar only application** using SwiftUI MenuBarExtra (no dock icon, no main window)
2. **Global hotkey** (Control+Shift+Space default, configurable) with advanced support for Fn and modifier-only triggers
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
- **Hotkey recorder fixed** (2026-01-24) - Settings hotkey recorder now captures key events using NSViewRepresentable with first responder
- **Modifier-only hotkey crash fixed** (2026-01-24) - Removed UInt32 casts that caused crash when using ⌘ alone or other modifier-only hotkeys

### Performance Fixes
- **WAV encoding optimized** - Uses `withUnsafeBufferPointer` for O(1) memory copy instead of per-sample loop
- **WhisperKit worker scaling** - Dynamic worker count based on CPU cores (min 2, max 16, typically cores/2)

## Advanced Hotkey Support (2026-01-24)

Added comprehensive hotkey support beyond standard Carbon API limitations:

### New Capabilities
- **Normal shortcuts** - ⌘+Z, ⌥+Z, ⌃+Z, etc. (using Carbon/HotKey package)
- **Fn key combinations** - Fn+Z, Fn+Space, etc. (using CGEventTap)
- **Modifier-only triggers** - ⌘ alone, ⌥ alone, ⌃⌥ chord (using CGEventTap)

### Implementation
- **Shortcut model** (`Sources/FlowDictate/Hotkeys/Shortcut.swift`)
  - Constants: `kModsFnBit` (custom Fn bit), `kKeyCodeModifiersOnly` (-1 sentinel)
  - Conversion functions between NSEvent, CGEvent, and Carbon modifier formats
  - Shortcut description rendering (e.g., "Fn+Z", "⌘⌥", "⌃⇧Space")

- **CGEventTap backend** (`Sources/FlowDictate/Hotkeys/CGEventTapHotkeyService.swift`)
  - Uses `.cgSessionEventTap` with `.listenOnly` option
  - "Tap-only" semantics for modifier-only: triggers only when no non-modifier key pressed
  - Handles `.tapDisabledByTimeout` / `.tapDisabledByUserInput` re-enabling
  - Requires Input Monitoring permission (shows guidance alert if missing)

- **Unified HotkeyService** (`Sources/FlowDictate/Services/HotkeyService.swift`)
  - Automatically selects backend: Carbon for standard shortcuts, CGEventTap for Fn/modifier-only
  - `suspend()`/`resume()` methods for safe hotkey recording

- **Improved recorder** (`Sources/FlowDictate/Views/HotkeyRecorderView.swift`)
  - Normal keys: Accept immediately on keyDown (not keyUp)
  - Modifier-only: Accept when all modifiers released back to 0
  - Prevents premature acceptance that would break combos like ⌘+Z

### Permissions
- Standard shortcuts (⌘+Z, etc.): No extra permissions needed
- Fn or modifier-only: Requires Input Monitoring permission in System Settings

### Tests
- 67 unit tests covering shortcut model, conversion functions, and recorder behavior

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

## Accuracy Improvement Initiative (2026-01-27)

Multi-strategy effort to improve transcription accuracy. Uses per-strategy feature branches with TDD workflow.

### Phase 1 — COMPLETE (3 branches, ready to merge)

All 3 strategies implemented in parallel on separate feature branches. All tests pass. Not yet merged to main.

| # | Strategy | Branch | Tests | Commit | Key Changes |
|---|----------|--------|-------|--------|-------------|
| 1 | Model upgrade | `feature/accuracy-1-model-upgrade` | 72 | `8a0cb2f` | Added `smallEn` (244M) and `largev3Turbo` (809M) to WhisperModel enum. English default changed from `baseEn` to `smallEn`. |
| 2 | Custom vocabulary | `feature/accuracy-2-custom-vocabulary` | 80 | `051e7f2` | New `PromptBuilder` struct, `customVocabulary` + `promptConditioningEnabled` settings, `promptTokens` wired into DecodingOptions via WhisperKit tokenizer. |
| 3 | VAD + audio processing | `feature/accuracy-3-vad` | 83 | `f002fc6` | New `AudioProcessor` (normalize, RMS energy, trim silence) using Accelerate/vDSP. Pre-processes audio before inference. `noSpeechThreshold` set to 0.6. |

**Merge order note**: All 3 branches modify `WhisperKitBackend.swift`. Merge conflicts expected. Recommended order: #1, then #3, then #2 (or resolve manually).

**Manual testing instructions**:
1. **Strategy #1** — `git checkout feature/accuracy-1-model-upgrade && swift test` — verify 72 tests pass. Check Language.swift for `smallEn` and `largev3Turbo` cases.
2. **Strategy #2** — `git checkout feature/accuracy-2-custom-vocabulary && swift test` — verify 80 tests pass. Set vocabulary via SettingsManager, transcribe, check prompt tokens applied.
3. **Strategy #3** — `git checkout feature/accuracy-3-vad && swift test` — verify 83 tests pass. Check AudioProcessor normalizes and trims silence from audio before transcription.

**Files per branch**:
- Strategy #1: Modified `Language.swift`, `LanguageTests.swift`. Created `docs/plans/accuracy-1-model-upgrade.md`.
- Strategy #2: Created `PromptBuilder.swift`, `PromptBuilderTests.swift`. Modified `SettingsManager.swift`, `SettingsManagerTests.swift`, `WhisperKitBackend.swift`. Created `docs/plans/accuracy-2-custom-vocabulary.md`.
- Strategy #3: Created `AudioProcessor.swift`, `AudioProcessingTests.swift`. Modified `WhisperKitBackend.swift`. Created `docs/plans/accuracy-3-vad.md`.

### Phase 2 — NOT STARTED (depends on Phase 1 merge)

- [ ] Strategy #4: LLM post-processing — Confidence-guided correction of low-confidence words using local LLM (MLX Swift). Branch: `feature/accuracy-4-llm-correction`.
- [ ] Strategy #6: Feedback loop — Learn from user corrections over time. Branch: `feature/accuracy-6-feedback-loop`.

### Phase 3 — NOT STARTED (depends on Phase 2)

- [ ] Strategy #5: LoRA voice fine-tuning — In-app voice enrollment + LoRA adapter training. Branch: `feature/accuracy-5-lora-finetuning`.

### Phase 4 — DEFERRED

- [ ] Strategy #7: Apple SpeechAnalyzer — Alternative backend using macOS 26+ SpeechAnalyzer framework. Branch: `feature/accuracy-7-speech-analyzer`.

### Coordination

- Instance coordination log: `docs/INSTANCE_LOG.md`
- Per-strategy plan files: `docs/plans/accuracy-{1,2,3}-*.md` (on respective branches)
- Master plan: saved in `.claude/plans/typed-tickling-gizmo.md`

## Known Issues / Next Session

- [ ] **Merge Phase 1 branches** — All 3 accuracy feature branches need merging to main. Expect merge conflicts in WhisperKitBackend.swift.
- [ ] **Add Settings UI for custom vocabulary** — Strategy #2 added the backend (SettingsManager properties, PromptBuilder) but no UI field in SettingsView for entering vocabulary terms.
- [ ] **Review modifier-only hotkey activation** (2026-01-24) - Activation scenarios for ⌘ alone / ⌥ alone feel "wonky". Need to review and tune the CGEventTap trigger logic in `CGEventTapHotkeyService.swift`.

## Potential Next Steps

Ideas for future work (not committed to):
- [ ] Phase 2-4 accuracy strategies (see above)
- [ ] Text formatting/cleanup via LLM (like Wispr Flow)
- [ ] Streaming transcription for real-time feedback
- [ ] Multiple language quick-switch
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
- `Sources/FlowDictate/Hotkeys/` — Shortcut model and CGEventTap backend for advanced hotkey support

### Tests
- `Tests/FlowDictateTests/` — 67 unit tests (main), 72/80/83 on accuracy feature branches

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
