# Strategy #3: Voice Activity Detection (VAD) and Audio Normalization

## Research Findings

### WhisperKit Built-in VAD
WhisperKit (argmaxinc/WhisperKit) includes a built-in `VoiceActivityDetector` component. However, it is
tied into WhisperKit's internal pipeline and operates at the model level (post-mel-spectrogram). For our
use case, we want **pre-inference** audio processing to:

1. Skip sending silence-only audio to the model entirely (saves latency)
2. Normalize audio levels before inference (improves accuracy)
3. Trim leading/trailing silence (reduces unnecessary computation)

### Approach: Simple Energy-Based VAD
We use RMS (Root Mean Square) energy as the speech detection metric. This is computationally trivial
compared to ML-based VAD (e.g., Silero), and sufficient for our use case where we only need to:
- Detect if any speech is present at all
- Trim silence from the edges of the recording

We do NOT need:
- Real-time speech/silence segmentation during capture
- Speaker diarization
- Fine-grained speech boundaries

### Audio Normalization
Peak normalization scales all samples so the loudest sample reaches +/-1.0. This compensates for
varying microphone gain levels and distance, improving transcription accuracy across different setups.

## Implementation Plan

### 1. New File: `AudioProcessor.swift`
- `enum AudioProcessor` with static utility methods
- `normalize(_:)` - peak normalization to [-1, 1]
- `isSpeechPresent(in:threshold:)` - RMS energy check
- `rmsEnergy(of:)` - compute RMS energy
- `trimSilence(from:threshold:windowSize:)` - remove leading/trailing silence

### 2. Integration Point: `WhisperKitBackend.transcribe()`
- Before creating `DecodingOptions`, normalize and trim the audio
- If result is empty (all silence), return empty string early
- Set `noSpeechThreshold` to 0.6 for additional model-level filtering

### 3. What NOT to Change
- `AudioCaptureService.swift` - keep VAD in processing pipeline, not capture pipeline
- No external dependencies needed

## Design Decisions
- **Energy threshold: 0.01 RMS** - empirically reasonable for speech vs. ambient noise
- **Window size: 1600 samples (100ms at 16kHz)** - granular enough for speech boundaries
- **Minimum speech samples: 1600 (100ms)** - below this is likely a click/pop, not speech
- **Processing order: normalize first, then trim** - normalization makes threshold consistent
