# Status — FlowDictate

This file is the agent's running "project board".

## Current State
**COMPLETE** — All Definition of Done criteria met.

## Summary

FlowDictate is a complete macOS dictation app with:

1. **Menu bar application** using SwiftUI MenuBarExtra
2. **Global hotkey** (Control+Shift+Space default, configurable) using HotKey package
3. **Audio capture** via AVAudioEngine at 16kHz mono
4. **Local transcription** using WhisperKit (Apple Silicon optimized)
5. **Remote transcription** using OpenAI Whisper API
6. **Visual indicator** with floating NSPanel overlay
7. **Text paste** via clipboard + simulated Cmd+V
8. **Settings UI** with language, backend, and hotkey configuration (with recorder UI)
9. **Secure API key storage** in macOS Keychain
10. **Structured logging** with JSONL file output for debugging

## Repository

- GitHub: https://github.com/Magnus-Gille/flowdictate
- CI: GitHub Actions (macOS 14, build + test)
- Dependencies: WhisperKit, HotKey (via SPM)

## Files Created

### Source Code
- `Sources/FlowDictate/FlowDictateApp.swift` — App entry point
- `Sources/FlowDictate/Models/` — Language, AppState, AnyCodable, LogEvents
- `Sources/FlowDictate/Services/` — All core services including LoggingService
- `Sources/FlowDictate/Views/` — SwiftUI views

### Tests
- `Tests/FlowDictateTests/` — 29 unit tests

### Documentation
- `docs/PRD.md` — Product requirements
- `docs/ARCHITECTURE.md` — Architecture + diagrams
- `docs/NFRS.md` — Performance requirements
- `docs/SECURITY_PRIVACY.md` — Security + privacy
- `docs/TEST_PLAN.md` — Testing strategy
- `docs/DECISIONS.md` — Design decisions
- `docs/BENCHMARKS.md` — Performance benchmarks
- `docs/GITHUB_SETUP.md` — GitHub setup guide

### CI/CD
- `.github/workflows/ci.yml` — Build + test workflow
- `.github/dependabot.yml` — Dependency updates

## Definition of Done

All items complete:
- [x] Product functionality (7/7)
- [x] Backends (3/3)
- [x] Performance (2/2)
- [x] Quality (3/3)
- [x] Safety (2/2)

## How to Use

1. Clone the repository
2. Run `swift build` to build
3. Run `swift test` to run tests
4. Run `.build/debug/FlowDictate` to launch the app
5. Grant Microphone and Accessibility permissions when prompted
6. Press Control+Shift+Space to start dictating (configurable in Settings)
