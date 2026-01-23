# Model research (local + remote)

This doc seeds the agent’s research. Refine it as you validate in code/benchmarks.

## Local-first (recommended)

### Option A: WhisperKit (Core ML, Swift)
- Pros:
  - Built for Apple Silicon; uses Core ML acceleration
  - Designed for low-latency / streaming style workflows
  - Integrates naturally into Swift apps
- Cons:
  - Requires model conversion / distribution strategy
  - Needs careful memory management and warmup

What to do:
- Use WhisperKit as the primary local backend.
- Support multiple model sizes (small/medium recommended for Swedish+English).
- Add “warmup” on app launch or first hotkey press.

References:
- WhisperKit repo: https://github.com/argmaxinc/WhisperKit
- WhisperKit paper: https://openreview.net/forum?id=K4MXy4ZEPH

### Option B: whisper.cpp (Metal)
- Pros:
  - Mature, fast C/C++ implementation
  - Metal acceleration on macOS
  - CLI + library options
- Cons:
  - Integrating into Swift app may require bridging (FFI) or shipping a binary
  - Packaging and model download needs design

What to do:
- Keep as fallback backend if WhisperKit integration becomes hard, or for benchmarking.

Reference:
- https://github.com/ggerganov/whisper.cpp

### Option C (optional): Apple Speech frameworks
- Pros:
  - On-device speech recognition can be strong
  - Easy integration
- Cons:
  - Behavior varies; may require network depending on OS/features
  - Less control over models and latency tuning

Recommendation:
- Treat as optional experimental backend; keep it out of MVP unless it clearly wins.

## Remote (pay-as-you-go)

### OpenAI Audio Transcriptions API
Goal:
- Provide an optional remote backend for users who want higher accuracy or don’t want local model downloads.

Implementation notes:
- Must be behind a clean `TranscriptionBackend` interface.
- API key stored in Keychain, never logged.
- Provide an explicit toggle + disclosure.

Candidate models (verify latest):
- gpt-4o-transcribe (higher accuracy)
- gpt-4o-mini-transcribe (lower cost, lower latency)
- whisper-1 (legacy option)

References:
- OpenAI pricing: https://openai.com/api/pricing/
- Audio transcriptions guide: https://platform.openai.com/docs/guides/speech-to-text
- API reference: https://platform.openai.com/docs/api-reference/audio/createTranscription

## Benchmark plan (required)

Since your priority is latency, include a simple benchmark harness:
- Measure:
  - cold start (first transcription after launch)
  - warm start (after model loaded)
  - time-to-first-partial (if streaming)
  - time-to-final transcript
- Test with:
  - short utterance (1–3s)
  - medium (5–10s)
  - Swedish sample
  - English sample

Record results in `docs/BENCHMARKS.md`.

