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

## 2026-01-23 — Use Option+Space as default hotkey

- **Decision:** Default hotkey is Option+Space (⌥ Space)
- **Alternatives considered:**
  - Cmd+Shift+D (conflicts with some apps)
  - Double-tap Fn (not reliably capturable)
  - Ctrl+Space (conflicts with Spotlight on some configs)
- **Rationale:**
  - Option+Space is rarely used by other apps
  - Easy to press with one hand
  - Similar to common dictation shortcuts
- **Consequences:**
  - May conflict with some input methods
  - User can change in settings

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
