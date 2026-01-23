# FlowDictate Performance Benchmarks

This document records performance measurements for FlowDictate.

## Benchmark Instrumentation

The `BenchmarkService` class provides timing instrumentation for key operations:

- `hotkeyToRecordStart` — Time from hotkey press to audio capture start
- `recordingDuration` — Length of audio recording
- `transcriptionTime` — Time to transcribe audio to text
- `pasteTime` — Time to paste text into active application
- `totalDictationTime` — End-to-end time from hotkey to text pasted

## How to Generate Benchmarks

1. Build and run the app in debug mode
2. Perform multiple dictation cycles
3. Check Console.app for benchmark logs (category: "Benchmark")
4. Or call `BenchmarkService.shared.generateReport()` to get markdown output

## Recorded Measurements

### Initial Build — 2026-01-23

**Note:** These benchmarks need to be recorded on actual target hardware. The measurements below are placeholders based on design targets.

**Environment:**
- Date: 2026-01-23
- Hardware: MacBook Air M4, 32GB RAM (target)
- macOS: 14.x (Sonoma)
- Backend: Local (WhisperKit)
- Model: whisper-base

**Design Targets (from NFRS.md):**

| Metric | Target | Maximum |
|--------|--------|---------|
| Hotkey to record start | 50ms | 100ms |
| Local transcription (≤5s audio) | 500ms | 1000ms |
| Local transcription (5-15s audio) | 1000ms | 2000ms |
| Paste time | 50ms | 100ms |
| Total (short utterance) | 700ms | 1500ms |

**Actual Measurements:**

*To be recorded on first run on target hardware.*

| Metric | Count | Avg (ms) | Min (ms) | Max (ms) | P50 (ms) | P95 (ms) | P99 (ms) |
|--------|-------|----------|----------|----------|----------|----------|----------|
| hotkeyToRecordStart | - | - | - | - | - | - | - |
| transcriptionTime | - | - | - | - | - | - | - |
| pasteTime | - | - | - | - | - | - | - |
| totalDictationTime | - | - | - | - | - | - | - |

## Model Load Time

WhisperKit model loading is performed at app startup. Typical times:

| Model | Expected Load Time |
|-------|-------------------|
| tiny | ~1-2s |
| base | ~2-4s |
| small | ~5-10s |
| medium | ~15-30s |

## Memory Usage

| State | Expected Memory |
|-------|-----------------|
| Idle (model loaded) | ~200-400MB |
| Recording | +50MB |
| Transcribing | +100-200MB peak |

## Notes

- First transcription after model load may be slightly slower due to warm-up
- Remote (OpenAI) backend adds network latency (~200-500ms typical)
- Performance varies with audio length and complexity
- Neural Engine utilization significantly improves local transcription speed

## Appendix: Running Benchmarks

```swift
// In debug builds, benchmarks are automatically logged
// To generate a report programmatically:
let report = BenchmarkService.shared.generateReport()
print(report)
```
