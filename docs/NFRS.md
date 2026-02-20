# Non-functional Requirements (NFRs) — Sagascript

## 1. Performance & Latency

### Latency Budgets

| Phase | Target | Maximum | Notes |
|-------|--------|---------|-------|
| Hotkey to mic capture start | 50ms | 100ms | System overhead + audio engine setup |
| Audio capture (streaming) | Real-time | Real-time | No buffering delay during recording |
| Stop to transcription start | 50ms | 100ms | Audio finalization + model invocation |
| Local transcription (short utterance ≤5s) | 500ms | 1000ms | WhisperKit on M4/M1+ |
| Local transcription (medium utterance 5-15s) | 1000ms | 2000ms | |
| Remote transcription (short utterance) | 500ms | 1500ms | Network RTT + API processing |
| Text paste | 50ms | 100ms | Clipboard + Cmd+V simulation |
| **Total: hotkey release to text pasted** | **700ms** | **1500ms** | For short local transcription |

### Throughput
- Single concurrent dictation (no parallelism needed)
- Model warm-up at app launch (not per-request)

### Resource Usage

| Resource | Budget | Notes |
|----------|--------|-------|
| Idle CPU | < 1% | Menu bar presence only |
| Recording CPU | < 10% | Audio capture overhead |
| Transcription CPU | < 50% peak | Leverages Neural Engine/GPU |
| Idle Memory | < 100MB | Model can be unloaded if memory pressure |
| Loaded Memory | < 500MB | With medium WhisperKit model |
| Disk (model storage) | ~150MB | For small/medium model |

### Battery Impact
- Idle: Negligible
- Recording: Low (audio only)
- Transcription: Moderate spike, short duration
- Goal: No perceptible battery drain for typical usage patterns

## 2. Reliability

### Availability
- App should launch successfully 99.9% of the time
- Dictation should complete successfully 99% of the time (excluding user errors like empty audio)

### Error Recovery

| Scenario | Recovery |
|----------|----------|
| Audio capture fails | Retry once; show error indicator |
| Model fails to load | Retry with smaller model; offer remote fallback |
| Transcription throws error | Show error message; allow retry |
| Paste fails | Copy to clipboard; show "Copied" message |
| App crash | macOS will show crash dialog; user relaunches |

### Graceful Degradation
1. **No Accessibility permission**: Paste to clipboard only, prompt user
2. **No Microphone permission**: Show clear error, link to System Preferences
3. **Model download fails**: Use bundled small model
4. **Network unavailable (remote mode)**: Error message, suggest local mode

## 3. Scalability

Not applicable for MVP — single-user desktop app.

## 4. Security & Privacy

See `SECURITY_PRIVACY.md` for full details.

### Summary NFRs
- API keys stored in Keychain, never in plaintext
- Audio data kept in memory only, discarded after transcription
- No telemetry or analytics
- TLS required for any network communication
- Minimal permissions requested

## 5. Usability

### Accessibility
- VoiceOver support for settings UI
- Keyboard navigation in settings
- High contrast mode compatible

### Internationalization
- UI language: English (MVP)
- Transcription languages: English, Swedish

### Response Time Perception
- Recording indicator appears within 100ms of hotkey
- User should never wonder "is it working?"

## 6. Maintainability

### Code Quality
- Swift 5.9+ with strict concurrency checking
- Modular architecture (see `ARCHITECTURE.md`)
- Protocol-based backends for extensibility
- Comprehensive inline documentation for complex logic

### Testing
- Unit test coverage for core services (>70%)
- Integration tests for audio → transcription → paste flow
- No flaky tests

### Build Time
- Full build: < 2 minutes on M1+ Mac
- Incremental build: < 30 seconds

## 7. Portability

### Platform Support
- macOS 14.0 (Sonoma) minimum
- Apple Silicon (M1/M2/M3/M4) required for optimal WhisperKit performance
- Intel Macs: Not officially supported but may work with reduced performance

### Dependencies
- Swift Package Manager for all dependencies
- WhisperKit: Apache 2.0 license
- No proprietary dependencies

## 8. Observability

### Logging
- Structured logging via `os.Logger`
- Log levels: debug, info, warning, error
- No sensitive data in logs (audio, API keys)
- Debug logs disabled in release builds

### Metrics (MVP)
- Transcription latency logged (debug only)
- Error counts logged

### Future (Post-MVP)
- Optional opt-in anonymous usage metrics
- Performance dashboard

## 9. Deployment

### Distribution
- Direct download (DMG or ZIP) from GitHub Releases
- Future: Mac App Store

### Updates
- Manual update check via GitHub API
- Future: Sparkle framework for auto-updates

### Signing & Notarization
- Requires Apple Developer account (document setup steps)
- DMG must be notarized for Gatekeeper
- If no signing available: document how to allow unsigned app

## 10. Compliance

### Data Protection
- GDPR considerations: No user data collected
- Local-first: Audio processed on-device
- Remote: User explicitly opts in; OpenAI's DPA applies

### Open Source Licenses
- All dependencies must have compatible licenses (MIT, Apache 2.0, BSD)
- License file included in distribution
