# Claude Code Security Review — Round 3 (Final)

We have strong convergence on 14 of 18 findings. Three minor severity disputes and two new findings from Codex remain. Let me resolve everything.

## Remaining Disputes

### F-006: Paste target validation — ACCEPT Medium

Codex makes a fair point in round 2: the window between transcription completion and paste is not just milliseconds. Looking at the code flow:

1. `stopRecordingAndTranscribe()` starts async transcription (AppController.swift:326)
2. Transcription takes 0.3-3+ seconds
3. On completion, `pasteService.paste(text:)` is called (AppController.swift:361)

During step 2, the user could switch apps, or another app could steal focus. Codex is right that `autoPaste` defaults to `true` (SettingsManager.swift:17), making this the common code path.

**I accept Medium.** My original "Low-Medium" was too low. The transcription delay creates a meaningful window.

### F-NEW-1: URLSession.shared for model downloads — ACCEPT Info

Codex's counter-argument is valid: the model files are intentionally persisted to disk (`ModelDownloadService.swift:116`), so URL cache artifacts revealing "which model was downloaded" add no new information beyond what the persistent model file already reveals. The caching concern is moot.

**I concede to Info.** This is a consistency nit, not a real finding.

### F-NEW-3: No certificate pinning for OpenAI — ACCEPT Info

Codex is right that TLS certificate pinning for third-party APIs is a hardening measure, not a standard requirement. Apple's ATS provides a reasonable baseline. Pinning would also break for users behind corporate TLS inspection proxies, creating a support burden.

**I concede to Info.** This is a hardening note for future consideration if the app targets high-security environments.

## New Findings from Codex Round 2

### F-015: Privacy doc mismatch on audio retention — AGREE (Low)

Verified. `SECURITY_PRIVACY.md:51` says "~30s max ring buffer" but actual code uses 15 minutes (`AudioCaptureService.swift:19`). The retry retention (`lastCapturedAudio`) is also not mentioned in the privacy docs. This is a documentation accuracy issue.

### F-016: Clipboard retention not always brief — AGREE (Low)

Verified. When AX permission is denied, `PasteService.paste()` throws at line 56 *before* reaching `scheduleClipboardRestore()` at line 69. However, looking more carefully at the code flow: `saveClipboard()` is called at line 35, then clipboard is written at line 38-39, then AX check happens at line 44. If AX is denied, the throw at line 56 means `scheduleClipboardRestore()` is never called, leaving the transcription text on the clipboard indefinitely. The SECURITY_PRIVACY.md claim of "brief clipboard use" is inaccurate for this path.

Wait — actually re-reading `PasteService.swift` more carefully: the clipboard *save* happens at line 35, the clipboard *write* (with transcription text) at 38-39, then the AX check at 44. If AX fails, the method throws at 56. The `scheduleClipboardRestore()` is only called in the success path (line 69) or the catch block (line 75). Since the AX denial throws *before* the do/try/catch block at line 60, the restore IS skipped. Codex is right — this leaves transcription text on clipboard permanently in the no-AX case.

**Confirmed: Low severity, good catch.**

## Consensus Status — ALL RESOLVED

All findings now have agreed severity levels. No further rounds needed.
