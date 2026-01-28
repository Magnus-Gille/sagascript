# Strategy 2: Custom Vocabulary / Initial Prompt Conditioning

## Research Findings

### WhisperKit API for Prompt Conditioning
- WhisperKit `DecodingOptions` uses `promptTokens: [Int]?` (not `initialPrompt: String`)
- Text must be tokenized via `whisperKit.tokenizer.encode(text:)`
- Special tokens must be filtered: `.filter { $0 < tokenizer.specialTokens.specialTokenBegin }`
- The `usePrefillPrompt: true` option (already set) enables prompt conditioning
- Whisper's decoder accepts up to 224 tokens as initial prompt context

### Known Issues
- GitHub issue #372 reports empty results with promptTokens on large-v3 models
- Smaller models (tiny, base) may work better with prompt conditioning
- We should make this feature toggleable so users can disable if it causes issues

### Design Decisions
- `PromptBuilder` works at the string level (pure logic, easily testable)
- Tokenization happens in `WhisperKitBackend` where the tokenizer is available
- Token limit approximated as ~4 chars per token = 896 character max for prompt text
- Vocabulary stored as `[String]` in UserDefaults (JSON-encoded)
- Previous transcription context stored in-memory in WhisperKitBackend (not persisted)
- Empty vocabulary + no context = nil prompt (zero overhead)

## Implementation Plan

### 1. PromptBuilder (new file)
- `buildPrompt(vocabulary:previousContext:) -> String?`
- Formats: "Vocabulary: term1, term2, term3. [Previous context]"
- Truncates to 896 characters max
- Returns nil for empty inputs

### 2. SettingsManager additions
- `customVocabulary: [String]` via UserDefaults JSON encoding
- `promptConditioningEnabled: Bool` default true (AppStorage)
- Reset logic updated

### 3. WhisperKitBackend integration
- Store `lastTranscriptionContext: String?` for previous context
- In `transcribe()`, build prompt via PromptBuilder
- Tokenize prompt text using WhisperKit's tokenizer
- Set `options.promptTokens` with the encoded tokens
- Update `lastTranscriptionContext` after each successful transcription

### 4. Tests
- PromptBuilder unit tests (pure logic)
- SettingsManager tests for new properties
