# Decisions Log

Record assumptions and design decisions here, with rationale and dates.

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
  - Location: `~/Library/Logs/FlowDictate/` (standard macOS log location)
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
  - Existing users can delete preference: `defaults delete com.flowdictate autoPaste`

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
