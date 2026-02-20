# Decisions Log

Record assumptions and design decisions here, with rationale and dates.

---

## 2026-02-18 — TCC Permission Reset Workaround for Dev Builds

- **Problem:** After every rebuild, macOS TCC invalidates previously granted permissions (Microphone, Accessibility, Input Monitoring) because the ad-hoc code signature changes
- **Workaround:** Reset TCC entries and re-grant:
  ```
  tccutil reset Microphone com.sagascript.app
  tccutil reset Accessibility com.sagascript.app
  ```
  Then relaunch the app and grant permissions when prompted.
  Alternatively: System Settings > Privacy & Security, toggle Sagascript off/on for each permission.
- **Root cause:** Ad-hoc signing (`codesign --sign -`) generates a new signature each build. macOS ties TCC grants to the signature, not the bundle identifier alone.
- **Permanent fix:** Use a stable Developer ID certificate for signing. This would preserve permissions across rebuilds.

---

## 2026-01-23 — Use WhisperKit as primary local transcription engine

- **Decision:** Use WhisperKit (argmaxinc/WhisperKit) instead of whisper.cpp
- **Alternatives considered:**
  - whisper.cpp with Swift bindings
  - Apple Speech framework
  - MLX Whisper
- **Rationale:**
  - WhisperKit is pure Swift, designed for Apple Silicon
  - Uses Core ML with Neural Engine optimization
  - Actively maintained with Apple partnership
  - Clean Swift API, no C++ bridging needed
- **Consequences:**
  - Model files may need to be bundled or downloaded (~40-150MB)
  - Tied to Apple Silicon for best performance

---

## 2026-01-23 — Use Control+Shift+Space as default hotkey (updated)

- **Decision:** Default hotkey is Control+Shift+Space (⌃⇧Space)
- **Previous default:** Option+Space (changed due to conflicts)
- **Alternatives considered:**
  - Option+Space (conflicts with user's other app)
  - Cmd+Shift+D (conflicts with some apps)
  - Double-tap Fn (not reliably capturable)
  - Ctrl+Space (conflicts with Spotlight on some configs)
- **Rationale:**
  - Control+Shift+Space is rarely used by other apps
  - Avoids conflict with input method switches (Option+Space)
  - Three-key combo ensures intentional activation
  - Easy to press with one hand
- **Consequences:**
  - Slightly more complex to press than two-key combos
  - User can change in settings via hotkey recorder UI

---

## 2026-01-23 — Configurable hotkey via Settings UI

- **Decision:** Allow users to configure the dictation hotkey in Settings
- **Alternatives considered:**
  - Config file only (poor UX)
  - Fixed hotkey (inflexible)
  - Terminal command to change (not user-friendly)
- **Rationale:**
  - Users have different workflows and conflicting shortcuts
  - Visual feedback during recording makes it easy to use
  - Pressing Escape cancels without saving
- **Implementation:**
  - HotkeyRecorderView captures key combinations via NSEvent monitor
  - Requires at least one modifier (except for F-keys)
  - Changes take effect immediately after recording
- **Consequences:**
  - More code for hotkey capture UI
  - Must validate hotkeys don't conflict with system shortcuts

---

## 2026-01-23 — Use clipboard + Cmd+V for text insertion

- **Decision:** Copy text to clipboard and simulate Cmd+V to paste
- **Alternatives considered:**
  - CGEventKeyboardSetUnicodeString (limited to short strings)
  - Accessibility API text insertion (complex, app-specific)
  - AppleScript/osascript (slow, unreliable)
- **Rationale:**
  - Universal across all macOS apps
  - Reliable and well-tested pattern
  - Same approach used by other dictation apps
- **Consequences:**
  - Requires Accessibility permission
  - Briefly overwrites clipboard (could save/restore)
  - User's clipboard content may be replaced

---

## 2026-01-23 — Use Carbon API for global hotkeys

- **Decision:** Use Carbon Events API for global hotkey registration
- **Alternatives considered:**
  - HotKey Swift package (wraps Carbon)
  - CGEvent tap (more complex, needs root-like permissions)
  - NSEvent global monitor (doesn't work for all keys)
- **Rationale:**
  - Carbon API is the only reliable way for global hotkeys
  - Well-documented, stable for decades
  - HotKey package wraps it nicely with Swift API
- **Consequences:**
  - Must link Carbon framework
  - API is old but stable

---

## 2026-01-23 — Use macOS 14.0 (Sonoma) as minimum version

- **Decision:** Require macOS 14.0 or later
- **Alternatives considered:**
  - macOS 13.0 (Ventura)
  - macOS 12.0 (Monterey)
- **Rationale:**
  - WhisperKit requires modern Core ML features
  - Swift Concurrency improvements in macOS 14
  - Simplifies testing matrix
  - M4 chips run Sonoma or later
- **Consequences:**
  - Users on older macOS cannot use the app
  - Acceptable for target audience (M4 MacBook Air)

---

## 2026-01-23 — Store API keys in Keychain with device-only access

- **Decision:** Use kSecAttrAccessibleWhenUnlockedThisDeviceOnly for API key storage
- **Alternatives considered:**
  - UserDefaults (insecure)
  - Encrypted file (complex)
  - iCloud Keychain sync (exposes to other devices)
- **Rationale:**
  - Keychain is Apple's recommended secure storage
  - Device-only prevents sync to other devices
  - Accessible only when device is unlocked
- **Consequences:**
  - User must re-enter key if they reinstall or switch machines
  - Acceptable tradeoff for security

---

## 2026-01-23 — Use SPM (Swift Package Manager) for project structure

- **Decision:** Use Swift Package Manager instead of Xcode project
- **Alternatives considered:**
  - Xcode project (.xcodeproj)
  - Tuist
  - XcodeGen
- **Rationale:**
  - SPM is first-class in Swift ecosystem
  - Easier CI/CD without Xcode GUI
  - Better for open source (no Xcode version conflicts)
  - Can still open in Xcode for development
- **Consequences:**
  - Some Xcode-specific features (e.g., asset catalogs) need workarounds
  - May need to generate xcodeproj for certain tasks

---

## 2026-01-23 — Bundle small model, download larger models on demand

- **Decision:** Bundle whisper-tiny or whisper-base, offer larger models as download
- **Alternatives considered:**
  - Bundle all models (huge app size)
  - Download all models (slow first launch)
  - No bundled model (requires network)
- **Rationale:**
  - Small model provides immediate functionality
  - Users who want better accuracy can download
  - Keeps initial app size reasonable (~50MB vs 500MB+)
- **Consequences:**
  - Need model download/management code
  - First-use latency for larger models

---

## 2026-01-23 — Use NSPanel for recording overlay instead of SwiftUI popover

- **Decision:** Floating NSPanel for the "recording active" indicator
- **Alternatives considered:**
  - SwiftUI popover from menu bar
  - Notification banner
  - Menu bar icon only
- **Rationale:**
  - NSPanel can float above all windows
  - More control over appearance and positioning
  - Always visible during recording
- **Consequences:**
  - More code than simple SwiftUI
  - Must manage window level and activation

---

## 2026-01-23 — Show main window on launch for initial configuration

- **Decision:** Display a visible main window on app launch alongside menu bar
- **Alternatives considered:**
  - Menu bar only (original implementation)
  - Onboarding wizard
  - Preferences window auto-open
- **Rationale:**
  - Users expect to see UI when launching an app
  - Provides clear indication app is running
  - Shows hotkey configuration and current status
  - Makes transcription results visible without needing terminal
- **Consequences:**
  - Slight increase in UI code
  - Window can be closed; menu bar remains active

---

## 2026-01-23 — Print transcription to console instead of auto-paste

- **Decision:** Print transcription result to terminal/window instead of automatically pasting
- **Alternatives considered:**
  - Auto-paste to active app (original behavior)
  - Copy to clipboard only
  - Configurable output mode
- **Rationale:**
  - Auto-paste was surprising and could disrupt user workflow
  - Terminal output is visible and predictable
  - User can manually copy from main window if needed
  - Better for testing and debugging
- **Consequences:**
  - Less "magic" convenience
  - User sees transcription clearly before deciding what to do with it
  - Can add opt-in auto-paste feature later if desired

---

## 2026-01-23 — Enforce minimum recording duration (300ms)

- **Decision:** Require at least 300ms of recording time before stopping
- **Alternatives considered:**
  - No minimum (original behavior, caused 0-sample captures)
  - Smaller minimum (100ms)
  - Wait for first audio buffer before allowing stop
- **Rationale:**
  - Audio tap delivers buffers asynchronously (~64ms chunks)
  - Very short press could release before first buffer arrives
  - 300ms ensures at least several audio buffers captured
  - Balances responsiveness with reliability
- **Consequences:**
  - Quick taps are extended to minimum duration
  - Slight delay on very short recordings

---

## 2026-01-23 — Use smaller audio buffer size (1024 frames)

- **Decision:** Change audio tap buffer from 4096 to 1024 frames
- **Alternatives considered:**
  - 4096 frames (~256ms at 16kHz) - original
  - 512 frames (~32ms)
  - 2048 frames (~128ms)
- **Rationale:**
  - Smaller buffers arrive faster (first buffer in ~64ms vs ~256ms)
  - Combined with minimum recording duration, prevents 0-sample captures
  - Lower latency for real-time visualization if added later
- **Consequences:**
  - More buffer processing overhead (minimal impact)
  - Slightly more fragmented audio data

---

## 2026-01-23 — Use JSON Lines (JSONL) for structured logging

- **Decision:** Implement file-based logging using JSONL format
- **Alternatives considered:**
  - Plain text logs (hard to parse programmatically)
  - Binary log format (not human-readable)
  - Database logging (overkill, requires SQLite)
  - OSLog only (not easily accessible for AI analysis)
- **Rationale:**
  - JSONL is one JSON object per line, grep-friendly
  - Trivially parseable by AI tools and scripts
  - Human-readable with `cat` or `jq`
  - Standard format used by many log aggregators
- **Implementation:**
  - Location: `~/Library/Logs/Sagascript/` (standard macOS log location)
  - Rotation: Size-based (5MB), keep 5 files (max 25MB total)
  - Buffered writes with flush every 50 entries or 1 second
  - Session tracking with UUID per app session and dictation session
  - Dual output: JSONL to file + formatted console print
- **Consequences:**
  - Slight disk I/O overhead (mitigated by buffering)
  - Logs persist across sessions for debugging
  - Easy to correlate events across a dictation cycle

---

## 2026-01-23 — Enable auto-paste by default

- **Decision:** Change `autoPaste` default from `false` to `true`
- **Previous behavior:** Transcription copied to clipboard only; user must manually paste
- **New behavior:** Transcription is automatically pasted into active window via Cmd+V
- **Rationale:**
  - Users expect dictated text to appear where they're typing
  - Manual copy-paste defeats the purpose of hands-free dictation
  - Matches behavior of Wispr Flow and similar tools
  - Option still available in Settings for users who prefer clipboard-only
- **Consequences:**
  - Existing users with stored preferences are unaffected (their `autoPaste=false` persists)
  - New installs get the expected "dictate and paste" workflow
  - Existing users can delete preference: `defaults delete com.sagascript autoPaste`

---

## 2026-01-23 — WhisperKit Performance Optimization

- **Decision:** Optimize WhisperKit configuration for lowest latency
- **Problem:** Initial transcription was 1.3-1.6x RTF (slower than realtime)
- **Optimizations applied:**
  1. **Model prewarming** (`prewarm: true`) - Specializes CoreML models for ANE at load time
  2. **Full compute options** - melCompute, audioEncoderCompute, textDecoderCompute, prefillCompute
  3. **Greedy decoding** (`temperature: 0.0`) - Deterministic, no sampling overhead
  4. **Disabled quality checks** - Skip compression ratio, log prob, and no-speech thresholds
  5. **User-configurable model** - Settings UI to choose tinyEn, tiny, baseEn, base
  6. **Default model kept as `base`** - To avoid triggering download for users with cached models
- **Expected results:**
  - 5s audio: 0.3-0.5s (was 7-10s)
  - RTF: 0.06-0.1x (was 1.3-1.6x)
  - Model load: 2-3s (tinyEn), 4-5s (base with prewarm)
- **Rationale:**
  - Default conservative settings prioritize accuracy over speed
  - For dictation, speed is critical; users can re-record if needed
  - English-only models are more accurate for English text
- **Consequences:**
  - Slightly less accuracy with tiny models vs base
  - Quality thresholds disabled may accept some bad transcriptions
  - Model load slightly longer due to prewarming (but worth it for inference speed)

---

## 2026-01-23 — KB-Whisper Models for Swedish Transcription

- **Decision:** Add support for KB-Whisper models fine-tuned specifically for Swedish
- **Problem:** OpenAI's base Whisper model has 39.6% WER (Word Error Rate) on Swedish - essentially unusable for dictation
- **Solution:** Integrate KB-Whisper models from Kungliga Biblioteket (Swedish National Library)
- **KB-Whisper benefits:**
  | Model | Parameters | Swedish WER | vs OpenAI Base |
  |-------|-----------|-------------|----------------|
  | kb-whisper-tiny | 57.7M | 13.2% | ~3x better |
  | kb-whisper-base | 99.1M | 9.1% | **4x better** |
  | kb-whisper-small | 0.3B | 7.3% | 5x better |
- **Implementation:**
  1. Added `kbWhisperTiny`, `kbWhisperBase`, `kbWhisperSmall` to WhisperModel enum
  2. Added SwiftWhisper (whisper.cpp) as second transcription backend for Swedish models
  3. TranscriptionService routes to appropriate backend based on model type
  4. ModelDownloadService downloads GGML models from HuggingFace on first use
  5. Auto-model selection: when `autoSelectModel=true`, automatically uses kb-whisper-base for Swedish
  6. UI shows recommendation for Swedish users with standard models
- **Architecture:**
  - Standard models (tiny, base, etc.) → WhisperKit (CoreML/Neural Engine)
  - KB-Whisper Swedish models → whisper.cpp via SwiftWhisper (GGML)
  - Remote → OpenAI API
- **Model storage:**
  - Standard models: Downloaded by WhisperKit to its cache
  - KB-Whisper models: `~/Library/Application Support/Sagascript/Models/` (auto-downloaded)
- **Alternatives considered:**
  - Option 1: Convert KB-Whisper to CoreML (complex - whisperkittools not on PyPI, conversion non-trivial)
  - Option 3: Larger OpenAI multilingual models (still much worse than KB-Whisper on Swedish)
- **Rationale:**
  - KB-Whisper GGML models are pre-built and ready to download from HuggingFace
  - whisper.cpp is mature and well-tested
  - Auto-download provides seamless UX - no manual setup required
  - KB-whisper-base achieves 9.1% WER vs 39.6% for standard Whisper
- **Consequences:**
  - Two transcription backends to maintain (WhisperKit + whisper.cpp)
  - First Swedish transcription triggers ~60MB download
  - whisper.cpp may not leverage Neural Engine as well as WhisperKit (CPU-based)
- **Sources:**
  - [KB-Whisper Collection](https://huggingface.co/collections/KBLab/kb-whisper-67af9eafb24da903b63cc4aa)
  - [KB-Whisper Blog Post](https://kb-labb.github.io/posts/2025-03-07-welcome-KB-Whisper/)
  - [WhisperKit GitHub](https://github.com/argmaxinc/WhisperKit)

---

## 2026-01-24 — Remove main window, menu bar only (revised)

- **Decision:** Remove the main window scene; app is now menu bar only
- **Previous behavior:** App showed a main window on launch with status and transcription results
- **New behavior:** App runs only in menu bar; no dock icon, no main window
- **Rationale:**
  - Main window caused dock icon to appear (even with LSUIElement=true)
  - Redundant UI - MenuBarView already shows status, settings, and last transcription
  - Wispr Flow-style apps are menu bar utilities, not windowed apps
  - Window appearing on launch confused users about app purpose
- **Changes:**
  - Removed `Window("Sagascript", id: "main")` scene from SagascriptApp
  - Deleted MainWindowView.swift (now unused)
  - Settings still accessible via menu bar "Settings..." button
- **Consequences:**
  - No visible window on launch (expected for menu bar utilities)
  - Dock icon no longer appears
  - Users access all functionality via menu bar icon
- **Note:** This reverses the 2026-01-23 decision to show a main window on launch

---

## 2026-01-24 — Use NSViewRepresentable for hotkey recording (revised)

- **Decision:** Use NSViewRepresentable with first responder for hotkey capture in Settings
- **Problem:** Hotkey recorder showed "Press a key..." but never captured any key events
- **Root cause analysis:**
  - SwiftUI Button doesn't become first responder - nothing to receive keyboard events
  - Carbon's `GetApplicationEventTarget()` doesn't receive events routed to SwiftUI Settings windows in menu bar apps
  - NSEvent monitors fail because Settings windows in LSUIElement apps don't properly integrate with AppKit's responder chain
- **Alternatives considered:**
  - NSEvent.addLocalMonitorForEvents (no first responder to route events through)
  - NSEvent.addGlobalMonitorForEvents (only captures when app NOT active)
  - Carbon InstallEventHandler + GetApplicationEventTarget (Settings windows don't route through app event target)
  - SwiftUI .onKeyPress() (doesn't provide keyCode, needs focusable view)
  - CGEventTap (requires Input Monitoring permission - extra user friction)
- **Solution:** NSViewRepresentable wrapping a custom NSView that can become first responder
- **Rationale:**
  - NSView.keyDown() is called directly by AppKit when the view is first responder - no monitors needed
  - We explicitly make the view first responder when recording starts
  - This is similar to how [KeyboardShortcuts](https://github.com/sindresorhus/KeyboardShortcuts) uses NSSearchField
  - Full event access: NSEvent provides keyCode and modifierFlags
  - No extra permissions required (unlike CGEventTap)
- **Implementation:**
  - `KeyCaptureView`: NSView subclass with `acceptsFirstResponder=true`, overrides `keyDown()`
  - `KeyCaptureField`: NSViewRepresentable wrapper that manages first responder
  - When user clicks button, starts recording and makes KeyCaptureView first responder
  - Key events received directly via keyDown(), no event monitors needed
  - Removed CarbonKeyboardMonitor (~70 lines)
  - Existing `processKeyEvent()` logic unchanged (all 19 tests still pass)
- **Consequences:**
  - Simpler code (~55 lines vs ~70 lines for Carbon)
  - Reliable key capture by using AppKit's native first responder system
  - No deprecated APIs

---

## 2026-01-24 — CGEventTap Backend for Advanced Hotkeys

- **Decision:** Add CGEventTap-based hotkey backend for Fn and modifier-only shortcuts
- **Problem:** Carbon RegisterEventHotKey cannot handle:
  - Fn modifier (not represented in Carbon modifier flags)
  - Modifier-only triggers like "⌘ alone" (modifiers don't generate keyDown events)
- **Solution:** Dual-backend architecture with automatic selection
- **Implementation:**
  - **Shortcut model** (`Shortcut.swift`)
    - `kKeyCodeModifiersOnly = -1` sentinel for modifier-only shortcuts
    - `kModsFnBit = 1 << 16` custom bit for Fn (doesn't overlap Carbon bits)
    - Conversion functions between NSEvent, CGEvent, and stored modifier formats
  - **CGEventTapHotkeyService** (`CGEventTapHotkeyService.swift`)
    - Uses `.cgSessionEventTap` with `.listenOnly` for minimal privilege
    - "Tap-only" semantics: tracks `candidateActive` + `candidateCancelledByKey`
    - Only triggers modifier-only when no non-modifier key pressed during press-release cycle
    - Handles tap disabled by timeout/user input with re-enable
  - **HotkeyService facade** (`HotkeyService.swift`)
    - Selects backend based on `requiresCGEventTapBackend()`
    - Carbon backend for standard shortcuts (no extra permissions)
    - CGEventTap backend for Fn or modifier-only
  - **Recorder algorithm** (`HotkeyRecorderView.swift`)
    - Normal keys: Accept immediately on keyDown (not keyUp)
    - Modifier-only: Accept when all modifiers released AND no other key pressed
    - Prevents breaking combos like ⌘+Z
- **Permissions:**
  - Standard shortcuts: No extra permissions
  - Fn/modifier-only: Input Monitoring (shows system prompt + guidance alert)
  - Uses `CGPreflightListenEventAccess()` / `CGRequestListenEventAccess()`
- **Rationale:**
  - Apple DTS explicitly recommends CGEventTap for advanced hotkey scenarios
  - Dual-backend avoids forcing Input Monitoring permission on all users
  - "Tap-only" semantics prevent false triggers (user presses ⌘, then C → shouldn't trigger ⌘ alone)
- **Alternatives considered:**
  - CGEventTap only (forces Input Monitoring permission on everyone)
  - Accessibility API (same permission burden, more complex)
  - Double-tap detection (unreliable, timing-sensitive)
- **Consequences:**
  - Two backend implementations to maintain
  - Fn/modifier-only users need Input Monitoring permission
  - More comprehensive shortcut support (matches Wispr Flow capabilities)

---

## 2026-01-23 — Expert Review Security & Stability Fixes

- **Decision:** Address security and stability issues identified by senior code reviewers
- **Issues addressed:**

### HIGH PRIORITY (Security & Data Integrity)

1. **Transcript stdout printing** (AppController.swift:328-335)
   - Issue: Transcripts printed via `print()` could persist in macOS Diagnostics
   - Fix: Gated behind `#if DEBUG` - only prints in debug builds

2. **Blocking mic permission** (AudioCaptureService.swift:42-48)
   - Issue: `semaphore.wait()` blocked main thread, risking UI freeze and watchdog
   - Fix: Converted to async using `await AVCaptureDevice.requestAccess(for:)`

3. **Clipboard overwrite** (PasteService.swift:26-33)
   - Issue: `clearContents()` permanently destroyed previous clipboard
   - Fix: Save all clipboard types before paste, restore after 100ms delay

4. **Data loss on transcription failure** (AppController/AudioCaptureService)
   - Issue: Audio buffer cleared immediately, lost on failure
   - Fix: Audio retained in `lastCapturedAudio`, cleared only after success

5. **OpenAI backend vulnerabilities** (OpenAIBackend.swift)
   - Issues: Shared URLSession (caching), no timeouts, no size limits
   - Fixes: Ephemeral session, 60s timeout, 25MB check before upload

### MEDIUM PRIORITY (Performance & Safety)

6. **WAV encoding O(n) loop** (OpenAIBackend.swift:161-165)
   - Issue: Per-sample `append()` had high constant factor
   - Fix: Single `withUnsafeBufferPointer` copy

7. **Hardcoded 16 workers** (WhisperKitBackend.swift:222)
   - Issue: Over-subscribes on lower-core Macs (M1 Air = 8 cores)
   - Fix: Dynamic scaling: `min(16, max(2, ProcessInfo.activeProcessorCount / 2))`

8. **Unbounded audio buffer** (AudioCaptureService.swift:20-22)
   - Issue: 8-hour recording = 1.8GB RAM
   - Fix: 15-minute cap (14.4MB), warning logged when reached

9. **World-readable log files** (LoggingService.swift)
   - Issue: Default permissions allowed other users to read
   - Fix: 0o600 for files, 0o700 for directory

- **Rationale:** Expert review identified real security vulnerabilities and stability risks
- **Consequences:**
  - Clipboard restore adds ~100ms delay after paste
  - Buffer cap limits max recording to 15 minutes
  - Transcript viewing requires debug build or checking logs

---

## 2026-01-27 — Accuracy Strategy #1: Add small.en + large-v3-turbo Models

- **Decision:** Add `smallEn` (244M) and `largev3Turbo` (809M) as selectable WhisperKit models; change English default from `baseEn` to `smallEn`
- **Branch:** `feature/accuracy-1-model-upgrade` (commit `8a0cb2f`, 72 tests pass)
- **Alternatives considered:**
  - medium.en (622M) — slower than large-v3-turbo with worse accuracy
  - large-v3 (1.55B) — too large/slow for dictation latency targets
- **Rationale:**
  - `small.en` cuts English WER from ~15% to ~10% with only +200-400ms latency
  - `large-v3-turbo` (809M) has same encoder as large-v3 but only 4 decoder layers (same as tiny) — better accuracy AND faster than medium.en
  - Both fit comfortably on M4 MacBook Air (32GB)
- **HuggingFace identifiers:** `openai_whisper-small.en`, `openai_whisper-large-v3_turbo` (from argmaxinc/whisperkit-coreml)
- **Consequences:**
  - Larger model downloads (~460MB for small.en, ~600MB-1.6GB for large-v3-turbo)
  - First-time model load slower due to prewarming
  - Users who want lowest latency can still select tinyEn

---

## 2026-01-27 — Accuracy Strategy #2: Custom Vocabulary + Prompt Conditioning

- **Decision:** Add user-definable custom vocabulary and previous-context prompt conditioning via WhisperKit's `promptTokens` API
- **Branch:** `feature/accuracy-2-custom-vocabulary` (commit `051e7f2`, 80 tests pass)
- **Key discovery:** WhisperKit v0.15.0 does NOT have `initialPrompt: String?` on DecodingOptions. Instead uses `promptTokens: [Int]?` requiring tokenizer encoding
- **Implementation:**
  - `PromptBuilder` (stateless struct) — formats vocabulary + previous context into prompt, truncates to 896 chars
  - `SettingsManager` — added `customVocabulary: [String]` (JSON-encoded in UserDefaults) and `promptConditioningEnabled: Bool` (default true)
  - `WhisperKitBackend` — encodes prompt via `whisperKit.tokenizer.encode(text:)`, filters special tokens, sets `options.promptTokens`. Stores last 200 chars of transcription as context for next segment
- **Rationale:**
  - Whisper's prompt conditioning significantly improves recognition of domain-specific terms
  - Previous context helps maintain consistency across segments
  - Token-based approach (vs string) is what WhisperKit actually supports
- **Consequences:**
  - No Settings UI for vocabulary yet (backend only) — needs UI field in SettingsView
  - 896-char prompt limit prevents excessive token usage
  - Prompt conditioning can be disabled in settings

---

## 2026-01-27 — Accuracy Strategy #3: VAD + Audio Normalization

- **Decision:** Add audio preprocessing pipeline (normalization + silence trimming) before WhisperKit inference
- **Branch:** `feature/accuracy-3-vad` (commit `f002fc6`, 83 tests pass)
- **Implementation:**
  - `AudioProcessor` enum with static methods using Apple Accelerate/vDSP:
    - `normalize()` — scales peak to 1.0 using `vDSP_maxmgv` + `vDSP_vsmul`
    - `rmsEnergy()` — computes RMS via `vDSP_measqv`
    - `isSpeechPresent()` — RMS threshold check (default 0.01)
    - `trimSilence()` — windowed scan from both ends, removes leading/trailing silence
  - `WhisperKitBackend` — normalize → trim silence → skip if empty → transcribe
  - `noSpeechThreshold` changed from `nil` to `0.6`
- **Alternatives considered:**
  - Silero VAD (ONNX model, more accurate but adds dependency + complexity)
  - WebRTC VAD (C library, cross-platform but harder to integrate)
  - No VAD (current state, wastes inference on silence)
- **Rationale:**
  - Energy-based VAD is simple, zero-dependency, and effective for trimming silence
  - Accelerate/vDSP provides SIMD-optimized operations on Apple Silicon
  - Normalizing audio ensures consistent input levels regardless of mic gain
  - Trimming silence reduces inference time and prevents hallucinations on silent segments
- **Consequences:**
  - Adds ~1ms preprocessing overhead (negligible)
  - May clip very quiet speech if threshold too high (default 0.01 is conservative)
  - `noSpeechThreshold=0.6` may cause WhisperKit to return empty text for borderline audio

---

## 2026-01-24 — Fix Modifier-Only Hotkey Crash (UInt32 overflow)

- **Decision:** Remove all UInt32 casts for hotkeyKeyCode, use Int throughout
- **Problem:** App crashed when using modifier-only hotkeys like ⌘ alone
- **Root cause:** `kKeyCodeModifiersOnly = -1` (sentinel value) was being converted to `UInt32(-1)`, causing:
  > "Swift runtime failure: Negative value is not representable"
- **Locations fixed:**
  1. `AppController.swift:104` - Changed `UInt32(self.settingsManager.hotkeyKeyCode)` to Int
  2. `AppController.swift:559` - Changed `updateHotkey(keyCode: UInt32, modifiers: UInt32)` signature to Int
  3. `SettingsView.swift:82-83` - Removed UInt32 casts in `onHotkeyChanged` callback
  4. `HotkeyRecorderView.swift:208` - Added defensive check: if keyCode is a modifier key, force `kKeyCodeModifiersOnly`
- **Rationale:**
  - HotkeyService already has `register(keyCode: Int, modifiers: Int)` overload
  - Int can safely represent -1 sentinel; UInt32 cannot
  - Defensive check in recorder prevents storing hardware modifier keyCodes (like 55 for ⌘)
- **Consequences:**
  - Modifier-only hotkeys now work without crash
  - Type consistency: Int used throughout hotkey path
  - Tested with ⌘ alone, normal shortcuts (⌘+Z), and F-keys
