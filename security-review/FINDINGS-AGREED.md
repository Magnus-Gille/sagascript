# FlowDictate Security Review — Agreed Findings

**Reviewers:** Codex (gpt-5.3-codex) + Claude Code (claude-opus-4-6)
**Date:** 2026-02-09
**Rounds:** 3 (Codex R0 + Claude R1 + Codex R2 + Claude R3)
**Methodology:** Independent audit by Codex, cross-referenced and debated by Claude Code
**Scope:** Full codebase on `main` branch

## Summary

| Severity | Count | IDs |
|----------|-------|-----|
| **High** | 4 | F-001, F-002, F-003, F-004 |
| **Medium** | 4 | F-005, F-006, F-008, F-009 |
| **Low** | 7 | F-007, F-010, F-012, F-013, F-015, F-016, F-NEW-2 |
| **Info** | 4 | F-011, F-014, F-NEW-1, F-NEW-3 |
| **Total** | **19** | |

---

## High Severity

### F-001 — Model binaries downloaded without integrity verification
**Consensus Severity:** High (Codex originally: Critical; downgraded after debate)
**Code Location:** `Sources/FlowDictate/Services/ModelDownloadService.swift:105,116`, `Sources/FlowDictate/Services/WhisperCppBackend.swift:71`
**Description:** GGML model files are downloaded over HTTPS and loaded into whisper.cpp without SHA-256 hash verification. A compromised CDN or supply chain attack on the model hosting could deliver tampered model binaries, which are then parsed by native C code in a process with microphone and accessibility permissions.
**Scope:** Limited to the GGML/whisper.cpp path. WhisperKit models go through HubApi which has partial ETag-based integrity checks.
**Recommended Fix:**
1. Pin model artifacts to known SHA-256 hashes in the source code
2. Verify hash after download, before moving to final destination
3. Enforce expected file size as secondary check
4. Fail closed on mismatch

### F-002 — Dependency supply chain uses mutable branch + ignored lockfile
**Consensus Severity:** High
**Code Location:** `Package.swift:21`, `.gitignore:25`
**Description:** SwiftWhisper depends on `branch: "master"` — any upstream push changes what gets built. `Package.resolved` is gitignored, making builds non-reproducible. A single compromise of the upstream branch injects code into all builds.
**Recommended Fix:**
1. Pin SwiftWhisper to a specific commit hash or tagged release
2. Remove `Package.resolved` from `.gitignore` and commit it
3. Review dependency updates via PRs

### F-003 — Release build lacks hardened runtime and sandbox
**Consensus Severity:** High
**Code Location:** `scripts/build-app.sh:32`
**Description:** The build script ad-hoc signs with `codesign --force --deep --sign -` — no hardened runtime (`--options runtime`), no entitlements file, no notarization. Combined with microphone, accessibility, and input monitoring permissions, exploitation impact is amplified by weak process boundaries.
**Recommended Fix:**
1. Add `--options runtime` to codesign
2. Create explicit entitlements file listing only required capabilities
3. Stop using `--deep` (sign each binary individually)
4. Add notarization step for distribution builds
5. Eventually sign with Developer ID

### F-004 — "Local mode" performs network egress (documentation mismatch)
**Consensus Severity:** High
**Code Location:** `Sources/FlowDictate/Services/WhisperKitBackend.swift:148`, `Sources/FlowDictate/Services/ModelDownloadService.swift:105`, `docs/SECURITY_PRIVACY.md:68-69`
**Description:** Documentation states "No network requests made" and "Complete privacy" for local mode, but WhisperKit downloads models from Hugging Face on first use and ModelDownloadService downloads GGML files. Privacy-sensitive users may unintentionally leak IP/timing metadata.
**Recommended Fix:**
1. Distinguish "model setup" (may require network) from "transcription" (truly local)
2. Add explicit consent dialog before any model download
3. Provide true offline mode toggle
4. Update docs to accurately reflect network behavior

---

## Medium Severity

### F-005 — Clipboard-based paste exposes transcripts to clipboard snoopers
**Consensus Severity:** Medium (accepted industry practice)
**Code Location:** `Sources/FlowDictate/Services/PasteService.swift:38-39,69,128`
**Description:** Transcribed text is written to `NSPasteboard.general` before simulating Cmd+V, then restored after 100ms. Any clipboard-monitoring process can capture the text during this window.
**Note:** This is standard practice for dictation apps (Wispr Flow, Superwhisper) because direct AX insertion is incompatible with many apps (Electron, browsers). The 100ms restore window is tight and save/restore is implemented.
**Recommended Fix:** Offer AX-based insertion as optional high-privacy mode. Keep clipboard fallback as default.

### F-006 — Paste is global keystroke injection without target validation
**Consensus Severity:** Medium (debated, Claude initially Low-Medium, Codex defended Medium)
**Code Location:** `Sources/FlowDictate/Services/PasteService.swift:85-98`, `Sources/FlowDictate/Services/AppController.swift:332,361`
**Description:** CGEvent keyboard events are posted globally without verifying the frontmost app matches what the user intended. The transcription delay (0.3-3+ seconds) creates a window where focus could change. Auto-paste defaults to enabled.
**Recommended Fix:** Capture frontmost app PID/bundle before transcription; verify it matches before pasting. Optionally use targeted AX insertion.

### F-008 — OpenAI error response bodies logged verbatim
**Consensus Severity:** Medium
**Code Location:** `Sources/FlowDictate/Services/OpenAIBackend.swift:118-119`
**Description:** On non-200 responses, the raw response body is logged via os.log and the structured logging service. OpenAI error responses can include account metadata, request details, or other sensitive information.
**Recommended Fix:** Log only HTTP status code and a sanitized error type. Gate raw body logging behind an explicit debug flag with redaction.

### F-009 — CI workflow uses mutable GitHub Action tags
**Consensus Severity:** Medium
**Code Location:** `.github/workflows/ci.yml:15,18,23,44,47`
**Description:** Actions reference tag refs (`actions/checkout@v6`, `maxim-lobanov/setup-xcode@v1`, `actions/cache@v5`) instead of immutable commit SHAs. A compromised upstream tag could inject malicious code into CI.
**Recommended Fix:** Pin all third-party actions to commit SHAs. Add `permissions:` block to restrict GITHUB_TOKEN scope.

---

## Low Severity

### F-007 — Audio retained in memory for retry without expiry
**Consensus Severity:** Low (Codex originally Medium; downgraded after debate)
**Code Location:** `Sources/FlowDictate/Services/AudioCaptureService.swift:31,139`, `Sources/FlowDictate/Services/AppController.swift:356,453`
**Description:** Audio samples stored in `lastCapturedAudio` after capture for retry on failure. Cleared on successful transcription but retained indefinitely on failure. Swift prevents practical secure zeroization, and memory inspection requires elevated privileges.
**Recommended Fix:** Add TTL-based expiry (e.g., 30 seconds). Consider clearing on next recording start.

### F-010 — Persistent diagnostic logging creates behavior side-channels
**Consensus Severity:** Low
**Code Location:** `Sources/FlowDictate/Services/LoggingService.swift:4-5,109,131`, `Sources/FlowDictate/Services/AppController.swift:419`
**Description:** Structured JSONL logs include session IDs, durations, backend choice, language, audio sample counts, and result character counts. Written to `~/Library/Logs/FlowDictate/` with restrictive permissions (0o700 dir, 0o600 files). Transcription text is never logged.
**Recommended Fix:** Consider opt-in persistent logging for release builds. Add log retention controls.

### F-012 — Tests mutate production Keychain namespace
**Consensus Severity:** Low
**Code Location:** `Tests/FlowDictateTests/KeychainServiceTests.swift:10,12`, `Sources/FlowDictate/Services/KeychainService.swift:9`
**Description:** Unit tests use `KeychainService.shared` with the same `com.flowdictate.openai-api-key` service/account. Running tests on a dev machine with the app configured will delete the real API key.
**Recommended Fix:** Inject test-specific service/account strings or use a protocol-based mock.

### F-013 — Menu UI shows recent transcription text
**Consensus Severity:** Low
**Code Location:** `Sources/FlowDictate/Views/MenuBarView.swift:44-48`
**Description:** Menu dropdown shows `lastTranscription.prefix(50)`. Sensitive dictated text visible to shoulder surfers.
**Recommended Fix:** Add auto-clear after timeout. Consider "privacy mode" that hides transcript preview.

### F-015 — Privacy doc mismatch on audio retention (Codex R2 finding)
**Consensus Severity:** Low
**Code Location:** `docs/SECURITY_PRIVACY.md:51,53`, `Sources/FlowDictate/Services/AudioCaptureService.swift:19,139`
**Description:** Documentation claims "~30s max ring buffer" and immediate disposal, but code allows up to 15 minutes of audio and retains failed-capture audio for retry.
**Recommended Fix:** Update SECURITY_PRIVACY.md to accurately reflect the 15-minute buffer limit and retry retention behavior.

### F-016 — Clipboard not restored when AX permission denied (Codex R2 finding)
**Consensus Severity:** Low
**Code Location:** `Sources/FlowDictate/Services/PasteService.swift:35,38-39,44,56`
**Description:** When Accessibility permission is denied, `paste()` throws before reaching `scheduleClipboardRestore()`, leaving transcription text on the clipboard indefinitely. The original clipboard contents are lost.
**Recommended Fix:** Move clipboard restore to a `defer` block or ensure the AX check happens before writing to clipboard.

### F-NEW-2 — Debug builds print full transcription to stdout
**Consensus Severity:** Low
**Code Location:** `Sources/FlowDictate/Services/AppController.swift:341-348`
**Description:** `#if DEBUG` block prints complete transcription text via `print()`. Correctly gated, but `swift build` (without `-c release`) produces debug builds where all dictated text goes to stdout.
**Recommended Fix:** Use `os.log` with `.debug` level instead of `print()` for transcription output.

---

## Info

### F-011 — Keychain item uses default ACL
**Consensus Severity:** Info (Codex originally Low; conceded after debate)
**Code Location:** `Sources/FlowDictate/Services/KeychainService.swift:38-44`
**Description:** Keychain item uses `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` without explicit `SecAccessControlCreateWithFlags`. Current policy is appropriate — adding biometric requirements would break the UX for every transcription.
**Recommendation:** Document the access policy choice. Consider biometric protection only if API keys gain higher value.

### F-014 — Misleading NSAppleEventsUsageDescription
**Consensus Severity:** Info
**Code Location:** `AppBundle/Info.plist:23-24`
**Description:** `NSAppleEventsUsageDescription` text references "accessibility access to paste" but the app doesn't use Apple Events for pasting. The key may be unnecessary.
**Recommendation:** Remove if not used, or correct the description text.

### F-NEW-1 — ModelDownloadService uses URLSession.shared
**Consensus Severity:** Info (Claude originally Medium; conceded after debate)
**Code Location:** `Sources/FlowDictate/Services/ModelDownloadService.swift:105`
**Description:** Model downloads use `URLSession.shared` (with caching) instead of ephemeral session. Since models are intentionally persisted to disk, cache artifacts add no new information exposure.
**Recommendation:** Use ephemeral session for consistency with OpenAI backend, but low priority.

### F-NEW-3 — No TLS certificate pinning for OpenAI API
**Consensus Severity:** Info (Claude originally Medium; conceded after debate)
**Code Location:** `Sources/FlowDictate/Services/OpenAIBackend.swift:24-30`
**Description:** No custom URLSessionDelegate for certificate pinning. Apple's ATS provides standard TLS baseline. Pinning would break for users behind corporate TLS inspection proxies.
**Recommendation:** Consider as future hardening for high-security deployments. Not required for standard use.

---

## Prioritized Remediation Roadmap

### Immediate (before any public release)
1. **F-001** — Add SHA-256 hash verification for model downloads
2. **F-002** — Pin SwiftWhisper to commit hash, commit Package.resolved
3. **F-003** — Add hardened runtime to build script

### Short-term (first 1-2 updates)
4. **F-004** — Add model download consent, update privacy docs
5. **F-008** — Sanitize error response logging
6. **F-009** — Pin CI actions to SHAs
7. **F-016** — Fix clipboard restore on AX denial (defer block)
8. **F-015** — Update SECURITY_PRIVACY.md accuracy

### Medium-term (future releases)
9. **F-005/F-006** — Explore AX-based text insertion as privacy option
10. **F-007** — Add TTL-based audio retention expiry
11. **F-010** — Make persistent logging opt-in for release
12. **F-012** — Isolate test Keychain namespace
13. **F-013** — Add transcript auto-clear timeout
