# Strategy #1: Add small.en + large-v3-turbo Models

## Research Findings

### Model: small.en (openai/whisper-small.en)
- **Parameters**: 244M
- **Type**: English-only
- **WhisperKit CoreML identifier**: `openai_whisper-small.en`
- **HuggingFace repo**: `argmaxinc/whisperkit-coreml` (standard repo, no local path needed)
- **Expected performance**: More accurate than base.en (74M), moderate latency (~0.5s for 5s audio)
- **Compressed variant also available**: `openai_whisper-small.en_217MB`

### Model: large-v3-turbo (openai/whisper-large-v3-turbo)
- **Parameters**: 809M (pruned from large-v3's 1.54B by reducing decoder layers from 32 to 4)
- **Type**: Multilingual
- **WhisperKit CoreML identifier**: `openai_whisper-large-v3_turbo` (note: underscore before "turbo")
- **HuggingFace repo**: `argmaxinc/whisperkit-coreml` (standard repo, no local path needed)
- **Expected performance**: Near large-v3 accuracy with 6x faster inference, ~0.8s for 5s audio
- **Compressed variant also available**: `openai_whisper-large-v3_turbo_954MB`

### WhisperKit Compatibility
Both models are available as CoreML conversions in the `argmaxinc/whisperkit-coreml` repository on HuggingFace. They follow the standard WhisperKit model loading path and do not require local model files or GGML conversion.

## Implementation Plan

### Changes to `Sources/FlowDictate/Models/Language.swift`

1. Add two new enum cases to `WhisperModel`:
   - `case smallEn = "small.en"` (244M params, English only)
   - `case largev3Turbo = "large-v3-turbo"` (809M params, multilingual)

2. Update all computed properties (switch statements):
   - `displayName`: "Small (English)" / "Large V3 Turbo (Multilingual)"
   - `description`: With size/speed hints
   - `isEnglishOnly`: smallEn=true, largev3Turbo=false
   - `isSwedishOptimized`: both false
   - `parameterCount`: 244 / 809
   - `requiresLocalPath`: both false (standard HuggingFace models)
   - `modelName`: "openai_whisper-small.en" / "openai_whisper-large-v3_turbo"
   - `ggmlFilename`: both ""
   - `ggmlDownloadURL`: both nil
   - `ggmlSizeMB`: both 0

3. Update `standardModels` to include smallEn and largev3Turbo

4. Update `recommendedModel(for:)`:
   - English: `.smallEn` (upgraded from `.baseEn`)
   - Swedish: `.kbWhisperBase` (unchanged)
   - Auto: `.base` (unchanged)

### Changes to Tests
- Add tests for new model metadata, WhisperKit identifiers, and updated recommendations
- Update allCases count from 7 to 9
- Update standardModels count from 4 to 6

### No Changes Needed
- `SettingsManager.swift` - No model-specific logic to update
- KB-Whisper models - Unchanged
- Audio pipeline - Model loading is generic, works with any WhisperKit model

## Risk Assessment
- **Low risk**: Both models use standard WhisperKit model loading (HuggingFace download)
- **Memory**: large-v3-turbo at 809M params (~3.1GB CoreML) may be tight on 8GB Macs
- **Mitigation**: Model selection is user-driven; descriptions include size hints
