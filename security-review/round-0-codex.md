# FlowDictate Security & Privacy Review

## Summary Table

| ID | Severity | Title | Code Location |
|---|---|---|---|
| F-001 | **Critical** | Untrusted model binaries are downloaded and loaded without integrity verification | `Sources/FlowDictate/Services/ModelDownloadService.swift:105`, `Sources/FlowDictate/Services/WhisperCppBackend.swift:71` |
| F-002 | **High** | Dependency supply-chain is mutable (`master` branch + lockfile ignored) | `Package.swift:21`, `.gitignore:25` |
| F-003 | **High** | Release build process lacks hardened runtime and sandbox hardening | `scripts/build-app.sh:32` |
| F-004 | **High** | “Local mode” still performs network egress (privacy promise mismatch) | `Sources/FlowDictate/Services/WhisperKitBackend.swift:148`, `Sources/FlowDictate/Services/ModelDownloadService.swift:105`, `docs/SECURITY_PRIVACY.md:68` |
| F-005 | **Medium** | Clipboard-based paste exposes transcripts to clipboard snoopers | `Sources/FlowDictate/Services/PasteService.swift:38`, `Sources/FlowDictate/Services/PasteService.swift:69` |
| F-006 | **Medium** | Paste is global keystroke injection with no target validation | `Sources/FlowDictate/Services/PasteService.swift:97` |
| F-007 | **Medium** | Captured audio is retained in memory for retry with no expiry/secure wipe | `Sources/FlowDictate/Services/AudioCaptureService.swift:31`, `Sources/FlowDictate/Services/AudioCaptureService.swift:139`, `Sources/FlowDictate/Services/AppController.swift:453` |
| F-008 | **Medium** | OpenAI error response bodies are logged verbatim | `Sources/FlowDictate/Services/OpenAIBackend.swift:118`, `Sources/FlowDictate/Services/OpenAIBackend.swift:119` |
| F-009 | **Medium** | CI workflow uses mutable GitHub Action tags (not pinned to SHAs) | `.github/workflows/ci.yml:15`, `.github/workflows/ci.yml:18`, `.github/workflows/ci.yml:23` |
| F-010 | **Low** | Diagnostic telemetry is persisted by default, creating behavior side-channels | `Sources/FlowDictate/Services/LoggingService.swift:4`, `Sources/FlowDictate/Services/AppController.swift:419` |
| F-011 | **Low** | Keychain item policy relies on default ACL behavior | `Sources/FlowDictate/Services/KeychainService.swift:38` |
| F-012 | **Low** | Tests mutate the same Keychain namespace as production | `Tests/FlowDictateTests/KeychainServiceTests.swift:10`, `Sources/FlowDictate/Services/KeychainService.swift:9` |
| F-013 | **Low** | Menu UI leaks recent transcript text (shoulder-surfing side-channel) | `Sources/FlowDictate/Views/MenuBarView.swift:44` |
| F-014 | **Info** | Permission disclosure text is misleading for Apple Events key | `AppBundle/Info.plist:23` |

---

## Detailed Findings

### F-001
**Severity:** Critical  
**Title:** Untrusted model binaries are downloaded and loaded without integrity verification  
**Description and attack scenario:** The app downloads GGML model files over the network and directly loads them into `whisper.cpp` without checksum/signature verification. A compromised upstream model repo/CDN or malicious network path can deliver a tampered model. Since parsing is done by native code, this can become code execution in a process that also has mic/accessibility capabilities.  
**Code location:** `Sources/FlowDictate/Services/ModelDownloadService.swift:105`, `Sources/FlowDictate/Services/ModelDownloadService.swift:116`, `Sources/FlowDictate/Services/WhisperCppBackend.swift:71`  
**Recommended fix:** Pin model artifacts to trusted SHA-256 hashes (or signed manifest), verify before load, enforce expected file size/model ID, and fail closed on mismatch.

### F-002
**Severity:** High  
**Title:** Dependency supply-chain is mutable (`master` branch + lockfile ignored)  
**Description and attack scenario:** `SwiftWhisper` is pulled from `master`, and `Package.resolved` is ignored in git. Builds are not reproducible and can silently consume malicious upstream changes. A single compromise of upstream branch infrastructure can inject code into builds.  
**Code location:** `Package.swift:21`, `.gitignore:25`  
**Recommended fix:** Pin exact versions/commits, commit `Package.resolved`, and require reviewed dependency update PRs.

### F-003
**Severity:** High  
**Title:** Release build process lacks hardened runtime and sandbox hardening  
**Description and attack scenario:** Build script ad-hoc signs with `--deep` and no hardened runtime options. Combined with high-value permissions (microphone/accessibility/input monitoring), exploitation impact is larger because process hardening boundaries are weak.  
**Code location:** `scripts/build-app.sh:32`  
**Recommended fix:** Use Developer ID signing with hardened runtime (`--options runtime`), explicit entitlements, notarization, and avoid default `--deep` signing for production artifacts.

### F-004
**Severity:** High  
**Title:** “Local mode” still performs network egress (privacy promise mismatch)  
**Description and attack scenario:** Local backend can trigger network model downloads (WhisperKit/HuggingFace), while documentation claims local mode makes no network requests. Privacy-sensitive users may unintentionally leak metadata (IP, timing, usage).  
**Code location:** `Sources/FlowDictate/Services/WhisperKitBackend.swift:148`, `Sources/FlowDictate/Services/ModelDownloadService.swift:105`, `docs/SECURITY_PRIVACY.md:68`  
**Recommended fix:** Add explicit consent for model downloads, provide strict offline mode, and align UI/docs with actual network behavior.

### F-005
**Severity:** Medium  
**Title:** Clipboard-based paste exposes transcripts to clipboard snoopers  
**Description and attack scenario:** Transcribed text is written to global pasteboard before paste and restored later. Any clipboard-monitoring process can capture sensitive dictated text during this window.  
**Code location:** `Sources/FlowDictate/Services/PasteService.swift:38`, `Sources/FlowDictate/Services/PasteService.swift:69`, `Sources/FlowDictate/Services/PasteService.swift:128`  
**Recommended fix:** Prefer direct Accessibility insertion. Keep clipboard fallback optional and clearly labeled as lower privacy.

### F-006
**Severity:** Medium  
**Title:** Paste is global keystroke injection with no target validation  
**Description and attack scenario:** The app posts global `Cmd+V` events without binding to the intended target app/field. A malicious app can steal focus right before paste and receive dictated text.  
**Code location:** `Sources/FlowDictate/Services/PasteService.swift:97`  
**Recommended fix:** Capture intended frontmost app/element before transcribe and verify before insertion; use targeted AX insertion APIs instead of blind global key events.

### F-007
**Severity:** Medium  
**Title:** Captured audio is retained in memory for retry with no expiry/secure wipe  
**Description and attack scenario:** Audio is stored in `lastCapturedAudio` after capture and reused for retry. There is no TTL or secure zeroization. Post-failure memory inspection can recover sensitive speech.  
**Code location:** `Sources/FlowDictate/Services/AudioCaptureService.swift:31`, `Sources/FlowDictate/Services/AudioCaptureService.swift:139`, `Sources/FlowDictate/Services/AudioCaptureService.swift:154`, `Sources/FlowDictate/Services/AppController.swift:453`  
**Recommended fix:** Make retry retention opt-in, add strict expiration, and overwrite buffers before release.

### F-008
**Severity:** Medium  
**Title:** OpenAI error response bodies are logged verbatim  
**Description and attack scenario:** On non-200 responses, raw response body is logged. Upstream error content can include sensitive account/request metadata and can pollute logs.  
**Code location:** `Sources/FlowDictate/Services/OpenAIBackend.swift:118`, `Sources/FlowDictate/Services/OpenAIBackend.swift:119`  
**Recommended fix:** Log only sanitized status/error codes by default; gate raw body logging behind explicit debug mode with redaction.

### F-009
**Severity:** Medium  
**Title:** CI workflow uses mutable GitHub Action tags (not pinned to SHAs)  
**Description and attack scenario:** Workflow uses tag refs (`@v6`, `@v5`, `@v1`). If an upstream action tag is compromised, CI can execute malicious code and tamper build outputs.  
**Code location:** `.github/workflows/ci.yml:15`, `.github/workflows/ci.yml:18`, `.github/workflows/ci.yml:23`, `.github/workflows/ci.yml:44`, `.github/workflows/ci.yml:47`  
**Recommended fix:** Pin all third-party actions to immutable commit SHAs and restrict workflow token permissions.

### F-010
**Severity:** Low  
**Title:** Diagnostic telemetry is persisted by default, creating behavior side-channels  
**Description and attack scenario:** The app writes structured logs continuously, including session IDs, durations, backend, language, and result lengths. This enables local behavior profiling.  
**Code location:** `Sources/FlowDictate/Services/LoggingService.swift:4`, `Sources/FlowDictate/Services/AppController.swift:419`, `Sources/FlowDictate/Services/TranscriptionService.swift:68`  
**Recommended fix:** Disable persistent file logging by default in release, add explicit opt-in diagnostics and retention controls.

### F-011
**Severity:** Low  
**Title:** Keychain item policy relies on default ACL behavior  
**Description and attack scenario:** Keychain item sets accessibility but no explicit `kSecAttrAccessControl` policy. This depends on default ACL behavior and reduces explicit security posture for certification.  
**Code location:** `Sources/FlowDictate/Services/KeychainService.swift:38`  
**Recommended fix:** Use explicit access control policy (`SecAccessControlCreateWithFlags`) and document expected access behavior.

### F-012
**Severity:** Low  
**Title:** Tests mutate the same Keychain namespace as production  
**Description and attack scenario:** Unit tests call `deleteAPIKey()` on the same service/account used by the app. Running tests can erase real user API credentials on dev machines.  
**Code location:** `Tests/FlowDictateTests/KeychainServiceTests.swift:10`, `Tests/FlowDictateTests/KeychainServiceTests.swift:12`, `Sources/FlowDictate/Services/KeychainService.swift:9`  
**Recommended fix:** Inject test-specific Keychain service/account or mock Keychain backend in tests.

### F-013
**Severity:** Low  
**Title:** Menu UI leaks recent transcript text (shoulder-surfing side-channel)  
**Description and attack scenario:** The dropdown displays prefix of last transcription. Sensitive speech content can be exposed to anyone viewing screen/menu bar.  
**Code location:** `Sources/FlowDictate/Views/MenuBarView.swift:44`  
**Recommended fix:** Hide by default, add privacy mode, and auto-clear recent transcript preview quickly.

### F-014
**Severity:** Info  
**Title:** Permission disclosure text is misleading for Apple Events key  
**Description and attack scenario:** `NSAppleEventsUsageDescription` text references accessibility paste behavior, not Apple Events. This can mislead users and weaken compliance posture.  
**Code location:** `AppBundle/Info.plist:23`  
**Recommended fix:** Correct or remove unused Apple Events usage string and keep disclosures precise.