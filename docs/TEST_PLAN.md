# Test Plan — FlowDictate

## 1. Overview

This document outlines the testing strategy for FlowDictate, covering unit tests, integration tests, performance tests, and manual testing procedures.

## 2. Test Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     Test Pyramid                             │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│                    ┌───────────┐                            │
│                    │  Manual   │  ← Exploratory, E2E        │
│                    │   Tests   │                            │
│                    └───────────┘                            │
│               ┌─────────────────────┐                       │
│               │  Integration Tests  │  ← Audio → Transcribe │
│               │                     │    Paste Flow         │
│               └─────────────────────┘                       │
│        ┌─────────────────────────────────────┐              │
│        │           Unit Tests                 │  ← Services │
│        │  (70%+ coverage for core services)  │              │
│        └─────────────────────────────────────┘              │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

## 3. Unit Tests

### Test Framework
- XCTest (built-in)
- swift-testing (for async tests where applicable)

### Coverage Targets
- **Core services**: >70% line coverage
- **UI code**: Best effort (UI testing is separate)

### Unit Test Cases

#### HotkeyService Tests
| Test Case | Description |
|-----------|-------------|
| `testRegisterHotkey` | Hotkey registration succeeds |
| `testUnregisterHotkey` | Hotkey unregistration cleans up |
| `testHotkeyCallback` | Callback fires on simulated key event |
| `testConflictingHotkey` | Graceful handling when hotkey in use |

#### AudioCaptureService Tests
| Test Case | Description |
|-----------|-------------|
| `testStartCapture` | Audio engine starts without error |
| `testStopCapture` | Audio engine stops and returns data |
| `testAudioFormat` | Output is 16kHz mono Float32 |
| `testEmptyRecording` | Handles zero-length recording |
| `testPermissionDenied` | Proper error when mic permission denied |

#### TranscriptionService Tests
| Test Case | Description |
|-----------|-------------|
| `testBackendSelection` | Correct backend selected based on settings |
| `testLocalTranscription` | WhisperKit returns text for audio |
| `testRemoteTranscription` | OpenAI API returns text |
| `testEmptyAudio` | Handles empty audio input |
| `testCancellation` | Task cancellation stops transcription |

#### WhisperKitBackend Tests
| Test Case | Description |
|-----------|-------------|
| `testModelLoading` | Model loads successfully |
| `testWarmUp` | Warm-up completes without error |
| `testTranscribeEnglish` | English audio transcribed correctly |
| `testTranscribeSwedish` | Swedish audio transcribed correctly |
| `testLanguageDetection` | Auto-detect language works |

#### OpenAIBackend Tests
| Test Case | Description |
|-----------|-------------|
| `testAPIRequest` | Request formatted correctly |
| `testAPIResponse` | Response parsed correctly |
| `testNoAPIKey` | Error when key not configured |
| `testNetworkError` | Network errors handled gracefully |
| `testRateLimited` | 429 response handled |

#### PasteService Tests
| Test Case | Description |
|-----------|-------------|
| `testCopyToClipboard` | Text copied to pasteboard |
| `testSimulatePaste` | Cmd+V event generated |
| `testNoAccessibility` | Falls back to clipboard-only |
| `testEmptyText` | Handles empty string |

#### KeychainService Tests
| Test Case | Description |
|-----------|-------------|
| `testSaveKey` | API key saved to Keychain |
| `testRetrieveKey` | API key retrieved from Keychain |
| `testDeleteKey` | API key deleted from Keychain |
| `testUpdateKey` | Existing key updated |
| `testKeyNotFound` | Proper error when key doesn't exist |

#### SettingsManager Tests
| Test Case | Description |
|-----------|-------------|
| `testDefaultValues` | Settings have sensible defaults |
| `testPersistence` | Settings persist across instances |
| `testLanguageChange` | Language setting updates correctly |
| `testBackendChange` | Backend setting updates correctly |

### Mocking Strategy
- Protocol-based design enables mocking
- Mock backends for transcription tests
- Mock audio data for pipeline tests

```swift
class MockTranscriptionBackend: TranscriptionBackend {
    var transcribeResult: String = "mock result"
    var transcribeCalled = false

    func transcribe(audio: Data, language: Language) async throws -> String {
        transcribeCalled = true
        return transcribeResult
    }
}
```

## 4. Integration Tests

### Audio → Transcription → Paste Flow

| Test Case | Description |
|-----------|-------------|
| `testFullDictationFlow` | Record audio → transcribe → paste |
| `testPushToTalkMode` | Hold key → speak → release → text appears |
| `testToggleMode` | Press → speak → press → text appears |
| `testLanguageSwitching` | Switch language mid-session |
| `testBackendSwitching` | Switch local ↔ remote |

### Integration Test Fixtures
- Pre-recorded audio samples (English, Swedish)
- Expected transcription outputs
- Stored in `Tests/Fixtures/`

## 5. Performance Tests

### Latency Benchmarks

| Benchmark | Target | Method |
|-----------|--------|--------|
| `measureHotkeyLatency` | <100ms | Time from key event to capture start |
| `measureLocalTranscription` | <1000ms | Time for 5s audio transcription |
| `measurePasteLatency` | <100ms | Time from transcription to paste |
| `measureColdStart` | <3s | Time from app launch to ready |
| `measureWarmStart` | <500ms | Time with model preloaded |

### Memory Benchmarks

| Benchmark | Limit | Method |
|-----------|-------|--------|
| `measureIdleMemory` | <100MB | Memory after launch, idle |
| `measureRecordingMemory` | <150MB | Memory during recording |
| `measureTranscriptionMemory` | <500MB | Peak during transcription |

### How to Run Performance Tests
```bash
# Run performance tests
swift test --filter Performance

# Generate benchmark report
swift test --filter Performance 2>&1 | tee docs/BENCHMARKS.md
```

## 6. Manual Test Checklist

### First Launch
- [ ] App appears in menu bar
- [ ] Microphone permission requested
- [ ] Accessibility permission requested
- [ ] Settings accessible from menu

### Dictation (Push-to-talk)
- [ ] Hold Control+Shift+Space starts recording
- [ ] Menu bar icon changes to recording state
- [ ] Optional: HUD overlay appears
- [ ] Release key stops recording
- [ ] Text appears in active text field
- [ ] Icon returns to idle state

### Dictation (Toggle mode)
- [ ] First press starts recording
- [ ] Second press stops recording
- [ ] Transcription and paste work

### Language Support
- [ ] English transcription accurate
- [ ] Swedish transcription accurate
- [ ] Language setting changes work

### Backend Switching
- [ ] Local backend works offline
- [ ] Remote backend works with valid API key
- [ ] Error shown when API key invalid
- [ ] Clear feedback when backend changes

### Settings UI
- [ ] Hotkey can be changed
- [ ] Language can be changed
- [ ] Backend can be changed
- [ ] API key field is masked
- [ ] Settings persist after restart

### Error Handling
- [ ] Clear message when mic permission denied
- [ ] Clear message when accessibility denied
- [ ] Clear message when network fails (remote)
- [ ] App doesn't crash on errors

### Edge Cases
- [ ] Very short audio (<1s)
- [ ] Very long audio (>30s)
- [ ] Background noise only
- [ ] Switching apps during recording
- [ ] Multiple rapid dictations

## 7. Test Fixtures

### Audio Samples
Located in `Tests/Fixtures/Audio/`:

| File | Duration | Language | Content |
|------|----------|----------|---------|
| `english_short.wav` | 3s | English | "Hello, this is a test." |
| `english_medium.wav` | 10s | English | Longer paragraph |
| `swedish_short.wav` | 3s | Swedish | "Hej, det här är ett test." |
| `swedish_medium.wav` | 10s | Swedish | Longer paragraph |
| `silence.wav` | 3s | N/A | Silence only |
| `noise.wav` | 3s | N/A | Background noise |

### Expected Outputs
Located in `Tests/Fixtures/Expected/`:
- Corresponding `.txt` files with expected transcriptions

## 8. CI Configuration

### GitHub Actions Workflow
```yaml
name: Tests
on: [push, pull_request]
jobs:
  test:
    runs-on: macos-14
    steps:
      - uses: actions/checkout@v4
      - name: Build
        run: swift build
      - name: Run Tests
        run: swift test
      - name: Upload Coverage
        uses: codecov/codecov-action@v4
```

### CI Test Requirements
- All unit tests must pass
- No test flakiness (retry limit: 1)
- Coverage report generated

## 9. How to Run Tests

### All Tests
```bash
swift test
```

### Specific Test Suite
```bash
swift test --filter HotkeyServiceTests
swift test --filter AudioCaptureServiceTests
```

### With Verbose Output
```bash
swift test --verbose
```

### Generate Coverage Report
```bash
swift test --enable-code-coverage
xcrun llvm-cov report .build/debug/FlowDictatePackageTests.xctest/Contents/MacOS/FlowDictatePackageTests -instr-profile .build/debug/codecov/default.profdata
```

## 10. Test Environment Requirements

### For Unit Tests
- macOS 14.0+
- Xcode 15.0+ / Swift 5.9+
- No special permissions needed (mocked)

### For Integration Tests
- Microphone access (can use pre-recorded audio)
- Network access (for remote backend tests)

### For Manual Tests
- Physical microphone
- Accessibility permission granted
- (Optional) OpenAI API key for remote tests
