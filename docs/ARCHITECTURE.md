# Architecture — FlowDictate

## Overview

FlowDictate is a macOS menu bar application built with Swift/SwiftUI. It follows a modular architecture with clear separation between audio capture, transcription backends, and text insertion.

```
┌─────────────────────────────────────────────────────────────────┐
│                         FlowDictate                              │
├─────────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐  │
│  │ Menu Bar UI │  │  Settings   │  │   Recording Overlay     │  │
│  │  (SwiftUI)  │  │  (SwiftUI)  │  │   (AppKit NSPanel)      │  │
│  └──────┬──────┘  └──────┬──────┘  └───────────┬─────────────┘  │
│         │                │                      │                │
│         └────────────────┼──────────────────────┘                │
│                          │                                       │
│                          ▼                                       │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                    AppController                           │  │
│  │  - Coordinates hotkey, recording, transcription, paste     │  │
│  └───────────────────────────────────────────────────────────┘  │
│         │              │              │              │           │
│         ▼              ▼              ▼              ▼           │
│  ┌──────────┐  ┌─────────────┐ ┌────────────┐ ┌────────────┐   │
│  │ Hotkey   │  │   Audio     │ │Transcribe  │ │   Paste    │   │
│  │ Service  │  │  Capture    │ │  Service   │ │  Service   │   │
│  └──────────┘  └─────────────┘ └────────────┘ └────────────┘   │
│        │              │              │              │            │
│        │              │              ▼              │            │
│        │              │    ┌─────────────────┐      │            │
│        │              │    │ Backend Protocol│      │            │
│        │              │    ├─────────────────┤      │            │
│        │              │    │┌───────────────┐│      │            │
│        │              │    ││  WhisperKit   ││      │            │
│        │              │    ││   (Local)     ││      │            │
│        │              │    │└───────────────┘│      │            │
│        │              │    │┌───────────────┐│      │            │
│        │              │    ││   OpenAI API  ││      │            │
│        │              │    ││   (Remote)    ││      │            │
│        │              │    │└───────────────┘│      │            │
│        │              │    └─────────────────┘      │            │
│        ▼              ▼                              ▼            │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                     macOS System                          │   │
│  │  Carbon Events │ AVAudioEngine │ Keychain │ Accessibility │   │
│  └──────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

## Components

### 1. FlowDictateApp (Entry Point)
- SwiftUI `@main` app with `MenuBarExtra`
- Initializes all services at launch
- Manages app lifecycle

### 2. AppController
- Central coordinator (singleton/environment object)
- Manages state machine: Idle → Recording → Transcribing → Idle
- Connects hotkey events to audio capture and transcription
- Publishes state for UI binding

### 3. HotkeyService
- Registers global keyboard shortcut using Carbon API (or HotKey package)
- Supports push-to-talk (key down/up) and toggle modes
- Notifies AppController of events
- Handles key conflicts gracefully

### 4. AudioCaptureService
- Uses `AVAudioEngine` with input node
- Captures audio at 16kHz mono (WhisperKit requirement)
- Streams to ring buffer for low-latency
- Provides audio data as `Data` or `[Float]` for transcription

### 5. TranscriptionService
- Protocol-based design for swappable backends
- Manages model loading/warm-up for local backend
- Async transcription with cancellation support

```swift
protocol TranscriptionBackend {
    func transcribe(audio: Data, language: Language) async throws -> String
    var isReady: Bool { get async }
    func warmUp() async throws
}
```

### 6. WhisperKitBackend
- Implements `TranscriptionBackend`
- Uses WhisperKit Swift package
- Loads model at app start or first use
- Runs inference on Neural Engine/GPU

### 7. OpenAIBackend
- Implements `TranscriptionBackend`
- Calls OpenAI Audio Transcription API
- Handles API key retrieval from Keychain
- Uses `gpt-4o-transcribe` or `whisper-1` model

### 8. PasteService
- Copies transcribed text to clipboard
- Simulates Cmd+V via CGEvent API
- Requires Accessibility permission
- Falls back to clipboard-only if permission denied

### 9. KeychainService
- Wrapper around Security framework
- Stores/retrieves API keys securely
- Never logs or exposes key values

### 10. SettingsManager
- Persists user preferences via `@AppStorage` / UserDefaults
- Exposes typed settings: hotkey, language, backend, mode

### 11. UI Components
- **MenuBarView**: Dropdown menu from menu bar icon
- **SettingsView**: SwiftUI settings window
- **RecordingOverlay**: Floating NSPanel/NSWindow for visual indicator

## Data Flow

### Dictation Flow (Push-to-talk)

```
┌─────────┐    keyDown     ┌─────────────┐
│  User   │ ──────────────▶│HotkeyService│
└─────────┘                └──────┬──────┘
                                  │
                                  ▼
                          ┌─────────────┐
                          │AppController│
                          └──────┬──────┘
                                 │ startRecording()
                                 ▼
          ┌─────────────────────────────────────────┐
          │           AudioCaptureService           │
          │  ┌─────────────────────────────────┐    │
          │  │ AVAudioEngine capturing audio   │    │
          │  └─────────────────────────────────┘    │
          └─────────────────────────────────────────┘
                                 │
     keyUp                       │ audio data accumulates
       │                         │
       ▼                         ▼
┌─────────────┐         ┌─────────────┐
│HotkeyService│ ───────▶│AppController│
└─────────────┘         └──────┬──────┘
                               │ stopRecording()
                               ▼
                    ┌────────────────────┐
                    │TranscriptionService│
                    └─────────┬──────────┘
                              │
           ┌──────────────────┴──────────────────┐
           ▼                                      ▼
    ┌─────────────┐                      ┌─────────────┐
    │WhisperKit   │         OR           │  OpenAI     │
    │(local)      │                      │  (remote)   │
    └──────┬──────┘                      └──────┬──────┘
           │                                     │
           └──────────────────┬──────────────────┘
                              │ transcribed text
                              ▼
                       ┌─────────────┐
                       │ PasteService│
                       └──────┬──────┘
                              │
                              ▼
                   ┌─────────────────────┐
                   │ Text in active app  │
                   └─────────────────────┘
```

## Key Trade-offs

### 1. Carbon API for Hotkeys vs. Pure Swift
**Decision:** Use Carbon API (or HotKey package wrapping it)
**Rationale:** Only reliable way to capture global hotkeys on macOS. Pure Swift/SwiftUI has no native support.
**Consequence:** Must link Carbon framework; slightly legacy API but stable.

### 2. AVAudioEngine vs. AudioQueue/AudioUnit
**Decision:** Use AVAudioEngine
**Rationale:** Modern, Swift-friendly, handles device changes automatically.
**Consequence:** Slightly higher-level abstraction; sufficient for our needs.

### 3. Clipboard+Cmd+V vs. Direct Text Insertion
**Decision:** Copy to clipboard and simulate Cmd+V
**Rationale:** Works universally across apps; direct insertion requires private APIs.
**Consequence:** Requires Accessibility permission; may briefly overwrite clipboard.

### 4. Floating Panel vs. Popover for Recording Indicator
**Decision:** Use NSPanel (floating window)
**Rationale:** More control over appearance and positioning; visible over all apps.
**Consequence:** Requires careful window level management; more code than popover.

### 5. WhisperKit vs. whisper.cpp
**Decision:** WhisperKit as primary local backend
**Rationale:** Pure Swift, optimized for Apple Silicon, uses Core ML.
**Consequence:** May need to bundle or download model files (~40-150MB depending on size).

## Module Dependencies

```
FlowDictateApp
├── AppController
│   ├── HotkeyService
│   ├── AudioCaptureService
│   ├── TranscriptionService
│   │   ├── WhisperKitBackend
│   │   │   └── WhisperKit (external)
│   │   └── OpenAIBackend
│   │       └── KeychainService
│   └── PasteService
├── SettingsManager
└── UI
    ├── MenuBarView
    ├── SettingsView
    └── RecordingOverlay
```

## Threading Model

- **Main thread**: All UI, state updates
- **Audio capture**: Dedicated audio thread managed by AVAudioEngine
- **Transcription**: Background task via Swift Concurrency (Task)
- **Hotkey events**: Main thread callbacks (Carbon)

All state mutations go through `@MainActor`-isolated `AppController`.

## Model Management

WhisperKit models are large (40-150MB). Strategy:

1. **Bundle small model** (tiny/base) for immediate use
2. **Download larger model** (small/medium) on first launch if user opts in
3. **Store in Application Support** directory
4. **Warm up model** at launch to avoid first-use latency

## Error Handling

| Error | Handling |
|-------|----------|
| Microphone permission denied | Show settings guidance in UI |
| Accessibility permission denied | Paste to clipboard only; warn user |
| WhisperKit model load failed | Retry once; show error in menu bar |
| OpenAI API error | Show error toast; suggest checking key |
| Network timeout | Show timeout message; suggest local mode |
